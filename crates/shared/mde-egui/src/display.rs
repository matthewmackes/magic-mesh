//! SURFACE-7 (design locks 11 + 12) — the **display mode + `HiDPI` scale** model for
//! the bare DRM/KMS seat.
//!
//! The DRM runner ([`crate::drm::run_drm`]) owns KMS directly (E12 no-compositor),
//! so it — not a Wayland compositor — must pick the panel's native resolution, a
//! **fractional** egui scale (`pixels_per_point`) from the panel DPI, and offer a
//! **real KMS mode picker** to trade native resolution for HD (fewer pixels for
//! wgpu / VDI streaming). This module is the **pure, headless-testable core** of
//! that: EDID parsing, the DPI→scale fold, mode-list construction, and native/HD
//! selection are ordinary functions with fixtures; the one hardware-bound step —
//! the live KMS modeset — sits behind the injectable [`ModesetSeam`] so headless
//! builds get an **honest typed error** ([`ModesetError::NoDrmMaster`]) and never a
//! faked success (§7).
//!
//! The live DRM path ([`crate::drm`], `feature = "drm"`) builds a [`PanelInfo`] from
//! the real connector and supplies a real seam; this module carries no `drm`
//! dependency and compiles + tests on the headless farm. The Config tab that drives
//! it (SURFACE-6) reads the queryable [`DisplayController`] state.

/// A single scanout mode a connector can drive: a resolution + refresh, and whether
/// it is the connector's **preferred** (native panel) mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelMode {
    /// Horizontal active pixels.
    pub width: u32,
    /// Vertical active pixels.
    pub height: u32,
    /// Refresh rate in **millihertz** (so 59.94 Hz is `59_940`) — integer-exact so
    /// modes dedup/compare cleanly without float wobble.
    pub refresh_mhz: u32,
    /// The connector's preferred / native timing (the EDID first detailed timing).
    pub preferred: bool,
}

impl PanelMode {
    /// Construct a mode from whole-Hz refresh (the common fixture path).
    #[must_use]
    pub const fn new(width: u32, height: u32, refresh_hz: u32, preferred: bool) -> Self {
        Self {
            width,
            height,
            refresh_mhz: refresh_hz.saturating_mul(1000),
            preferred,
        }
    }

    /// Refresh rate in Hz (for display).
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // refresh_mhz < ~240_000; f32 is exact here
    pub fn refresh_hz(&self) -> f32 {
        self.refresh_mhz as f32 / 1000.0
    }

    /// Total pixel count — the cost proxy the HD trade-off minimises.
    #[must_use]
    pub fn pixels(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    /// How this mode reads to the picker: the native timing, exactly 1920×1080, or
    /// any other listed mode.
    #[must_use]
    pub const fn class(&self) -> ModeClass {
        if self.preferred {
            ModeClass::Native
        } else if self.width == HD_WIDTH && self.height == HD_HEIGHT {
            ModeClass::Hd
        } else {
            ModeClass::Other
        }
    }
}

/// 1920×1080 — the "HD" target the picker switches to for fewer pixels.
pub const HD_WIDTH: u32 = 1920;
/// 1920×1080 — the "HD" target the picker switches to for fewer pixels.
pub const HD_HEIGHT: u32 = 1080;

/// The role a mode plays in the picker (lock 12): native, HD (1080p), or other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeClass {
    /// The panel's native/preferred timing.
    Native,
    /// Exactly 1920×1080 — the reduced-pixel HD trade-off.
    Hd,
    /// Any other listed mode.
    Other,
}

/// The detected panel: its physical size, native mode, and the full mode list.
///
/// Built from EDID ([`parse_edid`]) or from the live connector ([`crate::drm`]);
/// the two agree on the same [`PanelMode`] vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelInfo {
    /// Physical active area in millimetres `(width, height)`, `(0, 0)` if the EDID /
    /// connector doesn't report it (then DPI is unknown → scale falls back to 1.0).
    pub phys_mm: (u32, u32),
    /// The native / preferred mode (drives the default framebuffer + the scale).
    pub native: PanelMode,
    /// Every mode the connector advertises, native-first (see [`build_mode_list`]).
    pub modes: Vec<PanelMode>,
}

