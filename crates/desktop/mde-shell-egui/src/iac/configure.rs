//! U17 — the **Configure** lens: run Ansible (pick a playbook + target group and
//! converge) alongside the live resolved mesh inventory (the `inventory` verb →
//! [`mackes_mesh_types::cloud::InventoryHost`]). A seam stub for now; the U17
//! worker fills [`configure_panel`].
//!
//! The playbook + group inputs live here (not on [`WorkloadsState`]) so the U17
//! worker owns them; the preserved arming/emit path reads them via
//! `WorkloadsState::configure_body`.

use mde_egui::egui;

use super::WorkloadsState;

/// The Configure lens's own state — the Ansible entrypoint the check/apply seams
/// converge.
#[derive(Debug)]
pub(super) struct State {
    /// The playbook selection (the Ansible entrypoint).
    pub(super) playbook: String,
    /// The target group (the mesh inventory group to converge).
    pub(super) group: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            playbook: "site.yml".to_string(),
            group: "cloud_vm".to_string(),
        }
    }
}

/// Render the Configure lens.
pub(super) fn configure_panel(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    super::workloads_pending(ui, "U17", "configure + live inventory");
    mde_egui::muted_note(
        ui,
        format!(
            "Ansible: playbook {} \u{00B7} group {}.",
            state.configure.playbook, state.configure.group
        ),
    );
}
