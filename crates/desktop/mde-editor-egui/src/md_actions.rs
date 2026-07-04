//! `md_actions` — the **pure markdown edit engine** behind the EDTB Formatting
//! strip (EDTB-2; design: `docs/design/editor-toolbar.md` lock #1).
//!
//! Formatting controls make real, visible markup edits — Bold wraps the
//! selection in `**`, the Style dropdown rewrites leading `#`s, the list
//! buttons toggle line prefixes — the buffer stays plain text and tree-sitter's
//! markdown grammar highlights the result. Every fn here is a pure operation
//! over the rope [`Buffer`] plus the caller's caret/selection spans (plain char
//! ranges — the widget's multi-caret model, passed as parameters so the engine
//! imports zero widget internals) and is unit-testable without a live egui
//! frame (§7).
//!
//! **Multi-caret contract.** Edits apply *highest-position-first* — the inverse
//! walk of the widget's ascending fan-out, same arithmetic: an edit never moves
//! text below itself, so every span stays valid in original coordinates while
//! the op runs, and a single ascending fix-up pass shifts the returned spans.
//! [`MdOutcome::selections`] carries the post-edit spans (ascending) so the
//! widget can restore every caret.
//!
//! **Undo contract.** Each op opens on a [`Buffer::commit_group`] boundary (so
//! it can never coalesce into the operator's in-flight typing group), commits
//! after every mutation, and reports the exact number of buffer undo groups it
//! created in [`MdOutcome::groups`]. That is the same compound-edit idiom the
//! widget's own fan-out edits use: its undo log replays `groups` buffer undos
//! as ONE operator-facing step, so every op here undoes in one step.

// `missing_const_for_fn` (nursery) is over-eager for small helpers whose
// const-ness we don't want to pin into the contract (the same call `buffer.rs`
// makes). The cast lints are allowed module-wide for the same reason as
// `widget.rs`: the fan-out fix-up converts between char indices (`usize`) and
// the signed shift accumulator (`isize`); every conversion is bounded by the
// document size.
#![allow(
    clippy::missing_const_for_fn,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use std::ops::Range;

use crate::buffer::Buffer;

/// The result of one formatting op.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MdOutcome {
    /// The post-edit caret/selection spans (char indices, ascending). Hand
    /// these back to the widget's carets; wrapping ops return the *inner* text
    /// span (markers outside), so toggling again with the returned span
    /// round-trips.
    pub selections: Vec<Range<usize>>,
    /// How many [`Buffer`] undo groups the op committed. The caller records
    /// this in its widget-level undo entry (exactly like the widget's own
    /// fan-out edits) so the whole op undoes/redoes as one step.
    pub groups: usize,
}

/// Which list prefix [`toggle_line_prefix`] toggles (design lock #8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListKind {
    /// `- ` bullets (`* ` / `+ ` are recognized as bullets when detecting).
    Bullet,
    /// `n. ` numbers, renumbered 1..n per contiguous run of selected lines.
    Numbered,
}

/// One op's transaction over the buffer: every mutation lands as its own undo
/// group (commit after each) and is counted, so the op can report the exact
/// group span its caller replays as one undo step.
struct Txn<'a> {
    buffer: &'a mut Buffer,
    groups: usize,
}

impl<'a> Txn<'a> {
    /// Open the op on a fresh group boundary so it never coalesces into the
    /// operator's in-flight typing group.
    fn new(buffer: &'a mut Buffer) -> Self {
        buffer.commit_group();
        Self { buffer, groups: 0 }
    }

    /// Insert + commit as one counted group (empty inserts are no-ops).
    fn ins(&mut self, at: usize, text: &str) {
        if text.is_empty() {
            return;
        }
        self.buffer.insert(at, text);
        self.buffer.commit_group();
        self.groups += 1;
    }

    /// Remove + commit as one counted group (empty ranges are no-ops).
    fn del(&mut self, range: Range<usize>) {
        if range.start >= range.end {
            return;
        }
        self.buffer.remove(range);
        self.buffer.commit_group();
        self.groups += 1;
    }
}

/// `sel` normalized: endpoints ordered and clamped to the buffer length.
fn ordered(sel: &Range<usize>, len: usize) -> Range<usize> {
    let a = sel.start.min(len);
    let b = sel.end.min(len);
    a.min(b)..a.max(b)
}

/// Sort spans ascending and merge overlapping/duplicate ones — the engine's
/// defensive normalization of the widget's caret set.
fn merged(mut spans: Vec<Range<usize>>) -> Vec<Range<usize>> {
    spans.sort_by_key(|r| (r.start, r.end));
    let mut out: Vec<Range<usize>> = Vec::with_capacity(spans.len());
    for s in spans {
        if let Some(prev) = out.last_mut() {
            // Merge strict overlaps and exact duplicates (coincident carets).
            let duplicate = s == *prev;
            if duplicate || s.start < prev.end {
                prev.end = prev.end.max(s.end);
                continue;
            }
        }
        out.push(s);
    }
    out
}

