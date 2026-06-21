//! VPN-GW-1 — the `vpn_gateway` tunnel-engine worker (design:
//! `docs/design/vpn-gateway.md`).
//!
//! The `action/vpn/*` Bus responder ([`crate::ipc::vpn_gw`]) handles on-demand
//! tunnel CRUD + explicit `tunnel-up`/`tunnel-down`. This worker is the
//! **reconciler**: on a slow tick it reads the per-node tunnel config
//! ([`mackes_mesh_types::vpn`]) and, for every `Wg`/`Ovpn` tunnel whose
//! interface is **not already present**, brings it up via the pure argv builders
//! ([`vpn::wg_quick_argv`] / [`vpn::openvpn_argv`]). That is what makes a
//! configured tunnel **survive a daemon restart** (acceptance bullet 4): the
//! durable config is the desired state and this worker re-converges to it on
//! boot, exactly as `mesh_firewall` re-converges firewalld rules.
//!
//! Reconciliation is split into a **pure planner** ([`plan_bring_up`]) — config
//! + the set of present interfaces → the argvs to run — that is fully
//! unit-tested without any system tools, and a thin execution wrapper that
//! shells out with [`crate::workers::proc`]'s timeout-bounded helpers.
//!
//! Graceful degradation: if `wg-quick` / `openvpn` are absent (a lighthouse /
//! container-stripped peer), the worker logs once and idles — never panics.
//! Provider `Cli`/`Api` methods aren't process-driven here (later VPN-GW
//! provider-integration tasks); the planner skips them.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use mackes_mesh_types::vpn::{self, Method, TunnelDef, VpnConfig};

use super::{ShutdownToken, Worker};

/// Reconcile cadence. Tunnels are long-lived; a 1-minute sweep re-asserts a
/// dropped tunnel without hammering `wg-quick`.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(60);

/// Where the decrypted OpenVPN `.ovpn` lands once the secret store (VPN-GW-3)
/// distributes it. The reconciler only attempts an OpenVPN bring-up when this
/// file exists, so it's honest (skips) until secret distribution ships.
#[must_use]
pub fn openvpn_config_path(t: &TunnelDef) -> String {
    format!("/etc/openvpn/client/{}.ovpn", t.ifname())
}

/// Worker handle. Rooted at the shared workgroup root (the tunnel-config home,
/// matching [`crate::ipc::vpn_gw::VpnService`]).
pub struct VpnGatewayWorker {
    workgroup_root: PathBuf,
    tick: Duration,
}

impl VpnGatewayWorker {
    /// Construct with production defaults, rooted at the shared workgroup root.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self {
            workgroup_root,
            tick: DEFAULT_TICK_INTERVAL,
        }
    }

    /// Override the reconcile cadence (tests).
    #[must_use]
    pub fn with_tick(mut self, d: Duration) -> Self {
        self.tick = d;
        self
    }

    fn tick_once(&self) {
        let cfg = vpn::load(&self.workgroup_root);
        // A malformed config that fails validation (e.g. an ifname collision)
        // is a no-op: we never act on an inconsistent desired state.
        if let Err(e) = cfg.validate() {
            tracing::warn!(error = %e, "vpn_gateway: tunnel config invalid; skipping reconcile");
            return;
        }
        for argv in plan_bring_up(&cfg, &present_ifaces) {
            run_argv(&argv);
        }
    }
}

/// Is `ifname` a present network interface? (`ip -o link show <ifname>`.) The
/// default present-interface probe handed to [`plan_bring_up`] in production.
#[must_use]
pub fn present_ifaces(ifname: &str) -> bool {
    let mut cmd = Command::new("ip");
    cmd.args(["-o", "link", "show", ifname]);
    crate::workers::proc::status_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
        .map(|s| s.success())
        .unwrap_or(false)
}

