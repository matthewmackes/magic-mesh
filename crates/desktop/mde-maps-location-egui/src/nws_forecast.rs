//! NWS hourly current/drive-ahead forecast layer and painter.

use mackes_mesh_types::nws_forecast::{
    ForecastKind, ForecastPeriod, ForecastSample, NwsForecastSnapshot, ATTRIBUTION,
};
use mde_egui::egui::{self, Align2, Color32, FontId, Painter, Pos2, Rect, Stroke};
use mde_egui::Style;

/// Hourly guidance is visibly stale after ninety minutes.
pub const SNAPSHOT_STALE_AFTER_MS: i64 = 90 * 60 * 1_000;
/// A retained mirror materially ahead of the seat clock is not current data.
const MAX_TIMESTAMP_FUTURE_SKEW_MS: i64 = 5_000;
/// Consumer-side bounds are independent of the producer's wire validation:
/// another node can write the retained mirror directly.
const MAX_PAINTABLE_SAMPLES: usize = 8;
const MAX_PAINTABLE_PERIODS_PER_SAMPLE: usize = 24;
const MAX_SAMPLE_ETA_FUTURE_MS: i64 = 6 * 60 * 60 * 1_000;
const MAX_PERIOD_DURATION_MS: i64 = 3 * 60 * 60 * 1_000;
const MAX_PERIOD_FUTURE_MS: i64 = 24 * 60 * 60 * 1_000;

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
            .filter(|generated| *generated <= now_ms.saturating_add(MAX_TIMESTAMP_FUTURE_SKEW_MS))
            .map(|generated| now_ms.saturating_sub(generated).max(0))
    }

    /// Whether the retained producer time is too far ahead to be trusted.
    #[must_use]
    pub fn future_dated(&self, now_ms: i64) -> bool {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.feed_generated_at_ms)
            .is_some_and(|generated| {
                generated > now_ms.saturating_add(MAX_TIMESTAMP_FUTURE_SKEW_MS)
            })
    }

    /// Whether the producer has missed the honest hourly freshness window.
    #[must_use]
    pub fn stale(&self, now_ms: i64) -> bool {
        self.future_dated(now_ms)
            || self
                .age_ms(now_ms)
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
    /// Retained samples admitted to the bounded paint pass.
    pub samples_considered: usize,
    /// Retained periods inspected by the bounded selection pass.
    pub periods_considered: usize,
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
        for sample in snapshot.samples.iter().take(MAX_PAINTABLE_SAMPLES) {
            stats.samples_considered += 1;
            if !sample_is_paintable(sample, now_ms) {
                continue;
            }
            let Some(period) = period_for_eta(sample, now_ms, &mut stats.periods_considered) else {
                continue;
            };
            let Some(point) = project(sample.latitude, sample.longitude) else {
                continue;
            };
            if point.any_nan()
                || !point.x.is_finite()
                || !point.y.is_finite()
                || !rect.expand(20.0).contains(point)
            {
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

fn sample_is_paintable(sample: &ForecastSample, now_ms: i64) -> bool {
    sample.latitude.is_finite()
        && (-90.0..=90.0).contains(&sample.latitude)
        && sample.longitude.is_finite()
        && (-180.0..=180.0).contains(&sample.longitude)
        && sample.distance_ahead_km.is_finite()
        && (0.0..=250.0).contains(&sample.distance_ahead_km)
        && sample.eta_at_ms <= now_ms.saturating_add(MAX_SAMPLE_ETA_FUTURE_MS)
}

fn period_for_eta<'a>(
    sample: &'a ForecastSample,
    now_ms: i64,
    periods_considered: &mut usize,
) -> Option<&'a ForecastPeriod> {
    let mut next_after_eta = None;
    for period in sample.periods.iter().take(MAX_PAINTABLE_PERIODS_PER_SAMPLE) {
        *periods_considered = periods_considered.saturating_add(1);
        if !period_is_paintable(period, now_ms) || period.end_at_ms <= now_ms {
            continue;
        }
        if period.start_at_ms <= sample.eta_at_ms && sample.eta_at_ms < period.end_at_ms {
            return Some(period);
        }
        if period.start_at_ms > sample.eta_at_ms && next_after_eta.is_none() {
            next_after_eta = Some(period);
        }
    }
    next_after_eta
}

fn period_is_paintable(period: &ForecastPeriod, now_ms: i64) -> bool {
    period.end_at_ms > period.start_at_ms
        && period.end_at_ms.saturating_sub(period.start_at_ms) <= MAX_PERIOD_DURATION_MS
        && period.end_at_ms <= now_ms.saturating_add(MAX_PERIOD_FUTURE_MS)
        && matches!(period.temperature_unit.as_str(), "F" | "C")
        && period
            .precipitation_percent
            .is_none_or(|percent| percent <= 100)
        && period.humidity_percent.is_none_or(|percent| percent <= 100)
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
        (Some(_), _) if layer.future_dated(now_ms) => {
            ("NWS hourly · FUTURE producer time".to_string(), Style::WARN)
        }
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
        let mut periods_considered = 0;
        assert!(period_for_eta(sample, now, &mut periods_considered).is_some());
        assert_eq!(periods_considered, 1);
        periods_considered = 0;
        assert!(period_for_eta(sample, now + 60 * 60_000 + 1, &mut periods_considered).is_none());
        assert_eq!(periods_considered, 1);
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

    #[test]
    fn oversized_malformed_snapshot_is_bounded_and_valid_samples_still_render() {
        let now = 1_000_000;
        let mut retained = snapshot(now);
        let invalid_period = ForecastPeriod {
            start_at_ms: now + 1,
            end_at_ms: now,
            ..retained.samples[0].periods[0].clone()
        };
        let malformed_periods = vec![invalid_period; MAX_PAINTABLE_PERIODS_PER_SAMPLE + 32];
        let base = retained.samples[0].clone();
        retained.samples = (0..MAX_PAINTABLE_SAMPLES + 32)
            .map(|index| ForecastSample {
                grid_id: format!("BOX-{index}"),
                periods: malformed_periods.clone(),
                ..base.clone()
            })
            .collect();

        let mut layer = NwsForecastLayerState::default();
        layer.fold(retained);
        let ctx = egui::Context::default();
        let mut projected = 0;
        let mut bounded = PaintStats::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                bounded = paint_layer(ui.painter(), rect, &layer, now, |_lat, _lon| {
                    projected += 1;
                    Some(rect.center())
                });
            });
        });
        assert_eq!(bounded.samples_considered, MAX_PAINTABLE_SAMPLES);
        assert_eq!(
            bounded.periods_considered,
            MAX_PAINTABLE_SAMPLES * MAX_PAINTABLE_PERIODS_PER_SAMPLE
        );
        assert_eq!(bounded.markers, 0);
        assert_eq!(projected, 0, "malformed periods must not reach projection");
        assert!(bounded.badge, "malformed mirror still gets a state badge");

        layer.fold(snapshot(now));
        projected = 0;
        let mut valid = PaintStats::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                valid = paint_layer(ui.painter(), rect, &layer, now, |_lat, _lon| {
                    projected += 1;
                    Some(rect.center())
                });
            });
        });
        assert_eq!(valid.samples_considered, 1);
        assert_eq!(valid.periods_considered, 1);
        assert_eq!(valid.markers, 1, "valid forecast samples must still render");
        assert_eq!(projected, 1);
    }

    #[test]
    fn malformed_coordinates_and_future_feed_time_are_non_live() {
        let now = 1_000_000;
        let mut retained = snapshot(now);
        retained.samples[0].latitude = f64::NAN;
        retained.feed_generated_at_ms = Some(now + MAX_TIMESTAMP_FUTURE_SKEW_MS + 1);
        let mut layer = NwsForecastLayerState::default();
        layer.fold(retained);
        assert!(layer.future_dated(now));
        assert!(layer.stale(now));

        let ctx = egui::Context::default();
        let mut stats = PaintStats::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                stats = paint_layer(ui.painter(), ui.max_rect(), &layer, now, |_lat, _lon| {
                    panic!("invalid coordinates must not reach projection")
                });
            });
        });
        assert_eq!(stats.markers, 0);
        assert_eq!(stats.samples_considered, 1);
        assert_eq!(stats.periods_considered, 0);
        assert!(stats.non_live);
    }
}
