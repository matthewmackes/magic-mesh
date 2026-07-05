//! The **mesh share-session** (EDITOR-COLLAB-2): carry EDITOR-COLLAB-1's CRDT
//! ([`CollabDoc`]) over the Mackes Bus as a peer-to-peer editing session.
//!
//! Design: `docs/design/editor.md` Phase 3 — "Zed's multiplayer, but P2P over
//! the mesh". COLLAB-1 landed the replicated model ([`crate::crdt`]); this unit
//! is the **transport + session glue** that makes two (or more) editors on the
//! mesh edit the same document, with **no cloud** — every frame rides the local
//! Bus spool, which the Bus broker federates to peers over Nebula (the exact
//! delivery `chat`/`clipboard` already use).
//!
//! # What rides the wire
//!
//! Everything is scoped to a mesh [`SessionId`] — one Bus topic,
//! `collab/session/<id>`. A peer's [`CollabSession`] both publishes to and polls
//! that topic; it skips its own echo by matching the sender. The frame set
//! ([`FrameKind`]):
//!
//! * **`Hello`** — join step 1: "here is my [state vector]; send me what I'm
//!   missing." Broadcast when a peer joins (or reconnects).
//! * **`Sync`** — join step 2 (directed at the joiner): the [`CollabDoc::diff`]
//!   answering the joiner's state vector, **plus the answerer's own state
//!   vector** so the joiner can send back anything the answerer lacks (the
//!   symmetric y-sync handshake — the whole reconnect story, no server, no
//!   sequence numbers).
//! * **`Update`** — an incremental [`CollabDoc::take_updates`] payload: one
//!   local edit fanned out to peers, merged with [`CollabDoc::apply_remote`].
//! * **`Presence`** — a peer's display name + cursor/selection + visible
//!   [`Viewport`], so every editor paints the others' carets and a follower can
//!   track the leader's scroll (COLLAB-3; the `viewport` field is
//!   `#[serde(default)]`, so COLLAB-2-era frames without it still decode).
//! * **`Grant`** — the host granting/revoking a guest's [`Access`] (the
//!   permission model below).
//! * **`Leave`** — a peer dropping out (its presence is pruned).
//!
//! [state vector]: CollabDoc::encode_state_vector
//!
//! # Permissions (host + guests, cooperative)
//!
//! A session has exactly one [`Role::Host`] (the peer that opened the buffer and
//! seeded the doc) and any number of [`Role::Guest`]s. Each peer holds an
//! [`Access`] — [`Access::ReadWrite`] or [`Access::ReadOnly`]; the host
//! `grant`s/revokes it and broadcasts the change. A read-only peer's local edits
//! are **refused before they are broadcast** ([`CollabSession::local_insert`] /
//! [`CollabSession::local_remove`] return `false`) — the cooperative half of the
//! model. Hard enforcement (a host that *relays* and can drop a rogue peer's
//! ops) would need the host to sit between peers rather than the shared-spool
//! broadcast this unit uses; that relay is out of scope here and honestly noted
//! rather than faked (§7).
//!
//! # Follow mode (COLLAB-3)
//!
//! [`CollabSession::follow`] pins a known peer as the **followed** collaborator:
//! every later `Presence` frame from that peer surfaces on the poll as
//! [`PollOutcome::follow`] — its cursor/selection + viewport — which the editor
//! surface replays onto its local view ([`crate::follow::apply_follow`] drives
//! the scroll/selection). The standard break idioms are enforced *here*, not
//! left to the UI: any **local edit** ([`CollabSession::local_insert`] /
//! [`CollabSession::local_remove`] — even one refused for read-only access)
//! breaks follow, the surface reports scroll/click/key gestures through
//! [`CollabSession::note_local_input`], and the followed peer **leaving** ends
//! follow with an explicit [`PollOutcome::follow_ended`] edge so the UI clears
//! its "Following …" affordance. The affordance itself is
//! [`crate::follow::follow_banner`].
//!
//! # Why no daemon-side relay
//!
//! The session is a **pure Bus client**, exactly like
//! [`mde_files_egui`-style](crate) `chat`/`clipboard` bridges: it opens a local
//! `Persist`, writes frames, and polls them back. Cross-node delivery is the
//! Bus's job (the broker over Nebula), so COLLAB-2 needs **no** new `mackesd`
//! worker — the editors are the peers and they talk to each other through the
//! Bus they already share.
//!
//! # Testing (§7 — real convergence, no live-bus assertion)
//!
//! The transport is a seam ([`CollabTransport`]) so the tests drive a real
//! **in-process** [`FakeBus`] (it genuinely stores and replays frames — not a
//! behavior mock): two sessions **converge** on concurrent edits, a fresh guest
//! **catches up through the handshake**, presence and permissions propagate. A
//! separate test round-trips the production [`BusTransport`] through a throwaway
//! `Persist` tempdir (a real local Bus, not the live mesh) so that path is
//! proven reachable, never dead. The **live smoke** — two mesh nodes, one hosts
//! a real file, the other joins and both see each other's keystrokes — is gated
//! on the panel wiring (a later unit owns `panel.rs`) and a running 2-node mesh;
//! it is documented, not asserted headless.

// `CollabSession` / `CollabError` / `CollabMessage` / `CollabTransport` all carry
// the module's `collab` domain prefix; stripping it would collide with the
// generic `Session`/`Error`/`Message`/`Transport` names every importer would
// then have to disambiguate. Same call `crdt.rs` makes for `CrdtError`.
#![allow(clippy::module_name_repetitions)]

use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::crdt::{CollabDoc, EditSink, TextEdit};

/// Topic prefix every session's frames ride under. The full topic is
/// `collab/session/<id>` — one topic per [`SessionId`], sitting under the Bus's
/// `collab/` namespace beside `chat/` and `action/vdi/`.
pub const COLLAB_TOPIC_PREFIX: &str = "collab/session/";

// ───────────────────────────── identity ─────────────────────────────

/// A mesh editing-session identity — the scope of one shared document.
///
/// Validated to a safe topic segment on construction (it becomes the last path
/// component of the Bus topic), so `collab/session/<id>` is always a legal
/// topic. Cheap to clone (one `String`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(String);

impl SessionId {
    /// The maximum id length (a topic segment, not free text).
    pub const MAX_LEN: usize = 128;

    /// Build a session id, rejecting anything that would make an unsafe or
    /// ambiguous Bus topic.
    ///
    /// # Errors
    /// [`CollabError::BadSessionId`] when `id` is empty, longer than
    /// [`Self::MAX_LEN`], contains a `..` path-escape, or holds a character
    /// outside `[A-Za-z0-9._-]` (which includes `/`, so an id can never inject a
    /// deeper topic path).
    pub fn new(id: impl Into<String>) -> Result<Self, CollabError> {
        let id = id.into();
        let ok = !id.is_empty()
            && id.len() <= Self::MAX_LEN
            && !id.contains("..")
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        if ok {
            Ok(Self(id))
        } else {
            Err(CollabError::BadSessionId(id))
        }
    }

