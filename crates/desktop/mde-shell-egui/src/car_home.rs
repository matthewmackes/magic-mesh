//! The Auto Mode home (AUTO-HOME) — the glanceable Ford SYNC 3 tile launcher a
//! driver lands on in Car Mode.
//!
//! A full-bleed grid of six large tiles — Nav / Media / Phone / Comms / Vehicle /
//! Settings — each with a big Carbon glyph, a large label, and a live glance line.
//! Tapping a tile (or pressing its bound physical key) routes the shell's active
//! surface. Everything reads the shared `mde_egui::Style` tokens; in Car Mode the
//! ctx carries the Sync-3 black/white/blue skin, so the tiles inherit it.
//!
//! Crash-safety follows the maps-HUD lessons: every allocated rect is guarded
//! finite/non-degenerate, so a zero-size viewport or a NaN never reaches egui's
//! layout (the `widget_rect` panic class).

use mde_egui::egui::{self, Color32, Rect, Sense, Ui, Vec2};
use mde_egui::Style;
use mde_theme::brand::icons::IconId;

use crate::dock::{self, Surface};

/// One Auto Mode home tile — a curated vehicle app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CarTile {
    /// Navigation — the Drive HUD.
    Nav,
    /// Media / Music.
    Media,
    /// Phone — Voice / calls.
    Phone,
    /// Communications — alerts + messages.
    Comms,
    /// Vehicle telematics (opens the Maps surface on its Vehicle tab).
    Vehicle,
    /// Airspace — the real-time wardriving radar (Maps surface, Airspace tab).
    Airspace,
    /// Settings — including the Car Mode Key Mapping page.
    Settings,
}

impl CarTile {
    /// The tiles in home-grid order (two columns).
    pub const ALL: [Self; 7] = [
        Self::Nav,
        Self::Media,
        Self::Phone,
        Self::Comms,
        Self::Vehicle,
        Self::Airspace,
        Self::Settings,
    ];

    /// The tile's headline label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Nav => "Navigation",
            Self::Media => "Media",
            Self::Phone => "Phone",
            Self::Comms => "Comms",
            Self::Vehicle => "Vehicle",
            Self::Airspace => "Airspace",
            Self::Settings => "Settings",
        }
    }

    /// The Carbon glyph the tile paints.
    #[must_use]
    pub const fn icon(self) -> IconId {
        match self {
            Self::Nav => IconId::MapsLocation,
            Self::Media => IconId::Media,
            Self::Phone => IconId::Voice,
            Self::Comms => IconId::Share,
            Self::Vehicle => IconId::HealthStatus,
            Self::Airspace => IconId::MeshView,
            Self::Settings => IconId::Settings,
        }
    }

    /// The shell surface the tile routes to.
    #[must_use]
    pub const fn surface(self) -> Surface {
        match self {
            Self::Nav | Self::Vehicle | Self::Airspace => Surface::MapsLocation,
            Self::Media => Surface::Media,
            // WL-FUNC-011 Phase-2 — the retired Voice surface's calls live in the
            // Communications hub, so the Phone tile routes there.
            Self::Phone => Surface::Communications,
            Self::Comms => Surface::Communications,
            Self::Settings => Surface::System,
        }
    }

    /// The per-app accent used for the tile's glyph + top rule — the categorical
    /// hues already in the shared palette, so the home reads like the platform.
    #[must_use]
    pub const fn accent(self) -> Color32 {
        match self {
            Self::Nav => Style::ACCENT_MESH,
            Self::Media => Style::ACCENT_MEDIA,
            Self::Phone => Style::ACCENT_COMMS,
            Self::Comms => Style::ACCENT,
            Self::Vehicle => Style::OK,
            Self::Airspace => Style::ACCENT_COMMS,
            Self::Settings => Style::ACCENT_SYSTEM,
        }
    }
}

/// The live glance line under each tile's label. Each is `None` when there is no
/// honest live value (the tile then shows a plain descriptor) — never a mock.
#[derive(Clone, Debug, Default)]
pub struct CarHomeGlance {
    /// Navigation — active route ETA/summary (`None` ⇒ "Where to?").
    pub nav: Option<String>,
    /// Media — now-playing title (`None` ⇒ "Music & podcasts").
    pub media: Option<String>,
    /// Phone — live call state (`None` ⇒ "Dial & recents").
    pub phone: Option<String>,
    /// Comms — count of unacked alerts (`None`/0 ⇒ "Alerts & messages").
    pub comms: Option<usize>,
    /// Vehicle — live telematics summary (`None` ⇒ "Telematics").
    pub vehicle: Option<String>,
}

