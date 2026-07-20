//! U18 — the **Status** lens: day-2 per-node backend health, the live workload
//! metrics (CPU / mem / disk from [`mackes_mesh_types::cloud::WorkloadRow`]),
//! desired-vs-actual drift, and the session audit trail. A seam stub for now; the
//! U18 worker fills [`status_panel`].

use mde_egui::egui;

use super::WorkloadsState;

/// The Status lens's own state (U18 owns its fields).
#[derive(Debug, Default)]
pub(super) struct State;

/// Render the Status lens — an honest stub for the day-2 health / metrics / drift
/// body (U18), plus the preserved session audit trail (its permanent home).
pub(super) fn status_panel(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    super::workloads_pending(ui, "U18", "status \u{2014} health, metrics + drift");
    super::mirror_summary(ui, state);
    super::render_audit(ui, &state.audit);
}
