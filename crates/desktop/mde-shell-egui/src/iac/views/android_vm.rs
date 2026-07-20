//! U16 — the **Android VM** delivery view: VMs providing Android via the
//! two-layer Cuttlefish (`cvd`) backend. The roster is the U3 seam; the rich
//! per-type body (VNC/WebRTC console) lands with U16.

use mde_egui::egui;

use super::super::{DeliveryView, WorkloadsState};

/// The Android VM view's own state (U16 owns its fields).
#[derive(Debug, Default)]
pub(in crate::iac) struct State;

/// Render the Android VM view.
pub(super) fn view(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    super::super::roster(ui, state, DeliveryView::AndroidVm);
}