impl PanelInfo {
    /// Assemble a panel from a native mode, physical size, and the raw connector
    /// modes — deduping + ordering them via [`build_mode_list`].
    #[must_use]
    pub fn new(native: PanelMode, phys_mm: (u32, u32), raw_modes: &[PanelMode]) -> Self {
        let modes = build_mode_list(&native, raw_modes);
        Self {
            phys_mm,
            native,
            modes,
        }
    }

    /// The computed fractional egui scale for this panel (lock 11).
    #[must_use]
    pub fn scale(&self) -> f32 {
        scale_for_panel(self)
    }

    /// The best listed mode for a role, if present (native always is; HD only if the
    /// connector actually advertises 1920×1080 — never fabricated).
    #[must_use]
    pub fn select(&self, class: ModeClass) -> Option<&PanelMode> {
        select_mode(&self.modes, class)
    }
}

// --- EDID parse (lock 11: native detect + physical size) ------------------------

/// The panel facts a single EDID base block yields: the native (first detailed)
/// timing and the physical active-area size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdidPanel {
    /// The native mode from the first detailed timing descriptor (marked preferred).
    pub native: PanelMode,
    /// Physical active area `(width, height)` in millimetres from that descriptor.
    pub phys_mm: (u32, u32),
}

/// Why an EDID blob could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdidError {
    /// Fewer than the 128-byte base block.
    TooShort(usize),
    /// The fixed `00 FF FF FF FF FF FF 00` header didn't match.
    BadHeader,
    /// The first descriptor is a display descriptor (pixel clock 0), not a preferred
    /// timing — no native mode to read.
    NoPreferredTiming,
    /// The parsed timing was degenerate (zero total) — a corrupt block.
    DegenerateTiming,
}

impl std::fmt::Display for EdidError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort(n) => write!(f, "EDID too short: {n} bytes (need 128)"),
            Self::BadHeader => write!(f, "EDID header mismatch"),
            Self::NoPreferredTiming => write!(f, "EDID first descriptor is not a preferred timing"),
            Self::DegenerateTiming => write!(f, "EDID preferred timing has a zero total"),
        }
    }
}

impl std::error::Error for EdidError {}

const EDID_HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];

/// Parse an EDID base block's **first detailed timing descriptor** into the native
/// mode + physical size (lock 11).
///
/// The descriptor at offset 54 packs 12-bit active/blanking counts (low byte + a
/// shared high-nibble byte) and 12-bit mm sizes the same way; refresh is
/// `pixel_clock / (h_total · v_total)`. Only the 128-byte base block is read (CEA
/// extension blocks aren't needed for the native timing).
///
/// # Errors
/// [`EdidError`] for a short/corrupt block or a first descriptor that carries no
/// preferred timing.
pub fn parse_edid(edid: &[u8]) -> Result<EdidPanel, EdidError> {
    if edid.len() < 128 {
        return Err(EdidError::TooShort(edid.len()));
    }
    if edid[0..8] != EDID_HEADER {
        return Err(EdidError::BadHeader);
    }
    // First detailed timing descriptor: 18 bytes at offset 54.
    let d = &edid[54..72];
    let pixel_clock_10khz = u32::from(u16::from_le_bytes([d[0], d[1]]));
    if pixel_clock_10khz == 0 {
        return Err(EdidError::NoPreferredTiming);
    }
    // 12-bit fields: low byte | (high nibble << 8). The shared byte holds the
    // active high nibble in bits 7..4 and the blanking high nibble in bits 3..0.
    let h_active = u32::from(d[2]) | (u32::from(d[4] & 0xF0) << 4);
    let h_blank = u32::from(d[3]) | (u32::from(d[4] & 0x0F) << 8);
    let v_active = u32::from(d[5]) | (u32::from(d[7] & 0xF0) << 4);
    let v_blank = u32::from(d[6]) | (u32::from(d[7] & 0x0F) << 8);
    let h_mm = u32::from(d[12]) | (u32::from(d[14] & 0xF0) << 4);
    let v_mm = u32::from(d[13]) | (u32::from(d[14] & 0x0F) << 8);

    let h_total = h_active + h_blank;
    let v_total = v_active + v_blank;
    if h_total == 0 || v_total == 0 || h_active == 0 || v_active == 0 {
        return Err(EdidError::DegenerateTiming);
    }
    let clock_hz = f64::from(pixel_clock_10khz) * 10_000.0;
    let refresh_hz = clock_hz / (f64::from(h_total) * f64::from(v_total));

    Ok(EdidPanel {
        native: PanelMode {
            width: h_active,
            height: v_active,
            refresh_mhz: round_f64_to_u32(refresh_hz * 1000.0),
            preferred: true,
        },
        phys_mm: (h_mm, v_mm),
    })
}

