//! U16 — the **App VM** delivery view: VMs whose individual apps are forwarded
//! into the MDE desktop (VDI app-mode / `session_broker`). The roster is the U3
//! seam; the rich per-type body lands with U16.

use mde_egui::egui;

use super::super::{DeliveryView, WorkloadsState};

/// The App VM view's own state (U16 owns its fields).
#[derive(Debug, Default)]
pub(in crate::iac) struct State;

/// Render the App VM view.
pub(super) fn view(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    super::super::roster(ui, state, DeliveryView::AppVm);
}
