//! `brand::icons` — the monochrome Quasar line-art icon set (QBRAND-2).
//!
//! The 20 brand glyphs (`assets/brand/quasar/*.svg`, QBRAND-10) embedded as
//! inline SVG consts behind [`IconId`], plus the SVG→raster loader
//! ([`icon_image`]) every surface draws them through. The glyphs are authored
//! in `currentColor` (the text-lockup wordmark excepted — see below), so ONE
//! embedded set serves every tint: the loader substitutes the caller's color
//! pre-parse and rasterizes with `resvg` at the exact requested pixel size —
//! DPI-crisp at any scale, no pre-baked PNG ladder.
//!
//! ## Toolkit-free by design
//!
//! This crate stays free of a GUI dependency (QBRAND lock #4: the daemon and
//! packaging read the same crate as the shell — `mackesd --version` must not
//! pull egui), so the loader returns a plain RGBA8 buffer ([`IconImage`])
//! rather than an egui texture. The shell wraps it in one line:
//!
//! ```ignore
//! let img = mde_theme::brand::icons::icon_image(id, size, tint)?;
//! let color = egui::ColorImage::from_rgba_unmultiplied(img.size_usize(), &img.rgba);
//! let tex = ctx.load_texture(id.name(), color, egui::TextureOptions::LINEAR);
//! ```
//!
//! Tints come in as a plain `[r, g, b, a]` array so callers pass their
//! `mde_egui::Style` token colors directly — this crate never re-derives token
//! values (that would fork the design system's source of truth).
//!
//! ## The wordmark logotype
//!
//! [`IconId::Wordmark`] is the official stacked text lockup ("MDE" / "Quazar"
//! / "Mackes Display Environment") — pure `<text>` elements carrying their own
//! brand fills (it is the one glyph NOT authored in `currentColor`, so the
//! tint does not recolor it). `resvg` is built here without its `text`/fontdb
//! features (the minimal, farm-vendorable configuration), so the lockup
//! parses and keeps its 320×184 aspect but rasterizes fully transparent — it
//! never panics, and callers wanting the visible lockup should use the
//! official raster assets (`assets/brand/quasar/app-icon-*.png` /
//! `brand::logo`, QBRAND-3). The SVG's own `<desc>` flags the designed fix:
//! outlining the letterforms to paths, with resvg's `text` feature + a
//! bundled fontdb as the heavier alternative.

use std::fmt;

use resvg::{tiny_skia, usvg};

/// Embed one Quasar brand SVG from `assets/brand/quasar/` at compile time.
macro_rules! quasar_svg {
    ($file:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../assets/brand/quasar/",
            $file
        ))
    };
}

/// Identifier for every glyph in the Quasar brand set.
///
/// The product marks, the 14 dock/surface glyphs and the 3 node-role badges —
/// one variant per SVG in `assets/brand/quasar/`; [`IconId::svg`] resolves the
/// embedded source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconId {
    /// The round mesh-node constellation product mark (`mark.svg`, the 64×64
    /// trace of the official artwork; the blue plane rides a 0.55-opacity
    /// group so single-color tinting keeps the two-tone hierarchy).
    Mark,
    /// The stacked "MDE / Quazar / Mackes Display Environment" text lockup
    /// (`wordmark.svg`, 320×184 — rasterizes transparent without a fontdb;
    /// see the module docs).
    Wordmark,
    /// A single mesh node/peer glyph (`node.svg`), health-tinted by callers.
    Node,
    /// The Workbench (fleet command) surface glyph.
    Workbench,
    /// The Instances (VM broker) surface glyph.
    Instances,
    /// The remote Desktop surface glyph.
    Desktop,
    /// The Music surface glyph.
    Music,
    /// The Media player surface glyph.
    Media,
    /// The Files surface glyph.
    Files,
    /// The Voice surface glyph.
    Voice,
    /// The Browser surface glyph.
    Browser,
    /// The Terminal surface glyph.
    Terminal,
    /// The Editor (code editor) surface glyph.
    Editor,
    /// The Chat surface glyph.
    Chat,
    /// The System surface glyph.
    System,
    /// The Storage surface glyph.
    Storage,
    /// The Mesh-View (topology map) surface glyph.
    MeshView,
    /// The Workstation role badge.
    Workstation,
    /// The Server role badge.
    Server,
    /// The Lighthouse role badge.
    Lighthouse,
}

