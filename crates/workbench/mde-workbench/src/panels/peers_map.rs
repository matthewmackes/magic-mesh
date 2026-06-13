//! PD-7 (Q24/L17/L18) — the live mesh map.
//!
//! The Peers panel's Map view: a force-directed canvas where edge
//! length pulls proportional to measured overlay RTT (the mesh's
//! *shape* becomes information — near peers cluster, slow paths
//! stretch), nodes carry presence styling, edges carry RTT labels,
//! and an unreachable peer's edge renders dashed-red with an ×.
//! Clicking a node selects that peer in the directory (W87 — the
//! map is a lens, the detail pane is the surface).
//!
//! RTT comes from the `mesh_latency` worker's snapshot cache (the
//! PD-6 transport probe's output). Layout is a deterministic spring
//! relaxation computed on data change — no animation loop (the W22
//! render budget applies; flow particles join with the PD-10
//! wallpaper engine).

use std::collections::HashMap;

use cosmic::iced::widget::canvas::{self, Frame, Path, Stroke, Text};
use cosmic::iced::{mouse, Pixels, Point, Rectangle, Renderer, Theme};
use mde_theme::Palette;

use crate::cosmic_compat::prelude::*;

/// One node's map datum.
#[derive(Debug, Clone, PartialEq)]
pub struct MapNode {
    pub hostname: String,
    /// `online` | `idle` | `offline`.
    pub presence: String,
    /// Measured overlay RTT from self, ms (`None` = unreachable /
    /// self / unmeasured).
    pub rtt_ms: Option<f64>,
    /// `true` for this machine (the map's anchor).
    pub is_self: bool,
    /// PD-7/L18 — this peer's recent overlay throughput, normalized to
    /// 0.0..=1.0 (from its Netdata `system.net`). Drives the flow-particle
    /// density + speed on its self→peer edge; 0.0 = idle (no particles, so
    /// an idle mesh draws nothing extra — the L22 budget).
    pub flow: f64,
}

/// Deterministic seed angle for a hostname (stable across refreshes
/// — no `Math.random`, no churn).
fn seed_angle(hostname: &str) -> f32 {
    let h: u32 = hostname.bytes().fold(2_166_136_261_u32, |acc, b| {
        (acc ^ u32::from(b)).wrapping_mul(16_777_619)
    });
    (h % 6283) as f32 / 1000.0
}

/// Ideal spring length for an RTT (px): 120 px floor + 6 px per ms,
/// capped so a 200 ms relay hop still fits a laptop canvas.
fn ideal_len(rtt_ms: Option<f64>) -> f32 {
    let ms = rtt_ms.unwrap_or(40.0).min(120.0) as f32;
    120.0 + ms * 6.0
}

/// Compute the force layout (pure, deterministic): self anchored at
/// the origin; springs along self→peer edges sized by RTT; pairwise
/// repulsion keeps non-edged peers apart. Returns unit-space
/// positions centered on (0,0) for the draw pass to scale.
#[must_use]
pub fn layout(nodes: &[MapNode]) -> HashMap<String, (f32, f32)> {
    let mut pos: HashMap<String, (f32, f32)> = nodes
        .iter()
        .map(|n| {
            if n.is_self {
                (n.hostname.clone(), (0.0, 0.0))
            } else {
                let a = seed_angle(&n.hostname);
                let r = ideal_len(n.rtt_ms);
                (n.hostname.clone(), (a.cos() * r, a.sin() * r))
            }
        })
        .collect();

    for _ in 0..200 {
        let snapshot = pos.clone();
        for n in nodes {
            if n.is_self {
                continue; // anchored
            }
            let (mut x, mut y) = snapshot[&n.hostname];
            let (mut fx, mut fy) = (0.0_f32, 0.0_f32);
            // Spring to self (edge length = RTT ideal).
            let want = ideal_len(n.rtt_ms);
            let d = (x * x + y * y).sqrt().max(1.0);
            let stretch = (d - want) / d;
            fx -= x * stretch * 0.2;
            fy -= y * stretch * 0.2;
            // Repulsion from every other peer.
            for m in nodes {
                if m.hostname == n.hostname || m.is_self {
                    continue;
                }
                let (ox, oy) = snapshot[&m.hostname];
                let dx = x - ox;
                let dy = y - oy;
                let dist2 = (dx * dx + dy * dy).max(25.0);
                let push = 12_000.0 / dist2;
                let dist = dist2.sqrt();
                fx += dx / dist * push;
                fy += dy / dist * push;
            }
            x += fx;
            y += fy;
            pos.insert(n.hostname.clone(), (x, y));
        }
    }
    pos
}

