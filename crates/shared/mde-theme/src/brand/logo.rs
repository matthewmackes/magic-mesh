//! `brand::logo` — the product mark + wordmark LOCKUP (QBRAND-3).
//!
//! The shell's one-stop for the platform identity lockup: the horizontal
//! mark-left/wordmark-right artwork the About panel headers with (design lock
//! #13, [`MDE-QUAZAR-MAIN.png`]), the tintable vector mark for chrome, and the
//! user-facing product name/tagline (lock #10) that the About panel (QBRAND-6)
//! and chrome (QBRAND-5) print.
//!
//! ## Rasters are the source of truth for the *visible* lockup
//!
//! The mark and wordmark SVGs ([`icons`](super::icons)) render imperfectly on
//! their own — the wordmark is pure `<text>` and rasterizes transparent under
//! this crate's fontdb-free `resvg` (see the icons module docs). So the
//! official artwork PNGs — not the SVGs — are the source of truth for the
//! composed lockup the operator sees. This module wraps them so surfaces read
//! one canonical copy rather than each hard-coding an asset path.
//!
//! ## Embed vs. reference — the centered splash lockup lives elsewhere
//!
//! Design lock #11 paints the boot-splash from [`MDE-QUAZAR-WALLPAPER1.png`]
//! (the centered mark + wordmark + loading bar). That ≈470 KB raster is
//! **deliberately not embedded here**: the boot-splash (QBRAND-4,
//! `mde-shell-egui/src/splash.rs`) already `include_bytes!`es it directly, and
//! `include_bytes!` is not deduplicated across compilation units by default, so
//! a second copy here would double the blob in the shell binary for no gain.
//! [`SPLASH_LOCKUP_ASSET`] names the canonical asset (a reference, not a blob)
//! for packaging and discoverability; only the About-header lockup — which no
//! other module owns — is embedded, via [`lockup_horizontal`].
//!
//! ## Toolkit-free
//!
//! Like the rest of `mde-theme` (QBRAND lock #4), this module never pulls a GUI
//! dependency: raster accessors hand back the raw PNG bytes and [`mark_rgba`]
//! returns a plain [`IconImage`], leaving the egui texture upload to the shell.
//!
//! [`MDE-QUAZAR-MAIN.png`]: lockup_horizontal
//! [`MDE-QUAZAR-WALLPAPER1.png`]: SPLASH_LOCKUP_ASSET

use super::icons::{self, IconError, IconId, IconImage};

/// The user-facing product name (design lock #10).
///
/// Shown in the About panel, the boot-splash and the window chrome;
/// `magic-mesh` stays the infra/mesh and package name underneath (the GNOME
/// vs. `gnome-shell` split).
pub const PRODUCT_NAME: &str = "MDE Quazar";

/// The product tagline / expansion of the name (design lock #10) — the
/// "Mackes Display Environment" line the About panel sets beneath the lockup.
pub const PRODUCT_TAGLINE: &str = "Mackes Display Environment";

/// The workspace-relative path to the centered boot-splash lockup
/// (`MDE-QUAZAR-WALLPAPER1.png`, design lock #11).
///
/// This is a **reference, not an embedded blob**: the boot-splash (QBRAND-4)
/// owns the `include_bytes!` of this asset, so re-embedding it here would
/// duplicate ≈470 KB in the binary (see the module docs). Packaging scripts and
/// callers that need the on-disk path resolve it through this const.
pub const SPLASH_LOCKUP_ASSET: &str = "assets/brand/MDE-QUAZAR-WALLPAPER1.png";

/// The 8-byte PNG signature (`\x89PNG\r\n\x1a\n`) every embedded raster begins
/// with — used by the tests to prove the accessors return real PNG data.
pub const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// The horizontal product lockup — the official mark-left / wordmark-right
/// artwork (`MDE-QUAZAR-MAIN.png`), embedded at compile time.
///
/// This is the About-panel header image (design lock #13). Returned as raw PNG
/// bytes; the shell decodes and uploads it as a texture.
#[must_use]
pub const fn lockup_horizontal() -> &'static [u8] {
    include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../assets/brand/MDE-QUAZAR-MAIN.png"
    ))
}

