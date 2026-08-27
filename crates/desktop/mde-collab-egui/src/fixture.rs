//! [`FixtureData`] — an in-memory [`CollabData`] implementation that owns a set
//! of projections, for the headless tests and a future demo mount.
//!
//! This is the stand-in for the real `BusReader`-backed data source (a later
//! shell-mount phase): it holds owned [`CollabReadModel`](mde_collab_types) shapes
//! and hands out references to them, so the surface renders and emits exactly as
//! it will against the live read side — without a Bus, a worker, or a clock.

use std::collections::HashMap;

use mde_collab_types::{
    ActivityEntry, ActivityFeed, ActorClock, ActorId, AlertInbox, CallKind, CallParticipantState,
    CallParticipantView, CallState, CallView, ChannelTasks, ClipboardLane, ConversationTimeline,
    DeliveryState, DiscordBridgeBoard, DocumentId, DocumentSession, DocumentSessions, EventId,
    FileReferences, MediaSessionV1, MessagePins, MessageView, SavedMessages, SpaceDirectory,
    SpaceId, SpaceKind, SpaceRole, SpaceSummary, ThreadId, ThreadTimeline, TransferJobs,
};

use crate::CollabData;

/// An owned, in-memory [`CollabData`] source for tests and demos.
#[derive(Debug, Clone)]
pub struct FixtureData {
    me: ActorId,
    now_unix_ms: i64,
    directory: SpaceDirectory,
    activity: HashMap<Option<SpaceId>, ActivityFeed>,
    conversations: HashMap<SpaceId, ConversationTimeline>,
    message_pins: HashMap<SpaceId, MessagePins>,
    saved_messages: SavedMessages,
    threads: HashMap<ThreadId, ThreadTimeline>,
    thread_roots: HashMap<EventId, ThreadId>,
    channel_tasks: HashMap<SpaceId, ChannelTasks>,
    call_state: CallState,
    media_sessions: Vec<MediaSessionV1>,
    file_references: HashMap<SpaceId, FileReferences>,
    transfer_jobs: TransferJobs,
    alert_inbox: AlertInbox,
    clipboard_lanes: HashMap<SpaceId, ClipboardLane>,
    document_sessions: HashMap<SpaceId, DocumentSessions>,
    document_bodies: HashMap<DocumentId, String>,
    discord_bridge_board: Option<DiscordBridgeBoard>,
}

impl FixtureData {
    /// A fixture with the local seat `me` and the injected `now_unix_ms`, no
    /// spaces or projections yet — build them up with the `with_*` methods.
    #[must_use]
    pub fn new(me: impl Into<ActorId>, now_unix_ms: i64) -> Self {
        let me = me.into();
        Self {
            me: me.clone(),
            now_unix_ms,
            directory: SpaceDirectory::default(),
            activity: HashMap::new(),
            conversations: HashMap::new(),
            message_pins: HashMap::new(),
            saved_messages: SavedMessages {
                actor: me,
                messages: Vec::new(),
            },
            threads: HashMap::new(),
            thread_roots: HashMap::new(),
            channel_tasks: HashMap::new(),
            call_state: CallState::default(),
            media_sessions: Vec::new(),
            file_references: HashMap::new(),
            transfer_jobs: TransferJobs::default(),
            alert_inbox: AlertInbox::default(),
            clipboard_lanes: HashMap::new(),
            document_sessions: HashMap::new(),
            document_bodies: HashMap::new(),
            discord_bridge_board: None,
        }
    }

    /// Add a rail space.
    #[must_use]
    pub fn with_space(mut self, summary: SpaceSummary) -> Self {
        self.directory.spaces.push(summary);
        self
    }

    /// Set the Activity feed for `space` (`None` = the cross-space feed).
    #[must_use]
    pub fn with_activity(mut self, space: Option<SpaceId>, feed: ActivityFeed) -> Self {
        self.activity.insert(space, feed);
        self
    }

    /// Set the main conversation timeline (keyed by its own `space`).
    #[must_use]
    pub fn with_conversation(mut self, timeline: ConversationTimeline) -> Self {
        self.conversations.insert(timeline.space, timeline);
        self
    }

    /// Set the shared message pins for one space.
    #[must_use]
    pub fn with_message_pins(mut self, pins: MessagePins) -> Self {
        self.message_pins.insert(pins.space, pins);
        self
    }

    /// Set the local actor's private saved-message projection.
    #[must_use]
    pub fn with_saved_messages(mut self, saved: SavedMessages) -> Self {
        self.saved_messages = saved;
        self
    }

    /// Add a thread timeline and index it by the message `root` it hangs off, so
    /// [`thread_for_root`](CollabData::thread_for_root) resolves the "N replies"
    /// affordance.
    #[must_use]
    pub fn with_thread(mut self, root: EventId, timeline: ThreadTimeline) -> Self {
        self.thread_roots.insert(root, timeline.thread);
        self.threads.insert(timeline.thread, timeline);
        self
    }

