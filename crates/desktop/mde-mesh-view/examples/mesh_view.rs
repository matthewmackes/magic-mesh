//! Runnable acceptance surface for [`mde_mesh_view::MeshView`].
//!
//! Stands up a Wayland client through the shared harness and paints a small
//! **sample** mesh — 3 lighthouses (one elected leader), a few peers with varied
//! health, and several active links — so the procedural canvas is visually
//! runnable on a Wayland session:
//!
//! ```text
//! cargo run -p mde-mesh-view --example mesh_view
//! ```
//!
//! The sample lives **here**, in the example. The [`MeshView`] widget itself
//! draws only the [`MeshState`] it is handed — no embedded demo data.

use mde_egui::egui;
use mde_egui::{eframe, run_client, Style};
use mde_mesh_view::{Health, MeshLink, MeshNode, MeshState, MeshView, Role};

struct MeshViewDemo {
    state: MeshState,
    reduce_motion: bool,
}

impl eframe::App for MeshViewDemo {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(Style::SP_S);
            ui.horizontal(|ui| {
                ui.heading(
                    egui::RichText::new("MCNF · Mesh View")
                        .color(Style::TEXT)
                        .size(Style::HEADING),
                );
                ui.add_space(Style::SP_M);
                ui.checkbox(&mut self.reduce_motion, "reduce motion");
            });
            ui.colored_label(
                Style::TEXT_DIM,
                "live mesh state — leader pulse · health colours · travelling link activity",
            );
            ui.add_space(Style::SP_S);

            MeshView::new(&self.state)
                .reduce_motion(self.reduce_motion)
                .show(ui);
        });
    }
}

/// The SAMPLE mesh — example-only data, never in the widget render path: three
/// lighthouses (nyc3 is the elected leader, sfo3 degraded) and four workstation
/// peers (one Down), and a spread of active links. All auto-placed, so the
/// lighthouses cluster on the inner ring and the peers ring around them.
fn sample_state() -> MeshState {
    let nodes = vec![
        MeshNode::new("lh-nyc3", "lighthouse-nyc3", Role::Lighthouse, Health::Ok).leader(),
        MeshNode::new("lh-fra1", "lighthouse-fra1", Role::Lighthouse, Health::Ok),
        MeshNode::new("lh-sfo3", "lighthouse-sfo3", Role::Lighthouse, Health::Warn),
        MeshNode::new("eagle", "eagle", Role::Workstation, Health::Ok),
        MeshNode::new("media", "media-server", Role::Workstation, Health::Warn),
        MeshNode::new("ws-01", "workstation-01", Role::Workstation, Health::Ok),
        MeshNode::new("ws-02", "workstation-02", Role::Workstation, Health::Down),
    ];
    let links = vec![
        MeshLink::new("lh-nyc3", "lh-fra1", 0.85),
        MeshLink::new("lh-fra1", "lh-sfo3", 0.5),
        MeshLink::new("lh-nyc3", "lh-sfo3", 0.3),
        MeshLink::new("lh-nyc3", "eagle", 0.6),
        MeshLink::new("lh-fra1", "ws-01", 0.9),
        MeshLink::new("lh-nyc3", "media", 0.4),
        MeshLink::new("eagle", "ws-01", 0.2),
        MeshLink::new("lh-sfo3", "ws-02", 0.0), // idle link to the down node
    ];
    MeshState { nodes, links }
}

fn main() -> eframe::Result<()> {
    run_client("org.magicmesh.MeshView", "MCNF · Mesh View", |_cc| {
        MeshViewDemo {
            state: sample_state(),
            reduce_motion: false,
        }
    })
}
