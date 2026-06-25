//! FRONTDOOR-1/2 — the "Front Door" home: a Win10-Start two-pane shell wrapping
//! a GPU `canvas` tile grid.
//!
//! FRONTDOOR-1 (the de-risk track, `docs/design/front-door.md`) replaced the old
//! deep-widget-tree home (the "4-second menu") with a tile grid drawn as flat GPU
//! 2D geometry on `cosmic::iced::widget::canvas` — the same lighter render path
//! Routing's path-graph and the Peers map use — so it paints immediately, with
//! skeleton placeholders while real data streams in.
//!
//! FRONTDOOR-2 (this layer) wraps that grid in the locked **Win10 Start** shell
//! (design Q1/Q5/Q98): a fixed left **rail** (identity · pinned · the predominant
//! DevOps + Data Center entries) and a right **pane** (a full-width omnibox above
//! the FRONTDOOR-1 tile grid). The rail's DevOps / Data Center entries navigate to
//! the real `build-farm` / `datacenter` panel routes (§7 — no dead buttons); the
//! omnibox renders + tracks its text locally but does NOT search yet (that's
//! FRONTDOOR-6). Carbon chrome: follow-OS theme, Blue 60 accent, comfortable
//! density — all via `mde-theme` tokens, never raw hex (§4).
//!
//! SCOPE held to FRONTDOOR-1/2 only:
//! - Static placeholder tiles (REAL bus-backed data is FRONTDOOR-4).
//! - `draw` only — tile click → detail view is FRONTDOOR-5, so the canvas keeps
//!   `type State = ()` and the default `update` / `mouse_interaction`.
//! - Omnibox is render + local text state only (search logic is FRONTDOOR-6).
//! - No full-screen mode (FRONTDOOR-3) and no wallpaper backdrop here.

