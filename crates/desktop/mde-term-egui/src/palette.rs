//! The terminal **content palette** — the one sanctioned §4 carve-out.
//!
//! §4 governance forbids raw colour literals outside the shared [`Style`]
//! module: chrome renders through tokens. Terminal *content* is different in
//! kind — programs address a standardised 16/256-colour space by **index**
//! (`SGR 30..37`, `38;5;n`, bright variants), and those indices must resolve to
//! stable ANSI-shaped hues whatever the chrome looks like: the red in
//! `ls --color` belongs to the program, not the design system. The
//! mesh-terminal design (lock 14) therefore carves the content palette out
//! explicitly: **this module is the only place in the crate a raw colour value
//! may appear**, every raw value is a named ANSI-slot constant, and the chrome
//! around the grid (background, cursor, selection, chips) still renders purely
//! through `Style` tokens.
//!
//! The default table is **Quasar-derived wherever a token carries the same
//! meaning** — red = `DANGER`, green = `OK`, yellow = `WARN`, blue = `ACCENT`,
//! white = `TEXT`, bright blue = `ACCENT_HI`, black = `BG` — so terminal
//! content sits naturally on the platform look. Slots with no token
//! equivalent (magenta, cyan, the remaining brights) are standard ANSI hues
//! tuned for the dark Quasar background. The classic preset tables
//! (Solarized/Gruvbox/Nord, user-pickable) are TERM-11.

use mde_egui::egui::Color32;
use mde_egui::Style;

use crate::screen::{Cell, CellColor};

// ── The 16 ANSI slots (0..=7 normal, 8..=15 bright) ────────────────────────

/// Slot 0 — black. The app background token, so "black" content melts into
/// the chrome exactly as it does in a classic dark terminal.
pub const BLACK: Color32 = Style::BG;
/// Slot 1 — red (the platform's danger token).
pub const RED: Color32 = Style::DANGER;
/// Slot 2 — green (the platform's ok token).
pub const GREEN: Color32 = Style::OK;
/// Slot 3 — yellow (the platform's warn token).
pub const YELLOW: Color32 = Style::WARN;
/// Slot 4 — blue (the platform's accent token).
pub const BLUE: Color32 = Style::ACCENT;
/// Slot 5 — magenta. No token equivalent; a standard ANSI hue for the dark bg.
pub const MAGENTA: Color32 = Color32::from_rgb(0xC6, 0x78, 0xDD);
/// Slot 6 — cyan. No token equivalent; a standard ANSI hue for the dark bg.
pub const CYAN: Color32 = Color32::from_rgb(0x56, 0xB6, 0xC2);
/// Slot 7 — white (the platform's primary text token).
pub const WHITE: Color32 = Style::TEXT;
/// Slot 8 — bright black: the conventional de-emphasis grey, kept darker than
/// `TEXT_DIM` so it also works as a background block.
pub const BRIGHT_BLACK: Color32 = Color32::from_rgb(0x52, 0x52, 0x5E);
/// Slot 9 — bright red (the danger hue, lifted).
pub const BRIGHT_RED: Color32 = Color32::from_rgb(0xFF, 0x8A, 0x85);
/// Slot 10 — bright green (the ok hue, lifted).
pub const BRIGHT_GREEN: Color32 = Color32::from_rgb(0x7F, 0xE3, 0xAC);
/// Slot 11 — bright yellow (the warn hue, lifted).
pub const BRIGHT_YELLOW: Color32 = Color32::from_rgb(0xFF, 0xCE, 0x8E);
/// Slot 12 — bright blue (the platform's hovered-accent token).
pub const BRIGHT_BLUE: Color32 = Style::ACCENT_HI;
/// Slot 13 — bright magenta.
pub const BRIGHT_MAGENTA: Color32 = Color32::from_rgb(0xDD, 0xA2, 0xEC);
/// Slot 14 — bright cyan.
pub const BRIGHT_CYAN: Color32 = Color32::from_rgb(0x8A, 0xD7, 0xE1);
/// Slot 15 — bright white.
pub const BRIGHT_WHITE: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);

/// The 16-slot ANSI table, indexed by slot number.
pub const ANSI16: [Color32; 16] = [
    BLACK,
    RED,
    GREEN,
    YELLOW,
    BLUE,
    MAGENTA,
    CYAN,
    WHITE,
    BRIGHT_BLACK,
    BRIGHT_RED,
    BRIGHT_GREEN,
    BRIGHT_YELLOW,
    BRIGHT_BLUE,
    BRIGHT_MAGENTA,
    BRIGHT_CYAN,
    BRIGHT_WHITE,
];

