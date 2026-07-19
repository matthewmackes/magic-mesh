//! Native egui renderer for the Maps & Location workspace.

use mde_egui::egui::{
    self, Align, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, Vec2,
};
use mde_egui::Style;

use crate::model::{
    BackupRecord, CheckState, DeadZoneSeverity, DeadZoneState, DeviceIoState, EncryptedVaultState,
    FirmwareWorkflow, LocationManager, LocationSample, LocationSource, Mg90ManagementMethod,
    Mg90SettingCategory, Mg90SettingDescriptor, Mg90State, OfflineMapManagerState,
    OfflineNavigationReadiness, OfflineNavigationStatus, ProviderContract, RoutePlan,
    SettingValueType, SetupStep, SourceStatus, TripRecorderState, VehicleState, WorkspaceTab,
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

fn show_drive(ui: &mut egui::Ui, state: &mut MapsLocationSurface) {
    let primary = state.locations.primary_sample().cloned();
    let offline_status = state.offline_navigation_status();
    offline_navigation_card(ui, &offline_status);
    ui.add_space(Style::SP_S);
    if let Some(warning) = state.primary_location_warning() {
        warning_strip(ui, &warning, Style::WARN);
    }
    warning_strip(
        ui,
        &state.local_navigation.active_route.traffic_alert,
        TRAFFIC,
    );
    warning_strip(
        ui,
        &state.local_navigation.active_route.weather_alert,
        WEATHER,
    );

    ui.horizontal_top(|ui| {
        map_canvas(
            ui,
            &mut state.map,
            &state.locations,
            &state.local_navigation.active_route,
            &state.dead_zones,
            420.0,
        );
        ui.add_space(Style::SP_M);
        ui.vertical(|ui| {
            ui.set_width(280.0);
            drive_guidance(ui, &state.local_navigation.active_route);
            ui.add_space(Style::SP_S);
            card(ui, "Vehicle", |ui| {
                metric(
                    ui,
                    "Speed",
                    &format!("{:.0} mph", sample_speed(&primary)),
                    Style::TEXT,
                );
                metric(
                    ui,
                    "Heading",
                    &format!("{:.0} deg", sample_heading(&primary)),
                    Style::TEXT,
                );
                metric(
                    ui,
                    "GNSS source",
                    state.locations.primary.label(),
                    Style::ACCENT,
                );
                metric(
                    ui,
                    "Accuracy",
                    &format!("{:.1} m", sample_accuracy(&primary)),
                    health_tone(primary.as_ref()),
                );
                metric(
                    ui,
                    "Active WAN",
                    &state.mg90.status.active_wan,
                    Style::ACCENT_TERMINALS,
                );
                metric(ui, "Cellular", &state.mg90.status.link_quality, Style::OK);
            });
            ui.add_space(Style::SP_S);
            card(ui, "Controls", |ui| {
                ui.horizontal_wrapped(|ui| {
                    let _ = ui.button("Mute");
                    let _ = ui.button("Cancel route");
                    let _ = ui.button("Route overview");
                });
            });
            destinations(ui, &state.local_navigation.destinations);
        });
    });
}

fn drive_guidance(ui: &mut egui::Ui, route: &RoutePlan) {
    card(ui, "Next maneuver", |ui| {
        ui.label(
            RichText::new(&route.next_maneuver)
                .size(Style::TITLE)
                .color(Style::TEXT_STRONG),
        );
        ui.add_space(Style::SP_XS);
        metric(
            ui,
            "Distance",
            &format!("{:.1} mi", route.distance_to_maneuver_mi),
            Style::ACCENT_HI,
        );
        metric(ui, "Current road", &route.current_road, Style::TEXT);
        ui.separator();
        metric(ui, "ETA", &route.eta, Style::TEXT_STRONG);
        metric(
            ui,
            "Remaining",
            &format!(
                "{} min / {:.1} mi",
                route.remaining_time_min, route.remaining_distance_mi
            ),
            Style::TEXT,
        );
    });
}

fn destinations(ui: &mut egui::Ui, destinations: &[crate::model::Destination]) {
    card(ui, "Recent and favorites", |ui| {
        for destination in destinations {
            ui.horizontal(|ui| {
                status_dot(ui, Style::ACCENT);
                ui.label(RichText::new(&destination.label).color(Style::TEXT));
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{:.1} mi", destination.distance_mi))
                            .size(Style::SMALL)
                            .color(Style::TEXT_DIM),
                    );
                    ui.label(
                        RichText::new(&destination.category)
                            .size(Style::SMALL)
                            .color(Style::TEXT_DIM),
                    );
                });
            });
        }
    });
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
        &state.local_navigation.active_route,
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
    map: &mut crate::model::MapViewState,
    locations: &LocationManager,
    route: &RoutePlan,
    dead_zones: &DeadZoneState,
    height: f32,
) {
    let clip_width = ui.clip_rect().width().max(1.0);
    let available = ui.available_width();
    let width = if available.is_finite() && available > 0.0 {
        available.min(clip_width).max(1.0)
    } else {
        clip_width
    };
    let desired = egui::vec2(width, height);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::drag());
    if response.dragged() {
        let delta = response.drag_delta();
        map.pan[0] += delta.x * 0.02;
        map.pan[1] += delta.y * 0.02;
    }
    let scroll = ui.input(|input| input.raw_scroll_delta.y);
    if response.hovered() && scroll.abs() > 0.0 {
        map.zoom = (map.zoom + scroll.signum() * 0.1).clamp(3.0, 18.0);
    }

    let painter = ui.painter_at(rect);
    let bg = if map.dark_mode {
        MAP_DARK_BG
    } else {
        MAP_LIGHT_BG
    };
    let road = if map.dark_mode { ROAD_DARK } else { ROAD_LIGHT };
    painter.rect_filled(rect, Style::RADIUS, bg);
    painter.rect_stroke(
        rect,
        Style::RADIUS,
        Stroke::new(1.0, Style::BORDER),
        egui::StrokeKind::Inside,
    );

    paint_map_grid(&painter, rect, road, map);
    if map.dead_zone_overlay {
        for (idx, _) in dead_zones.zones.iter().enumerate() {
            let center = map_point(rect, idx as f32 * 0.14 + 0.56, 0.46);
            painter.circle_filled(center, 30.0, Style::DANGER.gamma_multiply(0.16));
            painter.circle_stroke(center, 30.0, Stroke::new(1.0, Style::DANGER));
        }
    }
    if map.weather_overlay {
        let weather_rect =
            Rect::from_min_max(map_point(rect, 0.60, 0.18), map_point(rect, 0.98, 0.54));
        painter.rect_filled(weather_rect, Style::RADIUS, WEATHER.gamma_multiply(0.15));
    }
    if map.traffic_overlay {
        let a = map_point(rect, 0.52, 0.56);
        let b = map_point(rect, 0.70, 0.48);
        painter.line_segment([a, b], Stroke::new(5.0, TRAFFIC));
    }
    if map.route_visible {
        let points = [
            map_point(rect, 0.18, 0.78),
            map_point(rect, 0.34, 0.62),
            map_point(rect, 0.48, 0.58),
            map_point(rect, 0.64, 0.40),
            map_point(rect, 0.83, 0.30),
        ];
        painter.add(Shape::line(points.to_vec(), Stroke::new(6.0, ROUTE_BLUE)));
        painter.add(Shape::line(
            vec![
                map_point(rect, 0.28, 0.72),
                map_point(rect, 0.58, 0.70),
                map_point(rect, 0.80, 0.50),
            ],
            Stroke::new(2.0, ROUTE_ALT),
        ));
        painter.text(
            map_point(rect, 0.64, 0.36),
            Align2::LEFT_BOTTOM,
            &route.next_maneuver,
            FontId::proportional(Style::SMALL),
            Style::TEXT_STRONG,
        );
    }

    for (idx, crumb) in locations
        .sources
        .iter()
        .filter(|source| source.kind == locations.primary || source.sample.healthy())
        .enumerate()
    {
        let center = map_point(rect, 0.22 + idx as f32 * 0.05, 0.76 - idx as f32 * 0.04);
        painter.circle_filled(center, 4.0, health_color(&crumb.sample));
    }

    let vehicle = map_point(rect, 0.42, 0.58);
    paint_vehicle_marker(
        &painter,
        vehicle,
        health_color_opt(locations.primary_sample()),
    );
    if map.gnss_overlay {
        painter.circle_stroke(
            vehicle,
            42.0,
            Stroke::new(1.0, Style::ACCENT.gamma_multiply(0.65)),
        );
    }
    painter.text(
        rect.left_top() + egui::vec2(Style::SP_S, Style::SP_S),
        Align2::LEFT_TOP,
        format!(
            "zoom {:.1} | rotate {:.0} deg | pitch {:.0} deg",
            map.zoom, map.rotation_deg, map.pitch_deg
        ),
        FontId::proportional(Style::SMALL),
        if map.dark_mode {
            Style::TEXT_DIM
        } else {
            Style::BG
        },
    );
    painter.text(
        rect.right_bottom() - egui::vec2(Style::SP_S, Style::SP_S),
        Align2::RIGHT_BOTTOM,
        &map.attribution,
        FontId::proportional(Style::SMALL),
        if map.dark_mode {
            Style::TEXT_DIM
        } else {
            Style::BG
        },
    );
}

