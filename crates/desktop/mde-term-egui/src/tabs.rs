//! Tabs (TERM-5) — each tab an independent split tree over TERM-4.
//!
//! Design lock Q6: "tabs, each holding its own split tree." A
//! [`TabbedTerminal`] owns a `Vec` of [`SplitTerminal`]s — one whole Terminator
//! split layout per tab — plus the active index. Every tab keeps its full tree,
//! its live PTYs, its focus and zoom state: switching tabs only re-points the
//! active index, so an inactive tab is never torn down (its reader threads keep
//! pumping their shells in the background) and comes back exactly as it was left.
//!
//! - **new** (`Ctrl+Shift+T` or the `+` button) spawns a fresh single-leaf
//!   [`SplitTerminal`] from the shared spawn recipe and focuses it; an honest
//!   spawn failure raises the error chip and adds no tab (§7 — never a fake tab);
//! - **close** (the per-tab `×`) drops that tab's [`SplitTerminal`], which
//!   SIGHUPs and reaps every pane in it (TERM-2's `Drop`); the last tab closing
//!   empties the surface and the binary closes the window — the same
//!   last-thing-closes lifecycle TERM-4 gives the last pane;
//! - **reorder** (drag a tab along the bar, or `Ctrl+Shift+PageUp`/`PageDown`)
//!   moves a tab in the strip; the active tab follows its own move;
//! - **switch** (`Ctrl+PageDown`/`PageUp`, or a click) changes the active index
//!   and nothing else, so per-tab state is preserved by construction.
//!
//! Within the active tab, TERM-4's Terminator chords (`Ctrl+Shift+O/E/W/X`,
//! `Alt+arrows`, divider drags, Alt-drag rearrange) work unchanged — the app
//! routes them to [`TabbedTerminal::active_mut`]. A pane self-exiting until its
//! tab is empty auto-closes that tab (the tab-level echo of TERM-4's
//! close-on-exit).
//!
//! §4: the tab bar chrome is pure `Style` tokens (active/inactive plate, the
//! bottom hairline, the accent underline, the `×`/`+` affordances) — no raw
//! colour. The terminal *content* palette stays [`crate::palette`]'s carve-out,
//! reached only through the panes.

use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mde_egui::egui::{
    pos2, vec2, Align2, Context, CursorIcon, FontId, Key, Modifiers, Rect, Sense, Stroke,
    StrokeKind, Ui, UiBuilder,
};
use mde_egui::Style;

use crate::appearance::{Appearance, AppearancePicker};
use crate::keymap::{Action, Keymap};
use crate::layout::SavedLayout;
use crate::layout_ui::{LayoutIntent, LayoutManager};
use crate::picker::{PickOutcome, ReattachTarget, RemotePicker, RemoteTarget};
use crate::presets::Preset;
use crate::pty::SpawnOptions;
use crate::remote::{BusPtyClient, PtyBus, RemotePty};
use crate::roster::{BusRoster, RosterClient, RosterSnapshot};
use crate::splits::SplitTerminal;
use crate::widget::chip;

/// Tab-strip height in points (the `+` button is a square of this side).
const TAB_BAR_H: f32 = Style::SP_XL;
/// Inner horizontal padding at each end of a tab plate.
const TAB_PAD: f32 = Style::SP_S;
/// The square reserved on a tab's right for its `×` close affordance.
const CLOSE_BOX: f32 = Style::SP_M;
/// A tab never renders narrower than this (short titles still read as tabs).
const TAB_MIN_W: f32 = Style::SP_XL * 2.0;
/// A tab never renders wider than this (long titles clip under the `×`).
const TAB_MAX_W: f32 = Style::SP_XL * 5.0;
/// The accent underline thickness on the active tab.
const UNDERLINE_PX: f32 = 2.0;
/// How long the new-tab spawn-failure chip stays up.
const ERROR_TTL: Duration = Duration::from_secs(6);

/// The View → Zoom font-size range + step (points), mirroring the appearance
/// picker's own knob so the menu and the picker agree — this is the shared size
/// knob, not a second profile system.
const FONT_ZOOM_MIN: f32 = 8.0;
/// See [`FONT_ZOOM_MIN`].
const FONT_ZOOM_MAX: f32 = 40.0;
/// See [`FONT_ZOOM_MIN`].
const FONT_ZOOM_STEP: f32 = 1.0;

/// A tab-strip command decoded from the keyboard by [`consume_tab_commands`]
/// and applied through [`TabbedTerminal::apply_tab`].
///
/// Terminator-compatible defaults (design lock Q15): new tab is `Ctrl+Shift+T`,
/// switch is `Ctrl+PageDown`/`PageUp`, move is `Ctrl+Shift+PageDown`/`PageUp`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TabCommand {
    /// Open a fresh tab and focus it (`Ctrl+Shift+T`).
    New,
    /// Activate the next tab, wrapping (`Ctrl+PageDown`).
    Next,
    /// Activate the previous tab, wrapping (`Ctrl+PageUp`).
    Prev,
    /// Move the active tab one place left (`Ctrl+Shift+PageUp`).
    MoveLeft,
    /// Move the active tab one place right (`Ctrl+Shift+PageDown`).
    MoveRight,
    /// Toggle the "new terminal on → <peer>" remote picker (`Ctrl+Shift+R`,
    /// TERM-8) — the keyboard twin of the tab-bar remote button.
    ToggleRemote,
    /// Toggle the saved-layouts overlay (`Ctrl+Shift+L`, TERM-10) — the keyboard
    /// twin of the tab-bar layouts button.
    ToggleLayouts,
    /// Toggle the appearance picker (`Ctrl+Shift+P`, TERM-11) — the keyboard twin
    /// of the tab-bar appearance button.
    ToggleAppearance,
}

/// Decode and **consume** this frame's tab-strip chords before any pane widget
/// clones the event stream — a consumed chord never reaches a shell.
///
/// egui's `consume_key` matches on modifier *subsets* — a `Ctrl` pattern also
/// matches a `Ctrl+Shift` event — so the more specific move chords
/// (`Ctrl+Shift+PageDown`/`PageUp`) are claimed **before** the switch chords
/// (`Ctrl+PageDown`/`PageUp`), or the switch would swallow the move. Plain
/// `Shift+PageUp`/`PageDown` carries no ctrl, so neither claims it and it stays
/// TERM-3's scrollback paging.
#[must_use]
pub fn consume_tab_commands(ctx: &Context) -> Vec<TabCommand> {
    ctx.input_mut(|input| {
        let mut cmds = Vec::new();
        let cs = Modifiers::CTRL | Modifiers::SHIFT;
        if input.consume_key(cs, Key::T) {
            cmds.push(TabCommand::New);
        }
        // Move (Ctrl+Shift) before switch (Ctrl): the Ctrl pattern would
        // otherwise also match the Ctrl+Shift event and steal the move.
        if input.consume_key(cs, Key::PageDown) {
            cmds.push(TabCommand::MoveRight);
        }
        if input.consume_key(cs, Key::PageUp) {
            cmds.push(TabCommand::MoveLeft);
        }
        if input.consume_key(Modifiers::CTRL, Key::PageDown) {
            cmds.push(TabCommand::Next);
        }
        if input.consume_key(Modifiers::CTRL, Key::PageUp) {
            cmds.push(TabCommand::Prev);
        }
        if input.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::R) {
            cmds.push(TabCommand::ToggleRemote);
        }
        if input.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::L) {
            cmds.push(TabCommand::ToggleLayouts);
        }
        if input.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::P) {
            cmds.push(TabCommand::ToggleAppearance);
        }
        cmds
    })
}

