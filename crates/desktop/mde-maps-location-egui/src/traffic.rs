//! Keyless NCDOT TIMS traffic-event model and painter (OVERLAY-3).

use mackes_mesh_types::traffic::{TrafficEvent, TrafficSnapshot, ATTRIBUTION};
use mde_egui::egui::{self, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke};
use mde_egui::Style;

/// Five missed one-minute polls make retained traffic visibly stale.
pub const SNAPSHOT_STALE_AFTER_MS: i64 = 5 * 60 * 1_000;

const ROADWORK: Color32 = Color32::from_rgb(0xF2, 0x82, 0x22); // style-leak-ok: map-content-color

/// Retained complete nearby NCDOT event set.
#[derive(Debug, Clone, Default)]
pub struct TrafficLayerState {
    /// Latest vehicle-centred snapshot.
    pub snapshot: Option<TrafficSnapshot>,
}

impl TrafficLayerState {
    /// Replace the previous current set wholesale.
    pub fn fold(&mut self, snapshot: TrafficSnapshot) {
        self.snapshot = Some(snapshot);
    }

    /// Age since the last successful fetch or conditional validation.
    #[must_use]
    pub fn age_ms(&self, now_ms: i64) -> Option<i64> {
        self.snapshot
            .as_ref()
            .map(|snapshot| now_ms.saturating_sub(snapshot.fetched_at_ms).max(0))
    }

    /// Whether the adapter missed five expected polls.
    #[must_use]
    pub fn stale(&self, now_ms: i64) -> bool {
        self.age_ms(now_ms)
            .is_some_and(|age| age > SNAPSHOT_STALE_AFTER_MS)
    }

    /// Whether a failed refresh/fix loss paused the last-good set.
    #[must_use]
    pub fn paused(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .gaps
                .iter()
                .any(|gap| gap.starts_with("NCDOT traffic paused:"))
        })
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
    /// Current projected incident markers painted.
    pub markers: usize,
    /// Whether the honest age/no-data badge painted.
    pub badge: bool,
}

/// Paint current traffic markers and one freshness badge.
pub fn paint_layer<F>(
    painter: &Painter,
    rect: Rect,
    layer: &TrafficLayerState,
    now_ms: i64,
    mut project: F,
) -> PaintStats
where
    F: FnMut(f64, f64) -> Option<Pos2>,
{
    if !rect.is_finite() || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return PaintStats::default();
    }
    let dimmed = layer.stale(now_ms) || layer.paused();
    let mut stats = PaintStats::default();
    if let Some(snapshot) = &layer.snapshot {
        let marker_painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
        for event in snapshot
            .events
            .iter()
            .filter(|event| event.ends_at_ms.is_none_or(|end| end >= now_ms))
        {
            let Some(point) = project(event.latitude, event.longitude) else {
                continue;
            };
            if point.any_nan() || !rect.expand(18.0).contains(point) {
                continue;
            }
            paint_marker(&marker_painter, point, event, dimmed);
            stats.markers += 1;
        }
    }
    paint_age_badge(painter, rect, layer, now_ms);
    stats.badge = true;
    stats
}

fn paint_marker(painter: &Painter, point: Pos2, event: &TrafficEvent, dimmed: bool) {
    let tone = if dimmed {
        Style::TEXT_DIM
    } else if event.full_closure {
        Style::DANGER
    } else if event.event_type.eq_ignore_ascii_case("roadwork") {
        ROADWORK
    } else {
        Style::ACCENT_HI
    };
    let alpha = if dimmed { 0.38 } else { 0.92 };
    let radius = if event.full_closure { 8.0 } else { 6.5 };
    let points = vec![
        point + egui::vec2(0.0, -radius),
        point + egui::vec2(radius, 0.0),
        point + egui::vec2(0.0, radius),
        point + egui::vec2(-radius, 0.0),
    ];
    painter.add(Shape::convex_polygon(
        points,
        tone.gamma_multiply(alpha),
        Stroke::new(1.25, Color32::WHITE.gamma_multiply(alpha)),
    ));
    if event.full_closure {
        painter.line_segment(
            [point + egui::vec2(-3.0, 0.0), point + egui::vec2(3.0, 0.0)],
            Stroke::new(1.5, Style::BG.gamma_multiply(alpha)),
        );
    }
}

