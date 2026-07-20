//! U14 — the **placement picker**: choose the mesh node (local or a remote peer
//! over Nebula) a new workload is placed on, with per-node capacity bars driven
//! by [`mackes_mesh_types::cloud::NodeCapacity`]. A seam stub for now; the U14
//! worker fills [`placement_picker`] + [`State`] and returns the chosen node.

use mde_egui::egui;

use super::WorkloadsState;

/// The placement picker's own state (U14 owns its fields).
#[derive(Debug, Default)]
pub(super) struct State;

/// Render the placement picker; return the node the operator chose this frame, if
/// any (the provision panel stores it as the placement target). The stub returns
/// `None` until U14 lands the picker.
pub(super) fn placement_picker(ui: &mut egui::Ui, state: &mut WorkloadsState) -> Option<String> {
    super::workloads_pending(ui, "U14", "placement picker \u{2014} node + capacity bars");
    super::mirror_summary(ui, state);
    None
}
