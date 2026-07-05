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
//!
//! **TMUX-FC-4 — the native chrome** (design locks #4/#8/#15): the **native
//! Quasar status bar** ([`status_bar`] — session name · the window list · a
//! clock, all `Style` tokens, deliberately ignoring the user's tmux `status-*`
//! config), the **toolbar** ([`toolbar`] — one-click pane/window ops resolved
//! through the same [`op_intents`] targets the menu uses), the **curated
//! ~30-command fuzzy palette** ([`PALETTE_COMMANDS`] + [`fuzzy_score`] — every
//! row an intent against the live model), and the **enriched context menus**
//! on window tabs + pane rows — including the `join-pane -h` trigger FC-3 left
//! open (the pane row's "Join Into Window ▸ beside" items). Every affordance
//! still issues a tmux command through the FC-1/2/3 round-trip; `%`-events
//! (and the tagged replies) reconcile the model — never a direct tree edit.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use mde_egui::egui::{
    self, pos2, Align2, Button, CursorIcon, FontId, Key, Modifiers, Pos2, Rect, RichText,
    ScrollArea, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2,
};
use mde_egui::Style;

use crate::mesh_tmux::MeshControlChannel;
use crate::remote::PtyBus;
use crate::roster::{RosterClient, RosterSnapshot};
use crate::splits::{self, NodePath, SplitDir};
use crate::tmux::{
    commands, resize_for_divider, CommandSink, LayoutDir, ResizeDir, SessionInfo, Status,
    StockLayout, TmuxController, TmuxLaunch, TmuxModel, TmuxPane, TmuxPaneIo, TmuxSession,
    TmuxWindow,
};
use crate::widget::TerminalWidget;

/// The default grid a mesh control channel dials at, before the mounted view's
/// `refresh-client -C` sizes it to the real on-screen rect (TMUX-FC-6).
const MESH_DIAL_COLS: u16 = 80;
/// See [`MESH_DIAL_COLS`].
const MESH_DIAL_ROWS: u16 = 24;

/// The pointer slop either side of a divider strip that still grabs it.
const DIVIDER_HIT_SLOP: f32 = 3.0;

/// The status bar's height (design lock #8) — one `SP_XL` band, like the strip.
const STATUS_BAR_H: f32 = Style::SP_XL;

/// How many cells a toolbar/palette resize nudge moves a pane.
const RESIZE_STEP: u16 = 5;

