//! The terminal **content palette** — the one sanctioned §4 carve-out.
//!
//! §4 governance forbids raw colour literals outside the shared [`Style`]
//! module: chrome renders through tokens. Terminal *content* is different in
//! kind — programs address a standardised 16/256-colour space by **index**
//! (`SGR 30..37`, `38;5;n`, bright variants), and those indices must resolve to
//! stable ANSI-shaped hues whatever the chrome looks like: the red in
//! `ls --color` belongs to the program, not the design system. The
//! mesh-terminal design (lock 14) therefore carves the content palette out
//! explicitly: **this module and [`crate::presets`] are the only places in the
//! crate a raw colour value may appear**, and the chrome around the grid (tab
//! bar, chips, borders, selection overlay, focus ring, the pickers) still
//! renders purely through `Style` tokens.
//!
//! ## The [`Palette`] model (TERM-11)
//!
//! Where TERM-3 shipped one fixed table, TERM-11 makes the content palette a
//! runtime value: a [`Palette`] carries the 16 ANSI slots plus the terminal's
//! default foreground / background / cursor colours (the three "role" colours a
//! `SGR 0` reset and the cursor resolve to). [`Palette::from_tokens`] is the
//! **Construct default, derived from `mde-theme`/`Style` tokens wherever a token
//! carries the same meaning** — red = `DANGER`, green = `OK`, yellow = `WARN`,
//! blue = `ACCENT`, white = `TEXT`, bright blue = `ACCENT_HI`, black/bg = `BG`,
//! fg/cursor = `TEXT` — so terminal content sits naturally on the platform
//! look. Slots with no token equivalent (magenta, cyan, the remaining brights)
//! are standard ANSI hues tuned for the dark Construct background.
//!
//! The **classic presets** (Solarized dark/light, Gruvbox, Nord), user-pickable
//! via the appearance picker, are data in [`crate::presets`]: their defining hex
//! is the one legitimate place those literals live, and they build [`Palette`]s
//! of the exact same shape. A preset's default fg/bg/cursor *are* content (they
//! theme the grid area), so the grid background and default-cell colours follow
//! the active palette; the surrounding chrome stays pure `Style` tokens.

use mde_egui::egui::Color32;
use mde_egui::Style;

use crate::screen::{Cell, CellColor};

// ── The 16 ANSI slots of the Construct default (0..=7 normal, 8..=15 bright) ────

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

/// The Construct default 16-slot ANSI table, indexed by slot number.
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

/// A terminal colour scheme: the 16 ANSI slots plus the three "role" colours a
/// reset / cursor resolve to.
///
/// The 16 ANSI slots redefine what `SGR 30..37`/`90..97` and `38;5;0..15` paint;
/// the higher 256-colour range (`16..=255`) is the fixed xterm cube + greyscale
/// ([`indexed`]) and is intentionally *not* themed, exactly as every classic
/// scheme leaves it. `fg`/`bg` are the default foreground/background a `SGR 0`
/// reset resolves to (and `bg` fills the grid); `cursor` is the block/bar/
/// underline cursor's colour. All are the content carve-out — a preset themes
/// the grid area, while the app chrome stays `Style` tokens.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Palette {
    /// The 16 ANSI slots (`0..=7` normal, `8..=15` bright).
    pub ansi: [Color32; 16],
    /// The default foreground (`SGR 0` / [`CellColor::Default`] fg).
    pub fg: Color32,
    /// The default background (the grid fill + [`CellColor::Default`] bg).
    pub bg: Color32,
    /// The cursor colour (block fill / bar / underline / hollow outline).
    pub cursor: Color32,
}

impl Palette {
    /// The **Construct default**, derived from `mde-theme`/`Style` tokens — the
    /// platform look (see the module docs). Not hand-picked hex: every slot that
    /// carries a token meaning *is* that token.
    #[must_use]
    pub const fn from_tokens() -> Self {
        Self {
            ansi: ANSI16,
            fg: Style::TEXT,
            bg: Style::BG,
            cursor: Style::TEXT,
        }
    }

