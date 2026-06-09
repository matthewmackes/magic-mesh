//! BUS-1.1 — `mde-bus` subprocess supervisor.
//!
//! Spawns the `mde-bus daemon` binary and restarts it on exit. The
//! supervisor itself is a regular mackesd worker; the per-task
//! restart policy (set in `crates/mackesd/src/bin/mackesd.rs` when
//! the worker is spawned) governs back-off + circuit-breaker
//! semantics. This worker is the inner restart loop — it relaunches
//! `mde-bus` immediately when the child returns 0, and propagates a
//! non-zero exit as a `worker error` so the outer supervisor can
//! back off.
//!
//! v6.x epic locks (see `docs/design/v6.x-mackes-bus.md`):
//! - Every peer runs its own broker (Round 2 — gossip-synced). The
//!   binary is the broker container.
//! - mDNS-on-Nebula discovery + ntfy broker supervision land in
//!   BUS-1.2 + BUS-1.3 inside the binary; this worker only ensures
//!   the binary is alive.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::{Child, Command};
use tracing::{debug, error, info, warn};

use super::{ShutdownToken, Worker};

/// Default search path for the binary. Production installs ship
/// `mde-bus` into `/usr/bin/`; the dev build path is under
/// `target/debug/`. The supervisor probes both, in that order, when
/// no explicit path is configured.
const DEFAULT_PROD_PATH: &str = "/usr/bin/mde-bus";

/// Cool-down between unconditional restarts. The outer supervisor's
/// `RestartPolicy::Always` already wraps this worker, so the inner
/// cooldown only paces immediate respawns when the child exits 0
/// (i.e. a clean shutdown that we still want to reverse, e.g.
/// systemctl restart). 1s mirrors `nebula_supervisor`'s reload pace.
const RESPAWN_COOLDOWN: Duration = Duration::from_secs(1);

/// Async worker that supervises the `mde-bus` subprocess.
pub struct BusSupervisor {
    /// Optional explicit binary path. When `None`, the supervisor
    /// probes the production then dev locations.
    binary_path_override: Option<PathBuf>,
    /// Last `Child` handle so shutdown can kill the running process
    /// cleanly. Set to `Some` whenever a child is alive.
    child: Option<Child>,
}

impl BusSupervisor {
    /// Construct a supervisor with no binary-path override. Will
    /// probe `/usr/bin/mde-bus` then the `MDE_BUS_BIN` env var on
    /// every spawn cycle so a fresh RPM install is picked up without
    /// a mackesd restart.
    #[must_use]
    pub fn new() -> Self {
        Self {
            binary_path_override: None,
            child: None,
        }
    }

    /// Pin a specific binary path. Used by tests + by deployment
    /// recipes that ship `mde-bus` in a non-standard location.
    #[must_use]
    pub fn with_binary(mut self, path: PathBuf) -> Self {
        self.binary_path_override = Some(path);
        self
    }

    fn resolve_binary(&self) -> Option<PathBuf> {
        if let Some(p) = &self.binary_path_override {
            return Some(p.clone());
        }
        let prod = PathBuf::from(DEFAULT_PROD_PATH);
        if prod.is_file() {
            return Some(prod);
        }
        // Dev fallback — let the operator point at the cargo target
        // dir via env. Mirrors the pattern several other workers use
        // (e.g. `MDE_HTTPS_TUNNEL_CERT` in nebula_https_listener).
        if let Ok(p) = std::env::var("MDE_BUS_BIN") {
            let p = PathBuf::from(p);
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }

    async fn spawn_child(&mut self) -> anyhow::Result<()> {
        let bin = match self.resolve_binary() {
            Some(p) => p,
            None => {
                // Graceful degrade — silently no-op until the binary
                // appears. Matches `gluster_worker`'s "skip when
                // glusterd isn't installed" pattern. The outer tick
                // will retry.
                debug!("mde-bus binary not found; supervisor idle");
                return Ok(());
            }
        };
        info!(binary = %bin.display(), "spawning mde-bus daemon");
        let child = Command::new(&bin)
            .arg("daemon")
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        self.child = Some(child);
        Ok(())
    }
}

impl Default for BusSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Worker for BusSupervisor {
    fn name(&self) -> &'static str {
        "bus_supervisor"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // Outer loop: keep the child alive. Inner select waits on
        // either the child exiting OR a shutdown signal.
        loop {
            if self.child.is_none() {
                if let Err(e) = self.spawn_child().await {
                    warn!(error = %e, "mde-bus spawn failed; will retry after cooldown");
                }
            }
            match self.child.as_mut() {
                Some(child) => {
                    tokio::select! {
                        wait = child.wait() => {
                            match wait {
                                Ok(status) if status.success() => {
                                    info!("mde-bus exited cleanly; respawning");
                                }
                                Ok(status) => {
                                    error!(?status, "mde-bus exited non-zero; will respawn");
                                }
                                Err(e) => {
                                    error!(error = %e, "mde-bus wait failed");
                                }
                            }
                            self.child = None;
                            tokio::time::sleep(RESPAWN_COOLDOWN).await;
                        }
                        () = shutdown.wait() => {
                            info!("shutdown requested; killing mde-bus child");
                            if let Some(mut c) = self.child.take() {
                                let _ = c.kill().await;
                                let _ = c.wait().await;
                            }
                            return Ok(());
                        }
                    }
                }
                None => {
                    // No child running (binary missing). Poll on a
                    // slow tick so a freshly-installed RPM is picked
                    // up without a daemon restart.
                    tokio::select! {
                        () = tokio::time::sleep(Duration::from_secs(30)) => {}
                        () = shutdown.wait() => {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_stable() {
        let w = BusSupervisor::new();
        assert_eq!(w.name(), "bus_supervisor");
    }

    #[test]
    fn resolve_binary_honors_override() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let w = BusSupervisor::new().with_binary(tmp.path().to_path_buf());
        let resolved = w.resolve_binary().expect("override should resolve");
        assert_eq!(resolved, tmp.path().to_path_buf());
    }

    #[test]
    fn resolve_binary_returns_none_when_unavailable() {
        // Clear the env override + use a deliberately broken path.
        // This is sensitive to /usr/bin/mde-bus presence on the
        // dev box, so run only when the file is absent.
        if !std::path::Path::new(DEFAULT_PROD_PATH).is_file()
            && std::env::var("MDE_BUS_BIN").is_err()
        {
            let w = BusSupervisor::new();
            assert!(w.resolve_binary().is_none());
        }
    }
}
