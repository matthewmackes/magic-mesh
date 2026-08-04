//! `mde-collab-egui` — the **Communications surface** (WL-FUNC-011 Phase 3).
//!
//! A single [`CommunicationsSurface`] widget on the shared `mde-egui`
//! **Construct** harness. It renders the read-side
//! [`CollabReadModel`](mde_collab_types::CollabReadModel) projections and emits
//! typed [`CollabCommand`](mde_collab_types::CollabCommand)s — it owns **no**
//! authoritative state and calls **no** providers (governance: a pure UI crate,
//! §6 desktop-shell tier, edges pointing inward to `mde-egui` + the
//! `mde-collab-types` contracts).
//!
//! # The frame
//!
//! Every mode renders inside one persistent frame:
//!
//! * a Teams-like **app rail** down the far left — Activity, Teams, Calls, Files,
//!   Alerts, Transfers, Clipboard, and Settings — with every click routing
//!   through one app-selection seam;
//! * a **Teams + Channels rail** next to it, listing the
//!   [`SpaceDirectory`](mde_collab_types::SpaceDirectory) with per-space unread
//!   badges (the selection key for every other pane);
//! * a channel header across the top. The Teams app exposes Posts / Files /
//!   Calls / Tasks channel tabs, while other apps expose a single app header and keep
//!   their existing bodies. [`Mode::Activity`], [`Mode::Messages`],
//!   [`Mode::Calls`], [`Mode::Files`], [`Mode::Transfers`], [`Mode::Documents`],
//!   [`Mode::Alerts`], and [`Mode::Clipboard`] are all implemented. Documents
//!   (WL-FUNC-011 Phase 3c foundation) embeds the real `mde-editor-egui` editor —
//!   a Project sub-mode (the full IDE) and a default Document sub-mode (a one-pane
//!   Markdown editor) — and emits the collab document commands; the CRDT live
//!   co-edit / three-way merge / review sidecar / versioning are marked in-code
//!   follow-ups, never faked;
//! * a persistent **call bar** across the bottom that renders the
//!   [`CallState`](mde_collab_types::CallState) read model and survives every
//!   mode/space switch, with controls wired to the call commands even though the
//!   media plane lands later. The [`Mode::Calls`] tab is the full roster + controls
//!   view of that same read model — start (audio / video / screen-share), the
//!   per-call participant roster, mute / camera / screen-source toggles, an in-call
//!   DTMF keypad, and hang up — all emitting typed call
//!   [`CollabCommand`](mde_collab_types::CollabCommand)s. The live media transport
//!   (WebRTC P2P for direct calls, an elected LiveKit SFU for group/failover, and
//!   the existing SIP account/DID/G.711 behind a LiveKit SIP gateway) is the
//!   explicit, in-code-marked media-plane follow-up; there is **no** recording or
//!   transcription anywhere (deliberately absent from the UI, commands, and state).
//!
//! # The core modes
//!
//! * [`Mode::Activity`] — an action-oriented chronological feed from the
//!   [`ActivityFeed`](mde_collab_types::ActivityFeed) projection with band
//!   filters, and deliberately **no** competing global search box (spec §2).
//! * [`Mode::Messages`] — a Markdown conversation timeline
//!   ([`ConversationTimeline`](mde_collab_types::ConversationTimeline)) with
//!   anchored threads ([`ThreadTimeline`](mde_collab_types::ThreadTimeline)), a
//!   multiline composer whose <kbd>Ctrl</kbd>+<kbd>Enter</kbd> emits
//!   [`SendMessage`](mde_collab_types::CollabCommand::SendMessage), locally
//!   persisted drafts, honest delivery state, and an edit/delete affordance that
//!   reflects the core's five-minute author window (spec §3).
//! * [`Mode::Files`] — the files a space owns **references** to
//!   ([`FileReferences`](mde_collab_types::FileReferences)) with their owner +
//!   content address, a picker that reuses the file-manager's listing to
//!   [`LinkFile`](mde_collab_types::CollabCommand::LinkFile), a reference-remove
//!   ([`UnlinkFile`](mde_collab_types::CollabCommand::UnlinkFile)) kept distinct
//!   from a typed-confirm permanent delete, and shared-transfer controls
//!   ([`StartTransfer`](mde_collab_types::CollabCommand::StartTransfer) /
//!   [`ControlTransfer`](mde_collab_types::CollabCommand::ControlTransfer)) whose
//!   state is read from the WL-FUNC-006 ledger mirror (no second authority).
//! * [`Mode::Tasks`] — basic channel tasks/action items read from
//!   [`ChannelTasks`](mde_collab_types::ChannelTasks), with create/check/complete
//!   controls emitting typed collaboration commands.
//!
//! # Data + commands
//!
//! The surface READS projections through the [`CollabData`] trait it is handed
//! and EMITS commands into a [`CommandSink`] the caller drains. For this phase
//! the crate stands alone with [`FixtureData`]; the real
//! `BusReader`-backed [`CollabData`] and the sink-to-`action/collab/*` drain are
//! a later shell-mount phase.

#![doc(html_no_source)]