    /// Resolve a 256-colour palette slot against this scheme: the 16 ANSI slots
    /// come from [`Self::ansi`]; `16..=255` is the fixed xterm cube + greyscale
    /// ([`indexed`]), unthemed. Total over every `u8`.
    #[must_use]
    pub const fn color(&self, slot: u8) -> Color32 {
        if (slot as usize) < self.ansi.len() {
            self.ansi[slot as usize]
        } else {
            indexed(slot)
        }
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::from_tokens()
    }
}

/// One level of the xterm 6×6×6 colour cube: `0, 95, 135, 175, 215, 255` —
/// the standard 256-colour table every terminal program calibrates against.
const fn cube_level(step: u8) -> u8 {
    if step == 0 {
        0
    } else {
        55 + step * 40
    }
}

/// Resolve a 256-colour palette slot against the **Construct default**.
///
/// The 16 ANSI names, then the xterm 6×6×6 colour cube (`16..=231`), then the
/// 24-step greyscale ramp (`232..=255`) — a total function, every `u8` a defined
/// colour. [`Palette::color`] overrides only the `0..16` range per scheme; the
/// cube + greyscale it defers to here are shared by every palette.
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

/// Resolve one cell to concrete `(fg, bg)` paint colours **through `palette`**.
///
/// - `Default` maps to the palette's role colours (`fg` on `bg`) so untouched
///   text *is* the active scheme (the Construct default's are the platform tokens);
/// - `Palette(n)` resolves through [`Palette::color`], with bold promoting the
///   low slots to their bright twins (the classic `SGR 1` behaviour);
/// - `Rgb` passes straight through — **true-colour already works** (design
///   lock 13/20: the engine carries `Rgb` cells end-to-end; TERM-13 only adds
///   the remaining fidelity extras);
/// - `bold` lifts, `dim` fades, `inverse` swaps, `hidden` conceals (fg = bg —
///   the glyph vanishes but stays real for selection/copy).
#[must_use]
pub fn cell_colors(cell: &Cell, palette: &Palette) -> (Color32, Color32) {
    let attrs = cell.attrs;
    let mut fg = match cell.fg {
        CellColor::Default => palette.fg,
        CellColor::Palette(slot) => palette.color(if attrs.bold && slot < 8 {
            slot + 8
        } else {
            slot
        }),
        CellColor::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
    };
    let mut bg = match cell.bg {
        CellColor::Default => palette.bg,
        CellColor::Palette(slot) => palette.color(slot),
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
        // The Construct-token derivations (the §4 carve-out's "derive where
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
    fn the_default_palette_is_the_token_derivation() {
        // TERM-11: the default `Palette` is not hand-picked hex — it is the
        // token table plus the platform role colours.
        let p = Palette::from_tokens();
        assert_eq!(p.ansi, ANSI16);
        assert_eq!(p.fg, Style::TEXT);
        assert_eq!(p.bg, Style::BG);
        assert_eq!(p.cursor, Style::TEXT);
        // `color` reads the scheme's slots for the low range …
        assert_eq!(p.color(1), Style::DANGER);
        assert_eq!(p.color(4), Style::ACCENT);
        // … and defers to the fixed xterm cube/greyscale above it.
        assert_eq!(p.color(196), indexed(196));
        assert_eq!(p.color(244), indexed(244));
        assert_eq!(Palette::default(), Palette::from_tokens());
    }

    #[test]
    fn a_scheme_repaints_the_low_slots_but_not_the_256_cube() {
        // A hand-built scheme with a distinctive slot-1 proves `color` reads the
        // scheme, and that the fixed cube/greyscale range is untouched by it.
        let mut ansi = ANSI16;
        ansi[1] = Color32::from_rgb(0x01, 0x02, 0x03);
        let scheme = Palette {
            ansi,
            fg: Color32::WHITE,
            bg: Color32::BLACK,
            cursor: Color32::WHITE,
        };
        assert_eq!(scheme.color(1), Color32::from_rgb(0x01, 0x02, 0x03));
        assert_ne!(scheme.color(1), indexed(1));
        // The 6×6×6 cube slot is identical across schemes.
        assert_eq!(scheme.color(196), indexed(196));
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
    fn default_cells_are_the_palette_role_colours() {
        let p = Palette::from_tokens();
        let (fg, bg) = cell_colors(
            &cell(CellColor::Default, CellColor::Default, CellAttrs::default()),
            &p,
        );
        // The Construct default's role colours are the chrome tokens.
        assert_eq!(fg, Style::TEXT);
        assert_eq!(bg, Style::BG);
    }

    #[test]
    fn a_preset_scheme_repaints_the_default_cell() {
        // Under a different scheme the reset fg/bg follow the palette (content
        // carve-out) — a program's plain text *is* the active theme.
        let scheme = Palette {
            ansi: ANSI16,
            fg: Color32::from_rgb(0x83, 0x94, 0x96),
            bg: Color32::from_rgb(0x00, 0x2b, 0x36),
            cursor: Color32::from_rgb(0x83, 0x94, 0x96),
        };
        let (fg, bg) = cell_colors(
            &cell(CellColor::Default, CellColor::Default, CellAttrs::default()),
            &scheme,
        );
        assert_eq!(fg, Color32::from_rgb(0x83, 0x94, 0x96));
        assert_eq!(bg, Color32::from_rgb(0x00, 0x2b, 0x36));
    }

    #[test]
    fn truecolor_passes_straight_through() {
        // Lock 13/20: the engine carries Rgb cells, so 24-bit colour already
        // renders — no palette quantisation on this path (or scheme influence).
        let (fg, bg) = cell_colors(
            &cell(
                CellColor::Rgb(10, 20, 30),
                CellColor::Rgb(200, 100, 50),
                CellAttrs::default(),
            ),
            &Palette::from_tokens(),
        );
        assert_eq!(fg, Color32::from_rgb(10, 20, 30));
        assert_eq!(bg, Color32::from_rgb(200, 100, 50));
    }

    #[test]
    fn bold_promotes_low_slots_and_visibly_lifts_everything() {
        let p = Palette::from_tokens();
        let bold = CellAttrs {
            bold: true,
            ..CellAttrs::default()
        };
        // SGR 1 + red → the bright-red slot (then lifted).
        let (fg, _) = cell_colors(&cell(CellColor::Palette(1), CellColor::Default, bold), &p);
        assert_eq!(fg, lift(BRIGHT_RED));
        // A high slot is not promoted, only lifted.
        let (fg, _) = cell_colors(&cell(CellColor::Palette(123), CellColor::Default, bold), &p);
        assert_eq!(fg, lift(indexed(123)));
        // Default-fg bold (the everyday `\e[1m`) must be visibly distinct.
        let (plain, _) = cell_colors(
            &cell(CellColor::Default, CellColor::Default, CellAttrs::default()),
            &p,
        );
        let (bolded, _) = cell_colors(&cell(CellColor::Default, CellColor::Default, bold), &p);
        assert_ne!(plain, bolded, "bold default text must not paint like plain");
    }

    #[test]
    fn inverse_swaps_and_hidden_conceals() {
        let p = Palette::from_tokens();
        let inverse = CellAttrs {
            inverse: true,
            ..CellAttrs::default()
        };
        let (fg, bg) = cell_colors(
            &cell(CellColor::Palette(1), CellColor::Palette(4), inverse),
            &p,
        );
        assert_eq!((fg, bg), (BLUE, RED));

        let hidden = CellAttrs {
            hidden: true,
            ..CellAttrs::default()
        };
        let (fg, bg) = cell_colors(
            &cell(CellColor::Palette(1), CellColor::Palette(4), hidden),
            &p,
        );
        assert_eq!(fg, bg, "a concealed glyph paints invisibly");
    }

    #[test]
    fn dim_fades_the_foreground() {
        let p = Palette::from_tokens();
        let dim = CellAttrs {
            dim: true,
            ..CellAttrs::default()
        };
        let (faint, _) = cell_colors(&cell(CellColor::Default, CellColor::Default, dim), &p);
        assert_ne!(faint, Style::TEXT, "dim text must not paint like plain");
    }
}
