//! WL-FUNC-011 Phase 2 — the mackesd `collab` worker: the live spine that makes
//! [`mde_collab_core`] real on the mesh.
//!
//! [`mde_collab_core::CollabEngine`] is the headless core (validate command →
//! sign events → SQLite projection → convergent merge); this worker is the I/O
//! loop that drives it, the same shape the [`super::chat`] worker has over the
//! chat contracts (which it will EVENTUALLY replace — Phase 4; for now it runs
//! ALONGSIDE chat). It owns five jobs, all folding into one per-actor
//! [`CollabEngine`]:
//!
//!   1. **Drain commands (lock 1/10).** Subscribes every `action/collab/<verb>`
//!      lane ([`topics::command_topic`]), decodes a [`CollabCommand`], and runs
//!      [`CollabEngine::apply`] with this node's Ed25519 identity — the same key
//!      + pattern the chat worker signs with. A denied command returns a typed
//!      [`mde_collab_core::CollabError`] that is LOGGED (visible), never a silent
//!      no-op.
//!   2. **Persist + project.** On success the engine returns signed events; each
//!      is appended to this node's own per-space [`FileActorLog`] (the
//!      Syncthing-replicable unit, under the MDE data root) BEFORE it is relayed,
//!      and projected into the SQLite read models (the projection folds inside
//!      `apply`). The durable log is the source of truth: a restart rebuilds the
//!      projection by [`CollabEngine::merge`]-ing every replicated log back in.
//!   3. **Publish.** Each live signed event is published on
//!      `collab/event/<space>/<actor>` ([`topics::event_topic`]) and the affected
//!      `state/collab/*` read models are republished ([`topics::state_topic`] /
//!      [`topics::space_state_topic`]) so the surface + other nodes see the change
//!      — the chat-worker publish + latest-wins dedup cadence.
//!   4. **Ingest + converge.** Consumes incoming `collab/event/*` from OTHER
//!      actors (bus live fast-path) AND backfills from replicated actor logs
//!      (Syncthing durable-path) → [`CollabEngine::merge`] (signature-checked, so
//!      a forged event is DROPPED; idempotent + order-independent, so replays and
//!      out-of-order delivery converge). A reconnecting node backfills its logs on
//!      boot and converges.
//!   5. **Universal (rank 0).** Runs on EVERY node incl. a headless Lighthouse,
//!      exactly like the chat worker it parallels.
//!
//! **Testability.** The two seams — the Bus root and the actor-log root — are
//! both injectable to a tempdir, and every publish is an in-process
//! [`Persist::write`], so the whole drain → apply → project → publish → ingest →
//! converge flow drives headless with no live mesh. Live multi-node delivery +
//! real Syncthing backfill are integration-gated; the worker logic, the fold, and
//! the convergence are what land here with unit tests.

#![cfg(feature = "async-services")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use mde_bus::hooks::config::Priority;
use mde_bus::persist::{Persist, StoredMessage};
use mde_collab_core::{ActorLog, CollabEngine, Ed25519Signer, FileActorLog, Projection, RandomIds};
use mde_collab_types::topics::{self, projection as proj};
use mde_collab_types::{ActorId, CollabCommand, CollabEventEnvelope, SpaceId};

use super::{ShutdownToken, Worker};

/// The default poll cadence (tests override with a short value; the loop is
/// entirely edge-driven off the Bus so the interval only bounds latency).
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Every [`CollabCommand`] verb, as the fixed `action/collab/<verb>` lane set the
/// worker drains. Fixed (not discovered via `list_topics`) so each lane's drain
/// cursor is seeded to head on the first tick and stays strictly forward-only — a
/// restart never replays a stale command backlog as a re-send. Must stay in step
/// with [`CollabCommand::verb`]; `command_verbs_cover_every_variant` pins it.
const COMMAND_VERBS: &[&str] = &[
    "create_space",
    "rename_space",
    "delete_space",
    "add_member",
    "remove_member",
    "set_member_role",
    "join_space",
    "leave_space",
    "set_presence",
    "send_message",
    "edit_message",
    "delete_message",
    "start_thread",
    "reply_in_thread",
    "ack_alert",
    "snooze_alert",
    "run_alert_action",
    "set_alert_mute",
    "set_severity_threshold",
    "set_do_not_disturb",
    "publish_clipboard",
    "attach_clipboard",
    "pin_clipboard",
    "unpin_clipboard",
    "delete_clipboard",
    "clear_clipboard",
    "create_document",
    "update_document",
    "request_review",
    "submit_review",
    "link_file",
    "unlink_file",
    "start_transfer",
    "control_transfer",
    "start_call",
    "answer_call",
    "decline_call",
    "hang_up_call",
    "send_dtmf",
    "set_call_muted",
    "request_ai_suggestion",
];

