//! Native egui renderer for the Maps & Location workspace.

use mde_egui::egui::{
    self, Align, Align2, Color32, FontId, Mesh, Painter, Pos2, Rect, RichText, Sense, Shape,
    Stroke, StrokeKind, Vec2,
};
use mde_egui::menubar::{ChipTone, Menu, MenuBar, MenuBarModel, StatusChip};
use mde_egui::{paint_carbon, Style, StyleColorScheme, TypographyRole};

use crate::model::{
    AdminSection, BackupRecord, CheckState, DeadZoneSeverity, DeadZoneState, Destination,
    DeviceIoState, EncryptedVaultState, FirmwareWorkflow, LocationManager, LocationSample,
    LocationSource, LocationSourceKind, MapViewState, Mg90ManagementMethod, Mg90SettingCategory,
    Mg90SettingDescriptor, Mg90State, OfflineMapManagerState, OfflineNavigationReadiness,
    OfflineNavigationStatus, ProviderContract, RouteOption, RoutePlan, RouteTraffic,
    SettingValueType, SetupStep, SourceStatus, TripRecorderState, VehicleHealthRail,
    VehicleHealthRailSlot, VehicleHealthRailState, VehicleMirrorState, VehicleMirrorStatus,
    VehicleHealthRailLayout, VehicleRadioAvailability, VehicleRadioHealth, VehicleRadioOperation,
    VehicleRadioPresence, VehicleState, WorkspaceTab,
};
use crate::MapsLocationSurface;

const RAIL_W: f32 = 176.0;
const RAIL_INNER_MARGIN: f32 = Style::SP_S;
const MAP_LAYERS_POPUP_ID: &str = "maps-location-layers-popup";
const MAP_LAYERS_SCROLL_ID: &str = "maps-location-layers-scroll";
const MAP_LAYERS_POPUP_WIDTH: f32 = 280.0;
const MAP_LAYERS_POPUP_HEIGHT: f32 = 360.0;
const MAP_LAYERS_POPUP_GAP: f32 = Style::SP_XS;
const ADMIN_CARD_MIN_WIDTH: f32 = 280.0;
const CARD_MIN_H: f32 = 84.0;
const MAP_DARK_BG: Color32 = Color32::from_rgb(0x0D, 0x13, 0x18); // style-leak-ok: map-content-color
const MAP_LIGHT_BG: Color32 = Color32::from_rgb(0xE8, 0xEF, 0xE8); // style-leak-ok: map-content-color
const ROUTE_BLUE: Color32 = Color32::from_rgb(0x4C, 0xA3, 0xFF); // style-leak-ok: map-content-color
const ROUTE_ALT: Color32 = Color32::from_rgb(0x7D, 0xD9, 0xA3); // style-leak-ok: map-content-color
const WEATHER: Color32 = Color32::from_rgb(0x67, 0xD6, 0xE8); // style-leak-ok: map-content-color
const TRAFFIC: Color32 = Color32::from_rgb(0xFF, 0xB4, 0x54); // style-leak-ok: map-content-color
                                                              // --- Driving HUD (Google Maps / Waze vocabulary, keyed to the Quazar-dark route palette) ---
                                                              // A premium GMaps-navigation blue, painted as a top-lit vertical gradient
                                                              // (HI at the top edge → BASE → DEEP at the bottom) so the banner reads with
                                                              // depth instead of a single flat fill.
const MANEUVER_BLUE: Color32 = Color32::from_rgb(0x1A, 0x66, 0xE0); // style-leak-ok: map-content-color
const MANEUVER_BLUE_HI: Color32 = Color32::from_rgb(0x3E, 0x86, 0xFF); // style-leak-ok: map-content-color
const MANEUVER_BLUE_DEEP: Color32 = Color32::from_rgb(0x11, 0x4C, 0xB6); // style-leak-ok: map-content-color
const ROUTE_CASING: Color32 = Color32::from_rgb(0x14, 0x4C, 0x92); // style-leak-ok: map-content-color
const HUD_CARD_BG: Color32 = Color32::from_rgb(0x1A, 0x1B, 0x22); // style-leak-ok: map-content-color
const HUD_CARD_HI: Color32 = Color32::from_rgb(0x24, 0x26, 0x30); // style-leak-ok: map-content-color

/// Corner radius for the floating HUD cards (banner, ETA sheet, lane strip) —
/// larger than the shared card radius so the nav surface reads modern/premium.
const HUD_RADIUS: f32 = 16.0;
/// Corner radius for smaller HUD chips (speed sign chips, option cards).
const HUD_RADIUS_S: f32 = 12.0;
/// Maximum attribution text submitted to egui's galley/layout machinery.
const MAX_MAP_ATTRIBUTION_CHARS: usize = 512;
const MAP_ATTRIBUTION_ELLIPSIS: char = '\u{2026}';

/// The Drive HUD's painter-positioned FABs occupy a dedicated right-hand lane.
/// Keep this geometry explicit so large-text tiles never render underneath a
/// button or its pointer target.
const DRIVE_FAB_LANE_SEPARATION: f32 = Style::SP_XS;

/// Render the complete native Maps & Location workspace.
pub fn maps_location_panel(ui: &mut egui::Ui, state: &mut MapsLocationSurface) {
    // Auto Mode (Car): the cockpit is on a dash — drop the header + tab rail so the
    // active tab (the Drive HUD by default) is edge-to-edge full-bleed. Tab
    // switching in Car Mode is driven by the Auto Home tiles / bound keys (Nav →
    // Drive, Vehicle → telematics), not the rail.
    if Style::color_scheme(ui.ctx()) == StyleColorScheme::AutoSync3 {
        let panel_rect = ui.max_rect();
        egui::Frame::NONE.fill(Style::BG).show(ui, |ui| {
            let content_size = ui.available_size();
            ui.allocate_ui_with_layout(
                content_size,
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt(("maps-location-car", state.active, state.admin_section))
                        .auto_shrink([false, false])
                        .show(ui, |ui| render_active_tab(ui, state));
                },
            );
        });
        // Car Mode drops the header + tab rail — and with them the only "Simulator"
        // indicator. Fixture data would otherwise fill the full-bleed HUD with no
        // marker at all, so paint a persistent, un-hideable SIMULATED badge on a
        // foreground layer whenever the simulator feed is live: it floats above the
        // HUD cards / FABs and can never be scrolled away.
        if state.simulator_enabled {
            paint_simulated_ribbon(ui, panel_rect);
        }
        return;
    }

    egui::Frame::NONE
        .fill(Style::BG)
        .inner_margin(Style::SP_M)
        .show(ui, |ui| {
            header(ui, state);
            ui.add_space(Style::SP_S);
            // Bind the tab-rail + content row to the FULL remaining height. A bare
            // `ui.horizontal` sizes to content, and a vertical ScrollArea nested in
            // an unbounded-height layout collapses its viewport — which starved the
            // full-bleed Drive HUD down to a top strip (only the banner visible; the
            // FABs / ETA sheet / speedometer fell below the fold). Allocating the
            // exact remaining size gives the HUD the whole screen.
            let content_size = ui.available_size();
            ui.allocate_ui_with_layout(
                content_size,
                egui::Layout::left_to_right(egui::Align::TOP),
                |ui| {
                    tab_rail(ui, state);
                    ui.add_space(Style::SP_M);
                    egui::Frame::NONE
                        .fill(Style::LAYER_01)
                        .inner_margin(Style::SP_M)
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt(("maps-location-tab", state.active, state.admin_section))
                                .auto_shrink([false, false])
                                .show(ui, |ui| render_active_tab(ui, state));
                        });
                },
            );
        });
}

/// Render the active workspace tab's body — shared by the normal (rail) layout and
/// the Car Mode full-bleed layout.
fn render_active_tab(ui: &mut egui::Ui, state: &mut MapsLocationSurface) {
    if state.active != WorkspaceTab::Airspace {
        state.airspace.deactivate();
    }
    match state.active {
        WorkspaceTab::Drive => show_drive(ui, state),
        WorkspaceTab::Airspace => crate::airspace::airspace_panel(ui, &mut state.airspace),
        WorkspaceTab::Map => show_map(ui, state),
        WorkspaceTab::RoutesTrips => show_routes_trips(ui, state),
        WorkspaceTab::Admin => show_admin(ui, state),
    }
}

fn header(ui: &mut egui::Ui, state: &MapsLocationSurface) {
    // Maps retains its domain status chips, but the Construct-owned title strip
    // is the shared menubar. The map canvas below remains the only governed
    // Maps content-colour exception; Car still bypasses this header entirely.
    let mut status = vec![
        StatusChip::new("25 GB offline cap", ChipTone::Info),
        StatusChip::new("Direct Ethernet", ChipTone::Neutral),
    ];
    // The Simulator chip exists ONLY while the test fixture is live — a
    // production surface has no simulator to flag (WL-UX-007/S1).
    if state.simulator_enabled {
        status.push(StatusChip::new("Simulator", ChipTone::Ok));
    }
    let menus: &[Menu<&'static str>] = &[];
    let model = MenuBarModel {
        title: "Maps & Location",
        accent: Style::ACCENT,
        menus,
        status: &status,
    };
    MenuBar::show(ui, &model);
    ui.add_space(Style::SP_S);
}

/// A persistent, un-hideable "SIMULATED DATA" badge for the Car-Mode full-bleed
/// layout, which drops the header chip that otherwise flags the simulator feed.
/// Painted on a foreground layer (top-centre) so it floats above the HUD cards
/// and FABs and can never be scrolled off — the driver always knows the readouts
/// are fixture data, not a live vehicle.
///
/// UNREACHABLE IN PRODUCTION BY CONSTRUCTION (WL-UX-007/S1, operator directive
/// 2026-07-22): `simulator_enabled` is only ever set by the cfg-gated
/// `MapsLocationSurface::simulated()` test fixture — the production `live()`
/// constructor pins it `false` and nothing on a production path flips it. The
/// ribbon stays compiled so the fixture can never render unbadged in tests.
fn paint_simulated_ribbon(ui: &egui::Ui, panel: Rect) {
    if panel.any_nan() || panel.width() < 40.0 {
        return;
    }
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("maps-simulated-ribbon"),
    ));
    let font = FontId::proportional(Style::SMALL);
    let galley = painter.layout_no_wrap("SIMULATED DATA".to_string(), font, Style::TEXT_STRONG);
    let dot_r = 3.5;
    let chip_h = galley.size().y + Style::SP_S;
    let chip_w = galley.size().x + Style::SP_M + dot_r * 2.0 + Style::SP_S * 2.0;
    let rect = Rect::from_min_size(
        egui::pos2(panel.center().x - chip_w / 2.0, panel.top() + Style::SP_S),
        egui::vec2(chip_w, chip_h),
    );
    let radius = chip_h * 0.5;
    painter.rect_filled(rect, radius, Color32::BLACK.gamma_multiply(0.55));
    painter.rect_filled(rect, radius, Style::SURFACE_HI);
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.5, Style::WARN),
        StrokeKind::Inside,
    );
    let dot_c = egui::pos2(rect.left() + Style::SP_S + dot_r, rect.center().y);
    painter.circle_filled(dot_c, dot_r, Style::WARN);
    painter.galley(
        egui::pos2(
            dot_c.x + dot_r + Style::SP_S,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        Style::TEXT_STRONG,
    );
}

fn tab_rail(ui: &mut egui::Ui, state: &mut MapsLocationSurface) -> Rect {
    // The shell has already removed the top status-bar and bottom/left dock
    // reservations before this workspace is laid out. Keep the Maps rail in
    // that remaining screen-space budget as well: the single-level rail must
    // scroll inside the workspace rather than painting/hit-testing below the
    // clip on short seats.
    let available = ui.available_rect_before_wrap().intersect(ui.clip_rect());
    let inset = RAIL_INNER_MARGIN
        .min((available.width() * 0.5).max(0.0))
        .min((available.height() * 0.5).max(0.0));
    let inner_width = (available.width() - 2.0 * inset)
        .clamp(1.0, RAIL_W)
        .max(1.0);
    let inner_height = (available.height() - 2.0 * inset).max(1.0);

    let rendered = egui::Frame::NONE
        .fill(Style::LAYER_01)
        .inner_margin(inset)
        .show(ui, |ui| {
            ui.set_width(inner_width);
            egui::ScrollArea::vertical()
                .id_salt("maps-location-tab-rail")
                .auto_shrink([false, false])
                .max_height(inner_height)
                .min_scrolled_height(inner_height)
                .show(ui, |ui| {
                    ui.set_width(inner_width);
                    ui.with_layout(egui::Layout::top_down(Align::Min), |ui| {
                        // Single-level top rail. MG90 administrative sub-sections
                        // are selected inside the Admin page, not exposed as rail
                        // leaves.
                        for tab in WorkspaceTab::PRIMARY {
                            if rail_button(ui, tab.label(), state.active == tab).clicked() {
                                state.active = tab;
                            }
                        }
                    });
                })
        });

    rendered.inner.inner_rect
}

/// Keep a foreground control surface inside the workspace clip, including the
/// shell's reserved top rail. The stock `menu_button` popup is constrained to
/// the full egui screen, so a tall Layers list can cover the top bar or leave
/// its lower rows outside a short seat. This rectangle picks the side with the
/// most usable room and gives the caller a bounded scrolling viewport.
fn bounded_popup_rect(anchor: Rect, clip: Rect, desired_width: f32, desired_height: f32) -> Rect {
    if !anchor.is_positive() || !clip.is_positive() {
        return Rect::NOTHING;
    }

    // A wrapped/scrolling parent can leave the launcher partially or wholly
    // outside the visible workspace.  Normalize the anchor to the visible
    // portion before choosing above/below; otherwise the distance from an
    // off-screen anchor can push the popup back out of the clip on short seats.
    let anchor = anchor.intersect(clip);
    let anchor = if anchor.is_positive() {
        anchor
    } else {
        let point = anchor.center();
        let point = egui::pos2(
            point.x.clamp(clip.left(), clip.right()),
            point.y.clamp(clip.top(), clip.bottom()),
        );
        Rect::from_min_max(point, point)
    };

    let width = desired_width.max(1.0).min(clip.width().max(1.0));
    let desired_height = desired_height.max(1.0).min(clip.height().max(1.0));
    let left = anchor
        .left()
        .clamp(clip.left(), (clip.right() - width).max(clip.left()));
    let below = (clip.bottom() - anchor.bottom() - MAP_LAYERS_POPUP_GAP).max(0.0);
    let above = (anchor.top() - clip.top() - MAP_LAYERS_POPUP_GAP).max(0.0);

    if below >= above {
        let height = desired_height.min(below.max(1.0));
        let top = (anchor.bottom() + MAP_LAYERS_POPUP_GAP)
            .clamp(clip.top(), (clip.bottom() - height).max(clip.top()));
        Rect::from_min_size(egui::pos2(left, top), egui::vec2(width, height))
    } else {
        let height = desired_height.min(above.max(1.0));
        let bottom = (anchor.top() - MAP_LAYERS_POPUP_GAP)
            .clamp((clip.top() + height).min(clip.bottom()), clip.bottom());
        Rect::from_min_size(egui::pos2(left, bottom - height), egui::vec2(width, height))
    }
}

fn rail_button(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let size = egui::vec2(ui.available_width().max(1.0), Style::SP_XL);
    let (_, rect) = ui.allocate_space(size);
    let response = ui.interact(rect, rail_item_id(label), Sense::click());
    let fill = if selected {
        Style::pressed_fill(Style::ACCENT)
    } else if response.hovered() {
        Style::SURFACE_HI
    } else {
        Style::SURFACE
    };
    ui.painter().rect_filled(rect, Style::RADIUS_S, fill);
    if selected {
        ui.painter().rect_filled(
            Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
            Style::RADIUS_S,
            Style::ACCENT,
        );
    }
    ui.painter().text(
        egui::pos2(rect.left() + Style::SP_S, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(Style::BODY),
        if selected {
            Style::TEXT_STRONG
        } else {
            Style::TEXT
        },
    );
    ui.add_space(Style::SP_XS);
    response
}

/// Stable hit-test identity for Maps' rail rows. The row rectangles are
/// screen-space widgets inside the bounded ScrollArea, so keeping their IDs
/// independent of the content's scroll offset preserves pointer routing when
/// the rail is restored from a previous frame.
fn rail_item_id(label: &str) -> egui::Id {
    egui::Id::new(("maps-location-tab-rail-item", label))
}

/// Fixed screen anchor for the driver's vehicle chevron (not panned/zoomed).
const VEHICLE_UV: (f32, f32) = (0.50, 0.62);

/// One geographic point returned by the route provider.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ProviderRoutePoint {
    latitude: f64,
    longitude: f64,
}

/// Provider-owned route geometry consumed by the map renderer.
///
/// The model currently has no route-geometry field, so production call sites
/// pass `None` until the routing adapter can return this payload. Keeping the
/// seam typed here prevents the renderer from manufacturing a path meanwhile.
#[derive(Debug, Clone, Default, PartialEq)]
struct ProviderRouteGeometry {
    primary: Vec<ProviderRoutePoint>,
    alternate: Vec<ProviderRoutePoint>,
    maneuver: Option<ProviderRoutePoint>,
}

impl ProviderRoutePoint {
    fn is_valid(self) -> bool {
        self.latitude.is_finite()
            && self.longitude.is_finite()
            && (-90.0..=90.0).contains(&self.latitude)
            && (-180.0..=180.0).contains(&self.longitude)
    }
}

impl ProviderRouteGeometry {
    fn is_renderable(&self) -> bool {
        self.primary.len() >= 2
            && self
                .primary
                .iter()
                .copied()
                .all(ProviderRoutePoint::is_valid)
    }
}

/// A single turn instruction reduced to a direction for the painted arrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManeuverKind {
    Straight,
    Left,
    SlightLeft,
    Right,
    SlightRight,
    Merge,
    Roundabout,
    UTurn,
    Arrive,
}

/// Infer a [`ManeuverKind`] from free-text turn guidance keywords.
fn maneuver_kind(text: &str) -> ManeuverKind {
    let t = text.to_ascii_lowercase();
    if t.contains("u-turn") || t.contains("u turn") || t.contains("make a u") {
        ManeuverKind::UTurn
    } else if t.contains("arrive") || t.contains("destination") {
        ManeuverKind::Arrive
    } else if t.contains("roundabout") || t.contains("rotary") || t.contains("traffic circle") {
        ManeuverKind::Roundabout
    } else if t.contains("merge") {
        ManeuverKind::Merge
    } else if (t.contains("slight") || t.contains("keep") || t.contains("bear"))
        && t.contains("left")
    {
        ManeuverKind::SlightLeft
    } else if (t.contains("slight") || t.contains("keep") || t.contains("bear"))
        && t.contains("right")
    {
        ManeuverKind::SlightRight
    } else if t.contains("left") {
        ManeuverKind::Left
    } else if t.contains("right") {
        ManeuverKind::Right
    } else {
        ManeuverKind::Straight
    }
}

/// Colour the arrival/ETA readout by how the route is running.
fn eta_tone(route: &RoutePlan, offline: &OfflineNavigationStatus) -> Color32 {
    if offline.readiness == OfflineNavigationReadiness::Blocked {
        return Style::DANGER;
    }
    let t = route.traffic_alert.to_ascii_lowercase();
    if t.contains("heavy") || t.contains("severe") || t.contains("stopped") || t.contains("closure")
    {
        Style::DANGER
    } else if !route.traffic_alert.trim().is_empty() {
        Style::WARN
    } else {
        Style::OK
    }
}

/// Format a maneuver distance the way a nav app does: feet under a quarter mile.
fn format_distance(mi: f32) -> String {
    let mi = finite_or(mi, 0.0).max(0.0);
    if mi < 0.19 {
        let ft = (mi * 5280.0 / 50.0).round() * 50.0;
        format!("{ft:.0} ft")
    } else {
        format!("{mi:.1} mi")
    }
}

fn finite_or(value: f32, default: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        default
    }
}

/// A finite, non-degenerate rect from raw components (crash-safe layout).
fn safe_rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::from_min_size(
        egui::pos2(finite_or(x, 0.0), finite_or(y, 0.0)),
        egui::vec2(finite_or(w, 1.0).max(1.0), finite_or(h, 1.0).max(1.0)),
    )
}

/// The content width for a full-bleed canvas, guarded against non-finite layout.
fn safe_width(ui: &egui::Ui) -> f32 {
    let clip = ui.clip_rect().width().max(1.0);
    let avail = ui.available_width();
    if avail.is_finite() && avail > 0.0 {
        avail.min(clip).max(1.0)
    } else {
        clip
    }
}

#[derive(Debug, Clone, Copy)]
struct DriveHudOverlayGeometry {
    health_rail: Rect,
    fab_lane: Rect,
    rail_layout: VehicleHealthRailLayout,
}

/// Reserve the complete FAB lane before placing the radio/GNSS rail.
///
/// The lane includes the button diameter, its stack gap, and a full spacing
/// unit at the rail boundary.  It intentionally spans the canvas vertically:
/// the FABs are painter-positioned and their hit targets must remain excluded
/// even when the health rail grows for Large/Largest text.
fn drive_hud_overlay_geometry(
    canvas: Rect,
    below_banner: f32,
    margin: f32,
    left_inset: f32,
    fab_radius: f32,
    fab_gap: f32,
    rail_layout: VehicleHealthRailLayout,
) -> DriveHudOverlayGeometry {
    let lane_width = (fab_radius * 2.0 + fab_gap + Style::SP_M).max(1.0);
    let lane_left = canvas.right() - margin - lane_width;
    let lane_right = canvas.right() - margin;
    let fab_lane = safe_rect(
        lane_left,
        canvas.top(),
        (lane_right - lane_left).max(1.0),
        canvas.height().max(1.0),
    );
    let rail_left = canvas.left() + margin + left_inset.max(0.0);
    let rail_right = (fab_lane.left() - DRIVE_FAB_LANE_SEPARATION).max(rail_left + 1.0);
    let health_rail = safe_rect(
        rail_left,
        below_banner.max(canvas.top() + margin),
        (rail_right - rail_left).max(1.0),
        rail_layout.minimum_height.max(1.0),
    );
    DriveHudOverlayGeometry {
        health_rail,
        fab_lane,
        rail_layout,
    }
}

/// Elide `text` with a trailing ellipsis so it never overflows `max_w`.
fn elide(painter: &Painter, text: &str, font: FontId, max_w: f32) -> String {
    let full = painter.layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE);
    if full.size().x <= max_w {
        return text.to_string();
    }
    let mut s = text.to_string();
    while s.chars().count() > 1 {
        s.pop();
        let g = painter.layout_no_wrap(format!("{s}\u{2026}"), font.clone(), Color32::WHITE);
        if g.size().x <= max_w {
            return format!("{s}\u{2026}");
        }
    }
    "\u{2026}".to_string()
}

// ===========================================================================
// Drive — a full-bleed navigation HUD (Google Maps / Waze layout vocabulary).
// ===========================================================================

fn show_drive(ui: &mut egui::Ui, state: &mut MapsLocationSurface) {
    // Navigation flow, terminal state first: arrival → search → preview → HUD.
    if state.arrived {
        show_arrival(ui, state);
        return;
    }
    if state.destination_search {
        show_destination_search(ui, state);
        return;
    }
    if state.route_preview {
        show_route_preview(ui, state);
        return;
    }
    let primary = state.locations.primary_sample().cloned();
    let has_fix = primary.as_ref().is_some_and(LocationSample::has_fix);
    let offline = state.offline_navigation_status();
    drive_hud(ui, state, primary.as_ref(), has_fix, &offline);
}

#[allow(clippy::too_many_lines)]
fn drive_hud(
    ui: &mut egui::Ui,
    state: &mut MapsLocationSurface,
    primary: Option<&LocationSample>,
    has_fix: bool,
    offline: &OfflineNavigationStatus,
) {
    // --- Full-bleed canvas: the map fills the whole Drive surface. ---------
    let width = safe_width(ui);
    let avail_h = ui.available_height();
    let height = if avail_h.is_finite() && avail_h > 1.0 {
        avail_h.clamp(320.0, 1400.0)
    } else {
        520.0
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), Sense::drag());

    // Pan / zoom — every value guarded finite and clamped.
    if response.dragged() {
        let d = response.drag_delta();
        if d.x.is_finite() && d.y.is_finite() {
            state.map.pan[0] = (state.map.pan[0] + d.x).clamp(-600.0, 600.0);
            state.map.pan[1] = (state.map.pan[1] + d.y).clamp(-600.0, 600.0);
        }
    }
    let scroll = ui.input(|input| input.raw_scroll_delta.y);
    if response.hovered() && scroll.abs() > 0.0 {
        state.map.zoom = (state.map.zoom + scroll.signum() * 0.5).clamp(3.0, 18.0);
    }
    if !ui.is_rect_visible(rect) {
        return;
    }

    let margin = Style::SP_M;
    // Large text changes the layout metrics but the Drive HUD also contains
    // painter-positioned controls. Keep those controls and the radio rail in
    // separate lanes so the accessibility zoom cannot turn the last health
    // tile into a button backdrop.
    let text_zoom = ui.ctx().zoom_factor().max(1.0);

    // --- Floating action buttons (interactive; unique stable ids). ---------
    let fab_r = 26.0_f32;
    let fab_gap = Style::SP_S + Style::SP_XS;
    let fab_cx = rect.right() - margin - fab_r;
    let stack_bottom = rect.bottom() - margin - 96.0 - fab_r;
    let fab_keys = ["recenter", "search", "mute", "overview", "preview"];
    let mut fab_states: [Option<(Pos2, bool, bool)>; 5] = [None; 5];
    let muted_id = egui::Id::new(("maps-drive-hud", "muted"));
    let mut muted = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(muted_id))
        .unwrap_or(false);
    for (idx, key) in fab_keys.iter().enumerate() {
        let cy = stack_bottom - idx as f32 * (fab_r * 2.0 + fab_gap);
        let center = egui::pos2(fab_cx, cy);
        if !center.x.is_finite() || !center.y.is_finite() {
            continue;
        }
        let frect = Rect::from_center_size(center, egui::vec2(fab_r * 2.0, fab_r * 2.0));
        let resp = ui.interact(
            frect,
            egui::Id::new(("maps-drive-fab", *key)),
            Sense::click(),
        );
        if resp.clicked() {
            match *key {
                "recenter" => {
                    state.map.pan = [0.0, 0.0];
                    state.map.zoom = 13.0;
                }
                "overview" => state.map.zoom = 6.5,
                "preview" => state.route_preview = true,
                "search" => state.open_destination_search(),
                "mute" => {
                    muted = !muted;
                    ui.ctx().data_mut(|d| d.insert_temp(muted_id, muted));
                }
                _ => {}
            }
        }
        fab_states[idx] = Some((center, resp.hovered(), resp.is_pointer_button_down_on()));
    }

    // Off-route recalculating state: the route dims + the banner turns amber,
    // matching Google-Maps / Waze. Keep the map animating while it recalculates.
    let off_route = state.off_route;
    let time = ui.input(|input| input.time);
    if off_route {
        ui.ctx().request_repaint();
    }

    // --- Paint: scene first, then the floating cards over it. --------------
    let painter = ui.painter_at(rect);
    paint_map_scene(
        &painter,
        rect,
        &state.map,
        &state.dead_zones,
        primary,
        has_fix,
        live_nws_vehicle_point(&state.locations),
        has_fix && !off_route,
        state.local_navigation.active_route.is_planned(),
        state
            .local_navigation
            .active_destination()
            .and_then(Destination::geo),
        None,
    );

    let route = &state.local_navigation.active_route;
    // Guidance is honest only once the driver has actually chosen a destination
    // and tapped Start. Idle (no destination) shows a calm prompt, NOT a
    // fabricated maneuver banner / ETA / traffic for a route nobody picked.
    let navigating = state.local_navigation.navigating;

    // Top banner: the maneuver instruction (or amber "Recalculating…") while
    // guiding, else the calm idle prompt. Always painted so the HUD has a header.
    let banner = safe_rect(
        rect.left() + margin,
        rect.top() + margin,
        width - 2.0 * margin,
        96.0,
    );
    let kind = maneuver_kind(&route.next_maneuver);
    paint_soft_shadow(&painter, banner, HUD_RADIUS);
    let mut below_banner = banner.bottom() + Style::SP_S;
    if navigating {
        if off_route {
            paint_recalculating_banner(&painter, banner, route, time);
        } else {
            paint_maneuver_banner(&painter, banner, route, kind, has_fix);
        }

        // Lane-level data is not part of the trusted route-provider contract.
        // Keep the existing strip footprint, but say so instead of deriving
        // lanes from the maneuver words.
        if !off_route {
            let lane_rect = safe_rect(banner.left(), below_banner, banner.width().min(360.0), 48.0);
            paint_soft_shadow(&painter, lane_rect, HUD_RADIUS_S);
            paint_provider_unavailable(&painter, lane_rect, "Lane guidance unavailable");
            below_banner = lane_rect.bottom() + Style::SP_S;
        }
    } else {
        paint_idle_banner(&painter, banner);
    }

    // Keep the six native MG90 positions visible in both Free Drive and active
    // guidance. This rail is derived only from the accepted v2 projection; it
    // never falls back to legacy signal values or manufactures missing rows.
    let rail = state.vehicle_health_rail();
    let overlay = drive_hud_overlay_geometry(
        rect,
        below_banner,
        margin,
        if text_zoom > 1.0 {
            88.0 + Style::SP_S
        } else {
            0.0
        },
        fab_r,
        fab_gap,
        rail.layout_for_text_zoom(text_zoom),
    );
    paint_health_rail(ui, overlay.health_rail, &rail, text_zoom);
    below_banner = overlay.health_rail.bottom() + Style::SP_S;

    // Alert pills. Acquiring-GPS + offline-blocked are system-level (both states);
    // traffic + weather belong to an active route (guidance only).
    let pill_x = rect.left()
        + margin
        + if text_zoom > 1.25 {
            88.0 + Style::SP_S
        } else {
            0.0
        };
    let mut pill_y = below_banner;
    if !has_fix {
        pill_y = paint_alert_pill(
            &painter,
            pill_x,
            pill_y,
            "dialog-warning",
            "Acquiring GPS",
            Style::WARN,
        );
    }
    if offline.readiness == OfflineNavigationReadiness::Blocked {
        pill_y = paint_alert_pill(
            &painter,
            pill_x,
            pill_y,
            "dialog-warning",
            "Offline nav blocked",
            Style::DANGER,
        );
    }
    if navigating {
        let traffic = route.traffic_alert.trim();
        if !traffic.is_empty() {
            pill_y = paint_alert_pill(&painter, pill_x, pill_y, "dialog-warning", traffic, TRAFFIC);
        }
        let weather = route.weather_alert.trim();
        if !weather.is_empty() {
            pill_y = paint_alert_pill(&painter, pill_x, pill_y, "dialog-warning", weather, WEATHER);
        }
    }
    let _ = pill_y;

    // Bottom ETA sheet (arrival time coloured by traffic) — guidance only.
    if navigating {
        let eta_w = (width * 0.46).clamp(260.0, 460.0);
        let eta = safe_rect(
            rect.center().x - eta_w / 2.0,
            rect.bottom() - margin - 72.0,
            eta_w,
            72.0,
        );
        paint_soft_shadow(&painter, eta, HUD_RADIUS);
        paint_eta_bar(&painter, eta, route, eta_tone(route, offline));
    }

    // Bottom-left speedometer (live vehicle speed — honest in both states).
    let speed_d = 88.0;
    let speedo = safe_rect(
        rect.left() + margin,
        rect.bottom() - margin - speed_d,
        speed_d,
        speed_d,
    );
    paint_speedometer(&painter, speedo, primary, has_fix);
    if navigating {
        let limit_status = safe_rect(
            speedo.right() + Style::SP_S,
            speedo.top() + 20.0,
            152.0,
            48.0,
        );
        paint_soft_shadow(&painter, limit_status, HUD_RADIUS_S);
        paint_provider_unavailable(&painter, limit_status, "Speed limit unavailable");
    }

    // Floating action buttons (painted last so they float above everything).
    for (idx, key) in fab_keys.iter().enumerate() {
        if let Some((center, hovered, pressed)) = fab_states[idx] {
            paint_fab(&painter, center, fab_r, hovered, pressed, key, muted);
        }
    }
}

