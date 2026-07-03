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
//!   snaps back to live.
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

use std::time::Duration;

use mde_egui::egui::text::LayoutJob;
use mde_egui::egui::{
    self, Align2, Context, Event, EventFilter, FontId, Key, Modifiers, MouseWheelUnit, Pos2, Rect,
    Response, Sense, Stroke, StrokeKind, TextFormat, Ui, Vec2,
};
use mde_egui::Style;

use crate::palette;
use crate::pty::LocalPty;
use crate::screen::{Cell, Screen};

/// Repaint cadence while the session is live. PTY output arrives on the pump
/// thread with no egui waker, so the surface heartbeats at ~30 fps and stops
/// once the child exits.
const LIVE_REPAINT: Duration = Duration::from_millis(33);

/// Cursor blink half-period in seconds (the classic ~500 ms phase).
const BLINK_HALF_PERIOD: f64 = 0.5;

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

/// How the cursor cell paints this frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CursorPaint {
    /// Not drawn (scrolled into history, blink-off phase, or session ended).
    Hidden,
    /// The unfocused outline.
    Hollow,
    /// The focused filled block (glyph repainted in the bg token over it).
    Block,
}

/// Everything the paint pass needs besides the screen itself. Bundled so the
/// headless render tests drive the exact painter the live widget uses.
struct PaintSpec {
    font_id: FontId,
    cell: Vec2,
    first_abs: usize,
    selection: Option<Selection>,
    cursor: CursorPaint,
    /// Lines currently scrolled back (paints the position chip when > 0).
    scrolled: usize,
    /// The child exited — paint the honest session-ended chip.
    ended: bool,
}

/// The interactive terminal pane: one [`LocalPty`] session rendered as an
/// egui widget. See the module docs for the frame anatomy.
pub struct TerminalWidget {
    pty: LocalPty,
    font_size: f32,
    cursor_blink: bool,
    /// Lines scrolled back into history; `0` = live.
    scroll_offset: usize,
    /// Fractional wheel remainder (smooth trackpads scroll in sub-lines).
    scroll_accum: f32,
    selection: Option<Selection>,
    last_grid: Option<(u16, u16)>,
}

impl TerminalWidget {
    /// Wrap a spawned session. The widget sizes the PTY to its rect on the
    /// first frame, so the spawn dimensions only cover the gap until then.
    #[must_use]
    pub const fn new(pty: LocalPty) -> Self {
        Self {
            pty,
            font_size: Style::BODY,
            cursor_blink: true,
            scroll_offset: 0,
            scroll_accum: 0.0,
            selection: None,
            last_grid: None,
        }
    }

    /// The content font size in points (lock 13: font size is a knob).
    #[must_use]
    pub const fn with_font_size(mut self, size: f32) -> Self {
        self.font_size = size;
        self
    }

    /// Whether the focused block cursor blinks (lock 13: cursor style knob).
    #[must_use]
    pub const fn with_cursor_blink(mut self, blink: bool) -> Self {
        self.cursor_blink = blink;
        self
    }

    /// The underlying session (splits/broadcast in TERM-4/6 route through it).
    #[must_use]
    pub const fn pty(&self) -> &LocalPty {
        &self.pty
    }

