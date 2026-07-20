//! WL-FUNC-011 — the **Communications** surface, mounted live in the shell.
//!
//! `mde-collab-egui`'s [`CommunicationsSurface`] is a pure UI widget: it renders
//! the [`CollabReadModel`](mde_collab_types) projections through a
//! [`CollabData`] source it is handed and emits typed
//! [`CollabCommand`](mde_collab_types::CollabCommand)s into a [`CommandSink`] the
//! caller drains. This module is the shell-side mount that makes it real on the
//! mesh — the standalone crate carried only a [`FixtureData`](mde_collab_egui) and
//! left the Bus wiring "for a later shell-mount phase". That phase is here:
//!
//!   * [`LiveCollabData`] is the Bus-backed [`CollabData`]. Each refresh folds the
//!     collab worker's retained `state/collab/*` mirrors (the `directory` rail, and
//!     — per space in that directory — the `activity` feed, the `conversation`
//!     timeline, and the `call-state`) into the owned projection shapes the surface
//!     reads. It is a **pure renderer** over the worker's read-model: the shell
//!     never depends on the mackesd collab worker crate — the Bus JSON is the seam
//!     (the same discipline as `chat.rs`).
//!   * [`CommunicationsState`] owns the surface + the data source, refreshes the
//!     fold on a poll cadence while in view, and drains the surface's emitted
//!     commands onto `action/collab/<verb>` ([`topics::command_topic_for`]) so the
//!     collab worker applies them.
//!
//! Activity + Messages are live (the surface implements them in full); the
//! labeled-for-later modes stay labeled — no faked data (§7). Live multi-node
//! delivery is the worker's job; this mount is the read-fold + command-publish
//! seam, headless-testable against a tempdir [`Persist`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_egui::egui;
use serde::de::DeserializeOwned;

use mde_collab_egui::{CollabData, CommandSink, CommunicationsSurface};
use mde_collab_types::topics::{self, projection as proj};
use mde_collab_types::{
    ActivityFeed, ActorId, CallState, CollabCommand, ConversationTimeline, SpaceDirectory, SpaceId,
    ThreadId, ThreadTimeline,
};

use crate::bus_reader::BusReader;

/// Poll cadence — matches the collab worker's own 2 s tick so the rail +
/// conversations stay live without a cold-start wait (the `chat.rs` cadence).
const REFRESH: Duration = Duration::from_secs(2);

/// The local seat's wall time in epoch milliseconds (the collab worker's
/// `now_unix_ms` shape). Injected into [`CollabData::now_unix_ms`] so the surface
/// evaluates the message edit/delete window + relative ages against a real clock.
fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// The newest (latest-wins) body retained on `topic`, decoded into `T`. `None`
/// when the topic carries no message or the body won't decode — the honest
/// pre-projection state, never a fake (§7).
fn read_state<T: DeserializeOwned>(persist: &Persist, topic: &str) -> Option<T> {
    let msg = persist.read_latest(topic).ok().flatten()?;
    serde_json::from_str(&msg.body?).ok()
}

/// The Bus-backed [`CollabData`] the Communications surface reads.
///
/// Owns the folded projection shapes (so the trait can hand out `&` references,
/// the same shape [`FixtureData`](mde_collab_egui) has) and rebuilds them from the
/// retained `state/collab/*` mirrors on [`refresh`](Self::refresh). The worker
/// publishes each projection latest-wins; this is the surface's window onto that
/// read side.
pub(crate) struct LiveCollabData {
    /// The shared fail-soft Bus-reader seam (holds the resolved spool path).
    reader: BusReader,
    /// This node's collaboration identity — the bare hostname, matching the
    /// collab worker's `self_host` (so "my message" alignment + the author-scoped
    /// edit affordance resolve against the same actor).
    me: ActorId,
    /// The injected wall time, refreshed each fold.
    now_unix_ms: i64,
    /// The rail directory (folded from `state/collab/directory`).
    directory: SpaceDirectory,
    /// Per-space Activity feeds, keyed `Some(space)` to match the surface's
    /// `data.activity(self.selected_space())` read (folded from
    /// `state/collab/activity/<space>`).
    activity: HashMap<Option<SpaceId>, ActivityFeed>,
    /// Per-space conversation timelines (folded from
    /// `state/collab/conversation/<space>`).
    conversations: HashMap<SpaceId, ConversationTimeline>,
    /// The aggregated active-call state — every space's `state/collab/call-state`
    /// concatenated into the one persistent call bar's read model.
    call_state: CallState,
    /// The last fold time; the poll self-throttles to [`REFRESH`].
    last_poll: Option<Instant>,
}

