//! Default fonts for every E12 surface — **Intel One Mono** (governance §4: the
//! shared `Style` is the single source of look, and the font set is part of it).
//!
//! Intel One Mono (SIL OFL-1.1 — `assets/fonts/IntelOneMono-NOTICE.txt`) is
//! **embedded** so every surface renders identically on the immutable bootc
//! image, with no dependency on a system-installed font. It is installed as the
//! **default for both the Proportional and Monospace families**, with egui's
//! built-in fonts kept *after* it as glyph fallback (emoji / CJK coverage Intel
//! One Mono lacks).

use std::sync::Arc;

use egui::{Context, FontData, FontDefinitions, FontFamily};

/// The embedded Intel One Mono face (SIL OFL-1.1), an OpenType/CFF `.otf` —
/// egui's `ttf-parser` backend reads CFF outlines natively.
const INTEL_ONE_MONO: &[u8] = include_bytes!("../assets/fonts/IntelOneMono-Regular.otf");

/// Key for the Intel One Mono face in egui's font map.
const INTEL_ONE_MONO_KEY: &str = "IntelOneMono";

/// Install Intel One Mono as the default font on `ctx`. Called from
/// [`crate::Style::install`], so every surface that uses the shared `Style` gets
/// it for free.
pub fn install(ctx: &Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        INTEL_ONE_MONO_KEY.to_owned(),
        Arc::new(FontData::from_static(INTEL_ONE_MONO)),
    );
    // Intel One Mono first in BOTH families so it is the default everywhere;
    // egui's built-ins stay after it as fallback for glyphs Intel One Mono
    // doesn't cover.
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, INTEL_ONE_MONO_KEY.to_owned());
    }
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    #[test]
    fn intel_one_mono_is_embedded_and_valid() {
        // include_bytes! resolved a real, non-empty OpenType/CFF face (magic
        // `OTTO`) — not a stray/missing file.
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
        // forces egui to actually parse the embedded CFF face (set_fonts alone
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
