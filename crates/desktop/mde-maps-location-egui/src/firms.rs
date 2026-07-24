//! NASA FIRMS thermal-hotspot model and painter (WL-FUNC-012 / OVERLAY-6).
//!
//! FIRMS is a contextual near-real-time feed, not a safety-of-life signal.
//! Retained snapshots therefore carry their provider availability, fetch age,
//! and pause/error gaps all the way to the map.  A consumer never paints a
//! hotspot set unless the snapshot proves that a keyed fetch succeeded.

use mackes_mesh_types::firms::{FirmsAvailability, FirmsHotspot, FirmsSnapshot, ATTRIBUTION};
use mde_egui::egui::{self, Color32, FontId, Painter, Pos2, Rect, Stroke};
use mde_egui::Style;

/// Three missed fifteen-minute polls make retained FIRMS hotspots stale.
pub const SNAPSHOT_STALE_AFTER_MS: i64 = 45 * 60 * 1_000;

/// Keep an untrusted retained hotspot set bounded before projection and paint.
const MAX_PAINTABLE_HOTSPOTS: usize = 256;
/// A producer clock may be a few seconds ahead, but not enough to make a
/// retained future snapshot look like a current successful fetch.
const MAX_TIMESTAMP_FUTURE_SKEW_MS: i64 = 5_000;

const HOTSPOT_FILL: Color32 = Color32::from_rgb(0xFF, 0xA0, 0x00); // style-leak-ok: map-content-color
const HOTSPOT_CORE: Color32 = Color32::from_rgb(0xFF, 0xE0, 0x57); // style-leak-ok: map-content-color

/// Retained complete FIRMS snapshot.
#[derive(Debug, Clone, Default)]
pub struct FirmsLayerState {
    /// Latest vehicle-centred hotspot snapshot.
    pub snapshot: Option<FirmsSnapshot>,
}

impl FirmsLayerState {
    /// Replace the previous latest-wins snapshot wholesale.
    pub fn fold(&mut self, snapshot: FirmsSnapshot) {
        self.snapshot = Some(snapshot);
    }

    /// Age since the last successful FIRMS request.
    #[must_use]
    pub fn age_ms(&self, now_ms: i64) -> Option<i64> {
        self.snapshot
            .as_ref()?
            .fetched_at_ms
            .map(|fetched| now_ms.saturating_sub(fetched).max(0))
    }

    /// Whether the retained producer timestamps are too far ahead of the
    /// consumer clock to be trusted as current data.
    #[must_use]
    pub fn future_dated(&self, now_ms: i64) -> bool {
        self.snapshot.as_ref().is_some_and(|snapshot| {
            let latest_credible = now_ms.saturating_add(MAX_TIMESTAMP_FUTURE_SKEW_MS);
            snapshot.published_at_ms > latest_credible
                || snapshot
                    .fetched_at_ms
                    .is_some_and(|fetched| fetched > latest_credible)
        })
    }

    /// Whether the retained successful fetch is older than three poll periods.
    #[must_use]
    pub fn stale(&self, now_ms: i64) -> bool {
        self.age_ms(now_ms)
            .is_some_and(|age| age > SNAPSHOT_STALE_AFTER_MS)
    }

    /// Whether the producer explicitly paused its last-good set.
    #[must_use]
    pub fn paused(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .gaps
                .iter()
                .any(|gap| gap.starts_with("NASA FIRMS paused:"))
        })
    }

    /// Whether the operator has not sealed the free NASA MAP_KEY.
    #[must_use]
    pub fn unconfigured(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.availability == FirmsAvailability::Unconfigured)
    }

    /// Whether the secret backend has reported an unusable credential state.
    #[must_use]
    pub fn secret_store_error(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.availability == FirmsAvailability::SecretStoreError)
    }

    /// Required active-layer attribution.
    #[must_use]
    pub const fn attribution() -> &'static str {
        ATTRIBUTION
    }
}

/// Observable paint facts used by headless regression tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PaintStats {
    /// Projected FIRMS hotspots painted in the viewport.
    pub hotspots: usize,
    /// Whether the honest status badge painted.
    pub badge: bool,
}

/// Paint distinct FRP-sized FIRMS heat dots and one honest status badge.
pub fn paint_layer<F>(
    painter: &Painter,
    rect: Rect,
    layer: &FirmsLayerState,
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
    let dimmed = layer.stale(now_ms) || layer.paused() || layer.future_dated(now_ms);
    if let Some(snapshot) = &layer.snapshot {
        // A retained body with unconfigured/secret-error availability is not
        // evidence of a successful query.  Do not trust any malformed hotspot
        // rows that may have been carried alongside that status.
        let ready =
            snapshot.availability == FirmsAvailability::Ready && snapshot.fetched_at_ms.is_some();
        if ready && !layer.future_dated(now_ms) {
            let hotspot_painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
            for hotspot in snapshot
                .hotspots
                .iter()
                .take(MAX_PAINTABLE_HOTSPOTS)
                .filter(|hotspot| hotspot_is_paintable(hotspot, snapshot, now_ms))
            {
                let Some(point) = project(hotspot.latitude, hotspot.longitude) else {
                    continue;
                };
                if point.any_nan() || !rect.expand(18.0).contains(point) {
                    continue;
                }
                paint_hotspot(&hotspot_painter, point, hotspot, dimmed);
                stats.hotspots += 1;
            }
        }
    }
    paint_status_badge(painter, rect, layer, now_ms);
    stats.badge = true;
    stats
}

