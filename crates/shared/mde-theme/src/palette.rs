//! Color palette tokens. Locks: Q2 (accent), Q3 (charcoal),
//! Q4 (4 elevation tiers), Q5 (light theme ships in v2.2),
//! Q7 (adaptive borders). See `docs/design/visual-identity.md`
//! § 2 for the rationale and the full table.

use crate::color::Rgba;
use crate::theme::Theme;

/// A complete palette for one theme. All eight tokens are
/// guaranteed populated. Color picks come from the lock survey;
/// adjust at survey time, not at call sites.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// Lowest surface in the elevation stack. Carbon Gray 100 (dark) /
    /// Gray 10 (light).
    pub background: Rgba,
    /// Standard surface — cards, panels, sidebars (Carbon layer-01).
    pub surface: Rgba,
    /// Raised surface — modals, popovers, command palette (layer-02).
    pub raised: Rgba,
    /// Overlay surface — tooltips, dropdown menus.
    pub overlay: Rgba,
    /// Single interactive accent — Carbon Blue 60. Same in both themes
    /// by design (one restrained accent).
    pub accent: Rgba,
    /// Hairline border in dark mode; 1 px solid border in light
    /// mode (Q7 adaptive).
    pub border: Rgba,
    /// Primary text color. Dark: near-white. Light: near-black.
    pub text: Rgba,
    /// Muted / secondary text color.
    pub text_muted: Rgba,
    /// Semantic success (green) — confirmations, healthy status.
    /// These three semantic roles are the only named colors aside from
    /// the single accent (E5.3: centralized here so every surface reads
    /// them from one place instead of hardcoding the raw color).
    pub success: Rgba,
    /// Semantic danger (red) — errors, destructive actions.
    pub danger: Rgba,
    /// Semantic warning (amber) — cautions, pending / at-risk status.
    pub warning: Rgba,
}

impl Palette {
    /// Resolve the palette for a given theme.
    pub const fn for_theme(theme: Theme) -> Self {
        match theme {
            Theme::Dark => Self::dark(),
            Theme::Light => Self::light(),
        }
    }

    /// Dark-theme palette — **IBM Carbon Gray 100** (E9, Carbon-only,
    /// 2026-06-07; supersedes the Classic-ChromeOS CR-1 set). Tokens are
    /// the published Carbon values (carbondesignsystem.com/elements/
    /// color/tokens) — the same Gray ramp + Blue 60 the shell's `mde-ui`
    /// single-sources; per §2.2 change one only with a spec reference +
    /// update the pinning tests in the same commit.
    pub const fn dark() -> Self {
        Self {
            // Carbon background — Gray 100.
            background: Rgba::rgb(0x16, 0x16, 0x16),
            // Layer-01 (cards, panels, sidebars) — Gray 90.
            surface: Rgba::rgb(0x26, 0x26, 0x26),
            // Layer-02 (modals, popovers, raised surfaces) — Gray 80.
            raised: Rgba::rgb(0x39, 0x39, 0x39),
            // Overlay tier (tooltips, dropdowns, heavier divider) — Gray 70.
            overlay: Rgba::rgb(0x52, 0x52, 0x52),
            // Interactive accent — Carbon Blue 60.
            accent: Rgba::rgb(0x0f, 0x62, 0xfe),
            // Border-subtle on Gray 100 — Gray 80.
            border: Rgba::rgb(0x39, 0x39, 0x39),
            // Text primary on dark — Gray 10.
            text: Rgba::rgb(0xf4, 0xf4, 0xf4),
            // Text secondary / helper — Gray 50.
            text_muted: Rgba::rgb(0x8d, 0x8d, 0x8d),
            // Support roles — Carbon Green 50 / Red 60 / Yellow 30.
            success: Rgba::rgb(0x24, 0xa1, 0x48),
            danger: Rgba::rgb(0xda, 0x1e, 0x28),
            warning: Rgba::rgb(0xf1, 0xc2, 0x1b),
        }
    }

