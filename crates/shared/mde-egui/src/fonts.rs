//! Default fonts for every E12 surface (governance §4: the shared `Style` is the
//! single source of look, and the font set is part of it).
//!
//! The platform is **Kdam Thmor Pro-first**: **Kdam Thmor Pro** (SIL OFL-1.1 —
//! `assets/fonts/KdamThmorPro-OFL.txt`) is the primary Construct UI face across
//! shell, workspace chrome, headings, nav, prose, and Browser chrome. **Inter**
//! (SIL OFL-1.1 — `assets/fonts/Inter-OFL.txt`) remains embedded as the broad
//! proportional fallback behind Kdam Thmor Pro. **IBM Plex Mono** (SIL OFL-1.1 —
//! `assets/fonts/IBMPlexMono-OFL.txt`) is kept for terminals, code, logs, IDs,
//! metrics, and other fixed-width roles. **Roboto** (SIL OFL-1.1 —
//! `assets/fonts/Roboto-OFL.txt`) remains embedded only as a fallback for older
//! Browser-family references. **Intel One Mono** (SIL OFL-1.1) is kept as the
//! monospace fallback rung for any glyph Plex lacks. All faces embed on the
//! immutable bootc image, so every surface renders identically with no
//! system-installed-font dependency; egui's built-in fonts stay last for emoji /
//! CJK coverage.
//!
//! Named families [`FontFamily::Name("heading")`] / [`FontFamily::Name("nav")`]
//! both resolve to Kdam Thmor Pro, so a surface can name a role without leaving
//! the shared Construct UI face.

use std::sync::Arc;

use egui::{Context, FontData, FontDefinitions, FontFamily};

/// The embedded Kdam Thmor Pro face (SIL OFL-1.1), a TrueType `.ttf`.
const KDAM_THMOR_PRO: &[u8] = include_bytes!("../assets/fonts/KdamThmorPro-Regular.ttf");

/// The embedded Inter variable face (SIL OFL-1.1), a TrueType `.ttf`.
const INTER: &[u8] = include_bytes!("../assets/fonts/Inter.ttf");

/// The embedded **IBM Plex Mono** face (SIL OFL-1.1), a TrueType `.ttf` — the
/// mono-first platform identity face (design lock #3).
const IBM_PLEX_MONO: &[u8] = include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf");

/// The embedded Roboto face (SIL OFL-1.1), retained as a fallback for old
/// Browser-family references.
const ROBOTO: &[u8] = include_bytes!("../assets/fonts/Roboto-Regular.ttf");

/// The embedded Intel One Mono face (SIL OFL-1.1), an OpenType/CFF `.otf` — kept
/// as the monospace fallback rung behind IBM Plex Mono.
const INTEL_ONE_MONO: &[u8] = include_bytes!("../assets/fonts/IntelOneMono-Regular.otf");

/// Key for the Kdam Thmor Pro face in egui's font map.
const KDAM_THMOR_PRO_KEY: &str = "KdamThmorPro";

/// Key for the Inter face in egui's font map.
const INTER_KEY: &str = "Inter";

/// Key for the IBM Plex Mono face in egui's font map.
const IBM_PLEX_MONO_KEY: &str = "IBMPlexMono";

/// Key for the Roboto face in egui's font map.
const ROBOTO_KEY: &str = "Roboto";

/// Key for the Intel One Mono face in egui's font map.
const INTEL_ONE_MONO_KEY: &str = "IntelOneMono";

/// The named families a surface can opt a role into without minting a bespoke family
/// name. Both resolve to Kdam Thmor Pro, the shared Construct UI face.
pub const HEADING_FAMILY: &str = "heading";
/// See [`HEADING_FAMILY`].
pub const NAV_FAMILY: &str = "nav";
/// Browser chrome family name. It resolves to Kdam Thmor Pro first so the Browser
/// uses the same Construct UI face as the rest of the platform.
pub const BROWSER_CHROME_FAMILY: &str = "browser-chrome";

/// Install the platform font set on `ctx`. Called from [`crate::Style::install`],
/// so every surface that uses the shared `Style` gets it for free.
pub fn install(ctx: &Context) {
    ctx.set_fonts(definitions());
}

