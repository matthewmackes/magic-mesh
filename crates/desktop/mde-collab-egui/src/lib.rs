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
//! * a **spaces rail** down the left, listing the
//!   [`SpaceDirectory`](mde_collab_types::SpaceDirectory) with per-space unread
//!   badges (the selection key for every other pane);
//! * per-space **mode tabs** across the top — [`Mode::Activity`],
//!   [`Mode::Messages`], [`Mode::Calls`], [`Mode::Files`], [`Mode::Transfers`],
//!   [`Mode::Documents`], [`Mode::Alerts`], and [`Mode::Clipboard`] are all
//!   implemented. Documents
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
//!   composer whose <kbd>Enter</kbd> emits
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
mod calls;
mod clipboard;
mod data;
mod documents;
mod files;
mod fixture;
mod frame;
mod icons;
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
    pub const TABS: [Self; 8] = [
        Self::Activity,
        Self::Messages,
        Self::Calls,
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
            | Self::Files
            | Self::Transfers
            | Self::Documents
            | Self::Alerts
            | Self::Clipboard => true,
        }
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
    /// The active Activity filter band.
    activity_filter: ActivityFilter,
    /// The thread anchored open in Messages mode, if any.
    open_thread: Option<ThreadId>,
    /// Per-space main-composer drafts — persist locally across mode/space
    /// switches (a switched-away draft is never lost).
    drafts: HashMap<SpaceId, String>,
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
}

impl CommunicationsSurface {
    /// A fresh surface, defaulting to [`Mode::Activity`] with no space picked yet
    /// (the first rail row is selected on the first [`ui`](Self::ui) call).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The space currently shown in the panes.
    #[must_use]
    pub fn selected_space(&self) -> Option<SpaceId> {
        self.selected_space
    }

    /// Show `space` in the panes.
    pub fn select_space(&mut self, space: SpaceId) {
        if self.selected_space != Some(space) {
            self.selected_space = Some(space);
            // A space switch closes any anchored thread + cancels an inline edit,
            // and closes the file picker + any pending permanent-delete confirm
            // (both are per-space intents); the drafts (keyed by space/thread)
            // deliberately survive.
            self.open_thread = None;
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
    }

    /// The active mode tab.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Switch the active mode tab.
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
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

    /// The stable egui id of `space`'s main composer text field — a fixed id so a
    /// caller (or a headless test) can request focus on it deterministically.
    #[must_use]
    pub fn composer_edit_id(&self, space: SpaceId) -> egui::Id {
        egui::Id::new(("mde-collab-composer", space.as_uuid()))
    }

    /// Render the whole surface inside `ui`: the spaces rail, the mode tabs, the
    /// persistent call bar, and the active mode body. Reads projections from
    /// `data` and pushes every emitted command into `sink`.
    pub fn ui(&mut self, ui: &mut egui::Ui, data: &dyn CollabData, sink: &mut CommandSink) {
        // Default the selection to the first rail row so the frame is usable the
        // moment a directory exists.
        if self.selected_space.is_none() {
            self.selected_space = data.space_directory().spaces.first().map(|s| s.id);
        }

        egui::SidePanel::left(ui.id().with("collab-rail"))
            .resizable(false)
            .exact_width(frame::RAIL_W)
            .frame(frame::rail_frame())
            .show_inside(ui, |ui| self.rail(ui, data, sink));

        // The call bar is added before the tabs + body so it stays pinned to the
        // bottom regardless of which mode is showing — it survives every switch.
        egui::TopBottomPanel::bottom(ui.id().with("collab-callbar"))
            .frame(frame::bar_frame())
            .show_inside(ui, |ui| self.call_bar(ui, data, sink));

        egui::TopBottomPanel::top(ui.id().with("collab-tabs"))
            .frame(frame::bar_frame())
            .show_inside(ui, |ui| self.mode_tabs(ui));

        egui::CentralPanel::default()
            .frame(frame::body_frame())
            .show_inside(ui, |ui| self.mode_body(ui, data, sink));
    }

    /// The active mode's central body.
    fn mode_body(&mut self, ui: &mut egui::Ui, data: &dyn CollabData, sink: &mut CommandSink) {
        match self.mode {
            Mode::Activity => self.activity_body(ui, data),
            Mode::Messages => self.messages_body(ui, data, sink),
            Mode::Calls => self.calls_body(ui, data, sink),
            Mode::Files => self.files_body(ui, data, sink),
            Mode::Transfers => self.transfers_body(ui, data, sink),
            Mode::Documents => self.documents_body(ui, data, sink),
            Mode::Alerts => self.alerts_body(ui, data, sink),
            Mode::Clipboard => self.clipboard_body(ui, data, sink),
        }
    }
}