use cosmic::iced::widget::canvas::{self, Frame, Path, Text};
use cosmic::iced::widget::text::Alignment;
use cosmic::iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use cosmic::iced::{Background, Border, Element, Length, Padding, Pixels, Point, Size};
use cosmic::Theme;
use mde_theme::{FontSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;
use crate::model::Group;

/// FRONTDOOR-2 — the Front Door's own message set, threaded through
/// [`crate::Message::FrontDoor`]. Only the variant we actually handle exists
/// (§7): the omnibox text-change. Rail navigation reuses the app-level
/// [`crate::Message::SelectPanel`] directly (it drives the real router), so it
/// needs no variant here.
#[derive(Debug, Clone)]
pub enum Message {
    /// The omnibox text changed. FRONTDOOR-2 only records it into local state;
    /// the search behavior it will drive is FRONTDOOR-6.
    OmniboxChanged(String),
}

/// The fixed width of the left rail (design Q5 — a Win10-Start identity/pinned/
/// surfaces column). Comfortable-density Start rails sit around this width.
const RAIL_WIDTH: f32 = 260.0;

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

/// The Front Door home state: the placeholder tile set + a loading flag, plus the
/// FRONTDOOR-2 omnibox query. While `loading`, the grid draws flat grey skeleton
/// cards instead of labeled tiles (Q92 — skeleton placeholders, no layout shift).
#[derive(Debug, Clone)]
pub struct FrontDoor {
    /// The tiles to draw (static placeholders for FRONTDOOR-1).
    pub tiles: Vec<Tile>,
    /// True → render skeletons; false → render labeled tiles.
    pub loading: bool,
    /// FRONTDOOR-2 — the omnibox's live text. Tracked here so the field is
    /// controlled; the search it will drive is FRONTDOOR-6 (no behavior yet).
    pub query: String,
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
            query: String::new(),
        }
    }

    /// FRONTDOOR-2 — fold a Front Door message into local state. Only the omnibox
    /// text-change lands here; it has no side effects (search is FRONTDOOR-6), so
    /// the caller (`app.rs`) needs no follow-up `Task`.
    pub fn update(&mut self, message: Message) {
        match message {
            Message::OmniboxChanged(q) => self.query = q,
        }
    }

    /// FRONTDOOR-2 — the Win10-Start two-pane view: a fixed left **rail** beside a
    /// right **pane** (the full-width omnibox above the FRONTDOOR-1 tile grid).
    #[must_use]
    pub fn view(&self) -> Element<'_, crate::Message, Theme> {
        let palette = crate::live_theme::palette();
        row![self.rail(palette), self.right_pane(palette)]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// The left rail (design Q5): identity → Pinned → the predominant DevOps +
    /// Data Center surfaces. Fixed width, scrollable so a short window still
    /// reaches every entry. No power control: the Front Door has no existing
    /// local power/session action to call, so §7 says omit it (better an absent
    /// control than a dead button) — the mesh-power tile is FRONTDOOR-4 data.
    fn rail(&self, palette: Palette) -> Element<'_, crate::Message, Theme> {
        let sizes = FontSize::defaults();

        // Identity — the account this Front Door belongs to. A static label for
        // now (live identity is FRONTDOOR-4); rendered, not interactive.
        let account = whoami_label();
        let identity = column![
            text(account)
                .size(TypeRole::Heading.size_in(sizes))
                .colr(palette.text.into_cosmic_color()),
            text("This node")
                .size(TypeRole::Caption.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color()),
        ]
        .spacing(2);

        // Pinned — the launchers that have a real route today. Each entry
        // navigates somewhere real (§7); we don't list a pin we can't open yet.
        let pinned = column![
            rail_section_label("Pinned", palette),
            rail_link(
                "Peers",
                crate::Message::SelectPanel {
                    group: Group::Mesh,
                    panel: "peers",
                },
                palette,
                false,
            ),
            rail_link(
                "Mesh Bus",
                crate::Message::SelectPanel {
                    group: Group::Mesh,
                    panel: "mesh_bus",
                },
                palette,
                false,
            ),
        ]
        .spacing(4);

        // The predominant surfaces (the brief: DevOps + Data Center front-and-
        // center). Rendered as accent-emphasized rail links that navigate to the
        // real `build-farm` / `datacenter` panel routes (§7).
        let surfaces = column![
            rail_section_label("Surfaces", palette),
            rail_link(
                "DevOps",
                crate::Message::SelectPanel {
                    group: Group::Provisioning,
                    panel: "build-farm",
                },
                palette,
                true,
            ),
            rail_link(
                "Data Center",
                crate::Message::SelectPanel {
                    group: Group::Provisioning,
                    panel: "datacenter",
                },
                palette,
                true,
            ),
        ]
        .spacing(4);

        let body = column![
            identity,
            Space::new().height(Length::Fixed(16.0)),
            surfaces,
            Space::new().height(Length::Fixed(16.0)),
            pinned,
        ]
        .spacing(8)
        .width(Length::Fill);

        let scroller = scrollable(container(body).padding(Padding::from([20u16, 16u16])))
            .width(Length::Fill)
            .height(Length::Fill);

        container(scroller)
            .width(Length::Fixed(RAIL_WIDTH))
            .height(Length::Fill)
            .style(move |_t: &Theme| container::Style {
                background: Some(Background::Color(palette.surface.into_cosmic_color())),
                border: Border {
                    color: palette.border.into_cosmic_color(),
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..container::Style::default()
            })
            .into()
    }

    /// The right pane: the full-width omnibox (FRONTDOOR-2 render + local text;
    /// search is FRONTDOOR-6) above the FRONTDOOR-1 canvas tile grid.
    fn right_pane(&self, palette: Palette) -> Element<'_, crate::Message, Theme> {
        let omnibox: Element<'_, crate::Message, Theme> =
            text_input("Search apps, files, mesh, or ask Copilot…", &self.query)
                .on_input(|s| crate::Message::FrontDoor(Message::OmniboxChanged(s)))
                .padding(Padding::from([10u16, 14u16]))
                .width(Length::Fill)
                .into();

        let omnibox_bar = container(omnibox)
            .width(Length::Fill)
            .padding(Padding::from([16u16, 16u16]));

        let pane = column![omnibox_bar, self.tile_grid()]
            .width(Length::Fill)
            .height(Length::Fill);

        container(pane)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_t: &Theme| container::Style {
                background: Some(Background::Color(palette.background.into_cosmic_color())),
                ..container::Style::default()
            })
            .into()
    }

    /// The FRONTDOOR-1 tile grid drawn on `canvas` (GPU 2D geometry, NOT a widget
    /// tree). The program paints from the live palette (it ignores the stock theme
    /// passed to `draw`), so `themer(None, ..)` bridges the stock-themed canvas
    /// back into the surrounding cosmic theme — same pattern as Routing's path
    /// graph and the Peers map. Unchanged by FRONTDOOR-2; it's the right-pane
    /// tile area.
    fn tile_grid(&self) -> Element<'_, crate::Message, Theme> {
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

