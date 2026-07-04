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
//! - **Rich telemetry sparklines (EXPLORER-4).** The card shows a live status
//!   ring, the mesh facts (role/leader/version), and a **metric grid** of
//!   load / mem / net / uptime. Load and mem draw a real **sparkline** built from
//!   the samples this shell has actually polled over time (a rolling per-unit
//!   history, never synthesised — §7); a metric with no live source (net) or a
//!   scalar-only metric (uptime) stays honestly dimmed rather than faking a
//!   trend. The per-type action bars (EXPLORER-5) fill the same card without
//!   re-wiring this mount.
//!
//! ## Per-type action bars (EXPLORER-5)
//!
//! The hero card grows a **launchpad** action bar under the telemetry, keyed on
//! the focused unit's kind. Every verb drives a **real seam** (§7 — no dead
//! buttons); a verb with no reachable seam is honestly disabled with its reason
//! on hover, never a no-op:
//!
//! - **Cloud instance** — `Console` (routes to the Desktop/VDI surface when the
//!   instance reports an address, else honestly disabled), and
//!   `Start` / `Stop` / `Reboot` / `Delete` — each publishes an [`InstanceReq`]
//!   on `action/cloud/<verb>`, the **QC-11** typed cloud bus the openstack worker
//!   drains (§6 — a wire mirror, not a daemon-crate link, over the SAME Bus this
//!   surface reads `state/units` from). A volume/image/network hero offers
//!   `Inspect` (routes to the Cloud surface); its `Delete` is honestly disabled
//!   (the QC-11 verb set is instance-only).
//! - **Peer** — `Open in Fleet` (routes to the Mesh view), a live `Health-check`
//!   (re-requests the aggregator's `action/units/get-stream` stream), and
//!   `Evict` (honestly disabled — no bus eviction verb yet).
//! - **LAN host** — `Invite to mesh` routes to the Provisioning plane (the
//!   existing pairing/enrollment flow) plus a `Health-check` refresh.
//!
//! **Arming (§14/§15/§16).** Every destructive verb (instance stop/reboot/
//! delete, the LAN invite) is gated behind the platform **typed-arming** confirm
//! — the exact `surface_card` / `mde-files` idiom: the verb arms on the first
//! click, then fires only once the operator types the unit's name back
//! ([`ExplorerState::confirm_armed`]). Non-destructive verbs fire immediately.
//! Routing to another surface reuses the shell's ONE navigation grammar — the
//! `shell/goto/*` · `shell/plane/*` toast chyron [`crate::toast_bridge`] resolves
//! (the same seam [`crate::storage`]'s walled-row hand-off publishes), so this
//! surface never touches the dock/`Surface` plumbing itself.

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

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_egui::egui::{
    self, Align, Align2, Color32, FontId, Layout, Rect, RichText, Sense, Stroke, StrokeKind,
    UiBuilder, Vec2,
};
use mde_egui::{muted_note, Motion, Style};

use crate::toast_bridge::TOAST_TOPIC;

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

// ── Telemetry sparklines (EXPLORER-4) ──
/// Rolling telemetry history depth — 60 points ≈ 5 minutes at the 5 s poll
/// cadence. A ring-buffer behaviour cap, not a metric literal.
const HISTORY_LEN: usize = 60;
/// A metric cell's / sparkline's plot width (2.5 XL grid steps — wide enough for
/// a legible trend under the big value). A §4-grid behaviour param.
const SPARK_W: f32 = Style::SP_XL * 2.5;
/// Sparkline plot height (one L step).
const SPARK_H: f32 = Style::SP_L;
/// A metric cell's full height: the value line + the plot + the caption.
const METRIC_CELL_H: f32 = Style::SP_XL * 2.25;
/// Sparkline polyline stroke width (a painter behaviour param, like the ring).
const SPARK_STROKE_W: f32 = 1.5;
/// Memory sparkline full-scale — a 0–100 % axis.
const MEM_FULL_SCALE: f32 = 100.0;
/// Load sparkline reference ceiling: scale to at least 1.0 (one core busy) so an
/// idle line reads low, and let a real peak above it expand the axis (never
/// clipped) rather than pinning to a fabricated maximum.
const LOAD_REF_CEIL: f32 = 1.0;

// ── IPAM table mode (EXPLORER-10, design E7) ──
/// The prefix length every discovered address is aggregated under: the /24
/// broadcast-domain granularity (the conventional subnet unit the aggregator's
/// L2/L3-adjacency edges already reason in). A live-discovered *view*, not a
/// manual netmask allocation (E3) — the network is the source of truth.
const IPAM_PREFIX_BITS: u32 = 24;
/// Usable host addresses in a /24 (256 minus the network + broadcast) — the
/// denominator for the honest free/used capacity readout.
const IPAM_USABLE_PER_24: usize = 254;
/// One IPAM table row's height (a productive-density row — one L grid step).
const IPAM_ROW_H: f32 = Style::SP_L;
/// The fixed address column width — wide enough for a full dotted-quad in the
/// mono face. A §4-grid behaviour param, not a scattered px.
const IPAM_ADDR_COL: f32 = Style::SP_XL * 4.0;
/// The fixed type-badge column width (right-aligned).
const IPAM_TYPE_COL: f32 = Style::SP_XL * 3.0;
/// The prefix-capacity meter width (used/free bar in the prefix header).
const IPAM_BAR_W: f32 = Style::SP_XL * 3.0;

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

/// The kind of a derived relationship between two units — a **local** mirror of
/// the aggregator's `edges::EdgeKind` (EXPLORER-7, design E2). The variant names
/// AND the `rename_all` MUST match the worker's enum so the wire tokens
/// (`mesh_tunnel` / `cloud_attach` / `l2_l3_adjacency` / `host_placement` /
/// `storage_usage`) decode byte-for-byte (§6 — mirror the contract, never link the
/// daemon crate). An unknown future kind fails only that edge's parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EdgeKind {
    /// A mesh tunnel between two peers — direct or relayed via a lighthouse.
    MeshTunnel,
    /// A cloud attachment: instance→network/volume/image, network→subnet/router.
    CloudAttach,
    /// L2/L3 adjacency: two LAN hosts sharing a subnet (one broadcast domain).
    L2L3Adjacency,
    /// Host placement: a cloud object runs on a mesh node (the DCIM relation).
    HostPlacement,
    /// Storage usage: a volume attached to an instance / backed by a pool.
    StorageUsage,
}

/// One typed relationship between two units — a **local** mirror of the
/// aggregator's `edges::Edge` (EXPLORER-7, design E8). `from`/`to` are the fold's
/// stable unit ids (`peer:` / `lan:` / `cloud:<kind>:`) — except the not-modelled
/// subnet/router/pool endpoints, which carry their own prefixed ids; a chip only
/// jumps when the endpoint resolves to a unit on the shelf (§7 — every chip a real
/// hero, never a dead link).
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct Edge {
    /// The relation kind (drives the chip section).
    kind: EdgeKind,
    /// The source unit id.
    from: String,
    /// The target unit id (or a non-unit subnet/router/pool endpoint id).
    to: String,
    /// A short human-readable qualifier (`direct` / `runs on node-a` …); absent
    /// when the worker had nothing to add.
    #[serde(default)]
    detail: Option<String>,
}

/// The body published to `state/units/<node>` — a mirror of the aggregator's
/// `UnitsState` (the fields the shell reads; `published_at_ms` stays ignored). The
/// typed `edges` set (EXPLORER-7) rides alongside the units and drives the
/// hero-card edge chips (EXPLORER-8).
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
struct UnitsState {
    /// The publishing node id.
    host: String,
    /// Every unit that node folded.
    units: Vec<Unit>,
    /// The typed relationships derived from the same unioned sources (EXPLORER-7).
    edges: Vec<Edge>,
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

// ─────────────────────── the action-dispatch seam (EXPLORER-5) ───────────────────────

/// The `action/cloud/` namespace prefix every QC-11 cloud verb request rides —
/// a **local mirror** of `mackesd::workers::openstack::verbs::CLOUD_ACTION_PREFIX`
/// (§6: the shell mirrors the wire contract, never links the daemon crate). A
/// byte-pinned test keeps it equal to the worker's prefix.
const CLOUD_ACTION_PREFIX: &str = "action/cloud/";

/// The aggregator's E9 pull verb — a mirror of
/// `mackesd::workers::unit_aggregator::verb::UNITS_REQUEST_TOPIC`. Publishing an
/// (empty) request forces a fresh live unit stream: the honest "health-check".
const UNITS_REQUEST_TOPIC: &str = "action/units/get-stream";

/// The Bus topic for cloud verb `verb`: `action/cloud/<verb>`.
fn cloud_topic(verb: &str) -> String {
    format!("{CLOUD_ACTION_PREFIX}{verb}")
}

/// The shell's mirror of the openstack worker's `InstanceRequest` — the typed
/// body a lifecycle verb takes (§6: serialises to the identical `{"instance":…}`
/// body `verbs::parse_instance_request` decodes, never a daemon-crate link).
#[derive(Debug, Serialize)]
struct InstanceReq {
    /// The Nova server id (or name) to act on.
    instance: String,
}

/// The injectable publish seam over the Bus — production writes each request
/// through [`Persist`] ([`BusActions`]); tests inject a recording fake so the
/// dispatched topic + body are asserted headless. The same seam pattern
/// [`UnitsClient`] uses for the read side.
trait ActionSink {
    /// Publish `body` on `topic` (a request / navigation chyron). `Err` carries a
    /// human-readable reason (no Bus dir, a write fault) for the honest note.
    fn publish(&self, topic: &str, body: &str) -> Result<(), String>;
}

/// The production sink: append the request to the desktop-client Bus spool — the
/// SAME persist-first path [`crate::services_flow`] and [`crate::storage`] publish
/// their actions through.
struct BusActions {
    /// The desktop-client Bus spool root (`None` ⇒ no Bus ⇒ an honest "no Bus"
    /// error, never a silent success).
    bus_root: Option<PathBuf>,
}

impl ActionSink for BusActions {
    fn publish(&self, topic: &str, body: &str) -> Result<(), String> {
        let root = self
            .bus_root
            .clone()
            .ok_or_else(|| "No mesh Bus directory — join this node to a mesh first.".to_string())?;
        Persist::open(root)
            .and_then(|p| p.write(topic, Priority::Default, None, Some(body)))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// The Nova object id a cloud unit acts on: strip the aggregator's
/// `cloud:<kind>:` id prefix (`sources::CloudKind::unit_id`) back to the bare
/// object id the QC-11 verb targets; a non-cloud id falls through unchanged.
fn cloud_object_id(unit: &Unit) -> String {
    let mut parts = unit.id.splitn(3, ':');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("cloud"), Some(_kind), Some(object)) => object.to_string(),
        _ => unit.id.clone(),
    }
}

/// A navigation chyron body on the shell's ONE toast lane — the same shape
/// [`crate::storage::StorageState::emit_goto`] / `chat::navigate_via_toast`
/// publish, so KIRON-2's bridge (the shell's single nav authority) carries the
/// operator to `verb`'s target surface/plane.
fn nav_body(source_host: &str, headline: &str, verb: &str) -> String {
    serde_json::json!({
        "severity": "info",
        "source_host": source_host,
        "flag": "EXPLORER",
        "headline": headline,
        "action_label": "Open",
        "action_verb": verb,
    })
    .to_string()
}

/// One hero verb the operator can trigger, keyed on the focused unit's kind. The
/// real seam each reaches is resolved by [`verb_seam`]; a verb whose seam is
/// `Err` is honestly disabled (§7 — never a no-op button).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    /// Instance: open a SPICE/VNC console (routes to the Desktop/VDI surface).
    Console,
    /// Instance: `openstack server start`.
    Start,
    /// Instance: `openstack server stop` — destructive (armed).
    Stop,
    /// Instance: `openstack server reboot` — destructive (armed).
    Reboot,
    /// Instance: `openstack server delete` — destructive (armed).
    Delete,
    /// Volume/image/network: inspect (routes to the Cloud surface).
    Inspect,
    /// Volume/image/network: delete — no QC-11 verb yet (honestly disabled).
    ObjectDelete,
    /// Peer: open in the Fleet mesh view.
    OpenInFleet,
    /// Peer/LAN: re-request the live unit stream (a health refresh).
    HealthCheck,
    /// Peer: evict from the mesh — no bus verb yet (honestly disabled).
    Evict,
    /// LAN host: invite to the mesh — routes to Provisioning; destructive (armed).
    Invite,
}

