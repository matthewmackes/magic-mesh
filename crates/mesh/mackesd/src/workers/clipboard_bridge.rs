//! E12-9 — `clipboard_bridge`: the mackesd **VDI clipboard bridge** worker.
//!
//! The first of the E12-9 client↔VM bridges. Where [`super::session_broker`]
//! (E12-5b) *tracks* which peer serves which VM to which client and
//! [`super::session_roaming`] (E12-8) makes those desktops *follow the user*, this
//! worker relays **clipboard content** between a VDI client and the VM desktop it
//! is connected to, over the mesh Bus — a copy on the client becomes a paste in the
//! guest (and, once the capture half lands, vice-versa).
//!
//! ## Scope (a buildable slice)
//!
//! This unit ships the pure bridge model + the relay worker + an **injectable
//! clipboard-access seam**; the actual OS/VM-guest clipboard read/write is gated
//! behind that seam. Everything a clip flows through is here and green:
//!
//! - The **pure core** is fully unit-tested with no bus and no clock: [`relay`]
//!   (the per-direction Forward/Drop/Truncate decision under a [`ClipboardPolicy`]),
//!   [`fold_latest`] (the latest-clipboard-per-session fold, the same latest-wins
//!   shape as [`super::scheduler::fold_capacity`]), and the [`ClipPayload`] size
//!   model (a [`MAX_CLIP_BYTES`] ceiling — an oversized payload is refused with a
//!   typed [`ClipboardError`], never relayed unbounded).
//! - The sole outward seam is the injectable [`ClipboardAccess`] trait
//!   (`read_local` / `write_local`). Production wires [`OsClipboardAccess`], whose
//!   methods return a typed [`ClipboardAccessError::IntegrationGated`] naming
//!   exactly what the live call needs (the SPICE/RDP vdagent or `wl-clipboard`
//!   bridge into the connected VM desktop) — never a fake success (§7-legal, exactly
//!   like [`super::session_broker::MeshSessionStore`]). A test fake drives the whole
//!   drain → policy → relay pipeline without a live guest.
//!
//! ## Not leader-gated (unlike the broker)
//!
//! Clipboard relay is **per-session and node-local**, not cluster-consensus: the
//! node serving a session must apply that session's clips to *its* guest, so every
//! node runs the bridge (rank-0-default, like [`super::session_broker`]) and there
//! is no shared plane to converge and hence no election. The broker is leader-gated
//! because it writes the *shared* roaming-session directory; a clip has no such
//! shared write, so gating it would wrongly silence every non-leader node's relay.
//!
//! ## Reused types (no parallel session model — §6 glue)
//!
//! A clip is scoped to a live VDI session by its [`SessionId`] — imported verbatim
//! from [`super::session_broker`], the very identity the broker mints and roams, so
//! the bridge never invents a parallel session key.

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use mde_bus::persist::Persist;

use super::session_broker::SessionId;
use super::{ShutdownToken, Worker};

/// Bus topic the worker drains for clipboard-relay events.
///
/// Host-agnostic — the shell (client copy) or a guest-agent capture publishes a
/// [`ClipboardEvent`] here and the node serving the session applies it to its local
/// clipboard endpoint. Sits beside the broker's `action/vdi/session` and the
/// roaming plane's `action/vdi/roaming` under the shared `action/vdi/` namespace.
pub const ACTION_TOPIC: &str = "action/vdi/clipboard";

/// The hard ceiling on a single clip's payload, in bytes (1 MiB).
///
/// Clipboard content is untrusted and unbounded in principle; the bridge refuses to
/// relay a payload over this ceiling ([`ClipPayload::checked`] / [`parse_event`]
/// reject it with a typed [`ClipboardError::PayloadTooLarge`], and [`relay`]
/// defensively truncates to it). Generous for the text-first slice; a later
/// image/rich-text format may raise or per-format it.
pub const MAX_CLIP_BYTES: usize = 1024 * 1024;

/// Relay cadence.
///
/// The bus read is a cheap local log scan and a copy/paste is a slow, human-paced
/// event, so a 2 s poll is responsive without spinning (the same cadence
/// [`super::session_broker`] / [`super::scheduler`] drain at).
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

// ───────────────────────────── data model ─────────────────────────────

/// The MIME-tagged format of a clip's payload.
///
/// Text is the only format this slice carries; the tag is modeled now so
/// image/rich-text drop in as additional variants later (each with its own
/// [`ClipFormat::mime`]) without reshaping [`ClipPayload`] or the wire event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipFormat {
    /// UTF-8 plain text (`text/plain;charset=utf-8`).
    Text,
}

impl ClipFormat {
    /// The MIME type this format advertises on the wire — the extension point a
    /// future image/rich-text variant plugs into.
    #[must_use]
    pub const fn mime(self) -> &'static str {
        match self {
            Self::Text => "text/plain;charset=utf-8",
        }
    }
}

/// Which way a clip is flowing between the two ends of a VDI session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipDirection {
    /// The client copied — apply the clip into the VM guest's clipboard (paste on
    /// the remote desktop).
    ClientToGuest,
    /// The guest copied — surface the clip on the client's local clipboard.
    GuestToClient,
}

