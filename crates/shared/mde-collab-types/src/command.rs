//! [`CollabCommand`] — the typed operations the surface asks the worker to do.
//!
//! A command is an *intent* published on `action/collab/*`; the worker
//! validates it, mints + signs the resulting [`CollabEventEnvelope`]s, and
//! projects them into the read models. Commands are never signed themselves
//! (the worker signs the events they produce); they are plain typed requests.

use serde::{Deserialize, Serialize};

use crate::ids::{CallId, DocumentId, EventId, FileRefId, SpaceId, ThreadId, TransferId};
use crate::space::{SpaceKind, SpaceRole};
use crate::value::{
    AiSuggestionKind, ClipboardItem, DocumentChange, FileRef, MessageBody, PresenceState,
    ReviewVerdict, Severity, TransferDirection, TransferMethod,
};
use crate::ActorId;

/// How to control a live transfer (the pause/resume/cancel verbs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferControl {
    /// Pause a running transfer.
    Pause,
    /// Resume a paused transfer.
    Resume,
    /// Cancel a transfer.
    Cancel,
}

/// A typed Communications operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollabCommand {
    // ---- Space lifecycle -----------------------------------------------
    /// Create a new space.
    CreateSpace {
        /// The kind of space.
        kind: SpaceKind,
        /// Human name.
        name: String,
    },
    /// Rename a space.
    RenameSpace {
        /// The space.
        space: SpaceId,
        /// The new name.
        name: String,
    },
    /// Delete a space (owner-gated at the worker).
    DeleteSpace {
        /// The space.
        space: SpaceId,
    },

    // ---- Membership + presence -----------------------------------------
    /// Add a member to a space.
    AddMember {
        /// The space.
        space: SpaceId,
        /// The member to add.
        actor: ActorId,
        /// The role to grant.
        role: SpaceRole,
    },
    /// Remove a member from a space.
    RemoveMember {
        /// The space.
        space: SpaceId,
        /// The member to remove.
        actor: ActorId,
    },
    /// Change a member's role.
    SetMemberRole {
        /// The space.
        space: SpaceId,
        /// The member.
        actor: ActorId,
        /// The new role.
        role: SpaceRole,
    },
    /// Join an open-join space yourself.
    JoinSpace {
        /// The space.
        space: SpaceId,
    },
    /// Leave a space yourself.
    LeaveSpace {
        /// The space.
        space: SpaceId,
    },
    /// Set your own presence + optional status line.
    SetPresence {
        /// The new presence.
        presence: PresenceState,
        /// Optional free-text status.
        #[serde(default)]
        status: Option<String>,
    },

    // ---- Messages + threads --------------------------------------------
    /// Post a message into a space (optionally into a thread).
    SendMessage {
        /// The space.
        space: SpaceId,
        /// The thread, when replying in one.
        #[serde(default)]
        thread: Option<ThreadId>,
        /// Markdown body.
        body: MessageBody,
    },
    /// Edit one of your own messages (within the edit window).
    EditMessage {
        /// The space.
        space: SpaceId,
        /// The message to edit.
        target: EventId,
        /// The new Markdown body.
        body: MessageBody,
    },
    /// Delete one of your own messages (within the delete window).
    DeleteMessage {
        /// The space.
        space: SpaceId,
        /// The message to delete.
        target: EventId,
    },
    /// Start a reply thread rooted at a message.
    StartThread {
        /// The space.
        space: SpaceId,
        /// The message the thread hangs off.
        root: EventId,
        /// Optional thread title.
        #[serde(default)]
        title: Option<String>,
    },
    /// Reply within an existing thread.
    ReplyInThread {
        /// The space.
        space: SpaceId,
        /// The thread.
        thread: ThreadId,
        /// Markdown body.
        body: MessageBody,
    },

    // ---- Alerts --------------------------------------------------------
    /// Acknowledge an alert.
    AckAlert {
        /// The space.
        space: SpaceId,
        /// The alert event.
        alert: EventId,
    },
    /// Snooze an alert until a time.
    SnoozeAlert {
        /// The space.
        space: SpaceId,
        /// The alert event.
        alert: EventId,
        /// Injected epoch-ms the snooze expires at.
        until_unix_ms: i64,
    },
    /// Run a typed inline alert action (destructive requires `armed`).
    RunAlertAction {
        /// The space.
        space: SpaceId,
        /// The alert event.
        alert: EventId,
        /// The action id within the alert.
        action_id: String,
        /// Whether a destructive action is armed.
        armed: bool,
    },
    /// Mute (or unmute) an alert source for the local seat.
    SetAlertMute {
        /// The alert source key (a node, a lane, a space).
        source: String,
        /// `true` to mute, `false` to unmute.
        muted: bool,
    },
    /// Set the least-severe level that still rings (the seat's threshold).
    SetSeverityThreshold {
        /// The minimum severity that rings.
        threshold: Severity,
    },
    /// Toggle fleet-wide Do-Not-Disturb.
    SetDoNotDisturb {
        /// `true` to enable DND, `false` to clear it.
        enabled: bool,
    },

    // ---- Clipboard -----------------------------------------------------
    /// Publish a clipboard item into a space's clipboard lane.
    PublishClipboard {
        /// The space.
        space: SpaceId,
        /// The captured clip.
        item: ClipboardItem,
    },
    /// Attach an existing clipboard item to a message (re-share a clip).
    AttachClipboard {
        /// The space.
        space: SpaceId,
        /// The clip event to attach.
        clip: EventId,
    },
    /// Pin a clipboard item (survives cap + clear).
    PinClipboard {
        /// The space.
        space: SpaceId,
        /// The clip event.
        clip: EventId,
    },
    /// Unpin a clipboard item.
    UnpinClipboard {
        /// The space.
        space: SpaceId,
        /// The clip event.
        clip: EventId,
    },
    /// Delete a single clipboard item.
    DeleteClipboard {
        /// The space.
        space: SpaceId,
        /// The clip event.
        clip: EventId,
    },
    /// Clear all unpinned clipboard items in a space.
    ClearClipboard {
        /// The space.
        space: SpaceId,
    },

    // ---- Documents + reviews -------------------------------------------
    /// Create a collaboratively-edited document in a space.
    CreateDocument {
        /// The space.
        space: SpaceId,
        /// The new document id.
        document: DocumentId,
        /// Title.
        title: String,
    },
    /// Apply a change to a document (edit bytes are content-addressed).
    UpdateDocument {
        /// The space.
        space: SpaceId,
        /// The document.
        document: DocumentId,
        /// The change.
        change: DocumentChange,
    },
    /// Request a review on a document.
    RequestReview {
        /// The space.
        space: SpaceId,
        /// The document.
        document: DocumentId,
        /// The reviewers.
        reviewers: Vec<ActorId>,
    },
    /// Submit a review on a document.
    SubmitReview {
        /// The space.
        space: SpaceId,
        /// The document.
        document: DocumentId,
        /// The verdict.
        verdict: ReviewVerdict,
        /// Optional comment.
        #[serde(default)]
        comment: Option<String>,
    },

    // ---- File references -----------------------------------------------
    /// Link a file into a space.
    LinkFile {
        /// The space.
        space: SpaceId,
        /// The stable file-reference id.
        file: FileRefId,
        /// The file metadata.
        reference: FileRef,
    },
    /// Remove a file reference from a space.
    UnlinkFile {
        /// The space.
        space: SpaceId,
        /// The file-reference id.
        file: FileRefId,
    },

    // ---- Transfers -----------------------------------------------------
    /// Start a transfer for a linked file.
    StartTransfer {
        /// The space.
        space: SpaceId,
        /// The transfer's control id.
        transfer: TransferId,
        /// The file to move.
        file: FileRefId,
        /// The transport.
        method: TransferMethod,
        /// The direction relative to this seat.
        direction: TransferDirection,
    },
    /// Pause/resume/cancel a live transfer.
    ControlTransfer {
        /// The transfer's control id.
        transfer: TransferId,
        /// The control verb.
        control: TransferControl,
    },

    // ---- Calls ---------------------------------------------------------
    /// Start a call/co-edit/remote-desktop session.
    StartCall {
        /// The space.
        space: SpaceId,
        /// The new call id.
        call: CallId,
        /// What the call carries.
        kind: crate::value::CallKind,
    },
    /// Answer a ringing call.
    AnswerCall {
        /// The call.
        call: CallId,
    },
    /// Decline a ringing call.
    DeclineCall {
        /// The call.
        call: CallId,
    },
    /// Hang up / leave an active call.
    HangUpCall {
        /// The call.
        call: CallId,
    },
    /// Send an in-call DTMF digit.
    SendDtmf {
        /// The call.
        call: CallId,
        /// The digit/tone character.
        digit: char,
    },
    /// Toggle in-call microphone mute.
    SetCallMuted {
        /// The call.
        call: CallId,
        /// `true` to mute.
        muted: bool,
    },

    // ---- AI ------------------------------------------------------------
    /// Request an AI suggestion (smart reply / summary / action item / draft).
    RequestAiSuggestion {
        /// The space.
        space: SpaceId,
        /// The event it should be about, if any.
        #[serde(default)]
        target: Option<EventId>,
        /// The kind of assistance requested.
        kind: AiSuggestionKind,
    },
}

