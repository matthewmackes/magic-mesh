//! Default fonts for every E12 surface (governance §4: the shared `Style` is the
//! single source of look, and the font set is part of it).
//!
//! Inter (SIL OFL-1.1 — `assets/fonts/Inter-OFL.txt`) is embedded as the
//! platform proportional UI face so every surface renders identically on the
//! immutable bootc image, with no dependency on a system-installed font.
//! Intel One Mono (SIL OFL-1.1 — `assets/fonts/IntelOneMono-NOTICE.txt`) remains
//! embedded as the default monospace face for terminals, editors, code previews,
//! and other places where fixed-width glyphs are required. egui's built-in fonts
//! stay after both faces as glyph fallback (emoji / CJK coverage).

use std::sync::Arc;

use egui::{Context, FontData, FontDefinitions, FontFamily};

/// The embedded Inter variable face (SIL OFL-1.1), a TrueType `.ttf`.
const INTER: &[u8] = include_bytes!("../assets/fonts/Inter.ttf");

/// The embedded Intel One Mono face (SIL OFL-1.1), an OpenType/CFF `.otf` —
/// egui's `ttf-parser` backend reads CFF outlines natively.
const INTEL_ONE_MONO: &[u8] = include_bytes!("../assets/fonts/IntelOneMono-Regular.otf");

/// Key for the Inter face in egui's font map.
const INTER_KEY: &str = "Inter";

/// Key for the Intel One Mono face in egui's font map.
const INTEL_ONE_MONO_KEY: &str = "IntelOneMono";

/// Install the platform font set on `ctx`. Called from [`crate::Style::install`],
/// so every surface that uses the shared `Style` gets it for free.
pub fn install(ctx: &Context) {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert(INTER_KEY.to_owned(), Arc::new(FontData::from_static(INTER)));
    fonts.font_data.insert(
        INTEL_ONE_MONO_KEY.to_owned(),
        Arc::new(FontData::from_static(INTEL_ONE_MONO)),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, INTER_KEY.to_owned());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, INTEL_ONE_MONO_KEY.to_owned());
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    #[test]
    fn platform_fonts_are_embedded_and_valid() {
        // include_bytes! resolved real, non-empty font files — not stray/missing
        // paths. Inter is a TrueType face (`0x00010000`); Intel One Mono is an
        // OpenType/CFF face (`OTTO`).
        assert!(
            super::INTER.len() > 500_000,
            "Inter TTF looks too small ({} bytes)",
            super::INTER.len()
        );
        assert_eq!(&super::INTER[0..4], &[0x00, 0x01, 0x00, 0x00]);
        assert!(
            super::INTEL_ONE_MONO.len() > 50_000,
            "Intel One Mono OTF looks too small ({} bytes)",
            super::INTEL_ONE_MONO.len()
        );
        assert_eq!(&super::INTEL_ONE_MONO[0..4], b"OTTO");
    }

    #[test]
    fn install_parses_and_lays_out_headless() {
        // Registering the font set must work without a GPU (CPU-only Context),
        // and a frame that lays out text in BOTH families must succeed — this
        // forces egui to actually parse the embedded faces (set_fonts alone
        // defers parsing to the first frame).
        let ctx = egui::Context::default();
        super::install(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.label("proportional glyphs");
                ui.monospace("monospace glyphs");
            });
        });
    }
}
