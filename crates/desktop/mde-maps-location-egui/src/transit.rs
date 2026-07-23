//! MBTA GTFS-Realtime nearby-vehicle layer and painter.

use mackes_mesh_types::transit::{TransitOccupancy, TransitSnapshot, TransitVehicle, ATTRIBUTION};
use mde_egui::egui::{self, Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke};
use mde_egui::Style;

/// Vehicles become visibly stale after one minute.
pub const VEHICLE_GREY_AFTER_MS: i64 = 60_000;
/// Vehicles disappear after two minutes without a position.
pub const VEHICLE_DROP_AFTER_MS: i64 = 120_000;
/// Three missed 20-second polls make the feed badge stale.
pub const SNAPSHOT_STALE_AFTER_MS: i64 = 60_000;

const ROUTE_RED: Color32 = Color32::from_rgb(0xDA, 0x29, 0x1C); // style-leak-ok: map-content-color
const ROUTE_ORANGE: Color32 = Color32::from_rgb(0xED, 0x8B, 0x00); // style-leak-ok: map-content-color
const ROUTE_BLUE: Color32 = Color32::from_rgb(0x00, 0x71, 0xBC); // style-leak-ok: map-content-color
const ROUTE_GREEN: Color32 = Color32::from_rgb(0x00, 0x88, 0x43); // style-leak-ok: map-content-color
const ROUTE_PURPLE: Color32 = Color32::from_rgb(0x80, 0x2D, 0x8E); // style-leak-ok: map-content-color

/// Retained complete MBTA snapshot plus local label preference.
#[derive(Debug, Clone, Default)]
pub struct TransitLayerState {
    /// Latest nearby set, absent before the adapter publishes.
    pub snapshot: Option<TransitSnapshot>,
    /// Whether bounded vehicle labels are visible.
    pub show_labels: bool,
}

impl TransitLayerState {
    /// Replace the previous full-dataset fold wholesale.
    pub fn fold(&mut self, snapshot: TransitSnapshot) {
        self.snapshot = Some(snapshot);
    }

    /// Feed age based on its server-generation stamp, not merely fetch success.
    #[must_use]
    pub fn age_ms(&self, now_ms: i64) -> Option<i64> {
        self.snapshot
            .as_ref()
            .map(|snapshot| now_ms.saturating_sub(snapshot.feed_generated_at_ms).max(0))
    }

    /// Whether three expected updates have been missed.
    #[must_use]
    pub fn stale(&self, now_ms: i64) -> bool {
        self.age_ms(now_ms)
            .is_some_and(|age| age > SNAPSHOT_STALE_AFTER_MS)
    }

    /// Required active-layer attribution.
    #[must_use]
    pub const fn attribution() -> &'static str {
        ATTRIBUTION
    }
}

/// Observable paint output for headless tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PaintStats {
    /// Nearby vehicle chevrons painted.
    pub vehicles: usize,
    /// Optional labels painted.
    pub labels: usize,
    /// Whether the honest age/no-data badge painted.
    pub badge: bool,
}

/// Paint route-aware bearing chevrons with exact occupancy state tinting.
pub fn paint_layer<F>(
    painter: &Painter,
    rect: Rect,
    layer: &TransitLayerState,
    now_ms: i64,
    mut project: F,
) -> PaintStats
where
    F: FnMut(f64, f64) -> Option<Pos2>,
{
    if !rect.is_finite() || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return PaintStats::default();
    }
    let mut stats = PaintStats::default();
    let feed_stale = layer.stale(now_ms);
    let marker_painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    if let Some(snapshot) = &layer.snapshot {
        for vehicle in &snapshot.vehicles {
            if !valid_coordinates(vehicle) {
                continue;
            }
            let age = now_ms.saturating_sub(vehicle.observed_at_ms).max(0);
            if age > VEHICLE_DROP_AFTER_MS {
                continue;
            }
            let Some(point) = project(vehicle.latitude, vehicle.longitude) else {
                continue;
            };
            if point.any_nan() || !rect.expand(16.0).contains(point) {
                continue;
            }
            let tone = if feed_stale || age > VEHICLE_GREY_AFTER_MS {
                Style::TEXT_DIM
            } else {
                occupancy_tone(vehicle)
            };
            paint_chevron(&marker_painter, point, vehicle.bearing_deg, tone);
            stats.vehicles += 1;
            if layer.show_labels {
                if let Some(label) = vehicle.label.as_deref().or(vehicle.route_id.as_deref()) {
                    marker_painter.text(
                        point + egui::vec2(10.0, 0.0),
                        Align2::LEFT_CENTER,
                        label,
                        FontId::proportional(Style::SMALL),
                        tone,
                    );
                    stats.labels += 1;
                }
            }
        }
    }
    paint_age_badge(painter, rect, layer, now_ms);
    stats.badge = true;
    stats
}

