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
use std::fs::File;
use std::io::{BufRead, BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use std::str::FromStr;

use ed25519_dalek::SigningKey;
use mde_bus::hooks::config::Priority;
use mde_bus::persist::{Persist, StoredMessage};
use mde_collab_core::{
    ingest_and_register_file, ActorLog, BlobStore, CollabEngine, Ed25519Signer, FileActorLog,
    FsBlobStore, Projection, RandomIds,
};
use mde_collab_types::topics::{self, projection as proj};
use mde_collab_types::value::{
    sha256_hex, AlertAction, AlertActionKind, AlertPayload, ClipItemKind, ClipboardItem,
    PayloadRef, Severity,
};
use mde_collab_types::{
    ActorId, AiSuggestionRequestStatus, AiSuggestionRequestView, AiSuggestionRequests,
    CollabCommand, CollabEventEnvelope, CollabEventKind, SpaceId, SpaceKind, SpaceRole,
    MAX_CLIPBOARD_TEXT_BYTES,
};

use super::{ShutdownToken, Worker};
use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};

/// The alert-source topic prefixes the collab worker folds into
/// [`AlertRaised`](CollabEventKind::AlertRaised) events — the same truthful lanes
/// the `super::chat` worker's `ALERT_LANE_PREFIXES` subscribes to (the emitters
/// keep publishing their own events unchanged; we adapt them). Kept in step with
/// the chat set so a node's alerts fold identically into both suites during the
/// Phase-4 overlap.
const ALERT_LANE_PREFIXES: &[&str] = &[
    "event/security/",
    "fleet/sec",
    "event/firewall",
    "event/compute/",
    "event/kvm/",
    "event/dc/",
    "fdo/",
    "event/notify/",
    "fleet/health/",
];

/// The cross-mesh clipboard-capture lane the `clipboard_sync` worker broadcasts
/// on. Its body is `{ id, text, source, time }` (text-only, bounded); we fold
/// each capture into a [`ClipboardPublished`](CollabEventKind::ClipboardPublished)
/// event, recomputing the full content address + gating on size.
const CLIPBOARD_CAPTURE_TOPIC: &str = "event/clipboard/clip";

/// The clipboard-lane size ceiling: a clip up to 1 MiB rides the lane; anything
/// larger is a Transfer, not a clip, so it is **not** folded here (never
/// truncated) — it belongs to the WL-FUNC-006 transfer path.
const MAX_CLIP_BYTES: u64 = MAX_CLIPBOARD_TEXT_BYTES as u64;

/// The name of the node's own **system space** — a per-node space the worker owns
/// that holds its folded node-level facts (alerts + clipboard captures), so those
/// facts land in a real, ackable, member space rather than a headless id.
const SYSTEM_SPACE_NAME: &str = "System";

/// The default poll cadence (tests override with a short value; the loop is
/// entirely edge-driven off the Bus so the interval only bounds latency).
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Keep startup recovery responsive without spinning on a missing/unopenable Bus.
const MIN_BUS_RETRY_INTERVAL: Duration = Duration::from_millis(10);
const MAX_BUS_RETRY_INTERVAL: Duration = Duration::from_secs(2);

#[cfg(test)]
type BusOpenFn = dyn Fn() -> Result<Option<Persist>, String> + Send + Sync;
#[cfg(test)]
type CursorPrimeFn =
    dyn Fn(&Persist) -> Result<BTreeMap<String, Option<String>>, String> + Send + Sync;

/// Worker-side merge slices stay comfortably below the collaboration core's
/// fail-closed 4,096-envelope admission cap. The core still owns the hard
/// all-or-nothing batch bound; the worker just avoids manufacturing oversized
/// aggregate batches from retained Bus lanes or actor logs.
const MAX_WORKER_MERGE_BATCH_EVENTS: usize = 4096;

/// Historical actor-log work admitted from one lane in one poll. A lane gets a
/// live-tail slice and a sequential-history slice, so this deliberately stays
/// tiny: with sixteen selected lanes the worst-case poll admits 128 envelopes.
/// Signature verification and projection updates are CPU-heavy enough that a
/// 256-envelope slice held one Tokio worker for minutes on the four-core Dell.
const LOG_BACKFILL_SLICE_EVENTS: usize = 4;
/// Bound total retained-log work while allowing every normally hot fleet lane
/// to advance in the same worker tick. One slice was insufficient: a handful
/// of continuously appended system logs could occupy every recent turn and
/// starve a newly replicated Teams log indefinitely.
const LOG_BACKFILL_SLICES_PER_TICK: usize = 16;

/// Actor-log JSONL lines mirror the projection's 256 KiB serialized-envelope
/// boundary, plus the writer's trailing `\n`. Oversized retained lines are
/// skipped before serde/projection so a hostile durable log cannot force
/// unbounded materialization in this worker.
const MAX_ACTOR_LOG_LINE_BYTES: usize = 256 * 1024 + 1;

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
    "commit_file_generation",
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
    "cancel_ai_suggestion",
];

/// The only hosted provider permitted for Communications suggestions.
const AI_PROVIDER_DIGITALOCEAN: &str = "digitalocean";

/// Keep the provider sidecar read model bounded and deterministic.
const MAX_AI_REQUEST_ROWS: usize = 64;

/// Honest fail-closed reason before the operator grants global cloud consent.
const AI_CONSENT_REQUIRED: &str = "DigitalOcean cloud consent is required before AI suggestions";

