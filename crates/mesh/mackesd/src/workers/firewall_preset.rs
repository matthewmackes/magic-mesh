//! NF-21.3 — `firewall_preset` worker.
//!
//! Owns the firewalld preset that opens the ports Nebula needs to
//! reach this peer. Replaces the Python helper
//! `mesh_nebula.py::apply_nebula_firewall_preset` so the legacy
//! module can fully retire (DEAD-2.14 plan).
//!
//! Lifecycle:
//!
//! 1. **First tick:** apply the desired-ports list once. Non-
//!    lighthouses open UDP/4242 inbound (so other peers can
//!    UDP-hole-punch in); lighthouses additionally open TCP/443
//!    inbound (so peers can fall back to the NF-1 covert TCP/443
//!    listener when UDP is blocked).
//! 2. **Subsequent ticks:** poll the role-marker mtime
//!    ([`crate::ipc::nebula::DEFAULT_ROLE_HOST_MARKER`]). When the
//!    mtime advances OR the marker file appears/disappears (role
//!    flip during a lighthouse re-election), re-apply.
//!
//! Tailscale's UDP/41641 preset (the v1.x default) is NOT cleaned
//! up here — leave existing rules alone so a peer migrating from
//! Tailscale doesn't lose connectivity mid-flight. The mackesd
//! cleanup pass retires the Tailscale preset in NF-6.x once the
//! operator confirms the migration succeeded.
//!
//! Shells out to `firewall-cmd` (the canonical Fedora abstraction
//! over nftables). On peers without `firewall-cmd` on PATH, the
//! worker logs a single warning and stays idle — every subsequent
//! tick observes "no firewall-cmd" and short-circuits. Idempotent
//! by virtue of `firewall-cmd --add-port`: re-adding an existing
//! port is a quiet no-op.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::{ShutdownToken, Worker};

/// Default sweep cadence. Role-marker changes are rare (lighthouse
/// re-election only), so a 30 s tick keeps the worker quiet.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(30);

/// Default path of the role.host marker
/// (`is_lighthouse = marker.exists()`).
pub const DEFAULT_ROLE_MARKER_PATH: &str = crate::ipc::nebula::DEFAULT_ROLE_HOST_MARKER;

/// Nebula firewall preset: (port, protocol) tuples to open
/// inbound. Mirrors `mesh_nebula.py::NEBULA_FIREWALL_PORTS` for
/// lighthouses; non-lighthouses get only the UDP entry.
///
/// - UDP/4242 — native Nebula outer-tunnel port. Required inbound
///   on every peer so other peers can hole-punch in.
/// - TCP/443  — NF-1 covert TCP/443 fallback listener. Lighthouses
///   only — they're the rendezvous for the covert path.
const NEBULA_PORTS_ALL_PEERS: &[(u16, &str)] = &[(4242, "udp")];
const NEBULA_PORTS_LIGHTHOUSE_EXTRA: &[(u16, &str)] = &[(443, "tcp")];

/// KDC-INTEROP — KDE Connect's LAN port range (TCP+UDP 1714–1764). Opened only
/// on **Workstation-rank** peers, where the `kdc_host` worker runs and binds
/// 1716; a paired phone discovers + connects over the LAN through these. Headless
/// Lighthouse/Server peers don't run `kdc_host`, so they never open them (keeps
/// their underlay attack surface to the Nebula bootstrap ports). Expressed as
/// `firewall-cmd` port-range specs since the range is 51 ports.
const KDE_CONNECT_PORT_SPECS: &[&str] = &["1714-1764/tcp", "1714-1764/udp"];

/// Whether this peer runs the Workstation worker set (rank ≥ 2) — i.e. it runs
/// `kdc_host` and therefore should open the KDE Connect ports. Tolerant resolver
/// (an unpinned dev box reads as Workstation), matching the worker pool's default.
fn runs_kdc_host() -> bool {
    crate::worker_role::resolve_rank() >= mde_role::Role::Workstation.rank()
}

/// Worker handle. Tracks the last-observed role-marker state +
/// last-applied port set so the worker doesn't re-shell-out on
/// every tick.
pub struct FirewallPresetWorker {
    role_marker_path: PathBuf,
    tick_interval: Duration,
    firewall_cmd: &'static str,
    last_marker_mtime: Option<SystemTime>,
    last_marker_existed: Option<bool>,
    last_applied_lighthouse: Option<bool>,
}

