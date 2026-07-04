//! The **rope buffer core** (EDITOR-2): the real, editable document model the
//! editor surface is built on.
//!
//! [`Buffer`] wraps a [`ropey::Rope`] — the same O(log n) rope used by
//! Zed/Helix-style editors — so open, line-index, insert, remove, and save all
//! stay cheap on a large file (no per-keystroke O(n) full-string
//! materialization). It is a **pure data structure**: no egui, no UI. EDITOR-1
//! landed the mountable shell + honest empty state; EDITOR-3's text widget will
//! consume this `Buffer` to render and edit an open document. It is
//! runtime-reachable now — a genuinely usable type with real behaviour, never a
//! stub (§7).
//!
//! What it gives the widget:
//!
//! * **Open** ([`Buffer::open`]) — reads a file encoding-aware: valid `UTF-8`
//!   loads verbatim; invalid bytes fall back to a lossy decode and set
//!   [`Buffer::is_lossy`] so the surface can tell the operator honestly.
//! * **Line index** — [`Buffer::len_lines`] / [`Buffer::char_to_line`] /
//!   [`Buffer::line_to_char`] ride `ropey`'s line metric (O(log n)).
//! * **Edit** — [`Buffer::insert`] / [`Buffer::remove`] work on `ropey` char
//!   indices and mark the buffer dirty.
//! * **Undo/redo** — reversible, grouped edits. Consecutive same-kind,
//!   contiguous edits (typing a run of characters, or a run of backspaces)
//!   auto-coalesce into ONE undo group; the widget draws an explicit boundary
//!   between groups with [`Buffer::commit_group`] (time-independent, so tests are
//!   deterministic — no `Instant::now`). [`Buffer::undo`] / [`Buffer::redo`]
//!   restore the text and return a cursor-position hint. History is bounded.
//! * **Save** — [`Buffer::save`] / [`Buffer::save_as`] stream the rope to disk
//!   (the on-disk bytes actually change) and clear the dirty flag.
//! * **Edit deltas** (EDITOR-5) — every rope mutation (insert, remove, and each
//!   op an undo/redo replays) records an [`EditDelta`] in tree-sitter's
//!   `InputEdit` shape; [`Buffer::take_edits`] drains them so the highlighter
//!   re-parses **incrementally** (`Tree::edit` + reparse), never a full-file
//!   pass per keystroke. The queue is bounded: on overflow the drain reports
//!   `None` and the consumer does one full reparse.

// `module_name_repetitions`: `Buffer` is the domain name for this module's one
// public type; renaming it to avoid echoing the `buffer` module would be worse.
// `missing_const_for_fn` (nursery) is over-eager here: whether a small mutator
// happens to be `const`-callable is an implementation detail we don't want to
// pin into the buffer's public contract (same call the repo makes in
// `mde-files-egui` / `mde-media-core`).
#![allow(clippy::module_name_repetitions, clippy::missing_const_for_fn)]

use std::borrow::Cow;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};

use ropey::Rope;

/// Upper bound on retained undo groups. Old groups fall off the front once the
/// history exceeds this, so a long editing session never grows history without
/// limit while still covering any realistic undo reach.
const HISTORY_LIMIT: usize = 256;

/// Upper bound on pending [`EditDelta`]s held for the highlighter. When nothing
/// drains the ledger (a plain-text buffer with no highlighter attached, or a
/// massive scripted edit run between frames), the ledger overflows instead of
/// growing without limit — the consumer then does one full reparse.
const EDIT_LEDGER_LIMIT: usize = 1024;