/// The remote-terminal subsystem (TERM-8): the Bus seam that opens broker
/// sessions, the roster source the picker reads, and the picker itself.
///
/// Bundled so a headless test injects fakes for the whole flow via
/// [`TabbedTerminal::with_remote_hub`].
pub struct RemoteHub {
    /// The Bus seam remote panes drive their broker verbs over.
    bus: Arc<dyn PtyBus>,
    /// The presence-roster source the picker reads.
    roster: Arc<dyn RosterClient>,
    /// The "new terminal on → <peer>" picker + manual host entry.
    picker: RemotePicker,
}

impl RemoteHub {
    /// The production hub — the live Bus + roster resolved from the environment.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            bus: Arc::new(BusPtyClient::from_env()),
            roster: Arc::new(BusRoster::from_env()),
            picker: RemotePicker::new(),
        }
    }

    /// Construct with explicit seams (tests inject fakes).
    #[must_use]
    pub fn with_clients(bus: Arc<dyn PtyBus>, roster: Arc<dyn RosterClient>) -> Self {
        Self {
            bus,
            roster,
            picker: RemotePicker::new(),
        }
    }

    /// Open a remote session on `target` at the given initial grid (the pane's
    /// first frame corrects the geometry).
    fn make_remote(&self, target: &RemoteTarget, cols: u16, rows: u16) -> RemotePty {
        RemotePty::open(
            Arc::clone(&self.bus),
            &target.peer,
            &target.label,
            cols,
            rows,
        )
    }

    /// TERM-14 — reattach a pane to a still-running brokered session (the reattach
    /// picker's pick), routing through [`RemotePty::reattach`] instead of `open`.
    fn make_reattach(&self, target: &ReattachTarget, cols: u16, rows: u16) -> RemotePty {
        RemotePty::reattach(
            Arc::clone(&self.bus),
            &target.peer,
            &target.label,
            &target.id,
            cols,
            rows,
        )
    }

    /// TERM-14 — the broker's reattachable-session index (drives the picker list).
    fn reattachable(&self) -> Vec<crate::remote::SessionSummary> {
        self.bus.list_sessions()
    }

    /// The latest presence roster (TERM-MENUBAR-1: the Session menu lists the
    /// reachable peers straight from it, alongside the picker).
    fn roster_snapshot(&self) -> Option<RosterSnapshot> {
        self.roster.snapshot()
    }
}

/// One tab: a whole split layout plus a stable strip label.
struct Tab {
    /// The tab's independent Terminator split tree + session registry.
    term: SplitTerminal,
    /// The strip label — a monotonic ordinal, stable across reorder/close so a
    /// tab can be followed by eye (editable/shell-driven titles are TERM-12).
    title: String,
}

/// A tab laid out on the strip this frame: its index and hit rects.
#[derive(Clone, Copy)]
struct TabSlot {
    /// Index into [`TabbedTerminal::tabs`].
    idx: usize,
    /// The whole tab plate (select + drag hit target).
    rect: Rect,
    /// The `×` close affordance within the plate.
    close: Rect,
}

/// The tabbed terminal: a stack of independent split layouts with one active.
///
/// See the module docs for the tab lifecycle and the chord map. The app mounts
/// exactly one of these; [`Self::show`] paints the strip and the active tab's
/// panes, and [`Self::is_empty`] tells the binary when to close the window.
pub struct TabbedTerminal {
    /// Every tab, left-to-right in strip order.
    tabs: Vec<Tab>,
    /// The active tab's index (always valid while `tabs` is non-empty).
    active: usize,
    /// The spawn recipe every new tab's first shell reuses.
    spawn_opts: SpawnOptions,
    /// Monotonic source of tab labels.
    next_no: usize,
    /// The tab being dragged along the strip, by its current index.
    drag: Option<usize>,
    /// The last new-tab spawn failure, chip-displayed until [`ERROR_TTL`].
    error: Option<(String, Instant)>,
    /// The remote-terminal subsystem (TERM-8): the broker seam + roster + picker.
    remote: RemoteHub,
    /// The saved-layouts overlay + its mesh-synced store (TERM-10).
    layouts: LayoutManager,
    /// The surface-wide appearance (TERM-11): scheme + font size + cursor style.
    appearance: Appearance,
    /// The appearance picker overlay that edits [`Self::appearance`] (TERM-11).
    appearance_picker: AppearancePicker,
    /// The rebindable keymap (TERM-12): the single action table every chord
    /// resolves through, driving both tab and split commands + the pane actions.
    keymap: Keymap,
}

impl TabbedTerminal {
    /// Open the surface with one tab holding one shell spawned from
    /// `spawn_opts` (the recipe every later tab/split reuses).
    ///
    /// # Errors
    ///
    /// The first shell's spawn failure — whatever the OS refused
    /// ([`SplitTerminal::new`]). Later new-tab failures surface as the strip's
    /// error chip instead, since a session is already running.
    pub fn new(spawn_opts: SpawnOptions) -> io::Result<Self> {
        Self::with_remote_hub(spawn_opts, RemoteHub::from_env())
    }

    /// Open the surface with an explicit [`RemoteHub`] (tests inject fake broker +
    /// roster seams; production uses [`Self::new`] → [`RemoteHub::from_env`]).
    ///
    /// # Errors
    /// The first local shell's spawn failure ([`SplitTerminal::new`]).
    pub fn with_remote_hub(spawn_opts: SpawnOptions, remote: RemoteHub) -> io::Result<Self> {
        let term = SplitTerminal::new(spawn_opts.clone())?;
        Ok(Self {
            tabs: vec![Tab {
                term,
                title: "1".to_owned(),
            }],
            active: 0,
            spawn_opts,
            next_no: 2,
            drag: None,
            error: None,
            remote,
            layouts: LayoutManager::local(),
            appearance: Appearance::default(),
            appearance_picker: AppearancePicker::new(),
            keymap: Keymap::default(),
        })
    }

    /// `true` once every tab has closed — the surface should close with it.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// The number of open tabs.
    #[must_use]
    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    /// The active tab's index (meaningless when [`Self::is_empty`]).
    #[must_use]
    pub const fn active_index(&self) -> usize {
        self.active
    }

    /// The active tab's split terminal, for routing TERM-4 chords into it.
    #[must_use]
    pub fn active_mut(&mut self) -> Option<&mut SplitTerminal> {
        self.tabs.get_mut(self.active).map(|t| &mut t.term)
    }

    /// The split terminal of tab `i`, if any.
    #[must_use]
    pub fn tab(&self, i: usize) -> Option<&SplitTerminal> {
        self.tabs.get(i).map(|t| &t.term)
    }

    /// Tab `i`'s strip label, if any.
    #[must_use]
    pub fn tab_title(&self, i: usize) -> Option<&str> {
        self.tabs.get(i).map(|t| t.title.as_str())
    }

