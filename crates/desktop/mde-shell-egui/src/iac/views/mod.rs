//! U16 — the **five delivery-type views**: the cockpit's primary metaphor. Each
//! view renders the roster of its delivery type (from the folded `state/cloud`
//! mirror) and, once U16 lands, the rich per-type body (live metrics, drift,
//! console attach). Each view is a disjoint file so the five U16 workers never
//! collide; this module declares them + the [`dispatch`] the cockpit calls.

use mde_egui::egui;

use super::{DeliveryView, WorkloadsState};

pub(super) mod android_vm;
pub(super) mod app_vm;
pub(super) mod container;
pub(super) mod desktop_vm;
pub(super) mod service_vm;

/// Dispatch to the selected delivery view's render fn (the cockpit's Roster lens).
pub(super) fn dispatch(ui: &mut egui::Ui, state: &mut WorkloadsState, view: DeliveryView) {
    match view {
        DeliveryView::DesktopVm => desktop_vm::view(ui, state),
        DeliveryView::ServiceVm => service_vm::view(ui, state),
        DeliveryView::AppVm => app_vm::view(ui, state),
        DeliveryView::AndroidVm => android_vm::view(ui, state),
        DeliveryView::ServiceContainer => container::view(ui, state),
    }
}