/// Seconds in a civil day — the clock fold's modulus.
const DAY_SECS: i64 = 86_400;

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
    /// Join (move) a pane into another window (`join-pane -h/-v -s % -t @`) —
    /// the pane-row drag onto a window row (stacked), or the pane context
    /// menu's "Join Into Window" items (beside = `-h`, the FC-4 trigger).
    JoinPane {
        /// The pane being moved.
        src: u32,
        /// The window it joins.
        window: u32,
        /// How it lands: a native [`SplitDir::V`] (side by side) is tmux's
        /// `-h`, [`SplitDir::H`] (stacked) its `-v` — the same mapping as
        /// [`TmuxIntent::SplitPane`].
        dir: SplitDir,
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
    // ── TMUX-FC-4: navigation + arrangement (status bar / toolbar / palette) ─
    /// Step the current window forward (`next-window`).
    NextWindow,
    /// Step the current window back (`previous-window`).
    PrevWindow,
    /// Jump to the most recently used window (`last-window`).
    LastWindow,
    /// Cycle the active pane forward (`select-pane -t :.+`).
    NextPane,
    /// Cycle the active pane back (`select-pane -t :.-`).
    PrevPane,
    /// Swap a pane with the next one (`swap-pane -D`).
    SwapPaneNext(u32),
    /// Swap a pane with the previous one (`swap-pane -U`).
    SwapPanePrev(u32),
    /// Nudge a pane by cells in a direction (`resize-pane -L/-R/-U/-D`).
    ResizePaneBy(u32, ResizeDir, u16),
    /// Re-apply a stock layout to a window (`select-layout`).
    SelectLayout(u32, StockLayout),
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
        TmuxIntent::JoinPane { src, window, dir } => Some(commands::join_pane(*src, *window, *dir)),
        TmuxIntent::SwapPanes(a, b) => Some(commands::swap_panes(*a, *b)),
        TmuxIntent::MoveWindowBefore { src, dst } => Some(commands::move_window_before(*src, *dst)),
        TmuxIntent::MoveWindowAfter { src, dst } => Some(commands::move_window_after(*src, *dst)),
        TmuxIntent::RenamePane(p, title) => Some(commands::rename_pane(*p, title)),
        TmuxIntent::ResizePaneWidth(p, cols) => Some(commands::resize_pane_width(*p, *cols)),
        TmuxIntent::ResizePaneHeight(p, rows) => Some(commands::resize_pane_height(*p, *rows)),
        TmuxIntent::ClientResize(cols, rows) => Some(commands::refresh_client_size(*cols, *rows)),
        TmuxIntent::NextWindow => Some(commands::next_window()),
        TmuxIntent::PrevWindow => Some(commands::previous_window()),
        TmuxIntent::LastWindow => Some(commands::last_window()),
        TmuxIntent::NextPane => Some(commands::select_pane_next()),
        TmuxIntent::PrevPane => Some(commands::select_pane_prev()),
        TmuxIntent::SwapPaneNext(p) => Some(commands::swap_pane_next(*p)),
        TmuxIntent::SwapPanePrev(p) => Some(commands::swap_pane_prev(*p)),
        TmuxIntent::ResizePaneBy(p, dir, cells) => Some(commands::resize_pane(*p, *dir, *cells)),
        TmuxIntent::SelectLayout(w, layout) => Some(commands::select_layout(*w, *layout)),
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
    /// Open the FC-4 fuzzy command palette (starting a client first if needed).
    ShowPalette,
    /// Open the TMUX-FC-5 templates ("projects") window.
    ShowTemplates,
    /// Open the TMUX-FC-6 mesh peer picker (attach tmux on a node).
    ShowMesh,
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

/// One pane's edit row in the template editor — just its seeded command line
/// (empty = a bare shell).
#[derive(Clone, Default, PartialEq, Eq, Debug)]
struct PaneEdit {
    /// The command line to seed (blank leaves a plain shell).
    command: String,
}

/// One window's edit block in the template editor (TMUX-FC-5): a name + its
/// panes, plus how the panes split.
#[derive(Clone, PartialEq, Eq, Debug)]
struct WindowEdit {
    /// The window name.
    name: String,
    /// The panes (at least one), in order.
    panes: Vec<PaneEdit>,
    /// How each extra pane splits off (beside / stacked).
    split: SplitDir,
}

impl Default for WindowEdit {
    fn default() -> Self {
        Self {
            name: String::new(),
            panes: vec![PaneEdit::default()],
            split: SplitDir::V,
        }
    }
}

/// The in-progress template editor (TMUX-FC-5): a name + windows, authored fresh
/// ("New template") or captured from the live session ("Save current as
/// template"). Saving converts it to a [`crate::SessionTemplate`] blueprint.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
struct TemplateEdit {
    /// The template name (and, sanitised, the session it opens).
    name: String,
    /// The windows to build.
    windows: Vec<WindowEdit>,
}

impl TemplateEdit {
    /// Convert the editor into a persistable template blueprint: each window's
    /// non-blank panes become [`crate::BlueprintPane`]s, evened out `tiled`.
    fn to_template(&self) -> crate::SessionTemplate {
        let windows = self
            .windows
            .iter()
            .map(|w| {
                let panes: Vec<crate::BlueprintPane> = w
                    .panes
                    .iter()
                    .map(|p| {
                        let cmd = p.command.trim();
                        if cmd.is_empty() {
                            crate::BlueprintPane::shell()
                        } else {
                            crate::BlueprintPane::cmd(cmd)
                        }
                    })
                    .collect();
                let panes = if panes.is_empty() {
                    vec![crate::BlueprintPane::shell()]
                } else {
                    panes
                };
                crate::BlueprintWindow::new(w.name.trim(), panes, w.split, Some(StockLayout::Tiled))
            })
            .collect();
        crate::SessionTemplate::new(self.name.trim(), crate::Blueprint::new(windows))
    }
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
// Several independent open/reveal toggles — idiomatic egui panel state, not a
// state machine worth encoding as flags.
#[allow(clippy::struct_excessive_bools)]
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
    /// The FC-4 command palette's own state (open/query/selection).
    palette: PaletteUi,
    /// Whether the TMUX-FC-5 templates ("projects") window is open.
    templates_open: bool,
    /// The in-progress template editor, if any (TMUX-FC-5).
    tpl_edit: Option<TemplateEdit>,
    /// Whether the TMUX-FC-6 mesh peer picker is open.
    mesh_open: bool,
    /// The manual-host buffer in the mesh picker (a node not on the roster).
    mesh_host: String,
}

impl ChromeUi {
    /// Open the FC-4 command palette fresh (empty query, first row selected).
    fn open_palette(&mut self) {
        self.palette = PaletteUi {
            open: true,
            ..PaletteUi::default()
        };
    }
}

/// The FC-4 command palette's UI state — its own struct so each chrome
/// affordance keeps one flat toggle.
#[derive(Default)]
struct PaletteUi {
    /// Whether the overlay is open.
    open: bool,
    /// The fuzzy query.
    query: String,
    /// The keyboard-selected row (an index into the filtered list).
    sel: usize,
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
    /// TMUX-FC-5 — the platform-managed persisted state (remembered session +
    /// templates) + its atomic store.
    store: crate::TmuxStateStore,
    /// The loaded persisted state.
    state: crate::TmuxState,
    /// Whether the first-frame auto-reattach (TMUX-FC-5) has run yet.
    booted: bool,
    /// TMUX-FC-6 — the mesh peer roster source (reachable nodes to attach on).
    roster: Option<Arc<dyn RosterClient>>,
    /// TMUX-FC-6 — the Bus PTY-broker seam a mesh attach dials `tmux -CC` over.
    bus: Option<Arc<dyn PtyBus>>,
}

impl TmuxChrome {
    /// A fresh chrome with no live tmux session (the sidebar hidden), loading the
    /// TMUX-FC-5 persisted state (remembered session + saved templates) from the
    /// platform config dir. The remembered session is not re-entered until the
    /// first [`Self::pump`] (so construction stays cheap + side-effect-free).
    #[must_use]
    pub fn new() -> Self {
        let store = crate::TmuxStateStore::from_env();
        let state = store.load();
        Self {
            store,
            state,
            roster: Some(Arc::new(crate::roster::BusRoster::from_env())),
            bus: Some(Arc::new(crate::remote::BusPtyClient::from_env())),
            ..Self::default()
        }
    }

    /// A chrome over an explicit persisted store + state (the TMUX-FC-5 test
    /// seam — points the store at a tempdir instead of the live config dir).
    #[cfg(test)]
    pub(crate) fn with_store(store: crate::TmuxStateStore, state: crate::TmuxState) -> Self {
        Self {
            store,
            state,
            ..Self::default()
        }
    }

    /// The saved templates (the test/read view).
    #[cfg(test)]
    pub(crate) fn templates(&self) -> &[crate::SessionTemplate] {
        &self.state.templates
    }

    /// Drain the control channel into the model — call once per frame, before the
    /// tree renders (the [`crate::panel::terminal_pump`] slot). On the very first
    /// call it performs the TMUX-FC-5 **auto-reattach**: if a session was
    /// remembered, a control client re-enters it (`new-session -A` — a still-live
    /// detached session is resumed, a killed one recreated) and the tree opens.
    pub fn pump(&mut self) {
        if !self.booted {
            self.booted = true;
            if let Some(name) = self.state.last_session.clone() {
                if !self.is_active() {
                    self.controller = Some(TmuxController::connect(&TmuxLaunch::session(&name)));
                    self.ui.tree_open = true;
                }
            }
        }
        if let Some(ctrl) = self.controller.as_mut() {
            ctrl.pump();
        }
        // Remember whatever session the client is actually attached to (the
        // round-trip truth), so the next relaunch reattaches it.
        self.remember_current();
    }

    /// Persist the currently-attached session name as the one to reattach on the
    /// next launch — only when it actually changed (no per-frame write churn).
    fn remember_current(&mut self) {
        let current = self.controller.as_ref().and_then(|c| {
            let model = c.model();
            model
                .current_session()
                .and_then(|s| model.session(s))
                .map(|s| s.name().to_owned())
        });
        if let Some(name) = current {
            if self.state.last_session.as_deref() != Some(name.as_str()) {
                self.state.last_session = Some(name);
                let _ = self.store.save(&self.state);
            }
        }
    }

    /// Open a saved template ("project"): ensure a control client, then write
    /// its blueprint — a fresh session built from the recipe the client switches
    /// onto. The `%`-events reconcile the tree (the round-trip, as ever).
    fn open_template(&mut self, index: usize) {
        let Some(tpl) = self.state.templates.get(index).cloned() else {
            return;
        };
        self.ensure_client();
        self.ui.tree_open = true;
        let session = crate::session_safe(&tpl.name);
        if let Some(ctrl) = self.controller.as_ref() {
            for line in tpl.blueprint.commands(&session) {
                let _ = ctrl.send(&line);
            }
        }
    }

    /// TMUX-FC-6 — dial `tmux -CC` on a mesh peer over the Bus PTY broker and
    /// drive it with this same chrome. The controller/model/tree are
    /// transport-agnostic ([`crate::tmux::ControlLink`]), so the sidebar, tabs,
    /// panes, status bar, toolbar, and palette all control the remote session
    /// exactly as a local one; the FC-2 all-sessions picker then enumerates the
    /// peer's sessions (`list-sessions` round-trips over the broker). Replaces any
    /// current controller — one active session at a time. A blank host, or no Bus
    /// resolved, is honestly a no-op (§7 — never a fabricated remote attach).
    fn attach_peer(&mut self, host: &str) {
        let host = host.trim();
        let Some(bus) = self.bus.clone() else {
            return;
        };
        if host.is_empty() {
            return;
        }
        let channel = MeshControlChannel::dial(bus, host, "main", MESH_DIAL_COLS, MESH_DIAL_ROWS);
        self.controller = Some(TmuxController::over(Box::new(channel)));
        self.ui.tree_open = true;
        self.ui.mesh_open = false;
    }

    /// The reachable-peer roster for the FC-6 picker (empty when no roster is
    /// published — the picker then leans on manual host entry).
    fn mesh_roster(&self) -> Option<RosterSnapshot> {
        self.roster.as_ref().and_then(|r| r.snapshot())
    }

    /// Persist the edited template (append or replace by name) — the editor's
    /// Save. A blank name is ignored (§7 — no nameless template).
    fn save_template(&mut self, edit: &TemplateEdit) {
        if edit.name.trim().is_empty() {
            return;
        }
        let tpl = edit.to_template();
        if let Some(slot) = self.state.templates.iter_mut().find(|t| t.name == tpl.name) {
            *slot = tpl;
        } else {
            self.state.templates.push(tpl);
        }
        let _ = self.store.save(&self.state);
    }

    /// Delete a saved template by index + persist.
    fn delete_template(&mut self, index: usize) {
        if index < self.state.templates.len() {
            self.state.templates.remove(index);
            let _ = self.store.save(&self.state);
        }
    }

    /// Capture the live session's window/pane structure into a fresh template
    /// editor (its commands left blank for the user to fill) — "Save current as
    /// template". Falls back to a one-window skeleton when nothing is live.
    fn capture_template_edit(&self) -> TemplateEdit {
        let windows = self
            .controller
            .as_ref()
            .map(|ctrl| capture_windows(ctrl.model()))
            .unwrap_or_default();
        TemplateEdit {
            name: String::new(),
            windows: if windows.is_empty() {
                vec![WindowEdit::default()]
            } else {
                windows
            },
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
            Some(TmuxMenuChoice::ShowPalette) => {
                self.ensure_client();
                self.ui.open_palette();
            }
            Some(TmuxMenuChoice::ShowTemplates) => self.ui.templates_open = true,
            Some(TmuxMenuChoice::ShowMesh) => self.ui.mesh_open = true,
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
        self.controller
            .as_ref()
            .map(TmuxController::model)
            .map_or_else(Vec::new, |model| op_intents(model, op))
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
                ..
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
            toolbar(ui, model, state, &mut intents);
            // Carve the remaining body: the mounted pane tree above the FC-4
            // native status bar (a fixed bottom band — deterministic, no
            // panel-allocation order games).
            let avail = ui.available_rect_before_wrap();
            let split_y = (avail.max.y - STATUS_BAR_H).max(avail.min.y);
            let body = Rect::from_min_max(avail.min, pos2(avail.max.x, split_y));
            let status = Rect::from_min_max(pos2(avail.min.x, split_y), avail.max);
            {
                let mut body_ui =
                    ui.new_child(UiBuilder::new().max_rect(body).id_salt("tmux-view-body"));
                view_body(
                    &mut body_ui,
                    model,
                    window,
                    state,
                    mounts,
                    &sink,
                    &mut intents,
                );
            }
            {
                let mut status_ui =
                    ui.new_child(UiBuilder::new().max_rect(status).id_salt("tmux-status-bar"));
                status_bar(&mut status_ui, model, window, &mut intents);
            }
            if state.palette.open {
                render_palette(ui, model, state, &mut intents);
            }
            true
        };
        self.dispatch(intents);
        mounted
    }

    /// Render the TMUX-FC-5 **templates** overlays (the projects window + its
    /// editor) — floating egui windows, so they mount regardless of whether a
    /// control client is live (a template can *start* a session from cold). The
    /// [`crate::panel`] drives this each frame after the body.
    pub fn overlays(&mut self, ui: &mut Ui) {
        if self.ui.templates_open {
            match render_templates(ui, &self.state.templates) {
                Some(TemplateAction::Open(i)) => self.open_template(i),
                Some(TemplateAction::Delete(i)) => self.delete_template(i),
                Some(TemplateAction::NewEditor) => {
                    self.ui.tpl_edit = Some(TemplateEdit {
                        name: String::new(),
                        windows: vec![WindowEdit::default()],
                    });
                }
                Some(TemplateAction::CaptureEditor) => {
                    self.ui.tpl_edit = Some(self.capture_template_edit());
                }
                Some(TemplateAction::Close) => self.ui.templates_open = false,
                None => {}
            }
        }
        if let Some(mut edit) = self.ui.tpl_edit.take() {
            match render_template_editor(ui, &mut edit) {
                Some(EditorAction::Save) => {
                    self.save_template(&edit);
                    // Keep the editor closed; the templates list now shows it.
                }
                Some(EditorAction::Cancel) => {}
                None => self.ui.tpl_edit = Some(edit),
            }
        }
        // TMUX-FC-6 — the mesh peer picker (attach tmux on any node).
        if self.ui.mesh_open {
            let roster = self.mesh_roster();
            match render_mesh_picker(ui, roster.as_ref(), &mut self.ui.mesh_host) {
                Some(MeshAction::Attach(host)) => self.attach_peer(&host),
                Some(MeshAction::Close) => self.ui.mesh_open = false,
                None => {}
            }
        }
    }

    /// Whether the templates window is open (the menu's toggle-state twin).
    #[must_use]
    pub const fn templates_open(&self) -> bool {
        self.ui.templates_open
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

/// A window's acting pane: its active pane, falling back to its first — the
/// target every "act on the current pane" affordance resolves.
fn active_pane_of(model: &TmuxModel, window: u32) -> Option<u32> {
    model
        .window(window)
        .and_then(TmuxWindow::active_pane)
        .or_else(|| model.panes_of_window(window).first().copied())
}

/// Resolve a window/pane op against the model: the current window's acting
/// pane. The ONE target-resolution path the Tmux menu, the FC-4 toolbar, and
/// the palette all share (§6 — one dispatch path). Honest nothing when there is
/// no live window/pane to act on, or for the chrome-level choices the surface
/// handles itself.
fn op_intents(model: &TmuxModel, op: TmuxMenuChoice) -> Vec<TmuxIntent> {
    if op == TmuxMenuChoice::NewWindow {
        return vec![TmuxIntent::NewWindow];
    }
    let Some(window) = active_window(model) else {
        return Vec::new();
    };
    if op == TmuxMenuChoice::KillWindow {
        return vec![TmuxIntent::KillWindow(window)];
    }
    let Some(pane) = active_pane_of(model, window) else {
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

/// The reorder intent nudging `window` one slot left/right in the strip order —
/// shared by the tab context menu and the palette. `None` at the edge (§7 —
/// honestly nothing, never a wrapped surprise).
fn nudge_window_intent(model: &TmuxModel, window: u32, left: bool) -> Option<TmuxIntent> {
    let order = model.windows_in_order();
    let pos = order.iter().position(|w| *w == window)?;
    if left {
        let dst = *order.get(pos.checked_sub(1)?)?;
        Some(TmuxIntent::MoveWindowBefore { src: window, dst })
    } else {
        let dst = *order.get(pos + 1)?;
        Some(TmuxIntent::MoveWindowAfter { src: window, dst })
    }
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
/// context menu ([`pane_context_menu`]) carries the FC-3 pane ops plus the
/// FC-4 swap/join items — each an intent that becomes a tmux command.
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
        resp.context_menu(|ui| pane_context_menu(ui, model, pane, title, state, intents));
        rows.push((RowTarget::Pane(pane), resp.rect));
    });
}

/// The pane row's context menu (FC-3 ops + the FC-4 additions): split · zoom ·
/// break · **swap next/previous** · the **Join Into Window** submenu — whose
/// "beside" items are the `join-pane -h` trigger FC-3 noted as missing — plus
/// rename-title and close. Every item raises an intent that becomes a tmux
/// command; the drag-drop join stays stacked (`-v`), this menu carries the
/// explicit direction choice.
fn pane_context_menu(
    ui: &mut Ui,
    model: &TmuxModel,
    pane: u32,
    title: &str,
    state: &mut ChromeUi,
    intents: &mut Vec<TmuxIntent>,
) {
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
    if ui.button("Swap With Next").clicked() {
        intents.push(TmuxIntent::SwapPaneNext(pane));
        ui.close_menu();
    }
    if ui.button("Swap With Previous").clicked() {
        intents.push(TmuxIntent::SwapPanePrev(pane));
        ui.close_menu();
    }
    ui.menu_button("Join Into Window", |ui| {
        let src_window = model.pane(pane).and_then(TmuxPane::window);
        let mut any = false;
        for w in model.windows_in_order() {
            if src_window == Some(w) {
                continue;
            }
            let Some(win) = model.window(w) else {
                continue;
            };
            any = true;
            if ui
                .button(format!("@{w} {} \u{2014} beside", win.name()))
                .clicked()
            {
                intents.push(TmuxIntent::JoinPane {
                    src: pane,
                    window: w,
                    dir: SplitDir::V, // tmux `join-pane -h`
                });
                ui.close_menu();
            }
            if ui
                .button(format!("@{w} {} \u{2014} stacked", win.name()))
                .clicked()
            {
                intents.push(TmuxIntent::JoinPane {
                    src: pane,
                    window: w,
                    dir: SplitDir::H, // tmux `join-pane -v`
                });
                ui.close_menu();
            }
        }
        if !any {
            ui.label(
                RichText::new("no other window")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
        }
    });
    if ui.button("Rename Title\u{2026}").clicked() {
        state.pane_rename = Some((pane, title.to_owned()));
        ui.close_menu();
    }
    if ui.button("Close Pane").clicked() {
        intents.push(TmuxIntent::ClosePane(pane));
        ui.close_menu();
    }
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
        RowTarget::Window(w) if src_window != Some(*w) => Some(TmuxIntent::JoinPane {
            src,
            window: *w,
            // A drop joins stacked (tmux's own default split direction); the
            // pane context menu carries the explicit beside/`-h` choice (FC-4).
            dir: SplitDir::H,
        }),
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
            // FC-4: the full window context menu — select · rename · reorder ·
            // new/kill, every item a tmux command through the round-trip.
            resp.context_menu(|ui| {
                if ui.button("Select").clicked() {
                    intents.push(TmuxIntent::SelectWindow(window));
                    ui.close_menu();
                }
                if ui.button("Rename\u{2026}").clicked() {
                    state.win_rename = Some((window, win.name().to_owned()));
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Move Left").clicked() {
                    intents.extend(nudge_window_intent(model, window, true));
                    ui.close_menu();
                }
                if ui.button("Move Right").clicked() {
                    intents.extend(nudge_window_intent(model, window, false));
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("New Window").clicked() {
                    intents.push(TmuxIntent::NewWindow);
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
// TMUX-FC-4 — the toolbar.
// ─────────────────────────────────────────────────────────────────────────────

/// The toolbar's op buttons: label · hover hint (naming the tmux command) ·
/// the shared menu-op it resolves through ([`op_intents`], §6 one dispatch
/// path). A const table so the affordance→command map is testable data.
const TOOLBAR_OPS: [(&str, &str, TmuxMenuChoice); 7] = [
    (
        "Split \u{2192}",
        "Split the active pane beside (split-window -h)",
        TmuxMenuChoice::SplitRight,
    ),
    (
        "Split \u{2193}",
        "Split the active pane below (split-window -v)",
        TmuxMenuChoice::SplitDown,
    ),
    (
        "Zoom",
        "Toggle the active pane's zoom (resize-pane -Z)",
        TmuxMenuChoice::ZoomPane,
    ),
    (
        "Break",
        "Break the active pane out to its own window (break-pane)",
        TmuxMenuChoice::BreakPane,
    ),
    (
        "\u{00d7} Pane",
        "Close the active pane (kill-pane)",
        TmuxMenuChoice::ClosePane,
    ),
    (
        "+ Win",
        "Open a fresh window (new-window)",
        TmuxMenuChoice::NewWindow,
    ),
    (
        "\u{00d7} Win",
        "Kill the current window (kill-window)",
        TmuxMenuChoice::KillWindow,
    ),
];

/// FC-4 — the toolbar between the tab strip and the mounted view: one-click
/// pane/window ops (each resolving its target through [`op_intents`], exactly
/// as the Tmux menu does) plus the palette, sidebar and detach affordances.
/// All `Style` tokens (§4); every button's hover text names its tmux command.
fn toolbar(ui: &mut Ui, model: &TmuxModel, state: &mut ChromeUi, intents: &mut Vec<TmuxIntent>) {
    ui.horizontal_wrapped(|ui| {
        ui.add_space(Style::SP_XS);
        for (label, hint, op) in TOOLBAR_OPS {
            let resp = ui
                .add(Button::new(
                    RichText::new(label).size(Style::SMALL).color(Style::TEXT),
                ))
                .on_hover_text(hint);
            if resp.clicked() {
                intents.extend(op_intents(model, op));
            }
        }
        ui.add_space(Style::SP_S);
        if ui
            .add(Button::new(
                RichText::new("Commands\u{2026}")
                    .size(Style::SMALL)
                    .color(Style::ACCENT_TERMINALS),
            ))
            .on_hover_text("The tmux command palette (fuzzy, ~30 curated ops)")
            .clicked()
        {
            state.open_palette();
        }
        if ui
            .add(Button::new(
                RichText::new("Projects\u{2026}")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            ))
            .on_hover_text("Saved session templates (TMUX-FC-5)")
            .clicked()
        {
            state.templates_open = true;
        }
        if ui
            .add(Button::new(
                RichText::new("Mesh\u{2026}")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            ))
            .on_hover_text("Attach tmux on a mesh node (TMUX-FC-6)")
            .clicked()
        {
            state.mesh_open = true;
        }
        if ui
            .add(Button::new(
                RichText::new(if state.tree_open {
                    "Tree \u{25C0}"
                } else {
                    "Tree \u{25B6}"
                })
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
            ))
            .on_hover_text("Show/hide the session tree sidebar")
            .clicked()
        {
            state.tree_open = !state.tree_open;
        }
        if ui
            .add(Button::new(
                RichText::new("Detach")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            ))
            .on_hover_text(
                "Detach this control client (detach-client) \u{2014} the session keeps running",
            )
            .clicked()
        {
            intents.push(TmuxIntent::Detach);
        }
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// TMUX-FC-4 — the native Quasar status bar.
// ─────────────────────────────────────────────────────────────────────────────

/// FC-4 — the **native Quasar status bar** (design lock #8): the session name,
/// the window list (current highlighted, `Z` while zoomed; a click round-trips
/// `select-window`), and a wall clock — rendered from the live model through
/// `Style` tokens and deliberately **ignoring the user's tmux `status-*`
/// config** (tmux's own status line never renders here; panes carry only their
/// content, so the platform look is the one look).
fn status_bar(ui: &mut Ui, model: &TmuxModel, current: u32, intents: &mut Vec<TmuxIntent>) {
    let rect = ui.available_rect_before_wrap();
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, Style::SURFACE);
    painter.hline(rect.x_range(), rect.min.y, Stroke::new(1.0, Style::BORDER));

    ui.horizontal_centered(|ui| {
        ui.add_space(Style::SP_S);
        let name = model
            .current_session()
            .and_then(|s| model.session(s))
            .map_or("tmux", TmuxSession::name);
        ui.label(
            RichText::new(format!("[{name}]"))
                .size(Style::SMALL)
                .color(Style::ACCENT_TERMINALS)
                .strong(),
        );
        ui.add_space(Style::SP_S);
        for window in model.windows_in_order() {
            let Some(win) = model.window(window) else {
                continue;
            };
            let idx = win
                .index()
                .map_or_else(|| format!("@{window}"), |i| i.to_string());
            let mark = if window == current { "*" } else { "" };
            let zoom = if win.is_zoomed() { "Z" } else { "" };
            let color = if window == current {
                Style::TEXT
            } else {
                Style::TEXT_DIM
            };
            let resp = ui.add(
                Button::new(
                    RichText::new(format!("{idx}:{}{mark}{zoom}", win.name()))
                        .size(Style::SMALL)
                        .color(color),
                )
                .frame(false),
            );
            if window == current {
                ui.painter().rect_filled(
                    Rect::from_min_max(pos2(resp.rect.min.x, resp.rect.max.y - 1.0), resp.rect.max),
                    0.0,
                    Style::ACCENT,
                );
            }
            if resp.clicked() {
                intents.push(TmuxIntent::SelectWindow(window));
            }
        }
        // The clock, right-aligned; repaint scheduled at the minute rollover
        // so the painted minute is never stale.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(Style::SP_S);
            let now = now_unix();
            ui.label(
                RichText::new(hhmm(now))
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            ui.ctx()
                .request_repaint_after(Duration::from_secs(secs_to_next_minute(now).max(1)));
        });
    });
}

/// Seconds since the Unix epoch (0 on a pre-epoch clock — never a panic).
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// The wall-clock `HH:MM` for a Unix timestamp — the platform's one tiny clock
/// fold (the shell dock's `timers::hhmm`), restated here because the terminal
/// tier cannot reach across surface crates (§6).
fn hhmm(unix_secs: i64) -> String {
    let tod = unix_secs.rem_euclid(DAY_SECS);
    format!("{:02}:{:02}", tod / 3600, (tod % 3600) / 60)
}

/// Seconds until the next minute rollover — the status clock's repaint alarm.
fn secs_to_next_minute(unix_secs: i64) -> u64 {
    let into = unix_secs.rem_euclid(60);
    u64::try_from(60 - into).unwrap_or(60)
}

// ─────────────────────────────────────────────────────────────────────────────
// TMUX-FC-4 — the curated fuzzy command palette.
// ─────────────────────────────────────────────────────────────────────────────

/// One palette row's semantic op — resolved against the live model + chrome
/// state by [`palette_resolve`] when chosen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PaletteOp {
    NewWindow,
    KillWindow,
    RenameWindow,
    NextWindow,
    PrevWindow,
    LastWindow,
    MoveWindowLeft,
    MoveWindowRight,
    SplitRight,
    SplitDown,
    ClosePane,
    ZoomPane,
    BreakPane,
    NextPane,
    PrevPane,
    RenamePaneTitle,
    SwapPaneNext,
    SwapPanePrev,
    ResizeLeft,
    ResizeRight,
    ResizeUp,
    ResizeDown,
    LayoutEvenH,
    LayoutEvenV,
    LayoutTiled,
    NewSession,
    AttachPicker,
    RenameSession,
    KillSession,
    RefreshSessions,
    DetachClient,
    ToggleSidebar,
}

/// The curated command set (design lock #15 — "the ~30 most-used tmux
/// actions"), fuzzy-searchable. Labels are what [`fuzzy_score`] matches.
const PALETTE_COMMANDS: [(&str, PaletteOp); 32] = [
    ("New window", PaletteOp::NewWindow),
    ("Kill window", PaletteOp::KillWindow),
    ("Rename window\u{2026}", PaletteOp::RenameWindow),
    ("Next window", PaletteOp::NextWindow),
    ("Previous window", PaletteOp::PrevWindow),
    ("Last window", PaletteOp::LastWindow),
    ("Move window left", PaletteOp::MoveWindowLeft),
    ("Move window right", PaletteOp::MoveWindowRight),
    ("Split pane right", PaletteOp::SplitRight),
    ("Split pane down", PaletteOp::SplitDown),
    ("Close pane", PaletteOp::ClosePane),
    ("Zoom pane", PaletteOp::ZoomPane),
    ("Break pane to window", PaletteOp::BreakPane),
    ("Next pane", PaletteOp::NextPane),
    ("Previous pane", PaletteOp::PrevPane),
    ("Rename pane title\u{2026}", PaletteOp::RenamePaneTitle),
    ("Swap pane with next", PaletteOp::SwapPaneNext),
    ("Swap pane with previous", PaletteOp::SwapPanePrev),
    ("Resize pane left", PaletteOp::ResizeLeft),
    ("Resize pane right", PaletteOp::ResizeRight),
    ("Resize pane up", PaletteOp::ResizeUp),
    ("Resize pane down", PaletteOp::ResizeDown),
    ("Layout: even horizontal", PaletteOp::LayoutEvenH),
    ("Layout: even vertical", PaletteOp::LayoutEvenV),
    ("Layout: tiled", PaletteOp::LayoutTiled),
    ("New session\u{2026}", PaletteOp::NewSession),
    ("Attach session\u{2026}", PaletteOp::AttachPicker),
    ("Rename session\u{2026}", PaletteOp::RenameSession),
    ("Kill session", PaletteOp::KillSession),
    ("Refresh session list", PaletteOp::RefreshSessions),
    ("Detach client", PaletteOp::DetachClient),
    ("Toggle sidebar tree", PaletteOp::ToggleSidebar),
];

/// Case-insensitive fuzzy subsequence score: every `query` character (spaces
/// skipped) must appear in `label` in order; **lower is better**. Gaps cost
/// their length and a mid-word jump costs extra, so a prefix beats a tail hit
/// and word-start matches ("kilw" → "Kill window") rank naturally. `None` = no
/// match; an empty query matches everything at 0 (the full curated list).
fn fuzzy_score(query: &str, label: &str) -> Option<u32> {
    let label: Vec<char> = label.to_lowercase().chars().collect();
    let mut score: u32 = 0;
    let mut pos: usize = 0;
    for qc in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let at = (pos..label.len()).find(|&j| label[j] == qc)?;
        let gap = u32::try_from(at - pos).unwrap_or(u32::MAX);
        score = score.saturating_add(gap);
        let word_start = at == 0 || label[at - 1] == ' ';
        if !word_start && at != pos {
            score = score.saturating_add(2);
        }
        pos = at + 1;
    }
    Some(score)
}

/// The palette rows matching `query`, best score first (stable by table order
/// on ties). Pure — the filtering the render mounts, unit-tested headlessly.
fn palette_filter(query: &str) -> Vec<usize> {
    let mut scored: Vec<(u32, usize)> = PALETTE_COMMANDS
        .iter()
        .enumerate()
        .filter_map(|(i, (label, _))| fuzzy_score(query, label).map(|s| (s, i)))
        .collect();
    scored.sort_unstable();
    scored.into_iter().map(|(_, i)| i).collect()
}

/// Resolve a chosen palette op against the live model + chrome state: the
/// command ops become [`TmuxIntent`]s (targets = the current window's acting
/// pane, the same resolution every other affordance uses); the editor ops open
/// the matching inline editor (revealing the sidebar that hosts it); the
/// chrome ops toggle UI state. Honest nothing when there is no live target.
fn palette_resolve(op: PaletteOp, model: &TmuxModel, state: &mut ChromeUi) -> Vec<TmuxIntent> {
    let window = active_window(model);
    let pane = window.and_then(|w| active_pane_of(model, w));
    match op {
        PaletteOp::NewWindow => vec![TmuxIntent::NewWindow],
        PaletteOp::KillWindow => window.map_or_else(Vec::new, |w| vec![TmuxIntent::KillWindow(w)]),
        PaletteOp::RenameWindow => {
            if let Some(w) = window {
                let name = model.window(w).map_or("", TmuxWindow::name).to_owned();
                state.win_rename = Some((w, name));
            }
            Vec::new()
        }
        PaletteOp::NextWindow => vec![TmuxIntent::NextWindow],
        PaletteOp::PrevWindow => vec![TmuxIntent::PrevWindow],
        PaletteOp::LastWindow => vec![TmuxIntent::LastWindow],
        PaletteOp::MoveWindowLeft => window
            .and_then(|w| nudge_window_intent(model, w, true))
            .map_or_else(Vec::new, |i| vec![i]),
        PaletteOp::MoveWindowRight => window
            .and_then(|w| nudge_window_intent(model, w, false))
            .map_or_else(Vec::new, |i| vec![i]),
        PaletteOp::SplitRight => op_intents(model, TmuxMenuChoice::SplitRight),
        PaletteOp::SplitDown => op_intents(model, TmuxMenuChoice::SplitDown),
        PaletteOp::ClosePane => op_intents(model, TmuxMenuChoice::ClosePane),
        PaletteOp::ZoomPane => op_intents(model, TmuxMenuChoice::ZoomPane),
        PaletteOp::BreakPane => op_intents(model, TmuxMenuChoice::BreakPane),
        PaletteOp::NextPane => vec![TmuxIntent::NextPane],
        PaletteOp::PrevPane => vec![TmuxIntent::PrevPane],
        PaletteOp::RenamePaneTitle => {
            if let Some(p) = pane {
                let title = model.pane(p).map_or("", TmuxPane::title).to_owned();
                state.pane_rename = Some((p, title));
                // The pane-rename editor lives in the sidebar tree — reveal it.
                state.tree_open = true;
            }
            Vec::new()
        }
        PaletteOp::SwapPaneNext => {
            pane.map_or_else(Vec::new, |p| vec![TmuxIntent::SwapPaneNext(p)])
        }
        PaletteOp::SwapPanePrev => {
            pane.map_or_else(Vec::new, |p| vec![TmuxIntent::SwapPanePrev(p)])
        }
        PaletteOp::ResizeLeft => pane.map_or_else(Vec::new, |p| {
            vec![TmuxIntent::ResizePaneBy(p, ResizeDir::Left, RESIZE_STEP)]
        }),
        PaletteOp::ResizeRight => pane.map_or_else(Vec::new, |p| {
            vec![TmuxIntent::ResizePaneBy(p, ResizeDir::Right, RESIZE_STEP)]
        }),
        PaletteOp::ResizeUp => pane.map_or_else(Vec::new, |p| {
            vec![TmuxIntent::ResizePaneBy(p, ResizeDir::Up, RESIZE_STEP)]
        }),
        PaletteOp::ResizeDown => pane.map_or_else(Vec::new, |p| {
            vec![TmuxIntent::ResizePaneBy(p, ResizeDir::Down, RESIZE_STEP)]
        }),
        PaletteOp::LayoutEvenH => window.map_or_else(Vec::new, |w| {
            vec![TmuxIntent::SelectLayout(w, StockLayout::EvenHorizontal)]
        }),
        PaletteOp::LayoutEvenV => window.map_or_else(Vec::new, |w| {
            vec![TmuxIntent::SelectLayout(w, StockLayout::EvenVertical)]
        }),
        PaletteOp::LayoutTiled => window.map_or_else(Vec::new, |w| {
            vec![TmuxIntent::SelectLayout(w, StockLayout::Tiled)]
        }),
        PaletteOp::NewSession => {
            // The new-session name field lives in the sidebar — reveal both.
            state.new_open = true;
            state.tree_open = true;
            Vec::new()
        }
        PaletteOp::AttachPicker => {
            state.picker_open = true;
            vec![TmuxIntent::RefreshSessions]
        }
        PaletteOp::RenameSession => {
            let name = model
                .current_session()
                .and_then(|s| model.session(s))
                .map(TmuxSession::name);
            if let Some(name) = name {
                state.rename = Some(SessionRename {
                    target: name.to_owned(),
                    buffer: name.to_owned(),
                });
                state.tree_open = true;
            }
            Vec::new()
        }
        PaletteOp::KillSession => model
            .current_session()
            .and_then(|s| model.session(s))
            .map_or_else(Vec::new, |s| {
                vec![TmuxIntent::KillSession(s.name().to_owned())]
            }),
        PaletteOp::RefreshSessions => vec![TmuxIntent::RefreshSessions],
        PaletteOp::DetachClient => vec![TmuxIntent::Detach],
        PaletteOp::ToggleSidebar => {
            state.tree_open = !state.tree_open;
            Vec::new()
        }
    }
}

/// FC-4 — the palette overlay: a query field over the fuzzy-filtered curated
/// list. Arrows move the selection, Enter runs it, Esc closes, a click runs a
/// row — the navigation keys are consumed **before** the query field sees
/// them, so typing only ever edits the query.
fn render_palette(ui: &Ui, model: &TmuxModel, state: &mut ChromeUi, intents: &mut Vec<TmuxIntent>) {
    let mut open = state.palette.open;
    let mut chosen: Option<PaletteOp> = None;
    egui::Window::new("tmux commands")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_TOP, Vec2::new(0.0, Style::SP_XL * 2.0))
        .show(ui.ctx(), |ui| {
            let (up, down, enter, esc) = ui.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::ArrowUp),
                    i.consume_key(Modifiers::NONE, Key::ArrowDown),
                    i.consume_key(Modifiers::NONE, Key::Enter),
                    i.consume_key(Modifiers::NONE, Key::Escape),
                )
            });
            if esc {
                open = false;
            }

            let field = ui.text_edit_singleline(&mut state.palette.query);
            field.request_focus();
            if field.changed() {
                state.palette.sel = 0;
            }

            let filtered = palette_filter(&state.palette.query);
            if filtered.is_empty() {
                ui.label(
                    RichText::new("no matching command")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                return;
            }
            if down {
                state.palette.sel = (state.palette.sel + 1).min(filtered.len() - 1);
            }
            if up {
                state.palette.sel = state.palette.sel.saturating_sub(1);
            }
            state.palette.sel = state.palette.sel.min(filtered.len() - 1);
            if enter {
                chosen = filtered
                    .get(state.palette.sel)
                    .map(|&i| PALETTE_COMMANDS[i].1);
            }

            ScrollArea::vertical()
                .max_height(Style::SP_XL * 8.0)
                .show(ui, |ui| {
                    for (row, &idx) in filtered.iter().enumerate() {
                        let (label, op) = PALETTE_COMMANDS[idx];
                        let resp = ui.selectable_label(
                            row == state.palette.sel,
                            RichText::new(label).size(Style::SMALL),
                        );
                        if resp.clicked() {
                            chosen = Some(op);
                        }
                    }
                });
        });
    if let Some(op) = chosen {
        intents.extend(palette_resolve(op, model, state));
        open = false;
    }
    state.palette.open = open;
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

// ─────────────────────────────────────────────────────────────────────────────
// TMUX-FC-5 — the templates ("projects") window + editor.
// ─────────────────────────────────────────────────────────────────────────────

/// One action the templates window raises this frame (at most one).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TemplateAction {
    /// Open (build + attach) the template at this index.
    Open(usize),
    /// Delete the template at this index.
    Delete(usize),
    /// Open a fresh, empty editor.
    NewEditor,
    /// Open an editor pre-filled from the live session's structure.
    CaptureEditor,
    /// Close the window.
    Close,
}

/// The template editor's outcome this frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EditorAction {
    /// Save the edited template.
    Save,
    /// Discard the edit.
    Cancel,
}

/// Capture the live model's windows into editor rows — one [`WindowEdit`] per
/// linked window (its name, one blank pane per live pane, beside-split default).
/// Pure over the model, so the "Save current as template" capture is unit-tested
/// headlessly.
fn capture_windows(model: &TmuxModel) -> Vec<WindowEdit> {
    model
        .windows_in_order()
        .into_iter()
        .filter_map(|w| {
            let win = model.window(w)?;
            let name = if win.name().is_empty() {
                format!("win{w}")
            } else {
                win.name().to_owned()
            };
            let count = model.panes_of_window(w).len().max(1);
            Some(WindowEdit {
                name,
                panes: vec![PaneEdit::default(); count],
                split: SplitDir::V,
            })
        })
        .collect()
}

/// The templates ("projects") window: the saved list (each Open/Delete) plus the
/// "New" + "Save current" authoring entry points. Returns the one action raised.
fn render_templates(ui: &Ui, templates: &[crate::SessionTemplate]) -> Option<TemplateAction> {
    let mut action = None;
    egui::Window::new("tmux templates")
        .collapsible(false)
        .resizable(true)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ui.ctx(), |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Projects")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM)
                        .strong(),
                );
                if ui.button("New\u{2026}").clicked() {
                    action = Some(TemplateAction::NewEditor);
                }
                if ui
                    .button("Save current\u{2026}")
                    .on_hover_text("Capture the live session's layout as a new template")
                    .clicked()
                {
                    action = Some(TemplateAction::CaptureEditor);
                }
            });
            ui.add_space(Style::SP_XS);
            ui.separator();

            if templates.is_empty() {
                ui.add_space(Style::SP_XS);
                ui.label(
                    RichText::new("No saved templates yet")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
            }
            ScrollArea::vertical()
                .max_height(Style::SP_XL * 8.0)
                .show(ui, |ui| {
                    for (i, tpl) in templates.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(tpl.name.as_str())
                                    .size(Style::BODY)
                                    .color(Style::TEXT),
                            );
                            let wins = tpl.blueprint.windows.len();
                            let panes = tpl.blueprint.pane_count();
                            ui.label(
                                RichText::new(format!("{wins}w \u{00b7} {panes}p"))
                                    .size(Style::SMALL)
                                    .color(Style::TEXT_DIM),
                            );
                            if ui.button("Open").clicked() {
                                action = Some(TemplateAction::Open(i));
                            }
                            if ui.button("Delete").clicked() {
                                action = Some(TemplateAction::Delete(i));
                            }
                        });
                    }
                });

            ui.add_space(Style::SP_S);
            ui.separator();
            if ui.button("Close").clicked() {
                action = Some(TemplateAction::Close);
            }
        });
    action
}