mod activity;
mod alerts;
mod anim;
mod calls;
mod clipboard;
mod data;
mod documents;
mod files;
mod fixture;
mod frame;
mod icons;
use crate::icons::CommsHoverExt;
mod messages;
mod transfers;

#[cfg(test)]
mod tests;

pub use data::{
    amend_affordance, relative_age, AmendAffordance, CollabData, CommandSink, EDIT_WINDOW_MS,
};
pub use documents::{DocSubMode, DocTemplate, DocView};
pub use fixture::FixtureData;
pub use icons::ALL_COLLAB_ICONS;

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use mde_collab_types::{CallId, EventId, Severity, SpaceId, ThreadId};

pub use files::file_ref_of_path;

// Re-export the harness `egui` so a mount site and the tests resolve to the one
// pinned toolkit version through this crate alone.
pub use mde_egui::egui;

/// A per-space mode tab. Every tab is implemented, including
/// [`Documents`](Self::Documents), which embeds the real `mde-editor-egui` editor
/// (its Project sub-mode is the full IDE; its default Document sub-mode is a
/// one-pane Markdown editor) and emits the collab document commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// The action-oriented chronological Activity feed.
    #[default]
    Activity,
    /// The Markdown conversation timeline + anchored threads.
    Messages,
    /// The calls roster + controls — the full view of the persistent call bar's
    /// [`CallState`](mde_collab_types::CallState): start / answer / decline /
    /// mute / DTMF / hang up. The live media transport is a marked follow-up.
    Calls,
    /// Basic channel tasks/action items.
    Tasks,
    /// The files linked into a space (their references + shared transfers).
    Files,
    /// The shared transfer jobs (the WL-FUNC-006 ledger mirror) + their controls.
    Transfers,
    /// The documents mode — the embedded editor (a Project IDE sub-mode + a
    /// default one-pane Markdown Document sub-mode) over the space's documents.
    Documents,
    /// The fleet-wide alert inbox (severity/source/state + ack/snooze/actions).
    Alerts,
    /// The space's clipboard lane (MIME items + publish/attach/pin/delete).
    Clipboard,
}

impl Mode {
    /// The tabs in display order.
    pub const TABS: [Self; 9] = [
        Self::Activity,
        Self::Messages,
        Self::Calls,
        Self::Tasks,
        Self::Files,
        Self::Transfers,
        Self::Documents,
        Self::Alerts,
        Self::Clipboard,
    ];

    /// The tab label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Activity => "Activity",
            Self::Messages => "Messages",
            Self::Calls => "Calls",
            Self::Tasks => "Tasks",
            Self::Files => "Files",
            Self::Transfers => "Transfers",
            Self::Documents => "Documents",
            Self::Alerts => "Alerts",
            Self::Clipboard => "Clipboard",
        }
    }

    /// Whether this mode is implemented. Every mode is now implemented, including
    /// Documents (WL-FUNC-011 Phase 3c foundation) — no tab is a labeled-for-later
    /// placeholder. Retained as the mode-tab tint predicate.
    #[must_use]
    pub const fn is_implemented(self) -> bool {
        match self {
            Self::Activity
            | Self::Messages
            | Self::Calls
            | Self::Tasks
            | Self::Files
            | Self::Transfers
            | Self::Documents
            | Self::Alerts
            | Self::Clipboard => true,
        }
    }

    /// Whether this is a **dense**, manage-heavy pane — a full conversation
    /// timeline, a file/document manager, or the clipboard lane — ill-suited to a
    /// glance from behind the wheel. Auto Mode (Car Mode) biases the default away
    /// from these toward the glanceable [`Alerts`](Self::Alerts) inbox; it never
    /// removes them (a passenger can still open one).
    #[must_use]
    pub const fn is_dense(self) -> bool {
        matches!(
            self,
            Self::Messages | Self::Tasks | Self::Files | Self::Documents | Self::Clipboard
        )
    }
}

/// The Teams-style app rail route. This is the visible Mesh Teams app model;
/// existing mode bodies remain the implementation behind each app route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MeshTeamsApp {
    /// Cross-space attention inbox.
    #[default]
    Activity,
    /// Teams/channels conversation workspace.
    Teams,
    /// Calls roster and active-call controls.
    Calls,
    /// Channel files / document entry points.
    Files,
    /// Alert inbox.
    Alerts,
    /// Transfer ledger.
    Transfers,
    /// Shared clipboard lane.
    Clipboard,
    /// Local Mesh Teams preferences.
    Settings,
}

impl MeshTeamsApp {
    /// App rail order locked by WL-UX-010.
    pub const ALL: [Self; 8] = [
        Self::Activity,
        Self::Teams,
        Self::Calls,
        Self::Files,
        Self::Alerts,
        Self::Transfers,
        Self::Clipboard,
        Self::Settings,
    ];

