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
/// Outline / divider line — Gray 70.
pub const OUTLINE: Color = tok(carbon::GRAY_70);
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

// ---------- Primary (Carbon Blue accent) ----------

/// On-dark interactive accent — Blue 50.
pub const PRIMARY: Color = tok(carbon::BLUE_50);
/// Text on the Blue fill — White.
pub const ON_PRIMARY: Color = tok(carbon::WHITE);

// ---------- Presence pip colors ----------

pub const PRESENCE_AVAILABLE: Color = SUCCESS;
pub const PRESENCE_OFFLINE: Color = OUTLINE;

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
    }
}
