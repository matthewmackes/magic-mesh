//! VV-2 (v4.1.0) — `voice_config` worker.
//!
//! Owns the `/var/lib/mackesd/voice-desired.json` document the
//! `mackesd voice render-config` CLI reads on every reload of
//! `kamailio-mde.service` + `rtpengine-mde.service`. Two
//! responsibilities:
//!
//! 1. **Seed the document on boot.** If the file is missing
//!    (fresh peer, recovery), write a `VoiceDesired::boot_default`
//!    JSON so the `ExecStartPre` render-config helper produces a
//!    valid config rather than silently skipping the peer.
//!
//! 2. **Reload on change.** Poll the file's mtime every
//!    [`DEFAULT_TICK_INTERVAL`]. When the mtime advances (a
//!    future policy-driven writer flips a new approved
//!    `voice_mesh` / `voice_public` revision into the file, OR an
//!    operator hand-edits during development), shell out to
//!    `systemctl try-reload-or-restart kamailio-mde.service
//!    rtpengine-mde.service` so both daemons pick up the new
//!    config.
//!
//! Today the only writer is this worker's own boot-seed code +
//! manual operator edits. The richer "policy lifecycle writes the
//! file when a revision flips to `applied`" pipeline lands in a
//! future VV-2-followup task — see `docs/PROJECT_WORKLIST.md` for
//! the current state.
//!
//! The worker does NOT regenerate the on-disk Kamailio / `RTPengine`
//! configs itself — that's the `mackesd voice render-config`
//! `ExecStartPre`'s job. Triggering `try-reload-or-restart` re-runs
//! `ExecStartPre`, which re-runs render-config, which reads the new
//! desired.json and writes the four configs. One source of truth;
//! no parallel write paths.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::{ShutdownToken, Worker};

/// Default sweep cadence. Voice policy changes are infrequent +
/// operator-initiated (vs the always-on mesh telemetry), so a
/// 5 s tick is plenty.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(5);

/// Default path of the operator-visible `VoiceDesired` JSON
/// document. Lives under `/var/lib/mackesd/` so the daemon's
/// own user can write it; the `ExecStartPre` helper runs as
/// root and reads from here to produce `/etc/kamailio-mde/*`.
///
/// Re-exports the canonical path defined in
/// [`crate::voice::materialize::DEFAULT_DESIRED_JSON`] (the
/// always-on lib module). Kept under this legacy name so the
/// rest of the async-services tree doesn't need to flip its
/// imports.
pub use crate::voice::materialize::DEFAULT_DESIRED_JSON;

/// Worker handle. Tracks the last-observed mtime so the worker
/// doesn't reload on every tick when the file is unchanged.
pub struct VoiceConfigWorker {
    desired_json: PathBuf,
    node_id: String,
    tick_interval: Duration,
    last_mtime: Option<SystemTime>,
    units_to_reload: Vec<&'static str>,
}

impl VoiceConfigWorker {
    /// Construct a worker pinned to the given node id. Uses the
    /// default desired-json path + default kamailio-mde +
    /// rtpengine-mde unit names.
    #[must_use]
    pub fn new(node_id: String) -> Self {
        Self {
            desired_json: PathBuf::from(DEFAULT_DESIRED_JSON),
            node_id,
            tick_interval: DEFAULT_TICK_INTERVAL,
            last_mtime: None,
            units_to_reload: vec!["kamailio-mde.service", "rtpengine-mde.service"],
        }
    }

    /// Override the desired-json path — used by tests that can't
    /// write under `/var`.
    #[must_use]
    pub fn with_desired_json(mut self, path: PathBuf) -> Self {
        self.desired_json = path;
        self
    }

    /// Override the tick interval — used by tests that need a
    /// faster pulse.
    #[must_use]
    pub fn with_tick_interval(mut self, interval: Duration) -> Self {
        self.tick_interval = interval;
        self
    }

    /// Override the units list — used by tests so the worker
    /// doesn't shell out to the live `systemctl`.
    #[must_use]
    pub fn with_units(mut self, units: Vec<&'static str>) -> Self {
        self.units_to_reload = units;
        self
    }

    /// One tick of the worker's loop. Public so tests can drive
    /// it deterministically without the tokio-time pulse.
    ///
    /// Returns `TickOutcome::Reloaded` if this tick triggered a
    /// reload, `TickOutcome::Idle` otherwise. Errors are
    /// non-fatal — the supervisor's restart policy handles
    /// hard failures.
    pub fn tick_once(&mut self) -> TickOutcome {
        // Seed the file on first tick if it doesn't exist yet —
        // every peer should have *some* desired.json so the
        // `ExecStartPre` helper produces a valid config.
        if !self.desired_json.exists() {
            if let Err(e) = seed_boot_default(&self.desired_json, &self.node_id) {
                tracing::warn!(
                    target: "mackesd::voice_config",
                    error = %e,
                    path = %self.desired_json.display(),
                    "failed to seed boot-default voice-desired.json"
                );
                return TickOutcome::Idle;
            }
        }
        // Look for a forward mtime jump vs the last-seen one.
        match std::fs::metadata(&self.desired_json).and_then(|m| m.modified()) {
            Ok(now) => {
                let advanced = self.last_mtime.map_or(true, |last| now > last);
                self.last_mtime = Some(now);
                if !advanced {
                    return TickOutcome::Idle;
                }
            }
            Err(e) => {
                tracing::warn!(
                    target: "mackesd::voice_config",
                    error = %e,
                    "stat() of voice-desired.json failed"
                );
                return TickOutcome::Idle;
            }
        }
        // mtime advanced — trigger reload on both units. Use
        // `try-reload-or-restart` so the worker is a no-op while
        // the units aren't enabled (the v4.1.0 spec ships the
        // units disabled by default; an operator enables them
        // once VV-4 / VV-14 are green).
        for unit in &self.units_to_reload {
            match try_reload(unit) {
                Ok(()) => tracing::info!(
                    target: "mackesd::voice_config",
                    unit = %unit,
                    "try-reload-or-restart triggered"
                ),
                Err(e) => tracing::warn!(
                    target: "mackesd::voice_config",
                    unit = %unit,
                    error = %e,
                    "try-reload-or-restart failed"
                ),
            }
        }
        TickOutcome::Reloaded
    }
}

