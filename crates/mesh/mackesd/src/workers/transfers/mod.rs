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
//!   implement — defaulted here to [`TransferLaneRunner`], which wires the
//!   TRANSFERS-2 HTTP lane and keeps the remaining lanes honestly gated (§7).
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

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use serde::Serialize;
use tokio::task::JoinHandle;

use super::{ShutdownToken, Worker};

pub mod destination;
pub mod job;
pub mod lane;
pub mod ledger;
pub mod queue;
pub mod sync_pair;
pub mod verb;

pub use destination::{
    destinations_from_state, discover_destinations, is_ad_hoc_endpoint, DestinationKind,
    TransferDestination,
};
pub use job::{Method, TransferJob, TransferPolicy, TransferState, Transition};
pub use lane::{
    GatedLaneRunner, HttpWgetLane, LaneOutcome, LaneRunner, MusicLibraryLane, NodeLane,
    ProgressSink, RsyncLane, TransferLaneRunner,
};
pub use ledger::Ledger;
pub use queue::{QueueError, TransferQueue};
pub use sync_pair::{SyncPair, SyncPairStore};
pub use verb::{inbox_dir, take_verbs, write_verb, TransferVerb};

/// Default number of jobs run in parallel when the cap env is unset (Q12).
pub const DEFAULT_PARALLEL_CAP: usize = 3;

/// Env var that overrides the parallel cap (Q12 — "configurable cap").
pub const CAP_ENV: &str = "MDE_TRANSFERS_PARALLEL_CAP";

/// Inbox drain cadence — a submitted/paused job is picked up within this window.
pub const POLL: Duration = Duration::from_secs(2);
/// The existing Chat worker folds `event/notify/*`; transfer terminal events use
/// this source lane instead of creating a new notification surface.
pub const TRANSFER_NOTIFY_TOPIC: &str = "event/notify/transfers";

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
    bus_root: Option<PathBuf>,
    cap: usize,
    lane: Arc<dyn LaneRunner>,
    poll: Duration,
}

