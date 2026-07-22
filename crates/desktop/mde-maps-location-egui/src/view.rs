//! Native egui renderer for the Maps & Location workspace.

use mde_egui::egui::{
    self, Align, Align2, Color32, FontId, Mesh, Painter, Pos2, Rect, RichText, Sense, Shape,
    Stroke, StrokeKind, Vec2,
};
use mde_egui::{paint_carbon, Style, StyleColorScheme};

use crate::model::{
    BackupRecord, CheckState, DeadZoneSeverity, DeadZoneState, Destination, DeviceIoState,
    EncryptedVaultState, FirmwareWorkflow, LocationManager, LocationSample, LocationSource,
    MapViewState, Mg90ManagementMethod, Mg90SettingCategory, Mg90SettingDescriptor, Mg90State,
    OfflineMapManagerState, OfflineNavigationReadiness, OfflineNavigationStatus, ProviderContract,
    RouteOption, RoutePlan, RouteTraffic, SettingValueType, SetupStep, SourceStatus,
    TripRecorderState, VehicleState, WorkspaceTab,
};
use crate::MapsLocationSurface;

const RAIL_W: f32 = 176.0;
const HEADER_H: f32 = mde_egui::menubar::BAR_HEIGHT + Style::SP_S;
const CARD_MIN_H: f32 = 84.0;
const MAP_DARK_BG: Color32 = Color32::from_rgb(0x0D, 0x13, 0x18); // style-leak-ok: map-content-color
const MAP_LIGHT_BG: Color32 = Color32::from_rgb(0xE8, 0xEF, 0xE8); // style-leak-ok: map-content-color
const ROAD_DARK: Color32 = Color32::from_rgb(0x42, 0x50, 0x57); // style-leak-ok: map-content-color
const ROAD_LIGHT: Color32 = Color32::from_rgb(0xB8, 0xC3, 0xB6); // style-leak-ok: map-content-color
const ROUTE_BLUE: Color32 = Color32::from_rgb(0x4C, 0xA3, 0xFF); // style-leak-ok: map-content-color
const ROUTE_ALT: Color32 = Color32::from_rgb(0x7D, 0xD9, 0xA3); // style-leak-ok: map-content-color
const WEATHER: Color32 = Color32::from_rgb(0x67, 0xD6, 0xE8); // style-leak-ok: map-content-color
const TRAFFIC: Color32 = Color32::from_rgb(0xFF, 0xB4, 0x54); // style-leak-ok: map-content-color
                                                              // --- Driving HUD (Google Maps / Waze vocabulary, keyed to the Quasar-dark route palette) ---
                                                              // A premium GMaps-navigation blue, painted as a top-lit vertical gradient
                                                              // (HI at the top edge → BASE → DEEP at the bottom) so the banner reads with
                                                              // depth instead of a single flat fill.
const MANEUVER_BLUE: Color32 = Color32::from_rgb(0x1A, 0x66, 0xE0); // style-leak-ok: map-content-color
const MANEUVER_BLUE_HI: Color32 = Color32::from_rgb(0x3E, 0x86, 0xFF); // style-leak-ok: map-content-color
const MANEUVER_BLUE_DEEP: Color32 = Color32::from_rgb(0x11, 0x4C, 0xB6); // style-leak-ok: map-content-color
const LANE_BG: Color32 = Color32::from_rgb(0x0E, 0x2A, 0x54); // style-leak-ok: map-content-color
const LANE_DIM: Color32 = Color32::from_rgb(0x6A, 0x7E, 0xA2); // style-leak-ok: map-content-color
const ROUTE_CASING: Color32 = Color32::from_rgb(0x14, 0x4C, 0x92); // style-leak-ok: map-content-color
const SIGN_WHITE: Color32 = Color32::from_rgb(0xF4, 0xF6, 0xFA); // style-leak-ok: map-content-color
const SIGN_RED: Color32 = Color32::from_rgb(0xD4, 0x2A, 0x2A); // style-leak-ok: map-content-color
const SIGN_INK: Color32 = Color32::from_rgb(0x15, 0x17, 0x1D); // style-leak-ok: map-content-color
const HUD_CARD_BG: Color32 = Color32::from_rgb(0x1A, 0x1B, 0x22); // style-leak-ok: map-content-color
const HUD_CARD_HI: Color32 = Color32::from_rgb(0x24, 0x26, 0x30); // style-leak-ok: map-content-color
const ROAD_CASING_DARK: Color32 = Color32::from_rgb(0x24, 0x2C, 0x33); // style-leak-ok: map-content-color
const ROAD_CASING_LIGHT: Color32 = Color32::from_rgb(0x9C, 0xA8, 0x9C); // style-leak-ok: map-content-color

/// Corner radius for the floating HUD cards (banner, ETA sheet, lane strip) —
/// larger than the shared card radius so the nav surface reads modern/premium.
const HUD_RADIUS: f32 = 16.0;
/// Corner radius for smaller HUD chips (speed sign chips, option cards).
const HUD_RADIUS_S: f32 = 12.0;

/// Render the complete native Maps & Location workspace.
pub fn maps_location_panel(ui: &mut egui::Ui, state: &mut MapsLocationSurface) {
    ui.visuals_mut().override_text_color = Some(Style::TEXT);

    // Auto Mode (Car): the cockpit is on a dash — drop the header + tab rail so the
    // active tab (the Drive HUD by default) is edge-to-edge full-bleed. Tab
    // switching in Car Mode is driven by the Auto Home tiles / bound keys (Nav →
    // Drive, Vehicle → telematics), not the rail.
    if Style::color_scheme(ui.ctx()) == StyleColorScheme::AutoSync3 {
        egui::Frame::NONE.fill(Style::BG).show(ui, |ui| {
            let content_size = ui.available_size();
            ui.allocate_ui_with_layout(
                content_size,
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt(("maps-location-car", state.active))
                        .auto_shrink([false, false])
                        .show(ui, |ui| render_active_tab(ui, state));
                },
            );
        });
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
                                .id_salt(("maps-location-tab", state.active))
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
    match state.active {
        WorkspaceTab::Drive => show_drive(ui, state),
        WorkspaceTab::Airspace => crate::airspace::airspace_panel(ui, &mut state.airspace),
        WorkspaceTab::Map => show_map(ui, state),
        WorkspaceTab::RoutesTrips => show_routes_trips(ui, state),
        WorkspaceTab::Vehicle => show_vehicle(ui, &state.vehicle),
        WorkspaceTab::Connectivity => show_connectivity(ui, &state.mg90),
        WorkspaceTab::DevicesIo => show_devices_io(ui, &mut state.devices),
        WorkspaceTab::LocationSources => show_location_sources(ui, &mut state.locations),
        WorkspaceTab::Mg90Setup => show_mg90_setup(ui, &mut state.mg90, &state.offline_maps),
        WorkspaceTab::Mg90Settings => show_mg90_settings(ui, state),
        WorkspaceTab::FirmwareRecovery => {
            show_firmware_recovery(ui, &state.firmware, &state.devices)
        }
        WorkspaceTab::Simulator => show_simulator(ui, state),
    }
}

