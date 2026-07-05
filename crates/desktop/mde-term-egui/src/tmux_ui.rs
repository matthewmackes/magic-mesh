//! **Session management chrome** (TMUX-FC-2) + **window & pane operations and
//! the pane-content mount** (TMUX-FC-3).
//!
//! The session / window / pane sidebar tree, the full session ops (create ·
//! attach · detach · kill · rename), the all-sessions picker, the window tab
//! strip + mounted pane view, and every window/pane mutation (split / close /
//! zoom · break / join / swap / move · drag-resize + drag-reorder · rename) —
//! all **glue over TMUX-FC-1's** [`TmuxController`] (`crate::tmux`).
//!
//! Design: `docs/design/tmux-first-class.md` (lock #4 chrome, #5 sessions, #9
//! pane ops). The discipline the design's risk section demands is preserved
//! verbatim here: a GUI op **never mutates the tree directly** — it emits a
//! [`TmuxIntent`], which [`command_for`] turns into the exact `tmux` command
//! line ([`crate::tmux::commands`]), the controller writes it, and the resulting
//! `%`-event (or tagged reply) reconciles [`crate::tmux::TmuxModel`]. The next
//! frame's render is the round-trip's visible half.
//!
//! What this module carries:
//! * [`TmuxIntent`] + [`command_for`] — the pure GUI-intent → tmux-command map
//!   (the one place a chrome click becomes a command; unit-tested).
//! * [`render_tree`] — the sessions→windows→panes sidebar over a live
//!   [`TmuxModel`], with the session ops, the window/pane rename + kill
//!   affordances, per-row context menus, and pane-row **drag** (onto a pane =
//!   `swap-pane`, onto another window = `join-pane`); emits intents.
//! * [`render_picker_contents`] — the picker listing **all** sessions (attached
//!   AND detached, from [`TmuxController::all_sessions`]).
//! * The **mounted window view** ([`TmuxChrome::window_body`]): the tab strip
//!   (select · drag-reorder → `move-window` · context rename/kill · `+` new)
//!   over the active window's pane tree, each tmux pane a real
//!   [`TerminalWidget`] fed by `%output` (the TERM-3 grid, §6) with typed input
//!   riding `send-keys`; divider **drag-resize** resolves to `resize-pane`
//!   ([`crate::tmux::resize_for_divider`]) on release.
//! * [`TmuxChrome`] — the surface-held state: the optional live controller (tmux
//!   is opt-in, lock #16 — no auto-attach) + the UI-only bits + the widget
//!   mounts, wiring pump → render → dispatch. [`crate::panel`] holds one.
//! * [`TmuxMenuChoice`] — the top-menu-bar (`crate::menubar`) tmux entries route
//!   OUT to the surface, which owns the controller the menu drives.

use std::collections::HashMap;

use mde_egui::egui::{
    self, Button, CursorIcon, FontId, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind,
    Ui, UiBuilder, Vec2,
};
use mde_egui::Style;

use crate::splits::{self, NodePath, SplitDir};
use crate::tmux::{
    commands, resize_for_divider, CommandSink, LayoutDir, SessionInfo, Status, TmuxController,
    TmuxLaunch, TmuxModel, TmuxPaneIo, TmuxWindow,
};
use crate::widget::TerminalWidget;

/// The pointer slop either side of a divider strip that still grabs it.
const DIVIDER_HIT_SLOP: f32 = 3.0;

/// One GUI intent from the chrome — every session/window/pane op the tree, the
/// tab strip, the mounted view, or the picker can raise. Owns its strings so it
/// outlives the render borrow.
///
/// Each maps through [`command_for`] to a real `tmux` command line (the
/// out-of-band ones — [`Self::StartClient`], [`Self::RefreshSessions`] — are
/// handled by [`TmuxChrome::dispatch`] since they act on the channel itself, not
/// as a command). Never a direct tree mutation (the design's round-trip rule).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TmuxIntent {
    /// Start a `tmux -CC` control client (attach `main`, creating it if absent).
    StartClient,
    /// Create + switch to a new named session (`new-session -s`).
    NewSession(String),
    /// Re-attach a (detached) session onto this client (`switch-client -t`).
    AttachSession(String),
    /// Detach this control client (`detach-client`); the session keeps running.
    Detach,
    /// Kill a session by name (`kill-session -t`).
    KillSession(String),
    /// Rename a session (`rename-session -t <target> <new>`).
    RenameSession(String, String),
    /// Make a window the active one (`select-window -t @`).
    SelectWindow(u32),
    /// Make a pane the active one (`select-pane -t %`).
    SelectPane(u32),
    /// Re-request the full session enumeration (the picker's refresh).
    RefreshSessions,
    // ── TMUX-FC-3: window & pane operations ─────────────────────────────────
    /// Open a fresh window in the session (`new-window`).
    NewWindow,
    /// Kill a window (`kill-window -t @`).
    KillWindow(u32),
    /// Rename a window (`rename-window -t @ <name>`).
    RenameWindow(u32, String),
    /// Split a pane — a native [`SplitDir::V`] (side by side) is tmux `-h`,
    /// [`SplitDir::H`] (stacked) tmux `-v` (`split-window`).
    SplitPane(u32, SplitDir),
    /// Close a pane (`kill-pane -t %`).
    ClosePane(u32),
    /// Toggle a pane's zoom (`resize-pane -Z`); `%layout-change` flags carry
    /// the truth back.
    ZoomPane(u32),
    /// Break a pane out into its own window (`break-pane -s %`).
    BreakPane(u32),
    /// Join (move) a pane into another window (`join-pane -v -s % -t @`) — the
    /// pane-row drag onto a window row.
    JoinPane {
        /// The pane being moved.
        src: u32,
        /// The window it joins.
        window: u32,
    },
    /// Swap two panes in place (`swap-pane -d`) — the pane-row drag onto
    /// another pane row.
    SwapPanes(u32, u32),
    /// Reorder: move a window before another (`move-window -b`) — the tab drag.
    MoveWindowBefore {
        /// The dragged window.
        src: u32,
        /// The tab it lands before.
        dst: u32,
    },
    /// Reorder: move a window after another (`move-window -a`) — the tab drag
    /// past the last tab.
    MoveWindowAfter {
        /// The dragged window.
        src: u32,
        /// The tab it lands after.
        dst: u32,
    },
    /// Rename a pane's title (`select-pane -T`); the tagged `list-panes` reply
    /// reconciles (tmux emits no `%`-event for titles).
    RenamePane(u32, String),
    /// Set a pane's exact width in cells (`resize-pane -x`) — a vertical
    /// divider drag, resolved through [`resize_for_divider`].
    ResizePaneWidth(u32, u16),
    /// Set a pane's exact height in cells (`resize-pane -y`) — a horizontal
    /// divider drag.
    ResizePaneHeight(u32, u16),
    /// Report the mounted view's cell grid as the control client's size
    /// (`refresh-client -C`), so tmux lays windows out to what's on screen.
    ClientResize(u16, u16),
}