    /// Rail label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Activity => "Activity",
            Self::Teams => "Teams",
            Self::Calls => "Calls",
            Self::Files => "Files",
            Self::Alerts => "Alerts",
            Self::Transfers => "Transfers",
            Self::Clipboard => "Clipboard",
            Self::Settings => "Settings",
        }
    }

    /// Backing mode, when the app route owns one of the existing bodies.
    #[must_use]
    pub const fn mode(self) -> Option<Mode> {
        match self {
            Self::Activity => Some(Mode::Activity),
            Self::Teams => Some(Mode::Messages),
            Self::Calls => Some(Mode::Calls),
            Self::Files => Some(Mode::Files),
            Self::Alerts => Some(Mode::Alerts),
            Self::Transfers => Some(Mode::Transfers),
            Self::Clipboard => Some(Mode::Clipboard),
            Self::Settings => None,
        }
    }
}

/// The Teams app's per-channel tab strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelTab {
    /// Channel conversation posts.
    #[default]
    Posts,
    /// Files linked to the selected channel.
    Files,
    /// Calls in the selected channel.
    Calls,
    /// Basic channel tasks/action items.
    Tasks,
}

impl ChannelTab {
    /// Channel tab display order.
    pub const ALL: [Self; 4] = [Self::Posts, Self::Files, Self::Calls, Self::Tasks];

    /// Tab label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Posts => "Posts",
            Self::Files => "Files",
            Self::Calls => "Calls",
            Self::Tasks => "Tasks",
        }
    }

    /// Backing mode for the tab.
    #[must_use]
    pub const fn mode(self) -> Mode {
        match self {
            Self::Posts => Mode::Messages,
            Self::Files => Mode::Files,
            Self::Calls => Mode::Calls,
            Self::Tasks => Mode::Tasks,
        }
    }
}

fn app_for_mode(mode: Mode) -> MeshTeamsApp {
    match mode {
        Mode::Activity => MeshTeamsApp::Activity,
        Mode::Messages => MeshTeamsApp::Teams,
        Mode::Calls => MeshTeamsApp::Calls,
        Mode::Tasks => MeshTeamsApp::Teams,
        Mode::Files | Mode::Documents => MeshTeamsApp::Files,
        Mode::Transfers => MeshTeamsApp::Transfers,
        Mode::Alerts => MeshTeamsApp::Alerts,
        Mode::Clipboard => MeshTeamsApp::Clipboard,
    }
}

fn channel_tab_for_mode(mode: Mode) -> Option<ChannelTab> {
    match mode {
        Mode::Messages => Some(ChannelTab::Posts),
        Mode::Files | Mode::Documents => Some(ChannelTab::Files),
        Mode::Calls => Some(ChannelTab::Calls),
        Mode::Tasks => Some(ChannelTab::Tasks),
        _ => None,
    }
}

/// The band an [`ActivityFeed`](mde_collab_types::ActivityFeed) row is filtered
/// into, grouping the event-kind tags the projection carries. The Activity feed
/// filters by band; there is deliberately no global search box (spec §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActivityFilter {
    /// Every event kind.
    #[default]
    All,
    /// Messages + threads.
    Messages,
    /// Alerts (raised/acked/snoozed/actioned).
    Alerts,
    /// Calls (started/participant-changed/ended).
    Calls,
    /// File links + transfers.
    Files,
    /// Membership, presence, and space-lifecycle events.
    People,
}

impl ActivityFilter {
    /// The filter chips in display order.
    pub const ALL: [Self; 6] = [
        Self::All,
        Self::Messages,
        Self::Alerts,
        Self::Calls,
        Self::Files,
        Self::People,
    ];

    /// The chip label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Messages => "Messages",
            Self::Alerts => "Alerts",
            Self::Calls => "Calls",
            Self::Files => "Files",
            Self::People => "People",
        }
    }

    /// Whether an [`ActivityEntry`](mde_collab_types::ActivityEntry) with the
    /// stable `kind_tag` (matching
    /// [`CollabEventKind::tag`](mde_collab_types::CollabEventKind::tag)) falls in
    /// this band. [`All`](Self::All) matches everything.
    #[must_use]
    pub fn matches(self, kind_tag: &str) -> bool {
        match self {
            Self::All => true,
            Self::Messages => kind_tag.starts_with("message_") || kind_tag.starts_with("thread_"),
            Self::Alerts => kind_tag.starts_with("alert_"),
            Self::Calls => kind_tag.starts_with("call_"),
            Self::Files => kind_tag.starts_with("file_") || kind_tag.starts_with("transfer_"),
            Self::People => {
                kind_tag.starts_with("member_")
                    || kind_tag.starts_with("presence_")
                    || kind_tag.starts_with("space_")
            }
        }
    }
}

