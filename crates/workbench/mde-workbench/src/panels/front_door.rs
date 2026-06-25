//! FRONTDOOR-1 — the GPU `canvas` tile-grid "Front Door" home.
//!
//! The de-risk track of the Front Door redesign (`docs/design/front-door.md`):
//! the old `mde-workbench` home/dashboard view built a deep iced widget tree,
//! which is what made it slow (the "4-second menu"). This view instead draws the
//! tile grid as flat GPU 2D geometry on `cosmic::iced::widget::canvas` — the same
//! lighter render path Routing's path-graph and the Peers map already use — so it
//! paints immediately, with skeleton placeholders while real data streams in.
//!
//! SCOPE (renderer de-risk only):
//! - Static placeholder tiles (REAL bus-backed data is FRONTDOOR-4).
//! - `draw` only — click interaction (hit-testing → detail view) is FRONTDOOR-5,
//!   so we keep `type State = ()` and rely on the canvas default `update` /
//!   `mouse_interaction`.
//!
//! Carbon tokens only (§4): every color comes from `live_theme::palette()` via
//! `into_cosmic_color()`; no raw hex here.

use cosmic::iced::widget::canvas::{self, Frame, Path, Text};
use cosmic::iced::widget::text::Alignment;
use cosmic::iced::{Element, Length, Pixels, Point, Size};
use cosmic::Theme;
use mde_theme::Palette;

use crate::cosmic_compat::prelude::*;

/// The Carbon token a tile's accent strip + label color reads from. Picked per
/// tile kind so DevOps/Data-Center/alert tiles read distinctly against the
/// background without any raw color. FRONTDOOR-4 will swap these for live status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileTone {
    /// The single interactive accent — mesh / Copilot / launchers.
    Accent,
    /// Healthy / system-nominal.
    Success,
    /// Pending / at-risk (build farm, node health caution).
    Warning,
    /// Errors / alerts.
    Danger,
    /// Neutral informational (system, generic app launchers).
    Neutral,
}

impl TileTone {
    /// Resolve this tone to its live Carbon color (§4 — token, never hex).
    fn color(self, p: &Palette) -> cosmic::iced::Color {
        match self {
            TileTone::Accent => p.accent.into_cosmic_color(),
            TileTone::Success => p.success.into_cosmic_color(),
            TileTone::Warning => p.warning.into_cosmic_color(),
            TileTone::Danger => p.danger.into_cosmic_color(),
            TileTone::Neutral => p.text_muted.into_cosmic_color(),
        }
    }
}

/// One placeholder tile in the grid. FRONTDOOR-4 backs `label`/`tone` with live
/// mde-bus topic data; here they are static seeds proving the render path.
#[derive(Debug, Clone)]
pub struct Tile {
    /// The card's short label, drawn centered on the tile.
    pub label: String,
    /// Which Carbon token the accent strip + label read.
    pub tone: TileTone,
}

impl Tile {
    fn new(label: &str, tone: TileTone) -> Self {
        Self {
            label: label.to_string(),
            tone,
        }
    }
}

/// The Front Door home state: the placeholder tile set + a loading flag. While
/// `loading`, the grid draws flat grey skeleton cards instead of labeled tiles
/// (Q92 — skeleton placeholders, no layout shift).
#[derive(Debug, Clone)]
pub struct FrontDoor {
    /// The tiles to draw (static placeholders for FRONTDOOR-1).
    pub tiles: Vec<Tile>,
    /// True → render skeletons; false → render labeled tiles.
    pub loading: bool,
}

impl Default for FrontDoor {
    fn default() -> Self {
        Self::new()
    }
}

impl FrontDoor {
    /// Seed the home grid with the design's widget set (Q99: mesh map, build/
    /// farm, alerts, node health, Copilot, system) plus a few app launchers.
    /// Real data is FRONTDOOR-4; these labels/tones are correct placeholders.
    #[must_use]
    pub fn new() -> Self {
        let tiles = vec![
            Tile::new("Mesh Map", TileTone::Accent),
            Tile::new("Build / Farm", TileTone::Warning),
            Tile::new("Alerts", TileTone::Danger),
            Tile::new("Node Health", TileTone::Success),
            Tile::new("Copilot", TileTone::Accent),
            Tile::new("System", TileTone::Neutral),
            Tile::new("Data Center", TileTone::Accent),
            Tile::new("DevOps", TileTone::Warning),
            Tile::new("Files", TileTone::Neutral),
            Tile::new("Terminal", TileTone::Neutral),
            Tile::new("Settings", TileTone::Neutral),
            Tile::new("Music", TileTone::Neutral),
        ];
        Self {
            tiles,
            loading: false,
        }
    }

    /// The Front Door view — the tile grid drawn on `canvas` (GPU 2D geometry,
    /// NOT a widget tree). The program paints from the live palette (it ignores
    /// the stock theme passed to `draw`), so `themer(None, ..)` bridges the
    /// stock-themed canvas back into the surrounding cosmic theme — same pattern
    /// as Routing's path graph and the Peers map.
    #[must_use]
    pub fn view(&self) -> Element<'_, crate::Message, Theme> {
        let program = TileGrid {
            tiles: self.tiles.clone(),
            loading: self.loading,
            palette: crate::live_theme::palette(),
        };
        let canvas_stock: cosmic::iced::Element<'_, crate::Message, cosmic::iced::Theme> =
            cosmic::iced::widget::canvas(program)
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        cosmic::iced::widget::themer(None, canvas_stock).into()
    }
}

