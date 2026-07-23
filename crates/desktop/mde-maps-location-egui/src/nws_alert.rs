//! NWS active-weather-alert layer model and painter.

use mackes_mesh_types::nws_alert::{
    AlertPolygon, GeoPoint, NwsAlert, NwsAlertSnapshot, NwsSeverity, ATTRIBUTION,
};
use mde_egui::egui::{self, Color32, FontId, Mesh, Painter, Pos2, Rect, Shape, Stroke};
use mde_egui::Style;

/// Three missed minute polls make a safety layer visibly stale.
pub const SNAPSHOT_STALE_AFTER_MS: i64 = 3 * 60 * 1_000;

const SEVERITY_EXTREME: Color32 = Color32::from_rgb(0xD4, 0x2A, 0x2A); // style-leak-ok: map-content-color
const SEVERITY_SEVERE: Color32 = Color32::from_rgb(0xF2, 0x82, 0x22); // style-leak-ok: map-content-color
const SEVERITY_MODERATE: Color32 = Color32::from_rgb(0xF1, 0xC2, 0x32); // style-leak-ok: map-content-color

/// Retained complete NWS snapshot.
#[derive(Debug, Clone, Default)]
pub struct NwsAlertLayerState {
    /// Latest snapshot, absent before the adapter publishes.
    pub snapshot: Option<NwsAlertSnapshot>,
}

impl NwsAlertLayerState {
    /// Replace the prior active set wholesale.
    pub fn fold(&mut self, snapshot: NwsAlertSnapshot) {
        self.snapshot = Some(snapshot);
    }

    /// Snapshot age in milliseconds.
    #[must_use]
    pub fn age_ms(&self, now_ms: i64) -> Option<i64> {
        self.snapshot
            .as_ref()
            .map(|snapshot| now_ms.saturating_sub(snapshot.fetched_at_ms).max(0))
    }

    /// Whether the safety feed has missed three expected polls.
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

/// Observable paint facts used by headless regression tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PaintStats {
    /// Alert polygons painted or outlined in the viewport.
    pub polygons: usize,
    /// Whether the vehicle is inside at least one non-expired warning polygon.
    pub inside_alert: bool,
    /// Whether the age/no-data badge rendered.
    pub badge: bool,
}

/// Paint alert polygons, an in-warning banner, and a freshness badge.
pub fn paint_layer<F>(
    painter: &Painter,
    rect: Rect,
    layer: &NwsAlertLayerState,
    now_ms: i64,
    vehicle: Option<GeoPoint>,
    mut project: F,
) -> PaintStats
where
    F: FnMut(f64, f64) -> Option<Pos2>,
{
    if !rect.is_finite() || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return PaintStats::default();
    }
    let stale = layer.stale(now_ms);
    let mut stats = PaintStats::default();
    let mut inside: Option<&NwsAlert> = None;
    if let Some(snapshot) = &layer.snapshot {
        for alert in snapshot
            .alerts
            .iter()
            .filter(|alert| !expired(alert, now_ms))
        {
            if vehicle.is_some_and(|point| alert_contains(alert, point))
                && inside.is_none_or(|current| {
                    severity_rank(alert.severity) > severity_rank(current.severity)
                })
            {
                inside = Some(alert);
            }
            for polygon in &alert.polygons {
                if paint_polygon(painter, rect, polygon, alert.severity, stale, &mut project) {
                    stats.polygons += 1;
                }
            }
        }
    }
    if let Some(alert) = inside {
        paint_warning_banner(painter, rect, alert, stale);
        stats.inside_alert = true;
    }
    paint_age_badge(painter, rect, layer, now_ms);
    stats.badge = true;
    stats
}

fn expired(alert: &NwsAlert, now_ms: i64) -> bool {
    alert.expires_at_ms.is_some_and(|expires| expires < now_ms)
}