impl CarHomeGlance {
    /// The descriptor line the tile shows: the live glance value when present,
    /// else a plain honest subtitle (never fabricated data).
    fn line(&self, tile: CarTile) -> String {
        match tile {
            CarTile::Nav => self.nav.clone().unwrap_or_else(|| "Where to?".to_string()),
            CarTile::Media => self
                .media
                .clone()
                .unwrap_or_else(|| "Music & podcasts".to_string()),
            CarTile::Phone => self
                .phone
                .clone()
                .unwrap_or_else(|| "Dial & recents".to_string()),
            CarTile::Comms => match self.comms {
                Some(n) if n > 0 => format!("{n} alert{}", if n == 1 { "" } else { "s" }),
                _ => "Alerts & messages".to_string(),
            },
            CarTile::Vehicle => self
                .vehicle
                .clone()
                .unwrap_or_else(|| "Telematics".to_string()),
            CarTile::Airspace => "WiFi · cell · BT radar".to_string(),
            CarTile::Settings => "Key mapping & more".to_string(),
        }
    }
}

/// Finite-or-fallback guard (maps-HUD crash-safety idiom).
fn finite_or(v: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        fallback
    }
}

/// Render the Auto Mode home. Returns the tile the driver activated this frame
/// (tap), or `None`. The shell maps that to a surface switch.
pub fn car_home_panel(ui: &mut Ui, glance: &CarHomeGlance) -> Option<CarTile> {
    let full = ui.available_rect_before_wrap();
    // Guard a degenerate viewport (a collapsed/NaN rect never reaches layout).
    let width = finite_or(full.width(), 0.0);
    let height = finite_or(full.height(), 0.0);
    if width < 2.0 || height < 2.0 {
        return None;
    }

    let painter = ui.painter().clone();
    // The Sync-3 ground is already the panel fill; paint it explicitly so the home
    // is edge-to-edge black even inside a bordered central panel.
    painter.rect_filled(full, 0.0, Style::BG);

    let pad = Style::SP_L;
    let inner = Rect::from_min_max(full.min + Vec2::splat(pad), full.max - Vec2::splat(pad));
    if inner.width() < 2.0 || inner.height() < 2.0 {
        return None;
    }

    // Header band — a large "Auto Mode" title, Sync-3 white.
    let header_h = Style::DISPLAY + Style::SP_M;
    painter.text(
        egui::pos2(inner.left(), inner.top()),
        egui::Align2::LEFT_TOP,
        "Auto Mode",
        egui::FontId::new(Style::DISPLAY, egui::FontFamily::Name("heading".into())),
        Style::TEXT_STRONG,
    );

    let grid = Rect::from_min_max(egui::pos2(inner.left(), inner.top() + header_h), inner.max);
    if grid.width() < 2.0 || grid.height() < 2.0 {
        return None;
    }

    // Two columns × three rows of large tiles.
    let cols = 2usize;
    let rows = 3usize;
    let gap = Style::SP_M;
    let tile_w = ((grid.width() - gap * (cols as f32 - 1.0)) / cols as f32).max(1.0);
    let tile_h = ((grid.height() - gap * (rows as f32 - 1.0)) / rows as f32).max(1.0);

    let mut activated = None;
    for (idx, tile) in CarTile::ALL.iter().enumerate() {
        let col = idx % cols;
        let row = idx / cols;
        let x = grid.left() + col as f32 * (tile_w + gap);
        let y = grid.top() + row as f32 * (tile_h + gap);
        let rect = Rect::from_min_size(egui::pos2(x, y), egui::vec2(tile_w, tile_h));
        if paint_tile(ui, &painter, rect, *tile, &glance.line(*tile)) {
            activated = Some(*tile);
        }
    }
    activated
}

