//! The VDI Desktop **pointer / geometry seam** — the pure coordinate math split
//! out of the Desktop god-module (pure relocation, no behaviour change).
//!
//! Panel-space egui points ↔ guest desktop pixels: [`map_pointer_to_desktop`] and
//! its event wrapper [`remap_pointer_event`] (vdi-vm-2), plus the DPI-aware desktop
//! size negotiation ([`target_desktop_size`] / [`seat_max_px`] / [`body_device_px`],
//! vdi-vm-8) and the shared [`to_desktop_dim`] round+clamp. All DPI-independent and
//! unit-tested off-UI; no `live-vdi` transport gate touches this cluster.
//!
//! `use super::*` pulls in the parent's `egui` re-export; as a child module it reads
//! the parent's private items directly, so only what the parent and the tests call
//! back into is `pub(super)`/`pub(crate)`.

use super::*;

/// Map an egui pointer position (egui **points**, panel/screen space) to a guest
/// **desktop pixel**, given the desktop texture's painted `rect` (also in points)
/// and the guest `desktop_size` in pixels.
///
/// The remote framebuffer is painted to *fill* `rect`, which sits below/right of
/// the dock + menubar chrome, so its top-left origin is non-zero. A pointer at
/// fraction `f` across the rect corresponds to the same fraction across the guest
/// desktop, so the transform
///
/// 1. subtracts the rect's top-left origin (`pos - rect.min`),
/// 2. divides by the rect size for the `0..1` fraction, then
/// 3. multiplies by `desktop_size` to land in guest pixels.
///
/// Because *both* the pointer and `rect` are reported in egui points, the panel's
/// `pixels_per_point` cancels in the fraction — the mapping is DPI-independent and
/// correct whether the desktop was negotiated at the panel's native size (a crisp
/// 1:1 paint) or a smaller hardcoded size egui upscales (vdi-vm-8). The result is
/// clamped to the real guest bounds `[0, w-1] × [0, h-1]` (never `u16::MAX`), so a
/// drag that slips a pixel past the panel edge still lands on a real edge pixel.
pub(super) fn map_pointer_to_desktop(
    pos: egui::Pos2,
    rect: egui::Rect,
    desktop_size: (u16, u16),
) -> egui::Pos2 {
    let (w, h) = desktop_size;
    let fraction = |v: f32, min: f32, extent: f32| {
        if extent > 0.0 {
            (v - min) / extent
        } else {
            0.0
        }
    };
    let fx = fraction(pos.x, rect.min.x, rect.width());
    let fy = fraction(pos.y, rect.min.y, rect.height());
    let last_x = f32::from(w.saturating_sub(1));
    let last_y = f32::from(h.saturating_sub(1));
    egui::pos2(
        (fx * f32::from(w)).clamp(0.0, last_x),
        (fy * f32::from(h)).clamp(0.0, last_y),
    )
}

/// Rewrite a pointer event's position from panel space into guest desktop pixels
/// via [`map_pointer_to_desktop`]. Every non-pointer event (key, wheel, text,
/// focus, touch) is returned unchanged, so ONLY the coordinate bug is fixed and
/// every other input semantic (button mapping, scroll, key events) is preserved.
pub(super) fn remap_pointer_event(
    event: egui::Event,
    rect: egui::Rect,
    desktop_size: (u16, u16),
) -> egui::Event {
    match event {
        egui::Event::PointerMoved(pos) => {
            egui::Event::PointerMoved(map_pointer_to_desktop(pos, rect, desktop_size))
        }
        egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers,
        } => egui::Event::PointerButton {
            pos: map_pointer_to_desktop(pos, rect, desktop_size),
            button,
            pressed,
            modifiers,
        },
        other => other,
    }
}

/// vdi-vm-8 — the pure geometry seam: the guest desktop size to negotiate from a
/// panel's real size. `available` is the panel size in egui **points**, `ppp` the
/// output's pixels-per-point, and `max` the seat-resolution ceiling in **device
/// pixels**. The panel points are scaled to device pixels (`available * ppp`),
/// rounded, and each axis clamped to `[1, max]` so the shell never asks a guest for
/// MORE pixels than the seat can display (nor for zero). At `ppp == 1` the result
/// equals the (rounded, clamped) panel — the DPI-aware 1:1 target the pointer
/// transform then maps against, so panel↔desktop scale is ~1:1 (composes with
/// vdi-vm-2). Kept pure so the clamp / round / DPI behaviour is unit-tested off-UI.
pub(crate) fn target_desktop_size(available: egui::Vec2, ppp: f32, max: (u16, u16)) -> (u16, u16) {
    let px = available * ppp;
    let (mw, mh) = max;
    (
        to_desktop_dim(px.x).min(mw.max(1)),
        to_desktop_dim(px.y).min(mh.max(1)),
    )
}

/// vdi-vm-8 — the seat resolution ceiling in **device pixels**: the full egui output
/// rect scaled by `pixels_per_point`. Used as the `max` clamp for
/// [`target_desktop_size`] so no negotiated desktop exceeds what the seat can show.
pub(super) fn seat_max_px(ctx: &egui::Context) -> (u16, u16) {
    let ppp = ctx.pixels_per_point();
    let s = ctx.screen_rect().size() * ppp;
    (to_desktop_dim(s.x), to_desktop_dim(s.y))
}

/// The shell's current output size in guest **device pixels** — the vdi-vm-8 desktop
/// size hint for a live RDP/SPICE connect. At connect time the Desktop *panel* is not
/// mounted yet (the connect is dispatched from the Chooser / menu chrome), so this
/// estimates from the full egui output rect, which the desktop panel is a sub-rect of
/// (it sits under the dock + menubar). Routed through [`target_desktop_size`] with the
/// seat resolution as both estimate and ceiling. The live path
/// (`VdiState::note_resize_target`) refines to the true panel size on a material
/// resize; the worst case here is a crisp downscale the pointer transform keeps exact.
pub(crate) fn body_device_px(ctx: &egui::Context) -> (u16, u16) {
    target_desktop_size(
        ctx.screen_rect().size(),
        ctx.pixels_per_point(),
        seat_max_px(ctx),
    )
}

/// Round + clamp a device-pixel extent into `[1, u16::MAX]` — a desktop dimension
/// is always at least one pixel, and a non-finite input degrades to `1`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "value is rounded then clamped into [1, u16::MAX]; non-finite maps to 1"
)]
fn to_desktop_dim(v: f32) -> u16 {
    if v.is_finite() {
        v.round().clamp(1.0, f32::from(u16::MAX)) as u16
    } else {
        1
    }
}
