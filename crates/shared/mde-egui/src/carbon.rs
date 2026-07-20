//! **Mackes-Carbon** — the shared SVG icon loader for every E12 surface.
//!
//! Mackes-Carbon is the canonical platform icon set: IBM Carbon repackaged as a
//! freedesktop SVG theme (`assets/icons/Mackes-Carbon`, freedesktop
//! Icon-Naming-Spec names — `go-previous`, `view-refresh`, `bookmark-new`, …).
//! Every glyph is authored `fill="currentColor"`, so one embedded glyph serves
//! every tint: this loader rasterizes the vector artwork to a coverage **alpha
//! mask** and multiplies that mask by the caller's [`Color32`], the classic
//! *symbolic-icon* behavior — any glyph renders in any color.
//!
//! # Why a shared loader
//!
//! The shell used to PAINT its chrome icons procedurally (one hand-rolled
//! `match` arm per glyph). That does not scale to a platform-wide icon standard.
//! This module is the reusable foundation every surface builds on:
//!
//! - a central **name → SVG bytes** registry ([`carbon_svg_bytes`]) of the
//!   curated subset embedded via `include_bytes!` (see
//!   `crates/shared/mde-egui/assets/carbon/`), so rendering is deterministic and
//!   needs no installed icon theme at runtime;
//! - a rasterizer ([`carbon_raster`]) that renders `currentColor` as a coverage
//!   mask and tints it with any [`Color32`];
//! - a `ctx`-memory texture **cache** ([`carbon_texture`]) keyed by
//!   `(name, size_px, color)` so a glyph rasterizes once, not every frame;
//! - two ergonomic entry points: a widget ([`carbon_icon`]) and a painter-level
//!   draw ([`paint_carbon`]) that mirrors the shape of the shell's old
//!   `paint_chrome_icon` so existing call sites reroute with a one-line change.
//!
//! SVG → raster is done with `resvg` (which re-exports its `usvg` parser and
//! `tiny-skia` rasterizer) — pure Rust, no system libraries. The Carbon glyphs
//! are flat single-color `<path>` artwork with no `<text>`, so `resvg` is built
//! without its `text`/`fontdb` features.

// The public entry points intentionally repeat the module name (`carbon_icon`,
// `carbon_texture`, `paint_carbon`): they are the platform icon-standard API and
// read best fully qualified from another crate (`mde_egui::carbon::carbon_icon`)
// or through the re-exports.
#![allow(clippy::module_name_repetitions)]

use egui::{
    Color32, Context, Painter, Pos2, Rect, Response, Sense, TextureHandle, TextureOptions, Ui,
};
use resvg::{tiny_skia, usvg};

