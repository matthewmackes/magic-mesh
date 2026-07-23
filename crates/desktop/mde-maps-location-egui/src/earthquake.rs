//! USGS earthquake overlay model and painter (WL-FUNC-012 / OVERLAY-10).
//!
//! The daemon owns fetching and normalization. This desktop module only folds
//! the retained typed snapshot, derives honest age/staleness, and paints it via
//! the basemap projection supplied by the caller.

use mackes_mesh_types::earthquake::{EarthquakeEvent, EarthquakeSnapshot, PagerAlert, ATTRIBUTION};
use mde_egui::egui::{self, Color32, FontId, Painter, Pos2, Rect, Stroke};
use mde_egui::Style;

/// Five missed one-minute polls turn a retained snapshot stale.
pub const SNAPSHOT_STALE_AFTER_MS: i64 = 5 * 60 * 1_000;
/// Quake marker fade window required by the overlay catalog.
pub const EVENT_FADE_AFTER_MS: i64 = 24 * 60 * 60 * 1_000;

const PAGER_YELLOW: Color32 = Color32::from_rgb(0xF1, 0xC2, 0x32); // style-leak-ok: map-content-color
const PAGER_ORANGE: Color32 = Color32::from_rgb(0xF2, 0x82, 0x22); // style-leak-ok: map-content-color

/// Retained state for the ambient earthquake layer.
#[derive(Debug, Clone, Default)]
pub struct EarthquakeLayerState {
    /// Latest complete snapshot, or `None` before any adapter publish.
    pub snapshot: Option<EarthquakeSnapshot>,
}

impl EarthquakeLayerState {
    /// Replace the prior feed wholesale. This makes upstream revisions and
    /// deletes converge rather than accumulating dead event markers.
    pub fn fold(&mut self, snapshot: EarthquakeSnapshot) {
        self.snapshot = Some(snapshot);
    }

    /// Age since the last successful fetch or conditional validation.
    #[must_use]
    pub fn age_ms(&self, now_ms: i64) -> Option<i64> {
        self.snapshot
            .as_ref()
            .map(|snapshot| (now_ms - snapshot.fetched_at_ms).max(0))
    }

    /// Whether the retained snapshot has missed five expected polls.
    #[must_use]
    pub fn stale(&self, now_ms: i64) -> bool {
        self.age_ms(now_ms)
            .is_some_and(|age| age > SNAPSHOT_STALE_AFTER_MS)
    }

    /// Attribution appended whenever the layer toggle is active.
    #[must_use]
    pub const fn attribution() -> &'static str {
        ATTRIBUTION
    }
}

/// Observable result of one layer paint, useful to keep headless smoke tests
/// tied to real marker output rather than merely proving the panel did not panic.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PaintStats {
    /// Events projected inside the viewport and painted.
    pub markers: usize,
    /// Whether the age/no-data badge was painted.
    pub badge: bool,
}

/// Paint magnitude-scaled, PAGER-coloured quake markers plus an honest age
/// badge. `project` is the current basemap projection seam; returning `None`
/// omits an off-map/unprojectable event without inventing a location.
pub fn paint_layer<F>(
    painter: &Painter,
    rect: Rect,
    layer: &EarthquakeLayerState,
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
        let clip = rect.intersect(painter.clip_rect());
        let marker_painter = painter.with_clip_rect(clip);
        for event in &snapshot.events {
            let event_age = (now_ms - event.occurred_at_ms).max(0);
            if event_age > EVENT_FADE_AFTER_MS {
                continue;
            }
            let Some(point) = project(event.latitude, event.longitude) else {
                continue;
            };
            if point.any_nan() || !rect.expand(18.0).contains(point) {
                continue;
            }
            paint_marker(&marker_painter, point, event, event_age, stale);
            stats.markers += 1;
        }
    }
    paint_age_badge(painter, rect, layer, now_ms);
    stats.badge = true;
    stats
}

fn paint_marker(
    painter: &Painter,
    point: Pos2,
    event: &EarthquakeEvent,
    event_age_ms: i64,
    stale: bool,
) {
    let magnitude = event.magnitude.unwrap_or(0.0).max(0.0);
    let radius = (4.0 + magnitude * 1.7).clamp(4.0, 18.0);
    let age_fade = 1.0 - (event_age_ms as f32 / EVENT_FADE_AFTER_MS as f32).clamp(0.0, 1.0);
    let tone = if stale {
        Style::TEXT_DIM
    } else {
        pager_color(event.pager_alert)
    };
    let alpha = if stale {
        (age_fade * 0.32).max(0.10)
    } else {
        (age_fade * 0.85).max(0.16)
    };
    painter.circle_filled(point, radius + 4.0, tone.gamma_multiply(alpha * 0.20));
    painter.circle_filled(point, radius, tone.gamma_multiply(alpha));
    painter.circle_stroke(
        point,
        radius,
        Stroke::new(1.25, Color32::WHITE.gamma_multiply(alpha * 0.85)),
    );
}

