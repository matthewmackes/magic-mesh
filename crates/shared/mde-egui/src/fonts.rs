//! Bundled Construct fonts and the role-based font installation contract.
//!
//! Every face is embedded in the immutable platform image.  The catalog is
//! deliberately finite: Personalization can choose among these faces, while
//! egui's built-in families remain at the end of every stack for emoji, CJK,
//! and other glyphs a selected face does not contain.

use std::sync::Arc;

use egui::{Context, FontData, FontDefinitions, FontFamily, Id};
use serde::{Deserialize, Serialize};

const KDAM_THMOR_PRO: &[u8] = include_bytes!("../assets/fonts/KdamThmorPro-Regular.ttf");
const INTER: &[u8] = include_bytes!("../assets/fonts/Inter.ttf");
const IBM_PLEX_MONO: &[u8] = include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf");
const ROBOTO: &[u8] = include_bytes!("../assets/fonts/Roboto-Regular.ttf");
const INTEL_ONE_MONO: &[u8] = include_bytes!("../assets/fonts/IntelOneMono-Regular.otf");
const MOZILLA_HEADLINE: &[u8] = include_bytes!("../assets/fonts/MozillaHeadline.ttf");

const KDAM_KEY: &str = "KdamThmorPro";
const INTER_KEY: &str = "Inter";
const IBM_KEY: &str = "IBMPlexMono";
const ROBOTO_KEY: &str = "Roboto";
const INTEL_KEY: &str = "IntelOneMono";
const MOZILLA_KEY: &str = "MozillaHeadline";

/// The six immutable fonts available to Construct surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformFont {
    /// Expressive geometric platform face.
    KdamThmorPro,
    /// Neutral proportional interface face.
    Inter,
    /// IBM's fixed-width workhorse face.
    IbmPlexMono,
    /// Compact proportional fallback face.
    Roboto,
    /// Intel's fixed-width utility face.
    IntelOneMono,
    /// Mozilla's variable display/headline face.
    MozillaHeadline,
}

impl PlatformFont {
    /// Every bundled platform font in the order shown by Personalization.
    pub const ALL: [Self; 6] = [
        Self::KdamThmorPro,
        Self::Inter,
        Self::IbmPlexMono,
        Self::Roboto,
        Self::IntelOneMono,
        Self::MozillaHeadline,
    ];

    /// The human-readable catalog name.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::KdamThmorPro => "Kdam Thmor Pro",
            Self::Inter => "Inter",
            Self::IbmPlexMono => "IBM Plex Mono",
            Self::Roboto => "Roboto",
            Self::IntelOneMono => "Intel One Mono",
            Self::MozillaHeadline => "Mozilla Headline",
        }
    }

    /// A short preview description for the Theme card.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::KdamThmorPro => "Expressive platform display",
            Self::Inter => "Neutral everyday interface",
            Self::IbmPlexMono => "Technical monospace classic",
            Self::Roboto => "Familiar compact sans",
            Self::IntelOneMono => "Crisp fixed-width utility",
            Self::MozillaHeadline => "Editorial variable headline",
        }
    }

    const fn key(self) -> &'static str {
        match self {
            Self::KdamThmorPro => KDAM_KEY,
            Self::Inter => INTER_KEY,
            Self::IbmPlexMono => IBM_KEY,
            Self::Roboto => ROBOTO_KEY,
            Self::IntelOneMono => INTEL_KEY,
            Self::MozillaHeadline => MOZILLA_KEY,
        }
    }

    /// The named family used for this font's preview card.
    #[must_use]
    pub fn preview_family(self) -> FontFamily {
        FontFamily::Name(Arc::from(format!("preview-{}", self.key())))
    }
}

/// The three semantic font roles exposed by Personalization → Theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontSelection {
    /// Proportional copy, navigation, and browser chrome.
    #[serde(default = "default_interface_font")]
    pub interface: PlatformFont,
    /// Display, title, and headline typography.
    #[serde(default = "default_display_font")]
    pub display: PlatformFont,
    /// Fixed-width code, telemetry, and terminal-adjacent content.
    #[serde(default = "default_monospace_font")]
    pub monospace: PlatformFont,
}

const fn default_interface_font() -> PlatformFont {
    PlatformFont::MozillaHeadline
}

const fn default_display_font() -> PlatformFont {
    PlatformFont::MozillaHeadline
}

const fn default_monospace_font() -> PlatformFont {
    PlatformFont::IntelOneMono
}

