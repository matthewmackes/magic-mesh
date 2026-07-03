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
//!
//! Tree-sitter highlighting + multi-cursor (EDITOR-4/5), tabs + splittable panes,
//! and the fuzzy finder / command palette land in the following units; they grow
//! [`EditorSurface`] / [`EditorView`] and render into [`editor_panel`] without
//! re-wiring the shell.
//!
//! Layering (§6): the surface state + render seam live in [`panel`], the widget in
//! [`widget`], the document model in [`buffer`]; the only in-workspace edge points
//! inward to [`mde_egui`] (the harness + the shared Carbon `Style`).

pub mod buffer;
pub mod panel;
pub mod widget;

use mde_egui::{eframe, egui};

pub use buffer::Buffer;
pub use panel::{editor_panel, EditorSurface};
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
