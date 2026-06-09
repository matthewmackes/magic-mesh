//! Color tokens + spacing constants for `mde-voice-hud`.
//!
//! The HUD is a Portal-full overlay surface. E9.9 (§1) — its palette is the
//! IBM Carbon Gray-100 dark tokens + Blue accent, like the rest of the
//! platform (the original Material-3 / Mackes-orange sub-palette predated the
//! Carbon-only collapse and was a §1 violation). The Material-3 *role* names
//! (`SURF`, `ON_SURF`, `PRIMARY`…) are kept for call-site stability; the values
//! are Carbon now.
//!
//! Per the design-tokens lint snapshot allowlist, hex literals outside
//! `data/css/tokens.css` get caught — this module is one of the canonical token
//! sites for the voice-HUD surface and is the only place that carries them.

use iced::Color;

/// Helper: convert an 8-bit RGB hex into an Iced `Color`.
const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}
const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a,
    }
}

// ---------- Surface palette (IBM Carbon Gray-100 dark ramp) ----------

/// Base surface — Carbon Gray 100.
pub const SURF: Color = rgb(0x16, 0x16, 0x16);
/// Dim variant for the inner SIP-log + activity panels — Gray 100.
pub const SURF_DIM: Color = rgb(0x16, 0x16, 0x16);
/// Low-elevation container (topbar / call bar) — Gray 90.
pub const SURF_C_LOW: Color = rgb(0x26, 0x26, 0x26);
/// Mid-elevation container (keypad keys / hop pills) — Gray 80.
pub const SURF_C: Color = rgb(0x39, 0x39, 0x39);
/// High-elevation hover state — Gray 70.
pub const SURF_C_HI: Color = rgb(0x52, 0x52, 0x52);
/// Hierarchical accent surface (selected tab / avatar bg) — Gray 60.
pub const SURF_C_HIER: Color = rgb(0x6f, 0x6f, 0x6f);
/// Outline / divider line — Gray 70.
pub const OUTLINE: Color = rgb(0x52, 0x52, 0x52);
/// Outline variant (lighter divider) — Gray 80.
pub const OUTLINE_VAR: Color = rgb(0x39, 0x39, 0x39);

// ---------- Foreground palette (Carbon text ramp) ----------

pub const ON_SURF: Color = rgb(0xf4, 0xf4, 0xf4); // Gray 10
pub const ON_SURF_VAR: Color = rgb(0xc6, 0xc6, 0xc6); // Gray 30
pub const ON_SURF_MUTED: Color = rgb(0x8d, 0x8d, 0x8d); // Gray 50

// ---------- Status colors (Carbon support, dark) ----------

pub const SUCCESS: Color = rgb(0x42, 0xbe, 0x65); // Green 40
pub const WARNING: Color = rgb(0xf1, 0xc2, 0x1b); // Yellow 30
pub const ERROR: Color = rgb(0xfa, 0x4d, 0x56); // Red 50
pub const INFO: Color = rgb(0x78, 0xa9, 0xff); // Blue 40

// ---------- Primary (Carbon Blue accent) ----------

pub const PRIMARY: Color = rgb(0x45, 0x89, 0xff); // Blue 50 (on-dark accent)
pub const ON_PRIMARY: Color = rgb(0xff, 0xff, 0xff); // text on the Blue fill
pub const PRIMARY_C: Color = rgb(0x00, 0x43, 0xce); // Blue 70 (container)
pub const ON_PRIMARY_C: Color = rgb(0xd0, 0xe2, 0xff); // Blue 20 (on-container)
pub const PRIMARY_FIXED: Color = rgb(0x78, 0xa9, 0xff); // Blue 40 (on-call pip)

// ---------- Accept / decline FAB ----------

/// Background for the Call FAB — Carbon Green 60.
pub const ACCEPT_C: Color = rgb(0x19, 0x80, 0x38);
/// Foreground glyph color on the Call FAB — Green 30.
pub const ACCEPT_FG: Color = rgb(0x6f, 0xdc, 0x8c);
/// Background for the Hangup FAB — Carbon Red 60.
pub const ERROR_C: Color = rgb(0xda, 0x1e, 0x28);
pub const ON_ERROR_C: Color = rgb(0xff, 0xff, 0xff);

// ---------- Presence pip colors ----------

pub const PRESENCE_AVAILABLE: Color = SUCCESS;
pub const PRESENCE_ON_CALL: Color = PRIMARY_FIXED;
pub const PRESENCE_AWAY: Color = WARNING;
pub const PRESENCE_DND: Color = ERROR;
pub const PRESENCE_OFFLINE: Color = OUTLINE;

// ---------- Translucent overlays ----------

pub const SCRIM_55: Color = rgba(0x00, 0x00, 0x00, 0.55);
pub const HOVER_TINT_8: Color = rgba(0xff, 0xff, 0xff, 0.04);

// ---------- HUD dimensions ----------

/// Cozy default width (px).
pub const HUD_W: f32 = 420.0;
/// Cozy default height (px).
pub const HUD_H: f32 = 720.0;
/// Bottom margin above the dock (px).
pub const HUD_MARGIN_BOTTOM: i32 = 56;
/// Right margin from the screen edge (px).
pub const HUD_MARGIN_RIGHT: i32 = 16;
/// Row height for keypad keys + peer rows (cozy density, px).
pub const ROW_H: f32 = 64.0;

// ---------- Border radii ----------

pub const R_XS: f32 = 8.0;
pub const R_S: f32 = 12.0;
pub const R_M: f32 = 16.0;
pub const R_L: f32 = 20.0;
pub const R_XL: f32 = 28.0;
pub const R_FULL: f32 = 999.0;

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
}
