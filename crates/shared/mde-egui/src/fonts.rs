//! Default fonts for every E12 surface — **Droid Sans Mono** (governance §4: the
//! shared `Style` is the single source of look, and the font set is part of it).
//!
//! Droid Sans Mono (Apache License 2.0 — `assets/fonts/DroidSansMono-NOTICE.txt`)
//! is **embedded** so every surface renders identically on the immutable bootc
//! image, with no dependency on a system-installed font. It is installed as the
//! **default for both the Proportional and Monospace families**, with egui's
//! built-in fonts kept *after* it as glyph fallback (emoji / CJK coverage Droid
//! Sans Mono lacks).

use std::sync::Arc;

use egui::{Context, FontData, FontDefinitions, FontFamily};

/// The embedded Droid Sans Mono face (Apache License 2.0).
const DROID_SANS_MONO: &[u8] = include_bytes!("../assets/fonts/DroidSansMono.ttf");

/// Key for the Droid Sans Mono face in egui's font map.
const DROID_SANS_MONO_KEY: &str = "DroidSansMono";

/// Install Droid Sans Mono as the default font on `ctx`. Called from
/// [`crate::Style::install`], so every surface that uses the shared `Style` gets
/// it for free.
pub fn install(ctx: &Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        DROID_SANS_MONO_KEY.to_owned(),
        Arc::new(FontData::from_static(DROID_SANS_MONO)),
    );
    // Droid Sans Mono first in BOTH families so it is the default everywhere;
    // egui's built-ins stay after it as fallback for glyphs Droid Sans Mono
    // doesn't cover.
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, DROID_SANS_MONO_KEY.to_owned());
    }
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    #[test]
    fn droid_sans_mono_is_embedded_and_valid() {
        // include_bytes! resolved a real, non-empty TrueType face (magic 0x00010000)
        // — not a stray/missing file.
        assert!(
            super::DROID_SANS_MONO.len() > 50_000,
            "Droid Sans Mono TTF looks too small ({} bytes)",
            super::DROID_SANS_MONO.len()
        );
        assert_eq!(&super::DROID_SANS_MONO[0..4], &[0x00, 0x01, 0x00, 0x00]);
    }

    #[test]
    fn install_does_not_panic_headless() {
        // Registering the font set must work without a GPU (CPU-only Context).
        super::install(&egui::Context::default());
    }
}
