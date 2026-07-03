//! The VT engine core — glue over `alacritty_terminal` (§6, no re-implemented
//! VT parsing).
//!
//! [`Terminal`] owns an `alacritty_terminal::Term` (the cell grid + the
//! soft-capped scrollback ring) and its ANSI/xterm `Processor` (the bundled
//! `vte` state machine). Callers [`feed`](Terminal::feed) PTY bytes in and read
//! a render-agnostic [`Screen`] out. Everything terminal-shaped — SGR, cursor
//! motion, clears, wrapping, tab stops, scroll-off into history — is handled by
//! the mature engine; this module only bridges its grid to [`crate::screen`].

use std::sync::{Arc, Mutex, PoisonError};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::{Cell as GridCell, Flags};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{Color, Processor};

use crate::screen::{Cell, CellAttrs, CellColor, CursorPos, Screen};

/// The default scrollback soft-cap (lines) when a caller doesn't specify one.
///
/// "Unlimited (soft-capped)" per the design lock (Q11): large enough to feel
/// unbounded in practice, bounded so a runaway `yes` can't exhaust memory.
pub const DEFAULT_SCROLLBACK: usize = 100_000;

/// A window-facing event the engine surfaced while parsing.
///
/// The small subset TERM-12 acts on (per-pane titles + the bell) — a local
/// mirror of the alacritty `Event`s we care about, so callers never depend on
/// the engine crate's enum.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TermEvent {
    /// The running program set the terminal title (OSC 0/2) — the auto-derived
    /// pane title (TERM-12).
    Title(String),
    /// The running program reset the title to its default.
    ResetTitle,
    /// The terminal rang the bell (`BEL`, `0x07`).
    Bell,
}

/// An `alacritty_terminal` event listener that records the [`TermEvent`]s
/// TERM-12 consumes (title + bell) into a shared queue the owning [`Terminal`]
/// drains each frame; every other engine event is ignored (there is no window
/// here to service clipboard/resize/wakeup requests).
#[derive(Clone, Default)]
struct EventSink {
    events: Arc<Mutex<Vec<TermEvent>>>,
}

impl EventListener for EventSink {
    fn send_event(&self, event: Event) {
        let mapped = match event {
            Event::Title(title) => TermEvent::Title(title),
            Event::ResetTitle => TermEvent::ResetTitle,
            Event::Bell => TermEvent::Bell,
            _ => return,
        };
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(mapped);
    }
}

/// A minimal [`Dimensions`] carrier for `Term::new`/`Term::resize`.
///
/// The engine owns the real scrollback sizing (via [`Config::scrolling_history`]);
/// this only reports the *visible* rectangle, so `total_lines == screen_lines`.
#[derive(Clone, Copy)]
struct GridDims {
    columns: usize,
    screen_lines: usize,
}

impl Dimensions for GridDims {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// A live terminal: a cell grid + soft-capped scrollback, fed ANSI/xterm bytes.
pub struct Terminal {
    term: Term<EventSink>,
    parser: Processor,
    /// The event queue the [`EventSink`] fills (title/bell), drained per frame.
    events: Arc<Mutex<Vec<TermEvent>>>,
    /// Monotonic count of bytes fed — the activity/silence watcher (TERM-12)
    /// samples this to tell "new output this frame" apart from a quiet pane.
    bytes_seen: u64,
}

impl Terminal {
    /// Open a terminal of `cols × rows` visible cells with `scrollback` lines of
    /// history. Dimensions are clamped to at least `1×1` so the engine always
    /// has a valid grid.
    #[must_use]
    pub fn new(cols: usize, rows: usize, scrollback: usize) -> Self {
        let config = Config {
            scrolling_history: scrollback,
            ..Config::default()
        };
        let dims = GridDims {
            columns: cols.max(1),
            screen_lines: rows.max(1),
        };
        let events = Arc::new(Mutex::new(Vec::new()));
        let term = Term::new(
            config,
            &dims,
            EventSink {
                events: Arc::clone(&events),
            },
        );
        Self {
            term,
            parser: Processor::new(),
            events,
            bytes_seen: 0,
        }
    }

    /// Open a terminal with the [`DEFAULT_SCROLLBACK`] soft-cap.
    #[must_use]
    pub fn with_default_scrollback(cols: usize, rows: usize) -> Self {
        Self::new(cols, rows, DEFAULT_SCROLLBACK)
    }

    /// Feed a run of PTY bytes through the ANSI/xterm parser, updating the grid.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.bytes_seen = self.bytes_seen.wrapping_add(bytes.len() as u64);
        for &byte in bytes {
            self.parser.advance(&mut self.term, byte);
        }
    }