/// The universal, rank-0 collaboration worker for one node.
pub struct CollabWorker {
    /// This node's collaboration identity (the bare hostname — the same identity
    /// the chat worker uses as its roster/DM key).
    self_actor: ActorId,
    /// This node's persisted Ed25519 signing key ([`crate::node_key`]); every
    /// authored event is signed with it.
    signing_key: SigningKey,
    /// The Syncthing-replicable actor-log root (`<space>/<actor>.jsonl` beneath).
    log_root: PathBuf,
    /// Poll cadence.
    poll_interval: Duration,
    /// Bus root override (tests point it at a tempdir Persist).
    bus_root_override: Option<PathBuf>,
}

impl CollabWorker {
    /// Construct with production defaults. `self_host` is this node's bare
    /// hostname (the collaboration actor identity), `signing_key` its persisted
    /// node identity ([`crate::node_key`]). The actor logs live under
    /// `<workgroup_root>/collab/logs` — the Syncthing-replicated tree, matching
    /// the chat worker's `<workgroup_root>/<self>/chat/…` layout.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, self_host: String, signing_key: SigningKey) -> Self {
        let log_root = workgroup_root.join("collab").join("logs");
        Self {
            self_actor: ActorId::new(self_host),
            signing_key,
            log_root,
            poll_interval: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
        }
    }

    /// Override the Bus root (tests point it at a tempdir Persist).
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    /// Override the actor-log root (tests point it at a tempdir).
    #[must_use]
    pub fn with_log_root(mut self, p: PathBuf) -> Self {
        self.log_root = p;
        self
    }

    /// Override the poll cadence (tests use a short value).
    #[must_use]
    pub const fn with_poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    /// One poll pass — the headless-testable core (drives the whole worker with
    /// an injected Persist + tempdir roots, no tokio timer, no live mesh).
    fn tick_once(&self, persist: &Persist, state: &mut CollabState, now_ms: i64) {
        let mut touched: BTreeSet<SpaceId> = BTreeSet::new();
        let mut changed = false;
        self.drain_commands(persist, state, now_ms, &mut touched, &mut changed);
        self.drain_inbound(persist, state, &mut touched, &mut changed);
        self.backfill_logs(state, &mut touched, &mut changed);
        self.publish_read_models(persist, state, &touched, changed);
    }

    /// Drain every `action/collab/<verb>` lane: decode the [`CollabCommand`], run
    /// [`CollabEngine::apply`] (validate against the folded state + mint + sign
    /// the events with this node's identity), append each event to this node's own
    /// per-space actor log (durable) BEFORE relaying it, and publish it live. A
    /// denied command is logged (visible), never a silent drop.
    fn drain_commands(
        &self,
        persist: &Persist,
        state: &mut CollabState,
        now_ms: i64,
        touched: &mut BTreeSet<SpaceId>,
        changed: &mut bool,
    ) {
        let signer = Ed25519Signer::new(self.signing_key.clone());
        for verb in COMMAND_VERBS {
            let topic = topics::command_topic(verb);
            for m in take_new_forward(persist, &mut state.cursors, &topic) {
                let Some(body) = m.body.as_deref() else {
                    tracing::warn!(target: "mackesd::collab", verb, "action/collab command with empty body");
                    continue;
                };
                let cmd: CollabCommand = match serde_json::from_str(body) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(target: "mackesd::collab", verb, error = %e, "bad action/collab command body");
                        continue;
                    }
                };
                let events = match state.engine.apply(&cmd, &signer, &mut state.ids, now_ms) {
                    Ok(evs) => evs,
                    Err(e) => {
                        // A denied action is a typed error — visible, never a silent no-op.
                        tracing::warn!(target: "mackesd::collab", verb, error = %e, "collab command denied");
                        continue;
                    }
                };
                for env in &events {
                    // Durable-append to this node's own per-space actor log BEFORE
                    // we relay, so we never publish an event the log couldn't
                    // persist (append is idempotent; the log is the source of truth
                    // a restart rebuilds the projection from).
                    match self.append_own(state, env) {
                        Ok(()) => {
                            self.publish_event(persist, env);
                            if !env.space_id.is_nil() {
                                touched.insert(env.space_id);
                            }
                            *changed = true;
                        }
                        Err(e) => tracing::warn!(
                            target: "mackesd::collab",
                            error = %e,
                            "actor-log append failed; not publishing event",
                        ),
                    }
                }
            }
        }
    }

    /// Append `env` to this node's own `<log_root>/<space>/<self>.jsonl` actor log,
    /// caching the open handle per space so a hot lane does not reopen + reload the
    /// file each event. Idempotent by event id.
    fn append_own(
        &self,
        state: &mut CollabState,
        env: &CollabEventEnvelope,
    ) -> mde_collab_core::Result<()> {
        let space = env.space_id;
        if !state.own_logs.contains_key(&space) {
            let log = FileActorLog::open(&self.log_root, space, &self.self_actor)?;
            state.own_logs.insert(space, log);
        }
        let log = state
            .own_logs
            .get_mut(&space)
            .expect("own actor log just inserted");
        log.append(env)?;
        Ok(())
    }

    /// Drain the live `collab/event/*` lanes for events authored by OTHER actors
    /// (our own lane is already in the engine) and merge them: signature-checked
    /// (a forged event is dropped), deduped, order-independent.
    fn drain_inbound(
        &self,
        persist: &Persist,
        state: &mut CollabState,
        touched: &mut BTreeSet<SpaceId>,
        changed: &mut bool,
    ) {
        let all_topics = persist.list_topics().unwrap_or_default();
        let mut incoming: Vec<CollabEventEnvelope> = Vec::new();
        for topic in &all_topics {
            if !topic.starts_with(topics::EVENT_PREFIX) {
                continue;
            }
            match topics::parse_event_topic(topic) {
                // Skip our own authored lane — those events are already ingested.
                Some((_space, actor)) if actor == self.self_actor => continue,
                Some(_) => {}
                None => continue,
            }
            // Events are idempotent under merge, so drain the full lane on first
            // sight (a foreign lane only appears once it carries events) and
            // forward thereafter.
            for m in take_new_all(persist, &mut state.cursors, topic) {
                let Some(body) = m.body.as_deref() else {
                    continue;
                };
                match serde_json::from_str::<CollabEventEnvelope>(body) {
                    Ok(env) => incoming.push(env),
                    Err(e) => tracing::warn!(
                        target: "mackesd::collab",
                        topic = topic.as_str(),
                        error = %e,
                        "bad collab/event envelope",
                    ),
                }
            }
        }
        self.merge_batch(state, incoming, touched, changed, "bus");
    }

    /// Backfill from the replicated actor logs on disk (the Syncthing durable
    /// path): re-read each log whose file grew since we last saw it and merge its
    /// envelopes (idempotent). This is how a reconnecting node converges once
    /// Syncthing has delivered a neighbour's log, and how a restart rebuilds its
    /// own projection from its own durable log.
    ///
    /// WL-FUNC-011 Phase 2 follow-up: re-reads the whole grown log (mirroring the
    /// engine's own full-refold note); a worker at fleet scale would fold each log
    /// incrementally from a per-file offset.
    fn backfill_logs(
        &self,
        state: &mut CollabState,
        touched: &mut BTreeSet<SpaceId>,
        changed: &mut bool,
    ) {
        let mut incoming: Vec<CollabEventEnvelope> = Vec::new();
        for path in collect_log_files(&self.log_root) {
            let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if state.log_sizes.get(&path) == Some(&len) {
                continue; // unchanged since last backfill
            }
            incoming.extend(read_log_envelopes(&path));
            state.log_sizes.insert(path, len);
        }
        self.merge_batch(state, incoming, touched, changed, "log");
    }

    /// Merge a batch of foreign/replicated events into the engine, marking the
    /// touched spaces + whether anything was newly accepted, and logging any
    /// dropped-unverifiable count. The shared tail of [`drain_inbound`] +
    /// [`backfill_logs`].
    fn merge_batch(
        &self,
        state: &mut CollabState,
        incoming: Vec<CollabEventEnvelope>,
        touched: &mut BTreeSet<SpaceId>,
        changed: &mut bool,
        source: &'static str,
    ) {
        if incoming.is_empty() {
            return;
        }
        for env in &incoming {
            if !env.space_id.is_nil() {
                touched.insert(env.space_id);
            }
        }
        match state.engine.merge(incoming) {
            Ok(outcome) => {
                if outcome.accepted > 0 {
                    *changed = true;
                }
                if outcome.dropped_invalid > 0 {
                    tracing::warn!(
                        target: "mackesd::collab",
                        source,
                        dropped = outcome.dropped_invalid,
                        "dropped unverifiable collab events (bad/absent signature)",
                    );
                }
            }
            Err(e) => {
                tracing::warn!(target: "mackesd::collab", source, error = %e, "collab merge failed")
            }
        }
    }

    /// Publish one signed event live on `collab/event/<space>/<self>`.
    fn publish_event(&self, persist: &Persist, env: &CollabEventEnvelope) {
        let topic = topics::event_topic(env.space_id, &self.self_actor);
        match serde_json::to_string(env) {
            Ok(body) => publish(persist, &topic, &body),
            Err(e) => {
                tracing::warn!(target: "mackesd::collab", error = %e, "serialize collab event failed")
            }
        }
    }

    /// Republish the `state/collab/*` read models the surface + peers render:
    /// the per-space models for every touched space, and — whenever anything
    /// changed — the fleet-wide directory / presence / alert-inbox / transfer-jobs
    /// rollups. Latest-wins with a per-topic body cache, so an unchanged model is
    /// not rewritten (the chat-worker cadence).
    fn publish_read_models(
        &self,
        persist: &Persist,
        state: &mut CollabState,
        touched: &BTreeSet<SpaceId>,
        changed: bool,
    ) {
        for &space in touched {
            // Per-space read models. Each projection query is computed into an
            // owned Result first (releasing the engine borrow) before the
            // last-published cache is touched.
            let conversation = state.engine.projection().conversation_timeline(space, None);
            publish_state(
                persist,
                &mut state.last_published,
                &topics::space_state_topic(proj::CONVERSATION, space),
                conversation,
            );
            let activity = state.engine.projection().activity_feed(space);
            publish_state(
                persist,
                &mut state.last_published,
                &topics::space_state_topic(proj::ACTIVITY, space),
                activity,
            );
            let clipboard = state.engine.projection().clipboard_lane(space);
            publish_state(
                persist,
                &mut state.last_published,
                &topics::space_state_topic(proj::CLIPBOARD_LANE, space),
                clipboard,
            );
            let files = state.engine.projection().file_references(space);
            publish_state(
                persist,
                &mut state.last_published,
                &topics::space_state_topic(proj::FILE_REFERENCES, space),
                files,
            );
            let docs = state.engine.projection().document_sessions(Some(space));
            publish_state(
                persist,
                &mut state.last_published,
                &topics::space_state_topic(proj::DOCUMENT_SESSIONS, space),
                docs,
            );
            let calls = state.engine.projection().call_state(Some(space));
            publish_state(
                persist,
                &mut state.last_published,
                &topics::space_state_topic(proj::CALL_STATE, space),
                calls,
            );
        }

        if !changed {
            return;
        }
        let directory = state.engine.projection().space_directory(&self.self_actor);
        publish_state(
            persist,
            &mut state.last_published,
            &topics::state_topic(proj::SPACE_DIRECTORY),
            directory,
        );
        let presence = state.engine.projection().presence_board();
        publish_state(
            persist,
            &mut state.last_published,
            &topics::state_topic(proj::PRESENCE),
            presence,
        );
        let alerts = state.engine.projection().alert_inbox();
        publish_state(
            persist,
            &mut state.last_published,
            &topics::state_topic(proj::ALERT_INBOX),
            alerts,
        );
        let transfers = state.engine.projection().transfer_jobs();
        publish_state(
            persist,
            &mut state.last_published,
            &topics::state_topic(proj::TRANSFER_JOBS),
            transfers,
        );
    }
}

