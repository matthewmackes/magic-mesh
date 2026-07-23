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
    let dimmed = layer.stale(now_ms) || layer.paused();
    let mut stats = PaintStats::default();
    let mut highest = None::<&AirQualityStation>;
    if !expired {
        if let Some(snapshot) = &layer.snapshot {
            let marker_painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
            for station in &snapshot.stations {
                let Some(point) = project(station.latitude, station.longitude) else {
                    continue;
                };
                if point.any_nan() || !rect.expand(18.0).contains(point) {
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
        station.aqi, station.parameter
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
                "AirNow AQI · {} · {} stations · degraded",
                age_label(age),
                snapshot.stations.len()
            ),
            Style::WARN,
        ),
        (Some(snapshot), Some(age)) => (
            format!(
                "AirNow AQI · {} · {} stations",
                age_label(age),
                snapshot.stations.len()
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
        layer.fold(AirQualitySnapshot::unconfigured("rig-1", 1));
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