/// Per-tick result. Exposed for tests that want to assert the
/// reload edge condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickOutcome {
    /// Nothing to do this tick.
    Idle,
    /// The file mtime advanced; we triggered systemctl reload.
    Reloaded,
}

#[async_trait::async_trait]
impl Worker for VoiceConfigWorker {
    fn name(&self) -> &'static str {
        "voice_config"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            // Drop into a blocking tick — the work is std::fs +
            // std::process, no point pretending it's async.
            let _ = self.tick_once();
            tokio::select! {
                _ = shutdown.wait() => break,
                _ = tokio::time::sleep(self.tick_interval) => {},
            }
        }
        Ok(())
    }
}

fn seed_boot_default(path: &Path, node_id: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("voice_config: mkdir {}: {e}", parent.display()))?;
    }
    let desired = mde_voice_config::VoiceDesired::boot_default(node_id);
    let body = serde_json::to_string_pretty(&desired)
        .map_err(|e| anyhow::anyhow!("voice_config: serialize boot_default: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body.as_bytes())
        .map_err(|e| anyhow::anyhow!("voice_config: write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        anyhow::anyhow!(
            "voice_config: rename {} → {}: {e}",
            tmp.display(),
            path.display()
        )
    })?;
    Ok(())
}

fn try_reload(unit: &str) -> Result<(), String> {
    let out = std::process::Command::new("systemctl")
        .args(["try-reload-or-restart", unit])
        .output()
        .map_err(|e| format!("systemctl try-reload-or-restart {unit}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_name_is_voice_config() {
        let w = VoiceConfigWorker::new("peer:test".to_owned());
        assert_eq!(w.name(), "voice_config");
    }

    #[test]
    fn first_tick_seeds_missing_file_with_boot_default() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("voice-desired.json");
        let mut w = VoiceConfigWorker::new("peer:alice".to_owned())
            .with_desired_json(path.clone())
            // No units to reload — keep the test out of systemctl.
            .with_units(vec![]);
        assert!(!path.exists());
        let outcome = w.tick_once();
        assert!(path.exists(), "first tick must write the boot-default file");
        // Reading it back, the JSON should round-trip.
        let body = std::fs::read_to_string(&path).expect("read seeded file");
        let parsed: mde_voice_config::VoiceDesired =
            serde_json::from_str(&body).expect("seeded file is valid JSON");
        assert_eq!(parsed.node_id, "peer:alice");
        assert_eq!(parsed.mesh_bind_device, "nebula1");
        // First tick after seeding *should* report Reloaded since
        // the mtime is freshly observed; subsequent identical
        // ticks should report Idle.
        assert_eq!(outcome, TickOutcome::Reloaded);
    }

    #[test]
    fn second_tick_against_unchanged_file_is_idle() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("voice-desired.json");
        let mut w = VoiceConfigWorker::new("peer:bob".to_owned())
            .with_desired_json(path.clone())
            .with_units(vec![]);
        let _ = w.tick_once(); // seed + first reload edge
        let outcome = w.tick_once();
        assert_eq!(outcome, TickOutcome::Idle);
    }

    #[test]
    fn tick_re_reloads_when_file_mtime_advances() {
        use std::thread::sleep;
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("voice-desired.json");
        let mut w = VoiceConfigWorker::new("peer:carol".to_owned())
            .with_desired_json(path.clone())
            .with_units(vec![]);
        let _ = w.tick_once(); // first edge
        let _ = w.tick_once(); // idle
                               // Touch the file so its mtime moves forward. mtime
                               // resolution on Linux is nanosecond, but some filesystems
                               // round to seconds — wait > 1 s to be safe across CI hosts.
        sleep(Duration::from_millis(1100));
        std::fs::write(&path, "{\"node_id\":\"peer:carol\",\"mesh_bind_device\":\"nebula1\",\"mesh_bind_address\":\"0.0.0.0\",\"rtp_port_min\":30000,\"rtp_port_max\":40000}").expect("rewrite");
        let outcome = w.tick_once();
        assert_eq!(outcome, TickOutcome::Reloaded);
    }

    #[tokio::test]
    async fn worker_exits_on_shutdown_token() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut w = VoiceConfigWorker::new("peer:test".to_owned())
            .with_desired_json(tmp.path().join("voice-desired.json"))
            .with_units(vec![])
            .with_tick_interval(Duration::from_millis(50));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let _ = tx.send(true);
        let result = tokio::time::timeout(Duration::from_secs(3), w.run(token))
            .await
            .expect("worker must exit on shutdown");
        assert!(result.is_ok());
    }

    #[test]
    fn seed_creates_parent_dirs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let nested = tmp.path().join("nested").join("dirs").join("d.json");
        seed_boot_default(&nested, "peer:d").expect("seed");
        assert!(nested.exists());
    }
}