impl ClipDirection {
    /// The reverse direction (for one-way policy reasoning + a future echo path).
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::ClientToGuest => Self::GuestToClient,
            Self::GuestToClient => Self::ClientToGuest,
        }
    }
}

/// A typed clipboard-model failure (distinct from the [`ClipboardAccessError`] the
/// live seam returns).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardError {
    /// The payload exceeds [`MAX_CLIP_BYTES`] — refused rather than relayed
    /// unbounded (enforced at the trust boundary by [`parse_event`] /
    /// [`ClipPayload::checked`]).
    PayloadTooLarge {
        /// The offending payload length in bytes.
        len: usize,
        /// The ceiling it blew past ([`MAX_CLIP_BYTES`]).
        max: usize,
    },
}

impl std::fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PayloadTooLarge { len, max } => {
                write!(f, "clipboard payload too large: {len} B > {max} B ceiling")
            }
        }
    }
}

impl std::error::Error for ClipboardError {}

/// One clip's content: a [`ClipFormat`] tag plus the bytes (text, for this slice).
///
/// The size ceiling ([`MAX_CLIP_BYTES`]) is enforced at the trust boundary via
/// [`Self::checked`]; direct construction ([`Self::text`], public fields) is
/// unchecked for internal callers that already hold bounded content, exactly as the
/// broker's [`super::session_broker::VdiSession`] is directly constructible.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClipPayload {
    /// The payload's format.
    pub format: ClipFormat,
    /// The clip content (UTF-8 text for [`ClipFormat::Text`]).
    pub content: String,
}

impl ClipPayload {
    /// A text clip (unchecked — for internal callers holding known-bounded content).
    #[must_use]
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            format: ClipFormat::Text,
            content: content.into(),
        }
    }

    /// The payload length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.content.len()
    }

    /// Whether the payload is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    /// Verify the payload is within the [`MAX_CLIP_BYTES`] ceiling.
    ///
    /// # Errors
    /// [`ClipboardError::PayloadTooLarge`] when it isn't — the caller must not relay
    /// an unbounded clip.
    pub fn ensure_within_ceiling(&self) -> Result<(), ClipboardError> {
        let len = self.content.len();
        if len > MAX_CLIP_BYTES {
            return Err(ClipboardError::PayloadTooLarge {
                len,
                max: MAX_CLIP_BYTES,
            });
        }
        Ok(())
    }

    /// Build a payload, refusing one over the [`MAX_CLIP_BYTES`] ceiling.
    ///
    /// # Errors
    /// [`ClipboardError::PayloadTooLarge`] — the validated constructor [`parse_event`]
    /// routes untrusted wire content through.
    pub fn checked(format: ClipFormat, content: String) -> Result<Self, ClipboardError> {
        let p = Self { format, content };
        p.ensure_within_ceiling()?;
        Ok(p)
    }

    /// A copy truncated to at most `max` bytes, cut on a UTF-8 char boundary so the
    /// text stays valid (the relay's Truncate outcome).
    #[must_use]
    pub fn truncated(&self, max: usize) -> Self {
        let mut end = max.min(self.content.len());
        while end > 0 && !self.content.is_char_boundary(end) {
            end -= 1;
        }
        Self {
            format: self.format,
            content: self.content[..end].to_string(),
        }
    }
}

/// A clipboard-relay event drained off [`ACTION_TOPIC`] — the wire verb the shell /
/// guest-agent publishes, scoped to a live VDI [`SessionId`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClipboardEvent {
    /// The session this clip belongs to (a broker [`SessionId`]).
    pub session_id: SessionId,
    /// Which way the clip is flowing.
    pub direction: ClipDirection,
    /// The clip content.
    pub payload: ClipPayload,
}

/// Parse + validate a [`ClipboardEvent`] body off the bus.
///
/// Enforces the [`MAX_CLIP_BYTES`] ceiling at the trust boundary — an oversized clip
/// is refused here so the worker never relays it (it is dropped with a warn).
///
/// # Errors
/// A human-readable message on malformed JSON, an unknown enum tag, or an oversized
/// payload ([`ClipboardError::PayloadTooLarge`]).
pub fn parse_event(body: &str) -> Result<ClipboardEvent, String> {
    let ev: ClipboardEvent =
        serde_json::from_str(body).map_err(|e| format!("malformed clipboard event: {e}"))?;
    ev.payload
        .ensure_within_ceiling()
        .map_err(|e| format!("clipboard event rejected: {e}"))?;
    Ok(ev)
}

// ───────────────────────────── policy ─────────────────────────────

/// The per-session relay policy the [`relay`] decision consults.
///
/// Two dimensions the operator/session owner controls: a **per-session allow/deny**
/// (with a default for un-listed sessions) and an optional **one-way** restriction
/// (relay only client→guest, or only guest→client — e.g. let a user paste *into* a
/// VM but block the VM exfiltrating to the client). An optional per-policy soft cap
/// tightens the [`MAX_CLIP_BYTES`] ceiling into a Truncate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardPolicy {
    /// Explicit per-session allow (`true`) / deny (`false`) overrides.
    sessions: BTreeMap<SessionId, bool>,
    /// The verdict for a session not in `sessions`.
    default_allow: bool,
    /// If `Some(d)`, only direction `d` is relayed; the other is dropped.
    one_way: Option<ClipDirection>,
    /// An optional soft cap (≤ [`MAX_CLIP_BYTES`]); a longer clip is Truncated.
    truncate_over_bytes: Option<usize>,
}

