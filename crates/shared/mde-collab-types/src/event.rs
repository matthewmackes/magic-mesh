//! [`CollabEventKind`] — the taxonomy of everything that can happen in a space.
//!
//! One variant per event class the seven replaced subsystems produce. Each is
//! the *body* of a [`CollabEventEnvelope`](crate::CollabEventEnvelope): the
//! envelope adds the identity, clock, timestamp, and signature; the kind says
//! what happened and carries its typed (inline) payload. Large substance
//! (document/CRDT blobs, file bytes) is carried out-of-band and named by the
//! envelope's content-addressed [`PayloadRef`](crate::PayloadRef).
//!
//! Coverage of the 519-row parity ledger's event classes is asserted by the
//! `ledger_coverage` test in `tests.rs`.

use serde::{Deserialize, Serialize};

use crate::ids::{CallId, DocumentId, EventId, FileRefId, ThreadId, TransferId};
use crate::space::{SpaceKind, SpaceRole};
use crate::value::{
    AiSuggestion, AlertPayload, CallKind, CallParticipantState, ClipboardItem, DocumentChange,
    FileRef, MessageBody, PresenceState, ReviewVerdict, TransferDirection, TransferMethod,
    TransferState,
};
use crate::ActorId;

/// The body of one signed event. External-tagged so every variant round-trips
/// losslessly; `snake_case` tags are the stable wire names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollabEventKind {
    // ---- Space lifecycle ------------------------------------------------
    /// A space was created.
    SpaceCreated {
        /// The kind of space.
        kind: SpaceKind,
        /// Human name/title.
        name: String,
    },
    /// A space was renamed.
    SpaceRenamed {
        /// The new name.
        name: String,
    },
    /// A space was archived (hidden, retained).
    SpaceArchived,
    /// A space was deleted by its owner.
    SpaceDeleted,

    // ---- Membership + presence -----------------------------------------
    /// A member joined (or was added to) the space.
    MemberJoined {
        /// The member.
        actor: ActorId,
        /// Their role.
        role: SpaceRole,
    },
    /// A member left (or was removed from) the space.
    MemberLeft {
        /// The member.
        actor: ActorId,
    },
    /// A member's role changed.
    MemberRoleChanged {
        /// The member.
        actor: ActorId,
        /// The new role.
        role: SpaceRole,
    },
    /// A member's presence/status changed.
    PresenceChanged {
        /// The member.
        actor: ActorId,
        /// The new presence.
        presence: PresenceState,
        /// Optional free-text status line.
        #[serde(default)]
        status: Option<String>,
    },

    // ---- Messages + threads --------------------------------------------
    /// A message was posted (in the space's main timeline, or in a thread when
    /// `thread` is set).
    MessagePosted {
        /// Markdown body.
        body: MessageBody,
        /// The thread this belongs to, if it is a threaded reply.
        #[serde(default)]
        thread: Option<ThreadId>,
    },
    /// A message was edited by its author (the net-new edit window).
    MessageEdited {
        /// The edited message's event id.
        target: EventId,
        /// The new Markdown body.
        body: MessageBody,
    },
    /// A message was deleted by its author (the net-new delete window).
    MessageDeleted {
        /// The deleted message's event id.
        target: EventId,
    },
    /// A reply thread was started, rooted at a message.
    ThreadStarted {
        /// The new thread's id.
        thread: ThreadId,
        /// The message the thread hangs off.
        root: EventId,
        /// Optional thread title.
        #[serde(default)]
        title: Option<String>,
    },
    /// A thread was marked resolved.
    ThreadResolved {
        /// The resolved thread.
        thread: ThreadId,
    },

    // ---- Alerts --------------------------------------------------------
    /// An alert was raised (folded from a truthful Bus lane) into the space.
    AlertRaised {
        /// The alert substance.
        alert: AlertPayload,
    },
    /// An alert was acknowledged.
    AlertAcknowledged {
        /// The alert's event id.
        target: EventId,
    },
    /// An alert was snoozed until a time.
    AlertSnoozed {
        /// The alert's event id.
        target: EventId,
        /// Injected epoch-ms the snooze expires at.
        until_unix_ms: i64,
    },
    /// A typed inline alert action was invoked (safe fires immediately;
    /// destructive requires `armed`).
    AlertActionInvoked {
        /// The alert's event id.
        target: EventId,
        /// The action id within the alert.
        action_id: String,
        /// Whether a destructive action was armed.
        armed: bool,
        /// The outcome, once known (fired/refused_unarmed/…).
        #[serde(default)]
        outcome: Option<String>,
    },

    // ---- Clipboard -----------------------------------------------------
    /// A clipboard item was published into the space's clipboard lane.
    ClipboardPublished {
        /// The captured clip.
        item: ClipboardItem,
    },
    /// A clipboard item was pinned (survives cap + clear).
    ClipboardPinned {
        /// The clip's event id.
        target: EventId,
    },
    /// A clipboard item was unpinned.
    ClipboardUnpinned {
        /// The clip's event id.
        target: EventId,
    },
    /// A clipboard item was deleted.
    ClipboardDeleted {
        /// The clip's event id.
        target: EventId,
    },

    // ---- Documents + reviews -------------------------------------------
    /// A collaboratively-edited document was created.
    DocumentCreated {
        /// The document id.
        document: DocumentId,
        /// Title.
        title: String,
    },
    /// A document was updated (the edit bytes are content-addressed).
    DocumentUpdated {
        /// The document id.
        document: DocumentId,
        /// The change (a content-addressed CRDT update).
        change: DocumentChange,
    },
    /// A review was requested on a document.
    ReviewRequested {
        /// The document under review.
        document: DocumentId,
        /// The requested reviewers.
        reviewers: Vec<ActorId>,
    },
    /// A review was submitted on a document.
    ReviewSubmitted {
        /// The reviewed document.
        document: DocumentId,
        /// The verdict.
        verdict: ReviewVerdict,
        /// Optional review comment.
        #[serde(default)]
        comment: Option<String>,
    },

    // ---- File references -----------------------------------------------
    /// A file was linked into the space.
    FileLinked {
        /// The stable file-reference id.
        file: FileRefId,
        /// The file metadata (name/size/hash/mime).
        reference: FileRef,
    },
    /// A file reference was removed from the space.
    FileUnlinked {
        /// The file-reference id.
        file: FileRefId,
    },

    // ---- Transfers -----------------------------------------------------
    /// A file transfer was started for a linked file (the control handle; byte
    /// progress lives in the WL-FUNC-006 ledger keyed by `transfer`).
    TransferStarted {
        /// The transfer's control id.
        transfer: TransferId,
        /// The file being moved.
        file: FileRefId,
        /// The transport.
        method: TransferMethod,
        /// The direction relative to this seat.
        direction: TransferDirection,
    },
    /// A transfer's lifecycle state changed (queued→active→…/paused/…).
    TransferStateChanged {
        /// The transfer's control id.
        transfer: TransferId,
        /// The new state.
        state: TransferState,
    },

    // ---- Calls ---------------------------------------------------------
    /// A call/co-edit/remote-desktop session was started.
    CallStarted {
        /// The call id.
        call: CallId,
        /// What the call carries.
        kind: CallKind,
        /// Who initiated it.
        initiator: ActorId,
    },
    /// A participant's state within a call changed (ringing/connected/left/…).
    CallParticipantChanged {
        /// The call id.
        call: CallId,
        /// The participant.
        actor: ActorId,
        /// Their new state.
        state: CallParticipantState,
    },
    /// A call ended.
    CallEnded {
        /// The call id.
        call: CallId,
        /// A short reason (hung_up/declined/failed/…).
        #[serde(default)]
        reason: Option<String>,
    },

    // ---- AI suggestion metadata ----------------------------------------
    /// An AI suggestion was offered into the space (metadata only).
    AiSuggestionOffered {
        /// The suggestion metadata + provenance.
        suggestion: AiSuggestion,
    },
    /// An AI suggestion was accepted or dismissed.
    AiSuggestionResolved {
        /// The suggestion id.
        suggestion_id: String,
        /// Whether it was accepted (`true`) or dismissed (`false`).
        accepted: bool,
    },
}

