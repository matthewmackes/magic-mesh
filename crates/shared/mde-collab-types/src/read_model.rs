//! [`CollabReadModel`] — the retained read-side projections the egui surface
//! consumes off `state/collab/*`.
//!
//! These are **shapes only** — pure struct/enum definitions with no logic. The
//! collab worker folds the signed event log into them and publishes them
//! latest-wins; the surface renders them. Nothing here computes anything.

use serde::{Deserialize, Serialize};

use crate::clock::ActorClock;
use crate::ids::{CallId, DocumentId, EventId, FileRefId, SpaceId, ThreadId, TransferId};
use crate::space::{SpaceKind, SpaceRole};
use crate::value::{
    AlertPayload, CallKind, CallParticipantState, ClipItemKind, DeliveryState, FileRef,
    PresenceState, Severity, TransferDirection, TransferMethod, TransferState,
};
use crate::ActorId;

/// The full set of read-side projections, so a caller can name each shape by
/// one type. Each variant is an independently-published `state/collab/*` model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollabReadModel {
    /// The left-rail directory of spaces.
    SpaceDirectory(SpaceDirectory),
    /// A space's action-oriented Activity feed.
    Activity(ActivityFeed),
    /// A conversation or thread timeline.
    ConversationTimeline(ConversationTimeline),
    /// A thread timeline (root + replies).
    ThreadTimeline(ThreadTimeline),
    /// The live document co-edit sessions.
    DocumentSessions(DocumentSessions),
    /// The files linked into a space.
    FileReferences(FileReferences),
    /// The transfer jobs (mirror of the WL-FUNC-006 ledger, read-side).
    TransferJobs(TransferJobs),
    /// The global alert inbox.
    AlertInbox(AlertInbox),
    /// A space's clipboard lane.
    ClipboardLane(ClipboardLane),
    /// The presence board.
    Presence(PresenceBoard),
    /// The active call state.
    CallState(CallState),
}

/// The rail directory of spaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SpaceDirectory {
    /// One row per space the seat is a member of.
    pub spaces: Vec<SpaceSummary>,
}

/// A single rail row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceSummary {
    /// The space id.
    pub id: SpaceId,
    /// Its kind.
    pub kind: SpaceKind,
    /// Its name.
    pub name: String,
    /// The seat's role in it.
    pub role: SpaceRole,
    /// Unread event count feeding the badge (zero paints nothing).
    pub unread: u32,
    /// Member count.
    pub members: u32,
    /// The most recent activity clock (rail sort key).
    pub last_activity: ActorClock,
}

/// A space's chronological, action-oriented Activity feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ActivityFeed {
    /// The space this feed is for (`None` for the cross-space Activity).
    #[serde(default)]
    pub space: Option<SpaceId>,
    /// Newest-last feed entries.
    pub entries: Vec<ActivityEntry>,
}

/// One Activity row — a projected summary of an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityEntry {
    /// The event this summarizes.
    pub event_id: EventId,
    /// The space it happened in.
    pub space: SpaceId,
    /// The actor.
    pub actor: ActorId,
    /// The event's clock.
    pub clock: ActorClock,
    /// The event's creation time (epoch ms).
    pub created_unix_ms: i64,
    /// The event-kind discriminant (matches `CollabEventKind::tag`), so the
    /// feed can filter by band without re-parsing the whole event.
    pub kind_tag: String,
    /// A short human summary line.
    pub summary: String,
}

/// A conversation (or in-thread) timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationTimeline {
    /// The space.
    pub space: SpaceId,
    /// The thread, when this is a thread view.
    #[serde(default)]
    pub thread: Option<ThreadId>,
    /// Ordered messages.
    pub messages: Vec<MessageView>,
}

/// A rendered message row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageView {
    /// The message event id.
    pub event_id: EventId,
    /// The author.
    pub author: ActorId,
    /// Creation time (epoch ms).
    pub created_unix_ms: i64,
    /// The (possibly edited) Markdown body.
    pub body: String,
    /// Whether the message was edited.
    pub edited: bool,
    /// Whether the message was deleted (tombstone; body may be redacted).
    pub deleted: bool,
    /// Honest delivery state (never a faked read receipt).
    pub delivery: DeliveryState,
    /// Reply count, when this message roots a thread.
    #[serde(default)]
    pub reply_count: u32,
}

/// A thread's root + replies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadTimeline {
    /// The space.
    pub space: SpaceId,
    /// The thread id.
    pub thread: ThreadId,
    /// The root message.
    pub root: MessageView,
    /// The replies, ordered.
    pub replies: Vec<MessageView>,
    /// Whether the thread is resolved.
    pub resolved: bool,
}

/// The live document co-edit sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DocumentSessions {
    /// One row per open session.
    pub sessions: Vec<DocumentSession>,
}

/// One document session view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentSession {
    /// The document.
    pub document: DocumentId,
    /// The space it lives in.
    pub space: SpaceId,
    /// Title.
    pub title: String,
    /// Current participants.
    pub participants: Vec<ActorId>,
    /// The call backing the live session, if one is open.
    #[serde(default)]
    pub call: Option<CallId>,
}

