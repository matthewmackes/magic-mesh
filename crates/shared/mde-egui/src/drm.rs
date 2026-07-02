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
//! **libinput** (+ udev, stage 3). The stack is heavy and hardware-bound, so it is
//! feature-gated (`feature = "drm"`) and **degrades cleanly with a typed
//! [`DrmError`] when no DRM master is available** (CI / headless / another master
//! already holds the seat) — the caller then falls back to the windowed runner.
//!
//! **Status (E12-2): all three stages compile + link green** — DRM/GBM bring-up ·
//! EGL/`egui_glow` present · the libinput seat + the continuous page-flip loop. The
//! farm can only *compile* this path (no DRM master headless); the live render +
//! input on a real seat is the hardware-gated `/preview`, which is why the unit
//! stays `[>]` (a render loop that compiles is not yet one that *works*).

// FFI backend: DRM/GBM/EGL/GL all require `unsafe`. The crate denies unsafe by
// default (mirroring the workspace); this one FFI module opts in — the rest of
// mde-egui stays unsafe-free.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use std::os::fd::OwnedFd;
use std::os::unix::fs::OpenOptionsExt;

use drm::control::{connector, crtc, Device as ControlDevice, Mode, ModeTypeFlags};
use drm::Device as BasicDevice;

use crate::display::{PanelInfo, PanelMode};
use gbm::AsRaw;
use input::event::keyboard::{KeyState, KeyboardEvent, KeyboardEventTrait};
use input::event::pointer::{ButtonState, PointerEvent};
use input::{Event as LiEvent, Libinput, LibinputInterface};
use khronos_egl as egl;

/// Why the bare-seat backend could not start / present. The shell treats any
/// variant as "no usable seat here" and falls back to the windowed runner.
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
    /// EGL display/context/surface setup failed.
    Egl(String),
    /// GL / `egui_glow` painter setup failed.
    Gl(String),
    /// The DRM modeset / framebuffer / page-flip present failed.
    Present(String),
}

impl std::fmt::Display for DrmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DrmError::NoDrmMaster(why) => write!(f, "no usable DRM master: {why}"),
            DrmError::NoOutput(why) => write!(f, "no usable DRM output: {why}"),
            DrmError::Gbm(why) => write!(f, "GBM surface allocation failed: {why}"),
            DrmError::Egl(why) => write!(f, "EGL setup failed: {why}"),
            DrmError::Gl(why) => write!(f, "GL/egui_glow setup failed: {why}"),
            DrmError::Present(why) => write!(f, "DRM present failed: {why}"),
        }
    }
}

impl std::error::Error for DrmError {}

fn egl_err(e: impl std::fmt::Display) -> DrmError {
    DrmError::Egl(e.to_string())
}

/// A DRM primary node wrapped so it implements the `drm` device traits (KMS).
///
/// Public because it appears in [`set_layout`]'s signature (`gbm::Device<Card>`);
/// its inner fd is private, so it is only ever produced inside this module.
pub struct Card(File);

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

/// The resolved scanout target: the connector to drive, a CRTC for it, the mode,
/// and — for the **multi-head** path (E12-18) — its top-left position in the
/// virtual desktop. The single-head [`run_drm`] loop ignores `position` (it drives
/// one output at the origin); [`resolve_outputs`] fills it left-to-right.
///
/// Public so the multi-CRTC drive ([`set_layout`], for E12-19's `host_state`
/// worker + the per-monitor-VM demo) has a nameable resolved-head type; its fields
/// carry `drm` crate handles the caller only round-trips back into `set_layout`.
pub struct Output {
    /// The connector (physical output) this head drives.
    pub connector: connector::Handle,
    /// The CRTC scanning this head out (distinct per head under [`resolve_outputs`]).
    pub crtc: crtc::Handle,
    /// The mode (resolution/refresh) set on this head.
    pub mode: Mode,
    /// Top-left of this output in the virtual desktop (px). `(0, 0)` for the
    /// primary; abutting offsets for the rest under [`resolve_outputs`].
    pub position: (i32, i32),
}