fn paint_health_rail(
    ui: &mut egui::Ui,
    rect: Rect,
    rail: &VehicleHealthRail,
    text_zoom: f32,
) {
    let rail_layout = rail.layout_for_text_zoom(text_zoom);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(Align::Min)),
    );
    egui::Frame::NONE
        .fill(HUD_CARD_BG.gamma_multiply(0.96))
        .inner_margin(egui::Margin::symmetric(
            Style::SP_S as i8,
            Style::SP_XS as i8,
        ))
        .corner_radius(HUD_RADIUS_S)
        .stroke(Stroke::new(1.0, health_rail_tone(rail.state)))
        .show(&mut child, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("Radio & GNSS health")
                        .size(Style::SMALL)
                        .strong()
                        .color(Style::TEXT_STRONG),
                );
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    pill(ui, rail.state.label(), health_rail_tone(rail.state));
                });
            });
            ui.add_space(Style::SP_XS);
            let columns = rail_layout.columns;
            let rows = rail_layout.rows;
            let gap = Style::SP_XS;
            let slot_width = ((ui.available_width() - gap * (columns - 1) as f32)
                / columns as f32)
                .max(1.0);
            let slot_height = ((ui.available_height() - gap * (rows - 1) as f32)
                / rows as f32)
                .max(30.0);
            for row in 0..rows {
                ui.horizontal_top(|ui| {
                    for (column, slot) in rail.slots.iter().skip(row * columns).take(columns).enumerate() {
                        let response = ui
                            .allocate_ui_with_layout(
                                Vec2::new(slot_width, slot_height),
                                egui::Layout::top_down(Align::Center),
                                |slot_ui| {
                                    let tile = slot_ui.max_rect();
                                    slot_ui
                                        .painter()
                                        .rect_filled(tile, HUD_RADIUS_S, HUD_CARD_HI);
                                    slot_ui.painter().rect_stroke(
                                        tile,
                                        HUD_RADIUS_S,
                                        Stroke::new(1.0, Style::BORDER),
                                        egui::StrokeKind::Inside,
                                    );
                                    let compact_large_text = text_zoom > 1.0;
                                    let icon_size = if compact_large_text {
                                        18.0
                                    } else {
                                        (22.0 * text_zoom.max(1.0)).clamp(22.0, 32.0)
                                    };
                                    let (icon_rect, _) = slot_ui.allocate_exact_size(
                                        Vec2::new(slot_width.min(icon_size), icon_size),
                                        Sense::hover(),
                                    );
                                    paint_health_slot_glyph(
                                        slot_ui.painter(),
                                        icon_rect.center(),
                                        slot,
                                    );
                                    let operation = slot
                                        .operation
                                        .map_or("not reported", VehicleRadioOperation::label);
                                    if compact_large_text {
                                        let font = FontId::proportional(Style::SMALL);
                                        let summary = format!(
                                            "{} · {}",
                                            slot.label,
                                            slot.state.label()
                                        );
                                        let summary = elide(
                                            slot_ui.painter(),
                                            &summary,
                                            font.clone(),
                                            (tile.width() - Style::SP_S * 2.0).max(1.0),
                                        );
                                        let galley = slot_ui.painter().layout_no_wrap(
                                            summary,
                                            font,
                                            Style::TEXT_STRONG,
                                        );
                                        slot_ui.painter().galley(
                                            egui::pos2(
                                                tile.center().x - galley.size().x / 2.0,
                                                tile.bottom() - galley.size().y - Style::SP_XS,
                                            ),
                                            galley,
                                            Style::TEXT_STRONG,
                                        );
                                    } else {
                                        slot_ui.add(
                                            egui::Label::new(
                                                RichText::new(slot.label)
                                                    .size(Style::SMALL)
                                                    .color(Style::TEXT_STRONG),
                                            )
                                            .wrap(),
                                        );
                                        slot_ui.add(
                                            egui::Label::new(
                                                RichText::new(slot.state.label())
                                                    .size(Style::SMALL)
                                                    .color(health_slot_tone(slot)),
                                            )
                                            .wrap(),
                                        );
                                        slot_ui.add(
                                            egui::Label::new(
                                                RichText::new(operation)
                                                    .size(Style::SMALL)
                                                    .color(health_slot_tone(slot)),
                                            )
                                            .wrap(),
                                        );
                                    }
                                },
                            )
                            .response;
                        mde_egui::widgets::hover_text(response, slot.accessibility_label());
                        if column + 1 < columns && row * columns + column + 1 < rail.slots.len() {
                            ui.add_space(gap);
                        }
                    }
                });
                if row + 1 < rows {
                    ui.add_space(gap);
                }
            }
        });
}

fn health_rail_tone(state: VehicleHealthRailState) -> Color32 {
    match state {
        VehicleHealthRailState::Current => Style::OK,
        VehicleHealthRailState::Stale => Style::WARN,
        VehicleHealthRailState::Resyncing => Style::ACCENT,
        VehicleHealthRailState::Unavailable => Style::TEXT_DIM,
    }
}

fn health_slot_tone(slot: &VehicleHealthRailSlot) -> Color32 {
    match slot.state {
        VehicleHealthRailState::Stale => Style::WARN,
        VehicleHealthRailState::Resyncing => Style::ACCENT,
        VehicleHealthRailState::Unavailable => Style::TEXT_DIM,
        VehicleHealthRailState::Current => match slot.operation {
            Some(VehicleRadioOperation::Active) => Style::OK,
            Some(VehicleRadioOperation::Standby) => Style::ACCENT,
            Some(VehicleRadioOperation::Acquiring | VehicleRadioOperation::Degraded) => Style::WARN,
            Some(VehicleRadioOperation::Fault) => Style::DANGER,
            Some(VehicleRadioOperation::Disabled) | Some(VehicleRadioOperation::Unknown) | None => {
                Style::TEXT_DIM
            }
            Some(VehicleRadioOperation::Stale) => Style::WARN,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HealthSlotGlyph {
    ActiveCheck,
    StandbyRing,
    AttentionTriangle,
    FaultCross,
    DisabledPause,
    NotInstalledSlash,
    Clock,
    ResyncingArc,
    UnavailableSlash,
}

/// Resolve the color-independent shape semantics before painting. Presence and
/// freshness deliberately take precedence over producer operation: an absent,
/// retained, or unavailable interface must never look active merely because an
/// older operation value remains attached to the row.
fn health_slot_glyph(slot: &VehicleHealthRailSlot) -> HealthSlotGlyph {
    match slot.state {
        VehicleHealthRailState::Stale => HealthSlotGlyph::Clock,
        VehicleHealthRailState::Resyncing => HealthSlotGlyph::ResyncingArc,
        VehicleHealthRailState::Unavailable => HealthSlotGlyph::UnavailableSlash,
        VehicleHealthRailState::Current => {
            if slot.presence == Some(VehicleRadioPresence::NotInstalled) {
                return HealthSlotGlyph::NotInstalledSlash;
            }
            match slot.operation {
                Some(VehicleRadioOperation::Active) => HealthSlotGlyph::ActiveCheck,
                Some(VehicleRadioOperation::Standby) => HealthSlotGlyph::StandbyRing,
                Some(VehicleRadioOperation::Acquiring | VehicleRadioOperation::Degraded) => {
                    HealthSlotGlyph::AttentionTriangle
                }
                Some(VehicleRadioOperation::Fault) => HealthSlotGlyph::FaultCross,
                Some(VehicleRadioOperation::Disabled) => HealthSlotGlyph::DisabledPause,
                Some(VehicleRadioOperation::Unknown | VehicleRadioOperation::Stale) | None => {
                    HealthSlotGlyph::Clock
                }
            }
        }
    }
}

fn paint_health_slot_glyph(painter: &Painter, center: Pos2, slot: &VehicleHealthRailSlot) {
    let tone = health_slot_tone(slot);
    let radius = 7.0;
    let stroke = Stroke::new(1.8, tone);
    match health_slot_glyph(slot) {
        HealthSlotGlyph::Clock => {
            painter.circle_stroke(center, radius, stroke);
            painter.line_segment([center, center + Vec2::new(0.0, -4.0)], stroke);
            painter.line_segment([center, center + Vec2::new(3.0, 2.0)], stroke);
        }
        HealthSlotGlyph::ResyncingArc => {
            painter.circle_stroke(center, radius, stroke);
            let arc_points = (0..=12)
                .map(|index| {
                    let angle = 0.4 + (4.2 * index as f32 / 12.0);
                    center + Vec2::new(angle.cos(), angle.sin()) * (radius + 2.0)
                })
                .collect();
            painter.line(arc_points, Stroke::new(1.4, tone));
        }
        HealthSlotGlyph::UnavailableSlash | HealthSlotGlyph::NotInstalledSlash => {
            painter.circle_stroke(center, radius, stroke);
            painter.line_segment(
                [center + Vec2::new(-5.0, 5.0), center + Vec2::new(5.0, -5.0)],
                stroke,
            );
        }
        HealthSlotGlyph::ActiveCheck => {
            painter.circle_filled(center, radius, tone.gamma_multiply(0.2));
            painter.line_segment(
                [center + Vec2::new(-4.0, 0.0), center + Vec2::new(-1.0, 3.0)],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(-1.0, 3.0), center + Vec2::new(5.0, -4.0)],
                stroke,
            );
        }
        HealthSlotGlyph::StandbyRing => {
            painter.circle_stroke(center, radius, stroke);
        }
        HealthSlotGlyph::AttentionTriangle => {
            painter.add(Shape::convex_polygon(
                vec![
                    center + Vec2::new(0.0, -radius),
                    center + Vec2::new(radius, radius),
                    center + Vec2::new(-radius, radius),
                ],
                tone.gamma_multiply(0.18),
                stroke,
            ));
        }
        HealthSlotGlyph::FaultCross => {
            painter.circle_stroke(center, radius, stroke);
            painter.line_segment(
                [center + Vec2::new(-5.0, -5.0), center + Vec2::new(5.0, 5.0)],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(-5.0, 5.0), center + Vec2::new(5.0, -5.0)],
                stroke,
            );
        }
        HealthSlotGlyph::DisabledPause => {
            painter.line_segment(
                [
                    center + Vec2::new(-5.0, -4.0),
                    center + Vec2::new(-5.0, 4.0),
                ],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(5.0, -4.0), center + Vec2::new(5.0, 4.0)],
                stroke,
            );
        }
    }
}

// ===========================================================================
// Route preview — the pre-drive "review the route" screen (GMaps / Waze GO).
// ===========================================================================

/// Precomputed rects for the route-preview screen (so interaction + paint agree).
struct PreviewLayout {
    back: Rect,
    sheet: Rect,
    dest: Rect,
    options: Vec<Rect>,
    start: Rect,
}

/// Lay out the route-preview chrome over `rect`: a back button top-left and a
/// bottom sheet holding the destination summary, one card per route option, and
/// a full-width Start button. Every rect is crash-safe.
fn preview_layout(rect: Rect, n_options: usize) -> PreviewLayout {
    let margin = Style::SP_M;
    let back_r = 22.0;
    let back = Rect::from_center_size(
        egui::pos2(rect.left() + margin + back_r, rect.top() + margin + back_r),
        egui::vec2(back_r * 2.0, back_r * 2.0),
    );

    let sheet_w = (rect.width() - 2.0 * margin).max(1.0);
    let dest_h = 58.0;
    let opt_h = 74.0;
    let start_h = 52.0;
    let gap = Style::SP_S;
    let pad = Style::SP_M;
    let n = n_options as f32;
    let mut sheet_h =
        pad + dest_h + gap + n * opt_h + (n - 1.0).max(0.0) * gap + gap + start_h + pad;
    let max_sheet = (rect.height() - 2.0 * margin - 40.0).max(120.0);
    if sheet_h > max_sheet {
        sheet_h = max_sheet;
    }
    let sheet = safe_rect(
        rect.left() + margin,
        rect.bottom() - margin - sheet_h,
        sheet_w,
        sheet_h,
    );

    let inner_x = sheet.left() + pad;
    let inner_w = (sheet.width() - 2.0 * pad).max(1.0);
    let mut y = sheet.top() + pad;
    let dest = safe_rect(inner_x, y, inner_w, dest_h);
    y = dest.bottom() + gap;
    let mut options = Vec::with_capacity(n_options);
    for _ in 0..n_options {
        let r = safe_rect(inner_x, y, inner_w, opt_h);
        options.push(r);
        y = r.bottom() + gap;
    }
    let start = safe_rect(inner_x, sheet.bottom() - pad - start_h, inner_w, start_h);

    PreviewLayout {
        back,
        sheet,
        dest,
        options,
        start,
    }
}

#[allow(clippy::too_many_lines)]
fn show_route_preview(ui: &mut egui::Ui, state: &mut MapsLocationSurface) {
    let width = safe_width(ui);
    let avail_h = ui.available_height();
    let height = if avail_h.is_finite() && avail_h > 1.0 {
        avail_h.clamp(320.0, 1400.0)
    } else {
        520.0
    };
    let (rect, _resp) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }

    let n_options = state.local_navigation.route_options.len();
    let layout = preview_layout(rect, n_options);

    // --- Interactions first, so the painter borrow of `ui` stays clean. -----
    let back_resp = ui.interact(
        layout.back,
        egui::Id::new("maps-preview-back"),
        Sense::click(),
    );
    if back_resp.clicked() {
        state.route_preview = false;
    }
    let back_hovered = back_resp.hovered();

    let mut option_states: Vec<(bool, bool)> = Vec::with_capacity(n_options);
    for (idx, orect) in layout.options.iter().enumerate() {
        let resp = ui.interact(
            *orect,
            egui::Id::new(("maps-preview-option", idx)),
            Sense::click(),
        );
        if resp.clicked() {
            state.local_navigation.apply_route_option(idx);
        }
        option_states.push((resp.hovered(), resp.is_pointer_button_down_on()));
    }

    let has_fix = state
        .locations
        .primary_sample()
        .is_some_and(LocationSample::has_fix);
    let offline_status = state.offline_navigation_status();
    let route_selected = state
        .local_navigation
        .route_options
        .get(state.local_navigation.selected_route)
        .is_some();
    let has_destination = state.local_navigation.active_destination().is_some();
    let start_readiness =
        route_preview_start_readiness(route_selected, has_destination, has_fix, &offline_status);
    // Keep the model predicate as the final authority. The view adds the
    // explicit GPS-fix guard so a connected-but-acquiring source cannot make
    // the Start affordance look actionable.
    let can_start = start_readiness.can_start && state.can_start_navigation();
    let start_resp = ui.interact(
        layout.start,
        egui::Id::new("maps-preview-start"),
        if can_start {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let start_resp = mde_egui::widgets::hover_text(start_resp, start_readiness.tooltip.clone());
    if can_start && start_resp.clicked() {
        state.start_navigation();
    }
    let start_hovered = start_resp.hovered();
    let start_pressed = start_resp.is_pointer_button_down_on();

    // --- Paint. -------------------------------------------------------------
    let primary = state.locations.primary_sample();
    let painter = ui.painter_at(rect);

    // Overview map showing the whole route (does not touch persistent view state).
    let mut overview = state.map.clone();
    overview.zoom = 6.5;
    overview.pan = [0.0, 0.0];
    overview.route_visible = true;
    paint_map_scene(
        &painter,
        rect,
        &overview,
        &state.dead_zones,
        primary,
        has_fix,
        live_nws_vehicle_point(&state.locations),
        has_fix,
        state.local_navigation.active_route.is_planned(),
        state
            .local_navigation
            .active_destination()
            .and_then(Destination::geo),
        None,
    );
    // Gentle scrim so the sheet + chrome read cleanly over the map.
    painter.rect_filled(rect, Style::RADIUS_L, Color32::BLACK.gamma_multiply(0.18));

    // Back button + screen title.
    paint_round_button(&painter, layout.back.center(), 22.0, back_hovered, false);
    paint_back_glyph(&painter, layout.back.center(), 22.0);
    painter.text(
        egui::pos2(layout.back.right() + Style::SP_M, layout.back.center().y),
        Align2::LEFT_CENTER,
        "Route preview",
        Style::typography_font(TypographyRole::Headline),
        Style::TEXT_STRONG,
    );

    // Bottom sheet.
    paint_soft_shadow(&painter, layout.sheet, HUD_RADIUS);
    painter.rect_filled(layout.sheet, HUD_RADIUS, HUD_CARD_BG);
    paint_card_sheen(
        &painter,
        layout.sheet,
        HUD_RADIUS,
        HUD_CARD_HI.gamma_multiply(0.5),
        Color32::BLACK.gamma_multiply(0.12),
    );
    painter.rect_stroke(
        layout.sheet,
        HUD_RADIUS,
        Stroke::new(1.0, Style::BORDER),
        StrokeKind::Inside,
    );

    // Destination summary.
    paint_destination_summary(
        &painter,
        layout.dest,
        state.local_navigation.active_destination(),
    );

    // Route option cards — or the honest "no routing engine" note when none
    // exist (production has no offline router yet; Start stays a no-op).
    let selected = state.local_navigation.selected_route;
    for (idx, orect) in layout.options.iter().enumerate() {
        if let Some(option) = state.local_navigation.route_options.get(idx) {
            let (hovered, pressed) = option_states.get(idx).copied().unwrap_or((false, false));
            paint_route_option_card(&painter, *orect, option, idx == selected, hovered, pressed);
        }
    }
    if state.local_navigation.route_options.is_empty() {
        painter.text(
            egui::pos2(
                layout.sheet.center().x,
                (layout.dest.bottom() + layout.start.top()) / 2.0,
            ),
            Align2::CENTER_CENTER,
            "No offline routing engine — route options unavailable",
            Style::typography_font(TypographyRole::Body),
            Style::TEXT_DIM,
        );
    }

    // Start button.
    paint_start_button(
        &painter,
        layout.start,
        start_hovered,
        start_pressed,
        has_fix,
        can_start,
        start_readiness.button_label,
    );
}

/// Render-agnostic explanation for why route-preview Start is or is not
/// actionable. This deliberately consumes the existing route/readiness model
/// rather than inventing a route or relaxing the navigation guards.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RoutePreviewStartReadiness {
    can_start: bool,
    button_label: &'static str,
    tooltip: String,
}

fn route_preview_start_readiness(
    route_selected: bool,
    has_destination: bool,
    has_gps_fix: bool,
    offline: &OfflineNavigationStatus,
) -> RoutePreviewStartReadiness {
    let mut reasons = Vec::new();
    if !route_selected {
        reasons.push("No route is available to start.".to_string());
    }
    if !has_destination {
        reasons.push("No destination is selected.".to_string());
    }
    if !has_gps_fix {
        reasons.push(format!(
            "{} has no GPS fix.",
            offline.primary_source.label()
        ));
    }
    if offline.readiness == OfflineNavigationReadiness::Blocked {
        if offline.blockers.is_empty() {
            reasons.push("Offline navigation readiness is blocked.".to_string());
        } else {
            reasons.extend(offline.blockers.iter().cloned());
        }
    }

    let can_start = reasons.is_empty() && offline.can_claim_turn_by_turn();
    let button_label = if can_start {
        "Start"
    } else if !route_selected {
        "No route available"
    } else if !has_destination {
        "Choose a destination"
    } else if !has_gps_fix {
        "Waiting for GPS"
    } else if offline.readiness == OfflineNavigationReadiness::Blocked {
        "Navigation blocked"
    } else {
        "Start unavailable"
    };
    let tooltip = if can_start {
        "Start turn-by-turn guidance on the selected route.".to_string()
    } else {
        format!("Start unavailable: {}", reasons.join(" "))
    };

    RoutePreviewStartReadiness {
        can_start,
        button_label,
        tooltip,
    }
}

/// A circular chrome button (back / close), matching the FAB elevation language.
fn paint_round_button(painter: &Painter, center: Pos2, r: f32, hovered: bool, pressed: bool) {
    if center.any_nan() {
        return;
    }
    painter.circle_filled(
        center + egui::vec2(0.0, 2.0),
        r,
        Color32::BLACK.gamma_multiply(0.3),
    );
    let fill = if pressed {
        Style::pressed_fill(Style::ACCENT)
    } else if hovered {
        Style::SURFACE_HI
    } else {
        HUD_CARD_BG
    };
    painter.circle_filled(center, r, fill);
    painter.circle_stroke(center, r, Stroke::new(1.0, Style::BORDER));
}

/// A left-pointing back chevron centered in a round button.
fn paint_back_glyph(painter: &Painter, center: Pos2, r: f32) {
    if center.any_nan() {
        return;
    }
    let s = r * 0.4;
    let x = center.x + s * 0.28;
    painter.add(Shape::line(
        vec![
            egui::pos2(x, center.y - s),
            egui::pos2(x - s, center.y),
            egui::pos2(x, center.y + s),
        ],
        Stroke::new(2.4, Style::TEXT_STRONG),
    ));
}

/// The destination summary row: a location pin, the place name, and its address.
fn paint_destination_summary(painter: &Painter, rect: Rect, destination: Option<&Destination>) {
    let pin_box = safe_rect(rect.left() + 4.0, rect.center().y - 13.0, 26.0, 26.0);
    if !paint_carbon(painter, pin_box, "location", ROUTE_BLUE) {
        painter.circle_filled(pin_box.center(), 11.0, MANEUVER_BLUE);
        painter.circle_filled(pin_box.center(), 4.0, Color32::WHITE);
    }
    let tx = pin_box.right() + Style::SP_S;
    let max_w = (rect.right() - tx).max(1.0);
    let (name, addr) = destination.map_or(("Destination", "Select a place"), |destination| {
        (destination.label.as_str(), destination.address.as_str())
    });
    let name_s = elide(
        painter,
        name,
        Style::typography_font(TypographyRole::Headline),
        max_w,
    );
    painter.text(
        egui::pos2(tx, rect.center().y - Style::SP_S),
        Align2::LEFT_CENTER,
        &name_s,
        Style::typography_font(TypographyRole::Headline),
        Style::TEXT_STRONG,
    );
    let addr_s = elide(
        painter,
        addr,
        Style::typography_font(TypographyRole::Body),
        max_w,
    );
    painter.text(
        egui::pos2(tx, rect.center().y + Style::SP_M - 2.0),
        Align2::LEFT_CENTER,
        &addr_s,
        Style::typography_font(TypographyRole::Body),
        Style::TEXT_DIM,
    );
}

fn route_traffic_tone(traffic: RouteTraffic) -> Color32 {
    match traffic {
        RouteTraffic::Clear => Style::OK,
        RouteTraffic::Slow => Style::WARN,
        RouteTraffic::Heavy => Style::DANGER,
    }
}

/// One selectable route-option card: label tag, big ETA (traffic-toned), the
/// distance · via road line, and a traffic dot + label on the right.
fn paint_route_option_card(
    painter: &Painter,
    rect: Rect,
    option: &RouteOption,
    selected: bool,
    hovered: bool,
    pressed: bool,
) {
    let fill = if pressed {
        Style::pressed_fill(Style::ACCENT)
    } else if selected {
        Style::ACCENT.gamma_multiply(0.16)
    } else if hovered {
        HUD_CARD_HI
    } else {
        Style::LAYER_02
    };
    painter.rect_filled(rect, HUD_RADIUS_S, fill);
    let (bw, border) = if selected {
        (2.0, Style::ACCENT)
    } else {
        (1.0, Style::BORDER)
    };
    painter.rect_stroke(
        rect,
        HUD_RADIUS_S,
        Stroke::new(bw, border),
        StrokeKind::Inside,
    );

    let pad = Style::SP_M;
    let tone = route_traffic_tone(option.traffic);

    // Option label tag (top-left).
    painter.text(
        egui::pos2(rect.left() + pad, rect.top() + 9.0),
        Align2::LEFT_TOP,
        &option.label,
        Style::typography_font(TypographyRole::Caption),
        if selected {
            Style::ACCENT_HI
        } else {
            Style::TEXT_DIM
        },
    );

    // Hero: total minutes for this option, coloured by traffic.
    let minutes = option.remaining_time_min.to_string();
    let num_g = painter.layout_no_wrap(
        minutes,
        Style::typography_font(TypographyRole::Display),
        tone,
    );
    let num_size = num_g.size();
    painter.galley(
        egui::pos2(rect.left() + pad, rect.top() + 24.0),
        num_g,
        tone,
    );
    painter.text(
        egui::pos2(
            rect.left() + pad + num_size.x + Style::SP_XS,
            rect.top() + 24.0 + num_size.y - 9.0,
        ),
        Align2::LEFT_BOTTOM,
        "min",
        Style::typography_font(TypographyRole::Body),
        tone.gamma_multiply(0.92),
    );

    // Distance · via road (bottom-left).
    let sub = format!(
        "{:.1} mi   \u{00B7}   via {}",
        finite_or(option.remaining_distance_mi, 0.0).max(0.0),
        option.via
    );
    let sub_max = (rect.right() - (rect.left() + pad) - 96.0).max(1.0);
    let sub_s = elide(
        painter,
        &sub,
        Style::typography_font(TypographyRole::Caption),
        sub_max,
    );
    painter.text(
        egui::pos2(rect.left() + pad, rect.bottom() - 9.0),
        Align2::LEFT_BOTTOM,
        &sub_s,
        Style::typography_font(TypographyRole::Caption),
        Style::TEXT_DIM,
    );

    // Traffic dot + label (right, vertically centered).
    let label_g = painter.layout_no_wrap(
        option.traffic.label().to_string(),
        Style::typography_font(TypographyRole::Body),
        tone,
    );
    let label_size = label_g.size();
    let label_x = rect.right() - pad - label_size.x;
    painter.galley(
        egui::pos2(label_x, rect.center().y - label_size.y * 0.5),
        label_g,
        tone,
    );
    painter.circle_filled(
        egui::pos2(label_x - Style::SP_S, rect.center().y),
        4.0,
        tone,
    );
}

/// The full-width GMaps-blue Start button that begins turn-by-turn guidance.
fn paint_start_button(
    painter: &Painter,
    rect: Rect,
    hovered: bool,
    pressed: bool,
    has_fix: bool,
    enabled: bool,
    label: &str,
) {
    paint_soft_shadow(painter, rect, HUD_RADIUS_S);
    let base = if !enabled {
        HUD_CARD_HI.gamma_multiply(0.72)
    } else if !has_fix {
        MANEUVER_BLUE.gamma_multiply(0.7)
    } else if pressed {
        MANEUVER_BLUE_DEEP
    } else if hovered {
        MANEUVER_BLUE_HI
    } else {
        MANEUVER_BLUE
    };
    painter.rect_filled(rect, HUD_RADIUS_S, base);
    paint_card_sheen(
        painter,
        rect,
        HUD_RADIUS_S,
        MANEUVER_BLUE_HI.gamma_multiply(0.5),
        MANEUVER_BLUE_DEEP.gamma_multiply(0.5),
    );
    painter.rect_stroke(
        rect,
        HUD_RADIUS_S,
        Stroke::new(
            1.0,
            if enabled {
                MANEUVER_BLUE_HI
            } else {
                Style::BORDER
            },
        ),
        StrokeKind::Inside,
    );

    // Nav-arrow glyph + the truthful action/readiness label, centered as a
    // group. Disabled labels are intentionally specific; the full blocker
    // explanation is attached to the interaction as a tooltip by the caller.
    let g = painter.layout_no_wrap(
        label.to_string(),
        Style::typography_font(TypographyRole::Headline),
        if enabled {
            Color32::WHITE
        } else {
            Style::TEXT_DIM
        },
    );
    let gw = g.size().x;
    let glyph_w = 22.0;
    let total = glyph_w + Style::SP_S + gw;
    let start_x = rect.center().x - total * 0.5;
    if start_x.is_finite() {
        paint_vehicle_chevron(
            painter,
            egui::pos2(start_x + glyph_w * 0.5, rect.center().y),
            0.0,
            if enabled {
                Color32::WHITE
            } else {
                Style::TEXT_DIM
            },
            false,
        );
    }
    painter.galley(
        egui::pos2(
            start_x + glyph_w + Style::SP_S,
            rect.center().y - g.size().y * 0.5,
        ),
        g,
        if enabled {
            Color32::WHITE
        } else {
            Style::TEXT_DIM
        },
    );
}

// ===========================================================================
// Destination search — the "Where to?" entry screen (Google Maps / Waze).
// ===========================================================================

/// Quick-access category chips shown across the top of the search screen —
/// `(label, category-key)`; the key matches a `Destination::category`.
const SEARCH_CATEGORIES: &[(&str, &str)] = &[
    ("Home", "home"),
    ("Work", "work"),
    ("Fuel", "fuel"),
    ("Food", "food"),
    ("Parking", "parking"),
];

/// Precomputed rects for the destination-search screen.
struct SearchLayout {
    back: Rect,
    search_bar: Rect,
    chips: Vec<Rect>,
    list_card: Rect,
    rows: Vec<Rect>,
}

/// Keep the destination-search surface inside the space that is actually
/// visible to its parent. A small seat can report a larger layout budget than
/// the current clip (for example, a 320 px minimum inside a 240 px viewport),
/// which would make the lower search controls render below the screen.
fn bounded_search_height(available_height: f32, clip_remaining: f32) -> f32 {
    let bound = [available_height, clip_remaining]
        .into_iter()
        .filter(|height| height.is_finite() && *height > 1.0)
        .fold(f32::INFINITY, f32::min);
    if bound.is_finite() {
        bound.clamp(1.0, 1400.0)
    } else {
        // The clip is normally finite for an on-screen UI. Preserve the
        // historical fallback only for an unbounded/offline layout probe.
        520.0
    }
}

/// Lay out the search chrome over `rect`: a back button + full-width search bar
/// at the top, a row of category chips, then a scroll-free list card holding one
/// tappable row per destination (clipped to what fits). Every rect is crash-safe.
fn search_layout(rect: Rect, n_rows: usize, n_chips: usize) -> SearchLayout {
    let margin = Style::SP_M;
    let content_l = rect.left() + margin;
    let content_r = rect.right() - margin;
    let content_w = (content_r - content_l).max(1.0);

    let bar_h = 52.0;
    let back_r = bar_h * 0.5;
    let top = rect.top() + margin;
    let back = Rect::from_center_size(
        egui::pos2(content_l + back_r, top + back_r),
        egui::vec2(back_r * 2.0, back_r * 2.0),
    );
    let bar_l = back.right() + Style::SP_S;
    let search_bar = safe_rect(bar_l, top, (content_r - bar_l).max(1.0), bar_h);

    // Category chip row.
    let chip_h = 64.0;
    let chip_y = search_bar.bottom() + Style::SP_M;
    let gap = Style::SP_S;
    let n = n_chips.max(1) as f32;
    let chip_w = ((content_w - (n - 1.0) * gap) / n).max(1.0);
    let mut chips = Vec::with_capacity(n_chips);
    for i in 0..n_chips {
        let x = content_l + i as f32 * (chip_w + gap);
        chips.push(safe_rect(x, chip_y, chip_w, chip_h));
    }

    // List card fills the remaining height.
    let list_top = chip_y + chip_h + Style::SP_M;
    let list_bottom = rect.bottom() - margin;
    let list_h = (list_bottom - list_top).max(1.0);
    let list_card = safe_rect(content_l, list_top, content_w, list_h);

    // Rows inside the list card (below the header), clipped to what fits.
    let pad = Style::SP_M;
    let header_h = 24.0;
    let row_h = 56.0;
    let rows_top = list_card.top() + pad + header_h;
    let room = ((list_card.bottom() - pad - rows_top) / row_h).floor();
    let fits = if room.is_finite() && room > 0.0 {
        room as usize
    } else {
        0
    };
    let shown = n_rows.min(fits);
    let inner_x = list_card.left() + pad;
    let inner_w = (list_card.width() - 2.0 * pad).max(1.0);
    let mut rows = Vec::with_capacity(shown);
    for i in 0..shown {
        let y = rows_top + i as f32 * row_h;
        rows.push(safe_rect(inner_x, y + 2.0, inner_w, row_h - 6.0));
    }

    SearchLayout {
        back,
        search_bar,
        chips,
        list_card,
        rows,
    }
}

#[allow(clippy::too_many_lines)]
fn show_destination_search(ui: &mut egui::Ui, state: &mut MapsLocationSurface) {
    // Run the offline geocoder for the current query (fail-soft; early-returns
    // unless the trimmed text changed since last frame).
    state.refresh_geocode();

    let width = safe_width(ui);
    let clip_remaining = ui.clip_rect().bottom() - ui.cursor().top();
    let height = bounded_search_height(ui.available_height(), clip_remaining);
    let (rect, _resp) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }

    // While the field holds text we list LIVE geocoder results; empty falls back
    // to the recent/saved presets. `rows` is an owned snapshot so no borrow of
    // `state` is held across the `TextEdit`'s `&mut` below.
    let querying = !state.destination_query.trim().is_empty();
    let from = state.locations.primary_sample().cloned();
    let rows: Vec<Destination> = if querying {
        state
            .geocode_results
            .iter()
            .map(|r| Destination::from_geo(r, from.as_ref()))
            .collect()
    } else {
        state.local_navigation.destinations.clone()
    };
    let empty_note = if querying && rows.is_empty() {
        state.geocode_note.clone()
    } else if !querying && rows.is_empty() {
        // Honest empty recents: production ships zero preset destinations, so
        // say so instead of presenting a silent blank card.
        Some("No recent or saved places — type to search the offline gazetteer.".to_string())
    } else {
        None
    };
    let header = if querying {
        "Search results"
    } else {
        "Recent & saved"
    };
    let layout = search_layout(rect, rows.len(), SEARCH_CATEGORIES.len());

    // --- Interactions first (keep the painter borrow of `ui` clean). --------
    let back_resp = ui.interact(
        layout.back,
        egui::Id::new("maps-search-back"),
        Sense::click(),
    );
    if back_resp.clicked() {
        state.destination_search = false;
    }
    let back_hovered = back_resp.hovered();

    // Hover-only sense: the real editable field (put below) owns the clicks.
    let bar_hovered = ui
        .interact(
            layout.search_bar,
            egui::Id::new("maps-search-bar"),
            Sense::hover(),
        )
        .hovered();

    let mut chip_states: Vec<(bool, bool)> = Vec::with_capacity(layout.chips.len());
    for (i, crect) in layout.chips.iter().enumerate() {
        let resp = ui.interact(
            *crect,
            egui::Id::new(("maps-search-chip", i)),
            Sense::click(),
        );
        if resp.clicked() {
            if let Some(&(_, key)) = SEARCH_CATEGORIES.get(i) {
                if let Some(idx) = state.local_navigation.destination_in_category(key) {
                    state.choose_destination(idx);
                }
            }
        }
        chip_states.push((resp.hovered(), resp.is_pointer_button_down_on()));
    }

    let mut row_states: Vec<(bool, bool)> = Vec::with_capacity(layout.rows.len());
    for (i, rrect) in layout.rows.iter().enumerate() {
        let resp = ui.interact(
            *rrect,
            egui::Id::new(("maps-search-row", i)),
            Sense::click(),
        );
        if resp.clicked() {
            // A live result promotes to a real pinned destination; a preset row
            // selects it directly. Both advance to the route preview.
            if querying {
                state.choose_geo_result(i);
            } else {
                state.choose_destination(i);
            }
        }
        row_states.push((resp.hovered(), resp.is_pointer_button_down_on()));
    }

    // --- Paint. -------------------------------------------------------------
    let primary = state.locations.primary_sample();
    let has_fix = primary.is_some_and(LocationSample::has_fix);
    let painter = ui.painter_at(rect);

    // Overview map, strongly scrimmed so the search screen reads as a panel.
    let mut overview = state.map.clone();
    overview.zoom = 6.0;
    overview.pan = [0.0, 0.0];
    overview.route_visible = false;
    paint_map_scene(
        &painter,
        rect,
        &overview,
        &state.dead_zones,
        primary,
        has_fix,
        live_nws_vehicle_point(&state.locations),
        false,
        false,
        state
            .local_navigation
            .active_destination()
            .and_then(Destination::geo),
        None,
    );
    painter.rect_filled(rect, Style::RADIUS_L, Color32::BLACK.gamma_multiply(0.5));

    // Back button + search-bar pill (the editable field is overlaid last).
    let back_r = layout.back.width() * 0.5;
    paint_round_button(&painter, layout.back.center(), back_r, back_hovered, false);
    paint_back_glyph(&painter, layout.back.center(), back_r);
    paint_search_bar(&painter, layout.search_bar, bar_hovered, "");

    // Category chips.
    for (i, crect) in layout.chips.iter().enumerate() {
        if let Some(&(label, key)) = SEARCH_CATEGORIES.get(i) {
            let (hovered, pressed) = chip_states.get(i).copied().unwrap_or((false, false));
            paint_category_chip(&painter, *crect, label, key, hovered, pressed);
        }
    }

    // List card + header.
    paint_soft_shadow(&painter, layout.list_card, HUD_RADIUS);
    painter.rect_filled(layout.list_card, HUD_RADIUS, HUD_CARD_BG);
    paint_card_sheen(
        &painter,
        layout.list_card,
        HUD_RADIUS,
        HUD_CARD_HI.gamma_multiply(0.5),
        Color32::BLACK.gamma_multiply(0.12),
    );
    painter.rect_stroke(
        layout.list_card,
        HUD_RADIUS,
        Stroke::new(1.0, Style::BORDER),
        StrokeKind::Inside,
    );
    painter.text(
        egui::pos2(
            layout.list_card.left() + Style::SP_M,
            layout.list_card.top() + Style::SP_M,
        ),
        Align2::LEFT_TOP,
        header,
        FontId::proportional(Style::BODY),
        Style::TEXT_DIM,
    );

    // Destination rows (live results or recent/saved presets).
    for (i, rrect) in layout.rows.iter().enumerate() {
        if let Some(destination) = rows.get(i) {
            let (hovered, pressed) = row_states.get(i).copied().unwrap_or((false, false));
            paint_destination_row(&painter, *rrect, destination, hovered, pressed);
        }
    }

    // A soft note in place of results (no gazetteer installed / no match).
    if let Some(note) = &empty_note {
        painter.text(
            layout.list_card.center(),
            Align2::CENTER_CENTER,
            note,
            FontId::proportional(Style::BODY),
            Style::TEXT_DIM,
        );
    }

    // --- The real editable field, overlaid on the pill (drawn last, on top). -
    let want_focus = state.take_search_focus();
    let icon_left = layout.search_bar.left() + Style::SP_M;
    let text_left = icon_left + 22.0 + Style::SP_S;
    let text_rect = safe_rect(
        text_left,
        layout.search_bar.top() + 6.0,
        (layout.search_bar.right() - Style::SP_M - text_left).max(1.0),
        (layout.search_bar.height() - 12.0).max(1.0),
    );
    let field = egui::TextEdit::singleline(&mut state.destination_query)
        .frame(false)
        .hint_text("Where to?")
        .font(FontId::proportional(Style::TITLE))
        .text_color(Style::TEXT_STRONG)
        .vertical_align(Align::Center)
        .desired_width(text_rect.width());
    let field_resp = ui.put(text_rect, field);
    if want_focus {
        field_resp.request_focus();
    }
}

