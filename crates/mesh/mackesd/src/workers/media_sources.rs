//! MEDIA-14 — the mackesd **mesh media-source discovery aggregator**.
//!
//! Design: `docs/design/mesh-media-player.md` (row 26 "Mesh discovery"). The
//! `mde-media-egui` Sources panel (MEDIA-8) renders ONE list of every mesh
//! media source; this worker is the mesh-side (§6) collector that builds it.
//! Two discovery lanes, folded into one deduped roster published to
//! [`MEDIA_SOURCES_TOPIC`] (`state/media/sources`):
//!
//! 1. **Mesh registry (peer-advertised).** Every node ALREADY advertises the
//!    media services it runs through the replicated peers plane
//!    (`mackes_mesh_types::peers::PeerRecord`, PD-2): its listening media ports
//!    (`descriptors.media`, the pinned `descriptors::MEDIA_PORTS` scan —
//!    Jellyfin, DLNA) and, when it exports the shared `/mnt/mesh-storage`
//!    mount, its mesh file share (`descriptors.mesh_fs`). No second
//!    advertisement channel is minted — this is §6 glue over the existing
//!    plane, reusing the same `read_peers` fold `mesh_media.rs` /
//!    `desktop_sources.rs` read.
//! 2. **mDNS (LAN).** Media servers advertised on the local LAN
//!    (`_jellyfin._tcp`) browsed with the SAME `mdns-sd` machinery the
//!    `mdns_relay` worker uses — including its anti-loop `mde-relay-origin`
//!    TXT guard, so a peer-republished service never double-counts against the
//!    mesh-registry lane.
//!
//! **Reachability is derived, never probed** (the load-bearing lock): peer
//! sources fold the roster's health + staleness verdict, and mDNS entries are
//! live-by-presence (the daemon's TTL expiry removes them). There is NO
//! synchronous connect — a dead peer never blocks the tick.
//!
//! **Honest scope (§7).** The lanes the acceptance names as "via mDNS + the
//! mesh registry" are implemented in full; DLNA/UPnP that a mesh node runs is
//! discovered through the registry lane (its `descriptors.media` "dlna" row).
//! Pure-LAN, non-mesh DLNA discovery is SSDP (multicast 239.255.255.250:1900),
//! a different mechanism than mDNS — it is honestly OUT of this worker's mDNS
//! reach and is surfaced as a `gated:` note on the mDNS lane rather than
//! faked. The music-only services in the port scan (`navidrome-airsonic`,
//! `mpd`) are mde-music's domain, not the media player's, so
//! [`media_kind_from_service`] returns `None` for them and they never appear
//! as a media-player source.

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::peers::{peers_dir, read_peers, PeerRecord};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use super::{ShutdownToken, Worker};

/// The retained-latest state topic the merged source roster is published to.
/// The Sources panel (MEDIA-8) reads the newest record off this topic.
pub const MEDIA_SOURCES_TOPIC: &str = "state/media/sources";

/// Republish cadence. Discovery is human-paced; a 2 s poll keeps a newly
/// discovered source visible without spinning the peers plane.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(2);

/// Republish heartbeat.
///
/// Between heartbeats the roster publishes only when the fold changed; once
/// elapsed it republishes unconditionally so a late subscriber /
/// freshly-pruned topic still finds a recent record (mirrors
/// `desktop_sources`' publish gating).
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(30);

/// A peer record older than this is treated as gone (belt-and-braces over the
/// health-reconciler's `health` field, which is the primary authority).
pub const PEER_STALE_MS: u64 = 10 * 60 * 1000;

/// The `descriptors.media` service name a Jellyfin instance registers under
/// (the `descriptors::MEDIA_PORTS` scan tag). Matching the pinned constant
/// keeps the producer + this consumer speaking the same token.
pub const SERVICE_JELLYFIN: &str = "jellyfin";

/// The `descriptors.media` service name a DLNA/UPnP media server registers
/// under (the `descriptors::MEDIA_PORTS` scan tag).
pub const SERVICE_DLNA: &str = "dlna";

/// The `descriptors.media` service name this-player-as-server (MEDIA-15's mesh
/// media server) registers under. Pinning it here so MEDIA-15's producer and
/// MEDIA-14's discovery agree byte-for-byte; today a source appears ONLY when
/// a peer genuinely advertises it (no fabricated `MeshPlayer` row).
pub const SERVICE_MESH_PLAYER: &str = "mde-media";

/// The mDNS service types the media lane browses. Jellyfin advertises
/// `_jellyfin._tcp` on the LAN; the value maps the bare type onto its
/// [`MediaKind`]. (DLNA/UPnP is SSDP, not mDNS — see the module docs.)
pub const MEDIA_MDNS_TYPES: &[(&str, MediaKind)] = &[("_jellyfin._tcp", MediaKind::Jellyfin)];

// ───────────────────────────── data model ─────────────────────────────

/// The kind of media source, as the acceptance enumerates them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    /// A Jellyfin media server (the MEDIA-9 client's target).
    Jellyfin,
    /// A DLNA/UPnP media server.
    Dlna,
    /// This-player-as-server — a peer running the MEDIA-15 mesh media server.
    MeshPlayer,
    /// A mesh file share (`/mnt/mesh-storage`) browsable for media.
    FileShare,
}