    /// The id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The Bus topic this session's frames ride: `collab/session/<id>`.
    #[must_use]
    pub fn topic(&self) -> String {
        format!("{COLLAB_TOPIC_PREFIX}{}", self.0)
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Derive a yrs client id (the [`CollabDoc`] site id) from a peer identity.
///
/// yrs's convergence precondition is that every peer's client id is **unique
/// among the session's peers**; each peer hashes *its own* stable mesh identity
/// (hostname / Nebula cert subject) to pick its site id, so two distinct peers
/// land on distinct ids.
///
/// The hash is folded into a **32-bit** range (and forced nonzero): yrs is a
/// port of Yjs, whose client ids are 32-bit (`random.uint32()`), and a
/// full-width `u64` site id with high bits set makes yrs silently fail to merge
/// a remote op — the delta integrates nowhere and the peers diverge. Folding to
/// Yjs's own 32-bit range keeps integration correct; distinct peers still land
/// on distinct ids (a 32-bit collision across a session's handful of peers is
/// the same negligible risk Yjs itself accepts). A production mesh can swap this
/// for the low 32 bits of the node's Nebula certificate fingerprint without
/// touching the session logic.
#[must_use]
pub fn client_id_for(peer: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    peer.hash(&mut hasher);
    (hasher.finish() & 0xFFFF_FFFF) | 1
}

// ───────────────────────────── roles + access ─────────────────────────────

/// A participant's role in the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The peer that opened the buffer and seeded the shared doc; the one
    /// authority that grants/revokes guest [`Access`].
    Host,
    /// A peer that joined an existing session.
    Guest,
}

/// A participant's edit permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// May edit — local edits are broadcast to peers.
    ReadWrite,
    /// May follow along but not edit — local edits are refused before broadcast.
    ReadOnly,
}

impl Access {
    /// Whether this access level permits editing.
    #[must_use]
    pub const fn can_edit(self) -> bool {
        matches!(self, Self::ReadWrite)
    }
}

// ───────────────────────────── presence ─────────────────────────────

/// A remote peer's caret + optional selection, in **char** indices (the
/// [`crate::buffer::Buffer`] / [`TextEdit`] index space — never bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorPos {
    /// The fixed end of the selection (equals `head` for a bare caret).
    pub anchor: usize,
    /// The moving end / caret position.
    pub head: usize,
}

impl CursorPos {
    /// A bare caret at char `at` (no selection).
    #[must_use]
    pub const fn caret(at: usize) -> Self {
        Self {
            anchor: at,
            head: at,
        }
    }

    /// Whether this is a non-empty selection (not just a caret).
    #[must_use]
    pub const fn is_selection(&self) -> bool {
        self.anchor != self.head
    }

    /// The selected char range, normalized so `start <= end`.
    #[must_use]
    pub const fn range(&self) -> Range<usize> {
        if self.anchor <= self.head {
            self.anchor..self.head
        } else {
            self.head..self.anchor
        }
    }
}

/// A peer's **visible line span** (COLLAB-3 follow mode).
///
/// The 0-based buffer lines its editor viewport currently shows
/// (`first_line..=last_line`, inclusive). Broadcast inside [`Presence`] so a
/// follower can track the leader's *scroll*, not just its caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Viewport {
    /// The first (topmost) visible buffer line, 0-based.
    pub first_line: usize,
    /// The last (bottommost) visible buffer line, 0-based, inclusive.
    pub last_line: usize,
}

/// A peer's presence: who they are, where their cursor is, and what they see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presence {
    /// The peer's stable identity (the same string fed to [`client_id_for`]).
    pub peer: String,
    /// A human display name for the caret label.
    pub name: String,
    /// The peer's caret/selection, or `None` when it hasn't reported one yet.
    #[serde(default)]
    pub cursor: Option<CursorPos>,
    /// The peer's visible line span, or `None` when it hasn't reported one yet.
    /// `#[serde(default)]` keeps COLLAB-2-era frames (no field on the wire)
    /// decoding — wire-compatible both ways, no migration.
    #[serde(default)]
    pub viewport: Option<Viewport>,
}

/// What a follower learns about its followed peer from one poll (COLLAB-3).
///
/// The freshest cursor/selection + viewport that peer reported. The editor
/// surface replays it onto the local view ([`crate::follow::apply_follow`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowUpdate {
    /// The followed peer's identity.
    pub peer: String,
    /// The followed peer's display name (drives the "Following …" affordance).
    pub name: String,
    /// The followed peer's caret/selection, when reported.
    pub cursor: Option<CursorPos>,
    /// The followed peer's visible line span, when reported.
    pub viewport: Option<Viewport>,
}

/// What the session knows about one remote peer: its latest [`Presence`] and the
/// [`Access`] the host has granted it (both drive the collaborators UI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePeer {
    /// The peer's latest reported presence.
    pub presence: Presence,
    /// The peer's edit permission as last announced by the host (optimistic
    /// [`Access::ReadWrite`] until a [`FrameKind::Grant`] says otherwise).
    pub access: Access,
}

// ───────────────────────────── wire frames ─────────────────────────────

/// One frame on a session's Bus topic: who sent it, which session, and the body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollabMessage {
    /// The [`SessionId`] string (echoed so a stray cross-topic frame is caught).
    pub session: String,
    /// The sending peer's identity (used to skip our own echo).
    pub from: String,
    /// The frame body.
    pub kind: FrameKind,
}

/// The body of a [`CollabMessage`] — the session protocol verbs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum FrameKind {
    /// Join step 1: my state vector — answer with what I'm missing.
    Hello {
        /// [`CollabDoc::encode_state_vector`] of the joiner.
        #[serde(with = "b64")]
        state_vector: Vec<u8>,
    },
    /// Join step 2 (directed at `to`): the diff answering `to`'s `Hello`, plus my
    /// own state vector so `to` can reply with anything I'm missing.
    Sync {
        /// The peer this answer is for (others ignore its payload).
        to: String,
        /// [`CollabDoc::diff`] — only the ops `to` is missing.
        #[serde(with = "b64")]
        diff: Vec<u8>,
        /// The answerer's own [`CollabDoc::encode_state_vector`].
        #[serde(with = "b64")]
        state_vector: Vec<u8>,
    },
    /// An incremental CRDT update (one local edit) to merge.
    Update {
        /// A [`CollabDoc::take_updates`] payload.
        #[serde(with = "b64")]
        update: Vec<u8>,
    },
    /// A peer's presence (name + cursor/selection).
    Presence {
        /// The reported presence.
        presence: Presence,
    },
    /// The host grants/revokes a peer's edit permission.
    Grant {
        /// The peer whose access changes.
        to: String,
        /// The new access level.
        access: Access,
    },
    /// The sender is leaving the session (prune its presence).
    Leave,
}

/// Base64 (standard) codec for the binary CRDT payloads, so a frame is compact
/// JSON text rather than a `serde` byte-array. Used via `#[serde(with = "b64")]`.
mod b64 {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    const ENGINE: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&ENGINE.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        ENGINE.decode(text).map_err(serde::de::Error::custom)
    }
}

