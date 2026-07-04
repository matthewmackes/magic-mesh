//! EDITOR-LSP-3 — **cross-file navigation + workspace edits**: the UI + apply
//! layer over the [`lsp`](crate::lsp) client's navigation requests.
//!
//! The client ([`lsp`](crate::lsp)) speaks the wire — `textDocument/definition`,
//! `references`, `prepareRename` / `rename`, and `formatting` — and folds each
//! reply to a flat local type ([`Location`], [`WorkspaceEdit`], [`TextEdit`],
//! [`PrepareRename`]). This module is what the panel drives on top of that:
//!
//! * **Position glue** — [`char_to_lsp_pos`] turns the caret's rope char offset
//!   into the LSP wire position (zero-based line + UTF-16 column) a request
//!   needs; [`resolve_range`] / [`char_of`] turn a reply's wire position back
//!   into a rope char range to jump onto or splice. Both share `lsp_ui`'s
//!   [`resolve_pos`](crate::lsp_ui::resolve_pos) so a jump and a diagnostic land
//!   on the same char.
//! * **The apply seams** — [`apply_edits_to_open_buffer`] lands a file's edits on
//!   its live [`EditorView`] as one undoable step (a format, or the open-buffer
//!   leg of a rename); [`apply_edits_on_disk`] rewrites a file that isn't open
//!   (the closed-buffer leg). A rename's [`WorkspaceEdit`] fans across both.
//! * **Two overlays** — [`ReferencesPanel`] (the `file:line:col preview` results
//!   list, the EDITOR-8 project-search idiom) and [`RenameBox`] (the small
//!   new-name input), both pure state + a thin token-styled render (§4).
//!
//! Everything here is pure state + methods over `(&Buffer / &mut EditorView)` and
//! flat data — no live server — so the position math and the edit application are
//! unit-tested directly (§7); the request → reply round-trip is proven in
//! [`lsp`](crate::lsp)'s fake-server tests.

// The same allow `search.rs` / `finder.rs` make: `module_name_repetitions` — the
// public types are the domain names for this module; `missing_const_for_fn`
// (nursery) is over-eager on the small overlay mutators.
#![allow(clippy::module_name_repetitions, clippy::missing_const_for_fn)]

use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

use mde_egui::egui::{self, Align, Align2, Key, Layout, Modifiers, RichText, ScrollArea, Vec2};
use mde_egui::Style;

use crate::buffer::Buffer;
use crate::lsp::{Location, LspRange, TextEdit};
use crate::lsp_ui::resolve_pos;
use crate::widget::EditorView;

/// The references / rename overlay plate width — twelve shared spacing units (§4).
const PANEL_WIDTH: f32 = Style::SP_XL * 12.0;
/// The drop from the panel top the anchored overlay sits at (matches the
/// EDITOR-8 search overlays).
const TOP_DROP: f32 = Style::SP_XL * 2.0;
/// The references list's max height before it scrolls.
const LIST_MAX_H: f32 = Style::SP_XL * 9.0;
/// The reference-preview length cap (a results row stays one line).
const PREVIEW_CAP: usize = 120;

// ---------------------------------------------------------------------------
// Position glue — rope char offsets ⇄ LSP wire positions (UTF-16 columns).
// ---------------------------------------------------------------------------

/// Narrow a `usize` to `u32` for the LSP wire, saturating rather than wrapping
/// (a document past `u32::MAX` lines/columns is not a real editing case).
fn clamp_u32(x: usize) -> u32 {
    u32::try_from(x).unwrap_or(u32::MAX)
}

