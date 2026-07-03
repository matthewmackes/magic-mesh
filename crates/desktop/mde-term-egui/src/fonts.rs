//! The terminal grid's bundled monospace font.
//!
//! **Droid Sans Mono** (Apache License 2.0 — `assets/fonts/DroidSansMono-NOTICE.txt`)
//! is embedded in the crate so the terminal grid renders a crisp, clean monospace
//! face on the immutable bootc image, with no dependency on a system-installed
//! font. This is the platform default (the shared `Style` installs the same face);
//! it renders a plain monospace with **no programming ligatures** — the terminal
//! traded the earlier Fira Code ligature face (TERM-13) for the platform-default
//! Droid Sans Mono. The grid paints through `FontId::monospace` (see
//! [`crate::widget`]); [`install`] registers the embedded face at the **highest
//! priority** of egui's `Monospace` family, so the grid — and the terminal's own
//! monospace chrome — draw with the bundled font.
//!
//! [`install`] uses `Context::add_font`, which is **additive**: it layers the
//! face onto whatever the shared `Style::install` already set (§4), rather than
//! replacing the font set, so the Proportional family and the emoji/CJK
//! fallbacks stay intact.

use mde_egui::egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
use mde_egui::egui::{Context, FontData, FontFamily};

/// The embedded Droid Sans Mono face (Apache License 2.0) — a clean monospace
/// font (no programming ligatures).
const DROID_SANS_MONO: &[u8] = include_bytes!("../assets/fonts/DroidSansMono.ttf");

/// Key for the bundled terminal face in egui's font map.
const TERM_FONT_KEY: &str = "mde-term-mono";

/// Register the bundled monospace font as the terminal grid's monospace face.
///
/// Called once from the surface binary's setup (after `Style::install`); the
/// call is idempotent — egui skips the reload once the face is present — so an
/// embedder may safely call it again.
pub fn install(ctx: &Context) {
    ctx.add_font(FontInsert::new(
        TERM_FONT_KEY,
        FontData::from_static(DROID_SANS_MONO),
        // Highest in Monospace: the grid's `FontId::monospace` glyphs resolve to
        // the bundled face first, egui's built-ins staying as fallback.
        vec![InsertFontFamily {
            family: FontFamily::Monospace,
            priority: FontPriority::Highest,
        }],
    ));
}

#[cfg(test)]
mod tests {
    use super::{install, DROID_SANS_MONO};

    #[test]
    fn droid_sans_mono_face_is_embedded_and_valid() {
        // include_bytes! resolved a real, non-empty TrueType face (magic
        // 0x00010000) — not a stray/missing asset.
        assert!(
            DROID_SANS_MONO.len() > 50_000,
            "the bundled monospace TTF looks too small ({} bytes)",
            DROID_SANS_MONO.len()
        );
        assert_eq!(&DROID_SANS_MONO[0..4], &[0x00, 0x01, 0x00, 0x00]);
    }

    #[test]
    fn install_registers_the_grid_font_headless() {
        // Registering the terminal face must work without a GPU (CPU-only
        // Context) and never panic.
        install(&mde_egui::egui::Context::default());
    }
}
