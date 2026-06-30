//! Color tokens for `mde-voice-hud`.
//!
//! The HUD is a Portal-full overlay surface. E9.9 (§1) — its palette is the
//! IBM Carbon Gray-100 dark tokens + Blue accent, like the rest of the
//! platform (the original Material-3 / Mackes-orange sub-palette predated the
//! Carbon-only collapse and was a §1 violation). The Material-3 *role* names
//! (`SURF`, `ON_SURF`, `PRIMARY`…) are kept for call-site stability.
//!
//! §4 — the hex is **not** carried here: every value is a named step from the
//! single-sourced [`mde_theme::carbon`] ramp, converted to this crate's
//! `cosmic::iced::Color` locally (the HUD's iced version skews from the one
//! `Rgba::into_iced_color()` targets). This module maps Carbon ramp steps to
//! HUD roles; it holds no raw color literals. Only roles with live call
//! sites are kept (sweep-3 I3 removed the dead H2-residue set).

use cosmic::iced::Color;
use mde_theme::carbon;
use mde_theme::Rgba;

/// Convert an `mde_theme` ramp token to this crate's `cosmic::iced::Color`, carrying the
/// token's own alpha. The hex lives once, in [`mde_theme::carbon`].
const fn tok(c: Rgba) -> Color {
    Color {
        r: c.r as f32 / 255.0,
        g: c.g as f32 / 255.0,
        b: c.b as f32 / 255.0,
        a: c.a,
    }
}

// ---------- Surface palette (IBM Carbon Gray-100 dark ramp) ----------

/// Base surface — Carbon Gray 100.
pub const SURF: Color = tok(carbon::GRAY_100);
/// Mid-elevation container (keypad keys / hop pills) — Gray 80.
pub const SURF_C: Color = tok(carbon::GRAY_80);
/// High-elevation hover state — Gray 70.
pub const SURF_C_HI: Color = tok(carbon::GRAY_70);
/// Outline variant (lighter divider) — Gray 80.
pub const OUTLINE_VAR: Color = tok(carbon::GRAY_80);

// ---------- Foreground palette (Carbon text ramp) ----------

/// Primary text — Gray 10.
pub const ON_SURF: Color = tok(carbon::GRAY_10);
/// Secondary text — Gray 30.
pub const ON_SURF_VAR: Color = tok(carbon::GRAY_30);
/// Muted / helper text — Gray 50.
pub const ON_SURF_MUTED: Color = tok(carbon::GRAY_50);

// ---------- Status colors (Carbon support, dark) ----------

/// Success — on-dark Green 40.
pub const SUCCESS: Color = tok(carbon::GREEN_40);
/// Error — on-dark Red 50.
pub const ERROR: Color = tok(carbon::RED_50);
/// Info — Blue 40.
pub const INFO: Color = tok(carbon::BLUE_40);
/// Warning — on-dark Yellow 30 (degraded / caveated states, e.g. a fixture
/// roster or a `StateTone::Warning` load-state).
pub const WARNING: Color = tok(carbon::YELLOW_30);

// ---------- Primary (Carbon Blue accent) ----------

/// On-dark interactive accent — Blue 50.
pub const PRIMARY: Color = tok(carbon::BLUE_50);
/// Text on the Blue fill — White.
pub const ON_PRIMARY: Color = tok(carbon::WHITE);

// ---------- Presence color ----------

/// An available / online mesh peer (drives the resolved-chip "mesh · <name>").
pub const PRESENCE_AVAILABLE: Color = SUCCESS;

// ---------- Contrast-aware label colour (WCAG) ----------

/// WCAG relative luminance of `c` (0.0 = black … 1.0 = white). The sRGB channels
/// are linearised per the WCAG 2.x definition before the luma weights apply.
/// Pure; reads the already-token-derived `Color`, so it mints nothing.
fn relative_luminance(c: Color) -> f32 {
    fn linearize(ch: f32) -> f32 {
        if ch <= 0.03928 {
            ch / 12.92
        } else {
            ((ch + 0.055) / 1.055).powf(2.4)
        }
    }
    let (lr, lg, lb) = (linearize(c.r), linearize(c.g), linearize(c.b));
    lr.mul_add(0.2126, lg.mul_add(0.7152, lb * 0.0722))
}