/// The Communications surface widget.
///
/// Holds **only view state** — the picked space, the active mode + filter, the
/// open thread, and locally-persisted composer drafts. It never holds a
/// projection or an authoritative value; those are read through [`CollabData`]
/// each frame and commands go out through [`CommandSink`]. Construct one per
/// mount and call [`ui`](Self::ui) each frame.
#[derive(Debug, Default)]
pub struct CommunicationsSurface {
    /// The space shown in every non-rail pane (defaults to the first rail row).
    selected_space: Option<SpaceId>,
    /// The active mode tab.
    mode: Mode,
    /// The selected Teams-style app rail route.
    app: MeshTeamsApp,
    /// The selected tab inside the Teams/channel workspace.
    channel_tab: ChannelTab,
    /// The active Activity filter band.
    activity_filter: ActivityFilter,
    /// The thread anchored open in Messages mode, if any.
    open_thread: Option<ThreadId>,
    /// Per-space main-composer drafts — persist locally across mode/space
    /// switches (a switched-away draft is never lost).
    drafts: HashMap<SpaceId, String>,
    /// Per-space task composer drafts.
    task_drafts: HashMap<SpaceId, String>,
    /// The source post attached to a per-space task draft. This is view state
    /// until the user submits the draft; the signed task command carries the
    /// source event only after that explicit action.
    task_sources: HashMap<SpaceId, EventId>,
    /// Per-space current-channel find text. This is a local view filter, not a
    /// collaboration event and not a suite-wide/global search index.
    channel_find: HashMap<SpaceId, String>,
    /// A task's source post to bring into view after the Tasks → Posts jump.
    /// The source remains a projection lookup, never a locally fabricated post.
    focused_message: Option<(SpaceId, EventId)>,
    /// Per-message local quick reaction. This is strictly view state for this
    /// seat; it is never emitted as a collaboration command/event and does not
    /// imply a mesh-visible reaction system.
    local_reactions: HashMap<EventId, messages::LocalReaction>,
    /// Per-thread reply-composer drafts.
    thread_drafts: HashMap<ThreadId, String>,
    /// The message being inline-edited (its id + the working buffer), if any.
    editing: Option<(EventId, String)>,
    /// Files mode — the open "link a file" picker's current browse directory, or
    /// `None` when the picker is closed. The picker reuses the file-manager's
    /// [`mde_files`] `LocalFsBackend` listing (§reuse).
    file_picker: Option<PathBuf>,
    /// Files mode — the pending **permanent-delete** typed-confirm, or `None`.
    /// Distinct from a plain "remove from space" (which is a single-click
    /// [`UnlinkFile`](mde_collab_types::CollabCommand::UnlinkFile)); a permanent
    /// delete is gated behind typing the file's exact name (spec: not undoable).
    files_confirm_delete: Option<files::PendingDelete>,
    /// Files mode — a transient, honest notice line (e.g. a file the picker could
    /// not read to hash). Shown once, cleared on the next successful action; never
    /// a silent swallow (§7).
    files_notice: Option<String>,
    /// Alerts mode — the local seat's least-severe level that still rings. Held as
    /// view state (the worker treats [`SetSeverityThreshold`] as a per-seat local
    /// preference, not a convergent event) and mirrored out as the command. Below
    /// this level (and, under DND, below Critical) an alert is dimmed as hushed.
    alert_threshold: Severity,
    /// Alerts mode — fleet Do-Not-Disturb: only Critical alerts ring. View state,
    /// mirrored out as [`SetDoNotDisturb`].
    alert_dnd: bool,
    /// Alerts mode — the alert sources the seat has muted (a local preference,
    /// mirrored out as [`SetAlertMute`]). A muted source's alerts are shown dimmed
    /// as hushed, never hidden (§7 — a muted alert is still a real fact).
    alert_muted_sources: BTreeSet<String>,
    /// Alerts mode — the pending **armed** destructive alert action (its alert +
    /// action id), or `None`. A destructive action arms on the first click and
    /// fires [`RunAlertAction`] with `armed: true` only on the confirm click — the
    /// same two-step gate the core's `DestructiveNotArmed` guard enforces.
    alert_arming: Option<(EventId, String)>,
    /// Clipboard mode — per-space publish-composer drafts (persist locally across
    /// mode/space switches, like the message composer draft).
    clip_drafts: HashMap<SpaceId, String>,
    /// Documents mode — the embedded editors (a one-pane Markdown Document editor +
    /// the full Project IDE editor) plus the picked-document/title + sub-mode/view
    /// toggles. Reuses `mde-editor-egui`; owns no authoritative content (the
    /// canonical Markdown lives in the editor rope and is read back on save).
    documents: documents::DocumentsState,
    /// Calls mode — the local seat's media device selection (mic/camera/screen) and
    /// its outgoing camera/screen-share intents. Seat-level **view state**: the real
    /// device enumeration and the act of binding a device to the live media plane
    /// (WebRTC/LiveKit sender) are the marked media-plane follow-up, never faked in
    /// this pure UI crate. The mic/camera/screen *mute*-vs-live *audio* mute stays a
    /// real convergent command ([`SetCallMuted`](mde_collab_types::CollabCommand::SetCallMuted)).
    call_media: calls::CallMediaPrefs,
    /// Calls mode — the call whose in-call **DTMF keypad** is open, or `None`. A
    /// per-view intent (a space switch closes it); each keypad press emits a real
    /// [`SendDtmf`](mde_collab_types::CollabCommand::SendDtmf) command.
    dtmf_pad: Option<CallId>,
    /// Auto Mode (Car Mode) — a one-shot latch tracking whether the glanceable
    /// default-to-[`Alerts`](Mode::Alerts) bias has already been applied for the
    /// current car-mode session. Set the first frame the surface sees the
    /// [`AutoSync3`](mde_egui::StyleColorScheme::AutoSync3) car skin, cleared when
    /// it leaves — so a driver lands on the alert inbox but can still navigate to
    /// any mode afterward (the bias is a default, never a lock).
    car_bias_applied: bool,
    /// Session-scoped local clipboard publication preference. The shell owns
    /// the provider; this mirrors the setting into the Mesh Teams Settings UI.
    clipboard_publishing_enabled: bool,
}

