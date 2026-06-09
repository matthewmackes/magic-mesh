//! Typography tokens. CR-1 (2026-05-25) swapped the v2.2 lock
//! (Geologica + IBM Plex Mono) for Classic ChromeOS Roboto +
//! Roboto Mono per `docs/design/chromeos-classic-spec.md`. The
//! pre-CR-1 Q11/Q12 lock is grandfathered in source comments
//! for archaeology; the live tokens are Roboto/Roboto Mono.

/// Display + body font family. Roboto — Fedora's
/// `google-roboto-fonts` package supplies it. Q11 grandfathered
/// (was Geologica).
pub const FONT_DISPLAY_BODY: &str = "Roboto";

/// Monospace font family. Roboto Mono — Fedora's
/// `google-roboto-mono-fonts` package supplies it. Q12
/// grandfathered (was IBM Plex Mono).
pub const FONT_MONO: &str = "Roboto Mono";

/// Type scale ratio. Classic ChromeOS uses a flatter scale than
/// Q14's 1.2 minor third — body 13, display 22, ~1.69×. The
/// constant stays here for legacy callers that read it; the
/// FontSize tier defaults are the live source of truth.
pub const SCALE_RATIO: f32 = 1.2;

/// Roles a piece of text can take. Maps to the eight tiers in
/// [`FontSize`]. Use [`TypeRole::size_in`] to look up the
/// resolved pixel size from a [`FontSize`] token bundle, and
/// [`TypeRole::letter_spacing_in`] for the matching tracking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypeRole {
    /// 12 sp caption / label / chip.
    Caption,
    /// 14 sp body copy.
    Body,
    /// 17 sp subheading.
    Subheading,
    /// 20 sp heading.
    Heading,
    /// 24 sp section title.
    Section,
    /// 28 sp page / display title.
    Display,
    /// 13 sp monospace inline.
    Mono,
}

impl TypeRole {
    /// Pixel size for this role from a [`FontSize`] bundle.
    pub fn size_in(self, sizes: FontSize) -> f32 {
        match self {
            TypeRole::Caption => sizes.caption,
            TypeRole::Body => sizes.body,
            TypeRole::Subheading => sizes.subheading,
            TypeRole::Heading => sizes.heading,
            TypeRole::Section => sizes.section,
            TypeRole::Display => sizes.display,
            TypeRole::Mono => sizes.mono,
        }
    }

    /// Letter-spacing (em) for this role from a [`LetterSpacing`]
    /// bundle.
    pub fn letter_spacing_in(self, ls: LetterSpacing) -> f32 {
        match self {
            TypeRole::Display => ls.display,
            TypeRole::Section => ls.section,
            TypeRole::Heading => ls.heading,
            TypeRole::Caption | TypeRole::Body | TypeRole::Subheading => ls.body,
            TypeRole::Mono => ls.mono,
        }
    }

    /// Weight for this role from a [`FontWeight`] bundle. Display
    /// / headings / sections / captions are medium; body and mono
    /// are regular.
    pub fn weight_in(self, w: FontWeight) -> u16 {
        match self {
            TypeRole::Display
            | TypeRole::Section
            | TypeRole::Heading
            | TypeRole::Subheading
            | TypeRole::Caption => w.medium,
            TypeRole::Body | TypeRole::Mono => w.regular,
        }
    }

    /// Font family for this role. Mono returns [`FONT_MONO`];
    /// every other role returns [`FONT_DISPLAY_BODY`] (Geologica
    /// single-family per Q11/Q12).
    pub fn family(self) -> &'static str {
        match self {
            TypeRole::Mono => FONT_MONO,
            _ => FONT_DISPLAY_BODY,
        }
    }
}

/// Sizes in scale points (sp), one tier per type role.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontSize {
    /// Caption / label — 12 sp.
    pub caption: f32,
    /// Body copy — 14 sp.
    pub body: f32,
    /// Subheading — 17 sp.
    pub subheading: f32,
    /// Heading — 20 sp.
    pub heading: f32,
    /// Section title — 24 sp.
    pub section: f32,
    /// Page / display title — 28 sp.
    pub display: f32,
    /// Monospace inline / code-fragment size — 13 sp.
    pub mono: f32,
}

