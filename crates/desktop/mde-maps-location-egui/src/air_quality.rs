//! Credential-gated US EPA AirNow AQI model and painter (OVERLAY-7).

use mackes_mesh_types::air_quality::{
    AirNowAvailability, AirQualitySnapshot, AirQualityStation, ATTRIBUTION,
};
use mde_egui::egui::{self, Align2, Color32, FontId, Painter, Pos2, Rect, Stroke};
use mde_egui::Style;

/// AirNow updates hourly; observations become visibly stale after two hours.
pub const SNAPSHOT_STALE_AFTER_MS: i64 = 2 * 60 * 60 * 1_000;
/// Observations older than six hours are removed instead of painted as current.
pub const SNAPSHOT_DROP_AFTER_MS: i64 = 6 * 60 * 60 * 1_000;
/// Maximum amount of retained Bus data the UI will inspect or paint per frame.
/// This mirrors the adapter's retained-station cap, but is still required at
/// this untrusted consumer boundary because a peer can publish malformed data.
const MAX_PAINTABLE_STATIONS: usize = 256;
/// Small clock skew tolerated when checking externally supplied timestamps.
const MAX_TIMESTAMP_FUTURE_SKEW_MS: i64 = 5_000;
/// AirNow permits an observation to lead its fetch by at most one hour.
const MAX_OBSERVATION_FUTURE_SKEW_MS: i64 = 60 * 60 * 1_000;
/// Keep the UI's station timestamp contract aligned with the adapter.
const MAX_OBSERVATION_AGE_MS: i64 = SNAPSHOT_DROP_AFTER_MS;
const MAX_PARAMETER_LABEL_CHARS: usize = 32;

const AQI_GOOD: Color32 = Color32::from_rgb(0x00, 0xE4, 0x00); // style-leak-ok: map-content-color
const AQI_MODERATE: Color32 = Color32::from_rgb(0xFF, 0xFF, 0x00); // style-leak-ok: map-content-color
const AQI_SENSITIVE: Color32 = Color32::from_rgb(0xFF, 0x7E, 0x00); // style-leak-ok: map-content-color
const AQI_UNHEALTHY: Color32 = Color32::from_rgb(0xFF, 0x00, 0x00); // style-leak-ok: map-content-color
const AQI_VERY_UNHEALTHY: Color32 = Color32::from_rgb(0x8F, 0x3F, 0x97); // style-leak-ok: map-content-color
const AQI_HAZARDOUS: Color32 = Color32::from_rgb(0x7E, 0x00, 0x23); // style-leak-ok: map-content-color

/// Retained complete nearby AirNow station set.
#[derive(Debug, Clone, Default)]
pub struct AirQualityLayerState {
    /// Latest adapter status/snapshot.
    pub snapshot: Option<AirQualitySnapshot>,
}

impl AirQualityLayerState {
    /// Replace the prior current set wholesale.
    pub fn fold(&mut self, snapshot: AirQualitySnapshot) {
        self.snapshot = Some(snapshot);
    }

    /// Age since the last successful keyed fetch.
    #[must_use]
    pub fn age_ms(&self, now_ms: i64) -> Option<i64> {
        self.snapshot
            .as_ref()?
            .fetched_at_ms
            .map(|fetched| now_ms.saturating_sub(fetched).max(0))
    }

    /// Whether a retained snapshot carries a timestamp too far in the future
    /// to be presented as current.  A future timestamp otherwise becomes age
    /// zero and can create a false-success overlay indefinitely.
    #[must_use]
    pub fn future_dated(&self, now_ms: i64) -> bool {
        let Some(snapshot) = &self.snapshot else {
            return false;
        };
        let latest_credible = now_ms.saturating_add(MAX_TIMESTAMP_FUTURE_SKEW_MS);
        snapshot.published_at_ms > latest_credible
            || snapshot.fetched_at_ms.is_some_and(|fetched| {
                fetched > latest_credible
                    || fetched
                        > snapshot
                            .published_at_ms
                            .saturating_add(MAX_TIMESTAMP_FUTURE_SKEW_MS)
            })
    }

