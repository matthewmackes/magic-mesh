//! mackesd-06 — the reconcile worker under the async supervisor.
//!
//! The reconcile loop (`crate::worker::run_loop`, started by
//! [`crate::worker::spawn_reconcile_worker`]) historically ran on a RAW
//! `std::thread` OUTSIDE the [`super::Supervisor`]: if a tick panicked,
//! the thread died and reconcile was silently dead for the daemon's
//! whole lifetime — no restart, no circuit breaker, no census update.
//! This adapter brings it under the supervisor so it gets the same
//! restart-on-panic + back-off + breaker treatment as every other
//! worker.
//!
//! The threading model is PRESERVED: the actual blocking tick (sync
//! `rusqlite` SQL + FS reads) still runs on the dedicated `std::thread`
//! that `spawn_reconcile_worker` starts — it never runs on the tokio
//! scheduler, so it can't stall the runtime's timers. This adapter only
//! (a) bridges the supervisor's async [`ShutdownToken`] into the sync
//! `AtomicBool` the loop polls, and (b) surfaces an unexpected
//! inner-thread exit (i.e. a panic — the loop otherwise never returns)
//! as an `Err` so the supervisor restarts it. The cadence
//! ([`crate::worker::RECONCILE_INTERVAL_S`]) and the tick work are
//! untouched. Mirrors [`super::heartbeat::HeartbeatWorker`].

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::{ShutdownToken, Worker};

/// Async worker wrapping [`crate::worker::spawn_reconcile_worker`].
///
/// Re-derives the inner thread on every supervisor restart, so a
/// transient cause that killed the previous tick (a corrupt store, a
/// briefly-unmounted QNM-Shared root) starts from a clean slate.
pub struct ReconcileWorker {
    workgroup_root: PathBuf,
    node_id: String,
    db_path: PathBuf,
}

impl ReconcileWorker {
    /// Construct a reconcile worker pinned to the given QNM-Shared root,
    /// stable node id, and local SQL store path. Runs at the locked
    /// [`crate::worker::RECONCILE_INTERVAL_S`] cadence.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String, db_path: PathBuf) -> Self {
        Self {
            workgroup_root,
            node_id,
            db_path,
        }
    }
}

#[async_trait::async_trait]
impl Worker for ReconcileWorker {
    fn name(&self) -> &'static str {
        "reconcile"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // Reuse the existing sync spawn_reconcile_worker — it owns the
        // real reconcile cadence + tick semantics on its own dedicated
        // OS thread (so the blocking rusqlite/FS work never pins the
        // tokio scheduler). We just bridge the async ShutdownToken into
        // the sync AtomicBool the loop polls.
        let flag = Arc::new(AtomicBool::new(false));
        let handle = crate::worker::spawn_reconcile_worker(
            self.workgroup_root.clone(),
            self.node_id.clone(),
            self.db_path.clone(),
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
                        // The inner thread exited without a shutdown
                        // request — its run_loop caught + logged every
                        // ordinary error and otherwise never returns, so
                        // an exit here means it PANICKED. Surface it as
                        // an Err so the supervisor restarts under its
                        // policy instead of reconcile silently dying for
                        // the daemon's lifetime (the mackesd-06 fix).
                        return Err(anyhow::anyhow!(
                            "reconcile sync worker exited unexpectedly"
                        ));
                    }
                }
            }
        }
        // Best-effort join on shutdown. JoinHandle::join is sync, so hop
        // onto a blocking task so we don't pin the tokio scheduler
        // waiting on the thread to finish its current tick.
        let _ = tokio::task::spawn_blocking(move || handle.join()).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::{new_status_map, workers_ready, RestartPolicy, Spawn, Supervisor};

    #[tokio::test]
    async fn reconcile_worker_name_is_reconcile() {
        // The census/status name must stay "reconcile" so it lines up
        // with the worker_names roster the daemon already pushed.
        let w = ReconcileWorker::new(
            PathBuf::from("/tmp/reconcile-test"),
            "peer:test".to_owned(),
            PathBuf::from("/tmp/reconcile-test/mackesd.db"),
        );
        assert_eq!(w.name(), "reconcile");
    }

    #[tokio::test]
    async fn reconcile_worker_exits_on_shutdown_token() {
        // Bridging the async ShutdownToken → the sync AtomicBool must
        // stop the inner thread and let run() return Ok promptly.
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut w = ReconcileWorker::new(
            tmp.path().to_path_buf(),
            "peer:test".to_owned(),
            tmp.path().join("mackesd.db"),
        );
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let _ = tx.send(true);
        let result = tokio::time::timeout(Duration::from_secs(5), w.run(token))
            .await
            .expect("worker must exit on shutdown");
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn reconcile_worker_is_registered_with_the_supervisor() {
        // mackesd-06 — the whole point: reconcile is now a SUPERVISED
        // worker, not a detached std::thread. Proof: after spawning it
        // into a Supervisor with a status map, a live "reconcile" row
        // appears in the census (so it shares the restart + breaker
        // path of every sibling), and it drains cleanly on shutdown.
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut sup = Supervisor::new();
        let status = new_status_map();
        sup.set_status_map(Arc::clone(&status));
        sup.spawn(Spawn::new(
            ReconcileWorker::new(
                tmp.path().to_path_buf(),
                "peer:test".to_owned(),
                tmp.path().join("mackesd.db"),
            ),
            RestartPolicy::OnFailure,
        ));
        // Give the spawn a beat to register in the status map.
        tokio::time::sleep(Duration::from_millis(50)).await;
        {
            let g = status.lock().unwrap();
            let row = g
                .get("reconcile")
                .expect("reconcile registered with the supervisor");
            assert!(row.alive, "reconcile worker is alive under the supervisor");
            assert!(!row.breaker_tripped);
        }
        let (alive, total, tripped) = workers_ready(&status);
        assert_eq!((alive, total, tripped), (1, 1, 0));
        sup.shutdown_and_join().await.unwrap();
        let (alive, _, _) = workers_ready(&status);
        assert_eq!(alive, 0, "exit recorded after supervisor shutdown");
    }
}