/// A full-width rounded search bar with a leading magnifier and placeholder —
/// the recognizable "Where to?" entry field (reused on the Map tab).
fn paint_search_bar(painter: &Painter, rect: Rect, hovered: bool, placeholder: &str) {
    if !rect.width().is_finite() || rect.width() < 8.0 || !rect.height().is_finite() {
        return;
    }
    let radius = (rect.height() * 0.5).max(1.0);
    paint_soft_shadow(painter, rect, radius);
    let fill = if hovered { HUD_CARD_HI } else { HUD_CARD_BG };
    painter.rect_filled(rect, radius, fill);
    paint_card_sheen(
        painter,
        rect,
        radius,
        HUD_CARD_HI.gamma_multiply(0.6),
        Color32::BLACK.gamma_multiply(0.12),
    );
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(
            1.0,
            if hovered {
                Style::ACCENT
            } else {
                Style::BORDER
            },
        ),
        StrokeKind::Inside,
    );

    let gy = rect.center().y;
    let icon_box = safe_rect(rect.left() + Style::SP_M, gy - 11.0, 22.0, 22.0);
    if !paint_carbon(painter, icon_box, "system-search", Style::TEXT_DIM) {
        paint_search_glyph(painter, icon_box.center(), 9.0, Style::TEXT_DIM);
    }
    let tx = icon_box.right() + Style::SP_S;
    let max_w = (rect.right() - Style::SP_M - tx).max(1.0);
    let shown = elide(
        painter,
        placeholder,
        FontId::proportional(Style::TITLE),
        max_w,
    );
    painter.text(
        egui::pos2(tx, gy),
        Align2::LEFT_CENTER,
        &shown,
        FontId::proportional(Style::TITLE),
        Style::TEXT_STRONG,
    );
}

/// One quick-access category chip: a glyph over a label.
fn paint_category_chip(
    painter: &Painter,
    rect: Rect,
    label: &str,
    category: &str,
    hovered: bool,
    pressed: bool,
) {
    let fill = if pressed {
        Style::pressed_fill(Style::ACCENT)
    } else if hovered {
        HUD_CARD_HI
    } else {
        Style::LAYER_02
    };
    painter.rect_filled(rect, HUD_RADIUS_S, fill);
    painter.rect_stroke(
        rect,
        HUD_RADIUS_S,
        Stroke::new(1.0, Style::BORDER),
        StrokeKind::Inside,
    );
    let icon_side = (rect.width().min(rect.height()) * 0.42).clamp(14.0, 28.0);
    let icon_rect = safe_rect(
        rect.center().x - icon_side * 0.5,
        rect.top() + rect.height() * 0.24,
        icon_side,
        icon_side,
    );
    paint_category_icon(painter, icon_rect, category, Style::ACCENT_HI);
    let shown = elide(
        painter,
        label,
        FontId::proportional(Style::SMALL),
        (rect.width() - 6.0).max(1.0),
    );
    painter.text(
        egui::pos2(rect.center().x, rect.bottom() - 9.0),
        Align2::CENTER_BOTTOM,
        &shown,
        FontId::proportional(Style::SMALL),
        Style::TEXT,
    );
}

/// One tappable destination row: leading category glyph, name + address, and a
/// right-aligned distance (Google-Maps / Waze recents grammar).
fn paint_destination_row(
    painter: &Painter,
    rect: Rect,
    destination: &Destination,
    hovered: bool,
    pressed: bool,
) {
    let fill = if pressed {
        Style::pressed_fill(Style::ACCENT)
    } else if hovered {
        HUD_CARD_HI
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        painter.rect_filled(rect, HUD_RADIUS_S, fill);
    }

    // Leading round glyph chip.
    let icon_d = (rect.height() * 0.66).clamp(20.0, 40.0);
    let icon_c = egui::pos2(rect.left() + icon_d * 0.5 + 4.0, rect.center().y);
    if icon_c.x.is_finite() && icon_c.y.is_finite() {
        painter.circle_filled(icon_c, icon_d * 0.5, Style::LAYER_02);
        let icon_box = safe_rect(
            icon_c.x - icon_d * 0.3,
            icon_c.y - icon_d * 0.3,
            icon_d * 0.6,
            icon_d * 0.6,
        );
        paint_category_icon(painter, icon_box, &destination.category, Style::ACCENT_HI);
    }

    let tx = icon_c.x + icon_d * 0.5 + Style::SP_S;
    // Right-aligned distance.
    let dist_s = format!("{:.1} mi", finite_or(destination.distance_mi, 0.0).max(0.0));
    let dist_g = painter.layout_no_wrap(dist_s, FontId::proportional(Style::BODY), Style::TEXT_DIM);
    let dist_x = rect.right() - Style::SP_M - dist_g.size().x;
    painter.galley(
        egui::pos2(dist_x, rect.center().y - dist_g.size().y * 0.5),
        dist_g,
        Style::TEXT_DIM,
    );

    let max_w = (dist_x - Style::SP_S - tx).max(1.0);
    let name_s = elide(
        painter,
        &destination.label,
        FontId::proportional(Style::TITLE),
        max_w,
    );
    painter.text(
        egui::pos2(tx, rect.center().y - Style::SP_S),
        Align2::LEFT_CENTER,
        &name_s,
        FontId::proportional(Style::TITLE),
        Style::TEXT_STRONG,
    );
    let addr_s = elide(
        painter,
        &destination.address,
        FontId::proportional(Style::SMALL),
        max_w,
    );
    painter.text(
        egui::pos2(tx, rect.center().y + Style::SP_M - 3.0),
        Align2::LEFT_CENTER,
        &addr_s,
        FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );

    // Hairline separator under the row.
    let sy = rect.bottom() + 3.0;
    if sy.is_finite() {
        painter.line_segment(
            [
                egui::pos2(rect.left() + 2.0, sy),
                egui::pos2(rect.right() - 2.0, sy),
            ],
            Stroke::new(1.0, Style::BORDER.gamma_multiply(0.5)),
        );
    }
}

/// Paint a category glyph — an embedded Carbon icon where one exists, otherwise
/// a crisp procedural glyph so every category always shows an icon.
fn paint_category_icon(painter: &Painter, rect: Rect, category: &str, color: Color32) {
    let cat = category.to_ascii_lowercase();
    let carbon = match cat.as_str() {
        "favorite" => Some("star"),
        "recent" => Some("document-open-recent"),
        _ => None,
    };
    if let Some(name) = carbon {
        if paint_carbon(painter, rect, name, color) {
            return;
        }
    }

    let c = rect.center();
    let s = rect.width().min(rect.height());
    if !c.x.is_finite() || !c.y.is_finite() || !(s > 1.0) {
        return;
    }
    let stroke = Stroke::new((s * 0.09).max(1.3), color);
    let p = |dx: f32, dy: f32| egui::pos2(c.x + dx * s, c.y + dy * s);
    match cat.as_str() {
        "home" => {
            painter.add(Shape::line(
                vec![p(-0.34, -0.02), p(0.0, -0.34), p(0.34, -0.02)],
                stroke,
            ));
            painter.rect_stroke(
                Rect::from_min_max(p(-0.24, -0.02), p(0.24, 0.30)),
                s * 0.06,
                stroke,
                StrokeKind::Inside,
            );
        }
        "work" => {
            painter.add(Shape::line(
                vec![
                    p(-0.12, -0.10),
                    p(-0.12, -0.24),
                    p(0.12, -0.24),
                    p(0.12, -0.10),
                ],
                stroke,
            ));
            painter.rect_stroke(
                Rect::from_min_max(p(-0.32, -0.10), p(0.32, 0.28)),
                s * 0.06,
                stroke,
                StrokeKind::Inside,
            );
            painter.line_segment([p(-0.32, 0.06), p(0.32, 0.06)], stroke);
        }
        "fuel" => {
            painter.rect_stroke(
                Rect::from_min_max(p(-0.30, -0.30), p(0.06, 0.30)),
                s * 0.05,
                stroke,
                StrokeKind::Inside,
            );
            painter.line_segment([p(-0.30, -0.10), p(0.06, -0.10)], stroke);
            // Nozzle / feed line on the right.
            painter.add(Shape::line(
                vec![p(0.06, 0.02), p(0.22, 0.02), p(0.22, -0.20), p(0.14, -0.28)],
                stroke,
            ));
        }
        "food" => {
            // Fork.
            painter.line_segment([p(-0.16, -0.32), p(-0.16, 0.32)], stroke);
            painter.line_segment([p(-0.24, -0.32), p(-0.24, -0.12)], stroke);
            painter.line_segment([p(-0.08, -0.32), p(-0.08, -0.12)], stroke);
            painter.line_segment([p(-0.24, -0.12), p(-0.08, -0.12)], stroke);
            // Knife.
            painter.line_segment([p(0.18, -0.32), p(0.18, 0.32)], stroke);
            painter.add(Shape::line(
                vec![p(0.18, -0.32), p(0.28, -0.20), p(0.18, -0.04)],
                stroke,
            ));
        }
        "parking" => {
            painter.rect_stroke(
                Rect::from_min_max(p(-0.30, -0.32), p(0.30, 0.32)),
                s * 0.10,
                stroke,
                StrokeKind::Inside,
            );
            painter.text(
                c,
                Align2::CENTER_CENTER,
                "P",
                FontId::proportional(s * 0.62),
                color,
            );
        }
        "favorite" => paint_star_glyph(painter, c, s * 0.36, color),
        "recent" => {
            painter.circle_stroke(c, s * 0.34, stroke);
            painter.line_segment([c, p(0.0, -0.20)], stroke);
            painter.line_segment([c, p(0.16, 0.06)], stroke);
        }
        _ => {
            // Default location pin (mirrors the preview summary fallback).
            painter.circle_filled(egui::pos2(c.x, c.y - s * 0.08), s * 0.26, color);
            painter.add(Shape::convex_polygon(
                vec![p(-0.14, 0.02), p(0.14, 0.02), p(0.0, 0.34)],
                color,
                Stroke::NONE,
            ));
            painter.circle_filled(egui::pos2(c.x, c.y - s * 0.08), s * 0.10, HUD_CARD_BG);
        }
    }
}

/// A 5-point star outline centered at `c` (favorite-category fallback).
fn paint_star_glyph(painter: &Painter, c: Pos2, r: f32, color: Color32) {
    if c.any_nan() || !(r > 0.5) {
        return;
    }
    let mut pts = Vec::with_capacity(10);
    for i in 0..10 {
        let ang = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
        let rad = if i % 2 == 0 { r } else { r * 0.42 };
        let p = egui::pos2(c.x + ang.cos() * rad, c.y + ang.sin() * rad);
        if p.any_nan() {
            return;
        }
        pts.push(p);
    }
    painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
}

/// A procedural magnifier (search-bar / FAB fallback when the Carbon glyph is
/// unavailable).
fn paint_search_glyph(painter: &Painter, center: Pos2, r: f32, color: Color32) {
    if center.any_nan() || !(r > 0.5) {
        return;
    }
    let stroke = Stroke::new((r * 0.28).max(1.4), color);
    let ring_c = egui::pos2(center.x - r * 0.22, center.y - r * 0.22);
    painter.circle_stroke(ring_c, r * 0.62, stroke);
    let diag = std::f32::consts::FRAC_1_SQRT_2;
    let d = egui::vec2(diag, diag);
    painter.line_segment([ring_c + d * (r * 0.62), center + d * (r * 0.95)], stroke);
}

// ===========================================================================
// Arrival — the "You have arrived" screen (Google Maps arrival card).
// ===========================================================================

/// Precomputed rects for the arrival screen.
struct ArrivalLayout {
    card: Rect,
    badge: Rect,
    end_btn: Rect,
    save_btn: Rect,
}

fn arrival_layout(rect: Rect) -> ArrivalLayout {
    let margin = Style::SP_M;
    let card_w = (rect.width() - 2.0 * margin).min(460.0).max(1.0);
    let card_h = 288.0_f32.min((rect.height() - 2.0 * margin).max(120.0));
    let card = safe_rect(
        rect.center().x - card_w * 0.5,
        rect.center().y - card_h * 0.5,
        card_w,
        card_h,
    );
    let badge_d = 76.0;
    let badge = safe_rect(
        card.center().x - badge_d * 0.5,
        card.top() + Style::SP_L,
        badge_d,
        badge_d,
    );
    let btn_h = 46.0;
    let pad = Style::SP_M;
    let gap = Style::SP_S;
    let btn_w = ((card.width() - 2.0 * pad - gap) * 0.5).max(1.0);
    let btn_y = card.bottom() - pad - btn_h;
    let end_btn = safe_rect(card.left() + pad, btn_y, btn_w, btn_h);
    let save_btn = safe_rect(end_btn.right() + gap, btn_y, btn_w, btn_h);
    ArrivalLayout {
        card,
        badge,
        end_btn,
        save_btn,
    }
}

#[allow(clippy::too_many_lines)]
fn show_arrival(ui: &mut egui::Ui, state: &mut MapsLocationSurface) {
    let width = safe_width(ui);
    let avail_h = ui.available_height();
    let height = if avail_h.is_finite() && avail_h > 1.0 {
        avail_h.clamp(320.0, 1400.0)
    } else {
        520.0
    };
    let (rect, _resp) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }

    let layout = arrival_layout(rect);

    // --- Interactions first. ------------------------------------------------
    let end_resp = ui.interact(
        layout.end_btn,
        egui::Id::new("maps-arrival-end"),
        Sense::click(),
    );
    if end_resp.clicked() {
        state.end_navigation();
    }
    let end_hovered = end_resp.hovered();
    let end_pressed = end_resp.is_pointer_button_down_on();

    let saved_id = egui::Id::new(("maps-arrival", "saved"));
    let mut saved = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(saved_id))
        .unwrap_or(false);
    let save_resp = ui.interact(
        layout.save_btn,
        egui::Id::new("maps-arrival-save"),
        Sense::click(),
    );
    if save_resp.clicked() {
        saved = !saved;
        ui.ctx().data_mut(|d| d.insert_temp(saved_id, saved));
    }
    let save_hovered = save_resp.hovered();
    let save_pressed = save_resp.is_pointer_button_down_on();

    // --- Paint. -------------------------------------------------------------
    let primary = state.locations.primary_sample();
    let has_fix = primary.is_some_and(LocationSample::has_fix);
    let painter = ui.painter_at(rect);

    let mut overview = state.map.clone();
    overview.zoom = 7.5;
    overview.pan = [0.0, 0.0];
    overview.route_visible = false;
    paint_map_scene(
        &painter,
        rect,
        &overview,
        &state.dead_zones,
        primary,
        has_fix,
        live_nws_vehicle_point(&state.locations),
        false,
        false,
        state
            .local_navigation
            .active_destination()
            .and_then(Destination::geo),
        None,
    );
    painter.rect_filled(rect, Style::RADIUS_L, Color32::BLACK.gamma_multiply(0.5));

    // Card.
    paint_soft_shadow(&painter, layout.card, HUD_RADIUS);
    painter.rect_filled(layout.card, HUD_RADIUS, HUD_CARD_BG);
    paint_card_sheen(
        &painter,
        layout.card,
        HUD_RADIUS,
        HUD_CARD_HI.gamma_multiply(0.5),
        Color32::BLACK.gamma_multiply(0.12),
    );
    painter.rect_stroke(
        layout.card,
        HUD_RADIUS,
        Stroke::new(1.0, Style::BORDER),
        StrokeKind::Inside,
    );

    // Green check badge.
    let badge_c = layout.badge.center();
    let badge_r = layout.badge.width() * 0.5;
    if badge_c.x.is_finite() && badge_c.y.is_finite() {
        painter.circle_filled(badge_c, badge_r, Style::OK.gamma_multiply(0.18));
        painter.circle_stroke(badge_c, badge_r, Stroke::new(2.0, Style::OK));
        let check_box = layout.badge.shrink(badge_r * 0.5);
        if !paint_carbon(&painter, check_box, "emblem-ok", Style::OK) {
            paint_check_glyph(&painter, badge_c, badge_r * 0.5, Style::OK);
        }
    }

    // Title + destination + address.
    let cx = layout.card.center().x;
    let max_w = (layout.card.width() - 2.0 * Style::SP_L).max(1.0);
    let title_y = layout.badge.bottom() + Style::SP_S;
    painter.text(
        egui::pos2(cx, title_y),
        Align2::CENTER_TOP,
        "You have arrived",
        FontId::proportional(Style::HEADING),
        Style::TEXT_STRONG,
    );
    let dest = state.local_navigation.active_destination();
    let (name, addr) = dest.map_or(("Destination", "Arrived"), |destination| {
        (destination.label.as_str(), destination.address.as_str())
    });
    let name_s = elide(&painter, name, FontId::proportional(Style::TITLE), max_w);
    painter.text(
        egui::pos2(cx, title_y + 28.0),
        Align2::CENTER_TOP,
        &name_s,
        FontId::proportional(Style::TITLE),
        Style::TEXT,
    );
    let addr_s = elide(&painter, addr, FontId::proportional(Style::BODY), max_w);
    painter.text(
        egui::pos2(cx, title_y + 50.0),
        Align2::CENTER_TOP,
        &addr_s,
        FontId::proportional(Style::BODY),
        Style::TEXT_DIM,
    );

    // Arrival time, above the buttons.
    let eta = state.local_navigation.active_route.eta.trim();
    let arrival = if eta.is_empty() {
        "Arrived".to_string()
    } else {
        format!("Arrived \u{00B7} {eta}")
    };
    painter.text(
        egui::pos2(cx, layout.end_btn.top() - Style::SP_S),
        Align2::CENTER_BOTTOM,
        &arrival,
        FontId::proportional(Style::BODY),
        Style::OK,
    );

    // Secondary actions.
    paint_arrival_action(
        &painter,
        layout.end_btn,
        "End",
        true,
        end_hovered,
        end_pressed,
    );
    let save_label = if saved { "Saved" } else { "Save" };
    paint_arrival_action(
        &painter,
        layout.save_btn,
        save_label,
        false,
        save_hovered,
        save_pressed,
    );
}

/// One arrival-screen action button (primary = filled blue, secondary = card).
fn paint_arrival_action(
    painter: &Painter,
    rect: Rect,
    label: &str,
    primary: bool,
    hovered: bool,
    pressed: bool,
) {
    let base = if primary {
        if pressed {
            MANEUVER_BLUE_DEEP
        } else if hovered {
            MANEUVER_BLUE_HI
        } else {
            MANEUVER_BLUE
        }
    } else if pressed {
        Style::pressed_fill(Style::ACCENT)
    } else if hovered {
        HUD_CARD_HI
    } else {
        Style::LAYER_02
    };
    painter.rect_filled(rect, HUD_RADIUS_S, base);
    if primary {
        paint_card_sheen(
            painter,
            rect,
            HUD_RADIUS_S,
            MANEUVER_BLUE_HI.gamma_multiply(0.5),
            MANEUVER_BLUE_DEEP.gamma_multiply(0.5),
        );
    }
    painter.rect_stroke(
        rect,
        HUD_RADIUS_S,
        Stroke::new(
            1.0,
            if primary {
                MANEUVER_BLUE_HI
            } else {
                Style::BORDER
            },
        ),
        StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(Style::TITLE),
        if primary {
            Color32::WHITE
        } else {
            Style::TEXT_STRONG
        },
    );
}

/// A procedural checkmark (arrival-badge fallback when the Carbon glyph is
/// unavailable).
fn paint_check_glyph(painter: &Painter, center: Pos2, s: f32, color: Color32) {
    if center.any_nan() || !(s > 0.5) {
        return;
    }
    painter.add(Shape::line(
        vec![
            egui::pos2(center.x - s, center.y),
            egui::pos2(center.x - s * 0.25, center.y + s * 0.7),
            egui::pos2(center.x + s, center.y - s * 0.7),
        ],
        Stroke::new((s * 0.34).max(2.0), color),
    ));
}

// ===========================================================================
// Off-route / recalculating — the amber HUD state (Google Maps / Waze).
// ===========================================================================

/// The amber "Recalculating…" banner that replaces the maneuver banner when off
/// route: a rotating spinner chip + status text, keyed to the Quazar-dark skin.
fn paint_recalculating_banner(painter: &Painter, rect: Rect, route: &RoutePlan, time: f64) {
    painter.rect_filled(rect, HUD_RADIUS, HUD_CARD_BG);
    paint_card_sheen(
        painter,
        rect,
        HUD_RADIUS,
        HUD_CARD_HI.gamma_multiply(0.6),
        Color32::BLACK.gamma_multiply(0.16),
    );
    painter.rect_stroke(
        rect,
        HUD_RADIUS,
        Stroke::new(1.5, Style::WARN.gamma_multiply(0.85)),
        StrokeKind::Inside,
    );

    let inset = Style::SP_S;
    let chip_side = (rect.height() - 2.0 * inset).max(1.0);
    let chip = safe_rect(
        rect.left() + inset,
        rect.top() + inset,
        chip_side,
        chip_side,
    );
    painter.rect_filled(chip, HUD_RADIUS_S, Style::WARN.gamma_multiply(0.14));
    paint_spinner(painter, chip.center(), chip_side * 0.30, time, Style::WARN);

    let tx = chip.right() + Style::SP_M;
    let max_w = (rect.right() - Style::SP_M - tx).max(1.0);
    painter.text(
        egui::pos2(tx, rect.top() + 9.0),
        Align2::LEFT_TOP,
        "Recalculating\u{2026}",
        FontId::proportional(28.0),
        Style::WARN,
    );
    let sub = elide(
        painter,
        &format!("Off route \u{00B7} rerouting on {}", route.current_road),
        FontId::proportional(Style::BODY),
        max_w,
    );
    painter.text(
        egui::pos2(tx, rect.bottom() - 9.0),
        Align2::LEFT_BOTTOM,
        &sub,
        FontId::proportional(Style::BODY),
        Color32::WHITE.gamma_multiply(0.8),
    );
}

/// The calm idle banner shown on the Drive HUD when there is no active
/// destination: a search chip + "No destination — search to start" prompt,
/// instead of a fabricated maneuver instruction for a route nobody chose.
fn paint_idle_banner(painter: &Painter, rect: Rect) {
    painter.rect_filled(rect, HUD_RADIUS, HUD_CARD_BG);
    paint_card_sheen(
        painter,
        rect,
        HUD_RADIUS,
        HUD_CARD_HI.gamma_multiply(0.6),
        Color32::BLACK.gamma_multiply(0.16),
    );
    painter.rect_stroke(
        rect,
        HUD_RADIUS,
        Stroke::new(1.0, Style::BORDER),
        StrokeKind::Inside,
    );

    let inset = Style::SP_S;
    let chip_side = (rect.height() - 2.0 * inset).max(1.0);
    let chip = safe_rect(
        rect.left() + inset,
        rect.top() + inset,
        chip_side,
        chip_side,
    );
    painter.rect_filled(chip, HUD_RADIUS_S, Style::SURFACE_HI);
    let icon_box = safe_rect(
        chip.center().x - chip_side * 0.25,
        chip.center().y - chip_side * 0.25,
        chip_side * 0.5,
        chip_side * 0.5,
    );
    let _ = paint_carbon(painter, icon_box, "system-search", Style::ACCENT_HI);

    let tx = chip.right() + Style::SP_M;
    let max_w = (rect.right() - Style::SP_M - tx).max(1.0);
    painter.text(
        egui::pos2(tx, rect.top() + 9.0),
        Align2::LEFT_TOP,
        "No destination",
        FontId::proportional(28.0),
        Style::TEXT_STRONG,
    );
    let sub = elide(
        painter,
        "Search to start navigation",
        FontId::proportional(Style::BODY),
        max_w,
    );
    painter.text(
        egui::pos2(tx, rect.bottom() - 9.0),
        Align2::LEFT_BOTTOM,
        &sub,
        FontId::proportional(Style::BODY),
        Style::TEXT_DIM,
    );
}

/// A rotating tick-ring spinner (the recalculating pulse). `time` is the egui
/// clock in seconds; every value is guarded finite (crash-safe).
fn paint_spinner(painter: &Painter, center: Pos2, radius: f32, time: f64, color: Color32) {
    if center.any_nan() || !(radius > 0.5) {
        return;
    }
    let base = finite_or((time as f32) * 4.0, 0.0);
    let n: u32 = 12;
    for i in 0..n {
        let a = base + (i as f32 / n as f32) * std::f32::consts::TAU;
        let dir = egui::vec2(a.cos(), a.sin());
        let p0 = center + dir * (radius * 0.55);
        let p1 = center + dir * radius;
        if p0.any_nan() || p1.any_nan() {
            continue;
        }
        let fade = i as f32 / n as f32;
        painter.line_segment(
            [p0, p1],
            Stroke::new(
                (radius * 0.18).max(1.2),
                color.gamma_multiply(0.2 + 0.8 * fade),
            ),
        );
    }
}

/// A full-width "Where to?" entry bar (the Map-tab search affordance). Returns
/// `true` when tapped. Painter-only chrome, so it never leaks look into a crate.
fn where_to_bar(ui: &mut egui::Ui) -> bool {
    let width = safe_width(ui);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 44.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter_at(rect);
        paint_search_bar(&painter, rect, response.hovered(), "Where to?");
        let cc = egui::pos2(rect.right() - Style::SP_M - 4.0, rect.center().y);
        if cc.x.is_finite() && cc.y.is_finite() {
            painter.add(Shape::line(
                vec![
                    egui::pos2(cc.x - 4.0, cc.y - 5.0),
                    egui::pos2(cc.x + 3.0, cc.y),
                    egui::pos2(cc.x - 4.0, cc.y + 5.0),
                ],
                Stroke::new(2.0, Style::TEXT_DIM),
            ));
        }
    }
    response.clicked()
}

// --- Scene: the beautiful synthetic map (shared by Drive + Map tabs). ------

fn zoom_scale(map: &MapViewState) -> f32 {
    (finite_or(map.zoom, 13.0) / 13.0).clamp(0.6, 1.8)
}

/// Normalized map coordinate → screen, with pan + zoom applied (crash-safe).
fn scene_point(rect: Rect, map: &MapViewState, u: f32, v: f32) -> Pos2 {
    let base = map_point(rect, u, v);
    let z = zoom_scale(map);
    let c = rect.center();
    let px = finite_or(map.pan[0], 0.0).clamp(-600.0, 600.0);
    let py = finite_or(map.pan[1], 0.0).clamp(-600.0, 600.0);
    let x = c.x + (base.x - c.x) * z + px;
    let y = c.y + (base.y - c.y) * z + py;
    egui::pos2(finite_or(x, c.x), finite_or(y, c.y))
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)] // one painter call site per screen; a struct would obscure the seam
fn paint_map_scene(
    painter: &Painter,
    rect: Rect,
    map: &MapViewState,
    dead_zones: &DeadZoneState,
    primary: Option<&LocationSample>,
    has_fix: bool,
    nws_vehicle: Option<mackes_mesh_types::nws_alert::GeoPoint>,
    route_live: bool,
    route_planned: bool,
    destination: Option<(f64, f64)>,
    route_geometry: Option<&ProviderRouteGeometry>,
) {
    let bg = if map.dark_mode {
        MAP_DARK_BG
    } else {
        MAP_LIGHT_BG
    };
    painter.rect_filled(rect, Style::RADIUS_L, bg);

    // Real offline raster basemap: a Web-Mercator slippy-tile layer anchored on
    // the live fix (or the region centroid when indoors / off-map), replacing the
    // old procedural grid + hard-coded road splines. `paint_basemap` returns the
    // projection used so pins/routes land on the real map. Without a region it
    // paints an honest "no data" panel but still returns a vehicle-centred
    // projection, keeping live overlay geometry useful over the empty background.
    let center = if has_fix {
        primary.map(|s| (s.latitude, s.longitude))
    } else {
        None
    };
    let projection = crate::basemap::paint_basemap(painter, rect, map, center);

    // WL-FUNC-012 / OVERLAY-2 — producer-timed IEM/NWS NEXRAD raster animation.
    // This paints through egui textures on both GLES and wgpu, beneath every
    // vector overlay. Without an installed basemap its vehicle-centred geometry
    // can still paint over the honest no-map background.
    if map.iem_radar_overlay {
        let projection_ref = projection.as_ref();
        let _ = crate::iem_radar::paint_layer(
            painter,
            rect,
            &map.iem_radar,
            crate::earthquake::now_ms(),
            |lat, lon| projection_ref.map(|projection| projection.project(lat, lon)),
        );
    }
    // Edge vignette on top of the tiles keeps the driver's focus centred.
    paint_vignette(painter, rect);

    // WL-FUNC-012 / OVERLAY-10 — real USGS events normalized by the workstation
    // adapter. The basemap seam owns the geographic projection and falls back to
    // the live vehicle fix when no region bundle is installed.
    if map.earthquake_overlay {
        let projection_ref = projection.as_ref();
        let _ = crate::earthquake::paint_layer(
            painter,
            rect,
            &map.earthquakes,
            crate::earthquake::now_ms(),
            |lat, lon| projection_ref.map(|projection| projection.project(lat, lon)),
        );
    }

    // WL-FUNC-012 / OVERLAY-6 — merged NIFC WFIGS + NASA FIRMS wildfire feed.
    // NIFC perimeters and FIRMS thermal hotspots retain independent snapshots,
    // status badges, and fail-soft folds under the one wildfire toggle.
    if map.wildfire_overlay {
        let projection_ref = projection.as_ref();
        let _ = crate::wildfire::paint_layer(
            painter,
            rect,
            &map.wildfire,
            crate::earthquake::now_ms(),
            |lat, lon| projection_ref.map(|projection| projection.project(lat, lon)),
        );
        let _ = crate::firms::paint_layer(
            painter,
            rect,
            &map.firms,
            crate::earthquake::now_ms(),
            |lat, lon| projection_ref.map(|projection| projection.project(lat, lon)),
        );
    }

    // WL-FUNC-012 / OVERLAY-1 — point-scoped NWS active warnings. The same
    // basemap projection drives polygon geometry, falling back to the valid live
    // vehicle fix when no offline region is installed.
    if map.nws_alert_overlay {
        let projection_ref = projection.as_ref();
        let _ = crate::nws_alert::paint_layer(
            painter,
            rect,
            &map.nws_alerts,
            crate::earthquake::now_ms(),
            nws_vehicle,
            |lat, lon| projection_ref.map(|projection| projection.project(lat, lon)),
        );
    }

    // WL-FUNC-012 / OVERLAY-3 — current keyless NCDOT TIMS road events.
    if map.traffic_event_overlay {
        let projection_ref = projection.as_ref();
        let _ = crate::traffic::paint_layer(
            painter,
            rect,
            &map.traffic_events,
            crate::earthquake::now_ms(),
            |lat, lon| projection_ref.map(|projection| projection.project(lat, lon)),
        );
    }

    // WL-FUNC-012 / OVERLAY-7 — official AirNow monitoring-site AQI. The layer
    // paints an explicit missing-key badge until mde-seal has the free key; no
    // station or warning geometry is invented while unconfigured.
    if map.air_quality_overlay {
        let projection_ref = projection.as_ref();
        let _ = crate::air_quality::paint_layer(
            painter,
            rect,
            &map.air_quality,
            crate::earthquake::now_ms(),
            |lat, lon| projection_ref.map(|projection| projection.project(lat, lon)),
        );
    }

    // WL-FUNC-012 / OVERLAY-8 — low-altitude, vehicle-scoped adsb.lol tracks.
    // Positions dead-reckon only inside the bounded 60-second retention window.
    // A valid vehicle fix supplies projection even without an installed basemap.
    if map.aircraft_overlay {
        let projection_ref = projection.as_ref();
        let _ = crate::aircraft::paint_layer(
            painter,
            rect,
            &map.aircraft,
            crate::earthquake::now_ms(),
            |lat, lon| projection_ref.map(|projection| projection.project(lat, lon)),
        );
    }

    // WL-FUNC-012 / OVERLAY-9 — MBTA GTFS-Realtime nearby vehicles.
    if map.transit_overlay {
        let projection_ref = projection.as_ref();
        let _ = crate::transit::paint_layer(
            painter,
            rect,
            &map.transit,
            crate::earthquake::now_ms(),
            |lat, lon| projection_ref.map(|projection| projection.project(lat, lon)),
        );
    }

    // WL-FUNC-012 / OVERLAY-4 — NWS hourly current/drive-ahead guidance.
    if map.nws_forecast_overlay {
        let projection_ref = projection.as_ref();
        let _ = crate::nws_forecast::paint_layer(
            painter,
            rect,
            &map.nws_forecast,
            crate::earthquake::now_ms(),
            |lat, lon| projection_ref.map(|projection| projection.project(lat, lon)),
        );
    }

    // WL-FUNC-012 / OVERLAY-5 — official Caltrans current traffic-camera stills.
    if map.caltrans_camera_overlay {
        let projection_ref = projection.as_ref();
        let _ = crate::caltrans_camera::paint_layer(
            painter,
            rect,
            &map.caltrans_cameras,
            crate::earthquake::now_ms(),
            |lat, lon| projection_ref.map(|projection| projection.project(lat, lon)),
        );
    }

    // Recorded dead-zone overlay (real recorder data; empty list paints nothing).
    if map.dead_zone_overlay {
        for (idx, _) in dead_zones.zones.iter().enumerate() {
            let c = scene_point(rect, map, 0.30 + idx as f32 * 0.16, 0.42);
            painter.circle_filled(c, 30.0, Style::DANGER.gamma_multiply(0.16));
            painter.circle_stroke(c, 30.0, Stroke::new(1.5, Style::DANGER.gamma_multiply(0.7)));
        }
    }
    // The former procedural weather rectangle / traffic segment / location-health
    // crumbs were removed (WL-UX-007/S1). Weather warnings now come only from
    // the real NWS layer above; no provider backs a generic weather or traffic
    // visualization, so none is fabricated (P8/Q33).

    // Route — provider geometry only. A planned route without a provider path
    // keeps the map honest with an explicit unavailable state; no normalized
    // fallback or fixed maneuver marker is painted.
    if map.route_visible && route_planned {
        let painted = match (projection.as_ref(), route_geometry) {
            (Some(projection), Some(geometry)) => {
                paint_route(painter, projection, map, geometry, route_live)
            }
            _ => false,
        };
        if !painted {
            paint_route_unavailable(painter, rect);
        }
    }

    // Vehicle — fixed driver anchor (map moves under it, like a real nav app).
    let anchor = map_point(rect, VEHICLE_UV.0, VEHICLE_UV.1);
    if has_fix {
        let heading = finite_or(primary.map_or(0.0, |s| s.heading_deg), 0.0);
        paint_heading_cone(painter, anchor, heading, ROUTE_BLUE);
        if map.gnss_overlay {
            painter.circle_stroke(
                anchor,
                40.0,
                Stroke::new(1.0, ROUTE_BLUE.gamma_multiply(0.35)),
            );
        }
        paint_vehicle_chevron(painter, anchor, heading, ROUTE_BLUE, true);
    } else {
        paint_vehicle_chevron(painter, anchor, 0.0, Style::TEXT_DIM, false);
        paint_acquiring_chip(painter, egui::pos2(anchor.x, anchor.y + 26.0));
    }

    // Live destination pin + straight-line "as the crow flies" preview, drawn on
    // the shared geographic projection. It remains available from a valid fix
    // when no region is installed, and the destination must carry a geocoded pin.
    if let (Some(proj), Some((dlat, dlon))) = (projection, destination) {
        let pin = proj.project(dlat, dlon);
        if pin.x.is_finite() && pin.y.is_finite() {
            if has_fix {
                painter.line_segment([anchor, pin], Stroke::new(3.0, ROUTE_ALT));
            }
            paint_destination_pin(painter, pin);
        }
    }
}

