//! E12-1 acceptance surface — the trivial client that proves the harness.
//!
//! It launches as a Wayland client through [`mde_egui::run_client`], renders a
//! window themed entirely by the shared [`Style`] (no raw colours/spacing here —
//! `Style` is the sole source), and animates a reveal driven by a
//! [`Motion`]-timed `animate_bool`. On a Wayland session:
//!
//! ```text
//! cargo run -p mde-egui --example hello
//! ```

use eframe::egui;
use mde_egui::{run_client, Motion, Style};

struct Hello {
    expanded: bool,
}

impl eframe::App for Hello {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(Style::SP_L);
            ui.heading(
                egui::RichText::new("MCNF · egui harness")
                    .color(Style::TEXT)
                    .size(Style::HEADING),
            );
            ui.add_space(Style::SP_S);
            ui.colored_label(
                Style::TEXT_DIM,
                "E12 · Construct — one Style, one Motion table, one toolkit.",
            );
            ui.add_space(Style::SP_M);

            if ui
                .button(if self.expanded { "Collapse" } else { "Expand" })
                .clicked()
            {
                self.expanded = !self.expanded;
            }

            // The reveal is a Motion::BASE-timed animate_bool (lock 10): `t`
            // eases 0→1 as `expanded` flips, and we drive both a progress fill
            // and a fade off the same value.
            let t = Motion::animate(ctx, "hello-reveal", self.expanded, Motion::BASE);
            ui.add_space(Style::SP_S);
            ui.add(
                egui::ProgressBar::new(t)
                    .fill(Style::ACCENT)
                    .desired_width(Style::SP_XL * 8.0),
            );
            if t > 0.001 {
                ui.add_space(Style::SP_S * t);
                ui.colored_label(
                    Style::TEXT.gamma_multiply(t),
                    "Revealed content — faded in by the shared Motion table.",
                );
            }

            // Keep repainting while the animation is in flight.
            if t > 0.001 && t < 0.999 {
                ctx.request_repaint();
            }
        });
    }
}

fn main() -> eframe::Result<()> {
    run_client("org.magicmesh.HelloEgui", "MCNF egui harness", |_cc| {
        Hello { expanded: false }
    })
}
