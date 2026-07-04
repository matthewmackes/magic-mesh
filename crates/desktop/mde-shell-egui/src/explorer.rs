//! EXPLORER-3 — the Discovery-surface **hero-card mode** (`docs/design/unit-explorer.md`,
//! locked 2026-07-04; locks #5/#9/#10/#11/#12 + #21/#23).
//!
//! A cinematic, one-unit-at-a-time view over **every discovered unit** — mesh
//! peers, off-mesh LAN hosts, and every `OpenStack` object — navigable like a
//! media shelf: a full-bleed **hero card** for the focused unit, arrow / chevron
//! paging through the set, category filter chips, and a bottom **filmstrip** of
//! neighbours.
//!
//! ## Thin renderer (§6)
//!
//! The daemon does the work: the `mackesd` `unit_aggregator` worker (EXPLORER-1)
//! unions the three sources into one typed `Unit` stream and publishes the
//! per-node mirror `state/units/<node>`. This surface only **reads** those
//! mirrors off the Bus — it never scans, never links the daemon crate. Exactly
//! like [`crate::storage`] folds `state/storage/*` and [`crate::chooser`] folds
//! `state/desktops/sources`, the wire payload is decoded into **local** serde
//! mirrors of the worker's types (the shell leans inward on `mde-bus` only).
//!
//! ## Honest at empty (#23)
//!
//! Before any mirror lands the card shows **this node** as the first hero unit
//! with an honest "Discovering units…" line — never a blank pane, never a faked
//! peer (§7). Real units replace it the instant a mirror streams in.
//!
//! ## Clean seams for the rest of the EXPLORER epic
//!
//! - **Scan-active gate (#24).** The aggregator's scan-active flag is an
//!   in-process `Arc<AtomicBool>` inside the daemon (§6 boundary) — there is no
//!   Bus seam to set it from the shell yet. EXPLORER-3 realises the reachable
//!   half honestly: it reads the mirrors **only while the surface is visible**
//!   (the mount polls it only then). When a Bus scan-active verb lands, the same
//!   visibility signal drives it — no dead publish is minted here (§7).
//! - **Mosaic + IPAM (EXPLORER-11 / EXPLORER-10).** This unit is the hero-card
//!   mode only. The zoomable mosaic overview + summary strip and the IPAM table
//!   are their own modes; the category chips here are the seed of the #8 filter.
//! - **Rich telemetry sparklines + per-type action bars (EXPLORER-4 / EXPLORER-5).**
//!   The card shows the telemetry it can honestly read today (health tier, mesh
//!   role/leader/version, any reported load/mem/uptime) and marks the rest
//!   unknown; the load/mem/net **sparklines** and the armed action verbs fill the
//!   same card without re-wiring this mount.

// This module is canvas/painter code (the hero glyphs, status ring, filmstrip
// thumbnails). Its geometry is a few pixel positions per frame, so — exactly as
// `mde-mesh-view` documents crate-wide — the pedantic numeric-cast lints and
// `suboptimal_flops` are allowed here: `center.y + r * 0.4` reads far clearer
// than the `mul_add` rewrite, and the precision/throughput gain is irrelevant.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::suboptimal_flops
)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use mde_bus::persist::Persist;
use mde_egui::egui::{
    self, Align, Align2, Color32, FontId, Layout, Rect, RichText, Sense, Stroke, StrokeKind,
    UiBuilder, Vec2,
};
use mde_egui::{muted_note, Motion, Style};

/// The memory key the mount toggles the Mesh-Map surface's **Explorer** lens on
/// (read in `main.rs`'s poll gate + the surface arm). Kept here so the one key
/// can't drift between the two call sites.
pub const LENS_KEY: &str = "explorer-lens-active";

/// The per-node mirror topic prefix the aggregator publishes to — MUST equal
/// `mackesd::workers::unit_aggregator::state_topic`'s `state/units/<node>` shape
/// (pinned in tests against a real body).
const STATE_PREFIX: &str = "state/units/";

/// Bus-poll cadence. The read is a cheap local spool scan and discovery is
/// event-paced, so the shared 5 s cadence surfaces a new/removed unit without
/// spinning — the same cadence every other plane refreshes at.
const REFRESH: Duration = Duration::from_secs(5);

/// Filmstrip thumbnail width — three XL grid steps, wide enough for a mini glyph
/// plus a truncated name. A behaviour param on the §4 grid, not a scattered px.
const THUMB_W: f32 = Style::SP_XL * 3.0;

/// Filmstrip thumbnail height (matches the Chooser card's thumb well proportion).
const THUMB_H: f32 = Style::SP_XL * 2.25;

/// The health ring's stroke width (a painter behaviour param, like the Chooser's
/// `Stroke::new(1.0, …)` plate border).
const RING_STROKE_W: f32 = 3.0;

/// A dimmed/unreachable ring's thinner stroke.
const OFFLINE_RING_W: f32 = 1.5;

/// The procedural type-glyph stroke width.
const GLYPH_STROKE_W: f32 = 2.0;

/// The hero display name's font size — the cinematic display type (#10 "big
/// display name", O11 generous type), derived from the §4 `HEADING` token so it
/// scales with the type ramp rather than a raw px literal.
const HERO_TITLE_FS: f32 = Style::HEADING * 1.5;

/// Fraction of the hero width a single page-step slides through (Carbon
/// productive-motion horizontal slide, #21).
const SLIDE_FRACTION: f32 = 0.12;

/// The dimmed-minimal card's opacity (#12) — dim enough to read "limited detail",
/// bright enough that the known facts stay legible (the Chooser's offline idiom).
const DIMMED_OPACITY: f32 = 0.55;

/// The hero ring diameter as a fraction of the smaller hero dimension, clamped to
/// a sane band so it stays cinematic on a large surface and legible when compact.
const RING_FRACTION: f32 = 0.32;
/// Minimum hero ring diameter (two XL steps).
const RING_MIN: f32 = Style::SP_XL * 2.0;
/// Maximum hero ring diameter (four XL steps).
const RING_MAX: f32 = Style::SP_XL * 4.0;

// ─────────────────────────── wire mirrors (§6) ───────────────────────────

/// The kind of a discovered unit — a **local** mirror of the aggregator's
/// `UnitKind` (EXPLORER-1). Decoded from the mirror body; the shell never links
/// the daemon crate (§6). An unknown future kind fails only that unit's parse,
/// not the whole stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum UnitKind {
    /// An in-mesh Nebula peer (source: the mesh mirror).
    Peer,
    /// An off-mesh LAN host from the active scan (EXPLORER-2).
    LanHost,
    /// A Nova compute instance.
    Instance,
    /// A Cinder volume.
    Volume,
    /// A Glance image.
    Image,
    /// A Neutron network.
    Network,
}

