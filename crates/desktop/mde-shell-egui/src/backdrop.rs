//! BRAND-1 — the empty-desktop **brand backdrop**: the centered logo lockup that is
//! the shell's bottom-most desktop layer (`docs/design/desktop-logo-backdrop.md`).
//!
//! Under E12 "Quasar" the shell IS the desktop, so "the empty desktop" is the
//! Desktop surface with nothing brokered in: the [`crate::vdi`] no-desktop state and
//! the [`crate::discovery`] empty root desktop. Both paint the honest brand lockup —
//! large, centered, text-free — through this one helper, with any status relocated
//! to a small line *below* the logo (lock 2, §7 honesty preserved).
//!
//! The lockup is embedded (lock 7) and decoded ONCE to a cached texture (the
//! 1270² RGBA upload is shared by every empty path, never re-decoded per frame). All
//! colour is a `mde-theme`/`Style` token — the Carbon §4 background token behind the
//! hero, the muted-text token for the status (§4, no raw hex). Motion is the shared
//! `Motion` table: an eased opacity crossfade toward the coverage target plus a very
//! slow idle breathe (lock 10).

use std::io::Cursor;

use mde_egui::egui::{self, Align2, Color32, FontId, Rect, TextureHandle, TextureOptions};
use mde_egui::{Motion, Style};

/// The brand lockup, embedded so the shell renders it with no filesystem / RPM-path
/// dependency (lock 7). Native 1270×1270, 8-bit RGBA.
const LOGO_LOCKUP: &[u8] = include_bytes!("../../../../assets/brand/logo-lockup.png");

/// The lockup's native pixel size. The hero is never scaled past this (lock 9): on a
/// large display it simply stops growing at native rather than upscaling into
/// softness — so it stays crisp (downscale-only).
const NATIVE_MAX_PX: f32 = 1270.0;

/// Hero size as a fraction of the free area's shorter side (lock 3 — a large hero,
/// ~half the shorter dimension). A behaviour param, not a metric literal.
const HERO_FRACTION: f32 = 0.5;

/// The watermark opacity once a surface/window covers the display (lock 6) — faint
/// enough to read as "background", still visible where content leaves gaps.
const WATERMARK_ALPHA: f32 = 0.12;

/// Idle-breathe opacity amplitude (lock 10) — a barely-there sway: "alive", not
/// "pulsing" (the design's explicit distraction risk).
const BREATHE_AMPLITUDE: f32 = 0.04;

/// Idle-breathe period in seconds (lock 10) — deliberately slow.
const BREATHE_PERIOD_SECS: f64 = 6.0;

/// The shared crossfade animation key. Exactly one backdrop paints per frame (the
/// shell shows one central view at a time), so a single key yields a continuous eased
/// crossfade across every empty↔covered transition (lock 10).
const CROSSFADE_KEY: &str = "shell-brand-backdrop-coverage";

/// The egui-memory key the decoded lockup texture is cached under, so the 1270²
/// RGBA upload happens once per process and is reused by every empty path — never
/// re-decoded per frame (lock 7; the design's texture-memory note).
const TEXTURE_KEY: &str = "shell-brand-backdrop-texture";

/// Whether the display the backdrop paints on is empty (the full-opacity hero) or
/// covered by a surface/window (dimmed to a watermark). Lock 6, resolved per display
/// by the caller.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Coverage {
    /// Nothing covers the display — the hero at full opacity, breathing while idle.
    Empty,
    /// A surface/window is open — the logo drops to a low watermark, still visible in
    /// the gaps the content leaves.
    Covered,
}