// ───────────────────────────── errors ─────────────────────────────

/// A share-session error surfaced honestly to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollabError {
    /// A session id that isn't a legal Bus topic segment (see [`SessionId::new`]).
    BadSessionId(String),
}

impl fmt::Display for CollabError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadSessionId(id) => write!(f, "invalid collab session id: {id:?}"),
        }
    }
}

impl std::error::Error for CollabError {}

// ───────────────────────────── transport seam ─────────────────────────────

/// The publish/subscribe transport a [`CollabSession`] rides.
///
/// Deliberately dumb — it moves opaque frame **bodies** on a topic and tracks a
/// per-consumer cursor; all framing lives in the session. Production is
/// [`BusTransport`] (the local `Persist`); tests drive [`FakeBus`] (in-process).
pub trait CollabTransport {
    /// Publish `body` onto `topic`. Best-effort: a missing/again-broken Bus is a
    /// silent no-op, never a panic (the honest solo-host state).
    fn publish(&self, topic: &str, body: &str);

    /// Return the frame bodies on `topic` newer than `cursor`, advancing
    /// `cursor` past them. An empty return leaves `cursor` unchanged.
    fn poll(&self, topic: &str, cursor: &mut Option<String>) -> Vec<String>;

    /// The newest message id on `topic`, or `None` when empty — used to prime a
    /// joiner's cursor past history it will instead receive compactly through the
    /// handshake.
    fn tail(&self, topic: &str) -> Option<String>;
}

/// The live Bus-backed transport: a synchronous local `Persist` write/scan on
/// `collab/session/<id>`.
///
/// The same persist-first path the Files chat/mesh bridges take — it holds only
/// the resolved Bus spool dir and opens a fresh `Persist` per call (`Persist`
/// isn't `Send`). A node with no Bus (`None` root) degrades to the honest
/// solo-host no-op.
pub struct BusTransport {
    /// The resolved Bus client spool dir, or `None` when this node has no Bus.
    bus_root: Option<PathBuf>,
}

impl BusTransport {
    /// Resolve the Bus spool dir from the environment (the production path).
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
        }
    }

    /// Construct with an explicit spool root (tests point this at a tempdir, or
    /// `None` to exercise the no-Bus no-op).
    #[must_use]
    pub const fn with_root(bus_root: Option<PathBuf>) -> Self {
        Self { bus_root }
    }

    /// Open a fresh `Persist` on the spool, or `None` when there's no Bus / the
    /// open failed (both degrade to a silent no-op upstream).
    fn open(&self) -> Option<mde_bus::persist::Persist> {
        let root = self.bus_root.clone()?;
        mde_bus::persist::Persist::open(root).ok()
    }
}

impl CollabTransport for BusTransport {
    fn publish(&self, topic: &str, body: &str) {
        let Some(persist) = self.open() else {
            return;
        };
        let _ = persist.write(
            topic,
            mde_bus::hooks::config::Priority::Default,
            None,
            Some(body),
        );
    }

    fn poll(&self, topic: &str, cursor: &mut Option<String>) -> Vec<String> {
        let Some(persist) = self.open() else {
            return Vec::new();
        };
        let Ok(messages) = persist.list_since(topic, cursor.as_deref()) else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(messages.len());
        for msg in messages {
            *cursor = Some(msg.ulid);
            if let Some(body) = msg.body {
                out.push(body);
            }
        }
        out
    }

    fn tail(&self, topic: &str) -> Option<String> {
        self.open()?.latest_ulid(topic).ok().flatten()
    }
}

/// A [`FakeBus`]'s per-topic append-log: `topic -> [(seq, body)]`.
type TopicLog = BTreeMap<String, Vec<(u64, String)>>;

/// An in-process [`CollabTransport`] for tests: a shared append-log per topic.
///
/// Not a behavior mock — it genuinely stores and replays every frame, so two
/// sessions handed the *same* `FakeBus` (it is cheap to clone; the log is
/// shared) converge exactly as they would over the real Bus. A process-global
/// monotonic sequence gives a total frame order the string cursor scans.
#[derive(Clone, Default)]
pub struct FakeBus {
    /// The append-log, shared across every clone.
    log: Arc<Mutex<TopicLog>>,
    /// Process-wide monotonic frame sequence.
    seq: Arc<AtomicU64>,
}

impl FakeBus {
    /// A fresh, empty in-process bus.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl CollabTransport for FakeBus {
    fn publish(&self, topic: &str, body: &str) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let mut guard = self.log.lock().unwrap_or_else(PoisonError::into_inner);
        guard
            .entry(topic.to_string())
            .or_default()
            .push((seq, body.to_string()));
    }

    fn poll(&self, topic: &str, cursor: &mut Option<String>) -> Vec<String> {
        let after = cursor
            .as_deref()
            .and_then(|c| c.parse::<u64>().ok())
            .unwrap_or(0);
        let mut out = Vec::new();
        let mut last = after;
        {
            let guard = self.log.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(entries) = guard.get(topic) {
                for (seq, body) in entries {
                    if *seq > after {
                        out.push(body.clone());
                        last = *seq;
                    }
                }
            }
        }
        if last != after {
            *cursor = Some(last.to_string());
        }
        out
    }

    fn tail(&self, topic: &str) -> Option<String> {
        let guard = self.log.lock().unwrap_or_else(PoisonError::into_inner);
        let tail = guard
            .get(topic)
            .and_then(|e| e.last())
            .map(|(seq, _)| seq.to_string());
        tail
    }
}

// ───────────────────────────── the session ─────────────────────────────

/// What one [`CollabSession::poll`] pump produced for the caller to apply.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PollOutcome {
    /// The char-space script to replay onto the editor's [`crate::buffer::Buffer`]
    /// **in order** (via [`TextEdit::apply_to`]) — the merged remote edits.
    pub edits: Vec<TextEdit>,
    /// A peer's presence or the roster changed → repaint the collaborator carets.
    pub peers_changed: bool,
    /// The host changed **our** [`Access`] → refresh the edit guards / UI.
    pub access_changed: bool,
    /// The followed peer reported fresh presence → drive the local view to track
    /// it (COLLAB-3 follow mode). `None` when not following, or when the
    /// followed peer was silent this poll.
    pub follow: Option<FollowUpdate>,
    /// Follow mode ended **remotely** this poll — the followed peer left the
    /// session. The UI clears its "Following …" affordance on this edge.
    /// (A *locally* broken follow — an edit or [`CollabSession::note_local_input`]
    /// — is known synchronously and never raises this.)
    pub follow_ended: bool,
}

