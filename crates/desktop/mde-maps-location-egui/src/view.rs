//! Native egui renderer for the Maps & Location workspace.

use mde_egui::egui::{
    self, Align, Align2, Color32, FontId, Mesh, Painter, Pos2, Rect, RichText, Sense, Shape,
    Stroke, StrokeKind, Vec2,
};
use mde_egui::{paint_carbon, Style};

use crate::model::{
    BackupRecord, CheckState, DeadZoneSeverity, DeadZoneState, DeviceIoState, EncryptedVaultState,
    FirmwareWorkflow, LocationManager, LocationSample, LocationSource, MapViewState,
    Mg90ManagementMethod, Mg90SettingCategory, Mg90SettingDescriptor, Mg90State,
    OfflineMapManagerState, OfflineNavigationReadiness, OfflineNavigationStatus, ProviderContract,
    RoutePlan, SettingValueType, SetupStep, SourceStatus, TripRecorderState, VehicleState,
    WorkspaceTab,
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
const MANEUVER_BLUE: Color32 = Color32::from_rgb(0x14, 0x40, 0x78); // style-leak-ok: map-content-color
const MANEUVER_BLUE_HI: Color32 = Color32::from_rgb(0x1B, 0x53, 0x99); // style-leak-ok: map-content-color
const ROUTE_CASING: Color32 = Color32::from_rgb(0x14, 0x4C, 0x92); // style-leak-ok: map-content-color
const SIGN_WHITE: Color32 = Color32::from_rgb(0xF4, 0xF6, 0xFA); // style-leak-ok: map-content-color
const SIGN_RED: Color32 = Color32::from_rgb(0xD4, 0x2A, 0x2A); // style-leak-ok: map-content-color
const SIGN_INK: Color32 = Color32::from_rgb(0x15, 0x17, 0x1D); // style-leak-ok: map-content-color
const HUD_CARD_BG: Color32 = Color32::from_rgb(0x1A, 0x1B, 0x22); // style-leak-ok: map-content-color
const ROAD_CASING_DARK: Color32 = Color32::from_rgb(0x24, 0x2C, 0x33); // style-leak-ok: map-content-color
const ROAD_CASING_LIGHT: Color32 = Color32::from_rgb(0x9C, 0xA8, 0x9C); // style-leak-ok: map-content-color

/// Render the complete native Maps & Location workspace.
pub fn maps_location_panel(ui: &mut egui::Ui, state: &mut MapsLocationSurface) {
    ui.visuals_mut().override_text_color = Some(Style::TEXT);
    egui::Frame::NONE
        .fill(Style::BG)
        .inner_margin(Style::SP_M)
        .show(ui, |ui| {
            header(ui, state);
            ui.add_space(Style::SP_S);
            ui.horizontal(|ui| {
                tab_rail(ui, state);
                ui.add_space(Style::SP_M);
                egui::Frame::NONE
                    .fill(Style::LAYER_01)
                    .inner_margin(Style::SP_M)
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt(("maps-location-tab", state.active))
                            .auto_shrink([false, false])
                            .show(ui, |ui| match state.active {
                                WorkspaceTab::Drive => show_drive(ui, state),
                                WorkspaceTab::Map => show_map(ui, state),
                                WorkspaceTab::RoutesTrips => show_routes_trips(ui, state),
                                WorkspaceTab::Vehicle => show_vehicle(ui, &state.vehicle),
                                WorkspaceTab::Connectivity => show_connectivity(ui, &state.mg90),
                                WorkspaceTab::DevicesIo => show_devices_io(ui, &mut state.devices),
                                WorkspaceTab::LocationSources => {
                                    show_location_sources(ui, &mut state.locations)
                                }
                                WorkspaceTab::Mg90Setup => {
                                    show_mg90_setup(ui, &mut state.mg90, &state.offline_maps)
                                }
                                WorkspaceTab::Mg90Settings => show_mg90_settings(ui, state),
                                WorkspaceTab::FirmwareRecovery => {
                                    show_firmware_recovery(ui, &state.firmware, &state.devices)
                                }
                                WorkspaceTab::Simulator => show_simulator(ui, state),
                            });
                    });
            });
        });
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
                for tab in WorkspaceTab::ALL {
                    let selected = state.active == tab;
                    let response = rail_button(ui, tab.label(), selected);
                    if response.clicked() {
                        state.active = tab;
                    }
                }
            });
        });
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
    let fab_keys = ["recenter", "mute", "overview"];
    let mut fab_states: [Option<(Pos2, bool, bool)>; 3] = [None; 3];
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
                "mute" => {
                    muted = !muted;
                    ui.ctx().data_mut(|d| d.insert_temp(muted_id, muted));
                }
                _ => {}
            }
        }
        fab_states[idx] = Some((center, resp.hovered(), resp.is_pointer_button_down_on()));
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
    );

    let route = &state.local_navigation.active_route;

    // Top maneuver banner (the dominant instruction, Google-Maps blue).
    let banner = safe_rect(
        rect.left() + margin,
        rect.top() + margin,
        width - 2.0 * margin,
        96.0,
    );
    paint_soft_shadow(&painter, banner, Style::RADIUS_L);
    paint_maneuver_banner(
        &painter,
        banner,
        route,
        maneuver_kind(&route.next_maneuver),
        has_fix,
    );

    // Alert pills stacked under the banner (Waze-style report chips).
    let pill_x = rect.left() + margin;
    let mut pill_y = banner.bottom() + Style::SP_S;
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
    paint_soft_shadow(&painter, eta, Style::RADIUS_L);
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
    if map.route_visible {
        paint_route(painter, rect, map, has_fix);
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

fn paint_route(painter: &Painter, rect: Rect, map: &MapViewState, has_fix: bool) {
    if !has_fix {
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
    let size = 15.0;
    if glow {
        for r in [30.0_f32, 22.0, 15.0] {
            painter.circle_filled(center, r, ROUTE_BLUE.gamma_multiply(0.09));
        }
    }
    let tip = center + f * (size * 1.15);
    let bl = center - f * (size * 0.85) - rt * (size * 0.9);
    let br = center - f * (size * 0.85) + rt * (size * 0.9);
    let notch = center - f * (size * 0.25);
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
        Stroke::new(2.0, Color32::WHITE),
    ));
    painter.circle_filled(center, 2.5, Color32::WHITE);
}