    /// Apply one tab-strip [`TabCommand`].
    pub fn apply_tab(&mut self, cmd: TabCommand) {
        match cmd {
            TabCommand::New => self.new_tab(),
            TabCommand::Next => self.step_active(true),
            TabCommand::Prev => self.step_active(false),
            TabCommand::MoveLeft => {
                if self.active > 0 {
                    self.move_tab(self.active, self.active - 1);
                }
            }
            TabCommand::MoveRight => {
                if self.active + 1 < self.tabs.len() {
                    self.move_tab(self.active, self.active + 1);
                }
            }
            TabCommand::ToggleRemote => self.remote.picker.toggle(),
            TabCommand::ToggleLayouts => self.layouts.toggle(),
            TabCommand::ToggleAppearance => self.appearance_picker.toggle(),
        }
    }

    /// The rebindable keymap (TERM-12) — the shell reads it to render a settings
    /// pane and rebinds through [`Self::keymap_mut`].
    #[must_use]
    pub const fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    /// Mutable access to the keymap, for rebinding / applying a config override.
    pub const fn keymap_mut(&mut self) -> &mut Keymap {
        &mut self.keymap
    }

    // ── TERM-MENUBAR-1 seams ────────────────────────────────────────────────
    // Surface-wide knobs the top menu bar drives: font-size zoom and the colour
    // scheme both edit the shared [`Appearance`] the surface already pushes into
    // every pane each frame ([`Self::show`]), and the roster feeds the Session
    // menu. No new state — the menu is a discoverable face over TERM-11 + TERM-8.

    /// The surface content font size in points (the View → Zoom read-back).
    #[must_use]
    pub const fn font_size(&self) -> f32 {
        self.appearance.font_size
    }

    /// Set the surface font size, clamped to the appearance picker's own range
    /// (the shared knob, not a second profile system).
    const fn set_font_size(&mut self, size: f32) {
        self.appearance.font_size = size.clamp(FONT_ZOOM_MIN, FONT_ZOOM_MAX);
    }

    /// Grow the surface font one step (View → Zoom In).
    pub const fn zoom_in(&mut self) {
        self.set_font_size(self.appearance.font_size + FONT_ZOOM_STEP);
    }

    /// Shrink the surface font one step (View → Zoom Out).
    pub const fn zoom_out(&mut self) {
        self.set_font_size(self.appearance.font_size - FONT_ZOOM_STEP);
    }

    /// Reset the surface font to the platform default (View → Reset Zoom).
    pub const fn zoom_reset(&mut self) {
        self.set_font_size(Style::BODY);
    }

    /// Select a bundled colour scheme (View → Colour Scheme), driving the same
    /// [`Preset`] palettes the appearance picker offers (TERM-11).
    pub const fn set_preset(&mut self, preset: Preset) {
        self.appearance.palette = preset.palette();
    }

    /// The bundled preset the surface scheme currently matches, if any (the
    /// View → Colour Scheme checkmark).
    #[must_use]
    pub fn current_preset(&self) -> Option<Preset> {
        Preset::matching(&self.appearance.palette)
    }

    /// The latest mesh presence roster (the Session menu's peer list, TERM-8).
    #[must_use]
    pub fn roster_snapshot(&self) -> Option<RosterSnapshot> {
        self.remote.roster_snapshot()
    }

    /// Decode this frame's chords through the rebindable [`Keymap`] and apply
    /// each resolved [`Action`] — tab commands to the surface, split commands to
    /// the active tab, and the TERM-12 pane actions to the active tab's focused
    /// pane. Replaces the old hardcoded `consume_tab_commands` +
    /// `consume_commands` ladders in the binary's update loop (§6: the same
    /// `consume_key` decode, now table-driven).
    pub fn dispatch_keys(&mut self, ctx: &Context) {
        for action in self.keymap.consume(ctx) {
            if let Some(tab_cmd) = action.as_tab_command() {
                self.apply_tab(tab_cmd);
            } else if let Some(cmd) = action.as_command() {
                if let Some(active) = self.active_mut() {
                    active.apply(cmd);
                }
            } else if let Some(active) = self.active_mut() {
                // The TERM-12 pane actions have no legacy enum.
                match action {
                    Action::RenamePane => active.begin_rename_focused(),
                    Action::ToggleActivityWatch => active.toggle_activity_watch_focused(),
                    Action::ToggleSilenceWatch => active.toggle_silence_watch_focused(),
                    _ => {}
                }
            }
        }
    }

    /// Open a fresh tab whose first pane is a **remote** mesh shell on `target`
    /// (TERM-8), driven over the TERM-7 broker. The pane surfaces its own honest
    /// connecting / unreachable state — opening never fails here (§7). Splits
    /// within the tab reuse the local spawn recipe, as elsewhere.
    pub fn open_remote_tab(&mut self, target: &RemoteTarget) {
        let remote = self
            .remote
            .make_remote(target, self.spawn_opts.cols, self.spawn_opts.rows);
        let term = SplitTerminal::from_remote(remote, self.spawn_opts.clone());
        self.tabs.push(Tab {
            term,
            title: self.next_no.to_string(),
        });
        self.next_no += 1;
        self.active = self.tabs.len() - 1;
    }

    /// TERM-14 — open a fresh tab whose first pane **reattaches** to a still-running
    /// brokered session (from the reattach picker). The pane replays the session's
    /// buffered scrollback then streams the live PTY; it surfaces its own honest
    /// connecting/failed state, so reattaching never fails here (§7).
    pub fn open_reattach_tab(&mut self, target: &ReattachTarget) {
        let remote = self
            .remote
            .make_reattach(target, self.spawn_opts.cols, self.spawn_opts.rows);
        let term = SplitTerminal::from_remote(remote, self.spawn_opts.clone());
        self.tabs.push(Tab {
            term,
            title: self.next_no.to_string(),
        });
        self.next_no += 1;
        self.active = self.tabs.len() - 1;
    }

    // ── TERM-10: capture the whole surface into a saved layout, and launch one ──

    /// Capture the whole surface — every tab's split tree + per-pane relaunch spec
    /// — into a named [`SavedLayout`], stamped with the `origin` node. The pure
    /// projection the [`crate::layout::LayoutStore`] persists to the mesh-synced
    /// share; empty tabs (none, in practice) are skipped.
    #[must_use]
    pub fn capture_layout(
        &self,
        name: impl Into<String>,
        origin: impl Into<String>,
    ) -> SavedLayout {
        let tabs = self
            .tabs
            .iter()
            .filter_map(|t| t.term.capture_tab(t.title.clone()))
            .collect();
        SavedLayout {
            name: name.into(),
            origin: origin.into(),
            tabs,
            active: self.active,
        }
    }

    /// Launch a saved layout: **append** each of its tabs to the surface,
    /// rebuilding every pane — local shells respawned at their saved cwd +
    /// command, remote panes reconnected to their target node over the TERM-7
    /// broker — and focus the first appended tab. Appending (rather than replacing)
    /// keeps the current work intact, so a layout is a repeatable add-on anywhere.
    /// Returns the number of tabs added.
    ///
    /// # Errors
    /// The first local shell's spawn failure ([`SplitTerminal::from_layout`]);
    /// tabs opened before it stay open (the surface never half-closes).
    pub fn launch_layout(&mut self, layout: &SavedLayout) -> io::Result<usize> {
        let base = self.spawn_opts.clone();
        let cols = self.spawn_opts.cols;
        let rows = self.spawn_opts.rows;
        let first_new = self.tabs.len();
        for lt in &layout.tabs {
            let term = {
                // Disjoint field borrow: the remote hub (shared) mints each remote
                // pane; the tab vec is pushed after this block releases the borrow.
                let remote = &self.remote;
                let mut make_remote =
                    |target: &RemoteTarget| remote.make_remote(target, cols, rows);
                SplitTerminal::from_layout(lt, base.clone(), &mut make_remote)?
            };
            let title = self.next_no.to_string();
            self.tabs.push(Tab { term, title });
            self.next_no += 1;
        }
        let added = self.tabs.len() - first_new;
        if added > 0 {
            self.active = first_new;
        }
        Ok(added)
    }