    /// Render one frame into `ui`, consuming this frame's input. Fills all
    /// available space.
    pub fn show(&mut self, ui: &mut Ui) -> Response {
        let font_id = FontId::monospace(self.font_size);
        let cell = ui.fonts(|f| Vec2::new(f.glyph_width(&font_id, 'M'), f.row_height(&font_id)));
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let (cols, rows) = grid_size(rect.size(), cell);

        // A changed rect maps to a new grid: engine reflow + TIOCSWINSZ.
        if self.last_grid != Some((cols, rows)) {
            self.pty.resize(cols, rows);
            self.last_grid = Some((cols, rows));
        }

        // Input first, so a scroll/snap lands in this frame's snapshot.
        let history = self
            .pty
            .with_terminal(crate::engine::Terminal::scrollback_len);
        self.handle_input(ui, &response, cell, usize::from(rows), history);
        self.scroll_offset = self.scroll_offset.min(history);

        // One engine lock for the visible window (O(rows × cols), never the
        // full history).
        let screen = self.pty.with_terminal(|t| t.window(self.scroll_offset));
        let first_abs = history - self.scroll_offset;

        self.handle_pointer(&response, rect, cell, first_abs, &screen);

        let ended = self.pty.is_output_closed();
        let cursor = if ended || self.scroll_offset > 0 {
            CursorPaint::Hidden
        } else if !response.has_focus() {
            CursorPaint::Hollow
        } else if !self.cursor_blink || blink_on(ui.input(|i| i.time)) {
            CursorPaint::Block
        } else {
            CursorPaint::Hidden
        };

        paint_grid(
            &ui.painter_at(rect),
            rect,
            &screen,
            &PaintSpec {
                font_id,
                cell,
                first_abs,
                selection: self.selection,
                cursor,
                scrolled: self.scroll_offset,
                ended,
            },
        );

        if !ended {
            ui.ctx().request_repaint_after(LIVE_REPAINT);
        }
        response
    }