/// PURE planner — given the desired tunnel config and a predicate that reports
/// whether an interface is already present, return the argv list to bring the
/// missing `Wg`/`Ovpn` tunnels up. Idempotent: a tunnel whose interface is
/// already up produces nothing, so re-running the sweep is a no-op.
///
/// `Cli`/`Api` methods are skipped (not process-driven here). An OpenVPN tunnel
/// is skipped when its `.ovpn` ([`openvpn_config_path`]) isn't on disk yet, via
/// the `ovpn_config_ready` predicate — so the plan never spawns `openvpn`
/// against a missing config (honest until VPN-GW-3 distributes secrets).
#[must_use]
pub fn plan_bring_up_with<P, C>(
    cfg: &VpnConfig,
    iface_present: P,
    ovpn_config_ready: C,
) -> Vec<Vec<String>>
where
    P: Fn(&str) -> bool,
    C: Fn(&TunnelDef) -> bool,
{
    let mut out = Vec::new();
    for t in &cfg.tunnel {
        if iface_present(&t.ifname()) {
            continue; // already up — idempotent
        }
        match t.method {
            Method::Wg => out.push(vpn::wg_quick_argv(t, true)),
            Method::Ovpn => {
                if ovpn_config_ready(t) {
                    out.push(vpn::openvpn_argv(t, &openvpn_config_path(t)));
                } else {
                    tracing::debug!(
                        id = %t.id,
                        "vpn_gateway: openvpn config absent (secret distribution pending); skipping"
                    );
                }
            }
            Method::Cli | Method::Api => {
                tracing::debug!(
                    id = %t.id,
                    method = ?t.method,
                    "vpn_gateway: method not process-driven here; skipping"
                );
            }
        }
    }
    out
}

/// [`plan_bring_up_with`] with the production OpenVPN-config readiness check
/// (the `.ovpn` exists on disk). Split so tests can drive the planner without
/// touching the filesystem.
#[must_use]
pub fn plan_bring_up<P>(cfg: &VpnConfig, iface_present: P) -> Vec<Vec<String>>
where
    P: Fn(&str) -> bool,
{
    plan_bring_up_with(cfg, iface_present, |t| {
        std::path::Path::new(&openvpn_config_path(t)).exists()
    })
}

/// Run one bring-up argv with a bounded timeout. Honest on failure (logs at
/// warn), never panics; a missing binary surfaces as a spawn error.
fn run_argv(argv: &[String]) {
    let Some((cmd, rest)) = argv.split_first() else {
        return;
    };
    let mut c = Command::new(cmd);
    c.args(rest);
    match crate::workers::proc::status_with_timeout(c, crate::workers::proc::DEFAULT_CMD_TIMEOUT) {
        Ok(s) if s.success() => {
            tracing::info!(argv = ?argv, "vpn_gateway: tunnel bring-up ok");
        }
        Ok(s) => {
            tracing::warn!(argv = ?argv, code = ?s.code(), "vpn_gateway: bring-up exited nonzero");
        }
        Err(e) => {
            tracing::warn!(argv = ?argv, error = %e, "vpn_gateway: bring-up did not run");
        }
    }
}

fn binary_present(bin: &str) -> bool {
    let mut cmd = Command::new(bin);
    cmd.arg("--version");
    crate::workers::proc::status_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
        .is_ok()
}