/// Resolve **every** connected output — one distinct CRTC per connector — and lay
/// them out left-to-right (E12-18's multi-CRTC enumeration; the atomic-modeset core
/// [`set_layout`] drives them).
///
/// It walks all
/// connectors, keeps the connected ones with a mode, and assigns each a CRTC that
/// no earlier output already took (a shared CRTC can't scan out two heads). The
/// first output sits at the origin; each subsequent output abuts the previous at
/// its width — the v1 single-row relative arrangement (matches `mde-seat`'s
/// `DisplayLayout::auto_arrange`). Headless-degrades exactly like the single path
/// ([`DrmError::NoOutput`] when nothing is connected).
fn resolve_outputs(card: &Card) -> Result<Vec<Output>, DrmError> {
    let res = card
        .resource_handles()
        .map_err(|e| DrmError::NoOutput(format!("resource_handles: {e}")))?;

    let mut outputs: Vec<Output> = Vec::new();
    let mut used_crtcs: Vec<crtc::Handle> = Vec::new();
    let mut x = 0_i32;

    for &conn_handle in res.connectors() {
        let Ok(conn) = card.get_connector(conn_handle, false) else {
            continue;
        };
        if conn.state() != connector::State::Connected {
            continue;
        }
        let Some(&mode) = conn.modes().first() else {
            continue;
        };
        // Prefer this connector's current CRTC, else any compatible one, skipping
        // CRTCs an earlier head already claimed so two outputs never collide.
        let candidates = conn
            .current_encoder()
            .and_then(|enc| card.get_encoder(enc).ok())
            .map(|enc| res.filter_crtcs(enc.possible_crtcs()))
            .unwrap_or_default();
        let crtc = candidates
            .iter()
            .chain(res.crtcs())
            .copied()
            .find(|c| !used_crtcs.contains(c));
        let Some(crtc) = crtc else {
            // Out of CRTCs — more heads than the GPU can scan out. The ones we
            // resolved still drive; the extra connector is left dark (honest).
            continue;
        };
        used_crtcs.push(crtc);
        let (w, _h) = mode.size();
        outputs.push(Output {
            connector: conn_handle,
            crtc,
            mode,
            position: (x, 0),
        });
        x += i32::from(w);
    }

    if outputs.is_empty() {
        return Err(DrmError::NoOutput(
            "no connected connector with a mode".into(),
        ));
    }
    Ok(outputs)
}

/// Drive a resolved multi-head layout: `set_crtc` each output onto its framebuffer
/// at its mode, and blank any CRTC not in the layout (enable/disable + per-output
/// mode set + relative arrangement — the atomic-modeset core, E12-18).
///
/// `fbs` supplies one framebuffer per `outputs` entry (same order); a caller with
/// one shared framebuffer (mirrored heads) passes the same handle repeatedly, a
/// caller scanning a different VM texture per head (the E12-10 demo) passes
/// distinct ones. Every CRTC on the card that no output claims is disabled with
/// `set_crtc(None)` so a de-arranged head goes properly dark.
///
/// This is the hardware-bound drive: the farm can only *compile* it (no DRM
/// master), so it stays behind the `drm` feature and is exercised live only on a
/// real seat (the hardware-gated multi-monitor demo). Kept `pub` so E12-19's
/// `host_state` worker + the demo can call it without it reading as dead code.
///
/// # Errors
/// [`DrmError::Present`] if a `set_crtc` fails, or if `fbs` is shorter than
/// `outputs` (a caller contract the type can't express).
pub fn set_layout(
    gbm: &gbm::Device<Card>,
    outputs: &[Output],
    fbs: &[drm::control::framebuffer::Handle],
) -> Result<(), DrmError> {
    if fbs.len() < outputs.len() {
        return Err(DrmError::Present(format!(
            "set_layout: {} framebuffers for {} outputs",
            fbs.len(),
            outputs.len()
        )));
    }
    let res = gbm
        .resource_handles()
        .map_err(|e| DrmError::Present(format!("resource_handles: {e}")))?;
    let claimed: Vec<crtc::Handle> = outputs.iter().map(|o| o.crtc).collect();

    for (out, &fb) in outputs.iter().zip(fbs) {
        // A CRTC's scanout origin is the head's position in the virtual desktop,
        // so a single shared framebuffer shows each head its own slice.
        let (x, y) = out.position;
        gbm.set_crtc(
            out.crtc,
            Some(fb),
            (u32::try_from(x).unwrap_or(0), u32::try_from(y).unwrap_or(0)),
            &[out.connector],
            Some(out.mode),
        )
        .map_err(|e| DrmError::Present(format!("set_crtc({:?}): {e}", out.connector)))?;
    }
    // Disable every CRTC not in the layout — a de-arranged head goes dark cleanly.
    for &c in res.crtcs() {
        if !claimed.contains(&c) {
            let _ = gbm.set_crtc(c, None, (0, 0), &[], None);
        }
    }
    Ok(())
}

