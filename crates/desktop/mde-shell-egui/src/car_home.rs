//! The Auto Mode home (AUTO-HOME) — the CarPlay-Dashboard-style Car Mode home.
//!
//! PLATFORM-INTERFACES Q31/Q32: the home is **persistent split cards** — the Nav
//! card (largest), the Media / now-playing card, and the glance card (vehicle
//! telematics + comms alerts) — over a compact single-row **six-app strip**:
//! Nav / Media / Music / Comms / Vehicle / Settings. The Airspace TILE is gone
//! (the radar stays a Maps tab + keeps its keymap actions); the Phone tile's
//! calls live in the Communications hub. Everything paints on the kept SYNC3
//! dark + Ford-blue palette (Q30); glance values are honest — absent data reads
//! as a plain descriptor, never a fabricated number (Q35/P8).
//!
//! Crash-safety follows the maps-HUD lessons: every allocated rect is guarded
//! finite/non-degenerate, so a zero-size viewport or a NaN never reaches egui's
//! layout (the `widget_rect` panic class).

use mde_egui::egui::{self, Color32, Rect, Sense, Ui, Vec2};
use mde_egui::{Density, Style};
use mde_theme::brand::icons::IconId;

use crate::dock::{self, Surface};

/// One Auto Mode app — a curated vehicle app on the home's app strip.
///
/// PLATFORM-INTERFACES Q32 — exactly six: the Airspace tile is dropped (the
/// radar remains a Maps tab reachable from Nav + the keymap), the Phone tile is
/// folded into Comms (WL-FUNC-011 folded Voice's calls into Communications),
/// and Music (split from Media) joins the roster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CarTile {
    /// Navigation — the Drive HUD.
    Nav,
    /// Media — the full player (video + library).
    Media,
    /// Music — the dedicated music surface (Q32: new tile, split from Media).
    Music,
    /// Communications — calls + alerts + messages (the Phone tile folded in).
    Comms,
    /// Vehicle telematics (opens the Maps surface on its Vehicle tab).
    Vehicle,
    /// Settings — including the Car Mode Key Mapping page.
    Settings,
}

impl CarTile {
    /// The six apps in strip order (PLATFORM-INTERFACES Q32).
    pub const ALL: [Self; 6] = [
        Self::Nav,
        Self::Media,
        Self::Music,
        Self::Comms,
        Self::Vehicle,
        Self::Settings,
    ];

    /// The tile's headline label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Nav => "Navigation",
            Self::Media => "Media",
            Self::Music => "Music",
            Self::Comms => "Comms",
            Self::Vehicle => "Vehicle",
            Self::Settings => "Settings",
        }
    }

    /// The Carbon glyph the tile paints.
    #[must_use]
    pub const fn icon(self) -> IconId {
        match self {
            Self::Nav => IconId::MapsLocation,
            Self::Media => IconId::Media,
            Self::Music => IconId::Music,
            Self::Comms => IconId::Share,
            Self::Vehicle => IconId::HealthStatus,
            Self::Settings => IconId::Settings,
        }
    }

    /// The shell surface the tile routes to.
    #[must_use]
    pub const fn surface(self) -> Surface {
        match self {
            Self::Nav | Self::Vehicle => Surface::MapsLocation,
            Self::Media => Surface::Media,
            Self::Music => Surface::Music,
            // WL-FUNC-011 Phase-2 — the retired Voice surface's calls live in the
            // Communications hub; Q32 folds the old Phone tile in here too.
            Self::Comms => Surface::Communications,
            Self::Settings => Surface::System,
        }
    }

    /// The per-app accent used for the strip tile's glyph + hover cue — the
    /// categorical hues already in the shared palette, so the strip reads like
    /// the platform (Music shares the dock's Media-group hue).
    #[must_use]
    pub const fn accent(self) -> Color32 {
        match self {
            Self::Nav => Style::ACCENT_MESH,
            Self::Media | Self::Music => Style::ACCENT_MEDIA,
            Self::Comms => Style::ACCENT,
            Self::Vehicle => Style::OK,
            Self::Settings => Style::ACCENT_SYSTEM,
        }
    }
}