impl UnitKind {
    /// The human label for the type badge (#10).
    const fn label(self) -> &'static str {
        match self {
            Self::Peer => "Peer",
            Self::LanHost => "LAN host",
            Self::Instance => "Instance",
            Self::Volume => "Volume",
            Self::Image => "Image",
            Self::Network => "Network",
        }
    }

    /// The proximity/trust category this kind belongs to (locks #7/#8, O8).
    const fn category(self) -> Category {
        match self {
            Self::Peer => Category::Mesh,
            Self::LanHost => Category::Lan,
            Self::Instance | Self::Volume | Self::Image | Self::Network => Category::Cloud,
        }
    }
}

/// Where a unit sits relative to the mesh — a mirror of the aggregator's
/// `Reachability` (#10, the reachability line).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "where", rename_all = "snake_case")]
enum Reachability {
    /// Inside the mesh — a live Nebula peer.
    InMesh,
    /// Seen on the local LAN, outside the mesh.
    OnLan,
    /// A cloud object hosted on `node` (the host-node tag, lock #20).
    CloudObject {
        /// The mesh node that hosts this object.
        node: String,
    },
}

/// A unit's coarse health tier — a mirror of the aggregator's `Health`. Drives
/// the status ring's colour (#9); `Unknown`/absent stays honestly un-tinted (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Health {
    /// No active alarms.
    Healthy,
    /// A warning-tier alarm is active.
    Degraded,
    /// A critical-tier alarm is active.
    Critical,
    /// Known to the directory but not currently reachable.
    Unreachable,
    /// Reported, but the tier couldn't be classified.
    Unknown,
}

impl Health {
    /// The §4 status-colour token for this tier (#9). `Unreachable`/`Unknown`
    /// read as dim, never a fabricated "healthy" green (§7).
    const fn ring_color(self) -> Color32 {
        match self {
            Self::Healthy => Style::OK,
            Self::Degraded => Style::WARN,
            Self::Critical => Style::DANGER,
            Self::Unreachable | Self::Unknown => Style::TEXT_DIM,
        }
    }
}

/// Rich telemetry for a readable unit — a mirror of the aggregator's `Telemetry`.
/// Every field is optional so a partially-readable unit is honest field-by-field
/// (§7). EXPLORER-1 leaves it absent; EXPLORER-4 folds live sources in.
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
struct Telemetry {
    /// 1-minute load average, when readable.
    load1: Option<f32>,
    /// Memory-used percentage (0–100), when readable.
    mem_used_pct: Option<f32>,
    /// Uptime in seconds, when readable.
    uptime_s: Option<u64>,
}

impl Telemetry {
    /// Whether any field is actually populated (else the card shows the honest
    /// "not yet reported" note rather than a row of blanks, §7).
    const fn any(&self) -> bool {
        self.load1.is_some() || self.mem_used_pct.is_some() || self.uptime_s.is_some()
    }
}

/// Mesh-mirror facts folded onto a peer — a mirror of the aggregator's
/// `MeshFacts`. `None`/false where the directory row is silent (§7).
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
struct MeshFacts {
    /// The peer's pinned deployment role (`lighthouse`/`workstation`).
    role: Option<String>,
    /// Whether this peer holds the `/mesh/leader` lease.
    leader: bool,
    /// The peer's installed `mde` version, when detected.
    mde_version: Option<String>,
}

/// One discovered unit — a **local** mirror of the aggregator's `Unit`, carrying
/// exactly the fields the hero card renders. Serde ignores the daemon-only fields
/// (cloud detail, enrichment) EXPLORER-4/9 render, so this decodes the same body
/// without linking the daemon crate (§6).
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct Unit {
    /// Stable, source-namespaced id (the dedup + self key).
    id: String,
    /// The kind badge.
    kind: UnitKind,
    /// Big display name (#10).
    name: String,
    /// In-mesh / on-LAN / cloud-object+node (#10).
    reachability: Reachability,
    /// Best-known address, when a source reported one (§7 `None` otherwise).
    #[serde(default)]
    address: Option<String>,
    /// Coarse health where a real source reports it; `None` ⇒ unprobed (§7).
    #[serde(default)]
    health: Option<Health>,
    /// Rich telemetry where readable; `None` ⇒ unprobed (§7).
    #[serde(default)]
    telemetry: Option<Telemetry>,
    /// Mesh-mirror facts for a peer; `None` otherwise.
    #[serde(default)]
    mesh: Option<MeshFacts>,
    /// First observation, ms since the Unix epoch (E10).
    #[serde(default)]
    first_seen_ms: u64,
    /// Most-recent observation, ms since the Unix epoch (E10).
    #[serde(default)]
    last_seen_ms: u64,
}

/// The body published to `state/units/<node>` — a mirror of the aggregator's
/// `UnitsState` (only the fields the shell reads; `edges`/`published_at_ms` are
/// ignored by serde until EXPLORER-8 renders edge chips).
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
struct UnitsState {
    /// The publishing node id.
    host: String,
    /// Every unit that node folded.
    units: Vec<Unit>,
}

// ─────────────────────────── category identity ───────────────────────────

/// The three proximity categories a unit falls into (locks #7/#8, O8). Each
/// carries a distinct §4 accent + a coherent label used on chips, badges, and the
/// status ring. (EXPLORER-15 promotes these to dedicated Mesh/LAN/Cloud tokens;
/// EXPLORER-3 maps onto the existing accent set — token-based, no raw hex.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    /// In-mesh peers.
    Mesh,
    /// Off-mesh LAN hosts.
    Lan,
    /// `OpenStack` cloud objects.
    Cloud,
}

impl Category {
    /// The three categories in proximity order (mesh → LAN → cloud), for the chips.
    const ALL: [Self; 3] = [Self::Mesh, Self::Lan, Self::Cloud];

    /// The chip / divider label.
    const fn label(self) -> &'static str {
        match self {
            Self::Mesh => "Mesh",
            Self::Lan => "LAN",
            Self::Cloud => "Cloud",
        }
    }

    /// The §4 accent token for this category (O8 — distinct per world).
    const fn accent(self) -> Color32 {
        match self {
            Self::Mesh => Style::ACCENT_MESH,
            Self::Lan => Style::ACCENT_TERMINALS,
            Self::Cloud => Style::ACCENT_WORKLOADS,
        }
    }

    /// The category's slot in the rollup-count array + proximity ordering.
    const fn index(self) -> usize {
        match self {
            Self::Mesh => 0,
            Self::Lan => 1,
            Self::Cloud => 2,
        }
    }
}

// ─────────────────────────── the units read seam ───────────────────────────

/// The injectable read seam over the Bus units mirrors — production talks the Bus
/// ([`BusUnits`]); tests inject a fake so the fold + render are exercised
/// headless. The same pattern the Chooser's `DesktopSourcesClient` uses.
trait UnitsClient {
    /// Read the latest `state/units/<node>` body from every node's mirror.
    fn read(&self) -> Vec<UnitsState>;
}