fn paint_age_badge(painter: &Painter, rect: Rect, layer: &TrafficLayerState, now_ms: i64) {
    let (label, tone) = match (&layer.snapshot, layer.age_ms(now_ms)) {
        (None, _) => ("NCDOT traffic · no data".to_string(), Style::TEXT_DIM),
        (Some(_), Some(age)) if layer.paused() => (
            format!("NCDOT traffic · PAUSED · {}", age_label(age)),
            Style::WARN,
        ),
        (Some(_), Some(age)) if age > SNAPSHOT_STALE_AFTER_MS => (
            format!("NCDOT traffic · STALE {}", age_label(age)),
            Style::WARN,
        ),
        (Some(snapshot), Some(age)) if !snapshot.gaps.is_empty() => (
            format!(
                "NCDOT traffic · {} · {} events · degraded",
                age_label(age),
                snapshot.events.len()
            ),
            Style::WARN,
        ),
        (Some(snapshot), Some(age)) => (
            format!(
                "NCDOT traffic · {} · {} events",
                age_label(age),
                snapshot.events.len()
            ),
            Style::TEXT,
        ),
        (Some(_), None) => ("NCDOT traffic · no timestamp".to_string(), Style::WARN),
    };
    let galley = painter.layout_no_wrap(label, FontId::proportional(Style::SMALL), tone);
    let pad = egui::vec2(Style::SP_S, Style::SP_XS);
    let row_height = galley.size().y + pad.y * 2.0 + Style::SP_XS;
    let badge = Rect::from_min_size(
        egui::pos2(
            rect.right() - galley.size().x - pad.x * 2.0 - Style::SP_S,
            rect.top() + Style::SP_S + row_height * 8.0,
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
    use super::*;

    fn snapshot(now: i64) -> TrafficSnapshot {
        let mut snapshot = TrafficSnapshot::empty("rig-1", now, 35.7, -78.65, 100);
        snapshot.events.push(TrafficEvent {
            id: "2188564".to_string(),
            road: "NC 55".to_string(),
            summary: "Construction on NC 55".to_string(),
            event_type: "roadwork".to_string(),
            event_subtype: "construction".to_string(),
            condition: Some("Open".to_string()),
            lanes_affected: Some("Lane Affected".to_string()),
            direction: Some("Both Directions".to_string()),
            county: Some("Wake".to_string()),
            full_closure: false,
            latitude: 35.5574,
            longitude: -78.7545,
            distance_km: 20.0,
            starts_at_ms: Some(now - 60_000),
            ends_at_ms: Some(now + 60_000),
            updated_at_ms: Some(now),
        });
        snapshot
    }

    #[test]
    fn fold_replaces_and_paused_dims_before_stale_cutoff() {
        let now = 1_000_000;
        let mut layer = TrafficLayerState::default();
        let mut first = snapshot(now);
        first.events.push(TrafficEvent {
            id: "removed".to_string(),
            ..first.events[0].clone()
        });
        layer.fold(first);
        layer.fold(snapshot(now + 1));
        assert_eq!(layer.snapshot.as_ref().expect("snapshot").events.len(), 1);
        assert!(!layer.stale(now + 1));
        layer
            .snapshot
            .as_mut()
            .expect("snapshot")
            .gaps
            .push("NCDOT traffic paused: HTTP 429".to_string());
        assert!(layer.paused());
    }

    #[test]
    fn expired_events_do_not_paint() {
        let now = 1_000_000;
        let mut expired = snapshot(now);
        expired.events[0].ends_at_ms = Some(now - 1);
        let mut layer = TrafficLayerState::default();
        layer.fold(expired);
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
        assert!(stats.badge);
    }

    #[test]
    fn painter_emits_real_marker_and_badge() {
        let now = 1_000_000;
        let mut layer = TrafficLayerState::default();
        layer.fold(snapshot(now));
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
        assert!(stats.badge);
        assert!(ctx.tessellate(output.shapes, output.pixels_per_point).len() >= 2);
    }
}