impl ClipboardPolicy {
    /// The permissive default: every session, both directions, bounded only by the
    /// hard [`MAX_CLIP_BYTES`] ceiling. The bridge relays by default; the operator
    /// tightens with the builders below.
    #[must_use]
    pub const fn allow_all() -> Self {
        Self {
            sessions: BTreeMap::new(),
            default_allow: true,
            one_way: None,
            truncate_over_bytes: None,
        }
    }

    /// The locked-down default: no session relays until explicitly allowed.
    #[must_use]
    pub const fn deny_all() -> Self {
        Self {
            sessions: BTreeMap::new(),
            default_allow: false,
            one_way: None,
            truncate_over_bytes: None,
        }
    }

    /// Explicitly allow `session` (overrides the default).
    #[must_use]
    pub fn allow_session(mut self, session: impl Into<SessionId>) -> Self {
        self.sessions.insert(session.into(), true);
        self
    }

    /// Explicitly deny `session` (overrides the default).
    #[must_use]
    pub fn deny_session(mut self, session: impl Into<SessionId>) -> Self {
        self.sessions.insert(session.into(), false);
        self
    }

    /// Restrict the bridge to a single direction.
    #[must_use]
    pub const fn one_way(mut self, direction: ClipDirection) -> Self {
        self.one_way = Some(direction);
        self
    }

    /// Set a soft byte cap that Truncates longer clips (clamped to the hard ceiling).
    #[must_use]
    pub const fn truncate_over(mut self, bytes: usize) -> Self {
        self.truncate_over_bytes = Some(bytes);
        self
    }

    /// Whether `session` may relay at all.
    #[must_use]
    pub fn allows_session(&self, session: &str) -> bool {
        self.sessions
            .get(session)
            .copied()
            .unwrap_or(self.default_allow)
    }

    /// Whether `direction` is permitted (honors the one-way restriction).
    #[must_use]
    pub fn allows_direction(&self, direction: ClipDirection) -> bool {
        self.one_way.is_none_or(|only| only == direction)
    }

    /// The effective byte cap: the soft cap tightened against the hard
    /// [`MAX_CLIP_BYTES`] ceiling (a clip over this is Truncated by [`relay`]).
    #[must_use]
    pub fn effective_cap(&self) -> usize {
        self.truncate_over_bytes
            .unwrap_or(MAX_CLIP_BYTES)
            .min(MAX_CLIP_BYTES)
    }
}

// ───────────────────────────── pure: relay decision ─────────────────────────────

/// Why [`relay`] dropped a clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    /// The session is denied by the [`ClipboardPolicy`].
    SessionDenied,
    /// The clip's direction is blocked by a one-way [`ClipboardPolicy`].
    DirectionBlocked,
}

/// The per-clip relay decision the worker applies through the seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayDecision {
    /// Relay the payload verbatim.
    Forward(ClipPayload),
    /// Relay a size-bounded copy (over the policy's effective cap / the hard ceiling).
    Truncate(ClipPayload),
    /// Do not relay — policy refused it.
    Drop(DropReason),
}

/// The pure relay decision: apply `policy` to `event`.
///
/// - A denied session → [`RelayDecision::Drop`] ([`DropReason::SessionDenied`]).
/// - A direction blocked by a one-way policy → `Drop` ([`DropReason::DirectionBlocked`]).
/// - A payload over the policy's [`ClipboardPolicy::effective_cap`] (the soft cap
///   tightened against [`MAX_CLIP_BYTES`]) → [`RelayDecision::Truncate`] of a bounded
///   copy — so the bridge never forwards an unbounded clip even if handed one directly.
/// - Otherwise → [`RelayDecision::Forward`] verbatim.
///
/// No I/O and no clock — fully unit-testable.
#[must_use]
pub fn relay(event: &ClipboardEvent, policy: &ClipboardPolicy) -> RelayDecision {
    if !policy.allows_session(&event.session_id) {
        return RelayDecision::Drop(DropReason::SessionDenied);
    }
    if !policy.allows_direction(event.direction) {
        return RelayDecision::Drop(DropReason::DirectionBlocked);
    }
    let cap = policy.effective_cap();
    if event.payload.len() > cap {
        return RelayDecision::Truncate(event.payload.truncated(cap));
    }
    RelayDecision::Forward(event.payload.clone())
}

// ───────────────────────────── pure: latest fold ─────────────────────────────

/// Fold a stream of [`ClipboardEvent`]s into a latest-wins-by-session clipboard map.
///
/// A later event for the same session overwrites earlier ones — exactly the shape of
/// [`super::scheduler::fold_capacity`]. The result is the current clipboard for each
/// session, regardless of which end last set it (a session's clipboard is shared
/// between its client and guest). Deterministic (id-keyed) and clock-free.
#[must_use]
pub fn fold_latest<'a>(
    events: impl IntoIterator<Item = &'a ClipboardEvent>,
) -> BTreeMap<SessionId, ClipPayload> {
    let mut map = BTreeMap::new();
    for e in events {
        map.insert(e.session_id.clone(), e.payload.clone());
    }
    map
}