    /// Render the remote picker overlay (when open) and act on a pick: open a fresh
    /// remote tab, or reattach a still-running session (TERM-14).
    fn show_remote_picker(&mut self, ctx: &Context) {
        // The reattachable-session index is read first (a shared &self borrow),
        // then handed to the picker alongside the roster.
        let sessions = self.remote.reattachable();
        // Disjoint field borrows: the picker (mut) reads the roster (shared).
        let outcome = {
            let hub = &mut self.remote;
            hub.picker.show(ctx, hub.roster.as_ref(), &sessions)
        };
        match outcome {
            Some(PickOutcome::New(target)) => self.open_remote_tab(&target),
            Some(PickOutcome::Reattach(target)) => self.open_reattach_tab(&target),
            None => {}
        }
    }

    /// Render the saved-layouts overlay (when open) and act on its intent: a save
    /// captures this whole surface and persists it (stamped with this node); a
    /// launch rebuilds the stored arrangement, appending its tabs.
    fn show_layout_overlay(&mut self, ctx: &Context) {
        match self.layouts.show(ctx) {
            Some(LayoutIntent::Save(name)) => {
                let origin = crate::layout::local_node();
                let layout = self.capture_layout(name, origin);
                self.layouts.persist(&layout);
            }
            Some(LayoutIntent::Launch(layout)) => {
                let added = self.launch_layout(&layout);
                self.layouts.note_launch(&layout.name, &added);
            }
            None => {}
        }
    }

    /// Open a fresh tab (a single-leaf split tree) and focus it. A spawn
    /// failure raises the error chip and leaves the tab set untouched (§7).
    pub fn new_tab(&mut self) {
        match SplitTerminal::new(self.spawn_opts.clone()) {
            Ok(term) => {
                self.tabs.push(Tab {
                    term,
                    title: self.next_no.to_string(),
                });
                self.next_no += 1;
                self.active = self.tabs.len() - 1;
            }
            Err(err) => {
                self.error = Some((format!("could not open a tab: {err}"), Instant::now()));
            }
        }
    }