    /// Set a space's basic channel tasks/action-items read model.
    #[must_use]
    pub fn with_channel_tasks(mut self, tasks: ChannelTasks) -> Self {
        self.channel_tasks.insert(tasks.space, tasks);
        self
    }

    /// Add an active call to the call bar's read model.
    #[must_use]
    pub fn with_call(mut self, call: CallView) -> Self {
        self.call_state.active.push(call);
        self
    }

    /// Retain published [`MediaSessionV1`] documents as-is. The fixture never
    /// synthesizes a Connected session from [`CallState`].
    #[must_use]
    pub fn with_media_sessions(mut self, sessions: Vec<MediaSessionV1>) -> Self {
        self.media_sessions = sessions;
        self
    }

    /// Set a space's linked-file references (the Files mode's read model).
    #[must_use]
    pub fn with_file_references(mut self, refs: FileReferences) -> Self {
        self.file_references.insert(refs.space, refs);
        self
    }

    /// Set the transfer-jobs mirror (the read-side of the WL-FUNC-006 ledger the
    /// Files + Transfers modes read state from).
    #[must_use]
    pub fn with_transfer_jobs(mut self, jobs: TransferJobs) -> Self {
        self.transfer_jobs = jobs;
        self
    }

    /// Set the fleet-wide alert inbox (the Alerts mode's read model).
    #[must_use]
    pub fn with_alert_inbox(mut self, inbox: AlertInbox) -> Self {
        self.alert_inbox = inbox;
        self
    }

    /// Set a space's clipboard lane (the Clipboard mode's read model, keyed by
    /// its own `space`).
    #[must_use]
    pub fn with_clipboard_lane(mut self, lane: ClipboardLane) -> Self {
        self.clipboard_lanes.insert(lane.space, lane);
        self
    }

    /// Set a space's live document co-edit sessions (the Documents mode's picker
    /// read model, keyed by `space`).
    #[must_use]
    pub fn with_document_sessions(mut self, space: SpaceId, sessions: DocumentSessions) -> Self {
        self.document_sessions.insert(space, sessions);
        self
    }

    /// Set the resolved canonical Markdown body for `document` — the bytes the
    /// shell's content-addressed blob store would resolve a document's payload to,
    /// so a test's "open this session and it loads" is real, never faked.
    #[must_use]
    pub fn with_document_body(mut self, document: DocumentId, body: impl Into<String>) -> Self {
        self.document_bodies.insert(document, body.into());
        self
    }

    /// One space, one live document session row, and a resolved Markdown body.
    /// Two-seat share/join/follow/close tests build both seats from this so
    /// membership and the picker row are real, never faked. View/edit permission
    /// is not a projection field — the live collab session Access is the authority.
    #[must_use]
    pub fn document_share(
        me: &str,
        space: SpaceId,
        document: DocumentId,
        participants: &[&str],
        body: &str,
    ) -> Self {
        Self::document_share_with_role(me, space, document, participants, body, SpaceRole::Member)
    }

    /// Same as [`Self::document_share`] with an explicit directory role so
    /// owner-vs-member share/join tests do not invent a second fixture shape.
    #[must_use]
    pub fn document_share_with_role(
        me: &str,
        space: SpaceId,
        document: DocumentId,
        participants: &[&str],
        body: &str,
        role: SpaceRole,
    ) -> Self {
        Self::new(me, 1_000)
            .with_space(space_summary(
                space,
                SpaceKind::Project,
                "Docs",
                role,
                0,
                participants.len() as u32,
                1_000,
            ))
            .with_document_sessions(
                space,
                DocumentSessions {
                    sessions: vec![DocumentSession {
                        document,
                        space,
                        title: "Runbook".to_owned(),
                        participants: participants.iter().map(|id| ActorId::new(*id)).collect(),
                        call: None,
                    }],
                },
            )
            .with_document_body(document, body)
    }

    /// Set the Discord bridge status board. Tests use this to exercise the
    /// read-only UI seam; no Discord provider is called and no server is
    /// fabricated by default.
    #[must_use]
    pub fn with_discord_bridge_board(mut self, board: DiscordBridgeBoard) -> Self {
        self.discord_bridge_board = Some(board);
        self
    }

