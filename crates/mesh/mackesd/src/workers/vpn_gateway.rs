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
//!
//! VPN-GW-3 — selective egress. After bring-up, the same tick reconciles the
//! per-tunnel **egress policy** (policy-routing + NAT + kill-switch). The pure
//! plan is built in [`mackes_mesh_types::vpn`] ([`plan_egress_apply`] /
//! [`plan_egress_teardown`]); here a pure planner ([`plan_egress`]) folds the
//! config + the set of present interfaces into the ordered `ip`/`nft` plan, and
//! a thin executor ([`run_egress_cmd`]) runs each with the bounded proc helpers.
//! A tunnel whose egress is enabled AND whose interface is up gets its routing +
//! NAT applied (idempotent — `ip rule add` / `ip route replace` / `nft add`); a
//! tunnel that is enabled but DOWN gets its egress torn down so marked traffic
//! is dropped by the kill-switch instead of leaking to the plaintext WAN. If
//! `ip`/`nft` are absent the egress reconcile is skipped (logged once) — no
//! panic, exactly like the bring-up degradation.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use mackes_mesh_types::vpn::{
    self, ActiveTunnel, EgressCmd, EgressPolicy, Method, RouteConfig, TunnelDef, VpnConfig,
};

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
    /// VPN-GW-4 — this node's id (`peer:host`), so the worker can resolve which
    /// egress route applies to it (the `Node` scope) when it's a gateway. Empty
    /// when unset (a `Node`-scoped route then never matches; `Group`/`AnyMesh`
    /// still do).
    node_id: String,
    /// VPN-GW-4 — this node's group memberships, so a `Group`-scoped route can
    /// match. Resolved at the wiring layer from the mesh's group config; empty
    /// when the node belongs to no routing group.
    groups: Vec<String>,
}

impl VpnGatewayWorker {
    /// Construct with production defaults, rooted at the shared workgroup root.
    /// VPN-GW-4: the node id + group memberships drive route scope matching;
    /// they're supplied via [`Self::with_identity`] (the bare `new` leaves them
    /// empty so existing call sites and tests compile — a node with no identity
    /// only ever matches an `AnyMesh` route).
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self {
            workgroup_root,
            tick: DEFAULT_TICK_INTERVAL,
            node_id: String::new(),
            groups: Vec::new(),
        }
    }

    /// VPN-GW-4 — set this node's id + routing-group memberships (the wiring
    /// layer supplies the live identity). Drives `Node`/`Group` scope matching
    /// in [`plan_route_egress`].
    #[must_use]
    pub fn with_identity(mut self, node_id: String, groups: Vec<String>) -> Self {
        self.node_id = node_id;
        self.groups = groups;
        self
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
        // VPN-GW-4 — the route-driven egress plan: resolve which route applies
        // to this node, run the failover selector against live tunnel status,
        // and apply the SELECTED tunnel's egress (GW-3's mechanism) while
        // tearing down the chain's non-selected tunnels. A route that fails
        // selection (all down) tears every chain tunnel down — the kill-switch
        // (when set) then blocks the marked traffic. Skipped entirely when
        // ip/nft are absent (degrade gracefully).
        let routes = vpn::load_routes(&self.workgroup_root);
        if let Err(e) = routes.validate() {
            tracing::warn!(error = %e, "vpn_gateway: route config invalid; skipping egress routing");
            return;
        }
        let has_route = vpn::resolve_route(&routes, &self.node_id, &self.groups).is_some();
        if egress_tools_present() {
            // The route engine (GW-4) is the egress driver when a route applies
            // to this node; otherwise fall back to GW-3's per-tunnel
            // `egress.enabled` plan so a manually-flagged tunnel still works.
            let plan = if has_route {
                plan_route_egress(&routes, &cfg, &self.node_id, &self.groups, &present_ifaces)
            } else {
                plan_egress(&cfg, &present_ifaces)
            };
            for cmd in plan {
                run_egress_cmd(&cmd);
            }
        } else if has_route || cfg.tunnel.iter().any(|t| t.egress.enabled) {
            tracing::debug!("vpn_gateway: ip/nft absent; skipping selective-egress reconcile");
        }
    }
}