impl LiveCollabData {
    /// A fresh source over `bus_root` (the desktop-client spool). No projections
    /// yet — the first [`refresh`](Self::refresh) folds them.
    fn new(bus_root: Option<PathBuf>) -> Self {
        Self {
            reader: BusReader::new(bus_root),
            me: ActorId::new(crate::explorer::local_hostname()),
            now_unix_ms: now_unix_ms(),
            directory: SpaceDirectory::default(),
            activity: HashMap::new(),
            conversations: HashMap::new(),
            call_state: CallState::default(),
            last_poll: None,
        }
    }

    /// Re-fold on the [`REFRESH`] cadence while the surface is in view, and keep
    /// the frame loop ticking so a worker republish surfaces without operator
    /// input (the `chat.rs` poll shape).
    fn poll(&mut self, ctx: &egui::Context) {
        if self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH) {
            self.last_poll = Some(Instant::now());
            self.refresh();
            ctx.request_repaint_after(REFRESH);
        }
    }

    /// Fold the retained `state/collab/*` mirrors into the owned projections. Opens
    /// the spool fail-soft: no spool / an unopenable store clears to the honest
    /// off-mesh empty state (§7). The `directory` names the spaces; each space's
    /// per-space projections are then read off the one open handle.
    fn refresh(&mut self) {
        self.now_unix_ms = now_unix_ms();
        let Some(persist) = self.reader.open() else {
            self.directory = SpaceDirectory::default();
            self.activity.clear();
            self.conversations.clear();
            self.call_state = CallState::default();
            return;
        };

        self.directory =
            read_state(&persist, &topics::state_topic(proj::SPACE_DIRECTORY)).unwrap_or_default();

        let mut activity = HashMap::new();
        let mut conversations = HashMap::new();
        let mut call_state = CallState::default();
        for summary in &self.directory.spaces {
            let space = summary.id;
            if let Some(feed) = read_state::<ActivityFeed>(
                &persist,
                &topics::space_state_topic(proj::ACTIVITY, space),
            ) {
                activity.insert(Some(space), feed);
            }
            if let Some(convo) = read_state::<ConversationTimeline>(
                &persist,
                &topics::space_state_topic(proj::CONVERSATION, space),
            ) {
                conversations.insert(space, convo);
            }
            if let Some(calls) = read_state::<CallState>(
                &persist,
                &topics::space_state_topic(proj::CALL_STATE, space),
            ) {
                // The trait exposes one aggregate CallState (the call bar's read
                // model); the worker publishes it per space, so concatenate.
                call_state.active.extend(calls.active);
            }
        }
        self.activity = activity;
        self.conversations = conversations;
        self.call_state = call_state;
    }
}

impl CollabData for LiveCollabData {
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

    fn thread(&self, _space: SpaceId, _thread: ThreadId) -> Option<&ThreadTimeline> {
        // The collab worker does not (yet) republish per-thread timelines; the
        // surface's "N replies" affordance stays closed honestly until it does.
        None
    }

    fn call_state(&self) -> &CallState {
        &self.call_state
    }
}

/// The shell-side mount of the Communications surface: the widget + its live data
/// source + the publish seam that routes emitted commands onto `action/collab/*`.
pub(crate) struct CommunicationsState {
    /// The pure `mde-collab-egui` widget (owns only view state).
    surface: CommunicationsSurface,
    /// The Bus-backed projection source the widget renders.
    data: LiveCollabData,
    /// The resolved spool path commands are published through (kept alongside the
    /// reader's copy because publishing needs the open/write error text; the
    /// fail-soft `BusReader` swallows it).
    bus_root: Option<PathBuf>,
}

impl Default for CommunicationsState {
    /// Resolve the desktop-client spool via the canonical GUI resolution
    /// ([`mde_bus::client_data_dir`]), exactly like `ChatState::default`.
    fn default() -> Self {
        Self::new(mde_bus::client_data_dir())
    }
}

