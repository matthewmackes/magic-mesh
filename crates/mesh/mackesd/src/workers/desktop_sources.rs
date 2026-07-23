//! CHOOSER-1 — the mackesd **desktop-source discovery aggregator**.
//!
//! Design: `docs/design/desktop-chooser.md` (§Architecture, locks 5/14). The
//! Chooser surface (CHOOSER-2, `mde-shell-egui`) renders ONE list of every
//! discovered desktop source; this worker is the mesh-side (§6) collector
//! that builds it. Four discovery lanes, folded into one deduped roster
//! published to [`SOURCES_TOPIC`] (`state/desktops/sources`):
//!
//! 1. **Mesh registry (peer-advertised).** Every node ALREADY advertises what
//!    desktops it serves through the replicated peers plane
//!    (`mackes_mesh_types::peers::PeerRecord`, PD-2): its own seat's RDP/VNC
//!    listeners (`descriptors.remote_access`) and the VM desktops it hosts
//!    (`descriptors.vms`). The small advertised shape is
//!    [`AdvertisedDesktop`]; the pure fold [`advertised_from_peer`] lifts it
//!    from a peer's published record — no second advertisement channel is
//!    minted (§6 glue over the existing plane).
//! 2. **mDNS (LAN).** RDP (`_rdp._tcp`), VNC (`_rfb._tcp`) and Spice
//!    (`_spice._tcp`) endpoints browsed with the SAME `mdns-sd` machinery the
//!    `mdns_relay` worker uses — including its anti-loop `mde-relay-origin`
//!    TXT guard, so a peer-republished service never double-counts against
//!    the mesh-registry lane.
//! 3. **Local KVM.** This node's libvirt guests via the MV-3
//!    [`super::vm_lifecycle::LibvirtBackend`] seam (`virsh list --all`,
//!    bounded by the EFF-20 proc timeout). Every defined VM is a source (its
//!    console is Spice per MV-3's domain XML), carrying its live power state.
//! 4. **Manual.** Operator-added `host:port` + protocol endpoints, drained
//!    off the typed `action/desktops/{add-source,remove-source}` verbs (§9 —
//!    a typed body, never a command string) and persisted node-locally;
//!    `action/desktops/refresh` forces a re-enumerate + republish.
//!
//! **Reachability is derived, never probed** (lock 14): peer sources fold
//! roster presence + health, VM sources fold power state, mDNS entries are
//! live-by-presence (the daemon's TTL expiry removes them), and manual
//! entries are honestly `Unknown`. **Live KVM enumeration is honestly gated**
//! (§7, mirroring `mesh_mount`): no `virsh` on the box → a typed
//! [`VmEnumerateError::Gated`], surfaced in the published per-lane status —
//! never a faked (or silently missing) source. The `thumbnail_ref` field
//! ships now, honestly empty (`null`), for CHOOSER-3 to fill.

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::peers::{peers_dir, read_peers, PeerRecord};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use super::vm_lifecycle::{vm_state_from_str, Instance, LibvirtBackend, VirshCli, VmState};
use super::{ShutdownToken, Worker};
use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};

/// The retained-latest state topic the merged source roster is published to.
/// The Chooser surface (CHOOSER-2) reads the newest record off this topic.
pub const SOURCES_TOPIC: &str = "state/desktops/sources";

/// Typed verb: add a manual desktop source (`action/<domain>/<verb>`, §9).
pub const ADD_SOURCE_TOPIC: &str = "action/desktops/add-source";

/// Typed verb: remove a previously-added manual source by its id.
pub const REMOVE_SOURCE_TOPIC: &str = "action/desktops/remove-source";

/// Typed verb: force a re-enumerate + republish (the operator's refresh).
pub const REFRESH_TOPIC: &str = "action/desktops/refresh";

/// Shared-Bus capability verbs for the two manual-source mutations. Refresh is
/// intentionally not listed: it only re-enumerates read-only discovery lanes
/// and republishes the derived roster, so it remains an open harmless nudge.
const DESKTOP_ADD_SOURCE_AUTH_VERB: &str = "desktop-add-source";
const DESKTOP_REMOVE_SOURCE_AUTH_VERB: &str = "desktop-remove-source";

/// Action-drain cadence. Discovery is human-paced; a 2 s poll keeps verb
/// latency imperceptible without spinning virsh or the peers plane.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(2);

/// Republish heartbeat.
///
/// Between heartbeats the roster publishes only when the fold changed; once
/// elapsed it republishes unconditionally so a late subscriber /
/// freshly-pruned topic still finds a recent record (mirrors
/// `vm_lifecycle`'s publish gating).
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(30);

/// A peer record older than this is treated as gone (belt-and-braces over the
/// health-reconciler's `health` field, which is the primary authority).
pub const PEER_STALE_MS: u64 = 10 * 60 * 1000;

/// The mDNS service types the desktop lanes browse (design lock 5): RDP,
/// VNC (`_rfb` is the RFB protocol's registered type), and Spice.
pub const DESKTOP_MDNS_TYPES: &[&str] = &["_rdp._tcp", "_rfb._tcp", "_spice._tcp"];

/// Filename of the node-local manual-source store (under the store root).
/// CHOOSER-9 later lifts manual sources onto the mesh-synced plane; the
/// node-local file keeps them durable across restarts today.
pub const MANUAL_STORE_FILE: &str = "manual-sources.json";

// ───────────────────────────── data model ─────────────────────────────

/// A desktop-session protocol a source can be connected over.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum DesktopProtocol {
    /// Remote Desktop Protocol (`mde-vdi-rdp`).
    Rdp,
    /// VNC / RFB (`mde-vdi-vnc`).
    Vnc,
    /// Spice (`mde-vdi-spice`, CHOOSER-5).
    Spice,
}

impl DesktopProtocol {
    /// Stable wire/log tag.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Rdp => "rdp",
            Self::Vnc => "vnc",
            Self::Spice => "spice",
        }
    }

    /// The protocol's well-known default port, when one exists. Spice has no
    /// canonical default (libvirt autoports it), so it is honestly `None` —
    /// a Spice endpoint's port must come from discovery or the operator.
    #[must_use]
    pub const fn default_port(self) -> Option<u16> {
        match self {
            Self::Rdp => Some(3389),
            Self::Vnc => Some(5900),
            Self::Spice => None,
        }
    }

    /// Map a bare mDNS service type onto its desktop protocol (`None` for a
    /// non-desktop type).
    #[must_use]
    pub fn from_mdns_type(bare: &str) -> Option<Self> {
        match bare {
            "_rdp._tcp" => Some(Self::Rdp),
            "_rfb._tcp" => Some(Self::Vnc),
            "_spice._tcp" => Some(Self::Spice),
            _ => None,
        }
    }
}

/// One protocol a source offers, with the port when it is known. A `None`
/// port means the transport is brokered (a local VM's Spice console) or
/// defaulted at connect time — never a guessed number.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct ProtocolOffer {
    /// The protocol.
    pub protocol: DesktopProtocol,
    /// The advertised/known port, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

impl ProtocolOffer {
    /// Construct an offer.
    #[must_use]
    pub const fn new(protocol: DesktopProtocol, port: Option<u16>) -> Self {
        Self { protocol, port }
    }
}

/// Derived (never live-probed — lock 14) reachability of a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reachability {
    /// Roster/VM state says the source should answer.
    Reachable,
    /// Roster/VM state says it won't (the card greys with `reason`).
    Unreachable,
    /// Nothing derivable (a manual endpoint is never probed) — honest.
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
    /// A local libvirt/KVM guest console.
    LocalVm,
    /// Operator-added.
    Manual,
}

