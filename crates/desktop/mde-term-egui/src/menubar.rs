//! TERM-MENUBAR-1 — the **top menu bar** across the terminal surface (operator
//! directive; design lineage: `mde-editor-egui`'s Word-97 menu bar).
//!
//! The terminal already carries every Terminator-class feature behind chords +
//! the TERM-15 right-click menu; this is the *discoverable* face over them — an
//! `egui::menu::bar` of drop-downs, each item the **mouse twin of an existing
//! seam** (§6, one dispatch path), never a new behaviour and never a stub. Per
//! the editor menu's governing lock (**no dead entries** — an item ships only
//! when its seam exists, §7), an item whose feature is genuinely missing is
//! *omitted*, and an item whose feature needs context (Copy with no selection)
//! renders **disabled**, never a silent no-op.
//!
//! The menus and the seam each drives:
//!
//! * **File** — New Tab / New Remote Session… ([`TabCommand`]), Close Tab
//!   ([`TabbedTerminal::close_tab`]), Quit (`ViewportCommand::Close`).
//! * **Edit** — Copy ([`SplitTerminal::copy_focused`], greyed with no
//!   selection), Find… ([`SplitTerminal::toggle_search_focused`], TERM-9),
//!   Clear ([`SplitTerminal::clear_focused`], the `Ctrl+L` twin).
//! * **View** — Colour Scheme ([`TabbedTerminal::set_preset`], the TERM-11
//!   [`Preset`] palettes), Appearance… (the TERM-11 picker), Zoom In/Out/Reset
//!   ([`TabbedTerminal::zoom_in`] &c — the shared font-size knob).
//! * **Terminal** — New Session, Broadcast Input ([`SplitTerminal::set_broadcast`],
//!   the TERM-6 grouped input), Bell ([`SplitTerminal::set_bell_config_all`],
//!   TERM-12).
//! * **Splits** — Split H/V, Focus (Alt+arrows), Close / Zoom Pane, Layouts…
//!   (all [`Command`] / [`TabCommand`], TERM-4/10).
//! * **Tabs** — New / Close / Rename, Next / Prev, Move Left / Right (TERM-5/12).
//! * **Session** — the TERM-8 mesh roster: the reachable peers (attach opens a
//!   remote tab through [`TabbedTerminal::open_remote_tab`]) + the picker.
//! * **Help** — a keyboard-shortcuts reference read live from the rebindable
//!   [`Keymap`] (TERM-12).
//!
//! Each item's shortcut renders beside it; a keymap-bound action resolves its
//! **current** chord from the live [`Keymap`] (so a rebind is reflected), while
//! the fixed widget chords (Copy/Find/Clear) carry their literal hint.
//!
//! **Honestly omitted** (no landed seam, so no dead entry — the editor's Find
//! precedent): **Paste** (the pane consumes `Event::Paste` only while it holds
//! egui keyboard focus, which a menu click surrenders — a menu Paste would
//! silently drop; `Ctrl+Shift+V` / middle-click remain the paste paths),
//! **Select All** (there is no whole-buffer selection seam), **Reset** (the VT
//! engine is fed only by the PTY reader thread — no external reset seam; a
//! shell-stdin reset would be a fake), and the **status-bar toggle** (the
//! surface has no status bar). The **Tmux** menu (TMUX-FC-2) is wired at the
//! marked seam below; its items route OUT as a [`TmuxMenuChoice`] the surface
//! applies to its [`crate::tmux_ui::TmuxChrome`] (create · attach-picker ·
//! detach · toggle-tree), context-gated on a live control client.
//!
//! §4: the bar renders through the shared Carbon [`Style`] install — no forced
//! colours, so egui's disabled dimming reads correctly.

use std::cell::RefCell;

use mde_egui::egui::{self, Button, Context, RichText, Ui};
use mde_egui::Style;

use crate::bell::BellConfig;
use crate::keymap::{Action, Keymap};
use crate::picker::RemoteTarget;
use crate::presets::Preset;
use crate::splits::{Broadcast, Command, NavDir, SplitDir, SplitTerminal};
use crate::tabs::TabCommand;
use crate::tmux_ui::TmuxMenuChoice;
use crate::TabbedTerminal;

/// Minimum drop-down width so short menus don't collapse into slivers — six
/// shared spacing units (§4), the editor menu bar's derivation.
const MENU_MIN_W: f32 = Style::SP_XL * 6.0;

/// The bar's menu titles, left to right.
///
/// The **Tmux** menu (TMUX-FC-2) slots in before Session — its items route OUT to
/// the surface's [`crate::tmux_ui::TmuxChrome`] (which owns the optional live
/// control client the menu toggles), not into [`apply`] (which only touches the
/// [`TabbedTerminal`]).
pub const MENU_TITLES: [&str; 9] = [
    "File", "Edit", "View", "Terminal", "Splits", "Tabs", "Tmux", "Session", "Help",
];

// ─────────────────────────────── actions ────────────────────────────────────

/// The four bell styles as a menu-facing choice, mapped to a [`BellConfig`] at
/// dispatch (keeping [`MenuAction`] `Copy` + the bell module out of the action
/// vocabulary).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BellMode {
    /// A BEL is swallowed.
    Off,
    /// Flash the pane (the default).
    Visual,
    /// Raise an audible notice.
    Audible,
    /// Flash *and* notice.
    Both,
}

