//! MEDIA-3 — self-healing Navidrome supervisor for `Lighthouse_Media`.
//!
//! The deploy substrate already lives in `install-helpers/setup-media-navidrome.sh`:
//! it writes `mcnf-music-store.service` (rclone mount) and `mcnf-navidrome.service`
//! (capped Podman Navidrome). This worker adopts those units instead of
//! duplicating their full definitions in Rust: on a media lighthouse it installs
//! them via the helper when absent, then keeps both enabled and active.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{ShutdownToken, Worker};

/// The rclone-backed music-library mount unit.
pub const STORE_UNIT: &str = "mcnf-music-store.service";
/// The Navidrome container unit.
pub const NAVIDROME_UNIT: &str = "mcnf-navidrome.service";
/// Packaged setup helper that materializes creds and writes both units.
pub const DEFAULT_SETUP_HELPER: &str = "/usr/libexec/mackesd/setup-media-navidrome";
/// Repo/dev fallback for a non-installed checkout.
pub const REPO_SETUP_HELPER: &str = "install-helpers/setup-media-navidrome.sh";

const DEFAULT_INTERVAL: Duration = Duration::from_secs(60);

/// One idempotent reconciliation action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Run the setup helper because one or both units are absent.
    RunSetup(PathBuf),
    /// Ensure a unit is enabled and active.
    EnableNow(&'static str),
}

/// Unit state as observed by the runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitState {
    /// Whether systemd knows the unit.
    pub exists: bool,
    /// Whether the unit is active.
    pub active: bool,
}

impl UnitState {
    /// Unit exists and is active.
    #[must_use]
    pub const fn active() -> Self {
        Self {
            exists: true,
            active: true,
        }
    }

    /// Unit exists but is not active.
    #[must_use]
    pub const fn inactive() -> Self {
        Self {
            exists: true,
            active: false,
        }
    }

    /// Unit is absent/unknown to systemd.
    #[must_use]
    pub const fn missing() -> Self {
        Self {
            exists: false,
            active: false,
        }
    }
}

/// Pure MEDIA-3 reconciliation planner.
#[must_use]
pub fn plan_tick(store: UnitState, navidrome: UnitState, helper: Option<&Path>) -> Vec<Action> {
    if !store.exists || !navidrome.exists {
        return helper
            .map(|path| vec![Action::RunSetup(path.to_path_buf())])
            .unwrap_or_default();
    }

    let mut actions = Vec::new();
    if !store.active {
        actions.push(Action::EnableNow(STORE_UNIT));
    }
    if !navidrome.active {
        actions.push(Action::EnableNow(NAVIDROME_UNIT));
    }
    actions
}

/// System boundary for the worker; tests use a fake.
pub trait MediaNavidromeOps: Send {
    /// Observe a unit's current existence/activity.
    fn unit_state(&mut self, unit: &str) -> UnitState;
    /// Find the setup helper to run when units are absent.
    fn setup_helper(&mut self) -> Option<PathBuf>;
    /// Run the setup helper.
    fn run_setup(&mut self, helper: &Path) -> Result<(), String>;
    /// Enable and start a unit.
    fn enable_now(&mut self, unit: &str) -> Result<(), String>;
}

/// Real systemd/helper runner.
#[derive(Default)]
pub struct SystemOps;

impl SystemOps {
    fn systemctl_status(args: &[&str]) -> bool {
        std::process::Command::new("systemctl")
            .args(args)
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
}

impl MediaNavidromeOps for SystemOps {
    fn unit_state(&mut self, unit: &str) -> UnitState {
        UnitState {
            exists: Self::systemctl_status(["cat", unit].as_slice()),
            active: Self::systemctl_status(["is-active", "--quiet", unit].as_slice()),
        }
    }

    fn setup_helper(&mut self) -> Option<PathBuf> {
        [DEFAULT_SETUP_HELPER, REPO_SETUP_HELPER]
            .into_iter()
            .map(PathBuf::from)
            .find(|path| path.exists())
    }

