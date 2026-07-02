//! `formfactor` — SURFACE-9: the 2-in-1 **formfactor signal + auto-rotation** core
//! (design `docs/design/surface-tablet-enablement.md`, locks 9 + 15).
//!
//! A convertible (Surface Pro/Book) is two machines in one chassis: a **laptop**
//! when the Type Cover is attached and flat, a **tablet** when it is detached or
//! folded back (`SW_TABLET_MODE`). And held in the hand it is rotated freely, so the
//! display **and the touch matrix** must follow the accelerometer as one. This module
//! is the **pure, headless-testable core** of both reactions:
//!
//! 1. **Formfactor** — a debounced [`FormfactorDebounce`] state machine folds the
//!    `SW_TABLET_MODE` switch + Type-Cover attach/detach into a stable
//!    [`Formfactor`] (Laptop / Tablet). The seat feeds raw switch/device edges; a
//!    confirmed flip is pushed on the [side channel](push_formfactor) the shell drains
//!    and republishes to the mesh Bus as `event/hardware/formfactor` (§6: this shared
//!    crate never touches the Bus itself — same seam idiom as [`crate::hostkeys`]).
//! 2. **Auto-rotation** — [`orientation_from_accel`] folds an iio accelerometer
//!    gravity vector into an [`Orientation`], and [`AutoRotate`] debounces it into a
//!    display [`Rotation`], honouring a **rotation lock** (user toggle, a hardware
//!    `SW_ROTATE_LOCK`, or a sticky manual override). A confirmed rotation is applied
//!    to **both** the KMS scanout ([`RotationApply`]) and the touch matrix
//!    ([`crate::touch::TouchTranslator::set_rotation`]) with the *same* value by
//!    [`apply_rotation`], so display + touch rotate together and taps land correctly.
//!
//! **Honest hardware gate (§7).** The pure folds + state machines are exercised by
//! ordinary unit tests. The live reads (sysfs accelerometer, libinput switch) and the
//! live KMS rotate are integration-gated: [`SysfsAccel`] returns a typed
//! [`SensorError`] when no iio accelerometer exists (a farm/CI host has none), and the
//! [`RotationApply`] seam's headless arm ([`HeadlessRotate`]) returns a typed
//! [`RotateError::Unsupported`] rather than pretending the panel turned.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use crate::touch::{Rotation, TouchTranslator};

/// The device's physical form: a laptop or a tablet.
///
/// Laptop = Type Cover attached + flat; Tablet = cover detached or folded back so
/// `SW_TABLET_MODE` asserts. Drives the shell's tablet-mode UX (OSK auto-raise, touch
/// density) via the mesh Bus signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Formfactor {
    /// Type Cover attached and in the flat typing position — the desktop UX.
    Laptop,
    /// Cover detached or folded back (`SW_TABLET_MODE`) — the touch-first UX.
    Tablet,
}

impl Formfactor {
    /// The stable wire token the shell publishes on `event/hardware/formfactor`
    /// (`"laptop"` / `"tablet"`). Kept here so the seat and the shell agree on it.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Laptop => "laptop",
            Self::Tablet => "tablet",
        }
    }
}

/// The raw switch/device state the seat reads each edge: the kernel `SW_TABLET_MODE`
/// switch and whether a Type-Cover keyboard is currently enumerated.
///
/// The *derived* formfactor is Tablet when the tablet switch asserts **or** the cover
/// is detached — either alone puts the device in the touch-first posture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SwitchState {
    /// `SW_TABLET_MODE` — the lid folded back past the laptop hinge range.
    pub tablet_mode: bool,
    /// A Type-Cover (keyboard-capable) device is enumerated on the seat.
    pub cover_attached: bool,
}

impl SwitchState {
    /// Fold the raw switches into the formfactor they imply (before debouncing):
    /// Tablet if the tablet switch is set **or** the cover is gone.
    #[must_use]
    pub const fn raw_formfactor(self) -> Formfactor {
        if self.tablet_mode || !self.cover_attached {
            Formfactor::Tablet
        } else {
            Formfactor::Laptop
        }
    }
}