    /// Keyboard + clipboard + wheel, from this frame's event stream.
    fn handle_input(
        &mut self,
        ui: &Ui,
        response: &Response,
        cell: Vec2,
        rows: usize,
        history: usize,
    ) {
        if response.clicked() || response.drag_started() {
            response.request_focus();
        }
        // A lone terminal grabs the keyboard at launch (TERM-4's split panes
        // manage focus explicitly; here "nothing focused" means us).
        if ui.memory(|m| m.focused().is_none()) && !self.pty.is_output_closed() {
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
            match event {
                // Wheel scrolling works on hover, focused or not.
                Event::MouseWheel { unit, delta, .. } if response.hovered() => {
                    self.wheel(unit, delta.y, cell.y, rows, history);
                }
                _ if !focused => {}
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

    /// Mouse selection: press anchors, drag extends, plain click clears.
    fn handle_pointer(
        &mut self,
        response: &Response,
        rect: Rect,
        cell: Vec2,
        first_abs: usize,
        screen: &Screen,
    ) {
        let pos_to_cell = |pos: Pos2| {
            let (row, col) = cell_at(rect.min, cell, pos, screen.cols(), screen.rows());
            CellPos {
                row: first_abs + row,
                col,
            }
        };
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
        } else if response.clicked() {
            self.selection = None;
        }
    }

    /// Copy the current selection to the clipboard (no-op without one).
    fn copy_selection(&self, ctx: &Context) {
        if let Some(sel) = self.selection {
            // One-shot full snapshot: the selection may live in history.
            let text = self.pty.with_terminal(|t| selected_text(&t.full(), &sel));
            if !text.is_empty() {
                ctx.copy_text(text);
            }
        }
    }

    /// Queue bytes to the shell and snap the view back to live. A dead
    /// session refuses input; the ended chip already tells that story, so the
    /// error is deliberately dropped here.
    fn send(&mut self, bytes: &[u8]) {
        self.scroll_offset = 0;
        self.scroll_accum = 0.0;
        let _ = self.pty.send_input(bytes);
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
    fn of(cell: &Cell) -> Self {
        let (fg, bg) = palette::cell_colors(cell);
        Self {
            fg,
            bg,
            italic: cell.attrs.italic,
            underline: cell.attrs.underline,
            strikeout: cell.attrs.strikeout,
        }
    }

    /// True for a cell that paints nothing (default-bg blank, no decoration)
    /// — trailing runs of these are skipped entirely.
    fn is_blank(&self, ch: char) -> bool {
        ch == ' ' && self.bg == Style::BG && !self.underline && !self.strikeout
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
    painter.rect_filled(rect, 0.0, Style::BG);

    for row in 0..screen.rows() {
        if let Some(cells) = screen.row(row) {
            paint_row(painter, rect.min, spec, row, cells);
        }
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

    // Chrome chips (pure Style tokens): scrollback position + session end.
    if spec.scrolled > 0 {
        chip(
            painter,
            Pos2::new(rect.max.x - Style::SP_S, rect.min.y + Style::SP_S),
            Align2::RIGHT_TOP,
            &format!("+{} lines", spec.scrolled),
            Style::TEXT_DIM,
        );
    }
    if spec.ended {
        chip(
            painter,
            rect.center(),
            Align2::CENTER_CENTER,
            "session ended",
            Style::TEXT_DIM,
        );
    }
}

/// Paint one row as batched same-style runs: one bg rect + one galley per run
/// (never a galley per cell), with the trailing default-blank tail trimmed.
fn paint_row(painter: &egui::Painter, origin: Pos2, spec: &PaintSpec, row: usize, cells: &[Cell]) {
    let mut end = cells.len();
    while end > 0 && RunStyle::of(&cells[end - 1]).is_blank(cells[end - 1].ch) {
        end -= 1;
    }
    let mut col = 0;
    while col < end {
        let style = RunStyle::of(&cells[col]);
        let mut run_end = col + 1;
        while run_end < end && RunStyle::of(&cells[run_end]) == style {
            run_end += 1;
        }
        let run = cell_span_rect(origin, spec.cell, row, col, run_end - col);
        if style.bg != Style::BG {
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

/// The cursor block: filled + glyph repainted in the bg token (focused), or
/// the hollow outline (unfocused).
fn paint_cursor(painter: &egui::Painter, origin: Pos2, screen: &Screen, spec: &PaintSpec) {
    let cur = screen.cursor();
    let cols = screen.cols();
    if spec.cursor == CursorPaint::Hidden || cur.row >= screen.rows() || cols == 0 {
        return;
    }
    let col = cur.col.min(cols - 1);
    let block = cell_span_rect(origin, spec.cell, cur.row, col, 1);
    match spec.cursor {
        CursorPaint::Block => {
            painter.rect_filled(block, 0.0, Style::TEXT);
            let ch = screen.cell(cur.row, col).map_or(' ', |c| c.ch);
            if ch != ' ' {
                let galley = painter.layout_job(LayoutJob::single_section(
                    ch.to_string(),
                    TextFormat {
                        font_id: spec.font_id.clone(),
                        color: Style::BG,
                        ..TextFormat::default()
                    },
                ));
                painter.galley(block.min, galley, Style::BG);
            }
        }
        CursorPaint::Hollow => {
            painter.rect_stroke(
                block,
                0.0,
                Stroke::new(1.0, Style::TEXT),
                StrokeKind::Inside,
            );
        }
        CursorPaint::Hidden => {}
    }
}

/// A small status chip: SURFACE plate, hairline border, dimmed label.
fn chip(painter: &egui::Painter, at: Pos2, anchor: Align2, label: &str, color: egui::Color32) {
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
            first_abs: 0,
            selection: None,
            cursor: CursorPaint::Block,
            scrolled: 0,
            ended: false,
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
        assert!(has(palette::cell_colors(tc).1), "truecolor bg rect");
        // Chrome: the grid base is the BG token; the block cursor is TEXT.
        assert!(has(Style::BG), "background fill");
        assert!(has(Style::TEXT), "block cursor fill");
    }

    #[test]
    fn selection_scrollback_chip_and_ended_chip_render() {
        let mut term = Terminal::new(20, 3, 100);
        term.feed(b"one\r\ntwo\r\nthree");
        let screen = term.viewport();
        let colors = tessellate_colors(&screen, |font_id, cell| PaintSpec {
            font_id,
            cell,
            first_abs: 0,
            selection: Some(sel((0, 0), (1, 2))),
            cursor: CursorPaint::Hollow,
            scrolled: 7,
            ended: true,
        });
        let has = |c: egui::Color32| colors.contains(&c);
        assert!(
            has(Style::ACCENT.gamma_multiply(0.35)),
            "selection overlay uses the token blend"
        );
        assert!(has(Style::SURFACE), "chip plate");
        assert!(has(Style::TEXT_DIM), "chip label");
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
        let (cols, rows) = widget.pty().with_terminal(|t| (t.cols(), t.rows()));
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
}