/// The production units reader: enumerate every `state/units/<node>` topic by
/// prefix and decode each node's latest body (the same `list_topics` +
/// latest-wins idiom [`crate::storage`] uses for `state/storage/*`).
struct BusUnits {
    /// The desktop-client Bus spool root (`None` ⇒ no Bus dir ⇒ empty read, the
    /// honest solo-host state).
    bus_root: Option<PathBuf>,
}

impl UnitsClient for BusUnits {
    fn read(&self) -> Vec<UnitsState> {
        let Some(root) = self.bus_root.clone() else {
            return Vec::new();
        };
        let Ok(persist) = Persist::open(root) else {
            return Vec::new();
        };
        let topics = persist.list_topics().unwrap_or_default();
        let mut states = Vec::new();
        for topic in topics.iter().filter(|t| t.starts_with(STATE_PREFIX)) {
            let latest = persist
                .list_since(topic, None)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|m| m.body)
                .next_back();
            if let Some(body) = latest {
                if let Ok(state) = serde_json::from_str::<UnitsState>(&body) {
                    states.push(state);
                }
            }
        }
        states
    }
}

// ─────────────────────────── pure fold ───────────────────────────

/// The stable unit id a peer (or self) folds under — mirrors the aggregator's
/// `peer_unit_id`.
fn peer_self_id(host: &str) -> String {
    format!("peer:{host}")
}