/// Capability context for every mutable `action/collab/<verb>` command.
const COLLAB_AUTH_VERB: &str = "collab-command";

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
    /// Syncthing-replicated content-addressed Files payload root.
    content_root: PathBuf,
    /// Poll cadence.
    poll_interval: Duration,
    /// Bus root override (tests point it at a tempdir Persist).
    bus_root_override: Option<PathBuf>,
    /// Dynamic Bus resolve/open seam for startup-race tests.
    #[cfg(test)]
    bus_open_override: Option<Arc<BusOpenFn>>,
    /// Fail-closed transient-lane cursor-prime seam for startup-race tests.
    #[cfg(test)]
    cursor_prime_override: Option<Arc<CursorPrimeFn>>,
    /// Verifier for the root-only capability on the mutable command lanes.
    authorizer: Arc<ActionAuthorizer>,
    /// Whether global hosted-AI consent has been granted for this seat.
    ///
    /// This defaults fail-closed. The sealed-key/provider adapter that will
    /// consume this sidecar can flip it only through a future explicit consent
    /// authority; until then AI requests are visible failures and every local
    /// collaboration workflow continues.
    ai_cloud_consent: bool,
    /// Worker-owned real-media providers. Production explicitly activates the
    /// existing SIP/RTP adapter when its governed account is healthy; otherwise
    /// the registry remains fail-closed.
    call_media_providers: super::collab_media::CallMediaProviderRegistry,
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
        let content_root = workgroup_root.join("collab").join("content");
        Self {
            self_actor: ActorId::new(self_host),
            signing_key,
            log_root,
            content_root,
            poll_interval: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
            #[cfg(test)]
            bus_open_override: None,
            #[cfg(test)]
            cursor_prime_override: None,
            authorizer: Arc::new(ActionAuthorizer::production()),
            ai_cloud_consent: false,
            call_media_providers: super::collab_media::CallMediaProviderRegistry::production(),
        }
    }

    /// Override the Bus root (tests point it at a tempdir Persist).
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    /// Override dynamic Bus resolution/opening without changing production
    /// retry behavior.
    #[cfg(test)]
    #[must_use]
    fn with_bus_opener(mut self, open: Arc<BusOpenFn>) -> Self {
        self.bus_open_override = Some(open);
        self
    }

    /// Override transient-lane cursor priming for deterministic failure tests.
    #[cfg(test)]
    #[must_use]
    fn with_cursor_primer(mut self, prime: Arc<CursorPrimeFn>) -> Self {
        self.cursor_prime_override = Some(prime);
        self
    }

    /// Override the Bus capability verifier for deterministic hostile fixtures.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_authorizer(mut self, authorizer: Arc<ActionAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    /// Override AI-cloud consent for deterministic sidecar tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_ai_cloud_consent_for_test(mut self, consent: bool) -> Self {
        self.ai_cloud_consent = consent;
        self
    }

    /// Register bounded real-media proof providers for tests or future
    /// in-daemon WebRTC/SIP/LiveKit adapters.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_call_media_providers(
        mut self,
        providers: super::collab_media::CallMediaProviderRegistry,
    ) -> Self {
        self.call_media_providers = providers;
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
    pub fn with_poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    fn open_bus(&self) -> Result<Option<Persist>, String> {
        #[cfg(test)]
        if let Some(open) = self.bus_open_override.as_ref() {
            return open();
        }

        Persist::open(collab_bus_root(self.bus_root_override.clone()))
            .map(Some)
            .map_err(|error| error.to_string())
    }

    fn prime_transient_cursors(
        &self,
        persist: &Persist,
    ) -> Result<BTreeMap<String, Option<String>>, String> {
        #[cfg(test)]
        if let Some(prime) = self.cursor_prime_override.as_ref() {
            return prime(persist);
        }

        prime_transient_cursors(persist)
    }

    /// One poll pass — the headless-testable core (drives the whole worker with
    /// an injected Persist + tempdir roots, no tokio timer, no live mesh).
    fn tick_once(&self, persist: &Persist, state: &mut CollabState, now_ms: i64) {
        let mut touched = std::mem::take(&mut state.pending_file_projection_spaces);
        let mut changed = false;
        self.drain_inbound_sip_call(persist, state, now_ms, &mut touched, &mut changed);
        self.drain_call_provider_revocations(persist, state, now_ms, &mut touched, &mut changed);
        self.drain_commands(persist, state, now_ms, &mut touched, &mut changed);
        self.drain_inbound(persist, state, &mut touched, &mut changed);
        self.backfill_logs(state, &mut touched, &mut changed);
        // Fold the node's external subsystems into collab facts (WL-FUNC-011): the
        // truthful Bus alert lanes → AlertRaised, and the cross-mesh clipboard
        // captures → ClipboardPublished. Each folds into the node's own system
        // space, which is bootstrapped LAZILY — only the first time the node
        // actually has a node-level fact to record — so a node that never sees an
        // alert or clip carries no system space. The emitters publish unchanged.
        self.drain_alert_lanes(persist, state, now_ms, &mut touched, &mut changed);
        self.drain_clipboard_captures(persist, state, now_ms, &mut touched, &mut changed);
        self.publish_read_models(persist, state, &touched, changed);
    }

    /// Admit a provider-observed inbound SIP dialog into exactly one existing
    /// signed Collaboration space. The provider identity is untrusted until it
    /// normalizes to one current member alongside this node. The worker mints
    /// the sole opaque CallId and authors the sole CallStarted event; the SIP
    /// adapter only binds its exact still-pending dialog to that authority.
    fn drain_inbound_sip_call(
        &self,
        persist: &Persist,
        state: &mut CollabState,
        now_ms: i64,
        touched: &mut BTreeSet<SpaceId>,
        changed: &mut bool,
    ) {
        let Some(offer) = self.call_media_providers.pending_inbound_sip_call() else {
            return;
        };
        let space = match admit_inbound_sip_identity(state.engine.state(), &self.self_actor, &offer)
        {
            Ok(space) => space,
            Err(reason) => {
                let _ = self
                    .call_media_providers
                    .bind_inbound_sip_call(&offer, None);
                tracing::warn!(target: "mackesd::collab", reason, "inbound SIP identity refused");
                return;
            }
        };
        let call = mde_collab_types::CallId::new();
        // This compare-and-consume is the stale-dialog boundary. A newer INVITE
        // replacing the offer while membership was inspected cannot inherit the
        // earlier identity's authority.
        if !self
            .call_media_providers
            .bind_inbound_sip_call(&offer, Some(call))
        {
            tracing::warn!(target: "mackesd::collab", "inbound SIP dialog changed before authority binding");
            return;
        }
        let signer = Ed25519Signer::new(self.signing_key.clone());
        let command = CollabCommand::StartCall {
            space,
            call,
            kind: mde_collab_types::CallKind::Audio,
        };
        let events = match state
            .engine
            .apply(&command, &signer, &mut state.ids, now_ms)
        {
            Ok(events) => events,
            Err(error) => {
                tracing::warn!(target: "mackesd::collab", %call, %error, "admitted inbound SIP call could not be authored");
                return;
            }
        };
        for env in &events {
            match self.append_own(state, env) {
                Ok(()) => {
                    self.publish_event(persist, env);
                    touched.insert(space);
                    *changed = true;
                }
                Err(error) => {
                    tracing::warn!(target: "mackesd::collab", %call, %error, "inbound SIP actor-log append failed; event not published")
                }
            }
        }
    }

    fn drain_call_provider_revocations(
        &self,
        persist: &Persist,
        state: &mut CollabState,
        now_ms: i64,
        touched: &mut BTreeSet<SpaceId>,
        changed: &mut bool,
    ) {
        let signer = Ed25519Signer::new(self.signing_key.clone());
        for call in self.call_media_providers.take_revoked_calls() {
            let command = CollabCommand::HangUpCall { call };
            let events = match state
                .engine
                .apply(&command, &signer, &mut state.ids, now_ms)
            {
                Ok(events) => events,
                Err(error) => {
                    tracing::warn!(target: "mackesd::collab", %call, %error, "provider call revocation no longer matched an active local call");
                    continue;
                }
            };
            for env in &events {
                match self.append_own(state, env) {
                    Ok(()) => {
                        self.publish_event(persist, env);
                        if !env.space_id.is_nil() {
                            touched.insert(env.space_id);
                        }
                        *changed = true;
                    }
                    Err(error) => {
                        tracing::warn!(target: "mackesd::collab", %call, %error, "provider revocation actor-log append failed; event not published")
                    }
                }
            }
        }
    }

    /// The node's own **system space** id — derived deterministically from this
    /// node's actor so a restart finds the same space in its rebuilt log rather
    /// than minting a fresh one each boot. (A stable UUID formed from the actor's
    /// SHA-256, via the id contract's `FromStr`, so we take no direct `uuid` dep.)
    fn system_space_id(&self) -> SpaceId {
        let hex = sha256_hex(self.self_actor.as_str().as_bytes());
        let uuid = format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32],
        );
        SpaceId::from_str(&uuid).unwrap_or_else(|_| SpaceId::nil())
    }

    /// Find-or-create the node's system space: if the folded state already holds
    /// it (a prior boot's log rebuilt it), reuse it; otherwise bootstrap it by
    /// authoring `SpaceCreated` + `MemberJoined` (this node an Owner) so the folded
    /// alerts/clips land in a real member space whose ack/pin/snooze validate.
    /// Returns the id, or `None` if the (logged) bootstrap could not land.
    fn ensure_system_space(
        &self,
        persist: &Persist,
        state: &mut CollabState,
        now_ms: i64,
        touched: &mut BTreeSet<SpaceId>,
        changed: &mut bool,
    ) -> Option<SpaceId> {
        let system = self.system_space_id();
        if system.is_nil() {
            return None;
        }
        if state.engine.state().space(system).is_some() {
            return Some(system);
        }
        self.author_and_relay(
            persist,
            state,
            system,
            CollabEventKind::SpaceCreated {
                kind: SpaceKind::Team,
                name: format!("{SYSTEM_SPACE_NAME} \u{b7} {}", self.self_actor),
            },
            now_ms,
            touched,
            changed,
        );
        self.author_and_relay(
            persist,
            state,
            system,
            CollabEventKind::MemberJoined {
                actor: self.self_actor.clone(),
                role: SpaceRole::Owner,
            },
            now_ms,
            touched,
            changed,
        );
        state.engine.state().space(system).map(|_| system)
    }

    /// Author one worker-adapted event into `space`, then relay it exactly as a
    /// command-produced event is: durable-append to this node's own actor log
    /// BEFORE publishing, publish it live, and mark the space touched + changed.
    /// The shared tail of the fold + bootstrap paths.
    fn author_and_relay(
        &self,
        persist: &Persist,
        state: &mut CollabState,
        space: SpaceId,
        kind: CollabEventKind,
        now_ms: i64,
        touched: &mut BTreeSet<SpaceId>,
        changed: &mut bool,
    ) {
        let signer = Ed25519Signer::new(self.signing_key.clone());
        let env = match state
            .engine
            .author(space, kind, &signer, &mut state.ids, now_ms)
        {
            Ok(env) => env,
            Err(e) => {
                tracing::warn!(target: "mackesd::collab", error = %e, "collab author (fold) failed");
                return;
            }
        };
        match self.append_own(state, &env) {
            Ok(()) => {
                self.publish_event(persist, &env);
                if !env.space_id.is_nil() {
                    touched.insert(env.space_id);
                }
                *changed = true;
            }
            Err(e) => tracing::warn!(
                target: "mackesd::collab",
                error = %e,
                "actor-log append failed; not publishing folded event",
            ),
        }
    }

    /// Drain every alert lane (mirroring the chat worker's set) forward-only and
    /// fold each truthful alert body into an [`AlertRaised`](CollabEventKind)
    /// event in the node's system space (bootstrapped lazily on the first fold).
    /// Forward-only (seed-to-head on first sight) so a restart never re-raises a
    /// stale alert backlog.
    fn drain_alert_lanes(
        &self,
        persist: &Persist,
        state: &mut CollabState,
        now_ms: i64,
        touched: &mut BTreeSet<SpaceId>,
        changed: &mut bool,
    ) {
        let all_topics = match persist.list_topics() {
            Ok(topics) => topics,
            Err(error) => {
                tracing::warn!(target: "mackesd::collab", %error, "alert topic discovery failed; transient lanes left untouched");
                return;
            }
        };
        for topic in &all_topics {
            if !is_alert_lane(topic) {
                continue;
            }
            let messages = match take_new_forward(persist, &mut state.cursors, topic) {
                Ok(messages) => messages,
                Err(error) => {
                    tracing::warn!(target: "mackesd::collab", topic, %error, "alert lane read failed; cursor left unchanged");
                    continue;
                }
            };
            for m in messages {
                let Some(body) = m.body.as_deref() else {
                    continue;
                };
                if let Some(alert) = fold_alert_payload(topic, body, self.self_actor.as_str()) {
                    let Some(system) =
                        self.ensure_system_space(persist, state, now_ms, touched, changed)
                    else {
                        continue;
                    };
                    self.author_and_relay(
                        persist,
                        state,
                        system,
                        CollabEventKind::AlertRaised { alert },
                        now_ms,
                        touched,
                        changed,
                    );
                } else {
                    tracing::debug!(target: "mackesd::collab", topic = topic.as_str(), "alert body not foldable; skipped");
                }
            }
        }
    }

    /// Drain the cross-mesh clipboard-capture lane forward-only and fold each
    /// capture into a [`ClipboardPublished`](CollabEventKind) event in the node's
    /// system space (bootstrapped lazily on the first fold; the full content
    /// address is recomputed, and a >100 MB clip is skipped, not truncated — it
    /// belongs to the Transfers path).
    fn drain_clipboard_captures(
        &self,
        persist: &Persist,
        state: &mut CollabState,
        now_ms: i64,
        touched: &mut BTreeSet<SpaceId>,
        changed: &mut bool,
    ) {
        let messages = match take_new_forward(persist, &mut state.cursors, CLIPBOARD_CAPTURE_TOPIC)
        {
            Ok(messages) => messages,
            Err(error) => {
                tracing::warn!(target: "mackesd::collab", %error, "clipboard lane read failed; cursor left unchanged");
                return;
            }
        };
        for m in messages {
            let Some(body) = m.body.as_deref() else {
                continue;
            };
            if let Some(item) = fold_clip_item(body) {
                let Some(system) =
                    self.ensure_system_space(persist, state, now_ms, touched, changed)
                else {
                    continue;
                };
                self.author_and_relay(
                    persist,
                    state,
                    system,
                    CollabEventKind::ClipboardPublished { item },
                    now_ms,
                    touched,
                    changed,
                );
            }
        }
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
        // Read the complete fixed command-lane set against a temporary cursor
        // map first. If any Bus read fails, commit neither cursor movement nor
        // command effects: the next tick retries from the same known boundary.
        let mut next_cursors = state.cursors.clone();
        let mut pending = Vec::with_capacity(COMMAND_VERBS.len());
        for verb in COMMAND_VERBS {
            let topic = topics::command_topic(verb);
            let messages = match take_new_forward(persist, &mut next_cursors, &topic) {
                Ok(messages) => messages,
                Err(error) => {
                    tracing::warn!(target: "mackesd::collab", verb, %error, "command lane read failed; command sweep left untouched");
                    return;
                }
            };
            pending.push((*verb, messages));
        }
        state.cursors = next_cursors;

        for (verb, messages) in pending {
            for m in messages {
                let Some(body) = m.body.as_deref() else {
                    tracing::warn!(target: "mackesd::collab", verb, "action/collab command with empty body");
                    continue;
                };
                if let Err(error) = self.authorizer.authorize(
                    body,
                    MutationContext {
                        verb: COLLAB_AUTH_VERB,
                        node: self.self_actor.as_str(),
                        target: verb,
                    },
                ) {
                    tracing::warn!(
                        target: "mackesd::collab",
                        verb,
                        error = %error,
                        "refused unauthorized action/collab command",
                    );
                    continue;
                }
                let mut envelope: serde_json::Value = match serde_json::from_str(body) {
                    Ok(value) => value,
                    Err(e) => {
                        tracing::warn!(target: "mackesd::collab", verb, error = %e, "bad action/collab command body");
                        continue;
                    }
                };
                let Some(object) = envelope.as_object_mut() else {
                    tracing::warn!(target: "mackesd::collab", verb, "action/collab command envelope is not an object");
                    continue;
                };
                object.remove("armed_token");
                object.remove("schema_version");
                let cmd: CollabCommand = match serde_json::from_value(envelope) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(target: "mackesd::collab", verb, error = %e, "bad action/collab command body");
                        continue;
                    }
                };
                if cmd.verb() != verb {
                    tracing::warn!(
                        target: "mackesd::collab",
                        topic_verb = verb,
                        command_verb = cmd.verb(),
                        "refused action/collab command routed on the wrong verb lane",
                    );
                    continue;
                }
                let existing_call_kind = command_call_id(&cmd).and_then(|call| {
                    state
                        .engine
                        .projection()
                        .call_state(None)
                        .ok()?
                        .active
                        .into_iter()
                        .find(|active| active.call == call)
                        .map(|active| active.kind)
                });
                if let Err(error) = self
                    .call_media_providers
                    .admit_command(&cmd, existing_call_kind)
                {
                    tracing::warn!(
                        target: "mackesd::collab",
                        verb,
                        error = %error,
                        "refused call command without an admitted media provider",
                    );
                    continue;
                }
                // Execute the provider side effect before applying the command
                // to the collaboration engine. `engine.apply` advances the
                // in-memory projection; doing it first would leave call state
                // behind when a proof-only or unavailable provider refuses
                // execution.
                if let Err(error) = self
                    .call_media_providers
                    .execute_command(&cmd, existing_call_kind)
                {
                    tracing::warn!(
                        target: "mackesd::collab",
                        verb,
                        error = %error,
                        "refused call command before signed call-state mutation",
                    );
                    continue;
                }
                let events = match &cmd {
                    CollabCommand::LinkFile {
                        space,
                        file,
                        reference,
                    } => {
                        let payload = PayloadRef {
                            sha256_hex: reference.sha256_hex.clone(),
                            len: reference.size,
                            content_type: reference.mime.clone(),
                        };
                        let blobs = FsBlobStore::new(&self.content_root);
                        let bytes = match blobs.get(&payload) {
                            Ok(bytes) => bytes,
                            Err(error) => {
                                tracing::warn!(
                                    target: "mackesd::collab",
                                    verb,
                                    error = %error,
                                    "refused LinkFile without exact canonical payload",
                                );
                                continue;
                            }
                        };
                        match ingest_and_register_file(
                            &mut state.engine,
                            &blobs,
                            *space,
                            *file,
                            reference.clone(),
                            Cursor::new(bytes),
                            &signer,
                            &mut state.ids,
                            now_ms,
                        ) {
                            Ok(registered) => registered.events,
                            Err(error) => {
                                tracing::warn!(target: "mackesd::collab", verb, error = %error, "collab file registration denied");
                                continue;
                            }
                        }
                    }
                    _ => match state.engine.apply(&cmd, &signer, &mut state.ids, now_ms) {
                        Ok(events) => events,
                        Err(error) => {
                            tracing::warn!(target: "mackesd::collab", verb, error = %error, "collab command denied");
                            continue;
                        }
                    },
                };
                self.handle_ai_sidecar(&cmd, state, now_ms, touched, changed);
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
            let log = FileActorLog::open_append_only(&self.log_root, space, &self.self_actor)?;
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
        let all_topics = match persist.list_topics() {
            Ok(topics) => topics,
            Err(error) => {
                tracing::warn!(target: "mackesd::collab", %error, "collab event topic discovery failed; durable lanes left untouched");
                return;
            }
        };
        let mut incoming: Vec<CollabEventEnvelope> =
            Vec::with_capacity(MAX_WORKER_MERGE_BATCH_EVENTS);
        for topic in &all_topics {
            if !topic.starts_with(topics::EVENT_PREFIX) {
                continue;
            }
            let Some((topic_space, topic_actor)) = topics::parse_event_topic(topic) else {
                continue;
            };
            // Skip our own authored lane — those events are already ingested.
            if topic_actor == self.self_actor {
                continue;
            }
            // Events are idempotent under merge, so drain the full lane on first
            // sight (a foreign lane only appears once it carries events) and
            // forward thereafter.
            let messages = match take_new_all(persist, &mut state.cursors, topic) {
                Ok(messages) => messages,
                Err(error) => {
                    tracing::warn!(target: "mackesd::collab", topic, %error, "collab event lane read failed; cursor left unchanged");
                    continue;
                }
            };
            for m in messages {
                let Some(body) = m.body.as_deref() else {
                    continue;
                };
                if body.len() > MAX_ACTOR_LOG_LINE_BYTES {
                    tracing::warn!(
                        target: "mackesd::collab",
                        topic = topic.as_str(),
                        bytes = body.len(),
                        max_bytes = MAX_ACTOR_LOG_LINE_BYTES,
                        "skipping oversized collab/event envelope",
                    );
                    continue;
                }
                match serde_json::from_str::<CollabEventEnvelope>(body) {
                    Ok(env) if env.space_id == topic_space && env.actor == topic_actor => {
                        incoming.push(env);
                        if incoming.len() == MAX_WORKER_MERGE_BATCH_EVENTS {
                            self.merge_batch(
                                state,
                                std::mem::take(&mut incoming),
                                touched,
                                changed,
                                "bus",
                            );
                            incoming.reserve(MAX_WORKER_MERGE_BATCH_EVENTS);
                        }
                    }
                    Ok(env) => tracing::warn!(
                        target: "mackesd::collab",
                        topic = topic.as_str(),
                        topic_space = %topic_space,
                        topic_actor = %topic_actor,
                        envelope_space = %env.space_id,
                        envelope_actor = %env.actor,
                        "refused collab/event envelope routed on a mismatched identity lane",
                    ),
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

    /// Backfill from replicated actor logs in a bounded batch per tick.
    /// Byte offsets are carried across ticks. Candidate selection alternates
    /// between newest-modified first (live convergence) and oldest-modified
    /// first (fair historical recovery), so a fresh log cannot sit behind a
    /// fleet's many smaller retained logs and a continuously hot log cannot
    /// starve old history.
    /// A truncation resets that file to byte zero; an unterminated tail is left
    /// unread until Syncthing delivers its newline.
    fn backfill_logs(
        &self,
        state: &mut CollabState,
        touched: &mut BTreeSet<SpaceId>,
        changed: &mut bool,
    ) {
        let mut paths = collect_log_files(&self.log_root);
        sort_backfill_candidates(&mut paths, state.prefer_recent_log_backfill);
        let mut progressed = 0usize;
        let mut selected = 0usize;
        for path in paths {
            if selected == LOG_BACKFILL_SLICES_PER_TICK {
                break;
            }
            let len = std::fs::metadata(&path).map_or(0, |metadata| metadata.len());
            let mut offset = state.log_offsets.get(&path).copied().unwrap_or(0);
            if offset > len {
                offset = 0;
            }
            let complete_end = complete_log_end(&path);
            let live_offset = state.log_live_offsets.get(&path).copied();
            if offset == len && live_offset == Some(complete_end) {
                continue;
            }
            selected += 1;

            // The sequential lane below preserves bounded full-history recovery,
            // but a multi-megabyte actor log can remain behind that cursor for
            // minutes. Fold newly appended complete records from the live edge
            // first so Teams and clipboard actions are observable immediately.
            // If more than one slice arrived at once, also fold the newest slice;
            // the sequential lane will recover the skipped middle idempotently.
            if live_offset != Some(complete_end) {
                let (incoming, next_live_offset) = match live_offset {
                    Some(previous) if previous <= complete_end => {
                        let (incoming, next) =
                            read_log_envelope_chunk(&path, previous, LOG_BACKFILL_SLICE_EVENTS);
                        self.merge_batch(state, incoming, touched, changed, "log-live");
                        if next < complete_end {
                            read_log_tail_envelope_chunk(&path, LOG_BACKFILL_SLICE_EVENTS)
                        } else {
                            (Vec::new(), next)
                        }
                    }
                    _ => read_log_tail_envelope_chunk(&path, LOG_BACKFILL_SLICE_EVENTS),
                };
                self.merge_batch(state, incoming, touched, changed, "log-live-tail");
                state
                    .log_live_offsets
                    .insert(path.clone(), next_live_offset);
                progressed += 1;
            }

            if offset == len {
                continue;
            }
            let (incoming, next_offset) =
                read_log_envelope_chunk(&path, offset, LOG_BACKFILL_SLICE_EVENTS);
            self.merge_batch(state, incoming, touched, changed, "log");
            if next_offset != offset {
                state.log_offsets.insert(path, next_offset);
                progressed += 1;
            }
        }
        if progressed > 0 {
            state.prefer_recent_log_backfill = !state.prefer_recent_log_backfill;
        }
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
        for chunk in incoming.chunks(MAX_WORKER_MERGE_BATCH_EVENTS) {
            match state.engine.merge(chunk.to_vec()) {
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
                    tracing::warn!(
                        target: "mackesd::collab",
                        source,
                        error = %e,
                        "collab merge failed",
                    );
                    break;
                }
            }
        }
    }

    /// Publish one signed event live on `collab/event/<space>/<self>`.
    fn publish_event(&self, persist: &Persist, env: &CollabEventEnvelope) {
        let topic = topics::event_topic(env.space_id, &self.self_actor);
        match serde_json::to_string(env) {
            Ok(body) => {
                publish(persist, &topic, &body);
            }
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
            if !publish_state(
                persist,
                &mut state.last_published,
                &topics::space_state_topic(proj::FILE_REFERENCES, space),
                files,
            ) {
                state.pending_file_projection_spaces.insert(space);
            }
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
            let media_readiness = state
                .engine
                .projection()
                .call_media_readiness(&self.self_actor, Some(space));
            publish_state(
                persist,
                &mut state.last_published,
                &topics::space_state_topic(proj::CALL_MEDIA_READINESS, space),
                media_readiness,
            );
            let ai_requests = Ok(state.ai_requests.for_space(space));
            publish_state(
                persist,
                &mut state.last_published,
                &topics::space_state_topic(proj::AI_REQUESTS, space),
                ai_requests,
            );
        }

        if !changed {
            // Provider health changes independently of signed collaboration
            // state. Re-probe the retained readiness board on every worker
            // tick so a revoked/disconnected provider cannot leave a stale
            // LiveMediaVerified row behind, and a recovered provider can
            // become usable without manufacturing a call-state mutation.
            super::collab_media::publish_retained_call_media_verification(
                persist,
                &mut state.last_published,
                &self.call_media_providers,
            );
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
        let media_readiness = state
            .engine
            .projection()
            .call_media_readiness(&self.self_actor, None);
        let media_readiness_available = publish_state(
            persist,
            &mut state.last_published,
            &topics::state_topic(proj::CALL_MEDIA_READINESS),
            media_readiness,
        );
        if media_readiness_available {
            super::collab_media::publish_retained_call_media_verification(
                persist,
                &mut state.last_published,
                &self.call_media_providers,
            );
        }
        let ai_requests = Ok(state.ai_requests.all());
        publish_state(
            persist,
            &mut state.last_published,
            &topics::state_topic(proj::AI_REQUESTS),
            ai_requests,
        );
    }

    /// Apply worker-owned AI request/cancel sidecar state after core admission.
    ///
    /// The core validates space membership, request-id shape, and target scoping.
    /// This sidecar deliberately does **not** call DigitalOcean yet; it publishes
    /// bounded request state so the UI can show fail-closed consent/provider
    /// status and so the future sealed-key adapter has a cancellable request id.
    fn handle_ai_sidecar(
        &self,
        cmd: &CollabCommand,
        state: &mut CollabState,
        now_ms: i64,
        touched: &mut BTreeSet<SpaceId>,
        changed: &mut bool,
    ) {
        match cmd {
            CollabCommand::RequestAiSuggestion {
                space,
                request_id,
                target,
                kind,
            } => {
                state.ai_requests.request(AiRequestAdmission {
                    space: *space,
                    request_id,
                    actor: &self.self_actor,
                    kind: *kind,
                    target: *target,
                    now_ms,
                    cloud_consent: self.ai_cloud_consent,
                });
                touched.insert(*space);
                *changed = true;
            }
            CollabCommand::CancelAiSuggestion { space, request_id } => {
                if state.ai_requests.cancel(*space, request_id, now_ms) {
                    touched.insert(*space);
                    *changed = true;
                }
            }
            _ => {}
        }
    }
}

fn command_call_id(command: &CollabCommand) -> Option<mde_collab_types::CallId> {
    match command {
        CollabCommand::AnswerCall { call }
        | CollabCommand::DeclineCall { call }
        | CollabCommand::HangUpCall { call }
        | CollabCommand::SendDtmf { call, .. }
        | CollabCommand::SetCallMuted { call, .. } => Some(*call),
        _ => None,
    }
}

fn admit_inbound_sip_identity(
    domain: &mde_collab_core::DomainState,
    local_actor: &ActorId,
    offer: &super::collab_media::InboundCallOffer,
) -> Result<SpaceId, &'static str> {
    if offer.provider_call_id.is_empty()
        || offer.provider_call_id.len() > 255
        || offer
            .provider_call_id
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err("malformed provider call identity");
    }
    let identity = normalize_sip_identity(&offer.identity).ok_or("malformed SIP identity")?;
    let mut matched = None;
    for (space, aggregate) in &domain.spaces {
        if aggregate.deleted
            || !domain.is_member(*space, local_actor)
            || !aggregate.members.iter().any(|(actor, member)| {
                member.present
                    && actor != local_actor
                    && normalize_sip_identity(actor.as_str()).as_deref() == Some(identity.as_str())
            })
        {
            continue;
        }
        if matched.replace(*space).is_some() {
            return Err("ambiguous SIP identity authority");
        }
    }
    matched.ok_or("SIP identity has no authorized Collaboration space")
}

/// Canonicalize the provider's identity into the same comparison form used for
/// Collaboration actors. Display names, URI parameters, passwords, controls,
/// and non-ASCII lookalikes are deliberately not accepted as authority.
fn normalize_sip_identity(raw: &str) -> Option<String> {
    let mut value = raw.trim();
    if value.len() > 253 || value.is_empty() || !value.is_ascii() {
        return None;
    }
    if value.starts_with('<') && value.ends_with('>') {
        value = &value[1..value.len() - 1];
    } else if value.contains(['<', '>']) {
        return None;
    }
    if value
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("sip:"))
    {
        value = &value[4..];
    } else if value
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("sips:"))
    {
        value = &value[5..];
    }
    if value.is_empty()
        || value.contains([';', '?', ':'])
        || value.bytes().any(|byte| {
            !(byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+' | b'@'))
        })
        || value.starts_with('@')
        || value.ends_with('@')
        || value.matches('@').count() > 1
    {
        return None;
    }
    Some(value.to_ascii_lowercase())
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
    /// Next complete JSONL-record byte offset for each replicated actor log.
    log_offsets: BTreeMap<PathBuf, u64>,
    /// Complete-record edge already folded through the low-latency live lane.
    /// Full history still advances independently through `log_offsets`.
    log_live_offsets: BTreeMap<PathBuf, u64>,
    /// Alternates recent delivery with fair retained-history recovery.
    prefer_recent_log_backfill: bool,
    /// The last published body per `state/collab/*` topic — skip republishing an
    /// identical read model (latest-wins churn guard).
    last_published: BTreeMap<String, String>,
    /// Spaces whose canonical Files projection failed to publish. The signed
    /// actor log and folded engine are already authoritative, so later ticks
    /// retry only the derived retained view until it catches up.
    pending_file_projection_spaces: BTreeSet<SpaceId>,
    /// Worker-owned DigitalOcean AI request/cancel sidecar state.
    ai_requests: AiRequestBoard,
}