impl Verb {
    /// The button label.
    const fn label(self) -> &'static str {
        match self {
            Self::Console => "Console",
            Self::Start => "Start",
            Self::Stop => "Stop",
            Self::Reboot => "Reboot",
            Self::Delete | Self::ObjectDelete => "Delete",
            Self::Inspect => "Inspect",
            Self::OpenInFleet => "Open in Fleet",
            Self::HealthCheck => "Health-check",
            Self::Evict => "Evict",
            Self::Invite => "Invite to mesh",
        }
    }

    /// Whether this verb mutates the fleet's trust/lifecycle state and so must
    /// pass the typed-arming confirm before it fires. (`ObjectDelete`/`Evict`
    /// carry the flag too, but their seam is disabled so arming is never reached.)
    const fn destructive(self) -> bool {
        matches!(
            self,
            Self::Stop
                | Self::Reboot
                | Self::Delete
                | Self::ObjectDelete
                | Self::Evict
                | Self::Invite
        )
    }
}

/// The verbs a unit of `kind` offers, in bar order.
const fn verbs_for(kind: UnitKind) -> &'static [Verb] {
    match kind {
        UnitKind::Instance => &[
            Verb::Console,
            Verb::Start,
            Verb::Stop,
            Verb::Reboot,
            Verb::Delete,
        ],
        UnitKind::Volume | UnitKind::Image | UnitKind::Network => {
            &[Verb::Inspect, Verb::ObjectDelete]
        }
        UnitKind::Peer => &[Verb::OpenInFleet, Verb::HealthCheck, Verb::Evict],
        UnitKind::LanHost => &[Verb::Invite, Verb::HealthCheck],
    }
}

/// A resolved real seam a verb dispatches through.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HeroAction {
    /// Publish an [`InstanceReq`] on `action/cloud/<verb>` (QC-11).
    Cloud {
        /// The `instance-*` verb stem.
        verb: &'static str,
        /// The target Nova object id.
        instance: String,
    },
    /// Publish the units get-stream request (a live health refresh).
    Refresh,
    /// Raise a navigation chyron for `verb` (`shell/goto/*` · `shell/plane/*`).
    Goto {
        /// The nav-grammar verb the toast bridge resolves.
        verb: String,
        /// The chyron headline naming the hand-off.
        headline: String,
    },
}

/// Resolve a verb on `unit` to its real seam, or an honest reason it is disabled
/// (§7 — a verb with no reachable seam is never a live no-op button).
fn verb_seam(verb: Verb, unit: &Unit) -> Result<HeroAction, String> {
    let cloud = |stem: &'static str| HeroAction::Cloud {
        verb: stem,
        instance: cloud_object_id(unit),
    };
    match verb {
        Verb::Console => match unit.address.as_deref() {
            Some(addr) if !addr.is_empty() => Ok(HeroAction::Goto {
                verb: "shell/goto/desktop".to_string(),
                headline: format!("Open the Desktop surface to reach {} ({addr}).", unit.name),
            }),
            _ => Err("No console endpoint reported yet.".to_string()),
        },
        Verb::Start => Ok(cloud("instance-start")),
        Verb::Stop => Ok(cloud("instance-stop")),
        Verb::Reboot => Ok(cloud("instance-reboot")),
        Verb::Delete => Ok(cloud("instance-delete")),
        Verb::Inspect => Ok(HeroAction::Goto {
            verb: "shell/goto/instances".to_string(),
            headline: format!("Open the Cloud surface to inspect {}.", unit.name),
        }),
        Verb::ObjectDelete => Err(format!(
            "{} deletion isn't on the cloud bus yet — instance lifecycle only.",
            unit.kind.label()
        )),
        Verb::OpenInFleet => Ok(HeroAction::Goto {
            verb: "shell/goto/mesh".to_string(),
            headline: format!("Open {} in the Fleet mesh view.", unit.name),
        }),
        Verb::HealthCheck => Ok(HeroAction::Refresh),
        Verb::Evict => Err("Mesh eviction isn't exposed on the bus yet.".to_string()),
        Verb::Invite => Ok(HeroAction::Goto {
            verb: "shell/plane/provisioning".to_string(),
            headline: format!("Bring {} into the mesh in Provisioning.", unit.name),
        }),
    }
}

/// The honest inline note after a verb fires (never a fabricated result — it
/// states the request was published, not that the fleet has acted, §7).
fn done_note(verb: Verb, unit: &Unit) -> String {
    match verb {
        Verb::Console => format!("Opening the console surface for {}…", unit.name),
        Verb::Start => format!("Start requested for {}.", unit.name),
        Verb::Stop => format!("Stop requested for {}.", unit.name),
        Verb::Reboot => format!("Reboot requested for {}.", unit.name),
        Verb::Delete => format!("Delete requested for {}.", unit.name),
        Verb::Inspect => format!("Opening the Cloud surface for {}…", unit.name),
        Verb::OpenInFleet => format!("Opening {} in the Fleet view…", unit.name),
        Verb::HealthCheck => "Re-requested the live unit stream.".to_string(),
        Verb::Invite => format!("Opening Provisioning to invite {}…", unit.name),
        // Disabled seams never fire; kept exhaustive for the honest fallback.
        Verb::ObjectDelete | Verb::Evict => "No reachable seam.".to_string(),
    }
}

/// A destructive verb armed on one unit, awaiting its typed-name confirm — the
/// platform typed-arming interlock (the `surface_card` / `mde-files` idiom).
struct ArmedVerb {
    /// The unit id the verb is armed against (arming is per-unit).
    unit_id: String,
    /// Which destructive verb is armed.
    verb: Verb,
    /// The operator's typed echo, matched against the unit name to arm.
    echo: String,
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

/// Union every node's published edge set into one, deduped by the `(kind, from,
/// to)` triple. Each node already derives + dedups its own set (EXPLORER-7), but a
/// peer that mirrors another node's view republishes the same edges, so the union
/// collapses those cross-node duplicates. Pure — the chip region's data model,
/// unit-tested without a Bus.
fn fold_edges(states: &[UnitsState]) -> Vec<Edge> {
    let mut seen: HashSet<(EdgeKind, String, String)> = HashSet::new();
    let mut edges = Vec::new();
    for state in states {
        for edge in &state.edges {
            if seen.insert((edge.kind, edge.from.clone(), edge.to.clone())) {
                edges.push(edge.clone());
            }
        }
    }
    edges
}

// ─────────────────── edge-chip grouping (EXPLORER-8, design E1/E6) ───────────────────

/// One jump chip: a related unit reachable from the focused hero over an edge —
/// its name + kind glyph, and the id the click jumps focus to. Only units actually
/// on the shelf become chips (§7 — a chip always lands on a real hero, never a dead
/// subnet/router/pool endpoint the aggregator left unmodelled).
#[derive(Debug, Clone, PartialEq)]
struct ChipItem {
    /// The neighbour unit id the chip jumps to.
    id: String,
    /// Its display name.
    name: String,
    /// Its kind (drives the glyph + category accent).
    kind: UnitKind,
}

/// One grouped chip section on the hero card: a header (the design's Tunnels /
/// Networks / Volumes / Same subnet / Runs on `<node>` / Storage) + its row of
/// jump chips. A section only exists when it has ≥1 chip (absent kinds omitted).
#[derive(Debug, Clone, PartialEq)]
struct EdgeSection {
    /// The section header.
    header: String,
    /// Its jump chips, ordered by neighbour name.
    chips: Vec<ChipItem>,
}

/// The other endpoint of `edge` relative to the focused unit `focus_id`, or `None`
/// when the edge isn't incident to it. Handles both symmetric (peer↔peer,
/// host↔host) and directed edges — the focus can sit on either end.
fn neighbor_of<'a>(edge: &'a Edge, focus_id: &str) -> Option<&'a str> {
    if edge.from == focus_id {
        Some(edge.to.as_str())
    } else if edge.to == focus_id {
        Some(edge.from.as_str())
    } else {
        None
    }
}

/// The host node name a `HostPlacement` edge names — its directed `to` endpoint is
/// always the node's `peer:<node>` unit, so strip that prefix (falling back to the
/// raw id if the shape ever drifts).
fn placement_node(edge: &Edge) -> &str {
    edge.to.strip_prefix("peer:").unwrap_or(&edge.to)
}

/// The chip section an incident `edge` falls under, from the focused unit's view:
/// its display rank (the design's ordering — Tunnels, Networks, Volumes, …, Same
/// subnet, Runs on `<node>`, Storage) + the section header. A cloud attachment is
/// grouped by what the neighbour **is** (so it reads correctly whichever end is
/// focused): a network → Networks, a volume → Volumes, an image → Images, and the
/// reverse (an instance) → Instances.
fn section_for(edge: &Edge, neighbor: &Unit) -> (u8, String) {
    match edge.kind {
        EdgeKind::MeshTunnel => (0, "Tunnels".to_string()),
        EdgeKind::CloudAttach => match neighbor.kind {
            UnitKind::Network => (1, "Networks".to_string()),
            UnitKind::Volume => (2, "Volumes".to_string()),
            UnitKind::Image => (3, "Images".to_string()),
            _ => (4, "Instances".to_string()),
        },
        EdgeKind::L2L3Adjacency => (5, "Same subnet".to_string()),
        EdgeKind::HostPlacement => (6, format!("Runs on {}", placement_node(edge))),
        EdgeKind::StorageUsage => (7, "Storage".to_string()),
    }
}

// ─────────────────── IPAM prefix aggregation (EXPLORER-10, E7) ───────────────────

/// One occupied address within a discovered prefix — a unit that reported an IPv4
/// address in this /24. Carries what the row renders + the id the row-click jumps
/// to. Real discovery only (§7): a unit with no address is never a phantom slot.
#[derive(Debug, Clone, PartialEq, Eq)]
struct IpamOccupant {
    /// The occupant's IPv4 address.
    addr: Ipv4Addr,
    /// The occupant unit's id — the hero the row jumps to.
    unit_id: String,
    /// The occupant unit's display name.
    name: String,
    /// The occupant's kind (drives the type badge + category accent).
    kind: UnitKind,
}

/// One discovered subnet/prefix in the IPAM table (design E7): a /24 the fold
/// derived purely from occupant addresses, its occupants, and the category it
/// reads as. A **live-discovered mirror** — no manual allocation, no CIDR the
/// network didn't tell us (E3). The gateway + capacity are conventional derivations
/// over the /24, honest about what's real (occupants) vs conventional (the .1).
#[derive(Debug, Clone, PartialEq, Eq)]
struct IpamPrefix {
    /// The /24 network address (last octet zeroed).
    network: Ipv4Addr,
    /// The proximity category (from the dominant occupant kind) — the accent.
    category: Category,
    /// A discovered tenant-net name, when a `CloudAttach` edge links an occupant
    /// to a `cloud:network` unit on the shelf (EXPLORER-7). `None` for mesh/LAN
    /// prefixes (no network object to name them). Never fabricated (§7).
    label: Option<String>,
    /// The occupants, ordered by address then id.
    occupants: Vec<IpamOccupant>,
}

impl IpamPrefix {
    /// The CIDR string, e.g. `10.42.0.0/24`.
    fn cidr(&self) -> String {
        format!("{}/{IPAM_PREFIX_BITS}", self.network)
    }

    /// The conventional gateway address — the prefix's first host (`.1`). A derived
    /// convention (what every IPAM tool shows), not a probed fact.
    const fn gateway(&self) -> Ipv4Addr {
        let o = self.network.octets();
        Ipv4Addr::new(o[0], o[1], o[2], 1)
    }

    /// The count of **distinct** occupied addresses (two units on one address count
    /// once) — the honest "used" tally.
    fn used(&self) -> usize {
        let mut n = 0;
        let mut prev: Option<Ipv4Addr> = None;
        for o in &self.occupants {
            if prev != Some(o.addr) {
                n += 1;
                prev = Some(o.addr);
            }
        }
        n
    }

    /// The free host count over the /24's usable range (never underflows).
    fn free(&self) -> usize {
        IPAM_USABLE_PER_24.saturating_sub(self.used())
    }
}

/// Parse a unit address to an IPv4, tolerating a `/mask` CIDR suffix or a `:port`
/// tail and surrounding whitespace; `None` for an absent / IPv6 / unparseable
/// address (those units simply don't occupy an IPv4 prefix — honest, not faked).
fn parse_ipv4(addr: &str) -> Option<Ipv4Addr> {
    let head = addr.trim();
    let head = head.split('/').next().unwrap_or(head);
    if let Ok(ip) = head.parse::<Ipv4Addr>() {
        return Some(ip);
    }
    head.rsplit_once(':')
        .and_then(|(h, _)| h.parse::<Ipv4Addr>().ok())
}

/// The /24 network address an IPv4 falls in (last octet zeroed).
const fn slash24(ip: Ipv4Addr) -> Ipv4Addr {
    let o = ip.octets();
    Ipv4Addr::new(o[0], o[1], o[2], 0)
}

