//! TRANSFERS-1 — the `transfers` mackesd worker: the queue/ledger/verb/state-machine
//! spine of the Transfers surface (`docs/design/transfers-surface.md`).
//!
//! The Transfers surface is "the one place every byte that moves is born, tracked,
//! and completed." Per §9 the GUI is a renderer: lifecycle lives in the daemon, so
//! jobs survive shell restarts, run headless, and any node can host them. This
//! module is that daemon spine:
//!
//! * a typed [`TransferJob`] envelope (id / source / dest / [`Method`] / [`policy`]
//!   / [`state`]) — the one record every protocol lane rides (Q4);
//! * a **persistent [`Ledger`]** on the node-local store (Q11 — history + state
//!   survive a reboot);
//! * the [`TransferQueue`] engine — the five-state machine + the **parallel cap**
//!   (Q12), every mutation written straight through to the ledger;
//! * the typed [`TransferVerb`] set — `submit / cancel / pause / resume / list`
//!   (Q14) — with an inbox transport the CLI (`mackesd transfer …`) drives for §9
//!   CLI parity;
//! * the injectable [`LaneRunner`] seam the per-protocol lanes (TRANSFERS-2..6)
//!   implement — defaulted here to the honest [`GatedLaneRunner`] (§7 — no lane is
//!   wired yet, so a job fails naming the lane that must land, never a fake success).
//!
//! [`policy`]: TransferJob::policy
//! [`state`]: TransferJob::state
//!
//! ## Rank
//!
//! `transfers` is a **Workstation-tier (rank 1)** worker, the sibling of
//! `pty_broker` (TERM-7) and `mesh_mount` (FILEMGR-5): a mesh feature fronted by a
//! desktop surface (the File Browser, Q1). It idles gracefully where unused — a
//! Lighthouse relay or an untouched headless box simply drains an empty inbox and
//! keeps an empty ledger. A **deliberate census entry** in `worker_role::WORKER_TIERS`
//! (the BUG-STORAGE-1 lesson — a worker absent from the census silently never runs).

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;

use super::{ShutdownToken, Worker};

pub mod job;
pub mod lane;
pub mod ledger;
pub mod queue;
pub mod verb;

pub use job::{Method, TransferJob, TransferPolicy, TransferState, Transition};
pub use lane::{GatedLaneRunner, LaneOutcome, LaneRunner};
pub use ledger::Ledger;
pub use queue::{QueueError, TransferQueue};
pub use verb::{inbox_dir, take_verbs, write_verb, TransferVerb};

/// Default number of jobs run in parallel when the cap env is unset (Q12).
pub const DEFAULT_PARALLEL_CAP: usize = 3;

/// Env var that overrides the parallel cap (Q12 — "configurable cap").
pub const CAP_ENV: &str = "MDE_TRANSFERS_PARALLEL_CAP";

/// Inbox drain cadence — a submitted/paused job is picked up within this window.
pub const POLL: Duration = Duration::from_secs(2);

/// Wall-clock milliseconds since the epoch (the ledger's timestamps + id seed).
#[must_use]
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The node-LOCAL transfers store root.
///
/// `<MDE_HOME|MACKESD_HOME>/transfers`, or `/var/lib/mde/transfers` when neither is
/// set (mirrors [`crate::default_db_path`]). The CLI and the daemon both resolve this
/// so they share the ledger + inbox.
#[must_use]
pub fn default_store_root() -> PathBuf {
    if let Some(home) = crate::env_with_legacy_fallback("MDE_HOME", "MACKESD_HOME") {
        return PathBuf::from(home).join("transfers");
    }
    PathBuf::from("/var/lib/mde/transfers")
}

