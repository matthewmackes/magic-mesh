//! The **custom code-editor text widget** (EDITOR-3): the immediate-mode egui
//! surface that renders + edits a live rope [`Buffer`](crate::buffer::Buffer).
//!
//! This is the core of the editor. It is NOT a mockup: the widget reads the real
//! rope, paints only the **visible** lines (viewport culling — a 100k-line buffer
//! never paints every line), and every key/pointer gesture mutates the actual
//! `Buffer`, so typing changes the rope and the next frame re-renders it (§7).
//!
//! The split of concerns:
//!
//! * [`EditorView`] holds the **widget state** the buffer itself has no business
//!   knowing — the caret (a char index into the rope), the selection anchor, the
//!   sticky goal column for vertical motion, and the soft-wrap toggle. It carries
//!   the *pure* cursor-movement, selection, and edit-application logic so those
//!   are unit-testable **without** a live egui frame (the `EditorView::*` methods
//!   below take a `&Buffer`/`&mut Buffer` and a synthetic [`egui::Event`], never a
//!   `Ui`).
//! * [`editor_widget`] is the one egui entry point: it lays the view out inside a
//!   scroll area, maps the pointer to a rope char index through the monospace
//!   glyph metrics (click / drag / double- / triple-click), routes this frame's
//!   key events into the view, and paints the gutter + text + selection + caret
//!   through the shared Carbon [`Style`] tokens (§4 — no raw hex, no scattered
//!   metric).
//!
//! Multi-cursor, tree-sitter highlighting, and the fuzzy finder land in
//! EDITOR-4/5+; this unit is the single-caret editing core they build on.

// `EditorView` is the domain name for this module's widget-state type; renaming
// it to dodge the `widget` echo would be worse (the same call `buffer.rs` makes).
// `missing_const_for_fn` (nursery) is over-eager for small mutators whose
// const-ness we don't want to pin into the public contract. `suboptimal_flops`
// is allowed for the layout arithmetic: `origin + col * glyph_w` reads far
// clearer than the `mul_add` rewrite, and the precision/throughput gain is
// irrelevant for a few pixel positions per row (same rationale + repo precedent
// as `mde-mesh-view` / `mde-panel-egui`).
#![allow(
    clippy::module_name_repetitions,
    clippy::missing_const_for_fn,
    clippy::suboptimal_flops
)]

use std::ops::Range;
use std::time::Duration;

use mde_egui::egui::{
    self, pos2, vec2, Align2, Event, EventFilter, FontId, Key, Modifiers, Pos2, Rect, Response,
    ScrollArea, Sense, Stroke, Ui,
};
use mde_egui::Style;

use crate::buffer::Buffer;

/// Soft-tab width: a Tab keypress inserts this many spaces (the editor is
/// spaces-by-default, the common Rust convention). Not a metric — a text unit.
const TAB_SPACES: usize = 4;

/// The caret's bar width, derived from the 4px half-step token (≈ 2px) so it is a
/// crisp beam at any DPI without hard-coding a pixel (§4 — token-derived, never a
/// raw metric).
const CARET_W: f32 = Style::SP_XS / 2.0;

/// Half-second caret blink phase (the classic editor cadence). Time-derived from
/// the egui frame clock, so it needs no wall-clock and stays test-free.
const BLINK_HZ: f64 = 2.0;

/// The character class used for word-wise selection/motion (double-click and
/// `Ctrl`+Arrow): a word run, a whitespace run, or a punctuation run. Expanding a
/// selection stops at a class boundary, matching every editor's word semantics.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    /// Identifier characters — alphanumerics and `_`.
    Word,
    /// Whitespace (never a newline here: word spans are line-local).
    Space,
    /// Everything else — punctuation / symbols.
    Punct,
}

/// Classify one character into its [`Class`] for word-wise gestures.
fn class_of(c: char) -> Class {
    if c == '_' || c.is_alphanumeric() {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Punct
    }
}

/// Characters in logical `line`, **excluding** its trailing newline (`\n` or
/// `\r\n`). Measured straight off the rope's line slice (no `String` alloc), so
/// it stays O(log n) + O(line) and never materializes the whole document.
///
/// Returns 0 for an out-of-range line (the caller clamps, but this keeps the
/// helper total rather than panicking — `panic` is a denied lint).
fn line_len(buffer: &Buffer, line: usize) -> usize {
    if line >= buffer.len_lines() {
        return 0;
    }
    let slice = buffer.rope().line(line);
    let mut n = slice.len_chars();
    if n > 0 && slice.char(n - 1) == '\n' {
        n -= 1;
        if n > 0 && slice.char(n - 1) == '\r' {
            n -= 1;
        }
    }
    n
}

/// The char index of `(line, col)`, with `line` clamped into the buffer and `col`
/// clamped to the line's length (so a click past the end lands at end-of-line and
/// a vertical move onto a short line snaps to its end).
fn char_at(buffer: &Buffer, line: usize, col: usize) -> usize {
    let line = line.min(buffer.len_lines().saturating_sub(1));
    let start = buffer.line_to_char(line);
    start + col.min(line_len(buffer, line))
}

/// The inclusive-start/exclusive-end char span of the word (or whitespace/punct
/// run) under `idx` — the double-click selection. Line-local: a word never spans
/// a newline.
fn word_span(buffer: &Buffer, idx: usize) -> Range<usize> {
    let idx = idx.min(buffer.len_chars());
    let line = buffer.char_to_line(idx);
    let start = buffer.line_to_char(line);
    let chars: Vec<char> = buffer.rope().line(line).chars().collect();
    let llen = line_len(buffer, line);
    if llen == 0 {
        return start..start;
    }
    let col = (idx - start).min(llen);
    // Probe the char the caret sits on; at end-of-line, probe the last char.
    let probe = if col >= llen { llen - 1 } else { col };
    let cls = class_of(chars[probe]);
    let mut s = probe;
    while s > 0 && class_of(chars[s - 1]) == cls {
        s -= 1;
    }
    let mut e = probe;
    while e + 1 < llen && class_of(chars[e + 1]) == cls {
        e += 1;
    }
    (start + s)..(start + e + 1)
}

/// The char span of the whole logical line under `idx`, excluding its trailing
/// newline — the triple-click selection.
fn line_span(buffer: &Buffer, idx: usize) -> Range<usize> {
    let idx = idx.min(buffer.len_chars());
    let line = buffer.char_to_line(idx);
    let start = buffer.line_to_char(line);
    start..(start + line_len(buffer, line))
}