/// The proximity category a prefix reads as: the most common occupant category,
/// tie-broken toward proximity order (mesh → LAN → cloud) so a mixed prefix is
/// deterministic.
fn dominant_category(occupants: &[IpamOccupant]) -> Category {
    let mut counts = [0usize; 3];
    for o in occupants {
        counts[o.kind.category().index()] += 1;
    }
    let mut best = Category::Mesh;
    let mut best_n = 0usize;
    for cat in Category::ALL {
        if counts[cat.index()] > best_n {
            best_n = counts[cat.index()];
            best = cat;
        }
    }
    best
}

/// A discovered tenant-net name for `occupants`, from the EXPLORER-7 edge set: the
/// first `CloudAttach` edge that links an occupant to a `cloud:network` unit on the
/// shelf. Occupants + edges are pre-sorted so the pick is deterministic. `None`
/// when no network object names the prefix (mesh/LAN) — never invented (§7).
fn network_label(
    occupants: &[IpamOccupant],
    edges: &[Edge],
    by_id: &HashMap<&str, &Unit>,
) -> Option<String> {
    for occ in occupants {
        for edge in edges {
            if edge.kind != EdgeKind::CloudAttach {
                continue;
            }
            let other = if edge.from == occ.unit_id {
                &edge.to
            } else if edge.to == occ.unit_id {
                &edge.from
            } else {
                continue;
            };
            if let Some(net) = by_id.get(other.as_str()) {
                if net.kind == UnitKind::Network {
                    return Some(net.name.clone());
                }
            }
        }
    }
    None
}

