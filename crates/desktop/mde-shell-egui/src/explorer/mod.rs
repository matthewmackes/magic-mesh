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
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use crate::bus_reader::BusReader;
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

/// EXPLORER-12 — how long the surface must sit without ANY input before the
/// ambient idle auto-cycle begins (a living-wall display only comes alive after a
/// clear pause). A behaviour cadence in the same `Duration` register as
/// [`REFRESH`] — deliberately NOT a `Motion` easing token (§4 governs the *visual*
/// transition, which the hero's `Motion::BASE` page-slide still owns).
const AMBIENT_IDLE: Duration = Duration::from_secs(30);

/// EXPLORER-12 — the deliberately slow dwell between ambient auto-advances once
/// the cycle is running (a calm wall-clock crawl, never a flicker). A behaviour
/// cadence like [`AMBIENT_IDLE`]; each advance's motion still eases through the
/// `Motion` table via the hero paging animation, so §4 stays honoured.
const AMBIENT_DWELL: Duration = Duration::from_secs(10);

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

// ── Mosaic overview mode (EXPLORER-11, design O1/O3/O6) ──
/// One mosaic hero-tile's width — wide enough for a mini glyph plus a truncated
/// name (a §4-grid behaviour param, not a scattered px).
const MOSAIC_TILE_W: f32 = Style::SP_XL * 4.5;
/// One mosaic hero-tile's height (the mini status-ring well + the name + badge).
const MOSAIC_TILE_H: f32 = Style::SP_XL * 3.75;
/// The gap between mosaic tiles — rows AND columns share it, so `mosaic_columns`
/// and the row layout agree on the grid step (D-pad nav stays true, O6).
const MOSAIC_GAP: f32 = Style::SP_M;
/// The mini status-ring diameter inside a mosaic tile — echoes the hero ring at
/// tile scale (O1 "mini hero tiles").
const MOSAIC_RING_D: f32 = Style::SP_XL * 1.5;
/// The keyboard/D-pad focus-ring stroke width — a deliberately thick, high
/// contrast ring so the selection is always legible for couch nav
/// (EXPLORER-18, O11). Painted only through the ONE shared [`focus_ring`]
/// helper so every navigable element wears the identical ring.
const FOCUS_RING_W: f32 = 2.5;
/// The tile→hero zoom-in duration — the O3 shared-element reveal, pinned to the
/// §4 Motion table's deliberate step (never a literal duration).
const ZOOM_SECS: f32 = Motion::SLOW;
/// The zoom/settle opacity floor: the hero starts this faint as it grows from the
/// tile, and the mosaic settles back in from here on Back (O3).
const ZOOM_FADE_FLOOR: f32 = 0.4;

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

    /// The type words the `/` universal search matches for this kind (EXPLORER-14,
    /// O7 — typing "nova" lands on instances): the badge label plus the design's
    /// own taxonomy names (lock #4 — Nova instances / Cinder volumes / Glance
    /// images / Neutron networks), so the operator's `OpenStack` vocabulary finds
    /// the right units without a synonym table to maintain.
    const fn search_terms(self) -> &'static str {
        match self {
            Self::Peer => "peer mesh",
            Self::LanHost => "lan host",
            Self::Instance => "instance nova server",
            Self::Volume => "volume cinder",
            Self::Image => "image glance",
            Self::Network => "network neutron",
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

/// The E5 enrichment block folded onto a unit — a **local** mirror of the
/// aggregator's `Extras`, decoding exactly the fields the `/` universal search
/// matches (EXPLORER-14, O7): the rDNS/mDNS name, the offline OUI vendor, the
/// service fingerprint labels, and the open key/value tail (`open_ports` /
/// `type_guess` / …). Every field honestly absent when unprobed (§7).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(default)]
struct UnitExtras {
    /// Reverse-DNS / mDNS name (E5).
    rdns: Option<String>,
    /// MAC OUI vendor from the offline table (E5).
    oui_vendor: Option<String>,
    /// Service/port fingerprint labels (`"ssh, vnc"`).
    fingerprint: Option<String>,
    /// The open discovered key/values (the worker's `open_ports`, `type_guess`,
    /// `actions`, … tail).
    extra: BTreeMap<String, String>,
}

/// One discovered unit — a **local** mirror of the aggregator's `Unit`, carrying
/// exactly the fields the hero card renders + the `/` search matches. Serde
/// ignores the remaining daemon-only fields (the E4 cloud detail sheet), so this
/// decodes the same body without linking the daemon crate (§6).
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
    /// The E5 enrichment block (the search's service/MAC-vendor/rDNS fields).
    #[serde(default)]
    extras: UnitExtras,
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
/// status ring.
///
/// **Category identity (EXPLORER-15, design O8).** The accents ARE the shared
/// categorical palette `mde_egui::Style` defines ONCE for the picker groups and
/// the explorer categories (PICKER-2 — Mesh keeps its own `ACCENT_MESH` green,
/// LAN speaks the terminal teal, Cloud the workloads purple): one colour
/// language, no duplicate tokens minted here, no raw hex (§4). The mapping in
/// [`accent`](Self::accent) is the ONE authority every site reads — tiles,
/// chips, filmstrip dividers, IPAM bands, and the hero status ring's
/// discovering arc — alongside the per-kind procedural glyph family
/// ([`paint_kind_glyph`]) that rides the same accent at every scale.
/// Serialisable (`snake_case` tokens) because the active filter rides the
/// EXPLORER-13 view record (O5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
        // arch-11: open through the shared BusReader seam.
        let Some(persist) = BusReader::new(self.bus_root.clone()).open() else {
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
        // arch-11: writer — the shared BusReader seam is read-only; this publish
        // keeps Persist::open because it surfaces the write error to the caller.
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

    /// Whether this verb makes sense fanned across a multi-selection
    /// (EXPLORER-17, design O10): the per-unit lifecycle + health verbs —
    /// "reboot 3 instances, health-check 5 peers". A navigation hand-off
    /// (console / inspect / open-in-Fleet / invite) targets ONE surface for ONE
    /// unit and stays single-hero; the dead-seam verbs (`ObjectDelete`/`Evict`)
    /// are excluded by the seam filter anyway (§7 — no dead bulk verb).
    const fn bulk_capable(self) -> bool {
        matches!(
            self,
            Self::Start | Self::Stop | Self::Reboot | Self::Delete | Self::HealthCheck
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

// ─────────────── multi-select + armed bulk actions (EXPLORER-17, O10) ───────────────

/// A destructive **bulk** verb armed over the whole marked selection
/// (EXPLORER-17), awaiting its typed confirm — the same typed-arming interlock
/// as [`ArmedVerb`], keyed to the selection instead of one unit. The echo must
/// match [`bulk_phrase`] so the operator states exactly what fires and how
/// many units it hits before anything dispatches.
struct BulkArm {
    /// Which destructive verb is armed over the selection.
    verb: Verb,
    /// The operator's typed echo, matched against [`bulk_phrase`] to arm.
    echo: String,
}

/// The typed confirm phrase arming a bulk `verb` over `n` units — the verb
/// word plus the exact count (`"delete 3"`), so arming names the blast radius
/// the way the single-unit interlock names the unit.
fn bulk_phrase(verb: Verb, n: usize) -> String {
    format!("{} {n}", verb.label().to_lowercase())
}

/// The outcome of one bulk run (EXPLORER-17): the per-unit dispatch tallies —
/// an honest **requested** rollup (the requests were published; the fleet acts
/// asynchronously — never a fabricated "done", §7).
struct BulkRollup {
    /// The verb that ran.
    verb: Verb,
    /// How many marked units it fanned across.
    total: usize,
    /// How many per-unit dispatches published cleanly.
    ok: usize,
    /// The units whose dispatch failed: `(name, reason)`.
    failed: Vec<(String, String)>,
}

/// The rollup's inline note (`true` ⇒ carries a failure) — "Reboot requested
/// for 3/3 units.", failures named per-unit with their honest reason.
fn bulk_note(r: &BulkRollup) -> (String, bool) {
    let note = format!(
        "{} requested for {}/{} units.",
        r.verb.label(),
        r.ok,
        r.total
    );
    if r.failed.is_empty() {
        (note, false)
    } else {
        let names: Vec<String> = r
            .failed
            .iter()
            .map(|(name, why)| format!("{name} — {why}"))
            .collect();
        (format!("{note} Failed: {}.", names.join("; ")), true)
    }
}

/// The verbs the whole selection **shares** (EXPLORER-17, O10): the
/// intersection of every marked unit's per-type verb set, kept to the
/// bulk-capable lifecycle/health verbs, and only where the seam resolves for
/// EVERY unit — the bar never offers a verb that would dead-end on any member
/// (§7 — no dead bulk verb). Order follows the first unit's bar order. Pure —
/// the selection model, unit-tested without a render.
fn shared_bulk_verbs(units: &[Unit]) -> Vec<Verb> {
    let Some(first) = units.first() else {
        return Vec::new();
    };
    verbs_for(first.kind)
        .iter()
        .copied()
        .filter(|v| v.bulk_capable())
        .filter(|v| units.iter().all(|u| verbs_for(u.kind).contains(v)))
        .filter(|&v| units.iter().all(|u| verb_seam(v, u).is_ok()))
        .collect()
}

/// How a mosaic tile pick lands (EXPLORER-17 over the O3 zoom): Ctrl/Cmd
/// toggles the tile's mark, Shift range-marks from the focus anchor, and a
/// plain pick keeps the O1 zoom-into-hero. Pure over the modifier state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickAction {
    /// Toggle the picked tile's mark (Ctrl/Cmd-click).
    ToggleMark,
    /// Mark the whole run from the focus anchor to the pick (Shift-click).
    RangeMark,
    /// Zoom the picked tile into its hero (the plain O3 pick).
    Zoom,
}

/// Resolve the modifier state at pick time to its [`PickAction`].
const fn pick_action(mods: egui::Modifiers) -> PickAction {
    if mods.command || mods.ctrl {
        PickAction::ToggleMark
    } else if mods.shift {
        PickAction::RangeMark
    } else {
        PickAction::Zoom
    }
}

// ─────────────────────────── pure fold ───────────────────────────

/// The stable unit id a peer (or self) folds under — mirrors the aggregator's
/// `peer_unit_id`.
fn peer_self_id(host: &str) -> String {
    format!("peer:{host}")
}

/// Union every node's mirror into one shelf: dedup by id keeping the freshest
/// observation (lock #20 dedup), then order **pinned first** (O9), then **this
/// node** (#23), then by proximity category, then by name (locks #7). Pure —
/// the render's data model, unit-tested without a Bus.
fn fold_units(states: &[UnitsState], local_host: &str, pinned: &[String]) -> Vec<Unit> {
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
    sort_units(&mut units, &self_id, pinned);
    units
}

/// The ONE shelf ordering (shared by the fold and a live pin re-sort): pinned →
/// self → proximity category, then case-folded name, then id (deterministic).
fn sort_units(units: &mut [Unit], self_id: &str, pinned: &[String]) {
    units.sort_by(|a, b| {
        proximity_rank(a, self_id, pinned)
            .cmp(&proximity_rank(b, self_id, pinned))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// The sort key: pinned to the very front (O9), then this node, then the unit's
/// category slot.
fn proximity_rank(unit: &Unit, self_id: &str, pinned: &[String]) -> u8 {
    if pinned.iter().any(|p| p == &unit.id) {
        0
    } else if unit.id == self_id {
        1
    } else {
        // category().index() is 0..=2 → 2..=4, never overflowing a u8.
        u8::try_from(unit.kind.category().index()).unwrap_or(2) + 2
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
/// this node's own hero unit (#23) + orders it first. Crate-visible (`pub`, the
/// `clippy::redundant_pub_crate` form in a private module) so the NODE-GRADE-2 grade
/// fold ([`crate::chrome`]) pins the local grade row off the SAME resolution (no
/// third copy of the idiom).
pub fn local_hostname() -> String {
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

// ─────────────── universal search (EXPLORER-14, design O7) ───────────────
//
// A self-contained fuzzy matcher — the editor's `fuzzy` idiom (EDITOR-7)
// mirrored locally: that scorer is private to `mde-editor-egui`, and sharing it
// would mean a new cross-cutting module this unit's scope forbids, so the small
// subsequence scorer (boundary + contiguity bonuses, gap + leading penalties)
// lives here. Pure data-in / data-out — unit-tested without a render.

/// Max rows the ranked hit list shows — enough to disambiguate without burying
/// the best match (typing more narrows the head).
const SEARCH_MAX_HITS: usize = 8;

/// Score awarded when a matched char sits on a word boundary (a separator or a
/// camel-case hump) — the humps a human aims at.
const BOUNDARY_BONUS: i32 = 16;
/// Score awarded when a matched char immediately follows the previous match — a
/// contiguous run of the query (e.g. `5900` inside `22,5900`).
const CONTIGUOUS_BONUS: i32 = 8;
/// Bonus when the matched char has the same case as the query char, so an
/// exact-case hit edges out a case-folded one.
const CASE_BONUS: i32 = 2;
/// Penalty per skipped char in a gap between two matches (capped), so a tightly
/// packed match outranks a scattered one.
const GAP_PENALTY: i32 = -1;
/// Penalty per leading unmatched char before the first match (capped), so a
/// match near the start outranks one buried deep in the string.
const LEADING_PENALTY: i32 = -1;
/// Cap on the per-match gap / leading penalty so one very long field can't
/// dominate the score with penalties alone.
const PENALTY_CAP: usize = 12;

/// Whether `cur` begins a new "word" given the char `prev` before it — a
/// separator break (incl. the `:`/`,` the MAC / port-list fields carry) or a
/// camel-case hump.
const fn is_word_boundary(prev: char, cur: char) -> bool {
    let separator = matches!(prev, '/' | '\\' | '_' | '-' | '.' | ' ' | ':' | ',');
    let camel_hump = !prev.is_uppercase() && cur.is_uppercase();
    separator || camel_hump
}

/// The penalty count for a `gap` of skipped chars, capped so a single long span
/// can't swamp the score.
fn capped_gap(gap: usize) -> i32 {
    i32::try_from(gap.min(PENALTY_CAP)).unwrap_or(0)
}

/// Score `needle` against `haystack`, or `None` when `needle` is not an
/// in-order, case-insensitive subsequence of `haystack`. Higher is better; an
/// empty needle scores a neutral `0` (the caller decides what an empty query
/// means). Greedy left-to-right — the standard lightweight fuzzy heuristic —
/// with boundary/contiguity/case bonuses and gap/leading penalties folded in.
fn fuzzy_score(needle: &str, haystack: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = haystack.chars().collect();
    let ndl: Vec<char> = needle.chars().collect();

    let mut total: i32 = 0;
    let mut ni = 0usize;
    let mut prev: Option<usize> = None;

    for (hi, &hc) in hay.iter().enumerate() {
        let Some(&nc) = ndl.get(ni) else { break };
        if !hc.eq_ignore_ascii_case(&nc) {
            continue;
        }
        // Exactly one of the three position scores applies per matched char.
        total += match prev {
            Some(p) if p + 1 == hi => CONTIGUOUS_BONUS,
            Some(p) => GAP_PENALTY * capped_gap(hi - p - 1),
            None => LEADING_PENALTY * capped_gap(hi),
        };
        if hi == 0 || is_word_boundary(hay[hi - 1], hc) {
            total += BOUNDARY_BONUS;
        }
        if hc == nc {
            total += CASE_BONUS;
        }
        prev = Some(hi);
        ni += 1;
    }

    (ni == ndl.len()).then_some(total)
}

/// The searchable text fields of one unit (O7 "Everything"): name · address
/// (IP) · the LAN id key (the host's MAC, or its IP fallback) · the kind's type
/// words · the hosting node · the discovered service labels / open ports /
/// enrichment names. Only real discovered values — an absent field contributes
/// nothing (§7, never a fabricated haystack). Fields score independently so a
/// match can never straddle two unrelated fields.
fn search_fields(unit: &Unit) -> Vec<&str> {
    let mut fields = vec![unit.name.as_str(), unit.kind.search_terms()];
    if let Some(addr) = unit.address.as_deref() {
        fields.push(addr);
    }
    // The LAN unit id's key IS the host's MAC when ARP knows it (the aggregator's
    // `lan:<mac-or-ip>` contract) — the MAC-prefix search field.
    if let Some(key) = unit.id.strip_prefix("lan:") {
        fields.push(key);
    }
    if let Reachability::CloudObject { node } = &unit.reachability {
        fields.push(node);
    }
    let extras = &unit.extras;
    fields.extend(extras.rdns.as_deref());
    fields.extend(extras.oui_vendor.as_deref());
    fields.extend(extras.fingerprint.as_deref());
    for key in ["open_ports", "type_guess"] {
        if let Some(v) = extras.extra.get(key) {
            fields.push(v);
        }
    }
    fields
}

/// The unit's best single-field score for `query`, or `None` when no field
/// matches.
fn unit_search_score(query: &str, unit: &Unit) -> Option<i32> {
    search_fields(unit)
        .into_iter()
        .filter_map(|f| fuzzy_score(query, f))
        .max()
}

/// The `/` search overlay's live state (EXPLORER-14): the query, the selected
/// row in the ranked hit list, and the one-shot focus request for the box.
struct SearchState {
    /// The live query text (persisted while active — the O5 "active search").
    query: String,
    /// The selected hit row (Enter jumps it; Up/Down move it).
    sel: usize,
    /// Focus the query box on the next rendered frame (set on open/restore).
    focus_pending: bool,
}

impl SearchState {
    /// A freshly opened (or restored) search with `query` pre-filled.
    const fn open(query: String) -> Self {
        Self {
            query,
            sel: 0,
            focus_pending: true,
        }
    }
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

/// Which surface mode the Explorer renders (design O1's three modes). The
/// **mosaic** overview is the whole-fleet landing (EXPLORER-11); picking a tile
/// zooms into the one-unit **hero** card (EXPLORER-3); the **IPAM** table is the
/// NetBox-style discovered-address view (EXPLORER-10). The category filter chips
/// scope all three; the header toggles between them. Serialisable (`snake_case`
/// tokens) because the last mode rides the EXPLORER-13 view record (O5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SurfaceMode {
    /// The zoomable, category-clustered mosaic of mini hero tiles — the landing
    /// (EXPLORER-11, O1).
    #[default]
    Mosaic,
    /// The one-unit-at-a-time hero card (EXPLORER-3) — the zoom-in focus mode.
    Hero,
    /// The NetBox-style discovered prefix/IP table (EXPLORER-10).
    Ipam,
}

/// The file the Explorer's persisted preferences round-trip through — one JSON
/// record under the client data dir, the SETTINGS-nav / `PowerHonorConfig` idiom.
const PREFS_FILE: &str = "explorer-prefs.json";

/// The Explorer surface's persisted preferences: the EXPLORER-12 ambient-idle
/// toggle plus the EXPLORER-13 **view-continuity record** (O5) — the last mode,
/// last-selected unit id, and active category filter, restored on open so the
/// surface is continuous across lock/unlock and restarts. Persisted the
/// SETTINGS-nav way: one JSON file, atomic temp + rename, a missing / malformed /
/// legacy (ambient-only) file folding each absent field to its default (never a
/// fatal, §7).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
struct ExplorerPrefs {
    /// The EXPLORER-12 ambient idle auto-cycle toggle — **OFF by default** (the
    /// `bool` default): an unattended screen only comes alive when the operator
    /// opts in, never on its own.
    ambient_idle: bool,
    /// EXPLORER-13 — the last active surface mode (Mosaic/Hero/IPAM), restored
    /// on open (O5). Defaults to the mosaic landing.
    mode: SurfaceMode,
    /// EXPLORER-13 — the last-selected unit id, re-focused once its unit streams
    /// back in (`None` when nothing real was focused). A remembered unit that has
    /// left the fleet simply never lands — the view falls back to the front of
    /// the shelf (graceful, §7 — never a phantom selection).
    selected: Option<String>,
    /// EXPLORER-13 — the active category filter (`None` ⇒ All), restored on open.
    filter: Option<Category>,
    /// EXPLORER-14 — the active `/` search query (empty ⇒ the search is closed);
    /// a non-empty query restores the overlay open with it (the O5 "active
    /// search" half of the view record).
    search: String,
    /// EXPLORER-16 — the pinned unit ids (O9), in the order they were pinned;
    /// pinned units sort to the front of the mosaic + filmstrip.
    pinned: Vec<String>,
    /// EXPLORER-16 — the Pinned filter chip: scope the view to pinned units.
    /// Composes with the category filter (Pinned ∩ Cloud is a real scope).
    pinned_only: bool,
}

impl ExplorerPrefs {
    /// The default prefs path (`<client-data-dir>/explorer-prefs.json`), or `None`
    /// in a headless context — mirrors `SettingsNav::default_path`.
    fn default_path() -> Option<PathBuf> {
        mde_bus::client_data_dir().map(|d| d.join(PREFS_FILE))
    }

    /// Load from `path`, folding a missing / malformed file to the default.
    fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Load from the default path (default when absent / unresolvable).
    fn load() -> Self {
        Self::default_path().map_or_else(Self::default, |p| Self::load_from(&p))
    }

    /// Write to `path` (atomic temp + rename, like `SettingsNav::save_to`).
    ///
    /// # Errors
    /// The [`std::io::Error`] if the dir cannot be created or the file cannot be
    /// written / renamed.
    fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// Whether ANY user interaction landed this frame — a key/D-pad press, typed
/// text, a pointer button/move, or a scroll. A held key or pointer button also
/// counts, so the ambient cycle stays paused for the whole press, not just its
/// edge. This is the ONE signal the auto-cycle treats as "the operator is here".
fn is_user_input(i: &egui::InputState) -> bool {
    if i.pointer.any_down() || !i.keys_down.is_empty() {
        return true;
    }
    i.events.iter().any(|e| {
        matches!(
            e,
            egui::Event::Key { .. }
                | egui::Event::Text(_)
                | egui::Event::Paste(_)
                | egui::Event::PointerButton { .. }
                | egui::Event::PointerMoved(_)
                | egui::Event::MouseWheel { .. }
        )
    })
}

/// Whether `/` was pressed this frame — consumed (the key's paired text event is
/// dropped) so the slash that OPENS the search never leaks into the query box
/// the same frame (EXPLORER-14).
fn slash_pressed(ui: &egui::Ui) -> bool {
    ui.ctx().input_mut(|i| {
        let hit = i.key_pressed(egui::Key::Slash);
        if hit {
            i.events
                .retain(|e| !matches!(e, egui::Event::Text(t) if t == "/"));
        }
        hit
    })
}

/// Whether motion is suppressed — egui's `animation_time` collapsed to zero, which
/// is the shell's reduce-motion posture in the egui epoch (the retired `mde-theme`
/// reduce-motion engine is gone). An auto-moving wall display is exactly the
/// motion a reduce-motion seat opts out of (WCAG 2.2.2 pause/stop/hide), so the
/// ambient cycle stays parked under it.
fn reduce_motion(ctx: &egui::Context) -> bool {
    ctx.style().animation_time <= 0.0
}

/// Pure ambient-cycle timing gate: given the frame clock and the last-input /
/// last-advance marks (all egui-clock seconds), is a step due? A step waits for a
/// full [`AMBIENT_IDLE`] of quiet AND a full [`AMBIENT_DWELL`] since the previous
/// step — the first drives entry, the second throttles the crawl. Pure over its
/// inputs so the cadence is unit-tested without a render.
fn ambient_due(now: f64, last_input: f64, last_advance: f64) -> bool {
    now - last_input >= AMBIENT_IDLE.as_secs_f64()
        && now - last_advance >= AMBIENT_DWELL.as_secs_f64()
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
    /// The active surface mode — the mosaic overview, the hero card, or the IPAM
    /// table (EXPLORER-11/10). The mosaic is the landing (O1).
    mode: SurfaceMode,
    /// The origin tile rect a zoom-in animates out from (O3 shared-element zoom);
    /// `None` ⇒ the hero was entered without a spatial origin (a direct toggle /
    /// keyboard pick with no live tile rect) so it simply fades in.
    zoom_from: Option<Rect>,
    /// When the current tile→hero zoom-in began — the transition clock. `None` ⇒
    /// no zoom is in flight.
    zoom_start: Option<Instant>,
    /// When the mosaic was last (re-)entered from the hero — the O3 zoom-out
    /// settle; drives a brief fade-in so Back reads as a reverse zoom.
    mosaic_enter: Option<Instant>,
    /// The focused mosaic tile's on-screen rect from the last frame — the origin a
    /// keyboard/D-pad Enter zooms from (a mouse pick carries its own rect).
    focus_rect: Option<Rect>,
    /// The persisted surface preferences: the EXPLORER-12 ambient-idle toggle +
    /// the EXPLORER-13 view record (O5). Loaded + restored on construction;
    /// re-saved whenever the live view drifts from it ([`Self::persist_view`]).
    prefs: ExplorerPrefs,
    /// Where the prefs record persists — the client-data-dir file in production,
    /// `None` in tests / a headless context (no writes, the honest no-op).
    prefs_path: Option<PathBuf>,
    /// EXPLORER-12: the egui-clock second of the last user input — the idle clock
    /// the ambient cycle waits on. `None` until the first frame seeds it at mount,
    /// so the idle window starts when the surface opens, not at session start.
    last_input_at: Option<f64>,
    /// EXPLORER-12: the egui-clock second the ambient cycle last stepped the focus
    /// — the dwell throttle between auto-advances. Tracks the input clock while the
    /// operator is present so the first step lands a clean [`AMBIENT_IDLE`] later.
    last_advance_at: Option<f64>,
    /// A tap-to-focus target from OUTSIDE the surface (the NODE-GRADE-2 dock grade
    /// row → this node's hero, #7) that hasn't landed yet because its unit hasn't
    /// streamed in. Applied on each [`Self::refresh`] until the peer appears, so the
    /// jump lands even when the Explorer hadn't polled that peer when it was tapped.
    pending_focus: Option<String>,
    /// EXPLORER-14: the `/` universal-search overlay while open (`None` ⇒
    /// closed) — the query box + ranked hit list over every discovered unit.
    search: Option<SearchState>,
    /// EXPLORER-17: the marked unit ids (O10 multi-select), in mark order.
    /// Deliberately **transient** — a selection is action-scoped, so it does
    /// NOT ride the O5 view record; pruned to the live shelf on each refresh.
    marked: Vec<String>,
    /// EXPLORER-17: the destructive bulk verb armed over the selection,
    /// awaiting its typed [`bulk_phrase`] confirm (`None` ⇒ nothing armed).
    bulk_arm: Option<BulkArm>,
    /// EXPLORER-17: the last bulk run's per-unit rollup (`None` ⇒ no run yet).
    bulk_rollup: Option<BulkRollup>,
}

impl Default for ExplorerState {
    fn default() -> Self {
        let mut state = Self {
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
            zoom_from: None,
            zoom_start: None,
            mosaic_enter: None,
            focus_rect: None,
            prefs: ExplorerPrefs::default(),
            prefs_path: ExplorerPrefs::default_path(),
            last_input_at: None,
            last_advance_at: None,
            pending_focus: None,
            search: None,
            marked: Vec::new(),
            bulk_arm: None,
            bulk_rollup: None,
        };
        // EXPLORER-13 — restore the persisted view record (O5). The mirrors
        // haven't been read yet, so the remembered selection is held and lands
        // on the first refresh that carries its unit.
        state.apply_restore(ExplorerPrefs::load());
        state
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

    /// EXPLORER-12 — the ambient idle auto-cycle tick, run once per frame from
    /// [`Self::show`]. When the toggle is on and the surface has sat without ANY
    /// input for [`AMBIENT_IDLE`], step the shared focus one unit every
    /// [`AMBIENT_DWELL`] so an unattended screen keeps informing; ANY key / D-pad /
    /// pointer / scroll interaction resets the idle clock and pauses it at once, so
    /// it never fights the operator. Never runs under reduce-motion, and only in the
    /// hero / mosaic modes (the IPAM table has no focus to advance). Self-gating and
    /// cheap — safe to call every frame.
    fn tick_ambient(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        // ANY interaction this frame (or the very first frame) re-arms the idle
        // clock — the pause. Advancing the focus ourselves is NOT input, so the
        // cycle never treats its own step as a reason to stop.
        if self.last_input_at.is_none() || ctx.input(is_user_input) {
            self.last_input_at = Some(now);
            self.last_advance_at = Some(now);
            return;
        }
        // Gate: opted in, an advanceable mode, no search session in flight (the
        // operator is mid-thought — never crawl the focus out from under them),
        // real motion allowed, >1 unit to walk.
        if !self.prefs.ambient_idle
            || matches!(self.mode, SurfaceMode::Ipam)
            || self.search.is_some()
            || reduce_motion(ctx)
            || self.hero_count() < 2
        {
            return;
        }
        // Keep frames coming with no input so the idle timer actually elapses.
        ctx.request_repaint_after(AMBIENT_DWELL);
        let last_input = self.last_input_at.unwrap_or(now);
        let last_advance = self.last_advance_at.unwrap_or(now);
        if ambient_due(now, last_input, last_advance) {
            self.ambient_step();
            self.last_advance_at = Some(now);
        }
    }

    /// Advance the ambient cycle one unit, wrapping at the end so a wall display
    /// loops the shelf forever (unlike the operator's [`Self::page_next`], which
    /// clamps). A no-op below two units — nothing to cycle through.
    fn ambient_step(&mut self) {
        let count = self.hero_count();
        if count > 1 {
            self.focus = (self.focus + 1) % count;
        }
    }

    /// Flip the ambient-idle toggle and persist the new value (the SETTINGS-nav
    /// idiom — a durable choice that survives lock/unlock + restart). The flipping
    /// click is itself input, so the idle clock re-arms on the same frame and the
    /// cycle never fires the instant it's switched on.
    fn toggle_ambient(&mut self) {
        self.prefs.ambient_idle = !self.prefs.ambient_idle;
        self.save_prefs();
    }

    // ─────────────── view persistence (EXPLORER-13, design O5) ───────────────

    /// Restore a persisted view record (O5): the last mode, the active category
    /// filter, the last-selected unit — the selection is held via
    /// [`Self::pending_focus`] until its unit streams in, so a remembered unit
    /// that has left the fleet gracefully falls back to the front of the shelf
    /// (§7 — never a phantom focus) — and an active `/` search reopens with its
    /// query (EXPLORER-14). The ONE restore path [`Default`] and the tests share.
    fn apply_restore(&mut self, prefs: ExplorerPrefs) {
        self.mode = prefs.mode;
        self.filter = prefs.filter;
        self.pending_focus.clone_from(&prefs.selected);
        self.search = (!prefs.search.is_empty()).then(|| SearchState::open(prefs.search.clone()));
        self.prefs = prefs;
    }

    /// Persist the current prefs record to [`Self::prefs_path`] — a silent no-op
    /// in a headless/test context with no path (§7 — honest, never a fake write).
    fn save_prefs(&self) {
        if let Some(path) = &self.prefs_path {
            let _ = self.prefs.save_to(path);
        }
    }

    /// The focused unit's id in the current view (`None` on the #23 placeholder /
    /// an empty filtered view) — what the view record remembers as "selected".
    fn focused_unit_id(&self) -> Option<String> {
        self.filtered_indices()
            .get(self.focus)
            .map(|&i| self.units[i].id.clone())
    }

    /// The view-continuity record the live surface currently amounts to (O5).
    /// A still-held restore target ([`Self::pending_focus`]) stays the remembered
    /// selection until it lands, so an early frame before its unit streams in
    /// can't clobber the memory with `None`.
    fn view_snapshot(&self) -> ExplorerPrefs {
        ExplorerPrefs {
            ambient_idle: self.prefs.ambient_idle,
            mode: self.mode,
            selected: self
                .pending_focus
                .clone()
                .or_else(|| self.focused_unit_id()),
            filter: self.filter,
            search: self
                .search
                .as_ref()
                .map(|s| s.query.clone())
                .unwrap_or_default(),
            pinned: self.prefs.pinned.clone(),
            pinned_only: self.prefs.pinned_only,
        }
    }

    /// Persist the view when (and only when) it drifted from the stored record —
    /// one atomic write per real change (a mode/filter/selection move), nothing
    /// per idle frame. Driven from the end of [`Self::show`], so every input path
    /// (keys, chips, clicks, jumps) funnels through the one save.
    fn persist_view(&mut self) {
        let snapshot = self.view_snapshot();
        if snapshot != self.prefs {
            self.prefs = snapshot;
            self.save_prefs();
        }
    }

    // ─────────────── universal search (EXPLORER-14, design O7) ───────────────

    /// Open the `/` search overlay with an empty query (the box takes focus on
    /// the next frame).
    fn open_search(&mut self) {
        self.search = Some(SearchState::open(String::new()));
    }

    /// The ranked hits for `query` over the WHOLE shelf (O7 "Everything" — the
    /// search ignores the active category filter; the jump clears a hiding one):
    /// each unit's best field score, best first, ties keeping shelf order (a
    /// stable sort), capped at [`SEARCH_MAX_HITS`]. Returns absolute indices
    /// into [`Self::units`]. An empty/blank query yields nothing (the box just
    /// opened — no fake "everything matches" wall).
    fn search_hits(&self, query: &str) -> Vec<usize> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(usize, i32)> = self
            .units
            .iter()
            .enumerate()
            .filter_map(|(i, u)| unit_search_score(query, u).map(|s| (i, s)))
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        scored.truncate(SEARCH_MAX_HITS);
        scored.into_iter().map(|(i, _)| i).collect()
    }

    /// Land a search pick: jump the shared hero/mosaic focus to the hit (the ONE
    /// focus path — [`Self::jump_to_id`], which clears a hiding filter so the
    /// jump always lands) and close the overlay. A pick made from the IPAM table
    /// returns to the hero card first (the table has no per-unit focus to jump).
    fn jump_to_search_hit(&mut self, id: &str) {
        if self.mode == SurfaceMode::Ipam {
            self.mode = SurfaceMode::Hero;
        }
        self.jump_to_id(id);
        self.search = None;
    }

    /// Search-overlay input: Esc closes (clearing the query — the persisted
    /// "active search" only lives while the overlay does), Enter jumps the
    /// selected hit, Up/Down move the selection. Read raw so they work while the
    /// query box holds keyboard focus; every printable key stays the box's.
    fn handle_search_keys(&mut self, ui: &egui::Ui) {
        let (esc, enter, up, down) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
            )
        });
        if esc {
            self.search = None;
            return;
        }
        let Some(query) = self.search.as_ref().map(|s| s.query.clone()) else {
            return;
        };
        let hits = self.search_hits(&query);
        if hits.is_empty() {
            return;
        }
        let last = hits.len() - 1;
        if down {
            if let Some(s) = self.search.as_mut() {
                s.sel = (s.sel + 1).min(last);
            }
        }
        if up {
            if let Some(s) = self.search.as_mut() {
                s.sel = s.sel.saturating_sub(1);
            }
        }
        if enter {
            let sel = self.search.as_ref().map_or(0, |s| s.sel).min(last);
            let id = self.units[hits[sel]].id.clone();
            self.jump_to_search_hit(&id);
        }
    }

    /// The `/` search overlay (EXPLORER-14): the query box + the ranked hit list
    /// over every discovered unit — name/IP/MAC/type/node/service (O7). A row
    /// click (or Enter) jumps the hero/mosaic focus to the hit; the honest
    /// no-match note names the fields it searched (§7 — never a silent blank).
    fn search_overlay(&mut self, ui: &mut egui::Ui) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = Style::SP_S;
            ui.label(
                RichText::new("/")
                    .size(Style::BODY)
                    .strong()
                    .color(Style::ACCENT_HI),
            );
            let resp = ui.add(
                egui::TextEdit::singleline(&mut search.query)
                    .hint_text("Search name · IP · MAC · type · node · service")
                    .desired_width(Style::SP_XL * 8.0),
            );
            if search.focus_pending {
                resp.request_focus();
                search.focus_pending = false;
            }
            if resp.changed() {
                search.sel = 0;
            }
        });
        let (query, sel) = (search.query.clone(), search.sel);
        let hits = self.search_hits(&query);
        if hits.is_empty() {
            if !query.trim().is_empty() {
                muted_note(
                    ui,
                    "No unit matches — searched name, IP, MAC, type, node, and service.",
                );
            }
            return;
        }
        ui.add_space(Style::SP_XS);
        let mut jump: Option<String> = None;
        for (row, &abs) in hits.iter().enumerate() {
            let unit = &self.units[abs];
            if search_hit_row(ui, unit, row == sel.min(hits.len() - 1)) {
                jump = Some(unit.id.clone());
            }
        }
        if let Some(id) = jump {
            self.jump_to_search_hit(&id);
        }
    }

    // ─────────────── pinning + the Pinned cluster (EXPLORER-16, O9) ───────────────

    /// Whether `id` is pinned. The shelf is dozens of units, so the linear scan
    /// over the small persisted list beats maintaining a second mirror set.
    fn is_pinned(&self, id: &str) -> bool {
        self.prefs.pinned.iter().any(|p| p == id)
    }

    /// Pin/unpin a unit (O9): flip its id in the persisted pin set, re-sort the
    /// shelf so pinned units surface to the front at once, and keep the
    /// operator's focus on the unit it was on — a pin re-orders the shelf, it
    /// never teleports the view.
    fn toggle_pin(&mut self, id: &str) {
        let keep = self.focused_unit_id();
        if let Some(pos) = self.prefs.pinned.iter().position(|p| p == id) {
            self.prefs.pinned.remove(pos);
        } else {
            self.prefs.pinned.push(id.to_string());
        }
        self.save_prefs();
        let self_id = peer_self_id(&self.local_host);
        sort_units(&mut self.units, &self_id, &self.prefs.pinned);
        if let Some(keep) = keep {
            self.refocus(&keep);
        }
    }

    /// Re-anchor focus onto `id` in the current view WITHOUT touching any filter
    /// (unlike [`Self::jump_to_id`]): after a pin re-sort the unit may have left
    /// the view entirely (unpinned under the Pinned chip) — focus then folds to
    /// the front of what remains.
    fn refocus(&mut self, id: &str) {
        let target = self.units.iter().position(|u| u.id == id);
        let pos = target.and_then(|abs| self.filtered_indices().iter().position(|&i| i == abs));
        self.focus = pos.unwrap_or(0);
    }

    /// Toggle the Pinned filter chip (O9): scope every mode to pinned units,
    /// re-anchoring focus to the front of the new view (the `set_filter` idiom).
    fn set_pinned_only(&mut self, on: bool) {
        if self.prefs.pinned_only != on {
            self.prefs.pinned_only = on;
            self.focus = 0;
            self.save_prefs();
        }
    }

    /// The honest empty-view note: the Pinned scope explains how to pin; a
    /// category filter names its category (§7 — the note says why it's empty).
    fn empty_note_text(&self) -> String {
        if self.prefs.pinned_only {
            "No pinned units yet — press P (or right-click a tile) to pin one.".to_string()
        } else {
            format!(
                "No {} units discovered yet.",
                self.filter.map_or("", Category::label)
            )
        }
    }

    /// Re-read + re-fold the shelf. Split from the cadence gate so the pure fold
    /// stays testable; a dark Bus yields an empty shelf (→ the #23 self card),
    /// never a panic.
    fn refresh(&mut self) {
        let states = self.client.read();
        self.edges = fold_edges(&states);
        self.units = fold_units(&states, &self.local_host, &self.prefs.pinned);
        // EXPLORER-17 — a mark on a departed unit is meaningless: prune the
        // selection to the live shelf so a bulk verb can never target a ghost,
        // and drop a bulk arm whose selection emptied out from under it.
        let live: HashSet<&str> = self.units.iter().map(|u| u.id.as_str()).collect();
        self.marked.retain(|id| live.contains(id.as_str()));
        if self.marked.is_empty() {
            self.bulk_arm = None;
        }
        self.sample_history();
        self.apply_pending_focus();
    }

    /// Focus the hero card on a mesh node by hostname — the NODE-GRADE-2 dock grade
    /// row's tap target (#7). Reuses the surface's ONE focus-set path
    /// ([`Self::jump_to_id`], keyed by the aggregator's `peer:<host>` id) and lands
    /// in the hero mode. If that peer's unit hasn't streamed in yet the target is
    /// held ([`Self::pending_focus`]) and applied on the next refresh, so a tap that
    /// arrives before the Explorer has polled the peer still lands.
    pub fn focus_node(&mut self, host: &str) {
        let id = peer_self_id(host);
        self.mode = SurfaceMode::Hero;
        self.jump_to_id(&id);
        self.pending_focus = (!self.units.iter().any(|u| u.id == id)).then_some(id);
    }

    /// Re-apply a held cross-surface focus target once its unit has streamed in (a
    /// NODE-GRADE-2 tap that arrived before the peer's unit did). A no-op when
    /// nothing is pending or the target is still absent.
    fn apply_pending_focus(&mut self) {
        let Some(id) = self.pending_focus.clone() else {
            return;
        };
        if self.units.iter().any(|u| u.id == id) {
            self.jump_to_id(&id);
            self.pending_focus = None;
        }
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

    /// The indices of `units` matching the active category filter (all when
    /// `None`) and — under the EXPLORER-16 Pinned chip — the pin set.
    fn filtered_indices(&self) -> Vec<usize> {
        self.units
            .iter()
            .enumerate()
            .filter(|(_, u)| {
                self.filter.is_none_or(|c| u.kind.category() == c)
                    && (!self.prefs.pinned_only || self.is_pinned(&u.id))
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// How many hero pages the current view has — the filtered count, or **1** for
    /// the honest self placeholder when nothing has streamed in yet (#23). An
    /// active filter (category or Pinned) with no matches is an honest **0**,
    /// never a fake self card.
    fn hero_count(&self) -> usize {
        let n = self.filtered_indices().len();
        if n == 0 && self.filter.is_none() && !self.prefs.pinned_only {
            1
        } else {
            n
        }
    }

    /// Per-category rollup counts over the whole shelf (drives the chip badges +
    /// the O2 summary strip).
    fn category_counts(&self) -> [usize; 3] {
        let mut counts = [0usize; 3];
        for unit in &self.units {
            counts[unit.kind.category().index()] += 1;
        }
        counts
    }

    /// The fleet health rollup over the whole shelf — `[green, warn, down]`: green
    /// = healthy, warn = degraded, down = critical **or** unreachable. An
    /// unprobed/unknown unit is counted in none of them (honest, §7). Drives the O2
    /// summary strip's health tallies.
    fn health_rollup(&self) -> [usize; 3] {
        let mut rollup = [0usize; 3];
        for unit in &self.units {
            match unit.health {
                Some(Health::Healthy) => rollup[0] += 1,
                Some(Health::Degraded) => rollup[1] += 1,
                Some(Health::Critical | Health::Unreachable) => rollup[2] += 1,
                _ => {}
            }
        }
        rollup
    }

    /// The count of discovered units that reported a (non-empty) address — the O2
    /// summary strip's "total addresses" tally. A unit with no address is never
    /// counted (§7 — only real discovery).
    fn total_addresses(&self) -> usize {
        self.units
            .iter()
            .filter(|u| u.address.as_deref().is_some_and(|a| !a.trim().is_empty()))
            .count()
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
    /// scope the hero shelf); under the EXPLORER-16 Pinned chip the table keeps
    /// the prefixes where a pinned unit lives (whole-prefix context — the
    /// neighbours around your pinned units are the point of an address table).
    /// Pure over the folded state — the render's data model, unit-tested without
    /// a Bus.
    fn ipam_prefixes(&self) -> Vec<IpamPrefix> {
        derive_prefixes(&self.units, &self.edges)
            .into_iter()
            .filter(|p| self.filter.is_none_or(|c| p.category == c))
            .filter(|p| {
                !self.prefs.pinned_only || p.occupants.iter().any(|o| self.is_pinned(&o.unit_id))
            })
            .collect()
    }

    /// A row-click in the IPAM table: return to the hero card and jump its focus to
    /// the occupant unit — reusing the surface's ONE focus-set/jump path
    /// ([`Self::jump_to_id`], which also clears a hiding filter so the jump lands).
    fn jump_from_ipam(&mut self, id: &str) {
        self.mode = SurfaceMode::Hero;
        self.jump_to_id(id);
    }

    // ─────────────────── the mosaic overview (EXPLORER-11) ───────────────────

    /// Switch surface mode from a direct header toggle — a clean cut, not a stale
    /// shared-element zoom: any in-flight zoom is cleared, and landing on the
    /// mosaic seeds the O3 settle fade. A no-op toggle to the current mode leaves
    /// the animation state untouched.
    fn set_mode(&mut self, mode: SurfaceMode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.zoom_from = None;
        self.zoom_start = None;
        self.mosaic_enter = (mode == SurfaceMode::Mosaic).then(Instant::now);
    }

    /// Zoom a picked mosaic tile into its full hero (O1/O3): focus the unit at
    /// `pos` (its index in the current filtered view), switch to the hero mode, and
    /// seed the shared-element zoom from the tile's `from` rect (a keyboard Enter
    /// with no live rect passes `None` → the hero simply fades in). Reuses the ONE
    /// focus-set path (a stale arm/note from the old focus is dropped).
    fn zoom_into(&mut self, pos: usize, from: Option<Rect>) {
        self.focus = pos;
        self.mode = SurfaceMode::Hero;
        self.zoom_from = from;
        self.zoom_start = Some(Instant::now());
        self.mosaic_enter = None;
        self.arm = None;
        self.last_action_note = None;
    }

    /// Zoom back out to the mosaic overview (O3 reverse — Back/Esc): return to the
    /// mosaic with the just-focused tile still selected (spatially coherent) and a
    /// brief settle fade. The hero's zoom-in state is cleared.
    fn back_to_mosaic(&mut self) {
        self.mode = SurfaceMode::Mosaic;
        self.zoom_from = None;
        self.zoom_start = None;
        self.mosaic_enter = Some(Instant::now());
    }

    /// The current tile→hero zoom transform: `Some((rect, opacity))` while the O3
    /// shared-element zoom is in flight, else `None` once it completes (or was
    /// never seeded). Clears the zoom state on completion so the hero settles into
    /// its normal full-frame paging.
    fn zoom_progress(&mut self, full: Rect) -> Option<(Rect, f32)> {
        let start = self.zoom_start?;
        let p = (start.elapsed().as_secs_f32() / ZOOM_SECS).clamp(0.0, 1.0);
        if p >= 1.0 {
            self.zoom_from = None;
            self.zoom_start = None;
            return None;
        }
        let eased = ease_out(p);
        let from = self.zoom_from.unwrap_or(full);
        Some((
            lerp_rect(from, full, eased),
            flerp(ZOOM_FADE_FLOOR, 1.0, eased),
        ))
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
            // The EXPLORER-16 pin toggle leads the bar: a purely local shelf
            // re-order (no bus seam to gate), so it is never armed.
            let pinned = self.is_pinned(&unit.id);
            let pin_text = RichText::new(if pinned { "Unpin" } else { "Pin" })
                .size(Style::SMALL)
                .color(if pinned {
                    Style::ACCENT_HI
                } else {
                    Style::TEXT
                });
            let pin_button = egui::Button::new(pin_text)
                .fill(Style::SURFACE)
                .stroke(Stroke::new(
                    1.0,
                    if pinned {
                        Style::ACCENT_HI
                    } else {
                        Style::BORDER
                    },
                ));
            if ui
                .add(pin_button)
                .on_hover_text("Pinned units sort to the front (P)")
                .clicked()
            {
                self.toggle_pin(&unit.id);
            }
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

    // ─────────── multi-select + armed bulk actions (EXPLORER-17, O10) ───────────

    /// Whether `id` is in the marked selection (the same linear-scan trade-off
    /// as [`Self::is_pinned`] — the mark set is small).
    fn is_marked(&self, id: &str) -> bool {
        self.marked.iter().any(|m| m == id)
    }

    /// Mark/unmark one unit (Ctrl/Cmd-click, or Space on the D-pad). Emptying
    /// the selection disarms any pending bulk verb — nothing left to fire at.
    fn toggle_mark(&mut self, id: &str) {
        if let Some(pos) = self.marked.iter().position(|m| m == id) {
            self.marked.remove(pos);
        } else {
            self.marked.push(id.to_string());
        }
        if self.marked.is_empty() {
            self.bulk_arm = None;
        }
    }

    /// Shift-click range mark: add every unit between view positions `a` and
    /// `b` (inclusive, either order) in the current filtered view to the
    /// selection — additive, like every file-manager range select.
    fn mark_range(&mut self, a: usize, b: usize) {
        let view = self.filtered_indices();
        for pos in a.min(b)..=a.max(b) {
            let Some(&idx) = view.get(pos) else { continue };
            let id = &self.units[idx].id;
            if !self.is_marked(id) {
                self.marked.push(id.clone());
            }
        }
    }

    /// Clear the whole selection (Esc / the Clear button) — marks, any pending
    /// bulk arm, and the stale rollup note go together.
    fn clear_marks(&mut self) {
        self.marked.clear();
        self.bulk_arm = None;
        self.bulk_rollup = None;
    }

    /// The marked units still on the shelf, in shelf order (the deterministic
    /// per-unit dispatch + rollup order).
    fn marked_units(&self) -> Vec<Unit> {
        self.units
            .iter()
            .filter(|u| self.is_marked(&u.id))
            .cloned()
            .collect()
    }

    /// Whether the armed bulk verb's typed echo matches the exact
    /// [`bulk_phrase`] for the CURRENT selection size — a selection that grew
    /// or shrank since arming re-gates until the operator re-states the count.
    fn bulk_ready(&self) -> bool {
        let n = self.marked_units().len();
        n > 0
            && self
                .bulk_arm
                .as_ref()
                .is_some_and(|a| a.echo.trim() == bulk_phrase(a.verb, n))
    }

    /// The bulk confirm gate: fire the armed verb across the selection IFF the
    /// typed phrase matches ([`Self::bulk_ready`]). Returns whether it ran —
    /// the ONE gate the Confirm button and the tests share (the
    /// [`Self::confirm_armed`] idiom, selection-wide).
    fn confirm_bulk(&mut self) -> bool {
        if !self.bulk_ready() {
            return false;
        }
        let Some(arm) = self.bulk_arm.take() else {
            return false;
        };
        self.run_bulk(arm.verb);
        true
    }

    /// Execute `verb` across every marked unit, one real dispatch per unit
    /// (O10), folding the outcomes into the [`BulkRollup`]. Callers gate:
    /// non-destructive verbs run directly, destructive only via
    /// [`Self::confirm_bulk`].
    fn run_bulk(&mut self, verb: Verb) {
        let units = self.marked_units();
        let total = units.len();
        let mut ok = 0usize;
        let mut failed: Vec<(String, String)> = Vec::new();
        for unit in &units {
            match verb_seam(verb, unit).and_then(|action| self.dispatch(&action)) {
                Ok(()) => ok += 1,
                Err(why) => failed.push((unit.name.clone(), why)),
            }
        }
        self.bulk_rollup = Some(BulkRollup {
            verb,
            total,
            ok,
            failed,
        });
    }

    /// The bulk action bar under the mosaic (EXPLORER-17): the selection count,
    /// the verbs the whole selection SHARES (or the honest none-shared note,
    /// §7), Clear, the typed bulk-arming challenge, and the per-unit rollup of
    /// the last run.
    fn bulk_bar(&mut self, ui: &mut egui::Ui) {
        let units = self.marked_units();
        let n = units.len();
        let verbs = shared_bulk_verbs(&units);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = Style::SP_S;
            ui.label(
                RichText::new(format!("{n} selected"))
                    .size(Style::SMALL)
                    .strong()
                    .color(Style::TEXT),
            );
            if verbs.is_empty() {
                // A mixed selection with nothing in common: say so — never a
                // dead or padded verb (§7).
                ui.label(
                    RichText::new("No shared actions across this selection.")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
            }
            for &verb in &verbs {
                let armed_here = self.bulk_arm.as_ref().is_some_and(|a| a.verb == verb);
                let tint = if verb.destructive() {
                    Style::DANGER
                } else {
                    Style::TEXT
                };
                let button =
                    egui::Button::new(RichText::new(verb.label()).size(Style::SMALL).color(tint))
                        .fill(Style::SURFACE)
                        .stroke(Stroke::new(
                            1.0,
                            if armed_here {
                                Style::DANGER
                            } else {
                                Style::BORDER
                            },
                        ));
                if ui.add(button).clicked() {
                    if verb.destructive() {
                        self.bulk_arm = Some(BulkArm {
                            verb,
                            echo: String::new(),
                        });
                        self.bulk_rollup = None;
                    } else {
                        self.run_bulk(verb);
                    }
                }
            }
            if ui
                .button(RichText::new("Clear").size(Style::SMALL))
                .clicked()
            {
                self.clear_marks();
            }
        });
        // The typed bulk challenge (the EXPLORER-5 arming idiom, selection-wide).
        if let Some(verb) = self.bulk_arm.as_ref().map(|a| a.verb) {
            self.bulk_challenge(ui, verb, n);
        }
        // The honest per-unit rollup of the last run.
        if let Some(rollup) = &self.bulk_rollup {
            let (note, is_err) = bulk_note(rollup);
            ui.add_space(Style::SP_XS);
            ui.label(RichText::new(note).size(Style::SMALL).color(if is_err {
                Style::DANGER
            } else {
                Style::TEXT_DIM
            }));
        }
    }

    /// The typed **bulk** challenge row (EXPLORER-17): type the exact
    /// [`bulk_phrase`] (`"<verb> <count>"`) to enable Confirm — the
    /// [`Self::arm_challenge`] idiom widened to the whole selection. Confirm
    /// fires through the ONE gate ([`Self::confirm_bulk`]).
    fn bulk_challenge(&mut self, ui: &mut egui::Ui, verb: Verb, n: usize) {
        let phrase = bulk_phrase(verb, n);
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new(format!(
                "Type \u{201C}{phrase}\u{201D} to arm {} on {n} units.",
                verb.label().to_lowercase()
            ))
            .size(Style::SMALL)
            .color(Style::WARN),
        );
        ui.add_space(Style::SP_XS);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = Style::SP_S;
            if let Some(arm) = self.bulk_arm.as_mut() {
                ui.add(
                    egui::TextEdit::singleline(&mut arm.echo)
                        .hint_text(phrase.as_str())
                        .desired_width(Style::SP_XL * 5.0),
                );
            }
            let ready = self.bulk_ready();
            let confirm = egui::Button::new(
                RichText::new(format!("Confirm {}", verb.label())).size(Style::SMALL),
            )
            .fill(Style::SURFACE)
            .stroke(Stroke::new(1.0, Style::DANGER));
            if ui.add_enabled(ready, confirm).clicked() {
                self.confirm_bulk();
            }
            if ui
                .button(RichText::new("Cancel").size(Style::SMALL))
                .clicked()
            {
                self.bulk_arm = None;
            }
        });
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
        // Input routing: the open `/` search owns the keyboard (EXPLORER-14);
        // otherwise the per-mode nav runs (O6 — the hero pages + zooms out, the
        // mosaic grid-navs + zooms in; the IPAM table is scroll-only), gated off
        // whenever ANY text box holds focus (the search box, the arming echo) so
        // typing a name never pages the shelf or zooms out from under the
        // operator.
        if self.search.is_some() {
            self.handle_search_keys(ui);
        } else if !ui.ctx().wants_keyboard_input() {
            if slash_pressed(ui) {
                self.open_search();
            } else {
                // P pins/unpins the focused unit (EXPLORER-16, O9) in the two
                // modes that HAVE a visible focused unit.
                if !matches!(self.mode, SurfaceMode::Ipam)
                    && ui.input(|i| i.key_pressed(egui::Key::P))
                {
                    if let Some(id) = self.focused_unit_id() {
                        self.toggle_pin(&id);
                    }
                }
                match self.mode {
                    SurfaceMode::Hero => self.handle_keys(ui),
                    SurfaceMode::Mosaic => self.handle_mosaic_keys(ui),
                    SurfaceMode::Ipam => {}
                }
            }
        }
        // Keep focus valid against the freshest (possibly re-filtered) view — the
        // one focus index the mosaic tiles + hero pages share.
        let count = self.hero_count();
        self.focus = if count == 0 {
            0
        } else {
            self.focus.min(count - 1)
        };

        // EXPLORER-12: the ambient idle auto-cycle — crawl the shared focus forward
        // while the surface sits untouched (a no-op when the toggle is off, input
        // just landed, or reduce-motion is set). After the focus clamp so a step
        // lands on a valid index; before the panels so it shows this same frame.
        self.tick_ambient(ui.ctx());

        egui::TopBottomPanel::top(ui.id().with("explorer-chips"))
            .frame(egui::Frame::NONE.inner_margin(Style::SP_S))
            .show_inside(ui, |ui| self.header(ui));
        // The `/` universal-search overlay rides between the summary strip and
        // the active mode, so the hit list never covers the filter chips.
        if self.search.is_some() {
            egui::TopBottomPanel::top(ui.id().with("explorer-search"))
                .frame(egui::Frame::NONE.inner_margin(Style::SP_S))
                .show_inside(ui, |ui| self.search_overlay(ui));
        }
        match self.mode {
            SurfaceMode::Mosaic => {
                // EXPLORER-17 — the bulk action bar rides under the mosaic
                // while units are marked: the shared verbs over the selection,
                // the typed bulk arming, and the per-unit rollup.
                if !self.marked.is_empty() {
                    egui::TopBottomPanel::bottom(ui.id().with("explorer-bulk"))
                        .frame(egui::Frame::NONE.inner_margin(Style::SP_S))
                        .show_inside(ui, |ui| self.bulk_bar(ui));
                }
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.inner_margin(Style::SP_S))
                    .show_inside(ui, |ui| self.mosaic(ui));
            }
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
        // EXPLORER-13 — persist the view record when this frame changed it (O5):
        // after the panels, so every input path above funnels into the one save.
        self.persist_view();
    }

    /// The summary/filter strip (O2): the Mosaic ⇄ Hero ⇄ IPAM mode toggle + the
    /// right-aligned fleet rollup (health tallies + total addresses), then the
    /// category filter chips (#8) that scope whichever mode is active.
    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = Style::SP_S;
            if chip(
                ui,
                "Mosaic",
                self.mode == SurfaceMode::Mosaic,
                Style::ACCENT,
                Style::TEXT,
            ) {
                self.set_mode(SurfaceMode::Mosaic);
            }
            if chip(
                ui,
                "Hero",
                self.mode == SurfaceMode::Hero,
                Style::ACCENT,
                Style::TEXT,
            ) {
                self.set_mode(SurfaceMode::Hero);
            }
            if chip(
                ui,
                "IPAM",
                self.mode == SurfaceMode::Ipam,
                Style::ACCENT,
                Style::TEXT,
            ) {
                self.set_mode(SurfaceMode::Ipam);
            }
            // The O2 fleet rollup pushed to the right edge, with the EXPLORER-12
            // ambient-idle toggle just inboard of it (right-to-left adds the rollup
            // first, so the toggle sits to its left).
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.rollup(ui);
                ui.add_space(Style::SP_M);
                if chip(
                    ui,
                    "Ambient",
                    self.prefs.ambient_idle,
                    Style::ACCENT,
                    Style::TEXT,
                ) {
                    self.toggle_ambient();
                }
            });
        });
        ui.add_space(Style::SP_XS);
        self.chips(ui);
    }

    /// The O2 fleet rollup cluster on the right of the summary strip: the health
    /// tallies (green / warn / down) + the total discovered addresses. A live
    /// whole-fleet glance over the folded shelf (§7 — real tiers only).
    fn rollup(&self, ui: &mut egui::Ui) {
        let [green, warn, down] = self.health_rollup();
        ui.spacing_mut().item_spacing.x = Style::SP_S;
        // Right-to-left layout → add right-most first: addresses, then down/warn/up.
        ui.label(
            RichText::new(format!("{} addr", self.total_addresses()))
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        health_dot(ui, Style::DANGER, down);
        health_dot(ui, Style::WARN, warn);
        health_dot(ui, Style::OK, green);
    }

    /// Hero-mode input (#6, O6 D-pad-first): Left/Right page, Home/End jump to the
    /// ends, Esc/Backspace zoom back out to the mosaic overview (O3). Consumed from
    /// this frame's input; a fullscreen text surface never sees them because only
    /// the active surface renders.
    fn handle_keys(&mut self, ui: &egui::Ui) {
        let (left, right, home, end, back) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::Home),
                i.key_pressed(egui::Key::End),
                i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Backspace),
            )
        });
        if back {
            self.back_to_mosaic();
            return;
        }
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

    /// Mosaic-mode grid nav (O6/O11): Left/Right step one tile, Up/Down move a
    /// whole row, Home/End jump to the ends, Enter zooms the focused tile into
    /// its hero (O3), **Space marks it** (the EXPLORER-17 D-pad mark — the
    /// file-manager idiom), and Esc clears a live selection. Couch-or-desk —
    /// the same focus index the hero pages, so a zoom lands on exactly the
    /// selected tile. The column step matches the render's (`mosaic_columns`
    /// over the inner content width).
    fn handle_mosaic_keys(&mut self, ui: &egui::Ui) {
        let cols = mosaic_columns(ui.available_width() - Style::SP_S * 2.0);
        let count = self.hero_count();
        let (left, right, up, down, home, end, enter, mark, esc) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::Home),
                i.key_pressed(egui::Key::End),
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if mark {
            if let Some(id) = self.focused_unit_id() {
                self.toggle_mark(&id);
            }
        }
        if esc && !self.marked.is_empty() {
            self.clear_marks();
        }
        if left {
            self.focus = grid_move(self.focus, count, cols, GridDir::Left);
        }
        if right {
            self.focus = grid_move(self.focus, count, cols, GridDir::Right);
        }
        if up {
            self.focus = grid_move(self.focus, count, cols, GridDir::Up);
        }
        if down {
            self.focus = grid_move(self.focus, count, cols, GridDir::Down);
        }
        if home {
            self.focus = 0;
        }
        if end {
            self.focus = count.saturating_sub(1);
        }
        if enter && count > 0 {
            let from = self.focus_rect;
            self.zoom_into(self.focus, from);
        }
    }

    /// The top category filter chips (#8): All + Mesh/LAN/Cloud with rollup
    /// counts, each accent-tinted (O8), plus the EXPLORER-16 **Pinned** chip (O9)
    /// scoping to the pin set (composable with a category). Selecting one scopes
    /// the shelf; All clears both axes.
    fn chips(&mut self, ui: &mut egui::Ui) {
        let counts = self.category_counts();
        let total = self.units.len();
        // The honest pin tally: pinned ids whose unit is actually on the shelf
        // (a pinned unit that left the fleet isn't counted as present, §7).
        let pinned_here = self.units.iter().filter(|u| self.is_pinned(&u.id)).count();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = Style::SP_S;
            if chip(
                ui,
                &format!("All · {total}"),
                self.filter.is_none() && !self.prefs.pinned_only,
                Style::ACCENT,
                Style::TEXT,
            ) {
                self.set_filter(None);
                self.set_pinned_only(false);
            }
            for cat in Category::ALL {
                let label = format!("{} · {}", cat.label(), counts[cat.index()]);
                let active = self.filter == Some(cat);
                // The chip wears its category accent at rest too (EXPLORER-15,
                // O8) — the filter row IS the category legend.
                if chip(ui, &label, active, cat.accent(), cat.accent()) {
                    self.set_filter(if active { None } else { Some(cat) });
                }
            }
            let pin_active = self.prefs.pinned_only;
            if chip(
                ui,
                &format!("Pinned · {pinned_here}"),
                pin_active,
                Style::ACCENT_HI,
                Style::TEXT,
            ) {
                self.set_pinned_only(!pin_active);
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

        // The shared-element zoom-in (O3): while a tile→hero zoom is in flight the
        // hero card grows from the picked tile's rect with a fade, snapping the
        // page-slide out of the way so the two don't fight; otherwise the usual
        // Carbon slide + cross-fade on a page change (#21).
        let anim_id = ui.id().with("explorer-hero-anim");
        let (child_rect, fade) = if let Some((rect, opacity)) = self.zoom_progress(full) {
            ui.ctx()
                .animate_value_with_time(anim_id, self.focus as f32, 0.0);
            if opacity < 1.0 {
                ui.ctx().request_repaint();
            }
            (rect, opacity)
        } else {
            let visual = ui
                .ctx()
                .animate_value_with_time(anim_id, self.focus as f32, Motion::BASE);
            let delta = self.focus as f32 - visual;
            let slide = (delta * full.width() * SLIDE_FRACTION).clamp(-full.width(), full.width());
            let fade = (1.0 - delta.abs()).clamp(0.0, 1.0);
            (full.translate(Vec2::new(slide, 0.0)), fade)
        };
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
            None if self.filter.is_none() && !self.prefs.pinned_only => {
                // #23 — no mirror yet: show THIS node, discovering.
                hero_card(&mut child, &self_placeholder(&self.local_host), true, None);
            }
            None => {
                // A filter/Pinned scope with no matches — honest, not blank.
                child.add_space(full.height() * 0.35);
                muted_note(&mut child, self.empty_note_text());
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
    /// cluster dividers (the O9 Pinned run first, then the #8 categories), the
    /// focused thumb accented; a click jumps the hero (#6), a right-click toggles
    /// the thumb's pin (O9).
    fn filmstrip(&mut self, ui: &mut egui::Ui) {
        let indices = self.filtered_indices();
        if indices.is_empty() {
            ui.allocate_space(Vec2::new(ui.available_width(), THUMB_H));
            return;
        }
        egui::ScrollArea::horizontal().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = Style::SP_S;
                let mut last_cluster: Option<Cluster> = None;
                let mut jump: Option<usize> = None;
                let mut pin_toggle: Option<String> = None;
                for (pos, &idx) in indices.iter().enumerate() {
                    let unit = &self.units[idx];
                    let pinned = self.is_pinned(&unit.id);
                    let cluster = if pinned {
                        Cluster::Pinned
                    } else {
                        Cluster::Cat(unit.kind.category())
                    };
                    // Cluster dividers — only meaningful in the unfiltered view.
                    if self.filter.is_none() && last_cluster != Some(cluster) {
                        filmstrip_divider(ui, cluster);
                        last_cluster = Some(cluster);
                    }
                    let (clicked, pin) = thumbnail(ui, unit, pos == self.focus, pinned);
                    if clicked {
                        jump = Some(pos);
                    }
                    if pin {
                        pin_toggle = Some(unit.id.clone());
                    }
                }
                if let Some(pos) = jump {
                    self.focus = pos;
                }
                if let Some(id) = pin_toggle {
                    self.toggle_pin(&id);
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

    /// The mosaic overview (EXPLORER-11, design O1): a category-clustered grid of
    /// mini hero tiles — the whole-fleet landing. Picking a tile zooms it into its
    /// full hero (O3); the keyboard/D-pad focus ring always shows the selection
    /// (O6/O11). Honest-empty falls back to this node's own discovering tile (#23),
    /// or a "no matches" note under a filter — never a blank pane (§7).
    fn mosaic(&mut self, ui: &mut egui::Ui) {
        let indices = self.filtered_indices();
        if indices.is_empty() {
            self.focus_rect = None;
            ui.add_space(Style::SP_L);
            if self.filter.is_none() && !self.prefs.pinned_only {
                // #23 — no mirror yet: show THIS node's own tile, discovering.
                let me = self_placeholder(&self.local_host);
                ui.vertical_centered(|ui| {
                    mosaic_tile(ui, &me, true, false, false);
                    ui.add_space(Style::SP_S);
                    muted_note(ui, "Discovering units… others tile in as they're found.");
                });
            } else {
                muted_note(ui, self.empty_note_text());
            }
            return;
        }

        // The O3 zoom-out settle: a brief fade-in when the mosaic was just
        // re-entered from a hero, so Back reads as a reverse zoom.
        let settle = if let Some(t) = self.mosaic_enter {
            let p = (t.elapsed().as_secs_f32() / Motion::BASE).clamp(0.0, 1.0);
            if p >= 1.0 {
                self.mosaic_enter = None;
            }
            ui.ctx().request_repaint();
            flerp(ZOOM_FADE_FLOOR, 1.0, p)
        } else {
            1.0
        };

        // Cluster the (already pinned-then-proximity-sorted) filtered view into
        // contiguous runs — the Pinned front cluster (O9) then the category
        // clusters (O1/O8).
        let clusters = self.cluster_runs(&indices);

        let focus = self.focus;
        let mut pick: Option<(usize, Rect)> = None;
        let mut pin_toggle: Option<String> = None;
        let mut focus_rect: Option<Rect> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_opacity(settle);
                let cols = mosaic_columns(ui.available_width());
                let mut pos = 0usize; // the running index into the filtered view
                for (cluster, run) in &clusters {
                    mosaic_cluster_header(ui, *cluster, run.len());
                    let mut k = 0;
                    while k < run.len() {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = MOSAIC_GAP;
                            for _ in 0..cols {
                                let Some(&idx) = run.get(k) else { break };
                                let focused = pos == focus;
                                let unit = &self.units[idx];
                                let pinned = self.is_pinned(&unit.id);
                                let marked = self.is_marked(&unit.id);
                                let (rect, clicked, pin) =
                                    mosaic_tile(ui, unit, focused, pinned, marked);
                                if focused {
                                    focus_rect = Some(rect);
                                }
                                if clicked {
                                    pick = Some((pos, rect));
                                }
                                if pin {
                                    pin_toggle = Some(unit.id.clone());
                                }
                                pos += 1;
                                k += 1;
                            }
                        });
                        ui.add_space(MOSAIC_GAP);
                    }
                }
            });
        self.focus_rect = focus_rect;
        if let Some(id) = pin_toggle {
            self.toggle_pin(&id);
        }
        // EXPLORER-17 — a modified pick marks instead of zooming: Ctrl/Cmd
        // toggles the tile, Shift range-marks from the focus anchor; a plain
        // pick keeps the O3 zoom.
        if let Some((pos, rect)) = pick {
            match pick_action(ui.input(|i| i.modifiers)) {
                PickAction::ToggleMark => {
                    if let Some(&idx) = self.filtered_indices().get(pos) {
                        let id = self.units[idx].id.clone();
                        self.toggle_mark(&id);
                    }
                    self.focus = pos;
                }
                PickAction::RangeMark => {
                    self.mark_range(self.focus, pos);
                    self.focus = pos;
                }
                PickAction::Zoom => self.zoom_into(pos, Some(rect)),
            }
        }
    }

    /// Fold the (already sorted) filtered view into contiguous cluster runs —
    /// the Pinned front cluster (O9) then the category runs (O1/O8). Pure over
    /// the folded state — the mosaic's grouping model, unit-tested without a
    /// render.
    fn cluster_runs(&self, indices: &[usize]) -> Vec<(Cluster, Vec<usize>)> {
        let mut clusters: Vec<(Cluster, Vec<usize>)> = Vec::new();
        for &idx in indices {
            let unit = &self.units[idx];
            let cluster = if self.is_pinned(&unit.id) {
                Cluster::Pinned
            } else {
                Cluster::Cat(unit.kind.category())
            };
            match clusters.last_mut() {
                Some((c, run)) if *c == cluster => run.push(idx),
                _ => clusters.push((cluster, vec![idx])),
            }
        }
        clusters
    }
}

mod render;
use render::*;

#[cfg(test)]
mod tests;