    /// Drain the window events (title/bell) the parser surfaced since the last
    /// drain — the TERM-12 pane chrome reads these each frame.
    #[must_use]
    pub fn drain_events(&self) -> Vec<TermEvent> {
        std::mem::take(&mut *self.events.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// The monotonic count of bytes fed into the engine — the activity/silence
    /// watcher (TERM-12) folds the delta between frames.
    #[must_use]
    pub const fn bytes_seen(&self) -> u64 {
        self.bytes_seen
    }

    /// Resize the visible grid to `cols × rows` (clamped to at least `1×1`).
    /// Scrollback is preserved by the engine.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let dims = GridDims {
            columns: cols.max(1),
            screen_lines: rows.max(1),
        };
        self.term.resize(dims);
    }

    /// Number of scrollback (off-screen history) lines currently retained.
    #[must_use]
    pub fn scrollback_len(&self) -> usize {
        self.term.grid().history_size()
    }

    /// Visible columns.
    #[must_use]
    pub fn cols(&self) -> usize {
        self.term.grid().columns()
    }

    /// Visible rows.
    #[must_use]
    pub fn rows(&self) -> usize {
        self.term.grid().screen_lines()
    }

    /// Snapshot just the visible viewport as a [`Screen`].
    #[must_use]
    pub fn viewport(&self) -> Screen {
        self.snapshot(false)
    }

    /// Snapshot the scrollback history **and** the visible viewport as one
    /// [`Screen`] (history rows first). Used by scrollback search (TERM-9).
    #[must_use]
    pub fn full(&self) -> Screen {
        self.snapshot(true)
    }

    /// Snapshot a viewport-sized window whose top edge sits `offset` lines
    /// above the live viewport's top — the scrollback viewport (TERM-3).
    ///
    /// `window(0)` is exactly [`Self::viewport`]; `offset` clamps to the
    /// retained history. A renderer scrolled deep into a 100k-line history
    /// pays only O(rows × cols) per frame here, never the full-history copy
    /// of [`Self::full`].
    #[must_use]
    // offset ≤ history, far below i32::MAX.
    #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
    pub fn window(&self, offset: usize) -> Screen {
        let offset = offset.min(self.scrollback_len());
        self.snapshot_rows(Line(-(offset as i32)), self.rows())
    }

    // Grid indices are `i32`/`usize` bounded by the terminal dimensions (a few
    // thousand at most), so the row/column casts below cannot truncate or wrap.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss
    )]
    fn snapshot(&self, include_history: bool) -> Screen {
        let grid = self.term.grid();
        let top = if include_history {
            grid.topmost_line()
        } else {
            Line(0)
        };
        let bottom = grid.bottommost_line();
        let row_count = (bottom.0 - top.0 + 1).max(0) as usize;
        self.snapshot_rows(top, row_count)
    }

    // Same bounded-index casts as `snapshot`.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss
    )]
    fn snapshot_rows(&self, top: Line, row_count: usize) -> Screen {
        let grid = self.term.grid();
        let cols = grid.columns();

        let mut cells = Vec::with_capacity(row_count.saturating_mul(cols));
        for row in 0..row_count {
            let line = Line(top.0 + row as i32);
            for col in 0..cols {
                let src = &grid[Point::new(line, Column(col))];
                cells.push(convert_cell(src));
            }
        }

        // Rows from the top of THIS snapshot; a cursor below the window (deep
        // scrollback view) lands at `row >= row_count`, which renderers treat
        // as "not visible".
        let cursor_point = grid.cursor.point;
        let cursor = CursorPos {
            row: (cursor_point.line.0 - top.0).max(0) as usize,
            col: cursor_point.column.0,
        };

        Screen::new(cols, row_count, cells, cursor)
    }
}

/// Map one engine grid cell onto the render-agnostic [`Cell`].
const fn convert_cell(src: &GridCell) -> Cell {
    Cell {
        ch: src.c,
        fg: convert_color(src.fg),
        bg: convert_color(src.bg),
        attrs: convert_flags(src.flags),
    }
}

/// Normalise an engine colour into a [`CellColor`].
///
/// Named ANSI slots `0..=15` (and any other in-palette named slot) collapse to
/// [`CellColor::Palette`]; the special slots (`Foreground`/`Background`/cursor,
/// which sit at index `256+`) become [`CellColor::Default`] and are resolved by
/// the renderer from the cell's role.
#[allow(clippy::cast_possible_truncation)] // guarded by the `0..=255` arm.
const fn convert_color(color: Color) -> CellColor {
    match color {
        Color::Spec(rgb) => CellColor::Rgb(rgb.r, rgb.g, rgb.b),
        Color::Indexed(index) => CellColor::Palette(index),
        Color::Named(named) => match named as usize {
            slot @ 0..=255 => CellColor::Palette(slot as u8),
            _ => CellColor::Default,
        },
    }
}