fn paint_map_grid(
    painter: &egui::Painter,
    rect: Rect,
    road: Color32,
    map: &crate::model::MapViewState,
) {
    let center = rect.center() + egui::vec2(map.pan[0], map.pan[1]);
    for i in -4..=4 {
        let offset = i as f32 * 54.0;
        painter.line_segment(
            [
                egui::pos2(rect.left(), center.y + offset * 0.62),
                egui::pos2(rect.right(), center.y + offset),
            ],
            Stroke::new(if i == 0 { 3.0 } else { 1.0 }, road),
        );
        painter.line_segment(
            [
                egui::pos2(center.x + offset, rect.top()),
                egui::pos2(center.x + offset * 0.68, rect.bottom()),
            ],
            Stroke::new(if i == 1 { 3.0 } else { 1.0 }, road.gamma_multiply(0.86)),
        );
    }
}

fn paint_vehicle_marker(painter: &egui::Painter, center: Pos2, tone: Color32) {
    let points = vec![
        center + egui::vec2(0.0, -18.0),
        center + egui::vec2(12.0, 14.0),
        center + egui::vec2(0.0, 8.0),
        center + egui::vec2(-12.0, 14.0),
    ];
    painter.add(Shape::convex_polygon(
        points,
        tone,
        Stroke::new(2.0, Style::TEXT_STRONG),
    ));
    painter.circle_filled(center, 4.0, Style::TEXT_STRONG);
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

fn sample_speed(sample: &Option<LocationSample>) -> f32 {
    sample.as_ref().map_or(0.0, |sample| sample.speed_mph)
}

fn sample_heading(sample: &Option<LocationSample>) -> f32 {
    sample.as_ref().map_or(0.0, |sample| sample.heading_deg)
}

fn sample_accuracy(sample: &Option<LocationSample>) -> f32 {
    sample.as_ref().map_or(0.0, |sample| sample.accuracy_m)
}

fn health_tone(sample: Option<&LocationSample>) -> Color32 {
    sample.map_or(Style::WARN, health_color)
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

fn health_color_opt(sample: Option<&LocationSample>) -> Color32 {
    sample.map_or(Style::WARN, health_color)
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