impl TransfersWorker {
    /// Production constructor: the node-local store, the env cap, and the method
    /// dispatcher (HTTP wired; future lanes still honestly gated).
    #[must_use]
    pub fn new(store_root: PathBuf) -> Self {
        Self {
            store_root,
            bus_root: mde_bus::default_data_dir()
                .or_else(|| Some(PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))),
            cap: default_cap(),
            lane: Arc::new(TransferLaneRunner),
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

    /// Override the Bus root used for terminal notification tests.
    #[must_use]
    pub fn with_bus_root(mut self, bus_root: Option<PathBuf>) -> Self {
        self.bus_root = bus_root;
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
            sync_pairs: SyncPairStore::open(&self.store_root)?,
            tasks: HashMap::new(),
            lane: Arc::clone(&self.lane),
            store_root: self.store_root.clone(),
            notify: self.bus_root.clone().map(TransferNotifier::new),
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
            "transfers worker up (queue/ledger/verb spine; http lane wired, remaining lanes honestly gated)",
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
    sync_pairs: SyncPairStore,
    tasks: HashMap<String, JoinHandle<LaneOutcome>>,
    lane: Arc<dyn LaneRunner>,
    store_root: PathBuf,
    notify: Option<TransferNotifier>,
}

impl Engine {
    /// One scheduler pass: apply inbox verbs, reap finished tasks, fill to the cap.
    async fn tick(&mut self) {
        self.drain_inbox();
        self.schedule_sync_pairs_at(now_ms());
        self.reap().await;
        self.fill();
    }

    /// Fire every due saved sync pair by enqueueing a normal rsync job.
    fn schedule_sync_pairs_at(&mut self, now: u64) {
        for pair in self.sync_pairs.load_all() {
            if !pair.due_at(now) {
                continue;
            }
            let id = pair.id.clone();
            let job = pair.to_job();
            let job_id = job.id.clone();
            match self.queue.submit(job) {
                Ok(_) => {
                    if let Err(e) = self.sync_pairs.mark_fired(&id, now) {
                        tracing::warn!(
                            target: "mackesd::transfers",
                            pair = %id, job = %job_id, error = %e,
                            "sync pair fired but last_fired stamp failed"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        target: "mackesd::transfers",
                        pair = %id, error = %e,
                        "sync pair enqueue failed"
                    );
                }
            }
        }
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
                TransferVerb::SaveSyncPair(pair) => {
                    let id = pair.id.clone();
                    if let Err(e) = self.sync_pairs.upsert(&pair) {
                        tracing::warn!(target: "mackesd::transfers", pair = %id, error = %e, "save sync pair failed");
                    }
                }
                TransferVerb::RemoveSyncPair(id) => {
                    if let Err(e) = self.sync_pairs.remove(&id) {
                        tracing::warn!(target: "mackesd::transfers", pair = %id, error = %e, "remove sync pair failed");
                    }
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
        let mut terminal = Vec::new();
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
            } else if let Some(job) = self.queue.get(&id) {
                if job.state.is_terminal() {
                    terminal.push(job);
                }
            }
        }
        if let (Some(notify), false) = (&self.notify, terminal.is_empty()) {
            notify.emit_terminal_batch(&terminal);
        }
    }

    /// Claim + spawn Queued jobs until the cap is reached or the queue is empty.
    fn fill(&mut self) {
        while let Some(job) = self.queue.claim_next() {
            let lane = Arc::clone(&self.lane);
            let queue = self.queue.clone();
            let progress_id = job.id.clone();
            let progress = ProgressSink::new(move |pct| {
                if let Err(e) = queue.set_progress(&progress_id, pct) {
                    tracing::warn!(
                        target: "mackesd::transfers",
                        id = %progress_id, error = %e,
                        "progress update failed"
                    );
                }
            });
            let running = job.clone();
            let handle = tokio::spawn(async move { lane.run(&running, progress).await });
            self.tasks.insert(job.id, handle);
        }
    }
}

#[derive(Clone)]
struct TransferNotifier {
    bus_root: PathBuf,
}

impl TransferNotifier {
    fn new(bus_root: PathBuf) -> Self {
        Self { bus_root }
    }

    fn emit_terminal_batch(&self, jobs: &[TransferJob]) {
        if let [job] = jobs {
            self.emit_terminal(job);
            return;
        }
        let done = jobs
            .iter()
            .filter(|j| j.state == TransferState::Done)
            .count();
        let failed = jobs
            .iter()
            .filter(|j| j.state == TransferState::Failed)
            .count();
        let severity = if failed > 0 { "warning" } else { "info" };
        let summary = match (done, failed) {
            (done, 0) => format!("{done} transfers completed"),
            (0, failed) => format!("{failed} transfers failed"),
            (done, failed) => format!("{done} transfers completed, {failed} failed"),
        };
        self.emit_body(severity, summary, None, None, None);
    }

    fn emit_terminal(&self, job: &TransferJob) {
        let (severity, summary) = match job.state {
            TransferState::Done => (
                "info",
                format!("transfer {} completed ({})", short_id(&job.id), job.method),
            ),
            TransferState::Failed => (
                "warning",
                format!(
                    "transfer {} failed ({}){}",
                    short_id(&job.id),
                    job.method,
                    job.error
                        .as_deref()
                        .filter(|e| !e.is_empty())
                        .map_or_else(String::new, |e| format!(": {e}"))
                ),
            ),
            _ => return,
        };
        self.emit_body(
            severity,
            summary,
            Some(&job.id),
            Some(job.state.as_str()),
            Some(job.method.as_str()),
        );
    }

    fn emit_body(
        &self,
        severity: &str,
        summary: String,
        transfer_id: Option<&str>,
        transfer_state: Option<&str>,
        method: Option<&str>,
    ) {
        let body = TransferNotifyBody {
            severity,
            source: "transfers",
            summary,
            host: std::env::var("HOSTNAME").unwrap_or_else(|_| "local".into()),
            ts_unix_ms: now_ms() as i64,
            transfer_id,
            transfer_state,
            method,
        };
        let Ok(json) = serde_json::to_string(&body) else {
            return;
        };
        match Persist::open(self.bus_root.clone()) {
            Ok(persist) => {
                if let Err(e) =
                    persist.write(TRANSFER_NOTIFY_TOPIC, Priority::Default, None, Some(&json))
                {
                    tracing::debug!(target: "mackesd::transfers", error = %e, "transfer notify publish failed");
                }
            }
            Err(e) => {
                tracing::debug!(target: "mackesd::transfers", error = %e, "transfer notify persist open failed");
            }
        }
    }
}

#[derive(Serialize)]
struct TransferNotifyBody<'a> {
    severity: &'a str,
    source: &'a str,
    summary: String,
    host: String,
    ts_unix_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    transfer_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transfer_state: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    method: Option<&'a str>,
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
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
        async fn run(&self, _job: &TransferJob, _progress: ProgressSink) -> LaneOutcome {
            let mut rx = self.release.clone();
            let _ = rx.wait_for(|v| *v).await;
            LaneOutcome::Done
        }
    }

