//! U19 — the **Containers** lens: Podman / Quadlet service containers (the
//! `container-deploy` verb renders a Quadlet `.container` unit installed as a
//! systemd service, rootless by default). A seam stub for now; the U19 worker
//! fills [`containers_panel`].

use mde_egui::egui;

use super::WorkloadsState;

/// The Containers lens's own state (U19 owns its fields).
#[derive(Debug, Default)]
pub(super) struct State;

/// Render the Containers lens.
pub(super) fn containers_panel(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    super::workloads_pending(ui, "U19", "containers \u{2014} Podman / Quadlet");
    super::mirror_summary(ui, state);
}
