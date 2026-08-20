//! `mde-collab-types` — the stable **public contracts** for the Communications
//! suite (WL-FUNC-011, Phase 0).
//!
//! This is the leaf crate the whole suite compiles against: the mackesd collab
//! worker (`mde-collab-core`), and the egui surface (`mde-collab-egui`) all
//! import these types so a change to the wire shape is a change *here*, in one
//! reviewed place. It is deliberately minimal in behaviour — **pure types +
//! serialization + Ed25519 signing, with no business logic and no I/O**. There
//! is no Bus, no `SQLite`, no wall-clock: every timestamp and logical-clock value
//! is injected by the caller, so the same event log replays deterministically.
//!
//! # What lives here
//!
//! * [`ids`] — the seven opaque, stable UUID identifiers: [`SpaceId`],
//!   [`EventId`], [`ThreadId`], [`DocumentId`], [`FileRefId`], [`TransferId`],
//!   [`CallId`].
//! * [`space`] — [`SpaceKind`] (Direct/Team/Incident/Project) and [`SpaceRole`]
//!   (Owner/Member).
//! * [`clock`] — the [`ActorId`] identity and the [`ActorClock`] Hybrid Logical
//!   Clock that causally orders a space's log.
//! * [`value`] — the shared leaf value types (payload refs, alert/clipboard/
//!   file/AI payloads, the presence/severity/delivery/transfer/call enums).
//! * [`event`] — [`CollabEventKind`], the event taxonomy covering every class
//!   the seven replaced subsystems produce.
//! * [`envelope`] — [`CollabEventEnvelope`], the versioned, Ed25519-signed unit
//!   of the log, with deterministic canonical [`signing_bytes`] and content-
//!   addressed ([`PayloadRef`]) large-payload references.
//! * [`command`] — [`CollabCommand`], the typed operations the surface requests.
//! * [`transfer_v2`] — the strict endpoint/operation-separated `TransferJobV2`
//!   contract and its bounded executor/lifecycle types.
//! * [`read_model`] — [`CollabReadModel`] and its projection structs, the
//!   read-side shapes the surface renders.
//! * [`media`] — WL-FUNC-024 bounded [`MediaSessionV1`] / [`MediaTrackKind`] /
//!   [`MediaSessionStateV1`] contracts for the live media plane.
//! * [`topics`] — the `action/collab/*`, `state/collab/*`, and
//!   `collab/event/<space>/<actor>` topic helpers.
//!
//! # Signing
//!
//! The [`CollabEventEnvelope`] is signed with `ed25519-dalek` v2 — the exact
//! dep, version, and pattern mde-chat uses (openssl is forbidden). The signed
//! canonical bytes are domain-separated, field-delimited, and in a fixed order;
//! the signature field is excluded, so tampering with any other field
//! (actor/space/clock/timestamp/kind/payload-ref) invalidates the signature.
//!
//! [`signing_bytes`]: CollabEventEnvelope::signing_bytes

#![forbid(unsafe_code)]

/// WL-FUNC-016/WL-FUNC-011 strict rich clipboard transport contracts.
pub mod clipboard_v2;
pub mod clock;
pub mod command;
pub mod envelope;
pub mod event;
pub mod ids;
/// WL-FUNC-024 — bounded live-media session contracts (offer/answer, tracks, state).
pub mod media;
pub mod read_model;
pub mod space;
pub mod topics;
/// WL-FUNC-011 — the bounded adapter into the existing Files transfer view.
pub mod transfer;
/// WL-FUNC-011 — strict endpoint/operation-separated TransferJob V2 contracts.
pub mod transfer_v2;
pub mod value;

#[cfg(test)]
mod tests;

