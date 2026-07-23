//! NIFC WFIGS wildfire-perimeter model and painter (WL-FUNC-012 / OVERLAY-6).
//!
//! The daemon owns the bounded ArcGIS query and GeoJSON normalization. This
//! module folds complete snapshots, derives honest stale/paused state, and
//! paints containment-tinted perimeters through the real basemap projection.

use mackes_mesh_types::wildfire::{
    WildfirePerimeter, WildfirePolygon, WildfireSnapshot, ATTRIBUTION,
};
use mde_egui::egui::{self, Color32, FontId, Mesh, Painter, Pos2, Rect, Shape, Stroke};
use mde_egui::Style;

/// Three missed fifteen-minute polls make the retained perimeter set stale.
pub const SNAPSHOT_STALE_AFTER_MS: i64 = 45 * 60 * 1_000;

const WILDFIRE_FILL: Color32 = Color32::from_rgb(0xE8, 0x52, 0x2E); // style-leak-ok: map-content-color
const CONTAINMENT_MID: Color32 = Color32::from_rgb(0xF2, 0x82, 0x22); // style-leak-ok: map-content-color

/// Retained complete WFIGS snapshot.
#[derive(Debug, Clone, Default)]
pub struct WildfireLayerState {
    /// Latest vehicle-centred current-perimeter snapshot.
    pub snapshot: Option<WildfireSnapshot>,
}

impl WildfireLayerState {
    /// Replace the previous server result wholesale so revised or removed
    /// perimeters converge.
    pub fn fold(&mut self, snapshot: WildfireSnapshot) {
        self.snapshot = Some(snapshot);
    }

    /// Age since the last successful query or validator-backed 304.
    #[must_use]
    pub fn age_ms(&self, now_ms: i64) -> Option<i64> {
        self.snapshot
            .as_ref()
            .map(|snapshot| now_ms.saturating_sub(snapshot.fetched_at_ms).max(0))
    }

    /// Whether three expected refreshes have been missed.
    #[must_use]
    pub fn stale(&self, now_ms: i64) -> bool {
        self.age_ms(now_ms)
            .is_some_and(|age| age > SNAPSHOT_STALE_AFTER_MS)
    }

    /// Whether a failed refresh or loss of the fresh same-host fix has paused
    /// the last-good set. Paused data dims immediately, before the age cutoff.
    #[must_use]
    pub fn paused(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .gaps
                .iter()
                .any(|gap| gap.starts_with("NIFC wildfire paused:"))
        })
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
    /// Polygon exteriors painted or outlined in the viewport.
    pub polygons: usize,
    /// Whether the honest age/no-data badge painted.
    pub badge: bool,
}

/// Paint WFIGS polygons and one honest freshness/provider badge.
pub fn paint_layer<F>(
    painter: &Painter,
    rect: Rect,
    layer: &WildfireLayerState,
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
        let clip = rect.intersect(painter.clip_rect());
        let perimeter_painter = painter.with_clip_rect(clip);
        for perimeter in &snapshot.perimeters {
            for polygon in &perimeter.polygons {
                if paint_polygon(
                    &perimeter_painter,
                    rect,
                    polygon,
                    perimeter,
                    dimmed,
                    &mut project,
                ) {
                    stats.polygons += 1;
                }
            }
        }
    }
    paint_age_badge(painter, rect, layer, now_ms);
    stats.badge = true;
    stats
}

fn paint_polygon<F>(
    painter: &Painter,
    rect: Rect,
    polygon: &WildfirePolygon,
    perimeter: &WildfirePerimeter,
    dimmed: bool,
    project: &mut F,
) -> bool
where
    F: FnMut(f64, f64) -> Option<Pos2>,
{
    let projected_rings: Vec<Vec<Pos2>> = polygon
        .rings
        .iter()
        .filter_map(|ring| {
            let points: Vec<_> = ring
                .iter()
                .filter_map(|point| project(point.latitude, point.longitude))
                .filter(|point| !point.any_nan())
                .collect();
            (points.len() >= 3).then_some(points)
        })
        .collect();
    let Some(outer) = projected_rings.first() else {
        return false;
    };
    if !Rect::from_points(outer).intersects(rect.expand(24.0)) {
        return false;
    }

    let outline = if dimmed {
        Style::TEXT_DIM
    } else {
        containment_tone(perimeter.percent_contained)
    };
    // Hole-free perimeters get a concave-safe fill. With interior rings, the
    // outline is still accurate but the fill is omitted so exclusion holes are
    // never falsely shown as burning.
    if projected_rings.len() == 1 {
        paint_concave_fill(
            painter,
            outer,
            WILDFIRE_FILL.gamma_multiply(if dimmed { 0.07 } else { 0.20 }),
        );
    }
    for ring in projected_rings {
        painter.add(Shape::closed_line(
            ring,
            Stroke::new(
                2.0,
                outline.gamma_multiply(if dimmed { 0.38 } else { 0.92 }),
            ),
        ));
    }
    true
}