// ───────────────────────────── access seam ─────────────────────────────

/// A typed failure from the [`ClipboardAccess`] seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardAccessError {
    /// The live OS/guest clipboard channel isn't wired in this build/environment —
    /// names the op + what is missing. §7-legal: a real method returning a real typed
    /// error, exactly as [`super::session_broker::MeshSessionStore`] does, never a
    /// fake success.
    IntegrationGated {
        /// Which seam op (`read_local` / `write_local`).
        op: &'static str,
        /// What the live call needs before it can run.
        reason: String,
    },
    /// A seam op failed for a concrete runtime reason.
    Failed {
        /// Which seam op failed.
        op: &'static str,
        /// The failure detail.
        reason: String,
    },
}

impl std::fmt::Display for ClipboardAccessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IntegrationGated { op, reason } => {
                write!(f, "{op}: integration-gated — {reason}")
            }
            Self::Failed { op, reason } => write!(f, "{op}: {reason}"),
        }
    }
}

impl std::error::Error for ClipboardAccessError {}

/// The injectable OS/guest clipboard seam: read + write this node's local clipboard
/// endpoint for a session.
///
/// `write_local` is the relay sink (apply a client's clip to the guest); `read_local`
/// backs the echo guard (skip re-applying a clip the endpoint already holds, so the
/// guest's change signal doesn't loop) and, once wired, the guest→client capture
/// half. Production wires [`OsClipboardAccess`]; the tests drive an in-memory fake.
pub trait ClipboardAccess {
    /// Read the current local clipboard for `session_id` (`None` = empty).
    ///
    /// # Errors
    /// A [`ClipboardAccessError`] — `IntegrationGated` until the live vdagent bridge
    /// lands, else `Failed`.
    fn read_local(&self, session_id: &str) -> Result<Option<ClipPayload>, ClipboardAccessError>;

    /// Apply `payload` to the local clipboard for `session_id`.
    ///
    /// # Errors
    /// A [`ClipboardAccessError`] — `IntegrationGated` until the live vdagent bridge
    /// lands, else `Failed`.
    fn write_local(
        &self,
        session_id: &str,
        payload: &ClipPayload,
    ) -> Result<(), ClipboardAccessError>;
}

/// Production [`ClipboardAccess`]: the live OS/guest clipboard channel.
///
/// This slice (E12-9) delivers the pure bridge + the seam; the live executor (the
/// SPICE/RDP clipboard vdagent, or the `wl-clipboard` bridge into the connected VM
/// desktop, reached over the Nebula overlay) is wired by a later E12 unit. Until then
/// each method returns a typed [`ClipboardAccessError::IntegrationGated`] naming
/// exactly what the live call needs — never a fake success (§7).
#[derive(Debug, Clone, Copy, Default)]
pub struct OsClipboardAccess;

impl ClipboardAccess for OsClipboardAccess {
    fn read_local(&self, session_id: &str) -> Result<Option<ClipPayload>, ClipboardAccessError> {
        Err(ClipboardAccessError::IntegrationGated {
            op: "read_local",
            reason: format!(
                "session {session_id} → needs the live OS/guest clipboard channel (the SPICE/RDP \
                 clipboard vdagent or the wl-clipboard bridge into the connected VM desktop); the \
                 guest clipboard link isn't wired yet"
            ),
        })
    }

    fn write_local(
        &self,
        session_id: &str,
        _payload: &ClipPayload,
    ) -> Result<(), ClipboardAccessError> {
        Err(ClipboardAccessError::IntegrationGated {
            op: "write_local",
            reason: format!(
                "session {session_id} → needs the live OS/guest clipboard channel (the SPICE/RDP \
                 clipboard vdagent or the wl-clipboard bridge into the connected VM desktop); the \
                 guest clipboard link isn't wired yet"
            ),
        })
    }
}

// ───────────────────────────── bus + worker ─────────────────────────────

/// Read new [`ACTION_TOPIC`] messages since `cursor`, advancing it. A short sync
/// open-read-drop (never crosses an `.await`), mirroring [`super::session_broker`].
fn read_new_events(bus_root: &Path, cursor: &mut Option<String>) -> Vec<ClipboardEvent> {
    let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
        return vec![];
    };
    let Ok(msgs) = persist.list_since(ACTION_TOPIC, cursor.as_deref()) else {
        return vec![];
    };
    let mut out = Vec::new();
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let body = msg.body.as_deref().unwrap_or("");
        match parse_event(body) {
            Ok(e) => out.push(e),
            Err(e) => {
                tracing::warn!(ulid = %msg.ulid, error = %e, "clipboard_bridge: bad clipboard event");
            }
        }
    }
    out
}

/// Seed the cursor to the newest existing message so a (re)start doesn't re-apply a
/// stale clipboard from the backlog (a clip is transient, unlike the broker's
/// fold-from-genesis roster). `None` when the topic is empty.
fn prime_cursor(bus_root: &Path) -> Option<String> {
    let persist = Persist::open(bus_root.to_path_buf()).ok()?;
    let msgs = persist.list_since(ACTION_TOPIC, None).ok()?;
    msgs.last().map(|m| m.ulid.clone())
}

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