pub use clipboard_v2::{
    reject_duplicate_json_keys, ClipboardClipId, ClipboardDenialReasonV2, ClipboardDisclosureV2,
    ClipboardEchoGuardV2, ClipboardEnvelopeV2, ClipboardEnvelopeV2DecodeError,
    ClipboardEnvelopeV2ValidationError, ClipboardIdentityValidationError, ClipboardMimeKind,
    ClipboardMimeOfferV2, ClipboardNodeId, ClipboardPayloadV2, ClipboardSeatId,
    ClipboardSelectionDecisionV2, ClipboardSelectionV2, ClipboardSelectionV2DecodeError,
    ClipboardSessionId, ClipboardSignedAttributionV2, ClipboardSourceV2, ClipboardTargetV2,
    ClipboardTypedMetadataV2, ClipboardUnavailableReason, ClipboardUnsupportedReason,
    CLIPBOARD_ENVELOPE_V2_SCHEMA_VERSION, MAX_CLIPBOARD_ECHO_HOPS,
    MAX_CLIPBOARD_ENVELOPE_V2_JSON_BYTES, MAX_CLIPBOARD_FILE_ITEMS, MAX_CLIPBOARD_ID_BYTES,
    MAX_CLIPBOARD_IMAGE_DIMENSION_PX, MAX_CLIPBOARD_INLINE_TEXT_BYTES, MAX_CLIPBOARD_OFFERS,
    MAX_CLIPBOARD_PAYLOAD_BYTES, MAX_CLIPBOARD_PREVIEW_BYTES,
    MAX_CLIPBOARD_SELECTION_V2_JSON_BYTES, MAX_CLIPBOARD_TTL_MS,
};
pub use clock::{ActorClock, ActorId};
pub use command::{
    CollabCommand, TaskAction, TaskActionValidationError, TransferControl, MAX_TASK_TITLE_BYTES,
};
pub use envelope::{last_writer_wins, CollabEventEnvelope, EventSignature, SCHEMA_VERSION};
pub use event::CollabEventKind;
pub use ids::{CallId, DocumentId, EventId, FileRefId, SpaceId, ThreadId, TransferId};
pub use media::{
    media_answer_topic, media_offer_topic, media_session_topic, media_sfu_election_topic,
    MediaDescriptionV1, MediaFailureReasonV1, MediaSessionStateV1, MediaSessionV1,
    MediaSessionV1DecodeError, MediaSessionV1ValidationError, MediaSignalingRoleV1, MediaTrackKind,
    SfuElectionV1, MAX_MEDIA_ACTOR_BYTES, MAX_MEDIA_DESCRIPTION_V1_JSON_BYTES,
    MAX_MEDIA_RECONNECT_ATTEMPTS, MAX_MEDIA_SESSION_V1_JSON_BYTES, MAX_MEDIA_TRACKS,
    MAX_SFU_ELECTION_PARTICIPANTS, MAX_SFU_ELECTION_V1_JSON_BYTES, MEDIA_SESSION_V1_SCHEMA_VERSION,
    MEDIA_STATE_PREFIX,
};
pub use read_model::{
    ActivityEntry, ActivityFeed, AiSuggestionRequestStatus, AiSuggestionRequestView,
    AiSuggestionRequests, AlertInbox, AlertView, CallMediaAdapter, CallMediaAdmission,
    CallMediaFrameEvidence, CallMediaReadiness, CallMediaRequirement, CallMediaSession,
    CallMediaVerification, CallMediaVerificationRow, CallMediaVerificationStatus,
    CallParticipantView, CallState, CallView, ChannelTasks, ClipboardLane, ClipboardView,
    CollabReadModel, CommsBadges, ConversationTimeline, DiscordBridgeBoard,
    DiscordBridgeConfigStatus, DiscordBridgeFlowStatus, DiscordBridgeProvenance,
    DiscordBridgeProvenanceSource, DiscordBridgeView, DocumentSession, DocumentSessions,
    FileReferenceView, FileReferences, MessagePins, MessageView, PresenceBoard, PresenceView,
    SavedMessageView, SavedMessages, SpaceDirectory, SpaceSummary, TaskView, ThreadTimeline,
    TransferJobView, TransferJobs,
};
pub use space::{SpaceKind, SpaceRole};
pub use transfer::{admit_v2_job, TransferLedgerAdmissionError};
pub use transfer_v2::{
    ChecksumMode, ChecksumPolicy, OpaqueNodeRef, OpaqueProfileRef, OpaqueResourceRef,
    RecurringSchedule, ScrapeOutputKind, TransferAction, TransferControlV2, TransferEndpoint,
    TransferError, TransferErrorCode, TransferJobV2, TransferJobV2DecodeError,
    TransferJobV2ValidationError, TransferKind, TransferLocation, TransferLocationFamily,
    TransferOperation, TransferPhase, TransferProgress, TransferRefValidationError,
    MAX_TRANSFER_ATTEMPTS, MAX_TRANSFER_BANDWIDTH_BYTES_PER_SECOND, MAX_TRANSFER_CONTENT_BYTES,
    MAX_TRANSFER_CONTENT_TYPE_BYTES, MAX_TRANSFER_ERROR_DETAIL_BYTES,
    MAX_TRANSFER_JOB_V2_JSON_BYTES, MAX_TRANSFER_OPAQUE_REF_BYTES,
    MAX_TRANSFER_RATE_BYTES_PER_SECOND, MAX_TRANSFER_RECURRENCE_RUNS,
    MAX_TRANSFER_RECURRENCE_SECONDS, TRANSFER_JOB_V2_SCHEMA_VERSION,
};
pub use value::{
    clipboard_clip_id, sha256_hex, AiSuggestion, AiSuggestionKind, AlertAction, AlertActionKind,
    AlertPayload, CallKind, CallParticipantState, ClipItemKind, ClipboardClipBody,
    ClipboardClipValidationError, ClipboardItem, DeliveryState, DocumentChange, FileRef,
    MessageBody, PayloadRef, PresenceState, ReviewVerdict, Severity, TransferDirection,
    TransferMethod, TransferState, MAX_CLIPBOARD_TEXT_BYTES,
};