/// Turn a [`TmuxIntent`] into the exact `tmux` command line, or [`None`] for the
/// two intents [`TmuxChrome::dispatch`] handles on the channel directly.
#[must_use]
pub fn command_for(intent: &TmuxIntent) -> Option<String> {
    match intent {
        TmuxIntent::NewSession(name) => Some(commands::new_session(name)),
        TmuxIntent::AttachSession(name) => Some(commands::attach_session(name)),
        TmuxIntent::Detach => Some(commands::detach_client()),
        TmuxIntent::KillSession(name) => Some(commands::kill_session(name)),
        TmuxIntent::RenameSession(target, name) => Some(commands::rename_session(target, name)),
        TmuxIntent::SelectWindow(w) => Some(commands::select_window(*w)),
        TmuxIntent::SelectPane(p) => Some(commands::select_pane(*p)),
        TmuxIntent::NewWindow => Some(commands::new_window()),
        TmuxIntent::KillWindow(w) => Some(commands::kill_window(*w)),
        TmuxIntent::RenameWindow(w, name) => Some(commands::rename_window(*w, name)),
        TmuxIntent::SplitPane(p, dir) => Some(commands::split_window(*p, *dir)),
        TmuxIntent::ClosePane(p) => Some(commands::kill_pane(*p)),
        TmuxIntent::ZoomPane(p) => Some(commands::zoom_pane(*p)),
        TmuxIntent::BreakPane(p) => Some(commands::break_pane(*p)),
        // A drop joins stacked (tmux's own default split direction).
        TmuxIntent::JoinPane { src, window } => {
            Some(commands::join_pane(*src, *window, SplitDir::H))
        }
        TmuxIntent::SwapPanes(a, b) => Some(commands::swap_panes(*a, *b)),
        TmuxIntent::MoveWindowBefore { src, dst } => Some(commands::move_window_before(*src, *dst)),
        TmuxIntent::MoveWindowAfter { src, dst } => Some(commands::move_window_after(*src, *dst)),
        TmuxIntent::RenamePane(p, title) => Some(commands::rename_pane(*p, title)),
        TmuxIntent::ResizePaneWidth(p, cols) => Some(commands::resize_pane_width(*p, *cols)),
        TmuxIntent::ResizePaneHeight(p, rows) => Some(commands::resize_pane_height(*p, *rows)),
        TmuxIntent::ClientResize(cols, rows) => Some(commands::refresh_client_size(*cols, *rows)),
        TmuxIntent::StartClient | TmuxIntent::RefreshSessions => None,
    }
}

/// A tmux top-menu choice (`crate::menubar`) the surface applies.
///
/// These drive the surface-held [`TmuxChrome`] (which owns the optional live
/// controller), so they route OUT of the bar rather than into its `apply` (which
/// only touches the [`crate::TabbedTerminal`]). The window/pane ops resolve
/// their target from the model (the current window's active pane) — the same
/// intents the sidebar/view affordances raise.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TmuxMenuChoice {
    /// Start a tmux control client and reveal the tree.
    NewSession,
    /// Open the all-sessions picker (starting a client first if needed).
    ShowPicker,
    /// Detach the control client.
    Detach,
    /// Show/hide the sidebar tree.
    ToggleTree,
    /// Split the active pane side-by-side (`split-window -h`).
    SplitRight,
    /// Split the active pane stacked (`split-window -v`).
    SplitDown,
    /// Toggle the active pane's zoom.
    ZoomPane,
    /// Break the active pane out into its own window.
    BreakPane,
    /// Close the active pane.
    ClosePane,
    /// Open a fresh window.
    NewWindow,
    /// Kill the current window.
    KillWindow,
}

/// An in-progress inline session rename.
#[derive(Clone, PartialEq, Eq, Debug)]
struct SessionRename {
    /// The session name being renamed (the `rename-session` target).
    target: String,
    /// The edit buffer.
    buffer: String,
}

/// What a sidebar row is — the pane-drag drop resolution's target set.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RowTarget {
    /// A window row (its id).
    Window(u32),
    /// A pane row (its id).
    Pane(u32),
}

/// The UI-only state of the chrome (everything that is NOT the live controller).
#[derive(Default)]
struct ChromeUi {
    /// Whether the sidebar tree is mounted.
    tree_open: bool,
    /// Whether the all-sessions picker window is open.
    picker_open: bool,
    /// Whether the inline "new session" name field is revealed.
    new_open: bool,
    /// The new-session name buffer.
    new_name: String,
    /// The in-progress session rename, if any.
    rename: Option<SessionRename>,
    /// The in-progress window rename `(window, buffer)`, if any.
    win_rename: Option<(u32, String)>,
    /// The in-progress pane-title rename `(pane, buffer)`, if any.
    pane_rename: Option<(u32, String)>,
    /// A window tab being dragged for reorder, if any.
    drag_window: Option<u32>,
    /// A sidebar pane row being dragged (→ swap/join), if any.
    drag_pane: Option<u32>,
    /// A divider drag in flight: `(window, divider path, current ratio)` — a
    /// view-side preview only; the model changes when the released drag's
    /// `resize-pane` round-trips through `%layout-change`.
    resize_drag: Option<(u32, NodePath, f32)>,
    /// The last client cell grid reported via `refresh-client -C`.
    client_grid: Option<(u16, u16)>,
}

/// The surface-held tmux chrome.
///
/// The optional live [`TmuxController`] (tmux is **opt-in**, lock #16 — nothing
/// attaches until the user asks), the UI-only state, and the mounted pane
/// widgets, wiring the per-frame pump → render → dispatch cycle.
#[derive(Default)]
pub struct TmuxChrome {
    /// The live control connection, once the user starts one (`None` = no tmux).
    controller: Option<TmuxController>,
    /// The UI-only state.
    ui: ChromeUi,
    /// The mounted pane widgets, by tmux pane id — each a real TERM-3
    /// [`TerminalWidget`] over the pane's shared engine (§6, no second grid).
    mounts: HashMap<u32, TerminalWidget>,
}

impl TmuxChrome {
    /// A fresh chrome with no tmux session (the sidebar hidden).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Drain the control channel into the model — call once per frame, before the
    /// tree renders (the [`crate::panel::terminal_pump`] slot).
    pub fn pump(&mut self) {
        if let Some(ctrl) = self.controller.as_mut() {
            ctrl.pump();
        }
    }

