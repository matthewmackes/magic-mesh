//! MEDIA-2 acceptance surface for the **DRM overlay video plane** — the live half of
//! the seam in [`mde_egui::video_plane`].
//!
//! Run on a host with a free DRM master (a plain VT, no X/Wayland):
//!
//! ```text
//! cargo run -p mde-egui --example hello_video_plane --features drm
//! ```
//!
//! It brings the seat up, enumerates the card's planes, and prints the MEDIA-2
//! decision for a 1080p frame shown in a centred player pane: either the chosen
//! **overlay plane** (id + geometry + z-order below the egui shell) or the honest
//! **render-to-texture fallback** with its reason. On a headless host it prints the
//! `NoDrmMaster` fallback cleanly — the same contract as `hello_drm`. The live scanout
//! of a *decoded* frame is the mpv-gated leg (MEDIA-3/4/8).

use mde_egui::{probe_primary_video_plane, FallbackReason, PaneRect, VideoPath};

fn main() {
    // A 1080p video shown in a 1280x720 pane centred on a typical 1080p output.
    let video = (1920, 1080);
    let pane = PaneRect::new(320, 180, 1280, 720);

    match probe_primary_video_plane(video, pane) {
        Ok((set, path)) => {
            println!(
                "planes enumerated: {} (egui plane id {}, crtc index {})",
                set.planes.len(),
                set.egui_plane_id,
                set.crtc_index
            );
            for p in &set.planes {
                println!(
                    "  plane {:>3}  {:?}  crtcs=0b{:b}  zpos={:?}",
                    p.id, p.kind, p.possible_crtcs, p.zpos
                );
            }
            match path {
                VideoPath::Overlay(plan) => {
                    println!(
                        "→ OVERLAY plane {} (zpos {:?}); placement {:?}",
                        plan.plane_id, plan.zpos, plan.placement
                    );
                    println!("  egui punches a transparent hole at {:?}", plan.punch_hole);
                }
                VideoPath::Texture(reason) => {
                    println!("→ {reason}");
                }
            }
        }
        Err(e) => {
            // Headless / no free master → the honest render-to-texture fallback.
            debug_assert!(VideoPath::texture_no_drm().is_texture());
            println!("no DRM overlay seat here ({e}); {}", FallbackReason::NoDrm);
        }
    }
}