fn paint_polygon<F>(
    painter: &Painter,
    rect: Rect,
    polygon: &AlertPolygon,
    severity: NwsSeverity,
    stale: bool,
    project: &mut F,
) -> bool
where
    F: FnMut(f64, f64) -> Option<Pos2>,
{
    let mut projected_rings = Vec::new();
    for ring in &polygon.rings {
        let points: Vec<Pos2> = ring
            .iter()
            .filter_map(|point| project(point.latitude, point.longitude))
            .filter(|point| !point.any_nan())
            .collect();
        if points.len() >= 3 {
            projected_rings.push(points);
        }
    }
    let Some(outer) = projected_rings.first() else {
        return false;
    };
    if !outer.iter().any(|point| rect.expand(24.0).contains(*point)) {
        return false;
    }
    let tone = if stale {
        Style::TEXT_DIM
    } else {
        severity_color(severity)
    };
    // A polygon with holes is outlined but not filled: filling the exterior
    // would falsely warn inside an exclusion hole. Hole-free polygons get a
    // real concave-safe ear-clipped translucent fill.
    if projected_rings.len() == 1 {
        paint_concave_fill(
            painter,
            outer,
            tone.gamma_multiply(if stale { 0.08 } else { 0.18 }),
        );
    }
    for ring in projected_rings {
        painter.add(Shape::closed_line(
            ring,
            Stroke::new(2.0, tone.gamma_multiply(if stale { 0.40 } else { 0.85 })),
        ));
    }
    true
}

fn paint_concave_fill(painter: &Painter, points: &[Pos2], color: Color32) {
    let triangles = triangulate(points);
    if triangles.is_empty() {
        return;
    }
    let mut mesh = Mesh::default();
    for point in points {
        mesh.colored_vertex(*point, color);
    }
    for [a, b, c] in triangles {
        mesh.add_triangle(a as u32, b as u32, c as u32);
    }
    painter.add(mesh);
}

/// Ear-clip a simple concave polygon. Duplicate closing points are ignored.
fn triangulate(raw: &[Pos2]) -> Vec<[usize; 3]> {
    let mut count = raw.len();
    if count > 1 && raw[0].distance_sq(raw[count - 1]) < 0.0001 {
        count -= 1;
    }
    if count < 3 {
        return Vec::new();
    }
    let ccw = signed_area(&raw[..count]) > 0.0;
    let mut indices: Vec<usize> = (0..count).collect();
    let mut out = Vec::with_capacity(count.saturating_sub(2));
    let mut guard = count * count;
    while indices.len() > 3 && guard > 0 {
        guard -= 1;
        let mut clipped = false;
        for i in 0..indices.len() {
            let a = indices[(i + indices.len() - 1) % indices.len()];
            let b = indices[i];
            let c = indices[(i + 1) % indices.len()];
            if !convex(raw[a], raw[b], raw[c], ccw) {
                continue;
            }
            if indices.iter().copied().any(|candidate| {
                candidate != a
                    && candidate != b
                    && candidate != c
                    && point_in_triangle(raw[candidate], raw[a], raw[b], raw[c])
            }) {
                continue;
            }
            out.push(if ccw { [a, b, c] } else { [c, b, a] });
            indices.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            return Vec::new();
        }
    }
    if indices.len() == 3 {
        out.push(if ccw {
            [indices[0], indices[1], indices[2]]
        } else {
            [indices[2], indices[1], indices[0]]
        });
    }
    out
}

fn signed_area(points: &[Pos2]) -> f32 {
    points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
        .map(|(a, b)| a.x * b.y - b.x * a.y)
        .sum::<f32>()
        * 0.5
}

fn convex(a: Pos2, b: Pos2, c: Pos2, ccw: bool) -> bool {
    let cross = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    if ccw {
        cross > 0.0001
    } else {
        cross < -0.0001
    }
}

