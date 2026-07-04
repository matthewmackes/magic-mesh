//! `mde-editor-egui` — the MCNF **"Quasar"** native code-editor surface.
//!
//! A Zed-style, keyboard-driven Rust code editor adapted to the DRM-native egui
//! shell (design: `docs/design/editor.md`). It exposes an [`EditorSurface`] state
//! struct + the [`editor_panel`] seam, mounted in the one Quasar shell
//! (`mde-shell-egui`) as `Surface::Editor` through the exact seam `mde-files-egui`
//! gives the shell (`files_panel` / `real_browser`): the shell owns the surface
//! state and drives it per-frame with the `*_panel` fn.
//!
//! What has landed:
//!
//! * EDITOR-1 — the mountable surface + the honest "No file open" empty state (§7).
//! * EDITOR-2 — the rope [`Buffer`]: the real editable document model.
//! * EDITOR-3 — the custom text [`widget`]: viewport-culled render + gutter,
//!   caret + selection, mouse hit-testing (click / drag / word / line), keyboard
//!   editing + motion, undo/redo, scroll, and a soft-wrap toggle. The widget
//!   edits the live rope (§7 — runtime-reachable, not a mockup).
//! * EDITOR-4 — multi-cursor + column selection: the single caret generalizes to
//!   a `Vec` of carets (add above/below, add-at-next-match, Alt-click toggle,
//!   Alt-drag box select); every edit fans out across all of them, overlapping
//!   carets merge, and `Esc` collapses to the single primary caret.
//! * EDITOR-7 — the **fuzzy file finder** ([`finder`], `Cmd`/`Ctrl-P`) + the
//!   **command palette** ([`palette`], `Cmd`/`Ctrl-Shift-P`): two overlays whose
//!   trigger chords are intercepted at the panel level (before the widget reads
//!   this frame's events) and which drive the existing surface seams over a small
//!   self-contained [`fuzzy`] matcher.
//! * EDITOR-5 — **tree-sitter syntax highlighting** ([`highlight`]): per-open-
//!   buffer grammars picked by file extension (rust / python / js / ts / json /
//!   toml / markdown / bash), re-parsed **incrementally** on edit (the buffer's
//!   edit deltas splice the old tree — no full-file reparse per keystroke) and
//!   painted through the shared Carbon code-theme tokens ([`mde_egui::code`],
//!   §4). Unknown extensions honestly render plain (§7).
//! * EDTB-1 — the **Word-97 menu bar + Standard toolbar**
//!   ([`menu_bar`] / [`toolbar`]; design:
//!   `docs/design/editor-toolbar.md`): File/Edit/View/Tools/Help menus and the
//!   New·Open·Save | Cut·Copy·Paste | Undo·Redo | Zoom strip, every control
//!   routed through the same palette / `EditorView` seams (§6) — no dead
//!   entries (lock #4). Zoom is a real per-view font scale on [`EditorView`].
//! * EDTB-2/3 — the **markdown formatting engine + Formatting toolbar**
//!   ([`md_actions`] / [`format_bar`]): the Style dropdown, B/I/U/S, list, and
//!   indent controls (Word's second toolbar row) plus the **Insert** (Table
//!   grid-picker) and **Format** menus, each driving `md_actions` on the live
//!   buffer as one operator undo step. Only the standalone Table menu (cell ops)
//!   and Print/Spell/Find await later phases.
//! * EDITOR-LSP-1/2 — the **LSP client** ([`lsp`]) wired into the surface: the
//!   panel owns a per-document [`LspClient`], driving `didOpen`/`didChange`/
//!   `didClose` off the real buffer's open/edit/close lifecycle, and the widget
//!   paints the published diagnostics (`lsp_ui`) as gutter severity markers +
//!   inline underlines with a hover message. An absent server binary surfaces
//!   the honest [`LspState::Unavailable`] status, never a faked session (§7).
//!
//! Tabs + splittable panes land in the following units; they grow
//! [`EditorSurface`] / [`EditorView`] and render into [`editor_panel`] without
//! re-wiring the shell.
//!
//! Layering (§6): the surface state + render seam live in [`panel`], the widget in
//! [`widget`], the document model in [`buffer`]; the only in-workspace edge points
//! inward to [`mde_egui`] (the harness + the shared Carbon `Style`).

pub mod buffer;
pub mod crdt;
mod finder;
mod format_bar;
mod fuzzy;
pub mod highlight;
pub mod lsp;
pub mod lsp_ui;
pub mod md_actions;
mod menu_bar;
mod palette;
pub mod panel;
pub mod project_tree;
mod toolbar;
pub mod widget;

use mde_egui::{eframe, egui};

pub use buffer::Buffer;
pub use crdt::{CollabDoc, CrdtError, EditSink, TextEdit};
pub use highlight::{Highlighter, Language};
pub use lsp::{Diagnostic, LspClient, LspState, Severity};
pub use lsp_ui::DiagnosticsOverlay;
pub use panel::{editor_panel, EditorSurface};
pub use project_tree::ProjectTree;
pub use widget::{editor_widget, EditorView};

/// Build the production [`EditorSurface`] the E12 shell owns and mounts with
/// [`editor_panel`] — the editor analogue of `mde_files_egui::real_browser()`.
///
/// The surface opens to the honest empty state with no document (§7); the
/// operator opens one through the scratch affordance or, once they land, the
/// fuzzy-open / Files send (EDITOR-7/9 call [`EditorSurface::open_path`] /
/// [`EditorSurface::open_text`]). Factored out so the shell mounts the surface
/// through one named constructor, exactly as it builds Files via `real_browser()`
/// and Terminal via `real_terminal()`.
#[must_use]
pub fn real_editor() -> EditorSurface {
    EditorSurface::default()
}

/// The eframe application: the [`EditorSurface`] rendered each frame.
///
/// The standalone binary's app wrapper; it owns the window `CentralPanel` and
/// renders the surface through the shared [`editor_panel`] fn — the exact call
/// the E12 shell makes to mount the editor as an embedded panel, so standalone
/// and embedded are identical (E12 §5 EMBED).
pub struct EditorApp {
    /// The one editor surface this window renders.
    editor: EditorSurface,
}

impl EditorApp {
    /// Build the surface over a fresh [`EditorSurface`] (`real_editor()`).
    #[must_use]
    pub fn new() -> Self {
        Self {
            editor: real_editor(),
        }
    }
}

impl Default for EditorApp {
    fn default() -> Self {
        Self::new()
    }
}

impl eframe::App for EditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Thin frame wrapper (E12 §5 EMBED): the binary only owns the window
        // `CentralPanel`; the surface itself renders through the shared
        // [`editor_panel`] fn — the exact same call the E12 shell makes to mount
        // the editor as an embedded panel, so standalone and embedded are
        // identical.
        egui::CentralPanel::default().show(ctx, |ui| {
            editor_panel(ui, &mut self.editor);
        });
    }
}