    fn run_setup(&mut self, helper: &Path) -> Result<(), String> {
        let out = std::process::Command::new(helper)
            .output()
            .map_err(|e| format!("run {}: {e}", helper.display()))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "{} exited {}: {}",
                helper.display(),
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    fn enable_now(&mut self, unit: &str) -> Result<(), String> {
        let out = std::process::Command::new("systemctl")
            .args(["enable", "--now", unit])
            .output()
            .map_err(|e| format!("systemctl enable --now {unit}: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "systemctl enable --now {unit} exited {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }
}

/// Worker wrapper.
pub struct MediaNavidromeWorker<O = SystemOps> {
    ops: O,
    interval: Duration,
}

impl MediaNavidromeWorker<SystemOps> {
    /// Production worker.
    #[must_use]
    pub fn new() -> Self {
        Self {
            ops: SystemOps,
            interval: DEFAULT_INTERVAL,
        }
    }
}

impl Default for MediaNavidromeWorker<SystemOps> {
    fn default() -> Self {
        Self::new()
    }
}

impl<O: MediaNavidromeOps> MediaNavidromeWorker<O> {
    /// Test/custom worker.
    #[must_use]
    pub const fn with_ops(ops: O, interval: Duration) -> Self {
        Self { ops, interval }
    }

    /// Reconcile once. Returns the planned actions even when an action fails, so
    /// tests can assert the decision separately from the system boundary.
    pub fn tick_once(&mut self) -> Result<Vec<Action>, String> {
        let store = self.ops.unit_state(STORE_UNIT);
        let navidrome = self.ops.unit_state(NAVIDROME_UNIT);
        let helper = self.ops.setup_helper();
        let actions = plan_tick(store, navidrome, helper.as_deref());

        for action in &actions {
            match action {
                Action::RunSetup(path) => self.ops.run_setup(path)?,
                Action::EnableNow(unit) => self.ops.enable_now(unit)?,
            }
        }
        Ok(actions)
    }
}

#[async_trait::async_trait]
impl<O: MediaNavidromeOps + 'static> Worker for MediaNavidromeWorker<O> {
    fn name(&self) -> &'static str {
        "media_navidrome"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let _ = self.tick_once();
        let mut ticker = tokio::time::interval(self.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = shutdown.wait() => break,
                _ = ticker.tick() => {
                    if let Err(err) = self.tick_once() {
                        tracing::warn!(error = %err, "media_navidrome: reconcile failed");
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeOps {
        store: UnitState,
        nav: UnitState,
        helper: Option<PathBuf>,
        calls: Vec<String>,
    }

    impl MediaNavidromeOps for FakeOps {
        fn unit_state(&mut self, unit: &str) -> UnitState {
            match unit {
                STORE_UNIT => self.store,
                NAVIDROME_UNIT => self.nav,
                other => panic!("unexpected unit {other}"),
            }
        }

        fn setup_helper(&mut self) -> Option<PathBuf> {
            self.helper.clone()
        }

        fn run_setup(&mut self, helper: &Path) -> Result<(), String> {
            self.calls.push(format!("setup:{}", helper.display()));
            Ok(())
        }

        fn enable_now(&mut self, unit: &str) -> Result<(), String> {
            self.calls.push(format!("enable:{unit}"));
            Ok(())
        }
    }

    #[test]
    fn plan_runs_setup_when_units_are_missing() {
        let helper = Path::new(DEFAULT_SETUP_HELPER);
        assert_eq!(
            plan_tick(UnitState::missing(), UnitState::missing(), Some(helper)),
            vec![Action::RunSetup(helper.to_path_buf())]
        );
    }

    #[test]
    fn plan_does_nothing_when_missing_units_have_no_helper() {
        assert!(plan_tick(UnitState::missing(), UnitState::missing(), None).is_empty());
    }

    #[test]
    fn plan_starts_only_inactive_existing_units() {
        assert_eq!(
            plan_tick(UnitState::inactive(), UnitState::active(), None),
            vec![Action::EnableNow(STORE_UNIT)]
        );
        assert_eq!(
            plan_tick(UnitState::active(), UnitState::inactive(), None),
            vec![Action::EnableNow(NAVIDROME_UNIT)]
        );
    }

    #[test]
    fn tick_executes_the_planned_actions() {
        let helper = PathBuf::from("/tmp/setup-media-navidrome.sh");
        let ops = FakeOps {
            store: UnitState::missing(),
            nav: UnitState::missing(),
            helper: Some(helper.clone()),
            calls: Vec::new(),
        };
        let mut worker = MediaNavidromeWorker::with_ops(ops, Duration::from_secs(1));
        let actions = worker.tick_once().expect("tick");
        assert_eq!(actions, vec![Action::RunSetup(helper.clone())]);
        assert_eq!(
            worker.ops.calls,
            vec![format!("setup:{}", helper.display())]
        );
    }
}