/// The next word boundary at or after `idx` (the `Ctrl`+Right / word-motion
/// target): skip any run the caret is inside, landing at the end of the current
/// word or the start of the next.
fn next_word(buffer: &Buffer, idx: usize) -> usize {
    let len = buffer.len_chars();
    if idx >= len {
        return len;
    }
    let rope = buffer.rope();
    let mut i = idx;
    let start_cls = class_of(rope.char(i));
    // Consume the current run, then any following whitespace, so the caret lands
    // on the next meaningful token.
    while i < len && class_of(rope.char(i)) == start_cls {
        i += 1;
    }
    while i < len && class_of(rope.char(i)) == Class::Space {
        i += 1;
    }
    i
}

/// The previous word boundary at or before `idx` (the `Ctrl`+Left target): skip
/// any whitespace immediately behind the caret, then the word run before it.
fn prev_word(buffer: &Buffer, idx: usize) -> usize {
    if idx == 0 {
        return 0;
    }
    let rope = buffer.rope();
    let mut i = idx;
    while i > 0 && class_of(rope.char(i - 1)) == Class::Space {
        i -= 1;
    }
    if i == 0 {
        return 0;
    }
    let cls = class_of(rope.char(i - 1));
    while i > 0 && class_of(rope.char(i - 1)) == cls {
        i -= 1;
    }
    i
}

/// A prefix-sum map from logical lines to **visual rows** for soft-wrap, so the
/// widget can virtualize (cull) a wrapped document by row without an O(n) walk
/// per frame.
///
/// `rows_before[i]` is the number of visual rows above logical line `i`; the last
/// entry is the total visual-row count. Rebuilt only when the wrap width or the
/// buffer's `(len_chars, len_lines)` shape changes — a small O(lines) pass that a
/// per-keystroke edit does not trigger. Wrap is a **basic** pass this unit
/// (fixed-width character wrapping, no word-breaking); EDITOR-4/5 refine it.
#[derive(Default)]
struct WrapMap {
    /// Wrap width in columns this map was built for (0 ⇒ never built).
    cols: usize,
    /// Buffer char count when built — half of the cheap staleness check.
    len_chars: usize,
    /// Buffer line count when built — the other half.
    len_lines: usize,
    /// `rows_before[i]` = visual rows above logical line `i`; `len == len_lines + 1`.
    rows_before: Vec<usize>,
}

impl WrapMap {
    /// Whether this map is still valid for `buffer` wrapped at `cols` columns.
    fn is_valid(&self, buffer: &Buffer, cols: usize) -> bool {
        self.cols == cols
            && self.len_chars == buffer.len_chars()
            && self.len_lines == buffer.len_lines()
    }

    /// Rebuild the prefix sums for `buffer` wrapped at `cols` columns (O(lines)).
    fn rebuild(&mut self, buffer: &Buffer, cols: usize) {
        let cols = cols.max(1);
        let lines = buffer.len_lines();
        self.rows_before.clear();
        self.rows_before.reserve(lines + 1);
        let mut acc = 0;
        for line in 0..lines {
            self.rows_before.push(acc);
            let llen = line_len(buffer, line);
            acc += llen.div_ceil(cols).max(1);
        }
        self.rows_before.push(acc);
        self.cols = cols;
        self.len_chars = buffer.len_chars();
        self.len_lines = buffer.len_lines();
    }

    /// Total visual rows (≥ 1).
    fn total(&self) -> usize {
        self.rows_before.last().copied().unwrap_or(1).max(1)
    }

    /// The logical line owning visual row `vr`.
    fn line_at(&self, vr: usize) -> usize {
        let line = self.rows_before.partition_point(|&r| r <= vr);
        line.saturating_sub(1).min(self.len_lines.saturating_sub(1))
    }

    /// The first visual row of logical line `line`.
    fn row_of(&self, line: usize) -> usize {
        self.rows_before.get(line).copied().unwrap_or(0)
    }
}

/// One painted **visual row**: a slice `[start, end)` of a logical line rendered
/// on a single screen row. Without wrap, one logical line is exactly one visual
/// row; with wrap, a long line spans several. Painting, hit-testing, selection,
/// and caret placement all consume this one shape, so both modes share a path.
#[derive(Clone, Copy)]
struct VisRow {
    /// The logical line this row belongs to.
    line: usize,
    /// First char index shown on this row.
    start: usize,
    /// One-past-the-last char index shown on this row (excludes the newline).
    end: usize,
    /// Whether this is the first visual row of its logical line (the row that
    /// carries the gutter line number).
    first: bool,
    /// Whether the logical line continues past `end` only via its newline (used
    /// to draw a trailing selection hint on a multi-line selection).
    line_end: usize,
}

/// The monospace cell + gutter metrics for one frame, resolved once from the
/// font. `Copy` so the paint helpers take it by value without a borrow.
#[derive(Clone, Copy)]
struct Metrics {
    /// Advance width of one glyph (`'M'` in a monospace face = every glyph).
    glyph_w: f32,
    /// Line height.
    row_h: f32,
    /// Width of the left gutter (line-number column).
    gutter_w: f32,
}

/// The monospace font every editor glyph + the cell metrics resolve against.
///
/// The shell's `fonts::install` puts the bundled face first in the Monospace
/// family, so this id renders through it (mirrors `mde-term-egui`).
fn body_font() -> FontId {
    FontId::monospace(Style::BODY)
}

/// The widget state for one open document (EDITOR-3).
///
/// Holds the caret, the selection anchor, the vertical-motion goal column, and
/// the soft-wrap toggle — everything the pure [`Buffer`](crate::buffer::Buffer)
/// does not itself track.
///
/// The caret and anchor are **char indices** into the rope (not `(line, col)`),
/// so they compose directly with `Buffer::insert`/`remove`; `(line, col)` is
/// derived on demand through the rope's O(log n) line index. All movement,
/// selection, and edit-application logic lives here as `&Buffer`/`&mut Buffer`
/// methods so it is unit-testable without a live egui frame.
pub struct EditorView {
    /// Caret position — a char index into the rope.
    cursor: usize,
    /// Selection anchor — the fixed end of the selection, or `None` when there is
    /// no selection. The selection is always `min(anchor, cursor)..max(..)`.
    anchor: Option<usize>,
    /// Sticky column for vertical motion: set on Up/Down/PageUp/Down and kept
    /// across a run of them so the caret tracks the same column over short lines,
    /// cleared by any horizontal move or edit.
    goal_col: Option<usize>,
    /// Soft-wrap toggle: on wraps long lines to the viewport (no horizontal
    /// scroll), off keeps lines unwrapped and scrolls horizontally.
    wrap: bool,
    /// Widest line seen so far, in chars — the horizontal-scroll extent when
    /// unwrapped. Grows only (a cheap monotonic estimate; a precise re-measure
    /// lands with the highlighter pass), so the scrollbar never jitters.
    max_line_chars: usize,
    /// Cached wrap prefix sums, rebuilt lazily when wrap is on.
    wrap_map: WrapMap,
    /// Set by any caret move/edit; consumed by the renderer to scroll the caret
    /// back into view exactly once (so it doesn't fight the user's own scroll).
    reveal_caret: bool,
}

