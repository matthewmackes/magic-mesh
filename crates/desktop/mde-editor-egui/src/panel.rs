//! The **lib panel seam** (EDITOR-1): the code-editor surface the one Quasar
//! shell (`mde-shell-egui`) embeds as `Surface::Editor`.
//!
//! Under E12 "Quasar" the mesh surfaces are **panels in the one shell**, not
//! separate clients (§5 EMBED — there is no compositor). This module exposes the
//! editor surface through the exact seam `mde-files-egui` gives the shell for the
//! file manager:
//!
//! * [`EditorSurface`] is the render-agnostic surface state the shell holds
//!   directly (the analogue of `mde_files_egui::FileBrowser`), built by
//!   [`real_editor`](crate::real_editor).
//! * [`editor_panel`] renders the surface into the shell body (the analogue of
//!   `files_panel`): the editor chrome + the honest empty state, all through the
//!   shared Carbon [`Style`] tokens (§4).
//!
//! EDITOR-1 is the mountable shell only: the surface owns no open document yet
//! (the rope buffer + tab/pane model land in EDITOR-2/3), so the panel always
//! renders the honest empty state (§7). The `&mut EditorSurface` seam matches the
//! mount contract the shell wires for Files/Terminal, so those units fill it
//! without re-wiring the shell.

use mde_egui::egui::{RichText, Ui};
use mde_egui::Style;

/// The honest empty-state headline the scaffold renders when no document is open
/// (§7) — a real, reachable message, never a `todo!()`/stub.
const NO_FILE_TITLE: &str = "No file open";
/// The honest empty-state hint paired with [`NO_FILE_TITLE`] — the §7 copy the
/// design doc locks: "open a file to start editing".
const NO_FILE_HINT: &str = "Open a file to start editing.";

/// The code-editor surface the E12 shell embeds (EDITOR-1).
///
/// The render-agnostic surface state the shell holds directly and drives with
/// [`editor_panel`], mirroring `mde-files-egui`'s `FileBrowser` seam. EDITOR-1 is
/// the mountable SHELL only: it owns no open document yet — the rope buffer
/// (EDITOR-2) and the tab/pane model (EDITOR-3) add the open-document state to
/// this struct — so the panel always renders the honest empty state (§7). Kept as
/// a real state struct (not a free function) so those units grow it without
/// churning the shell mount seam.
#[derive(Default)]
pub struct EditorSurface {}

/// Render the editor surface into the shell body — mirrors `files_panel`.
///
/// Paints the editor chrome + the honest empty state (§7): EDITOR-1 is the
/// mountable shell, so no document is open and the empty state is what the
/// operator sees. `surface` is the mount seam the shell wires (the analogue of
/// `files_panel`'s `&mut FileBrowser`); EDITOR-2/3 fill it with the rope buffer +
/// tab/pane model and render it here, so wiring the `&mut` seam now keeps the
/// shell mount stable across those units. The scaffold reads nothing from it yet.
pub fn editor_panel(ui: &mut Ui, _surface: &mut EditorSurface) {
    editor_chrome(ui);
    empty_state(ui);
}

/// The editor's identity strip — a compact header naming the surface, drawn
/// through [`Style`] tokens (§4). Honest chrome: it shows no faked tabs or file
/// state (there is no open document yet), only the surface's own title.
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

/// The "no document" face — the honest empty state (§7). A real, reachable
/// message, never a `todo!()`/stub: EDITOR-2/3 replace it with the rope buffer +
/// text widget once a file can be opened. Every value is a shared [`Style`] token
/// (§4), no raw hex or metric.
fn empty_state(ui: &mut Ui) {
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
    });
}

#[cfg(test)]
mod tests {
    use super::{editor_panel, NO_FILE_HINT, NO_FILE_TITLE};
    use crate::real_editor;
    use mde_egui::egui::{self, pos2, vec2, Rect};
    use mde_egui::Style;

    /// The panel seam mounts headlessly: build the real surface, render one frame
    /// through the editor panel, and tessellate it on the CPU so any paint-path
    /// fault surfaces as a failure — the same `Context::run` → `tessellate` path
    /// the shell's mount test drives, minus the GPU. Proves the empty-state chrome
    /// actually paints (it is runtime-reachable, not a `todo!()`).
    #[test]
    fn editor_panel_mounts_and_renders_headless() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut surface = real_editor();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                editor_panel(ui, &mut surface);
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the mounted editor surface produced no draw primitives"
        );
    }

    /// The empty-state copy is real, honest §7 copy — not an empty string and not
    /// a `todo!()` placeholder. It tells the operator there is no open document
    /// and how to start, matching the design doc's locked message.
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