impl BellMode {
    /// The menu label.
    const fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Visual => "Visual",
            Self::Audible => "Audible",
            Self::Both => "Visual + Audible",
        }
    }

    /// The [`BellConfig`] this mode selects.
    const fn config(self) -> BellConfig {
        match self {
            Self::Off => BellConfig::off(),
            Self::Visual => BellConfig::visual_only(),
            Self::Audible => BellConfig::audible_only(),
            Self::Both => BellConfig::both(),
        }
    }

    /// The mode a live [`BellConfig`] reads as (the Bell submenu checkmark).
    const fn from_config(config: BellConfig) -> Self {
        match (config.visual, config.audible) {
            (false, false) => Self::Off,
            (true, false) => Self::Visual,
            (false, true) => Self::Audible,
            (true, true) => Self::Both,
        }
    }
}

/// One action a menu item dispatches — each routes to a real seam in [`apply`]
/// (§7, no dead entries). `Copy` so the static item tables can hold it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuAction {
    /// Open a fresh local tab (`TabCommand::New`).
    NewTab,
    /// Toggle the mesh "new terminal on → peer" picker (`TabCommand::ToggleRemote`).
    OpenRemotePicker,
    /// Close the active tab (`TabbedTerminal::close_tab`).
    CloseTab,
    /// Close the surface's viewport (the standalone window / the embed host).
    Quit,
    /// Copy the focused pane's selection (`SplitTerminal::copy_focused`).
    Copy,
    /// Toggle the focused pane's scrollback search (`toggle_search_focused`).
    Find,
    /// Clear the focused pane (`SplitTerminal::clear_focused`).
    Clear,
    /// Select a bundled colour scheme (`TabbedTerminal::set_preset`).
    SetPreset(Preset),
    /// Open the TERM-11 appearance picker (`TabCommand::ToggleAppearance`).
    OpenAppearance,
    /// Grow the surface font one step (`TabbedTerminal::zoom_in`).
    ZoomIn,
    /// Shrink the surface font one step (`TabbedTerminal::zoom_out`).
    ZoomOut,
    /// Reset the surface font to the default (`TabbedTerminal::zoom_reset`).
    ZoomReset,
    /// Set the broadcast-input routing (`SplitTerminal::set_broadcast`, TERM-6).
    SetBroadcast(Broadcast),
    /// Set every pane's bell style (`SplitTerminal::set_bell_config_all`).
    SetBell(BellMode),
    /// Split the focused pane (`Command::Split`).
    Split(SplitDir),
    /// Move focus to an adjacent pane (`Command::Focus`).
    Focus(NavDir),
    /// Close the focused pane (`Command::Close`).
    ClosePane,
    /// Maximize / restore the focused pane (`Command::ToggleZoom`).
    ZoomPane,
    /// Open the TERM-10 saved-layouts overlay (`TabCommand::ToggleLayouts`).
    OpenLayouts,
    /// Begin renaming the focused pane (`begin_rename_focused`, TERM-12).
    RenamePane,
    /// Activate the next tab (`TabCommand::Next`).
    NextTab,
    /// Activate the previous tab (`TabCommand::Prev`).
    PrevTab,
    /// Move the active tab one place left (`TabCommand::MoveLeft`).
    MoveTabLeft,
    /// Move the active tab one place right (`TabCommand::MoveRight`).
    MoveTabRight,
    /// Raise the keyboard-shortcuts reference (handled by [`MenuBar`]).
    ShowShortcuts,
}

/// Dispatch a [`MenuAction`] to its real seam.
///
/// `Copy` / `Find` / `Clear` route through the active tab's focused pane; the
/// surface-wide knobs (zoom, scheme, broadcast, bell) act on the tab.
/// [`MenuAction::ShowShortcuts`] is intercepted by [`MenuBar`] before this is
/// called, so it is a no-op here.
pub fn apply(action: MenuAction, term: &mut TabbedTerminal, ctx: &Context) {
    match action {
        MenuAction::NewTab => term.apply_tab(TabCommand::New),
        MenuAction::OpenRemotePicker => term.apply_tab(TabCommand::ToggleRemote),
        MenuAction::CloseTab => term.close_tab(term.active_index()),
        MenuAction::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        MenuAction::Copy => {
            if let Some(active) = term.active_mut() {
                active.copy_focused(ctx);
            }
        }
        MenuAction::Find => {
            if let Some(active) = term.active_mut() {
                active.toggle_search_focused();
            }
        }
        MenuAction::Clear => {
            if let Some(active) = term.active_mut() {
                active.clear_focused();
            }
        }
        MenuAction::SetPreset(preset) => term.set_preset(preset),
        MenuAction::OpenAppearance => term.apply_tab(TabCommand::ToggleAppearance),
        MenuAction::ZoomIn => term.zoom_in(),
        MenuAction::ZoomOut => term.zoom_out(),
        MenuAction::ZoomReset => term.zoom_reset(),
        MenuAction::SetBroadcast(mode) => {
            if let Some(active) = term.active_mut() {
                active.set_broadcast(mode);
            }
        }
        MenuAction::SetBell(mode) => {
            if let Some(active) = term.active_mut() {
                active.set_bell_config_all(mode.config());
            }
        }
        MenuAction::Split(dir) => {
            if let Some(active) = term.active_mut() {
                active.apply(Command::Split(dir));
            }
        }
        MenuAction::Focus(dir) => {
            if let Some(active) = term.active_mut() {
                active.apply(Command::Focus(dir));
            }
        }
        MenuAction::ClosePane => {
            if let Some(active) = term.active_mut() {
                active.apply(Command::Close);
            }
        }
        MenuAction::ZoomPane => {
            if let Some(active) = term.active_mut() {
                active.apply(Command::ToggleZoom);
            }
        }
        MenuAction::OpenLayouts => term.apply_tab(TabCommand::ToggleLayouts),
        MenuAction::RenamePane => {
            if let Some(active) = term.active_mut() {
                active.begin_rename_focused();
            }
        }
        MenuAction::NextTab => term.apply_tab(TabCommand::Next),
        MenuAction::PrevTab => term.apply_tab(TabCommand::Prev),
        MenuAction::MoveTabLeft => term.apply_tab(TabCommand::MoveLeft),
        MenuAction::MoveTabRight => term.apply_tab(TabCommand::MoveRight),
        // Owned by the stateful MenuBar (opens the reference window).
        MenuAction::ShowShortcuts => {}
    }
}