/// The files linked into a space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReferences {
    /// The space.
    pub space: SpaceId,
    /// The linked files.
    pub files: Vec<FileReferenceView>,
}

/// One linked-file row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReferenceView {
    /// The stable file-reference id.
    pub file: FileRefId,
    /// The file metadata.
    pub reference: FileRef,
    /// Who linked it.
    pub linked_by: ActorId,
    /// When it was linked (epoch ms).
    pub linked_unix_ms: i64,
}

/// The transfer jobs — a read-side mirror of the WL-FUNC-006 progress ledger
/// (this crate never owns a second progress authority).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TransferJobs {
    /// One row per job.
    pub jobs: Vec<TransferJobView>,
}

/// One transfer-job row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferJobView {
    /// The transfer's control id.
    pub transfer: TransferId,
    /// The file being moved.
    pub file: FileRefId,
    /// Transport.
    pub method: TransferMethod,
    /// Direction.
    pub direction: TransferDirection,
    /// State.
    pub state: TransferState,
    /// Bytes moved so far (mirrored from the ledger).
    pub moved: u64,
    /// Total bytes (mirrored from the ledger; `0` if unknown).
    pub total: u64,
}

/// The global alert inbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AlertInbox {
    /// Newest-first alert rows.
    pub alerts: Vec<AlertView>,
}

/// One alert-inbox row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertView {
    /// The alert event id.
    pub event_id: EventId,
    /// The space it was projected into.
    pub space: SpaceId,
    /// The alert substance.
    pub alert: AlertPayload,
    /// Whether it has been acknowledged.
    pub acknowledged: bool,
    /// The snooze expiry (epoch ms), when snoozed.
    #[serde(default)]
    pub snoozed_until_unix_ms: Option<i64>,
}

/// A space's clipboard lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardLane {
    /// The space.
    pub space: SpaceId,
    /// Newest-first clip rows.
    pub items: Vec<ClipboardView>,
}

/// One clipboard-lane row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardView {
    /// The clip event id.
    pub event_id: EventId,
    /// Text vs URI.
    pub kind: ClipItemKind,
    /// A short preview.
    pub preview: String,
    /// SHA-256 (lower-hex) of the full content.
    pub sha256_hex: String,
    /// The source node.
    pub source: String,
    /// When captured (epoch ms).
    pub at_unix_ms: i64,
    /// Whether pinned.
    pub pinned: bool,
}

/// The presence board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PresenceBoard {
    /// One row per known member.
    pub members: Vec<PresenceView>,
}

/// One presence row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceView {
    /// The member.
    pub actor: ActorId,
    /// Their presence.
    pub presence: PresenceState,
    /// Their free-text status, if any.
    #[serde(default)]
    pub status: Option<String>,
    /// Their node role badge, if any.
    #[serde(default)]
    pub role_badge: Option<String>,
}

/// The active call state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CallState {
    /// One row per active call.
    pub active: Vec<CallView>,
}

/// One active-call row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallView {
    /// The call id.
    pub call: CallId,
    /// The space it is in.
    pub space: SpaceId,
    /// What the call carries.
    pub kind: CallKind,
    /// When it started (epoch ms).
    pub started_unix_ms: i64,
    /// The participants and their states.
    pub participants: Vec<CallParticipantView>,
}

/// One call-participant row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallParticipantView {
    /// The participant.
    pub actor: ActorId,
    /// Their call state.
    pub state: CallParticipantState,
    /// Whether they are muted.
    pub muted: bool,
}

/// The unread/alert badge counters the shell reads for the launcher tile + dock
/// cell (bounded dimensions; a read-side rollup, not a second authority).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CommsBadges {
    /// Total unread across spaces.
    pub unread: u32,
    /// Unacknowledged alerts.
    pub alerts: u32,
    /// The most severe unacknowledged alert.
    #[serde(default)]
    pub top_severity: Option<Severity>,
    /// Active transfer count.
    pub active_transfers: u32,
    /// Active call count.
    pub active_calls: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_model_variants_round_trip() {
        let models = [
            CollabReadModel::SpaceDirectory(SpaceDirectory::default()),
            CollabReadModel::Activity(ActivityFeed::default()),
            CollabReadModel::ConversationTimeline(ConversationTimeline {
                space: SpaceId::new(),
                thread: None,
                messages: Vec::new(),
            }),
            CollabReadModel::DocumentSessions(DocumentSessions::default()),
            CollabReadModel::TransferJobs(TransferJobs::default()),
            CollabReadModel::AlertInbox(AlertInbox::default()),
            CollabReadModel::Presence(PresenceBoard::default()),
            CollabReadModel::CallState(CallState::default()),
        ];
        for m in models {
            let json = serde_json::to_string(&m).expect("serialize");
            let back: CollabReadModel = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(m, back);
        }
    }

    #[test]
    fn badges_default_is_all_zero() {
        let b = CommsBadges::default();
        assert_eq!(b.unread, 0);
        assert_eq!(b.alerts, 0);
        assert!(b.top_severity.is_none());
    }
}
