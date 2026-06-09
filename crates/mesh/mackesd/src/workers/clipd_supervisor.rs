//! BUS-5.1 — `mde-clipd` subprocess supervisor.
//!
//! Spawns the `mde-clipd` binary and restarts it on exit. The daemon
//! connects to the Wayland clipboard via `wlr-data-control-unstable-v1`
//! and logs every selection event. One instance runs per Wayland session;
//! the supervisor probes `$WAYLAND_DISPLAY` before spawning and idles on
//! a slow tick when no display is available.
//!
//! Mirrors the `bus_supervisor` pattern: inner restart loop, outer
//! `RestartPolicy::Always` from the mackesd supervisor provides back-off.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::{Child, Command};
use tracing::{debug, error, info, warn};

use super::{ShutdownToken, Worker};

const DEFAULT_PROD_PATH: &str = "/usr/bin/mde-clipd";

/// Cool-down between unconditional restarts (clean exit 0 → immediate
/// respawn is paced to avoid thundering-herd on compositor restart).
const RESPAWN_COOLDOWN: Duration = Duration::from_secs(2);

/// How long to wait when `$WAYLAND_DISPLAY` is missing before retrying.
const NO_DISPLAY_POLL: Duration = Duration::from_secs(30);

/// Async worker that supervises the `mde-clipd` subprocess.
pub struct ClipdSupervisor {
    binary_path_override: Option<PathBuf>,
    child: Option<Child>,
}

impl ClipdSupervisor {
    /// Construct a supervisor with no binary-path override.
    #[must_use]
    pub fn new() -> Self {
        Self {
            binary_path_override: None,
            child: None,
        }
    }

    /// Pin a specific binary path — used by tests and non-standard installs.
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
        if let Ok(p) = std::env::var("MDE_CLIPD_BIN") {
            let p = PathBuf::from(p);
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }

    async fn spawn_child(&mut self) -> anyhow::Result<()> {
        // Require a Wayland display before spawning.
        if std::env::var("WAYLAND_DISPLAY").is_err() {
            debug!("mde-clipd: $WAYLAND_DISPLAY unset; supervisor idle");
            return Ok(());
        }
        let bin = match self.resolve_binary() {
            Some(p) => p,
            None => {
                debug!("mde-clipd binary not found; supervisor idle");
                return Ok(());
            }
        };
        info!(binary = %bin.display(), "spawning mde-clipd");
        let child = Command::new(&bin)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        self.child = Some(child);
        Ok(())
    }
}

impl Default for ClipdSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Worker for ClipdSupervisor {
    fn name(&self) -> &'static str {
        "clipd_supervisor"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            if self.child.is_none() {
                if let Err(e) = self.spawn_child().await {
                    warn!(error = %e, "mde-clipd spawn failed; will retry");
                }
            }
            match self.child.as_mut() {
                Some(child) => {
                    tokio::select! {
                        wait = child.wait() => {
                            match wait {
                                Ok(status) if status.success() => {
                                    info!("mde-clipd exited cleanly; respawning");
                                }
                                Ok(status) => {
                                    error!(?status, "mde-clipd exited non-zero; will respawn");
                                }
                                Err(e) => {
                                    error!(error = %e, "mde-clipd wait failed");
                                }
                            }
                            self.child = None;
                            tokio::time::sleep(RESPAWN_COOLDOWN).await;
                        }
                        () = shutdown.wait() => {
                            info!("shutdown: killing mde-clipd child");
                            if let Some(mut c) = self.child.take() {
                                let _ = c.kill().await;
                                let _ = c.wait().await;
                            }
                            return Ok(());
                        }
                    }
                }
                None => {
                    // Binary missing or display unavailable — poll slowly.
                    tokio::select! {
                        () = tokio::time::sleep(NO_DISPLAY_POLL) => {}
                        () = shutdown.wait() => { return Ok(()); }
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
        assert_eq!(ClipdSupervisor::new().name(), "clipd_supervisor");
    }

    #[test]
    fn resolve_binary_honors_override() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let w = ClipdSupervisor::new().with_binary(tmp.path().to_path_buf());
        let resolved = w.resolve_binary().expect("override should resolve");
        assert_eq!(resolved, tmp.path().to_path_buf());
    }

    #[test]
    fn resolve_binary_returns_none_when_unavailable() {
        if !std::path::Path::new(DEFAULT_PROD_PATH).is_file()
            && std::env::var("MDE_CLIPD_BIN").is_err()
        {
            let w = ClipdSupervisor::new();
            assert!(w.resolve_binary().is_none());
        }
    }
}
