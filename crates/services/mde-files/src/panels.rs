//! Phase 1.4 / 1.5 / 1.7 — auxiliary panel state for the mde-files
//! workspace.
//!
//! Three small state machines:
//!
//!   * [`DetailsPanel`] (Phase 1.4) — visibility + content cache
//!     for the right-side metadata pane. Surfaces metadata,
//!     permissions, mesh availability, and operation history for
//!     the focused row. Open/close + content swap go through
//!     dedicated methods so the view code stays a pure function of
//!     state.
//!   * [`ContextMenu`] (Phase 1.5) — right-click menu model. Holds
//!     the open/closed flag plus the row the menu was opened over
//!     (the menu's commands act on that row). The locked item list
//!     is `[Open, CopyPath, SendTo, Rename, Delete, Properties]`.
//!   * [`OperationDrawer`] (Phase 1.7) — slide-up drawer showing
//!     in-flight transfer operations. State: visibility flag +
//!     ordered list of [`OpRow`] (one row per active op + the last
//!     few completed ones).
//!
//! All three are pure data — no Iced widgets here. The view layer
//! imports them and renders accordingly. This keeps the unit tests
//! out of the wgpu / Wayland event-loop dep chain.

use std::collections::VecDeque;

// =========================================================================
// Phase 1.4 — Details panel.
// =========================================================================

/// Right-side details panel. Open/closed + the row whose metadata
/// is currently being shown. The content itself is rendered by the
/// view code, which consults the backend for the latest metadata
/// each frame — we don't cache the metadata here so a refresh can't
/// surface stale fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DetailsPanel {
    /// `true` while the panel is visible.
    open: bool,
    /// The row whose details are showing. `None` when the panel is
    /// closed OR when nothing is focused yet.
    target: Option<String>,
}

impl DetailsPanel {
    /// Construct a closed panel.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Open the panel on `target`. Idempotent.
    pub fn open(&mut self, target: impl Into<String>) {
        self.open = true;
        self.target = Some(target.into());
    }

    /// Close the panel + drop the target reference.
    pub fn close(&mut self) {
        self.open = false;
        self.target = None;
    }

    /// Toggle visibility. When opening with no current target,
    /// stays closed (per the Phase 1.4 lock: "hidden when nothing
    /// selected").
    pub fn toggle(&mut self, focused: Option<&str>) {
        if self.open {
            self.close();
        } else if let Some(t) = focused {
            self.open(t);
        }
    }

    /// Tell the panel about a new focus target. If the panel is
    /// open, the target follows; if it's closed, the target is
    /// remembered so a subsequent `toggle()` opens on the right
    /// row.
    pub fn set_target(&mut self, target: Option<&str>) {
        if self.open {
            self.target = target.map(str::to_string);
            // Auto-close when focus moves to "nothing"; the design
            // lock says the panel hides when nothing is selected.
            if self.target.is_none() {
                self.open = false;
            }
        } else {
            // Keep the remembered target so toggle restores it.
            self.target = target.map(str::to_string);
        }
    }

    /// `true` while the panel is visible.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Currently-displayed row, if any.
    #[must_use]
    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }
}

// =========================================================================
// Phase 1.5 — Right-click context menu.
// =========================================================================

/// Right-click context-menu state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ContextMenu {
    /// `true` while the menu is open.
    open: bool,
    /// The row the menu was opened over. All commands the user
    /// selects act on this row.
    row: Option<String>,
    /// Anchor position in window coords, used by the view to place
    /// the floating menu.
    anchor: Option<(f32, f32)>,
}

impl ContextMenu {
    /// Construct a closed menu.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Open the menu over `row` at the given window-coord anchor.
    pub fn open(&mut self, row: impl Into<String>, anchor: (f32, f32)) {
        self.open = true;
        self.row = Some(row.into());
        self.anchor = Some(anchor);
    }

    /// Close the menu.
    pub fn close(&mut self) {
        self.open = false;
        self.row = None;
        self.anchor = None;
    }

    /// `true` while the menu is open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Row the menu was opened over.
    #[must_use]
    pub fn row(&self) -> Option<&str> {
        self.row.as_deref()
    }

    /// Window-coord anchor where the floating menu should render.
    #[must_use]
    pub fn anchor(&self) -> Option<(f32, f32)> {
        self.anchor
    }