    /// Close tab `i`: its split terminal drops (every pane SIGHUP'd + reaped),
    /// and the active index falls to a neighbouring tab.
    pub fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        drop(self.tabs.remove(i));
        if self.drag == Some(i) {
            self.drag = None;
        }
        if self.tabs.is_empty() {
            self.active = 0;
        } else if self.active > i || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
    }

    /// Activate tab `i` (no-op out of range). Nothing else changes, so the
    /// previously-active tab keeps its full state.
    pub fn select(&mut self, i: usize) {
        if i < self.tabs.len() {
            self.active = i;
        }
    }

    /// Step the active index one tab `forward` (else back), wrapping the strip.
    fn step_active(&mut self, forward: bool) {
        let n = self.tabs.len();
        if n == 0 {
            return;
        }
        self.active = if forward {
            (self.active + 1) % n
        } else {
            (self.active + n - 1) % n
        };
    }

    /// Move the tab at `from` to index `to`, keeping the active index pinned to
    /// the same logical tab through the shift.
    fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = reindex_after_move(self.active, from, to);
        if let Some(d) = self.drag {
            self.drag = Some(reindex_after_move(d, from, to));
        }
    }

    /// Render one frame: the tab strip, then the active tab's panes below it.
    /// An active tab emptied by its last pane closing (self-exit or explicit
    /// close) auto-closes here.
    pub fn show(&mut self, ui: &mut Ui) {
        if self.tabs.is_empty() {
            return;
        }
        let full = ui.available_rect_before_wrap();
        let bar = Rect::from_min_max(full.min, pos2(full.max.x, full.min.y + TAB_BAR_H));
        let body = Rect::from_min_max(pos2(full.min.x, bar.max.y), full.max);

        self.show_tab_bar(ui, bar);

        let appearance = self.appearance;
        if let Some(tab) = self.tabs.get_mut(self.active) {
            // Push the surface appearance into the active tab before it renders,
            // so a picker change reaches every visible pane (TERM-11).
            tab.term.set_appearance(appearance);
            let mut body_ui = ui.new_child(UiBuilder::new().max_rect(body).id_salt("term-body"));
            tab.term.show(&mut body_ui);
        }
        self.paint_error(ui, body);
        // The remote picker floats over the body (TERM-8); a pick opens a tab.
        self.show_remote_picker(ui.ctx());
        // The saved-layouts overlay floats over the body (TERM-10); save captures
        // this surface, launch rebuilds a stored one.
        self.show_layout_overlay(ui.ctx());
        // The appearance picker floats over the body (TERM-11); it edits the
        // surface scheme / font / cursor in place.
        self.appearance_picker.show(ui.ctx(), &mut self.appearance);

        // A tab whose last pane just closed empties its split terminal — close
        // the tab (the tab-level echo of TERM-4's last-pane lifecycle).
        if self
            .tabs
            .get(self.active)
            .is_some_and(|t| t.term.is_empty())
        {
            self.close_tab(self.active);
        }
    }

    /// The strip: the bar plate + hairline, each tab (active/hover/rest states),
    /// its `×`, and the trailing `+` new-tab button. All `Style` tokens (§4).
    fn show_tab_bar(&mut self, ui: &Ui, bar: Rect) {
        let painter = ui.painter();
        painter.rect_filled(bar, 0.0, Style::SURFACE);
        painter.rect_filled(
            Rect::from_min_max(pos2(bar.min.x, bar.max.y - 1.0), bar.max),
            0.0,
            Style::BORDER,
        );

        let new_rect = Rect::from_min_size(
            pos2(bar.max.x - TAB_BAR_H, bar.min.y),
            vec2(TAB_BAR_H, TAB_BAR_H),
        );
        // The remote-terminal button sits just left of the new-tab `+` (TERM-8),
        // the saved-layouts button just left of that (TERM-10), and the appearance
        // button just left of that (TERM-11).
        let remote_rect = Rect::from_min_size(
            pos2(new_rect.min.x - TAB_BAR_H, bar.min.y),
            vec2(TAB_BAR_H, TAB_BAR_H),
        );
        let layouts_rect = Rect::from_min_size(
            pos2(remote_rect.min.x - TAB_BAR_H, bar.min.y),
            vec2(TAB_BAR_H, TAB_BAR_H),
        );
        let appearance_rect = Rect::from_min_size(
            pos2(layouts_rect.min.x - TAB_BAR_H, bar.min.y),
            vec2(TAB_BAR_H, TAB_BAR_H),
        );
        let strip = Rect::from_min_max(bar.min, pos2(appearance_rect.min.x, bar.max.y));

        let slots = self.tab_slots(ui, strip);
        self.paint_tabs(ui, strip, &slots);
        if Self::paint_appearance_button(ui, appearance_rect, self.appearance_picker.is_open()) {
            self.appearance_picker.toggle();
        }
        if Self::paint_layouts_button(ui, layouts_rect, self.layouts.is_open()) {
            self.layouts.toggle();
        }
        if Self::paint_remote_button(ui, remote_rect, self.remote.picker.is_open()) {
            self.remote.picker.toggle();
        }
        Self::paint_new_button(ui, new_rect);

        // Interact + apply. Close wins its sub-rect (registered after the plate,
        // per egui's later-interact-claims-the-pointer rule).
        let mut to_close = None;
        for slot in &slots {
            let plate = ui.interact(
                slot.rect,
                ui.id().with(("term-tab", slot.idx)),
                Sense::click_and_drag(),
            );
            if plate.drag_started() {
                self.drag = Some(slot.idx);
                self.active = slot.idx;
            } else if plate.clicked() {
                self.active = slot.idx;
            }
            let close = ui
                .interact(
                    slot.close,
                    ui.id().with(("term-tab-x", slot.idx)),
                    Sense::click(),
                )
                .on_hover_cursor(CursorIcon::PointingHand);
            if close.clicked() {
                to_close = Some(slot.idx);
            }
        }
        self.drag_reorder(ui, &slots);
        if let Some(i) = to_close {
            self.close_tab(i);
        }
    }

    /// Lay the tabs out left-to-right within `strip`, content-sized and clamped.
    fn tab_slots(&self, ui: &Ui, strip: Rect) -> Vec<TabSlot> {
        let font = FontId::monospace(Style::SMALL);
        let mut slots = Vec::with_capacity(self.tabs.len());
        let mut x = strip.min.x;
        for (idx, tab) in self.tabs.iter().enumerate() {
            let title_w = ui
                .painter()
                .layout_no_wrap(tab.title.clone(), font.clone(), Style::TEXT)
                .size()
                .x;
            let w = (TAB_PAD + title_w + Style::SP_XS + CLOSE_BOX + TAB_PAD)
                .clamp(TAB_MIN_W, TAB_MAX_W);
            let rect = Rect::from_min_size(pos2(x, strip.min.y), vec2(w, strip.height()));
            let close = Rect::from_center_size(
                pos2(rect.max.x - TAB_PAD - CLOSE_BOX / 2.0, rect.center().y),
                vec2(CLOSE_BOX, CLOSE_BOX),
            );
            slots.push(TabSlot { idx, rect, close });
            x += w;
        }
        slots
    }

    /// Paint every tab plate, title and `×`, clipped to the strip.
    fn paint_tabs(&self, ui: &Ui, strip: Rect, slots: &[TabSlot]) {
        let clip = ui.painter().with_clip_rect(strip);
        let font = FontId::monospace(Style::SMALL);
        let hover = ui.input(|i| i.pointer.hover_pos());
        for slot in slots {
            let active = slot.idx == self.active;
            let hovered = hover.is_some_and(|p| slot.rect.contains(p));
            let fill = if active {
                Style::SURFACE_HI
            } else if hovered {
                Style::SURFACE
            } else {
                Style::BG
            };
            clip.rect_filled(slot.rect, 0.0, fill);
            if active {
                clip.rect_filled(
                    Rect::from_min_max(
                        pos2(slot.rect.min.x, slot.rect.max.y - UNDERLINE_PX),
                        slot.rect.max,
                    ),
                    0.0,
                    Style::ACCENT,
                );
            }
            // Right border hairline between tabs.
            clip.rect_filled(
                Rect::from_min_max(pos2(slot.rect.max.x - 1.0, slot.rect.min.y), slot.rect.max),
                0.0,
                Style::BORDER,
            );

            let title = self.tabs[slot.idx].title.clone();
            let text_color = if active { Style::TEXT } else { Style::TEXT_DIM };
            let text_rect = Rect::from_min_max(
                pos2(slot.rect.min.x + TAB_PAD, slot.rect.min.y),
                pos2(slot.close.min.x - Style::SP_XS, slot.rect.max.y),
            );
            clip.with_clip_rect(text_rect.intersect(strip)).text(
                pos2(text_rect.min.x, slot.rect.center().y),
                Align2::LEFT_CENTER,
                title,
                font.clone(),
                text_color,
            );

            let close_hot = hover.is_some_and(|p| slot.close.contains(p));
            clip.text(
                slot.close.center(),
                Align2::CENTER_CENTER,
                "\u{00d7}",
                font.clone(),
                if close_hot {
                    Style::DANGER
                } else {
                    Style::TEXT_DIM
                },
            );
        }
    }

    /// The trailing `+` button: a token plate with an accent-on-hover glyph.
    fn paint_new_button(ui: &Ui, rect: Rect) {
        let resp = ui
            .interact(rect, ui.id().with("term-new-tab"), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        let painter = ui.painter();
        if resp.hovered() {
            painter.rect_filled(rect, 0.0, Style::SURFACE_HI);
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0, Style::ACCENT),
                StrokeKind::Inside,
            );
        }
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "+",
            FontId::monospace(Style::BODY),
            if resp.hovered() {
                Style::ACCENT
            } else {
                Style::TEXT_DIM
            },
        );
    }

    /// The remote-terminal button (TERM-8): a token plate with a globe glyph that
    /// lights the accent when the picker is open or hovered. Returns whether it was
    /// clicked. All `Style` tokens (§4).
    fn paint_remote_button(ui: &Ui, rect: Rect, open: bool) -> bool {
        let resp = ui
            .interact(rect, ui.id().with("term-remote-btn"), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text("New terminal on a mesh node (Ctrl+Shift+R)");
        let painter = ui.painter();
        let hot = open || resp.hovered();
        if hot {
            painter.rect_filled(rect, 0.0, Style::SURFACE_HI);
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0, Style::ACCENT),
                StrokeKind::Inside,
            );
        }
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "\u{2325}",
            FontId::monospace(Style::BODY),
            if hot { Style::ACCENT } else { Style::TEXT_DIM },
        );
        resp.clicked()
    }

    /// The saved-layouts button (TERM-10): a token plate with a split-pane glyph
    /// that lights the accent when the overlay is open or hovered. Returns whether
    /// it was clicked. All `Style` tokens (§4).
    fn paint_layouts_button(ui: &Ui, rect: Rect, open: bool) -> bool {
        let resp = ui
            .interact(rect, ui.id().with("term-layouts-btn"), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text("Saved layouts (Ctrl+Shift+L)");
        let painter = ui.painter();
        let hot = open || resp.hovered();
        if hot {
            painter.rect_filled(rect, 0.0, Style::SURFACE_HI);
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0, Style::ACCENT),
                StrokeKind::Inside,
            );
        }
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "\u{25EB}",
            FontId::monospace(Style::BODY),
            if hot { Style::ACCENT } else { Style::TEXT_DIM },
        );
        resp.clicked()
    }

    /// The appearance button (TERM-11): a token plate with a half-disc glyph
    /// (light/dark theming) that lights the accent when the picker is open or
    /// hovered. Returns whether it was clicked. All `Style` tokens (§4).
    fn paint_appearance_button(ui: &Ui, rect: Rect, open: bool) -> bool {
        let resp = ui
            .interact(rect, ui.id().with("term-appearance-btn"), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text("Appearance \u{2014} palette + look (Ctrl+Shift+P)");
        let painter = ui.painter();
        let hot = open || resp.hovered();
        if hot {
            painter.rect_filled(rect, 0.0, Style::SURFACE_HI);
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0, Style::ACCENT),
                StrokeKind::Inside,
            );
        }
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "\u{25D0}",
            FontId::monospace(Style::BODY),
            if hot { Style::ACCENT } else { Style::TEXT_DIM },
        );
        resp.clicked()
    }

    /// While a tab is in flight, reorder it live as the pointer crosses slots.
    fn drag_reorder(&mut self, ui: &Ui, slots: &[TabSlot]) {
        let Some(cur) = self.drag else { return };
        let (pointer, down) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.any_down()));
        if !down {
            self.drag = None;
            return;
        }
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        if let Some(p) = pointer {
            let target = slots
                .iter()
                .find(|s| s.rect.contains(p))
                .map_or(cur, |s| s.idx);
            if target != cur {
                self.move_tab(cur, target);
            }
        }
    }

    /// The transient new-tab spawn-failure chip (§7 — an honest error, never a
    /// fake tab), centred over the body.
    fn paint_error(&mut self, ui: &Ui, body: Rect) {
        if let Some((msg, since)) = &self.error {
            if since.elapsed() < ERROR_TTL {
                chip(
                    ui.painter(),
                    pos2(body.center().x, body.min.y + Style::SP_S),
                    Align2::CENTER_TOP,
                    msg,
                    Style::DANGER,
                );
            } else {
                self.error = None;
            }
        }
    }
}

