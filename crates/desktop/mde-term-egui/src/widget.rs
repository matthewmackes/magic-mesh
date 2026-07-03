//! The interactive terminal widget (TERM-3) — the engine's cell grid painted
//! through egui, with keyboard/mouse fed back to the PTY.
//!
//! [`TerminalWidget`] owns a [`LocalPty`] session and, each frame:
//!
//! - **sizes** the grid from the available rect ÷ the monospace cell metrics
//!   (a changed size propagates to the PTY + engine in one [`LocalPty::resize`]);
//! - **paints** the visible window: per-cell fg/bg through the content
//!   [`palette`] (the §4 carve-out), bold/italic/underline/strikeout/inverse/
//!   dim/hidden via the resolved colours + egui text format, the block cursor
//!   (blinking when focused, hollow when not), and the selection overlay;
//! - **feeds input**: printable text, editing keys and xterm escape sequences
//!   to the PTY; `Ctrl+Shift+C` copies the selection, paste events land as PTY
//!   input; the mouse wheel scrolls the scrollback window and any key input
//!   snaps back to live;
//! - **forwards the mouse** (TERM-13): when the running app enables mouse
//!   tracking, pointer clicks/drags/scroll/hover are encoded as SGR (1006)
//!   reports ([`crate::mouse`]) and fed to the PTY instead of driving local
//!   selection — with a **Shift-bypass** so Shift+drag always selects natively.
//!
//! **Batching:** the painter never lays out one galley per cell. Each row is
//! split into contiguous **same-style runs** (equal resolved fg/bg + the
//! format-bearing attrs); each run is one background rect + one galley placed
//! at its cell-quantised x. Trailing default-blank cells are trimmed first, so
//! an idle 200×60 grid tessellates only its real content — that keeps large
//! grids at a few dozen shapes per frame instead of ~12k.
//!
//! §4: all chrome here (background, cursor, selection overlay, the scrollback
//! and session-ended chips) is `Style` tokens; the only colour table lives in
//! [`palette`] with its documented carve-out.

use std::sync::Arc;
use std::time::Duration;

use mde_egui::egui::text::LayoutJob;
use mde_egui::egui::{
    self, Align2, Context, Event, EventFilter, FontId, Key, Modifiers, MouseWheelUnit, Pos2, Rect,
    Response, RichText, Sense, Stroke, StrokeKind, TextFormat, Ui, Vec2,
};
use mde_egui::Style;

use crate::appearance::{Appearance, CursorShape};
use crate::bell::{Bell, BellConfig};
use crate::engine::TermEvent;
use crate::menu::{BusChatClient, ChatBus, CommandRunner, ContextMenu, OsCommandRunner};
use crate::mouse::{encode_sgr, MouseButton, MouseEvent};
use crate::notify::{BusNotifyClient, NoticeLevel, NotifyBus, TermNotice};
use crate::palette::{self, Palette};
use crate::pty::LocalPty;
use crate::remote::RemotePty;
use crate::screen::{Cell, Screen};
use crate::search::Search;
use crate::session::Session;
use crate::smart::{self, BusLaunchClient, LaunchBus};
use crate::title::PaneTitle;
use crate::watch::{ActivityWatch, WatchEvent, WatchMode};

/// Repaint cadence while the session is live. PTY output arrives on the pump
/// thread with no egui waker, so the surface heartbeats at ~30 fps and stops
/// once the child exits.
const LIVE_REPAINT: Duration = Duration::from_millis(33);

/// Cursor blink half-period in seconds (the classic ~500 ms phase).
const BLINK_HALF_PERIOD: f64 = 0.5;

/// Height of the per-pane title strip (TERM-12) — a compact caption bar above
/// the grid (`SMALL` type padded on the 4px half-step).
const TITLE_STRIP_H: f32 = Style::SMALL + Style::SP_XS * 2.0;

/// Peak opacity (0–255) of the visual-bell flash overlay at the ring instant.
const BELL_FLASH_PEAK: f32 = 90.0;

/// A cell address in **absolute snapshot space**: `row` counts from the top of
/// the retained scrollback (row `history` is the first live viewport row).
/// Selections anchor here so they stay put while output scrolls the window.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct CellPos {
    row: usize,
    col: usize,
}

/// A mouse selection: the pressed `anchor` and the dragged `head`, unordered.
/// The selected range is the reading-order span between them (inclusive of
/// the head cell), like every terminal's stream selection.
#[derive(Clone, Copy, Debug)]
struct Selection {
    anchor: CellPos,
    head: CellPos,
}

impl Selection {
    /// The reading-order `(start, end)` bounds (both inclusive).
    fn bounds(&self) -> (CellPos, CellPos) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// The selected column span `[start, end)` on `row`, or `None` if the row
    /// is outside the selection.
    fn row_span(&self, row: usize, cols: usize) -> Option<(usize, usize)> {
        let (start, end) = self.bounds();
        if row < start.row || row > end.row {
            return None;
        }
        let a = if row == start.row { start.col } else { 0 };
        let b = if row == end.row {
            end.col.saturating_add(1).min(cols)
        } else {
            cols
        };
        (a < b).then_some((a, b))
    }
}

/// The smart-clipboard behaviour toggles (design lock Q12: "optional
/// copy-on-select + paste-on-middle-click").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ClipboardOptions {
    /// Copy a finished selection to the clipboard the moment the drag ends (no
    /// explicit `Ctrl+Shift+C` needed). Off by default — the conservative
    /// choice, so a stray drag never clobbers the clipboard.
    pub copy_on_select: bool,
    /// Middle-click pastes the last selection (the X11 PRIMARY convention,
    /// emulated in-process). On by default.
    pub paste_on_middle_click: bool,
}

impl Default for ClipboardOptions {
    fn default() -> Self {
        Self {
            copy_on_select: false,
            paste_on_middle_click: true,
        }
    }
}

/// A search match projected into the current window: `len` cells at `col` on
/// window-local `row`, `current` for the focused hit. Built each frame from
/// [`Search`] and painted through `Style` tokens.
struct SearchHit {
    row: usize,
    col: usize,
    len: usize,
    current: bool,
}

/// The search-overlay label + tone for this frame (`None` when search is off).
struct SearchBar {
    label: String,
    tone: egui::Color32,
}

/// How the cursor cell paints this frame. The *shape* it fills (block / bar /
/// underline) is [`PaintSpec::cursor_shape`]; this is only its visibility/focus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CursorPaint {
    /// Not drawn (scrolled into history, blink-off phase, or session ended).
    Hidden,
    /// The unfocused outline (a hollow block, whatever the shape).
    Hollow,
    /// The focused, filled cursor — painted in the configured shape, glyph
    /// repainted in the palette bg over a block.
    Filled,
}

/// Everything the paint pass needs besides the screen itself. Bundled so the
/// headless render tests drive the exact painter the live widget uses.
struct PaintSpec {
    font_id: FontId,
    cell: Vec2,
    /// The active content colour scheme (TERM-11): the grid fill + every cell's
    /// resolved colour flow through this, so a preset repaints the content.
    palette: Palette,
    /// The focused cursor's shape (block / bar / underline).
    cursor_shape: CursorShape,
    first_abs: usize,
    selection: Option<Selection>,
    cursor: CursorPaint,
    /// Lines currently scrolled back (paints the position chip when > 0).
    scrolled: usize,
    /// A node marker for a remote pane (`None` for a local one) — the pane is
    /// visually marked with the mesh node its shell runs on (TERM-8).
    node: Option<String>,
    /// An honest status chip (text + colour): a local "session ended", or a
    /// remote connecting / reconnecting / ended / failed note (§7).
    note: Option<(String, egui::Color32)>,
    /// Scrollback-search hits within this window (TERM-9), painted as a token
    /// underlay; the current hit reads brighter.
    search_hits: Vec<SearchHit>,
    /// The search-overlay chip (`None` when the overlay is closed).
    search_bar: Option<SearchBar>,
}

/// The interactive terminal pane: one [`Session`] (a local PTY shell or a remote
/// mesh shell) rendered as an egui widget. See the module docs for the frame
/// anatomy.
pub struct TerminalWidget {
    session: Session,
    font_size: f32,
    cursor_blink: bool,
    /// The focused cursor's shape (TERM-11 knob).
    cursor_shape: CursorShape,
    /// The active content colour scheme (TERM-11): Quasar default or a preset.
    palette: Palette,
    /// Lines scrolled back into history; `0` = live.
    scroll_offset: usize,
    /// Fractional wheel remainder (smooth trackpads scroll in sub-lines).
    scroll_accum: f32,
    selection: Option<Selection>,
    /// The mouse button currently held for SGR drag reporting (TERM-13), set on
    /// a reported press and cleared on its release, so motion reports carry the
    /// held button. `None` when no button is down (or reporting is off).
    mouse_report_button: Option<crate::mouse::MouseButton>,
    last_grid: Option<(u16, u16)>,
    /// This frame's locally-typed bytes, kept so the split multiplexer can fan
    /// them out to grouped panes (TERM-6 broadcast). Filled by [`Self::send`]
    /// as the pane types, drained by [`Self::take_input_echo`], and cleared at
    /// the top of every [`Self::show`] so it only ever holds one frame's input.
    input_echo: Vec<u8>,
    /// Scrollback search state (TERM-9): the overlay + query + match list.
    search: Search,
    /// The scrollback length at the last search rescan — a change (new output)
    /// re-triggers a rescan while the overlay is open.
    search_history: usize,
    /// A pending "scroll the current match into view" request (set on
    /// open/type/next/prev, consumed in [`Self::show`]).
    search_follow: bool,
    /// Smart-clipboard behaviour toggles (copy-on-select, middle-click paste).
    clip: ClipboardOptions,
    /// The last selection, kept as an in-process PRIMARY buffer that a
    /// middle-click pastes — X11's select-to-copy without a real X server.
    primary: Option<String>,
    /// The Bus seam a Ctrl-clicked URL/path is dispatched over (TERM-9): the
    /// mesh surface-launch path. Injectable so tests record it.
    launch_bus: Arc<dyn LaunchBus>,
    /// The pane's title (TERM-12): auto-derived from the running command's OSC
    /// title, user-overridable via rename, shown in the pane's chrome strip.
    title: PaneTitle,
    /// The activity/silence watcher (TERM-12): per-pane toggles that fire a
    /// notice through the [`Self::notify_bus`] seam on the matching output edge.
    watch: ActivityWatch,
    /// The configurable bell (TERM-12): visual flash and/or an audible notice on
    /// the terminal `BEL`.
    bell: Bell,
    /// The notification Bus seam (TERM-12) the watcher + audible bell publish
    /// over — a desktop toast. Injectable so tests record it.
    notify_bus: Arc<dyn NotifyBus>,
    /// The selection context menu (TERM-15): the user's custom commands + the
    /// Chat recipient. The four built-in mesh actions are always offered.
    menu: ContextMenu,
    /// The Bus seam send-selection-to-Chat publishes over (TERM-15) — the
    /// NOTIFY-CHAT `action/chat/send` verb. Injectable so tests record it.
    chat_bus: Arc<dyn ChatBus>,
    /// The dispatch seam a custom command's argv runs through (TERM-15) —
    /// production spawns it detached in the pane's cwd. Injectable so tests
    /// record it.
    runner: Arc<dyn CommandRunner>,
    /// Set when the context menu's **new-terminal-here** item is chosen; the
    /// split multiplexer drains it ([`Self::take_new_terminal_here`]) and splits
    /// the pane inheriting its cwd (TERM-15 reuses the TERM-4/5 spawn).
    new_terminal_here: bool,
}

/// The item a TERM-15 context-menu click selected, recorded inside the menu
/// closure and dispatched after it returns (so no `self` borrow crosses the
/// closure). `Custom` carries the index into the config's command list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MenuChoice {
    /// A user-defined custom command by list index.
    Custom(usize),
    /// Send the selection to Chat.
    Chat,
    /// Open the selection as a path in Files.
    Files,
    /// Open the selection as a URL in the mesh browser.
    Browser,
    /// Split a new terminal here, inheriting the pane's cwd.
    NewHere,
}

impl TerminalWidget {
    /// Wrap a spawned local shell. The widget sizes the PTY to its rect on the
    /// first frame, so the spawn dimensions only cover the gap until then.
    #[must_use]
    pub fn new(pty: LocalPty) -> Self {
        Self::over(Session::Local(pty))
    }

    /// Wrap a remote mesh shell (TERM-8), driven over the broker. The widget
    /// resizes it to its rect on the first frame exactly as it does a local one.
    #[must_use]
    pub fn new_remote(remote: RemotePty) -> Self {
        Self::over(Session::Remote(Box::new(remote)))
    }