// ──────────────────────────────── gating ────────────────────────────────────

/// What must hold for an item to be **enabled** — context gating over seams that
/// all exist (§7 disable, not phasing).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Gate {
    /// Always enabled.
    Always,
    /// Needs a non-empty selection in the focused pane (Copy).
    HasSelection,
    /// Needs more than one pane in the active tab (Focus / Zoom Pane).
    MultiPane,
    /// Needs more than one tab (Next / Prev).
    MultiTab,
    /// The active tab is not already leftmost (Move Left).
    CanMoveLeft,
    /// The active tab is not already rightmost (Move Right).
    CanMoveRight,
}

impl Gate {
    /// Whether the gate passes under `cx`.
    #[must_use]
    pub const fn enabled(self, cx: &MenuContext) -> bool {
        match self {
            Self::Always => true,
            Self::HasSelection => cx.has_selection,
            Self::MultiPane => cx.pane_count > 1,
            Self::MultiTab => cx.tab_count > 1,
            Self::CanMoveLeft => cx.active_index > 0,
            Self::CanMoveRight => cx.active_index + 1 < cx.tab_count,
        }
    }
}

/// The per-frame surface-state snapshot the bar renders from (built by
/// [`context`]) — the bar never reaches into the surface mid-render, so its
/// gating + checkmarks are unit-testable without egui.
#[derive(Clone, Copy, Debug)]
pub struct MenuContext {
    /// Open tabs.
    pub tab_count: usize,
    /// The active tab's index.
    pub active_index: usize,
    /// Panes in the active tab.
    pub pane_count: usize,
    /// The focused pane holds a non-empty selection (Copy's gate).
    pub has_selection: bool,
    /// The focused pane's search overlay is open.
    pub is_searching: bool,
    /// The active tab's broadcast-input routing (the Broadcast checkmark).
    pub broadcast: Broadcast,
    /// The focused pane's bell mode (the Bell checkmark).
    pub bell: BellMode,
    /// The bundled colour scheme the surface matches, if any (Scheme checkmark).
    pub preset: Option<Preset>,
    /// The surface font size in points (informs Zoom Out's clamp readout).
    pub font_size: f32,
}

/// Snapshot `term` into a [`MenuContext`] (the read half of a render frame).
#[must_use]
pub fn context(term: &TabbedTerminal) -> MenuContext {
    let active = term.tab(term.active_index());
    MenuContext {
        tab_count: term.tab_count(),
        active_index: term.active_index(),
        pane_count: active.map_or(0, SplitTerminal::session_count),
        has_selection: active.is_some_and(SplitTerminal::focused_has_selection),
        is_searching: active.is_some_and(SplitTerminal::focused_is_searching),
        broadcast: active.map_or(Broadcast::Off, SplitTerminal::broadcast),
        bell: active
            .and_then(SplitTerminal::focused_bell_config)
            .map_or(BellMode::Visual, BellMode::from_config),
        preset: term.current_preset(),
        font_size: term.font_size(),
    }
}

/// The check-state for an item, or `None` for a plain command item.
fn checked(action: MenuAction, cx: &MenuContext) -> Option<bool> {
    match action {
        MenuAction::SetBroadcast(mode) => Some(cx.broadcast == mode),
        MenuAction::SetBell(mode) => Some(cx.bell == mode),
        MenuAction::SetPreset(preset) => Some(cx.preset == Some(preset)),
        _ => None,
    }
}

// ─────────────────────────── the static menu data ───────────────────────────

/// Where an item's shortcut hint comes from.
#[derive(Clone, Copy)]
enum Shortcut {
    /// No shortcut.
    None,
    /// A fixed widget chord (Copy/Find/Clear — not in the rebindable table).
    Fixed(&'static str),
    /// A rebindable [`Action`] — resolved to its *current* chord from the live
    /// [`Keymap`], so a user rebind is reflected in the menu.
    Bound(Action),
}

/// One menu item: label, action, shortcut source, enablement gate, and whether a
/// group separator precedes it.
struct Item {
    label: &'static str,
    action: MenuAction,
    shortcut: Shortcut,
    gate: Gate,
    sep_before: bool,
}

impl Item {
    const fn new(label: &'static str, action: MenuAction, shortcut: Shortcut, gate: Gate) -> Self {
        Self {
            label,
            action,
            shortcut,
            gate,
            sep_before: false,
        }
    }