fn containment_tone(percent: Option<f32>) -> Color32 {
    match percent {
        Some(percent) if percent >= 90.0 => Style::OK,
        Some(percent) if percent >= 50.0 => Style::WARN,
        Some(percent) if percent >= 25.0 => CONTAINMENT_MID,
        Some(_) => Style::DANGER,
        None => WILDFIRE_FILL,
    }
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
        for index in 0..indices.len() {
            let a = indices[(index + indices.len() - 1) % indices.len()];
            let b = indices[index];
            let c = indices[(index + 1) % indices.len()];
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
            indices.remove(index);
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

fn paint_age_badge(painter: &Painter, rect: Rect, layer: &WildfireLayerState, now_ms: i64) {
    let (label, tone) = match (&layer.snapshot, layer.age_ms(now_ms)) {
        (None, _) => (
            "Wildfire · NIFC no data · FIRMS key needed".to_string(),
            Style::TEXT_DIM,
        ),
        (Some(_), Some(age)) if layer.paused() => (
            format!(
                "Wildfire · NIFC PAUSED {} · FIRMS key needed",
                age_label(age)
            ),
            Style::WARN,
        ),
        (Some(_), Some(age)) if age > SNAPSHOT_STALE_AFTER_MS => (
            format!(
                "Wildfire · NIFC STALE {} · FIRMS key needed",
                age_label(age)
            ),
            Style::WARN,
        ),
        (Some(snapshot), Some(age)) if !snapshot.gaps.is_empty() => (
            format!(
                "Wildfire · NIFC {} · {} perimeters · degraded · FIRMS key needed",
                age_label(age),
                snapshot.perimeters.len()
            ),
            Style::WARN,
        ),
        (Some(snapshot), Some(age)) => (
            format!(
                "Wildfire · NIFC {} · {} perimeters · FIRMS key needed",
                age_label(age),
                snapshot.perimeters.len()
            ),
            Style::TEXT,
        ),
        (Some(_), None) => (
            "Wildfire · NIFC no timestamp · FIRMS key needed".to_string(),
            Style::WARN,
        ),
    };
    let galley = painter.layout_no_wrap(label, FontId::proportional(Style::SMALL), tone);
    let pad = egui::vec2(Style::SP_S, Style::SP_XS);
    let row_height = galley.size().y + pad.y * 2.0 + Style::SP_XS;
    let badge = Rect::from_min_size(
        egui::pos2(
            rect.right() - galley.size().x - pad.x * 2.0 - Style::SP_S,
            rect.top() + Style::SP_S + row_height * 7.0,
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
    use mackes_mesh_types::wildfire::{WildfirePoint, WildfirePolygon};

    use super::*;

    fn snapshot(now_ms: i64) -> WildfireSnapshot {
        let mut snapshot = WildfireSnapshot::empty("rig-1", now_ms, 44.0, -120.0, 200);
        snapshot.perimeters.push(WildfirePerimeter {
            id: "42".to_string(),
            incident_name: "Morrill".to_string(),
            unique_fire_id: None,
            acres: Some(642_029.0),
            percent_contained: Some(50.0),
            perimeter_updated_at_ms: Some(now_ms - 60_000),
            polygons: vec![WildfirePolygon {
                rings: vec![vec![
                    WildfirePoint {
                        latitude: 43.9,
                        longitude: -120.1,
                    },
                    WildfirePoint {
                        latitude: 43.9,
                        longitude: -119.9,
                    },
                    WildfirePoint {
                        latitude: 44.1,
                        longitude: -119.9,
                    },
                    WildfirePoint {
                        latitude: 43.9,
                        longitude: -120.1,
                    },
                ]],
            }],
        });
        snapshot
    }

    #[test]
    fn fold_replaces_whole_set_and_paused_dims_immediately() {
        let now = 10_000_000;
        let mut layer = WildfireLayerState::default();
        let mut first = snapshot(now);
        first.perimeters.push(WildfirePerimeter {
            id: "removed".to_string(),
            ..first.perimeters[0].clone()
        });
        layer.fold(first);
        layer.fold(snapshot(now + 1));
        assert_eq!(
            layer.snapshot.as_ref().expect("snapshot").perimeters.len(),
            1
        );
        assert!(!layer.stale(now + 1));
        layer
            .snapshot
            .as_mut()
            .expect("snapshot")
            .gaps
            .push("NIFC wildfire paused: ArcGIS HTTP 429".to_string());
        assert!(layer.paused());
    }

    #[test]
    fn containment_tone_preserves_unknown_and_reported_extremes() {
        assert_eq!(containment_tone(Some(0.0)), Style::DANGER);
        assert_eq!(containment_tone(Some(100.0)), Style::OK);
        assert_eq!(containment_tone(None), WILDFIRE_FILL);
    }

    #[test]
    fn painter_emits_projected_perimeter_and_honest_badge() {
        let now = 10_000_000;
        let mut layer = WildfireLayerState::default();
        layer.fold(snapshot(now));
        let ctx = egui::Context::default();
        let mut observed = PaintStats::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect();
                observed = paint_layer(ui.painter(), rect, &layer, now, |lat, lon| {
                    Some(egui::pos2(
                        rect.center().x + ((lon + 120.0) * 100.0) as f32,
                        rect.center().y - ((lat - 44.0) * 100.0) as f32,
                    ))
                });
            });
        });
        assert_eq!(observed.polygons, 1);
        assert!(observed.badge);
        assert!(ctx.tessellate(output.shapes, output.pixels_per_point).len() >= 3);
    }
}
