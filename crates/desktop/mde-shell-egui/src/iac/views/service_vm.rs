//! U16 — the **Service VM** delivery view: headless VMs running a service exposed
//! on the mesh. The roster is the U3 seam; the rich per-type body lands with U16.

use mde_egui::egui;

use super::super::{DeliveryView, WorkloadsState};

/// The Service VM view's own state (U16 owns its fields).
#[derive(Debug, Default)]
pub(in crate::iac) struct State;

/// Render the Service VM view.
pub(super) fn view(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    super::super::roster(ui, state, DeliveryView::ServiceVm);
}