/// In-memory per-run worker state, carried across ticks.
struct CollabState {
    /// The folded collaboration engine for this node's actor (validate + sign +
    /// project + merge). Its projection is an in-memory SQLite store rebuilt from
    /// the durable actor logs on boot — the logs are the Syncthing-replicable
    /// source of truth, the projection is derived + convergent.
    engine: CollabEngine,
    /// The event-id source for authored events (random UUIDv4).
    ids: RandomIds,
    /// Per-topic drain cursor (forward-only for command lanes, drain-all-on-first
    /// -sight for event lanes — see [`take_new_forward`] / [`take_new_all`]).
    cursors: BTreeMap<String, Option<String>>,
    /// This node's own per-space actor logs, kept open across ticks so a hot lane
    /// does not reopen + reload the file each append.
    own_logs: BTreeMap<SpaceId, FileActorLog>,
    /// The last-seen byte length of each replicated log file — a log is re-read +
    /// merged only when its file has grown.
    log_sizes: BTreeMap<PathBuf, u64>,
    /// The last published body per `state/collab/*` topic — skip republishing an
    /// identical read model (latest-wins churn guard).
    last_published: BTreeMap<String, String>,
}

impl CollabState {
    /// A fresh per-run state for `actor`, with an in-memory SQLite projection.
    fn new(actor: ActorId) -> mde_collab_core::Result<Self> {
        Ok(Self {
            engine: CollabEngine::new(actor, Projection::open_in_memory()?),
            ids: RandomIds,
            cursors: BTreeMap::new(),
            own_logs: BTreeMap::new(),
            log_sizes: BTreeMap::new(),
            last_published: BTreeMap::new(),
        })
    }
}