/// The curated Mackes-Carbon subset embedded into the binary, as a
/// `name → SVG bytes` registry. `name` is the freedesktop
/// (or, for a handful sourced from the raw Carbon library, the Carbon) glyph
/// name; the bytes are the exact `currentColor` SVG source. Extend this table as
/// the platform-wide icon sweep pulls more glyphs in — it is the single place a
/// new embedded icon is registered.
static REGISTRY: &[(&str, &[u8])] = &[
    // --- navigation / history -------------------------------------------------
    (
        "go-previous",
        include_bytes!("../assets/carbon/go-previous.svg"),
    ),
    ("go-next", include_bytes!("../assets/carbon/go-next.svg")),
    ("go-up", include_bytes!("../assets/carbon/go-up.svg")),
    ("go-down", include_bytes!("../assets/carbon/go-down.svg")),
    (
        "view-refresh",
        include_bytes!("../assets/carbon/view-refresh.svg"),
    ),
    (
        "process-stop",
        include_bytes!("../assets/carbon/process-stop.svg"),
    ),
    (
        "document-open-recent",
        include_bytes!("../assets/carbon/document-open-recent.svg"),
    ),
    // --- chrome actions -------------------------------------------------------
    (
        "open-menu",
        include_bytes!("../assets/carbon/open-menu.svg"),
    ),
    (
        "window-close",
        include_bytes!("../assets/carbon/window-close.svg"),
    ),
    ("new-tab", include_bytes!("../assets/carbon/new-tab.svg")),
    (
        "view-grid",
        include_bytes!("../assets/carbon/view-grid.svg"),
    ),
    (
        "bookmark-new",
        include_bytes!("../assets/carbon/bookmark-new.svg"),
    ),
    (
        "system-search",
        include_bytes!("../assets/carbon/system-search.svg"),
    ),
    (
        "edit-find",
        include_bytes!("../assets/carbon/edit-find.svg"),
    ),
    ("zoom-in", include_bytes!("../assets/carbon/zoom-in.svg")),
    ("zoom-out", include_bytes!("../assets/carbon/zoom-out.svg")),
    (
        "document-print",
        include_bytes!("../assets/carbon/document-print.svg"),
    ),
    (
        "document-edit",
        include_bytes!("../assets/carbon/document-edit.svg"),
    ),
    ("download", include_bytes!("../assets/carbon/download.svg")),
    (
        "camera-photo",
        include_bytes!("../assets/carbon/camera-photo.svg"),
    ),
    ("share", include_bytes!("../assets/carbon/share.svg")),
    ("view", include_bytes!("../assets/carbon/view.svg")),
    ("globe", include_bytes!("../assets/carbon/globe.svg")),
    (
        "system-shutdown",
        include_bytes!("../assets/carbon/system-shutdown.svg"),
    ),
    ("list-add", include_bytes!("../assets/carbon/list-add.svg")),
    (
        "list-remove",
        include_bytes!("../assets/carbon/list-remove.svg"),
    ),
    (
        "text-x-generic",
        include_bytes!("../assets/carbon/text-x-generic.svg"),
    ),
    (
        "emblem-ok",
        include_bytes!("../assets/carbon/emblem-ok.svg"),
    ),
    ("star", include_bytes!("../assets/carbon/star.svg")),
    ("overlay", include_bytes!("../assets/carbon/overlay.svg")),
    // --- status / security ----------------------------------------------------
    (
        "security-high",
        include_bytes!("../assets/carbon/security-high.svg"),
    ),
    (
        "changes-prevent",
        include_bytes!("../assets/carbon/changes-prevent.svg"),
    ),
    (
        "system-lock-screen",
        include_bytes!("../assets/carbon/system-lock-screen.svg"),
    ),
    (
        "dialog-warning",
        include_bytes!("../assets/carbon/dialog-warning.svg"),
    ),
    (
        "notification",
        include_bytes!("../assets/carbon/notification.svg"),
    ),
    (
        "weather-clear-night",
        include_bytes!("../assets/carbon/weather-clear-night.svg"),
    ),
    // --- media transport ------------------------------------------------------
    (
        "media-playback-start",
        include_bytes!("../assets/carbon/media-playback-start.svg"),
    ),
    (
        "media-playback-pause",
        include_bytes!("../assets/carbon/media-playback-pause.svg"),
    ),
    (
        "media-playback-stop",
        include_bytes!("../assets/carbon/media-playback-stop.svg"),
    ),
    (
        "media-skip-backward",
        include_bytes!("../assets/carbon/media-skip-backward.svg"),
    ),
    (
        "media-skip-forward",
        include_bytes!("../assets/carbon/media-skip-forward.svg"),
    ),
    // --- audio volume ---------------------------------------------------------
    (
        "audio-volume-high",
        include_bytes!("../assets/carbon/audio-volume-high.svg"),
    ),
    (
        "audio-volume-low",
        include_bytes!("../assets/carbon/audio-volume-low.svg"),
    ),
    (
        "audio-volume-muted",
        include_bytes!("../assets/carbon/audio-volume-muted.svg"),
    ),
];

/// Look up the embedded SVG source for a Mackes-Carbon glyph `name`.
///
/// Returns `None` for a name that is not in the curated embedded subset — the
/// caller falls back (e.g. a procedural draw) rather than panicking, which keeps
/// the loader safe to call speculatively during the platform icon sweep.
#[must_use]
pub fn carbon_svg_bytes(name: &str) -> Option<&'static [u8]> {
    REGISTRY
        .iter()
        .find_map(|(key, bytes)| (*key == name).then_some(*bytes))
}

/// Every glyph `name` currently embedded, in registry order — the set of icons a
/// surface may reference by name today. Useful for exhaustiveness tests as the
/// sweep grows the registry.
#[must_use]
pub fn carbon_names() -> impl Iterator<Item = &'static str> {
    REGISTRY.iter().map(|(key, _)| *key)
}