impl MediaKind {
    /// Stable wire/log tag.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Jellyfin => "jellyfin",
            Self::Dlna => "dlna",
            Self::MeshPlayer => "mesh_player",
            Self::FileShare => "file_share",
        }
    }
}

/// Map an advertised `descriptors.media` service name onto a media-player
/// source kind. `None` for a service that isn't one of the player's sources
/// (the music-only `navidrome-airsonic` / `mpd` rows are mde-music's domain,
/// not a Jellyfin-class media source), so it is honestly excluded rather than
/// mis-typed.
#[must_use]
pub fn media_kind_from_service(name: &str) -> Option<MediaKind> {
    match name.trim().to_ascii_lowercase().as_str() {
        SERVICE_JELLYFIN => Some(MediaKind::Jellyfin),
        SERVICE_DLNA => Some(MediaKind::Dlna),
        SERVICE_MESH_PLAYER => Some(MediaKind::MeshPlayer),
        _ => None,
    }
}

/// A protocol a media source is reached over.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MediaProtocol {
    /// The Jellyfin REST API (HTTP).
    Jellyfin,
    /// DLNA/UPnP (SOAP over HTTP).
    Dlna,
    /// A plain HTTP media endpoint (the mesh media server).
    Http,
    /// Browse files over the mesh sshfs mount (FILEMGR-5 mesh-mount).
    MeshFs,
}

impl MediaProtocol {
    /// Stable wire/log tag.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Jellyfin => "jellyfin",
            Self::Dlna => "dlna",
            Self::Http => "http",
            Self::MeshFs => "mesh-fs",
        }
    }

    /// The natural protocol set a source of this kind is dialed over.
    #[must_use]
    pub fn for_kind(kind: MediaKind) -> Vec<Self> {
        match kind {
            MediaKind::Jellyfin => vec![Self::Jellyfin],
            MediaKind::Dlna => vec![Self::Dlna],
            MediaKind::MeshPlayer => vec![Self::Http],
            MediaKind::FileShare => vec![Self::MeshFs],
        }
    }
}

/// Derived (never live-probed) reachability of a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reachability {
    /// Roster state says the source should answer.
    Reachable,
    /// Roster state says it won't (the pip greys with `reason`).
    Unreachable,
    /// Nothing derivable — honest.
    Unknown,
}

/// Which discovery lane produced a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceOrigin {
    /// Peer-advertised via the replicated peers plane.
    MeshPeer,
    /// Discovered on the local LAN via mDNS.
    Mdns,
}

/// One merged media source — a row of the published roster.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MediaSource {
    /// Stable id (`jellyfin:<node>:<port>` / `dlna:<node>:<port>` /
    /// `mesh-player:<node>:<port>` / `file-share:<node>` /
    /// `mdns:<host>:<port>:<kind>`).
    pub id: String,
    /// Display name for the Sources panel.
    pub name: String,
    /// The node/host the panel groups by.
    pub node: String,
    /// The kind of media source.
    pub kind: MediaKind,
    /// The address a client dials (overlay IP / `<node>.mesh` / LAN address).
    pub host: String,
    /// The advertised/known port, if any (`None` for a file share).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// The dialable locator (e.g. `http://<host>:<port>`, `mesh-fs://<host>`).
    pub endpoint: String,
    /// Protocols the source is reached over, deduped + sorted.
    pub protocols: Vec<MediaProtocol>,
    /// The discovery lane this source came from.
    pub origin: SourceOrigin,
    /// Derived reachability (never a blocking probe).
    pub reachability: Reachability,
    /// Human-readable reason when not reachable (the greyed pip's caption).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// ─────────────────── lane 1: mesh-registry (peer-advertised) ───────────────────

/// Derive a peer's reachability from its roster row — the health-reconciler's
/// `health` verdict (the primary authority) plus a staleness belt-and-braces.
/// Pure; never a probe.
#[must_use]
pub fn peer_reachability(health: &str, stale: bool) -> (Reachability, Option<String>) {
    if stale {
        return (
            Reachability::Unreachable,
            Some("peer heartbeat stale".to_string()),
        );
    }
    match health {
        "unreachable" => (
            Reachability::Unreachable,
            Some("peer unreachable".to_string()),
        ),
        // A degraded/critical peer still answers on the network — the media
        // service may well serve; only a hard unreachable greys the pip.
        "healthy" | "degraded" | "critical" => (Reachability::Reachable, None),
        _ => (Reachability::Unknown, None),
    }
}

/// The address clients dial for a peer: its overlay IP, else `<node>.mesh`.
#[must_use]
fn peer_host(rec: &PeerRecord) -> String {
    rec.overlay_ip
        .clone()
        .unwrap_or_else(|| format!("{}.{}", rec.hostname, super::mesh_dns::MESH_SUFFIX))
}

/// Build the dialable locator for a media source.
#[must_use]
fn endpoint_for(kind: MediaKind, host: &str, port: Option<u16>) -> String {
    match kind {
        MediaKind::FileShare => format!("mesh-fs://{host}"),
        MediaKind::Jellyfin | MediaKind::Dlna | MediaKind::MeshPlayer => port.map_or_else(
            || format!("http://{host}"),
            |p| format!("http://{host}:{p}"),
        ),
    }
}