    /// Same, with a Word-style group separator drawn above.
    const fn sep(mut self) -> Self {
        self.sep_before = true;
        self
    }
}

use Gate::{Always, CanMoveLeft, CanMoveRight, HasSelection, MultiPane, MultiTab};
use MenuAction as A;
use Shortcut::{Bound, Fixed, None as NoKey};

const FILE_ITEMS: [Item; 4] = [
    Item::new("New Tab", A::NewTab, Bound(Action::TabNew), Always),
    Item::new(
        "New Remote Session\u{2026}",
        A::OpenRemotePicker,
        Bound(Action::ToggleRemote),
        Always,
    ),
    Item::new("Close Tab", A::CloseTab, NoKey, Always).sep(),
    Item::new("Quit", A::Quit, NoKey, Always),
];

const EDIT_ITEMS: [Item; 3] = [
    Item::new("Copy", A::Copy, Fixed("Ctrl+Shift+C"), HasSelection),
    Item::new("Find\u{2026}", A::Find, Fixed("Ctrl+Shift+F"), Always),
    Item::new("Clear", A::Clear, Fixed("Ctrl+L"), Always).sep(),
];

/// The colour-scheme submenu — the TERM-11 [`Preset`] palettes as check items.
const SCHEME_ITEMS: [Item; 5] = [
    Item::new("Quasar", A::SetPreset(Preset::Quasar), NoKey, Always),
    Item::new(
        "Solarized Dark",
        A::SetPreset(Preset::SolarizedDark),
        NoKey,
        Always,
    ),
    Item::new(
        "Solarized Light",
        A::SetPreset(Preset::SolarizedLight),
        NoKey,
        Always,
    ),
    Item::new("Gruvbox", A::SetPreset(Preset::Gruvbox), NoKey, Always),
    Item::new("Nord", A::SetPreset(Preset::Nord), NoKey, Always),
];

/// The View menu below the Colour Scheme submenu.
const VIEW_ITEMS: [Item; 4] = [
    Item::new(
        "Appearance\u{2026}",
        A::OpenAppearance,
        Bound(Action::ToggleAppearance),
        Always,
    )
    .sep(),
    Item::new("Zoom In", A::ZoomIn, NoKey, Always).sep(),
    Item::new("Zoom Out", A::ZoomOut, NoKey, Always),
    Item::new("Reset Zoom", A::ZoomReset, NoKey, Always),
];

/// The Terminal menu's lead command (Broadcast + Bell are submenus).
const TERMINAL_ITEMS: [Item; 1] = [Item::new(
    "New Session",
    A::NewTab,
    Bound(Action::TabNew),
    Always,
)];

/// The Broadcast-Input submenu (the TERM-6 grouped input) as check items.
const BROADCAST_ITEMS: [Item; 3] = [
    Item::new("Off", A::SetBroadcast(Broadcast::Off), NoKey, Always),
    Item::new(
        "All Panes",
        A::SetBroadcast(Broadcast::All),
        Bound(Action::BroadcastAll),
        Always,
    ),
    Item::new(
        "Group",
        A::SetBroadcast(Broadcast::Group),
        Bound(Action::BroadcastGroup),
        Always,
    ),
];

/// The Bell submenu (TERM-12) as check items.
const BELL_ITEMS: [Item; 4] = [
    Item::new("Off", A::SetBell(BellMode::Off), NoKey, Always),
    Item::new("Visual", A::SetBell(BellMode::Visual), NoKey, Always),
    Item::new("Audible", A::SetBell(BellMode::Audible), NoKey, Always),
    Item::new(
        BellMode::Both.label(),
        A::SetBell(BellMode::Both),
        NoKey,
        Always,
    ),
];

const SPLITS_ITEMS: [Item; 9] = [
    Item::new(
        "Split Horizontal",
        A::Split(SplitDir::H),
        Bound(Action::SplitHorizontal),
        Always,
    ),
    Item::new(
        "Split Vertical",
        A::Split(SplitDir::V),
        Bound(Action::SplitVertical),
        Always,
    ),
    Item::new(
        "Focus Left",
        A::Focus(NavDir::Left),
        Bound(Action::FocusLeft),
        MultiPane,
    )
    .sep(),
    Item::new(
        "Focus Right",
        A::Focus(NavDir::Right),
        Bound(Action::FocusRight),
        MultiPane,
    ),
    Item::new(
        "Focus Up",
        A::Focus(NavDir::Up),
        Bound(Action::FocusUp),
        MultiPane,
    ),
    Item::new(
        "Focus Down",
        A::Focus(NavDir::Down),
        Bound(Action::FocusDown),
        MultiPane,
    ),
    Item::new("Close Pane", A::ClosePane, Bound(Action::ClosePane), Always).sep(),
    Item::new(
        "Zoom Pane",
        A::ZoomPane,
        Bound(Action::ToggleZoom),
        MultiPane,
    ),
    Item::new(
        "Layouts\u{2026}",
        A::OpenLayouts,
        Bound(Action::ToggleLayouts),
        Always,
    )
    .sep(),
];

const TABS_ITEMS: [Item; 7] = [
    Item::new("New Tab", A::NewTab, Bound(Action::TabNew), Always),
    Item::new("Close Tab", A::CloseTab, NoKey, Always),
    Item::new(
        "Rename Pane\u{2026}",
        A::RenamePane,
        Bound(Action::RenamePane),
        Always,
    ),
    Item::new("Next Tab", A::NextTab, Bound(Action::TabNext), MultiTab).sep(),
    Item::new("Previous Tab", A::PrevTab, Bound(Action::TabPrev), MultiTab),
    Item::new(
        "Move Left",
        A::MoveTabLeft,
        Bound(Action::TabMoveLeft),
        CanMoveLeft,
    )
    .sep(),
    Item::new(
        "Move Right",
        A::MoveTabRight,
        Bound(Action::TabMoveRight),
        CanMoveRight,
    ),
];

const HELP_ITEMS: [Item; 1] = [Item::new(
    "Keyboard Shortcuts\u{2026}",
    A::ShowShortcuts,
    NoKey,
    Always,
)];

// ───────────────────────────────── render ───────────────────────────────────

/// Resolve an item's shortcut hint against the live keymap.
fn shortcut_text(shortcut: Shortcut, keymap: &Keymap) -> String {
    match shortcut {
        Shortcut::None => String::new(),
        Shortcut::Fixed(s) => s.to_owned(),
        Shortcut::Bound(action) => keymap
            .binding_for(action)
            .map(|chord| chord.to_string())
            .unwrap_or_default(),
    }
}

/// Render one item as a button (checkable items carry a `✓` glyph + keep their
/// shortcut). Records the chosen action into `out`.
fn render_item(
    ui: &mut Ui,
    item: &Item,
    cx: &MenuContext,
    keymap: &Keymap,
    out: &RefCell<Option<Outcome>>,
) {
    if item.sep_before {
        ui.separator();
    }
    let enabled = item.gate.enabled(cx);
    let label = match checked(item.action, cx) {
        Some(true) => format!("\u{2713} {}", item.label),
        Some(false) => format!("    {}", item.label),
        None => item.label.to_owned(),
    };
    let clicked = ui
        .add_enabled(
            enabled,
            Button::new(label).shortcut_text(shortcut_text(item.shortcut, keymap)),
        )
        .clicked();
    if clicked {
        *out.borrow_mut() = Some(Outcome::Action(item.action));
        ui.close_menu();
    }
}

/// Render a flat list of items into a drop-down.
fn render_items(
    ui: &mut Ui,
    items: &[Item],
    cx: &MenuContext,
    keymap: &Keymap,
    out: &RefCell<Option<Outcome>>,
) {
    for item in items {
        render_item(ui, item, cx, keymap, out);
    }
}

/// A nested submenu of check/command items (Colour Scheme, Broadcast, Bell).
fn submenu(
    ui: &mut Ui,
    title: &str,
    items: &[Item],
    cx: &MenuContext,
    keymap: &Keymap,
    out: &RefCell<Option<Outcome>>,
) {
    ui.menu_button(title, |ui| {
        ui.set_min_width(MENU_MIN_W);
        render_items(ui, items, cx, keymap, out);
    });
}

/// What a rendered frame chose — a plain action or a Session peer to attach.
enum Outcome {
    /// A menu item's action.
    Action(MenuAction),
    /// Attach a session on this mesh peer (Session menu).
    AttachPeer(String),
}

/// The stateful top menu bar: it owns only the shortcuts-reference toggle; every
/// other bit of state lives in the [`TabbedTerminal`] it renders over.
#[derive(Default)]
pub struct MenuBar {
    /// Whether the keyboard-shortcuts reference window is open.
    shortcuts_open: bool,
}

impl MenuBar {
    /// A fresh bar (reference window closed).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Render the bar over `term` and apply the chosen action. Reads a snapshot
    /// of `term` up front (context + a keymap clone + the roster), renders every
    /// drop-down against it, then applies the one chosen action mutably — so no
    /// borrow of `term` is held across the render.
    ///
    /// `tmux_active` gates the Tmux menu's context-sensitive items (Detach / Hide
    /// tree need a live control client); a [`TmuxMenuChoice`] is returned rather
    /// than applied, since the surface — not the [`TabbedTerminal`] — owns the
    /// optional control client the menu drives.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        term: &mut TabbedTerminal,
        ctx: &Context,
        tmux_active: bool,
    ) -> Option<TmuxMenuChoice> {
        let cx = context(term);
        let keymap = term.keymap().clone();
        let roster = term.roster_snapshot();
        let out: RefCell<Option<Outcome>> = RefCell::new(None);
        let tmux_out: RefCell<Option<TmuxMenuChoice>> = RefCell::new(None);

        egui::menu::bar(ui, |ui| {
            ui.add_space(Style::SP_XS);
            ui.menu_button("File", |ui| {
                ui.set_min_width(MENU_MIN_W);
                render_items(ui, &FILE_ITEMS, &cx, &keymap, &out);
            });
            ui.menu_button("Edit", |ui| {
                ui.set_min_width(MENU_MIN_W);
                render_items(ui, &EDIT_ITEMS, &cx, &keymap, &out);
            });
            ui.menu_button("View", |ui| {
                ui.set_min_width(MENU_MIN_W);
                submenu(ui, "Colour Scheme", &SCHEME_ITEMS, &cx, &keymap, &out);
                render_items(ui, &VIEW_ITEMS, &cx, &keymap, &out);
            });
            ui.menu_button("Terminal", |ui| {
                ui.set_min_width(MENU_MIN_W);
                render_items(ui, &TERMINAL_ITEMS, &cx, &keymap, &out);
                ui.separator();
                submenu(ui, "Broadcast Input", &BROADCAST_ITEMS, &cx, &keymap, &out);
                submenu(ui, "Bell", &BELL_ITEMS, &cx, &keymap, &out);
            });
            ui.menu_button("Splits", |ui| {
                ui.set_min_width(MENU_MIN_W);
                render_items(ui, &SPLITS_ITEMS, &cx, &keymap, &out);
            });
            ui.menu_button("Tabs", |ui| {
                ui.set_min_width(MENU_MIN_W);
                render_items(ui, &TABS_ITEMS, &cx, &keymap, &out);
            });
            // ── TMUX-FC-2 seam ──────────────────────────────────────────────
            // The tmux session menu — its choices route OUT (to the surface's
            // TmuxChrome), not through `apply`; the session/window/pane tree +
            // ops live in the sidebar the "New tmux session" item reveals.
            ui.menu_button("Tmux", |ui| {
                ui.set_min_width(MENU_MIN_W);
                tmux_menu(ui, tmux_active, &tmux_out);
            });
            ui.menu_button("Session", |ui| {
                ui.set_min_width(MENU_MIN_W);
                session_menu(ui, roster.as_ref(), &cx, &keymap, &out);
            });
            ui.menu_button("Help", |ui| {
                ui.set_min_width(MENU_MIN_W);
                render_items(ui, &HELP_ITEMS, &cx, &keymap, &out);
            });
        });

        match out.into_inner() {
            Some(Outcome::Action(MenuAction::ShowShortcuts)) => self.shortcuts_open = true,
            Some(Outcome::Action(action)) => apply(action, term, ctx),
            Some(Outcome::AttachPeer(peer)) => term.open_remote_tab(&RemoteTarget {
                label: peer.clone(),
                peer,
            }),
            None => {}
        }

        self.shortcuts_window(ctx, term.keymap());
        tmux_out.into_inner()
    }

