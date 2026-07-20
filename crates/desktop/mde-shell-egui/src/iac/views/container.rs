//! U16 — the **Service Container** delivery view: Podman / Quadlet service
//! containers (rootless by default). The roster is the U3 seam; the rich per-type
//! body lands with U16.

use mde_egui::egui;

use super::super::{DeliveryView, WorkloadsState};

/// The Service Container view's own state (U16 owns its fields).
#[derive(Debug, Default)]
pub(in crate::iac) struct State;

/// Render the Service Container view.
pub(super) fn view(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    super::super::roster(ui, state, DeliveryView::ServiceContainer);
}