/// Debounces the raw [`SwitchState`]-derived formfactor so a bouncing hinge switch or
/// a momentary cover re-enumeration doesn't flap the shell's whole UX.
///
/// A candidate different from the committed value must be observed
/// [`stable_needed`](Self::with_threshold) consecutive samples before it commits.
#[derive(Debug, Clone)]
pub struct FormfactorDebounce {
    committed: Formfactor,
    pending: Option<(Formfactor, u32)>,
    stable_needed: u32,
}

impl FormfactorDebounce {
    /// A debouncer starting from an initial (assumed-settled) formfactor, requiring
    /// three agreeing samples before a change commits.
    #[must_use]
    pub const fn new(initial: Formfactor) -> Self {
        Self::with_threshold(initial, 3)
    }

    /// As [`new`](Self::new) but with an explicit stability threshold (samples that
    /// must agree). A threshold of `0` or `1` commits on the first differing sample.
    #[must_use]
    pub const fn with_threshold(initial: Formfactor, stable_needed: u32) -> Self {
        Self {
            committed: initial,
            pending: None,
            stable_needed,
        }
    }

    /// The currently committed (debounced) formfactor.
    #[must_use]
    pub const fn current(&self) -> Formfactor {
        self.committed
    }

    /// Feed one raw sample; return `Some(new)` only when a *change* commits.
    ///
    /// A sample equal to the committed value clears any pending candidate; a differing
    /// one is counted, and once it reaches the threshold it commits and is returned.
    pub fn observe(&mut self, raw: Formfactor) -> Option<Formfactor> {
        if raw == self.committed {
            self.pending = None;
            return None;
        }
        let count = match self.pending {
            Some((f, n)) if f == raw => n + 1,
            _ => 1,
        };
        if count >= self.stable_needed.max(1) {
            self.committed = raw;
            self.pending = None;
            Some(raw)
        } else {
            self.pending = Some((raw, count));
            None
        }
    }
}

/// The display's physical orientation as read from the accelerometer, named by the
/// [`Rotation`] the shell must apply to keep content upright.
///
/// **Axis convention** (the [`SysfsAccel`] seam applies the panel's iio mount matrix
/// first, so the fold sees a normalised frame): `+x` = the panel's right edge, `+y` =
/// the panel's top edge, `+z` = out of the screen toward the viewer, gravity in
/// g-units. Upright landscape ⇒ gravity toward the bottom edge, `g ≈ (0, -1, 0)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// Upright landscape — no rotation. Gravity along `-y`.
    Normal,
    /// The panel's right edge is down (held rotated 90° counter-clockwise); content
    /// rotates 90° clockwise to stay upright ([`Rotation::Rotate90`], xrandr `right`).
    /// Gravity along `+x`.
    PortraitRight,
    /// Upside-down landscape ([`Rotation::Rotate180`], xrandr `inverted`). Gravity
    /// along `+y`.
    Inverted,
    /// The panel's left edge is down (held rotated 90° clockwise); content rotates
    /// 90° counter-clockwise ([`Rotation::Rotate270`], xrandr `left`). Gravity along
    /// `-x`.
    PortraitLeft,
}

impl Orientation {
    /// The display [`Rotation`] that keeps content upright in this orientation. The
    /// same value is driven into the touch matrix so taps land correctly (lock 15).
    #[must_use]
    pub const fn rotation(self) -> Rotation {
        match self {
            Self::Normal => Rotation::None,
            Self::PortraitRight => Rotation::Rotate90,
            Self::Inverted => Rotation::Rotate180,
            Self::PortraitLeft => Rotation::Rotate270,
        }
    }
}

/// Fold a gravity vector `(x, y, z)` (g-units, panel frame — see [`Orientation`]) into
/// the orientation it implies, or `None` when the device is too flat to tell.
///
/// A pure decision with a **dead zone**: if the in-plane gravity component
/// `√(x²+y²)` is below [`FLAT_THRESHOLD_G`] the panel is lying flat (face up/down) and
/// the orientation is ambiguous, so we return `None` and hold the last rotation — the
/// same behaviour iio-sensor-proxy uses to avoid spinning the screen on a desk. When
/// tilted, the dominant in-plane axis picks the quadrant.
#[must_use]
pub fn orientation_from_accel(x: f32, y: f32, z: f32) -> Option<Orientation> {
    // The out-of-plane component (z) is ignored for the quadrant; only the in-plane
    // gravity direction names the orientation. Guard against NaN from a bad read.
    if !x.is_finite() || !y.is_finite() || !z.is_finite() {
        return None;
    }
    let in_plane = x.hypot(y);
    if in_plane < FLAT_THRESHOLD_G {
        return None;
    }
    // The dominant in-plane axis + its sign names the quadrant.
    if x.abs() >= y.abs() {
        if x >= 0.0 {
            Some(Orientation::PortraitRight)
        } else {
            Some(Orientation::PortraitLeft)
        }
    } else if y <= 0.0 {
        Some(Orientation::Normal)
    } else {
        Some(Orientation::Inverted)
    }
}

