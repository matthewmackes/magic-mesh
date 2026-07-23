//! adsb.lol low-altitude aircraft layer model and painter.

use mackes_mesh_types::aircraft::{AircraftSnapshot, AircraftTrack, ATTRIBUTION};
use mde_egui::egui::{self, Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke};
use mde_egui::Style;

/// Five missed three-second polls make the layer visibly stale.
pub const SNAPSHOT_STALE_AFTER_MS: i64 = 15_000;
/// Hold the last qualified position fully visible for this long.
pub const TRACK_FADE_AFTER_MS: i64 = 30_000;
/// Stop projecting and remove the marker after this age.
pub const TRACK_DROP_AFTER_MS: i64 = 60_000;

const LOW_ALTITUDE: Color32 = Color32::from_rgb(0xF0, 0x66, 0x3A); // style-leak-ok: map-content-color
const HIGH_ALTITUDE: Color32 = Color32::from_rgb(0x47, 0xB6, 0xE8); // style-leak-ok: map-content-color
const EARTH_RADIUS_NM: f64 = 3_440.065;

/// Retained complete aircraft snapshot plus local label preference.
#[derive(Debug, Clone, Default)]
pub struct AircraftLayerState {
    /// Latest point-scoped set, absent before the adapter publishes.
    pub snapshot: Option<AircraftSnapshot>,
    /// Whether safe optional broadcast callsigns are painted.
    pub show_callsigns: bool,
}

impl AircraftLayerState {
    /// Replace the previous complete set wholesale.
    pub fn fold(&mut self, snapshot: AircraftSnapshot) {
        self.snapshot = Some(snapshot);
    }

    /// Snapshot age in milliseconds.
    #[must_use]
    pub fn age_ms(&self, now_ms: i64) -> Option<i64> {
        self.snapshot
            .as_ref()
            .map(|snapshot| now_ms.saturating_sub(snapshot.fetched_at_ms).max(0))
    }

    /// Whether the adapter has missed five visible-layer polls.
    #[must_use]
    pub fn stale(&self, now_ms: i64) -> bool {
        self.age_ms(now_ms)
            .is_some_and(|age| age > SNAPSHOT_STALE_AFTER_MS)
    }

    /// Required attribution whenever the toggle is active.
    #[must_use]
    pub const fn attribution() -> &'static str {
        ATTRIBUTION
    }
}

/// Observable paint facts for headless regression tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PaintStats {
    /// Live/fading aircraft markers painted in the viewport.
    pub markers: usize,
    /// Optional callsign labels painted.
    pub labels: usize,
    /// Whether the honest no-data/age badge painted.
    pub badge: bool,
}

