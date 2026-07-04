//! The **custom code-editor text widget** (EDITOR-3/4): the immediate-mode egui
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
//!   knowing — the carets (each a char index into the rope plus a selection
//!   anchor and a sticky goal column), the primary caret, the soft-wrap toggle,
//!   and a widget-level undo log. It carries the *pure* cursor-movement,
//!   selection, and edit-application logic so those are unit-testable **without**
//!   a live egui frame (the `EditorView::*` methods below take a `&Buffer`/`&mut
//!   Buffer` and a synthetic [`egui::Event`], never a `Ui`).
//! * [`editor_widget`] is the one egui entry point: it lays the view out inside a
//!   scroll area, maps the pointer to a rope char index through the monospace
//!   glyph metrics (click / drag / double- / triple-click / Alt-click / Alt-drag),
//!   routes this frame's key events into the view, and paints the gutter + text +
//!   selection + carets through the shared Carbon [`Style`] tokens (§4 — no raw
//!   hex, no scattered metric).
//!
//! Multi-cursor + column selection land here (EDITOR-4): the single `(caret,
//! anchor)` generalizes to a `Vec` of carets, every edit fans out across all of
//! them, and overlapping carets merge.
//!
//! Syntax highlighting lands here too (EDITOR-5): [`editor_widget`] takes the
//! document's optional [`Highlighter`], syncs it with this frame's edits
//! (incremental re-parse via the buffer's edit deltas), resolves the **visible**
//! window's [`HighlightSpan`]s once, and the row paint draws each line span by
//! span in its Carbon code-token color ([`mde_egui::code`], §4) — viewport
//! culling intact, only on-screen glyphs get styled draws.

// `EditorView` is the domain name for this module's widget-state type; renaming
// it to dodge the `widget` echo would be worse (the same call `buffer.rs` makes).
// `missing_const_for_fn` (nursery) is over-eager for small mutators whose
// const-ness we don't want to pin into the public contract. `suboptimal_flops`
// is allowed for the layout arithmetic: `origin + col * glyph_w` reads far
// clearer than the `mul_add` rewrite, and the precision/throughput gain is
// irrelevant for a few pixel positions per row (same rationale + repo precedent
// as `mde-mesh-view` / `mde-panel-egui`). The cast lints are allowed
// module-wide: the multi-cursor geometry + fan-out edit arithmetic convert
// between char indices (`usize`), signed shift accumulators (`isize`), and
// row/column-to-pixel offsets (`f32`); every conversion is bounded by the
// document size, so the precision/truncation/sign/wrap lints are noise here
// (this generalizes the inline allows EDITOR-3 already carried at each site).
#![allow(
    clippy::module_name_repetitions,
    clippy::missing_const_for_fn,
    clippy::suboptimal_flops,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

use std::ops::Range;
use std::time::Duration;

use mde_egui::egui::{
    self, pos2, vec2, Align2, Color32, Event, EventFilter, FontId, Key, Modifiers, Pos2, Rect,
    Response, ScrollArea, Sense, Stroke, Ui,
};
use mde_egui::Style;

use crate::buffer::Buffer;
use crate::highlight::{HighlightSpan, Highlighter};

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

/// The next occurrence of `needle` (a char slice) at or after `from`, wrapping
/// past the end back to the document start — the `Ctrl`+D "add cursor at next
/// match" search. Reads the rope char-by-char (O(log n) each), so it never
/// materializes the whole document; `needle` is a short selection, so the
/// naive scan is cheap for a user-driven gesture.
fn find_next(buffer: &Buffer, needle: &[char], from: usize) -> Option<usize> {
    let n = buffer.len_chars();
    let m = needle.len();
    if m == 0 || m > n {
        return None;
    }
    let rope = buffer.rope();
    let last = n - m;
    let matches = |start: usize| (0..m).all(|k| rope.char(start + k) == needle[k]);
    (from..=last)
        .chain(0..from.min(last + 1))
        .find(|&start| matches(start))
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

/// One caret in the view: a char index into the rope plus a selection anchor and
/// a sticky goal column. A single-cursor editor is exactly a one-element
/// [`EditorView`] caret vec; multi-cursor (EDITOR-4) grows it.
///
/// The cursor and anchor are **char indices** into the rope (not `(line, col)`),
/// so they compose directly with `Buffer::insert`/`remove`.
#[derive(Clone, Copy)]
struct Caret {
    /// Caret position — a char index into the rope.
    cursor: usize,
    /// Selection anchor — the fixed end of the selection, or `None` when there is
    /// no selection. The selection is always `min(anchor, cursor)..max(..)`.
    anchor: Option<usize>,
    /// Sticky column for vertical motion: set on Up/Down/PageUp/Down and kept
    /// across a run of them so the caret tracks the same column over short lines,
    /// cleared by any horizontal move or edit.
    goal_col: Option<usize>,
}

impl Caret {
    /// A bare caret at `cursor` with no selection and no goal column.
    const fn at(cursor: usize) -> Self {
        Self {
            cursor,
            anchor: None,
            goal_col: None,
        }
    }

    /// This caret's selection as a char range, or `None` when nothing is selected
    /// (no anchor, or the anchor coincides with the cursor).
    fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.anchor?;
        let (lo, hi) = (anchor.min(self.cursor), anchor.max(self.cursor));
        (lo < hi).then_some(lo..hi)
    }

    /// The `[lo, hi]` char span this caret occupies — its selection, or the caret
    /// point (`lo == hi`) with no selection. Drives overlap-merge + the fan-out
    /// edit order.
    fn span(&self) -> (usize, usize) {
        self.anchor.map_or((self.cursor, self.cursor), |a| {
            (a.min(self.cursor), a.max(self.cursor))
        })
    }

    /// Move this caret to `new`, extending the selection when `extend` (Shift):
    /// the first extend drops an anchor at the old cursor; a non-extend move
    /// clears the selection.
    fn move_to(&mut self, new: usize, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = new;
    }
}

/// Which kind of edit a widget undo step coalesces with: a run of same-kind
/// single-caret edits (typing, or a run of deletions) merges into one undo step,
/// matching the EDITOR-3 buffer grouping semantics.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Insert,
    Delete,
}

/// One widget-level undo step: how many buffer groups it spans (so undo/redo can
/// unwind them together) plus the full caret set before and after, so undo
/// restores **every** caret of a fan-out edit — one step per multi-caret edit.
struct LogEntry {
    /// Number of buffer undo groups this step spans (one per buffer mutation; a
    /// fan-out edit spans several).
    groups: usize,
    /// The caret set + primary index before the edit (restored on undo).
    before: (Vec<Caret>, usize),
    /// The caret set + primary index after the edit (restored on redo).
    after: (Vec<Caret>, usize),
}