/// The LSP wire position (zero-based line, UTF-16 `character`) for the rope char
/// offset `char_idx` — the inverse of
/// [`resolve_pos`](crate::lsp_ui::resolve_pos), used to build a request from the
/// caret. A column counts UTF-16 code units (the protocol default), so an astral
/// char contributes two.
#[must_use]
pub(crate) fn char_to_lsp_pos(buffer: &Buffer, char_idx: usize) -> (u32, u32) {
    let idx = char_idx.min(buffer.len_chars());
    let line = buffer.char_to_line(idx);
    let line_start = buffer.line_to_char(line);
    let utf16: usize = buffer
        .rope()
        .slice(line_start..idx)
        .chars()
        .map(char::len_utf16)
        .sum();
    (clamp_u32(line), clamp_u32(utf16))
}

/// The rope char offset for an LSP position — the jump target for a definition /
/// reference pick (shares `lsp_ui`'s resolver so jumps and diagnostics agree).
#[must_use]
pub(crate) fn char_of(buffer: &Buffer, line: u32, character: u32) -> usize {
    resolve_pos(buffer, line, character)
}

/// The rope char range for an LSP [`LspRange`] (clamped so `end >= start`).
#[must_use]
pub(crate) fn resolve_range(buffer: &Buffer, range: LspRange) -> Range<usize> {
    let start = resolve_pos(buffer, range.start_line, range.start_character);
    let end = resolve_pos(buffer, range.end_line, range.end_character).max(start);
    start..end
}

/// The identifier-like word (`[A-Za-z0-9_]+`) spanning char offset `char_idx`,
/// as `(char-range, text)` — the local rename prefill + validity check, always
/// available with no server round-trip. `None` when the caret is not on a word.
#[must_use]
pub(crate) fn word_under_cursor(buffer: &Buffer, char_idx: usize) -> Option<(Range<usize>, String)> {
    let len = buffer.len_chars();
    let idx = char_idx.min(len);
    let rope = buffer.rope();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut start = idx;
    while start > 0 && is_word(rope.char(start - 1)) {
        start -= 1;
    }
    let mut end = idx;
    while end < len && is_word(rope.char(end)) {
        end += 1;
    }
    if end == start {
        return None;
    }
    Some((start..end, rope.slice(start..end).to_string()))
}

// ---------------------------------------------------------------------------
// The apply seams — land a file's LSP edits on an open buffer or on disk.
// ---------------------------------------------------------------------------

/// Apply `edits` to a live open buffer through its [`EditorView`], as ONE
/// undoable operator step (EDITOR-LSP-3). Every edit is resolved against the
/// buffer *before* the first splice, so the ranges stay valid; returns the count
/// applied. The undo/redo + LSP `didChange` sync fall out of the shared
/// [`EditorView::apply_text_edits`] path, exactly like a typed edit.
pub(crate) fn apply_edits_to_open_buffer(
    view: &mut EditorView,
    buffer: &mut Buffer,
    edits: &[TextEdit],
) -> usize {
    let resolved: Vec<(Range<usize>, String)> = edits
        .iter()
        .map(|e| (resolve_range(buffer, e.range), e.new_text.clone()))
        .collect();
    view.apply_text_edits(buffer, &resolved)
}

/// Apply `edits` to `text` off-buffer, returning the rewritten text — the pure
/// core of [`apply_edits_on_disk`]. Resolves every edit against the original
/// text, then splices high-offset-first so the earlier offsets stay valid.
#[must_use]
pub(crate) fn apply_edits_to_text(text: &str, edits: &[TextEdit]) -> String {
    let mut buffer = Buffer::from_text(text);
    let mut resolved: Vec<(Range<usize>, String)> = edits
        .iter()
        .map(|e| (resolve_range(&buffer, e.range), e.new_text.clone()))
        .collect();
    resolved.sort_by_key(|e| std::cmp::Reverse(e.0.start));
    for (range, new_text) in &resolved {
        if range.end > range.start {
            buffer.remove(range.clone());
        }
        if !new_text.is_empty() {
            buffer.insert(range.start, new_text);
        }
    }
    buffer.rope().to_string()
}