/// One level of the xterm 6×6×6 colour cube: `0, 95, 135, 175, 215, 255` —
/// the standard 256-colour table every terminal program calibrates against.
const fn cube_level(step: u8) -> u8 {
    if step == 0 {
        0
    } else {
        55 + step * 40
    }
}

/// Resolve a 256-colour palette slot: the 16 ANSI names, then the xterm
/// 6×6×6 colour cube (`16..=231`), then the 24-step greyscale ramp
/// (`232..=255`). Total function — every `u8` is a defined colour.
#[must_use]
pub const fn indexed(slot: u8) -> Color32 {
    match slot {
        0..=15 => ANSI16[slot as usize],
        16..=231 => {
            let n = slot - 16;
            Color32::from_rgb(
                cube_level(n / 36),
                cube_level((n / 6) % 6),
                cube_level(n % 6),
            )
        }
        // 8, 18, 28, … 238 — the xterm greyscale ramp.
        232..=255 => {
            let v = 8 + (slot - 232) * 10;
            Color32::from_rgb(v, v, v)
        }
    }
}

/// Lift a colour toward white by 2/5 — the visible weight cue for bold runs.
///
/// egui lays out one embedded face (no bold variant), so bold renders as the
/// classic terminal treatment: bright-slot promotion for the low palette plus
/// this lift. A real bold face (and ligatures) is TERM-13's rendering-fidelity
/// unit.
const fn lift(c: Color32) -> Color32 {
    // u16 arithmetic; each channel result is ≤ 255 by construction.
    #[allow(clippy::cast_possible_truncation)]
    const fn ch(v: u8) -> u8 {
        (v as u16 + (255 - v as u16) * 2 / 5) as u8
    }
    Color32::from_rgb(ch(c.r()), ch(c.g()), ch(c.b()))
}