    /// Whether observations are older than two hours.
    #[must_use]
    pub fn stale(&self, now_ms: i64) -> bool {
        self.age_ms(now_ms)
            .is_some_and(|age| age > SNAPSHOT_STALE_AFTER_MS)
    }

    /// Whether the last keyed refresh failed or the fresh vehicle fix vanished.
    #[must_use]
    pub fn paused(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .gaps
                .iter()
                .any(|gap| gap.starts_with("AirNow AQI paused:"))
        })
    }

    /// Whether the free API key is still missing.
    #[must_use]
    pub fn unconfigured(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.availability == AirNowAvailability::Unconfigured)
    }

    /// Required active-layer attribution.
    #[must_use]
    pub const fn attribution() -> &'static str {
        ATTRIBUTION
    }
}

/// Headless-observable paint facts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PaintStats {
    /// Current projected station markers painted.
    pub markers: usize,
    /// Whether a fresh >=150 AQI banner painted.
    pub alert_banner: bool,
    /// Whether the honest config/age/no-data badge painted.
    pub badge: bool,
}

/// Paint AirNow station circles, a high-AQI banner, and one status badge.
pub fn paint_layer<F>(
    painter: &Painter,
    rect: Rect,
    layer: &AirQualityLayerState,
    now_ms: i64,
    mut project: F,
) -> PaintStats
where
    F: FnMut(f64, f64) -> Option<Pos2>,
{
    if !rect.is_finite() || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return PaintStats::default();
    }
    let age = layer.age_ms(now_ms);
    let expired = age.is_some_and(|age| age > SNAPSHOT_DROP_AFTER_MS);
    let future_dated = layer.future_dated(now_ms);
    let dimmed = layer.stale(now_ms) || layer.paused() || future_dated;
    let mut stats = PaintStats::default();
    let mut highest = None::<&AirQualityStation>;
    if !expired && !future_dated {
        if let Some(snapshot) = &layer.snapshot {
            // A station list is only paintable after a keyed AirNow fetch.  In
            // particular, do not trust a malformed retained payload that
            // carries stations alongside an explicit unconfigured or secret
            // store error state.
            let ready = snapshot.availability == AirNowAvailability::Ready
                && snapshot.fetched_at_ms.is_some();
            if !ready {
                paint_status_badge(painter, rect, layer, now_ms, expired);
                return PaintStats {
                    badge: true,
                    ..stats
                };
            }
            let fetched_at_ms = snapshot
                .fetched_at_ms
                .expect("ready AirNow snapshot has a fetch timestamp");
            let marker_painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
            for station in snapshot.stations.iter().take(MAX_PAINTABLE_STATIONS) {
                // The worker rejects these values, but the shared Bus is an
                // untrusted boundary.  Keep a malformed Ready envelope from
                // feeding invalid coordinates or an out-of-contract AQI into
                // the projection and painter.
                if !station_is_paintable(station, fetched_at_ms, now_ms) {
                    continue;
                }
                let Some(point) = project(station.latitude, station.longitude) else {
                    continue;
                };
                if !point.x.is_finite()
                    || !point.y.is_finite()
                    || !rect.expand(18.0).contains(point)
                {
                    continue;
                }
                paint_marker(&marker_painter, point, station, dimmed);
                stats.markers += 1;
                if highest.is_none_or(|current| station.aqi > current.aqi) {
                    highest = Some(station);
                }
            }
        }
    }
    if !dimmed {
        if let Some(station) = highest.filter(|station| station.aqi >= 150) {
            paint_alert_banner(painter, rect, station);
            stats.alert_banner = true;
        }
    }
    paint_status_badge(painter, rect, layer, now_ms, expired);
    stats.badge = true;
    stats
}