/// Apply `edits` to a file that isn't open in the editor (the closed-buffer leg
/// of a rename [`WorkspaceEdit`]): read, splice, write back.
///
/// # Errors
/// Any [`io::Error`] from reading or writing `path`.
pub(crate) fn apply_edits_on_disk(path: &Path, edits: &[TextEdit]) -> io::Result<()> {
    let text = std::fs::read_to_string(path)?;
    let updated = apply_edits_to_text(&text, edits);
    std::fs::write(path, updated)
}

// ---------------------------------------------------------------------------
// The find-references results list (the EDITOR-8 project-search idiom).
// ---------------------------------------------------------------------------

/// One reference row: the file + display `line:col` (1-based), the zero-based
/// wire position the jump uses, and the trimmed source-line preview.
#[derive(Clone, Debug)]
pub(crate) struct RefRow {
    /// The file the reference is in.
    pub(crate) path: PathBuf,
    /// The 1-based display line.
    pub(crate) line: usize,
    /// The 1-based display column.
    pub(crate) col: usize,
    /// The zero-based line for the jump (LSP wire).
    pub(crate) line0: u32,
    /// The zero-based UTF-16 column for the jump (LSP wire).
    pub(crate) char0: u32,
    /// The trimmed, length-capped source-line preview.
    pub(crate) preview: String,
}

impl RefRow {
    /// Build a row from a resolved [`Location`] + its source-line `preview`.
    pub(crate) fn from_location(loc: &Location, preview: &str) -> Self {
        let mut preview = preview.trim().to_owned();
        if preview.chars().count() > PREVIEW_CAP {
            preview = preview.chars().take(PREVIEW_CAP).collect();
        }
        Self {
            path: loc.path.clone(),
            line: loc.range.start_line as usize + 1,
            col: loc.range.start_character as usize + 1,
            line0: loc.range.start_line,
            char0: loc.range.start_character,
            preview,
        }
    }
}

/// The find-references results overlay (EDITOR-LSP-3) — the `Shift-F12` list the
/// operator picks a reference from to jump to it. The results-list + pick idiom
/// mirrors the EDITOR-8 project search, fed by the LSP references reply.
#[derive(Default)]
pub(crate) struct ReferencesPanel {
    /// Whether the overlay is shown.
    open: bool,
    /// The reference rows.
    rows: Vec<RefRow>,
    /// The highlighted row.
    selected: usize,
}

impl ReferencesPanel {
    /// Whether the overlay is open.
    #[must_use]
    pub(crate) const fn is_open(&self) -> bool {
        self.open
    }

    /// Open the overlay over `rows` (from a references reply), highlighting the
    /// first.
    pub(crate) fn open_with(&mut self, rows: Vec<RefRow>) {
        self.rows = rows;
        self.selected = 0;
        self.open = true;
    }

    /// Close the overlay (Esc, or after a pick).
    pub(crate) fn close(&mut self) {
        self.open = false;
    }

    /// Move the highlight (saturating; the list doesn't wrap) — mirrors the
    /// project-search selection.
    fn move_selection(&mut self, forward: bool) {
        let len = self.rows.len();
        if len == 0 {
            self.selected = 0;
            return;
        }
        let last = len - 1;
        let cur = self.selected.min(last);
        self.selected = if forward {
            (cur + 1).min(last)
        } else {
            cur.saturating_sub(1)
        };
    }

    /// The highlighted row, if any.
    fn selected_row(&self) -> Option<&RefRow> {
        self.rows.get(self.selected)
    }

    /// The rows (test seam).
    #[cfg(test)]
    pub(crate) fn rows(&self) -> &[RefRow] {
        &self.rows
    }

    /// The highlighted index (test seam).
    #[cfg(test)]
    pub(crate) const fn selected_index(&self) -> usize {
        self.selected
    }

    /// Move the highlight (test seam over the private mutator).
    #[cfg(test)]
    pub(crate) fn step(&mut self, forward: bool) {
        self.move_selection(forward);
    }
}