/// New messages on `topic` since the cursor, seeding the cursor to the current
/// head on first sight (no backlog replay), then advancing it. The forward-only
/// discipline the command lanes use so a restart never re-executes a stale
/// command (mirrors the chat worker's drain cursor).
fn take_new_forward(
    persist: &Persist,
    cursors: &mut BTreeMap<String, Option<String>>,
    topic: &str,
) -> Vec<StoredMessage> {
    match cursors.get(topic) {
        None => {
            let head = persist
                .list_since(topic, None)
                .ok()
                .and_then(|m| m.last().map(|x| x.ulid.clone()));
            cursors.insert(topic.to_string(), head);
            Vec::new()
        }
        Some(cur) => {
            let cur = cur.clone();
            let msgs = persist
                .list_since(topic, cur.as_deref())
                .unwrap_or_default();
            if let Some(last) = msgs.last() {
                cursors.insert(topic.to_string(), Some(last.ulid.clone()));
            }
            msgs
        }
    }
}

/// New messages on `topic`, draining the FULL lane on first sight (then forward).
/// The event lanes use this: a `collab/event/*` lane only appears once it carries
/// events, so draining it from the start converges a node that discovered the
/// lane after start; merge is idempotent + signature-checked, so replay is safe.
fn take_new_all(
    persist: &Persist,
    cursors: &mut BTreeMap<String, Option<String>>,
    topic: &str,
) -> Vec<StoredMessage> {
    let since = cursors.get(topic).cloned().flatten();
    let msgs = persist
        .list_since(topic, since.as_deref())
        .unwrap_or_default();
    if let Some(last) = msgs.last() {
        cursors.insert(topic.to_string(), Some(last.ulid.clone()));
    } else {
        cursors.entry(topic.to_string()).or_insert(None);
    }
    msgs
}