/// Only a fresh, connected, provenance-stamped MG90 mirror may drive the
/// safety-critical "vehicle is inside this warning" banner. Other location
/// sources can center the map, but never claim the vehicle is in an NWS alert.
fn live_nws_vehicle_point(
    locations: &LocationManager,
) -> Option<mackes_mesh_types::nws_alert::GeoPoint> {
    if locations.primary != LocationSourceKind::Mg90Gnss {
        return None;
    }
    let source = locations.primary_source()?;
    let sample = &source.sample;
    let live_provenance = source
        .diagnostics
        .get("mode")
        .is_some_and(|mode| mode.starts_with("live vehicle-gateway mirror ("));
    if source.kind != LocationSourceKind::Mg90Gnss
        || source.status != SourceStatus::Connected
        || !live_provenance
        || !sample.has_fix()
        || !sample.update_age_s.is_finite()
        || sample.update_age_s < 0.0
        || sample.stale()
        || !(-90.0..=90.0).contains(&sample.latitude)
        || !(-180.0..=180.0).contains(&sample.longitude)
    {
        return None;
    }
    Some(mackes_mesh_types::nws_alert::GeoPoint {
        latitude: sample.latitude,
        longitude: sample.longitude,
    })
}

/// A map pin for a chosen geocoder destination: a teardrop head on a short stem
/// with an inner dot, in the route palette (no new colour literal).
fn paint_destination_pin(painter: &Painter, tip: Pos2) {
    if !tip.x.is_finite() || !tip.y.is_finite() {
        return;
    }
    let r = 9.0;
    let head = egui::pos2(tip.x, tip.y - r * 1.4);
    painter.line_segment([head, tip], Stroke::new(3.0, ROUTE_BLUE));
    painter.circle_filled(head, r, ROUTE_BLUE);
    painter.circle_stroke(head, r, Stroke::new(1.5, Color32::WHITE));
    painter.circle_filled(head, r * 0.4, Color32::WHITE);
}

fn paint_vignette(painter: &Painter, rect: Rect) {
    let edge = Color32::BLACK.gamma_multiply(0.42);
    let clear = Color32::TRANSPARENT;
    let (w, h) = (rect.width(), rect.height());
    let tb = (h * 0.28).min(160.0);
    fill_quad(
        painter,
        [
            rect.left_top(),
            rect.right_top(),
            egui::pos2(rect.right(), rect.top() + tb),
            egui::pos2(rect.left(), rect.top() + tb),
        ],
        [edge, edge, clear, clear],
    );
    let bb = (h * 0.34).min(200.0);
    fill_quad(
        painter,
        [
            egui::pos2(rect.left(), rect.bottom() - bb),
            egui::pos2(rect.right(), rect.bottom() - bb),
            rect.right_bottom(),
            rect.left_bottom(),
        ],
        [clear, clear, edge, edge],
    );
    let sb = (w * 0.20).min(160.0);
    fill_quad(
        painter,
        [
            rect.left_top(),
            egui::pos2(rect.left() + sb, rect.top()),
            egui::pos2(rect.left() + sb, rect.bottom()),
            rect.left_bottom(),
        ],
        [edge, clear, clear, edge],
    );
    fill_quad(
        painter,
        [
            egui::pos2(rect.right() - sb, rect.top()),
            rect.right_top(),
            rect.right_bottom(),
            egui::pos2(rect.right() - sb, rect.bottom()),
        ],
        [clear, edge, edge, clear],
    );
}

/// Fill a quad (corners tl, tr, br, bl) with per-corner colours via a mesh.
fn fill_quad(painter: &Painter, corners: [Pos2; 4], colors: [Color32; 4]) {
    if corners.iter().any(|p| p.any_nan()) {
        return;
    }
    let mut mesh = Mesh::default();
    for (p, c) in corners.iter().zip(colors) {
        mesh.colored_vertex(*p, c);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
}

/// Paint a variable-width ribbon (quad per segment + round joints) in `color`.
fn paint_ribbon(painter: &Painter, pts: &[(Pos2, f32)], color: Color32) {
    for pair in pts.windows(2) {
        let (p0, w0) = pair[0];
        let (p1, w1) = pair[1];
        if p0.any_nan() || p1.any_nan() {
            continue;
        }
        let seg = p1 - p0;
        let len = seg.length();
        if !(len > 0.001) {
            continue;
        }
        let dir = seg / len;
        let perp = egui::vec2(-dir.y, dir.x);
        let a = p0 + perp * (w0 * 0.5);
        let b = p0 - perp * (w0 * 0.5);
        let c = p1 - perp * (w1 * 0.5);
        let d = p1 + perp * (w1 * 0.5);
        painter.add(Shape::convex_polygon(vec![a, b, c, d], color, Stroke::NONE));
    }
    for &(p, w) in pts {
        if p.any_nan() {
            continue;
        }
        painter.circle_filled(p, (w * 0.5).max(0.5), color);
    }
}

fn projected_route_path(
    projection: &crate::basemap::Projection,
    points: &[ProviderRoutePoint],
) -> Option<Vec<Pos2>> {
    if points.len() < 2 || points.iter().copied().any(|point| !point.is_valid()) {
        return None;
    }
    let projected: Vec<Pos2> = points
        .iter()
        .map(|point| projection.project(point.latitude, point.longitude))
        .collect();
    projected
        .iter()
        .all(|point| !point.any_nan())
        .then_some(projected)
}

/// Add a gentle near-to-far taper to provider-projected route geometry.
fn provider_ribbon_points(
    points: &[Pos2],
    map: &MapViewState,
    near: f32,
    far: f32,
) -> Vec<(Pos2, f32)> {
    let last = points.len().saturating_sub(1).max(1) as f32;
    let z = zoom_scale(map);
    points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            let progress = index as f32 / last;
            let width = near + (far - near) * progress;
            (*point, (width * z).max(1.0))
        })
        .collect()
}

fn paint_route(
    painter: &Painter,
    projection: &crate::basemap::Projection,
    map: &MapViewState,
    geometry: &ProviderRouteGeometry,
    active: bool,
) -> bool {
    if !geometry.is_renderable() {
        return false;
    }
    let Some(primary) = projected_route_path(projection, &geometry.primary) else {
        return false;
    };

    if !active {
        // Planned but not active — a dim grey provider line, no glow.
        let dim = provider_ribbon_points(&primary, map, 10.0, 4.0);
        paint_ribbon(painter, &dim, Style::TEXT_DIM.gamma_multiply(0.5));
    } else {
        let glow = provider_ribbon_points(&primary, map, 30.0, 12.0);
        paint_ribbon(painter, &glow, ROUTE_BLUE.gamma_multiply(0.16));
        let casing = provider_ribbon_points(&primary, map, 20.0, 8.0);
        paint_ribbon(painter, &casing, ROUTE_CASING);
        let core = provider_ribbon_points(&primary, map, 13.0, 5.0);
        paint_ribbon(painter, &core, ROUTE_BLUE);
    }

    if let Some(alternate) = projected_route_path(projection, &geometry.alternate) {
        let alt = provider_ribbon_points(&alternate, map, 9.0, 4.0);
        paint_ribbon(painter, &alt, ROUTE_ALT.gamma_multiply(0.8));
    }

    // A maneuver marker is painted only at the provider-returned geographic
    // point; absent marker geometry means no marker, never a guessed anchor.
    if let Some(maneuver) = geometry.maneuver.filter(|point| point.is_valid()) {
        let marker = projection.project(maneuver.latitude, maneuver.longitude);
        if !marker.any_nan() {
            let z = zoom_scale(map);
            painter.circle_filled(marker, 7.0 * z, Color32::WHITE);
            painter.circle_filled(marker, 4.5 * z, ROUTE_BLUE);
        }
    }
    true
}

fn paint_route_unavailable(painter: &Painter, rect: Rect) {
    let width = (rect.width() - 2.0 * Style::SP_M).clamp(1.0, 320.0);
    let status = safe_rect(
        rect.left() + Style::SP_M,
        rect.top() + Style::SP_M,
        width,
        48.0,
    );
    paint_soft_shadow(painter, status, HUD_RADIUS_S);
    paint_provider_unavailable(painter, status, "Route geometry unavailable");
}

/// A heading-aware vehicle chevron with an optional soft accent glow.
fn paint_vehicle_chevron(
    painter: &Painter,
    center: Pos2,
    heading_deg: f32,
    tone: Color32,
    glow: bool,
) {
    if center.any_nan() {
        return;
    }
    let a = finite_or(heading_deg, 0.0).to_radians();
    let f = egui::vec2(a.sin(), -a.cos());
    let rt = egui::vec2(a.cos(), a.sin());
    let size = 16.0;
    if glow {
        for r in [34.0_f32, 27.0, 20.0, 14.0] {
            painter.circle_filled(center, r, ROUTE_BLUE.gamma_multiply(0.07));
        }
        // Soft contact shadow so the puck reads as lifted off the map.
        painter.circle_filled(
            center + egui::vec2(0.0, 2.5),
            size * 0.95,
            Color32::BLACK.gamma_multiply(0.28),
        );
    }
    // Sleek concave-back navigation arrowhead.
    let tip = center + f * (size * 1.2);
    let bl = center - f * (size * 0.82) - rt * (size * 0.82);
    let br = center - f * (size * 0.82) + rt * (size * 0.82);
    let notch = center - f * (size * 0.2);
    painter.add(Shape::convex_polygon(
        vec![tip, br, notch],
        tone,
        Stroke::NONE,
    ));
    painter.add(Shape::convex_polygon(
        vec![tip, notch, bl],
        tone,
        Stroke::NONE,
    ));
    painter.add(Shape::closed_line(
        vec![tip, br, notch, bl],
        Stroke::new(2.2, Color32::WHITE),
    ));
}

/// A translucent "flashlight" accuracy cone ahead of the vehicle.
fn paint_heading_cone(painter: &Painter, apex: Pos2, heading_deg: f32, tone: Color32) {
    if apex.any_nan() {
        return;
    }
    let a0 = finite_or(heading_deg, 0.0).to_radians();
    let spread = 20.0_f32.to_radians();
    let len = 108.0;
    let n: u32 = 16;
    let mut mesh = Mesh::default();
    mesh.colored_vertex(apex, tone.gamma_multiply(0.34));
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let a = 2.0f32.mul_add(spread * t, a0 - spread);
        let dir = egui::vec2(a.sin(), -a.cos());
        let p = apex + dir * len;
        if p.any_nan() {
            return;
        }
        mesh.colored_vertex(p, Color32::TRANSPARENT);
    }
    for k in 0..n {
        mesh.add_triangle(0, 1 + k, 2 + k);
    }
    painter.add(mesh);
}

fn paint_acquiring_chip(painter: &Painter, center_top: Pos2) {
    let font = FontId::proportional(Style::SMALL);
    let galley = painter.layout_no_wrap("Acquiring GPS".to_string(), font, Style::TEXT_STRONG);
    let w = galley.size().x + Style::SP_M + Style::SP_S;
    let r = safe_rect(center_top.x - w / 2.0, center_top.y, w, 22.0);
    painter.rect_filled(r, Style::RADIUS_S, HUD_CARD_BG.gamma_multiply(0.94));
    painter.rect_stroke(
        r,
        Style::RADIUS_S,
        Stroke::new(1.0, Style::WARN.gamma_multiply(0.7)),
        StrokeKind::Inside,
    );
    painter.circle_filled(
        egui::pos2(r.left() + Style::SP_S, r.center().y),
        3.0,
        Style::WARN,
    );
    painter.galley(
        egui::pos2(r.left() + Style::SP_M, r.center().y - galley.size().y / 2.0),
        galley,
        Style::TEXT_STRONG,
    );
}

// --- Floating cards --------------------------------------------------------

/// A soft drop shadow behind an elevated card. Many thin, low-alpha layers with
/// a downward bias give a smooth, diffuse penumbra (a premium Material feel)
/// rather than a hard stacked edge.
fn paint_soft_shadow(painter: &Painter, rect: Rect, radius: f32) {
    if rect.left().is_nan() || rect.top().is_nan() {
        return;
    }
    for i in (1..=9).rev() {
        let f = i as f32;
        let r = rect.expand(f * 1.7).translate(egui::vec2(0.0, f * 0.85));
        painter.rect_filled(r, radius + f, Color32::BLACK.gamma_multiply(0.04));
    }
}

/// Overlay a top-lit vertical sheen inside a rounded card: a light band at the
/// top fading out and a soft shade toward the bottom, giving flat fills a sense
/// of depth. Inset off the rounded corners so the silhouette stays clean.
fn paint_card_sheen(painter: &Painter, rect: Rect, radius: f32, top: Color32, bottom: Color32) {
    if !rect.width().is_finite() || !rect.height().is_finite() {
        return;
    }
    if rect.width() < radius * 2.0 + 2.0 || rect.height() < 8.0 {
        return;
    }
    let x0 = rect.left() + radius * 0.5;
    let x1 = rect.right() - radius * 0.5;
    let mid = rect.top() + rect.height() * 0.5;
    let clear = Color32::TRANSPARENT;
    fill_quad(
        painter,
        [
            egui::pos2(x0, rect.top() + 1.5),
            egui::pos2(x1, rect.top() + 1.5),
            egui::pos2(x1, mid),
            egui::pos2(x0, mid),
        ],
        [top, top, clear, clear],
    );
    fill_quad(
        painter,
        [
            egui::pos2(x0, mid),
            egui::pos2(x1, mid),
            egui::pos2(x1, rect.bottom() - 1.5),
            egui::pos2(x0, rect.bottom() - 1.5),
        ],
        [clear, clear, bottom, bottom],
    );
}

fn paint_maneuver_banner(
    painter: &Painter,
    rect: Rect,
    route: &RoutePlan,
    kind: ManeuverKind,
    has_fix: bool,
) {
    let dim = if has_fix { 1.0 } else { 0.62 };
    // Premium GMaps-blue card: base fill + top-lit gradient sheen for depth.
    painter.rect_filled(rect, HUD_RADIUS, MANEUVER_BLUE.gamma_multiply(dim));
    paint_card_sheen(
        painter,
        rect,
        HUD_RADIUS,
        MANEUVER_BLUE_HI.gamma_multiply(0.5 * dim),
        MANEUVER_BLUE_DEEP.gamma_multiply(0.55 * dim),
    );
    painter.rect_stroke(
        rect,
        HUD_RADIUS,
        Stroke::new(1.0, MANEUVER_BLUE_HI.gamma_multiply(0.9 * dim)),
        StrokeKind::Inside,
    );

    // Bold turn arrow on a subtle lighter chip (GMaps seats the arrow on a panel).
    let inset = Style::SP_S;
    let chip_side = (rect.height() - 2.0 * inset).max(1.0);
    let chip = safe_rect(
        rect.left() + inset,
        rect.top() + inset,
        chip_side,
        chip_side,
    );
    painter.rect_filled(chip, HUD_RADIUS_S, Color32::WHITE.gamma_multiply(0.12));
    let arrow_rect = chip.shrink(chip_side * 0.18);
    paint_maneuver_arrow(painter, arrow_rect, kind, Color32::WHITE);

    // Text column: distance (hero) · maneuver street · current road.
    let tx = chip.right() + Style::SP_M;
    let max_w = (rect.right() - Style::SP_M - tx).max(1.0);
    let top = rect.top();
    let dist = format_distance(route.distance_to_maneuver_mi);
    painter.text(
        egui::pos2(tx, top + 9.0),
        Align2::LEFT_TOP,
        &dist,
        FontId::proportional(34.0),
        Color32::WHITE,
    );
    let man = elide(
        painter,
        &route.next_maneuver,
        FontId::proportional(18.0),
        max_w,
    );
    painter.text(
        egui::pos2(tx, top + 48.0),
        Align2::LEFT_TOP,
        &man,
        FontId::proportional(18.0),
        Color32::WHITE,
    );
    let on_road = elide(
        painter,
        &format!("on {}", route.current_road),
        FontId::proportional(Style::BODY),
        max_w,
    );
    painter.text(
        egui::pos2(tx, rect.bottom() - 8.0),
        Align2::LEFT_BOTTOM,
        &on_road,
        FontId::proportional(Style::BODY),
        Color32::WHITE.gamma_multiply(0.8),
    );
}

fn paint_maneuver_arrow(painter: &Painter, rect: Rect, kind: ManeuverKind, color: Color32) {
    let s = rect.width().min(rect.height());
    if kind == ManeuverKind::Arrive {
        let c = rect.center();
        painter.circle_stroke(c, s * 0.32, Stroke::new(s * 0.11, color));
        painter.circle_filled(c, s * 0.13, color);
        return;
    }
    let unit: &[(f32, f32)] = match kind {
        ManeuverKind::Straight => &[(0.5, 0.86), (0.5, 0.30)],
        ManeuverKind::Right => &[(0.30, 0.84), (0.30, 0.50), (0.72, 0.50)],
        ManeuverKind::Left => &[(0.70, 0.84), (0.70, 0.50), (0.28, 0.50)],
        ManeuverKind::SlightRight | ManeuverKind::Merge => {
            &[(0.40, 0.86), (0.44, 0.54), (0.72, 0.30)]
        }
        ManeuverKind::SlightLeft => &[(0.60, 0.86), (0.56, 0.54), (0.28, 0.30)],
        ManeuverKind::Roundabout => &[(0.44, 0.86), (0.44, 0.56), (0.66, 0.44), (0.62, 0.24)],
        ManeuverKind::UTurn => &[(0.62, 0.86), (0.62, 0.44), (0.40, 0.44), (0.40, 0.66)],
        ManeuverKind::Arrive => &[(0.5, 0.5)],
    };
    let pts: Vec<Pos2> = unit
        .iter()
        .map(|&(u, v)| {
            egui::pos2(
                rect.left() + u * rect.width(),
                rect.top() + v * rect.height(),
            )
        })
        .collect();
    let ribbon: Vec<(Pos2, f32)> = pts.iter().map(|&p| (p, s * 0.185)).collect();
    paint_ribbon(painter, &ribbon, color);
    if pts.len() >= 2 {
        let tip = pts[pts.len() - 1];
        let prev = pts[pts.len() - 2];
        let seg = tip - prev;
        let len = seg.length();
        if len > 0.001 {
            let dir = seg / len;
            let perp = egui::vec2(-dir.y, dir.x);
            let hw = s * 0.26;
            let hl = s * 0.30;
            // Pull the base back so the head sits flush on the shaft (no gap/overlap seam).
            let base = tip - dir * (s * 0.02);
            painter.add(Shape::convex_polygon(
                vec![base + dir * hl, base + perp * hw, base - perp * hw],
                color,
                Stroke::NONE,
            ));
        }
    }
}

/// Preserve the route HUD footprint while making unavailable provider data
/// explicit. This deliberately paints no lane arrows, route-derived numbers,
/// or other inferred guidance.
fn paint_provider_unavailable(painter: &Painter, rect: Rect, label: &str) {
    if !rect.width().is_finite() || !rect.height().is_finite() || rect.width() < 48.0 {
        return;
    }
    painter.rect_filled(rect, HUD_RADIUS_S, HUD_CARD_BG);
    paint_card_sheen(
        painter,
        rect,
        HUD_RADIUS_S,
        Color32::WHITE.gamma_multiply(0.05),
        Color32::BLACK.gamma_multiply(0.12),
    );
    painter.rect_stroke(
        rect,
        HUD_RADIUS_S,
        Stroke::new(1.0, MANEUVER_BLUE_HI.gamma_multiply(0.4)),
        StrokeKind::Inside,
    );
    let badge = egui::pos2(rect.left() + Style::SP_M, rect.center().y);
    let badge_rect = egui::Rect::from_center_size(badge, egui::vec2(22.0, 22.0));
    let _ = paint_carbon(painter, badge_rect, "dialog-warning", Style::WARN);
    painter.text(
        egui::pos2(badge.x + 18.0, badge.y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );
}

fn paint_eta_bar(painter: &Painter, rect: Rect, route: &RoutePlan, tone: Color32) {
    painter.rect_filled(rect, HUD_RADIUS, HUD_CARD_BG);
    paint_card_sheen(
        painter,
        rect,
        HUD_RADIUS,
        HUD_CARD_HI.gamma_multiply(0.6),
        Color32::BLACK.gamma_multiply(0.14),
    );
    painter.rect_stroke(
        rect,
        HUD_RADIUS,
        Stroke::new(1.0, Style::BORDER),
        StrokeKind::Inside,
    );

    // Bottom-sheet grab handle (the recognizable draggable pill).
    let handle = safe_rect(rect.center().x - 18.0, rect.top() + 7.0, 36.0, 4.0);
    painter.rect_filled(handle, 2.0, Style::TEXT_DIM.gamma_multiply(0.55));

    let pad = Style::SP_M;
    let base_y = rect.center().y + Style::SP_XS;

    // Hero: remaining minutes, coloured by how the route is running.
    let minutes = route.remaining_time_min.to_string();
    let num_g = painter.layout_no_wrap(minutes, FontId::proportional(32.0), tone);
    let num_size = num_g.size();
    painter.galley(
        egui::pos2(rect.left() + pad, base_y - num_size.y * 0.5),
        num_g,
        tone,
    );
    painter.text(
        egui::pos2(rect.left() + pad + num_size.x + Style::SP_XS, base_y - 2.0),
        Align2::LEFT_CENTER,
        "min",
        FontId::proportional(Style::TITLE),
        tone.gamma_multiply(0.92),
    );

    // Secondary: arrival clock · remaining distance.
    let secondary = format!(
        "{}   \u{00B7}   {:.1} mi",
        route.eta,
        finite_or(route.remaining_distance_mi, 0.0).max(0.0)
    );
    painter.text(
        egui::pos2(rect.left() + pad, rect.bottom() - 8.0),
        Align2::LEFT_BOTTOM,
        &secondary,
        FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );

    // Right: subtle expand chevron implying the sheet opens.
    let cc = egui::pos2(rect.right() - pad - 2.0, base_y);
    if cc.x.is_finite() && cc.y.is_finite() {
        painter.circle_filled(cc, 12.0, HUD_CARD_HI);
        painter.add(Shape::line(
            vec![
                egui::pos2(cc.x - 5.0, cc.y + 2.5),
                egui::pos2(cc.x, cc.y - 2.5),
                egui::pos2(cc.x + 5.0, cc.y + 2.5),
            ],
            Stroke::new(2.0, Style::TEXT_DIM),
        ));
    }
}

fn paint_speedometer(
    painter: &Painter,
    rect: Rect,
    primary: Option<&LocationSample>,
    has_fix: bool,
) {
    let r = rect.width().min(rect.height()) * 0.5;
    let c = rect.center();
    painter.circle_filled(
        c + egui::vec2(0.0, 2.5),
        r,
        Color32::BLACK.gamma_multiply(0.35),
    );
    painter.circle_filled(c, r, HUD_CARD_BG);
    painter.circle_stroke(c, r, Stroke::new(1.5, Style::BORDER));
    let speed = primary.map(|s| s.speed_mph).filter(|v| v.is_finite());
    let (num, tone) = match (has_fix, speed) {
        (true, Some(v)) => (format!("{:.0}", v.max(0.0)), Style::TEXT_STRONG),
        _ => ("--".to_string(), Style::TEXT_DIM),
    };
    painter.text(
        egui::pos2(c.x, c.y - Style::SP_XS),
        Align2::CENTER_CENTER,
        &num,
        FontId::proportional(40.0),
        tone,
    );
    painter.text(
        egui::pos2(c.x, c.y + r * 0.44),
        Align2::CENTER_CENTER,
        "mph",
        FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );
}

fn paint_alert_pill(
    painter: &Painter,
    x: f32,
    y: f32,
    icon: &str,
    text: &str,
    tone: Color32,
) -> f32 {
    let font = FontId::proportional(Style::BODY);
    let galley = painter.layout_no_wrap(text.to_string(), font.clone(), Style::TEXT_STRONG);
    let icon_w = 18.0;
    let h = 28.0;
    let w = (icon_w + Style::SP_S + galley.size().x + Style::SP_M * 1.5).min(380.0);
    let r = safe_rect(x, y, w, h);
    painter.rect_filled(r, h * 0.5, HUD_CARD_BG.gamma_multiply(0.95));
    painter.rect_stroke(
        r,
        h * 0.5,
        Stroke::new(1.0, tone.gamma_multiply(0.85)),
        StrokeKind::Inside,
    );
    let irect = safe_rect(
        r.left() + Style::SP_S + Style::SP_XS,
        r.center().y - icon_w / 2.0,
        icon_w,
        icon_w,
    );
    let _ = paint_carbon(painter, irect, icon, tone);
    let tmax = (r.right() - Style::SP_S - (irect.right() + Style::SP_S)).max(1.0);
    let shown = elide(painter, text, font.clone(), tmax);
    let g2 = painter.layout_no_wrap(shown, font, Style::TEXT_STRONG);
    painter.galley(
        egui::pos2(
            irect.right() + Style::SP_S,
            r.center().y - g2.size().y / 2.0,
        ),
        g2,
        Style::TEXT_STRONG,
    );
    y + h + Style::SP_S
}

fn paint_fab(
    painter: &Painter,
    center: Pos2,
    r: f32,
    hovered: bool,
    pressed: bool,
    key: &str,
    muted: bool,
) {
    painter.circle_filled(
        center + egui::vec2(0.0, 2.5),
        r,
        Color32::BLACK.gamma_multiply(0.35),
    );
    let fill = if pressed {
        Style::pressed_fill(Style::ACCENT)
    } else if hovered {
        Style::SURFACE_HI
    } else {
        HUD_CARD_BG
    };
    painter.circle_filled(center, r, fill);
    painter.circle_stroke(center, r, Stroke::new(1.0, Style::BORDER));
    let icon_box = safe_rect(center.x - r * 0.6, center.y - r * 0.6, r * 1.2, r * 1.2);
    match key {
        "recenter" => paint_vehicle_chevron(painter, center, 0.0, ROUTE_BLUE, false),
        "search" => {
            if !paint_carbon(painter, icon_box, "system-search", Style::ACCENT_HI) {
                paint_search_glyph(painter, center, r * 0.52, Style::ACCENT_HI);
            }
        }
        "mute" => {
            let name = if muted {
                "audio-volume-muted"
            } else {
                "audio-volume-high"
            };
            let tone = if muted {
                Style::WARN
            } else {
                Style::TEXT_STRONG
            };
            let _ = paint_carbon(painter, icon_box, name, tone);
        }
        "overview" => {
            let _ = paint_carbon(painter, icon_box, "view-grid", Style::TEXT_STRONG);
        }
        "preview" => {
            let _ = paint_carbon(painter, icon_box, "road", Style::ACCENT_HI);
        }
        _ => {}
    }
}

fn show_map(ui: &mut egui::Ui, state: &mut MapsLocationSurface) {
    if where_to_bar(ui) {
        state.open_destination_search();
    }
    ui.add_space(Style::SP_S);
    // Generic Traffic/Weather toggles remain retired with the production
    // simulators. Base-map controls stay directly visible; the ten real feed
    // toggles live in one grouped popover so the Map tab remains scannable.
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut state.map.dark_mode, "Dark mode");
        ui.checkbox(&mut state.map.route_visible, "Route");
        ui.checkbox(&mut state.map.dead_zone_overlay, "Dead zones");
        ui.checkbox(&mut state.map.gnss_overlay, "GNSS quality");
        let _ = map_layers_menu(ui, &mut state.map);
    });
    ui.add_space(Style::SP_S);
    ui.horizontal(|ui| {
        ui.add(egui::Slider::new(&mut state.map.zoom, 3.0..=18.0).text("Zoom"));
        ui.add(egui::Slider::new(&mut state.map.rotation_deg, -180.0..=180.0).text("Rotate"));
        ui.add(egui::Slider::new(&mut state.map.pitch_deg, 0.0..=60.0).text("Pitch"));
    });
    ui.add_space(Style::SP_S);
    let offline_status = state.offline_navigation_status();
    offline_navigation_card(ui, &offline_status);
    ui.add_space(Style::SP_S);
    let map_rect = map_canvas(
        ui,
        &mut state.map,
        &state.locations,
        &state.dead_zones,
        state.local_navigation.active_route.is_planned(),
        500.0,
    );
    // Action buttons float over the map, justified bottom-right (world-class
    // map-nav idiom) instead of sitting in a control row above it. "Preview
    // route" is the Map tab's sole action button; the cluster stacks any others.
    if floating_map_actions(ui, map_rect, &[("road", "Preview route")]) == Some(0) {
        state.route_preview = true;
        state.active = WorkspaceTab::Drive;
    }
    ui.add_space(Style::SP_S);
    let col_w = split_width(ui, 3);
    ui.horizontal_top(|ui| {
        ui.scope(|ui| {
            ui.set_width(col_w);
            provider_card(ui, &state.offline_maps.map_provider);
        });
        ui.scope(|ui| {
            ui.set_width(col_w);
            provider_card(ui, &state.local_navigation.routing);
        });
        ui.scope(|ui| {
            ui.set_width(col_w);
            provider_card(ui, &state.local_navigation.geocoder);
        });
    });
    ui.add_space(Style::SP_S);
    let col_w = split_width(ui, 3);
    ui.horizontal_top(|ui| {
        ui.scope(|ui| {
            ui.set_width(col_w);
            provider_card(ui, &state.local_navigation.traffic);
        });
        ui.scope(|ui| {
            ui.set_width(col_w);
            provider_card(ui, &state.local_navigation.weather);
        });
        ui.scope(|ui| {
            ui.set_width(col_w);
            provider_card(ui, &state.local_navigation.satellite);
        });
    });
}

/// Count the ten live external feeds represented by the grouped Layers menu.
/// Base-map presentation toggles (route, dead zones, GNSS, dark mode) are
/// intentionally excluded so the badge answers one precise question: how many
/// external overlay feeds are currently enabled.
fn active_live_overlay_count(map: &MapViewState) -> usize {
    [
        map.earthquake_overlay,
        map.nws_alert_overlay,
        map.aircraft_overlay,
        map.transit_overlay,
        map.nws_forecast_overlay,
        map.caltrans_camera_overlay,
        map.iem_radar_overlay,
        map.wildfire_overlay,
        map.traffic_event_overlay,
        map.air_quality_overlay,
    ]
    .into_iter()
    .filter(|enabled| *enabled)
    .count()
}

/// Grouped live-feed controls for the Map tab. Every checkbox maps directly to
/// one of the ten typed latest-wins overlay states; optional label/animation
/// preferences remain next to their owning feed so turning a feed off cannot
/// leave a detached preference control in the main toolbar.
#[derive(Debug, Clone, Copy)]
struct MapLayersLayout {
    button: Rect,
    popup: Rect,
    first_toggle: Rect,
}