/// The widget state for one open document (EDITOR-3/4).
///
/// Holds the carets, the primary caret, the vertical-motion goal columns, the
/// soft-wrap toggle, and a widget-level undo log — everything the pure
/// [`Buffer`](crate::buffer::Buffer) does not itself track.
///
/// All movement, selection, and edit-application logic lives here as
/// `&Buffer`/`&mut Buffer` methods so it is unit-testable without a live egui
/// frame. Every edit **fans out** across all carets (highest-index-safe via an
/// ascending offset accumulator); overlapping carets **merge**; `Esc` collapses
/// to the single primary caret.
pub struct EditorView {
    /// The carets, normalized (sorted + merged) after each gesture. Always
    /// non-empty; a single-cursor view is a one-element vec (all EDITOR-3
    /// behavior + tests). Every edit fans out across all of them.
    carets: Vec<Caret>,
    /// Index of the **primary** caret — the one whose viewport-reveal + status
    /// line the surface honors (EDITOR-3's single caret).
    primary: usize,
    /// Soft-wrap toggle: on wraps long lines to the viewport (no horizontal
    /// scroll), off keeps lines unwrapped and scrolls horizontally.
    wrap: bool,
    /// Widest line seen so far, in chars — the horizontal-scroll extent when
    /// unwrapped. Grows only (a cheap monotonic estimate; a precise re-measure
    /// lands with the highlighter pass), so the scrollbar never jitters.
    max_line_chars: usize,
    /// Cached wrap prefix sums, rebuilt lazily when wrap is on.
    wrap_map: WrapMap,
    /// Set by any caret move/edit; consumed by the renderer to scroll the primary
    /// caret back into view exactly once (so it doesn't fight the user's scroll).
    reveal_caret: bool,
    /// Widget-level undo stack (newest last). Groups a whole fan-out edit into
    /// one step so undo/redo restore every caret; single-caret runs coalesce.
    undo_log: Vec<LogEntry>,
    /// Undone steps available for redo.
    redo_log: Vec<LogEntry>,
    /// Whether the newest [`LogEntry`] still accepts a coalescing same-kind edit.
    group_open: bool,
    /// The kind of the newest logged edit, for coalescing.
    last_kind: Option<EditKind>,
    /// Snapshot captured at the start of the in-flight edit (its before-state).
    edit_before: Option<(Vec<Caret>, usize)>,
    /// Anchor cell `(visual row, column)` of an in-progress Alt+drag column
    /// (box) selection, or `None` when no box drag is active.
    box_anchor: Option<(usize, usize)>,
}

impl Default for EditorView {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorView {
    /// A fresh view over a document: one caret at the top, no selection, wrap off.
    #[must_use]
    pub fn new() -> Self {
        Self {
            carets: vec![Caret::at(0)],
            primary: 0,
            wrap: false,
            max_line_chars: 0,
            wrap_map: WrapMap::default(),
            reveal_caret: false,
            undo_log: Vec::new(),
            redo_log: Vec::new(),
            group_open: false,
            last_kind: None,
            edit_before: None,
            box_anchor: None,
        }
    }

    /// The primary caret (shared read seam for the EDITOR-3 accessors).
    fn primary_caret(&self) -> &Caret {
        &self.carets[self.primary.min(self.carets.len().saturating_sub(1))]
    }

    /// The primary caret's char index into the rope.
    #[must_use]
    pub fn cursor(&self) -> usize {
        self.primary_caret().cursor
    }

    /// The primary caret's selection as a char range, or `None` when nothing is
    /// selected (no anchor, or the anchor coincides with the caret).
    #[must_use]
    pub fn selection(&self) -> Option<Range<usize>> {
        self.primary_caret().selection()
    }

    /// Every caret's non-empty selection, sorted by start — the copy/cut source
    /// and the render's selection bands.
    fn selections(&self) -> Vec<Range<usize>> {
        let mut v: Vec<Range<usize>> = self.carets.iter().filter_map(Caret::selection).collect();
        v.sort_by_key(|r| r.start);
        v
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

    /// The primary caret's 1-based `(line, column)` for the status strip.
    #[must_use]
    pub fn line_col(&self, buffer: &Buffer) -> (usize, usize) {
        let cursor = self.primary_caret().cursor.min(buffer.len_chars());
        let line = buffer.char_to_line(cursor);
        (line + 1, cursor - buffer.line_to_char(line) + 1)
    }

    /// Place a single caret at char index `idx` (clamped), dropping any other
    /// carets + selection — the seam a Files-send / finder jump (EDITOR-7/9) uses
    /// to reveal a location.
    pub fn place_cursor(&mut self, buffer: &Buffer, idx: usize) {
        self.carets = vec![Caret::at(idx.min(buffer.len_chars()))];
        self.primary = 0;
        self.group_open = false;
        self.box_anchor = None;
        self.reveal_caret = true;
    }

    /// Clamp every caret back inside the buffer (called each frame in case the
    /// rope shrank underneath the view) and keep the primary index valid.
    fn clamp(&mut self, buffer: &Buffer) {
        let len = buffer.len_chars();
        for c in &mut self.carets {
            c.cursor = c.cursor.min(len);
            if let Some(a) = c.anchor {
                c.anchor = Some(a.min(len));
            }
        }
        if self.carets.is_empty() {
            self.carets.push(Caret::at(0));
        }
        if self.primary >= self.carets.len() {
            self.primary = self.carets.len() - 1;
        }
    }

    // ── caret geometry ──────────────────────────────────────────────────────

    /// The primary caret's logical line.
    fn cur_line(&self, buffer: &Buffer) -> usize {
        buffer.char_to_line(self.primary_caret().cursor.min(buffer.len_chars()))
    }

    // ── normalization (merge overlapping carets) ─────────────────────────────

    /// Sort the carets by span start and merge any that overlap or touch, so a
    /// gesture that runs two carets together leaves exactly one. A no-op for a
    /// single caret (so EDITOR-3's reverse-oriented selections keep their
    /// direction); the primary is re-found by its cursor position afterward.
    fn normalize(&mut self) {
        if self.carets.len() <= 1 {
            return;
        }
        let pc = self.carets[self.primary.min(self.carets.len() - 1)].cursor;
        self.carets.sort_by_key(|c| c.span().0);
        let mut merged: Vec<Caret> = Vec::with_capacity(self.carets.len());
        for c in std::mem::take(&mut self.carets) {
            if let Some(prev) = merged.last_mut() {
                let (plo, phi) = prev.span();
                let (clo, chi) = c.span();
                if clo <= phi {
                    // Overlap or touch → fuse into the previous caret's span.
                    let nlo = plo.min(clo);
                    let nhi = phi.max(chi);
                    if nhi > nlo {
                        prev.anchor = Some(nlo);
                        prev.cursor = nhi;
                    } else {
                        prev.anchor = None;
                        prev.cursor = nlo;
                    }
                    prev.goal_col = None;
                    continue;
                }
            }
            merged.push(c);
        }
        self.carets = merged;
        self.primary = self
            .carets
            .iter()
            .position(|c| {
                let (lo, hi) = c.span();
                pc >= lo && pc <= hi
            })
            .unwrap_or(0);
    }

    // ── widget-level undo log ────────────────────────────────────────────────

    /// A deep snapshot of the caret set + primary index.
    fn snapshot(&self) -> (Vec<Caret>, usize) {
        (self.carets.clone(), self.primary)
    }

    /// Capture the before-state at the start of an edit (once per edit).
    fn begin_edit(&mut self) {
        if self.edit_before.is_none() {
            self.edit_before = Some(self.snapshot());
        }
    }

    /// Close out an edit: record `groups` buffer groups + the caret before/after
    /// as one undo step, coalescing a same-kind single-caret run into the open
    /// step so a type run undoes at once. A zero-group (no-op) edit records
    /// nothing. A `multi` edit always starts its own closed step.
    fn finish_edit(&mut self, groups: usize, kind: EditKind, multi: bool) {
        let before = self.edit_before.take();
        if groups == 0 {
            return;
        }
        self.reveal_caret = true;
        self.redo_log.clear();
        let after = self.snapshot();
        if !multi && self.group_open && self.last_kind == Some(kind) {
            if let Some(e) = self.undo_log.last_mut() {
                e.groups += groups;
                e.after = after;
                return;
            }
        }
        self.undo_log.push(LogEntry {
            groups,
            before: before.unwrap_or_else(|| after.clone()),
            after,
        });
        self.group_open = !multi;
        self.last_kind = Some(kind);
    }

    /// The ascending fan-out order: caret indices sorted by their span start, so
    /// an edit walks left-to-right accumulating a signed offset.
    fn fanout_order(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.carets.len()).collect();
        order.sort_by_key(|&i| self.carets[i].span().0);
        order
    }