/// Lift the media sources out of one peer's published record.
///
/// Yields each player-relevant `descriptors.media` service (Jellyfin / DLNA /
/// the mesh media server — music-only rows are skipped) plus, when the peer
/// exports the shared mesh mount, its file share. The local node's own record
/// is skipped — a node's own media is local, not a mesh source to itself.
#[must_use]
pub fn media_sources_from_peer(rec: &PeerRecord, self_node: &str) -> Vec<MediaSource> {
    if rec.hostname.eq_ignore_ascii_case(self_node) {
        return Vec::new();
    }
    let Some(desc) = rec.descriptors.as_ref() else {
        return Vec::new(); // a pre-PD-2 writer advertises nothing
    };
    let host = peer_host(rec);
    let (reachability, reason) = peer_reachability(&rec.health, rec.is_stale(PEER_STALE_MS));

    let mut out = Vec::new();
    for svc in &desc.media {
        let Some(kind) = media_kind_from_service(&svc.name) else {
            continue; // music-only / unknown service — not a player source
        };
        let id = format!("{}:{}:{}", kind.tag(), rec.hostname, svc.port);
        out.push(MediaSource {
            id,
            name: format!("{} on {}", kind_label(kind), rec.hostname),
            node: rec.hostname.clone(),
            kind,
            host: host.clone(),
            port: Some(svc.port),
            endpoint: endpoint_for(kind, &host, Some(svc.port)),
            protocols: MediaProtocol::for_kind(kind),
            origin: SourceOrigin::MeshPeer,
            reachability,
            reason: reason.clone(),
        });
    }
    // A peer that exports the shared /mnt/mesh-storage mount offers a mesh
    // file share the player can browse for media.
    if desc.mesh_fs.present {
        out.push(MediaSource {
            id: format!("file-share:{}", rec.hostname),
            name: format!("Mesh files on {}", rec.hostname),
            node: rec.hostname.clone(),
            kind: MediaKind::FileShare,
            host: host.clone(),
            port: None,
            endpoint: endpoint_for(MediaKind::FileShare, &host, None),
            protocols: MediaProtocol::for_kind(MediaKind::FileShare),
            origin: SourceOrigin::MeshPeer,
            reachability,
            reason,
        });
    }
    out
}

/// A human display label for a kind (used in the auto-generated source name).
#[must_use]
const fn kind_label(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Jellyfin => "Jellyfin",
        MediaKind::Dlna => "DLNA",
        MediaKind::MeshPlayer => "Mesh media",
        MediaKind::FileShare => "Mesh files",
    }
}

// ───────────────────────── lane 2: mDNS (LAN) ─────────────────────────

/// One mDNS-discovered media endpoint on the local LAN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdnsEndpoint {
    /// The mDNS fullname (the daemon's removal key).
    pub fullname: String,
    /// Instance name (e.g. `Jellyfin Media Server`).
    pub instance: String,
    /// Resolved address (deterministically the lowest IPv4, else IPv6).
    pub host: String,
    /// Advertised port.
    pub port: u16,
    /// The media kind the service type maps to.
    pub kind: MediaKind,
}

/// Map a bare mDNS service type onto its media kind (`None` for a non-media
/// type).
#[must_use]
pub fn media_kind_from_mdns_type(bare: &str) -> Option<MediaKind> {
    MEDIA_MDNS_TYPES
        .iter()
        .find(|(t, _)| *t == bare)
        .map(|(_, k)| *k)
}

/// Lift a resolved mDNS service into an endpoint.
///
/// `None` when it isn't a media type, carries the `mdns_relay` anti-loop
/// origin TXT (a service a mesh peer republished — the registry lane already
/// carries that peer), or resolved no address.
#[must_use]
pub fn endpoint_from_service_info(bare: &str, info: &ServiceInfo) -> Option<MdnsEndpoint> {
    let kind = media_kind_from_mdns_type(bare)?;
    if info
        .get_property_val_str(super::mdns_relay::RELAY_ORIGIN_TXT)
        .is_some()
    {
        return None;
    }
    let mut addrs: Vec<std::net::IpAddr> = info.get_addresses().iter().copied().collect();
    addrs.sort_by_key(|ip| (ip.is_ipv6(), *ip));
    let host = addrs.first()?.to_string();
    Some(MdnsEndpoint {
        fullname: info.get_fullname().to_string(),
        instance: super::mdns_relay::instance_name(info, bare),
        host,
        port: info.get_port(),
        kind,
    })
}

/// Fold an mDNS endpoint into a roster row. Presence in the live mDNS cache
/// IS the reachability signal (the daemon expires dead services) — no probe.
#[must_use]
pub fn source_from_mdns(ep: &MdnsEndpoint) -> MediaSource {
    MediaSource {
        id: format!("mdns:{}:{}:{}", ep.host, ep.port, ep.kind.tag()),
        name: ep.instance.clone(),
        node: ep.host.clone(),
        kind: ep.kind,
        host: ep.host.clone(),
        port: Some(ep.port),
        endpoint: endpoint_for(ep.kind, &ep.host, Some(ep.port)),
        protocols: MediaProtocol::for_kind(ep.kind),
        origin: SourceOrigin::Mdns,
        reachability: Reachability::Reachable,
        reason: None,
    }
}

