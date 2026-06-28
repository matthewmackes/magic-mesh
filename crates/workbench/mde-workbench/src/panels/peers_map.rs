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

/// LIGHTHOUSE-7/9 — the overlay IPs of the mesh's lighthouses, read from the
/// world-readable mesh-status snapshot (`/run/mde/mesh-status.json`,
/// `network.lighthouse_ips`). This is the LIGHTHOUSE-9 **authoritative**
/// "is a lighthouse" signal (Nebula `static_host_map` membership) — anchors run
/// Server tier for storage, so the directory `role` field under-reports them.
/// Every map surface (wallpaper, Peers Map, Fleet rollup) keys lighthouse status
/// on this same set so they all agree on which node is an anchor. Missing/old
/// snapshot = empty (callers then fall back to the `role` field). Pure read.
#[must_use]
pub fn lighthouse_overlay_ips() -> Vec<String> {
    parse_lighthouse_ips(&std::fs::read_to_string("/run/mde/mesh-status.json").unwrap_or_default())
}

/// Parse `network.lighthouse_ips` out of a mesh-status snapshot body (pure).
#[must_use]
pub fn parse_lighthouse_ips(raw: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(raw.trim())
        .ok()
        .and_then(|v| {
            v.get("network")?
                .get("lighthouse_ips")?
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
        })
        .unwrap_or_default()
}

/// Whether a directory record is a lighthouse anchor (LIGHTHOUSE-7/9). A node is
/// an anchor when its overlay IP is in the snapshot's `lighthouse_ips` set (the
/// authoritative Nebula-membership signal) OR its directory `role` is
/// `lighthouse` (the back-fill for records that predate IP-membership stamping).
/// An empty `overlay_ip` never matches (a blank IP can't be a lighthouse).
#[must_use]
pub fn is_lighthouse(role: &str, overlay_ip: &str, lh_ips: &[String]) -> bool {
    role.eq_ignore_ascii_case("lighthouse")
        || (!overlay_ip.is_empty() && lh_ips.iter().any(|l| l == overlay_ip))
}

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
    /// LIGHTHOUSE-7 (wallpaper hero motif) — `true` when this node is an
    /// overlay anchor (a lighthouse). A lighthouse node gets the beacon hero
    /// treatment in the draw pass (a Carbon Green-50 beacon halo when online,
    /// danger otherwise) so the relay/anchor nodes read as the hero of the
    /// mesh map — both on the live desktop wallpaper and the Peers Map view.
    pub lighthouse: bool,
    /// PD-7/L18 — this peer's self→peer link traffic, normalized to
    /// 0.0..=1.0. MESHMAP-6: the REAL per-link tx rate from the mackesd
    /// `link-traffic` collector (nftables accounting) when present, else the
    /// per-node `sample_flows` Netdata proxy (MESHMAP-3). Drives the
    /// flow-particle density + speed on the self→peer edge; 0.0 = idle (no
    /// particles, so an idle mesh draws nothing extra — the L22 budget).
    pub flow: f64,
    /// MESHMAP-6 — this peer's peer→self link traffic, normalized 0.0..=1.0.
    /// The REAL per-link rx rate from the `link-traffic` collector when
    /// present; 0.0 when the proxy is the source (the per-node proxy can't
    /// split direction, so the reverse stream simply stays off and the edge
    /// shows the busy-ness it can honestly attribute — never a fake split).
    /// Drives the peer→self particle stream.
    pub flow_rx: f64,
    /// MESHMAP-7 (W7) — the relay node's **hostname** when this peer's overlay
    /// path is relayed through a lighthouse (resolved by the caller from the
    /// `PathInfo.relay_via` overlay-IP against the roster). `None` for a direct
    /// path. The draw pass bends the self→peer edge through this node's
    /// projected position (two segments via the relay); direct paths are
    /// straight.
    pub relay_via: Option<String>,
}

impl MapNode {
    /// MESHMAP-2 (W2) — this node's stable identity hue, derived from its
    /// hostname (`mde_theme::node_hue_for`). Deterministic across reboots + the
    /// same on every peer's map. The node DOT FILL + the packet particles this
    /// node *sends* read this hue; presence drives the ring (a separate signal).
    #[must_use]
    pub fn hue(&self) -> mde_theme::Rgba {
        mde_theme::node_hue_for(&self.hostname)
    }
}