    // ── edits (selection-aware, fan out across every caret) ──────────────────

    /// Insert `text` at every caret, replacing each caret's selection first. Walks
    /// carets left-to-right with a running offset so earlier edits don't
    /// invalidate later carets' indices; each buffer op is its own group so the
    /// whole fan-out undoes as one widget step.
    fn insert(&mut self, buffer: &mut Buffer, text: &str) {
        let multi = self.carets.len() > 1;
        self.begin_edit();
        let l = text.chars().count();
        let order = self.fanout_order();
        let mut shift: isize = 0;
        let mut groups = 0usize;
        for &i in &order {
            let (s, e) = self.carets[i].span();
            let s2 = (s as isize + shift) as usize;
            let e2 = (e as isize + shift) as usize;
            if e2 > s2 {
                buffer.remove(s2..e2);
                buffer.commit_group();
                groups += 1;
            }
            if l > 0 {
                buffer.insert(s2, text);
                buffer.commit_group();
                groups += 1;
            }
            self.carets[i].cursor = s2 + l;
            self.carets[i].anchor = None;
            self.carets[i].goal_col = None;
            shift += l as isize - (e as isize - s as isize);
        }
        self.normalize();
        self.finish_edit(groups, EditKind::Insert, multi);
    }

    /// Backspace at every caret: delete each caret's selection if any, else the
    /// char before it. Fan-out with a running offset.
    fn backspace(&mut self, buffer: &mut Buffer) {
        let multi = self.carets.len() > 1;
        self.begin_edit();
        let order = self.fanout_order();
        let mut shift: isize = 0;
        let mut groups = 0usize;
        for &i in &order {
            let (s, e) = self.carets[i].span();
            if e > s {
                let s2 = (s as isize + shift) as usize;
                let e2 = (e as isize + shift) as usize;
                buffer.remove(s2..e2);
                buffer.commit_group();
                groups += 1;
                self.carets[i].cursor = s2;
                shift -= (e - s) as isize;
            } else {
                let c2 = (self.carets[i].cursor as isize + shift) as usize;
                if c2 > 0 {
                    buffer.remove(c2 - 1..c2);
                    buffer.commit_group();
                    groups += 1;
                    self.carets[i].cursor = c2 - 1;
                    shift -= 1;
                } else {
                    self.carets[i].cursor = c2;
                }
            }
            self.carets[i].anchor = None;
            self.carets[i].goal_col = None;
        }
        self.normalize();
        self.finish_edit(groups, EditKind::Delete, multi);
    }

    /// Forward-delete at every caret: delete each caret's selection if any, else
    /// the char at it. Fan-out with a running offset.
    fn delete(&mut self, buffer: &mut Buffer) {
        let multi = self.carets.len() > 1;
        self.begin_edit();
        let order = self.fanout_order();
        let mut shift: isize = 0;
        let mut groups = 0usize;
        for &i in &order {
            let (s, e) = self.carets[i].span();
            if e > s {
                let s2 = (s as isize + shift) as usize;
                let e2 = (e as isize + shift) as usize;
                buffer.remove(s2..e2);
                buffer.commit_group();
                groups += 1;
                self.carets[i].cursor = s2;
                shift -= (e - s) as isize;
            } else {
                let c2 = (self.carets[i].cursor as isize + shift) as usize;
                if c2 < buffer.len_chars() {
                    buffer.remove(c2..c2 + 1);
                    buffer.commit_group();
                    groups += 1;
                    shift -= 1;
                }
                self.carets[i].cursor = c2;
            }
            self.carets[i].anchor = None;
            self.carets[i].goal_col = None;
        }
        self.normalize();
        self.finish_edit(groups, EditKind::Delete, multi);
    }

    /// Delete only the carets' selections (no char fallback) — the Cut edit. A
    /// caret with no selection is left in place.
    fn delete_selections(&mut self, buffer: &mut Buffer) {
        let multi = self.carets.len() > 1;
        self.begin_edit();
        let order = self.fanout_order();
        let mut shift: isize = 0;
        let mut groups = 0usize;
        for &i in &order {
            let (s, e) = self.carets[i].span();
            if e > s {
                let s2 = (s as isize + shift) as usize;
                let e2 = (e as isize + shift) as usize;
                buffer.remove(s2..e2);
                buffer.commit_group();
                groups += 1;
                self.carets[i].cursor = s2;
                shift -= (e - s) as isize;
            } else {
                self.carets[i].cursor = (self.carets[i].cursor as isize + shift) as usize;
            }
            self.carets[i].anchor = None;
            self.carets[i].goal_col = None;
        }
        self.normalize();
        self.finish_edit(groups, EditKind::Delete, multi);
    }

    /// Select the whole buffer (collapses to one caret).
    fn select_all(&mut self, buffer: &Buffer) {
        self.carets = vec![Caret {
            cursor: buffer.len_chars(),
            anchor: Some(0),
            goal_col: None,
        }];
        self.primary = 0;
        self.group_open = false;
        self.reveal_caret = true;
    }

    /// Undo one widget step, unwinding its buffer groups and restoring every
    /// caret. Returns whether it changed anything.
    fn undo(&mut self, buffer: &mut Buffer) -> bool {
        buffer.commit_group();
        let Some(entry) = self.undo_log.pop() else {
            return false;
        };
        for _ in 0..entry.groups {
            buffer.undo();
        }
        self.carets.clone_from(&entry.before.0);
        self.primary = entry.before.1.min(self.carets.len().saturating_sub(1));
        self.redo_log.push(entry);
        self.group_open = false;
        self.last_kind = None;
        self.reveal_caret = true;
        true
    }

