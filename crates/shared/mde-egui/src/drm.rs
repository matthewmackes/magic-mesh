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
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use std::os::fd::OwnedFd;
use std::os::unix::fs::OpenOptionsExt;

use drm::buffer;
use drm::control::{
    connector, crtc, plane, property, Device as ControlDevice, FbCmd2Flags, Mode, ModeTypeFlags,
};
use drm::Device as BasicDevice;

use crate::display::{PanelInfo, PanelMode};
use gbm::AsRaw;
use input::event::device::DeviceEvent;
use input::event::keyboard::{KeyState, KeyboardEvent, KeyboardEventTrait};
use input::event::pointer::{ButtonState, PointerEvent};
use input::event::switch::{Switch, SwitchEvent, SwitchState as LiSwitchState};
use input::event::touch::{TouchEvent, TouchEventPosition, TouchEventSlot};
use input::event::EventTrait;
use input::{DeviceCapability, Event as LiEvent, Libinput, LibinputInterface};
use khronos_egl as egl;

use crate::formfactor::{
    apply_rotation, orientation_from_accel, push_formfactor, take_rotation_commands, AccelSensor,
    AutoRotate, FormfactorDebounce, RotateCommand, RotateError, RotationApply, SwitchState,
    SysfsAccel,
};
use crate::touch::{RawContact, Rotation, TouchTransform, TouchTranslator};
use crate::video_plane::{
    PaneRect, PlaneInfo, PlaneKind, PlaneSet, VideoPath, VideoPlaneError, VideoPlanePlan,
    VideoScanout,
};

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

/// The `DRM_MODE_ROTATE_*` bit for a [`Rotation`], as the KMS plane `rotation`
/// property expects it (a bitmask; we set exactly the one rotate bit, no reflect).
const fn rotate_bits(rotation: Rotation) -> u64 {
    match rotation {
        Rotation::None => 1,      // DRM_MODE_ROTATE_0
        Rotation::Rotate90 => 2,  // DRM_MODE_ROTATE_90
        Rotation::Rotate180 => 4, // DRM_MODE_ROTATE_180
        Rotation::Rotate270 => 8, // DRM_MODE_ROTATE_270
    }
}

/// Discover the active primary plane's `rotation` property for a CRTC, if the driver
/// exposes one. Returns the `(plane, property)` handles the live [`PlaneRotate`] seam
/// drives; `None` when no bound plane carries a `rotation` property (many simple /
/// virtual KMS drivers don't) — SURFACE-9 then honestly leaves rotation un-applied so
/// display + touch never desync.
fn discover_rotation_prop(
    dev: &gbm::Device<Card>,
    crtc: crtc::Handle,
) -> Option<(plane::Handle, property::Handle)> {
    let planes = dev.plane_handles().ok()?;
    for ph in planes {
        let Ok(info) = dev.get_plane(ph) else {
            continue;
        };
        // The plane already scanning this CRTC out is its primary plane.
        if info.crtc() != Some(crtc) {
            continue;
        }
        let Ok(props) = dev.get_properties(ph) else {
            continue;
        };
        let (handles, _values) = props.as_props_and_values();
        for &prop in handles {
            if let Ok(pinfo) = dev.get_property(prop) {
                if pinfo.name().to_str() == Ok("rotation") {
                    return Some((ph, prop));
                }
            }
        }
    }
    None
}

/// The live KMS scanout-rotation seam (SURFACE-9 lock 15): sets the primary plane's
/// `rotation` property so the framebuffer scans out rotated to match the touch matrix.
/// Constructed per-apply over the borrowed KMS device + the discovered handles; it
/// honestly returns [`RotateError::Commit`] when the legacy property set is rejected
/// (e.g. a driver that only rotates via an atomic commit) — never a faked turn.
struct PlaneRotate<'a> {
    dev: &'a gbm::Device<Card>,
    plane: plane::Handle,
    prop: property::Handle,
}

impl RotationApply for PlaneRotate<'_> {
    fn apply(&mut self, rotation: Rotation) -> Result<(), RotateError> {
        self.dev
            .set_property(self.plane, self.prop, rotate_bits(rotation))
            .map_err(|e| RotateError::Commit(format!("set plane rotation: {e}")))
    }
}