/// Aggregate the folded unit shelf (+ the EXPLORER-7 edges) into the IPAM table:
/// every /24 an addressed unit occupies, its occupants, capacity, and — for a
/// tenant net — its discovered name. Pure over the fold (no probe, no allocation),
/// so the aggregation + occupancy are unit-tested without a Bus or a render.
fn derive_prefixes(units: &[Unit], edges: &[Edge]) -> Vec<IpamPrefix> {
    let by_id: HashMap<&str, &Unit> = units.iter().map(|u| (u.id.as_str(), u)).collect();
    let mut buckets: BTreeMap<Ipv4Addr, Vec<IpamOccupant>> = BTreeMap::new();
    for unit in units {
        let Some(addr) = unit.address.as_deref().and_then(parse_ipv4) else {
            continue;
        };
        buckets
            .entry(slash24(addr))
            .or_default()
            .push(IpamOccupant {
                addr,
                unit_id: unit.id.clone(),
                name: unit.name.clone(),
                kind: unit.kind,
            });
    }
    let mut prefixes: Vec<IpamPrefix> = buckets
        .into_iter()
        .map(|(network, mut occupants)| {
            occupants.sort_by(|a, b| a.addr.cmp(&b.addr).then_with(|| a.unit_id.cmp(&b.unit_id)));
            let category = dominant_category(&occupants);
            let label = network_label(&occupants, edges, &by_id);
            IpamPrefix {
                network,
                category,
                label,
                occupants,
            }
        })
        .collect();
    // Proximity order (mesh → LAN → cloud), then by network address.
    prefixes.sort_by(|a, b| {
        a.category
            .index()
            .cmp(&b.category.index())
            .then_with(|| a.network.cmp(&b.network))
    });
    prefixes
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
fn paint_kind_glyph(
    painter: &egui::Painter,
    center: egui::Pos2,
    r: f32,
    kind: UnitKind,
    color: Color32,
) {
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
                [
                    egui::pos2(center.x, screen.max.y),
                    egui::pos2(center.x, base_y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(center.x - r * 0.5, base_y),
                    egui::pos2(center.x + r * 0.5, base_y),
                ],
                stroke,
            );
        }
        // Instance — three stacked server bays, each with an indicator dot.
        UnitKind::Instance => {
            for k in 0u8..3 {
                let cy = center.y + (f32::from(k) - 1.0) * r * 0.7;
                let bay =
                    Rect::from_center_size(egui::pos2(center.x, cy), Vec2::new(r * 1.7, r * 0.5));
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
                [
                    egui::pos2(body.min.x + r * 0.25, bar_y),
                    egui::pos2(body.max.x - r * 0.25, bar_y),
                ],
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
                center
                    + Vec2::angled(std::f32::consts::FRAC_PI_2 + std::f32::consts::FRAC_PI_3) * r,
                center
                    + Vec2::angled(std::f32::consts::FRAC_PI_2 - std::f32::consts::FRAC_PI_3) * r,
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

// ─────────────────────────── telemetry history (EXPLORER-4) ───────────────────────────

/// A rolling ring of the real telemetry samples this shell has actually observed
/// for one unit — the honest sparkline source (§7). Every point is a value read
/// from a live mirror on a past poll, never synthesised: an empty series ⇒ the
/// metric is honestly dimmed, never a fabricated demo curve. The daemon publishes
/// scalars (EXPLORER-1) and the shell folds each poll's reading into the trend,
/// so a live peer grows a genuine load/mem history without a new probe.
#[derive(Default)]
struct UnitHistory {
    /// Observed 1-minute load averages, oldest → newest.
    load1: VecDeque<f32>,
    /// Observed memory-used percentages, oldest → newest.
    mem_used_pct: VecDeque<f32>,
}

impl UnitHistory {
    /// Fold this poll's readable scalars into the trend — a metric absent this
    /// tick simply isn't recorded (its series stays honest, §7).
    fn record(&mut self, t: &Telemetry) {
        if let Some(v) = t.load1 {
            push_bounded(&mut self.load1, v);
        }
        if let Some(v) = t.mem_used_pct {
            push_bounded(&mut self.mem_used_pct, v);
        }
    }
}

/// Push `v` onto a bounded ring, dropping the oldest sample past [`HISTORY_LEN`].
fn push_bounded(ring: &mut VecDeque<f32>, v: f32) {
    if ring.len() >= HISTORY_LEN {
        ring.pop_front();
    }
    ring.push_back(v);
}

// ─────────────────────────── the surface state ───────────────────────────

/// Which surface mode the Explorer renders (design O1's three modes; the mosaic
/// overview is EXPLORER-11). EXPLORER-10 adds the **IPAM** address table beside the
/// hero card, toggled from the header — the category filter chips scope both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SurfaceMode {
    /// The one-unit-at-a-time hero card (EXPLORER-3) — the default landing.
    #[default]
    Hero,
    /// The NetBox-style discovered prefix/IP table (EXPLORER-10).
    Ipam,
}

/// The Discovery-surface hero-card state (EXPLORER-3): the folded unit shelf, the
/// focused index, the active category filter, and the Bus read seam.
pub struct ExplorerState {
    /// The units read seam (Bus in production, a fake in tests).
    client: Box<dyn UnitsClient>,
    /// This node's hostname — its self hero unit (#23) + first-sort key.
    local_host: String,
    /// The folded shelf: deduped, self-first, proximity-ordered.
    units: Vec<Unit>,
    /// The folded typed edge set (EXPLORER-7) — every node's mirror unioned +
    /// deduped, the source of the hero-card edge chips (EXPLORER-8).
    edges: Vec<Edge>,
    /// Rolling per-unit telemetry history keyed by unit id — the sparkline source
    /// (EXPLORER-4), sampled each refresh from real readings only (§7).
    history: HashMap<String, UnitHistory>,
    /// The focused hero index, into the currently-**filtered** view.
    focus: usize,
    /// The active category filter (`None` ⇒ all, #8).
    filter: Option<Category>,
    /// When the Bus was last polled (drives the fixed cadence).
    last_poll: Option<Instant>,
    /// The publish seam the action bar dispatches verbs through (EXPLORER-5) —
    /// production writes the Bus; tests inject a recording fake.
    action_sink: Box<dyn ActionSink>,
    /// The destructive verb currently armed (awaiting its typed-name confirm), if
    /// any — the typed-arming interlock (EXPLORER-5).
    arm: Option<ArmedVerb>,
    /// The last verb's honest inline note (`true` ⇒ an error/gated reason).
    last_action_note: Option<(String, bool)>,
    /// The active surface mode — the hero card or the IPAM table (EXPLORER-10).
    mode: SurfaceMode,
}

impl Default for ExplorerState {
    fn default() -> Self {
        Self {
            client: Box::new(BusUnits {
                bus_root: mde_bus::client_data_dir(),
            }),
            local_host: local_hostname(),
            units: Vec::new(),
            edges: Vec::new(),
            history: HashMap::new(),
            focus: 0,
            filter: None,
            last_poll: None,
            action_sink: Box::new(BusActions {
                bus_root: mde_bus::client_data_dir(),
            }),
            arm: None,
            last_action_note: None,
            mode: SurfaceMode::default(),
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
        self.edges = fold_edges(&states);
        self.units = fold_units(&states, &self.local_host);
        self.sample_history();
    }

    /// Fold this poll's readable telemetry into each unit's rolling sparkline
    /// history (EXPLORER-4). Every recorded point is a value we actually read this
    /// tick (§7 — the trend is observed, never synthesised); a unit that has left
    /// the shelf has its history pruned so a departed unit can't leave a ghost
    /// curve behind.
    fn sample_history(&mut self) {
        let live: HashSet<&str> = self.units.iter().map(|u| u.id.as_str()).collect();
        self.history.retain(|id, _| live.contains(id.as_str()));
        for unit in &self.units {
            let Some(t) = &unit.telemetry else { continue };
            // Only start/extend a trend for a unit that reports a series metric —
            // an all-absent telemetry block leaves the history honestly empty.
            if t.load1.is_none() && t.mem_used_pct.is_none() {
                continue;
            }
            self.history.entry(unit.id.clone()).or_default().record(t);
        }
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

    // ─────────────────── edge chips + jump (EXPLORER-8) ───────────────────

    /// The focused unit's incident edges (EXPLORER-7), grouped into the design's
    /// chip sections (E6) in display order — one section per edge kind present,
    /// each a row of jump chips to the related units. Only edges whose neighbour
    /// resolves to a unit **on the shelf** yield a chip (a subnet/router/pool
    /// endpoint isn't a hero, so it can't be jumped to — omitted, §7); a section
    /// with no such chip is dropped, so no empty header ever shows. Pure over the
    /// folded state — unit-tested without a render.
    fn grouped_edges(&self, focus: &Unit) -> Vec<EdgeSection> {
        let by_id: HashMap<&str, &Unit> = self.units.iter().map(|u| (u.id.as_str(), u)).collect();
        // rank → (header, chips); the BTreeMap keeps the design's section order.
        let mut groups: std::collections::BTreeMap<u8, (String, Vec<ChipItem>)> =
            std::collections::BTreeMap::new();
        // Dedup a neighbour within a section (two edges of one kind to the same
        // unit collapse to one chip).
        let mut seen: HashSet<(u8, String)> = HashSet::new();
        for edge in &self.edges {
            let Some(neighbor_id) = neighbor_of(edge, &focus.id) else {
                continue;
            };
            let Some(neighbor) = by_id.get(neighbor_id).copied() else {
                continue;
            };
            let (rank, header) = section_for(edge, neighbor);
            if !seen.insert((rank, neighbor_id.to_string())) {
                continue;
            }
            groups
                .entry(rank)
                .or_insert_with(|| (header, Vec::new()))
                .1
                .push(ChipItem {
                    id: neighbor.id.clone(),
                    name: neighbor.name.clone(),
                    kind: neighbor.kind,
                });
        }
        groups
            .into_values()
            .map(|(header, mut chips)| {
                chips.sort_by(|a, b| {
                    a.name
                        .to_lowercase()
                        .cmp(&b.name.to_lowercase())
                        .then_with(|| a.id.cmp(&b.id))
                });
                EdgeSection { header, chips }
            })
            .collect()
    }

    /// Jump the hero focus to the unit `id` (a chip click). Reuses the surface's
    /// one focus-set path — the `focus` index into the filtered view — resolving
    /// the neighbour's position; when the active category filter would hide it, the
    /// filter clears so the jump always lands. A stale arm/note from the old focus
    /// is dropped. A no-op if the id has left the shelf.
    fn jump_to_id(&mut self, id: &str) {
        let Some(abs) = self.units.iter().position(|u| u.id == id) else {
            return;
        };
        let cat = self.units[abs].kind.category();
        if self.filter.is_some_and(|f| f != cat) {
            self.filter = None;
        }
        if let Some(pos) = self.filtered_indices().iter().position(|&i| i == abs) {
            self.focus = pos;
            self.arm = None;
            self.last_action_note = None;
        }
    }

    // ─────────────────── the IPAM table mode (EXPLORER-10) ───────────────────

    /// The discovered prefix/IP table for the current view: every /24 an addressed
    /// unit occupies, scoped by the active category filter (the same chips that
    /// scope the hero shelf). Pure over the folded state — the render's data model,
    /// unit-tested without a Bus.
    fn ipam_prefixes(&self) -> Vec<IpamPrefix> {
        derive_prefixes(&self.units, &self.edges)
            .into_iter()
            .filter(|p| self.filter.is_none_or(|c| p.category == c))
            .collect()
    }

    /// A row-click in the IPAM table: return to the hero card and jump its focus to
    /// the occupant unit — reusing the surface's ONE focus-set/jump path
    /// ([`Self::jump_to_id`], which also clears a hiding filter so the jump lands).
    fn jump_from_ipam(&mut self, id: &str) {
        self.mode = SurfaceMode::Hero;
        self.jump_to_id(id);
    }

    // ─────────────────── the per-type action bar (EXPLORER-5) ───────────────────

    /// Arm a destructive verb on `unit_id` — the first click on a destructive
    /// button; the UI then shows the typed-name challenge. Clears any stale note.
    fn arm_verb(&mut self, verb: Verb, unit_id: &str) {
        self.arm = Some(ArmedVerb {
            unit_id: unit_id.to_string(),
            verb,
            echo: String::new(),
        });
        self.last_action_note = None;
    }

    /// Whether the armed verb's typed echo matches `expected` (the unit name) —
    /// the gate the Confirm button enables on (the typed-arming interlock).
    fn arm_ready(&self, expected: &str) -> bool {
        self.arm
            .as_ref()
            .is_some_and(|a| a.echo.trim() == expected && !expected.is_empty())
    }

    /// The confirm path: fire the armed verb IFF the typed echo matches `unit`'s
    /// name (the arming gate — a destructive verb does **nothing** until armed +
    /// echoed). Returns whether it fired. The ONE place the gate lives, shared by
    /// the Confirm button and the tests.
    fn confirm_armed(&mut self, unit: &Unit) -> bool {
        let armed = self
            .arm
            .as_ref()
            .filter(|a| a.unit_id == unit.id)
            .map(|a| a.verb);
        let Some(verb) = armed else {
            return false;
        };
        if !self.arm_ready(&unit.name) {
            return false;
        }
        self.arm = None;
        self.fire(verb, unit);
        true
    }

    /// Dispatch a verb: resolve its real seam and publish it, folding the honest
    /// result into the inline note. A disabled seam never reaches here (the button
    /// is disabled), but stays honest if it does.
    fn fire(&mut self, verb: Verb, unit: &Unit) {
        match verb_seam(verb, unit) {
            Ok(action) => {
                let res = self.dispatch(&action);
                self.last_action_note = Some(match res {
                    Ok(()) => (done_note(verb, unit), false),
                    Err(e) => (
                        format!(
                            "Couldn't {} {} — {e}",
                            verb.label().to_lowercase(),
                            unit.name
                        ),
                        true,
                    ),
                });
            }
            Err(reason) => self.last_action_note = Some((reason, true)),
        }
    }

    /// Publish one resolved seam over the injected sink.
    fn dispatch(&self, action: &HeroAction) -> Result<(), String> {
        match action {
            HeroAction::Cloud { verb, instance } => {
                let body = serde_json::to_string(&InstanceReq {
                    instance: instance.clone(),
                })
                .map_err(|e| e.to_string())?;
                self.action_sink.publish(&cloud_topic(verb), &body)
            }
            HeroAction::Refresh => self.action_sink.publish(UNITS_REQUEST_TOPIC, "{}"),
            HeroAction::Goto { verb, headline } => self
                .action_sink
                .publish(TOAST_TOPIC, &nav_body(&self.local_host, headline, verb)),
        }
    }

    /// The launchpad action bar under the hero card: the focused unit's per-type
    /// verbs, the typed-arming challenge for any armed destructive verb, and the
    /// honest inline result note. Every verb drives a real seam or is honestly
    /// disabled (§7).
    fn action_bar(&mut self, ui: &mut egui::Ui, unit: &Unit) {
        ui.add_space(Style::SP_M);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = Style::SP_S;
            for &verb in verbs_for(unit.kind) {
                self.verb_button(ui, verb, unit);
            }
        });
        // The typed-name challenge when a destructive verb on THIS unit is armed.
        if self.arm.as_ref().is_some_and(|a| a.unit_id == unit.id) {
            self.arm_challenge(ui, unit);
        }
        // The honest inline result note (published, gated, or a write fault).
        if let Some((note, is_err)) = &self.last_action_note {
            ui.add_space(Style::SP_XS);
            ui.label(RichText::new(note).size(Style::SMALL).color(if *is_err {
                Style::DANGER
            } else {
                Style::TEXT_DIM
            }));
        }
    }

    /// The grouped edge-chip region under the card (EXPLORER-8, design E1/E6): the
    /// focused unit's related units, grouped by relationship (Tunnels / Networks /
    /// Volumes / Same subnet / Runs on `<node>` / Storage), each a row of chips
    /// that jump the hero focus to the neighbour. Absent sections are simply not
    /// drawn (no empty header, §7). The whole region is skipped when the unit has
    /// no jumpable edges.
    fn edge_chips(&mut self, ui: &mut egui::Ui, unit: &Unit) {
        let sections = self.grouped_edges(unit);
        if sections.is_empty() {
            return;
        }
        ui.add_space(Style::SP_M);
        let mut jump: Option<String> = None;
        for section in &sections {
            ui.add_space(Style::SP_S);
            ui.label(
                RichText::new(&section.header)
                    .size(Style::SMALL)
                    .strong()
                    .color(Style::TEXT_DIM),
            );
            ui.add_space(Style::SP_XS);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::splat(Style::SP_XS);
                for chip in &section.chips {
                    if edge_chip(ui, chip) {
                        jump = Some(chip.id.clone());
                    }
                }
            });
        }
        if let Some(id) = jump {
            self.jump_to_id(&id);
        }
    }

    /// One verb button: fires immediately when non-destructive, arms when
    /// destructive, or is honestly disabled with its reason on hover (§7).
    fn verb_button(&mut self, ui: &mut egui::Ui, verb: Verb, unit: &Unit) {
        let seam = verb_seam(verb, unit);
        let armed_here = self
            .arm
            .as_ref()
            .is_some_and(|a| a.unit_id == unit.id && a.verb == verb);
        let tint = if verb.destructive() {
            Style::DANGER
        } else {
            Style::TEXT
        };
        let text = RichText::new(verb.label())
            .size(Style::SMALL)
            .color(if seam.is_ok() { tint } else { Style::TEXT_DIM });
        let button = egui::Button::new(text)
            .fill(Style::SURFACE)
            .stroke(Stroke::new(
                1.0,
                if armed_here {
                    Style::DANGER
                } else {
                    Style::BORDER
                },
            ));
        let resp = ui.add_enabled(seam.is_ok(), button);
        match &seam {
            Err(reason) => {
                resp.on_disabled_hover_text(reason.clone());
            }
            Ok(_) => {
                if resp.clicked() {
                    if verb.destructive() {
                        self.arm_verb(verb, &unit.id);
                    } else {
                        self.fire(verb, unit);
                    }
                }
            }
        }
    }

    /// The typed-arming challenge row: type the unit name to enable Confirm (the
    /// `surface_card::show_mok_arm` / `mde-files` typed-echo idiom, reused not
    /// reinvented). Confirm fires through the ONE gate ([`Self::confirm_armed`]).
    fn arm_challenge(&mut self, ui: &mut egui::Ui, unit: &Unit) {
        let Some(verb) = self.arm.as_ref().map(|a| a.verb) else {
            return;
        };
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new(format!(
                "Type \u{201C}{}\u{201D} to arm {}.",
                unit.name,
                verb.label().to_lowercase()
            ))
            .size(Style::SMALL)
            .color(Style::WARN),
        );
        ui.add_space(Style::SP_XS);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = Style::SP_S;
            if let Some(arm) = self.arm.as_mut() {
                ui.add(
                    egui::TextEdit::singleline(&mut arm.echo)
                        .hint_text(unit.name.as_str())
                        .desired_width(Style::SP_XL * 5.0),
                );
            }
            let ready = self.arm_ready(&unit.name);
            let confirm = egui::Button::new(
                RichText::new(format!("Confirm {}", verb.label())).size(Style::SMALL),
            )
            .fill(Style::SURFACE)
            .stroke(Stroke::new(1.0, Style::DANGER));
            if ui.add_enabled(ready, confirm).clicked() {
                self.confirm_armed(unit);
            }
            if ui
                .button(RichText::new("Cancel").size(Style::SMALL))
                .clicked()
            {
                self.arm = None;
            }
        });
    }

    /// Render the surface: the mode toggle + category chips header, then the active
    /// mode (hero card · filmstrip, or the IPAM table). The one public entry the
    /// mount drives per frame.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        // Hero paging keys only steer the hero mode.
        if self.mode == SurfaceMode::Hero {
            self.handle_keys(ui);
        }
        // Keep focus valid against the freshest (possibly re-filtered) view.
        let count = self.hero_count();
        self.focus = if count == 0 {
            0
        } else {
            self.focus.min(count - 1)
        };

        egui::TopBottomPanel::top(ui.id().with("explorer-chips"))
            .frame(egui::Frame::NONE.inner_margin(Style::SP_S))
            .show_inside(ui, |ui| self.header(ui));
        match self.mode {
            SurfaceMode::Hero => {
                egui::TopBottomPanel::bottom(ui.id().with("explorer-strip"))
                    .frame(egui::Frame::NONE.inner_margin(Style::SP_S))
                    .show_inside(ui, |ui| self.filmstrip(ui));
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show_inside(ui, |ui| self.hero(ui));
            }
            SurfaceMode::Ipam => {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.inner_margin(Style::SP_S))
                    .show_inside(ui, |ui| self.ipam_table(ui));
            }
        }
    }

    /// The header: the Hero ⇄ IPAM mode toggle (EXPLORER-10), then the category
    /// filter chips (#8) that scope whichever mode is active.
    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = Style::SP_S;
            if chip(ui, "Hero", self.mode == SurfaceMode::Hero, Style::ACCENT) {
                self.mode = SurfaceMode::Hero;
            }
            if chip(ui, "IPAM", self.mode == SurfaceMode::Ipam, Style::ACCENT) {
                self.mode = SurfaceMode::Ipam;
            }
        });
        ui.add_space(Style::SP_XS);
        self.chips(ui);
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
            if chip(
                ui,
                &format!("All · {total}"),
                self.filter.is_none(),
                Style::ACCENT,
            ) {
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
                {
                    let history = self.history.get(&unit.id);
                    hero_card(&mut child, &unit, false, history);
                }
                // The EXPLORER-5 launchpad: real per-type verbs under the card.
                self.action_bar(&mut child, &unit);
                // The EXPLORER-8 connectivity region: grouped edge chips that jump
                // the hero focus to the unit's related neighbours.
                self.edge_chips(&mut child, &unit);
            }
            None if self.filter.is_none() => {
                // #23 — no mirror yet: show THIS node, discovering.
                hero_card(&mut child, &self_placeholder(&self.local_host), true, None);
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
            ui.painter().line_segment(
                [
                    egui::pos2(c.x - h * 0.5, c.y - h),
                    egui::pos2(c.x + h * 0.5, c.y),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    egui::pos2(c.x + h * 0.5, c.y),
                    egui::pos2(c.x - h * 0.5, c.y + h),
                ],
                stroke,
            );
        } else {
            ui.painter().line_segment(
                [
                    egui::pos2(c.x + h * 0.5, c.y - h),
                    egui::pos2(c.x - h * 0.5, c.y),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    egui::pos2(c.x - h * 0.5, c.y),
                    egui::pos2(c.x + h * 0.5, c.y + h),
                ],
                stroke,
            );
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
        egui::ScrollArea::horizontal().show(ui, |ui| {
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

    /// The IPAM table mode (EXPLORER-10, design E7): a NetBox-style live address
    /// table over the discovered prefixes — each /24 an addressed unit occupies,
    /// its occupants, free/used capacity, and gateway. Rows jump the hero focus to
    /// the occupant. Honest-empty when nothing is addressed yet (§7).
    fn ipam_table(&mut self, ui: &mut egui::Ui) {
        let prefixes = self.ipam_prefixes();
        if prefixes.is_empty() {
            ui.add_space(Style::SP_L);
            let note = self.filter.map_or_else(
                || {
                    "No addressed units discovered yet — the table fills as units \
                     report their addresses."
                        .to_string()
                },
                |cat| format!("No {} prefixes discovered yet.", cat.label()),
            );
            muted_note(ui, note);
            return;
        }
        let mut jump: Option<String> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for prefix in &prefixes {
                    ipam_prefix_header(ui, prefix);
                    ipam_column_header(ui);
                    if prefix.occupants.is_empty() {
                        // A discovered prefix with no occupants stays honestly empty
                        // rather than faking a full table (§7).
                        muted_note(ui, "No occupants discovered in this prefix.");
                    } else {
                        let gw = prefix.gateway();
                        for (row, occ) in prefix.occupants.iter().enumerate() {
                            if ipam_address_row(ui, occ, gw, row % 2 == 0) {
                                jump = Some(occ.unit_id.clone());
                            }
                        }
                    }
                    ui.add_space(Style::SP_M);
                }
            });
        if let Some(id) = jump {
            self.jump_from_ipam(&id);
        }
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
    let text =
        RichText::new(label)
            .size(Style::SMALL)
            .color(if active { Style::BG } else { Style::TEXT });
    let button = egui::Button::new(text)
        .fill(if active { accent } else { Style::SURFACE })
        .stroke(Stroke::new(
            1.0,
            if active { accent } else { Style::BORDER },
        ));
    ui.add(button).clicked()
}

/// One edge jump chip (EXPLORER-8): a mini kind glyph + the neighbour's name in a
/// clickable pill, the border tinted with the neighbour's category accent (the
/// EXPLORER-15 / PICKER category-accent language, §4 tokens — no raw hex). Returns
/// whether it was clicked (the hero-focus jump). Hand-painted (a glyph beside text)
/// rather than an `egui::Button` so the procedural kind glyph rides inside.
fn edge_chip(ui: &mut egui::Ui, chip: &ChipItem) -> bool {
    let accent = chip.kind.category().accent();
    let galley = ui.painter().layout_no_wrap(
        truncate(&chip.name, 18),
        FontId::proportional(Style::SMALL),
        Style::TEXT,
    );
    let glyph = Style::SP_M;
    let pad = Style::SP_S;
    let gap = Style::SP_XS;
    let w = pad + glyph + gap + galley.size().x + pad;
    let h = glyph.max(galley.size().y) + Style::SP_XS * 2.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    let hovered = resp.hovered();
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        Style::RADIUS,
        if hovered {
            Style::SURFACE_HI
        } else {
            Style::SURFACE
        },
    );
    painter.rect_stroke(
        rect,
        Style::RADIUS,
        Stroke::new(1.0, if hovered { accent } else { Style::BORDER }),
        StrokeKind::Inside,
    );
    paint_kind_glyph(
        painter,
        egui::pos2(rect.min.x + pad + glyph * 0.5, rect.center().y),
        glyph * 0.42,
        chip.kind,
        accent,
    );
    let text_h = galley.size().y;
    painter.galley(
        egui::pos2(
            rect.min.x + pad + glyph + gap,
            rect.center().y - text_h * 0.5,
        ),
        galley,
        Style::TEXT,
    );
    resp.on_hover_text(&chip.name).clicked()
}