/// Union every node's mirror into one shelf: dedup by id keeping the freshest
/// observation (lock #20 dedup), then order **this node first** (#23), then by
/// proximity category, then by name (locks #7). Pure — the render's data model,
/// unit-tested without a Bus.
fn fold_units(states: &[UnitsState], local_host: &str) -> Vec<Unit> {
    let self_id = peer_self_id(local_host);
    let mut by_id: HashMap<String, Unit> = HashMap::new();
    for state in states {
        for unit in &state.units {
            match by_id.get(&unit.id) {
                Some(existing) if existing.last_seen_ms >= unit.last_seen_ms => {}
                _ => {
                    by_id.insert(unit.id.clone(), unit.clone());
                }
            }
        }
    }
    let mut units: Vec<Unit> = by_id.into_values().collect();
    units.sort_by(|a, b| {
        proximity_rank(a, &self_id)
            .cmp(&proximity_rank(b, &self_id))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
    units
}

/// The sort key: this node first (0), then the unit's category slot + 1.
fn proximity_rank(unit: &Unit, self_id: &str) -> u8 {
    if unit.id == self_id {
        0
    } else {
        // category().index() is 0..=2 → 1..=3, never overflowing a u8.
        u8::try_from(unit.kind.category().index()).unwrap_or(2) + 1
    }
}

// ─────────────────────────── text helpers ───────────────────────────

/// The reachability line (#10): "In mesh" / "On LAN" / "Cloud object · <node>",
/// with the address appended when a source reported one (§7 — nothing faked).
fn reachability_line(reach: &Reachability, address: Option<&str>) -> String {
    let base = match reach {
        Reachability::InMesh => "In mesh".to_string(),
        Reachability::OnLan => "On LAN".to_string(),
        Reachability::CloudObject { node } => format!("Cloud object · {node}"),
    };
    match address {
        Some(addr) if !addr.is_empty() => format!("{base} · {addr}"),
        _ => base,
    }
}

/// Format an uptime/duration in whole seconds as a compact "Nd Nh" / "Nh Nm" /
/// "Nm" / "Ns" string.
fn fmt_duration(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let mins = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else if mins > 0 {
        format!("{mins}m")
    } else {
        format!("{secs}s")
    }
}

/// A "last seen …" phrasing from a millisecond gap (E10 — honest presence).
fn fmt_seen_ago(gap_ms: u64) -> String {
    if gap_ms < 5_000 {
        "just now".to_string()
    } else {
        format!("{} ago", fmt_duration(gap_ms / 1_000))
    }
}

/// Wall-clock milliseconds since the Unix epoch (saturating, never panicking).
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The local hostname — `$HOSTNAME` → `/proc/sys/kernel/hostname` → `/etc/hostname`
/// → `"localhost"` (the shell-tier idiom, shared with [`crate::storage`]). Names
/// this node's own hero unit (#23) + orders it first.
fn local_hostname() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    for path in ["/proc/sys/kernel/hostname", "/etc/hostname"] {
        if let Ok(h) = std::fs::read_to_string(path) {
            let h = h.trim();
            if !h.is_empty() {
                return h.to_string();
            }
        }
    }
    "localhost".to_string()
}

// ─────────────────────────── procedural glyphs (#9) ───────────────────────────

/// Paint an arc of `sweep` radians starting at `start` around `center`, as a
/// short polyline of `line_segment`s (font-independent painter primitives).
fn paint_arc(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    start: f32,
    sweep: f32,
    stroke: Stroke,
) {
    const STEPS: usize = 40;
    let mut prev = center + Vec2::angled(start) * radius;
    for i in 1..=STEPS {
        let a = start + sweep * (i as f32 / STEPS as f32);
        let next = center + Vec2::angled(a) * radius;
        painter.line_segment([prev, next], stroke);
        prev = next;
    }
}

/// Paint the health status ring around the hero glyph (#9): a solid ring in the
/// health tier's §4 colour, or — when health is `Unknown`/absent (still
/// discovering) — a rotating accent arc over a faint track, an **honest** "still
/// probing" spinner (real state, never faked telemetry). Returns whether it
/// animated (so the caller can keep the repaint heartbeat alive).
fn paint_status_ring(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    health: Option<Health>,
    accent: Color32,
    time: f64,
) -> bool {
    match health {
        Some(Health::Healthy | Health::Degraded | Health::Critical) => {
            let color = health.map_or(Style::BORDER, Health::ring_color);
            painter.circle_stroke(center, radius, Stroke::new(RING_STROKE_W, color));
            false
        }
        Some(Health::Unreachable) => {
            painter.circle_stroke(center, radius, Stroke::new(OFFLINE_RING_W, Style::TEXT_DIM));
            false
        }
        // Unknown / not-yet-reported: a faint full track + a rotating accent arc.
        _ => {
            painter.circle_stroke(center, radius, Stroke::new(OFFLINE_RING_W, Style::BORDER));
            let start = (time % 2.0) as f32 * std::f32::consts::PI;
            paint_arc(
                painter,
                center,
                radius,
                start,
                std::f32::consts::FRAC_PI_2,
                Stroke::new(RING_STROKE_W, accent),
            );
            true
        }
    }
}

/// Paint a distinct procedural line-art glyph for the unit kind, centred in a box
/// of half-extent `r` (Carbon-inspired, font-independent painter primitives —
/// like the mesh-map canvas). `color` is the category accent.
#[allow(clippy::many_single_char_names)]
fn paint_kind_glyph(painter: &egui::Painter, center: egui::Pos2, r: f32, kind: UnitKind, color: Color32) {
    let stroke = Stroke::new(GLYPH_STROKE_W, color);
    match kind {
        // Peer — a hub with three spokes to satellite rings (a mesh node).
        UnitKind::Peer => {
            painter.circle_filled(center, r * 0.28, color);
            for k in 0u8..3 {
                let a = std::f32::consts::TAU * (f32::from(k) / 3.0) - std::f32::consts::FRAC_PI_2;
                let sat = center + Vec2::angled(a) * r;
                painter.line_segment([center, sat], stroke);
                painter.circle_stroke(sat, r * 0.2, stroke);
            }
        }
        // LAN host — a monitor: screen rect on a short stand.
        UnitKind::LanHost => {
            let screen = Rect::from_center_size(
                center - Vec2::new(0.0, r * 0.15),
                Vec2::new(r * 1.7, r * 1.15),
            );
            painter.rect_stroke(screen, Style::RADIUS * 0.5, stroke, StrokeKind::Middle);
            let base_y = screen.max.y + r * 0.45;
            painter.line_segment(
                [egui::pos2(center.x, screen.max.y), egui::pos2(center.x, base_y)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(center.x - r * 0.5, base_y), egui::pos2(center.x + r * 0.5, base_y)],
                stroke,
            );
        }
        // Instance — three stacked server bays, each with an indicator dot.
        UnitKind::Instance => {
            for k in 0u8..3 {
                let cy = center.y + (f32::from(k) - 1.0) * r * 0.7;
                let bay = Rect::from_center_size(
                    egui::pos2(center.x, cy),
                    Vec2::new(r * 1.7, r * 0.5),
                );
                painter.rect_stroke(bay, Style::RADIUS * 0.4, stroke, StrokeKind::Middle);
                painter.circle_filled(egui::pos2(bay.min.x + r * 0.28, cy), GLYPH_STROKE_W, color);
            }
        }
        // Volume — a drive: tall rounded rect with a bottom "used" bar.
        UnitKind::Volume => {
            let body = Rect::from_center_size(center, Vec2::new(r * 1.3, r * 1.8));
            painter.rect_stroke(body, Style::RADIUS * 0.6, stroke, StrokeKind::Middle);
            let bar_y = body.max.y - r * 0.4;
            painter.line_segment(
                [egui::pos2(body.min.x + r * 0.25, bar_y), egui::pos2(body.max.x - r * 0.25, bar_y)],
                stroke,
            );
        }
        // Image — a snapshot disc: concentric circles.
        UnitKind::Image => {
            painter.circle_stroke(center, r, stroke);
            painter.circle_filled(center, r * 0.32, color);
        }
        // Network — a small triangle graph (three linked nodes, no central hub).
        UnitKind::Network => {
            let nodes: [egui::Pos2; 3] = [
                center + Vec2::angled(-std::f32::consts::FRAC_PI_2) * r,
                center + Vec2::angled(std::f32::consts::FRAC_PI_2 + std::f32::consts::FRAC_PI_3) * r,
                center + Vec2::angled(std::f32::consts::FRAC_PI_2 - std::f32::consts::FRAC_PI_3) * r,
            ];
            for k in 0..3 {
                painter.line_segment([nodes[k], nodes[(k + 1) % 3]], stroke);
            }
            for n in nodes {
                painter.circle_stroke(n, r * 0.24, stroke);
            }
        }
    }
}

// ─────────────────────────── the surface state ───────────────────────────

/// The Discovery-surface hero-card state (EXPLORER-3): the folded unit shelf, the
/// focused index, the active category filter, and the Bus read seam.
pub struct ExplorerState {
    /// The units read seam (Bus in production, a fake in tests).
    client: Box<dyn UnitsClient>,
    /// This node's hostname — its self hero unit (#23) + first-sort key.
    local_host: String,
    /// The folded shelf: deduped, self-first, proximity-ordered.
    units: Vec<Unit>,
    /// The focused hero index, into the currently-**filtered** view.
    focus: usize,
    /// The active category filter (`None` ⇒ all, #8).
    filter: Option<Category>,
    /// When the Bus was last polled (drives the fixed cadence).
    last_poll: Option<Instant>,
}

impl Default for ExplorerState {
    fn default() -> Self {
        Self {
            client: Box::new(BusUnits {
                bus_root: mde_bus::client_data_dir(),
            }),
            local_host: local_hostname(),
            units: Vec::new(),
            focus: 0,
            filter: None,
            last_poll: None,
        }
    }
}

impl ExplorerState {
    /// The bus-poll seam: re-read the mirrors when the cadence has elapsed, then
    /// keep the repaint heartbeat alive so a unit streaming in on any node
    /// surfaces without input. Called by the mount **only while the surface is
    /// visible** (the honest reachable half of the #24 scan-active gate). Cheap
    /// enough to call every frame — it self-gates.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            self.refresh();
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// Re-read + re-fold the shelf. Split from the cadence gate so the pure fold
    /// stays testable; a dark Bus yields an empty shelf (→ the #23 self card),
    /// never a panic.
    fn refresh(&mut self) {
        let states = self.client.read();
        self.units = fold_units(&states, &self.local_host);
    }

    /// The indices of `units` matching the active filter (all when `None`).
    fn filtered_indices(&self) -> Vec<usize> {
        self.units
            .iter()
            .enumerate()
            .filter(|(_, u)| self.filter.is_none_or(|c| u.kind.category() == c))
            .map(|(i, _)| i)
            .collect()
    }

    /// How many hero pages the current view has — the filtered count, or **1** for
    /// the honest self placeholder when nothing has streamed in yet (#23).
    fn hero_count(&self) -> usize {
        let n = self.filtered_indices().len();
        if n == 0 && self.filter.is_none() {
            1
        } else {
            n
        }
    }

    /// Per-category rollup counts over the whole shelf (drives the chip badges +
    /// the seed of the O2 summary strip).
    fn category_counts(&self) -> [usize; 3] {
        let mut counts = [0usize; 3];
        for unit in &self.units {
            counts[unit.kind.category().index()] += 1;
        }
        counts
    }

    /// Page one unit toward the end of the shelf (Right / ›, #6).
    fn page_next(&mut self) {
        let count = self.hero_count();
        if count > 0 {
            self.focus = (self.focus + 1).min(count - 1);
        }
    }

    /// Page one unit toward the start of the shelf (Left / ‹, #6).
    const fn page_prev(&mut self) {
        self.focus = self.focus.saturating_sub(1);
    }

    /// Set the active filter and re-anchor focus to the front of the new view.
    fn set_filter(&mut self, filter: Option<Category>) {
        if self.filter != filter {
            self.filter = filter;
            self.focus = 0;
        }
    }

    /// Render the hero-card surface (chips · hero · filmstrip). The one public
    /// entry the mount drives per frame.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        self.handle_keys(ui);
        // Keep focus valid against the freshest (possibly re-filtered) view.
        let count = self.hero_count();
        self.focus = if count == 0 {
            0
        } else {
            self.focus.min(count - 1)
        };

        egui::TopBottomPanel::top(ui.id().with("explorer-chips"))
            .frame(egui::Frame::NONE.inner_margin(Style::SP_S))
            .show_inside(ui, |ui| self.chips(ui));
        egui::TopBottomPanel::bottom(ui.id().with("explorer-strip"))
            .frame(egui::Frame::NONE.inner_margin(Style::SP_S))
            .show_inside(ui, |ui| self.filmstrip(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show_inside(ui, |ui| self.hero(ui));
    }

    /// Left/Right (and Home/End) paging (#6, O6 D-pad-first). Consumed from this
    /// frame's input; a fullscreen text surface never sees them because only the
    /// active surface renders.
    fn handle_keys(&mut self, ui: &egui::Ui) {
        let (left, right, home, end) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::Home),
                i.key_pressed(egui::Key::End),
            )
        });
        if left {
            self.page_prev();
        }
        if right {
            self.page_next();
        }
        if home {
            self.focus = 0;
        }
        if end {
            self.focus = self.hero_count().saturating_sub(1);
        }
    }

    /// The top category filter chips (#8): All + Mesh/LAN/Cloud with rollup
    /// counts, each accent-tinted (O8). Selecting one scopes the shelf.
    fn chips(&mut self, ui: &mut egui::Ui) {
        let counts = self.category_counts();
        let total = self.units.len();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = Style::SP_S;
            if chip(ui, &format!("All · {total}"), self.filter.is_none(), Style::ACCENT) {
                self.set_filter(None);
            }
            for cat in Category::ALL {
                let label = format!("{} · {}", cat.label(), counts[cat.index()]);
                let active = self.filter == Some(cat);
                if chip(ui, &label, active, cat.accent()) {
                    self.set_filter(if active { None } else { Some(cat) });
                }
            }
        });
    }

    /// The full-bleed hero card for the focused unit (#5/#9/#10/#11/#12) with
    /// Carbon slide+fade paging (#21) and side chevrons. Falls back to the honest
    /// self / empty state when nothing matches.
    fn hero(&mut self, ui: &mut egui::Ui) {
        let indices = self.filtered_indices();

        // Side chevrons (mouse affordance beside keyboard/filmstrip nav).
        let count = self.hero_count();
        let full = ui.max_rect();
        if self.chevron(ui, full, false) {
            self.page_prev();
        }
        if self.chevron(ui, full, true) {
            self.page_next();
        }

        // Carbon slide + cross-fade on a page change (#21).
        let anim_id = ui.id().with("explorer-hero-anim");
        let visual = ui
            .ctx()
            .animate_value_with_time(anim_id, self.focus as f32, Motion::BASE);
        let delta = self.focus as f32 - visual;
        let slide = (delta * full.width() * SLIDE_FRACTION).clamp(-full.width(), full.width());
        let fade = (1.0 - delta.abs()).clamp(0.0, 1.0);

        let child_rect = full.translate(Vec2::new(slide, 0.0));
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(child_rect)
                .layout(Layout::top_down(Align::Center)),
        );
        child.set_opacity(fade);

        match indices.get(self.focus).copied() {
            Some(idx) => {
                let unit = self.units[idx].clone();
                hero_card(&mut child, &unit, false);
            }
            None if self.filter.is_none() => {
                // #23 — no mirror yet: show THIS node, discovering.
                hero_card(&mut child, &self_placeholder(&self.local_host), true);
            }
            None => {
                // A filter with no matches — honest, not blank.
                child.add_space(full.height() * 0.35);
                muted_note(
                    &mut child,
                    format!(
                        "No {} units discovered yet.",
                        self.filter.map_or("", Category::label)
                    ),
                );
            }
        }
        // Page position ("3 / 12"), so the shelf's extent is always legible.
        if count > 1 {
            let pos = format!("{} / {count}", self.focus + 1);
            ui.painter().text(
                egui::pos2(full.center().x, full.max.y - Style::SP_M),
                Align2::CENTER_BOTTOM,
                pos,
                FontId::proportional(Style::SMALL),
                Style::TEXT_DIM,
            );
        }
    }

    /// A left/right paging chevron painted at the hero edge; returns whether it was
    /// clicked. Dimmed + inert at the ends of the shelf.
    fn chevron(&self, ui: &egui::Ui, hero: Rect, right: bool) -> bool {
        let count = self.hero_count();
        let enabled = if right {
            self.focus + 1 < count
        } else {
            self.focus > 0
        };
        let w = Style::SP_XL;
        let rect = if right {
            Rect::from_min_max(
                egui::pos2(hero.max.x - w, hero.center().y - w),
                egui::pos2(hero.max.x, hero.center().y + w),
            )
        } else {
            Rect::from_min_max(
                egui::pos2(hero.min.x, hero.center().y - w),
                egui::pos2(hero.min.x + w, hero.center().y + w),
            )
        };
        let id = ui.id().with(if right { "chev-r" } else { "chev-l" });
        let resp = ui.interact(rect, id, Sense::click());
        let color = if !enabled {
            Style::BORDER
        } else if resp.hovered() {
            Style::ACCENT_HI
        } else {
            Style::TEXT_DIM
        };
        let c = rect.center();
        let h = w * 0.4;
        let stroke = Stroke::new(GLYPH_STROKE_W, color);
        if right {
            ui.painter()
                .line_segment([egui::pos2(c.x - h * 0.5, c.y - h), egui::pos2(c.x + h * 0.5, c.y)], stroke);
            ui.painter()
                .line_segment([egui::pos2(c.x + h * 0.5, c.y), egui::pos2(c.x - h * 0.5, c.y + h)], stroke);
        } else {
            ui.painter()
                .line_segment([egui::pos2(c.x + h * 0.5, c.y - h), egui::pos2(c.x - h * 0.5, c.y)], stroke);
            ui.painter()
                .line_segment([egui::pos2(c.x - h * 0.5, c.y), egui::pos2(c.x + h * 0.5, c.y + h)], stroke);
        }
        enabled && resp.clicked()
    }

    /// The bottom filmstrip (#5): a horizontal strip of neighbour thumbnails with
    /// category dividers, the focused thumb accented; a click jumps the hero (#6).
    fn filmstrip(&mut self, ui: &mut egui::Ui) {
        let indices = self.filtered_indices();
        if indices.is_empty() {
            ui.allocate_space(Vec2::new(ui.available_width(), THUMB_H));
            return;
        }
        egui::ScrollArea::horizontal()
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = Style::SP_S;
                    let mut last_cat: Option<Category> = None;
                    let mut jump: Option<usize> = None;
                    for (pos, &idx) in indices.iter().enumerate() {
                        let unit = &self.units[idx];
                        let cat = unit.kind.category();
                        // Category dividers (#8) — only meaningful in the unfiltered view.
                        if self.filter.is_none() && last_cat != Some(cat) {
                            filmstrip_divider(ui, cat);
                            last_cat = Some(cat);
                        }
                        if thumbnail(ui, unit, pos == self.focus) {
                            jump = Some(pos);
                        }
                    }
                    if let Some(pos) = jump {
                        self.focus = pos;
                    }
                });
            });
    }
}