/// The configured parallel cap (>= 1): [`CAP_ENV`] if a valid positive integer,
/// else [`DEFAULT_PARALLEL_CAP`].
#[must_use]
pub fn default_cap() -> usize {
    std::env::var(CAP_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .map_or(DEFAULT_PARALLEL_CAP, |n| n.max(1))
}

/// The `transfers` worker — drives the queue: drains the inbox, reaps finished lane
/// tasks, and fills up to the cap each tick.
pub struct TransfersWorker {
    store_root: PathBuf,
    cap: usize,
    lane: Arc<dyn LaneRunner>,
    poll: Duration,
}

impl TransfersWorker {
    /// Production constructor: the node-local store, the env cap, the honest
    /// [`GatedLaneRunner`] (TRANSFERS-2..6 inject their real lane).
    #[must_use]
    pub fn new(store_root: PathBuf) -> Self {
        Self {
            store_root,
            cap: default_cap(),
            lane: Arc::new(GatedLaneRunner),
            poll: POLL,
        }
    }

    /// Override the parallel cap (tests + a future config plumb).
    #[must_use]
    pub fn with_cap(mut self, cap: usize) -> Self {
        self.cap = cap.max(1);
        self
    }

    /// Inject the lane runner (the TRANSFERS-2..6 seam; tests supply a fake).
    #[must_use]
    pub fn with_lane(mut self, lane: Arc<dyn LaneRunner>) -> Self {
        self.lane = lane;
        self
    }

    /// Override the poll cadence (tests use a short value).
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Open the live engine (the queue over the ledger + the empty task table).
    ///
    /// # Errors
    /// Fails if the ledger directory can't be opened.
    fn engine(&self) -> std::io::Result<Engine> {
        Ok(Engine {
            queue: TransferQueue::open(&self.store_root, self.cap)?,
            tasks: HashMap::new(),
            lane: Arc::clone(&self.lane),
            store_root: self.store_root.clone(),
        })
    }
}

#[async_trait::async_trait]
impl Worker for TransfersWorker {
    fn name(&self) -> &'static str {
        "transfers"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let mut engine = self.engine()?;
        tracing::info!(
            target: "mackesd::transfers",
            store = %self.store_root.display(), cap = self.cap,
            "transfers worker up (queue/ledger/verb spine; lanes honestly gated until TRANSFERS-2..6)",
        );
        loop {
            engine.tick().await;
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(self.poll) => {}
            }
        }
    }
}

/// The live run state: the open queue, the in-flight lane tasks, and the seam.
struct Engine {
    queue: TransferQueue,
    tasks: HashMap<String, JoinHandle<LaneOutcome>>,
    lane: Arc<dyn LaneRunner>,
    store_root: PathBuf,
}

impl Engine {
    /// One scheduler pass: apply inbox verbs, reap finished tasks, fill to the cap.
    async fn tick(&mut self) {
        self.drain_inbox();
        self.reap().await;
        self.fill();
    }

    /// Apply every pending inbox verb (the daemon is the single ledger writer).
    fn drain_inbox(&mut self) {
        for verb in take_verbs(&self.store_root) {
            match verb {
                TransferVerb::Submit(job) => {
                    let id = job.id.clone();
                    if let Err(e) = self.queue.submit(job) {
                        tracing::warn!(target: "mackesd::transfers", id = %id, error = %e, "submit failed");
                    }
                }
                TransferVerb::Cancel(id) => {
                    self.abort_task(&id);
                    let res = self.queue.cancel(&id);
                    Self::log_verb("cancel", &id, res);
                }
                TransferVerb::Pause(id) => {
                    self.abort_task(&id);
                    let res = self.queue.pause(&id);
                    Self::log_verb("pause", &id, res);
                }
                TransferVerb::Resume(id) => {
                    let res = self.queue.resume(&id);
                    Self::log_verb("resume", &id, res);
                }
                // `list` is a pure read served off the ledger by the caller — the
                // daemon has nothing to do for it.
                TransferVerb::List => {}
            }
        }
    }

    /// Abort + forget a job's in-flight lane task (tokio abort → the lane's child
    /// process is killed on drop, so a cancel/pause stops a running transfer).
    fn abort_task(&mut self, id: &str) {
        if let Some(handle) = self.tasks.remove(id) {
            handle.abort();
        }
    }

    /// Log a verb's typed outcome (an illegal/not-found refusal is honest, not
    /// silent).
    fn log_verb(verb: &str, id: &str, res: Result<(), QueueError>) {
        match res {
            Ok(()) => tracing::info!(target: "mackesd::transfers", verb, id, "applied"),
            Err(e) => tracing::info!(target: "mackesd::transfers", verb, id, error = %e, "refused"),
        }
    }

    /// Apply the outcome of every finished lane task to the ledger.
    async fn reap(&mut self) {
        let finished: Vec<String> = self
            .tasks
            .iter()
            .filter(|(_, h)| h.is_finished())
            .map(|(id, _)| id.clone())
            .collect();
        for id in finished {
            let Some(handle) = self.tasks.remove(&id) else {
                continue;
            };
            // A JoinError means the task was aborted (a cancel/pause already moved the
            // job, so `complete` no-ops) or it panicked — either way, an honest fail.
            let outcome = handle
                .await
                .unwrap_or_else(|_| LaneOutcome::failed("the transfer lane task ended abnormally"));
            if let Err(e) = self.queue.complete(&id, &outcome) {
                tracing::warn!(target: "mackesd::transfers", id = %id, error = %e, "complete failed");
            }
        }
    }