/// A thin vertical category divider + rotated-free label between filmstrip
/// sections (#8).
fn filmstrip_divider(ui: &mut egui::Ui, cat: Category) {
    ui.vertical(|ui| {
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(cat.label())
                .size(Style::SMALL)
                .color(cat.accent()),
        );
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
            ui.painter()
                .rect_filled(rect, Style::RADIUS, Style::SURFACE);
            ui.painter().rect_stroke(
                rect,
                Style::RADIUS,
                Stroke::new(1.0, border),
                StrokeKind::Inside,
            );
            // Mini glyph.
            let glyph_c = egui::pos2(rect.center().x, rect.min.y + THUMB_H * 0.36);
            paint_kind_glyph(
                ui.painter(),
                glyph_c,
                THUMB_H * 0.2,
                unit.kind,
                cat.accent(),
            );
            // Status dot.
            if let Some(h) = unit.health {
                ui.painter().circle_filled(
                    rect.right_top() + Vec2::new(-Style::SP_S, Style::SP_S),
                    Style::SP_XS * 0.7,
                    h.ring_color(),
                );
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

// ─────────────────── IPAM table render (EXPLORER-10) ───────────────────

/// The flexible occupant-name column width: the row less the fixed address + type
/// columns and the leading indent, floored so a narrow surface still shows a name.
fn ipam_name_col_w(avail: f32) -> f32 {
    (avail - Style::SP_M - IPAM_ADDR_COL - IPAM_TYPE_COL).max(Style::SP_XL * 2.0)
}

/// A rough char budget for a name column of `width` at the body face — keeps a long
/// name inside its cell rather than overrunning the type column.
fn ipam_name_budget(width: f32) -> usize {
    ((width / (Style::BODY * 0.6)) as usize).max(6)
}

/// A dim small-face `RichText` for a table caption / column header.
fn ipam_dim(text: &str) -> RichText {
    RichText::new(text)
        .size(Style::SMALL)
        .color(Style::TEXT_DIM)
}

/// A fixed-width table cell holding one left-aligned label (keeps the columns
/// aligned across every prefix's rows).
fn ipam_cell(ui: &mut egui::Ui, width: f32, text: RichText) {
    ui.allocate_ui_with_layout(
        Vec2::new(width, IPAM_ROW_H),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.label(text);
        },
    );
}

/// The prefix header band (design E7): the CIDR + category badge + discovered
/// tenant-net label on the left; the capacity meter, free/used tally, and gateway
/// on the right. A subtle `SURFACE_HI` band with a category-accent tab.
fn ipam_prefix_header(ui: &mut egui::Ui, p: &IpamPrefix) {
    let accent = p.category.accent();
    // Reserve the band + accent-tab slots so they paint BEHIND the row content.
    let band = ui.painter().add(egui::Shape::Noop);
    let tab = ui.painter().add(egui::Shape::Noop);
    let rect = ui
        .horizontal(|ui| {
            ui.set_min_width(ui.available_width());
            ui.set_min_height(IPAM_ROW_H);
            ui.add_space(Style::SP_S);
            ui.label(
                RichText::new(p.cidr())
                    .monospace()
                    .strong()
                    .color(Style::TEXT),
            );
            ui.label(
                RichText::new(p.category.label())
                    .size(Style::SMALL)
                    .color(accent)
                    .background_color(Style::SURFACE),
            );
            if let Some(label) = &p.label {
                ui.label(ipam_dim(&format!("· {label}")));
            }
            // The right cluster: gateway · free/used · capacity meter.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(Style::SP_S);
                let gw = p.gateway();
                let gw_txt = p
                    .occupants
                    .iter()
                    .find(|o| o.addr == gw)
                    .map_or_else(|| format!("gw {gw}"), |o| format!("gw {gw} · {}", o.name));
                ui.label(
                    RichText::new(gw_txt)
                        .monospace()
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                ui.add_space(Style::SP_M);
                ui.label(ipam_dim(&format!("{} used · {} free", p.used(), p.free())));
                ui.add_space(Style::SP_S);
                used_free_bar(ui, p.used(), accent);
            });
        })
        .response
        .rect;
    ui.painter().set(
        band,
        egui::Shape::rect_filled(rect, Style::RADIUS * 0.5, Style::SURFACE_HI),
    );
    let tab_rect = Rect::from_min_max(rect.min, egui::pos2(rect.min.x + Style::SP_XS, rect.max.y));
    ui.painter()
        .set(tab, egui::Shape::rect_filled(tab_rect, 0.0, accent));
}

/// The slim column-header row under a prefix band (Address · Occupant · Type),
/// aligned to the address rows' fixed columns.
fn ipam_column_header(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let name_w = ipam_name_col_w(ui.available_width());
        ui.add_space(Style::SP_M);
        ipam_cell(ui, IPAM_ADDR_COL, ipam_dim("Address"));
        ipam_cell(ui, name_w, ipam_dim("Occupant"));
        ipam_cell(ui, IPAM_TYPE_COL, ipam_dim("Type"));
    });
}

/// One occupied-address row: the address (mono; the gateway host accent-tinted),
/// the occupant name (a link-toned jump affordance), and its type badge. Zebra
/// banded, hover-highlit, and clickable — a click jumps the hero focus to the
/// occupant. Returns whether it was clicked.
fn ipam_address_row(ui: &mut egui::Ui, occ: &IpamOccupant, gw: Ipv4Addr, zebra: bool) -> bool {
    let accent = occ.kind.category().accent();
    let is_gw = occ.addr == gw;
    // Reserve the zebra band slot so it paints BEHIND the row content.
    let band = ui.painter().add(egui::Shape::Noop);
    let resp = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.set_min_width(ui.available_width());
            let name_w = ipam_name_col_w(ui.available_width());
            ui.add_space(Style::SP_M);
            ipam_cell(
                ui,
                IPAM_ADDR_COL,
                RichText::new(occ.addr.to_string())
                    .monospace()
                    .color(if is_gw { accent } else { Style::TEXT }),
            );
            ipam_cell(
                ui,
                name_w,
                RichText::new(truncate(&occ.name, ipam_name_budget(name_w)))
                    .size(Style::BODY)
                    .color(Style::ACCENT_HI),
            );
            ipam_cell(
                ui,
                IPAM_TYPE_COL,
                RichText::new(occ.kind.label())
                    .size(Style::SMALL)
                    .color(accent),
            );
        })
        .response
        .interact(Sense::click());
    let fill = if resp.hovered() {
        Style::SURFACE_HI
    } else if zebra {
        Style::SURFACE
    } else {
        Style::BG
    };
    ui.painter().set(
        band,
        egui::Shape::rect_filled(resp.rect, Style::RADIUS * 0.5, fill),
    );
    resp.on_hover_text(format!("Jump to {}", occ.name))
        .clicked()
}

/// The prefix capacity meter: a thin bar with the used fraction of the /24 filled
/// in the category accent over a surface track (the honest used/free ratio).
fn used_free_bar(ui: &mut egui::Ui, used: usize, accent: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(IPAM_BAR_W, Style::SP_S), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, Style::RADIUS * 0.5, Style::SURFACE);
    let frac = (used as f32 / IPAM_USABLE_PER_24 as f32).clamp(0.0, 1.0);
    if frac > 0.0 {
        let fill = Rect::from_min_size(rect.min, Vec2::new(rect.width() * frac, rect.height()));
        painter.rect_filled(fill, Style::RADIUS * 0.5, accent);
    }
    painter.rect_stroke(
        rect,
        Style::RADIUS * 0.5,
        Stroke::new(1.0, Style::BORDER),
        StrokeKind::Inside,
    );
}