/// One rope mutation in tree-sitter's `InputEdit` shape (EDITOR-5).
///
/// Byte offsets plus `(row, byte-column)` points for the edit's start, its old
/// end, and its new end. Recorded by **every** rope mutation — insert, remove,
/// and each op an undo/redo replays — so an incremental re-parser can splice
/// its old tree instead of re-reading the whole document per keystroke.
///
/// Kept toolkit-free (plain `usize` tuples, no tree-sitter types) so the buffer
/// stays a pure data structure; the highlight engine converts to `InputEdit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditDelta {
    /// Byte offset where the edit begins.
    pub start_byte: usize,
    /// Byte offset one past the replaced span, in the **old** text.
    pub old_end_byte: usize,
    /// Byte offset one past the inserted span, in the **new** text.
    pub new_end_byte: usize,
    /// `(row, byte-column)` of the edit start.
    pub start_point: (usize, usize),
    /// `(row, byte-column)` one past the replaced span, in the old text.
    pub old_end_point: (usize, usize),
    /// `(row, byte-column)` one past the inserted span, in the new text.
    pub new_end_point: (usize, usize),
}

/// The bounded pending-delta queue between the buffer and its highlighter.
///
/// Deltas accumulate here on every mutation and are drained by
/// [`Buffer::take_edits`]. Past [`EDIT_LEDGER_LIMIT`] the ledger flips to
/// `overflowed` and drops its backlog: the next drain reports the overflow so
/// the consumer performs one full reparse instead of replaying a huge tail.
#[derive(Default)]
struct EditLedger {
    deltas: Vec<EditDelta>,
    overflowed: bool,
}

impl EditLedger {
    /// Queue one delta, flipping to overflow at the cap.
    fn push(&mut self, delta: EditDelta) {
        if self.overflowed {
            return;
        }
        if self.deltas.len() >= EDIT_LEDGER_LIMIT {
            self.overflowed = true;
            self.deltas.clear();
            return;
        }
        self.deltas.push(delta);
    }

    /// Drain: `Some(deltas)` to replay incrementally, `None` when the ledger
    /// overflowed (full reparse required). Either way the ledger resets.
    fn take(&mut self) -> Option<Vec<EditDelta>> {
        if std::mem::take(&mut self.overflowed) {
            self.deltas.clear();
            return None;
        }
        Some(std::mem::take(&mut self.deltas))
    }
}

/// The `(byte offset, (row, byte-column))` of char index `char_idx` — the
/// tree-sitter-shaped coordinates of one buffer position (O(log n)).
fn byte_point(rope: &Rope, char_idx: usize) -> (usize, (usize, usize)) {
    let byte = rope.char_to_byte(char_idx);
    let row = rope.char_to_line(char_idx);
    (byte, (row, byte - rope.line_to_byte(row)))
}

/// THE insert primitive: splice `text` into `rope` at char `at`, recording the
/// [`EditDelta`] onto `ledger`. Every insertion — direct edits and undo/redo
/// replays — funnels through here so no rope mutation escapes the ledger.
fn splice_insert(rope: &mut Rope, ledger: &mut EditLedger, at: usize, text: &str) {
    let (start_byte, start_point) = byte_point(rope, at);
    rope.insert(at, text);
    let (new_end_byte, new_end_point) = byte_point(rope, at + text.chars().count());
    ledger.push(EditDelta {
        start_byte,
        old_end_byte: start_byte,
        new_end_byte,
        start_point,
        old_end_point: start_point,
        new_end_point,
    });
}

/// THE remove primitive: cut the char `range` out of `rope`, recording the
/// [`EditDelta`] onto `ledger` (the counterpart of [`splice_insert`]).
fn splice_remove(rope: &mut Rope, ledger: &mut EditLedger, range: Range<usize>) {
    let (start_byte, start_point) = byte_point(rope, range.start);
    let (old_end_byte, old_end_point) = byte_point(rope, range.end);
    rope.remove(range);
    ledger.push(EditDelta {
        start_byte,
        old_end_byte,
        new_end_byte: start_byte,
        start_point,
        old_end_point,
        new_end_point: start_point,
    });
}

/// One reversible edit against the rope, stored so it can be replayed (redo) or
/// inverted (undo). `Remove` keeps the removed text so undo can re-insert it.
enum EditOp {
    /// `text` was inserted starting at char index `at`.
    Insert { at: usize, text: String },
    /// `text` was removed starting at char index `at`.
    Remove { at: usize, text: String },
}

