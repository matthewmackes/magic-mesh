//! Visual tokens — the IBM Carbon Gray-100 dark palette mde-files renders with
//! (E10.7, §4 Carbon-only). **AUD-9 (2026-06-11):** every value here is now
//! re-sourced from the canonical `mde_theme::carbon` ramp via the `c`/`ca` const
//! converters — `mde-theme` is the §4 single source, and these `PF_*`/legacy
//! names are call-site-stable *aliases* (the alpha-tints derive from
//! `carbon::BLUE_50`/`WHITE`), not a parallel hardcoded palette. `theme()`
//! builds the five-slot iced base palette from them; widget code reads the rest
//! directly for surface/border/row detail not covered by that base.

use cosmic::iced::Color;
use mde_theme::carbon;

/// §4 single-source: every color below derives from the canonical IBM Carbon
/// ramp in `mde_theme::carbon` via these two const converters — no raw hex
/// lives in this file. `c` is a fully-opaque ramp step; `ca` overrides the
/// alpha for tints/overlays.
const fn c(x: mde_theme::Rgba) -> Color {
    Color {
        r: x.r as f32 / 255.0,
        g: x.g as f32 / 255.0,
        b: x.b as f32 / 255.0,
        a: x.a,
    }
}

const fn ca(x: mde_theme::Rgba, a: f32) -> Color {
    Color {
        r: x.r as f32 / 255.0,
        g: x.g as f32 / 255.0,
        b: x.b as f32 / 255.0,
        a,
    }
}

const fn white_alpha(a: f32) -> Color {
    ca(carbon::WHITE, a)
}

// ─── IBM Carbon Gray-100 dark surface ramp (E10.7 / §4 single-source) ──────
// The dark layer ramp, deepest → lightest. Constant names keep their legacy
// `PF_*` spelling for call-site stability; the VALUES are re-sourced from
// `mde_theme::carbon` (the §4 single source — these are aliases, not copies).
pub const PF_BG_100: Color = c(carbon::GRAY_100); // Gray 100 — page bg
pub const PF_BG_200: Color = c(carbon::GRAY_90); // Gray 90 — titlebar/sidebar
pub const PF_BG_300: Color = c(carbon::GRAY_80); // Gray 80 — content/field
pub const PF_BG_400: Color = c(carbon::GRAY_70); // Gray 70 — overlay/hover
pub const PF_BORDER: Color = c(carbon::GRAY_70); // Gray 70 — border

pub const PF_TEXT_100: Color = c(carbon::GRAY_10); // Gray 10 — text primary
pub const PF_TEXT_200: Color = c(carbon::GRAY_30); // Gray 30 — text secondary
pub const PF_TEXT_300: Color = c(carbon::GRAY_50); // Gray 50 — text helper

// ─── Carbon Blue interactive accent (on dark) ──────────────────────────────
pub const ACCENT: Color = c(carbon::BLUE_50); // Blue 50 — on-dark accent
pub const ACCENT_HI: Color = c(carbon::BLUE_40); // Blue 40 — accent hover
/// The "self / local node" status hue — Carbon Teal 30, kept distinct from the
/// Blue mesh accent. (Legacy name `RUST`; value is `carbon::TEAL_30`.)
pub const RUST: Color = c(carbon::TEAL_30);

/// Carbon Blue 60 — primary-button fill.
pub const BUTTON_ACCENT: Color = c(carbon::BLUE_60);
/// Carbon Blue 50 — primary-button hover.
pub const BUTTON_ACCENT_HI: Color = c(carbon::BLUE_50);

// ─── Carbon support status colours (dark) ──────────────────────────────────
pub const PF_INFO: Color = c(carbon::BLUE_50); // Blue 50
pub const PF_SUCCESS: Color = c(carbon::GREEN_40); // Green 40
pub const PF_DANGER: Color = c(carbon::RED_50); // Red 50

// ─── Common derived colours / surfaces ─────────────────────────────────────
pub const BG: Color = PF_BG_100;
pub const FG: Color = PF_TEXT_100;
pub const FG_DIM: Color = PF_TEXT_200;
pub const FG_FAINT: Color = PF_TEXT_300;
pub const WINDOW: Color = PF_BG_300;