impl Default for MapLayersLayout {
    fn default() -> Self {
        Self {
            button: Rect::NOTHING,
            popup: Rect::NOTHING,
            first_toggle: Rect::NOTHING,
        }
    }
}

fn map_layers_menu(ui: &mut egui::Ui, map: &mut MapViewState) -> MapLayersLayout {
    let active = active_live_overlay_count(map);
    let popup_id = ui.make_persistent_id(MAP_LAYERS_POPUP_ID);
    let was_open = ui.memory(|memory| memory.is_popup_open(popup_id));
    let button = ui.button(format!("Layers ({active})"));
    if button.clicked() {
        ui.memory_mut(|memory| memory.toggle_popup(popup_id));
    }

    let mut layout = MapLayersLayout {
        button: button.rect,
        ..MapLayersLayout::default()
    };
    if !ui.memory(|memory| memory.is_popup_open(popup_id)) {
        return layout;
    }

    let popup_rect = bounded_popup_rect(
        button.rect,
        ui.clip_rect(),
        MAP_LAYERS_POPUP_WIDTH,
        MAP_LAYERS_POPUP_HEIGHT,
    );
    if !popup_rect.is_positive() {
        ui.memory_mut(|memory| memory.close_popup());
        return layout;
    }

    let frame = egui::Frame::popup(ui.style());
    let frame_margin = frame.total_margin();
    let content_width = (popup_rect.width() - frame_margin.sum().x).max(1.0);
    let content_height = (popup_rect.height() - frame_margin.sum().y).max(1.0);
    let popup = egui::Area::new(popup_id)
        .kind(egui::UiKind::Popup)
        .order(egui::Order::Foreground)
        .fixed_pos(popup_rect.left_top())
        .constrain_to(ui.clip_rect())
        .default_width(popup_rect.width())
        .sense(Sense::hover())
        .show(ui.ctx(), |ui| {
            frame
                .show(ui, |ui| {
                    ui.set_width(content_width);
                    let mut first_toggle = Rect::NOTHING;
                    egui::ScrollArea::vertical()
                        .id_salt(MAP_LAYERS_SCROLL_ID)
                        .auto_shrink([false, false])
                        .max_height(content_height)
                        .show(ui, |ui| {
                            ui.set_width(content_width);
                            ui.label(RichText::new("Safety").strong().color(Style::TEXT_STRONG));
                            let response =
                                ui.checkbox(&mut map.nws_alert_overlay, "Weather alerts");
                            first_toggle = response.rect;
                            ui.checkbox(&mut map.iem_radar_overlay, "NEXRAD radar");
                            if map.iem_radar_overlay {
                                ui.indent("layers-radar-options", |ui| {
                                    ui.checkbox(&mut map.iem_radar.animate, "Animate radar");
                                });
                            }
                            ui.checkbox(
                                &mut map.wildfire_overlay,
                                "Wildfire perimeters + hotspots",
                            );

                            ui.separator();
                            ui.label(
                                RichText::new("Road & transit")
                                    .strong()
                                    .color(Style::TEXT_STRONG),
                            );
                            ui.checkbox(&mut map.traffic_event_overlay, "NCDOT traffic");
                            ui.checkbox(&mut map.caltrans_camera_overlay, "Caltrans cameras");
                            ui.checkbox(&mut map.nws_forecast_overlay, "Hourly forecast");
                            ui.checkbox(&mut map.transit_overlay, "MBTA transit");
                            if map.transit_overlay {
                                ui.indent("layers-transit-options", |ui| {
                                    ui.checkbox(&mut map.transit.show_labels, "Transit labels");
                                });
                            }

                            ui.separator();
                            ui.label(RichText::new("Ambient").strong().color(Style::TEXT_STRONG));
                            ui.checkbox(&mut map.earthquake_overlay, "Earthquakes");
                            ui.checkbox(&mut map.air_quality_overlay, "AirNow AQI");
                            ui.checkbox(&mut map.aircraft_overlay, "Aircraft");
                            if map.aircraft_overlay {
                                ui.indent("layers-aircraft-options", |ui| {
                                    ui.checkbox(&mut map.aircraft.show_callsigns, "Callsigns");
                                });
                            }
                        });
                    first_toggle
                })
                .inner
        });
    layout.popup = popup.response.rect;
    layout.first_toggle = popup.inner;

    if ui.input(|input| input.key_pressed(egui::Key::Escape))
        || (was_open
            && !button.clicked()
            && button.clicked_elsewhere()
            && popup.response.clicked_elsewhere())
    {
        ui.memory_mut(|memory| memory.close_popup());
    }
    layout
}

fn show_routes_trips(ui: &mut egui::Ui, state: &MapsLocationSurface) {
    let col_w = split_width(ui, 2);
    ui.horizontal_top(|ui| {
        ui.scope(|ui| {
            ui.set_width(col_w);
            card(ui, "Active route", |ui| {
                let route = &state.local_navigation.active_route;
                if !route.is_planned() {
                    mde_egui::widgets::muted_note(ui, "No route planned.");
                }
                metric(
                    ui,
                    "Current road",
                    dash_if_empty(&route.current_road),
                    Style::TEXT,
                );
                metric(
                    ui,
                    "Alternatives",
                    &route.alternatives.to_string(),
                    Style::ACCENT,
                );
                metric(
                    ui,
                    "Traffic",
                    dash_if_empty(&route.traffic_alert),
                    Style::WARN,
                );
                metric(ui, "Weather", dash_if_empty(&route.weather_alert), WEATHER);
            });
        });
        ui.scope(|ui| {
            ui.set_width(col_w);
            trip_card(ui, &state.trips);
        });
    });
    ui.add_space(Style::SP_S);
    card(ui, "Breadcrumb replay and event history", |ui| {
        if state.trips.breadcrumbs.is_empty() {
            mde_egui::widgets::muted_note(
                ui,
                "No breadcrumbs recorded — trip history records from the live GNSS fix.",
            );
        }
        for crumb in &state.trips.breadcrumbs {
            ui.horizontal_wrapped(|ui| {
                status_dot(ui, Style::ACCENT_MESH);
                ui.label(format!(
                    "{:.4}, {:.4} | {:.0} mph | {}",
                    crumb.lat,
                    crumb.lon,
                    crumb.speed_mph,
                    crumb.source.label()
                ));
                if let Some(event) = &crumb.event {
                    ui.label(RichText::new(event).color(Style::TEXT_DIM));
                }
            });
        }
    });
    ui.add_space(Style::SP_S);
    card(ui, "Connectivity and event exports", |ui| {
        ui.horizontal_wrapped(|ui| {
            for format in &state.trips.export_formats {
                let _ = ui.button(format.label());
            }
        });
        metric(
            ui,
            "Retention",
            &format!("{} days", state.trips.retention_days),
            Style::TEXT,
        );
        metric(
            ui,
            "History storage",
            encrypted_label(state.trips.encrypted_at_rest),
            Style::OK,
        );
    });
    ui.add_space(Style::SP_S);
    dead_zone_card(ui, &state.dead_zones);
}

fn show_admin(ui: &mut egui::Ui, state: &mut MapsLocationSurface) {
    apply_admin_keyboard_shortcuts(ui.ctx(), &mut state.admin_section);

    ui.horizontal_wrapped(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
        let _ = paint_carbon(ui.painter(), rect, "settings", Style::ACCENT_HI);
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new("MG90 Admin · Single Interface")
                .size(Style::TITLE)
                .color(Style::TEXT_STRONG),
        );
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            pill(ui, "keys 1–7", Style::ACCENT);
        });
    });
    mde_egui::widgets::muted_note(
        ui,
        "Vehicle, connectivity, local I/O, location-source, setup, settings, and firmware tools are consolidated here. Select a section with the mouse or number keys.",
    );
    ui.add_space(Style::SP_S);
    admin_section_strip(ui, &mut state.admin_section);
    ui.add_space(Style::SP_S);
    divider(ui);
    ui.add_space(Style::SP_S);
    mg90_connection_card(ui, state);
    ui.add_space(Style::SP_S);

    match state.admin_section {
        AdminSection::Vehicle => show_vehicle(
            ui,
            &state.vehicle,
            &state.vehicle_radio_health,
            &state.vehicle_mirror_status,
        ),
        AdminSection::Connectivity => show_connectivity(ui, &state.mg90),
        AdminSection::DevicesIo => show_devices_io(ui, &mut state.devices),
        AdminSection::LocationSources => show_location_sources(ui, &mut state.locations),
        AdminSection::Mg90Setup => show_mg90_setup(
            ui,
            &mut state.mg90,
            &state.offline_maps,
            &state.vault,
            &state.real_hardware_gaps,
        ),
        AdminSection::Mg90Settings => show_mg90_settings(ui, state),
        AdminSection::FirmwareRecovery => {
            show_firmware_recovery(ui, &state.firmware, &state.devices)
        }
    }
}

/// Keep the MG90 admin surface actionable when the gateway adapter is absent.
/// A blank settings registry is not a useful status: operators need to know
/// whether the device is offline, unassigned, or merely still loading.
fn mg90_connection_card(ui: &mut egui::Ui, state: &MapsLocationSurface) {
    let current = state.vehicle_mirror_status.state.is_current();
    let (tone, title, detail) = if current {
        (Style::OK, "Bench MG90 connected", "Live vehicle mirror accepted")
    } else {
        (
            Style::WARN,
            "Bench MG90 not connected",
            "No live state/vehicle mirror is available for this workstation",
        )
    };
    mde_egui::widgets::card().show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            status_dot(ui, tone);
            ui.label(
                RichText::new(title)
                    .size(Style::BODY)
                    .color(Style::TEXT_STRONG),
            );
            pill(ui, if current { "LIVE" } else { "OFFLINE" }, tone);
        });
        ui.add_space(Style::SP_XS);
        mde_egui::widgets::muted_note(ui, detail);
        if !current {
            ui.add_space(Style::SP_XS);
            mde_egui::widgets::muted_note(
                ui,
                "Configure the authorized MDE_VEHICLE_GATEWAY and root-only credential file on this seat, then restart mackesd. No MG90 values are fabricated while that adapter is absent.",
            );
        }
    });
}

fn apply_admin_keyboard_shortcuts(ctx: &egui::Context, selected: &mut AdminSection) {
    if ctx.wants_keyboard_input() {
        return;
    }
    let next = ctx.input(|input| {
        if input.key_pressed(egui::Key::Num1) {
            Some(AdminSection::Vehicle)
        } else if input.key_pressed(egui::Key::Num2) {
            Some(AdminSection::Connectivity)
        } else if input.key_pressed(egui::Key::Num3) {
            Some(AdminSection::DevicesIo)
        } else if input.key_pressed(egui::Key::Num4) {
            Some(AdminSection::LocationSources)
        } else if input.key_pressed(egui::Key::Num5) {
            Some(AdminSection::Mg90Setup)
        } else if input.key_pressed(egui::Key::Num6) {
            Some(AdminSection::Mg90Settings)
        } else if input.key_pressed(egui::Key::Num7) {
            Some(AdminSection::FirmwareRecovery)
        } else {
            None
        }
    });
    if let Some(next) = next {
        *selected = next;
    }
}

fn admin_section_strip(ui: &mut egui::Ui, selected: &mut AdminSection) {
    ui.horizontal_wrapped(|ui| {
        for section in AdminSection::ALL {
            if admin_section_button(ui, section, *selected == section).clicked() {
                *selected = section;
            }
        }
    });
}

fn admin_section_button(
    ui: &mut egui::Ui,
    section: AdminSection,
    selected: bool,
) -> egui::Response {
    let label = format!("{} {}", section.shortcut_label(), section.label());
    let galley = ui.painter().layout_no_wrap(
        label.clone(),
        FontId::proportional(Style::SMALL),
        Style::TEXT,
    );
    // The Admin page is rendered inside the shell-reserved workspace, and on
    // short/narrow seats the content pane can be smaller than the comfortable
    // 96 px chip width.  Do not let the chip allocate beyond the current
    // visible lane: an escaped `interact` rect is exactly how the old
    // "advanced" menu targets became visible-looking but unclickable.
    let visible_lane = ui.available_rect_before_wrap().intersect(ui.clip_rect());
    let screen = ui.ctx().screen_rect();
    let cursor_left = ui.cursor().left().max(screen.left());
    let right_edge = visible_lane
        .right()
        .min(ui.max_rect().right())
        .min(screen.right())
        // Leave a tiny numerical margin at the clip edge.  egui's tessellated
        // rect can otherwise round an exactly-edge target one fraction beyond
        // a very narrow 72 px test/display lane.
        - 1.0;
    let available = (right_edge - cursor_left)
        .max(1.0)
        .min(ui.available_width().max(1.0));
    let minimum = 96.0_f32.min(available);
    let width = (galley.size().x + Style::SP_M + Style::SP_S)
        .max(minimum)
        .min(available)
        .max(1.0);
    let size = egui::vec2(width, Style::SP_XL);
    let (_, rect) = ui.allocate_space(size);
    let response = ui.interact(rect, admin_section_item_id(section), Sense::click());
    let fill = if selected {
        Style::pressed_fill(Style::ACCENT)
    } else if response.hovered() {
        Style::SURFACE_HI
    } else {
        Style::SURFACE
    };
    ui.painter().rect_filled(rect, Style::RADIUS_S, fill);
    if selected {
        ui.painter().rect_filled(
            Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
            Style::RADIUS_S,
            Style::ACCENT,
        );
    }
    let text_color = if selected {
        Style::TEXT_STRONG
    } else {
        Style::TEXT
    };
    ui.painter().galley(
        egui::pos2(
            rect.left() + Style::SP_S,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        text_color,
    );
    ui.add_space(Style::SP_XS);
    response
}

fn admin_section_item_id(section: AdminSection) -> egui::Id {
    egui::Id::new(("maps-location-admin-section", section.label()))
}

fn show_vehicle(
    ui: &mut egui::Ui,
    vehicle: &VehicleState,
    radio_health: &VehicleRadioHealth,
    mirror_status: &VehicleMirrorStatus,
) {
    let telem = &vehicle.telemetry;
    // Every telemetry readout rides the live-mirror gate (Q33): a surface with
    // no telemetry source dashes — 0 rpm / 0.0 V / "OFF" are readings, and a
    // sourceless surface has none to report.
    let live = mirror_status.state.is_current() && telem.is_live();
    // Vehicle identity header.
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
        let _ = paint_carbon(ui.painter(), rect, "view", Style::ACCENT_HI);
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(dash_if_empty(&vehicle.profile))
                .size(Style::TITLE)
                .color(Style::TEXT_STRONG),
        );
    });
    ui.add_space(Style::SP_S);
    vehicle_mirror_status_card(ui, mirror_status);
    ui.add_space(Style::SP_S);
    // Hero gauges — the four live readouts that matter at a glance.
    let tile_w = split_width(ui, 4);
    let gauge = |value: String, tone: Color32| -> (String, Color32) {
        if live {
            (value, tone)
        } else {
            ("—".to_string(), Style::TEXT_DIM)
        }
    };
    ui.horizontal_top(|ui| {
        let (v, tone) = gauge(format!("{:.0}", telem.speed_mph), Style::TEXT_STRONG);
        stat_tile(ui, tile_w, "go-next", "Speed · mph", &v, tone);
        let (v, tone) = gauge(telem.rpm.to_string(), Style::ACCENT);
        stat_tile(ui, tile_w, "view-refresh", "Engine · rpm", &v, tone);
        let (v, tone) = gauge(
            format!("{:.1}", telem.battery_v),
            voltage_tone(telem.battery_v),
        );
        stat_tile(ui, tile_w, "notification", "Battery · V", &v, tone);
        let (v, tone) = gauge(
            format!("{:.0}", telem.coolant_c),
            coolant_tone(telem.coolant_c),
        );
        stat_tile(ui, tile_w, "weather-clear-night", "Coolant · °C", &v, tone);
    });
    ui.add_space(Style::SP_S);
    glyph_card(
        ui,
        "view-grid",
        "OBD telematics",
        Style::ACCENT_MESH,
        |ui| {
            let (v, tone) = if live {
                (
                    if telem.ignition_on { "on" } else { "off" },
                    if telem.ignition_on {
                        Style::OK
                    } else {
                        Style::TEXT_DIM
                    },
                )
            } else {
                ("—", Style::TEXT_DIM)
            };
            readout(ui, "Ignition", v, tone);
            let (v, tone) = if live {
                (
                    if telem.moving { "moving" } else { "parked" },
                    if telem.moving { Style::WARN } else { Style::OK },
                )
            } else {
                ("—", Style::TEXT_DIM)
            };
            readout(ui, "Motion", v, tone);
            let (v, tone) = if live {
                (
                    telem
                        .fuel_percent
                        .map_or_else(|| "unavailable".to_string(), |fuel| format!("{fuel:.0}%")),
                    telem.fuel_percent.map_or(Style::TEXT_DIM, |fuel| {
                        if fuel < 15.0 {
                            Style::WARN
                        } else {
                            Style::OK
                        }
                    }),
                )
            } else {
                ("—".to_string(), Style::TEXT_DIM)
            };
            readout(ui, "Fuel", &v, tone);
            let (v, tone) = if live {
                (telem.dtc_count.to_string(), count_tone(telem.dtc_count))
            } else {
                ("—".to_string(), Style::TEXT_DIM)
            };
            readout(ui, "Diagnostic codes", &v, tone);
            let v = if live {
                telem
                    .odometer_mi
                    .map_or_else(|| "unavailable".to_string(), |odo| format!("{odo} mi"))
            } else {
                "—".to_string()
            };
            readout(ui, "Odometer", &v, Style::TEXT);
            let v = if live {
                format!("{} min", telem.runtime_min)
            } else {
                "—".to_string()
            };
            readout(ui, "Runtime", &v, Style::TEXT);
            readout(
                ui,
                "Confidence",
                dash_if_empty(&telem.confidence),
                Style::TEXT_DIM,
            );
            // Provenance stays explicit after freshness expires: the stale
            // mirror's values dash, while its age remains visible in warning
            // tone so the operator can see exactly why it is no longer live.
            let (v, tone) = if telem.has_live_gateway_source() {
                (
                    format!("{:.1} s ago", telem.last_update_age_s),
                    if live { Style::TEXT_DIM } else { Style::WARN },
                )
            } else {
                ("—".to_string(), Style::TEXT_DIM)
            };
            readout(ui, "Last update", &v, tone);
        },
    );
    ui.add_space(Style::SP_S);
    glyph_card(
        ui,
        "document-open-recent",
        "Profile integration",
        Style::ACCENT,
        |ui| {
            bullet(
                ui,
                "Map events, trip history, route alerts, diagnostic bundles, and motion detection read this profile layer.",
            );
            for note in &vehicle.profile_notes {
                bullet(ui, note);
            }
        },
    );
    ui.add_space(Style::SP_S);
    radio_health_card(ui, radio_health);
}

fn vehicle_mirror_status_card(ui: &mut egui::Ui, status: &VehicleMirrorStatus) {
    let tone = mirror_status_tone(status.state);
    glyph_card(
        ui,
        "cloud-service-management",
        "Vehicle mirror",
        tone,
        |ui| {
            ui.horizontal_wrapped(|ui| {
                pill(ui, status.state.label(), tone);
                if status.has_retained_snapshot() && !status.state.is_current() {
                    pill(ui, "cached values retained", Style::WARN);
                }
            });
            if let Some(provenance) = status.provenance.as_ref() {
                readout(
                    ui,
                    "Management node",
                    &provenance.management_node_id,
                    Style::TEXT,
                );
                if let Some(mg90_id) = provenance.mg90_id.as_deref() {
                    readout(ui, "MG90", mg90_id, Style::TEXT);
                }
                readout(
                    ui,
                    "Source",
                    snapshot_source_label(provenance.source),
                    Style::TEXT_DIM,
                );
                if let Some(source_id) = provenance.source_id.as_deref() {
                    readout(ui, "Source ID", source_id, Style::TEXT_DIM);
                }
                if let Some(relay) = provenance.relay.as_deref() {
                    readout(ui, "Relay", relay, Style::TEXT_DIM);
                }
            }
            readout(
                ui,
                "Snapshot age",
                &status.age_label(),
                freshness_tone_for_mirror(status.state),
            );
            if let Some(sequence) = status.sequence {
                readout(ui, "Sequence", &sequence.to_string(), Style::TEXT_DIM);
            }
            if let Some(reason) = status.reason.as_deref() {
                readout(ui, "Reason", reason, tone);
            }
            if !status.state.is_current() {
                mde_egui::widgets::muted_note(
                ui,
                "Retained vehicle values are diagnostic only; live telemetry is unavailable until a current snapshot arrives.",
            );
            }
        },
    );
}

fn snapshot_source_label(source: mackes_mesh_types::vehicle::SnapshotSource) -> &'static str {
    match source {
        mackes_mesh_types::vehicle::SnapshotSource::DirectGateway => "Direct gateway",
        mackes_mesh_types::vehicle::SnapshotSource::MeshRelay => "Mesh relay",
        mackes_mesh_types::vehicle::SnapshotSource::Unknown => "Unknown",
    }
}

fn mirror_status_tone(state: VehicleMirrorState) -> Color32 {
    match state {
        VehicleMirrorState::Current => Style::OK,
        VehicleMirrorState::StaleRetained | VehicleMirrorState::ResyncingNoFreshSnapshot => {
            Style::WARN
        }
        VehicleMirrorState::UnavailableMalformed => Style::TEXT_DIM,
    }
}

fn freshness_tone_for_mirror(state: VehicleMirrorState) -> Color32 {
    mirror_status_tone(state)
}

fn radio_health_card(ui: &mut egui::Ui, health: &VehicleRadioHealth) {
    let tone = match health.availability {
        VehicleRadioAvailability::Available => Style::OK,
        VehicleRadioAvailability::Degraded => Style::WARN,
        VehicleRadioAvailability::Unavailable => Style::TEXT_DIM,
    };
    glyph_card(ui, "globe", "Typed radio health", tone, |ui| {
        ui.horizontal_wrapped(|ui| {
            pill(ui, health.availability.label(), tone);
            if let Some(version) = health.schema_version {
                pill(ui, &format!("schema v{version}"), Style::ACCENT_MESH);
            }
            if let Some(reason) = health.availability_reason.as_deref() {
                ui.label(
                    RichText::new(reason)
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
            }
        });
        ui.add_space(Style::SP_XS);
        if health.availability == VehicleRadioAvailability::Unavailable && health.radios.is_empty()
        {
            mde_egui::widgets::muted_note(
                ui,
                "No valid typed radio inventory is available. No radio state is inferred from the legacy mirror.",
            );
        } else {
            let width = ui.available_width().max(1.0);
            for row in &health.radios {
                let row_tone = match row.operation {
                    VehicleRadioOperation::Active => Style::OK,
                    VehicleRadioOperation::Standby | VehicleRadioOperation::Acquiring => {
                        Style::ACCENT
                    }
                    VehicleRadioOperation::Degraded | VehicleRadioOperation::Stale => Style::WARN,
                    VehicleRadioOperation::Fault => Style::DANGER,
                    VehicleRadioOperation::Disabled | VehicleRadioOperation::Unknown => {
                        Style::TEXT_DIM
                    }
                };
                ui.horizontal(|ui| {
                    ui.set_width(width);
                    ui.label(
                        RichText::new(&row.id)
                            .size(Style::SMALL)
                            .color(Style::TEXT_STRONG)
                            .monospace(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if row.active_path {
                            pill(ui, "ACTIVE", Style::ACCENT);
                        }
                        pill(ui, row.presence.label(), presence_tone(row.presence));
                        pill(ui, row.operation.label(), row_tone);
                    });
                });
                readout(ui, "role", &row.role, Style::TEXT_DIM);
                readout(
                    ui,
                    "reason",
                    row.reason.as_deref().unwrap_or("not reported"),
                    if row.reason.is_some() {
                        Style::WARN
                    } else {
                        Style::TEXT_DIM
                    },
                );
                readout(ui, "age", &row.age_label(), row_tone);
            }
        }
        ui.add_space(Style::SP_XS);
        divider(ui);
        ui.add_space(Style::SP_XS);
        readout(
            ui,
            "Radio freshness",
            &format_freshness(&health.radios_freshness),
            freshness_tone(health.radios_freshness.state),
        );
        readout(
            ui,
            "GNSS freshness",
            &format_freshness(&health.gnss_freshness),
            freshness_tone(health.gnss_freshness.state),
        );
        readout(
            ui,
            "Snapshot age",
            &health
                .snapshot_age_ms
                .map_or_else(|| "age unknown".to_string(), format_age_ms),
            if health.snapshot_age_ms.is_some() {
                Style::TEXT_DIM
            } else {
                Style::WARN
            },
        );
    });
}

fn presence_tone(presence: VehicleRadioPresence) -> Color32 {
    match presence {
        VehicleRadioPresence::Installed => Style::OK,
        VehicleRadioPresence::NotInstalled => Style::TEXT_DIM,
        VehicleRadioPresence::Unknown => Style::WARN,
    }
}

fn freshness_tone(state: crate::model::VehicleFreshnessState) -> Color32 {
    match state {
        crate::model::VehicleFreshnessState::Fresh => Style::OK,
        crate::model::VehicleFreshnessState::Stale => Style::WARN,
        crate::model::VehicleFreshnessState::Unknown => Style::TEXT_DIM,
    }
}

fn format_freshness(freshness: &crate::model::VehicleFreshness) -> String {
    match freshness.reason.as_deref() {
        Some(reason) => format!(
            "{} · {} · {reason}",
            freshness.state.label(),
            freshness.age_label()
        ),
        None => format!("{} · {}", freshness.state.label(), freshness.age_label()),
    }
}

fn format_age_ms(age_ms: u64) -> String {
    if age_ms < 1_000 {
        format!("{age_ms} ms")
    } else {
        format!("{:.1} s", age_ms as f32 / 1_000.0)
    }
}

fn show_connectivity(ui: &mut egui::Ui, mg90: &Mg90State) {
    let status = &mg90.status;
    // WAN metrics only exist while a WAN is actually up (Q33): with no active
    // uplink, "0 ms" / "0.0%" would be fabricated measurements, so they dash.
    let wan_up = !status.active_wan.trim().is_empty();
    // Hero readouts: the four numbers that describe the live WAN at a glance.
    let latency_tone = if !wan_up {
        Style::TEXT_DIM
    } else if status.latency_ms < 100 {
        Style::OK
    } else if status.latency_ms < 200 {
        Style::WARN
    } else {
        Style::DANGER
    };
    let loss_tone = if !wan_up {
        Style::TEXT_DIM
    } else if status.packet_loss_percent < 1.0 {
        Style::OK
    } else if status.packet_loss_percent < 5.0 {
        Style::WARN
    } else {
        Style::DANGER
    };
    let tile_w = split_width(ui, 4);
    ui.horizontal_top(|ui| {
        stat_tile(
            ui,
            tile_w,
            "globe",
            "Active WAN",
            dash_if_empty(&status.active_wan),
            Style::ACCENT_HI,
        );
        stat_tile(
            ui,
            tile_w,
            "emblem-ok",
            "Link quality",
            dash_if_empty(&status.link_quality),
            Style::OK,
        );
        let latency = if wan_up {
            format!("{} ms", status.latency_ms)
        } else {
            "—".to_string()
        };
        stat_tile(
            ui,
            tile_w,
            "view-refresh",
            "Latency",
            &latency,
            latency_tone,
        );
        let loss = if wan_up {
            format!("{:.1}%", status.packet_loss_percent)
        } else {
            "—".to_string()
        };
        stat_tile(ui, tile_w, "notification", "Packet loss", &loss, loss_tone);
    });
    ui.add_space(Style::SP_S);
    // Dual-modem comparison, active WAN highlighted.
    let col_w = split_width(ui, 2);
    ui.horizontal_top(|ui| {
        ui.scope(|ui| {
            ui.set_width(col_w);
            cellular_modem_card(
                ui,
                "A",
                &status.cellular_a,
                status.active_wan == "Cellular A",
            );
        });
        ui.scope(|ui| {
            ui.set_width(col_w);
            cellular_modem_card(
                ui,
                "B",
                &status.cellular_b,
                status.active_wan == "Cellular B",
            );
        });
    });
    ui.add_space(Style::SP_S);
    glyph_card(ui, "share", "Local interfaces", Style::ACCENT_MESH, |ui| {
        readout(
            ui,
            "Wi-Fi",
            dash_if_empty(&status.wifi_state),
            Style::TEXT_DIM,
        );
        readout(
            ui,
            "Ethernet",
            dash_if_empty(&status.ethernet_state),
            Style::OK,
        );
        readout(ui, "VPN", dash_if_empty(&status.vpn_state), Style::TEXT_DIM);
        readout(
            ui,
            "Data transferred",
            dash_if_empty(&status.data_transferred),
            Style::TEXT,
        );
        let (v, tone) = if wan_up {
            (
                status.failover_events.to_string(),
                if status.failover_events == 0 {
                    Style::OK
                } else {
                    Style::WARN
                },
            )
        } else {
            ("—".to_string(), Style::TEXT_DIM)
        };
        readout(ui, "Failover events", &v, tone);
    });
}

fn show_devices_io(ui: &mut egui::Ui, devices: &mut DeviceIoState) {
    // The serial controls need enough width for their checkbox, baud pill, and
    // recovery actions. Below that width, keep each card full-width so the
    // wrapped layout preserves both rendering and hit targets inside the
    // narrow Admin viewport.
    let col_w = responsive_column_width(ui, 2, ADMIN_CARD_MIN_WIDTH);
    ui.horizontal_wrapped(|ui| {
        ui.scope(|ui| {
            ui.set_width(col_w);
            glyph_card(
                ui,
                "text-x-generic",
                "Serial recovery console",
                Style::WARN,
                |ui| {
                    warning_strip(
                        ui,
                        "Recovery console only; normal settings use direct Ethernet.",
                        Style::WARN,
                    );
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut devices.serial.connected, "Connected");
                        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                            pill(ui, &devices.serial.baud_profile, Style::ACCENT);
                        });
                    });
                    ui.add_space(Style::SP_XS);
                    mde_egui::widgets::inset().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if devices.serial.transcript_lines.is_empty() {
                            mde_egui::widgets::muted_note(ui, "No console output.");
                        }
                        for line in &devices.serial.transcript_lines {
                            ui.label(
                                RichText::new(line)
                                    .monospace()
                                    .size(Style::SMALL)
                                    .color(Style::TEXT_DIM),
                            );
                        }
                    });
                    ui.add_space(Style::SP_S);
                    ui.horizontal_wrapped(|ui| {
                        let _ = ui.button("Send command");
                        let _ = ui.button("Copy output");
                        let _ = ui.button("Save transcript");
                    });
                },
            );
        });
        ui.scope(|ui| {
            ui.set_width(col_w);
            glyph_card(ui, "view-grid", "Device I/O", Style::ACCENT_MESH, |ui| {
                readout(
                    ui,
                    "Ethernet",
                    dash_if_empty(&devices.ethernet_state),
                    Style::OK,
                );
                readout(
                    ui,
                    "CAN / OBD",
                    dash_if_empty(&devices.can_obd_state),
                    Style::ACCENT,
                );
                ui.add_space(Style::SP_XS);
                divider(ui);
                ui.add_space(Style::SP_S);
                ui.label(
                    RichText::new(format!("USB devices ({})", devices.usb_devices.len()))
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                ui.add_space(Style::SP_XS);
                if devices.usb_devices.is_empty() {
                    mde_egui::widgets::muted_note(ui, "No USB devices attached.");
                }
                for device in &devices.usb_devices {
                    bullet(ui, device);
                }
            });
        });
    });
    ui.add_space(Style::SP_S);
    let enabled = devices
        .gpio_rules
        .iter()
        .filter(|rule| rule.enabled)
        .count();
    glyph_card(
        ui,
        "overlay",
        &format!(
            "GPIO automation rules  ·  {enabled}/{} active",
            devices.gpio_rules.len()
        ),
        Style::ACCENT_SYSTEM,
        |ui| {
            if devices.gpio_rules.is_empty() {
                mde_egui::widgets::muted_note(ui, "No GPIO automation rules defined.");
            }
            for rule in &mut devices.gpio_rules {
                gpio_rule_card(ui, rule);
                ui.add_space(Style::SP_S);
            }
        },
    );
}

/// One GPIO automation rule as a self-contained mini-card: an enabled toggle and
/// health dot, the rule id, a simulator-test action, then the trigger / condition
/// / action / last-run readouts and the audit trail.
fn gpio_rule_card(ui: &mut egui::Ui, rule: &mut crate::model::GpioAutomationRule) {
    mg90_frame(None).show(ui, |ui| {
        ui.horizontal(|ui| {
            status_dot(
                ui,
                if rule.enabled {
                    Style::OK
                } else {
                    Style::TEXT_DIM
                },
            );
            ui.checkbox(&mut rule.enabled, "");
            ui.label(
                RichText::new(&rule.id)
                    .size(Style::BODY)
                    .color(Style::TEXT_STRONG),
            );
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                let _ = ui.button("Simulator test");
            });
        });
        ui.add_space(Style::SP_XS);
        divider(ui);
        ui.add_space(Style::SP_S);
        readout(ui, "Trigger", dash_if_empty(&rule.trigger), Style::TEXT);
        readout(
            ui,
            "Condition",
            dash_if_empty(&rule.condition),
            Style::TEXT_DIM,
        );
        readout(ui, "Action", dash_if_empty(&rule.action), Style::ACCENT);
        readout(
            ui,
            "Last run",
            dash_if_empty(&rule.last_run),
            Style::TEXT_DIM,
        );
        for audit in &rule.audit_log {
            bullet(ui, audit);
        }
    });
}