impl IconId {
    /// Every glyph in the set, for exhaustive iteration (dock catalogs, tests).
    pub const ALL: [Self; 20] = [
        Self::Mark,
        Self::Wordmark,
        Self::Node,
        Self::Workbench,
        Self::Instances,
        Self::Desktop,
        Self::Music,
        Self::Media,
        Self::Files,
        Self::Voice,
        Self::Browser,
        Self::Terminal,
        Self::Editor,
        Self::Chat,
        Self::System,
        Self::Storage,
        Self::MeshView,
        Self::Workstation,
        Self::Server,
        Self::Lighthouse,
    ];

    /// The embedded SVG source for this glyph — `currentColor` line-art in a
    /// square viewBox (`0 0 32 32` for the surface/role/node glyphs, `0 0 64
    /// 64` for the mark); the wordmark alone is a `0 0 320 184` text lockup.
    #[must_use]
    pub const fn svg(self) -> &'static str {
        match self {
            Self::Mark => quasar_svg!("mark.svg"),
            Self::Wordmark => quasar_svg!("wordmark.svg"),
            Self::Node => quasar_svg!("node.svg"),
            Self::Workbench => quasar_svg!("surface-workbench.svg"),
            Self::Instances => quasar_svg!("surface-instances.svg"),
            Self::Desktop => quasar_svg!("surface-desktop.svg"),
            Self::Music => quasar_svg!("surface-music.svg"),
            Self::Media => quasar_svg!("surface-media.svg"),
            Self::Files => quasar_svg!("surface-files.svg"),
            Self::Voice => quasar_svg!("surface-voice.svg"),
            Self::Browser => quasar_svg!("surface-browser.svg"),
            Self::Terminal => quasar_svg!("surface-terminal.svg"),
            Self::Editor => quasar_svg!("surface-editor.svg"),
            Self::Chat => quasar_svg!("surface-chat.svg"),
            Self::System => quasar_svg!("surface-system.svg"),
            Self::Storage => quasar_svg!("surface-storage.svg"),
            Self::MeshView => quasar_svg!("surface-mesh-view.svg"),
            Self::Workstation => quasar_svg!("role-workstation.svg"),
            Self::Server => quasar_svg!("role-server.svg"),
            Self::Lighthouse => quasar_svg!("role-lighthouse.svg"),
        }
    }

    /// The glyph's stable asset name (the SVG file stem, e.g.
    /// `"surface-terminal"`) — handy as an egui texture debug-name and for
    /// packaging scripts that resolve the on-disk asset.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Mark => "mark",
            Self::Wordmark => "wordmark",
            Self::Node => "node",
            Self::Workbench => "surface-workbench",
            Self::Instances => "surface-instances",
            Self::Desktop => "surface-desktop",
            Self::Music => "surface-music",
            Self::Media => "surface-media",
            Self::Files => "surface-files",
            Self::Voice => "surface-voice",
            Self::Browser => "surface-browser",
            Self::Terminal => "surface-terminal",
            Self::Editor => "surface-editor",
            Self::Chat => "surface-chat",
            Self::System => "surface-system",
            Self::Storage => "surface-storage",
            Self::MeshView => "surface-mesh-view",
            Self::Workstation => "role-workstation",
            Self::Server => "role-server",
            Self::Lighthouse => "role-lighthouse",
        }
    }
}

/// A rasterized glyph — plain RGBA8 with *straight* (unmultiplied) alpha,
/// row-major, ready for `egui::ColorImage::from_rgba_unmultiplied` (see the
/// module docs for the one-line shell wrapper).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconImage {
    /// Raster width in pixels (equals the requested size for the square
    /// glyphs; wider for the wordmark lockup).
    pub width: u32,
    /// Raster height in pixels — always exactly the requested `size_px`.
    pub height: u32,
    /// RGBA8 pixel data, straight alpha, row-major; `width × height × 4` bytes.
    pub rgba: Vec<u8>,
}

impl IconImage {
    /// `[width, height]` as `usize` — the shape `egui::ColorImage` wants.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)] // usize ≥ 32 bits on every MCNF target
    pub const fn size_usize(&self) -> [usize; 2] {
        [self.width as usize, self.height as usize]
    }
}

/// Why a glyph failed to rasterize. Every [`IconId`] source is embedded at
/// compile time and covered by tests, so in practice only [`Self::ZeroSize`]
/// is reachable from caller input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconError {
    /// The requested raster size was zero pixels.
    ZeroSize,
    /// The embedded SVG failed to parse — a build-time asset bug.
    Parse {
        /// The glyph whose embedded source failed to parse.
        id: IconId,
        /// The `usvg` parse error, stringified.
        reason: String,
    },
    /// The raster buffer could not be allocated for these dimensions.
    Alloc {
        /// The glyph being rasterized.
        id: IconId,
        /// The raster width that failed to allocate.
        width: u32,
        /// The raster height that failed to allocate.
        height: u32,
    },
}