    /// Canonical item set. Locked 2026-05-19 per the v2.0.0 mde-
    /// files design spec (`docs/design/v2.0.0-mde-files/design-
    /// spec.md` § Phase 1.5).
    #[must_use]
    pub fn items(&self) -> &'static [ContextMenuItem] {
        &[
            ContextMenuItem::Open,
            ContextMenuItem::CopyPath,
            ContextMenuItem::SendTo,
            ContextMenuItem::Rename,
            ContextMenuItem::Delete,
            ContextMenuItem::Properties,
        ]
    }
}

/// One entry in the right-click menu. Locked set per the design
/// spec; the view code keys SVG icons + labels off this enum so
/// "add a new menu item" is a one-place change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMenuItem {
    /// Launch the file in its default application.
    Open,
    /// Copy the file's absolute path to the clipboard.
    CopyPath,
    /// Open the Send To submenu (Phase 3.1 entry point).
    SendTo,
    /// Inline rename — the row enters edit mode.
    Rename,
    /// Move to trash (Phase 4.2 trash adapter handles the actual
    /// op).
    Delete,
    /// Open the Properties dialog.
    Properties,
}

impl ContextMenuItem {
    /// User-facing label. Locked copy — changes to the design spec
    /// must update both the spec and this method.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::CopyPath => "Copy path",
            Self::SendTo => "Send to…",
            Self::Rename => "Rename",
            Self::Delete => "Delete", // voice-allow:destroy (file deletion is destroy, not set-removal)
            Self::Properties => "Properties",
        }
    }

    /// Whether this item is destructive (renders in red, requires
    /// confirm).
    #[must_use]
    pub const fn is_destructive(self) -> bool {
        matches!(self, Self::Delete)
    }
}

// =========================================================================
// Phase 1.6 — Drag-and-drop state.
// =========================================================================

/// Drag-and-drop session state. Tracks "the user is dragging row X
/// and is currently hovering over target Y". The view layer reads
/// this to render the drag pill + the highlighted drop target;
/// `MdeFiles::update` translates `Drop(target)` into a
/// `Backend::send_to(target, Copy, Ask)` call.
///
/// All fields are pure data — actual mouse-event handling lives
/// with the Iced widget bindings, not here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DragSession {
    /// Currently-dragged row keys (file identifiers). Empty when
    /// no drag is in progress. Multiple when the user grabbed a
    /// multi-selection.
    sources: Vec<String>,
    /// Drop target currently under the cursor, if any. Tracked so
    /// the view can highlight it.
    hover_target: Option<DragTarget>,
}

/// Where a drag is currently hovered. The set is locked — Phase
/// 1.6 only supports dropping onto sidebar peers / groups / sites
/// because that's all `Backend::send_to` accepts (per the Phase
/// 3.x Destination enum). Local-filesystem drops route through
/// the Iced widget's file-drop bridge, not this state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DragTarget {
    /// One named peer.
    Peer(String),
    /// Peer group by name.
    Group(String),
    /// Peers by role.
    Role(String),
    /// Peers in a region/site.
    Site(String),
}

impl DragSession {
    /// Start a drag from the given row keys. Replaces any
    /// in-flight drag.
    pub fn start<I, S>(&mut self, sources: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.sources = sources.into_iter().map(Into::into).collect();
        self.hover_target = None;
    }

    /// Update the hover target as the cursor moves.
    pub fn set_hover(&mut self, target: Option<DragTarget>) {
        self.hover_target = target;
    }

    /// Cancel the drag (user pressed Escape, etc.). Returns the
    /// in-flight row count so the caller can show a brief
    /// "cancelled" message.
    pub fn cancel(&mut self) -> usize {
        let n = self.sources.len();
        self.sources.clear();
        self.hover_target = None;
        n
    }

    /// Complete the drag — returns `(sources, target)` if a target
    /// is under the cursor, or `None` if the drop happened over
    /// empty space (treated as cancel).
    pub fn finish(&mut self) -> Option<(Vec<String>, DragTarget)> {
        let target = self.hover_target.clone()?;
        let sources = std::mem::take(&mut self.sources);
        self.hover_target = None;
        Some((sources, target))
    }

    /// `true` while a drag is in progress.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.sources.is_empty()
    }

    /// Currently-dragged row keys.
    #[must_use]
    pub fn sources(&self) -> &[String] {
        &self.sources
    }

    /// Currently-hovered drop target, if any.
    #[must_use]
    pub fn hover_target(&self) -> Option<&DragTarget> {
        self.hover_target.as_ref()
    }
}

// =========================================================================
// Phase 1.7 — Operation drawer.
// =========================================================================