impl CollabState {
    /// A fresh per-run state for `actor`, with an in-memory SQLite projection.
    fn new(actor: ActorId) -> mde_collab_core::Result<Self> {
        Ok(Self {
            engine: CollabEngine::new(actor, Projection::open_in_memory()?),
            ids: RandomIds,
            cursors: BTreeMap::new(),
            own_logs: BTreeMap::new(),
            log_offsets: BTreeMap::new(),
            log_live_offsets: BTreeMap::new(),
            prefer_recent_log_backfill: true,
            last_published: BTreeMap::new(),
            pending_file_projection_spaces: BTreeSet::new(),
            ai_requests: AiRequestBoard::default(),
        })
    }
}

/// Arguments for admitting a DigitalOcean AI sidecar request.
struct AiRequestAdmission<'a> {
    space: SpaceId,
    request_id: &'a str,
    actor: &'a ActorId,
    kind: mde_collab_types::AiSuggestionKind,
    target: Option<mde_collab_types::EventId>,
    now_ms: i64,
    cloud_consent: bool,
}

/// Bounded in-memory state for the worker-owned AI request sidecar.
#[derive(Default)]
struct AiRequestBoard {
    rows: BTreeMap<(SpaceId, String), AiSuggestionRequestView>,
}

impl AiRequestBoard {
    /// Admit a request into the sidecar, fail-closed when cloud consent is absent.
    fn request(&mut self, admission: AiRequestAdmission<'_>) {
        let (status, error) = if admission.cloud_consent {
            (AiSuggestionRequestStatus::Pending, None)
        } else {
            (
                AiSuggestionRequestStatus::Failed,
                Some(AI_CONSENT_REQUIRED.to_string()),
            )
        };
        let view = AiSuggestionRequestView {
            request_id: admission.request_id.to_string(),
            space: admission.space,
            requested_by: admission.actor.clone(),
            kind: admission.kind,
            target: admission.target,
            status,
            provider: AI_PROVIDER_DIGITALOCEAN.to_string(),
            model: None,
            error,
            updated_unix_ms: admission.now_ms,
        };
        self.rows
            .insert((admission.space, admission.request_id.to_string()), view);
        self.prune();
    }

    /// Cancel a pending request. Failed/offered/unknown rows are left unchanged.
    fn cancel(&mut self, space: SpaceId, request_id: &str, now_ms: i64) -> bool {
        let Some(row) = self.rows.get_mut(&(space, request_id.to_string())) else {
            return false;
        };
        if row.status != AiSuggestionRequestStatus::Pending {
            return false;
        }
        row.status = AiSuggestionRequestStatus::Canceled;
        row.error = None;
        row.updated_unix_ms = now_ms;
        true
    }

    /// The per-space request board.
    fn for_space(&self, space: SpaceId) -> AiSuggestionRequests {
        self.model(self.rows.values().filter(|row| row.space == space))
    }

    /// The fleet-wide request board.
    fn all(&self) -> AiSuggestionRequests {
        self.model(self.rows.values())
    }