/// The template editor window (TMUX-FC-5): the name, its windows (each a name, a
/// beside/stacked split toggle, and its pane command lines), and Save/Cancel.
/// Mutates `edit` in place; returns Save/Cancel when pressed.
fn render_template_editor(ui: &Ui, edit: &mut TemplateEdit) -> Option<EditorAction> {
    let mut action = None;
    let mut remove_window: Option<usize> = None;
    egui::Window::new("edit template")
        .collapsible(false)
        .resizable(true)
        .anchor(Align2::CENTER_CENTER, Vec2::new(Style::SP_XL * 4.0, 0.0))
        .show(ui.ctx(), |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Name")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                ui.text_edit_singleline(&mut edit.name);
            });
            ui.add_space(Style::SP_XS);
            ui.separator();

            let win_count = edit.windows.len();
            ScrollArea::vertical()
                .max_height(Style::SP_XL * 9.0)
                .show(ui, |ui| {
                    for (wi, window) in edit.windows.iter_mut().enumerate() {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!("Window {}", wi + 1))
                                        .size(Style::SMALL)
                                        .color(Style::ACCENT_TERMINALS),
                                );
                                ui.text_edit_singleline(&mut window.name);
                                if ui
                                    .selectable_label(window.split == SplitDir::V, "beside")
                                    .clicked()
                                {
                                    window.split = SplitDir::V;
                                }
                                if ui
                                    .selectable_label(window.split == SplitDir::H, "stacked")
                                    .clicked()
                                {
                                    window.split = SplitDir::H;
                                }
                                // Never leave a template with zero windows.
                                if win_count > 1 && ui.button("\u{00d7} win").clicked() {
                                    remove_window = Some(wi);
                                }
                            });
                            let mut remove_pane: Option<usize> = None;
                            let pane_count = window.panes.len();
                            for (pi, pane) in window.panes.iter_mut().enumerate() {
                                ui.horizontal(|ui| {
                                    ui.add_space(Style::SP_M);
                                    ui.label(
                                        RichText::new(format!("%{pi}"))
                                            .size(Style::SMALL)
                                            .color(Style::TEXT_DIM),
                                    );
                                    ui.add(
                                        egui::TextEdit::singleline(&mut pane.command)
                                            .hint_text("command (blank = shell)")
                                            .desired_width(Style::SP_XL * 6.0),
                                    );
                                    if pane_count > 1 && ui.button("\u{00d7}").clicked() {
                                        remove_pane = Some(pi);
                                    }
                                });
                            }
                            if let Some(pi) = remove_pane {
                                window.panes.remove(pi);
                            }
                            if ui.button("+ pane").clicked() {
                                window.panes.push(PaneEdit::default());
                            }
                        });
                    }
                });
            if let Some(wi) = remove_window {
                edit.windows.remove(wi);
            }
            if ui.button("+ window").clicked() {
                edit.windows.push(WindowEdit::default());
            }

            ui.add_space(Style::SP_S);
            ui.separator();
            ui.horizontal(|ui| {
                let can_save = !edit.name.trim().is_empty();
                if ui.add_enabled(can_save, Button::new("Save")).clicked() {
                    action = Some(EditorAction::Save);
                }
                if ui.button("Cancel").clicked() {
                    action = Some(EditorAction::Cancel);
                }
            });
        });
    action
}