/// A translucent "flashlight" accuracy cone ahead of the vehicle.
fn paint_heading_cone(painter: &Painter, apex: Pos2, heading_deg: f32, tone: Color32) {
    if apex.any_nan() {
        return;
    }
    let a0 = finite_or(heading_deg, 0.0).to_radians();
    let spread = 22.0_f32.to_radians();
    let len = 92.0;
    let n: u32 = 12;
    let mut mesh = Mesh::default();
    mesh.colored_vertex(apex, tone.gamma_multiply(0.30));
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

/// A soft drop shadow behind an elevated card (stacked translucent layers).
fn paint_soft_shadow(painter: &Painter, rect: Rect, radius: f32) {
    for i in (1..=5).rev() {
        let f = i as f32;
        let r = rect.expand(f * 1.4).translate(egui::vec2(0.0, f * 0.7));
        painter.rect_filled(r, radius + f, Color32::BLACK.gamma_multiply(0.06));
    }
}

fn paint_maneuver_banner(
    painter: &Painter,
    rect: Rect,
    route: &RoutePlan,
    kind: ManeuverKind,
    has_fix: bool,
) {
    let fill = if has_fix {
        MANEUVER_BLUE
    } else {
        MANEUVER_BLUE.gamma_multiply(0.6)
    };
    painter.rect_filled(rect, Style::RADIUS_L, fill);
    painter.rect_stroke(
        rect,
        Style::RADIUS_L,
        Stroke::new(1.0, MANEUVER_BLUE_HI),
        StrokeKind::Inside,
    );

    let inset = Style::SP_S;
    let arrow_side = (rect.height() - 2.0 * inset).max(1.0);
    let arrow_rect = safe_rect(
        rect.left() + inset,
        rect.top() + inset,
        arrow_side,
        arrow_side,
    );
    paint_maneuver_arrow(painter, arrow_rect, kind, Color32::WHITE);

    let tx = arrow_rect.right() + Style::SP_M;
    let max_w = (rect.right() - Style::SP_M - tx).max(1.0);
    let dist = format_distance(route.distance_to_maneuver_mi);
    painter.text(
        egui::pos2(tx, rect.top() + inset + 2.0),
        Align2::LEFT_TOP,
        &dist,
        FontId::proportional(34.0),
        Color32::WHITE,
    );
    let man = elide(
        painter,
        &route.next_maneuver,
        FontId::proportional(17.0),
        max_w,
    );
    painter.text(
        egui::pos2(tx, rect.center().y + Style::SP_S),
        Align2::LEFT_CENTER,
        &man,
        FontId::proportional(17.0),
        Color32::WHITE,
    );
    let on_road = elide(
        painter,
        &format!("on {}", route.current_road),
        FontId::proportional(Style::BODY),
        max_w,
    );
    painter.text(
        egui::pos2(tx, rect.bottom() - inset - 1.0),
        Align2::LEFT_BOTTOM,
        &on_road,
        FontId::proportional(Style::BODY),
        Color32::WHITE.gamma_multiply(0.78),
    );
}

fn paint_maneuver_arrow(painter: &Painter, rect: Rect, kind: ManeuverKind, color: Color32) {
    let s = rect.width().min(rect.height());
    if kind == ManeuverKind::Arrive {
        let c = rect.center();
        painter.circle_stroke(c, s * 0.30, Stroke::new(s * 0.10, color));
        painter.circle_filled(c, s * 0.12, color);
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
    let ribbon: Vec<(Pos2, f32)> = pts.iter().map(|&p| (p, s * 0.15)).collect();
    paint_ribbon(painter, &ribbon, color);
    if pts.len() >= 2 {
        let tip = pts[pts.len() - 1];
        let prev = pts[pts.len() - 2];
        let seg = tip - prev;
        let len = seg.length();
        if len > 0.001 {
            let dir = seg / len;
            let perp = egui::vec2(-dir.y, dir.x);
            let hw = s * 0.20;
            let hl = s * 0.26;
            painter.add(Shape::convex_polygon(
                vec![tip + dir * hl, tip + perp * hw, tip - perp * hw],
                color,
                Stroke::NONE,
            ));
        }
    }
}

fn paint_eta_bar(painter: &Painter, rect: Rect, route: &RoutePlan, tone: Color32) {
    painter.rect_filled(rect, Style::RADIUS_L, HUD_CARD_BG);
    painter.rect_stroke(
        rect,
        Style::RADIUS_L,
        Stroke::new(1.0, Style::BORDER),
        StrokeKind::Inside,
    );
    let pad = Style::SP_M;
    painter.circle_filled(egui::pos2(rect.left() + pad, rect.center().y), 5.0, tone);
    let lx = rect.left() + pad + Style::SP_M;
    painter.text(
        egui::pos2(lx, rect.center().y - Style::SP_S),
        Align2::LEFT_CENTER,
        &route.eta,
        FontId::proportional(30.0),
        tone,
    );
    painter.text(
        egui::pos2(lx, rect.center().y + Style::SP_M),
        Align2::LEFT_CENTER,
        "arrival",
        FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );
    let rx = rect.right() - pad;
    painter.text(
        egui::pos2(rx, rect.center().y - Style::SP_S),
        Align2::RIGHT_CENTER,
        &format!("{} min", route.remaining_time_min),
        FontId::proportional(Style::TITLE),
        Style::TEXT_STRONG,
    );
    painter.text(
        egui::pos2(rx, rect.center().y + Style::SP_M),
        Align2::RIGHT_CENTER,
        &format!(
            "{:.1} mi",
            finite_or(route.remaining_distance_mi, 0.0).max(0.0)
        ),
        FontId::proportional(Style::BODY),
        Style::TEXT_DIM,
    );
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
        _ => {}
    }
}

fn show_map(ui: &mut egui::Ui, state: &mut MapsLocationSurface) {
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
    map_canvas(
        ui,
        &mut state.map,
        &state.locations,
        &state.dead_zones,
        500.0,
    );
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
    card(ui, &vehicle.profile, |ui| {
        metric(
            ui,
            "Vehicle speed",
            &format!("{:.0} mph", vehicle.telemetry.speed_mph),
            Style::TEXT_STRONG,
        );
        metric(
            ui,
            "Engine RPM",
            &vehicle.telemetry.rpm.to_string(),
            Style::TEXT,
        );
        metric(
            ui,
            "Coolant",
            &format!("{:.1} C", vehicle.telemetry.coolant_c),
            Style::TEXT,
        );
        metric(
            ui,
            "Battery",
            &format!("{:.1} V", vehicle.telemetry.battery_v),
            Style::OK,
        );
        metric(
            ui,
            "Fuel",
            &vehicle
                .telemetry
                .fuel_percent
                .map_or_else(|| "unavailable".to_string(), |fuel| format!("{fuel:.0}%")),
            Style::TEXT,
        );
        metric(
            ui,
            "DTCs",
            &vehicle.telemetry.dtc_count.to_string(),
            Style::TEXT,
        );
        metric(
            ui,
            "Ignition",
            bool_label(vehicle.telemetry.ignition_on),
            Style::ACCENT,
        );
        metric(
            ui,
            "Moving",
            bool_label(vehicle.telemetry.moving),
            Style::WARN,
        );
        metric(
            ui,
            "Odometer",
            &vehicle
                .telemetry
                .odometer_mi
                .map_or_else(|| "unavailable".to_string(), |odo| format!("{odo} mi")),
            Style::TEXT,
        );
        metric(
            ui,
            "Runtime",
            &format!("{} min", vehicle.telemetry.runtime_min),
            Style::TEXT,
        );
        metric(
            ui,
            "Telemetry confidence",
            &vehicle.telemetry.confidence,
            Style::TEXT_DIM,
        );
        metric(
            ui,
            "Last update",
            &format!("{:.1} s", vehicle.telemetry.last_update_age_s),
            Style::TEXT_DIM,
        );
    });
    ui.add_space(Style::SP_S);
    card(ui, "Profile integration", |ui| {
        bullet(
            ui,
            "Map events, trip history, route alerts, diagnostic bundles, and motion detection read this profile layer.",
        );
        for note in &vehicle.profile_notes {
            bullet(ui, note);
        }
    });
}

fn show_connectivity(ui: &mut egui::Ui, mg90: &Mg90State) {
    let col_w = split_width(ui, 3);
    ui.horizontal_top(|ui| {
        ui.scope(|ui| {
            ui.set_width(col_w);
            card(ui, "Active WAN", |ui| {
                metric(ui, "Link", &mg90.status.active_wan, Style::ACCENT_HI);
                metric(ui, "Quality", &mg90.status.link_quality, Style::OK);
                metric(
                    ui,
                    "Latency",
                    &format!("{} ms", mg90.status.latency_ms),
                    Style::TEXT,
                );
                metric(
                    ui,
                    "Packet loss",
                    &format!("{:.1}%", mg90.status.packet_loss_percent),
                    Style::TEXT,
                );
            });
        });
        ui.scope(|ui| {
            ui.set_width(col_w);
            cellular_card(ui, "Cellular A", &mg90.status.cellular_a);
        });
        ui.scope(|ui| {
            ui.set_width(col_w);
            cellular_card(ui, "Cellular B", &mg90.status.cellular_b);
        });
    });
    ui.add_space(Style::SP_S);
    card(ui, "Local MG90 surfaces", |ui| {
        metric(ui, "Wi-Fi", &mg90.status.wifi_state, Style::TEXT_DIM);
        metric(ui, "Ethernet", &mg90.status.ethernet_state, Style::OK);
        metric(ui, "VPN", &mg90.status.vpn_state, Style::TEXT_DIM);
        metric(
            ui,
            "Data transferred",
            &mg90.status.data_transferred,
            Style::TEXT,
        );
        metric(
            ui,
            "Failover events",
            &mg90.status.failover_events.to_string(),
            Style::WARN,
        );
    });
}

fn show_devices_io(ui: &mut egui::Ui, devices: &mut DeviceIoState) {
    let col_w = split_width(ui, 2);
    ui.horizontal_top(|ui| {
        ui.scope(|ui| {
            ui.set_width(col_w);
            card(ui, "Serial recovery console", |ui| {
                warning_strip(
                    ui,
                    "Recovery console only; normal settings use direct Ethernet.",
                    Style::WARN,
                );
                ui.horizontal(|ui| {
                    ui.checkbox(&mut devices.serial.connected, "Connected");
                    ui.label(format!("Profile {}", devices.serial.baud_profile));
                });
                for line in &devices.serial.transcript_lines {
                    ui.monospace(line);
                }
                ui.horizontal_wrapped(|ui| {
                    let _ = ui.button("Send command");
                    let _ = ui.button("Copy output");
                    let _ = ui.button("Save transcript");
                });
            });
        });
        ui.scope(|ui| {
            ui.set_width(col_w);
            card(ui, "Device state", |ui| {
                metric(ui, "Ethernet", &devices.ethernet_state, Style::OK);
                metric(ui, "CAN/OBD", &devices.can_obd_state, Style::ACCENT);
                for device in &devices.usb_devices {
                    metric(ui, "USB", device, Style::TEXT);
                }
            });
        });
    });
    ui.add_space(Style::SP_S);
    card(ui, "GPIO automation rules", |ui| {
        for rule in &mut devices.gpio_rules {
            ui.separator();
            ui.horizontal(|ui| {
                ui.checkbox(&mut rule.enabled, "");
                ui.label(RichText::new(&rule.id).color(Style::TEXT_STRONG));
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    let _ = ui.button("Simulator test");
                });
            });
            metric(ui, "Trigger", &rule.trigger, Style::TEXT);
            metric(ui, "Condition", &rule.condition, Style::TEXT_DIM);
            metric(ui, "Action", &rule.action, Style::ACCENT);
            metric(ui, "Last run", &rule.last_run, Style::TEXT_DIM);
            for audit in &rule.audit_log {
                bullet(ui, audit);
            }
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
    let col_w = split_width(ui, 2);
    ui.horizontal_top(|ui| {
        ui.scope(|ui| {
            ui.set_width(col_w);
            card(ui, "Offline first-time setup", |ui| {
                for step in SetupStep::ALL {
                    let tone = if step <= mg90.setup_step {
                        Style::OK
                    } else {
                        Style::TEXT_DIM
                    };
                    ui.horizontal(|ui| {
                        status_dot(ui, tone);
                        ui.label(RichText::new(step.label()).color(tone));
                    });
                }
                if ui.button("Advance simulator setup").clicked() {
                    mg90.advance_setup_simulated();
                }
            });
        });
        ui.scope(|ui| {
            ui.set_width(col_w);
            card(ui, "Direct Ethernet assumptions", |ui| {
                metric(
                    ui,
                    "Managed MG90s",
                    &mg90.managed_devices.to_string(),
                    Style::TEXT,
                );
                metric(
                    ui,
                    "Management path",
                    "dedicated direct Ethernet cable only",
                    Style::OK,
                );
                metric(ui, "Model", mg90.model.label(), Style::TEXT);
                metric(ui, "MGOS", &mg90.capabilities.mgos_version, Style::TEXT);
                metric(
                    ui,
                    "Offline map",
                    &offline_maps.default_region,
                    Style::ACCENT_SYSTEM,
                );
                metric(
                    ui,
                    "Authenticated",
                    bool_label(mg90.authenticated),
                    Style::OK,
                );
            });
        });
    });
    ui.add_space(Style::SP_S);
    card(ui, "Operator checklist", |ui| {
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
    ui.add_space(Style::SP_S);
    card(ui, "Factory reset guardrails", |ui| {
        warning_strip(
            ui,
            "Factory reset loses configuration; backup and typed confirmation are required.",
            Style::DANGER,
        );
        metric(
            ui,
            "Backup required",
            bool_label(mg90.reset.backup_required),
            Style::WARN,
        );
        metric(
            ui,
            "Backup completed",
            bool_label(mg90.reset.backup_completed),
            Style::OK,
        );
        ui.horizontal(|ui| {
            ui.label("Confirmation");
            ui.text_edit_singleline(&mut mg90.reset.typed_confirmation);
            let enabled = mg90.reset.armed();
            let _ = ui.add_enabled(enabled, egui::Button::new("Reset MG90"));
        });
        for step in &mg90.reset.reconnect_workflow {
            bullet(ui, step);
        }
    });
}

fn show_mg90_settings(ui: &mut egui::Ui, state: &MapsLocationSurface) {
    if state.moving() {
        warning_strip(
            ui,
            "Vehicle is moving. Dangerous MG90 changes warn but are not blocked in v1.",
            Style::WARN,
        );
    }
    for category in Mg90SettingCategory::ALL {
        card(ui, category.label(), |ui| {
            let settings: Vec<&Mg90SettingDescriptor> = state
                .mg90
                .settings
                .iter()
                .filter(|setting| setting.category == category)
                .collect();
            if settings.is_empty() {
                ui.label(
                    RichText::new("Capability-detected section is visible; no descriptor loaded in simulator fixture.")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
            }
            for setting in settings {
                setting_row(ui, state, setting);
            }
        });
        ui.add_space(Style::SP_S);
    }
}

fn show_firmware_recovery(ui: &mut egui::Ui, firmware: &FirmwareWorkflow, devices: &DeviceIoState) {
    let col_w = split_width(ui, 2);
    ui.horizontal_top(|ui| {
        ui.scope(|ui| {
            ui.set_width(col_w);
            card(ui, "Firmware lifecycle", |ui| {
            metric(ui, "Current firmware", &firmware.current, Style::TEXT);
            metric(ui, "Target package", &firmware.target_package, Style::TEXT_DIM);
            metric(
                ui,
                "Restore point ready",
                bool_label(firmware.restore_point_ready),
                Style::OK,
            );
            ui.add(egui::ProgressBar::new(f32::from(firmware.progress_percent) / 100.0).text(
                format!("{}%", firmware.progress_percent),
            ));
            for check in &firmware.checks {
                ui.horizontal(|ui| {
                    status_dot(ui, check_tone(check.state));
                    ui.label(&check.label);
                });
            }
            });
        });
        ui.scope(|ui| {
            ui.set_width(col_w);
            card(ui, "Recovery console", |ui| {
            metric(ui, "Serial profile", &devices.serial.baud_profile, Style::TEXT);
            metric(ui, "Connected", bool_label(devices.serial.connected), Style::TEXT);
            bullet(ui, "Do not allow blind firmware install.");
            bullet(ui, "Validate MG90 model, MGOS family, package integrity, power, backup, direct Ethernet, credentials, and rollback plan.");
            bullet(ui, "Post-update reconnect and validation must run before the workflow completes.");
            });
        });
    });
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
) {
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
        return;
    }

    let painter = ui.painter_at(rect);
    let primary = locations.primary_sample();
    let has_fix = primary.is_some_and(LocationSample::has_fix);
    paint_map_scene(&painter, rect, map, locations, dead_zones, primary, has_fix);
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
}

fn map_point(rect: Rect, x: f32, y: f32) -> Pos2 {
    egui::pos2(
        rect.left() + rect.width() * x.clamp(0.0, 1.0),
        rect.top() + rect.height() * y.clamp(0.0, 1.0),
    )
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

fn cellular_card(ui: &mut egui::Ui, label: &str, link: &crate::model::CellularLink) {
    card(ui, label, |ui| {
        metric(ui, "SIM", &link.sim_state, Style::TEXT);
        metric(ui, "Carrier", &link.carrier, Style::TEXT);
        metric(
            ui,
            "Signal",
            &format!("{} dBm", link.signal_dbm),
            if link.healthy { Style::OK } else { Style::WARN },
        );
        metric(ui, "Technology", &link.technology, Style::ACCENT);
        metric(ui, "WAN IP", &link.wan_ip, Style::TEXT_DIM);
    });
}

fn setting_row(ui: &mut egui::Ui, state: &MapsLocationSurface, setting: &Mg90SettingDescriptor) {
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(&setting.display_name).color(Style::TEXT_STRONG));
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
    metric(
        ui,
        "Value type",
        value_type_label(&setting.value_type),
        Style::TEXT,
    );
    if !setting.validation.is_empty() {
        for rule in &setting.validation {
            metric(ui, "Validation", &rule.label, Style::TEXT_DIM);
        }
    }
    if let Some(plan) = state.setting_change_plan(&setting.id) {
        metric(ui, "Plan", &plan.steps.join(" -> "), Style::TEXT_DIM);
        metric(ui, "Backup", bool_label(plan.backup_required), Style::OK);
        metric(
            ui,
            "Rollback",
            bool_label(plan.rollback_supported),
            Style::TEXT,
        );
        metric(
            ui,
            "Moving warning",
            bool_label(plan.moving_warning),
            Style::WARN,
        );
    }
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