    /// Wrap either backing behind the shared render/input path.
    #[must_use]
    fn over(session: Session) -> Self {
        Self {
            session,
            font_size: Style::BODY,
            cursor_blink: true,
            cursor_shape: CursorShape::default(),
            palette: Palette::from_tokens(),
            scroll_offset: 0,
            scroll_accum: 0.0,
            selection: None,
            mouse_report_button: None,
            last_grid: None,
            input_echo: Vec::new(),
            search: Search::new(),
            search_history: 0,
            search_follow: false,
            clip: ClipboardOptions::default(),
            primary: None,
            launch_bus: Arc::new(BusLaunchClient::from_env()),
            title: PaneTitle::new("shell"),
            watch: ActivityWatch::default(),
            bell: Bell::default(),
            notify_bus: Arc::new(BusNotifyClient::from_env()),
            menu: ContextMenu::default(),
            chat_bus: Arc::new(BusChatClient::from_env()),
            runner: Arc::new(OsCommandRunner),
            new_terminal_here: false,
        }
    }

    /// The content font size in points (lock 13: font size is a knob).
    #[must_use]
    pub const fn with_font_size(mut self, size: f32) -> Self {
        self.font_size = size;
        self
    }

    /// Whether the focused cursor blinks (lock 13: cursor style knob).
    #[must_use]
    pub const fn with_cursor_blink(mut self, blink: bool) -> Self {
        self.cursor_blink = blink;
        self
    }

    /// The focused cursor's shape — block / bar / underline (lock 13/Q13).
    #[must_use]
    pub const fn with_cursor_shape(mut self, shape: CursorShape) -> Self {
        self.cursor_shape = shape;
        self
    }

    /// The active content colour scheme — the Quasar default or a preset (TERM-11).
    #[must_use]
    pub const fn with_palette(mut self, palette: Palette) -> Self {
        self.palette = palette;
        self
    }

    /// Adopt the surface's [`Appearance`] (TERM-11): scheme + font size + cursor
    /// style. The split multiplexer calls this on every pane each frame, so a
    /// change in the appearance picker reaches every live shell at once. A no-op
    /// when nothing changed (the fields are plain assignments).
    pub const fn apply_appearance(&mut self, a: &Appearance) {
        self.palette = a.palette;
        self.font_size = a.font_size;
        self.cursor_shape = a.cursor_shape;
        self.cursor_blink = a.cursor_blink;
    }

    /// Smart-clipboard toggles (copy-on-select, middle-click paste — Q12).
    #[must_use]
    pub const fn with_clipboard_options(mut self, opts: ClipboardOptions) -> Self {
        self.clip = opts;
        self
    }

    /// Inject the surface-launch Bus seam (tests record the routed opens;
    /// production resolves the live Bus via [`Self::over`]).
    #[must_use]
    pub fn with_launch_bus(mut self, bus: Arc<dyn LaunchBus>) -> Self {
        self.launch_bus = bus;
        self
    }

    /// Inject the notification Bus seam (TERM-12) — tests record the raised
    /// notices; production resolves the live Bus via [`Self::over`].
    #[must_use]
    pub fn with_notify_bus(mut self, bus: Arc<dyn NotifyBus>) -> Self {
        self.notify_bus = bus;
        self
    }

    /// The selection context menu config (TERM-15): the user's custom commands +
    /// the Chat recipient. The split multiplexer pushes the surface's shared menu
    /// into every pane, so a config change reaches every live shell.
    #[must_use]
    pub fn with_context_menu(mut self, menu: ContextMenu) -> Self {
        self.menu = menu;
        self
    }

    /// Adopt the surface's context-menu config (TERM-15) — the split multiplexer
    /// calls this on every pane, mirroring [`Self::apply_appearance`]. A no-op
    /// when nothing changed.
    pub fn apply_context_menu(&mut self, menu: &ContextMenu) {
        if &self.menu != menu {
            self.menu = menu.clone();
        }
    }

    /// Inject the Chat Bus seam (TERM-15) — tests record the sends; production
    /// resolves the live Bus via [`Self::over`].
    #[must_use]
    pub fn with_chat_bus(mut self, bus: Arc<dyn ChatBus>) -> Self {
        self.chat_bus = bus;
        self
    }

    /// Inject the custom-command runner seam (TERM-15) — tests record the argv;
    /// production spawns the process via [`Self::over`].
    #[must_use]
    pub fn with_command_runner(mut self, runner: Arc<dyn CommandRunner>) -> Self {
        self.runner = runner;
        self
    }

    /// Take (and clear) the pending **new-terminal-here** request the context menu
    /// raised (TERM-15). The split multiplexer polls this after rendering the pane
    /// and, when set, splits the pane inheriting its cwd.
    pub fn take_new_terminal_here(&mut self) -> bool {
        std::mem::take(&mut self.new_terminal_here)
    }

    /// Raise a new-terminal-here request as if the context menu were chosen —
    /// the split multiplexer's drain test drives the reused spawn through this.
    #[cfg(test)]
    pub(crate) const fn request_new_terminal_here(&mut self) {
        self.new_terminal_here = true;
    }

    /// Seed the pane's fallback title (its stable ordinal), shown until the
    /// running command sets its own OSC title (TERM-12).
    #[must_use]
    pub fn with_title_fallback(mut self, fallback: impl Into<String>) -> Self {
        self.title = PaneTitle::new(fallback);
        self
    }

    /// This pane's title (TERM-12) — its shown label + override/edit state.
    #[must_use]
    pub const fn pane_title(&self) -> &PaneTitle {
        &self.title
    }

    /// The pane's shown title text (override → derived → fallback).
    #[must_use]
    pub fn title_text(&self) -> &str {
        self.title.display()
    }

    /// Begin renaming this pane (the `RenamePane` action / a title-strip click).
    pub fn begin_rename(&mut self) {
        self.title.begin_edit();
    }

    /// Set the pane's title override outright — a programmatic rename (an empty
    /// name reverts to the derived/fallback title).
    pub fn set_title_override(&mut self, name: impl Into<String>) {
        self.title.set_override(name);
    }

    /// This pane's activity/silence watch state (TERM-12).
    #[must_use]
    pub const fn watch(&self) -> ActivityWatch {
        self.watch
    }

    /// Toggle watch-for-activity on this pane (the `ToggleActivityWatch` action).
    pub fn toggle_activity_watch(&mut self) {
        self.watch.toggle(WatchMode::Activity);
    }

    /// Toggle watch-for-silence on this pane (the `ToggleSilenceWatch` action).
    pub fn toggle_silence_watch(&mut self) {
        self.watch.toggle(WatchMode::Silence);
    }

    /// This pane's bell configuration (TERM-12).
    #[must_use]
    pub const fn bell_config(&self) -> BellConfig {
        self.bell.config()
    }

    /// Set this pane's bell style (visual / audible / off).
    pub const fn set_bell_config(&mut self, config: BellConfig) {
        self.bell.set_config(config);
    }

    /// Whether the pane should reap (close) — a local child exit, or a remote
    /// clean shell exit (a remote failure lingers). The split multiplexer's
    /// close-on-exit (TERM-4) reads this.
    #[must_use]
    pub fn is_output_closed(&self) -> bool {
        self.session.is_output_closed()
    }

    /// Run `f` against this pane's engine state (tests + the splits registry read
    /// the grid through it).
    pub fn with_terminal<R>(&self, f: impl FnOnce(&crate::engine::Terminal) -> R) -> R {
        self.session.with_terminal(f)
    }

    /// The local PTY when this pane is a local shell (the reap / child-pid tests
    /// read through it); `None` for a remote pane.
    #[must_use]
    pub const fn local_pty(&self) -> Option<&LocalPty> {
        self.session.local()
    }

    /// This pane's remote target — its peer + node marker — when it is a remote
    /// shell (TERM-10 layout capture records it); `None` for a local pane. Rebuilt
    /// straight into the remote-open path, so a saved remote pane reconnects to
    /// the same node.
    #[must_use]
    pub fn remote_target(&self) -> Option<crate::picker::RemoteTarget> {
        self.session.remote().map(|r| crate::picker::RemoteTarget {
            peer: r.peer().to_string(),
            label: r.node_label().to_string(),
        })
    }

    /// This pane's active content scheme (TERM-11) — the appearance the split
    /// multiplexer last pushed in. Test-only: production reads the surface's own
    /// [`Appearance`], never a pane's copy.
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn palette(&self) -> Palette {
        self.palette
    }

    /// Render one frame into `ui`, consuming this frame's input. Fills all
    /// available space.
    pub fn show(&mut self, ui: &mut Ui) -> Response {
        // Drain any pending backing work first (a remote pane reads its Bus state
        // log; a local pane pumps on its own threads, so this is a no-op there).
        self.session.poll();
        // One frame of local input only: last frame's broadcast echo is spent.
        self.input_echo.clear();
        let now = ui.input(|i| i.time);
        // TERM-12: fold the engine's title/bell events + the activity/silence
        // watcher into pane state (raising any due notices) before rendering.
        self.pump_pane_events(now);

        // The grid paints monospace; `crate::fonts::install` puts the bundled
        // Droid Sans Mono face first in the Monospace family, so this `FontId`
        // resolves to it.
        let font_id = FontId::monospace(self.font_size);
        let cell = ui.fonts(|f| Vec2::new(f.glyph_width(&font_id, 'M'), f.row_height(&font_id)));

        // Carve the pane into a TERM-12 title strip and the grid below it.
        let area = ui.available_rect_before_wrap();
        let strip_h = TITLE_STRIP_H.min(area.height());
        let strip_rect = Rect::from_min_size(area.min, Vec2::new(area.width(), strip_h));
        let rect = Rect::from_min_max(Pos2::new(area.min.x, area.min.y + strip_h), area.max);
        self.show_title_strip(ui, strip_rect);

        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        // A rename in progress holds the keyboard on this pane (so its shell —
        // and any other pane — doesn't also see the typed name, TERM-12).
        if self.title.is_editing() {
            response.request_focus();
        }
        let (cols, rows) = grid_size(rect.size(), cell);

        // A changed rect maps to a new grid: engine reflow + a resize to the
        // backing (TIOCSWINSZ locally, a `pty/resize` verb remotely).
        if self.last_grid != Some((cols, rows)) {
            self.session.resize(cols, rows);
            self.last_grid = Some((cols, rows));
        }

        // TERM-13 mouse reporting: forward SGR (1006) reports to the PTY when the
        // running app enabled mouse tracking — unless Shift is held, the bypass
        // that always keeps native text selection. Computed once here and threaded
        // through input so the wheel routes to a scroll report, not the scrollback.
        let modifiers = ui.input(|i| i.modifiers);
        let mouse_report = !modifiers.shift
            && self
                .session
                .with_terminal(|t| t.mouse_reporting() && t.sgr_mouse());

        // Input first, so a scroll/snap lands in this frame's snapshot.
        let history = self
            .session
            .with_terminal(crate::engine::Terminal::scrollback_len);
        self.handle_input(
            ui,
            &response,
            cell,
            usize::from(rows),
            history,
            mouse_report,
        );

        // Scrollback search (TERM-9): rescan on a query/mode change or new
        // output, then scroll the current match into view. The full-history
        // snapshot is taken only when a rescan is actually due.
        if self.search.active() && (self.search.dirty() || history != self.search_history) {
            let full = self.session.with_terminal(crate::engine::Terminal::full);
            self.search.recompute(&full);
            self.search_history = history;
        }
        if self.search_follow {
            if let Some(row) = self.search.current_row() {
                self.scroll_offset = scroll_for_row(row, history, usize::from(rows));
            }
            self.search_follow = false;
        }
        self.scroll_offset = self.scroll_offset.min(history);

        // One engine read for the visible window (O(rows × cols), never the
        // full history).
        let screen = self.session.with_terminal(|t| t.window(self.scroll_offset));
        let first_abs = history - self.scroll_offset;

        // Mouse-mode apps own the pointer (TERM-13): forward SGR reports and skip
        // the local selection gestures. Shift-bypass (folded into `mouse_report`)
        // and a non-tracking app both fall through to the native selection path.
        if mouse_report {
            self.report_mouse(ui, rect, cell, &screen);
        } else {
            self.mouse_report_button = None;
            self.handle_pointer(&response, rect, cell, first_abs, &screen, modifiers);
        }

        // The backing's render chrome: liveness (cursor + repaint), the node
        // marker (remote), and the honest status note (§7).
        let render = self.session.render_state();
        let live = render.live;
        let cursor = self.cursor_paint(&response, ui.input(|i| i.time), live);
        let search_hits = self.search_hits(first_abs, screen.rows());
        let search_bar = self.search_bar();
        paint_grid(
            &ui.painter_at(rect),
            rect,
            &screen,
            &PaintSpec {
                font_id,
                cell,
                palette: self.palette,
                cursor_shape: self.cursor_shape,
                first_abs,
                selection: self.selection,
                cursor,
                scrolled: self.scroll_offset,
                node: render.node,
                note: render.note,
                search_hits,
                search_bar,
            },
        );

        // TERM-12 visual bell: a brief translucent flash over the pane that
        // decays over `bell::FLASH_SECS`.
        let flash = self.bell.flash_alpha(now);
        if flash > 0.0 {
            // `flash * peak` is bounded to 0..255 by the clamp, so the cast is exact.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let alpha = (flash * BELL_FLASH_PEAK).clamp(0.0, 255.0) as u8;
            ui.painter_at(rect)
                .rect_filled(rect, 0.0, egui::Color32::from_white_alpha(alpha));
        }

        if live {
            ui.ctx().request_repaint_after(LIVE_REPAINT);
        } else if self.bell.is_flashing(now) || self.watch.is_active() {
            // Keep frames coming for the flash decay + the silence timer even
            // when the shell itself is idle.
            ui.ctx().request_repaint_after(LIVE_REPAINT);
        }

        // TERM-15: the selection context menu (custom commands + mesh actions),
        // right-click on the grid. Attached last so it reads this frame's
        // selection.
        self.show_selection_menu(&response);
        response
    }