fn hotspot_is_paintable(hotspot: &FirmsHotspot, snapshot: &FirmsSnapshot, now_ms: i64) -> bool {
    hotspot.latitude.is_finite()
        && (-90.0..=90.0).contains(&hotspot.latitude)
        && hotspot.longitude.is_finite()
        && (-180.0..=180.0).contains(&hotspot.longitude)
        && hotspot.observed_at_ms <= now_ms.saturating_add(MAX_TIMESTAMP_FUTURE_SKEW_MS)
        && snapshot.fetched_at_ms.is_none_or(|fetched| {
            hotspot.observed_at_ms <= fetched.saturating_add(MAX_TIMESTAMP_FUTURE_SKEW_MS)
        })
}

fn paint_hotspot(painter: &Painter, point: Pos2, hotspot: &FirmsHotspot, dimmed: bool) {
    let tone = if dimmed {
        Style::TEXT_DIM
    } else {
        HOTSPOT_FILL
    };
    let alpha = if dimmed { 0.35 } else { 0.84 };
    let frp = hotspot.frp_mw.unwrap_or(0.0).max(0.0).min(500.0);
    let radius = 4.0 + (frp / 500.0) * 6.0;
    painter.circle_filled(point, radius, tone.gamma_multiply(alpha));
    painter.circle_stroke(
        point,
        radius,
        Stroke::new(1.25, Color32::WHITE.gamma_multiply(alpha)),
    );
    painter.circle_filled(
        point,
        (radius * 0.38).max(1.5),
        if dimmed {
            Style::TEXT_DIM.gamma_multiply(0.72)
        } else {
            HOTSPOT_CORE.gamma_multiply(0.95)
        },
    );
}