/// Apply a committed rotation to **both** the scanout and the touch matrix (lock 15).
/// When the driver exposed no rotation property (`rot_prop` is `None`) or the KMS
/// commit fails, the touch matrix is left unrotated so the two stay in sync, and the
/// honest reason is reported once (the live rotate is the hardware-gated path).
fn drive_rotation(
    dev: &gbm::Device<Card>,
    rot_prop: Option<(plane::Handle, property::Handle)>,
    touch: &mut TouchTranslator,
    rotation: Rotation,
) {
    let Some((plane, prop)) = rot_prop else {
        eprintln!(
            "mde-egui: auto-rotate to {rotation:?} — no KMS rotation property on this \
             scanout; display + touch left unrotated (hardware-gated)"
        );
        return;
    };
    let mut seam = PlaneRotate { dev, plane, prop };
    if let Err(e) = apply_rotation(rotation, &mut seam, touch) {
        eprintln!("mde-egui: auto-rotate to {rotation:?} failed ({e}); touch left in sync");
    }
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

/// The bare-seat present loop's **wake policy** (perf-1), factored out pure so it can
/// be unit-tested headlessly — the live present path itself can only be exercised on
/// a real seat.
///
/// The loop is **event-driven**: it blocks in `poll(2)` on the libinput fd until input
/// arrives, egui's requested repaint deadline expires, or a periodic task (accelerometer
/// sample / touch long-press tick) is due — then renders *only* when there is something
/// to show. This mirrors eframe's winit contract (`ControlFlow` Wait vs WaitUntil vs
/// Poll, decided from egui's [`egui::ViewportOutput::repaint_delay`]): a `Duration::MAX`
/// delay means "idle — sleep until an fd wakes us" and drops the CPU to ~0, a
/// `Duration::ZERO` delay means "repaint continuously" (a streaming VDI session, a
/// spinner), and a finite delay arms a bounded wait. Before this, the loop rendered the
/// full UI every vblank forever, so every `request_repaint_after` in the shell was dead.
mod wake {
    use std::time::{Duration, Instant};

    /// Upper bound (ms) on a *finite* poll wait. Caps the `c_int` timeout and bounds
    /// the worst-case latency of a finite deadline; 1 s is far longer than any real UI
    /// repaint delay. The genuinely-idle case (`None`/`None`) still blocks indefinitely
    /// — this only caps requests that named a finite deadline.
    const MAX_FINITE_MS: u128 = 1000;

    /// egui's requested repaint delay → the loop's repaint deadline, as a `Duration`
    /// from *now*. `None` = idle (egui's `Duration::MAX` sentinel: no repaint wanted);
    /// `Some(d)` = repaint due in `d` (`Some(ZERO)` = due immediately, i.e. continuous
    /// repaint).
    #[must_use]
    pub fn repaint_deadline(repaint_delay: Duration) -> Option<Duration> {
        if repaint_delay == Duration::MAX {
            None
        } else {
            Some(repaint_delay)
        }
    }

    /// egui's requested repaint delay → an absolute monotonic wake deadline for the
    /// DRM loop. `None` is the idle state; `Some(now)` is an immediate repaint.
    #[must_use]
    pub fn repaint_deadline_at(repaint_delay: Duration, now: Instant) -> Option<Instant> {
        repaint_deadline(repaint_delay).and_then(|delay| now.checked_add(delay))
    }

    /// The `poll(2)` timeout (ms) for this iteration, from the time until the next
    /// repaint (`None` = egui idle) and the time until the soonest periodic task
    /// (`None` = none pending). Returns:
    /// * `-1` → block indefinitely until an fd is ready (both deadlines idle — the
    ///   true-idle path that lets the CPU reach ~0);
    /// * `0`  → do not block (a deadline is already due / continuous repaint);
    /// * `>0` → block up to that many ms (the sooner of the two deadlines, rounded up
    ///   so a sub-ms deadline can never collapse to a 0-ms busy poll, capped at
    ///   [`MAX_FINITE_MS`]).
    #[must_use]
    pub fn poll_timeout_ms(
        until_repaint: Option<Duration>,
        until_periodic: Option<Duration>,
    ) -> i32 {
        let deadline = match (until_repaint, until_periodic) {
            (None, None) => return -1, // genuinely idle: sleep until an fd is ready
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
        };
        if deadline.is_zero() {
            return 0;
        }
        // Round any sub-ms remainder UP to a whole ms (so we wake no earlier than the
        // deadline — never a hair early to re-poll), then clamp into [1, MAX_FINITE_MS]
        // so a sub-ms deadline can't collapse to a 0-ms (busy) poll and a huge one can't
        // overflow the c_int timeout. The clamped result fits i32 by construction.
        let ms = deadline
            .as_nanos()
            .div_ceil(1_000_000)
            .clamp(1, MAX_FINITE_MS);
        ms as i32
    }

    /// Whether to render + present this iteration. The loop renders on the first frame,
    /// on any input event, on a forced seat-side state change (a rotation / formfactor
    /// transition / host-key the shell must see but that produced no egui event), or when
    /// the repaint deadline has elapsed — and otherwise goes back to sleep, leaving the
    /// last frame on screen (no idle repaint). This is the switch that makes the shell's
    /// repaint throttling (seat-snapshot pump, VDI frame pacing) actually take effect.
    #[must_use]
    pub const fn should_render(
        first_frame: bool,
        has_input: bool,
        force_render: bool,
        repaint_due: bool,
    ) -> bool {
        first_frame || has_input || force_render || repaint_due
    }

    /// The DRM framebuffer cached for a GBM buffer-object (keyed by its stable `gbm_bo`
    /// pointer), inserting one via `make` on a miss. The GBM scanout surface hands out a
    /// small fixed ring of buffer-objects whose `gbm_bo` pointers are stable across
    /// lock/release cycles, so this attaches exactly one framebuffer per buffer-object
    /// and reuses it for every page-flip — the standard BO-userdata idiom — instead of
    /// add/rm-framebuffer every frame (perf-1). A `make` error is not cached, so the
    /// next frame retries.
    pub fn cached_or_try_insert<F: Copy, E>(
        cache: &mut Vec<(usize, F)>,
        key: usize,
        make: impl FnOnce() -> Result<F, E>,
    ) -> Result<F, E> {
        if let Some(&(_, fb)) = cache.iter().find(|&&(k, _)| k == key) {
            return Ok(fb);
        }
        let fb = make()?;
        cache.push((key, fb));
        Ok(fb)
    }

    #[cfg(test)]
    mod tests {
        use super::{
            cached_or_try_insert, poll_timeout_ms, repaint_deadline, repaint_deadline_at,
            should_render,
        };
        use crate::motion::{AnimatedScalar, Motion, MotionMode, MotionPreset};
        use std::time::{Duration, Instant};

        fn motion_frame(
            ctx: &egui::Context,
            target: f32,
            preset: MotionPreset,
            mode: MotionMode,
            time: f64,
        ) -> (Duration, AnimatedScalar) {
            let mut animated = None;
            let out = ctx.run(
                egui::RawInput {
                    time: Some(time),
                    ..Default::default()
                },
                |ctx| {
                    animated = Some(Motion::animate_typed_with_mode(
                        ctx,
                        "motion-drm-3",
                        target,
                        preset,
                        mode,
                    ));
                },
            );
            let repaint_delay = out
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .expect("root viewport output")
                .repaint_delay;
            (
                repaint_delay,
                animated.expect("motion driver returned a value"),
            )
        }

        fn motion_frame_many(
            ctx: &egui::Context,
            target: f32,
            mode: MotionMode,
            time: f64,
        ) -> (Duration, [AnimatedScalar; 7]) {
            const PRESETS: [MotionPreset; 7] = [
                MotionPreset::Control,
                MotionPreset::Panel,
                MotionPreset::Popover,
                MotionPreset::Dialog,
                MotionPreset::Page,
                MotionPreset::Layout,
                MotionPreset::DragSettle,
            ];

            let mut animated = [AnimatedScalar::settled(0.0); 7];
            let out = ctx.run(
                egui::RawInput {
                    time: Some(time),
                    ..Default::default()
                },
                |ctx| {
                    for (slot, preset) in PRESETS.into_iter().enumerate() {
                        animated[slot] = Motion::animate_typed_with_mode(
                            ctx,
                            ("motion-drm-6-many", slot),
                            target,
                            preset,
                            mode,
                        );
                    }
                },
            );
            let repaint_delay = out
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .expect("root viewport output")
                .repaint_delay;
            (repaint_delay, animated)
        }

        #[test]
        fn idle_blocks_indefinitely() {
            // egui idle + no periodic task ⇒ poll blocks forever (CPU→0), never spins.
            // This is the perf-1 keystone: the idle path must NOT be a bounded busy wait.
            assert_eq!(poll_timeout_ms(None, None), -1);
        }

        #[test]
        fn max_delay_is_idle() {
            assert_eq!(repaint_deadline(Duration::MAX), None);
        }

        #[test]
        fn zero_delay_polls_without_blocking() {
            // Continuous repaint (streaming VDI / spinner): due now, don't block.
            assert_eq!(repaint_deadline(Duration::ZERO), Some(Duration::ZERO));
            assert_eq!(poll_timeout_ms(Some(Duration::ZERO), None), 0);
        }

        #[test]
        fn absolute_deadline_preserves_idle_now_and_finite_delays() {
            let now = Instant::now();
            assert_eq!(repaint_deadline_at(Duration::MAX, now), None);
            assert_eq!(repaint_deadline_at(Duration::ZERO, now), Some(now));
            assert_eq!(
                repaint_deadline_at(Duration::from_millis(16), now),
                now.checked_add(Duration::from_millis(16))
            );
        }

        #[test]
        fn finite_delay_waits_that_long() {
            assert_eq!(
                repaint_deadline(Duration::from_millis(16)),
                Some(Duration::from_millis(16))
            );
            assert_eq!(poll_timeout_ms(Some(Duration::from_millis(16)), None), 16);
        }

        #[test]
        fn periodic_bounds_an_idle_ui() {
            // Idle UI but an accelerometer sample is due in 200 ms: wake to service it
            // (auto-rotate keeps working) without ever rendering while idle.
            assert_eq!(poll_timeout_ms(None, Some(Duration::from_millis(200))), 200);
        }

        #[test]
        fn soonest_deadline_wins() {
            assert_eq!(
                poll_timeout_ms(
                    Some(Duration::from_millis(16)),
                    Some(Duration::from_millis(200))
                ),
                16
            );
            assert_eq!(
                poll_timeout_ms(
                    Some(Duration::from_millis(500)),
                    Some(Duration::from_millis(33))
                ),
                33
            );
        }

        #[test]
        fn sub_millisecond_rounds_up_no_spin() {
            // A 500 µs deadline must round up to 1 ms, never collapse to a 0-ms busy poll.
            assert_eq!(poll_timeout_ms(Some(Duration::from_micros(500)), None), 1);
        }

        #[test]
        fn finite_wait_is_capped() {
            assert_eq!(poll_timeout_ms(Some(Duration::from_secs(3600)), None), 1000);
        }

        #[test]
        fn render_policy() {
            // Idle: nothing to do ⇒ don't render.
            assert!(!should_render(false, false, false, false));
            // Any single trigger forces an immediate render.
            assert!(should_render(true, false, false, false)); // first frame / modeset
            assert!(should_render(false, true, false, false)); // input event
            assert!(should_render(false, false, true, false)); // rotation/formfactor/host-key
            assert!(should_render(false, false, false, true)); // repaint deadline elapsed
        }

        #[test]
        fn motion_drm_3_active_animation_requests_immediate_repaint() {
            let ctx = egui::Context::default();
            let _ = motion_frame(&ctx, 0.0, MotionPreset::Control, MotionMode::Normal, 0.0);

            let (repaint_delay, animated) = motion_frame(
                &ctx,
                1.0,
                MotionPreset::Control,
                MotionMode::Normal,
                1.0 / 60.0,
            );

            assert!(!animated.is_settled(), "changed target should be active");
            assert_eq!(
                repaint_delay,
                Duration::ZERO,
                "active shared motion must keep the DRM loop warm"
            );
            let now = Instant::now();
            let repaint_at = repaint_deadline_at(repaint_delay, now);
            assert_eq!(repaint_at, Some(now));
            assert_eq!(
                poll_timeout_ms(repaint_at.map(|t| t.saturating_duration_since(now)), None),
                0
            );
            assert!(should_render(false, false, false, true));
        }

        #[test]
        fn motion_drm_3_settled_animation_allows_idle_sleep() {
            let ctx = egui::Context::default();
            let _ = motion_frame(&ctx, 0.0, MotionPreset::Control, MotionMode::Normal, 0.0);

            let mut repaint_delay = Duration::ZERO;
            let mut animated = AnimatedScalar::settled(0.0);
            for frame in 1..20 {
                (repaint_delay, animated) = motion_frame(
                    &ctx,
                    1.0,
                    MotionPreset::Control,
                    MotionMode::Normal,
                    f64::from(frame) / 60.0,
                );
            }

            assert!(animated.is_settled(), "animation should be at rest");
            assert_eq!(
                repaint_delay,
                Duration::MAX,
                "settled shared motion must stop continuous repainting"
            );
            assert_eq!(repaint_deadline_at(repaint_delay, Instant::now()), None);
            assert_eq!(poll_timeout_ms(None, None), -1);
            assert!(!should_render(false, false, false, false));
        }

        #[test]
        fn motion_drm_3_delayed_page_flip_uses_clamped_motion_dt() {
            let ctx = egui::Context::default();
            let _ = motion_frame(&ctx, 0.0, MotionPreset::Page, MotionMode::Normal, 0.0);
            let (_, first_active) = motion_frame(
                &ctx,
                100.0,
                MotionPreset::Page,
                MotionMode::Normal,
                1.0 / 60.0,
            );

            let (repaint_delay, after_stall) =
                motion_frame(&ctx, 100.0, MotionPreset::Page, MotionMode::Normal, 5.0);

            let max_expected_elapsed = first_active.elapsed() + (1.0 / 30.0) + f32::EPSILON;
            assert!(
                after_stall.elapsed() <= max_expected_elapsed,
                "delayed flip advanced by {}, expected <= {max_expected_elapsed}",
                after_stall.elapsed()
            );
            assert!(
                !after_stall.is_settled(),
                "a delayed page flip must not jump directly to the endpoint"
            );
            assert_eq!(
                repaint_delay,
                Duration::ZERO,
                "still-active motion keeps scheduling immediate frames"
            );
        }

        #[test]
        fn motion_drm_6_simultaneous_motion_settles_and_returns_to_idle() {
            let ctx = egui::Context::default();
            let _ = motion_frame_many(&ctx, 0.0, MotionMode::Normal, 0.0);

            let (active_delay, active) =
                motion_frame_many(&ctx, 100.0, MotionMode::Normal, 1.0 / 120.0);
            assert_eq!(
                active_delay,
                Duration::ZERO,
                "simultaneous active motion keeps the DRM loop warm"
            );
            assert!(
                active.iter().any(|value| !value.is_settled()),
                "changed targets should put at least one preset in flight"
            );

            let mut repaint_delay = active_delay;
            let mut values = active;
            for frame in 2..80 {
                (repaint_delay, values) =
                    motion_frame_many(&ctx, 100.0, MotionMode::Normal, f64::from(frame) / 120.0);
            }

            assert!(
                values.iter().all(|value| value.is_settled()),
                "all simultaneous preset carriers should settle"
            );
            assert!(
                values
                    .iter()
                    .all(|value| value.value().is_finite() && value.target().is_finite()),
                "simultaneous motion must not produce non-finite values"
            );
            assert_eq!(
                repaint_delay,
                Duration::MAX,
                "settled simultaneous motion must let the DRM loop return to idle"
            );
            assert_eq!(repaint_deadline_at(repaint_delay, Instant::now()), None);
            assert_eq!(poll_timeout_ms(None, None), -1);
        }

        #[test]
        fn fb_cache_adds_once_per_bo() {
            // The framebuffer is created once per buffer-object and reused thereafter;
            // a second lookup for the same BO must NOT call `make` again.
            let mut cache: Vec<(usize, u32)> = Vec::new();
            let mut calls = 0u32;
            let mut mk = |fb: u32| -> Result<u32, ()> {
                calls += 1;
                Ok(fb)
            };
            // First BO (key 0xA): miss → inserts fb 100.
            assert_eq!(cached_or_try_insert(&mut cache, 0xA, || mk(100)), Ok(100));
            // Same BO again: hit → returns 100 WITHOUT calling make (999 never used).
            assert_eq!(cached_or_try_insert(&mut cache, 0xA, || mk(999)), Ok(100));
            // A different BO (key 0xB): miss → inserts fb 200.
            assert_eq!(cached_or_try_insert(&mut cache, 0xB, || mk(200)), Ok(200));
            // make() ran exactly twice (the two distinct BOs), not three times.
            assert_eq!(calls, 2);
            assert_eq!(cache.len(), 2);
        }

        #[test]
        fn fb_cache_does_not_cache_errors() {
            let mut cache: Vec<(usize, u32)> = Vec::new();
            let err = cached_or_try_insert::<u32, &str>(&mut cache, 0xC, || Err("boom"));
            assert_eq!(err, Err("boom"));
            assert!(cache.is_empty());
            // A subsequent success for the same key inserts cleanly.
            assert_eq!(
                cached_or_try_insert(&mut cache, 0xC, || Ok::<u32, &str>(7)),
                Ok(7)
            );
            assert_eq!(cache.len(), 1);
        }
    }
}

/// How often the bare-seat loop wakes to sample the accelerometer while otherwise idle
/// (only when a sensor is present) so auto-rotate keeps working without pinning the CPU.
const ACCEL_SAMPLE_INTERVAL: Duration = Duration::from_millis(200);

/// How often the loop wakes to advance time-driven touch gestures (the long-press fire)
/// while a finger is down, so a held-still finger still long-presses even with no new
/// contact events.
const GESTURE_TICK_INTERVAL: Duration = Duration::from_millis(33);

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

    // a11y-01: the runtime AccessKit consumer seam. On a seat that opts in (MDE_A11Y=1,
    // the same env idiom as MDE_DRM_ESC_QUIT below), this turns on egui's AccessKit tree
    // generation and drains each rendered frame's tree into the sink — so the shipped
    // bare-DRM seat actually exports an accessibility tree instead of only doing so from
    // #[cfg(test)]. Default OFF (unset), and a genuine no-op when the crate is built
    // without the `accesskit` feature. a11y-02's screen reader replaces the default sink
    // via the same [`crate::a11y::AccessKitSink`] seam.
    let mut a11y = crate::a11y::A11yBridge::from_env();
    a11y.enable(&egui_ctx);

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
    // The previous frame's scanout buffer, released back to the surface ring only after
    // the next flip completes (GBM hands out a small ring of buffers). Its framebuffer is
    // NOT tracked here — it lives in `fb_cache`, keyed by the buffer-object, and is reused
    // across flips instead of being rebuilt every frame (perf-1).
    let mut prev: Option<gbm::BufferObject<()>> = None;
    // perf-1: one DRM framebuffer per GBM buffer-object, keyed by its stable `gbm_bo`
    // pointer. The surface ring is a handful of buffers, so this stays tiny; destroyed en
    // masse at teardown.
    let mut fb_cache: Vec<(usize, drm::control::framebuffer::Handle)> = Vec::new();
    // The framebuffer pixel geometry is fixed for the surface's chosen format — compute it
    // once, not per frame.
    let (fb_depth, fb_bpp) = match gbm_format {
        gbm::Format::Xrgb2101010 | gbm::Format::Argb2101010 => (30u32, 32u32),
        gbm::Format::Rgb565 | gbm::Format::Bgr565 => (16, 16),
        _ => (24, 32),
    };
    // perf-1 event-driven wake state: when egui next needs to paint (`None` = idle, egui
    // asked for no repaint). Seeded to "now" so the first iteration renders immediately
    // (the modeset frame) without blocking in poll.
    let mut next_repaint_at: Option<Instant> = Some(Instant::now());
    // Count of down touch contacts — while > 0 the loop wakes on a short cadence so a
    // held-still finger's long-press still fires (`gestures.tick`) with no new events.
    let mut touch_active: u32 = 0;
    let mut quit = false;
    // Esc is a normal key on a shipped desktop — it must NEVER tear the seat down
    // (any dialog/field owns it). Quitting the DRM session on Esc is a dev-only
    // escape hatch, opt-in via `MDE_DRM_ESC_QUIT`; production leaves it unset so the
    // desktop survives Esc and forwards it to egui like any other key.
    let esc_quits = std::env::var_os("MDE_DRM_ESC_QUIT").is_some();
    // Modifier state: updated on each KeyDown/KeyUp before feeding egui Key events.
    let mut shift = false;
    let mut ctrl = false;
    let mut alt = false;

    // SURFACE-8 (lock 13): the touchscreen shares this one input pipeline. The
    // translator maps libinput's normalized multitouch contacts through the active
    // mode + fractional scale + rotation into egui touch + synthesized-pointer events;
    // rotation starts at None here (SURFACE-9 drives it live via `set_rotation`). The
    // pure transform + translation are unit-tested in `crate::touch`; this is the live
    // libinput read that only a real seat exercises (the farm compiles it, no
    // touchscreen — the honest hardware gate).
    let mut touch = TouchTranslator::new(TouchTransform::new(wp, hp, ppp));

    // SURFACE-11 (lock 16): fold the SAME multitouch contact stream into gestures —
    // two-finger scroll, pinch-zoom, long-press → secondary click, and edge-swipes.
    // It shares `touch`'s transform (never re-deriving coordinates, §6); its outputs
    // become egui scroll/zoom/secondary-click events, and edge-swipes are pushed on the
    // seat→shell side channel the shell drains to reveal the dock/tablet bar.
    let mut gestures =
        crate::gestures::GestureRecognizer::new(crate::gestures::GestureConfig::default());
    let mut gesture_out: Vec<crate::gestures::Gesture> = Vec::new();

    // SURFACE-9 (locks 9 + 15): formfactor signal + auto-rotation, both driven off the
    // seat's live evdev/iio streams and the shared pure cores in `crate::formfactor`.
    //
    // Formfactor: `SW_TABLET_MODE` (libinput switch) + Type-Cover attach/detach
    // (keyboard-capable device add/remove) fold into a debounced Tablet/Laptop, pushed
    // on the side channel the shell republishes to the mesh Bus. We start assuming no
    // cover (Tablet); libinput replays every existing device as `Added` on the first
    // dispatch, so a present cover settles to Laptop within the debounce window.
    let mut kbd_count: u32 = 0;
    let mut switches = SwitchState {
        tablet_mode: false,
        cover_attached: false,
    };
    let mut formfactor = FormfactorDebounce::new(switches.raw_formfactor());
    push_formfactor(formfactor.current()); // a startup baseline for the shell.

    // Auto-rotation: the iio accelerometer (real sysfs reads; `None` + honestly inert
    // on a host without one) folds into an orientation, debounced into a KMS rotation
    // that drives BOTH the scanout plane and the touch matrix as one. The live plane
    // rotation is discovered once; when the driver exposes none it degrades honestly.
    let mut auto = AutoRotate::new();
    let mut accel = SysfsAccel::discover().ok();
    let rot_prop = discover_rotation_prop(&gbm, heads[0].crtc);
    let mut last_accel = std::time::Instant::now();

    while !quit {
        // 0. perf-1 WAKE GATE — block until there is a reason to wake, instead of
        //    spinning at refresh rate. Sleep until: input is readable on the libinput
        //    fd, egui's repaint deadline expires, or a periodic task (accel sample /
        //    touch long-press tick) is due. When egui is idle and nothing periodic is
        //    pending, this blocks indefinitely and the CPU reaches ~0 (eframe's
        //    ControlFlow::Wait). The present path below still paces rendering to vblank.
        let now = Instant::now();
        let until_repaint = next_repaint_at.map(|t| t.saturating_duration_since(now));
        let until_accel = accel
            .as_ref()
            .map(|_| ACCEL_SAMPLE_INTERVAL.saturating_sub(last_accel.elapsed()));
        let until_touch = (touch_active > 0).then_some(GESTURE_TICK_INTERVAL);
        let until_periodic = match (until_accel, until_touch) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let timeout = wake::poll_timeout_ms(until_repaint, until_periodic);
        let mut pfd = libc::pollfd {
            fd: libinput.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `pfd` is a single fully-initialised pollfd over a valid, owned fd
        // (libinput's epoll fd, live for the loop). poll only reads `events` and writes
        // `revents`, and blocks up to `timeout` ms. Its result is advisory — every wake
        // reason is re-derived below — so an error/EINTR wake is harmless (one idle pass).
        let _ = unsafe { libc::poll(&mut pfd as *mut libc::pollfd, 1, timeout) };

        // A seat-side state change this iteration that the shell must see promptly even
        // though it produced no egui input event — a display rotation (scanout changed),
        // a formfactor transition (tablet/laptop → the shell's dock/tablet bar), or a
        // host-key press (Super/media keys the shell dispatches). Any of these forces a
        // render so the idle-gate below can't strand it until the next input.
        let mut force_render = false;

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
                // SURFACE-8 (lock 13): touchscreen contacts. libinput reports a
                // per-contact slot + a position transformed into the *unrotated* panel
                // pixel space (`x_transformed(wp)`); normalise it and let the shared
                // translator apply the mode/scale/rotation transform + synthesize the
                // single-touch pointer. Down/Motion carry a position; Up/Cancel do not
                // (the translator reuses the contact's last position). Frame events
                // (contact-set boundaries) don't map to an egui event.
                LiEvent::Touch(te) => {
                    // A device without slots reports None → fall back to the seat slot
                    // so a single-touch panel still tracks one coherent contact.
                    let slot_of = |s: Option<u32>, seat: u32| s.unwrap_or(seat);
                    // Normalise a libinput panel-pixel coordinate to [0,1] for the
                    // mode/scale/rotation transform. The casts are bounded (display
                    // spans < ~16k px, the position lies within the panel), so the
                    // f64→f32 / u32→f32 narrowing is exact here.
                    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
                    let norm = |pos: f64, span: u32| (pos as f32 / span as f32).clamp(0.0, 1.0);
                    let contact = match te {
                        TouchEvent::Down(d) => {
                            // A finger went down — keep the loop waking on a short cadence
                            // so a held-still long-press still fires (perf-1).
                            touch_active += 1;
                            Some(RawContact::Down {
                                slot: slot_of(d.slot(), d.seat_slot()),
                                u: norm(d.x_transformed(wp), wp),
                                v: norm(d.y_transformed(hp), hp),
                                force: None,
                            })
                        }
                        TouchEvent::Motion(m) => Some(RawContact::Move {
                            slot: slot_of(m.slot(), m.seat_slot()),
                            u: norm(m.x_transformed(wp), wp),
                            v: norm(m.y_transformed(hp), hp),
                            force: None,
                        }),
                        TouchEvent::Up(u) => {
                            touch_active = touch_active.saturating_sub(1);
                            Some(RawContact::Up {
                                slot: slot_of(u.slot(), u.seat_slot()),
                            })
                        }
                        TouchEvent::Cancel(c) => {
                            touch_active = touch_active.saturating_sub(1);
                            Some(RawContact::Cancel {
                                slot: slot_of(c.slot(), c.seat_slot()),
                            })
                        }
                        _ => None,
                    };
                    if let Some(contact) = contact {
                        // Keep the software cursor in step with a single-finger tap so
                        // the drawn crosshair follows touch too.
                        if let RawContact::Down { u, v, .. } | RawContact::Move { u, v, .. } =
                            contact
                        {
                            pointer = touch.transform().to_points(u, v);
                        }
                        touch.feed(contact, &mut events);
                        // SURFACE-11: the same contact also folds into the gesture
                        // recognizer (multitouch scroll/zoom/long-press/edge-swipe),
                        // over the identical transform so gestures track the display.
                        gestures.feed(
                            contact,
                            touch.transform(),
                            start.elapsed(),
                            &mut gesture_out,
                        );
                    }
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
                    if code == 1 && pressed && esc_quits {
                        quit = true; // ESC exits the DRM shell — dev-only (MDE_DRM_ESC_QUIT)
                    }
                    // E12-19 (lock 8): the XF86 media/system keys + the Super leader
                    // have no egui::Key, so forward the host-relevant scancodes on
                    // the host-key side channel. The shell drains them each frame and
                    // dispatches them host-first (even over a fullscreen guest); they
                    // are NOT emitted as egui events, so the guest never sees them.
                    if crate::hostkeys::is_host_key(code) {
                        crate::hostkeys::push_host_key(code, pressed);
                        // Host keys aren't forwarded to egui, so force a render for the
                        // shell to dispatch them (launcher/media keys) even when idle.
                        force_render = true;
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
                // SURFACE-9 (lock 9): the SW_TABLET_MODE switch → formfactor. A folded
                // lid puts the device in the touch-first posture regardless of the cover.
                LiEvent::Switch(SwitchEvent::Toggle(t)) => {
                    if t.switch() == Some(Switch::TabletMode) {
                        switches.tablet_mode = t.switch_state() == LiSwitchState::On;
                        if let Some(f) = formfactor.observe(switches.raw_formfactor()) {
                            push_formfactor(f);
                            force_render = true;
                        }
                    }
                }
                // SURFACE-9 (lock 9): Type-Cover attach/detach shows up as a
                // keyboard-capable device add/remove; a detached cover → Tablet.
                LiEvent::Device(dev_ev) => {
                    let added_dev = match dev_ev {
                        DeviceEvent::Added(e) => Some((true, e.device())),
                        DeviceEvent::Removed(e) => Some((false, e.device())),
                        _ => None,
                    };
                    if let Some((added, device)) = added_dev {
                        if device.has_capability(DeviceCapability::Keyboard) {
                            kbd_count = if added {
                                kbd_count + 1
                            } else {
                                kbd_count.saturating_sub(1)
                            };
                            switches.cover_attached = kbd_count > 0;
                            if let Some(f) = formfactor.observe(switches.raw_formfactor()) {
                                push_formfactor(f);
                                force_render = true;
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // SURFACE-11 (lock 16): advance the time-driven gestures (a finger held still
        // long-presses without any new contact event), then translate every recognized
        // gesture into egui input: two-finger scroll → a wheel delta, pinch → a zoom,
        // long-press → a synthesized secondary (right) click, and edge-swipes onto the
        // shell side channel (the dock/tablet-bar reveal).
        gestures.tick(start.elapsed(), &mut gesture_out);
        for gesture in &gesture_out {
            match *gesture {
                crate::gestures::Gesture::Scroll(delta) => {
                    events.push(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta,
                        modifiers: egui::Modifiers::default(),
                    });
                }
                crate::gestures::Gesture::Zoom(factor) => {
                    events.push(egui::Event::Zoom(factor));
                }
                crate::gestures::Gesture::SecondaryClick(pos) => {
                    events.push(egui::Event::PointerMoved(pos));
                    for pressed in [true, false] {
                        events.push(egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Secondary,
                            pressed,
                            modifiers: egui::Modifiers::default(),
                        });
                    }
                }
                crate::gestures::Gesture::EdgeSwipe(edge) => {
                    crate::gestures::push_edge_swipe(edge);
                }
            }
        }
        gesture_out.clear();

        // SURFACE-9 (lock 15): drain the shell's rotation commands (Config tab /
        // hotkey) — a lock freezes auto-rotate, a manual override forces + holds an
        // orientation, applied to BOTH scanout + touch immediately.
        let rotation_cmds = take_rotation_commands();
        if !rotation_cmds.is_empty() {
            // A shell-side rotation command arrived — force a render so the scanout /
            // lock state is reflected even if egui was otherwise idle.
            force_render = true;
        }
        for cmd in rotation_cmds {
            match cmd {
                RotateCommand::Lock(locked) => auto.set_user_lock(locked),
                RotateCommand::Manual(rotation) => {
                    let applied = auto.apply_manual(rotation);
                    drive_rotation(&gbm, rot_prop, &mut touch, applied);
                }
            }
        }

        // SURFACE-9 (lock 15): sample the accelerometer a few times a second (not every
        // frame — it is a sysfs read and the orientation is slow), fold it to an
        // orientation, and on a debounced change rotate display + touch as one. Inert +
        // honest when there is no sensor (`accel` is `None`) or auto-rotate is locked.
        if last_accel.elapsed() >= ACCEL_SAMPLE_INTERVAL {
            last_accel = Instant::now();
            if let Some(sensor) = accel.as_mut() {
                if let Ok((ax, ay, az)) = sensor.read() {
                    if let Some(rotation) = auto.observe(orientation_from_accel(ax, ay, az)) {
                        drive_rotation(&gbm, rot_prop, &mut touch, rotation);
                        force_render = true;
                    }
                }
            }
        }

        // 2. decide whether this iteration has anything to present (perf-1). Render on
        //    the first frame (the modeset), on any input, on a forced seat-side state
        //    change (rotation / formfactor / host-key), or when egui's repaint deadline
        //    has elapsed; otherwise fall through to the next poll WITHOUT rendering,
        //    leaving the last frame on screen. This is what makes the shell's
        //    request_repaint_after throttling (seat-snapshot pump, VDI frame pacing)
        //    actually take effect. Input is never dropped: `events` is only discarded when
        //    it is empty (a non-empty `events` forces a render here).
        // a11y-01: let the AccessKit consumer wake a render so the exported tree can't go
        // stale while the loop is idle — the accessibility analogue of the seat-side
        // force_render above (an AT client connecting, or the reader requesting a
        // re-scan). A no-op unless AccessKit is enabled and the sink asked for a refresh.
        if a11y.wants_render() {
            force_render = true;
        }

        let first_frame = prev.is_none();
        let repaint_due = next_repaint_at.is_some_and(|t| Instant::now() >= t);
        if !wake::should_render(first_frame, !events.is_empty(), force_render, repaint_due) {
            continue;
        }

        // 3. run + paint the egui frame
        let raw_input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(start.elapsed().as_secs_f64()),
            events,
            ..Default::default()
        };
        let cur = pointer;
        let mut full_output = egui_ctx.run(raw_input, |ctx| {
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
        // a11y-01: hand this frame's AccessKit tree to the consumer seam (a no-op unless
        // AccessKit is enabled). Done before `shapes` / `textures_delta` are consumed
        // below; it only takes `platform_output.accesskit_update`, leaving the rest of
        // `full_output` intact.
        a11y.drain(&mut full_output);
        // eframe's contract: egui reports how long until it next needs to paint via the
        // root viewport's `repaint_delay` (`Duration::MAX` == idle). Arm the next wake
        // from it (perf-1) — this is what gives request_repaint_after teeth. Read before
        // `shapes` is moved into `tessellate` below. A missing ROOT viewport (never
        // happens in practice) falls back to ZERO, i.e. the old always-repaint behavior.
        let repaint_delay = full_output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(Duration::ZERO, |vo| vo.repaint_delay);
        next_repaint_at = wake::repaint_deadline_at(repaint_delay, Instant::now());
        let clipped = egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        painter.paint_and_update_textures(
            [wp, hp],
            full_output.pixels_per_point,
            &clipped,
            &full_output.textures_delta,
        );
        egl.swap_buffers(display, surface).map_err(egl_err)?;

        // 4. scan the new front buffer out — set_crtc on the first frame, page-flip
        //    after (waiting for the flip to complete before recycling buffers).
        let bo = unsafe {
            gbm_surface
                .lock_front_buffer()
                .map_err(|e| DrmError::Present(format!("lock_front_buffer: {e}")))?
        };
        // perf-1: reuse the framebuffer already built for this buffer-object (the surface
        // recycles a small ring of BOs; their `gbm_bo` pointers are stable), rather than
        // add_framebuffer/rm_framebuffer every frame. The FB stays valid for the BO's
        // lifetime and is destroyed with the rest at teardown.
        let fb = wake::cached_or_try_insert(&mut fb_cache, bo.as_raw() as usize, || {
            gbm.add_framebuffer(&bo, fb_depth, fb_bpp)
                .map_err(|e| DrmError::Present(format!("add_framebuffer: {e}")))
        })?;
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
        // Release the previous scanout buffer back to the surface ring now that the flip
        // to the new one has completed. Its framebuffer stays cached (keyed by the BO) for
        // when the ring hands the same buffer-object back — no per-frame FB churn (perf-1).
        if let Some(prev_bo) = prev.take() {
            drop(prev_bo);
        }
        prev = Some(bo);
    }

    // teardown (best-effort; the OS reclaims the rest on exit)
    if let Some(bo) = prev.take() {
        drop(bo);
    }
    // Destroy the framebuffers cached across the surface's buffer-object ring (perf-1).
    for (_, fb) in fb_cache {
        let _ = gbm.destroy_framebuffer(fb);
    }
    painter.destroy();
    Ok(())
}

// ── MEDIA-2: the live overlay video-plane wiring (hardware-gated) ─────────────
//
// The pure plane-selection + geometry seam lives in [`crate::video_plane`] and is unit-
// tested against a fake catalog. This is its live half: it reads REAL DRM planes (type
// / zpos / possible_crtcs) into a [`PlaneSet`] so `plan_video` can pick a hardware
// overlay plane beneath the egui shell, and drives KMS `set_plane` to scan a
// framebuffer out on it. It compiles on the farm (behind `feature = "drm"`) but only
// *presents* on a real seat — the same honest posture as [`run_drm`]; a headless host
// degrades cleanly to [`DrmError::NoDrmMaster`] and the caller uses the render-to-
// texture fallback ([`VideoPath::texture_no_drm`]).

/// Read a named property's raw value off `plane`, if the driver exposes it.
fn plane_prop_value(dev: &gbm::Device<Card>, plane: plane::Handle, name: &str) -> Option<u64> {
    let props = dev.get_properties(plane).ok()?;
    for (&handle, &value) in props.iter() {
        if let Ok(info) = dev.get_property(handle) {
            if info.name().to_str() == Ok(name) {
                return Some(value);
            }
        }
    }
    None
}

/// Read a named property's *handle* off `plane` (to `set_property` it later).
fn plane_prop_handle(
    dev: &gbm::Device<Card>,
    plane: plane::Handle,
    name: &str,
) -> Option<property::Handle> {
    let props = dev.get_properties(plane).ok()?;
    let (handles, _values) = props.as_props_and_values();
    for &handle in handles {
        if let Ok(info) = dev.get_property(handle) {
            if info.name().to_str() == Ok(name) {
                return Some(handle);
            }
        }
    }
    None
}

/// Enumerate this card's planes into a [`PlaneSet`] for the CRTC `crtc`.
///
/// Reads each plane's `type` (→ [`PlaneKind`]), `zpos`, and `possible_crtcs`, and
/// identifies the primary plane bound to `crtc` as the egui plane (excluded from video,
/// ordered above the video plane).
///
/// The live enumerate half of the MEDIA-2 seam. On a headless host `plane_handles`
/// yields nothing (or the primary only), so [`PlaneSet::plan_video`] returns the
/// render-to-texture fallback — no faked plane.
///
/// # Errors
/// [`VideoPlaneError::Enumerate`] when the KMS resource / plane list cannot be read, or
/// the target `crtc` is not in the card's CRTC list.
pub fn probe_video_plane(
    dev: &gbm::Device<Card>,
    crtc: crtc::Handle,
) -> Result<PlaneSet, VideoPlaneError> {
    let res = dev
        .resource_handles()
        .map_err(|e| VideoPlaneError::Enumerate(format!("resource_handles: {e}")))?;
    // A plane's `possible_crtcs` bit N addresses the N-th CRTC in this list, so the
    // target CRTC's list index IS its bit position — the pure seam's `crtc_index`.
    let crtc_index =
        u32::try_from(res.crtcs().iter().position(|&c| c == crtc).ok_or_else(|| {
            VideoPlaneError::Enumerate("target CRTC not in resource list".into())
        })?)
        .map_err(|_| VideoPlaneError::Enumerate("CRTC index out of range".into()))?;

    let handles = dev
        .plane_handles()
        .map_err(|e| VideoPlaneError::Enumerate(format!("plane_handles: {e}")))?;

    let crtc_list = res.crtcs();
    let mut planes: Vec<PlaneInfo> = Vec::new();
    let mut egui_plane_id = 0u32;
    let mut egui_zpos = None;
    for ph in handles {
        let Ok(info) = dev.get_plane(ph) else {
            continue;
        };
        let id = u32::from(ph);
        let kind =
            plane_prop_value(dev, ph, "type").map_or(PlaneKind::Unknown, PlaneKind::from_drm_type);
        let zpos = plane_prop_value(dev, ph, "zpos");
        // Rebuild the possible-CRTC bitmask in terms of this list's indices (the `drm`
        // crate hides the raw mask; `filter_crtcs` gives the handles it addresses).
        let possible_crtcs = res
            .filter_crtcs(info.possible_crtcs())
            .iter()
            .fold(0u32, |acc, c| match crtc_list.iter().position(|x| x == c) {
                Some(idx) if idx < 32 => acc | (1u32 << idx),
                _ => acc,
            });
        // The egui shell scans out through the primary plane bound to this CRTC.
        if kind == PlaneKind::Primary && info.crtc() == Some(crtc) {
            egui_plane_id = id;
            egui_zpos = zpos;
        }
        planes.push(PlaneInfo {
            id,
            kind,
            possible_crtcs,
            zpos,
        });
    }

    Ok(PlaneSet {
        planes,
        egui_plane_id,
        egui_zpos,
        crtc_index,
    })
}

/// The live [`VideoScanout`]: drives KMS `set_plane` to scan a decoded framebuffer out
/// on the chosen overlay plane beneath the egui shell (or clear it).
///
/// Its `Frame` is a real `drm` framebuffer handle — the mpv render API imports the
/// decoded frame as a dmabuf/KMS framebuffer (MEDIA-3/4/8) and hands the handle here.
///
/// Compiles on the farm; only commits on a real seat (the hardware-gated live scanout,
/// the same posture as [`run_drm`]). The pure geometry + selection it consumes is unit-
/// tested via `crate::video_plane`'s `RecordingScanout`.
pub struct DrmVideoScanout<'a> {
    dev: &'a gbm::Device<Card>,
    crtc: crtc::Handle,
}

impl<'a> DrmVideoScanout<'a> {
    /// A live scanout that composites the video plane onto `crtc` via `dev`.
    #[must_use]
    pub const fn new(dev: &'a gbm::Device<Card>, crtc: crtc::Handle) -> Self {
        Self { dev, crtc }
    }

    /// Reconstruct a `drm` plane handle from the pure seam's `u32` plane id.
    fn plane_handle(id: u32) -> Result<plane::Handle, VideoPlaneError> {
        drm::control::from_u32::<plane::Handle>(id)
            .ok_or_else(|| VideoPlaneError::Commit(format!("invalid plane id {id}")))
    }
}

impl VideoScanout for DrmVideoScanout<'_> {
    type Frame = drm::control::framebuffer::Handle;

    fn present(
        &mut self,
        frame: Self::Frame,
        plan: &VideoPlanePlan,
    ) -> Result<(), VideoPlaneError> {
        let plane = Self::plane_handle(plan.plane_id)?;
        let Some(placement) = plan.placement else {
            // Nothing visible this frame → detach the plane rather than program a
            // degenerate rect.
            return self.clear(plan.plane_id);
        };
        // Order the video plane below the egui plane where the driver lets us. An
        // immutable `zpos` (no property, or a read-only one) is honestly left as the
        // driver's fixed ordering — never faked.
        if let Some(z) = plan.zpos {
            if let Some(prop) = plane_prop_handle(self.dev, plane, "zpos") {
                let _ = self.dev.set_property(plane, prop, z);
            }
        }
        self.dev
            .set_plane(
                plane,
                self.crtc,
                Some(frame),
                0,
                placement.crtc_rect,
                placement.src_rect_16_16,
            )
            .map_err(|e| VideoPlaneError::Commit(format!("set_plane: {e}")))
    }

    fn clear(&mut self, plane_id: u32) -> Result<(), VideoPlaneError> {
        let plane = Self::plane_handle(plane_id)?;
        self.dev
            .set_plane(plane, self.crtc, None, 0, (0, 0, 0, 0), (0, 0, 0, 0))
            .map_err(|e| VideoPlaneError::Commit(format!("clear set_plane: {e}")))
    }
}

/// Bring the DRM seat up far enough to enumerate its planes and resolve the MEDIA-2
/// video-plane decision for the primary output.
///
/// The hardware-gated live surface for the overlay seam (driven by the
/// `hello_video_plane` example).
///
/// Returns the enumerated [`PlaneSet`] and the resolved [`VideoPath`] for a `video`
/// frame of `(w, h)` shown in `pane`. When an overlay plane is chosen it also drives a
/// live `clear` on it (a harmless liveness check that the plane detaches) — the actual
/// video-frame present is the mpv-gated leg (MEDIA-3/4/8), so this proves the plane
/// path end-to-end short of a decoded frame.
///
/// # Errors
/// [`DrmError::NoDrmMaster`] on a headless host (the caller then uses the render-to-
/// texture fallback), or the other [`DrmError`]s when the seat cannot be brought up.
pub fn probe_primary_video_plane(
    video: (u32, u32),
    pane: PaneRect,
) -> Result<(PlaneSet, VideoPath), DrmError> {
    let (_node, file) = open_primary_node()?;
    let card = Card(file);
    let outputs = resolve_outputs(&card)?;
    let crtc = outputs[0].crtc;
    let (w, h) = outputs[0].mode.size();
    let screen = (u32::from(w), u32::from(h));

    let gbm = gbm::Device::new(card).map_err(|e| DrmError::Gbm(format!("gbm device: {e}")))?;
    let set = probe_video_plane(&gbm, crtc).map_err(|e| DrmError::Present(e.to_string()))?;
    let path = set.plan_video(video, pane, screen);

    if let VideoPath::Overlay(plan) = &path {
        let mut scanout = DrmVideoScanout::new(&gbm, crtc);
        if let Err(e) = scanout.clear(plan.plane_id) {
            eprintln!(
                "mde-egui: video-plane liveness clear failed ({e}); plane path still resolved"
            );
        }
    }
    Ok((set, path))
}

// ── QC-23 Tier 1: PRIME-import liveness check (hardware-gated, no QEMU) ───────
//
// `docs/design/qc23-virtio-gpu-zerocopy-rescope.md` §5 Tier 1. The shell-side half
// of a real dmabuf importer (Option B, §3.5) is `prime_fd_to_buffer` (fd → GEM
// handle) → `add_planar_framebuffer` (GEM handle → KMS framebuffer); this codebase
// has never actually exercised either call. The hard, still-unsolved half (§3.3) is
// getting a dmabuf fd OUT of QEMU in the first place — this check is entirely
// independent of that. It round-trips a **locally-allocated** GBM buffer through
// this project's own export/import primitives (`buffer_to_prime_fd` →
// `prime_fd_to_buffer`) with no QEMU involvement at all, proving the *shell-side*
// half of the mechanism actually works on real hardware. Mirrors
// `probe_primary_video_plane`'s own precedent above: a harmless liveness probe
// (built, then immediately torn down) rather than a real QEMU-sourced frame.

/// A single-plane [`buffer::PlanarBuffer`] built from a **re-imported** GEM handle.
/// `prime_fd_to_buffer` only returns a bare handle — unlike a `gbm::BufferObject`
/// (which already implements `PlanarBuffer` directly for its OWN handle, e.g. the
/// scanout buffers [`run_drm`] already builds), a handle re-imported from a raw fd
/// carries no size/format/pitch of its own, so whatever protocol handed over the fd
/// must supply that metadata out-of-band. Here it comes from the same
/// locally-allocated buffer whose fd [`probe_prime_import_liveness`] round-trips (a
/// real cross-process handoff — the still-blocked §3.3 half — would carry it some
/// other way, e.g. the virtio-gpu resource's own create parameters). Single-plane
/// only (planes 1-3 empty): every format this project's virtio-gpu path would
/// actually scan out (XRGB8888 et al.) is single-plane; multi-planar YUV formats
/// are out of scope here (§3.5's own shader-risk note already flags them as a
/// separate, format-dependent follow-on).
#[derive(Debug, Clone, Copy)]
struct ReimportedGemBuffer {
    /// Buffer width, px (from the original local allocation).
    width: u32,
    /// Buffer height, px (from the original local allocation).
    height: u32,
    /// Pixel format (from the original local allocation).
    format: gbm::Format,
    /// Explicit modifier, or `None` to let `add_planar_framebuffer` omit
    /// [`FbCmd2Flags::MODIFIERS`] (see [`explicit_modifier`]).
    modifier: Option<gbm::Modifier>,
    /// Bytes per scanline (from the original local allocation).
    pitch: u32,
    /// The freshly re-imported GEM handle — NOT the original buffer's own handle.
    handle: buffer::Handle,
}

impl buffer::PlanarBuffer for ReimportedGemBuffer {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn format(&self) -> gbm::Format {
        self.format
    }

    fn modifier(&self) -> Option<gbm::Modifier> {
        self.modifier
    }

    fn pitches(&self) -> [u32; 4] {
        [self.pitch, 0, 0, 0]
    }

    fn handles(&self) -> [Option<buffer::Handle>; 4] {
        [Some(self.handle), None, None, None]
    }

    fn offsets(&self) -> [u32; 4] {
        [0, 0, 0, 0]
    }
}

/// `Some(modifier)` unless `modifier` is GBM's driver-implicit/unspecified
/// sentinel (`DrmModifier::Invalid`) — mirrors the exact filter
/// `drm::control::Device::add_planar_framebuffer`'s own implementation applies to
/// a [`buffer::PlanarBuffer::modifier`] before asserting it against
/// [`FbCmd2Flags::MODIFIERS`] (`drm` 0.14.1 `src/control/mod.rs:356-360`), so
/// [`probe_prime_import_liveness`] doesn't have to duplicate that invariant
/// inline — get it wrong and `add_planar_framebuffer` panics instead of
/// returning a clean `Result`. `DrmModifier` (aka [`gbm::Modifier`]) has no
/// `PartialEq` impl, so this matches the same `drm`-crate source's own idiom
/// (`!matches!(modifier, DrmModifier::Invalid)`) rather than `!=`.
fn explicit_modifier(modifier: gbm::Modifier) -> Option<gbm::Modifier> {
    if matches!(modifier, gbm::Modifier::Invalid) {
        None
    } else {
        Some(modifier)
    }
}

/// The outcome of [`probe_prime_import_liveness`]: proof the re-imported prime fd
/// was accepted as a real KMS framebuffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrimeImportLiveness {
    /// The (immediately torn down) framebuffer's KMS object id — evidence
    /// `add_planar_framebuffer` accepted the re-imported handle.
    pub framebuffer_id: u32,
}

/// QC-23 Tier 1 (`docs/design/qc23-virtio-gpu-zerocopy-rescope.md` §5): round-trip
/// a **locally-allocated** GBM buffer through this project's own PRIME import
/// primitives, with no QEMU involvement at all.
///
/// Allocates a small scanout-capable GBM buffer, exports its dmabuf fd
/// (`buffer_to_prime_fd`), re-imports that fd as a fresh GEM handle
/// (`prime_fd_to_buffer` — the same call a real QEMU-delivered dmabuf fd would go
/// through), wraps it in a [`ReimportedGemBuffer`] carrying the metadata a real
/// handoff would need to supply out-of-band, and builds a KMS framebuffer from it
/// (`add_planar_framebuffer`) — then immediately tears it down
/// (`destroy_framebuffer`). This is a liveness probe, not a real present — it
/// never touches a plane/CRTC at all (plane selection + `set_plane` is already
/// proven by MEDIA-2's [`probe_primary_video_plane`] above; this proves only the
/// import primitives Option B (§3.5) still needed exercising), mirroring
/// `probe_primary_video_plane`'s own choice of a harmless `clear()` over a real
/// frame.
///
/// # Errors
/// [`DrmError::NoDrmMaster`] on a headless host (no seat to probe at all — the
/// same degrade every other live check in this module uses), or the other
/// [`DrmError`] variants when the GBM/KMS calls themselves fail.
pub fn probe_prime_import_liveness() -> Result<PrimeImportLiveness, DrmError> {
    let (_node, file) = open_primary_node()?;
    let card = Card(file);
    let gbm = gbm::Device::new(card).map_err(|e| DrmError::Gbm(format!("gbm device: {e}")))?;

    // A small scanout-capable buffer — its pixel content is never read or
    // written; this proves the *handle* plumbing, not a rendered frame.
    let bo = gbm
        .create_buffer_object::<()>(
            64,
            64,
            gbm::Format::Xrgb8888,
            gbm::BufferObjectFlags::SCANOUT,
        )
        .map_err(|e| DrmError::Gbm(format!("create_buffer_object: {e}")))?;
    let (width, height) = (bo.width(), bo.height());
    let pitch = bo.stride();
    let modifier = explicit_modifier(bo.modifier());

    // Export → re-import: the exact round trip the shell side of a real
    // cross-process dmabuf handoff (QEMU → here, once §3.3 is solved) would need,
    // exercised here against a buffer this same process already owns.
    let local_handle = buffer::Buffer::handle(&bo);
    let fd = gbm
        .buffer_to_prime_fd(local_handle, 0)
        .map_err(|e| DrmError::Present(format!("buffer_to_prime_fd: {e}")))?;
    let reimported_handle = gbm
        .prime_fd_to_buffer(fd.as_fd())
        .map_err(|e| DrmError::Present(format!("prime_fd_to_buffer: {e}")))?;

    let planar = ReimportedGemBuffer {
        width,
        height,
        format: gbm::Format::Xrgb8888,
        modifier,
        pitch,
        handle: reimported_handle,
    };
    let flags = if modifier.is_some() {
        FbCmd2Flags::MODIFIERS
    } else {
        FbCmd2Flags::empty()
    };
    let fb = gbm
        .add_planar_framebuffer(&planar, flags)
        .map_err(|e| DrmError::Present(format!("add_planar_framebuffer: {e}")))?;
    let framebuffer_id = u32::from(fb);
    let _ = gbm.destroy_framebuffer(fb);

    Ok(PrimeImportLiveness { framebuffer_id })
}

#[cfg(test)]
mod tests {
    use super::{
        explicit_modifier, open_primary_node, probe_prime_import_liveness, DrmError,
        ReimportedGemBuffer,
    };
    use drm::buffer::{Handle as GemHandle, PlanarBuffer};

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

    // ── QC-23 Tier 1: PRIME-import liveness ──

    #[test]
    fn reimported_gem_buffer_maps_a_single_plane() {
        // Pure mapping check — no hardware: a synthetic handle stands in for
        // whatever prime_fd_to_buffer would really return on a live seat.
        let handle = drm::control::from_u32::<GemHandle>(7).expect("nonzero handle");
        let planar = ReimportedGemBuffer {
            width: 64,
            height: 64,
            format: gbm::Format::Xrgb8888,
            modifier: Some(gbm::Modifier::Linear),
            pitch: 256,
            handle,
        };
        assert_eq!(planar.size(), (64, 64));
        assert_eq!(planar.format(), gbm::Format::Xrgb8888);
        assert_eq!(planar.pitches(), [256, 0, 0, 0]);
        assert_eq!(planar.offsets(), [0, 0, 0, 0]);
        let handles = planar.handles();
        assert_eq!(handles[0], Some(handle));
        assert!(handles[1..].iter().all(Option::is_none));
        assert!(matches!(planar.modifier(), Some(gbm::Modifier::Linear)));
    }

    #[test]
    fn explicit_modifier_filters_the_invalid_sentinel() {
        // The Invalid sentinel (GBM's "driver picked an implicit modifier, don't
        // ask" value) must map to None, or add_planar_framebuffer's own
        // has_modifier/modifier.is_some() assert would panic instead of a clean
        // Result — see explicit_modifier's doc comment.
        assert!(explicit_modifier(gbm::Modifier::Invalid).is_none());
        assert!(matches!(
            explicit_modifier(gbm::Modifier::Linear),
            Some(gbm::Modifier::Linear)
        ));
    }

    #[test]
    fn prime_import_liveness_degrades_cleanly() {
        // Same total/never-panic contract as headless_degrades_cleanly above: a
        // headless host (the farm/CI case) must return the clean NoDrmMaster
        // fallback; a dev box with a GPU may instead return Ok.
        match probe_prime_import_liveness() {
            Ok(_) => {}
            Err(DrmError::NoDrmMaster(_)) => {}
            Err(other) => panic!("expected a clean NoDrmMaster fallback, got {other:?}"),
        }
    }
}