/// Render the references overlay on `ctx` and return the [`RefRow`] the operator
/// opened this frame (Enter on the highlight, or a click), closing the overlay on
/// a pick or Esc. A no-op returning `None` while closed.
pub(crate) fn show_references(ctx: &egui::Context, refs: &mut ReferencesPanel) -> Option<RefRow> {
    if !refs.open {
        return None;
    }
    let (up, down, enter, esc) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::ArrowDown),
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    if esc {
        refs.close();
        return None;
    }
    if up {
        refs.move_selection(false);
    }
    if down {
        refs.move_selection(true);
    }
    let mut picked: Option<RefRow> = if enter {
        refs.selected_row().cloned()
    } else {
        None
    };
    let mut clicked_row: Option<usize> = None;

    egui::Window::new("References")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_TOP, Vec2::new(0.0, TOP_DROP))
        .show(ctx, |ui| {
            ui.set_min_width(PANEL_WIDTH);
            ui.add_space(Style::SP_XS);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{} references", refs.rows.len()))
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM)
                        .strong(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new("\u{21B5} jump  \u{2191}\u{2193} move  esc close")
                            .size(Style::SMALL)
                            .color(Style::TEXT_DIM),
                    );
                });
            });
            ui.add_space(Style::SP_XS);
            ui.separator();

            ScrollArea::vertical()
                .id_salt("editor-lsp-references-list")
                .max_height(LIST_MAX_H)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (row, hit) in refs.rows.iter().enumerate() {
                        if ref_row(ui, hit, row == refs.selected) {
                            clicked_row = Some(row);
                        }
                    }
                });
        });

    if let Some(row) = clicked_row {
        refs.selected = row;
        picked = refs.rows.get(row).cloned();
    }
    if picked.is_some() {
        refs.close();
    }
    picked
}