/// The canvas program. Click hit-testing emits the hostname via the
/// `on_click` constructor closure into the panel's message space.
pub struct MapProgram {
    pub nodes: Vec<MapNode>,
    pub positions: HashMap<String, (f32, f32)>,
    pub palette: Palette,
    /// PD-7/L18 — the flow-particle animation phase (0.0..=1.0), advanced
    /// by the panel's frame tick. Particles ride each active edge at
    /// `(phase + k/N) mod 1`; a fresh value each frame moves them.
    pub flow_phase: f32,
}

/// Node hit radius (px, post-scale).
const HIT_R: f32 = 26.0;

/// Edge hit half-width (px) — how close a click must land to a self→peer
/// segment to open its trace card (PD-7/L19-20).
const EDGE_HIT: f32 = 8.0;

impl MapProgram {
    /// Scale unit-space positions into the canvas rect.
    fn projected(&self, bounds: &Rectangle) -> HashMap<String, Point> {
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (-1.0_f32, -1.0_f32, 1.0_f32, 1.0_f32);
        for (x, y) in self.positions.values() {
            min_x = min_x.min(*x);
            min_y = min_y.min(*y);
            max_x = max_x.max(*x);
            max_y = max_y.max(*y);
        }
        let span_x = (max_x - min_x).max(1.0);
        let span_y = (max_y - min_y).max(1.0);
        let pad = 60.0;
        let sx = (bounds.width - pad * 2.0) / span_x;
        let sy = (bounds.height - pad * 2.0) / span_y;
        let s = sx.min(sy).min(1.5); // never blow tiny meshes up absurdly
        let cx = (min_x + max_x) / 2.0;
        let cy = (min_y + max_y) / 2.0;
        self.positions
            .iter()
            .map(|(host, (x, y))| {
                (
                    host.clone(),
                    Point::new(
                        bounds.width / 2.0 + (x - cx) * s,
                        bounds.height / 2.0 + (y - cy) * s,
                    ),
                )
            })
            .collect()
    }

    /// The hostname under `point`, if any (the panel's click handler).
    #[must_use]
    pub fn hit(&self, bounds: &Rectangle, point: Point) -> Option<String> {
        let proj = self.projected(bounds);
        self.nodes
            .iter()
            .filter_map(|n| {
                let p = proj.get(&n.hostname)?;
                let d2 = (p.x - point.x).powi(2) + (p.y - point.y).powi(2);
                (d2 <= HIT_R * HIT_R).then(|| (d2, n.hostname.clone()))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, h)| h)
    }

    /// PD-7/L19-20 — the peer whose self→peer **edge** is under `point`
    /// (within [`EDGE_HIT`] px of the segment), if any. Used for the
    /// trace-card open: a click that misses every node but lands on an
    /// edge opens that edge's trace. Self has no edge to itself.
    #[must_use]
    pub fn hit_edge(&self, bounds: &Rectangle, point: Point) -> Option<String> {
        let proj = self.projected(bounds);
        let origin = self
            .nodes
            .iter()
            .find(|n| n.is_self)
            .and_then(|n| proj.get(&n.hostname))
            .copied()?;
        self.nodes
            .iter()
            .filter(|n| !n.is_self)
            .filter_map(|n| {
                let to = proj.get(&n.hostname)?;
                let d = point_segment_dist(point, origin, *to);
                (d <= EDGE_HIT).then(|| (d, n.hostname.clone()))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, h)| h)
    }
}