// ─────────────────────────────────────────────────────────────────────────────
// TMUX-FC-6 — the mesh peer picker.
// ─────────────────────────────────────────────────────────────────────────────

/// One action the mesh peer picker raises this frame.
#[derive(Clone, PartialEq, Eq, Debug)]
enum MeshAction {
    /// Attach `tmux -CC` on this host (a roster peer or a typed one).
    Attach(String),
    /// Close the picker.
    Close,
}

/// The mesh peer picker (TMUX-FC-6): the reachable roster peers (a pick dials
/// `tmux -CC` on that node over the Bus broker) + a manual host field for a node
/// not on the roster + an honest empty/solo caption. The picked session is the
/// same chrome, driven over the mesh transport.
fn render_mesh_picker(
    ui: &Ui,
    roster: Option<&RosterSnapshot>,
    manual: &mut String,
) -> Option<MeshAction> {
    let mut action = None;
    egui::Window::new("tmux on a mesh node")
        .collapsible(false)
        .resizable(true)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ui.ctx(), |ui| {
            ui.label(
                RichText::new("Attach tmux on\u{2026}")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM)
                    .strong(),
            );
            ui.add_space(Style::SP_XS);

            let reachable: Vec<_> = roster
                .map(|snap| {
                    snap.peers
                        .iter()
                        .filter(|p| p.is_reachable())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if reachable.is_empty() {
                ui.label(
                    RichText::new("No mesh peers online")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
            }
            ScrollArea::vertical()
                .max_height(Style::SP_XL * 7.0)
                .show(ui, |ui| {
                    for peer in reachable {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(peer.display.as_str())
                                    .size(Style::BODY)
                                    .color(Style::TEXT),
                            );
                            ui.label(
                                RichText::new(peer.presence.label())
                                    .size(Style::SMALL)
                                    .color(Style::OK),
                            );
                            if ui.button("Attach").clicked() {
                                action = Some(MeshAction::Attach(peer.host.clone()));
                            }
                        });
                    }
                });

            ui.add_space(Style::SP_S);
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Host")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                let resp = ui.add(
                    egui::TextEdit::singleline(manual)
                        .hint_text("node name")
                        .desired_width(Style::SP_XL * 5.0),
                );
                let submit = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button("Attach\u{2026}").clicked() || submit) && !manual.trim().is_empty() {
                    action = Some(MeshAction::Attach(manual.trim().to_owned()));
                    manual.clear();
                }
            });

            ui.add_space(Style::SP_XS);
            if ui.button("Close").clicked() {
                action = Some(MeshAction::Close);
            }
        });
    action
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
            command_for(&TmuxIntent::JoinPane {
                src: 4,
                window: 2,
                dir: SplitDir::H
            })
            .as_deref(),
            Some("join-pane -v -s %4 -t @2")
        );
        // The FC-4 "beside" trigger — the -h path FC-3 noted as missing.
        assert_eq!(
            command_for(&TmuxIntent::JoinPane {
                src: 4,
                window: 2,
                dir: SplitDir::V
            })
            .as_deref(),
            Some("join-pane -h -s %4 -t @2")
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
        // Onto a different window's row: join (move) the pane there (stacked —
        // the explicit beside/-h choice lives in the FC-4 context menu).
        assert_eq!(
            pane_drop_intent(1, Some(0), &rows, pos2(50.0, 70.0)),
            Some(TmuxIntent::JoinPane {
                src: 1,
                window: 7,
                dir: SplitDir::H
            })
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

    // ── TMUX-FC-4: the intent → command map for the new chrome ops ───────────

    #[test]
    fn command_for_maps_each_fc4_op_to_its_tmux_line() {
        assert_eq!(
            command_for(&TmuxIntent::NextWindow).as_deref(),
            Some("next-window")
        );
        assert_eq!(
            command_for(&TmuxIntent::PrevWindow).as_deref(),
            Some("previous-window")
        );
        assert_eq!(
            command_for(&TmuxIntent::LastWindow).as_deref(),
            Some("last-window")
        );
        assert_eq!(
            command_for(&TmuxIntent::NextPane).as_deref(),
            Some("select-pane -t :.+")
        );
        assert_eq!(
            command_for(&TmuxIntent::PrevPane).as_deref(),
            Some("select-pane -t :.-")
        );
        assert_eq!(
            command_for(&TmuxIntent::SwapPaneNext(3)).as_deref(),
            Some("swap-pane -D -t %3")
        );
        assert_eq!(
            command_for(&TmuxIntent::SwapPanePrev(3)).as_deref(),
            Some("swap-pane -U -t %3")
        );
        assert_eq!(
            command_for(&TmuxIntent::ResizePaneBy(2, ResizeDir::Left, 5)).as_deref(),
            Some("resize-pane -t %2 -L 5")
        );
        assert_eq!(
            command_for(&TmuxIntent::SelectLayout(1, StockLayout::Tiled)).as_deref(),
            Some("select-layout -t @1 tiled")
        );
    }

    // ── TMUX-FC-4: the shared target resolution + the reorder nudge ──────────

    /// A two-window fixture: @0 "editor" (panes 1,2 — pane 2 active) is
    /// current; @1 "logs" sits beside it.
    fn two_window_model() -> TmuxModel {
        model_from(
            b"%session-changed $0 main\n\
              %window-add @0\n\
              %window-renamed @0 editor\n\
              %layout-change @0 f9d3,80x24,0,0{40x24,0,0,1,39x24,41,0,2}\n\
              %window-pane-changed @0 %2\n\
              %window-add @1\n\
              %window-renamed @1 logs\n\
              %session-window-changed $0 @0\n",
        )
    }

    #[test]
    fn op_intents_resolves_the_toolbar_affordances_against_the_model() {
        let model = two_window_model();
        // Every toolbar button's op resolves to its intent on the acting pane
        // (@0's active pane %2) / the current window — the emission map behind
        // each affordance.
        assert_eq!(
            op_intents(&model, TmuxMenuChoice::SplitRight),
            vec![TmuxIntent::SplitPane(2, SplitDir::V)]
        );
        assert_eq!(
            op_intents(&model, TmuxMenuChoice::SplitDown),
            vec![TmuxIntent::SplitPane(2, SplitDir::H)]
        );
        assert_eq!(
            op_intents(&model, TmuxMenuChoice::ZoomPane),
            vec![TmuxIntent::ZoomPane(2)]
        );
        assert_eq!(
            op_intents(&model, TmuxMenuChoice::BreakPane),
            vec![TmuxIntent::BreakPane(2)]
        );
        assert_eq!(
            op_intents(&model, TmuxMenuChoice::ClosePane),
            vec![TmuxIntent::ClosePane(2)]
        );
        assert_eq!(
            op_intents(&model, TmuxMenuChoice::NewWindow),
            vec![TmuxIntent::NewWindow]
        );
        assert_eq!(
            op_intents(&model, TmuxMenuChoice::KillWindow),
            vec![TmuxIntent::KillWindow(0)]
        );
        // An empty model resolves honestly to nothing (§7 — no fake target).
        assert!(op_intents(&TmuxModel::new(), TmuxMenuChoice::ZoomPane).is_empty());
    }

    #[test]
    fn nudge_window_intent_moves_within_the_strip_and_stops_at_the_edges() {
        let model = two_window_model();
        assert_eq!(
            nudge_window_intent(&model, 1, true),
            Some(TmuxIntent::MoveWindowBefore { src: 1, dst: 0 })
        );
        assert_eq!(
            nudge_window_intent(&model, 0, false),
            Some(TmuxIntent::MoveWindowAfter { src: 0, dst: 1 })
        );
        // The edges honestly refuse (no wrap surprises).
        assert_eq!(nudge_window_intent(&model, 0, true), None);
        assert_eq!(nudge_window_intent(&model, 1, false), None);
        // An unknown window resolves to nothing.
        assert_eq!(nudge_window_intent(&model, 9, true), None);
    }

    // ── TMUX-FC-4: the fuzzy palette ─────────────────────────────────────────

    #[test]
    fn fuzzy_score_matches_subsequences_and_ranks_prefixes_first() {
        // An exact prefix is a perfect (zero-cost) match.
        assert_eq!(fuzzy_score("zoom", "Zoom pane"), Some(0));
        // Word-start subsequences match cheaply ("kilw" → "Kill window").
        assert!(fuzzy_score("kilw", "Kill window").is_some());
        // No subsequence → no match.
        assert_eq!(fuzzy_score("kilw", "Kill session"), None);
        assert_eq!(fuzzy_score("xyz", "Zoom pane"), None);
        // The empty query matches everything at zero (lists the whole set).
        assert_eq!(fuzzy_score("", "anything"), Some(0));
        // A prefix hit ranks above a scattered hit of the same query.
        let prefix = fuzzy_score("split", "Split pane right").expect("prefix");
        let scattered = fuzzy_score("split", "Swap pane with next... lit").unwrap_or(u32::MAX);
        assert!(prefix < scattered, "{prefix} vs {scattered}");
    }

    #[test]
    fn palette_filter_narrows_the_curated_set_and_keeps_order_on_ties() {
        // Empty query: the whole curated set (~30, lock #15), table order.
        let all = palette_filter("");
        assert_eq!(all.len(), PALETTE_COMMANDS.len());
        assert!(
            (28..=34).contains(&PALETTE_COMMANDS.len()),
            "the curated set stays ~30 (got {})",
            PALETTE_COMMANDS.len()
        );
        assert_eq!(all[0], 0, "ties keep table order");
        // "split" narrows to exactly the two split rows, best first.
        let split = palette_filter("split");
        let labels: Vec<&str> = split.iter().map(|&i| PALETTE_COMMANDS[i].0).collect();
        assert!(labels.contains(&"Split pane right"), "{labels:?}");
        assert!(labels.contains(&"Split pane down"), "{labels:?}");
        assert_eq!(labels.len(), 2, "{labels:?}");
        // No match → an honest empty list.
        assert!(palette_filter("qqqqq").is_empty());
    }

    #[test]
    fn palette_resolve_emits_the_op_intents_against_the_model() {
        let model = two_window_model();
        let mut state = ChromeUi::default();
        // Command ops → the same intents every other affordance raises.
        assert_eq!(
            palette_resolve(PaletteOp::NextWindow, &model, &mut state),
            vec![TmuxIntent::NextWindow]
        );
        assert_eq!(
            palette_resolve(PaletteOp::SplitRight, &model, &mut state),
            vec![TmuxIntent::SplitPane(2, SplitDir::V)]
        );
        assert_eq!(
            palette_resolve(PaletteOp::SwapPaneNext, &model, &mut state),
            vec![TmuxIntent::SwapPaneNext(2)]
        );
        assert_eq!(
            palette_resolve(PaletteOp::ResizeLeft, &model, &mut state),
            vec![TmuxIntent::ResizePaneBy(2, ResizeDir::Left, RESIZE_STEP)]
        );
        assert_eq!(
            palette_resolve(PaletteOp::LayoutTiled, &model, &mut state),
            vec![TmuxIntent::SelectLayout(0, StockLayout::Tiled)]
        );
        assert_eq!(
            palette_resolve(PaletteOp::MoveWindowRight, &model, &mut state),
            vec![TmuxIntent::MoveWindowAfter { src: 0, dst: 1 }]
        );
        assert_eq!(
            palette_resolve(PaletteOp::KillSession, &model, &mut state),
            vec![TmuxIntent::KillSession("main".to_owned())]
        );
        // Editor ops open the inline editor (revealing its sidebar host) and
        // emit nothing — the rename itself round-trips on submit.
        assert!(palette_resolve(PaletteOp::RenameWindow, &model, &mut state).is_empty());
        assert_eq!(state.win_rename, Some((0, "editor".to_owned())));
        assert!(palette_resolve(PaletteOp::RenamePaneTitle, &model, &mut state).is_empty());
        assert_eq!(state.pane_rename.as_ref().map(|(p, _)| *p), Some(2));
        assert!(state.tree_open, "the pane-rename editor's host is revealed");
        // Chrome ops toggle UI state.
        state.tree_open = false;
        assert!(palette_resolve(PaletteOp::ToggleSidebar, &model, &mut state).is_empty());
        assert!(state.tree_open);
        assert_eq!(
            palette_resolve(PaletteOp::AttachPicker, &model, &mut state),
            vec![TmuxIntent::RefreshSessions]
        );
        assert!(state.picker_open);
    }

    // ── TMUX-FC-4: the toolbar + native status bar render headless ───────────

    #[test]
    fn toolbar_and_status_bar_render_headless() {
        let model = two_window_model();
        let mut state = ChromeUi::default();
        let mut intents: Vec<TmuxIntent> = Vec::new();
        let prims = headless(|ui| {
            toolbar(ui, &model, &mut state, &mut intents);
            status_bar(ui, &model, 0, &mut intents);
        });
        assert!(prims > 0, "the FC-4 chrome did not tessellate");
        // Rendering alone raises no intents — only real clicks do.
        assert!(intents.is_empty(), "no phantom intents: {intents:?}");
    }

    #[test]
    fn the_palette_overlay_renders_headless_when_open() {
        let model = two_window_model();
        let mut state = ChromeUi::default();
        state.open_palette();
        let mut intents: Vec<TmuxIntent> = Vec::new();
        let prims = headless(|ui| {
            render_palette(ui, &model, &mut state, &mut intents);
        });
        assert!(prims > 0, "the palette overlay did not tessellate");
        assert!(state.palette.open, "no input → it stays open");
        assert!(intents.is_empty());
    }

    // ── TMUX-FC-4: the status clock folds ────────────────────────────────────

    #[test]
    fn the_status_clock_folds_read_correctly() {
        assert_eq!(hhmm(0), "00:00");
        assert_eq!(hhmm(3_661), "01:01");
        assert_eq!(hhmm(86_399), "23:59");
        assert_eq!(hhmm(86_400), "00:00");
        assert_eq!(secs_to_next_minute(0), 60);
        assert_eq!(secs_to_next_minute(59), 1);
        assert_eq!(secs_to_next_minute(61), 59);
    }

    // ── TMUX-FC-5: templates + persistence ───────────────────────────────────

    use crate::blueprint::{Blueprint, BlueprintPane, BlueprintWindow};
    use crate::tmux_store::{SessionTemplate, TmuxState, TmuxStateStore};

    fn temp_store() -> (tempfile::TempDir, TmuxStateStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tmux").join("state.json");
        let store = TmuxStateStore::with_path(Some(path));
        (dir, store)
    }

    #[test]
    fn saving_and_deleting_a_template_persists_through_the_store() {
        let (_dir, store) = temp_store();
        let mut chrome = TmuxChrome::with_store(store.clone(), TmuxState::default());
        let edit = TemplateEdit {
            name: "Dev".to_owned(),
            windows: vec![WindowEdit {
                name: "edit".to_owned(),
                panes: vec![
                    PaneEdit {
                        command: "vim".to_owned(),
                    },
                    PaneEdit::default(), // blank → a bare shell
                ],
                split: SplitDir::V,
            }],
        };
        chrome.save_template(&edit);
        assert_eq!(chrome.templates().len(), 1);
        // It persisted: a fresh store over the same path reads it back.
        assert_eq!(store.load().templates.len(), 1);
        let saved = &chrome.templates()[0];
        assert_eq!(saved.name, "Dev");
        assert_eq!(saved.blueprint.pane_count(), 2);

        // A same-name save replaces rather than duplicates.
        chrome.save_template(&edit);
        assert_eq!(chrome.templates().len(), 1);

        // A blank name is ignored (§7 — no nameless template).
        chrome.save_template(&TemplateEdit {
            name: "  ".to_owned(),
            windows: vec![WindowEdit::default()],
        });
        assert_eq!(chrome.templates().len(), 1);

        chrome.delete_template(0);
        assert!(chrome.templates().is_empty());
        assert!(store.load().templates.is_empty());
    }

    #[test]
    fn template_edit_converts_blank_commands_to_shells() {
        let edit = TemplateEdit {
            name: "  Mesh Ops  ".to_owned(),
            windows: vec![WindowEdit {
                name: "ops".to_owned(),
                panes: vec![
                    PaneEdit {
                        command: "meshctl status".to_owned(),
                    },
                    PaneEdit {
                        command: "   ".to_owned(),
                    },
                ],
                split: SplitDir::H,
            }],
        };
        let tpl = edit.to_template();
        assert_eq!(tpl.name, "Mesh Ops", "the name is trimmed");
        let win = &tpl.blueprint.windows[0];
        assert_eq!(win.panes[0], BlueprintPane::cmd("meshctl status"));
        assert_eq!(win.panes[1], BlueprintPane::shell(), "blank → shell");
        assert_eq!(win.split, SplitDir::H);
    }

    #[test]
    fn capture_windows_mirrors_the_live_session_windows() {
        // @0 "editor" has two panes; @1 "logs" has none yet (no layout streamed).
        let model = two_window_model();
        let caught = capture_windows(&model);
        assert_eq!(caught.len(), 2, "one WindowEdit per linked window");
        assert_eq!(caught[0].name, "editor");
        assert_eq!(caught[0].panes.len(), 2, "the two-pane window's pane count");
        assert_eq!(caught[1].name, "logs");
        assert_eq!(
            caught[1].panes.len(),
            1,
            "a paneless window still seeds one"
        );
        // Every captured pane starts blank (the user fills the commands).
        assert!(caught[0].panes.iter().all(|p| p.command.is_empty()));

        // Cold (no controller) → the editor falls back to a one-window skeleton.
        let store = TmuxStateStore::with_path(None);
        let chrome = TmuxChrome::with_store(store, TmuxState::default());
        assert_eq!(chrome.capture_template_edit().windows.len(), 1);
    }

    #[test]
    fn the_templates_window_and_editor_render_headless() {
        let templates = vec![SessionTemplate::new(
            "Dev",
            Blueprint::new(vec![BlueprintWindow::new(
                "edit",
                vec![BlueprintPane::shell()],
                SplitDir::V,
                None,
            )]),
        )];
        let prims = headless(|ui| {
            let _ = render_templates(ui, &templates);
        });
        assert!(prims > 0, "the templates window did not tessellate");

        let mut edit = TemplateEdit {
            name: "New".to_owned(),
            windows: vec![WindowEdit::default()],
        };
        let prims = headless(|ui| {
            let _ = render_template_editor(ui, &mut edit);
        });
        assert!(prims > 0, "the template editor did not tessellate");
    }

    #[test]
    fn a_chrome_with_no_remembered_session_stays_quiet_on_pump() {
        // No remembered session → pump attaches nothing (opt-in honesty holds).
        let store = TmuxStateStore::with_path(None);
        let mut chrome = TmuxChrome::with_store(store, TmuxState::default());
        chrome.pump();
        assert!(!chrome.is_active(), "nothing to reattach → no client");
        assert!(!chrome.tree_open(), "and the tree stays hidden");
    }

    #[test]
    fn opening_the_templates_window_via_the_menu_reveals_it() {
        let store = TmuxStateStore::with_path(None);
        let mut chrome = TmuxChrome::with_store(store, TmuxState::default());
        assert!(!chrome.templates_open());
        chrome.apply_menu(Some(TmuxMenuChoice::ShowTemplates));
        assert!(
            chrome.templates_open(),
            "the menu opens the templates window"
        );
    }

    // ── TMUX-FC-5: a guarded live reattach + template open (needs tmux) ───────

    fn tmux_available() -> bool {
        std::process::Command::new("tmux")
            .arg("-V")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn live_auto_reattach_re_enters_a_remembered_session() {
        if !tmux_available() {
            eprintln!("skipping: tmux is not installed");
            return;
        }
        // A remembered session name → the first pump re-enters it (opening the
        // tree). Unique per-process so it never collides with the user's tmux.
        let name = format!("mde-fc5-reattach-{}", std::process::id());
        let store = TmuxStateStore::with_path(None);
        let mut chrome = TmuxChrome::with_store(
            store,
            TmuxState {
                last_session: Some(name.clone()),
                templates: Vec::new(),
            },
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !chrome.is_active() {
            assert!(
                std::time::Instant::now() < deadline,
                "auto-reattach never attached the remembered session"
            );
            chrome.pump();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(chrome.tree_open(), "reattach reveals the tree");
        // Tear the recreated session down (reattach uses new-session -A).
        let _ = std::process::Command::new("tmux")
            .args(["kill-session", "-t", &name])
            .output();
    }

    #[test]
    fn live_open_template_builds_its_layout() {
        if !tmux_available() {
            eprintln!("skipping: tmux is not installed");
            return;
        }
        let name = format!("mde-fc5-tpl-{}", std::process::id());
        let store = TmuxStateStore::with_path(None);
        let tpl = SessionTemplate::new(
            name.clone(),
            Blueprint::new(vec![
                BlueprintWindow::new(
                    "ops",
                    vec![BlueprintPane::cmd("true"), BlueprintPane::cmd("true")],
                    SplitDir::V,
                    Some(StockLayout::EvenHorizontal),
                ),
                BlueprintWindow::new("shell", vec![BlueprintPane::shell()], SplitDir::H, None),
            ]),
        );
        let mut chrome = TmuxChrome::with_store(
            store,
            TmuxState {
                last_session: None,
                templates: vec![tpl],
            },
        );
        chrome.open_template(0);
        // The blueprint built two windows, the first (ops) split into two panes.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            chrome.pump();
            let ok = chrome.controller.as_ref().is_some_and(|c| {
                let m = c.model();
                m.windows_in_order().len() == 2
                    && m.windows_in_order()
                        .first()
                        .is_some_and(|w| m.panes_of_window(*w).len() == 2)
            });
            if ok {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the template's layout never built"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = std::process::Command::new("tmux")
            .args(["kill-session", "-t", &crate::session_safe(&name)])
            .output();
    }

    // ── TMUX-FC-6: the mesh peer picker + attach wiring ──────────────────────

    use crate::remote::test_support::FakeBus;
    use crate::roster::{PeerEntry, Presence};

    fn peer(host: &str, presence: Presence) -> PeerEntry {
        PeerEntry {
            host: host.to_owned(),
            display: host.to_owned(),
            presence,
        }
    }

    #[test]
    fn the_mesh_picker_renders_reachable_peers_and_the_empty_state() {
        let snap = RosterSnapshot {
            self_host: "here".to_owned(),
            peers: vec![
                peer("oak", Presence::Online),
                peer("cedar", Presence::Offline),
            ],
        };
        let mut manual = String::new();
        let prims = headless(|ui| {
            let _ = render_mesh_picker(ui, Some(&snap), &mut manual);
        });
        assert!(prims > 0, "the mesh picker did not tessellate");
        // With no roster it still renders (the manual-host + empty caption path).
        let prims = headless(|ui| {
            let _ = render_mesh_picker(ui, None, &mut manual);
        });
        assert!(prims > 0, "the empty mesh picker did not tessellate");
    }

    #[test]
    fn the_menu_opens_the_mesh_picker() {
        let store = TmuxStateStore::with_path(None);
        let mut chrome = TmuxChrome::with_store(store, TmuxState::default());
        assert!(!chrome.ui.mesh_open);
        chrome.apply_menu(Some(TmuxMenuChoice::ShowMesh));
        assert!(chrome.ui.mesh_open, "the menu opens the mesh peer picker");
    }

    #[test]
    fn attach_peer_dials_the_broker_and_drives_the_same_chrome() {
        // A mesh attach dials `tmux -CC` on the peer over the (fake) Bus broker and
        // makes the chrome active — the same controller/model a local session uses.
        let bus = FakeBus::new();
        let store = TmuxStateStore::with_path(None);
        let mut chrome = TmuxChrome::with_store(store, TmuxState::default());
        chrome.bus = Some(Arc::new(bus.clone()));
        chrome.attach_peer("oak");
        assert!(chrome.is_active(), "a mesh attach makes the chrome active");
        assert!(chrome.tree_open(), "and reveals the tree");
        // The worker opened the peer shell + exec'd control mode over the broker.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while bus.verb_count("open") == 0 {
            assert!(std::time::Instant::now() < deadline, "no open verb");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(bus.published().iter().all(|p| p.peer == "oak"));
        // A blank host is honestly a no-op (no second controller churn).
        chrome.attach_peer("   ");
        assert!(chrome.is_active());
        drop(chrome); // joins the mesh worker
    }
}