    /// Whether a live control channel is up (attached or attaching) — the gate for
    /// the tree, the session ops, and the context-sensitive menu items.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.controller
            .as_ref()
            .is_some_and(|c| matches!(c.status(), Status::Connecting | Status::Attached))
    }

    /// Whether the sidebar tree is currently mounted (the menu's toggle state).
    #[must_use]
    pub const fn tree_open(&self) -> bool {
        self.ui.tree_open
    }

    /// Apply a top-menu choice (`crate::menubar`), if one was made this frame.
    pub fn apply_menu(&mut self, choice: Option<TmuxMenuChoice>) {
        match choice {
            Some(TmuxMenuChoice::NewSession) => {
                self.ensure_client();
                self.ui.tree_open = true;
            }
            Some(TmuxMenuChoice::ShowPicker) => {
                self.ensure_client();
                self.ui.tree_open = true;
                self.ui.picker_open = true;
                self.refresh_sessions();
            }
            Some(TmuxMenuChoice::Detach) => {
                if let Some(ctrl) = self.controller.as_ref() {
                    let _ = ctrl.send(&commands::detach_client());
                }
            }
            Some(TmuxMenuChoice::ToggleTree) => self.ui.tree_open = !self.ui.tree_open,
            Some(op) => {
                let intents = self.menu_op_intents(op);
                self.dispatch(intents);
            }
            None => {}
        }
    }

    /// Resolve a menu window/pane op against the model — the current window's
    /// active pane (falling back to the window's first pane). Honest nothing
    /// when there is no live window/pane to act on.
    fn menu_op_intents(&self, op: TmuxMenuChoice) -> Vec<TmuxIntent> {
        let Some(model) = self.controller.as_ref().map(TmuxController::model) else {
            return Vec::new();
        };
        if op == TmuxMenuChoice::NewWindow {
            return vec![TmuxIntent::NewWindow];
        }
        let Some(window) = active_window(model) else {
            return Vec::new();
        };
        if op == TmuxMenuChoice::KillWindow {
            return vec![TmuxIntent::KillWindow(window)];
        }
        let Some(pane) = model
            .window(window)
            .and_then(TmuxWindow::active_pane)
            .or_else(|| model.panes_of_window(window).first().copied())
        else {
            return Vec::new();
        };
        match op {
            TmuxMenuChoice::SplitRight => vec![TmuxIntent::SplitPane(pane, SplitDir::V)],
            TmuxMenuChoice::SplitDown => vec![TmuxIntent::SplitPane(pane, SplitDir::H)],
            TmuxMenuChoice::ZoomPane => vec![TmuxIntent::ZoomPane(pane)],
            TmuxMenuChoice::BreakPane => vec![TmuxIntent::BreakPane(pane)],
            TmuxMenuChoice::ClosePane => vec![TmuxIntent::ClosePane(pane)],
            _ => Vec::new(),
        }
    }

    /// Mount the sidebar tree (a left [`egui::SidePanel`]) + the picker window,
    /// dispatching whatever ops the user raised this frame. A no-op when the tree
    /// is hidden, so tmux truly costs nothing until opened.
    pub fn sidebar(&mut self, ui: &mut Ui) {
        if !self.ui.tree_open {
            return;
        }
        let mut intents: Vec<TmuxIntent> = Vec::new();
        {
            let Self {
                controller,
                ui: state,
                ..
            } = self;
            let ctrl = controller.as_ref();
            egui::SidePanel::left("tmux-tree")
                .resizable(true)
                .default_width(Style::SP_XL * 6.0)
                .show_inside(ui, |ui| {
                    render_tree(ui, ctrl, state, &mut intents);
                });
            if state.picker_open {
                render_picker(ui, ctrl, state, &mut intents);
            }
        }
        self.dispatch(intents);
    }

    /// Mount the active tmux window as the surface body: the tab strip over the
    /// window's pane tree, every pane a live [`TerminalWidget`] (TMUX-FC-3's
    /// pane-content mount). Returns `false` — paint the native terminal instead
    /// — while no control client is live or no window exists yet, so the
    /// surface coexists honestly (lock #3: tmux is opt-in per tab strip).
    pub fn window_body(&mut self, ui: &mut Ui) -> bool {
        if !self.is_active() {
            return false;
        }
        let mut intents: Vec<TmuxIntent> = Vec::new();
        let mounted = {
            let Self {
                controller,
                ui: state,
                mounts,
            } = self;
            let Some(ctrl) = controller.as_ref() else {
                return false;
            };
            let model = ctrl.model();
            let Some(window) = active_window(model) else {
                return false;
            };
            let Some(sink) = ctrl.sink() else {
                return false;
            };
            // Panes the server dropped unmount with their windows.
            mounts.retain(|pane, _| model.pane(*pane).is_some());
            tab_strip(ui, model, window, state, &mut intents);
            view_body(ui, model, window, state, mounts, &sink, &mut intents);
            true
        };
        self.dispatch(intents);
        mounted
    }

    /// Start a control client if none is live (idempotent) — the opt-in attach.
    fn ensure_client(&mut self) {
        if !self.is_active() {
            self.controller = Some(TmuxController::connect(&TmuxLaunch::default()));
        }
    }

    /// Ask the server for the full session list (feeds the picker).
    fn refresh_sessions(&self) {
        if let Some(ctrl) = self.controller.as_ref() {
            let _ = ctrl.request_sessions();
        }
    }

    /// Issue each raised intent as its tmux command (the round-trip's first half);
    /// the out-of-band intents act on the channel itself. Ops whose truth rides a
    /// tagged reply rather than a `%`-event (pane titles, window order) are
    /// followed by the matching enumeration request so the model reconciles.
    fn dispatch(&mut self, intents: Vec<TmuxIntent>) {
        for intent in intents {
            match &intent {
                TmuxIntent::StartClient => {
                    self.ensure_client();
                    self.ui.tree_open = true;
                }
                TmuxIntent::RefreshSessions => self.refresh_sessions(),
                other => {
                    if let (Some(cmd), Some(ctrl)) = (command_for(other), self.controller.as_ref())
                    {
                        let _ = ctrl.send(&cmd);
                        match other {
                            TmuxIntent::RenamePane(..) => {
                                let _ = ctrl.request_pane_titles();
                            }
                            TmuxIntent::MoveWindowBefore { .. }
                            | TmuxIntent::MoveWindowAfter { .. } => {
                                let _ = ctrl.request_window_order();
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}

/// The window the view mounts: the session's current one, falling back to the
/// first of the tab strip.
fn active_window(model: &TmuxModel) -> Option<u32> {
    model
        .current_window()
        .filter(|w| model.window(*w).is_some())
        .or_else(|| model.windows_in_order().first().copied())
}

// ─────────────────────────────────────────────────────────────────────────────
// The sidebar tree.
// ─────────────────────────────────────────────────────────────────────────────

/// Render the sidebar content: the session op toolbar + the current session's
/// windows→panes tree (control mode streams the *attached* session's detail).
/// Emits [`TmuxIntent`]s; never mutates the model.
fn render_tree(
    ui: &mut Ui,
    controller: Option<&TmuxController>,
    state: &mut ChromeUi,
    intents: &mut Vec<TmuxIntent>,
) {
    ui.add_space(Style::SP_S);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("TMUX")
                .size(Style::SMALL)
                .color(Style::ACCENT_TERMINALS)
                .strong(),
        );
    });
    ui.add_space(Style::SP_XS);

    let Some(ctrl) =
        controller.filter(|c| matches!(c.status(), Status::Connecting | Status::Attached))
    else {
        no_session(ui, controller, intents);
        return;
    };

    let model = ctrl.model();
    // The session op row (create · picker · detach) — the always-available ops.
    ui.horizontal_wrapped(|ui| {
        if ui.button("+ Session").clicked() {
            state.new_open = !state.new_open;
        }
        if ui.button("Sessions\u{2026}").clicked() {
            state.picker_open = true;
            intents.push(TmuxIntent::RefreshSessions);
        }
        if ui.button("Detach").clicked() {
            intents.push(TmuxIntent::Detach);
        }
    });

    // The inline "new session" name field (create op).
    if state.new_open {
        ui.horizontal(|ui| {
            let resp = ui.text_edit_singleline(&mut state.new_name);
            let submit = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("Create").clicked() || submit) && !state.new_name.trim().is_empty() {
                intents.push(TmuxIntent::NewSession(state.new_name.trim().to_owned()));
                state.new_name.clear();
                state.new_open = false;
            }
        });
    }

    ui.add_space(Style::SP_XS);
    ui.separator();

    let mut rows: Vec<(RowTarget, Rect)> = Vec::new();
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            session_node(ui, model, state, intents, &mut rows);
        });

    // Resolve an in-flight pane-row drag: released over another pane swaps,
    // over another window joins — each a tmux command, never a tree edit.
    if let Some(src) = state.drag_pane {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        let (pointer, released) = ui.input(|i| {
            (
                i.pointer.latest_pos(),
                i.pointer.primary_released() || !i.pointer.any_down(),
            )
        });
        if released {
            if let Some(pos) = pointer {
                let src_window = model.pane(src).and_then(crate::tmux::TmuxPane::window);
                if let Some(intent) = pane_drop_intent(src, src_window, &rows, pos) {
                    intents.push(intent);
                }
            }
            state.drag_pane = None;
        }
    }
}