    /// Redo one widget step, re-applying its buffer groups and restoring every
    /// caret. Returns whether it changed anything.
    fn redo(&mut self, buffer: &mut Buffer) -> bool {
        buffer.commit_group();
        let Some(entry) = self.redo_log.pop() else {
            return false;
        };
        for _ in 0..entry.groups {
            buffer.redo();
        }
        self.carets.clone_from(&entry.after.0);
        self.primary = entry.after.1.min(self.carets.len().saturating_sub(1));
        self.undo_log.push(entry);
        self.group_open = false;
        self.last_kind = None;
        self.reveal_caret = true;
        true
    }

    // ── multi-cursor gestures ────────────────────────────────────────────────

    /// Add a caret one logical line above (`delta = -1`) or below (`delta = +1`)
    /// the primary, at its goal column, and make the new caret primary — the
    /// `Ctrl`+`Alt`+Up/Down "add cursor above/below". Returns `false` when there
    /// is no room (already at the document edge).
    fn add_caret_vertical(&mut self, buffer: &mut Buffer, delta: isize) -> bool {
        buffer.commit_group();
        self.group_open = false;
        let len = buffer.len_chars();
        let p = *self.primary_caret();
        let line = buffer.char_to_line(p.cursor.min(len));
        let col = p.cursor - buffer.line_to_char(line);
        let goal = p.goal_col.unwrap_or(col);
        let max_line = buffer.len_lines().saturating_sub(1);
        let tline = (line as isize + delta).clamp(0, max_line as isize) as usize;
        if tline == line {
            return false;
        }
        let mut nc = Caret::at(char_at(buffer, tline, goal));
        nc.goal_col = Some(goal);
        self.carets.push(nc);
        self.primary = self.carets.len() - 1;
        self.reveal_caret = true;
        self.normalize();
        true
    }

    /// `Ctrl`+D: the first press with no selection selects the word under the
    /// primary caret; each later press adds a caret selecting the **next**
    /// occurrence of the primary selection (wrapping), and makes it primary.
    fn add_next_match(&mut self, buffer: &mut Buffer) -> bool {
        buffer.commit_group();
        self.group_open = false;
        let p = *self.primary_caret();
        let (needle, from): (Vec<char>, usize) = if let Some(sel) = p.selection() {
            (buffer.rope().slice(sel.clone()).chars().collect(), sel.end)
        } else {
            let span = word_span(buffer, p.cursor);
            if span.start >= span.end {
                return false;
            }
            self.carets[self.primary].anchor = Some(span.start);
            self.carets[self.primary].cursor = span.end;
            self.reveal_caret = true;
            return true;
        };
        if needle.is_empty() {
            return false;
        }
        let m = needle.len();
        let Some(start) = find_next(buffer, &needle, from) else {
            return false;
        };
        let new_span = (start, start + m);
        if self.carets.iter().any(|c| c.span() == new_span) {
            // Every match already has a caret; nothing new to add.
            return true;
        }
        let mut nc = Caret::at(start + m);
        nc.anchor = Some(start);
        self.carets.push(nc);
        self.primary = self.carets.len() - 1;
        self.reveal_caret = true;
        self.normalize();
        true
    }

    /// Toggle a caret at char `idx` (Alt-click): remove the caret whose span
    /// covers `idx` if there is one (never the last caret), else add a bare caret
    /// there and make it primary.
    fn toggle_caret_at(&mut self, buffer: &mut Buffer, idx: usize) {
        buffer.commit_group();
        self.group_open = false;
        self.box_anchor = None;
        if let Some(pos) = self.carets.iter().position(|c| {
            let (lo, hi) = c.span();
            idx >= lo && idx <= hi
        }) {
            if self.carets.len() > 1 {
                self.carets.remove(pos);
                self.primary = self.primary.min(self.carets.len() - 1);
            }
            // A lone caret at `idx` stays — the view always keeps ≥ 1 caret.
        } else {
            self.carets.push(Caret::at(idx));
            self.primary = self.carets.len() - 1;
        }
        self.reveal_caret = true;
        self.normalize();
    }

    /// Column (box) selection: replace the carets with one per visual row from
    /// `r0..=r1`, each selecting the `c0..c1` column band (clamped to the row's
    /// content) — the Alt+drag gesture. The drag-end row is primary.
    fn column_select(
        &mut self,
        buffer: &Buffer,
        r0: usize,
        c0: usize,
        r1: usize,
        c1: usize,
        cols: usize,
    ) {
        let (rlo, rhi) = (r0.min(r1), r0.max(r1));
        let (clo, chi) = (c0.min(c1), c0.max(c1));
        let mut carets = Vec::with_capacity(rhi - rlo + 1);
        for vr in rlo..=rhi {
            let row = self.vis_row(buffer, vr, cols);
            let rlen = row.end - row.start;
            let a = row.start + clo.min(rlen);
            let b = row.start + chi.min(rlen);
            let mut caret = Caret::at(b);
            if b > a {
                caret.anchor = Some(a);
            }
            carets.push(caret);
        }
        if carets.is_empty() {
            return;
        }
        self.primary = r1.saturating_sub(rlo).min(carets.len() - 1);
        self.carets = carets;
        self.group_open = false;
        self.reveal_caret = true;
    }

    /// Collapse to the single primary caret (`Esc`), dropping the rest and any
    /// selection; if already single, clear its selection (EDITOR-3). Returns
    /// whether anything changed.
    fn collapse(&mut self) -> bool {
        if self.carets.len() > 1 {
            let cursor = self.primary_caret().cursor;
            self.carets = vec![Caret::at(cursor)];
            self.primary = 0;
            self.group_open = false;
            self.reveal_caret = true;
            true
        } else if self.carets[0].anchor.take().is_some() {
            self.reveal_caret = true;
            true
        } else {
            false
        }
    }

    // ── pointer selection ───────────────────────────────────────────────────

    /// Click at char `idx`: place a single caret (extend from the primary on
    /// Shift-click), closing the current undo group so a later type run starts
    /// fresh.
    fn click(&mut self, buffer: &mut Buffer, idx: usize, extend: bool) {
        buffer.commit_group();
        self.group_open = false;
        self.box_anchor = None;
        let caret = if extend {
            let mut c = *self.primary_caret();
            c.goal_col = None;
            c.move_to(idx, true);
            c
        } else {
            Caret::at(idx)
        };
        self.carets = vec![caret];
        self.primary = 0;
        self.reveal_caret = true;
    }

    /// Drag to char `idx`: extend a single selection from the drag's anchor.
    fn drag(&mut self, idx: usize) {
        let mut caret = *self.primary_caret();
        caret.goal_col = None;
        caret.move_to(idx, true);
        self.carets = vec![caret];
        self.primary = 0;
        self.reveal_caret = true;
    }