/// A live mesh editing session over a [`CollabDoc`] and a [`CollabTransport`].
///
/// One per shared buffer. The owner (the editor surface) mirrors every local
/// buffer edit through [`Self::local_insert`] / [`Self::local_remove`], flushes
/// once per frame with [`Self::flush`], reports its caret with
/// [`Self::set_cursor`] + [`Self::publish_presence`], and pumps remote traffic
/// with [`Self::poll`] — replaying the returned [`PollOutcome::edits`] onto the
/// buffer.
pub struct CollabSession {
    /// The session scope (its Bus topic).
    session: SessionId,
    /// This peer's stable identity.
    me: String,
    /// This peer's display name (defaults to `me`).
    name: String,
    /// This peer's role.
    role: Role,
    /// This peer's edit permission (the host may change a guest's).
    access: Access,
    /// The replicated document (COLLAB-1).
    doc: CollabDoc,
    /// The transport poll cursor.
    cursor: Option<String>,
    /// Our last-set caret/selection, broadcast by [`Self::publish_presence`].
    self_cursor: Option<CursorPos>,
    /// Our last-set visible line span, broadcast by [`Self::publish_presence`].
    self_viewport: Option<Viewport>,
    /// The peer we are following (COLLAB-3), or `None` when not following.
    following: Option<String>,
    /// Remote peers: presence + granted access, keyed by peer identity.
    peers: BTreeMap<String, RemotePeer>,
    /// Count of frames skipped (undecodable / wrong-session / failed merge) — an
    /// honest observability counter in lieu of a logger in this GUI crate.
    dropped: usize,
}

impl CollabSession {
    /// Start **hosting** a session over `initial_text` (the editor's current
    /// buffer text). The host seeds the shared doc; guests join empty and catch
    /// up through the handshake (bridge contract rule 1 — never re-seed).
    #[must_use]
    pub fn host(session: SessionId, me: impl Into<String>, initial_text: &str) -> Self {
        let me = me.into();
        let doc = CollabDoc::from_text(initial_text, client_id_for(&me));
        Self::assemble(session, me, Role::Host, doc)
    }

    /// **Join** an existing session as a guest: an empty doc that catches up via
    /// [`Self::join`]'s handshake. A guest starts [`Access::ReadWrite`]
    /// (optimistic); the host revokes to [`Access::ReadOnly`] with a `Grant`.
    #[must_use]
    pub fn guest(session: SessionId, me: impl Into<String>) -> Self {
        let me = me.into();
        let doc = CollabDoc::new(client_id_for(&me));
        Self::assemble(session, me, Role::Guest, doc)
    }

    /// Shared field init for [`Self::host`] / [`Self::guest`].
    fn assemble(session: SessionId, me: String, role: Role, doc: CollabDoc) -> Self {
        Self {
            session,
            name: me.clone(),
            me,
            role,
            access: Access::ReadWrite,
            doc,
            cursor: None,
            self_cursor: None,
            self_viewport: None,
            following: None,
            peers: BTreeMap::new(),
            dropped: 0,
        }
    }

    /// Set a friendly display name for this peer's caret (builder).
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// This peer's identity.
    #[must_use]
    pub fn me(&self) -> &str {
        &self.me
    }

    /// This peer's role.
    #[must_use]
    pub const fn role(&self) -> Role {
        self.role
    }

    /// This peer's edit permission.
    #[must_use]
    pub const fn access(&self) -> Access {
        self.access
    }

    /// Whether this peer may currently edit (shorthand for `access().can_edit()`).
    #[must_use]
    pub const fn can_edit(&self) -> bool {
        self.access.can_edit()
    }

    /// The session scope.
    #[must_use]
    pub const fn session_id(&self) -> &SessionId {
        &self.session
    }

    /// The replicated document (read-only view — `to_text` / `len_chars`).
    #[must_use]
    pub const fn doc(&self) -> &CollabDoc {
        &self.doc
    }

    /// The known remote peers (presence + access), keyed by identity — the UI
    /// reads collaborator carets and the RO/RW roster from here.
    #[must_use]
    pub const fn peers(&self) -> &BTreeMap<String, RemotePeer> {
        &self.peers
    }

    /// How many frames this session has skipped as undecodable / wrong-session /
    /// unmergeable (honest observability — a live session should stay at `0`).
    #[must_use]
    pub const fn dropped_frames(&self) -> usize {
        self.dropped
    }

    /// **Announce** ourselves and request catch-up (join step 1).
    ///
    /// Primes the poll cursor past the existing backlog (the handshake catches us
    /// up compactly instead of replaying history), publishes a `Hello` carrying
    /// our state vector, and broadcasts our initial presence. Works for a host
    /// too (a reconnecting host requests anything it missed while away).
    pub fn join(&mut self, transport: &dyn CollabTransport) {
        self.cursor = transport.tail(&self.session.topic());
        let state_vector = self.doc.encode_state_vector();
        self.publish(transport, &FrameKind::Hello { state_vector });
        self.publish_presence(transport);
    }

    /// Mirror a local buffer **insert** into the shared doc (bridge contract rule
    /// 2 — same args as `Buffer::insert`). Returns `false` and does nothing for a
    /// read-only participant (the cooperative permission gate). The queued update
    /// is broadcast on the next [`Self::flush`]. A local edit is local input, so
    /// it breaks follow mode (even a refused read-only edit — the *gesture* is
    /// what breaks follow, the standard idiom).
    pub fn local_insert(&mut self, char_idx: usize, text: &str) -> bool {
        self.note_local_input();
        if !self.access.can_edit() {
            return false;
        }
        self.doc.forward_insert(char_idx, text);
        true
    }

    /// Mirror a local buffer **remove** into the shared doc (bridge contract rule
    /// 2 — same args as `Buffer::remove`). Returns `false` and does nothing for a
    /// read-only participant. Breaks follow mode like [`Self::local_insert`].
    pub fn local_remove(&mut self, range: Range<usize>) -> bool {
        self.note_local_input();
        if !self.access.can_edit() {
            return false;
        }
        self.doc.forward_remove(range);
        true
    }

    /// **Broadcast** every local edit queued since the last flush — the
    /// once-per-frame publish seam. A no-op for a read-only peer (its outbox is
    /// drained and dropped defensively).
    pub fn flush(&mut self, transport: &dyn CollabTransport) {
        let updates = self.doc.take_updates();
        if !self.access.can_edit() {
            return;
        }
        for update in updates {
            self.publish(transport, &FrameKind::Update { update });
        }
    }

    /// Set this peer's caret/selection (broadcast by [`Self::publish_presence`]).
    pub const fn set_cursor(&mut self, cursor: Option<CursorPos>) {
        self.self_cursor = cursor;
    }

    /// Set this peer's visible line span (broadcast by
    /// [`Self::publish_presence`]) — what a follower of *this* peer tracks.
    pub const fn set_viewport(&mut self, viewport: Option<Viewport>) {
        self.self_viewport = viewport;
    }

    /// Broadcast this peer's current presence (name + cursor + viewport) to the
    /// session.
    pub fn publish_presence(&self, transport: &dyn CollabTransport) {
        let presence = Presence {
            peer: self.me.clone(),
            name: self.name.clone(),
            cursor: self.self_cursor,
            viewport: self.self_viewport,
        };
        self.publish(transport, &FrameKind::Presence { presence });
    }

    // ── follow mode (COLLAB-3) ──

    /// Start **following** `peer`: its later `Presence` frames surface as
    /// [`PollOutcome::follow`] for the local view to track. Returns `false`
    /// (and follows nobody) when `peer` isn't in the roster — you can only
    /// follow a collaborator you can see.
    pub fn follow(&mut self, peer: &str) -> bool {
        if self.peers.contains_key(peer) {
            self.following = Some(peer.to_string());
            true
        } else {
            false
        }
    }