/// Where an index lands after the tab at `from` is removed and re-inserted at
/// `to`: the moved tab follows to `to`, and everything the shift crossed moves
/// one place toward the gap.
#[must_use]
const fn reindex_after_move(idx: usize, from: usize, to: usize) -> usize {
    if idx == from {
        to
    } else if from < idx && idx <= to {
        idx - 1
    } else if to <= idx && idx < from {
        idx + 1
    } else {
        idx
    }
}

#[cfg(test)]
mod tests {
    use mde_egui::egui::{self, Event, RawInput};

    use super::*;
    use crate::pty::SpawnOptions;
    use crate::remote::test_support::FakeBus;
    use crate::roster::test_support::FakeRoster;
    use crate::roster::Presence;

    // ── fixtures ────────────────────────────────────────────────────────────

    fn sh_opts() -> SpawnOptions {
        SpawnOptions {
            shell: Some("/bin/sh".to_owned()),
            ..SpawnOptions::default()
        }
    }

    fn tabs() -> TabbedTerminal {
        TabbedTerminal::new(sh_opts()).expect("first shell")
    }

    /// A surface with fake broker + roster seams (one online peer, `oak`).
    fn tabs_with_fakes() -> (TabbedTerminal, FakeBus) {
        let bus = FakeBus::new();
        let roster = FakeRoster::with_peers("eagle", &[("oak", Presence::Online)]);
        let hub = RemoteHub::with_clients(Arc::new(bus.clone()), Arc::new(roster));
        let term = TabbedTerminal::with_remote_hub(sh_opts(), hub).expect("first shell");
        (term, bus)
    }