/// Round a non-negative f64 to u32, saturating — the one lossy conversion, isolated
/// and bounded (refresh in `mHz` is `< ~240_000`).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn round_f64_to_u32(x: f64) -> u32 {
    let r = x.round();
    if r <= 0.0 {
        0
    } else if r >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        r as u32
    }
}

// --- DPI → fractional scale (lock 11) -------------------------------------------

/// DPI that maps to a 1.0 scale.
///
/// Chosen so a ~267-PPI Surface panel lands at ~2.25 (a crisp, correctly-sized
/// fractional `HiDPI`), a 1080p/24″ desktop (~92 PPI) at 1.0, and a 1440p/24″
/// (~122 PPI) at 1.0 — the `HiDPI`-toolkit "logical density" convention, not the
/// legacy 96-DPI 1×.
pub const REFERENCE_DPI: f32 = 120.0;
/// Minimum egui scale — never shrink UI below 1×.
pub const MIN_SCALE: f32 = 1.0;
/// Maximum egui scale — a sane ceiling for absurd DPI reports.
pub const MAX_SCALE: f32 = 3.0;

/// Panel DPI from active pixels + physical mm along one axis, or `None` when the
/// physical size is unknown (mm == 0) — then the caller falls back to a 1.0 scale.
#[must_use]
#[allow(clippy::cast_precision_loss)] // px/mm are small (< 10_000); f32 is exact for a ratio
pub fn panel_dpi(width_px: u32, width_mm: u32) -> Option<f32> {
    if width_mm == 0 || width_px == 0 {
        return None;
    }
    let inches = width_mm as f32 / 25.4;
    Some(width_px as f32 / inches)
}

/// The DPI→scale fold (lock 11): a **fractional** egui `pixels_per_point`.
///
/// Quantised to quarter steps (crisp text-hinting-friendly fractions like
/// 1.25 / 2.0 / 2.25) and clamped to `[MIN_SCALE, MAX_SCALE]`.
#[must_use]
pub fn fractional_scale(dpi: f32) -> f32 {
    let raw = dpi / REFERENCE_DPI;
    let quantised = (raw * 4.0).round() / 4.0;
    quantised.clamp(MIN_SCALE, MAX_SCALE)
}

/// The fractional scale for a whole panel: DPI from the native width + physical
/// width, folded to a scale; `1.0` when the physical size is unknown.
#[must_use]
pub fn scale_for_panel(panel: &PanelInfo) -> f32 {
    panel_dpi(panel.native.width, panel.phys_mm.0).map_or(1.0, fractional_scale)
}

// --- mode-list construction + selection (lock 12) --------------------------------

/// Build the picker's mode list from the native mode + the connector's raw modes.
///
/// Dedup by (w, h, refresh), guarantee the native mode is present + the only one
/// flagged `preferred`, and order it **native-first, then by pixel count
/// descending** (largest → smallest, so the HD trade-off reads as "step down").
///
/// HD is **not fabricated**: a 1920×1080 entry appears only if the connector
/// actually advertises it (real EDID modes), keeping the picker honest.
#[must_use]
pub fn build_mode_list(native: &PanelMode, raw_modes: &[PanelMode]) -> Vec<PanelMode> {
    let mut out: Vec<PanelMode> = Vec::with_capacity(raw_modes.len() + 1);
    // Native first, canonicalised as the sole preferred entry.
    let native = PanelMode {
        preferred: true,
        ..*native
    };
    out.push(native);
    for m in raw_modes {
        let candidate = PanelMode {
            preferred: false,
            ..*m
        };
        // Same geometry+refresh as native (regardless of its preferred flag) is the
        // native entry already in — skip so native isn't duplicated.
        if candidate.width == native.width
            && candidate.height == native.height
            && candidate.refresh_mhz == native.refresh_mhz
        {
            continue;
        }
        if out.iter().any(|e| {
            e.width == candidate.width
                && e.height == candidate.height
                && e.refresh_mhz == candidate.refresh_mhz
        }) {
            continue;
        }
        out.push(candidate);
    }
    // Native stays first; the rest by pixel count desc, then refresh desc, stable.
    out[1..].sort_by(|a, b| {
        b.pixels()
            .cmp(&a.pixels())
            .then(b.refresh_mhz.cmp(&a.refresh_mhz))
    });
    out
}