/// One merged desktop source — a row of the published roster.
///
/// The per-source shape the CHOOSER-1 acceptance pins: id, display name,
/// node/host, protocols offered, derived reachability (+ a human reason when
/// greyed), OS hint when genuinely known, power state for VMs, and the
/// thumbnail ref CHOOSER-3 will fill (always serialized; honestly `null`
/// today).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DesktopSource {
    /// Stable id (`peer:<node>` / `peer-vm:<node>:<vm>` / `vm:<node>:<name>`
    /// / `mdns:<host>:<port>:<proto>` / `manual:<host>:<port>:<proto>`).
    pub id: String,
    /// Display name for the card.
    pub name: String,
    /// The node/host the Chooser groups by (design lock 3).
    pub node: String,
    /// The address a client connects to (overlay IP / `<host>.mesh` / LAN
    /// address); for a local VM the serving node (the console is brokered).
    pub host: String,
    /// Protocols offered, deduped + sorted.
    pub protocols: Vec<ProtocolOffer>,
    /// The discovery lane this source came from.
    pub origin: SourceOrigin,
    /// Derived reachability (lock 14 — never a blocking probe).
    pub reachability: Reachability,
    /// Human-readable reason when not reachable (the greyed card's caption).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// OS hint when genuinely known (a mesh peer's seat is an MCNF Linux
    /// desktop); `None` rather than a guess.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_hint: Option<String>,
    /// Live power state for VM sources (`running` / `shut off` / …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_state: Option<String>,
    /// Thumbnail reference — the key CHOOSER-3 fills with periodic previews.
    /// ALWAYS serialized (no skip) so consumers see the field; honestly
    /// `null` until a thumbnail pipeline exists.
    pub thumbnail_ref: Option<String>,
}

// ─────────────────── lane 1: mesh-registry (peer-advertised) ───────────────────

/// The small peer-advertised desktop shape.
///
/// What one node's published state says it serves — lifted from the peer's
/// replicated [`PeerRecord`] by [`advertised_from_peer`]: the node's own
/// seat (its RDP/VNC listeners, `vm == None`) and each VM desktop it hosts
/// (`vm == Some(name)`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AdvertisedDesktop {
    /// The advertising node's hostname.
    pub node: String,
    /// The address clients dial (overlay IP, else `<node>.mesh`).
    pub host: String,
    /// `None` = the node's own seat desktop; `Some(name)` = a VM it serves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vm: Option<String>,
    /// Protocols the desktop is served over.
    pub protocols: Vec<ProtocolOffer>,
    /// The VM's advertised power state (`None` for the seat).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_state: Option<String>,
    /// Derived from roster presence/health (+ VM power state).
    pub reachability: Reachability,
    /// Human reason when not reachable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Derive a peer's reachability from its roster row — the health-reconciler's
/// `health` verdict (the primary authority) plus a staleness belt-and-braces.
/// Pure; never a probe (lock 14).
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
        // A degraded/critical peer still answers on the network — the desktop
        // may well connect; only a hard unreachable greys the card.
        "healthy" | "degraded" | "critical" => (Reachability::Reachable, None),
        _ => (Reachability::Unknown, None),
    }
}

/// Lift the advertised desktops out of one peer's published record.
///
/// Yields the seat (when its RDP/VNC listeners are advertised) + each hosted
/// VM. The local node's own record is skipped — its VMs come from the richer
/// live KVM lane, and its own seat is not a remote desktop to itself.
#[must_use]
pub fn advertised_from_peer(rec: &PeerRecord, self_node: &str) -> Vec<AdvertisedDesktop> {
    if rec.hostname.eq_ignore_ascii_case(self_node) {
        return Vec::new();
    }
    let Some(desc) = rec.descriptors.as_ref() else {
        return Vec::new(); // a pre-PD-2 writer advertises nothing
    };
    let host = rec
        .overlay_ip
        .clone()
        .unwrap_or_else(|| format!("{}.{}", rec.hostname, super::mesh_dns::MESH_SUFFIX));
    let (reachability, reason) = peer_reachability(&rec.health, rec.is_stale(PEER_STALE_MS));

    let mut out = Vec::new();
    let mut seat = Vec::new();
    if desc.remote_access.rdp {
        seat.push(ProtocolOffer::new(
            DesktopProtocol::Rdp,
            DesktopProtocol::Rdp.default_port(),
        ));
    }
    if desc.remote_access.vnc {
        seat.push(ProtocolOffer::new(
            DesktopProtocol::Vnc,
            DesktopProtocol::Vnc.default_port(),
        ));
    }
    if !seat.is_empty() {
        out.push(AdvertisedDesktop {
            node: rec.hostname.clone(),
            host: host.clone(),
            vm: None,
            protocols: seat,
            power_state: None,
            reachability,
            reason: reason.clone(),
        });
    }
    for vm in &desc.vms {
        let live = matches!(
            vm_state_from_str(&vm.state),
            VmState::Running | VmState::Paused
        );
        let (r, why) = if reachability == Reachability::Reachable && !live {
            (
                Reachability::Unreachable,
                Some(format!("vm {}", vm.state.trim())),
            )
        } else {
            (reachability, reason.clone())
        };
        out.push(AdvertisedDesktop {
            node: rec.hostname.clone(),
            host: host.clone(),
            vm: Some(vm.name.clone()),
            // MV-3 domains carry Spice graphics; the console is brokered by
            // the serving peer (E12 VDI), so no port is claimed here.
            protocols: vec![ProtocolOffer::new(DesktopProtocol::Spice, None)],
            power_state: Some(vm.state.clone()),
            reachability: r,
            reason: why,
        });
    }
    out
}

/// Fold one advertised desktop into a roster row.
#[must_use]
pub fn source_from_advertised(ad: &AdvertisedDesktop) -> DesktopSource {
    let (id, name, os_hint) = ad.vm.as_ref().map_or_else(
        || {
            (
                format!("peer:{}", ad.node),
                ad.node.clone(),
                // A mesh peer's seat is an MCNF (Linux) desktop — genuinely
                // known, not a guess.
                Some("linux".to_string()),
            )
        },
        |vm| (format!("peer-vm:{}:{vm}", ad.node), vm.clone(), None),
    );
    DesktopSource {
        id,
        name,
        node: ad.node.clone(),
        host: ad.host.clone(),
        protocols: ad.protocols.clone(),
        origin: SourceOrigin::MeshPeer,
        reachability: ad.reachability,
        reason: ad.reason.clone(),
        os_hint,
        power_state: ad.power_state.clone(),
        thumbnail_ref: None,
    }
}

// ───────────────────────── lane 2: mDNS (LAN) ─────────────────────────

/// One mDNS-discovered desktop endpoint on the local LAN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdnsEndpoint {
    /// The mDNS fullname (the daemon's removal key).
    pub fullname: String,
    /// Instance name (e.g. `Office PC`).
    pub instance: String,
    /// Resolved address (deterministically the lowest IPv4, else IPv6).
    pub host: String,
    /// Advertised port.
    pub port: u16,
    /// The desktop protocol the service type maps to.
    pub protocol: DesktopProtocol,
}

/// Lift a resolved mDNS service into an endpoint.
///
/// `None` when it isn't a desktop type, carries the `mdns_relay` anti-loop
/// origin TXT (a service a mesh peer republished — the registry lane already
/// carries that peer), or resolved no address.
#[must_use]
pub fn endpoint_from_service_info(bare: &str, info: &ServiceInfo) -> Option<MdnsEndpoint> {
    let protocol = DesktopProtocol::from_mdns_type(bare)?;
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
        protocol,
    })
}

/// Fold an mDNS endpoint into a roster row. Presence in the live mDNS cache
/// IS the reachability signal (the daemon expires dead services) — no probe.
#[must_use]
pub fn source_from_mdns(ep: &MdnsEndpoint) -> DesktopSource {
    DesktopSource {
        id: format!("mdns:{}:{}:{}", ep.host, ep.port, ep.protocol.tag()),
        name: ep.instance.clone(),
        node: ep.host.clone(),
        host: ep.host.clone(),
        protocols: vec![ProtocolOffer::new(ep.protocol, Some(ep.port))],
        origin: SourceOrigin::Mdns,
        reachability: Reachability::Reachable,
        reason: None,
        os_hint: None,
        power_state: None,
        thumbnail_ref: None,
    }
}

// ─────────────────────── lane 3: local KVM guests ───────────────────────

/// A typed local-VM enumeration failure — the honest-gate discipline
/// (mirrors `mesh_mount::MountError::Gated`, §7): a box without a hypervisor
/// toolchain refuses cleanly, never fakes a source list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmEnumerateError {
    /// The prerequisites aren't on this box (no `virsh`). The honest
    /// headless/CI gate — surfaced in the published lane status.
    Gated(String),
    /// libvirt answered with an error (surfaced verbatim, no sources).
    Backend(String),
}

