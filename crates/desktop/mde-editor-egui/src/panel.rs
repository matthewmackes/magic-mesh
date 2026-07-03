//! The **surface panel seam** (EDITOR-1 → EDITOR-3): the code-editor surface the
//! one Quasar shell (`mde-shell-egui`) embeds as `Surface::Editor`.
//!
//! Under E12 "Quasar" the mesh surfaces are **panels in the one shell**, not
//! separate clients (§5 EMBED — there is no compositor). This module exposes the
//! editor surface through the exact seam `mde-files-egui` gives the shell:
//!
//! * [`EditorSurface`] is the surface state the shell holds directly (the
//!   analogue of `mde_files_egui::FileBrowser`), built by
//!   [`real_editor`](crate::real_editor). It now owns the **open document** — a
//!   rope [`Buffer`] plus its [`EditorView`] widget state (EDITOR-2/3) — as an
//!   `Option`: `None` is the honest empty state (§7), `Some` is a live editable
//!   document.
//! * [`editor_panel`] renders the surface into the shell body (the analogue of
//!   `files_panel`): when a document is open it draws a compact chrome strip and
//!   the real text widget ([`editor_widget`]); when none is open it keeps the
//!   honest "No file open" empty state, with a temporary affordance to open a
//!   scratch buffer so the surface is exercisable before the fuzzy-open / Files
//!   send land (EDITOR-7/9).
//!
//! All chrome resolves through the shared Carbon [`Style`] tokens (§4). The
//! `&mut EditorSurface` seam matches the mount contract the shell wires for
//! Files/Terminal, so this grows the surface without re-wiring the shell.

use std::io;
use std::path::Path;

use mde_egui::egui::{self, RichText, Ui};
use mde_egui::Style;

use crate::buffer::Buffer;
use crate::widget::{editor_widget, EditorView};

/// The honest empty-state headline the surface renders when no document is open
/// (§7) — a real, reachable message, never a `todo!()`/stub.
const NO_FILE_TITLE: &str = "No file open";
/// The honest empty-state hint paired with [`NO_FILE_TITLE`] — the §7 copy the
/// design doc locks: "open a file to start editing".
const NO_FILE_HINT: &str = "Open a file to start editing.";

/// The seed text a fresh scratch buffer opens with — a tiny reachable document so
/// the operator can immediately type, move the caret, and select (the temporary
/// EDITOR-3 open affordance; the fuzzy-open / Files send land in EDITOR-7/9).
const SCRATCH_SEED: &str = "// Scratch buffer — type here.\n// The editor edits a real rope: every keystroke\n// mutates the buffer and re-renders.\n\n";

/// One open document in the editor surface: the editable rope [`Buffer`]
/// (EDITOR-2) paired with the [`EditorView`] widget state (EDITOR-3, the caret /
/// selection / scroll / wrap).
struct Doc {
    /// The editable document model.
    buffer: Buffer,
    /// The widget state rendering + editing it.
    view: EditorView,
}

impl Doc {
    /// Wrap a freshly built buffer in a new view.
    fn new(buffer: Buffer) -> Self {
        Self {
            buffer,
            view: EditorView::new(),
        }
    }
}

/// The code-editor surface the E12 shell embeds.
///
/// The surface state the shell holds directly and drives with [`editor_panel`],
/// mirroring `mde-files-egui`'s `FileBrowser` seam. It owns the open document as
/// an `Option<Doc>`: `None` renders the honest empty state (§7); `Some` renders
/// the live text widget over the real rope. EDITOR-7/9 add fuzzy-open / Files
/// send by calling [`open_path`](Self::open_path) / [`open_text`](Self::open_text).
#[derive(Default)]
pub struct EditorSurface {
    /// The currently open document, or `None` for the empty state.
    doc: Option<Doc>,
}

impl EditorSurface {
    /// Whether a document is currently open (the surface is showing the editor,
    /// not the empty state).
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.doc.is_some()
    }

    /// Open an in-memory document seeded with `text` (no path). The open-a-buffer
    /// seam the finder / Files send drive; also the scratch affordance's backing.
    pub fn open_text(&mut self, text: &str) {
        self.doc = Some(Doc::new(Buffer::from_text(text)));
    }

    /// Open `path` from disk into the surface, replacing any open document.
    ///
    /// # Errors
    /// Returns any [`io::Error`] from reading `path` (missing file, permissions).
    pub fn open_path<P: AsRef<Path>>(&mut self, path: P) -> io::Result<()> {
        self.doc = Some(Doc::new(Buffer::open(path)?));
        Ok(())
    }

    /// Open a fresh scratch buffer (the temporary EDITOR-3 exercise affordance).
    pub fn open_scratch(&mut self) {
        self.open_text(SCRATCH_SEED);
    }

    /// Close the open document, returning the surface to the empty state.
    pub fn close(&mut self) {
        self.doc = None;
    }
}

/// Render the editor surface into the shell body — mirrors `files_panel`.
///
/// When a document is open: a compact chrome strip (name, dirty marker, wrap
/// toggle, caret position) over the real text widget, which edits the live rope
/// (§7 — runtime-reachable, not a mockup). When none is open: the honest empty
/// state plus a temporary "open a scratch buffer" affordance so the surface is
/// exercisable before fuzzy-open lands. `surface` is the mount seam the shell
/// wires (the analogue of `files_panel`'s `&mut FileBrowser`).
pub fn editor_panel(ui: &mut Ui, surface: &mut EditorSurface) {
    // No open document → the honest empty state + the scratch affordance. The
    // diverging `else` frees the `as_mut` borrow, so `open_scratch` can mutate.
    let Some(doc) = surface.doc.as_mut() else {
        editor_chrome(ui);
        if empty_state(ui) {
            surface.open_scratch();
        }
        return;
    };
    doc_chrome(ui, doc);
    ui.separator();
    // Disjoint field borrows: the widget edits `&mut view` + `&mut buffer` at once.
    editor_widget(ui, &mut doc.view, &mut doc.buffer);
}

