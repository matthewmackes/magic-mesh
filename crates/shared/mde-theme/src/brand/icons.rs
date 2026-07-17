//! `brand::icons` — the monochrome Quazar line-art icon set (QBRAND-2).
//!
//! The 39 brand glyphs (`assets/brand/quasar/*.svg`, QBRAND-10 + the
//! NAVBAR-W10-1 tray set) embedded as
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

/// Embed one Quazar brand SVG from `assets/brand/quasar/` at compile time.
macro_rules! quasar_svg {
    ($file:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../assets/brand/quasar/",
            $file
        ))
    };
}

/// Identifier for every glyph in the Quazar brand set.
///
/// The product marks, the 18 dock/surface glyphs, the 3 node-role badges, the
/// 14 Win10-taskbar tray glyphs (NAVBAR-W10-1, tuned to stay legible rasterized
/// at 16px), and shared UI action glyphs — one variant per SVG in
/// `assets/brand/quasar/`;
/// [`IconId::svg`] resolves the embedded source.
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
    /// The Bookmarks manager surface glyph.
    Bookmarks,
    /// The Terminal surface glyph.
    Terminal,
    /// The Editor (code editor) surface glyph.
    Editor,
    /// The Chat surface glyph.
    Chat,
    /// The Phones hub surface glyph — a smartphone outline (KDC-MESH-9).
    Phones,
    /// The System surface glyph.
    System,
    /// The Storage surface glyph.
    Storage,
    /// The Mesh-View (topology map) surface glyph.
    MeshView,
    /// The Settings (host-controls) surface glyph — a toothed cog. Distinct from
    /// the spoked [`System`](Self::System) glyph; the dock's right-side Settings
    /// button (PICKER-2) draws this gear.
    Settings,
    /// Shared UI: search/magnifier glyph for compact search fields.
    Search,
    /// Shared UI: close/clear `x` glyph for compact dismiss and clear buttons.
    Close,
    /// The Workstation role badge.
    Workstation,
    /// The Server role badge.
    Server,
    /// The Lighthouse role badge.
    Lighthouse,
    /// Tray: mesh signal strength — four ascending bars.
    Signal,
    /// Tray: active VDI session — a monitor carrying a two-node link mark
    /// (the Desktop monitor + a connection, per NAVBAR-W10 W2/W10).
    Sessions,
    /// Tray: Start / Advanced menu — the Win10-style left rail menu glyph.
    Start,
    /// Tray: dock pin — holds the left launcher rail open.
    Pin,
    /// Tray: the overflow-flyout `^` chevron (NAVBAR-W10 W10/W13).
    ChevronUp,
    /// Tray: speaker with sound-wave arcs (volume).
    Volume,
    /// Tray: speaker with an `×` mark — the muted state for the volume
    /// micro-flyout (NAVBAR-W10 W7).
    VolumeMuted,
    /// Tray: the Bluetooth rune, drawn 12 units wide so the crossing strokes
    /// stay separable at 16px (no prior BT glyph existed in the set).
    BluetoothSmall,
    /// Tray: battery outline, no charge fill (the empty step of the W8
    /// fill-level ladder; the other steps share this exact outline).
    BatteryEmpty,
    /// Tray: battery at ~25% — the shared outline + a 4-unit fill bar.
    BatteryQuarter,
    /// Tray: battery at ~50% — the shared outline + an 8-unit fill bar.
    BatteryHalf,
    /// Tray: battery at ~75% — the shared outline + a 12-unit fill bar.
    BatteryThreeQuarter,
    /// Tray: battery at 100% — the shared outline + the full 16-unit fill bar.
    BatteryFull,
    /// Tray: a standalone solid charge bolt sized to overlay any
    /// `Battery*` glyph at the same raster size (the Win10 idiom: the bolt
    /// spans the icon, overflowing the outline) — draw the fill-level glyph,
    /// then this on top while charging. Also reads alone as "charging".
    BatteryBolt,
}

impl IconId {
    /// Every glyph in the set, for exhaustive iteration (dock catalogs, tests).
    pub const ALL: [Self; 39] = [
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
        Self::Bookmarks,
        Self::Terminal,
        Self::Editor,
        Self::Chat,
        Self::Phones,
        Self::System,
        Self::Storage,
        Self::MeshView,
        Self::Settings,
        Self::Search,
        Self::Close,
        Self::Workstation,
        Self::Server,
        Self::Lighthouse,
        Self::Signal,
        Self::Sessions,
        Self::Start,
        Self::Pin,
        Self::ChevronUp,
        Self::Volume,
        Self::VolumeMuted,
        Self::BluetoothSmall,
        Self::BatteryEmpty,
        Self::BatteryQuarter,
        Self::BatteryHalf,
        Self::BatteryThreeQuarter,
        Self::BatteryFull,
        Self::BatteryBolt,
    ];

    /// The tray glyph subset (NAVBAR-W10-1) — every glyph the 40px taskbar's
    /// tray renders at 16px, for targeted iteration in the tray and its tests.
    pub const TRAY: [Self; 14] = [
        Self::Signal,
        Self::Sessions,
        Self::Start,
        Self::Pin,
        Self::ChevronUp,
        Self::Volume,
        Self::VolumeMuted,
        Self::BluetoothSmall,
        Self::BatteryEmpty,
        Self::BatteryQuarter,
        Self::BatteryHalf,
        Self::BatteryThreeQuarter,
        Self::BatteryFull,
        Self::BatteryBolt,
    ];