impl std::fmt::Display for VmEnumerateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gated(m) => write!(f, "gated: {m}"),
            Self::Backend(m) => write!(f, "error: {m}"),
        }
    }
}

impl std::error::Error for VmEnumerateError {}

/// The injectable local-VM enumeration seam. Production is
/// [`LibvirtEnumerator`] over the MV-3 `VirshCli`; tests inject a fake.
pub trait VmEnumerator: Send + Sync {
    /// This node's defined VMs (every one is a console source), or a typed
    /// gate/error — NEVER a fabricated list.
    ///
    /// # Errors
    /// [`VmEnumerateError::Gated`] on a box without the toolchain;
    /// [`VmEnumerateError::Backend`] when libvirt errors.
    fn enumerate(&self) -> Result<Vec<Instance>, VmEnumerateError>;
}

/// The production enumerator: gates on `virsh` being present, then reuses the
/// MV-3 [`LibvirtBackend`] roster read (`virsh list --all`, EFF-20-bounded).
pub struct LibvirtEnumerator {
    backend: Arc<dyn LibvirtBackend + Send + Sync>,
}

impl Default for LibvirtEnumerator {
    fn default() -> Self {
        Self::new()
    }
}

impl LibvirtEnumerator {
    /// Production wiring over [`VirshCli`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            backend: Arc::new(VirshCli::new()),
        }
    }

    /// Inject a backend (tests drive the gate+fold over `FakeLibvirt`).
    #[must_use]
    pub fn with_backend(backend: Arc<dyn LibvirtBackend + Send + Sync>) -> Self {
        Self { backend }
    }
}

impl VmEnumerator for LibvirtEnumerator {
    fn enumerate(&self) -> Result<Vec<Instance>, VmEnumerateError> {
        // Honest gate FIRST (§7): no virsh on this box → a typed refusal,
        // never a shell-out into a confusing failure.
        if !super::mesh_mount::binary_on_path("virsh") {
            return Err(VmEnumerateError::Gated(
                "virsh not found — no local hypervisor toolchain (node-virt.yml provisions it)"
                    .to_string(),
            ));
        }
        self.backend
            .list()
            .map_err(|e| VmEnumerateError::Backend(e.to_string()))
    }
}

/// Fold one local VM into a roster row.
///
/// The console is a Spice source (MV-3's domain XML gives every guest Spice
/// graphics; brokering the transport is the E12 VDI path, so no port is
/// claimed); reachability derives from the power state (running/paused
/// consoles answer; a shut-off VM greys with its state as the reason —
/// CHOOSER-7 starts it from the card).
#[must_use]
pub fn source_from_vm(node: &str, inst: &Instance) -> DesktopSource {
    let state = inst.state.trim().to_string();
    let live = matches!(
        vm_state_from_str(&state),
        VmState::Running | VmState::Paused
    );
    DesktopSource {
        id: format!("vm:{node}:{}", inst.name),
        name: inst.name.clone(),
        node: node.to_string(),
        host: node.to_string(),
        protocols: vec![ProtocolOffer::new(DesktopProtocol::Spice, None)],
        origin: SourceOrigin::LocalVm,
        reachability: if live {
            Reachability::Reachable
        } else {
            Reachability::Unreachable
        },
        reason: (!live).then(|| format!("vm {state}")),
        os_hint: None,
        power_state: Some(state),
        thumbnail_ref: None,
    }
}

// ─────────────────────── lane 4: manual sources + verbs ───────────────────────

/// One operator-added source — also the typed body of an
/// `action/desktops/add-source` request (§9: host + port + protocol, never a
/// command string).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManualSource {
    /// Optional display name (defaults to `host:port`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Host/IP to connect to.
    pub host: String,
    /// Port to connect to.
    pub port: u16,
    /// The protocol to connect over.
    pub protocol: DesktopProtocol,
}

impl ManualSource {
    /// The stable source id (`manual:<host>:<port>:<proto>`) — also the
    /// remove-source key.
    #[must_use]
    pub fn id(&self) -> String {
        format!("manual:{}:{}:{}", self.host, self.port, self.protocol.tag())
    }

    /// Display name (the operator's, else `host:port`).
    #[must_use]
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("{}:{}", self.host, self.port))
    }
}

/// Fold one manual source into a roster row. A manual endpoint is never
/// probed (lock 14), so its reachability is an honest `Unknown`.
#[must_use]
pub fn source_from_manual(m: &ManualSource) -> DesktopSource {
    DesktopSource {
        id: m.id(),
        name: m.display_name(),
        node: m.host.clone(),
        host: m.host.clone(),
        protocols: vec![ProtocolOffer::new(m.protocol, Some(m.port))],
        origin: SourceOrigin::Manual,
        reachability: Reachability::Unknown,
        reason: None,
        os_hint: None,
        power_state: None,
        thumbnail_ref: None,
    }
}

/// Typed body of an `action/desktops/remove-source` request.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemoveSourceRequest {
    /// The manual source id ([`ManualSource::id`]) to remove.
    pub id: String,
}

/// Parse + validate an add-source body.
///
/// # Errors
/// A human-readable message on malformed JSON, an empty host, or port 0.
pub fn parse_add_source(body: &str) -> Result<ManualSource, String> {
    let req: ManualSource =
        serde_json::from_str(body).map_err(|e| format!("malformed add-source request: {e}"))?;
    if req.host.trim().is_empty() {
        return Err("add-source: host must not be empty".to_string());
    }
    if req.port == 0 {
        return Err("add-source: port must be non-zero".to_string());
    }
    Ok(req)
}

/// Parse + validate a remove-source body.
///
/// # Errors
/// A human-readable message on malformed JSON or an empty id.
pub fn parse_remove_source(body: &str) -> Result<RemoveSourceRequest, String> {
    let req: RemoveSourceRequest =
        serde_json::from_str(body).map_err(|e| format!("malformed remove-source request: {e}"))?;
    if req.id.trim().is_empty() {
        return Err("remove-source: id must not be empty".to_string());
    }
    Ok(req)
}

fn manual_store_path(store_root: &Path) -> PathBuf {
    store_root.join(MANUAL_STORE_FILE)
}

/// Load the node-local manual-source store (absent/corrupt → empty, never
/// fatal — a half-written file must not kill the worker).
#[must_use]
pub fn load_manual_sources(store_root: &Path) -> Vec<ManualSource> {
    std::fs::read_to_string(manual_store_path(store_root))
        .ok()
        .and_then(|data| serde_json::from_str(&data).ok())
        .unwrap_or_default()
}

/// Persist the manual-source store atomically (temp + rename, the peers-plane
/// idiom).
///
/// # Errors
/// Filesystem/serialization failures.
pub fn save_manual_sources(store_root: &Path, sources: &[ManualSource]) -> std::io::Result<()> {
    std::fs::create_dir_all(store_root)?;
    let json = serde_json::to_string_pretty(sources)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = store_root.join(format!(".{MANUAL_STORE_FILE}.tmp"));
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, manual_store_path(store_root))
}

/// Resolve the node-local store root for manual sources
/// (`<XDG_DATA_HOME>/mde/desktops`, or `/var/lib/mde/desktops` headless) —
/// the `bookmarks::resolve_local_root` idiom.
#[must_use]
pub fn resolve_store_root() -> PathBuf {
    dirs::data_dir().map_or_else(
        || PathBuf::from("/var/lib/mde/desktops"),
        |d| d.join("mde").join("desktops"),
    )
}

// ───────────────────────────── the merge fold ─────────────────────────────

