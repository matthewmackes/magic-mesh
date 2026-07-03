//! Classic terminal colour presets (TERM-11) — the one place raw hex is legit.
//!
//! §4 keeps raw colour out of the UI, and [`crate::palette`] carves out the
//! *content* palette. This module is the documented home for the **classic
//! schemes' defining hex**: Solarized (Ethan Schoonover), Gruvbox (morhetz), and
//! Nord (Arctic Ice Studio) each publish an exact 16-colour ANSI table plus a
//! default foreground/background. Those constants only mean anything *as* their
//! published values, so reproducing them here is the correct representation —
//! not a §4 violation but the data a "classic preset" *is*. Each builds a
//! [`Palette`] of the same shape the Quasar default ([`Palette::from_tokens`])
//! does, so the renderer treats a preset and the default identically.
//!
//! [`Preset`] is the pickable set (the Quasar default plus the four classics)
//! the appearance picker lists; [`Preset::palette`] resolves each to its
//! [`Palette`]. The default's arm delegates to the token derivation (no hex).

// The classic schemes publish their colours as packed `#rrggbb` hex; the tables
// below read far cleaner un-separated (`0x073642`) than digit-grouped, so this
// data module opts out of the separator lint for its published constants.
#![allow(clippy::unreadable_literal)]

use mde_egui::egui::Color32;

use crate::palette::Palette;

/// Short for a `#rrggbb` literal — keeps the tables below scannable.
const fn hex(rgb: u32) -> Color32 {
    #[allow(clippy::cast_possible_truncation)]
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Assemble a preset [`Palette`] from its 16-slot table and role colours.
const fn scheme(ansi: [u32; 16], fg: u32, bg: u32, cursor: u32) -> Palette {
    Palette {
        ansi: [
            hex(ansi[0]),
            hex(ansi[1]),
            hex(ansi[2]),
            hex(ansi[3]),
            hex(ansi[4]),
            hex(ansi[5]),
            hex(ansi[6]),
            hex(ansi[7]),
            hex(ansi[8]),
            hex(ansi[9]),
            hex(ansi[10]),
            hex(ansi[11]),
            hex(ansi[12]),
            hex(ansi[13]),
            hex(ansi[14]),
            hex(ansi[15]),
        ],
        fg: hex(fg),
        bg: hex(bg),
        cursor: hex(cursor),
    }
}

// ── Solarized (Ethan Schoonover) ─────────────────────────────────────────────
// The two Solarized modes share one 16-colour ANSI table and differ only in
// which base tone is the default fg/bg — so `dark` and `light` are the same
// slots on inverted bases, exactly as published.

/// The shared Solarized ANSI-16 table.
const SOLARIZED_ANSI: [u32; 16] = [
    0x073642, // 0  black        base02
    0xdc322f, // 1  red          red
    0x859900, // 2  green        green
    0xb58900, // 3  yellow       yellow
    0x268bd2, // 4  blue         blue
    0xd33682, // 5  magenta      magenta
    0x2aa198, // 6  cyan         cyan
    0xeee8d5, // 7  white        base2
    0x002b36, // 8  br black     base03
    0xcb4b16, // 9  br red       orange
    0x586e75, // 10 br green     base01
    0x657b83, // 11 br yellow    base00
    0x839496, // 12 br blue      base0
    0x6c71c4, // 13 br magenta   violet
    0x93a1a1, // 14 br cyan      base1
    0xfdf6e3, // 15 br white     base3
];

/// Solarized Dark — base03 background, base0 foreground.
#[must_use]
pub const fn solarized_dark() -> Palette {
    scheme(SOLARIZED_ANSI, 0x839496, 0x002b36, 0x839496)
}

/// Solarized Light — base3 background, base00 foreground.
#[must_use]
pub const fn solarized_light() -> Palette {
    scheme(SOLARIZED_ANSI, 0x657b83, 0xfdf6e3, 0x657b83)
}

// ── Gruvbox dark (morhetz) ───────────────────────────────────────────────────

/// Gruvbox (dark, medium contrast).
#[must_use]
pub const fn gruvbox_dark() -> Palette {
    scheme(
        [
            0x282828, // 0  black       bg0
            0xcc241d, // 1  red         red
            0x98971a, // 2  green       green
            0xd79921, // 3  yellow      yellow
            0x458588, // 4  blue        blue
            0xb16286, // 5  magenta     purple
            0x689d6a, // 6  cyan        aqua
            0xa89984, // 7  white       fg4/gray
            0x928374, // 8  br black    gray
            0xfb4934, // 9  br red      br red
            0xb8bb26, // 10 br green    br green
            0xfabd2f, // 11 br yellow   br yellow
            0x83a598, // 12 br blue     br blue
            0xd3869b, // 13 br magenta  br purple
            0x8ec07c, // 14 br cyan     br aqua
            0xebdbb2, // 15 br white    fg1
        ],
        0xebdbb2, // fg  fg1
        0x282828, // bg  bg0
        0xebdbb2, // cursor
    )
}

// ── Nord (Arctic Ice Studio) ─────────────────────────────────────────────────

/// Nord — the official terminal ANSI mapping over the nord0..nord15 tones.
#[must_use]
pub const fn nord() -> Palette {
    scheme(
        [
            0x3b4252, // 0  black       nord1
            0xbf616a, // 1  red         nord11
            0xa3be8c, // 2  green       nord14
            0xebcb8b, // 3  yellow      nord13
            0x81a1c1, // 4  blue        nord9
            0xb48ead, // 5  magenta     nord15
            0x88c0d0, // 6  cyan        nord8
            0xe5e9f0, // 7  white       nord5
            0x4c566a, // 8  br black    nord3
            0xbf616a, // 9  br red      nord11
            0xa3be8c, // 10 br green    nord14
            0xebcb8b, // 11 br yellow   nord13
            0x81a1c1, // 12 br blue     nord9
            0xb48ead, // 13 br magenta  nord15
            0x8fbcbb, // 14 br cyan     nord7
            0xeceff4, // 15 br white    nord6
        ],
        0xd8dee9, // fg  nord4
        0x2e3440, // bg  nord0
        0xd8dee9, // cursor
    )
}

/// A user-pickable colour scheme: the Quasar default (token-derived) plus the
/// bundled classics. The appearance picker lists [`Self::ALL`]; each resolves to
/// a [`Palette`] via [`Self::palette`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Preset {
    /// The platform default, derived from `mde-theme`/`Style` tokens.
    #[default]
    Quasar,
    /// Solarized, dark mode.
    SolarizedDark,
    /// Solarized, light mode.
    SolarizedLight,
    /// Gruvbox, dark.
    Gruvbox,
    /// Nord.
    Nord,
}

