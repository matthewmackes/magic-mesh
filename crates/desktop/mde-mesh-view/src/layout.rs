//! Pure layout math for [`MeshView`](crate::MeshView).
//!
//! Ring placement, the normalized→screen mapping, and pulse interpolation all
//! live here as **GPU-free pure functions over plain data**, so the geometry is
//! unit-tested without a render context (no eframe, no window). The render path
//! in `view.rs` is a thin painter over these results.

use std::f32::consts::{FRAC_PI_2, TAU};

use mde_egui::egui::{pos2, vec2, Pos2, Rect, Vec2};

use crate::state::{MeshState, Role};

/// Normalized radius of the **inner** ring that auto-placed lighthouses sit on
/// when there is more than one. A lone lighthouse goes dead-centre instead.
pub const LH_RING_RADIUS: f32 = 0.12;
/// Normalized radius of the **outer** ring that auto-placed peers sit on.
pub const PEER_RING_RADIUS: f32 = 0.34;

/// The first ring slot points straight up (12 o'clock).
const START_ANGLE: f32 = -FRAC_PI_2;
/// Normalized canvas centre.
const CENTER: Vec2 = Vec2 { x: 0.5, y: 0.5 };

/// Evenly place `count` points on a circle of `radius` around `center`,
/// starting at `start_angle` and advancing clockwise. Empty when `count == 0`.
#[must_use]
pub fn ring(center: Vec2, radius: f32, count: usize, start_angle: f32) -> Vec<Vec2> {
    (0..count)
        .map(|i| {
            let frac = i as f32 / count as f32;
            let ang = start_angle + TAU * frac;
            center + vec2(ang.cos() * radius, ang.sin() * radius)
        })
        .collect()
}

/// Compute a **normalized** `0.0..=1.0` position for every node, aligned to
/// `state.nodes`:
///
/// - a node with an explicit [`pos`](crate::MeshNode::pos) keeps it;
/// - a lone auto-placed lighthouse sits at the centre; several share the inner
///   ring ([`LH_RING_RADIUS`]);
/// - auto-placed Workstation peers share the outer ring
///   ([`PEER_RING_RADIUS`]).
#[must_use]
pub fn auto_layout(state: &MeshState) -> Vec<Vec2> {
    let mut out = vec![CENTER; state.nodes.len()];
    let mut lighthouses = Vec::new();
    let mut peers = Vec::new();
    for (i, node) in state.nodes.iter().enumerate() {
        if let Some(p) = node.pos {
            out[i] = p;
        } else if matches!(node.role, Role::Lighthouse) {
            lighthouses.push(i);
        } else {
            peers.push(i);
        }
    }

    if lighthouses.len() == 1 {
        out[lighthouses[0]] = CENTER;
    } else {
        for (slot, &i) in ring(CENTER, LH_RING_RADIUS, lighthouses.len(), START_ANGLE)
            .iter()
            .zip(&lighthouses)
        {
            out[i] = *slot;
        }
    }

    for (slot, &i) in ring(CENTER, PEER_RING_RADIUS, peers.len(), START_ANGLE)
        .iter()
        .zip(&peers)
    {
        out[i] = *slot;
    }

    out
}

/// Map a normalized `0.0..=1.0` point into the (already inset) `content` rect.
#[must_use]
pub fn to_screen(norm: Vec2, content: Rect) -> Pos2 {
    pos2(
        content.min.x + norm.x * content.width(),
        content.min.y + norm.y * content.height(),
    )
}

/// Screen-space centre of every node.
///
/// [`auto_layout`] resolved into `area` inset by `margin` (the band reserved
/// for discs + labels so they stay inside the canvas). Returned `Vec` is
/// aligned to `state.nodes`.
#[must_use]
pub fn place(state: &MeshState, area: Rect, margin: f32) -> Vec<Pos2> {
    let content = area.shrink(margin);
    auto_layout(state)
        .iter()
        .map(|n| to_screen(*n, content))
        .collect()
}