/// Fold the four lanes into ONE deduped, stably-ordered source list — the
/// load-bearing merge the acceptance pins. Rules:
///
/// 1. Peer-advertised desktops seed the list (the roster is the reachability
///    authority for mesh nodes).
/// 2. An mDNS endpoint that resolves to a known peer **seat** (same address,
///    or its instance name matches the node) folds its protocol into that
///    card instead of duplicating it; an unknown LAN endpoint becomes its own
///    card.
/// 3. Local VM sources append as-is (unique per `(node, name)` by
///    construction).
/// 4. A manual source whose `(host, port, protocol)` is already offered is
///    deduped away; the rest append with honest `Unknown` reachability.
///
/// Output is sorted `(node, name, id)` case-insensitively so the published
/// roster is stable across ticks (grouping by node — design lock 3).
#[must_use]
pub fn merge_sources(
    advertised: &[AdvertisedDesktop],
    mdns: &[MdnsEndpoint],
    local_vms: &[DesktopSource],
    manual: &[ManualSource],
) -> Vec<DesktopSource> {
    let mut out: Vec<DesktopSource> = advertised.iter().map(source_from_advertised).collect();

    for ep in mdns {
        let seat = out.iter().position(|s| {
            s.id.starts_with("peer:")
                && (s.host == ep.host || s.node.eq_ignore_ascii_case(&ep.instance))
        });
        match seat {
            Some(i) => {
                if !out[i].protocols.iter().any(|p| p.protocol == ep.protocol) {
                    out[i]
                        .protocols
                        .push(ProtocolOffer::new(ep.protocol, Some(ep.port)));
                }
            }
            None => out.push(source_from_mdns(ep)),
        }
    }

    out.extend(local_vms.iter().cloned());

    for m in manual {
        let dup = out.iter().any(|s| {
            s.host == m.host
                && s.protocols
                    .iter()
                    .any(|p| p.protocol == m.protocol && p.port == Some(m.port))
        });
        if !dup {
            out.push(source_from_manual(m));
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
    out
}

// ───────────────────────── the published record ─────────────────────────

/// One discovery lane's honest status (`ok …` / `gated: …` / `error: …`) —
/// so the Chooser can say WHY a lane is empty instead of silently omitting
/// sources (§7).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaneStatus {
    /// Lane name (`mesh-registry` / `mdns` / `local-kvm` / `manual`).
    pub lane: String,
    /// Status string.
    pub status: String,
}

/// The full record published to [`SOURCES_TOPIC`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DesktopSourcesState {
    /// Publishing node id.
    pub node: String,
    /// The merged, deduped source roster.
    pub sources: Vec<DesktopSource>,
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

/// CHOOSER-1 — the desktop-source discovery aggregator worker.
pub struct DesktopSourcesWorker {
    /// This node's id (the publish stamp + the local-VM `node`).
    node_id: String,
    /// The replicated workgroup root the peers plane lives under.
    workgroup_root: PathBuf,
    /// Node-local root the manual-source store persists under.
    store_root: PathBuf,
    /// The injectable local-VM enumeration seam.
    vms: Arc<dyn VmEnumerator>,
    /// Action-drain cadence.
    tick: Duration,
    /// Unconditional-republish heartbeat.
    heartbeat: Duration,
    /// Bus root override (tests). `None` ⇒ `mde_bus::default_data_dir`.
    bus_root_override: Option<PathBuf>,
    /// The manual sources (mirrors the on-disk store).
    manual: Vec<ManualSource>,
    /// Live mDNS endpoints, keyed by fullname (the daemon's removal key).
    mdns_seen: HashMap<String, MdnsEndpoint>,
    /// mDNS lane status for the published record.
    mdns_lane: String,
    /// local-kvm lane status for the published record.
    vm_lane: String,
    /// Per-action-topic drain cursors.
    cursors: HashMap<&'static str, String>,
    /// Fingerprint of the last published fold (publish-on-change gate).
    last_fingerprint: Option<String>,
    /// Shared, fail-closed authorization gate for manual-source mutations.
    authorizer: Arc<ActionAuthorizer>,
}

impl DesktopSourcesWorker {
    /// Construct with production seams: the [`LibvirtEnumerator`] VM lane and
    /// the default cadences. `node_id` stamps the publish; `workgroup_root`
    /// locates the peers plane; `store_root` holds the manual-source store.
    #[must_use]
    pub fn new(node_id: String, workgroup_root: PathBuf, store_root: PathBuf) -> Self {
        Self {
            node_id,
            workgroup_root,
            store_root,
            vms: Arc::new(LibvirtEnumerator::new()),
            tick: DEFAULT_TICK_INTERVAL,
            heartbeat: PUBLISH_HEARTBEAT,
            bus_root_override: None,
            manual: Vec::new(),
            mdns_seen: HashMap::new(),
            mdns_lane: "idle".to_string(),
            vm_lane: "idle".to_string(),
            cursors: HashMap::new(),
            last_fingerprint: None,
            authorizer: Arc::new(ActionAuthorizer::production()),
        }
    }

    /// Inject the VM-enumeration seam (tests).
    #[must_use]
    pub fn with_enumerator(mut self, vms: Arc<dyn VmEnumerator>) -> Self {
        self.vms = vms;
        self
    }

    /// Override the Bus spool root (tests).
    #[must_use]
    pub fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    /// Override the action-drain cadence (tests avoid multi-second waits).
    #[must_use]
    pub const fn with_tick(mut self, d: Duration) -> Self {
        self.tick = d;
        self
    }

    /// Inject an isolated verifier and replay ledger for hostile action tests.
    /// Production always uses the systemd-credential-backed authorizer.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_authorizer(mut self, authorizer: Arc<ActionAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    /// Add a manual source (idempotent on the id). Returns whether the set
    /// changed; a change persists the store.
    fn handle_add(&mut self, req: ManualSource) -> bool {
        if self.manual.iter().any(|m| m.id() == req.id()) {
            return false;
        }
        self.manual.push(req);
        self.persist_manual();
        true
    }

    /// Remove a manual source by id. Only manual sources are removable —
    /// discovered sources reappear by discovery, so removing one would be a
    /// lie; a non-manual id is a logged no-op.
    fn handle_remove(&mut self, id: &str) -> bool {
        let before = self.manual.len();
        self.manual.retain(|m| m.id() != id);
        let changed = self.manual.len() != before;
        if changed {
            self.persist_manual();
        } else {
            tracing::warn!(
                target: "mackesd::desktop_sources",
                id,
                "remove-source: not a manual source id; ignored",
            );
        }
        changed
    }

    fn persist_manual(&self) {
        if let Err(e) = save_manual_sources(&self.store_root, &self.manual) {
            tracing::warn!(
                target: "mackesd::desktop_sources",
                error = %e,
                "manual-source store write failed",
            );
        }
    }

    /// Authenticate one raw manual-source mutation before parsing it into a
    /// typed request or touching the node-local store. Targets are the stable
    /// manual-source id, so a capability cannot be replayed for another
    /// endpoint. The refresh verb deliberately stays outside this helper: it
    /// performs no mutation, only read-only discovery and a derived publish.
    fn authorize_mutation(&self, topic: &'static str, body: &str) -> Result<(), String> {
        let (verb, target) = match topic {
            ADD_SOURCE_TOPIC => {
                let target = parse_add_source(body)
                    .map(|request| request.id())
                    .unwrap_or_default();
                (DESKTOP_ADD_SOURCE_AUTH_VERB, target)
            }
            REMOVE_SOURCE_TOPIC => {
                let target = parse_remove_source(body)
                    .map(|request| request.id)
                    .unwrap_or_default();
                (DESKTOP_REMOVE_SOURCE_AUTH_VERB, target)
            }
            other => return Err(format!("unknown desktop mutation topic: {other}")),
        };
        self.authorizer.authorize(
            body,
            MutationContext {
                verb,
                node: &self.node_id,
                target: &target,
            },
        )
    }

    /// Drain one action topic since its cursor, returning the new bodies.
    fn drain_topic(
        persist: &Persist,
        topic: &'static str,
        cursors: &mut HashMap<&'static str, String>,
    ) -> Vec<String> {
        let cursor = cursors.get(topic).cloned();
        let Ok(msgs) = persist.list_since(topic, cursor.as_deref()) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for msg in msgs {
            cursors.insert(topic, msg.ulid.clone());
            out.push(msg.body.unwrap_or_default());
        }
        out
    }

    /// Drain the three typed verbs. Returns `(manual_changed, refresh)`.
    fn drain_actions(&mut self, persist: &Persist) -> (bool, bool) {
        let mut changed = false;
        for body in Self::drain_topic(persist, ADD_SOURCE_TOPIC, &mut self.cursors) {
            if let Err(error) = self.authorize_mutation(ADD_SOURCE_TOPIC, &body) {
                tracing::warn!(
                    target: "mackesd::desktop_sources",
                    error = %error,
                    "refused unauthorized add-source mutation"
                );
                continue;
            }
            match parse_add_source(&body) {
                Ok(req) => changed |= self.handle_add(req),
                Err(e) => {
                    tracing::warn!(target: "mackesd::desktop_sources", error = %e, "bad add-source");
                }
            }
        }
        for body in Self::drain_topic(persist, REMOVE_SOURCE_TOPIC, &mut self.cursors) {
            if let Err(error) = self.authorize_mutation(REMOVE_SOURCE_TOPIC, &body) {
                tracing::warn!(
                    target: "mackesd::desktop_sources",
                    error = %error,
                    "refused unauthorized remove-source mutation"
                );
                continue;
            }
            match parse_remove_source(&body) {
                Ok(req) => changed |= self.handle_remove(&req.id),
                Err(e) => {
                    tracing::warn!(target: "mackesd::desktop_sources", error = %e, "bad remove-source");
                }
            }
        }
        // Refresh is an open, harmless read/nudge: it only re-enumerates
        // discovery and republishes the derived state; it never updates the
        // manual store or invokes a privileged mutator.
        let refresh = !Self::drain_topic(persist, REFRESH_TOPIC, &mut self.cursors).is_empty();
        (changed, refresh)
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

    /// Fold a VM enumeration outcome into the lane status + the instance
    /// list (an error contributes NO sources — never a fake, §7).
    fn fold_vm_result(&mut self, res: Result<Vec<Instance>, VmEnumerateError>) -> Vec<Instance> {
        match res {
            Ok(list) => {
                self.vm_lane = format!("ok ({} vms)", list.len());
                list
            }
            Err(e) => {
                tracing::debug!(target: "mackesd::desktop_sources", error = %e, "vm enumeration unavailable");
                self.vm_lane = e.to_string();
                Vec::new()
            }
        }
    }

    /// Enumerate local VMs on a blocking thread (virsh shells out).
    async fn enumerate_vms(&mut self) -> Vec<Instance> {
        let vms = Arc::clone(&self.vms);
        let res = match tokio::task::spawn_blocking(move || vms.enumerate()).await {
            Ok(r) => r,
            Err(e) => Err(VmEnumerateError::Backend(format!("enumerate join: {e}"))),
        };
        self.fold_vm_result(res)
    }

    /// Read the peers plane + fold every lane into the merged roster.
    fn collect_sources(&self, vm_list: &[Instance]) -> Vec<DesktopSource> {
        let peers = read_peers(&peers_dir(&self.workgroup_root));
        let mut advertised = Vec::new();
        for rec in &peers {
            advertised.extend(advertised_from_peer(rec, &self.node_id));
        }
        let mut mdns: Vec<MdnsEndpoint> = self.mdns_seen.values().cloned().collect();
        mdns.sort_by(|a, b| a.fullname.cmp(&b.fullname));
        let vms: Vec<DesktopSource> = vm_list
            .iter()
            .map(|i| source_from_vm(&self.node_id, i))
            .collect();
        merge_sources(&advertised, &mdns, &vms, &self.manual)
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
            LaneStatus {
                lane: "local-kvm".to_string(),
                status: self.vm_lane.clone(),
            },
            LaneStatus {
                lane: "manual".to_string(),
                status: format!("ok ({} sources)", self.manual.len()),
            },
        ]
    }

    /// Publish the roster when the fold changed (or `force`). Returns whether
    /// a record was written.
    fn publish(&mut self, persist: &Persist, sources: Vec<DesktopSource>, force: bool) -> bool {
        let lanes = self.lanes();
        let fingerprint = serde_json::to_string(&(&sources, &lanes)).unwrap_or_default();
        if !force && self.last_fingerprint.as_deref() == Some(fingerprint.as_str()) {
            return false;
        }
        let state = DesktopSourcesState {
            node: self.node_id.clone(),
            sources,
            lanes,
            published_at_ms: now_ms(),
        };
        let Ok(body) = serde_json::to_string(&state) else {
            return false;
        };
        if let Err(e) = persist.write(SOURCES_TOPIC, Priority::Default, None, Some(&body)) {
            tracing::warn!(target: "mackesd::desktop_sources", error = %e, "sources publish failed");
            return false;
        }
        self.last_fingerprint = Some(fingerprint);
        true
    }

    /// Start the desktop-type mDNS browsers (graceful degrade: no daemon /
    /// no multicast interface → an honest `gated:` lane, worker keeps
    /// aggregating the other lanes).
    fn start_mdns_browsers(&mut self) -> Option<MdnsBrowse> {
        let daemon = match ServiceDaemon::new() {
            Ok(d) => d,
            Err(e) => {
                self.mdns_lane = format!("gated: no mDNS daemon ({e})");
                return None;
            }
        };
        let mut browsers = Vec::new();
        for bare in DESKTOP_MDNS_TYPES {
            match daemon.browse(&super::mdns_relay::browse_type(bare)) {
                Ok(rx) => browsers.push((*bare, rx)),
                Err(e) => {
                    tracing::warn!(target: "mackesd::desktop_sources", service_type = bare, error = %e, "mdns browse failed");
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
impl Worker for DesktopSourcesWorker {
    fn name(&self) -> &'static str {
        "desktop_sources"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self
            .bus_root_override
            .clone()
            .or_else(mde_bus::default_data_dir)
        else {
            tracing::debug!(target: "mackesd::desktop_sources", "no bus root; worker idle");
            return Ok(());
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(target: "mackesd::desktop_sources", error = %e, "persist open failed; worker idle");
                return Ok(());
            }
        };
        self.manual = load_manual_sources(&self.store_root);
        // Prime each verb cursor at its tail: manual sources are durable in
        // the store, so replaying an old add would resurrect a removed one.
        for topic in [ADD_SOURCE_TOPIC, REMOVE_SOURCE_TOPIC, REFRESH_TOPIC] {
            if let Ok(Some(ulid)) = persist.latest_ulid(topic) {
                self.cursors.insert(topic, ulid);
            }
        }
        let browse = self.start_mdns_browsers();

        // Immediate first publish so the Chooser doesn't wait a heartbeat.
        let vm_list = self.enumerate_vms().await;
        let sources = self.collect_sources(&vm_list);
        self.publish(&persist, sources, true);
        let mut last_pub = Instant::now();

        let mut tick = tokio::time::interval(self.tick);
        tick.tick().await; // burn the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let (changed, refresh) = self.drain_actions(&persist);
                    let mdns_changed = self.drain_mdns(browse.as_ref());
                    let due = last_pub.elapsed() >= self.heartbeat;
                    if changed || refresh || mdns_changed || due {
                        let vm_list = self.enumerate_vms().await;
                        let sources = self.collect_sources(&vm_list);
                        // A refresh/heartbeat republishes unconditionally
                        // (late subscribers); otherwise only on change.
                        if self.publish(&persist, sources, refresh || due) {
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
    use crate::ipc::action_auth::{authorize_test_body, ActionAuthorizer};
    use mackes_mesh_types::peers::{RemoteAccess, ServiceDescriptors, VmInfo};

    const AUTH_KEY: &[u8] = b"desktop-sources-action-auth-key";
    const AUTH_NOW: i64 = 1_700_000_000_000;

    fn peer(
        hostname: &str,
        health: &str,
        overlay_ip: Option<&str>,
        rdp: bool,
        vnc: bool,
        vms: Vec<VmInfo>,
    ) -> PeerRecord {
        let mut rec = PeerRecord::now(hostname, Some("12.0.0".into()), health);
        rec.overlay_ip = overlay_ip.map(str::to_string);
        rec.descriptors = Some(ServiceDescriptors {
            remote_access: RemoteAccess {
                ssh: true,
                rdp,
                vnc,
            },
            vms,
            ..ServiceDescriptors::default()
        });
        rec
    }

    fn vm_info(name: &str, state: &str) -> VmInfo {
        VmInfo {
            name: name.into(),
            state: state.into(),
            vcpus: Some(2),
            memory_mb: Some(2048),
            addresses: vec![],
        }
    }

    // ── lane 1: the advertised shape ──

    #[test]
    fn advertised_from_peer_lifts_seat_and_vm_desktops() {
        let rec = peer(
            "oak",
            "healthy",
            Some("10.42.0.7"),
            true,
            true,
            vec![vm_info("win11", "running"), vm_info("dev", "shut off")],
        );
        let ads = advertised_from_peer(&rec, "elm");
        assert_eq!(ads.len(), 3, "seat + two VMs");
        // The seat: RDP + VNC at their well-known default ports, overlay host.
        let seat = &ads[0];
        assert_eq!(seat.node, "oak");
        assert_eq!(seat.host, "10.42.0.7");
        assert!(seat.vm.is_none());
        assert_eq!(
            seat.protocols,
            vec![
                ProtocolOffer::new(DesktopProtocol::Rdp, Some(3389)),
                ProtocolOffer::new(DesktopProtocol::Vnc, Some(5900)),
            ]
        );
        assert_eq!(seat.reachability, Reachability::Reachable);
        // The running VM: a Spice console, reachable.
        let win = ads
            .iter()
            .find(|a| a.vm.as_deref() == Some("win11"))
            .unwrap();
        assert_eq!(
            win.protocols,
            vec![ProtocolOffer::new(DesktopProtocol::Spice, None)]
        );
        assert_eq!(win.power_state.as_deref(), Some("running"));
        assert_eq!(win.reachability, Reachability::Reachable);
        // The stopped VM: greyed with its state as the reason.
        let dev = ads.iter().find(|a| a.vm.as_deref() == Some("dev")).unwrap();
        assert_eq!(dev.reachability, Reachability::Unreachable);
        assert_eq!(dev.reason.as_deref(), Some("vm shut off"));
        assert_eq!(dev.power_state.as_deref(), Some("shut off"));
    }

    #[test]
    fn advertised_from_peer_skips_self_and_empty_advertisers() {
        let own = peer(
            "elm",
            "healthy",
            None,
            true,
            true,
            vec![vm_info("v", "running")],
        );
        assert!(
            advertised_from_peer(&own, "elm").is_empty(),
            "own record is skipped — local VMs ride the live KVM lane"
        );
        // A peer with no desktop listeners and no VMs advertises nothing
        // (ssh alone is not a desktop).
        let quiet = peer("ash", "healthy", None, false, false, vec![]);
        assert!(advertised_from_peer(&quiet, "elm").is_empty());
        // A pre-PD-2 writer (no descriptors) advertises nothing.
        let bare = PeerRecord::now("older", None, "healthy");
        assert!(advertised_from_peer(&bare, "elm").is_empty());
    }

    #[test]
    fn advertised_host_falls_back_to_mesh_fqdn() {
        let rec = peer("oak", "healthy", None, true, false, vec![]);
        let ads = advertised_from_peer(&rec, "elm");
        assert_eq!(ads[0].host, "oak.mesh");
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
        assert_eq!(
            peer_reachability("critical", false),
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
    fn stale_peer_desktops_grey_with_the_stale_reason() {
        let mut rec = peer("oak", "healthy", Some("10.42.0.7"), true, false, vec![]);
        rec.last_seen_ms = 1; // ancient
        let ads = advertised_from_peer(&rec, "elm");
        assert_eq!(ads[0].reachability, Reachability::Unreachable);
        assert_eq!(ads[0].reason.as_deref(), Some("peer heartbeat stale"));
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
    fn mdns_fold_lifts_desktop_types() {
        let rdp = endpoint_from_service_info("_rdp._tcp", &svc("_rdp._tcp", "OfficePC", 3389, &[]))
            .unwrap();
        assert_eq!(rdp.protocol, DesktopProtocol::Rdp);
        assert_eq!(rdp.host, "192.168.1.60");
        assert_eq!(rdp.port, 3389);
        assert_eq!(rdp.instance, "OfficePC");
        let vnc =
            endpoint_from_service_info("_rfb._tcp", &svc("_rfb._tcp", "pi", 5900, &[])).unwrap();
        assert_eq!(vnc.protocol, DesktopProtocol::Vnc);
        let spice =
            endpoint_from_service_info("_spice._tcp", &svc("_spice._tcp", "vmhost", 5930, &[]))
                .unwrap();
        assert_eq!(spice.protocol, DesktopProtocol::Spice);
    }

    #[test]
    fn mdns_fold_skips_non_desktop_and_relayed_services() {
        // A non-desktop type never becomes a source.
        assert!(
            endpoint_from_service_info("_ssh._tcp", &svc("_ssh._tcp", "shell", 22, &[])).is_none()
        );
        // A service a mesh peer republished (mdns_relay's anti-loop TXT) is
        // skipped — the registry lane already carries that peer.
        let relayed = svc(
            "_rdp._tcp",
            "OfficePC-10-42-0-9",
            3389,
            &[(super::super::mdns_relay::RELAY_ORIGIN_TXT, "10.42.0.9")],
        );
        assert!(endpoint_from_service_info("_rdp._tcp", &relayed).is_none());
    }

    // ── lane 3: the local-VM fold + honest gate ──

    #[test]
    fn source_from_vm_derives_reachability_from_power_state() {
        let node = "elm";
        let running = source_from_vm(
            node,
            &Instance {
                id: "3".into(),
                name: "dev".into(),
                state: "running".into(),
            },
        );
        assert_eq!(running.id, "vm:elm:dev");
        assert_eq!(running.reachability, Reachability::Reachable);
        assert_eq!(running.power_state.as_deref(), Some("running"));
        assert_eq!(
            running.protocols,
            vec![ProtocolOffer::new(DesktopProtocol::Spice, None)]
        );
        assert_eq!(running.origin, SourceOrigin::LocalVm);

        let off = source_from_vm(
            node,
            &Instance {
                id: "-".into(),
                name: "win".into(),
                state: "shut off".into(),
            },
        );
        assert_eq!(off.reachability, Reachability::Unreachable);
        assert_eq!(off.reason.as_deref(), Some("vm shut off"));
        assert_eq!(off.power_state.as_deref(), Some("shut off"));

        // A paused console still answers (the qemu process is live).
        let paused = source_from_vm(
            node,
            &Instance {
                id: "4".into(),
                name: "p".into(),
                state: "paused".into(),
            },
        );
        assert_eq!(paused.reachability, Reachability::Reachable);
    }

    #[test]
    fn gated_vm_lane_contributes_no_sources_and_an_honest_status() {
        let mut w = worker_at(
            tempfile::tempdir().unwrap().path(),
            tempfile::tempdir().unwrap().path(),
        );
        let list = w.fold_vm_result(Err(VmEnumerateError::Gated("virsh not found".into())));
        assert!(list.is_empty(), "a gate NEVER fabricates a source");
        assert_eq!(w.vm_lane, "gated: virsh not found");
        let lanes = w.lanes();
        let kvm = lanes.iter().find(|l| l.lane == "local-kvm").unwrap();
        assert!(kvm.status.starts_with("gated:"));

        // A backend error is likewise honest — surfaced, no sources.
        let list = w.fold_vm_result(Err(VmEnumerateError::Backend("libvirtd down".into())));
        assert!(list.is_empty());
        assert_eq!(w.vm_lane, "error: libvirtd down");

        // And a real roster flips the lane back to ok.
        let list = w.fold_vm_result(Ok(vec![Instance {
            id: "1".into(),
            name: "dev".into(),
            state: "running".into(),
        }]));
        assert_eq!(list.len(), 1);
        assert_eq!(w.vm_lane, "ok (1 vms)");
    }

    // ── lane 4: verbs + the manual store ──

    #[test]
    fn add_source_parses_and_validates() {
        let req = parse_add_source(
            r#"{"name":"lab box","host":"192.168.1.50","port":3389,"protocol":"rdp"}"#,
        )
        .unwrap();
        assert_eq!(req.host, "192.168.1.50");
        assert_eq!(req.protocol, DesktopProtocol::Rdp);
        assert_eq!(req.id(), "manual:192.168.1.50:3389:rdp");
        assert_eq!(req.display_name(), "lab box");
        // Name defaults to host:port.
        let unnamed = parse_add_source(r#"{"host":"h","port":5900,"protocol":"vnc"}"#).unwrap();
        assert_eq!(unnamed.display_name(), "h:5900");
        // Rejections are typed + human-readable.
        assert!(parse_add_source("nope").is_err());
        assert!(parse_add_source(r#"{"host":"","port":1,"protocol":"vnc"}"#).is_err());
        assert!(parse_add_source(r#"{"host":"h","port":0,"protocol":"vnc"}"#).is_err());
        assert!(parse_add_source(r#"{"host":"h","port":1,"protocol":"telnet"}"#).is_err());
    }

    #[test]
    fn remove_source_parses_and_validates() {
        let req = parse_remove_source(r#"{"id":"manual:h:5900:vnc"}"#).unwrap();
        assert_eq!(req.id, "manual:h:5900:vnc");
        assert!(parse_remove_source(r#"{"id":""}"#).is_err());
        assert!(parse_remove_source("nope").is_err());
    }

    #[test]
    fn manual_store_round_trips_and_tolerates_absence() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_manual_sources(dir.path()).is_empty());
        let sources = vec![ManualSource {
            name: None,
            host: "h".into(),
            port: 5900,
            protocol: DesktopProtocol::Vnc,
        }];
        save_manual_sources(dir.path(), &sources).unwrap();
        assert_eq!(load_manual_sources(dir.path()), sources);
        // Corrupt store → empty, never fatal.
        std::fs::write(dir.path().join(MANUAL_STORE_FILE), "{ not json").unwrap();
        assert!(load_manual_sources(dir.path()).is_empty());
    }

    // ── the merge fold ──

    fn ad_seat(node: &str, host: &str) -> AdvertisedDesktop {
        AdvertisedDesktop {
            node: node.into(),
            host: host.into(),
            vm: None,
            protocols: vec![ProtocolOffer::new(DesktopProtocol::Rdp, Some(3389))],
            power_state: None,
            reachability: Reachability::Reachable,
            reason: None,
        }
    }

    fn ep(instance: &str, host: &str, port: u16, protocol: DesktopProtocol) -> MdnsEndpoint {
        MdnsEndpoint {
            fullname: format!("{instance}._x._tcp.local."),
            instance: instance.into(),
            host: host.into(),
            port,
            protocol,
        }
    }

    #[test]
    fn merge_folds_a_known_peers_mdns_protocol_into_its_card() {
        // oak's VNC shows up on the LAN via mDNS at oak's address → the offer
        // folds into oak's card instead of a duplicate.
        let merged = merge_sources(
            &[ad_seat("oak", "10.42.0.7")],
            &[ep("oak", "10.42.0.7", 5901, DesktopProtocol::Vnc)],
            &[],
            &[],
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, "peer:oak");
        assert_eq!(
            merged[0].protocols,
            vec![
                ProtocolOffer::new(DesktopProtocol::Rdp, Some(3389)),
                ProtocolOffer::new(DesktopProtocol::Vnc, Some(5901)),
            ]
        );
        // A protocol the card already offers isn't duplicated.
        let merged = merge_sources(
            &[ad_seat("oak", "10.42.0.7")],
            &[ep("OAK", "192.168.1.9", 3390, DesktopProtocol::Rdp)],
            &[],
            &[],
        );
        assert_eq!(merged.len(), 1, "instance-name match (case-insensitive)");
        assert_eq!(merged[0].protocols.len(), 1);
    }

    #[test]
    fn merge_keeps_an_unknown_lan_endpoint_as_its_own_card() {
        let merged = merge_sources(
            &[ad_seat("oak", "10.42.0.7")],
            &[ep("OfficePC", "192.168.1.60", 3389, DesktopProtocol::Rdp)],
            &[],
            &[],
        );
        assert_eq!(merged.len(), 2);
        let lan = merged
            .iter()
            .find(|s| s.origin == SourceOrigin::Mdns)
            .unwrap();
        assert_eq!(lan.id, "mdns:192.168.1.60:3389:rdp");
        assert_eq!(lan.name, "OfficePC");
        assert_eq!(lan.reachability, Reachability::Reachable);
    }

    #[test]
    fn merge_dedups_a_manual_duplicate_and_keeps_a_new_one() {
        let dup = ManualSource {
            name: None,
            host: "10.42.0.7".into(),
            port: 3389,
            protocol: DesktopProtocol::Rdp,
        };
        let fresh = ManualSource {
            name: Some("spare".into()),
            host: "192.168.1.99".into(),
            port: 5900,
            protocol: DesktopProtocol::Vnc,
        };
        let merged = merge_sources(&[ad_seat("oak", "10.42.0.7")], &[], &[], &[dup, fresh]);
        assert_eq!(merged.len(), 2, "the duplicate folded away");
        let manual = merged
            .iter()
            .find(|s| s.origin == SourceOrigin::Manual)
            .unwrap();
        assert_eq!(manual.name, "spare");
        assert_eq!(manual.reachability, Reachability::Unknown, "never probed");
        // Same host, DIFFERENT port → a genuinely distinct endpoint, kept.
        let alt_port = ManualSource {
            name: None,
            host: "10.42.0.7".into(),
            port: 3390,
            protocol: DesktopProtocol::Rdp,
        };
        let merged = merge_sources(&[ad_seat("oak", "10.42.0.7")], &[], &[], &[alt_port]);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn merge_output_is_stably_ordered_by_node_then_name() {
        let vms = vec![
            source_from_vm(
                "elm",
                &Instance {
                    id: "1".into(),
                    name: "zeta".into(),
                    state: "running".into(),
                },
            ),
            source_from_vm(
                "elm",
                &Instance {
                    id: "2".into(),
                    name: "alpha".into(),
                    state: "running".into(),
                },
            ),
        ];
        let merged = merge_sources(
            &[ad_seat("oak", "10.42.0.7"), ad_seat("ash", "10.42.0.8")],
            &[],
            &vms,
            &[],
        );
        let order: Vec<&str> = merged.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            order,
            vec!["peer:ash", "vm:elm:alpha", "vm:elm:zeta", "peer:oak"]
        );
    }

    // ── the published record ──

    #[test]
    fn published_state_carries_an_honestly_empty_thumbnail_field() {
        let state = DesktopSourcesState {
            node: "elm".into(),
            sources: vec![source_from_manual(&ManualSource {
                name: None,
                host: "h".into(),
                port: 5900,
                protocol: DesktopProtocol::Vnc,
            })],
            lanes: vec![],
            published_at_ms: 1,
        };
        let body = serde_json::to_string(&state).unwrap();
        // The CHOOSER-3 key ships now, honestly null — never a fake ref.
        assert!(body.contains("\"thumbnail_ref\":null"));
        let back: DesktopSourcesState = serde_json::from_str(&body).unwrap();
        assert_eq!(back, state);
    }

    // ── worker orchestration over fake seams (no libvirt, no LAN) ──

    struct FakeVms(Result<Vec<Instance>, VmEnumerateError>);
    impl VmEnumerator for FakeVms {
        fn enumerate(&self) -> Result<Vec<Instance>, VmEnumerateError> {
            self.0.clone()
        }
    }

    fn worker_at(workgroup: &Path, store: &Path) -> DesktopSourcesWorker {
        DesktopSourcesWorker::new(
            "elm".to_string(),
            workgroup.to_path_buf(),
            store.to_path_buf(),
        )
        .with_authorizer(Arc::new(ActionAuthorizer::for_test(
            AUTH_KEY,
            store.join(".auth"),
            AUTH_NOW,
        )))
        .with_enumerator(Arc::new(FakeVms(Ok(vec![]))))
    }

    fn authorized_body(unsigned: &str, verb: &str, target: &str, nonce: &str) -> String {
        authorize_test_body(
            AUTH_KEY,
            unsigned,
            MutationContext {
                verb,
                node: "elm",
                target,
            },
            nonce,
            AUTH_NOW + 30_000,
        )
    }

    fn authorized_add_body(unsigned: &str, nonce: &str) -> String {
        let target = parse_add_source(unsigned).unwrap().id();
        authorized_body(unsigned, DESKTOP_ADD_SOURCE_AUTH_VERB, &target, nonce)
    }

    fn authorized_remove_body(unsigned: &str, nonce: &str) -> String {
        let target = parse_remove_source(unsigned).unwrap().id;
        authorized_body(unsigned, DESKTOP_REMOVE_SOURCE_AUTH_VERB, &target, nonce)
    }

    fn temp_persist() -> (tempfile::TempDir, Persist) {
        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().to_path_buf()).unwrap();
        (dir, persist)
    }

    fn latest_state(persist: &Persist) -> DesktopSourcesState {
        let msgs = persist.list_since(SOURCES_TOPIC, None).unwrap();
        let body = msgs.last().unwrap().body.clone().unwrap();
        serde_json::from_str(&body).unwrap()
    }

    #[test]
    fn add_source_verb_adds_persists_and_publishes() {
        let (_bus, persist) = temp_persist();
        let wg = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let mut w = worker_at(wg.path(), store.path());
        persist
            .write(
                ADD_SOURCE_TOPIC,
                Priority::Default,
                None,
                Some(&authorized_add_body(
                    r#"{"host":"192.168.1.50","port":3389,"protocol":"rdp","schema_version":1}"#,
                    "add-one",
                )),
            )
            .unwrap();
        let (changed, refresh) = w.drain_actions(&persist);
        assert!(changed);
        assert!(!refresh);
        assert_eq!(w.manual.len(), 1);
        // Durable: a fresh load sees it.
        assert_eq!(load_manual_sources(store.path()), w.manual);
        // The published roster carries it.
        let sources = w.collect_sources(&[]);
        assert!(w.publish(&persist, sources, false));
        let state = latest_state(&persist);
        assert_eq!(state.node, "elm");
        assert_eq!(state.sources.len(), 1);
        assert_eq!(state.sources[0].id, "manual:192.168.1.50:3389:rdp");
        // Re-adding the same endpoint is idempotent.
        persist
            .write(
                ADD_SOURCE_TOPIC,
                Priority::Default,
                None,
                Some(&authorized_add_body(
                    r#"{"host":"192.168.1.50","port":3389,"protocol":"rdp","schema_version":1}"#,
                    "add-two",
                )),
            )
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(!changed);
        assert_eq!(w.manual.len(), 1);
    }

    #[test]
    fn remove_source_verb_removes_and_persists() {
        let (_bus, persist) = temp_persist();
        let wg = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let mut w = worker_at(wg.path(), store.path());
        persist
            .write(
                ADD_SOURCE_TOPIC,
                Priority::Default,
                None,
                Some(&authorized_add_body(
                    r#"{"host":"h","port":5900,"protocol":"vnc","schema_version":1}"#,
                    "add-three",
                )),
            )
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(changed);
        persist
            .write(
                REMOVE_SOURCE_TOPIC,
                Priority::Default,
                None,
                Some(&authorized_remove_body(
                    r#"{"id":"manual:h:5900:vnc","schema_version":1}"#,
                    "remove-one",
                )),
            )
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(changed);
        assert!(w.manual.is_empty());
        assert!(load_manual_sources(store.path()).is_empty());
        // Removing a non-manual id is a logged no-op, never a panic.
        persist
            .write(
                REMOVE_SOURCE_TOPIC,
                Priority::Default,
                None,
                Some(&authorized_remove_body(
                    r#"{"id":"peer:oak","schema_version":1}"#,
                    "remove-two",
                )),
            )
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(!changed);
    }

    #[test]
    fn refresh_verb_nudges_and_publish_gates_on_change() {
        let (_bus, persist) = temp_persist();
        let wg = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let mut w = worker_at(wg.path(), store.path());
        persist
            .write(REFRESH_TOPIC, Priority::Default, None, Some(""))
            .unwrap();
        let (changed, refresh) = w.drain_actions(&persist);
        assert!(!changed);
        assert!(refresh);
        // publish-on-change: the first publish writes, an identical fold
        // doesn't, a forced (refresh/heartbeat) one does.
        let sources = w.collect_sources(&[]);
        assert!(w.publish(&persist, sources.clone(), false));
        assert!(!w.publish(&persist, sources.clone(), false));
        assert!(w.publish(&persist, sources, true));
    }

    #[test]
    fn add_source_requires_exact_single_use_capability_before_store_write() {
        let (_bus, persist) = temp_persist();
        let wg = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let mut w = worker_at(wg.path(), store.path());
        let unsigned = r#"{"host":"10.0.0.4","port":3389,"protocol":"rdp","schema_version":1}"#;

        persist
            .write(ADD_SOURCE_TOPIC, Priority::Default, None, Some(unsigned))
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(!changed, "unsigned add must not touch the manual store");
        assert!(w.manual.is_empty());
        assert!(load_manual_sources(store.path()).is_empty());

        let armed = authorized_add_body(unsigned, "add-hostile");
        let tampered = armed.replace("3389", "3390");
        persist
            .write(ADD_SOURCE_TOPIC, Priority::Default, None, Some(&tampered))
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(!changed, "tampered add must be refused before persistence");
        assert!(w.manual.is_empty());

        persist
            .write(ADD_SOURCE_TOPIC, Priority::Default, None, Some(&armed))
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(changed);
        assert_eq!(w.manual.len(), 1);

        persist
            .write(ADD_SOURCE_TOPIC, Priority::Default, None, Some(&armed))
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(!changed, "replaying an add capability must be refused");
        assert_eq!(w.manual.len(), 1);
    }

    #[test]
    fn remove_source_requires_exact_single_use_capability_before_store_write() {
        let (_bus, persist) = temp_persist();
        let wg = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let mut w = worker_at(wg.path(), store.path());
        let add = ManualSource {
            name: None,
            host: "10.0.0.5".into(),
            port: 5900,
            protocol: DesktopProtocol::Vnc,
        };
        assert!(w.handle_add(add.clone()));
        let unsigned = format!(r#"{{"id":"{}","schema_version":1}}"#, add.id());

        persist
            .write(
                REMOVE_SOURCE_TOPIC,
                Priority::Default,
                None,
                Some(&unsigned),
            )
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(!changed, "unsigned remove must not touch the manual store");
        assert_eq!(w.manual, vec![add.clone()]);

        let armed = authorized_remove_body(&unsigned, "remove-hostile");
        let tampered = armed.replace("10.0.0.5", "10.0.0.6");
        persist
            .write(
                REMOVE_SOURCE_TOPIC,
                Priority::Default,
                None,
                Some(&tampered),
            )
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(
            !changed,
            "tampered remove must be refused before persistence"
        );
        assert_eq!(w.manual, vec![add.clone()]);

        persist
            .write(REMOVE_SOURCE_TOPIC, Priority::Default, None, Some(&armed))
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(changed);
        assert!(w.manual.is_empty());

        persist
            .write(REMOVE_SOURCE_TOPIC, Priority::Default, None, Some(&armed))
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(!changed, "replaying a remove capability must be refused");
        assert!(w.manual.is_empty());
    }

    #[test]
    fn collect_sources_folds_the_peers_plane_and_local_vms() {
        let (_bus, persist) = temp_persist();
        let wg = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        // A peer advertising an RDP seat, plus our own record (skipped).
        let pdir = peers_dir(wg.path());
        mackes_mesh_types::peers::write_peer_record(
            &pdir,
            &peer("oak", "healthy", Some("10.42.0.7"), true, false, vec![]),
        )
        .unwrap();
        mackes_mesh_types::peers::write_peer_record(
            &pdir,
            &peer("elm", "healthy", Some("10.42.0.2"), true, true, vec![]),
        )
        .unwrap();
        let mut w = worker_at(wg.path(), store.path());
        let vms = vec![Instance {
            id: "1".into(),
            name: "dev".into(),
            state: "running".into(),
        }];
        let sources = w.collect_sources(&vms);
        let ids: Vec<&str> = sources.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["vm:elm:dev", "peer:oak"]);
        assert!(w.publish(&persist, sources, false));
        let state = latest_state(&persist);
        assert_eq!(state.sources.len(), 2);
        assert_eq!(state.lanes.len(), 4);
    }
}
