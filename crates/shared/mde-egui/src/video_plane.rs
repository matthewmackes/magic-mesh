//! MEDIA-2 — the DRM **overlay video-plane seam** (`docs/design/mesh-media-player.md`
//! Q4/§Architecture): pick a hardware **overlay plane** to scan mpv's video out on,
//! *beneath* the egui shell, and compute the plane geometry that tracks the player
//! pane — with an honest **render-to-texture fallback** when no spare overlay plane
//! is available (or there is no DRM master at all).
//!
//! # Why a seam
//!
//! The bare-seat backend ([`crate::drm`]) owns the DRM/KMS master and scans the egui
//! UI out through its primary plane. For video, the design puts the decoded frame on
//! a **separate hardware overlay plane** so the GPU composites it (best power/latency)
//! and egui's chrome/OSD draws *above* it — the shell paints a transparent hole where
//! the pane sits and the video plane shows through beneath.
//!
//! Whether a given GPU actually exposes a spare overlay plane (and whether its z-order
//! can be pushed below the UI plane) is **hardware-specific and only verifiable on a
//! real seat**. So this module is split like [`crate::drm`] is:
//!
//! - The **pure seam** here — plane classification ([`PlaneKind`]), selection
//!   ([`PlaneSet::plan_video`]), and geometry ([`plane_placement`]) — is toolkit- and
//!   hardware-agnostic, compiles in the default (non-`drm`) build, and is fully
//!   unit-tested against an in-tree [`FakeCatalog`] / [`RecordingScanout`] (the same
//!   posture as `mde-media-core`'s `FakeMpv`).
//! - The **live wiring** — enumerating real DRM planes and driving `set_plane` — lives
//!   behind `feature = "drm"` in [`crate::drm`] (`probe_video_plane` /
//!   `DrmVideoScanout`). It compiles on the farm but only *presents* on real hardware.
//!
//! # The render-to-texture fallback (honest gate)
//!
//! When [`PlaneSet::plan_video`] finds no usable overlay plane — a single-plane GPU,
//! every overlay already claimed, or (via [`VideoPath::texture_no_drm`]) no DRM at all
//! such as the farm build VM or the windowed dev runner — it returns
//! [`VideoPath::Texture`]. That is the signal for the surface (MEDIA-8) to fall back to
//! rendering mpv into an **egui texture** (mpv's render API → a GL texture painted as a
//! normal egui image). That path is slower (a GPU copy per frame instead of a scanout
//! plane) but universally correct; it is the documented fallback, **not** a faked
//! plane.

// Geometry does aspect-fit + clamp math in f64 then narrows to the u32/i32 the KMS
// `set_plane` ioctl takes. Displays span < ~16k px and every value here is bounded to
// the screen, so the narrowing casts are exact — allow the pedantic cast lints for the
// two geometry helpers rather than sprinkle per-line allows. The two doc/name nursery
// lints are allowed module-wide to keep the summary-rich first paragraphs this crate
// favours (cf. `drm.rs`) and the `plane`/`pane` domain vocabulary — mirroring the
// module-level pedantic allows the sibling `mde-media-core` crate uses.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_long_first_doc_paragraph,
    clippy::similar_names
)]

/// The KMS plane type, as reported by the plane's `type` enum property
/// (`DRM_PLANE_TYPE_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaneKind {
    /// An **overlay** plane (`DRM_PLANE_TYPE_OVERLAY` = 0) — the kind we scan video
    /// out on, composited by the GPU.
    Overlay,
    /// A CRTC's built-in **primary** plane (`DRM_PLANE_TYPE_PRIMARY` = 1) — the one the
    /// egui shell scans out through; never chosen for video.
    Primary,
    /// A **cursor** plane (`DRM_PLANE_TYPE_CURSOR` = 2) — never chosen for video.
    Cursor,
    /// A `type` value the driver reported that we do not recognise.
    Unknown,
}

impl PlaneKind {
    /// Classify a raw `DRM_PLANE_TYPE_*` value (the `type` enum property's value).
    #[must_use]
    pub const fn from_drm_type(raw: u64) -> Self {
        match raw {
            0 => Self::Overlay,
            1 => Self::Primary,
            2 => Self::Cursor,
            _ => Self::Unknown,
        }
    }
}