impl fmt::Display for IconError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroSize => write!(f, "icon raster size must be non-zero"),
            Self::Parse { id, reason } => {
                write!(f, "embedded SVG for {id:?} failed to parse: {reason}")
            }
            Self::Alloc { id, width, height } => {
                write!(
                    f,
                    "raster buffer alloc failed for {id:?} at {width}x{height}"
                )
            }
        }
    }
}

impl std::error::Error for IconError {}

/// Rasterize a brand glyph at exactly `size_px` tall, tinted `[r, g, b, a]`.
///
/// The glyph's `currentColor` is substituted with the tint's RGB *before*
/// parsing (the line-art glyphs are authored in `currentColor`, so a string
/// substitution colors every stroke and fill; per-glyph `opacity` groups keep
/// the traced artwork's tonal hierarchy under the single color); the tint's
/// alpha is applied per-pixel after rasterization, since SVG hex colors carry
/// no alpha channel. The raster
/// height is exactly `size_px` and the width follows the source aspect ratio —
/// `size_px × size_px` for the square glyphs, proportionally wider for the
/// wordmark — so the result is DPI-crisp at whatever physical size the caller
/// computed.
///
/// # Errors
///
/// [`IconError::ZeroSize`] for `size_px == 0`; [`IconError::Parse`] /
/// [`IconError::Alloc`] only on an embedded-asset or dimension bug (both are
/// exercised across the whole set by this module's tests).
#[allow(
    clippy::cast_precision_loss,      // size_px → f32: raster sizes are small (≪ 2^24)
    clippy::cast_possible_truncation, // rounded, clamped-positive f32 → u32
    clippy::cast_sign_loss            // width ≥ 1.0 by the .max(1.0) clamp
)]
pub fn icon_image(id: IconId, size_px: u32, tint: [u8; 4]) -> Result<IconImage, IconError> {
    if size_px == 0 {
        return Err(IconError::ZeroSize);
    }
    let [red, green, blue, alpha] = tint;
    let colored = id
        .svg()
        .replace("currentColor", &format!("#{red:02x}{green:02x}{blue:02x}"));
    let options = usvg::Options::default();
    let tree = usvg::Tree::from_str(&colored, &options).map_err(|err| IconError::Parse {
        id,
        reason: err.to_string(),
    })?;

    let svg_size = tree.size();
    let scale = size_px as f32 / svg_size.height();
    let width = (svg_size.width() * scale).round().max(1.0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(width, size_px).ok_or(IconError::Alloc {
        id,
        width,
        height: size_px,
    })?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    // tiny-skia rasterizes premultiplied; egui wants straight alpha. Demultiply
    // and fold the tint's alpha into the coverage in one pass.
    let mut rgba = Vec::with_capacity(pixmap.pixels().len() * 4);
    for px in pixmap.pixels() {
        let c = px.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), scale_alpha(c.alpha(), alpha)]);
    }
    Ok(IconImage {
        width,
        height: size_px,
        rgba,
    })
}

/// Scale a rasterized coverage alpha by the tint's alpha (`coverage × tint /
/// 255`, rounding half up) — how the tint's alpha channel is applied, since
/// the pre-parse color substitution can only carry RGB.
fn scale_alpha(coverage: u8, tint_alpha: u8) -> u8 {
    let scaled = (u16::from(coverage) * u16::from(tint_alpha) + 127) / 255;
    u8::try_from(scaled).unwrap_or(u8::MAX) // ≤ 255 by construction
}

#[cfg(test)]
#[allow(clippy::panic)] // tests fail by panicking, with per-glyph context
mod tests {
    use super::{icon_image, IconError, IconId};

    /// A light Gray-10-ish tint used across the raster tests.
    const TINT: [u8; 4] = [0xe0, 0xe0, 0xe0, 0xff];

    /// Count pixels with any coverage — the "rasterized non-empty" probe.
    fn opaque_pixels(rgba: &[u8]) -> usize {
        rgba.chunks_exact(4).filter(|px| px[3] > 0).count()
    }

    /// Byte index of the strongest-coverage pixel — a geometry-independent
    /// anchor for the tint assertions (the official mark trace has no
    /// guaranteed feature at any fixed coordinate).
    fn max_alpha_index(rgba: &[u8]) -> usize {
        rgba.chunks_exact(4)
            .enumerate()
            .max_by_key(|(_, px)| px[3])
            .map_or(0, |(i, _)| i * 4)
    }