fn pager_color(alert: Option<PagerAlert>) -> Color32 {
    match alert {
        Some(PagerAlert::Green) => Style::OK,
        Some(PagerAlert::Yellow) => PAGER_YELLOW,
        Some(PagerAlert::Orange) => PAGER_ORANGE,
        Some(PagerAlert::Red) => Style::DANGER,
        None => Style::ACCENT_HI,
    }
}

fn paint_age_badge(painter: &Painter, rect: Rect, layer: &EarthquakeLayerState, now_ms: i64) {
    let (label, tone) = match (&layer.snapshot, layer.age_ms(now_ms)) {
        (None, _) => ("USGS earthquakes · no data".to_string(), Style::TEXT_DIM),
        (Some(snapshot), Some(age)) if age > SNAPSHOT_STALE_AFTER_MS => (
            format!("USGS earthquakes · STALE {}", age_label(age)),
            Style::WARN,
        ),
        (Some(snapshot), Some(age)) if !snapshot.gaps.is_empty() => (
            format!("USGS earthquakes · {} · degraded", age_label(age)),
            Style::WARN,
        ),
        (Some(_), Some(age)) => (
            format!("USGS earthquakes · {}", age_label(age)),
            Style::TEXT,
        ),
        (Some(_), None) => ("USGS earthquakes · no timestamp".to_string(), Style::WARN),
    };
    let galley = painter.layout_no_wrap(label, FontId::proportional(Style::SMALL), tone);
    let pad = egui::vec2(Style::SP_S, Style::SP_XS);
    let badge = Rect::from_min_size(
        rect.right_top() - egui::vec2(galley.size().x + pad.x * 2.0 + Style::SP_S, -Style::SP_S),
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
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h", seconds / 3_600)
    }
}

/// Current wall-clock time in Unix milliseconds.
#[must_use]
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(fetched_at_ms: i64) -> EarthquakeSnapshot {
        let mut snapshot = EarthquakeSnapshot::empty("rig-1", fetched_at_ms);
        snapshot.events.push(EarthquakeEvent {
            id: "ci40659474".to_string(),
            occurred_at_ms: fetched_at_ms - 60_000,
            updated_at_ms: fetched_at_ms,
            latitude: 35.956,
            longitude: -117.95,
            depth_km: 2.98,
            magnitude: Some(4.2),
            place: "4 km WNW of Little Lake, CA".to_string(),
            pager_alert: Some(PagerAlert::Orange),
            detail_url: None,
        });
        snapshot
    }

    #[test]
    fn fold_replaces_revised_and_deleted_events_wholesale() {
        let mut layer = EarthquakeLayerState::default();
        let mut first = snapshot(1_000_000);
        first.events.push(EarthquakeEvent {
            id: "deleted".to_string(),
            ..first.events[0].clone()
        });
        layer.fold(first);

        let mut revised = snapshot(1_060_000);
        revised.events[0].updated_at_ms += 60_000;
        revised.events[0].magnitude = Some(4.4);
        layer.fold(revised);

        let current = layer.snapshot.as_ref().expect("snapshot");
        assert_eq!(current.events.len(), 1);
        assert_eq!(current.events[0].magnitude, Some(4.4));
        assert!(current.events.iter().all(|event| event.id != "deleted"));
    }

    #[test]
    fn snapshot_staleness_is_derived_from_fetch_time() {
        let mut layer = EarthquakeLayerState::default();
        layer.fold(snapshot(1_000_000));
        assert!(!layer.stale(1_000_000 + SNAPSHOT_STALE_AFTER_MS));
        assert!(layer.stale(1_000_001 + SNAPSHOT_STALE_AFTER_MS));
    }

    #[test]
    fn painter_emits_real_marker_and_age_badge_without_basemap_fixture() {
        let now = 1_060_000;
        let mut layer = EarthquakeLayerState::default();
        layer.fold(snapshot(now));
        let ctx = egui::Context::default();
        let mut observed = PaintStats::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                observed = paint_layer(ui.painter(), rect, &layer, now, |_lat, _lon| {
                    Some(rect.center())
                });
            });
        });
        assert_eq!(observed.markers, 1);
        assert!(observed.badge);
        assert!(ctx.tessellate(output.shapes, output.pixels_per_point).len() >= 3);
    }

    #[test]
    fn old_events_expire_and_stale_snapshot_still_gets_badge() {
        let now = 100_000_000;
        let mut old = snapshot(now - EVENT_FADE_AFTER_MS - 1);
        old.events[0].occurred_at_ms = now - EVENT_FADE_AFTER_MS - 1;
        let mut layer = EarthquakeLayerState::default();
        layer.fold(old);
        let ctx = egui::Context::default();
        let mut observed = PaintStats::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                observed = paint_layer(ui.painter(), rect, &layer, now, |_lat, _lon| {
                    Some(rect.center())
                });
            });
        });
        assert_eq!(observed.markers, 0);
        assert!(observed.badge);
        assert!(layer.stale(now));
    }
}