impl CommunicationsSurface {
    /// A fresh surface, defaulting to [`Mode::Activity`] with no space picked yet
    /// (the first rail row is selected on the first [`ui`](Self::ui) call).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Update the session-scoped clipboard publication preference shown by
    /// Mesh Teams Settings.
    pub fn set_clipboard_publishing_enabled(&mut self, enabled: bool) {
        self.clipboard_publishing_enabled = enabled;
    }

    /// Read the session-scoped clipboard publication preference.
    #[must_use]
    pub fn clipboard_publishing_enabled(&self) -> bool {
        self.clipboard_publishing_enabled
    }

    /// The space currently shown in the panes.
    #[must_use]
    pub fn selected_space(&self) -> Option<SpaceId> {
        self.selected_space
    }

    /// Show `space` in the panes.
    pub fn select_space(&mut self, space: SpaceId) {
        self.set_selected_space(Some(space));
    }

    /// Apply a live directory selection, clearing space-scoped view intents when
    /// membership convergence changes the selected key or removes it entirely.
    fn set_selected_space(&mut self, selected: Option<SpaceId>) {
        if self.selected_space == selected {
            return;
        }
        self.selected_space = selected;
        // A space switch closes any anchored thread + cancels an inline edit,
        // and closes the file picker + any pending permanent-delete confirm
        // (both are per-space intents); the drafts (keyed by space/thread)
        // deliberately survive.
        self.open_thread = None;
        self.focused_message = None;
        self.editing = None;
        self.file_picker = None;
        self.files_confirm_delete = None;
        self.files_notice = None;
        // A pending armed destructive alert action is a per-view intent — a
        // space switch disarms it (it must be re-armed deliberately).
        self.alert_arming = None;
        // The open in-call DTMF keypad is a per-view intent — a space switch
        // closes it. The seat-level media device prefs (mic/camera/screen)
        // deliberately survive: they are the seat's, not the space's.
        self.dtmf_pad = None;
        // The picked document is a per-space intent — reset it (the editor
        // content is replaced on the next load, so nothing stale leaks across
        // spaces). The embedded editors themselves survive as scratch state.
        self.documents.on_space_switch();
    }

    /// The active mode tab.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The active Teams-style app rail route.
    #[must_use]
    pub fn app(&self) -> MeshTeamsApp {
        self.app
    }

    /// The active tab inside the Teams/channel workspace.
    #[must_use]
    pub fn channel_tab(&self) -> ChannelTab {
        self.channel_tab
    }