/// One enumerated KMS plane, reduced to what plane selection needs. The live
/// enumerator ([`crate::drm::probe_video_plane`]) builds these from real `drm` handles;
/// tests build them directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneInfo {
    /// The plane's KMS object id (`u32` from the `drm` `plane::Handle`). Identity only
    /// — used to exclude the egui plane and to map the choice back to a handle.
    pub id: u32,
    /// What kind of plane this is.
    pub kind: PlaneKind,
    /// The `possible_crtcs` bitmask: bit *N* is set when this plane can drive the
    /// *N*-th CRTC in the card's CRTC list (matches KMS's own bit ordering).
    pub possible_crtcs: u32,
    /// The plane's current `zpos` property value, or [`None`] when the driver exposes
    /// no `zpos` property on it (then z-order is the driver's fixed plane ordering and
    /// [`VideoPlanePlan::zpos`] is left [`None`] — an honest "can't reorder" note).
    pub zpos: Option<u64>,
}

impl PlaneInfo {
    /// Whether this plane can scan out on the CRTC at `crtc_index` (bit test on
    /// [`Self::possible_crtcs`]). A `crtc_index` ≥ 32 can never match (the bitmask is
    /// 32-wide), so it returns `false` rather than overflow-shift.
    #[must_use]
    pub const fn supports_crtc(&self, crtc_index: u32) -> bool {
        if crtc_index >= 32 {
            return false;
        }
        self.possible_crtcs & (1u32 << crtc_index) != 0
    }
}

/// A rectangle in a CRTC's pixel space — the player-pane the video plane tracks, or a
/// resolved destination/hole rect. `x`/`y` may be negative (a pane scrolled partly off
/// the top/left edge); width/height are unsigned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneRect {
    /// Left edge, CRTC pixels (may be negative when partly off-screen).
    pub x: i32,
    /// Top edge, CRTC pixels (may be negative when partly off-screen).
    pub y: i32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl PaneRect {
    /// A pane rect from its `(x, y, w, h)` parts.
    #[must_use]
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// The resolved plane geometry to program with `set_plane`: where on the CRTC the
/// (possibly cropped) frame lands, and which source region of the frame feeds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// The on-screen destination rect `(x, y, w, h)` in CRTC pixels — the visible,
    /// screen-clamped portion of the fitted video. Feeds `set_plane`'s `crtc_rect`.
    pub crtc_rect: (i32, i32, u32, u32),
    /// The source rect `(x, y, w, h)` in the frame, in **16.16 fixed-point** (KMS's
    /// `set_plane` source-coordinate convention). Cropped to match `crtc_rect` when the
    /// pane runs off the screen edge.
    pub src_rect_16_16: (u32, u32, u32, u32),
}

/// The full plan for scanning video out on a chosen overlay plane this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoPlanePlan {
    /// The chosen overlay plane's KMS id.
    pub plane_id: u32,
    /// The `zpos` value to set on the video plane so it sits **below** the egui plane,
    /// or [`None`] when the driver exposes no settable `zpos` (rely on the fixed plane
    /// ordering — the honest "can't reorder" case).
    pub zpos: Option<u64>,
    /// The geometry to program, or [`None`] when the pane is fully off-screen / zero-
    /// area this frame — the caller then *clears* the plane (shows nothing) rather than
    /// programming a degenerate rect.
    pub placement: Option<Placement>,
    /// The transparent rect the egui shell must leave in its own plane so the video
    /// plane shows through beneath it (equals the visible destination rect); [`None`]
    /// when nothing is visible this frame.
    pub punch_hole: Option<PaneRect>,
}

/// Why the seam chose the render-to-texture fallback instead of an overlay plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    /// No DRM master at all — the farm build VM, CI, or the windowed dev runner.
    NoDrm,
    /// A DRM seat is up but no spare overlay plane can drive this CRTC (single-plane
    /// GPU, or every overlay already claimed).
    NoOverlayPlane,
    /// The surface explicitly asked for the texture path (e.g. a user/driver override).
    Forced,
}

