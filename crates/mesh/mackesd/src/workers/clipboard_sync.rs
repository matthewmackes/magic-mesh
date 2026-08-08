//! CLIP-SYNC-1 — mesh clipboard history worker.
//!
//! Consumes canonical text-clipboard events from the Mackes Bus
//! (`event/clipboard/clip`) and appends them to ONE mesh-global history file on
//! the QNM-Shared replicated root (`<root>/clipboard/history.json`). Every peer
//! runs this worker; the single shared file is the mesh-global clipboard (no
//! per-user/per-node partition — the single-operator model, design lock O8).
//!
//! The canonical event body is `{ id, text, source, time }`. `id` is the stable
//! content fingerprint, `source` is the producer node/lane, and `time` is an
//! RFC3339 timestamp. The worker deliberately does not read the OS clipboard or
//! shell out to compositor-specific tools; seat, browser, KDC/mobile, and VDI
//! producers publish the shared lane and this worker folds that lane into
//! durable history.
//!
//! Operator locks (design `docs/design/notify-hub-redesign.md`, survey round 1,
//! 2026-06-18):
//!   * O2 echo-loop — **debounce identical content**: a copy whose text
//!     equals the most-recent applied clip is dropped. This is what kills
//!     the click-to-load echo without origin-tagging the selection.
//!   * O3 dedup — **move-to-top**: re-copying existing text bumps the one
//!     entry to the front instead of duplicating.
//!   * O4 no size cap — any text length syncs (the bus-retention worker
//!     bounds the bus; the history stays at 50 + pinned).
//!   * O6 stamp — each entry carries its source node + an RFC3339 time so
//!     the viewer renders "from <node> · <age>".
//!   * O7 pins — pinned entries are **exempt from the 50-cap and
//!     unlimited**; only unpinned entries are trimmed.
//!
//! The history mutations (`apply_clip` / `apply_clip_event`) are pure + fully
//! unit-tested; the worker body is the I/O glue (tail the Bus lane and
//! read/merge/write the shared file under the shared-root guard). The
//! `action/clipboard/*` IPC responder (`ipc::clipboard`) edits the same file for
//! the viewer's delete/pin/clear verbs.
//!
//! **Concurrency.** Each writer (this worker, the IPC responder, every
//! peer) does an unlocked read → mutate → atomic-`rename` write of the one
//! shared `history.json` — the same last-writer-wins shape the sibling
//! shared-state responders (`ipc::connect`, the peer directory) use against
//! the replicated root. The atomic rename prevents a torn read; a rare
//! concurrent pin-vs-capture can lose one update, self-healing on the next
//! capture. A real clipboard never sustains the write rate where this
//! matters, so a cross-node lock is deliberately not taken here (it would
//! add a Syncthing round-trip to every copy).

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mackes_mesh_types::cloud::{cloud_request_digest, CloudArmSigner, CloudArmedToken};
use mackes_mesh_types::vdi_clipboard::{
    ClipboardEnvelopeV2, ClipboardEnvelopeV2ValidationError, ClipboardMaterialization,
    ClipboardSessionConsentV1, ClipboardSessionConsentValidationError, VdiClipboardText,
    CLIPBOARD_MATERIALIZATION_TOPIC,
};
use mde_bus::persist::Persist;
use mde_collab_types::{
    ClipboardEnvelopeV2 as CollabClipboardEnvelopeV2,
    ClipboardEnvelopeV2ValidationError as CollabClipboardEnvelopeV2ValidationError,
    ClipboardMimeKind as CollabClipboardMimeKind, ClipboardPayloadV2 as CollabClipboardPayloadV2,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

pub mod mesh;
pub mod session;

use super::clipboard_bridge::{ClipDirection, ClipPayload, ClipboardEvent};
use super::session_broker::{EtcdSessionStore, MeshSessionStore, SessionState, SessionStore};
use super::{ShutdownToken, Worker};
use crate::ipc::action_auth::{
    production_action_signer, ActionAuthorizer, MutationContext, ACTION_SCHEMA_VERSION,
    MAX_AUTH_TTL_MS,
};

pub use mackes_mesh_types::vdi_clipboard::CLIPBOARD_SESSION_CONSENT_TOPIC;

/// The daemon-owned VNC-to-seat handoff is deliberately narrower than the
/// general clipboard action lane: only the shell's truthful VNC source form is
/// eligible for conversion, and only an active session record can supply the
/// destination seat.
const VNC_SOURCE_PREFIX: &str = "vnc:";

/// The shared text-only clipboard ceiling. Keep mesh history on the same
/// bounded contract as the VDI bridge; oversized payloads must never become
/// durable replicated state.
const MAX_CLIP_BYTES: usize = super::clipboard_bridge::MAX_CLIP_BYTES;

/// Non-pinned entries kept in the shared history (O7: pins are exempt +
/// unlimited, so the real file can be longer than this).
pub const HISTORY_CAP: usize = 50;

/// Bus topic every text clip is broadcast on. The viewer + any tailing
/// consumer subscribe here for real-time updates; the durable record is
/// the history file.
pub const CLIP_TOPIC: &str = "event/clipboard/clip";

/// Bus topic for the versioned rich clipboard contract. This lane is kept
/// separate from [`CLIP_TOPIC`] so legacy text consumers cannot deserialize a
/// rich envelope as an old `{ id, text, source, time }` body.
pub const CLIPBOARD_ENVELOPE_V2_TOPIC: &str = "event/clipboard/envelope-v2";

/// Bus topic for the signed `mde-collab-types` rich clipboard contract.
///
/// This must remain distinct from [`CLIPBOARD_ENVELOPE_V2_TOPIC`]: that lane
/// carries the existing `mackes_mesh_types` VDI envelope and has a different
/// wire shape. Keeping both explicit preserves deployed VDI producers while
/// allowing the collaboration contract to reach this worker's real intake.
pub const COLLAB_CLIPBOARD_ENVELOPE_V2_TOPIC: &str = "event/clipboard/collab-envelope-v2";

/// Payload-free cursor for local rich clipboard send requests.
const MESH_SEND_CURSOR_FILE_NAME: &str = "clipboard-sync.mesh-send.cursor.json";

/// Payload-free cursor for target-specific authenticated mesh frames.
const MESH_RECEIVE_CURSOR_FILE_NAME: &str = "clipboard-sync.mesh-receive.cursor.json";

/// Capability verb for the explicit clipboard publishing consent control.
pub const CLIPBOARD_SESSION_CONSENT_AUTH_VERB: &str = "clipboard-session-consent";

/// Maximum encoded consent command size, including its short-lived capability.
/// This is deliberately smaller than the generic action cap because the
/// command carries only identity, timestamps, state, and authentication.
const MAX_CLIPBOARD_SESSION_CONSENT_COMMAND_BYTES: usize = 16 * 1024;

/// Maximum encoded capability field accepted by the typed consent envelope.
/// Cloud armed tokens are substantially shorter; this bound prevents a forged
/// command from turning the parser into an allocation sink before authorization.
const MAX_CLIPBOARD_SESSION_CONSENT_TOKEN_BYTES: usize = 1024;

/// Per-daemon cursor file kept beside the local Bus log. It is deliberately
/// not stored under the replicated workgroup root: each daemon must acknowledge
/// the canonical lane independently, while the history itself is mesh-global.
const CURSOR_FILE_NAME: &str = "clipboard-sync.cursor.json";

/// The V2 cursor is separate from the legacy text cursor because the two lanes
/// have independent schemas and acknowledgement rules.
const V2_CURSOR_FILE_NAME: &str = "clipboard-sync.v2.cursor.json";

/// Durable cursor for the distinct `mde-collab-types` envelope lane.
const COLLAB_V2_CURSOR_FILE_NAME: &str = "clipboard-sync.collab-v2.cursor.json";

/// Durable cursor for the authenticated consent-control lane.
const CONSENT_CURSOR_FILE_NAME: &str = "clipboard-sync.consent.cursor.json";

/// Keep the source-session replay ledger bounded. The ledger stores only
/// identity + sequence markers, never inline payload bytes.
const MAX_V2_SOURCE_LANES: usize = 256;

/// Keep collaboration-envelope replay state bounded and payload-free.
const MAX_COLLAB_V2_SOURCE_LANES: usize = 256;

/// Keep consent state bounded independently from replay markers. The ledger
/// stores only safe source identity and timestamps; it never stores payloads.
const MAX_V2_CONSENT_SESSIONS: usize = 256;

/// Bus-drain cadence for the canonical clipboard event lane.
pub const CLIP_EVENT_POLL_INTERVAL: Duration = Duration::from_millis(400);

/// One clipboard entry in the mesh-global history. `id` is a stable
/// content fingerprint so the viewer/IPC can address an entry (pin/delete)
/// without shipping the full text back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipEntry {
    /// Stable id (content fingerprint) — addresses the entry for pin/delete.
    pub id: String,
    /// The clip text (verbatim; O4 — no size cap, no secret filtering).
    pub text: String,
    /// Node that captured the clip (O6 source attribution).
    pub source: String,
    /// RFC3339 capture timestamp (O6 — the viewer renders relative age).
    pub time: String,
    /// O7 — pinned entries survive the cap + a mesh-wide clear.
    #[serde(default)]
    pub pinned: bool,
}

/// Canonical `event/clipboard/clip` Bus body.
///
/// Keep this shape compatible with existing producers and consumers: exactly the
/// public clipboard event fields `{ id, text, source, time }`. Durable history
/// adds `pinned` locally, but event producers cannot set it through this body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipEventBody {
    /// Stable id (content fingerprint) — addresses the entry for pin/delete.
    pub id: String,
    /// The clip text (verbatim; O4 — no size cap, no secret filtering).
    pub text: String,
    /// Producer node/lane that emitted the event.
    pub source: String,
    /// RFC3339 capture timestamp.
    pub time: String,
}

impl ClipEventBody {
    /// Build the canonical event body from local text/source/time inputs.
    #[must_use]
    pub fn from_text(text: &str, source: &str, time: &str) -> Self {
        Self {
            id: clip_id(text),
            text: text.to_string(),
            source: source.to_string(),
            time: time.to_string(),
        }
    }
}

/// Authenticated Bus envelope for one clipboard session-consent update.
///
/// The outer schema is the existing privileged-action schema consumed by
/// [`ActionAuthorizer`]. The nested value is the only typed semantic payload;
/// in particular, there is no clipboard text, MIME value, Files reference, or
/// arbitrary byte field in this control envelope. Unknown fields fail closed
/// before the capability is even considered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipboardSessionConsentCommandV1 {
    /// Existing exact-body action schema.
    pub schema_version: u16,
    /// Explicit bounded consent state for one source node/seat/session.
    pub consent: ClipboardSessionConsentV1,
    /// Exact-body capability minted by the root/systemd signer.
    pub armed_token: String,
}

impl ClipboardSessionConsentCommandV1 {
    /// Decode the bounded, strict Bus body. The nested consent type performs
    /// its own intrinsic schema/identity/timestamp/expiry validation during
    /// deserialization; freshness and update ordering are checked by the
    /// daemon ledger after authentication.
    fn from_json(body: &str) -> Result<Self, String> {
        if body.len() > MAX_CLIPBOARD_SESSION_CONSENT_COMMAND_BYTES {
            return Err(format!(
                "clipboard consent command body exceeds {MAX_CLIPBOARD_SESSION_CONSENT_COMMAND_BYTES} bytes"
            ));
        }
        let command = serde_json::from_str::<Self>(body)
            .map_err(|error| format!("malformed clipboard consent command: {error}"))?;
        if command.schema_version != ACTION_SCHEMA_VERSION as u16 {
            return Err(format!(
                "clipboard consent command requires schema_version {ACTION_SCHEMA_VERSION}"
            ));
        }
        if command.armed_token.is_empty()
            || command.armed_token.len() > MAX_CLIPBOARD_SESSION_CONSENT_TOKEN_BYTES
            || command.armed_token.trim() != command.armed_token
        {
            return Err("clipboard consent command has an invalid armed_token field".to_string());
        }
        Ok(command)
    }
}

/// Canonical capability target for one consented source session. Keeping all
/// three source identity components in the target makes a token minted for one
/// seat/session unusable for another even when a caller accidentally reuses a
/// local node scope.
#[must_use]
pub fn clipboard_session_consent_auth_target(consent: &ClipboardSessionConsentV1) -> String {
    format!(
        "source:{}:seat:{}:session:{}",
        consent.source_node, consent.source_seat, consent.source_session
    )
}

/// Payload forms that the current daemon materialization path cannot preserve
/// without an additional Files or rich-MIME adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardEnvelopeV2UnsupportedPayload {
    /// The bytes live in the Files executor and must not be treated as text.
    FilesReference,
    /// More than the exact text/plain representation was offered.
    RichMime,
}

/// A bounded V2 admission or materialization failure. Shared contract errors
/// remain typed so replay, identity, expiry, and intrinsic payload failures are
/// distinguishable from the daemon's currently unsupported representations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardEnvelopeV2BoundaryError {
    /// The serialized body was too large or malformed.
    Decode(String),
    /// The shared V2 contract rejected the decoded envelope.
    Admission(ClipboardEnvelopeV2ValidationError),
    /// The envelope is valid but this text-only materialization path cannot
    /// preserve its representation.
    UnsupportedPayload(ClipboardEnvelopeV2UnsupportedPayload),
    /// The bounded source-session high-water table has no free lane.
    LedgerCapacityExceeded {
        /// Maximum number of source-session replay lanes.
        max: usize,
    },
    /// The source session has not passed the separate typed consent boundary.
    Consent(ClipboardSessionConsentBoundaryError),
    /// A valid source timestamp cannot be represented as an RFC3339 chrono
    /// timestamp for the existing materialization record.
    TimestampOutOfRange {
        /// Source timestamp in Unix epoch milliseconds.
        timestamp_ms: u64,
    },
}

impl std::fmt::Display for ClipboardEnvelopeV2BoundaryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(error) => write!(formatter, "clipboard V2 body decode failed: {error}"),
            Self::Admission(error) => write!(formatter, "clipboard V2 admission failed: {error}"),
            Self::UnsupportedPayload(ClipboardEnvelopeV2UnsupportedPayload::FilesReference) => {
                formatter.write_str(
                    "clipboard V2 Files payload is not materializable by the text-only daemon path",
                )
            }
            Self::UnsupportedPayload(ClipboardEnvelopeV2UnsupportedPayload::RichMime) => formatter
                .write_str(
                "clipboard V2 rich MIME offers are not materializable by the text-only daemon path",
            ),
            Self::LedgerCapacityExceeded { max } => write!(
                formatter,
                "clipboard V2 source-session ledger is full (maximum {max})"
            ),
            Self::Consent(error) => {
                write!(formatter, "clipboard V2 consent admission failed: {error}")
            }
            Self::TimestampOutOfRange { timestamp_ms } => write!(
                formatter,
                "clipboard V2 timestamp {timestamp_ms} is outside the RFC3339 range"
            ),
        }
    }
}

impl std::error::Error for ClipboardEnvelopeV2BoundaryError {}

/// A typed failure from the daemon-local clipboard consent seam. The Bus
/// command is authenticated and decoded before it reaches this ledger; the
/// ledger itself remains bounded and payload-free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardSessionConsentBoundaryError {
    /// No consent has been admitted for this exact source identity.
    Missing,
    /// A consent record exists, but it is explicitly disabled.
    Disabled,
    /// The shared consent contract rejected the record or its freshness.
    Admission(ClipboardSessionConsentValidationError),
    /// The bounded in-memory consent table has no free identity lane.
    LedgerCapacityExceeded {
        /// Maximum number of consented source identities.
        max: usize,
    },
}

impl std::fmt::Display for ClipboardSessionConsentBoundaryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => formatter.write_str("no consent is admitted for the source session"),
            Self::Disabled => formatter.write_str("source-session clipboard consent is disabled"),
            Self::Admission(error) => write!(formatter, "{error}"),
            Self::LedgerCapacityExceeded { max } => write!(
                formatter,
                "clipboard consent ledger is full (maximum {max})"
            ),
        }
    }
}

impl std::error::Error for ClipboardSessionConsentBoundaryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Admission(error) => Some(error),
            Self::Missing | Self::Disabled | Self::LedgerCapacityExceeded { .. } => None,
        }
    }
}

/// The explicit typed fold produced for the only V2 representation the
/// existing daemon handoff can preserve. The original envelope remains attached
/// so the worker can advance its source-session high-water marker without
/// rebuilding or guessing any payload representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardEnvelopeV2TextFold {
    /// The admitted rich envelope, including its source ordering metadata.
    pub envelope: ClipboardEnvelopeV2,
    /// The bounded text payload copied without truncation or re-encoding.
    pub text: VdiClipboardText,
    /// Stable source attribution for [`ClipboardMaterialization`].
    pub source: String,
    /// Source timestamp converted to the legacy materialization's RFC3339 form.
    pub time: String,
}

/// Admit one serialized V2 body against the prior source-session high-water
/// marker. The first body establishes the lane's claimed identity; subsequent
/// bodies must match that identity and strictly increase its sequence. A
/// caller with a stronger authenticated source context should pass that context
/// directly to [`ClipboardEnvelopeV2::admit`] before calling the fold seam.
pub fn admit_serialized_clipboard_envelope_v2(
    body: &[u8],
    previous: Option<&ClipboardEnvelopeV2>,
    now_ms: u64,
) -> Result<ClipboardEnvelopeV2, ClipboardEnvelopeV2BoundaryError> {
    let envelope = ClipboardEnvelopeV2::from_json_bytes(body)
        .map_err(|error| ClipboardEnvelopeV2BoundaryError::Decode(error.to_string()))?;
    if let Some(previous) = previous {
        envelope
            .admit(
                &previous.source_node,
                &previous.source_seat,
                &previous.source_session,
                None,
                now_ms,
            )
            .map_err(ClipboardEnvelopeV2BoundaryError::Admission)?;
        if envelope.sequence <= previous.sequence {
            return Err(ClipboardEnvelopeV2BoundaryError::Admission(
                ClipboardEnvelopeV2ValidationError::Replay {
                    previous: previous.sequence,
                    received: envelope.sequence,
                },
            ));
        }
    } else {
        // There is no publisher identity field in mde-bus::StoredMessage. The
        // first message can therefore only establish a source-session lane;
        // later messages are bound to this marker. Authenticated adapters must
        // use ClipboardEnvelopeV2::admit with their trusted identity context.
        envelope
            .admit(
                &envelope.source_node,
                &envelope.source_seat,
                &envelope.source_session,
                None,
                now_ms,
            )
            .map_err(ClipboardEnvelopeV2BoundaryError::Admission)?;
    }
    Ok(envelope)
}