/// Synthesise this node's own hero unit for the honest empty state (#23) — a real
/// self-reference (hostname, in-mesh), never a faked peer; health stays unknown
/// (the ring spins "discovering") until a real mirror lands.
fn self_placeholder(host: &str) -> Unit {
    Unit {
        id: peer_self_id(host),
        kind: UnitKind::Peer,
        name: host.to_string(),
        reachability: Reachability::InMesh,
        address: None,
        health: None,
        telemetry: None,
        mesh: None,
        first_seen_ms: 0,
        last_seen_ms: 0,
    }
}

// ─────────────────────────── render helpers ───────────────────────────

/// A Carbon filter/nav pill; returns whether it was clicked. Active = accent
/// fill; inactive = surface with a dim border (all §4 tokens).
fn chip(ui: &mut egui::Ui, label: &str, active: bool, accent: Color32) -> bool {
    let text = RichText::new(label)
        .size(Style::SMALL)
        .color(if active { Style::BG } else { Style::TEXT });
    let button = egui::Button::new(text)
        .fill(if active { accent } else { Style::SURFACE })
        .stroke(Stroke::new(1.0, if active { accent } else { Style::BORDER }));
    ui.add(button).clicked()
}

/// A thin vertical category divider + rotated-free label between filmstrip
/// sections (#8).
fn filmstrip_divider(ui: &mut egui::Ui, cat: Category) {
    ui.vertical(|ui| {
        ui.add_space(Style::SP_XS);
        ui.label(RichText::new(cat.label()).size(Style::SMALL).color(cat.accent()));
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(Style::SP_XS, THUMB_H * 0.6), Sense::hover());
        ui.painter().line_segment(
            [rect.center_top(), rect.center_bottom()],
            Stroke::new(1.0, Style::BORDER),
        );
    });
}