impl Default for FirewallPresetWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl FirewallPresetWorker {
    /// Construct a worker pinned to the default role-marker path +
    /// `firewall-cmd` shell-out.
    #[must_use]
    pub fn new() -> Self {
        Self {
            role_marker_path: PathBuf::from(DEFAULT_ROLE_MARKER_PATH),
            tick_interval: DEFAULT_TICK_INTERVAL,
            firewall_cmd: "firewall-cmd",
            last_marker_mtime: None,
            last_marker_existed: None,
            last_applied_lighthouse: None,
        }
    }

    /// Override the role-marker path — used by tests.
    #[must_use]
    pub fn with_role_marker_path(mut self, path: PathBuf) -> Self {
        self.role_marker_path = path;
        self
    }

    /// Override the tick interval — used by tests that need a
    /// faster pulse.
    #[must_use]
    pub fn with_tick_interval(mut self, interval: Duration) -> Self {
        self.tick_interval = interval;
        self
    }

    /// Override the `firewall-cmd` shell-out — empty string disables
    /// shell-out so tests don't touch the live firewalld.
    #[must_use]
    pub fn with_firewall_cmd(mut self, cmd: &'static str) -> Self {
        self.firewall_cmd = cmd;
        self
    }

    /// One tick of the worker's loop. Public so tests can drive
    /// it deterministically without the tokio-time pulse.
    pub fn tick_once(&mut self) -> TickOutcome {
        let is_lighthouse = self.role_marker_path.exists();
        let role_changed = match self.last_marker_existed {
            None => true, // first tick — always apply
            Some(prev) => prev != is_lighthouse,
        };
        // Also reapply if the marker file mtime advanced (e.g.,
        // an enrolment refresh touched the file even though the
        // role didn't flip).
        let mtime_advanced = if is_lighthouse {
            match std::fs::metadata(&self.role_marker_path).and_then(|m| m.modified()) {
                Ok(now) => {
                    let advanced = self.last_marker_mtime.is_none_or(|last| now > last);
                    self.last_marker_mtime = Some(now);
                    advanced
                }
                Err(_) => false,
            }
        } else {
            self.last_marker_mtime = None;
            false
        };
        self.last_marker_existed = Some(is_lighthouse);
        if !role_changed && !mtime_advanced {
            return TickOutcome::Idle;
        }
        // Apply.
        let ports = desired_ports(is_lighthouse);
        if self.firewall_cmd.is_empty() {
            // Test mode — skip shell-out but record what we would
            // have applied.
            self.last_applied_lighthouse = Some(is_lighthouse);
            return TickOutcome::AppliedSkippedShell;
        }
        match apply_preset(self.firewall_cmd, &ports) {
            Ok(()) => {
                self.last_applied_lighthouse = Some(is_lighthouse);
                // KDC-INTEROP — Workstation peers also open the KDE Connect range
                // so a phone can pair/connect over the LAN. Best-effort: a failure
                // here must not undo the (succeeded) Nebula preset.
                if runs_kdc_host() {
                    if let Err(e) = apply_port_specs(self.firewall_cmd, KDE_CONNECT_PORT_SPECS) {
                        tracing::warn!(
                            target: "mackesd::firewall_preset",
                            error = %e,
                            "nebula preset applied, but KDE Connect ports deferred (KDC-INTEROP)"
                        );
                    }
                }
                // PLANES-16 — bind the overlay to the trusted zone and the
                // underlay to the tight public zone (W69/W70). Best-effort:
                // a zone failure must not undo the (succeeded) port preset,
                // so it's logged, not propagated.
                let plan = zone_plan(
                    is_lighthouse,
                    OVERLAY_IFACE,
                    default_underlay_iface().as_deref(),
                );
                if let Err(e) = apply_zones(self.firewall_cmd, &plan) {
                    tracing::warn!(
                        target: "mackesd::firewall_preset",
                        error = %e,
                        "nebula port preset applied, but zone plan deferred (PLANES-16)"
                    );
                }
                tracing::info!(
                    target: "mackesd::firewall_preset",
                    is_lighthouse,
                    ports = ?ports,
                    "applied nebula firewall preset + zones"
                );
                TickOutcome::Applied
            }
            Err(e) => {
                tracing::warn!(
                    target: "mackesd::firewall_preset",
                    error = %e,
                    "failed to apply nebula firewall preset"
                );
                TickOutcome::Failed
            }
        }
    }
}