fn station_is_paintable(station: &AirQualityStation, fetched_at_ms: i64, now_ms: i64) -> bool {
    let latest_credible = now_ms.saturating_add(MAX_TIMESTAMP_FUTURE_SKEW_MS);
    station.aqi <= 500
        && station.latitude.is_finite()
        && (-90.0..=90.0).contains(&station.latitude)
        && station.longitude.is_finite()
        && (-180.0..=180.0).contains(&station.longitude)
        && station.distance_km.is_finite()
        && station.distance_km >= 0.0
        && station.observed_at_ms <= latest_credible
        && station.observed_at_ms <= fetched_at_ms.saturating_add(MAX_OBSERVATION_FUTURE_SKEW_MS)
        && fetched_at_ms.saturating_sub(station.observed_at_ms) <= MAX_OBSERVATION_AGE_MS
}

fn station_count_label(snapshot: &AirQualitySnapshot, now_ms: i64) -> String {
    let count = snapshot.fetched_at_ms.map_or(0, |fetched_at_ms| {
        snapshot
            .stations
            .iter()
            .take(MAX_PAINTABLE_STATIONS)
            .filter(|station| station_is_paintable(station, fetched_at_ms, now_ms))
            .count()
    });
    if snapshot.stations.len() > MAX_PAINTABLE_STATIONS {
        format!("{count}+ stations · capped")
    } else {
        format!("{count} stations")
    }
}

fn bounded_parameter_label(parameter: &str) -> String {
    let mut chars = parameter.chars();
    let mut label = chars
        .by_ref()
        .take(MAX_PARAMETER_LABEL_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        label.push('…');
    }
    label
}

fn paint_marker(painter: &Painter, point: Pos2, station: &AirQualityStation, dimmed: bool) {
    let tone = if dimmed {
        Style::TEXT_DIM
    } else {
        aqi_color(station.aqi)
    };
    let alpha = if dimmed { 0.38 } else { 0.88 };
    let radius = 6.0 + (f32::from(station.aqi.min(300)) / 300.0) * 4.0;
    painter.circle_filled(point, radius, tone.gamma_multiply(alpha));
    painter.circle_stroke(
        point,
        radius,
        Stroke::new(1.25, Color32::WHITE.gamma_multiply(alpha)),
    );
    painter.text(
        point,
        Align2::CENTER_CENTER,
        station.aqi,
        FontId::proportional(8.0),
        if dimmed { Style::BG } else { Color32::BLACK },
    );
}

fn paint_alert_banner(painter: &Painter, rect: Rect, station: &AirQualityStation) {
    let label = format!(
        "AirNow air quality alert · AQI {} · {}",
        station.aqi,
        bounded_parameter_label(&station.parameter)
    );
    let galley = painter.layout_no_wrap(label, FontId::proportional(Style::BODY), Color32::WHITE);
    let pad = egui::vec2(Style::SP_M, Style::SP_S);
    let banner = Rect::from_center_size(
        egui::pos2(rect.center().x, rect.top() + Style::SP_XL * 1.5),
        galley.size() + pad * 2.0,
    )
    .intersect(rect);
    painter.rect_filled(
        banner,
        Style::RADIUS_M,
        aqi_color(station.aqi).gamma_multiply(0.92),
    );
    painter.rect_stroke(
        banner,
        Style::RADIUS_M,
        Stroke::new(1.5, Color32::WHITE.gamma_multiply(0.72)),
        egui::StrokeKind::Inside,
    );
    painter.galley(
        banner.center() - galley.size() * 0.5,
        galley,
        Color32::WHITE,
    );
}