/// Below this in-plane gravity magnitude (g) the panel is treated as lying flat and
/// its orientation as ambiguous (≈30° of tilt is required to commit a rotation).
pub const FLAT_THRESHOLD_G: f32 = 0.5;

/// The auto-rotation controller: debounces accelerometer orientations into a display
/// [`Rotation`], honouring a rotation **lock** (user toggle, a hardware
/// `SW_ROTATE_LOCK`, or a sticky manual override).
///
/// It decides *what* the rotation should be; the seat applies the decision to both
/// the display and the touch matrix via [`apply_rotation`] so the two never desync.
#[derive(Debug, Clone)]
pub struct AutoRotate {
    current: Rotation,
    pending: Option<(Orientation, u32)>,
    stable_needed: u32,
    user_lock: bool,
    hw_lock: bool,
    manual_lock: bool,
}

impl Default for AutoRotate {
    fn default() -> Self {
        Self::new()
    }
}

impl AutoRotate {
    /// A controller starting unrotated and unlocked, requiring three agreeing accel
    /// samples before a rotation commits (the accelerometer is noisy).
    #[must_use]
    pub const fn new() -> Self {
        Self::with_threshold(3)
    }

    /// As [`new`](Self::new) with an explicit accel-stability threshold.
    #[must_use]
    pub const fn with_threshold(stable_needed: u32) -> Self {
        Self {
            current: Rotation::None,
            pending: None,
            stable_needed,
            user_lock: false,
            hw_lock: false,
            manual_lock: false,
        }
    }

    /// The current committed display rotation.
    #[must_use]
    pub const fn current(&self) -> Rotation {
        self.current
    }

    /// Whether auto-rotation is currently frozen — by the user toggle, the hardware
    /// rotation-lock switch, or a sticky manual override.
    #[must_use]
    pub const fn is_locked(&self) -> bool {
        self.user_lock || self.hw_lock || self.manual_lock
    }

    /// Set the **user** rotation-lock toggle. Unlocking also clears a sticky manual
    /// override (an explicit unlock means "resume following the sensor").
    pub const fn set_user_lock(&mut self, locked: bool) {
        self.user_lock = locked;
        if !locked {
            self.manual_lock = false;
        }
    }

    /// Track a **hardware** rotation-lock switch (`SW_ROTATE_LOCK`) if the device has
    /// one. Independent of the user toggle; either freezes auto-rotation.
    pub const fn set_hw_lock(&mut self, locked: bool) {
        self.hw_lock = locked;
    }

    /// Apply a **manual** rotation override, freezing at it until the user unlock
    /// clears the manual lock. Returns the new current rotation (the seat applies it
    /// to display + touch). A manual choice always wins over the sensor.
    pub const fn apply_manual(&mut self, rotation: Rotation) -> Rotation {
        self.current = rotation;
        self.manual_lock = true;
        self.pending = None;
        rotation
    }

    /// Feed one accelerometer-derived orientation; return `Some(rot)` only when a
    /// rotation *change* commits (the seat then applies it to display + touch).
    ///
    /// Locked ⇒ always `None`. A `None` orientation (flat/ambiguous) clears the
    /// pending candidate and holds. A differing orientation must be observed
    /// [`stable_needed`](Self::with_threshold) consecutive samples before it commits.
    pub fn observe(&mut self, orientation: Option<Orientation>) -> Option<Rotation> {
        if self.is_locked() {
            self.pending = None;
            return None;
        }
        let Some(o) = orientation else {
            self.pending = None;
            return None;
        };
        if o.rotation() == self.current {
            self.pending = None;
            return None;
        }
        let count = match self.pending {
            Some((p, n)) if p == o => n + 1,
            _ => 1,
        };
        if count >= self.stable_needed.max(1) {
            self.current = o.rotation();
            self.pending = None;
            Some(self.current)
        } else {
            self.pending = Some((o, count));
            None
        }
    }
}