/// Convert a live `drm` KMS [`Mode`] into the toolkit-agnostic [`PanelMode`]
/// (SURFACE-7 lock 12), carrying its preferred flag so the mode-list build knows the
/// native timing.
fn from_drm_mode(mode: &Mode) -> PanelMode {
    let (w, h) = mode.size();
    PanelMode::new(
        u32::from(w),
        u32::from(h),
        mode.vrefresh(),
        mode.mode_type().contains(ModeTypeFlags::PREFERRED),
    )
}

/// Build the typed [`PanelInfo`] (native mode + physical size + full mode list) from
/// a live connector (SURFACE-7 locks 11 + 12).
///
/// The native mode is the connector's PREFERRED timing if it flags one, else its
/// first mode (the driver's own preference order). Physical size comes from the
/// connector's reported mm (EDID-derived by the kernel); `(0, 0)` when unknown, which
/// makes the fractional scale fall back to 1.0. Returns `None` when the connector
/// advertises no modes (nothing to drive).
fn panel_from_connector(conn: &connector::Info) -> Option<PanelInfo> {
    let modes = conn.modes();
    if modes.is_empty() {
        return None;
    }
    let native = modes
        .iter()
        .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
        .or_else(|| modes.first())
        .map(from_drm_mode)?;
    let raw: Vec<PanelMode> = modes.iter().map(from_drm_mode).collect();
    let phys_mm = conn.size().unwrap_or((0, 0));
    Some(PanelInfo::new(native, phys_mm, &raw))
}

/// libinput device opener for a bare seat (root on a VT). The present loop pumps
/// egui input from here; on a host with logind a seat manager would mediate fd
/// access — that path is a follow-up.
struct SeatInterface;

impl LibinputInterface for SeatInterface {
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(flags)
            .open(path)
            .map(OwnedFd::from)
            .map_err(|e| e.raw_os_error().unwrap_or(5))
    }

    fn close_restricted(&mut self, fd: OwnedFd) {
        drop(fd);
    }
}