/// The live glance values the dashboard cards read. Each is `None` when there
/// is no honest live value (the card then shows a plain descriptor) — never a
/// mock (PLATFORM-INTERFACES Q31 + honesty P8).
#[derive(Clone, Debug, Default)]
pub struct CarHomeGlance {
    /// Navigation — active route summary/ETA (`None` ⇒ "Where to?").
    pub nav: Option<String>,
    /// Media — now-playing title (`None` ⇒ "Music & podcasts").
    pub media: Option<String>,
    /// Comms — count of retained (unacked) alerts (`None`/0 ⇒ "Alerts & messages").
    pub comms: Option<usize>,
    /// Vehicle — live telematics summary (`None` ⇒ "Telematics").
    pub vehicle: Option<String>,
}

impl CarHomeGlance {
    /// The Nav card's line: the live route summary, else the honest prompt.
    #[must_use]
    pub fn nav_line(&self) -> String {
        self.nav.clone().unwrap_or_else(|| "Where to?".to_string())
    }

    /// The Media card's line: the now-playing title, else the honest descriptor.
    #[must_use]
    pub fn media_line(&self) -> String {
        self.media
            .clone()
            .unwrap_or_else(|| "Music & podcasts".to_string())
    }

    /// The glance card's comms row: the alert count, else the honest descriptor.
    #[must_use]
    pub fn comms_line(&self) -> String {
        match self.comms {
            Some(n) if n > 0 => format!("{n} alert{}", if n == 1 { "" } else { "s" }),
            _ => "Alerts & messages".to_string(),
        }
    }

