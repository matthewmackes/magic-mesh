//! U19 — the **Images** lens: the golden per-delivery-type image roster
//! (bootc / osbuild builds → [`mackes_mesh_types::cloud::ImageRow`]) with the
//! SHA256 Syncthing airgap lane. A seam stub for now; the U19 worker fills
//! [`images_panel`].

use mde_egui::egui;

use super::WorkloadsState;

/// The Images lens's own state (U19 owns its fields).
#[derive(Debug, Default)]
pub(super) struct State;

/// Render the Images lens.
pub(super) fn images_panel(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    super::workloads_pending(ui, "U19", "images \u{2014} golden per-type roster");
    super::mirror_summary(ui, state);
}
