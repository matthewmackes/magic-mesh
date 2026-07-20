//! [`FixtureData`] — an in-memory [`CollabData`] implementation that owns a set
//! of projections, for the headless tests and a future demo mount.
//!
//! This is the stand-in for the real `BusReader`-backed data source (a later
//! shell-mount phase): it holds owned [`CollabReadModel`](mde_collab_types) shapes
//! and hands out references to them, so the surface renders and emits exactly as
//! it will against the live read side — without a Bus, a worker, or a clock.

use std::collections::HashMap;

use mde_collab_types::{
    ActivityEntry, ActivityFeed, ActorClock, ActorId, CallKind, CallParticipantState,
    CallParticipantView, CallState, CallView, ConversationTimeline, DeliveryState, EventId,
    FileReferences, MessageView, SpaceDirectory, SpaceId, SpaceKind, SpaceRole, SpaceSummary,
    ThreadId, ThreadTimeline, TransferJobs,
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
    threads: HashMap<ThreadId, ThreadTimeline>,
    thread_roots: HashMap<EventId, ThreadId>,
    call_state: CallState,
    file_references: HashMap<SpaceId, FileReferences>,
    transfer_jobs: TransferJobs,
}

impl FixtureData {
    /// A fixture with the local seat `me` and the injected `now_unix_ms`, no
    /// spaces or projections yet — build them up with the `with_*` methods.
    #[must_use]
    pub fn new(me: impl Into<ActorId>, now_unix_ms: i64) -> Self {
        Self {
            me: me.into(),
            now_unix_ms,
            directory: SpaceDirectory::default(),
            activity: HashMap::new(),
            conversations: HashMap::new(),
            threads: HashMap::new(),
            thread_roots: HashMap::new(),
            call_state: CallState::default(),
            file_references: HashMap::new(),
            transfer_jobs: TransferJobs::default(),
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

    /// Add a thread timeline and index it by the message `root` it hangs off, so
    /// [`thread_for_root`](CollabData::thread_for_root) resolves the "N replies"
    /// affordance.
    #[must_use]
    pub fn with_thread(mut self, root: EventId, timeline: ThreadTimeline) -> Self {
        self.thread_roots.insert(root, timeline.thread);
        self.threads.insert(timeline.thread, timeline);
        self
    }

    /// Add an active call to the call bar's read model.
    #[must_use]
    pub fn with_call(mut self, call: CallView) -> Self {
        self.call_state.active.push(call);
        self
    }

    /// Set a space's linked-file references (the Files mode's read model).
    #[must_use]
    pub fn with_file_references(mut self, refs: FileReferences) -> Self {
        self.file_references.insert(refs.space, refs);
        self
    }

    /// Set the transfer-jobs mirror (the read-side of the WL-FUNC-006 ledger the
    /// Files mode's transfer controls read state from).
    #[must_use]
    pub fn with_transfer_jobs(mut self, jobs: TransferJobs) -> Self {
        self.transfer_jobs = jobs;
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

    fn thread(&self, space: SpaceId, thread: ThreadId) -> Option<&ThreadTimeline> {
        self.threads.get(&thread).filter(|t| t.space == space)
    }

    fn thread_for_root(&self, _space: SpaceId, root: EventId) -> Option<ThreadId> {
        self.thread_roots.get(&root).copied()
    }

    fn call_state(&self) -> &CallState {
        &self.call_state
    }

    fn file_references(&self, space: SpaceId) -> Option<&FileReferences> {
        self.file_references.get(&space)
    }

    fn transfer_jobs(&self) -> Option<&TransferJobs> {
        Some(&self.transfer_jobs)
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