    /// The cursor paint mode for this frame: hidden when the backing is dead or
    /// scrolled into history, hollow when the pane is unfocused, else filled on
    /// the blink-on phase (or always, when blink is off). `time` is the egui
    /// frame clock (seconds).
    fn cursor_paint(&self, response: &Response, time: f64, live: bool) -> CursorPaint {
        if !live || self.scroll_offset > 0 {
            CursorPaint::Hidden
        } else if !response.has_focus() {
            CursorPaint::Hollow
        } else if !self.cursor_blink || blink_on(time) {
            CursorPaint::Filled
        } else {
            CursorPaint::Hidden
        }
    }

    /// Fold this frame's engine title/bell events + the activity/silence
    /// watcher into pane state, raising any due notices through the notify seam.
    /// `now` is the egui frame clock (seconds).
    fn pump_pane_events(&mut self, now: f64) {
        // One engine read for both the drained events and the output counter.
        let (events, seq) = self
            .session
            .with_terminal(|t| (t.drain_events(), t.bytes_seen()));
        for event in events {
            match event {
                TermEvent::Title(title) => self.title.set_derived(title),
                TermEvent::ResetTitle => self.title.reset_derived(),
                TermEvent::Bell => self.ring_bell(now),
            }
        }
        self.tick_watch(seq, now);
    }

    /// Fold a terminal `BEL` at frame time `now`: start the visual flash (if the
    /// bell is visual) and publish an audible notice (if audible), per the pane's
    /// [`BellConfig`]. Split out so the config wiring is unit-testable.
    fn ring_bell(&mut self, now: f64) {
        let effect = self.bell.ring(now);
        if effect.notify {
            self.raise_notice(
                NoticeLevel::Warning,
                format!("bell \u{2014} {}", self.title.display()),
            );
        }
    }

    /// Fold one watcher observation and publish a notice on an edge. Split out so
    /// the wiring is unit-testable with a synthetic output counter + clock.
    fn tick_watch(&mut self, seq: u64, now: f64) -> Option<WatchEvent> {
        let edge = self.watch.observe(seq, now)?;
        let what = match edge {
            WatchEvent::Activity => "activity",
            WatchEvent::Silence => "silence",
        };
        self.raise_notice(
            NoticeLevel::Info,
            format!("{what} \u{2014} {}", self.title.display()),
        );
        Some(edge)
    }

    /// Publish a desktop notice through the shared notify seam (best-effort — a
    /// no-Bus node degrades to a silently dropped notice, never a panic/hang).
    fn raise_notice(&self, level: NoticeLevel, headline: String) {
        let _ = self.notify_bus.notify(&TermNotice::new(
            level,
            crate::layout::local_node(),
            headline,
        ));
    }

    /// The per-pane title strip (TERM-12): the shown title (or the live rename
    /// buffer) on the left, the watch badge on the right. A click begins a
    /// rename. All chrome resolves through `Style` tokens (§4).
    fn show_title_strip(&mut self, ui: &Ui, rect: Rect) {
        let resp = ui.interact(rect, ui.id().with("term-title"), Sense::click());
        if resp.clicked() && !self.title.is_editing() {
            self.title.begin_edit();
        }

        let painter = ui.painter_at(rect);
        let editing = self.title.is_editing();
        let bg = if editing {
            Style::SURFACE_HI
        } else {
            Style::SURFACE
        };
        painter.rect_filled(rect, 0.0, bg);
        painter.hline(rect.x_range(), rect.max.y, Stroke::new(1.0, Style::BORDER));

        let font = FontId::proportional(Style::SMALL);
        let (text, color) = if editing {
            (
                format!("{}\u{2502}", self.title.edit_buffer().unwrap_or_default()),
                Style::TEXT,
            )
        } else if self.title.is_overridden() {
            (self.title.display().to_owned(), Style::TEXT)
        } else {
            (self.title.display().to_owned(), Style::TEXT_DIM)
        };
        painter.text(
            Pos2::new(rect.min.x + Style::SP_XS, rect.center().y),
            Align2::LEFT_CENTER,
            text,
            font.clone(),
            color,
        );

        // Right-aligned watch badge, when a watch is armed.
        if let Some((label, tone)) = watch_badge(self.watch.mode()) {
            let galley = painter.layout_no_wrap(label.to_owned(), font, tone);
            let x = rect.max.x - Style::SP_XS - galley.size().x;
            painter.galley(
                Pos2::new(x, rect.center().y - galley.size().y / 2.0),
                galley,
                tone,
            );
        }
    }

    /// Drive an in-progress rename from one event (the `search_event` idiom):
    /// type into the buffer, backspace, commit (Enter) or cancel (Escape).
    /// Anything else is swallowed so it never reaches the shell mid-rename.
    fn rename_event(&mut self, event: &Event) {
        match event {
            Event::Text(text) => {
                if let Some(buf) = self.title.edit_buffer_mut() {
                    buf.extend(text.chars().filter(|c| !c.is_control()));
                }
            }
            Event::Key {
                key, pressed: true, ..
            } => match key {
                Key::Backspace => {
                    if let Some(buf) = self.title.edit_buffer_mut() {
                        buf.pop();
                    }
                }
                Key::Enter => self.title.commit_edit(),
                Key::Escape => self.title.cancel_edit(),
                _ => {}
            },
            _ => {}
        }
    }