// ───────────────────────────── the merge fold ─────────────────────────────

/// Fold the two lanes into ONE deduped, stably-ordered source list — the
/// load-bearing merge the acceptance pins. Rules:
///
/// 1. Peer-advertised sources seed the list (the roster is the reachability
///    authority for mesh nodes).
/// 2. An mDNS endpoint that resolves to a source a mesh peer ALREADY advertises
///    (same kind + same host, or its instance name matches a peer node) is
///    deduped away — the registry lane already carries it (latest-per-publisher:
///    the mesh-authoritative row wins). An unknown LAN endpoint becomes its own
///    card.
/// 3. A final union-by-id pass guarantees no duplicate ids survive.
///
/// Output is sorted `(node, name, id)` case-insensitively so the published
/// roster is stable across ticks (grouping by node).
#[must_use]
pub fn merge_media_sources(
    peer_sources: &[MediaSource],
    mdns: &[MdnsEndpoint],
) -> Vec<MediaSource> {
    let mut out: Vec<MediaSource> = peer_sources.to_vec();

    for ep in mdns {
        let already = out.iter().any(|s| {
            s.kind == ep.kind && (s.host == ep.host || s.node.eq_ignore_ascii_case(&ep.instance))
        });
        if !already {
            out.push(source_from_mdns(ep));
        }
    }

    for s in &mut out {
        s.protocols.sort_unstable();
        s.protocols.dedup();
    }
    out.sort_by(|a, b| {
        a.node
            .to_lowercase()
            .cmp(&b.node.to_lowercase())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
    // Union by stable id (belt-and-braces — the fold above dedups mDNS against
    // the registry, this catches any residual id collision).
    out.dedup_by(|a, b| a.id == b.id);
    out
}

// ───────────────────────── the published record ─────────────────────────

/// One discovery lane's honest status (`ok …` / `gated: …`) — so the Sources
/// panel can say WHY a lane is empty instead of silently omitting sources (§7).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaneStatus {
    /// Lane name (`mesh-registry` / `mdns`).
    pub lane: String,
    /// Status string.
    pub status: String,
}

/// The full record published to [`MEDIA_SOURCES_TOPIC`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MediaSourcesState {
    /// Publishing node id.
    pub node: String,
    /// The merged, deduped source roster.
    pub sources: Vec<MediaSource>,
    /// Per-lane discovery status.
    pub lanes: Vec<LaneStatus>,
    /// Wall-clock publish time (ms since the Unix epoch).
    pub published_at_ms: u64,
}

// ───────────────────────────── the worker ─────────────────────────────

/// The live mDNS browse handles. The daemon handle is held for the worker's
/// lifetime (dropping it would tear the browse down).
struct MdnsBrowse {
    _daemon: ServiceDaemon,
    browsers: Vec<(&'static str, mdns_sd::Receiver<ServiceEvent>)>,
}

/// MEDIA-14 — the mesh media-source discovery aggregator worker.
pub struct MediaSourcesWorker {
    /// This node's id (the publish stamp).
    node_id: String,
    /// The replicated workgroup root the peers plane lives under.
    workgroup_root: PathBuf,
    /// Republish cadence.
    tick: Duration,
    /// Unconditional-republish heartbeat.
    heartbeat: Duration,
    /// Bus root override (tests). `None` ⇒ `mde_bus::default_data_dir`.
    bus_root_override: Option<PathBuf>,
    /// Live mDNS endpoints, keyed by fullname (the daemon's removal key).
    mdns_seen: HashMap<String, MdnsEndpoint>,
    /// mDNS lane status for the published record.
    mdns_lane: String,
    /// Fingerprint of the last published fold (publish-on-change gate).
    last_fingerprint: Option<String>,
}

impl MediaSourcesWorker {
    /// Construct with production seams + the default cadences. `node_id` stamps
    /// the publish; `workgroup_root` locates the peers plane.
    #[must_use]
    pub fn new(node_id: String, workgroup_root: PathBuf) -> Self {
        Self {
            node_id,
            workgroup_root,
            tick: DEFAULT_TICK_INTERVAL,
            heartbeat: PUBLISH_HEARTBEAT,
            bus_root_override: None,
            mdns_seen: HashMap::new(),
            mdns_lane: "idle".to_string(),
            last_fingerprint: None,
        }
    }

    /// Override the Bus spool root (tests).
    #[must_use]
    pub fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    /// Override the tick cadence (tests avoid multi-second waits).
    #[must_use]
    pub const fn with_tick(mut self, d: Duration) -> Self {
        self.tick = d;
        self
    }