impl Preset {
    /// Every pickable preset, in the picker's display order.
    pub const ALL: [Self; 5] = [
        Self::Quasar,
        Self::SolarizedDark,
        Self::SolarizedLight,
        Self::Gruvbox,
        Self::Nord,
    ];

    /// The picker label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Quasar => "Quasar",
            Self::SolarizedDark => "Solarized Dark",
            Self::SolarizedLight => "Solarized Light",
            Self::Gruvbox => "Gruvbox",
            Self::Nord => "Nord",
        }
    }

    /// The scheme this preset selects. The default's arm is the token derivation
    /// (no hex); the classics are this module's published data.
    #[must_use]
    pub const fn palette(self) -> Palette {
        match self {
            Self::Quasar => Palette::from_tokens(),
            Self::SolarizedDark => solarized_dark(),
            Self::SolarizedLight => solarized_light(),
            Self::Gruvbox => gruvbox_dark(),
            Self::Nord => nord(),
        }
    }

    /// The preset whose [`Palette`] equals `palette`, if any — how the picker
    /// marks the active choice. `None` for a scheme not in the bundled set.
    #[must_use]
    pub fn matching(palette: &Palette) -> Option<Self> {
        Self::ALL.into_iter().find(|p| &p.palette() == palette)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_unpacks_channels() {
        assert_eq!(hex(0x00_2b_36), Color32::from_rgb(0x00, 0x2b, 0x36));
        assert_eq!(hex(0xff_ff_ff), Color32::from_rgb(255, 255, 255));
    }

    #[test]
    fn every_preset_has_a_full_16_slot_table_and_roles() {
        for preset in Preset::ALL {
            let p = preset.palette();
            assert_eq!(p.ansi.len(), 16, "{}", preset.label());
            // Slot resolution reads the scheme's own table for the low range.
            assert_eq!(p.color(0), p.ansi[0]);
            assert_eq!(p.color(15), p.ansi[15]);
            // The reset colours are set (bg differs from fg for a legible scheme).
            assert_ne!(p.fg, p.bg, "{} needs contrast", preset.label());
        }
    }

    #[test]
    fn solarized_dark_and_light_share_ansi_but_invert_the_base() {
        let dark = solarized_dark();
        let light = solarized_light();
        // Published truth: the 16 ANSI slots are identical between modes …
        assert_eq!(dark.ansi, light.ansi);
        // … and only the base fg/bg flips, so the two are still distinct schemes.
        assert_ne!(dark, light);
        assert_ne!(dark.bg, light.bg);
        // Spot-check the defining hex so a transcription slip is caught.
        assert_eq!(dark.bg, Color32::from_rgb(0x00, 0x2b, 0x36)); // base03
        assert_eq!(light.bg, Color32::from_rgb(0xfd, 0xf6, 0xe3)); // base3
        assert_eq!(dark.color(1), Color32::from_rgb(0xdc, 0x32, 0x2f)); // red
    }

    #[test]
    fn nord_and_gruvbox_carry_their_signature_backgrounds() {
        assert_eq!(nord().bg, Color32::from_rgb(0x2e, 0x34, 0x40)); // nord0
        assert_eq!(nord().color(4), Color32::from_rgb(0x81, 0xa1, 0xc1)); // nord9
        assert_eq!(gruvbox_dark().bg, Color32::from_rgb(0x28, 0x28, 0x28)); // bg0
        assert_eq!(gruvbox_dark().color(3), Color32::from_rgb(0xd7, 0x99, 0x21));
        // yellow
    }

    #[test]
    fn the_presets_are_pairwise_distinct() {
        let all: Vec<Palette> = Preset::ALL.iter().map(|p| p.palette()).collect();
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b, "presets must be distinguishable");
            }
        }
    }

    #[test]
    fn matching_round_trips_each_preset_and_default() {
        for preset in Preset::ALL {
            assert_eq!(Preset::matching(&preset.palette()), Some(preset));
        }
        // The token default resolves to the Quasar preset (its palette equals it).
        assert_eq!(
            Preset::matching(&Palette::from_tokens()),
            Some(Preset::Quasar)
        );
        // A scheme outside the bundled set matches nothing.
        let custom = Palette {
            fg: Color32::from_rgb(1, 2, 3),
            ..Palette::from_tokens()
        };
        assert_eq!(Preset::matching(&custom), None);
    }
}