    /// Double-click: select the word under `idx` (collapses to one caret).
    fn select_word(&mut self, buffer: &mut Buffer, idx: usize) {
        buffer.commit_group();
        self.group_open = false;
        self.box_anchor = None;
        let span = word_span(buffer, idx);
        self.carets = vec![Caret {
            cursor: span.end,
            anchor: Some(span.start),
            goal_col: None,
        }];
        self.primary = 0;
        self.reveal_caret = true;
    }

    /// Triple-click: select the logical line under `idx` (collapses to one caret).
    fn select_line(&mut self, buffer: &mut Buffer, idx: usize) {
        buffer.commit_group();
        self.group_open = false;
        self.box_anchor = None;
        let span = line_span(buffer, idx);
        self.carets = vec![Caret {
            cursor: span.end,
            anchor: Some(span.start),
            goal_col: None,
        }];
        self.primary = 0;
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

    /// The key half of [`apply_event`](Self::apply_event): motion, editing,
    /// multi-cursor, and undo/redo. `Ctrl`/`Cmd` is `modifiers.command`; `Shift`
    /// extends selection; `Ctrl`+`Alt`+Up/Down add carets; `Ctrl`+D adds the next
    /// match.
    #[allow(clippy::too_many_lines)]
    fn apply_key(&mut self, buffer: &mut Buffer, key: Key, mods: Modifiers, rows: usize) -> bool {
        let shift = mods.shift;
        let cmd = mods.command;
        let alt = mods.alt;
        match key {
            // ── horizontal motion (fans across every caret) ──
            Key::ArrowLeft => self.move_horizontal(buffer, cmd, shift, false),
            Key::ArrowRight => self.move_horizontal(buffer, cmd, shift, true),
            Key::Home => {
                buffer.commit_group();
                self.group_open = false;
                let len = buffer.len_chars();
                for c in &mut self.carets {
                    c.goal_col = None;
                    let line = buffer.char_to_line(c.cursor.min(len));
                    let t = if cmd { 0 } else { buffer.line_to_char(line) };
                    c.move_to(t, shift);
                }
                self.reveal_caret = true;
                self.normalize();
                true
            }
            Key::End => {
                buffer.commit_group();
                self.group_open = false;
                let len = buffer.len_chars();
                for c in &mut self.carets {
                    c.goal_col = None;
                    let line = buffer.char_to_line(c.cursor.min(len));
                    let t = if cmd {
                        len
                    } else {
                        buffer.line_to_char(line) + line_len(buffer, line)
                    };
                    c.move_to(t, shift);
                }
                self.reveal_caret = true;
                self.normalize();
                true
            }
            // ── vertical motion / add-cursor above/below ──
            Key::ArrowUp if cmd && alt => self.add_caret_vertical(buffer, -1),
            Key::ArrowDown if cmd && alt => self.add_caret_vertical(buffer, 1),
            Key::ArrowUp => self.vertical(buffer, -1, shift),
            Key::ArrowDown => self.vertical(buffer, 1, shift),
            Key::PageUp => self.vertical(buffer, -(rows.max(1) as isize), shift),
            Key::PageDown => self.vertical(buffer, rows.max(1) as isize, shift),
            // ── editing (fans out) ──
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
            // ── selection / multi-cursor / history ──
            Key::A if cmd => {
                self.select_all(buffer);
                true
            }
            Key::D if cmd => self.add_next_match(buffer),
            Key::Z if cmd && shift => self.redo(buffer),
            Key::Z if cmd => self.undo(buffer),
            Key::Y if cmd => self.redo(buffer),
            Key::Escape => self.collapse(),
            _ => false,
        }
    }

    /// Horizontal motion for every caret: by word when `cmd`, extending on
    /// `shift`, in the `forward` direction. Always reports a change.
    fn move_horizontal(
        &mut self,
        buffer: &mut Buffer,
        cmd: bool,
        shift: bool,
        forward: bool,
    ) -> bool {
        buffer.commit_group();
        self.group_open = false;
        let len = buffer.len_chars();
        for c in &mut self.carets {
            c.goal_col = None;
            let t = if forward {
                if cmd {
                    next_word(buffer, c.cursor)
                } else {
                    (c.cursor + 1).min(len)
                }
            } else if cmd {
                prev_word(buffer, c.cursor)
            } else {
                c.cursor.saturating_sub(1)
            };
            c.move_to(t, shift);
        }
        self.reveal_caret = true;
        self.normalize();
        true
    }

    /// Vertical caret motion by `delta` lines for every caret, each preserving its
    /// own goal column so a run of Up/Down tracks the same column across short
    /// lines. Always returns `true`.
    fn vertical(&mut self, buffer: &mut Buffer, delta: isize, extend: bool) -> bool {
        buffer.commit_group();
        self.group_open = false;
        let max_line = buffer.len_lines().saturating_sub(1);
        let len = buffer.len_chars();
        for c in &mut self.carets {
            let line = buffer.char_to_line(c.cursor.min(len));
            let col = c.cursor - buffer.line_to_char(line);
            let goal = c.goal_col.unwrap_or(col);
            let target_line = (line as isize + delta).clamp(0, max_line as isize) as usize;
            let target = char_at(buffer, target_line, goal);
            c.move_to(target, extend);
            c.goal_col = Some(goal);
        }
        self.reveal_caret = true;
        self.normalize();
        true
    }

    // ── egui frame ──────────────────────────────────────────────────────────

    /// Resolve visual row `vr` into its painted slice — one shape for both the
    /// unwrapped (one row per line) and wrapped (many rows per line) paths.
    fn vis_row(&self, buffer: &Buffer, vr: usize, cols: usize) -> VisRow {
        if self.wrap {
            let line = self
                .wrap_map
                .line_at(vr)
                .min(buffer.len_lines().saturating_sub(1));
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

    /// The `(visual row, column-within-row)` for a caret at char index `cursor`.
    fn caret_cell_at(&self, buffer: &Buffer, cursor: usize, cols: usize) -> (usize, usize) {
        let cursor = cursor.min(buffer.len_chars());
        let line = buffer.char_to_line(cursor);
        let col = cursor - buffer.line_to_char(line);
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

    /// The primary caret's `(visual row, column-within-row)` — the reveal target.
    fn caret_cell(&self, buffer: &Buffer, cols: usize) -> (usize, usize) {
        self.caret_cell_at(buffer, self.primary_caret().cursor, cols)
    }
}

/// Render + edit an open [`Buffer`](crate::buffer::Buffer) through its
/// [`EditorView`] — the one egui entry point for the code editor (EDITOR-3/4/5).
///
/// Fills the available space with a scroll area, paints only the visible rows
/// (viewport culling), maps the pointer to a rope char index for click/drag/
/// double-/triple-/Alt-click/Alt-drag selection, routes this frame's key events
/// into the view, and draws the gutter + text + selections + carets through
/// [`Style`] tokens (§4). `highlight` is the document's syntax highlighter
/// (EDITOR-5) or `None` for plain text: when present it is synced with this
/// frame's edits (incremental re-parse) and the visible rows paint span by span
/// in their code-token colors. Returns the content [`Response`] so the surface
/// can observe focus/hover.
pub fn editor_widget(
    ui: &mut Ui,
    view: &mut EditorView,
    buffer: &mut Buffer,
    highlight: Option<&mut Highlighter>,
) -> Response {
    view.clamp(buffer);

    let font = body_font();
    let (glyph_w, row_h) = ui.fonts(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
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
            editor_body(ui, view, buffer, highlight, metrics, wrap_cols, total_rows)
        })
        .inner
}

/// The scroll-area body: allocate the virtual content, handle input, paint the
/// visible rows. Split out of [`editor_widget`] so each half stays legible.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
fn editor_body(
    ui: &mut Ui,
    view: &mut EditorView,
    buffer: &mut Buffer,
    highlight: Option<&mut Highlighter>,
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
    let (rect, resp) = ui.allocate_exact_size(vec2(content_w, content_h), Sense::click_and_drag());
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
    view.max_line_chars = view
        .max_line_chars
        .max(line_len(buffer, view.cur_line(buffer)));

    // EDITOR-5 — bring the syntax tree up to date with this frame's edits
    // (incremental: the buffer's edit deltas splice the old tree, no full-file
    // reparse per keystroke), then resolve the highlight spans for just the
    // VISIBLE window once — the query cost scales with the viewport, matching
    // the paint culling. Plain-text documents pass `None` and skip all of it.
    let spans = highlight.map_or_else(Vec::new, |hl| {
        hl.sync(buffer);
        if first < last {
            let w_start = view.vis_row(buffer, first, wrap_cols).start;
            let w_end = view.vis_row(buffer, last - 1, wrap_cols).end;
            hl.spans_in(buffer.rope(), w_start..w_end)
        } else {
            Vec::new()
        }
    });

    paint(
        ui, view, buffer, m, origin, first, last, wrap_cols, &resp, &spans,
    );

    // Reveal the primary caret exactly once after a move/edit (don't fight scroll).
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
/// triple-click selection, plus the Alt-click / Alt-drag multi-cursor gestures.
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
    let (shift, alt) = ui.input(|i| (i.modifiers.shift, i.modifiers.alt));

    // Resolve the pointer while only borrowing the view/buffer immutably, then
    // mutate — a closure capturing them would clash with the `&mut` the gestures
    // need.
    let Some(pos) = resp.interact_pointer_pos() else {
        return;
    };
    let idx = hit_char(view, buffer, m, origin, total_rows, wrap_cols, pos);
    let cell = hit_cell(m, origin, total_rows, pos);

    if alt {
        // Alt gestures: box (column) drag + toggle-caret click.
        if resp.drag_started() {
            view.box_anchor = Some(cell);
        }
        if resp.dragged() {
            if let Some((r0, c0)) = view.box_anchor {
                view.column_select(buffer, r0, c0, cell.0, cell.1, wrap_cols);
            }
        } else if resp.clicked() {
            view.toggle_caret_at(buffer, idx);
        }
        return;
    }

    if resp.clicked() || resp.drag_started() {
        view.box_anchor = None;
    }
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
    let (vr, col) = hit_cell(m, origin, total_rows, pos);
    let row = view.vis_row(buffer, vr, wrap_cols);
    let span = row.end - row.start;
    row.start + col.min(span)
}

/// The `(visual row, column)` cell under a screen `pos`, rounded to the nearest
/// glyph boundary. The shared basis for [`hit_char`] and the Alt-drag box.
fn hit_cell(m: Metrics, origin: Pos2, total_rows: usize, pos: Pos2) -> (usize, usize) {
    let vr = (((pos.y - origin.y) / m.row_h).floor().max(0.0) as usize)
        .min(total_rows.saturating_sub(1));
    let col = (((pos.x - origin.x - m.gutter_w) / m.glyph_w) + 0.5)
        .floor()
        .max(0.0) as usize;
    (vr, col)
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
                let sels = view.selections();
                if !sels.is_empty() {
                    // Join every caret's selection with a newline (top-to-bottom).
                    let text = sels
                        .iter()
                        .map(|r| buffer.rope().slice(r.clone()).to_string())
                        .collect::<Vec<_>>()
                        .join("\n");
                    ui.ctx().copy_text(text);
                    if matches!(event, Event::Cut) {
                        view.delete_selections(buffer);
                    }
                }
            }
            _ => {
                view.apply_event(buffer, event, rows);
            }
        }
    }
}

/// Paint the visible rows: current-line highlight (per caret line), selection
/// bands (per caret), gutter numbers, text glyphs (span-sliced into their code-
/// token colors when `spans` is non-empty, EDITOR-5), and the blinking carets —
/// all through [`Style`]/[`mde_egui::code`] tokens (§4).
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
    spans: &[HighlightSpan],
) {
    let clip = ui.clip_rect();
    let painter = ui.painter_at(clip);
    // Text/caret clip: everything right of the pinned gutter, so scrolled text
    // slides under the gutter rather than over it.
    let text_clip = Rect::from_min_max(pos2(clip.left() + m.gutter_w, clip.top()), clip.max);
    let text_painter = painter.with_clip_rect(text_clip);
    let text_x0 = origin.x + m.gutter_w;
    let len = buffer.len_chars();
    let caret_lines: Vec<usize> = view
        .carets
        .iter()
        .map(|c| buffer.char_to_line(c.cursor.min(len)))
        .collect();
    let selections = view.selections();

    // Rows ascend through the document, so one monotonic index walks the sorted
    // span list across the whole paint (a span crossing a row break — a block
    // comment — is not passed until every row it covers has painted).
    let mut span_idx = 0usize;

    for vr in first..last {
        let row = view.vis_row(buffer, vr, wrap_cols);
        #[allow(clippy::cast_precision_loss)]
        let y = origin.y + vr as f32 * m.row_h;

        // Current-line highlight (subtle raised fill across the text area) for
        // every caret's line.
        if caret_lines.contains(&row.line) {
            text_painter.rect_filled(
                Rect::from_min_max(pos2(text_clip.left(), y), pos2(clip.right(), y + m.row_h)),
                0.0,
                Style::SURFACE,
            );
        }

        // Selection bands for this row (one per caret selection).
        for sel in &selections {
            paint_selection(&text_painter, &row, sel, text_x0, y, m);
        }

        // Row text, span-sliced into code-token colors (EDITOR-5; plain rows —
        // no highlighter or no captures — paint as one foreground run). Only
        // this row's slices materialize, never the whole document.
        while span_idx < spans.len() && spans[span_idx].range.end <= row.start {
            span_idx += 1;
        }
        paint_row_text(
            &text_painter,
            buffer,
            &row,
            &spans[span_idx..],
            text_x0,
            y,
            m,
        );
    }

    paint_gutter(
        &painter,
        view,
        buffer,
        m,
        origin,
        clip,
        first,
        last,
        wrap_cols,
        &caret_lines,
    );
    paint_carets(&text_painter, ui, view, buffer, m, origin, wrap_cols, resp);
}