/// The VDI clipboard-bridge worker. Per-session + node-local (NOT leader-gated).
pub struct ClipboardBridgeWorker {
    /// The injectable OS/guest clipboard seam (production: [`OsClipboardAccess`]).
    access: Box<dyn ClipboardAccess + Send + Sync>,
    /// The relay policy (production default: [`ClipboardPolicy::allow_all`]).
    policy: ClipboardPolicy,
    /// Relay cadence.
    poll: Duration,
    /// Bus root override (tests). `None` ⇒ [`default_bus_root`].
    bus_root_override: Option<PathBuf>,
}

impl Default for ClipboardBridgeWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipboardBridgeWorker {
    /// Construct with production defaults: the live [`OsClipboardAccess`] seam, the
    /// permissive [`ClipboardPolicy::allow_all`], the default cadence, and the
    /// auto-resolved bus root.
    #[must_use]
    pub fn new() -> Self {
        Self {
            access: Box::new(OsClipboardAccess),
            policy: ClipboardPolicy::allow_all(),
            poll: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
        }
    }

    /// Inject a clipboard-access seam (tests). Production uses [`OsClipboardAccess`].
    #[must_use]
    pub fn with_access(mut self, access: Box<dyn ClipboardAccess + Send + Sync>) -> Self {
        self.access = access;
        self
    }

    /// Set the relay policy. Production defaults to [`ClipboardPolicy::allow_all`].
    #[must_use]
    pub fn with_policy(mut self, policy: ClipboardPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Override the relay cadence (tests, to avoid multi-second waits).
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Override the Bus root (tests).
    #[must_use]
    pub fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    fn bus_root(&self) -> Option<PathBuf> {
        self.bus_root_override.clone().or_else(default_bus_root)
    }

    /// Apply one clip: run the pure [`relay`] decision, guard against echo, and relay
    /// the resulting payload through the seam. `latest` carries the last-relayed clip
    /// per session (the incremental [`fold_latest`]); it doubles as the in-memory echo
    /// guard so a re-sent clip isn't re-applied even when `read_local` is gated.
    fn relay_event(&self, event: &ClipboardEvent, latest: &mut BTreeMap<SessionId, ClipPayload>) {
        let payload = match relay(event, &self.policy) {
            RelayDecision::Forward(p) => p,
            RelayDecision::Truncate(p) => {
                tracing::debug!(session = %event.session_id, "clipboard_bridge: truncated an oversized clip to the cap");
                p
            }
            RelayDecision::Drop(reason) => {
                tracing::debug!(session = %event.session_id, ?reason, "clipboard_bridge: policy dropped a clip");
                return;
            }
        };
        // Track the latest clip per session; keep the prior value to guard against echo.
        let prev = latest.insert(event.session_id.clone(), payload.clone());
        // Echo guard 1 — in-memory: a clip identical to the last one we relayed for
        // this session is a no-op (re-applying it would re-fire the guest's change
        // signal → an echo storm). Works even when read_local is integration-gated.
        if prev.as_ref() == Some(&payload) {
            tracing::debug!(session = %event.session_id, "clipboard_bridge: clip already current (in-memory); skipping echo");
            return;
        }
        // Echo guard 2 — live: if the endpoint already holds this exact clip, skip.
        // A gated / failed read falls through to the write (honest — the write is the
        // real relay, itself gated in this slice, and defers with a log).
        if let Ok(Some(current)) = self.access.read_local(&event.session_id) {
            if current == payload {
                tracing::debug!(session = %event.session_id, "clipboard_bridge: clip already current (endpoint); skipping echo");
                return;
            }
        }
        if let Err(e) = self.access.write_local(&event.session_id, &payload) {
            match e {
                ClipboardAccessError::IntegrationGated { .. } => {
                    tracing::info!(error = %e, "clipboard_bridge: clipboard access integration-gated; deferring relay");
                }
                ClipboardAccessError::Failed { .. } => {
                    tracing::warn!(error = %e, "clipboard_bridge: clipboard write failed");
                }
            }
        }
    }

    /// Drain new clipboard events (advancing `cursor`) and relay each through the seam,
    /// maintaining the latest-per-session fold.
    fn drain_and_relay(
        &self,
        bus_root: &Path,
        cursor: &mut Option<String>,
        latest: &mut BTreeMap<SessionId, ClipPayload>,
    ) {
        for event in read_new_events(bus_root, cursor) {
            self.relay_event(&event, latest);
        }
    }
}

#[async_trait::async_trait]
impl Worker for ClipboardBridgeWorker {
    fn name(&self) -> &'static str {
        "clipboard_bridge"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self.bus_root() else {
            tracing::debug!("clipboard_bridge: no bus root; worker idle");
            return Ok(());
        };
        // Skip the backlog on start — a clip is transient, so a restart must not
        // re-apply a stale clipboard (unlike the broker's fold-from-genesis roster).
        let mut cursor = prime_cursor(&bus_root);
        let mut latest: BTreeMap<SessionId, ClipPayload> = BTreeMap::new();
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    self.drain_and_relay(&bus_root, &mut cursor, &mut latest);
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
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }

    fn event(session: &str, direction: ClipDirection, content: &str) -> ClipboardEvent {
        ClipboardEvent {
            session_id: session.to_string(),
            direction,
            payload: ClipPayload::text(content),
        }
    }

    // ── format + direction ──

    #[test]
    fn clip_format_mime_and_serde_snake_case() {
        assert_eq!(ClipFormat::Text.mime(), "text/plain;charset=utf-8");
        assert_eq!(
            serde_json::to_string(&ClipFormat::Text).unwrap(),
            "\"text\""
        );
    }

    #[test]
    fn clip_direction_opposite_and_serde_snake_case() {
        assert_eq!(
            ClipDirection::ClientToGuest.opposite(),
            ClipDirection::GuestToClient
        );
        assert_eq!(
            ClipDirection::GuestToClient.opposite(),
            ClipDirection::ClientToGuest
        );
        assert_eq!(
            serde_json::to_string(&ClipDirection::ClientToGuest).unwrap(),
            "\"client_to_guest\""
        );
    }

    // ── payload size model (the required cap + typed error) ──

    #[test]
    fn checked_accepts_within_ceiling_and_rejects_oversized() {
        // A within-ceiling payload constructs fine.
        let ok = ClipPayload::checked(ClipFormat::Text, "hello".into()).expect("within ceiling");
        assert_eq!(ok.len(), 5);
        assert!(!ok.is_empty());
        // One byte over the ceiling is refused with the typed error (never relayed).
        let big = "a".repeat(MAX_CLIP_BYTES + 1);
        let err = ClipPayload::checked(ClipFormat::Text, big).unwrap_err();
        assert_eq!(
            err,
            ClipboardError::PayloadTooLarge {
                len: MAX_CLIP_BYTES + 1,
                max: MAX_CLIP_BYTES,
            }
        );
    }

    #[test]
    fn truncated_is_bounded_and_char_boundary_safe() {
        // A multi-byte char that would be split at the cap is dropped whole.
        let p = ClipPayload::text("a😀b"); // 1 + 4 + 1 bytes
        let t = p.truncated(3); // 'a' (1) + start of the 4-byte emoji → cut back to 1
        assert_eq!(t.content, "a");
        assert!(t.len() <= 3);
        // A cap at/above the length is a no-op copy.
        assert_eq!(p.truncated(999).content, "a😀b");
    }

    // ── parse_event (round-trip + the size reject at ingest) ──