/// Why a live KMS rotate could not be applied. The seat treats any variant as "the
/// panel did not turn" and leaves the touch matrix unrotated so the two stay in sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RotateError {
    /// This scanout path cannot rotate — no DRM plane `rotation` property, an
    /// unsupported driver, or the headless/windowed runner. The honest §7 default.
    Unsupported(String),
    /// The rotate property existed but the KMS commit setting it failed.
    Commit(String),
}

impl std::fmt::Display for RotateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(why) => write!(f, "KMS rotate unsupported: {why}"),
            Self::Commit(why) => write!(f, "KMS rotate commit failed: {why}"),
        }
    }
}

impl std::error::Error for RotateError {}

/// The injectable seam that applies a [`Rotation`] to the **display scanout**.
///
/// The live DRM implementation (the KMS rotate property) lives in [`crate::drm`] behind
/// `feature = "drm"`; tests and headless hosts use [`HeadlessRotate`].
pub trait RotationApply {
    /// Rotate the scanout to `rotation`, or return an honest [`RotateError`] when the
    /// path cannot (headless / unsupported driver) — never faked.
    ///
    /// # Errors
    /// Returns [`RotateError::Unsupported`] when the runner/driver cannot rotate, or
    /// [`RotateError::Commit`] when the KMS commit fails.
    fn apply(&mut self, rotation: Rotation) -> Result<(), RotateError>;
}

/// The headless/windowed KMS seam: every apply is an honest [`RotateError::Unsupported`].
///
/// There is no scanout to rotate, so the seat leaves the touch matrix unrotated too
/// (display + touch stay in sync), and the real rotate is the hardware-gated path.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeadlessRotate;

impl RotationApply for HeadlessRotate {
    fn apply(&mut self, _rotation: Rotation) -> Result<(), RotateError> {
        Err(RotateError::Unsupported(
            "no DRM scanout on this runner".into(),
        ))
    }
}

/// Apply a rotation to **both** the display scanout and the touch matrix with the same
/// value, so they rotate as one and taps keep landing correctly (lock 15).
///
/// The **display rotates first**: if the KMS seam fails (headless / unsupported), the
/// touch matrix is left untouched, so a partial rotate can never desync the two — they
/// always carry the same rotation. On success both carry `rotation`.
///
/// # Errors
/// Propagates the [`RotationApply`] seam's [`RotateError`]; on error the touch matrix
/// is unchanged.
pub fn apply_rotation(
    rotation: Rotation,
    display: &mut impl RotationApply,
    touch: &mut TouchTranslator,
) -> Result<(), RotateError> {
    display.apply(rotation)?;
    touch.set_rotation(rotation);
    Ok(())
}

/// Read an iio accelerometer's gravity vector. The live seam is [`SysfsAccel`]; tests
/// inject a fake. Returns g-units in the panel frame (mount matrix already applied).
pub trait AccelSensor {
    /// Read the current gravity vector `(x, y, z)` in g-units, or a typed error when no
    /// accelerometer is present / readable.
    ///
    /// # Errors
    /// Returns a [`SensorError`] when the iio device is absent or a read/parse fails.
    fn read(&mut self) -> Result<(f32, f32, f32), SensorError>;
}

/// Why an accelerometer read failed. `Absent` is the honest headless default (a farm /
/// CI host has no iio accelerometer); the shell reports auto-rotate as unavailable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SensorError {
    /// No iio accelerometer device was found on this host.
    Absent(String),
    /// The device exists but a channel read / scale parse failed.
    Read(String),
}

impl std::fmt::Display for SensorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Absent(why) => write!(f, "no accelerometer: {why}"),
            Self::Read(why) => write!(f, "accelerometer read failed: {why}"),
        }
    }
}

impl std::error::Error for SensorError {}