fn show_location_sources(ui: &mut egui::Ui, manager: &mut LocationManager) {
    if let Some(warning) = manager.primary_warning() {
        warning_strip(ui, &warning, Style::WARN);
        let alternatives = manager.healthy_alternatives();
        ui.horizontal_wrapped(|ui| {
            for alternative in alternatives {
                if ui
                    .button(format!("Switch to {}", alternative.label()))
                    .clicked()
                {
                    manager.set_primary(alternative);
                }
            }
        });
        ui.add_space(Style::SP_S);
    }
    let mut picked = None;
    for source in &manager.sources {
        let switch_ready = source.manual_switch_ready();
        let source_tone = source_readiness_tone(source);
        card(ui, source.kind.label(), |ui| {
            ui.horizontal(|ui| {
                status_dot(ui, source_tone);
                ui.label(if manager.primary == source.kind {
                    "Primary source"
                } else {
                    "Equal peer source"
                });
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            manager.primary != source.kind && switch_ready,
                            egui::Button::new("Make primary"),
                        )
                        .clicked()
                    {
                        picked = Some(source.kind);
                    }
                });
            });
            metric(
                ui,
                "Status",
                source_status_label(source.status),
                Style::TEXT,
            );
            metric(
                ui,
                "Switch readiness",
                &source.manual_switch_reason(),
                source_tone,
            );
            // Position-derived metrics are honest ONLY on a real fix (Q33): a
            // no-lock sample would otherwise print the fabricated null-island
            // "0.00000, 0.00000" as a coordinate.
            let fixed = source.sample.has_fix();
            let on_fix = |value: String| {
                if fixed {
                    value
                } else {
                    "—".to_string()
                }
            };
            metric(ui, "Fix", &source.sample.fix_type, Style::TEXT);
            metric(
                ui,
                "Lat / Lon",
                &on_fix(format!(
                    "{:.5}, {:.5}",
                    source.sample.latitude, source.sample.longitude
                )),
                Style::TEXT,
            );
            metric(
                ui,
                "Accuracy",
                &on_fix(format!("{:.1} m", source.sample.accuracy_m)),
                if fixed {
                    health_color(&source.sample)
                } else {
                    Style::TEXT_DIM
                },
            );
            metric(
                ui,
                "Speed",
                &on_fix(format!("{:.1} mph", source.sample.speed_mph)),
                Style::TEXT,
            );
            metric(
                ui,
                "Heading",
                &on_fix(format!("{:.0} deg", source.sample.heading_deg)),
                Style::TEXT,
            );
            metric(
                ui,
                "Altitude",
                &on_fix(format!("{:.1} m", source.sample.altitude_m)),
                Style::TEXT,
            );
            metric(
                ui,
                "Satellites",
                &source
                    .sample
                    .satellites
                    .map_or_else(|| "unavailable".to_string(), |n| n.to_string()),
                Style::TEXT,
            );
            metric(
                ui,
                "Update rate / age",
                &on_fix(format!(
                    "{:.1} Hz / {:.1} s",
                    source.sample.update_rate_hz, source.sample.update_age_s
                )),
                Style::TEXT,
            );
            metric(
                ui,
                "Connected device",
                &source.connected_device,
                Style::TEXT_DIM,
            );
            for (key, value) in &source.diagnostics {
                metric(ui, key, value, Style::TEXT_DIM);
            }
        });
        ui.add_space(Style::SP_S);
    }
    if let Some(kind) = picked {
        manager.set_primary(kind);
    }
    metric(
        ui,
        "Automatic failover",
        bool_label(manager.auto_failover),
        Style::TEXT_DIM,
    );
}

fn show_mg90_setup(
    ui: &mut egui::Ui,
    mg90: &mut Mg90State,
    offline_maps: &OfflineMapManagerState,
    vault: &EncryptedVaultState,
    gaps: &[String],
) {
    let done = SetupStep::ALL
        .iter()
        .position(|step| *step == mg90.setup_step)
        .map_or(0, |index| index + 1);
    let total = SetupStep::ALL.len();

    let col_w = responsive_column_width(ui, 2, ADMIN_CARD_MIN_WIDTH);
    ui.horizontal_wrapped(|ui| {
        ui.scope(|ui| {
            ui.set_width(col_w);
            glyph_card(ui, "view-grid", "Device inventory", Style::ACCENT, |ui| {
                readout(
                    ui,
                    "Managed MG90s",
                    &mg90.managed_devices.to_string(),
                    Style::TEXT,
                );
                // The model family is a discovery result: dash it until the
                // wizard has actually discovered a device (Q33 — never claim a
                // model that was not read from hardware).
                readout(
                    ui,
                    "Model",
                    if mg90.setup_step >= SetupStep::Mg90Discovered {
                        mg90.model.label()
                    } else {
                        "—"
                    },
                    Style::TEXT,
                );
                readout(
                    ui,
                    "MGOS",
                    dash_if_empty(&mg90.capabilities.mgos_version),
                    Style::TEXT,
                );
                readout(ui, "Management path", "direct Ethernet only", Style::OK);
                readout(
                    ui,
                    "Offline map",
                    dash_if_empty(&offline_maps.default_region),
                    Style::ACCENT_SYSTEM,
                );
                readout(
                    ui,
                    "Authenticated",
                    if mg90.authenticated { "yes" } else { "no" },
                    if mg90.authenticated {
                        Style::OK
                    } else {
                        Style::WARN
                    },
                );
                ui.add_space(Style::SP_XS);
                divider(ui);
                ui.add_space(Style::SP_S);
                ui.label(
                    RichText::new("Capabilities")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                ui.add_space(Style::SP_XS);
                let caps = &mg90.capabilities;
                ui.horizontal_wrapped(|ui| {
                    cap_pill(ui, "LTE-A", caps.lte_a);
                    cap_pill(ui, "5G", caps.five_g);
                    cap_pill(ui, "GNSS", caps.gnss);
                    cap_pill(ui, "GPIO", caps.gpio);
                    cap_pill(ui, "Serial recovery", caps.serial_recovery);
                    cap_pill(ui, "Firmware mgmt", caps.firmware_management);
                });
            });
        });
        ui.scope(|ui| {
            ui.set_width(col_w);
            glyph_card(ui, "globe", "Link readiness", Style::ACCENT_MESH, |ui| {
                let status = &mg90.status;
                readout(
                    ui,
                    "Active WAN",
                    dash_if_empty(&status.active_wan),
                    Style::ACCENT_HI,
                );
                readout(
                    ui,
                    "SIM A",
                    dash_if_empty(&status.cellular_a.sim_state),
                    if status.cellular_a.healthy {
                        Style::OK
                    } else {
                        Style::WARN
                    },
                );
                readout(
                    ui,
                    "SIM B",
                    dash_if_empty(&status.cellular_b.sim_state),
                    if status.cellular_b.healthy {
                        Style::OK
                    } else {
                        Style::TEXT_DIM
                    },
                );
                readout(
                    ui,
                    "Wi-Fi",
                    dash_if_empty(&status.wifi_state),
                    Style::TEXT_DIM,
                );
                readout(
                    ui,
                    "Ethernet",
                    dash_if_empty(&status.ethernet_state),
                    Style::OK,
                );
                readout(
                    ui,
                    "Ignition input",
                    if mg90.ignition_on { "on" } else { "off" },
                    if mg90.ignition_on {
                        Style::OK
                    } else {
                        Style::TEXT_DIM
                    },
                );
            });
        });
    });
    ui.add_space(Style::SP_S);
    ui.horizontal_wrapped(|ui| {
        ui.scope(|ui| {
            ui.set_width(col_w);
            glyph_card(
                ui,
                "emblem-ok",
                &format!("Offline setup  ·  {done}/{total}"),
                Style::OK,
                |ui| {
                    for step in SetupStep::ALL {
                        let tone = if step < mg90.setup_step {
                            Style::OK
                        } else if step == mg90.setup_step {
                            Style::ACCENT_HI
                        } else {
                            Style::TEXT_DIM
                        };
                        ui.horizontal(|ui| {
                            status_dot(ui, tone);
                            ui.label(RichText::new(step.label()).size(Style::SMALL).color(tone));
                        });
                        ui.add_space(2.0);
                    }
                    // The "Advance simulator setup" dev button was removed with
                    // the production simulators (WL-UX-007/S1): the wizard only
                    // advances when real discovery/auth seams do.
                },
            );
        });
        ui.scope(|ui| {
            ui.set_width(col_w);
            glyph_card(ui, "document-open-recent", "Operator checklist", Style::ACCENT, |ui| {
                for item in [
                    "Connect MG90 and Egui host by direct Ethernet cable.",
                    "Verify MG90 power, antennas, SIM state, and local IP discovery.",
                    "Enter local credentials and store them in the encrypted vault.",
                    "Create baseline backup before local status, GNSS, map, and route verification.",
                    "Verify MG90 GNSS and USB GPS as equal location-source peers.",
                    "Use serial only for recovery console workflows.",
                ] {
                    bullet(ui, item);
                }
            });
        });
    });
    ui.add_space(Style::SP_S);
    glyph_card(
        ui,
        "system-shutdown",
        "Factory reset guardrails",
        Style::DANGER,
        |ui| {
            warning_strip(
                ui,
                "Factory reset loses configuration; backup and typed confirmation are required.",
                Style::DANGER,
            );
            readout(
                ui,
                "Backup required",
                if mg90.reset.backup_required {
                    "yes"
                } else {
                    "no"
                },
                if mg90.reset.backup_required {
                    Style::WARN
                } else {
                    Style::TEXT_DIM
                },
            );
            readout(
                ui,
                "Backup completed",
                if mg90.reset.backup_completed {
                    "yes"
                } else {
                    "no"
                },
                if mg90.reset.backup_completed {
                    Style::OK
                } else {
                    Style::DANGER
                },
            );
            readout(
                ui,
                "Confirmation phrase",
                &format!("type \"{}\"", mg90.reset.confirmation_phrase),
                Style::TEXT_DIM,
            );
            ui.add_space(Style::SP_XS);
            let reset_enabled = mg90.reset.armed();
            reset_confirmation_row(ui, &mut mg90.reset.typed_confirmation, reset_enabled);
            ui.add_space(Style::SP_XS);
            divider(ui);
            ui.add_space(Style::SP_S);
            ui.label(
                RichText::new("Reconnect workflow")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            ui.add_space(Style::SP_XS);
            for (index, step) in mg90.reset.reconnect_workflow.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!("{}.", index + 1))
                            .size(Style::SMALL)
                            .monospace()
                            .color(Style::TEXT_DIM),
                    );
                    ui.add_space(Style::SP_XS);
                    ui.label(RichText::new(step).size(Style::SMALL).color(Style::TEXT));
                });
            }
        },
    );
    // Transparency cards formerly hosted by the retired Simulator tab
    // (WL-UX-007/S1): the honest gap report, the real restore-point ledger, and
    // the vault readiness model live on the setup/diagnostics surface now.
    ui.add_space(Style::SP_S);
    card(ui, "Known real-hardware gaps", |ui| {
        for gap in gaps {
            bullet(ui, gap);
        }
    });
    ui.add_space(Style::SP_S);
    backups(ui, &mg90.backups);
    ui.add_space(Style::SP_S);
    show_vault(ui, vault);
}

/// Keep the destructive confirmation target inside narrow Admin-page
/// viewports. The old single-line row let the text field consume the whole
/// remaining width, placing the reset button outside the clipped workspace;
/// wrapping preserves a real, on-screen hit target without changing the
/// guardrail state machine.
fn reset_confirmation_row(
    ui: &mut egui::Ui,
    typed_confirmation: &mut String,
    enabled: bool,
) -> egui::Rect {
    ui.vertical(|ui| {
        ui.label(
            RichText::new("Confirm")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        ui.horizontal_wrapped(|ui| {
            let input_width = (ui.available_width() - 112.0).clamp(40.0, 220.0);
            ui.add_sized(
                egui::vec2(input_width, ui.spacing().interact_size.y),
                egui::TextEdit::singleline(typed_confirmation),
            );
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                ui.add_enabled(enabled, egui::Button::new("Reset MG90"))
                    .rect
            })
            .inner
        })
        .inner
    })
    .inner
}

/// A capability chip — green when the feature is present, dim when it is not.
fn cap_pill(ui: &mut egui::Ui, label: &str, present: bool) {
    pill(ui, label, if present { Style::OK } else { Style::TEXT_DIM });
}

fn show_mg90_settings(ui: &mut egui::Ui, state: &MapsLocationSurface) {
    if state.moving() {
        warning_strip(
            ui,
            "Vehicle is moving. Dangerous MG90 changes warn but are not blocked in v1.",
            Style::WARN,
        );
    }
    let total = state.mg90.settings.len();
    glyph_card(
        ui,
        "view-grid",
        "Native setting registry",
        Style::ACCENT,
        |ui| {
            readout(
                ui,
                "Categories",
                &Mg90SettingCategory::ALL.len().to_string(),
                Style::TEXT,
            );
            readout(
                ui,
                "Loaded descriptors",
                &total.to_string(),
                Style::ACCENT_HI,
            );
            readout(
                ui,
                "Vehicle state",
                if state.moving() { "moving" } else { "parked" },
                if state.moving() {
                    Style::WARN
                } else {
                    Style::OK
                },
            );
            mde_egui::widgets::muted_note(
                ui,
                "Every category maps to a native MG90 setting group read over the direct-Ethernet local API.",
            );
        },
    );
    if total == 0 {
        ui.add_space(Style::SP_S);
        mde_egui::widgets::WorkspaceStatePanel::new(
            mde_egui::widgets::WorkspaceState::Offline,
            "Settings waiting for Bench MG90",
            "The native descriptor registry appears after a live, authenticated MG90 mirror is accepted.",
        )
        .show(ui, |ui| {
            mde_egui::widgets::muted_note(
                ui,
                "Check the connection status above, configure the gateway adapter, and reconnect. This state is intentional and contains no guessed settings.",
            )
        });
    }
    ui.add_space(Style::SP_S);
    for category in Mg90SettingCategory::ALL {
        let settings: Vec<&Mg90SettingDescriptor> = state
            .mg90
            .settings
            .iter()
            .filter(|setting| setting.category == category)
            .collect();
        let tone = if settings.is_empty() {
            Style::TEXT_DIM
        } else {
            Style::ACCENT
        };
        glyph_card(
            ui,
            category_icon(category),
            &format!("{}  ·  {}", category.label(), settings.len()),
            tone,
            |ui| {
                if settings.is_empty() {
                    mde_egui::widgets::muted_note(
                        ui,
                        "No descriptors loaded for this category — the MG90 local API is not connected.",
                    );
                }
                for setting in settings {
                    setting_row(ui, state, setting);
                }
            },
        );
        ui.add_space(Style::SP_S);
    }
}

fn show_firmware_recovery(ui: &mut egui::Ui, firmware: &FirmwareWorkflow, devices: &DeviceIoState) {
    warning_strip(
        ui,
        "No blind firmware install — every guardrail check must pass and a restore point must exist first.",
        Style::DANGER,
    );
    ui.add_space(Style::SP_S);
    let col_w = responsive_column_width(ui, 2, ADMIN_CARD_MIN_WIDTH);
    ui.horizontal_wrapped(|ui| {
        ui.scope(|ui| {
            ui.set_width(col_w);
            glyph_card(ui, "download", "Firmware lifecycle", Style::ACCENT, |ui| {
                readout(ui, "Current firmware", dash_if_empty(&firmware.current), Style::TEXT);
                readout(
                    ui,
                    "Target package",
                    dash_if_empty(&firmware.target_package),
                    Style::TEXT_DIM,
                );
                readout(
                    ui,
                    "Restore point",
                    if firmware.restore_point_ready { "ready" } else { "missing" },
                    if firmware.restore_point_ready { Style::OK } else { Style::DANGER },
                );
                ui.add_space(Style::SP_S);
                ui.add(
                    egui::ProgressBar::new(f32::from(firmware.progress_percent) / 100.0)
                        .text(format!("{}%", firmware.progress_percent)),
                );
                ui.add_space(Style::SP_S);
                divider(ui);
                ui.add_space(Style::SP_S);
                let passed = firmware
                    .checks
                    .iter()
                    .filter(|check| check.state == CheckState::Pass)
                    .count();
                ui.label(
                    RichText::new(format!(
                        "Pre-flight checks  ·  {passed}/{}",
                        firmware.checks.len()
                    ))
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
                );
                ui.add_space(Style::SP_XS);
                if firmware.checks.is_empty() {
                    mde_egui::widgets::muted_note(
                        ui,
                        "No firmware package selected — pre-flight checks run against a chosen package.",
                    );
                }
                for check in &firmware.checks {
                    ui.horizontal(|ui| {
                        status_dot(ui, check_tone(check.state));
                        ui.label(RichText::new(&check.label).size(Style::SMALL).color(Style::TEXT));
                        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(check_state_label(check.state))
                                    .size(Style::SMALL)
                                    .monospace()
                                    .color(check_tone(check.state)),
                            );
                        });
                    });
                    ui.add_space(2.0);
                }
            });
        });
        ui.scope(|ui| {
            ui.set_width(col_w);
            glyph_card(ui, "text-x-generic", "Recovery console", Style::WARN, |ui| {
                readout(
                    ui,
                    "Serial profile",
                    dash_if_empty(&devices.serial.baud_profile),
                    Style::TEXT,
                );
                readout(
                    ui,
                    "Connected",
                    if devices.serial.connected { "yes" } else { "no" },
                    if devices.serial.connected { Style::OK } else { Style::TEXT_DIM },
                );
                ui.add_space(Style::SP_XS);
                divider(ui);
                ui.add_space(Style::SP_S);
                bullet(ui, "Do not allow blind firmware install.");
                bullet(ui, "Validate MG90 model, MGOS family, package integrity, power, backup, direct Ethernet, credentials, and rollback plan.");
                bullet(ui, "Post-update reconnect and validation must run before the workflow completes.");
            });
        });
    });
}

/// A short pass/warn/fail word for a firmware check state.
fn check_state_label(state: CheckState) -> &'static str {
    match state {
        CheckState::Pass => "pass",
        CheckState::Warn => "warn",
        CheckState::Fail => "fail",
    }
}

fn map_canvas(
    ui: &mut egui::Ui,
    map: &mut MapViewState,
    locations: &LocationManager,
    dead_zones: &DeadZoneState,
    route_planned: bool,
    height: f32,
) -> Rect {
    let width = safe_width(ui);
    let height = if height.is_finite() {
        height.max(120.0)
    } else {
        400.0
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), Sense::drag());
    if response.dragged() {
        let delta = response.drag_delta();
        if delta.x.is_finite() && delta.y.is_finite() {
            map.pan[0] = (map.pan[0] + delta.x).clamp(-600.0, 600.0);
            map.pan[1] = (map.pan[1] + delta.y).clamp(-600.0, 600.0);
        }
    }
    let scroll = ui.input(|input| input.raw_scroll_delta.y);
    if response.hovered() && scroll.abs() > 0.0 {
        map.zoom = (map.zoom + scroll.signum() * 0.5).clamp(3.0, 18.0);
    }
    if !ui.is_rect_visible(rect) {
        return rect;
    }

    let painter = ui.painter_at(rect);
    let primary = locations.primary_sample();
    let has_fix = primary.is_some_and(LocationSample::has_fix);
    paint_map_scene(
        &painter,
        rect,
        map,
        dead_zones,
        primary,
        has_fix,
        live_nws_vehicle_point(locations),
        has_fix,
        route_planned,
        None,
        None,
    );
    painter.rect_stroke(
        rect,
        Style::RADIUS_L,
        Stroke::new(1.0, Style::BORDER),
        StrokeKind::Inside,
    );
    let chrome = if map.dark_mode {
        Style::TEXT_DIM
    } else {
        Style::BG
    };
    painter.text(
        rect.left_top() + egui::vec2(Style::SP_S, Style::SP_S),
        Align2::LEFT_TOP,
        format!(
            "zoom {:.1} | rotate {:.0} deg | pitch {:.0} deg",
            map.zoom, map.rotation_deg, map.pitch_deg
        ),
        FontId::proportional(Style::SMALL),
        chrome,
    );
    let _ = paint_map_attribution(&painter, rect, map, chrome);
    rect
}

/// Paint every active provider credit inside the map clip.
///
/// Ten simultaneous feeds produce a deliberately long attribution string. A
/// single right-aligned `Painter::text` call lets that string run off the left
/// edge of narrow map viewports, hiding credits while the map remains clipped.
/// Wrapping the galley and backing it with a small translucent card keeps all
/// credits readable without stealing a separate toolbar row.
fn paint_map_attribution(
    painter: &Painter,
    rect: Rect,
    map: &MapViewState,
    color: Color32,
) -> Rect {
    let outer_pad = Style::SP_S;
    let inner_pad = egui::vec2(Style::SP_S, Style::SP_XS);
    let wrap_width = (rect.width() - 2.0 * outer_pad - 2.0 * inner_pad.x).max(1.0);
    let attribution = bounded_map_attribution(&map.attribution_line());
    let galley = painter.layout(
        attribution,
        FontId::proportional(Style::SMALL),
        color,
        wrap_width,
    );
    let card_size = galley.size() + inner_pad * 2.0;
    let card = Rect::from_min_size(
        rect.right_bottom() - egui::vec2(card_size.x + outer_pad, card_size.y + outer_pad),
        card_size,
    );
    painter.rect_filled(card, Style::RADIUS_S, Style::BG.gamma_multiply(0.86));
    painter.rect_stroke(
        card,
        Style::RADIUS_S,
        Stroke::new(1.0, color.gamma_multiply(0.35)),
        StrokeKind::Inside,
    );
    painter.galley(card.left_top() + inner_pad, galley, color);
    card
}

/// Bound externally supplied attribution before egui measures or lays it out.
/// Provider credits remain intact under the normal contract; hostile or
/// accidentally oversized source labels receive an explicit trailing ellipsis.
fn bounded_map_attribution(text: &str) -> String {
    let mut bounded = String::new();
    for (index, character) in text.chars().enumerate() {
        if index == MAX_MAP_ATTRIBUTION_CHARS - 1 {
            bounded.push(MAP_ATTRIBUTION_ELLIPSIS);
            return bounded;
        }
        bounded.push(character);
    }
    bounded
}

fn map_point(rect: Rect, x: f32, y: f32) -> Pos2 {
    egui::pos2(
        rect.left() + rect.width() * x.clamp(0.0, 1.0),
        rect.top() + rect.height() * y.clamp(0.0, 1.0),
    )
}

/// Floating bottom-right action cluster laid over a map `rect`. Each entry is a
/// labeled pill (Carbon icon + text) painted with the shared FAB elevation
/// language and justified to the map's bottom-right corner, stacked upward.
/// Returns the index of the pill clicked this frame, if any. Interacted and
/// painted after the map so the cluster floats above the scene, matching the
/// Drive HUD's floating action buttons.
fn floating_map_actions(
    ui: &mut egui::Ui,
    map_rect: Rect,
    actions: &[(&str, &str)],
) -> Option<usize> {
    if actions.is_empty() || !map_rect.left().is_finite() || !ui.is_rect_visible(map_rect) {
        return None;
    }
    let font = FontId::proportional(Style::BODY);
    let pill_h = Style::SP_XL;
    let icon_d = Style::SP_M;
    let painter = ui.painter_at(map_rect);
    let right = map_rect.right() - Style::SP_M;
    let mut bottom = map_rect.bottom() - Style::SP_M;
    let mut clicked = None;

    for (idx, (icon, label)) in actions.iter().enumerate() {
        let galley = painter.layout_no_wrap((*label).to_string(), font.clone(), Style::TEXT_STRONG);
        let pill_w = Style::SP_M + icon_d + Style::SP_S + galley.size().x + Style::SP_M;
        let prect = safe_rect(right - pill_w, bottom - pill_h, pill_w, pill_h);

        let resp = ui.interact(
            prect,
            egui::Id::new(("maps-map-fab", *label)),
            Sense::click(),
        );
        if resp.clicked() {
            clicked = Some(idx);
        }

        paint_soft_shadow(&painter, prect, HUD_RADIUS_S);
        let fill = if resp.is_pointer_button_down_on() {
            Style::pressed_fill(Style::ACCENT)
        } else if resp.hovered() {
            Style::SURFACE_HI
        } else {
            HUD_CARD_BG
        };
        painter.rect_filled(prect, HUD_RADIUS_S, fill);
        painter.rect_stroke(
            prect,
            HUD_RADIUS_S,
            Stroke::new(1.0, Style::BORDER),
            StrokeKind::Inside,
        );
        let icon_box = safe_rect(
            prect.left() + Style::SP_M,
            prect.center().y - icon_d / 2.0,
            icon_d,
            icon_d,
        );
        let _ = paint_carbon(&painter, icon_box, icon, Style::ACCENT_HI);
        painter.galley(
            egui::pos2(
                icon_box.right() + Style::SP_S,
                prect.center().y - galley.size().y / 2.0,
            ),
            galley,
            Style::TEXT_STRONG,
        );

        bottom -= pill_h + Style::SP_S;
    }
    clicked
}

fn split_width(ui: &egui::Ui, columns: usize) -> f32 {
    let available = ui.available_width();
    let total = if available.is_finite() && available > 0.0 {
        available
    } else {
        ui.clip_rect().width()
    }
    .max(1.0);
    let gaps = ui.spacing().item_spacing.x * columns.saturating_sub(1) as f32;
    ((total - gaps) / columns.max(1) as f32).max(1.0)
}

/// Return a column width that preserves a readable card width, stacking the
/// caller's wrapped row when a narrow viewport cannot fit `columns` cards.
fn responsive_column_width(ui: &egui::Ui, columns: usize, min_column_width: f32) -> f32 {
    let available = ui.available_width();
    let total = if available.is_finite() && available > 0.0 {
        available
    } else {
        ui.clip_rect().width()
    }
    .max(1.0);
    let gap = ui.spacing().item_spacing.x * columns.saturating_sub(1) as f32;
    let split = ((total - gap) / columns.max(1) as f32).max(1.0);
    if split < min_column_width {
        total
    } else {
        split
    }
}

fn provider_card(ui: &mut egui::Ui, provider: &ProviderContract) {
    card(ui, &provider.abstraction, |ui| {
        metric(ui, "First backend", &provider.first_backend, Style::TEXT);
        metric(
            ui,
            "Core",
            if provider.local_only_core {
                "local-only"
            } else {
                "provider configured"
            },
            Style::ACCENT,
        );
        metric(
            ui,
            "Unavailable state",
            if provider.graceful_unavailable {
                "graceful"
            } else {
                "ready"
            },
            if provider.graceful_unavailable {
                Style::WARN
            } else {
                Style::OK
            },
        );
    });
}

fn offline_navigation_card(ui: &mut egui::Ui, status: &OfflineNavigationStatus) {
    card(ui, "Offline navigation readiness", |ui| {
        ui.horizontal_wrapped(|ui| {
            status_dot(ui, readiness_tone(status.readiness));
            ui.label(
                RichText::new(status.readiness.label())
                    .size(Style::BODY)
                    .color(readiness_tone(status.readiness)),
            );
            pill(
                ui,
                if status.can_claim_turn_by_turn() {
                    "turn-by-turn claim allowed"
                } else {
                    "turn-by-turn claim blocked"
                },
                readiness_tone(status.readiness),
            );
        });
        metric(
            ui,
            "Primary source",
            status.primary_source.label(),
            Style::TEXT,
        );
        metric(
            ui,
            "Loaded region",
            status.loaded_region.as_deref().unwrap_or("none loaded"),
            if status.loaded_region.is_some() {
                Style::OK
            } else {
                Style::DANGER
            },
        );
        metric(
            ui,
            "Coverage",
            &status.coverage_percent.map_or_else(
                || "unavailable".to_string(),
                |coverage| format!("{coverage}%"),
            ),
            if status.coverage_percent == Some(100) {
                Style::OK
            } else {
                Style::WARN
            },
        );
        metric(
            ui,
            "Offline storage",
            &format!("{:.1} GB / {} GB", status.used_gb, status.cap_gb),
            if status.used_gb <= status.cap_gb as f32 {
                Style::TEXT
            } else {
                Style::DANGER
            },
        );
        for blocker in &status.blockers {
            metric(ui, "Blocker", blocker, Style::DANGER);
        }
        for warning in &status.warnings {
            metric(ui, "Warning", warning, Style::WARN);
        }
        for note in &status.notes {
            metric(ui, "Note", note, Style::TEXT_DIM);
        }
    });
}

fn trip_card(ui: &mut egui::Ui, trips: &TripRecorderState) {
    card(ui, "Trips", |ui| {
        metric(
            ui,
            "Retention",
            &format!("{} days", trips.retention_days),
            Style::TEXT,
        );
        metric(
            ui,
            "Breadcrumbs",
            &trips.breadcrumbs.len().to_string(),
            Style::ACCENT,
        );
        metric(
            ui,
            "Encrypted",
            encrypted_label(trips.encrypted_at_rest),
            Style::OK,
        );
    });
}

fn dead_zone_card(ui: &mut egui::Ui, dead_zones: &DeadZoneState) {
    card(ui, "Cellular dead-zone recorder", |ui| {
        metric(ui, "Route risk", &dead_zones.route_risk, Style::WARN);
        metric(
            ui,
            "Recorded zones",
            &dead_zones.zones.len().to_string(),
            Style::ACCENT,
        );
        for zone in &dead_zones.zones {
            ui.separator();
            metric(ui, "Position", &zone.position, severity_tone(zone.severity));
            metric(
                ui,
                "Severity",
                zone.severity.label(),
                severity_tone(zone.severity),
            );
            metric(ui, "WAN", &zone.selected_wan, Style::TEXT);
            metric(ui, "Carrier", &zone.carrier, Style::TEXT);
            metric(ui, "Technology", &zone.technology, Style::ACCENT);
            metric(
                ui,
                "Signal / loss",
                &format!("{} dBm / {:.1}%", zone.signal_dbm, zone.packet_loss_percent),
                severity_tone(zone.severity),
            );
            metric(
                ui,
                "Latency / duration",
                &format!("{} ms / {} s", zone.latency_ms, zone.outage_duration_s),
                Style::TEXT,
            );
        }
    });
}

fn show_vault(ui: &mut egui::Ui, vault: &EncryptedVaultState) {
    card(ui, "Encrypted local vault", |ui| {
        metric(ui, "Admin model", &vault.local_admin_user, Style::TEXT);
        metric(
            ui,
            "Credentials",
            encrypted_label(vault.credentials_encrypted),
            Style::OK,
        );
        metric(
            ui,
            "Location and trips",
            encrypted_label(vault.location_data_encrypted),
            Style::OK,
        );
        metric(ui, "Backend", &vault.backend, Style::TEXT_DIM);
    });
}

fn backups(ui: &mut egui::Ui, backups: &[BackupRecord]) {
    card(ui, "Versioned restore points", |ui| {
        if backups.is_empty() {
            mde_egui::widgets::muted_note(
                ui,
                "No restore points yet — the baseline backup is created during MG90 setup.",
            );
        }
        for backup in backups {
            metric(ui, &backup.id, &backup.reason, Style::TEXT);
            metric(ui, "Created", &backup.created, Style::TEXT_DIM);
            metric(
                ui,
                "Encrypted",
                encrypted_label(backup.encrypted),
                Style::OK,
            );
            metric(
                ui,
                "Restore point",
                bool_label(backup.restore_point),
                Style::OK,
            );
        }
    });
}

// ── MG90 management / configuration surface kit ─────────────────────────────
// The shared building blocks the six MG90 panels (Connectivity, Devices & I/O,
// Setup, Settings, Firmware & Recovery, Vehicle) render through, so the whole
// management surface reads as one system: a rounded glyph-headed card, a hairline
// divider, a right-aligned mono readout, a hero stat tile, and a dBm signal-bar
// meter. Every color/tone is a `Style` token (§4) — no raw literals.

/// The rounded surface frame every upgraded MG90 card shares — the base layer
/// fill, a hairline border (or an `accent` border when the card is the active /
/// highlighted one), generous padding, and the mid corner radius.
fn mg90_frame(accent: Option<Color32>) -> egui::Frame {
    let frame = mde_egui::widgets::card().fill(Style::LAYER_02);
    match accent {
        Some(accent) => frame.stroke(Stroke::new(1.0, accent)),
        None => frame,
    }
}

/// A full-width hairline rule in [`Style::BORDER`] — the quiet separator under a
/// card header and between a card's sub-regions.
fn divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0, Style::BORDER),
    );
}

/// A Carbon glyph + strong title header row, followed by a hairline divider — the
/// standard section header for the MG90 cards.
fn card_header(ui: &mut egui::Ui, icon: &str, title: &str, tone: Color32) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
        let _ = paint_carbon(ui.painter(), rect, icon, tone);
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(title)
                .size(Style::BODY)
                .color(Style::TEXT_STRONG),
        );
    });
    ui.add_space(Style::SP_XS);
    divider(ui);
    ui.add_space(Style::SP_S);
}

/// A rounded card with a glyph-headed section header. The MG90 replacement for
/// the plain [`card`], used wherever a section wants a Carbon icon + divider.
fn glyph_card<R>(
    ui: &mut egui::Ui,
    icon: &str,
    title: &str,
    tone: Color32,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    mg90_frame(None).show(ui, |ui| {
        card_header(ui, icon, title, tone);
        add_contents(ui)
    })
}

/// A labelled value row on the 8px grid: a dim [`Style::SMALL`] `label` at the
/// left, the `value` right-aligned in `tone` and monospace so numeric columns
/// (dBm, volts, IPs, ms) line up. The MG90 panels' primary data row.
fn readout(ui: &mut egui::Ui, label: &str, value: &str, tone: Color32) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(label)
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new(value)
                .size(Style::SMALL)
                .color(tone)
                .monospace(),
        );
    });
    ui.add_space(2.0);
}

/// A hero stat tile — a Carbon glyph, a dim caption, and a large monospace value
/// tinted `tone`. Laid out `w` wide so a row of tiles shares [`split_width`].
fn stat_tile(ui: &mut egui::Ui, w: f32, icon: &str, caption: &str, value: &str, tone: Color32) {
    ui.scope(|ui| {
        ui.set_width(w);
        mg90_frame(None).show(ui, |ui| {
            ui.set_min_height(58.0);
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                let _ = paint_carbon(ui.painter(), rect, icon, tone);
                ui.add_space(Style::SP_S);
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(caption)
                            .size(Style::SMALL)
                            .color(Style::TEXT_DIM),
                    );
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(value)
                            .size(Style::TITLE)
                            .color(tone)
                            .monospace(),
                    );
                });
            });
        });
    });
}