impl Default for EditorView {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorView {
    /// A fresh view over a document: caret at the top, no selection, wrap off.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cursor: 0,
            anchor: None,
            goal_col: None,
            wrap: false,
            max_line_chars: 0,
            wrap_map: WrapMap::default(),
            reveal_caret: false,
        }
    }

    /// The caret's char index into the rope.
    #[must_use]
    pub const fn cursor(&self) -> usize {
        self.cursor
    }

    /// The current selection as a char range, or `None` when nothing is selected
    /// (no anchor, or the anchor coincides with the caret).
    #[must_use]
    pub fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.anchor?;
        let (lo, hi) = (anchor.min(self.cursor), anchor.max(self.cursor));
        (lo < hi).then_some(lo..hi)
    }

    /// Whether soft-wrap is on.
    #[must_use]
    pub const fn wrap(&self) -> bool {
        self.wrap
    }

    /// Flip the soft-wrap toggle (the chrome strip's Wrap control).
    pub fn toggle_wrap(&mut self) {
        self.wrap = !self.wrap;
    }

    /// The caret's 1-based `(line, column)` for the status strip.
    #[must_use]
    pub fn line_col(&self, buffer: &Buffer) -> (usize, usize) {
        let cursor = self.cursor.min(buffer.len_chars());
        let line = buffer.char_to_line(cursor);
        (line + 1, cursor - buffer.line_to_char(line) + 1)
    }

    /// Place the caret at char index `idx` (clamped), clearing any selection — the
    /// seam a Files-send / finder jump (EDITOR-7/9) uses to reveal a location.
    pub fn place_cursor(&mut self, buffer: &Buffer, idx: usize) {
        self.cursor = idx.min(buffer.len_chars());
        self.anchor = None;
        self.goal_col = None;
        self.reveal_caret = true;
    }

    /// Clamp the caret + anchor back inside the buffer (called each frame in case
    /// the rope shrank underneath the view).
    fn clamp(&mut self, buffer: &Buffer) {
        let len = buffer.len_chars();
        self.cursor = self.cursor.min(len);
        if let Some(a) = self.anchor {
            self.anchor = Some(a.min(len));
        }
    }

    // ── caret geometry ──────────────────────────────────────────────────────

    /// The caret's logical line.
    fn cur_line(&self, buffer: &Buffer) -> usize {
        buffer.char_to_line(self.cursor.min(buffer.len_chars()))
    }

    /// The caret's column within its logical line.
    fn cur_col(&self, buffer: &Buffer) -> usize {
        let line = self.cur_line(buffer);
        self.cursor - buffer.line_to_char(line)
    }

    // ── selection-aware cursor placement ────────────────────────────────────

    /// Move the caret to `new`, extending the selection when `extend` (Shift):
    /// the first extend drops an anchor at the old caret; a non-extend move drops
    /// the selection.
    fn set_cursor(&mut self, new: usize, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = new;
        self.reveal_caret = true;
    }

    // ── edits (selection-aware) ─────────────────────────────────────────────

    /// Insert `text` at the caret, replacing the selection first if there is one.
    /// The caret lands after the inserted text.
    fn insert(&mut self, buffer: &mut Buffer, text: &str) {
        if let Some(range) = self.selection() {
            buffer.remove(range.clone());
            self.cursor = range.start;
        }
        self.anchor = None;
        self.goal_col = None;
        buffer.insert(self.cursor, text);
        self.cursor += text.chars().count();
        self.reveal_caret = true;
    }

    /// Backspace: delete the selection if any, else the char before the caret.
    fn backspace(&mut self, buffer: &mut Buffer) {
        if let Some(range) = self.selection() {
            buffer.remove(range.clone());
            self.cursor = range.start;
            self.anchor = None;
        } else if self.cursor > 0 {
            buffer.remove(self.cursor - 1..self.cursor);
            self.cursor -= 1;
        }
        self.goal_col = None;
        self.reveal_caret = true;
    }

    /// Forward-delete: delete the selection if any, else the char at the caret.
    fn delete(&mut self, buffer: &mut Buffer) {
        if let Some(range) = self.selection() {
            buffer.remove(range.clone());
            self.cursor = range.start;
            self.anchor = None;
        } else if self.cursor < buffer.len_chars() {
            buffer.remove(self.cursor..self.cursor + 1);
        }
        self.goal_col = None;
        self.reveal_caret = true;
    }

    /// Select the whole buffer.
    fn select_all(&mut self, buffer: &Buffer) {
        self.anchor = Some(0);
        self.cursor = buffer.len_chars();
        self.goal_col = None;
        self.reveal_caret = true;
    }

    /// Undo one group, restoring the caret to the buffer's returned hint.
    fn undo(&mut self, buffer: &mut Buffer) {
        if let Some(hint) = buffer.undo() {
            self.cursor = hint.min(buffer.len_chars());
            self.anchor = None;
            self.goal_col = None;
            self.reveal_caret = true;
        }
    }

    /// Redo one group, restoring the caret to the buffer's returned hint.
    fn redo(&mut self, buffer: &mut Buffer) {
        if let Some(hint) = buffer.redo() {
            self.cursor = hint.min(buffer.len_chars());
            self.anchor = None;
            self.goal_col = None;
            self.reveal_caret = true;
        }
    }

    // ── pointer selection ───────────────────────────────────────────────────

    /// Click at char `idx`: place the caret (extend on Shift-click), closing the
    /// current undo group so a later type run starts fresh.
    fn click(&mut self, buffer: &mut Buffer, idx: usize, extend: bool) {
        buffer.commit_group();
        self.goal_col = None;
        self.set_cursor(idx, extend);
    }

    /// Drag to char `idx`: extend the selection from the drag's anchor.
    fn drag(&mut self, idx: usize) {
        self.goal_col = None;
        self.set_cursor(idx, true);
    }

    /// Double-click: select the word under `idx`.
    fn select_word(&mut self, buffer: &mut Buffer, idx: usize) {
        buffer.commit_group();
        let span = word_span(buffer, idx);
        self.anchor = Some(span.start);
        self.cursor = span.end;
        self.goal_col = None;
        self.reveal_caret = true;
    }

    /// Triple-click: select the logical line under `idx`.
    fn select_line(&mut self, buffer: &mut Buffer, idx: usize) {
        buffer.commit_group();
        let span = line_span(buffer, idx);
        self.anchor = Some(span.start);
        self.cursor = span.end;
        self.goal_col = None;
        self.reveal_caret = true;
    }

    // ── keyboard ────────────────────────────────────────────────────────────

    /// Apply one key/text event to the document, returning whether it changed the
    /// caret or the buffer (so the renderer knows to reveal the caret + repaint).
    ///
    /// Pure over `(&mut EditorView, &mut Buffer)` — no `Ui` — so the whole
    /// keymap is unit-testable with synthetic [`egui::Event`]s. `rows` is the
    /// viewport height in rows, used by PageUp/PageDown. Clipboard events
    /// (`Copy`/`Cut`) need the egui context and are handled by the caller;
    /// `Paste` carries its text and is applied here.
    fn apply_event(&mut self, buffer: &mut Buffer, event: &Event, rows: usize) -> bool {
        match event {
            Event::Text(text) | Event::Paste(text) if !text.is_empty() => {
                self.insert(buffer, text);
                true
            }
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => self.apply_key(buffer, *key, *modifiers, rows),
            _ => false,
        }
    }

    /// The key half of [`apply_event`](Self::apply_event): motion, editing, and
    /// undo/redo. `Ctrl`/`Cmd` is `modifiers.command`; `Shift` extends selection.
    #[allow(clippy::too_many_lines)]
    fn apply_key(&mut self, buffer: &mut Buffer, key: Key, mods: Modifiers, rows: usize) -> bool {
        let shift = mods.shift;
        let cmd = mods.command;
        match key {
            // ── horizontal motion ──
            Key::ArrowLeft => {
                buffer.commit_group();
                self.goal_col = None;
                let target = if cmd {
                    prev_word(buffer, self.cursor)
                } else {
                    self.cursor.saturating_sub(1)
                };
                self.set_cursor(target, shift);
                true
            }
            Key::ArrowRight => {
                buffer.commit_group();
                self.goal_col = None;
                let target = if cmd {
                    next_word(buffer, self.cursor)
                } else {
                    (self.cursor + 1).min(buffer.len_chars())
                };
                self.set_cursor(target, shift);
                true
            }
            Key::Home => {
                buffer.commit_group();
                self.goal_col = None;
                let target = if cmd {
                    0
                } else {
                    buffer.line_to_char(self.cur_line(buffer))
                };
                self.set_cursor(target, shift);
                true
            }
            Key::End => {
                buffer.commit_group();
                self.goal_col = None;
                let target = if cmd {
                    buffer.len_chars()
                } else {
                    let line = self.cur_line(buffer);
                    buffer.line_to_char(line) + line_len(buffer, line)
                };
                self.set_cursor(target, shift);
                true
            }
            // ── vertical motion (keeps the goal column) ──
            Key::ArrowUp => self.vertical(buffer, -1, shift),
            Key::ArrowDown => self.vertical(buffer, 1, shift),
            #[allow(clippy::cast_possible_wrap)]
            Key::PageUp => self.vertical(buffer, -(rows.max(1) as isize), shift),
            #[allow(clippy::cast_possible_wrap)]
            Key::PageDown => self.vertical(buffer, rows.max(1) as isize, shift),
            // ── editing ──
            Key::Enter => {
                self.insert(buffer, "\n");
                true
            }
            Key::Tab => {
                self.insert(buffer, &" ".repeat(TAB_SPACES));
                true
            }
            Key::Backspace => {
                self.backspace(buffer);
                true
            }
            Key::Delete => {
                self.delete(buffer);
                true
            }
            // ── selection / history ──
            Key::A if cmd => {
                self.select_all(buffer);
                true
            }
            Key::Z if cmd && shift => {
                self.redo(buffer);
                true
            }
            Key::Z if cmd => {
                self.undo(buffer);
                true
            }
            Key::Y if cmd => {
                self.redo(buffer);
                true
            }
            Key::Escape => {
                if self.anchor.take().is_some() {
                    self.reveal_caret = true;
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    /// Vertical caret motion by `delta` lines, preserving the goal column so a
    /// run of Up/Down tracks the same column across short lines. Always returns
    /// `true` (it moved the caret) so it composes in the key-dispatch match.
    fn vertical(&mut self, buffer: &mut Buffer, delta: isize, extend: bool) -> bool {
        buffer.commit_group();
        let line = self.cur_line(buffer);
        let col = self.cur_col(buffer);
        let goal = self.goal_col.unwrap_or(col);
        let max_line = buffer.len_lines().saturating_sub(1);
        #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
        let target_line = (line as isize + delta).clamp(0, max_line as isize) as usize;
        let target = char_at(buffer, target_line, goal);
        self.set_cursor(target, extend);
        self.goal_col = Some(goal);
        true
    }

    // ── egui frame ──────────────────────────────────────────────────────────

    /// Resolve visual row `vr` into its painted slice — one shape for both the
    /// unwrapped (one row per line) and wrapped (many rows per line) paths.
    fn vis_row(&self, buffer: &Buffer, vr: usize, cols: usize) -> VisRow {
        if self.wrap {
            let line = self.wrap_map.line_at(vr).min(buffer.len_lines().saturating_sub(1));
            let base = self.wrap_map.row_of(line);
            let sub = vr.saturating_sub(base);
            let line_start = buffer.line_to_char(line);
            let llen = line_len(buffer, line);
            let start = line_start + (sub * cols).min(llen);
            let end = line_start + ((sub + 1) * cols).min(llen);
            VisRow {
                line,
                start,
                end,
                first: sub == 0,
                line_end: line_start + llen,
            }
        } else {
            let line = vr.min(buffer.len_lines().saturating_sub(1));
            let start = buffer.line_to_char(line);
            let end = start + line_len(buffer, line);
            VisRow {
                line,
                start,
                end,
                first: true,
                line_end: end,
            }
        }
    }

    /// The caret's `(visual row, column-within-row)` for painting.
    fn caret_cell(&self, buffer: &Buffer, cols: usize) -> (usize, usize) {
        let line = self.cur_line(buffer);
        let col = self.cur_col(buffer);
        if self.wrap {
            let cols = cols.max(1);
            let llen = line_len(buffer, line);
            // A caret exactly at a wrap boundary belongs at the end of the previous
            // row, not the start of an empty next row.
            let (sub, xcol) = if col > 0 && col % cols == 0 && col == llen {
                (col / cols - 1, cols)
            } else {
                (col / cols, col % cols)
            };
            (self.wrap_map.row_of(line) + sub, xcol)
        } else {
            (line, col)
        }
    }
}

/// Render + edit an open [`Buffer`](crate::buffer::Buffer) through its
/// [`EditorView`] — the one egui entry point for the code editor (EDITOR-3).
///
/// Fills the available space with a scroll area, paints only the visible rows
/// (viewport culling), maps the pointer to a rope char index for click/drag/
/// double-/triple-click selection, routes this frame's key events into the view,
/// and draws the gutter + text + selection + caret through [`Style`] tokens (§4).
/// Returns the content [`Response`] so the surface can observe focus/hover.
pub fn editor_widget(ui: &mut Ui, view: &mut EditorView, buffer: &mut Buffer) -> Response {
    view.clamp(buffer);

    let font = body_font();
    let (glyph_w, row_h) =
        ui.fonts(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
    let gutter_w = gutter_width(buffer.len_lines(), glyph_w);
    let metrics = Metrics {
        glyph_w,
        row_h,
        gutter_w,
    };

    // Wrap width in columns from the available text span; keep the map fresh.
    let avail = ui.available_size();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let wrap_cols = (((avail.x - gutter_w) / glyph_w).floor().max(1.0)) as usize;
    if view.wrap && !view.wrap_map.is_valid(buffer, wrap_cols) {
        view.wrap_map.rebuild(buffer, wrap_cols);
    }
    let total_rows = if view.wrap {
        view.wrap_map.total()
    } else {
        buffer.len_lines()
    };

    let scroll = if view.wrap {
        ScrollArea::vertical()
    } else {
        ScrollArea::both()
    };
    scroll
        .id_salt("mde-editor-view")
        .auto_shrink([false, false])
        .drag_to_scroll(false)
        .show(ui, |ui| {
            editor_body(ui, view, buffer, metrics, wrap_cols, total_rows)
        })
        .inner
}

/// The scroll-area body: allocate the virtual content, handle input, paint the
/// visible rows. Split out of [`editor_widget`] so each half stays legible.
#[allow(clippy::too_many_lines)]
fn editor_body(
    ui: &mut Ui,
    view: &mut EditorView,
    buffer: &mut Buffer,
    m: Metrics,
    wrap_cols: usize,
    total_rows: usize,
) -> Response {
    let clip = ui.clip_rect();
    // Content extent: full virtual height so the scrollbar is honest; width is the
    // widest line (unwrapped) or the viewport (wrapped, so no h-scroll appears).
    #[allow(clippy::cast_precision_loss)]
    let content_h = total_rows as f32 * m.row_h;
    let content_w = if view.wrap {
        clip.width()
    } else {
        #[allow(clippy::cast_precision_loss)]
        let text_w = view.max_line_chars as f32 * m.glyph_w + m.glyph_w;
        (m.gutter_w + text_w).max(clip.width())
    };
    let (rect, resp) =
        ui.allocate_exact_size(vec2(content_w, content_h), Sense::click_and_drag());
    let origin = rect.min;

    // Viewport height in rows, for PageUp/PageDown.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let rows_visible = ((clip.height() / m.row_h).ceil().max(1.0)) as usize;

    // Input first. Pointer hit-testing reads the geometry that was *displayed*
    // (the pre-edit `total_rows` + `wrap_map`); a keystroke may then change the
    // buffer, so everything below is recomputed against the current buffer.
    handle_pointer(&resp, ui, view, buffer, m, origin, total_rows, wrap_cols);
    handle_keys(&resp, ui, view, buffer, rows_visible);

    // Re-validate geometry after any edit so the paint pass never indexes a line
    // that a delete removed (the wrap map + row count must match the live buffer).
    view.clamp(buffer);
    if view.wrap && !view.wrap_map.is_valid(buffer, wrap_cols) {
        view.wrap_map.rebuild(buffer, wrap_cols);
    }
    let total = if view.wrap {
        view.wrap_map.total()
    } else {
        buffer.len_lines()
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let first = (((clip.top() - origin.y) / m.row_h).floor().max(0.0)) as usize;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let last = (((clip.bottom() - origin.y) / m.row_h).ceil().max(0.0) as usize).min(total);

    // Grow the horizontal extent from the lines we can see + the caret line, so a
    // long line becomes scrollable once it enters the viewport.
    for vr in first..last {
        let row = view.vis_row(buffer, vr, wrap_cols);
        view.max_line_chars = view.max_line_chars.max(line_len(buffer, row.line));
    }
    view.max_line_chars = view.max_line_chars.max(line_len(buffer, view.cur_line(buffer)));

    paint(ui, view, buffer, m, origin, first, last, wrap_cols, &resp);

    // Reveal the caret exactly once after a move/edit (don't fight user scroll).
    if std::mem::take(&mut view.reveal_caret) {
        let (vr, xcol) = view.caret_cell(buffer, wrap_cols);
        #[allow(clippy::cast_precision_loss)]
        let caret_pos = pos2(
            origin.x + m.gutter_w + xcol as f32 * m.glyph_w,
            origin.y + vr as f32 * m.row_h,
        );
        // Pad left by the gutter so revealing a line-start also shows its number.
        let reveal = Rect::from_min_size(
            pos2(caret_pos.x - m.gutter_w, caret_pos.y),
            vec2(m.gutter_w + m.glyph_w, m.row_h),
        );
        ui.scroll_to_rect(reveal, None);
    }

    resp
}

/// Map the pointer to a rope char index and drive click / drag / double- /
/// triple-click selection.
#[allow(clippy::too_many_arguments)]
fn handle_pointer(
    resp: &Response,
    ui: &Ui,
    view: &mut EditorView,
    buffer: &mut Buffer,
    m: Metrics,
    origin: Pos2,
    total_rows: usize,
    wrap_cols: usize,
) {
    if resp.clicked() || resp.drag_started() || resp.double_clicked() || resp.triple_clicked() {
        resp.request_focus();
    }
    let shift = ui.input(|i| i.modifiers.shift);

    // Resolve the pointer to a char index while only borrowing the view/buffer
    // immutably, then mutate — a closure capturing them would clash with the
    // `&mut` the selection gestures need.
    let idx = resp
        .interact_pointer_pos()
        .map(|pos| hit_char(view, buffer, m, origin, total_rows, wrap_cols, pos));
    let Some(idx) = idx else { return };

    if resp.triple_clicked() {
        view.select_line(buffer, idx);
    } else if resp.double_clicked() {
        view.select_word(buffer, idx);
    } else if resp.dragged() {
        view.drag(idx);
    } else if resp.clicked() {
        view.click(buffer, idx, shift);
    }
}

/// The char index under a screen `pos`, rounded to the nearest glyph boundary so
/// the right half of a glyph lands the caret after it.
#[allow(clippy::too_many_arguments)]
fn hit_char(
    view: &EditorView,
    buffer: &Buffer,
    m: Metrics,
    origin: Pos2,
    total_rows: usize,
    wrap_cols: usize,
    pos: Pos2,
) -> usize {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let vr = (((pos.y - origin.y) / m.row_h).floor().max(0.0) as usize)
        .min(total_rows.saturating_sub(1));
    let row = view.vis_row(buffer, vr, wrap_cols);
    let span = row.end - row.start;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let col = (((pos.x - origin.x - m.gutter_w) / m.glyph_w) + 0.5)
        .floor()
        .max(0.0) as usize;
    row.start + col.min(span)
}

/// Route this frame's key + clipboard events into the view while it is focused.
fn handle_keys(resp: &Response, ui: &Ui, view: &mut EditorView, buffer: &mut Buffer, rows: usize) {
    if resp.clicked() || resp.drag_started() {
        resp.request_focus();
    }
    // A freshly mounted editor grabs the keyboard so typing works immediately
    // (nothing else in the shell body is focused when the surface is shown).
    if ui.memory(|mem| mem.focused().is_none()) {
        resp.request_focus();
    }
    if !resp.has_focus() {
        return;
    }
    // Keep Tab / arrows / Escape as editing keys, not egui focus traversal.
    ui.memory_mut(|mem| {
        mem.set_focus_lock_filter(
            resp.id,
            EventFilter {
                tab: true,
                horizontal_arrows: true,
                vertical_arrows: true,
                escape: true,
            },
        );
    });

    let events = ui.input(|i| i.events.clone());
    for event in &events {
        match event {
            Event::Copy | Event::Cut => {
                if let Some(range) = view.selection() {
                    let text = buffer.rope().slice(range).to_string();
                    ui.ctx().copy_text(text);
                    if matches!(event, Event::Cut) {
                        view.delete(buffer);
                    }
                }
            }
            _ => {
                view.apply_event(buffer, event, rows);
            }
        }
    }
}

/// Paint the visible rows: current-line highlight, selection, gutter numbers,
/// text glyphs, and the blinking caret — all through [`Style`] tokens (§4).
#[allow(clippy::too_many_arguments)]
fn paint(
    ui: &Ui,
    view: &EditorView,
    buffer: &Buffer,
    m: Metrics,
    origin: Pos2,
    first: usize,
    last: usize,
    wrap_cols: usize,
    resp: &Response,
) {
    let clip = ui.clip_rect();
    let painter = ui.painter_at(clip);
    // Text/caret clip: everything right of the pinned gutter, so scrolled text
    // slides under the gutter rather than over it.
    let text_clip = Rect::from_min_max(pos2(clip.left() + m.gutter_w, clip.top()), clip.max);
    let text_painter = painter.with_clip_rect(text_clip);
    let text_x0 = origin.x + m.gutter_w;
    let cursor_line = view.cur_line(buffer);
    let selection = view.selection();

    for vr in first..last {
        let row = view.vis_row(buffer, vr, wrap_cols);
        #[allow(clippy::cast_precision_loss)]
        let y = origin.y + vr as f32 * m.row_h;

        // Current-line highlight (subtle raised fill across the text area).
        if row.line == cursor_line {
            text_painter.rect_filled(
                Rect::from_min_max(pos2(text_clip.left(), y), pos2(clip.right(), y + m.row_h)),
                0.0,
                Style::SURFACE,
            );
        }

        // Selection band for this row.
        if let Some(sel) = &selection {
            paint_selection(&text_painter, &row, sel, text_x0, y, m);
        }

        // Row text (materializes only this slice, never the whole document).
        if row.end > row.start {
            let text = buffer.rope().slice(row.start..row.end).to_string();
            text_painter.text(
                pos2(text_x0, y),
                Align2::LEFT_TOP,
                text,
                body_font(),
                Style::TEXT,
            );
        }
    }

    paint_gutter(&painter, view, buffer, m, origin, clip, first, last, wrap_cols, cursor_line);
    paint_caret(&text_painter, ui, view, buffer, m, origin, wrap_cols, resp);
}

/// Paint the selection band for one visual row, with a trailing hint when the
/// selection continues past this line's end via the newline.
fn paint_selection(
    painter: &egui::Painter,
    row: &VisRow,
    sel: &Range<usize>,
    text_x0: f32,
    y: f32,
    m: Metrics,
) {
    let lo = sel.start.clamp(row.start, row.end);
    let hi = sel.end.clamp(row.start, row.end);
    #[allow(clippy::cast_precision_loss)]
    let left = text_x0 + (lo - row.start) as f32 * m.glyph_w;
    #[allow(clippy::cast_precision_loss)]
    let mut right = text_x0 + (hi - row.start) as f32 * m.glyph_w;
    // Selection crosses this line's break (multi-line select): hint a glyph on
    // the line's last visual row.
    let is_last_row_of_line = row.end == row.line_end;
    let selection_spans_break = sel.end > row.line_end;
    if is_last_row_of_line && selection_spans_break {
        right += m.glyph_w;
    }
    if right > left {
        painter.rect_filled(
            Rect::from_min_max(pos2(left, y), pos2(right, y + m.row_h)),
            0.0,
            Style::ACCENT.gamma_multiply(0.35),
        );
    }
}

/// Paint the pinned line-number gutter for the visible rows.
#[allow(clippy::too_many_arguments)]
fn paint_gutter(
    painter: &egui::Painter,
    view: &EditorView,
    buffer: &Buffer,
    m: Metrics,
    origin: Pos2,
    clip: Rect,
    first: usize,
    last: usize,
    wrap_cols: usize,
    cursor_line: usize,
) {
    // Pin the gutter to the visible left edge even under horizontal scroll.
    let gx = clip.left();
    painter.rect_filled(
        Rect::from_min_max(pos2(gx, clip.top()), pos2(gx + m.gutter_w, clip.bottom())),
        0.0,
        Style::SURFACE,
    );
    painter.vline(
        gx + m.gutter_w,
        clip.y_range(),
        Stroke::new(1.0, Style::BORDER),
    );
    let num_x = gx + m.gutter_w - Style::SP_S;
    for vr in first..last {
        let row = view.vis_row(buffer, vr, wrap_cols);
        if !row.first {
            continue; // wrapped continuation rows carry no number
        }
        #[allow(clippy::cast_precision_loss)]
        let y = origin.y + vr as f32 * m.row_h;
        let color = if row.line == cursor_line {
            Style::TEXT
        } else {
            Style::TEXT_DIM
        };
        painter.text(
            pos2(num_x, y),
            Align2::RIGHT_TOP,
            (row.line + 1).to_string(),
            FontId::monospace(Style::SMALL),
            color,
        );
    }
}

/// Paint the caret: a solid accent beam while focused (blinking on the frame
/// clock), a hollow beam when unfocused.
#[allow(clippy::too_many_arguments)]
fn paint_caret(
    painter: &egui::Painter,
    ui: &Ui,
    view: &EditorView,
    buffer: &Buffer,
    m: Metrics,
    origin: Pos2,
    wrap_cols: usize,
    resp: &Response,
) {
    let (vr, xcol) = view.caret_cell(buffer, wrap_cols);
    #[allow(clippy::cast_precision_loss)]
    let x = origin.x + m.gutter_w + xcol as f32 * m.glyph_w;
    #[allow(clippy::cast_precision_loss)]
    let y = origin.y + vr as f32 * m.row_h;
    let caret = Rect::from_min_size(pos2(x, y), vec2(CARET_W, m.row_h));
    if resp.has_focus() {
        let time = ui.input(|i| i.time);
        if blink_on(time) {
            painter.rect_filled(caret, 0.0, Style::ACCENT);
        }
        // Keep frames coming so the caret actually blinks while idle.
        ui.ctx().request_repaint_after(Duration::from_secs_f64(0.5));
    } else {
        painter.rect_stroke(
            caret,
            0.0,
            Stroke::new(1.0, Style::TEXT_DIM),
            egui::StrokeKind::Middle,
        );
    }
}

/// The caret-blink phase for frame time `time` (seconds): on for the first half
/// of each blink period, off for the second.
#[allow(clippy::cast_possible_truncation)]
fn blink_on(time: f64) -> bool {
    (time * BLINK_HZ).floor() as i64 % 2 == 0
}

/// The gutter width for a buffer of `lines` lines: enough glyphs for the widest
/// line number (min 3 digits) plus a token pad each side.
fn gutter_width(lines: usize, glyph_w: f32) -> f32 {
    let digits = lines.to_string().len().max(3);
    #[allow(clippy::cast_precision_loss)]
    let w = digits as f32 * glyph_w;
    w + Style::SP_S * 2.0
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::{editor_widget, line_len, line_span, word_span, EditorView};
    use crate::buffer::Buffer;
    use mde_egui::egui::{self, pos2, vec2, Event, Key, Modifiers, Rect};
    use mde_egui::Style;

    fn key(key: Key, mods: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: mods,
        }
    }

    fn shift() -> Modifiers {
        Modifiers {
            shift: true,
            ..Modifiers::NONE
        }
    }

    fn cmd() -> Modifiers {
        Modifiers::COMMAND
    }

    // ── cursor movement ──────────────────────────────────────────────────────

    #[test]
    fn arrows_move_the_caret_across_chars_and_lines() {
        let mut buf = Buffer::from_text("ab\ncd");
        let mut view = EditorView::new();
        // Right twice → end of line 0 (col 2).
        view.apply_event(&mut buf, &key(Key::ArrowRight, Modifiers::NONE), 10);
        view.apply_event(&mut buf, &key(Key::ArrowRight, Modifiers::NONE), 10);
        assert_eq!(view.cursor(), 2);
        // Right again crosses the newline into line 1, col 0 (char index 3).
        view.apply_event(&mut buf, &key(Key::ArrowRight, Modifiers::NONE), 10);
        assert_eq!(view.cursor(), 3);
        assert_eq!(view.line_col(&buf), (2, 1));
        // Down from line 1 clamps to the last line; Up returns to line 0.
        view.apply_event(&mut buf, &key(Key::ArrowUp, Modifiers::NONE), 10);
        assert_eq!(view.line_col(&buf).0, 1, "Up moved to line 1 (1-based)");
        // Left underflow saturates at the document start.
        for _ in 0..10 {
            view.apply_event(&mut buf, &key(Key::ArrowLeft, Modifiers::NONE), 10);
        }
        assert_eq!(view.cursor(), 0);
    }

    #[test]
    fn home_end_and_document_extremes() {
        let mut buf = Buffer::from_text("hello\nworld\n!");
        let mut view = EditorView::new();
        // Jump into the middle of line 1.
        view.place_cursor(&buf, 8); // "wo|rld"
        view.apply_event(&mut buf, &key(Key::Home, Modifiers::NONE), 10);
        assert_eq!(view.cursor(), 6, "Home → start of line 1");
        view.apply_event(&mut buf, &key(Key::End, Modifiers::NONE), 10);
        assert_eq!(view.cursor(), 11, "End → end of line 1 (before newline)");
        // Ctrl+Home / Ctrl+End reach the document extremes.
        view.apply_event(&mut buf, &key(Key::Home, cmd()), 10);
        assert_eq!(view.cursor(), 0);
        view.apply_event(&mut buf, &key(Key::End, cmd()), 10);
        assert_eq!(view.cursor(), buf.len_chars());
    }

    #[test]
    fn vertical_motion_keeps_the_goal_column_over_a_short_line() {
        // A long line, a short line, a long line: Down then Down must return to
        // the original column, not stick at the short line's end.
        let mut buf = Buffer::from_text("abcdef\nxy\nabcdef");
        let mut view = EditorView::new();
        view.place_cursor(&buf, 5); // line 0, col 5
        view.apply_event(&mut buf, &key(Key::ArrowDown, Modifiers::NONE), 10);
        assert_eq!(view.line_col(&buf), (2, 3), "clamped to the short line end");
        view.apply_event(&mut buf, &key(Key::ArrowDown, Modifiers::NONE), 10);
        assert_eq!(view.line_col(&buf), (3, 6), "goal column restored on line 2");
    }

    // ── selection ────────────────────────────────────────────────────────────

    #[test]
    fn shift_arrows_extend_and_a_plain_move_drops_the_selection() {
        let mut buf = Buffer::from_text("abcdef");
        let mut view = EditorView::new();
        view.apply_event(&mut buf, &key(Key::ArrowRight, shift()), 10);
        view.apply_event(&mut buf, &key(Key::ArrowRight, shift()), 10);
        assert_eq!(view.selection(), Some(0..2), "shift+right selected 'ab'");
        // A plain move collapses the selection.
        view.apply_event(&mut buf, &key(Key::ArrowRight, Modifiers::NONE), 10);
        assert_eq!(view.selection(), None);
        assert_eq!(view.cursor(), 3);
    }

    #[test]
    fn select_all_spans_the_whole_buffer() {
        let mut buf = Buffer::from_text("abc\ndef");
        let mut view = EditorView::new();
        view.apply_event(&mut buf, &key(Key::A, cmd()), 10);
        assert_eq!(view.selection(), Some(0..buf.len_chars()));
    }

    #[test]
    fn word_and_line_spans_drive_double_and_triple_click() {
        let buf = Buffer::from_text("foo bar_baz  qux\nsecond");
        // Double-click inside "bar_baz" selects the whole identifier (‘_’ is a
        // word char), not just "bar".
        assert_eq!(word_span(&buf, 5), 4..11);
        // A click in the run of spaces selects the whitespace run.
        assert_eq!(word_span(&buf, 11), 11..13);
        // Triple-click selects the logical line without its newline.
        assert_eq!(line_span(&buf, 2), 0..16);
        assert_eq!(line_span(&buf, 20), 17..23, "line 1 span");
    }

    #[test]
    fn select_word_gesture_sets_the_selection() {
        let mut buf = Buffer::from_text("alpha beta");
        let mut view = EditorView::new();
        view.select_word(&mut buf, 7); // inside "beta"
        assert_eq!(view.selection(), Some(6..10));
    }

    // ── edit through the view ────────────────────────────────────────────────

    #[test]
    fn a_text_event_inserts_into_the_real_buffer() {
        let mut buf = Buffer::from_text("");
        let mut view = EditorView::new();
        for ch in ["H", "i"] {
            let changed = view.apply_event(&mut buf, &Event::Text(ch.to_owned()), 10);
            assert!(changed, "a text event reports a change");
        }
        assert_eq!(buf.rope().to_string(), "Hi", "the rope actually changed");
        assert_eq!(view.cursor(), 2, "caret advanced past the inserted text");
        // Enter and Tab are real edits too.
        view.apply_event(&mut buf, &key(Key::Enter, Modifiers::NONE), 10);
        view.apply_event(&mut buf, &key(Key::Tab, Modifiers::NONE), 10);
        assert_eq!(buf.rope().to_string(), "Hi\n    ");
    }

    #[test]
    fn typing_over_a_selection_replaces_it() {
        let mut buf = Buffer::from_text("abcdef");
        let mut view = EditorView::new();
        view.apply_event(&mut buf, &key(Key::A, cmd()), 10); // select all
        view.apply_event(&mut buf, &Event::Text("X".to_owned()), 10);
        assert_eq!(buf.rope().to_string(), "X");
        assert_eq!(view.cursor(), 1);
    }

    #[test]
    fn backspace_deletes_the_selection_then_falls_back_to_one_char() {
        let mut buf = Buffer::from_text("abcdef");
        let mut view = EditorView::new();
        view.place_cursor(&buf, 1);
        view.apply_event(&mut buf, &key(Key::ArrowRight, shift()), 10);
        view.apply_event(&mut buf, &key(Key::ArrowRight, shift()), 10);
        assert_eq!(view.selection(), Some(1..3));
        // Delete-selection removes "bc".
        view.apply_event(&mut buf, &key(Key::Backspace, Modifiers::NONE), 10);
        assert_eq!(buf.rope().to_string(), "adef");
        assert_eq!(view.selection(), None);
        // With no selection, Backspace deletes the char before the caret ("a").
        view.apply_event(&mut buf, &key(Key::Backspace, Modifiers::NONE), 10);
        assert_eq!(buf.rope().to_string(), "def");
    }

    #[test]
    fn forward_delete_removes_the_char_at_the_caret() {
        let mut buf = Buffer::from_text("abc");
        let mut view = EditorView::new();
        view.apply_event(&mut buf, &key(Key::Delete, Modifiers::NONE), 10);
        assert_eq!(buf.rope().to_string(), "bc");
        assert_eq!(view.cursor(), 0);
    }

    // ── undo / redo via the view ─────────────────────────────────────────────

    #[test]
    fn undo_and_redo_run_through_the_view_and_restore_the_caret() {
        let mut buf = Buffer::from_text("");
        let mut view = EditorView::new();
        for ch in ["h", "i"] {
            view.apply_event(&mut buf, &Event::Text(ch.to_owned()), 10);
        }
        assert_eq!(buf.rope().to_string(), "hi");
        // Ctrl+Z undoes the whole coalesced type run and restores the caret to 0.
        view.apply_event(&mut buf, &key(Key::Z, cmd()), 10);
        assert_eq!(buf.rope().to_string(), "");
        assert_eq!(view.cursor(), 0, "undo restored the pre-run caret hint");
        // Ctrl+Y redoes it, caret back at the end.
        view.apply_event(&mut buf, &key(Key::Y, cmd()), 10);
        assert_eq!(buf.rope().to_string(), "hi");
        assert_eq!(view.cursor(), 2);
        // Ctrl+Shift+Z is redo too (round-trips after an undo).
        view.apply_event(&mut buf, &key(Key::Z, cmd()), 10);
        assert_eq!(buf.rope().to_string(), "");
        let mut redo_mods = cmd();
        redo_mods.shift = true;
        view.apply_event(&mut buf, &key(Key::Z, redo_mods), 10);
        assert_eq!(buf.rope().to_string(), "hi");
    }

    // ── pure helpers ─────────────────────────────────────────────────────────

    #[test]
    fn line_len_excludes_the_trailing_newline() {
        let buf = Buffer::from_text("abc\n\nxy\n");
        assert_eq!(line_len(&buf, 0), 3);
        assert_eq!(line_len(&buf, 1), 0, "a blank line has zero length");
        assert_eq!(line_len(&buf, 2), 2);
        assert_eq!(line_len(&buf, 3), 0, "the final empty line");
    }

    // ── headless render ──────────────────────────────────────────────────────

    /// The widget mounts + tessellates over a seeded buffer: run one real egui
    /// frame through `editor_widget` on the CPU (no GPU) and assert it produces
    /// draw primitives — proof the gutter/text/caret actually paint (runtime-
    /// reachable, not a mockup).
    #[test]
    fn widget_paints_non_empty_primitives_over_a_seeded_buffer() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut buf = Buffer::from_text("fn main() {\n    println!(\"hi\");\n}\n");
        let mut view = EditorView::new();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                editor_widget(ui, &mut view, &mut buf);
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "the editor widget produced no primitives");
    }

    #[test]
    fn wrap_toggle_switches_and_still_paints() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        // A line far longer than the viewport, to exercise the wrap path.
        let mut buf = Buffer::from_text(&"word ".repeat(400));
        let mut view = EditorView::new();
        assert!(!view.wrap());
        view.toggle_wrap();
        assert!(view.wrap());
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                editor_widget(ui, &mut view, &mut buf);
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "the wrapped editor produced no primitives");
        // The wrap map saw more visual rows than the single logical line.
        assert!(
            view.wrap_map.total() > 1,
            "a long line wrapped into multiple visual rows"
        );
    }
}