    fn model<'a>(
        &self,
        rows: impl IntoIterator<Item = &'a AiSuggestionRequestView>,
    ) -> AiSuggestionRequests {
        let mut requests: Vec<AiSuggestionRequestView> = rows.into_iter().cloned().collect();
        requests.sort_by(|a, b| {
            (
                a.updated_unix_ms,
                a.space,
                a.request_id.as_str(),
                a.requested_by.as_str(),
            )
                .cmp(&(
                    b.updated_unix_ms,
                    b.space,
                    b.request_id.as_str(),
                    b.requested_by.as_str(),
                ))
        });
        AiSuggestionRequests { requests }
    }

    fn prune(&mut self) {
        while self.rows.len() > MAX_AI_REQUEST_ROWS {
            let Some(oldest) = self
                .rows
                .iter()
                .min_by(|(key_a, row_a), (key_b, row_b)| {
                    (row_a.updated_unix_ms, key_a).cmp(&(row_b.updated_unix_ms, key_b))
                })
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.rows.remove(&oldest);
        }
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
) -> Result<Vec<StoredMessage>, String> {
    match cursors.get(topic) {
        None => {
            let head = persist
                .latest_ulid(topic)
                .map_err(|error| error.to_string())?;
            cursors.insert(topic.to_string(), head);
            Ok(Vec::new())
        }
        Some(cur) => {
            let cur = cur.clone();
            let msgs = persist
                .list_since(topic, cur.as_deref())
                .map_err(|error| error.to_string())?;
            if let Some(last) = msgs.last() {
                cursors.insert(topic.to_string(), Some(last.ulid.clone()));
            }
            Ok(msgs)
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
) -> Result<Vec<StoredMessage>, String> {
    let since = cursors.get(topic).cloned().flatten();
    let msgs = persist
        .list_since(topic, since.as_deref())
        .map_err(|error| error.to_string())?;
    if let Some(last) = msgs.last() {
        cursors.insert(topic.to_string(), Some(last.ulid.clone()));
    } else {
        cursors.entry(topic.to_string()).or_insert(None);
    }
    Ok(msgs)
}

/// Prime every forward-only transient lane at its current tail as one
/// activation transaction. Durable `collab/event/*` lanes are deliberately not
/// included: they must replay retained signed history into the projection.
fn prime_transient_cursors(persist: &Persist) -> Result<BTreeMap<String, Option<String>>, String> {
    let mut cursors = BTreeMap::new();
    for verb in COMMAND_VERBS {
        let topic = topics::command_topic(verb);
        let head = persist
            .latest_ulid(&topic)
            .map_err(|error| format!("prime {topic}: {error}"))?;
        cursors.insert(topic, head);
    }

    let clipboard_head = persist
        .latest_ulid(CLIPBOARD_CAPTURE_TOPIC)
        .map_err(|error| format!("prime {CLIPBOARD_CAPTURE_TOPIC}: {error}"))?;
    cursors.insert(CLIPBOARD_CAPTURE_TOPIC.to_string(), clipboard_head);

    let all_topics = persist
        .list_topics()
        .map_err(|error| format!("discover transient lanes: {error}"))?;
    for topic in all_topics.into_iter().filter(|topic| is_alert_lane(topic)) {
        let head = persist
            .latest_ulid(&topic)
            .map_err(|error| format!("prime {topic}: {error}"))?;
        cursors.insert(topic, head);
    }
    Ok(cursors)
}

/// Serialize + publish a read model, skipping the write when the body is
/// byte-identical to what was last published on the topic (latest-wins). A model
/// the projection could not build is logged at debug + skipped, never faked.
fn publish_state<T: serde::Serialize>(
    persist: &Persist,
    last_published: &mut BTreeMap<String, String>,
    topic: &str,
    model: mde_collab_core::Result<T>,
) -> bool {
    match model {
        Ok(m) => match serde_json::to_string(&m) {
            Ok(body) => {
                if last_published.get(topic).map(String::as_str) == Some(body.as_str()) {
                    return true;
                }
                if publish(persist, topic, &body) {
                    last_published.insert(topic.to_string(), body);
                    true
                } else {
                    false
                }
            }
            Err(e) => {
                tracing::warn!(target: "mackesd::collab", topic, error = %e, "serialize read model failed");
                false
            }
        },
        Err(e) => {
            tracing::debug!(target: "mackesd::collab", topic, error = %e, "read model unavailable");
            false
        }
    }
}

/// In-process Bus publish (best-effort). Writing to the local Persist store is the
/// same store the broker + surface read; whether it federates to peers is the
/// broker's job (the live multi-node reach is integration-gated).
fn publish(persist: &Persist, topic: &str, body: &str) -> bool {
    if let Err(e) = persist.write(topic, Priority::Default, None, Some(body)) {
        tracing::debug!(target: "mackesd::collab", topic, error = %e, "collab publish failed");
        false
    } else {
        true
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
        if !space_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&space_dir) else {
            continue;
        };
        for file in files.flatten() {
            let p = file.path();
            if file.file_type().map(|t| t.is_file()).unwrap_or(false)
                && p.extension().and_then(|e| e.to_str()) == Some("jsonl")
            {
                out.push(p);
            }
        }
    }
    out
}

fn sort_backfill_candidates(paths: &mut [PathBuf], prefer_recent: bool) {
    paths.sort_by(|left, right| {
        let left_metadata = std::fs::metadata(left).ok();
        let right_metadata = std::fs::metadata(right).ok();
        let left_modified = left_metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let right_modified = right_metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let modified_order = if prefer_recent {
            right_modified.cmp(&left_modified)
        } else {
            left_modified.cmp(&right_modified)
        };
        modified_order
            .then_with(|| {
                left_metadata
                    .as_ref()
                    .map_or(u64::MAX, std::fs::Metadata::len)
                    .cmp(
                        &right_metadata
                            .as_ref()
                            .map_or(u64::MAX, std::fs::Metadata::len),
                    )
            })
            .then_with(|| left.cmp(right))
    });
}

/// Byte offset immediately after the final newline-terminated record.
fn complete_log_end(path: &Path) -> u64 {
    const SCAN_BLOCK_BYTES: usize = 64 * 1024;

    let Ok(mut file) = File::open(path) else {
        return 0;
    };
    let len = file.metadata().map_or(0, |metadata| metadata.len());
    let mut cursor = len;
    while cursor > 0 {
        let start = cursor.saturating_sub(SCAN_BLOCK_BYTES as u64);
        let Ok(block_len) = usize::try_from(cursor - start) else {
            return 0;
        };
        let mut block = vec![0_u8; block_len];
        if file.seek(SeekFrom::Start(start)).is_err() || file.read_exact(&mut block).is_err() {
            return 0;
        }
        if let Some(index) = block.iter().rposition(|byte| *byte == b'\n') {
            return start + index as u64 + 1;
        }
        cursor = start;
    }
    0
}

/// Read the newest bounded set of complete records without scanning an entire
/// multi-megabyte actor log. The scan window is sized for `max_records` records
/// at the hard per-line limit, so memory and I/O remain bounded.
fn read_log_tail_envelope_chunk(
    path: &Path,
    max_records: usize,
) -> (Vec<CollabEventEnvelope>, u64) {
    let complete_end = complete_log_end(path);
    if complete_end == 0 || max_records == 0 {
        return (Vec::new(), complete_end);
    }
    let max_scan = (MAX_ACTOR_LOG_LINE_BYTES + 1).saturating_mul(max_records.saturating_add(1));
    let scan_start = complete_end.saturating_sub(max_scan as u64);
    let Ok(scan_len) = usize::try_from(complete_end - scan_start) else {
        return (Vec::new(), complete_end);
    };
    let Ok(mut file) = File::open(path) else {
        return (Vec::new(), complete_end);
    };
    if file.seek(SeekFrom::Start(scan_start)).is_err() {
        return (Vec::new(), complete_end);
    }
    let mut bytes = vec![0_u8; scan_len];
    if file.read_exact(&mut bytes).is_err() {
        return (Vec::new(), complete_end);
    }
    let newline_offsets = bytes
        .iter()
        .enumerate()
        .filter_map(|(index, byte)| (*byte == b'\n').then_some(index))
        .collect::<Vec<_>>();
    let start = if newline_offsets.len() > max_records {
        scan_start + newline_offsets[newline_offsets.len() - max_records - 1] as u64 + 1
    } else if scan_start == 0 {
        0
    } else {
        // The bounded scan may begin in the middle of an oversized record.
        // Skip that fragment; the sequential lane will diagnose it later.
        newline_offsets
            .first()
            .map_or(complete_end, |index| scan_start + *index as u64 + 1)
    };
    let (incoming, next_offset) = read_log_envelope_chunk(path, start, max_records);
    (incoming, next_offset.min(complete_end))
}

/// Read at most `max_records` complete JSONL records starting at `offset`.
/// Returns valid envelopes plus the next complete-record boundary.
fn read_log_envelope_chunk(
    path: &Path,
    offset: u64,
    max_records: usize,
) -> (Vec<CollabEventEnvelope>, u64) {
    let Ok(mut file) = File::open(path) else {
        return (Vec::new(), offset);
    };
    let len = file.metadata().map_or(0, |metadata| metadata.len());
    let start = offset.min(len);
    let terminated = if len == 0 {
        true
    } else if file.seek(SeekFrom::End(-1)).is_ok() {
        let mut tail = [0_u8; 1];
        file.read_exact(&mut tail).is_ok() && tail[0] == b'\n'
    } else {
        false
    };
    if file.seek(SeekFrom::Start(start)).is_err() {
        return (Vec::new(), start);
    }
    let mut reader = BufReader::new(file);
    let mut batch = Vec::with_capacity(max_records.min(MAX_WORKER_MERGE_BATCH_EVENTS));
    let mut next_offset = start;
    let mut records = 0_usize;
    while records < max_records {
        let line_start = reader.stream_position().unwrap_or(next_offset);
        match read_bounded_log_line(&mut reader, MAX_ACTOR_LOG_LINE_BYTES) {
            Ok(Some(BoundedLogLine::Line(line))) => {
                let line_end = reader.stream_position().unwrap_or(line_start);
                if line_end == len && !terminated {
                    break;
                }
                records = records.saturating_add(1);
                next_offset = line_end;
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                match serde_json::from_slice::<CollabEventEnvelope>(&line) {
                    Ok(env) => {
                        batch.push(env);
                    }
                    Err(e) => tracing::warn!(
                        target: "mackesd::collab",
                        path = %path.display(),
                        offset = line_start,
                        error = %e,
                        "skipping malformed actor-log line",
                    ),
                }
            }
            Ok(Some(BoundedLogLine::OverLimit)) => {
                let line_end = reader.stream_position().unwrap_or(line_start);
                if line_end == len && !terminated {
                    break;
                }
                records = records.saturating_add(1);
                next_offset = line_end;
                tracing::warn!(
                    target: "mackesd::collab",
                    path = %path.display(),
                    offset = line_start,
                    max_bytes = MAX_ACTOR_LOG_LINE_BYTES,
                    "skipping oversized actor-log line",
                );
            }
            Ok(None) => break,
            Err(e) => {
                tracing::warn!(
                    target: "mackesd::collab",
                    path = %path.display(),
                    error = %e,
                    "actor-log read failed",
                );
                break;
            }
        }
    }
    (batch, next_offset)
}

#[cfg(test)]
fn read_log_envelope_chunks(path: &Path, mut on_chunk: impl FnMut(Vec<CollabEventEnvelope>)) {
    let mut offset = 0_u64;
    loop {
        let (batch, next_offset) =
            read_log_envelope_chunk(path, offset, MAX_WORKER_MERGE_BATCH_EVENTS);
        if !batch.is_empty() {
            on_chunk(batch);
        }
        if next_offset == offset {
            break;
        }
        offset = next_offset;
    }
}

enum BoundedLogLine {
    Line(Vec<u8>),
    OverLimit,
}

fn read_bounded_log_line<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
) -> std::io::Result<Option<BoundedLogLine>> {
    let mut line = Vec::new();
    let mut over_limit = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if line.is_empty() && !over_limit {
                return Ok(None);
            }
            if over_limit {
                return Ok(Some(BoundedLogLine::OverLimit));
            }
            return Ok(Some(BoundedLogLine::Line(line)));
        }

        let newline_at = available.iter().position(|b| *b == b'\n');
        let consume = newline_at.map_or(available.len(), |pos| pos + 1);
        if !over_limit {
            let next_len = line.len().saturating_add(consume);
            if next_len > max_bytes {
                over_limit = true;
            } else {
                line.extend_from_slice(&available[..consume]);
            }
        }
        reader.consume(consume);

        if newline_at.is_some() {
            if over_limit {
                return Ok(Some(BoundedLogLine::OverLimit));
            }
            if line.ends_with(b"\n") {
                line.pop();
            }
            if line.ends_with(b"\r") {
                line.pop();
            }
            return Ok(Some(BoundedLogLine::Line(line)));
        }
    }
}

/// Whether `topic` is a truthful alert lane the collab worker folds — one of the
/// [`ALERT_LANE_PREFIXES`], and not one of the collab suite's own lanes (so a
/// republished `state/collab/*` model or a `collab/event/*` delivery is never
/// mistaken for an alert to re-raise).
fn is_alert_lane(topic: &str) -> bool {
    if topic.starts_with(topics::ACTION_PREFIX)
        || topic.starts_with(topics::STATE_PREFIX)
        || topic.starts_with(topics::EVENT_PREFIX)
    {
        return false;
    }
    ALERT_LANE_PREFIXES.iter().any(|p| topic.starts_with(p))
}

/// Fold a truthful Bus alert body into an [`AlertPayload`], mirroring `mde-chat`'s
/// `fold_alert`: read `severity`/`priority` loosely, prefer a
/// `summary`/`headline`/`title`/`alert`/`message` line for the headline (else the
/// topic-derived flag), attribute the `source` (explicit `source`, else
/// `host`/`hostname`, else this node), copy the remaining string fields verbatim,
/// and map any typed `actions` array. A non-object body is not an alert we fold.
fn fold_alert_payload(topic: &str, body: &str, origin: &str) -> Option<AlertPayload> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let obj = value.as_object()?;
    let str_field = |k: &str| obj.get(k).and_then(serde_json::Value::as_str);

    let severity = str_field("severity")
        .or_else(|| str_field("priority"))
        .map_or(Severity::Info, classify_severity);
    let source = str_field("source")
        .or_else(|| str_field("host"))
        .or_else(|| str_field("hostname"))
        .unwrap_or(origin)
        .to_owned();
    let headline = ["summary", "headline", "title", "alert", "message"]
        .iter()
        .find_map(|k| str_field(k).filter(|s| !s.is_empty()))
        .map_or_else(|| alert_flag(topic).to_owned(), str::to_owned);

    let mut fields = BTreeMap::new();
    for (k, v) in obj {
        if matches!(
            k.as_str(),
            "severity"
                | "priority"
                | "source"
                | "summary"
                | "headline"
                | "title"
                | "alert"
                | "message"
                | "actions"
                | "goto"
                | "ts_unix_ms"
                | "host"
                | "hostname"
        ) {
            continue;
        }
        if let Some(s) = v.as_str() {
            fields.insert(k.clone(), s.to_owned());
        }
    }

    let actions = obj
        .get("actions")
        .and_then(serde_json::Value::as_array)
        .map(|arr| arr.iter().filter_map(fold_alert_action).collect())
        .unwrap_or_default();
    let goto = str_field("goto").map(str::to_owned);

    Some(AlertPayload {
        severity,
        source,
        headline,
        fields,
        actions,
        goto,
    })
}

/// Fold one typed inline alert action object (`{ id, label, verb, kind }`).
fn fold_alert_action(value: &serde_json::Value) -> Option<AlertAction> {
    let obj = value.as_object()?;
    let id = obj
        .get("id")
        .and_then(serde_json::Value::as_str)?
        .to_owned();
    let label = obj
        .get("label")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&id)
        .to_owned();
    let verb = obj
        .get("verb")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let kind = obj
        .get("kind")
        .or_else(|| obj.get("type"))
        .and_then(serde_json::Value::as_str)
        .map_or(AlertActionKind::Safe, classify_action_kind);
    Some(AlertAction {
        id,
        label,
        verb,
        kind,
    })
}

/// Classify a loose severity string into the collab [`Severity`] band (mirrors
/// the chat classifier's tokens).
fn classify_severity(s: &str) -> Severity {
    match s.trim().to_ascii_lowercase().as_str() {
        "crit" | "critical" | "error" | "err" | "fatal" | "urgent" => Severity::Critical,
        "warn" | "warning" | "high" => Severity::Warning,
        _ => Severity::Info,
    }
}

/// Classify a loose action-kind string into an [`AlertActionKind`].
fn classify_action_kind(s: &str) -> AlertActionKind {
    match s.trim().to_ascii_lowercase().as_str() {
        "destructive" | "danger" | "armed" => AlertActionKind::Destructive,
        "ack" | "acknowledge" => AlertActionKind::Ack,
        "snooze" => AlertActionKind::Snooze,
        _ => AlertActionKind::Safe,
    }
}

/// A coarse category flag for an alert topic (the headline fallback), mirroring
/// the chat worker's `alert_flag` families.
fn alert_flag(topic: &str) -> &'static str {
    if topic.starts_with("event/security/") || topic.starts_with("fleet/sec") {
        "security"
    } else if topic.starts_with("event/firewall") {
        "firewall"
    } else if topic.starts_with("fleet/health/") {
        "health"
    } else if topic.starts_with("event/notify/") {
        "notify"
    } else if topic.starts_with("fdo/") {
        "desktop"
    } else if topic.starts_with("event/compute/")
        || topic.starts_with("event/kvm/")
        || topic.starts_with("event/dc/")
    {
        "compute"
    } else {
        "system"
    }
}

/// Fold a cross-mesh clipboard capture body (`{ id, text, source, time }`) into a
/// [`ClipboardItem`]: recompute the full SHA-256 content address (the capture
/// carries only a 16-hex prefix), compute the byte length, detect a URI vs. text,
/// and carry the source. A clip over [`MAX_CLIP_BYTES`] is **not** folded (it is a
/// Transfer, never truncated into the lane).
fn fold_clip_item(body: &str) -> Option<ClipboardItem> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let obj = value.as_object()?;
    let text = obj.get("text").and_then(serde_json::Value::as_str)?;
    let len = text.len() as u64;
    if !clip_fits_lane(len) {
        tracing::info!(
            target: "mackesd::collab",
            len,
            "clip exceeds the clipboard-lane cap; it belongs to Transfers, not folded",
        );
        return None;
    }
    let source = obj
        .get("source")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let trimmed = text.trim_start();
    let kind = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        ClipItemKind::Uri
    } else {
        ClipItemKind::Text
    };
    Some(ClipboardItem {
        kind,
        preview: clip_preview(text),
        sha256_hex: sha256_hex(text.as_bytes()),
        len,
        source,
    })
}

/// Whether a clip of `len` bytes rides the clipboard lane (≤ [`MAX_CLIP_BYTES`]);
/// a larger clip is a Transfer instead. A pure boundary seam so the size gate is
/// testable without allocating a 100 MB fixture.
const fn clip_fits_lane(len: u64) -> bool {
    len <= MAX_CLIP_BYTES
}

/// A capped, single-line preview of clip content for the lane row (never the full
/// possibly-large payload).
fn clip_preview(text: &str) -> String {
    const PREVIEW_MAX: usize = 160;
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > PREVIEW_MAX {
        let head: String = one_line.chars().take(PREVIEW_MAX).collect();
        format!("{head}\u{2026}")
    } else {
        one_line
    }
}

fn collab_bus_root(override_root: Option<PathBuf>) -> PathBuf {
    collab_bus_root_or_system(override_root.or_else(mde_bus::default_data_dir))
}