impl CollabEventKind {
    /// A stable discriminant string for this kind — used for grouping, metrics,
    /// and the ledger-coverage test. Matches the serde wire tag.
    #[must_use]
    pub const fn tag(&self) -> &'static str {
        match self {
            Self::SpaceCreated { .. } => "space_created",
            Self::SpaceRenamed { .. } => "space_renamed",
            Self::SpaceArchived => "space_archived",
            Self::SpaceDeleted => "space_deleted",
            Self::MemberJoined { .. } => "member_joined",
            Self::MemberLeft { .. } => "member_left",
            Self::MemberRoleChanged { .. } => "member_role_changed",
            Self::PresenceChanged { .. } => "presence_changed",
            Self::MessagePosted { .. } => "message_posted",
            Self::MessageEdited { .. } => "message_edited",
            Self::MessageDeleted { .. } => "message_deleted",
            Self::ThreadStarted { .. } => "thread_started",
            Self::ThreadResolved { .. } => "thread_resolved",
            Self::AlertRaised { .. } => "alert_raised",
            Self::AlertAcknowledged { .. } => "alert_acknowledged",
            Self::AlertSnoozed { .. } => "alert_snoozed",
            Self::AlertActionInvoked { .. } => "alert_action_invoked",
            Self::ClipboardPublished { .. } => "clipboard_published",
            Self::ClipboardPinned { .. } => "clipboard_pinned",
            Self::ClipboardUnpinned { .. } => "clipboard_unpinned",
            Self::ClipboardDeleted { .. } => "clipboard_deleted",
            Self::DocumentCreated { .. } => "document_created",
            Self::DocumentUpdated { .. } => "document_updated",
            Self::ReviewRequested { .. } => "review_requested",
            Self::ReviewSubmitted { .. } => "review_submitted",
            Self::FileLinked { .. } => "file_linked",
            Self::FileUnlinked { .. } => "file_unlinked",
            Self::TransferStarted { .. } => "transfer_started",
            Self::TransferStateChanged { .. } => "transfer_state_changed",
            Self::CallStarted { .. } => "call_started",
            Self::CallParticipantChanged { .. } => "call_participant_changed",
            Self::CallEnded { .. } => "call_ended",
            Self::AiSuggestionOffered { .. } => "ai_suggestion_offered",
            Self::AiSuggestionResolved { .. } => "ai_suggestion_resolved",
        }
    }
}