/// The point a fraction `phase` (`0.0..=1.0`) of the way along the segment
/// `a → b` — where a travelling activity pulse sits this frame.
#[must_use]
pub fn pulse_pos(a: Pos2, b: Pos2, phase: f32) -> Pos2 {
    a + (b - a) * phase
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::{
        auto_layout, place, pulse_pos, ring, to_screen, CENTER, LH_RING_RADIUS, PEER_RING_RADIUS,
        START_ANGLE,
    };
    use crate::state::{Health, MeshLink, MeshNode, MeshState, Role};
    use mde_egui::egui::{pos2, vec2, Rect};

    const EPS: f32 = 1e-4;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < EPS
    }

    fn lighthouse(id: &str) -> MeshNode {
        MeshNode::new(id, id, Role::Lighthouse, Health::Ok)
    }
    fn peer(id: &str) -> MeshNode {
        MeshNode::new(id, id, Role::Workstation, Health::Ok)
    }

    #[test]
    fn ring_is_empty_for_zero_count() {
        assert!(ring(CENTER, 0.3, 0, START_ANGLE).is_empty());
    }

    #[test]
    fn ring_places_count_points_all_at_radius() {
        let c = vec2(0.5, 0.5);
        let pts = ring(c, 0.3, 6, START_ANGLE);
        assert_eq!(pts.len(), 6);
        for p in pts {
            assert!(close((p - c).length(), 0.3), "point off the ring: {p:?}");
        }
    }

    #[test]
    fn ring_first_slot_points_straight_up() {
        let c = vec2(0.5, 0.5);
        let pts = ring(c, 0.3, 4, START_ANGLE);
        // 12 o'clock: same x, smaller y (up in screen space).
        assert!(close(pts[0].x, c.x));
        assert!(close(pts[0].y, c.y - 0.3));
    }

    #[test]
    fn ring_spaces_four_points_evenly() {
        let c = vec2(0.5, 0.5);
        let pts = ring(c, 0.25, 4, START_ANGLE);
        // up, right, down, left.
        assert!(
            close(pts[1].x, c.x + 0.25) && close(pts[1].y, c.y),
            "right: {:?}",
            pts[1]
        );
        assert!(
            close(pts[2].x, c.x) && close(pts[2].y, c.y + 0.25),
            "down: {:?}",
            pts[2]
        );
        assert!(
            close(pts[3].x, c.x - 0.25) && close(pts[3].y, c.y),
            "left: {:?}",
            pts[3]
        );
    }

    #[test]
    fn lone_lighthouse_sits_at_centre() {
        let s = MeshState {
            nodes: vec![lighthouse("lh")],
            links: vec![],
        };
        assert_eq!(auto_layout(&s)[0], vec2(0.5, 0.5));
    }

    #[test]
    fn explicit_position_is_preserved() {
        let s = MeshState {
            nodes: vec![lighthouse("lh").at(vec2(0.2, 0.8))],
            links: vec![],
        };
        assert_eq!(auto_layout(&s)[0], vec2(0.2, 0.8));
    }

    #[test]
    fn auto_peers_land_on_the_outer_ring() {
        let s = MeshState {
            nodes: vec![lighthouse("lh"), peer("a"), peer("b"), peer("c")],
            links: vec![],
        };
        // The lighthouse is index 0 (centred); the three peers follow.
        for p in auto_layout(&s).iter().skip(1) {
            assert!(
                close((*p - CENTER).length(), PEER_RING_RADIUS),
                "peer not on the outer ring: {p:?}"
            );
        }
    }

    #[test]
    fn multiple_lighthouses_land_on_the_inner_ring() {
        let s = MeshState {
            nodes: vec![lighthouse("a"), lighthouse("b"), lighthouse("c")],
            links: vec![],
        };
        for p in auto_layout(&s) {
            assert!(
                close((p - CENTER).length(), LH_RING_RADIUS),
                "lighthouse not on the inner ring: {p:?}"
            );
        }
    }

    #[test]
    fn to_screen_maps_corners_and_centre() {
        let r = Rect::from_min_size(pos2(10.0, 20.0), vec2(200.0, 100.0));
        assert_eq!(to_screen(vec2(0.0, 0.0), r), r.min);
        assert_eq!(to_screen(vec2(1.0, 1.0), r), r.max);
        let mid = to_screen(vec2(0.5, 0.5), r);
        assert!(close(mid.x, r.center().x) && close(mid.y, r.center().y));
    }

    #[test]
    fn place_link_endpoints_match_node_centres() {
        // Two explicit-pos nodes joined by a link: the link's drawn endpoints
        // are exactly those node centres (links are lines between node centres).
        let s = MeshState {
            nodes: vec![
                peer("a").at(vec2(0.25, 0.25)),
                peer("b").at(vec2(0.75, 0.60)),
            ],
            links: vec![MeshLink::new("a", "b", 0.5)],
        };
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0));
        let margin = 40.0;
        let centres = place(&s, area, margin);
        let content = area.shrink(margin);
        assert_eq!(centres[0], to_screen(vec2(0.25, 0.25), content));
        assert_eq!(centres[1], to_screen(vec2(0.75, 0.60), content));
    }

    #[test]
    fn pulse_pos_walks_from_a_to_b() {
        let a = pos2(0.0, 0.0);
        let b = pos2(10.0, 20.0);
        assert_eq!(pulse_pos(a, b, 0.0), a);
        assert_eq!(pulse_pos(a, b, 1.0), b);
        let mid = pulse_pos(a, b, 0.5);
        assert!(close(mid.x, 5.0) && close(mid.y, 10.0));
    }
}