/// The live sysfs iio accelerometer reader.
///
/// Reads `in_accel_{x,y,z}_raw × in_accel_*_scale` under an `iio:deviceN` directory.
/// Plain file reads (no `unsafe`); it compiles everywhere and simply reports
/// [`SensorError::Absent`] where there is no sensor, so auto-rotation is honestly inert
/// on a non-tablet / headless host (§7).
#[derive(Debug, Clone)]
pub struct SysfsAccel {
    device_dir: PathBuf,
    scale: f32,
}

impl SysfsAccel {
    /// The iio bus root scanned for an accelerometer device.
    const IIO_ROOT: &'static str = "/sys/bus/iio/devices";

    /// Discover the first iio device that exposes `in_accel_x_raw`, reading its shared
    /// `in_accel_scale` (defaulting to 1.0 when a per-channel scale is used instead).
    ///
    /// # Errors
    /// [`SensorError::Absent`] when no iio accelerometer directory is present.
    pub fn discover() -> Result<Self, SensorError> {
        Self::discover_in(Path::new(Self::IIO_ROOT))
    }

    /// [`discover`](Self::discover) rooted at an explicit iio directory (the test seam).
    ///
    /// # Errors
    /// [`SensorError::Absent`] when no `iio:device*` under `root` exposes an accel raw.
    pub fn discover_in(root: &Path) -> Result<Self, SensorError> {
        let entries = std::fs::read_dir(root)
            .map_err(|e| SensorError::Absent(format!("{}: {e}", root.display())))?;
        for entry in entries.flatten() {
            let dir = entry.path();
            if dir.join("in_accel_x_raw").exists() {
                // Prefer the shared scale; fall back to 1.0 (raw already in g-ish units).
                let scale = read_f32(&dir.join("in_accel_scale")).unwrap_or(1.0);
                return Ok(Self {
                    device_dir: dir,
                    scale: if scale.is_finite() && scale != 0.0 {
                        scale
                    } else {
                        1.0
                    },
                });
            }
        }
        Err(SensorError::Absent(format!(
            "no iio accelerometer under {}",
            root.display()
        )))
    }

    fn axis(&self, axis: char) -> Result<f32, SensorError> {
        let raw = read_f32(&self.device_dir.join(format!("in_accel_{axis}_raw")))
            .ok_or_else(|| SensorError::Read(format!("in_accel_{axis}_raw")))?;
        Ok(raw * self.scale)
    }
}

impl AccelSensor for SysfsAccel {
    fn read(&mut self) -> Result<(f32, f32, f32), SensorError> {
        Ok((self.axis('x')?, self.axis('y')?, self.axis('z')?))
    }
}

/// Read a whitespace-trimmed `f32` from a sysfs attribute, or `None` if absent/unparseable.
fn read_f32(path: &Path) -> Option<f32> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<f32>().ok())
}

// --- the seat → shell side channel (formfactor + rotation-lock commands) ------------
//
// Same idiom as `crate::hostkeys`: a process-thread-local hand-off across the
// runner→surface seam (the DRM present loop and the shell render run on one thread).
// The seat pushes a confirmed formfactor flip; the shell drains it once per frame and
// republishes it on the mesh Bus. Inbound, the shell pushes rotation-lock / manual
// commands the seat drains and applies to its `AutoRotate`.

thread_local! {
    /// The latest confirmed formfactor flip not yet drained by the shell (`None` once
    /// drained). Only a debounced *change* is pushed, so the shell publishes sparsely.
    static FORMFACTOR: RefCell<Option<Formfactor>> = const { RefCell::new(None) };
    /// Pending rotation-lock / manual-rotate commands from the shell to the seat.
    static ROTATE_CMDS: RefCell<Vec<RotateCommand>> = const { RefCell::new(Vec::new()) };
}

/// A rotation command the shell (its Config tab / a hotkey) sends to the seat's
/// [`AutoRotate`]: freeze/unfreeze, or force a specific orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotateCommand {
    /// Set the user rotation-lock toggle (freeze at the current orientation).
    Lock(bool),
    /// Force a specific rotation (a sticky manual override until unlocked).
    Manual(Rotation),
}

/// Push a confirmed formfactor flip from the seat to the shell (a no-op-cheap
/// thread-local write). The shell drains it each frame and publishes to the Bus.
pub fn push_formfactor(formfactor: Formfactor) {
    FORMFACTOR.with(|c| *c.borrow_mut() = Some(formfactor));
}