/// Map a hostname to a coarse geographic centroid in the same unit space the
/// force [`layout`] uses (so geo + force positions are mutually scaled by the
/// shared `projected()` pass). Returns `None` when no region token is present —
/// the node then keeps its force-layout position (W4 fallback). Pure + offline:
/// no network, no geoIP database — a small static table keyed off a region
/// token detected in the hostname (the DO droplet naming convention) plus
/// coarse continent fallbacks.
///
/// The centroids are spread on a rough equirectangular world projection
/// (`x` ≈ longitude, growing eastward — west-coast NA negative, EU/Asia
/// positive; `y` ≈ −latitude, growing southward) and scaled to the layout's
/// unit space (a few hundred units), so DO's NA/EU/AP regions spread across the
/// canvas.
#[must_use]
fn region_centroid(hostname: &str) -> Option<(f32, f32)> {
    let h = hostname.to_ascii_lowercase();
    // DO region centroids (the regions the fleet's lighthouses live in),
    // west → east. (x, y) in layout unit space.
    const TABLE: &[(&str, (f32, f32))] = &[
        // North America.
        ("sfo", (-520.0, -60.0)), // San Francisco
        ("tor", (-180.0, -90.0)), // Toronto
        ("nyc", (-120.0, -70.0)), // New York
        // Europe.
        ("lon", (120.0, -120.0)), // London
        ("ams", (170.0, -130.0)), // Amsterdam
        ("fra", (220.0, -110.0)), // Frankfurt
        // Asia / Pacific.
        ("blr", (460.0, 40.0)),  // Bangalore
        ("sgp", (560.0, 60.0)),  // Singapore
        ("syd", (640.0, 260.0)), // Sydney
    ];
    for (tok, c) in TABLE {
        if h.contains(tok) {
            return Some(*c);
        }
    }
    // Coarse continent fallback for hostnames that name a continent but not a
    // specific city/region.
    const CONTINENT: &[(&str, (f32, f32))] = &[
        ("-us", (-200.0, -60.0)),
        ("-na", (-200.0, -60.0)),
        ("-eu", (180.0, -110.0)),
        ("-asia", (520.0, 40.0)),
        ("-ap", (520.0, 40.0)),
    ];
    for (tok, c) in CONTINENT {
        if h.contains(tok) {
            return Some(*c);
        }
    }
    None
}

/// MESHMAP-1 (W4) — geographic layout: each node whose hostname carries a known
/// region token is placed at that region's centroid (spread + jittered so two
/// nodes in one region don't overlap); every other node falls back to its
/// force-directed [`layout`] position. Self stays at the origin only when its
/// own region is unknown (a geo-known self sits at its real centroid). Pure +
/// deterministic + offline.
///
/// When **no** node carries a region token this returns exactly the force
/// [`layout`] (geo is then a no-op), so a region-less workgroup is unchanged.
#[must_use]
pub fn geo_layout(nodes: &[MapNode]) -> HashMap<String, (f32, f32)> {
    let force = layout(nodes);
    // Group geo-known nodes by region so we can fan co-located nodes out around
    // the centroid (deterministic by seed_angle, no overlap).
    let mut by_centroid: HashMap<(i32, i32), Vec<&str>> = HashMap::new();
    for n in nodes {
        if let Some((cx, cy)) = region_centroid(&n.hostname) {
            by_centroid
                .entry((cx as i32, cy as i32))
                .or_default()
                .push(&n.hostname);
        }
    }
    let mut pos = force;
    for n in nodes {
        let Some((cx, cy)) = region_centroid(&n.hostname) else {
            continue; // unknown region → keep force-layout position (W4 fallback)
        };
        let group = &by_centroid[&(cx as i32, cy as i32)];
        let (gx, gy) = if group.len() <= 1 {
            (cx, cy)
        } else {
            // Fan co-located nodes onto a small deterministic ring around the
            // centroid (stable by hostname, so no churn).
            let idx = group.iter().position(|h| *h == n.hostname).unwrap_or(0);
            let a = seed_angle(&n.hostname) + idx as f32 * 0.9;
            (cx + a.cos() * 46.0, cy + a.sin() * 46.0)
        };
        pos.insert(n.hostname.clone(), (gx, gy));
    }
    pos
}