/// Per-tick result. Exposed for tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickOutcome {
    /// No role change + no mtime advance; nothing to do.
    Idle,
    /// Preset applied via `firewall-cmd`.
    Applied,
    /// Test-mode short-circuit (empty `firewall_cmd`); records
    /// intent without shelling out.
    AppliedSkippedShell,
    /// Shell-out failed; warning logged.
    Failed,
}

#[async_trait::async_trait]
impl Worker for FirewallPresetWorker {
    fn name(&self) -> &'static str {
        "firewall_preset"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            let _ = self.tick_once();
            tokio::select! {
                _ = shutdown.wait() => break,
                _ = tokio::time::sleep(self.tick_interval) => {},
            }
        }
        Ok(())
    }
}

/// Pure helper — desired (port, proto) list for a peer's role.
/// All peers: UDP/4242. Lighthouses additionally: TCP/443.
#[must_use]
pub fn desired_ports(is_lighthouse: bool) -> Vec<(u16, &'static str)> {
    let mut out: Vec<(u16, &'static str)> = NEBULA_PORTS_ALL_PEERS.iter().copied().collect();
    if is_lighthouse {
        out.extend(NEBULA_PORTS_LIGHTHOUSE_EXTRA.iter().copied());
    }
    out
}

// ───────────────────────── PLANES-16: firewalld zones ─────────────────
//
// W69/W70/W71. The overlay is a trust boundary, not just a set of open
// ports: every peer on the Nebula overlay is inside the ≤8-peer trust
// envelope (§8), so the overlay interface lands in firewalld's **trusted**
// zone (W69) — all overlay traffic is accepted, and §3 crypto + the
// Nebula cert is what gates who's *on* the overlay. The underlay
// (physical NIC) gets the **tight** `public` zone with only the per-role
// ports Nebula needs to bootstrap a tunnel (W70). Revocation is NOT a
// firewall concern (W71): a revoked peer is evicted by the Nebula
// blocklist (`mesh_firewall` / the CA blocklist), never by a zone rule.

/// The Nebula overlay interface — always bound to the `trusted` zone.
pub const OVERLAY_IFACE: &str = "nebula1";
/// firewalld's built-in all-accept zone for the overlay (W69).
pub const OVERLAY_ZONE: &str = "trusted";
/// firewalld's built-in tight zone for the underlay NIC (W70).
pub const UNDERLAY_ZONE: &str = "public";

/// A firewalld zone plan (PLANES-16): interface→zone bindings plus the
/// inbound ports each zone permits. The overlay zone needs no per-port
/// rule (it accepts everything); the underlay zone carries the role-tight
/// Nebula bootstrap ports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZonePlan {
    /// `(interface, zone)` bindings to enforce with `--change-interface`.
    pub bindings: Vec<(String, String)>,
    /// `(zone, port, proto)` inbound allowances.
    pub ports: Vec<(String, u16, &'static str)>,
}

/// Build the role's zone plan. `underlay_iface` is `None` when the
/// physical NIC couldn't be determined (we still bind the overlay to
/// trusted — that's the load-bearing W69 invariant and needs no underlay).
#[must_use]
pub fn zone_plan(
    is_lighthouse: bool,
    overlay_iface: &str,
    underlay_iface: Option<&str>,
) -> ZonePlan {
    let mut bindings = vec![(overlay_iface.to_string(), OVERLAY_ZONE.to_string())];
    let mut ports = Vec::new();
    if let Some(under) = underlay_iface {
        bindings.push((under.to_string(), UNDERLAY_ZONE.to_string()));
        // The same role-tight Nebula bootstrap ports the port preset opens,
        // but scoped to the tight underlay zone (W70).
        for (port, proto) in desired_ports(is_lighthouse) {
            ports.push((UNDERLAY_ZONE.to_string(), port, proto));
        }
    }
    ZonePlan { bindings, ports }
}

/// Render a [`ZonePlan`] into idempotent `firewall-cmd` argument batches
/// (each inner vec is one `firewall-cmd` invocation, sans the binary). A
/// trailing `--reload` is the caller's job.
#[must_use]
pub fn zone_cmd_batches(plan: &ZonePlan) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for (iface, zone) in &plan.bindings {
        out.push(vec![
            "--permanent".to_string(),
            "--zone".to_string(),
            zone.clone(),
            "--change-interface".to_string(),
            iface.clone(),
        ]);
    }
    for (zone, port, proto) in &plan.ports {
        out.push(vec![
            "--permanent".to_string(),
            "--zone".to_string(),
            zone.clone(),
            "--add-port".to_string(),
            format!("{port}/{proto}"),
        ]);
    }
    out
}

