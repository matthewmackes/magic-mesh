//! MCNF first-run deployment-role chooser (E12-8 / PKG-5).
//!
//! An egui window shown once, at first boot, to pin this box's deployment role —
//! **Lighthouse · Workstation** (governance §5). Picking a role runs
//! `mackesd role-pin <role>` (the upgrade-only ENT-2 path) and exits. If a role is
//! already pinned it exits immediately, so an `/etc/xdg/autostart` entry can launch
//! it every login as a no-op after the first run.
//!
//! Rewritten from the libcosmic GUI to the shared `mde-egui` harness (E12): one
//! `run_client` call, all look from the shared `Style`.

use mde_egui::{eframe, egui, run_client, Style};
use mde_role::Role;

/// One role's display copy: `(name, blurb)`.
fn role_blurb(role: Role) -> (&'static str, &'static str) {
    match role {
        Role::Lighthouse => (
            "Lighthouse",
            "Always-on relay + control plane — Nebula overlay, the mackesd \
             control plane, the media server, and the CA/signer. No desktop. \
             VPS-friendly. (Rank 0)",
        ),
        Role::Workstation => (
            "Workstation",
            "The full Quasar stack — the egui-DRM shell + VDI + local \
             KVM/cloud-hypervisor + Podman. A headless machine is just a \
             Workstation without a local display. (Rank 1)",
        ),
    }
}

/// Pin `slug` via `mackesd role-pin` (upgrade-only, fail-closed). `Ok(())` on a
/// successful pin; `Err(msg)` otherwise (mackesd missing / refused the downgrade).
fn pin_role(slug: &str) -> Result<(), String> {
    match std::process::Command::new("mackesd")
        .args(["role-pin", slug])
        .status()
    {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("mackesd role-pin exited with status {s}")),
        Err(e) => Err(format!("could not run mackesd role-pin: {e}")),
    }
}

/// The chooser surface. The only mutable state is the inline status line, which
/// only ever carries a role-pin *failure* (a successful pin exits the process).
struct RoleChooser {
    status: String,
}

impl eframe::App for RoleChooser {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(Style::SP_L);
            ui.heading(
                egui::RichText::new("MCNF — choose this machine's role")
                    .color(Style::TEXT)
                    .size(Style::HEADING),
            );
            ui.add_space(Style::SP_S);
            ui.colored_label(
                Style::TEXT_DIM,
                "One deployment role is pinned per machine at install. You can \
                 upgrade later (Lighthouse → Workstation), never downgrade.",
            );
            ui.add_space(Style::SP_S);

            // The single-source project disclaimer / mission.
            egui::ScrollArea::vertical()
                .max_height(Style::SP_XL * 4.0)
                .show(ui, |ui| {
                    ui.colored_label(Style::TEXT_DIM, mde_disclaimer::TEXT);
                });
            ui.add_space(Style::SP_M);

            for role in Role::all() {
                let (name, blurb) = role_blurb(role);
                let button = egui::Button::new(
                    egui::RichText::new(format!("{name}\n{blurb}")).color(Style::TEXT),
                )
                .fill(Style::SURFACE);
                if ui.add_sized([ui.available_width(), 0.0], button).clicked() {
                    match pin_role(role.as_str()) {
                        Ok(()) => std::process::exit(0),
                        Err(e) => self.status = e,
                    }
                }
                ui.add_space(Style::SP_S);
            }

            if !self.status.is_empty() {
                ui.add_space(Style::SP_S);
                ui.colored_label(Style::DANGER, format!("⚠  {}", self.status));
            }
        });
    }
}

fn main() -> eframe::Result<()> {
    // First-run gate: if a role is already pinned, this is a no-op (so a first-boot
    // autostart that fires every login does nothing after run one).
    if mde_role::load().is_ok() {
        return Ok(());
    }
    run_client("org.magicmesh.RoleChooser", "MCNF — Role Chooser", |_cc| {
        RoleChooser {
            status: String::new(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::role_blurb;
    use mde_role::Role;

    #[test]
    fn every_role_has_display_copy() {
        for r in Role::all() {
            let (name, blurb) = role_blurb(r);
            assert!(!name.is_empty(), "role {r:?} has no name");
            assert!(blurb.len() > 20, "role {r:?} blurb too short");
        }
    }

    #[test]
    fn the_two_roles_are_lighthouse_and_workstation() {
        // The 2-role model: only Lighthouse and Workstation, no middle role.
        assert_eq!(Role::all().len(), 2);
        assert_eq!(role_blurb(Role::Lighthouse).0, "Lighthouse");
        assert_eq!(role_blurb(Role::Workstation).0, "Workstation");
    }
}