/// Decode the engine cell flags into [`CellAttrs`].
const fn convert_flags(flags: Flags) -> CellAttrs {
    CellAttrs {
        bold: flags.contains(Flags::BOLD),
        dim: flags.contains(Flags::DIM),
        italic: flags.contains(Flags::ITALIC),
        underline: flags.contains(Flags::UNDERLINE),
        inverse: flags.contains(Flags::INVERSE),
        strikeout: flags.contains(Flags::STRIKEOUT),
        hidden: flags.contains(Flags::HIDDEN),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh terminal, feed `bytes`, return the viewport snapshot.
    fn run(cols: usize, rows: usize, bytes: &[u8]) -> Screen {
        let mut term = Terminal::new(cols, rows, 1000);
        term.feed(bytes);
        term.viewport()
    }

    fn ch(screen: &Screen, row: usize, col: usize) -> char {
        screen.cell(row, col).map_or('\u{0}', |c| c.ch)
    }

    #[test]
    fn plain_text_lands_left_to_right_and_advances_the_cursor() {
        let s = run(10, 3, b"hi");
        assert_eq!(ch(&s, 0, 0), 'h');
        assert_eq!(ch(&s, 0, 1), 'i');
        assert_eq!(s.cursor(), CursorPos { row: 0, col: 2 });
    }

    #[test]
    fn an_osc_title_and_a_bel_surface_as_drainable_events() {
        let mut term = Terminal::with_default_scrollback(20, 5);
        let before = term.bytes_seen();
        // OSC 0 sets the title (BEL-terminated); the terminator BEL is not a bell.
        term.feed(b"\x1b]0;deploy\x07");
        assert!(
            term.bytes_seen() > before,
            "feeding advanced the byte counter"
        );
        // A lone BEL rings.
        term.feed(b"\x07");

        let events = term.drain_events();
        assert!(
            events.contains(&TermEvent::Title("deploy".to_owned())),
            "the running command's title was captured: {events:?}"
        );
        assert!(
            events.contains(&TermEvent::Bell),
            "the lone BEL was captured: {events:?}"
        );
        // Draining is one-shot.
        assert!(term.drain_events().is_empty());
    }

    #[test]
    fn carriage_return_and_linefeed_move_the_cursor() {
        let s = run(10, 3, b"a\r\nb");
        assert_eq!(ch(&s, 0, 0), 'a');
        assert_eq!(ch(&s, 1, 0), 'b');
    }

    #[test]
    fn sgr_named_colors_set_the_foreground() {
        // ESC[31m = red (ANSI slot 1), then reset.
        let s = run(10, 1, b"\x1b[31mR\x1b[0mX");
        let r = s.cell(0, 0).expect("cell R");
        assert_eq!(r.ch, 'R');
        assert_eq!(r.fg, CellColor::Palette(1));
        // After SGR 0 the default foreground returns.
        assert_eq!(s.cell(0, 1).expect("cell X").fg, CellColor::Default);
    }

    #[test]
    fn sgr_bright_named_color_maps_to_the_high_palette_slot() {
        // ESC[92m = bright green (ANSI slot 10).
        let s = run(10, 1, b"\x1b[92mG");
        assert_eq!(s.cell(0, 0).expect("cell").fg, CellColor::Palette(10));
    }

    #[test]
    fn sgr_256_color_indexes_the_palette() {
        // ESC[38;5;123m — 256-colour foreground index 123.
        let s = run(10, 1, b"\x1b[38;5;123mY");
        assert_eq!(s.cell(0, 0).expect("cell").fg, CellColor::Palette(123));
    }

    #[test]
    fn sgr_truecolor_sets_rgb_foreground_and_background() {
        // fg = rgb(10,20,30), bg = rgb(200,100,50).
        let s = run(10, 1, b"\x1b[38;2;10;20;30m\x1b[48;2;200;100;50mZ");
        let z = s.cell(0, 0).expect("cell");
        assert_eq!(z.fg, CellColor::Rgb(10, 20, 30));
        assert_eq!(z.bg, CellColor::Rgb(200, 100, 50));
    }

    #[test]
    fn sgr_attributes_bold_italic_underline_inverse() {
        let s = run(10, 1, b"\x1b[1;3;4;7mA");
        let a = s.cell(0, 0).expect("cell").attrs;
        assert!(a.bold, "bold");
        assert!(a.italic, "italic");
        assert!(a.underline, "underline");
        assert!(a.inverse, "inverse");
    }

    #[test]
    fn cup_positions_the_cursor_absolutely() {
        // ESC[2;5H — row 2, col 5 (1-based) => row 1, col 4 (0-based).
        let s = run(10, 5, b"\x1b[2;5HX");
        assert_eq!(ch(&s, 1, 4), 'X');
    }

    #[test]
    fn cursor_up_and_forward_are_relative() {
        // CUP row3col3 (0-based 2,2); CUU 1 -> row1; CUF 2 -> col4; write.
        let s = run(10, 5, b"\x1b[3;3H\x1b[1A\x1b[2CU");
        assert_eq!(ch(&s, 1, 4), 'U');
    }

    #[test]
    fn cursor_down_and_back_are_relative() {
        // Home; CUD 2 -> row2; CUF 3 -> col3; write 'D' (cursor -> col4);
        // CUB 2 -> col2; write 'E'. So (2,3)='D' and (2,2)='E'.
        let s = run(10, 5, b"\x1b[1;1H\x1b[2B\x1b[3CD\x1b[2DE");
        assert_eq!(ch(&s, 2, 3), 'D');
        assert_eq!(ch(&s, 2, 2), 'E');
    }

    #[test]
    fn erase_in_line_clears_to_the_end() {
        // Write 5, move to col 3 (0-based 2), erase-to-end (ESC[0K).
        let s = run(10, 1, b"ABCDE\x1b[1;3H\x1b[0K");
        assert_eq!(ch(&s, 0, 0), 'A');
        assert_eq!(ch(&s, 0, 1), 'B');
        assert_eq!(ch(&s, 0, 2), ' '); // C erased
        assert_eq!(ch(&s, 0, 4), ' '); // E erased
    }

    #[test]
    fn erase_in_display_clears_the_whole_screen() {
        let s = run(10, 3, b"line1\r\nline2\x1b[2J");
        for row in 0..3 {
            for col in 0..10 {
                assert_eq!(ch(&s, row, col), ' ', "cell {row},{col} should be blank");
            }
        }
    }

    #[test]
    fn autowrap_carries_text_to_the_next_row() {
        // 4 columns, 6 chars -> "abcd" then "ef" on the next row.
        let s = run(4, 3, b"abcdef");
        assert_eq!(s.line_text(0), "abcd");
        assert_eq!(s.line_text(1), "ef");
    }

    #[test]
    fn horizontal_tab_advances_to_the_next_tab_stop() {
        // Default tab stops every 8 columns: tab from col 0 -> col 8.
        let s = run(20, 1, b"\tX");
        assert_eq!(ch(&s, 0, 8), 'X');
    }

    #[test]
    fn window_slices_history_at_the_requested_depth() {
        // 2 visible rows; L1/L2 scroll into history, viewport = L3/L4.
        let mut term = Terminal::new(10, 2, 1000);
        term.feed(b"L1\r\nL2\r\nL3\r\nL4");
        assert_eq!(term.scrollback_len(), 2);

        // Depth 0 is exactly the live viewport.
        assert_eq!(term.window(0), term.viewport());

        // One line back: the window spans L2/L3.
        let w1 = term.window(1);
        assert_eq!(
            (w1.line_text(0), w1.line_text(1)),
            ("L2".into(), "L3".into())
        );

        // Full depth — and any over-ask clamps to it.
        let w2 = term.window(2);
        assert_eq!(
            (w2.line_text(0), w2.line_text(1)),
            ("L1".into(), "L2".into())
        );
        assert_eq!(term.window(99), w2);

        // The live cursor sits below a scrolled window: its snapshot row is
        // pushed past the window height (renderers read that as "hidden").
        assert!(w2.cursor().row >= w2.rows(), "cursor is off-window");
        assert_eq!(term.window(0).cursor(), term.viewport().cursor());
    }

    #[test]
    fn lines_scrolled_off_the_top_land_in_scrollback() {
        // 2 visible rows; 4 lines pushed -> 2 lines scroll into history.
        let mut term = Terminal::new(10, 2, 1000);
        term.feed(b"L1\r\nL2\r\nL3\r\nL4");
        assert_eq!(term.rows(), 2);
        assert!(term.scrollback_len() >= 2, "history retained");

        // The viewport shows the last two lines...
        let vp = term.viewport();
        assert_eq!(vp.line_text(vp.rows() - 1), "L4");

        // ...and the full snapshot reaches back to the scrolled-off first line.
        let full = term.full();
        assert_eq!(full.line_text(0), "L1");
        assert_eq!(full.rows(), term.scrollback_len() + 2);
    }
}
