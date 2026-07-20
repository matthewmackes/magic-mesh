//! U15 — the **provision form**: author a [`mackes_mesh_types::cloud::WorkloadSpec`]
//! (delivery type · sizing · image · network isolation) with the raw-HCL escape
//! hatch, then hand it to `set-desired` / `plan` / armed `provision`. A seam stub
//! for now; the U15 worker fills [`provision_form`] + [`State`].

use mde_egui::egui;

use super::WorkloadsState;

/// The provision form's own state (U15 owns the draft spec fields).
#[derive(Debug, Default)]
pub(super) struct State;

/// Render the provision form for the placement node the picker selected.
pub(super) fn provision_form(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    super::workloads_pending(ui, "U15", "provision form + raw-HCL escape hatch");
    match state.selected_node() {
        Some(node) => mde_egui::muted_note(ui, format!("Placement target: {node}.")),
        None => mde_egui::muted_note(
            ui,
            "No placement node selected yet \u{2014} pick one in the placement picker above.",
        ),
    };
}