    /// The keyboard-shortcuts reference (Help → Keyboard Shortcuts), read live
    /// from the rebindable [`Keymap`] so it always shows the current bindings.
    fn shortcuts_window(&mut self, ctx: &Context, keymap: &Keymap) {
        if !self.shortcuts_open {
            return;
        }
        egui::Window::new("Keyboard Shortcuts")
            .open(&mut self.shortcuts_open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("Rebindable (TERM-12 keymap)")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                egui::Grid::new("term-shortcuts-bound")
                    .num_columns(2)
                    .spacing([Style::SP_L, Style::SP_XS])
                    .show(ui, |ui| {
                        for action in Action::all() {
                            ui.label(action_label(*action));
                            let chord = keymap
                                .binding_for(*action)
                                .map_or_else(|| "\u{2014}".to_owned(), |c| c.to_string());
                            ui.label(RichText::new(chord).color(Style::ACCENT));
                            ui.end_row();
                        }
                    });
                ui.add_space(Style::SP_S);
                ui.label(
                    RichText::new("Fixed")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                egui::Grid::new("term-shortcuts-fixed")
                    .num_columns(2)
                    .spacing([Style::SP_L, Style::SP_XS])
                    .show(ui, |ui| {
                        for (label, chord) in FIXED_SHORTCUTS {
                            ui.label(label);
                            ui.label(RichText::new(chord).color(Style::ACCENT));
                            ui.end_row();
                        }
                    });
            });
    }
}