/// Pick the best listed mode for a role (lock 12).
///
/// The native entry for [`ModeClass::Native`]; for [`ModeClass::Hd`] the
/// highest-refresh 1920×1080; for [`ModeClass::Other`] the largest non-native/non-HD
/// mode. `None` if none matches (e.g. the panel advertises no 1080p) — the picker
/// then omits that choice.
#[must_use]
pub fn select_mode(modes: &[PanelMode], class: ModeClass) -> Option<&PanelMode> {
    match class {
        ModeClass::Native => modes.iter().find(|m| m.preferred),
        ModeClass::Hd => modes
            .iter()
            .filter(|m| !m.preferred && m.width == HD_WIDTH && m.height == HD_HEIGHT)
            .max_by_key(|m| m.refresh_mhz),
        ModeClass::Other => modes
            .iter()
            .filter(|m| m.class() == ModeClass::Other)
            .max_by_key(|m| (m.pixels(), u64::from(m.refresh_mhz))),
    }
}

// --- the injectable modeset seam (lock 12) --------------------------------------

/// Why a KMS modeset could not be applied. Headless builds (no DRM master) get
/// [`ModesetError::NoDrmMaster`] — the honest gated state that is **never** faked as
/// success (§7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModesetError {
    /// No DRM device / master to modeset on (the farm / CI / headless case).
    NoDrmMaster(String),
    /// The requested mode isn't one the connector advertises (not in the mode list).
    UnknownMode {
        /// Requested width.
        width: u32,
        /// Requested height.
        height: u32,
    },
    /// The live KMS `set_crtc`/atomic commit failed on a real seat.
    Kms(String),
}

impl std::fmt::Display for ModesetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDrmMaster(why) => write!(f, "no DRM master for modeset: {why}"),
            Self::UnknownMode { width, height } => {
                write!(
                    f,
                    "mode {width}x{height} is not advertised by the connector"
                )
            }
            Self::Kms(why) => write!(f, "KMS modeset failed: {why}"),
        }
    }
}

impl std::error::Error for ModesetError {}

/// The hardware-bound step, injected so the pure model stays headless-testable.
///
/// A KMS modeset to `mode`: the live implementation ([`crate::drm`]) issues the real
/// `set_crtc`; the headless [`HeadlessModeset`] returns [`ModesetError::NoDrmMaster`].
pub trait ModesetSeam {
    /// Apply `mode` as the active scanout mode.
    ///
    /// # Errors
    /// [`ModesetError`] — `NoDrmMaster` headless, `Kms` on a live failure.
    fn apply(&self, mode: &PanelMode) -> Result<(), ModesetError>;
}

/// The headless seam: every modeset is an honest [`ModesetError::NoDrmMaster`]. This
/// is the farm / CI / windowed-fallback default — a truthful gated state, not a stub
/// that pretends the mode changed.
#[derive(Debug, Default, Clone, Copy)]
pub struct HeadlessModeset;

impl ModesetSeam for HeadlessModeset {
    fn apply(&self, _mode: &PanelMode) -> Result<(), ModesetError> {
        Err(ModesetError::NoDrmMaster(
            "headless seam — no DRM device present".into(),
        ))
    }
}

/// The queryable display state for a live seat: panel, active mode + scale, seam.
///
/// Owns the native↔HD switch (lock 12) and the scale override (lock 11); SURFACE-6's
/// Config tab reads its getters and drives [`set_mode`](Self::set_mode) /
/// [`set_scale_override`](Self::set_scale_override).
pub struct DisplayController {
    panel: PanelInfo,
    active: PanelMode,
    scale_override: Option<f32>,
    seam: Box<dyn ModesetSeam + Send + Sync>,
}

impl DisplayController {
    /// Build a controller for a detected panel + a modeset seam; the active mode
    /// starts at native, the scale at the panel's computed fractional scale.
    #[must_use]
    pub fn new(panel: PanelInfo, seam: Box<dyn ModesetSeam + Send + Sync>) -> Self {
        let active = panel.native;
        Self {
            panel,
            active,
            scale_override: None,
            seam,
        }
    }

    /// A controller with the [`HeadlessModeset`] seam — the windowed-fallback / test
    /// default whose [`set_mode`](Self::set_mode) honestly can't modeset.
    #[must_use]
    pub fn headless(panel: PanelInfo) -> Self {
        Self::new(panel, Box::new(HeadlessModeset))
    }

