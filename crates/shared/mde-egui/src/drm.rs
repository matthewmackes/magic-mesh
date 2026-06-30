//! E12-2 — the **bare-seat backend**: egui rendered directly on a DRM/KMS seat,
//! with **no Wayland compositor** (governance §5; design
//! `docs/design/quasar-vdi-desktop.md`).
//!
//! This is the production runner for the MCNF shell; the eframe
//! [`crate::run_client`] path stays the dev/windowed runner. Both paint the same
//! backend-agnostic egui UI through the shared [`crate::Style`].
//!
//! The render path is **GL** — EGL on a GBM scanout surface, painted by
//! `egui_glow` — rather than wgpu, because that is the reliable bare-KMS path and
//! matches the GLES renderers used across the DRM ecosystem; the seat input is
//! **libinput** (+ udev). The stack is heavy and hardware-bound, so it is
//! feature-gated (`feature = "drm"`) and **degrades cleanly with a typed
//! [`DrmError`] when no DRM master is available** (CI / headless / another master
//! already holds the seat), per the E12-2 acceptance — the caller then falls back
//! to the windowed runner.
//!
//! **Status: in progress (E12-2).** This slice establishes the dependency stack +
//! the seat-acquisition + headless-degrade contract; the GBM/EGL render loop and
//! the libinput→egui input pump land in the following slices.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// Why the bare-seat backend could not start. The shell treats any variant as
/// "no usable seat here" and falls back to the windowed runner.
#[derive(Debug)]
pub enum DrmError {
    /// No usable DRM primary node / master — a headless host, no `/dev/dri/cardN`,
    /// or another DRM master already holds the seat.
    NoDrmMaster(String),
    /// The seat was acquired but the render/input backend is not yet wired (the
    /// in-progress E12-2 state; removed once the GBM/EGL loop lands).
    NotYetWired,
}

impl std::fmt::Display for DrmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DrmError::NoDrmMaster(why) => write!(f, "no usable DRM master: {why}"),
            DrmError::NotYetWired => {
                write!(
                    f,
                    "DRM seat acquired but the render loop is not yet wired (E12-2 in progress)"
                )
            }
        }
    }
}

impl std::error::Error for DrmError {}

/// Find and open the first usable DRM primary node (`/dev/dri/card0`, `card1`, …).
///
/// Returns the opened device or [`DrmError::NoDrmMaster`] when none can be opened —
/// the headless/CI case the acceptance requires to degrade cleanly.
fn open_primary_node() -> Result<(PathBuf, File), DrmError> {
    let dri = Path::new("/dev/dri");
    let mut last = String::from("no /dev/dri present");
    for idx in 0..8 {
        let path = dri.join(format!("card{idx}"));
        if !path.exists() {
            continue;
        }
        match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(e) => last = format!("{}: {e}", path.display()),
        }
    }
    Err(DrmError::NoDrmMaster(last))
}

/// Run an MCNF egui surface on the bare DRM/KMS seat (no compositor).
///
/// Establishes the seat (opens the primary DRM node), then — in the slices that
/// follow — sets up GBM + EGL + the `egui_glow` painter, pumps libinput into egui,
/// and atomically page-flips each frame onto the CRTC.
///
/// # Errors
/// [`DrmError::NoDrmMaster`] when no DRM master is available (headless/CI), so the
/// caller can fall back to [`crate::run_client`]; [`DrmError::NotYetWired`] on a
/// host that *does* have a seat, until the render loop lands.
pub fn run_drm(app_id: &str) -> Result<(), DrmError> {
    let (node, _dev) = open_primary_node()?;
    // Seat acquired. The render/input loop lands in the next E12-2 slice; until
    // then this reports the in-progress state explicitly rather than pretending.
    let _ = (app_id, node);
    Err(DrmError::NotYetWired)
}

#[cfg(test)]
mod tests {
    use super::{open_primary_node, DrmError};

    #[test]
    fn headless_degrades_cleanly() {
        // The seat probe must be total — never panic — and on a host with no DRM
        // master (the farm/CI case) it must return the clean NoDrmMaster fallback
        // the shell relies on. On a dev box with a GPU it may instead return Ok.
        match open_primary_node() {
            Ok(_) => {}
            Err(DrmError::NoDrmMaster(_)) => {}
            Err(other) => panic!("expected a clean NoDrmMaster fallback, got {other:?}"),
        }
    }
}