impl CommunicationsState {
    /// A fresh mount over `bus_root`.
    fn new(bus_root: Option<PathBuf>) -> Self {
        Self {
            surface: CommunicationsSurface::new(),
            data: LiveCollabData::new(bus_root.clone()),
            bus_root,
        }
    }

    /// Re-fold the `state/collab/*` mirrors on the poll cadence (the shell calls
    /// this while Communications is the surface in view).
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        self.data.poll(ctx);
    }

    /// Render the surface and route the frame's emitted commands. The widget reads
    /// [`self.data`](LiveCollabData) and pushes intent into a per-frame
    /// [`CommandSink`]; this drains the sink and publishes each command onto
    /// `action/collab/<verb>` so the collab worker applies it.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        let mut sink = CommandSink::new();
        self.surface.ui(ui, &self.data, &mut sink);
        drain_to_bus(&mut sink, self.bus_root.as_deref());
    }
}

/// Drain every command the surface emitted this frame onto `action/collab/*`. A
/// publish failure is logged (visible) and dropped — never a silent swallow, and
/// never a faked local apply (the worker is the one authority).
fn drain_to_bus(sink: &mut CommandSink, bus_root: Option<&Path>) {
    for command in sink.drain() {
        let topic = topics::command_topic_for(&command);
        if let Err(e) = publish_command(bus_root, &topic, &command) {
            tracing::debug!(
                target: "shell::communications",
                verb = command.verb(),
                error = %e,
                "collab command publish failed",
            );
        }
    }
}