/// Drain the latest formfactor flip, if any (the shell calls this once per frame).
///
/// Returns `Some` only when the formfactor changed since the last drain — so the shell
/// publishes exactly on a transition. On the windowed fallback (no seat) it is always
/// `None`, self-gating the Bus publish to the real DRM seat.
#[must_use]
pub fn drain_formfactor() -> Option<Formfactor> {
    FORMFACTOR.with(|c| c.borrow_mut().take())
}

/// Send a rotation command from the shell to the seat (freeze / manual override).
pub fn request_rotation(command: RotateCommand) {
    ROTATE_CMDS.with(|q| q.borrow_mut().push(command));
}

/// Drain every pending rotation command (the seat calls this once per frame and
/// applies each to its [`AutoRotate`]). Empty on the windowed fallback.
#[must_use]
pub fn take_rotation_commands() -> Vec<RotateCommand> {
    ROTATE_CMDS.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::touch::TouchTransform;

    // --- accel → orientation fold ---------------------------------------------------

    #[test]
    fn accel_maps_each_quadrant_to_its_orientation() {
        // Gravity down each in-plane axis names a quadrant (panel frame convention).
        assert_eq!(
            orientation_from_accel(0.0, -1.0, 0.0),
            Some(Orientation::Normal)
        );
        assert_eq!(
            orientation_from_accel(1.0, 0.0, 0.0),
            Some(Orientation::PortraitRight)
        );
        assert_eq!(
            orientation_from_accel(0.0, 1.0, 0.0),
            Some(Orientation::Inverted)
        );
        assert_eq!(
            orientation_from_accel(-1.0, 0.0, 0.0),
            Some(Orientation::PortraitLeft)
        );
    }

    #[test]
    fn accel_flat_on_a_desk_is_ambiguous() {
        // Face up (g≈+z) or face down (g≈-z): no in-plane component → hold, don't spin.
        assert_eq!(orientation_from_accel(0.0, 0.0, 1.0), None);
        assert_eq!(orientation_from_accel(0.02, -0.03, 0.99), None);
        // A NaN read is ambiguous, never a panic.
        assert_eq!(orientation_from_accel(f32::NAN, 0.0, 0.0), None);
    }

    #[test]
    fn accel_tilt_past_the_dead_zone_commits() {
        // Just inside the dead zone → None; just past it → a quadrant.
        assert_eq!(orientation_from_accel(0.3, 0.3, 0.9), None); // in-plane ≈0.42 < 0.5
        assert!(orientation_from_accel(0.5, 0.1, 0.85).is_some());
    }

    #[test]
    fn orientation_rotation_matches_the_xrandr_convention() {
        assert_eq!(Orientation::Normal.rotation(), Rotation::None);
        assert_eq!(Orientation::PortraitRight.rotation(), Rotation::Rotate90);
        assert_eq!(Orientation::Inverted.rotation(), Rotation::Rotate180);
        assert_eq!(Orientation::PortraitLeft.rotation(), Rotation::Rotate270);
    }

    // --- formfactor debounce --------------------------------------------------------

    #[test]
    fn switch_state_derives_formfactor() {
        // Cover attached + flat → Laptop; tablet switch OR cover gone → Tablet.
        assert_eq!(
            SwitchState {
                tablet_mode: false,
                cover_attached: true
            }
            .raw_formfactor(),
            Formfactor::Laptop
        );
        assert_eq!(
            SwitchState {
                tablet_mode: true,
                cover_attached: true
            }
            .raw_formfactor(),
            Formfactor::Tablet
        );
        assert_eq!(
            SwitchState {
                tablet_mode: false,
                cover_attached: false
            }
            .raw_formfactor(),
            Formfactor::Tablet
        );
    }

    #[test]
    fn formfactor_debounce_requires_stable_samples() {
        let mut d = FormfactorDebounce::with_threshold(Formfactor::Laptop, 3);
        // Two tablet samples: not yet committed (bounce filtered).
        assert_eq!(d.observe(Formfactor::Tablet), None);
        assert_eq!(d.observe(Formfactor::Tablet), None);
        assert_eq!(d.current(), Formfactor::Laptop);
        // Third commits and reports the change exactly once.
        assert_eq!(d.observe(Formfactor::Tablet), Some(Formfactor::Tablet));
        assert_eq!(d.current(), Formfactor::Tablet);
        // A steady state reports no further change.
        assert_eq!(d.observe(Formfactor::Tablet), None);
    }

    #[test]
    fn formfactor_debounce_resets_on_a_flap() {
        let mut d = FormfactorDebounce::with_threshold(Formfactor::Laptop, 3);
        assert_eq!(d.observe(Formfactor::Tablet), None); // 1
        assert_eq!(d.observe(Formfactor::Tablet), None); // 2
        assert_eq!(d.observe(Formfactor::Laptop), None); // back to committed → reset
        assert_eq!(d.observe(Formfactor::Tablet), None); // count restarts at 1
        assert_eq!(d.current(), Formfactor::Laptop, "the flap didn't commit");
    }

    // --- auto-rotate controller -----------------------------------------------------

    #[test]
    fn autorotate_commits_after_stable_orientation() {
        let mut ar = AutoRotate::with_threshold(2);
        assert_eq!(ar.observe(Some(Orientation::PortraitRight)), None);
        assert_eq!(
            ar.observe(Some(Orientation::PortraitRight)),
            Some(Rotation::Rotate90)
        );
        assert_eq!(ar.current(), Rotation::Rotate90);
        // Same orientation again → no repeat change.
        assert_eq!(ar.observe(Some(Orientation::PortraitRight)), None);
    }

    #[test]
    fn autorotate_lock_freezes_orientation() {
        let mut ar = AutoRotate::with_threshold(1);
        ar.set_user_lock(true);
        // Even a decisive orientation change is ignored while locked.
        assert_eq!(ar.observe(Some(Orientation::Inverted)), None);
        assert_eq!(ar.current(), Rotation::None);
        // Unlock resumes following the sensor.
        ar.set_user_lock(false);
        assert_eq!(
            ar.observe(Some(Orientation::Inverted)),
            Some(Rotation::Rotate180)
        );
    }

    #[test]
    fn hardware_lock_also_freezes() {
        let mut ar = AutoRotate::with_threshold(1);
        ar.set_hw_lock(true);
        assert!(ar.is_locked());
        assert_eq!(ar.observe(Some(Orientation::PortraitLeft)), None);
        ar.set_hw_lock(false);
        assert!(!ar.is_locked());
    }

    #[test]
    fn manual_override_wins_and_sticks_until_unlock() {
        let mut ar = AutoRotate::with_threshold(1);
        assert_eq!(ar.apply_manual(Rotation::Rotate270), Rotation::Rotate270);
        assert!(ar.is_locked(), "a manual override freezes auto-rotate");
        // The sensor cannot override a manual choice.
        assert_eq!(ar.observe(Some(Orientation::Normal)), None);
        assert_eq!(ar.current(), Rotation::Rotate270);
        // An explicit user unlock clears the manual lock and resumes auto: the sensor
        // now rotates the display away from the stuck manual value.
        ar.set_user_lock(false);
        assert!(!ar.is_locked());
        assert_eq!(
            ar.observe(Some(Orientation::Normal)),
            Some(Rotation::None),
            "auto-rotate resumes after the manual lock is released"
        );
    }

    #[test]
    fn autorotate_ambiguous_reading_holds() {
        let mut ar = AutoRotate::with_threshold(1);
        // A flat/None reading never rotates.
        assert_eq!(ar.observe(None), None);
        assert_eq!(ar.current(), Rotation::None);
    }

    // --- display + touch stay in sync ----------------------------------------------

    /// A fake KMS seam recording the last rotation it was asked to apply, and whether
    /// the next apply should fail (the headless case).
    struct FakeKms {
        last: Option<Rotation>,
        fail: bool,
    }
    impl RotationApply for FakeKms {
        fn apply(&mut self, rotation: Rotation) -> Result<(), RotateError> {
            if self.fail {
                return Err(RotateError::Unsupported("test".into()));
            }
            self.last = Some(rotation);
            Ok(())
        }
    }

    #[test]
    fn apply_rotation_drives_display_and_touch_with_the_same_value() {
        let mut kms = FakeKms {
            last: None,
            fail: false,
        };
        let mut touch = TouchTranslator::new(TouchTransform::new(2000, 1000, 1.0));
        apply_rotation(Rotation::Rotate90, &mut kms, &mut touch).expect("applied");
        // BOTH carry the identical rotation — display + touch rotate as one.
        assert_eq!(kms.last, Some(Rotation::Rotate90));
        assert_eq!(touch.transform().rotation, Rotation::Rotate90);
        assert_eq!(
            kms.last.expect("display rotated"),
            touch.transform().rotation,
            "display rotation and set_rotation argument must match"
        );
    }

    #[test]
    fn failed_kms_leaves_touch_unrotated_so_they_never_desync() {
        let mut kms = FakeKms {
            last: None,
            fail: true,
        };
        let mut touch = TouchTranslator::new(TouchTransform::new(2000, 1000, 1.0));
        // The touch matrix starts at a known rotation; a failed display rotate must
        // leave it there (never rotate touch without the display, or taps mis-land).
        touch.set_rotation(Rotation::Rotate180);
        let err = apply_rotation(Rotation::Rotate90, &mut kms, &mut touch);
        assert!(matches!(err, Err(RotateError::Unsupported(_))));
        assert_eq!(
            touch.transform().rotation,
            Rotation::Rotate180,
            "touch held its rotation when the display couldn't rotate"
        );
    }

    #[test]
    fn headless_rotate_is_honestly_unsupported() {
        let mut h = HeadlessRotate;
        assert!(matches!(
            h.apply(Rotation::Rotate90),
            Err(RotateError::Unsupported(_))
        ));
    }

    // --- sysfs seam + wire tokens ---------------------------------------------------

    #[test]
    fn sysfs_accel_absent_is_honest() {
        // No iio device dir → an honest Absent error, never a faked vector.
        let err = SysfsAccel::discover_in(Path::new("/nonexistent/iio/root"));
        assert!(matches!(err, Err(SensorError::Absent(_))));
    }

    #[test]
    fn sysfs_accel_reads_a_fixture_device() {
        // A synthetic iio device dir with raw channels + a scale reads back scaled.
        let dir = std::env::temp_dir().join(format!(
            "surf9_iio_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let dev = dir.join("iio:device0");
        std::fs::create_dir_all(&dev).expect("mkdir");
        std::fs::write(dev.join("in_accel_x_raw"), "100\n").expect("x");
        std::fs::write(dev.join("in_accel_y_raw"), "-980\n").expect("y");
        std::fs::write(dev.join("in_accel_z_raw"), "20\n").expect("z");
        std::fs::write(dev.join("in_accel_scale"), "0.001\n").expect("scale");

        let mut accel = SysfsAccel::discover_in(&dir).expect("discovers the device");
        let (x, y, z) = accel.read().expect("reads");
        assert!((x - 0.1).abs() < 1e-4, "{x}");
        assert!((y + 0.98).abs() < 1e-4, "{y}");
        assert!((z - 0.02).abs() < 1e-4, "{z}");
        // The scaled vector folds to the upright orientation (gravity ≈ -y).
        assert_eq!(orientation_from_accel(x, y, z), Some(Orientation::Normal));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn formfactor_wire_tokens_are_stable() {
        assert_eq!(Formfactor::Laptop.as_wire(), "laptop");
        assert_eq!(Formfactor::Tablet.as_wire(), "tablet");
    }

    // --- side channel ---------------------------------------------------------------

    #[test]
    fn formfactor_side_channel_carries_the_latest_flip() {
        // Nothing pushed → nothing to publish.
        let _ = drain_formfactor(); // clear any prior test's residue on this thread
        assert_eq!(drain_formfactor(), None);
        push_formfactor(Formfactor::Tablet);
        assert_eq!(drain_formfactor(), Some(Formfactor::Tablet));
        // Drained once — a second drain is empty (publish exactly on the transition).
        assert_eq!(drain_formfactor(), None);
    }

    #[test]
    fn rotation_command_side_channel_round_trips() {
        let _ = take_rotation_commands(); // clear residue
        request_rotation(RotateCommand::Lock(true));
        request_rotation(RotateCommand::Manual(Rotation::Rotate90));
        let cmds = take_rotation_commands();
        assert_eq!(
            cmds,
            vec![
                RotateCommand::Lock(true),
                RotateCommand::Manual(Rotation::Rotate90)
            ]
        );
        assert!(take_rotation_commands().is_empty());
    }
}