/// One filmstrip thumbnail — a mini glyph + status dot + truncated name; the
/// focused thumb wears an accent border. Returns whether it was clicked (#6 jump).
fn thumbnail(ui: &mut egui::Ui, unit: &Unit, focused: bool) -> bool {
    let cat = unit.kind.category();
    let resp = ui
        .scope_builder(UiBuilder::new().sense(Sense::click()), |ui| {
            ui.set_min_size(Vec2::new(THUMB_W, THUMB_H));
            let rect = Rect::from_min_size(ui.min_rect().min, Vec2::new(THUMB_W, THUMB_H));
            let hovered = ui.rect_contains_pointer(rect);
            let border = if focused {
                cat.accent()
            } else if hovered {
                Style::ACCENT
            } else {
                Style::BORDER
            };
            ui.painter().rect_filled(rect, Style::RADIUS, Style::SURFACE);
            ui.painter()
                .rect_stroke(rect, Style::RADIUS, Stroke::new(1.0, border), StrokeKind::Inside);
            // Mini glyph.
            let glyph_c = egui::pos2(rect.center().x, rect.min.y + THUMB_H * 0.36);
            paint_kind_glyph(ui.painter(), glyph_c, THUMB_H * 0.2, unit.kind, cat.accent());
            // Status dot.
            if let Some(h) = unit.health {
                ui.painter()
                    .circle_filled(rect.right_top() + Vec2::new(-Style::SP_S, Style::SP_S), Style::SP_XS * 0.7, h.ring_color());
            }
            // Truncated name.
            let name = truncate(&unit.name, 12);
            ui.painter().text(
                egui::pos2(rect.center().x, rect.max.y - Style::SP_S),
                Align2::CENTER_BOTTOM,
                name,
                FontId::proportional(Style::SMALL),
                Style::TEXT,
            );
        })
        .response;
    resp.on_hover_text(&unit.name).clicked()
}

/// Truncate a name to `max` chars with an ellipsis, so a long id never blows the
/// thumbnail width.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

/// The hero card body (#9/#10/#11/#12): the status ring + type glyph, the
/// name/type/reachability headline, and rich telemetry when reachable else a
/// dimmed-minimal card with explicit unknowns. `discovering` renders the #23
/// self card's "Discovering units…" line.
fn hero_card(ui: &mut egui::Ui, unit: &Unit, discovering: bool) {
    let cat = unit.kind.category();
    let rich = hero_is_rich(unit);
    ui.add_space(Style::SP_L);

    // The status ring + type glyph (#9).
    let side = (ui.available_width().min(ui.available_height()) * RING_FRACTION)
        .clamp(RING_MIN, RING_MAX);
    let (ring_rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
    let center = ring_rect.center();
    let radius = side * 0.5 - RING_STROKE_W;
    let time = ui.input(|i| i.time);
    let spinning = paint_status_ring(ui.painter(), center, radius, unit.health, cat.accent(), time);
    paint_kind_glyph(ui.painter(), center, radius * 0.5, unit.kind, cat.accent());
    if spinning {
        ui.ctx().request_repaint();
    }

    ui.add_space(Style::SP_M);

    // Name + type badge + reachability (#10).
    ui.label(
        RichText::new(&unit.name)
            .size(HERO_TITLE_FS)
            .strong()
            .color(Style::TEXT),
    );
    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = Style::SP_S;
        // Centre the badge row within the top-down-centre layout.
        ui.add_space(ui.available_width() * 0.5 - Style::SP_XL * 2.0);
        ui.label(
            RichText::new(unit.kind.label())
                .size(Style::SMALL)
                .color(cat.accent())
                .background_color(Style::SURFACE_HI),
        );
        ui.label(
            RichText::new(reachability_line(&unit.reachability, unit.address.as_deref()))
                .size(Style::BODY)
                .color(Style::TEXT_DIM),
        );
    });

    ui.add_space(Style::SP_M);

    if discovering {
        muted_note(ui, "Discovering units… others stream in as they're found.");
        return;
    }

    if rich {
        hero_telemetry(ui, unit);
    } else {
        // Dimmed-minimal card (#12) — only what's known, no faked fields (§7).
        ui.scope(|ui| {
            ui.set_opacity(DIMMED_OPACITY);
            let note = match unit.reachability {
                Reachability::OnLan => "Outside the mesh — limited detail until adopted.",
                _ => "Not reachable — showing only what's known.",
            };
            muted_note(ui, note);
        });
    }

    // First/last-seen footer (E10) — real presence, honest for a fresh unit.
    if unit.last_seen_ms > 0 {
        let now = now_ms();
        let ago = fmt_seen_ago(now.saturating_sub(unit.last_seen_ms));
        let tracked = fmt_duration(unit.last_seen_ms.saturating_sub(unit.first_seen_ms) / 1_000);
        ui.add_space(Style::SP_M);
        ui.label(
            RichText::new(format!("Last seen {ago} · tracked {tracked}"))
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
    }
}