    /// Switch the active mode tab.
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.app = app_for_mode(mode);
        if let Some(tab) = channel_tab_for_mode(mode) {
            self.channel_tab = tab;
        }
    }

    /// Route the taskbar's direct Editor launch into Documents with a clean,
    /// full-width editor canvas. Optional project/outline sidebars can still be
    /// reopened from the editor View menu once the workspace is open.
    pub fn open_editor(&mut self) {
        self.documents.prepare_direct_entry();
        self.set_mode(Mode::Documents);
    }

    /// Switch the Teams-style app route.
    pub fn set_app(&mut self, app: MeshTeamsApp) {
        self.app = app;
        if let Some(mode) = app.mode() {
            self.mode = if app == MeshTeamsApp::Teams {
                self.channel_tab.mode()
            } else {
                mode
            };
        }
    }

    /// Switch the Teams/channel tab and route to its existing body.
    pub fn set_channel_tab(&mut self, tab: ChannelTab) {
        self.focused_message = None;
        self.channel_tab = tab;
        self.app = MeshTeamsApp::Teams;
        self.mode = tab.mode();
    }

    /// The active Activity filter band.
    #[must_use]
    pub fn activity_filter(&self) -> ActivityFilter {
        self.activity_filter
    }

    /// The main-composer draft for `space` (empty when there is none).
    #[must_use]
    pub fn draft(&self, space: SpaceId) -> &str {
        self.drafts.get(&space).map_or("", String::as_str)
    }

    /// Set the main-composer draft for `space` (used by the shell to seed a draft
    /// and by tests to stage composer text).
    pub fn set_draft(&mut self, space: SpaceId, text: impl Into<String>) {
        self.drafts.insert(space, text.into());
    }

    /// The task-composer draft for `space` (empty when there is none).
    #[must_use]
    pub fn task_draft(&self, space: SpaceId) -> &str {
        self.task_drafts.get(&space).map_or("", String::as_str)
    }

    /// Set the task-composer draft for `space`.
    pub fn set_task_draft(&mut self, space: SpaceId, text: impl Into<String>) {
        self.task_drafts.insert(space, text.into());
        self.task_sources.remove(&space);
    }

    /// Seed the task composer from a projected post and retain its source event
    /// until the operator explicitly submits the bounded draft.
    pub(crate) fn begin_task_from_message(
        &mut self,
        space: SpaceId,
        message: EventId,
        title: impl Into<String>,
    ) {
        self.task_drafts.insert(space, title.into());
        self.task_sources.insert(space, message);
        self.set_channel_tab(ChannelTab::Tasks);
    }

    /// Return the post focus requested by a task source jump.
    #[cfg(test)]
    pub(crate) fn focused_message_for_test(&self) -> Option<(SpaceId, EventId)> {
        self.focused_message
    }

    /// Move from a projected task back to its source post. The post is only
    /// focused; the next render still resolves it from the retained projection.
    pub(crate) fn focus_task_source(&mut self, space: SpaceId, message: EventId) {
        self.select_space(space);
        self.set_channel_tab(ChannelTab::Posts);
        self.focused_message = Some((space, message));
    }

    /// The current-channel find query for `space`.
    #[must_use]
    pub fn channel_find(&self, space: SpaceId) -> &str {
        self.channel_find.get(&space).map_or("", String::as_str)
    }

    /// Set the current-channel find query for `space`. This is local-only view
    /// state and never emits a collaboration command.
    pub fn set_channel_find(&mut self, space: SpaceId, text: impl Into<String>) {
        self.channel_find.insert(space, text.into());
    }

    /// The local-only quick reaction this seat has attached to `message`, if any.
    #[must_use]
    pub(crate) fn local_reaction(&self, message: EventId) -> Option<messages::LocalReaction> {
        self.local_reactions.get(&message).copied()
    }

    /// Toggle this seat's local-only quick reaction for a message. Re-clicking
    /// the same reaction clears it; choosing a different reaction replaces it.
    pub(crate) fn toggle_local_reaction(
        &mut self,
        message: EventId,
        reaction: messages::LocalReaction,
    ) {
        if self.local_reactions.get(&message).copied() == Some(reaction) {
            self.local_reactions.remove(&message);
        } else {
            self.local_reactions.insert(message, reaction);
        }
    }

    /// The stable egui id of `space`'s main composer text field — a fixed id so a
    /// caller (or a headless test) can request focus on it deterministically.
    #[must_use]
    pub fn composer_edit_id(&self, space: SpaceId) -> egui::Id {
        egui::Id::new(("mde-collab-composer", space.as_uuid()))
    }

    /// The stable egui id of `thread`'s reply-composer text field.
    #[must_use]
    pub fn thread_composer_edit_id(&self, thread: ThreadId) -> egui::Id {
        egui::Id::new(("mde-collab-thread-composer", thread.as_uuid()))
    }

    #[cfg(test)]
    pub(crate) fn open_thread_for_test(&mut self, thread: ThreadId) {
        self.open_thread = Some(thread);
    }

    #[cfg(test)]
    pub(crate) fn thread_draft_for_test(&self, thread: ThreadId) -> &str {
        self.thread_drafts.get(&thread).map_or("", String::as_str)
    }

    /// Render the whole surface inside `ui`: the app rail, Teams + Channels
    /// rail, channel header, persistent call bar, and active app body. Reads projections from
    /// `data` and pushes every emitted command into `sink`.
    pub fn ui(&mut self, ui: &mut egui::Ui, data: &dyn CollabData, sink: &mut CommandSink) {
        if std::env::var_os("MDE_DRM_LINEAR_SCANOUT").is_some() {
            let rect = ui.max_rect();
            eprintln!(
                "communications viewport proof: available={}x{}, rect=({}, {})..({}, {})",
                ui.available_width(),
                ui.available_height(),
                rect.min.x,
                rect.min.y,
                rect.max.x,
                rect.max.y
            );
        }
        // Reconcile the retained selection with the newest directory so every
        // pane follows the same key when membership removal or re-enrollment
        // advances the read model between frames.
        let selected =
            frame::reconciled_selected_space(self.selected_space, data.space_directory());
        if selected != self.selected_space {
            self.set_selected_space(selected);
        }
        if std::env::var_os("MDE_DRM_LINEAR_SCANOUT").is_some() {
            eprintln!(
                "communications state proof: mode={:?}, app={:?}, selected_space={:?}",
                self.mode, self.app, self.selected_space
            );
        }

        // Auto Mode (Car Mode): on entering the Ford SYNC 3 car dash, land a driver
        // on the glanceable Alerts inbox instead of a dense manage-heavy pane. A
        // one-shot latch (re-armed on leaving car mode) so this is a *default*, not
        // a lock — the driver can still switch to any mode afterward. The non-car
        // surface keeps its own default untouched.
        if car_mode(ui) {
            if !self.car_bias_applied {
                if self.mode.is_dense() {
                    self.set_mode(Mode::Alerts);
                }
                self.car_bias_applied = true;
            }
        } else {
            self.car_bias_applied = false;
        }

        // Construct owns one shared workspace identity strip. The channel/app
        // header below remains Mesh Teams domain chrome; it must not also carry
        // the host workspace title or shell session control.
        let _ = mde_egui::nav_chrome::AppFrame::new("Mesh Teams")
            .leading_title()
            .show(ui);
        ui.add_space(mde_egui::Style::SP_XS);

        // Keep the workspace body usable on narrow direct-DRM seats. The
        // Communications rails are supporting navigation; the shell's shared
        // dock remains available for route changes while a narrow workspace
        // gives the active document canvas the full width. Without this
        // breakpoint, the nested fixed panels consume the available width and
        // egui quietly collapses the editor body to zero.
        let narrow = ui.available_width() < 1024.0;
        if !narrow {
            egui::SidePanel::left(ui.id().with("collab-app-rail"))
                .resizable(false)
                .exact_width(frame::APP_RAIL_W)
                .frame(frame::rail_frame())
                .show_inside(ui, |ui| self.app_rail(ui));

            egui::SidePanel::left(ui.id().with("collab-channel-rail"))
                .resizable(false)
                .exact_width(frame::CHANNEL_RAIL_W)
                .frame(frame::rail_frame())
                .show_inside(ui, |ui| self.rail(ui, data, sink));

            egui::SidePanel::right(ui.id().with("collab-details"))
                .resizable(false)
                .exact_width(frame::DETAILS_W)
                .frame(frame::rail_frame())
                .show_inside(ui, |ui| self.details_pane(ui, data));
        }

        if !narrow {
            // The call bar is added before the tabs + body so it stays pinned to
            // the bottom regardless of which mode is showing — it survives
            // every switch. At narrow widths the shell dock/status chrome is
            // the compact interaction surface and these nested bars are
            // omitted so they cannot consume the active canvas height.
            egui::TopBottomPanel::bottom(ui.id().with("collab-callbar"))
                .frame(frame::bar_frame())
                .show_inside(ui, |ui| self.call_bar(ui, data, sink));

            egui::TopBottomPanel::top(ui.id().with("collab-channel-header"))
                .frame(frame::bar_frame())
                .show_inside(ui, |ui| self.channel_header(ui, data));
        }

        // The mode body crossfades in on a switch (lock #4) rather than swapping
        // instantly — a distance-independent fade on the shared Page tier, wrapped
        // around the same per-mode body render.
        let mode_slot = anim::mode_index(self.mode);
        let body_size = ui.available_size();
        if std::env::var_os("MDE_DRM_LINEAR_SCANOUT").is_some() {
            eprintln!(
                "communications prebody proof: available={}x{}, rect={:?}",
                body_size.x,
                body_size.y,
                ui.max_rect()
            );
        }
        ui.allocate_ui_with_layout(body_size, egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_min_size(body_size);
            frame::body_frame().show(ui, |ui| {
                if std::env::var_os("MDE_DRM_LINEAR_SCANOUT").is_some() {
                    eprintln!(
                        "communications central proof: available={}x{}, rect={:?}",
                        ui.available_width(),
                        ui.available_height(),
                        ui.max_rect()
                    );
                }
                anim::switch_body(ui, mode_slot, |ui| self.mode_body(ui, data, sink));
            });
        });
    }

    /// The active mode's central body.
    fn mode_body(&mut self, ui: &mut egui::Ui, data: &dyn CollabData, sink: &mut CommandSink) {
        if self.app == MeshTeamsApp::Settings {
            self.settings_body(ui, data, sink);
            return;
        }
        match self.mode {
            Mode::Activity => self.activity_body(ui, data),
            Mode::Messages => self.messages_body(ui, data, sink),
            Mode::Calls => self.calls_body(ui, data, sink),
            Mode::Tasks => self.tasks_body(ui, data, sink),
            Mode::Files => self.files_body(ui, data, sink),
            Mode::Transfers => self.transfers_body(ui, data, sink),
            Mode::Documents => self.documents_body(ui, data, sink),
            Mode::Alerts => self.alerts_body(ui, data, sink),
            Mode::Clipboard => self.clipboard_body(ui, data, sink),
        }
    }

    /// Local Mesh Teams settings pane. These are real preferences already wired
    /// through the Alerts command lane plus read-only provider/bridge status
    /// seams. It never calls a provider or invents device/server rows.
    fn settings_body(&mut self, ui: &mut egui::Ui, data: &dyn CollabData, sink: &mut CommandSink) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.heading("Mesh Teams Settings");
                ui.label(
                    egui::RichText::new("Local notification preferences for this seat.")
                        .color(mde_egui::Style::TEXT_DIM),
                );
                ui.add_space(mde_egui::Style::SP_S);
                self.alert_pref_bar(ui, sink);
                ui.add_space(mde_egui::Style::SP_M);
                ui.label(
                    egui::RichText::new("Clipboard privacy")
                        .strong()
                        .color(mde_egui::Style::TEXT_STRONG),
                );
                let response = ui.checkbox(
                    &mut self.clipboard_publishing_enabled,
                    "Publish local clipboard copies to Mesh Teams",
                );
                let _ = response.comms_hover_text(
                    "Off by default for new sessions. Remote clipboard history remains visible when off; enabling publishes only new local copies.",
                );
                ui.label(
                    egui::RichText::new("This setting applies only to the current session.")
                        .small()
                        .color(mde_egui::Style::TEXT_DIM),
                );
                ui.add_space(mde_egui::Style::SP_M);
                ui.label(
                    egui::RichText::new("Provider devices")
                        .strong()
                        .color(mde_egui::Style::TEXT_STRONG),
                );
                ui.label(
                    egui::RichText::new(
                        "Visible but disabled until the live media provider enumerates microphone, \
                         camera, and screen sources.",
                    )
                    .color(mde_egui::Style::TEXT_DIM),
                );
                ui.add_space(mde_egui::Style::SP_S);
                self.call_device_row(ui);
                ui.add_space(mde_egui::Style::SP_M);
                discord_bridge_settings(ui, data);
            });
    }
}