impl EditOp {
    /// Re-apply this edit forward (used by redo), recording its delta.
    fn apply(&self, rope: &mut Rope, ledger: &mut EditLedger) {
        match self {
            Self::Insert { at, text } => splice_insert(rope, ledger, *at, text),
            Self::Remove { at, text } => {
                splice_remove(rope, ledger, *at..*at + text.chars().count());
            }
        }
    }

    /// Apply this edit's inverse (used by undo), recording its delta: an insert
    /// becomes a remove of the same span, a remove becomes an insert of the
    /// removed text.
    fn invert(&self, rope: &mut Rope, ledger: &mut EditLedger) {
        match self {
            Self::Insert { at, text } => {
                splice_remove(rope, ledger, *at..*at + text.chars().count());
            }
            Self::Remove { at, text } => splice_insert(rope, ledger, *at, text),
        }
    }
}

/// Whether `next` continues `prev` as the same contiguous gesture, so the two
/// coalesce into one undo group: a run of forward typing (each insert right
/// after the previous one), or a run of deletions at the same spot (forward
/// delete) / immediately before it (backspace).
fn coalesces(prev: &EditOp, next: &EditOp) -> bool {
    match (prev, next) {
        (
            EditOp::Insert {
                at: prev_start,
                text: prev_text,
            },
            EditOp::Insert { at: next_start, .. },
        ) => *next_start == *prev_start + prev_text.chars().count(),
        (
            EditOp::Remove { at: prev_start, .. },
            EditOp::Remove {
                at: next_start,
                text: next_text,
            },
        ) => *next_start == *prev_start || *next_start + next_text.chars().count() == *prev_start,
        _ => false,
    }
}

/// One undo unit: the ordered ops applied as a single gesture, plus the cursor
/// hints to restore. `cursor_before` is where the caret sat before the group
/// (restored on undo); `cursor_after` is where it ended (restored on redo).
struct EditGroup {
    ops: Vec<EditOp>,
    cursor_before: usize,
    cursor_after: usize,
}

/// Bounded undo/redo history over reversible [`EditGroup`]s.
///
/// `undo` is the stack of applied groups (newest last); `redo` holds groups that
/// were undone. `open` marks whether the top group still accepts coalescing —
/// [`commit`](History::commit), an undo, a redo, or a non-contiguous edit all
/// close it so the next edit starts a fresh group.
#[derive(Default)]
struct History {
    undo: Vec<EditGroup>,
    redo: Vec<EditGroup>,
    open: bool,
}

impl History {
    /// Record `op` (already applied to the rope). Coalesces into the open top
    /// group when it continues the same gesture, else starts a new group. Any
    /// new edit invalidates the redo stack.
    fn record(&mut self, op: EditOp, cursor_before: usize, cursor_after: usize) {
        self.redo.clear();
        if self.open {
            if let Some(group) = self.undo.last_mut() {
                if group.ops.last().is_some_and(|prev| coalesces(prev, &op)) {
                    group.ops.push(op);
                    group.cursor_after = cursor_after;
                    return;
                }
            }
        }
        self.undo.push(EditGroup {
            ops: vec![op],
            cursor_before,
            cursor_after,
        });
        self.open = true;
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
    }

    /// Close the open group so the next edit begins a new one.
    fn commit(&mut self) {
        self.open = false;
    }

    /// Undo the newest group, mutating `rope` (deltas onto `ledger`); returns
    /// its `cursor_before` hint.
    fn undo(&mut self, rope: &mut Rope, ledger: &mut EditLedger) -> Option<usize> {
        self.open = false;
        let group = self.undo.pop()?;
        for op in group.ops.iter().rev() {
            op.invert(rope, ledger);
        }
        let cursor = group.cursor_before;
        self.redo.push(group);
        Some(cursor)
    }