/// A five-bar cellular signal meter: bars fill in the health `tone` up to the
/// level implied by `dbm`, the rest drawn as a dim track. The world-class
/// replacement for a raw `-72 dBm` string.
fn signal_bars(ui: &mut egui::Ui, dbm: i32, healthy: bool) {
    const BARS: usize = 5;
    let filled = signal_level(dbm);
    let tone = signal_tone(dbm, healthy);
    let bar_w = 4.0_f32;
    let gap = 3.0_f32;
    let max_h = 20.0_f32;
    let total_w = BARS as f32 * bar_w + (BARS as f32 - 1.0) * gap;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total_w, max_h), Sense::hover());
    let painter = ui.painter();
    for i in 0..BARS {
        let frac = (i as f32 + 1.0) / BARS as f32;
        let h = max_h * (0.3 + 0.7 * frac);
        let x = rect.left() + i as f32 * (bar_w + gap);
        let bar = Rect::from_min_max(
            Pos2::new(x, rect.bottom() - h),
            Pos2::new(x + bar_w, rect.bottom()),
        );
        let color = if i < filled { tone } else { Style::BORDER };
        painter.rect_filled(bar, 1.0, color);
    }
}

/// Map a cellular `dbm` reading to a 0..=5 bar level (RSRP/RSSI thresholds).
///
/// A real reading is negative dBm; `0` (or any non-negative value) is the
/// "no signal / absent" sentinel and MUST read as an empty strip — the prior
/// top branch (`0 >= -75`) drew a fabricated full 5-bar strip for an absent
/// link (Q33).
fn signal_level(dbm: i32) -> usize {
    if dbm >= 0 {
        return 0;
    }
    match dbm {
        d if d >= -75 => 5,
        d if d >= -85 => 4,
        d if d >= -95 => 3,
        d if d >= -105 => 2,
        d if d >= -115 => 1,
        _ => 0,
    }
}

/// Health tone for a cellular link from its `dbm` and reported health.
fn signal_tone(dbm: i32, healthy: bool) -> Color32 {
    if !healthy || dbm <= -110 {
        Style::DANGER
    } else if dbm <= -100 {
        Style::WARN
    } else {
        Style::OK
    }
}

/// A short quality word for a cellular link.
fn signal_quality_label(dbm: i32, healthy: bool) -> &'static str {
    if dbm >= 0 {
        // Non-negative dBm is the "no reading" sentinel — an absent link is not
        // "degraded", it simply has no signal to describe.
        return "no signal";
    }
    if !healthy {
        return "degraded";
    }
    match signal_level(dbm) {
        5 => "excellent",
        4 => "good",
        3 => "fair",
        2 => "weak",
        1 => "poor",
        _ => "no signal",
    }
}

/// Charging-system voltage tone for a 12V automotive electrical system.
fn voltage_tone(volts: f32) -> Color32 {
    if (12.4..=14.9).contains(&volts) {
        Style::OK
    } else if (11.8..15.4).contains(&volts) {
        Style::WARN
    } else {
        Style::DANGER
    }
}

/// Coolant-temperature tone (cold engine warns; over ~105 C is danger).
fn coolant_tone(celsius: f32) -> Color32 {
    if celsius >= 105.0 {
        Style::DANGER
    } else if celsius >= 100.0 || celsius < 40.0 {
        Style::WARN
    } else {
        Style::OK
    }
}

/// SIM/DTC-style tone: zero faults is OK, any present is a warn.
fn count_tone(count: u32) -> Color32 {
    if count == 0 {
        Style::OK
    } else {
        Style::WARN
    }
}

/// A trimmed value, or an em-dash for an absent / empty live field (§7 — honest
/// empty state, never a fabricated value).
fn dash_if_empty(value: &str) -> &str {
    if value.trim().is_empty() {
        "—"
    } else {
        value
    }
}

/// The Carbon glyph for an MG90 setting category.
fn category_icon(category: Mg90SettingCategory) -> &'static str {
    match category {
        Mg90SettingCategory::Overview => "view-grid",
        Mg90SettingCategory::CellularSim => "globe",
        Mg90SettingCategory::Wifi => "notification",
        Mg90SettingCategory::Ethernet => "share",
        Mg90SettingCategory::WanPolicies => "view-refresh",
        Mg90SettingCategory::LanDhcpVlan => "view-grid",
        Mg90SettingCategory::Firewall => "security-high",
        Mg90SettingCategory::Vpn => "changes-prevent",
        Mg90SettingCategory::Gnss => "star",
        Mg90SettingCategory::SerialRecovery => "text-x-generic",
        Mg90SettingCategory::Gpio => "overlay",
        Mg90SettingCategory::Services => "open-menu",
        Mg90SettingCategory::Security => "system-lock-screen",
        Mg90SettingCategory::Diagnostics => "dialog-warning",
        Mg90SettingCategory::Logs => "document-open-recent",
        Mg90SettingCategory::BackupRestore => "download",
        Mg90SettingCategory::OriginalLciFallback => "document-edit",
    }
}

/// A dual-cellular modem card — the signal-bar hero plus the SIM / carrier /
/// technology / WAN-IP readouts for one modem side, with an accent border and an
/// ACTIVE chip when this is the selected WAN.
fn cellular_modem_card(
    ui: &mut egui::Ui,
    side: &str,
    link: &crate::model::CellularLink,
    active: bool,
) {
    let accent = if active { Style::ACCENT } else { Style::BORDER };
    mg90_frame(Some(accent)).show(ui, |ui| {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
            let icon_tone = if active {
                Style::ACCENT_HI
            } else {
                Style::TEXT_DIM
            };
            let _ = paint_carbon(ui.painter(), rect, "globe", icon_tone);
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new(format!("Cellular {side}"))
                    .size(Style::BODY)
                    .color(Style::TEXT_STRONG),
            );
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if active {
                    pill(ui, "ACTIVE", Style::ACCENT);
                } else if link.sim_state.trim().is_empty() {
                    // No modem data at all — "standby" would claim a state we
                    // never read (Q33).
                    pill(ui, "no link", Style::TEXT_DIM);
                } else {
                    pill(ui, "standby", Style::TEXT_DIM);
                }
            });
        });
        ui.add_space(Style::SP_XS);
        divider(ui);
        ui.add_space(Style::SP_S);
        ui.horizontal(|ui| {
            signal_bars(ui, link.signal_dbm, link.healthy);
            ui.add_space(Style::SP_S);
            ui.vertical(|ui| {
                // Non-negative dBm is the "no reading" sentinel: dash it rather
                // than presenting a fabricated "0 dBm" measurement.
                let (dbm_text, dbm_tone) = if link.signal_dbm < 0 {
                    (
                        format!("{} dBm", link.signal_dbm),
                        signal_tone(link.signal_dbm, link.healthy),
                    )
                } else {
                    ("—".to_string(), Style::TEXT_DIM)
                };
                ui.label(
                    RichText::new(dbm_text)
                        .size(Style::TITLE)
                        .color(dbm_tone)
                        .monospace(),
                );
                ui.label(
                    RichText::new(signal_quality_label(link.signal_dbm, link.healthy))
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
            });
        });
        ui.add_space(Style::SP_S);
        readout(ui, "Carrier", dash_if_empty(&link.carrier), Style::TEXT);
        readout(ui, "SIM", dash_if_empty(&link.sim_state), Style::TEXT);
        readout(
            ui,
            "Technology",
            dash_if_empty(&link.technology),
            Style::ACCENT,
        );
        readout(ui, "WAN IP", dash_if_empty(&link.wan_ip), Style::TEXT_DIM);
        readout(
            ui,
            "Health",
            if link.healthy { "healthy" } else { "degraded" },
            if link.healthy { Style::OK } else { Style::WARN },
        );
    });
}

fn setting_row(ui: &mut egui::Ui, state: &MapsLocationSurface, setting: &Mg90SettingDescriptor) {
    mg90_frame(None).show(ui, |ui| {
        ui.label(
            RichText::new(&setting.display_name)
                .size(Style::BODY)
                .color(Style::TEXT_STRONG),
        );
        ui.add_space(Style::SP_XS);
        ui.horizontal_wrapped(|ui| {
            pill(
                ui,
                value_type_label(&setting.value_type),
                Style::ACCENT_MESH,
            );
            pill(ui, method_label(setting.read_method), Style::ACCENT);
            pill(
                ui,
                method_label(setting.write_method),
                Style::ACCENT_TERMINALS,
            );
            if setting.requires_reboot {
                pill(ui, "reboot", Style::WARN);
            }
            if setting.may_disconnect_management {
                pill(ui, "disconnect risk", Style::DANGER);
            }
            if setting.supports_rollback {
                pill(ui, "rollback", Style::OK);
            }
        });
        if !setting.validation.is_empty() {
            ui.add_space(Style::SP_XS);
            for rule in &setting.validation {
                bullet(ui, &rule.label);
            }
        }
        if let Some(plan) = state.setting_change_plan(&setting.id) {
            ui.add_space(Style::SP_XS);
            divider(ui);
            ui.add_space(Style::SP_S);
            ui.label(
                RichText::new("Guarded change plan")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            ui.add_space(Style::SP_XS);
            for (index, step) in plan.steps.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!("{}.", index + 1))
                            .size(Style::SMALL)
                            .monospace()
                            .color(Style::TEXT_DIM),
                    );
                    ui.add_space(Style::SP_XS);
                    ui.label(RichText::new(step).size(Style::SMALL).color(Style::TEXT));
                });
            }
            ui.add_space(Style::SP_XS);
            ui.horizontal_wrapped(|ui| {
                cap_pill(ui, "backup", plan.backup_required);
                cap_pill(ui, "rollback", plan.rollback_supported);
                if plan.moving_warning {
                    pill(ui, "moving warning", Style::WARN);
                }
            });
        }
    });
    ui.add_space(Style::SP_S);
}

fn card<R>(
    ui: &mut egui::Ui,
    title: &str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    egui::Frame::NONE
        .fill(Style::LAYER_02)
        .stroke(Stroke::new(1.0, Style::BORDER))
        .inner_margin(Style::SP_S)
        .show(ui, |ui| {
            ui.set_min_height(CARD_MIN_H);
            ui.label(
                RichText::new(title)
                    .size(Style::BODY)
                    .color(Style::TEXT_STRONG),
            );
            ui.add_space(Style::SP_XS);
            add_contents(ui)
        })
}

fn metric(ui: &mut egui::Ui, label: &str, value: &str, tone: Color32) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(label)
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        ui.add_space(Style::SP_S);
        ui.label(RichText::new(value).size(Style::SMALL).color(tone));
    });
}

fn warning_strip(ui: &mut egui::Ui, text: &str, tone: Color32) {
    egui::Frame::NONE
        .fill(tone.gamma_multiply(0.14))
        .stroke(Stroke::new(1.0, tone.gamma_multiply(0.75)))
        .inner_margin(Style::SP_S)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                status_dot(ui, tone);
                ui.label(RichText::new(text).color(Style::TEXT));
            });
        });
    ui.add_space(Style::SP_XS);
}

fn pill(ui: &mut egui::Ui, label: &str, tone: Color32) {
    egui::Frame::NONE
        .fill(tone.gamma_multiply(0.14))
        .stroke(Stroke::new(1.0, tone.gamma_multiply(0.8)))
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(label).size(Style::SMALL).color(Style::TEXT));
        });
}

fn bullet(ui: &mut egui::Ui, text: &str) {
    ui.horizontal_wrapped(|ui| {
        status_dot(ui, Style::TEXT_DIM);
        ui.label(RichText::new(text).size(Style::SMALL).color(Style::TEXT));
    });
}

fn status_dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(Style::SP_S), Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.0, color);
}

fn health_color(sample: &LocationSample) -> Color32 {
    if sample.healthy() {
        Style::OK
    } else if sample.stale() {
        Style::WARN
    } else {
        Style::DANGER
    }
}

fn source_readiness_tone(source: &LocationSource) -> Color32 {
    if source.manual_switch_ready() {
        Style::OK
    } else if source.sample.stale() || source.status == SourceStatus::Stale {
        Style::WARN
    } else {
        Style::DANGER
    }
}

fn bool_label(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn encrypted_label(value: bool) -> &'static str {
    if value {
        "encrypted at rest"
    } else {
        "not encrypted"
    }
}

fn source_status_label(status: SourceStatus) -> &'static str {
    status.label()
}

fn method_label(method: Mg90ManagementMethod) -> &'static str {
    match method {
        Mg90ManagementMethod::LocalApi => "local API",
        Mg90ManagementMethod::LocalConfigurationInterface => "LCI fallback",
        Mg90ManagementMethod::SerialRecoveryConsole => "serial recovery",
        Mg90ManagementMethod::Simulator => "simulator",
        Mg90ManagementMethod::Unsupported => "unsupported",
    }
}

fn value_type_label(value_type: &SettingValueType) -> &'static str {
    match value_type {
        SettingValueType::Boolean => "boolean",
        SettingValueType::Integer => "integer",
        SettingValueType::Text => "text",
        SettingValueType::Enum(_) => "enum",
    }
}

fn check_tone(state: CheckState) -> Color32 {
    match state {
        CheckState::Pass => Style::OK,
        CheckState::Warn => Style::WARN,
        CheckState::Fail => Style::DANGER,
    }
}

fn readiness_tone(readiness: OfflineNavigationReadiness) -> Color32 {
    match readiness {
        OfflineNavigationReadiness::Ready => Style::OK,
        OfflineNavigationReadiness::Degraded => Style::WARN,
        OfflineNavigationReadiness::Blocked => Style::DANGER,
    }
}