    /// Light-theme palette — **IBM Carbon Gray 10** (E9, Carbon-only).
    /// The Gray-10 counterpart to [`Self::dark`]; the same Carbon Blue 60
    /// interactive accent + support ramp, inverted gray surfaces.
    pub const fn light() -> Self {
        Self {
            // Carbon background — Gray 10.
            background: Rgba::rgb(0xf4, 0xf4, 0xf4),
            // Layer-01 — White.
            surface: Rgba::rgb(0xff, 0xff, 0xff),
            // Layer-02 — White (Carbon layers white-on-Gray-10).
            raised: Rgba::rgb(0xff, 0xff, 0xff),
            // Overlay / layer-hover — Gray 10 hover.
            overlay: Rgba::rgb(0xe8, 0xe8, 0xe8),
            // Interactive accent — Carbon Blue 60 (same as dark).
            accent: Rgba::rgb(0x0f, 0x62, 0xfe),
            // Border-subtle-01 (light) — Gray 20.
            border: Rgba::rgb(0xe0, 0xe0, 0xe0),
            // Text primary on light — Gray 100.
            text: Rgba::rgb(0x16, 0x16, 0x16),
            // Text secondary — Gray 70.
            text_muted: Rgba::rgb(0x52, 0x52, 0x52),
            // Support roles — Carbon Green 50 / Red 60 / Yellow 30.
            success: Rgba::rgb(0x24, 0xa1, 0x48),
            danger: Rgba::rgb(0xda, 0x1e, 0x28),
            warning: Rgba::rgb(0xf1, 0xc2, 0x1b),
        }
    }

    /// Translucent indigo wash used for hover states (Q8).
    /// Returns the accent at 8% opacity.
    pub fn hover_tint(&self) -> Rgba {
        self.accent.with_alpha(0.08)
    }

    /// Active (mouse-down) state — accent at 12% opacity.
    pub fn active_tint(&self) -> Rgba {
        self.accent.with_alpha(0.12)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_is_carbon_blue_60() {
        // E9 (2026-06-07): Carbon-only — the interactive accent is
        // Carbon Blue 60 (carbondesignsystem.com), replacing the retired
        // Q2 indigo. Same value in both themes.
        let p = Palette::dark();
        assert_eq!((p.accent.r, p.accent.g, p.accent.b), (0x0f, 0x62, 0xfe));
    }

    #[test]
    fn accent_is_uniform_carbon_blue_across_themes() {
        // Carbon uses one interactive blue regardless of gray theme — the
        // ChromeOS per-theme accent shift is retired.
        assert_eq!(Palette::dark().accent, Palette::light().accent);
        let d = Palette::dark().accent;
        assert!(d.b > d.r && d.b > d.g, "accent reads as blue");
    }

    #[test]
    fn dark_background_matches_carbon_gray_100() {
        // E9: Carbon Gray 100 is the dark page surface
        // (carbondesignsystem.com gray ramp).
        let bg = Palette::dark().background;
        assert_eq!((bg.r, bg.g, bg.b), (0x16, 0x16, 0x16));
    }

    #[test]
    fn surfaces_follow_carbon_gray_ramp() {
        // E9: dark layers walk the Carbon ramp — Gray 90 (layer-01) for
        // surface, Gray 80 (layer-02) for raised, both above Gray 100 bg.
        let d = Palette::dark();
        assert_eq!((d.surface.r, d.surface.g, d.surface.b), (0x26, 0x26, 0x26));
        assert_eq!((d.raised.r, d.raised.g, d.raised.b), (0x39, 0x39, 0x39));
    }

    #[test]
    fn border_is_solid_carbon_subtle() {
        // E9: Carbon border-subtle — Gray 80 on dark, Gray 20 on light;
        // solid (no alpha hairline).
        let d = Palette::dark();
        assert_eq!((d.border.r, d.border.g, d.border.b), (0x39, 0x39, 0x39));
        let l = Palette::light();
        assert_eq!((l.border.r, l.border.g, l.border.b), (0xe0, 0xe0, 0xe0));
        assert!(d.border.a >= 0.95);
        assert!(l.border.a >= 0.95);
    }

    #[test]
    fn hover_tint_uses_accent_at_8pct() {
        let p = Palette::dark();
        let h = p.hover_tint();
        assert_eq!(h.r, p.accent.r);
        assert!((h.a - 0.08).abs() < 0.001);
    }
}