/// The union of 0-based line indices the selections touch, sorted + deduped. A
/// non-empty selection ending exactly at a line start does NOT include that
/// line (the standard editor convention).
fn selected_lines(buffer: &Buffer, selections: &[Range<usize>]) -> Vec<usize> {
    let len = buffer.len_chars();
    let mut lines = Vec::new();
    for sel in selections {
        let sel = ordered(sel, len);
        let first = buffer.char_to_line(sel.start);
        let mut last = buffer.char_to_line(sel.end);
        if sel.end > sel.start && last > first && buffer.line_to_char(last) == sel.end {
            last -= 1;
        }
        lines.extend(first..=last);
    }
    lines.sort_unstable();
    lines.dedup();
    lines
}

/// The text of line `l` without its trailing newline.
fn line_text(buffer: &Buffer, l: usize) -> String {
    let mut s = buffer.line(l);
    if s.ends_with('\n') {
        s.pop();
    }
    if s.ends_with('\r') {
        s.pop();
    }
    s
}

/// One line-anchored splice in ORIGINAL document coordinates (valid because
/// ops apply highest-position-first: an edit never moves text below itself).
#[derive(Clone, Copy)]
struct LineEdit {
    /// Char index the splice starts at.
    at: usize,
    /// Chars removed at `at` (the stripped prefix).
    removed: usize,
    /// Chars inserted at `at` (the new prefix).
    inserted: usize,
}

/// Map an original char position through `edits` (ascending by `at`). A
/// position inside a replaced prefix lands just after its replacement.
fn map_pos(p: usize, edits: &[LineEdit]) -> usize {
    let mut shift: isize = 0;
    for e in edits {
        if p < e.at {
            break;
        }
        if p < e.at + e.removed {
            return ((e.at + e.inserted) as isize + shift) as usize;
        }
        shift += e.inserted as isize - e.removed as isize;
    }
    (p as isize + shift) as usize
}

/// The input selections mapped through the op's line edits, input order
/// preserved (`len` = the pre-op buffer length the inputs are clamped to).
fn remap(selections: &[Range<usize>], len: usize, edits: &[LineEdit]) -> Vec<Range<usize>> {
    selections
        .iter()
        .map(|s| {
            let s = ordered(s, len);
            map_pos(s.start, edits)..map_pos(s.end, edits)
        })
        .collect()
}

// ─── toggle_wrap ─────────────────────────────────────────────────────────────

/// The closing counterpart of `open`: `<u>` → `</u>` (markdown has no
/// underline — honest HTML-in-md, lock #1); every symmetric markdown marker
/// (`**`, `*`, `~~`, `` ` ``) closes with itself.
fn close_marker(open: &str) -> String {
    if open.len() > 2 && open.starts_with('<') && open.ends_with('>') && !open.starts_with("</") {
        format!("</{}", &open[1..])
    } else {
        open.to_owned()
    }
}

/// Expand a bare caret to the word under it (alphanumeric + `_` run), or an
/// empty span at the caret when it doesn't touch a word.
fn word_at(buffer: &Buffer, caret: usize) -> Range<usize> {
    let rope = buffer.rope();
    let caret = caret.min(rope.len_chars());
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut s = caret;
    while s > 0 && is_word(rope.char(s - 1)) {
        s -= 1;
    }
    let mut e = caret;
    while e < rope.len_chars() && is_word(rope.char(e)) {
        e += 1;
    }
    s..e
}

/// Whether the rope's chars in `range` spell exactly `text`.
fn slice_is(buffer: &Buffer, range: Range<usize>, text: &str) -> bool {
    range.end <= buffer.len_chars() && buffer.rope().slice(range) == text
}

/// Length of the run of `c` at the start of `text`, in chars.
fn lead_run(text: &str, c: char) -> usize {
    text.chars().take_while(|&x| x == c).count()
}

/// Length of the run of `c` at the end of `text`, in chars.
fn trail_run(text: &str, c: char) -> usize {
    text.chars().rev().take_while(|&x| x == c).count()
}

/// `Some(c)` when `marker` is a run of one repeated char (`*`, `**`, `~~`,
/// `` ` `` …) — the markers whose toggle detection counts adjacent runs.
fn run_char(marker: &str) -> Option<char> {
    let mut chars = marker.chars();
    let c = chars.next()?;
    chars.all(|x| x == c).then_some(c)
}

/// Whether a run of `run` adjacent `c`s carries this marker. A `*` run holds
/// italic exactly when it is odd (`**` is bold-only, `***` bold + italic —
/// real markdown emphasis arithmetic, so toggling Italic inside `**bold**`
/// wraps instead of eating a star); any other marker when the run reaches the
/// marker's length.
fn run_carries(run: usize, c: char, k: usize) -> bool {
    if c == '*' && k == 1 {
        run % 2 == 1
    } else {
        run >= k
    }
}

/// Whether the selection text itself carries the markers (`**bold**` selected
/// with the stars included).
fn wrapped_inside(buffer: &Buffer, span: Range<usize>, open: &str, close: &str) -> bool {
    let (k_open, k_close) = (open.chars().count(), close.chars().count());
    if span.end - span.start < k_open + k_close {
        return false;
    }
    let text = buffer.rope().slice(span).to_string();
    match run_char(open) {
        Some(c) if open == close => {
            run_carries(lead_run(&text, c).min(trail_run(&text, c)), c, k_open)
        }
        _ => text.starts_with(open) && text.ends_with(close),
    }
}