impl CollabCommand {
    /// A stable discriminant string for this command — matches the serde wire
    /// tag; used for the `action/collab/<verb>` topic and metrics.
    #[must_use]
    pub const fn verb(&self) -> &'static str {
        match self {
            Self::CreateSpace { .. } => "create_space",
            Self::RenameSpace { .. } => "rename_space",
            Self::DeleteSpace { .. } => "delete_space",
            Self::AddMember { .. } => "add_member",
            Self::RemoveMember { .. } => "remove_member",
            Self::SetMemberRole { .. } => "set_member_role",
            Self::JoinSpace { .. } => "join_space",
            Self::LeaveSpace { .. } => "leave_space",
            Self::SetPresence { .. } => "set_presence",
            Self::SendMessage { .. } => "send_message",
            Self::EditMessage { .. } => "edit_message",
            Self::DeleteMessage { .. } => "delete_message",
            Self::StartThread { .. } => "start_thread",
            Self::ReplyInThread { .. } => "reply_in_thread",
            Self::AckAlert { .. } => "ack_alert",
            Self::SnoozeAlert { .. } => "snooze_alert",
            Self::RunAlertAction { .. } => "run_alert_action",
            Self::SetAlertMute { .. } => "set_alert_mute",
            Self::SetSeverityThreshold { .. } => "set_severity_threshold",
            Self::SetDoNotDisturb { .. } => "set_do_not_disturb",
            Self::PublishClipboard { .. } => "publish_clipboard",
            Self::AttachClipboard { .. } => "attach_clipboard",
            Self::PinClipboard { .. } => "pin_clipboard",
            Self::UnpinClipboard { .. } => "unpin_clipboard",
            Self::DeleteClipboard { .. } => "delete_clipboard",
            Self::ClearClipboard { .. } => "clear_clipboard",
            Self::CreateDocument { .. } => "create_document",
            Self::UpdateDocument { .. } => "update_document",
            Self::RequestReview { .. } => "request_review",
            Self::SubmitReview { .. } => "submit_review",
            Self::LinkFile { .. } => "link_file",
            Self::UnlinkFile { .. } => "unlink_file",
            Self::StartTransfer { .. } => "start_transfer",
            Self::ControlTransfer { .. } => "control_transfer",
            Self::StartCall { .. } => "start_call",
            Self::AnswerCall { .. } => "answer_call",
            Self::DeclineCall { .. } => "decline_call",
            Self::HangUpCall { .. } => "hang_up_call",
            Self::SendDtmf { .. } => "send_dtmf",
            Self::SetCallMuted { .. } => "set_call_muted",
            Self::RequestAiSuggestion { .. } => "request_ai_suggestion",
        }
    }
}