fn discord_bridge_settings(ui: &mut egui::Ui, data: &dyn CollabData) {
    ui.label(
        egui::RichText::new("Discord bridge")
            .strong()
            .color(mde_egui::Style::TEXT_STRONG),
    );
    ui.label(
        egui::RichText::new(
            "Read-only bridge status from the mesh worker. No Discord provider is called here, \
             and no server is shown unless a bridge row was projected.",
        )
        .color(mde_egui::Style::TEXT_DIM),
    );
    ui.add_space(mde_egui::Style::SP_S);
    for row in crate::frame::discord_bridge_rows_for_settings(data.discord_bridge_board()) {
        egui::Frame::NONE
            .fill(mde_egui::Style::LAYER_01)
            .stroke(egui::Stroke::new(
                mde_egui::Style::STROKE_HAIRLINE,
                mde_egui::Style::BORDER,
            ))
            .inner_margin(mde_egui::Style::SP_S)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(row.label.as_str())
                            .strong()
                            .color(mde_egui::Style::TEXT),
                    );
                    ui.label(
                        egui::RichText::new(row.status)
                            .small()
                            .color(discord_bridge_status_color(row.status)),
                    );
                });
                bridge_status_line(ui, "Discord → Mesh", row.inbound);
                bridge_status_line(ui, "Mesh → Discord", row.outbound);
                bridge_status_line(ui, "Provenance", row.provenance.as_str());
                if let Some(detail) = row.detail.as_deref() {
                    ui.label(
                        egui::RichText::new(detail)
                            .small()
                            .color(mde_egui::Style::TEXT_DIM),
                    );
                }
            });
        ui.add_space(mde_egui::Style::SP_XS);
    }
}