fn definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        KDAM_THMOR_PRO_KEY.to_owned(),
        Arc::new(FontData::from_static(KDAM_THMOR_PRO)),
    );
    fonts
        .font_data
        .insert(INTER_KEY.to_owned(), Arc::new(FontData::from_static(INTER)));
    fonts.font_data.insert(
        IBM_PLEX_MONO_KEY.to_owned(),
        Arc::new(FontData::from_static(IBM_PLEX_MONO)),
    );
    fonts.font_data.insert(
        ROBOTO_KEY.to_owned(),
        Arc::new(FontData::from_static(ROBOTO)),
    );
    fonts.font_data.insert(
        INTEL_ONE_MONO_KEY.to_owned(),
        Arc::new(FontData::from_static(INTEL_ONE_MONO)),
    );

    // Proportional/UI copy is Kdam Thmor Pro first; Inter remains the broad
    // proportional fallback rung behind it.
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    proportional.insert(0, INTER_KEY.to_owned());
    proportional.insert(0, KDAM_THMOR_PRO_KEY.to_owned());

    // Monospace is now IBM Plex Mono primary (mono-first identity), Intel One Mono
    // second as the glyph fallback, then egui's built-in mono.
    let mono = fonts.families.entry(FontFamily::Monospace).or_default();
    mono.insert(0, IBM_PLEX_MONO_KEY.to_owned());
    mono.insert(1, INTEL_ONE_MONO_KEY.to_owned());

    // Named role families → Kdam Thmor Pro. Fixed-width content must choose
    // Monospace.
    for role in [HEADING_FAMILY, NAV_FAMILY] {
        fonts.families.insert(
            FontFamily::Name(Arc::from(role)),
            vec![
                KDAM_THMOR_PRO_KEY.to_owned(),
                INTER_KEY.to_owned(),
                IBM_PLEX_MONO_KEY.to_owned(),
            ],
        );
    }
    fonts.families.insert(
        FontFamily::Name(Arc::from(BROWSER_CHROME_FAMILY)),
        vec![
            KDAM_THMOR_PRO_KEY.to_owned(),
            INTER_KEY.to_owned(),
            ROBOTO_KEY.to_owned(),
        ],
    );

    fonts
}

#[cfg(test)]
mod tests {
    use egui::{FontFamily, FontId};
    use std::sync::Arc;

    #[test]
    fn platform_fonts_are_embedded_and_valid() {
        // include_bytes! resolved real, non-empty font files — not stray/missing
        // paths. Kdam Thmor Pro, Inter, and IBM Plex Mono are TrueType faces
        // (`0x00010000`); Intel One Mono is an OpenType/CFF face (`OTTO`).
        assert!(
            super::KDAM_THMOR_PRO.len() > 80_000,
            "Kdam Thmor Pro TTF looks too small ({} bytes)",
            super::KDAM_THMOR_PRO.len()
        );
        assert_eq!(&super::KDAM_THMOR_PRO[0..4], &[0x00, 0x01, 0x00, 0x00]);
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
            super::ROBOTO.len() > 150_000,
            "Roboto TTF looks too small ({} bytes)",
            super::ROBOTO.len()
        );
        assert_eq!(&super::ROBOTO[0..4], &[0x00, 0x01, 0x00, 0x00]);
        assert!(
            super::INTEL_ONE_MONO.len() > 50_000,
            "Intel One Mono OTF looks too small ({} bytes)",
            super::INTEL_ONE_MONO.len()
        );
        assert_eq!(&super::INTEL_ONE_MONO[0..4], b"OTTO");
    }

    #[test]
    fn install_sets_kdam_thmor_pro_as_the_platform_ui_face() {
        let fonts = super::definitions();
        assert_eq!(
            fonts
                .families
                .get(&FontFamily::Proportional)
                .expect("proportional family")[0],
            super::KDAM_THMOR_PRO_KEY,
            "proportional UI copy should start with Kdam Thmor Pro"
        );
        assert_eq!(
            fonts
                .families
                .get(&FontFamily::Name(Arc::from(super::HEADING_FAMILY)))
                .expect("heading family")[0],
            super::KDAM_THMOR_PRO_KEY,
            "heading/nav roles should stay on the platform UI face"
        );
        assert_eq!(
            fonts
                .families
                .get(&FontFamily::Name(Arc::from(super::BROWSER_CHROME_FAMILY)))
                .expect("browser chrome family")[0],
            super::KDAM_THMOR_PRO_KEY,
            "Browser chrome should inherit the platform UI face"
        );
    }

    #[test]
    fn install_parses_and_lays_out_headless() {
        // Registering the font set must work without a GPU (CPU-only Context), and a
        // frame that lays out text in the proportional, monospace, AND the Kdam-first
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
                let browser = FontId::new(
                    13.0,
                    FontFamily::Name(Arc::from(super::BROWSER_CHROME_FAMILY)),
                );
                ui.label(egui::RichText::new("Browser chrome").font(browser));
            });
        });
    }
}
