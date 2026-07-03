//! `mde-editor-egui` — the MCNF **"Quasar"** native code-editor surface (EDITOR-1).
//!
//! A Zed-style, keyboard-driven Rust code editor adapted to the DRM-native egui
//! shell (design: `docs/design/editor.md`). This crate opens with EDITOR-1: the
//! MOUNTABLE SHELL only. It exposes an [`EditorSurface`] state struct + the
//! [`editor_panel`] seam that renders the editor chrome and an honest empty state
//! ("No file open — open a file to start editing", §7 — a real reachable state,
//! never a `todo!()` stub), mounted in the one Quasar shell (`mde-shell-egui`) as
//! `Surface::Editor` through the exact seam `mde-files-egui` gives the shell
//! (`files_panel` / `real_browser`): the shell owns the surface state and drives
//! it per-frame with the `*_panel` fn.
//!
//! The rope buffer (EDITOR-2), the custom egui text widget with multi-cursor
//! (EDITOR-3), tree-sitter highlighting, tabs + splittable panes, and the fuzzy
//! finder / command palette / project search land in the following units. They
//! grow [`EditorSurface`] and render into [`editor_panel`] without re-wiring the
//! shell — that is the point of landing the mount seam first.
//!
//! Layering (§6): the surface state + render seam live in [`panel`]; the only
//! in-workspace edge points inward to [`mde_egui`] (the harness + the shared
//! Carbon `Style`).

pub mod panel;

use mde_egui::{eframe, egui};

pub use panel::{editor_panel, EditorSurface};

/// Build the production [`EditorSurface`] the E12 shell owns and mounts with
/// [`editor_panel`] — the editor analogue of `mde_files_egui::real_browser()`.
///
/// EDITOR-1 is the mountable shell, so this is a bare surface holding no open
/// document (the rope buffer + tab/pane model land in EDITOR-2/3); the panel
/// renders the honest empty state until then (§7). Factored out so the shell
/// mounts the surface through one named constructor, exactly as it builds Files
/// via `real_browser()` and Terminal via `real_terminal()`.
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