/// The rail's account identity. Best-effort from the environment (`$USER`),
/// falling back to a neutral label — no probe in `view()`. Live identity is
/// FRONTDOOR-4.
fn whoami_label() -> String {
    std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "Account".to_string())
}

/// A rail section header (Pinned / Surfaces), muted + caption-sized.
fn rail_section_label<'a>(label: &'a str, palette: Palette) -> Element<'a, crate::Message, Theme> {
    text(label)
        .size(TypeRole::Caption.size_in(FontSize::defaults()))
        .colr(palette.text_muted.into_cosmic_color())
        .into()
}

/// A full-width rail link. `emphasized` marks the predominant DevOps / Data
/// Center surfaces (design Q5): an accent-tinted fill + accent text so they read
/// front-and-center; ordinary pins read as quiet ghost rows. Every link carries a
/// REAL `on_press` route (§7 — no dead buttons).
fn rail_link<'a>(
    label: &'a str,
    msg: crate::Message,
    palette: Palette,
    emphasized: bool,
) -> Element<'a, crate::Message, Theme> {
    let accent = palette.accent.into_cosmic_color();
    let fg = if emphasized {
        accent
    } else {
        palette.text.into_cosmic_color()
    };
    let raised = palette.raised.into_cosmic_color();
    let hover_tint = palette.hover_tint().into_cosmic_color();
    let idle_bg = if emphasized {
        hover_tint
    } else {
        cosmic::iced::Color::TRANSPARENT
    };

    button(
        text(label)
            .size(TypeRole::Body.size_in(FontSize::defaults()))
            .colr(fg),
    )
    .width(Length::Fill)
    .padding(Padding::from([8u16, 12u16]))
    .sty(
        move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
            use cosmic::iced::widget::button::Status;
            let bg = match status {
                Status::Hovered | Status::Pressed => {
                    if emphasized {
                        accent_tint(accent)
                    } else {
                        raised
                    }
                }
                _ => idle_bg,
            };
            cosmic::iced::widget::button::Style {
                snap: false,
                background: Some(Background::Color(bg)),
                text_color: fg,
                border: Border {
                    color: cosmic::iced::Color::TRANSPARENT,
                    width: 0.0,
                    radius: 6.0.into(),
                },
                shadow: cosmic::iced::Shadow::default(),
                ..cosmic::iced::widget::button::Style::default()
            }
        },
    )
    .on_press(msg)
    .into()
}

/// A stronger accent wash for an emphasized rail link's hover/press — the accent
/// at low alpha, so the row lifts without flipping to a full accent fill.
fn accent_tint(accent: cosmic::iced::Color) -> cosmic::iced::Color {
    cosmic::iced::Color { a: 0.28, ..accent }
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
        // widget set + a few launchers; not loading by default. FRONTDOOR-2 — the
        // omnibox query starts empty.
        let fd = FrontDoor::new();
        assert_eq!(fd.tiles.len(), 12);
        assert!(!fd.loading);
        assert!(fd.query.is_empty());
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
    fn omnibox_change_records_the_query_locally() {
        // FRONTDOOR-2 — the omnibox is a controlled field: a text-change updates
        // local state (so the field shows the typed text), with no other effect
        // (search is FRONTDOOR-6).
        let mut fd = FrontDoor::new();
        fd.update(Message::OmniboxChanged("build farm".to_string()));
        assert_eq!(fd.query, "build farm");
        fd.update(Message::OmniboxChanged(String::new()));
        assert!(fd.query.is_empty());
    }

    #[test]
    fn two_pane_view_constructs() {
        // FRONTDOOR-2 — the two-pane view (rail + omnibox + canvas grid) builds
        // without panicking, in both the loading and loaded states.
        let mut fd = FrontDoor::new();
        let _: Element<'_, crate::Message, Theme> = fd.view();
        fd.loading = true;
        let _: Element<'_, crate::Message, Theme> = fd.view();
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