/// Map a Linux evdev key code to an [`egui::Key`].
///
/// Returns `None` for modifier keys, numpad keys without a clear mapping, and
/// unmapped codes. Modifier key state is tracked separately in [`run_drm`].
fn drm_key(code: u32) -> Option<egui::Key> {
    Some(match code {
        1 => egui::Key::Escape,
        14 => egui::Key::Backspace,
        15 => egui::Key::Tab,
        28 | 96 => egui::Key::Enter,
        57 => egui::Key::Space,
        // F-keys
        59 => egui::Key::F1,
        60 => egui::Key::F2,
        61 => egui::Key::F3,
        62 => egui::Key::F4,
        63 => egui::Key::F5,
        64 => egui::Key::F6,
        65 => egui::Key::F7,
        66 => egui::Key::F8,
        67 => egui::Key::F9,
        68 => egui::Key::F10,
        87 => egui::Key::F11,
        88 => egui::Key::F12,
        // Navigation cluster
        102 => egui::Key::Home,
        107 => egui::Key::End,
        103 => egui::Key::ArrowUp,
        108 => egui::Key::ArrowDown,
        105 => egui::Key::ArrowLeft,
        106 => egui::Key::ArrowRight,
        104 => egui::Key::PageUp,
        109 => egui::Key::PageDown,
        110 => egui::Key::Insert,
        111 => egui::Key::Delete,
        // Letters (physical key — layout-independent)
        16 => egui::Key::Q,
        17 => egui::Key::W,
        18 => egui::Key::E,
        19 => egui::Key::R,
        20 => egui::Key::T,
        21 => egui::Key::Y,
        22 => egui::Key::U,
        23 => egui::Key::I,
        24 => egui::Key::O,
        25 => egui::Key::P,
        30 => egui::Key::A,
        31 => egui::Key::S,
        32 => egui::Key::D,
        33 => egui::Key::F,
        34 => egui::Key::G,
        35 => egui::Key::H,
        36 => egui::Key::J,
        37 => egui::Key::K,
        38 => egui::Key::L,
        44 => egui::Key::Z,
        45 => egui::Key::X,
        46 => egui::Key::C,
        47 => egui::Key::V,
        48 => egui::Key::B,
        49 => egui::Key::N,
        50 => egui::Key::M,
        // Digit row
        2 => egui::Key::Num1,
        3 => egui::Key::Num2,
        4 => egui::Key::Num3,
        5 => egui::Key::Num4,
        6 => egui::Key::Num5,
        7 => egui::Key::Num6,
        8 => egui::Key::Num7,
        9 => egui::Key::Num8,
        10 => egui::Key::Num9,
        11 => egui::Key::Num0,
        _ => return None,
    })
}

/// Map a Linux evdev key code + shift state to a printable character (US QWERTY).
///
/// Returns `None` for non-printable or unmapped codes.
#[allow(clippy::too_many_lines)]
fn drm_char(code: u32, shift: bool) -> Option<char> {
    Some(match (code, shift) {
        // Letters
        (16, false) => 'q',
        (16, true) => 'Q',
        (17, false) => 'w',
        (17, true) => 'W',
        (18, false) => 'e',
        (18, true) => 'E',
        (19, false) => 'r',
        (19, true) => 'R',
        (20, false) => 't',
        (20, true) => 'T',
        (21, false) => 'y',
        (21, true) => 'Y',
        (22, false) => 'u',
        (22, true) => 'U',
        (23, false) => 'i',
        (23, true) => 'I',
        (24, false) => 'o',
        (24, true) => 'O',
        (25, false) => 'p',
        (25, true) => 'P',
        (30, false) => 'a',
        (30, true) => 'A',
        (31, false) => 's',
        (31, true) => 'S',
        (32, false) => 'd',
        (32, true) => 'D',
        (33, false) => 'f',
        (33, true) => 'F',
        (34, false) => 'g',
        (34, true) => 'G',
        (35, false) => 'h',
        (35, true) => 'H',
        (36, false) => 'j',
        (36, true) => 'J',
        (37, false) => 'k',
        (37, true) => 'K',
        (38, false) => 'l',
        (38, true) => 'L',
        (44, false) => 'z',
        (44, true) => 'Z',
        (45, false) => 'x',
        (45, true) => 'X',
        (46, false) => 'c',
        (46, true) => 'C',
        (47, false) => 'v',
        (47, true) => 'V',
        (48, false) => 'b',
        (48, true) => 'B',
        (49, false) => 'n',
        (49, true) => 'N',
        (50, false) => 'm',
        (50, true) => 'M',
        // Digits and their shift-symbols
        (2, false) => '1',
        (2, true) => '!',
        (3, false) => '2',
        (3, true) => '@',
        (4, false) => '3',
        (4, true) => '#',
        (5, false) => '4',
        (5, true) => '$',
        (6, false) => '5',
        (6, true) => '%',
        (7, false) => '6',
        (7, true) => '^',
        (8, false) => '7',
        (8, true) => '&',
        (9, false) => '8',
        (9, true) => '*',
        (10, false) => '9',
        (10, true) => '(',
        (11, false) => '0',
        (11, true) => ')',
        // Punctuation
        (12, false) => '-',
        (12, true) => '_',
        (13, false) => '=',
        (13, true) => '+',
        (26, false) => '[',
        (26, true) => '{',
        (27, false) => ']',
        (27, true) => '}',
        (43, false) => '\\',
        (43, true) => '|',
        (39, false) => ';',
        (39, true) => ':',
        (40, false) => '\'',
        (40, true) => '"',
        (41, false) => '`',
        (41, true) => '~',
        (51, false) => ',',
        (51, true) => '<',
        (52, false) => '.',
        (52, true) => '>',
        (53, false) => '/',
        (53, true) => '?',
        (57, _) => ' ',
        _ => return None,
    })
}