fn bridge_status_line(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(label)
                .small()
                .color(mde_egui::Style::TEXT_DIM),
        );
        ui.label(
            egui::RichText::new(value)
                .small()
                .color(mde_egui::Style::TEXT),
        );
    });
}

fn discord_bridge_status_color(status: &str) -> egui::Color32 {
    match status {
        "Configured" => mde_egui::Style::OK,
        "Provider unavailable" => mde_egui::Style::WARN,
        _ => mde_egui::Style::TEXT_DIM,
    }
}

/// Whether the surface is running on the in-vehicle **Ford SYNC 3** car-dash skin
/// ([`AutoSync3`](mde_egui::StyleColorScheme::AutoSync3)) — installed by the shell
/// only while Car Mode is active. When true the surface takes a conservative,
/// glanceable Auto Mode treatment: it lands a driver on the Alerts inbox (see
/// [`ui`](CommunicationsSurface::ui)) and enlarges the alert rows + persistent
/// call bar so they read at a glance. The non-car surface reads `false` here and
/// renders exactly as before — every branch is glue, never a replacement.
pub(crate) fn car_mode(ui: &egui::Ui) -> bool {
    mde_egui::Style::color_scheme(ui.ctx()) == mde_egui::StyleColorScheme::AutoSync3
}

/// The maximum number of cards a moving driver should scan in one glance.
/// This mirrors the shell's published `car_motion_policy` seam without making
/// the pure Communications crate depend on the shell crate.
pub(crate) const CAR_GLANCE_LIST_MAX: usize = 6;

/// Bound one car-path list when the shell has published the live in-motion fold.
/// A missing fold or non-Car palette leaves desktop behavior unchanged.
#[must_use]
pub(crate) const fn bounded_car_list_len(is_car: bool, in_motion: bool, full_len: usize) -> usize {
    if is_car && in_motion {
        if full_len < CAR_GLANCE_LIST_MAX {
            full_len
        } else {
            CAR_GLANCE_LIST_MAX
        }
    } else {
        full_len
    }
}

/// Read the shell's same-frame in-motion publication for a Communications
/// surface. The string is the documented Context seam owned by the shell's
/// `car_motion_policy`; absent state is deliberately the unrestricted default.
#[must_use]
pub(crate) fn car_glance_limit(ui: &egui::Ui, full_len: usize) -> usize {
    let in_motion = ui.ctx().data(|data| {
        data.get_temp::<bool>(egui::Id::new("mcnf-car-motion-in-motion"))
            .unwrap_or(false)
    });
    bounded_car_list_len(car_mode(ui), in_motion, full_len)
}