/// Paint the brand backdrop as the bottom-most layer of the current panel: the solid
/// Carbon §4 background token, the centered hero lockup at the coverage-derived
/// opacity, and — when `status` is given (empty displays only, lock 2) — an honest
/// status line placed clearly BELOW the hero, never over it.
///
/// Call this FIRST in the panel body: it draws through the painter and consumes no
/// layout, so the panel's other widgets lay out over it (a covered display's list
/// floats above the watermark, lock 6).
pub(crate) fn show(ui: &mut egui::Ui, coverage: Coverage, status: Option<(&str, &str)>) {
    let free = ui.max_rect();

    // Lock 8: the solid Carbon §4 background token (the canonical empty-canvas
    // colour) is the fill the hero sits on — a theme token, never a raw hex. A clone
    // of the panel painter so `Image::paint_at` can still borrow `ui`.
    let painter = ui.painter().clone();
    painter.rect_filled(free, 0.0, Style::BG);

    // Lock 4: centre in the OPTICAL free area. `ui.max_rect()` is the rect the shell
    // handed this panel — already the viewport minus the real top-chrome
    // (`TopBottomPanel::top`) and dock (`SidePanel::left`) the shell painted — so the
    // centre tracks the live chrome/dock sizes instead of hardcoding a height.
    let side = hero_side(free, ui.ctx().pixels_per_point());
    let logo = Rect::from_center_size(free.center(), egui::vec2(side, side));

    // Lock 10: ease the opacity toward the coverage target (crossfade), then add a
    // slow breathe only at the settled full-opacity idle state.
    let empty = coverage == Coverage::Empty;
    let reveal = Motion::animate(ui.ctx(), CROSSFADE_KEY, empty, Motion::SLOW);
    let mut alpha = WATERMARK_ALPHA + reveal * (1.0 - WATERMARK_ALPHA);
    let breathing = empty && reveal > 0.999;
    if breathing {
        alpha = (alpha + breathe_offset(ui.input(|i| i.time))).clamp(0.0, 1.0);
    }

    // Lock 1/3/9: the centered hero, tinted to the opacity (a white tint at `alpha`
    // is a pure alpha multiply). If the embedded asset somehow can't decode, the
    // solid Carbon background still stands — a fail-soft, never a panic (§7).
    if let Some(texture) = logo_texture(ui.ctx()) {
        egui::Image::new(egui::load::SizedTexture::new(texture.id(), logo.size()))
            .tint(Color32::WHITE.gamma_multiply(alpha))
            .paint_at(ui, logo);
    }

    // Lock 2: any honest status is a small line clearly BELOW the hero, never over
    // it. Painted (not laid out) so it can't push the panel's other content around.
    if let Some((title, detail)) = status {
        paint_status_below(&painter, free, logo, title, detail);
    }

    // Lock 10: keep painting while the crossfade is in flight or the idle logo
    // breathes, so neither freezes between input events.
    if breathing || (reveal > 0.001 && reveal < 0.999) {
        ui.ctx().request_repaint();
    }
}

/// The hero side length in logical points: half the free area's shorter side
/// (lock 3), clamped so the 1270 px texture is never drawn larger than native
/// (lock 9). The cap is DPI-aware — `NATIVE_MAX_PX / pixels_per_point` keeps the
/// on-screen hero at ≤ 1270 physical px, so it only ever downscales (stays crisp).
fn hero_side(free: Rect, pixels_per_point: f32) -> f32 {
    let shorter = free.width().min(free.height());
    let native_cap = NATIVE_MAX_PX / pixels_per_point.max(f32::EPSILON);
    (shorter * HERO_FRACTION).clamp(0.0, native_cap)
}

/// The idle breathe: a small, slow sinusoidal opacity offset (lock 10). Bounded to
/// ±[`BREATHE_AMPLITUDE`], so narrowing the f64 sine to f32 for an opacity nudge
/// can't meaningfully truncate.
#[allow(
    clippy::cast_possible_truncation,
    reason = "the sine is bounded to [-1, 1]; the f32 narrowing for an opacity nudge is exact enough"
)]
fn breathe_offset(time: f64) -> f32 {
    let phase = (time * std::f64::consts::TAU / BREATHE_PERIOD_SECS).sin();
    BREATHE_AMPLITUDE * phase as f32
}

/// Paint the status title + detail centered below the hero (lock 2). The title is a
/// short primary-text line; the detail is dimmed and wrapped to the free width so a
/// long honest caption never runs off-panel or back over the logo.
fn paint_status_below(painter: &egui::Painter, free: Rect, logo: Rect, title: &str, detail: &str) {
    let center_x = free.center().x;
    let title_rect = painter.text(
        egui::pos2(center_x, logo.bottom() + Style::SP_L),
        Align2::CENTER_TOP,
        title,
        FontId::proportional(Style::BODY),
        Style::TEXT,
    );
    let wrap = (free.width() - Style::SP_XL * 2.0).max(Style::SP_XL);
    let galley = painter.layout(
        detail.to_owned(),
        FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
        wrap,
    );
    let detail_pos = egui::pos2(
        center_x - galley.size().x / 2.0,
        title_rect.bottom() + Style::SP_XS,
    );
    painter.galley(detail_pos, galley, Style::TEXT_DIM);
}

/// The decoded lockup texture, cached in egui memory so the 1270² RGBA upload
/// happens once and is shared by every empty path (`vdi` / `discovery`), never
/// re-decoded per frame (lock 7). `TextureHandle` is a cheap ref-counted handle, so
/// caching + cloning it is free; holding it in memory keeps the one upload alive for
/// the process.
fn logo_texture(ctx: &egui::Context) -> Option<TextureHandle> {
    let id = egui::Id::new(TEXTURE_KEY);
    // Fast path: the resolved texture (or a cached `None` from an earlier failed
    // decode) is already in egui memory — a cheap ref-counted clone.
    if let Some(cached) = ctx.data_mut(|d| d.get_temp::<Option<TextureHandle>>(id)) {
        return cached;
    }
    // Slow path (first paint): decode + upload OUTSIDE the `data_mut` lock. Doing the
    // upload inside `data_mut` would re-enter the egui context lock (`load_texture`
    // read-locks the context that `data_mut` already write-locks) and deadlock the
    // frame — so resolve first, then cache the handle.
    let texture = decode_texture(ctx);
    ctx.data_mut(|d| d.insert_temp(id, texture.clone()));
    texture
}