    /// Redo the most recently undone group, mutating `rope` (deltas onto
    /// `ledger`); returns its `cursor_after` hint.
    fn redo(&mut self, rope: &mut Rope, ledger: &mut EditLedger) -> Option<usize> {
        let group = self.redo.pop()?;
        for op in &group.ops {
            op.apply(rope, ledger);
        }
        let cursor = group.cursor_after;
        self.open = false;
        self.undo.push(group);
        Some(cursor)
    }
}

/// A `ropey`-backed text buffer: the editable document model (EDITOR-2).
///
/// Holds the rope, the on-disk path (if any), a clean/dirty flag, whether the
/// last open decoded lossily, and the undo/redo history. See the [module
/// docs](self) for the full picture. Cheap on large files — every edit is an
/// O(log n) rope splice, never a full-string rebuild.
pub struct Buffer {
    rope: Rope,
    path: Option<PathBuf>,
    dirty: bool,
    lossy: bool,
    history: History,
    edits: EditLedger,
}

impl Buffer {
    /// A fresh, empty, unnamed scratch buffer (a brand-new untitled document).
    #[must_use]
    pub fn scratch() -> Self {
        Self {
            rope: Rope::new(),
            path: None,
            dirty: false,
            lossy: false,
            history: History::default(),
            edits: EditLedger::default(),
        }
    }

