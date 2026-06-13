//! EPIC-MESH-PROBE (MESH-PROBE-4) — the scheduled probe worker.
//!
//! This is the thin async supervisor wrapper around the synchronous
//! probe cycle that lives (ungated) in [`crate::probe_nmap`]. The
//! worker ticks on the fast interval (liveness + curated ports) and
//! promotes to a deep `-sV`/NSE pass on the slower interval (Q6
//! two-tier cadence); each cycle resolves mesh-peer targets, scans
//! them, writes this peer's `probe-inventory.json` into mesh-home,
//! and announces `probe/changed` on the Bus when the inventory
//! changed. Spawned from `run_serve`.
//!
//! The cycle body + inventory write + Bus publish are sync (no tokio),
//! so they live in `probe_nmap` where the `mackesd probe scan/refresh`
//! CLI can reach them without the `async-services` feature; this
//! module only adds the [`Worker`] + cadence-timer integration.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;

use super::{ShutdownToken, Worker};
use crate::probe_nmap::{run_probe_cycle, DEFAULT_NMAP_BINARY};

/// Fast-tier cadence — liveness + curated-port pass (Q6).
pub const FAST_TICK: Duration = Duration::from_secs(60);
/// Deep-tier cadence — `-sV`/NSE identification pass (Q6).
pub const DEEP_INTERVAL: Duration = Duration::from_secs(600);
/// Default bundled-NSE script dir (MESH-PROBE-3 install path).
pub const DEFAULT_NSE_DIR: &str = "/usr/share/mde/nmap";

/// The scheduled probe worker.
pub struct ProbeWorker {
    workgroup_root: PathBuf,
    self_node_id: String,
    home: PathBuf,
    nmap_binary: String,
    nse_dir: String,
    fast_tick: Duration,
    deep_interval: Duration,
    last_deep: std::sync::Mutex<Option<std::time::Instant>>,
}

impl ProbeWorker {
    /// Construct the worker for `self_node_id` scanning the mesh rooted
    /// at `workgroup_root`. Uses the system `nmap` + the bundled NSE dir +
    /// `$HOME` for the arbitrary-target / do-not-scan config files.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, self_node_id: String) -> Self {
        let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/root"), PathBuf::from);
        Self {
            workgroup_root,
            self_node_id,
            home,
            nmap_binary: DEFAULT_NMAP_BINARY.to_owned(),
            nse_dir: DEFAULT_NSE_DIR.to_owned(),
            fast_tick: FAST_TICK,
            deep_interval: DEEP_INTERVAL,
            last_deep: std::sync::Mutex::new(None),
        }
    }

    /// `true` when the deep interval has elapsed since the last deep
    /// pass (or none yet). Mutex-guarded; mirrors the gluster worker's
    /// `quota_probe_due` rate-limit so the heavy `-sV`/NSE pass fires
    /// at most once per `deep_interval`.
    fn deep_due(&self) -> bool {
        let mut guard = self
            .last_deep
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = std::time::Instant::now();
        let due = match *guard {
            None => true,
            Some(last) => now.duration_since(last) >= self.deep_interval,
        };
        if due {
            *guard = Some(now);
        }
        due
    }

    fn tick_once(&self) {
        let deep = self.deep_due();
        run_probe_cycle(
            &self.workgroup_root,
            &self.self_node_id,
            &self.home,
            &self.nmap_binary,
            &self.nse_dir,
            deep,
        );
    }
}

#[async_trait]
impl Worker for ProbeWorker {
    fn name(&self) -> &'static str {
        "probe"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // First tick is a deep pass (last_deep is None) so a fresh
        // daemon publishes a full inventory promptly.
        self.tick_once();
        loop {
            tokio::select! {
                _ = shutdown.wait() => return Ok(()),
                _ = tokio::time::sleep(self.fast_tick) => self.tick_once(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_due_fires_once_then_rate_limits() {
        let w = ProbeWorker::new(PathBuf::from("/qnm"), "self".into());
        assert!(w.deep_due(), "first call is due");
        assert!(!w.deep_due(), "immediately after, rate-limited");
    }

    #[test]
    fn worker_name_is_probe() {
        assert_eq!(
            ProbeWorker::new(PathBuf::from("/x"), "n".into()).name(),
            "probe"
        );
    }
}