/// Run an MCNF egui surface on the bare DRM/KMS seat (no compositor).
///
/// `ui` paints the surface each frame against an [`egui::Context`] (the shared
/// [`crate::Style`] is installed before the first paint). Brings the seat up, then
/// runs the present loop: pump libinput → egui events, render through `Style`, and
/// scan each frame out via DRM page-flip (Esc quits). Blocks until the surface exits.
///
/// # Errors
/// [`DrmError::NoDrmMaster`] when no DRM master is available (headless/CI) so the
/// caller can fall back to [`crate::run_client`]; the other variants on a seat that
/// can't be driven / presented.
pub fn run_drm(app_id: &str, mut ui: impl FnMut(&egui::Context)) -> Result<(), DrmError> {
    let _ = app_id;
    let (_node, file) = open_primary_node()?;
    let card = Card(file);
    // Enumerate every connected head (E12-18 multi-CRTC). The primary drives the
    // GBM/EGL surface + present loop; additional heads with the SAME mode size
    // mirror it (clone mode) — a real second-monitor drive with legacy page-flip.
    // Heterogeneous heads (different modes / a distinct VM texture per head — the
    // two-monitors-two-VMs demo) are the hardware-gated drive E12-19 does with the
    // same [`set_layout`] primitive over per-head framebuffers + atomic commits.
    let resolved = resolve_outputs(&card)?;
    let primary_size = resolved[0].mode.size();
    let heads: Vec<Output> = {
        let mut it = resolved.into_iter();
        let mut v = vec![it.next().expect("resolve_outputs is non-empty on Ok")];
        for o in it {
            if o.mode.size() == primary_size {
                // Mirror at the origin — same framebuffer, same viewport.
                v.push(Output {
                    position: (0, 0),
                    ..o
                });
            }
        }
        v
    };
    let (w, h) = heads[0].mode.size();
    let (wp, hp) = (u32::from(w), u32::from(h));

    // GBM device from the DRM fd (the `gbm::Device` also drives KMS via
    // the drm-support feature, so it stands in for `card` from here on).
    // The GBM *surface* is created AFTER EGL config selection so its format
    // matches EGL_NATIVE_VISUAL_ID — creating the surface first and then
    // searching for a matching EGL config inverts the dependency and causes
    // Mesa to internally re-select a different DRI config for the window
    // surface than the one the context was created with, triggering
    // EGL_BAD_MATCH at eglMakeCurrent (seen live on Eagle Intel iGPU).
    let gbm = gbm::Device::new(card).map_err(|e| DrmError::Gbm(format!("gbm device: {e}")))?;

    // --- EGL on the GBM device (Mesa accepts the gbm device as the native display) ---
    let egl = unsafe { egl::DynamicInstance::<egl::EGL1_4>::load_required() }
        .map_err(|e| DrmError::Egl(format!("load libEGL: {e}")))?;
    let display = unsafe {
        egl.get_display(gbm.as_raw() as *mut c_void)
            .ok_or_else(|| DrmError::Egl("eglGetDisplay returned no display".into()))?
    };
    egl.initialize(display)
        .map_err(|e| DrmError::Egl(format!("eglInitialize: {e}")))?;
    egl.bind_api(egl::OPENGL_ES_API)
        .map_err(|e| DrmError::Egl(format!("eglBindAPI: {e}")))?;

    let attribs = [
        egl::SURFACE_TYPE,
        egl::WINDOW_BIT,
        egl::RENDERABLE_TYPE,
        egl::OPENGL_ES2_BIT,
        egl::RED_SIZE,
        8,
        egl::GREEN_SIZE,
        8,
        egl::BLUE_SIZE,
        8,
        egl::ALPHA_SIZE,
        0,
        egl::NONE,
    ];
    let mut configs = Vec::with_capacity(32);
    egl.choose_config(display, &attribs, &mut configs)
        .map_err(|e| DrmError::Egl(format!("eglChooseConfig: {e}")))?;
    if configs.is_empty() {
        return Err(DrmError::Egl("eglChooseConfig: no configs matched".into()));
    }
    // Pick config + GBM format together: prefer XRGB8888, then ARGB8888, then
    // accept the first config whose NATIVE_VISUAL_ID converts to any recognized
    // GBM format. On Intel iris with Mesa 26 all configs advertise
    // XRGB2101010 (0x30335258, "XR30") — the fallback branch handles that.
    const DRM_FORMAT_XRGB8888: egl::Int = 0x3432_5258;
    const DRM_FORMAT_ARGB8888: egl::Int = 0x3432_5241;
    let (config, gbm_format) = configs
        .iter()
        .copied()
        .find(|&c| {
            egl.get_config_attrib(display, c, egl::NATIVE_VISUAL_ID) == Ok(DRM_FORMAT_XRGB8888)
        })
        .map(|c| (c, gbm::Format::Xrgb8888))
        .or_else(|| {
            configs
                .iter()
                .copied()
                .find(|&c| {
                    egl.get_config_attrib(display, c, egl::NATIVE_VISUAL_ID)
                        == Ok(DRM_FORMAT_ARGB8888)
                })
                .map(|c| (c, gbm::Format::Argb8888))
        })
        .or_else(|| {
            // Use DrmFourcc TryFrom to accept ANY format the driver advertises.
            use std::convert::TryFrom;
            configs.iter().copied().find_map(|c| {
                let vid = egl
                    .get_config_attrib(display, c, egl::NATIVE_VISUAL_ID)
                    .ok()?;
                let fmt = gbm::Format::try_from(vid as u32).ok()?;
                Some((c, fmt))
            })
        })
        .ok_or_else(|| DrmError::Egl("no EGL config with a recognized GBM format".into()))?;

    // GBM scanout surface — format chosen to match the selected EGL config.
    let gbm_surface = gbm
        .create_surface::<()>(
            wp,
            hp,
            gbm_format,
            gbm::BufferObjectFlags::SCANOUT | gbm::BufferObjectFlags::RENDERING,
        )
        .map_err(|e| DrmError::Gbm(format!("gbm surface {wp}x{hp}: {e}")))?;

    let context = egl
        .create_context(
            display,
            config,
            None,
            &[egl::CONTEXT_MAJOR_VERSION, 2, egl::NONE],
        )
        .map_err(|e| DrmError::Egl(format!("eglCreateContext: {e}")))?;
    let surface = unsafe {
        egl.create_window_surface(display, config, gbm_surface.as_raw() as *mut c_void, None)
            .map_err(|e| DrmError::Egl(format!("eglCreateWindowSurface: {e}")))?
    };
    egl.make_current(display, Some(surface), Some(surface), Some(context))
        .map_err(|e| DrmError::Egl(format!("eglMakeCurrent: {e}")))?;

    // --- glow + egui_glow on the EGL context ---
    let gl = unsafe {
        glow::Context::from_loader_function(|s| {
            egl.get_proc_address(s)
                .map_or(std::ptr::null(), |f| f as *const c_void)
        })
    };
    let mut painter = egui_glow::Painter::new(Arc::new(gl), "", None, false)
        .map_err(|e| DrmError::Gl(e.to_string()))?;

    // --- the present loop: pump the seat, render, scan out, repeat (Esc quits) ---
    let egui_ctx = egui::Context::default();
    crate::Style::install(&egui_ctx);

    // SURFACE-7 lock 11: detect the primary panel (native mode + physical size) and
    // drive a FRACTIONAL HiDPI egui scale from its DPI, so UI is crisp + correctly
    // sized on a high-PPI panel (a Surface Pro lands ~2.25) instead of the 1.0
    // default. The mode LIST + native detect are real here; the live native↔HD
    // KMS switch (lock 12) is driven through the injectable [`display::ModesetSeam`]
    // — its headless arm returns an honest gated error and the running loop's live
    // surface-rebuild switch is the hardware-gated follow-up, so the seat starts at
    // native (no faked hot-switch). `gbm` also speaks KMS, so it reads the connector.
    let ppp = gbm
        .get_connector(heads[0].connector, false)
        .ok()
        .and_then(|conn| panel_from_connector(&conn))
        .map_or(1.0, |panel| panel.scale());
    if (ppp - 1.0).abs() > f32::EPSILON {
        egui_ctx.set_pixels_per_point(ppp);
    }

    let mut libinput = Libinput::new_with_udev(SeatInterface);
    libinput
        .udev_assign_seat("seat0")
        .map_err(|()| DrmError::Present("libinput: udev_assign_seat(seat0) failed".into()))?;

    // egui works in POINTS = pixels / pixels_per_point. The framebuffer is `wp × hp`
    // physical pixels; the layout + pointer live in points, so a fractional scale
    // sizes the UI correctly. With ppp == 1.0 this is byte-identical to the old
    // pixel-space path.
    let (pw, ph) = (wp as f32 / ppp, hp as f32 / ppp);
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(pw, ph));
    let mut pointer = egui::pos2(pw / 2.0, ph / 2.0);
    let start = std::time::Instant::now();
    // The previous frame's scanout buffer + framebuffer, freed only after the next
    // flip completes (GBM hands out a small ring of buffers).
    let mut prev: Option<(gbm::BufferObject<()>, drm::control::framebuffer::Handle)> = None;
    let mut quit = false;
    // Modifier state: updated on each KeyDown/KeyUp before feeding egui Key events.
    let mut shift = false;
    let mut ctrl = false;
    let mut alt = false;

    while !quit {
        // 1. drain libinput → egui events
        libinput
            .dispatch()
            .map_err(|e| DrmError::Present(format!("libinput dispatch: {e}")))?;
        let mut events = Vec::new();
        for event in &mut libinput {
            match event {
                LiEvent::Pointer(PointerEvent::Motion(m)) => {
                    // libinput deltas are physical pixels; egui pointer is in points.
                    pointer.x = (pointer.x + m.dx() as f32 / ppp).clamp(0.0, pw);
                    pointer.y = (pointer.y + m.dy() as f32 / ppp).clamp(0.0, ph);
                    events.push(egui::Event::PointerMoved(pointer));
                }
                LiEvent::Pointer(PointerEvent::MotionAbsolute(m)) => {
                    pointer = egui::pos2(
                        m.absolute_x_transformed(wp) as f32 / ppp,
                        m.absolute_y_transformed(hp) as f32 / ppp,
                    );
                    events.push(egui::Event::PointerMoved(pointer));
                }
                LiEvent::Pointer(PointerEvent::Button(b)) => {
                    events.push(egui::Event::PointerButton {
                        pos: pointer,
                        button: egui::PointerButton::Primary,
                        pressed: b.button_state() == ButtonState::Pressed,
                        modifiers: egui::Modifiers::default(),
                    });
                }
                LiEvent::Keyboard(KeyboardEvent::Key(k)) => {
                    let pressed = k.key_state() == KeyState::Pressed;
                    let code = k.key();
                    // Track modifier state
                    match code {
                        42 | 54 => shift = pressed, // LEFTSHIFT | RIGHTSHIFT
                        29 | 97 => ctrl = pressed,  // LEFTCTRL | RIGHTCTRL
                        56 | 100 => alt = pressed,  // LEFTALT | RIGHTALT
                        _ => {}
                    }
                    if code == 1 && pressed {
                        quit = true; // ESC exits the DRM shell
                    }
                    // E12-19 (lock 8): the XF86 media/system keys + the Super leader
                    // have no egui::Key, so forward the host-relevant scancodes on
                    // the host-key side channel. The shell drains them each frame and
                    // dispatches them host-first (even over a fullscreen guest); they
                    // are NOT emitted as egui events, so the guest never sees them.
                    if crate::hostkeys::is_host_key(code) {
                        crate::hostkeys::push_host_key(code, pressed);
                    }
                    let modifiers = egui::Modifiers {
                        alt,
                        ctrl,
                        shift,
                        ..Default::default()
                    };
                    if let Some(key) = drm_key(code) {
                        events.push(egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed,
                            repeat: false,
                            modifiers,
                        });
                    }
                    // Text for printable keys (not when ctrl/alt held — those are shortcuts)
                    if pressed && !ctrl && !alt {
                        if let Some(ch) = drm_char(code, shift) {
                            events.push(egui::Event::Text(ch.to_string()));
                        }
                    }
                }
                _ => {}
            }
        }

        // 2. run + paint the egui frame
        let raw_input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(start.elapsed().as_secs_f64()),
            events,
            ..Default::default()
        };
        let cur = pointer;
        let full_output = egui_ctx.run(raw_input, |ctx| {
            ui(ctx);
            // Software cursor: draw a small crosshair/dot at the pointer position.
            // The DRM backend has no OS cursor; we own the whole framebuffer.
            let layer = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                egui::Id::new("drm_cursor"),
            ));
            layer.circle_filled(cur, 4.0, egui::Color32::WHITE);
            layer.circle_stroke(cur, 4.0, egui::Stroke::new(1.0, egui::Color32::BLACK));
        });
        let clipped = egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        painter.paint_and_update_textures(
            [wp, hp],
            full_output.pixels_per_point,
            &clipped,
            &full_output.textures_delta,
        );
        egl.swap_buffers(display, surface).map_err(egl_err)?;

        // 3. scan the new front buffer out — set_crtc on the first frame, page-flip
        //    after (waiting for the flip to complete before recycling buffers).
        let bo = unsafe {
            gbm_surface
                .lock_front_buffer()
                .map_err(|e| DrmError::Present(format!("lock_front_buffer: {e}")))?
        };
        let (fb_depth, fb_bpp) = match gbm_format {
            gbm::Format::Xrgb2101010 | gbm::Format::Argb2101010 => (30u32, 32u32),
            gbm::Format::Rgb565 | gbm::Format::Bgr565 => (16, 16),
            _ => (24, 32),
        };
        let fb = gbm
            .add_framebuffer(&bo, fb_depth, fb_bpp)
            .map_err(|e| DrmError::Present(format!("add_framebuffer: {e}")))?;
        if prev.is_none() {
            // First frame: the atomic-modeset core lights every head onto this
            // framebuffer (each at its viewport) and blanks any unclaimed CRTC.
            // Single-head → one set_crtc at the origin, identical to before.
            let fbs = vec![fb; heads.len()];
            set_layout(&gbm, &heads, &fbs)?;
        } else {
            // Flip every head to the new front buffer, then drain one PageFlip
            // completion per head before recycling buffers (vblank sync).
            for head in &heads {
                gbm.page_flip(head.crtc, fb, drm::control::PageFlipFlags::EVENT, None)
                    .map_err(|e| DrmError::Present(format!("page_flip: {e}")))?;
            }
            let mut pending = heads.len();
            while pending > 0 {
                let evs = gbm
                    .receive_events()
                    .map_err(|e| DrmError::Present(format!("receive_events: {e}")))?;
                for ev in evs {
                    if matches!(ev, drm::control::Event::PageFlip(_)) {
                        pending -= 1;
                    }
                }
            }
        }
        if let Some((prev_bo, prev_fb)) = prev.take() {
            let _ = gbm.destroy_framebuffer(prev_fb);
            drop(prev_bo);
        }
        prev = Some((bo, fb));
    }

    // teardown (best-effort; the OS reclaims the rest on exit)
    if let Some((bo, fb)) = prev.take() {
        let _ = gbm.destroy_framebuffer(fb);
        drop(bo);
    }
    painter.destroy();
    Ok(())
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