/// Publish one [`CollabCommand`] on `topic` (`action/collab/<verb>`) through the
/// persist-first Bus path. Mirrors `chat.rs`'s `publish`: the writer opens its own
/// `Persist` (not the fail-soft `BusReader`) because it needs the error text.
fn publish_command(
    bus_root: Option<&Path>,
    topic: &str,
    command: &CollabCommand,
) -> Result<(), String> {
    let Some(root) = bus_root else {
        return Err("No local Bus — the mesh daemon may be down.".to_string());
    };
    let body =
        serde_json::to_string(command).map_err(|e| format!("serialize collab command: {e}"))?;
    let persist = Persist::open(root.to_path_buf())
        .map_err(|e| format!("Couldn't open the local Bus: {e}"))?;
    persist
        .write(topic, Priority::Default, None, Some(&body))
        .map_err(|e| format!("Bus write failed: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use mde_collab_types::value::{CallKind, DeliveryState, MessageBody};
    use mde_collab_types::{
        ActivityEntry, ActorClock, CallParticipantState, CallParticipantView, CallView, EventId,
        MessageView, SpaceKind, SpaceRole, SpaceSummary,
    };

    fn persist_at(root: &Path) -> Persist {
        Persist::open(root.to_path_buf()).expect("open persist")
    }

    /// Write a `state/collab/*` retained mirror as the worker would.
    fn write_state<T: serde::Serialize>(persist: &Persist, topic: &str, model: &T) {
        let body = serde_json::to_string(model).expect("serialize model");
        persist
            .write(topic, Priority::Default, None, Some(&body))
            .expect("write state");
    }

    fn space_summary(id: SpaceId, name: &str) -> SpaceSummary {
        SpaceSummary {
            id,
            kind: SpaceKind::Team,
            name: name.to_owned(),
            role: SpaceRole::Owner,
            unread: 0,
            members: 2,
            last_activity: ActorClock::at(1_000, 0),
        }
    }

    fn message(author: &ActorId, body: &str) -> MessageView {
        MessageView {
            event_id: EventId::new(),
            author: author.clone(),
            created_unix_ms: 1_000,
            body: body.to_owned(),
            edited: false,
            deleted: false,
            delivery: DeliveryState::Sent,
            reply_count: 0,
        }
    }

    #[test]
    fn live_collab_data_folds_state_collab_mirrors_into_the_projections() {
        // A fixture set of `state/collab/*` mirror rows — the directory plus one
        // space's Activity, conversation, and call-state — folds into the exact
        // projections the surface reads.
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());

        let ops = SpaceId::new();
        let me = ActorId::new("eagle");
        let peer = ActorId::new("falcon");

        write_state(
            &persist,
            &topics::state_topic(proj::SPACE_DIRECTORY),
            &SpaceDirectory {
                spaces: vec![space_summary(ops, "Team Ops")],
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::CONVERSATION, ops),
            &ConversationTimeline {
                space: ops,
                thread: None,
                messages: vec![
                    message(&peer, "deploy is green"),
                    message(&me, "shipped the rail"),
                ],
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::ACTIVITY, ops),
            &ActivityFeed {
                space: Some(ops),
                entries: vec![ActivityEntry {
                    event_id: EventId::new(),
                    space: ops,
                    actor: peer.clone(),
                    clock: ActorClock::at(1_000, 0),
                    created_unix_ms: 1_000,
                    kind_tag: "message_posted".to_owned(),
                    summary: "posted a message".to_owned(),
                }],
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::CALL_STATE, ops),
            &CallState {
                active: vec![CallView {
                    call: mde_collab_types::CallId::new(),
                    space: ops,
                    kind: CallKind::Audio,
                    started_unix_ms: 1_000,
                    participants: vec![CallParticipantView {
                        actor: me.clone(),
                        state: CallParticipantState::Connected,
                        muted: false,
                    }],
                }],
            },
        );

        let mut data = LiveCollabData::new(Some(dir.path().to_path_buf()));
        data.refresh();

        // Directory folded — the rail row is present.
        assert_eq!(data.space_directory().spaces.len(), 1, "directory folded");
        assert_eq!(data.space_directory().spaces[0].id, ops);
        assert_eq!(data.space_directory().spaces[0].name, "Team Ops");

        // Conversation folded under its space, in order.
        let convo = data.conversation(ops).expect("conversation folded");
        assert_eq!(convo.messages.len(), 2);
        assert_eq!(convo.messages[0].body, "deploy is green");
        assert_eq!(convo.messages[1].author, me);

        // Activity folded, keyed Some(space) as the surface reads it.
        let feed = data.activity(Some(ops)).expect("activity folded");
        assert_eq!(feed.entries.len(), 1);
        assert_eq!(feed.entries[0].kind_tag, "message_posted");

        // Per-space call-state aggregated into the one call-bar read model.
        assert_eq!(data.call_state().active.len(), 1, "call-state aggregated");
        assert_eq!(data.call_state().active[0].space, ops);
    }

    #[test]
    fn no_spool_folds_to_the_honest_empty_state() {
        // No configured spool → the honest off-mesh empty projections, never a
        // panic and never faked data (§7).
        let mut data = LiveCollabData::new(None);
        data.refresh();
        assert!(data.space_directory().spaces.is_empty());
        assert!(data.activity(None).is_none());
        assert!(data.call_state().active.is_empty());
    }

    #[test]
    fn a_send_message_command_publishes_to_action_collab_send() {
        // A surface-emitted SendMessage (recorded in the CommandSink exactly as the
        // composer's Enter does) drains onto `action/collab/send` with a body that
        // round-trips back to the same typed command — the publish seam.
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());
        let ops = SpaceId::new();

        let mut sink = CommandSink::new();
        sink.emit(CollabCommand::SendMessage {
            space: ops,
            thread: None,
            body: MessageBody::new("hello **mesh**"),
        });

        drain_to_bus(&mut sink, Some(dir.path()));
        assert!(sink.is_empty(), "the sink was drained");

        // The command landed on the canonical `action/collab/send` topic.
        let topic = topics::command_topic("send_message");
        assert_eq!(topic, "action/collab/send_message");
        let published = persist
            .read_latest(&topic)
            .expect("read command")
            .expect("command published");
        let back: CollabCommand =
            serde_json::from_str(published.body.as_deref().expect("command body"))
                .expect("decode command");
        assert_eq!(
            back,
            CollabCommand::SendMessage {
                space: ops,
                thread: None,
                body: MessageBody::new("hello **mesh**"),
            },
            "the published body is the emitted SendMessage",
        );
    }

    #[test]
    fn publish_without_a_spool_is_a_visible_error_not_a_panic() {
        // No spool → a typed Err (logged by the drain), never a panic or a faked
        // local apply.
        let err = publish_command(
            None,
            &topics::command_topic("send_message"),
            &CollabCommand::LeaveSpace {
                space: SpaceId::new(),
            },
        )
        .expect_err("no spool must be an error");
        assert!(err.contains("No local Bus"), "explains the down mesh");
    }
}