    /// A realistic small dataset for a demo mount and the frame-render tests: two
    /// spaces, an Activity feed spanning several bands, a conversation with the
    /// seat's own fresh message plus a peer's, an anchored thread, and one active
    /// call — all wired to the first space so the surface's default selection
    /// lands on populated panes.
    #[must_use]
    pub fn demo() -> Self {
        let me = ActorId::new("eagle");
        let peer = ActorId::new("falcon");
        let now = 1_000_000;

        let ops = SpaceId::new();
        let incident = SpaceId::new();

        let root_id = EventId::new();
        let thread = ThreadId::new();

        let conversation = ConversationTimeline {
            space: ops,
            thread: None,
            messages: vec![
                message(
                    EventId::new(),
                    &peer,
                    now - 600_000,
                    "Morning — deploy is green.",
                    DeliveryState::Delivered,
                    0,
                ),
                message(
                    root_id,
                    &me,
                    now - 60_000,
                    "## Standup\n- shipped the rail\n- threads next",
                    DeliveryState::Sent,
                    2,
                ),
                message(
                    EventId::new(),
                    &peer,
                    now - 20_000,
                    "Nice. Queued a review.",
                    DeliveryState::Queued,
                    0,
                ),
            ],
        };

        let thread_timeline = ThreadTimeline {
            space: ops,
            thread,
            root: message(
                root_id,
                &me,
                now - 60_000,
                "Threads next",
                DeliveryState::Sent,
                2,
            ),
            replies: vec![
                message(
                    EventId::new(),
                    &peer,
                    now - 40_000,
                    "Anchored under the root?",
                    DeliveryState::Delivered,
                    0,
                ),
                message(
                    EventId::new(),
                    &me,
                    now - 30_000,
                    "Yes — right column.",
                    DeliveryState::Sent,
                    0,
                ),
            ],
            resolved: false,
        };

        let feed = ActivityFeed {
            space: Some(ops),
            entries: vec![
                activity(
                    EventId::new(),
                    ops,
                    &peer,
                    now - 600_000,
                    "message_posted",
                    "posted a message",
                ),
                activity(
                    EventId::new(),
                    ops,
                    &me,
                    now - 300_000,
                    "file_linked",
                    "linked deploy.log",
                ),
                activity(
                    EventId::new(),
                    ops,
                    &peer,
                    now - 120_000,
                    "alert_raised",
                    "raised a warning",
                ),
                activity(
                    EventId::new(),
                    ops,
                    &me,
                    now - 60_000,
                    "call_started",
                    "started an audio call",
                ),
                activity(
                    EventId::new(),
                    ops,
                    &peer,
                    now - 30_000,
                    "member_joined",
                    "joined the space",
                ),
            ],
        };

        let call = CallView {
            call: mde_collab_types::CallId::new(),
            space: ops,
            kind: CallKind::Audio,
            started_unix_ms: now - 60_000,
            participants: vec![
                CallParticipantView {
                    actor: me.clone(),
                    state: CallParticipantState::Connected,
                    muted: false,
                },
                CallParticipantView {
                    actor: peer.clone(),
                    state: CallParticipantState::Connected,
                    muted: true,
                },
            ],
        };

        Self::new(me, now)
            .with_space(space_summary(
                ops,
                SpaceKind::Team,
                "Team Ops",
                SpaceRole::Owner,
                3,
                4,
                now - 20_000,
            ))
            .with_space(space_summary(
                incident,
                SpaceKind::Incident,
                "Incident 42",
                SpaceRole::Member,
                0,
                6,
                now - 900_000,
            ))
            .with_conversation(conversation)
            .with_thread(root_id, thread_timeline)
            .with_activity(Some(ops), feed)
            .with_call(call)
    }
}

impl CollabData for FixtureData {
    fn me(&self) -> &ActorId {
        &self.me
    }

    fn now_unix_ms(&self) -> i64 {
        self.now_unix_ms
    }

    fn space_directory(&self) -> &SpaceDirectory {
        &self.directory
    }

    fn activity(&self, space: Option<SpaceId>) -> Option<&ActivityFeed> {
        self.activity.get(&space)
    }

    fn conversation(&self, space: SpaceId) -> Option<&ConversationTimeline> {
        self.conversations.get(&space)
    }

    fn message_pinned(&self, space: SpaceId, message: EventId) -> bool {
        self.message_pins
            .get(&space)
            .is_some_and(|pins| pins.messages.contains(&message))
    }

    fn message_saved(&self, space: SpaceId, message: EventId) -> bool {
        self.saved_messages.actor == self.me
            && self
                .saved_messages
                .messages
                .iter()
                .any(|saved| saved.space == space && saved.message == message)
    }

    fn thread(&self, space: SpaceId, thread: ThreadId) -> Option<&ThreadTimeline> {
        self.threads.get(&thread).filter(|t| t.space == space)
    }

    fn thread_for_root(&self, _space: SpaceId, root: EventId) -> Option<ThreadId> {
        self.thread_roots.get(&root).copied()
    }

    fn channel_tasks(&self, space: SpaceId) -> Option<&ChannelTasks> {
        self.channel_tasks.get(&space)
    }

    fn call_state(&self) -> &CallState {
        &self.call_state
    }