fn point_in_triangle(point: Pos2, a: Pos2, b: Pos2, c: Pos2) -> bool {
    let sign = |p1: Pos2, p2: Pos2, p3: Pos2| {
        (p1.x - p3.x) * (p2.y - p3.y) - (p2.x - p3.x) * (p1.y - p3.y)
    };
    let d1 = sign(point, a, b);
    let d2 = sign(point, b, c);
    let d3 = sign(point, c, a);
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

fn alert_contains(alert: &NwsAlert, point: GeoPoint) -> bool {
    alert.polygons.iter().any(|polygon| {
        let Some(outer) = polygon.rings.first() else {
            return false;
        };
        point_in_geo_ring(point, outer)
            && !polygon
                .rings
                .iter()
                .skip(1)
                .any(|hole| point_in_geo_ring(point, hole))
    })
}

fn point_in_geo_ring(point: GeoPoint, ring: &[GeoPoint]) -> bool {
    if ring.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let yi = ring[i].latitude;
        let yj = ring[j].latitude;
        let xi = ring[i].longitude;
        let xj = ring[j].longitude;
        if ((yi > point.latitude) != (yj > point.latitude))
            && point.longitude < (xj - xi) * (point.latitude - yi) / (yj - yi) + xi
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn severity_color(severity: NwsSeverity) -> Color32 {
    match severity {
        NwsSeverity::Extreme => SEVERITY_EXTREME,
        NwsSeverity::Severe => SEVERITY_SEVERE,
        NwsSeverity::Moderate => SEVERITY_MODERATE,
        NwsSeverity::Minor => Style::OK,
        NwsSeverity::Unknown => Style::TEXT_DIM,
    }
}

fn severity_rank(severity: NwsSeverity) -> u8 {
    match severity {
        NwsSeverity::Extreme => 4,
        NwsSeverity::Severe => 3,
        NwsSeverity::Moderate => 2,
        NwsSeverity::Minor => 1,
        NwsSeverity::Unknown => 0,
    }
}

fn paint_warning_banner(painter: &Painter, rect: Rect, alert: &NwsAlert, stale: bool) {
    let tone = if stale {
        Style::TEXT_DIM
    } else {
        severity_color(alert.severity)
    };
    let prefix = if stale {
        "STALE NWS ALERT"
    } else {
        "NWS ALERT"
    };
    let detail = if alert.headline.trim().is_empty() {
        &alert.event
    } else {
        &alert.headline
    };
    let text = format!("{prefix} · {detail}");
    let galley = painter.layout_no_wrap(text, FontId::proportional(Style::BODY), Style::TEXT);
    let width = (galley.size().x + Style::SP_L * 2.0).min(rect.width() - Style::SP_L * 2.0);
    let banner = Rect::from_center_size(
        egui::pos2(rect.center().x, rect.top() + Style::SP_XL),
        egui::vec2(width.max(80.0), galley.size().y + Style::SP_M),
    );
    painter.rect_filled(banner, Style::RADIUS_M, Style::BG.gamma_multiply(0.94));
    painter.rect_stroke(
        banner,
        Style::RADIUS_M,
        Stroke::new(2.0, tone),
        egui::StrokeKind::Inside,
    );
    painter.galley(
        egui::pos2(
            banner.center().x - galley.size().x * 0.5,
            banner.center().y - galley.size().y * 0.5,
        ),
        galley,
        Style::TEXT,
    );
}

fn paint_age_badge(painter: &Painter, rect: Rect, layer: &NwsAlertLayerState, now_ms: i64) {
    let (label, tone) = match (&layer.snapshot, layer.age_ms(now_ms)) {
        (None, _) => ("NWS alerts · no data".to_string(), Style::TEXT_DIM),
        (Some(_), Some(age)) if age > SNAPSHOT_STALE_AFTER_MS => (
            format!("NWS alerts · STALE {}", age_label(age)),
            Style::WARN,
        ),
        (Some(snapshot), Some(age)) if !snapshot.gaps.is_empty() => (
            format!("NWS alerts · {} · degraded", age_label(age)),
            Style::WARN,
        ),
        (Some(snapshot), Some(age)) => (
            format!(
                "NWS alerts · {} · {} active",
                age_label(age),
                snapshot.alerts.len()
            ),
            Style::TEXT,
        ),
        (Some(_), None) => ("NWS alerts · no timestamp".to_string(), Style::WARN),
    };
    let galley = painter.layout_no_wrap(label, FontId::proportional(Style::SMALL), tone);
    let pad = egui::vec2(Style::SP_S, Style::SP_XS);
    // Row two: USGS uses row one when both ambient feeds are visible.
    let offset_y = Style::SP_S + galley.size().y + pad.y * 2.0 + Style::SP_XS;
    let badge = Rect::from_min_size(
        egui::pos2(
            rect.right() - galley.size().x - pad.x * 2.0 - Style::SP_S,
            rect.top() + offset_y,
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
    use mackes_mesh_types::nws_alert::{GeometrySource, NwsAlert};

    use super::*;

    fn alert(now: i64) -> NwsAlert {
        NwsAlert {
            id: "warning".to_string(),
            event: "Severe Thunderstorm Warning".to_string(),
            headline: "Severe Thunderstorm Warning issued".to_string(),
            area_desc: "Test County".to_string(),
            severity: NwsSeverity::Severe,
            urgency: "Immediate".to_string(),
            certainty: "Observed".to_string(),
            sent_at_ms: Some(now - 60_000),
            expires_at_ms: Some(now + 60_000),
            polygons: vec![AlertPolygon {
                rings: vec![vec![
                    GeoPoint {
                        latitude: 0.0,
                        longitude: 0.0,
                    },
                    GeoPoint {
                        latitude: 0.0,
                        longitude: 2.0,
                    },
                    GeoPoint {
                        latitude: 2.0,
                        longitude: 2.0,
                    },
                    GeoPoint {
                        latitude: 2.0,
                        longitude: 0.0,
                    },
                    GeoPoint {
                        latitude: 0.0,
                        longitude: 0.0,
                    },
                ]],
            }],
            geometry_source: Some(GeometrySource::Inline),
        }
    }

    #[test]
    fn point_in_warning_respects_exterior_and_holes() {
        let now = 1_000_000;
        let mut warning = alert(now);
        warning.polygons[0].rings.push(vec![
            GeoPoint {
                latitude: 0.8,
                longitude: 0.8,
            },
            GeoPoint {
                latitude: 0.8,
                longitude: 1.2,
            },
            GeoPoint {
                latitude: 1.2,
                longitude: 1.2,
            },
            GeoPoint {
                latitude: 1.2,
                longitude: 0.8,
            },
        ]);
        assert!(alert_contains(
            &warning,
            GeoPoint {
                latitude: 0.5,
                longitude: 0.5
            }
        ));
        assert!(!alert_contains(
            &warning,
            GeoPoint {
                latitude: 1.0,
                longitude: 1.0
            }
        ));
        assert!(!alert_contains(
            &warning,
            GeoPoint {
                latitude: 3.0,
                longitude: 3.0
            }
        ));
    }

    #[test]
    fn concave_polygon_triangulates_without_filling_outside_notch() {
        let points = vec![
            egui::pos2(0.0, 0.0),
            egui::pos2(4.0, 0.0),
            egui::pos2(4.0, 4.0),
            egui::pos2(2.0, 2.0),
            egui::pos2(0.0, 4.0),
        ];
        assert_eq!(triangulate(&points).len(), 3);
    }

    #[test]
    fn painter_emits_polygon_inside_banner_and_badge() {
        let now = 1_000_000;
        let mut snapshot = NwsAlertSnapshot::empty("rig-1", now);
        snapshot.alerts.push(alert(now));
        let mut layer = NwsAlertLayerState::default();
        layer.fold(snapshot);
        let ctx = egui::Context::default();
        let mut stats = PaintStats::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                stats = paint_layer(
                    ui.painter(),
                    rect,
                    &layer,
                    now,
                    Some(GeoPoint {
                        latitude: 1.0,
                        longitude: 0.5,
                    }),
                    |lat, lon| {
                        Some(egui::pos2(
                            rect.left() + lon as f32 * 10.0,
                            rect.top() + lat as f32 * 10.0,
                        ))
                    },
                );
            });
        });
        assert_eq!(stats.polygons, 1);
        assert!(stats.inside_alert);
        assert!(stats.badge);
        assert!(
            !ctx.tessellate(output.shapes, output.pixels_per_point)
                .is_empty(),
            "layer emits tessellated geometry"
        );
    }

    #[test]
    fn expired_alert_does_not_paint_or_banner() {
        let now = 1_000_000;
        let mut old = alert(now);
        old.expires_at_ms = Some(now - 1);
        let mut snapshot = NwsAlertSnapshot::empty("rig-1", now);
        snapshot.alerts.push(old);
        let mut layer = NwsAlertLayerState::default();
        layer.fold(snapshot);
        let ctx = egui::Context::default();
        let mut stats = PaintStats::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                stats = paint_layer(ui.painter(), rect, &layer, now, None, |_lat, _lon| {
                    Some(rect.center())
                });
            });
        });
        assert_eq!(stats.polygons, 0);
        assert!(!stats.inside_alert);
        assert!(stats.badge);
    }
}