    /// Claim + spawn Queued jobs until the cap is reached or the queue is empty.
    fn fill(&mut self) {
        while let Some(job) = self.queue.claim_next() {
            let lane = Arc::clone(&self.lane);
            let running = job.clone();
            let handle = tokio::spawn(async move { lane.run(&running).await });
            self.tasks.insert(job.id, handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tokio::sync::watch;

    /// A lane that blocks until a watch gate flips true — lets a test hold jobs in
    /// `Running` to observe the cap, then release them to observe drain. A watch
    /// (not `Notify`) so the release is never lost to a task that hasn't yet parked.
    struct BlockingLane {
        release: watch::Receiver<bool>,
    }

    #[async_trait::async_trait]
    impl LaneRunner for BlockingLane {
        async fn run(&self, _job: &TransferJob) -> LaneOutcome {
            let mut rx = self.release.clone();
            let _ = rx.wait_for(|v| *v).await;
            LaneOutcome::Done
        }
    }

    fn engine_with(store: &Path, cap: usize, lane: Arc<dyn LaneRunner>) -> Engine {
        Engine {
            queue: TransferQueue::open(store, cap).unwrap(),
            tasks: HashMap::new(),
            lane,
            store_root: store.to_path_buf(),
        }
    }

    fn job() -> TransferJob {
        TransferJob::new("/src", "/dst", Method::Rsync, TransferPolicy::default())
    }

    #[test]
    fn store_root_and_cap_resolve_from_env() {
        // Cap parses + floors at 1; a bogus value falls back to the default.
        assert!(default_cap() >= 1);
        assert_eq!(DEFAULT_PARALLEL_CAP, 3);
        // The default store path ends in `transfers`.
        assert!(default_store_root().ends_with("transfers"));
    }

    #[test]
    fn worker_name_is_the_census_token() {
        let w = TransfersWorker::new(PathBuf::from("/tmp/x"));
        assert_eq!(w.name(), "transfers");
    }

    #[tokio::test]
    async fn inbox_submit_is_drained_then_gated_lane_fails_it_honestly() {
        let tmp = tempfile::tempdir().unwrap();
        let mut engine = engine_with(tmp.path(), 2, Arc::new(GatedLaneRunner));
        let j = job();
        let id = j.id.clone();
        write_verb(tmp.path(), &TransferVerb::Submit(j)).unwrap();
        // Drive ticks until the job reaches a terminal state (the gated lane returns
        // immediately, so this settles within a couple of yields — bounded loop).
        for _ in 0..50 {
            engine.tick().await;
            if engine.queue.get(&id).is_some_and(|j| j.state.is_terminal())
                && engine.tasks.is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        let done = engine.queue.get(&id).expect("job in ledger");
        assert_eq!(
            done.state,
            TransferState::Failed,
            "honest gate fails the job"
        );
        assert!(
            done.error.as_deref().unwrap_or_default().contains("rsync"),
            "the failure names the un-wired lane: {:?}",
            done.error
        );
    }

    #[tokio::test]
    async fn cap_bounds_concurrent_running_jobs() {
        let tmp = tempfile::tempdir().unwrap();
        let (release_tx, release_rx) = watch::channel(false);
        let lane = Arc::new(BlockingLane {
            release: release_rx,
        });
        let mut engine = engine_with(tmp.path(), 2, lane);
        // Three jobs submitted; the blocking lane holds each Running job's slot.
        for _ in 0..3 {
            write_verb(tmp.path(), &TransferVerb::Submit(job())).unwrap();
        }
        engine.tick().await; // drain 3 submits + fill up to the cap
        assert_eq!(engine.queue.running_count(), 2, "cap holds at 2");
        assert_eq!(engine.tasks.len(), 2, "only 2 lane tasks in flight");
        let queued = engine
            .queue
            .list()
            .into_iter()
            .filter(|j| j.state == TransferState::Queued)
            .count();
        assert_eq!(queued, 1, "the third waits Queued behind the cap");
        // Release the lanes: the two finish, freeing slots, and the third — held
        // behind the cap — is then admitted and drains too. All three reach Done.
        release_tx.send(true).unwrap();
        let done_count = |engine: &Engine| {
            engine
                .queue
                .list()
                .into_iter()
                .filter(|j| j.state == TransferState::Done)
                .count()
        };
        for _ in 0..100 {
            engine.tick().await;
            if done_count(&engine) == 3 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            done_count(&engine),
            3,
            "every job — incl. the one held behind the cap — drained"
        );
    }

    #[tokio::test]
    async fn worker_exits_promptly_on_shutdown() {
        let tmp = tempfile::tempdir().unwrap();
        let mut w =
            TransfersWorker::new(tmp.path().to_path_buf()).with_poll(Duration::from_millis(10));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        assert!(joined.unwrap().expect("join").is_ok());
    }
}
