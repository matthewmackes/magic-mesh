//! Default fonts for every E12 surface — **Fira Code** (governance §4: the shared
//! `Style` is the single source of look, and the font set is part of it).
//!
//! Fira Code (SIL Open Font License — `assets/fonts/OFL.txt`) is **embedded** so
//! every surface renders identically on the immutable bootc image, with no
//! dependency on a system-installed font. It is installed as the **default for both
//! the Proportional and Monospace families**, with egui's built-in fonts kept
//! *after* it as glyph fallback (emoji / CJK coverage Fira Code lacks).

use std::sync::Arc;

use egui::{Context, FontData, FontDefinitions, FontFamily};

/// The embedded Fira Code Regular face (SIL Open Font License, v6.2).
const FIRA_CODE: &[u8] = include_bytes!("../assets/fonts/FiraCode-Regular.ttf");

/// Key for the Fira Code face in egui's font map.
const FIRA_CODE_KEY: &str = "FiraCode";

/// Install Fira Code as the default font on `ctx`. Called from
/// [`crate::Style::install`], so every surface that uses the shared `Style` gets
/// it for free.
pub fn install(ctx: &Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        FIRA_CODE_KEY.to_owned(),
        Arc::new(FontData::from_static(FIRA_CODE)),
    );
    // Fira Code first in BOTH families so it is the default everywhere; egui's
    // built-ins stay after it as fallback for glyphs Fira Code doesn't cover.
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, FIRA_CODE_KEY.to_owned());
    }
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    #[test]
    fn fira_code_is_embedded_and_valid() {
        // include_bytes! resolved a real, non-empty TrueType face (magic 0x00010000)
        // — not a stray/missing file.
        assert!(
            super::FIRA_CODE.len() > 50_000,
            "Fira Code TTF looks too small ({} bytes)",
            super::FIRA_CODE.len()
        );
        assert_eq!(&super::FIRA_CODE[0..4], &[0x00, 0x01, 0x00, 0x00]);
    }

    #[test]
    fn install_does_not_panic_headless() {
        // Registering the font set must work without a GPU (CPU-only Context).
        super::install(&egui::Context::default());
    }
}
