//! U16 — the **Desktop VM** delivery view: full VM desktops delivered as native
//! VDI seats. The roster is the U3 seam; the rich per-type body (console attach,
//! per-seat metrics) lands with U16.

use mde_egui::egui;

use super::super::{DeliveryView, WorkloadsState};

/// The Desktop VM view's own state (U16 owns its fields).
#[derive(Debug, Default)]
pub(in crate::iac) struct State;

/// Render the Desktop VM view.
pub(super) fn view(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    super::super::roster(ui, state, DeliveryView::DesktopVm);
}