/// One references row: `file:line:col  preview`, highlighted when selected.
/// Returns `true` when clicked. Token-styled (§4), mirroring the EDITOR-8
/// project-search row.
fn ref_row(ui: &mut egui::Ui, row: &RefRow, selected: bool) -> bool {
    let name = row.path.file_name().map_or_else(
        || row.path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let label = format!("{}:{}:{}  {}", name, row.line, row.col, row.preview);
    ui.selectable_label(
        selected,
        RichText::new(label).size(Style::SMALL).color(Style::TEXT),
    )
    .clicked()
}

// ---------------------------------------------------------------------------
// The rename input box.
// ---------------------------------------------------------------------------

/// The rename new-name input (EDITOR-LSP-3, `F2`): a small field prefilled with
/// the word under the cursor (refined by a `prepareRename` placeholder when the
/// server offers one), holding the symbol's wire position so the panel can fire
/// `textDocument/rename` on submit.
#[derive(Default)]
pub(crate) struct RenameBox {
    /// Whether the box is shown.
    open: bool,
    /// The file the symbol is in.
    path: PathBuf,
    /// The symbol's zero-based line (LSP wire).
    line: u32,
    /// The symbol's zero-based UTF-16 column (LSP wire).
    character: u32,
    /// The live new-name field.
    input: String,
    /// Set on open so the field grabs the keyboard for one frame.
    focus_field: bool,
    /// Whether the operator has edited the field — so a late `prepareRename`
    /// placeholder doesn't clobber what they typed.
    edited: bool,
}

impl RenameBox {
    /// Whether the box is open.
    #[must_use]
    pub(crate) const fn is_open(&self) -> bool {
        self.open
    }

    /// Open the box for the symbol at `(path, line, character)`, prefilled with
    /// `prefill` (the word under the cursor).
    pub(crate) fn open_for(&mut self, path: PathBuf, line: u32, character: u32, prefill: &str) {
        self.open = true;
        self.path = path;
        self.line = line;
        self.character = character;
        prefill.clone_into(&mut self.input);
        self.focus_field = true;
        self.edited = false;
    }

    /// Close the box.
    pub(crate) fn close(&mut self) {
        self.open = false;
    }

    /// Refine the prefill from a `prepareRename` placeholder — only while the box
    /// is open and the operator hasn't started typing (so it never clobbers an
    /// in-progress edit).
    pub(crate) fn set_placeholder(&mut self, placeholder: &str) {
        if self.open && !self.edited {
            placeholder.clone_into(&mut self.input);
        }
    }

    /// The symbol's target `(path, line, character)` — read on submit to fire the
    /// rename request.
    #[must_use]
    pub(crate) fn target(&self) -> (PathBuf, u32, u32) {
        (self.path.clone(), self.line, self.character)
    }

    /// The current field text (test seam).
    #[cfg(test)]
    pub(crate) fn input(&self) -> &str {
        &self.input
    }
}

/// Render the rename box on `ctx` and return the submitted new name (Enter on a
/// non-empty field), or `None`. Esc closes it. The panel reads [`RenameBox::target`]
/// to fire `textDocument/rename`, then closes the box. A no-op returning `None`
/// while closed.
pub(crate) fn show_rename(ctx: &egui::Context, rename: &mut RenameBox) -> Option<String> {
    if !rename.open {
        return None;
    }
    let (enter, esc) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    if esc {
        rename.close();
        return None;
    }
    let mut changed = false;
    egui::Window::new("Rename symbol")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_TOP, Vec2::new(0.0, TOP_DROP))
        .show(ctx, |ui| {
            ui.set_min_width(PANEL_WIDTH);
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new("Rename to")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM)
                    .strong(),
            );
            ui.add_space(Style::SP_XS);
            let field = ui.add(
                egui::TextEdit::singleline(&mut rename.input)
                    .hint_text("new name\u{2026}")
                    .desired_width(f32::INFINITY),
            );
            changed = field.changed();
            if std::mem::take(&mut rename.focus_field) {
                field.request_focus();
            }
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new("\u{21B5} rename  esc cancel")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
        });
    if changed {
        rename.edited = true;
    }
    let name = rename.input.trim();
    if enter && !name.is_empty() {
        Some(name.to_owned())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_edits_on_disk, apply_edits_to_open_buffer, apply_edits_to_text, char_of,
        char_to_lsp_pos, resolve_range, word_under_cursor, ReferencesPanel, RefRow, RenameBox,
    };
    use crate::buffer::Buffer;
    use crate::lsp::{Location, LspRange, TextEdit};
    use crate::widget::EditorView;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// One text edit over a single line at the given UTF-16 columns.
    fn edit(line: u32, c0: u32, c1: u32, new_text: &str) -> TextEdit {
        TextEdit {
            range: LspRange {
                start_line: line,
                start_character: c0,
                end_line: line,
                end_character: c1,
            },
            new_text: new_text.to_owned(),
        }
    }

    fn location(path: &str, line: u32, character: u32) -> Location {
        Location {
            path: PathBuf::from(path),
            range: LspRange {
                start_line: line,
                start_character: character,
                end_line: line,
                end_character: character + 1,
            },
        }
    }

    #[test]
    fn char_pos_round_trips_through_the_wire_including_astral() {
        // "😀xy\nlet z" — the emoji is one char but two UTF-16 units, so the
        // wire column past it is 2 while the char offset is 1.
        let buf = Buffer::from_text("😀xy\nlet z\n");
        // Char offset 1 (after the emoji) → line 0, wire col 2.
        assert_eq!(char_to_lsp_pos(&buf, 1), (0, 2));
        // And back: wire (0, 2) resolves to char 1.
        assert_eq!(char_of(&buf, 0, 2), 1);
        // A caret on line 1 ("let z"): char offset of 'z' = line1_start(5) + 4 = 9.
        let z = buf.line_to_char(1) + 4;
        assert_eq!(char_to_lsp_pos(&buf, z), (1, 4));
        assert_eq!(char_of(&buf, 1, 4), z);
    }

    #[test]
    fn word_under_cursor_spans_the_identifier() {
        let buf = Buffer::from_text("let foo_bar = 1;\n");
        // Anywhere inside "foo_bar" (chars 4..11) returns the whole word.
        let (range, word) = word_under_cursor(&buf, 6).expect("on a word");
        assert_eq!(word, "foo_bar");
        assert_eq!(range, 4..11);
        // Between two non-word chars (offset 13, the space between `=` and `1`,
        // with `=` to its left): not a word.
        assert!(word_under_cursor(&buf, 13).is_none());
    }

    #[test]
    fn apply_edits_to_text_lands_a_multi_edit_format() {
        // Two edits with distinct replacements over one line — the off-buffer
        // (on-disk) apply path.
        let text = "let foo = old;\n";
        let edits = [edit(0, 4, 7, "bar"), edit(0, 10, 13, "new")];
        assert_eq!(apply_edits_to_text(text, &edits), "let bar = new;\n");
    }

    #[test]
    fn apply_edits_to_open_buffer_is_undoable() {
        let mut buf = Buffer::from_text("name here\n");
        let mut view = EditorView::new();
        // Replace "name" (0..4) with "title".
        let edits = [edit(0, 0, 4, "title")];
        let n = apply_edits_to_open_buffer(&mut view, &mut buf, &edits);
        assert_eq!(n, 1);
        assert_eq!(buf.rope().to_string(), "title here\n");
        assert!(view.undo(&mut buf), "the applied edit is undoable in an open buffer");
        assert_eq!(buf.rope().to_string(), "name here\n");
    }

    #[test]
    fn resolve_range_clamps_reversed_to_empty() {
        let buf = Buffer::from_text("abcdef\n");
        let r = resolve_range(
            &buf,
            LspRange {
                start_line: 0,
                start_character: 2,
                end_line: 0,
                end_character: 5,
            },
        );
        assert_eq!(r, 2..5);
    }

    #[test]
    fn apply_edits_on_disk_rewrites_a_closed_file() {
        let base = std::env::temp_dir().join(format!(
            "mde-editor-lspnav-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&base).expect("temp dir");
        let file = base.join("closed.rs");
        std::fs::write(&file, "let foo = 1;\n").expect("seed file");
        apply_edits_on_disk(&file, &[edit(0, 4, 7, "bar")]).expect("apply on disk");
        assert_eq!(
            std::fs::read_to_string(&file).expect("read back"),
            "let bar = 1;\n"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn references_panel_selection_and_rows() {
        let mut panel = ReferencesPanel::default();
        assert!(!panel.is_open());
        let rows = vec![
            RefRow::from_location(&location("/tmp/a.rs", 0, 4), "let a = 1;"),
            RefRow::from_location(&location("/tmp/b.rs", 5, 2), "  use a;"),
        ];
        panel.open_with(rows);
        assert!(panel.is_open());
        assert_eq!(panel.rows().len(), 2);
        assert_eq!(panel.rows()[0].line, 1, "display line is 1-based");
        assert_eq!(panel.rows()[0].preview, "let a = 1;", "the preview is trimmed");
        assert_eq!(panel.selected_index(), 0);
        panel.step(true);
        assert_eq!(panel.selected_index(), 1);
        panel.step(true);
        assert_eq!(panel.selected_index(), 1, "selection saturates at the last row");
        panel.step(false);
        assert_eq!(panel.selected_index(), 0);
    }

    #[test]
    fn rename_box_prefill_placeholder_and_target() {
        let mut rename = RenameBox::default();
        rename.open_for(PathBuf::from("/tmp/x.rs"), 2, 4, "foo");
        assert!(rename.is_open());
        assert_eq!(rename.input(), "foo");
        // A late prepareRename placeholder refines the prefill (operator hasn't typed).
        rename.set_placeholder("foo_refined");
        assert_eq!(rename.input(), "foo_refined");
        assert_eq!(rename.target(), (PathBuf::from("/tmp/x.rs"), 2, 4));
        rename.close();
        assert!(!rename.is_open());
    }
}