/// A rasterized Carbon glyph — plain RGBA8 with *straight* (unmultiplied) alpha,
/// row-major, sized for [`egui::ColorImage::from_rgba_unmultiplied`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarbonRaster {
    /// Raster width in pixels (follows the source aspect; square for the Carbon
    /// glyphs, which are authored on a 32×32 canvas).
    pub width: u32,
    /// Raster height in pixels — always exactly the requested `size_px`.
    pub height: u32,
    /// RGBA8 pixel data, straight alpha, row-major; `width × height × 4` bytes.
    /// Every opaque pixel carries the requested tint RGB; alpha is the glyph's
    /// coverage scaled by the tint's alpha.
    pub rgba: Vec<u8>,
}

impl CarbonRaster {
    /// `[width, height]` as `usize` — the shape [`egui::ColorImage`] wants.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)] // usize ≥ 32 bits on every MCNF target
    pub const fn size_usize(&self) -> [usize; 2] {
        [self.width as usize, self.height as usize]
    }
}

/// Rasterize a Mackes-Carbon glyph to a `color`-tinted RGBA buffer `size_px`
/// tall (width follows the source aspect ratio).
///
/// **Tinting is an alpha-mask multiply.** The glyph's `currentColor` is
/// substituted with an opaque color *before* parsing, so `resvg` renders the
/// artwork as a fully-opaque shape; the rasterizer's per-pixel **coverage**
/// (the anti-aliased alpha) becomes the icon mask. Each output pixel then takes
/// the requested `color`'s RGB, and the mask coverage scaled by the `color`'s
/// alpha as its alpha. The result is the symbolic-icon behavior: the same glyph
/// renders crisply in any color the caller asks for, DPI-sharp at whatever
/// physical size the caller computed.
///
/// Returns `None` for an unknown `name`, a zero `size_px`, an SVG that fails to
/// parse, or a raster buffer that cannot be allocated — the loader never panics.
#[must_use]
#[allow(
    clippy::cast_precision_loss,      // size_px → f32: raster sizes are small (≪ 2^24)
    clippy::cast_possible_truncation, // rounded, clamped-positive f32 → u32
    clippy::cast_sign_loss            // width ≥ 1.0 by the .max(1.0) clamp
)]
pub fn carbon_raster(name: &str, size_px: u32, color: Color32) -> Option<CarbonRaster> {
    if size_px == 0 {
        return None;
    }
    let bytes = carbon_svg_bytes(name)?;
    let source = std::str::from_utf8(bytes).ok()?;
    // Render `currentColor` as opaque black so the rasterizer produces a clean
    // coverage mask; the RGB is discarded per-pixel below and replaced by `color`.
    let opaque = source.replace("currentColor", "#000000");
    let tree = usvg::Tree::from_str(&opaque, &usvg::Options::default()).ok()?;

    let svg_size = tree.size();
    let scale = size_px as f32 / svg_size.height();
    let width = (svg_size.width() * scale).round().max(1.0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(width, size_px)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    let [red, green, blue, alpha] = color.to_array();
    let mut rgba = Vec::with_capacity(pixmap.pixels().len() * 4);
    for px in pixmap.pixels() {
        // The fill is opaque, so the premultiplied pixel's alpha *is* the glyph
        // coverage — the alpha mask. Tint it with the caller's `color`.
        let coverage = px.alpha();
        rgba.extend_from_slice(&[red, green, blue, scale_alpha(coverage, alpha)]);
    }
    Some(CarbonRaster {
        width,
        height: size_px,
        rgba,
    })
}

/// Scale a coverage alpha by the tint's alpha (`coverage × tint / 255`, rounding
/// half up) — how the tint's alpha channel folds into the glyph mask.
fn scale_alpha(coverage: u8, tint_alpha: u8) -> u8 {
    let scaled = (u16::from(coverage) * u16::from(tint_alpha) + 127) / 255;
    u8::try_from(scaled).unwrap_or(u8::MAX) // ≤ 255 by construction
}

/// Rasterize a Carbon glyph and upload it as a `ctx`-cached [`TextureHandle`],
/// keyed by `(name, size_px, color)`.
///
/// The texture is memoized in `ctx` memory, so a glyph rasterizes once per
/// distinct `(name, size_px, color)` and every later frame reuses the handle —
/// this is a hot path, so re-rasterizing per frame would be wasteful. `size_px`
/// is a physical pixel count (multiply the logical size by
/// [`Context::pixels_per_point`] for a DPI-crisp glyph — [`paint_carbon`] and
/// [`carbon_icon`] do this for you).
///
/// Returns `None` for an unknown `name` or a glyph that fails to rasterize; the
/// `None` is cached too, so a miss is not retried every frame.
#[must_use]
pub fn carbon_texture(
    ctx: &Context,
    name: &str,
    size_px: u32,
    color: Color32,
) -> Option<TextureHandle> {
    let key = egui::Id::new(("mde-egui-carbon", name, size_px, color.to_array()));
    if let Some(cached) = ctx.data_mut(|data| data.get_temp::<Option<TextureHandle>>(key)) {
        return cached;
    }
    let handle = carbon_raster(name, size_px, color).map(|raster| {
        let image = egui::ColorImage::from_rgba_unmultiplied(raster.size_usize(), &raster.rgba);
        ctx.load_texture(format!("carbon-{name}"), image, TextureOptions::LINEAR)
    });
    ctx.data_mut(|data| data.insert_temp(key, handle.clone()));
    handle
}

/// Paint a Mackes-Carbon glyph into `rect`, tinted `color`, at the painter's
/// pixel density — the painter-level entry point that mirrors the shape of the
/// shell's old `paint_chrome_icon(painter, rect, icon, color)`.
///
/// Returns `true` if the glyph painted, `false` if `name` is not in the
/// registry (or failed to rasterize) so the caller can fall back to a procedural
/// draw. The glyph is centered and aspect-fit inside a small inset of `rect`.
#[must_use]
#[allow(
    clippy::cast_precision_loss,      // logical px → f32: small values
    clippy::cast_possible_truncation, // rounded, clamped-positive f32 → u32
    clippy::cast_sign_loss            // size_px ≥ 1.0 by the .max(1.0) clamp
)]
pub fn paint_carbon(painter: &Painter, rect: Rect, name: &str, color: Color32) -> bool {
    let draw = rect.shrink(2.0);
    let logical = draw.width().min(draw.height()).max(1.0);
    let size_px = (logical * painter.ctx().pixels_per_point())
        .round()
        .max(1.0) as u32;
    let Some(texture) = carbon_texture(painter.ctx(), name, size_px, color) else {
        return false;
    };
    let [width, height] = texture.size();
    let aspect = width.max(1) as f32 / height.max(1) as f32;
    let image_rect = fit_centered(draw, aspect);
    painter.image(
        texture.id(),
        image_rect,
        Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    true
}

/// A Mackes-Carbon glyph as a hover-sensing widget, `size × size` logical
/// points, tinted the current [`egui::Visuals`] text color.
///
/// This is the ergonomic entry point for laying an icon out in a `Ui`. For a
/// specific tint or a bare painter, use [`paint_carbon`]. If `name` is unknown
/// the widget still allocates its space (so layout is stable) but paints
/// nothing — call [`carbon_svg_bytes`] first if you need to branch on presence.
pub fn carbon_icon(ui: &mut Ui, name: &str, size: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
    if ui.is_rect_visible(rect) {
        let color = ui.visuals().text_color();
        let _ = paint_carbon(ui.painter(), rect, name, color);
    }
    response
}

/// Aspect-fit `aspect` (`w / h`) centered inside `rect`, never overflowing it.
fn fit_centered(rect: Rect, aspect: f32) -> Rect {
    let aspect = aspect.max(0.01);
    let rect_aspect = rect.width().max(1.0) / rect.height().max(1.0);
    let size = if aspect > rect_aspect {
        egui::vec2(rect.width(), rect.width() / aspect)
    } else {
        egui::vec2(rect.height() * aspect, rect.height())
    };
    Rect::from_center_size(rect.center(), size)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)] // tests fail by panicking, with context