/// One row in the operation drawer. Fields mirror the backend's
/// `AuditEntry` for completed ops + add a progress field for
/// in-flight ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpRow {
    /// Stable op id (allocated by the backend).
    pub op_id: u64,
    /// Human-friendly source label (filename or "5 files").
    pub source: String,
    /// Human-friendly destination label ("pine.mesh" or "Audio group").
    pub destination: String,
    /// Progress as a permille (0..=1000). 1000 = complete.
    pub progress_permille: u16,
    /// Op state — drives the badge + which controls are enabled.
    pub state: OpState,
}

/// State of one transfer operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpState {
    /// Waiting on the queue (haven't started transferring yet).
    Queued,
    /// Actively transferring.
    Running,
    /// Successfully completed.
    Completed,
    /// Failed; user can retry or dismiss.
    Failed,
    /// Cancelled by the user.
    Cancelled,
}

impl OpState {
    /// `true` when the op is still running or queued.
    #[must_use]
    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }

    /// `true` when the user-visible state is "this op is over".
    #[must_use]
    pub fn is_terminal(self) -> bool {
        !self.is_active()
    }

    /// Whether the Cancel button should be enabled.
    #[must_use]
    pub fn can_cancel(self) -> bool {
        self.is_active()
    }

    /// Whether the Retry button should be enabled.
    #[must_use]
    pub fn can_retry(self) -> bool {
        matches!(self, Self::Failed | Self::Cancelled)
    }
}

/// Slide-up drawer state. Holds visibility + the ordered op list.
/// New ops push to the front; the most recent 32 (completed +
/// active) are kept so the drawer doesn't grow unbounded.
#[derive(Debug, Clone, Default)]
pub struct OperationDrawer {
    open: bool,
    ops: VecDeque<OpRow>,
}

/// Max ops retained — completed ops past this fall off the back.
pub const OP_DRAWER_CAPACITY: usize = 32;

impl OperationDrawer {
    /// Construct an empty drawer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Show or hide the drawer.
    pub fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    /// `true` while the drawer is visible.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Add a new op (or update an existing one with the same
    /// `op_id`). Inserts at the front so newest is visible first.
    pub fn upsert(&mut self, row: OpRow) {
        if let Some(idx) = self.ops.iter().position(|r| r.op_id == row.op_id) {
            self.ops[idx] = row;
        } else {
            self.ops.push_front(row);
            while self.ops.len() > OP_DRAWER_CAPACITY {
                self.ops.pop_back();
            }
        }
    }

    /// Remove the op with the given id, if present. Returns
    /// `true` if a row was removed.
    pub fn dismiss(&mut self, op_id: u64) -> bool {
        if let Some(idx) = self.ops.iter().position(|r| r.op_id == op_id) {
            self.ops.remove(idx);
            true
        } else {
            false
        }
    }

    /// All op rows, newest first.
    #[must_use]
    pub fn rows(&self) -> Vec<OpRow> {
        self.ops.iter().cloned().collect()
    }