/// Whether the markers sit immediately OUTSIDE the selection (`bold` selected
/// inside `**bold**` — the exact span [`toggle_wrap`] returns after wrapping,
/// so toggling again with the returned span round-trips).
fn wrapped_outside(buffer: &Buffer, span: Range<usize>, open: &str, close: &str) -> bool {
    let (k_open, k_close) = (open.chars().count(), close.chars().count());
    if span.start < k_open
        || span.end + k_close > buffer.len_chars()
        || !slice_is(buffer, span.start - k_open..span.start, open)
        || !slice_is(buffer, span.end..span.end + k_close, close)
    {
        return false;
    }
    match run_char(open) {
        Some(c) if open == close => {
            let rope = buffer.rope();
            let mut left = 0;
            while left < span.start && rope.char(span.start - 1 - left) == c {
                left += 1;
            }
            let mut right = 0;
            while span.end + right < rope.len_chars() && rope.char(span.end + right) == c {
                right += 1;
            }
            run_carries(left.min(right), c, k_open)
        }
        _ => true,
    }
}

/// Toggle a wrap marker around every selection: `**` Bold, `*` Italic, `~~`
/// strikethrough, `` ` `` code, `<u>` underline (closes as `</u>`).
///
/// Per span: an already-wrapped span unwraps (the markers may sit inside the
/// selection or immediately around it — both are detected, with real markdown
/// star-run arithmetic disambiguating `*` inside `**`); anything else wraps. A
/// bare caret expands to the word under it; a caret touching no word drops an
/// empty marker pair and parks the caret between the markers (toggling again
/// removes the pair). Returned spans cover the inner text, markers outside.
pub fn toggle_wrap(buffer: &mut Buffer, selections: &[Range<usize>], marker: &str) -> MdOutcome {
    let close = close_marker(marker);
    let (k_open, k_close) = (marker.chars().count(), close.chars().count());
    let len = buffer.len_chars();
    if k_open == 0 {
        return MdOutcome {
            selections: remap(selections, len, &[]),
            groups: 0,
        };
    }
    // Expand bare carets to their words on the pristine buffer, then merge —
    // two carets in one word wrap it once.
    let expanded: Vec<Range<usize>> = selections
        .iter()
        .map(|sel| {
            let sel = ordered(sel, len);
            if sel.is_empty() {
                word_at(buffer, sel.start)
            } else {
                sel
            }
        })
        .collect();
    let sels = merged(expanded);

    let mut txn = Txn::new(buffer);
    // (local result span in original coords, signed length delta) per span,
    // collected highest-first.
    let mut spans: Vec<(Range<usize>, isize)> = Vec::with_capacity(sels.len());
    for sel in sels.iter().rev() {
        let (s, e) = (sel.start, sel.end);
        let grew = (k_open + k_close) as isize;
        let entry = if s == e {
            if wrapped_outside(txn.buffer, s..e, marker, &close) {
                txn.del(e..e + k_close);
                txn.del(s - k_open..s);
                (s - k_open..s - k_open, -grew)
            } else {
                txn.ins(s, &format!("{marker}{close}"));
                (s + k_open..s + k_open, grew)
            }
        } else if wrapped_inside(txn.buffer, s..e, marker, &close) {
            txn.del(e - k_close..e);
            txn.del(s..s + k_open);
            (s..e - k_open - k_close, -grew)
        } else if wrapped_outside(txn.buffer, s..e, marker, &close) {
            txn.del(e..e + k_close);
            txn.del(s - k_open..s);
            (s - k_open..e - k_open, -grew)
        } else {
            txn.ins(e, &close);
            txn.ins(s, marker);
            (s + k_open..e + k_open, grew)
        };
        spans.push(entry);
    }
    let groups = txn.groups;
    // Ascending fix-up: each span shifts by the net delta of the edits below.
    let mut shift: isize = 0;
    let mut out = Vec::with_capacity(spans.len());
    for (span, delta) in spans.into_iter().rev() {
        out.push((span.start as isize + shift) as usize..(span.end as isize + shift) as usize);
        shift += delta;
    }
    MdOutcome {
        selections: out,
        groups,
    }
}

// ─── set_heading ─────────────────────────────────────────────────────────────

/// Normalize the selected lines' leading `#`s to heading `level` (0 strips to
/// body text; levels past 6 clamp to 6 — markdown's deepest heading).
///
/// Any existing `#`-run (plus its following spaces) is replaced by exactly
/// `level` hashes and one space, so mixed lines come out uniform; a line
/// already at the level is left untouched (idempotent, no undo noise).
pub fn set_heading(buffer: &mut Buffer, selections: &[Range<usize>], level: u8) -> MdOutcome {
    let level = usize::from(level.min(6));
    let len = buffer.len_chars();
    let lines = selected_lines(buffer, selections);
    let prefix = if level == 0 {
        String::new()
    } else {
        format!("{} ", "#".repeat(level))
    };
    let mut txn = Txn::new(buffer);
    let mut edits: Vec<LineEdit> = Vec::new();
    for &l in lines.iter().rev() {
        let ls = txn.buffer.line_to_char(l);
        let text = line_text(txn.buffer, l);
        let hashes = lead_run(&text, '#');
        let strip = if hashes == 0 {
            0
        } else {
            hashes + text.chars().skip(hashes).take_while(|&c| c == ' ').count()
        };
        let current: String = text.chars().take(strip).collect();
        if current == prefix {
            continue;
        }
        txn.del(ls..ls + strip);
        txn.ins(ls, &prefix);
        edits.push(LineEdit {
            at: ls,
            removed: strip,
            inserted: prefix.chars().count(),
        });
    }
    edits.reverse();
    let groups = txn.groups;
    MdOutcome {
        selections: remap(selections, len, &edits),
        groups,
    }
}