    /// The detected panel (physical size, native mode, full mode list).
    #[must_use]
    pub const fn panel(&self) -> &PanelInfo {
        &self.panel
    }

    /// Every mode the picker offers (native-first).
    #[must_use]
    pub fn modes(&self) -> &[PanelMode] {
        &self.panel.modes
    }

    /// The currently-active scanout mode.
    #[must_use]
    pub const fn active_mode(&self) -> &PanelMode {
        &self.active
    }

    /// The panel's native/preferred mode.
    #[must_use]
    pub const fn native_mode(&self) -> &PanelMode {
        &self.panel.native
    }

    /// The panel's own computed fractional scale (before any override).
    #[must_use]
    pub fn computed_scale(&self) -> f32 {
        self.panel.scale()
    }

    /// The **effective** egui scale to hand egui: the override if set, else the
    /// computed fractional scale.
    #[must_use]
    pub fn effective_scale(&self) -> f32 {
        self.scale_override.unwrap_or_else(|| self.panel.scale())
    }

    /// The active manual scale override, if any.
    #[must_use]
    pub const fn scale_override(&self) -> Option<f32> {
        self.scale_override
    }

    /// Set (or clear with `None`) a manual scale override, clamped to
    /// `[MIN_SCALE, MAX_SCALE]` — the Config tab's scale slider (lock 11).
    pub fn set_scale_override(&mut self, scale: Option<f32>) {
        self.scale_override = scale.map(|s| s.clamp(MIN_SCALE, MAX_SCALE));
    }

    /// Switch to a specific advertised mode via the seam (lock 12). The target must
    /// be one of [`modes`](Self::modes); a live modeset that succeeds updates the
    /// active mode, a headless seam leaves it unchanged and surfaces the honest
    /// [`ModesetError::NoDrmMaster`].
    ///
    /// # Errors
    /// [`ModesetError::UnknownMode`] if `target` isn't advertised; whatever the seam
    /// returns otherwise (`NoDrmMaster` headless, `Kms` on a live failure).
    pub fn set_mode(&mut self, target: &PanelMode) -> Result<(), ModesetError> {
        let matched = self.panel.modes.iter().find(|m| {
            m.width == target.width
                && m.height == target.height
                && m.refresh_mhz == target.refresh_mhz
        });
        let Some(&mode) = matched else {
            return Err(ModesetError::UnknownMode {
                width: target.width,
                height: target.height,
            });
        };
        self.seam.apply(&mode)?;
        self.active = mode;
        Ok(())
    }

    /// Switch to the best mode of a role (native / HD / other) via the seam — the
    /// picker's native↔HD button (lock 12).
    ///
    /// # Errors
    /// [`ModesetError::UnknownMode`] if the panel advertises no such mode; else the
    /// seam's error.
    pub fn set_mode_class(&mut self, class: ModeClass) -> Result<(), ModesetError> {
        let Some(&mode) = self.panel.select(class) else {
            return Err(ModesetError::UnknownMode {
                width: 0,
                height: 0,
            });
        };
        self.set_mode(&mode)
    }