    /// Active op count (queued + running).
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.ops.iter().filter(|r| r.state.is_active()).count()
    }

    /// Total op count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// `true` when no ops are tracked.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- Phase 1.4 ----------

    #[test]
    fn details_panel_starts_closed() {
        let d = DetailsPanel::new();
        assert!(!d.is_open());
        assert!(d.target().is_none());
    }

    #[test]
    fn details_panel_open_then_close() {
        let mut d = DetailsPanel::new();
        d.open("notes.md");
        assert!(d.is_open());
        assert_eq!(d.target(), Some("notes.md"));
        d.close();
        assert!(!d.is_open());
        assert!(d.target().is_none());
    }

    #[test]
    fn details_panel_toggle_requires_focused_row_to_open() {
        let mut d = DetailsPanel::new();
        d.toggle(None);
        assert!(!d.is_open(), "Phase 1.4 lock: hidden when nothing selected");
        d.toggle(Some("a.txt"));
        assert!(d.is_open());
        assert_eq!(d.target(), Some("a.txt"));
        d.toggle(None);
        assert!(!d.is_open(), "second toggle closes");
    }

    #[test]
    fn details_panel_set_target_when_open_follows_focus() {
        let mut d = DetailsPanel::new();
        d.open("a");
        d.set_target(Some("b"));
        assert_eq!(d.target(), Some("b"));
        assert!(d.is_open());
    }

    #[test]
    fn details_panel_auto_closes_when_focus_clears_while_open() {
        let mut d = DetailsPanel::new();
        d.open("a");
        d.set_target(None);
        assert!(!d.is_open(), "panel must hide when focus clears");
        assert!(d.target().is_none());
    }

    #[test]
    fn details_panel_remembers_target_while_closed() {
        let mut d = DetailsPanel::new();
        d.set_target(Some("a"));
        assert!(!d.is_open());
        let remembered = d.target().map(str::to_string);
        d.toggle(remembered.as_deref());
        assert!(d.is_open());
        assert_eq!(d.target(), Some("a"));
    }

    // ---------- Phase 1.5 ----------

    #[test]
    fn context_menu_starts_closed() {
        let m = ContextMenu::new();
        assert!(!m.is_open());
        assert!(m.row().is_none());
        assert!(m.anchor().is_none());
    }

    #[test]
    fn context_menu_open_records_row_and_anchor() {
        let mut m = ContextMenu::new();
        m.open("notes.md", (100.0, 200.0));
        assert!(m.is_open());
        assert_eq!(m.row(), Some("notes.md"));
        assert_eq!(m.anchor(), Some((100.0, 200.0)));
    }

    #[test]
    fn context_menu_close_drops_everything() {
        let mut m = ContextMenu::new();
        m.open("a", (0.0, 0.0));
        m.close();
        assert!(!m.is_open());
        assert!(m.row().is_none());
        assert!(m.anchor().is_none());
    }

    #[test]
    fn context_menu_locked_item_set() {
        let m = ContextMenu::new();
        let items = m.items();
        assert_eq!(items.len(), 6);
        assert!(items.contains(&ContextMenuItem::Open));
        assert!(items.contains(&ContextMenuItem::CopyPath));
        assert!(items.contains(&ContextMenuItem::SendTo));
        assert!(items.contains(&ContextMenuItem::Rename));
        assert!(items.contains(&ContextMenuItem::Delete));
        assert!(items.contains(&ContextMenuItem::Properties));
    }

    #[test]
    fn context_menu_labels_are_locked() {
        assert_eq!(ContextMenuItem::Open.label(), "Open");
        assert_eq!(ContextMenuItem::CopyPath.label(), "Copy path");
        assert_eq!(ContextMenuItem::SendTo.label(), "Send to…");
        assert_eq!(ContextMenuItem::Rename.label(), "Rename");
        assert_eq!(ContextMenuItem::Delete.label(), "Delete"); // voice-allow:test-data
        assert_eq!(ContextMenuItem::Properties.label(), "Properties");
    }

    #[test]
    fn context_menu_only_delete_is_destructive() {
        assert!(ContextMenuItem::Delete.is_destructive());
        for it in [
            ContextMenuItem::Open,
            ContextMenuItem::CopyPath,
            ContextMenuItem::SendTo,
            ContextMenuItem::Rename,
            ContextMenuItem::Properties,
        ] {
            assert!(!it.is_destructive(), "{it:?} must not be destructive");
        }
    }

    // ---------- Phase 1.6 ----------

    #[test]
    fn drag_session_starts_inactive() {
        let d = DragSession::default();
        assert!(!d.is_active());
        assert!(d.sources().is_empty());
        assert!(d.hover_target().is_none());
    }

    #[test]
    fn drag_session_start_records_sources() {
        let mut d = DragSession::default();
        d.start(["a.txt", "b.txt"]);
        assert!(d.is_active());
        assert_eq!(d.sources(), &["a.txt".to_string(), "b.txt".to_string()]);
    }

    #[test]
    fn drag_session_hover_set_and_finish() {
        let mut d = DragSession::default();
        d.start(["x.bin"]);
        d.set_hover(Some(DragTarget::Peer("pine".into())));
        let result = d.finish().expect("drop landed on a target");
        assert_eq!(result.0, vec!["x.bin".to_string()]);
        assert_eq!(result.1, DragTarget::Peer("pine".into()));
        // After finish, session is inactive again.
        assert!(!d.is_active());
        assert!(d.hover_target().is_none());
    }

    #[test]
    fn drag_session_finish_with_no_target_returns_none() {
        let mut d = DragSession::default();
        d.start(["x"]);
        assert!(d.finish().is_none(), "drop on empty space = cancel");
    }

    #[test]
    fn drag_session_cancel_reports_source_count() {
        let mut d = DragSession::default();
        d.start(["a", "b", "c"]);
        let n = d.cancel();
        assert_eq!(n, 3);
        assert!(!d.is_active());
    }

    #[test]
    fn drag_session_set_hover_to_none_clears() {
        let mut d = DragSession::default();
        d.start(["x"]);
        d.set_hover(Some(DragTarget::Group("audio".into())));
        d.set_hover(None);
        assert!(d.hover_target().is_none());
        // Sources are still in flight — only the hover cleared.
        assert!(d.is_active());
    }

    #[test]
    fn drag_target_variants_distinct() {
        // The destination set mirrors Backend::Destination — every
        // variant must be representable.
        let _peer = DragTarget::Peer("pine".into());
        let _group = DragTarget::Group("audio".into());
        let _role = DragTarget::Role("host".into());
        let _site = DragTarget::Site("lab".into());
    }

    // ---------- Phase 1.7 ----------

    fn op(id: u64, state: OpState, progress: u16) -> OpRow {
        OpRow {
            op_id: id,
            source: format!("file{id}.bin"),
            destination: "pine.mesh".into(),
            progress_permille: progress,
            state,
        }
    }

    #[test]
    fn op_drawer_starts_closed_and_empty() {
        let d = OperationDrawer::new();
        assert!(!d.is_open());
        assert_eq!(d.len(), 0);
        assert!(d.is_empty());
    }

    #[test]
    fn op_drawer_upsert_new_inserts_front() {
        let mut d = OperationDrawer::new();
        d.upsert(op(1, OpState::Running, 0));
        d.upsert(op(2, OpState::Queued, 0));
        let rows = d.rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].op_id, 2, "newest at front");
        assert_eq!(rows[1].op_id, 1);
    }

    #[test]
    fn op_drawer_upsert_existing_updates_in_place() {
        let mut d = OperationDrawer::new();
        d.upsert(op(1, OpState::Running, 0));
        d.upsert(op(2, OpState::Running, 0));
        d.upsert(op(1, OpState::Completed, 1000));
        let rows = d.rows();
        assert_eq!(rows.len(), 2, "no duplicate row");
        let row1 = rows.iter().find(|r| r.op_id == 1).unwrap();
        assert_eq!(row1.state, OpState::Completed);
        assert_eq!(row1.progress_permille, 1000);
    }

    #[test]
    fn op_drawer_caps_at_capacity() {
        let mut d = OperationDrawer::new();
        for i in 0..(OP_DRAWER_CAPACITY as u64 + 5) {
            d.upsert(op(i, OpState::Completed, 1000));
        }
        assert_eq!(d.len(), OP_DRAWER_CAPACITY);
        // Oldest should have been dropped — newest is at front.
        let rows = d.rows();
        let newest = rows[0].op_id;
        assert_eq!(newest, OP_DRAWER_CAPACITY as u64 + 4);
    }

    #[test]
    fn op_drawer_dismiss_removes_one() {
        let mut d = OperationDrawer::new();
        d.upsert(op(1, OpState::Running, 0));
        d.upsert(op(2, OpState::Running, 0));
        assert!(d.dismiss(1));
        assert_eq!(d.len(), 1);
        assert!(!d.dismiss(1), "second dismiss returns false");
    }

    #[test]
    fn op_drawer_active_count_only_counts_queued_and_running() {
        let mut d = OperationDrawer::new();
        d.upsert(op(1, OpState::Queued, 0));
        d.upsert(op(2, OpState::Running, 500));
        d.upsert(op(3, OpState::Completed, 1000));
        d.upsert(op(4, OpState::Failed, 200));
        d.upsert(op(5, OpState::Cancelled, 100));
        assert_eq!(d.active_count(), 2);
        assert_eq!(d.len(), 5);
    }

    #[test]
    fn op_state_predicates() {
        assert!(OpState::Queued.is_active());
        assert!(OpState::Running.is_active());
        assert!(OpState::Completed.is_terminal());
        assert!(OpState::Failed.is_terminal());
        assert!(OpState::Cancelled.is_terminal());

        assert!(OpState::Queued.can_cancel());
        assert!(OpState::Running.can_cancel());
        assert!(!OpState::Completed.can_cancel());

        assert!(OpState::Failed.can_retry());
        assert!(OpState::Cancelled.can_retry());
        assert!(!OpState::Running.can_retry());
        assert!(!OpState::Completed.can_retry());
    }

    #[test]
    fn op_drawer_set_open_toggles_visibility() {
        let mut d = OperationDrawer::new();
        d.set_open(true);
        assert!(d.is_open());
        d.set_open(false);
        assert!(!d.is_open());
    }
}
