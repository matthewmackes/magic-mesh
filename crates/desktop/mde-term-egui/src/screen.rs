//! The render-agnostic terminal screen model.
//!
//! [`Screen`] is a flat, immutable snapshot of a terminal grid — a rectangle of
//! [`Cell`]s plus the cursor position — with **no** engine or toolkit types in
//! its surface. The VT engine ([`crate::engine`]) produces one on demand; the
//! egui pane widget (TERM-3) and the scrollback search (TERM-9) consume it. It
//! is deliberately dumb: all VT semantics live in the engine, all rendering
//! lives in the surface, and this is the wire between them.

/// A single colour a cell can carry, normalised out of the engine's palette.
///
/// The engine speaks in named ANSI slots, 256-colour indices, and 24-bit specs;
/// this collapses those into the three cases a renderer actually resolves. The
/// mapping from the engine colour lives in [`crate::engine`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CellColor {
    /// The terminal's default foreground or background — the renderer picks the
    /// concrete Carbon/preset colour by the cell's role (fg vs bg field).
    #[default]
    Default,
    /// A palette slot `0..=255`: the 16 ANSI names live at `0..16`, the
    /// 6×6×6 colour cube and the greyscale ramp fill `16..256`.
    Palette(u8),
    /// A 24-bit true-colour value (`SGR 38;2;r;g;b` / `48;2;r;g;b`).
    Rgb(u8, u8, u8),
}

/// The rendering attributes a cell can carry, decoded from the engine flags.
///
/// One `bool` per attribute mirrors the engine's own cell flag bitfield and is
/// the natural shape for a renderer to consume; a state machine would only
/// obscure it.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CellAttrs {
    /// `SGR 1` — bold / bright weight.
    pub bold: bool,
    /// `SGR 2` — faint / dim.
    pub dim: bool,
    /// `SGR 3` — italic.
    pub italic: bool,
    /// `SGR 4` — underline (any of the underline family).
    pub underline: bool,
    /// `SGR 7` — reverse video (fg/bg swapped by the renderer).
    pub inverse: bool,
    /// `SGR 9` — crossed-out.
    pub strikeout: bool,
    /// `SGR 8` — concealed (rendered as blank, text preserved for copy).
    pub hidden: bool,
}

/// One grid cell: a character plus its colours and attributes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Cell {
    /// The displayed character (`' '` for an empty cell).
    pub ch: char,
    /// Foreground colour.
    pub fg: CellColor,
    /// Background colour.
    pub bg: CellColor,
    /// Rendering attributes.
    pub attrs: CellAttrs,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: CellColor::Default,
            bg: CellColor::Default,
            attrs: CellAttrs::default(),
        }
    }
}

/// The cursor position within a [`Screen`], in `0`-based `row`/`col`.
///
/// `row` is measured from the **top of the snapshot** — so in a viewport-only
/// snapshot it is the visible row, and in a full snapshot (with scrollback) it
/// includes the history rows above the viewport.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CursorPos {
    /// Row from the top of the snapshot.
    pub row: usize,
    /// Column from the left edge.
    pub col: usize,
}

/// An immutable rectangular snapshot of the terminal grid.
///
/// Cells are stored row-major (`rows * cols`). A snapshot may be viewport-only
/// or include the scrollback history above the viewport; see
/// [`crate::engine::Terminal::viewport`] and
/// [`crate::engine::Terminal::full`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Screen {
    cols: usize,
    rows: usize,
    cells: Vec<Cell>,
    cursor: CursorPos,
}

impl Screen {
    /// Build a screen from a row-major cell buffer.
    ///
    /// The caller guarantees `cells.len() == rows * cols`; any shortfall is
    /// padded and any excess truncated so accessors stay in bounds.
    #[must_use]
    pub fn new(cols: usize, rows: usize, mut cells: Vec<Cell>, cursor: CursorPos) -> Self {
        cells.resize(rows.saturating_mul(cols), Cell::default());
        Self {
            cols,
            rows,
            cells,
            cursor,
        }
    }

    /// Width in columns.
    #[must_use]
    pub const fn cols(&self) -> usize {
        self.cols
    }

    /// Height in rows (visible rows, plus scrollback rows in a full snapshot).
    #[must_use]
    pub const fn rows(&self) -> usize {
        self.rows
    }

    /// The cursor position.
    #[must_use]
    pub const fn cursor(&self) -> CursorPos {
        self.cursor
    }

    /// The cell at `(row, col)`, or `None` if out of range.
    #[must_use]
    pub fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        self.cells
            .get(row.saturating_mul(self.cols).saturating_add(col))
    }

    /// A whole row as a cell slice, or `None` if `row` is out of range.
    #[must_use]
    pub fn row(&self, row: usize) -> Option<&[Cell]> {
        if row >= self.rows {
            return None;
        }
        let start = row.saturating_mul(self.cols);
        self.cells.get(start..start.saturating_add(self.cols))
    }

    /// The text of a row with trailing blanks trimmed — a convenience for
    /// scrollback search (TERM-9) and tests.
    #[must_use]
    pub fn line_text(&self, row: usize) -> String {
        let Some(cells) = self.row(row) else {
            return String::new();
        };
        let text: String = cells.iter().map(|c| c.ch).collect();
        text.trim_end().to_owned()
    }
}
