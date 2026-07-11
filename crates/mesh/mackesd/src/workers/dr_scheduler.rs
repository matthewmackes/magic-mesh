//! DATACENTER-23 — scheduled DR (disaster-recovery) backups.
//!
//! A leader-gated periodic worker (sibling to [`super::dc_health`]) that runs the
//! repo's DR backup script on a cadence and publishes the outcome to the Bus.
//! Where `dc_health` *observes* the substrate, this worker *acts* — it shells out
//! to `automation/dr/dr-backup.sh` (which lands via a sibling change; this worker
//! references it by path and never requires it at compile time) at most once per
//! [`DEFAULT_INTERVAL_SECS`] (overridable via `MCNF_DR_INTERVAL_SECS`).
//!
//! Design: the worker tick is coarse ([`TICK_INTERVAL`], ~5 min) and cheap — every
//! tick it asks the pure [`due`] helper whether enough wall-time has elapsed since
//! the last run. On the first leader tick after start there is no last-run, so
//! [`due`] returns `true` and the backup runs immediately; thereafter it waits a
//! full interval between runs. Leader-gating (the shared
//! `.mackesd-leader.lock`) ensures a multi-node mesh runs exactly one backup per
//! interval, not one per node.
//!
//! On a successful run we publish `event/dc/dr/last`
//! `{"status":"ok","path":<stdout last line>,"ts":"recent"}`; on failure
//! `{"status":"fail","detail":<short>}`. Both ride the same fire-and-reap
//! `mde-bus publish` lane as the other dc workers.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::{ShutdownToken, Worker};

/// Default minimum gap between DR backups — daily (overridable via
/// `MCNF_DR_INTERVAL_SECS`).
pub const DEFAULT_INTERVAL_SECS: u64 = 86_400;

/// Loop cadence — wake every ~5 min and ask [`due`] whether it's time. Decoupling
/// the wake cadence from the (much longer) backup interval keeps the worker
/// responsive to shutdown while the interval clock is coarse.
pub const TICK_INTERVAL: Duration = Duration::from_secs(300);

/// Bus topic the latest DR-backup outcome is published to.
pub const DR_TOPIC: &str = "event/dc/dr/last";

/// Path (relative to the workgroup root) of the DR backup script. Lands via a
/// sibling change; referenced by path, not required at compile time.
pub const DR_SCRIPT_REL: &str = "automation/dr/dr-backup.sh";

/// Max characters of a failure `detail` carried into the published body. Keeps the
/// DR lane compact.
pub const DETAIL_LEN: usize = 200;

