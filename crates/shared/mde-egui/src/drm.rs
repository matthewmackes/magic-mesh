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
//! **Status: in progress (E12-2), built in stages so each farm compile validates a
//! bounded slice of the native APIs:**
//! - stage 1 (this slice): the **DRM/KMS modeset target** + the **GBM scanout
//!   surface** — open the primary node, pick a connected connector + its preferred
//!   mode + a compatible CRTC, and allocate a GBM surface at that resolution.
//! - stage 2 (next): EGL display/context on the GBM device + a `glow` context +
//!   `egui_glow` painting + the page-flip present loop.
//! - stage 3: the libinput → egui raw-input pump.

use std::fs::{File, OpenOptions};
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};

use drm::control::{connector, crtc, Device as ControlDevice, Mode};
use drm::Device as BasicDevice;

/// Why the bare-seat backend could not start. The shell treats any variant as
/// "no usable seat here" and falls back to the windowed runner.
#[derive(Debug)]
pub enum DrmError {
    /// No usable DRM primary node / master — a headless host, no `/dev/dri/cardN`,
    /// or another DRM master already holds the seat.
    NoDrmMaster(String),
    /// The DRM device opened but KMS resources / a connected output could not be
    /// resolved (no connected connector, no mode, no compatible CRTC).
    NoOutput(String),
    /// GBM scanout-surface allocation failed.
    Gbm(String),
    /// The render/input loop past GBM is not yet wired (the in-progress stage-1
    /// state; removed once the EGL/`egui_glow` present loop lands).
    NotYetWired,
}

impl std::fmt::Display for DrmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DrmError::NoDrmMaster(why) => write!(f, "no usable DRM master: {why}"),
            DrmError::NoOutput(why) => write!(f, "no usable DRM output: {why}"),
            DrmError::Gbm(why) => write!(f, "GBM surface allocation failed: {why}"),
            DrmError::NotYetWired => {
                write!(f, "DRM seat + GBM surface up; EGL/egui_glow present loop not yet wired (E12-2 in progress)")
            }
        }
    }
}

impl std::error::Error for DrmError {}

/// A DRM primary node wrapped so it implements the `drm` device traits (KMS).
struct Card(File);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}
impl BasicDevice for Card {}
impl ControlDevice for Card {}

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

/// The resolved scanout target: the connector to drive, the mode to set, and a
/// CRTC that can drive it.
struct Output {
    connector: connector::Handle,
    crtc: crtc::Handle,
    mode: Mode,
}

/// Resolve a connected output (connector + preferred mode + a compatible CRTC).
fn resolve_output(card: &Card) -> Result<Output, DrmError> {
    let res = card
        .resource_handles()
        .map_err(|e| DrmError::NoOutput(format!("resource_handles: {e}")))?;

    // First connected connector with at least one mode.
    for &conn_handle in res.connectors() {
        let Ok(conn) = card.get_connector(conn_handle, false) else {
            continue;
        };
        if conn.state() != connector::State::Connected {
            continue;
        }
        // The first mode is the driver's preferred mode.
        let Some(&mode) = conn.modes().first() else {
            continue;
        };
        // A CRTC compatible with this connector: prefer the current encoder's
        // possible_crtcs, else the first CRTC the resources expose.
        let crtc = conn
            .current_encoder()
            .and_then(|enc| card.get_encoder(enc).ok())
            .and_then(|enc| res.filter_crtcs(enc.possible_crtcs()).first().copied())
            .or_else(|| res.crtcs().first().copied())
            .ok_or_else(|| DrmError::NoOutput("no CRTC for the connected connector".into()))?;

        return Ok(Output {
            connector: conn_handle,
            crtc,
            mode,
        });
    }
    Err(DrmError::NoOutput(
        "no connected connector with a mode".into(),
    ))
}

/// Run an MCNF egui surface on the bare DRM/KMS seat (no compositor).
///
/// Stage 1 brings the seat up to a GBM scanout surface at the output's native
/// resolution; the EGL/`egui_glow` present loop + the libinput pump land in the
/// stages that follow.
///
/// # Errors
/// [`DrmError::NoDrmMaster`] when no DRM master is available (headless/CI), so the
/// caller can fall back to [`crate::run_client`]; [`DrmError::NoOutput`] /
/// [`DrmError::Gbm`] on a seat that can't be driven; [`DrmError::NotYetWired`] once
/// the seat + GBM surface are up, until the present loop lands.
pub fn run_drm(app_id: &str) -> Result<(), DrmError> {
    let (_node, file) = open_primary_node()?;
    let card = Card(file);

    let output = resolve_output(&card)?;
    let (w, h) = output.mode.size();

    // Allocate the GBM scanout surface at the mode resolution. XRGB8888 +
    // SCANOUT|RENDERING is the universally-supported KMS-presentable format.
    let gbm = gbm::Device::new(card).map_err(|e| DrmError::Gbm(format!("gbm device: {e}")))?;
    let _surface = gbm
        .create_surface::<()>(
            u32::from(w),
            u32::from(h),
            gbm::Format::Xrgb8888,
            gbm::BufferObjectFlags::SCANOUT | gbm::BufferObjectFlags::RENDERING,
        )
        .map_err(|e| DrmError::Gbm(format!("gbm surface {w}x{h}: {e}")))?;

    // Seat + GBM surface are up at the native mode. The EGL context + egui_glow
    // present loop (stage 2) and the libinput pump (stage 3) land next; until then
    // report the in-progress state honestly rather than pretending to render.
    let _ = (app_id, output.connector, output.crtc);
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
