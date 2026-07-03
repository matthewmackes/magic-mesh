//! The terminal grid's bundled programming font (TERM-13).
//!
//! **Fira Code** (SIL Open Font License — `assets/fonts/OFL.txt`) is embedded in
//! the crate so the terminal grid renders a crisp monospace face with
//! **programming ligatures** on the immutable bootc image, with no dependency on
//! a system-installed font. The grid paints through `FontId::monospace` (see
//! [`crate::widget`]); [`install`] registers the embedded face at the **highest
//! priority** of egui's `Monospace` family, so the grid — and the terminal's own
//! monospace chrome — draw with the bundled ligature font.
//!
//! [`install`] uses `Context::add_font`, which is **additive**: it layers the
//! face onto whatever the shared `Style::install` already set (§4), rather than
//! replacing the font set, so the Proportional family and the emoji/CJK
//! fallbacks stay intact.

use mde_egui::egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
use mde_egui::egui::{Context, FontData, FontFamily};

/// The embedded Fira Code Regular face (SIL Open Font License, v6.2) — a
/// monospace font with programming ligatures.
const FIRA_CODE: &[u8] = include_bytes!("../assets/fonts/FiraCode-Regular.ttf");

/// Key for the bundled terminal face in egui's font map.
const TERM_FONT_KEY: &str = "mde-term-ligature";

/// Register the bundled ligature font as the terminal grid's monospace face.
///
/// Called once from the surface binary's setup (after `Style::install`); the
/// call is idempotent — egui skips the reload once the face is present — so an
/// embedder may safely call it again.
pub fn install(ctx: &Context) {
    ctx.add_font(FontInsert::new(
        TERM_FONT_KEY,
        FontData::from_static(FIRA_CODE),
        // Highest in Monospace: the grid's `FontId::monospace` glyphs resolve to
        // the bundled ligature face first, egui's built-ins staying as fallback.
        vec![InsertFontFamily {
            family: FontFamily::Monospace,
            priority: FontPriority::Highest,
        }],
    ));
}

#[cfg(test)]
mod tests {
    use super::{install, FIRA_CODE};

    #[test]
    fn fira_code_ligature_face_is_embedded_and_valid() {
        // include_bytes! resolved a real, non-empty TrueType face (magic
        // 0x00010000) — not a stray/missing asset.
        assert!(
            FIRA_CODE.len() > 50_000,
            "the bundled ligature TTF looks too small ({} bytes)",
            FIRA_CODE.len()
        );
        assert_eq!(&FIRA_CODE[0..4], &[0x00, 0x01, 0x00, 0x00]);
    }

    #[test]
    fn install_registers_the_grid_font_headless() {
        // Registering the terminal face must work without a GPU (CPU-only
        // Context) and never panic.
        install(&mde_egui::egui::Context::default());
    }
}
