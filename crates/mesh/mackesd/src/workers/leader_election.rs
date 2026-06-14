//! Continuous leader election (ONBOARD-6).
//!
//! Leadership is a 60s lease on `<QNM-Shared>/.mackesd-leader.lock`
//! ([`crate::leader`]). The election primitive existed and `force_take`
//! / the upgrade-intent watcher could acquire it, but **nothing renewed
//! it continuously** — the upgrade watcher only calls `try_acquire`
//! while processing an in-flight upgrade, so in steady state no node
//! ever claimed the lock, the Workbench showed "NO LEADER", and every
//! leader-gated surface stayed dark even with QNM-Shared mounted.
//!
//! This worker closes that: every [`crate::leader::RENEW_INTERVAL`] it
//! calls [`crate::leader::try_acquire`], which acquires the lease if
//! free/expired and renews it if we already hold it — so exactly one
//! node holds a fresh lock at all times and the directory/fleet/health
//! services that gate on leadership actually run.
//!
//! Requires QNM-Shared to be a shared mount (so all nodes contend for
//! the same lock file); a missing/unmounted root just logs a warning
//! and retries — honest degradation, never a crash.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::time::Duration;

use super::{ShutdownToken, Worker};
use crate::leader::{try_acquire, AcquireResult};

/// Worker handle.
pub struct LeaderElection {
    lock_path: PathBuf,
    node_id: String,
    tick: Duration,
}

impl LeaderElection {
    /// Elect on `<workgroup_root>/.mackesd-leader.lock` as `node_id`,
    /// renewing every [`crate::leader::RENEW_INTERVAL`] (well inside the
    /// 60s lease).
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        Self {
            lock_path: workgroup_root.join(".mackesd-leader.lock"),
            node_id,
            tick: crate::leader::RENEW_INTERVAL,
        }
    }

    /// Override the renew cadence (tests use a short value).
    #[must_use]
    pub fn with_tick(mut self, tick: Duration) -> Self {
        self.tick = tick;
        self
    }

    /// One election attempt — acquire or renew the lease. Returns the
    /// outcome (also for tests). A QNM-Shared I/O error (unmounted root)
    /// logs + reports `None` rather than failing the worker.
    pub fn tick_once(&self) -> Option<AcquireResult> {
        match try_acquire(&self.lock_path, &self.node_id) {
            Ok(result) => {
                match &result {
                    AcquireResult::Acquired => tracing::info!(
                        target: "mackesd::leader_election",
                        node_id = %self.node_id,
                        "acquired/renewed mesh leadership lease",
                    ),
                    AcquireResult::HeldBy {
                        leader_id,
                        lease_remaining_s,
                    } => tracing::debug!(
                        target: "mackesd::leader_election",
                        leader = %leader_id, lease_remaining_s,
                        "following current mesh leader",
                    ),
                    AcquireResult::ExpiredLease => tracing::debug!(
                        target: "mackesd::leader_election",
                        "leader lease expired; will contend next tick",
                    ),
                }
                Some(result)
            }
            Err(e) => {
                tracing::warn!(
                    target: "mackesd::leader_election",
                    error = %e,
                    lock = %self.lock_path.display(),
                    "leader election I/O failed (is QNM-Shared mounted?); retrying",
                );
                None
            }
        }
    }
}

#[async_trait::async_trait]
impl Worker for LeaderElection {
    fn name(&self) -> &'static str {
        "leader_election"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // First attempt immediately so a fresh mesh elects a leader
        // without waiting a full renew interval.
        self.tick_once();
        loop {
            tokio::select! {
                () = tokio::time::sleep(self.tick) => { self.tick_once(); }
                () = shutdown.wait() => {
                    tracing::info!(target: "mackesd::leader_election", "shutdown requested");
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_node_acquires_then_renews() {
        let tmp = tempfile::tempdir().unwrap();
        let w = LeaderElection::new(tmp.path().to_path_buf(), "peer:a".into());
        // First tick acquires.
        assert!(matches!(w.tick_once(), Some(AcquireResult::Acquired)));
        assert!(tmp.path().join(".mackesd-leader.lock").exists());
        // Subsequent ticks renew (still Acquired for the same node).
        assert!(matches!(w.tick_once(), Some(AcquireResult::Acquired)));
    }

    #[test]
    fn name_is_kebab_stable() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            LeaderElection::new(tmp.path().to_path_buf(), "n".into()).name(),
            "leader_election"
        );
    }
}