/// WCAG contrast ratio between two colours (1.0 = identical … 21.0 = black/white).
fn contrast_ratio(a: Color, b: Color) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Pick the chip/label foreground that reads best over `fill`: the dark text
/// token ([`SURF`], Gray 100) on light fills, the light text token ([`ON_SURF`],
/// Gray 10) on dark fills — whichever wins the WCAG contrast. This is the §4
/// contrast fix for the status chips, which used to paint raw white over the
/// light Green-40 / Blue-40 fills (≈ 1.9 : 1, an AA fail); the pick mirrors the
/// call pill, which already lays dark text over its saturated status fills. Both
/// candidates are single-sourced `mde-theme` tokens — nothing is minted.
#[must_use]
pub fn label_on(fill: Color) -> Color {
    if contrast_ratio(ON_SURF, fill) >= contrast_ratio(SURF, fill) {
        ON_SURF
    } else {
        SURF
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb8(c: Color) -> (u8, u8, u8) {
        (
            (c.r * 255.0).round() as u8,
            (c.g * 255.0).round() as u8,
            (c.b * 255.0).round() as u8,
        )
    }

    /// E9.9 / §1 — pin the HUD palette to IBM Carbon dark tokens, so a drift
    /// back to the retired Material-3 / Mackes-orange values fails here.
    #[test]
    fn hud_palette_is_carbon() {
        assert_eq!(rgb8(SURF), (0x16, 0x16, 0x16), "base = Gray 100");
        assert_eq!(rgb8(SURF_C), (0x39, 0x39, 0x39), "mid = Gray 80");
        assert_eq!(rgb8(ON_SURF), (0xf4, 0xf4, 0xf4), "text = Gray 10");
        assert_eq!(rgb8(PRIMARY), (0x45, 0x89, 0xff), "accent = Blue 50");
        assert_eq!(rgb8(SUCCESS), (0x42, 0xbe, 0x65), "success = Green 40");
        assert_eq!(rgb8(ERROR), (0xfa, 0x4d, 0x56), "error = Red 50");
        assert_eq!(rgb8(WARNING), (0xf1, 0xc2, 0x1b), "warning = Yellow 30");
    }

    /// Axis-3 contrast — the light Green/Blue/Red status fills must take the DARK
    /// label (Gray 100), the same dark-on-fill treatment the call pill uses; this
    /// is the fix for the old raw white-on-Green-40 (≈ 1.9 : 1) AA fail.
    #[test]
    fn label_on_picks_dark_text_over_light_fills() {
        assert_eq!(label_on(SUCCESS), SURF, "green-40 fill → dark label");
        assert_eq!(label_on(PRESENCE_AVAILABLE), SURF, "presence fill → dark");
        assert_eq!(label_on(INFO), SURF, "blue-40 fill → dark label");
        assert_eq!(label_on(PRIMARY), SURF, "blue-50 fill → dark label");
        assert_eq!(label_on(ERROR), SURF, "red-50 fill → dark label");
    }

    /// The neutral dark fills (Gray 70 empty/partial chip, Gray 100) keep the
    /// light label.
    #[test]
    fn label_on_picks_light_text_over_dark_fills() {
        assert_eq!(label_on(SURF_C_HI), ON_SURF, "gray-70 fill → light label");
        assert_eq!(label_on(SURF), ON_SURF, "gray-100 fill → light label");
    }

    /// Quantify the fix on the worst offender (Green-40): the chosen label clears
    /// WCAG AA (≥ 4.5 : 1) where the old raw white was well under 3 : 1.
    #[test]
    fn label_on_clears_aa_where_raw_white_failed() {
        let chosen = label_on(SUCCESS);
        assert!(
            contrast_ratio(chosen, SUCCESS) >= 4.5,
            "chip label must clear AA on Green-40, got {}",
            contrast_ratio(chosen, SUCCESS)
        );
        assert!(
            contrast_ratio(Color::WHITE, SUCCESS) < 3.0,
            "raw white on Green-40 was the failing baseline"
        );
    }

    /// Relative luminance spans the full black→white range (sanity-checks the
    /// sRGB linearisation).
    #[test]
    fn relative_luminance_spans_black_to_white() {
        assert!(relative_luminance(Color::BLACK) < 0.01);
        assert!(relative_luminance(Color::WHITE) > 0.99);
    }
}