impl std::fmt::Display for FallbackReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let why = match self {
            Self::NoDrm => "no DRM master (headless / windowed runner)",
            Self::NoOverlayPlane => "no spare overlay plane on this CRTC",
            Self::Forced => "render-to-texture forced by the caller",
        };
        write!(f, "render-to-texture fallback: {why}")
    }
}

/// How the surface should present video this frame: on a hardware overlay plane, or
/// via the render-to-texture fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoPath {
    /// Scan out on the chosen overlay plane per the [`VideoPlanePlan`].
    Overlay(VideoPlanePlan),
    /// Fall back to rendering mpv into an egui texture — see the module docs; the
    /// reason is carried for the honest log/OSD note.
    Texture(FallbackReason),
}

impl VideoPath {
    /// The render-to-texture path taken when there is no DRM master at all (the caller
    /// has no [`PlaneSet`] to consult — headless, the farm VM, or the windowed runner).
    #[must_use]
    pub const fn texture_no_drm() -> Self {
        Self::Texture(FallbackReason::NoDrm)
    }

    /// The render-to-texture path taken when the caller explicitly overrides to it.
    #[must_use]
    pub const fn texture_forced() -> Self {
        Self::Texture(FallbackReason::Forced)
    }

    /// The chosen overlay plan, if this is the overlay path.
    #[must_use]
    pub const fn overlay(&self) -> Option<&VideoPlanePlan> {
        match self {
            Self::Overlay(plan) => Some(plan),
            Self::Texture(_) => None,
        }
    }

    /// Whether this is the render-to-texture fallback.
    #[must_use]
    pub const fn is_texture(&self) -> bool {
        matches!(self, Self::Texture(_))
    }
}

/// The planes of one CRTC as the seam sees them: every plane on the card, plus which
/// plane the egui shell scans out through (excluded from video selection) and that
/// plane's z-order, and the index of the CRTC being driven.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneSet {
    /// Every plane enumerated on the card.
    pub planes: Vec<PlaneInfo>,
    /// The KMS id of the plane the egui shell scans out through (its primary plane).
    /// Never chosen for video, and the plane the video plane is ordered below.
    pub egui_plane_id: u32,
    /// The egui plane's `zpos`, when known — used to target a video `zpos` strictly
    /// below it. [`None`] when the driver exposes no `zpos` (fixed ordering).
    pub egui_zpos: Option<u64>,
    /// The index (in the card's CRTC list) of the CRTC being driven.
    pub crtc_index: u32,
}

impl PlaneSet {
    /// Pick the video overlay plane for this CRTC: the first [`PlaneKind::Overlay`]
    /// plane that can drive [`Self::crtc_index`] and is not the egui plane. Returns
    /// [`None`] when there is none (→ the texture fallback).
    #[must_use]
    pub fn select_overlay(&self) -> Option<&PlaneInfo> {
        self.planes.iter().find(|p| {
            p.kind == PlaneKind::Overlay
                && p.id != self.egui_plane_id
                && p.supports_crtc(self.crtc_index)
        })
    }

    /// Resolve how to present video this frame for a `video` frame of `(w, h)` shown in
    /// `pane`, on a `screen` of `(w, h)` CRTC pixels.
    ///
    /// Selects an overlay plane ([`Self::select_overlay`]); if none, returns
    /// [`VideoPath::Texture`]([`FallbackReason::NoOverlayPlane`]). Otherwise computes the
    /// pane-tracking geometry ([`plane_placement`]) and a `zpos` below the egui plane,
    /// and returns [`VideoPath::Overlay`].
    #[must_use]
    pub fn plan_video(&self, video: (u32, u32), pane: PaneRect, screen: (u32, u32)) -> VideoPath {
        let Some(plane) = self.select_overlay() else {
            return VideoPath::Texture(FallbackReason::NoOverlayPlane);
        };
        let placement = plane_placement(video, pane, screen);
        // Target a zpos strictly below the egui plane so the UI composites above the
        // video. Only when BOTH the egui plane and the chosen video plane expose a
        // `zpos` property — otherwise leave it None and rely on the driver's fixed
        // plane ordering (the honest "can't reorder here" note).
        let zpos = match (self.egui_zpos, plane.zpos) {
            (Some(egui_z), Some(_)) => Some(egui_z.saturating_sub(1)),
            _ => None,
        };
        let punch_hole = placement.map(|p| {
            let (x, y, w, h) = p.crtc_rect;
            PaneRect::new(x, y, w, h)
        });
        VideoPath::Overlay(VideoPlanePlan {
            plane_id: plane.id,
            zpos,
            placement,
            punch_hole,
        })
    }
}

