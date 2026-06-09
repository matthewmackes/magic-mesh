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
                tracing::info!(
                    target: "mackesd::firewall_preset",
                    is_lighthouse,
                    ports = ?ports,
                    "applied nebula firewall preset"
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
        let out = std::process::Command::new(firewall_cmd)
            .args(["--permanent", "--add-port", &spec])
            .output()
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
    let out = std::process::Command::new(firewall_cmd)
        .arg("--reload")
        .output()
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
    fn which_returns_none_for_missing_binary() {
        assert!(which("definitely-not-a-real-binary-xyz").is_none());
    }

    #[test]
    fn which_returns_none_for_empty_string() {
        assert!(which("").is_none());
    }
}
