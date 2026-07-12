//! Default fonts for every E12 surface (governance §4: the shared `Style` is the
//! single source of look, and the font set is part of it).
//!
//! The platform is **mono-first** (design lock #3): **IBM Plex Mono** (SIL
//! OFL-1.1 — `assets/fonts/IBMPlexMono-OFL.txt`) is the primary monospace face —
//! headings, nav, data/metrics/IDs, terminals, editors, code. **Inter** (SIL
//! OFL-1.1 — `assets/fonts/Inter-OFL.txt`) stays the proportional face for humanist
//! **prose** (this contains the reflow blast radius — surfaces opt individual
//! roles into mono via the [`FontFamily::Name`] families below rather than a global
//! swap). **Intel One Mono** (SIL OFL-1.1) is kept as the monospace fallback rung
//! for any glyph Plex lacks. All faces embed on the immutable bootc image, so every
//! surface renders identically with no system-installed-font dependency; egui's
//! built-in fonts stay last for emoji / CJK coverage.
//!
//! Named families [`FontFamily::Name("heading")`] / [`FontFamily::Name("nav")`]
//! both resolve to IBM Plex Mono, so a surface opts a heading or nav label into the
//! mono identity without switching its call site to `.monospace()`.

use std::sync::Arc;

use egui::{Context, FontData, FontDefinitions, FontFamily};

/// The embedded Inter variable face (SIL OFL-1.1), a TrueType `.ttf`.
const INTER: &[u8] = include_bytes!("../assets/fonts/Inter.ttf");

/// The embedded **IBM Plex Mono** face (SIL OFL-1.1), a TrueType `.ttf` — the
/// mono-first platform identity face (design lock #3).
const IBM_PLEX_MONO: &[u8] = include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf");

/// The embedded Intel One Mono face (SIL OFL-1.1), an OpenType/CFF `.otf` — kept
/// as the monospace fallback rung behind IBM Plex Mono.
const INTEL_ONE_MONO: &[u8] = include_bytes!("../assets/fonts/IntelOneMono-Regular.otf");

/// Key for the Inter face in egui's font map.
const INTER_KEY: &str = "Inter";

/// Key for the IBM Plex Mono face in egui's font map.
const IBM_PLEX_MONO_KEY: &str = "IBMPlexMono";

/// Key for the Intel One Mono face in egui's font map.
const INTEL_ONE_MONO_KEY: &str = "IntelOneMono";

/// The named families a surface can opt a role into so mono-first headings/nav do
/// not require touching every call site (design lock #3). Both resolve to IBM Plex
/// Mono; kept here so no surface mints a bespoke family name.
pub const HEADING_FAMILY: &str = "heading";
/// See [`HEADING_FAMILY`].
pub const NAV_FAMILY: &str = "nav";

/// Install the platform font set on `ctx`. Called from [`crate::Style::install`],
/// so every surface that uses the shared `Style` gets it for free.
pub fn install(ctx: &Context) {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert(INTER_KEY.to_owned(), Arc::new(FontData::from_static(INTER)));
    fonts.font_data.insert(
        IBM_PLEX_MONO_KEY.to_owned(),
        Arc::new(FontData::from_static(IBM_PLEX_MONO)),
    );
    fonts.font_data.insert(
        INTEL_ONE_MONO_KEY.to_owned(),
        Arc::new(FontData::from_static(INTEL_ONE_MONO)),
    );

    // Proportional (prose) stays Inter — contains the reflow to per-surface opt-in.
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, INTER_KEY.to_owned());

    // Monospace is now IBM Plex Mono primary (mono-first identity), Intel One Mono
    // second as the glyph fallback, then egui's built-in mono.
    let mono = fonts.families.entry(FontFamily::Monospace).or_default();
    mono.insert(0, IBM_PLEX_MONO_KEY.to_owned());
    mono.insert(1, INTEL_ONE_MONO_KEY.to_owned());

    // Named role families → the mono identity, so a surface opts a heading/nav label
    // into Plex Mono without switching to `.monospace()` (Plex → Intel fallback →
    // egui built-ins for emoji/CJK).
    for role in [HEADING_FAMILY, NAV_FAMILY] {
        fonts.families.insert(
            FontFamily::Name(Arc::from(role)),
            vec![IBM_PLEX_MONO_KEY.to_owned(), INTEL_ONE_MONO_KEY.to_owned()],
        );
    }

    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use egui::{FontFamily, FontId};
    use std::sync::Arc;

    #[test]
    fn platform_fonts_are_embedded_and_valid() {
        // include_bytes! resolved real, non-empty font files — not stray/missing
        // paths. Inter + IBM Plex Mono are TrueType faces (`0x00010000`); Intel One
        // Mono is an OpenType/CFF face (`OTTO`).
        assert!(
            super::INTER.len() > 500_000,
            "Inter TTF looks too small ({} bytes)",
            super::INTER.len()
        );
        assert_eq!(&super::INTER[0..4], &[0x00, 0x01, 0x00, 0x00]);
        assert!(
            super::IBM_PLEX_MONO.len() > 100_000,
            "IBM Plex Mono TTF looks too small ({} bytes)",
            super::IBM_PLEX_MONO.len()
        );
        assert_eq!(&super::IBM_PLEX_MONO[0..4], &[0x00, 0x01, 0x00, 0x00]);
        assert!(
            super::INTEL_ONE_MONO.len() > 50_000,
            "Intel One Mono OTF looks too small ({} bytes)",
            super::INTEL_ONE_MONO.len()
        );
        assert_eq!(&super::INTEL_ONE_MONO[0..4], b"OTTO");
    }

    #[test]
    fn install_parses_and_lays_out_headless() {
        // Registering the font set must work without a GPU (CPU-only Context), and a
        // frame that lays out text in the proportional, monospace, AND the mono-first
        // named "heading" family must succeed — this forces egui to actually parse
        // the embedded faces (set_fonts alone defers parsing to the first frame).
        let ctx = egui::Context::default();
        super::install(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.label("proportional prose glyphs");
                ui.monospace("monospace glyphs");
                let heading = FontId::new(16.0, FontFamily::Name(Arc::from(super::HEADING_FAMILY)));
                ui.label(egui::RichText::new("mono heading").font(heading));
            });
        });
    }
}
