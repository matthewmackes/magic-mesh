//! `mde-collab-types` — the stable **public contracts** for the Communications
//! suite (WL-FUNC-011, Phase 0).
//!
//! This is the leaf crate the whole suite compiles against: the mackesd collab
//! worker (`mde-collab-core`), and the egui surface (`mde-collab-egui`) all
//! import these types so a change to the wire shape is a change *here*, in one
//! reviewed place. It is deliberately minimal in behaviour — **pure types +
//! serialization + Ed25519 signing, with no business logic and no I/O**. There
//! is no Bus, no SQLite, no wall-clock: every timestamp and logical-clock value
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
//! * [`read_model`] — [`CollabReadModel`] and its projection structs, the
//!   read-side shapes the surface renders.
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

pub mod clock;
pub mod command;
pub mod envelope;
pub mod event;
pub mod ids;
pub mod read_model;
pub mod space;
pub mod topics;
pub mod value;

#[cfg(test)]
mod tests;

pub use clock::{ActorClock, ActorId};
pub use command::{CollabCommand, TransferControl};
pub use envelope::{CollabEventEnvelope, EventSignature, SCHEMA_VERSION};
pub use event::CollabEventKind;
pub use ids::{CallId, DocumentId, EventId, FileRefId, SpaceId, ThreadId, TransferId};
pub use read_model::{
    ActivityEntry, ActivityFeed, AlertInbox, AlertView, CallParticipantView, CallState, CallView,
    ClipboardLane, ClipboardView, CollabReadModel, CommsBadges, ConversationTimeline,
    DocumentSession, DocumentSessions, FileReferenceView, FileReferences, MessageView,
    PresenceBoard, PresenceView, SpaceDirectory, SpaceSummary, ThreadTimeline, TransferJobView,
    TransferJobs,
};
pub use space::{SpaceKind, SpaceRole};
pub use value::{
    sha256_hex, AiSuggestion, AiSuggestionKind, AlertAction, AlertActionKind, AlertPayload,
    CallKind, CallParticipantState, ClipItemKind, ClipboardItem, DeliveryState, DocumentChange,
    FileRef, MessageBody, PayloadRef, PresenceState, ReviewVerdict, Severity, TransferDirection,
    TransferMethod, TransferState,
};