impl FontSize {
    /// Token defaults — Classic ChromeOS tiers per CR-1
    /// (2026-05-25). Source: `docs/design/chromeos-classic-spec
    /// .md` § Typography (UI body 13, Section header 11, Page
    /// title 18, Display title 22, Monospace 12). The
    /// `subheading` + `heading` slots interpolate between
    /// `body` and `section` since the spec doesn't carry
    /// distinct tiers for them — picked 15 and 16 to keep the
    /// progression monotonic.
    pub const fn defaults() -> Self {
        Self {
            // Section header (small caps, +0.5 px letter-space).
            caption: 11.0,
            // UI body.
            body: 13.0,
            subheading: 15.0,
            heading: 16.0,
            // Page title.
            section: 18.0,
            // Display title.
            display: 22.0,
            // Monospace.
            mono: 12.0,
        }
    }
}

/// Letter-spacing adjustments per role. Q15: tight on display,
/// default on body. Values are in fractional em — apply via the
/// Iced widget's `letter-spacing` analogue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LetterSpacing {
    /// Tighten display titles ~1.5%.
    pub display: f32,
    /// Tighten section titles ~1%.
    pub section: f32,
    /// Tighten headings ~1%.
    pub heading: f32,
    /// Body / subheading / caption stay neutral.
    pub body: f32,
    /// Monospace stays neutral.
    pub mono: f32,
}

impl LetterSpacing {
    /// Defaults per Q15.
    pub const fn defaults() -> Self {
        Self {
            display: -0.015,
            section: -0.010,
            heading: -0.010,
            body: 0.000,
            mono: 0.000,
        }
    }
}

/// Font weights — Geologica's variable axis exposes 100..900;
/// the design system uses two: 400 (regular) and 500 (medium).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontWeight {
    /// 400 — body, caption.
    pub regular: u16,
    /// 500 — display, headings, section titles, button labels.
    pub medium: u16,
}

impl FontWeight {
    /// Defaults: 400 / 500.
    pub const fn defaults() -> Self {
        Self {
            regular: 400,
            medium: 500,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_is_1_2_minor_third() {
        assert!((SCALE_RATIO - 1.2).abs() < 0.001);
    }

    #[test]
    fn body_size_is_chromeos_13px() {
        // CR-1 (2026-05-25): UI body Roboto 400 / 13 px / 18 px
        // line per docs/design/chromeos-classic-spec.md.
        assert_eq!(FontSize::defaults().body as i32, 13);
    }

    #[test]
    fn display_size_is_chromeos_22px() {
        // CR-1: Display title Roboto 400 / 22 px / 28 px line.
        assert_eq!(FontSize::defaults().display as i32, 22);
    }

    #[test]
    fn display_tracks_tighter_than_body() {
        let ls = LetterSpacing::defaults();
        assert!(ls.display < ls.body);
    }

    #[test]
    fn medium_weight_is_500() {
        assert_eq!(FontWeight::defaults().medium, 500);
    }

    #[test]
    fn font_family_is_roboto() {
        // CR-1 (2026-05-25): Classic ChromeOS Roboto replaces
        // Geologica per docs/design/chromeos-classic-spec.md.
        assert_eq!(FONT_DISPLAY_BODY, "Roboto");
    }

    #[test]
    fn mono_is_roboto_mono() {
        // CR-1: Roboto Mono replaces IBM Plex Mono.
        assert_eq!(FONT_MONO, "Roboto Mono");
    }

    #[test]
    fn type_role_size_resolves() {
        let sizes = FontSize::defaults();
        assert_eq!(TypeRole::Body.size_in(sizes) as i32, 13);
        assert_eq!(TypeRole::Display.size_in(sizes) as i32, 22);
        assert_eq!(TypeRole::Mono.size_in(sizes) as i32, 12);
    }

    #[test]
    fn type_role_weight_resolves() {
        let w = FontWeight::defaults();
        assert_eq!(TypeRole::Body.weight_in(w), 400);
        assert_eq!(TypeRole::Heading.weight_in(w), 500);
        assert_eq!(TypeRole::Display.weight_in(w), 500);
        assert_eq!(TypeRole::Mono.weight_in(w), 400);
    }

    #[test]
    fn type_role_letter_spacing_resolves() {
        let ls = LetterSpacing::defaults();
        // Display + section + heading are tightened.
        assert!(TypeRole::Display.letter_spacing_in(ls) < 0.0);
        assert!(TypeRole::Section.letter_spacing_in(ls) < 0.0);
        // Body / subheading / caption are neutral.
        assert_eq!(TypeRole::Body.letter_spacing_in(ls), 0.0);
        assert_eq!(TypeRole::Mono.letter_spacing_in(ls), 0.0);
    }

    #[test]
    fn type_role_family_routes_mono_separately() {
        assert_eq!(TypeRole::Mono.family(), FONT_MONO);
        assert_eq!(TypeRole::Body.family(), FONT_DISPLAY_BODY);
        assert_eq!(TypeRole::Display.family(), FONT_DISPLAY_BODY);
    }
}
