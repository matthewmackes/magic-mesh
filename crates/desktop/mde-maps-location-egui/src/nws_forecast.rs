//! NWS hourly current/drive-ahead forecast layer and painter.

use mackes_mesh_types::nws_forecast::{
    ForecastKind, ForecastPeriod, ForecastSample, NwsForecastSnapshot, ATTRIBUTION,
};
use mde_egui::egui::{self, Align2, Color32, FontId, Painter, Pos2, Rect, Stroke};
use mde_egui::Style;

/// Hourly guidance is visibly stale after ninety minutes.
pub const SNAPSHOT_STALE_AFTER_MS: i64 = 90 * 60 * 1_000;

/// Retained complete NWS hourly snapshot.
#[derive(Debug, Clone, Default)]
pub struct NwsForecastLayerState {
    /// Latest snapshot, including explicit no-fix state.
    pub snapshot: Option<NwsForecastSnapshot>,
}

impl NwsForecastLayerState {
    /// Replace the prior current/drive-ahead sample set wholesale.
    pub fn fold(&mut self, snapshot: NwsForecastSnapshot) {
        self.snapshot = Some(snapshot);
    }

    /// Feed age derived from NWS `generatedAt`, never merely local fetch success.
    #[must_use]
    pub fn age_ms(&self, now_ms: i64) -> Option<i64> {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.feed_generated_at_ms)
            .map(|generated| now_ms.saturating_sub(generated).max(0))
    }

    /// Whether the producer has missed the honest hourly freshness window.
    #[must_use]
    pub fn stale(&self, now_ms: i64) -> bool {
        self.age_ms(now_ms)
            .is_some_and(|age| age > SNAPSHOT_STALE_AFTER_MS)
    }

    /// Whether the worker retained an older snapshot after losing its fresh fix
    /// or failing a refresh. Paused route markers must never look live.
    #[must_use]
    pub fn paused(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .gaps
                .iter()
                .any(|gap| gap.starts_with("NWS forecast paused:"))
        })
    }

    /// Required active-layer attribution.
    #[must_use]
    pub const fn attribution() -> &'static str {
        ATTRIBUTION
    }
}

/// Observable painter facts used by headless tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PaintStats {
    /// Current/drive-ahead sample glyphs painted.
    pub markers: usize,
    /// Whether the honest state badge painted.
    pub badge: bool,
    /// Whether every painted marker was forced to the non-live tone.
    pub non_live: bool,
}

/// Paint current/drive-ahead weather glyphs selected for each sample's ETA.
pub fn paint_layer<F>(
    painter: &Painter,
    rect: Rect,
    layer: &NwsForecastLayerState,
    now_ms: i64,
    mut project: F,
) -> PaintStats
where
    F: FnMut(f64, f64) -> Option<Pos2>,
{
    if !rect.is_finite() || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return PaintStats::default();
    }
    let non_live = layer.stale(now_ms) || layer.paused();
    let marker_painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    let mut stats = PaintStats::default();
    if let Some(snapshot) = &layer.snapshot {
        for sample in &snapshot.samples {
            let Some(period) = period_for_eta(sample, now_ms) else {
                continue;
            };
            let Some(point) = project(sample.latitude, sample.longitude) else {
                continue;
            };
            if point.any_nan() || !rect.expand(20.0).contains(point) {
                continue;
            }
            paint_marker(&marker_painter, point, sample, period, non_live);
            stats.markers += 1;
        }
    }
    paint_age_badge(painter, rect, layer, now_ms);
    stats.badge = true;
    stats.non_live = non_live;
    stats
}

fn period_for_eta(sample: &ForecastSample, now_ms: i64) -> Option<&ForecastPeriod> {
    sample
        .periods
        .iter()
        .find(|period| {
            period.end_at_ms > now_ms
                && period.start_at_ms <= sample.eta_at_ms
                && sample.eta_at_ms < period.end_at_ms
        })
        .or_else(|| {
            sample
                .periods
                .iter()
                .find(|period| period.end_at_ms > now_ms && period.start_at_ms > sample.eta_at_ms)
        })
}

fn paint_marker(
    painter: &Painter,
    point: Pos2,
    sample: &ForecastSample,
    period: &ForecastPeriod,
    stale: bool,
) {
    let tone = if stale {
        Style::TEXT_DIM
    } else {
        forecast_tone(period.kind)
    };
    let radius = if sample.distance_ahead_km == 0.0 {
        14.0
    } else {
        12.0
    };
    painter.circle_filled(point, radius + 4.0, tone.gamma_multiply(0.16));
    painter.circle_filled(point, radius, Style::BG.gamma_multiply(0.93));
    painter.circle_stroke(point, radius, Stroke::new(1.5, tone));
    painter.text(
        point,
        Align2::CENTER_CENTER,
        forecast_label(period.kind),
        FontId::proportional(Style::SMALL),
        tone,
    );
    painter.text(
        point + egui::vec2(0.0, radius + Style::SP_XS),
        Align2::CENTER_TOP,
        format!("{}°{}", period.temperature, period.temperature_unit),
        FontId::proportional(Style::SMALL),
        tone,
    );
}

fn forecast_tone(kind: ForecastKind) -> Color32 {
    match kind {
        ForecastKind::Thunderstorm => Style::DANGER,
        ForecastKind::Rain => Style::ACCENT_HI,
        ForecastKind::Wintry => Style::ACCENT,
        ForecastKind::LowVisibility | ForecastKind::Wind => Style::WARN,
        ForecastKind::Clear => Style::OK,
        ForecastKind::Cloudy => Style::TEXT,
        ForecastKind::Unknown => Style::TEXT_DIM,
    }
}