/// CR-4 — bridge the mde-files local theme tokens to the
/// `mde_theme::Palette` shape so `mde_iced_components::object_card`
/// renders against the same IBM Carbon surfaces + Blue accent as the
/// rest of the manager (E10.7 — Carbon-only, §1).
#[must_use]
pub fn mde_files_palette() -> mde_theme::Palette {
    use mde_theme::Rgba;
    let to_rgba = |c: Color| {
        Rgba::rgba(
            (c.r * 255.0).round() as u8,
            (c.g * 255.0).round() as u8,
            (c.b * 255.0).round() as u8,
            c.a,
        )
    };
    // The three semantic roles (success/danger/warning) are platform-wide
    // tokens single-sourced in `mde_theme` (E5.3); reuse them verbatim rather
    // than minting a divergent set here — only the surface/accent tiers are
    // mde-files-specific.
    let semantic = mde_theme::Palette::dark();
    mde_theme::Palette {
        background: to_rgba(BG),
        surface: to_rgba(WINDOW_TITLEBAR),
        raised: to_rgba(WINDOW),
        overlay: to_rgba(PF_BG_400),
        accent: to_rgba(ACCENT),
        border: to_rgba(PF_BORDER),
        text: to_rgba(FG),
        text_muted: to_rgba(FG_FAINT),
        success: semantic.success,
        danger: semantic.danger,
        warning: semantic.warning,
    }
}
pub const WINDOW_TITLEBAR: Color = PF_BG_200;
pub const WINDOW_SIDE: Color = c(carbon::GRAY_90); // Gray 90
pub const DIVIDER: Color = white_alpha(0.08);

pub const ROW_HOVER: Color = white_alpha(0.05);
pub const ROW_HOVER_FAINT: Color = white_alpha(0.03);
// E10.7 — the selected-row highlight + the "primary" tints are Carbon Blue 50
// (`carbon::BLUE_50`) tints. Names keep their legacy amber/rust spelling.
pub const ACTIVE_RUST_BG: Color = ca(carbon::BLUE_50, 0.16);
pub const ACTIVE_RUST_BORDER: Color = ACCENT;
pub const PRIMARY_AMBER_BG: Color = ca(carbon::BLUE_50, 0.06);
pub const PRIMARY_AMBER_BG_HOVER: Color = ca(carbon::BLUE_50, 0.12);
pub const PRIMARY_AMBER_BG_ACTIVE: Color = ca(carbon::BLUE_50, 0.18);
pub const PRIMARY_AMBER_BORDER: Color = ca(carbon::BLUE_50, 0.55);

pub const MESH_PILL_BG: Color = ca(carbon::BLUE_50, 0.10);
pub const MESH_PILL_BORDER: Color = ca(carbon::BLUE_50, 0.25);
pub const LOCAL_PILL_BG: Color = white_alpha(0.03);
pub const LOCAL_PILL_BORDER: Color = white_alpha(0.06);

pub const MESH_ROW_BG: Color = ca(carbon::BLUE_50, 0.025);
pub const MESH_ROW_BG_HOVER: Color = ca(carbon::BLUE_50, 0.06);

pub const BANNER_BORDER: Color = ca(carbon::BLUE_50, 0.18);
pub const BANNER_TINT_A: Color = ca(carbon::BLUE_50, 0.10);

pub const ROW_DIVIDER: Color = white_alpha(0.03);

/// Carbon Gray 80 list-divider.
pub const LIST_ROW_DIVIDER: Color = c(carbon::GRAY_80);
/// Carbon Blue 50 selection overlay at 15 % alpha.
pub const LIST_SELECTION_BG: Color = ca(carbon::BLUE_50, 0.15);

// ─── Dimensions ────────────────────────────────────────────────────────────
pub const WIN_W: f32 = 1480.0;
pub const WIN_H: f32 = 940.0;
pub const TITLEBAR_H: f32 = 32.0;
pub const SIDEBAR_W: f32 = 248.0;
pub const SIDE_ROW_PAD_Y: f32 = 5.0;
pub const SIDE_ROW_PAD_X: f32 = 14.0;
pub const SIDE_ROW_GAP: f32 = 10.0;