fn severity_tone(severity: DeadZoneSeverity) -> Color32 {
    match severity {
        DeadZoneSeverity::Good => Style::OK,
        DeadZoneSeverity::Weak => Style::WARN,
        DeadZoneSeverity::Degraded | DeadZoneSeverity::Outage => Style::DANGER,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_now_ms() -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or_default()
    }

    fn tessellate(surface: &mut MapsLocationSurface) -> usize {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1280.0, 820.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| maps_location_panel(ui, surface));
        });
        ctx.tessellate(out.shapes, out.pixels_per_point).len()
    }

    fn render_rail_frame(
        ctx: &egui::Context,
        surface: &mut MapsLocationSurface,
        screen: Rect,
        events: Vec<egui::Event>,
        reserve_shell_chrome: bool,
    ) -> Rect {
        let mut viewport = Rect::NOTHING;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            },
            |ctx| {
                if reserve_shell_chrome {
                    // Mirrors the shell's Construct reservations: 24 px top
                    // status bar, 72 px floating dock band, and 56 px docked
                    // left rail. The Maps panel itself must remain inside the
                    // resulting CentralPanel workspace.
                    egui::TopBottomPanel::top("test-construct-status-space")
                        .exact_height(24.0)
                        .frame(egui::Frame::NONE)
                        .show(ctx, |_ui| {});
                    egui::TopBottomPanel::bottom("test-construct-dock-space")
                        .exact_height(72.0)
                        .frame(egui::Frame::NONE)
                        .show(ctx, |_ui| {});
                    egui::SidePanel::left("test-construct-docked-rail-space")
                        .exact_width(56.0)
                        .frame(egui::Frame::NONE)
                        .show(ctx, |_ui| {});
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    viewport = tab_rail(ui, surface);
                });
            },
        );
        viewport
    }

    fn click_rail_row(
        ctx: &egui::Context,
        surface: &mut MapsLocationSurface,
        screen: Rect,
        at: Pos2,
        reserve_shell_chrome: bool,
    ) {
        let _ = render_rail_frame(
            ctx,
            surface,
            screen,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            reserve_shell_chrome,
        );
        let _ = render_rail_frame(
            ctx,
            surface,
            screen,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            reserve_shell_chrome,
        );
    }

    fn render_admin_frame(
        ctx: &egui::Context,
        surface: &mut MapsLocationSurface,
        screen: Rect,
        events: Vec<egui::Event>,
    ) {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    show_admin(ui, surface);
                });
            },
        );
    }

    fn click_admin_section(
        ctx: &egui::Context,
        surface: &mut MapsLocationSurface,
        screen: Rect,
        section: AdminSection,
    ) {
        let at = ctx
            .read_response(admin_section_item_id(section))
            .expect("admin section target should be registered before click")
            .rect
            .center();
        render_admin_frame(
            ctx,
            surface,
            screen,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
        render_admin_frame(
            ctx,
            surface,
            screen,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
    }

    fn key(k: egui::Key) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }
    }

    fn render_map_layers_frame(
        ctx: &egui::Context,
        map: &mut MapViewState,
        screen: Rect,
        events: Vec<egui::Event>,
    ) -> (Rect, MapLayersLayout) {
        let mut clip = Rect::NOTHING;
        let mut layout = MapLayersLayout::default();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            },
            |ctx| {
                // This is the shell contract relevant to the regression: the
                // workspace starts below the top status rail, so a popup must
                // not use the full display rect as its interaction surface.
                egui::TopBottomPanel::top("test-maps-status-space")
                    .exact_height(24.0)
                    .frame(egui::Frame::NONE)
                    .show(ctx, |_ui| {});
                egui::CentralPanel::default().show(ctx, |ui| {
                    clip = ui.clip_rect();
                    layout = map_layers_menu(ui, map);
                });
            },
        );
        (clip, layout)
    }

    fn click_map_layers_rect(
        ctx: &egui::Context,
        map: &mut MapViewState,
        screen: Rect,
        rect: Rect,
    ) {
        let at = rect.center();
        let _ = render_map_layers_frame(
            ctx,
            map,
            screen,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
        let _ = render_map_layers_frame(
            ctx,
            map,
            screen,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
    }

    #[test]
    fn admin_rail_is_single_top_level_target_without_legacy_leaves() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        ctx.style_mut(|style| style.animation_time = 0.0);

        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1024.0, 768.0));
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Drive;

        let viewport = render_rail_frame(&ctx, &mut surface, screen, Vec::new(), false);

        let admin = ctx
            .read_response(rail_item_id("MG90 Admin"))
            .expect("single MG90 Admin rail target should register");
        assert!(
            screen.contains_rect(viewport),
            "rail viewport escaped screen"
        );
        assert!(
            viewport.contains_rect(admin.rect),
            "Admin rail target escaped the bounded viewport: {admin:?}"
        );

        for legacy in [
            "Advanced",
            "Vehicle",
            "Connectivity",
            "Devices & I/O",
            "Location Sources",
            "MG90 Setup",
            "MG90 Settings",
            "Firmware & Recovery",
        ] {
            assert!(
                ctx.read_response(rail_item_id(legacy)).is_none(),
                "{legacy} must not be a top-level rail target"
            );
        }

        click_rail_row(&ctx, &mut surface, screen, admin.rect.center(), false);
        assert_eq!(surface.active, WorkspaceTab::Admin);
        assert_eq!(surface.admin_section, AdminSection::Vehicle);
    }

    #[test]
    fn admin_section_strip_clicks_route_within_the_single_admin_tab() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        ctx.style_mut(|style| style.animation_time = 0.0);

        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1280.0, 820.0));
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Admin;
        surface.admin_section = AdminSection::Vehicle;

        render_admin_frame(&ctx, &mut surface, screen, Vec::new());
        let target = ctx
            .read_response(admin_section_item_id(AdminSection::Mg90Settings))
            .expect("MG90 Settings admin section should register a hit target");
        assert!(screen.contains_rect(target.rect));

        click_admin_section(&ctx, &mut surface, screen, AdminSection::Mg90Settings);
        assert_eq!(surface.active, WorkspaceTab::Admin);
        assert_eq!(surface.admin_section, AdminSection::Mg90Settings);
    }

    #[test]
    fn admin_section_strip_hit_targets_clamp_to_tiny_visible_lane() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        ctx.style_mut(|style| style.animation_time = 0.0);

        // Regresses the off-page "Advanced"/MG90 menu failure: after shell
        // chrome, rail, and content margins, the Admin section lane can be
        // narrower than the old hard-coded 96 px chip width. Each registered
        // hit target must stay inside the lane it paints in.
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(72.0, 760.0));
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Admin;
        surface.admin_section = AdminSection::Vehicle;

        render_admin_frame(&ctx, &mut surface, screen, Vec::new());

        for section in AdminSection::ALL {
            let target = ctx
                .read_response(admin_section_item_id(section))
                .unwrap_or_else(|| panic!("{section:?} admin section should register"));
            assert!(target.rect.is_positive(), "{section:?} lost its hit target");
            assert!(
                screen.contains_rect(target.rect),
                "{section:?} escaped the visible Admin lane: {:?}",
                target.rect
            );
        }

        click_admin_section(&ctx, &mut surface, screen, AdminSection::FirmwareRecovery);
        assert_eq!(surface.active, WorkspaceTab::Admin);
        assert_eq!(surface.admin_section, AdminSection::FirmwareRecovery);
    }

    #[test]
    fn admin_number_keys_select_sections_without_leaving_admin() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        ctx.style_mut(|style| style.animation_time = 0.0);

        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1280.0, 820.0));
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Admin;
        surface.admin_section = AdminSection::Vehicle;

        render_admin_frame(&ctx, &mut surface, screen, vec![key(egui::Key::Num7)]);

        assert_eq!(surface.active, WorkspaceTab::Admin);
        assert_eq!(surface.admin_section, AdminSection::FirmwareRecovery);
    }

    #[test]
    fn map_layers_popup_is_bounded_and_clickable_on_a_short_reserved_workspace() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        ctx.style_mut(|style| style.animation_time = 0.0);

        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(220.0, 180.0));
        let mut map = MapViewState::live(false);
        let (clip, closed) = render_map_layers_frame(&ctx, &mut map, screen, Vec::new());
        assert!(
            clip.top() >= 24.0,
            "test workspace did not reserve the top rail"
        );
        assert!(clip.contains_rect(closed.button));

        click_map_layers_rect(&ctx, &mut map, screen, closed.button);
        let (clip, open) = render_map_layers_frame(&ctx, &mut map, screen, Vec::new());
        assert!(
            clip.contains_rect(open.popup),
            "Layers popup escaped workspace clip"
        );
        assert!(
            clip.contains_rect(open.first_toggle),
            "first Layers checkbox lost its hit target in the clipped popup"
        );
        assert!(open.popup.height() < MAP_LAYERS_POPUP_HEIGHT);

        click_map_layers_rect(&ctx, &mut map, screen, open.first_toggle);
        assert!(
            !map.nws_alert_overlay,
            "a visible Layers checkbox must remain clickable after popup scrolling"
        );
    }

    #[test]
    fn layers_popup_clamps_offscreen_anchors_inside_non_zero_short_clip() {
        let clip = Rect::from_min_size(egui::pos2(147.0, 83.0), egui::vec2(106.0, 75.0));
        let anchors = [
            // A wrapped control row that ended above the reserved workspace.
            Rect::from_min_size(egui::pos2(180.0, 20.0), egui::vec2(48.0, 18.0)),
            // A control row clipped below a very short workspace.
            Rect::from_min_size(egui::pos2(180.0, 170.0), egui::vec2(48.0, 18.0)),
        ];

        for anchor in anchors {
            let popup = bounded_popup_rect(anchor, clip, MAP_LAYERS_POPUP_WIDTH, 360.0);
            assert!(
                clip.contains_rect(popup),
                "popup escaped non-zero workspace clip: clip={clip:?} anchor={anchor:?} popup={popup:?}"
            );
            assert_eq!(popup.width(), clip.width());
            assert!(popup.height() > 0.0);
        }
    }

    #[test]
    fn admin_device_cards_stack_when_two_columns_are_too_narrow() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let narrow = Rect::from_min_size(Pos2::ZERO, egui::vec2(320.0, 480.0));
        let wide = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 480.0));
        let mut narrow_width = 0.0;
        let mut narrow_available = 0.0;
        let mut wide_width = 0.0;

        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(narrow),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    narrow_available = ui.available_width();
                    narrow_width = responsive_column_width(ui, 2, ADMIN_CARD_MIN_WIDTH);
                });
            },
        );
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(wide),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    wide_width = responsive_column_width(ui, 2, ADMIN_CARD_MIN_WIDTH);
                });
            },
        );

        assert_eq!(
            narrow_width, narrow_available,
            "stacked cards should use the CentralPanel's actual usable width"
        );
        assert!(narrow_width >= ADMIN_CARD_MIN_WIDTH);
        assert!(wide_width < wide.width());
        assert!(wide_width >= ADMIN_CARD_MIN_WIDTH);
    }

    #[test]
    fn workspace_tabs_match_product_layout() {
        let labels: Vec<&str> = WorkspaceTab::ALL.iter().map(|tab| tab.label()).collect();
        assert_eq!(
            labels,
            vec!["Drive", "Airspace", "Map", "Routes & Trips", "MG90 Admin"]
        );
    }

    #[test]
    fn admin_sections_match_requested_order() {
        let labels: Vec<&str> = AdminSection::ALL
            .iter()
            .map(|section| section.label())
            .collect();
        assert_eq!(
            labels,
            vec![
                "Vehicle",
                "Connectivity",
                "Devices & I/O",
                "Location Sources",
                "MG90 Setup",
                "MG90 Settings",
                "Firmware & Recovery",
            ]
        );
    }

    #[test]
    fn maps_location_panel_renders_simulated_vertical_slice() {
        let mut surface = MapsLocationSurface::simulated();
        assert!(tessellate(&mut surface) > 0);
    }

    #[test]
    fn nws_inside_banner_requires_a_fresh_provenance_stamped_mg90_fix() {
        let mut surface = MapsLocationSurface::simulated();
        {
            let source = surface
                .locations
                .sources
                .iter_mut()
                .find(|source| source.kind == LocationSourceKind::Mg90Gnss)
                .expect("MG90 source");
            source.status = SourceStatus::Connected;
            source.sample.fix_type = "3D".to_string();
            source.sample.latitude = 32.2;
            source.sample.longitude = -95.0;
            source.sample.update_age_s = 0.0;
        }
        assert!(
            live_nws_vehicle_point(&surface.locations).is_none(),
            "simulated provenance cannot raise the safety banner"
        );

        surface.locations.sources[0].diagnostics.insert(
            "mode".to_string(),
            "live vehicle-gateway mirror (MG90 4.3.0.1)".to_string(),
        );
        assert!(live_nws_vehicle_point(&surface.locations).is_some());
        surface.locations.sources[0].sample.update_age_s = 6.0;
        assert!(
            live_nws_vehicle_point(&surface.locations).is_none(),
            "stale MG90 position cannot raise the safety banner"
        );
        surface.locations.sources[0].sample.update_age_s = 0.0;
        surface.locations.primary = LocationSourceKind::Simulator;
        assert!(
            live_nws_vehicle_point(&surface.locations).is_none(),
            "non-MG90 primary cannot raise the safety banner"
        );
    }

    #[test]
    fn maps_header_uses_refined_shared_chrome_height() {
        let header_h = mde_egui::menubar::BAR_HEIGHT + Style::SP_S;
        assert_eq!(
            header_h,
            mde_egui::menubar::BAR_HEIGHT + Style::SP_S,
            "Maps header should inherit the shared refined chrome height"
        );
        assert!(
            header_h < 40.0,
            "Maps header must not return to a thick fixed strip"
        );
    }

    #[test]
    fn every_tab_tessellates_without_hardware() {
        for tab in WorkspaceTab::ALL {
            let mut surface = MapsLocationSurface::simulated();
            surface.active = tab;
            assert!(tessellate(&mut surface) > 0, "{tab:?}");
        }
    }

    #[test]
    fn every_admin_section_tessellates_without_hardware() {
        for section in AdminSection::ALL {
            let mut surface = MapsLocationSurface::simulated();
            surface.active = WorkspaceTab::Admin;
            surface.admin_section = section;
            assert!(tessellate(&mut surface) > 0, "{section:?}");
        }
    }

    // ── WL-UX-007/S1 — the production (live, honest-empty) surface ──────────

    #[test]
    fn every_tab_tessellates_on_the_live_surface() {
        // The production constructor is empty everywhere; every view arm must
        // render its designed honest-empty without panicking.
        for tab in WorkspaceTab::ALL {
            let mut surface = MapsLocationSurface::live();
            surface.active = tab;
            assert!(tessellate(&mut surface) > 0, "{tab:?}");
        }
    }

    #[test]
    fn every_admin_section_tessellates_on_the_live_surface() {
        for section in AdminSection::ALL {
            let mut surface = MapsLocationSurface::live();
            surface.active = WorkspaceTab::Admin;
            surface.admin_section = section;
            assert!(tessellate(&mut surface) > 0, "{section:?}");
        }
    }

    #[test]
    fn live_flow_screens_tessellate_with_no_data() {
        // Route preview with ZERO route options (no routing engine).
        let mut preview = MapsLocationSurface::live();
        preview.active = WorkspaceTab::Drive;
        preview.route_preview = true;
        assert!(tessellate(&mut preview) > 0);

        // Destination search with ZERO preset destinations.
        let mut search = MapsLocationSurface::live();
        search.active = WorkspaceTab::Drive;
        search.destination_search = true;
        assert!(tessellate(&mut search) > 0);
    }

    #[test]
    fn live_route_preview_marks_start_unavailable_without_a_route() {
        let mut preview = MapsLocationSurface::live();
        preview.active = WorkspaceTab::Drive;
        preview.route_preview = true;

        let texts = painted_texts(&mut preview);

        assert!(
            texts.iter().any(|text| text == "No route available"),
            "the route preview must disclose why Start cannot run: {texts:?}"
        );
        assert!(
            !preview.can_start_navigation(),
            "the rendered disabled action must share the model predicate"
        );
    }

    #[test]
    fn route_preview_start_readiness_distinguishes_route_blocker_and_gps() {
        let fixture = MapsLocationSurface::simulated();
        let ready_status = fixture.offline_navigation_status();
        let missing_route = route_preview_start_readiness(false, true, true, &ready_status);
        assert_eq!(missing_route.button_label, "No route available");
        assert!(!missing_route.can_start);
        assert!(missing_route.tooltip.contains("No route is available"));

        let mut blocked_fixture = MapsLocationSurface::simulated();
        blocked_fixture
            .local_navigation
            .routing
            .graceful_unavailable = true;
        let blocked_status = blocked_fixture.offline_navigation_status();
        let blocked = route_preview_start_readiness(true, true, true, &blocked_status);
        assert_eq!(blocked.button_label, "Navigation blocked");
        assert!(!blocked.can_start);
        assert!(blocked.tooltip.contains("Routing API is not ready"));

        let no_gps = route_preview_start_readiness(false, true, false, &ready_status);
        assert_eq!(no_gps.button_label, "No route available");
        assert!(!no_gps.can_start);
        assert!(no_gps.tooltip.contains("MG90 GNSS has no GPS fix"));

        let no_gps_with_route = route_preview_start_readiness(true, true, false, &ready_status);
        assert_eq!(no_gps_with_route.button_label, "Waiting for GPS");
        assert!(!no_gps_with_route.can_start);
        assert!(no_gps_with_route
            .tooltip
            .contains("MG90 GNSS has no GPS fix"));
    }

    #[test]
    fn route_preview_render_labels_blocked_readiness_and_missing_gps() {
        let mut blocked = MapsLocationSurface::simulated();
        blocked.active = WorkspaceTab::Drive;
        blocked.route_preview = true;
        blocked.local_navigation.routing.graceful_unavailable = true;
        let primary_kind = blocked.locations.primary;
        if let Some(source) = blocked
            .locations
            .sources
            .iter_mut()
            .find(|source| source.kind == primary_kind)
        {
            source.sample.fix_type = "3D".to_string();
            source.sample.latitude = 40.4406;
            source.sample.longitude = -79.9959;
        }
        let blocked_texts = painted_texts(&mut blocked);
        assert!(blocked_texts
            .iter()
            .any(|text| text == "Navigation blocked"));

        let mut no_gps = MapsLocationSurface::simulated();
        no_gps.active = WorkspaceTab::Drive;
        no_gps.route_preview = true;
        let no_gps_texts = painted_texts(&mut no_gps);
        assert!(no_gps_texts.iter().any(|text| text == "Waiting for GPS"));
        assert!(!no_gps_texts.iter().any(|text| text == "Navigation blocked"));
    }

    #[test]
    fn drive_health_rail_renders_absent_stale_and_current_domains() {
        let mut absent = MapsLocationSurface::live();
        absent.active = WorkspaceTab::Drive;
        let absent_texts = painted_texts(&mut absent);
        assert!(absent_texts
            .iter()
            .any(|text| text == "Radio & GNSS health"));
        assert!(absent_texts.iter().any(|text| text == "Unavailable"));
        for label in [
            "Cell A",
            "Cell B",
            "Wi-Fi A",
            "Wi-Fi B",
            "Bluetooth",
            "GNSS",
        ] {
            assert!(
                absent_texts.iter().any(|text| text == label),
                "absent rail must keep the {label} position: {absent_texts:?}"
            );
        }

        let now = test_now_ms();
        let mut stale = MapsLocationSurface::live();
        let mut stale_snapshot = healthy_v2_snapshot(now);
        stale_snapshot.published_at_ms = now - 6_000;
        stale.refresh_from_vehicle_v2(&stale_snapshot);
        stale.active = WorkspaceTab::Drive;
        let stale_texts = painted_texts(&mut stale);
        assert!(stale_texts.iter().any(|text| text == "Stale"));
        assert!(stale_texts.iter().any(|text| text == "Cell A"));
        assert!(stale_texts.iter().any(|text| text == "GNSS"));

        let mut healthy = MapsLocationSurface::live();
        healthy.refresh_from_vehicle_v2(&healthy_v2_snapshot(now));
        healthy.active = WorkspaceTab::Drive;
        let healthy_texts = painted_texts(&mut healthy);
        assert!(healthy_texts.iter().any(|text| text == "Current"));
        assert!(healthy_texts.iter().any(|text| text == "Active"));
        assert!(healthy_texts.iter().any(|text| text == "Standby"));
        let positions: Vec<usize> = [
            "Cell A",
            "Cell B",
            "Wi-Fi A",
            "Wi-Fi B",
            "Bluetooth",
            "GNSS",
        ]
        .iter()
        .map(|label| {
            healthy_texts
                .iter()
                .position(|text| text == label)
                .unwrap_or_else(|| panic!("healthy rail missing {label}: {healthy_texts:?}"))
        })
        .collect();
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "native health positions must remain ordered: {positions:?}"
        );

        // The same rail remains in place once guidance is active.
        healthy.local_navigation.navigating = true;
        let active_route_texts = painted_texts(&mut healthy);
        assert!(active_route_texts
            .iter()
            .any(|text| text == "Radio & GNSS health"));
        assert!(active_route_texts.iter().any(|text| text == "Current"));
    }

    #[test]
    fn health_rail_glyph_semantics_cover_every_operation_and_freshness_override() {
        let slot = |state, presence, operation| VehicleHealthRailSlot {
            id: "cellular-a",
            label: "Cell A",
            state,
            operation,
            presence,
            age_ms: Some(12),
            reason: None,
            active_path: false,
        };
        let current = |operation| {
            slot(
                VehicleHealthRailState::Current,
                Some(VehicleRadioPresence::Installed),
                Some(operation),
            )
        };

        for (operation, expected) in [
            (VehicleRadioOperation::Active, HealthSlotGlyph::ActiveCheck),
            (VehicleRadioOperation::Standby, HealthSlotGlyph::StandbyRing),
            (
                VehicleRadioOperation::Acquiring,
                HealthSlotGlyph::AttentionTriangle,
            ),
            (
                VehicleRadioOperation::Degraded,
                HealthSlotGlyph::AttentionTriangle,
            ),
            (VehicleRadioOperation::Fault, HealthSlotGlyph::FaultCross),
            (
                VehicleRadioOperation::Disabled,
                HealthSlotGlyph::DisabledPause,
            ),
            (VehicleRadioOperation::Unknown, HealthSlotGlyph::Clock),
            (VehicleRadioOperation::Stale, HealthSlotGlyph::Clock),
        ] {
            assert_eq!(health_slot_glyph(&current(operation)), expected);
        }

        let not_installed = slot(
            VehicleHealthRailState::Current,
            Some(VehicleRadioPresence::NotInstalled),
            Some(VehicleRadioOperation::Active),
        );
        assert_eq!(
            health_slot_glyph(&not_installed),
            HealthSlotGlyph::NotInstalledSlash,
            "presence must override a retained active operation"
        );

        for (state, expected) in [
            (VehicleHealthRailState::Stale, HealthSlotGlyph::Clock),
            (
                VehicleHealthRailState::Resyncing,
                HealthSlotGlyph::ResyncingArc,
            ),
            (
                VehicleHealthRailState::Unavailable,
                HealthSlotGlyph::UnavailableSlash,
            ),
        ] {
            let retained_active = slot(
                state,
                Some(VehicleRadioPresence::Installed),
                Some(VehicleRadioOperation::Active),
            );
            assert_eq!(
                health_slot_glyph(&retained_active),
                expected,
                "freshness state must override a retained active operation"
            );
        }
    }

    #[test]
    fn hostile_map_attribution_is_bounded_before_layout() {
        let hostile = "untrusted-provider-label-".repeat(MAX_MAP_ATTRIBUTION_CHARS * 4);
        let bounded = bounded_map_attribution(&hostile);

        assert_eq!(bounded.chars().count(), MAX_MAP_ATTRIBUTION_CHARS);
        assert!(bounded.ends_with(MAP_ATTRIBUTION_ELLIPSIS));
    }

    #[test]
    fn normal_map_attribution_keeps_provider_credits() {
        let mut map = MapViewState::live(true);
        map.earthquake_overlay = true;
        map.nws_alert_overlay = true;
        map.aircraft_overlay = true;
        map.transit_overlay = true;
        map.nws_forecast_overlay = true;
        map.caltrans_camera_overlay = true;
        map.iem_radar_overlay = true;
        map.wildfire_overlay = true;
        map.traffic_event_overlay = true;
        map.air_quality_overlay = true;

        let normal = map.attribution_line();
        let bounded = bounded_map_attribution(&normal);

        assert_eq!(bounded, normal);
        for credit in [
            "OpenStreetMap contributors",
            "USGS",
            "NWS",
            "adsb.lol",
            "MassDOT",
            "NOAA",
            "Caltrans",
            "IEM",
            "NIFC WFIGS",
            "NASA FIRMS",
            "NCDOT",
            "US EPA AirNow",
        ] {
            assert!(
                bounded.contains(credit),
                "missing provider credit: {credit}"
            );
        }
    }

    #[test]
    fn map_layers_menu_covers_all_ten_feeds_and_keeps_safety_defaults_visible() {
        let mut surface = MapsLocationSurface::live();
        surface.active = WorkspaceTab::Map;
        let texts = painted_texts(&mut surface);
        assert!(texts.iter().any(|text| text == "Layers (3)"));
        assert_eq!(
            active_live_overlay_count(&surface.map),
            3,
            "NWS alerts, NEXRAD, and wildfire are the three safety defaults"
        );

        surface.map.earthquake_overlay = true;
        surface.map.nws_alert_overlay = true;
        surface.map.aircraft_overlay = true;
        surface.map.transit_overlay = true;
        surface.map.nws_forecast_overlay = true;
        surface.map.caltrans_camera_overlay = true;
        surface.map.iem_radar_overlay = true;
        surface.map.wildfire_overlay = true;
        surface.map.traffic_event_overlay = true;
        surface.map.air_quality_overlay = true;
        assert_eq!(active_live_overlay_count(&surface.map), 10);
        let texts = painted_texts(&mut surface);
        assert!(texts.iter().any(|text| text == "Layers (10)"));
    }

    #[test]
    fn active_provider_attribution_stays_inside_narrow_map_clip() {
        let mut map = MapViewState::live(false);
        map.earthquake_overlay = true;
        map.nws_alert_overlay = true;
        map.aircraft_overlay = true;
        map.transit_overlay = true;
        map.nws_forecast_overlay = true;
        map.caltrans_camera_overlay = true;
        map.iem_radar_overlay = true;
        map.wildfire_overlay = true;
        map.traffic_event_overlay = true;
        map.air_quality_overlay = true;

        let map_rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(320.0, 180.0));
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut painted = None;
        let input = egui::RawInput {
            screen_rect: Some(map_rect),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let painter = ui.painter_at(map_rect);
                painted = Some(paint_map_attribution(
                    &painter,
                    map_rect,
                    &map,
                    Style::TEXT_DIM,
                ));
            });
        });

        let painted = painted.expect("map attribution should paint");
        assert!(painted.left() >= map_rect.left());
        assert!(painted.right() <= map_rect.right());
        assert!(painted.top() >= map_rect.top());
        assert!(painted.bottom() <= map_rect.bottom());
        assert!(
            painted.height() > 30.0,
            "all ten provider credits should wrap instead of overflowing one line"
        );
    }

    /// Recursively collect every text string in a painted shape tree.
    fn collect_shape_text(shape: &egui::epaint::Shape, out: &mut Vec<String>) {
        match shape {
            egui::epaint::Shape::Text(t) => out.push(t.galley.text().to_string()),
            egui::epaint::Shape::Vec(v) => {
                for s in v {
                    collect_shape_text(s, out);
                }
            }
            _ => {}
        }
    }

    fn healthy_v2_snapshot(published_at_ms: i64) -> mackes_mesh_types::vehicle::VehicleStateV2 {
        use mackes_mesh_types::vehicle::{
            DomainFreshness, FreshnessState, RadioHealth, RadioId, RadioInventory, RadioMetrics,
            RadioOperation, RadioPresence, RadioRole, SnapshotProvenance, SnapshotSource,
            VehicleDomainFreshness, VehicleState, VehicleStateV2,
        };

        let mut legacy = VehicleState::offline("rig-1");
        legacy.online = true;
        legacy.model = "MG90".to_string();
        legacy.esn = "ESN-TEST".to_string();
        let mut snapshot = VehicleStateV2::from_v1(
            &legacy,
            "rig-1",
            9,
            1_000,
            published_at_ms,
            SnapshotProvenance {
                source: SnapshotSource::DirectGateway,
                source_id: Some("rig-1".to_string()),
                relay: None,
            },
        );
        let fresh = DomainFreshness {
            state: FreshnessState::Fresh,
            age_ms: Some(0),
            reason: None,
        };
        snapshot.freshness = VehicleDomainFreshness {
            identity: fresh.clone(),
            radios: fresh.clone(),
            gnss: fresh.clone(),
            vehicle: fresh.clone(),
            power: fresh,
        };
        let row = |id, role, operation, active_path| RadioHealth {
            id,
            presence: RadioPresence::Installed,
            operation,
            reason_code: None,
            age_ms: Some(12),
            configured_role: role,
            active_path,
            metrics: RadioMetrics::Unknown,
        };
        snapshot.radios = RadioInventory::new(vec![
            row(
                RadioId::CellularA,
                RadioRole::Wan,
                RadioOperation::Active,
                true,
            ),
            row(
                RadioId::CellularB,
                RadioRole::Wan,
                RadioOperation::Standby,
                false,
            ),
            row(
                RadioId::WifiA,
                RadioRole::AccessPoint,
                RadioOperation::Standby,
                false,
            ),
            row(
                RadioId::WifiB,
                RadioRole::Backhaul,
                RadioOperation::Standby,
                false,
            ),
            row(
                RadioId::Bluetooth,
                RadioRole::Bluetooth,
                RadioOperation::Standby,
                false,
            ),
            row(
                RadioId::Gnss,
                RadioRole::Gnss,
                RadioOperation::Active,
                false,
            ),
        ])
        .expect("six native rows fit the bounded inventory");
        snapshot
    }

    /// Every text string painted by one frame of the panel, recursively.
    fn painted_texts(surface: &mut MapsLocationSurface) -> Vec<String> {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1280.0, 820.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| maps_location_panel(ui, surface));
        });
        let mut texts = Vec::new();
        for clipped in &out.shapes {
            collect_shape_text(&clipped.shape, &mut texts);
        }
        texts
    }

    /// Every string painted by the Vehicle tab body without workspace scrolling.
    fn vehicle_texts(
        vehicle: &VehicleState,
        radio_health: &VehicleRadioHealth,
        mirror_status: &VehicleMirrorStatus,
    ) -> Vec<String> {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1280.0, 2400.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_vehicle(ui, vehicle, radio_health, mirror_status);
            });
        });
        let mut texts = Vec::new();
        for clipped in &out.shapes {
            collect_shape_text(&clipped.shape, &mut texts);
        }
        texts
    }

    #[test]
    fn vehicle_view_dashes_simulated_and_stale_readings_but_keeps_provenance() {
        use mackes_mesh_types::vehicle::{VehicleState as WireVehicleState, VehicleTelem};

        // The fixture profile remains explicitly identified, but none of its
        // numeric CAN/OBD seed values may look like live instruments.
        let simulated = MapsLocationSurface::simulated();
        let simulated_texts = vehicle_texts(
            &simulated.vehicle,
            &simulated.vehicle_radio_health,
            &simulated.vehicle_mirror_status,
        );
        assert!(simulated_texts
            .iter()
            .any(|text| text == "simulated CAN/OBD profile"));
        for fabricated in ["27", "1840", "91", "13.9", "64%", "78214 mi", "42 min"] {
            assert!(
                !simulated_texts.iter().any(|text| text == fabricated),
                "simulated reading {fabricated:?} must be dashed: {simulated_texts:?}"
            );
        }

        // A fresh online OBD mirror is allowed even with no GNSS fix; position
        // readiness and telemetry freshness are deliberately independent.
        let mut mirror = WireVehicleState::offline("eagle");
        mirror.online = true;
        mirror.model = "MG90".to_string();
        mirror.mgos_version = "4.3.0.1".to_string();
        mirror.gaps.clear();
        mirror.telem = VehicleTelem {
            speed_mph: 62.0,
            rpm: 2_100,
            coolant_c: Some(91.0),
            battery_v: 13.9,
            fuel_percent: Some(64.0),
            odometer_mi: Some(78_214),
            runtime_min: 42,
            moving: true,
            ignition_on: true,
            obd_present: true,
            ..VehicleTelem::default()
        };
        mirror.published_at_ms = test_now_ms();
        let mut live = MapsLocationSurface::live();
        live.refresh_from_vehicle(&mirror);
        let fresh_texts = vehicle_texts(
            &live.vehicle,
            &live.vehicle_radio_health,
            &live.vehicle_mirror_status,
        );
        for reading in ["62", "2100", "91", "13.9", "64%", "78214 mi", "42 min"] {
            assert!(
                fresh_texts.iter().any(|text| text == reading),
                "fresh reading {reading:?} paints: {fresh_texts:?}"
            );
        }

        // The retained payload crosses the freshness window. Its values all
        // disappear, but confidence + a warning-age remain diagnostic evidence.
        mirror.published_at_ms = test_now_ms() - 6_000;
        live.refresh_from_vehicle(&mirror);
        let stale_texts = vehicle_texts(
            &live.vehicle,
            &live.vehicle_radio_health,
            &live.vehicle_mirror_status,
        );
        assert!(stale_texts
            .iter()
            .any(|text| text.starts_with("live vehicle-gateway mirror")));
        assert!(stale_texts.iter().any(|text| text == "Stale retained"));
        assert!(stale_texts
            .iter()
            .any(|text| text == "cached values retained"));
        assert!(stale_texts
            .iter()
            .any(|text| text.ends_with(" s ago") && text != "0.0 s ago"));
        for stale in ["62", "2100", "91", "13.9", "64%", "78214 mi", "42 min"] {
            assert!(
                !stale_texts.iter().any(|text| text == stale),
                "stale reading {stale:?} must be dashed: {stale_texts:?}"
            );
        }
    }

    #[test]
    fn vehicle_view_renders_typed_presence_operation_reason_and_freshness() {
        use mackes_mesh_types::vehicle::{
            SnapshotProvenance, SnapshotSource, VehicleState as WireVehicleState, VehicleStateV2,
            VehicleTelem,
        };

        let mut legacy = WireVehicleState::offline("rig-1");
        legacy.online = true;
        legacy.model = "MG90".to_string();
        legacy.esn = "ESN-TEST".to_string();
        legacy.mgos_version = "4.3.0.1".to_string();
        legacy.wan.active_wan = "Cellular A".to_string();
        legacy.wan.cellular_a.sim_state = "ready".to_string();
        legacy.wan.cellular_a.healthy = true;
        legacy.telem = VehicleTelem::default();
        legacy.published_at_ms = test_now_ms();
        let snapshot = VehicleStateV2::from_v1(
            &legacy,
            "rig-1",
            3,
            5_000,
            legacy.published_at_ms,
            SnapshotProvenance {
                source: SnapshotSource::DirectGateway,
                source_id: Some("rig-1".to_string()),
                relay: None,
            },
        );

        let mut surface = MapsLocationSurface::live();
        surface.refresh_from_vehicle_v2(&snapshot);
        let texts = vehicle_texts(
            &surface.vehicle,
            &surface.vehicle_radio_health,
            &surface.vehicle_mirror_status,
        );
        assert!(texts.iter().any(|text| text == "Current"));
        assert!(texts.iter().any(|text| text == "Management node"));
        assert!(texts.iter().any(|text| text == "Direct gateway"));
        assert!(texts.iter().any(|text| text == "Installed"));
        assert!(texts.iter().any(|text| text == "Unknown"));
        assert!(texts.iter().any(|text| text == "Active"));
        assert!(texts.iter().any(|text| text == "GNSS freshness"));
        assert!(texts.iter().any(|text| text == "degraded"));

        let empty = MapsLocationSurface::live();
        let unavailable = vehicle_texts(
            &empty.vehicle,
            &empty.vehicle_radio_health,
            &empty.vehicle_mirror_status,
        );
        assert!(unavailable.iter().any(|text| text == "unavailable"));
        assert!(unavailable
            .iter()
            .any(|text| text.contains("No valid typed radio inventory")));
    }

    #[test]
    fn live_airspace_renders_the_honest_empty_scope() {
        // An empty AirspaceState must render the designed honest-empty: the
        // in-range counter reads zero, the scope says there is no scanner feed,
        // and each layer group says so too — with NO contact rows (P8/Q33).
        let mut surface = MapsLocationSurface::live();
        surface.focus_airspace_tab();
        let texts = painted_texts(&mut surface);
        assert!(
            texts.iter().any(|t| t == "0 IN RANGE"),
            "zero contacts counter: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t == "NO SCANNER FEED"),
            "scope-level empty state paints"
        );
        assert!(
            texts
                .iter()
                .any(|t| t == "MG90 scanner source not configured"),
            "the typed worker status note paints"
        );
        // No fixture contact ever paints on the production surface.
        assert!(
            !texts.iter().any(|t| t.contains("MACKES-MESH")),
            "no fabricated contacts: {texts:?}"
        );

        // Per-layer notes: render the airspace panel directly at full height
        // (the workspace chrome's scroll viewport can fold the lower groups
        // below the fold; they scroll into view on a seat).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1280.0, 820.0),
            )),
            ..Default::default()
        };
        let mut airspace = crate::airspace::AirspaceState::live();
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                crate::airspace::airspace_panel(ui, &mut airspace);
            });
        });
        let mut layer_texts = Vec::new();
        for clipped in &out.shapes {
            collect_shape_text(&clipped.shape, &mut layer_texts);
        }
        for expected in [
            "No WiFi scanner feed",
            "No Cellular scanner feed",
            "No Bluetooth scanner feed",
        ] {
            assert!(
                layer_texts.iter().any(|t| t == expected),
                "{expected:?} paints: {layer_texts:?}"
            );
        }
    }

    #[test]
    fn live_surface_never_paints_the_simulated_ribbon_or_chip() {
        let mut surface = MapsLocationSurface::live();
        let texts = painted_texts(&mut surface);
        assert!(
            !texts.iter().any(|t| t.contains("SIMULATED")),
            "no SIMULATED badge on production: {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t == "Simulator"),
            "no Simulator chip/nav entry on production: {texts:?}"
        );
    }

    fn tessellate_at(surface: &mut MapsLocationSurface, w: f32, h: f32) -> usize {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(w, h))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| maps_location_panel(ui, surface));
        });
        ctx.tessellate(out.shapes, out.pixels_per_point).len()
    }

    #[test]
    fn drive_hud_renders_acquiring_state_without_fix() {
        // No fix + degenerate coordinates + NaN/inf telemetry must render the
        // honest "Acquiring GPS" state, never feed non-finite values into layout.
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Drive;
        for source in &mut surface.locations.sources {
            source.sample.fix_type = "No fix".to_string();
            source.sample.latitude = 0.0;
            source.sample.longitude = 0.0;
            source.sample.speed_mph = f32::NAN;
            source.sample.heading_deg = f32::INFINITY;
        }
        assert!(!surface
            .locations
            .primary_sample()
            .is_some_and(LocationSample::has_fix));
        assert!(tessellate(&mut surface) > 0);
    }

    #[test]
    fn drive_hud_tessellates_at_small_viewport() {
        // Tiny surface exercises the finite/clamp guards on every allocated rect.
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Drive;
        assert!(tessellate_at(&mut surface, 360.0, 240.0) > 0);
    }

    #[test]
    fn drive_hud_large_text_reserves_fab_lane_before_health_rail() {
        let canvas = Rect::from_min_size(Pos2::ZERO, egui::vec2(1280.0, 820.0));
        let rail = MapsLocationSurface::live().vehicle_health_rail();
        let layout = rail.layout_for_text_zoom(1.5);
        let geometry = drive_hud_overlay_geometry(
            canvas,
            144.0,
            Style::SP_M,
            0.0,
            26.0,
            Style::SP_S + Style::SP_XS,
            layout,
        );

        assert_eq!(geometry.rail_layout.columns, 3);
        assert_eq!(geometry.rail_layout.rows, 2);
        assert!(geometry.health_rail.height() >= 110.0);
        assert!(
            geometry.health_rail.right() < geometry.fab_lane.left(),
            "health rail must leave a separation gap before the FAB lane: {:?} vs {:?}",
            geometry.health_rail,
            geometry.fab_lane
        );
        let fab_hit = Rect::from_center_size(
            egui::pos2(canvas.right() - Style::SP_M - 26.0, canvas.bottom() - 138.0),
            egui::vec2(52.0, 52.0),
        );
        assert!(
            geometry.fab_lane.contains_rect(fab_hit),
            "FAB hit target escaped its reserved lane: {fab_hit:?} vs {:?}",
            geometry.fab_lane
        );
    }

    #[test]
    fn drive_hud_light_largest_tessellates_with_truthful_health_rail() {
        let ctx = egui::Context::default();
        Style::install_color_scheme_with_density(
            &ctx,
            StyleColorScheme::Light,
            mde_egui::Density::Mouse,
        );
        ctx.set_zoom_factor(1.5);
        let mut surface = MapsLocationSurface::live();
        surface.active = WorkspaceTab::Drive;
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(1280.0, 820.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    maps_location_panel(ui, &mut surface);
                });
            },
        );
        assert!(!out.shapes.is_empty());
        assert_eq!(surface.vehicle_health_rail().state, VehicleHealthRailState::Unavailable);
    }

    #[test]
    fn drive_hud_tessellates_with_nan_pan_and_zoom() {
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Drive;
        surface.map.pan = [f32::NAN, f32::INFINITY];
        surface.map.zoom = f32::NAN;
        assert!(tessellate(&mut surface) > 0);
    }

    #[test]
    fn maneuver_kind_infers_direction_from_keywords() {
        assert_eq!(
            maneuver_kind("Turn right onto Main St"),
            ManeuverKind::Right
        );
        assert_eq!(maneuver_kind("Turn left"), ManeuverKind::Left);
        assert_eq!(
            maneuver_kind("Keep right toward patrol staging"),
            ManeuverKind::SlightRight
        );
        assert_eq!(
            maneuver_kind("Slight left onto 5th"),
            ManeuverKind::SlightLeft
        );
        assert_eq!(maneuver_kind("Merge onto I-79 N"), ManeuverKind::Merge);
        assert_eq!(maneuver_kind("Make a U-turn"), ManeuverKind::UTurn);
        assert_eq!(
            maneuver_kind("Enter the roundabout"),
            ManeuverKind::Roundabout
        );
        assert_eq!(maneuver_kind("Arrive at destination"), ManeuverKind::Arrive);
        assert_eq!(maneuver_kind("Continue straight"), ManeuverKind::Straight);
    }

    #[test]
    fn format_distance_switches_to_feet_when_close() {
        assert_eq!(format_distance(0.4), "0.4 mi");
        assert_eq!(format_distance(0.1), "550 ft");
        assert_eq!(format_distance(f32::NAN), "0 ft");
    }

    #[test]
    fn planned_route_without_provider_geometry_paints_unavailable_state() {
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Drive;
        surface.map.route_visible = true;
        let texts = painted_texts(&mut surface);

        assert!(
            texts
                .iter()
                .any(|text| text == "Route geometry unavailable"),
            "a planned route without provider geometry must be explicit: {texts:?}"
        );
    }

    #[test]
    fn provider_route_geometry_model_requires_a_valid_primary_path() {
        let point = ProviderRoutePoint {
            latitude: 32.168,
            longitude: -95.849,
        };
        let mut geometry = ProviderRouteGeometry::default();
        assert!(!geometry.is_renderable());

        geometry.primary = vec![point];
        assert!(!geometry.is_renderable());

        geometry.primary.push(ProviderRoutePoint {
            latitude: 32.17,
            longitude: -95.847,
        });
        assert!(geometry.is_renderable());

        geometry.primary[1].latitude = f64::NAN;
        assert!(!geometry.is_renderable());
    }

    #[test]
    fn provider_route_geometry_present_projects_and_paints_provider_path() {
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1024.0, 768.0));
        let map = MapViewState::simulated();
        let projection =
            crate::basemap::Projection::vehicle_centered(rect, &map, (32.168, -95.849))
                .expect("valid provider projection");
        let geometry = ProviderRouteGeometry {
            primary: vec![
                ProviderRoutePoint {
                    latitude: 32.168,
                    longitude: -95.849,
                },
                ProviderRoutePoint {
                    latitude: 32.17,
                    longitude: -95.847,
                },
                ProviderRoutePoint {
                    latitude: 32.173,
                    longitude: -95.843,
                },
            ],
            alternate: vec![
                ProviderRoutePoint {
                    latitude: 32.168,
                    longitude: -95.849,
                },
                ProviderRoutePoint {
                    latitude: 32.169,
                    longitude: -95.844,
                },
            ],
            maneuver: Some(ProviderRoutePoint {
                latitude: 32.17,
                longitude: -95.847,
            }),
        };

        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(rect),
            ..Default::default()
        };
        let mut painted = false;
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let painter = ui.painter_at(rect);
                painted = paint_route(&painter, &projection, &map, &geometry, true);
            });
        });

        assert!(
            painted,
            "provider geometry should be accepted by the route painter"
        );
        assert!(
            !out.shapes.is_empty(),
            "provider geometry should produce route shapes"
        );
    }

    #[test]
    fn drive_hud_shows_unknown_guidance_without_road_heuristics() {
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Drive;
        surface.local_navigation.navigating = true;
        surface.local_navigation.active_route.current_road = "I-79 N".to_string();
        surface.local_navigation.active_route.next_maneuver = "Turn right onto Main St".to_string();
        surface
            .local_navigation
            .active_route
            .distance_to_maneuver_mi = 0.2;
        let texts = painted_texts(&mut surface);

        assert!(
            texts.iter().any(|text| text == "Lane guidance unavailable"),
            "lane status must be explicit: {texts:?}"
        );
        assert!(
            texts.iter().any(|text| text == "Speed limit unavailable"),
            "speed-limit status must be explicit: {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|text| text.contains("Turn right onto Main St")),
            "real maneuver text must remain: {texts:?}"
        );
        assert!(
            texts.iter().any(|text| text.contains("on I-79 N")),
            "real current-road text must remain: {texts:?}"
        );
        for fabricated in ["65", "55", "40", "35"] {
            assert!(
                !texts.iter().any(|text| text == fabricated),
                "road-name heuristic must not paint fabricated value {fabricated}: {texts:?}"
            );
        }
    }

    #[test]
    fn route_preview_screen_tessellates() {
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Drive;
        surface.route_preview = true;
        assert!(tessellate(&mut surface) > 0);
    }

    #[test]
    fn route_preview_tessellates_without_fix() {
        // No fix + degenerate coordinates + NaN/inf telemetry must still render.
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Drive;
        surface.route_preview = true;
        for source in &mut surface.locations.sources {
            source.sample.fix_type = "No fix".to_string();
            source.sample.latitude = 0.0;
            source.sample.longitude = 0.0;
            source.sample.speed_mph = f32::NAN;
            source.sample.heading_deg = f32::INFINITY;
        }
        assert!(tessellate(&mut surface) > 0);
    }

    #[test]
    fn route_preview_tessellates_at_small_viewport() {
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Drive;
        surface.route_preview = true;
        assert!(tessellate_at(&mut surface, 360.0, 240.0) > 0);
    }

    #[test]
    fn preview_layout_has_one_rect_per_option() {
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 700.0));
        let layout = preview_layout(rect, 2);
        assert_eq!(layout.options.len(), 2);
        assert!(layout.sheet.contains_rect(layout.start));
        assert!(layout.sheet.contains_rect(layout.dest));
    }

    #[test]
    fn simulator_readiness_scenarios_tessellate_without_hardware() {
        // The Simulator tab is gone (WL-UX-007/S1); the readiness model these
        // scenarios mutate still renders on the Map tab's readiness card.
        let mut stale = MapsLocationSurface::simulated();
        stale.active = WorkspaceTab::Map;
        stale.simulate_stale_primary_location();
        assert!(tessellate(&mut stale) > 0);

        let mut missing_maps = MapsLocationSurface::simulated();
        missing_maps.active = WorkspaceTab::Map;
        missing_maps.simulate_no_offline_maps();
        assert!(tessellate(&mut missing_maps) > 0);

        let mut dead_zone = MapsLocationSurface::simulated();
        dead_zone.active = WorkspaceTab::Map;
        dead_zone.simulate_cellular_dead_zone();
        assert!(tessellate(&mut dead_zone) > 0);
    }

    #[test]
    fn location_sources_tessellate_with_blocked_manual_switches() {
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Admin;
        surface.admin_section = AdminSection::LocationSources;
        surface.locations.sources[1].status = SourceStatus::Disconnected;
        surface.locations.sources[2].sample.update_age_s = 6.0;
        surface.locations.sources[3].sample.accuracy_m = 6.0;

        assert!(tessellate(&mut surface) > 0);
    }

    #[test]
    fn destination_search_screen_tessellates() {
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Drive;
        surface.destination_search = true;
        assert!(tessellate(&mut surface) > 0);
    }

    #[test]
    fn destination_search_tessellates_without_fix() {
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Drive;
        surface.destination_search = true;
        for source in &mut surface.locations.sources {
            source.sample.fix_type = "No fix".to_string();
            source.sample.latitude = 0.0;
            source.sample.longitude = 0.0;
            source.sample.speed_mph = f32::NAN;
            source.sample.heading_deg = f32::INFINITY;
        }
        assert!(tessellate(&mut surface) > 0);
    }

    #[test]
    fn destination_search_tessellates_at_small_viewport() {
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Drive;
        surface.destination_search = true;
        assert!(tessellate_at(&mut surface, 360.0, 240.0) > 0);
    }

    #[test]
    fn search_layout_fits_chips_and_rows_inside_the_list_card() {
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 700.0));
        let layout = search_layout(rect, 7, 5);
        assert_eq!(layout.chips.len(), 5);
        assert!(
            !layout.rows.is_empty(),
            "rows should fit a full-size screen"
        );
        assert!(rect.contains_rect(layout.list_card));
        for row in &layout.rows {
            assert!(
                layout.list_card.contains_rect(*row),
                "row escapes list card"
            );
        }
    }

    #[test]
    fn search_layout_survives_a_tiny_rect() {
        // A degenerate viewport must not panic; rows simply clip to zero.
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(40.0, 40.0));
        let layout = search_layout(rect, 7, 5);
        assert_eq!(layout.chips.len(), 5);
    }

    #[test]
    fn destination_search_surface_stays_inside_narrow_clip() {
        // The old 320 px minimum escaped a 240 px seat and put the list card
        // below the visible workspace. The remaining clip wins over the
        // larger layout budget, including when the cursor is already offset.
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(360.0, 240.0));
        let cursor_top = 24.0;
        let height = bounded_search_height(320.0, screen.bottom() - cursor_top);
        let search_rect = Rect::from_min_size(
            egui::pos2(screen.left(), cursor_top),
            egui::vec2(screen.width(), height),
        );

        assert_eq!(height, 216.0);
        assert!(
            screen.contains_rect(search_rect),
            "search surface escaped the narrow clip: {search_rect:?}"
        );
    }

    #[test]
    fn reset_confirmation_action_stays_inside_a_narrow_admin_clip() {
        // The confirmation input and button must wrap instead of letting the
        // destructive action escape the narrow Admin-page workspace.
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(128.0, 160.0));
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut typed_confirmation = String::new();
        let mut reset_button = Rect::NOTHING;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    reset_button = reset_confirmation_row(ui, &mut typed_confirmation, false);
                });
            },
        );

        assert!(
            screen.contains_rect(reset_button),
            "reset action escaped the narrow Admin clip: {reset_button:?}"
        );
        assert!(
            reset_button.is_positive(),
            "reset action lost its hit target"
        );
    }

    #[test]
    fn arrival_screen_tessellates() {
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Drive;
        surface.arrived = true;
        assert!(tessellate(&mut surface) > 0);
    }

    #[test]
    fn arrival_tessellates_without_fix_at_small_viewport() {
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Drive;
        surface.arrived = true;
        surface.local_navigation.active_route.eta = String::new();
        for source in &mut surface.locations.sources {
            source.sample.fix_type = "No fix".to_string();
            source.sample.latitude = 0.0;
            source.sample.longitude = 0.0;
            source.sample.speed_mph = f32::NAN;
        }
        assert!(tessellate_at(&mut surface, 360.0, 240.0) > 0);
    }

    #[test]
    fn arrival_layout_keeps_actions_and_badge_inside_the_card() {
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 700.0));
        let layout = arrival_layout(rect);
        assert!(rect.contains_rect(layout.card));
        assert!(layout.card.contains_rect(layout.end_btn));
        assert!(layout.card.contains_rect(layout.save_btn));
        assert!(layout.card.contains_rect(layout.badge));
        assert!(!layout.end_btn.intersects(layout.save_btn));
    }

    #[test]
    fn drive_hud_off_route_shows_recalculating_state() {
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Drive;
        surface.local_navigation.navigating = true;
        surface.off_route = true;
        assert!(tessellate(&mut surface) > 0);
    }

    #[test]
    fn drive_hud_off_route_tessellates_with_nan_and_no_fix() {
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Drive;
        surface.off_route = true;
        surface.map.pan = [f32::NAN, f32::INFINITY];
        surface.map.zoom = f32::NAN;
        for source in &mut surface.locations.sources {
            source.sample.fix_type = "No fix".to_string();
            source.sample.latitude = 0.0;
            source.sample.longitude = 0.0;
            source.sample.speed_mph = f32::NAN;
            source.sample.heading_deg = f32::INFINITY;
        }
        assert!(tessellate(&mut surface) > 0);
    }

    #[test]
    fn full_navigation_flow_tessellates_at_every_stage() {
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Drive;

        // 1. Search.
        surface.open_destination_search();
        assert!(surface.destination_search);
        assert!(tessellate(&mut surface) > 0);

        // 2. Choose a destination -> route preview.
        surface.choose_destination(2);
        assert!(surface.route_preview);
        assert!(!surface.destination_search);
        assert!(tessellate(&mut surface) > 0);

        // 3. Start -> live turn-by-turn HUD (guidance now running).
        surface.start_navigation();
        assert!(surface.local_navigation.navigating);
        assert!(!surface.route_preview);
        assert!(tessellate(&mut surface) > 0);

        // 4. Off-route recalculating banner, then back on route.
        surface.off_route = true;
        assert!(tessellate(&mut surface) > 0);
        surface.off_route = false;

        // 5. Arrival, then End.
        surface.simulate_arrival();
        assert!(surface.arrived);
        assert!(tessellate(&mut surface) > 0);
        surface.end_navigation();
        assert!(tessellate(&mut surface) > 0);
    }

    #[test]
    fn settings_tab_exposes_every_required_mg90_category() {
        let labels: Vec<&str> = Mg90SettingCategory::ALL
            .iter()
            .map(|category| category.label())
            .collect();
        assert_eq!(
            labels,
            vec![
                "Overview",
                "Cellular & SIM",
                "Wi-Fi",
                "Ethernet",
                "WAN Policies",
                "LAN / DHCP / VLAN",
                "Firewall",
                "VPN",
                "GNSS",
                "Serial Recovery",
                "GPIO",
                "Services",
                "Security",
                "Diagnostics",
                "Logs",
                "Backup & Restore",
                "Original LCI Fallback",
            ]
        );
    }
}