fn valid_coordinates(vehicle: &TransitVehicle) -> bool {
    vehicle.latitude.is_finite()
        && vehicle.longitude.is_finite()
        && (-90.0..=90.0).contains(&vehicle.latitude)
        && (-180.0..=180.0).contains(&vehicle.longitude)
}

fn paint_chevron(painter: &Painter, point: Pos2, bearing_deg: Option<f32>, tone: Color32) {
    let heading = bearing_deg
        .filter(|bearing| bearing.is_finite() && (0.0..360.0).contains(bearing))
        .unwrap_or(0.0)
        .to_radians();
    let rotate = |x: f32, y: f32| {
        egui::pos2(
            point.x + x * heading.cos() - y * heading.sin(),
            point.y + x * heading.sin() + y * heading.cos(),
        )
    };
    painter.add(Shape::convex_polygon(
        vec![rotate(0.0, -8.0), rotate(-5.5, 6.0), rotate(5.5, 6.0)],
        tone.gamma_multiply(0.82),
        Stroke::new(1.2, Color32::WHITE.gamma_multiply(0.85)),
    ));
}

fn occupancy_tone(vehicle: &TransitVehicle) -> Color32 {
    let route = route_tone(vehicle.route_id.as_deref());
    match vehicle.occupancy {
        Some(
            TransitOccupancy::Full
            | TransitOccupancy::NotAcceptingPassengers
            | TransitOccupancy::NotBoardable,
        ) => Style::DANGER,
        Some(TransitOccupancy::StandingRoomOnly | TransitOccupancy::CrushedStandingRoomOnly) => {
            Style::WARN
        }
        _ => {
            // GTFS explicitly permits >100 crush loads; clamp only the visual
            // strength while the typed wire value remains exact.
            let load = vehicle.occupancy_percentage.unwrap_or(50).min(150) as f32 / 150.0;
            route.gamma_multiply(0.65 + load * 0.35)
        }
    }
}

fn route_tone(route_id: Option<&str>) -> Color32 {
    let route = route_id.unwrap_or_default();
    if route.eq_ignore_ascii_case("Red") || route.eq_ignore_ascii_case("Mattapan") {
        ROUTE_RED
    } else if route.eq_ignore_ascii_case("Orange") {
        ROUTE_ORANGE
    } else if route.eq_ignore_ascii_case("Blue") {
        ROUTE_BLUE
    } else if route.starts_with("Green-") {
        ROUTE_GREEN
    } else if route.starts_with("CR-") {
        ROUTE_PURPLE
    } else {
        Style::ACCENT_HI
    }
}

