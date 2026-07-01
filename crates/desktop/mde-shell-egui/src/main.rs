//! `mde-shell-egui` — the single MCNF E12 "Quasar" egui shell (E12-3).
//!
//! One eframe app on the `mde-egui` harness. A thin persistent **chrome bar**
//! (peers · sessions · status + an Expand toggle) sits over a central view that
//! is either:
//!
//! * the **session EmptyState** (collapsed) — a real session is a fullscreen VM
//!   texture from `mde-vdi`, a later unit; or
//! * the **Workbench** five-plane nav (expanded) — This Node / Controller /
//!   Network / Fleet / Provisioning.
//!
//! The expand/collapse transition eases through the shared `Motion` table and the
//! whole surface renders through the shared `Style` (governance §4/§5/§7). This is
//! the skeleton the panels (Workbench/Files/Music/Voice) and the VM session-view
//! plug into.

mod chrome;
mod datacenter;
mod session;
mod workbench;

use mde_egui::{eframe, egui, run_client, Motion, Style};

use workbench::Plane;

/// The shell's whole UI state: whether the chrome bar is expanded into the
/// Workbench, and which plane the Workbench has selected.
#[derive(Default)]
struct Shell {
    /// `true` while the chrome bar is expanded into the full Workbench.
    expanded: bool,
    /// The Workbench plane shown when expanded.
    plane: Plane,
    /// Fleet plane — live per-node KVM host health + VM roster, and the
    /// host-targeted VM lifecycle controls (MV-6). Subscribes to the Bus.
    datacenter: datacenter::DatacenterState,
}

impl Shell {
    /// Flip between the collapsed session view and the expanded Workbench.
    fn toggle_expand(&mut self) {
        self.expanded = !self.expanded;
    }
}

impl eframe::App for Shell {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // The Fleet plane subscribes to the live KVM/VM Bus topics. Poll on the
        // shared cadence while expanded (the read is a cheap local scan) so a host
        // health flip or a new VM surfaces without operator input; the poll
        // self-gates and keeps the repaint heartbeat alive.
        if self.expanded {
            self.datacenter.poll(ctx);
        }

        // The thin persistent chrome bar (48px = SP_XL + SP_M).
        egui::TopBottomPanel::top("mcnf-chrome")
            .exact_height(Style::SP_XL + Style::SP_M)
            .show(ctx, |ui| {
                if chrome::show(ui, self.expanded) {
                    self.toggle_expand();
                }
            });

        // Expand transition: 0.0 = collapsed (session), 1.0 = expanded (Workbench).
        let t = Motion::animate(ctx, "shell-expand", self.expanded, Motion::BASE);

        egui::CentralPanel::default().show(ctx, |ui| {
            // Cross-fade the two central views through the midpoint so they never
            // fight for layout: the session fades out over the first half, the
            // Workbench fades in over the second.
            if t < 0.5 {
                ui.set_opacity((1.0 - t * 2.0).clamp(0.0, 1.0));
                session::show(ui);
            } else {
                let a = (t * 2.0 - 1.0).clamp(0.0, 1.0);
                ui.set_opacity(a);
                // A small rise as the Workbench settles in.
                ui.add_space((1.0 - a) * Style::SP_S);
                workbench::show(ui, &mut self.plane, &mut self.datacenter);
            }
        });

        // Keep painting while the transition is in flight.
        if t > 0.001 && t < 0.999 {
            ctx.request_repaint();
        }
    }
}

fn main() -> eframe::Result<()> {
    run_client("org.magicmesh.Shell", "MCNF", |_cc| Shell::default())
}

#[cfg(test)]
mod tests {
    use super::{Plane, Shell};

    #[test]
    fn shell_starts_collapsed_on_this_node() {
        let s = Shell::default();
        assert!(
            !s.expanded,
            "the shell opens to the session view, not the Workbench"
        );
        assert_eq!(s.plane, Plane::ThisNode);
    }

    #[test]
    fn toggle_expand_flips_the_workbench() {
        let mut s = Shell::default();
        assert!(!s.expanded);
        s.toggle_expand();
        assert!(s.expanded);
        s.toggle_expand();
        assert!(!s.expanded);
    }
}