/// Paint one visual row's glyphs, sliced by the highlight `spans` that overlap
/// it (EDITOR-5): gaps between spans draw as plain foreground text, each span
/// draws in its [`CodeToken`](mde_egui::code::CodeToken) color at its monospace
/// column offset. `spans` starts at the first span that may still overlap this
/// row (the caller's monotonic walk); iteration stops at the first span past
/// the row's end, so per-row cost tracks the row's own span count.
fn paint_row_text(
    painter: &egui::Painter,
    buffer: &Buffer,
    row: &VisRow,
    spans: &[HighlightSpan],
    text_x0: f32,
    y: f32,
    m: Metrics,
) {
    if row.end <= row.start {
        return;
    }
    let mut cursor = row.start;
    for span in spans {
        if span.range.start >= row.end {
            break;
        }
        let start = span.range.start.max(cursor).min(row.end);
        let end = span.range.end.min(row.end);
        if end <= cursor {
            continue;
        }
        if start > cursor {
            draw_slice(
                painter,
                buffer,
                row,
                cursor..start,
                text_x0,
                y,
                m,
                Style::TEXT,
            );
        }
        draw_slice(
            painter,
            buffer,
            row,
            start..end,
            text_x0,
            y,
            m,
            span.token.color(),
        );
        cursor = end;
    }
    if cursor < row.end {
        draw_slice(
            painter,
            buffer,
            row,
            cursor..row.end,
            text_x0,
            y,
            m,
            Style::TEXT,
        );
    }
}