/// FRONTDOOR-1 layout constants — a comfortable-density snap grid (Q80). Columns
/// are computed from `bounds.width`; the tile width/gap give ~180 px cards.
const TILE_W: f32 = 180.0;
const TILE_H: f32 = 96.0;
const GAP: f32 = 12.0;
const PAD: f32 = 16.0;
const RADIUS: f32 = 8.0;
/// The accent strip down the left edge of each card.
const STRIP_W: f32 = 5.0;

/// The canvas program that draws the tile grid. Holds an owned snapshot of the
/// tiles + the live palette so `draw` is pure geometry (no global reads mid-paint).
#[derive(Debug)]
pub struct TileGrid {
    tiles: Vec<Tile>,
    loading: bool,
    palette: Palette,
}

impl TileGrid {
    /// Columns that fit in `width` at the comfortable tile size, clamped to at
    /// least one so a narrow panel still renders a single column.
    fn columns(width: f32) -> usize {
        let usable = (width - 2.0 * PAD + GAP).max(TILE_W);
        ((usable / (TILE_W + GAP)).floor() as usize).max(1)
    }

    /// The top-left corner of tile `i` for a grid of `cols` columns.
    fn tile_origin(i: usize, cols: usize) -> Point {
        let col = i % cols;
        let row = i / cols;
        Point::new(
            PAD + col as f32 * (TILE_W + GAP),
            PAD + row as f32 * (TILE_H + GAP),
        )
    }
}

impl canvas::Program<crate::Message> for TileGrid {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &cosmic::iced::Renderer,
        _theme: &cosmic::iced::Theme,
        bounds: cosmic::iced::Rectangle,
        _cursor: cosmic::iced::mouse::Cursor,
    ) -> Vec<cosmic::iced::widget::canvas::Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let p = &self.palette;

        // Carbon page background under the cards.
        frame.fill(
            &Path::rectangle(Point::ORIGIN, bounds.size()),
            p.background.into_cosmic_color(),
        );

        let cols = Self::columns(bounds.width);
        let card_size = Size::new(TILE_W, TILE_H);
        let surface = p.surface.into_cosmic_color();
        // Skeleton fill: the raised surface token, a touch above `surface`, so a
        // loading card reads as a flat grey placeholder (no label, no strip).
        let skeleton = p.raised.into_cosmic_color();

        for (i, tile) in self.tiles.iter().enumerate() {
            let origin = Self::tile_origin(i, cols);

            if self.loading {
                // Flat grey skeleton rounded-rect — Q92, no layout shift.
                frame.fill(
                    &Path::rounded_rectangle(origin, card_size, RADIUS.into()),
                    skeleton,
                );
                continue;
            }

            // The card surface.
            frame.fill(
                &Path::rounded_rectangle(origin, card_size, RADIUS.into()),
                surface,
            );

            // The tone-colored accent strip down the card's left edge.
            let strip_origin = Point::new(origin.x, origin.y);
            frame.fill(
                &Path::rounded_rectangle(strip_origin, Size::new(STRIP_W, TILE_H), RADIUS.into()),
                tile.tone.color(p),
            );

            // The centered label.
            frame.fill_text(Text {
                content: tile.label.clone(),
                position: Point::new(origin.x + TILE_W / 2.0, origin.y + TILE_H / 2.0 - 7.0),
                color: p.text.into_cosmic_color(),
                size: Pixels(14.0),
                align_x: Alignment::Center,
                ..Text::default()
            });
        }

        vec![frame.into_geometry()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_seeds_the_design_widget_set() {
        // FRONTDOOR-1 — the home seeds ~12 placeholder tiles covering the Q99
        // widget set + a few launchers; not loading by default.
        let fd = FrontDoor::new();
        assert_eq!(fd.tiles.len(), 12);
        assert!(!fd.loading);
        // The design's named widgets are present.
        for want in ["Mesh Map", "Build / Farm", "Alerts", "Copilot", "System"] {
            assert!(
                fd.tiles.iter().any(|t| t.label == want),
                "missing widget tile: {want}"
            );
        }
    }

    #[test]
    fn columns_fit_the_width_and_never_zero() {
        // A wide panel packs several columns; a sliver still renders one.
        assert!(TileGrid::columns(1200.0) >= 5);
        assert_eq!(TileGrid::columns(10.0), 1);
        assert_eq!(TileGrid::columns(0.0), 1);
    }

    #[test]
    fn tile_origins_advance_by_row_and_column() {
        // Tile 0 sits at the pad; the next column steps right by tile+gap; the
        // first tile of the second row steps down by tile+gap.
        let o0 = TileGrid::tile_origin(0, 3);
        assert!((o0.x - PAD).abs() < f32::EPSILON);
        assert!((o0.y - PAD).abs() < f32::EPSILON);
        let o1 = TileGrid::tile_origin(1, 3);
        assert!((o1.x - (PAD + TILE_W + GAP)).abs() < f32::EPSILON);
        assert!((o1.y - PAD).abs() < f32::EPSILON);
        let o3 = TileGrid::tile_origin(3, 3);
        assert!((o3.x - PAD).abs() < f32::EPSILON);
        assert!((o3.y - (PAD + TILE_H + GAP)).abs() < f32::EPSILON);
    }
}