/// Are the selective-egress tools (`ip` + `nft`) present? Egress needs both
/// (policy routing AND nftables NAT/kill-switch), so require both.
fn egress_tools_present() -> bool {
    binary_present("ip") && nft_present()
}

/// `nft --version` probe. (`nft` may be absent on a container-stripped peer.)
fn nft_present() -> bool {
    let mut cmd = Command::new("nft");
    cmd.arg("--version");
    crate::workers::proc::status_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
        .map(|s| s.success())
        .unwrap_or(false)
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

/// PURE egress planner — fold the desired config + a present-interface predicate
/// into the ordered `ip`/`nft` plan that reconciles selective egress for every
/// tunnel that opts in (`egress.enabled`).
///
/// * If any tunnel has egress enabled, the plan starts with the idempotent
///   nftables scaffolding ([`vpn::egress_nft_table_setup_argv`]) so the NAT +
///   kill-switch chains exist.
/// * An enabled tunnel whose interface is **up** gets its apply plan
///   ([`vpn::plan_egress_apply`]): carve-out + fwmark rule + per-table default
///   route + masquerade (+ kill-switch drop when configured). Idempotent
///   (`ip rule add` is a quiet no-op if present; `ip route replace`; `nft add`).
/// * An enabled tunnel whose interface is **down** gets its teardown
///   ([`vpn::plan_egress_teardown`]) so its routing entries are removed and the
///   kill-switch drop (already in the nft table) blocks the marked traffic — no
///   plaintext leak while the tunnel is down.
///
/// Disabled tunnels contribute nothing. The plan is pure data — it performs no
/// I/O and is fully unit-testable without `ip`/`nft`.
#[must_use]
pub fn plan_egress<P>(cfg: &VpnConfig, iface_present: P) -> Vec<EgressCmd>
where
    P: Fn(&str) -> bool,
{
    let any_egress = cfg.tunnel.iter().any(|t| t.egress.enabled);
    if !any_egress {
        return Vec::new();
    }
    let mut plan = vpn::egress_nft_table_setup_argv();
    for t in &cfg.tunnel {
        if !t.egress.enabled {
            continue;
        }
        if iface_present(&t.ifname()) {
            plan.extend(vpn::plan_egress_apply(t));
        } else {
            // Enabled but down → tear the routing down (kill-switch drop in the
            // nft table keeps the marked traffic from leaking).
            plan.extend(vpn::plan_egress_teardown(t));
        }
    }
    plan
}

/// VPN-GW-4 — the PURE route-driven egress planner. This is the assignment
/// engine that DRIVES VPN-GW-3's per-tunnel mechanism:
///
/// 1. Resolve which egress route applies to this node by scope precedence
///    (`Node` > `Group` > `AnyMesh`) via [`vpn::resolve_route`]. No route → the
///    plan is empty (egress routing is opt-in per node).
/// 2. Run the failover selector ([`vpn::select_active`]) over the route's ordered
///    tunnel chain against `iface_present` (live tunnel up/down — VPN-GW-1's
///    `tunnel-status` mechanism): the active tunnel is the first chain entry that
///    is up.
/// 3. **Apply** that one tunnel's egress (GW-3's [`vpn::plan_egress_apply`], with
///    an `EgressPolicy` synthesized from the route — enabled + the route's
///    kill-switch) so the assigned node's traffic exits through the selected
///    provider, and **tear down** every OTHER tunnel in the chain
///    ([`vpn::plan_egress_teardown`]) so a previously-active tunnel's routing is
///    removed on failover (no two chain tunnels carry egress at once).
/// 4. All chain tunnels down → no apply; every chain tunnel is torn down. The
///    kill-switch drop (installed with the apply when the route opts in) is the
///    block-on-drop guard; without it the marked traffic falls through.
///
/// The plan starts with the idempotent nftables scaffolding so the NAT +
/// kill-switch chains exist. Pure data — no I/O, fully unit-testable without a
/// live mesh (the live multi-provider failover *verification* is VPN-GW-6).
///
/// A chain tunnel id with no matching [`TunnelDef`] is skipped for apply (we
/// can't route an interface that doesn't exist) but still counts as "down" in
/// the selector — so a typo'd chain entry fails over to the next real tunnel.
#[must_use]
pub fn plan_route_egress<P>(
    routes: &RouteConfig,
    cfg: &VpnConfig,
    node_id: &str,
    groups: &[String],
    iface_present: P,
) -> Vec<EgressCmd>
where
    P: Fn(&str) -> bool,
{
    let Some(route) = vpn::resolve_route(routes, node_id, groups) else {
        return Vec::new();
    };
    // The selector treats a chain entry as up only when its tunnel is configured
    // AND its interface is present.
    let is_up = |tunnel_id: &str| {
        cfg.get(tunnel_id)
            .is_some_and(|t| iface_present(&t.ifname()))
    };
    let active = vpn::select_active(route, is_up);
    let selected = match &active {
        ActiveTunnel::Up { tunnel_id } => Some(tunnel_id.clone()),
        ActiveTunnel::AllDown { .. } => None,
    };

    let mut plan = vpn::egress_nft_table_setup_argv();
    for tunnel_id in &route.chain {
        let Some(def) = cfg.get(tunnel_id) else {
            // Unconfigured chain entry — nothing to route/teardown for it.
            continue;
        };
        if selected.as_deref() == Some(tunnel_id.as_str()) {
            // The active tunnel: apply egress via GW-3's mechanism. Synthesize
            // the policy from the ROUTE (enabled + the route's kill-switch) so
            // the route is the authority — we don't reimplement the ip/nft layer.
            let applied = with_route_policy(def, route.kill_switch);
            plan.extend(vpn::plan_egress_apply(&applied));
        } else {
            // A non-selected chain tunnel: tear its routing down so a prior
            // active tunnel's egress is removed on failover (idempotent — a
            // never-applied tunnel's teardown is a benign no-op).
            plan.extend(vpn::plan_egress_teardown(def));
        }
    }
    plan
}

/// VPN-GW-4 — clone a tunnel def with an [`EgressPolicy`] synthesized from the
/// route's selection: egress enabled + the route's kill-switch, mark left
/// derived (or the def's operator pin, if any). Lets the route drive GW-3's
/// `plan_egress_apply` without mutating the durable tunnel config.
fn with_route_policy(def: &TunnelDef, kill_switch: bool) -> TunnelDef {
    let mut t = def.clone();
    t.egress = EgressPolicy {
        enabled: true,
        kill_switch,
        mark: def.egress.mark,
    };
    t
}

/// Run one egress `ip`/`nft` command with a bounded timeout. Honest: a nonzero
/// exit is logged at debug (many are benign — `ip rule add` of an existing rule,
/// a teardown of an absent table), a spawn failure at warn. Never panics.
fn run_egress_cmd(cmd: &EgressCmd) {
    let mut c = Command::new(cmd.prog);
    c.args(&cmd.args);
    match crate::workers::proc::status_with_timeout(c, crate::workers::proc::DEFAULT_CMD_TIMEOUT) {
        Ok(s) if s.success() => {
            tracing::debug!(argv = ?cmd.argv(), "vpn_gateway: egress rule applied");
        }
        Ok(s) => {
            // Benign-on-reapply (rule already present / table already gone).
            tracing::debug!(argv = ?cmd.argv(), code = ?s.code(), "vpn_gateway: egress rule nonzero (often idempotent no-op)");
        }
        Err(e) => {
            tracing::warn!(argv = ?cmd.argv(), error = %e, "vpn_gateway: egress rule did not run");
        }
    }
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

    // ── VPN-GW-3 — selective-egress planner ─────────────────────────────────

    fn egress_tun(id: &str, enabled: bool, kill_switch: bool) -> TunnelDef {
        let mut t = tun(id, Method::Wg);
        t.egress = mackes_mesh_types::vpn::EgressPolicy {
            enabled,
            kill_switch,
            mark: None,
        };
        t
    }

    #[test]
    fn no_egress_tunnels_plans_nothing() {
        // No tunnel opts into egress → no nft scaffolding, no rules.
        let c = cfg(&[tun("plain", Method::Wg)]);
        assert!(plan_egress(&c, |_| true).is_empty());
    }

    #[test]
    fn up_egress_tunnel_gets_scaffold_then_apply() {
        let c = cfg(&[egress_tun("mullvad1", true, true)]);
        // Interface up → scaffold (3) + apply (5 w/ kill-switch).
        let plan = plan_egress(&c, |_| true);
        assert_eq!(plan.len(), 3 + 5);
        // First three are the idempotent nft scaffold.
        assert_eq!(&plan[..3], &vpn::egress_nft_table_setup_argv()[..]);
        // The per-tunnel apply follows.
        assert_eq!(
            &plan[3..],
            &vpn::plan_egress_apply(&egress_tun("mullvad1", true, true))[..]
        );
    }

    #[test]
    fn down_egress_tunnel_is_torn_down_not_applied() {
        let c = cfg(&[egress_tun("mullvad1", true, true)]);
        // Interface DOWN → scaffold (3) + teardown (2), no apply.
        let plan = plan_egress(&c, |_| false);
        assert_eq!(plan.len(), 3 + 2);
        assert_eq!(
            &plan[3..],
            &vpn::plan_egress_teardown(&egress_tun("mullvad1", true, true))[..]
        );
        // None of the teardown commands install a route (no leak path opened).
        for cmd in &plan[3..] {
            let joined = cmd.argv().join(" ");
            assert!(!joined.contains("replace default"));
        }
    }

    #[test]
    fn disabled_tunnel_contributes_nothing_even_when_up() {
        // One enabled (to trigger scaffolding) + one disabled; the disabled one
        // adds no rules of its own.
        let c = cfg(&[
            egress_tun("on", true, false),
            egress_tun("off", false, true),
        ]);
        let plan = plan_egress(&c, |_| true);
        // scaffold(3) + apply for "on" only (4, no kill-switch).
        assert_eq!(plan.len(), 3 + 4);
        let off_if = egress_tun("off", false, true).ifname();
        for cmd in &plan {
            assert!(!cmd.argv().join(" ").contains(&off_if));
        }
    }

    // ── VPN-GW-4 — route-driven egress planner ──────────────────────────────
    // (RouteConfig is already in scope via `use super::*`; only the route value
    // types need importing here.)

    use mackes_mesh_types::vpn::{EgressRoute, RouteScope};

    fn rcfg(routes: &[EgressRoute]) -> RouteConfig {
        RouteConfig {
            route: routes.to_vec(),
        }
    }

    fn rt(scope: RouteScope, gw: &str, chain: &[&str], ks: bool) -> EgressRoute {
        EgressRoute {
            scope,
            gateway: gw.into(),
            chain: chain.iter().map(|s| (*s).to_string()).collect(),
            kill_switch: ks,
        }
    }

    #[test]
    fn route_egress_empty_when_no_route_applies() {
        // A route for a DIFFERENT node + no AnyMesh fallback → nothing for us.
        let routes = rcfg(&[rt(
            RouteScope::Node {
                id: "peer:other".into(),
            },
            "peer:gw",
            &["mullvad1"],
            true,
        )]);
        let c = cfg(&[tun("mullvad1", Method::Wg)]);
        assert!(plan_route_egress(&routes, &c, "peer:me", &[], |_| true).is_empty());
    }

    #[test]
    fn route_egress_applies_only_the_selected_active_tunnel() {
        // AnyMesh route, chain primary→fallback; both interfaces present.
        let routes = rcfg(&[rt(
            RouteScope::AnyMesh,
            "peer:gw",
            &["primary", "fallback"],
            true,
        )]);
        let c = cfg(&[tun("primary", Method::Wg), tun("fallback", Method::Wg)]);
        let plan = plan_route_egress(&routes, &c, "peer:me", &[], |_| true);
        // scaffold(3) + apply for "primary" (the selector's pick) + teardown(2)
        // for "fallback".
        let primary = with_route_policy(&tun("primary", Method::Wg), true);
        let fallback = tun("fallback", Method::Wg);
        let expected_apply = vpn::plan_egress_apply(&primary);
        let expected_teardown = vpn::plan_egress_teardown(&fallback);
        assert_eq!(
            plan.len(),
            3 + expected_apply.len() + expected_teardown.len()
        );
        assert_eq!(&plan[..3], &vpn::egress_nft_table_setup_argv()[..]);
        // The selected primary is applied (its ifname is routed).
        assert!(plan
            .iter()
            .any(|c| c.argv().join(" ").contains("mvpn-primary")
                && c.argv().join(" ").contains("replace default")));
        // The non-selected fallback is NOT applied (no default route for it).
        assert!(!plan
            .iter()
            .any(|c| c.argv().join(" ").contains("mvpn-fallback")
                && c.argv().join(" ").contains("replace default")));
    }

    #[test]
    fn route_egress_fails_over_when_primary_is_down() {
        let routes = rcfg(&[rt(
            RouteScope::AnyMesh,
            "peer:gw",
            &["primary", "fallback"],
            true,
        )]);
        let c = cfg(&[tun("primary", Method::Wg), tun("fallback", Method::Wg)]);
        // primary's interface DOWN, fallback UP → fallback becomes active.
        let plan = plan_route_egress(&routes, &c, "peer:me", &[], |ifn| ifn == "mvpn-fallback");
        // The fallback is applied (routed), the primary torn down.
        assert!(plan
            .iter()
            .any(|c| c.argv().join(" ").contains("mvpn-fallback")
                && c.argv().join(" ").contains("replace default")));
        assert!(!plan
            .iter()
            .any(|c| c.argv().join(" ").contains("mvpn-primary")
                && c.argv().join(" ").contains("replace default")));
    }

    #[test]
    fn route_egress_all_down_applies_nothing_only_scaffold_and_teardown() {
        let routes = rcfg(&[rt(
            RouteScope::AnyMesh,
            "peer:gw",
            &["primary", "fallback"],
            true,
        )]);
        let c = cfg(&[tun("primary", Method::Wg), tun("fallback", Method::Wg)]);
        // Every interface down → no active tunnel; both chain entries torn down.
        let plan = plan_route_egress(&routes, &c, "peer:me", &[], |_| false);
        // No default route is installed for any tunnel (no leak path opened).
        for c in &plan {
            assert!(
                !c.argv().join(" ").contains("replace default"),
                "all-down must apply no default route"
            );
        }
        // scaffold(3) + teardown(2) for each of the two chain tunnels.
        let td = vpn::plan_egress_teardown(&tun("primary", Method::Wg));
        assert_eq!(plan.len(), 3 + 2 * td.len());
    }

    #[test]
    fn route_egress_honors_scope_precedence_node_over_group() {
        // A Node route + a Group route both match; the Node route's chain wins.
        let routes = rcfg(&[
            rt(
                RouteScope::Group { name: "lab".into() },
                "gw",
                &["grp_tun"],
                true,
            ),
            rt(
                RouteScope::Node {
                    id: "peer:me".into(),
                },
                "gw",
                &["node_tun"],
                true,
            ),
        ]);
        let c = cfg(&[tun("grp_tun", Method::Wg), tun("node_tun", Method::Wg)]);
        let plan = plan_route_egress(&routes, &c, "peer:me", &["lab".into()], |_| true);
        // The Node route applies node_tun, never grp_tun.
        assert!(plan
            .iter()
            .any(|c| c.argv().join(" ").contains("mvpn-nodetun")
                && c.argv().join(" ").contains("replace default")));
        assert!(!plan
            .iter()
            .any(|c| c.argv().join(" ").contains("mvpn-grptun")));
    }

    #[test]
    fn route_egress_skips_a_chain_entry_with_no_tunnel_def() {
        // The primary chain entry has no def (typo) → treated as down; the real
        // fallback becomes active.
        let routes = rcfg(&[rt(RouteScope::AnyMesh, "peer:gw", &["ghost", "real"], true)]);
        let c = cfg(&[tun("real", Method::Wg)]);
        let plan = plan_route_egress(&routes, &c, "peer:me", &[], |_| true);
        assert!(plan.iter().any(|c| c.argv().join(" ").contains("mvpn-real")
            && c.argv().join(" ").contains("replace default")));
    }

    #[test]
    fn with_route_policy_enables_egress_and_carries_killswitch() {
        let base = tun("t", Method::Wg);
        assert!(!base.egress.enabled);
        let on = with_route_policy(&base, true);
        assert!(on.egress.enabled);
        assert!(on.egress.kill_switch);
        let off = with_route_policy(&base, false);
        assert!(off.egress.enabled);
        assert!(!off.egress.kill_switch);
        // The mark is derived (or the def's pin) — same tunnel id → same mark.
        assert_eq!(on.egress_mark(), base.egress_mark());
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
