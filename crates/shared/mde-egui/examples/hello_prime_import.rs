//! QC-23 Tier 1 acceptance surface for the **PRIME-import liveness check**
//! (`docs/design/qc23-virtio-gpu-zerocopy-rescope.md` §5) — the shell-side half of
//! a real dmabuf importer (Option B, §3.5), proven independently of the still-
//! blocked QEMU-side half (§3.3: getting a dmabuf fd OUT of QEMU at all).
//!
//! Run on a host with a free DRM master (a plain VT, no X/Wayland):
//!
//! ```text
//! cargo run -p mde-egui --example hello_prime_import --features drm
//! ```
//!
//! Allocates a small local GBM buffer, round-trips its dmabuf fd through this
//! project's own `buffer_to_prime_fd` → `prime_fd_to_buffer` →
//! `add_planar_framebuffer` import primitives (no QEMU involved at all), and
//! prints the resulting KMS framebuffer id. On a headless host it prints the same
//! clean `NoDrmMaster` fallback as `hello_drm`/`hello_video_plane` — the live
//! import is the hardware-gated `/preview`.

use mde_egui::probe_prime_import_liveness;

fn main() {
    match probe_prime_import_liveness() {
        Ok(outcome) => {
            println!(
                "PRIME import round-trip OK — re-imported framebuffer id {}",
                outcome.framebuffer_id
            );
        }
        Err(e) => {
            println!("no DRM seat here ({e}); PRIME-import liveness not probed");
        }
    }
}