/// The Session drop-down: the reachable mesh peers (attach opens a remote tab),
/// an honest empty state, and the picker.
fn session_menu(
    ui: &mut Ui,
    roster: Option<&crate::roster::RosterSnapshot>,
    cx: &MenuContext,
    keymap: &Keymap,
    out: &RefCell<Option<Outcome>>,
) {
    let reachable: Vec<&crate::roster::PeerEntry> = roster
        .map(|snap| snap.peers.iter().filter(|p| p.is_reachable()).collect())
        .unwrap_or_default();

    if reachable.is_empty() {
        ui.add_enabled(false, Button::new("No mesh peers online"));
    } else {
        ui.label(
            RichText::new("Attach a session on\u{2026}")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        for peer in reachable {
            if ui.button(peer.display.as_str()).clicked() {
                *out.borrow_mut() = Some(Outcome::AttachPeer(peer.host.clone()));
                ui.close_menu();
            }
        }
    }
    ui.separator();
    render_items(
        ui,
        std::slice::from_ref(&Item::new(
            "Open Session Picker\u{2026}",
            MenuAction::OpenRemotePicker,
            Shortcut::Bound(Action::ToggleRemote),
            Gate::Always,
        )),
        cx,
        keymap,
        out,
    );
}

/// The Tmux drop-down (TMUX-FC-2): the session-management entry points. Each
/// item routes OUT as a [`TmuxMenuChoice`] the surface applies to its
/// [`crate::tmux_ui::TmuxChrome`]; the context-sensitive ones honestly grey out
/// without a live control client (§7). The full session/window/pane tree + the
/// per-session ops live in the sidebar "New tmux session" reveals.
fn tmux_menu(ui: &mut Ui, active: bool, out: &RefCell<Option<TmuxMenuChoice>>) {
    let choose = |ui: &mut Ui, choice: TmuxMenuChoice| {
        *out.borrow_mut() = Some(choice);
        ui.close_menu();
    };
    if ui.button("New tmux session").clicked() {
        choose(ui, TmuxMenuChoice::NewSession);
    }
    if ui.button("Attach Session\u{2026}").clicked() {
        choose(ui, TmuxMenuChoice::ShowPicker);
    }
    ui.separator();
    if ui.add_enabled(active, Button::new("Detach")).clicked() {
        choose(ui, TmuxMenuChoice::Detach);
    }
    if ui
        .add_enabled(active, Button::new("Hide/Show Tree"))
        .clicked()
    {
        choose(ui, TmuxMenuChoice::ToggleTree);
    }
}

/// The fixed (non-rebindable) chords the terminal widget handles directly — the
/// reference window's second section, so every real chord is documented.
const FIXED_SHORTCUTS: [(&str, &str); 6] = [
    ("Copy selection", "Ctrl+Shift+C"),
    ("Paste", "Ctrl+Shift+V"),
    ("Find in scrollback", "Ctrl+Shift+F"),
    ("Clear screen", "Ctrl+L"),
    ("Page scrollback", "Shift+PgUp / PgDn"),
    ("Paste primary selection", "Middle-click"),
];

/// A human label for a rebindable [`Action`] (the reference window's left column).
const fn action_label(action: Action) -> &'static str {
    match action {
        Action::SplitHorizontal => "Split horizontal",
        Action::SplitVertical => "Split vertical",
        Action::ClosePane => "Close pane",
        Action::ToggleZoom => "Zoom pane",
        Action::FocusLeft => "Focus left",
        Action::FocusRight => "Focus right",
        Action::FocusUp => "Focus up",
        Action::FocusDown => "Focus down",
        Action::BroadcastAll => "Broadcast to all panes",
        Action::BroadcastGroup => "Broadcast to group",
        Action::TabNew => "New tab",
        Action::TabNext => "Next tab",
        Action::TabPrev => "Previous tab",
        Action::TabMoveLeft => "Move tab left",
        Action::TabMoveRight => "Move tab right",
        Action::ToggleRemote => "Remote session picker",
        Action::ToggleLayouts => "Saved layouts",
        Action::ToggleAppearance => "Appearance picker",
        Action::RenamePane => "Rename pane",
        Action::ToggleActivityWatch => "Watch for activity",
        Action::ToggleSilenceWatch => "Watch for silence",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::SpawnOptions;

    /// Every top-level menu, for the structural assertions.
    const MENUS: [(&str, &[Item]); 6] = [
        ("File", &FILE_ITEMS),
        ("Edit", &EDIT_ITEMS),
        ("Terminal", &TERMINAL_ITEMS),
        ("Splits", &SPLITS_ITEMS),
        ("Tabs", &TABS_ITEMS),
        ("Help", &HELP_ITEMS),
    ];

    fn live_term() -> TabbedTerminal {
        TabbedTerminal::new(SpawnOptions {
            // A plain `/bin/sh` keeps the PTY spawn cheap + deterministic.
            shell: Some("/bin/sh".to_owned()),
            ..SpawnOptions::default()
        })
        .expect("spawn a local shell for the menu-bar test")
    }

    // ── structure (§7 no dead entries) ───────────────────────────────────────

    #[test]
    fn every_menu_is_nonempty_and_labeled() {
        for (title, items) in MENUS {
            assert!(!items.is_empty(), "menu {title} shipped empty");
            for item in items {
                assert!(!item.label.is_empty(), "{title} has an unlabeled item");
            }
        }
        for (title, items) in [
            ("Colour Scheme", &SCHEME_ITEMS[..]),
            ("Broadcast", &BROADCAST_ITEMS[..]),
            ("Bell", &BELL_ITEMS[..]),
            ("View", &VIEW_ITEMS[..]),
        ] {
            assert!(!items.is_empty(), "submenu {title} shipped empty");
        }
    }

    #[test]
    fn omitted_features_have_no_dead_entry() {
        // No landed seam → no item (the editor's Find precedent): Paste, Select
        // All, Reset, and a status-bar toggle are all honestly absent.
        let all_labels: Vec<&str> = MENUS
            .iter()
            .flat_map(|(_, items)| items.iter())
            .chain(VIEW_ITEMS.iter())
            .chain(SCHEME_ITEMS.iter())
            .chain(BROADCAST_ITEMS.iter())
            .chain(BELL_ITEMS.iter())
            .map(|i| i.label)
            .collect();
        // Exact-label match — "Reset Zoom" is a real font item, only a bare
        // "Reset" (the omitted terminal reset) would be a dead entry.
        for banned in ["Paste", "Select All", "Reset", "Status Bar"] {
            assert!(
                !all_labels.contains(&banned),
                "{banned} shipped without a landed seam"
            );
        }
        // The tmux menu is now wired (TMUX-FC-2) — its items route out to the
        // surface's TmuxChrome, not through this crate's `apply`.
        assert!(
            MENU_TITLES.contains(&"Tmux"),
            "TMUX-FC-2 wires the tmux menu"
        );
    }

    #[test]
    fn menu_order_is_stable() {
        assert_eq!(
            MENU_TITLES,
            ["File", "Edit", "View", "Terminal", "Splits", "Tabs", "Tmux", "Session", "Help"]
        );
    }

    // ── a representative item per menu dispatches its real seam ──────────────

    #[test]
    fn file_new_tab_opens_a_tab() {
        let ctx = Context::default();
        let mut term = live_term();
        assert_eq!(term.tab_count(), 1);
        apply(MenuAction::NewTab, &mut term, &ctx);
        assert_eq!(term.tab_count(), 2, "File → New Tab drove TabCommand::New");
    }

    #[test]
    fn splits_split_adds_a_pane() {
        let ctx = Context::default();
        let mut term = live_term();
        let before = term.tab(term.active_index()).unwrap().session_count();
        apply(MenuAction::Split(SplitDir::V), &mut term, &ctx);
        assert_eq!(
            term.tab(term.active_index()).unwrap().session_count(),
            before + 1,
            "Splits → Split Vertical drove Command::Split"
        );
    }

    #[test]
    fn edit_find_toggles_the_search_overlay() {
        let ctx = Context::default();
        let mut term = live_term();
        assert!(!context(&term).is_searching);
        apply(MenuAction::Find, &mut term, &ctx);
        assert!(
            context(&term).is_searching,
            "Edit → Find opened the overlay"
        );
        apply(MenuAction::Find, &mut term, &ctx);
        assert!(!context(&term).is_searching, "a second Find closed it");
    }

    #[test]
    fn terminal_broadcast_and_bell_set_state() {
        let ctx = Context::default();
        let mut term = live_term();
        apply(MenuAction::SetBroadcast(Broadcast::All), &mut term, &ctx);
        assert_eq!(context(&term).broadcast, Broadcast::All);
        apply(MenuAction::SetBell(BellMode::Both), &mut term, &ctx);
        assert_eq!(context(&term).bell, BellMode::Both, "Bell reached the pane");
    }

    #[test]
    fn view_scheme_and_zoom_drive_the_appearance() {
        let ctx = Context::default();
        let mut term = live_term();
        apply(MenuAction::SetPreset(Preset::Nord), &mut term, &ctx);
        assert_eq!(term.current_preset(), Some(Preset::Nord));
        let base = term.font_size();
        apply(MenuAction::ZoomIn, &mut term, &ctx);
        assert!(term.font_size() > base, "Zoom In grew the font");
        apply(MenuAction::ZoomReset, &mut term, &ctx);
        assert!((term.font_size() - Style::BODY).abs() < f32::EPSILON);
    }

    #[test]
    fn tabs_next_wraps_the_active_index() {
        let ctx = Context::default();
        let mut term = live_term();
        apply(MenuAction::NewTab, &mut term, &ctx); // now 2 tabs, active = 1
        assert_eq!(term.active_index(), 1);
        apply(MenuAction::NextTab, &mut term, &ctx); // wraps to 0
        assert_eq!(term.active_index(), 0, "Tabs → Next wrapped");
    }

    // ── honest gating (§7) ───────────────────────────────────────────────────

    #[test]
    fn copy_is_disabled_without_a_selection() {
        let term = live_term();
        let cx = context(&term);
        assert!(!cx.has_selection, "a fresh pane has no selection");
        assert!(
            !Gate::HasSelection.enabled(&cx),
            "Copy greys out with nothing to copy, never a no-op"
        );
        // The same gate passes once a selection exists.
        let with_sel = MenuContext {
            has_selection: true,
            ..cx
        };
        assert!(Gate::HasSelection.enabled(&with_sel));
    }

    #[test]
    fn tab_and_pane_gates_track_the_counts() {
        let single = MenuContext {
            tab_count: 1,
            active_index: 0,
            pane_count: 1,
            has_selection: false,
            is_searching: false,
            broadcast: Broadcast::Off,
            bell: BellMode::Visual,
            preset: None,
            font_size: Style::BODY,
        };
        // One tab, one pane: multi-gated items are all disabled.
        assert!(!Gate::MultiTab.enabled(&single));
        assert!(!Gate::MultiPane.enabled(&single));
        assert!(!Gate::CanMoveLeft.enabled(&single));
        assert!(!Gate::CanMoveRight.enabled(&single));
        // A middle tab of three, split into two panes: everything opens up.
        let middle = MenuContext {
            tab_count: 3,
            active_index: 1,
            pane_count: 2,
            ..single
        };
        assert!(Gate::MultiTab.enabled(&middle));
        assert!(Gate::MultiPane.enabled(&middle));
        assert!(Gate::CanMoveLeft.enabled(&middle));
        assert!(Gate::CanMoveRight.enabled(&middle));
    }

    #[test]
    fn checkmarks_read_back_the_live_state() {
        let cx = MenuContext {
            tab_count: 1,
            active_index: 0,
            pane_count: 1,
            has_selection: false,
            is_searching: false,
            broadcast: Broadcast::All,
            bell: BellMode::Audible,
            preset: Some(Preset::Gruvbox),
            font_size: Style::BODY,
        };
        assert_eq!(
            checked(MenuAction::SetBroadcast(Broadcast::All), &cx),
            Some(true)
        );
        assert_eq!(
            checked(MenuAction::SetBroadcast(Broadcast::Off), &cx),
            Some(false)
        );
        assert_eq!(
            checked(MenuAction::SetBell(BellMode::Audible), &cx),
            Some(true)
        );
        assert_eq!(
            checked(MenuAction::SetPreset(Preset::Gruvbox), &cx),
            Some(true)
        );
        assert_eq!(
            checked(MenuAction::SetPreset(Preset::Nord), &cx),
            Some(false)
        );
        // A plain command item is never a checkbox.
        assert_eq!(checked(MenuAction::NewTab, &cx), None);
    }

    #[test]
    fn bell_mode_round_trips_through_its_config() {
        for mode in [
            BellMode::Off,
            BellMode::Visual,
            BellMode::Audible,
            BellMode::Both,
        ] {
            assert_eq!(BellMode::from_config(mode.config()), mode);
        }
    }

    // ── the bar renders headless (all menus) ─────────────────────────────────

    #[test]
    fn menu_bar_renders_headless() {
        use mde_egui::egui::{self, pos2, vec2, Rect};
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut term = live_term();
        let mut bar = MenuBar::new();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                // `tmux_active = false` — no live control client in the test.
                let _ = bar.ui(ui, &mut term, ctx, false);
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the menu bar produced no draw primitives"
        );
    }
}