/// Whether the unit is "reachable-rich" (#11): a live mesh peer or a cloud
/// instance we can read, vs an outside/unreachable unit that gets the dimmed card.
const fn hero_is_rich(unit: &Unit) -> bool {
    match unit.kind {
        UnitKind::Peer => {
            matches!(unit.reachability, Reachability::InMesh)
                && matches!(
                    unit.health,
                    Some(Health::Healthy | Health::Degraded | Health::Critical)
                )
        }
        UnitKind::Instance => true,
        _ => false,
    }
}

/// The rich telemetry region (#11): the health pill, a peer's mesh facts
/// (role/leader/version), and any reported load/mem/uptime — or an honest
/// "Live telemetry not yet reported" line when a readable unit has none yet (§7;
/// EXPLORER-4 fills the sparklines here).
fn hero_telemetry(ui: &mut egui::Ui, unit: &Unit) {
    if let Some(health) = unit.health {
        ui.label(
            RichText::new(health_label(health))
                .size(Style::BODY)
                .color(health.ring_color()),
        );
    }
    if let Some(mesh) = &unit.mesh {
        let mut facts = Vec::new();
        if let Some(role) = &mesh.role {
            facts.push(role.clone());
        }
        if mesh.leader {
            facts.push("leader".to_string());
        }
        if let Some(v) = &mesh.mde_version {
            facts.push(format!("mde {v}"));
        }
        if !facts.is_empty() {
            ui.label(
                RichText::new(facts.join(" · "))
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
        }
    }
    match &unit.telemetry {
        Some(t) if t.any() => {
            ui.add_space(Style::SP_S);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = Style::SP_L;
                ui.add_space(ui.available_width() * 0.5 - Style::SP_XL * 2.0);
                if let Some(load) = t.load1 {
                    stat(ui, "load", format!("{load:.2}"));
                }
                if let Some(mem) = t.mem_used_pct {
                    stat(ui, "mem", format!("{mem:.0}%"));
                }
                if let Some(up) = t.uptime_s {
                    stat(ui, "uptime", fmt_duration(up));
                }
            });
        }
        _ => {
            muted_note(ui, "Live telemetry not yet reported.");
        }
    }
}

/// A compact labelled stat cell (value over caption) for the telemetry row.
fn stat(ui: &mut egui::Ui, caption: &str, value: String) {
    ui.vertical(|ui| {
        ui.label(RichText::new(value).size(Style::BODY).strong().color(Style::TEXT));
        ui.label(RichText::new(caption).size(Style::SMALL).color(Style::TEXT_DIM));
    });
}