/// The embedded SVG source of the product **mark** (the mesh-node
/// constellation glyph).
///
/// A convenience re-export of [`IconId::Mark`]'s source so chrome wanting the
/// tintable vector mark need not reach into [`icons`](super::icons). Authored
/// in `currentColor`; pass it through [`mark_rgba`] (or [`icons::icon_image`])
/// to rasterize it at a size and tint.
#[must_use]
pub const fn mark_svg() -> &'static str {
    IconId::Mark.svg()
}

/// The embedded SVG source of the stacked **wordmark** lockup — a convenience
/// re-export of [`IconId::Wordmark`]'s source.
///
/// Note: the wordmark is pure `<text>` and rasterizes transparent under this
/// crate's fontdb-free `resvg` (see the icons module docs); the visible
/// wordmark ships via the raster lockups ([`lockup_horizontal`] /
/// [`SPLASH_LOCKUP_ASSET`]). This accessor exists for callers that outline the
/// letterforms or supply their own fontdb.
#[must_use]
pub const fn wordmark_svg() -> &'static str {
    IconId::Wordmark.svg()
}

/// Rasterize the product **mark** at `size_px` tall, tinted `[r, g, b, a]` — a
/// thin wrapper over [`icons::icon_image`] for [`IconId::Mark`].
///
/// The go-to for chrome/watermarks that want the mark in a token color without
/// naming the [`IconId`]. See [`icons::icon_image`] for the tint semantics.
///
/// # Errors
///
/// Propagates [`IconError`] from the rasterizer — [`IconError::ZeroSize`] for a
/// zero `size_px`; a `Parse`/`Alloc` error only on an embedded-asset bug.
pub fn mark_rgba(size_px: u32, tint: [u8; 4]) -> Result<IconImage, IconError> {
    icons::icon_image(IconId::Mark, size_px, tint)
}

#[cfg(test)]
#[allow(clippy::panic)] // tests fail by panicking, with context
mod tests {
    use super::{
        lockup_horizontal, mark_rgba, mark_svg, wordmark_svg, IconId, PNG_MAGIC, PRODUCT_NAME,
        PRODUCT_TAGLINE, SPLASH_LOCKUP_ASSET,
    };

    #[test]
    fn horizontal_lockup_is_a_nonempty_png() {
        let bytes = lockup_horizontal();
        assert!(bytes.len() > PNG_MAGIC.len(), "lockup unexpectedly tiny");
        assert_eq!(&bytes[..8], &PNG_MAGIC, "horizontal lockup is not a PNG");
    }

    #[test]
    fn mark_and_wordmark_svg_reexport_the_icon_sources() {
        // Re-exports must be the exact same embedded source strings, not copies
        // that could drift from the icon set.
        assert_eq!(mark_svg(), IconId::Mark.svg(), "mark re-export drifted");
        assert_eq!(
            wordmark_svg(),
            IconId::Wordmark.svg(),
            "wordmark re-export drifted"
        );
        assert!(mark_svg().starts_with("<svg"), "mark source is not SVG");
        assert!(
            wordmark_svg().starts_with("<svg"),
            "wordmark source is not SVG"
        );
    }

    #[test]
    fn mark_rgba_rasterizes_nonempty() {
        let img = mark_rgba(64, [0xe0, 0xe0, 0xe0, 0xff]).expect("mark rasterizes");
        assert_eq!(img.height, 64);
        assert!(img.width >= 64);
        let [w, h] = img.size_usize();
        assert_eq!(img.rgba.len(), w * h * 4, "buffer len mismatch");
        assert!(
            img.rgba.chunks_exact(4).any(|px| px[3] > 0),
            "mark rasterized empty"
        );
    }

    #[test]
    fn product_name_and_tagline_are_the_locked_strings() {
        // Design lock #10 — the user-facing name/tagline every surface prints.
        assert_eq!(PRODUCT_NAME, "MDE Quazar");
        assert_eq!(PRODUCT_TAGLINE, "Mackes Display Environment");
    }

    #[test]
    fn splash_lockup_reference_points_at_a_real_asset() {
        // The centered lockup is referenced, not embedded (the splash owns the
        // blob). Prove the reference is honest: it names WALLPAPER1 and resolves
        // to a file on disk relative to the crate manifest.
        assert!(
            SPLASH_LOCKUP_ASSET.ends_with("MDE-QUAZAR-WALLPAPER1.png"),
            "splash reference names the wrong asset"
        );
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join(SPLASH_LOCKUP_ASSET);
        assert!(
            path.exists(),
            "splash lockup reference {SPLASH_LOCKUP_ASSET} does not resolve to a file"
        );
    }
}