/// Paint dead-reckoned, heading-rotated, altitude-tinted markers and freshness.
pub fn paint_layer<F>(
    painter: &Painter,
    rect: Rect,
    layer: &AircraftLayerState,
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
    let stale = layer.stale(now_ms);
    if let Some(snapshot) = &layer.snapshot {
        let marker_painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
        for aircraft in &snapshot.aircraft {
            if !aircraft.latitude.is_finite()
                || !aircraft.longitude.is_finite()
                || !(-90.0..=90.0).contains(&aircraft.latitude)
                || !(-180.0..=180.0).contains(&aircraft.longitude)
            {
                continue;
            }
            let age_ms = now_ms.saturating_sub(aircraft.observed_at_ms).max(0);
            if age_ms > TRACK_DROP_AFTER_MS {
                continue;
            }
            let (latitude, longitude) = dead_reckoned_position(aircraft, age_ms);
            let Some(point) = project(latitude, longitude) else {
                continue;
            };
            if point.any_nan() || !rect.expand(18.0).contains(point) {
                continue;
            }
            let alpha = track_alpha(age_ms);
            let tone = if stale {
                Style::TEXT_DIM
            } else {
                altitude_color(aircraft.estimated_agl_ft)
            }
            .gamma_multiply(alpha);
            paint_marker(&marker_painter, point, aircraft.track_deg, tone);
            stats.markers += 1;
            if layer.show_callsigns {
                if let Some(callsign) = aircraft.callsign.as_deref() {
                    marker_painter.text(
                        point + egui::vec2(10.0, 0.0),
                        Align2::LEFT_CENTER,
                        callsign,
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

fn paint_marker(painter: &Painter, point: Pos2, track_deg: Option<f32>, tone: Color32) {
    let heading = track_deg
        .filter(|heading| heading.is_finite() && (0.0..360.0).contains(heading))
        .unwrap_or(0.0)
        .to_radians();
    let rotate = |x: f32, y: f32| {
        egui::pos2(
            point.x + x * heading.cos() - y * heading.sin(),
            point.y + x * heading.sin() + y * heading.cos(),
        )
    };
    let nose = rotate(0.0, -9.0);
    let left = rotate(-6.0, 6.0);
    let right = rotate(6.0, 6.0);
    painter.add(Shape::convex_polygon(
        vec![nose, left, right],
        tone.gamma_multiply(0.78),
        Stroke::new(1.25, Color32::WHITE.gamma_multiply(tone.a() as f32 / 255.0)),
    ));
    painter.line_segment(
        [rotate(-3.5, 7.0), rotate(3.5, 7.0)],
        Stroke::new(1.5, tone),
    );
}

fn altitude_color(agl_ft: f32) -> Color32 {
    let amount = if agl_ft.is_finite() {
        (agl_ft / 3_000.0).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mix = |low: u8, high: u8| {
        (f32::from(low) + (f32::from(high) - f32::from(low)) * amount).round() as u8
    };
    let red = mix(LOW_ALTITUDE.r(), HIGH_ALTITUDE.r());
    let green = mix(LOW_ALTITUDE.g(), HIGH_ALTITUDE.g());
    let blue = mix(LOW_ALTITUDE.b(), HIGH_ALTITUDE.b());
    Color32::from_rgb(red, green, blue) // style-leak-ok: map-content-color
}

fn track_alpha(age_ms: i64) -> f32 {
    if age_ms <= TRACK_FADE_AFTER_MS {
        1.0
    } else {
        1.0 - ((age_ms - TRACK_FADE_AFTER_MS) as f32
            / (TRACK_DROP_AFTER_MS - TRACK_FADE_AFTER_MS) as f32)
            .clamp(0.0, 1.0)
    }
}

fn dead_reckoned_position(aircraft: &AircraftTrack, age_ms: i64) -> (f64, f64) {
    let Some(speed_kt) = aircraft
        .ground_speed_kt
        .filter(|speed| speed.is_finite() && (0.0..=750.0).contains(speed))
    else {
        return (aircraft.latitude, aircraft.longitude);
    };
    let Some(track_deg) = aircraft
        .track_deg
        .filter(|track| track.is_finite() && (0.0..360.0).contains(track))
    else {
        return (aircraft.latitude, aircraft.longitude);
    };
    let elapsed_s = age_ms.clamp(0, TRACK_DROP_AFTER_MS) as f64 / 1_000.0;
    let angular_distance = f64::from(speed_kt) * elapsed_s / 3_600.0 / EARTH_RADIUS_NM;
    let bearing = f64::from(track_deg).to_radians();
    let latitude = aircraft.latitude.to_radians();
    let longitude = aircraft.longitude.to_radians();
    let destination_latitude = (latitude.sin() * angular_distance.cos()
        + latitude.cos() * angular_distance.sin() * bearing.cos())
    .clamp(-1.0, 1.0)
    .asin();
    let destination_longitude = longitude
        + (bearing.sin() * angular_distance.sin() * latitude.cos())
            .atan2(angular_distance.cos() - latitude.sin() * destination_latitude.sin());
    (
        destination_latitude.to_degrees(),
        ((destination_longitude.to_degrees() + 540.0) % 360.0) - 180.0,
    )
}

fn paint_age_badge(painter: &Painter, rect: Rect, layer: &AircraftLayerState, now_ms: i64) {
    let (label, tone) = match (&layer.snapshot, layer.age_ms(now_ms)) {
        (None, _) => ("Aircraft · no data".to_string(), Style::TEXT_DIM),
        (Some(_), Some(age)) if age > SNAPSHOT_STALE_AFTER_MS => {
            (format!("Aircraft · STALE {}", age_label(age)), Style::WARN)
        }
        (Some(snapshot), Some(age)) if !snapshot.gaps.is_empty() => (
            format!(
                "Aircraft · {} · {} nearby · degraded",
                age_label(age),
                snapshot.aircraft.len()
            ),
            Style::WARN,
        ),
        (Some(snapshot), Some(age)) => (
            format!(
                "Aircraft · {} · {} nearby · {} quality filtered",
                age_label(age),
                snapshot.aircraft.len(),
                snapshot.quality_filtered
            ),
            Style::TEXT,
        ),
        (Some(_), None) => ("Aircraft · no timestamp".to_string(), Style::WARN),
    };
    let galley = painter.layout_no_wrap(label, FontId::proportional(Style::SMALL), tone);
    let pad = egui::vec2(Style::SP_S, Style::SP_XS);
    // Third overlay badge row, below USGS and NWS when all three are active.
    let row_height = galley.size().y + pad.y * 2.0 + Style::SP_XS;
    let badge = Rect::from_min_size(
        egui::pos2(
            rect.right() - galley.size().x - pad.x * 2.0 - Style::SP_S,
            rect.top() + Style::SP_S + row_height * 2.0,
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
    use mackes_mesh_types::aircraft::{AircraftPositionSource, AircraftTrack};

    use super::*;

    fn snapshot(now: i64) -> AircraftSnapshot {
        let mut snapshot = AircraftSnapshot::empty("rig-1", now, 40.7128, -74.006, 0.0);
        snapshot.aircraft.push(AircraftTrack {
            id: "aaacc3".to_string(),
            callsign: Some("N123AB".to_string()),
            observed_at_ms: now,
            latitude: 40.7128,
            longitude: -74.006,
            altitude_msl_ft: 425.0,
            estimated_agl_ft: 425.0,
            ground_speed_kt: Some(120.0),
            track_deg: Some(90.0),
            position_source: AircraftPositionSource::Adsb,
        });
        snapshot
    }

    #[test]
    fn eastbound_dead_reckoning_moves_longitude_without_large_latitude_change() {
        let aircraft = &snapshot(1_000_000).aircraft[0];
        let (latitude, longitude) = dead_reckoned_position(aircraft, 30_000);
        assert!((latitude - aircraft.latitude).abs() < 0.001);
        assert!(longitude > aircraft.longitude);
        assert_eq!(track_alpha(TRACK_FADE_AFTER_MS), 1.0);
        assert_eq!(track_alpha(TRACK_DROP_AFTER_MS), 0.0);
    }

    #[test]
    fn painter_rotates_tints_labels_fades_and_drops() {
        let now = 1_000_000;
        let mut layer = AircraftLayerState::default();
        layer.show_callsigns = true;
        layer.fold(snapshot(now));
        let ctx = egui::Context::default();
        let mut visible = PaintStats::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                visible = paint_layer(ui.painter(), rect, &layer, now + 31_000, |_lat, _lon| {
                    Some(rect.center())
                });
            });
        });
        assert_eq!(visible.markers, 1);
        assert_eq!(visible.labels, 1);
        assert!(visible.badge);
        assert!(ctx.tessellate(output.shapes, output.pixels_per_point).len() >= 3);

        let mut dropped = PaintStats::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                dropped = paint_layer(
                    ui.painter(),
                    rect,
                    &layer,
                    now + TRACK_DROP_AFTER_MS + 1,
                    |_lat, _lon| Some(rect.center()),
                );
            });
        });
        assert_eq!(dropped.markers, 0);
        assert!(dropped.badge);
    }

    #[test]
    fn fold_replaces_whole_set_and_attribution_is_odbl() {
        let mut layer = AircraftLayerState::default();
        layer.fold(snapshot(1_000));
        layer.fold(AircraftSnapshot::empty("rig-1", 2_000, 40.0, -74.0, 0.0));
        assert!(layer
            .snapshot
            .as_ref()
            .expect("snapshot")
            .aircraft
            .is_empty());
        assert!(AircraftLayerState::attribution().contains("ODbL"));
        assert!(layer.stale(2_000 + SNAPSHOT_STALE_AFTER_MS + 1));
    }
}