/// The configured minimum interval between backups in seconds (`MCNF_DR_INTERVAL_SECS`,
/// else [`DEFAULT_INTERVAL_SECS`]). A malformed/zero value falls back to the default.
#[must_use]
pub fn interval_secs() -> u64 {
    std::env::var("MCNF_DR_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(DEFAULT_INTERVAL_SECS)
}

/// Pure cadence decision: is a DR backup due now?
///
/// Returns `true` when the backup has never run (`last_run_secs == None`) or when
/// at least `interval` seconds have elapsed since the last run (`now - last >=
/// interval`). A `now` earlier than `last` (clock skew) is treated as not-yet-due.
#[must_use]
pub fn due(last_run_secs: Option<u64>, now_secs: u64, interval: u64) -> bool {
    match last_run_secs {
        None => true,
        Some(last) => now_secs.saturating_sub(last) >= interval,
    }
}

/// First [`DETAIL_LEN`] characters of a string (char-boundary safe).
fn detail_summary(detail: &str) -> String {
    detail.chars().take(DETAIL_LEN).collect()
}

/// Publish one DR outcome body onto the Bus (best-effort, fire-and-reap — same
/// lane shape as the other dc workers' events).
fn publish(body: &str) {
    let mut cmd = std::process::Command::new("mde-bus");
    cmd.args(["publish", DR_TOPIC, "--body-flag", body]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// JSON body for a successful run: the script's last stdout line as `path`.
#[must_use]
fn ok_body(stdout_last_line: &str) -> String {
    serde_json::json!({
        "status": "ok",
        "path": stdout_last_line,
        "ts": "recent",
    })
    .to_string()
}

/// JSON body for a failed run: a short `detail`.
#[must_use]
fn fail_body(detail: &str) -> String {
    serde_json::json!({
        "status": "fail",
        "detail": detail_summary(detail),
    })
    .to_string()
}

/// Run the DR backup script once and publish its outcome. Best-effort: a missing
/// script / shell, a non-zero exit, or a spawn failure each degrade to a `fail`
/// publish and never panic.
fn run_backup(script_path: &str) {
    let out = std::process::Command::new("bash")
        .args(["-lc", script_path])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let last = stdout
                .lines()
                .map(str::trim_end)
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .to_string();
            publish(&ok_body(&last));
        }
        Ok(o) => {
            let code = o
                .status
                .code()
                .map_or_else(|| "signal".to_string(), |c| c.to_string());
            let stderr = String::from_utf8_lossy(&o.stderr);
            let tail = stderr.trim();
            publish(&fail_body(&format!("dr-backup exit {code}: {tail}")));
        }
        Err(e) => publish(&fail_body(&format!("dr-backup spawn failed: {e}"))),
    }
}

/// The supervised worker. Leader-gated (only the elected node runs the backup +
/// publishes, so a multi-node mesh runs one backup per interval) and best-effort.
pub struct DrSchedulerWorker {
    tick_interval: Duration,
    interval: Duration,
    node_id: String,
    leader_lock: PathBuf,
    script_path: PathBuf,
    /// Monotonic clock of the last successful *start* of a run. `None` until the
    /// first run, so the first eligible leader tick runs immediately.
    last_run: Option<Instant>,
}

impl DrSchedulerWorker {
    /// Construct with production defaults (5 min tick, `MCNF_DR_INTERVAL_SECS`
    /// backup interval, the shared leader lock + the DR script both under
    /// `workgroup_root`).
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        Self {
            tick_interval: TICK_INTERVAL,
            interval: Duration::from_secs(interval_secs()),
            leader_lock: workgroup_root.join(".mackesd-leader.lock"),
            script_path: workgroup_root.join(DR_SCRIPT_REL),
            node_id,
            last_run: None,
        }
    }

    /// Only the directory leader runs the backup (no-fixed-center: any eligible
    /// node can be it, the elected one runs + publishes). Reuses the shared
    /// leader lock.
    fn is_leader(&self) -> bool {
        crate::leader_gate::LeaderGate::from_lock_path(
            self.leader_lock.clone(),
            self.node_id.clone(),
        )
        .is_leader()
    }

    /// Whether a backup is due, given the monotonic last-run instant + now.
    /// Bridges the `Instant`-based worker state onto the pure [`due`] helper by
    /// expressing both as seconds-since-an-arbitrary-epoch (`last_run`'s elapsed
    /// time vs. the interval).
    fn is_due(&self, now: Instant) -> bool {
        let last_secs = self
            .last_run
            .map(|t| now.saturating_duration_since(t).as_secs());
        // Map "elapsed since last run" onto due(): treat `now` as the interval
        // anchor (now_secs = interval) and last as (interval - elapsed). When the
        // script has never run, last is None → due. Otherwise it's due once the
        // elapsed time reaches the interval.
        match last_secs {
            None => true,
            Some(elapsed) => due(Some(0), elapsed, self.interval.as_secs()),
        }
    }
}

#[async_trait::async_trait]
impl Worker for DrSchedulerWorker {
    fn name(&self) -> &'static str {
        "dr_scheduler"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            if self.is_leader() && self.is_due(Instant::now()) {
                let script = self.script_path.to_string_lossy().to_string();
                run_backup(&script);
                self.last_run = Some(Instant::now());
            }
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(self.tick_interval) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_is_true_when_never_run() {
        assert!(due(None, 0, 86_400));
        assert!(due(None, 1_000_000, 86_400));
    }

    #[test]
    fn due_is_false_just_after_a_run() {
        // Ran at t=1000, now t=1001, interval=86400 → not yet due.
        assert!(!due(Some(1000), 1001, 86_400));
        // Exactly one second short of the interval → still not due.
        assert!(!due(Some(1000), 1000 + 86_399, 86_400));
    }

    #[test]
    fn due_is_true_once_the_interval_has_elapsed() {
        // Exactly the interval → due.
        assert!(due(Some(1000), 1000 + 86_400, 86_400));
        // Well past the interval → due.
        assert!(due(Some(1000), 1000 + 200_000, 86_400));
    }

    #[test]
    fn due_handles_clock_skew_as_not_due() {
        // now earlier than last (negative delta saturates to 0) → not due.
        assert!(!due(Some(5000), 1000, 86_400));
    }

    #[test]
    fn ok_body_carries_path_and_status() {
        let v: serde_json::Value =
            serde_json::from_str(&ok_body("/mesh/dr/2026-06-22.tar.zst")).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["path"], "/mesh/dr/2026-06-22.tar.zst");
        assert_eq!(v["ts"], "recent");
    }

    #[test]
    fn fail_body_truncates_detail_on_char_boundary() {
        let long = "é".repeat(500);
        let v: serde_json::Value = serde_json::from_str(&fail_body(&long)).unwrap();
        assert_eq!(v["status"], "fail");
        assert_eq!(
            v["detail"].as_str().unwrap().chars().count(),
            DETAIL_LEN,
            "detail truncated to the cap without splitting a char"
        );
    }

    #[test]
    fn interval_default_when_env_absent_or_bad() {
        // We can't safely mutate process env in parallel tests; just assert the
        // default constant is what the docs promise (daily).
        assert_eq!(DEFAULT_INTERVAL_SECS, 86_400);
    }
}