/// The honest "no tmux session" state — a single start affordance (the opt-in
/// attach), plus the last channel's exit reason if it just detached/died (§7).
fn no_session(ui: &mut Ui, controller: Option<&TmuxController>, intents: &mut Vec<TmuxIntent>) {
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("No tmux session")
            .size(Style::BODY)
            .color(Style::TEXT_DIM),
    );
    if let Some(Status::Exited(reason)) = controller.map(TmuxController::status) {
        if !reason.is_empty() {
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new(reason.as_str())
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
        }
    }
    ui.add_space(Style::SP_S);
    if ui.button("New tmux session").clicked() {
        intents.push(TmuxIntent::StartClient);
    }
}

/// The current session header (with the inline rename + kill) and its windows.
fn session_node(
    ui: &mut Ui,
    model: &TmuxModel,
    state: &mut ChromeUi,
    intents: &mut Vec<TmuxIntent>,
    rows: &mut Vec<(RowTarget, Rect)>,
) {
    let Some(sid) = model.current_session() else {
        return;
    };
    let name = model
        .session(sid)
        .map_or("", crate::tmux::TmuxSession::name)
        .to_owned();

    // The session row: name + rename + kill. A rename in progress swaps in an edit.
    let renaming_this = state.rename.as_ref().is_some_and(|r| r.target == name);
    if renaming_this {
        ui.horizontal(|ui| {
            let (submit, cancel) = {
                let rename = state.rename.as_mut().expect("renaming_this");
                let resp = ui.text_edit_singleline(&mut rename.buffer);
                let submit = ui.button("Rename").clicked()
                    || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                let cancel = ui.button("Cancel").clicked();
                (submit, cancel)
            };
            if submit {
                if let Some(rename) = state.rename.take() {
                    let new = rename.buffer.trim();
                    if !new.is_empty() {
                        intents.push(TmuxIntent::RenameSession(rename.target, new.to_owned()));
                    }
                }
            } else if cancel {
                state.rename = None;
            }
        });
    } else {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("\u{25CF} {name}"))
                    .size(Style::BODY)
                    .color(Style::TEXT)
                    .strong(),
            );
            if icon_button(ui, "rename").clicked() {
                state.rename = Some(SessionRename {
                    target: name.clone(),
                    buffer: name.clone(),
                });
            }
            if icon_button(ui, "kill").clicked() {
                intents.push(TmuxIntent::KillSession(name.clone()));
            }
            if icon_button(ui, "+win").clicked() {
                intents.push(TmuxIntent::NewWindow);
            }
        });
    }

    // The windows of the attached session → each expandable to its panes.
    for window in model.windows_in_order() {
        window_node(ui, model, window, state, intents, rows);
    }
}

/// One window row + its panes. Clicking selects (round-trips `select-window` /
/// `select-pane`); the FC-3 mutations ride the row affordances + context menus
/// + pane-row drag — every one a tmux command reconciled by `%`-events.
fn window_node(
    ui: &mut Ui,
    model: &TmuxModel,
    window: u32,
    state: &mut ChromeUi,
    intents: &mut Vec<TmuxIntent>,
    rows: &mut Vec<(RowTarget, Rect)>,
) {
    let Some(win) = model.window(window) else {
        return;
    };
    let active_pane = win.active_pane();
    let is_current = model.current_window() == Some(window);

    // An in-progress window rename swaps the row for an edit field.
    if state.win_rename.as_ref().is_some_and(|(w, _)| *w == window) {
        rename_row(ui, &mut state.win_rename, |w, name| {
            intents.push(TmuxIntent::RenameWindow(w, name));
        });
    } else {
        let zoom_tag = if win.is_zoomed() { "  [Z]" } else { "" };
        let label = format!("@{window}  {}{zoom_tag}", win.name());
        ui.horizontal(|ui| {
            ui.add_space(Style::SP_M);
            let resp = ui.selectable_label(is_current, RichText::new(label).color(Style::TEXT));
            if resp.clicked() {
                intents.push(TmuxIntent::SelectWindow(window));
            }
            resp.context_menu(|ui| {
                if ui.button("Rename\u{2026}").clicked() {
                    state.win_rename = Some((window, win.name().to_owned()));
                    ui.close_menu();
                }
                if ui.button("New Window").clicked() {
                    intents.push(TmuxIntent::NewWindow);
                    ui.close_menu();
                }
                if ui.button("Kill Window").clicked() {
                    intents.push(TmuxIntent::KillWindow(window));
                    ui.close_menu();
                }
            });
            rows.push((RowTarget::Window(window), resp.rect));
        });
    }

    for pane in model.panes_of_window(window) {
        pane_node(ui, model, pane, active_pane, state, intents, rows);
    }
}

/// One pane row: click selects, drag picks it up (→ swap/join on drop), and the
/// context menu carries the FC-3 pane ops (split · zoom · break · rename title ·
/// close) — each an intent that becomes a tmux command.
fn pane_node(
    ui: &mut Ui,
    model: &TmuxModel,
    pane: u32,
    active_pane: Option<u32>,
    state: &mut ChromeUi,
    intents: &mut Vec<TmuxIntent>,
    rows: &mut Vec<(RowTarget, Rect)>,
) {
    if state.pane_rename.as_ref().is_some_and(|(p, _)| *p == pane) {
        rename_row(ui, &mut state.pane_rename, |p, title| {
            intents.push(TmuxIntent::RenamePane(p, title));
        });
        return;
    }
    let title = model.pane(pane).map_or("", crate::tmux::TmuxPane::title);
    let is_active = active_pane == Some(pane);
    let text = if title.is_empty() {
        format!("%{pane}")
    } else {
        format!("%{pane}  {title}")
    };
    let color = if is_active {
        Style::ACCENT
    } else {
        Style::TEXT_DIM
    };
    ui.horizontal(|ui| {
        ui.add_space(Style::SP_XL);
        let resp = ui
            .selectable_label(
                is_active,
                RichText::new(text).size(Style::SMALL).color(color),
            )
            .interact(Sense::click_and_drag());
        if resp.clicked() {
            intents.push(TmuxIntent::SelectPane(pane));
        }
        if resp.drag_started() {
            state.drag_pane = Some(pane);
        }
        if state.drag_pane == Some(pane) {
            ui.painter().rect_stroke(
                resp.rect,
                Style::RADIUS,
                Stroke::new(1.0, Style::ACCENT_HI),
                StrokeKind::Inside,
            );
        }
        resp.context_menu(|ui| {
            if ui.button("Split Right").clicked() {
                intents.push(TmuxIntent::SplitPane(pane, SplitDir::V));
                ui.close_menu();
            }
            if ui.button("Split Down").clicked() {
                intents.push(TmuxIntent::SplitPane(pane, SplitDir::H));
                ui.close_menu();
            }
            if ui.button("Zoom").clicked() {
                intents.push(TmuxIntent::ZoomPane(pane));
                ui.close_menu();
            }
            if ui.button("Break to Window").clicked() {
                intents.push(TmuxIntent::BreakPane(pane));
                ui.close_menu();
            }
            if ui.button("Rename Title\u{2026}").clicked() {
                state.pane_rename = Some((pane, title.to_owned()));
                ui.close_menu();
            }
            if ui.button("Close Pane").clicked() {
                intents.push(TmuxIntent::ClosePane(pane));
                ui.close_menu();
            }
        });
        rows.push((RowTarget::Pane(pane), resp.rect));
    });
}

