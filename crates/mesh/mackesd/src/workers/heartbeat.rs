//! v2.0.0 Phase B.8 — heartbeat worker reparented under `workers/`.
//!
//! The actual implementation lives in [`crate::telemetry`] (shipped
//! in Phase 12.3.3). This module surfaces it as a typed worker the
//! Phase A.2 supervisor can register alongside the other Phase B
//! workers, so every long-running task in the unified backend goes
//! through the same trait + restart-policy + shutdown plumbing.
//!
//! The legacy sync entry point (`telemetry::spawn_heartbeat_worker`)
//! stays callable for the v1.x reconcile binary path until Phase B
//! is fully in place. New code should construct [`HeartbeatWorker`]
//! and hand it to the supervisor instead.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::{ShutdownToken, Worker};

/// Async worker wrapping [`crate::telemetry::spawn_heartbeat_worker`].
///
/// Re-derives the worker on every supervisor restart so a transient
/// disk failure (QNM-Shared umount, e.g.) doesn't keep the worker
/// pinned to a stale path.
pub struct HeartbeatWorker {
    workgroup_root: PathBuf,
    node_id: String,
    interval: Duration,
}

impl HeartbeatWorker {
    /// Construct a new worker pinned to the given QNM-Shared root
    /// and stable node id, writing at the locked default cadence
    /// ([`crate::telemetry::HEARTBEAT_INTERVAL_S`]). Use
    /// [`Self::with_interval`] to apply the operator-tuned cadence.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        Self {
            workgroup_root,
            node_id,
            interval: Duration::from_secs(crate::telemetry::HEARTBEAT_INTERVAL_S),
        }
    }

    /// Override the heartbeat write cadence (E1.3 #3 —
    /// `/etc/mackesd/mackesd.toml`'s `heartbeat_interval_secs`).
    #[must_use]
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }
}

#[async_trait::async_trait]
impl Worker for HeartbeatWorker {
    fn name(&self) -> &'static str {
        "heartbeat"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // Reuse the existing sync spawn_heartbeat_worker — it owns
        // the actual cadence + write semantics. We just bridge the
        // async ShutdownToken into the sync AtomicBool it expects.
        let flag = Arc::new(AtomicBool::new(false));
        let handle = crate::telemetry::spawn_heartbeat_worker(
            self.workgroup_root.clone(),
            self.node_id.clone(),
            self.interval,
            Arc::clone(&flag),
        );
        // Wait for either shutdown OR the inner thread to die.
        loop {
            tokio::select! {
                _ = shutdown.wait() => {
                    flag.store(true, Ordering::Relaxed);
                    break;
                }
                _ = tokio::time::sleep(Duration::from_millis(250)) => {
                    if handle.is_finished() {
                        // Inner thread exited without our say-so.
                        // Treat as a failure so the supervisor
                        // restarts under its OnFailure policy.
                        return Err(anyhow::anyhow!(
                            "heartbeat sync worker exited unexpectedly"
                        ));
                    }
                }
            }
        }
        // Best-effort join on shutdown. JoinHandle::join is sync, so
        // we hop onto a blocking task so we don't pin the tokio
        // scheduler waiting on the thread.
        let _ = tokio::task::spawn_blocking(move || handle.join()).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn heartbeat_worker_name_matches_phase_b_lock() {
        let w = HeartbeatWorker::new(PathBuf::from("/tmp/heartbeat-test"), "peer:test".to_owned());
        assert_eq!(w.name(), "heartbeat");
    }

    #[tokio::test]
    async fn new_defaults_to_the_locked_cadence_and_with_interval_overrides() {
        // E1.3 #3 — the default is the 12.3.3 lock; the operator-tuned
        // cadence from /etc/mackesd/mackesd.toml is applied via with_interval.
        let w = HeartbeatWorker::new(PathBuf::from("/tmp/hb"), "peer:test".to_owned());
        assert_eq!(
            w.interval,
            Duration::from_secs(crate::telemetry::HEARTBEAT_INTERVAL_S)
        );
        let tuned = w.with_interval(Duration::from_secs(45));
        assert_eq!(tuned.interval, Duration::from_secs(45));
    }

    #[tokio::test]
    async fn heartbeat_worker_exits_on_shutdown_token() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut w = HeartbeatWorker::new(tmp.path().to_path_buf(), "peer:test".to_owned());
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        // Drive shutdown after a short delay so the worker actually
        // ticks once before exiting.
        let _ = tx.send(true);
        let result = tokio::time::timeout(Duration::from_secs(3), w.run(token))
            .await
            .expect("worker must exit on shutdown");
        assert!(result.is_ok());
    }
}