// ─── toggle_line_prefix ──────────────────────────────────────────────────────

/// One non-blank selected line's parse for the list toggles.
struct ListLine {
    /// 0-based line index.
    line: usize,
    /// Leading whitespace, in chars (preserved by every toggle).
    indent: usize,
    /// The existing list marker after the indent: `(char len, is_numbered)`.
    marker: Option<(usize, bool)>,
}

/// The list marker at the start of `rest` (`- ` / `* ` / `+ ` or `N. `), as
/// `(chars, is_numbered)` — `None` when the line carries neither.
fn list_marker(rest: &str) -> Option<(usize, bool)> {
    if rest.starts_with("- ") || rest.starts_with("* ") || rest.starts_with("+ ") {
        return Some((2, false));
    }
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let mut tail = rest.chars().skip(digits);
        if tail.next() == Some('.') && tail.next() == Some(' ') {
            return Some((digits + 2, true));
        }
    }
    None
}

/// Toggle a list prefix on the selected lines (design lock #8).
///
/// When EVERY non-blank selected line already carries `kind`'s marker the
/// toggle is OFF: markers are removed (indent kept). Otherwise it normalizes
/// ON: every non-blank line gets the marker, replacing any other list marker
/// (bullets convert to numbers and vice versa — the Word behavior), and
/// numbered runs are renumbered `1..n` (a mixed or mis-numbered run comes out
/// sequential). Blank lines are skipped entirely; they neither consume a
/// number nor split a run (a loose markdown list). Disjoint selections
/// restart numbering per contiguous run of selected lines.
pub fn toggle_line_prefix(
    buffer: &mut Buffer,
    selections: &[Range<usize>],
    kind: ListKind,
) -> MdOutcome {
    let len = buffer.len_chars();
    let lines = selected_lines(buffer, selections);
    let mut parsed: Vec<ListLine> = Vec::new();
    for &l in &lines {
        let text = line_text(buffer, l);
        let indent = text.chars().take_while(|c| c.is_whitespace()).count();
        if indent == text.chars().count() {
            continue; // blank: never prefixed
        }
        let rest: String = text.chars().skip(indent).collect();
        parsed.push(ListLine {
            line: l,
            indent,
            marker: list_marker(&rest),
        });
    }
    if parsed.is_empty() {
        return MdOutcome {
            selections: remap(selections, len, &[]),
            groups: 0,
        };
    }
    let wants_numbered = kind == ListKind::Numbered;
    let all_on = parsed
        .iter()
        .all(|p| p.marker.is_some_and(|(_, num)| num == wants_numbered));

    // Numbers for normalize-ON: 1..n per contiguous run of SELECTED lines
    // (blanks inside a run keep continuity without consuming a number).
    let mut numbers: Vec<usize> = Vec::with_capacity(parsed.len());
    if wants_numbered && !all_on {
        let mut counter = 0usize;
        let mut prev: Option<usize> = None;
        let mut items = parsed.iter().peekable();
        for &l in &lines {
            if prev.is_none_or(|p| l != p + 1) {
                counter = 0;
            }
            prev = Some(l);
            if items.peek().is_some_and(|p| p.line == l) {
                items.next();
                counter += 1;
                numbers.push(counter);
            }
        }
    } else {
        numbers = vec![0; parsed.len()];
    }

    let mut txn = Txn::new(buffer);
    let mut edits: Vec<LineEdit> = Vec::new();
    for (p, num) in parsed.iter().zip(numbers.iter()).rev() {
        let at = txn.buffer.line_to_char(p.line) + p.indent;
        if all_on {
            if let Some((mlen, _)) = p.marker {
                txn.del(at..at + mlen);
                edits.push(LineEdit {
                    at,
                    removed: mlen,
                    inserted: 0,
                });
            }
        } else {
            let new_marker = if wants_numbered {
                format!("{num}. ")
            } else {
                "- ".to_owned()
            };
            let mlen = p.marker.map_or(0, |(m, _)| m);
            if slice_is(txn.buffer, at..at + mlen, &new_marker) {
                continue; // already exactly this marker: no undo noise
            }
            txn.del(at..at + mlen);
            txn.ins(at, &new_marker);
            edits.push(LineEdit {
                at,
                removed: mlen,
                inserted: new_marker.chars().count(),
            });
        }
    }
    edits.reverse();
    let groups = txn.groups;
    MdOutcome {
        selections: remap(selections, len, &edits),
        groups,
    }
}

// ─── shift_indent ────────────────────────────────────────────────────────────