/// Fold an admitted V2 envelope into the existing text-only materialization
/// shape. Files references and rich MIME offers fail explicitly; neither is
/// represented as a `String` or inserted into the legacy text lane.
pub fn fold_clipboard_envelope_v2(
    envelope: ClipboardEnvelopeV2,
) -> Result<ClipboardEnvelopeV2TextFold, ClipboardEnvelopeV2BoundaryError> {
    envelope
        .validate()
        .map_err(ClipboardEnvelopeV2BoundaryError::Admission)?;
    let Some(text) = envelope.inline_text.clone() else {
        return Err(ClipboardEnvelopeV2BoundaryError::UnsupportedPayload(
            ClipboardEnvelopeV2UnsupportedPayload::FilesReference,
        ));
    };
    if envelope.mime_offers.len() != 1
        || !envelope.mime_offers[0].eq_ignore_ascii_case("text/plain")
    {
        return Err(ClipboardEnvelopeV2BoundaryError::UnsupportedPayload(
            ClipboardEnvelopeV2UnsupportedPayload::RichMime,
        ));
    }
    let timestamp = i64::try_from(envelope.timestamp_ms)
        .ok()
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .ok_or(ClipboardEnvelopeV2BoundaryError::TimestampOutOfRange {
            timestamp_ms: envelope.timestamp_ms,
        })?
        .to_rfc3339();
    let source = format!(
        "v2:{}:{}:{}",
        envelope.source_node, envelope.source_seat, envelope.source_session
    );
    Ok(ClipboardEnvelopeV2TextFold {
        envelope,
        text,
        source,
        time: timestamp,
    })
}

/// Payload forms the current daemon handoff must refuse for the collaboration
/// contract. Refusal is explicit: no representation is truncated, guessed, or
/// copied into the legacy text history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollabClipboardEnvelopeV2UnsupportedPayload {
    /// Bytes remain owned by Files; this worker has no Files materializer.
    FilesReference,
    /// More than the sole exact `text/plain` representation was offered.
    RichMime,
    /// The producer explicitly marked the representation unsupported.
    UnsupportedState,
    /// The producer explicitly marked the representation unavailable.
    UnavailableState,
}

/// Admission and materialization failures for the `mde-collab-types` lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollabClipboardEnvelopeV2BoundaryError {
    /// The bounded signed envelope could not be decoded.
    Decode(String),
    /// Intrinsic signature, expiry, replay, or echo admission failed.
    Admission(CollabClipboardEnvelopeV2ValidationError),
    /// The current text handoff cannot truthfully preserve the offered payload.
    UnsupportedPayload(CollabClipboardEnvelopeV2UnsupportedPayload),
    /// The envelope is addressed to a different node or seat.
    WrongTarget,
    /// The source session lacks fresh explicit publishing consent.
    Consent(ClipboardSessionConsentBoundaryError),
    /// The bounded source/session replay table has no free lane.
    LedgerCapacityExceeded {
        /// Maximum retained source/session identities.
        max: usize,
    },
    /// The inline text exceeded the existing VDI materialization contract.
    TextMaterialization(String),
    /// The contract timestamp cannot be represented by the handoff schema.
    TimestampOutOfRange {
        /// Creation timestamp in Unix milliseconds.
        timestamp_ms: u64,
    },
}

impl std::fmt::Display for CollabClipboardEnvelopeV2BoundaryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(error) => {
                write!(
                    formatter,
                    "collaboration clipboard V2 decode failed: {error}"
                )
            }
            Self::Admission(error) => {
                write!(
                    formatter,
                    "collaboration clipboard V2 admission failed: {error}"
                )
            }
            Self::UnsupportedPayload(
                CollabClipboardEnvelopeV2UnsupportedPayload::FilesReference,
            ) => formatter
                .write_str("collaboration clipboard Files payload requires the Files materializer"),
            Self::UnsupportedPayload(CollabClipboardEnvelopeV2UnsupportedPayload::RichMime) => {
                formatter.write_str(
                    "collaboration clipboard rich MIME cannot be downgraded to plain text",
                )
            }
            Self::UnsupportedPayload(
                CollabClipboardEnvelopeV2UnsupportedPayload::UnsupportedState,
            ) => formatter
                .write_str("collaboration clipboard representation is explicitly unsupported"),
            Self::UnsupportedPayload(
                CollabClipboardEnvelopeV2UnsupportedPayload::UnavailableState,
            ) => formatter
                .write_str("collaboration clipboard representation is explicitly unavailable"),
            Self::WrongTarget => {
                formatter.write_str("collaboration clipboard envelope targets another seat")
            }
            Self::Consent(error) => {
                write!(formatter, "collaboration clipboard consent failed: {error}")
            }
            Self::LedgerCapacityExceeded { max } => write!(
                formatter,
                "collaboration clipboard replay ledger is full (maximum {max})"
            ),
            Self::TextMaterialization(error) => write!(
                formatter,
                "collaboration clipboard text materialization failed: {error}"
            ),
            Self::TimestampOutOfRange { timestamp_ms } => write!(
                formatter,
                "collaboration clipboard timestamp {timestamp_ms} is outside the RFC3339 range"
            ),
        }
    }
}

impl std::error::Error for CollabClipboardEnvelopeV2BoundaryError {}

/// The sole collaboration-envelope representation this worker can currently
/// hand to the compositor-less seat provider. The original signed envelope is
/// retained only for the duration of the drain call; replay state records only
/// its bounded identity and sequence metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollabClipboardEnvelopeV2TextFold {
    /// Admitted signed contract.
    pub envelope: CollabClipboardEnvelopeV2,
    /// Exact bounded UTF-8 text.
    pub text: VdiClipboardText,
    /// Safe source attribution for the existing seat handoff.
    pub source: String,
    /// Source creation time in the handoff's existing RFC3339 form.
    pub time: String,
}

/// Fold only a sole inline `text/plain` collaboration offer. Files references,
/// rich MIME, and explicit unsupported/unavailable states fail closed and are
/// never converted into legacy history or raw-byte storage.
pub fn fold_collab_clipboard_envelope_v2(
    envelope: CollabClipboardEnvelopeV2,
) -> Result<CollabClipboardEnvelopeV2TextFold, CollabClipboardEnvelopeV2BoundaryError> {
    envelope
        .validate()
        .map_err(CollabClipboardEnvelopeV2BoundaryError::Admission)?;
    if envelope.offers.len() != 1 {
        return Err(CollabClipboardEnvelopeV2BoundaryError::UnsupportedPayload(
            CollabClipboardEnvelopeV2UnsupportedPayload::RichMime,
        ));
    }
    let offer = &envelope.offers[0];
    let text = match &offer.payload {
        CollabClipboardPayloadV2::InlineText { text }
            if offer.mime == CollabClipboardMimeKind::TextPlain =>
        {
            VdiClipboardText::new(text.clone()).map_err(|error| {
                CollabClipboardEnvelopeV2BoundaryError::TextMaterialization(error.to_string())
            })?
        }
        CollabClipboardPayloadV2::InlineText { .. } => {
            return Err(CollabClipboardEnvelopeV2BoundaryError::UnsupportedPayload(
                CollabClipboardEnvelopeV2UnsupportedPayload::RichMime,
            ));
        }
        CollabClipboardPayloadV2::FilesReference { .. } => {
            return Err(CollabClipboardEnvelopeV2BoundaryError::UnsupportedPayload(
                CollabClipboardEnvelopeV2UnsupportedPayload::FilesReference,
            ));
        }
        CollabClipboardPayloadV2::Unsupported { .. } => {
            return Err(CollabClipboardEnvelopeV2BoundaryError::UnsupportedPayload(
                CollabClipboardEnvelopeV2UnsupportedPayload::UnsupportedState,
            ));
        }
        CollabClipboardPayloadV2::Unavailable { .. } => {
            return Err(CollabClipboardEnvelopeV2BoundaryError::UnsupportedPayload(
                CollabClipboardEnvelopeV2UnsupportedPayload::UnavailableState,
            ));
        }
    };
    let time = i64::try_from(envelope.created_unix_ms)
        .ok()
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .ok_or(
            CollabClipboardEnvelopeV2BoundaryError::TimestampOutOfRange {
                timestamp_ms: envelope.created_unix_ms,
            },
        )?
        .to_rfc3339();
    let source = format!(
        "collab-v2:{}:{}:{}",
        envelope.source.node, envelope.source.seat, envelope.session
    );
    Ok(CollabClipboardEnvelopeV2TextFold {
        envelope,
        text,
        source,
        time,
    })
}

/// The mesh-global clipboard history (newest first). Serialized as the
/// whole `clipboard/history.json` document so a tailing node reads one
/// stable shape.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    /// Entries, newest first (index 0 is the current clipboard top).
    #[serde(default)]
    pub entries: Vec<ClipEntry>,
}