fn forecast_label(kind: ForecastKind) -> &'static str {
    match kind {
        ForecastKind::Thunderstorm => "TS",
        ForecastKind::Rain => "RAIN",
        ForecastKind::Wintry => "ICE",
        ForecastKind::LowVisibility => "FOG",
        ForecastKind::Wind => "WIND",
        ForecastKind::Clear => "CLR",
        ForecastKind::Cloudy => "CLD",
        ForecastKind::Unknown => "WX",
    }
}

fn paint_age_badge(painter: &Painter, rect: Rect, layer: &NwsForecastLayerState, now_ms: i64) {
    let (label, tone) = match (&layer.snapshot, layer.age_ms(now_ms)) {
        (None, _) => ("NWS hourly · no data".to_string(), Style::TEXT_DIM),
        (Some(snapshot), None) if snapshot.fetched_at_ms == 0 => {
            ("NWS hourly · no fresh vehicle fix".to_string(), Style::WARN)
        }
        (Some(_), None) => ("NWS hourly · no producer time".to_string(), Style::WARN),
        (Some(_), Some(age)) if age > SNAPSHOT_STALE_AFTER_MS => (
            format!("NWS hourly · STALE {}", age_label(age)),
            Style::WARN,
        ),
        (Some(snapshot), Some(age)) if !snapshot.gaps.is_empty() => (
            format!(
                "NWS hourly · {} · {} points · degraded",
                age_label(age),
                snapshot.samples.len()
            ),
            Style::WARN,
        ),
        (Some(snapshot), Some(age)) => (
            format!(
                "NWS hourly · {} · {} points",
                age_label(age),
                snapshot.samples.len()
            ),
            Style::TEXT,
        ),
    };
    let galley = painter.layout_no_wrap(label, FontId::proportional(Style::SMALL), tone);
    let pad = egui::vec2(Style::SP_S, Style::SP_XS);
    let row_height = galley.size().y + pad.y * 2.0 + Style::SP_XS;
    let badge = Rect::from_min_size(
        egui::pos2(
            rect.right() - galley.size().x - pad.x * 2.0 - Style::SP_S,
            rect.top() + Style::SP_S + row_height * 4.0,
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

    fn snapshot(now_ms: i64) -> NwsForecastSnapshot {
        let mut snapshot = NwsForecastSnapshot::empty("rig-1", now_ms, 42.36, -71.06);
        snapshot.feed_generated_at_ms = Some(now_ms);
        snapshot.samples.push(ForecastSample {
            distance_ahead_km: 25.0,
            eta_at_ms: now_ms + 30 * 60_000,
            latitude: 42.36,
            longitude: -71.06,
            grid_id: "BOX".to_string(),
            grid_x: 71,
            grid_y: 101,
            periods: vec![ForecastPeriod {
                number: 1,
                start_at_ms: now_ms,
                end_at_ms: now_ms + 60 * 60_000,
                is_daytime: true,
                temperature: 83,
                temperature_unit: "F".to_string(),
                precipitation_percent: Some(27),
                humidity_percent: Some(65),
                wind_speed: "9 mph".to_string(),
                wind_direction: "W".to_string(),
                short_forecast: "Thunderstorms".to_string(),
                kind: ForecastKind::Thunderstorm,
            }],
        });
        snapshot
    }

    #[test]
    fn period_selection_expires_and_never_reuses_past_guidance() {
        let now = 1_000_000;
        let snapshot = snapshot(now);
        let sample = &snapshot.samples[0];
        assert!(period_for_eta(sample, now).is_some());
        assert!(period_for_eta(sample, now + 60 * 60_000 + 1).is_none());
    }

    #[test]
    fn painter_marks_forecast_and_reports_explicit_no_fix() {
        let now = 1_000_000;
        let mut layer = NwsForecastLayerState::default();
        layer.fold(snapshot(now));
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
        assert_eq!(stats.markers, 1);
        assert!(stats.badge);
        assert_eq!(forecast_tone(ForecastKind::Thunderstorm), Style::DANGER);

        layer.fold(NwsForecastSnapshot::unavailable("rig-1", "no fresh fix"));
        assert_eq!(layer.age_ms(now), None);
        assert_eq!(layer.snapshot.as_ref().expect("snapshot").fetched_at_ms, 0);
    }

    #[test]
    fn generated_time_controls_staleness_and_attribution() {
        let now = 10_000_000;
        let mut layer = NwsForecastLayerState::default();
        layer.fold(snapshot(now));
        assert!(!layer.stale(now + SNAPSHOT_STALE_AFTER_MS));
        assert!(layer.stale(now + SNAPSHOT_STALE_AFTER_MS + 1));
        assert!(NwsForecastLayerState::attribution().contains("National Weather Service"));
    }

    #[test]
    fn fresh_aged_last_good_dims_immediately_when_fix_is_paused() {
        let now = 10_000_000;
        let mut retained = snapshot(now);
        retained
            .gaps
            .push("NWS forecast paused: fresh same-host MG90 fix unavailable".to_string());
        let mut layer = NwsForecastLayerState::default();
        layer.fold(retained);
        assert!(!layer.stale(now));
        assert!(layer.paused());

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
        assert_eq!(stats.markers, 1);
        assert!(stats.non_live, "paused route markers must dim immediately");
    }
}