    /// The embedded SVG source for this glyph — `currentColor` line-art in a
    /// square viewBox (`0 0 32 32` for the surface/role/node/tray glyphs, `0
    /// 0 64 64` for the mark); the wordmark alone is a `0 0 320 184` text
    /// lockup.
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
            Self::Bookmarks => quasar_svg!("surface-bookmarks.svg"),
            Self::Terminal => quasar_svg!("surface-terminal.svg"),
            Self::Editor => quasar_svg!("surface-editor.svg"),
            Self::Chat => quasar_svg!("surface-chat.svg"),
            Self::Phones => quasar_svg!("surface-phones.svg"),
            Self::System => quasar_svg!("surface-system.svg"),
            Self::Storage => quasar_svg!("surface-storage.svg"),
            Self::MeshView => quasar_svg!("surface-mesh-view.svg"),
            Self::Settings => quasar_svg!("surface-settings.svg"),
            Self::Search => quasar_svg!("ui-search.svg"),
            Self::Close => quasar_svg!("ui-close.svg"),
            Self::Workstation => quasar_svg!("role-workstation.svg"),
            Self::Server => quasar_svg!("role-server.svg"),
            Self::Lighthouse => quasar_svg!("role-lighthouse.svg"),
            Self::Signal => quasar_svg!("tray-signal.svg"),
            Self::Sessions => quasar_svg!("tray-sessions.svg"),
            Self::Start => quasar_svg!("tray-start.svg"),
            Self::Pin => quasar_svg!("tray-pin.svg"),
            Self::ChevronUp => quasar_svg!("tray-chevron-up.svg"),
            Self::Volume => quasar_svg!("tray-volume.svg"),
            Self::VolumeMuted => quasar_svg!("tray-volume-muted.svg"),
            Self::BluetoothSmall => quasar_svg!("tray-bluetooth-small.svg"),
            Self::BatteryEmpty => quasar_svg!("tray-battery-empty.svg"),
            Self::BatteryQuarter => quasar_svg!("tray-battery-quarter.svg"),
            Self::BatteryHalf => quasar_svg!("tray-battery-half.svg"),
            Self::BatteryThreeQuarter => quasar_svg!("tray-battery-three-quarter.svg"),
            Self::BatteryFull => quasar_svg!("tray-battery-full.svg"),
            Self::BatteryBolt => quasar_svg!("tray-battery-bolt.svg"),
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
            Self::Bookmarks => "surface-bookmarks",
            Self::Terminal => "surface-terminal",
            Self::Editor => "surface-editor",
            Self::Chat => "surface-chat",
            Self::Phones => "surface-phones",
            Self::System => "surface-system",
            Self::Storage => "surface-storage",
            Self::MeshView => "surface-mesh-view",
            Self::Settings => "surface-settings",
            Self::Search => "ui-search",
            Self::Close => "ui-close",
            Self::Workstation => "role-workstation",
            Self::Server => "role-server",
            Self::Lighthouse => "role-lighthouse",
            Self::Signal => "tray-signal",
            Self::Sessions => "tray-sessions",
            Self::Start => "tray-start",
            Self::Pin => "tray-pin",
            Self::ChevronUp => "tray-chevron-up",
            Self::Volume => "tray-volume",
            Self::VolumeMuted => "tray-volume-muted",
            Self::BluetoothSmall => "tray-bluetooth-small",
            Self::BatteryEmpty => "tray-battery-empty",
            Self::BatteryQuarter => "tray-battery-quarter",
            Self::BatteryHalf => "tray-battery-half",
            Self::BatteryThreeQuarter => "tray-battery-three-quarter",
            Self::BatteryFull => "tray-battery-full",
            Self::BatteryBolt => "tray-battery-bolt",
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
    fn tray_glyphs_rasterize_nonempty_at_16_and_24() {
        // NAVBAR-W10-1 §7 gate: the Win10 tray draws these at exactly 16px
        // (24px covers the flyout/hi-DPI step) — every glyph must come back
        // square, correctly sized and with real ink through the same
        // icon_image loader the shell uses.
        assert_eq!(IconId::TRAY.len(), 14, "tray subset size");
        for id in IconId::TRAY {
            for size in [16_u32, 24] {
                let img = icon_image(id, size, TINT)
                    .unwrap_or_else(|err| panic!("tray {id:?} @ {size}px failed: {err}"));
                assert_eq!(img.height, size, "tray {id:?} @ {size}px height");
                assert_eq!(img.width, size, "tray glyphs are square, {id:?}");
                assert!(
                    opaque_pixels(&img.rgba) > 0,
                    "tray {id:?} @ {size}px rasterized empty"
                );
            }
        }
    }

    #[test]
    fn battery_fill_ladder_is_strictly_monotonic_at_16px() {
        // The W8 fill-level ladder shares one outline and varies only the
        // fill bar, so at tray size (16px) each step must ink strictly more
        // pixels than the one below — proving the five levels stay visually
        // distinct where it matters.
        let ladder = [
            IconId::BatteryEmpty,
            IconId::BatteryQuarter,
            IconId::BatteryHalf,
            IconId::BatteryThreeQuarter,
            IconId::BatteryFull,
        ];
        let inked: Vec<usize> = ladder
            .iter()
            .map(|&id| {
                let img =
                    icon_image(id, 16, TINT).unwrap_or_else(|err| panic!("{id:?} failed: {err}"));
                opaque_pixels(&img.rgba)
            })
            .collect();
        for pair in inked.windows(2) {
            assert!(
                pair[0] < pair[1],
                "battery fill ladder not monotonic at 16px: {inked:?}"
            );
        }
    }

    #[test]
    fn zero_size_is_an_error_not_a_panic() {
        assert_eq!(icon_image(IconId::Mark, 0, TINT), Err(IconError::ZeroSize));
    }

    #[test]
    fn ids_names_and_sources_are_distinct_and_exhaustive() {
        // Guards a copy-paste slip in the two match tables: 36 ids, 36 unique
        // names, 36 unique embedded sources, all valid-looking SVG.
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