    fn media_sessions(&self) -> &[MediaSessionV1] {
        &self.media_sessions
    }

    fn file_references(&self, space: SpaceId) -> Option<&FileReferences> {
        self.file_references.get(&space)
    }

    fn transfer_jobs(&self) -> Option<&TransferJobs> {
        Some(&self.transfer_jobs)
    }

    fn alert_inbox(&self) -> Option<&AlertInbox> {
        Some(&self.alert_inbox)
    }

    fn clipboard_lane(&self, space: SpaceId) -> Option<&ClipboardLane> {
        self.clipboard_lanes.get(&space)
    }

    fn document_sessions(&self, space: SpaceId) -> Option<&DocumentSessions> {
        self.document_sessions.get(&space)
    }

    fn document_body(&self, document: DocumentId) -> Option<&str> {
        self.document_bodies.get(&document).map(String::as_str)
    }

    fn discord_bridge_board(&self) -> Option<&DiscordBridgeBoard> {
        self.discord_bridge_board.as_ref()
    }
}

/// Build a [`SpaceSummary`] rail row.
#[must_use]
pub fn space_summary(
    id: SpaceId,
    kind: SpaceKind,
    name: &str,
    role: SpaceRole,
    unread: u32,
    members: u32,
    last_activity_ms: i64,
) -> SpaceSummary {
    SpaceSummary {
        id,
        kind,
        name: name.to_owned(),
        role,
        unread,
        members,
        last_activity: ActorClock::at(last_activity_ms.max(0) as u64, 0),
    }
}

/// Build a [`MessageView`].
#[must_use]
pub fn message(
    event_id: EventId,
    author: &ActorId,
    created_unix_ms: i64,
    body: &str,
    delivery: DeliveryState,
    reply_count: u32,
) -> MessageView {
    MessageView {
        event_id,
        author: author.clone(),
        created_unix_ms,
        body: body.to_owned(),
        edited: false,
        deleted: false,
        delivery,
        reply_count,
    }
}

/// Build an [`ActivityEntry`].
#[must_use]
pub fn activity(
    event_id: EventId,
    space: SpaceId,
    actor: &ActorId,
    created_unix_ms: i64,
    kind_tag: &str,
    summary: &str,
) -> ActivityEntry {
    ActivityEntry {
        event_id,
        space,
        actor: actor.clone(),
        clock: ActorClock::at(created_unix_ms.max(0) as u64, 0),
        created_unix_ms,
        kind_tag: kind_tag.to_owned(),
        summary: summary.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_collab_types::{
        CallId, CallMediaAdapter, MediaSessionStateV1, MediaSessionV1, MediaTrackKind,
    };

    fn device_absent_session() -> MediaSessionV1 {
        MediaSessionV1::new(
            CallId::new(),
            SpaceId::new(),
            ActorId::new("eagle"),
            ActorId::new("falcon"),
            CallMediaAdapter::WebRtcP2p,
            MediaSessionStateV1::DeviceAbsent {
                track: MediaTrackKind::Audio,
            },
            vec![MediaTrackKind::Audio],
            false,
            false,
            false,
            0,
            None,
            None,
        )
        .expect("valid device-absent session")
    }

    #[test]
    fn document_share_with_role_records_the_directory_role() {
        use mde_collab_types::{DocumentId, SpaceId, SpaceRole};

        let space = SpaceId::new();
        let document = DocumentId::new();
        let owner = FixtureData::document_share_with_role(
            "eagle",
            space,
            document,
            &["eagle", "falcon"],
            "# Runbook\n",
            SpaceRole::Owner,
        );
        assert_eq!(owner.space_directory().spaces[0].role, SpaceRole::Owner);
        assert_eq!(
            FixtureData::document_share(
                "falcon",
                space,
                document,
                &["eagle", "falcon"],
                "# Runbook\n"
            )
            .space_directory()
            .spaces[0]
                .role,
            SpaceRole::Member,
            "the default share fixture stays a member row"
        );
    }

    #[test]
    fn media_sessions_returns_retained_documents_without_inventing_connected() {
        let demo = FixtureData::demo();
        assert!(
            !demo.call_state().active.is_empty(),
            "demo fixture carries a signaling call so a fake Connected session would have a target"
        );
        assert!(
            demo.media_sessions().is_empty(),
            "signaling CallState must not become a MediaSessionV1"
        );
        assert!(
            demo.media_sessions()
                .iter()
                .all(|session| !session.state.claims_live_media()),
            "default fixture must not invent a connected media session"
        );

        let session = device_absent_session();
        let data = demo.with_media_sessions(vec![session.clone()]);
        assert_eq!(data.media_sessions(), std::slice::from_ref(&session));
        assert!(
            !data.media_sessions()[0].state.claims_live_media(),
            "a published DeviceAbsent document stays DeviceAbsent"
        );
    }
}