    /// An in-memory buffer holding `text`, with no path and a clean flag (a
    /// scratch document seeded with content — handy for previews and tests).
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            path: None,
            dirty: false,
            lossy: false,
            history: History::default(),
            edits: EditLedger::default(),
        }
    }

    /// Open `path`, reading it encoding-aware.
    ///
    /// Valid `UTF-8` loads verbatim. Invalid bytes are replaced (lossy decode)
    /// and [`is_lossy`](Self::is_lossy) is set, so the surface can tell the
    /// operator the file was not clean `UTF-8` rather than silently corrupt it.
    /// The buffer starts clean (not dirty). The whole file is read once here (an
    /// open is allowed to be O(n)); subsequent edits stay O(log n).
    ///
    /// # Errors
    /// Returns any [`io::Error`] from reading `path` (missing file, permissions,
    /// I/O failure).
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let path = path.as_ref();
        let bytes = std::fs::read(path)?;
        let decoded = String::from_utf8_lossy(&bytes);
        let lossy = matches!(&decoded, Cow::Owned(_));
        Ok(Self {
            rope: Rope::from_str(&decoded),
            path: Some(path.to_path_buf()),
            dirty: false,
            lossy,
            history: History::default(),
            edits: EditLedger::default(),
        })
    }

    /// The buffer's on-disk path, or `None` for an unsaved scratch buffer.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Whether the buffer has unsaved edits.
    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Whether the last [`open`](Self::open) replaced invalid `UTF-8` bytes (the
    /// on-disk file was not clean `UTF-8`).
    #[must_use]
    pub const fn is_lossy(&self) -> bool {
        self.lossy
    }

    /// Borrow the underlying [`Rope`] — the read seam the text widget renders
    /// from (line slices, char iteration) without copying the whole document.
    #[must_use]
    pub const fn rope(&self) -> &Rope {
        &self.rope
    }

    /// Total number of characters in the buffer.
    #[must_use]
    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    /// Number of lines (`ropey`'s line metric; a trailing newline yields a final
    /// empty line, and an empty buffer still counts as one line).
    #[must_use]
    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    /// The 0-based line containing char index `char_idx` (O(log n)).
    ///
    /// # Panics
    /// Panics if `char_idx` is past the end of the buffer (`ropey`'s contract).
    #[must_use]
    pub fn char_to_line(&self, char_idx: usize) -> usize {
        self.rope.char_to_line(char_idx)
    }

    /// The char index of the first character of line `line_idx` (O(log n)).
    ///
    /// # Panics
    /// Panics if `line_idx` is past the end of the buffer (`ropey`'s contract).
    #[must_use]
    pub fn line_to_char(&self, line_idx: usize) -> usize {
        self.rope.line_to_char(line_idx)
    }

    /// The text of line `line_idx`, including its trailing newline if present.
    /// Materializes only that ONE line, never the whole document.
    ///
    /// # Panics
    /// Panics if `line_idx` is past the end of the buffer (`ropey`'s contract).
    #[must_use]
    pub fn line(&self, line_idx: usize) -> String {
        self.rope.line(line_idx).to_string()
    }

    /// Insert `text` at char index `char_idx`, marking the buffer dirty and
    /// recording the edit for undo. An empty `text` is a no-op. O(log n) — it
    /// never rebuilds the whole string.
    ///
    /// # Panics
    /// Panics if `char_idx` is past the end of the buffer (`ropey`'s contract).
    pub fn insert(&mut self, char_idx: usize, text: &str) {
        if text.is_empty() {
            return;
        }
        splice_insert(&mut self.rope, &mut self.edits, char_idx, text);
        let after = char_idx + text.chars().count();
        self.history.record(
            EditOp::Insert {
                at: char_idx,
                text: text.to_owned(),
            },
            char_idx,
            after,
        );
        self.dirty = true;
    }

    /// Remove the characters in `range` (char indices), marking the buffer dirty
    /// and recording the edit for undo. An empty/reversed range is a no-op.
    /// O(log n).
    ///
    /// # Panics
    /// Panics if `range` extends past the end of the buffer (`ropey`'s
    /// contract).
    pub fn remove(&mut self, range: Range<usize>) {
        let (start, end) = (range.start, range.end);
        if start >= end {
            return;
        }
        let removed = self.rope.slice(start..end).to_string();
        splice_remove(&mut self.rope, &mut self.edits, start..end);
        self.history.record(
            EditOp::Remove {
                at: start,
                text: removed,
            },
            end,
            start,
        );
        self.dirty = true;
    }

    /// Drain the [`EditDelta`]s recorded since the last drain — the seam an
    /// incremental highlighter syncs its tree through (EDITOR-5).
    ///
    /// `Some(deltas)` (possibly empty) replays incrementally in order; `None`
    /// means the ledger overflowed (more than [`EDIT_LEDGER_LIMIT`] mutations
    /// piled up undrained), so the consumer must re-parse from scratch. Either
    /// way the ledger is reset.
    pub fn take_edits(&mut self) -> Option<Vec<EditDelta>> {
        self.edits.take()
    }

    /// Close the current undo group. The next edit starts a fresh group even if
    /// it would otherwise coalesce — the boundary the widget draws between
    /// distinct gestures (e.g. after a caret jump or a paste).
    pub fn commit_group(&mut self) {
        self.history.commit();
    }

    /// Undo the most recent edit group, restoring the text. Returns a
    /// cursor-position hint (the char index the caret sat at before the group),
    /// or `None` if there is nothing to undo.
    pub fn undo(&mut self) -> Option<usize> {
        let cursor = self.history.undo(&mut self.rope, &mut self.edits)?;
        self.dirty = true;
        Some(cursor)
    }

    /// Redo the most recently undone group, re-applying the text. Returns a
    /// cursor-position hint (the char index the caret ended at), or `None` if
    /// there is nothing to redo.
    pub fn redo(&mut self) -> Option<usize> {
        let cursor = self.history.redo(&mut self.rope, &mut self.edits)?;
        self.dirty = true;
        Some(cursor)
    }

    /// Whether there is an edit group available to [`undo`](Self::undo).
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.history.undo.is_empty()
    }

    /// Whether there is an undone group available to [`redo`](Self::redo).
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.history.redo.is_empty()
    }

    /// Write the buffer to its current path, clearing the dirty flag. Streams the
    /// rope chunk-by-chunk (no full-string materialization), so the on-disk bytes
    /// actually change.
    ///
    /// # Errors
    /// Returns an [`io::Error`] if the buffer has no path (use
    /// [`save_as`](Self::save_as)) or if the write fails.
    pub fn save(&mut self) -> io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "buffer has no path; use save_as",
            ));
        };
        self.write_to(&path)?;
        self.dirty = false;
        Ok(())
    }

    /// Write the buffer to `path`, adopt it as the buffer's path, and clear the
    /// dirty flag.
    ///
    /// # Errors
    /// Returns an [`io::Error`] if the write fails.
    pub fn save_as<P: AsRef<Path>>(&mut self, path: P) -> io::Result<()> {
        let path = path.as_ref().to_path_buf();
        self.write_to(&path)?;
        self.path = Some(path);
        self.dirty = false;
        Ok(())
    }

    /// Stream the rope to `path`, creating/truncating it. Buffered + flushed so
    /// the bytes are on disk when this returns.
    fn write_to(&self, path: &Path) -> io::Result<()> {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);
        self.rope.write_to(&mut writer)?;
        writer.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::Buffer;
    use std::time::{Duration, Instant};

    #[test]
    fn open_reads_a_file_back_verbatim() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hello.txt");
        std::fs::write(&path, b"hello\nworld\n").expect("seed file");

        let buf = Buffer::open(&path).expect("open");

        assert_eq!(buf.rope().to_string(), "hello\nworld\n");
        assert!(!buf.is_dirty(), "a freshly opened file is clean");
        assert!(!buf.is_lossy(), "clean UTF-8 must not flag lossy");
        assert_eq!(buf.path(), Some(path.as_path()));
    }

    #[test]
    fn open_invalid_utf8_falls_back_lossy_and_flags_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bad.bin");
        // 0xff / 0xfe are never valid UTF-8 lead bytes.
        std::fs::write(&path, [0xff_u8, 0xfe, b'h', b'i']).expect("seed file");

        let buf = Buffer::open(&path).expect("open");

        assert!(buf.is_lossy(), "invalid UTF-8 must set the lossy flag");
        let text = buf.rope().to_string();
        assert!(text.ends_with("hi"), "the valid tail survives the decode");
        assert!(
            text.contains('\u{FFFD}'),
            "invalid bytes decode to the replacement char"
        );
    }

    #[test]
    fn insert_and_remove_are_correct_and_mark_dirty() {
        let mut buf = Buffer::from_text("hello");

        buf.insert(5, " world");
        assert_eq!(buf.rope().to_string(), "hello world");
        assert_eq!(buf.len_chars(), 11);
        assert!(buf.is_dirty(), "an edit marks the buffer dirty");

        buf.remove(0..6); // drop "hello "
        assert_eq!(buf.rope().to_string(), "world");
        assert_eq!(buf.len_chars(), 5);
    }

    #[test]
    fn empty_edits_are_no_ops() {
        let mut buf = Buffer::from_text("abc");
        buf.insert(0, "");
        buf.remove(2..2); // empty range
                          // A reversed range (start > end) is also a no-op; build it from values so
                          // it isn't a lint-visible literal reversed range.
        let (hi, lo) = (3_usize, 1_usize);
        buf.remove(hi..lo);
        assert_eq!(buf.rope().to_string(), "abc");
        assert!(!buf.is_dirty(), "no-op edits leave the buffer clean");
        assert!(!buf.can_undo(), "no-op edits record no history");
    }

    #[test]
    fn line_index_navigates_by_line_and_char() {
        let buf = Buffer::from_text("aa\nbbb\nc\n");
        // "aa\n" | "bbb\n" | "c\n" | "" (trailing newline yields a final line)
        assert_eq!(buf.len_lines(), 4);
        assert_eq!(buf.char_to_line(0), 0);
        assert_eq!(buf.char_to_line(3), 1, "char 3 is the first of line 1");
        assert_eq!(buf.line_to_char(1), 3);
        assert_eq!(buf.line(1), "bbb\n");
    }

    #[test]
    fn grouped_typing_undoes_as_one_group() {
        let mut buf = Buffer::from_text("");
        for (i, ch) in ["h", "e", "l", "l", "o"].iter().enumerate() {
            buf.insert(i, ch);
        }
        assert_eq!(buf.rope().to_string(), "hello");
        assert!(buf.can_undo());

        // Five contiguous inserts collapse to ONE undo group.
        assert_eq!(buf.undo(), Some(0), "undo restores the pre-group caret");
        assert_eq!(buf.rope().to_string(), "");
        assert!(!buf.can_undo(), "the run was a single group");
        assert!(buf.can_redo());

        assert_eq!(buf.redo(), Some(5), "redo restores the post-group caret");
        assert_eq!(buf.rope().to_string(), "hello");
        assert!(!buf.can_redo());
    }

    #[test]
    fn commit_group_starts_a_fresh_undo_boundary() {
        let mut buf = Buffer::from_text("");
        buf.insert(0, "a");
        buf.insert(1, "b"); // coalesces -> group "ab"
        buf.commit_group();
        buf.insert(2, "c");
        buf.insert(3, "d"); // new group "cd"
        assert_eq!(buf.rope().to_string(), "abcd");

        buf.undo();
        assert_eq!(
            buf.rope().to_string(),
            "ab",
            "first undo drops only the second group"
        );
        buf.undo();
        assert_eq!(buf.rope().to_string(), "");
        assert!(!buf.can_undo());
    }

    #[test]
    fn contiguous_backspaces_undo_as_one_group() {
        let mut buf = Buffer::from_text("abc");
        buf.remove(2..3); // -> "ab"
        buf.remove(1..2); // -> "a"  (backspace, coalesces)
        buf.remove(0..1); // -> ""   (backspace, coalesces)
        assert_eq!(buf.rope().to_string(), "");
        assert!(buf.can_undo());

        buf.undo();
        assert_eq!(
            buf.rope().to_string(),
            "abc",
            "three backspaces undo together"
        );
        assert!(!buf.can_undo(), "the run was a single group");
    }

    #[test]
    fn undo_redo_on_empty_history_returns_none() {
        let mut buf = Buffer::from_text("x");
        assert_eq!(buf.undo(), None);
        assert_eq!(buf.redo(), None);
        assert_eq!(buf.rope().to_string(), "x");
    }

    #[test]
    fn save_changes_the_bytes_on_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("edit.txt");
        std::fs::write(&path, b"abc").expect("seed file");

        let mut buf = Buffer::open(&path).expect("open");
        buf.insert(3, "DEF");
        assert!(buf.is_dirty());

        buf.save().expect("save");
        assert!(!buf.is_dirty(), "save clears the dirty flag");
        assert_eq!(
            std::fs::read(&path).expect("read back"),
            b"abcDEF",
            "the on-disk bytes actually changed"
        );
    }

    #[test]
    fn save_as_writes_and_adopts_the_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("as.txt");

        let mut buf = Buffer::from_text("x\n");
        assert!(buf.path().is_none());

        buf.save_as(&target).expect("save_as");
        assert_eq!(
            buf.path(),
            Some(target.as_path()),
            "save_as adopts the path"
        );
        assert!(!buf.is_dirty());
        assert_eq!(std::fs::read(&target).expect("read back"), b"x\n");
    }

    #[test]
    fn save_without_a_path_errors() {
        let mut buf = Buffer::from_text("scratch");
        let err = buf.save().expect_err("scratch buffer has no path");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    // ── EDITOR-5: edit deltas for the incremental highlighter ────────────────

    #[test]
    fn insert_records_a_byte_and_point_delta_across_lines_and_multibyte() {
        let mut buf = Buffer::from_text("ab\ncd");
        buf.take_edits().expect("fresh ledger drains empty");

        // Insert a multi-byte, multi-line snippet before the final 'd' (char 4 =
        // byte 4, line 1 col 1). "xé\nz" is 4 chars / 5 bytes with one newline.
        buf.insert(4, "xé\nz");

        let deltas = buf.take_edits().expect("no overflow");
        assert_eq!(deltas.len(), 1);
        let d = deltas[0];
        assert_eq!(d.start_byte, 4);
        assert_eq!(d.old_end_byte, 4, "an insert replaces nothing");
        assert_eq!(d.new_end_byte, 9, "5 inserted BYTES (é is 2)");
        assert_eq!(d.start_point, (1, 1));
        assert_eq!(d.old_end_point, (1, 1));
        assert_eq!(
            d.new_end_point,
            (2, 1),
            "the newline in the insert lands the new end on row 2 byte-col 1"
        );
    }

    #[test]
    fn remove_records_the_replaced_span_and_collapses_to_the_start() {
        let mut buf = Buffer::from_text("ab\ncxé\nzd");
        buf.take_edits().expect("drain the empty ledger");

        // Cut chars 4..8 ("xé\nz") — the exact inverse of the insert above.
        buf.remove(4..8);

        let deltas = buf.take_edits().expect("no overflow");
        assert_eq!(deltas.len(), 1);
        let d = deltas[0];
        assert_eq!(d.start_byte, 4);
        assert_eq!(d.old_end_byte, 9, "the removed span was 5 bytes");
        assert_eq!(d.new_end_byte, 4, "a remove inserts nothing");
        assert_eq!(d.start_point, (1, 1));
        assert_eq!(d.old_end_point, (2, 1));
        assert_eq!(d.new_end_point, (1, 1));
    }

    #[test]
    fn undo_and_redo_record_deltas_too() {
        let mut buf = Buffer::from_text("abc");
        buf.insert(3, "X");
        buf.take_edits().expect("drain the insert");

        buf.undo();
        let undo_deltas = buf.take_edits().expect("no overflow");
        assert_eq!(undo_deltas.len(), 1, "the undo's remove was recorded");
        assert_eq!(undo_deltas[0].old_end_byte, 4);
        assert_eq!(undo_deltas[0].new_end_byte, 3);

        buf.redo();
        let redo_deltas = buf.take_edits().expect("no overflow");
        assert_eq!(redo_deltas.len(), 1, "the redo's insert was recorded");
        assert_eq!(redo_deltas[0].old_end_byte, 3);
        assert_eq!(redo_deltas[0].new_end_byte, 4);
    }

    #[test]
    fn an_undrained_ledger_overflows_to_a_full_reparse_signal() {
        let mut buf = Buffer::from_text("");
        // Far past the cap without a drain (a highlighter-less buffer's life).
        for i in 0..1_500 {
            buf.insert(i, "a");
        }
        assert_eq!(
            buf.take_edits(),
            None,
            "an overflowed ledger reports None (full reparse)"
        );
        // The overflow drained + reset the ledger: recording works again.
        buf.insert(0, "b");
        let deltas = buf
            .take_edits()
            .expect("post-overflow ledger records again");
        assert_eq!(deltas.len(), 1);
    }

    #[test]
    fn large_buffer_edits_near_the_end_do_not_stall() {
        // 100k lines: construction + an edit run near the very end must stay fast
        // (a coarse "doesn't materialize the whole string per keystroke" sanity,
        // not a strict benchmark).
        let text = "lorem ipsum dolor sit\n".repeat(100_000);
        let mut buf = Buffer::from_text(&text);
        assert_eq!(buf.len_lines(), 100_001);

        let base = buf.len_chars();
        let start = Instant::now();
        for _ in 0..2_000 {
            let end = buf.len_chars();
            buf.insert(end, "x");
        }
        let elapsed = start.elapsed();

        assert_eq!(buf.len_chars(), base + 2_000);
        let tail = buf.rope().slice(base..).to_string();
        assert!(tail.chars().all(|c| c == 'x'), "the appended run is intact");
        assert!(
            elapsed < Duration::from_secs(30),
            "editing a 100k-line buffer should not stall: took {elapsed:?}"
        );
    }
}
