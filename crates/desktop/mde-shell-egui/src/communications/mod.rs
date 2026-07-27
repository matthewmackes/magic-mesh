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
//!     collab worker's retained `state/collab/*` mirrors into the owned projection
//!     shapes the surface reads. The heavy per-space mirrors (Activity,
//!     conversation, threads, files, clipboard, and document sessions) are folded
//!     for the focused channel instead of every channel on first open; fleet-wide
//!     rollups and the call bar still fold globally. It is a **pure renderer** over
//!     the worker's read-model: the shell never depends on the mackesd collab
//!     worker crate — the Bus JSON is the seam (the same discipline as `chat.rs`).
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

use mackes_mesh_types::cloud::CLOUD_ACTION_SCHEMA_VERSION;
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_egui::egui;
use serde::de::DeserializeOwned;

use mde_collab_egui::{CollabData, CommandSink, CommunicationsSurface};
use mde_collab_types::topics::{self, projection as proj};
use mde_collab_types::{
    ActivityFeed, ActorId, AlertInbox, CallState, ClipboardLane, CollabCommand,
    ConversationTimeline, DocumentSessions, EventId, FileReferences, SpaceDirectory, SpaceId,
    ThreadId, ThreadTimeline, TransferJobs,
};

use crate::bus_reader::BusReader;

/// Poll cadence — matches the collab worker's own 2 s tick so the rail +
/// conversations stay live without a cold-start wait (the `chat.rs` cadence).
const REFRESH: Duration = Duration::from_secs(2);

/// Defensive shell-side cap for retained Activity mirrors. The current collab
/// worker publishes the same 1,024-row cap, but a live seat can carry an older or
/// hand-authored Bus mirror; the UI boundary still must not paint or scan an
/// unbounded feed on low-end hardware.
const MAX_ACTIVITY_FEED_ENTRIES: usize = 1024;

/// Seat-local read cursors. This topic deliberately lives outside the
/// replicated `state/collab/*` namespace: read position is a UI preference for
/// this seat, not a collaboration event or a remote read receipt.
const LOCAL_READ_CURSORS_TOPIC: &str = "local/collab/read-cursors";

/// The canonical mesh clipboard responder namespace. Communications still emits
/// typed `action/collab/*` commands for its signed projection, but row
/// pin/delete/clear controls must also hit this lane so Mesh Teams edits the
/// same clipboard history as the Clipboard Viewer.
const CLIPBOARD_ACTION_PREFIX: &str = "action/clipboard/";

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

