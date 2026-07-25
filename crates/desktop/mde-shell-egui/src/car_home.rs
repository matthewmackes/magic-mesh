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
use mde_egui::{Density, Style, TypographyRole};
use mde_theme::brand::icons::IconId;

use crate::surfaces::{self, Surface};

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
    /// The narrowest safe visual labels for a 44pt Car strip tile.
    ///
    /// These are presentation labels only: [`Self::label`] remains the full
    /// accessible/widget label, so shortening the painted text never makes a
    /// route ambiguous to keyboard or assistive navigation.
    #[must_use]
    pub(crate) const fn compact_label(self) -> &'static str {
        match self {
            Self::Nav => "Nav",
            Self::Media => "Med",
            Self::Music => "Mus",
            Self::Comms => "Com",
            Self::Vehicle => "Veh",
            Self::Settings => "Set",
        }
    }

    /// Pick a strip label that fits without weakening the 44pt hit target.
    ///
    /// The strip is allowed to remain interactive at its exact minimum width;
    /// only its visual copy compresses. The intermediate threshold shortens
    /// the longest label before it can touch an adjacent tile.
    #[must_use]
    pub(crate) const fn strip_label(self, tile_width: f32) -> &'static str {
        if tile_width < Density::Touch.min_hit_target() + Style::SP_S * 2.0 {
            self.compact_label()
        } else if tile_width < Density::Touch.min_hit_target() + Style::SP_XL {
            match self {
                Self::Nav => "Nav",
                _ => self.label(),
            }
        } else {
            self.label()
        }
    }

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
/// mock (PLATFORM-INTERFACES Q31 + honesty P8). A non-empty `vehicle` value can
/// also be a degraded status label (for example stale/offline MG90); only
/// `vehicle_live` lets the card promote it with the strong live color.
#[derive(Clone, Debug, Default)]
pub struct CarHomeGlance {
    /// Navigation — active route summary/ETA (`None` ⇒ "Where to?").
    pub nav: Option<String>,
    /// Media — now-playing title (`None` ⇒ "Music & podcasts").
    pub media: Option<String>,
    /// Comms — count of retained (unacked) alerts (`None`/0 ⇒ "Alerts & messages").
    pub comms: Option<usize>,
    /// Vehicle — live telematics summary or a degraded MG90 status label
    /// (`None` ⇒ "Telematics").
    pub vehicle: Option<String>,
    /// Whether [`Self::vehicle`] is a fresh live telematics value. `false` keeps
    /// stale/offline/awaiting labels dim: explicit, but not promoted as live.
    pub vehicle_live: bool,
}

/// Keep untrusted gateway strings from creating huge galleys on a moving Car
/// dashboard. This is a render-side budget; the source value remains intact so
/// the shell's live model does not silently lose information.
const MAX_CARD_TEXT_CHARS: usize = 128;
const MAX_EXACT_ALERT_COUNT: usize = 999;

/// Normalize a gateway value to one bounded, single-line render source.
///
/// Returning the truncation bit separately lets the width fitter distinguish a
/// source that was already capped from one that merely happens to end at the
/// character budget. Iteration stops at the budget plus one character, so a
/// pathological payload never gets scanned or copied in full by the painter.
fn bounded_single_line(text: &str) -> (String, bool) {
    let mut out = String::with_capacity(text.len().min(MAX_CARD_TEXT_CHARS));
    let mut last_was_space = false;
    let mut chars = text.chars();
    for ch in chars.by_ref().take(MAX_CARD_TEXT_CHARS) {
        if ch.is_whitespace() || ch.is_control() {
            if !out.is_empty() && !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(ch);
            last_was_space = false;
        }
    }
    let truncated = chars.next().is_some();
    while out.ends_with(' ') {
        out.pop();
    }
    (out, truncated)
}