    #[test]
    fn parse_event_round_trips_and_rejects_malformed() {
        let ev = parse_event(
            r#"{"session_id":"s1","direction":"client_to_guest","payload":{"format":"text","content":"hi"}}"#,
        )
        .expect("valid event parses");
        assert_eq!(ev.session_id, "s1");
        assert_eq!(ev.direction, ClipDirection::ClientToGuest);
        assert_eq!(ev.payload.content, "hi");
        assert!(parse_event("nonsense").is_err());
        assert!(parse_event(r#"{"session_id":"s1","direction":"sideways","payload":{"format":"text","content":"x"}}"#).is_err());
    }

    #[test]
    fn parse_event_rejects_an_oversized_payload_at_ingest() {
        // An oversized clip on the bus is refused (typed error surfaced) — the worker
        // never relays it. Serialize a real event with a >ceiling payload and re-parse.
        let ev = ClipboardEvent {
            session_id: "s1".into(),
            direction: ClipDirection::ClientToGuest,
            payload: ClipPayload::text("a".repeat(MAX_CLIP_BYTES + 8)),
        };
        let body = serde_json::to_string(&ev).unwrap();
        let err = parse_event(&body).unwrap_err();
        assert!(err.contains("too large"), "names the size reject: {err}");
    }

    // ── policy ──

    #[test]
    fn policy_allow_deny_and_one_way() {
        let allow = ClipboardPolicy::allow_all();
        assert!(allow.allows_session("anything"));
        assert!(allow.allows_direction(ClipDirection::ClientToGuest));
        assert!(allow.allows_direction(ClipDirection::GuestToClient));

        // deny_all + an explicit allow for one session.
        let deny = ClipboardPolicy::deny_all().allow_session("s1");
        assert!(deny.allows_session("s1"));
        assert!(!deny.allows_session("s2"));

        // one-way blocks the reverse direction.
        let oneway = ClipboardPolicy::allow_all().one_way(ClipDirection::ClientToGuest);
        assert!(oneway.allows_direction(ClipDirection::ClientToGuest));
        assert!(!oneway.allows_direction(ClipDirection::GuestToClient));
    }

    // ── relay (the required pure decision: Forward / Drop / Truncate) ──

    #[test]
    fn relay_forwards_an_allowed_clip() {
        let out = relay(
            &event("s1", ClipDirection::ClientToGuest, "hello"),
            &ClipboardPolicy::allow_all(),
        );
        assert_eq!(out, RelayDecision::Forward(ClipPayload::text("hello")));
    }

    #[test]
    fn relay_drops_a_denied_session() {
        let policy = ClipboardPolicy::deny_all();
        assert_eq!(
            relay(&event("s1", ClipDirection::ClientToGuest, "x"), &policy),
            RelayDecision::Drop(DropReason::SessionDenied)
        );
    }

    #[test]
    fn relay_drops_a_direction_blocked_by_one_way() {
        // Only client→guest is allowed; a guest→client clip is dropped.
        let policy = ClipboardPolicy::allow_all().one_way(ClipDirection::ClientToGuest);
        assert_eq!(
            relay(&event("s1", ClipDirection::GuestToClient, "leak"), &policy),
            RelayDecision::Drop(DropReason::DirectionBlocked)
        );
        // …and the permitted direction still forwards.
        assert!(matches!(
            relay(&event("s1", ClipDirection::ClientToGuest, "ok"), &policy),
            RelayDecision::Forward(_)
        ));
    }

    #[test]
    fn relay_truncates_over_the_soft_cap() {
        let policy = ClipboardPolicy::allow_all().truncate_over(5);
        let out = relay(
            &event("s1", ClipDirection::ClientToGuest, "hello world"),
            &policy,
        );
        assert_eq!(out, RelayDecision::Truncate(ClipPayload::text("hello")));
    }

    #[test]
    fn relay_never_forwards_over_the_hard_ceiling() {
        // Even with no soft cap and an oversized payload handed directly to relay
        // (bypassing parse_event), the outcome is a Truncate to the hard ceiling —
        // the bridge never forwards unbounded.
        let policy = ClipboardPolicy::allow_all();
        let ev = event(
            "s1",
            ClipDirection::ClientToGuest,
            &"a".repeat(MAX_CLIP_BYTES + 10),
        );
        match relay(&ev, &policy) {
            RelayDecision::Truncate(p) => assert_eq!(p.len(), MAX_CLIP_BYTES),
            other => panic!("expected Truncate to the ceiling, got {other:?}"),
        }
    }

    // ── fold_latest (latest clipboard per session) ──

    #[test]
    fn fold_latest_is_latest_wins_by_session() {
        let e1 = event("s1", ClipDirection::ClientToGuest, "old");
        let e2 = event("s2", ClipDirection::GuestToClient, "other");
        let e3 = event("s1", ClipDirection::GuestToClient, "new"); // later s1 wins
        let map = fold_latest([&e1, &e2, &e3]);
        assert_eq!(map.len(), 2);
        assert_eq!(map["s1"].content, "new");
        assert_eq!(map["s2"].content, "other");
    }

    // ── the access seam ──

    #[test]
    fn os_clipboard_access_is_integration_gated_not_faked() {
        let access = OsClipboardAccess;
        for (label, err) in [
            (
                "read_local",
                access.read_local("s1").map(|_| ()).unwrap_err(),
            ),
            (
                "write_local",
                access
                    .write_local("s1", &ClipPayload::text("x"))
                    .unwrap_err(),
            ),
        ] {
            match err {
                ClipboardAccessError::IntegrationGated { op, reason } => {
                    assert_eq!(op, label);
                    assert!(
                        reason.contains("clipboard"),
                        "names the missing live channel: {reason}"
                    );
                }
                ClipboardAccessError::Failed { op, reason } => {
                    panic!("expected integration-gated, got Failed {{{op}: {reason}}}")
                }
            }
        }
    }

    /// An in-memory [`ClipboardAccess`] — the Fake seam. `rows` is the endpoint
    /// state; `writes` an append-log so a test can count relays (echo assertions).
    #[derive(Clone, Default)]
    struct FakeClipboard {
        rows: Arc<Mutex<BTreeMap<SessionId, ClipPayload>>>,
        writes: Arc<Mutex<Vec<(SessionId, ClipPayload)>>>,
    }

    impl ClipboardAccess for FakeClipboard {
        fn read_local(
            &self,
            session_id: &str,
        ) -> Result<Option<ClipPayload>, ClipboardAccessError> {
            Ok(self
                .rows
                .lock()
                .expect("rows mutex")
                .get(session_id)
                .cloned())
        }
        fn write_local(
            &self,
            session_id: &str,
            payload: &ClipPayload,
        ) -> Result<(), ClipboardAccessError> {
            self.rows
                .lock()
                .expect("rows mutex")
                .insert(session_id.to_string(), payload.clone());
            self.writes
                .lock()
                .expect("writes mutex")
                .push((session_id.to_string(), payload.clone()));
            Ok(())
        }
    }

    #[test]
    fn fake_clipboard_round_trips() {
        let access = FakeClipboard::default();
        assert!(access.read_local("s1").unwrap().is_none());
        access.write_local("s1", &ClipPayload::text("hi")).unwrap();
        assert_eq!(
            access.read_local("s1").unwrap(),
            Some(ClipPayload::text("hi"))
        );
    }

    #[test]
    fn topic_is_namespaced() {
        assert_eq!(ACTION_TOPIC, "action/vdi/clipboard");
        assert!(ACTION_TOPIC.starts_with("action/vdi/"));
    }

    #[test]
    fn worker_name_matches_module() {
        assert_eq!(ClipboardBridgeWorker::new().name(), "clipboard_bridge");
    }

    // ── worker wiring (seeded temp bus + injected fake access) ──

    /// Seed a temp bus with `action/vdi/clipboard` bodies and return its root.
    fn seed_bus(events: &[ClipboardEvent]) -> PathBuf {
        use mde_bus::hooks::config::Priority;
        // A per-process counter makes the dir unique across PARALLEL tests — `now_ms`
        // + event count alone collide when two tests share a millisecond, and one
        // test's `remove_dir_all` then corrupts another's bus.
        static SEED_SEQ: AtomicU64 = AtomicU64::new(0);
        let uniq = SEED_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("mde-clip-{}-{}-{uniq}", now_ms(), events.len()));
        let persist = Persist::open(dir.clone()).expect("open bus");
        for e in events {
            persist
                .write(
                    ACTION_TOPIC,
                    Priority::Default,
                    None,
                    Some(&serde_json::to_string(e).unwrap()),
                )
                .expect("write event");
        }
        dir
    }

    #[tokio::test]
    async fn worker_drains_and_relays_a_clip_through_the_seam() {
        // A client copy drained off the bus is applied to the guest via the fake seam.
        let bus = seed_bus(&[event("s1", ClipDirection::ClientToGuest, "copied text")]);
        let access = FakeClipboard::default();
        let rows = access.rows.clone();
        let w = ClipboardBridgeWorker::new()
            .with_access(Box::new(access))
            .with_bus_root(bus.clone());

        let mut cursor = None;
        let mut latest = BTreeMap::new();
        w.drain_and_relay(&bus, &mut cursor, &mut latest);

        assert_eq!(
            rows.lock().expect("rows mutex").get("s1"),
            Some(&ClipPayload::text("copied text")),
            "the clip was relayed into the guest endpoint"
        );
        // …and the latest-per-session fold tracked it.
        assert_eq!(latest["s1"], ClipPayload::text("copied text"));
        let _ = std::fs::remove_dir_all(&bus);
    }

    #[tokio::test]
    async fn worker_drops_a_denied_session_without_writing() {
        let bus = seed_bus(&[event("s1", ClipDirection::ClientToGuest, "secret")]);
        let access = FakeClipboard::default();
        let writes = access.writes.clone();
        let w = ClipboardBridgeWorker::new()
            .with_access(Box::new(access))
            .with_policy(ClipboardPolicy::deny_all())
            .with_bus_root(bus.clone());

        let mut cursor = None;
        let mut latest = BTreeMap::new();
        w.drain_and_relay(&bus, &mut cursor, &mut latest);

        assert!(
            writes.lock().expect("writes mutex").is_empty(),
            "a policy-denied clip is never written to the endpoint"
        );
        let _ = std::fs::remove_dir_all(&bus);
    }

    #[tokio::test]
    async fn worker_echo_guard_skips_a_repeated_clip() {
        // Two identical client copies in one drain ⇒ a single endpoint write (the
        // in-memory guard skips the echo).
        let bus = seed_bus(&[
            event("s1", ClipDirection::ClientToGuest, "same"),
            event("s1", ClipDirection::ClientToGuest, "same"),
        ]);
        let access = FakeClipboard::default();
        let writes = access.writes.clone();
        let w = ClipboardBridgeWorker::new()
            .with_access(Box::new(access))
            .with_bus_root(bus.clone());

        let mut cursor = None;
        let mut latest = BTreeMap::new();
        w.drain_and_relay(&bus, &mut cursor, &mut latest);

        assert_eq!(
            writes.lock().expect("writes mutex").len(),
            1,
            "the repeated clip was deduped, not re-applied"
        );
        let _ = std::fs::remove_dir_all(&bus);
    }

    #[tokio::test]
    async fn worker_echo_guard_skips_a_clip_the_endpoint_already_holds() {
        // A fresh worker (empty in-memory fold) whose endpoint already holds the clip
        // ⇒ the live read_local guard skips the write.
        let bus = seed_bus(&[event("s1", ClipDirection::ClientToGuest, "current")]);
        let access = FakeClipboard::default();
        access
            .write_local("s1", &ClipPayload::text("current"))
            .unwrap();
        let writes = access.writes.clone();
        let baseline = writes.lock().expect("writes mutex").len(); // 1 (the seed write)
        let w = ClipboardBridgeWorker::new()
            .with_access(Box::new(access))
            .with_bus_root(bus.clone());

        let mut cursor = None;
        let mut latest = BTreeMap::new();
        w.drain_and_relay(&bus, &mut cursor, &mut latest);

        assert_eq!(
            writes.lock().expect("writes mutex").len(),
            baseline,
            "no new write — the endpoint already held the clip"
        );
        let _ = std::fs::remove_dir_all(&bus);
    }

    #[tokio::test]
    async fn run_loop_primes_past_the_backlog_and_exits_on_shutdown() {
        // A pre-existing (backlog) clip must NOT be re-applied on start (a clip is
        // transient) — prime_cursor skips it; the loop then exits promptly on shutdown.
        let bus = seed_bus(&[event("s1", ClipDirection::ClientToGuest, "stale backlog")]);
        let access = FakeClipboard::default();
        let writes = access.writes.clone();
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut w = ClipboardBridgeWorker::new()
            .with_access(Box::new(access))
            .with_bus_root(bus.clone())
            .with_poll(Duration::from_millis(10));
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        assert!(joined.unwrap().expect("join").is_ok());
        assert!(
            writes.lock().expect("writes mutex").is_empty(),
            "the backlog clip was primed past, not re-applied on start"
        );
        let _ = std::fs::remove_dir_all(&bus);
    }
}