    fn key_event(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// One headless frame over a 900×540 surface with the given events.
    fn frame(ctx: &Context, term: &mut TabbedTerminal, events: Vec<Event>) {
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_max(pos2(0.0, 0.0), pos2(900.0, 540.0))),
            events,
            ..RawInput::default()
        };
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(Style::BG))
                .show(ctx, |ui| term.show(ui));
        });
    }

    fn settle(ctx: &Context, term: &mut TabbedTerminal, frames: usize) {
        for _ in 0..frames {
            frame(ctx, term, Vec::new());
        }
    }

    // ── reindex helper (pure) ────────────────────────────────────────────────

    #[test]
    fn reindex_after_move_tracks_the_active_tab() {
        // Move tab 0 → 2 with 4 tabs.
        assert_eq!(reindex_after_move(0, 0, 2), 2, "the moved tab follows");
        assert_eq!(reindex_after_move(1, 0, 2), 0, "crossed left, shifts down");
        assert_eq!(reindex_after_move(2, 0, 2), 1, "crossed left, shifts down");
        assert_eq!(reindex_after_move(3, 0, 2), 3, "untouched beyond the gap");
        // Move tab 3 → 0.
        assert_eq!(reindex_after_move(3, 3, 0), 0);
        assert_eq!(reindex_after_move(1, 3, 0), 2, "crossed right, shifts up");
        assert_eq!(reindex_after_move(0, 3, 0), 1);
    }

    // ── new / independent trees ──────────────────────────────────────────────

    #[test]
    fn opens_with_one_tab_and_one_shell() {
        let term = tabs();
        assert_eq!(term.tab_count(), 1);
        assert_eq!(term.active_index(), 0);
        assert!(!term.is_empty());
        assert_eq!(
            term.tab(0).expect("tab 0").session_count(),
            1,
            "one live shell in the first tab"
        );
    }

    #[test]
    fn new_tab_appends_focuses_and_is_independent() {
        let mut term = tabs();
        // Grow tab 0 to two panes.
        term.active_mut()
            .expect("active")
            .apply(crate::Command::Split(crate::SplitDir::V));
        assert_eq!(term.tab(0).expect("tab 0").session_count(), 2);

        term.new_tab();
        assert_eq!(term.tab_count(), 2);
        assert_eq!(term.active_index(), 1, "the new tab takes focus");
        assert_eq!(
            term.tab(1).expect("tab 1").session_count(),
            1,
            "a fresh tab starts single-leaf"
        );
        assert_eq!(
            term.tab(0).expect("tab 0").session_count(),
            2,
            "the older tab is untouched — its own split tree"
        );
    }

    // ── switching preserves per-tab state ────────────────────────────────────

    #[test]
    fn switching_preserves_each_tabs_split_layout() {
        let ctx = Context::default();
        Style::install(&ctx);
        let mut term = tabs();

        // Tab 0: three panes.
        {
            let a = term.active_mut().expect("active");
            a.apply(crate::Command::Split(crate::SplitDir::V));
            a.apply(crate::Command::Split(crate::SplitDir::H));
        }
        assert_eq!(term.tab(0).expect("tab 0").session_count(), 3);

        // Tab 1: two panes.
        term.new_tab();
        term.active_mut()
            .expect("active")
            .apply(crate::Command::Split(crate::SplitDir::V));
        assert_eq!(term.tab(1).expect("tab 1").session_count(), 2);

        // Drive frames on tab 1, then switch back to tab 0.
        settle(&ctx, &mut term, 3);
        term.select(0);
        settle(&ctx, &mut term, 3);
        assert_eq!(term.active_index(), 0);
        assert_eq!(
            term.tab(0).expect("tab 0").session_count(),
            3,
            "tab 0's whole layout survived the switch away and back"
        );
        assert!(!term.tab(0).expect("tab 0").is_empty());

        // And tab 1 is still intact and live.
        term.select(1);
        settle(&ctx, &mut term, 1);
        assert_eq!(
            term.tab(1).expect("tab 1").session_count(),
            2,
            "the inactive tab was never torn down"
        );
    }

    // ── close / reindex / last-tab lifecycle ────────────────────────────────

    #[test]
    fn close_tab_drops_it_and_reindexes_active() {
        let mut term = tabs();
        term.new_tab();
        term.new_tab(); // three tabs "1","2","3", active = 2
        assert_eq!(term.tab_count(), 3);

        // Close the middle tab while it is not active.
        term.select(2);
        term.close_tab(0);
        assert_eq!(term.tab_count(), 2);
        assert_eq!(term.active_index(), 1, "active followed its shift down");
        assert_eq!(term.tab_title(0), Some("2"));
        assert_eq!(term.tab_title(1), Some("3"));
    }

    #[test]
    fn closing_the_last_tab_empties_the_surface() {
        let mut term = tabs();
        term.close_tab(0);
        assert!(term.is_empty(), "no tabs left → the window closes");
        assert_eq!(term.tab_count(), 0);
    }

    #[test]
    fn last_pane_of_last_tab_closes_the_surface_through_show() {
        let ctx = Context::default();
        Style::install(&ctx);
        let mut term = tabs();
        // Close the single pane of the single tab via a TERM-4 command.
        term.active_mut()
            .expect("active")
            .apply(crate::Command::Close);
        // The next frame sees the emptied tab and drops it → surface empty.
        settle(&ctx, &mut term, 1);
        assert!(term.is_empty(), "last pane → last tab → window closes");
    }

    #[test]
    fn emptying_a_middle_tab_closes_just_that_tab() {
        let ctx = Context::default();
        Style::install(&ctx);
        let mut term = tabs();
        term.new_tab(); // tabs "1","2"; active = 1
        term.active_mut()
            .expect("active")
            .apply(crate::Command::Close);
        settle(&ctx, &mut term, 1);
        assert_eq!(term.tab_count(), 1, "the emptied tab closed");
        assert!(!term.is_empty(), "the other tab keeps the surface open");
        assert_eq!(term.tab_title(0), Some("1"));
    }

    // ── reorder ──────────────────────────────────────────────────────────────

    #[test]
    fn move_tab_reorders_and_active_follows() {
        let mut term = tabs();
        term.new_tab();
        term.new_tab(); // "1","2","3"
        term.select(0); // active = the "1" tab

        term.apply_tab(TabCommand::MoveRight);
        assert_eq!(
            (term.tab_title(0), term.tab_title(1), term.tab_title(2)),
            (Some("2"), Some("1"), Some("3")),
        );
        assert_eq!(term.active_index(), 1, "active rode with the moved tab");

        term.apply_tab(TabCommand::MoveRight);
        assert_eq!(
            (term.tab_title(0), term.tab_title(1), term.tab_title(2)),
            (Some("2"), Some("3"), Some("1")),
        );
        assert_eq!(term.active_index(), 2);

        // Off the right edge is a no-op.
        term.apply_tab(TabCommand::MoveRight);
        assert_eq!(term.active_index(), 2);
        assert_eq!(term.tab_title(2), Some("1"));

        term.apply_tab(TabCommand::MoveLeft);
        assert_eq!(
            (term.tab_title(0), term.tab_title(1), term.tab_title(2)),
            (Some("2"), Some("1"), Some("3")),
        );
        assert_eq!(term.active_index(), 1);
    }

    // ── switch navigation ───────────────────────────────────────────────────

    #[test]
    fn next_prev_wrap_around_the_strip() {
        let mut term = tabs();
        term.new_tab();
        term.new_tab(); // active = 2 of three
        term.apply_tab(TabCommand::Next);
        assert_eq!(term.active_index(), 0, "next wraps past the end");
        term.apply_tab(TabCommand::Prev);
        assert_eq!(term.active_index(), 2, "prev wraps past the start");
        term.apply_tab(TabCommand::Prev);
        assert_eq!(term.active_index(), 1);
    }

    #[test]
    fn apply_tab_new_opens_and_focuses() {
        let mut term = tabs();
        term.apply_tab(TabCommand::New);
        assert_eq!(term.tab_count(), 2);
        assert_eq!(term.active_index(), 1);
    }

    // ── chords ───────────────────────────────────────────────────────────────

    #[test]
    fn tab_chords_are_consumed_and_decoded() {
        let cs = Modifiers::CTRL | Modifiers::SHIFT;
        let ctx = Context::default();
        let raw = RawInput {
            events: vec![
                key_event(Key::T, cs),
                key_event(Key::PageDown, Modifiers::CTRL),
                key_event(Key::PageUp, Modifiers::CTRL),
                key_event(Key::PageDown, cs),
                key_event(Key::PageUp, cs),
            ],
            ..RawInput::default()
        };
        let _ = ctx.run(raw, |ctx| {
            let cmds = consume_tab_commands(ctx);
            // Move (Ctrl+Shift) is decoded before switch (Ctrl) so the switch
            // pattern cannot swallow the more-specific move chord.
            assert_eq!(
                cmds,
                vec![
                    TabCommand::New,
                    TabCommand::MoveRight,
                    TabCommand::MoveLeft,
                    TabCommand::Next,
                    TabCommand::Prev,
                ]
            );
            // Everything claimed was consumed — nothing leaks to a shell.
            ctx.input(|i| assert!(i.events.is_empty(), "events left: {:?}", i.events));
        });

        // A bare Shift+PageUp (TERM-3 scrollback paging) is NOT a tab chord.
        let raw = RawInput {
            events: vec![key_event(Key::PageUp, Modifiers::SHIFT)],
            ..RawInput::default()
        };
        let _ = ctx.run(raw, |ctx| {
            assert!(consume_tab_commands(ctx).is_empty());
            ctx.input(|i| assert_eq!(i.events.len(), 1, "left for the widget"));
        });
    }

    // ── remote terminal (TERM-8) ────────────────────────────────────────────

    #[test]
    fn ctrl_shift_r_decodes_the_remote_toggle() {
        let ctx = Context::default();
        let raw = RawInput {
            events: vec![key_event(Key::R, Modifiers::CTRL | Modifiers::SHIFT)],
            ..RawInput::default()
        };
        let _ = ctx.run(raw, |ctx| {
            assert_eq!(consume_tab_commands(ctx), vec![TabCommand::ToggleRemote]);
        });
    }

    #[test]
    fn the_remote_toggle_opens_and_closes_the_picker() {
        let (mut term, _bus) = tabs_with_fakes();
        assert!(!term.remote.picker.is_open());
        term.apply_tab(TabCommand::ToggleRemote);
        assert!(term.remote.picker.is_open(), "the picker opened");
        term.apply_tab(TabCommand::ToggleRemote);
        assert!(!term.remote.picker.is_open(), "the picker closed");
    }

    #[test]
    fn opening_a_remote_tab_adds_a_focused_tab_and_publishes_open() {
        let ctx = Context::default();
        Style::install(&ctx);
        let (mut term, bus) = tabs_with_fakes();
        term.open_remote_tab(&RemoteTarget {
            peer: "oak".into(),
            label: "oak".into(),
        });
        // A second tab opened, focused, with one (remote) pane.
        assert_eq!(term.tab_count(), 2);
        assert_eq!(term.active_index(), 1);
        assert_eq!(term.tab(1).expect("remote tab").session_count(), 1);
        // The pane drove the broker: an `open` verb went to oak's topic slot (§7 —
        // a real caller of the TERM-7 contract, not a stub).
        assert_eq!(bus.verb_count("open"), 1);
        assert_eq!(bus.published()[0].peer, "oak");
        // The surface renders the remote pane without panicking.
        settle(&ctx, &mut term, 2);
        assert!(!term.is_empty());
    }

    #[test]
    fn opening_a_reattach_tab_adds_a_focused_tab_and_publishes_reattach() {
        let ctx = Context::default();
        Style::install(&ctx);
        let (mut term, bus) = tabs_with_fakes();
        // TERM-14: reattach to a still-running session (from the picker's index).
        term.open_reattach_tab(&ReattachTarget {
            id: "term-oak-persisted".into(),
            peer: "oak".into(),
            label: "oak".into(),
        });
        assert_eq!(term.tab_count(), 2);
        assert_eq!(term.active_index(), 1);
        assert_eq!(term.tab(1).expect("reattach tab").session_count(), 1);
        // The pane REATTACHED over the broker (a `reattach` verb, never `open`).
        assert_eq!(bus.verb_count("reattach"), 1);
        assert_eq!(
            bus.verb_count("open"),
            0,
            "reattach never opens a new shell"
        );
        assert_eq!(bus.published()[0].peer, "oak");
        settle(&ctx, &mut term, 2);
        assert!(!term.is_empty());
    }

    // ── saved layouts (TERM-10) ──────────────────────────────────────────────

    #[test]
    fn ctrl_shift_l_decodes_the_layouts_toggle() {
        let ctx = Context::default();
        let raw = RawInput {
            events: vec![key_event(Key::L, Modifiers::CTRL | Modifiers::SHIFT)],
            ..RawInput::default()
        };
        let _ = ctx.run(raw, |ctx| {
            assert_eq!(consume_tab_commands(ctx), vec![TabCommand::ToggleLayouts]);
        });
    }

    #[test]
    fn the_layouts_toggle_opens_and_closes_the_overlay() {
        let mut term = tabs();
        assert!(!term.layouts.is_open());
        term.apply_tab(TabCommand::ToggleLayouts);
        assert!(term.layouts.is_open(), "the overlay opened");
        term.apply_tab(TabCommand::ToggleLayouts);
        assert!(!term.layouts.is_open(), "the overlay closed");
    }

    // ── appearance picker (TERM-11) ──────────────────────────────────────────

    #[test]
    fn ctrl_shift_p_decodes_the_appearance_toggle() {
        let ctx = Context::default();
        let raw = RawInput {
            events: vec![key_event(Key::P, Modifiers::CTRL | Modifiers::SHIFT)],
            ..RawInput::default()
        };
        let _ = ctx.run(raw, |ctx| {
            assert_eq!(
                consume_tab_commands(ctx),
                vec![TabCommand::ToggleAppearance]
            );
        });
    }

    #[test]
    fn the_appearance_toggle_opens_and_closes_the_picker() {
        let mut term = tabs();
        assert!(!term.appearance_picker.is_open());
        term.apply_tab(TabCommand::ToggleAppearance);
        assert!(term.appearance_picker.is_open(), "the picker opened");
        term.apply_tab(TabCommand::ToggleAppearance);
        assert!(!term.appearance_picker.is_open(), "the picker closed");
    }

    #[test]
    fn the_surface_appearance_reaches_the_active_tabs_panes() {
        use crate::presets::Preset;

        // TERM-11 end to end: a scheme set on the surface propagates through the
        // active tab into its panes across a rendered frame.
        let ctx = Context::default();
        Style::install(&ctx);
        let mut term = tabs();
        term.appearance = Appearance {
            palette: Preset::SolarizedDark.palette(),
            ..Appearance::default()
        };
        settle(&ctx, &mut term, 1);
        let split = term.tab(0).expect("tab 0");
        let pane = split.focused_session();
        assert_eq!(
            split.pane_palette(pane),
            Some(Preset::SolarizedDark.palette()),
            "the active tab's pane adopted the surface scheme"
        );
    }

    #[test]
    fn capture_then_launch_folds_the_whole_arrangement() {
        use crate::splits::{Command, Pane, SplitDir};

        let mut term = tabs(); // one tab, one local shell
                               // Split the active tab so it holds two panes.
        term.active_mut()
            .expect("active tab")
            .apply(Command::Split(SplitDir::V));
        assert_eq!(term.tab(0).expect("tab 0").session_count(), 2);

        // Capture the surface into a layout.
        let layout = term.capture_layout("Two panes", "eagle");
        assert_eq!(layout.tabs.len(), 1);
        assert_eq!(layout.tabs[0].root.pane_count(), 2);
        assert_eq!(layout.origin, "eagle");

        // Launch it: a fresh tab is appended, rebuilt to the same shape.
        let added = term.launch_layout(&layout).expect("launch");
        assert_eq!(added, 1);
        assert_eq!(term.tab_count(), 2);
        let rebuilt = term.tab(1).expect("rebuilt tab");
        assert_eq!(rebuilt.session_count(), 2, "both panes came back");
        assert!(matches!(
            rebuilt.tree(),
            Some(Pane::Split {
                dir: SplitDir::V,
                ..
            })
        ));
    }

    #[test]
    fn a_captured_layout_records_local_recipe_and_remote_target() {
        use crate::layout::LayoutPane;

        let ctx = Context::default();
        Style::install(&ctx);
        let (mut term, _bus) = tabs_with_fakes();
        // Add a remote tab (on oak) beside the first local tab.
        term.open_remote_tab(&RemoteTarget {
            peer: "oak".into(),
            label: "oak".into(),
        });

        let layout = term.capture_layout("Mixed", "eagle");
        assert_eq!(layout.tabs.len(), 2);

        // Tab 0: the local shell — its cwd (live, from /proc) + the shell recipe.
        let LayoutPane::Leaf(local) = &layout.tabs[0].root else {
            panic!("tab 0 is a lone local pane");
        };
        assert!(!local.is_remote());
        assert_eq!(local.command.as_deref(), Some("/bin/sh"));
        assert!(local.cwd.is_some(), "captured the live cwd");

        // Tab 1: the remote pane — its target node, ready to reconnect.
        let LayoutPane::Leaf(remote) = &layout.tabs[1].root else {
            panic!("tab 1 is a lone remote pane");
        };
        assert!(remote.is_remote());
        assert_eq!(remote.target.as_ref().expect("target").peer, "oak");
    }

    #[test]
    fn launching_a_remote_layout_reconnects_to_its_target_node() {
        use crate::layout::{LayoutPane, LayoutTab, PaneSpec, SavedLayout};

        let (mut term, bus) = tabs_with_fakes();
        // A layout whose single pane is a remote shell on oak.
        let layout = SavedLayout {
            name: "On oak".into(),
            origin: "eagle".into(),
            tabs: vec![LayoutTab {
                title: "1".into(),
                root: LayoutPane::leaf(PaneSpec::remote(RemoteTarget {
                    peer: "oak".into(),
                    label: "oak".into(),
                })),
            }],
            active: 0,
        };

        let added = term.launch_layout(&layout).expect("launch");
        assert_eq!(added, 1);
        let rebuilt = term.tab(term.active_index()).expect("rebuilt tab");
        assert_eq!(rebuilt.session_count(), 1);

        // Rebuilding the remote pane drove the TERM-7 broker: an `open` verb to
        // oak's topic slot — the reconnect request, really constructed (§7).
        assert_eq!(bus.verb_count("open"), 1);
        assert_eq!(bus.published()[0].peer, "oak");
    }
}