    /// Keyboard + clipboard + wheel, from this frame's event stream.
    ///
    /// `mouse_report` (TERM-13) is true when the running app has the mouse and
    /// Shift is up: the wheel then belongs to the app (reported as a scroll
    /// button by [`Self::report_mouse`]), so it no longer pages the scrollback.
    fn handle_input(
        &mut self,
        ui: &Ui,
        response: &Response,
        cell: Vec2,
        rows: usize,
        history: usize,
        mouse_report: bool,
    ) {
        if response.clicked() || response.drag_started() {
            response.request_focus();
        }
        // A lone terminal grabs the keyboard at launch (TERM-4's split panes
        // manage focus explicitly; here "nothing focused" means us).
        if ui.memory(|m| m.focused().is_none()) && !self.session.is_output_closed() {
            response.request_focus();
        }

        let focused = response.has_focus();
        if focused {
            // Tab/arrows/escape belong to the shell, not egui focus traversal.
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    response.id,
                    EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                );
            });
        }

        let (events, shift_held) = ui.input(|i| (i.events.clone(), i.modifiers.shift));
        for event in events {
            // Wheel scrolling works on hover, focused or not — unless a mouse-mode
            // app owns the wheel (TERM-13), in which case `report_mouse` forwards
            // it as a scroll-button report instead of paging the scrollback.
            if let Event::MouseWheel { unit, delta, .. } = &event {
                if response.hovered() && !mouse_report {
                    self.wheel(*unit, delta.y, cell.y, rows, history);
                }
                continue;
            }
            // A rename in progress (TERM-12) captures the keyboard — typed keys
            // edit the pane title and never reach the shell.
            if self.title.is_editing() {
                self.rename_event(&event);
                continue;
            }
            if !focused {
                continue;
            }
            // Ctrl+Shift+F opens / closes the scrollback-search overlay (TERM-9)
            // — claimed before the query grabs plain keys below.
            if let Event::Key {
                key: Key::F,
                pressed: true,
                modifiers,
                ..
            } = &event
            {
                if modifiers.ctrl && modifiers.shift {
                    self.search.toggle();
                    self.search_follow = self.search.active();
                    continue;
                }
            }
            // With the overlay open, typed keys drive the query, not the shell
            // (shell input resumes when it closes).
            if self.search.active() {
                self.search_event(&event);
                continue;
            }
            match event {
                Event::Text(text) => self.send(text.as_bytes()),
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => self.on_key(ui.ctx(), key, modifiers, rows, history),
                Event::Paste(text) => self.send(&paste_bytes(&text)),
                // winit folds BOTH Ctrl+C and Ctrl+Shift+C into `Copy` (the raw
                // key never reaches us); shift disambiguates — the chord copies
                // the selection, plain Ctrl+C stays the terminal's own ETX.
                Event::Copy => {
                    if shift_held {
                        self.copy_selection(ui.ctx());
                    } else {
                        self.send(&[0x03]);
                    }
                }
                // Ctrl+X likewise arrives as `Cut`; the shell gets its CAN byte.
                Event::Cut => self.send(&[0x18]),
                _ => {}
            }
        }
    }

    /// One key press while focused.
    fn on_key(
        &mut self,
        ctx: &Context,
        key: Key,
        modifiers: Modifiers,
        rows: usize,
        history: usize,
    ) {
        // The explicit copy/paste chords (bare-DRM backends deliver these as
        // raw keys; under winit they arrive as Copy/Paste events instead).
        if modifiers.ctrl && modifiers.shift && key == Key::C {
            self.copy_selection(ctx);
            return;
        }
        if modifiers.ctrl && modifiers.shift && key == Key::V {
            // Paste lands via `Event::Paste`; egui has no synchronous
            // clipboard read to fall back on here.
            return;
        }
        // Shift+PgUp/PgDn page the scrollback (terminal convention).
        if modifiers.shift && key == Key::PageUp {
            self.scroll_by(page_delta(rows), history);
            return;
        }
        if modifiers.shift && key == Key::PageDown {
            self.scroll_by(-page_delta(rows), history);
            return;
        }
        if let Some(bytes) = encode_key(key, modifiers) {
            self.send(&bytes);
        }
    }

    /// Fold one wheel event into the scrollback offset.
    fn wheel(
        &mut self,
        unit: MouseWheelUnit,
        delta_y: f32,
        cell_h: f32,
        rows: usize,
        history: usize,
    ) {
        // cols/rows are u16-bounded; f32 holds them exactly.
        #[allow(clippy::cast_precision_loss)]
        let lines = match unit {
            MouseWheelUnit::Line => delta_y,
            MouseWheelUnit::Point => delta_y / cell_h.max(1.0),
            MouseWheelUnit::Page => delta_y * rows as f32,
        };
        self.scroll_accum += lines;
        let whole = self.scroll_accum.trunc();
        if whole != 0.0 {
            self.scroll_accum -= whole;
            // Bounded by the wheel event's line count — far inside i64.
            #[allow(clippy::cast_possible_truncation)]
            self.scroll_by(whole as i64, history);
        }
    }

    /// Move the scrollback window by `delta` lines (positive = older), clamped
    /// to `[0, history]`.
    fn scroll_by(&mut self, delta: i64, history: usize) {
        let cur = i64::try_from(self.scroll_offset).unwrap_or(i64::MAX);
        let next = cur.saturating_add(delta).max(0);
        self.scroll_offset = usize::try_from(next).unwrap_or(usize::MAX).min(history);
    }

    /// Drive the search overlay from one event: type into the query, delete,
    /// navigate matches, or toggle regex/case. Anything else is swallowed so it
    /// never reaches the shell while the overlay is open.
    fn search_event(&mut self, event: &Event) {
        match event {
            Event::Text(text) => {
                self.search.push_str(text);
                self.search_follow = true;
            }
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => match key {
                Key::Backspace => {
                    self.search.pop_char();
                    self.search_follow = true;
                }
                Key::Escape => self.search.close(),
                // Enter / F3 step forward, Shift+ steps back — the classic
                // find-next / find-previous.
                Key::Enter | Key::F3 => {
                    if modifiers.shift {
                        self.search.prev_match();
                    } else {
                        self.search.next_match();
                    }
                    self.search_follow = true;
                }
                // Alt+R flips literal ⇄ regex; Alt+C cycles the case mode.
                Key::R if modifiers.alt => self.search.toggle_regex(),
                Key::C if modifiers.alt => self.search.cycle_case(),
                _ => {}
            },
            _ => {}
        }
    }

    /// The search matches falling inside the current window, projected to
    /// window-local rows for the painter (TERM-9).
    fn search_hits(&self, first_abs: usize, rows: usize) -> Vec<SearchHit> {
        if !self.search.active() {
            return Vec::new();
        }
        self.search
            .matches()
            .iter()
            .enumerate()
            .filter_map(|(i, m)| {
                let row = m.row.checked_sub(first_abs)?;
                (row < rows).then_some(SearchHit {
                    row,
                    col: m.col,
                    len: m.len,
                    current: self.search.current_index() == Some(i),
                })
            })
            .collect()
    }

    /// The search-overlay chip for this frame (`None` when closed).
    fn search_bar(&self) -> Option<SearchBar> {
        if !self.search.active() {
            return None;
        }
        let mut flags = String::new();
        if self.search.is_regex() {
            flags.push_str("re ");
        }
        flags.push_str(self.search.case().label());
        // A tuple match keeps the four states flat (and clear of the nursery's
        // if-let/else rewrite): error → empty → no-match → the i/n counter.
        let (status, tone) = match (
            self.search.error(),
            self.search.query().is_empty(),
            self.search.count(),
        ) {
            (Some(err), _, _) => (format!("regex: {err}"), Style::DANGER),
            (None, true, _) => ("type to search".to_string(), Style::TEXT_DIM),
            (None, false, 0) => ("no matches".to_string(), Style::TEXT_DIM),
            (None, false, n) => {
                let at = self.search.current_index().map_or(0, |i| i + 1);
                (format!("{at}/{n}"), Style::ACCENT)
            }
        };
        Some(SearchBar {
            label: format!("find: {}  {status}  [{flags}]", self.search.query()),
            tone,
        })
    }

    /// Set the selection to `[start, end)` cells on absolute `row` (the
    /// [`Selection`] head is inclusive, so it lands on `end - 1`).
    const fn set_span_selection(&mut self, row: usize, start: usize, end: usize) {
        if end <= start {
            return;
        }
        self.selection = Some(Selection {
            anchor: CellPos { row, col: start },
            head: CellPos { row, col: end - 1 },
        });
    }

    /// Finish a selection: refresh the in-process PRIMARY buffer (what a
    /// middle-click pastes) and, when copy-on-select is on, mirror it to the
    /// clipboard. A one-shot full snapshot — the selection may live in history.
    fn after_select(&mut self, ctx: &Context) {
        let Some(sel) = self.selection else {
            return;
        };
        let text = self
            .session
            .with_terminal(|t| selected_text(&t.full(), &sel));
        if text.is_empty() {
            return;
        }
        if self.clip.copy_on_select {
            ctx.copy_text(text.clone());
        }
        self.primary = Some(text);
    }

    /// The current selection's text (a one-shot full snapshot — the selection may
    /// live in history), or `None` when nothing is selected or it is empty. The
    /// input every TERM-15 context-menu action folds over.
    fn selection_text(&self) -> Option<String> {
        let sel = self.selection?;
        let text = self
            .session
            .with_terminal(|t| selected_text(&t.full(), &sel));
        (!text.is_empty()).then_some(text)
    }

    /// This pane's live cwd — the local shell's `/proc/<pid>/cwd`, the same source
    /// the TERM-10 layout capture reads. `None` for a remote pane or a gone pid.
    fn pane_cwd(&self) -> Option<std::path::PathBuf> {
        self.local_pty()
            .and_then(|pty| crate::layout::cwd_of_pid(pty.child_pid()))
    }

    /// The selection context menu (TERM-15): the user's custom commands over the
    /// selection (Terminator parity), then the four built-in mesh actions, each
    /// reusing an existing surface-launch verb (§6). Right-click on the grid.
    ///
    /// The closure only *records* the chosen item (no `self` borrow inside it);
    /// the effect is dispatched afterwards through the injectable seams, so each
    /// action is unit-tested headless via the recorders. §4 Carbon tokens carry
    /// the section captions; the menu chrome itself renders through the installed
    /// [`Style`] visuals.
    fn show_selection_menu(&mut self, response: &Response) {
        let Some(text) = self.selection_text() else {
            return;
        };
        let commands = self.menu.commands.clone();
        let mut chosen: Option<MenuChoice> = None;
        response.context_menu(|ui| {
            ui.set_max_width(220.0);
            if !commands.is_empty() {
                ui.label(
                    RichText::new("Custom commands")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                for (i, cmd) in commands.iter().enumerate() {
                    if ui.button(&cmd.label).clicked() {
                        chosen = Some(MenuChoice::Custom(i));
                        ui.close_menu();
                    }
                }
                ui.separator();
            }
            ui.label(
                RichText::new("Mesh actions")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            if ui.button("Send selection to Chat").clicked() {
                chosen = Some(MenuChoice::Chat);
                ui.close_menu();
            }
            if ui.button("Open path in Files").clicked() {
                chosen = Some(MenuChoice::Files);
                ui.close_menu();
            }
            if ui.button("Open URL in browser").clicked() {
                chosen = Some(MenuChoice::Browser);
                ui.close_menu();
            }
            if ui.button("New terminal here").clicked() {
                chosen = Some(MenuChoice::NewHere);
                ui.close_menu();
            }
        });
        match chosen {
            Some(MenuChoice::Custom(i)) => {
                if let Some(cmd) = commands.get(i) {
                    self.run_custom_command(cmd, &text);
                }
            }
            Some(MenuChoice::Chat) => self.send_selection_to_chat(&text),
            Some(MenuChoice::Files) => self.open_selection_in_files(&text),
            Some(MenuChoice::Browser) => self.open_selection_url(&text),
            Some(MenuChoice::NewHere) => self.new_terminal_here = true,
            None => {}
        }
    }

    /// Dispatch a custom command over the selection (TERM-15): substitute the
    /// selection into the template's argv and run it in the pane's cwd. Best-
    /// effort — a spawn failure is swallowed (the runner degrades honestly).
    pub(crate) fn run_custom_command(&self, cmd: &crate::menu::CustomCommand, selection: &str) {
        let _ = self
            .runner
            .run(&cmd.argv(selection), self.pane_cwd().as_deref());
    }

    /// Send the selection to Chat (TERM-15) — reuse the NOTIFY-CHAT
    /// `action/chat/send` verb via the [`ChatBus`] seam, to the config's recipient.
    pub(crate) fn send_selection_to_chat(&self, selection: &str) {
        let _ = self.chat_bus.send(&self.menu.chat_recipient, selection);
    }

    /// Open the selection as a path in the Files surface (TERM-15) — reuse the
    /// TERM-9 [`smart::LaunchRoute::Files`] surface-launch path.
    pub(crate) fn open_selection_in_files(&self, selection: &str) {
        let _ = self
            .launch_bus
            .open(&smart::LaunchRoute::Files(selection.to_string()));
    }

    /// Open the selection as a URL in the mesh browser (TERM-15) — reuse the
    /// TERM-9 [`smart::LaunchRoute::Bookmarks`] surface-launch path.
    pub(crate) fn open_selection_url(&self, selection: &str) {
        let _ = self
            .launch_bus
            .open(&smart::LaunchRoute::Bookmarks(selection.to_string()));
    }

    /// Mouse selection + the smart-clipboard gestures (TERM-9): double-click
    /// smart-selects a word/URL/path, triple-click a line, middle-click pastes
    /// the PRIMARY buffer, Ctrl+click opens a detected URL/path; press anchors,
    /// drag extends, plain click clears.
    fn handle_pointer(
        &mut self,
        response: &Response,
        rect: Rect,
        cell: Vec2,
        first_abs: usize,
        screen: &Screen,
        modifiers: Modifiers,
    ) {
        let ctx = &response.ctx;
        let pos_to_local = |pos: Pos2| cell_at(rect.min, cell, pos, screen.cols(), screen.rows());
        let pos_to_cell = |pos: Pos2| {
            let (row, col) = cell_at(rect.min, cell, pos, screen.cols(), screen.rows());
            CellPos {
                row: first_abs + row,
                col,
            }
        };

        // Triple-click → whole visible line; double-click → word/URL/path.
        if response.triple_clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let (row, _) = pos_to_local(pos);
                if let Some((s, e)) = smart::line_span(&row_chars(screen, row)) {
                    self.set_span_selection(first_abs + row, s, e);
                    self.after_select(ctx);
                }
            }
            return;
        }
        if response.double_clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let (row, col) = pos_to_local(pos);
                if let Some((_, s, e)) = smart::smart_span(&row_chars(screen, row), col) {
                    self.set_span_selection(first_abs + row, s, e);
                    self.after_select(ctx);
                }
            }
            return;
        }
        // Middle-click pastes the PRIMARY buffer (X11 select-to-paste emulation).
        if response.middle_clicked() {
            if self.clip.paste_on_middle_click {
                if let Some(text) = self.primary.clone() {
                    self.send(&paste_bytes(&text));
                }
            }
            return;
        }
        // Ctrl+click a detected URL/path → dispatch it to its surface over the
        // Bus (URL → Bookmarks, path → Files). A miss falls through to a click.
        if response.clicked() && (modifiers.command || modifiers.ctrl) {
            if let Some(pos) = response.interact_pointer_pos() {
                let (row, col) = pos_to_local(pos);
                if let Some(route) = smart::detect_launch(&row_chars(screen, row), col) {
                    let _ = self.launch_bus.open(&route);
                    return;
                }
            }
        }

        if response.drag_started() {
            if let Some(pos) = response.interact_pointer_pos() {
                let p = pos_to_cell(pos);
                self.selection = Some(Selection { anchor: p, head: p });
            }
        } else if response.dragged() {
            if let (Some(pos), Some(sel)) =
                (response.interact_pointer_pos(), self.selection.as_mut())
            {
                sel.head = pos_to_cell(pos);
            }
        } else if response.drag_stopped() {
            self.after_select(ctx);
        } else if response.clicked() {
            self.selection = None;
        }
    }

    /// Forward this frame's pointer activity to the running app as SGR (1006)
    /// mouse reports (TERM-13). Only reached when the app enabled mouse tracking
    /// **and** Shift is up (the caller's `mouse_report` gate), so this never
    /// steals a Shift+drag native selection.
    ///
    /// Presses/releases report per button; motion reports as a drag over the held
    /// button (DECSET 1002) or, if the app asked for any-motion (1003), as a
    /// buttonless hover; the wheel reports as the scroll pseudo-buttons. Every
    /// report is written straight to the backing ([`Self::write_raw`]) so it
    /// neither snaps the scrollback nor fans out to grouped panes.
    fn report_mouse(&mut self, ui: &Ui, rect: Rect, cell: Vec2, screen: &Screen) {
        let (motion_all, drag) = self
            .session
            .with_terminal(|t| (t.mouse_motion(), t.mouse_drag()));
        let cols = screen.cols();
        let rows = screen.rows();
        let events = ui.input(|i| i.events.clone());
        for event in &events {
            match event {
                Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    modifiers,
                } => {
                    if !rect.contains(*pos) {
                        continue;
                    }
                    let Some(btn) = MouseButton::from_egui(*button) else {
                        continue;
                    };
                    let (row, col) = cell_at(rect.min, cell, *pos, cols, rows);
                    let kind = if *pressed {
                        self.mouse_report_button = Some(btn);
                        MouseEvent::Press(btn)
                    } else {
                        if self.mouse_report_button == Some(btn) {
                            self.mouse_report_button = None;
                        }
                        MouseEvent::Release(btn)
                    };
                    self.write_raw(&encode_sgr(kind, col, row, *modifiers));
                }
                Event::PointerMoved(pos) => {
                    if !rect.contains(*pos) {
                        continue;
                    }
                    let (row, col) = cell_at(rect.min, cell, *pos, cols, rows);
                    let mods = ui.input(|i| i.modifiers);
                    if let Some(btn) = self.mouse_report_button {
                        // Motion with a button held: DECSET 1002 or 1003.
                        if drag || motion_all {
                            self.write_raw(&encode_sgr(MouseEvent::Drag(btn), col, row, mods));
                        }
                    } else if motion_all {
                        // Buttonless hover: DECSET 1003 any-motion only.
                        self.write_raw(&encode_sgr(MouseEvent::Motion, col, row, mods));
                    }
                }
                Event::MouseWheel { delta, .. } => {
                    // egui wheel: +y is up (older content) — the scroll-up
                    // button; a zero-y (pure horizontal) wheel reports nothing.
                    let kind = if delta.y > 0.0 {
                        MouseEvent::ScrollUp
                    } else if delta.y < 0.0 {
                        MouseEvent::ScrollDown
                    } else {
                        continue;
                    };
                    let Some(pos) = ui.input(|i| i.pointer.hover_pos()) else {
                        continue;
                    };
                    if !rect.contains(pos) {
                        continue;
                    }
                    let (row, col) = cell_at(rect.min, cell, pos, cols, rows);
                    let mods = ui.input(|i| i.modifiers);
                    self.write_raw(&encode_sgr(kind, col, row, mods));
                }
                _ => {}
            }
        }
    }

    /// Copy the current selection to the clipboard (no-op without one).
    fn copy_selection(&self, ctx: &Context) {
        if let Some(sel) = self.selection {
            // One-shot full snapshot: the selection may live in history.
            let text = self
                .session
                .with_terminal(|t| selected_text(&t.full(), &sel));
            if !text.is_empty() {
                ctx.copy_text(text);
            }
        }
    }

    /// Queue locally-typed `bytes` to this pane's shell and record them for
    /// broadcast fan-out, then snap the view back to live. Recording here (not
    /// in [`Self::write_input`]) is what makes the *typed* bytes — and only
    /// those — the source the multiplexer replays to grouped panes.
    fn send(&mut self, bytes: &[u8]) {
        self.input_echo.extend_from_slice(bytes);
        self.write_input(bytes);
    }

    /// Write `bytes` to the PTY and snap to live — the shared tail of local
    /// typing ([`Self::send`]) and broadcast fan-out ([`Self::feed_broadcast`]).
    /// A dead session refuses input; the ended chip already tells that story,
    /// so the error is deliberately dropped here.
    fn write_input(&mut self, bytes: &[u8]) {
        self.scroll_offset = 0;
        self.scroll_accum = 0.0;
        let _ = self.session.send_input(bytes);
    }

    /// Write synthesized bytes straight to the backing, bypassing the
    /// scroll-snap and the broadcast echo — the path for SGR mouse reports
    /// (TERM-13), which must not disturb the scrollback view or fan out to
    /// grouped panes (each pane owns its own pointer).
    fn write_raw(&self, bytes: &[u8]) {
        let _ = self.session.send_input(bytes);
    }

    /// Take this frame's locally-typed bytes for broadcast fan-out. The pane
    /// has already sent them to its own shell; this hands the multiplexer a
    /// copy to replay into the other panes of the broadcasting set (TERM-6).
    #[must_use]
    pub fn take_input_echo(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.input_echo)
    }

    /// Replay broadcast `bytes` (another pane's typing) into this pane's shell
    /// through the identical [`LocalPty`] write path local input uses (§6 —
    /// this widget still owns every PTY write). Not re-recorded into the echo,
    /// so a fan-out can never re-fan.
    pub fn feed_broadcast(&mut self, bytes: &[u8]) {
        self.write_input(bytes);
    }
}