mod tests {
    use super::*;

    /// A glyph known to be in the embedded subset.
    const SAMPLE: &str = "go-previous";
    /// An opaque red tint used across the raster assertions.
    const RED: Color32 = Color32::from_rgb(220, 40, 40);

    fn opaque_pixels(rgba: &[u8]) -> usize {
        rgba.chunks_exact(4).filter(|px| px[3] > 0).count()
    }

    #[test]
    fn registry_is_populated_and_unique() {
        assert!(
            carbon_names().count() >= 40,
            "curated subset should be present"
        );
        let mut names: Vec<&str> = carbon_names().collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "registry names must be unique");
    }

    #[test]
    fn every_embedded_glyph_rasterizes_non_blank() {
        // Proves the whole curated subset is valid SVG and produces coverage.
        for name in carbon_names() {
            let raster =
                carbon_raster(name, 32, RED).unwrap_or_else(|| panic!("{name} must rasterize"));
            assert_eq!(raster.height, 32, "{name}: height must equal size_px");
            assert!(raster.width >= 1, "{name}: width must be positive");
            assert_eq!(
                raster.rgba.len(),
                (raster.width * raster.height * 4) as usize,
                "{name}: rgba buffer must be width*height*4"
            );
            assert!(
                opaque_pixels(&raster.rgba) > 0,
                "{name}: rasterized glyph must be non-blank (some covered pixels)"
            );
        }
    }

    #[test]
    fn raster_is_non_blank_and_correctly_shaped() {
        let raster = carbon_raster(SAMPLE, 32, RED).expect("sample must rasterize");
        // A 32px arrow covers a meaningful fraction but is far from a full square.
        let covered = opaque_pixels(&raster.rgba);
        let total = (raster.width * raster.height) as usize;
        assert!(covered > 20, "glyph must cover real pixels, got {covered}");
        assert!(
            covered < total,
            "glyph must not fill the whole square ({covered}/{total})"
        );
    }

    #[test]
    fn tint_applies_the_requested_color() {
        let red = carbon_raster(SAMPLE, 32, Color32::from_rgb(200, 0, 0)).unwrap();
        let blue = carbon_raster(SAMPLE, 32, Color32::from_rgb(0, 0, 200)).unwrap();
        // Every fully-opaque pixel must carry exactly the requested tint RGB.
        for px in red.rgba.chunks_exact(4).filter(|px| px[3] == 255) {
            assert_eq!([px[0], px[1], px[2]], [200, 0, 0], "red tint must apply");
        }
        for px in blue.rgba.chunks_exact(4).filter(|px| px[3] == 255) {
            assert_eq!([px[0], px[1], px[2]], [0, 0, 200], "blue tint must apply");
        }
        // Same mask, different color → identical coverage, different RGB.
        assert_eq!(red.width, blue.width);
        let red_cov: Vec<u8> = red.rgba.chunks_exact(4).map(|px| px[3]).collect();
        let blue_cov: Vec<u8> = blue.rgba.chunks_exact(4).map(|px| px[3]).collect();
        assert_eq!(red_cov, blue_cov, "tint must not change the coverage mask");
    }

    #[test]
    fn tint_alpha_scales_coverage() {
        let solid = carbon_raster(SAMPLE, 32, Color32::from_rgba_unmultiplied(200, 0, 0, 255));
        let half = carbon_raster(SAMPLE, 32, Color32::from_rgba_unmultiplied(200, 0, 0, 128));
        let solid_max = solid
            .unwrap()
            .rgba
            .chunks_exact(4)
            .map(|px| px[3])
            .max()
            .unwrap();
        let half_max = half
            .unwrap()
            .rgba
            .chunks_exact(4)
            .map(|px| px[3])
            .max()
            .unwrap();
        assert_eq!(solid_max, 255, "opaque tint keeps full coverage");
        assert!(
            half_max < solid_max,
            "half-alpha tint dims the mask ({half_max})"
        );
    }

    #[test]
    fn unknown_name_is_none_not_panic() {
        assert!(carbon_svg_bytes("definitely-not-a-glyph").is_none());
        assert!(carbon_raster("definitely-not-a-glyph", 32, RED).is_none());
        assert!(carbon_raster(SAMPLE, 0, RED).is_none(), "zero size is None");
    }

    #[test]
    fn texture_cache_returns_the_same_handle() {
        let ctx = Context::default();
        let first = carbon_texture(&ctx, SAMPLE, 24, RED).expect("first load");
        let second = carbon_texture(&ctx, SAMPLE, 24, RED).expect("cache hit");
        assert_eq!(first.id(), second.id(), "same key must reuse the texture");
        // A different color is a different cache entry → a different texture.
        let other = carbon_texture(&ctx, SAMPLE, 24, Color32::from_rgb(0, 0, 200)).unwrap();
        assert_ne!(
            first.id(),
            other.id(),
            "distinct color must be a distinct texture"
        );
        // An unknown name caches and returns None.
        assert!(carbon_texture(&ctx, "definitely-not-a-glyph", 24, RED).is_none());
    }
}