/// MESHMAP-1 (W4) — does any node in the set carry a known region token? When
/// false, [`geo_layout`] equals the force [`layout`] and the faint map backdrop
/// is suppressed (a region-less workgroup gets the plain force map). Pure.
#[must_use]
pub fn any_geo_known(nodes: &[MapNode]) -> bool {
    nodes.iter().any(|n| region_centroid(&n.hostname).is_some())
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
    /// MESHMAP-3 (W3/W5) — **self's own** normalized overlay throughput
    /// (0.0..=1.0). The self→peer particle stream (self is the sender that
    /// direction) keys its density + speed on this; the peer→self stream keys
    /// on the peer's `flow` (the peer is the sender that direction). 0.0 ⇒ no
    /// self-sent particles (the W8 idle gate per direction).
    pub self_flow: f64,
    /// MESHMAP-1 (W4) — render the faint Carbon-Gray geographic backdrop +
    /// treat `positions` as geo-projected. Set by the caller from
    /// [`any_geo_known`]; when false the map is the plain force layout (no
    /// backdrop), so a region-less workgroup is unchanged.
    pub geo: bool,
    /// MESHMAP-5 (W8) — reduce-motion: draw STATIC colored edges (no particle
    /// animation), per Carbon §4 + WCAG 2.3.1. Set by the caller from
    /// `crate::live_theme::reduce_motion()`. Edges still carry the sender-hue
    /// tint so the per-direction color identity survives; only the moving dots
    /// are dropped.
    pub reduce_motion: bool,
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

/// MESHMAP-3/-4 — draw one direction's packet-particle stream riding a polyline
/// (`waypoints`: `[origin, …relay…, dest]`, ≥2 points). Density (`1..=6`) +
/// along-path speed scale with the sender's normalized `flow`; the dots are the
/// sender's `color` (W3 by-sender). `reverse` walks the path dest→origin (the
/// peer→self direction). Below the idle gate (`flow ≤ 0.02`) nothing is drawn,
/// so an idle direction costs nothing (W8). Pure draw into `frame`.
fn draw_particles(
    frame: &mut Frame,
    waypoints: &[Point],
    flow: f64,
    color: cosmic::iced::Color,
    phase: f32,
    reverse: bool,
) {
    if flow <= 0.02 || waypoints.len() < 2 {
        return;
    }
    let count = 1 + (flow * 5.0).round() as usize; // 1..=6
    let speed = 0.5 + flow as f32; // laps-per-cycle
    for k in 0..count {
        let base = (k as f32) / count as f32;
        let mut t = (phase * speed + base).fract();
        if reverse {
            t = 1.0 - t;
        }
        let pt = polyline_point(waypoints, t);
        frame.fill(&Path::circle(pt, 2.0), color);
    }
}

/// Point at fraction `t` (0.0..=1.0) along a polyline by arc length (so a bent
/// relay path animates at a constant visual speed across both segments). Pure.
fn polyline_point(pts: &[Point], t: f32) -> Point {
    if pts.len() < 2 {
        return pts.first().copied().unwrap_or(Point::ORIGIN);
    }
    let seg_len = |a: Point, b: Point| ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
    let total: f32 = pts.windows(2).map(|w| seg_len(w[0], w[1])).sum();
    if total <= f32::EPSILON {
        return pts[0];
    }
    let target = t.clamp(0.0, 1.0) * total;
    let mut acc = 0.0;
    for w in pts.windows(2) {
        let l = seg_len(w[0], w[1]);
        if acc + l >= target {
            let local = if l <= f32::EPSILON {
                0.0
            } else {
                (target - acc) / l
            };
            return Point::new(
                w[0].x + (w[1].x - w[0].x) * local,
                w[0].y + (w[1].y - w[0].y) * local,
            );
        }
        acc += l;
    }
    *pts.last().unwrap()
}

/// Midpoint of a polyline for the RTT label: the middle vertex when the path is
/// bent (a relay path's label sits at the relay), else the segment midpoint.
fn polyline_mid(pts: &[Point]) -> Point {
    match pts.len() {
        0 => Point::ORIGIN,
        1 => pts[0],
        2 => Point::new((pts[0].x + pts[1].x) / 2.0, (pts[0].y + pts[1].y) / 2.0),
        _ => pts[pts.len() / 2],
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

        // MESHMAP-1 (W4) — faint Carbon-Gray map backdrop under the scene when
        // the nodes are geo-placed. A low-contrast equator + meridian crosshair
        // (Gray 70 hairlines) reads as a world-map ground without any raster
        // asset; suppressed when no node is geo-known (plain force map).
        if self.geo {
            let grid = p.overlay.into_cosmic_color(); // Gray 70 hairline
            let eq = bounds.height * 0.46; // ~equator on the projection
            frame.stroke(
                &Path::line(Point::new(0.0, eq), Point::new(bounds.width, eq)),
                Stroke::default().with_color(grid).with_width(1.0),
            );
            for frac in [0.25_f32, 0.5, 0.75] {
                let mx = bounds.width * frac;
                frame.stroke(
                    &Path::line(Point::new(mx, 0.0), Point::new(mx, bounds.height)),
                    Stroke::default().with_color(grid).with_width(1.0),
                );
            }
        }

        // Self's identity hue (the sender hue for the self→peer direction).
        let self_hue = self
            .nodes
            .iter()
            .find(|n| n.is_self)
            .map(MapNode::hue)
            .unwrap_or(p.accent);

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
                // MESHMAP-4 (W7) — a relayed path bends through the relaying
                // lighthouse node's projected position (two segments); a direct
                // path is one straight segment. Particles ride the same polyline.
                let waypoints: Vec<Point> = match n
                    .relay_via
                    .as_deref()
                    .filter(|via| reachable && *via != n.hostname)
                    .and_then(|via| proj.get(via))
                {
                    Some(&relay) => vec![origin, relay, to],
                    None => vec![origin, to],
                };
                for seg in waypoints.windows(2) {
                    frame.stroke(
                        &Path::line(seg[0], seg[1]),
                        Stroke::default()
                            .with_color(color)
                            .with_width(if reachable { 1.5 } else { 1.0 }),
                    );
                }
                // MESHMAP-3/-5/-6 (W3/W5/W8) — TWO per-direction particle
                // streams, each colored by that direction's SENDER (EtherApe
                // by-node coloring). MESHMAP-6 splits direction off the REAL
                // per-link byte counters: the self→peer stream rides forward on
                // `n.flow` (the per-link tx — or the per-node `sample_flows`
                // proxy busy-ness where the collector doesn't cover this peer,
                // floored by self's own `self_flow`), sent by SELF (self_hue);
                // the peer→self stream rides reverse on `n.flow_rx` (the per-link
                // rx — 0 under the proxy, so the reverse stream honestly stays
                // off), sent by the PEER (its hue). Density+speed ∝ the rate.
                // Each direction's idle gate is independent (rate ≤ 0.02 ⇒
                // nothing), so an idle edge draws no dots (W8 zero-CPU). Under
                // reduce-motion (W8/WCAG 2.3.1) no dots are drawn at all — the
                // edge keeps the sender-hue tint below but never animates.
                if reachable && !self.reduce_motion {
                    // self→peer (forward along the waypoints): sent by self,
                    // driven by the real per-link tx (proxy busy-ness fallback,
                    // floored by self's own throughput).
                    draw_particles(
                        &mut frame,
                        &waypoints,
                        n.flow.max(self.self_flow),
                        self_hue.into_cosmic_color(),
                        self.flow_phase,
                        false,
                    );
                    // peer→self (reverse along the waypoints): sent by the peer,
                    // driven by the real per-link rx (0 under the proxy → off).
                    draw_particles(
                        &mut frame,
                        &waypoints,
                        n.flow_rx,
                        n.hue().into_cosmic_color(),
                        self.flow_phase,
                        true,
                    );
                }
                // RTT label at the midpoint of the (possibly bent) path; × for
                // unreachable. The midpoint is the polyline's middle vertex when
                // bent, else the segment midpoint.
                let mid = polyline_mid(&waypoints);
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
            // MESHMAP-2 (W2) — the node DOT FILL is the node's STABLE IDENTITY
            // HUE (hash(hostname)→hue, same across reboots + every peer's map);
            // PRESENCE drives the RING color only (online/idle→border, offline→
            // danger). An offline node's hue is dimmed (60 % alpha) so it reads
            // as "this is still <node>, but dark". (Previously presence drove
            // both fill + ring, which gave every online node the same green.)
            let hue = n.hue();
            let fill = if n.presence == "offline" {
                hue.with_alpha(0.6)
            } else {
                hue
            };
            let ring = match n.presence.as_str() {
                "online" | "idle" => p.border,
                _ => p.danger,
            };
            let r = if n.is_self { 14.0 } else { 10.0 };
            // LIGHTHOUSE-7 — the beacon hero motif. An anchor node renders a
            // concentric beacon halo behind its presence dot: Carbon Green-50
            // (`beacon_healthy`) when the lighthouse is online, danger otherwise
            // — the same green/red split the Hub footer + Lighthouses tab beacons
            // use, so every lighthouse surface agrees. The halo + ring lift the
            // relay/anchor nodes out of the peer field as the map's hero, ambient
            // on the desktop (no animation loop — L22).
            if n.lighthouse {
                // Green strictly when the anchor is online (matching its green
                // presence dot); idle / offline / unknown all read danger, so the
                // beacon never shows healthy-green over a down or degraded
                // anchor (the dot uses `online` for green too).
                let beacon = if n.presence == "online" {
                    p.beacon_healthy
                } else {
                    p.danger
                };
                // Outer faint halo, then a bright beacon ring hugging the node.
                for (rr, w) in [(r + 9.0, 1.0_f32), (r + 5.0, 1.5)] {
                    frame.stroke(
                        &Path::circle(at, rr),
                        Stroke::default()
                            .with_color(beacon.into_cosmic_color())
                            .with_width(w),
                    );
                }
            }
            frame.fill(&Path::circle(at, r), fill.into_cosmic_color());
            frame.stroke(
                &Path::circle(at, r + 2.0),
                Stroke::default()
                    .with_color(ring.into_cosmic_color())
                    .with_width(1.0),
            );
            // The label; a lighthouse carries the ◉ beacon marker (matching the
            // shell's mesh-welcome lighthouse glyph) so it's nameable as an
            // anchor at a glance, and sits below the wider beacon halo.
            let label = match (n.is_self, n.lighthouse) {
                (true, true) => format!("◉ {} (this machine)", n.hostname),
                (true, false) => format!("{} (this machine)", n.hostname),
                (false, true) => format!("◉ {}", n.hostname),
                (false, false) => n.hostname.clone(),
            };
            let label_y = if n.lighthouse { r + 11.0 } else { r + 6.0 };
            frame.fill_text(Text {
                content: label,
                position: Point::new(at.x, at.y + label_y),
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

/// MESHMAP-6 — one peer's real per-link traffic, normalized 0.0..=1.0,
/// as the mackesd `link-traffic` collector records it (nftables byte
/// deltas). `tx` is self→peer, `rx` is peer→self.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LinkFlow {
    /// self→peer rate (0.0..=1.0).
    pub tx: f64,
    /// peer→self rate (0.0..=1.0).
    pub rx: f64,
}

/// MESHMAP-6 — read the per-link traffic cache (the `link-traffic`
/// collector output) into a host→(tx,rx) map. Missing cache = empty (the
/// consumer falls back to the per-node `sample_flows` proxy). Same
/// `~/.cache/mde/` idiom as the latency cache.
#[must_use]
pub fn read_link_traffic() -> HashMap<String, LinkFlow> {
    let Some(home) = std::env::var_os("HOME") else {
        return HashMap::new();
    };
    let path = std::path::PathBuf::from(home).join(".cache/mde/link-traffic.json");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    parse_link_traffic(&raw)
}

/// Parse the link-traffic snapshot JSON (pure). A missing direction reads
/// 0.0; a malformed body yields an empty map (honest "no data" → proxy).
#[must_use]
pub fn parse_link_traffic(raw: &str) -> HashMap<String, LinkFlow> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return HashMap::new();
    };
    v.get("peers")
        .and_then(|p| p.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(host, e)| {
                    let flow = LinkFlow {
                        tx: e
                            .get("tx_rate")
                            .and_then(serde_json::Value::as_f64)
                            .unwrap_or(0.0),
                        rx: e
                            .get("rx_rate")
                            .and_then(serde_json::Value::as_f64)
                            .unwrap_or(0.0),
                    };
                    (host.clone(), flow)
                })
                .collect()
        })
        .unwrap_or_default()
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
            lighthouse: false,
            flow: 0.0,
            flow_rx: 0.0,
            relay_via: None,
        }
    }

    // ── MESHMAP-6: per-link traffic cache parse ──

    #[test]
    fn parse_link_traffic_reads_tx_and_rx_rates() {
        let raw = r#"{"checked_at":1,"peers":{
            "anvil":{"tx_rate":0.42,"rx_rate":0.11,"tx_bps":5040000.0,"rx_bps":1320000.0},
            "forge":{"tx_rate":0.0,"rx_rate":0.0,"tx_bps":0.0,"rx_bps":0.0}
        }}"#;
        let m = parse_link_traffic(raw);
        assert_eq!(m.len(), 2);
        assert!((m["anvil"].tx - 0.42).abs() < 1e-9);
        assert!((m["anvil"].rx - 0.11).abs() < 1e-9);
        assert_eq!(m["forge"].tx, 0.0);
    }

    #[test]
    fn parse_link_traffic_defaults_missing_direction_to_zero() {
        // A row carrying only one direction reads the other as 0 (a one-way
        // link still shows, no panic).
        let raw = r#"{"peers":{"oak":{"tx_rate":0.5}}}"#;
        let m = parse_link_traffic(raw);
        assert!((m["oak"].tx - 0.5).abs() < 1e-9);
        assert_eq!(m["oak"].rx, 0.0);
    }

    #[test]
    fn parse_link_traffic_rejects_garbage_falls_back_to_proxy() {
        // An empty map is the "no real data → consumer keeps the proxy" signal.
        assert!(parse_link_traffic("not json").is_empty());
        assert!(parse_link_traffic("{}").is_empty());
    }

    #[test]
    fn is_lighthouse_keys_on_overlay_ip_or_role() {
        let lh_ips = vec!["10.42.0.1".to_string(), "10.42.0.2".to_string()];
        // Authoritative overlay-IP membership flags an anchor even when the
        // directory role under-reports it (Server tier — the LIGHTHOUSE-9 case).
        assert!(is_lighthouse("server", "10.42.0.1", &lh_ips));
        // The role back-fill still flags pre-IP-stamping records (case-insensitive).
        assert!(is_lighthouse("Lighthouse", "10.42.0.9", &[]));
        // A plain peer with neither signal is not an anchor.
        assert!(!is_lighthouse("server", "10.42.0.9", &lh_ips));
        // An empty overlay IP never matches, even against a populated set.
        assert!(!is_lighthouse("server", "", &lh_ips));
        assert!(parse_lighthouse_ips("junk").is_empty());
    }

    #[test]
    fn lighthouse_node_is_distinguished_from_a_plain_peer() {
        // A lighthouse node carries the hero flag; a plain peer does not.
        // The flag is what the draw pass keys the beacon halo + ◉ label on, so
        // the map can tell an anchor apart from a worker — the LIGHTHOUSE-7
        // wallpaper-hero contract. (The draw itself is exercised live; the data
        // distinction is the unit-testable contract.)
        let mut lh = node("lh-01", "online", Some(8.0), false);
        lh.lighthouse = true;
        let peer = node("worker", "online", Some(8.0), false);
        assert!(lh.lighthouse, "the anchor is flagged a lighthouse");
        assert!(!peer.lighthouse, "a plain peer is not");
        assert_ne!(lh, peer, "the hero flag makes the nodes distinct");
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
            self_flow: 0.0,
            geo: false,
            reduce_motion: false,
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
            self_flow: 0.0,
            geo: false,
            reduce_motion: false,
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

    #[test]
    fn node_hue_distinguishes_nodes_and_is_stable() {
        // MESHMAP-2 (W2) — a node's identity hue is its hostname's stable hue,
        // distinct from other nodes' and unchanged across `MapNode` rebuilds
        // (presence change doesn't move the hue — only the ring).
        let a = node("anvil", "online", Some(5.0), false);
        let a2 = node("anvil", "offline", None, false);
        let b = node("forge", "online", Some(5.0), false);
        assert_eq!(a.hue(), a2.hue(), "hue is stable across presence change");
        assert_ne!(a.hue(), b.hue(), "distinct hostnames get distinct hues");
        // The hue is exactly the mde-theme node-hue (single-sourced, no local
        // color math in the panel — §4).
        assert_eq!(a.hue(), mde_theme::node_hue_for("anvil"));
    }

    #[test]
    fn geo_layout_places_known_regions_and_falls_back_otherwise() {
        // MESHMAP-1 (W4) — a hostname carrying a region token lands at that
        // region's centroid band; a region-less host keeps its force position.
        let nodes = vec![
            node("self-laptop", "online", None, true), // no region → force
            node("lighthouse-nyc3", "online", Some(20.0), false), // NA east
            node("lighthouse-sfo3", "online", Some(80.0), false), // NA west
            node("lighthouse-fra1", "online", Some(110.0), false), // EU
        ];
        assert!(any_geo_known(&nodes), "region tokens detected");
        let geo = geo_layout(&nodes);
        let force = layout(&nodes);
        let x = |m: &HashMap<String, (f32, f32)>, h: &str| m[h].0;
        // West-coast (sfo) sits west of east-coast (nyc) which sits west of EU.
        assert!(
            x(&geo, "lighthouse-sfo3") < x(&geo, "lighthouse-nyc3"),
            "sfo west of nyc: {} vs {}",
            x(&geo, "lighthouse-sfo3"),
            x(&geo, "lighthouse-nyc3")
        );
        assert!(
            x(&geo, "lighthouse-nyc3") < x(&geo, "lighthouse-fra1"),
            "nyc west of fra"
        );
        // The region-less self keeps its force-layout position (W4 fallback).
        assert_eq!(
            geo["self-laptop"], force["self-laptop"],
            "unknown-region node falls back to the force position"
        );
    }

    #[test]
    fn geo_layout_is_force_layout_when_no_region_known() {
        // A region-less workgroup gets exactly the force map (geo is a no-op),
        // and the backdrop is suppressed.
        let nodes = vec![
            node("self", "online", None, true),
            node("oak", "online", Some(10.0), false),
            node("elm", "online", Some(40.0), false),
        ];
        assert!(!any_geo_known(&nodes), "no region tokens");
        assert_eq!(
            geo_layout(&nodes),
            layout(&nodes),
            "region-less → identical to force layout"
        );
    }

    #[test]
    fn relayed_edge_bends_through_the_relay_node() {
        // MESHMAP-4 (W7) — when a peer's path is relayed via a node we have a
        // position for, the edge is a 3-point polyline through the relay; a
        // direct path stays a 2-point segment. Exercise the waypoint selection
        // the draw pass uses (the geometry that bends the path).
        let mut relayed = node("forge", "online", Some(60.0), false);
        relayed.relay_via = Some("lighthouse-01".into());
        let proj: HashMap<String, Point> = [
            ("self".to_string(), Point::new(0.0, 0.0)),
            ("forge".to_string(), Point::new(100.0, 0.0)),
            ("lighthouse-01".to_string(), Point::new(50.0, 80.0)),
        ]
        .into_iter()
        .collect();
        let origin = proj["self"];
        let to = proj["forge"];
        let waypoints: Vec<Point> = match relayed
            .relay_via
            .as_deref()
            .filter(|via| *via != relayed.hostname)
            .and_then(|via| proj.get(via))
        {
            Some(&relay) => vec![origin, relay, to],
            None => vec![origin, to],
        };
        assert_eq!(waypoints.len(), 3, "relayed path bends (3 vertices)");
        assert_eq!(waypoints[1], proj["lighthouse-01"], "bend is at the relay");
        // The label sits at the relay vertex for a bent path.
        assert_eq!(polyline_mid(&waypoints), proj["lighthouse-01"]);

        // A direct (no relay) path is a straight 2-point segment.
        let direct = node("oak", "online", Some(10.0), false);
        let dwp: Vec<Point> = match direct.relay_via.as_deref().and_then(|via| proj.get(via)) {
            Some(&relay) => vec![origin, relay, to],
            None => vec![origin, to],
        };
        assert_eq!(dwp.len(), 2, "direct path is straight");
    }

    #[test]
    fn polyline_point_is_arc_length_parametrized() {
        // t=0 → start, t=1 → end, t=0.5 → arc-length midpoint (across a bend).
        let pts = [
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(10.0, 10.0),
        ];
        assert_eq!(polyline_point(&pts, 0.0), pts[0]);
        assert_eq!(polyline_point(&pts, 1.0), pts[2]);
        // Total length 20; half (10) lands exactly at the bend vertex.
        let m = polyline_point(&pts, 0.5);
        assert!((m.x - 10.0).abs() < 0.01 && m.y.abs() < 0.01, "mid at bend");
    }
}