/// Resolve one cell to concrete `(fg, bg)` paint colours.
///
/// - `Default` maps to the chrome tokens (`TEXT` on `BG`) so untouched text
///   *is* the platform look;
/// - `Palette(n)` resolves through [`indexed`], with bold promoting the low
///   slots to their bright twins (the classic `SGR 1` behaviour);
/// - `Rgb` passes straight through — **true-colour already works** (design
///   lock 13/20: the engine carries `Rgb` cells end-to-end; TERM-13 only adds
///   the remaining fidelity extras);
/// - `bold` lifts, `dim` fades, `inverse` swaps, `hidden` conceals (fg = bg —
///   the glyph vanishes but stays real for selection/copy).
#[must_use]
pub fn cell_colors(cell: &Cell) -> (Color32, Color32) {
    let attrs = cell.attrs;
    let mut fg = match cell.fg {
        CellColor::Default => Style::TEXT,
        CellColor::Palette(slot) => indexed(if attrs.bold && slot < 8 {
            slot + 8
        } else {
            slot
        }),
        CellColor::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
    };
    let mut bg = match cell.bg {
        CellColor::Default => Style::BG,
        CellColor::Palette(slot) => indexed(slot),
        CellColor::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
    };
    if attrs.bold {
        fg = lift(fg);
    }
    if attrs.dim {
        // Translucent over the cell's bg rect reads as faint.
        fg = fg.gamma_multiply(0.6);
    }
    if attrs.inverse {
        core::mem::swap(&mut fg, &mut bg);
    }
    if attrs.hidden {
        fg = bg;
    }
    (fg, bg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::CellAttrs;

    fn cell(fg: CellColor, bg: CellColor, attrs: CellAttrs) -> Cell {
        Cell {
            ch: 'x',
            fg,
            bg,
            attrs,
        }
    }

    #[test]
    fn the_16_slot_table_is_wired_and_token_derived_where_it_can_be() {
        // Every named slot resolves through `indexed`.
        for (slot, &c) in ANSI16.iter().enumerate() {
            assert_eq!(indexed(u8::try_from(slot).expect("slot fits")), c);
        }
        // The Quasar-token derivations (the §4 carve-out's "derive where
        // sensible" clause).
        assert_eq!(RED, Style::DANGER);
        assert_eq!(GREEN, Style::OK);
        assert_eq!(YELLOW, Style::WARN);
        assert_eq!(BLUE, Style::ACCENT);
        assert_eq!(WHITE, Style::TEXT);
        assert_eq!(BRIGHT_BLUE, Style::ACCENT_HI);
        assert_eq!(BLACK, Style::BG);
    }

    #[test]
    fn the_colour_cube_matches_the_xterm_levels() {
        // Corners of the 6×6×6 cube.
        assert_eq!(indexed(16), Color32::from_rgb(0, 0, 0));
        assert_eq!(indexed(231), Color32::from_rgb(255, 255, 255));
        // 196 = 16 + 36*5 → pure max red.
        assert_eq!(indexed(196), Color32::from_rgb(255, 0, 0));
        // 46 = 16 + 6*5 → pure max green; 21 → pure max blue.
        assert_eq!(indexed(46), Color32::from_rgb(0, 255, 0));
        assert_eq!(indexed(21), Color32::from_rgb(0, 0, 255));
        // An interior point: 60 = 16 + 36*1 + 6*1 + 2 → (95, 95, 135).
        assert_eq!(indexed(60), Color32::from_rgb(95, 95, 135));
    }

    #[test]
    fn the_greyscale_ramp_matches_the_xterm_levels() {
        assert_eq!(indexed(232), Color32::from_rgb(8, 8, 8));
        assert_eq!(indexed(244), Color32::from_rgb(128, 128, 128));
        assert_eq!(indexed(255), Color32::from_rgb(238, 238, 238));
    }

    #[test]
    fn default_cells_are_the_chrome_tokens() {
        let (fg, bg) = cell_colors(&cell(
            CellColor::Default,
            CellColor::Default,
            CellAttrs::default(),
        ));
        assert_eq!(fg, Style::TEXT);
        assert_eq!(bg, Style::BG);
    }

    #[test]
    fn truecolor_passes_straight_through() {
        // Lock 13/20: the engine carries Rgb cells, so 24-bit colour already
        // renders — no palette quantisation on this path.
        let (fg, bg) = cell_colors(&cell(
            CellColor::Rgb(10, 20, 30),
            CellColor::Rgb(200, 100, 50),
            CellAttrs::default(),
        ));
        assert_eq!(fg, Color32::from_rgb(10, 20, 30));
        assert_eq!(bg, Color32::from_rgb(200, 100, 50));
    }

    #[test]
    fn bold_promotes_low_slots_and_visibly_lifts_everything() {
        let bold = CellAttrs {
            bold: true,
            ..CellAttrs::default()
        };
        // SGR 1 + red → the bright-red slot (then lifted).
        let (fg, _) = cell_colors(&cell(CellColor::Palette(1), CellColor::Default, bold));
        assert_eq!(fg, lift(BRIGHT_RED));
        // A high slot is not promoted, only lifted.
        let (fg, _) = cell_colors(&cell(CellColor::Palette(123), CellColor::Default, bold));
        assert_eq!(fg, lift(indexed(123)));
        // Default-fg bold (the everyday `\e[1m`) must be visibly distinct.
        let (plain, _) = cell_colors(&cell(
            CellColor::Default,
            CellColor::Default,
            CellAttrs::default(),
        ));
        let (bolded, _) = cell_colors(&cell(CellColor::Default, CellColor::Default, bold));
        assert_ne!(plain, bolded, "bold default text must not paint like plain");
    }

    #[test]
    fn inverse_swaps_and_hidden_conceals() {
        let inverse = CellAttrs {
            inverse: true,
            ..CellAttrs::default()
        };
        let (fg, bg) = cell_colors(&cell(CellColor::Palette(1), CellColor::Palette(4), inverse));
        assert_eq!((fg, bg), (BLUE, RED));

        let hidden = CellAttrs {
            hidden: true,
            ..CellAttrs::default()
        };
        let (fg, bg) = cell_colors(&cell(CellColor::Palette(1), CellColor::Palette(4), hidden));
        assert_eq!(fg, bg, "a concealed glyph paints invisibly");
    }

    #[test]
    fn dim_fades_the_foreground() {
        let dim = CellAttrs {
            dim: true,
            ..CellAttrs::default()
        };
        let (faint, _) = cell_colors(&cell(CellColor::Default, CellColor::Default, dim));
        assert_ne!(faint, Style::TEXT, "dim text must not paint like plain");
    }
}