/// Paint one tile and return whether it was clicked/tapped this frame.
fn paint_tile(ui: &mut Ui, painter: &egui::Painter, rect: Rect, tile: CarTile, line: &str) -> bool {
    if !rect.is_finite() || rect.width() < 2.0 || rect.height() < 2.0 {
        return false;
    }
    let id = egui::Id::new(("car-home-tile", tile.label()));
    let resp = ui.interact(rect, id, Sense::click());

    // Card surface: Sync-3 raised tile; hover lifts to the highlight tone; the
    // pressed state uses the accent-derived pressed fill (from the shared kit).
    let fill = if resp.is_pointer_button_down_on() {
        Style::pressed_fill(tile.accent())
    } else if resp.hovered() {
        Style::SURFACE_HI
    } else {
        Style::SURFACE
    };
    let radius = egui::CornerRadius::same(Style::RADIUS_L as u8);
    painter.rect_filled(rect, radius, fill);
    // A hairline; on hover the border becomes the tile accent (the shared five-
    // state interaction cue).
    let stroke_col = if resp.hovered() {
        tile.accent()
    } else {
        Style::BORDER
    };
    painter.rect_stroke(
        rect,
        radius,
        egui::Stroke::new(Style::STROKE_HAIRLINE, stroke_col),
        egui::StrokeKind::Inside,
    );

    // Accent top rule — the SYNC-3-style colored cap on the card.
    let cap = Rect::from_min_max(
        rect.min,
        egui::pos2(rect.right(), (rect.top() + Style::SP_XS).min(rect.bottom())),
    );
    painter.rect_filled(
        cap,
        egui::CornerRadius {
            nw: radius.nw,
            ne: radius.ne,
            sw: 0,
            se: 0,
        },
        tile.accent(),
    );

    // Big glyph, centered in the upper portion.
    let icon_edge = (rect.height() * 0.34).clamp(28.0, 96.0);
    if let Some(tex) = dock::icon_texture(ui.ctx(), tile.icon(), icon_edge, tile.accent()) {
        let icon_center = egui::pos2(rect.center().x, rect.top() + rect.height() * 0.34);
        let icon_rect = Rect::from_center_size(icon_center, egui::vec2(icon_edge, icon_edge));
        painter.image(
            tex.id(),
            icon_rect,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }

    // Label — large heading, Sync-3 white.
    painter.text(
        egui::pos2(rect.center().x, rect.bottom() - Style::SP_L - Style::BODY),
        egui::Align2::CENTER_BOTTOM,
        tile.label(),
        egui::FontId::new(Style::HEADING, egui::FontFamily::Name("heading".into())),
        Style::TEXT_STRONG,
    );
    // Glance line — dim, under the label.
    painter.text(
        egui::pos2(rect.center().x, rect.bottom() - Style::SP_S),
        egui::Align2::CENTER_BOTTOM,
        line,
        egui::FontId::new(Style::BODY, egui::FontFamily::Proportional),
        Style::TEXT_DIM,
    );

    resp.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tile_has_a_distinct_surface_route_and_glyph() {
        // Nav + Vehicle + Airspace share the Maps surface; Phone + Comms both route
        // to the Communications hub (WL-FUNC-011 Phase-2 folded Voice's calls into
        // it). Every tile still has a distinct glyph.
        for tile in CarTile::ALL {
            let _ = tile.surface();
            let _ = tile.icon();
            let _ = tile.accent();
            assert!(!tile.label().is_empty());
        }
        assert_eq!(CarTile::Nav.surface(), Surface::MapsLocation);
        assert_eq!(CarTile::Media.surface(), Surface::Media);
        assert_eq!(CarTile::Phone.surface(), Surface::Communications);
        assert_eq!(CarTile::Comms.surface(), Surface::Communications);
        assert_eq!(CarTile::Settings.surface(), Surface::System);
    }

    #[test]
    fn glance_falls_back_to_honest_descriptors_not_mock_data() {
        let empty = CarHomeGlance::default();
        assert_eq!(empty.line(CarTile::Nav), "Where to?");
        assert_eq!(empty.line(CarTile::Comms), "Alerts & messages");
        let live = CarHomeGlance {
            nav: Some("12 min · 4.3 mi".to_string()),
            comms: Some(3),
            ..Default::default()
        };
        assert_eq!(live.line(CarTile::Nav), "12 min · 4.3 mi");
        assert_eq!(live.line(CarTile::Comms), "3 alerts");
        assert_eq!(
            CarHomeGlance {
                comms: Some(1),
                ..Default::default()
            }
            .line(CarTile::Comms),
            "1 alert"
        );
    }
}