fn header(ui: &mut egui::Ui, state: &MapsLocationSurface) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), HEADER_H), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, Style::RADIUS, Style::LAYER_01);
    painter.rect_stroke(
        rect,
        Style::RADIUS,
        Stroke::new(1.0, Style::BORDER),
        egui::StrokeKind::Inside,
    );

    let title_pos = egui::pos2(rect.left() + Style::SP_M, rect.center().y - Style::SP_XS);
    painter.text(
        title_pos,
        Align2::LEFT_CENTER,
        "Maps & Location",
        FontId::proportional(Style::TITLE),
        Style::TEXT_STRONG,
    );
    painter.text(
        title_pos + egui::vec2(0.0, Style::SP_S + Style::SP_XS),
        Align2::LEFT_CENTER,
        "Native offline navigation, local MG90 management, simulator active",
        FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );

    let mut x = rect.right() - Style::SP_M;
    x = header_chip(ui, rect, x, "25 GB offline cap", Style::ACCENT_SYSTEM);
    x = header_chip(ui, rect, x, "Direct Ethernet", Style::ACCENT_TERMINALS);
    let sim_tone = if state.simulator_enabled {
        Style::OK
    } else {
        Style::WARN
    };
    let _ = header_chip(ui, rect, x, "Simulator", sim_tone);
}

fn header_chip(ui: &egui::Ui, header: Rect, right: f32, label: &str, tone: Color32) -> f32 {
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        FontId::proportional(Style::SMALL),
        Style::TEXT,
    );
    let chip_w = galley.size().x + Style::SP_M + Style::SP_S;
    let rect = Rect::from_min_size(
        egui::pos2(right - chip_w, header.center().y - Style::SP_S),
        egui::vec2(chip_w, Style::SP_M),
    );
    ui.painter()
        .rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    ui.painter().circle_filled(
        egui::pos2(rect.left() + Style::SP_S, rect.center().y),
        3.0,
        tone,
    );
    ui.painter().galley(
        egui::pos2(
            rect.left() + Style::SP_M,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        Style::TEXT,
    );
    rect.left() - Style::SP_S
}

fn tab_rail(ui: &mut egui::Ui, state: &mut MapsLocationSurface) {
    egui::Frame::NONE
        .fill(Style::LAYER_01)
        .inner_margin(Style::SP_S)
        .show(ui, |ui| {
            ui.set_width(RAIL_W);
            ui.vertical(|ui| {
                // Primary surfaces — the clean first-level nav.
                for tab in WorkspaceTab::PRIMARY {
                    if rail_button(ui, tab.label(), state.active == tab).clicked() {
                        state.active = tab;
                    }
                }
                // "Advanced" — progressive disclosure for the technical/config
                // sections. Tapping it expands/collapses the nested submenu; it
                // reads as selected while one of its children is the active tab.
                let open = state.advanced_open();
                if advanced_parent_button(ui, state.active.is_advanced(), open).clicked() {
                    state.toggle_advanced();
                }
                if open {
                    // Second-level list, indented under Advanced.
                    ui.horizontal(|ui| {
                        ui.add_space(Style::SP_S);
                        ui.vertical(|ui| {
                            for tab in WorkspaceTab::ADVANCED {
                                if rail_button(ui, tab.label(), state.active == tab).clicked() {
                                    state.active = tab;
                                }
                            }
                        });
                    });
                }
            });
        });
}

/// The top-level **Advanced** rail entry: a [`rail_button`] carrying a
/// disclosure chevron (▸ collapsed / ▾ expanded). Reads as selected while one
/// of its nested sections is active so the driver always knows they are inside
/// the Advanced group even when the submenu is collapsed.
fn advanced_parent_button(ui: &mut egui::Ui, selected: bool, expanded: bool) -> egui::Response {
    let size = egui::vec2(ui.available_width(), Style::SP_XL);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
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
    ui.painter().text(
        egui::pos2(rect.left() + Style::SP_S, rect.center().y),
        Align2::LEFT_CENTER,
        "Advanced",
        FontId::proportional(Style::BODY),
        text_color,
    );
    // Disclosure chevron on the right edge (Carbon glyph, painter fallback).
    let box_r = Style::SP_S;
    let icon_box = safe_rect(
        rect.right() - Style::SP_S - box_r * 2.0,
        rect.center().y - box_r,
        box_r * 2.0,
        box_r * 2.0,
    );
    let name = if expanded {
        "chevron--down"
    } else {
        "chevron--right"
    };
    if !paint_carbon(ui.painter(), icon_box, name, text_color) {
        paint_disclosure_chevron(ui.painter(), icon_box.center(), expanded, text_color);
    }
    ui.add_space(Style::SP_XS);
    response
}

/// Painter fallback for the Advanced disclosure chevron: a small triangle
/// pointing down when `expanded`, right when collapsed.
fn paint_disclosure_chevron(painter: &Painter, center: Pos2, expanded: bool, color: Color32) {
    if !center.x.is_finite() || !center.y.is_finite() {
        return;
    }
    let r = Style::SP_XS;
    let pts = if expanded {
        vec![
            egui::pos2(center.x - r, center.y - r * 0.5),
            egui::pos2(center.x + r, center.y - r * 0.5),
            egui::pos2(center.x, center.y + r * 0.7),
        ]
    } else {
        vec![
            egui::pos2(center.x - r * 0.5, center.y - r),
            egui::pos2(center.x + r * 0.7, center.y),
            egui::pos2(center.x - r * 0.5, center.y + r),
        ]
    };
    painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
}