/// Decode the embedded PNG once and upload it. Linear sampling — the hero only ever
/// downscales (lock 9), which reads crisper than nearest. Returns `None` (never
/// panics) if the embedded asset can't decode, so the caller fails soft to the bare
/// Carbon background (§7).
fn decode_texture(ctx: &egui::Context) -> Option<TextureHandle> {
    let image = decode_rgba(LOGO_LOCKUP)?;
    Some(ctx.load_texture(TEXTURE_KEY, image, TextureOptions::LINEAR))
}

/// Decode an 8-bit RGBA PNG to an [`egui::ColorImage`] with the `png` crate (the
/// decoder `mde-files` already builds thumbnails on). Fail-soft on any malformed
/// input rather than panicking — a corrupt embedded asset is a build error, and the
/// bare Carbon background is the honest runtime fallback (§7).
fn decode_rgba(bytes: &[u8]) -> Option<egui::ColorImage> {
    let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    // The embedded lockup is 8-bit RGBA; anything else would be a build-asset error,
    // not a runtime state — bail rather than misread the bytes.
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    let w = usize::try_from(info.width).ok()?;
    let h = usize::try_from(info.height).ok()?;
    let needed = w.checked_mul(h)?.checked_mul(4)?;
    let pixels = buf.get(..needed)?;
    Some(egui::ColorImage::from_rgba_unmultiplied([w, h], pixels))
}

#[cfg(test)]
#[allow(clippy::float_cmp, reason = "exact layout arithmetic on exact inputs")]
mod tests {
    use super::*;
    use mde_egui::egui::{pos2, vec2};

    /// Render one headless 960×640 frame of the backdrop and tessellate it on the
    /// CPU — the same `Context::run` → `tessellate` path the DRM runner drives, minus
    /// the GPU. Returns whether it drew primitives and whether the lockup texture was
    /// cached (proving the one decode+upload happened).
    fn run(coverage: Coverage, status: Option<(&str, &str)>) -> (bool, bool) {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, coverage, status));
        });
        let cached = ctx.data_mut(|d| {
            d.get_temp::<Option<TextureHandle>>(egui::Id::new(TEXTURE_KEY))
                .flatten()
                .is_some()
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        (!prims.is_empty(), cached)
    }

    #[test]
    fn the_embedded_lockup_decodes_to_a_native_rgba_square() {
        // Proves the embedded asset (lock 7) is a real, correctly-shaped 1270² RGBA
        // image — not a stray/missing/mis-encoded file.
        let img = decode_rgba(LOGO_LOCKUP).expect("the embedded lockup decodes");
        assert_eq!(img.size, [1270, 1270], "native lockup size");
    }

    #[test]
    fn an_empty_display_paints_the_hero_and_caches_the_texture() {
        // The empty path: full-opacity hero + a status line below it, and the one
        // texture upload is cached for reuse (lock 1/7).
        let (drew, cached) = run(
            Coverage::Empty,
            Some((
                "No desktop connected",
                "Broker a VM desktop — it renders here.",
            )),
        );
        assert!(drew, "the empty hero backdrop produced no draw primitives");
        assert!(
            cached,
            "the lockup texture was not cached after the first paint"
        );
    }

    #[test]
    fn a_covered_display_paints_the_watermark_backdrop() {
        // The covered path (lock 6): the watermark still draws real geometry behind
        // whatever content covers the display, with no status line.
        let (drew, cached) = run(Coverage::Covered, None);
        assert!(drew, "the watermark backdrop produced no draw primitives");
        assert!(
            cached,
            "the lockup texture was not cached on the covered path"
        );
    }

    #[test]
    fn the_lockup_texture_is_decoded_once_and_reused() {
        // Two resolves on the same context hand back the SAME uploaded texture —
        // the decode+upload is not repeated per call (lock 7; texture-memory note).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let first = logo_texture(&ctx).expect("decodes once").id();
        let second = logo_texture(&ctx).expect("served from cache").id();
        assert_eq!(first, second, "the lockup must be uploaded once and reused");
    }

    #[test]
    fn the_hero_is_a_downscale_only_half_hero() {
        // Lock 3: half the shorter side on a normal panel.
        let panel = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        assert_eq!(hero_side(panel, 1.0), 300.0, "half the shorter (600) side");

        // Lock 9: capped at native on a huge display — it never upscales past 1270 px.
        let huge = Rect::from_min_size(pos2(0.0, 0.0), vec2(6000.0, 6000.0));
        assert_eq!(hero_side(huge, 1.0), NATIVE_MAX_PX, "capped at native");

        // Lock 9 (DPI-aware): at 2× the physical cap is native, so the logical cap
        // halves — the on-screen hero stays ≤ 1270 physical px.
        assert_eq!(
            hero_side(huge, 2.0),
            NATIVE_MAX_PX / 2.0,
            "DPI-aware native cap"
        );
    }
}