/// The right-aligned title-strip badge for a pane's watch mode (TERM-12), or
/// `None` when unwatched. `Style` tokens (§4): accent for activity, warn for
/// silence.
const fn watch_badge(mode: WatchMode) -> Option<(&'static str, egui::Color32)> {
    match mode {
        WatchMode::Off => None,
        WatchMode::Activity => Some(("watch: activity", Style::ACCENT)),
        WatchMode::Silence => Some(("watch: silence", Style::WARN)),
    }
}

// ── Pure geometry / encoding folds (unit-tested without a UI) ───────────────

/// Grid dimensions for an available rect: floor division by the cell metrics,
/// at least 1×1. A milli-cell epsilon absorbs f32 ratio noise so a rect sized
/// for exactly N cells yields N (960.0/9.6 is 99.999992…, not 100).
fn grid_size(avail: Vec2, cell: Vec2) -> (u16, u16) {
    // Floored non-negative ratios bounded far below u16::MAX by real window
    // sizes; the saturating cast is the clamp.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let dim =
        |span: f32, unit: f32| (((span / unit.max(1.0)) + 1e-3).floor().max(1.0) as u16).max(1);
    (dim(avail.x, cell.x), dim(avail.y, cell.y))
}

/// The window-local `(row, col)` under a pointer position, clamped into the
/// grid (drags may leave the rect).
fn cell_at(origin: Pos2, cell: Vec2, pos: Pos2, cols: usize, rows: usize) -> (usize, usize) {
    // Non-negative after the max(0.0); magnitudes bounded by the grid clamp.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let axis = |p: f32, o: f32, unit: f32, limit: usize| {
        (((p - o) / unit.max(1.0)).floor().max(0.0) as usize).min(limit.saturating_sub(1))
    };
    (
        axis(pos.y, origin.y, cell.y, rows),
        axis(pos.x, origin.x, cell.x, cols),
    )
}

/// The xterm modifier parameter: `1 + shift(1) + alt(2) + ctrl(4)`.
const fn mod_code(m: Modifiers) -> u8 {
    1 + (m.shift as u8) + 2 * (m.alt as u8) + 4 * (m.ctrl as u8)
}

/// A CSI cursor-key sequence: `ESC [ <letter>` bare, `ESC [ 1;<mod> <letter>`
/// modified.
fn csi_letter(letter: u8, m: Modifiers) -> Vec<u8> {
    let code = mod_code(m);
    if code == 1 {
        vec![0x1b, b'[', letter]
    } else {
        let mut seq = format!("\x1b[1;{code}").into_bytes();
        seq.push(letter);
        seq
    }
}

/// A CSI tilde sequence: `ESC [ <num> ~` bare, `ESC [ <num>;<mod> ~` modified.
fn csi_tilde(num: u8, m: Modifiers) -> Vec<u8> {
    let code = mod_code(m);
    if code == 1 {
        format!("\x1b[{num}~").into_bytes()
    } else {
        format!("\x1b[{num};{code}~").into_bytes()
    }
}

/// An SS3 function-key sequence (`ESC O <letter>`), which xterm switches to
/// the CSI form when modified.
fn ss3_or_csi(letter: u8, m: Modifiers) -> Vec<u8> {
    if mod_code(m) == 1 {
        vec![0x1b, b'O', letter]
    } else {
        csi_letter(letter, m)
    }
}

/// The control byte for `Ctrl+<key>`, per the ASCII control-plane layout.
const fn ctrl_byte(key: Key) -> Option<u8> {
    Some(match key {
        Key::A => 0x01,
        Key::B => 0x02,
        Key::C => 0x03,
        Key::D => 0x04,
        Key::E => 0x05,
        Key::F => 0x06,
        Key::G => 0x07,
        Key::H => 0x08,
        Key::I => 0x09,
        Key::J => 0x0a,
        Key::K => 0x0b,
        Key::L => 0x0c,
        Key::M => 0x0d,
        Key::N => 0x0e,
        Key::O => 0x0f,
        Key::P => 0x10,
        Key::Q => 0x11,
        Key::R => 0x12,
        Key::S => 0x13,
        Key::T => 0x14,
        Key::U => 0x15,
        Key::V => 0x16,
        Key::W => 0x17,
        Key::X => 0x18,
        Key::Y => 0x19,
        Key::Z => 0x1a,
        Key::Space => 0x00,
        Key::OpenBracket => 0x1b,
        Key::Backslash => 0x1c,
        Key::CloseBracket => 0x1d,
        Key::Slash => 0x1f,
        _ => return None,
    })
}

/// Encode a non-text key press as the byte sequence an xterm sends.
///
/// Printable keys return `None` — their bytes arrive via `Event::Text`, so
/// encoding them here would double-send. Ctrl-chords return their control
/// bytes (the backend suppresses Text while Ctrl is held).
fn encode_key(key: Key, m: Modifiers) -> Option<Vec<u8>> {
    let seq = match key {
        Key::Enter => b"\r".to_vec(),
        // Shift+Tab is the CSI back-tab.
        Key::Tab => {
            if m.shift {
                b"\x1b[Z".to_vec()
            } else {
                b"\t".to_vec()
            }
        }
        // DEL — the xterm default (kbs=^?).
        Key::Backspace => vec![0x7f],
        Key::Escape => vec![0x1b],
        Key::ArrowUp => csi_letter(b'A', m),
        Key::ArrowDown => csi_letter(b'B', m),
        Key::ArrowRight => csi_letter(b'C', m),
        Key::ArrowLeft => csi_letter(b'D', m),
        Key::Home => csi_letter(b'H', m),
        Key::End => csi_letter(b'F', m),
        Key::Insert => csi_tilde(2, m),
        Key::Delete => csi_tilde(3, m),
        Key::PageUp => csi_tilde(5, m),
        Key::PageDown => csi_tilde(6, m),
        Key::F1 => ss3_or_csi(b'P', m),
        Key::F2 => ss3_or_csi(b'Q', m),
        Key::F3 => ss3_or_csi(b'R', m),
        Key::F4 => ss3_or_csi(b'S', m),
        Key::F5 => csi_tilde(15, m),
        Key::F6 => csi_tilde(17, m),
        Key::F7 => csi_tilde(18, m),
        Key::F8 => csi_tilde(19, m),
        Key::F9 => csi_tilde(20, m),
        Key::F10 => csi_tilde(21, m),
        Key::F11 => csi_tilde(23, m),
        Key::F12 => csi_tilde(24, m),
        _ if m.ctrl && !m.alt => vec![ctrl_byte(key)?],
        _ => return None,
    };
    Some(seq)
}

/// Pasted text as PTY input: newlines become carriage returns, the byte a
/// terminal's Enter sends (bracketed paste is a TERM-13 refinement).
fn paste_bytes(text: &str) -> Vec<u8> {
    text.replace("\r\n", "\n").replace('\n', "\r").into_bytes()
}

/// One page of scrollback travel: a viewport height less one line of overlap.
fn page_delta(rows: usize) -> i64 {
    i64::try_from(rows.saturating_sub(1).max(1)).unwrap_or(i64::MAX)
}

/// The selection's text from a full snapshot, rows joined with `\n`. Rows cut
/// at the right edge are trimmed of trailing blanks (the padding cells are
/// grid artifacts, not content); an explicit partial span keeps its spaces.
fn selected_text(screen: &Screen, sel: &Selection) -> String {
    let (start, end) = sel.bounds();
    let mut lines = Vec::new();
    for row in start.row..=end.row.min(screen.rows().saturating_sub(1)) {
        let Some((a, b)) = sel.row_span(row, screen.cols()) else {
            continue;
        };
        let Some(cells) = screen.row(row) else {
            continue;
        };
        let text: String = cells[a..b].iter().map(|c| c.ch).collect();
        if b == screen.cols() {
            lines.push(text.trim_end().to_owned());
        } else {
            lines.push(text);
        }
    }
    lines.join("\n")
}

/// The ~500 ms blink phase for a monotonically growing time.
fn blink_on(time: f64) -> bool {
    (time / BLINK_HALF_PERIOD).rem_euclid(2.0) < 1.0
}

/// A window row's glyphs as a `char` slice — the input the smart-selection and
/// launch-detection folds ([`crate::smart`]) read (one cell = one column).
fn row_chars(screen: &Screen, row: usize) -> Vec<char> {
    screen
        .row(row)
        .map(|cells| cells.iter().map(|c| c.ch).collect())
        .unwrap_or_default()
}

/// The scrollback offset that brings absolute `row` into the viewport with a
/// quarter-window of context above it. `0` keeps the live view; a deep history
/// row scrolls up. Used to follow the current search match (TERM-9).
fn scroll_for_row(row: usize, history: usize, rows: usize) -> usize {
    let top = row.saturating_sub(rows / 4);
    history.saturating_sub(top).min(history)
}

// ── The paint pass ──────────────────────────────────────────────────────────

/// The style identity of a run: cells with equal keys batch into one galley.
#[derive(PartialEq)]
struct RunStyle {
    fg: egui::Color32,
    bg: egui::Color32,
    italic: bool,
    underline: bool,
    strikeout: bool,
}