    /// Stop following (the explicit affordance click). Idempotent.
    pub fn unfollow(&mut self) {
        self.following = None;
    }

    /// The peer currently being followed, or `None`.
    #[must_use]
    pub fn following(&self) -> Option<&str> {
        self.following.as_deref()
    }

    /// Report a **local input gesture** (scroll, click, caret key) — any local
    /// input breaks follow mode, the standard idiom. Returns whether a follow
    /// was actually broken (so the surface repaints its affordance exactly
    /// once). Local *edits* break follow on their own via
    /// [`Self::local_insert`] / [`Self::local_remove`].
    pub fn note_local_input(&mut self) -> bool {
        self.following.take().is_some()
    }

    /// **Grant** (or revoke) a peer's edit permission — host only. Records the
    /// grant in the local roster and broadcasts it. Returns `false` for a guest
    /// (only the host holds the permission authority).
    pub fn grant(&mut self, peer: &str, access: Access, transport: &dyn CollabTransport) -> bool {
        if self.role != Role::Host {
            return false;
        }
        self.set_peer_access(peer, access);
        self.publish(
            transport,
            &FrameKind::Grant {
                to: peer.to_string(),
                access,
            },
        );
        true
    }

    /// Announce that this peer is **leaving** (peers prune its presence).
    pub fn leave(&self, transport: &dyn CollabTransport) {
        self.publish(transport, &FrameKind::Leave);
    }

    /// **Pump** the transport: apply every new remote frame and report what
    /// changed. The returned [`PollOutcome::edits`] must be replayed onto the
    /// editor buffer (in order) before the next local edit is mirrored.
    pub fn poll(&mut self, transport: &dyn CollabTransport) -> PollOutcome {
        let topic = self.session.topic();
        let bodies = transport.poll(&topic, &mut self.cursor);
        let mut outcome = PollOutcome::default();
        for body in bodies {
            let Ok(msg) = serde_json::from_str::<CollabMessage>(&body) else {
                self.dropped += 1;
                continue;
            };
            if msg.session != self.session.as_str() {
                self.dropped += 1; // a stray frame on the wrong session's topic
                continue;
            }
            if msg.from == self.me {
                continue; // our own echo off the shared spool
            }
            self.handle(&msg, transport, &mut outcome);
        }
        outcome
    }

    /// Dispatch one decoded frame.
    fn handle(
        &mut self,
        msg: &CollabMessage,
        transport: &dyn CollabTransport,
        out: &mut PollOutcome,
    ) {
        match &msg.kind {
            FrameKind::Hello { state_vector } => {
                // Answer with the diff the joiner is missing + our own state
                // vector (symmetric y-sync). Only answer if we actually hold ops
                // the joiner lacks, or we're the host (the content authority) —
                // duplicate answers are idempotent, so multiple holders answering
                // is safe if imperfectly tidy.
                if let Ok(diff) = self.doc.diff(state_vector) {
                    if self.role == Role::Host || !diff.is_empty() {
                        let sv = self.doc.encode_state_vector();
                        self.publish(
                            transport,
                            &FrameKind::Sync {
                                to: msg.from.clone(),
                                diff,
                                state_vector: sv,
                            },
                        );
                    }
                }
                self.note_peer(&msg.from);
                out.peers_changed = true;
            }
            FrameKind::Sync {
                to,
                diff,
                state_vector,
            } => {
                if to == &self.me {
                    match self.doc.apply_remote(diff) {
                        Ok(edits) => out.edits.extend(edits),
                        Err(_) => self.dropped += 1,
                    }
                    // Close the loop: send back anything the answerer is missing
                    // (a terminal `Update`, never another `Sync` — no ping-pong).
                    if let Ok(back) = self.doc.diff(state_vector) {
                        if !back.is_empty() {
                            self.publish(transport, &FrameKind::Update { update: back });
                        }
                    }
                }
            }
            FrameKind::Update { update } => match self.doc.apply_remote(update) {
                Ok(edits) => out.edits.extend(edits),
                Err(_) => self.dropped += 1,
            },
            FrameKind::Presence { presence } => {
                // Follow mode: the followed peer's fresh presence drives the
                // local view. Later frames in the same poll overwrite earlier
                // ones — the follower lands on the freshest report.
                if self.following.as_deref() == Some(msg.from.as_str()) {
                    out.follow = Some(FollowUpdate {
                        peer: presence.peer.clone(),
                        name: presence.name.clone(),
                        cursor: presence.cursor,
                        viewport: presence.viewport,
                    });
                }
                self.upsert_presence(presence.clone());
                out.peers_changed = true;
            }
            FrameKind::Grant { to, access } => {
                if to == &self.me {
                    if self.access != *access {
                        self.access = *access;
                        out.access_changed = true;
                    }
                } else {
                    self.set_peer_access(to, *access);
                    out.peers_changed = true;
                }
            }
            FrameKind::Leave => {
                if self.peers.remove(&msg.from).is_some() {
                    out.peers_changed = true;
                }
                // The followed peer left → follow ends remotely; the UI clears
                // its "Following …" affordance on this edge.
                if self.following.as_deref() == Some(msg.from.as_str()) {
                    self.following = None;
                    out.follow_ended = true;
                }
            }
        }
    }

    /// Publish a frame wrapped in the session envelope. Best-effort — a failed
    /// encode / missing Bus is a silent no-op (never a panic).
    fn publish(&self, transport: &dyn CollabTransport, kind: &FrameKind) {
        let msg = CollabMessage {
            session: self.session.as_str().to_string(),
            from: self.me.clone(),
            kind: kind.clone(),
        };
        if let Ok(body) = serde_json::to_string(&msg) {
            transport.publish(&self.session.topic(), &body);
        }
    }

    /// Note a peer we've heard from (no cursor yet) so it shows in the roster.
    fn note_peer(&mut self, peer: &str) {
        self.peers
            .entry(peer.to_string())
            .or_insert_with(|| RemotePeer {
                presence: Presence {
                    peer: peer.to_string(),
                    name: peer.to_string(),
                    cursor: None,
                    viewport: None,
                },
                access: Access::ReadWrite,
            });
    }

    /// Upsert a peer's presence, preserving any known access grant.
    fn upsert_presence(&mut self, presence: Presence) {
        self.peers
            .entry(presence.peer.clone())
            .and_modify(|p| p.presence = presence.clone())
            .or_insert(RemotePeer {
                presence,
                access: Access::ReadWrite,
            });
    }

    /// Record a peer's access in the roster, preserving any known presence.
    fn set_peer_access(&mut self, peer: &str, access: Access) {
        self.peers
            .entry(peer.to_string())
            .and_modify(|p| p.access = access)
            .or_insert_with(|| RemotePeer {
                presence: Presence {
                    peer: peer.to_string(),
                    name: peer.to_string(),
                    cursor: None,
                    viewport: None,
                },
                access,
            });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Access, CollabError, CollabMessage, CollabSession, CollabTransport, CursorPos, FakeBus,
        FrameKind, Presence, Role, SessionId, Viewport, COLLAB_TOPIC_PREFIX,
    };