    /// Revert to the native mode (lock 12: revertible) via the seam.
    ///
    /// # Errors
    /// The seam's error (`NoDrmMaster` headless, `Kms` on a live failure).
    pub fn revert_native(&mut self) -> Result<(), ModesetError> {
        let native = self.panel.native;
        self.set_mode(&native)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3000×2000 @ 60 Hz, 260×173 mm Surface-class EDID (Pro-ish 3:2 panel),
    /// header + first detailed timing hand-built so the parse is exercised end to
    /// end. Bytes outside the header + descriptor are zeroed (unused by the parse).
    fn surface_edid() -> [u8; 128] {
        let mut e = [0u8; 128];
        e[0..8].copy_from_slice(&EDID_HEADER);
        // First detailed timing descriptor at offset 54.
        // Pick a pixel clock so refresh ≈ 60 Hz: h_total=3200, v_total=2084,
        // clock = 3200*2084*60 = 400.128 MHz → /10kHz = 40013 (0x9C4D).
        let d = 54;
        let clock_10khz: u16 = 40_013;
        let [lo, hi] = clock_10khz.to_le_bytes();
        e[d] = lo;
        e[d + 1] = hi;
        // h_active 3000 = 0xBB8 → lo 0xB8, hi nibble 0xB. h_blank 200 = 0x0C8 → lo 0xC8, hi 0x0.
        e[d + 2] = 0xB8; // h_active lo
        e[d + 3] = 0xC8; // h_blank lo
        e[d + 4] = 0xB0; // h_active hi(0xB)<<4 | h_blank hi(0x0)
                         // v_active 2000 = 0x7D0 → lo 0xD0, hi 0x7. v_blank 84 = 0x054 → lo 0x54, hi 0x0.
        e[d + 5] = 0xD0; // v_active lo
        e[d + 6] = 0x54; // v_blank lo
        e[d + 7] = 0x70; // v_active hi(0x7)<<4 | v_blank hi(0x0)
                         // physical size 260 x 173 mm → 260=0x104 (lo 0x04, hi 0x1), 173=0x0AD (lo 0xAD, hi 0x0).
        e[d + 12] = 0x04; // h_mm lo
        e[d + 13] = 0xAD; // v_mm lo
        e[d + 14] = 0x10; // h_mm hi(0x1)<<4 | v_mm hi(0x0)
        e
    }

    #[test]
    fn edid_parses_native_mode_and_physical_size() {
        let panel = parse_edid(&surface_edid()).expect("valid EDID");
        assert_eq!(panel.native.width, 3000);
        assert_eq!(panel.native.height, 2000);
        assert!(panel.native.preferred);
        assert_eq!(panel.phys_mm, (260, 173));
        // refresh ≈ 60 Hz (±0.5 Hz).
        let hz = panel.native.refresh_hz();
        assert!((hz - 60.0).abs() < 0.5, "refresh {hz} not ≈ 60");
    }

    #[test]
    fn edid_rejects_bad_input() {
        assert_eq!(parse_edid(&[0u8; 10]), Err(EdidError::TooShort(10)));
        let mut bad = surface_edid();
        bad[0] = 0x12;
        assert_eq!(parse_edid(&bad), Err(EdidError::BadHeader));
        let mut no_timing = surface_edid();
        no_timing[54] = 0;
        no_timing[55] = 0;
        assert_eq!(parse_edid(&no_timing), Err(EdidError::NoPreferredTiming));
    }

    #[test]
    fn dpi_to_scale_fold_is_fractional_and_clamped() {
        // Surface Pro-ish: 267 PPI → ~2.225 → quantised to 2.25 (fractional, crisp).
        let dpi = panel_dpi(2880, 274).expect("has mm"); // 2880 / (274/25.4) ≈ 267
        assert!((dpi - 267.0).abs() < 2.0, "dpi {dpi}");
        assert!((fractional_scale(dpi) - 2.25).abs() < f32::EPSILON);
        // Low-DPI desktop → clamped up to 1.0, never below.
        assert!((fractional_scale(90.0) - 1.0).abs() < f32::EPSILON);
        // Quarter-step quantisation.
        assert!((fractional_scale(150.0) - 1.25).abs() < f32::EPSILON); // 150/120=1.25
                                                                        // Absurd DPI clamps at the ceiling.
        assert!((fractional_scale(9000.0) - MAX_SCALE).abs() < f32::EPSILON);
        // No physical size → no DPI.
        assert_eq!(panel_dpi(1920, 0), None);
    }

    #[test]
    fn scale_for_panel_falls_back_without_mm() {
        let native = PanelMode::new(1920, 1080, 60, true);
        let no_mm = PanelInfo::new(native, (0, 0), &[]);
        assert!((no_mm.scale() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn mode_list_dedups_native_first_and_includes_hd() {
        let native = PanelMode::new(3000, 2000, 60, true);
        let raw = vec![
            PanelMode::new(1920, 1080, 60, false),
            PanelMode::new(1920, 1080, 60, false), // dup
            PanelMode::new(3000, 2000, 60, false), // dup of native (flag differs)
            PanelMode::new(2560, 1600, 60, false),
        ];
        let modes = build_mode_list(&native, &raw);
        // Native is first + the only preferred entry.
        assert!(modes[0].preferred);
        assert_eq!((modes[0].width, modes[0].height), (3000, 2000));
        assert_eq!(modes.iter().filter(|m| m.preferred).count(), 1);
        // No duplicates.
        assert_eq!(modes.len(), 3, "dedup: 3000, 2560, 1920 = {modes:?}");
        // Ordered by pixel count desc after native.
        assert_eq!((modes[1].width, modes[1].height), (2560, 1600));
        assert_eq!((modes[2].width, modes[2].height), (1920, 1080));
        // HD is selectable; native + Hd classify right.
        assert_eq!(modes[0].class(), ModeClass::Native);
        assert_eq!(modes[2].class(), ModeClass::Hd);
    }

    #[test]
    fn native_vs_hd_selection() {
        let native = PanelMode::new(3000, 2000, 60, true);
        let raw = vec![
            PanelMode::new(1920, 1080, 60, false),
            PanelMode::new(1920, 1080, 48, false),
        ];
        let panel = PanelInfo::new(native, (260, 173), &raw);
        let n = panel.select(ModeClass::Native).expect("native");
        assert_eq!((n.width, n.height), (3000, 2000));
        let hd = panel.select(ModeClass::Hd).expect("hd");
        assert_eq!((hd.width, hd.height), (1920, 1080));
        assert_eq!(hd.refresh_mhz, 60_000, "picks the highest-refresh 1080p");
        // A panel with no 1080p → no HD choice (never fabricated).
        let no_hd = PanelInfo::new(native, (260, 173), &[PanelMode::new(2560, 1600, 60, false)]);
        assert_eq!(no_hd.select(ModeClass::Hd), None);
    }

    #[test]
    fn headless_modeset_is_honestly_gated_not_faked() {
        let native = PanelMode::new(3000, 2000, 60, true);
        let panel = PanelInfo::new(native, (260, 173), &[PanelMode::new(1920, 1080, 60, false)]);
        let mut ctrl = DisplayController::headless(panel);
        assert_eq!(*ctrl.active_mode(), native);
        // The HD switch is honestly refused headless — and the active mode does NOT
        // change (no faked success).
        let err = ctrl.set_mode_class(ModeClass::Hd).unwrap_err();
        assert!(matches!(err, ModesetError::NoDrmMaster(_)));
        assert_eq!(
            *ctrl.active_mode(),
            native,
            "active unchanged on gated failure"
        );
    }

    /// A test seam that records the applied mode and reports success — proves the
    /// [`DisplayController`] wiring drives the seam + updates the active mode without
    /// needing a real DRM master.
    #[derive(Default)]
    struct RecordingSeam {
        applied: std::sync::Mutex<Vec<PanelMode>>,
    }
    // Implemented on the Arc so the test can keep a handle to inspect the record
    // after handing a boxed clone to the controller (coherence: the trait is local).
    impl ModesetSeam for std::sync::Arc<RecordingSeam> {
        fn apply(&self, mode: &PanelMode) -> Result<(), ModesetError> {
            self.applied.lock().expect("lock").push(*mode);
            Ok(())
        }
    }

    #[test]
    fn set_mode_drives_seam_and_updates_active() {
        let native = PanelMode::new(3000, 2000, 60, true);
        let panel = PanelInfo::new(native, (260, 173), &[PanelMode::new(1920, 1080, 60, false)]);
        let seam = std::sync::Arc::new(RecordingSeam::default());
        let mut ctrl = DisplayController::new(panel, Box::new(std::sync::Arc::clone(&seam)));
        ctrl.set_mode_class(ModeClass::Hd)
            .expect("live seam succeeds");
        assert_eq!(
            (ctrl.active_mode().width, ctrl.active_mode().height),
            (1920, 1080)
        );
        // Revert drives the seam again, back to native.
        ctrl.revert_native().expect("revert");
        assert_eq!(*ctrl.active_mode(), native);
        let applied = seam.applied.lock().expect("lock").clone();
        assert_eq!(applied.len(), 2, "HD then native = 2 modesets");
        assert_eq!((applied[0].width, applied[0].height), (1920, 1080));
        assert!(applied[1].preferred);
    }

    #[test]
    fn scale_override_clamps_and_clears() {
        let native = PanelMode::new(3000, 2000, 60, true);
        let panel = PanelInfo::new(native, (260, 173), &[]);
        let mut ctrl = DisplayController::headless(panel);
        let computed = ctrl.computed_scale();
        assert!((ctrl.effective_scale() - computed).abs() < f32::EPSILON);
        ctrl.set_scale_override(Some(9.0)); // clamps to MAX_SCALE
        assert!((ctrl.effective_scale() - MAX_SCALE).abs() < f32::EPSILON);
        ctrl.set_scale_override(None);
        assert!((ctrl.effective_scale() - computed).abs() < f32::EPSILON);
    }
}