impl Default for FontSelection {
    fn default() -> Self {
        Self {
            interface: PlatformFont::MozillaHeadline,
            display: PlatformFont::MozillaHeadline,
            monospace: PlatformFont::IntelOneMono,
        }
    }
}

/// Named family for display/headline typography.
pub const HEADING_FAMILY: &str = "heading";
/// Named family for navigation typography.
pub const NAV_FAMILY: &str = "nav";
/// Named family for Browser chrome typography.
pub const BROWSER_CHROME_FAMILY: &str = "browser-chrome";

fn selection_id() -> Id {
    Id::new("mde-egui-font-selection")
}

/// Install the default role selection. This remains the compatibility API used
/// by unrelated callers and by standalone surfaces.
pub fn install(ctx: &Context) {
    install_with_selection(ctx, FontSelection::default());
}

/// Return the selection last installed on `ctx`, falling back to platform defaults
/// for a fresh context.
#[must_use]
pub fn installed_selection(ctx: &Context) -> FontSelection {
    ctx.data_mut(|data| data.get_persisted(selection_id()).unwrap_or_default())
}

/// Return the named family used to preview one catalog face.
#[must_use]
pub fn preview_family(font: PlatformFont) -> FontFamily {
    font.preview_family()
}

/// Install all bundled faces and map the selected roles to complete fallback stacks.
pub fn install_with_selection(ctx: &Context, selection: FontSelection) {
    ctx.set_fonts(definitions(selection));
    ctx.data_mut(|data| data.insert_persisted(selection_id(), selection));
}

fn definitions(selection: FontSelection) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let builtin_proportional = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mut builtin_fallbacks = builtin_proportional;
    if let Some(builtin_mono) = fonts.families.get(&FontFamily::Monospace) {
        append_unique(&mut builtin_fallbacks, builtin_mono);
    }
    for (key, bytes) in [
        (KDAM_KEY, KDAM_THMOR_PRO),
        (INTER_KEY, INTER),
        (IBM_KEY, IBM_PLEX_MONO),
        (ROBOTO_KEY, ROBOTO),
        (INTEL_KEY, INTEL_ONE_MONO),
        (MOZILLA_KEY, MOZILLA_HEADLINE),
    ] {
        fonts
            .font_data
            .insert(key.to_owned(), Arc::new(FontData::from_static(bytes)));
    }

    let interface = stack_with_fallbacks(
        selection.interface,
        [
            PlatformFont::MozillaHeadline,
            PlatformFont::Inter,
            PlatformFont::Roboto,
            PlatformFont::KdamThmorPro,
            PlatformFont::IbmPlexMono,
            PlatformFont::IntelOneMono,
        ],
        &builtin_fallbacks,
    );
    let display = stack_with_fallbacks(
        selection.display,
        [
            PlatformFont::MozillaHeadline,
            PlatformFont::KdamThmorPro,
            PlatformFont::Inter,
            PlatformFont::Roboto,
            PlatformFont::IbmPlexMono,
            PlatformFont::IntelOneMono,
        ],
        &builtin_fallbacks,
    );
    let monospace = stack_with_fallbacks(
        selection.monospace,
        [
            PlatformFont::IntelOneMono,
            PlatformFont::IbmPlexMono,
            PlatformFont::Inter,
            PlatformFont::Roboto,
            PlatformFont::KdamThmorPro,
            PlatformFont::MozillaHeadline,
        ],
        &builtin_fallbacks,
    );

    fonts
        .families
        .insert(FontFamily::Proportional, interface.clone());
    fonts.families.insert(FontFamily::Monospace, monospace);
    fonts
        .families
        .insert(FontFamily::Name(Arc::from(HEADING_FAMILY)), display);
    fonts
        .families
        .insert(FontFamily::Name(Arc::from(NAV_FAMILY)), interface.clone());
    fonts.families.insert(
        FontFamily::Name(Arc::from(BROWSER_CHROME_FAMILY)),
        interface,
    );

    // Every catalog card can lay out its own sample without changing the active
    // role. Its selected face is first, followed by the same complete fallback set.
    for font in PlatformFont::ALL {
        fonts.families.insert(
            font.preview_family(),
            stack_with_fallbacks(font, PlatformFont::ALL, &builtin_fallbacks),
        );
    }
    fonts
}