    fn sid(id: &str) -> SessionId {
        SessionId::new(id).expect("valid session id")
    }

    /// Replay a poll outcome's edits onto a text `String` (a stand-in for the
    /// editor buffer) — proves the returned script transforms the peer's text.
    fn apply_edits(text: &mut String, edits: &[super::TextEdit]) {
        for edit in edits {
            match edit {
                super::TextEdit::Insert { at, text: t } => {
                    let byte = text.char_indices().nth(*at).map_or(text.len(), |(b, _)| b);
                    text.insert_str(byte, t);
                }
                super::TextEdit::Remove { range } => {
                    let start = text
                        .char_indices()
                        .nth(range.start)
                        .map_or(text.len(), |(b, _)| b);
                    let end = text
                        .char_indices()
                        .nth(range.end)
                        .map_or(text.len(), |(b, _)| b);
                    text.replace_range(start..end, "");
                }
            }
        }
    }

    // ── identity + framing ──

    #[test]
    fn session_id_validates_and_builds_a_safe_topic() {
        assert_eq!(sid("proj-42.md").topic(), "collab/session/proj-42.md");
        assert!(SessionId::new("").is_err(), "empty rejected");
        assert!(
            SessionId::new("a/b").is_err(),
            "slash would inject a topic path"
        );
        assert!(SessionId::new("../escape").is_err(), "path escape rejected");
        assert!(SessionId::new("a b").is_err(), "space rejected");
        assert!(SessionId::new("x".repeat(SessionId::MAX_LEN + 1)).is_err());
        assert!(sid("only-safe_chars.123")
            .topic()
            .starts_with(COLLAB_TOPIC_PREFIX));
        let err = SessionId::new("bad/id").expect_err("a slash id is rejected");
        assert_eq!(err, CollabError::BadSessionId("bad/id".to_string()));
    }

    #[test]
    fn distinct_peers_get_distinct_client_ids() {
        // yrs's convergence precondition — distinct peers, distinct site ids.
        assert_ne!(super::client_id_for("host"), super::client_id_for("guest"));
        assert_ne!(super::client_id_for("nyc3"), super::client_id_for("eagle"));
        // Deterministic per peer.
        assert_eq!(super::client_id_for("host"), super::client_id_for("host"));
        // Folded into Yjs's 32-bit, nonzero range — a full-width u64 site id
        // makes yrs silently drop a remote merge (the regression this locks).
        for peer in [
            "host",
            "guest",
            "nyc3",
            "eagle",
            "a-very-long-peer-identity",
        ] {
            let id = super::client_id_for(peer);
            assert!(
                id != 0 && u32::try_from(id).is_ok(),
                "{peer}: {id} out of 32-bit range"
            );
        }
    }

    #[test]
    fn frames_round_trip_through_base64_json() {
        // The binary CRDT payload survives the base64/JSON envelope intact.
        let msg = CollabMessage {
            session: "s1".into(),
            from: "host".into(),
            kind: FrameKind::Update {
                update: vec![0x00, 0xff, 0x13, 0x37, 0x00],
            },
        };
        let body = serde_json::to_string(&msg).expect("encode");
        assert!(
            body.contains("\"t\":\"update\""),
            "internally tagged: {body}"
        );
        let back: CollabMessage = serde_json::from_str(&body).expect("decode");
        assert_eq!(back, msg);
    }

    // ── the join handshake ──

    #[test]
    fn guest_join_handshake_catches_up_the_hosts_document() {
        let bus = FakeBus::new();
        let mut host = CollabSession::host(sid("proj"), "host", "fn main() {}\n");
        let mut guest = CollabSession::guest(sid("proj"), "guest");

        // Guest announces + requests catch-up.
        guest.join(&bus);
        // Host sees the Hello and answers with a directed Sync (the full doc).
        let host_out = host.poll(&bus);
        assert!(
            host_out.edits.is_empty(),
            "the host applies nothing on a join"
        );
        assert!(host_out.peers_changed, "the host now knows the joiner");
        // Guest applies the Sync → its (empty) doc becomes the host's text.
        let guest_out = guest.poll(&bus);
        assert_eq!(guest.doc().to_text(), "fn main() {}\n");
        // …and the returned script rebuilds the same text on a fresh buffer.
        let mut buffer = String::new();
        apply_edits(&mut buffer, &guest_out.edits);
        assert_eq!(buffer, "fn main() {}\n");
        assert_eq!(host.dropped_frames(), 0);
        assert_eq!(guest.dropped_frames(), 0);
    }

    #[test]
    fn join_catches_up_history_the_primed_cursor_skipped() {
        // The host edits BEFORE the guest joins; the guest primes past those
        // Update frames, yet still converges via the handshake diff.
        let bus = FakeBus::new();
        let mut host = CollabSession::host(sid("doc"), "host", "start ");
        assert!(host.local_insert(6, "middle "));
        host.flush(&bus);
        let end = host.doc().len_chars();
        assert!(host.local_insert(end, "end"));
        host.flush(&bus);
        assert_eq!(host.doc().to_text(), "start middle end");

        let mut guest = CollabSession::guest(sid("doc"), "guest");
        guest.join(&bus); // primes cursor PAST both Update frames
        host.poll(&bus); // answers the Hello with the full-state Sync
        guest.poll(&bus); // applies the Sync
        assert_eq!(guest.doc().to_text(), host.doc().to_text());
        assert_eq!(guest.dropped_frames(), 0);
    }

    // ── convergence over the transport ──

    #[test]
    fn two_sessions_converge_on_concurrent_edits() {
        let bus = FakeBus::new();
        let mut host = CollabSession::host(sid("c"), "host", "base\n");
        let mut guest = CollabSession::guest(sid("c"), "guest");
        guest.join(&bus);
        host.poll(&bus);
        guest.poll(&bus);
        assert_eq!(guest.doc().to_text(), "base\n");

        // Both edit "offline" (no interleaving poll), then exchange.
        assert!(host.local_insert(0, "H: "));
        host.flush(&bus);
        let end = guest.doc().len_chars();
        assert!(guest.local_insert(end, "G: tail\n"));
        guest.flush(&bus);

        // Each pumps the other's update → identical text on both peers.
        host.poll(&bus);
        guest.poll(&bus);
        assert_eq!(
            host.doc().to_text(),
            guest.doc().to_text(),
            "peers converge"
        );
        assert_eq!(host.doc().to_text(), "H: base\nG: tail\n");
        assert_eq!(host.dropped_frames() + guest.dropped_frames(), 0);
    }