// ─── Font families ─────────────────────────────────────────────────────────
//
// Iced expects font *byte slices* registered up-front for custom fonts. The
// host system's Red Hat font installation is preferred (it ships with the MDE
// RPM); we expose the names so widgets can pick them by `iced::Font::with_name`.
pub const FONT_TEXT: &str = "Red Hat Text";
pub const FONT_DISPLAY: &str = "Red Hat Display";
pub const FONT_MONO: &str = "Red Hat Mono";

// ─── Status-dot colours ────────────────────────────────────────────────────
use crate::model::PeerStatus;

#[must_use]
pub fn peer_status_dot(status: PeerStatus) -> Color {
    match status {
        PeerStatus::Online => PF_SUCCESS,
        PeerStatus::Idle => ACCENT,
        PeerStatus::Offline => PF_BORDER,
        PeerStatus::Self_ => RUST,
    }
}

// ─── Iced theme ────────────────────────────────────────────────────────────

/// The Iced base theme — built directly from the IBM Carbon tokens above
/// (E10.7, §1 Carbon-only). The old `tokens.css` read is gone: that file
/// carries the Material-indigo GTK-era values and is shared, so seeding from it
/// re-introduced a non-Carbon base; the shell has no GTK layer to stay "in sync"
/// with any more. The accent is fixed Carbon Blue (the per-user accent picker
/// only retints icons, not the UI accent).
#[must_use]
pub fn theme() -> cosmic::iced::Theme {
    cosmic::iced::Theme::custom(
        "MDE".to_string(),
        cosmic::iced::theme::Palette {
            background: WINDOW,
            text: FG,
            primary: ACCENT,
            warning: c(carbon::YELLOW_30), // Carbon Yellow 30
            success: PF_SUCCESS,
            danger: PF_DANGER,
        },
    )
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

    /// E10.7 / §1 — pin the core palette to its IBM Carbon Gray-100 dark token
    /// values, so a future drift back toward the retired Material/PatternFly
    /// warm-dark palette fails here. (Carbon spec: carbondesignsystem.com.)
    #[test]
    fn palette_is_carbon_gray_100_dark() {
        // Gray ramp (surface tiers).
        assert_eq!(rgb8(PF_BG_100), (0x16, 0x16, 0x16), "bg = Gray 100");
        assert_eq!(rgb8(PF_BG_200), (0x26, 0x26, 0x26), "titlebar = Gray 90");
        assert_eq!(rgb8(PF_BG_300), (0x39, 0x39, 0x39), "content = Gray 80");
        assert_eq!(rgb8(PF_BG_400), (0x52, 0x52, 0x52), "overlay = Gray 70");
        // Text ramp.
        assert_eq!(rgb8(PF_TEXT_100), (0xf4, 0xf4, 0xf4), "text = Gray 10");
        assert_eq!(rgb8(PF_TEXT_300), (0x8d, 0x8d, 0x8d), "helper = Gray 50");
        // Interactive accent + primary button = Carbon Blue (no amber/indigo).
        assert_eq!(rgb8(ACCENT), (0x45, 0x89, 0xff), "accent = Blue 50");
        assert_eq!(rgb8(BUTTON_ACCENT), (0x0f, 0x62, 0xfe), "button = Blue 60");
        // Carbon support colours.
        assert_eq!(rgb8(PF_SUCCESS), (0x42, 0xbe, 0x65), "success = Green 40");
        assert_eq!(rgb8(PF_DANGER), (0xfa, 0x4d, 0x56), "danger = Red 50");
    }

    /// The Iced base theme is built from the Carbon constants (not the retired
    /// Material `tokens.css`), so the runtime background/accent are Carbon
    /// regardless of any installed token file.
    #[test]
    fn iced_base_theme_is_carbon() {
        let pal = theme().palette();
        assert_eq!(rgb8(pal.background), (0x39, 0x39, 0x39)); // WINDOW = Gray 80
        assert_eq!(rgb8(pal.primary), (0x45, 0x89, 0xff)); // accent = Blue 50
        assert_eq!(rgb8(pal.text), (0xf4, 0xf4, 0xf4)); // Gray 10
    }
}