/// The hero card body (#9/#10/#11/#12): the status ring + type glyph, the
/// name/type/reachability headline, and rich telemetry when reachable else a
/// dimmed-minimal card with explicit unknowns. `discovering` renders the #23
/// self card's "Discovering units…" line; `history` carries the focused unit's
/// rolling sparkline samples (EXPLORER-4, `None` for the placeholder/dimmed path).
fn hero_card(ui: &mut egui::Ui, unit: &Unit, discovering: bool, history: Option<&UnitHistory>) {
    let cat = unit.kind.category();
    let rich = hero_is_rich(unit);
    ui.add_space(Style::SP_L);

    // The status ring + type glyph (#9).
    let side =
        (ui.available_width().min(ui.available_height()) * RING_FRACTION).clamp(RING_MIN, RING_MAX);
    let (ring_rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
    let center = ring_rect.center();
    let radius = side * 0.5 - RING_STROKE_W;
    let time = ui.input(|i| i.time);
    let spinning = paint_status_ring(
        ui.painter(),
        center,
        radius,
        unit.health,
        cat.accent(),
        time,
    );
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
            RichText::new(reachability_line(
                &unit.reachability,
                unit.address.as_deref(),
            ))
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
        hero_telemetry(ui, unit, history);
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

/// The rich telemetry region (#11, EXPLORER-4): the health pill, a peer's mesh
/// facts (role/leader/version), and the **metric grid** — load / mem / net /
/// uptime, load and mem drawing a real sparkline from `history` — or an honest
/// "Live telemetry not yet reported" line when a readable unit has nothing to
/// show yet (§7).
fn hero_telemetry(ui: &mut egui::Ui, unit: &Unit, history: Option<&UnitHistory>) {
    let accent = unit.kind.category().accent();
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

    // The load/mem sparklines draw only from real accumulated samples (§7).
    let load_series = history.map(|h| &h.load1).filter(|s| !s.is_empty());
    let mem_series = history.map(|h| &h.mem_used_pct).filter(|s| !s.is_empty());
    let telemetry = unit.telemetry.clone().unwrap_or_default();
    // Show the grid once there's *anything* real to show — a scalar this tick or
    // an accumulated trend; otherwise the honest "nothing yet" line, not a wall
    // of empty cells.
    if telemetry.any() || load_series.is_some() || mem_series.is_some() {
        ui.add_space(Style::SP_S);
        metric_grid(ui, &telemetry, load_series, mem_series, accent);
    } else {
        muted_note(ui, "Live telemetry not yet reported.");
    }
}

/// The centred load · mem · net · uptime metric grid (EXPLORER-4). A fixed-width
/// row so the surrounding top-down-centre layout centres it cleanly. Each metric
/// is honest field-by-field: a readable value + sparkline where a source exists,
/// a dimmed "no source" cell where none does (net), never a fabricated trend.
fn metric_grid(
    ui: &mut egui::Ui,
    t: &Telemetry,
    load_series: Option<&VecDeque<f32>>,
    mem_series: Option<&VecDeque<f32>>,
    accent: Color32,
) {
    let row_w = SPARK_W * 4.0 + Style::SP_L * 3.0;
    ui.allocate_ui_with_layout(
        Vec2::new(row_w, METRIC_CELL_H),
        Layout::left_to_right(Align::Min),
        |ui| {
            ui.spacing_mut().item_spacing.x = Style::SP_L;
            metric_cell(
                ui,
                "load",
                t.load1.map(|v| format!("{v:.2}")),
                load_series,
                LOAD_REF_CEIL,
                accent,
            );
            metric_cell(
                ui,
                "mem",
                t.mem_used_pct.map(|v| format!("{v:.0}%")),
                mem_series,
                MEM_FULL_SCALE,
                accent,
            );
            // Net has no live source on today's mirror — an honest dimmed cell,
            // not a faked throughput curve (§7). It lights up when the aggregator
            // begins reporting a rate.
            metric_cell(ui, "net", None, None, 0.0, accent);
            // Uptime is a scalar counter, not a trend — show the value with a
            // neutral baseline rather than a meaningless ramp.
            metric_cell(
                ui,
                "uptime",
                t.uptime_s.map(fmt_duration),
                None,
                0.0,
                accent,
            );
        },
    );
}

/// One metric cell: the current value (or a dimmed "—" when unreadable), a
/// sparkline of the real observed `series` when it has ≥2 points, and a caption.
/// The placeholder is honest per case: "collecting…" for a readable metric still
/// filling its trend, a neutral baseline for a scalar-only metric, "no source"
/// where nothing is reported at all (§7).
fn metric_cell(
    ui: &mut egui::Ui,
    caption: &str,
    value: Option<String>,
    series: Option<&VecDeque<f32>>,
    full_scale: f32,
    color: Color32,
) {
    ui.allocate_ui_with_layout(
        Vec2::new(SPARK_W, METRIC_CELL_H),
        Layout::top_down(Align::Center),
        |ui| {
            ui.set_min_width(SPARK_W);
            let has_value = value.is_some();
            match value {
                Some(v) => ui.label(
                    RichText::new(v)
                        .size(Style::BODY)
                        .strong()
                        .color(Style::TEXT),
                ),
                None => ui.label(
                    RichText::new("—")
                        .size(Style::BODY)
                        .strong()
                        .color(Style::TEXT_DIM),
                ),
            };
            match (series, has_value) {
                (Some(s), _) if s.len() >= 2 => sparkline(ui, s, full_scale, color),
                // A readable series metric that hasn't filled two points yet.
                (Some(_), _) => spark_note(ui, "collecting…"),
                // A scalar-only metric (uptime): a neutral baseline, no fake trend.
                (None, true) => spark_baseline(ui),
                // No live source at all (net): honestly dimmed unknown.
                (None, false) => spark_note(ui, "no source"),
            }
            ui.label(
                RichText::new(caption)
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
        },
    );
}

/// Draw a sparkline of `samples` (oldest → newest) scaled to `[0, full_scale]`,
/// the axis expanding to fit any real peak above the reference so a spike is
/// never clipped. Newest reading dotted. Real observed points only — the caller
/// guarantees ≥2 (§7).
fn sparkline(ui: &mut egui::Ui, samples: &VecDeque<f32>, full_scale: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(SPARK_W, SPARK_H), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, Style::RADIUS * 0.5, Style::SURFACE);
    let n = samples.len();
    if n < 2 {
        return;
    }
    // Scale to the metric's reference, but never below a real peak (no clipping).
    let peak = samples
        .iter()
        .copied()
        .fold(full_scale, f32::max)
        .max(f32::EPSILON);
    let pad = SPARK_STROKE_W;
    let plot_h = rect.height() - pad * 2.0;
    let x_at = |i: usize| rect.min.x + rect.width() * (i as f32 / (n - 1) as f32);
    let y_at = |v: f32| rect.max.y - pad - plot_h * (v / peak).clamp(0.0, 1.0);
    let stroke = Stroke::new(SPARK_STROKE_W, color);
    let pts: Vec<egui::Pos2> = samples
        .iter()
        .enumerate()
        .map(|(i, &v)| egui::pos2(x_at(i), y_at(v)))
        .collect();
    for seg in pts.windows(2) {
        painter.line_segment([seg[0], seg[1]], stroke);
    }
    // Emphasise the newest reading with a dot.
    if let Some(&last) = pts.last() {
        painter.circle_filled(last, SPARK_STROKE_W * 1.5, color);
    }
}

/// A dimmed placeholder occupying the sparkline's footprint (keeps the grid rows
/// aligned) with an honest short caption — "collecting…" / "no source" (§7).
fn spark_note(ui: &mut egui::Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(SPARK_W, SPARK_H), Sense::hover());
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );
}