fn stack<const N: usize>(first: PlatformFont, fallback: [PlatformFont; N]) -> Vec<String> {
    let mut result = Vec::with_capacity(N);
    result.push(first.key().to_owned());
    for font in fallback {
        if font != first {
            result.push(font.key().to_owned());
        }
    }
    result
}

fn stack_with_fallbacks<const N: usize>(
    first: PlatformFont,
    fallback: [PlatformFont; N],
    builtins: &[String],
) -> Vec<String> {
    let mut result = stack(first, fallback);
    append_unique(&mut result, builtins);
    result
}

fn append_unique(result: &mut Vec<String>, candidates: &[String]) {
    for candidate in candidates {
        if !result.contains(candidate) {
            result.push(candidate.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{FontFamily, FontId, RichText};

    #[test]
    fn all_six_platform_fonts_are_embedded_and_valid() {
        let assets = [
            (KDAM_THMOR_PRO, &[0x00, 0x01, 0x00, 0x00][..]),
            (INTER, &[0x00, 0x01, 0x00, 0x00][..]),
            (IBM_PLEX_MONO, &[0x00, 0x01, 0x00, 0x00][..]),
            (ROBOTO, &[0x00, 0x01, 0x00, 0x00][..]),
            (INTEL_ONE_MONO, b"OTTO"),
            (MOZILLA_HEADLINE, &[0x00, 0x01, 0x00, 0x00][..]),
        ];
        for (bytes, tag) in assets {
            assert!(bytes.len() > 50_000, "font asset is unexpectedly small");
            assert_eq!(&bytes[..4], tag);
        }
        assert_eq!(PlatformFont::ALL.len(), 6);
    }

    #[test]
    fn defaults_are_mozilla_for_ui_and_intel_for_monospace() {
        let selection = FontSelection::default();
        assert_eq!(selection.interface, PlatformFont::MozillaHeadline);
        assert_eq!(selection.display, PlatformFont::MozillaHeadline);
        assert_eq!(selection.monospace, PlatformFont::IntelOneMono);
        let fonts = definitions(selection);
        assert_eq!(fonts.families[&FontFamily::Proportional][0], MOZILLA_KEY);
        assert_eq!(
            fonts.families[&FontFamily::Name(Arc::from(HEADING_FAMILY))][0],
            MOZILLA_KEY
        );
        assert_eq!(fonts.families[&FontFamily::Monospace][0], INTEL_KEY);
    }

    #[test]
    fn each_role_maps_to_the_selected_face_and_keeps_fallbacks() {
        let selection = FontSelection {
            interface: PlatformFont::Roboto,
            display: PlatformFont::KdamThmorPro,
            monospace: PlatformFont::IbmPlexMono,
        };
        let fonts = definitions(selection);
        assert_eq!(fonts.families[&FontFamily::Proportional][0], ROBOTO_KEY);
        assert_eq!(
            fonts.families[&FontFamily::Name(Arc::from(NAV_FAMILY))][0],
            ROBOTO_KEY
        );
        assert_eq!(
            fonts.families[&FontFamily::Name(Arc::from(BROWSER_CHROME_FAMILY))][0],
            ROBOTO_KEY
        );
        assert_eq!(
            fonts.families[&FontFamily::Name(Arc::from(HEADING_FAMILY))][0],
            KDAM_KEY
        );
        assert_eq!(fonts.families[&FontFamily::Monospace][0], IBM_KEY);
        for font in PlatformFont::ALL {
            let family = fonts.families.get(&font.preview_family()).unwrap();
            assert_eq!(family[0], font.key());
            assert!(family.len() > 6, "egui built-in fallback is missing");
        }
    }

    #[test]
    fn selected_fonts_layout_headlessly_for_every_role_and_preview() {
        let ctx = Context::default();
        install_with_selection(
            &ctx,
            FontSelection {
                interface: PlatformFont::Inter,
                display: PlatformFont::MozillaHeadline,
                monospace: PlatformFont::IntelOneMono,
            },
        );
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.label(
                    RichText::new("interface navigation browser")
                        .font(FontId::new(16.0, FontFamily::Proportional)),
                );
                ui.label(RichText::new("display headline").font(FontId::new(
                    24.0,
                    FontFamily::Name(Arc::from(HEADING_FAMILY)),
                )));
                ui.monospace("fixed-width telemetry");
                for font in PlatformFont::ALL {
                    ui.label(
                        RichText::new(font.label()).font(FontId::new(16.0, preview_family(font))),
                    );
                }
            });
        });
    }
}