    /// Drain pending mDNS browse events into the live endpoint cache.
    fn drain_mdns(&mut self, browse: Option<&MdnsBrowse>) -> bool {
        let Some(browse) = browse else { return false };
        let mut changed = false;
        for (bare, rx) in &browse.browsers {
            while let Ok(event) = rx.try_recv() {
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        if let Some(ep) = endpoint_from_service_info(bare, &info) {
                            let prev = self.mdns_seen.insert(ep.fullname.clone(), ep.clone());
                            if prev.as_ref() != Some(&ep) {
                                changed = true;
                            }
                        }
                    }
                    ServiceEvent::ServiceRemoved(_ty, fullname) => {
                        changed |= self.mdns_seen.remove(&fullname).is_some();
                    }
                    _ => {}
                }
            }
        }
        changed
    }

    /// Read the peers plane + fold both lanes into the merged roster.
    fn collect_sources(&self) -> Vec<MediaSource> {
        let peers = read_peers(&peers_dir(&self.workgroup_root));
        let mut peer_sources = Vec::new();
        for rec in &peers {
            peer_sources.extend(media_sources_from_peer(rec, &self.node_id));
        }
        let mut mdns: Vec<MdnsEndpoint> = self.mdns_seen.values().cloned().collect();
        mdns.sort_by(|a, b| a.fullname.cmp(&b.fullname));
        merge_media_sources(&peer_sources, &mdns)
    }

    fn lanes(&self) -> Vec<LaneStatus> {
        vec![
            LaneStatus {
                lane: "mesh-registry".to_string(),
                status: "ok".to_string(),
            },
            LaneStatus {
                lane: "mdns".to_string(),
                status: self.mdns_lane.clone(),
            },
        ]
    }

    /// Publish the roster when the fold changed (or `force`). Returns whether
    /// a record was written.
    fn publish(&mut self, persist: &Persist, sources: Vec<MediaSource>, force: bool) -> bool {
        let lanes = self.lanes();
        let fingerprint = serde_json::to_string(&(&sources, &lanes)).unwrap_or_default();
        if !force && self.last_fingerprint.as_deref() == Some(fingerprint.as_str()) {
            return false;
        }
        let state = MediaSourcesState {
            node: self.node_id.clone(),
            sources,
            lanes,
            published_at_ms: now_ms(),
        };
        let Ok(body) = serde_json::to_string(&state) else {
            return false;
        };
        if let Err(e) = persist.write(MEDIA_SOURCES_TOPIC, Priority::Default, None, Some(&body)) {
            tracing::warn!(target: "mackesd::media_sources", error = %e, "sources publish failed");
            return false;
        }
        self.last_fingerprint = Some(fingerprint);
        true
    }

    /// Start the media-type mDNS browsers (graceful degrade: no daemon / no
    /// multicast interface → an honest `gated:` lane, worker keeps aggregating
    /// the registry lane).
    fn start_mdns_browsers(&mut self) -> Option<MdnsBrowse> {
        let daemon = match ServiceDaemon::new() {
            Ok(d) => d,
            Err(e) => {
                self.mdns_lane = format!("gated: no mDNS daemon ({e})");
                return None;
            }
        };
        let mut browsers = Vec::new();
        for (bare, _kind) in MEDIA_MDNS_TYPES {
            match daemon.browse(&super::mdns_relay::browse_type(bare)) {
                Ok(rx) => browsers.push((*bare, rx)),
                Err(e) => {
                    tracing::warn!(target: "mackesd::media_sources", service_type = bare, error = %e, "mdns browse failed");
                }
            }
        }
        if browsers.is_empty() {
            self.mdns_lane = "gated: no mDNS browse".to_string();
            return None;
        }
        self.mdns_lane = format!("ok ({} types)", browsers.len());
        Some(MdnsBrowse {
            _daemon: daemon,
            browsers,
        })
    }
}

#[async_trait::async_trait]
impl Worker for MediaSourcesWorker {
    fn name(&self) -> &'static str {
        "media_sources"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self
            .bus_root_override
            .clone()
            .or_else(mde_bus::default_data_dir)
        else {
            tracing::debug!(target: "mackesd::media_sources", "no bus root; worker idle");
            return Ok(());
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(target: "mackesd::media_sources", error = %e, "persist open failed; worker idle");
                return Ok(());
            }
        };
        let browse = self.start_mdns_browsers();

        // Immediate first publish so the Sources panel doesn't wait a heartbeat.
        let sources = self.collect_sources();
        self.publish(&persist, sources, true);
        let mut last_pub = Instant::now();

        let mut tick = tokio::time::interval(self.tick);
        tick.tick().await; // burn the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let mdns_changed = self.drain_mdns(browse.as_ref());
                    let due = last_pub.elapsed() >= self.heartbeat;
                    if mdns_changed || due {
                        let sources = self.collect_sources();
                        // A heartbeat republishes unconditionally (late
                        // subscribers); otherwise only on change.
                        if self.publish(&persist, sources, due) {
                            last_pub = Instant::now();
                        }
                    }
                }
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