fn rail_button(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let size = egui::vec2(ui.available_width(), Style::SP_XL);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
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

/// Normalized (u right, v down) route polyline the synthetic HUD scene follows.
/// `v == 1.0` is the near edge (bottom) so road/route ribbons taper wider there.
const ROUTE_UV: &[(f32, f32)] = &[
    (0.50, 1.05),
    (0.50, 0.62),
    (0.52, 0.46),
    (0.585, 0.32),
    (0.64, 0.22),
    (0.68, 0.14),
];

/// Normalized alternate-route polyline (drawn dimmer than the active route).
const ALT_UV: &[(f32, f32)] = &[(0.50, 0.62), (0.40, 0.50), (0.34, 0.38), (0.30, 0.28)];

/// Fixed screen anchor for the driver's vehicle chevron (not panned/zoomed).
const VEHICLE_UV: (f32, f32) = (0.50, 0.62);

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

/// One lane in the lane-guidance strip: the arrow it shows and whether it is a
/// recommended lane for the upcoming maneuver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LaneCue {
    dir: ManeuverKind,
    recommended: bool,
}

/// Mock a lane set for the upcoming maneuver (Waze / Google-Maps lane guidance).
/// Turn maneuvers get a small bank of lanes with the turn lane(s) highlighted;
/// non-turn maneuvers return an empty set so the strip stays hidden.
fn mock_lanes(kind: ManeuverKind) -> Vec<LaneCue> {
    let lane = |dir, recommended| LaneCue { dir, recommended };
    match kind {
        ManeuverKind::Right => vec![
            lane(ManeuverKind::Straight, false),
            lane(ManeuverKind::Straight, false),
            lane(ManeuverKind::Right, true),
        ],
        ManeuverKind::SlightRight | ManeuverKind::Merge => vec![
            lane(ManeuverKind::Straight, false),
            lane(ManeuverKind::Straight, false),
            lane(ManeuverKind::SlightRight, true),
        ],
        ManeuverKind::Left => vec![
            lane(ManeuverKind::Left, true),
            lane(ManeuverKind::Straight, false),
            lane(ManeuverKind::Straight, false),
        ],
        ManeuverKind::SlightLeft => vec![
            lane(ManeuverKind::SlightLeft, true),
            lane(ManeuverKind::Straight, false),
            lane(ManeuverKind::Straight, false),
        ],
        ManeuverKind::UTurn => vec![
            lane(ManeuverKind::UTurn, true),
            lane(ManeuverKind::Straight, false),
        ],
        // Straight / Roundabout / Arrive: no lane strip.
        _ => Vec::new(),
    }
}

/// Whether the lane-guidance strip should show: a live fix, a lane set exists for
/// the maneuver, and the maneuver is near (within a half mile). A non-finite
/// distance hides the strip (crash-safe).
fn lane_guidance_active(route: &RoutePlan, kind: ManeuverKind, has_fix: bool) -> bool {
    has_fix
        && finite_or(route.distance_to_maneuver_mi, f32::INFINITY) <= 0.5
        && !mock_lanes(kind).is_empty()
}

