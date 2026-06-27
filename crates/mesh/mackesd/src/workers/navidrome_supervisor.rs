//! MEDIA-pkg-2 — Navidrome self-heal supervisor.
//!
//! Runs ONLY on a `Lighthouse_Media` node (capability-gated like
//! [`media_registry`](super::media_registry), via `runs_in("navidrome",
//! deploy_class)`). Unlike [`bus_supervisor`](super::bus_supervisor) (which
//! spawns a binary), Navidrome runs as the systemd unit `mcnf-navidrome.service`
//! that `setup-media-navidrome.sh` installs with `Restart=always`. This worker
//! ADOPTS that unit: each tick it
//!   * does nothing if the unit is active (systemd's Restart=always handles
//!     ordinary crashes),
//!   * `systemctl restart`s it if it's installed but down (belt-and-suspenders
//!     past systemd's own restart limits),
//!   * re-provisions via the RPM-shipped `setup-media-navidrome` (MEDIA-pkg) if
//!     the unit is MISSING and the creds env is present (a node that gained the
//!     Media capability but was never set up self-heals),
//!   * logs a clear "needs setup" if the unit is missing AND no creds env exists.
//!
//! The decision is the pure [`decide`] fn (unit-tested); `tick_once` is the thin
//! systemctl/exec shell.

#![cfg(feature = "async-services")]

use std::process::Command;
use std::time::Duration;

use super::{ShutdownToken, Worker};

/// 30 s tick — Navidrome is slow-changing; systemd's Restart=always handles the
/// fast path, so this only needs to catch down/missing within ~30 s.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(30);

/// The systemd unit `setup-media-navidrome.sh` installs.
const UNIT: &str = "mcnf-navidrome.service";
/// RPM-shipped bring-up script (MEDIA-pkg asset → /usr/libexec/mackesd/...).
const SETUP: &str = "/usr/libexec/mackesd/setup-media-navidrome";
/// Default creds env the bring-up needs (DO_SPACES_* + ND_ADMIN_*).
const CREDS: &str = "/etc/mackesd/media-spaces.env";

/// What the supervisor should do this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Unit is active — nothing to do.
    Healthy,
    /// Unit installed but down — restart it.
    Restart,
    /// Unit missing but creds present — re-provision via the setup script.
    Reprovision,
    /// Unit missing + no creds — can't adopt; log for the operator.
    NeedsSetup,
}

/// Pure supervisor decision from (unit active?, unit installed?, creds present?).
/// `active` wins regardless of the others; otherwise restart an installed unit,
/// re-provision a missing one when creds exist, else flag needs-setup.
#[must_use]
fn decide(active: bool, unit_installed: bool, creds_present: bool) -> Action {
    if active {
        Action::Healthy
    } else if unit_installed {
        Action::Restart
    } else if creds_present {
        Action::Reprovision
    } else {
        Action::NeedsSetup
    }
}

/// The MEDIA-pkg-2 worker.
pub struct NavidromeSupervisor {
    tick: Duration,
}

impl NavidromeSupervisor {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tick: DEFAULT_TICK_INTERVAL,
        }
    }

    fn tick_once(&self) {
        let active = sc(&["is-active", "--quiet", UNIT]);
        let installed = sc(&["cat", UNIT]); // exit 0 iff the unit file exists
        let creds = std::path::Path::new(CREDS).exists();
        match decide(active, installed, creds) {
            Action::Healthy => {}
            Action::Restart => {
                tracing::warn!("navidrome_supervisor: {UNIT} down — restarting");
                let _ = run(&["restart", UNIT]);
            }
            Action::Reprovision => {
                tracing::warn!(
                    "navidrome_supervisor: {UNIT} missing — re-provisioning via {SETUP}"
                );
                // The setup script is idempotent; default args read CREDS.
                let _ = Command::new(SETUP).output();
            }
            Action::NeedsSetup => {
                tracing::warn!(
                    "navidrome_supervisor: {UNIT} missing + no creds env at {CREDS}; \
                     cannot adopt (run setup-media-navidrome with the media-spaces secret)"
                );
            }
        }
    }
}

/// `systemctl <args>` → true on exit 0 (false on any failure/absence).
fn sc(args: &[&str]) -> bool {
    run(args).is_some_and(|o| o.status.success())
}

fn run(args: &[&str]) -> Option<std::process::Output> {
    Command::new("systemctl").args(args).output().ok()
}

#[async_trait::async_trait]
impl Worker for NavidromeSupervisor {
    fn name(&self) -> &'static str {
        "navidrome_supervisor"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(self.tick) => { self.tick_once(); }
                _ = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supervise_decision_matrix() {
        // active wins regardless
        assert_eq!(decide(true, true, true), Action::Healthy);
        assert_eq!(decide(true, false, false), Action::Healthy);
        // down + installed → restart
        assert_eq!(decide(false, true, true), Action::Restart);
        assert_eq!(decide(false, true, false), Action::Restart);
        // missing + creds → re-provision
        assert_eq!(decide(false, false, true), Action::Reprovision);
        // missing + no creds → needs setup
        assert_eq!(decide(false, false, false), Action::NeedsSetup);
    }
}