/// Shift the selected lines' list nesting by `delta` levels of two spaces
/// (design lock #8): positive indents, negative outdents.
///
/// Outdenting removes at most the spaces a line actually has — the indent
/// floors at column 0, never negative. Blank (whitespace-only) lines are
/// left alone.
pub fn shift_indent(buffer: &mut Buffer, selections: &[Range<usize>], delta: isize) -> MdOutcome {
    let len = buffer.len_chars();
    let lines = selected_lines(buffer, selections);
    let mut txn = Txn::new(buffer);
    let mut edits: Vec<LineEdit> = Vec::new();
    let width = 2 * delta.unsigned_abs();
    if delta != 0 {
        let pad = " ".repeat(width);
        for &l in lines.iter().rev() {
            let ls = txn.buffer.line_to_char(l);
            let text = line_text(txn.buffer, l);
            if text.trim().is_empty() {
                continue;
            }
            if delta > 0 {
                txn.ins(ls, &pad);
                edits.push(LineEdit {
                    at: ls,
                    removed: 0,
                    inserted: width,
                });
            } else {
                let take = lead_run(&text, ' ').min(width);
                if take > 0 {
                    txn.del(ls..ls + take);
                    edits.push(LineEdit {
                        at: ls,
                        removed: take,
                        inserted: 0,
                    });
                }
            }
        }
    }
    edits.reverse();
    let groups = txn.groups;
    MdOutcome {
        selections: remap(selections, len, &edits),
        groups,
    }
}

// ─── insert_table ────────────────────────────────────────────────────────────

/// Insert a markdown table skeleton at the caret's line start (design lock
/// #8 — the grid picker's landing seam).
///
/// The skeleton is one `Col 1 … Col n` header row, the `---` separator row,
/// and `rows` empty body rows, every cell padded to one uniform width so the
/// source reads as a grid; `rows`/`cols` clamp to at least 1. The whole
/// skeleton is ONE buffer insert — a single undo group — and the returned
/// selection covers the first header label, ready to overtype.
pub fn insert_table(buffer: &mut Buffer, caret: usize, rows: usize, cols: usize) -> MdOutcome {
    let (rows, cols) = (rows.max(1), cols.max(1));
    let caret = caret.min(buffer.len_chars());
    let ls = buffer.line_to_char(buffer.char_to_line(caret));
    let labels: Vec<String> = (1..=cols).map(|i| format!("Col {i}")).collect();
    // Labels are ASCII, so byte length == char width.
    let width = labels.iter().map(String::len).max().unwrap_or(0);
    let row_of = |cells: &[String]| format!("| {} |\n", cells.join(" | "));
    let header: Vec<String> = labels.iter().map(|l| format!("{l:width$}")).collect();
    let mut table = row_of(&header);
    table.push_str(&row_of(&vec!["-".repeat(width); cols]));
    let body = row_of(&vec![" ".repeat(width); cols]);
    for _ in 0..rows {
        table.push_str(&body);
    }
    let mut txn = Txn::new(buffer);
    txn.ins(ls, &table);
    let first_label = labels.first().map_or(0, String::len);
    MdOutcome {
        selections: std::iter::once(ls + 2..ls + 2 + first_label).collect(),
        groups: txn.groups,
    }
}