/// Width-fit one card value before handing it to egui's text layout.
///
/// The source is bounded first, then binary-searched by Unicode scalar value
/// count. That keeps the expensive layout work logarithmic and never splits a
/// UTF-8 code point; the final ellipsis makes the loss visible to the driver.
fn fit_card_text(
    painter: &egui::Painter,
    text: &str,
    font: &egui::FontId,
    max_width: f32,
) -> String {
    let (source, truncated) = bounded_single_line(text);
    if source.is_empty() {
        return source;
    }
    let max_width = if max_width.is_finite() {
        max_width.max(1.0)
    } else {
        1.0
    };
    let candidate = if truncated {
        format!("{source}…")
    } else {
        source.clone()
    };
    if painter
        .layout_no_wrap(candidate.clone(), font.clone(), Color32::WHITE)
        .size()
        .x
        <= max_width
    {
        return candidate;
    }

    let chars: Vec<char> = source.chars().collect();
    let mut low = 0;
    let mut high = chars.len();
    while low < high {
        let mid = (low + high).div_ceil(2);
        let prefix: String = chars.iter().take(mid).collect();
        let candidate = format!("{prefix}…");
        if painter
            .layout_no_wrap(candidate, font.clone(), Color32::WHITE)
            .size()
            .x
            <= max_width
        {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    if low == 0 {
        "…".to_string()
    } else {
        let prefix: String = chars.into_iter().take(low).collect();
        format!("{prefix}…")
    }
}

/// Paint a left-anchored value at the bottom of a card without allowing its
/// galley to escape the card's inner width.
fn paint_card_bottom_line(
    painter: &egui::Painter,
    rect: Rect,
    text: &str,
    font: egui::FontId,
    color: Color32,
) {
    let max_width = (rect.width() - Style::SP_M * 2.0).max(1.0);
    let line = fit_card_text(painter, text, &font, max_width);
    let galley = painter.layout_no_wrap(line, font, color);
    painter.galley(
        egui::pos2(
            rect.left() + Style::SP_M,
            rect.bottom() - Style::SP_M - galley.size().y,
        ),
        galley,
        color,
    );
}

/// Paint a left-anchored value centered in a glance row, reserving the row's
/// right inset so a live value cannot touch the card edge.
fn paint_glance_line(
    painter: &egui::Painter,
    rect: Rect,
    left: f32,
    text: &str,
    font: egui::FontId,
    color: Color32,
) {
    let max_width = (rect.right() - left - Style::SP_S).max(1.0);
    let line = fit_card_text(painter, text, &font, max_width);
    let galley = painter.layout_no_wrap(line, font, color);
    painter.galley(
        egui::pos2(left, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
}

impl CarHomeGlance {
    /// A gateway can publish an optional field with an empty payload while its
    /// feed is present but sparse. Treat that exactly like absent data: an
    /// empty strong-colored line would falsely suggest a live reading.
    fn non_empty(value: Option<&str>) -> Option<&str> {
        value.map(str::trim).filter(|value| {
            value
                .chars()
                .any(|ch| !ch.is_whitespace() && !ch.is_control())
        })
    }

    fn nav_value(&self) -> Option<&str> {
        Self::non_empty(self.nav.as_deref())
    }

    fn media_value(&self) -> Option<&str> {
        Self::non_empty(self.media.as_deref())
    }

    fn vehicle_value(&self) -> Option<&str> {
        Self::non_empty(self.vehicle.as_deref())
    }

    /// The Nav card's line: the live route summary, else the honest prompt.
    #[must_use]
    pub fn nav_line(&self) -> String {
        self.nav_value().unwrap_or("Where to?").to_owned()
    }

    /// The Media card's line: the now-playing title, else the honest descriptor.
    #[must_use]
    pub fn media_line(&self) -> String {
        self.media_value().unwrap_or("Music & podcasts").to_owned()
    }

    /// The glance card's comms row: the alert count, else the honest descriptor.
    #[must_use]
    pub fn comms_line(&self) -> String {
        match self.comms {
            Some(n) if n > 0 => {
                if n > MAX_EXACT_ALERT_COUNT {
                    "999+ alerts".to_string()
                } else {
                    format!("{n} alert{}", if n == 1 { "" } else { "s" })
                }
            }
            _ => "Alerts & messages".to_string(),
        }
    }

    /// The glance card's vehicle row: live telematics, else the honest descriptor.
    #[must_use]
    pub fn vehicle_line(&self) -> String {
        self.vehicle_value().unwrap_or("Telematics").to_owned()
    }

    /// Whether the vehicle row is an honest fresh MG90 reading. A stale/offline
    /// status label remains useful to the driver but must paint dim.
    #[must_use]
    pub fn vehicle_line_live(&self) -> bool {
        self.vehicle_live && self.vehicle_value().is_some()
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
/// rect is degenerate or too small for safe Car touch targets.
pub(crate) fn dashboard_layout(body: Rect) -> Option<CarHomeLayout> {
    if !body.is_finite() || body.width() < 2.0 || body.height() < 2.0 {
        return None;
    }
    let gap = Style::SP_M;
    let cols = CarTile::ALL.len() as f32;
    let touch_target = Density::Touch.min_hit_target();
    // Car's one-row strip and split cards are all primary targets. If the
    // viewport cannot hold six square 44pt targets plus the design gaps, or
    // two 44pt glance-card rows plus the strip, returning no layout is safer
    // than painting controls outside the body or shrinking them below the
    // driver's touch contract.
    let min_strip_height = touch_target + Style::SP_XL;
    let min_cards_height = touch_target * 2.0 + gap;
    let min_body_width = touch_target * cols + gap * (cols - 1.0);
    let min_body_height = min_strip_height + gap + min_cards_height;
    if body.width() < min_body_width || body.height() < min_body_height {
        return None;
    }
    // The strip stays compact but never below a real touch row: the finger
    // hit-target floor plus room for the glyph + label (Density::Touch, Q35).
    let strip_h = (body.height() * 0.26)
        .max(min_strip_height)
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
    let tile_w = (body.width() - gap * (cols - 1.0)) / cols;
    let strip = core::array::from_fn(|i| {
        Rect::from_min_size(
            egui::pos2(body.left() + i as f32 * (tile_w + gap), strip_top),
            egui::vec2(tile_w, strip_h),
        )
    });
    let layout = CarHomeLayout {
        nav_card,
        media_card,
        glance_card,
        strip,
    };
    // Keep the guard close to the geometry producer: future ratio changes must
    // fail closed instead of handing egui an off-body interaction rectangle.
    if !body.contains_rect(layout.nav_card)
        || !body.contains_rect(layout.media_card)
        || !body.contains_rect(layout.glance_card)
        || layout.strip.iter().any(|rect| !body.contains_rect(*rect))
    {
        return None;
    }
    Some(layout)
}

/// Resolve the one Car Home activation returned to the shell.
///
/// The cards are deliberately painted in visual order, but their interaction
/// responses must not use last-writer-wins routing. Navigation is the primary
/// blue Home action; if a future inset, accessibility expansion, or rounding
/// change makes its hit region overlap a later card, it must still open the
/// Navigation home instead of falling through to Vehicle/OBD.
fn activated_car_tile(
    nav_clicked: bool,
    media_clicked: bool,
    vehicle_clicked: bool,
    strip_clicked: [bool; 6],
) -> Option<CarTile> {
    if nav_clicked {
        return Some(CarTile::Nav);
    }
    if media_clicked {
        return Some(CarTile::Media);
    }
    if vehicle_clicked {
        return Some(CarTile::Vehicle);
    }
    CarTile::ALL
        .into_iter()
        .zip(strip_clicked)
        .find_map(|(tile, clicked)| clicked.then_some(tile))
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
        Style::typography_font(TypographyRole::Display),
        Style::SYNC3_TEXT_STRONG,
    );

    let body = Rect::from_min_max(egui::pos2(inner.left(), inner.top() + header_h), inner.max);
    let Some(layout) = dashboard_layout(body) else {
        // A narrow/short workspace is an honest degraded state, not a reason
        // to paint controls below the touch-target floor. Tell the driver why
        // the cards are absent instead of leaving a title over an empty panel.
        if body.is_finite() && body.width() >= 2.0 && body.height() >= 2.0 {
            painter.text(
                body.center(),
                egui::Align2::CENTER_CENTER,
                "Resize workspace to use Auto Mode",
                Style::typography_font(TypographyRole::Body),
                Style::SYNC3_TEXT_DIM,
            );
        }
        return None;
    };

    let nav_clicked = paint_nav_card(ui, &painter, layout.nav_card, glance);
    let media_clicked = paint_media_card(ui, &painter, layout.media_card, glance);
    // The glance card's dominant content is the vehicle telematics summary, so
    // its (full-card, Density::Touch) tap lands on the Vehicle telematics tab.
    let vehicle_clicked = paint_glance_card(ui, &painter, layout.glance_card, glance);
    let strip_clicked = core::array::from_fn(|index| {
        paint_app_tile(ui, &painter, layout.strip[index], CarTile::ALL[index])
    });
    activated_car_tile(nav_clicked, media_clicked, vehicle_clicked, strip_clicked)
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
    label: &'static str,
) -> Option<(egui::Painter, bool)> {
    if !rect.is_finite() || rect.width() < 2.0 || rect.height() < 2.0 {
        return None;
    }
    let resp = ui.interact(rect, egui::Id::new(("car-home-card", salt)), Sense::click());
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
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
    mde_egui::focus::paint_focus_ring(painter, rect, resp.has_focus());
    // Clip the content to the card so a long now-playing title never overflows.
    Some((
        painter.with_clip_rect(rect),
        surfaces::response_activated(ui, &resp),
    ))
}

/// A card's icon + app-name header row, top-left, SYNC3-accent tinted glyph.
fn card_header(ui: &Ui, p: &egui::Painter, rect: Rect, icon: IconId, title: &str) {
    let edge = (rect.height() * 0.2).clamp(20.0, 48.0);
    if let Some(tex) = surfaces::icon_texture(ui.ctx(), icon, edge, Style::SYNC3_ACCENT) {
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
        Style::typography_font(TypographyRole::Headline),
        Style::SYNC3_TEXT_DIM,
    );
}

/// The Nav card — the largest card: the live route/ETA glance while guidance
/// runs, else the honest "Where to?" prompt. Tap opens Navigation.
fn paint_nav_card(ui: &mut Ui, painter: &egui::Painter, rect: Rect, g: &CarHomeGlance) -> bool {
    let Some((p, clicked)) = card_plate(ui, painter, rect, "nav", "Navigation") else {
        return false;
    };
    card_header(ui, &p, rect, IconId::MapsLocation, "Navigation");
    // The glance line — live values read strong, the absent-data prompt reads
    // dim (the honesty cue: a fallback never masquerades as a reading).
    let live = g.nav_value().is_some();
    paint_card_bottom_line(
        &p,
        rect,
        &g.nav_line(),
        Style::typography_font(TypographyRole::Title),
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
    let Some((p, clicked)) = card_plate(ui, painter, rect, "media", "Media") else {
        return false;
    };
    card_header(ui, &p, rect, IconId::Media, "Media");
    let live = g.media_value().is_some();
    paint_card_bottom_line(
        &p,
        rect,
        &g.media_line(),
        Style::typography_font(TypographyRole::Body),
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
    if let Some(tex) = surfaces::icon_texture(ui.ctx(), icon, edge, tint) {
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
    paint_glance_line(
        p,
        rect,
        rect.left() + edge + Style::SP_S,
        line,
        Style::typography_font(TypographyRole::Body),
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
    let Some((p, clicked)) = card_plate(ui, painter, rect, "glance", "Vehicle telematics") else {
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
            g.vehicle_line_live(),
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
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tile.label())
    });

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

    // Clip all tile content so a future label or icon change cannot paint over
    // a neighboring touch target.
    let content_painter = painter.with_clip_rect(rect);

    // Glyph centered in the upper portion, per-app accent tint.
    let icon_edge = (rect.height() * 0.32).clamp(18.0, 44.0);
    if let Some(tex) = surfaces::icon_texture(ui.ctx(), tile.icon(), icon_edge, tile.accent()) {
        let icon_center = egui::pos2(rect.center().x, rect.top() + rect.height() * 0.38);
        let icon_rect = Rect::from_center_size(icon_center, egui::vec2(icon_edge, icon_edge));
        content_painter.image(
            tex.id(),
            icon_rect,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }

    // Label — compact, SYNC3 white. The full `tile.label()` remains in
    // WidgetInfo above for accessibility.
    content_painter.text(
        egui::pos2(rect.center().x, rect.bottom() - Style::SP_S),
        egui::Align2::CENTER_BOTTOM,
        tile.strip_label(rect.width()),
        Style::typography_font(TypographyRole::Label),
        Style::SYNC3_TEXT_STRONG,
    );

    mde_egui::focus::paint_focus_ring(painter, rect, resp.has_focus());
    surfaces::response_activated(ui, &resp)
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
    fn navigation_home_route_wins_over_vehicle_fallback() {
        // This models the failure mode directly: the large blue Navigation
        // card and the later telematics card both report a hit. Navigation must
        // remain the selected route; the shell can then record the normal
        // expanded-surface transition for its Back action.
        assert_eq!(
            activated_car_tile(true, false, true, [false; 6]),
            Some(CarTile::Nav)
        );
        assert_ne!(CarTile::Nav, CarTile::Vehicle);

        // The Vehicle card keeps its independent telematics route when the
        // Navigation card was not activated.
        assert_eq!(
            activated_car_tile(false, false, true, [false; 6]),
            Some(CarTile::Vehicle)
        );
    }

    #[test]
    fn glance_lines_fall_back_to_honest_descriptors_not_mock_data() {
        let empty = CarHomeGlance::default();
        assert_eq!(empty.nav_line(), "Where to?");
        assert_eq!(empty.media_line(), "Music & podcasts");
        assert_eq!(empty.comms_line(), "Alerts & messages");
        assert_eq!(empty.vehicle_line(), "Telematics");

        // A sparse gateway payload is not a live reading: empty and whitespace
        // strings must keep the dim honest descriptors instead of a blank,
        // strong-colored card line.
        let sparse = CarHomeGlance {
            nav: Some("  ".to_string()),
            media: Some(String::new()),
            vehicle: Some("\n\t".to_string()),
            ..Default::default()
        };
        assert_eq!(sparse.nav_line(), "Where to?");
        assert_eq!(sparse.media_line(), "Music & podcasts");
        assert_eq!(sparse.vehicle_line(), "Telematics");

        // Control-only gateway payloads are also absent data. They are not
        // removed by `str::trim`, but the render sanitizer removes them; do
        // not classify that resulting empty line as a live reading.
        let control_only = CarHomeGlance {
            nav: Some("\u{0000}\u{0007}".to_string()),
            media: Some("\u{001b}".to_string()),
            vehicle: Some("\u{001f}".to_string()),
            ..Default::default()
        };
        assert_eq!(control_only.nav_line(), "Where to?");
        assert_eq!(control_only.media_line(), "Music & podcasts");
        assert_eq!(control_only.vehicle_line(), "Telematics");

        let live = CarHomeGlance {
            nav: Some("12 min · 4.3 mi · ETA 14:32".to_string()),
            media: Some("Comfortably Numb · Pink Floyd".to_string()),
            comms: Some(3),
            vehicle: Some("38 mph".to_string()),
            vehicle_live: true,
        };
        assert_eq!(live.nav_line(), "12 min · 4.3 mi · ETA 14:32");
        assert_eq!(live.media_line(), "Comfortably Numb · Pink Floyd");
        assert_eq!(live.comms_line(), "3 alerts");
        assert_eq!(live.vehicle_line(), "38 mph");
        assert!(live.vehicle_line_live());

        let degraded = CarHomeGlance {
            vehicle: Some("MG90 stale · 6 s".to_string()),
            vehicle_live: false,
            ..Default::default()
        };
        assert_eq!(degraded.vehicle_line(), "MG90 stale · 6 s");
        assert!(
            !degraded.vehicle_line_live(),
            "explicit stale/offline MG90 labels stay dim, not promoted as live"
        );
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
        assert_eq!(
            CarHomeGlance {
                comms: Some(MAX_EXACT_ALERT_COUNT + 1),
                ..Default::default()
            }
            .comms_line(),
            "999+ alerts"
        );
    }

    #[test]
    fn dashboard_bounds_hostile_live_text_before_painting() {
        let hostile = format!("Destination\n{}", "🚗".repeat(MAX_CARD_TEXT_CHARS * 16));
        let glance = CarHomeGlance {
            nav: Some(hostile.clone()),
            media: Some(hostile.clone()),
            comms: Some(usize::MAX),
            vehicle: Some(hostile),
            vehicle_live: true,
        };
        let (_, shapes) = drive_with_screen(&glance, vec![vec![]], vec2(512.0, 640.0));
        let texts = painted_text(&shapes);
        let truncated = texts
            .iter()
            .filter(|text| text.ends_with('…'))
            .collect::<Vec<_>>();

        // Nav, Media, and Vehicle each receive a bounded, visible truncation
        // marker; no newline can turn a moving-card value into a second line.
        assert_eq!(truncated.len(), 3, "bounded live values in {texts:?}");
        assert!(texts.iter().all(|text| !text.contains('\n')));
        assert!(
            truncated
                .iter()
                .all(|text| text.chars().count() <= MAX_CARD_TEXT_CHARS + 1),
            "painted values remain within the scalar budget: {texts:?}"
        );
        assert!(
            texts.iter().any(|text| text == "999+ alerts"),
            "large counts use an honest bounded indicator: {texts:?}"
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
            assert!(r.width() >= Density::Touch.min_hit_target());
            assert!(r.height() >= Density::Touch.min_hit_target());
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
        for (name, card) in [
            ("nav", l.nav_card),
            ("media", l.media_card),
            ("glance", l.glance_card),
        ] {
            assert!(
                card.width() >= Density::Touch.min_hit_target()
                    && card.height() >= Density::Touch.min_hit_target(),
                "{name} card remains a touch target: {card:?}"
            );
        }

        // A one-row six-app strip cannot remain a safe touch surface below its
        // minimum width, and the split cards cannot remain two touch rows below
        // their minimum height. Reject both before producing off-body rects.
        let cols = CarTile::ALL.len() as f32;
        let min_width = Density::Touch.min_hit_target() * cols + Style::SP_M * (cols - 1.0);
        let min_height = (Density::Touch.min_hit_target() + Style::SP_XL)
            + Style::SP_M
            + (Density::Touch.min_hit_target() * 2.0 + Style::SP_M);
        assert!(dashboard_layout(Rect::from_min_size(
            pos2(0.0, 0.0),
            vec2(min_width - 1.0, min_height + 64.0),
        ))
        .is_none());
        assert!(dashboard_layout(Rect::from_min_size(
            pos2(0.0, 0.0),
            vec2(min_width + 64.0, min_height - 1.0),
        ))
        .is_none());
        let minimum = dashboard_layout(Rect::from_min_size(
            pos2(0.0, 0.0),
            vec2(min_width, min_height),
        ))
        .expect("the exact safe minimum still has a complete dashboard");
        assert!(minimum.strip.iter().all(|r| {
            r.width() >= Density::Touch.min_hit_target()
                && r.height() >= Density::Touch.min_hit_target()
        }));

        // A degenerate body never lays out (crash-safety).
        assert!(dashboard_layout(Rect::from_min_size(pos2(0.0, 0.0), vec2(1.0, 1.0))).is_none());
        assert!(
            dashboard_layout(Rect::from_min_size(pos2(0.0, 0.0), vec2(f32::NAN, 100.0))).is_none()
        );
    }

    /// The headless render harness: `Context::run` → tessellate (the DRM
    /// runner's path minus the GPU), driving the panel over a margin-less
    /// CentralPanel so the geometry matches [`dashboard_layout`] exactly.
    fn drive_with_screen(
        glance: &CarHomeGlance,
        frames: Vec<Vec<egui::Event>>,
        screen_size: Vec2,
    ) -> (Vec<Option<CarTile>>, Vec<egui::epaint::ClippedShape>) {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut picks = Vec::new();
        let mut shapes = Vec::new();
        for events in frames {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), screen_size)),
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

    fn drive(
        glance: &CarHomeGlance,
        frames: Vec<Vec<egui::Event>>,
    ) -> (Vec<Option<CarTile>>, Vec<egui::epaint::ClippedShape>) {
        drive_with_screen(glance, frames, vec2(1024.0, 640.0))
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

    fn car_home_body_rect(screen_size: Vec2) -> Rect {
        let full = Rect::from_min_size(pos2(0.0, 0.0), screen_size);
        let inner = Rect::from_min_max(
            full.min + Vec2::splat(Style::SP_L),
            full.max - Vec2::splat(Style::SP_L),
        );
        Rect::from_min_max(
            pos2(inner.left(), inner.top() + Style::DISPLAY + Style::SP_M),
            inner.max,
        )
    }

    fn captured_car_home_canvas(
        glance: &CarHomeGlance,
        screen_size: Vec2,
    ) -> crate::screenshot::Canvas {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), screen_size)),
            ..Default::default()
        };
        let mut cap = crate::screenshot::Capture::new();
        let mut render = |ctx: &egui::Context| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    let _ = car_home_panel(ui, glance);
                });
        };
        let _settle = cap.frame(&ctx, input(), &mut render);
        let canvas = cap.frame(&ctx, input(), &mut render);
        assert_eq!(
            (canvas.width(), canvas.height()),
            (
                screen_size.x.round() as usize,
                screen_size.y.round() as usize
            ),
            "Car Home pixel proof canvas must match the driven viewport"
        );
        assert!(!canvas.is_blank(), "Car Home pixel proof must not be blank");
        canvas
    }

    fn rect_pixels(rect: Rect, canvas: &crate::screenshot::Canvas) -> usize {
        let x0 = rect.left().floor().max(0.0) as usize;
        let y0 = rect.top().floor().max(0.0) as usize;
        let x1 = rect.right().ceil().min(canvas.width() as f32) as usize;
        let y1 = rect.bottom().ceil().min(canvas.height() as f32) as usize;
        x1.saturating_sub(x0) * y1.saturating_sub(y0)
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
    fn car_home_pixel_proof_paints_sync3_dashboard_and_honest_mg90_state() {
        let screen_size = vec2(1024.0, 640.0);
        let glance = CarHomeGlance {
            nav: Some("12 min · 4.3 mi · ETA 14:32".to_string()),
            media: Some("Local radio · 101.1 FM".to_string()),
            comms: Some(2),
            vehicle: Some("MG90 stale · no fix".to_string()),
            vehicle_live: false,
        };
        let canvas = captured_car_home_canvas(&glance, screen_size);
        let body = car_home_body_rect(screen_size);
        let layout = dashboard_layout(body).expect("pixel proof body lays out");

        let total_pixels = canvas.width() * canvas.height();
        let bg_pixels = canvas.count_exact_color(Style::SYNC3_BG);
        assert!(
            bg_pixels > total_pixels / 20,
            "Car Home proof must paint the SYNC3 ground, got {bg_pixels}/{total_pixels}"
        );

        let nav_interior = Rect::from_min_max(
            layout.nav_card.min + Vec2::splat(Style::SP_L),
            layout.nav_card.max - Vec2::splat(Style::SP_L),
        );
        let nav_surface_pixels =
            canvas.count_near_color_in_rect(nav_interior, Style::SYNC3_SURFACE, 1);
        let nav_interior_pixels = rect_pixels(nav_interior, &canvas);
        assert!(
            (nav_surface_pixels as f32) >= (nav_interior_pixels as f32 * 0.55),
            "Navigation card must rasterize as a SYNC3 surface, got {nav_surface_pixels}/{nav_interior_pixels}"
        );

        let accent_cap = Rect::from_min_max(
            layout.nav_card.min,
            pos2(
                layout.nav_card.right(),
                (layout.nav_card.top() + Style::SP_XS).min(layout.nav_card.bottom()),
            ),
        );
        let accent_pixels = canvas.count_near_color_in_rect(accent_cap, Style::SYNC3_ACCENT, 4);
        let accent_total = rect_pixels(accent_cap, &canvas);
        assert!(
            (accent_pixels as f32) >= (accent_total as f32 * 0.70),
            "Navigation card must retain its Ford-blue accent cap, got {accent_pixels}/{accent_total}"
        );

        let inset = Rect::from_min_max(
            layout.glance_card.min + Vec2::splat(Style::SP_M),
            layout.glance_card.max - Vec2::splat(Style::SP_M),
        );
        let half = inset.height() / 2.0;
        let vehicle_row = Rect::from_min_size(inset.min, vec2(inset.width(), half));
        let comms_row = Rect::from_min_size(
            pos2(inset.left(), inset.top() + half),
            vec2(inset.width(), half),
        );
        let vehicle_dim = canvas.count_near_color_in_rect(vehicle_row, Style::SYNC3_TEXT_DIM, 24);
        let vehicle_strong =
            canvas.count_near_color_in_rect(vehicle_row, Style::SYNC3_TEXT_STRONG, 24);
        assert!(
            vehicle_dim > 24 && vehicle_dim > vehicle_strong * 2,
            "stale MG90 vehicle text must rasterize dim, not live-strong ({vehicle_dim} dim vs {vehicle_strong} strong pixels)"
        );

        let comms_strong = canvas.count_near_color_in_rect(comms_row, Style::SYNC3_TEXT_STRONG, 24);
        assert!(
            comms_strong > 24,
            "live alert count must rasterize as strong Car text, got {comms_strong} pixels"
        );
    }

    #[test]
    fn dashboard_renders_an_honest_notice_when_touch_layout_does_not_fit() {
        let (picks, shapes) =
            drive_with_screen(&CarHomeGlance::default(), vec![vec![]], vec2(420.0, 220.0));
        assert_eq!(
            picks,
            vec![None],
            "a too-small workspace has no active target"
        );
        let texts = painted_text(&shapes);
        assert!(
            texts
                .iter()
                .any(|text| text == "Resize workspace to use Auto Mode"),
            "small workspaces explain their degraded state: {texts:?}"
        );
    }

    #[test]
    fn narrow_strip_labels_stay_inside_tiles_without_losing_accessible_names() {
        let minimum = Density::Touch.min_hit_target();
        assert_eq!(CarTile::Nav.strip_label(minimum), "Nav");
        assert_eq!(CarTile::Settings.strip_label(minimum), "Set");
        assert_eq!(CarTile::Nav.label(), "Navigation");
        assert_eq!(CarTile::Settings.label(), "Settings");

        // 512px remains large enough for six 44pt targets, but is narrow
        // enough to exercise the intermediate visual-label branch.
        let (_, shapes) =
            drive_with_screen(&CarHomeGlance::default(), vec![vec![]], vec2(512.0, 640.0));
        let texts = painted_text(&shapes);
        assert!(
            texts.iter().any(|text| text == "Nav"),
            "compact Nav label in {texts:?}"
        );
        assert!(
            texts.iter().any(|text| text == "Settings"),
            "unshortened Settings label in {texts:?}"
        );
        // The card still exposes its full title; only the strip copy is
        // shortened, preserving an accessible route name in WidgetInfo.
        assert_eq!(texts.iter().filter(|text| *text == "Navigation").count(), 1);
    }

    #[test]
    fn dashboard_renders_a_populated_live_glance() {
        let glance = CarHomeGlance {
            nav: Some("12 min · 4.3 mi · ETA 14:32".to_string()),
            media: Some("Comfortably Numb · Pink Floyd".to_string()),
            comms: Some(3),
            vehicle: Some("38 mph".to_string()),
            vehicle_live: true,
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

        // Every strip target routes its own app; none of the six becomes an
        // unreachable decorative icon at the edge of the row.
        for (tile, rect) in CarTile::ALL.into_iter().zip(l.strip) {
            let (picks, _) = drive(&CarHomeGlance::default(), tap(rect.center()));
            assert_eq!(picks.last(), Some(&Some(tile)), "{tile:?} strip tap");
        }

        // The Nav card routes CarTile::Nav.
        let (picks, _) = drive(&CarHomeGlance::default(), tap(l.nav_card.center()));
        assert_eq!(picks.last(), Some(&Some(CarTile::Nav)));

        // The Media card remains independently reachable from its split card.
        let (picks, _) = drive(&CarHomeGlance::default(), tap(l.media_card.center()));
        assert_eq!(picks.last(), Some(&Some(CarTile::Media)));

        // The glance card routes CarTile::Vehicle (its telematics tab target).
        let (picks, _) = drive(&CarHomeGlance::default(), tap(l.glance_card.center()));
        assert_eq!(picks.last(), Some(&Some(CarTile::Vehicle)));
    }

    #[test]
    fn nav_card_remains_reachable_at_the_narrowest_safe_touch_layout() {
        // Derive the smallest body that the production layout accepts, then
        // account for the Auto title and the shell's inset. This is the
        // boundary most likely to regress into an unreachable or misrouted
        // large Navigation tile when a seat is narrow.
        let cols = CarTile::ALL.len() as f32;
        let touch_target = Density::Touch.min_hit_target();
        let gap = Style::SP_M;
        let body_size = vec2(
            touch_target * cols + gap * (cols - 1.0),
            (touch_target + Style::SP_XL) + gap + (touch_target * 2.0 + gap),
        );
        let screen_size = vec2(
            body_size.x + Style::SP_L * 2.0,
            body_size.y + Style::SP_L * 2.0 + Style::DISPLAY + Style::SP_M,
        );
        let body = Rect::from_min_size(
            pos2(Style::SP_L, Style::SP_L + Style::DISPLAY + Style::SP_M),
            body_size,
        );
        let layout = dashboard_layout(body).expect("exact 44pt-safe boundary lays out");
        assert!(layout.nav_card.width() >= touch_target);
        assert!(layout.nav_card.height() >= touch_target);

        let tap = |pos: egui::Pos2| {
            vec![
                vec![],
                vec![egui::Event::PointerMoved(pos), pointer_button(pos, true)],
                vec![pointer_button(pos, false)],
            ]
        };
        let (picks, _) = drive_with_screen(
            &CarHomeGlance {
                vehicle: Some("MG90 telematics".to_string()),
                ..Default::default()
            },
            tap(layout.nav_card.center()),
            screen_size,
        );
        assert_eq!(
            picks.last(),
            Some(&Some(CarTile::Nav)),
            "the large Nav card must not fall through to Vehicle/OBD at the safe narrow boundary"
        );

        // One point below either body dimension must fail closed rather than
        // expose a partial target or an off-body activation region.
        for undersized in [
            vec2(screen_size.x - 1.0, screen_size.y),
            vec2(screen_size.x, screen_size.y - 1.0),
        ] {
            let (picks, shapes) =
                drive_with_screen(&CarHomeGlance::default(), vec![vec![]], undersized);
            assert_eq!(
                picks,
                vec![None],
                "unsupported seat size has no active target"
            );
            assert!(painted_text(&shapes)
                .iter()
                .any(|text| text == "Resize workspace to use Auto Mode"));
        }
    }

    #[test]
    fn focused_car_targets_activate_with_enter() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let glance = CarHomeGlance::default();
        let mut picks = Vec::new();

        let mut frame = |events: Vec<egui::Event>| {
            let out = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1024.0, 640.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ctx, |ui| picks.push(car_home_panel(ui, &glance)));
                },
            );
            assert!(!ctx.tessellate(out.shapes, out.pixels_per_point).is_empty());
        };

        // Register the hand-painted controls before requesting focus by their
        // stable IDs, matching the shell's keyboard-navigation lifecycle.
        frame(vec![]);
        ctx.memory_mut(|memory| {
            memory.request_focus(egui::Id::new(("car-home-app", "Navigation")))
        });
        frame(vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]);

        assert_eq!(picks, vec![None, Some(CarTile::Nav)]);
    }
}