    /// The glance card's vehicle row: live telematics, else the honest descriptor.
    #[must_use]
    pub fn vehicle_line(&self) -> String {
        self.vehicle
            .clone()
            .unwrap_or_else(|| "Telematics".to_string())
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

/// The dashboard's split-card + app-strip geometry (PLATFORM-INTERFACES Q31).
///
/// The card band owns the upper ~three-quarters: the Nav card is the largest
/// (left, full band height), the Media and glance cards stack in the right
/// column. The app strip is one compact row across the bottom — a single row
/// keeps all six apps visible in one glance line under the cards and leaves
/// the card band the vertical majority (a 2×3 block would either shrink the
/// cards or overflow the remaining third).
pub(crate) struct CarHomeLayout {
    /// The Nav card — the largest card, left of the split.
    pub(crate) nav_card: Rect,
    /// The Media / now-playing card — right column, top.
    pub(crate) media_card: Rect,
    /// The glance card (vehicle + comms) — right column, bottom.
    pub(crate) glance_card: Rect,
    /// The six app-strip tiles, in [`CarTile::ALL`] order.
    pub(crate) strip: [Rect; 6],
}

/// Compute the dashboard geometry for the home's body rect, or `None` when the
/// rect is degenerate (crash-safety: nothing tiny/NaN reaches egui layout).
pub(crate) fn dashboard_layout(body: Rect) -> Option<CarHomeLayout> {
    if !body.is_finite() || body.width() < 2.0 || body.height() < 2.0 {
        return None;
    }
    let gap = Style::SP_M;
    // The strip stays compact but never below a real touch row: the finger
    // hit-target floor plus room for the glyph + label (Density::Touch, Q35).
    let strip_h = (body.height() * 0.26)
        .max(Density::Touch.min_hit_target() + Style::SP_XL)
        .min(body.height() * 0.45);
    let cards_h = (body.height() - strip_h - gap).max(1.0);
    let nav_w = ((body.width() - gap) * 0.56).max(1.0);
    let nav_card = Rect::from_min_size(body.min, egui::vec2(nav_w, cards_h));
    let right_x = nav_card.right() + gap;
    let right_w = (body.right() - right_x).max(1.0);
    let half_h = ((cards_h - gap) / 2.0).max(1.0);
    let media_card =
        Rect::from_min_size(egui::pos2(right_x, body.top()), egui::vec2(right_w, half_h));
    let glance_card = Rect::from_min_size(
        egui::pos2(right_x, media_card.bottom() + gap),
        egui::vec2(right_w, half_h),
    );
    let strip_top = body.bottom() - strip_h;
    let cols = CarTile::ALL.len() as f32;
    let tile_w = ((body.width() - gap * (cols - 1.0)) / cols).max(1.0);
    let strip = core::array::from_fn(|i| {
        Rect::from_min_size(
            egui::pos2(body.left() + i as f32 * (tile_w + gap), strip_top),
            egui::vec2(tile_w, strip_h),
        )
    });
    Some(CarHomeLayout {
        nav_card,
        media_card,
        glance_card,
        strip,
    })
}

/// Render the Auto Mode home. Returns the tile the driver activated this frame
/// (a card or strip tap), or `None`. The shell maps that to a surface switch.
pub fn car_home_panel(ui: &mut Ui, glance: &CarHomeGlance) -> Option<CarTile> {
    let full = ui.available_rect_before_wrap();
    // Guard a degenerate viewport (a collapsed/NaN rect never reaches layout).
    let width = finite_or(full.width(), 0.0);
    let height = finite_or(full.height(), 0.0);
    if width < 2.0 || height < 2.0 {
        return None;
    }

    let painter = ui.painter().clone();
    // The SYNC3 ground (Q30) — edge-to-edge black even inside a bordered panel.
    painter.rect_filled(full, 0.0, Style::SYNC3_BG);

    let pad = Style::SP_L;
    let inner = Rect::from_min_max(full.min + Vec2::splat(pad), full.max - Vec2::splat(pad));
    if inner.width() < 2.0 || inner.height() < 2.0 {
        return None;
    }

    // Header band — a large "Auto Mode" title, SYNC3 white.
    let header_h = Style::DISPLAY + Style::SP_M;
    painter.text(
        egui::pos2(inner.left(), inner.top()),
        egui::Align2::LEFT_TOP,
        "Auto Mode",
        egui::FontId::new(Style::DISPLAY, egui::FontFamily::Name("heading".into())),
        Style::SYNC3_TEXT_STRONG,
    );

    let body = Rect::from_min_max(egui::pos2(inner.left(), inner.top() + header_h), inner.max);
    let layout = dashboard_layout(body)?;

    let mut activated = None;
    if paint_nav_card(ui, &painter, layout.nav_card, glance) {
        activated = Some(CarTile::Nav);
    }
    if paint_media_card(ui, &painter, layout.media_card, glance) {
        activated = Some(CarTile::Media);
    }
    // The glance card's dominant content is the vehicle telematics summary, so
    // its (full-card, Density::Touch) tap lands on the Vehicle telematics tab.
    if paint_glance_card(ui, &painter, layout.glance_card, glance) {
        activated = Some(CarTile::Vehicle);
    }
    for (tile, rect) in CarTile::ALL.into_iter().zip(layout.strip) {
        if paint_app_tile(ui, &painter, rect, tile) {
            activated = Some(tile);
        }
    }
    activated
}

/// Paint one dashboard card's shared plate — the SYNC3_SURFACE ground on
/// RADIUS_L with the Ford-blue accent cap + five-state hover/press cues — and
/// return a card-clipped painter plus whether the card was tapped. `None` for a
/// degenerate rect.
fn card_plate(
    ui: &mut Ui,
    painter: &egui::Painter,
    rect: Rect,
    salt: &'static str,
) -> Option<(egui::Painter, bool)> {
    if !rect.is_finite() || rect.width() < 2.0 || rect.height() < 2.0 {
        return None;
    }
    let resp = ui.interact(rect, egui::Id::new(("car-home-card", salt)), Sense::click());
    let fill = if resp.is_pointer_button_down_on() {
        Style::pressed_fill(Style::SYNC3_ACCENT)
    } else if resp.hovered() {
        Style::SYNC3_SURFACE_HI
    } else {
        Style::SYNC3_SURFACE
    };
    let radius = egui::CornerRadius::same(Style::RADIUS_L as u8);
    painter.rect_filled(rect, radius, fill);
    let stroke_col = if resp.hovered() {
        Style::SYNC3_ACCENT
    } else {
        Style::SYNC3_BORDER
    };
    painter.rect_stroke(
        rect,
        radius,
        egui::Stroke::new(Style::STROKE_HAIRLINE, stroke_col),
        egui::StrokeKind::Inside,
    );
    // Accent top rule — the SYNC3-style Ford-blue cap on the card.
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
        Style::SYNC3_ACCENT,
    );
    // Clip the content to the card so a long now-playing title never overflows.
    Some((painter.with_clip_rect(rect), resp.clicked()))
}

/// A card's icon + app-name header row, top-left, SYNC3-accent tinted glyph.
fn card_header(ui: &Ui, p: &egui::Painter, rect: Rect, icon: IconId, title: &str) {
    let edge = (rect.height() * 0.2).clamp(20.0, 48.0);
    if let Some(tex) = dock::icon_texture(ui.ctx(), icon, edge, Style::SYNC3_ACCENT) {
        let icon_rect = Rect::from_min_size(
            egui::pos2(rect.left() + Style::SP_M, rect.top() + Style::SP_M),
            egui::vec2(edge, edge),
        );
        p.image(
            tex.id(),
            icon_rect,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    p.text(
        egui::pos2(
            rect.left() + Style::SP_M + edge + Style::SP_S,
            rect.top() + Style::SP_M + edge / 2.0,
        ),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::new(
            Style::TYPE_HEADLINE,
            egui::FontFamily::Name("heading".into()),
        ),
        Style::SYNC3_TEXT_DIM,
    );
}

/// The Nav card — the largest card: the live route/ETA glance while guidance
/// runs, else the honest "Where to?" prompt. Tap opens Navigation.
fn paint_nav_card(ui: &mut Ui, painter: &egui::Painter, rect: Rect, g: &CarHomeGlance) -> bool {
    let Some((p, clicked)) = card_plate(ui, painter, rect, "nav") else {
        return false;
    };
    card_header(ui, &p, rect, IconId::MapsLocation, "Navigation");
    // The glance line — live values read strong, the absent-data prompt reads
    // dim (the honesty cue: a fallback never masquerades as a reading).
    let live = g.nav.is_some();
    p.text(
        egui::pos2(rect.left() + Style::SP_M, rect.bottom() - Style::SP_M),
        egui::Align2::LEFT_BOTTOM,
        g.nav_line(),
        egui::FontId::new(Style::TYPE_TITLE2, egui::FontFamily::Proportional),
        if live {
            Style::SYNC3_TEXT_STRONG
        } else {
            Style::SYNC3_TEXT_DIM
        },
    );
    clicked
}

/// The Media card — the now-playing glance (honest "Music & podcasts" when
/// nothing is loaded). Tap opens Media.
fn paint_media_card(ui: &mut Ui, painter: &egui::Painter, rect: Rect, g: &CarHomeGlance) -> bool {
    let Some((p, clicked)) = card_plate(ui, painter, rect, "media") else {
        return false;
    };
    card_header(ui, &p, rect, IconId::Media, "Media");
    let live = g.media.is_some();
    p.text(
        egui::pos2(rect.left() + Style::SP_M, rect.bottom() - Style::SP_M),
        egui::Align2::LEFT_BOTTOM,
        g.media_line(),
        egui::FontId::new(Style::TYPE_TITLE3, egui::FontFamily::Proportional),
        if live {
            Style::SYNC3_TEXT_STRONG
        } else {
            Style::SYNC3_TEXT_DIM
        },
    );
    clicked
}

/// One glance-card row: a tinted glyph + its line at glance size.
fn glance_row(
    ui: &Ui,
    p: &egui::Painter,
    rect: Rect,
    icon: IconId,
    tint: Color32,
    line: &str,
    live: bool,
) {
    if !rect.is_finite() || rect.width() < 2.0 || rect.height() < 2.0 {
        return;
    }
    let edge = (rect.height() * 0.6).clamp(16.0, 32.0);
    if let Some(tex) = dock::icon_texture(ui.ctx(), icon, edge, tint) {
        let icon_rect = Rect::from_center_size(
            egui::pos2(rect.left() + edge / 2.0, rect.center().y),
            egui::vec2(edge, edge),
        );
        p.image(
            tex.id(),
            icon_rect,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    p.text(
        egui::pos2(rect.left() + edge + Style::SP_S, rect.center().y),
        egui::Align2::LEFT_CENTER,
        line,
        egui::FontId::new(Style::TYPE_TITLE3, egui::FontFamily::Proportional),
        if live {
            Style::SYNC3_TEXT_STRONG
        } else {
            Style::SYNC3_TEXT_DIM
        },
    );
}

/// The glance card — the vehicle telematics summary (the live MG90 glance when
/// the gateway drives location) over the comms alert count. Tap opens the
/// Vehicle telematics tab.
fn paint_glance_card(ui: &mut Ui, painter: &egui::Painter, rect: Rect, g: &CarHomeGlance) -> bool {
    let Some((p, clicked)) = card_plate(ui, painter, rect, "glance") else {
        return false;
    };
    let inset = Rect::from_min_max(
        rect.min + Vec2::splat(Style::SP_M),
        rect.max - Vec2::splat(Style::SP_M),
    );
    if inset.is_finite() && inset.width() >= 2.0 && inset.height() >= 2.0 {
        let half = inset.height() / 2.0;
        let vehicle_row = Rect::from_min_size(inset.min, egui::vec2(inset.width(), half));
        let comms_row = Rect::from_min_size(
            egui::pos2(inset.left(), inset.top() + half),
            egui::vec2(inset.width(), half),
        );
        glance_row(
            ui,
            &p,
            vehicle_row,
            IconId::HealthStatus,
            Style::OK,
            &g.vehicle_line(),
            g.vehicle.is_some(),
        );
        glance_row(
            ui,
            &p,
            comms_row,
            IconId::Share,
            Style::SYNC3_ACCENT,
            &g.comms_line(),
            g.comms.is_some_and(|n| n > 0),
        );
    }
    clicked
}

/// Paint one compact app-strip tile (glyph over label) and return whether it
/// was tapped this frame.
fn paint_app_tile(ui: &mut Ui, painter: &egui::Painter, rect: Rect, tile: CarTile) -> bool {
    if !rect.is_finite() || rect.width() < 2.0 || rect.height() < 2.0 {
        return false;
    }
    let id = egui::Id::new(("car-home-app", tile.label()));
    let resp = ui.interact(rect, id, Sense::click());

    let fill = if resp.is_pointer_button_down_on() {
        Style::pressed_fill(tile.accent())
    } else if resp.hovered() {
        Style::SYNC3_SURFACE_HI
    } else {
        Style::SYNC3_SURFACE
    };
    let radius = egui::CornerRadius::same(Style::RADIUS_M as u8);
    painter.rect_filled(rect, radius, fill);
    let stroke_col = if resp.hovered() {
        tile.accent()
    } else {
        Style::SYNC3_BORDER
    };
    painter.rect_stroke(
        rect,
        radius,
        egui::Stroke::new(Style::STROKE_HAIRLINE, stroke_col),
        egui::StrokeKind::Inside,
    );

    // Glyph centered in the upper portion, per-app accent tint.
    let icon_edge = (rect.height() * 0.32).clamp(18.0, 44.0);
    if let Some(tex) = dock::icon_texture(ui.ctx(), tile.icon(), icon_edge, tile.accent()) {
        let icon_center = egui::pos2(rect.center().x, rect.top() + rect.height() * 0.38);
        let icon_rect = Rect::from_center_size(icon_center, egui::vec2(icon_edge, icon_edge));
        painter.image(
            tex.id(),
            icon_rect,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }

    // Label — compact, SYNC3 white.
    painter.text(
        egui::pos2(rect.center().x, rect.bottom() - Style::SP_S),
        egui::Align2::CENTER_BOTTOM,
        tile.label(),
        egui::FontId::new(Style::TYPE_SUBHEADLINE, egui::FontFamily::Proportional),
        Style::SYNC3_TEXT_STRONG,
    );

    resp.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_egui::egui::{pos2, vec2};

    /// PLATFORM-INTERFACES Q32 — exactly six apps, each with its route + glyph.
    #[test]
    fn roster_is_exactly_the_six_auto_apps_with_their_routes() {
        assert_eq!(CarTile::ALL.len(), 6);
        assert_eq!(
            CarTile::ALL,
            [
                CarTile::Nav,
                CarTile::Media,
                CarTile::Music,
                CarTile::Comms,
                CarTile::Vehicle,
                CarTile::Settings,
            ]
        );
        for tile in CarTile::ALL {
            let _ = tile.icon();
            let _ = tile.accent();
            assert!(!tile.label().is_empty());
        }
        assert_eq!(CarTile::Nav.surface(), Surface::MapsLocation);
        assert_eq!(CarTile::Media.surface(), Surface::Media);
        assert_eq!(CarTile::Music.surface(), Surface::Music);
        assert_eq!(CarTile::Comms.surface(), Surface::Communications);
        assert_eq!(CarTile::Vehicle.surface(), Surface::MapsLocation);
        assert_eq!(CarTile::Settings.surface(), Surface::System);
    }

    #[test]
    fn glance_lines_fall_back_to_honest_descriptors_not_mock_data() {
        let empty = CarHomeGlance::default();
        assert_eq!(empty.nav_line(), "Where to?");
        assert_eq!(empty.media_line(), "Music & podcasts");
        assert_eq!(empty.comms_line(), "Alerts & messages");
        assert_eq!(empty.vehicle_line(), "Telematics");

        let live = CarHomeGlance {
            nav: Some("12 min · 4.3 mi · ETA 14:32".to_string()),
            media: Some("Comfortably Numb · Pink Floyd".to_string()),
            comms: Some(3),
            vehicle: Some("38 mph".to_string()),
        };
        assert_eq!(live.nav_line(), "12 min · 4.3 mi · ETA 14:32");
        assert_eq!(live.media_line(), "Comfortably Numb · Pink Floyd");
        assert_eq!(live.comms_line(), "3 alerts");
        assert_eq!(live.vehicle_line(), "38 mph");
        assert_eq!(
            CarHomeGlance {
                comms: Some(1),
                ..Default::default()
            }
            .comms_line(),
            "1 alert"
        );
        // A zero count is not an alert — the honest descriptor, never "0 alerts".
        assert_eq!(
            CarHomeGlance {
                comms: Some(0),
                ..Default::default()
            }
            .comms_line(),
            "Alerts & messages"
        );
    }

    /// Q31 — the split-card band owns the vertical majority, the Nav card is the
    /// largest, and the strip is one compact touch-height row of six.
    #[test]
    fn dashboard_layout_splits_cards_over_a_single_row_strip() {
        let body = Rect::from_min_size(pos2(0.0, 0.0), vec2(1024.0, 560.0));
        let l = dashboard_layout(body).expect("a real body rect lays out");

        // The Nav card is the largest card.
        let area = |r: Rect| r.width() * r.height();
        assert!(area(l.nav_card) > area(l.media_card));
        assert!(area(l.nav_card) > area(l.glance_card));

        // The card band holds the vertical majority; the strip is the remainder.
        let strip_h = l.strip[0].height();
        assert!(l.nav_card.height() > strip_h);
        assert!(strip_h >= Density::Touch.min_hit_target());

        // Six strip tiles, in order, disjoint, inside the body.
        assert_eq!(l.strip.len(), CarTile::ALL.len());
        for (i, r) in l.strip.iter().enumerate() {
            assert!(body.contains_rect(*r), "strip tile {i} inside the body");
            if i > 0 {
                assert!(
                    r.left() > l.strip[i - 1].right(),
                    "strip tiles ordered + disjoint"
                );
            }
        }
        // Cards don't overlap each other or the strip.
        assert!(l.media_card.left() > l.nav_card.right());
        assert!(l.glance_card.top() > l.media_card.bottom());
        assert!(l.strip[0].top() > l.nav_card.bottom());

        // A degenerate body never lays out (crash-safety).
        assert!(dashboard_layout(Rect::from_min_size(pos2(0.0, 0.0), vec2(1.0, 1.0))).is_none());
        assert!(
            dashboard_layout(Rect::from_min_size(pos2(0.0, 0.0), vec2(f32::NAN, 100.0))).is_none()
        );
    }

    /// The headless render harness: `Context::run` → tessellate (the DRM
    /// runner's path minus the GPU), driving the panel over a margin-less
    /// CentralPanel so the geometry matches [`dashboard_layout`] exactly.
    fn drive(
        glance: &CarHomeGlance,
        frames: Vec<Vec<egui::Event>>,
    ) -> (Vec<Option<CarTile>>, Vec<egui::epaint::ClippedShape>) {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut picks = Vec::new();
        let mut shapes = Vec::new();
        for events in frames {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1024.0, 640.0))),
                events,
                ..Default::default()
            };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        picks.push(car_home_panel(ui, glance));
                    });
            });
            let prims = ctx.tessellate(out.shapes.clone(), out.pixels_per_point);
            assert!(!prims.is_empty(), "frame produced no draw primitives");
            shapes = out.shapes;
        }
        (picks, shapes)
    }

    /// Every painted text run from a frame's shapes.
    fn painted_text(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
        fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for shape in shapes {
            walk(&shape.shape, &mut out);
        }
        out
    }

    /// Q31 + honesty P8 — with no live data the cards paint their honest
    /// absent-data descriptors and never a fabricated reading.
    #[test]
    fn dashboard_renders_honest_fallbacks_with_default_glance() {
        let (picks, shapes) = drive(&CarHomeGlance::default(), vec![vec![]]);
        assert_eq!(picks, vec![None], "no input activates nothing");

        let texts = painted_text(&shapes);
        for expected in [
            "Auto Mode",
            "Navigation",
            "Media",
            "Where to?",
            "Music & podcasts",
            "Telematics",
            "Alerts & messages",
        ] {
            assert!(
                texts.iter().any(|t| t == expected),
                "expected {expected:?} in {texts:?}"
            );
        }
        // All six strip labels paint.
        for tile in CarTile::ALL {
            assert!(
                texts.iter().any(|t| t == tile.label()),
                "strip label {:?} in {texts:?}",
                tile.label()
            );
        }
        // No fabricated readings: nothing numeric leaks from an empty glance.
        assert!(
            !texts.iter().any(|t| t.contains("mph")
                || t.contains("alert ")
                || t.contains("ETA")
                || t.contains("min ·")),
            "an empty glance must paint no invented readings: {texts:?}"
        );
    }

    #[test]
    fn dashboard_renders_a_populated_live_glance() {
        let glance = CarHomeGlance {
            nav: Some("12 min · 4.3 mi · ETA 14:32".to_string()),
            media: Some("Comfortably Numb · Pink Floyd".to_string()),
            comms: Some(3),
            vehicle: Some("38 mph".to_string()),
        };
        let (_, shapes) = drive(&glance, vec![vec![]]);
        let texts = painted_text(&shapes);
        for expected in [
            "12 min · 4.3 mi · ETA 14:32",
            "Comfortably Numb · Pink Floyd",
            "3 alerts",
            "38 mph",
        ] {
            assert!(
                texts.iter().any(|t| t == expected),
                "expected {expected:?} in {texts:?}"
            );
        }
        // The live values replace the prompts, not join them.
        assert!(
            !texts.iter().any(|t| t == "Where to?"),
            "a live route replaces the prompt: {texts:?}"
        );
    }

    fn pointer_button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// A tap on a strip tile / card routes its `CarTile` back to the shell.
    #[test]
    fn taps_route_the_strip_and_the_cards() {
        // Recompute the panel's own geometry (margin-less CentralPanel over the
        // 1024×640 screen → `full` == screen).
        let full = Rect::from_min_size(pos2(0.0, 0.0), vec2(1024.0, 640.0));
        let inner = Rect::from_min_max(
            full.min + Vec2::splat(Style::SP_L),
            full.max - Vec2::splat(Style::SP_L),
        );
        let body = Rect::from_min_max(
            pos2(inner.left(), inner.top() + Style::DISPLAY + Style::SP_M),
            inner.max,
        );
        let l = dashboard_layout(body).expect("layout");

        let tap = |pos: egui::Pos2| {
            vec![
                vec![],
                vec![egui::Event::PointerMoved(pos), pointer_button(pos, true)],
                vec![pointer_button(pos, false)],
            ]
        };

        // Strip: the Music tile (index 2 in Q32 order) routes CarTile::Music.
        let (picks, _) = drive(&CarHomeGlance::default(), tap(l.strip[2].center()));
        assert_eq!(picks.last(), Some(&Some(CarTile::Music)));

        // The Nav card routes CarTile::Nav.
        let (picks, _) = drive(&CarHomeGlance::default(), tap(l.nav_card.center()));
        assert_eq!(picks.last(), Some(&Some(CarTile::Nav)));

        // The glance card routes CarTile::Vehicle (its telematics tab target).
        let (picks, _) = drive(&CarHomeGlance::default(), tap(l.glance_card.center()));
        assert_eq!(picks.last(), Some(&Some(CarTile::Vehicle)));
    }
}