fn paint_status_badge(
    painter: &Painter,
    rect: Rect,
    layer: &AirQualityLayerState,
    now_ms: i64,
    expired: bool,
) {
    let (label, tone) = match (&layer.snapshot, layer.age_ms(now_ms)) {
        (None, _) => ("AirNow AQI · no data".to_string(), Style::TEXT_DIM),
        (Some(_), _) if layer.future_dated(now_ms) => (
            "AirNow AQI · invalid future timestamp".to_string(),
            Style::DANGER,
        ),
        (Some(snapshot), _) if snapshot.availability == AirNowAvailability::Unconfigured => (
            "AirNow AQI · API key not configured".to_string(),
            Style::WARN,
        ),
        (Some(snapshot), _) if snapshot.availability == AirNowAvailability::SecretStoreError => (
            "AirNow AQI · secret store unavailable".to_string(),
            Style::DANGER,
        ),
        (Some(_), Some(age)) if expired => (
            format!("AirNow AQI · EXPIRED {}", age_label(age)),
            Style::DANGER,
        ),
        (Some(_), Some(age)) if layer.paused() => (
            format!("AirNow AQI · PAUSED · {}", age_label(age)),
            Style::WARN,
        ),
        (Some(_), Some(age)) if age > SNAPSHOT_STALE_AFTER_MS => (
            format!("AirNow AQI · STALE {}", age_label(age)),
            Style::WARN,
        ),
        (Some(snapshot), Some(age)) if !snapshot.gaps.is_empty() => (
            format!(
                "AirNow AQI · {} · {} · degraded",
                age_label(age),
                station_count_label(snapshot, now_ms)
            ),
            Style::WARN,
        ),
        (Some(snapshot), Some(age)) => (
            format!(
                "AirNow AQI · {} · {}",
                age_label(age),
                station_count_label(snapshot, now_ms)
            ),
            Style::TEXT,
        ),
        (Some(_), None) => (
            "AirNow AQI · awaiting first fetch".to_string(),
            Style::TEXT_DIM,
        ),
    };
    let galley = painter.layout_no_wrap(label, FontId::proportional(Style::SMALL), tone);
    let pad = egui::vec2(Style::SP_S, Style::SP_XS);
    let row_height = galley.size().y + pad.y * 2.0 + Style::SP_XS;
    let badge = Rect::from_min_size(
        egui::pos2(
            rect.right() - galley.size().x - pad.x * 2.0 - Style::SP_S,
            rect.top() + Style::SP_S + row_height * 9.0,
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

fn aqi_color(aqi: u16) -> Color32 {
    match aqi {
        0..=50 => AQI_GOOD,
        51..=100 => AQI_MODERATE,
        101..=150 => AQI_SENSITIVE,
        151..=200 => AQI_UNHEALTHY,
        201..=300 => AQI_VERY_UNHEALTHY,
        _ => AQI_HAZARDOUS,
    }
}

fn age_label(age_ms: i64) -> String {
    let minutes = age_ms.max(0) / 60_000;
    if minutes < 60 {
        format!("{minutes}m")
    } else {
        format!("{}h", minutes / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(now: i64, aqi: u16) -> AirQualitySnapshot {
        let mut snapshot = AirQualitySnapshot::empty("rig-1", now, now, 35.78, -78.64, 100);
        snapshot.stations.push(AirQualityStation {
            id: "840371830014".to_string(),
            name: Some("Millbrook School".to_string()),
            parameter: "PM2.5".to_string(),
            aqi,
            latitude: 35.7829,
            longitude: -78.5742,
            distance_km: 6.0,
            observed_at_ms: now - 20 * 60_000,
        });
        snapshot
    }

    #[test]
    fn fresh_high_aqi_paints_marker_banner_and_badge() {
        let now = 1_000_000_000;
        let mut layer = AirQualityLayerState::default();
        layer.fold(snapshot(now, 156));
        let ctx = egui::Context::default();
        let mut stats = PaintStats::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                stats = paint_layer(ui.painter(), rect, &layer, now, |_lat, _lon| {
                    Some(rect.center())
                });
            });
        });
        assert_eq!(stats.markers, 1);
        assert!(stats.alert_banner);
        assert!(stats.badge);
        assert!(ctx.tessellate(output.shapes, output.pixels_per_point).len() >= 3);
    }

    #[test]
    fn stale_data_loses_banner_and_expired_data_loses_markers() {
        let now = 1_000_000_000;
        let mut layer = AirQualityLayerState::default();
        layer.fold(snapshot(now, 200));
        let ctx = egui::Context::default();
        for (age, expected_markers) in [
            (SNAPSHOT_STALE_AFTER_MS + 1, 1),
            (SNAPSHOT_DROP_AFTER_MS + 1, 0),
        ] {
            let mut stats = PaintStats::default();
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let rect = ui.max_rect();
                    stats = paint_layer(ui.painter(), rect, &layer, now + age, |_lat, _lon| {
                        Some(rect.center())
                    });
                });
            });
            assert_eq!(stats.markers, expected_markers);
            assert!(!stats.alert_banner);
            assert!(stats.badge);
        }
    }

    #[test]
    fn unconfigured_state_is_explicit_and_never_paints_markers() {
        let mut layer = AirQualityLayerState::default();
        let mut snapshot = AirQualitySnapshot::unconfigured("rig-1", 1);
        // A malformed or stale retained body must not turn an explicit missing
        // key state into painted AQI observations.
        snapshot.stations.push(AirQualityStation {
            id: "unexpected".to_string(),
            name: None,
            parameter: "PM2.5".to_string(),
            aqi: 180,
            latitude: 35.78,
            longitude: -78.64,
            distance_km: 1.0,
            observed_at_ms: 1,
        });
        layer.fold(snapshot);
        assert!(layer.unconfigured());
        let ctx = egui::Context::default();
        let mut stats = PaintStats::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                stats = paint_layer(ui.painter(), ui.max_rect(), &layer, 2, |_lat, _lon| None);
            });
        });
        assert_eq!(stats.markers, 0);
        assert!(!stats.alert_banner);
        assert!(stats.badge);
        let retained = layer.snapshot.as_ref().expect("unconfigured snapshot");
        assert_eq!(retained.fetched_at_ms, None);
        assert!(retained.query_latitude.is_none());
        assert!(retained.query_longitude.is_none());
    }

    #[test]
    fn secret_store_error_never_paints_retained_stations() {
        let now = 1_000_000_000;
        let mut snapshot = snapshot(now, 180);
        snapshot.availability = AirNowAvailability::SecretStoreError;
        snapshot
            .gaps
            .push("AirNow secret store unavailable".to_string());
        let mut layer = AirQualityLayerState::default();
        layer.fold(snapshot);
        let ctx = egui::Context::default();
        let mut stats = PaintStats::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                stats = paint_layer(ui.painter(), rect, &layer, now, |_lat, _lon| {
                    Some(rect.center())
                });
            });
        });
        assert_eq!(stats.markers, 0);
        assert!(!stats.alert_banner);
        assert!(stats.badge);
    }

    #[test]
    fn malformed_ready_stations_are_not_projected_or_painted() {
        let now = 1_000_000_000;
        let mut snapshot = snapshot(now, 80);
        snapshot.stations.extend([
            AirQualityStation {
                id: "nan-latitude".to_string(),
                name: None,
                parameter: "PM2.5".to_string(),
                aqi: 500,
                latitude: f64::NAN,
                longitude: -78.64,
                distance_km: 1.0,
                observed_at_ms: now,
            },
            AirQualityStation {
                id: "out-of-range-longitude".to_string(),
                name: None,
                parameter: "PM2.5".to_string(),
                aqi: 500,
                latitude: 35.78,
                longitude: 181.0,
                distance_km: 1.0,
                observed_at_ms: now,
            },
            AirQualityStation {
                id: "non-finite-distance".to_string(),
                name: None,
                parameter: "PM2.5".to_string(),
                aqi: 500,
                latitude: 35.78,
                longitude: -78.64,
                distance_km: f32::INFINITY,
                observed_at_ms: now,
            },
            AirQualityStation {
                id: "out-of-contract-aqi".to_string(),
                name: None,
                parameter: "PM2.5".to_string(),
                aqi: 501,
                latitude: 35.78,
                longitude: -78.64,
                distance_km: 1.0,
                observed_at_ms: now,
            },
        ]);
        let mut layer = AirQualityLayerState::default();
        layer.fold(snapshot);
        let ctx = egui::Context::default();
        let mut projected = 0;
        let mut stats = PaintStats::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                stats = paint_layer(ui.painter(), rect, &layer, now, |_lat, _lon| {
                    projected += 1;
                    Some(rect.center())
                });
            });
        });
        assert_eq!(
            projected, 1,
            "only the validated station reaches projection"
        );
        assert_eq!(stats.markers, 1);
        assert!(
            !stats.alert_banner,
            "rejected AQI must not drive the banner"
        );
        assert!(stats.badge);
    }

    #[test]
    fn station_iteration_is_bounded_at_the_consumer_boundary() {
        let now = 1_000_000_000;
        let mut snapshot = AirQualitySnapshot::empty("rig-1", now, now, 35.78, -78.64, 100);
        snapshot.stations = (0..MAX_PAINTABLE_STATIONS + 64)
            .map(|index| AirQualityStation {
                id: format!("station-{index}"),
                name: None,
                parameter: "PM2.5".to_string(),
                aqi: 40,
                latitude: 35.0 + index as f64 * 0.0001,
                longitude: -78.0,
                distance_km: 1.0,
                observed_at_ms: now - 20 * 60_000,
            })
            .collect();
        assert_eq!(
            station_count_label(&snapshot, now),
            "256+ stations · capped"
        );

        let mut layer = AirQualityLayerState::default();
        layer.fold(snapshot);
        let ctx = egui::Context::default();
        let mut projected = 0;
        let mut stats = PaintStats::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                stats = paint_layer(ui.painter(), rect, &layer, now, |_lat, _lon| {
                    projected += 1;
                    Some(rect.center())
                });
            });
        });
        assert_eq!(projected, MAX_PAINTABLE_STATIONS);
        assert_eq!(stats.markers, MAX_PAINTABLE_STATIONS);
        assert!(!stats.alert_banner);
    }

    #[test]
    fn future_dated_observation_is_not_presented_as_current() {
        let now = 1_000_000_000;
        let mut snapshot = snapshot(now, 200);
        snapshot.stations[0].observed_at_ms = now + MAX_TIMESTAMP_FUTURE_SKEW_MS + 1;
        let mut layer = AirQualityLayerState::default();
        layer.fold(snapshot);
        let ctx = egui::Context::default();
        let mut projected = 0;
        let mut stats = PaintStats::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                stats = paint_layer(ui.painter(), rect, &layer, now, |_lat, _lon| {
                    projected += 1;
                    Some(rect.center())
                });
            });
        });
        assert_eq!(projected, 0);
        assert_eq!(stats.markers, 0);
        assert!(!stats.alert_banner);
        assert!(stats.badge);
    }

    #[test]
    fn future_dated_snapshot_is_not_presented_as_fresh() {
        let now = 1_000_000_000;
        let future = now + MAX_TIMESTAMP_FUTURE_SKEW_MS + 1;
        let mut snapshot = snapshot(now, 200);
        snapshot.published_at_ms = future;
        snapshot.fetched_at_ms = Some(future);
        let mut layer = AirQualityLayerState::default();
        layer.fold(snapshot);
        assert!(layer.future_dated(now));
        let ctx = egui::Context::default();
        let mut stats = PaintStats::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                stats = paint_layer(ui.painter(), rect, &layer, now, |_lat, _lon| {
                    Some(rect.center())
                });
            });
        });
        assert_eq!(stats.markers, 0);
        assert!(!stats.alert_banner);
        assert!(stats.badge);
    }

    #[test]
    fn invalid_viewport_is_a_noop() {
        let ctx = egui::Context::default();
        let mut stats = PaintStats {
            markers: 1,
            alert_banner: true,
            badge: true,
        };
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            stats = paint_layer(
                &ctx.layer_painter(egui::LayerId::background()),
                Rect::from_min_max(Pos2::ZERO, Pos2::ZERO),
                &AirQualityLayerState::default(),
                0,
                |_lat, _lon| Some(Pos2::new(f32::NAN, f32::NAN)),
            );
        });
        assert_eq!(stats, PaintStats::default());
    }
}