/// Serialize + publish a read model, skipping the write when the body is
/// byte-identical to what was last published on the topic (latest-wins). A model
/// the projection could not build is logged at debug + skipped, never faked.
fn publish_state<T: serde::Serialize>(
    persist: &Persist,
    last_published: &mut BTreeMap<String, String>,
    topic: &str,
    model: mde_collab_core::Result<T>,
) {
    match model {
        Ok(m) => match serde_json::to_string(&m) {
            Ok(body) => {
                if last_published.get(topic).map(String::as_str) == Some(body.as_str()) {
                    return;
                }
                publish(persist, topic, &body);
                last_published.insert(topic.to_string(), body);
            }
            Err(e) => {
                tracing::warn!(target: "mackesd::collab", topic, error = %e, "serialize read model failed")
            }
        },
        Err(e) => {
            tracing::debug!(target: "mackesd::collab", topic, error = %e, "read model unavailable")
        }
    }
}

/// In-process Bus publish (best-effort). Writing to the local Persist store is the
/// same store the broker + surface read; whether it federates to peers is the
/// broker's job (the live multi-node reach is integration-gated).
fn publish(persist: &Persist, topic: &str, body: &str) {
    if let Err(e) = persist.write(topic, Priority::Default, None, Some(body)) {
        tracing::debug!(target: "mackesd::collab", topic, error = %e, "collab publish failed");
    }
}

/// Every `<space>/<actor>.jsonl` actor-log file under `root` (two levels: a space
/// directory, then its per-actor logs). Missing/unreadable dirs yield an empty
/// set (a fresh node with no logs yet).
fn collect_log_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(spaces) = std::fs::read_dir(root) else {
        return out;
    };
    for space_entry in spaces.flatten() {
        let space_dir = space_entry.path();
        if !space_dir.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&space_dir) else {
            continue;
        };
        for file in files.flatten() {
            let p = file.path();
            if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                out.push(p);
            }
        }
    }
    out
}

/// Read every signed envelope from one JSON-lines actor-log file. A torn/partial
/// trailing line (a crash between sign + fsync) or a malformed line is skipped,
/// never fatal.
fn read_log_envelopes(path: &Path) -> Vec<CollabEventEnvelope> {
    let mut out = Vec::new();
    let Ok(content) = std::fs::read_to_string(path) else {
        return out;
    };
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<CollabEventEnvelope>(line) {
            Ok(env) => out.push(env),
            Err(e) => tracing::warn!(
                target: "mackesd::collab",
                path = %path.display(),
                error = %e,
                "skipping malformed actor-log line",
            ),
        }
    }
    out
}

fn resolve_default_bus_root(
    env_root: Option<std::ffi::OsString>,
    data_dir: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(root) = env_root.filter(|root| !root.is_empty()) {
        return Some(PathBuf::from(root));
    }
    Some(data_dir?.join("mde").join("bus"))
}