/// The human label for a health tier.
const fn health_label(health: Health) -> &'static str {
    match health {
        Health::Healthy => "Healthy",
        Health::Degraded => "Degraded",
        Health::Critical => "Critical",
        Health::Unreachable => "Unreachable",
        Health::Unknown => "Status unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake units reader that replays preset per-node mirror states.
    struct FakeUnits(Vec<UnitsState>);
    impl UnitsClient for FakeUnits {
        fn read(&self) -> Vec<UnitsState> {
            self.0.clone()
        }
    }

    impl ExplorerState {
        /// Build headless over a fake reader + a fixed hostname, folded once.
        fn with_fake(states: Vec<UnitsState>, host: &str) -> Self {
            let mut s = Self {
                client: Box::new(FakeUnits(states)),
                local_host: host.to_string(),
                units: Vec::new(),
                focus: 0,
                filter: None,
                last_poll: None,
            };
            s.refresh();
            s
        }
    }

    fn unit(id: &str, kind: UnitKind, name: &str, last: u64) -> Unit {
        Unit {
            id: id.to_string(),
            kind,
            name: name.to_string(),
            reachability: match kind {
                UnitKind::Peer => Reachability::InMesh,
                UnitKind::LanHost => Reachability::OnLan,
                _ => Reachability::CloudObject {
                    node: "node-a".to_string(),
                },
            },
            address: None,
            health: matches!(kind, UnitKind::Peer).then_some(Health::Healthy),
            telemetry: None,
            mesh: None,
            first_seen_ms: 100,
            last_seen_ms: last,
        }
    }

    #[test]
    fn wire_mirror_decodes_a_real_aggregator_body_ignoring_daemon_only_fields() {
        // Byte-for-byte the shape `unit_aggregator::UnitsState` serialises, incl.
        // the `edges` / `published_at_ms` / cloud / extras fields the shell ignores.
        let body = r#"{
            "host":"node-a",
            "units":[{
                "id":"peer:node-a","kind":"peer","name":"node-a",
                "reachability":{"where":"in_mesh"},
                "address":"10.42.0.1","health":"healthy",
                "mesh":{"role":"lighthouse","leader":true,"mde_version":"12.0.0"},
                "cloud":null,"first_seen_ms":1,"last_seen_ms":2,"extras":{}
            }],
            "edges":[{"kind":"mesh_tunnel","from":"peer:node-a","to":"peer:node-b","detail":"direct"}],
            "published_at_ms":3
        }"#;
        let state: UnitsState = serde_json::from_str(body).expect("decodes the aggregator body");
        assert_eq!(state.host, "node-a");
        assert_eq!(state.units.len(), 1);
        let u = &state.units[0];
        assert_eq!(u.kind, UnitKind::Peer);
        assert_eq!(u.reachability, Reachability::InMesh);
        assert_eq!(u.health, Some(Health::Healthy));
        assert!(u.mesh.as_ref().is_some_and(|m| m.leader));
        // The topic prefix matches the aggregator's `state/units/<node>` shape.
        assert!(super::STATE_PREFIX.starts_with("state/units/"));
    }

    #[test]
    fn fold_dedups_by_id_keeping_the_freshest_observation() {
        // The same peer appears on two nodes' mirrors; the freshest last_seen wins.
        let a = UnitsState {
            host: "node-a".into(),
            units: vec![unit("peer:x", UnitKind::Peer, "x-old", 100)],
        };
        let b = UnitsState {
            host: "node-b".into(),
            units: vec![unit("peer:x", UnitKind::Peer, "x-new", 200)],
        };
        let folded = fold_units(&[a, b], "me");
        assert_eq!(folded.len(), 1, "deduped by id");
        assert_eq!(folded[0].name, "x-new", "freshest observation kept");
    }

    #[test]
    fn fold_orders_self_first_then_proximity_then_name() {
        let state = UnitsState {
            host: "me".into(),
            units: vec![
                unit("cloud:instance:i1", UnitKind::Instance, "web", 10),
                unit("lan:aa", UnitKind::LanHost, "printer", 10),
                unit("peer:zeta", UnitKind::Peer, "zeta", 10),
                unit("peer:me", UnitKind::Peer, "me", 10),
                unit("peer:alpha", UnitKind::Peer, "alpha", 10),
            ],
        };
        let folded = fold_units(&[state], "me");
        let ids: Vec<&str> = folded.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "peer:me",             // self first (#23)
                "peer:alpha",          // then mesh by name
                "peer:zeta",
                "lan:aa",              // then LAN
                "cloud:instance:i1",   // then cloud
            ]
        );
    }

    #[test]
    fn category_mapping_and_counts() {
        assert_eq!(UnitKind::Peer.category(), Category::Mesh);
        assert_eq!(UnitKind::LanHost.category(), Category::Lan);
        assert_eq!(UnitKind::Volume.category(), Category::Cloud);
        let s = ExplorerState::with_fake(
            vec![UnitsState {
                host: "me".into(),
                units: vec![
                    unit("peer:me", UnitKind::Peer, "me", 10),
                    unit("lan:a", UnitKind::LanHost, "a", 10),
                    unit("cloud:instance:i", UnitKind::Instance, "i", 10),
                    unit("cloud:volume:v", UnitKind::Volume, "v", 10),
                ],
            }],
            "me",
        );
        assert_eq!(s.category_counts(), [1, 1, 2]); // 1 mesh, 1 lan, 2 cloud
    }

    #[test]
    fn empty_shows_the_self_placeholder_page_hash_23() {
        let s = ExplorerState::with_fake(vec![], "anvil");
        // No mirror yet → exactly one hero page (this node), never zero/blank.
        assert_eq!(s.hero_count(), 1);
        let placeholder = self_placeholder(&s.local_host);
        assert_eq!(placeholder.id, "peer:anvil");
        assert_eq!(placeholder.name, "anvil");
        assert!(placeholder.health.is_none(), "self stays honestly unprobed");
    }

    #[test]
    fn filter_scopes_the_view_and_reanchors_focus() {
        let mut s = ExplorerState::with_fake(
            vec![UnitsState {
                host: "me".into(),
                units: vec![
                    unit("peer:me", UnitKind::Peer, "me", 10),
                    unit("lan:a", UnitKind::LanHost, "a", 10),
                    unit("cloud:instance:i", UnitKind::Instance, "i", 10),
                ],
            }],
            "me",
        );
        s.focus = 2;
        s.set_filter(Some(Category::Lan));
        assert_eq!(s.filtered_indices().len(), 1);
        assert_eq!(s.focus, 0, "focus re-anchors to the front of the new view");
        // A filter with no matches yields zero pages (honest empty, not the self card).
        s.set_filter(Some(Category::Cloud));
        assert_eq!(s.hero_count(), 1); // one instance
        let empty = ExplorerState::with_fake(vec![], "me");
        let mut empty = empty;
        empty.set_filter(Some(Category::Cloud));
        assert_eq!(empty.hero_count(), 0);
    }

    #[test]
    fn paging_clamps_at_both_ends() {
        let mut s = ExplorerState::with_fake(
            vec![UnitsState {
                host: "me".into(),
                units: vec![
                    unit("peer:me", UnitKind::Peer, "me", 10),
                    unit("lan:a", UnitKind::LanHost, "a", 10),
                    unit("cloud:instance:i", UnitKind::Instance, "i", 10),
                ],
            }],
            "me",
        );
        assert_eq!(s.hero_count(), 3);
        s.page_prev();
        assert_eq!(s.focus, 0, "clamps at the start");
        s.page_next();
        s.page_next();
        s.page_next(); // past the end
        assert_eq!(s.focus, 2, "clamps at the end");
    }

    #[test]
    fn reachability_line_is_honest_per_kind() {
        assert_eq!(
            reachability_line(&Reachability::InMesh, Some("10.42.0.1")),
            "In mesh · 10.42.0.1"
        );
        assert_eq!(reachability_line(&Reachability::OnLan, None), "On LAN");
        assert_eq!(
            reachability_line(
                &Reachability::CloudObject {
                    node: "bigboy".into()
                },
                None
            ),
            "Cloud object · bigboy"
        );
    }

    #[test]
    fn rich_vs_dimmed_classification() {
        // A live in-mesh peer → rich; an off-mesh LAN host → dimmed-minimal (#12).
        assert!(hero_is_rich(&unit("peer:x", UnitKind::Peer, "x", 10)));
        assert!(!hero_is_rich(&unit("lan:a", UnitKind::LanHost, "a", 10)));
        assert!(hero_is_rich(&unit(
            "cloud:instance:i",
            UnitKind::Instance,
            "i",
            10
        )));
        // A volume/image/network is a summary card, not rich telemetry.
        assert!(!hero_is_rich(&unit("cloud:volume:v", UnitKind::Volume, "v", 10)));
    }

    #[test]
    fn fmt_duration_reads_compactly() {
        assert_eq!(fmt_duration(30), "30s");
        assert_eq!(fmt_duration(90), "1m");
        assert_eq!(fmt_duration(3_720), "1h 2m");
        assert_eq!(fmt_duration(90_000), "1d 1h");
    }

    #[test]
    fn hero_card_renders_headless_across_states() {
        // Exercise the real render (glyphs, ring, telemetry, dimmed, empty) so a
        // panic in any painter path is caught headless — no GPU, like backdrop's.
        let states = vec![UnitsState {
            host: "me".into(),
            units: vec![
                Unit {
                    mesh: Some(MeshFacts {
                        role: Some("lighthouse".into()),
                        leader: true,
                        mde_version: Some("12.0.0".into()),
                    }),
                    telemetry: Some(Telemetry {
                        load1: Some(0.42),
                        mem_used_pct: Some(37.0),
                        uptime_s: Some(90_061),
                    }),
                    ..unit("peer:me", UnitKind::Peer, "me", now_ms())
                },
                unit("lan:aa", UnitKind::LanHost, "printer", now_ms()),
                unit("cloud:instance:i1", UnitKind::Instance, "web", now_ms()),
            ],
        }];

        for filter in [None, Some(Category::Mesh), Some(Category::Lan), Some(Category::Cloud)] {
            let mut s = ExplorerState::with_fake(states.clone(), "me");
            s.set_filter(filter);
            let ctx = egui::Context::default();
            Style::install(&ctx);
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(1200.0, 800.0))),
                ..Default::default()
            };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| s.show(ui));
            });
            let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
            assert!(!prims.is_empty(), "the hero surface drew primitives");
        }

        // And the honest empty (#23) self card renders too.
        let mut empty = ExplorerState::with_fake(vec![], "solo");
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(1000.0, 700.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| empty.show(ui));
        });
        assert!(!ctx.tessellate(out.shapes, out.pixels_per_point).is_empty());
    }
}