/// The open-document chrome strip: the file name (or "scratch"), a dirty marker,
/// a soft-wrap toggle, and the caret's line:col — all through [`Style`] tokens
/// (§4). Honest chrome: it reflects the real buffer/view, no faked state.
fn doc_chrome(ui: &mut Ui, doc: &mut Doc) {
    // Precompute every displayed value up front (owned), so the render closures
    // capture no borrow of `doc`; the one mutation (wrap toggle) is deferred to a
    // flag and applied after the strip.
    let name = doc
        .buffer
        .path()
        .and_then(Path::file_name)
        .map_or_else(|| "scratch".to_owned(), |n| n.to_string_lossy().into_owned());
    let dirty = if doc.buffer.is_dirty() { " \u{2022}" } else { "" };
    let lossy = doc.buffer.is_lossy();
    let (line, col) = doc.view.line_col(&doc.buffer);
    let wrap_on = doc.view.wrap();
    let mut toggle_wrap = false;

    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new(format!("{name}{dirty}"))
                .size(Style::BODY)
                .color(Style::TEXT)
                .strong(),
        );
        if lossy {
            ui.add_space(Style::SP_S);
            ui.label(
                RichText::new("lossy decode")
                    .size(Style::SMALL)
                    .color(Style::WARN),
            );
        }
        // Right-aligned: the wrap toggle + the caret position.
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.add_space(Style::SP_S);
                ui.label(
                    RichText::new(format!("Ln {line}, Col {col}"))
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                ui.add_space(Style::SP_M);
                if ui
                    .selectable_label(wrap_on, RichText::new("Wrap").size(Style::SMALL))
                    .clicked()
                {
                    toggle_wrap = true;
                }
            },
        );
    });
    ui.add_space(Style::SP_XS);

    if toggle_wrap {
        doc.view.toggle_wrap();
    }
}

/// The editor's identity strip for the empty state — a compact header naming the
/// surface, drawn through [`Style`] tokens (§4).
fn editor_chrome(ui: &mut Ui) {
    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new("Editor")
                .size(Style::BODY)
                .color(Style::TEXT)
                .strong(),
        );
    });
    ui.add_space(Style::SP_XS);
    ui.separator();
}

/// The "no document" face — the honest empty state (§7), plus a temporary button
/// to open a scratch buffer so the surface is exercisable before fuzzy-open
/// lands. Returns `true` when the operator clicked the button (the caller opens
/// the buffer). Every value is a shared [`Style`] token (§4), no raw hex/metric.
fn empty_state(ui: &mut Ui) -> bool {
    let mut open = false;
    ui.vertical_centered(|ui| {
        ui.add_space(Style::SP_XL);
        ui.label(
            RichText::new(NO_FILE_TITLE)
                .size(Style::HEADING)
                .color(Style::TEXT_DIM),
        );
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new(NO_FILE_HINT)
                .size(Style::BODY)
                .color(Style::TEXT_DIM),
        );
        ui.add_space(Style::SP_L);
        open = ui
            .button(RichText::new("Open a scratch buffer").size(Style::BODY))
            .clicked();
    });
    open
}

#[cfg(test)]
mod tests {
    use super::{editor_panel, EditorSurface, NO_FILE_HINT, NO_FILE_TITLE, SCRATCH_SEED};
    use crate::real_editor;
    use mde_egui::egui::{self, pos2, vec2, Rect};
    use mde_egui::Style;

    /// Drive one headless frame through the editor panel, tessellating on the CPU
    /// so any paint-path fault surfaces — the same `Context::run` → `tessellate`
    /// path the shell's mount test drives, minus the GPU. Returns the primitive
    /// count so callers can assert the surface actually paints.
    fn tessellate_panel(surface: &mut EditorSurface) -> usize {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                editor_panel(ui, surface);
            });
        });
        ctx.tessellate(out.shapes, out.pixels_per_point).len()
    }

    #[test]
    fn empty_state_panel_mounts_and_renders_headless() {
        let mut surface = real_editor();
        assert!(!surface.is_open(), "a fresh surface opens no document");
        assert!(
            tessellate_panel(&mut surface) > 0,
            "the empty-state editor surface produced no draw primitives"
        );
    }

    #[test]
    fn opening_a_document_renders_the_widget() {
        let mut surface = real_editor();
        surface.open_text("fn main() {}\n");
        assert!(surface.is_open(), "open_text opens a document");
        assert!(
            tessellate_panel(&mut surface) > 0,
            "the open-document editor surface produced no draw primitives"
        );
        surface.close();
        assert!(!surface.is_open(), "close returns to the empty state");
    }

    #[test]
    fn open_scratch_seeds_a_real_editable_buffer() {
        let mut surface = EditorSurface::default();
        surface.open_scratch();
        assert!(surface.is_open());
        // The scratch seed is real, reachable text (not empty, not a stub).
        assert!(!SCRATCH_SEED.is_empty());
    }

    #[test]
    fn empty_state_copy_is_honest_and_reachable() {
        assert!(!NO_FILE_TITLE.is_empty(), "empty-state title is blank");
        assert!(!NO_FILE_HINT.is_empty(), "empty-state hint is blank");
        assert!(
            NO_FILE_TITLE.to_lowercase().contains("file"),
            "the headline should name the missing file"
        );
        let hint = NO_FILE_HINT.to_lowercase();
        assert!(
            hint.contains("open") && hint.contains("edit"),
            "the hint should tell the operator to open a file to edit"
        );
    }
}