    #[test]
    fn a_replayed_buffer_tracks_the_shared_doc() {
        // The guest's editor buffer, rebuilt purely from poll outcomes, equals
        // the shared doc — the end-to-end bridge the panel wiring will drive.
        let bus = FakeBus::new();
        let mut host = CollabSession::host(sid("b"), "host", "one\n");
        let mut guest = CollabSession::guest(sid("b"), "guest");
        let mut buffer = String::new();

        guest.join(&bus);
        host.poll(&bus);
        apply_edits(&mut buffer, &guest.poll(&bus).edits); // seed
        assert_eq!(buffer, "one\n");

        let end = host.doc().len_chars();
        assert!(host.local_insert(end, "two\n"));
        host.flush(&bus);
        apply_edits(&mut buffer, &guest.poll(&bus).edits); // incremental
        assert_eq!(buffer, "one\ntwo\n");
        assert_eq!(buffer, guest.doc().to_text());
    }

    #[test]
    fn three_peers_all_converge() {
        let bus = FakeBus::new();
        let mut host = CollabSession::host(sid("m"), "host", "shared\n");
        let mut g1 = CollabSession::guest(sid("m"), "g1");
        let mut g2 = CollabSession::guest(sid("m"), "g2");

        // g1 joins and catches up.
        g1.join(&bus);
        host.poll(&bus);
        g1.poll(&bus);
        // g2 joins later; both host and g1 may answer (idempotent).
        g2.join(&bus);
        host.poll(&bus);
        g1.poll(&bus);
        g2.poll(&bus);

        // Everyone edits, then gossip drains.
        assert!(host.local_insert(0, "h| "));
        host.flush(&bus);
        assert!(g1.local_insert(0, "1| "));
        g1.flush(&bus);
        let end = g2.doc().len_chars();
        assert!(g2.local_insert(end, "g2-line\n"));
        g2.flush(&bus);
        for s in [&mut host, &mut g1, &mut g2] {
            s.poll(&bus);
        }

        assert_eq!(host.doc().to_text(), g1.doc().to_text(), "host/g1 diverged");
        assert_eq!(g1.doc().to_text(), g2.doc().to_text(), "g1/g2 diverged");
        let merged = host.doc().to_text();
        for marker in ["h| ", "1| ", "shared\n", "g2-line\n"] {
            assert!(merged.contains(marker), "{marker:?} lost in {merged:?}");
        }
    }

    // ── presence ──

    #[test]
    fn presence_propagates_cursor_and_name() {
        let bus = FakeBus::new();
        let mut host = CollabSession::host(sid("p"), "host", "line\n");
        let mut guest = CollabSession::guest(sid("p"), "guest").with_name("Ada");
        guest.join(&bus); // publishes presence with no cursor yet
        let out = host.poll(&bus);
        assert!(out.peers_changed);
        assert_eq!(host.peers()["guest"].presence.name, "Ada");

        // Guest moves its caret and republishes.
        guest.set_cursor(Some(CursorPos::caret(3)));
        guest.publish_presence(&bus);
        let out = host.poll(&bus);
        assert!(out.peers_changed);
        let seen = host.peers()["guest"]
            .presence
            .cursor
            .expect("a reported cursor");
        assert_eq!(seen, CursorPos::caret(3));
        assert!(!seen.is_selection());

        // A selection presence round-trips its normalized range.
        guest.set_cursor(Some(CursorPos { anchor: 4, head: 1 }));
        guest.publish_presence(&bus);
        host.poll(&bus);
        let sel = host.peers()["guest"].presence.cursor.expect("selection");
        assert!(sel.is_selection());
        assert_eq!(sel.range(), 1..4);
    }

    #[test]
    fn leave_prunes_a_peers_presence() {
        let bus = FakeBus::new();
        let mut host = CollabSession::host(sid("lv"), "host", "x\n");
        let mut guest = CollabSession::guest(sid("lv"), "guest");
        guest.join(&bus);
        host.poll(&bus);
        assert!(host.peers().contains_key("guest"));
        guest.leave(&bus);
        let out = host.poll(&bus);
        assert!(out.peers_changed);
        assert!(!host.peers().contains_key("guest"), "left peer pruned");
    }

    // ── follow mode (COLLAB-3) ──

    #[test]
    fn presence_carries_the_viewport_and_collab2_frames_still_decode() {
        let bus = FakeBus::new();
        let mut host = CollabSession::host(sid("vp"), "host", "a\nb\nc\nd\n");
        let mut guest = CollabSession::guest(sid("vp"), "guest");
        guest.join(&bus);
        host.poll(&bus);

        // The guest reports what it sees; the host's roster reflects it.
        guest.set_viewport(Some(Viewport {
            first_line: 1,
            last_line: 3,
        }));
        guest.publish_presence(&bus);
        host.poll(&bus);
        let seen = host.peers()["guest"]
            .presence
            .viewport
            .expect("a reported viewport");
        assert_eq!(seen.first_line, 1);
        assert_eq!(seen.last_line, 3);

        // Wire-compat lock: a COLLAB-2-era presence frame (no `viewport` field
        // on the wire) still decodes — the field defaults to None, no
        // migration, mixed-build sessions keep working.
        let old_frame = r#"{"session":"vp","from":"legacy","kind":{"t":"presence","presence":{"peer":"legacy","name":"Old Build","cursor":{"anchor":2,"head":2}}}}"#;
        let msg: CollabMessage = serde_json::from_str(old_frame).expect("COLLAB-2 frame decodes");
        let FrameKind::Presence { presence } = msg.kind else {
            unreachable!("a presence frame decodes to FrameKind::Presence")
        };
        assert_eq!(presence.cursor, Some(CursorPos::caret(2)));
        assert_eq!(presence.viewport, None, "absent field defaults");
    }

    #[test]
    fn follow_requires_a_known_peer() {
        let bus = FakeBus::new();
        let mut host = CollabSession::host(sid("f0"), "host", "x\n");
        assert!(
            !host.follow("stranger"),
            "you can only follow a collaborator you can see"
        );
        assert_eq!(host.following(), None);

        let mut guest = CollabSession::guest(sid("f0"), "guest");
        guest.join(&bus);
        host.poll(&bus);
        assert!(host.follow("guest"), "a rostered peer is followable");
        assert_eq!(host.following(), Some("guest"));
        host.unfollow();
        assert_eq!(host.following(), None);
    }

    #[test]
    fn follow_surfaces_only_the_followed_peers_presence() {
        let bus = FakeBus::new();
        let mut host = CollabSession::host(sid("f1"), "host", "one\ntwo\nthree\n");
        let mut g1 = CollabSession::guest(sid("f1"), "g1").with_name("Ada");
        let mut g2 = CollabSession::guest(sid("f1"), "g2");
        g1.join(&bus);
        g2.join(&bus);
        host.poll(&bus);

        assert!(host.follow("g1"));

        // Both guests report presence; only the followed one drives the view.
        g1.set_cursor(Some(CursorPos { anchor: 1, head: 5 }));
        g1.set_viewport(Some(Viewport {
            first_line: 0,
            last_line: 2,
        }));
        g1.publish_presence(&bus);
        g2.set_cursor(Some(CursorPos::caret(9)));
        g2.publish_presence(&bus);

        let out = host.poll(&bus);
        let follow = out.follow.expect("the followed peer's presence surfaced");
        assert_eq!(follow.peer, "g1");
        assert_eq!(follow.name, "Ada", "the display name drives the affordance");
        assert_eq!(follow.cursor, Some(CursorPos { anchor: 1, head: 5 }));
        assert_eq!(
            follow.viewport,
            Some(Viewport {
                first_line: 0,
                last_line: 2
            })
        );
        assert!(!out.follow_ended);

        // A quiet followed peer surfaces nothing next poll (no fabricated pos).
        let out = host.poll(&bus);
        assert_eq!(out.follow, None);
    }