/// Paint one contiguous char slice of a visual row at its monospace column
/// offset in `color` — the shared draw for plain gaps and highlight spans.
#[allow(clippy::too_many_arguments)]
fn draw_slice(
    painter: &egui::Painter,
    buffer: &Buffer,
    row: &VisRow,
    chars: Range<usize>,
    text_x0: f32,
    y: f32,
    m: Metrics,
    color: Color32,
) {
    if chars.start >= chars.end {
        return;
    }
    let text = buffer.rope().slice(chars.clone()).to_string();
    #[allow(clippy::cast_precision_loss)]
    let x = text_x0 + (chars.start - row.start) as f32 * m.glyph_w;
    painter.text(pos2(x, y), Align2::LEFT_TOP, text, body_font(), color);
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
    caret_lines: &[usize],
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
        let color = if caret_lines.contains(&row.line) {
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

/// Paint every caret: a solid accent beam while focused (blinking on the frame
/// clock), a hollow beam when unfocused.
#[allow(clippy::too_many_arguments)]
fn paint_carets(
    painter: &egui::Painter,
    ui: &Ui,
    view: &EditorView,
    buffer: &Buffer,
    m: Metrics,
    origin: Pos2,
    wrap_cols: usize,
    resp: &Response,
) {
    let focused = resp.has_focus();
    let on = focused && blink_on(ui.input(|i| i.time));
    for c in &view.carets {
        let (vr, xcol) = view.caret_cell_at(buffer, c.cursor, wrap_cols);
        #[allow(clippy::cast_precision_loss)]
        let x = origin.x + m.gutter_w + xcol as f32 * m.glyph_w;
        #[allow(clippy::cast_precision_loss)]
        let y = origin.y + vr as f32 * m.row_h;
        let caret = Rect::from_min_size(pos2(x, y), vec2(CARET_W, m.row_h));
        if focused {
            if on {
                painter.rect_filled(caret, 0.0, Style::ACCENT);
            }
        } else {
            painter.rect_stroke(
                caret,
                0.0,
                Stroke::new(1.0, Style::TEXT_DIM),
                egui::StrokeKind::Middle,
            );
        }
    }
    if focused {
        // Keep frames coming so the caret actually blinks while idle.
        ui.ctx().request_repaint_after(Duration::from_secs_f64(0.5));
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
    use super::{editor_widget, find_next, line_len, line_span, word_span, Caret, EditorView};
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

    /// `Ctrl`/`Cmd` + `Alt` — the add-cursor-above/below chord.
    fn cmd_alt() -> Modifiers {
        Modifiers {
            alt: true,
            ..Modifiers::COMMAND
        }
    }

    // ── cursor movement (EDITOR-3, must still pass) ──────────────────────────

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
        assert_eq!(
            view.line_col(&buf),
            (3, 6),
            "goal column restored on line 2"
        );
    }

    // ── selection (EDITOR-3) ─────────────────────────────────────────────────

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

    // ── edit through the view (EDITOR-3) ─────────────────────────────────────

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

    // ── undo / redo via the view (EDITOR-3) ──────────────────────────────────

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
        assert_eq!(view.cursor(), 0, "undo restored the pre-run caret");
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

    // ── pure helpers (EDITOR-3) ──────────────────────────────────────────────

    #[test]
    fn line_len_excludes_the_trailing_newline() {
        let buf = Buffer::from_text("abc\n\nxy\n");
        assert_eq!(line_len(&buf, 0), 3);
        assert_eq!(line_len(&buf, 1), 0, "a blank line has zero length");
        assert_eq!(line_len(&buf, 2), 2);
        assert_eq!(line_len(&buf, 3), 0, "the final empty line");
    }

    // ── multi-cursor (EDITOR-4) ──────────────────────────────────────────────

    #[test]
    fn add_cursor_below_and_above_stack_carets() {
        let mut buf = Buffer::from_text("aaa\nbbb\nccc");
        let mut view = EditorView::new();
        view.place_cursor(&buf, 1); // line 0, col 1
                                    // Ctrl+Alt+Down adds a caret on line 1 at the same column, made primary.
        view.apply_event(&mut buf, &key(Key::ArrowDown, cmd_alt()), 10);
        assert_eq!(view.carets.len(), 2);
        assert_eq!(view.cursor(), 5, "line 1 col 1");
        // Again → line 2.
        view.apply_event(&mut buf, &key(Key::ArrowDown, cmd_alt()), 10);
        assert_eq!(view.carets.len(), 3);
        assert_eq!(view.cursor(), 9, "line 2 col 1");
        let mut cs: Vec<usize> = view.carets.iter().map(|c| c.cursor).collect();
        cs.sort_unstable();
        assert_eq!(cs, vec![1, 5, 9]);
        // Add-above from the bottom caret re-lands on line 1 → merges with the
        // existing caret there, so the count does not grow.
        view.apply_event(&mut buf, &key(Key::ArrowUp, cmd_alt()), 10);
        assert_eq!(
            view.carets.len(),
            3,
            "add-above onto an existing caret merges"
        );
    }

    #[test]
    fn add_cursor_at_next_match_selects_then_adds() {
        let mut buf = Buffer::from_text("foo bar foo baz foo");
        let mut view = EditorView::new();
        view.place_cursor(&buf, 1); // inside the first "foo"
                                    // First Ctrl+D selects the word.
        view.apply_event(&mut buf, &key(Key::D, cmd()), 10);
        assert_eq!(view.carets.len(), 1);
        assert_eq!(
            view.selection(),
            Some(0..3),
            "first Ctrl+D selects the word"
        );
        // Second Ctrl+D adds a caret at the next "foo" (chars 8..11), now primary.
        view.apply_event(&mut buf, &key(Key::D, cmd()), 10);
        assert_eq!(view.carets.len(), 2);
        assert_eq!(view.selection(), Some(8..11));
        // Third adds the last "foo" (16..19).
        view.apply_event(&mut buf, &key(Key::D, cmd()), 10);
        assert_eq!(view.carets.len(), 3);
        assert_eq!(view.selection(), Some(16..19));
        // find_next wraps back to the top match after the last one.
        assert_eq!(find_next(&buf, &['f', 'o', 'o'], 19), Some(0));
    }

    #[test]
    fn column_select_stacks_a_caret_per_row() {
        let buf = Buffer::from_text("abcd\nefgh\nijkl");
        let mut view = EditorView::new();
        // Box from row 0 col 1 to row 2 col 3 → one caret per row, each selecting
        // the col 1..3 band of its row (line starts 0, 5, 10).
        view.column_select(&buf, 0, 1, 2, 3, 80);
        assert_eq!(view.carets.len(), 3);
        assert_eq!(view.selections(), vec![1..3, 6..8, 11..13]);
    }

    #[test]
    fn a_fan_out_insert_edits_every_caret_and_undoes_as_one_step() {
        let mut buf = Buffer::from_text("a\nb\nc");
        let mut view = EditorView::new();
        // A caret at the start of each of the three lines.
        view.place_cursor(&buf, 0);
        view.apply_event(&mut buf, &key(Key::ArrowDown, cmd_alt()), 10);
        view.apply_event(&mut buf, &key(Key::ArrowDown, cmd_alt()), 10);
        assert_eq!(view.carets.len(), 3);
        // Typing fans out to every caret (proves it edits the real rope N times).
        view.apply_event(&mut buf, &Event::Text("X".to_owned()), 10);
        assert_eq!(buf.rope().to_string(), "Xa\nXb\nXc", "every caret inserted");
        // One undo reverts the whole fan-out and restores every caret.
        view.apply_event(&mut buf, &key(Key::Z, cmd()), 10);
        assert_eq!(
            buf.rope().to_string(),
            "a\nb\nc",
            "one undo reverts the fan-out"
        );
        assert_eq!(view.carets.len(), 3, "undo restored every caret");
        // Redo puts the text + carets back.
        view.apply_event(&mut buf, &key(Key::Y, cmd()), 10);
        assert_eq!(buf.rope().to_string(), "Xa\nXb\nXc");
        assert_eq!(view.carets.len(), 3);
    }

    #[test]
    fn fan_out_backspace_deletes_at_every_caret() {
        let mut buf = Buffer::from_text("Xa\nXb\nXc");
        let mut view = EditorView::new();
        // Carets just after each leading 'X' (cols 1 of each line): 1, 4, 7.
        view.place_cursor(&buf, 1);
        view.apply_event(&mut buf, &key(Key::ArrowDown, cmd_alt()), 10);
        view.apply_event(&mut buf, &key(Key::ArrowDown, cmd_alt()), 10);
        assert_eq!(view.carets.len(), 3);
        view.apply_event(&mut buf, &key(Key::Backspace, Modifiers::NONE), 10);
        assert_eq!(buf.rope().to_string(), "a\nb\nc", "backspace fanned out");
    }

    #[test]
    fn overlapping_carets_merge_on_normalize() {
        let mut buf = Buffer::from_text("abcdef");
        let mut view = EditorView::new();
        // Two selections that overlap (0..3 and 2..5).
        view.carets = vec![
            Caret {
                cursor: 3,
                anchor: Some(0),
                goal_col: None,
            },
            Caret {
                cursor: 5,
                anchor: Some(2),
                goal_col: None,
            },
        ];
        view.primary = 1;
        // A no-op motion (Right without shift would move; use normalize directly).
        view.normalize();
        assert_eq!(
            view.carets.len(),
            1,
            "overlapping selections merged into one"
        );
        assert_eq!(view.selection(), Some(0..5));
        let _ = &mut buf;
    }

    #[test]
    fn escape_collapses_to_a_single_primary_caret() {
        let mut buf = Buffer::from_text("aaa\nbbb\nccc");
        let mut view = EditorView::new();
        view.place_cursor(&buf, 1);
        view.apply_event(&mut buf, &key(Key::ArrowDown, cmd_alt()), 10);
        view.apply_event(&mut buf, &key(Key::ArrowDown, cmd_alt()), 10);
        assert_eq!(view.carets.len(), 3);
        let primary_cursor = view.cursor();
        let changed = view.apply_event(&mut buf, &key(Key::Escape, Modifiers::NONE), 10);
        assert!(changed, "Esc collapsed the multi-cursor");
        assert_eq!(view.carets.len(), 1, "Esc collapsed to one caret");
        assert_eq!(view.cursor(), primary_cursor, "the primary caret survives");
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
                editor_widget(ui, &mut view, &mut buf, None);
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the editor widget produced no primitives"
        );
    }

    #[test]
    fn multi_caret_widget_paints_every_caret() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut buf = Buffer::from_text("one\ntwo\nthree");
        let mut view = EditorView::new();
        // Stack three carets so the paint pass loops all of them.
        view.place_cursor(&buf, 0);
        view.apply_event(&mut buf, &key(Key::ArrowDown, cmd_alt()), 10);
        view.apply_event(&mut buf, &key(Key::ArrowDown, cmd_alt()), 10);
        assert_eq!(view.carets.len(), 3);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                editor_widget(ui, &mut view, &mut buf, None);
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the multi-caret editor produced no primitives"
        );
    }

    #[test]
    fn highlighted_widget_syncs_the_tree_and_paints() {
        // EDITOR-5 end-to-end through the real widget frame: a rust buffer with a
        // live highlighter paints, and the frame itself ran the highlighter's
        // sync (the parse happened inside `editor_widget`, not in test setup).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut buf = Buffer::from_text("fn main() {\n    let s = \"hi\"; // c\n}\n");
        let mut view = EditorView::new();
        let mut hl = crate::highlight::Highlighter::new(crate::highlight::Language::Rust)
            .expect("rust grammar loads");
        assert_eq!(hl.full_parses(), 0, "no parse before the frame");
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                editor_widget(ui, &mut view, &mut buf, Some(&mut hl));
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the highlighted editor produced no primitives"
        );
        assert_eq!(
            hl.full_parses(),
            1,
            "the widget frame ran the highlighter's initial sync"
        );
        // And the synced tree yields real spans over the visible text.
        assert!(
            !hl.spans_in(buf.rope(), 0..buf.len_chars()).is_empty(),
            "the frame-synced highlighter captures the rust snippet"
        );
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
                editor_widget(ui, &mut view, &mut buf, None);
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the wrapped editor produced no primitives"
        );
        // The wrap map saw more visual rows than the single logical line.
        assert!(
            view.wrap_map.total() > 1,
            "a long line wrapped into multiple visual rows"
        );
    }
}
