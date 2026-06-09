//! Accessibility variants. Locks: Q32 (reduced-motion = 80 ms
//! cross-fade fallback), UX-22 (high-contrast theme variant +
//! colorblind-safe accent variant). The variants are exposed as
//! overrides applied on top of a base [`Palette`].

use crate::color::Rgba;
use crate::palette::Palette;

/// User-selectable accessibility variants. Persist via stable
/// `id()` strings; resolve back via `from_id()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct A11y {
    /// Boost contrast to AAA (≥ 12:1 body text) and widen
    /// borders to 2 px instead of hairline.
    pub high_contrast: bool,
    /// Swap the indigo accent for a colorblind-safe alternative
    /// derived from the ColorBrewer "Set2" palette (mid-green
    /// `#4daf4a`), which discriminates well under deuteranopia
    /// and protanopia simulators.
    pub colorblind_safe: bool,
    /// Honor `prefers-reduced-motion`: collapse transitions to
    /// ≤ 80 ms cross-fade per Q32.
    pub reduce_motion: bool,
}

impl Default for A11y {
    fn default() -> Self {
        Self {
            high_contrast: false,
            colorblind_safe: false,
            reduce_motion: false,
        }
    }
}

impl A11y {
    /// Apply the variant to a base palette. Returns a new
    /// palette with the variant's overrides composed.
    pub fn apply(self, mut p: Palette) -> Palette {
        if self.colorblind_safe {
            // ColorBrewer Set2-derived green that's discernible
            // under deuteranopia, protanopia, and tritanopia.
            // (Indigo → green; preserves the "single accent"
            // contract of the design system.)
            p.accent = Rgba::rgb(0x4d, 0xaf, 0x4a);
        }
        if self.high_contrast {
            // Push text to fully opaque white/black for ≥ 12:1
            // body-on-background contrast, and brighten the
            // border so 1 px lines become unambiguous against the
            // ChromeOS surface tier (CR-1 made the default border
            // a hard solid color rather than an alpha hairline).
            let dark = p.background.r < 0x80;
            if dark {
                p.text = Rgba::rgba(0xff, 0xff, 0xff, 1.00);
                p.text_muted = Rgba::rgba(0xff, 0xff, 0xff, 0.85);
                // Bright white border at full alpha — maximum
                // contrast against the dark page surface.
                p.border = Rgba::rgba(0xff, 0xff, 0xff, 1.00);
            } else {
                p.text = Rgba::rgba(0x00, 0x00, 0x00, 1.00);
                p.text_muted = Rgba::rgba(0x00, 0x00, 0x00, 0.75);
                // Solid black border at full alpha.
                p.border = Rgba::rgba(0x00, 0x00, 0x00, 1.00);
            }
        }
        p
    }

    /// Stable identifier-bag for `~/.config/mde/preferences.toml`.
    /// Returns three id strings or "off" flags. Persisted as
    /// individual keys, not as one composite value.
    pub fn ids(self) -> (&'static str, &'static str, &'static str) {
        (
            if self.high_contrast { "on" } else { "off" },
            if self.colorblind_safe { "on" } else { "off" },
            if self.reduce_motion { "on" } else { "off" },
        )
    }

    /// Reduced-motion's transition duration cap (ms). 80 ms when
    /// the user has signalled reduced motion; the standard
    /// duration otherwise (180 ms per Q30).
    pub fn transition_duration_ms(self, standard_ms: u16) -> u16 {
        if self.reduce_motion {
            80
        } else {
            standard_ms
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[test]
    fn default_is_all_off() {
        let a = A11y::default();
        assert!(!a.high_contrast);
        assert!(!a.colorblind_safe);
        assert!(!a.reduce_motion);
    }

    #[test]
    fn colorblind_safe_swaps_accent() {
        let base = Palette::for_theme(Theme::Dark);
        let after = A11y {
            colorblind_safe: true,
            ..A11y::default()
        }
        .apply(base);
        assert_ne!(after.accent, base.accent);
        // Green hue check: g should dominate r and b.
        assert!(after.accent.g > after.accent.r);
        assert!(after.accent.g > after.accent.b);
    }

    #[test]
    fn colorblind_safe_does_not_change_other_tokens() {
        let base = Palette::for_theme(Theme::Dark);
        let after = A11y {
            colorblind_safe: true,
            ..A11y::default()
        }
        .apply(base);
        assert_eq!(after.background, base.background);
        assert_eq!(after.text, base.text);
    }

    #[test]
    fn high_contrast_boosts_text_to_fully_opaque() {
        let base = Palette::for_theme(Theme::Dark);
        let after = A11y {
            high_contrast: true,
            ..A11y::default()
        }
        .apply(base);
        assert!((after.text.a - 1.00).abs() < 0.001);
    }

    #[test]
    fn high_contrast_dark_keeps_dark_background() {
        let base = Palette::for_theme(Theme::Dark);
        let after = A11y {
            high_contrast: true,
            ..A11y::default()
        }
        .apply(base);
        assert_eq!(after.background, base.background);
    }

    #[test]
    fn high_contrast_brightens_border() {
        // CR-1 (2026-05-25): Classic ChromeOS uses solid borders
        // (alpha 1.0) by default — so "widen alpha" no longer
        // describes the high-contrast nudge. The new contract is
        // "brighten the border to maximum contrast against the
        // page surface" (white-on-dark / black-on-light).
        let dark_base = Palette::for_theme(Theme::Dark);
        let dark_after = A11y {
            high_contrast: true,
            ..A11y::default()
        }
        .apply(dark_base);
        // White border at full alpha in dark mode.
        assert_eq!(
            (
                dark_after.border.r,
                dark_after.border.g,
                dark_after.border.b
            ),
            (0xff, 0xff, 0xff),
        );
        assert!((dark_after.border.a - 1.0).abs() < 0.001);
        // The high-contrast border reads brighter than the base
        // (Classic ChromeOS divider #3c4043).
        let base_luma = (dark_base.border.r as u32
            + dark_base.border.g as u32
            + dark_base.border.b as u32) as f32
            / 3.0;
        let after_luma = (dark_after.border.r as u32
            + dark_after.border.g as u32
            + dark_after.border.b as u32) as f32
            / 3.0;
        assert!(after_luma > base_luma, "high_contrast brightens the border");
    }

    #[test]
    fn reduced_motion_caps_transitions_to_80ms() {
        let a = A11y {
            reduce_motion: true,
            ..A11y::default()
        };
        assert_eq!(a.transition_duration_ms(180), 80);
        assert_eq!(a.transition_duration_ms(280), 80);
    }

    #[test]
    fn reduced_motion_off_preserves_standard_duration() {
        let a = A11y::default();
        assert_eq!(a.transition_duration_ms(180), 180);
        assert_eq!(a.transition_duration_ms(280), 280);
    }

    #[test]
    fn variants_compose() {
        let base = Palette::for_theme(Theme::Dark);
        let both = A11y {
            high_contrast: true,
            colorblind_safe: true,
            reduce_motion: false,
        }
        .apply(base);
        // Both effects visible.
        assert_ne!(both.accent, base.accent); // colorblind-safe
        assert!((both.text.a - 1.00).abs() < 0.001); // high-contrast
    }
}