    struct ImmediateLane {
        outcome: LaneOutcome,
    }

    #[async_trait::async_trait]
    impl LaneRunner for ImmediateLane {
        async fn run(&self, _job: &TransferJob, _progress: ProgressSink) -> LaneOutcome {
            self.outcome.clone()
        }
    }

    fn engine_with(store: &Path, cap: usize, lane: Arc<dyn LaneRunner>) -> Engine {
        Engine {
            queue: TransferQueue::open(store, cap).unwrap(),
            sync_pairs: SyncPairStore::open(store).unwrap(),
            tasks: HashMap::new(),
            lane,
            store_root: store.to_path_buf(),
            notify: None,
        }
    }

    fn engine_with_notify(
        store: &Path,
        bus: &Path,
        cap: usize,
        lane: Arc<dyn LaneRunner>,
    ) -> Engine {
        Engine {
            queue: TransferQueue::open(store, cap).unwrap(),
            sync_pairs: SyncPairStore::open(store).unwrap(),
            tasks: HashMap::new(),
            lane,
            store_root: store.to_path_buf(),
            notify: Some(TransferNotifier::new(bus.to_path_buf())),
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
    async fn browser_download_and_scrape_outputs_land_as_ledger_jobs() {
        let tmp = tempfile::tempdir().unwrap();
        let source_dir = tmp.path().join("browser-out");
        let dest_dir = tmp.path().join("picked-destination");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::create_dir_all(&dest_dir).unwrap();
        let files = [
            ("download.bin", b"browser download".as_slice()),
            (
                "scrape-1.json",
                br#"{"url":"https://example.invalid/"}"#.as_slice(),
            ),
            ("scrape-2.md", b"# scraped page\n".as_slice()),
        ];
        let mut ids = Vec::new();
        for (name, body) in files {
            let source = source_dir.join(name);
            std::fs::write(&source, body).unwrap();
            let job = TransferJob::new(
                source.display().to_string(),
                dest_dir.display().to_string(),
                Method::BrowserDownload,
                TransferPolicy {
                    bwlimit: None,
                    verify: true,
                },
            );
            ids.push(job.id.clone());
            write_verb(tmp.path(), &TransferVerb::Submit(job)).unwrap();
        }

        let mut engine = engine_with(tmp.path(), 3, Arc::new(TransferLaneRunner));
        for _ in 0..100 {
            engine.tick().await;
            if engine.tasks.is_empty()
                && ids.iter().all(|id| {
                    engine
                        .queue
                        .get(id)
                        .is_some_and(|j| j.state == TransferState::Done)
                })
            {
                break;
            }
            tokio::task::yield_now().await;
        }

        for id in &ids {
            let job = engine.queue.get(id).expect("browser job in ledger");
            assert_eq!(job.method, Method::BrowserDownload);
            assert_eq!(job.state, TransferState::Done);
            assert_eq!(job.progress, Some(100));
            assert!(
                matches!(
                    job.integrity,
                    Some(crate::workers::transfers::job::IntegrityStatus::Verified { .. })
                ),
                "browser output job should carry verified integrity: {job:?}"
            );
        }
        for (name, body) in files {
            assert_eq!(std::fs::read(dest_dir.join(name)).unwrap(), body);
        }
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

    #[tokio::test]
    async fn failed_transfer_emits_one_notify_alert() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tempfile::tempdir().unwrap();
        let lane = Arc::new(ImmediateLane {
            outcome: LaneOutcome::failed("fixture failure"),
        });
        let mut engine = engine_with_notify(tmp.path(), bus.path(), 1, lane);
        write_verb(tmp.path(), &TransferVerb::Submit(job())).unwrap();
        for _ in 0..20 {
            engine.tick().await;
            if engine.tasks.is_empty()
                && engine
                    .queue
                    .list()
                    .iter()
                    .any(|j| j.state == TransferState::Failed)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        let persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let msgs = persist.list_since(TRANSFER_NOTIFY_TOPIC, None).unwrap();
        assert_eq!(msgs.len(), 1);
        let body: serde_json::Value =
            serde_json::from_str(msgs[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["source"], "transfers");
        assert_eq!(body["severity"], "warning");
        assert!(body["summary"]
            .as_str()
            .unwrap()
            .contains("fixture failure"));
        assert_eq!(body["transfer_state"], "failed");
    }

    #[tokio::test]
    async fn same_tick_terminal_batch_emits_one_coalesced_notify_alert() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tempfile::tempdir().unwrap();
        let lane = Arc::new(ImmediateLane {
            outcome: LaneOutcome::Done,
        });
        let mut engine = engine_with_notify(tmp.path(), bus.path(), 3, lane);
        for _ in 0..3 {
            write_verb(tmp.path(), &TransferVerb::Submit(job())).unwrap();
        }
        for _ in 0..20 {
            engine.tick().await;
            if engine.tasks.is_empty()
                && engine
                    .queue
                    .list()
                    .iter()
                    .filter(|j| j.state == TransferState::Done)
                    .count()
                    == 3
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        let persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let msgs = persist.list_since(TRANSFER_NOTIFY_TOPIC, None).unwrap();
        assert_eq!(msgs.len(), 1, "batch coalesces to one notification");
        let body: serde_json::Value =
            serde_json::from_str(msgs[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["severity"], "info");
        assert_eq!(body["summary"], "3 transfers completed");
        assert!(body.get("transfer_id").is_none());
    }

    #[tokio::test]
    async fn due_sync_pair_enqueues_once_then_waits_for_interval() {
        let tmp = tempfile::tempdir().unwrap();
        let mut engine = engine_with(
            tmp.path(),
            1,
            Arc::new(ImmediateLane {
                outcome: LaneOutcome::Done,
            }),
        );
        engine
            .sync_pairs
            .upsert(&SyncPair::new(
                "docs",
                "/src",
                "/dst",
                15,
                TransferPolicy::default(),
            ))
            .unwrap();

        engine.schedule_sync_pairs_at(1_000);
        let first = engine.queue.list();
        assert_eq!(first.len(), 1, "initially due pair fires once");
        assert_eq!(first[0].method, Method::Rsync);
        assert_eq!(first[0].source, "/src");
        assert_eq!(
            engine.sync_pairs.get("docs").unwrap().last_fired_ms,
            Some(1_000)
        );

        engine.schedule_sync_pairs_at(15_999);
        assert_eq!(
            engine.queue.list().len(),
            1,
            "pair does not duplicate before the interval elapses"
        );
        engine.schedule_sync_pairs_at(16_000);
        assert_eq!(
            engine.queue.list().len(),
            2,
            "pair fires again exactly at the next due time"
        );
    }

    #[tokio::test]
    async fn recurring_rsync_pair_fires_and_mirrors_on_tick() {
        if std::process::Command::new("rsync")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping recurring rsync fixture: rsync is not installed");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let source_dir = tmp.path().join("source");
        let dest_dir = tmp.path().join("dest");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join("file.txt"), b"first version").unwrap();

        let mut engine = engine_with(tmp.path().join("store").as_path(), 1, Arc::new(RsyncLane));
        engine
            .sync_pairs
            .upsert(&SyncPair::new(
                "mirror",
                format!("{}/", source_dir.display()),
                format!("{}/", dest_dir.display()),
                5,
                TransferPolicy::default(),
            ))
            .unwrap();

        for _ in 0..1_000 {
            engine.tick().await;
            if engine.tasks.is_empty()
                && engine
                    .queue
                    .list()
                    .iter()
                    .any(|j| j.state == TransferState::Done)
                && std::fs::read(dest_dir.join("file.txt"))
                    .is_ok_and(|bytes| bytes.as_slice() == b"first version")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            std::fs::read(dest_dir.join("file.txt")).unwrap(),
            b"first version"
        );
        assert!(
            engine
                .queue
                .list()
                .iter()
                .any(|j| j.state == TransferState::Done),
            "recurring rsync job should complete successfully: {:?}",
            engine.queue.list()
        );
        assert_eq!(
            engine
                .sync_pairs
                .get("mirror")
                .unwrap()
                .last_fired_ms
                .is_some(),
            true
        );
    }
}