/// Wall-clock epoch millis for the published record.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::peers::{MediaService, MeshFsUsage, ServiceDescriptors};

    fn peer(
        hostname: &str,
        health: &str,
        overlay_ip: Option<&str>,
        media: Vec<(&str, u16)>,
        mesh_fs: bool,
    ) -> PeerRecord {
        let mut rec = PeerRecord::now(hostname, Some("12.0.0".into()), health);
        rec.overlay_ip = overlay_ip.map(str::to_string);
        rec.descriptors = Some(ServiceDescriptors {
            media: media
                .into_iter()
                .map(|(name, port)| MediaService {
                    name: name.to_string(),
                    port,
                })
                .collect(),
            mesh_fs: MeshFsUsage {
                present: mesh_fs,
                used_bytes: if mesh_fs { 1024 } else { 0 },
                avail_bytes: if mesh_fs { 4096 } else { 0 },
            },
            ..ServiceDescriptors::default()
        });
        rec
    }

    // ── kind mapping ──

    #[test]
    fn service_name_maps_only_player_relevant_kinds() {
        assert_eq!(
            media_kind_from_service("jellyfin"),
            Some(MediaKind::Jellyfin)
        );
        assert_eq!(media_kind_from_service("dlna"), Some(MediaKind::Dlna));
        assert_eq!(
            media_kind_from_service("mde-media"),
            Some(MediaKind::MeshPlayer)
        );
        // case/space tolerant
        assert_eq!(
            media_kind_from_service("  Jellyfin "),
            Some(MediaKind::Jellyfin)
        );
        // music-only services are mde-music's domain, NOT a player source.
        assert_eq!(media_kind_from_service("navidrome-airsonic"), None);
        assert_eq!(media_kind_from_service("mpd"), None);
        assert_eq!(media_kind_from_service("plex"), None);
    }

    // ── lane 1: the peer-advertised fold ──

    #[test]
    fn peer_fold_lifts_jellyfin_dlna_and_file_share() {
        let rec = peer(
            "oak",
            "healthy",
            Some("10.42.0.7"),
            vec![("jellyfin", 8096), ("dlna", 8200), ("mpd", 6600)],
            true,
        );
        let out = media_sources_from_peer(&rec, "elm");
        // jellyfin + dlna + file-share (mpd skipped — music-only); 3 total.
        assert_eq!(out.len(), 3);

        let jf = out.iter().find(|s| s.kind == MediaKind::Jellyfin).unwrap();
        assert_eq!(jf.id, "jellyfin:oak:8096");
        assert_eq!(jf.node, "oak");
        assert_eq!(jf.host, "10.42.0.7");
        assert_eq!(jf.port, Some(8096));
        assert_eq!(jf.endpoint, "http://10.42.0.7:8096");
        assert_eq!(jf.protocols, vec![MediaProtocol::Jellyfin]);
        assert_eq!(jf.origin, SourceOrigin::MeshPeer);
        assert_eq!(jf.reachability, Reachability::Reachable);

        let dlna = out.iter().find(|s| s.kind == MediaKind::Dlna).unwrap();
        assert_eq!(dlna.id, "dlna:oak:8200");
        assert_eq!(dlna.protocols, vec![MediaProtocol::Dlna]);

        let share = out.iter().find(|s| s.kind == MediaKind::FileShare).unwrap();
        assert_eq!(share.id, "file-share:oak");
        assert_eq!(share.port, None);
        assert_eq!(share.endpoint, "mesh-fs://10.42.0.7");
        assert_eq!(share.protocols, vec![MediaProtocol::MeshFs]);

        // No music-only source leaked in.
        assert!(out.iter().all(|s| s.kind != MediaKind::MeshPlayer));
    }

    #[test]
    fn peer_fold_lifts_the_mesh_player_server() {
        // this-player-as-server (MEDIA-15) advertises the mde-media service.
        let rec = peer(
            "oak",
            "healthy",
            Some("10.42.0.7"),
            vec![("mde-media", 9600)],
            false,
        );
        let out = media_sources_from_peer(&rec, "elm");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, MediaKind::MeshPlayer);
        assert_eq!(out[0].id, "mesh_player:oak:9600");
        assert_eq!(out[0].protocols, vec![MediaProtocol::Http]);
    }

    #[test]
    fn peer_fold_skips_self_and_empty_advertisers() {
        // Own record is skipped — a node's own media is local.
        let own = peer("elm", "healthy", None, vec![("jellyfin", 8096)], true);
        assert!(media_sources_from_peer(&own, "elm").is_empty());
        // A peer with no media + no share advertises nothing.
        let quiet = peer("ash", "healthy", None, vec![], false);
        assert!(media_sources_from_peer(&quiet, "elm").is_empty());
        // A pre-PD-2 writer (no descriptors) advertises nothing.
        let bare = PeerRecord::now("older", None, "healthy");
        assert!(media_sources_from_peer(&bare, "elm").is_empty());
    }

    #[test]
    fn peer_fold_host_falls_back_to_mesh_fqdn() {
        let rec = peer("oak", "healthy", None, vec![("jellyfin", 8096)], false);
        let out = media_sources_from_peer(&rec, "elm");
        assert_eq!(out[0].host, "oak.mesh");
        assert_eq!(out[0].endpoint, "http://oak.mesh:8096");
    }

    #[test]
    fn peer_reachability_derivation_table() {
        assert_eq!(
            peer_reachability("healthy", false),
            (Reachability::Reachable, None)
        );
        assert_eq!(
            peer_reachability("degraded", false),
            (Reachability::Reachable, None)
        );
        let (r, why) = peer_reachability("unreachable", false);
        assert_eq!(r, Reachability::Unreachable);
        assert_eq!(why.as_deref(), Some("peer unreachable"));
        // Staleness wins even over a healthy last word.
        let (r, why) = peer_reachability("healthy", true);
        assert_eq!(r, Reachability::Unreachable);
        assert_eq!(why.as_deref(), Some("peer heartbeat stale"));
        assert_eq!(
            peer_reachability("unknown", false),
            (Reachability::Unknown, None)
        );
    }

    #[test]
    fn stale_peer_sources_grey_with_the_stale_reason() {
        let mut rec = peer(
            "oak",
            "healthy",
            Some("10.42.0.7"),
            vec![("jellyfin", 8096)],
            false,
        );
        rec.last_seen_ms = 1; // ancient
        let out = media_sources_from_peer(&rec, "elm");
        assert_eq!(out[0].reachability, Reachability::Unreachable);
        assert_eq!(out[0].reason.as_deref(), Some("peer heartbeat stale"));
    }

    // ── lane 2: the mDNS fold ──

    fn svc(bare: &str, instance: &str, port: u16, txt: &[(&str, &str)]) -> ServiceInfo {
        ServiceInfo::new(
            &super::super::mdns_relay::browse_type(bare),
            instance,
            &format!("{instance}.local."),
            "192.168.1.60",
            port,
            txt,
        )
        .unwrap()
    }

    #[test]
    fn mdns_fold_lifts_jellyfin() {
        let jf = endpoint_from_service_info(
            "_jellyfin._tcp",
            &svc("_jellyfin._tcp", "Living Room", 8096, &[]),
        )
        .unwrap();
        assert_eq!(jf.kind, MediaKind::Jellyfin);
        assert_eq!(jf.host, "192.168.1.60");
        assert_eq!(jf.port, 8096);
        assert_eq!(jf.instance, "Living Room");

        let src = source_from_mdns(&jf);
        assert_eq!(src.id, "mdns:192.168.1.60:8096:jellyfin");
        assert_eq!(src.origin, SourceOrigin::Mdns);
        assert_eq!(src.reachability, Reachability::Reachable);
        assert_eq!(src.endpoint, "http://192.168.1.60:8096");
    }

    #[test]
    fn mdns_fold_skips_non_media_and_relayed_services() {
        // A non-media type never becomes a source.
        assert!(
            endpoint_from_service_info("_ssh._tcp", &svc("_ssh._tcp", "shell", 22, &[])).is_none()
        );
        // A service a mesh peer republished (mdns_relay's anti-loop TXT) is
        // skipped — the registry lane already carries that peer.
        let relayed = svc(
            "_jellyfin._tcp",
            "Jellyfin-10-42-0-9",
            8096,
            &[(super::super::mdns_relay::RELAY_ORIGIN_TXT, "10.42.0.9")],
        );
        assert!(endpoint_from_service_info("_jellyfin._tcp", &relayed).is_none());
    }

    // ── the merge fold ──

    fn ep(instance: &str, host: &str, port: u16, kind: MediaKind) -> MdnsEndpoint {
        MdnsEndpoint {
            fullname: format!("{instance}._x._tcp.local."),
            instance: instance.into(),
            host: host.into(),
            port,
            kind,
        }
    }

    #[test]
    fn merge_dedups_a_known_peers_mdns_advert() {
        // oak's Jellyfin shows up on the LAN via mDNS at oak's address → the
        // registry (mesh-authoritative) row wins; the mDNS duplicate folds away.
        let peer_srcs = media_sources_from_peer(
            &peer(
                "oak",
                "healthy",
                Some("10.42.0.7"),
                vec![("jellyfin", 8096)],
                false,
            ),
            "elm",
        );
        let merged = merge_media_sources(
            &peer_srcs,
            &[ep("oak", "10.42.0.7", 8096, MediaKind::Jellyfin)],
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, "jellyfin:oak:8096");
        assert_eq!(merged[0].origin, SourceOrigin::MeshPeer);

        // Instance-name match (case-insensitive) also dedups.
        let merged = merge_media_sources(
            &peer_srcs,
            &[ep("OAK", "192.168.9.9", 8096, MediaKind::Jellyfin)],
        );
        assert_eq!(merged.len(), 1, "instance-name match against the peer node");
    }

    #[test]
    fn merge_keeps_an_unknown_lan_endpoint_as_its_own_card() {
        let peer_srcs = media_sources_from_peer(
            &peer(
                "oak",
                "healthy",
                Some("10.42.0.7"),
                vec![("jellyfin", 8096)],
                false,
            ),
            "elm",
        );
        let merged = merge_media_sources(
            &peer_srcs,
            &[ep("Basement", "192.168.1.60", 8096, MediaKind::Jellyfin)],
        );
        assert_eq!(merged.len(), 2);
        let lan = merged
            .iter()
            .find(|s| s.origin == SourceOrigin::Mdns)
            .unwrap();
        assert_eq!(lan.id, "mdns:192.168.1.60:8096:jellyfin");
        assert_eq!(lan.name, "Basement");
        assert_eq!(lan.reachability, Reachability::Reachable);
    }

    #[test]
    fn merge_output_is_stably_ordered_by_node_then_name() {
        let mut peer_srcs = Vec::new();
        peer_srcs.extend(media_sources_from_peer(
            &peer(
                "oak",
                "healthy",
                Some("10.42.0.7"),
                vec![("jellyfin", 8096)],
                false,
            ),
            "elm",
        ));
        peer_srcs.extend(media_sources_from_peer(
            &peer(
                "ash",
                "healthy",
                Some("10.42.0.8"),
                vec![("dlna", 8200)],
                true,
            ),
            "elm",
        ));
        let merged = merge_media_sources(&peer_srcs, &[]);
        let order: Vec<&str> = merged.iter().map(|s| s.id.as_str()).collect();
        // ash's DLNA + ash's file-share sort before oak's Jellyfin (by node).
        assert_eq!(
            order,
            vec!["dlna:ash:8200", "file-share:ash", "jellyfin:oak:8096"]
        );
    }

    #[test]
    fn merge_unions_duplicate_ids() {
        // Two identical peer rows (a defensive double-read) collapse to one.
        let one = media_sources_from_peer(
            &peer(
                "oak",
                "healthy",
                Some("10.42.0.7"),
                vec![("jellyfin", 8096)],
                false,
            ),
            "elm",
        );
        let mut dupd = one.clone();
        dupd.extend(one);
        let merged = merge_media_sources(&dupd, &[]);
        assert_eq!(merged.len(), 1);
    }

    // ── the published record ──

    #[test]
    fn published_state_round_trips() {
        let src = media_sources_from_peer(
            &peer(
                "oak",
                "healthy",
                Some("10.42.0.7"),
                vec![("jellyfin", 8096)],
                false,
            ),
            "elm",
        );
        let state = MediaSourcesState {
            node: "elm".into(),
            sources: src,
            lanes: vec![LaneStatus {
                lane: "mdns".into(),
                status: "ok (1 types)".into(),
            }],
            published_at_ms: 1,
        };
        let body = serde_json::to_string(&state).unwrap();
        assert!(body.contains("\"kind\":\"jellyfin\""));
        assert!(body.contains("\"endpoint\":\"http://10.42.0.7:8096\""));
        let back: MediaSourcesState = serde_json::from_str(&body).unwrap();
        assert_eq!(back, state);
    }

    // ── worker orchestration (no LAN, real peers plane) ──

    fn temp_persist() -> (tempfile::TempDir, Persist) {
        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().to_path_buf()).unwrap();
        (dir, persist)
    }

    fn latest_state(persist: &Persist) -> MediaSourcesState {
        let msgs = persist.list_since(MEDIA_SOURCES_TOPIC, None).unwrap();
        let body = msgs.last().unwrap().body.clone().unwrap();
        serde_json::from_str(&body).unwrap()
    }

    #[test]
    fn collect_sources_folds_the_peers_plane() {
        let wg = tempfile::tempdir().unwrap();
        let pdir = peers_dir(wg.path());
        // A peer serving Jellyfin + a mesh share, plus our own record (skipped).
        mackes_mesh_types::peers::write_peer_record(
            &pdir,
            &peer(
                "oak",
                "healthy",
                Some("10.42.0.7"),
                vec![("jellyfin", 8096)],
                true,
            ),
        )
        .unwrap();
        mackes_mesh_types::peers::write_peer_record(
            &pdir,
            &peer(
                "elm",
                "healthy",
                Some("10.42.0.2"),
                vec![("jellyfin", 8096)],
                true,
            ),
        )
        .unwrap();
        let w = MediaSourcesWorker::new("elm".to_string(), wg.path().to_path_buf());
        let sources = w.collect_sources();
        let ids: Vec<&str> = sources.iter().map(|s| s.id.as_str()).collect();
        // Only oak's sources (own record skipped); sorted by display name —
        // "Jellyfin on oak" before "Mesh files on oak".
        assert_eq!(ids, vec!["jellyfin:oak:8096", "file-share:oak"]);
    }

    #[test]
    fn publish_gates_on_change_and_forces_on_heartbeat() {
        let (_bus, persist) = temp_persist();
        let wg = tempfile::tempdir().unwrap();
        mackes_mesh_types::peers::write_peer_record(
            &peers_dir(wg.path()),
            &peer(
                "oak",
                "healthy",
                Some("10.42.0.7"),
                vec![("jellyfin", 8096)],
                false,
            ),
        )
        .unwrap();
        let mut w = MediaSourcesWorker::new("elm".to_string(), wg.path().to_path_buf());
        let sources = w.collect_sources();
        // publish-on-change: first writes, an identical fold doesn't, a forced
        // (heartbeat) one does.
        assert!(w.publish(&persist, sources.clone(), false));
        assert!(!w.publish(&persist, sources.clone(), false));
        assert!(w.publish(&persist, sources, true));

        let state = latest_state(&persist);
        assert_eq!(state.node, "elm");
        assert_eq!(state.sources.len(), 1);
        assert_eq!(state.sources[0].id, "jellyfin:oak:8096");
        assert_eq!(state.lanes.len(), 2);
    }
}