fn collab_bus_root_or_system(resolved: Option<PathBuf>) -> PathBuf {
    resolved.unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
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
        let retry_interval = self
            .poll_interval
            .clamp(MIN_BUS_RETRY_INTERVAL, MAX_BUS_RETRY_INTERVAL);
        let (persist, mut state) = loop {
            match self.open_bus() {
                Ok(Some(persist)) => match self.prime_transient_cursors(&persist) {
                    Ok(cursors) => match CollabState::new(self.self_actor.clone()) {
                        Ok(mut state) => {
                            state.cursors = cursors;
                            break (persist, state);
                        }
                        Err(error) => tracing::warn!(
                            target: "mackesd::collab",
                            %error,
                            "projection activation failed; collab startup will retry"
                        ),
                    },
                    Err(error) => tracing::warn!(
                        target: "mackesd::collab",
                        %error,
                        "transient cursor priming failed; collab startup will retry"
                    ),
                },
                Ok(None) => tracing::debug!(
                    target: "mackesd::collab",
                    "Bus root unavailable; collab startup will retry"
                ),
                Err(error) => tracing::warn!(
                    target: "mackesd::collab",
                    %error,
                    "Persist open failed; collab startup will retry"
                ),
            }

            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(retry_interval) => {}
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
        // A slow retained-history pass must not manufacture a permanent busy
        // loop by replaying every interval that elapsed while it was running.
        // Delay gives the runtime and DRM shell a full poll interval between
        // passes while the independent live-tail lane keeps fresh work visible.
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
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
    use crate::ipc::action_auth::authorize_test_body;
    use ed25519_dalek::SigningKey;
    use mde_collab_types::read_model::{
        CallMediaAdapter, CallMediaAdmission, CallMediaFrameEvidence, CallMediaVerification,
        CallMediaVerificationStatus,
    };
    use mde_collab_types::value::{CallKind, MessageBody};
    use mde_collab_types::{
        AlertInbox, CallId, CallMediaReadiness, ClipboardLane, CollabEventKind,
        ConversationTimeline, FileRef, FileRefId, FileReferences, PresenceState, SpaceDirectory,
        SpaceKind, SpaceRole, TransferControl, TransferDirection, TransferId, TransferJobs,
        TransferMethod, TransferState,
    };
    use rand::rngs::OsRng;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    const AUTH_KEY: &[u8] = b"collab-action-auth-test-key";
    const AUTH_NOW: i64 = 1_700_000_000_000;
    static NEXT_NONCE: AtomicU64 = AtomicU64::new(1);

    fn key() -> SigningKey {
        SigningKey::generate(&mut OsRng)
    }

    fn worker(root: &Path, actor: &str) -> CollabWorker {
        CollabWorker::new(root.to_path_buf(), actor.into(), key())
            .with_bus_root(root.join("bus"))
            .with_log_root(root.join("collab-logs"))
            .with_authorizer(Arc::new(ActionAuthorizer::for_test(
                AUTH_KEY,
                root.join("auth-ledger"),
                AUTH_NOW,
            )))
    }

    fn persist_at(root: &Path) -> Persist {
        Persist::open(root.join("bus")).expect("open persist")
    }

    fn authorized_command_body_for_topic(
        w: &CollabWorker,
        topic_verb: &str,
        cmd: &CollabCommand,
        nonce: &str,
    ) -> String {
        let mut unsigned = serde_json::to_value(cmd).expect("serialize command");
        unsigned["schema_version"] = serde_json::Value::from(1_u64);
        let unsigned = serde_json::to_string(&unsigned).expect("serialize command envelope");
        authorize_test_body(
            AUTH_KEY,
            &unsigned,
            MutationContext {
                verb: COLLAB_AUTH_VERB,
                node: w.self_actor.as_str(),
                target: topic_verb,
            },
            nonce,
            AUTH_NOW + 30_000,
        )
    }

    fn authorized_command_body(w: &CollabWorker, cmd: &CollabCommand, nonce: &str) -> String {
        authorized_command_body_for_topic(w, cmd.verb(), cmd, nonce)
    }

    fn write_command(w: &CollabWorker, persist: &Persist, cmd: &CollabCommand) {
        let nonce = format!(
            "collab-test-{:032x}",
            NEXT_NONCE.fetch_add(1, Ordering::Relaxed)
        );
        let body = authorized_command_body(w, cmd, &nonce);
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

    fn inbound_space(
        domain: &mut mde_collab_core::DomainState,
        local: &ActorId,
        remote: &str,
    ) -> SpaceId {
        use mde_collab_core::domain::{MemberAgg, SpaceAgg};

        let space = SpaceId::new();
        domain.spaces.insert(
            space,
            SpaceAgg {
                kind: SpaceKind::Direct,
                name: "SIP direct".to_string(),
                deleted: false,
                members: BTreeMap::from([
                    (
                        local.clone(),
                        MemberAgg {
                            role: SpaceRole::Owner,
                            present: true,
                        },
                    ),
                    (
                        ActorId::new(remote),
                        MemberAgg {
                            role: SpaceRole::Member,
                            present: true,
                        },
                    ),
                ]),
            },
        );
        space
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
            CollabCommand::RequestAiSuggestion {
                space,
                request_id: "ai:req-1".into(),
                target: None,
                kind: mde_collab_types::AiSuggestionKind::Summary,
            },
            CollabCommand::CancelAiSuggestion {
                space,
                request_id: "ai:req-1".into(),
            },
        ];
        for cmd in &samples {
            assert!(
                COMMAND_VERBS.contains(&cmd.verb()),
                "COMMAND_VERBS is missing the verb {:?}",
                cmd.verb()
            );
        }
        // The count must equal the full taxonomy (43 verbs) so a NEW command
        // variant forces an update here.
        assert_eq!(
            COMMAND_VERBS.len(),
            43,
            "COMMAND_VERBS drifted from the taxonomy"
        );
    }

    #[test]
    fn inbound_sip_identity_admission_is_exact_unique_and_fail_closed() {
        use super::super::collab_media::InboundCallOffer;

        let local = ActorId::new("eagle");
        let mut domain = mde_collab_core::DomainState::default();
        let authorized = inbound_space(&mut domain, &local, "alice@example.com");
        let offer = |identity: &str, provider_call_id: &str| InboundCallOffer {
            identity: identity.to_string(),
            provider_call_id: provider_call_id.to_string(),
        };

        assert_eq!(
            admit_inbound_sip_identity(
                &domain,
                &local,
                &offer("<SIP:Alice@Example.COM>", "dialog-1@example.com")
            ),
            Ok(authorized)
        );
        for hostile in [
            offer("mallory@example.com", "dialog-2@example.com"),
            offer("Alice <sip:alice@example.com>", "dialog-3@example.com"),
            offer("sip:alice@example.com;user=phone", "dialog-4@example.com"),
            offer("sip:alice@example.com", "stale dialog"),
            offer("sip:alice@example.com", ""),
        ] {
            assert!(
                admit_inbound_sip_identity(&domain, &local, &hostile).is_err(),
                "hostile identity/dialog must not acquire call authority: {hostile:?}"
            );
        }

        inbound_space(&mut domain, &local, "ALICE@EXAMPLE.COM");
        assert_eq!(
            admit_inbound_sip_identity(
                &domain,
                &local,
                &offer("sip:alice@example.com", "dialog-5@example.com")
            ),
            Err("ambiguous SIP identity authority")
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
        assert!(take_new_forward(&persist, &mut cursors, "t/forward")
            .expect("seed cursor")
            .is_empty());
        persist
            .write("t/forward", Priority::Default, None, Some("new"))
            .unwrap();
        let got = take_new_forward(&persist, &mut cursors, "t/forward").expect("drain new");
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
        let got = take_new_all(&persist, &mut cursors, "t/all").expect("drain backlog");
        assert_eq!(got.len(), 2, "the full backlog drains on first sight");
        // Forward thereafter.
        assert!(take_new_all(&persist, &mut cursors, "t/all")
            .expect("drain forward")
            .is_empty());
    }

    #[test]
    fn merge_batch_chunks_oversized_retained_input() {
        let dir = tempfile::tempdir().expect("tempdir");
        let w = worker(dir.path(), "eagle");
        let mut state = CollabState::new(w.self_actor.clone()).expect("state");
        let foreign_signer = Ed25519Signer::new(key());
        let mut foreign = CollabEngine::in_memory(ActorId::new("nyc3")).expect("engine");
        let mut fids = RandomIds;
        let env = foreign
            .apply(
                &CollabCommand::CreateSpace {
                    kind: SpaceKind::Team,
                    name: "oversized-batch".into(),
                },
                &foreign_signer,
                &mut fids,
                50,
            )
            .expect("foreign create")
            .remove(0);
        let space = env.space_id;
        let incoming = vec![env; MAX_WORKER_MERGE_BATCH_EVENTS * 4 + 1];
        let mut touched = BTreeSet::new();
        let mut changed = false;

        w.merge_batch(&mut state, incoming, &mut touched, &mut changed, "test");

        assert!(
            changed,
            "the first bounded chunk is accepted instead of the whole aggregate failing"
        );
        assert!(
            state.engine.state().space(space).is_some(),
            "oversized retained aggregates converge over bounded worker chunks"
        );
        assert!(touched.contains(&space));
    }

    #[test]
    fn backfill_logs_streams_retained_actor_log_in_chunks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let w = worker(dir.path(), "eagle");
        let mut state = CollabState::new(w.self_actor.clone()).expect("state");
        let foreign_signer = Ed25519Signer::new(key());
        let mut foreign = CollabEngine::in_memory(ActorId::new("nyc3")).expect("engine");
        let mut fids = RandomIds;
        let env = foreign
            .apply(
                &CollabCommand::CreateSpace {
                    kind: SpaceKind::Team,
                    name: "retained-log".into(),
                },
                &foreign_signer,
                &mut fids,
                50,
            )
            .expect("foreign create")
            .remove(0);
        let space = env.space_id;
        let log_path = w.log_root.join(space.to_string()).join("nyc3.jsonl");
        std::fs::create_dir_all(log_path.parent().expect("log parent")).expect("mkdir log parent");
        let line = serde_json::to_string(&env).expect("serialize event");
        {
            let mut file = File::create(&log_path).expect("create actor log");
            for _ in 0..=(LOG_BACKFILL_SLICE_EVENTS * 4) {
                writeln!(file, "{line}").expect("write actor log line");
            }
        }
        let mut touched = BTreeSet::new();
        let mut changed = false;

        w.backfill_logs(&mut state, &mut touched, &mut changed);

        assert!(
            changed,
            "a retained log longer than the core batch cap is streamed in bounded chunks"
        );
        assert!(state.engine.state().space(space).is_some());
        assert!(touched.contains(&space));
        let offset = state
            .log_offsets
            .get(&log_path)
            .copied()
            .expect("backfill offset");
        assert!(
            offset < std::fs::metadata(&log_path).expect("log metadata").len(),
            "one tick leaves retained history for a later bounded slice"
        );
    }

    #[test]
    fn backfill_live_tail_projects_fresh_event_behind_large_retained_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let w = worker(dir.path(), "eagle");
        let mut state = CollabState::new(w.self_actor.clone()).expect("state");
        let foreign_signer = Ed25519Signer::new(key());
        let mut foreign = CollabEngine::in_memory(ActorId::new("dell")).expect("engine");
        let mut fids = RandomIds;
        let created = foreign
            .apply(
                &CollabCommand::CreateSpace {
                    kind: SpaceKind::Team,
                    name: "large-hot-log".into(),
                },
                &foreign_signer,
                &mut fids,
                50,
            )
            .expect("create")
            .remove(0);
        let fresh = foreign
            .apply(
                &CollabCommand::SendMessage {
                    space: created.space_id,
                    thread: None,
                    body: MessageBody::new("fresh-at-live-edge"),
                },
                &foreign_signer,
                &mut fids,
                51,
            )
            .expect("fresh message")
            .remove(0);
        let log_path = w
            .log_root
            .join(created.space_id.to_string())
            .join("dell.jsonl");
        std::fs::create_dir_all(log_path.parent().expect("log parent")).expect("mkdir");
        let retained_line = serde_json::to_string(&created).expect("serialize retained");
        let mut file = File::create(&log_path).expect("create actor log");
        for _ in 0..(LOG_BACKFILL_SLICE_EVENTS * 4) {
            writeln!(file, "{retained_line}").expect("write retained row");
        }
        writeln!(
            file,
            "{}",
            serde_json::to_string(&fresh).expect("serialize fresh")
        )
        .expect("write fresh row");
        drop(file);
        let log_len = std::fs::metadata(&log_path).expect("log metadata").len();
        let mut touched = BTreeSet::new();
        let mut changed = false;

        w.backfill_logs(&mut state, &mut touched, &mut changed);

        let timeline = state
            .engine
            .projection()
            .conversation_timeline(created.space_id, None)
            .expect("timeline");
        assert!(
            timeline
                .messages
                .iter()
                .any(|message| message.body == "fresh-at-live-edge"),
            "the newest complete event projects before sequential history catches up"
        );
        assert_eq!(state.log_live_offsets.get(&log_path), Some(&log_len));
        assert!(
            state.log_offsets.get(&log_path).copied().unwrap_or(0) < log_len,
            "full retained history remains bounded to one sequential slice"
        );
    }

    #[test]
    fn backfill_batch_projects_a_new_small_log_alongside_retained_history() {
        let dir = tempfile::tempdir().expect("tempdir");
        let w = worker(dir.path(), "eagle");
        let mut state = CollabState::new(w.self_actor.clone()).expect("state");
        let foreign_signer = Ed25519Signer::new(key());
        let mut foreign = CollabEngine::in_memory(ActorId::new("nyc3")).expect("engine");
        let mut fids = RandomIds;
        let old = foreign
            .apply(
                &CollabCommand::CreateSpace {
                    kind: SpaceKind::Team,
                    name: "old-history".into(),
                },
                &foreign_signer,
                &mut fids,
                50,
            )
            .expect("old event")
            .remove(0);
        let fresh = foreign
            .apply(
                &CollabCommand::CreateSpace {
                    kind: SpaceKind::Team,
                    name: "fresh-membership".into(),
                },
                &foreign_signer,
                &mut fids,
                51,
            )
            .expect("fresh event")
            .remove(0);
        let old_path = w.log_root.join(old.space_id.to_string()).join("nyc3.jsonl");
        let fresh_path = w
            .log_root
            .join(fresh.space_id.to_string())
            .join("nyc3.jsonl");
        std::fs::create_dir_all(old_path.parent().expect("old parent")).expect("old mkdir");
        std::fs::create_dir_all(fresh_path.parent().expect("fresh parent")).expect("fresh mkdir");
        let old_line = serde_json::to_string(&old).expect("serialize old");
        let mut old_file = File::create(&old_path).expect("create old log");
        for _ in 0..=LOG_BACKFILL_SLICE_EVENTS {
            writeln!(old_file, "{old_line}").expect("write old log");
        }
        writeln!(
            File::create(&fresh_path).expect("create fresh log"),
            "{}",
            serde_json::to_string(&fresh).expect("serialize fresh")
        )
        .expect("write fresh log");
        let mut touched = BTreeSet::new();
        let mut changed = false;

        w.backfill_logs(&mut state, &mut touched, &mut changed);

        assert!(changed);
        assert!(
            state.engine.state().space(fresh.space_id).is_some(),
            "the small new actor log is projected in the first bounded batch"
        );
        assert!(
            state.engine.state().space(old.space_id).is_some(),
            "the same batch also advances retained history"
        );
        assert!(
            state
                .log_offsets
                .get(&old_path)
                .copied()
                .expect("old offset")
                < std::fs::metadata(&old_path).expect("old metadata").len(),
            "each individual log remains limited to one slice per tick"
        );
        assert_eq!(
            state.log_offsets.get(&fresh_path).copied(),
            Some(
                std::fs::metadata(&fresh_path)
                    .expect("fresh metadata")
                    .len()
            )
        );
    }

    #[test]
    fn backfill_batch_advances_fresh_and_stale_logs_and_alternates_fairly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let w = worker(dir.path(), "eagle");
        let mut state = CollabState::new(w.self_actor.clone()).expect("state");
        let foreign_signer = Ed25519Signer::new(key());
        let mut foreign = CollabEngine::in_memory(ActorId::new("nyc3")).expect("engine");
        let mut fids = RandomIds;
        let stale = foreign
            .apply(
                &CollabCommand::CreateSpace {
                    kind: SpaceKind::Team,
                    name: "small-stale-log".into(),
                },
                &foreign_signer,
                &mut fids,
                50,
            )
            .expect("stale event")
            .remove(0);
        let fresh = foreign
            .apply(
                &CollabCommand::CreateSpace {
                    kind: SpaceKind::Team,
                    name: "larger-fresh-log".into(),
                },
                &foreign_signer,
                &mut fids,
                51,
            )
            .expect("fresh event")
            .remove(0);
        let stale_path = w
            .log_root
            .join(stale.space_id.to_string())
            .join("nyc3.jsonl");
        let fresh_path = w
            .log_root
            .join(fresh.space_id.to_string())
            .join("nyc3.jsonl");
        std::fs::create_dir_all(stale_path.parent().expect("stale parent")).expect("stale mkdir");
        std::fs::create_dir_all(fresh_path.parent().expect("fresh parent")).expect("fresh mkdir");
        writeln!(
            File::create(&stale_path).expect("create stale log"),
            "{}",
            serde_json::to_string(&stale).expect("serialize stale")
        )
        .expect("write stale log");
        let fresh_line = serde_json::to_string(&fresh).expect("serialize fresh");
        {
            let mut file = File::create(&fresh_path).expect("create fresh log");
            for _ in 0..=LOG_BACKFILL_SLICE_EVENTS {
                writeln!(file, "{fresh_line}").expect("write fresh row");
            }
        }
        File::options()
            .write(true)
            .open(&stale_path)
            .expect("open stale log")
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
            )
            .expect("set stale time");
        File::options()
            .write(true)
            .open(&fresh_path)
            .expect("open fresh log")
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(2)),
            )
            .expect("set fresh time");
        assert!(
            std::fs::metadata(&fresh_path)
                .expect("fresh metadata")
                .len()
                > std::fs::metadata(&stale_path)
                    .expect("stale metadata")
                    .len(),
            "fixture proves size-first ordering would choose the wrong log"
        );
        let mut touched = BTreeSet::new();
        let mut changed = false;

        w.backfill_logs(&mut state, &mut touched, &mut changed);

        assert!(state.engine.state().space(fresh.space_id).is_some());
        assert!(
            state.engine.state().space(stale.space_id).is_some(),
            "one bounded tick advances more than the single hottest log"
        );
        assert!(!state.prefer_recent_log_backfill);

        w.backfill_logs(&mut state, &mut touched, &mut changed);

        assert_eq!(
            state.log_offsets.get(&fresh_path).copied(),
            Some(
                std::fs::metadata(&fresh_path)
                    .expect("fresh metadata")
                    .len()
            ),
            "the next historical turn finishes the remaining bounded slice"
        );
        assert!(state.prefer_recent_log_backfill);
    }

    #[test]
    fn actor_log_reader_skips_oversized_line_and_keeps_following_valid_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("actor.jsonl");
        let foreign_signer = Ed25519Signer::new(key());
        let mut foreign = CollabEngine::in_memory(ActorId::new("nyc3")).expect("engine");
        let mut fids = RandomIds;
        let env = foreign
            .apply(
                &CollabCommand::CreateSpace {
                    kind: SpaceKind::Team,
                    name: "valid-after-oversized".into(),
                },
                &foreign_signer,
                &mut fids,
                50,
            )
            .expect("foreign create")
            .remove(0);
        {
            let mut file = File::create(&path).expect("create actor log");
            file.write_all(&vec![b'x'; MAX_ACTOR_LOG_LINE_BYTES + 1])
                .expect("write oversized line");
            file.write_all(b"\n").expect("write newline");
            writeln!(
                file,
                "{}",
                serde_json::to_string(&env).expect("serialize event")
            )
            .expect("write valid line");
        }
        let mut got = Vec::new();

        read_log_envelope_chunks(&path, |chunk| got.extend(chunk));

        assert_eq!(got.len(), 1);
        assert_eq!(got[0].event_id, env.event_id);
    }

    #[test]
    fn collab_commands_require_exact_single_use_capability() {
        let dir = tempfile::tempdir().expect("tempdir");
        let w = worker(dir.path(), "eagle");
        let persist = persist_at(dir.path());
        let mut state = CollabState::new(w.self_actor.clone()).expect("state");
        let command = CollabCommand::CreateSpace {
            kind: SpaceKind::Team,
            name: "authorized".into(),
        };
        let topic = topics::command_topic(command.verb());

        // The command lane is seeded forward-only, then an unsigned body is
        // rejected before deserialization or any engine effect.
        w.tick_once(&persist, &mut state, 100);
        let unsigned = serde_json::to_string(&command).expect("serialize command");
        write_raw(&persist, &topic, &unsigned);
        w.tick_once(&persist, &mut state, 200);
        assert!(
            state.engine.state().spaces.is_empty(),
            "unsigned collab commands have no mutation effect"
        );

        // The exact body, context, and a fresh nonce authorize one mutation.
        let authorized =
            authorized_command_body(&w, &command, "collab-hostile-0000000000000000000000001");
        write_raw(&persist, &topic, &authorized);
        w.tick_once(&persist, &mut state, 300);
        assert_eq!(state.engine.state().spaces.len(), 1);

        // Replaying the same signed envelope is refused by the durable nonce
        // ledger, so it cannot create a second space.
        write_raw(&persist, &topic, &authorized);
        w.tick_once(&persist, &mut state, 400);
        assert_eq!(
            state.engine.state().spaces.len(),
            1,
            "a collab capability is single-use"
        );
    }

    #[test]
    fn collab_command_capability_is_bound_to_its_command_lane() {
        let dir = tempfile::tempdir().expect("tempdir");
        let w = worker(dir.path(), "eagle");
        let persist = persist_at(dir.path());
        let mut state = CollabState::new(w.self_actor.clone()).expect("state");

        // Seed every command cursor, then create a space through its matching
        // lane so the owner-gated delete below would be effective if it escaped
        // the routing check.
        w.tick_once(&persist, &mut state, 100);
        let create = CollabCommand::CreateSpace {
            kind: SpaceKind::Team,
            name: "lane-bound".into(),
        };
        write_command(&w, &persist, &create);
        w.tick_once(&persist, &mut state, 200);
        let space = only_space(&state);
        let event_count = state.engine.all_events().len();

        // The body is a valid, owner-authorized DeleteSpace command, but its
        // capability is bound to the create_space lane. Before the lane/verb
        // binding, the worker would apply this destructive command here.
        let delete = CollabCommand::DeleteSpace { space };
        let mismatched = authorized_command_body_for_topic(
            &w,
            "create_space",
            &delete,
            "collab-lane-mismatch-000000000000000000000001",
        );
        write_raw(
            &persist,
            &topics::command_topic("create_space"),
            &mismatched,
        );
        w.tick_once(&persist, &mut state, 300);

        assert!(
            state.engine.state().space(space).is_some(),
            "a capability for create_space must not authorize DeleteSpace"
        );
        assert_eq!(
            state.engine.all_events().len(),
            event_count,
            "a cross-lane command must not mint any event"
        );
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
            &w,
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
            &w,
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
    fn ai_request_without_cloud_consent_publishes_honest_failure_state() {
        // WL-FUNC-011 DigitalOcean boundary: an AI request is no longer a silent
        // zero-event no-op. Without global cloud consent the worker publishes a
        // bounded failed request row under state/collab/ai-requests, emits no
        // provider call/event, and normal collaboration state remains intact.
        let dir = tempfile::tempdir().expect("tempdir");
        let w = worker(dir.path(), "eagle");
        let persist = persist_at(dir.path());
        let mut state = CollabState::new(w.self_actor.clone()).expect("state");
        w.tick_once(&persist, &mut state, 100); // seed cursors

        write_command(
            &w,
            &persist,
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
        );
        w.tick_once(&persist, &mut state, 200);
        let space = only_space(&state);
        let event_count = state.engine.all_events().len();

        write_command(
            &w,
            &persist,
            &CollabCommand::RequestAiSuggestion {
                space,
                request_id: "ai:req-1".into(),
                target: None,
                kind: mde_collab_types::AiSuggestionKind::Summary,
            },
        );
        w.tick_once(&persist, &mut state, 300);

        assert_eq!(
            state.engine.all_events().len(),
            event_count,
            "a consent-blocked AI request does not mint collaboration history"
        );
        let topic = topics::state_topic(proj::AI_REQUESTS);
        assert_eq!(topic, "state/collab/ai-requests");
        let msg = persist
            .read_latest(&topic)
            .expect("read ai requests")
            .expect("ai request state published");
        let requests: AiSuggestionRequests =
            serde_json::from_str(msg.body.as_deref().expect("body")).expect("decode ai requests");
        assert_eq!(requests.requests.len(), 1);
        let row = &requests.requests[0];
        assert_eq!(row.request_id, "ai:req-1");
        assert_eq!(row.space, space);
        assert_eq!(row.status, AiSuggestionRequestStatus::Failed);
        assert_eq!(row.provider, AI_PROVIDER_DIGITALOCEAN);
        assert_eq!(row.error.as_deref(), Some(AI_CONSENT_REQUIRED));
        assert!(
            row.model.is_none(),
            "no model attribution exists before a provider call"
        );
    }

    #[test]
    fn admitted_ai_request_can_be_canceled_by_request_id() {
        // The provider adapter is still external/sealed-key-gated, but the worker
        // sidecar now has a real cancelable request boundary for it to consume.
        let dir = tempfile::tempdir().expect("tempdir");
        let w = worker(dir.path(), "eagle").with_ai_cloud_consent_for_test(true);
        let persist = persist_at(dir.path());
        let mut state = CollabState::new(w.self_actor.clone()).expect("state");
        w.tick_once(&persist, &mut state, 100); // seed cursors

        write_command(
            &w,
            &persist,
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
        );
        w.tick_once(&persist, &mut state, 200);
        let space = only_space(&state);
        let read_space_ai = |persist: &Persist| -> AiSuggestionRequests {
            let topic = topics::space_state_topic(proj::AI_REQUESTS, space);
            let msg = persist
                .read_latest(&topic)
                .expect("read space ai")
                .expect("space ai request state published");
            serde_json::from_str(msg.body.as_deref().expect("body")).expect("decode space ai")
        };

        write_command(
            &w,
            &persist,
            &CollabCommand::RequestAiSuggestion {
                space,
                request_id: "ai:req-2".into(),
                target: None,
                kind: mde_collab_types::AiSuggestionKind::SmartReply,
            },
        );
        w.tick_once(&persist, &mut state, 300);
        let requests = read_space_ai(&persist);
        assert_eq!(requests.requests.len(), 1);
        assert_eq!(
            requests.requests[0].status,
            AiSuggestionRequestStatus::Pending,
            "cloud-consented requests are admitted to the cancellable sidecar"
        );

        write_command(
            &w,
            &persist,
            &CollabCommand::CancelAiSuggestion {
                space,
                request_id: "ai:req-2".into(),
            },
        );
        w.tick_once(&persist, &mut state, 400);
        let requests = read_space_ai(&persist);
        assert_eq!(requests.requests.len(), 1);
        assert_eq!(
            requests.requests[0].status,
            AiSuggestionRequestStatus::Canceled
        );
        assert!(requests.requests[0].error.is_none());
    }

    #[test]
    fn empty_media_registry_refuses_fake_connected_call_state() {
        // Production currently has no registered SIP/WebRTC/LiveKit provider.
        // Starting a call must therefore leave both signed call state and the
        // provider readiness/verification boards empty.
        let dir = tempfile::tempdir().expect("tempdir");
        let w = worker(dir.path(), "eagle");
        let persist = persist_at(dir.path());
        let mut state = CollabState::new(w.self_actor.clone()).expect("state");
        w.tick_once(&persist, &mut state, 100); // seed cursors

        write_command(
            &w,
            &persist,
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
        );
        w.tick_once(&persist, &mut state, 200);
        let space = only_space(&state);

        let call = CallId::new();
        write_command(
            &w,
            &persist,
            &CollabCommand::StartCall {
                space,
                call,
                kind: CallKind::Audio,
            },
        );
        w.tick_once(&persist, &mut state, 300);

        let topic = topics::space_state_topic(proj::CALL_MEDIA_READINESS, space);
        assert_eq!(topic, format!("state/collab/call-media-readiness/{space}"));
        let msg = persist
            .read_latest(&topic)
            .expect("read media readiness")
            .expect("space media readiness published");
        let readiness: CallMediaReadiness =
            serde_json::from_str(msg.body.as_deref().expect("body"))
                .expect("decode media readiness");
        assert_eq!(readiness.local_actor, w.self_actor);
        assert!(
            readiness.sessions.is_empty(),
            "an empty production provider registry must not mint adapter readiness"
        );
        assert!(
            state
                .engine
                .projection()
                .call_state(Some(space))
                .expect("call state")
                .active
                .is_empty(),
            "an unavailable provider must not become fake connected call state"
        );

        let global_topic = topics::state_topic(proj::CALL_MEDIA_READINESS);
        assert_eq!(global_topic, "state/collab/call-media-readiness");
        assert!(
            persist
                .read_latest(&global_topic)
                .expect("read global media readiness")
                .is_some(),
            "the adapter can also consume the unscoped local readiness board"
        );

        let verification_topic = topics::state_topic(proj::CALL_MEDIA_VERIFICATION);
        assert_eq!(verification_topic, "state/collab/call-media-verification");
        let verification_msg = persist
            .read_latest(&verification_topic)
            .expect("read media verification")
            .expect("media verification published");
        let verification: CallMediaVerification =
            serde_json::from_str(verification_msg.body.as_deref().expect("body"))
                .expect("decode media verification");
        assert_eq!(verification.local_actor, w.self_actor);
        assert!(verification.rows.is_empty());
    }

    #[test]
    fn registered_call_media_provider_proves_frames_through_worker_publish() {
        struct WebRtcAudioProof;
        impl super::super::collab_media::CallMediaFrameVerifier for WebRtcAudioProof {
            fn execute_command(
                &self,
                _command: &CollabCommand,
                adapter: CallMediaAdapter,
            ) -> Result<(), super::super::collab_media::CallMediaProviderError> {
                assert_eq!(adapter, CallMediaAdapter::WebRtcP2p);
                Ok(())
            }

            fn prove_advancing_frames(
                &self,
                session: &mde_collab_types::CallMediaSession,
                adapter: CallMediaAdapter,
            ) -> Result<CallMediaFrameEvidence, super::super::collab_media::CallMediaProviderError>
            {
                assert_eq!(adapter, CallMediaAdapter::WebRtcP2p);
                assert_eq!(session.admission, CallMediaAdmission::AdapterReady);
                Ok(CallMediaFrameEvidence {
                    audio_frames: 3,
                    video_frames: 0,
                    screen_frames: 0,
                    data_messages: 0,
                })
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let mut providers = super::super::collab_media::CallMediaProviderRegistry::empty();
        providers
            .register(CallMediaAdapter::WebRtcP2p, WebRtcAudioProof)
            .expect("register WebRTC provider");
        let w = worker(dir.path(), "alice").with_call_media_providers(providers);
        let persist = persist_at(dir.path());
        let mut state = CollabState::new(w.self_actor.clone()).expect("state");
        w.tick_once(&persist, &mut state, 100); // seed cursors

        write_command(
            &w,
            &persist,
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
        );
        w.tick_once(&persist, &mut state, 200);
        let space = only_space(&state);
        write_command(
            &w,
            &persist,
            &CollabCommand::AddMember {
                space,
                actor: ActorId::new("bob"),
                role: SpaceRole::Member,
            },
        );
        w.tick_once(&persist, &mut state, 300);
        let call = CallId::new();
        write_command(
            &w,
            &persist,
            &CollabCommand::StartCall {
                space,
                call,
                kind: CallKind::Audio,
            },
        );
        w.tick_once(&persist, &mut state, 400);

        let mut bob = CollabEngine::in_memory(ActorId::new("bob")).expect("bob engine");
        bob.merge(state.engine.all_events()).expect("bob syncs");
        let mut bob_ids = RandomIds;
        let bob_signer = Ed25519Signer::new(key());
        let bob_events = bob
            .apply(
                &CollabCommand::AnswerCall { call },
                &bob_signer,
                &mut bob_ids,
                500,
            )
            .expect("bob answers");
        for env in &bob_events {
            write_event(&persist, env);
        }
        w.tick_once(&persist, &mut state, 600);

        let readiness_msg = persist
            .read_latest(&topics::state_topic(proj::CALL_MEDIA_READINESS))
            .expect("read media readiness")
            .expect("media readiness published");
        let readiness: CallMediaReadiness =
            serde_json::from_str(readiness_msg.body.as_deref().expect("body"))
                .expect("decode media readiness");
        assert_eq!(readiness.sessions.len(), 1);
        assert_eq!(
            readiness.sessions[0].admission,
            CallMediaAdmission::AdapterReady
        );
        assert_eq!(
            readiness.sessions[0].connected_participants,
            vec![ActorId::new("alice"), ActorId::new("bob")]
        );

        let verification_msg = persist
            .read_latest(&topics::state_topic(proj::CALL_MEDIA_VERIFICATION))
            .expect("read media verification")
            .expect("media verification published");
        let verification: CallMediaVerification =
            serde_json::from_str(verification_msg.body.as_deref().expect("body"))
                .expect("decode media verification");
        let webrtc = verification
            .rows
            .iter()
            .find(|row| row.adapter == CallMediaAdapter::WebRtcP2p)
            .expect("WebRTC row");
        assert_eq!(
            webrtc.status,
            CallMediaVerificationStatus::LiveMediaVerified
        );
        assert_eq!(
            webrtc.evidence,
            Some(CallMediaFrameEvidence {
                audio_frames: 3,
                video_frames: 0,
                screen_frames: 0,
                data_messages: 0,
            })
        );
        for adapter in [CallMediaAdapter::LiveKitSfu, CallMediaAdapter::SipGateway] {
            let row = verification
                .rows
                .iter()
                .find(|row| row.adapter == adapter)
                .expect("missing provider row");
            assert_eq!(row.status, CallMediaVerificationStatus::ProviderUnavailable);
            assert!(row.evidence.is_none());
        }
    }

    #[test]
    fn provider_observed_revocation_durably_ends_the_exact_call() {
        struct RevokingProvider {
            revoked: Arc<std::sync::Mutex<Vec<CallId>>>,
        }

        impl super::super::collab_media::CallMediaFrameVerifier for RevokingProvider {
            fn execute_command(
                &self,
                _command: &CollabCommand,
                adapter: CallMediaAdapter,
            ) -> Result<(), super::super::collab_media::CallMediaProviderError> {
                assert_eq!(adapter, CallMediaAdapter::WebRtcP2p);
                Ok(())
            }

            fn prove_advancing_frames(
                &self,
                _session: &mde_collab_types::CallMediaSession,
                _adapter: CallMediaAdapter,
            ) -> Result<CallMediaFrameEvidence, super::super::collab_media::CallMediaProviderError>
            {
                Err(
                    super::super::collab_media::CallMediaProviderError::TransportUnavailable {
                        detail: "remote peer ended transport".into(),
                    },
                )
            }

            fn take_revoked_calls(&self) -> Vec<CallId> {
                self.revoked
                    .lock()
                    .map(|mut calls| std::mem::take(&mut *calls))
                    .unwrap_or_default()
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let revoked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut providers = super::super::collab_media::CallMediaProviderRegistry::empty();
        providers
            .register(
                CallMediaAdapter::WebRtcP2p,
                RevokingProvider {
                    revoked: Arc::clone(&revoked),
                },
            )
            .expect("register revoking provider");
        let w = worker(dir.path(), "alice").with_call_media_providers(providers);
        let persist = persist_at(dir.path());
        let mut state = CollabState::new(w.self_actor.clone()).expect("state");
        w.tick_once(&persist, &mut state, 100);
        write_command(
            &w,
            &persist,
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
        );
        w.tick_once(&persist, &mut state, 200);
        let space = only_space(&state);
        let call = CallId::new();
        write_command(
            &w,
            &persist,
            &CollabCommand::StartCall {
                space,
                call,
                kind: CallKind::Audio,
            },
        );
        w.tick_once(&persist, &mut state, 300);
        assert_eq!(
            state
                .engine
                .projection()
                .call_state(Some(space))
                .expect("active call state")
                .active
                .len(),
            1
        );

        revoked.lock().expect("revocation queue").push(call);
        w.tick_once(&persist, &mut state, 400);
        assert!(
            state
                .engine
                .projection()
                .call_state(Some(space))
                .expect("revoked call state")
                .active
                .is_empty(),
            "provider-observed termination must end the exact signed call"
        );

        let log_path = w
            .log_root
            .join(space.to_string())
            .join(format!("{}.jsonl", w.self_actor.as_str()));
        let log = std::fs::read_to_string(log_path).expect("durable actor log");
        assert!(
            log.contains("call_ended") && log.contains(&call.to_string()),
            "provider revocation must be durable, not projection-only"
        );
    }

    #[test]
    fn proof_only_provider_failure_never_authors_call_state() {
        struct ProofOnlyProvider;
        impl super::super::collab_media::CallMediaFrameVerifier for ProofOnlyProvider {
            fn prove_advancing_frames(
                &self,
                _session: &mde_collab_types::CallMediaSession,
                _adapter: CallMediaAdapter,
            ) -> Result<CallMediaFrameEvidence, super::super::collab_media::CallMediaProviderError>
            {
                Ok(CallMediaFrameEvidence {
                    audio_frames: 99,
                    video_frames: 0,
                    screen_frames: 0,
                    data_messages: 0,
                })
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let mut providers = super::super::collab_media::CallMediaProviderRegistry::empty();
        providers
            .register(CallMediaAdapter::WebRtcP2p, ProofOnlyProvider)
            .expect("register proof-only provider");
        let w = worker(dir.path(), "alice").with_call_media_providers(providers);
        let persist = persist_at(dir.path());
        let mut state = CollabState::new(w.self_actor.clone()).expect("state");
        w.tick_once(&persist, &mut state, 100);
        write_command(
            &w,
            &persist,
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
        );
        w.tick_once(&persist, &mut state, 200);
        let space = only_space(&state);
        let events_before_call = state.engine.all_events().len();
        write_command(
            &w,
            &persist,
            &CollabCommand::StartCall {
                space,
                call: CallId::new(),
                kind: CallKind::Audio,
            },
        );
        w.tick_once(&persist, &mut state, 300);

        assert!(state
            .engine
            .projection()
            .call_state(Some(space))
            .expect("call state")
            .active
            .is_empty());
        assert_eq!(
            state.engine.all_events().len(),
            events_before_call,
            "the failed provider command must not append a signed call event"
        );
    }

    /// Create a space (this node becomes Owner) and link `reference` into it as
    /// `file`. Assumes the command cursors are already seeded. Returns the space.
    fn create_space_and_link(
        w: &CollabWorker,
        persist: &Persist,
        state: &mut CollabState,
        file: FileRefId,
        reference: FileRef,
        bytes: &[u8],
        t0: i64,
    ) -> SpaceId {
        write_command(
            w,
            persist,
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
        );
        w.tick_once(persist, state, t0);
        let space = only_space(state);
        let mut blobs = FsBlobStore::new(&w.content_root);
        let installed = blobs.put(bytes).expect("install canonical fixture payload");
        assert_eq!(installed.sha256_hex, reference.sha256_hex);
        assert_eq!(installed.len, reference.size);
        write_command(
            w,
            persist,
            &CollabCommand::LinkFile {
                space,
                file,
                reference,
            },
        );
        w.tick_once(persist, state, t0 + 100);
        space
    }

    #[test]
    fn linking_a_file_projects_a_reference_then_unlinking_removes_only_the_space_ref() {
        // WL-FUNC-011 — linking a file records the FileRef (owner + name + content
        // hash) into the space's file_references projection; unlinking removes the
        // SPACE REFERENCE, not the canonical file.
        let dir = tempfile::tempdir().expect("tempdir");
        let w = worker(dir.path(), "eagle");
        let persist = persist_at(dir.path());
        let mut state = CollabState::new(w.self_actor.clone()).expect("state");
        w.tick_once(&persist, &mut state, 100); // seed cursors

        let file = FileRefId::new();
        let bytes = b"deploy log bytes";
        let reference = FileRef {
            name: "deploy.log".into(),
            size: bytes.len() as u64,
            sha256_hex: sha256_hex(bytes),
            mime: Some("text/plain".into()),
        };
        let space = create_space_and_link(
            &w,
            &persist,
            &mut state,
            file,
            reference.clone(),
            bytes,
            200,
        );

        // The per-space file-references projection is published + carries the ref.
        let files_topic = topics::space_state_topic(proj::FILE_REFERENCES, space);
        assert_eq!(files_topic, format!("state/collab/file-references/{space}"));
        let read_refs = |persist: &Persist| -> FileReferences {
            let msg = persist
                .read_latest(&files_topic)
                .expect("read file_refs")
                .expect("file_refs published");
            serde_json::from_str(msg.body.as_deref().expect("body")).expect("decode file_refs")
        };
        let refs = read_refs(&persist);
        assert_eq!(refs.files.len(), 1, "the linked file is projected");
        assert_eq!(refs.files[0].file, file);
        assert_eq!(refs.files[0].reference.name, "deploy.log");
        assert_eq!(
            refs.files[0].reference.sha256_hex, reference.sha256_hex,
            "the projected FileRef carries the real content hash"
        );
        assert_eq!(
            refs.files[0].linked_by, w.self_actor,
            "the projection records who linked it (the owner)"
        );

        // Unlink: removes the space's reference.
        write_command(&w, &persist, &CollabCommand::UnlinkFile { space, file });
        w.tick_once(&persist, &mut state, 400);
        let refs = read_refs(&persist);
        assert!(
            refs.files.is_empty(),
            "unlinking removes the space's reference from the projection"
        );

        // ...but NOT the canonical file: unlink is a reference tombstone, so the
        // FileLinked event (which carries the file's content address/identity) is
        // still in the durable log alongside a separate FileUnlinked event — no
        // content purge happened.
        let events = state.engine.all_events();
        assert!(
            events.iter().any(|e| matches!(
                &e.kind,
                CollabEventKind::FileLinked { file: f, reference: r }
                    if *f == file && r.sha256_hex == reference.sha256_hex
            )),
            "the FileLinked event (the file's identity + hash) is retained after unlink"
        );
        assert!(
            events.iter().any(
                |e| matches!(&e.kind, CollabEventKind::FileUnlinked { file: f } if *f == file)
            ),
            "the unlink is a distinct reference tombstone, not a content delete"
        );
    }

    #[test]
    fn link_file_requires_exact_canonical_payload_before_authoritative_projection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let w = worker(dir.path(), "eagle");
        let persist = persist_at(dir.path());
        let mut state = CollabState::new(w.self_actor.clone()).expect("state");
        w.tick_once(&persist, &mut state, 100);

        write_command(
            &w,
            &persist,
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "payload admission".into(),
            },
        );
        w.tick_once(&persist, &mut state, 200);
        let space = only_space(&state);
        let file = FileRefId::new();
        let bytes = b"canonical payload must exist";
        let reference = FileRef {
            name: "admitted.bin".into(),
            size: bytes.len() as u64,
            sha256_hex: sha256_hex(bytes),
            mime: Some("application/octet-stream".into()),
        };
        let command = CollabCommand::LinkFile {
            space,
            file,
            reference: reference.clone(),
        };

        write_command(&w, &persist, &command);
        w.tick_once(&persist, &mut state, 300);
        assert!(
            !state.engine.all_events().iter().any(
                |event| matches!(&event.kind, CollabEventKind::FileLinked { file: found, .. } if *found == file)
            ),
            "metadata alone must not create a usable Files identity"
        );

        let mut blobs = FsBlobStore::new(&w.content_root);
        let installed = blobs.put(bytes).expect("install corrected-forward payload");
        assert_eq!(installed.sha256_hex, reference.sha256_hex);
        write_command(&w, &persist, &command);
        w.tick_once(&persist, &mut state, 400);

        let rows = state
            .engine
            .projection()
            .file_references(space)
            .expect("file projection");
        assert_eq!(rows.files.len(), 1);
        assert_eq!(rows.files[0].file, file);
        assert_eq!(rows.files[0].reference, reference);
    }

    #[test]
    fn transfer_start_then_cancel_projects_the_shared_ledger_state() {
        // WL-FUNC-011 — a linked file's transfer flows through the shared ledger:
        // StartTransfer projects the control handle (Queued); ControlTransfer moves
        // its state. Byte progress (moved/total) is mirrored from WL-FUNC-006, not
        // recomputed here — 0/0 until the ledger reports (no second authority).
        let dir = tempfile::tempdir().expect("tempdir");
        let w = worker(dir.path(), "eagle");
        let persist = persist_at(dir.path());
        let mut state = CollabState::new(w.self_actor.clone()).expect("state");
        w.tick_once(&persist, &mut state, 100); // seed cursors

        let file = FileRefId::new();
        let bytes = b"iso bytes";
        let reference = FileRef {
            name: "artifact.iso".into(),
            size: bytes.len() as u64,
            sha256_hex: sha256_hex(bytes),
            mime: None,
        };
        let space = create_space_and_link(&w, &persist, &mut state, file, reference, bytes, 200);

        let jobs_topic = topics::state_topic(proj::TRANSFER_JOBS);
        let read_jobs = |persist: &Persist| -> TransferJobs {
            let msg = persist
                .read_latest(&jobs_topic)
                .expect("read transfers")
                .expect("transfers published");
            serde_json::from_str(msg.body.as_deref().expect("body")).expect("decode transfers")
        };

        // Share to members → StartTransfer → the mirror carries a Queued job.
        let transfer = TransferId::new();
        write_command(
            &w,
            &persist,
            &CollabCommand::StartTransfer {
                space,
                transfer,
                file,
                method: TransferMethod::Node,
                direction: TransferDirection::Outbound,
            },
        );
        w.tick_once(&persist, &mut state, 400);
        let jobs = read_jobs(&persist);
        assert_eq!(jobs.jobs.len(), 1, "the transfer is projected");
        assert_eq!(jobs.jobs[0].transfer, transfer);
        assert_eq!(jobs.jobs[0].file, file);
        assert_eq!(jobs.jobs[0].state, TransferState::Queued);
        assert_eq!(
            (jobs.jobs[0].moved, jobs.jobs[0].total),
            (0, 0),
            "byte progress is mirrored from WL-FUNC-006, not owned here"
        );

        // A queued transfer may be canceled; still no second progress authority.
        write_command(
            &w,
            &persist,
            &CollabCommand::ControlTransfer {
                transfer,
                control: TransferControl::Cancel,
            },
        );
        w.tick_once(&persist, &mut state, 500);
        let jobs = read_jobs(&persist);
        assert_eq!(
            jobs.jobs[0].state,
            TransferState::Canceled,
            "ControlTransfer::Cancel moved the shared transfer to Canceled"
        );
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
    fn valid_signed_event_requires_exact_space_and_actor_lane_identity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let w = worker(dir.path(), "eagle");
        let persist = persist_at(dir.path());
        let mut state = CollabState::new(w.self_actor.clone()).expect("state");
        w.tick_once(&persist, &mut state, 100); // seed cursors

        let signer = Ed25519Signer::new(key());
        let mut foreign = CollabEngine::in_memory(ActorId::new("nyc3")).expect("engine");
        let mut ids = RandomIds;
        let created = foreign
            .apply(
                &CollabCommand::CreateSpace {
                    kind: SpaceKind::Team,
                    name: "remote".into(),
                },
                &signer,
                &mut ids,
                50,
            )
            .expect("foreign create");
        let space = created[0].space_id;
        for env in &created {
            let body = serde_json::to_string(env).expect("serialize event");
            write_raw(
                &persist,
                &topics::event_topic(space, &ActorId::new("impostor")),
                &body,
            );
            write_raw(
                &persist,
                &topics::event_topic(SpaceId::new(), &env.actor),
                &body,
            );
        }

        w.tick_once(&persist, &mut state, 200);
        assert!(
            state.engine.all_events().is_empty(),
            "valid signatures on mismatched actor or space lanes must fail closed",
        );

        for env in &created {
            write_event(&persist, env);
        }
        w.tick_once(&persist, &mut state, 300);
        assert!(
            state.engine.state().space(space).is_some(),
            "the same signed events remain admissible on their exact canonical lane",
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
            &wa,
            &pa,
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "shared".into(),
            },
        );
        wa.tick_once(&pa, &mut sa, 200);
        let space = only_space(&sa);
        write_command(
            &wa,
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
            &wa,
            &pa,
            &CollabCommand::SendMessage {
                space,
                thread: None,
                body: MessageBody::new("from-alpha"),
            },
        );
        wa.tick_once(&pa, &mut sa, 500);
        write_command(
            &wb,
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

    fn write_raw(persist: &Persist, topic: &str, body: &str) {
        persist
            .write(topic, Priority::Default, None, Some(body))
            .expect("write raw body");
    }

    // ── WL-FUNC-011 worker folds ─────────────────────────────────────────

    #[test]
    fn alert_lane_folds_into_an_alert_raised_event() {
        // A truthful alert published on a real alert lane (event/notify/*) is
        // folded — worker-side — into an AlertRaised collab event in the node's
        // system space, and rolls up into the fleet-wide alert inbox.
        let dir = tempfile::tempdir().expect("tempdir");
        let w = worker(dir.path(), "eagle");
        let persist = persist_at(dir.path());
        let mut state = CollabState::new(w.self_actor.clone()).expect("state");

        // The lane is discovered via list_topics, so it must exist before the
        // cursor is seeded; the drain is forward-only (a restart never re-raises a
        // stale backlog), so the pre-seed message is skipped, not folded.
        write_raw(
            &persist,
            "event/notify/disk",
            r#"{"severity":"info","source":"nyc3","summary":"seed"}"#,
        );
        w.tick_once(&persist, &mut state, 100); // seed the lane cursor (forward-only)

        // No fold has happened yet, so the system space is NOT created (it is
        // bootstrapped lazily, only when the node actually has a fact to record).
        let system = w.system_space_id();
        assert!(
            state.engine.state().space(system).is_none(),
            "the system space is not materialized until the first fold",
        );

        write_raw(
            &persist,
            "event/notify/disk",
            r#"{"severity":"warning","source":"nyc3","summary":"disk pre-fail","disk":"94%"}"#,
        );
        w.tick_once(&persist, &mut state, 200); // drain + fold → lazily bootstrap + author

        // The first fold lazily bootstrapped the node's system space (a real,
        // owned member space) and authored the alert into it.
        assert!(
            state.engine.state().is_owner(system, &w.self_actor),
            "the node owns its lazily-bootstrapped system space",
        );

        let topic = topics::state_topic(proj::ALERT_INBOX);
        assert_eq!(topic, "state/collab/alert-inbox");
        let msg = persist
            .read_latest(&topic)
            .expect("read inbox")
            .expect("alert inbox published");
        let inbox: AlertInbox =
            serde_json::from_str(msg.body.as_deref().expect("body")).expect("decode inbox");
        assert_eq!(inbox.alerts.len(), 1, "exactly the post-seed alert folded");
        let a = &inbox.alerts[0];
        assert_eq!(a.alert.headline, "disk pre-fail");
        assert_eq!(a.alert.source, "nyc3");
        assert_eq!(a.alert.severity, Severity::Warning);
        assert_eq!(
            a.alert.fields.get("disk").map(String::as_str),
            Some("94%"),
            "the folded alert carries the truthful structured fields",
        );
        assert_eq!(
            a.space, system,
            "the folded alert lives in the system space"
        );
    }

    #[test]
    fn clipboard_capture_folds_into_clipboard_published() {
        // A cross-mesh clipboard capture on event/clipboard/clip is folded into a
        // ClipboardPublished collab event with the RECOMPUTED full content address.
        let dir = tempfile::tempdir().expect("tempdir");
        let w = worker(dir.path(), "eagle");
        let persist = persist_at(dir.path());
        let mut state = CollabState::new(w.self_actor.clone()).expect("state");

        write_raw(
            &persist,
            "event/clipboard/clip",
            r#"{"id":"seed","text":"seed clip","source":"falcon","time":"t"}"#,
        );
        w.tick_once(&persist, &mut state, 100); // seed the capture cursor (forward-only)
        write_raw(
            &persist,
            "event/clipboard/clip",
            r#"{"id":"def","text":"https://example.test/x","source":"falcon","time":"t"}"#,
        );
        w.tick_once(&persist, &mut state, 200); // drain + fold → lazily bootstrap + author

        let system = w.system_space_id();
        let topic = topics::space_state_topic(proj::CLIPBOARD_LANE, system);
        let msg = persist
            .read_latest(&topic)
            .expect("read lane")
            .expect("clipboard lane published");
        let lane: ClipboardLane =
            serde_json::from_str(msg.body.as_deref().expect("body")).expect("decode lane");
        assert_eq!(lane.items.len(), 1, "exactly the post-seed clip folded");
        let item = &lane.items[0];
        assert_eq!(item.kind, ClipItemKind::Uri, "an http(s) clip is a URI");
        assert_eq!(item.source, "falcon");
        assert_eq!(
            item.sha256_hex,
            sha256_hex(b"https://example.test/x"),
            "the fold recomputes the real full content address (not the 16-hex capture prefix)",
        );
    }

    #[test]
    fn fold_alert_payload_reads_fields_and_actions() {
        let body = r#"{"severity":"crit","host":"core-1","summary":"meltdown","zone":"a",
            "actions":[{"id":"restart","label":"Restart","kind":"destructive","verb":"action/node/restart"}]}"#;
        let p = fold_alert_payload("fleet/health/breaker/x", body, "self").expect("fold");
        assert_eq!(p.severity, Severity::Critical);
        assert_eq!(p.source, "core-1");
        assert_eq!(p.headline, "meltdown");
        assert_eq!(p.fields.get("zone").map(String::as_str), Some("a"));
        assert_eq!(p.actions.len(), 1);
        assert_eq!(p.actions[0].kind, AlertActionKind::Destructive);
        assert_eq!(p.actions[0].id, "restart");
    }

    #[test]
    fn fold_alert_payload_falls_back_and_rejects_non_objects() {
        let p = fold_alert_payload("event/security/x", "{}", "eagle").expect("fold empty obj");
        assert_eq!(p.source, "eagle", "source falls back to the origin node");
        assert_eq!(
            p.headline, "security",
            "headline falls back to the topic flag"
        );
        assert_eq!(p.severity, Severity::Info);
        // A non-object body is not an alert we fold.
        assert!(fold_alert_payload("event/security/x", "not json", "eagle").is_none());
        assert!(fold_alert_payload("event/security/x", "\"a string\"", "eagle").is_none());
    }

    #[test]
    fn fold_clip_item_recomputes_hash_and_the_size_gate_holds() {
        let body = r#"{"id":"p","text":"hello mesh","source":"falcon","time":"t"}"#;
        let item = fold_clip_item(body).expect("fold clip");
        assert_eq!(item.kind, ClipItemKind::Text);
        assert_eq!(item.len, 10);
        assert_eq!(item.sha256_hex, sha256_hex(b"hello mesh"));
        assert_eq!(item.source, "falcon");
        // The >1 MiB → Transfers gate is a pure boundary (no large fixture needed).
        assert!(clip_fits_lane(MAX_CLIP_BYTES));
        assert!(!clip_fits_lane(MAX_CLIP_BYTES + 1));
    }

    #[test]
    fn is_alert_lane_excludes_the_suites_own_lanes() {
        assert!(is_alert_lane("event/notify/disk"));
        assert!(is_alert_lane("fleet/health/breaker/x"));
        assert!(is_alert_lane("event/security/audit"));
        // The clipboard capture lane is not an alert lane.
        assert!(!is_alert_lane("event/clipboard/clip"));
        // Collab's own lanes are never re-folded.
        assert!(!is_alert_lane("state/collab/alert-inbox"));
        assert!(!is_alert_lane("action/collab/ack_alert"));
        assert!(!is_alert_lane(&format!(
            "collab/event/{}/eagle",
            SpaceId::new()
        )));
    }

    #[test]
    fn service_bus_root_falls_back_to_the_shared_system_spool() {
        assert_eq!(
            collab_bus_root_or_system(None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
        assert_eq!(
            collab_bus_root_or_system(Some(PathBuf::from("/tmp/collab-explicit-bus"))),
            PathBuf::from("/tmp/collab-explicit-bus")
        );
    }

    #[tokio::test]
    async fn late_bus_and_cursor_prime_recover_without_replay_or_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bus_root = dir.path().join("bus");
        let persist = Persist::open(bus_root.clone()).expect("prepare delayed Bus");
        let mut w = worker(dir.path(), "eagle").with_poll_interval(Duration::from_millis(10));

        let stale = CollabCommand::CreateSpace {
            kind: SpaceKind::Team,
            name: "retained-must-not-replay".into(),
        };
        let stale_body =
            authorized_command_body(&w, &stale, "collab-late-bus-stale-00000000000000000001");
        persist
            .write(
                &topics::command_topic(stale.verb()),
                Priority::Default,
                None,
                Some(&stale_body),
            )
            .expect("write retained command");

        let fresh = CollabCommand::CreateSpace {
            kind: SpaceKind::Team,
            name: "fresh-after-recovery".into(),
        };
        let fresh_body =
            authorized_command_body(&w, &fresh, "collab-late-bus-fresh-00000000000000000001");
        let fresh_topic = topics::command_topic(fresh.verb());

        let open_attempts = Arc::new(AtomicU64::new(0));
        let open_attempts_for_worker = Arc::clone(&open_attempts);
        let bus_root_for_worker = bus_root.clone();
        w = w.with_bus_opener(Arc::new(move || {
            match open_attempts_for_worker.fetch_add(1, Ordering::SeqCst) {
                0 => Ok(None),
                1 => Err("injected unopenable Bus".into()),
                _ => Persist::open(bus_root_for_worker.clone())
                    .map(Some)
                    .map_err(|error| error.to_string()),
            }
        }));

        let prime_attempts = Arc::new(AtomicU64::new(0));
        let prime_attempts_for_worker = Arc::clone(&prime_attempts);
        w = w.with_cursor_primer(Arc::new(move |persist| {
            if prime_attempts_for_worker.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err("injected cursor list failure".into());
            }
            prime_transient_cursors(persist)
        }));

        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let worker_task = tokio::spawn(async move { w.run(token).await });

        tokio::time::timeout(Duration::from_secs(3), async {
            while prime_attempts.load(Ordering::SeqCst) < 2 {
                assert!(
                    !worker_task.is_finished(),
                    "worker exited during startup recovery"
                );
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("same worker must activate after late Bus and prime failure");
        assert!(open_attempts.load(Ordering::SeqCst) >= 4);

        persist
            .write(&fresh_topic, Priority::Default, None, Some(&fresh_body))
            .expect("write fresh command after activation");

        let directory_topic = topics::state_topic(proj::SPACE_DIRECTORY);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(message) = persist
                    .read_latest(&directory_topic)
                    .expect("read projected directory")
                {
                    let directory: SpaceDirectory =
                        serde_json::from_str(message.body.as_deref().expect("directory body"))
                            .expect("decode projected directory");
                    if directory.spaces.len() == 1 {
                        assert_eq!(directory.spaces[0].name, "fresh-after-recovery");
                        break;
                    }
                }
                assert!(
                    !worker_task.is_finished(),
                    "worker exited before fresh command"
                );
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("fresh command must project after late Bus without restart");

        // Let additional polls run: neither the retained startup command nor the
        // fresh command may execute again.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let event_count = persist
            .list_topics()
            .expect("list event topics")
            .into_iter()
            .filter(|topic| topic.starts_with(topics::EVENT_PREFIX))
            .map(|topic| persist.list_since(&topic, None).expect("list events").len())
            .sum::<usize>();
        assert_eq!(event_count, 2, "one CreateSpace emits exactly two events");

        tx.send(true).expect("signal shutdown");
        let result = tokio::time::timeout(Duration::from_secs(3), worker_task)
            .await
            .expect("worker shutdown timeout")
            .expect("worker task panicked");
        assert!(result.is_ok());
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