/// Perpendicular distance from `p` to the segment `a`–`b` (px).
fn point_segment_dist(p: Point, a: Point, b: Point) -> f32 {
    let (vx, vy) = (b.x - a.x, b.y - a.y);
    let (wx, wy) = (p.x - a.x, p.y - a.y);
    let len2 = vx * vx + vy * vy;
    if len2 <= f32::EPSILON {
        return (wx * wx + wy * wy).sqrt();
    }
    let t = ((wx * vx + wy * vy) / len2).clamp(0.0, 1.0);
    let (cx, cy) = (a.x + t * vx, a.y + t * vy);
    ((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt()
}

impl canvas::Program<crate::Message> for MapProgram {
    type State = ();

    fn update(
        &self,
        _state: &mut Self::State,
        event: &cosmic::iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<crate::Message>> {
        if let cosmic::iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event
        {
            let pos = cursor.position_in(bounds)?;
            let rect = Rectangle::with_size(bounds.size());
            // A node click selects the peer (W87); a click that misses every
            // node but lands on an edge opens that edge's trace card (L19-20).
            if let Some(host) = self.hit(&rect, pos) {
                return Some(canvas::Action::publish(crate::Message::Peers(
                    super::peers::Message::Select(host),
                )));
            }
            if let Some(host) = self.hit_edge(&rect, pos) {
                return Some(canvas::Action::publish(crate::Message::Peers(
                    super::peers::Message::EdgeClicked(host),
                )));
            }
        }
        None
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let rect = Rectangle::with_size(bounds.size());
        let proj = self.projected(&rect);
        let p = &self.palette;
        let self_point = self
            .nodes
            .iter()
            .find(|n| n.is_self)
            .and_then(|n| proj.get(&n.hostname))
            .copied();

        // Edges self→peer first (under the nodes).
        if let Some(origin) = self_point {
            for n in self.nodes.iter().filter(|n| !n.is_self) {
                let Some(&to) = proj.get(&n.hostname) else {
                    continue;
                };
                let reachable = n.rtt_ms.is_some();
                let color = if reachable {
                    p.border.into_cosmic_color()
                } else {
                    p.danger.into_cosmic_color()
                };
                frame.stroke(
                    &Path::line(origin, to),
                    Stroke::default()
                        .with_color(color)
                        .with_width(if reachable { 1.5 } else { 1.0 }),
                );
                // PD-7/L18 — flow particles: dots riding the edge while the
                // peer moves real overlay traffic (Netdata-sourced `flow`).
                // Density + along-edge speed scale with `flow`; an idle edge
                // (flow ≈ 0) draws nothing, so the canvas stays cheap (L22).
                if reachable && n.flow > 0.02 {
                    let count = 1 + (n.flow * 5.0).round() as usize; // 1..=6
                    let speed = 0.5 + n.flow as f32; // laps-per-cycle
                    for k in 0..count {
                        let base = (k as f32) / count as f32;
                        let t = (self.flow_phase * speed + base).fract();
                        let px = origin.x + (to.x - origin.x) * t;
                        let py = origin.y + (to.y - origin.y) * t;
                        frame.fill(
                            &Path::circle(Point::new(px, py), 2.0),
                            p.accent.into_cosmic_color(),
                        );
                    }
                }
                // RTT label at the midpoint; × for unreachable.
                let mid = Point::new((origin.x + to.x) / 2.0, (origin.y + to.y) / 2.0);
                let label = n.rtt_ms.map_or("×".to_string(), |ms| format!("{ms:.0} ms"));
                frame.fill_text(Text {
                    content: label,
                    position: mid,
                    color: p.text_muted.into_cosmic_color(),
                    size: Pixels(11.0),
                    ..Text::default()
                });
            }
        }

        // Nodes.
        for n in &self.nodes {
            let Some(&at) = proj.get(&n.hostname) else {
                continue;
            };
            let (fill, ring) = match n.presence.as_str() {
                "online" => (p.success, p.border),
                "idle" => (p.warning, p.border),
                _ => (p.text_muted, p.danger),
            };
            let r = if n.is_self { 14.0 } else { 10.0 };
            frame.fill(&Path::circle(at, r), fill.into_cosmic_color());
            frame.stroke(
                &Path::circle(at, r + 2.0),
                Stroke::default()
                    .with_color(ring.into_cosmic_color())
                    .with_width(1.0),
            );
            frame.fill_text(Text {
                content: if n.is_self {
                    format!("{} (this machine)", n.hostname)
                } else {
                    n.hostname.clone()
                },
                position: Point::new(at.x, at.y + r + 6.0),
                color: p.text.into_cosmic_color(),
                size: Pixels(12.0),
                ..Text::default()
            });
        }
        vec![frame.into_geometry()]
    }
}

/// Read the mesh-latency snapshot cache (the PD-6 probe output) into
/// a host→RTT map. Missing cache = empty (edges render unmeasured).
#[must_use]
pub fn read_latency_cache() -> HashMap<String, Option<f64>> {
    let Some(home) = std::env::var_os("HOME") else {
        return HashMap::new();
    };
    let path = std::path::PathBuf::from(home).join(".cache/mde/mesh-latency.json");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    parse_latency_cache(&raw)
}

/// Parse the snapshot JSON (pure).
#[must_use]
pub fn parse_latency_cache(raw: &str) -> HashMap<String, Option<f64>> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return HashMap::new();
    };
    v.get("peers")
        .and_then(|p| p.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(host, entry)| (host.clone(), entry.get("rtt_ms").and_then(|r| r.as_f64())))
                .collect()
        })
        .unwrap_or_default()
}