/// Content fingerprint for an entry id — a short hex SHA-256 prefix of the
/// text. Stable across nodes so the same clip dedups to one id mesh-wide.
#[must_use]
pub fn clip_id(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(text.as_bytes());
    // 16 hex chars (64 bits) is ample to avoid collisions across a 50+pin
    // history while staying short in the JSON + the bus body.
    let mut s = String::with_capacity(16);
    for b in &digest[..8] {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Apply a freshly captured clip to the history (pure — the whole O2/O3/O7
/// policy lives here, unit-tested without any I/O).
///
/// Returns `true` when the history changed (the caller then persists);
/// `false` when the clip was debounced away (O2) and nothing should be
/// written.
///
///   * **O2 debounce** — if `text` equals the current top entry's text, it
///     is a no-op (drops the click-to-load echo + a redundant re-copy of
///     the same already-top clip).
///   * **O3 dedup move-to-top** — if `text` matches a *lower* existing
///     entry, that entry is moved to the front (its pinned flag preserved)
///     rather than duplicated.
///   * **new** — otherwise a fresh entry is pushed to the front.
///   * **O7 cap** — after insertion, unpinned entries beyond
///     [`HISTORY_CAP`] are trimmed (oldest first); pinned entries are
///     never counted nor trimmed.
pub fn apply_clip(history: &mut History, text: &str, source: &str, now: &str) -> bool {
    let clip = ClipEventBody::from_text(text, source, now);
    apply_clip_event(history, &clip)
}

/// Apply a canonical `event/clipboard/clip` body to the history.
///
/// Preserves the event's `{ id, text, source, time }` fields and keeps `pinned`
/// as durable history-only state: moving an existing entry preserves its pin,
/// while a new event is always inserted unpinned.
#[must_use]
pub fn apply_clip_event(history: &mut History, clip: &ClipEventBody) -> bool {
    if clip.text.trim().is_empty() || clip.text.len() > MAX_CLIP_BYTES {
        return false;
    }
    // O2 — identical to the current top → debounce (no change, no echo).
    if history.entries.first().is_some_and(|e| e.text == clip.text) {
        return false;
    }
    // O3 — same text lower in the list → move it to the top, keeping its
    // pin + id, refreshing source/time to the capture that re-surfaced it.
    if let Some(pos) = history
        .entries
        .iter()
        .position(|e| e.id == clip.id || e.text == clip.text)
    {
        let mut existing = history.entries.remove(pos);
        existing.id = clip.id.clone();
        existing.text = clip.text.clone();
        existing.source = clip.source.clone();
        existing.time = clip.time.clone();
        history.entries.insert(0, existing);
    } else {
        history.entries.insert(
            0,
            ClipEntry {
                id: clip.id.clone(),
                text: clip.text.clone(),
                source: clip.source.clone(),
                time: clip.time.clone(),
                pinned: false,
            },
        );
    }
    trim_unpinned(history, HISTORY_CAP);
    true
}

/// Parse the canonical `event/clipboard/clip` Bus body.
///
/// # Errors
/// Human-readable validation error for malformed JSON, missing required fields,
/// or a non-RFC3339 timestamp.
pub fn parse_clip_event_body(body: &str) -> Result<ClipEventBody, String> {
    let clip: ClipEventBody =
        serde_json::from_str(body).map_err(|e| format!("malformed clipboard clip body: {e}"))?;
    if clip.id.trim().is_empty() {
        return Err("clipboard clip body missing `id`".to_string());
    }
    if clip.text.trim().is_empty() {
        return Err("clipboard clip body missing non-blank `text`".to_string());
    }
    if clip.source.trim().is_empty() {
        return Err("clipboard clip body missing `source`".to_string());
    }
    if clip.text.len() > MAX_CLIP_BYTES {
        return Err(format!(
            "clipboard clip body `text` exceeds {MAX_CLIP_BYTES} byte limit"
        ));
    }
    let expected_id = clip_id(&clip.text);
    if clip.id != expected_id {
        return Err(format!(
            "clipboard clip body `id` must match content fingerprint {expected_id}"
        ));
    }
    chrono::DateTime::parse_from_rfc3339(&clip.time)
        .map_err(|e| format!("clipboard clip body `time` must be RFC3339: {e}"))?;
    Ok(clip)
}

/// O7 — keep at most `cap` unpinned entries (oldest unpinned trimmed
/// first); pinned entries are exempt + unlimited. Preserves order.
pub fn trim_unpinned(history: &mut History, cap: usize) {
    // Entries are stored newest→oldest, so the *oldest* unpinned entries are
    // the last unpinned indices. Collect them in one pass, then drop the
    // oldest (tail) overflow — removing from the highest index first keeps
    // the earlier indices valid.
    let unpinned_idx: Vec<usize> = history
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.pinned)
        .map(|(i, _)| i)
        .collect();
    if unpinned_idx.len() <= cap {
        return;
    }
    for &idx in unpinned_idx[cap..].iter().rev() {
        history.entries.remove(idx);
    }
}

/// RFC3339 (UTC) timestamp for "now" — the stamp written into each entry.
#[must_use]
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// O6 — render a stored RFC3339 stamp as a short relative age ("just now",
/// "2m", "3h", "5d") for the viewer's "from <node> · <age>" label. Pure so
/// both the worker's logging and any consumer share one format; unknown /
/// future stamps fall back to "now".
#[must_use]
pub fn age_label(stamp: &str, now: chrono::DateTime<chrono::Utc>) -> String {
    let Ok(then) = chrono::DateTime::parse_from_rfc3339(stamp) else {
        return "now".to_string();
    };
    let secs = (now - then.with_timezone(&chrono::Utc)).num_seconds();
    if secs < 5 {
        "now".to_string()
    } else if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// The mesh-global history file under the replicated root.
#[must_use]
pub fn history_path(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("clipboard").join("history.json")
}

/// Read the shared history (an empty/missing/corrupt file → empty history,
/// never an error — a tailing node degrades gracefully pre-sync).
#[must_use]
pub fn read_history(path: &Path) -> History {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => History::default(),
    }
}

/// Atomic write-through of the history (tmp + rename), creating the
/// `clipboard/` dir as needed.
pub fn write_history(path: &Path, history: &History) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(history).map_err(|e| format!("encode: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body.as_bytes()).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("rename {} → {}: {e}", tmp.display(), path.display()))
}

/// The durable acknowledgement for one daemon's clipboard event lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CursorRecord {
    topic: String,
    ulid: String,
}

/// Locate the local cursor, next to the Bus data rather than in shared history.
#[must_use]
fn cursor_path(bus_root: &Path) -> PathBuf {
    bus_root.join(CURSOR_FILE_NAME)
}

/// Read a cursor only when it is for this exact lane. A malformed or foreign
/// record is treated as absent so a damaged local cursor cannot suppress new
/// events or make the worker consume an unrelated topic.
#[must_use]
fn read_cursor(path: &Path) -> Option<String> {
    let record: CursorRecord = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    (record.topic == CLIP_TOPIC && !record.ulid.trim().is_empty()).then_some(record.ulid)
}

/// Atomically persist the lane acknowledgement after its history mutation has
/// succeeded. A failed checkpoint is reported to the caller; the in-memory
/// cursor is intentionally not advanced, so the event is safely replayable.
fn write_cursor(path: &Path, ulid: &str) -> Result<(), String> {
    let record = CursorRecord {
        topic: CLIP_TOPIC.to_string(),
        ulid: ulid.to_string(),
    };
    let body = serde_json::to_vec(&record).map_err(|e| format!("encode cursor: {e}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("rename {} → {}: {e}", tmp.display(), path.display()))
}

/// Locate the local V2 cursor beside the legacy clipboard cursor.
#[must_use]
fn v2_cursor_path(bus_root: &Path) -> PathBuf {
    bus_root.join(V2_CURSOR_FILE_NAME)
}

/// Read a cursor for the explicit V2 lane. A malformed or foreign record is
/// treated as absent, matching the legacy cursor's fail-open replay behavior.
#[must_use]
fn read_v2_cursor(path: &Path) -> Option<String> {
    let record: CursorRecord = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    (record.topic == CLIPBOARD_ENVELOPE_V2_TOPIC && !record.ulid.trim().is_empty())
        .then_some(record.ulid)
}

/// Atomically persist the V2 lane acknowledgement.
fn write_v2_cursor(path: &Path, ulid: &str) -> Result<(), String> {
    let record = CursorRecord {
        topic: CLIPBOARD_ENVELOPE_V2_TOPIC.to_string(),
        ulid: ulid.to_string(),
    };
    let body = serde_json::to_vec(&record).map_err(|e| format!("encode V2 cursor: {e}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("rename {} → {}: {e}", tmp.display(), path.display()))
}

/// Locate the collaboration-envelope cursor beside the other clipboard lanes.
#[must_use]
fn collab_v2_cursor_path(bus_root: &Path) -> PathBuf {
    bus_root.join(COLLAB_V2_CURSOR_FILE_NAME)
}

/// Read only a cursor for the exact collaboration-envelope topic.
#[must_use]
fn read_collab_v2_cursor(path: &Path) -> Option<String> {
    let record: CursorRecord = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    (record.topic == COLLAB_CLIPBOARD_ENVELOPE_V2_TOPIC && !record.ulid.trim().is_empty())
        .then_some(record.ulid)
}

/// Atomically persist a collaboration-envelope acknowledgement. The cursor
/// contains only a topic and ULID; no clipboard payload reaches this file.
fn write_collab_v2_cursor(path: &Path, ulid: &str) -> Result<(), String> {
    let record = CursorRecord {
        topic: COLLAB_CLIPBOARD_ENVELOPE_V2_TOPIC.to_string(),
        ulid: ulid.to_string(),
    };
    let body = serde_json::to_vec(&record).map_err(|e| format!("encode collab V2 cursor: {e}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("rename {} → {}: {e}", tmp.display(), path.display()))
}

/// Locate the local authenticated consent cursor beside the clipboard lane
/// cursors. Consent state itself is intentionally in-memory and therefore
/// resets to disabled on every daemon start; this cursor only acknowledges
/// already-consumed Bus rows.
#[must_use]
fn consent_cursor_path(bus_root: &Path) -> PathBuf {
    bus_root.join(CONSENT_CURSOR_FILE_NAME)
}

/// Read a cursor only when it names the consent-control lane.
#[must_use]
fn read_consent_cursor(path: &Path) -> Option<String> {
    let record: CursorRecord = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    (record.topic == CLIPBOARD_SESSION_CONSENT_TOPIC && !record.ulid.trim().is_empty())
        .then_some(record.ulid)
}

/// Atomically acknowledge one terminal consent-control row.
fn write_consent_cursor(path: &Path, ulid: &str) -> Result<(), String> {
    let record = CursorRecord {
        topic: CLIPBOARD_SESSION_CONSENT_TOPIC.to_string(),
        ulid: ulid.to_string(),
    };
    let body = serde_json::to_vec(&record).map_err(|e| format!("encode consent cursor: {e}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("rename {} → {}: {e}", tmp.display(), path.display()))
}

/// Compact source-session high-water marker. Retaining only this metadata keeps
/// the replay guard bounded even when an admitted inline payload is large.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ClipboardEnvelopeV2ReplayMarker {
    source_node: String,
    source_seat: String,
    source_session: String,
    sequence: u64,
}

#[derive(Debug, Default)]
struct ClipboardEnvelopeV2Ledger {
    latest_by_session: BTreeMap<String, ClipboardEnvelopeV2ReplayMarker>,
}

impl ClipboardEnvelopeV2Ledger {
    /// Seed only bounded retained metadata at daemon startup. This prevents a
    /// replay published after a restart from being accepted merely because the
    /// worker's in-memory ledger was reset; retained payloads are never folded.
    fn seed_from_retained(&mut self, persist: &Persist) {
        let Ok(messages) = persist.read_tail(CLIPBOARD_ENVELOPE_V2_TOPIC, MAX_V2_SOURCE_LANES)
        else {
            return;
        };
        for message in messages {
            let Some(body) = message.body.as_deref() else {
                continue;
            };
            let Ok(envelope) = ClipboardEnvelopeV2::from_json(body) else {
                continue;
            };
            self.record(&envelope);
        }
    }

    fn admit(
        &self,
        body: &[u8],
        now_ms: u64,
    ) -> Result<ClipboardEnvelopeV2, ClipboardEnvelopeV2BoundaryError> {
        let decoded = ClipboardEnvelopeV2::from_json_bytes(body)
            .map_err(|error| ClipboardEnvelopeV2BoundaryError::Decode(error.to_string()))?;
        let previous = self.latest_by_session.get(&decoded.source_session);
        if let Some(previous) = previous {
            decoded
                .admit(
                    &previous.source_node,
                    &previous.source_seat,
                    &previous.source_session,
                    None,
                    now_ms,
                )
                .map_err(ClipboardEnvelopeV2BoundaryError::Admission)?;
            if decoded.sequence <= previous.sequence {
                return Err(ClipboardEnvelopeV2BoundaryError::Admission(
                    ClipboardEnvelopeV2ValidationError::Replay {
                        previous: previous.sequence,
                        received: decoded.sequence,
                    },
                ));
            }
        } else {
            if self.latest_by_session.len() >= MAX_V2_SOURCE_LANES {
                return Err(ClipboardEnvelopeV2BoundaryError::LedgerCapacityExceeded {
                    max: MAX_V2_SOURCE_LANES,
                });
            }
            decoded
                .admit(
                    &decoded.source_node,
                    &decoded.source_seat,
                    &decoded.source_session,
                    None,
                    now_ms,
                )
                .map_err(ClipboardEnvelopeV2BoundaryError::Admission)?;
        }
        Ok(decoded)
    }

    fn record(&mut self, envelope: &ClipboardEnvelopeV2) {
        let marker = ClipboardEnvelopeV2ReplayMarker {
            source_node: envelope.source_node.clone(),
            source_seat: envelope.source_seat.clone(),
            source_session: envelope.source_session.clone(),
            sequence: envelope.sequence,
        };
        let replace = self
            .latest_by_session
            .get(&envelope.source_session)
            .is_none_or(|previous| marker.sequence > previous.sequence);
        if replace {
            self.latest_by_session
                .insert(envelope.source_session.clone(), marker);
        }
    }
}

/// Safe identity key for one source clipboard session. Keeping all three
/// identity components in the key prevents a session id reused by another
/// node or seat from inheriting consent.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ClipboardSessionIdentityKey {
    source_node: String,
    source_seat: String,
    source_session: String,
}

impl ClipboardSessionIdentityKey {
    fn from_consent(consent: &ClipboardSessionConsentV1) -> Self {
        Self {
            source_node: consent.source_node.clone(),
            source_seat: consent.source_seat.clone(),
            source_session: consent.source_session.clone(),
        }
    }

    fn from_envelope(envelope: &ClipboardEnvelopeV2) -> Self {
        Self {
            source_node: envelope.source_node.clone(),
            source_seat: envelope.source_seat.clone(),
            source_session: envelope.source_session.clone(),
        }
    }

    fn from_collab_envelope(envelope: &CollabClipboardEnvelopeV2) -> Self {
        Self {
            source_node: envelope.source.node.to_string(),
            source_seat: envelope.source.seat.to_string(),
            source_session: envelope.session.to_string(),
        }
    }
}

/// Bounded, payload-free replay ledger for the collaboration envelope lane.
/// The complete signed body remains in Bus retention owned by the producer;
/// this worker keeps only full source identity and its sequence high-water.
#[derive(Debug, Default)]
struct CollabClipboardEnvelopeV2Ledger {
    latest_by_identity: BTreeMap<ClipboardSessionIdentityKey, u64>,
}

impl CollabClipboardEnvelopeV2Ledger {
    /// Seed replay metadata from retained envelopes without materializing them.
    fn seed_from_retained(&mut self, persist: &Persist) {
        let Ok(messages) = persist.read_tail(
            COLLAB_CLIPBOARD_ENVELOPE_V2_TOPIC,
            MAX_COLLAB_V2_SOURCE_LANES,
        ) else {
            return;
        };
        for message in messages {
            let Some(body) = message.body.as_deref() else {
                continue;
            };
            let Ok(envelope) = CollabClipboardEnvelopeV2::from_json(body) else {
                continue;
            };
            self.record(&envelope);
        }
    }

    fn admit(
        &self,
        body: &[u8],
        now_ms: u64,
    ) -> Result<CollabClipboardEnvelopeV2, CollabClipboardEnvelopeV2BoundaryError> {
        let envelope = CollabClipboardEnvelopeV2::from_json_bytes(body)
            .map_err(|error| CollabClipboardEnvelopeV2BoundaryError::Decode(error.to_string()))?;
        let key = ClipboardSessionIdentityKey::from_collab_envelope(&envelope);
        let previous = self.latest_by_identity.get(&key).copied();
        if previous.is_none() && self.latest_by_identity.len() >= MAX_COLLAB_V2_SOURCE_LANES {
            return Err(
                CollabClipboardEnvelopeV2BoundaryError::LedgerCapacityExceeded {
                    max: MAX_COLLAB_V2_SOURCE_LANES,
                },
            );
        }
        envelope
            .validate_at(now_ms, previous)
            .map_err(CollabClipboardEnvelopeV2BoundaryError::Admission)?;
        Ok(envelope)
    }

    fn record(&mut self, envelope: &CollabClipboardEnvelopeV2) {
        let key = ClipboardSessionIdentityKey::from_collab_envelope(envelope);
        let sequence = envelope.sequence;
        if self
            .latest_by_identity
            .get(&key)
            .is_none_or(|previous| sequence > *previous)
        {
            self.latest_by_identity.insert(key, sequence);
        }
    }
}

/// Daemon-local, bounded consent state for the V2 clipboard lane.
///
/// This is intentionally an in-memory typed ledger behind the authenticated
/// consent topic. Production constructs it empty on every daemon start, so no
/// V2 envelope can be materialized until a fresh signed control is admitted.
#[derive(Debug, Default)]
pub struct ClipboardSessionConsentLedger {
    latest_by_identity: BTreeMap<ClipboardSessionIdentityKey, ClipboardSessionConsentV1>,
}

impl ClipboardSessionConsentLedger {
    /// Admit one consent update after validating its identity, freshness,
    /// explicit state, and strict monotonic update ordering.
    pub fn admit(
        &mut self,
        consent: ClipboardSessionConsentV1,
        now_ms: u64,
    ) -> Result<(), ClipboardSessionConsentBoundaryError> {
        let key = ClipboardSessionIdentityKey::from_consent(&consent);
        let previous = self.latest_by_identity.get(&key);
        consent
            .admit(
                &consent.source_node,
                &consent.source_seat,
                &consent.source_session,
                previous,
                now_ms,
            )
            .map_err(ClipboardSessionConsentBoundaryError::Admission)?;
        if previous.is_none() && self.latest_by_identity.len() >= MAX_V2_CONSENT_SESSIONS {
            return Err(
                ClipboardSessionConsentBoundaryError::LedgerCapacityExceeded {
                    max: MAX_V2_CONSENT_SESSIONS,
                },
            );
        }
        self.latest_by_identity.insert(key, consent);
        Ok(())
    }

    /// Require an explicitly enabled, fresh consent for this exact envelope
    /// identity before any payload fold or materialization is attempted.
    fn authorize_envelope(
        &self,
        envelope: &ClipboardEnvelopeV2,
        now_ms: u64,
    ) -> Result<(), ClipboardSessionConsentBoundaryError> {
        self.authorize_identity(
            ClipboardSessionIdentityKey::from_envelope(envelope),
            &envelope.source_node,
            &envelope.source_seat,
            &envelope.source_session,
            now_ms,
        )
    }

    /// Apply the same authenticated, session-scoped consent boundary to the
    /// collaboration contract without weakening either wire format.
    fn authorize_collab_envelope(
        &self,
        envelope: &CollabClipboardEnvelopeV2,
        now_ms: u64,
    ) -> Result<(), ClipboardSessionConsentBoundaryError> {
        let source_node = envelope.source.node.to_string();
        let source_seat = envelope.source.seat.to_string();
        let source_session = envelope.session.to_string();
        self.authorize_identity(
            ClipboardSessionIdentityKey::from_collab_envelope(envelope),
            &source_node,
            &source_seat,
            &source_session,
            now_ms,
        )
    }

    fn authorize_identity(
        &self,
        key: ClipboardSessionIdentityKey,
        source_node: &str,
        source_seat: &str,
        source_session: &str,
        now_ms: u64,
    ) -> Result<(), ClipboardSessionConsentBoundaryError> {
        let Some(consent) = self.latest_by_identity.get(&key) else {
            return Err(ClipboardSessionConsentBoundaryError::Missing);
        };
        let enabled = consent
            .allows_clipboard_at(source_node, source_seat, source_session, None, now_ms)
            .map_err(ClipboardSessionConsentBoundaryError::Admission)?;
        if !enabled {
            return Err(ClipboardSessionConsentBoundaryError::Disabled);
        }
        Ok(())
    }
}

/// Writability for the shared clipboard history.
///
/// Pure core — `root_is_dir` is injected so it unit-tests without touching the
/// filesystem. See [`ClipboardSyncWorker::share_writable`] for the why.
///
/// Under SUBSTRATE-V2 `/mnt/mesh-storage` is a plain Syncthing directory.
/// Writable **iff the canonical root actually exists as a directory**: a present
/// plain dir is fine, but a missing/unprovisioned share (early boot before
/// Syncthing creates it) is NOT written into — that avoids a per-clip write error
/// landing on a bare local dir. Any non-canonical root (dev tree / tempdir) is
/// always writable.
#[must_use]
pub fn clip_share_writable_core(workgroup_root: &Path, root_is_dir: bool) -> bool {
    crate::shared_root_writable_core(workgroup_root, root_is_dir)
}

/// Writability for the shared clipboard history, reading the shared root's
/// directory state. Thin I/O wrapper over [`clip_share_writable_core`].
#[must_use]
pub fn clip_share_writable(workgroup_root: &Path) -> bool {
    clip_share_writable_core(workgroup_root, workgroup_root.is_dir())
}

/// The clipboard-sync worker. Holds the replicated root and folds canonical
/// `event/clipboard/clip` Bus bodies through [`apply_clip_event`].
pub struct ClipboardSyncWorker {
    workgroup_root: PathBuf,
    /// Local node identity used to reject collaboration envelopes addressed to
    /// another daemon before consent or materialization.
    target_node: String,
    /// Exact direct-seat identity that may consume a replicated history
    /// materialization on this node.
    target_seat: String,
    /// Bus root override (tests). `None` ⇒ [`crate::bus_publish::default_bus_root`].
    bus_root_override: Option<PathBuf>,
    /// Bus drain cadence.
    poll: Duration,
    /// Root-only signer for the daemon-authored VNC guest→seat handoff. A
    /// missing credential disables this handoff honestly; it never publishes
    /// an unsigned action body.
    vnc_action_signer: Option<CloudArmSigner>,
    /// Verifier for the root-authenticated clipboard session-consent control
    /// lane. Missing production credentials fail closed.
    consent_authorizer: Arc<ActionAuthorizer>,
    /// Read-only enrollment key/availability projection used to bind signed
    /// clipboard frames to authenticated mesh peers.
    mesh_peer_directory: Arc<dyn mesh::MeshClipboardPeerDirectory>,
}

impl ClipboardSyncWorker {
    /// Build the worker rooted at the replicated workgroup root.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self {
            workgroup_root,
            target_node: local_node_id(),
            target_seat: local_target_seat(),
            bus_root_override: None,
            poll: CLIP_EVENT_POLL_INTERVAL,
            vnc_action_signer: production_action_signer().ok(),
            consent_authorizer: Arc::new(ActionAuthorizer::production()),
            mesh_peer_directory: Arc::new(mesh::SqliteMeshClipboardPeerDirectory::new(
                crate::default_db_path(),
            )),
        }
    }

    /// Override the Bus root (tests).
    #[cfg(test)]
    #[must_use]
    fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    /// Override the direct-seat identity in deterministic tests.
    #[cfg(test)]
    #[must_use]
    fn with_target_seat(mut self, target_seat: impl Into<String>) -> Self {
        self.target_seat = target_seat.into();
        self
    }

    /// Override the local node identity for collaboration-envelope tests.
    #[cfg(test)]
    #[must_use]
    fn with_target_node(mut self, target_node: impl Into<String>) -> Self {
        self.target_node = target_node.into();
        self
    }

    fn bus_root(&self) -> Option<PathBuf> {
        self.bus_root_override
            .clone()
            .or_else(crate::bus_publish::default_bus_root)
    }

    /// Require the collaboration envelope's typed destination to identify this
    /// exact daemon node and seat. The existing materialization topic uses the
    /// historical `seat:<hostname>` spelling, while the strict collaboration
    /// identity alphabet intentionally excludes `:`; compare its suffix only.
    fn collab_target_matches(&self, envelope: &CollabClipboardEnvelopeV2) -> bool {
        let target_seat = self
            .target_seat
            .strip_prefix("seat:")
            .unwrap_or(&self.target_seat);
        envelope.target.node.as_str() == self.target_node
            && envelope.target.seat.as_str() == target_seat
    }

    /// Inject the daemon action signer in unit tests. Production obtains it
    /// from the root-only systemd credential in [`Self::new`].
    #[cfg(test)]
    #[must_use]
    fn with_vnc_action_signer(mut self, signer: CloudArmSigner) -> Self {
        self.vnc_action_signer = Some(signer);
        self
    }

    /// Inject the consent capability verifier in deterministic tests.
    #[cfg(test)]
    #[must_use]
    fn with_consent_authorizer(mut self, authorizer: Arc<ActionAuthorizer>) -> Self {
        self.consent_authorizer = authorizer;
        self
    }

    /// Decode the shell VNC source identity without trusting it as a route.
    /// The returned `(serving_peer, session_id)` is checked against the
    /// authoritative session records before it can become a target seat.
    fn parse_vnc_source(source: &str) -> Option<(&str, &str)> {
        let rest = source.strip_prefix(VNC_SOURCE_PREFIX)?;
        let (serving_peer, session_id) = rest.split_once(':')?;
        if serving_peer.trim().is_empty() || session_id.trim().is_empty() {
            return None;
        }
        Some((serving_peer, session_id))
    }

    /// Select the same etcd-first / replicated-file fallback store as the
    /// session broker. Clipboard routing must not read a different authority
    /// when the lease-backed session plane is enabled.
    fn session_store(&self) -> Box<dyn SessionStore + Send + Sync> {
        let endpoints = crate::substrate::etcd::default_endpoints();
        if endpoints.is_empty() {
            Box::new(MeshSessionStore::new(self.workgroup_root.clone()))
        } else {
            Box::new(EtcdSessionStore::new(endpoints))
        }
    }

    /// Resolve a VNC guest event to the active client's exact local target.
    /// The canonical event is not itself an authorization envelope, so the
    /// source is only a lookup hint; the active session roster is the authority.
    fn vnc_target_seat(&self, clip: &ClipEventBody) -> Result<Option<(String, String)>, String> {
        let Some((serving_peer, session_id)) = Self::parse_vnc_source(&clip.source) else {
            return Ok(None);
        };
        let sessions = self
            .session_store()
            .list()
            .map_err(|error| format!("read VDI session roster for VNC clipboard: {error}"))?;
        let Some(session) = sessions.into_iter().find(|session| {
            session.id == session_id
                && session.serving_peer == serving_peer
                && session.state == SessionState::Active
        }) else {
            // A stale/disconnected VNC event must never be guessed onto a seat.
            return Ok(Some((String::new(), String::new())));
        };
        super::clipboard_bridge::validate_target_seat(&session.client_peer).map_err(|error| {
            format!("VNC session client peer is not a safe target seat: {error}")
        })?;
        Ok(Some((session.id, session.client_peer)))
    }

    /// Mint the exact-body capability consumed by `clipboard_bridge` for one
    /// VNC guest→client event. Each publication attempt gets a fresh nonce:
    /// authorization consumes a nonce before the adapter write, so retrying a
    /// failed adapter must be able to obtain a new capability. The bridge's
    /// session/payload echo guard handles duplicate successful publications.
    fn signed_vnc_action(
        clip: &ClipEventBody,
        session_id: &str,
        target_seat: &str,
        signer: &CloudArmSigner,
    ) -> Result<String, String> {
        let event = ClipboardEvent {
            session_id: session_id.to_owned(),
            target_seat: target_seat.to_owned(),
            direction: ClipDirection::GuestToClient,
            payload: ClipPayload::checked(
                super::clipboard_bridge::ClipFormat::Text,
                clip.text.clone(),
            )
            .map_err(|error| format!("VNC clipboard payload rejected: {error}"))?,
            source: Some(clip.source.clone()),
        };
        let mut document = serde_json::to_value(event)
            .map_err(|error| format!("serialize VNC clipboard action: {error}"))?;
        document
            .as_object_mut()
            .ok_or_else(|| "VNC clipboard action is not a JSON object".to_string())?
            .insert(
                "schema_version".to_string(),
                serde_json::Value::from(ACTION_SCHEMA_VERSION),
            );
        let unsigned = document.to_string();
        let target = format!("session:{session_id}:seat:{target_seat}");
        let nonce = uuid::Uuid::new_v4().to_string();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "system clock is before the Unix epoch".to_string())
            .and_then(|duration| {
                i64::try_from(duration.as_millis())
                    .map_err(|_| "system clock is beyond the capability range".to_string())
            })?;
        let token = CloudArmedToken::mint(
            signer,
            &nonce,
            now_ms.saturating_add(MAX_AUTH_TTL_MS),
            super::clipboard_bridge::ACTION_AUTH_VERB,
            super::clipboard_bridge::ACTION_AUTH_NODE_SCOPE,
            &target,
            &cloud_request_digest(&unsigned).map_err(str::to_string)?,
        )
        .encode();
        document
            .as_object_mut()
            .ok_or_else(|| "VNC clipboard action is not a JSON object".to_string())?
            .insert("armed_token".to_string(), serde_json::Value::String(token));
        serde_json::to_string(&document)
            .map_err(|error| format!("serialize signed VNC clipboard action: {error}"))
    }

    /// Convert one accepted canonical VNC event into the signed action lane.
    /// `Ok(true)` means the event may be acknowledged; `Ok(false)` is reserved
    /// for a future deferred route. A missing/stale session is acknowledged but
    /// never guessed onto another seat.
    fn publish_vnc_action(
        &self,
        persist: &mut Persist,
        clip: &ClipEventBody,
    ) -> Result<bool, String> {
        let Some((session_id, target_seat)) = self.vnc_target_seat(clip)? else {
            return Ok(true);
        };
        if session_id.is_empty() {
            warn!(source = %clip.source, "discarding VNC clipboard event without an active matching session");
            return Ok(true);
        }
        let Some(signer) = self.vnc_action_signer.as_ref() else {
            return Err("VNC clipboard action signer is unavailable".to_string());
        };
        let body = Self::signed_vnc_action(clip, &session_id, &target_seat, signer)?;
        persist
            .write(
                super::clipboard_bridge::ACTION_TOPIC,
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(&body),
            )
            .map(|_| true)
            .map_err(|error| format!("publish signed VNC clipboard action: {error}"))
    }

    /// Whether it is safe to write `clipboard/history.json` under the shared
    /// root, **substrate-aware** (mirrors the boot_readiness SUBSTRATE-10
    /// probe).
    ///
    /// Post-SUBSTRATE-V2 `/mnt/mesh-storage` is a **plain Syncthing directory,
    /// not a FUSE mount** (design `substrate-v2.md` Q3/Q8: "now a plain local
    /// dir (NO FUSE)"), so a guard that gates the canonical path on a real
    /// `/proc/mounts` entry ([`crate::shared_root_writable`]) returns `false`
    /// for it and the worker would silently drop **every** clip —
    /// `history.json` is never written and the Hub's Clipboard Viewer reads an
    /// always-empty `action/clipboard/list`. When the etcd coordination plane
    /// is provisioned (the SUBSTRATE-1 endpoints file is present) the node is
    /// on SUBSTRATE-V2, the shared root is a plain dir, and there is no
    /// mountpoint to check — so it is writable. Absent the endpoints file we
    /// fall back to the dir-exists guard.
    fn share_writable(&self) -> bool {
        clip_share_writable(&self.workgroup_root)
    }

    /// Fold one canonical Bus clip into the shared history.
    fn handle_clip_event(&self, clip: &ClipEventBody) -> Result<bool, String> {
        if !self.share_writable() {
            return Ok(false);
        }
        let path = history_path(&self.workgroup_root);
        let mut history = read_history(&path);
        if !apply_clip_event(&mut history, clip) {
            return Ok(false);
        }
        write_history(&path, &history)?;
        Ok(true)
    }

    /// Publish a newly-observed replicated history head into this node's
    /// target-seat materialization lane. The shared history is the durable
    /// mesh transport; this local handoff is what lets the compositor-less DRM
    /// provider consume the value without fabricating a local capture event.
    fn materialize_replicated_head(
        &self,
        persist: &mut Persist,
        observed: &mut Option<ClipEventBody>,
    ) -> Result<bool, String> {
        let latest = read_history(&history_path(&self.workgroup_root))
            .entries
            .first()
            .map(|entry| ClipEventBody {
                id: entry.id.clone(),
                text: entry.text.clone(),
                source: entry.source.clone(),
                time: entry.time.clone(),
            });
        if latest == *observed {
            return Ok(false);
        }
        let Some(clip) = latest else {
            *observed = None;
            return Ok(false);
        };

        // Treat a malformed replicated row as observed so it is reported once,
        // never retried as a 400 ms log storm. A later valid head still differs
        // and will be delivered normally.
        if let Err(error) = parse_clip_event_body(
            &serde_json::to_string(&clip)
                .map_err(|encode| format!("encode replicated clipboard head: {encode}"))?,
        ) {
            *observed = Some(clip);
            return Err(format!(
                "refused malformed replicated clipboard head: {error}"
            ));
        }
        let text = VdiClipboardText::new(clip.text.clone())
            .map_err(|error| format!("replicated clipboard text rejected: {error}"))?;
        let handoff = ClipboardMaterialization::new(
            self.target_seat.clone(),
            text,
            clip.source.clone(),
            clip.time.clone(),
        );
        handoff
            .validate()
            .map_err(|error| format!("replicated clipboard handoff rejected: {error}"))?;
        let body = serde_json::to_string(&handoff)
            .map_err(|error| format!("encode replicated clipboard handoff: {error}"))?;
        persist
            .write(
                CLIPBOARD_MATERIALIZATION_TOPIC,
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(&body),
            )
            .map_err(|error| format!("publish replicated clipboard handoff: {error}"))?;
        *observed = Some(clip);
        Ok(true)
    }

    /// Emit the typed V2 text fold through the existing target-seat handoff.
    /// This method deliberately has no `ClipEventBody` conversion: the V2
    /// boundary has already proved that the sole offered representation is
    /// exactly `text/plain`.
    fn materialize_v2_text(
        &self,
        persist: &mut Persist,
        fold: &ClipboardEnvelopeV2TextFold,
    ) -> Result<(), String> {
        let handoff = ClipboardMaterialization::new(
            self.target_seat.clone(),
            fold.text.clone(),
            fold.source.clone(),
            fold.time.clone(),
        );
        handoff
            .validate()
            .map_err(|error| format!("V2 clipboard materialization rejected: {error}"))?;
        let body = serde_json::to_string(&handoff)
            .map_err(|error| format!("encode V2 clipboard materialization: {error}"))?;
        persist
            .write(
                CLIPBOARD_MATERIALIZATION_TOPIC,
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(&body),
            )
            .map(|_| ())
            .map_err(|error| format!("publish V2 clipboard materialization: {error}"))
    }

    /// Emit an admitted collaboration `text/plain` offer through the existing
    /// direct-seat handoff. This does not touch `clipboard/history.json`; raw
    /// binary, Files references, rich representations, and explicit refusal
    /// states never reach this method.
    fn materialize_collab_v2_text(
        &self,
        persist: &mut Persist,
        fold: &CollabClipboardEnvelopeV2TextFold,
    ) -> Result<(), String> {
        let handoff = ClipboardMaterialization::new(
            self.target_seat.clone(),
            fold.text.clone(),
            fold.source.clone(),
            fold.time.clone(),
        );
        handoff.validate().map_err(|error| {
            format!("collaboration clipboard materialization rejected: {error}")
        })?;
        let body = serde_json::to_string(&handoff)
            .map_err(|error| format!("encode collaboration clipboard materialization: {error}"))?;
        persist
            .write(
                CLIPBOARD_MATERIALIZATION_TOPIC,
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(&body),
            )
            .map(|_| ())
            .map_err(|error| format!("publish collaboration clipboard materialization: {error}"))
    }

    /// Acknowledge one terminal consent-control row. A failed checkpoint stops
    /// the current drain so a later row cannot leap over an unacknowledged row;
    /// the in-memory cursor remains unchanged and the Bus row is retryable.
    fn acknowledge_consent(
        cursor: &mut Option<String>,
        checkpoint: Option<&Path>,
        ulid: &str,
    ) -> bool {
        if let Some(path) = checkpoint {
            if let Err(error) = write_consent_cursor(path, ulid) {
                warn!(
                    target: "clipboard_sync",
                    ulid,
                    error = %error,
                    "clipboard consent cursor checkpoint failed"
                );
                return false;
            }
        }
        *cursor = Some(ulid.to_owned());
        true
    }

    /// Drain authenticated typed consent controls before V2 envelopes. Every
    /// malformed, unsigned, unauthorized, expired, or stale row is terminal
    /// for this cursor and cannot change the ledger. Only a body that passes
    /// strict typed decoding, exact capability binding, and the existing
    /// full-identity monotonic consent ledger can enable V2 materialization.
    fn drain_clipboard_consents(
        &self,
        persist: &mut Persist,
        cursor: &mut Option<String>,
        checkpoint: Option<&Path>,
        ledger: &mut ClipboardSessionConsentLedger,
        now_ms: u64,
    ) -> usize {
        persist.reopen_if_index_changed();
        let messages = match persist.list_since(CLIPBOARD_SESSION_CONSENT_TOPIC, cursor.as_deref())
        {
            Ok(messages) => messages,
            Err(error) => {
                debug!(
                    target: "clipboard_sync",
                    error = %error,
                    "clipboard consent drain failed"
                );
                return 0;
            }
        };
        let mut admitted = 0;
        for message in messages {
            let body = message.body.as_deref().unwrap_or("");
            let command = match ClipboardSessionConsentCommandV1::from_json(body) {
                Ok(command) => command,
                Err(error) => {
                    warn!(
                        target: "clipboard_sync",
                        ulid = %message.ulid,
                        error = %error,
                        "clipboard consent control rejected before authorization"
                    );
                    if !Self::acknowledge_consent(cursor, checkpoint, &message.ulid) {
                        break;
                    }
                    continue;
                }
            };
            let target = clipboard_session_consent_auth_target(&command.consent);
            if let Err(error) = self.consent_authorizer.authorize(
                body,
                MutationContext {
                    verb: CLIPBOARD_SESSION_CONSENT_AUTH_VERB,
                    node: &command.consent.source_node,
                    target: &target,
                },
            ) {
                warn!(
                    target: "clipboard_sync",
                    ulid = %message.ulid,
                    source_node = %command.consent.source_node,
                    source_seat = %command.consent.source_seat,
                    source_session = %command.consent.source_session,
                    error = %error,
                    "clipboard consent control unauthorized"
                );
                if !Self::acknowledge_consent(cursor, checkpoint, &message.ulid) {
                    break;
                }
                continue;
            }
            if let Err(error) = ledger.admit(command.consent, now_ms) {
                warn!(
                    target: "clipboard_sync",
                    ulid = %message.ulid,
                    error = %error,
                    "clipboard consent control failed ledger admission"
                );
                if !Self::acknowledge_consent(cursor, checkpoint, &message.ulid) {
                    break;
                }
                continue;
            }
            if !Self::acknowledge_consent(cursor, checkpoint, &message.ulid) {
                break;
            }
            admitted += 1;
        }
        admitted
    }

    /// Acknowledge one terminal V2 result. Malformed, expired, replayed, and
    /// currently unsupported envelopes are not retried forever; transient
    /// materialization failures return before this helper is called.
    fn acknowledge_v2(cursor: &mut Option<String>, checkpoint: Option<&Path>, ulid: &str) -> bool {
        if let Some(path) = checkpoint {
            if let Err(error) = write_v2_cursor(path, ulid) {
                warn!(
                    target: "clipboard_sync",
                    ulid,
                    error = %error,
                    "clipboard V2 cursor checkpoint failed"
                );
                return false;
            }
        }
        *cursor = Some(ulid.to_owned());
        true
    }

    /// Drain serialized V2 envelopes through bounded admission, source-session
    /// replay tracking, and the existing text-only materialization lane.
    fn drain_clipboard_envelopes(
        &self,
        persist: &mut Persist,
        cursor: &mut Option<String>,
        checkpoint: Option<&Path>,
        ledger: &mut ClipboardEnvelopeV2Ledger,
        consent_ledger: &ClipboardSessionConsentLedger,
        now_ms: u64,
    ) -> usize {
        persist.reopen_if_index_changed();
        let messages = match persist.list_since(CLIPBOARD_ENVELOPE_V2_TOPIC, cursor.as_deref()) {
            Ok(messages) => messages,
            Err(error) => {
                debug!(
                    target: "clipboard_sync",
                    error = %error,
                    "clipboard V2 envelope drain failed"
                );
                return 0;
            }
        };
        let mut materialized = 0;
        for message in messages {
            let body = message.body.as_deref().unwrap_or("");
            let envelope = match ledger.admit(body.as_bytes(), now_ms) {
                Ok(envelope) => envelope,
                Err(error) => {
                    warn!(
                        target: "clipboard_sync",
                        ulid = %message.ulid,
                        error = %error,
                        "clipboard V2 envelope rejected"
                    );
                    let _ = Self::acknowledge_v2(cursor, checkpoint, &message.ulid);
                    continue;
                }
            };
            if let Err(error) = consent_ledger.authorize_envelope(&envelope, now_ms) {
                warn!(
                    target: "clipboard_sync",
                    ulid = %message.ulid,
                    source_node = %envelope.source_node,
                    source_seat = %envelope.source_seat,
                    source_session = %envelope.source_session,
                    error = %error,
                    "clipboard V2 envelope withheld pending explicit fresh consent"
                );
                // Consent is an authenticated control lane. Leave this
                // envelope unacknowledged and do not advance replay state so a
                // later admitted update can authorize the same envelope.
                continue;
            }
            let fold = match fold_clipboard_envelope_v2(envelope.clone()) {
                Ok(fold) => fold,
                Err(error) => {
                    warn!(
                        target: "clipboard_sync",
                        ulid = %message.ulid,
                        source_session = %envelope.source_session,
                        error = %error,
                        "clipboard V2 envelope has no supported materialization"
                    );
                    // The envelope passed intrinsic/identity/expiry/replay
                    // admission. Record its sequence even though this worker
                    // cannot currently preserve its representation.
                    ledger.record(&envelope);
                    let _ = Self::acknowledge_v2(cursor, checkpoint, &message.ulid);
                    continue;
                }
            };
            if let Err(error) = self.materialize_v2_text(persist, &fold) {
                warn!(
                    target: "clipboard_sync",
                    ulid = %message.ulid,
                    source_session = %envelope.source_session,
                    error = %error,
                    "clipboard V2 materialization deferred"
                );
                continue;
            }
            ledger.record(&envelope);
            if Self::acknowledge_v2(cursor, checkpoint, &message.ulid) {
                materialized += 1;
            }
        }
        materialized
    }

    /// Acknowledge one terminal collaboration-envelope result without storing
    /// any clipboard body in the cursor file.
    fn acknowledge_collab_v2(
        cursor: &mut Option<String>,
        checkpoint: Option<&Path>,
        ulid: &str,
    ) -> bool {
        if let Some(path) = checkpoint {
            if let Err(error) = write_collab_v2_cursor(path, ulid) {
                warn!(
                    target: "clipboard_sync",
                    ulid,
                    error = %error,
                    "collaboration clipboard V2 cursor checkpoint failed"
                );
                return false;
            }
        }
        *cursor = Some(ulid.to_owned());
        true
    }

    /// Drain locally authored canonical rich envelopes into target-specific,
    /// enrollment-key-bound mesh frames. Every terminal refusal is typed and
    /// acknowledged so an unavailable or hostile peer cannot pin the lane.
    fn drain_mesh_send_requests(
        &self,
        persist: &mut Persist,
        cursor: &mut Option<String>,
        checkpoint: &Path,
        now_ms: u64,
    ) -> usize {
        persist.reopen_if_index_changed();
        let messages = match persist.list_since_limit(
            mesh::MESH_SEND_TOPIC,
            cursor.as_deref(),
            mesh::MAX_MESH_FRAMES_PER_TICK,
        ) {
            Ok(messages) => messages,
            Err(error) => {
                debug!(target: "clipboard_sync", %error, "clipboard mesh send drain failed");
                return 0;
            }
        };
        let mut sent = 0;
        for message in messages {
            let body = message.body.as_deref().unwrap_or("");
            let result = mesh::send_envelope(
                persist,
                self.mesh_peer_directory.as_ref(),
                &self.target_node,
                body.as_bytes(),
                now_ms,
            );
            if let Err(reason) = result {
                let (source_peer, target_peer) =
                    CollabClipboardEnvelopeV2::from_json_bytes(body.as_bytes())
                        .map(|envelope| {
                            (
                                envelope.source.node.to_string(),
                                envelope.target.node.to_string(),
                            )
                        })
                        .unwrap_or_default();
                mesh::publish_result(
                    persist,
                    &mesh::ClipboardMeshResultV1::Refused {
                        source_peer,
                        target_peer,
                        reason,
                    },
                );
            } else {
                sent += 1;
            }
            if let Err(error) =
                mesh::write_mesh_cursor(checkpoint, mesh::MESH_SEND_TOPIC, &message.ulid)
            {
                warn!(target: "clipboard_sync", %error, "clipboard mesh send cursor checkpoint failed");
                break;
            }
            *cursor = Some(message.ulid);
        }
        sent
    }

    /// Drain only this node's target-specific authenticated frame lane and
    /// forward admitted canonical envelopes to the existing collaboration
    /// authority. Replay state is payload-free and expiry-cleaned each tick.
    fn drain_mesh_receive_frames(
        &self,
        persist: &mut Persist,
        cursor: &mut Option<String>,
        checkpoint: &Path,
        ledger: &mut mesh::ClipboardMeshReplayLedger,
        now_ms: u64,
    ) -> usize {
        persist.reopen_if_index_changed();
        ledger.cleanup(now_ms);
        let topic = mesh::mesh_frame_topic(&self.target_node);
        let messages = match persist.list_since_limit(
            &topic,
            cursor.as_deref(),
            mesh::MAX_MESH_FRAMES_PER_TICK,
        ) {
            Ok(messages) => messages,
            Err(error) => {
                debug!(target: "clipboard_sync", %error, "clipboard mesh receive drain failed");
                return 0;
            }
        };
        let mut admitted = 0;
        for message in messages {
            let body = message.body.as_deref().unwrap_or("");
            match mesh::receive_frame(
                persist,
                self.mesh_peer_directory.as_ref(),
                &self.target_node,
                body.as_bytes(),
                ledger,
                now_ms,
            ) {
                Ok(result) => {
                    mesh::publish_result(persist, &result);
                    admitted += 1;
                }
                Err(reason) => {
                    let (source_peer, target_peer) =
                        mesh::ClipboardMeshFrameV1::from_json_bytes(body.as_bytes())
                            .map(|frame| (frame.source_peer, frame.target_peer))
                            .unwrap_or_default();
                    mesh::publish_result(
                        persist,
                        &mesh::ClipboardMeshResultV1::Refused {
                            source_peer,
                            target_peer,
                            reason,
                        },
                    );
                }
            }
            if let Err(error) = mesh::write_mesh_cursor(checkpoint, &topic, &message.ulid) {
                warn!(target: "clipboard_sync", %error, "clipboard mesh receive cursor checkpoint failed");
                break;
            }
            *cursor = Some(message.ulid);
        }
        admitted
    }

    /// Drain signed collaboration clipboard envelopes through bounded decode,
    /// exact-target, fresh-consent, replay, expiry, and echo admission. Only a
    /// sole inline `text/plain` offer can reach the existing seat handoff.
    fn drain_collab_clipboard_envelopes(
        &self,
        persist: &mut Persist,
        cursor: &mut Option<String>,
        checkpoint: Option<&Path>,
        ledger: &mut CollabClipboardEnvelopeV2Ledger,
        consent_ledger: &ClipboardSessionConsentLedger,
        now_ms: u64,
    ) -> usize {
        persist.reopen_if_index_changed();
        let messages =
            match persist.list_since(COLLAB_CLIPBOARD_ENVELOPE_V2_TOPIC, cursor.as_deref()) {
                Ok(messages) => messages,
                Err(error) => {
                    debug!(
                        target: "clipboard_sync",
                        error = %error,
                        "collaboration clipboard V2 drain failed"
                    );
                    return 0;
                }
            };
        let mut materialized = 0;
        for message in messages {
            let body = message.body.as_deref().unwrap_or("");
            let envelope = match ledger.admit(body.as_bytes(), now_ms) {
                Ok(envelope) => envelope,
                Err(error) => {
                    warn!(
                        target: "clipboard_sync",
                        ulid = %message.ulid,
                        error = %error,
                        "collaboration clipboard V2 envelope rejected"
                    );
                    if !Self::acknowledge_collab_v2(cursor, checkpoint, &message.ulid) {
                        break;
                    }
                    continue;
                }
            };
            if !self.collab_target_matches(&envelope) {
                warn!(
                    target: "clipboard_sync",
                    ulid = %message.ulid,
                    target_node = %envelope.target.node,
                    target_seat = %envelope.target.seat,
                    "collaboration clipboard V2 envelope targets another seat"
                );
                if !Self::acknowledge_collab_v2(cursor, checkpoint, &message.ulid) {
                    break;
                }
                continue;
            }
            if let Err(error) = consent_ledger.authorize_collab_envelope(&envelope, now_ms) {
                warn!(
                    target: "clipboard_sync",
                    ulid = %message.ulid,
                    source_node = %envelope.source.node,
                    source_seat = %envelope.source.seat,
                    source_session = %envelope.session,
                    error = %error,
                    "collaboration clipboard V2 withheld pending explicit fresh consent"
                );
                // Preserve ordering and retry eligibility: no later row may
                // advance the cursor past a consent-withheld envelope.
                break;
            }
            let fold = match fold_collab_clipboard_envelope_v2(envelope.clone()) {
                Ok(fold) => fold,
                Err(error) => {
                    warn!(
                        target: "clipboard_sync",
                        ulid = %message.ulid,
                        source_session = %envelope.session,
                        error = %error,
                        "collaboration clipboard V2 has no supported materialization"
                    );
                    // Files references, rich MIME, and explicit unavailable or
                    // unsupported states are terminal for this text-only
                    // adapter. Record only the sequence to prevent replay.
                    ledger.record(&envelope);
                    if !Self::acknowledge_collab_v2(cursor, checkpoint, &message.ulid) {
                        break;
                    }
                    continue;
                }
            };
            if let Err(error) = self.materialize_collab_v2_text(persist, &fold) {
                warn!(
                    target: "clipboard_sync",
                    ulid = %message.ulid,
                    source_session = %envelope.session,
                    error = %error,
                    "collaboration clipboard V2 materialization deferred"
                );
                // A transient handoff failure must not let a later row leap
                // over this envelope or advance its replay high-water.
                break;
            }
            ledger.record(&envelope);
            if Self::acknowledge_collab_v2(cursor, checkpoint, &message.ulid) {
                materialized += 1;
            } else {
                break;
            }
        }
        materialized
    }

    /// Drain new canonical `event/clipboard/clip` messages since `cursor`.
    fn drain_clip_events(
        &self,
        persist: &mut Persist,
        cursor: &mut Option<String>,
        checkpoint: Option<&Path>,
    ) -> usize {
        persist.reopen_if_index_changed();
        let msgs = match persist.list_since(CLIP_TOPIC, cursor.as_deref()) {
            Ok(msgs) => msgs,
            Err(e) => {
                debug!(target: "clipboard_sync", error = %e, "clipboard event drain failed");
                return 0;
            }
        };
        let mut applied = 0;
        for msg in msgs {
            let body = msg.body.as_deref().unwrap_or("");
            let clip = match parse_clip_event_body(body) {
                Ok(clip) => clip,
                Err(e) => {
                    warn!(target: "clipboard_sync", ulid = %msg.ulid, error = %e, "bad clipboard event body");
                    if let Some(path) = checkpoint {
                        if let Err(checkpoint_error) = write_cursor(path, &msg.ulid) {
                            warn!(target: "clipboard_sync", ulid = %msg.ulid, error = %checkpoint_error, "clipboard cursor checkpoint failed");
                            continue;
                        }
                    }
                    *cursor = Some(msg.ulid.clone());
                    continue;
                }
            };
            // VNC guest copies are canonical history events first, then become
            // signed target-seat mutations. Do the conversion before the
            // cursor can acknowledge the source event so a publish failure is
            // retryable rather than silently losing the guest copy.
            match self.publish_vnc_action(persist, &clip) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(error) => {
                    warn!(target: "clipboard_sync", ulid = %msg.ulid, %error, "VNC clipboard action publish deferred");
                    continue;
                }
            }
            match self.handle_clip_event(&clip) {
                Ok(true) => {
                    if let Some(path) = checkpoint {
                        if let Err(checkpoint_error) = write_cursor(path, &msg.ulid) {
                            warn!(target: "clipboard_sync", ulid = %msg.ulid, error = %checkpoint_error, "clipboard cursor checkpoint failed; event will be replayed");
                            continue;
                        }
                    }
                    *cursor = Some(msg.ulid.clone());
                    applied += 1;
                    debug!(
                        target: "clipboard_sync",
                        source = %clip.source,
                        "folded clipboard event ({} bytes)",
                        clip.text.len()
                    );
                }
                Ok(false) => {
                    // A non-applied valid event is either the O2 debounce or a
                    // non-writable shared root. Only the former is safe to
                    // acknowledge; handle_clip_event returns false for both,
                    // so leave the cursor unchanged and let the next tick
                    // retry until the shared history is writable.
                    if self.share_writable() {
                        if let Some(path) = checkpoint {
                            if let Err(checkpoint_error) = write_cursor(path, &msg.ulid) {
                                warn!(target: "clipboard_sync", ulid = %msg.ulid, error = %checkpoint_error, "clipboard cursor checkpoint failed");
                                continue;
                            }
                        }
                        *cursor = Some(msg.ulid.clone());
                    }
                }
                Err(e) => {
                    warn!(target: "clipboard_sync", ulid = %msg.ulid, "history write failed: {e}");
                }
            }
        }
        applied
    }
}

#[async_trait::async_trait]
impl Worker for ClipboardSyncWorker {
    fn name(&self) -> &'static str {
        "clipboard_sync"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self.bus_root() else {
            debug!("clipboard_sync: no bus root; worker idle");
            return Ok(());
        };
        let mut persist = match Persist::open(bus_root.clone()) {
            Ok(persist) => persist,
            Err(e) => {
                warn!(target: "clipboard_sync", error = %e, "bus open failed; worker idle");
                return Ok(());
            }
        };
        // Existing retained clipboard events may predate this daemon instance and
        // could resurrect a user-deleted/cleared history row. Start at the tail and
        // consume newly published lane events from here.
        let checkpoint = cursor_path(&bus_root);
        let mut cursor = read_cursor(&checkpoint);
        if cursor.is_none() {
            // First boot is intentionally forward-only: retained pre-daemon
            // events must not resurrect a user's deleted/cleared history.
            cursor = persist.latest_ulid(CLIP_TOPIC).ok().flatten();
            if let Some(ulid) = cursor.as_deref() {
                if let Err(e) = write_cursor(&checkpoint, ulid) {
                    warn!(target: "clipboard_sync", error = %e, "initial clipboard cursor checkpoint failed");
                }
            }
        }
        let consent_checkpoint = consent_cursor_path(&bus_root);
        let mut consent_cursor = read_consent_cursor(&consent_checkpoint);
        if consent_cursor.is_none() {
            // Consent is session-scoped and must not resurrect across a daemon
            // start. Seed the cursor past retained controls; only a control
            // published after this worker starts can establish fresh in-memory
            // consent for this process.
            consent_cursor = persist
                .latest_ulid(CLIPBOARD_SESSION_CONSENT_TOPIC)
                .ok()
                .flatten();
            if let Some(ulid) = consent_cursor.as_deref() {
                if let Err(error) = write_consent_cursor(&consent_checkpoint, ulid) {
                    warn!(target: "clipboard_sync", error = %error, "initial clipboard consent cursor checkpoint failed");
                }
            }
        }
        let v2_checkpoint = v2_cursor_path(&bus_root);
        let mut v2_cursor = read_v2_cursor(&v2_checkpoint);
        if v2_cursor.is_none() {
            // V2 follows the same forward-only first-boot rule as the legacy
            // lane. Retained V2 metadata still seeds replay protection, but it
            // is never materialized merely because this worker started.
            v2_cursor = persist
                .latest_ulid(CLIPBOARD_ENVELOPE_V2_TOPIC)
                .ok()
                .flatten();
            if let Some(ulid) = v2_cursor.as_deref() {
                if let Err(error) = write_v2_cursor(&v2_checkpoint, ulid) {
                    warn!(target: "clipboard_sync", error = %error, "initial clipboard V2 cursor checkpoint failed");
                }
            }
        }
        let mut v2_ledger = ClipboardEnvelopeV2Ledger::default();
        v2_ledger.seed_from_retained(&persist);
        let collab_v2_checkpoint = collab_v2_cursor_path(&bus_root);
        let mut collab_v2_cursor = read_collab_v2_cursor(&collab_v2_checkpoint);
        if collab_v2_cursor.is_none() {
            // First boot is forward-only, matching both existing clipboard
            // lanes. Retained metadata still seeds replay protection below.
            collab_v2_cursor = persist
                .latest_ulid(COLLAB_CLIPBOARD_ENVELOPE_V2_TOPIC)
                .ok()
                .flatten();
            if let Some(ulid) = collab_v2_cursor.as_deref() {
                if let Err(error) = write_collab_v2_cursor(&collab_v2_checkpoint, ulid) {
                    warn!(target: "clipboard_sync", error = %error, "initial collaboration clipboard V2 cursor checkpoint failed");
                }
            }
        }
        let mut collab_v2_ledger = CollabClipboardEnvelopeV2Ledger::default();
        collab_v2_ledger.seed_from_retained(&persist);
        let mesh_send_checkpoint = bus_root.join(MESH_SEND_CURSOR_FILE_NAME);
        let mut mesh_send_cursor =
            mesh::read_mesh_cursor(&mesh_send_checkpoint, mesh::MESH_SEND_TOPIC);
        if mesh_send_cursor.is_none() {
            // Local send requests are session actions, never durable desired
            // state. A daemon restart starts at the tail rather than reviving
            // an old clipboard generation.
            mesh_send_cursor = persist.latest_ulid(mesh::MESH_SEND_TOPIC).ok().flatten();
            if let Some(ulid) = mesh_send_cursor.as_deref() {
                if let Err(error) =
                    mesh::write_mesh_cursor(&mesh_send_checkpoint, mesh::MESH_SEND_TOPIC, ulid)
                {
                    warn!(target: "clipboard_sync", %error, "initial clipboard mesh send cursor checkpoint failed");
                }
            }
        }
        let mesh_receive_topic = mesh::mesh_frame_topic(&self.target_node);
        let mesh_receive_checkpoint = bus_root.join(MESH_RECEIVE_CURSOR_FILE_NAME);
        let mut mesh_receive_cursor =
            mesh::read_mesh_cursor(&mesh_receive_checkpoint, &mesh_receive_topic);
        // A receiver must inspect retained frames after first start or cursor
        // loss. Rebuild the payload-free high-water marks from the canonical
        // lane first, then signature/expiry admission safely rejects old or
        // already-forwarded entries without dropping a still-fresh transfer.
        let mut mesh_replay_ledger = mesh::ClipboardMeshReplayLedger::default();
        let startup_now_ms = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0);
        mesh_replay_ledger.seed_from_retained(&persist, startup_now_ms);
        // Consent starts disabled on every daemon/session start. Fresh signed
        // controls are drained before either V2 envelope lane on each tick.
        let mut v2_consent_ledger = ClipboardSessionConsentLedger::default();
        // Do not resurrect a retained clipboard at daemon start. Only a head
        // that changes while this worker is alive is a fresh mesh delivery.
        let mut observed_history_head = read_history(&history_path(&self.workgroup_root))
            .entries
            .first()
            .map(|entry| ClipEventBody {
                id: entry.id.clone(),
                text: entry.text.clone(),
                source: entry.source.clone(),
                time: entry.time.clone(),
            });
        info!(target: "clipboard_sync", "watching canonical clipboard bus lane");
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let now_ms = u64::try_from(chrono::Utc::now().timestamp_millis());
                    if let Ok(now_ms) = now_ms {
                        self.drain_clipboard_consents(
                            &mut persist,
                            &mut consent_cursor,
                            Some(&consent_checkpoint),
                            &mut v2_consent_ledger,
                            now_ms,
                        );
                        self.drain_clipboard_envelopes(
                            &mut persist,
                            &mut v2_cursor,
                            Some(&v2_checkpoint),
                            &mut v2_ledger,
                            &v2_consent_ledger,
                            now_ms,
                        );
                        self.drain_mesh_send_requests(
                            &mut persist,
                            &mut mesh_send_cursor,
                            &mesh_send_checkpoint,
                            now_ms,
                        );
                        self.drain_mesh_receive_frames(
                            &mut persist,
                            &mut mesh_receive_cursor,
                            &mesh_receive_checkpoint,
                            &mut mesh_replay_ledger,
                            now_ms,
                        );
                        self.drain_collab_clipboard_envelopes(
                            &mut persist,
                            &mut collab_v2_cursor,
                            Some(&collab_v2_checkpoint),
                            &mut collab_v2_ledger,
                            &v2_consent_ledger,
                            now_ms,
                        );
                    } else {
                        warn!(target: "clipboard_sync", "system clock is before the Unix epoch; clipboard V2 admission deferred");
                    }
                    self.drain_clip_events(&mut persist, &mut cursor, Some(&checkpoint));
                    if let Err(error) = self.materialize_replicated_head(
                        &mut persist,
                        &mut observed_history_head,
                    ) {
                        warn!(target: "clipboard_sync", %error, "replicated clipboard materialization failed");
                    }
                }
                () = shutdown.wait() => return Ok(()),
            }
        }
    }
}