    #[test]
    fn any_local_input_breaks_follow() {
        let bus = FakeBus::new();
        let mut host = CollabSession::host(sid("f2"), "host", "text\n");
        let mut guest = CollabSession::guest(sid("f2"), "guest");
        guest.join(&bus);
        host.poll(&bus);

        // A scroll/click/key gesture breaks follow exactly once.
        assert!(host.follow("guest"));
        assert!(host.note_local_input(), "the gesture broke the follow");
        assert_eq!(host.following(), None);
        assert!(!host.note_local_input(), "already broken — no double edge");

        // A local edit breaks follow on its own.
        assert!(host.follow("guest"));
        assert!(host.local_insert(0, "typed "));
        assert_eq!(host.following(), None, "an edit is local input");

        // Even a REFUSED read-only edit is a local gesture that breaks follow.
        // (The host publishes presence so the guest can roster — and follow — it.)
        host.publish_presence(&bus);
        guest.poll(&bus);
        assert!(guest.follow("host"));
        assert!(host.grant("guest", Access::ReadOnly, &bus));
        guest.poll(&bus);
        assert!(guest.follow("host"), "still followable while read-only");
        assert!(!guest.local_insert(0, "nope"), "RO edit refused");
        assert_eq!(guest.following(), None, "the gesture still broke follow");
    }

    #[test]
    fn the_followed_peer_leaving_ends_follow_with_an_edge() {
        let bus = FakeBus::new();
        let mut host = CollabSession::host(sid("f3"), "host", "x\n");
        let mut guest = CollabSession::guest(sid("f3"), "guest");
        guest.join(&bus);
        host.poll(&bus);
        assert!(host.follow("guest"));

        guest.leave(&bus);
        let out = host.poll(&bus);
        assert!(out.follow_ended, "the remote end raises the edge");
        assert_eq!(host.following(), None);

        // The edge is one-shot: the next poll is quiet.
        let out = host.poll(&bus);
        assert!(!out.follow_ended);
    }

    // ── permissions ──

    #[test]
    fn host_can_revoke_a_guest_to_read_only_and_the_guest_obeys() {
        let bus = FakeBus::new();
        let mut host = CollabSession::host(sid("perm"), "host", "doc\n");
        let mut guest = CollabSession::guest(sid("perm"), "guest");
        guest.join(&bus);
        host.poll(&bus);
        guest.poll(&bus);
        assert!(guest.can_edit(), "a guest joins read-write");

        // Host revokes.
        assert!(host.grant("guest", Access::ReadOnly, &bus));
        let out = guest.poll(&bus);
        assert!(out.access_changed, "the guest learns its access changed");
        assert_eq!(guest.access(), Access::ReadOnly);
        assert!(!guest.can_edit());

        // A read-only guest's local edit is refused and never broadcast.
        assert!(!guest.local_insert(0, "sneaky "), "RO edit refused");
        guest.flush(&bus);
        let host_out = host.poll(&bus);
        assert!(host_out.edits.is_empty(), "no edit crossed from a RO guest");
        assert_eq!(host.doc().to_text(), "doc\n");
        // The host's roster reflects the revocation too.
        assert_eq!(host.peers()["guest"].access, Access::ReadOnly);

        // Re-granting restores editing.
        assert!(host.grant("guest", Access::ReadWrite, &bus));
        guest.poll(&bus);
        assert!(guest.can_edit());
        assert!(guest.local_insert(0, "ok "));
        guest.flush(&bus);
        host.poll(&bus);
        assert_eq!(host.doc().to_text(), "ok doc\n");
    }

    #[test]
    fn a_guest_cannot_grant() {
        let bus = FakeBus::new();
        let mut guest = CollabSession::guest(sid("g"), "guest");
        assert!(
            !guest.grant("other", Access::ReadOnly, &bus),
            "guests hold no authority"
        );
        assert_eq!(guest.role(), Role::Guest);
    }

    // ── honesty ──

    #[test]
    fn a_malformed_frame_is_counted_and_never_panics() {
        let bus = FakeBus::new();
        let mut host = CollabSession::host(sid("h"), "host", "safe\n");
        // Inject raw garbage + a wrong-session frame directly onto the topic.
        bus.publish(&sid("h").topic(), "this is not json");
        let stray = CollabMessage {
            session: "someone-else".into(),
            from: "guest".into(),
            kind: FrameKind::Presence {
                presence: Presence {
                    peer: "guest".into(),
                    name: "g".into(),
                    cursor: None,
                    viewport: None,
                },
            },
        };
        bus.publish(
            &sid("h").topic(),
            &serde_json::to_string(&stray).expect("encode"),
        );
        let out = host.poll(&bus);
        assert!(out.edits.is_empty());
        assert_eq!(
            host.dropped_frames(),
            2,
            "both bad frames counted, none applied"
        );
        assert_eq!(host.doc().to_text(), "safe\n", "doc untouched");
    }

    // ── the production BusTransport over a real (local) Persist ──

    #[test]
    fn bus_transport_round_trips_through_a_persist_tempdir() {
        // Proves the live BusTransport path is reachable — a real Persist on a
        // throwaway dir (NOT the live mesh bus): two sessions on the same spool
        // converge exactly as over FakeBus. §7: no live-bus assertion, a real
        // local store.
        use super::BusTransport;
        let tmp = tempfile::tempdir().expect("tempdir");
        let bus = BusTransport::with_root(Some(tmp.path().to_path_buf()));

        let mut host = CollabSession::host(sid("rt"), "host", "hello mesh\n");
        let mut guest = CollabSession::guest(sid("rt"), "guest");
        guest.join(&bus);
        host.poll(&bus);
        guest.poll(&bus);
        assert_eq!(
            guest.doc().to_text(),
            "hello mesh\n",
            "guest caught up over the Bus"
        );

        let end = guest.doc().len_chars();
        assert!(guest.local_insert(end, "guest line\n"));
        guest.flush(&bus);
        host.poll(&bus);
        assert_eq!(host.doc().to_text(), "hello mesh\nguest line\n");
        assert_eq!(host.dropped_frames() + guest.dropped_frames(), 0);
    }

    #[test]
    fn no_bus_transport_is_a_silent_no_op() {
        // The honest solo-host path: no Bus dir → publish/poll do nothing, no panic.
        use super::BusTransport;
        let bus = BusTransport::with_root(None);
        let mut host = CollabSession::host(sid("solo"), "host", "alone\n");
        host.join(&bus);
        let out = host.poll(&bus);
        assert!(out.edits.is_empty() && !out.peers_changed);
        assert_eq!(host.doc().to_text(), "alone\n");
    }
}