impl RunStyle {
    fn of(cell: &Cell, palette: &Palette) -> Self {
        let (fg, bg) = palette::cell_colors(cell, palette);
        Self {
            fg,
            bg,
            italic: cell.attrs.italic,
            underline: cell.attrs.underline,
            strikeout: cell.attrs.strikeout,
        }
    }

    /// True for a cell that paints nothing (default-bg blank, no decoration)
    /// — trailing runs of these are skipped entirely. `default_bg` is the active
    /// palette's background (the grid fill), so a preset's blanks trim too.
    fn is_blank(&self, ch: char, default_bg: egui::Color32) -> bool {
        ch == ' ' && self.bg == default_bg && !self.underline && !self.strikeout
    }
}

/// The pixel rect spanning `width` cells at `(row, col)` of a grid rooted at
/// `origin`.
#[allow(clippy::cast_precision_loss)] // rows/cols are u16-bounded grid indices.
fn cell_span_rect(origin: Pos2, cell: Vec2, row: usize, col: usize, width: usize) -> Rect {
    Rect::from_min_size(
        Pos2::new(
            (col as f32).mul_add(cell.x, origin.x),
            (row as f32).mul_add(cell.y, origin.y),
        ),
        Vec2::new(width as f32 * cell.x, cell.y),
    )
}

/// Paint one screen window into `rect`. Free of widget state so the headless
/// render tests drive the exact production paint path.
fn paint_grid(painter: &egui::Painter, rect: Rect, screen: &Screen, spec: &PaintSpec) {
    // The grid base is the active scheme's background (content, TERM-11); the
    // chrome painted over it below stays `Style` tokens.
    painter.rect_filled(rect, 0.0, spec.palette.bg);

    for row in 0..screen.rows() {
        if let Some(cells) = screen.row(row) {
            paint_row(painter, rect.min, spec, row, cells);
        }
    }

    // Search-match underlay (TERM-9): every hit in the WARN token, the current
    // one brighter — the same token-blend discipline the selection overlay uses.
    for hit in &spec.search_hits {
        let tone = if hit.current {
            Style::WARN.gamma_multiply(0.55)
        } else {
            Style::WARN.gamma_multiply(0.28)
        };
        painter.rect_filled(
            cell_span_rect(rect.min, spec.cell, hit.row, hit.col, hit.len),
            0.0,
            tone,
        );
    }

    // Selection overlay — the same token blend `Style::install` uses for
    // egui's own text selection, so highlights read identically platform-wide.
    if let Some(sel) = &spec.selection {
        for row in 0..screen.rows() {
            if let Some((a, b)) = sel.row_span(spec.first_abs + row, screen.cols()) {
                painter.rect_filled(
                    cell_span_rect(rect.min, spec.cell, row, a, b - a),
                    0.0,
                    Style::ACCENT.gamma_multiply(0.35),
                );
            }
        }
    }

    paint_cursor(painter, rect.min, screen, spec);

    // Chrome chips (pure Style tokens): the node marker (remote pane), the
    // scrollback position, and the honest status note.
    if let Some(node) = &spec.node {
        chip(
            painter,
            Pos2::new(rect.min.x + Style::SP_S, rect.min.y + Style::SP_S),
            Align2::LEFT_TOP,
            &format!("\u{2325} {node}"),
            Style::ACCENT,
        );
    }
    if spec.scrolled > 0 {
        chip(
            painter,
            Pos2::new(rect.max.x - Style::SP_S, rect.min.y + Style::SP_S),
            Align2::RIGHT_TOP,
            &format!("+{} lines", spec.scrolled),
            Style::TEXT_DIM,
        );
    }
    if let Some((text, color)) = &spec.note {
        chip(painter, rect.center(), Align2::CENTER_CENTER, text, *color);
    }
    // The search overlay chip sits bottom-left (TERM-9), out of the way of the
    // scrollback-position chip top-right.
    if let Some(bar) = &spec.search_bar {
        chip(
            painter,
            Pos2::new(rect.min.x + Style::SP_S, rect.max.y - Style::SP_S),
            Align2::LEFT_BOTTOM,
            &bar.label,
            bar.tone,
        );
    }
}

/// Paint one row as batched same-style runs: one bg rect + one galley per run
/// (never a galley per cell), with the trailing default-blank tail trimmed.
fn paint_row(painter: &egui::Painter, origin: Pos2, spec: &PaintSpec, row: usize, cells: &[Cell]) {
    let default_bg = spec.palette.bg;
    let mut end = cells.len();
    while end > 0
        && RunStyle::of(&cells[end - 1], &spec.palette).is_blank(cells[end - 1].ch, default_bg)
    {
        end -= 1;
    }
    let mut col = 0;
    while col < end {
        let style = RunStyle::of(&cells[col], &spec.palette);
        let mut run_end = col + 1;
        while run_end < end && RunStyle::of(&cells[run_end], &spec.palette) == style {
            run_end += 1;
        }
        let run = cell_span_rect(origin, spec.cell, row, col, run_end - col);
        if style.bg != default_bg {
            painter.rect_filled(run, 0.0, style.bg);
        }
        let text: String = cells[col..run_end].iter().map(|c| c.ch).collect();
        // All-blank runs inside a line only need their bg rect.
        if !text.trim_end().is_empty() || style.underline || style.strikeout {
            let mut format = TextFormat {
                font_id: spec.font_id.clone(),
                color: style.fg,
                italics: style.italic,
                ..TextFormat::default()
            };
            if style.underline {
                format.underline = Stroke::new(1.0, style.fg);
            }
            if style.strikeout {
                format.strikethrough = Stroke::new(1.0, style.fg);
            }
            let galley = painter.layout_job(LayoutJob::single_section(text, format));
            painter.galley(run.min, galley, style.fg);
        }
        col = run_end;
    }
}

/// The cursor, in the active scheme's cursor colour (TERM-11 content carve-out):
/// the configured shape filled — a full block repaints the glyph over it in the
/// palette bg — when focused, or a hollow block outline when not.
fn paint_cursor(painter: &egui::Painter, origin: Pos2, screen: &Screen, spec: &PaintSpec) {
    let cur = screen.cursor();
    let cols = screen.cols();
    if spec.cursor == CursorPaint::Hidden || cur.row >= screen.rows() || cols == 0 {
        return;
    }
    let col = cur.col.min(cols - 1);
    let block = cell_span_rect(origin, spec.cell, cur.row, col, 1);
    let cursor_color = spec.palette.cursor;
    match spec.cursor {
        CursorPaint::Filled => {
            let shape = spec.cursor_shape.rect(block);
            painter.rect_filled(shape, 0.0, cursor_color);
            // Only a full-cell block sits over the glyph, so only it repaints the
            // glyph (in the palette bg) to stay legible; a bar/underline leaves
            // the glyph untouched.
            let ch = screen.cell(cur.row, col).map_or(' ', |c| c.ch);
            if spec.cursor_shape == CursorShape::Block && ch != ' ' {
                let galley = painter.layout_job(LayoutJob::single_section(
                    ch.to_string(),
                    TextFormat {
                        font_id: spec.font_id.clone(),
                        color: spec.palette.bg,
                        ..TextFormat::default()
                    },
                ));
                painter.galley(block.min, galley, spec.palette.bg);
            }
        }
        CursorPaint::Hollow => {
            painter.rect_stroke(
                block,
                0.0,
                Stroke::new(1.0, cursor_color),
                StrokeKind::Inside,
            );
        }
        CursorPaint::Hidden => {}
    }
}

/// A small status chip: SURFACE plate, hairline border, dimmed label.
/// Crate-shared: the split surface (TERM-4) paints its zoom/error chips
/// through the same primitive, so all terminal chrome chips match.
pub(crate) fn chip(
    painter: &egui::Painter,
    at: Pos2,
    anchor: Align2,
    label: &str,
    color: egui::Color32,
) {
    let galley = painter.layout_no_wrap(label.to_owned(), FontId::monospace(Style::SMALL), color);
    let text_rect = anchor.anchor_size(at, galley.size() + Vec2::splat(2.0 * Style::SP_XS));
    painter.rect_filled(text_rect, Style::RADIUS, Style::SURFACE);
    painter.rect_stroke(
        text_rect,
        Style::RADIUS,
        Stroke::new(1.0, Style::BORDER),
        StrokeKind::Inside,
    );
    painter.galley(text_rect.min + Vec2::splat(Style::SP_XS), galley, color);
}

#[cfg(test)]
mod tests {
    use mde_egui::egui::{pos2, vec2, RawInput};

    use super::*;
    use crate::engine::Terminal;
    use crate::pty::SpawnOptions;
    use crate::screen::CursorPos;

    // ── cols/rows-from-rect math ────────────────────────────────────────────

    #[test]
    fn grid_size_floors_the_rect_by_the_cell_metrics() {
        assert_eq!(grid_size(vec2(960.0, 600.0), vec2(9.6, 20.0)), (100, 30));
        // Partial cells don't count.
        assert_eq!(grid_size(vec2(959.9, 619.9), vec2(9.6, 20.0)), (99, 30));
        // Never below 1×1, and degenerate cell metrics can't divide by zero.
        assert_eq!(grid_size(vec2(3.0, 2.0), vec2(9.6, 20.0)), (1, 1));
        assert_eq!(grid_size(vec2(100.0, 100.0), vec2(0.0, 0.0)), (100, 100));
    }

    #[test]
    fn cell_at_quantises_and_clamps_pointer_positions() {
        let origin = pos2(100.0, 50.0);
        let cell = vec2(10.0, 20.0);
        assert_eq!(cell_at(origin, cell, pos2(100.0, 50.0), 80, 24), (0, 0));
        assert_eq!(cell_at(origin, cell, pos2(163.9, 129.9), 80, 24), (3, 6));
        // Outside the rect clamps into the grid (drags escape the widget).
        assert_eq!(
            cell_at(origin, cell, pos2(9999.0, 9999.0), 80, 24),
            (23, 79)
        );
        assert_eq!(cell_at(origin, cell, pos2(-5.0, -5.0), 80, 24), (0, 0));
    }

    // ── key → escape-sequence folds ─────────────────────────────────────────