/// A shared inline rename row for windows/panes: an edit field + Rename/Cancel.
/// Submits through `on_submit` (which raises the rename intent) and clears the
/// buffer either way.
fn rename_row(
    ui: &mut Ui,
    buffer: &mut Option<(u32, String)>,
    mut on_submit: impl FnMut(u32, String),
) {
    ui.horizontal(|ui| {
        let (submit, cancel) = {
            let Some((_, text)) = buffer.as_mut() else {
                return;
            };
            let resp = ui.text_edit_singleline(text);
            let submit = ui.button("Rename").clicked()
                || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            let cancel = ui.button("Cancel").clicked();
            (submit, cancel)
        };
        if submit {
            if let Some((id, text)) = buffer.take() {
                let new = text.trim();
                if !new.is_empty() {
                    on_submit(id, new.to_owned());
                }
            }
        } else if cancel {
            *buffer = None;
        }
    });
}

/// Resolve a released pane-row drag: over another pane row → swap the two; over
/// a *different* window's row → join (move) the pane into that window. Pure —
/// the sidebar's drop rule, unit-tested headlessly.
fn pane_drop_intent(
    src: u32,
    src_window: Option<u32>,
    rows: &[(RowTarget, Rect)],
    pos: Pos2,
) -> Option<TmuxIntent> {
    let (target, _) = rows.iter().find(|(_, rect)| rect.contains(pos))?;
    match target {
        RowTarget::Pane(dst) if *dst != src => Some(TmuxIntent::SwapPanes(src, *dst)),
        RowTarget::Window(w) if src_window != Some(*w) => {
            Some(TmuxIntent::JoinPane { src, window: *w })
        }
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The mounted window view: tab strip + pane tree of live TERM-3 widgets.
// ─────────────────────────────────────────────────────────────────────────────

/// The window tab strip: one selectable tab per linked window (select ·
/// drag-reorder → `move-window` · context rename/kill) plus the `+` new-window
/// affordance. The strip order is the model's — a reorder becomes visible when
/// the tagged window-order reply reconciles it.
fn tab_strip(
    ui: &mut Ui,
    model: &TmuxModel,
    current: u32,
    state: &mut ChromeUi,
    intents: &mut Vec<TmuxIntent>,
) {
    let mut tabs: Vec<(u32, Rect)> = Vec::new();
    ui.horizontal_wrapped(|ui| {
        ui.add_space(Style::SP_XS);
        for window in model.windows_in_order() {
            let Some(win) = model.window(window) else {
                continue;
            };
            let zoom_tag = if win.is_zoomed() { " [Z]" } else { "" };
            let text = if win.name().is_empty() {
                format!("@{window}{zoom_tag}")
            } else {
                format!("{}{zoom_tag}", win.name())
            };
            let resp = ui
                .selectable_label(window == current, RichText::new(text).size(Style::SMALL))
                .interact(Sense::click_and_drag());
            if resp.clicked() {
                intents.push(TmuxIntent::SelectWindow(window));
            }
            if resp.drag_started() {
                state.drag_window = Some(window);
            }
            if state.drag_window == Some(window) {
                ui.painter().rect_stroke(
                    resp.rect,
                    Style::RADIUS,
                    Stroke::new(1.0, Style::ACCENT_HI),
                    StrokeKind::Inside,
                );
            }
            resp.context_menu(|ui| {
                if ui.button("Rename\u{2026}").clicked() {
                    state.win_rename = Some((window, win.name().to_owned()));
                    ui.close_menu();
                }
                if ui.button("Kill Window").clicked() {
                    intents.push(TmuxIntent::KillWindow(window));
                    ui.close_menu();
                }
            });
            tabs.push((window, resp.rect));
        }
        if ui
            .button("+")
            .on_hover_text("New window (new-window)")
            .clicked()
        {
            intents.push(TmuxIntent::NewWindow);
        }
    });

    // The strip hosts the window-rename editor when the sidebar (which also
    // renders one) is closed, so a tab-context rename always has a field.
    if !state.tree_open && state.win_rename.is_some() {
        rename_row(ui, &mut state.win_rename, |w, name| {
            intents.push(TmuxIntent::RenameWindow(w, name));
        });
    }

    // Resolve an in-flight tab drag on release: dropping on/left of a tab moves
    // the dragged window before it; past the last tab moves it after.
    if let Some(src) = state.drag_window {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        let (pointer, released) = ui.input(|i| {
            (
                i.pointer.latest_pos(),
                i.pointer.primary_released() || !i.pointer.any_down(),
            )
        });
        if released {
            if let Some(pos) = pointer {
                if let Some(intent) = tab_drop_intent(src, &tabs, pos.x) {
                    intents.push(intent);
                }
            }
            state.drag_window = None;
        }
    }
}

/// Resolve a released tab drag at pointer-x: the dragged window moves **before**
/// the first tab whose centre lies right of the pointer, or **after** the last
/// tab when the pointer is past every centre. Pure — the reorder rule the tab
/// strip applies, unit-tested headlessly.
fn tab_drop_intent(src: u32, tabs: &[(u32, Rect)], x: f32) -> Option<TmuxIntent> {
    if tabs.len() < 2 {
        return None;
    }
    match tabs.iter().find(|(_, rect)| rect.center().x > x) {
        Some((dst, _)) if *dst != src => Some(TmuxIntent::MoveWindowBefore { src, dst: *dst }),
        Some(_) => None,
        None => {
            let (last, _) = tabs.last()?;
            (*last != src).then_some(TmuxIntent::MoveWindowAfter { src, dst: *last })
        }
    }
}

/// The mounted pane tree of the active window: each tmux pane a live TERM-3
/// [`TerminalWidget`] over the pane's shared engine (typed input → `send-keys`),
/// laid out by the window's own tmux arrangement ([`TmuxModel::window_tree`] →
/// [`splits::layout`], §6 — the native geometry verbatim). Dividers drag with a
/// view-side preview and resolve to `resize-pane` on release; the view's cell
/// grid is reported as the client size so tmux fits what's on screen.
fn view_body(
    ui: &mut Ui,
    model: &TmuxModel,
    window: u32,
    state: &mut ChromeUi,
    mounts: &mut HashMap<u32, TerminalWidget>,
    sink: &CommandSink,
    intents: &mut Vec<TmuxIntent>,
) {
    let rect = ui.available_rect_before_wrap();

    // Report the view's cell grid as the control client's size (once per
    // change): tmux sizes the session to the attached client, so this is what
    // makes the mounted layout fill the real on-screen rect.
    let font_id = FontId::monospace(Style::BODY);
    let cell = ui.fonts(|f| Vec2::new(f.glyph_width(&font_id, 'M'), f.row_height(&font_id)));
    if cell.x > 0.0 && cell.y > 0.0 {
        let grid = crate::widget::grid_size(rect.size(), cell);
        if state.client_grid != Some(grid) {
            state.client_grid = Some(grid);
            intents.push(TmuxIntent::ClientResize(grid.0, grid.1));
        }
    }

    let Some(mut tree) = model.window_tree(window) else {
        ui.add_space(Style::SP_M);
        ui.horizontal(|ui| {
            ui.add_space(Style::SP_M);
            ui.label(
                RichText::new("Waiting for the tmux window layout\u{2026}")
                    .size(Style::BODY)
                    .color(Style::TEXT_DIM),
            );
        });
        return;
    };

    // A divider drag previews on a per-frame copy of the arrangement — the
    // model itself only moves when the released drag's `resize-pane`
    // round-trips through `%layout-change` (the doctrine, in pixels).
    if let Some((w, path, ratio)) = state.resize_drag {
        if w == window {
            if let Some(r) = tree.ratio_mut(path) {
                *r = ratio;
            }
        }
    }

    let lay = splits::layout(&tree, rect);
    let active = model.window(window).and_then(TmuxWindow::active_pane);

    for (sid, prect) in &lay.leaves {
        // Leaf ids are tmux pane ids by construction (`Layout::to_pane_tree`).
        let Ok(pane) = u32::try_from(sid.0) else {
            continue;
        };
        if let std::collections::hash_map::Entry::Vacant(slot) = mounts.entry(pane) {
            let Some(engine) = model.pane_terminal(pane) else {
                continue;
            };
            slot.insert(TerminalWidget::new_tmux(TmuxPaneIo::new(
                pane,
                engine,
                sink.clone(),
            )));
        }
        let Some(widget) = mounts.get_mut(&pane) else {
            continue;
        };
        let mut pane_ui = ui.new_child(
            UiBuilder::new()
                .max_rect(*prect)
                .id_salt(("tmux-mount", pane)),
        );
        let resp = widget.show(&mut pane_ui);
        if resp.clicked() {
            resp.request_focus();
            intents.push(TmuxIntent::SelectPane(pane));
        }
        // Hand the keyboard to tmux's active pane when nothing local holds it.
        if active == Some(pane) && ui.memory(|m| m.focused().is_none()) {
            resp.request_focus();
        }
    }

    // The active-pane ring (only worth telling apart with multiple panes).
    if lay.leaves.len() > 1 {
        if let Some((_, arect)) = lay
            .leaves
            .iter()
            .find(|(sid, _)| active.is_some_and(|a| u64::from(a) == sid.0))
        {
            ui.painter().rect_stroke(
                *arect,
                0.0,
                Stroke::new(1.0, Style::ACCENT),
                StrokeKind::Inside,
            );
        }
    }

    view_dividers(ui, model, window, state, &lay, intents);
}

/// The mounted view's dividers: a drag previews the cut (view-side only); its
/// release maps the final ratio back to the exact `resize-pane` via the tmux
/// layout ([`resize_for_divider`]) — the boundary then really moves when the
/// `%layout-change` comes back.
fn view_dividers(
    ui: &Ui,
    model: &TmuxModel,
    window: u32,
    state: &mut ChromeUi,
    lay: &splits::Layout,
    intents: &mut Vec<TmuxIntent>,
) {
    for div in &lay.dividers {
        let (hit, icon, line_size) = match div.dir {
            SplitDir::V => (
                div.rect.expand2(Vec2::new(DIVIDER_HIT_SLOP, 0.0)),
                CursorIcon::ResizeHorizontal,
                Vec2::new(1.0, div.rect.height()),
            ),
            SplitDir::H => (
                div.rect.expand2(Vec2::new(0.0, DIVIDER_HIT_SLOP)),
                CursorIcon::ResizeVertical,
                Vec2::new(div.rect.width(), 1.0),
            ),
        };
        let resp = ui
            .interact(
                hit,
                ui.id().with(("tmux-splitter", div.path)),
                Sense::drag(),
            )
            .on_hover_cursor(icon);
        if resp.dragged() {
            if let Some(pos) = resp.interact_pointer_pos() {
                state.resize_drag = Some((window, div.path, splits::pointer_ratio(div, pos)));
            }
        }
        if resp.drag_stopped() {
            if let Some((w, path, ratio)) = state.resize_drag.take() {
                if w == window {
                    if let Some(intent) = resize_intent(model, window, path, ratio) {
                        intents.push(intent);
                    }
                }
            }
        }
        let color = if resp.dragged() {
            Style::ACCENT
        } else if resp.hovered() {
            Style::ACCENT_HI
        } else {
            Style::BORDER
        };
        ui.painter().rect_filled(
            Rect::from_center_size(div.rect.center(), line_size),
            0.0,
            color,
        );
    }
}

/// Map a finished divider drag back to its `resize-pane` intent through the
/// window's tmux layout ([`resize_for_divider`]). Pure — unit-tested.
fn resize_intent(model: &TmuxModel, window: u32, path: NodePath, ratio: f32) -> Option<TmuxIntent> {
    let layout = model.window(window).and_then(TmuxWindow::layout)?;
    let resize = resize_for_divider(layout, path, ratio)?;
    Some(match resize.dir {
        LayoutDir::LeftRight => TmuxIntent::ResizePaneWidth(resize.pane, resize.cells),
        LayoutDir::TopBottom => TmuxIntent::ResizePaneHeight(resize.pane, resize.cells),
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// The all-sessions picker.
// ─────────────────────────────────────────────────────────────────────────────

/// The all-sessions picker window (attached AND detached) — a detached row offers
/// **Attach** (re-attach it), any row offers **Kill**. Its source is the
/// control-channel `list-sessions` reply ([`TmuxController::all_sessions`]).
fn render_picker(
    ui: &Ui,
    controller: Option<&TmuxController>,
    state: &mut ChromeUi,
    intents: &mut Vec<TmuxIntent>,
) {
    let mut open = true;
    egui::Window::new("tmux sessions")
        .collapsible(false)
        .resizable(true)
        .show(ui.ctx(), |ui| {
            open = !render_picker_contents(ui, controller, intents);
        });
    if !open {
        state.picker_open = false;
    }
}

/// The picker's body (its own fn so it mounts headlessly). Returns `true` when the
/// user asked to close it.
fn render_picker_contents(
    ui: &mut Ui,
    controller: Option<&TmuxController>,
    intents: &mut Vec<TmuxIntent>,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("All sessions")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM)
                .strong(),
        );
        if ui.button("Refresh").clicked() {
            intents.push(TmuxIntent::RefreshSessions);
        }
    });
    ui.add_space(Style::SP_XS);

    let sessions = controller.map_or(&[][..], TmuxController::all_sessions);
    if sessions.is_empty() {
        ui.label(
            RichText::new("No sessions reported yet")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
    }
    for info in sessions {
        session_row(ui, info, intents);
    }

    ui.add_space(Style::SP_S);
    ui.separator();
    ui.button("Close").clicked()
}

/// One picker row: name, an attached/detached chip, and the Attach/Kill ops.
fn session_row(ui: &mut Ui, info: &SessionInfo, intents: &mut Vec<TmuxIntent>) {
    ui.horizontal(|ui| {
        let (chip, color) = if info.attached {
            ("attached", Style::OK)
        } else {
            ("detached", Style::TEXT_DIM)
        };
        ui.label(
            RichText::new(info.name.as_str())
                .size(Style::BODY)
                .color(Style::TEXT),
        );
        ui.label(RichText::new(chip).size(Style::SMALL).color(color));
        ui.label(
            RichText::new(format!("{} win", info.windows))
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        // A detached session can be (re-)attached; the current one already is.
        if !info.attached && ui.button("Attach").clicked() {
            intents.push(TmuxIntent::AttachSession(info.name.clone()));
        }
        if ui.button("Kill").clicked() {
            intents.push(TmuxIntent::KillSession(info.name.clone()));
        }
    });
}

/// A small frameless text button for the row-inline ops (rename/kill), so the
/// tree rows stay compact and read as chrome rather than form buttons.
fn icon_button(ui: &mut Ui, label: &str) -> egui::Response {
    ui.add(
        Button::new(
            RichText::new(label)
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        )
        .frame(false),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::{Parser, TmuxModel};
    use mde_egui::egui::{pos2, vec2, Context, Rect};
    use std::sync::mpsc;

    // ── the intent → command map (the round-trip's first half) ───────────────

    #[test]
    fn command_for_maps_each_op_to_its_tmux_line() {
        assert_eq!(
            command_for(&TmuxIntent::NewSession("dev".to_owned())).as_deref(),
            Some("new-session -s 'dev'")
        );
        assert_eq!(
            command_for(&TmuxIntent::AttachSession("dev".to_owned())).as_deref(),
            Some("switch-client -t 'dev'")
        );
        assert_eq!(
            command_for(&TmuxIntent::Detach).as_deref(),
            Some("detach-client")
        );
        assert_eq!(
            command_for(&TmuxIntent::KillSession("dev".to_owned())).as_deref(),
            Some("kill-session -t 'dev'")
        );
        assert_eq!(
            command_for(&TmuxIntent::RenameSession(
                "dev".to_owned(),
                "prod".to_owned()
            ))
            .as_deref(),
            Some("rename-session -t 'dev' 'prod'")
        );
        assert_eq!(
            command_for(&TmuxIntent::SelectWindow(2)).as_deref(),
            Some("select-window -t @2")
        );
        assert_eq!(
            command_for(&TmuxIntent::SelectPane(5)).as_deref(),
            Some("select-pane -t %5")
        );
        // The two channel-level intents carry no command line (dispatch handles them).
        assert_eq!(command_for(&TmuxIntent::StartClient), None);
        assert_eq!(command_for(&TmuxIntent::RefreshSessions), None);
    }

    #[test]
    fn command_for_maps_each_window_and_pane_op() {
        assert_eq!(
            command_for(&TmuxIntent::NewWindow).as_deref(),
            Some("new-window")
        );
        assert_eq!(
            command_for(&TmuxIntent::KillWindow(3)).as_deref(),
            Some("kill-window -t @3")
        );
        assert_eq!(
            command_for(&TmuxIntent::RenameWindow(3, "ops".to_owned())).as_deref(),
            Some("rename-window -t @3 'ops'")
        );
        assert_eq!(
            command_for(&TmuxIntent::SplitPane(1, SplitDir::V)).as_deref(),
            Some("split-window -t %1 -h")
        );
        assert_eq!(
            command_for(&TmuxIntent::SplitPane(1, SplitDir::H)).as_deref(),
            Some("split-window -t %1 -v")
        );
        assert_eq!(
            command_for(&TmuxIntent::ClosePane(4)).as_deref(),
            Some("kill-pane -t %4")
        );
        assert_eq!(
            command_for(&TmuxIntent::ZoomPane(4)).as_deref(),
            Some("resize-pane -t %4 -Z")
        );
        assert_eq!(
            command_for(&TmuxIntent::BreakPane(4)).as_deref(),
            Some("break-pane -s %4")
        );
        assert_eq!(
            command_for(&TmuxIntent::JoinPane { src: 4, window: 2 }).as_deref(),
            Some("join-pane -v -s %4 -t @2")
        );
        assert_eq!(
            command_for(&TmuxIntent::SwapPanes(1, 5)).as_deref(),
            Some("swap-pane -d -s %1 -t %5")
        );
        assert_eq!(
            command_for(&TmuxIntent::MoveWindowBefore { src: 3, dst: 1 }).as_deref(),
            Some("move-window -b -s @3 -t @1")
        );
        assert_eq!(
            command_for(&TmuxIntent::MoveWindowAfter { src: 1, dst: 3 }).as_deref(),
            Some("move-window -a -s @1 -t @3")
        );
        assert_eq!(
            command_for(&TmuxIntent::RenamePane(7, "logs".to_owned())).as_deref(),
            Some("select-pane -t %7 -T 'logs'")
        );
        assert_eq!(
            command_for(&TmuxIntent::ResizePaneWidth(2, 55)).as_deref(),
            Some("resize-pane -t %2 -x 55")
        );
        assert_eq!(
            command_for(&TmuxIntent::ResizePaneHeight(2, 14)).as_deref(),
            Some("resize-pane -t %2 -y 14")
        );
        assert_eq!(
            command_for(&TmuxIntent::ClientResize(120, 40)).as_deref(),
            Some("refresh-client -C 120x40")
        );
    }

    fn model_from(stream: &[u8]) -> TmuxModel {
        let mut model = TmuxModel::new();
        let mut parser = Parser::new();
        for note in parser.feed(stream) {
            model.apply(note);
        }
        model
    }

    fn headless(mut add: impl FnMut(&mut Ui)) -> usize {
        let ctx = Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(480.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| add(ui));
        });
        ctx.tessellate(out.shapes, out.pixels_per_point).len()
    }

    // The render smoke tests assert the tessellate path completes without a
    // layout/paint panic (a non-empty primitive set), the same idiom the sibling
    // `panel`/`widget` mount tests use — egui batches shapes into one clipped
    // primitive per clip/texture, so a *count* can't prove "more content".

    // ── the tree folds a fixture model (the acceptance's "tree from a model") ─

    #[test]
    fn tree_renders_a_fixture_session_headless() {
        // A session with one window (two panes) — the live control-mode shape.
        let model = model_from(
            b"%session-changed $0 main\n\
              %window-add @0\n\
              %layout-change @0 f9d3,80x24,0,0{40x24,0,0,1,39x24,41,0,2}\n\
              %window-renamed @0 editor\n\
              %window-pane-changed @0 %1\n",
        );
        assert_eq!(model.current_session(), Some(0));
        assert_eq!(model.panes_of_window(0), vec![1, 2]);

        let prims = headless(|ui| {
            let mut state = ChromeUi::default();
            let mut intents = Vec::new();
            let mut rows = Vec::new();
            // No controller (no live channel), but the tree render reads the model
            // directly — drive the session-node folder over the fixture.
            session_node(ui, &model, &mut state, &mut intents, &mut rows);
            // Every window + pane became a drag/drop-addressable row.
            assert_eq!(rows.len(), 3, "1 window row + 2 pane rows, got {rows:?}");
        });
        assert!(prims > 0, "the fixture session tree did not tessellate");
    }

    #[test]
    fn no_session_state_offers_a_start_affordance_headless() {
        // The empty state raises `StartClient` when its one button is "clicked" —
        // exercised here by rendering; the button existing proves the opt-in path.
        let prims = headless(|ui| {
            let mut intents = Vec::new();
            no_session(ui, None, &mut intents);
        });
        assert!(prims > 0, "the empty tmux state did not tessellate");
    }

    // ── the picker lists attached + detached (the acceptance's picker) ───────

    #[test]
    fn picker_lists_attached_and_detached_rows_headless() {
        let attached = SessionInfo {
            name: "main".to_owned(),
            attached: true,
            windows: 3,
        };
        let detached = SessionInfo {
            name: "build".to_owned(),
            attached: false,
            windows: 1,
        };
        // The picker source is the controller's all_sessions; here render the rows
        // directly (the same fn the picker body calls) to prove both shapes paint.
        let mut intents = Vec::new();
        let prims = headless(|ui| {
            session_row(ui, &attached, &mut intents);
            session_row(ui, &detached, &mut intents);
        });
        assert!(prims > 0, "the picker rows did not tessellate");
    }

    // ── the chrome is opt-in: nothing attaches until asked (lock #16) ────────

    #[test]
    fn a_fresh_chrome_is_inactive_and_the_hidden_tree_is_a_no_op() {
        let mut chrome = TmuxChrome::new();
        assert!(
            !chrome.is_active(),
            "a fresh chrome must not auto-attach tmux"
        );
        assert!(!chrome.tree_open(), "the tree starts hidden (opt-in)");
        // Rendering the hidden tree is a no-op that attaches nothing — the surface
        // pays nothing for tmux until the user opts in.
        let _ = headless(|ui| chrome.sidebar(ui));
        assert!(
            !chrome.is_active(),
            "rendering a hidden tree must not have attached anything"
        );
        assert!(!chrome.tree_open(), "a hidden tree stays hidden");
        // And the window view honestly declines to mount — the surface paints
        // its native terminal instead (lock #3 coexistence).
        let _ = headless(|ui| {
            assert!(!chrome.window_body(ui), "no client → no tmux body");
        });
        assert!(chrome.mounts.is_empty(), "nothing mounted without a client");
    }

    // ── the drop rules (drag-reorder + swap/join), pure ───────────────────────

    #[test]
    fn tab_drop_moves_before_the_tab_right_of_the_pointer() {
        let tabs = vec![
            (10, Rect::from_min_max(pos2(0.0, 0.0), pos2(50.0, 20.0))),
            (11, Rect::from_min_max(pos2(50.0, 0.0), pos2(100.0, 20.0))),
            (12, Rect::from_min_max(pos2(100.0, 0.0), pos2(150.0, 20.0))),
        ];
        // Dropped at the strip's left edge: before the first tab.
        assert_eq!(
            tab_drop_intent(12, &tabs, 10.0),
            Some(TmuxIntent::MoveWindowBefore { src: 12, dst: 10 })
        );
        // Dropped between the second and third centres: before the third.
        assert_eq!(
            tab_drop_intent(10, &tabs, 90.0),
            Some(TmuxIntent::MoveWindowBefore { src: 10, dst: 12 })
        );
        // Dropped past every centre: after the last.
        assert_eq!(
            tab_drop_intent(10, &tabs, 400.0),
            Some(TmuxIntent::MoveWindowAfter { src: 10, dst: 12 })
        );
        // Dropped on itself, or with a lone tab: honestly nothing.
        assert_eq!(tab_drop_intent(11, &tabs, 60.0), None);
        assert_eq!(tab_drop_intent(12, &tabs, 400.0), None);
        assert_eq!(tab_drop_intent(10, &tabs[..1], 10.0), None);
    }

    #[test]
    fn pane_drop_swaps_on_a_pane_and_joins_on_another_window() {
        let rows = vec![
            (
                RowTarget::Window(0),
                Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 20.0)),
            ),
            (
                RowTarget::Pane(1),
                Rect::from_min_max(pos2(0.0, 20.0), pos2(100.0, 40.0)),
            ),
            (
                RowTarget::Pane(2),
                Rect::from_min_max(pos2(0.0, 40.0), pos2(100.0, 60.0)),
            ),
            (
                RowTarget::Window(7),
                Rect::from_min_max(pos2(0.0, 60.0), pos2(100.0, 80.0)),
            ),
        ];
        // Onto another pane row: swap the two panes.
        assert_eq!(
            pane_drop_intent(1, Some(0), &rows, pos2(50.0, 50.0)),
            Some(TmuxIntent::SwapPanes(1, 2))
        );
        // Onto a different window's row: join (move) the pane there.
        assert_eq!(
            pane_drop_intent(1, Some(0), &rows, pos2(50.0, 70.0)),
            Some(TmuxIntent::JoinPane { src: 1, window: 7 })
        );
        // Onto its own row / its own window / empty space: honestly nothing.
        assert_eq!(pane_drop_intent(1, Some(0), &rows, pos2(50.0, 30.0)), None);
        assert_eq!(pane_drop_intent(1, Some(0), &rows, pos2(50.0, 10.0)), None);
        assert_eq!(pane_drop_intent(1, Some(0), &rows, pos2(50.0, 200.0)), None);
    }

    // ── the divider drag → resize-pane glue ──────────────────────────────────

    #[test]
    fn resize_intent_maps_the_drag_through_the_window_layout() {
        let model = model_from(
            b"%window-add @0\n\
              %layout-change @0 f9d3,80x24,0,0{40x24,0,0,1,39x24,41,0,2}\n",
        );
        // The root divider dragged to a quarter: pane 1 → 20 of 79 cells wide.
        assert_eq!(
            resize_intent(&model, 0, NodePath::ROOT, 0.25),
            Some(TmuxIntent::ResizePaneWidth(1, 20))
        );
        // A window without a layout yields nothing.
        assert_eq!(resize_intent(&model, 9, NodePath::ROOT, 0.25), None);
    }

    // ── the mounted window view over a fixture model ─────────────────────────

    #[test]
    fn view_body_mounts_a_widget_per_pane_and_reports_the_client_grid() {
        let model = model_from(
            b"%session-changed $0 main\n\
              %window-add @0\n\
              %layout-change @0 f9d3,80x24,0,0{40x24,0,0,1,39x24,41,0,2}\n\
              %window-pane-changed @0 %1\n\
              %output %1 alpha\n\
              %output %2 beta\n",
        );
        let (tx, _rx) = mpsc::channel::<Vec<u8>>();
        let sink = CommandSink::for_tests(tx);
        let mut mounts: HashMap<u32, TerminalWidget> = HashMap::new();
        let mut state = ChromeUi::default();
        let mut intents: Vec<TmuxIntent> = Vec::new();

        let prims = headless(|ui| {
            view_body(ui, &model, 0, &mut state, &mut mounts, &sink, &mut intents);
        });
        assert!(prims > 0, "the mounted tmux view did not tessellate");
        // Each tmux pane got a live TERM-3 widget over its shared engine.
        assert_eq!(mounts.len(), 2, "one widget per pane, got {}", mounts.len());
        // The view reported its cell grid as the client size (refresh-client).
        assert!(
            intents
                .iter()
                .any(|i| matches!(i, TmuxIntent::ClientResize(..))),
            "no client-size report in {intents:?}"
        );
    }

    #[test]
    fn tab_strip_paints_the_window_tabs_headless() {
        let model = model_from(
            b"%session-changed $0 main\n\
              %window-add @0\n\
              %window-renamed @0 editor\n\
              %window-add @1\n\
              %window-renamed @1 logs\n\
              %session-window-changed $0 @1\n",
        );
        let mut state = ChromeUi::default();
        let mut intents: Vec<TmuxIntent> = Vec::new();
        let prims = headless(|ui| {
            tab_strip(ui, &model, 1, &mut state, &mut intents);
        });
        assert!(prims > 0, "the tab strip did not tessellate");
    }
}