fn local_node_id() -> String {
    let hostname = std::fs::read_to_string("/etc/hostname")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "unknown".to_owned());
    hostname.trim().to_owned()
}

fn local_target_seat() -> String {
    format!("seat:{}", local_node_id())
}

/// Build the supervisor-ready worker (call site in `run_serve`).
#[must_use]
pub fn build(workgroup_root: PathBuf) -> ClipboardSyncWorker {
    ClipboardSyncWorker::new(workgroup_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::action_auth::{authorize_test_body, ActionAuthorizer, MutationContext};
    use ed25519_dalek::SigningKey;
    use mackes_mesh_types::cloud::CloudArmSigner;
    use mde_bus::hooks::config::Priority;
    use mde_collab_types::{
        ClipboardClipId as CollabClipboardClipId,
        ClipboardMimeOfferV2 as CollabClipboardMimeOfferV2,
        ClipboardNodeId as CollabClipboardNodeId, ClipboardSeatId as CollabClipboardSeatId,
        ClipboardSessionId as CollabClipboardSessionId,
        ClipboardSourceV2 as CollabClipboardSourceV2, ClipboardTargetV2 as CollabClipboardTargetV2,
        ClipboardUnavailableReason as CollabClipboardUnavailableReason,
        ClipboardUnsupportedReason as CollabClipboardUnsupportedReason, FileRefId,
    };

    const CONSENT_AUTH_KEY: &[u8] = b"clipboard-consent-command-test-key";
    const CONSENT_AUTH_NOW: i64 = 1_700_000_000_000;

    fn entry(text: &str, pinned: bool) -> ClipEntry {
        ClipEntry {
            id: clip_id(text),
            text: text.to_string(),
            source: "n".into(),
            time: "2026-06-21T00:00:00+00:00".into(),
            pinned,
        }
    }

    fn v2_inline(sequence: u64, offers: &[&str]) -> ClipboardEnvelopeV2 {
        ClipboardEnvelopeV2::new_inline_text(
            "node-a",
            "seat-a",
            "session-a",
            sequence,
            1_700_000_000_000,
            offers.iter().map(|offer| (*offer).to_owned()).collect(),
            "preview",
            VdiClipboardText::new("hello").expect("bounded V2 fixture text"),
            1_700_000_060_000,
        )
        .expect("valid V2 inline fixture")
    }

    fn collab_v2_signing_key() -> SigningKey {
        SigningKey::from_bytes(&[7; 32])
    }

    fn collab_v2_with_offers(
        sequence: u64,
        offers: Vec<CollabClipboardMimeOfferV2>,
    ) -> CollabClipboardEnvelopeV2 {
        CollabClipboardEnvelopeV2::new(
            CollabClipboardClipId::from_uuid(uuid::Uuid::from_u128(0x1000 + sequence as u128)),
            CollabClipboardSourceV2::new(
                CollabClipboardNodeId::new("node-a").expect("source node"),
                CollabClipboardSeatId::new("seat-a").expect("source seat"),
            ),
            CollabClipboardTargetV2::new(
                CollabClipboardNodeId::new("eagle").expect("target node"),
                CollabClipboardSeatId::new("eagle").expect("target seat"),
            ),
            CollabClipboardSessionId::from_uuid(uuid::Uuid::from_u128(0x2000)),
            sequence,
            CONSENT_AUTH_NOW as u64,
            CONSENT_AUTH_NOW as u64 + 60_000,
            offers,
        )
        .expect("valid collaboration V2 fixture")
        .signed(&collab_v2_signing_key())
    }

    fn collab_v2_inline(sequence: u64, text: &str) -> CollabClipboardEnvelopeV2 {
        collab_v2_with_offers(
            sequence,
            vec![
                CollabClipboardMimeOfferV2::inline_text(CollabClipboardMimeKind::TextPlain, text)
                    .expect("valid collaboration inline text"),
            ],
        )
    }

    fn v2_consent(
        source_node: &str,
        source_seat: &str,
        source_session: &str,
        enabled: bool,
        updated_at_ms: u64,
        expires_at_ms: u64,
    ) -> ClipboardSessionConsentV1 {
        ClipboardSessionConsentV1::new(
            source_node,
            source_seat,
            source_session,
            enabled,
            updated_at_ms,
            expires_at_ms,
        )
        .expect("valid V2 consent fixture")
    }

    fn consent_unsigned_body(consent: &ClipboardSessionConsentV1) -> String {
        serde_json::json!({
            "schema_version": ACTION_SCHEMA_VERSION,
            "consent": consent,
        })
        .to_string()
    }

    fn signed_consent_body(
        consent: &ClipboardSessionConsentV1,
        nonce: &str,
        node: &str,
        target: &str,
    ) -> String {
        authorize_test_body(
            CONSENT_AUTH_KEY,
            &consent_unsigned_body(consent),
            MutationContext {
                verb: CLIPBOARD_SESSION_CONSENT_AUTH_VERB,
                node,
                target,
            },
            nonce,
            CONSENT_AUTH_NOW + 30_000,
        )
    }

    fn signed_consent(consent: &ClipboardSessionConsentV1, nonce: &str) -> String {
        let target = clipboard_session_consent_auth_target(consent);
        signed_consent_body(consent, nonce, &consent.source_node, &target)
    }

    fn consent_authorizer(root: &Path) -> Arc<ActionAuthorizer> {
        Arc::new(ActionAuthorizer::for_test(
            CONSENT_AUTH_KEY,
            root.join("auth"),
            CONSENT_AUTH_NOW,
        ))
    }

    #[test]
    fn worker_name_is_stable() {
        let w = ClipboardSyncWorker::new(PathBuf::from("/tmp"));
        assert_eq!(w.name(), "clipboard_sync");
    }

    #[test]
    fn v2_boundary_admits_plain_text_and_preserves_source_metadata() {
        let envelope = v2_inline(1, &["text/plain"]);
        let body = serde_json::to_vec(&envelope).expect("encode V2 envelope");
        let admitted = admit_serialized_clipboard_envelope_v2(&body, None, 1_700_000_000_001)
            .expect("admit V2 envelope");
        let fold = fold_clipboard_envelope_v2(admitted).expect("fold plain text");

        assert_eq!(fold.text.as_str(), "hello");
        assert_eq!(fold.source, "v2:node-a:seat-a:session-a");
        assert_eq!(fold.time, "2023-11-14T22:13:20+00:00");
        assert_eq!(fold.envelope.sequence, 1);
    }

    #[test]
    fn v2_boundary_rejects_replay_identity_and_expiry() {
        let previous = v2_inline(4, &["text/plain"]);
        let previous_body = serde_json::to_vec(&previous).expect("encode previous V2");

        assert!(matches!(
            admit_serialized_clipboard_envelope_v2(
                &previous_body,
                Some(&previous),
                1_700_000_000_001,
            ),
            Err(ClipboardEnvelopeV2BoundaryError::Admission(
                ClipboardEnvelopeV2ValidationError::Replay {
                    previous: 4,
                    received: 4
                }
            ))
        ));

        let mut cross_source = v2_inline(5, &["text/plain"]);
        cross_source.source_seat = "seat-b".to_owned();
        let cross_source_body =
            serde_json::to_vec(&cross_source).expect("encode identity mismatch");
        assert!(matches!(
            admit_serialized_clipboard_envelope_v2(
                &cross_source_body,
                Some(&previous),
                1_700_000_000_001,
            ),
            Err(ClipboardEnvelopeV2BoundaryError::Admission(
                ClipboardEnvelopeV2ValidationError::IdentityMismatch {
                    field: "source_seat"
                }
            ))
        ));

        let expired_body =
            serde_json::to_vec(&v2_inline(5, &["text/plain"])).expect("encode expired V2");
        assert!(matches!(
            admit_serialized_clipboard_envelope_v2(&expired_body, None, 1_700_000_060_000,),
            Err(ClipboardEnvelopeV2BoundaryError::Admission(
                ClipboardEnvelopeV2ValidationError::Expired { .. }
            ))
        ));
    }

    #[test]
    fn v2_boundary_never_downgrades_files_or_rich_mime_to_text() {
        let files = ClipboardEnvelopeV2::new_files(
            "node-a",
            "seat-a",
            "session-files",
            1,
            1_700_000_000_000,
            vec![
                "image/png".to_owned(),
                "application/octet-stream".to_owned(),
            ],
            "image",
            ClipboardEnvelopeV2::content_hash_for(b"png bytes"),
            9,
            "files:v2:payload-1",
            1_700_000_060_000,
        )
        .expect("valid Files envelope");
        let admitted_files = admit_serialized_clipboard_envelope_v2(
            &serde_json::to_vec(&files).expect("encode Files envelope"),
            None,
            1_700_000_000_001,
        )
        .expect("admit Files metadata");
        assert!(matches!(
            fold_clipboard_envelope_v2(admitted_files),
            Err(ClipboardEnvelopeV2BoundaryError::UnsupportedPayload(
                ClipboardEnvelopeV2UnsupportedPayload::FilesReference
            ))
        ));

        let rich = v2_inline(1, &["text/html", "text/plain"]);
        assert!(matches!(
            fold_clipboard_envelope_v2(rich),
            Err(ClipboardEnvelopeV2BoundaryError::UnsupportedPayload(
                ClipboardEnvelopeV2UnsupportedPayload::RichMime
            ))
        ));
    }

    #[test]
    fn v2_consent_ledger_is_default_disabled_and_binds_full_identity() {
        let envelope = v2_inline(1, &["text/plain"]);
        let mut ledger = ClipboardSessionConsentLedger::default();
        assert!(matches!(
            ledger.authorize_envelope(&envelope, 1_700_000_000_001),
            Err(ClipboardSessionConsentBoundaryError::Missing)
        ));

        let initial = v2_consent(
            "node-a",
            "seat-a",
            "session-a",
            true,
            1_700_000_000_000,
            1_700_000_060_000,
        );
        ledger
            .admit(initial.clone(), 1_700_000_000_001)
            .expect("admit enabled consent");
        assert!(ledger
            .authorize_envelope(&envelope, 1_700_000_000_001)
            .is_ok());

        let mut wrong_identity = envelope.clone();
        wrong_identity.source_seat = "seat-other".to_owned();
        assert!(matches!(
            ledger.authorize_envelope(&wrong_identity, 1_700_000_000_001),
            Err(ClipboardSessionConsentBoundaryError::Missing)
        ));

        let disabled = initial
            .update(false, 1_700_000_000_010, 1_700_000_060_010)
            .expect("construct disabling update");
        ledger
            .admit(disabled.clone(), 1_700_000_000_011)
            .expect("admit disabling update");
        assert!(matches!(
            ledger.authorize_envelope(&envelope, 1_700_000_000_011),
            Err(ClipboardSessionConsentBoundaryError::Disabled)
        ));
        assert!(matches!(
            ledger.admit(initial, 1_700_000_000_011),
            Err(ClipboardSessionConsentBoundaryError::Admission(
                ClipboardSessionConsentValidationError::StaleUpdate { .. }
            ))
        ));

        let expired = disabled
            .update(true, 1_700_000_000_020, 1_700_000_000_030)
            .expect("construct short-lived update");
        assert!(matches!(
            ledger.admit(expired, 1_700_000_000_030),
            Err(ClipboardSessionConsentBoundaryError::Admission(
                ClipboardSessionConsentValidationError::Expired { .. }
            ))
        ));
    }

    #[test]
    fn signed_consent_transport_enables_and_disables_exact_session() {
        let history_dir = tempfile::tempdir().expect("history root");
        let bus_dir = tempfile::tempdir().expect("bus root");
        let auth_dir = tempfile::tempdir().expect("auth root");
        let mut persist = Persist::open(bus_dir.path().to_path_buf()).expect("open bus");
        let worker = ClipboardSyncWorker::new(history_dir.path().to_path_buf())
            .with_bus_root(bus_dir.path().to_path_buf())
            .with_consent_authorizer(consent_authorizer(auth_dir.path()));
        let mut cursor = None;
        let checkpoint = bus_dir.path().join("consent.cursor");
        let mut ledger = ClipboardSessionConsentLedger::default();
        let enabled = v2_consent(
            "node-a",
            "seat-a",
            "session-a",
            true,
            CONSENT_AUTH_NOW as u64,
            CONSENT_AUTH_NOW as u64 + 60_000,
        );
        persist
            .write(
                CLIPBOARD_SESSION_CONSENT_TOPIC,
                Priority::Default,
                None,
                Some(&signed_consent(
                    &enabled,
                    "consent-enable-000000000000000000000000",
                )),
            )
            .expect("publish signed enable");

        assert_eq!(
            worker.drain_clipboard_consents(
                &mut persist,
                &mut cursor,
                Some(&checkpoint),
                &mut ledger,
                CONSENT_AUTH_NOW as u64 + 1,
            ),
            1
        );
        assert!(ledger
            .authorize_envelope(&v2_inline(1, &["text/plain"]), CONSENT_AUTH_NOW as u64 + 1)
            .is_ok());
        assert_eq!(read_consent_cursor(&checkpoint), cursor);

        let disabled = enabled
            .update(
                false,
                CONSENT_AUTH_NOW as u64 + 10,
                CONSENT_AUTH_NOW as u64 + 60_010,
            )
            .expect("newer disable");
        persist
            .write(
                CLIPBOARD_SESSION_CONSENT_TOPIC,
                Priority::Default,
                None,
                Some(&signed_consent(
                    &disabled,
                    "consent-disable-000000000000000000000000",
                )),
            )
            .expect("publish signed disable");
        assert_eq!(
            worker.drain_clipboard_consents(
                &mut persist,
                &mut cursor,
                Some(&checkpoint),
                &mut ledger,
                CONSENT_AUTH_NOW as u64 + 11,
            ),
            1
        );
        assert!(matches!(
            ledger.authorize_envelope(&v2_inline(2, &["text/plain"]), CONSENT_AUTH_NOW as u64 + 11),
            Err(ClipboardSessionConsentBoundaryError::Disabled)
        ));
    }

    #[test]
    fn consent_transport_rejects_wrong_target_without_enabling() {
        let history_dir = tempfile::tempdir().expect("history root");
        let bus_dir = tempfile::tempdir().expect("bus root");
        let auth_dir = tempfile::tempdir().expect("auth root");
        let mut persist = Persist::open(bus_dir.path().to_path_buf()).expect("open bus");
        let worker = ClipboardSyncWorker::new(history_dir.path().to_path_buf())
            .with_bus_root(bus_dir.path().to_path_buf())
            .with_consent_authorizer(consent_authorizer(auth_dir.path()));
        let consent = v2_consent(
            "node-a",
            "seat-a",
            "session-a",
            true,
            CONSENT_AUTH_NOW as u64,
            CONSENT_AUTH_NOW as u64 + 60_000,
        );
        let wrong_target = format!(
            "source:{}:seat:{}:session:other",
            consent.source_node, consent.source_seat
        );
        persist
            .write(
                CLIPBOARD_SESSION_CONSENT_TOPIC,
                Priority::Default,
                None,
                Some(&signed_consent_body(
                    &consent,
                    "consent-wrong-target-000000000000000",
                    &consent.source_node,
                    &wrong_target,
                )),
            )
            .expect("publish wrong-target control");
        let mut cursor = None;
        let mut ledger = ClipboardSessionConsentLedger::default();
        assert_eq!(
            worker.drain_clipboard_consents(
                &mut persist,
                &mut cursor,
                None,
                &mut ledger,
                CONSENT_AUTH_NOW as u64 + 1,
            ),
            0
        );
        assert!(matches!(
            ledger.authorize_envelope(&v2_inline(1, &["text/plain"]), CONSENT_AUTH_NOW as u64 + 1),
            Err(ClipboardSessionConsentBoundaryError::Missing)
        ));
    }

    #[test]
    fn consent_transport_rejects_malformed_unsigned_replay_and_expired_controls() {
        let history_dir = tempfile::tempdir().expect("history root");
        let bus_dir = tempfile::tempdir().expect("bus root");
        let auth_dir = tempfile::tempdir().expect("auth root");
        let mut persist = Persist::open(bus_dir.path().to_path_buf()).expect("open bus");
        let worker = ClipboardSyncWorker::new(history_dir.path().to_path_buf())
            .with_bus_root(bus_dir.path().to_path_buf())
            .with_consent_authorizer(consent_authorizer(auth_dir.path()));
        let consent = v2_consent(
            "node-a",
            "seat-a",
            "session-a",
            true,
            CONSENT_AUTH_NOW as u64,
            CONSENT_AUTH_NOW as u64 + 60_000,
        );
        let malformed = serde_json::json!({
            "schema_version": ACTION_SCHEMA_VERSION,
            "consent": consent,
            "payload": "clipboard bytes must not be accepted",
        })
        .to_string();
        assert!(ClipboardSessionConsentCommandV1::from_json(&malformed).is_err());
        persist
            .write(
                CLIPBOARD_SESSION_CONSENT_TOPIC,
                Priority::Default,
                None,
                Some(&malformed),
            )
            .expect("publish malformed control");
        persist
            .write(
                CLIPBOARD_SESSION_CONSENT_TOPIC,
                Priority::Default,
                None,
                Some(&consent_unsigned_body(&consent)),
            )
            .expect("publish unsigned control");

        let valid = signed_consent(&consent, "consent-replay-000000000000000000000000");
        persist
            .write(
                CLIPBOARD_SESSION_CONSENT_TOPIC,
                Priority::Default,
                None,
                Some(&valid),
            )
            .expect("publish signed control");
        persist
            .write(
                CLIPBOARD_SESSION_CONSENT_TOPIC,
                Priority::Default,
                None,
                Some(&valid),
            )
            .expect("publish replayed signed control");

        let expired = v2_consent(
            "node-a",
            "seat-a",
            "session-expired",
            true,
            CONSENT_AUTH_NOW as u64,
            CONSENT_AUTH_NOW as u64 + 1,
        );
        persist
            .write(
                CLIPBOARD_SESSION_CONSENT_TOPIC,
                Priority::Default,
                None,
                Some(&signed_consent(
                    &expired,
                    "consent-expired-000000000000000000000",
                )),
            )
            .expect("publish expired control");

        let mut cursor = None;
        let mut ledger = ClipboardSessionConsentLedger::default();
        assert_eq!(
            worker.drain_clipboard_consents(
                &mut persist,
                &mut cursor,
                None,
                &mut ledger,
                CONSENT_AUTH_NOW as u64 + 2,
            ),
            1,
            "only the first signed, fresh control may enter the ledger"
        );
        assert!(cursor.is_some());
        assert!(persist
            .list_since(CLIPBOARD_SESSION_CONSENT_TOPIC, cursor.as_deref())
            .expect("consent cursor read")
            .is_empty());
        assert!(ledger
            .authorize_envelope(&v2_inline(1, &["text/plain"]), CONSENT_AUTH_NOW as u64 + 2)
            .is_ok());
    }

    #[test]
    fn v2_worker_requires_fresh_enabled_consent_and_preserves_replay_cursor() {
        let history_dir = tempfile::tempdir().expect("history root");
        let bus_dir = tempfile::tempdir().expect("bus root");
        let mut persist = Persist::open(bus_dir.path().to_path_buf()).expect("open bus");
        let envelope = v2_inline(1, &["text/plain"]);
        let body = serde_json::to_string(&envelope).expect("encode V2 envelope");
        persist
            .write(
                CLIPBOARD_ENVELOPE_V2_TOPIC,
                Priority::Default,
                None,
                Some(&body),
            )
            .expect("publish V2 envelope");

        let worker = ClipboardSyncWorker::new(history_dir.path().to_path_buf())
            .with_bus_root(bus_dir.path().to_path_buf())
            .with_target_seat("seat:eagle");
        let mut cursor = None;
        let mut ledger = ClipboardEnvelopeV2Ledger::default();
        let consent_ledger = ClipboardSessionConsentLedger::default();
        assert_eq!(
            worker.drain_clipboard_envelopes(
                &mut persist,
                &mut cursor,
                None,
                &mut ledger,
                &consent_ledger,
                1_700_000_000_001,
            ),
            0,
            "default-disabled consent must prevent materialization"
        );
        assert!(cursor.is_none(), "withheld envelope must remain retryable");
        assert!(persist
            .read_latest(CLIPBOARD_MATERIALIZATION_TOPIC)
            .expect("read withheld handoff")
            .is_none());

        let mut consent_ledger = consent_ledger;
        consent_ledger
            .admit(
                v2_consent(
                    "node-a",
                    "seat-a",
                    "session-a",
                    true,
                    1_700_000_000_000,
                    1_700_000_060_000,
                ),
                1_700_000_000_001,
            )
            .expect("admit explicit fresh consent");
        assert_eq!(
            worker.drain_clipboard_envelopes(
                &mut persist,
                &mut cursor,
                None,
                &mut ledger,
                &consent_ledger,
                1_700_000_000_001,
            ),
            1
        );
        let handoff = persist
            .read_latest(CLIPBOARD_MATERIALIZATION_TOPIC)
            .expect("read V2 handoff")
            .expect("V2 handoff exists");
        let handoff: ClipboardMaterialization =
            serde_json::from_str(handoff.body.as_deref().expect("V2 handoff body"))
                .expect("decode V2 handoff");
        assert_eq!(handoff.target_seat, "seat:eagle");
        assert_eq!(handoff.text.as_str(), "hello");
        assert_eq!(handoff.source, "v2:node-a:seat-a:session-a");
        assert!(
            read_history(&history_path(history_dir.path()))
                .entries
                .is_empty(),
            "V2 handoff must not be downgraded into the legacy text history"
        );

        persist
            .write(
                CLIPBOARD_ENVELOPE_V2_TOPIC,
                Priority::Default,
                None,
                Some(&body),
            )
            .expect("publish replay V2 envelope");
        assert_eq!(
            worker.drain_clipboard_envelopes(
                &mut persist,
                &mut cursor,
                None,
                &mut ledger,
                &consent_ledger,
                1_700_000_000_002,
            ),
            0,
            "replayed source sequence is rejected before materialization"
        );
        assert_eq!(
            persist
                .list_since(CLIPBOARD_MATERIALIZATION_TOPIC, None)
                .expect("list V2 handoffs")
                .len(),
            1
        );
    }

    #[test]
    fn collab_v2_bus_intake_requires_consent_and_never_writes_legacy_history() {
        let history_dir = tempfile::tempdir().expect("history root");
        let bus_dir = tempfile::tempdir().expect("bus root");
        let mut persist = Persist::open(bus_dir.path().to_path_buf()).expect("open bus");
        let envelope = collab_v2_inline(1, "collaboration hello");
        let body = serde_json::to_string(&envelope).expect("encode collaboration envelope");
        persist
            .write(
                COLLAB_CLIPBOARD_ENVELOPE_V2_TOPIC,
                Priority::Default,
                None,
                Some(&body),
            )
            .expect("publish collaboration envelope");

        let worker = ClipboardSyncWorker::new(history_dir.path().to_path_buf())
            .with_bus_root(bus_dir.path().to_path_buf())
            .with_target_node("eagle")
            .with_target_seat("seat:eagle");
        let checkpoint = bus_dir.path().join("collab-v2.cursor");
        let mut cursor = None;
        let mut ledger = CollabClipboardEnvelopeV2Ledger::default();
        let consent_ledger = ClipboardSessionConsentLedger::default();
        assert_eq!(
            worker.drain_collab_clipboard_envelopes(
                &mut persist,
                &mut cursor,
                Some(&checkpoint),
                &mut ledger,
                &consent_ledger,
                CONSENT_AUTH_NOW as u64 + 1,
            ),
            0
        );
        assert!(cursor.is_none(), "consent-withheld input stays retryable");
        assert!(persist
            .read_latest(CLIPBOARD_MATERIALIZATION_TOPIC)
            .expect("read withheld handoff")
            .is_none());

        let mut consent_ledger = consent_ledger;
        consent_ledger
            .admit(
                v2_consent(
                    envelope.source.node.as_str(),
                    envelope.source.seat.as_str(),
                    &envelope.session.to_string(),
                    true,
                    CONSENT_AUTH_NOW as u64,
                    CONSENT_AUTH_NOW as u64 + 60_000,
                ),
                CONSENT_AUTH_NOW as u64 + 1,
            )
            .expect("admit collaboration source consent");
        assert_eq!(
            worker.drain_collab_clipboard_envelopes(
                &mut persist,
                &mut cursor,
                Some(&checkpoint),
                &mut ledger,
                &consent_ledger,
                CONSENT_AUTH_NOW as u64 + 1,
            ),
            1
        );
        let handoff = persist
            .read_latest(CLIPBOARD_MATERIALIZATION_TOPIC)
            .expect("read collaboration handoff")
            .expect("collaboration handoff exists");
        let handoff: ClipboardMaterialization =
            serde_json::from_str(handoff.body.as_deref().expect("handoff body"))
                .expect("decode collaboration handoff");
        assert_eq!(handoff.target_seat, "seat:eagle");
        assert_eq!(handoff.text.as_str(), "collaboration hello");
        assert_eq!(
            handoff.source,
            format!("collab-v2:node-a:seat-a:{}", envelope.session)
        );
        assert!(
            read_history(&history_path(history_dir.path()))
                .entries
                .is_empty(),
            "collaboration payloads never enter legacy durable history"
        );
        let cursor_body = std::fs::read_to_string(&checkpoint).expect("read payload-free cursor");
        assert!(!cursor_body.contains("collaboration hello"));
        assert_eq!(read_collab_v2_cursor(&checkpoint), cursor);

        persist
            .write(
                COLLAB_CLIPBOARD_ENVELOPE_V2_TOPIC,
                Priority::Default,
                None,
                Some(&body),
            )
            .expect("publish collaboration replay");
        assert_eq!(
            worker.drain_collab_clipboard_envelopes(
                &mut persist,
                &mut cursor,
                Some(&checkpoint),
                &mut ledger,
                &consent_ledger,
                CONSENT_AUTH_NOW as u64 + 2,
            ),
            0,
            "source/session replay must not materialize twice"
        );
        assert_eq!(
            persist
                .list_since(CLIPBOARD_MATERIALIZATION_TOPIC, None)
                .expect("list collaboration handoffs")
                .len(),
            1
        );
    }

    #[test]
    fn collab_v2_bus_intake_fails_closed_for_files_rich_states_and_echoes() {
        let history_dir = tempfile::tempdir().expect("history root");
        let bus_dir = tempfile::tempdir().expect("bus root");
        let mut persist = Persist::open(bus_dir.path().to_path_buf()).expect("open bus");
        let files = collab_v2_with_offers(
            1,
            vec![CollabClipboardMimeOfferV2::files_reference(
                CollabClipboardMimeKind::ImagePng,
                FileRefId::from_uuid(uuid::Uuid::from_u128(0x3000)),
                9,
                "a".repeat(64),
            )
            .expect("valid Files reference")],
        );
        let unsupported = collab_v2_with_offers(
            2,
            vec![CollabClipboardMimeOfferV2::unsupported(
                CollabClipboardMimeKind::TextPlain,
                CollabClipboardUnsupportedReason::TransportUnsupported,
            )],
        );
        let unavailable = collab_v2_with_offers(
            3,
            vec![CollabClipboardMimeOfferV2::unavailable(
                CollabClipboardMimeKind::TextPlain,
                CollabClipboardUnavailableReason::ProviderOffline,
            )],
        );
        let rich = collab_v2_with_offers(
            4,
            vec![CollabClipboardMimeOfferV2::inline_text(
                CollabClipboardMimeKind::TextHtml,
                "<b>rich</b>",
            )
            .expect("valid rich inline offer")],
        );
        let mut echo = collab_v2_inline(5, "must not echo");
        echo.echo_guard.visited_nodes.push(echo.target.node.clone());
        echo.sign(&collab_v2_signing_key());

        for envelope in [&files, &unsupported, &unavailable, &rich, &echo] {
            persist
                .write(
                    COLLAB_CLIPBOARD_ENVELOPE_V2_TOPIC,
                    Priority::Default,
                    None,
                    Some(&serde_json::to_string(envelope).expect("encode rejected envelope")),
                )
                .expect("publish rejected envelope");
        }

        let worker = ClipboardSyncWorker::new(history_dir.path().to_path_buf())
            .with_target_node("eagle")
            .with_target_seat("seat:eagle");
        let mut consent_ledger = ClipboardSessionConsentLedger::default();
        consent_ledger
            .admit(
                v2_consent(
                    files.source.node.as_str(),
                    files.source.seat.as_str(),
                    &files.session.to_string(),
                    true,
                    CONSENT_AUTH_NOW as u64,
                    CONSENT_AUTH_NOW as u64 + 60_000,
                ),
                CONSENT_AUTH_NOW as u64 + 1,
            )
            .expect("admit collaboration source consent");
        let mut cursor = None;
        let mut ledger = CollabClipboardEnvelopeV2Ledger::default();
        assert_eq!(
            worker.drain_collab_clipboard_envelopes(
                &mut persist,
                &mut cursor,
                None,
                &mut ledger,
                &consent_ledger,
                CONSENT_AUTH_NOW as u64 + 1,
            ),
            0
        );
        assert!(cursor.is_some(), "all terminal refusals are acknowledged");
        assert!(persist
            .list_since(COLLAB_CLIPBOARD_ENVELOPE_V2_TOPIC, cursor.as_deref())
            .expect("read collaboration tail")
            .is_empty());
        assert!(
            persist
                .read_latest(CLIPBOARD_MATERIALIZATION_TOPIC)
                .expect("read refused handoff")
                .is_none(),
            "Files, rich, explicit states, and echoes have no materialization effect"
        );
        assert!(
            read_history(&history_path(history_dir.path()))
                .entries
                .is_empty(),
            "no refused representation reaches legacy persistence"
        );
        assert_eq!(
            ledger
                .latest_by_identity
                .get(&ClipboardSessionIdentityKey::from_collab_envelope(&files)),
            Some(&4),
            "only valid admitted metadata advances replay state; echo is rejected intrinsically"
        );
    }

    #[test]
    fn vnc_promotion_is_bounded_attributed_and_signed_for_exact_seat() {
        let key = b"clipboard-sync-vnc-action-test-key";
        let signer = CloudArmSigner::new(key.to_vec()).expect("test signer");
        let auth_root = tempfile::tempdir().expect("auth root");
        let clip = ClipEventBody::from_text(
            "guest copy",
            "vnc:serving-peer:session-1",
            "2026-07-31T12:00:00Z",
        );
        let body = ClipboardSyncWorker::signed_vnc_action(&clip, "session-1", "seat:dell", &signer)
            .expect("signed VNC action");
        let event: ClipboardEvent = serde_json::from_str(&body).expect("action event");
        assert_eq!(event.direction, ClipDirection::GuestToClient);
        assert_eq!(event.target_seat, "seat:dell");
        assert_eq!(event.source.as_deref(), Some("vnc:serving-peer:session-1"));
        assert!(event.payload.len() <= MAX_CLIP_BYTES);

        let signed_now = CloudArmedToken::parse(
            serde_json::from_str::<serde_json::Value>(&body).expect("action JSON")["armed_token"]
                .as_str()
                .expect("armed token"),
        )
        .expect("parse armed token")
        .expires_at_ms
        .saturating_sub(MAX_AUTH_TTL_MS);
        let authorizer =
            ActionAuthorizer::for_test(key, auth_root.path().to_path_buf(), signed_now);
        authorizer
            .authorize(
                &body,
                MutationContext {
                    verb: super::super::clipboard_bridge::ACTION_AUTH_VERB,
                    node: super::super::clipboard_bridge::ACTION_AUTH_NODE_SCOPE,
                    target: "session:session-1:seat:seat:dell",
                },
            )
            .expect("exact target-seat capability verifies");
        assert!(authorizer
            .authorize(
                &body,
                MutationContext {
                    verb: super::super::clipboard_bridge::ACTION_AUTH_VERB,
                    node: super::super::clipboard_bridge::ACTION_AUTH_NODE_SCOPE,
                    target: "session:session-1:seat:seat:other",
                },
            )
            .is_err());

        // Authorization is consumed before the adapter write. A second
        // publication therefore needs a fresh nonce so a failed first write
        // remains retryable; the bridge's payload echo guard handles the
        // duplicate if the first write actually succeeded.
        let retry_body =
            ClipboardSyncWorker::signed_vnc_action(&clip, "session-1", "seat:dell", &signer)
                .expect("retry action is signed");
        let retry_now_ms = CloudArmedToken::parse(
            serde_json::from_str::<serde_json::Value>(&retry_body).expect("retry action JSON")
                ["armed_token"]
                .as_str()
                .expect("retry armed token"),
        )
        .expect("parse retry armed token")
        .expires_at_ms
        .saturating_sub(MAX_AUTH_TTL_MS);
        ActionAuthorizer::for_test(key, auth_root.path().to_path_buf(), retry_now_ms)
            .authorize(
                &retry_body,
                MutationContext {
                    verb: super::super::clipboard_bridge::ACTION_AUTH_VERB,
                    node: super::super::clipboard_bridge::ACTION_AUTH_NODE_SCOPE,
                    target: "session:session-1:seat:seat:dell",
                },
            )
            .expect("retry gets a fresh capability nonce");
    }

    #[test]
    fn vnc_source_routes_only_through_matching_active_session() {
        let root = tempfile::tempdir().expect("session root");
        let requested = super::super::session_broker::open_session(
            "session-1".to_owned(),
            "serving-peer".to_owned(),
            "vm-1".to_owned(),
            "seat:dell".to_owned(),
            1,
        );
        let active =
            super::super::session_broker::mark_active(&requested, 2).expect("active session");
        MeshSessionStore::new(root.path().to_path_buf())
            .publish(&active)
            .expect("persist active session");
        let worker = ClipboardSyncWorker::new(root.path().to_path_buf());
        let clip = ClipEventBody::from_text(
            "guest copy",
            "vnc:serving-peer:session-1",
            "2026-07-31T12:00:00Z",
        );
        assert_eq!(
            worker.vnc_target_seat(&clip).expect("route lookup"),
            Some(("session-1".to_owned(), "seat:dell".to_owned()))
        );
        let stale = ClipEventBody::from_text(
            "guest copy",
            "vnc:other-peer:session-1",
            "2026-07-31T12:00:00Z",
        );
        assert_eq!(
            worker.vnc_target_seat(&stale).expect("stale lookup"),
            Some((String::new(), String::new()))
        );
    }

    #[test]
    fn apply_pushes_new_clip_to_front_and_stamps_it() {
        let mut h = History::default();
        assert!(apply_clip(
            &mut h,
            "hello",
            "alpha",
            "2026-06-21T10:00:00+00:00"
        ));
        assert_eq!(h.entries.len(), 1);
        let e = &h.entries[0];
        assert_eq!(e.text, "hello");
        assert_eq!(e.source, "alpha"); // O6 source stamp
        assert_eq!(e.time, "2026-06-21T10:00:00+00:00"); // O6 time stamp
        assert!(!e.pinned);
        assert_eq!(e.id, clip_id("hello"));
    }

    #[test]
    fn o2_debounce_drops_identical_top_clip() {
        // Re-copying / the viewer echoing the SAME top clip is a no-op.
        let mut h = History::default();
        assert!(apply_clip(&mut h, "x", "a", "t1"));
        assert!(
            !apply_clip(&mut h, "x", "a", "t2"),
            "identical top → debounced"
        );
        assert!(
            !apply_clip(&mut h, "x", "b", "t3"),
            "even from a different source"
        );
        assert_eq!(h.entries.len(), 1);
        assert_eq!(h.entries[0].time, "t1", "no rewrite on debounce");
    }

    #[test]
    fn o3_dedup_moves_existing_entry_to_top() {
        let mut h = History::default();
        apply_clip(&mut h, "a", "n", "t1");
        apply_clip(&mut h, "b", "n", "t2");
        apply_clip(&mut h, "c", "n", "t3");
        // Re-copy "a" (now at the bottom) — it must move to the top, NOT dup.
        assert!(apply_clip(&mut h, "a", "host2", "t4"));
        assert_eq!(
            h.entries
                .iter()
                .map(|e| e.text.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "c", "b"]
        );
        assert_eq!(h.entries.len(), 3, "no duplicate");
        assert_eq!(
            h.entries[0].source, "host2",
            "source refreshed on re-surface"
        );
        assert_eq!(h.entries[0].time, "t4");
    }

    #[test]
    fn o3_dedup_preserves_pin_on_resurface() {
        let mut h = History {
            entries: vec![entry("top", false), entry("pinned-old", true)],
        };
        // Re-copy the pinned entry's text → moves to top, stays pinned.
        assert!(apply_clip(&mut h, "pinned-old", "n", "t"));
        assert_eq!(h.entries[0].text, "pinned-old");
        assert!(h.entries[0].pinned, "pin survives a move-to-top");
    }

    #[test]
    fn o7_cap_trims_to_50_unpinned_oldest_first() {
        let mut h = History::default();
        for i in 0..60 {
            apply_clip(&mut h, &format!("clip-{i}"), "n", "t");
        }
        assert_eq!(h.entries.len(), HISTORY_CAP, "trimmed to 50 unpinned");
        // Newest first; the 10 oldest (clip-0..clip-9) were dropped.
        assert_eq!(h.entries[0].text, "clip-59");
        assert_eq!(h.entries[HISTORY_CAP - 1].text, "clip-10");
        assert!(!h.entries.iter().any(|e| e.text == "clip-0"));
    }

    #[test]
    fn o7_pins_are_exempt_from_the_cap_and_unlimited() {
        // 50 pinned + 50 unpinned → file holds all 100; only unpinned capped.
        let mut h = History::default();
        for i in 0..50 {
            h.entries.push(entry(&format!("pin-{i}"), true));
        }
        for i in 0..60 {
            apply_clip(&mut h, &format!("clip-{i}"), "n", "t");
        }
        let pinned = h.entries.iter().filter(|e| e.pinned).count();
        let unpinned = h.entries.iter().filter(|e| !e.pinned).count();
        assert_eq!(pinned, 50, "every pin survives — unlimited");
        assert_eq!(unpinned, HISTORY_CAP, "unpinned still capped at 50");
        assert!(h.entries.len() > HISTORY_CAP, "file longer than the cap");
    }

    #[test]
    fn trim_unpinned_drops_oldest_unpinned_keeps_pins_in_place() {
        // newest→oldest: u3, p, u2, u1  (cap 2 unpinned → drop u1, the oldest)
        let mut h = History {
            entries: vec![
                entry("u3", false),
                entry("p", true),
                entry("u2", false),
                entry("u1", false),
            ],
        };
        trim_unpinned(&mut h, 2);
        assert_eq!(
            h.entries
                .iter()
                .map(|e| e.text.as_str())
                .collect::<Vec<_>>(),
            vec!["u3", "p", "u2"]
        );
    }

    #[test]
    fn clip_id_is_stable_and_content_addressed() {
        assert_eq!(clip_id("hello"), clip_id("hello"));
        assert_ne!(clip_id("hello"), clip_id("world"));
        assert_eq!(clip_id("hello").len(), 16);
    }

    #[test]
    fn canonical_clip_event_body_shape_is_locked() {
        let body = ClipEventBody::from_text("from bus", "seat/node-a", "2026-07-26T10:30:00Z");
        let encoded = serde_json::to_value(&body).unwrap();
        let obj = encoded.as_object().unwrap();
        let keys: std::collections::BTreeSet<&str> = obj.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["id", "source", "text", "time"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            "event/clipboard/clip stays compatible with {{ id, text, source, time }}"
        );
        assert_eq!(body.id, clip_id("from bus"));
        assert_eq!(parse_clip_event_body(&encoded.to_string()).unwrap(), body);
    }

    #[test]
    fn canonical_event_body_does_not_grant_pin_state() {
        let id = clip_id("pinned?");
        let parsed = parse_clip_event_body(
            &format!(
                r#"{{"id":"{id}","text":"pinned?","source":"remote","time":"2026-07-26T10:30:00Z","pinned":true}}"#
            ),
        )
        .unwrap();
        let mut h = History::default();
        assert!(apply_clip_event(&mut h, &parsed));
        assert_eq!(h.entries[0].id, id);
        assert!(!h.entries[0].pinned, "pin state is history-only");
    }

    #[test]
    fn malformed_clip_event_bodies_are_rejected() {
        for body in [
            "not json",
            r#"{"id":"","text":"x","source":"n","time":"2026-07-26T10:30:00Z"}"#,
            r#"{"id":"i","text":"   ","source":"n","time":"2026-07-26T10:30:00Z"}"#,
            r#"{"id":"i","text":"x","source":"","time":"2026-07-26T10:30:00Z"}"#,
            r#"{"id":"wrong","text":"x","source":"n","time":"2026-07-26T10:30:00Z"}"#,
            r#"{"id":"i","text":"x","source":"n","time":"today"}"#,
        ] {
            assert!(
                parse_clip_event_body(body).is_err(),
                "body should be rejected: {body}"
            );
        }
    }

    #[test]
    fn oversized_clip_event_is_rejected_before_history_persistence() {
        let text = "x".repeat(MAX_CLIP_BYTES + 1);
        let body = ClipEventBody::from_text(&text, "remote", "2026-07-26T10:30:00Z");
        let encoded = serde_json::to_string(&body).unwrap();
        assert!(parse_clip_event_body(&encoded)
            .expect_err("oversized text must be rejected")
            .contains("byte limit"));

        let mut history = History::default();
        assert!(!apply_clip_event(&mut history, &body));
        assert!(history.entries.is_empty());
    }

    #[test]
    fn read_history_tolerates_missing_and_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("clipboard/history.json");
        assert_eq!(read_history(&p), History::default()); // missing
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "not json").unwrap();
        assert_eq!(read_history(&p), History::default()); // corrupt → empty
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let p = history_path(dir.path());
        let mut h = History::default();
        apply_clip(&mut h, "round-trip", "src", "2026-06-21T10:00:00+00:00");
        write_history(&p, &h).unwrap();
        assert!(p.is_file());
        assert_eq!(read_history(&p), h);
    }

    #[test]
    fn history_path_is_clipboard_history_json() {
        assert_eq!(
            history_path(Path::new("/mnt/mesh")),
            PathBuf::from("/mnt/mesh/clipboard/history.json")
        );
    }

    #[test]
    fn age_label_buckets() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-06-21T12:00:00+00:00")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let at = |s: &str| {
            let t = now - chrono::Duration::seconds(s.parse::<i64>().unwrap());
            age_label(&t.to_rfc3339(), now)
        };
        assert_eq!(at("2"), "now");
        assert_eq!(at("30"), "30s");
        assert_eq!(at("120"), "2m");
        assert_eq!(at("7200"), "2h");
        assert_eq!(at("172800"), "2d");
        assert_eq!(age_label("garbage", now), "now"); // unparseable → now
    }

    #[test]
    fn bus_clip_events_write_and_dedup_history_end_to_end() {
        let history_dir = tempfile::tempdir().unwrap();
        let bus_dir = tempfile::tempdir().unwrap();
        let mut persist = Persist::open(bus_dir.path().to_path_buf()).unwrap();
        let bodies = [
            ClipEventBody::from_text("first", "nodeA", "2026-07-26T10:00:00Z"),
            ClipEventBody::from_text("second", "nodeB", "2026-07-26T10:01:00Z"),
            ClipEventBody::from_text("first", "nodeA", "2026-07-26T10:02:00Z"),
        ];
        for body in &bodies {
            persist
                .write(
                    CLIP_TOPIC,
                    Priority::Default,
                    None,
                    Some(&serde_json::to_string(body).unwrap()),
                )
                .unwrap();
        }

        let w = ClipboardSyncWorker::new(history_dir.path().to_path_buf())
            .with_bus_root(bus_dir.path().to_path_buf());
        let mut cursor = None;
        assert_eq!(w.drain_clip_events(&mut persist, &mut cursor, None), 3);
        let h = read_history(&history_path(history_dir.path()));
        assert_eq!(
            h.entries
                .iter()
                .map(|e| e.text.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert_eq!(h.entries[0].source, "nodeA");
        assert_eq!(h.entries[0].time, "2026-07-26T10:02:00Z");
    }

    #[test]
    fn durable_cursor_resumes_after_restart_without_replaying_retained_lane() {
        let history_dir = tempfile::tempdir().unwrap();
        let bus_dir = tempfile::tempdir().unwrap();
        let mut persist = Persist::open(bus_dir.path().to_path_buf()).unwrap();
        let first = ClipEventBody::from_text("first", "nodeA", "2026-07-26T10:00:00Z");
        persist
            .write(
                CLIP_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&first).unwrap()),
            )
            .unwrap();

        let checkpoint = cursor_path(bus_dir.path());
        let w = ClipboardSyncWorker::new(history_dir.path().to_path_buf());
        let mut cursor = None;
        assert_eq!(
            w.drain_clip_events(&mut persist, &mut cursor, Some(&checkpoint)),
            1
        );
        let saved = read_cursor(&checkpoint).expect("successful fold is checkpointed");
        assert_eq!(cursor.as_deref(), Some(saved.as_str()));

        // A restarted daemon loads the durable acknowledgement and consumes
        // only the event published after it, while the retained first event is
        // not folded a second time.
        let second = ClipEventBody::from_text("second", "nodeB", "2026-07-26T10:01:00Z");
        persist
            .write(
                CLIP_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&second).unwrap()),
            )
            .unwrap();
        let mut restarted_cursor = read_cursor(&checkpoint);
        assert_eq!(
            w.drain_clip_events(&mut persist, &mut restarted_cursor, Some(&checkpoint)),
            1
        );
        assert_eq!(
            read_history(&history_path(history_dir.path()))
                .entries
                .iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            vec!["second", "first"]
        );
    }

    #[test]
    fn replicated_history_head_materializes_once_for_the_exact_local_seat() {
        let history_dir = tempfile::tempdir().expect("history root");
        let bus_dir = tempfile::tempdir().expect("bus root");
        let mut persist = Persist::open(bus_dir.path().to_path_buf()).expect("open bus");
        let old = ClipEventBody::from_text("old clipboard", "seat:source", "2026-08-03T12:00:00Z");
        let mut history = History::default();
        assert!(apply_clip_event(&mut history, &old));
        write_history(&history_path(history_dir.path()), &history).expect("write old history");
        let mut observed = Some(old);

        let fresh = ClipEventBody::from_text("fresh clipboard", "seat:remote", &now_rfc3339());
        assert!(apply_clip_event(&mut history, &fresh));
        write_history(&history_path(history_dir.path()), &history).expect("write fresh history");
        let worker = ClipboardSyncWorker::new(history_dir.path().to_path_buf())
            .with_bus_root(bus_dir.path().to_path_buf())
            .with_target_seat("seat:eagle");

        assert!(worker
            .materialize_replicated_head(&mut persist, &mut observed)
            .expect("materialize changed head"));
        let message = persist
            .read_latest(CLIPBOARD_MATERIALIZATION_TOPIC)
            .expect("read materialization")
            .expect("materialization exists");
        let handoff: ClipboardMaterialization =
            serde_json::from_str(message.body.as_deref().expect("materialization body"))
                .expect("decode materialization");
        assert_eq!(handoff.target_seat, "seat:eagle");
        assert_eq!(String::from(handoff.text), "fresh clipboard");
        assert_eq!(handoff.source, "seat:remote");
        assert_eq!(handoff.time, fresh.time);
        assert!(
            persist
                .read_latest(CLIP_TOPIC)
                .expect("read capture lane")
                .is_none(),
            "a replicated handoff must not fabricate a local capture event"
        );

        assert!(!worker
            .materialize_replicated_head(&mut persist, &mut observed)
            .expect("unchanged head is a no-op"));
        assert_eq!(
            persist
                .list_since(CLIPBOARD_MATERIALIZATION_TOPIC, None)
                .expect("list handoffs")
                .len(),
            1,
            "one replicated head produces exactly one local handoff",
        );
    }

    #[test]
    fn failed_history_write_does_not_acknowledge_event_and_retries() {
        let history_dir = tempfile::tempdir().unwrap();
        let bus_dir = tempfile::tempdir().unwrap();
        let mut persist = Persist::open(bus_dir.path().to_path_buf()).unwrap();
        let body = ClipEventBody::from_text("retry me", "nodeA", "2026-07-26T10:00:00Z");
        persist
            .write(
                CLIP_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&body).unwrap()),
            )
            .unwrap();

        // Make the history parent unusable. The event must remain unacked.
        std::fs::write(history_dir.path().join("clipboard"), b"not a directory").unwrap();
        let checkpoint = cursor_path(bus_dir.path());
        let w = ClipboardSyncWorker::new(history_dir.path().to_path_buf());
        let mut cursor = None;
        assert_eq!(
            w.drain_clip_events(&mut persist, &mut cursor, Some(&checkpoint)),
            0
        );
        assert!(cursor.is_none());
        assert!(!checkpoint.exists());

        // Repair the destination and drain again: the same retained event is
        // now applied because the failed attempt never advanced the cursor.
        std::fs::remove_file(history_dir.path().join("clipboard")).unwrap();
        std::fs::create_dir(history_dir.path().join("clipboard")).unwrap();
        assert_eq!(
            w.drain_clip_events(&mut persist, &mut cursor, Some(&checkpoint)),
            1
        );
        assert_eq!(
            read_history(&history_path(history_dir.path())).entries[0].text,
            "retry me"
        );
        assert!(read_cursor(&checkpoint).is_some());
    }

    #[test]
    fn multi_line_clip_is_one_verbatim_entry() {
        let mut h = History::default();
        let snippet = "line one\nline two\nline three";
        let body = ClipEventBody::from_text(snippet, "n", "2026-07-26T10:30:00Z");
        assert!(apply_clip_event(&mut h, &body));
        assert_eq!(h.entries.len(), 1);
        assert_eq!(h.entries[0].text, snippet, "newlines preserved, one entry");
    }

    #[test]
    fn clip_share_writable_core_writes_when_root_exists() {
        // SUBSTRATE-V2: the canonical path is a plain Syncthing dir, so the
        // clipboard worker MUST treat an EXISTING dir as writable — otherwise
        // every clip was dropped, leaving the Hub's Clipboard Viewer empty.
        let canonical = Path::new(crate::CANONICAL_QNM_MOUNT);
        assert!(
            clip_share_writable_core(canonical, /* root_is_dir = */ true),
            "present plain dir → writable"
        );
    }

    #[test]
    fn clip_share_writable_core_skips_missing_root() {
        // The shared dir doesn't exist yet (early boot, before Syncthing
        // provisions it): NOT writable, so we don't error per-clip writing into a
        // missing path that would land on a bare local dir.
        let canonical = Path::new(crate::CANONICAL_QNM_MOUNT);
        assert!(!clip_share_writable_core(
            canonical, /* root_is_dir = */ false
        ));
    }

    #[test]
    fn clip_share_writable_core_allows_non_canonical_roots() {
        // A non-canonical root (dev tree / tempdir) is always writable.
        let dir = tempfile::tempdir().unwrap();
        assert!(clip_share_writable_core(dir.path(), true));
        assert!(clip_share_writable_core(dir.path(), false));
    }

    #[test]
    fn whitespace_only_clip_is_skipped() {
        let mut h = History::default();
        let body = ClipEventBody {
            id: "blank".into(),
            text: "   ".into(),
            source: "n".into(),
            time: "2026-07-26T10:30:00Z".into(),
        };
        assert!(!apply_clip_event(&mut h, &body));
        assert!(h.entries.is_empty(), "blank/whitespace selections skipped");
    }
}