/// Aspect-preserving fit of a `video` frame of `(w, h)` into `pane` — the letterboxed
/// destination rect, centred in the pane (no stretch / no z-fight from a wrong aspect).
/// Returns a zero-size rect at the pane origin when either the video or the pane has a
/// zero dimension.
#[must_use]
pub fn fit_rect(video: (u32, u32), pane: PaneRect) -> (i32, i32, u32, u32) {
    let (vw, vh) = video;
    if vw == 0 || vh == 0 || pane.width == 0 || pane.height == 0 {
        return (pane.x, pane.y, 0, 0);
    }
    let (vwf, vhf) = (f64::from(vw), f64::from(vh));
    let (pwf, phf) = (f64::from(pane.width), f64::from(pane.height));
    // Scale to the tighter axis so the whole frame fits inside the pane.
    let scale = (pwf / vwf).min(phf / vhf);
    let dw = (vwf * scale).round().max(0.0);
    let dh = (vhf * scale).round().max(0.0);
    // Centre the fitted frame in the pane.
    let dx = pane.x + (((pwf - dw) / 2.0).round() as i32);
    let dy = pane.y + (((phf - dh) / 2.0).round() as i32);
    (dx, dy, dw as u32, dh as u32)
}

/// Clamp a destination rect that shows the **whole** `video` frame to the `screen`
/// bounds, cropping the source region proportionally so the visible part stays
/// undistorted. Returns [`None`] when nothing is visible (fully off-screen or zero
/// area) — the caller then clears the plane.
///
/// `dest` is `(x, y, w, h)` in CRTC pixels (from [`fit_rect`]); the returned
/// [`Placement`] carries the screen-clamped `crtc_rect` and the matching cropped source
/// rect in 16.16 fixed-point.
#[must_use]
pub fn clamp_and_crop(
    dest: (i32, i32, u32, u32),
    video: (u32, u32),
    screen: (u32, u32),
) -> Option<Placement> {
    let (dx, dy, dw, dh) = dest;
    let (vw, vh) = video;
    let (sw, sh) = screen;
    if dw == 0 || dh == 0 || vw == 0 || vh == 0 || sw == 0 || sh == 0 {
        return None;
    }
    // Visible intersection of the destination rect with the screen (0,0,sw,sh), all in
    // i64 so a large width added to a negative origin can't wrap.
    let vx0 = i64::from(dx).max(0);
    let vy0 = i64::from(dy).max(0);
    let vx1 = (i64::from(dx) + i64::from(dw)).min(i64::from(sw));
    let vy1 = (i64::from(dy) + i64::from(dh)).min(i64::from(sh));
    if vx1 <= vx0 || vy1 <= vy0 {
        return None; // fully off-screen
    }
    let vis_w = (vx1 - vx0) as u32;
    let vis_h = (vy1 - vy0) as u32;

    // The destination shows the whole frame scaled by dw/vw (x) and dh/vh (y). The
    // visible sub-rect therefore maps back to this source sub-rect, in 16.16.
    let off_x = (vx0 - i64::from(dx)) as f64; // ≥ 0, how far the left crop is into dest
    let off_y = (vy0 - i64::from(dy)) as f64;
    let (dwf, dhf) = (f64::from(dw), f64::from(dh));
    let (vwf, vhf) = (f64::from(vw), f64::from(vh));

    let src_x = to_16_16(off_x * vwf / dwf);
    let src_y = to_16_16(off_y * vhf / dhf);
    let src_w = to_16_16(f64::from(vis_w) * vwf / dwf);
    let src_h = to_16_16(f64::from(vis_h) * vhf / dhf);

    Some(Placement {
        // vx0/vy0 are clamped into [0, screen], so they fit i32 for the crtc_rect.
        crtc_rect: (vx0 as i32, vy0 as i32, vis_w, vis_h),
        src_rect_16_16: (src_x, src_y, src_w, src_h),
    })
}