/// Keep only the newest Activity rows from a retained Bus mirror. Activity feeds
/// are newest-last by contract, so draining from the front preserves order and
/// keeps the cursor/virtualized renderer on the recent window.
fn bounded_activity_feed(mut feed: ActivityFeed) -> ActivityFeed {
    let overflow = feed.entries.len().saturating_sub(MAX_ACTIVITY_FEED_ENTRIES);
    if overflow > 0 {
        feed.entries.drain(0..overflow);
    }
    feed
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
    /// Activity feeds currently folded for paint, keyed `Some(space)` to match
    /// the surface's `data.activity(self.selected_space())` read (folded from
    /// `state/collab/activity/<space>`). The shell intentionally keeps this to
    /// the focused channel so opening Mesh Teams does not deserialize every
    /// retained per-space Activity body on a modest seat.
    activity: HashMap<Option<SpaceId>, ActivityFeed>,
    /// Per-space conversation timelines (folded from
    /// `state/collab/conversation/<space>`).
    conversations: HashMap<SpaceId, ConversationTimeline>,
    /// Retained thread timelines, keyed by their typed thread id. The worker's
    /// per-space thread topic carries one typed timeline, so the root index is
    /// built at the same time for the message-row reply affordance.
    threads: HashMap<ThreadId, ThreadTimeline>,
    /// Thread lookup by its owning space and root message event.
    thread_roots: HashMap<(SpaceId, EventId), ThreadId>,
    /// The aggregated active-call state — every space's `state/collab/call-state`
    /// concatenated into the one persistent call bar's read model.
    call_state: CallState,
    /// Per-space linked-file references (folded from
    /// `state/collab/file-references/<space>`).
    file_references: HashMap<SpaceId, FileReferences>,
    /// Fleet-wide transfer ledger mirror (folded from
    /// `state/collab/transfer-jobs`).
    transfer_jobs: Option<TransferJobs>,
    /// Fleet-wide alert inbox (folded from `state/collab/alert-inbox`).
    alert_inbox: Option<AlertInbox>,
    /// Per-space clipboard lanes (folded from
    /// `state/collab/clipboard-lane/<space>`).
    clipboard_lanes: HashMap<SpaceId, ClipboardLane>,
    /// Per-space live document-session lists (folded from
    /// `state/collab/document-sessions/<space>`).
    document_sessions: HashMap<SpaceId, DocumentSessions>,
    /// The local seat's durable read position for each space. Cursors are
    /// compared with the activity HLC, so a restart does not turn retained
    /// history into a new unread storm.
    read_cursors: HashMap<SpaceId, mde_collab_types::ActorClock>,
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
            threads: HashMap::new(),
            thread_roots: HashMap::new(),
            call_state: CallState::default(),
            file_references: HashMap::new(),
            transfer_jobs: None,
            alert_inbox: None,
            clipboard_lanes: HashMap::new(),
            document_sessions: HashMap::new(),
            read_cursors: HashMap::new(),
            last_poll: None,
        }
    }

    /// Re-fold on the [`REFRESH`] cadence while the surface is in view, and keep
    /// the frame loop ticking so a worker republish surfaces without operator
    /// input (the `chat.rs` poll shape).
    fn poll(&mut self, ctx: &egui::Context, focus_space: Option<SpaceId>) {
        if self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH) {
            self.last_poll = Some(Instant::now());
            self.refresh_for(focus_space);
            ctx.request_repaint_after(REFRESH);
        }
    }

    /// Fold the retained `state/collab/*` mirrors into the owned projections. Opens
    /// the spool fail-soft: no spool / an unopenable store clears to the honest
    /// off-mesh empty state (§7). The `directory` names the spaces and
    /// [`refresh_for`](Self::refresh_for) chooses which channel's heavy
    /// per-space projections are read from the one open handle.
    fn refresh(&mut self) {
        self.refresh_for(None);
    }

    /// Fold the retained `state/collab/*` mirrors into the owned projections for
    /// the currently focused channel. This is the seat .15 open-path guard: the
    /// directory and global rollups stay live, but expensive per-space bodies are
    /// read only for the selected channel (or the first directory row before the
    /// first UI frame has selected one).
    fn refresh_for(&mut self, focus_space: Option<SpaceId>) {
        self.now_unix_ms = now_unix_ms();
        let Some(persist) = self.reader.open() else {
            self.directory = SpaceDirectory::default();
            self.activity.clear();
            self.conversations.clear();
            self.threads.clear();
            self.thread_roots.clear();
            self.call_state = CallState::default();
            self.file_references.clear();
            self.transfer_jobs = None;
            self.alert_inbox = None;
            self.clipboard_lanes.clear();
            self.document_sessions.clear();
            self.read_cursors.clear();
            return;
        };

        self.directory =
            read_state(&persist, &topics::state_topic(proj::SPACE_DIRECTORY)).unwrap_or_default();
        self.read_cursors = read_state(&persist, LOCAL_READ_CURSORS_TOPIC).unwrap_or_default();
        let focus_space = focus_space
            .filter(|candidate| {
                self.directory
                    .spaces
                    .iter()
                    .any(|summary| summary.id == *candidate)
            })
            .or_else(|| self.directory.spaces.first().map(|summary| summary.id));

        let mut activity = HashMap::new();
        let mut conversations = HashMap::new();
        let mut threads = HashMap::new();
        let mut thread_roots = HashMap::new();
        let mut call_state = CallState::default();
        let mut file_references = HashMap::new();
        let mut clipboard_lanes = HashMap::new();
        let mut document_sessions = HashMap::new();
        for summary in &self.directory.spaces {
            let space = summary.id;
            if Some(space) == focus_space {
                if let Some(feed) = read_state::<ActivityFeed>(
                    &persist,
                    &topics::space_state_topic(proj::ACTIVITY, space),
                ) {
                    activity.insert(Some(space), bounded_activity_feed(feed));
                }
                if let Some(convo) = read_state::<ConversationTimeline>(
                    &persist,
                    &topics::space_state_topic(proj::CONVERSATION, space),
                ) {
                    conversations.insert(space, convo);
                }
                if let Some(thread) = read_state::<ThreadTimeline>(
                    &persist,
                    &topics::space_state_topic(proj::THREAD, space),
                ) {
                    thread_roots.insert((thread.space, thread.root.event_id), thread.thread);
                    threads.insert(thread.thread, thread);
                }
                if let Some(files) = read_state::<FileReferences>(
                    &persist,
                    &topics::space_state_topic(proj::FILE_REFERENCES, space),
                ) {
                    file_references.insert(space, files);
                }
                if let Some(clipboard) = read_state::<ClipboardLane>(
                    &persist,
                    &topics::space_state_topic(proj::CLIPBOARD_LANE, space),
                ) {
                    clipboard_lanes.insert(space, clipboard);
                }
                if let Some(sessions) = read_state::<DocumentSessions>(
                    &persist,
                    &topics::space_state_topic(proj::DOCUMENT_SESSIONS, space),
                ) {
                    document_sessions.insert(space, sessions);
                }
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
        for summary in &mut self.directory.spaces {
            let cursor = self
                .read_cursors
                .get(&summary.id)
                .copied()
                .unwrap_or_default();
            summary.unread = activity
                .get(&Some(summary.id))
                .map(|feed| {
                    feed.entries
                        .iter()
                        .rev()
                        .take_while(|entry| entry.clock > cursor)
                        .count()
                        .min(u32::MAX as usize) as u32
                })
                .unwrap_or_else(|| if summary.last_activity > cursor { 1 } else { 0 });
        }
        let transfer_jobs =
            read_state::<TransferJobs>(&persist, &topics::state_topic(proj::TRANSFER_JOBS));
        let alert_inbox =
            read_state::<AlertInbox>(&persist, &topics::state_topic(proj::ALERT_INBOX));
        self.activity = activity;
        self.conversations = conversations;
        self.threads = threads;
        self.thread_roots = thread_roots;
        self.call_state = call_state;
        self.file_references = file_references;
        self.transfer_jobs = transfer_jobs;
        self.alert_inbox = alert_inbox;
        self.clipboard_lanes = clipboard_lanes;
        self.document_sessions = document_sessions;
    }

    /// Advance a seat-local cursor to the newest activity currently visible in
    /// `space`. A failed write leaves the in-memory cursor unchanged, so the
    /// badge remains honest and the next render can retry the persistence.
    fn mark_space_read(&mut self, space: SpaceId) {
        let Some(latest) = self
            .activity
            .get(&Some(space))
            .and_then(|feed| feed.entries.last().map(|entry| entry.clock))
        else {
            return;
        };
        if self
            .read_cursors
            .get(&space)
            .is_some_and(|cursor| *cursor >= latest)
        {
            return;
        }

        let mut next = self.read_cursors.clone();
        next.insert(space, latest);
        let Ok(body) = serde_json::to_string(&next) else {
            tracing::debug!(target: "shell::communications", "failed to encode local read cursors");
            return;
        };
        let Some(persist) = self.reader.open() else {
            return;
        };
        if let Err(error) = persist.write(
            LOCAL_READ_CURSORS_TOPIC,
            Priority::Default,
            None,
            Some(&body),
        ) {
            tracing::debug!(
                target: "shell::communications",
                %error,
                "failed to persist local collaboration read cursor",
            );
            return;
        }

        self.read_cursors = next;
        if let Some(summary) = self.directory.spaces.iter_mut().find(|s| s.id == space) {
            summary.unread = 0;
        }
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

    fn thread(&self, space: SpaceId, thread: ThreadId) -> Option<&ThreadTimeline> {
        self.threads
            .get(&thread)
            .filter(|timeline| timeline.space == space)
    }

    fn thread_for_root(&self, space: SpaceId, root: EventId) -> Option<ThreadId> {
        self.thread_roots.get(&(space, root)).copied()
    }

    fn call_state(&self) -> &CallState {
        &self.call_state
    }

    fn file_references(&self, space: SpaceId) -> Option<&FileReferences> {
        self.file_references.get(&space)
    }

    fn transfer_jobs(&self) -> Option<&TransferJobs> {
        self.transfer_jobs.as_ref()
    }

    fn alert_inbox(&self) -> Option<&AlertInbox> {
        self.alert_inbox.as_ref()
    }

    fn clipboard_lane(&self, space: SpaceId) -> Option<&ClipboardLane> {
        self.clipboard_lanes.get(&space)
    }

    fn document_sessions(&self, space: SpaceId) -> Option<&DocumentSessions> {
        self.document_sessions.get(&space)
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
        self.data.poll(ctx, self.surface.selected_space());
    }

    /// Render the surface and route the frame's emitted commands. The widget reads
    /// [`self.data`](LiveCollabData) and pushes intent into a per-frame
    /// [`CommandSink`]; this drains the sink and publishes each command onto
    /// `action/collab/<verb>` so the collab worker applies it.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        let mut sink = CommandSink::new();
        let selected_before = self.surface.selected_space();
        self.surface.ui(ui, &self.data, &mut sink);
        let selected_after = self.surface.selected_space();
        if selected_after != selected_before {
            self.data.refresh_for(selected_after);
            ui.ctx().request_repaint();
        }
        if let Some(space) = self.surface.selected_space() {
            self.data.mark_space_read(space);
        }
        drain_to_bus(&mut sink, self.bus_root.as_deref(), &self.data);
    }
}

/// Drain every command the surface emitted this frame onto `action/collab/*`. A
/// publish failure is logged (visible) and dropped — never a silent swallow, and
/// never a faked local apply (the worker is the one authority).
fn drain_to_bus(sink: &mut CommandSink, bus_root: Option<&Path>, data: &dyn CollabData) {
    for command in sink.drain() {
        if let Err(e) = publish_canonical_clipboard_action(bus_root, data, &command) {
            tracing::debug!(
                target: "shell::communications",
                verb = command.verb(),
                error = %e,
                "canonical clipboard action publish failed",
            );
        }
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

/// Mirror Communications clipboard row mutations to the canonical
/// `action/clipboard/*` responder. The collab command remains the signed
/// projection authority; this companion request keeps the mesh-global
/// `clipboard/history.json` action semantics from drifting into a parallel
/// Communications-only store.
fn publish_canonical_clipboard_action(
    bus_root: Option<&Path>,
    data: &dyn CollabData,
    command: &CollabCommand,
) -> Result<(), String> {
    match command {
        CollabCommand::PinClipboard { space, clip } => {
            let id = clipboard_history_id_for(data, *space, *clip)?;
            publish_clipboard_action_request(bus_root, "pin", Some(&id))
        }
        CollabCommand::UnpinClipboard { space, clip } => {
            let id = clipboard_history_id_for(data, *space, *clip)?;
            publish_clipboard_action_request(bus_root, "unpin", Some(&id))
        }
        CollabCommand::DeleteClipboard { space, clip } => {
            let id = clipboard_history_id_for(data, *space, *clip)?;
            publish_clipboard_action_request(bus_root, "delete", Some(&id))
        }
        CollabCommand::ClearClipboard { .. } => {
            publish_clipboard_action_request(bus_root, "clear", None)
        }
        _ => Ok(()),
    }
}

fn clipboard_history_id_for(
    data: &dyn CollabData,
    space: SpaceId,
    clip: EventId,
) -> Result<String, String> {
    let item = data
        .clipboard_lane(space)
        .and_then(|lane| lane.items.iter().find(|item| item.event_id == clip))
        .ok_or_else(|| format!("clipboard item {clip} is not in the folded lane for {space}"))?;
    clipboard_history_id(&item.sha256_hex).ok_or_else(|| {
        format!(
            "clipboard item {clip} has an invalid content hash for canonical history addressing"
        )
    })
}

/// `clipboard_sync::clip_id` is the first 16 lower-hex chars of the full SHA-256.
/// The Communications read model carries the full content address, so the shell
/// can address canonical history rows without linking the daemon worker crate.
fn clipboard_history_id(sha256_hex: &str) -> Option<String> {
    let id = sha256_hex.get(..16)?;
    if id.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(id.to_ascii_lowercase())
    } else {
        None
    }
}

fn clipboard_action_topic(verb: &str) -> String {
    format!("{CLIPBOARD_ACTION_PREFIX}{verb}")
}

fn publish_clipboard_action_request(
    bus_root: Option<&Path>,
    verb: &str,
    id: Option<&str>,
) -> Result<(), String> {
    let Some(root) = bus_root else {
        return Err("No local Bus — the mesh daemon may be down.".to_string());
    };
    let unsigned = match id {
        Some(id) => serde_json::json!({
            "id": id,
            "schema_version": CLOUD_ACTION_SCHEMA_VERSION,
        }),
        None => serde_json::json!({
            "schema_version": CLOUD_ACTION_SCHEMA_VERSION,
        }),
    }
    .to_string();
    let auth_verb = format!("clipboard-{verb}");
    let target = id
        .map(|id| format!("entry:{id}"))
        .unwrap_or_else(|| "all-unpinned".to_string());
    let authorized =
        crate::iac::authorize_root_mutation_body(&unsigned, &auth_verb, "clipboard", &target)?;
    let persist = Persist::open(root.to_path_buf())
        .map_err(|e| format!("Couldn't open the local Bus: {e}"))?;
    mde_bus::rpc::publish_request(
        &persist,
        &clipboard_action_topic(verb),
        Priority::Default,
        None,
        Some(&authorized),
    )
    .map_err(|e| format!("Bus write failed: {e}"))?;
    Ok(())
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
    let mut envelope =
        serde_json::to_value(command).map_err(|e| format!("serialize collab command: {e}"))?;
    envelope["schema_version"] = serde_json::Value::from(CLOUD_ACTION_SCHEMA_VERSION);
    let body = serde_json::to_string(&envelope)
        .map_err(|e| format!("serialize collab command envelope: {e}"))?;
    let authorized = crate::iac::authorize_root_mutation_body(
        &body,
        "collab-command",
        &crate::explorer::local_hostname(),
        command.verb(),
    )?;
    let persist = Persist::open(root.to_path_buf())
        .map_err(|e| format!("Couldn't open the local Bus: {e}"))?;
    persist
        .write(topic, Priority::Default, None, Some(&authorized))
        .map_err(|e| format!("Bus write failed: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use mde_collab_types::value::{sha256_hex, CallKind, ClipItemKind, DeliveryState, MessageBody};
    use mde_collab_types::{
        ActivityEntry, ActorClock, CallParticipantState, CallParticipantView, CallView,
        ClipboardView, EventId, MessageView, SpaceKind, SpaceRole, SpaceSummary,
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

    fn activity_entry(space: SpaceId, actor: &ActorId, wall_ms: u64) -> ActivityEntry {
        ActivityEntry {
            event_id: EventId::new(),
            space,
            actor: actor.clone(),
            clock: ActorClock::at(wall_ms, 0),
            created_unix_ms: wall_ms as i64,
            kind_tag: "message_posted".to_owned(),
            summary: "posted a message".to_owned(),
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
        let thread_id = ThreadId::new();
        let thread_root = message(&peer, "thread root");
        write_state(
            &persist,
            &topics::space_state_topic(proj::THREAD, ops),
            &ThreadTimeline {
                space: ops,
                thread: thread_id,
                root: thread_root.clone(),
                replies: vec![message(&me, "thread reply")],
                resolved: false,
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::FILE_REFERENCES, ops),
            &FileReferences {
                space: ops,
                files: Vec::new(),
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::CLIPBOARD_LANE, ops),
            &ClipboardLane {
                space: ops,
                items: Vec::new(),
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::DOCUMENT_SESSIONS, ops),
            &DocumentSessions::default(),
        );
        write_state(
            &persist,
            &topics::state_topic(proj::TRANSFER_JOBS),
            &TransferJobs::default(),
        );
        write_state(
            &persist,
            &topics::state_topic(proj::ALERT_INBOX),
            &AlertInbox::default(),
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

        let thread = data.thread(ops, thread_id).expect("thread folded");
        assert_eq!(thread.root.event_id, thread_root.event_id);
        assert_eq!(thread.replies.len(), 1);
        assert_eq!(
            data.thread_for_root(ops, thread_root.event_id),
            Some(thread_id),
            "thread root lookup folded"
        );
        assert!(data.file_references(ops).is_some(), "files folded");
        assert!(data.transfer_jobs().is_some(), "transfers folded");
        assert!(data.alert_inbox().is_some(), "alerts folded");
        assert!(data.clipboard_lane(ops).is_some(), "clipboard folded");
        assert!(data.document_sessions(ops).is_some(), "documents folded");
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
        assert!(data.thread(SpaceId::new(), ThreadId::new()).is_none());
        assert!(data.transfer_jobs().is_none());
        assert!(data.alert_inbox().is_none());
    }

    #[test]
    fn first_open_folds_only_the_focused_channel_activity_body() {
        // Seat .15 regression guard: opening Mesh Teams should not deserialize
        // every retained channel Activity body before the first frame can paint.
        // The focused channel still folds exactly; the non-focused channel keeps
        // an attention badge derived from the directory clock until selected.
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());
        let focused = SpaceId::new();
        let noisy = SpaceId::new();
        let peer = ActorId::new("falcon");

        write_state(
            &persist,
            &topics::state_topic(proj::SPACE_DIRECTORY),
            &SpaceDirectory {
                spaces: vec![
                    space_summary(focused, "Focused Ops"),
                    space_summary(noisy, "Noisy Ops"),
                ],
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::ACTIVITY, focused),
            &ActivityFeed {
                space: Some(focused),
                entries: vec![
                    activity_entry(focused, &peer, 1_000),
                    activity_entry(focused, &peer, 1_001),
                ],
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::ACTIVITY, noisy),
            &ActivityFeed {
                space: Some(noisy),
                entries: (0..2_000)
                    .map(|index| activity_entry(noisy, &peer, 2_000 + index))
                    .collect(),
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::CONVERSATION, noisy),
            &ConversationTimeline {
                space: noisy,
                thread: None,
                messages: vec![message(&peer, "not on the first-open path")],
            },
        );

        let mut data = LiveCollabData::new(Some(dir.path().to_path_buf()));
        data.refresh_for(Some(focused));

        assert_eq!(
            data.activity(Some(focused))
                .expect("focused activity folded")
                .entries
                .len(),
            2,
            "the focused channel keeps its exact unread/activity feed"
        );
        assert!(
            data.activity(Some(noisy)).is_none(),
            "a non-focused channel's retained Activity body must not be deserialized on open"
        );
        assert!(
            data.conversation(noisy).is_none(),
            "non-focused heavy per-space mirrors stay out of the first-open fold"
        );
        let focused_row = data
            .space_directory()
            .spaces
            .iter()
            .find(|summary| summary.id == focused)
            .expect("focused row");
        let noisy_row = data
            .space_directory()
            .spaces
            .iter()
            .find(|summary| summary.id == noisy)
            .expect("noisy row");
        assert_eq!(focused_row.unread, 2);
        assert_eq!(
            noisy_row.unread, 1,
            "unfocused rows keep a cheap attention badge from the directory clock"
        );
    }

    #[test]
    fn focused_activity_feed_is_clamped_and_read_cursor_uses_newest_row() {
        // Seat .15 regression guard: a stale or older worker-retained Activity
        // mirror can be larger than the current core projection cap. The shell
        // keeps the newest-last contract, clamps that mirror at its read boundary,
        // and marks read from the newest retained row without a per-frame max scan.
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());
        let space = SpaceId::new();
        let peer = ActorId::new("falcon");

        write_state(
            &persist,
            &topics::state_topic(proj::SPACE_DIRECTORY),
            &SpaceDirectory {
                spaces: vec![space_summary(space, "Operations")],
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::ACTIVITY, space),
            &ActivityFeed {
                space: Some(space),
                entries: (0..2_000)
                    .map(|index| activity_entry(space, &peer, index))
                    .collect(),
            },
        );

        let mut data = LiveCollabData::new(Some(dir.path().to_path_buf()));
        data.refresh_for(Some(space));

        let feed = data.activity(Some(space)).expect("focused activity folded");
        assert_eq!(
            feed.entries.len(),
            MAX_ACTIVITY_FEED_ENTRIES,
            "oversized retained mirrors must be clamped at the UI read boundary"
        );
        assert_eq!(
            feed.entries.first().expect("first retained").clock,
            ActorClock::at(976, 0)
        );
        assert_eq!(
            feed.entries.last().expect("newest retained").clock,
            ActorClock::at(1_999, 0),
            "clamping keeps newest-last order"
        );
        assert_eq!(
            data.space_directory().spaces[0].unread,
            MAX_ACTIVITY_FEED_ENTRIES as u32,
            "unread counting is bounded to the retained activity window"
        );

        data.mark_space_read(space);

        assert_eq!(
            data.read_cursors.get(&space).copied(),
            Some(ActorClock::at(1_999, 0)),
            "mark-read advances to the newest retained row"
        );
        assert_eq!(data.space_directory().spaces[0].unread, 0);
    }

    #[test]
    fn read_cursors_drive_unread_badges_and_survive_reload() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());
        let space = SpaceId::new();
        let peer = ActorId::new("falcon");
        let feed = |entries| ActivityFeed {
            space: Some(space),
            entries,
        };

        write_state(
            &persist,
            &topics::state_topic(proj::SPACE_DIRECTORY),
            &SpaceDirectory {
                spaces: vec![space_summary(space, "Team Ops")],
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::ACTIVITY, space),
            &feed(vec![
                activity_entry(space, &peer, 1_000),
                activity_entry(space, &peer, 1_001),
            ]),
        );

        let mut data = LiveCollabData::new(Some(dir.path().to_path_buf()));
        data.refresh();
        assert_eq!(data.space_directory().spaces[0].unread, 2);

        data.mark_space_read(space);
        assert_eq!(data.space_directory().spaces[0].unread, 0);

        let mut reloaded = LiveCollabData::new(Some(dir.path().to_path_buf()));
        reloaded.refresh();
        assert_eq!(
            reloaded.space_directory().spaces[0].unread,
            0,
            "the seat-local cursor is durable across a shell reload"
        );

        write_state(
            &persist,
            &topics::space_state_topic(proj::ACTIVITY, space),
            &feed(vec![
                activity_entry(space, &peer, 1_000),
                activity_entry(space, &peer, 1_001),
                activity_entry(space, &peer, 1_002),
            ]),
        );
        reloaded.refresh();
        assert_eq!(
            reloaded.space_directory().spaces[0].unread,
            1,
            "only activity after the stored cursor is unread"
        );
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

        let data = LiveCollabData::new(Some(dir.path().to_path_buf()));
        drain_to_bus(&mut sink, Some(dir.path()), &data);
        assert!(sink.is_empty(), "the sink was drained");

        // The command landed on the canonical `action/collab/send` topic.
        let topic = topics::command_topic("send_message");
        assert_eq!(topic, "action/collab/send_message");
        let published = persist
            .read_latest(&topic)
            .expect("read command")
            .expect("command published");
        let envelope: serde_json::Value =
            serde_json::from_str(published.body.as_deref().expect("command body"))
                .expect("decode command envelope");
        assert_eq!(envelope["schema_version"], 1);
        assert!(
            envelope["armed_token"].as_str().is_some(),
            "mutable collab commands carry the root capability"
        );
        let mut command_value: serde_json::Value =
            serde_json::from_str(published.body.as_deref().expect("command body"))
                .expect("decode command envelope");
        let object = command_value
            .as_object_mut()
            .expect("command envelope object");
        object.remove("armed_token");
        object.remove("schema_version");
        let back: CollabCommand = serde_json::from_value(command_value).expect("decode command");
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

    #[test]
    fn clipboard_pin_publishes_collab_command_and_canonical_clipboard_action() {
        // Mesh Teams renders the collab read model, but a row pin must also hit
        // the canonical action/clipboard responder. The responder addresses rows
        // by clipboard_sync's 16-hex content id, not the collab EventId.
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());
        let ops = SpaceId::new();
        let clip = EventId::new();
        let text = b"canonical mesh clip";
        let full_hash = sha256_hex(text);
        let history_id = full_hash[..16].to_string();

        write_state(
            &persist,
            &topics::state_topic(proj::SPACE_DIRECTORY),
            &SpaceDirectory {
                spaces: vec![space_summary(ops, "Team Ops")],
            },
        );
        write_state(
            &persist,
            &topics::space_state_topic(proj::CLIPBOARD_LANE, ops),
            &ClipboardLane {
                space: ops,
                items: vec![ClipboardView {
                    event_id: clip,
                    kind: ClipItemKind::Text,
                    preview: "canonical mesh clip".to_string(),
                    sha256_hex: full_hash,
                    source: "falcon".to_string(),
                    at_unix_ms: 1_700_000_000_000,
                    pinned: false,
                }],
            },
        );

        let mut data = LiveCollabData::new(Some(dir.path().to_path_buf()));
        data.refresh_for(Some(ops));
        let mut sink = CommandSink::new();
        sink.emit(CollabCommand::PinClipboard { space: ops, clip });

        drain_to_bus(&mut sink, Some(dir.path()), &data);

        let collab = persist
            .read_latest(&topics::command_topic("pin_clipboard"))
            .expect("read collab command")
            .expect("collab command published");
        let collab_body: serde_json::Value =
            serde_json::from_str(collab.body.as_deref().expect("collab body"))
                .expect("decode collab command");
        assert_eq!(collab_body["schema_version"], 1);
        assert!(
            collab_body["armed_token"].as_str().is_some(),
            "collab projection command remains capability-gated"
        );

        let canonical = persist
            .read_latest(&clipboard_action_topic("pin"))
            .expect("read canonical clipboard action")
            .expect("canonical clipboard action published");
        let action_body: serde_json::Value =
            serde_json::from_str(canonical.body.as_deref().expect("action body"))
                .expect("decode canonical action");
        assert_eq!(action_body["schema_version"], 1);
        assert_eq!(action_body["id"], history_id);
        assert!(
            action_body["armed_token"].as_str().is_some(),
            "canonical clipboard mutation carries the responder capability"
        );
    }

    #[test]
    fn clear_clipboard_publishes_canonical_clear_request() {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = persist_at(dir.path());
        let ops = SpaceId::new();
        let data = LiveCollabData::new(Some(dir.path().to_path_buf()));
        let mut sink = CommandSink::new();
        sink.emit(CollabCommand::ClearClipboard { space: ops });

        drain_to_bus(&mut sink, Some(dir.path()), &data);

        let canonical = persist
            .read_latest(&clipboard_action_topic("clear"))
            .expect("read canonical clipboard action")
            .expect("canonical clipboard clear published");
        let action_body: serde_json::Value =
            serde_json::from_str(canonical.body.as_deref().expect("action body"))
                .expect("decode canonical action");
        assert_eq!(action_body["schema_version"], 1);
        assert!(
            action_body.get("id").is_none(),
            "clear targets all unpinned history, not a row id"
        );
        assert!(
            action_body["armed_token"].as_str().is_some(),
            "canonical clear carries the responder capability"
        );
    }
}