fn paint_age_badge(painter: &Painter, rect: Rect, layer: &TransitLayerState, now_ms: i64) {
    let (label, tone) = match (&layer.snapshot, layer.age_ms(now_ms)) {
        (None, _) => ("MBTA transit · no data".to_string(), Style::TEXT_DIM),
        (Some(_), Some(age)) if age > SNAPSHOT_STALE_AFTER_MS => (
            format!("MBTA transit · STALE {}", age_label(age)),
            Style::WARN,
        ),
        (Some(snapshot), Some(age)) if !snapshot.gaps.is_empty() => (
            format!(
                "MBTA transit · {} · {} nearby · degraded",
                age_label(age),
                snapshot.vehicles.len()
            ),
            Style::WARN,
        ),
        (Some(snapshot), Some(age)) => (
            format!(
                "MBTA transit · {} · {} nearby",
                age_label(age),
                snapshot.vehicles.len()
            ),
            Style::TEXT,
        ),
        (Some(_), None) => ("MBTA transit · no timestamp".to_string(), Style::WARN),
    };
    let galley = painter.layout_no_wrap(label, FontId::proportional(Style::SMALL), tone);
    let pad = egui::vec2(Style::SP_S, Style::SP_XS);
    let row_height = galley.size().y + pad.y * 2.0 + Style::SP_XS;
    let badge = Rect::from_min_size(
        egui::pos2(
            rect.right() - galley.size().x - pad.x * 2.0 - Style::SP_S,
            rect.top() + Style::SP_S + row_height * 3.0,
        ),
        galley.size() + pad * 2.0,
    );
    painter.rect_filled(badge, Style::RADIUS_S, Style::BG.gamma_multiply(0.86));
    painter.rect_stroke(
        badge,
        Style::RADIUS_S,
        Stroke::new(1.0, tone.gamma_multiply(0.55)),
        egui::StrokeKind::Inside,
    );
    painter.galley(badge.left_top() + pad, galley, tone);
}

fn age_label(age_ms: i64) -> String {
    let seconds = age_ms.max(0) / 1_000;
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m", seconds / 60)
    }
}

#[cfg(test)]
mod tests {
    use mackes_mesh_types::transit::{TransitSnapshot, TransitStopStatus};

    use super::*;

    fn snapshot(now: i64) -> TransitSnapshot {
        let mut snapshot = TransitSnapshot::empty("rig-1", now, now, "2.0", 42.36, -71.06);
        snapshot.vehicles.push(TransitVehicle {
            id: "y1891".to_string(),
            label: Some("1891".to_string()),
            route_id: Some("22".to_string()),
            observed_at_ms: now,
            latitude: 42.36,
            longitude: -71.06,
            bearing_deg: Some(45.0),
            speed_mps: Some(10.0),
            occupancy: Some(TransitOccupancy::Full),
            occupancy_percentage: Some(160),
            stop_id: Some("334".to_string()),
            stop_status: Some(TransitStopStatus::StoppedAt),
        });
        snapshot
    }

    #[test]
    fn painter_labels_greys_and_drops_by_observation_age() {
        let now = 1_000_000;
        let mut layer = TransitLayerState::default();
        layer.show_labels = true;
        layer.fold(snapshot(now));
        let ctx = egui::Context::default();
        let mut stats = PaintStats::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                stats = paint_layer(ui.painter(), rect, &layer, now + 61_000, |_lat, _lon| {
                    Some(rect.center())
                });
            });
        });
        assert_eq!(stats.vehicles, 1);
        assert_eq!(stats.labels, 1);
        let mut dropped = PaintStats::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                dropped = paint_layer(
                    ui.painter(),
                    rect,
                    &layer,
                    now + VEHICLE_DROP_AFTER_MS + 1,
                    |_lat, _lon| Some(rect.center()),
                );
            });
        });
        assert_eq!(dropped.vehicles, 0);
        assert!(dropped.badge);
    }

    #[test]
    fn route_and_occupancy_tones_preserve_exact_crush_load_wire_value() {
        let vehicle = &snapshot(1).vehicles[0];
        assert_eq!(vehicle.occupancy_percentage, Some(160));
        assert_eq!(occupancy_tone(vehicle), Style::DANGER);
        assert_eq!(route_tone(Some("Green-C")), ROUTE_GREEN);
        assert_eq!(route_tone(Some("CR-Fitchburg")), ROUTE_PURPLE);
    }

    #[test]
    fn fold_is_wholesale_and_attribution_names_massdot() {
        let mut layer = TransitLayerState::default();
        layer.fold(snapshot(1));
        layer.fold(TransitSnapshot::empty("rig-1", 2, 2, "2.0", 42.0, -71.0));
        assert!(layer
            .snapshot
            .as_ref()
            .expect("snapshot")
            .vehicles
            .is_empty());
        assert!(TransitLayerState::attribution().contains("MassDOT"));
    }
}