/// The full pane-tracking geometry: aspect-fit the `video` into `pane`, then clamp to
/// `screen`. [`None`] when nothing is visible this frame.
#[must_use]
pub fn plane_placement(video: (u32, u32), pane: PaneRect, screen: (u32, u32)) -> Option<Placement> {
    clamp_and_crop(fit_rect(video, pane), video, screen)
}

/// Convert a pixel coordinate (already ≥ 0 and bounded to the frame) to 16.16 fixed
/// point, rounding to the nearest 1/65536 and saturating at `u32::MAX`.
fn to_16_16(px: f64) -> u32 {
    let scaled = (px * 65536.0).round();
    if scaled <= 0.0 {
        0
    } else if scaled >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        scaled as u32
    }
}

/// An opaque framebuffer token handed across the pure scanout seam in tests. The live
/// path ([`crate::drm::DrmVideoScanout`]) uses a real `drm` framebuffer handle instead;
/// this stands in for it wherever no DRM is present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FbToken(pub u64);

/// The **handoff seam**: hand a decoded video frame (a dmabuf/framebuffer from mpv's
/// render API, or a [`FbToken`] in tests) to the chosen overlay plane, or clear the
/// plane. The live implementation drives KMS `set_plane`; the in-tree
/// [`RecordingScanout`] records the calls so the whole seam is exercised without
/// hardware.
pub trait VideoScanout {
    /// The frame-handle type: `FbToken` for the fake, a real framebuffer handle live.
    type Frame;

    /// Present `frame` on the plane per `plan` (program geometry + z-order, attach the
    /// framebuffer). A `plan` with `placement == None` is treated as [`Self::clear`].
    ///
    /// # Errors
    /// [`VideoPlaneError`] when the backend rejects the plane commit.
    fn present(&mut self, frame: Self::Frame, plan: &VideoPlanePlan)
        -> Result<(), VideoPlaneError>;

    /// Clear the plane `plane_id` (detach its framebuffer — show nothing).
    ///
    /// # Errors
    /// [`VideoPlaneError`] when the backend rejects the clear.
    fn clear(&mut self, plane_id: u32) -> Result<(), VideoPlaneError>;
}

/// A typed failure from the live plane seam (enumerate / commit). Kept coarse — the
/// caller only needs "the plane path is unavailable here" to fall back to a texture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoPlaneError {
    /// Enumerating the card's planes / properties failed.
    Enumerate(String),
    /// Programming the plane (`set_plane`) failed.
    Commit(String),
}

impl std::fmt::Display for VideoPlaneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Enumerate(why) => write!(f, "plane enumeration failed: {why}"),
            Self::Commit(why) => write!(f, "plane commit failed: {why}"),
        }
    }
}

impl std::error::Error for VideoPlaneError {}

/// A catalog of planes — the enumerate half of the seam. The live implementation reads
/// real DRM/KMS; [`FakeCatalog`] returns a scripted [`PlaneSet`].
pub trait PlaneCatalog {
    /// Enumerate the planes of the driven CRTC.
    ///
    /// # Errors
    /// [`VideoPlaneError::Enumerate`] when the underlying enumeration fails.
    fn plane_set(&self) -> Result<PlaneSet, VideoPlaneError>;
}

/// An in-tree [`PlaneCatalog`] that hands back a fixed [`PlaneSet`] — the airgap-safe,
/// hardware-free catalog the unit tests and any headless caller drive (mirrors
/// `mde_media_core::FakeMpv`). Reachable, not a `#[cfg(test)]` mock.
#[derive(Debug, Clone)]
pub struct FakeCatalog {
    /// The plane set this catalog reports.
    pub set: PlaneSet,
}

impl FakeCatalog {
    /// A fake catalog reporting `set`.
    #[must_use]
    pub const fn new(set: PlaneSet) -> Self {
        Self { set }
    }
}

impl PlaneCatalog for FakeCatalog {
    fn plane_set(&self) -> Result<PlaneSet, VideoPlaneError> {
        Ok(self.set.clone())
    }
}

