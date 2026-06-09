//! Theme enum + the resolved `Tokens` bundle. Locks: Q5 (ship
//! dark + light together in v2.2), Q6 (wizard asks at first
//! launch — the runtime default until the wizard answers is
//! `Theme::Dark`, but consumers should respect the persisted
//! preference once set).

use crate::density::Density;
use crate::palette::Palette;
use crate::radii::Radii;
use crate::shadows::Shadow;
use crate::spacing::Space;
use crate::typography::{FontSize, FontWeight, LetterSpacing};

/// Theme selection. Dark by default; light ships in v2.2.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Theme {
    /// Dark theme — charcoal background (Q3).
    Dark,
    /// Light theme — soft-white background. Co-ships with dark
    /// in v2.2 per Q5 + FU-2.
    Light,
}

impl Default for Theme {
    fn default() -> Self {
        Self::Dark
    }
}

impl Theme {
    /// Stable identifier for `preferences.toml`.
    pub fn id(self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
        }
    }

    /// Parse from the persisted identifier. Returns `None` on
    /// unknown input.
    pub fn from_id(s: &str) -> Option<Self> {
        match s {
            "dark" => Some(Theme::Dark),
            "light" => Some(Theme::Light),
            _ => None,
        }
    }
}

/// Resolved tokens for a (Theme, Density) pair. Every consumer
/// constructs one of these at startup (and on user-preference
/// change) and reads tokens from it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    /// Source theme.
    pub theme: Theme,
    /// Source density.
    pub density: Density,
    /// Color palette for `theme`.
    pub palette: Palette,
    /// Spacing tokens scaled for `density` (UX-24).
    pub space: Space,
    /// Font sizes (NOT density-scaled per UX-24).
    pub font_size: FontSize,
    /// Letter-spacing per role (Q15).
    pub letter_spacing: LetterSpacing,
    /// Font weights.
    pub weight: FontWeight,
    /// Corner radii (NOT density-scaled — visual identity is
    /// preserved across density modes).
    pub radii: Radii,
    /// Standard modal shadow — `Shadow::modal()`.
    pub modal_shadow: Shadow,
}

impl Tokens {
    /// Resolve tokens for a (theme, density) pair.
    pub fn resolve(theme: Theme, density: Density) -> Self {
        Self {
            theme,
            density,
            palette: Palette::for_theme(theme),
            space: Space::for_density(density),
            font_size: FontSize::defaults(),
            letter_spacing: LetterSpacing::defaults(),
            weight: FontWeight::defaults(),
            radii: Radii::defaults(),
            modal_shadow: Shadow::modal(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_default_is_dark() {
        assert_eq!(Theme::default(), Theme::Dark);
    }

    #[test]
    fn theme_id_round_trips() {
        for t in [Theme::Dark, Theme::Light] {
            assert_eq!(Some(t), Theme::from_id(t.id()));
        }
    }

    #[test]
    fn unknown_theme_id_returns_none() {
        assert!(Theme::from_id("sepia").is_none());
    }

    #[test]
    fn tokens_resolve_to_consistent_theme_palette() {
        let t = Tokens::resolve(Theme::Light, Density::Compact);
        assert_eq!(t.theme, Theme::Light);
        // Light theme background ≠ dark theme background.
        assert_eq!(t.palette.background, Palette::light().background);
    }

    #[test]
    fn font_size_is_not_density_scaled() {
        // UX-24: density scales spacing only.
        let c = Tokens::resolve(Theme::Dark, Density::Compact);
        let m = Tokens::resolve(Theme::Dark, Density::Comfortable);
        let s = Tokens::resolve(Theme::Dark, Density::Spacious);
        assert_eq!(c.font_size, m.font_size);
        assert_eq!(m.font_size, s.font_size);
    }

    #[test]
    fn radii_are_not_density_scaled() {
        let c = Tokens::resolve(Theme::Dark, Density::Compact);
        let s = Tokens::resolve(Theme::Dark, Density::Spacious);
        assert_eq!(c.radii, s.radii);
    }
}