/// Mock a posted speed limit from the road classification (no live sign data in
/// the simulator slice); the HUD keys the over-limit colour off this.
fn mock_speed_limit(route: &RoutePlan) -> u32 {
    let r = route.current_road.to_ascii_uppercase();
    if r.starts_with("I-") || r.contains("INTERSTATE") || r.contains("FWY") || r.contains("FREEWAY")
    {
        65
    } else if r.starts_with("US-") || r.starts_with("US ") || r.contains("HWY") || r.contains("SR-")
    {
        55
    } else if r.contains("AVE") || r.contains("BLVD") || r.contains("PKWY") {
        40
    } else {
        35
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
                "search" => state.destination_search = true,
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
        &state.locations,
        &state.dead_zones,
        primary,
        has_fix,
        has_fix && !off_route,
    );

    let route = &state.local_navigation.active_route;

    // Top maneuver banner (the dominant instruction, Google-Maps blue), or the
    // amber "Recalculating…" banner when off route.
    let banner = safe_rect(
        rect.left() + margin,
        rect.top() + margin,
        width - 2.0 * margin,
        96.0,
    );
    let kind = maneuver_kind(&route.next_maneuver);
    paint_soft_shadow(&painter, banner, HUD_RADIUS);
    if off_route {
        paint_recalculating_banner(&painter, banner, route, time);
    } else {
        paint_maneuver_banner(&painter, banner, route, kind, has_fix);
    }

    // Lane-guidance strip directly under the banner (only when a turn is near
    // and we are on route).
    let mut below_banner = banner.bottom() + Style::SP_S;
    if !off_route && lane_guidance_active(route, kind, has_fix) {
        let lanes = mock_lanes(kind);
        // Never exceed the banner width; `paint_lane_guidance` skips a too-narrow
        // strip, so a tiny viewport simply drops the lanes (no min>max clamp).
        let lane_w = (lanes.len() as f32 * 56.0).min(banner.width().max(1.0));
        let lane_rect = safe_rect(banner.left(), below_banner, lane_w, 48.0);
        paint_soft_shadow(&painter, lane_rect, HUD_RADIUS_S);
        paint_lane_guidance(&painter, lane_rect, &lanes);
        below_banner = lane_rect.bottom() + Style::SP_S;
    }

    // Alert pills stacked under the banner/lane strip (Waze-style report chips).
    let pill_x = rect.left() + margin;
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
    let traffic = route.traffic_alert.trim();
    if !traffic.is_empty() {
        pill_y = paint_alert_pill(&painter, pill_x, pill_y, "dialog-warning", traffic, TRAFFIC);
    }
    let weather = route.weather_alert.trim();
    if !weather.is_empty() {
        let _ = paint_alert_pill(&painter, pill_x, pill_y, "dialog-warning", weather, WEATHER);
    }

    // Bottom ETA sheet (arrival time coloured by traffic).
    let eta_w = (width * 0.46).clamp(260.0, 460.0);
    let eta = safe_rect(
        rect.center().x - eta_w / 2.0,
        rect.bottom() - margin - 72.0,
        eta_w,
        72.0,
    );
    paint_soft_shadow(&painter, eta, HUD_RADIUS);
    paint_eta_bar(&painter, eta, route, eta_tone(route, offline));

    // Bottom-left speedometer + round speed-limit sign.
    let speed_d = 88.0;
    let speedo = safe_rect(
        rect.left() + margin,
        rect.bottom() - margin - speed_d,
        speed_d,
        speed_d,
    );
    let limit = mock_speed_limit(route);
    paint_speedometer(&painter, speedo, primary, has_fix, limit);
    let sign_r = 32.0;
    let sign_center = egui::pos2(speedo.right() + Style::SP_S + sign_r, speedo.center().y);
    paint_speed_limit_sign(&painter, sign_center, sign_r, limit);

    // Floating action buttons (painted last so they float above everything).
    for (idx, key) in fab_keys.iter().enumerate() {
        if let Some((center, hovered, pressed)) = fab_states[idx] {
            paint_fab(&painter, center, fab_r, hovered, pressed, key, muted);
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

    let start_resp = ui.interact(
        layout.start,
        egui::Id::new("maps-preview-start"),
        Sense::click(),
    );
    if start_resp.clicked() {
        let selected = state.local_navigation.selected_route;
        state.local_navigation.apply_route_option(selected);
        state.route_preview = false;
    }
    let start_hovered = start_resp.hovered();
    let start_pressed = start_resp.is_pointer_button_down_on();

    // --- Paint. -------------------------------------------------------------
    let primary = state.locations.primary_sample();
    let has_fix = primary.is_some_and(LocationSample::has_fix);
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
        &state.locations,
        &state.dead_zones,
        primary,
        has_fix,
        has_fix,
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
        FontId::proportional(Style::TITLE),
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

    // Route option cards.
    let selected = state.local_navigation.selected_route;
    for (idx, orect) in layout.options.iter().enumerate() {
        if let Some(option) = state.local_navigation.route_options.get(idx) {
            let (hovered, pressed) = option_states.get(idx).copied().unwrap_or((false, false));
            paint_route_option_card(&painter, *orect, option, idx == selected, hovered, pressed);
        }
    }

    // Start button.
    paint_start_button(
        &painter,
        layout.start,
        start_hovered,
        start_pressed,
        has_fix,
    );
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
    let name_s = elide(painter, name, FontId::proportional(Style::TITLE), max_w);
    painter.text(
        egui::pos2(tx, rect.center().y - Style::SP_S),
        Align2::LEFT_CENTER,
        &name_s,
        FontId::proportional(Style::TITLE),
        Style::TEXT_STRONG,
    );
    let addr_s = elide(painter, addr, FontId::proportional(Style::BODY), max_w);
    painter.text(
        egui::pos2(tx, rect.center().y + Style::SP_M - 2.0),
        Align2::LEFT_CENTER,
        &addr_s,
        FontId::proportional(Style::BODY),
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
        FontId::proportional(Style::SMALL),
        if selected {
            Style::ACCENT_HI
        } else {
            Style::TEXT_DIM
        },
    );

    // Hero: total minutes for this option, coloured by traffic.
    let minutes = option.remaining_time_min.to_string();
    let num_g = painter.layout_no_wrap(minutes, FontId::proportional(27.0), tone);
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
        FontId::proportional(Style::BODY),
        tone.gamma_multiply(0.92),
    );

    // Distance · via road (bottom-left).
    let sub = format!(
        "{:.1} mi   \u{00B7}   via {}",
        finite_or(option.remaining_distance_mi, 0.0).max(0.0),
        option.via
    );
    let sub_max = (rect.right() - (rect.left() + pad) - 96.0).max(1.0);
    let sub_s = elide(painter, &sub, FontId::proportional(Style::SMALL), sub_max);
    painter.text(
        egui::pos2(rect.left() + pad, rect.bottom() - 9.0),
        Align2::LEFT_BOTTOM,
        &sub_s,
        FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );

    // Traffic dot + label (right, vertically centered).
    let label_g = painter.layout_no_wrap(
        option.traffic.label().to_string(),
        FontId::proportional(Style::BODY),
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
fn paint_start_button(painter: &Painter, rect: Rect, hovered: bool, pressed: bool, has_fix: bool) {
    paint_soft_shadow(painter, rect, HUD_RADIUS_S);
    let base = if !has_fix {
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
        Stroke::new(1.0, MANEUVER_BLUE_HI),
        StrokeKind::Inside,
    );

    // Nav-arrow glyph + "Start", centered as a group.
    let label = "Start";
    let g = painter.layout_no_wrap(
        label.to_string(),
        FontId::proportional(Style::HEADING),
        Color32::WHITE,
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
            Color32::WHITE,
            false,
        );
    }
    painter.galley(
        egui::pos2(
            start_x + glyph_w + Style::SP_S,
            rect.center().y - g.size().y * 0.5,
        ),
        g,
        Color32::WHITE,
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

    let n_rows = state.local_navigation.destinations.len();
    let layout = search_layout(rect, n_rows, SEARCH_CATEGORIES.len());

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

    let bar_resp = ui.interact(
        layout.search_bar,
        egui::Id::new("maps-search-bar"),
        Sense::click(),
    );
    let bar_hovered = bar_resp.hovered();

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
            state.choose_destination(i);
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
        &state.locations,
        &state.dead_zones,
        primary,
        has_fix,
        false,
    );
    painter.rect_filled(rect, Style::RADIUS_L, Color32::BLACK.gamma_multiply(0.5));

    // Back button + search bar.
    let back_r = layout.back.width() * 0.5;
    paint_round_button(&painter, layout.back.center(), back_r, back_hovered, false);
    paint_back_glyph(&painter, layout.back.center(), back_r);
    paint_search_bar(&painter, layout.search_bar, bar_hovered, "Where to?");

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
        "Recent & saved",
        FontId::proportional(Style::BODY),
        Style::TEXT_DIM,
    );

    // Destination rows.
    for (i, rrect) in layout.rows.iter().enumerate() {
        if let Some(destination) = state.local_navigation.destinations.get(i) {
            let (hovered, pressed) = row_states.get(i).copied().unwrap_or((false, false));
            paint_destination_row(&painter, *rrect, destination, hovered, pressed);
        }
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
        &state.locations,
        &state.dead_zones,
        primary,
        has_fix,
        false,
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
/// route: a rotating spinner chip + status text, keyed to the Quasar-dark skin.
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
fn paint_map_scene(
    painter: &Painter,
    rect: Rect,
    map: &MapViewState,
    locations: &LocationManager,
    dead_zones: &DeadZoneState,
    primary: Option<&LocationSample>,
    has_fix: bool,
    route_live: bool,
) {
    let bg = if map.dark_mode {
        MAP_DARK_BG
    } else {
        MAP_LIGHT_BG
    };
    let road_fill = if map.dark_mode { ROAD_DARK } else { ROAD_LIGHT };
    let road_casing = if map.dark_mode {
        ROAD_CASING_DARK
    } else {
        ROAD_CASING_LIGHT
    };
    painter.rect_filled(rect, Style::RADIUS_L, bg);
    paint_vignette(painter, rect);
    paint_perspective_grid(painter, rect, map, road_fill);

    // Road network — casing under fill, tapered wider toward the viewer.
    for uv in [
        &[(0.0_f32, 0.50_f32), (1.0, 0.455)][..],
        &[(0.0, 0.82), (0.5, 0.62), (1.0, 0.58)][..],
        &[(0.16, 1.05), (0.34, 0.62), (0.44, 0.30), (0.5, 0.10)][..],
        &[(0.66, 1.05), (0.66, 0.5), (0.84, 0.30), (0.98, 0.20)][..],
    ] {
        paint_road(painter, rect, map, uv, 15.0, 6.0, road_casing, road_fill);
    }
    paint_road(
        painter,
        rect,
        map,
        ROUTE_UV,
        30.0,
        8.0,
        road_casing,
        road_fill,
    );

    // Optional overlays keyed off the Map-tab toggles.
    if map.dead_zone_overlay {
        for (idx, _) in dead_zones.zones.iter().enumerate() {
            let c = scene_point(rect, map, 0.30 + idx as f32 * 0.16, 0.42);
            painter.circle_filled(c, 30.0, Style::DANGER.gamma_multiply(0.16));
            painter.circle_stroke(c, 30.0, Stroke::new(1.5, Style::DANGER.gamma_multiply(0.7)));
        }
    }
    if map.weather_overlay {
        let a = scene_point(rect, map, 0.60, 0.10);
        let b = scene_point(rect, map, 1.05, 0.44);
        painter.rect_filled(
            Rect::from_two_pos(a, b),
            Style::RADIUS,
            WEATHER.gamma_multiply(0.12),
        );
    }
    if map.traffic_overlay {
        let ta = scene_point(rect, map, 0.585, 0.32);
        let tb = scene_point(rect, map, 0.64, 0.22);
        painter.line_segment([ta, tb], Stroke::new(7.0 * zoom_scale(map), TRAFFIC));
    }

    // Route — the layered GMaps look (casing + bright core, rounded joints).
    // A dimmed grey line when not live (no fix, or off-route recalculating).
    if map.route_visible {
        paint_route(painter, rect, map, route_live);
    }

    // Location-health crumbs.
    for (idx, crumb) in locations
        .sources
        .iter()
        .filter(|source| source.kind == locations.primary || source.sample.healthy())
        .enumerate()
    {
        let c = scene_point(
            rect,
            map,
            0.22 + idx as f32 * 0.05,
            0.86 - idx as f32 * 0.04,
        );
        painter.circle_filled(c, 3.5, health_color(&crumb.sample));
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

fn paint_perspective_grid(painter: &Painter, rect: Rect, map: &MapViewState, road: Color32) {
    let color = road.gamma_multiply(0.30);
    let c = rect.center()
        + egui::vec2(
            finite_or(map.pan[0], 0.0) * 0.5,
            finite_or(map.pan[1], 0.0) * 0.5,
        );
    for i in -5..=5 {
        let off = i as f32 * 46.0;
        painter.line_segment(
            [
                egui::pos2(rect.left(), c.y + off * 0.7),
                egui::pos2(rect.right(), c.y + off),
            ],
            Stroke::new(1.0, color),
        );
        painter.line_segment(
            [
                egui::pos2(c.x + off, rect.top()),
                egui::pos2(c.x + off * 0.62, rect.bottom()),
            ],
            Stroke::new(1.0, color),
        );
    }
}

/// Build ribbon points (screen pos + width) for a normalized polyline, tapered
/// so the near (high-`v`) end is `w_near` wide and the far end `w_far`.
fn ribbon_points(
    rect: Rect,
    map: &MapViewState,
    uv: &[(f32, f32)],
    w_near: f32,
    w_far: f32,
) -> Vec<(Pos2, f32)> {
    let z = zoom_scale(map);
    uv.iter()
        .map(|&(u, v)| {
            let p = scene_point(rect, map, u, v);
            let w = w_far + (w_near - w_far) * v.clamp(0.0, 1.0);
            (p, (w * z).max(1.0))
        })
        .collect()
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

#[allow(clippy::too_many_arguments)]
fn paint_road(
    painter: &Painter,
    rect: Rect,
    map: &MapViewState,
    uv: &[(f32, f32)],
    w_near: f32,
    w_far: f32,
    casing: Color32,
    fill: Color32,
) {
    let casing_pts = ribbon_points(rect, map, uv, w_near + 5.0, w_far + 4.0);
    paint_ribbon(painter, &casing_pts, casing);
    let fill_pts = ribbon_points(rect, map, uv, w_near, w_far);
    paint_ribbon(painter, &fill_pts, fill);
}

fn paint_route(painter: &Painter, rect: Rect, map: &MapViewState, active: bool) {
    if !active {
        // Planned but not active — a dim grey line, no glow.
        let dim = ribbon_points(rect, map, ROUTE_UV, 10.0, 4.0);
        paint_ribbon(painter, &dim, Style::TEXT_DIM.gamma_multiply(0.5));
        return;
    }
    let glow = ribbon_points(rect, map, ROUTE_UV, 30.0, 12.0);
    paint_ribbon(painter, &glow, ROUTE_BLUE.gamma_multiply(0.16));
    let casing = ribbon_points(rect, map, ROUTE_UV, 20.0, 8.0);
    paint_ribbon(painter, &casing, ROUTE_CASING);
    let core = ribbon_points(rect, map, ROUTE_UV, 13.0, 5.0);
    paint_ribbon(painter, &core, ROUTE_BLUE);
    let alt = ribbon_points(rect, map, ALT_UV, 9.0, 4.0);
    paint_ribbon(painter, &alt, ROUTE_ALT.gamma_multiply(0.8));

    // Turn marker where the next maneuver happens.
    let z = zoom_scale(map);
    let m = scene_point(rect, map, ROUTE_UV[3].0, ROUTE_UV[3].1);
    painter.circle_filled(m, 7.0 * z, Color32::WHITE);
    painter.circle_filled(m, 4.5 * z, ROUTE_BLUE);
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

/// Paint the lane-guidance strip: a row of lane cells, recommended lane(s)
/// bright over a soft accent pill, the rest dimmed (Waze / Google-Maps grammar).
fn paint_lane_guidance(painter: &Painter, rect: Rect, lanes: &[LaneCue]) {
    if lanes.is_empty() || !rect.width().is_finite() || rect.width() < 8.0 {
        return;
    }
    painter.rect_filled(rect, HUD_RADIUS_S, LANE_BG);
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

    let n = lanes.len().max(1);
    let cell_w = rect.width() / n as f32;
    for (i, lane) in lanes.iter().enumerate() {
        let cx = rect.left() + (i as f32 + 0.5) * cell_w;
        if !cx.is_finite() {
            continue;
        }
        let cell_c = egui::pos2(cx, rect.center().y);
        let arrow_side = rect.height().min(cell_w) * 0.72;
        let arrow_rect = safe_rect(
            cell_c.x - arrow_side * 0.5,
            cell_c.y - arrow_side * 0.5,
            arrow_side,
            arrow_side,
        );
        if lane.recommended {
            painter.rect_filled(
                arrow_rect.expand(3.0),
                HUD_RADIUS_S,
                ROUTE_BLUE.gamma_multiply(0.22),
            );
        }
        let color = if lane.recommended {
            Color32::WHITE
        } else {
            LANE_DIM
        };
        paint_maneuver_arrow(painter, arrow_rect, lane.dir, color);
        if i > 0 {
            let sx = rect.left() + i as f32 * cell_w;
            painter.line_segment(
                [
                    egui::pos2(sx, rect.top() + 6.0),
                    egui::pos2(sx, rect.bottom() - 6.0),
                ],
                Stroke::new(1.0, MANEUVER_BLUE_HI.gamma_multiply(0.18)),
            );
        }
    }
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
    limit: u32,
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
        (true, Some(v)) => {
            let over = limit > 0 && v > limit as f32 + 0.5;
            let far_over = limit > 0 && v > limit as f32 + 8.0;
            let tone = if far_over {
                Style::DANGER
            } else if over {
                Style::WARN
            } else {
                Style::TEXT_STRONG
            };
            (format!("{:.0}", v.max(0.0)), tone)
        }
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

/// A round Waze/EU-style speed-limit sign: white face, red ring, black number.
fn paint_speed_limit_sign(painter: &Painter, center: Pos2, radius: f32, limit: u32) {
    if limit == 0 {
        return;
    }
    painter.circle_filled(
        center + egui::vec2(0.0, 2.5),
        radius,
        Color32::BLACK.gamma_multiply(0.35),
    );
    painter.circle_filled(center, radius, SIGN_WHITE);
    painter.circle_stroke(center, radius, Stroke::new(radius * 0.16, SIGN_RED));
    painter.text(
        center,
        Align2::CENTER_CENTER,
        &limit.to_string(),
        FontId::proportional(radius * 0.92),
        SIGN_INK,
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
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut state.map.dark_mode, "Dark mode");
        ui.checkbox(&mut state.map.route_visible, "Route");
        ui.checkbox(&mut state.map.traffic_overlay, "Traffic");
        ui.checkbox(&mut state.map.weather_overlay, "Weather");
        ui.checkbox(&mut state.map.dead_zone_overlay, "Dead zones");
        ui.checkbox(&mut state.map.gnss_overlay, "GNSS quality");
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

fn show_routes_trips(ui: &mut egui::Ui, state: &MapsLocationSurface) {
    let col_w = split_width(ui, 2);
    ui.horizontal_top(|ui| {
        ui.scope(|ui| {
            ui.set_width(col_w);
            card(ui, "Active route", |ui| {
                metric(
                    ui,
                    "Current road",
                    &state.local_navigation.active_route.current_road,
                    Style::TEXT,
                );
                metric(
                    ui,
                    "Alternatives",
                    &state.local_navigation.active_route.alternatives.to_string(),
                    Style::ACCENT,
                );
                metric(
                    ui,
                    "Traffic",
                    &state.local_navigation.active_route.traffic_alert,
                    Style::WARN,
                );
                metric(
                    ui,
                    "Weather",
                    &state.local_navigation.active_route.weather_alert,
                    WEATHER,
                );
            });
        });
        ui.scope(|ui| {
            ui.set_width(col_w);
            trip_card(ui, &state.trips);
        });
    });
    ui.add_space(Style::SP_S);
    card(ui, "Breadcrumb replay and event history", |ui| {
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

fn show_vehicle(ui: &mut egui::Ui, vehicle: &VehicleState) {
    let telem = &vehicle.telemetry;
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
    // Hero gauges — the four live readouts that matter at a glance.
    let tile_w = split_width(ui, 4);
    ui.horizontal_top(|ui| {
        stat_tile(
            ui,
            tile_w,
            "go-next",
            "Speed · mph",
            &format!("{:.0}", telem.speed_mph),
            Style::TEXT_STRONG,
        );
        stat_tile(
            ui,
            tile_w,
            "view-refresh",
            "Engine · rpm",
            &telem.rpm.to_string(),
            Style::ACCENT,
        );
        stat_tile(
            ui,
            tile_w,
            "notification",
            "Battery · V",
            &format!("{:.1}", telem.battery_v),
            voltage_tone(telem.battery_v),
        );
        stat_tile(
            ui,
            tile_w,
            "weather-clear-night",
            "Coolant · °C",
            &format!("{:.0}", telem.coolant_c),
            coolant_tone(telem.coolant_c),
        );
    });
    ui.add_space(Style::SP_S);
    glyph_card(
        ui,
        "view-grid",
        "OBD telematics",
        Style::ACCENT_MESH,
        |ui| {
            readout(
                ui,
                "Ignition",
                if telem.ignition_on { "on" } else { "off" },
                if telem.ignition_on {
                    Style::OK
                } else {
                    Style::TEXT_DIM
                },
            );
            readout(
                ui,
                "Motion",
                if telem.moving { "moving" } else { "parked" },
                if telem.moving { Style::WARN } else { Style::OK },
            );
            readout(
                ui,
                "Fuel",
                &telem
                    .fuel_percent
                    .map_or_else(|| "unavailable".to_string(), |fuel| format!("{fuel:.0}%")),
                telem.fuel_percent.map_or(Style::TEXT_DIM, |fuel| {
                    if fuel < 15.0 {
                        Style::WARN
                    } else {
                        Style::OK
                    }
                }),
            );
            readout(
                ui,
                "Diagnostic codes",
                &telem.dtc_count.to_string(),
                count_tone(telem.dtc_count),
            );
            readout(
                ui,
                "Odometer",
                &telem
                    .odometer_mi
                    .map_or_else(|| "unavailable".to_string(), |odo| format!("{odo} mi")),
                Style::TEXT,
            );
            readout(
                ui,
                "Runtime",
                &format!("{} min", telem.runtime_min),
                Style::TEXT,
            );
            readout(
                ui,
                "Confidence",
                dash_if_empty(&telem.confidence),
                Style::TEXT_DIM,
            );
            readout(
                ui,
                "Last update",
                &format!("{:.1} s ago", telem.last_update_age_s),
                if telem.last_update_age_s <= 5.0 {
                    Style::TEXT_DIM
                } else {
                    Style::WARN
                },
            );
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
}

fn show_connectivity(ui: &mut egui::Ui, mg90: &Mg90State) {
    let status = &mg90.status;
    // Hero readouts: the four numbers that describe the live WAN at a glance.
    let latency_tone = if status.latency_ms < 100 {
        Style::OK
    } else if status.latency_ms < 200 {
        Style::WARN
    } else {
        Style::DANGER
    };
    let loss_tone = if status.packet_loss_percent < 1.0 {
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
        stat_tile(
            ui,
            tile_w,
            "view-refresh",
            "Latency",
            &format!("{} ms", status.latency_ms),
            latency_tone,
        );
        stat_tile(
            ui,
            tile_w,
            "notification",
            "Packet loss",
            &format!("{:.1}%", status.packet_loss_percent),
            loss_tone,
        );
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
        readout(
            ui,
            "Failover events",
            &status.failover_events.to_string(),
            if status.failover_events == 0 {
                Style::OK
            } else {
                Style::WARN
            },
        );
    });
}

fn show_devices_io(ui: &mut egui::Ui, devices: &mut DeviceIoState) {
    let col_w = split_width(ui, 2);
    ui.horizontal_top(|ui| {
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
                    ui.horizontal(|ui| {
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
            metric(ui, "Fix", &source.sample.fix_type, Style::TEXT);
            metric(
                ui,
                "Lat / Lon",
                &format!(
                    "{:.5}, {:.5}",
                    source.sample.latitude, source.sample.longitude
                ),
                Style::TEXT,
            );
            metric(
                ui,
                "Accuracy",
                &format!("{:.1} m", source.sample.accuracy_m),
                health_color(&source.sample),
            );
            metric(
                ui,
                "Speed",
                &format!("{:.1} mph", source.sample.speed_mph),
                Style::TEXT,
            );
            metric(
                ui,
                "Heading",
                &format!("{:.0} deg", source.sample.heading_deg),
                Style::TEXT,
            );
            metric(
                ui,
                "Altitude",
                &format!("{:.1} m", source.sample.altitude_m),
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
                &format!(
                    "{:.1} Hz / {:.1} s",
                    source.sample.update_rate_hz, source.sample.update_age_s
                ),
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

fn show_mg90_setup(ui: &mut egui::Ui, mg90: &mut Mg90State, offline_maps: &OfflineMapManagerState) {
    let done = SetupStep::ALL
        .iter()
        .position(|step| *step == mg90.setup_step)
        .map_or(0, |index| index + 1);
    let total = SetupStep::ALL.len();

    let col_w = split_width(ui, 2);
    ui.horizontal_top(|ui| {
        ui.scope(|ui| {
            ui.set_width(col_w);
            glyph_card(ui, "view-grid", "Device inventory", Style::ACCENT, |ui| {
                readout(
                    ui,
                    "Managed MG90s",
                    &mg90.managed_devices.to_string(),
                    Style::TEXT,
                );
                readout(ui, "Model", mg90.model.label(), Style::TEXT);
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
    ui.horizontal_top(|ui| {
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
                    ui.add_space(Style::SP_XS);
                    if ui.button("Advance simulator setup").clicked() {
                        mg90.advance_setup_simulated();
                    }
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
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Confirm")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                ui.text_edit_singleline(&mut mg90.reset.typed_confirmation);
                let enabled = mg90.reset.armed();
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    let _ = ui.add_enabled(enabled, egui::Button::new("Reset MG90"));
                });
            });
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
                        "Section detected by capability profile; no descriptor loaded in the simulator fixture.",
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
    let col_w = split_width(ui, 2);
    ui.horizontal_top(|ui| {
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

fn show_simulator(ui: &mut egui::Ui, state: &mut MapsLocationSurface) {
    let offline_status = state.offline_navigation_status();
    offline_navigation_card(ui, &offline_status);
    ui.add_space(Style::SP_S);
    card(ui, "Simulator coverage", |ui| {
        ui.horizontal(|ui| {
            ui.label("Simulator mode");
            ui.label(RichText::new(bool_label(state.simulator_enabled)).color(Style::OK));
        });
        for item in [
            "MG90 discovery, local status, settings, reset, and firmware workflow",
            "MG90 GNSS, USB GPS, stale GPS, bad GPS accuracy, and manual source selection",
            "Driving, routing, offline maps, traffic, weather, satellite unavailable states",
            "Cellular dead zones, GPIO events, CAN/OBD, and Ford 2020 Interceptor telemetry",
            "Lost Ethernet, bad credentials, backups, rollback, and diagnostic exports",
        ] {
            bullet(ui, item);
        }
    });
    ui.add_space(Style::SP_S);
    card(ui, "Simulator scenarios", |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Stale primary source").clicked() {
                state.simulate_stale_primary_location();
            }
            if ui.button("Offline maps missing").clicked() {
                state.simulate_no_offline_maps();
            }
            if ui.button("Restore offline ready").clicked() {
                state.simulate_ready_offline_navigation();
            }
            if ui.button("Record cellular dead zone").clicked() {
                state.simulate_cellular_dead_zone();
            }
        });
        bullet(
            ui,
            "Scenario buttons mutate the same readiness model used by Drive, Map, and Location Sources.",
        );
        bullet(
            ui,
            "The simulator never auto-failovers; a healthy peer source still requires manual primary selection.",
        );
    });
    ui.add_space(Style::SP_S);
    card(ui, "Navigation flow (dev toggles)", |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Open \"Where to?\" search").clicked() {
                state.open_destination_search();
            }
            if ui.button("Preview route").clicked() {
                state.active = WorkspaceTab::Drive;
                state.route_preview = true;
            }
            let off_route_label = if state.off_route {
                "Clear off-route"
            } else {
                "Simulate off-route"
            };
            if ui.button(off_route_label).clicked() {
                state.toggle_off_route();
            }
            if ui.button("Arrive").clicked() {
                state.simulate_arrival();
            }
        });
        bullet(
            ui,
            "Drives the full flow: search -> preview -> Start -> HUD -> arrival, plus the off-route recalculating banner.",
        );
    });
    ui.add_space(Style::SP_S);
    show_vault(ui, &state.vault);
    ui.add_space(Style::SP_S);
    card(ui, "Known real-hardware gaps", |ui| {
        for gap in &state.real_hardware_gaps {
            bullet(ui, gap);
        }
    });
    ui.add_space(Style::SP_S);
    backups(ui, &state.mg90.backups);
}

fn map_canvas(
    ui: &mut egui::Ui,
    map: &mut MapViewState,
    locations: &LocationManager,
    dead_zones: &DeadZoneState,
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
        &painter, rect, map, locations, dead_zones, primary, has_fix, has_fix,
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
    painter.text(
        rect.right_bottom() - egui::vec2(Style::SP_S, Style::SP_S),
        Align2::RIGHT_BOTTOM,
        &map.attribution,
        FontId::proportional(Style::SMALL),
        chrome,
    );
    rect
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
    egui::Frame::NONE
        .fill(Style::LAYER_02)
        .stroke(Stroke::new(1.0, accent.unwrap_or(Style::BORDER)))
        .inner_margin(Style::SP_M)
        .corner_radius(mde_egui::widgets::corner(Style::RADIUS_M))
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
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(value)
                    .size(Style::SMALL)
                    .color(tone)
                    .monospace(),
            );
        });
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
fn signal_level(dbm: i32) -> usize {
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
                ui.label(
                    RichText::new(format!("{} dBm", link.signal_dbm))
                        .size(Style::TITLE)
                        .color(signal_tone(link.signal_dbm, link.healthy))
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

    #[test]
    fn workspace_tabs_match_product_layout() {
        let labels: Vec<&str> = WorkspaceTab::ALL.iter().map(|tab| tab.label()).collect();
        assert_eq!(
            labels,
            vec![
                "Drive",
                "Airspace",
                "Map",
                "Routes & Trips",
                "Vehicle",
                "Connectivity",
                "Devices & I/O",
                "Location Sources",
                "MG90 Setup",
                "MG90 Settings",
                "Firmware & Recovery",
                "Simulator",
            ]
        );
    }

    #[test]
    fn maps_location_panel_renders_simulated_vertical_slice() {
        let mut surface = MapsLocationSurface::simulated();
        assert!(tessellate(&mut surface) > 0);
    }

    #[test]
    fn maps_header_uses_refined_shared_chrome_height() {
        assert_eq!(
            HEADER_H,
            mde_egui::menubar::BAR_HEIGHT + Style::SP_S,
            "Maps header should inherit the shared refined chrome height"
        );
        assert!(
            HEADER_H < 40.0,
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

    fn route_on(road: &str) -> RoutePlan {
        RoutePlan {
            current_road: road.to_string(),
            next_maneuver: String::new(),
            distance_to_maneuver_mi: 0.0,
            eta: String::new(),
            remaining_time_min: 0,
            remaining_distance_mi: 0.0,
            alternatives: 0,
            traffic_alert: String::new(),
            weather_alert: String::new(),
        }
    }

    #[test]
    fn mock_speed_limit_keys_off_road_class() {
        assert_eq!(mock_speed_limit(&route_on("I-79 N")), 65);
        assert_eq!(mock_speed_limit(&route_on("US-30 W")), 55);
        assert_eq!(mock_speed_limit(&route_on("Grant Ave")), 40);
        assert_eq!(mock_speed_limit(&route_on("2nd St")), 35);
    }

    #[test]
    fn format_distance_switches_to_feet_when_close() {
        assert_eq!(format_distance(0.4), "0.4 mi");
        assert_eq!(format_distance(0.1), "550 ft");
        assert_eq!(format_distance(f32::NAN), "0 ft");
    }

    fn route_near(maneuver: &str, dist_mi: f32) -> RoutePlan {
        RoutePlan {
            current_road: "US-30 W".to_string(),
            next_maneuver: maneuver.to_string(),
            distance_to_maneuver_mi: dist_mi,
            eta: "14:32".to_string(),
            remaining_time_min: 18,
            remaining_distance_mi: 11.6,
            alternatives: 2,
            traffic_alert: String::new(),
            weather_alert: String::new(),
        }
    }

    #[test]
    fn mock_lanes_highlights_the_turn_lane() {
        let right = mock_lanes(ManeuverKind::Right);
        assert_eq!(right.len(), 3);
        let last = right.last().expect("lane present");
        assert!(last.recommended);
        assert_eq!(last.dir, ManeuverKind::Right);
        assert!(!right[0].recommended);

        assert!(
            mock_lanes(ManeuverKind::Left)
                .first()
                .expect("lane present")
                .recommended
        );
        assert!(mock_lanes(ManeuverKind::Straight).is_empty());
        assert!(mock_lanes(ManeuverKind::Roundabout).is_empty());
        assert!(mock_lanes(ManeuverKind::Arrive).is_empty());
    }

    #[test]
    fn lane_guidance_shows_only_near_a_turn_with_fix() {
        let near = route_near("Turn right onto Main St", 0.3);
        assert!(lane_guidance_active(&near, ManeuverKind::Right, true));
        // Far away, no fix, non-finite distance, and non-turn maneuvers all hide it.
        let far = route_near("Turn right onto Main St", 1.4);
        assert!(!lane_guidance_active(&far, ManeuverKind::Right, true));
        assert!(!lane_guidance_active(&near, ManeuverKind::Right, false));
        let nan = route_near("Turn right", f32::NAN);
        assert!(!lane_guidance_active(&nan, ManeuverKind::Right, true));
        assert!(!lane_guidance_active(&near, ManeuverKind::Straight, true));
    }

    #[test]
    fn drive_hud_renders_lane_guidance_near_a_turn() {
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::Drive;
        surface.local_navigation.active_route.next_maneuver = "Turn right onto Main St".to_string();
        surface
            .local_navigation
            .active_route
            .distance_to_maneuver_mi = 0.2;
        assert!(tessellate(&mut surface) > 0);
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
        let mut stale = MapsLocationSurface::simulated();
        stale.active = WorkspaceTab::Simulator;
        stale.simulate_stale_primary_location();
        assert!(tessellate(&mut stale) > 0);

        let mut missing_maps = MapsLocationSurface::simulated();
        missing_maps.active = WorkspaceTab::Simulator;
        missing_maps.simulate_no_offline_maps();
        assert!(tessellate(&mut missing_maps) > 0);

        let mut dead_zone = MapsLocationSurface::simulated();
        dead_zone.active = WorkspaceTab::Simulator;
        dead_zone.simulate_cellular_dead_zone();
        assert!(tessellate(&mut dead_zone) > 0);
    }

    #[test]
    fn location_sources_tessellate_with_blocked_manual_switches() {
        let mut surface = MapsLocationSurface::simulated();
        surface.active = WorkspaceTab::LocationSources;
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

        // 3. Start -> live turn-by-turn HUD.
        surface.route_preview = false;
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