/// An in-tree [`VideoScanout`] that records every `present`/`clear` instead of touching
/// hardware — the airgap-safe scanout the seam tests drive to prove the handoff without
/// a real plane.
#[derive(Debug, Default, Clone)]
pub struct RecordingScanout {
    /// Every plan presented, in order.
    pub presented: Vec<VideoPlanePlan>,
    /// The frame tokens presented, in order (parallel to [`Self::presented`]).
    pub frames: Vec<FbToken>,
    /// Every plane id cleared, in order.
    pub cleared: Vec<u32>,
}

impl RecordingScanout {
    /// A fresh recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl VideoScanout for RecordingScanout {
    type Frame = FbToken;

    fn present(
        &mut self,
        frame: Self::Frame,
        plan: &VideoPlanePlan,
    ) -> Result<(), VideoPlaneError> {
        if plan.placement.is_none() {
            return self.clear(plan.plane_id);
        }
        self.presented.push(plan.clone());
        self.frames.push(frame);
        Ok(())
    }

    fn clear(&mut self, plane_id: u32) -> Result<(), VideoPlaneError> {
        self.cleared.push(plane_id);
        Ok(())
    }
}

/// Plan-then-present in one call over the seam: enumerate `catalog`, resolve the
/// [`VideoPath`], and — on the overlay path — hand `frame` to `scanout`. Returns the
/// resolved path so the caller knows whether to also run the texture fallback.
///
/// # Errors
/// [`VideoPlaneError`] from the catalog enumeration or the scanout commit.
pub fn present_frame<C, S>(
    catalog: &C,
    scanout: &mut S,
    frame: S::Frame,
    video: (u32, u32),
    pane: PaneRect,
    screen: (u32, u32),
) -> Result<VideoPath, VideoPlaneError>
where
    C: PlaneCatalog,
    S: VideoScanout,
{
    let path = catalog.plane_set()?.plan_video(video, pane, screen);
    if let VideoPath::Overlay(plan) = &path {
        scanout.present(frame, plan)?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlay(id: u32, crtcs: u32) -> PlaneInfo {
        PlaneInfo {
            id,
            kind: PlaneKind::Overlay,
            possible_crtcs: crtcs,
            zpos: Some(1),
        }
    }
    fn primary(id: u32, crtcs: u32) -> PlaneInfo {
        PlaneInfo {
            id,
            kind: PlaneKind::Primary,
            possible_crtcs: crtcs,
            zpos: Some(0),
        }
    }
    fn cursor(id: u32, crtcs: u32) -> PlaneInfo {
        PlaneInfo {
            id,
            kind: PlaneKind::Cursor,
            possible_crtcs: crtcs,
            zpos: Some(2),
        }
    }

    fn set_with(planes: Vec<PlaneInfo>, egui_id: u32, crtc_index: u32) -> PlaneSet {
        PlaneSet {
            planes,
            egui_plane_id: egui_id,
            egui_zpos: Some(1),
            crtc_index,
        }
    }

    #[test]
    fn plane_kind_classifies_drm_type() {
        assert_eq!(PlaneKind::from_drm_type(0), PlaneKind::Overlay);
        assert_eq!(PlaneKind::from_drm_type(1), PlaneKind::Primary);
        assert_eq!(PlaneKind::from_drm_type(2), PlaneKind::Cursor);
        assert_eq!(PlaneKind::from_drm_type(9), PlaneKind::Unknown);
    }

    #[test]
    fn supports_crtc_is_a_bit_test_and_never_overflows() {
        let p = overlay(10, 0b101); // CRTC 0 and CRTC 2
        assert!(p.supports_crtc(0));
        assert!(!p.supports_crtc(1));
        assert!(p.supports_crtc(2));
        assert!(!p.supports_crtc(3));
        assert!(!p.supports_crtc(31));
        assert!(!p.supports_crtc(32)); // out of range → false, no panic
        assert!(!p.supports_crtc(9999));
    }

    #[test]
    fn selects_the_overlay_not_the_primary_cursor_or_egui_plane() {
        // egui scans out on the primary (id 1). The lone overlay (id 3) that supports
        // CRTC 0 must be chosen; the primary + cursor are never video planes.
        let set = set_with(vec![primary(1, 0b1), cursor(2, 0b1), overlay(3, 0b1)], 1, 0);
        let chosen = set.select_overlay().expect("an overlay exists");
        assert_eq!(chosen.id, 3);
    }

    #[test]
    fn skips_an_overlay_that_cannot_drive_this_crtc() {
        // Two overlays: id 3 only drives CRTC 0, id 4 drives CRTC 1. Driving CRTC 1
        // must pick id 4.
        let set = set_with(
            vec![primary(1, 0b11), overlay(3, 0b01), overlay(4, 0b10)],
            1,
            1,
        );
        assert_eq!(set.select_overlay().expect("overlay").id, 4);
    }

    #[test]
    fn no_overlay_plane_falls_back_to_texture() {
        // A single-plane GPU: only the primary. Video must fall back to the texture
        // path with the honest NoOverlayPlane reason.
        let set = set_with(vec![primary(1, 0b1)], 1, 0);
        let path = set.plan_video((1920, 1080), PaneRect::new(0, 0, 1920, 1080), (1920, 1080));
        assert_eq!(path, VideoPath::Texture(FallbackReason::NoOverlayPlane));
        assert!(path.is_texture());
        assert!(path.overlay().is_none());
    }

    #[test]
    fn overlay_present_but_offscreen_pane_clears_the_plane() {
        let set = set_with(vec![primary(1, 0b1), overlay(3, 0b1)], 1, 0);
        // Pane entirely to the left of the screen → nothing visible.
        let path = set.plan_video(
            (1920, 1080),
            PaneRect::new(-4000, 0, 1920, 1080),
            (1920, 1080),
        );
        // A plane exists, so we stay on the overlay path — just with nothing to show.
        let plan = path
            .overlay()
            .expect("a plane exists — should not fall back");
        assert_eq!(plan.plane_id, 3);
        assert!(plan.placement.is_none(), "offscreen → clear the plane");
        assert!(plan.punch_hole.is_none());
    }

    #[test]
    fn zpos_targets_below_the_egui_plane() {
        // egui plane zpos = 1; the video plane must be targeted at 0 (strictly below).
        let set = PlaneSet {
            planes: vec![primary(1, 0b1), overlay(3, 0b1)],
            egui_plane_id: 1,
            egui_zpos: Some(1),
            crtc_index: 0,
        };
        let path = set.plan_video((1280, 720), PaneRect::new(0, 0, 1280, 720), (1280, 720));
        let plan = path.overlay().expect("overlay");
        assert_eq!(plan.zpos, Some(0));
    }

    #[test]
    fn zpos_is_none_when_the_driver_exposes_no_zpos() {
        // No zpos property anywhere → we can't reorder; honest None (fixed ordering).
        let mut ov = overlay(3, 0b1);
        ov.zpos = None;
        let set = PlaneSet {
            planes: vec![primary(1, 0b1), ov],
            egui_plane_id: 1,
            egui_zpos: None,
            crtc_index: 0,
        };
        let plan = set
            .plan_video((1280, 720), PaneRect::new(0, 0, 1280, 720), (1280, 720))
            .overlay()
            .cloned()
            .expect("overlay");
        assert_eq!(plan.zpos, None);
    }

    #[test]
    fn fit_rect_preserves_aspect_and_centres_letterbox() {
        // A 1920x1080 (16:9) frame into a 1000x1000 pane → 1000x562 (rounded), centred:
        // horizontal full-width, vertical letterbox with ~219px bars.
        let (x, y, w, h) = fit_rect((1920, 1080), PaneRect::new(0, 0, 1000, 1000));
        assert_eq!(w, 1000);
        assert_eq!(h, 563); // 1080 * (1000/1920) = 562.5 → round 563
        assert_eq!(x, 0);
        assert_eq!(y, 219); // centred: round((1000 - 563) / 2) = round(218.5) = 219
    }

    #[test]
    fn fit_rect_pillarbox_for_a_tall_pane() {
        // 16:9 into a 500x1000 pane → width-limited: 500x281, pillarboxed vertically.
        let (x, y, w, h) = fit_rect((1920, 1080), PaneRect::new(10, 20, 500, 1000));
        assert_eq!(w, 500);
        assert_eq!(h, 281); // 1080*(500/1920)=281.25 → 281
        assert_eq!(x, 10);
        assert_eq!(y, 380); // 20 + round((1000 - 281) / 2) = 20 + round(359.5) = 380
    }

    #[test]
    fn fit_rect_zero_dims_are_empty() {
        assert_eq!(
            fit_rect((0, 1080), PaneRect::new(5, 6, 100, 100)),
            (5, 6, 0, 0)
        );
        assert_eq!(
            fit_rect((1920, 1080), PaneRect::new(5, 6, 0, 100)),
            (5, 6, 0, 0)
        );
    }

    #[test]
    fn full_frame_placement_is_whole_source_no_crop() {
        // Pane == screen, frame fills it (same aspect) → src is the whole frame in
        // 16.16, crtc is the whole screen.
        let p = plane_placement((1920, 1080), PaneRect::new(0, 0, 1920, 1080), (1920, 1080))
            .expect("visible");
        assert_eq!(p.crtc_rect, (0, 0, 1920, 1080));
        assert_eq!(p.src_rect_16_16, (0, 0, 1920 << 16, 1080 << 16));
    }

    #[test]
    fn offscreen_left_crops_the_source_proportionally() {
        // A 1000x1000 frame shown 1:1 at x = -400 on a 1920-wide screen: the left 400px
        // are cropped, 600px visible from x=0. Source x starts at 400<<16, width 600<<16.
        let placement =
            clamp_and_crop((-400, 0, 1000, 1000), (1000, 1000), (1920, 1080)).expect("visible");
        assert_eq!(placement.crtc_rect, (0, 0, 600, 1000));
        assert_eq!(placement.src_rect_16_16.0, 400 << 16); // src x
        assert_eq!(placement.src_rect_16_16.2, 600 << 16); // src w
    }

    #[test]
    fn fully_offscreen_is_none() {
        assert!(clamp_and_crop((-2000, 0, 1000, 1000), (1000, 1000), (1920, 1080)).is_none());
        assert!(clamp_and_crop((5000, 0, 100, 100), (100, 100), (1920, 1080)).is_none());
        assert!(clamp_and_crop((0, 0, 0, 100), (100, 100), (1920, 1080)).is_none());
    }

    #[test]
    fn to_16_16_rounds_and_saturates() {
        assert_eq!(to_16_16(0.0), 0);
        assert_eq!(to_16_16(-5.0), 0);
        assert_eq!(to_16_16(1.0), 1 << 16);
        assert_eq!(to_16_16(2.5), (2 << 16) + (1 << 15));
        assert_eq!(to_16_16(1e12), u32::MAX); // saturates, no wrap
    }

    #[test]
    fn recording_scanout_records_present_and_clears() {
        let set = set_with(vec![primary(1, 0b1), overlay(3, 0b1)], 1, 0);
        let catalog = FakeCatalog::new(set);
        let mut scanout = RecordingScanout::new();

        // Visible pane → a present is recorded with the frame token.
        let path = present_frame(
            &catalog,
            &mut scanout,
            FbToken(0xF00D),
            (1920, 1080),
            PaneRect::new(0, 0, 1920, 1080),
            (1920, 1080),
        )
        .expect("seam ok");
        assert!(matches!(path, VideoPath::Overlay(_)));
        assert_eq!(scanout.presented.len(), 1);
        assert_eq!(scanout.frames, vec![FbToken(0xF00D)]);
        assert_eq!(scanout.presented[0].plane_id, 3);
        assert!(scanout.cleared.is_empty());
    }

    #[test]
    fn present_of_an_offscreen_plan_clears_instead() {
        let mut scanout = RecordingScanout::new();
        let plan = VideoPlanePlan {
            plane_id: 7,
            zpos: Some(0),
            placement: None,
            punch_hole: None,
        };
        scanout.present(FbToken(1), &plan).expect("clear ok");
        assert!(scanout.presented.is_empty());
        assert_eq!(scanout.cleared, vec![7]);
    }

    #[test]
    fn texture_no_drm_is_the_headless_fallback() {
        let path = VideoPath::texture_no_drm();
        assert_eq!(path, VideoPath::Texture(FallbackReason::NoDrm));
        assert!(path.is_texture());
        // The Display note is the honest operator-facing reason.
        assert!(FallbackReason::NoDrm.to_string().contains("no DRM master"));
    }
}