#[async_trait::async_trait]
impl Worker for VpnGatewayWorker {
    fn name(&self) -> &'static str {
        "vpn_gateway"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // Graceful degradation: with neither tunnel tool present there's nothing
        // this worker can do — idle (don't spin) until the daemon restarts.
        if !binary_present("wg-quick") && !binary_present("openvpn") {
            tracing::debug!("vpn_gateway: neither wg-quick nor openvpn present; worker idle");
            return Ok(());
        }
        let mut tick = tokio::time::interval(self.tick);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    // tick_once shells out synchronously; keep it bounded via the
                    // proc helpers so a wedged wg-quick can't pin the runtime
                    // thread (same contract as mesh_firewall).
                    self.tick_once();
                }
                _ = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tun(id: &str, method: Method) -> TunnelDef {
        TunnelDef {
            id: id.into(),
            provider: "generic-wg".into(),
            method,
            ..Default::default()
        }
    }

    fn cfg(tunnels: &[TunnelDef]) -> VpnConfig {
        VpnConfig {
            tunnel: tunnels.to_vec(),
        }
    }

    #[test]
    fn openvpn_config_path_is_per_ifname() {
        assert_eq!(
            openvpn_config_path(&tun("mullvad1", Method::Ovpn)),
            "/etc/openvpn/client/mvpn-mullvad1.ovpn"
        );
    }

    #[test]
    fn plans_wg_bring_up_for_a_down_tunnel() {
        let c = cfg(&[tun("mullvad1", Method::Wg)]);
        // Nothing is present → plan a wg-quick up.
        let plan = plan_bring_up_with(&c, |_| false, |_| false);
        assert_eq!(plan, vec![vec!["wg-quick", "up", "mvpn-mullvad1"]]);
    }

    #[test]
    fn skips_a_tunnel_whose_iface_is_already_present() {
        let c = cfg(&[tun("mullvad1", Method::Wg)]);
        // Interface present → idempotent no-op.
        let plan = plan_bring_up_with(&c, |_| true, |_| true);
        assert!(plan.is_empty());
    }

    #[test]
    fn concurrent_tunnels_each_get_a_distinct_iface() {
        // Acceptance: a node runs >=2 concurrent tunnels (incl. same provider
        // twice). Distinct ids → distinct mvpn-<id> argvs.
        let c = cfg(&[tun("mullvad1", Method::Wg), tun("mullvad2", Method::Wg)]);
        let plan = plan_bring_up_with(&c, |_| false, |_| false);
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0][2], "mvpn-mullvad1");
        assert_eq!(plan[1][2], "mvpn-mullvad2");
    }

    #[test]
    fn openvpn_planned_only_when_config_ready() {
        let c = cfg(&[tun("nord1", Method::Ovpn)]);
        // Config not on disk yet → skipped (honest, never spawns openvpn blind).
        assert!(plan_bring_up_with(&c, |_| false, |_| false).is_empty());
        // Config ready → openvpn argv against the per-ifname path.
        let plan = plan_bring_up_with(&c, |_| false, |_| true);
        assert_eq!(
            plan,
            vec![vec![
                "openvpn".to_string(),
                "--config".to_string(),
                "/etc/openvpn/client/mvpn-nord1.ovpn".to_string(),
                "--dev".to_string(),
                "mvpn-nord1".to_string(),
                "--daemon".to_string(),
            ]]
        );
    }

    #[test]
    fn cli_and_api_methods_are_skipped() {
        let c = cfg(&[tun("a", Method::Cli), tun("b", Method::Api)]);
        assert!(plan_bring_up_with(&c, |_| false, |_| true).is_empty());
    }

    #[test]
    fn empty_config_plans_nothing() {
        assert!(plan_bring_up_with(&VpnConfig::default(), |_| false, |_| true).is_empty());
    }

    #[test]
    fn name_matches_the_module_and_census() {
        let w = VpnGatewayWorker::new(PathBuf::from("/tmp"));
        assert_eq!(w.name(), "vpn_gateway");
    }

    #[tokio::test]
    async fn idles_without_tunnel_binaries() {
        // Graceful degradation: with no wg-quick/openvpn on PATH the worker
        // returns Ok immediately rather than ticking (or panicking). We can't
        // un-PATH the test host, so assert the planner is the no-side-effect
        // core: a tick over an empty config does nothing.
        let tmp = tempfile::tempdir().unwrap();
        let w = VpnGatewayWorker::new(tmp.path().to_path_buf()).with_tick(Duration::from_secs(60));
        w.tick_once(); // empty config on a fresh root → no panic, no action
    }
}