/// A neutral baseline in the sparkline footprint for a scalar-only metric (its
/// value is real, but there is no series to trend — so no fabricated ramp, §7).
fn spark_baseline(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(SPARK_W, SPARK_H), Sense::hover());
    ui.painter().line_segment(
        [
            egui::pos2(rect.min.x, rect.center().y),
            egui::pos2(rect.max.x, rect.center().y),
        ],
        Stroke::new(1.0, Style::BORDER),
    );
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
                edges: Vec::new(),
                history: HashMap::new(),
                focus: 0,
                filter: None,
                last_poll: None,
                action_sink: Box::new(BusActions { bus_root: None }),
                arm: None,
                last_action_note: None,
                mode: SurfaceMode::default(),
            };
            s.refresh();
            s
        }
    }

    /// A recording action sink: captures every (topic, body) the action bar
    /// dispatches so a verb's real seam is asserted headless (no Bus).
    #[derive(Clone, Default)]
    struct FakeActions {
        calls: std::rc::Rc<std::cell::RefCell<Vec<(String, String)>>>,
    }
    impl ActionSink for FakeActions {
        fn publish(&self, topic: &str, body: &str) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push((topic.to_string(), body.to_string()));
            Ok(())
        }
    }

    impl ExplorerState {
        /// Swap in a recording sink and return its shared log — the EXPLORER-5
        /// verb-dispatch test seam.
        fn recording(&mut self) -> FakeActions {
            let fake = FakeActions::default();
            self.action_sink = Box::new(fake.clone());
            fake
        }
    }

    /// The single focused unit of `s`'s current view (the hero the bar acts on).
    fn focused(s: &ExplorerState) -> Unit {
        let idx = s.filtered_indices()[s.focus];
        s.units[idx].clone()
    }

    /// A reachable peer carrying live telemetry — the sparkline-path fixture.
    fn peer_with_telemetry(id: &str, name: &str, t: Telemetry) -> Unit {
        Unit {
            telemetry: Some(t),
            ..unit(id, UnitKind::Peer, name, now_ms())
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
        // the `published_at_ms` / cloud / extras daemon-only fields the shell
        // ignores, and the typed `edges` set (EXPLORER-7) the chips now decode.
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
        // The edge set decodes off the same body (EXPLORER-8).
        assert_eq!(state.edges.len(), 1);
        assert_eq!(state.edges[0].kind, EdgeKind::MeshTunnel);
        assert_eq!(state.edges[0].from, "peer:node-a");
        assert_eq!(state.edges[0].to, "peer:node-b");
        assert_eq!(state.edges[0].detail.as_deref(), Some("direct"));
        // The topic prefix matches the aggregator's `state/units/<node>` shape.
        assert!(super::STATE_PREFIX.starts_with("state/units/"));
    }

    #[test]
    fn every_edge_kind_token_matches_the_worker_wire() {
        // §6 — the shell's `EdgeKind` mirror MUST decode the worker's exact
        // `rename_all = "snake_case"` tokens; a drift here silently drops chips.
        let body = r#"[
            {"kind":"mesh_tunnel","from":"a","to":"b"},
            {"kind":"cloud_attach","from":"a","to":"b"},
            {"kind":"l2_l3_adjacency","from":"a","to":"b"},
            {"kind":"host_placement","from":"a","to":"b"},
            {"kind":"storage_usage","from":"a","to":"b"}
        ]"#;
        let edges: Vec<Edge> = serde_json::from_str(body).expect("all five kinds decode");
        assert_eq!(
            edges.iter().map(|e| e.kind).collect::<Vec<_>>(),
            vec![
                EdgeKind::MeshTunnel,
                EdgeKind::CloudAttach,
                EdgeKind::L2L3Adjacency,
                EdgeKind::HostPlacement,
                EdgeKind::StorageUsage,
            ]
        );
        // A `detail`-less edge decodes with `None` (the worker skips it when empty).
        assert!(edges[0].detail.is_none());
    }

    #[test]
    fn fold_dedups_by_id_keeping_the_freshest_observation() {
        // The same peer appears on two nodes' mirrors; the freshest last_seen wins.
        let a = UnitsState {
            host: "node-a".into(),
            units: vec![unit("peer:x", UnitKind::Peer, "x-old", 100)],
            edges: Vec::new(),
        };
        let b = UnitsState {
            host: "node-b".into(),
            units: vec![unit("peer:x", UnitKind::Peer, "x-new", 200)],
            edges: Vec::new(),
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
            edges: Vec::new(),
        };
        let folded = fold_units(&[state], "me");
        let ids: Vec<&str> = folded.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "peer:me",    // self first (#23)
                "peer:alpha", // then mesh by name
                "peer:zeta",
                "lan:aa",            // then LAN
                "cloud:instance:i1", // then cloud
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
                edges: Vec::new(),
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
                edges: Vec::new(),
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
                edges: Vec::new(),
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
        assert!(!hero_is_rich(&unit(
            "cloud:volume:v",
            UnitKind::Volume,
            "v",
            10
        )));
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
            edges: Vec::new(),
        }];

        for filter in [
            None,
            Some(Category::Mesh),
            Some(Category::Lan),
            Some(Category::Cloud),
        ] {
            let mut s = ExplorerState::with_fake(states.clone(), "me");
            s.set_filter(filter);
            let ctx = egui::Context::default();
            Style::install(&ctx);
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    Vec2::new(1200.0, 800.0),
                )),
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
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| empty.show(ui));
        });
        assert!(!ctx.tessellate(out.shapes, out.pixels_per_point).is_empty());
    }

    #[test]
    fn telemetry_history_accumulates_bounded_real_samples() {
        // A reachable peer that reports load/mem: repeated polls build a REAL
        // observed series (every point a value we read, never synthesised, §7),
        // ring-bounded to the history cap.
        let peer = peer_with_telemetry(
            "peer:me",
            "me",
            Telemetry {
                load1: Some(0.5),
                mem_used_pct: Some(40.0),
                uptime_s: Some(120),
            },
        );
        let states = vec![UnitsState {
            host: "me".into(),
            units: vec![peer],
            edges: Vec::new(),
        }];
        let mut s = ExplorerState::with_fake(states, "me"); // one refresh already
        for _ in 0..(HISTORY_LEN + 5) {
            s.refresh();
        }
        let h = s.history.get("peer:me").expect("peer accrued history");
        assert_eq!(h.load1.len(), HISTORY_LEN, "series ring-bounded to the cap");
        assert_eq!(h.mem_used_pct.len(), HISTORY_LEN);
        assert!(
            h.load1.iter().all(|&v| (v - 0.5).abs() < f32::EPSILON),
            "each point is the real observed value, not a faked curve"
        );
    }

    #[test]
    fn history_prunes_departed_units() {
        // A unit that leaves the shelf drops its stale history — no ghost curve.
        let present = vec![UnitsState {
            host: "me".into(),
            units: vec![peer_with_telemetry(
                "peer:gone",
                "gone",
                Telemetry {
                    load1: Some(1.0),
                    ..Default::default()
                },
            )],
            edges: Vec::new(),
        }];
        let mut s = ExplorerState::with_fake(present, "me");
        assert!(s.history.contains_key("peer:gone"));
        // The next read returns an empty shelf → the unit departs.
        s.client = Box::new(FakeUnits(vec![]));
        s.refresh();
        assert!(
            !s.history.contains_key("peer:gone"),
            "stale history pruned when the unit leaves"
        );
    }

    #[test]
    fn a_unit_without_a_series_metric_records_no_history() {
        // Telemetry with only a scalar counter (uptime) and no load/mem must NOT
        // start a trend — the sparkline source stays honestly empty (§7).
        let peer = peer_with_telemetry(
            "peer:me",
            "me",
            Telemetry {
                load1: None,
                mem_used_pct: None,
                uptime_s: Some(999),
            },
        );
        let s = ExplorerState::with_fake(
            vec![UnitsState {
                host: "me".into(),
                units: vec![peer],
                edges: Vec::new(),
            }],
            "me",
        );
        assert!(
            s.history.get("peer:me").is_none(),
            "no load/mem → no sparkline history minted"
        );
    }

    #[test]
    fn hero_card_renders_sparklines_when_reachable_else_dimmed() {
        // A reachable peer with telemetry, polled enough to fill a real sparkline,
        // renders the rich metric grid; an off-mesh LAN host renders the
        // dimmed-minimal card with no telemetry grid (#11/#12).
        let peer = Unit {
            mesh: Some(MeshFacts {
                role: Some("workstation".into()),
                leader: false,
                mde_version: Some("12.0.0".into()),
            }),
            ..peer_with_telemetry(
                "peer:me",
                "me",
                Telemetry {
                    load1: Some(0.8),
                    mem_used_pct: Some(55.0),
                    uptime_s: Some(90_061),
                },
            )
        };
        let states = vec![UnitsState {
            host: "me".into(),
            units: vec![peer, unit("lan:aa", UnitKind::LanHost, "printer", now_ms())],
            edges: Vec::new(),
        }];
        let mut s = ExplorerState::with_fake(states, "me");
        for _ in 0..4 {
            s.refresh(); // ≥2 samples → a drawable sparkline
        }
        assert!(
            s.history
                .get("peer:me")
                .is_some_and(|h| h.load1.len() >= 2 && h.mem_used_pct.len() >= 2),
            "the sparkline has ≥2 real points to draw"
        );

        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                Vec2::new(1200.0, 800.0),
            )),
            ..Default::default()
        };
        // Reachable peer focused → the sparkline / metric-grid path.
        s.focus = 0;
        let out = ctx.run(input.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| s.show(ui));
        });
        assert!(
            !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty(),
            "the rich sparkline card drew primitives"
        );
        // Dimmed LAN host focused → the dimmed-minimal path (no metric grid).
        s.focus = 1;
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| s.show(ui));
        });
        assert!(
            !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty(),
            "the dimmed-minimal card drew primitives"
        );
    }

    // ─────────────────────── EXPLORER-5 action bars ───────────────────────

    /// A cloud instance with a known Nova id + address (the console-enabled path).
    fn instance_unit(id: &str, name: &str) -> Unit {
        Unit {
            address: Some("10.0.0.5".to_string()),
            ..unit(id, UnitKind::Instance, name, now_ms())
        }
    }

    #[test]
    fn topic_and_id_mirrors_match_the_worker_contract() {
        // §6 — the shell's local mirrors must equal the openstack worker's wire.
        assert_eq!(CLOUD_ACTION_PREFIX, "action/cloud/");
        assert_eq!(UNITS_REQUEST_TOPIC, "action/units/get-stream");
        assert_eq!(cloud_topic("instance-stop"), "action/cloud/instance-stop");
        // The Nova id is the aggregator's `cloud:<kind>:<object-id>` tail.
        assert_eq!(
            cloud_object_id(&unit("cloud:instance:uuid-1", UnitKind::Instance, "web", 1)),
            "uuid-1"
        );
        // An object id that itself contains a colon keeps its remainder.
        assert_eq!(
            cloud_object_id(&unit("cloud:volume:pool:vol-9", UnitKind::Volume, "v", 1)),
            "pool:vol-9"
        );
    }

    #[test]
    fn instance_lifecycle_verbs_dispatch_over_the_cloud_bus() {
        let mut s = ExplorerState::with_fake(
            vec![UnitsState {
                host: "me".into(),
                units: vec![instance_unit("cloud:instance:i-9", "web")],
                edges: Vec::new(),
            }],
            "me",
        );
        let fake = s.recording();
        let u = instance_unit("cloud:instance:i-9", "web");

        // Start is non-destructive → fires immediately.
        s.fire(Verb::Start, &u);
        assert_eq!(
            fake.calls.borrow().as_slice(),
            &[(
                "action/cloud/instance-start".to_string(),
                r#"{"instance":"i-9"}"#.to_string()
            )],
            "Start publishes the QC-11 InstanceReq on action/cloud/instance-start"
        );

        // The three destructive verbs each publish their own topic once armed.
        for (verb, topic) in [
            (Verb::Stop, "action/cloud/instance-stop"),
            (Verb::Reboot, "action/cloud/instance-reboot"),
            (Verb::Delete, "action/cloud/instance-delete"),
        ] {
            fake.calls.borrow_mut().clear();
            s.arm_verb(verb, &u.id);
            s.arm.as_mut().expect("armed").echo = "web".to_string();
            assert!(s.confirm_armed(&u), "the typed-name confirm fires the verb");
            assert_eq!(
                fake.calls.borrow().as_slice(),
                &[(topic.to_string(), r#"{"instance":"i-9"}"#.to_string())],
                "{verb:?} publishes on {topic}"
            );
            assert!(s.arm.is_none(), "arming clears after the confirm");
        }
    }

    #[test]
    fn arming_gates_the_destructive_verbs() {
        let mut s = ExplorerState::with_fake(
            vec![UnitsState {
                host: "me".into(),
                units: vec![instance_unit("cloud:instance:i-9", "web")],
                edges: Vec::new(),
            }],
            "me",
        );
        let fake = s.recording();
        let u = instance_unit("cloud:instance:i-9", "web");

        // Arm Delete but leave the echo blank / wrong → nothing dispatches.
        s.arm_verb(Verb::Delete, &u.id);
        assert!(
            !s.confirm_armed(&u),
            "an un-echoed destructive verb is a no-op"
        );
        s.arm.as_mut().expect("armed").echo = "wrong".to_string();
        assert!(!s.confirm_armed(&u), "a mismatched echo never fires");
        assert!(
            fake.calls.borrow().is_empty(),
            "a destructive verb publishes NOTHING until armed + echoed"
        );

        // The exact name arms it.
        s.arm.as_mut().expect("armed").echo = "web".to_string();
        assert!(s.arm_ready("web"));
        assert!(s.confirm_armed(&u));
        assert_eq!(
            fake.calls.borrow().len(),
            1,
            "now it dispatches exactly once"
        );
    }

    #[test]
    fn peer_verbs_reach_the_fleet_and_the_live_stream() {
        let mut s = ExplorerState::with_fake(
            vec![UnitsState {
                host: "me".into(),
                units: vec![unit("peer:zeta", UnitKind::Peer, "zeta", now_ms())],
                edges: Vec::new(),
            }],
            "me",
        );
        let fake = s.recording();
        let peer = unit("peer:zeta", UnitKind::Peer, "zeta", now_ms());

        // Open in Fleet → a nav chyron carrying shell/goto/mesh.
        s.fire(Verb::OpenInFleet, &peer);
        {
            let calls = fake.calls.borrow();
            assert_eq!(calls[0].0, TOAST_TOPIC);
            assert!(
                calls[0].1.contains("shell/goto/mesh"),
                "open-in-Fleet routes to the mesh view: {}",
                calls[0].1
            );
        }

        // Health-check → the aggregator's get-stream refresh.
        fake.calls.borrow_mut().clear();
        s.fire(Verb::HealthCheck, &peer);
        assert_eq!(fake.calls.borrow()[0].0, "action/units/get-stream");

        // Evict has no bus verb → honestly disabled, never a dispatch.
        assert!(verb_seam(Verb::Evict, &peer).is_err());
    }

    #[test]
    fn lan_invite_is_armed_and_routes_to_provisioning() {
        let mut s = ExplorerState::with_fake(
            vec![UnitsState {
                host: "me".into(),
                units: vec![unit("lan:printer", UnitKind::LanHost, "printer", now_ms())],
                edges: Vec::new(),
            }],
            "me",
        );
        let fake = s.recording();
        let host = unit("lan:printer", UnitKind::LanHost, "printer", now_ms());

        // Invite is destructive (trust change) → gated on the typed name.
        s.arm_verb(Verb::Invite, &host.id);
        assert!(!s.confirm_armed(&host), "invite is a no-op until echoed");
        assert!(fake.calls.borrow().is_empty());
        s.arm.as_mut().expect("armed").echo = "printer".to_string();
        assert!(s.confirm_armed(&host));
        let calls = fake.calls.borrow();
        assert_eq!(calls[0].0, TOAST_TOPIC);
        assert!(
            calls[0].1.contains("shell/plane/provisioning"),
            "invite kicks the Provisioning pairing flow: {}",
            calls[0].1
        );
    }

    #[test]
    fn verbs_without_a_seam_are_honestly_disabled() {
        // Console with no address, object delete, and evict all resolve to a
        // reason, never a live no-op button (§7).
        let bare_instance = unit("cloud:instance:i", UnitKind::Instance, "web", 1);
        assert!(verb_seam(Verb::Console, &bare_instance).is_err());
        assert!(verb_seam(Verb::Console, &instance_unit("cloud:instance:i", "web")).is_ok());
        assert!(verb_seam(
            Verb::ObjectDelete,
            &unit("cloud:volume:v", UnitKind::Volume, "vol", 1)
        )
        .is_err());
        assert!(verb_seam(Verb::Evict, &unit("peer:x", UnitKind::Peer, "x", 1)).is_err());
        // Inspect routes to the Cloud surface (a real hand-off).
        assert!(verb_seam(
            Verb::Inspect,
            &unit("cloud:network:n", UnitKind::Network, "net", 1)
        )
        .is_ok());
    }

    #[test]
    fn each_kind_offers_its_own_verbs() {
        assert_eq!(verbs_for(UnitKind::Instance).len(), 5);
        assert_eq!(
            verbs_for(UnitKind::Volume),
            [Verb::Inspect, Verb::ObjectDelete].as_slice()
        );
        assert_eq!(
            verbs_for(UnitKind::Peer),
            [Verb::OpenInFleet, Verb::HealthCheck, Verb::Evict].as_slice()
        );
        assert_eq!(
            verbs_for(UnitKind::LanHost),
            [Verb::Invite, Verb::HealthCheck].as_slice()
        );
    }

    #[test]
    fn the_armed_action_bar_renders_headless() {
        // The hero + action bar + typed-arming challenge all tessellate cleanly.
        let mut s = ExplorerState::with_fake(
            vec![UnitsState {
                host: "me".into(),
                units: vec![instance_unit("cloud:instance:i-9", "web")],
                edges: Vec::new(),
            }],
            "me",
        );
        let u = focused(&s);
        s.arm_verb(Verb::Delete, &u.id); // show the challenge row
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                Vec2::new(1200.0, 800.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| s.show(ui));
        });
        assert!(!ctx.tessellate(out.shapes, out.pixels_per_point).is_empty());
    }

    // ─────────────────────── EXPLORER-8 edge chips ───────────────────────

    /// A typed edge between two ids (test fixtures build them directly, no wire).
    fn edge(kind: EdgeKind, from: &str, to: &str) -> Edge {
        Edge {
            kind,
            from: from.to_string(),
            to: to.to_string(),
            detail: None,
        }
    }

    /// A connectivity fixture: self + a peer, an instance wired to a network +
    /// volume, running on the peer, the volume attached (+ backed by a non-unit
    /// pool). One state so `fold_edges` + `grouped_edges` see the whole graph.
    fn connected_state() -> Vec<UnitsState> {
        vec![UnitsState {
            host: "me".into(),
            units: vec![
                unit("peer:me", UnitKind::Peer, "me", 10),
                unit("peer:anvil", UnitKind::Peer, "anvil", 10),
                unit("cloud:instance:i1", UnitKind::Instance, "web", 10),
                unit("cloud:network:n1", UnitKind::Network, "tenant", 10),
                unit("cloud:volume:v1", UnitKind::Volume, "data", 10),
            ],
            edges: vec![
                edge(EdgeKind::MeshTunnel, "peer:me", "peer:anvil"),
                edge(
                    EdgeKind::CloudAttach,
                    "cloud:instance:i1",
                    "cloud:network:n1",
                ),
                edge(
                    EdgeKind::CloudAttach,
                    "cloud:instance:i1",
                    "cloud:volume:v1",
                ),
                edge(EdgeKind::HostPlacement, "cloud:instance:i1", "peer:anvil"),
                edge(
                    EdgeKind::StorageUsage,
                    "cloud:volume:v1",
                    "cloud:instance:i1",
                ),
                // Backing pool: a non-unit endpoint — never a jump chip (§7).
                edge(EdgeKind::StorageUsage, "cloud:volume:v1", "pool:ceph"),
            ],
        }]
    }

    /// Focus the hero on the unit `id` (its position in the current filtered view).
    fn focus_on(s: &mut ExplorerState, id: &str) {
        let abs = s
            .units
            .iter()
            .position(|u| u.id == id)
            .expect("unit present");
        s.focus = s
            .filtered_indices()
            .iter()
            .position(|&i| i == abs)
            .expect("unit is in the active view");
    }

    #[test]
    fn edges_fold_and_dedup_across_node_mirrors() {
        // Two nodes republish the same derived edge — the union keeps one.
        let states = vec![
            UnitsState {
                host: "a".into(),
                units: vec![],
                edges: vec![edge(EdgeKind::MeshTunnel, "peer:a", "peer:b")],
            },
            UnitsState {
                host: "b".into(),
                units: vec![],
                edges: vec![
                    edge(EdgeKind::MeshTunnel, "peer:a", "peer:b"), // dup
                    edge(EdgeKind::MeshTunnel, "peer:b", "peer:c"), // new
                ],
            },
        ];
        assert_eq!(
            fold_edges(&states).len(),
            2,
            "cross-node duplicate collapses"
        );
    }

    #[test]
    fn edge_chips_group_by_kind_and_omit_absent_sections() {
        let s = ExplorerState::with_fake(connected_state(), "me");
        let instance = s
            .units
            .iter()
            .find(|u| u.id == "cloud:instance:i1")
            .cloned()
            .expect("instance folded");

        let sections = s.grouped_edges(&instance);
        // Design order: Networks, Volumes, Runs on <node>, Storage. Tunnels + Same
        // subnet are absent from an instance's view → no empty headers (§7).
        let headers: Vec<&str> = sections.iter().map(|sec| sec.header.as_str()).collect();
        assert_eq!(
            headers,
            vec!["Networks", "Volumes", "Runs on anvil", "Storage"]
        );
        // Each chip is the related unit (name + kind), jumpable.
        let chip_of = |header: &str| -> Vec<&str> {
            sections
                .iter()
                .find(|sec| sec.header == header)
                .map(|sec| sec.chips.iter().map(|c| c.name.as_str()).collect())
                .unwrap_or_default()
        };
        assert_eq!(chip_of("Networks"), vec!["tenant"]);
        assert_eq!(chip_of("Volumes"), vec!["data"]);
        assert_eq!(chip_of("Runs on anvil"), vec!["anvil"]);
        // Storage shows the attached volume; the non-unit backing pool is skipped.
        assert_eq!(chip_of("Storage"), vec!["data"]);
        assert!(
            sections
                .iter()
                .all(|sec| sec.chips.iter().all(|c| c.id != "pool:ceph")),
            "a non-unit pool endpoint never becomes a chip"
        );

        // A peer's view has only the mesh tunnel — every cloud section is absent.
        let me = s.units.iter().find(|u| u.id == "peer:me").cloned().unwrap();
        let peer_sections = s.grouped_edges(&me);
        assert_eq!(
            peer_sections
                .iter()
                .map(|x| x.header.as_str())
                .collect::<Vec<_>>(),
            vec!["Tunnels"]
        );
        assert_eq!(peer_sections[0].chips[0].id, "peer:anvil");
    }

    #[test]
    fn a_unit_with_only_a_non_unit_endpoint_shows_no_section() {
        // A volume backed solely by a pool (no attachment, no unit neighbour) has
        // nothing jumpable → the whole edge region is empty, not an empty header.
        let s = ExplorerState::with_fake(
            vec![UnitsState {
                host: "me".into(),
                units: vec![unit("cloud:volume:v9", UnitKind::Volume, "lonely", 10)],
                edges: vec![edge(EdgeKind::StorageUsage, "cloud:volume:v9", "pool:ceph")],
            }],
            "me",
        );
        let vol = s
            .units
            .iter()
            .find(|u| u.id == "cloud:volume:v9")
            .cloned()
            .unwrap();
        assert!(s.grouped_edges(&vol).is_empty());
    }

    #[test]
    fn a_chip_click_jumps_the_hero_focus_to_the_neighbour() {
        let mut s = ExplorerState::with_fake(connected_state(), "me");
        focus_on(&mut s, "cloud:instance:i1");
        // The Networks chip points at the tenant network.
        let sections = s.grouped_edges(&focused(&s));
        let net_chip = sections
            .iter()
            .find(|sec| sec.header == "Networks")
            .and_then(|sec| sec.chips.first())
            .expect("a network chip")
            .clone();
        assert_eq!(net_chip.id, "cloud:network:n1");

        // Clicking it (the jump path) moves the hero focus to that neighbour.
        s.jump_to_id(&net_chip.id);
        assert_eq!(focused(&s).id, "cloud:network:n1");
    }

    #[test]
    fn a_jump_to_a_filtered_out_neighbour_clears_the_filter() {
        // Focused on a cloud instance under the Cloud filter, jumping to its host
        // peer (a Mesh unit hidden by the filter) clears the filter so the jump
        // always lands — reusing the one focus-set path.
        let mut s = ExplorerState::with_fake(connected_state(), "me");
        s.set_filter(Some(Category::Cloud));
        focus_on(&mut s, "cloud:instance:i1");
        s.jump_to_id("peer:anvil");
        assert_eq!(
            s.filter, None,
            "the hiding filter clears on a cross-filter jump"
        );
        assert_eq!(focused(&s).id, "peer:anvil");
    }

    #[test]
    fn the_edge_chip_region_renders_headless() {
        // The grouped chips tessellate cleanly under the hero card.
        let mut s = ExplorerState::with_fake(connected_state(), "me");
        focus_on(&mut s, "cloud:instance:i1");
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                Vec2::new(1200.0, 900.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| s.show(ui));
        });
        assert!(!ctx.tessellate(out.shapes, out.pixels_per_point).is_empty());
    }

    // ─────────────────────── EXPLORER-10 IPAM table ───────────────────────

    /// A unit that reported an address — the IPAM-occupant fixture.
    fn addr_unit(id: &str, kind: UnitKind, name: &str, addr: &str) -> Unit {
        Unit {
            address: Some(addr.to_string()),
            ..unit(id, kind, name, now_ms())
        }
    }

    /// A live-discovered shelf spanning a mesh /24, a LAN /24, and a cloud tenant
    /// /24 (named by a `CloudAttach` edge), plus an address-less network + volume
    /// that must never become occupants.
    fn addressed_state() -> Vec<UnitsState> {
        vec![UnitsState {
            host: "me".into(),
            units: vec![
                addr_unit("peer:me", UnitKind::Peer, "me", "10.42.0.1"),
                addr_unit("peer:anvil", UnitKind::Peer, "anvil", "10.42.0.7"),
                addr_unit("lan:printer", UnitKind::LanHost, "printer", "172.20.0.50"),
                addr_unit("cloud:instance:i1", UnitKind::Instance, "web", "10.0.0.5"),
                addr_unit("cloud:instance:i2", UnitKind::Instance, "db", "10.0.0.9"),
                unit("cloud:network:n1", UnitKind::Network, "tenant", 10),
                unit("cloud:volume:v1", UnitKind::Volume, "data", 10),
            ],
            edges: vec![
                edge(
                    EdgeKind::CloudAttach,
                    "cloud:instance:i1",
                    "cloud:network:n1",
                ),
                edge(
                    EdgeKind::CloudAttach,
                    "cloud:instance:i2",
                    "cloud:network:n1",
                ),
            ],
        }]
    }

    #[test]
    fn ipam_aggregates_addresses_into_slash24_prefixes() {
        let s = ExplorerState::with_fake(addressed_state(), "me");
        let prefixes = s.ipam_prefixes();
        // Three /24s, proximity-ordered (mesh → LAN → cloud) then by network.
        let cidrs: Vec<String> = prefixes.iter().map(IpamPrefix::cidr).collect();
        assert_eq!(cidrs, vec!["10.42.0.0/24", "172.20.0.0/24", "10.0.0.0/24"]);

        // The mesh prefix: two peers sorted by address, gateway is the .1 host.
        let mesh = &prefixes[0];
        assert_eq!(mesh.category, Category::Mesh);
        assert_eq!(mesh.gateway(), "10.42.0.1".parse::<Ipv4Addr>().unwrap());
        let names: Vec<&str> = mesh.occupants.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["me", "anvil"],
            "occupants sorted by address (.1, .7)"
        );

        // The cloud prefix reads as Cloud; the address-less volume/network are
        // never phantom occupants (§7).
        let cloud = &prefixes[2];
        assert_eq!(cloud.category, Category::Cloud);
        assert_eq!(cloud.occupants.len(), 2);
        assert!(cloud
            .occupants
            .iter()
            .all(|o| o.unit_id != "cloud:volume:v1" && o.unit_id != "cloud:network:n1"));
    }

    #[test]
    fn ipam_occupancy_counts_used_and_free_over_the_slash24() {
        let s = ExplorerState::with_fake(addressed_state(), "me");
        let prefixes = s.ipam_prefixes();
        let mesh = &prefixes[0];
        assert_eq!(mesh.used(), 2);
        assert_eq!(mesh.free(), IPAM_USABLE_PER_24 - 2);
        let lan = &prefixes[1];
        assert_eq!(lan.used(), 1);
        assert_eq!(lan.free(), 253);
    }

    #[test]
    fn ipam_labels_a_tenant_prefix_from_a_cloud_attach_edge() {
        let s = ExplorerState::with_fake(addressed_state(), "me");
        let prefixes = s.ipam_prefixes();
        let cloud = prefixes
            .iter()
            .find(|p| p.category == Category::Cloud)
            .expect("a cloud prefix");
        assert_eq!(
            cloud.label.as_deref(),
            Some("tenant"),
            "the tenant net names its prefix via the CloudAttach edge (EXPLORER-7)"
        );
        // Mesh/LAN prefixes have no network object → no fabricated label (§7).
        assert!(prefixes[0].label.is_none());
        assert!(prefixes[1].label.is_none());
    }

    #[test]
    fn ipam_ignores_absent_and_unparseable_addresses() {
        // Parse tolerances: a CIDR mask + a :port tail both resolve; junk doesn't.
        assert_eq!(parse_ipv4("10.0.0.5/24"), "10.0.0.5".parse().ok());
        assert_eq!(parse_ipv4("10.0.0.5:5900"), "10.0.0.5".parse().ok());
        assert!(parse_ipv4("not-an-ip").is_none());
        assert!(
            parse_ipv4("fe80::1").is_none(),
            "IPv6 isn't a /24 occupant here"
        );

        // A unit with no address, and an IPv6 unit, yield no phantom prefixes.
        let units = vec![
            unit("peer:me", UnitKind::Peer, "me", 10),
            addr_unit("peer:v6", UnitKind::Peer, "v6", "fe80::1"),
            addr_unit("peer:ok", UnitKind::Peer, "ok", "10.42.0.3"),
        ];
        let prefixes = derive_prefixes(&units, &[]);
        assert_eq!(prefixes.len(), 1, "only the IPv4 unit anchors a prefix");
        assert_eq!(prefixes[0].occupants.len(), 1);
        // A wholly empty shelf → no prefixes at all (honest-empty, §7).
        assert!(derive_prefixes(&[], &[]).is_empty());
    }

    #[test]
    fn ipam_filter_scopes_prefixes_by_category() {
        let mut s = ExplorerState::with_fake(addressed_state(), "me");
        s.set_filter(Some(Category::Cloud));
        let cloud = s.ipam_prefixes();
        assert_eq!(cloud.len(), 1);
        assert_eq!(cloud[0].category, Category::Cloud);
        s.set_filter(Some(Category::Lan));
        assert_eq!(s.ipam_prefixes().len(), 1);
        s.set_filter(None);
        assert_eq!(s.ipam_prefixes().len(), 3);
    }

    #[test]
    fn ipam_row_click_jumps_to_the_occupant_hero() {
        let mut s = ExplorerState::with_fake(addressed_state(), "me");
        s.mode = SurfaceMode::Ipam;
        // A row click returns to the hero card, focused on the occupant.
        s.jump_from_ipam("lan:printer");
        assert_eq!(s.mode, SurfaceMode::Hero);
        assert_eq!(focused(&s).id, "lan:printer");

        // A jump from under a hiding category filter clears it so the jump lands.
        s.mode = SurfaceMode::Ipam;
        s.set_filter(Some(Category::Cloud));
        s.jump_from_ipam("peer:me");
        assert_eq!(s.mode, SurfaceMode::Hero);
        assert_eq!(s.filter, None, "the hiding filter clears on the jump");
        assert_eq!(focused(&s).id, "peer:me");
    }

    #[test]
    fn ipam_table_renders_headless_and_when_empty() {
        let mut render = |s: &mut ExplorerState| {
            let ctx = egui::Context::default();
            Style::install(&ctx);
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    Vec2::new(1200.0, 800.0),
                )),
                ..Default::default()
            };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| s.show(ui));
            });
            !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty()
        };
        // The populated IPAM table draws its prefix bands + address rows.
        let mut s = ExplorerState::with_fake(addressed_state(), "me");
        s.mode = SurfaceMode::Ipam;
        assert!(render(&mut s), "the IPAM table drew primitives");
        // Honest-empty (no addressed units) still draws the note, never panics.
        let mut empty = ExplorerState::with_fake(vec![], "solo");
        empty.mode = SurfaceMode::Ipam;
        assert!(render(&mut empty));
    }
}