    #[test]
    fn every_icon_rasterizes_nonempty_at_16_32_64() {
        for id in IconId::ALL {
            for size in [16_u32, 32, 64] {
                let img = icon_image(id, size, TINT)
                    .unwrap_or_else(|err| panic!("{id:?} @ {size}px failed: {err}"));
                assert_eq!(img.height, size, "{id:?} @ {size}px height");
                assert!(img.width >= size, "{id:?} @ {size}px width {}", img.width);
                let [w, h] = img.size_usize();
                assert_eq!(img.rgba.len(), w * h * 4, "{id:?} @ {size}px buffer len");
                // The wordmark is pure <text> and rasterizes transparent
                // without a fontdb (module docs; the dedicated wordmark test
                // pins that behavior) — every other glyph must show ink.
                if id != IconId::Wordmark {
                    assert!(
                        opaque_pixels(&img.rgba) > 0,
                        "{id:?} @ {size}px rasterized empty"
                    );
                }
            }
        }
    }

    #[test]
    fn tint_rgb_covers_every_inked_pixel() {
        // The mark is pure currentColor, so after the pre-parse substitution
        // EVERY covered pixel must carry exactly the tint's RGB (demultiply
        // is exact for 0x00/0xff channels), whatever the traced geometry.
        let img = icon_image(IconId::Mark, 64, [0xff, 0x00, 0x00, 0xff]).expect("mark rasterizes");
        let mut inked = 0_usize;
        for px in img.rgba.chunks_exact(4) {
            if px[3] > 0 {
                inked += 1;
                assert_eq!(&px[..3], &[0xff, 0x00, 0x00], "inked pixel off-tint");
            }
        }
        assert!(inked > 0, "mark rasterized empty");
        // Something renders at real strength too: the blue-plane node fills
        // sit in a 0.55-opacity group (≈ alpha 140), the white plane at full.
        let idx = max_alpha_index(&img.rgba);
        assert!(
            img.rgba[idx + 3] >= 128,
            "mark strongest coverage too faint: {}",
            img.rgba[idx + 3]
        );
    }

    #[test]
    fn tint_alpha_scales_coverage() {
        // Same glyph, same size, tint alpha 255 vs 128: the coverage raster
        // is identical, so each pixel's alpha must land at exactly
        // (coverage × 128 + 127) / 255 — checked at the strongest pixel,
        // independent of the traced geometry.
        let full = icon_image(IconId::Mark, 64, [0xff, 0xff, 0xff, 0xff]).expect("full-alpha mark");
        let half = icon_image(IconId::Mark, 64, [0xff, 0xff, 0xff, 0x80]).expect("half-alpha mark");
        let idx = max_alpha_index(&full.rgba);
        let coverage = full.rgba[idx + 3]; // scale_alpha(c, 255) == c
        assert!(coverage > 0, "mark rasterized empty");
        let expected = u8::try_from((u16::from(coverage) * 0x80 + 127) / 255).unwrap_or(u8::MAX);
        assert_eq!(
            half.rgba[idx + 3],
            expected,
            "tint alpha must scale coverage {coverage}"
        );
    }

    #[test]
    fn wordmark_is_empty_without_a_fontdb_but_never_panics() {
        // The official wordmark is a pure-<text> stacked lockup; resvg is
        // built here without text/fontdb (module docs), so it must parse,
        // keep its wide 320×184 aspect and return a fully transparent raster
        // — gracefully, no panic. The visible lockup ships via the official
        // raster assets (app-icon-*.png / brand::logo, QBRAND-3).
        let img = icon_image(IconId::Wordmark, 48, TINT).expect("wordmark parses + rasterizes");
        assert_eq!(img.height, 48);
        assert_eq!(img.width, 83, "320×184 aspect at 48px tall");
        let [w, h] = img.size_usize();
        assert_eq!(img.rgba.len(), w * h * 4);
        assert_eq!(
            opaque_pixels(&img.rgba),
            0,
            "text lockup unexpectedly rendered without a fontdb"
        );
    }

    #[test]
    fn zero_size_is_an_error_not_a_panic() {
        assert_eq!(icon_image(IconId::Mark, 0, TINT), Err(IconError::ZeroSize));
    }

    #[test]
    fn ids_names_and_sources_are_distinct_and_exhaustive() {
        // Guards a copy-paste slip in the two match tables: 20 ids, 20 unique
        // names, 20 unique embedded sources, all valid-looking SVG.
        let mut names: Vec<&str> = IconId::ALL.iter().map(|id| id.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), IconId::ALL.len(), "duplicate glyph name");

        let mut sources: Vec<&str> = IconId::ALL.iter().map(|id| id.svg()).collect();
        sources.sort_unstable();
        sources.dedup();
        assert_eq!(sources.len(), IconId::ALL.len(), "duplicate glyph source");

        for id in IconId::ALL {
            assert!(id.svg().starts_with("<svg"), "{id:?} source is not SVG");
        }
    }
}