fn paint_status_badge(painter: &Painter, rect: Rect, layer: &FirmsLayerState, now_ms: i64) {
    let (label, tone) = match (&layer.snapshot, layer.age_ms(now_ms)) {
        (None, _) => ("Wildfire · FIRMS no data".to_string(), Style::TEXT_DIM),
        (Some(_), _) if layer.unconfigured() => (
            "Wildfire · FIRMS API key not configured".to_string(),
            Style::WARN,
        ),
        (Some(_), _) if layer.secret_store_error() => (
            "Wildfire · FIRMS secret store unavailable".to_string(),
            Style::DANGER,
        ),
        (Some(snapshot), _) if layer.future_dated(now_ms) => (
            format!(
                "Wildfire · FIRMS FUTURE · {} hotspots",
                hotspot_count_label(snapshot)
            ),
            Style::WARN,
        ),
        (Some(_), age) if layer.paused() => (
            format!(
                "Wildfire · FIRMS PAUSED · {}",
                age.map_or_else(|| "no successful fetch".to_string(), age_label)
            ),
            Style::WARN,
        ),
        (Some(_), Some(age)) if age > SNAPSHOT_STALE_AFTER_MS => (
            format!("Wildfire · FIRMS STALE {}", age_label(age)),
            Style::WARN,
        ),
        (Some(snapshot), Some(age)) if !snapshot.gaps.is_empty() => (
            format!(
                "Wildfire · FIRMS {} · {} hotspots · degraded",
                age_label(age),
                hotspot_count_label(snapshot)
            ),
            Style::WARN,
        ),
        (Some(snapshot), Some(age)) => (
            format!(
                "Wildfire · FIRMS {} · {} hotspots",
                age_label(age),
                hotspot_count_label(snapshot)
            ),
            Style::TEXT,
        ),
        (Some(_), None) => (
            "Wildfire · FIRMS awaiting first fetch".to_string(),
            Style::TEXT_DIM,
        ),
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

fn hotspot_count_label(snapshot: &FirmsSnapshot) -> String {
    if snapshot.hotspots.len() > MAX_PAINTABLE_HOTSPOTS {
        format!("{}+", MAX_PAINTABLE_HOTSPOTS)
    } else {
        snapshot.hotspots.len().to_string()
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

    fn snapshot(now_ms: i64) -> FirmsSnapshot {
        let mut snapshot = FirmsSnapshot::empty(
            "rig-1",
            now_ms,
            now_ms,
            "VIIRS_NOAA20_NRT",
            44.0,
            -120.0,
            200,
        );
        snapshot.hotspots.push(FirmsHotspot {
            id: "hot-1".to_string(),
            latitude: 44.01,
            longitude: -120.02,
            brightness_k: Some(331.2),
            frp_mw: Some(42.0),
            confidence: Some("nominal".to_string()),
            satellite: Some("N20".to_string()),
            observed_at_ms: now_ms - 60_000,
            distance_km: 2.1,
        });
        snapshot
    }

    #[test]
    fn fold_replaces_set_and_reports_pause_and_age() {
        let now_ms = 10_000_000;
        let mut layer = FirmsLayerState::default();
        layer.fold(snapshot(now_ms));
        assert_eq!(layer.snapshot.as_ref().expect("snapshot").hotspots.len(), 1);
        assert!(!layer.stale(now_ms + 1));
        layer
            .snapshot
            .as_mut()
            .expect("snapshot")
            .gaps
            .push("NASA FIRMS paused: missing fresh vehicle fix".to_string());
        assert!(layer.paused());
        assert!(layer.stale(now_ms + SNAPSHOT_STALE_AFTER_MS + 1));
    }

    #[test]
    fn unconfigured_and_secret_error_never_paint_retained_rows() {
        for availability in [
            FirmsAvailability::Unconfigured,
            FirmsAvailability::SecretStoreError,
        ] {
            let now_ms = 10_000_000;
            let mut snapshot = snapshot(now_ms);
            snapshot.availability = availability;
            let mut layer = FirmsLayerState::default();
            layer.fold(snapshot);
            let ctx = egui::Context::default();
            let mut stats = PaintStats::default();
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    stats =
                        paint_layer(ui.painter(), ui.max_rect(), &layer, now_ms, |_lat, _lon| {
                            Some(ui.max_rect().center())
                        });
                });
            });
            assert_eq!(stats.hotspots, 0);
            assert!(stats.badge);
        }
    }

    #[test]
    fn fresh_hotspot_paints_distinct_marker_and_badge() {
        let now_ms = 10_000_000;
        let mut layer = FirmsLayerState::default();
        layer.fold(snapshot(now_ms));
        let ctx = egui::Context::default();
        let mut stats = PaintStats::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                stats = paint_layer(ui.painter(), rect, &layer, now_ms, |_lat, _lon| {
                    Some(rect.center())
                });
            });
        });
        assert_eq!(stats.hotspots, 1);
        assert!(stats.badge);
        assert!(ctx.tessellate(output.shapes, output.pixels_per_point).len() >= 3);
    }

    #[test]
    fn malformed_and_future_hotspots_are_not_projected() {
        let now_ms = 10_000_000;
        let mut retained = snapshot(now_ms);
        retained.hotspots.push(FirmsHotspot {
            latitude: f64::NAN,
            ..retained.hotspots[0].clone()
        });
        retained.hotspots.push(FirmsHotspot {
            longitude: 181.0,
            ..retained.hotspots[0].clone()
        });
        retained.hotspots.push(FirmsHotspot {
            observed_at_ms: now_ms + MAX_TIMESTAMP_FUTURE_SKEW_MS + 1,
            ..retained.hotspots[0].clone()
        });
        let mut layer = FirmsLayerState::default();
        layer.fold(retained);
        let ctx = egui::Context::default();
        let mut projected = 0;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                let _ = paint_layer(ui.painter(), rect, &layer, now_ms, |_lat, _lon| {
                    projected += 1;
                    Some(rect.center())
                });
            });
        });
        assert_eq!(projected, 1);
    }

    #[test]
    fn oversized_retained_snapshot_is_bounded_at_the_paint_boundary() {
        let now_ms = 10_000_000;
        let base = snapshot(now_ms).hotspots[0].clone();
        let mut retained = snapshot(now_ms);
        retained.hotspots = (0..(MAX_PAINTABLE_HOTSPOTS + 32))
            .map(|index| FirmsHotspot {
                id: format!("hot-{index}"),
                latitude: base.latitude,
                longitude: base.longitude,
                ..base.clone()
            })
            .collect();
        let mut layer = FirmsLayerState::default();
        layer.fold(retained);
        let ctx = egui::Context::default();
        let mut stats = PaintStats::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                stats = paint_layer(ui.painter(), rect, &layer, now_ms, |_lat, _lon| {
                    Some(rect.center())
                });
            });
        });
        assert_eq!(stats.hotspots, MAX_PAINTABLE_HOTSPOTS);
    }

    #[test]
    fn future_snapshot_is_not_painted_as_current() {
        let now_ms = 10_000_000;
        let mut retained = snapshot(now_ms);
        retained.published_at_ms = now_ms + MAX_TIMESTAMP_FUTURE_SKEW_MS + 1;
        retained.fetched_at_ms = Some(now_ms + MAX_TIMESTAMP_FUTURE_SKEW_MS + 1);
        let mut layer = FirmsLayerState::default();
        layer.fold(retained);
        assert!(layer.future_dated(now_ms));
        let ctx = egui::Context::default();
        let mut stats = PaintStats::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                stats = paint_layer(ui.painter(), ui.max_rect(), &layer, now_ms, |_lat, _lon| {
                    Some(ui.max_rect().center())
                });
            });
        });
        assert_eq!(stats.hotspots, 0);
        assert!(stats.badge);
    }
}