#[cfg(test)]
// `single_range_in_vec_init`: the engine API takes `&[Range]` caret sets, so a
// one-caret call site is a deliberate one-element array of one span — never a
// botched `vec![start; end]`.
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::{
        insert_table, set_heading, shift_indent, toggle_line_prefix, toggle_wrap, ListKind,
        MdOutcome,
    };
    use crate::buffer::Buffer;

    /// Undo one engine op: replay its `groups` buffer undo groups — exactly
    /// what the widget's undo-log entry does, i.e. ONE operator-facing step.
    fn undo_op(buf: &mut Buffer, out: &MdOutcome) {
        for _ in 0..out.groups {
            assert!(buf.undo().is_some(), "every counted group must undo");
        }
    }

    /// Redo one engine op (the counterpart of [`undo_op`]).
    fn redo_op(buf: &mut Buffer, out: &MdOutcome) {
        for _ in 0..out.groups {
            assert!(buf.redo().is_some(), "every counted group must redo");
        }
    }

    // ── toggle_wrap ──────────────────────────────────────────────────────────

    #[test]
    fn bold_wraps_a_selection_and_returns_the_inner_span() {
        let mut buf = Buffer::from_text("make this bold");
        let out = toggle_wrap(&mut buf, &[5..9], "**");
        assert_eq!(buf.rope().to_string(), "make **this** bold");
        assert_eq!(out.selections, vec![7..11], "inner text, markers outside");
    }

    #[test]
    fn bold_toggle_round_trips_via_the_returned_selection() {
        let mut buf = Buffer::from_text("make this bold");
        let on = toggle_wrap(&mut buf, &[5..9], "**");
        let off = toggle_wrap(&mut buf, &on.selections, "**");
        assert_eq!(buf.rope().to_string(), "make this bold");
        assert_eq!(off.selections, vec![5..9], "the original span comes back");
    }

    #[test]
    fn unwrap_when_the_selection_includes_the_markers() {
        let mut buf = Buffer::from_text("a **b** c");
        let out = toggle_wrap(&mut buf, &[2..7], "**");
        assert_eq!(buf.rope().to_string(), "a b c");
        assert_eq!(out.selections, vec![2..3]);
    }

    #[test]
    fn italic_inside_bold_wraps_instead_of_eating_a_star() {
        // `**` is bold-only: toggling Italic must ADD a star pair, not strip.
        let mut buf = Buffer::from_text("**bold**");
        toggle_wrap(&mut buf, &[2..6], "*");
        assert_eq!(buf.rope().to_string(), "***bold***");

        // `***` carries italic: toggling Italic strips exactly one pair.
        let mut buf = Buffer::from_text("***x***");
        toggle_wrap(&mut buf, &[3..4], "*");
        assert_eq!(buf.rope().to_string(), "**x**");
    }

    #[test]
    fn underline_uses_honest_html_and_round_trips() {
        let mut buf = Buffer::from_text("plain word here");
        let on = toggle_wrap(&mut buf, &[6..10], "<u>");
        assert_eq!(buf.rope().to_string(), "plain <u>word</u> here");
        assert_eq!(on.selections, vec![9..13]);
        toggle_wrap(&mut buf, &on.selections, "<u>");
        assert_eq!(buf.rope().to_string(), "plain word here");
    }

    #[test]
    fn code_and_strike_markers_round_trip() {
        let mut buf = Buffer::from_text("run ls now");
        let on = toggle_wrap(&mut buf, &[4..6], "`");
        assert_eq!(buf.rope().to_string(), "run `ls` now");
        toggle_wrap(&mut buf, &on.selections, "`");
        assert_eq!(buf.rope().to_string(), "run ls now");

        let mut buf = Buffer::from_text("old text");
        let on = toggle_wrap(&mut buf, &[0..3], "~~");
        assert_eq!(buf.rope().to_string(), "~~old~~ text");
        toggle_wrap(&mut buf, &on.selections, "~~");
        assert_eq!(buf.rope().to_string(), "old text");
    }

    #[test]
    fn a_bare_caret_wraps_the_word_under_it() {
        let mut buf = Buffer::from_text("hello world");
        let on = toggle_wrap(&mut buf, &[8..8], "**");
        assert_eq!(buf.rope().to_string(), "hello **world**");
        assert_eq!(on.selections, vec![8..13]);
        toggle_wrap(&mut buf, &on.selections, "**");
        assert_eq!(buf.rope().to_string(), "hello world");
    }

    #[test]
    fn a_caret_on_no_word_drops_an_empty_pair_and_toggles_it_away() {
        let mut buf = Buffer::from_text("a  b");
        let on = toggle_wrap(&mut buf, &[2..2], "**");
        assert_eq!(buf.rope().to_string(), "a **** b");
        assert_eq!(on.selections, vec![4..4], "caret parked between the pair");
        let off = toggle_wrap(&mut buf, &on.selections, "**");
        assert_eq!(buf.rope().to_string(), "a  b");
        assert_eq!(off.selections, vec![2..2]);
    }

    #[test]
    fn multi_selection_fan_out_wraps_every_span_and_round_trips() {
        let mut buf = Buffer::from_text("one two three");
        let on = toggle_wrap(&mut buf, &[0..3, 4..7, 8..13], "**");
        assert_eq!(buf.rope().to_string(), "**one** **two** **three**");
        assert_eq!(on.selections, vec![2..5, 10..13, 18..23]);
        let off = toggle_wrap(&mut buf, &on.selections, "**");
        assert_eq!(buf.rope().to_string(), "one two three");
        assert_eq!(off.selections, vec![0..3, 4..7, 8..13]);
    }

    #[test]
    fn two_carets_in_one_word_wrap_it_once() {
        let mut buf = Buffer::from_text("hello");
        let on = toggle_wrap(&mut buf, &[1..1, 3..3], "**");
        assert_eq!(buf.rope().to_string(), "**hello**");
        assert_eq!(on.selections, vec![2..7], "merged to one span");
    }

    #[test]
    fn wrap_handles_multibyte_text() {
        let mut buf = Buffer::from_text("café bar");
        let on = toggle_wrap(&mut buf, &[0..4], "**");
        assert_eq!(buf.rope().to_string(), "**café** bar");
        assert_eq!(on.selections, vec![2..6]);
        toggle_wrap(&mut buf, &on.selections, "**");
        assert_eq!(buf.rope().to_string(), "café bar");
    }

    #[test]
    fn wrap_is_one_undo_step_and_redoes() {
        let mut buf = Buffer::from_text("one two");
        let out = toggle_wrap(&mut buf, &[0..3, 4..7], "**");
        undo_op(&mut buf, &out);
        assert_eq!(buf.rope().to_string(), "one two");
        assert!(!buf.can_undo(), "the op was exactly `groups` groups");
        redo_op(&mut buf, &out);
        assert_eq!(buf.rope().to_string(), "**one** **two**");
    }

    #[test]
    fn an_op_never_coalesces_into_prior_typing() {
        let mut buf = Buffer::from_text("");
        buf.insert(0, "word"); // an open typing group
        let out = toggle_wrap(&mut buf, &[4..4], "**");
        assert_eq!(buf.rope().to_string(), "**word**");
        undo_op(&mut buf, &out);
        assert_eq!(buf.rope().to_string(), "word", "typing survives the undo");
        assert!(buf.can_undo(), "the typing group is still its own step");
    }

    // ── set_heading ──────────────────────────────────────────────────────────

    #[test]
    fn sets_a_heading_on_a_plain_line() {
        let mut buf = Buffer::from_text("title\nbody\n");
        let out = set_heading(&mut buf, &[2..2], 2);
        assert_eq!(buf.rope().to_string(), "## title\nbody\n");
        assert_eq!(out.selections, vec![5..5], "caret rides past the prefix");
    }

    #[test]
    fn normalizes_mixed_lines_to_one_level() {
        let mut buf = Buffer::from_text("# a\nb\n#### c\n");
        let len = buf.len_chars();
        set_heading(&mut buf, &[0..len], 2);
        assert_eq!(buf.rope().to_string(), "## a\n## b\n## c\n");
    }

    #[test]
    fn level_zero_strips_to_body_text() {
        let mut buf = Buffer::from_text("### x\n#y\n");
        let len = buf.len_chars();
        set_heading(&mut buf, &[0..len], 0);
        assert_eq!(buf.rope().to_string(), "x\ny\n");
    }

    #[test]
    fn heading_is_idempotent_at_the_same_level() {
        let mut buf = Buffer::from_text("## x\n");
        let out = set_heading(&mut buf, &[0..0], 2);
        assert_eq!(out.groups, 0, "already at the level: no undo noise");
        assert_eq!(buf.rope().to_string(), "## x\n");
    }

    #[test]
    fn heading_level_clamps_at_six() {
        let mut buf = Buffer::from_text("x\n");
        set_heading(&mut buf, &[0..0], 9);
        assert_eq!(buf.rope().to_string(), "###### x\n");
    }

    #[test]
    fn heading_is_one_undo_step() {
        let mut buf = Buffer::from_text("# a\nb\n");
        let len = buf.len_chars();
        let out = set_heading(&mut buf, &[0..len], 3);
        assert_eq!(buf.rope().to_string(), "### a\n### b\n");
        undo_op(&mut buf, &out);
        assert_eq!(buf.rope().to_string(), "# a\nb\n");
        assert!(!buf.can_undo());
    }

    // ── toggle_line_prefix ───────────────────────────────────────────────────

    #[test]
    fn bullet_on_plain_lines_then_off_round_trips() {
        let mut buf = Buffer::from_text("a\nb\n");
        let len = buf.len_chars();
        toggle_line_prefix(&mut buf, &[0..len], ListKind::Bullet);
        assert_eq!(buf.rope().to_string(), "- a\n- b\n");
        let len = buf.len_chars();
        toggle_line_prefix(&mut buf, &[0..len], ListKind::Bullet);
        assert_eq!(buf.rope().to_string(), "a\nb\n");
    }

    #[test]
    fn mixed_bullets_normalize_on_first() {
        let mut buf = Buffer::from_text("- a\nb\n");
        let len = buf.len_chars();
        let out = toggle_line_prefix(&mut buf, &[0..len], ListKind::Bullet);
        assert_eq!(buf.rope().to_string(), "- a\n- b\n");
        assert_eq!(out.groups, 1, "the already-bulleted line is untouched");
    }

    #[test]
    fn foreign_bullet_glyphs_count_as_on_and_strip() {
        let mut buf = Buffer::from_text("* a\n+ b\n- c\n");
        let len = buf.len_chars();
        toggle_line_prefix(&mut buf, &[0..len], ListKind::Bullet);
        assert_eq!(buf.rope().to_string(), "a\nb\nc\n");
    }

    #[test]
    fn numbered_on_assigns_a_sequence_and_off_round_trips() {
        let mut buf = Buffer::from_text("a\nb\nc\n");
        let len = buf.len_chars();
        toggle_line_prefix(&mut buf, &[0..len], ListKind::Numbered);
        assert_eq!(buf.rope().to_string(), "1. a\n2. b\n3. c\n");
        let len = buf.len_chars();
        toggle_line_prefix(&mut buf, &[0..len], ListKind::Numbered);
        assert_eq!(buf.rope().to_string(), "a\nb\nc\n");
    }

    #[test]
    fn numbered_normalize_renumbers_a_mixed_run() {
        let mut buf = Buffer::from_text("7. a\nb\n3. c\n");
        let len = buf.len_chars();
        toggle_line_prefix(&mut buf, &[0..len], ListKind::Numbered);
        assert_eq!(buf.rope().to_string(), "1. a\n2. b\n3. c\n");
    }

    #[test]
    fn numbering_converts_bullets_and_back() {
        let mut buf = Buffer::from_text("- a\n- b\n");
        let len = buf.len_chars();
        toggle_line_prefix(&mut buf, &[0..len], ListKind::Numbered);
        assert_eq!(buf.rope().to_string(), "1. a\n2. b\n");
        let len = buf.len_chars();
        toggle_line_prefix(&mut buf, &[0..len], ListKind::Bullet);
        assert_eq!(buf.rope().to_string(), "- a\n- b\n");
    }

    #[test]
    fn blank_lines_are_skipped_but_keep_the_run_together() {
        let mut buf = Buffer::from_text("a\n\nb\n");
        let len = buf.len_chars();
        toggle_line_prefix(&mut buf, &[0..len], ListKind::Numbered);
        assert_eq!(buf.rope().to_string(), "1. a\n\n2. b\n");
        let len = buf.len_chars();
        toggle_line_prefix(&mut buf, &[0..len], ListKind::Numbered);
        assert_eq!(buf.rope().to_string(), "a\n\nb\n", "blanks never block OFF");
    }

    #[test]
    fn list_toggle_preserves_indent() {
        let mut buf = Buffer::from_text("  a\n");
        toggle_line_prefix(&mut buf, &[1..1], ListKind::Bullet);
        assert_eq!(buf.rope().to_string(), "  - a\n");
    }

    #[test]
    fn disjoint_selections_restart_numbering_per_run() {
        let mut buf = Buffer::from_text("a\nb\nx\nc\nd\n");
        // Lines 0-1 and 3-4; line 2 unselected splits the runs.
        toggle_line_prefix(&mut buf, &[0..4, 6..10], ListKind::Numbered);
        assert_eq!(buf.rope().to_string(), "1. a\n2. b\nx\n1. c\n2. d\n");
    }

    #[test]
    fn list_toggle_is_one_undo_step() {
        let mut buf = Buffer::from_text("a\nb\nc\n");
        let len = buf.len_chars();
        let out = toggle_line_prefix(&mut buf, &[0..len], ListKind::Numbered);
        undo_op(&mut buf, &out);
        assert_eq!(buf.rope().to_string(), "a\nb\nc\n");
        assert!(!buf.can_undo());
        redo_op(&mut buf, &out);
        assert_eq!(buf.rope().to_string(), "1. a\n2. b\n3. c\n");
    }

    // ── shift_indent ─────────────────────────────────────────────────────────

    #[test]
    fn indents_selected_lines_two_spaces_and_round_trips() {
        let mut buf = Buffer::from_text("a\nb\n");
        let len = buf.len_chars();
        let out = shift_indent(&mut buf, &[0..len], 1);
        assert_eq!(buf.rope().to_string(), "  a\n  b\n");
        assert_eq!(
            out.selections,
            vec![2..len + 4],
            "the end sits above BOTH line pads, so it shifts by four"
        );
        let len = buf.len_chars();
        shift_indent(&mut buf, &[0..len], -1);
        assert_eq!(buf.rope().to_string(), "a\nb\n");
    }

    #[test]
    fn outdent_floors_at_column_zero() {
        let mut buf = Buffer::from_text("    a\n b\nc\n");
        let len = buf.len_chars();
        shift_indent(&mut buf, &[0..len], -1);
        assert_eq!(
            buf.rope().to_string(),
            "  a\nb\nc\n",
            "each line loses at most what it has — never negative"
        );
    }

    #[test]
    fn indent_skips_blank_lines() {
        let mut buf = Buffer::from_text("a\n\nb\n");
        let len = buf.len_chars();
        shift_indent(&mut buf, &[0..len], 1);
        assert_eq!(buf.rope().to_string(), "  a\n\n  b\n");
    }

    #[test]
    fn indent_is_one_undo_step() {
        let mut buf = Buffer::from_text("a\nb\n");
        let len = buf.len_chars();
        let out = shift_indent(&mut buf, &[0..len], 1);
        undo_op(&mut buf, &out);
        assert_eq!(buf.rope().to_string(), "a\nb\n");
        assert!(!buf.can_undo());
    }

    // ── insert_table ─────────────────────────────────────────────────────────

    #[test]
    fn builds_the_skeleton_shape() {
        let mut buf = Buffer::from_text("");
        let out = insert_table(&mut buf, 0, 2, 3);
        assert_eq!(
            buf.rope().to_string(),
            "| Col 1 | Col 2 | Col 3 |\n\
             | ----- | ----- | ----- |\n\
             |       |       |       |\n\
             |       |       |       |\n"
        );
        assert_eq!(out.selections, vec![2..7], "`Col 1` ready to overtype");
        assert_eq!(out.groups, 1, "the whole skeleton is one insert");
    }

    #[test]
    fn inserts_at_the_caret_line_start() {
        let mut buf = Buffer::from_text("text\n");
        insert_table(&mut buf, 2, 1, 1);
        assert_eq!(
            buf.rope().to_string(),
            "| Col 1 |\n| ----- |\n|       |\ntext\n",
            "the caret's line is pushed below the table"
        );
    }

    #[test]
    fn clamps_rows_and_cols_to_one() {
        let mut buf = Buffer::from_text("");
        insert_table(&mut buf, 0, 0, 0);
        assert_eq!(buf.rope().to_string(), "| Col 1 |\n| ----- |\n|       |\n");
    }

    #[test]
    fn wide_column_counts_pad_uniformly() {
        let mut buf = Buffer::from_text("");
        insert_table(&mut buf, 0, 1, 10);
        let text = buf.rope().to_string();
        let mut lines = text.lines();
        let header = lines.next().expect("header row");
        assert!(
            header.starts_with("| Col 1  |"),
            "short labels pad to width"
        );
        assert!(header.ends_with("| Col 10 |"));
        let widths: Vec<usize> = text.lines().map(str::len).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "every row is the same width: {widths:?}"
        );
    }

    #[test]
    fn table_is_a_single_undo_step() {
        let mut buf = Buffer::from_text("tail\n");
        let out = insert_table(&mut buf, 0, 3, 2);
        undo_op(&mut buf, &out);
        assert_eq!(buf.rope().to_string(), "tail\n");
        assert!(!buf.can_undo());
    }
}