fn default_bus_root() -> Option<PathBuf> {
    resolve_default_bus_root(std::env::var_os("MDE_BUS_ROOT"), dirs::data_dir())
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[async_trait::async_trait]
impl Worker for CollabWorker {
    fn name(&self) -> &'static str {
        "collab"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self.bus_root_override.clone().or_else(default_bus_root) else {
            tracing::debug!(target: "mackesd::collab", "no bus root; worker idle");
            return Ok(());
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(target: "mackesd::collab", error = %e, "persist open failed; worker idle");
                return Ok(());
            }
        };
        let mut state = match CollabState::new(self.self_actor.clone()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(target: "mackesd::collab", error = %e, "projection open failed; worker idle");
                return Ok(());
            }
        };
        // Rebuild the projection from the durable actor logs (own + replicated)
        // and publish the initial read models immediately.
        {
            let mut touched: BTreeSet<SpaceId> = BTreeSet::new();
            let mut changed = false;
            self.backfill_logs(&mut state, &mut touched, &mut changed);
            self.publish_read_models(
                &persist,
                &mut state,
                &touched,
                changed || !touched.is_empty(),
            );
        }
        let mut tick = tokio::time::interval(self.poll_interval);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    self.tick_once(&persist, &mut state, now_unix_ms());
                }
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use mde_collab_types::value::MessageBody;
    use mde_collab_types::{
        ConversationTimeline, PresenceState, SpaceDirectory, SpaceKind, SpaceRole,
    };
    use rand::rngs::OsRng;

    fn key() -> SigningKey {
        SigningKey::generate(&mut OsRng)
    }

    fn worker(root: &Path, actor: &str) -> CollabWorker {
        CollabWorker::new(root.to_path_buf(), actor.into(), key())
            .with_bus_root(root.join("bus"))
            .with_log_root(root.join("collab-logs"))
    }

    fn persist_at(root: &Path) -> Persist {
        Persist::open(root.join("bus")).expect("open persist")
    }

    fn write_command(persist: &Persist, cmd: &CollabCommand) {
        let body = serde_json::to_string(cmd).expect("serialize command");
        persist
            .write(
                &topics::command_topic(cmd.verb()),
                Priority::Default,
                None,
                Some(&body),
            )
            .expect("write command");
    }

    fn write_event(persist: &Persist, env: &CollabEventEnvelope) {
        let body = serde_json::to_string(env).expect("serialize event");
        persist
            .write(
                &topics::event_topic(env.space_id, &env.actor),
                Priority::Default,
                None,
                Some(&body),
            )
            .expect("write event");
    }

    fn only_space(state: &CollabState) -> SpaceId {
        let spaces: Vec<SpaceId> = state.engine.state().spaces.keys().copied().collect();
        assert_eq!(spaces.len(), 1, "exactly one space in the engine");
        spaces[0]
    }

    // ── pure helpers ────────────────────────────────────────────────────

    #[test]
    fn command_verbs_cover_every_command_variant() {
        // Every CollabCommand's verb must be a drained lane, or a command silently
        // never runs. Build one of each variant and assert its verb is listed.
        let space = SpaceId::new();
        let samples = [
            CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "x".into(),
            },
            CollabCommand::RenameSpace {
                space,
                name: "x".into(),
            },
            CollabCommand::DeleteSpace { space },
            CollabCommand::AddMember {
                space,
                actor: ActorId::new("a"),
                role: SpaceRole::Member,
            },
            CollabCommand::RemoveMember {
                space,
                actor: ActorId::new("a"),
            },
            CollabCommand::SetMemberRole {
                space,
                actor: ActorId::new("a"),
                role: SpaceRole::Owner,
            },
            CollabCommand::JoinSpace { space },
            CollabCommand::LeaveSpace { space },
            CollabCommand::SetPresence {
                presence: PresenceState::Online,
                status: None,
            },
            CollabCommand::SendMessage {
                space,
                thread: None,
                body: MessageBody::new("x"),
            },
        ];
        for cmd in &samples {
            assert!(
                COMMAND_VERBS.contains(&cmd.verb()),
                "COMMAND_VERBS is missing the verb {:?}",
                cmd.verb()
            );
        }
        // The count must equal the full taxonomy (41 verbs) so a NEW command
        // variant forces an update here.
        assert_eq!(
            COMMAND_VERBS.len(),
            41,
            "COMMAND_VERBS drifted from the taxonomy"
        );
    }

    #[test]
    fn take_new_forward_is_forward_only_on_first_sight() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());
        let mut cursors = BTreeMap::new();
        persist
            .write("t/forward", Priority::Default, None, Some("old"))
            .unwrap();
        // First sight seeds to head → the pre-existing message is NOT replayed.
        assert!(take_new_forward(&persist, &mut cursors, "t/forward").is_empty());
        persist
            .write("t/forward", Priority::Default, None, Some("new"))
            .unwrap();
        let got = take_new_forward(&persist, &mut cursors, "t/forward");
        assert_eq!(
            got.len(),
            1,
            "only the message written after the seed drains"
        );
        assert_eq!(got[0].body.as_deref(), Some("new"));
    }

    #[test]
    fn take_new_all_drains_backlog_on_first_sight() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());
        let mut cursors = BTreeMap::new();
        persist
            .write("t/all", Priority::Default, None, Some("a"))
            .unwrap();
        persist
            .write("t/all", Priority::Default, None, Some("b"))
            .unwrap();
        let got = take_new_all(&persist, &mut cursors, "t/all");
        assert_eq!(got.len(), 2, "the full backlog drains on first sight");
        // Forward thereafter.
        assert!(take_new_all(&persist, &mut cursors, "t/all").is_empty());
    }

    // ── the worker flow ─────────────────────────────────────────────────

    #[test]
    fn command_produces_signed_event_projected_and_published() {
        let dir = tempfile::tempdir().expect("tempdir");
        let w = worker(dir.path(), "eagle");
        let persist = persist_at(dir.path());
        let mut state = CollabState::new(w.self_actor.clone()).expect("state");

        // Tick once to seed the command cursors (forward-only), then publish a
        // CreateSpace command and drain it.
        w.tick_once(&persist, &mut state, 100);
        write_command(
            &persist,
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
        );
        w.tick_once(&persist, &mut state, 200);

        // Applied: the engine holds the space, with eagle a present Owner.
        let space = only_space(&state);
        assert!(state.engine.state().is_owner(space, &w.self_actor));

        // The input lane is the canonical action/collab topic.
        assert_eq!(
            topics::command_topic("create_space"),
            "action/collab/create_space"
        );

        // Published live events: collab/event/<space>/eagle carries the two signed
        // events (SpaceCreated + MemberJoined), and each verifies.
        let event_topic = topics::event_topic(space, &w.self_actor);
        assert_eq!(event_topic, format!("collab/event/{space}/eagle"));
        let published = persist.list_since(&event_topic, None).expect("list events");
        assert_eq!(published.len(), 2, "SpaceCreated + MemberJoined published");
        for m in &published {
            let env: CollabEventEnvelope =
                serde_json::from_str(m.body.as_deref().expect("event body")).expect("decode");
            assert!(env.verify(), "published event carries a valid signature");
            assert_eq!(env.actor, w.self_actor);
            assert_eq!(env.space_id, space);
        }

        // Projected + published read model: state/collab/directory lists the space.
        let dir_topic = topics::state_topic(proj::SPACE_DIRECTORY);
        assert_eq!(dir_topic, "state/collab/directory");
        let dir_msg = persist
            .read_latest(&dir_topic)
            .expect("read directory")
            .expect("directory published");
        let directory: SpaceDirectory =
            serde_json::from_str(dir_msg.body.as_deref().expect("dir body")).expect("decode dir");
        assert_eq!(directory.spaces.len(), 1);
        assert_eq!(directory.spaces[0].id, space);

        // Durable: this node's own actor log holds the two events.
        let log = FileActorLog::open(&w.log_root, space, &w.self_actor).expect("open log");
        assert_eq!(
            log.len(),
            2,
            "both events durably appended to the actor log"
        );

        // A follow-up SendMessage into the space projects into the conversation.
        write_command(
            &persist,
            &CollabCommand::SendMessage {
                space,
                thread: None,
                body: MessageBody::new("hello **mesh**"),
            },
        );
        w.tick_once(&persist, &mut state, 300);
        let convo_topic = topics::space_state_topic(proj::CONVERSATION, space);
        assert_eq!(convo_topic, format!("state/collab/conversation/{space}"));
        let convo_msg = persist
            .read_latest(&convo_topic)
            .expect("read convo")
            .expect("conversation published");
        let timeline: ConversationTimeline =
            serde_json::from_str(convo_msg.body.as_deref().expect("convo body")).expect("decode");
        assert_eq!(timeline.messages.len(), 1, "the message is projected");
    }

    #[test]
    fn foreign_event_merges_and_a_forged_event_is_dropped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let w = worker(dir.path(), "eagle");
        let persist = persist_at(dir.path());
        let mut state = CollabState::new(w.self_actor.clone()).expect("state");
        w.tick_once(&persist, &mut state, 100); // seed cursors

        // A foreign node (nyc3) authors a real space via its own engine.
        let foreign_signer = Ed25519Signer::new(key());
        let mut foreign = CollabEngine::in_memory(ActorId::new("nyc3")).expect("engine");
        let mut fids = RandomIds;
        let created = foreign
            .apply(
                &CollabCommand::CreateSpace {
                    kind: SpaceKind::Team,
                    name: "remote".into(),
                },
                &foreign_signer,
                &mut fids,
                50,
            )
            .expect("foreign create");
        let space = created[0].space_id;
        for env in &created {
            write_event(&persist, env); // publish on collab/event/<space>/nyc3
        }

        // A DISTINCT foreign event (a rename), then tamper its author so its
        // signature no longer verifies — a forgery on a lane the worker drains.
        let mut renamed = foreign
            .apply(
                &CollabCommand::RenameSpace {
                    space,
                    name: "tampered".into(),
                },
                &foreign_signer,
                &mut fids,
                60,
            )
            .expect("foreign rename");
        let mut forged = renamed.remove(0);
        let forged_id = forged.event_id;
        forged.actor = ActorId::new("attacker");
        assert!(!forged.verify(), "the tamper must invalidate the signature");
        write_event(&persist, &forged);

        w.tick_once(&persist, &mut state, 200);

        // The valid foreign events merged: the space exists, name unchanged.
        let agg = state
            .engine
            .state()
            .space(space)
            .expect("foreign space merged");
        assert_eq!(agg.name, "remote", "the valid create merged");
        // The forged rename was DROPPED: the name is not "tampered" and the forged
        // event id is absent from the engine's event set.
        assert!(
            !state
                .engine
                .all_events()
                .iter()
                .any(|e| e.event_id == forged_id),
            "the forged event was dropped, not ingested",
        );
    }

    #[test]
    fn two_workers_converge_on_divergent_commands() {
        let dir_a = tempfile::tempdir().expect("tempdir a");
        let dir_b = tempfile::tempdir().expect("tempdir b");
        let wa = worker(dir_a.path(), "alpha");
        let wb = worker(dir_b.path(), "beta");
        let pa = persist_at(dir_a.path());
        let pb = persist_at(dir_b.path());
        let mut sa = CollabState::new(wa.self_actor.clone()).expect("state a");
        let mut sb = CollabState::new(wb.self_actor.clone()).expect("state b");
        // Seed both.
        wa.tick_once(&pa, &mut sa, 100);
        wb.tick_once(&pb, &mut sb, 100);

        // alpha creates a shared space and adds beta as a member.
        write_command(
            &pa,
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "shared".into(),
            },
        );
        wa.tick_once(&pa, &mut sa, 200);
        let space = only_space(&sa);
        write_command(
            &pa,
            &CollabCommand::AddMember {
                space,
                actor: ActorId::new("beta"),
                role: SpaceRole::Member,
            },
        );
        wa.tick_once(&pa, &mut sa, 300);

        // Relay alpha's events onto beta's bus (simulating the broker / Syncthing),
        // so beta learns the space + its own membership, then converges.
        let relay = |from: &CollabState, to: &Persist| {
            for env in from.engine.all_events() {
                if env.actor == ActorId::new("alpha") {
                    write_event(to, &env);
                }
            }
        };
        relay(&sa, &pb);
        wb.tick_once(&pb, &mut sb, 400);
        assert!(
            sb.engine.state().is_member(space, &ActorId::new("beta")),
            "beta learned its membership by merging alpha's events",
        );

        // Divergent commands: each member posts a message on its own node.
        write_command(
            &pa,
            &CollabCommand::SendMessage {
                space,
                thread: None,
                body: MessageBody::new("from-alpha"),
            },
        );
        wa.tick_once(&pa, &mut sa, 500);
        write_command(
            &pb,
            &CollabCommand::SendMessage {
                space,
                thread: None,
                body: MessageBody::new("from-beta"),
            },
        );
        wb.tick_once(&pb, &mut sb, 600);

        // Exchange each node's authored events both directions.
        for env in sa.engine.all_events() {
            if env.actor == ActorId::new("alpha") {
                write_event(&pb, &env);
            }
        }
        for env in sb.engine.all_events() {
            if env.actor == ActorId::new("beta") {
                write_event(&pa, &env);
            }
        }
        wb.tick_once(&pb, &mut sb, 700);
        wa.tick_once(&pa, &mut sa, 800);

        // Convergence: byte-identical projected state regardless of the divergent
        // command order (mde-collab-core's guarantee, exercised through the worker
        // seams).
        let fa = sa.engine.projection().dump_tables().expect("dump a");
        let fb = sb.engine.projection().dump_tables().expect("dump b");
        assert_eq!(
            fa, fb,
            "the two workers converge to identical projected state"
        );
    }

    #[tokio::test]
    async fn worker_exits_on_shutdown_token() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut w = worker(dir.path(), "eagle").with_poll_interval(Duration::from_millis(50));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let _ = tx.send(true);
        let r = tokio::time::timeout(Duration::from_secs(3), w.run(token))
            .await
            .expect("worker must exit on shutdown");
        assert!(r.is_ok());
    }
}
