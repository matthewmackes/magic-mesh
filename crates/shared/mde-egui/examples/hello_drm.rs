//! E12-2 acceptance surface for the **bare-seat** (DRM/KMS) backend — no compositor.
//!
//! Run on a host with a free DRM master (a plain VT, no X/Wayland running):
//!
//! ```text
//! cargo run -p mde-egui --example hello_drm --features drm
//! ```
//!
//! On a headless host it returns `NoDrmMaster` cleanly (the shell's fallback
//! contract). The live render is the hardware-gated `/preview`.

use mde_egui::{egui, run_drm, Style};

fn main() {
    let result = run_drm("org.magicmesh.HelloDrm", |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(Style::SP_XL);
            ui.heading(
                egui::RichText::new("MCNF · egui owns the seat")
                    .color(Style::TEXT)
                    .size(Style::HEADING),
            );
            ui.add_space(Style::SP_S);
            ui.colored_label(
                Style::TEXT_DIM,
                "E12-2 — rendering on a bare DRM/KMS seat, no Wayland compositor.",
            );
        });
    });
    if let Err(e) = result {
        eprintln!("hello_drm: {e}");
        std::process::exit(1);
    }
}