/// NET-3 (PD-6/PD-7) — one peer's underlay path, as the `mesh_latency` cache
/// records it from the Nebula debug-SSH hostmap.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PathInfo {
    /// `"direct"` / `"relay"` / `"overlay"` (unknown).
    pub path: String,
    /// Chosen remote UDP endpoint (`"ip:port"`), when direct.
    pub endpoint: Option<String>,
    /// Relay peer this tunnel routes through, when relayed.
    pub relay_via: Option<String>,
}

/// Read the host→path map from the same `mesh-latency.json` cache (NET-3).
/// Missing cache / older snapshot = empty (the trace card falls back to the
/// honest "overlay" line).
#[must_use]
pub fn read_latency_paths() -> HashMap<String, PathInfo> {
    let Some(home) = std::env::var_os("HOME") else {
        return HashMap::new();
    };
    let path = std::path::PathBuf::from(home).join(".cache/mde/mesh-latency.json");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    parse_latency_paths(&raw)
}

/// Parse the snapshot's per-peer path fields (pure).
#[must_use]
pub fn parse_latency_paths(raw: &str) -> HashMap<String, PathInfo> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return HashMap::new();
    };
    v.get("peers")
        .and_then(|p| p.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(host, e)| {
                    let info = PathInfo {
                        path: e
                            .get("path")
                            .and_then(|p| p.as_str())
                            .unwrap_or("overlay")
                            .to_string(),
                        endpoint: e
                            .get("endpoint")
                            .and_then(|x| x.as_str())
                            .map(str::to_string),
                        relay_via: e
                            .get("relay_via")
                            .and_then(|x| x.as_str())
                            .map(str::to_string),
                    };
                    (host.clone(), info)
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_latency_paths_reads_direct_and_relay() {
        let raw = r#"{"checked_at":1,"peers":{
            "anvil":{"rtt_ms":12.0,"ok":true,"path":"direct","endpoint":"203.0.113.5:4242"},
            "forge":{"rtt_ms":null,"ok":false,"path":"relay","relay_via":"10.42.0.1"},
            "pine":{"rtt_ms":9.0,"ok":true,"path":"overlay"}
        }}"#;
        let m = parse_latency_paths(raw);
        assert_eq!(m["anvil"].path, "direct");
        assert_eq!(m["anvil"].endpoint.as_deref(), Some("203.0.113.5:4242"));
        assert_eq!(m["forge"].path, "relay");
        assert_eq!(m["forge"].relay_via.as_deref(), Some("10.42.0.1"));
        assert_eq!(m["pine"].path, "overlay");
        assert_eq!(m["pine"].endpoint, None);
    }

    #[test]
    fn parse_latency_paths_defaults_overlay_for_old_cache() {
        // A pre-NET-3 snapshot without path fields reads honestly as overlay.
        let raw = r#"{"checked_at":1,"peers":{"oak":{"rtt_ms":5.0,"ok":true}}}"#;
        let m = parse_latency_paths(raw);
        assert_eq!(m["oak"].path, "overlay");
    }

    fn node(host: &str, presence: &str, rtt: Option<f64>, is_self: bool) -> MapNode {
        MapNode {
            hostname: host.into(),
            presence: presence.into(),
            rtt_ms: rtt,
            is_self,
            flow: 0.0,
        }
    }

    #[test]
    fn layout_is_deterministic_and_rtt_proportional() {
        let nodes = vec![
            node("self", "online", None, true),
            node("near", "online", Some(5.0), false),
            node("far", "online", Some(100.0), false),
        ];
        let a = layout(&nodes);
        let b = layout(&nodes);
        assert_eq!(a, b, "same input, same layout — no churn");
        let d = |h: &str| {
            let (x, y) = a[h];
            (x * x + y * y).sqrt()
        };
        assert!(
            d("near") < d("far"),
            "5 ms must sit closer than 100 ms: {} vs {}",
            d("near"),
            d("far")
        );
        assert_eq!(a["self"], (0.0, 0.0), "self anchors the map");
    }

    #[test]
    fn latency_cache_parses_reachable_and_unreachable() {
        let raw = r#"{"checked_at":1,"peers":{"oak":{"rtt_ms":14.3,"ok":true},"elm":{"rtt_ms":null,"ok":false}}}"#;
        let m = parse_latency_cache(raw);
        assert_eq!(m["oak"], Some(14.3));
        assert_eq!(m["elm"], None);
        assert!(parse_latency_cache("junk").is_empty());
    }

    #[test]
    fn hit_testing_finds_the_nearest_node_only_within_radius() {
        let nodes = vec![
            node("self", "online", None, true),
            node("oak", "online", Some(10.0), false),
        ];
        let positions = layout(&nodes);
        let prog = MapProgram {
            nodes,
            positions,
            palette: Palette::dark(),
            flow_phase: 0.0,
        };
        let bounds = Rectangle::with_size(cosmic::iced::Size::new(800.0, 600.0));
        // A click far outside any node hits nothing.
        assert_eq!(prog.hit(&bounds, Point::new(5.0, 5.0)), None);
        // Some grid point lands on a node (exact projection is an
        // implementation detail; existence is the contract).
        let mut found = None;
        'grid: for gx in (0..800).step_by(10) {
            for gy in (0..600).step_by(10) {
                if let Some(h) = prog.hit(&bounds, Point::new(gx as f32, gy as f32)) {
                    found = Some(h);
                    break 'grid;
                }
            }
        }
        assert!(found.is_some(), "a node must be clickable somewhere");
    }

    #[test]
    fn point_segment_dist_is_perpendicular_and_clamped() {
        let a = Point::new(0.0, 0.0);
        let b = Point::new(10.0, 0.0);
        // On the segment → ~0.
        assert!(point_segment_dist(Point::new(5.0, 0.0), a, b) < 0.01);
        // Above the midpoint → the perpendicular offset.
        assert!((point_segment_dist(Point::new(5.0, 3.0), a, b) - 3.0).abs() < 0.01);
        // Past an endpoint → clamps to that endpoint's distance.
        assert!((point_segment_dist(Point::new(13.0, 4.0), a, b) - 5.0).abs() < 0.01);
    }

    #[test]
    fn edge_click_lands_between_self_and_a_peer() {
        // The midpoint of the self→peer segment must hit that edge (and no
        // node, since nodes sit at the segment's ends).
        let nodes = vec![
            node("self", "online", None, true),
            node("oak", "online", Some(20.0), false),
        ];
        let positions = layout(&nodes);
        let prog = MapProgram {
            nodes,
            positions,
            palette: Palette::dark(),
            flow_phase: 0.0,
        };
        let bounds = Rectangle::with_size(cosmic::iced::Size::new(800.0, 600.0));
        let proj = prog.projected(&bounds);
        let s = proj["self"];
        let o = proj["oak"];
        let mid = Point::new((s.x + o.x) / 2.0, (s.y + o.y) / 2.0);
        assert_eq!(prog.hit(&bounds, mid), None, "midpoint hits no node");
        assert_eq!(
            prog.hit_edge(&bounds, mid).as_deref(),
            Some("oak"),
            "midpoint hits the self→oak edge"
        );
    }
}