/// Best-effort discovery of the default-route (underlay) interface via
/// `ip route show default` → the `dev <iface>` token. `None` when `ip`
/// is absent or there's no default route (we then bind only the overlay).
#[must_use]
pub fn default_underlay_iface() -> Option<String> {
    // EFF-20 — bound `ip` so a hung invocation can't pin the tick.
    let mut cmd = std::process::Command::new("ip");
    cmd.args(["route", "show", "default"]);
    let out =
        crate::workers::proc::output_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
            .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_default_iface(&String::from_utf8_lossy(&out.stdout))
}

/// Pure: pull the `dev <iface>` from `ip route show default` output, never
/// returning the overlay interface (we never tighten the overlay NIC).
#[must_use]
pub fn parse_default_iface(route_output: &str) -> Option<String> {
    route_output
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("default"))
        .find_map(|line| {
            let mut toks = line.split_whitespace();
            while let Some(t) = toks.next() {
                if t == "dev" {
                    if let Some(dev) = toks.next() {
                        if dev != OVERLAY_IFACE {
                            return Some(dev.to_string());
                        }
                    }
                }
            }
            None
        })
}

/// Apply a zone plan via `firewall-cmd`, tolerating firewalld's
/// "already in this state" non-zero exits (`ZONE_ALREADY_SET`,
/// `ALREADY_ENABLED`). Reloads once at the end if any batch ran.
fn apply_zones(firewall_cmd: &str, plan: &ZonePlan) -> Result<(), String> {
    if which(firewall_cmd).is_none() {
        return Err(format!("{firewall_cmd} not on PATH; zone plan deferred"));
    }
    for batch in zone_cmd_batches(plan) {
        let mut cmd = std::process::Command::new(firewall_cmd);
        cmd.args(&batch);
        let out = crate::workers::proc::output_with_timeout(
            cmd,
            crate::workers::proc::DEFAULT_CMD_TIMEOUT,
        )
        .map_err(|e| format!("spawn {firewall_cmd} {batch:?}: {e}"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stderr.contains("ALREADY_ENABLED") && !stderr.contains("ZONE_ALREADY_SET") {
                return Err(format!(
                    "{firewall_cmd} {batch:?} failed: {}",
                    stderr.trim()
                ));
            }
        }
    }
    let mut reload = std::process::Command::new(firewall_cmd);
    reload.arg("--reload");
    let out = crate::workers::proc::output_with_timeout(
        reload,
        crate::workers::proc::DEFAULT_CMD_TIMEOUT,
    )
    .map_err(|e| format!("spawn {firewall_cmd} --reload: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{firewall_cmd} --reload failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// Shell out to `firewall-cmd --permanent --add-port <port>/<proto>`
/// for each entry + `firewall-cmd --reload` to activate. Idempotent:
/// re-adding an existing port is a no-op on the firewalld side.
fn apply_preset(firewall_cmd: &str, ports: &[(u16, &'static str)]) -> Result<(), String> {
    if which(firewall_cmd).is_none() {
        return Err(format!(
            "{firewall_cmd} not on PATH; preset deferred until firewalld is installed"
        ));
    }
    for (port, proto) in ports {
        let spec = format!("{port}/{proto}");
        let mut cmd = std::process::Command::new(firewall_cmd);
        cmd.args(["--permanent", "--add-port", &spec]);
        let out = crate::workers::proc::output_with_timeout(
            cmd,
            crate::workers::proc::DEFAULT_CMD_TIMEOUT,
        )
        .map_err(|e| format!("spawn {firewall_cmd} --add-port {spec}: {e}"))?;
        if !out.status.success() {
            // firewall-cmd returns non-zero when the port is
            // already there — treat that as success by checking
            // stderr. The canonical "already enabled" message is
            // "ALREADY_ENABLED".
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stderr.contains("ALREADY_ENABLED") {
                return Err(format!(
                    "{firewall_cmd} --add-port {spec} failed: {}",
                    stderr.trim()
                ));
            }
        }
    }
    let mut reload = std::process::Command::new(firewall_cmd);
    reload.arg("--reload");
    let out = crate::workers::proc::output_with_timeout(
        reload,
        crate::workers::proc::DEFAULT_CMD_TIMEOUT,
    )
    .map_err(|e| format!("spawn {firewall_cmd} --reload: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{firewall_cmd} --reload failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// Shell out to `firewall-cmd --permanent --add-port <spec>` for each
/// port-range spec (e.g. `1714-1764/tcp`) + a single `--reload`. Like
/// [`apply_preset`] but for range specs (KDE Connect). Idempotent:
/// `ALREADY_ENABLED` is treated as success.
fn apply_port_specs(firewall_cmd: &str, specs: &[&str]) -> Result<(), String> {
    if which(firewall_cmd).is_none() {
        return Err(format!(
            "{firewall_cmd} not on PATH; KDE Connect ports deferred until firewalld is installed"
        ));
    }
    for spec in specs {
        let mut cmd = std::process::Command::new(firewall_cmd);
        cmd.args(["--permanent", "--add-port", spec]);
        let out = crate::workers::proc::output_with_timeout(
            cmd,
            crate::workers::proc::DEFAULT_CMD_TIMEOUT,
        )
        .map_err(|e| format!("spawn {firewall_cmd} --add-port {spec}: {e}"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stderr.contains("ALREADY_ENABLED") {
                return Err(format!(
                    "{firewall_cmd} --add-port {spec} failed: {}",
                    stderr.trim()
                ));
            }
        }
    }
    let mut reload = std::process::Command::new(firewall_cmd);
    reload.arg("--reload");
    let out = crate::workers::proc::output_with_timeout(
        reload,
        crate::workers::proc::DEFAULT_CMD_TIMEOUT,
    )
    .map_err(|e| format!("spawn {firewall_cmd} --reload: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{firewall_cmd} --reload failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// Minimal `which`-style lookup over `$PATH`. Avoids pulling the
/// `which` crate just for this single use.
fn which(cmd: &str) -> Option<PathBuf> {
    if cmd.is_empty() {
        return None;
    }
    if Path::new(cmd).is_absolute() {
        return Path::new(cmd).is_file().then(|| PathBuf::from(cmd));
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(cmd);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_name_is_firewall_preset() {
        let w = FirewallPresetWorker::new();
        assert_eq!(w.name(), "firewall_preset");
    }

    #[test]
    fn desired_ports_non_lighthouse_is_udp_only() {
        let ports = desired_ports(false);
        assert_eq!(ports, vec![(4242_u16, "udp")]);
    }

    #[test]
    fn desired_ports_lighthouse_adds_tcp_443() {
        let ports = desired_ports(true);
        assert_eq!(ports, vec![(4242_u16, "udp"), (443_u16, "tcp")]);
    }

    #[test]
    fn zone_plan_always_binds_overlay_to_trusted() {
        // Even with no underlay discoverable, the W69 invariant holds:
        // nebula1 → trusted, and nothing tightens the overlay.
        let plan = zone_plan(false, OVERLAY_IFACE, None);
        assert_eq!(plan.bindings, vec![("nebula1".into(), "trusted".into())]);
        assert!(plan.ports.is_empty(), "no underlay → no underlay ports");
    }

    #[test]
    fn zone_plan_tightens_underlay_per_role() {
        // Non-lighthouse: overlay→trusted, eth0→public with UDP/4242 only.
        let node = zone_plan(false, OVERLAY_IFACE, Some("eth0"));
        assert_eq!(node.bindings[0], ("nebula1".into(), "trusted".into()));
        assert_eq!(node.bindings[1], ("eth0".into(), "public".into()));
        assert_eq!(node.ports, vec![("public".into(), 4242, "udp")]);
        // Lighthouse adds TCP/443 to the tight underlay zone (W70).
        let lh = zone_plan(true, OVERLAY_IFACE, Some("eth0"));
        assert_eq!(
            lh.ports,
            vec![
                ("public".into(), 4242, "udp"),
                ("public".into(), 443, "tcp")
            ]
        );
    }

    #[test]
    fn zone_cmd_batches_render_change_interface_and_ports() {
        let plan = zone_plan(true, OVERLAY_IFACE, Some("eth0"));
        let batches = zone_cmd_batches(&plan);
        // overlay bind, underlay bind, then the two underlay ports.
        assert!(batches.contains(&vec![
            "--permanent".into(),
            "--zone".into(),
            "trusted".into(),
            "--change-interface".into(),
            "nebula1".into(),
        ]));
        assert!(batches.contains(&vec![
            "--permanent".into(),
            "--zone".into(),
            "public".into(),
            "--add-port".into(),
            "443/tcp".into(),
        ]));
    }

    #[test]
    fn parse_default_iface_reads_dev_and_skips_overlay() {
        // The real `ip route show default` shape.
        let out = "default via 192.168.1.1 dev eth0 proto dhcp metric 100\n";
        assert_eq!(parse_default_iface(out), Some("eth0".to_string()));
        // A default route over the overlay itself is never chosen as the
        // underlay to tighten (we'd otherwise lock the mesh out).
        let over = "default via 10.42.0.1 dev nebula1 metric 50\n";
        assert_eq!(parse_default_iface(over), None);
        // No default route → nothing.
        assert_eq!(parse_default_iface("10.0.0.0/24 dev eth0\n"), None);
    }

    #[test]
    fn first_tick_applies_when_marker_missing() {
        // No role marker → host (non-lighthouse) role; still apply
        // on first tick.
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut w = FirewallPresetWorker::new()
            .with_role_marker_path(tmp.path().join("role.host"))
            .with_firewall_cmd(""); // skip shell-out
        assert_eq!(w.tick_once(), TickOutcome::AppliedSkippedShell);
        assert_eq!(w.last_applied_lighthouse, Some(false));
    }

    #[test]
    fn first_tick_applies_when_marker_present() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let marker = tmp.path().join("role.host");
        std::fs::write(&marker, "lighthouse").expect("seed marker");
        let mut w = FirewallPresetWorker::new()
            .with_role_marker_path(marker)
            .with_firewall_cmd("");
        assert_eq!(w.tick_once(), TickOutcome::AppliedSkippedShell);
        assert_eq!(w.last_applied_lighthouse, Some(true));
    }

    #[test]
    fn second_tick_idle_when_unchanged() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut w = FirewallPresetWorker::new()
            .with_role_marker_path(tmp.path().join("role.host"))
            .with_firewall_cmd("");
        assert_eq!(w.tick_once(), TickOutcome::AppliedSkippedShell);
        assert_eq!(w.tick_once(), TickOutcome::Idle);
    }

    #[test]
    fn role_flip_host_to_lighthouse_reapplies() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let marker = tmp.path().join("role.host");
        let mut w = FirewallPresetWorker::new()
            .with_role_marker_path(marker.clone())
            .with_firewall_cmd("");
        // First tick: no marker → host.
        assert_eq!(w.tick_once(), TickOutcome::AppliedSkippedShell);
        assert_eq!(w.last_applied_lighthouse, Some(false));
        // Promote to lighthouse.
        std::fs::write(&marker, "lighthouse").expect("seed marker");
        assert_eq!(w.tick_once(), TickOutcome::AppliedSkippedShell);
        assert_eq!(w.last_applied_lighthouse, Some(true));
    }

    #[test]
    fn role_flip_lighthouse_to_host_reapplies() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let marker = tmp.path().join("role.host");
        std::fs::write(&marker, "lighthouse").expect("seed marker");
        let mut w = FirewallPresetWorker::new()
            .with_role_marker_path(marker.clone())
            .with_firewall_cmd("");
        assert_eq!(w.tick_once(), TickOutcome::AppliedSkippedShell);
        assert_eq!(w.last_applied_lighthouse, Some(true));
        // Demote to host.
        std::fs::remove_file(&marker).expect("remove marker");
        assert_eq!(w.tick_once(), TickOutcome::AppliedSkippedShell);
        assert_eq!(w.last_applied_lighthouse, Some(false));
    }

    #[test]
    fn kde_connect_specs_cover_the_lan_range_tcp_and_udp() {
        // KDC-INTEROP — the Workstation-only KDE Connect ports are the full
        // 1714–1764 range over both TCP and UDP (discovery is UDP, links TCP).
        assert_eq!(KDE_CONNECT_PORT_SPECS, &["1714-1764/tcp", "1714-1764/udp"]);
    }

    #[test]
    fn apply_port_specs_skips_shell_for_empty_cmd() {
        // Empty firewall_cmd → which() returns None → deferred (no panic, no
        // shell-out), the same test-safe contract apply_preset honors.
        let r = apply_port_specs("", KDE_CONNECT_PORT_SPECS);
        assert!(r.is_err(), "empty cmd defers rather than shelling out");
    }

    #[test]
    fn which_returns_none_for_missing_binary() {
        assert!(which("definitely-not-a-real-binary-xyz").is_none());
    }

    #[test]
    fn which_returns_none_for_empty_string() {
        assert!(which("").is_none());
    }
}