    #[test]
    fn editing_keys_encode_their_terminal_bytes() {
        let none = Modifiers::NONE;
        assert_eq!(encode_key(Key::Enter, none), Some(b"\r".to_vec()));
        assert_eq!(encode_key(Key::Tab, none), Some(b"\t".to_vec()));
        assert_eq!(
            encode_key(Key::Tab, Modifiers::SHIFT),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(encode_key(Key::Backspace, none), Some(vec![0x7f]));
        assert_eq!(encode_key(Key::Escape, none), Some(vec![0x1b]));
    }

    #[test]
    fn cursor_keys_encode_xterm_csi_with_modifier_params() {
        let none = Modifiers::NONE;
        assert_eq!(encode_key(Key::ArrowUp, none), Some(b"\x1b[A".to_vec()));
        assert_eq!(encode_key(Key::ArrowLeft, none), Some(b"\x1b[D".to_vec()));
        assert_eq!(encode_key(Key::Home, none), Some(b"\x1b[H".to_vec()));
        assert_eq!(encode_key(Key::End, none), Some(b"\x1b[F".to_vec()));
        // Ctrl+Right — word motion in every readline: CSI 1;5C.
        assert_eq!(
            encode_key(Key::ArrowRight, Modifiers::CTRL),
            Some(b"\x1b[1;5C".to_vec())
        );
        // Shift+Alt+Up = 1 + 1 + 2 → parameter 4.
        assert_eq!(
            encode_key(Key::ArrowUp, Modifiers::SHIFT | Modifiers::ALT),
            Some(b"\x1b[1;4A".to_vec())
        );
    }

    #[test]
    fn paging_and_function_keys_encode_their_sequences() {
        let none = Modifiers::NONE;
        assert_eq!(encode_key(Key::PageUp, none), Some(b"\x1b[5~".to_vec()));
        assert_eq!(encode_key(Key::PageDown, none), Some(b"\x1b[6~".to_vec()));
        assert_eq!(encode_key(Key::Insert, none), Some(b"\x1b[2~".to_vec()));
        assert_eq!(encode_key(Key::Delete, none), Some(b"\x1b[3~".to_vec()));
        assert_eq!(
            encode_key(Key::Delete, Modifiers::CTRL),
            Some(b"\x1b[3;5~".to_vec())
        );
        assert_eq!(encode_key(Key::F1, none), Some(b"\x1bOP".to_vec()));
        assert_eq!(
            encode_key(Key::F1, Modifiers::CTRL),
            Some(b"\x1b[1;5P".to_vec())
        );
        assert_eq!(encode_key(Key::F5, none), Some(b"\x1b[15~".to_vec()));
        assert_eq!(encode_key(Key::F12, none), Some(b"\x1b[24~".to_vec()));
    }

    #[test]
    fn ctrl_chords_encode_control_bytes_and_plain_letters_stay_text() {
        assert_eq!(encode_key(Key::A, Modifiers::CTRL), Some(vec![0x01]));
        assert_eq!(encode_key(Key::C, Modifiers::CTRL), Some(vec![0x03]));
        assert_eq!(encode_key(Key::Z, Modifiers::CTRL), Some(vec![0x1a]));
        assert_eq!(encode_key(Key::Space, Modifiers::CTRL), Some(vec![0x00]));
        assert_eq!(
            encode_key(Key::OpenBracket, Modifiers::CTRL),
            Some(vec![0x1b])
        );
        // Printables without Ctrl arrive as Text events — encoding them here
        // would double-send.
        assert_eq!(encode_key(Key::A, Modifiers::NONE), None);
        assert_eq!(encode_key(Key::A, Modifiers::SHIFT), None);
    }

    #[test]
    fn paste_translates_newlines_to_carriage_returns() {
        assert_eq!(paste_bytes("ls -la\n"), b"ls -la\r".to_vec());
        assert_eq!(paste_bytes("a\r\nb\nc"), b"a\rb\rc".to_vec());
    }

    // ── selection math ──────────────────────────────────────────────────────

    fn sel(a: (usize, usize), h: (usize, usize)) -> Selection {
        Selection {
            anchor: CellPos { row: a.0, col: a.1 },
            head: CellPos { row: h.0, col: h.1 },
        }
    }

    #[test]
    fn row_spans_follow_reading_order_regardless_of_drag_direction() {
        let s = sel((0, 3), (2, 1));
        assert_eq!(s.row_span(0, 10), Some((3, 10)));
        assert_eq!(s.row_span(1, 10), Some((0, 10)));
        assert_eq!(s.row_span(2, 10), Some((0, 2))); // head cell inclusive
        assert_eq!(s.row_span(3, 10), None);
        // A backwards drag selects the identical range.
        let r = sel((2, 1), (0, 3));
        for row in 0..4 {
            assert_eq!(r.row_span(row, 10), s.row_span(row, 10));
        }
        // Single-cell selection.
        assert_eq!(sel((1, 4), (1, 4)).row_span(1, 10), Some((4, 5)));
    }

    #[test]
    fn selected_text_reads_the_fed_grid() {
        let mut term = Terminal::new(10, 3, 100);
        term.feed(b"hello\r\nworld\r\nmesh");
        let full = term.full();
        // (0,1) → (1,2): the tail of "hello", then "wor" (head-inclusive).
        assert_eq!(selected_text(&full, &sel((0, 1), (1, 2))), "ello\nwor");
        // Dragged backwards — same text.
        assert_eq!(selected_text(&full, &sel((1, 2), (0, 1))), "ello\nwor");
        // A single row, exact span keeps its shape.
        assert_eq!(selected_text(&full, &sel((2, 0), (2, 3))), "mesh");
        // Full-width rows trim their padding-cell tails.
        assert_eq!(selected_text(&full, &sel((0, 0), (1, 9))), "hello\nworld");
    }

    // ── headless render: fed grid → real draw primitives ───────────────────

    /// Run the production paint path headless (CPU tessellation, no GPU) and
    /// return every mesh vertex colour, so tests can assert the palette and
    /// attrs actually reached the draw stream.
    fn tessellate_colors(
        screen: &Screen,
        spec_of: impl Fn(FontId, Vec2) -> PaintSpec,
    ) -> Vec<egui::Color32> {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 500.0))),
            ..RawInput::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let font_id = FontId::monospace(Style::BODY);
                let cell =
                    ui.fonts(|f| Vec2::new(f.glyph_width(&font_id, 'M'), f.row_height(&font_id)));
                let rect = ui.available_rect_before_wrap();
                paint_grid(&ui.painter_at(rect), rect, screen, &spec_of(font_id, cell));
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "the paint pass produced no primitives");
        prims
            .into_iter()
            .filter_map(|p| match p.primitive {
                egui::epaint::Primitive::Mesh(mesh) => Some(mesh),
                egui::epaint::Primitive::Callback(_) => None,
            })
            .flat_map(|m| m.vertices.into_iter().map(|v| v.color))
            .collect()
    }

    fn plain_spec(font_id: FontId, cell: Vec2) -> PaintSpec {
        PaintSpec {
            font_id,
            cell,
            palette: Palette::from_tokens(),
            cursor_shape: CursorShape::Block,
            first_abs: 0,
            selection: None,
            cursor: CursorPaint::Filled,
            scrolled: 0,
            node: None,
            note: None,
            search_hits: Vec::new(),
            search_bar: None,
        }
    }

    #[test]
    fn fed_grid_renders_palette_colors_and_attrs_into_primitives() {
        let mut term = Terminal::new(40, 5, 100);
        // Red fg, blue bg run, bold+italic+underline text, a truecolor bg.
        term.feed(b"\x1b[31mred\x1b[0m \x1b[44mblue-bg\x1b[0m\r\n");
        term.feed(b"\x1b[1;3;4mBIU\x1b[0m \x1b[48;2;9;87;153mtc\x1b[0m");
        let screen = term.viewport();

        let colors = tessellate_colors(&screen, plain_spec);
        let has = |c: egui::Color32| colors.contains(&c);

        // The content palette reached the vertices: red glyphs, the blue bg
        // rect, and the pass-through truecolor bg (lock 13: Rgb cells render
        // today). The truecolor expectation is derived from the engine-parsed
        // cell, so fed SGR bytes → Rgb cell → vertex colour is asserted end to
        // end with no literal in between.
        assert!(has(palette::RED), "palette red glyph colour in the mesh");
        assert!(has(palette::BLUE), "palette blue bg rect in the mesh");
        let tc = screen.cell(1, 4).expect("the truecolor cell");
        assert!(
            matches!(tc.bg, crate::screen::CellColor::Rgb(..)),
            "engine kept the 24-bit bg"
        );
        assert!(
            has(palette::cell_colors(tc, &Palette::from_tokens()).1),
            "truecolor bg rect"
        );
        // The default scheme's roles are the chrome tokens: grid base = BG,
        // block cursor = the cursor colour (TEXT).
        assert!(has(Style::BG), "background fill");
        assert!(has(Style::TEXT), "block cursor fill");
    }

    #[test]
    fn truecolor_and_256color_render_unquantized_into_primitives() {
        // TERM-13 acceptance: 24-bit true-colour and 256-colour reach the draw
        // stream exactly — no quantization to a nearby palette slot.
        let mut term = Terminal::new(20, 2, 100);
        // A 24-bit fg + bg whose channels match no ANSI/cube slot, then a
        // 256-colour cube index on the second row.
        term.feed(b"\x1b[38;2;17;133;219m\x1b[48;2;201;42;99mTC\x1b[0m\r\n");
        term.feed(b"\x1b[38;5;208mIDX\x1b[0m");
        let screen = term.viewport();

        // The engine kept the 24-bit values as `Rgb` cells (not a palette slot).
        let tc = screen.cell(0, 0).expect("truecolor cell");
        assert_eq!(tc.fg, crate::screen::CellColor::Rgb(17, 133, 219));
        assert_eq!(tc.bg, crate::screen::CellColor::Rgb(201, 42, 99));

        let colors = tessellate_colors(&screen, plain_spec);
        let has = |c: egui::Color32| colors.contains(&c);
        // The exact 24-bit fg glyph + bg rect colours land in the mesh vertices.
        assert!(
            has(egui::Color32::from_rgb(17, 133, 219)),
            "24-bit fg exact"
        );
        assert!(has(egui::Color32::from_rgb(201, 42, 99)), "24-bit bg exact");
        // The 256-colour cube slot 208 resolves to its faithful xterm RGB.
        assert!(
            has(palette::indexed(208)),
            "256-colour slot 208 rendered faithfully"
        );
    }

    #[test]
    fn selection_scrollback_chip_and_ended_chip_render() {
        let mut term = Terminal::new(20, 3, 100);
        term.feed(b"one\r\ntwo\r\nthree");
        let screen = term.viewport();
        let colors = tessellate_colors(&screen, |font_id, cell| PaintSpec {
            font_id,
            cell,
            palette: Palette::from_tokens(),
            cursor_shape: CursorShape::Block,
            first_abs: 0,
            selection: Some(sel((0, 0), (1, 2))),
            cursor: CursorPaint::Hollow,
            scrolled: 7,
            node: Some("oak".to_string()),
            note: Some(("session ended".to_string(), Style::TEXT_DIM)),
            search_hits: Vec::new(),
            search_bar: None,
        });
        let has = |c: egui::Color32| colors.contains(&c);
        assert!(
            has(Style::ACCENT.gamma_multiply(0.35)),
            "selection overlay uses the token blend"
        );
        assert!(has(Style::SURFACE), "chip plate");
        assert!(has(Style::TEXT_DIM), "chip label");
        assert!(has(Style::ACCENT), "the remote node marker chip");
    }

    #[test]
    fn search_highlights_and_bar_render_through_tokens() {
        // TERM-9: the match underlay + overlay chip reach the draw stream as
        // pure `Style` tokens (the visual gate is lifted; tokens + tests suffice).
        let mut term = Terminal::new(20, 3, 100);
        term.feed(b"error here\r\nok\r\nmore error");
        let screen = term.viewport();
        let colors = tessellate_colors(&screen, |font_id, cell| PaintSpec {
            font_id,
            cell,
            palette: Palette::from_tokens(),
            cursor_shape: CursorShape::Block,
            first_abs: 0,
            selection: None,
            cursor: CursorPaint::Hidden,
            scrolled: 0,
            node: None,
            note: None,
            search_hits: vec![
                SearchHit {
                    row: 0,
                    col: 0,
                    len: 5,
                    current: true,
                },
                SearchHit {
                    row: 2,
                    col: 5,
                    len: 5,
                    current: false,
                },
            ],
            search_bar: Some(SearchBar {
                label: "find: error  1/2  [smart]".to_string(),
                tone: Style::ACCENT,
            }),
        });
        let has = |c: egui::Color32| colors.contains(&c);
        assert!(has(Style::WARN.gamma_multiply(0.55)), "current-match tone");
        assert!(has(Style::WARN.gamma_multiply(0.28)), "other-match tone");
        assert!(has(Style::SURFACE), "search-bar chip plate");
        assert!(has(Style::ACCENT), "search-bar accent label");
    }

    #[test]
    fn scroll_for_row_follows_a_match_with_context() {
        // history=100, rows=24 → a quarter-window (6 rows) of context above.
        assert_eq!(scroll_for_row(10, 100, 24), 96); // deep history, near top
        assert_eq!(scroll_for_row(2, 100, 24), 100); // top clamps to full depth
        assert_eq!(scroll_for_row(120, 100, 24), 0); // a live row keeps live view
    }

    #[test]
    fn empty_grid_tessellates_lean() {
        // The batching contract: an idle grid must not emit per-cell shapes.
        let term = Terminal::new(200, 60, 100);
        let screen = term.viewport();
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 960.0))),
            ..RawInput::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let font_id = FontId::monospace(Style::BODY);
                let cell =
                    ui.fonts(|f| Vec2::new(f.glyph_width(&font_id, 'M'), f.row_height(&font_id)));
                let rect = ui.available_rect_before_wrap();
                paint_grid(
                    &ui.painter_at(rect),
                    rect,
                    &screen,
                    &plain_spec(font_id, cell),
                );
            });
        });
        // 12k cells; a per-cell painter would emit thousands of shapes.
        assert!(
            out.shapes.len() < 50,
            "idle 200x60 grid should paint a handful of shapes, got {}",
            out.shapes.len()
        );
    }

    #[test]
    fn a_preset_palette_repaints_the_content_into_the_primitives() {
        use crate::presets::Preset;

        // TERM-11: selecting a preset must actually drive the renderer — the
        // scheme's 16 ANSI colours and its background must reach the draw stream,
        // and the default table must NOT (proving the preset applied, not chrome).
        let mut term = Terminal::new(20, 3, 100);
        term.feed(b"\x1b[31mred\x1b[0m \x1b[34mblue\x1b[0m");
        let screen = term.viewport();
        let nord = Preset::Nord.palette();

        let colors = tessellate_colors(&screen, |font_id, cell| PaintSpec {
            palette: nord,
            ..plain_spec(font_id, cell)
        });
        let has = |c: egui::Color32| colors.contains(&c);

        // The scheme's slot-1 (red) and slot-4 (blue) painted the glyph runs.
        assert!(has(nord.color(1)), "the preset's red reached the vertices");
        assert!(has(nord.color(4)), "the preset's blue reached the vertices");
        // The scheme's background filled the grid …
        assert!(has(nord.bg), "the preset background filled the grid");
        // … and its cursor colour drew the block cursor.
        assert!(has(nord.cursor), "the preset cursor colour drew the cursor");
        // The Quasar default's content red is gone — this is genuinely the
        // preset painting the grid, not the default table. (Style::BG can't be
        // used as a negative witness: the egui CentralPanel frame paints it
        // behind the grid regardless of the scheme.)
        assert!(
            !has(palette::RED),
            "the default red must not paint under a preset"
        );
    }

    #[test]
    fn the_cursor_style_knob_drives_the_paint_path() {
        // TERM-11: every cursor shape the knob offers paints a cursor through the
        // real paint path (the shape's geometry itself is asserted by
        // `CursorShape::rect`'s pure test in the appearance module).
        let mut term = Terminal::new(20, 3, 100);
        term.feed(b"x");
        let screen = term.viewport();
        let cursor = Palette::from_tokens().cursor;
        for shape in CursorShape::ALL {
            let colors = tessellate_colors(&screen, |font_id, cell| PaintSpec {
                cursor_shape: shape,
                ..plain_spec(font_id, cell)
            });
            assert!(
                colors.contains(&cursor),
                "the {} cursor reached the draw stream",
                shape.label()
            );
        }
    }

    #[test]
    fn the_font_size_knob_changes_the_grid_density() {
        // TERM-11: the font-size knob drives the widget's cell metrics, so a
        // larger font yields a coarser grid over the same rect (the sizing path
        // the live widget uses each frame).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut small_cell = Vec2::ZERO;
        let mut big_cell = Vec2::ZERO;
        let input = || RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 500.0))),
            ..RawInput::default()
        };
        // Two frames: the font atlas warms on the first, so the second measures
        // real glyph metrics.
        for _ in 0..2 {
            let _ = ctx.run(input(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.fonts(|f| {
                        let small = FontId::monospace(Style::BODY);
                        let big = FontId::monospace(Style::BODY * 2.0);
                        small_cell = Vec2::new(f.glyph_width(&small, 'M'), f.row_height(&small));
                        big_cell = Vec2::new(f.glyph_width(&big, 'M'), f.row_height(&big));
                    });
                });
            });
        }
        assert!(
            big_cell.x > small_cell.x && big_cell.y > small_cell.y,
            "bigger font, bigger cell"
        );
        let avail = vec2(900.0, 500.0);
        let (small_cols, small_rows) = grid_size(avail, small_cell);
        let (big_cols, big_rows) = grid_size(avail, big_cell);
        assert!(
            big_cols < small_cols && big_rows < small_rows,
            "a larger font packs fewer cells: {big_cols}x{big_rows} vs {small_cols}x{small_rows}"
        );
    }

    // ── the full widget over a real PTY, headless ───────────────────────────

    #[test]
    fn widget_show_sizes_the_pty_from_the_rect_and_tessellates() {
        let pty = LocalPty::spawn(SpawnOptions {
            shell: Some("/bin/sh".to_owned()),
            ..SpawnOptions::default()
        })
        .expect("spawn /bin/sh");
        let mut widget = TerminalWidget::new(pty);

        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = || RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 500.0))),
            ..RawInput::default()
        };
        // Two frames: fonts warm on the first; the second paints steady-state.
        let mut prim_count = 0;
        for _ in 0..2 {
            let out = ctx.run(input(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    widget.show(ui);
                });
            });
            prim_count = ctx.tessellate(out.shapes, out.pixels_per_point).len();
        }
        assert!(prim_count > 0, "widget frame produced draw primitives");

        // The resize mapped the rect to a real grid on both the engine and
        // the kernel side (§7: runtime-observable, not a mock).
        let (cols, rows) = widget.with_terminal(|t| (t.cols(), t.rows()));
        assert!(
            cols > 40 && rows > 10,
            "grid resized to the rect: {cols}x{rows}"
        );
        assert_eq!(
            widget.last_grid,
            Some((
                u16::try_from(cols).expect("cols"),
                u16::try_from(rows).expect("rows")
            ))
        );
    }

    // ── TERM-12: per-pane title, watch → notify, bell → notify ──────────────

    /// A notify seam that records every raised notice (the recorder half of the
    /// injectable [`NotifyBus`], mirroring the launch-bus recorder idiom).
    #[derive(Default)]
    struct RecordingNotifier {
        notices: std::sync::Mutex<Vec<TermNotice>>,
    }

    impl NotifyBus for RecordingNotifier {
        fn notify(&self, notice: &TermNotice) -> Result<(), String> {
            self.notices
                .lock()
                .expect("notices lock")
                .push(notice.clone());
            Ok(())
        }
    }

    /// A headless widget over a remote pane with no Bus — no shell spawn, no
    /// threads. The activity watcher is driven with a synthetic output counter,
    /// so the backing produces nothing itself.
    fn headless_widget() -> TerminalWidget {
        let bus: Arc<dyn crate::remote::PtyBus> =
            Arc::new(crate::remote::BusPtyClient::with_root(None));
        let remote = RemotePty::open(bus, "oak", "oak", 80, 24);
        TerminalWidget::new_remote(remote)
    }

    fn key_press(key: Key) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }
    }

    #[test]
    fn a_pane_title_derives_from_the_command_and_takes_a_rename() {
        let mut w = headless_widget();
        assert_eq!(w.title_text(), "shell"); // the fallback ordinal
                                             // The running command's OSC title becomes the shown title.
        w.title.set_derived("vim README");
        assert_eq!(w.title_text(), "vim README");

        // A rename via the in-widget edit idiom (Text appends, Enter commits).
        w.begin_rename(); // seeds the buffer with the display
        w.rename_event(&Event::Text("!".to_owned()));
        w.rename_event(&key_press(Key::Enter));
        assert_eq!(w.title_text(), "vim README!");
        assert!(w.pane_title().is_overridden());

        // Escape abandons a rename, leaving the title untouched.
        w.begin_rename();
        w.rename_event(&Event::Text("junk".to_owned()));
        w.rename_event(&key_press(Key::Escape));
        assert_eq!(w.title_text(), "vim README!");
    }

    #[test]
    fn a_silence_watch_publishes_one_notice_through_the_seam() {
        let rec = Arc::new(RecordingNotifier::default());
        let mut w = headless_widget().with_notify_bus(rec.clone());
        w.toggle_silence_watch();
        assert_eq!(w.watch().mode(), WatchMode::Silence);

        // Baseline at t=0, then cross the (default 10s) window with no output.
        assert_eq!(w.tick_watch(0, 0.0), None);
        assert_eq!(w.tick_watch(0, 100.0), Some(WatchEvent::Silence));
        // It does not re-fire while it stays quiet.
        assert_eq!(w.tick_watch(0, 200.0), None);

        let notices = rec.notices.lock().expect("lock").clone();
        assert_eq!(notices.len(), 1, "one silence notice");
        assert!(
            notices[0].headline.contains("silence"),
            "headline: {}",
            notices[0].headline
        );
        assert_eq!(notices[0].level, NoticeLevel::Info);
    }

    #[test]
    fn an_activity_watch_publishes_on_output_after_quiet() {
        let rec = Arc::new(RecordingNotifier::default());
        let mut w = headless_widget().with_notify_bus(rec.clone());
        w.toggle_activity_watch();

        assert_eq!(w.tick_watch(0, 0.0), None); // baseline
                                                // Output after > 10s of quiet fires activity.
        assert_eq!(w.tick_watch(50, 30.0), Some(WatchEvent::Activity));
        // Continuous output does not re-fire.
        assert_eq!(w.tick_watch(60, 30.5), None);
        assert_eq!(rec.notices.lock().expect("lock").len(), 1);
    }

    #[test]
    fn an_audible_bell_publishes_but_a_visual_one_does_not() {
        let rec = Arc::new(RecordingNotifier::default());
        let mut w = headless_widget().with_notify_bus(rec.clone());

        // Visual-only (the default): a flash, no notice.
        w.set_bell_config(BellConfig::visual_only());
        w.ring_bell(1.0);
        assert!(rec.notices.lock().expect("lock").is_empty());
        assert!(w.bell.is_flashing(1.0));

        // Audible: a notice on the seam.
        w.set_bell_config(BellConfig::audible_only());
        w.ring_bell(2.0);
        let notices = rec.notices.lock().expect("lock").clone();
        assert_eq!(notices.len(), 1);
        assert!(notices[0].headline.contains("bell"));
        assert_eq!(notices[0].level, NoticeLevel::Warning);
    }

    #[test]
    fn cursor_position_tracks_the_engine() {
        // The cursor the widget paints is the engine's, not a fabrication.
        let mut term = Terminal::new(10, 2, 10);
        term.feed(b"ab");
        assert_eq!(term.viewport().cursor(), CursorPos { row: 0, col: 2 });
    }

    #[test]
    fn blink_phase_alternates() {
        assert!(blink_on(0.0));
        assert!(!blink_on(0.6));
        assert!(blink_on(1.1));
    }

    // ── TERM-15: selection context-menu action dispatch ─────────────────────

    /// Recording twins of the TERM-15 seams (the launch-bus recorder idiom).
    #[derive(Default)]
    struct RecLaunch {
        routes: std::sync::Mutex<Vec<smart::LaunchRoute>>,
    }
    impl LaunchBus for RecLaunch {
        fn open(&self, route: &smart::LaunchRoute) -> Result<(), String> {
            self.routes.lock().expect("lock").push(route.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecChat {
        sends: std::sync::Mutex<Vec<(String, String)>>,
    }
    impl ChatBus for RecChat {
        fn send(&self, to: &str, text: &str) -> Result<(), String> {
            self.sends
                .lock()
                .expect("lock")
                .push((to.to_string(), text.to_string()));
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecRunner {
        argvs: std::sync::Mutex<Vec<Vec<String>>>,
    }
    impl CommandRunner for RecRunner {
        fn run(&self, argv: &[String], _cwd: Option<&std::path::Path>) -> Result<(), String> {
            self.argvs.lock().expect("lock").push(argv.to_vec());
            Ok(())
        }
    }

    /// Each built-in mesh action dispatches the RIGHT existing verb/launch, and a
    /// custom command runs the selection-substituted argv — all headless over the
    /// injected recorders (no Bus, no process spawn, no grid read).
    #[test]
    fn each_menu_action_dispatches_its_reused_verb() {
        let launch = Arc::new(RecLaunch::default());
        let chat = Arc::new(RecChat::default());
        let runner = Arc::new(RecRunner::default());
        let menu = ContextMenu {
            commands: vec![crate::menu::CustomCommand::new("open", "xdg-open {}")],
            chat_recipient: "eagle".to_string(),
        };
        let mut w = headless_widget()
            .with_launch_bus(launch.clone())
            .with_chat_bus(chat.clone())
            .with_command_runner(runner.clone())
            .with_context_menu(menu);

        // open-path-in-Files → the TERM-9 Files surface-launch route.
        w.open_selection_in_files("/etc/hosts");
        // open-URL-in-mesh-browser → the TERM-9 Bookmarks route.
        w.open_selection_url("https://mesh.local");
        assert_eq!(
            launch.routes.lock().expect("lock").as_slice(),
            &[
                smart::LaunchRoute::Files("/etc/hosts".to_string()),
                smart::LaunchRoute::Bookmarks("https://mesh.local".to_string()),
            ]
        );

        // send-selection-to-Chat → the NOTIFY-CHAT send verb, to the config peer.
        w.send_selection_to_chat("build failed");
        assert_eq!(
            chat.sends.lock().expect("lock").as_slice(),
            &[("eagle".to_string(), "build failed".to_string())]
        );

        // custom command → the substituted argv (selection injected).
        let cmd = w.menu.commands[0].clone();
        w.run_custom_command(&cmd, "report.pdf");
        assert_eq!(
            runner.argvs.lock().expect("lock").as_slice(),
            &[vec!["xdg-open".to_string(), "report.pdf".to_string()]]
        );

        // new-terminal-here → the flag the split multiplexer drains (once).
        assert!(!w.take_new_terminal_here());
        w.new_terminal_here = true;
        assert!(w.take_new_terminal_here());
        assert!(!w.take_new_terminal_here());
    }
}
