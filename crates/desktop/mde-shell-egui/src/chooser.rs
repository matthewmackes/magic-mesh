//! CHOOSER-2 — the **Desktop Chooser** surface: every discovered desktop as a
//! live card grid, grouped by node/host, one click from connecting.
//!
//! Design: `docs/design/desktop-chooser.md` (locks 1/2/3/6/14). The mackesd
//! CHOOSER-1 aggregator worker folds four discovery lanes (mesh-registry, mDNS,
//! local KVM, manual) into ONE roster on `state/desktops/sources`; this surface
//! renders that roster and drives the existing VDI attach path. It is the
//! Desktop surface's no-session face — the modernized successor to the E12-5b
//! flat picker list ([`crate::discovery`] keeps the broker-`Open` wire contract
//! this surface still emits through).
//!
//! * **Read** `state/desktops/sources` — the worker's latest
//!   [`DesktopSourcesState`] (sources + per-lane discovery status). The payload
//!   is a JSON boundary: **local** serde mirrors of the worker's wire types,
//!   exactly as `mde-files-egui::mesh_mount` (FILEMGR-9) mirrors the mesh-mount
//!   worker — the shell leans inward on `mde-bus` only, never on `mackesd` (§6).
//!   The [`DesktopSourcesClient`] seam is injectable so the model is unit-tested
//!   headless (a fake) while production talks the Bus ([`BusDesktopSources`]).
//! * **Connect** (CHOOSER-4) — activating a card raises the always-ask picker: the
//!   protocol when several are offered (lock 6 — never a silent default), the
//!   fullscreen/windowed choice (lock 9), and the single/span-all monitor choice
//!   (lock 12). Confirming hands a [`crate::vdi::ConnectRequest`] to [`crate::vdi`]
//!   (the Desktop surface takes over) and, for a mesh-brokered source (a peer
//!   seat / peer VM / local VM), publishes the broker `SessionRequest::Open`
//!   through [`crate::discovery::publish_open`] — the ONE copy of that wire
//!   shape (§6). An off-mesh endpoint (mDNS / manual) has no broker verb; its
//!   direct RDP/VNC/SPICE transport is the gated E12-4 layer — stated honestly on
//!   the note (§7 — never a silent stub, never a faked session).
//! * **Auto-popup** (lock 1) — the fold keeps a **seen set** of source ids; a
//!   genuinely new id after the first fold raises a one-shot popup flag the
//!   shell drains to surface the Chooser through its normal central-view
//!   switch.
//!
//! With no source discovered the grid gives way to the BRAND-1 backdrop
//! ([`crate::backdrop`]) with the honest reason below the logo; a populated grid
//! floats over the same backdrop dimmed to its watermark (lock 6). Reachability
//! is **read from the published state, never probed here** (lock 14): an
//! offline source renders greyed with the worker's reason and stays
//! non-interactive. Activating a connectable card raises the CHOOSER-4 always-ask
//! picker (protocol · display · monitors) and nothing connects until the operator
//! confirms it (lock 6 — never a silent protocol default).

// `pub(crate)` (was private) — WIN7-8 reuses `resolve_identity`/`resolve_seat`/
// `unix_millis` verbatim from `console::custom_sync`'s own mesh-synced Custom
// entries, so every identity-bound record in this crate agrees on the same
// resolution precedence (the `dock::response_activated`/`status::severity_color`
// cross-module-widening idiom already established this epic, restated for a
// `mod` declaration instead of a `fn`).
pub(crate) mod chooser_prefs;

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mde_egui::egui::{
    self, FontId, RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions,
};
use mde_egui::{muted_note, status_dot, Style};
use serde::{Deserialize, Serialize};

use crate::auth::{
    self, AuthStage, CredentialPrompt, CredentialStore, DesktopAuth, MeshCredentialStore,
    SealOutcome,
};
use crate::dock::DesktopRailSource;
use crate::vdi::{
    BrokerSessionLifecycle, ConnectRequest, DesktopEndpoint, DisplayMode, MonitorSpan,
    RequestedTarget, VdiProtocol,
};
use chooser_prefs::{unix_millis, ChooserPrefs, ManualEntry};

/// The retained-latest state topic the CHOOSER-1 worker publishes the merged
/// roster to. MUST equal `mackesd::workers::desktop_sources::SOURCES_TOPIC`
/// (cross-checked in tests).
pub(crate) const SOURCES_TOPIC: &str = "state/desktops/sources";

/// Roster refresh cadence. The Bus read is a cheap local spool scan and
/// discovery is human-paced, so a 5 s poll surfaces a new/removed desktop
/// without spinning — the same cadence the other planes refresh at.
const REFRESH: Duration = Duration::from_secs(5);

/// Card width — seven XL spacing steps: wide enough for a name plus a row of
/// protocol badges, narrow enough that a few cards wrap per row in the default
/// shell body beside the dock. A behaviour param on the §4 grid, not a metric
/// literal.
const CARD_WIDTH: f32 = Style::SP_XL * 7.0;

/// Card height — a fixed height keeps the grid regular across nodes so rows
/// read as one lattice (design lock 2). Sized to seat the CHOOSER-7 local-VM
/// power-control row under the badges without crowding a non-VM card.
const CARD_HEIGHT: f32 = Style::SP_XL * 7.0;

/// The thumbnail well's height — CHOOSER-3's periodic preview fills this area
/// with a decoded snapshot; a source with no (or an undecodable) snapshot ref
/// falls back to the shared monitor glyph.
const THUMB_HEIGHT: f32 = Style::SP_XL * 2.25;

/// The greyed-card opacity for an unreachable source (lock 14) — dim enough to
/// read "offline", bright enough that the reason stays legible.
const OFFLINE_OPACITY: f32 = 0.5;

/// Max decoded thumbnails held live at once — the Q7 bound so a large roster
/// never piles up unbounded GPU textures. LRU-evicted: only the most recently
/// shown cards keep a live texture; the rest fall back to the icon and re-decode
/// (cheaply, from the cached ref) if scrolled back into view.
const THUMB_CACHE_CAP: usize = 48;

/// The refresh throttle — a source's ref is re-decoded at most once per this
/// window even if the worker republishes a fresh snapshot faster. Combined with
/// the "decode only when the ref string actually changed" gate, this is the Q7
/// guarantee: NEVER a decode per card per frame, only periodic + cheap.
const THUMB_MIN_DECODE_INTERVAL: Duration = Duration::from_secs(2);

// ─────────────── wire mirrors of the CHOOSER-1 worker types ───────────────

/// A desktop-session protocol a source offers — the worker's `DesktopProtocol`
/// tag set, plus an honest catch-all so a future protocol degrades to an
/// unknown badge instead of failing the whole roster parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Protocol {
    /// Remote Desktop Protocol (`mde-vdi-rdp`).
    Rdp,
    /// VNC / RFB (`mde-vdi-vnc`).
    Vnc,
    /// Spice (`mde-vdi-spice`).
    Spice,
    /// A tag this build doesn't know — badged honestly, never connected blind.
    #[serde(other)]
    Unknown,
}

impl Protocol {
    /// The build-known, routable protocols — the CHOOSER-8 protocol-filter set
    /// (the `Unknown` catch-all is never an operator-selectable filter).
    const ALL: [Self; 3] = [Self::Rdp, Self::Vnc, Self::Spice];

    /// The card badge text.
    pub(crate) const fn badge(self) -> &'static str {
        match self {
            Self::Rdp => "RDP",
            Self::Vnc => "VNC",
            Self::Spice => "SPICE",
            Self::Unknown => "?",
        }
    }

    /// The wire tag the worker's `DesktopProtocol` serialises to (`snake_case`) —
    /// the value the CHOOSER-8 add-source verb carries. `None` for the unknown
    /// catch-all (a manual endpoint is only ever added over a known protocol).
    const fn wire_tag(self) -> Option<&'static str> {
        match self {
            Self::Rdp => Some("rdp"),
            Self::Vnc => Some("vnc"),
            Self::Spice => Some("spice"),
            Self::Unknown => None,
        }
    }

    /// The routable protocol a wire tag names, or `None` for one this build can't
    /// route — the inverse of [`wire_tag`](Self::wire_tag). CHOOSER-9 re-hydrates a
    /// roamed manual source's stored tag back through this before republishing.
    fn from_wire_tag(tag: &str) -> Option<Self> {
        match tag {
            "rdp" => Some(Self::Rdp),
            "vnc" => Some(Self::Vnc),
            "spice" => Some(Self::Spice),
            _ => None,
        }
    }

    /// The VDI route this protocol maps to, or `None` for a tag this build can't
    /// render (badged, never connected blind — §7).
    const fn route(self) -> Option<VdiProtocol> {
        match self {
            Self::Rdp => Some(VdiProtocol::Rdp),
            Self::Vnc => Some(VdiProtocol::Vnc),
            Self::Spice => Some(VdiProtocol::Spice),
            Self::Unknown => None,
        }
    }
}

/// One protocol offer on a source — a mirror of the worker's `ProtocolOffer`
/// (`port` is absent on the wire when the transport is brokered, e.g. a VM's
/// Spice console).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub(crate) struct ProtocolOffer {
    /// The protocol.
    pub(crate) protocol: Protocol,
    /// The advertised/known port, if any.
    #[serde(default)]
    pub(crate) port: Option<u16>,
}

/// Derived reachability, mirrored from the worker (lock 14 — derived from
/// roster/VM state mesh-side, NEVER probed here). An unknown tag degrades to
/// [`Self::Unknown`] — honest, never a parse failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reachability {
    /// Roster/VM state says the source should answer.
    Reachable,
    /// Roster/VM state says it won't — the card greys with the reason.
    Unreachable,
    /// Nothing derivable (a manual endpoint is never probed) — honest.
    #[serde(other)]
    Unknown,
}

impl Reachability {
    /// The states an operator can filter the grid to (CHOOSER-8) — every mirror
    /// variant, so an `Unknown`/unverified endpoint is filterable too.
    const ALL: [Self; 3] = [Self::Reachable, Self::Unreachable, Self::Unknown];

    /// The status-pip tone: live = OK, offline = danger, unverified = dim.
    const fn pip(self) -> egui::Color32 {
        match self {
            Self::Reachable => Style::OK,
            Self::Unreachable => Style::DANGER,
            Self::Unknown => Style::TEXT_DIM,
        }
    }

    /// The status-pip caption.
    const fn label(self) -> &'static str {
        match self {
            Self::Reachable => "reachable",
            Self::Unreachable => "offline",
            Self::Unknown => "unverified",
        }
    }

    /// Sort rank for the CHOOSER-8 "status" ordering — reachable desktops float
    /// first, unverified next, offline sinks to the bottom of its node group.
    const fn rank(self) -> u8 {
        match self {
            Self::Reachable => 0,
            Self::Unknown => 1,
            Self::Unreachable => 2,
        }
    }
}

/// Which discovery lane produced a source — the worker's `SourceOrigin`, with
/// an honest catch-all for a lane this build doesn't know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceOrigin {
    /// Peer-advertised via the replicated peers plane.
    MeshPeer,
    /// Discovered on the local LAN via mDNS.
    Mdns,
    /// A local libvirt/KVM guest console.
    LocalVm,
    /// Operator-added.
    Manual,
    /// A lane tag this build doesn't know.
    #[serde(other)]
    Unknown,
}

impl SourceOrigin {
    /// The card's origin caption.
    const fn label(self) -> &'static str {
        match self {
            Self::MeshPeer => "mesh peer",
            Self::Mdns => "LAN (mDNS)",
            Self::LocalVm => "local VM",
            Self::Manual => "manual",
            Self::Unknown => "discovered",
        }
    }

    /// Whether connecting goes through the mesh session broker (`Open` on
    /// `action/vdi/session`). Off-mesh endpoints have no broker verb — their
    /// direct client transport is the gated E12-4 layer.
    const fn is_mesh_brokered(self) -> bool {
        matches!(self, Self::MeshPeer | Self::LocalVm)
    }
}

/// One discovered desktop source — the projection of the worker's
/// `DesktopSource` this surface renders (serde ignores wire fields it doesn't
/// project).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct DesktopSource {
    /// Stable id (`peer:<node>` / `peer-vm:<node>:<vm>` / `vm:<node>:<name>` /
    /// `mdns:…` / `manual:…`) — the seen-set + pending-ask key.
    pub(crate) id: String,
    /// Display name for the card.
    pub(crate) name: String,
    /// The node/host the grid groups by (design lock 3).
    pub(crate) node: String,
    /// The address a client dials — the card tooltip's honest detail.
    pub(crate) host: String,
    /// Protocols offered, in the worker's stable order.
    #[serde(default)]
    pub(crate) protocols: Vec<ProtocolOffer>,
    /// The discovery lane this source came from.
    pub(crate) origin: SourceOrigin,
    /// Derived reachability (lock 14 — never a blocking probe).
    pub(crate) reachability: Reachability,
    /// Human-readable reason when not reachable (the greyed card's caption).
    #[serde(default)]
    pub(crate) reason: Option<String>,
    /// OS hint when genuinely known.
    #[serde(default)]
    pub(crate) os_hint: Option<String>,
    /// Live power state for VM sources (`running` / `shut off` / …).
    #[serde(default)]
    pub(crate) power_state: Option<String>,
    /// The CHOOSER-3 thumbnail ref — a `data:image/png;base64,…` snapshot the
    /// worker inlines periodically (a mesh peer's published snapshot, a local
    /// VM's framebuffer grab, an external endpoint's cheap probe). Resolved to a
    /// decoded, bounded-cached texture by [`ThumbnailCache`]; `null` (no live
    /// capture backend, the honest gate today) falls back to the monitor icon.
    #[serde(default)]
    pub(crate) thumbnail_ref: Option<String>,
}

impl DesktopSource {
    /// Whether a card click may connect: an offline source is greyed +
    /// non-interactive (lock 14 — CHOOSER-8 adds its retry affordance); an
    /// honest `Unknown` (a never-probed manual endpoint) may try.
    const fn connectable(&self) -> bool {
        !matches!(self.reachability, Reachability::Unreachable)
    }

    /// The dialable endpoint for a selected protocol, if discovery published one.
    /// The worker keeps the host separate from the protocol offer's port, and this
    /// fold preserves that typed shape for the live VDI transport.
    fn endpoint_for(&self, protocol: VdiProtocol) -> Option<DesktopEndpoint> {
        let port = self
            .protocols
            .iter()
            .find(|offer| offer.protocol.route() == Some(protocol))
            .and_then(|offer| offer.port)
            .or_else(|| port_from_host(&self.host))?;
        DesktopEndpoint::new(host_without_matching_port(&self.host, port), port)
    }
}

fn port_from_host(host: &str) -> Option<u16> {
    host.rsplit_once(':')
        .and_then(|(_, suffix)| suffix.parse::<u16>().ok())
}

fn host_without_matching_port(host: &str, port: u16) -> String {
    if let Some((addr, suffix)) = host.rsplit_once(':') {
        if suffix.parse::<u16>().ok() == Some(port) && !addr.is_empty() {
            return addr.to_string();
        }
    }
    host.to_string()
}

/// One discovery lane's honest status (`ok …` / `gated: …` / `error: …`) — so
/// the Chooser can say WHY a lane is empty instead of silently omitting
/// sources (§7).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct LaneStatus {
    /// Lane name (`mesh-registry` / `mdns` / `local-kvm` / `manual`).
    pub(crate) lane: String,
    /// Status string.
    pub(crate) status: String,
}

impl LaneStatus {
    /// Whether the lane is degraded (gated/errored) and worth surfacing.
    fn is_degraded(&self) -> bool {
        !self.status.starts_with("ok")
    }
}

/// The full record published on [`SOURCES_TOPIC`] — the projection this
/// surface renders (publisher node + timestamp stay on the wire, unprojected).
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub(crate) struct DesktopSourcesState {
    /// The merged, deduped source roster (worker-sorted by node, then name).
    #[serde(default)]
    pub(crate) sources: Vec<DesktopSource>,
    /// Per-lane discovery status.
    #[serde(default)]
    pub(crate) lanes: Vec<LaneStatus>,
}

/// Parse a `state/desktops/sources` record body; `None` on malformed JSON (an
/// honest miss, never a panic).
pub(crate) fn parse_sources(raw: &str) -> Option<DesktopSourcesState> {
    serde_json::from_str(raw).ok()
}

/// Group the worker-sorted roster into consecutive per-node runs, preserving
/// the published order (the worker sorts case-insensitively by node — design
/// lock 3, one unified view grouped by node/host).
fn group_by_node(sources: &[DesktopSource]) -> Vec<(&str, Vec<&DesktopSource>)> {
    let mut groups: Vec<(&str, Vec<&DesktopSource>)> = Vec::new();
    for source in sources {
        match groups.last_mut() {
            Some((node, members)) if node.eq_ignore_ascii_case(&source.node) => {
                members.push(source);
            }
            _ => groups.push((source.node.as_str(), vec![source])),
        }
    }
    groups
}

// ─────────────────── CHOOSER-7: local-VM power controls ───────────────────

/// The `action/vm/lifecycle` request topic the MV-3 `vm_lifecycle` worker drains
/// (flat; host-targeted by the request's `host` field). MUST equal
/// `mackesd::workers::vm_lifecycle::ACTION_TOPIC` (cross-checked in tests).
const LIFECYCLE_TOPIC: &str = "action/vm/lifecycle";

/// A power action a local-VM card button drives onto the mackesd `vm_lifecycle`
/// worker. The card renders only the ops valid for the VM's live power state
/// (§7 — never a button that can't act).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PowerOp {
    /// Boot a shut-off VM (`vm_lifecycle` Start).
    Start,
    /// Gracefully stop a running/paused VM (`vm_lifecycle` Stop, non-force).
    Stop,
    /// Suspend a running VM (`vm_lifecycle` Pause).
    Pause,
    /// Wake a paused VM (`vm_lifecycle` Resume).
    Resume,
}

impl PowerOp {
    /// The card button label.
    const fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Stop => "Stop",
            Self::Pause => "Pause",
            Self::Resume => "Resume",
        }
    }

    /// The present-progressive verb for the inline "applying…" note.
    const fn verb(self) -> &'static str {
        match self {
            Self::Start => "Starting",
            Self::Stop => "Stopping",
            Self::Pause => "Pausing",
            Self::Resume => "Resuming",
        }
    }

    /// The host-targeted `vm_lifecycle` request this op maps to. `host` is the VM's
    /// node (the worker there acts only on requests that `targets()` its id).
    fn to_request(self, host: &str, name: &str) -> VmPowerRequest {
        let host = host.to_string();
        let name = name.to_string();
        match self {
            Self::Start => VmPowerRequest::Start { host, name },
            Self::Stop => VmPowerRequest::Stop {
                host,
                name,
                force: false,
            },
            Self::Pause => VmPowerRequest::Pause { host, name },
            Self::Resume => VmPowerRequest::Resume { host, name },
        }
    }
}

/// The shell-side mirror of the verbs the CHOOSER-7 card power controls emit onto
/// the MV-3 `vm_lifecycle` worker — internally `op`-tagged exactly like the
/// worker's `LifecycleAction` (`#[serde(tag = "op", rename_all = "snake_case")]`),
/// so its `parse_action` accepts the body verbatim. This reuses the worker + its
/// topic (§6), mirroring them locally like `datacenter::Lifecycle` /
/// `discovery::ConnectRequest` do — the shell leans inward only on `mde-bus`,
/// never the daemon crate. `host` is always a concrete node id.
#[derive(Debug, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum VmPowerRequest {
    /// Boot a defined VM.
    Start { host: String, name: String },
    /// Stop a running/paused VM (graceful unless `force`).
    Stop {
        host: String,
        name: String,
        force: bool,
    },
    /// Suspend a running VM.
    Pause { host: String, name: String },
    /// Resume a suspended VM.
    Resume { host: String, name: String },
}

impl VmPowerRequest {
    /// Serialize to the request body. A fixed, derive-backed shape → serialization
    /// cannot realistically fail; an empty body (never produced here) is simply
    /// rejected by the worker's parser.
    fn to_body(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Publish a VM power request to `action/vm/lifecycle` via the persist-first path
/// (`mde-bus publish`'s own path): recorded locally + replicated to the target
/// node by the Bus. Records any failure in `last_error` — never panics. The exact
/// persist-write discipline as `discovery::publish` / `datacenter::publish` (§6).
fn publish_power(bus_root: Option<&Path>, last_error: &mut Option<String>, req: &VmPowerRequest) {
    let Some(root) = bus_root else {
        *last_error = Some("No mesh Bus directory — VM power actions unavailable.".to_string());
        return;
    };
    match mde_bus::persist::Persist::open(root.to_path_buf()).and_then(|p| {
        p.write(
            LIFECYCLE_TOPIC,
            mde_bus::hooks::config::Priority::Default,
            None,
            Some(&req.to_body()),
        )
    }) {
        Ok(_) => *last_error = None,
        Err(e) => *last_error = Some(format!("Couldn't publish VM power action: {e}")),
    }
}

/// A local VM's coarse power state, read from the aggregator's published
/// `power_state` string (§7 — the truth comes from the worker's roster, never
/// guessed here). Drives which power buttons a card offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PowerState {
    /// `running` — the console answers; Stop/Pause offered.
    Running,
    /// `paused` — suspended; Resume/Stop offered.
    Paused,
    /// `shut off` (or `crashed`) — Start offered (the "one click away" flow).
    ShutOff,
    /// A state this build doesn't map — no action offered (honest).
    Unknown,
}

impl PowerState {
    /// Map the aggregator's raw libvirt state string.
    fn from_wire(s: &str) -> Self {
        match s.trim() {
            "running" => Self::Running,
            "paused" => Self::Paused,
            "shut off" | "crashed" => Self::ShutOff,
            _ => Self::Unknown,
        }
    }

    /// The power ops offered from this state — the shell-side mirror of the
    /// worker's lifecycle state machine, so the card never shows a button the
    /// worker would reject.
    const fn actions(self) -> &'static [PowerOp] {
        match self {
            Self::ShutOff => &[PowerOp::Start],
            Self::Running => &[PowerOp::Stop, PowerOp::Pause],
            Self::Paused => &[PowerOp::Resume, PowerOp::Stop],
            Self::Unknown => &[],
        }
    }
}

/// The honest no-local-hypervisor gate (§7): the `local-kvm` lane's published
/// status when it reports the hypervisor toolchain is unavailable
/// (`gated: …` / `error: …`), else `None` when a live hypervisor is present. When
/// `Some`, a local-VM card shows its power buttons disabled with this reason,
/// never a control that pretends to act.
fn local_hypervisor_gate(lanes: &[LaneStatus]) -> Option<String> {
    lanes
        .iter()
        .find(|l| l.lane == "local-kvm")
        .filter(|l| {
            let s = l.status.trim_start();
            s.starts_with("gated") || s.starts_with("error")
        })
        .map(|l| l.status.clone())
}

/// Build the host-targeted `vm_lifecycle` request a card power click drives — the
/// pure card→worker mapping. `None` when the source isn't a **local** VM (a peer
/// VM is powered from its own node, not from here) or has left the roster.
fn build_power_request(sources: &[DesktopSource], id: &str, op: PowerOp) -> Option<VmPowerRequest> {
    let source = sources.iter().find(|s| s.id == id)?;
    if source.origin != SourceOrigin::LocalVm {
        return None;
    }
    Some(op.to_request(&source.node, &source.name))
}

// ─────────────── CHOOSER-8: card actions + find + offline states ───────────────

/// Typed verb: add a manual desktop source (`action/desktops/add-source`). MUST
/// equal `mackesd::workers::desktop_sources::ADD_SOURCE_TOPIC` (cross-checked in
/// tests). The card's Edit affordance republishes an edited manual endpoint over
/// this verb (add is idempotent on the source id).
const ADD_SOURCE_TOPIC: &str = "action/desktops/add-source";

/// Typed verb: remove a previously-added manual source by id. MUST equal
/// `mackesd::workers::desktop_sources::REMOVE_SOURCE_TOPIC`. Only a manual source
/// is removable — the worker no-ops a non-manual id, and the card only offers the
/// verb on manual origins (§7 — never a control that can't act).
const REMOVE_SOURCE_TOPIC: &str = "action/desktops/remove-source";

/// Typed verb: force a discovery re-enumerate + republish. MUST equal
/// `mackesd::workers::desktop_sources::REFRESH_TOPIC`. The offline card's Retry
/// affordance nudges this (a bodyless persist write) — never a shell-side probe
/// (lock 14): the roster refreshes on the next poll, nothing blocks here.
const REFRESH_TOPIC: &str = "action/desktops/refresh";

/// The shell-side mirror of the worker's `ManualSource` — the typed body of an
/// `action/desktops/add-source` request (§9: host + port + protocol, never a
/// command string). Mirrored locally like [`VmPowerRequest`], so the worker's
/// `parse_add_source` accepts it verbatim (§6 — the Bus JSON is the seam). An
/// empty `name` is skipped so the worker defaults it to `host:port`.
#[derive(Debug, Serialize)]
struct AddSourceRequest {
    /// Operator's display name, or `None` (worker defaults to `host:port`).
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Host/IP to connect to.
    host: String,
    /// Port to connect to.
    port: u16,
    /// The protocol wire tag (`rdp` / `vnc` / `spice`).
    protocol: &'static str,
}

/// The shell-side mirror of the worker's `RemoveSourceRequest` — the typed body
/// of an `action/desktops/remove-source` request (the manual source id).
#[derive(Debug, Serialize)]
struct RemoveSourceRequest {
    /// The `manual:<host>:<port>:<proto>` id to remove.
    id: String,
}

impl AddSourceRequest {
    /// Serialize to the request body (a fixed derive-backed shape — can't fail;
    /// the worker rejects an empty body it never receives here).
    fn to_body(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

impl RemoveSourceRequest {
    /// Serialize to the request body.
    fn to_body(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Publish a typed desktop-management verb (`action/desktops/*`) via the
/// persist-first path — the exact discipline as [`publish_power`] (§6): recorded
/// locally + replicated by the Bus. `body` is the verb's JSON, or `None` for the
/// bodyless refresh nudge. Records any failure in `last_error`; never panics,
/// never blocks on a peer (lock 14). `noun` names the action in the honest
/// no-Bus / failure message.
fn publish_source_action(
    bus_root: Option<&Path>,
    last_error: &mut Option<String>,
    topic: &str,
    body: Option<&str>,
    noun: &str,
) {
    let Some(root) = bus_root else {
        *last_error = Some(format!("No mesh Bus directory — {noun} unavailable."));
        return;
    };
    match mde_bus::persist::Persist::open(root.to_path_buf())
        .and_then(|p| p.write(topic, mde_bus::hooks::config::Priority::Default, None, body))
    {
        Ok(_) => *last_error = None,
        Err(e) => *last_error = Some(format!("Couldn't publish {noun}: {e}")),
    }
}

/// How the filtered grid is ordered within each node group (CHOOSER-8). Favorites
/// always float first (see [`order_members`]); this is the tiebreak below them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SortKey {
    /// The worker's published order (grouped by node, then name) — the default,
    /// a stable no-op tiebreak so the grid reads exactly as discovered.
    #[default]
    Discovered,
    /// Name A→Z (case-insensitive) within the node group.
    Name,
    /// Reachable desktops first, offline last (by [`Reachability::rank`]).
    Status,
}

impl SortKey {
    /// Every sort key, for the picker.
    const ALL: [Self; 3] = [Self::Discovered, Self::Name, Self::Status];

    /// The picker caption.
    const fn label(self) -> &'static str {
        match self {
            Self::Discovered => "As discovered",
            Self::Name => "Name (A–Z)",
            Self::Status => "Status (live first)",
        }
    }

    /// The within-group ordering this key imposes (`Discovered` keeps the
    /// published order via a stable no-op compare).
    fn cmp_sources(self, a: &DesktopSource, b: &DesktopSource) -> Ordering {
        match self {
            Self::Discovered => Ordering::Equal,
            Self::Name => a
                .name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase()),
            Self::Status => a.reachability.rank().cmp(&b.reachability.rank()),
        }
    }
}

/// The CHOOSER-8 live find controls: a free-text search plus the node / protocol
/// / status / OS filters and the sort key. Applied as a **pure fold over the
/// already-published roster** ([`FilterSort::matches`] + [`order_members`]) — the
/// grid narrows live with no probe and no blocking (§6, lock 14). `None` on a
/// filter means "any".
#[derive(Debug, Clone, Default)]
struct FilterSort {
    /// Free-text query, matched case-insensitively across name / node / host / OS.
    search: String,
    /// Restrict to one node/host, or all.
    node: Option<String>,
    /// Restrict to sources offering this protocol, or any.
    protocol: Option<Protocol>,
    /// Restrict to this reachability, or any.
    status: Option<Reachability>,
    /// Restrict to this OS hint, or any.
    os: Option<String>,
    /// The within-group ordering.
    sort: SortKey,
}

impl FilterSort {
    /// Whether this source passes every active filter + the search — the pure
    /// predicate the grid narrows through (tested headless).
    fn matches(&self, s: &DesktopSource) -> bool {
        let q = self.search.trim().to_ascii_lowercase();
        if !q.is_empty() {
            let hit = s.name.to_ascii_lowercase().contains(&q)
                || s.node.to_ascii_lowercase().contains(&q)
                || s.host.to_ascii_lowercase().contains(&q)
                || s.os_hint
                    .as_deref()
                    .is_some_and(|o| o.to_ascii_lowercase().contains(&q));
            if !hit {
                return false;
            }
        }
        if let Some(node) = self.node.as_deref() {
            if !s.node.eq_ignore_ascii_case(node) {
                return false;
            }
        }
        if let Some(proto) = self.protocol {
            if !s.protocols.iter().any(|o| o.protocol == proto) {
                return false;
            }
        }
        if let Some(status) = self.status {
            if s.reachability != status {
                return false;
            }
        }
        if let Some(os) = self.os.as_deref() {
            if s.os_hint.as_deref() != Some(os) {
                return false;
            }
        }
        true
    }

    /// Whether any filter/search is narrowing the grid — drives the "Clear"
    /// affordance and the "no match" (vs genuinely empty) copy.
    fn is_active(&self) -> bool {
        !self.search.trim().is_empty()
            || self.node.is_some()
            || self.protocol.is_some()
            || self.status.is_some()
            || self.os.is_some()
    }

    /// Drop every filter + the search (keeps the sort key — an ordering
    /// preference, not a narrowing).
    fn clear(&mut self) {
        self.search.clear();
        self.node = None;
        self.protocol = None;
        self.status = None;
        self.os = None;
    }
}

/// Order one node group's members: favorites float to the top (a shell-local
/// view preference), then the [`SortKey`] tiebreak — a **stable** sort, so
/// `Discovered` preserves the worker's published order exactly. Pure + tested.
fn order_members(members: &mut [&DesktopSource], sort: SortKey, favorites: &HashSet<String>) {
    members.sort_by(|a, b| {
        let fav_a = favorites.contains(&a.id);
        let fav_b = favorites.contains(&b.id);
        fav_b.cmp(&fav_a).then_with(|| sort.cmp_sources(a, b))
    });
}

/// The distinct nodes present in the roster, in first-seen (published) order —
/// the node filter's option list.
fn distinct_nodes(sources: &[DesktopSource]) -> Vec<String> {
    let mut seen = HashSet::new();
    sources
        .iter()
        .filter(|s| seen.insert(s.node.to_ascii_lowercase()))
        .map(|s| s.node.clone())
        .collect()
}

/// The distinct OS hints present in the roster, in first-seen order — the OS
/// filter's option list (absent when no source carries an OS hint).
fn distinct_os(sources: &[DesktopSource]) -> Vec<String> {
    let mut seen = HashSet::new();
    sources
        .iter()
        .filter_map(|s| s.os_hint.as_deref())
        .filter(|o| seen.insert(o.to_string()))
        .map(str::to_owned)
        .collect()
}

/// The in-progress manual-source form (CHOOSER-8 context-menu → Edit, or the
/// TESTVM-4 "Pin a desktop endpoint" ADD mode — an empty `original_id`). Seeded
/// from the source's current fields (edit) or blank (add); Save records the
/// CHOOSER-9 prefs register first, then mirrors through the worker's typed
/// verbs when a Bus exists (§6, never a command string). Only manual sources
/// are editable (their fields are the operator's). No `Debug` derive — the
/// `password` buffer must never reach a log (the `crate::auth` discipline).
struct ManualEdit {
    /// The manual source id being edited — the remove key + vanish guard.
    /// Empty = ADD mode (TESTVM-4): nothing to remove, no roster row required.
    original_id: String,
    /// Editable display name (empty → the worker defaults it to `host:port`).
    name: String,
    /// Editable host/IP.
    host: String,
    /// Editable port (a string buffer; parsed + non-zero-checked on Save).
    port: String,
    /// Editable protocol.
    protocol: Protocol,
    /// TESTVM-4 — optional login user stored with the entry (RDP wants one).
    username: String,
    /// TESTVM-4 — optional stored connect password (masked in the form; empty =
    /// none stored → the CHOOSER-6 one-time prompt + seal path applies instead).
    password: String,
    /// An inline validation error (empty host / bad port) — never a silent drop.
    error: Option<String>,
}

// ───────────────────── CHOOSER-3: the thumbnail cache ─────────────────────

/// One cached thumbnail slot for a source id.
struct ThumbSlot {
    /// The `thumbnail_ref` that produced `texture` — a changed ref means the
    /// worker published a fresh snapshot, so the slot re-decodes. `None` = the
    /// source carried no ref (a cached miss, so we don't retry every frame).
    ref_key: Option<String>,
    /// The decoded, uploaded texture — `None` when the ref was absent or
    /// undecodable, in which case the card draws the honest icon fallback (§7).
    texture: Option<TextureHandle>,
    /// When this slot was last (re)decoded — the throttle clock, so a churning
    /// ref can't force a decode every frame (Q7).
    decoded_at: Instant,
    /// Monotonic recency stamp for LRU eviction (bumped on every access).
    used: u64,
}

/// A bounded, throttled cache of decoded card thumbnails.
///
/// Q7 risk (design doc): periodic previews must be cheap — *never* a full decode
/// per card per frame. Two guards enforce that: a source is decoded only when its
/// `thumbnail_ref` string genuinely changed (worker-paced, not frame-paced) AND
/// no sooner than [`THUMB_MIN_DECODE_INTERVAL`] since its last decode; and the
/// live texture set is LRU-capped at [`THUMB_CACHE_CAP`]. Keyed by source id.
#[derive(Default)]
struct ThumbnailCache {
    /// Decoded slots, keyed by source id.
    slots: HashMap<String, ThumbSlot>,
    /// Monotonic access clock feeding the LRU recency stamps.
    clock: u64,
}

impl ThumbnailCache {
    /// The decoded texture for `source`, or `None` when there is no resolvable
    /// snapshot (the caller then draws the monitor-icon fallback). Decodes
    /// lazily, at most once per ref per throttle window — never per frame.
    fn texture_for(
        &mut self,
        ctx: &egui::Context,
        source: &DesktopSource,
    ) -> Option<TextureHandle> {
        let want = source.thumbnail_ref.as_deref();
        self.clock = self.clock.wrapping_add(1);
        let now = Instant::now();
        if Self::needs_decode(self.slots.get(&source.id), want, now) {
            // Decode + upload OUTSIDE any egui data lock. We are in the render
            // path here, NOT inside a `ctx.data_mut(…)` closure — and this cache
            // is a plain field, not egui memory — so `load_texture` (which
            // read-locks the context) can't re-enter a `data_mut` write lock and
            // DEADLOCK (the known parking_lot trap; cf. `backdrop::logo_texture`).
            let texture = want.and_then(|r| decode_thumbnail_ref(ctx, &source.id, r));
            self.slots.insert(
                source.id.clone(),
                ThumbSlot {
                    ref_key: want.map(str::to_owned),
                    texture,
                    decoded_at: now,
                    used: self.clock,
                },
            );
            self.evict();
        } else if let Some(slot) = self.slots.get_mut(&source.id) {
            slot.used = self.clock;
        }
        self.slots.get(&source.id).and_then(|s| s.texture.clone())
    }

    /// The pure decode-or-not decision (unit-tested without a GPU or a sleep):
    /// decode a never-seen slot; re-decode when the ref genuinely changed AND the
    /// throttle window has elapsed; otherwise keep the cached result (a stale but
    /// valid preview, or the icon fallback for a cached miss).
    fn needs_decode(slot: Option<&ThumbSlot>, want: Option<&str>, now: Instant) -> bool {
        slot.is_none_or(|s| {
            s.ref_key.as_deref() != want
                && now.duration_since(s.decoded_at) >= THUMB_MIN_DECODE_INTERVAL
        })
    }

    /// Evict the least-recently-used slots down to the cap. Dropping a slot drops
    /// its `TextureHandle`, freeing the GPU texture (Q7 bound).
    fn evict(&mut self) {
        while self.slots.len() > THUMB_CACHE_CAP {
            let Some(lru) = self
                .slots
                .iter()
                .min_by_key(|(_, s)| s.used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.slots.remove(&lru);
        }
    }
}

/// Resolve a `thumbnail_ref` to an uploaded texture, or `None` (icon fallback).
///
/// The ref is a `data:image/png;base64,…` snapshot inlined on the state plane;
/// this base64-decodes it, PNG-decodes to an [`egui::ColorImage`], and uploads.
/// Fail-soft at every step — a malformed/unknown ref is an honest `None`, never
/// a panic (§7).
fn decode_thumbnail_ref(ctx: &egui::Context, id: &str, ref_str: &str) -> Option<TextureHandle> {
    let image = decode_data_uri_png(ref_str)?;
    Some(ctx.load_texture(
        format!("chooser-thumb::{id}"),
        image,
        TextureOptions::LINEAR,
    ))
}

/// Decode a `data:[image/png];base64,<data>` URI to an RGBA [`egui::ColorImage`].
/// Only base64 PNG snapshots are decoded (the format the capture pipeline emits);
/// any other shape returns `None` so the card degrades to the icon (§7).
fn decode_data_uri_png(ref_str: &str) -> Option<egui::ColorImage> {
    use base64::Engine;
    let rest = ref_str.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    // Must be base64, and PNG (or an unspecified mediatype we optimistically
    // try as PNG) — never blindly trust an unknown encoding.
    if !meta.contains("base64") {
        return None;
    }
    let mediatype = meta.split(';').next().unwrap_or("");
    if !(mediatype.is_empty() || mediatype.eq_ignore_ascii_case("image/png")) {
        return None;
    }
    let raw = base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .ok()?;
    decode_png_rgba(&raw)
}

/// Decode 8-bit PNG bytes (RGBA or RGB) to an [`egui::ColorImage`], the same
/// `png`-crate path `backdrop::decode_rgba` uses; RGB is expanded opaque.
/// Fail-soft on any other shape (paletted/grayscale/16-bit → `None`).
/// `pub(crate)` because the QBRAND-4 boot-splash (`crate::splash`) decodes its
/// embedded artwork — an RGB8 wallpaper export — on this same path.
#[allow(
    clippy::redundant_pub_crate,
    reason = "pub(crate) items in a private surface module are this crate's idiom \
              (ChromeState, ChromeOutcome, …); the splash module consumes this"
)]
pub(crate) fn decode_png_rgba(bytes: &[u8]) -> Option<egui::ColorImage> {
    let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    let w = usize::try_from(info.width).ok()?;
    let h = usize::try_from(info.height).ok()?;
    match info.color_type {
        png::ColorType::Rgba => {
            let needed = w.checked_mul(h)?.checked_mul(4)?;
            let px = buf.get(..needed)?;
            Some(egui::ColorImage::from_rgba_unmultiplied([w, h], px))
        }
        png::ColorType::Rgb => {
            let needed = w.checked_mul(h)?.checked_mul(3)?;
            let px = buf.get(..needed)?;
            let mut rgba = Vec::with_capacity(w.checked_mul(h)?.checked_mul(4)?);
            for c in px.chunks_exact(3) {
                rgba.extend_from_slice(&[c[0], c[1], c[2], u8::MAX]);
            }
            Some(egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba))
        }
        _ => None,
    }
}

// ───────────────────────────── the client seam ─────────────────────────────

/// The desktop-sources read seam: the latest published roster off the Bus.
/// Injectable so the model is unit-tested headless (a fake) while production
/// talks the Bus ([`BusDesktopSources`]) — the FILEMGR-9 `MeshMountClient`
/// pattern.
pub(crate) trait DesktopSourcesClient {
    /// The newest [`DesktopSourcesState`], or `None` when nothing was
    /// published / nothing parses. Non-blocking — a local spool scan, never a
    /// peer probe (lock 14).
    fn latest(&self) -> Option<DesktopSourcesState>;

    /// Whether this node has a Bus spool at all — a gated read must not
    /// render as a live-looking "no desktops" (§7).
    fn has_bus(&self) -> bool;
}

/// The live Bus-backed client — a synchronous local `Persist` read of the one
/// retained-latest topic. Degrades honestly to `None` when there's no Bus dir
/// or no record — never a panic, never a hang.
pub(crate) struct BusDesktopSources {
    /// The resolved Bus client spool dir, or `None` when this node has no Bus.
    bus_root: Option<PathBuf>,
}

impl BusDesktopSources {
    /// Resolve the Bus spool dir from the environment (the production path).
    fn from_env() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
        }
    }

    /// Construct with an explicit spool root (tests point this at a tempdir
    /// or `None`).
    #[cfg(test)]
    pub(crate) const fn with_root(bus_root: Option<PathBuf>) -> Self {
        Self { bus_root }
    }
}

impl DesktopSourcesClient for BusDesktopSources {
    fn latest(&self) -> Option<DesktopSourcesState> {
        let root = self.bus_root.clone()?;
        let persist = mde_bus::persist::Persist::open(root).ok()?;
        // The worker writes one record per change (+ heartbeat); the newest
        // (last, ULID ascending) is the live roster.
        persist
            .list_since(SOURCES_TOPIC, None)
            .ok()?
            .into_iter()
            .filter_map(|m| m.body)
            .next_back()
            .as_deref()
            .and_then(parse_sources)
    }

    fn has_bus(&self) -> bool {
        self.bus_root.is_some()
    }
}

// ───────────────────────────── the Chooser state ─────────────────────────────

/// The in-progress connect the operator is configuring in the CHOOSER-4 picker
/// (locks 6/9/12): which source, and the three choices — protocol (seeded to the
/// first routable offer, always-asked when several exist), display mode (seeded to
/// fullscreen — the E12 idiom), and monitor span (seeded to single). Raised when a
/// connectable card is activated; drained into a [`ConnectRequest`] on confirm.
struct ConnectDraft {
    /// The source id being configured — the picker's key back into the roster.
    source_id: String,
    /// The protocol selected in the picker.
    protocol: VdiProtocol,
    /// Fullscreen vs windowed (lock 9).
    display: DisplayMode,
    /// Single vs span-all displays (lock 12).
    monitors: MonitorSpan,
    /// CHOOSER-6 — the one-time credential prompt for an external endpoint with no
    /// sealed credential yet. `None` for a mesh-brokered SSO source, for an
    /// external endpoint whose credential is already sealed (remembered), and
    /// before the first Connect resolves auth. When `Some`, the picker shows the
    /// masked username/password fields and Connect seals + connects. Cleared if the
    /// operator switches protocol (the credential is keyed per protocol).
    cred_prompt: Option<CredentialPrompt>,
}

/// The Chooser's state: the injectable roster read seam, the last published
/// roster, the auto-popup **seen set** (lock 1), the pending CHOOSER-4 connect
/// picker, and the one-shot connect hand-off the shell drains into
/// [`crate::vdi::VdiState`].
pub(crate) struct ChooserState {
    /// The roster read seam ([`BusDesktopSources`] in production).
    client: Box<dyn DesktopSourcesClient>,
    /// CHOOSER-6 — the sealed-credential store seam (injectable): a mesh-peer
    /// desktop authenticates by mesh-identity SSO (never touched), an external
    /// endpoint's credential is sealed/read here. [`MeshCredentialStore`] in
    /// production (the live seal is honest-gated mesh-side); a fake in tests.
    creds: Box<dyn CredentialStore>,
    /// Desktop-client Bus spool for the broker `Open` publish (the same
    /// resolved-once root the E12-5b picker held).
    bus_root: Option<PathBuf>,
    /// This node's peer name — the session's `client_peer` (resolved once).
    client_peer: String,
    /// The last published roster, if any.
    state: Option<DesktopSourcesState>,
    /// Source ids the operator has had on screen since this shell started —
    /// the auto-popup fold's memory (design lock 1).
    seen: HashSet<String>,
    /// Whether the first roster fold has seeded `seen` (the pre-existing
    /// world must not pop the Chooser at startup).
    seeded: bool,
    /// One-shot: a genuinely new source appeared — the shell drains this via
    /// [`Self::take_popup`] and surfaces the Chooser.
    popup: bool,
    /// When the Bus was last polled (drives the fixed cadence).
    last_poll: Option<Instant>,
    /// The last publish error, surfaced inline (honest; never a panic).
    last_error: Option<String>,
    /// An honest inline note about the last connect (what was requested, and
    /// which leg is gated).
    note: Option<String>,
    /// The connect the operator is configuring in the always-ask picker (lock 6/9/
    /// 12) — `None` when no card is being connected.
    pending: Option<ConnectDraft>,
    /// The request chosen this frame, if a connect fired — drained by the shell
    /// via [`Self::take_connect`] and handed to [`crate::vdi::VdiState`].
    connect: Option<ConnectRequest>,
    /// CHOOSER-3 — the bounded, throttled decode cache backing the card
    /// thumbnail wells (source `thumbnail_ref` → egui texture).
    thumbs: ThumbnailCache,
    /// CHOOSER-8 — the live find controls (search + node/protocol/status/OS
    /// filters + sort). A pure fold over the published roster; never a probe.
    filter: FilterSort,
    /// CHOOSER-9 — the operator's mesh-synced prefs (favorites + recents + manual
    /// sources) bound to the mesh identity and roamed per seat over the workgroup
    /// root (the MEDIA-16 seam). The authoritative store; [`favorites`](Self::favorites)
    /// / [`recents`](Self::recents) are its merged cache, refreshed each fold.
    prefs: ChooserPrefs,
    /// CHOOSER-9 — the merged pinned source ids (the [`prefs`](Self::prefs) view,
    /// refreshed each fold). A pin floats a card to the front of its node group and
    /// marks it — a view preference that follows the identity, not a roster mutation.
    favorites: HashSet<String>,
    /// CHOOSER-9 — the merged recently-used source ids (the [`prefs`](Self::prefs)
    /// view, refreshed each fold). A recently-connected desktop reads "recently
    /// used" on its card wherever the operator sits.
    recents: HashSet<String>,
    /// CHOOSER-9 — manual source ids already re-published to THIS seat's worker from
    /// the roamed prefs (once per session), so a manual source added on another seat
    /// materializes here without re-publishing every poll.
    hydrated_manual: HashSet<String>,
    /// TESTVM-4 — the merged manual registers (the [`prefs`](Self::prefs) view,
    /// refreshed each fold). The pinned-endpoint truth: any register the roster
    /// doesn't carry is folded into [`Self::sources_snapshot`] as a synthetic
    /// manual card, so a pinned endpoint is selectable with NO mesh discovery at
    /// all (no Bus, no worker roster) — and a register's stored credential
    /// connects it straight through the picker without a prompt.
    manual_cache: Vec<ManualEntry>,
    /// CHOOSER-8 — the in-progress manual-source edit (context-menu → Edit), or
    /// `None`. Mutually exclusive with the connect picker.
    manual_edit: Option<ManualEdit>,
}

impl Default for ChooserState {
    fn default() -> Self {
        Self::with_client(
            Box::new(BusDesktopSources::from_env()),
            mde_bus::client_data_dir(),
            crate::discovery::local_peer(),
            Box::new(MeshCredentialStore),
            ChooserPrefs::open_default(),
        )
    }
}

impl ChooserState {
    /// Construct over an explicit read seam + publish root + credential store +
    /// CHOOSER-9 prefs session (production wires the Bus + the mesh-side sealed
    /// store + the workgroup-root prefs; tests inject fakes, `None`, and an inert
    /// prefs session). Hydrates the favorites/recents cache from the prefs at once.
    fn with_client(
        client: Box<dyn DesktopSourcesClient>,
        bus_root: Option<PathBuf>,
        client_peer: String,
        creds: Box<dyn CredentialStore>,
        prefs: ChooserPrefs,
    ) -> Self {
        let mut state = Self {
            client,
            creds,
            bus_root,
            client_peer,
            state: None,
            seen: HashSet::new(),
            seeded: false,
            popup: false,
            last_poll: None,
            last_error: None,
            note: None,
            pending: None,
            connect: None,
            thumbs: ThumbnailCache::default(),
            filter: FilterSort::default(),
            prefs,
            favorites: HashSet::new(),
            recents: HashSet::new(),
            hydrated_manual: HashSet::new(),
            manual_cache: Vec::new(),
            manual_edit: None,
        };
        state.refresh_prefs_cache();
        state
    }

    /// CHOOSER-9 — refresh the favorites + recents + manual caches from the merged
    /// prefs (the synced view across every seat). Called on every fold so a
    /// pin/recent/manual change made at another seat surfaces here on the next poll.
    fn refresh_prefs_cache(&mut self) {
        let merged = self.prefs.merged();
        self.recents = merged.recents.iter().map(|r| r.id.clone()).collect();
        self.favorites = merged.favorites;
        self.manual_cache = merged.manual;
    }

    /// Whether the last published roster already carries source `id` — the
    /// synthetic-fold guard (a roster-backed card always wins over its register).
    fn roster_has(&self, id: &str) -> bool {
        self.state
            .as_ref()
            .is_some_and(|s| s.sources.iter().any(|x| x.id == id))
    }

    /// The bus-poll seam: refresh the roster when the cadence has elapsed,
    /// then keep the repaint heartbeat alive so a new source surfaces (and can
    /// auto-popup) without operator input. Cheap enough to call every frame —
    /// it self-gates.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            self.refresh();
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// The number of desktop sources the grid renders (`0` before the first fold)
    /// — the Desktop menu bar's live source-count status readout (MENUBAR-ALL).
    /// A pure read of the last projection plus the pinned registers the roster
    /// doesn't carry (TESTVM-4 — the count matches the cards), never a probe (§7).
    pub(crate) fn source_count(&self) -> usize {
        let roster = self.state.as_ref().map_or(0, |s| s.sources.len());
        let pinned = self
            .manual_cache
            .iter()
            .filter(|m| !self.roster_has(&m.id))
            .count();
        roster + pinned
    }

    /// Reconnect the newest recently-used source that is still present/connectable.
    /// This is the compact rail face of the same chooser state: it reuses the
    /// existing `activate` + `confirm_connect` path and returns the same
    /// [`ConnectRequest`] the expanded chooser would hand to the Desktop surface.
    /// If a source needs a credential prompt, the pending picker remains raised and
    /// `None` is returned so the shell can show the chooser face honestly.
    pub(crate) fn connect_last_recent(&mut self) -> Option<ConnectRequest> {
        self.refresh_prefs_cache();
        let sources = self.sources_snapshot();
        let recents = self.prefs.merged().recents;
        for recent in recents {
            let Some(source) = sources
                .iter()
                .find(|s| s.id == recent.id && s.connectable())
            else {
                continue;
            };
            let id = source.id.clone();
            self.activate(&sources, &id);
            self.confirm_connect(&sources);
            if let Some(request) = self.take_connect() {
                return Some(request);
            }
            if self.pending.is_some() {
                return None;
            }
        }
        None
    }

    /// Compact source rows for the Desktop rail flyout (NAVBAR-U2). This is only a
    /// bounded presentation of the same chooser snapshot; selecting a row must come
    /// back through [`Self::connect_source_id`] so the full chooser remains the
    /// source of truth for protocol/auth decisions.
    pub(crate) fn rail_sources(&mut self) -> Vec<DesktopRailSource> {
        self.refresh_prefs_cache();
        let mut sources = self.sources_snapshot();
        sources.sort_by(|a, b| {
            let af = self.favorites.contains(&a.id);
            let bf = self.favorites.contains(&b.id);
            bf.cmp(&af)
                .then_with(|| {
                    self.recents
                        .contains(&b.id)
                        .cmp(&self.recents.contains(&a.id))
                })
                .then_with(|| a.node.cmp(&b.node))
                .then_with(|| a.name.cmp(&b.name))
        });
        sources
            .iter()
            .map(|source| {
                let protocol = source
                    .protocols
                    .iter()
                    .find(|offer| offer.protocol.route().is_some())
                    .or_else(|| source.protocols.first())
                    .map_or("?", |offer| offer.protocol.badge());
                DesktopRailSource::new(
                    source.id.clone(),
                    source.name.clone(),
                    source.node.clone(),
                    protocol,
                    source.connectable(),
                    self.favorites.contains(&source.id),
                    self.recents.contains(&source.id),
                )
            })
            .collect()
    }

    /// Connect a source selected from the compact rail flyout. Reuses the same
    /// activation/confirmation path as the expanded chooser; if credentials are
    /// needed, the pending prompt remains in `ChooserState` and the shell simply
    /// surfaces Desktop/Chooser.
    pub(crate) fn connect_source_id(&mut self, id: &str) -> Option<ConnectRequest> {
        self.refresh_prefs_cache();
        let sources = self.sources_snapshot();
        self.activate(&sources, id);
        self.confirm_connect(&sources);
        self.take_connect()
    }

    /// Force an immediate roster re-read now (MENUBAR-ALL — the Desktop bar's
    /// **View → Refresh Sources**), bypassing the poll cadence. Reuses the SAME
    /// [`Self::refresh`] the cadence drives (§6, no second read path) and re-arms the
    /// cadence clock so the next `poll` doesn't immediately re-read.
    pub(crate) fn refresh_now(&mut self) {
        self.last_poll = Some(Instant::now());
        self.refresh();
    }

    /// Re-read the newest published roster and fold it (split from the
    /// cadence gate). A missing record keeps the last-known state — the read
    /// path never blanks a live grid on a transient read miss. Either way the
    /// CHOOSER-9 prefs cache is re-merged so a pin/recent/manual change roamed from
    /// another seat surfaces even when the roster itself didn't change.
    fn refresh(&mut self) {
        if let Some(state) = self.client.latest() {
            self.fold_sources(state);
        } else {
            self.refresh_prefs_cache();
        }
    }

    /// Fold one published roster into the state: any source id not yet in the
    /// **seen set** after the first fold raises the one-shot popup (design
    /// lock 1 — auto-popup on a new-source event). The first fold seeds the
    /// set silently so the pre-existing world doesn't pop the Chooser at
    /// startup, and a pending protocol ask whose source vanished is dropped.
    fn fold_sources(&mut self, state: DesktopSourcesState) {
        let fresh: Vec<String> = state
            .sources
            .iter()
            .map(|s| s.id.clone())
            .filter(|id| !self.seen.contains(id))
            .collect();
        if self.seeded && !fresh.is_empty() {
            self.popup = true;
        }
        self.seen.extend(fresh);
        self.seeded = true;
        self.state = Some(state);
        // CHOOSER-9 — capture the roster's manual sources into the synced prefs so
        // they roam, re-materialize any roamed manual source this seat's worker
        // hasn't heard of yet, then re-merge the favorites/recents/manual cache.
        self.capture_manual_sources();
        self.rematerialize_manual();
        self.refresh_prefs_cache();
        // A pending protocol ask whose source vanished is dropped — checked
        // against the roster AND the pinned registers (TESTVM-4: a synthetic
        // card's open picker must survive a roster fold that never carried it).
        if let Some(draft) = self.pending.as_ref() {
            let id = draft.source_id.clone();
            if !self.roster_has(&id) && !self.manual_cache.iter().any(|m| m.id == id) {
                self.pending = None;
            }
        }
    }

    /// CHOOSER-9 — record every manual-origin source in the roster into the synced
    /// prefs (present) so an operator-added desktop follows the identity to another
    /// seat. Only captures a source the prefs don't already carry as present, so a
    /// steady roster is a no-op; an edit/remove routes through the explicit prefs
    /// mutators instead.
    fn capture_manual_sources(&mut self) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let now = unix_millis();
        let captures: Vec<ManualEntry> = state
            .sources
            .iter()
            .filter(|s| s.origin == SourceOrigin::Manual)
            .filter_map(|s| {
                let offer = s.protocols.first()?;
                let port = offer.port?;
                let tag = offer.protocol.wire_tag()?;
                let name = (s.name != format!("{}:{port}", s.host)).then(|| s.name.clone());
                Some(ManualEntry {
                    id: s.id.clone(),
                    present: true,
                    host: s.host.clone(),
                    port,
                    protocol: tag.to_owned(),
                    name,
                    // A roster capture carries no credential — the worker never
                    // publishes one; a stored credential only ever comes from the
                    // operator's own form entry (TESTVM-4).
                    username: None,
                    password: None,
                    updated_ms: now,
                })
            })
            .collect();
        for entry in captures {
            // Only capture an endpoint the prefs have NEVER recorded — never one
            // that was removed (a tombstone), so a lingering roster row can't
            // resurrect a manual source the operator deleted on another seat.
            // (A known-present register is also skipped, so this can never
            // blank out a credential the operator stored on it.)
            if !self.prefs.knows_manual(&entry.id) {
                self.prefs.set_manual(entry);
            }
        }
    }

    /// CHOOSER-9 — re-publish any roamed manual source the synced prefs carry that
    /// this seat's roster doesn't yet show, over the ONE existing
    /// `action/desktops/add-source` verb (§6 — reusing the CHOOSER-8 seam, no new
    /// worker). Guarded once-per-id per session so a slow worker never spams the
    /// topic, and inert unless the workgroup root is actually provisioned (so an
    /// offline seat never re-materializes a phantom source). The worker then folds
    /// it into the roster on a later poll.
    fn rematerialize_manual(&mut self) {
        if !self.prefs.is_ready() {
            return;
        }
        let in_roster: HashSet<String> = self
            .state
            .as_ref()
            .map(|s| s.sources.iter().map(|x| x.id.clone()).collect())
            .unwrap_or_default();
        for entry in self.prefs.merged().manual {
            if in_roster.contains(&entry.id) || !self.hydrated_manual.insert(entry.id.clone()) {
                continue;
            }
            let Some(tag) = Protocol::from_wire_tag(&entry.protocol).and_then(Protocol::wire_tag)
            else {
                continue;
            };
            let body = AddSourceRequest {
                name: entry.name.clone(),
                host: entry.host.clone(),
                port: entry.port,
                protocol: tag,
            }
            .to_body();
            publish_source_action(
                self.bus_root.as_deref(),
                &mut self.last_error,
                ADD_SOURCE_TOPIC,
                Some(&body),
                "roamed desktop",
            );
        }
    }

    /// Take (and clear) the auto-popup request — the shell surfaces the
    /// Chooser through its normal central-view switch when this fires.
    pub(crate) fn take_popup(&mut self) -> bool {
        std::mem::take(&mut self.popup)
    }

    /// Take (and clear) the [`ConnectRequest`] a card connect chose this frame —
    /// the shell hands it to [`crate::vdi::VdiState`] so the Desktop surface
    /// takes over.
    pub(crate) const fn take_connect(&mut self) -> Option<ConnectRequest> {
        self.connect.take()
    }

    /// A cloned snapshot of the current roster (the render + act-on-click
    /// paths borrow it while mutating `self`), with the TESTVM-4 pinned
    /// endpoints folded in: any manual register the roster doesn't carry becomes
    /// a synthetic manual card — the exact row the worker's `source_from_manual`
    /// would publish (node = host, `host:port` name default, honest `Unknown`
    /// reachability, never probed) — so a pinned target is selectable with no
    /// mesh discovery at all.
    fn sources_snapshot(&self) -> Vec<DesktopSource> {
        let mut out = self
            .state
            .as_ref()
            .map(|s| s.sources.clone())
            .unwrap_or_default();
        for entry in &self.manual_cache {
            if self.roster_has(&entry.id) {
                continue;
            }
            // A register whose stored tag this build can't route stays off the
            // grid (it could never connect — §7), exactly like rematerialize.
            let Some(protocol) = Protocol::from_wire_tag(&entry.protocol) else {
                continue;
            };
            out.push(DesktopSource {
                id: entry.id.clone(),
                name: entry
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("{}:{}", entry.host, entry.port)),
                node: entry.host.clone(),
                host: entry.host.clone(),
                protocols: vec![ProtocolOffer {
                    protocol,
                    port: Some(entry.port),
                }],
                origin: SourceOrigin::Manual,
                reachability: Reachability::Unknown,
                reason: None,
                os_hint: None,
                power_state: None,
                thumbnail_ref: None,
            });
        }
        out
    }

    /// A connectable card was activated: raise the CHOOSER-4 always-ask picker,
    /// seeded to the first routable offer + the default display choices. Nothing
    /// connects here (lock 6 — always-ask; locks 9/12 make the display + monitor
    /// choice per-connection), so even a single-protocol source opens the picker.
    /// A source offering only a tag this build can't route opens no picker and
    /// says so honestly (§7). Offline cards never connect (lock 14).
    fn activate(&mut self, sources: &[DesktopSource], id: &str) {
        let Some(source) = sources.iter().find(|s| s.id == id) else {
            return;
        };
        if !source.connectable() {
            return;
        }
        // The connect picker and the manual-source edit form are mutually
        // exclusive — opening one closes the other.
        self.manual_edit = None;
        // The routable offers seed the picker; with none, there is nothing to
        // connect to — say so rather than raise an empty picker (§7).
        let Some(first) = source.protocols.iter().find_map(|o| o.protocol.route()) else {
            self.note = Some(format!("{} offers no connectable protocol.", source.name));
            return;
        };
        self.pending = Some(ConnectDraft {
            source_id: source.id.clone(),
            protocol: first,
            display: DisplayMode::Fullscreen,
            monitors: MonitorSpan::Single,
            // Auth is resolved on Connect (from the final chosen protocol), not
            // here — so a mesh peer connects SSO with no prompt and an external
            // endpoint only prompts if its credential isn't already sealed.
            cred_prompt: None,
        });
    }

    /// The operator confirmed the picker. CHOOSER-6 folds auth in here, in two
    /// phases so a mesh peer never sees a prompt and an external endpoint prompts
    /// only when its credential isn't already sealed:
    ///
    ///  * **Phase 1** (no prompt showing) — resolve the auth for the *final*
    ///    chosen protocol. A mesh-brokered source resolves to mesh-identity SSO
    ///    and connects straight through; an external endpoint with a sealed
    ///    credential connects with it (remembered); an external endpoint with no
    ///    sealed credential raises the one-time credential prompt and does NOT
    ///    connect yet (lock 6 — nothing connects without the input).
    ///  * **Phase 2** (prompt showing) — seal the entered credential (honest
    ///    [`SealOutcome`]) and connect with it.
    ///
    /// A store fault surfaces inline rather than silently prompting (§7).
    fn confirm_connect(&mut self, sources: &[DesktopSource]) {
        // The roster can move under the picker; if the source vanished, drop the
        // draft silently.
        let Some(source_id) = self.pending.as_ref().map(|d| d.source_id.clone()) else {
            return;
        };
        let Some(source) = sources.iter().find(|s| s.id == source_id).cloned() else {
            self.pending = None;
            return;
        };

        // Phase 2 — the credential prompt is up: seal + connect.
        if self
            .pending
            .as_ref()
            .is_some_and(|d| d.cred_prompt.is_some())
        {
            let draft = self.pending.take().expect("pending present");
            let prompt = draft.cred_prompt.expect("cred prompt present");
            let (resolved, outcome) = auth::remember(self.creds.as_ref(), &prompt);
            self.connect_source(
                &source,
                draft.protocol,
                draft.display,
                draft.monitors,
                resolved,
                Some(outcome),
            );
            return;
        }

        // Phase 1 — resolve auth for the final chosen protocol.
        let (protocol, display, monitors) = {
            let draft = self.pending.as_ref().expect("pending present");
            (draft.protocol, draft.display, draft.monitors)
        };
        let is_brokered = source.origin.is_mesh_brokered();
        // TESTVM-4 — a pinned endpoint whose register stores its own credential
        // connects straight through with it: no prompt, no store read — the same
        // `Sealed` shape `auth::resolve` yields, so everything downstream (the
        // honest note, the request, the gated E12-4 transport feed) is unchanged.
        if !is_brokered {
            let stored = self
                .manual_cache
                .iter()
                .find(|m| m.id == source.id)
                .and_then(|m| {
                    m.password
                        .clone()
                        .map(|pw| (m.username.clone().unwrap_or_default(), pw))
                });
            if let Some((username, password)) = stored {
                let auth = DesktopAuth::Sealed {
                    store_ref: auth::derive_store_ref(&source.host, protocol),
                    credential: auth::Credential::new(username, password),
                };
                self.pending = None;
                self.connect_source(&source, protocol, display, monitors, auth, None);
                return;
            }
        }
        match auth::resolve(
            is_brokered,
            &self.client_peer,
            &source.host,
            protocol,
            self.creds.as_ref(),
        ) {
            Ok(AuthStage::Ready(resolved)) => {
                self.pending = None;
                self.connect_source(&source, protocol, display, monitors, resolved, None);
            }
            Ok(AuthStage::Prompt(prompt)) => {
                // External endpoint, no sealed credential — raise the one-time
                // prompt; nothing connects until it's filled + confirmed.
                if let Some(draft) = self.pending.as_mut() {
                    draft.cred_prompt = Some(prompt);
                }
            }
            Err(e) => {
                self.last_error = Some(format!("Credential store: {e}"));
            }
        }
    }

    /// The operator backed out of the picker.
    fn cancel_connect(&mut self) {
        self.pending = None;
    }

    /// CHOOSER-7 — a local-VM card power button was clicked: publish the
    /// host-targeted `vm_lifecycle` request to `action/vm/lifecycle` (the ONE
    /// shared emitter, §6). The action targets the VM's own node (the worker there
    /// drops anything that doesn't `targets()` its id), and the discovery
    /// aggregator republishes the VM's new power state, which the card reflects on
    /// the next poll — never a faked local state flip here (§7). A publish failure
    /// surfaces on `last_error`, never a panic.
    fn power_action(&mut self, sources: &[DesktopSource], id: &str, op: PowerOp) {
        let Some(request) = build_power_request(sources, id, op) else {
            return;
        };
        publish_power(self.bus_root.as_deref(), &mut self.last_error, &request);
        if self.last_error.is_none() {
            if let Some(source) = sources.iter().find(|s| s.id == id) {
                self.note = Some(format!(
                    "{} {} on {} — the vm_lifecycle worker is applying it; the card reflects \
                     the new power state on the next refresh.",
                    op.verb(),
                    source.name,
                    source.node,
                ));
            }
        }
    }

    /// CHOOSER-9 — toggle a source's favorite/pin through the mesh-synced prefs. The
    /// pin is written to this seat's per-identity record (an un-pin is a tombstone,
    /// so it converges), roams to every other seat, and the merged cache refreshes
    /// so the card floats to the front of its node group at once. A view preference
    /// that follows the identity, never a roster mutation.
    fn toggle_favorite(&mut self, id: &str) {
        self.prefs.toggle_favorite(id, unix_millis());
        self.refresh_prefs_cache();
    }

    /// CHOOSER-8 — the offline card's Retry affordance (lock 14): nudge the
    /// discovery worker to re-enumerate + republish by publishing the bodyless
    /// `action/desktops/refresh` verb. This is the HONEST non-blocking retry — the
    /// shell never probes the endpoint or blocks; the greyed card simply reflects
    /// the fresh roster on the next 5 s poll. A publish failure surfaces on
    /// `last_error`, never a panic.
    fn retry_discovery(&mut self, sources: &[DesktopSource], id: &str) {
        publish_source_action(
            self.bus_root.as_deref(),
            &mut self.last_error,
            REFRESH_TOPIC,
            Some(""),
            "discovery refresh",
        );
        if self.last_error.is_none() {
            let name = sources
                .iter()
                .find(|s| s.id == id)
                .map_or("the endpoint", |s| s.name.as_str());
            self.note = Some(format!(
                "Re-checking discovery for {name} — the roster refreshes in a moment; \
                 nothing is probed from here.",
            ));
        }
    }

    /// CHOOSER-8 — remove a manual source (context-menu → Remove, manual origins
    /// only): publish the `action/desktops/remove-source` verb keyed on the source
    /// id. A non-manual id is a no-op here (the card never offers the verb on a
    /// discovered source, and the worker no-ops it too — §7). The roster drops the
    /// card on the next poll; a publish failure surfaces on `last_error`.
    fn remove_source(&mut self, sources: &[DesktopSource], id: &str) {
        let Some(source) = sources.iter().find(|s| s.id == id) else {
            return;
        };
        if source.origin != SourceOrigin::Manual {
            return;
        }
        // Mirror the remove to the discovery worker when a Bus exists; a
        // mesh-less seat skips it silently — the prefs tombstone below is the
        // remove for a pinned-register card (TESTVM-4), so no error is faked.
        if self.bus_root.is_some() {
            let body = RemoveSourceRequest { id: id.to_string() }.to_body();
            publish_source_action(
                self.bus_root.as_deref(),
                &mut self.last_error,
                REMOVE_SOURCE_TOPIC,
                Some(&body),
                "manual-source removal",
            );
        }
        // CHOOSER-9 — tombstone the synced manual register + any pin so the remove
        // roams to every seat (never a source that reappears elsewhere), then
        // refresh the merged cache.
        let now = unix_millis();
        self.prefs.remove_manual(id, now);
        self.prefs.set_favorite(id, false, now);
        self.hydrated_manual.remove(id);
        self.refresh_prefs_cache();
        if self
            .manual_edit
            .as_ref()
            .is_some_and(|e| e.original_id == id)
        {
            self.manual_edit = None;
        }
        if self.last_error.is_none() {
            self.note = Some(format!(
                "Removing {} — it drops from the roster on the next refresh.",
                source.name,
            ));
        }
    }

    /// CHOOSER-8 — open the manual-source edit form (context-menu → Edit, manual
    /// origins only), seeded from the source's current fields. Mutually exclusive
    /// with the connect picker.
    fn begin_edit(&mut self, sources: &[DesktopSource], id: &str) {
        let Some(source) = sources.iter().find(|s| s.id == id) else {
            return;
        };
        if source.origin != SourceOrigin::Manual {
            return;
        }
        let offer = source.protocols.first();
        // A manual source's display name defaults to `host:port` worker-side; seed
        // the field empty in that case so an unchanged Save re-defaults it.
        let default_name = offer.map_or_else(
            || source.host.clone(),
            |o| format!("{}:{}", source.host, o.port.unwrap_or_default()),
        );
        let name = if source.name == default_name {
            String::new()
        } else {
            source.name.clone()
        };
        // TESTVM-4 — seed any stored credential from the register so an edit
        // round-trips it (an untouched Save keeps the pinned endpoint's login).
        let stored = self.manual_cache.iter().find(|m| m.id == source.id);
        self.pending = None;
        self.manual_edit = Some(ManualEdit {
            original_id: source.id.clone(),
            name,
            host: source.host.clone(),
            port: offer
                .and_then(|o| o.port)
                .map(|p| p.to_string())
                .unwrap_or_default(),
            protocol: offer.map_or(Protocol::Rdp, |o| o.protocol),
            username: stored.and_then(|m| m.username.clone()).unwrap_or_default(),
            password: stored.and_then(|m| m.password.clone()).unwrap_or_default(),
            error: None,
        });
    }

    /// TESTVM-4 — open the manual form in ADD mode (an empty `original_id`): pin
    /// a desktop endpoint by hand (host:port + protocol + an optional stored
    /// credential), with or without mesh discovery. Mutually exclusive with the
    /// connect picker, exactly like [`Self::begin_edit`].
    fn begin_add(&mut self) {
        self.pending = None;
        self.manual_edit = Some(ManualEdit {
            original_id: String::new(),
            name: String::new(),
            host: String::new(),
            port: String::new(),
            protocol: Protocol::Rdp,
            username: String::new(),
            password: String::new(),
            error: None,
        });
    }

    /// CHOOSER-8 — the operator backed out of the edit form.
    fn cancel_manual_edit(&mut self) {
        self.manual_edit = None;
    }

    /// CHOOSER-8 / TESTVM-4 — Save the manual-source form (edit OR add mode):
    /// validate host + port, write the CHOOSER-9 prefs register FIRST (the
    /// shell-config truth — the pinned endpoint is selectable at once, mesh or
    /// no mesh), then mirror through the worker's typed verbs when a Bus exists
    /// — remove the old id, add the edited endpoint (§6; add is idempotent on
    /// the new id). A bad host/port sets the form's inline error and saves
    /// NOTHING; a publish failure surfaces on `last_error`. A roster-backed
    /// card reflects the change on the next poll.
    fn save_manual_edit(&mut self, sources: &[DesktopSource]) {
        // Read the draft's fields into owned locals FIRST, so the `self.manual_edit`
        // borrow is dropped before any publish / error re-borrow (borrow clean).
        let Some(edit) = self.manual_edit.as_ref() else {
            return;
        };
        // ADD mode carries no original id (TESTVM-4) — there is nothing to
        // vanish, nothing to remove, and no roster row to require.
        let adding = edit.original_id.is_empty();
        // An edited source can move under the form; if it vanished, drop the edit.
        if !adding && !sources.iter().any(|s| s.id == edit.original_id) {
            self.manual_edit = None;
            return;
        }
        let host = edit.host.trim().to_string();
        let port_str = edit.port.trim().to_string();
        let name = {
            let n = edit.name.trim();
            (!n.is_empty()).then(|| n.to_string())
        };
        let username = {
            let u = edit.username.trim();
            (!u.is_empty()).then(|| u.to_string())
        };
        let password = (!edit.password.is_empty()).then(|| edit.password.clone());
        let protocol = edit.protocol;
        let original_id = edit.original_id.clone();

        // Validate host + port; a failure sets the form's inline error and does
        // NOT publish (§7 — never a silent drop).
        let validated_port: Result<u16, &str> = if host.is_empty() {
            Err("Host must not be empty.")
        } else {
            match port_str.parse::<u16>() {
                Ok(0) => Err("Port must be non-zero."),
                Ok(p) => Ok(p),
                Err(_) => Err("Port must be a number (1\u{2013}65535)."),
            }
        };
        let port = match validated_port {
            Ok(p) => p,
            Err(msg) => {
                if let Some(e) = self.manual_edit.as_mut() {
                    e.error = Some(msg.to_string());
                }
                return;
            }
        };
        let Some(protocol_tag) = protocol.wire_tag() else {
            if let Some(e) = self.manual_edit.as_mut() {
                e.error = Some("Pick a connectable protocol.".to_string());
            }
            return;
        };
        // TESTVM-4 — the CHOOSER-9 prefs register is the shell-config truth and
        // is written FIRST: the pinned endpoint renders + connects from it (the
        // synthetic fold) whether or not any mesh worker ever hears the Bus
        // mirror below, and the record (credential included) roams per seat.
        let new_id = format!("manual:{host}:{port}:{protocol_tag}");
        let now = unix_millis();
        if !adding && original_id != new_id {
            // The endpoint key moved: tombstone the old register (+ its pin).
            self.prefs.remove_manual(&original_id, now);
            self.prefs.set_favorite(&original_id, false, now);
            self.hydrated_manual.remove(&original_id);
        }
        self.prefs.set_manual(ManualEntry {
            id: new_id.clone(),
            present: true,
            host: host.clone(),
            port,
            protocol: protocol_tag.to_owned(),
            name: name.clone(),
            username,
            password,
            updated_ms: now,
        });
        // This seat is publishing the add itself below (when a Bus exists) —
        // never let rematerialize double-publish the same id this session.
        self.hydrated_manual.insert(new_id);
        self.refresh_prefs_cache();

        // Mirror through the discovery worker's typed verbs when a Bus exists.
        // A mesh-less seat skips this silently — the register above already made
        // the endpoint selectable, so no error is faked (§7).
        if self.bus_root.is_some() {
            self.mirror_manual_save(
                (!adding).then_some(original_id.as_str()),
                name.as_deref(),
                &host,
                port,
                protocol_tag,
            );
        }

        let shown = name.unwrap_or_else(|| format!("{host}:{port}"));
        let verb = if adding { "Pinned" } else { "Updated" };
        let tail = if self.bus_root.is_some() {
            "the roster reflects it on the next refresh"
        } else {
            "no mesh Bus on this seat, so the card renders from the pin alone"
        };
        self.note = Some(format!(
            "{verb} {shown} ({} \u{00B7} {host}:{port}) — stored in the chooser prefs; {tail}.",
            protocol_tag.to_ascii_uppercase(),
        ));
        self.manual_edit = None;
    }

    /// The Bus leg of a manual-source save: remove the old id when the edit
    /// moved the endpoint key (`original_id` is `Some` only for an edit), then
    /// add the saved endpoint over the worker's typed verbs (§6 — the CHOOSER-8
    /// seam; add is idempotent on the new id). A publish failure surfaces on
    /// `last_error`, never a panic.
    fn mirror_manual_save(
        &mut self,
        original_id: Option<&str>,
        name: Option<&str>,
        host: &str,
        port: u16,
        protocol_tag: &'static str,
    ) {
        if let Some(original_id) = original_id {
            let remove_body = RemoveSourceRequest {
                id: original_id.to_owned(),
            }
            .to_body();
            publish_source_action(
                self.bus_root.as_deref(),
                &mut self.last_error,
                REMOVE_SOURCE_TOPIC,
                Some(&remove_body),
                "manual-source edit",
            );
            if self.last_error.is_some() {
                return;
            }
        }
        let add_body = AddSourceRequest {
            name: name.map(str::to_owned),
            host: host.to_owned(),
            port,
            protocol: protocol_tag,
        }
        .to_body();
        publish_source_action(
            self.bus_root.as_deref(),
            &mut self.last_error,
            ADD_SOURCE_TOPIC,
            Some(&add_body),
            "manual-source edit",
        );
    }

    /// Connect one source with the picked options: build the [`ConnectRequest`]
    /// for the Desktop surface, and — for a mesh-brokered source — publish the
    /// broker `SessionRequest::Open` through the ONE existing wire path
    /// ([`crate::discovery::publish_open`], §6). An off-mesh endpoint has no
    /// broker verb, so only the hand-off happens; either way the note says which
    /// leg is gated, and no session is ever faked (§7).
    fn connect_source(
        &mut self,
        source: &DesktopSource,
        protocol: VdiProtocol,
        display: DisplayMode,
        monitors: MonitorSpan,
        auth: DesktopAuth,
        seal: Option<SealOutcome>,
    ) {
        // The resolved auth mode, stated honestly on the note (§7) — SSO vs a
        // sealed credential. `summary()` is log-safe: it never carries the secret.
        let auth_summary = auth.summary();
        let mut broker_session = None;
        if source.origin.is_mesh_brokered() {
            // A peer seat's roster row has `name == node`, so `name` is the
            // broker's vm_id handle for seats AND VMs (the same handle the
            // E12-5b picker and Chat's Remote Control publish).
            let publication = crate::discovery::publish_open_record(
                self.bus_root.as_deref(),
                &mut self.last_error,
                &source.node,
                &source.name,
                &self.client_peer,
            );
            if self.last_error.is_none() {
                broker_session = Some(BrokerSessionLifecycle::new(
                    publication.id,
                    self.bus_root.clone(),
                ));
            }
            self.note = Some(format!(
                "Requested {} from {} via {} ({} \u{00B7} {}) — brokering over the mesh; \
                 authenticating with {auth_summary}.",
                source.name,
                source.node,
                protocol.label(),
                display.label(),
                monitors.label(),
            ));
        } else {
            self.note = Some(format!(
                "Direct {} connect to {} ({} \u{00B7} {}) — the live client transport attaches \
                 in E12-4; authenticating with {auth_summary}.",
                protocol.label(),
                source.host,
                display.label(),
                monitors.label(),
            ));
        }
        // CHOOSER-6 — the honest remember/seal outcome for an external credential
        // (never a faked "remembered"; a gated seal still connects in-memory).
        if let Some(outcome) = seal {
            let suffix = match outcome {
                SealOutcome::Sealed => {
                    " The credential is sealed in the secret store (remembered).".to_string()
                }
                SealOutcome::Gated(reason) => {
                    format!(" The credential drives this session but isn't remembered — {reason}.")
                }
                SealOutcome::Failed(reason) => {
                    format!(" The credential couldn't be sealed — {reason}.")
                }
            };
            if let Some(note) = self.note.as_mut() {
                note.push_str(&suffix);
            }
        }
        // Future protocol routes without a client must say so, never imply a live
        // session (§7).
        if !protocol.has_client() {
            if let Some(note) = self.note.as_mut() {
                note.push_str(" The selected client is not wired yet — no session is faked.");
            }
        }
        let mut request = ConnectRequest::new(
            RequestedTarget::new(source.node.clone(), source.name.clone())
                .with_endpoint(source.endpoint_for(protocol)),
            protocol,
            display,
            monitors,
            auth,
        );
        if let Some(broker_session) = broker_session {
            request = request.with_broker_session(broker_session);
        }
        self.connect = Some(request);
        // CHOOSER-9 — a genuine connect makes this desktop "recently used"; the
        // record roams so the operator's recents follow them to any seat.
        self.prefs
            .record_recent(&source.id, &source.name, unix_millis());
        self.refresh_prefs_cache();
    }

    /// The honest empty-grid copy: a missing Bus (a gated read), a worker
    /// that hasn't published yet, and a genuinely quiet mesh are three
    /// different truths (§7) — and quiet degraded lanes are named so an empty
    /// grid never hides WHY a lane found nothing.
    fn empty_copy(&self) -> (String, String) {
        if !self.client.has_bus() {
            return (
                "Desktop discovery unavailable".to_string(),
                "No mesh Bus directory on this node, so the discovered-desktop roster can't \
                 be read — joining the mesh (the mde-bus spool) unblocks the Chooser."
                    .to_string(),
            );
        }
        let Some(state) = self.state.as_ref() else {
            return (
                "Desktop discovery hasn't reported yet".to_string(),
                "The mackesd desktop-sources worker hasn't published a roster on this Bus — \
                 it publishes within moments of starting."
                    .to_string(),
            );
        };
        let mut detail = "No mesh peer, LAN endpoint, or local VM is advertising a desktop — \
                          a new discovery appears here within a few seconds."
            .to_string();
        let degraded: Vec<String> = state
            .lanes
            .iter()
            .filter(|l| l.is_degraded())
            .map(|l| format!("{} — {}", l.lane, l.status))
            .collect();
        if !degraded.is_empty() {
            detail.push_str(" Quiet lanes: ");
            detail.push_str(&degraded.join("; "));
            detail.push('.');
        }
        ("No desktops discovered".to_string(), detail)
    }
}

// ───────────────────────────── the panel render ─────────────────────────────

/// What a card interaction asked for this frame — applied after the grid loop
/// so the render borrows and the state mutation never fight.
enum CardAction {
    /// A card was clicked (raise the CHOOSER-4 connect picker).
    Activate(String),
    /// The connect picker was confirmed (connect with the chosen options).
    Confirm,
    /// The connect picker was dismissed.
    Cancel,
    /// CHOOSER-7 — a local-VM power button was clicked (drive the `vm_lifecycle`
    /// worker for that VM).
    Power {
        /// The source id (the roster key back to its node + name).
        id: String,
        /// The lifecycle op the button maps to.
        op: PowerOp,
    },
    /// CHOOSER-8 — pin/unpin a card (float it first in its node group).
    ToggleFavorite(String),
    /// CHOOSER-8 — the offline card's Retry: nudge a discovery re-enumerate.
    Retry(String),
    /// CHOOSER-8 — open the manual-source edit form (manual origins only).
    EditSource(String),
    /// CHOOSER-8 — remove a manual source (manual origins only).
    RemoveSource(String),
    /// CHOOSER-8 — the manual-source edit form was confirmed.
    SaveEdit,
    /// CHOOSER-8 — the manual-source edit form was dismissed.
    CancelEdit,
}

/// Render the Chooser into `ui`: the BRAND-1 backdrop first (full hero +
/// honest copy when nothing is discovered, the low watermark under a populated
/// grid — lock 6), then the node-grouped card grid, the CHOOSER-4 confirm
/// affordance when raised, and the degraded-lane notes.
pub(crate) fn chooser_panel(ui: &mut egui::Ui, state: &mut ChooserState) {
    let sources = state.sources_snapshot();
    let empty = sources.is_empty();

    let status = empty.then(|| state.empty_copy());
    let coverage = if empty {
        crate::backdrop::Coverage::Empty
    } else {
        crate::backdrop::Coverage::Covered
    };
    crate::backdrop::show(
        ui,
        coverage,
        status.as_ref().map(|(t, d)| (t.as_str(), d.as_str())),
    );

    if let Some(err) = state.last_error.as_deref() {
        ui.colored_label(Style::DANGER, err);
        ui.add_space(Style::SP_S);
    }

    // TESTVM-4 — pin a desktop endpoint by hand (host:port + protocol + an
    // optional stored credential). Offered with OR without a discovered roster,
    // so a mesh-less seat can still pin its first target — the register renders
    // as a card from the prefs alone.
    ui.add_space(Style::SP_S);
    if state.manual_edit.is_none()
        && ui
            .button(RichText::new("Pin a desktop endpoint\u{2026}").size(Style::SMALL))
            .clicked()
    {
        state.begin_add();
    }

    if empty {
        // The ADD form must still render over the empty backdrop (there is no
        // grid to host it), or the first pin could never be entered.
        let action = state
            .manual_edit
            .as_mut()
            .and_then(|edit| manual_edit_form(ui, edit));
        match action {
            Some(CardAction::SaveEdit) => state.save_manual_edit(&sources),
            Some(CardAction::CancelEdit) => state.cancel_manual_edit(),
            _ => {}
        }
        // The honest connect/save note still shows under the form (the §7 truth
        // line — e.g. "Pinned … no mesh Bus on this seat").
        if let Some(note) = state.note.as_deref() {
            ui.add_space(Style::SP_S);
            muted_note(ui, note);
        }
        return;
    }

    // Section label — the mature planes' idiom (dim, small, sentence case).
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("Discovered desktops")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
    );
    ui.add_space(Style::SP_XS);

    // CHOOSER-8 — the live find controls (search + node/protocol/status/OS
    // filters + sort). Its option lists come from the roster, and it mutates the
    // live filter BEFORE the render fold clones it, so the grid narrows this same
    // frame — a pure fold, never a probe (§6, lock 14).
    let nodes = distinct_nodes(&sources);
    let oses = distinct_os(&sources);
    filter_bar(ui, &mut state.filter, &nodes, &oses);
    ui.add_space(Style::SP_XS);

    // Pull the state fields the grid closure reads out to locals FIRST, then
    // borrow the disjoint `&mut` fields — so the closure captures only owned
    // values + those fields, never `state` wholesale (borrow clean).
    let filter = state.filter.clone();
    let favorites = state.favorites.clone();
    let recents = state.recents.clone();
    let pending_id = state.pending.as_ref().map(|d| d.source_id.clone());
    let note = state.note.clone();
    // CHOOSER-7 — the honest no-local-hypervisor gate, read once from the roster's
    // lane status and threaded to every local-VM card's power row.
    let power_gate: Option<String> = state
        .state
        .as_ref()
        .and_then(|s| local_hypervisor_gate(&s.lanes));
    let degraded: Vec<String> = state
        .state
        .as_ref()
        .map(|s| {
            s.lanes
                .iter()
                .filter(|l| l.is_degraded())
                .map(|l| format!("{} lane: {}", l.lane, l.status))
                .collect()
        })
        .unwrap_or_default();
    let thumbs = &mut state.thumbs;
    let pending_draft = &mut state.pending;
    let edit_draft = &mut state.manual_edit;

    let mut action: Option<CardAction> = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            action = chooser_grid(
                ui,
                &sources,
                &filter,
                &favorites,
                &recents,
                pending_id.as_deref(),
                note.as_deref(),
                power_gate.as_deref(),
                &degraded,
                thumbs,
                pending_draft,
                edit_draft,
            );
        });

    match action {
        Some(CardAction::Activate(id)) => state.activate(&sources, &id),
        Some(CardAction::Confirm) => state.confirm_connect(&sources),
        Some(CardAction::Cancel) => state.cancel_connect(),
        Some(CardAction::Power { id, op }) => state.power_action(&sources, &id, op),
        Some(CardAction::ToggleFavorite(id)) => state.toggle_favorite(&id),
        Some(CardAction::Retry(id)) => state.retry_discovery(&sources, &id),
        Some(CardAction::EditSource(id)) => state.begin_edit(&sources, &id),
        Some(CardAction::RemoveSource(id)) => state.remove_source(&sources, &id),
        Some(CardAction::SaveEdit) => state.save_manual_edit(&sources),
        Some(CardAction::CancelEdit) => state.cancel_manual_edit(),
        None => {}
    }
}

/// The scrollable grid body: the CHOOSER-8 live narrowing (filter → node groups
/// ordered favorites-first + by the sort key), the "no match" copy when a filter
/// zeroes the roster, the CHOOSER-4 connect picker, the CHOOSER-8 manual-source
/// edit form, and the honest note + degraded-lane lines. Returns the one card
/// action chosen this frame (applied by [`chooser_panel`] after the render).
#[allow(clippy::too_many_arguments)]
fn chooser_grid(
    ui: &mut egui::Ui,
    sources: &[DesktopSource],
    filter: &FilterSort,
    favorites: &HashSet<String>,
    recents: &HashSet<String>,
    pending_id: Option<&str>,
    note: Option<&str>,
    power_gate: Option<&str>,
    degraded: &[String],
    thumbs: &mut ThumbnailCache,
    pending_draft: &mut Option<ConnectDraft>,
    edit_draft: &mut Option<ManualEdit>,
) -> Option<CardAction> {
    let mut action: Option<CardAction> = None;

    // CHOOSER-8 — the live narrowing: filter the roster, then group by node (a
    // filtered subsequence of the worker-sorted roster stays sorted, so
    // consecutive runs are preserved) and order each group favorites-first + by
    // the sort key.
    let visible: Vec<DesktopSource> = sources
        .iter()
        .filter(|s| filter.matches(s))
        .cloned()
        .collect();

    if visible.is_empty() {
        ui.add_space(Style::SP_S);
        muted_note(
            ui,
            "No desktop matches the current search and filters — clear them to see the whole \
             roster.",
        );
    }

    for (node, mut members) in group_by_node(&visible) {
        order_members(&mut members, filter.sort, favorites);
        // The node/host group header (design lock 3).
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new(node)
                .color(Style::TEXT)
                .size(Style::BODY)
                .strong(),
        );
        ui.add_space(Style::SP_XS);
        ui.horizontal_wrapped(|ui| {
            for source in members {
                let pending = pending_id == Some(source.id.as_str());
                let favorite = favorites.contains(&source.id);
                let recent = recents.contains(&source.id);
                if let Some(a) =
                    source_card(ui, source, pending, favorite, recent, thumbs, power_gate)
                {
                    action = Some(a);
                }
                ui.add_space(Style::SP_S);
            }
        });
    }

    // The CHOOSER-4 always-ask connect picker — nothing connects unless the
    // operator confirms it (lock 6). The radios mutate the live draft.
    if let Some(draft) = pending_draft.as_mut() {
        if let Some(source) = sources.iter().find(|s| s.id == draft.source_id) {
            if let Some(a) = connect_picker(ui, source, draft) {
                action = Some(a);
            }
        }
    }

    // CHOOSER-8 — the manual-source form (mutually exclusive with the connect
    // picker). Its fields mutate the live draft; Save records the prefs register
    // then mirrors through the worker's typed verbs. TESTVM-4's ADD mode (empty
    // `original_id`) has no roster row to require, so it always renders.
    if let Some(edit) = edit_draft.as_mut() {
        if edit.original_id.is_empty() || sources.iter().any(|s| s.id == edit.original_id) {
            if let Some(a) = manual_edit_form(ui, edit) {
                action = Some(a);
            }
        }
    }

    if let Some(note) = note {
        ui.add_space(Style::SP_S);
        muted_note(ui, note);
    }

    // Degraded discovery lanes, named under the grid (§7 — a lane that found
    // nothing says why, instead of silently omitting).
    if !degraded.is_empty() {
        ui.add_space(Style::SP_S);
        for line in degraded {
            muted_note(ui, line);
        }
    }

    action
}

/// The CHOOSER-8 find bar: a search box + the node / protocol / status / OS
/// filters + the sort key, all `Style`-tokened (§4). Every control mutates the
/// live `filter` in place, so the grid narrows on the same frame — a pure fold
/// over the published roster (§6). `nodes` / `oses` are the roster's distinct
/// values (the combo option lists); the OS combo is omitted when no source
/// carries an OS hint. A Clear button appears while any filter is active.
fn filter_bar(ui: &mut egui::Ui, filter: &mut FilterSort, nodes: &[String], oses: &[String]) {
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut filter.search)
                .desired_width(Style::SP_XL * 5.0)
                .hint_text("Search name / node / OS…"),
        );
        ui.add_space(Style::SP_S);

        // Node filter.
        egui::ComboBox::from_id_salt("chooser-filter-node")
            .selected_text(filter.node.as_deref().unwrap_or("All nodes"))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut filter.node, None, "All nodes");
                for node in nodes {
                    ui.selectable_value(&mut filter.node, Some(node.clone()), node);
                }
            });
        ui.add_space(Style::SP_S);

        // Protocol filter.
        egui::ComboBox::from_id_salt("chooser-filter-proto")
            .selected_text(filter.protocol.map_or("Any protocol", Protocol::badge))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut filter.protocol, None, "Any protocol");
                for proto in Protocol::ALL {
                    ui.selectable_value(&mut filter.protocol, Some(proto), proto.badge());
                }
            });
        ui.add_space(Style::SP_S);

        // Status filter.
        egui::ComboBox::from_id_salt("chooser-filter-status")
            .selected_text(filter.status.map_or("Any status", Reachability::label))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut filter.status, None, "Any status");
                for status in Reachability::ALL {
                    ui.selectable_value(&mut filter.status, Some(status), status.label());
                }
            });
        ui.add_space(Style::SP_S);

        // OS filter — only when the roster carries OS hints.
        if !oses.is_empty() {
            egui::ComboBox::from_id_salt("chooser-filter-os")
                .selected_text(filter.os.as_deref().unwrap_or("Any OS"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut filter.os, None, "Any OS");
                    for os in oses {
                        ui.selectable_value(&mut filter.os, Some(os.clone()), os);
                    }
                });
            ui.add_space(Style::SP_S);
        }

        // Sort key.
        ui.label(
            RichText::new("Sort")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        egui::ComboBox::from_id_salt("chooser-sort")
            .selected_text(filter.sort.label())
            .show_ui(ui, |ui| {
                for key in SortKey::ALL {
                    ui.selectable_value(&mut filter.sort, key, key.label());
                }
            });

        // Clear — only while something is narrowing the grid.
        if filter.is_active() {
            ui.add_space(Style::SP_S);
            if ui
                .button(RichText::new("Clear").size(Style::SMALL))
                .clicked()
            {
                filter.clear();
            }
        }
    });
}

/// The inline controls a card's body surfaced this frame (CHOOSER-7/8): a power
/// op clicked, and/or the offline Retry affordance. Reconciled against the card's
/// primary click + its context menu by [`source_card`].
#[derive(Default)]
struct CardControls {
    /// A CHOOSER-7 local-VM power op clicked this frame.
    power: Option<PowerOp>,
    /// The CHOOSER-8 offline Retry button was clicked.
    retry: bool,
}

/// Render one desktop card: the thumbnail well (the decoded live preview, or the
/// honest monitor-icon fallback), the display name, the VM power state when there
/// is one, the protocol badge row, and the status pip — greyed with the worker's
/// reason when the source is offline (lock 14), with a pin marker when favorited
/// (CHOOSER-8). A left click on a connectable card activates it; a right click
/// raises the per-card context menu (CHOOSER-8). Returns the chosen action.
fn source_card(
    ui: &mut egui::Ui,
    source: &DesktopSource,
    pending: bool,
    favorite: bool,
    recent: bool,
    thumbs: &mut ThumbnailCache,
    gate: Option<&str>,
) -> Option<CardAction> {
    let card = egui::vec2(CARD_WIDTH, CARD_HEIGHT);
    // Every card senses clicks so it can raise the CHOOSER-8 context menu (right
    // click) — but only a connectable card's primary click ACTIVATES (an offline
    // card's left click is a no-op; its Retry/context menu drive it). Its inline
    // buttons stay live regardless.
    let sense = Sense::click();
    // The whole card is ONE interactive container. `UiBuilder::sense` registers the
    // card's click BELOW any widget inside it, so a power/Retry button (added
    // within) receives its own click instead — a Stop/Pause tap never doubles as a
    // console-open activate (the CHOOSER-7 co-existence, egui's documented idiom).
    let scoped = ui.scope_builder(egui::UiBuilder::new().sense(sense), |ui| {
        // Reserve exactly the card so the grid stays regular; the plate is painted
        // over this fixed rect and the body lays out within it.
        ui.set_min_size(card);
        ui.set_max_width(CARD_WIDTH);
        let rect = egui::Rect::from_min_size(ui.min_rect().min, card);
        let hovered = source.connectable() && ui.rect_contains_pointer(rect);

        // The card plate — painted first so the content lays out over it.
        let fill = if hovered {
            Style::SURFACE_HI
        } else {
            Style::SURFACE
        };
        let border = if pending {
            Style::ACCENT_HI
        } else if hovered {
            Style::ACCENT
        } else {
            Style::BORDER
        };
        ui.painter().rect_filled(rect, Style::RADIUS, fill);
        ui.painter().rect_stroke(
            rect,
            Style::RADIUS,
            Stroke::new(1.0, border),
            StrokeKind::Inside,
        );
        // CHOOSER-8 — the pin marker: a small accent dot in the top-right corner
        // (a painter primitive, font-independent) at full strength even on a
        // dimmed offline card.
        if favorite {
            ui.painter().circle_filled(
                rect.right_top() + egui::vec2(-Style::SP_S, Style::SP_S),
                Style::SP_XS * 0.75,
                Style::ACCENT_HI,
            );
        }

        if !source.connectable() {
            ui.set_opacity(OFFLINE_OPACITY);
        }
        ui.horizontal(|ui| {
            ui.add_space(Style::SP_S);
            ui.vertical(|ui| {
                ui.set_width(Style::SP_S.mul_add(-2.0, CARD_WIDTH));
                ui.add_space(Style::SP_S);
                card_body(ui, source, recent, thumbs, gate)
            })
            .inner
        })
        .inner
    });
    let controls = scoped.inner;
    let response = scoped.response.on_hover_text(card_tooltip(source));

    // CHOOSER-8 — the per-card context menu (right click). A menu pick takes
    // precedence over the inline controls + the primary click.
    let mut menu_action = None;
    response.context_menu(|ui| card_context_menu(ui, source, favorite, &mut menu_action));
    if menu_action.is_some() {
        return menu_action;
    }
    // An inline button click takes precedence over (and suppresses) the console-open.
    if let Some(op) = controls.power {
        return Some(CardAction::Power {
            id: source.id.clone(),
            op,
        });
    }
    if controls.retry {
        return Some(CardAction::Retry(source.id.clone()));
    }
    // Only a connectable card's primary click activates (lock 14).
    (response.clicked() && source.connectable()).then(|| CardAction::Activate(source.id.clone()))
}

/// The CHOOSER-8 per-card context menu: Connect (connectable only), Pin/Unpin,
/// Retry discovery (offline only), the KVM power ops for a local VM (reusing the
/// CHOOSER-7 state machine), and Edit / Remove for a manual source. Every item is
/// offered only when it can genuinely act (§7). Writes the chosen action into
/// `out` and closes the menu.
fn card_context_menu(
    ui: &mut egui::Ui,
    source: &DesktopSource,
    favorite: bool,
    out: &mut Option<CardAction>,
) {
    if source.connectable() && ui.button("Connect…").clicked() {
        *out = Some(CardAction::Activate(source.id.clone()));
        ui.close_menu();
    }
    let pin_label = if favorite { "Unpin" } else { "Pin to front" };
    if ui.button(pin_label).clicked() {
        *out = Some(CardAction::ToggleFavorite(source.id.clone()));
        ui.close_menu();
    }
    // The offline Retry (lock 14 — a non-blocking discovery re-enumerate, never a
    // probe from here).
    if !source.connectable() && ui.button("Retry discovery").clicked() {
        *out = Some(CardAction::Retry(source.id.clone()));
        ui.close_menu();
    }
    // KVM power — the CHOOSER-7 state-appropriate ops, only for a local VM.
    if source.origin == SourceOrigin::LocalVm {
        let ops = source
            .power_state
            .as_deref()
            .map_or(PowerState::Unknown, PowerState::from_wire)
            .actions();
        if !ops.is_empty() {
            ui.separator();
            for op in ops {
                if ui.button(op.label()).clicked() {
                    *out = Some(CardAction::Power {
                        id: source.id.clone(),
                        op: *op,
                    });
                    ui.close_menu();
                }
            }
        }
    }
    // Manage a manual (operator-added) source.
    if source.origin == SourceOrigin::Manual {
        ui.separator();
        if ui.button("Edit…").clicked() {
            *out = Some(CardAction::EditSource(source.id.clone()));
            ui.close_menu();
        }
        if ui.button("Remove").clicked() {
            *out = Some(CardAction::RemoveSource(source.id.clone()));
            ui.close_menu();
        }
    }
}

/// The card's thumbnail well: the source's decoded live preview when its
/// `thumbnail_ref` resolves (aspect-fit, letterboxed so a 16:9 desktop never
/// stretches), else the honest shared monitor glyph — never a fake screenshot
/// (§7). The decode is bounded + throttled by [`ThumbnailCache`] (Q7).
fn thumbnail_well(ui: &mut egui::Ui, source: &DesktopSource, thumbs: &mut ThumbnailCache) {
    let well = egui::vec2(ui.available_width(), THUMB_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(well, Sense::hover());
    // The recessed plate the icon sat on / the snapshot is letterboxed over.
    ui.painter().rect_filled(rect, Style::RADIUS, Style::BG);
    if let Some(tex) = thumbs.texture_for(ui.ctx(), source) {
        // A live snapshot decoded: aspect-fit (letterbox) inside the well.
        let fit = fit_centered(rect.shrink(Style::SP_XS), tex.size_vec2());
        egui::Image::new(egui::load::SizedTexture::new(tex.id(), fit.size())).paint_at(ui, fit);
    } else {
        // Honest fallback: the shared monitor glyph, never a fake screenshot.
        let glyph = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(Style::SP_XL * 2.0, Style::SP_XL * 1.6),
        );
        crate::session::draw_monitor(&ui.painter().clone(), glyph);
    }
}

/// The largest rect of `img`'s aspect ratio centered inside `bounds` (letterbox
/// fit — never upscale-stretch a snapshot to the well's aspect). A degenerate
/// image size falls back to the full bounds.
fn fit_centered(bounds: egui::Rect, img: egui::Vec2) -> egui::Rect {
    if img.x <= 0.0 || img.y <= 0.0 {
        return bounds;
    }
    let scale = (bounds.width() / img.x).min(bounds.height() / img.y);
    egui::Rect::from_center_size(bounds.center(), egui::vec2(img.x * scale, img.y * scale))
}

/// The card's content rows, top to bottom inside the plate. Returns the inline
/// controls surfaced this frame — a CHOOSER-7 power op on a local-VM card, and/or
/// the CHOOSER-8 offline Retry.
fn card_body(
    ui: &mut egui::Ui,
    source: &DesktopSource,
    recent: bool,
    thumbs: &mut ThumbnailCache,
    gate: Option<&str>,
) -> CardControls {
    thumbnail_well(ui, source, thumbs);
    ui.add_space(Style::SP_XS);

    // Name + (for a VM) its live power state.
    ui.label(
        RichText::new(&source.name)
            .color(Style::TEXT)
            .size(Style::BODY)
            .strong(),
    );
    if let Some(power) = source.power_state.as_deref() {
        let tone = if power.trim() == "running" {
            Style::OK
        } else {
            Style::TEXT_DIM
        };
        ui.colored_label(
            tone,
            RichText::new(format!("vm {power}")).size(Style::SMALL),
        );
    }
    ui.add_space(Style::SP_XS);

    // Protocol badges (design lock 2 — protocol is a per-card badge).
    ui.horizontal(|ui| {
        for offer in &source.protocols {
            protocol_badge(ui, *offer);
            ui.add_space(Style::SP_XS);
        }
    });
    ui.add_space(Style::SP_XS);

    // The status pip + the origin caption; a greyed card carries the
    // worker's reason instead of the caption (lock 14).
    ui.horizontal(|ui| {
        status_dot(ui, source.reachability.pip());
        ui.add_space(Style::SP_XS);
        match source.reason.as_deref() {
            Some(reason) if !source.connectable() => {
                muted_note(ui, reason);
            }
            _ => {
                // CHOOSER-9 — a recently-used desktop reads "recently used" wherever
                // the operator sits (the synced recents cache drives this marker).
                let caption = if recent {
                    format!(
                        "{} \u{00B7} {} \u{00B7} recently used",
                        source.reachability.label(),
                        source.origin.label()
                    )
                } else {
                    format!(
                        "{} \u{00B7} {}",
                        source.reachability.label(),
                        source.origin.label()
                    )
                };
                muted_note(ui, caption);
            }
        }
    });

    let mut controls = CardControls::default();

    // CHOOSER-7 — the local-VM power controls. Only a local VM (this node's
    // libvirt) is powered from here; a peer VM is powered from its own node.
    if source.origin == SourceOrigin::LocalVm {
        ui.add_space(Style::SP_XS);
        controls.power = power_row(ui, source, gate);
    }

    // CHOOSER-8 — the offline Retry affordance (lock 14). A local VM already
    // exposes its Start button (the bring-online path), so Retry is offered on the
    // OTHER offline cards (a peer/LAN endpoint) — a non-blocking discovery
    // re-enumerate, never a probe. It reads at full strength on the dimmed card.
    if !source.connectable() && source.origin != SourceOrigin::LocalVm {
        ui.add_space(Style::SP_XS);
        ui.set_opacity(1.0);
        if ui
            .add(egui::Button::new(RichText::new("Retry").size(Style::SMALL)))
            .on_hover_text("Re-check discovery — nothing is probed from here")
            .clicked()
        {
            controls.retry = true;
        }
    }

    controls
}

/// The local-VM power-control row (CHOOSER-7): buttons appropriate to the VM's
/// live power state — Start a stopped desktop (one click away), Stop/Pause a
/// running one, Resume a paused one. When the node has no local hypervisor
/// (`gate` is `Some`) the buttons render disabled with the honest reason, never a
/// control that pretends to act (§7). Returns the op clicked this frame, if any.
fn power_row(ui: &mut egui::Ui, source: &DesktopSource, gate: Option<&str>) -> Option<PowerOp> {
    let state = source
        .power_state
        .as_deref()
        .map_or(PowerState::Unknown, PowerState::from_wire);
    let ops = state.actions();
    // A live hypervisor + an unmapped state offers no honest action — draw nothing.
    if ops.is_empty() && gate.is_none() {
        return None;
    }
    // Power controls read at full strength even on a dimmed (offline) card — a
    // stopped desktop's Start button must look one click away, not greyed out.
    ui.set_opacity(1.0);
    let enabled = gate.is_none();
    let mut clicked = None;
    ui.horizontal(|ui| {
        for op in ops {
            if ui
                .add_enabled(
                    enabled,
                    egui::Button::new(RichText::new(op.label()).size(Style::SMALL)),
                )
                .clicked()
            {
                clicked = Some(*op);
            }
            ui.add_space(Style::SP_XS);
        }
    });
    if let Some(reason) = gate {
        muted_note(ui, format!("no local hypervisor — {reason}"));
    }
    clicked
}

/// The card tooltip — the honest connection detail (origin, dial address, OS
/// hint when genuinely known).
fn card_tooltip(source: &DesktopSource) -> String {
    let mut text = format!("{} \u{00B7} {}", source.origin.label(), source.host);
    if let Some(os) = source.os_hint.as_deref() {
        text.push_str(" \u{00B7} ");
        text.push_str(os);
    }
    text
}

/// One protocol badge chip. The known port rides the hover (the chip stays a
/// clean three-letter badge — lock 2).
fn protocol_badge(ui: &mut egui::Ui, offer: ProtocolOffer) {
    let galley = ui.painter().layout_no_wrap(
        offer.protocol.badge().to_string(),
        FontId::proportional(Style::SMALL),
        Style::ACCENT_HI,
    );
    let pad = egui::vec2(Style::SP_XS * 2.0, Style::SP_XS);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + pad * 2.0, Sense::hover());
    ui.painter()
        .rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    ui.painter()
        .galley(rect.min + pad, galley, Style::ACCENT_HI);
    if let Some(port) = offer.port {
        let _ = resp.on_hover_text(format!("port {port}"));
    }
}

/// The CHOOSER-4 always-ask connect picker (§7, never a silent stub): a protocol
/// radio row when the source offered several routable protocols (lock 6 — never a
/// silent default), the fullscreen/windowed choice (lock 9), and the single/span-
/// all monitor choice (lock 12), then Connect / Cancel. The radios mutate the live
/// `draft`; §4 chrome via `Style` tokens. Returns the confirm/cancel action.
fn connect_picker(
    ui: &mut egui::Ui,
    source: &DesktopSource,
    draft: &mut ConnectDraft,
) -> Option<CardAction> {
    let mut action = None;
    ui.add_space(Style::SP_M);
    ui.separator();
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new(format!("Connect to {}", source.name))
            .color(Style::TEXT)
            .size(Style::BODY)
            .strong(),
    );
    ui.add_space(Style::SP_XS);

    // The routable offers this source advertises, in the worker's stable order.
    let routable: Vec<VdiProtocol> = source
        .protocols
        .iter()
        .filter_map(|o| o.protocol.route())
        .collect();

    // Protocol — always-ask as a radio row when several are routable (lock 6).
    // A single routable protocol is stated (no false choice) so WHAT will be used
    // is still explicit.
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Protocol")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        if routable.len() > 1 {
            let before = draft.protocol;
            for proto in &routable {
                ui.radio_value(&mut draft.protocol, *proto, proto.label());
            }
            // CHOOSER-6 — a sealed credential is keyed per protocol, so switching
            // protocol invalidates any raised prompt (re-resolved on next Connect).
            if draft.protocol != before {
                draft.cred_prompt = None;
            }
        } else {
            ui.label(RichText::new(draft.protocol.label()).color(Style::TEXT));
        }
    });

    // Display mode — fullscreen or windowed (lock 9).
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Display")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.radio_value(&mut draft.display, DisplayMode::Fullscreen, "Fullscreen");
        ui.radio_value(&mut draft.display, DisplayMode::Windowed, "Windowed");
    });

    // Monitor span — a single display or span all (lock 12).
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Monitors")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.radio_value(&mut draft.monitors, MonitorSpan::Single, "Single display");
        ui.radio_value(&mut draft.monitors, MonitorSpan::All, "Span all");
    });

    // Future protocol routes without a client must say so, never imply a live
    // session (§7).
    if !draft.protocol.has_client() {
        ui.add_space(Style::SP_XS);
        muted_note(
            ui,
            format!(
                "The {} client is not wired yet — the request is recorded, but no session is faked.",
                draft.protocol.label()
            ),
        );
    }

    // CHOOSER-6 — the one-time credential prompt for an external endpoint with no
    // sealed credential yet (raised on the first Connect). Filled once, sealed on
    // the next Connect, then remembered.
    let prompting = draft.cred_prompt.is_some();
    if let Some(prompt) = draft.cred_prompt.as_mut() {
        credential_prompt_fields(ui, prompt);
    }

    ui.add_space(Style::SP_S);
    ui.horizontal(|ui| {
        // Once the prompt is up the Connect action seals the entered credential
        // before connecting — the label says so honestly.
        let connect_label = if prompting {
            format!("Save and connect via {}", draft.protocol.label())
        } else {
            format!("Connect via {}", draft.protocol.label())
        };
        if ui
            .button(RichText::new(connect_label).size(Style::BODY))
            .clicked()
        {
            action = Some(CardAction::Confirm);
        }
        ui.add_space(Style::SP_S);
        if ui
            .button(RichText::new("Cancel").size(Style::BODY))
            .clicked()
        {
            action = Some(CardAction::Cancel);
        }
    });
    action
}

/// The CHOOSER-6 one-time credential fields for an external endpoint: a masked
/// username/password pair under an honest note. §4 `Style` tokens throughout (no
/// raw hex); the password field is masked and the secret is never logged (the
/// [`CredentialPrompt`] buffer redacts through `Debug`).
fn credential_prompt_fields(ui: &mut egui::Ui, prompt: &mut CredentialPrompt) {
    ui.add_space(Style::SP_S);
    ui.separator();
    ui.add_space(Style::SP_XS);
    muted_note(
        ui,
        "This endpoint isn't on the mesh — enter its credentials once. They're sealed in the \
         secret store and remembered for next time; never stored in plaintext.",
    );
    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Username")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add(
            egui::TextEdit::singleline(&mut prompt.username)
                .desired_width(Style::SP_XL * 6.0)
                .hint_text("optional for VNC"),
        );
    });
    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Password")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add(
            egui::TextEdit::singleline(&mut prompt.password)
                .desired_width(Style::SP_XL * 6.0)
                .password(true),
        );
    });
}

/// The CHOOSER-8 manual-source edit form (context-menu → Edit): editable name /
/// host / port / protocol with an inline validation error, then Save / Cancel.
/// Save republishes through the worker's typed add/remove verbs (§6, never a
/// command string). The fields mutate the live `edit` draft; §4 `Style` tokens
/// throughout. Returns the save/cancel action.
fn manual_edit_form(ui: &mut egui::Ui, edit: &mut ManualEdit) -> Option<CardAction> {
    let mut action = None;
    ui.add_space(Style::SP_M);
    ui.separator();
    ui.add_space(Style::SP_S);
    let title = if edit.original_id.is_empty() {
        "Pin a desktop endpoint" // TESTVM-4 ADD mode
    } else {
        "Edit manual desktop"
    };
    ui.label(
        RichText::new(title)
            .color(Style::TEXT)
            .size(Style::BODY)
            .strong(),
    );
    ui.add_space(Style::SP_XS);

    edit_field(
        ui,
        "Name",
        &mut edit.name,
        "optional \u{2014} defaults to host:port",
    );
    edit_field(ui, "Host", &mut edit.host, "10.0.0.5 or host.local");
    edit_field(ui, "Port", &mut edit.port, "3389");

    // Protocol radio row (rdp / vnc / spice).
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Protocol")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        for proto in Protocol::ALL {
            ui.radio_value(&mut edit.protocol, proto, proto.badge());
        }
    });
    ui.add_space(Style::SP_XS);

    // TESTVM-4 — the optional stored credential. Filled → Connect goes straight
    // through with it (a pinned lab/test endpoint); left empty → the CHOOSER-6
    // one-time prompt + sealed store applies, exactly as before.
    edit_field(
        ui,
        "Username",
        &mut edit.username,
        "optional \u{2014} login user (RDP)",
    );
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Password")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add(
            egui::TextEdit::singleline(&mut edit.password)
                .desired_width(Style::SP_XL * 6.0)
                .password(true)
                .hint_text("optional \u{2014} stored with this endpoint"),
        );
    });
    ui.add_space(Style::SP_XS);
    muted_note(
        ui,
        "A stored password rides the roaming chooser prefs and connects with no prompt — \
         meant for lab/test endpoints. Leave it empty to be asked once and sealed in the \
         secret store instead.",
    );

    // The inline validation error (empty host / bad port) — never a silent drop.
    if let Some(err) = edit.error.as_deref() {
        ui.add_space(Style::SP_XS);
        ui.colored_label(Style::DANGER, err);
    }

    ui.add_space(Style::SP_S);
    ui.horizontal(|ui| {
        if ui.button(RichText::new("Save").size(Style::BODY)).clicked() {
            action = Some(CardAction::SaveEdit);
        }
        ui.add_space(Style::SP_S);
        if ui
            .button(RichText::new("Cancel").size(Style::BODY))
            .clicked()
        {
            action = Some(CardAction::CancelEdit);
        }
    });
    action
}

/// One labelled single-line edit row for the manual-source form (§4 tokens).
fn edit_field(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add(
            egui::TextEdit::singleline(value)
                .desired_width(Style::SP_XL * 6.0)
                .hint_text(hint),
        );
    });
    ui.add_space(Style::SP_XS);
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mde_egui::egui::{pos2, vec2, Rect};

    /// A fixture body in the exact `CHOOSER-1` wire shape (the worker's
    /// `DesktopSourcesState` serde output — `snake_case` tags, optional ports
    /// skipped when unknown, `thumbnail_ref` always present + honestly null).
    const FIXTURE: &str = r#"{
        "node": "elm",
        "sources": [
            {
                "id": "peer:oak", "name": "oak", "node": "oak", "host": "10.42.0.7",
                "protocols": [
                    {"protocol": "rdp", "port": 3389},
                    {"protocol": "vnc", "port": 5900}
                ],
                "origin": "mesh_peer", "reachability": "reachable",
                "os_hint": "linux", "thumbnail_ref": null
            },
            {
                "id": "peer-vm:oak:win11", "name": "win11", "node": "oak", "host": "10.42.0.7",
                "protocols": [{"protocol": "spice"}],
                "origin": "mesh_peer", "reachability": "unreachable",
                "reason": "vm shut off", "power_state": "shut off", "thumbnail_ref": null
            },
            {
                "id": "mdns:192.168.1.60:3389:rdp", "name": "OfficePC",
                "node": "192.168.1.60", "host": "192.168.1.60",
                "protocols": [{"protocol": "rdp", "port": 3389}],
                "origin": "mdns", "reachability": "reachable", "thumbnail_ref": null
            }
        ],
        "lanes": [
            {"lane": "mesh-registry", "status": "ok"},
            {"lane": "mdns", "status": "ok (3 types)"},
            {"lane": "local-kvm", "status": "gated: virsh not found"},
            {"lane": "manual", "status": "ok (0 sources)"}
        ],
        "published_at_ms": 1720000000000
    }"#;

    /// An in-memory [`DesktopSourcesClient`] with a canned roster.
    struct FakeSources(Option<DesktopSourcesState>);

    impl DesktopSourcesClient for FakeSources {
        fn latest(&self) -> Option<DesktopSourcesState> {
            self.0.clone()
        }

        fn has_bus(&self) -> bool {
            true
        }
    }

    /// A CHOOSER-6 credential store the integration tests share (via `Rc`) to
    /// seed + assert seals through a live [`ChooserState`]. It does a REAL
    /// seal→store→read round-trip so "prompt once then remember" is exercised
    /// end to end.
    #[derive(Clone, Default)]
    struct RecordingStore {
        inner: std::rc::Rc<std::cell::RefCell<HashMap<String, crate::auth::Credential>>>,
    }

    impl RecordingStore {
        /// The credential sealed under `store_ref`, if any (the round-trip proof).
        fn get_ref(&self, store_ref: &str) -> Option<crate::auth::Credential> {
            self.inner.borrow().get(store_ref).cloned()
        }

        /// Number of remembered credentials in the fake store.
        fn seal_count(&self) -> usize {
            self.inner.borrow().len()
        }
    }

    impl CredentialStore for RecordingStore {
        fn get(&self, store_ref: &str) -> Result<Option<crate::auth::Credential>, String> {
            Ok(self.inner.borrow().get(store_ref).cloned())
        }

        fn seal(&self, store_ref: &str, credential: &crate::auth::Credential) -> SealOutcome {
            self.inner
                .borrow_mut()
                .insert(store_ref.to_string(), credential.clone());
            SealOutcome::Sealed
        }
    }

    /// An inert CHOOSER-9 prefs session (its workgroup root is unprovisioned, so it
    /// is a silent no-op) — favorites/recents still track session-locally, exactly
    /// as an offline seat behaves, so the pre-CHOOSER-9 tests are unaffected.
    fn inert_prefs() -> ChooserPrefs {
        chooser_prefs::ChooserPrefs::new(
            chooser_prefs::ChooserPrefsStore::new(PathBuf::from("/no/such/mesh/root")),
            "matthew",
            "seat-test",
        )
    }

    /// A CHOOSER-9 prefs session over an explicit workgroup root + seat (the
    /// two-seat sync tests point two of these at one shared tempdir).
    fn prefs_at(root: PathBuf, seat: &str) -> ChooserPrefs {
        chooser_prefs::ChooserPrefs::new(
            chooser_prefs::ChooserPrefsStore::new(root),
            "matthew",
            seat,
        )
    }

    /// A `ChooserState` over a canned roster, with no publish root (the
    /// broker publish then records its honest error) and a fixed peer name. Most
    /// tests exercise mesh-peer sources (SSO), so the honest-gated production
    /// credential store is fine; the external-cred tests inject their own store.
    fn state_with(state: Option<DesktopSourcesState>) -> ChooserState {
        state_with_store(state, Box::new(MeshCredentialStore))
    }

    /// [`state_with`] over an explicit credential store (the CHOOSER-6 seam).
    fn state_with_store(
        state: Option<DesktopSourcesState>,
        creds: Box<dyn CredentialStore>,
    ) -> ChooserState {
        let mut s = ChooserState::with_client(
            Box::new(FakeSources(state)),
            None,
            "client-node".to_string(),
            creds,
            inert_prefs(),
        );
        s.refresh();
        s
    }

    fn fixture_state() -> DesktopSourcesState {
        parse_sources(FIXTURE).expect("the fixture decodes")
    }

    /// A minimal source row for fold/connect tests.
    fn source(id: &str, node: &str, protocols: &[Protocol]) -> DesktopSource {
        DesktopSource {
            id: id.to_string(),
            name: id.rsplit(':').next().unwrap_or(id).to_string(),
            node: node.to_string(),
            host: node.to_string(),
            protocols: protocols
                .iter()
                .map(|p| ProtocolOffer {
                    protocol: *p,
                    port: None,
                })
                .collect(),
            origin: SourceOrigin::MeshPeer,
            reachability: Reachability::Reachable,
            reason: None,
            os_hint: None,
            power_state: None,
            thumbnail_ref: None,
        }
    }

    fn roster(sources: Vec<DesktopSource>) -> DesktopSourcesState {
        DesktopSourcesState {
            sources,
            lanes: vec![],
        }
    }

    /// Encode a `w×h` opaque-grey RGBA PNG with the same `png` crate the shell
    /// decoder uses, so the thumbnail plumbing is driven end to end by a REAL
    /// snapshot (no opaque fixture blob).
    fn tiny_png(w: u32, h: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut bytes, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc.write_header().expect("png header");
            let px = vec![200u8; w as usize * h as usize * 4];
            writer.write_image_data(&px).expect("png data");
        }
        bytes
    }

    /// Wrap PNG bytes as the `data:image/png;base64,…` ref the worker inlines on
    /// the state plane — the exact shape [`decode_thumbnail_ref`] resolves.
    fn png_data_uri(png: &[u8]) -> String {
        use base64::Engine;
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(png)
        )
    }

    // ── the wire mirror ──

    #[test]
    fn topic_matches_the_worker_contract() {
        // Cross-check: MUST equal mackesd::workers::desktop_sources::SOURCES_TOPIC.
        assert_eq!(SOURCES_TOPIC, "state/desktops/sources");
    }

    #[test]
    fn the_chooser1_fixture_parses_to_the_projected_shape() {
        let state = fixture_state();
        assert_eq!(state.sources.len(), 3);

        // The peer seat: two offers with their well-known ports, an OS hint.
        let seat = &state.sources[0];
        assert_eq!(seat.id, "peer:oak");
        assert_eq!(seat.node, "oak");
        assert_eq!(seat.origin, SourceOrigin::MeshPeer);
        assert_eq!(seat.reachability, Reachability::Reachable);
        assert_eq!(seat.protocols.len(), 2);
        assert_eq!(seat.protocols[0].protocol, Protocol::Rdp);
        assert_eq!(seat.protocols[0].port, Some(3389));
        assert_eq!(seat.os_hint.as_deref(), Some("linux"));
        assert!(seat.thumbnail_ref.is_none(), "honestly null (CHOOSER-3)");
        assert!(seat.connectable());

        // The stopped VM: a brokered Spice offer (no port on the wire), the
        // worker's grey reason + power state, NOT connectable.
        let vm = &state.sources[1];
        assert_eq!(
            vm.protocols,
            vec![ProtocolOffer {
                protocol: Protocol::Spice,
                port: None
            }]
        );
        assert_eq!(vm.reachability, Reachability::Unreachable);
        assert_eq!(vm.reason.as_deref(), Some("vm shut off"));
        assert_eq!(vm.power_state.as_deref(), Some("shut off"));
        assert!(!vm.connectable());

        // The LAN endpoint.
        assert_eq!(state.sources[2].origin, SourceOrigin::Mdns);

        // The lanes, with the degraded one detectable.
        assert_eq!(state.lanes.len(), 4);
        let degraded: Vec<&str> = state
            .lanes
            .iter()
            .filter(|l| l.is_degraded())
            .map(|l| l.lane.as_str())
            .collect();
        assert_eq!(degraded, vec!["local-kvm"]);
    }

    #[test]
    fn unknown_tags_degrade_honestly_instead_of_failing_the_parse() {
        // A future worker minting a new protocol / lane / reachability tag
        // must not blank the whole roster: the mirrors degrade per-field.
        let raw = r#"{
            "sources": [{
                "id": "x", "name": "x", "node": "n", "host": "n",
                "protocols": [{"protocol": "quic-desktop"}],
                "origin": "carrier-pigeon", "reachability": "flaky",
                "thumbnail_ref": null
            }],
            "lanes": []
        }"#;
        let state = parse_sources(raw).expect("degrades, not fails");
        let s = &state.sources[0];
        assert_eq!(s.protocols[0].protocol, Protocol::Unknown);
        assert_eq!(s.origin, SourceOrigin::Unknown);
        assert_eq!(s.reachability, Reachability::Unknown);
        assert!(s.connectable(), "an honest Unknown may try");
    }

    #[test]
    fn malformed_state_is_an_honest_none() {
        assert!(parse_sources("not json").is_none());
    }

    #[test]
    fn bus_client_without_a_root_reads_none_and_reports_no_bus() {
        let client = BusDesktopSources::with_root(None);
        assert!(client.latest().is_none(), "no Bus dir → an honest None");
        assert!(!client.has_bus());
    }

    // ── grouping ──

    #[test]
    fn group_by_node_folds_consecutive_runs_in_published_order() {
        let state = fixture_state();
        let groups = group_by_node(&state.sources);
        let shape: Vec<(&str, usize)> = groups.iter().map(|(n, m)| (*n, m.len())).collect();
        // The worker sorts by node: 192.168.1.60 < oak — but the fixture is
        // in oak-first order, and grouping preserves the PUBLISHED order
        // (the worker owns the sort; the surface must not re-order it).
        assert_eq!(shape, vec![("oak", 2), ("192.168.1.60", 1)]);
    }

    // ── the seen-set / auto-popup fold (design lock 1) ──

    #[test]
    fn first_fold_seeds_silently_then_a_new_source_pops_once() {
        let mut state = state_with(Some(roster(vec![source(
            "peer:oak",
            "oak",
            &[Protocol::Rdp],
        )])));
        // The pre-existing world seeds the seen set without a popup.
        assert!(!state.take_popup(), "startup must not pop the Chooser");

        // The same roster again: nothing new, no popup.
        state.fold_sources(roster(vec![source("peer:oak", "oak", &[Protocol::Rdp])]));
        assert!(!state.take_popup());

        // A genuinely new source pops — once.
        state.fold_sources(roster(vec![
            source("peer:oak", "oak", &[Protocol::Rdp]),
            source("vm:elm:dev", "elm", &[Protocol::Spice]),
        ]));
        assert!(state.take_popup(), "a new source raises the popup");
        assert!(!state.take_popup(), "the popup drains once");
    }

    #[test]
    fn a_source_that_left_and_returned_does_not_repop() {
        let mut state = state_with(Some(roster(vec![source(
            "peer:oak",
            "oak",
            &[Protocol::Rdp],
        )])));
        let _ = state.take_popup();
        // oak flaps away and back: the operator already saw it — no re-pop.
        state.fold_sources(roster(vec![]));
        state.fold_sources(roster(vec![source("peer:oak", "oak", &[Protocol::Rdp])]));
        assert!(!state.take_popup(), "a seen source must not re-pop");
    }

    // ── the connect flow (CHOOSER-4) ──

    #[test]
    fn the_protocol_route_maps_wire_tags_to_vdi_routes() {
        // The routing fold: each renderable wire tag maps to its VDI route; an
        // unknown tag has none (badged, never connected blind — §7).
        assert_eq!(Protocol::Rdp.route(), Some(VdiProtocol::Rdp));
        assert_eq!(Protocol::Vnc.route(), Some(VdiProtocol::Vnc));
        assert_eq!(Protocol::Spice.route(), Some(VdiProtocol::Spice));
        assert_eq!(Protocol::Unknown.route(), None);
    }

    #[test]
    fn a_single_protocol_source_still_asks_display_options_then_hands_off_once() {
        let mut state = state_with(Some(roster(vec![source(
            "peer-vm:oak:web1",
            "oak",
            &[Protocol::Spice],
        )])));
        let sources = state.sources_snapshot();

        // Even a single protocol opens the picker: fullscreen/windowed + the
        // monitor span are per-connection choices (locks 9/12), so activate must
        // NOT connect — it seeds the draft to the one offer.
        state.activate(&sources, "peer-vm:oak:web1");
        assert!(
            state.take_connect().is_none(),
            "activate opens the picker, not a connect"
        );
        assert_eq!(
            state.pending.as_ref().map(|d| d.protocol),
            Some(VdiProtocol::Spice)
        );

        state.confirm_connect(&sources);
        // The broker publish had no Bus root → the honest inline error (the same
        // discipline as the E12-5b picker), but the Desktop hand-off still
        // happens so the surface reflects the pending connect.
        assert!(state
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("Bus")));
        let req = state.take_connect().expect("a request was handed off");
        assert_eq!(req.target.serving_peer, "oak");
        assert_eq!(req.target.name, "web1");
        assert_eq!(req.protocol, VdiProtocol::Spice);
        assert_eq!(req.display, DisplayMode::Fullscreen, "seeded to fullscreen");
        assert_eq!(
            req.monitors,
            MonitorSpan::Single,
            "seeded to single display"
        );
        assert!(state.take_connect().is_none(), "the hand-off drains once");
        // The Spice route is live-client-capable now, so the note should describe
        // the brokered request without a stale CHOOSER-5 gate.
        assert!(state
            .note
            .as_deref()
            .is_some_and(|n| n.contains("brokering over the mesh") && !n.contains("CHOOSER-5")));
    }

    /// Fold a single external (mDNS) RDP endpoint into a fresh `ChooserState`
    /// backed by `creds`, and return it + the source id.
    fn external_state(creds: Box<dyn CredentialStore>) -> (ChooserState, String) {
        let mut state = state_with_store(None, creds);
        let mut lan = source(
            "mdns:192.168.1.60:3389:rdp",
            "192.168.1.60",
            &[Protocol::Rdp],
        );
        lan.origin = SourceOrigin::Mdns;
        lan.name = "OfficePC".to_string();
        // The dial address the credential ref is derived from (host:port).
        lan.host = "192.168.1.60:3389".to_string();
        state.fold_sources(roster(vec![lan]));
        (state, "mdns:192.168.1.60:3389:rdp".to_string())
    }

    #[test]
    fn an_external_endpoint_prompts_once_seals_then_connects_without_a_broker_open() {
        // CHOOSER-6 — the full external fold: activate → the first Connect resolves
        // no sealed credential and raises a one-time prompt (does NOT connect) →
        // the operator fills it → the next Connect seals it + connects.
        let store = RecordingStore::default();
        let (mut state, id) = external_state(Box::new(store.clone()));
        let sources = state.sources_snapshot();

        // Phase 1: activate + Connect → the prompt is raised, nothing connects.
        state.activate(&sources, &id);
        state.confirm_connect(&sources);
        assert!(
            state.take_connect().is_none(),
            "an external endpoint with no sealed credential must not connect blind"
        );
        assert!(
            state
                .pending
                .as_ref()
                .is_some_and(|d| d.cred_prompt.is_some()),
            "the one-time credential prompt is raised"
        );

        // The operator fills the prompt once.
        {
            let prompt = state
                .pending
                .as_mut()
                .and_then(|d| d.cred_prompt.as_mut())
                .expect("the prompt is open");
            prompt.username = "administrator".to_string();
            prompt.password = "s3cr3t-pw".to_string();
        }

        // Phase 2: Connect → seals the credential + connects (no broker verb, no
        // Bus error — an external endpoint has no broker `Open`).
        state.confirm_connect(&sources);
        assert!(
            state.last_error.is_none(),
            "no broker verb for an off-mesh endpoint"
        );
        assert!(state.pending.is_none(), "the picker closes on connect");

        // The credential really round-tripped into the store (sealed), under the
        // derived `desktop/<host>/<proto>` ref.
        let sealed = store
            .get_ref("desktop/192.168.1.60:3389/rdp")
            .expect("the credential was sealed");
        assert_eq!(sealed.username, "administrator");
        assert_eq!(sealed.secret.expose(), "s3cr3t-pw");

        // The note names the gated direct-transport leg + that the credential is
        // sealed (remembered), and never leaks the secret.
        let note = state.note.clone().expect("a connect note");
        assert!(note.contains("RDP") && note.contains("E12-4"));
        assert!(note.contains("sealed") && note.contains("remembered"));
        assert!(!note.contains("s3cr3t-pw"), "the note leaked the secret");

        // The request carries the resolved sealed auth (secret redacted from Debug).
        let req = state.take_connect().expect("hand-off");
        assert_eq!(req.target.name, "OfficePC");
        assert_eq!(req.protocol, VdiProtocol::Rdp);
        assert_eq!(
            req.target
                .endpoint
                .as_ref()
                .map(|e| (e.host.as_str(), e.port)),
            Some(("192.168.1.60", 3389))
        );
        assert!(matches!(req.auth, DesktopAuth::Sealed { .. }));
        assert!(!format!("{req:?}").contains("s3cr3t-pw"));
    }

    #[test]
    fn a_remembered_external_credential_connects_without_a_second_prompt() {
        // A store that already holds the sealed credential: activate + one Connect
        // connects straight through, no prompt (the "then remembered" half).
        let store = RecordingStore::default();
        assert!(matches!(
            store.seal(
                "desktop/192.168.1.60:3389/rdp",
                &crate::auth::Credential::new("administrator", "s3cr3t-pw"),
            ),
            SealOutcome::Sealed
        ));
        let (mut state, id) = external_state(Box::new(store));
        let sources = state.sources_snapshot();

        state.activate(&sources, &id);
        state.confirm_connect(&sources);
        // No prompt was raised (the credential was remembered) and it connected.
        assert!(state.pending.is_none(), "a remembered cred needs no prompt");
        let req = state
            .take_connect()
            .expect("connects with the remembered cred");
        let DesktopAuth::Sealed { credential, .. } = req.auth else {
            unreachable!("expected the remembered sealed cred")
        };
        assert_eq!(credential.username, "administrator");
        assert_eq!(credential.secret.expose(), "s3cr3t-pw");
    }

    #[test]
    fn a_mesh_peer_connects_with_no_credential_prompt_via_sso() {
        // The SSO path: a mesh-brokered peer connects with the node's mesh identity
        // and no prompt is raised when there is no remembered guest credential.
        let store = RecordingStore::default();
        let mut state = state_with_store(
            Some(roster(vec![source("peer:oak", "oak", &[Protocol::Rdp])])),
            Box::new(store.clone()),
        );
        let sources = state.sources_snapshot();
        state.activate(&sources, "peer:oak");
        state.confirm_connect(&sources);
        assert!(state.pending.is_none(), "SSO needs no credential prompt");
        let req = state.take_connect().expect("SSO connects straight through");
        let DesktopAuth::MeshIdentity { node, guest } = req.auth else {
            unreachable!("expected mesh-identity SSO")
        };
        assert_eq!(node, "client-node");
        assert!(
            guest.is_none(),
            "no remembered guest credential was present"
        );
        assert_eq!(store.seal_count(), 0, "SSO resolution must not seal");
        // The broker publish had no Bus root → the honest inline error (mesh peer),
        // and the note names SSO, never a credential.
        assert!(
            req.broker_session.is_none(),
            "no lifecycle handle is attached when the Open publish had no Bus"
        );
        assert!(state.note.as_deref().is_some_and(|n| n.contains("SSO")));
    }

    #[test]
    fn a_mesh_peer_connect_keeps_the_published_broker_session_id() {
        let dir = temp_bus_dir("vdi-open");
        let mut state = state_with_bus(
            Some(roster(vec![source("peer:oak", "oak", &[Protocol::Rdp])])),
            Some(dir.clone()),
        );
        let sources = state.sources_snapshot();
        state.activate(&sources, "peer:oak");
        state.confirm_connect(&sources);
        let req = state.take_connect().expect("SSO connects straight through");
        let broker = req
            .broker_session
            .as_ref()
            .expect("successful broker Open attaches lifecycle metadata");
        let persist = mde_bus::persist::Persist::open(dir.clone()).expect("open bus");
        let msgs = persist
            .list_since("action/vdi/session", None)
            .expect("list");
        assert_eq!(msgs.len(), 1);
        let body = msgs[0].body.as_deref().expect("body");
        let v: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(v["op"], "open");
        assert_eq!(v["id"], broker.id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_mesh_peer_carries_the_overlay_endpoint_into_the_vdi_request() {
        let mut oak = source("peer:oak", "oak", &[Protocol::Rdp]);
        oak.host = "10.42.0.7".to_string();
        oak.protocols = vec![ProtocolOffer {
            protocol: Protocol::Rdp,
            port: Some(3389),
        }];
        let mut state =
            state_with_store(Some(roster(vec![oak])), Box::new(RecordingStore::default()));
        let sources = state.sources_snapshot();
        state.activate(&sources, "peer:oak");
        state.confirm_connect(&sources);
        let req = state.take_connect().expect("SSO connects straight through");
        assert_eq!(
            req.target
                .endpoint
                .as_ref()
                .map(|endpoint| (endpoint.host.as_str(), endpoint.port)),
            Some(("10.42.0.7", 3389)),
            "worker-published overlay host + RDP port must reach live-vdi"
        );
    }

    #[test]
    fn a_mesh_peer_can_carry_a_remembered_guest_credential_for_live_rdp() {
        let dir = temp_bus_dir("vdi-open-guest");
        let store = RecordingStore::default();
        assert_eq!(
            store.seal(
                "desktop/oak/rdp",
                &crate::auth::Credential::new("administrator", "mesh-rdp-pw"),
            ),
            SealOutcome::Sealed
        );
        let mut state = ChooserState::with_client(
            Box::new(FakeSources(Some(roster(vec![source(
                "peer:oak",
                "oak",
                &[Protocol::Rdp],
            )])))),
            Some(dir.clone()),
            "client-node".to_string(),
            Box::new(store),
            inert_prefs(),
        );
        state.refresh();
        let sources = state.sources_snapshot();
        state.activate(&sources, "peer:oak");
        state.confirm_connect(&sources);
        let req = state.take_connect().expect("SSO connects straight through");
        assert!(
            req.broker_session.is_some(),
            "mesh guest login still keeps broker lifecycle tracking"
        );
        assert!(!format!("{req:?}").contains("mesh-rdp-pw"));
        let DesktopAuth::MeshIdentity {
            node,
            guest: Some(guest),
        } = &req.auth
        else {
            unreachable!("expected mesh identity plus remembered guest credential")
        };
        assert_eq!(node, "client-node");
        assert_eq!(guest.store_ref, "desktop/oak/rdp");
        assert_eq!(guest.credential.username, "administrator");
        assert_eq!(guest.credential.secret.expose(), "mesh-rdp-pw");
        assert!(
            state
                .note
                .as_deref()
                .is_some_and(|n| n.contains("sealed guest credential")),
            "the note names guest auth without leaking the secret"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_production_credential_store_gate_is_honest_on_an_external_connect() {
        // On the live fleet the seal is gated: an external connect still hands off
        // (the entered credential drives the session in-memory) but the note says
        // it isn't remembered — never a faked "sealed" (§7).
        let (mut state, id) = external_state(Box::new(MeshCredentialStore));
        let sources = state.sources_snapshot();
        state.activate(&sources, &id);
        state.confirm_connect(&sources); // phase 1 → prompt
        {
            let prompt = state
                .pending
                .as_mut()
                .and_then(|d| d.cred_prompt.as_mut())
                .expect("prompt open");
            prompt.password = "in-memory-only".to_string();
        }
        state.confirm_connect(&sources); // phase 2 → gated seal + connect
        let note = state.note.clone().expect("note");
        assert!(
            note.contains("isn't remembered"),
            "a gated seal is honest, not faked as remembered: {note}"
        );
        assert!(
            !note.contains("in-memory-only"),
            "the note leaked the secret"
        );
        assert!(
            state.take_connect().is_some(),
            "the session still hands off"
        );
    }

    #[test]
    fn an_offline_source_never_connects() {
        let mut off = source("peer:ash", "ash", &[Protocol::Rdp]);
        off.reachability = Reachability::Unreachable;
        off.reason = Some("peer unreachable".to_string());
        let mut state = state_with(Some(roster(vec![off])));
        let sources = state.sources_snapshot();
        state.activate(&sources, "peer:ash");
        assert!(state.take_connect().is_none(), "greyed cards don't connect");
        assert!(
            state.pending.is_none(),
            "greyed cards don't open the picker"
        );
    }

    #[test]
    fn an_unknown_only_source_offers_no_connectable_protocol() {
        // A source advertising only a tag this build can't route: activation opens
        // no picker and says so honestly — never a blind connect (§7).
        let mut state = state_with(Some(roster(vec![source(
            "peer:oak",
            "oak",
            &[Protocol::Unknown],
        )])));
        let sources = state.sources_snapshot();
        state.activate(&sources, "peer:oak");
        assert!(state.pending.is_none(), "no routable protocol → no picker");
        assert!(state.take_connect().is_none());
        assert!(state
            .note
            .as_deref()
            .is_some_and(|n| n.contains("no connectable protocol")));
    }

    #[test]
    fn the_picker_seeds_the_first_routable_offer_skipping_unknown() {
        // [Unknown, Rdp]: the unknown tag is badged but never routed — the picker
        // seeds to RDP (the first routable offer).
        let mut state = state_with(Some(roster(vec![source(
            "peer:oak",
            "oak",
            &[Protocol::Unknown, Protocol::Rdp],
        )])));
        let sources = state.sources_snapshot();
        state.activate(&sources, "peer:oak");
        assert_eq!(
            state.pending.as_ref().map(|d| d.protocol),
            Some(VdiProtocol::Rdp)
        );
    }

    #[test]
    fn a_multi_protocol_source_asks_the_protocol_and_connects_only_on_confirm() {
        let mut state = state_with(Some(roster(vec![source(
            "peer:oak",
            "oak",
            &[Protocol::Rdp, Protocol::Vnc],
        )])));
        let sources = state.sources_snapshot();

        // Activation raises the CHOOSER-4 picker seeded to the first offer — it
        // must NOT connect (lock 6 — always-ask, never a silent first-pick).
        state.activate(&sources, "peer:oak");
        assert_eq!(
            state.pending.as_ref().map(|d| d.source_id.as_str()),
            Some("peer:oak")
        );
        assert_eq!(
            state.pending.as_ref().map(|d| d.protocol),
            Some(VdiProtocol::Rdp)
        );
        assert!(state.take_connect().is_none(), "no silent first-pick");

        // Cancel backs out.
        state.cancel_connect();
        assert!(state.pending.is_none());

        // Ask again, pick VNC + windowed + span-all, then confirm — the request
        // is built from exactly those choices (the CHOOSER-4 construction fold).
        state.activate(&sources, "peer:oak");
        {
            let draft = state.pending.as_mut().expect("the picker is open");
            draft.protocol = VdiProtocol::Vnc;
            draft.display = DisplayMode::Windowed;
            draft.monitors = MonitorSpan::All;
        }
        state.confirm_connect(&sources);
        assert!(state.pending.is_none());
        let req = state.take_connect().expect("confirm connects");
        assert_eq!(req.target.serving_peer, "oak");
        assert_eq!(req.protocol, VdiProtocol::Vnc);
        assert_eq!(req.display, DisplayMode::Windowed);
        assert_eq!(req.monitors, MonitorSpan::All);
    }

    // ── headless mount renders (the DRM runner's path, minus the GPU) ──

    /// Drive one headless 960×640 frame of `chooser_panel` and tessellate it
    /// on the CPU — the same `Context::run` → `tessellate` path the DRM
    /// runner drives. Returns whether it produced draw primitives.
    fn run_panel(state: &mut ChooserState) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| chooser_panel(ui, state));
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        !prims.is_empty()
    }

    #[test]
    fn an_empty_roster_renders_the_backdrop_with_the_honest_reason() {
        // A published-but-quiet roster: the BRAND-1 hero + the quiet-lane copy.
        let mut state = state_with(Some(DesktopSourcesState {
            sources: vec![],
            lanes: vec![LaneStatus {
                lane: "local-kvm".to_string(),
                status: "gated: virsh not found".to_string(),
            }],
        }));
        let (title, detail) = state.empty_copy();
        assert_eq!(title, "No desktops discovered");
        assert!(
            detail.contains("local-kvm") && detail.contains("gated"),
            "the quiet lane is named: {detail}"
        );
        assert!(
            run_panel(&mut state),
            "the empty Chooser backdrop produced no draw primitives"
        );
        assert!(state.take_connect().is_none());

        // No published record yet is a DIFFERENT honest truth.
        let mut unreported = state_with(None);
        let (title, _) = unreported.empty_copy();
        assert_eq!(title, "Desktop discovery hasn't reported yet");
        assert!(run_panel(&mut unreported));
    }

    #[test]
    fn a_missing_bus_reads_as_gated_not_as_a_quiet_mesh() {
        // §7 — a gated read must not render as a live-looking "no desktops".
        let state = ChooserState::with_client(
            Box::new(BusDesktopSources::with_root(None)),
            None,
            "client-node".to_string(),
            Box::new(MeshCredentialStore),
            inert_prefs(),
        );
        let (title, detail) = state.empty_copy();
        assert_eq!(title, "Desktop discovery unavailable");
        assert!(detail.contains("Bus") && detail.contains("unblocks"));
    }

    #[test]
    fn a_populated_roster_renders_the_grouped_card_grid() {
        let mut state = state_with(Some(fixture_state()));
        assert!(
            run_panel(&mut state),
            "the card grid produced no draw primitives"
        );
    }

    #[test]
    fn an_offline_source_renders_greyed_with_its_reason() {
        // The fixture's stopped VM is the greyed card; the render must
        // tessellate (the grey path draws real geometry + the reason).
        let mut state = state_with(Some(roster(vec![{
            let mut vm = source("peer-vm:oak:win11", "oak", &[Protocol::Spice]);
            vm.reachability = Reachability::Unreachable;
            vm.reason = Some("vm shut off".to_string());
            vm.power_state = Some("shut off".to_string());
            vm
        }])));
        assert!(
            run_panel(&mut state),
            "the offline-greyed card produced no draw primitives"
        );
    }

    #[test]
    fn the_raised_connect_picker_renders_the_chooser4_affordance() {
        let mut state = state_with(Some(roster(vec![source(
            "peer:oak",
            "oak",
            &[Protocol::Rdp, Protocol::Vnc],
        )])));
        let sources = state.sources_snapshot();
        state.activate(&sources, "peer:oak");
        assert!(
            run_panel(&mut state),
            "the connect-picker affordance produced no draw primitives"
        );
        // Rendering the picker is not a connect.
        assert!(state.take_connect().is_none());
    }

    #[test]
    fn the_external_credential_prompt_renders_with_masked_fields() {
        // CHOOSER-6 — an external endpoint whose first Connect found no sealed
        // credential renders the one-time username/password prompt (§4 tokens); it
        // tessellates and still hasn't connected (nothing connects blind).
        let store = RecordingStore::default();
        let (mut state, id) = external_state(Box::new(store));
        let sources = state.sources_snapshot();
        state.activate(&sources, &id);
        state.confirm_connect(&sources); // raise the prompt
        assert!(
            state
                .pending
                .as_ref()
                .is_some_and(|d| d.cred_prompt.is_some()),
            "the credential prompt is raised"
        );
        assert!(
            run_panel(&mut state),
            "the credential-prompt picker produced no draw primitives"
        );
        assert!(
            state.take_connect().is_none(),
            "rendering the prompt is not a connect"
        );
    }

    #[test]
    fn a_spice_picker_renders_without_a_stale_chooser5_gate() {
        // A Spice-only source: the picker renders through the normal live-client
        // path and does not show the retired CHOOSER-5 gate.
        let mut state = state_with(Some(roster(vec![source(
            "peer-vm:oak:win11",
            "oak",
            &[Protocol::Spice],
        )])));
        let sources = state.sources_snapshot();
        state.activate(&sources, "peer-vm:oak:win11");
        assert!(
            run_panel(&mut state),
            "the Spice picker produced no draw primitives"
        );
        assert!(state
            .pending
            .as_ref()
            .is_some_and(|draft| draft.protocol == VdiProtocol::Spice));
        assert!(state.take_connect().is_none(), "rendering is not a connect");
    }

    // ── CHOOSER-3: the thumbnail decode + bounded/throttled cache ──

    #[test]
    fn a_png_data_uri_ref_decodes_to_an_image_of_the_right_size() {
        let img = decode_data_uri_png(&png_data_uri(&tiny_png(4, 3)))
            .expect("a valid base64 PNG data URI decodes");
        assert_eq!(img.size, [4, 3], "the decode keeps the snapshot dimensions");
    }

    #[test]
    fn a_malformed_or_unsupported_ref_is_an_honest_none() {
        // Not a data URI at all.
        assert!(decode_data_uri_png("not a data uri").is_none());
        // A data URI, but not base64-encoded.
        assert!(decode_data_uri_png("data:image/png,QUJD").is_none());
        // A mediatype the shell doesn't decode (only PNG snapshots).
        assert!(decode_data_uri_png("data:image/jpeg;base64,QUJD").is_none());
        // Well-formed base64 whose bytes are not a PNG (`QUJD` == "ABC").
        assert!(decode_data_uri_png("data:image/png;base64,QUJD").is_none());
        // Garbage base64 payload.
        assert!(decode_data_uri_png("data:image/png;base64,%%%not-base64%%%").is_none());
    }

    #[test]
    fn source_to_thumbnail_plumbing_decodes_a_ref_and_falls_back_without_one() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut cache = ThumbnailCache::default();

        // A source carrying a real snapshot ref → a decoded, uploaded texture.
        let mut with_thumb = source("peer:oak", "oak", &[Protocol::Rdp]);
        with_thumb.thumbnail_ref = Some(png_data_uri(&tiny_png(6, 4)));
        let tex = cache
            .texture_for(&ctx, &with_thumb)
            .expect("a real snapshot ref resolves to a texture");
        assert_eq!(tex.size(), [6, 4], "the well shows the decoded snapshot");

        // A second frame with the SAME ref must NOT re-decode — the cached
        // handle (same texture id) is returned (Q7: never decode per frame).
        let again = cache
            .texture_for(&ctx, &with_thumb)
            .expect("the cached texture is returned");
        assert_eq!(again.id(), tex.id(), "an unchanged ref reuses the cache");

        // A source WITHOUT a ref → no texture → the honest monitor-icon fallback.
        let bare = source("peer:elm", "elm", &[Protocol::Rdp]);
        assert!(
            cache.texture_for(&ctx, &bare).is_none(),
            "no ref → the icon fallback, never a fake preview (§7)"
        );
    }

    #[test]
    fn the_decode_gate_is_first_sight_then_change_plus_throttle() {
        let t0 = Instant::now();
        let slot = ThumbSlot {
            ref_key: Some("snap-a".to_string()),
            texture: None,
            decoded_at: t0,
            used: 0,
        };
        // Never-seen source: decode now.
        assert!(ThumbnailCache::needs_decode(None, Some("snap-a"), t0));
        assert!(
            ThumbnailCache::needs_decode(None, None, t0),
            "a no-ref miss is cached too"
        );
        // Same ref: never re-decode (this is the per-frame no-op).
        assert!(!ThumbnailCache::needs_decode(
            Some(&slot),
            Some("snap-a"),
            t0
        ));
        // Changed ref but within the throttle window: keep the (stale) cache.
        assert!(!ThumbnailCache::needs_decode(
            Some(&slot),
            Some("snap-b"),
            t0
        ));
        // Changed ref AND the throttle window elapsed: re-decode the new snapshot.
        let later = t0 + THUMB_MIN_DECODE_INTERVAL + Duration::from_secs(1);
        assert!(ThumbnailCache::needs_decode(
            Some(&slot),
            Some("snap-b"),
            later
        ));
        // …but an unchanged ref stays a no-op even past the window.
        assert!(!ThumbnailCache::needs_decode(
            Some(&slot),
            Some("snap-a"),
            later
        ));
    }

    #[test]
    fn the_cache_is_lru_bounded() {
        let ctx = egui::Context::default();
        let mut cache = ThumbnailCache::default();
        // Touch more distinct sources than the cap; each first sight inserts a
        // slot (a no-ref miss is enough to exercise the eviction).
        for i in 0..(THUMB_CACHE_CAP + 6) {
            let s = source(&format!("peer:n{i}"), "n", &[Protocol::Rdp]);
            let _ = cache.texture_for(&ctx, &s);
        }
        assert_eq!(
            cache.slots.len(),
            THUMB_CACHE_CAP,
            "the live texture set is bounded (Q7)"
        );
        // The earliest-touched ids were evicted; the most-recent survive.
        assert!(
            !cache.slots.contains_key("peer:n0"),
            "the LRU slot was evicted"
        );
        assert!(
            cache
                .slots
                .contains_key(&format!("peer:n{}", THUMB_CACHE_CAP + 5)),
            "the most-recently shown card is retained"
        );
    }

    #[test]
    fn a_thumbnailed_card_renders_the_decoded_preview_end_to_end() {
        let mut thumbnailed = source("peer:oak", "oak", &[Protocol::Rdp]);
        thumbnailed.thumbnail_ref = Some(png_data_uri(&tiny_png(8, 6)));
        let mut state = state_with(Some(roster(vec![thumbnailed])));
        assert!(
            run_panel(&mut state),
            "the thumbnailed card produced no draw primitives"
        );
        // The render ran the full source→texture path into the bounded cache.
        assert_eq!(state.thumbs.slots.len(), 1);
        assert!(
            state.thumbs.slots.values().all(|s| s.texture.is_some()),
            "the card's snapshot decoded to a live texture"
        );
    }

    // ── CHOOSER-7: local-VM power controls ──

    /// A local-KVM source row in the exact `source_from_vm` shape (Spice console,
    /// reachability + reason derived from the power state) — the aggregator's
    /// `local-kvm` lane projection this surface renders.
    fn local_vm(name: &str, node: &str, power: &str) -> DesktopSource {
        let live = matches!(power.trim(), "running" | "paused");
        DesktopSource {
            id: format!("vm:{node}:{name}"),
            name: name.to_string(),
            node: node.to_string(),
            host: node.to_string(),
            protocols: vec![ProtocolOffer {
                protocol: Protocol::Spice,
                port: None,
            }],
            origin: SourceOrigin::LocalVm,
            reachability: if live {
                Reachability::Reachable
            } else {
                Reachability::Unreachable
            },
            reason: (!live).then(|| format!("vm {power}")),
            os_hint: None,
            power_state: Some(power.to_string()),
            thumbnail_ref: None,
        }
    }

    fn lane(name: &str, status: &str) -> LaneStatus {
        LaneStatus {
            lane: name.to_string(),
            status: status.to_string(),
        }
    }

    #[test]
    fn power_state_reflection_offers_state_appropriate_actions() {
        // State reflection: the card offers only the ops valid for the published
        // power state — a stopped VM starts (one click away), a running one
        // stops/pauses, a paused one resumes/stops, an unmapped state nothing.
        assert_eq!(
            PowerState::from_wire("shut off").actions().to_vec(),
            vec![PowerOp::Start]
        );
        assert_eq!(
            PowerState::from_wire("crashed").actions().to_vec(),
            vec![PowerOp::Start],
            "a crashed VM can be re-Started"
        );
        assert_eq!(
            PowerState::from_wire("running").actions().to_vec(),
            vec![PowerOp::Stop, PowerOp::Pause]
        );
        assert_eq!(
            PowerState::from_wire("paused").actions().to_vec(),
            vec![PowerOp::Resume, PowerOp::Stop]
        );
        assert!(
            PowerState::from_wire("pmsuspended").actions().is_empty(),
            "an unmapped state offers no blind action (§7)"
        );
    }

    #[test]
    fn the_lifecycle_topic_matches_the_worker_contract() {
        // Cross-check: MUST equal mackesd::workers::vm_lifecycle::ACTION_TOPIC.
        assert_eq!(LIFECYCLE_TOPIC, "action/vm/lifecycle");
    }

    #[test]
    fn power_ops_map_to_the_host_targeted_vm_lifecycle_verbs() {
        // Action dispatch (wire): each op serialises to the worker's LifecycleAction
        // shape, host-targeted so it can only act on the named node.
        let body = |op: PowerOp| op.to_request("elm", "dev").to_body();
        let start: serde_json::Value = serde_json::from_str(&body(PowerOp::Start)).unwrap();
        assert_eq!(start["op"], "start");
        let stop: serde_json::Value = serde_json::from_str(&body(PowerOp::Stop)).unwrap();
        assert_eq!(stop["op"], "stop");
        assert_eq!(stop["force"], false, "the card issues a graceful stop");
        let pause: serde_json::Value = serde_json::from_str(&body(PowerOp::Pause)).unwrap();
        assert_eq!(pause["op"], "pause");
        let resume: serde_json::Value = serde_json::from_str(&body(PowerOp::Resume)).unwrap();
        assert_eq!(resume["op"], "resume");
        for op in [
            PowerOp::Start,
            PowerOp::Stop,
            PowerOp::Pause,
            PowerOp::Resume,
        ] {
            let v: serde_json::Value = serde_json::from_str(&body(op)).unwrap();
            assert_eq!(v["host"], "elm", "host-targeted");
            assert_eq!(v["name"], "dev");
        }
    }

    #[test]
    fn build_power_request_targets_local_vms_and_skips_peers() {
        // Action dispatch (source→request): a local VM maps to a Start for its own
        // node + name; a peer VM/seat is powered from ITS node, never from here.
        let sources = vec![
            local_vm("dev", "elm", "shut off"),
            source("peer:oak", "oak", &[Protocol::Rdp]),
        ];
        let req = build_power_request(&sources, "vm:elm:dev", PowerOp::Start).expect("local maps");
        let v: serde_json::Value = serde_json::from_str(&req.to_body()).unwrap();
        assert_eq!(v["op"], "start");
        assert_eq!(v["host"], "elm");
        assert_eq!(v["name"], "dev");
        assert!(
            build_power_request(&sources, "peer:oak", PowerOp::Stop).is_none(),
            "a peer source is not driven from here"
        );
        assert!(
            build_power_request(&sources, "vm:elm:ghost", PowerOp::Start).is_none(),
            "a vanished id maps to nothing"
        );
    }

    #[test]
    fn the_no_hypervisor_gate_reads_the_local_kvm_lane_status() {
        // The honest-gate fold: a gated/errored local-kvm lane surfaces its reason
        // (power controls disable); a live "ok" lane does not gate.
        assert_eq!(
            local_hypervisor_gate(&[lane("local-kvm", "gated: virsh not found")]).as_deref(),
            Some("gated: virsh not found")
        );
        assert!(
            local_hypervisor_gate(&[lane("local-kvm", "error: libvirt refused")]).is_some(),
            "a backend error also gates"
        );
        assert!(
            local_hypervisor_gate(&[lane("local-kvm", "ok (2 vms)")]).is_none(),
            "a live hypervisor does not gate"
        );
        assert!(
            local_hypervisor_gate(&[lane("mdns", "ok")]).is_none(),
            "no local-kvm lane → no gate"
        );
        assert!(local_hypervisor_gate(&[]).is_none());
    }

    #[test]
    fn a_local_vm_power_click_routes_through_the_lifecycle_emitter() {
        // Driving a card power op with no Bus root records the honest publish error
        // (never a panic) — proving the click reaches the shared vm_lifecycle
        // emitter rather than faking a local state flip (§7).
        let mut state = state_with(Some(roster(vec![local_vm("dev", "elm", "shut off")])));
        let sources = state.sources_snapshot();
        state.power_action(&sources, "vm:elm:dev", PowerOp::Start);
        assert!(
            state
                .last_error
                .as_deref()
                .is_some_and(|e| e.contains("Bus")),
            "no Bus dir surfaces an honest error, not a panic: {:?}",
            state.last_error
        );

        // A peer/non-local source is a no-op here (no error, no note).
        let mut peer = state_with(Some(roster(vec![source(
            "peer:oak",
            "oak",
            &[Protocol::Rdp],
        )])));
        let peers = peer.sources_snapshot();
        peer.power_action(&peers, "peer:oak", PowerOp::Start);
        assert!(
            peer.last_error.is_none() && peer.note.is_none(),
            "a non-local source is never driven from the Chooser"
        );
    }

    #[test]
    fn a_stopped_local_vm_card_renders_the_power_controls() {
        // The shut-off local VM greys (offline) but its Start button draws at full
        // strength — the "one click away" affordance tessellates, and rendering it
        // is not a connect.
        let mut state = state_with(Some(roster(vec![local_vm("dev", "elm", "shut off")])));
        assert!(
            run_panel(&mut state),
            "the local-VM power row produced no draw primitives"
        );
        assert!(state.take_connect().is_none());
    }

    #[test]
    fn a_gated_local_kvm_lane_renders_disabled_power_controls_with_the_reason() {
        // A LocalVm card while the local-kvm lane reports no hypervisor: the buttons
        // render disabled and the honest reason draws (§7 — never a control that
        // pretends to act).
        let state = DesktopSourcesState {
            sources: vec![local_vm("dev", "elm", "running")],
            lanes: vec![lane("local-kvm", "gated: virsh not found")],
        };
        let mut cs = state_with(Some(state));
        assert!(
            run_panel(&mut cs),
            "the gated power row produced no draw primitives"
        );
    }

    // ── CHOOSER-8: card actions + find + non-blocking offline states ──

    /// A manual (operator-added) source row in the aggregator's `source_from_manual`
    /// shape — origin `Manual`, never probed (an honest `Unknown` reachability).
    fn manual_source(host: &str, port: u16, proto: Protocol) -> DesktopSource {
        DesktopSource {
            id: format!("manual:{host}:{port}:{}", proto.wire_tag().unwrap_or("?")),
            name: format!("{host}:{port}"),
            node: host.to_string(),
            host: host.to_string(),
            protocols: vec![ProtocolOffer {
                protocol: proto,
                port: Some(port),
            }],
            origin: SourceOrigin::Manual,
            reachability: Reachability::Unknown,
            reason: None,
            os_hint: None,
            power_state: None,
            thumbnail_ref: None,
        }
    }

    /// [`state_with`] over an explicit publish Bus root (a real temp spool) so the
    /// CHOOSER-8 verbs can be read back off the topic they land on.
    fn state_with_bus(
        state: Option<DesktopSourcesState>,
        bus_root: Option<PathBuf>,
    ) -> ChooserState {
        let mut s = ChooserState::with_client(
            Box::new(FakeSources(state)),
            bus_root,
            "client-node".to_string(),
            Box::new(MeshCredentialStore),
            inert_prefs(),
        );
        s.refresh();
        s
    }

    /// [`state_with_bus`] with a CHOOSER-9 prefs session over an explicit workgroup
    /// root + seat — so the two-seat sync tests can pin at one seat and read the
    /// roamed record at another over one shared mesh dir.
    fn state_with_prefs(
        state: Option<DesktopSourcesState>,
        bus_root: Option<PathBuf>,
        prefs_root: PathBuf,
        seat: &str,
    ) -> ChooserState {
        let mut s = ChooserState::with_client(
            Box::new(FakeSources(state)),
            bus_root,
            "client-node".to_string(),
            Box::new(MeshCredentialStore),
            prefs_at(prefs_root, seat),
        );
        s.refresh();
        s
    }

    /// A unique temp Bus dir (the crate's `std::env::temp_dir()` idiom — no
    /// `tempfile` dep), cleaned up by each test that uses it.
    fn temp_bus_dir(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("mde-chooser8-{tag}-{n}"))
    }

    #[test]
    fn the_desktop_action_topics_match_the_worker_contract() {
        // Cross-check: MUST equal the mackesd desktop_sources worker's verb topics.
        assert_eq!(ADD_SOURCE_TOPIC, "action/desktops/add-source");
        assert_eq!(REMOVE_SOURCE_TOPIC, "action/desktops/remove-source");
        assert_eq!(REFRESH_TOPIC, "action/desktops/refresh");
    }

    #[test]
    fn add_and_remove_source_bodies_match_the_worker_shape() {
        // The add-source body carries host + port + protocol (§9 — never a command
        // string); an absent name is skipped so the worker defaults it to host:port.
        let add = AddSourceRequest {
            name: Some("OfficePC".to_string()),
            host: "10.0.0.5".to_string(),
            port: 3389,
            protocol: "rdp",
        };
        let v: serde_json::Value = serde_json::from_str(&add.to_body()).unwrap();
        assert_eq!(v["name"], "OfficePC");
        assert_eq!(v["host"], "10.0.0.5");
        assert_eq!(v["port"], 3389);
        assert_eq!(v["protocol"], "rdp");
        let bare = AddSourceRequest {
            name: None,
            host: "h".to_string(),
            port: 1,
            protocol: "vnc",
        };
        let v2: serde_json::Value = serde_json::from_str(&bare.to_body()).unwrap();
        assert!(
            v2.get("name").is_none(),
            "a None name is skipped on the wire"
        );
        // The remove-source body is just the manual id.
        let rm = RemoveSourceRequest {
            id: "manual:h:1:vnc".to_string(),
        };
        let vr: serde_json::Value = serde_json::from_str(&rm.to_body()).unwrap();
        assert_eq!(vr["id"], "manual:h:1:vnc");
    }

    #[test]
    fn the_search_matches_name_node_host_and_os() {
        let state = fixture_state();
        let mut f = FilterSort::default();
        let hits = |f: &FilterSort| -> Vec<String> {
            state
                .sources
                .iter()
                .filter(|s| f.matches(s))
                .map(|s| s.id.clone())
                .collect()
        };
        // Name substring (case-insensitive).
        f.search = "office".to_string();
        assert_eq!(hits(&f), vec!["mdns:192.168.1.60:3389:rdp"]);
        // OS hint — only the peer seat carries "linux".
        f.search = "LINUX".to_string();
        assert_eq!(hits(&f), vec!["peer:oak"]);
        // Node/host substring.
        f.search = "192.168".to_string();
        assert_eq!(hits(&f), vec!["mdns:192.168.1.60:3389:rdp"]);
        // A blank/whitespace query matches the whole roster.
        f.search = "   ".to_string();
        assert_eq!(hits(&f).len(), 3);
    }

    #[test]
    fn filters_narrow_by_node_protocol_status_and_os() {
        let state = fixture_state();
        let count = |f: &FilterSort| state.sources.iter().filter(|s| f.matches(s)).count();

        assert_eq!(
            count(&FilterSort {
                node: Some("oak".to_string()),
                ..Default::default()
            }),
            2,
            "oak groups the seat + its VM"
        );
        assert_eq!(
            count(&FilterSort {
                protocol: Some(Protocol::Spice),
                ..Default::default()
            }),
            1,
            "only the VM offers Spice"
        );
        assert_eq!(
            count(&FilterSort {
                status: Some(Reachability::Reachable),
                ..Default::default()
            }),
            2
        );
        assert_eq!(
            count(&FilterSort {
                status: Some(Reachability::Unreachable),
                ..Default::default()
            }),
            1,
            "the offline VM"
        );
        assert_eq!(
            count(&FilterSort {
                os: Some("linux".to_string()),
                ..Default::default()
            }),
            1
        );
    }

    #[test]
    fn is_active_and_clear_reset_the_narrowing_but_keep_the_sort() {
        let mut f = FilterSort::default();
        assert!(!f.is_active(), "a default filter narrows nothing");
        f.search = " win ".to_string();
        assert!(f.is_active());
        f.search.clear();
        f.protocol = Some(Protocol::Rdp);
        f.node = Some("oak".to_string());
        assert!(f.is_active());
        f.sort = SortKey::Name;
        f.clear();
        assert!(!f.is_active(), "clear drops every filter + the search");
        assert_eq!(f.sort, SortKey::Name, "clear keeps the sort preference");
    }

    #[test]
    fn distinct_nodes_and_os_feed_the_filter_combos() {
        let state = fixture_state();
        // First-seen (published) order, deduped case-insensitively.
        assert_eq!(distinct_nodes(&state.sources), vec!["oak", "192.168.1.60"]);
        assert_eq!(distinct_os(&state.sources), vec!["linux"]);
    }

    #[test]
    fn order_members_floats_favorites_then_applies_the_sort_key() {
        let zeta = source("peer:zeta", "n", &[Protocol::Rdp]);
        let alpha = source("peer:alpha", "n", &[Protocol::Rdp]);
        let mid = source("peer:mid", "n", &[Protocol::Rdp]);
        let ids = |m: &[&DesktopSource]| m.iter().map(|s| s.id.clone()).collect::<Vec<_>>();

        // `Discovered` is a stable no-op — the published order is preserved.
        let mut m = vec![&zeta, &alpha, &mid];
        order_members(&mut m, SortKey::Discovered, &HashSet::new());
        assert_eq!(ids(&m), vec!["peer:zeta", "peer:alpha", "peer:mid"]);

        // `Name` sorts A→Z within the group.
        let mut m = vec![&zeta, &alpha, &mid];
        order_members(&mut m, SortKey::Name, &HashSet::new());
        assert_eq!(ids(&m), vec!["peer:alpha", "peer:mid", "peer:zeta"]);

        // A favorite floats first, ahead of the sort key.
        let favs: HashSet<String> = std::iter::once("peer:zeta".to_string()).collect();
        let mut m = vec![&zeta, &alpha, &mid];
        order_members(&mut m, SortKey::Name, &favs);
        assert_eq!(ids(&m), vec!["peer:zeta", "peer:alpha", "peer:mid"]);
    }

    #[test]
    fn the_status_sort_floats_reachable_before_offline() {
        let mut up = source("peer:up", "n", &[Protocol::Rdp]);
        up.reachability = Reachability::Reachable;
        let mut down = source("peer:down", "n", &[Protocol::Rdp]);
        down.reachability = Reachability::Unreachable;
        let mut unk = source("peer:unk", "n", &[Protocol::Rdp]);
        unk.reachability = Reachability::Unknown;
        let mut m = vec![&down, &unk, &up];
        order_members(&mut m, SortKey::Status, &HashSet::new());
        assert_eq!(
            m.iter().map(|s| s.reachability).collect::<Vec<_>>(),
            vec![
                Reachability::Reachable,
                Reachability::Unknown,
                Reachability::Unreachable
            ]
        );
    }

    #[test]
    fn toggle_favorite_pins_then_unpins() {
        let mut state = state_with(Some(roster(vec![source(
            "peer:oak",
            "oak",
            &[Protocol::Rdp],
        )])));
        assert!(!state.favorites.contains("peer:oak"));
        state.toggle_favorite("peer:oak");
        assert!(state.favorites.contains("peer:oak"), "a pin adds it");
        state.toggle_favorite("peer:oak");
        assert!(
            !state.favorites.contains("peer:oak"),
            "a second toggle unpins"
        );
    }

    #[test]
    fn an_offline_card_offers_retry_not_a_blind_connect() {
        // The non-blocking offline model: a click on the greyed card never connects
        // nor opens the picker (lock 14); Retry drives a discovery re-enumerate.
        let mut off = source("peer:ash", "ash", &[Protocol::Rdp]);
        off.reachability = Reachability::Unreachable;
        off.reason = Some("peer unreachable".to_string());
        let mut state = state_with(Some(roster(vec![off])));
        let sources = state.sources_snapshot();
        state.activate(&sources, "peer:ash");
        assert!(
            state.pending.is_none() && state.take_connect().is_none(),
            "a greyed card never connects"
        );
        // Retry reaches the refresh emitter (honest no-Bus error), returning at once
        // — never a probe, never a block.
        state.retry_discovery(&sources, "peer:ash");
        assert!(state
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("Bus")));
    }

    #[test]
    fn retry_discovery_writes_the_bodyless_refresh_verb() {
        let dir = temp_bus_dir("retry");
        let mut off = source("peer:ash", "ash", &[Protocol::Rdp]);
        off.reachability = Reachability::Unreachable;
        let mut state = state_with_bus(Some(roster(vec![off])), Some(dir.clone()));
        let sources = state.sources_snapshot();
        state.retry_discovery(&sources, "peer:ash");
        assert!(
            state.last_error.is_none(),
            "the refresh publish succeeded: {:?}",
            state.last_error
        );
        let persist = mde_bus::persist::Persist::open(dir.clone()).expect("open bus");
        let msgs = persist.list_since(REFRESH_TOPIC, None).expect("list");
        assert_eq!(msgs.len(), 1, "one Retry ⇒ one refresh nudge");
        assert!(state
            .note
            .as_deref()
            .is_some_and(|n| n.contains("Re-checking")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_source_targets_only_manual_origins() {
        let dir = temp_bus_dir("remove");
        let manual = manual_source("10.0.0.5", 3389, Protocol::Rdp);
        let manual_id = manual.id.clone();
        let mut state = state_with_bus(
            Some(roster(vec![
                manual,
                source("peer:oak", "oak", &[Protocol::Rdp]),
            ])),
            Some(dir.clone()),
        );
        let sources = state.sources_snapshot();

        // A discovered peer is never removed from here (no verb published).
        state.remove_source(&sources, "peer:oak");
        assert!(state.last_error.is_none() && state.note.is_none());

        // A manual source publishes the remove-source verb keyed on its id.
        state.remove_source(&sources, &manual_id);
        let persist = mde_bus::persist::Persist::open(dir.clone()).expect("open bus");
        let msgs = persist.list_since(REMOVE_SOURCE_TOPIC, None).expect("list");
        assert_eq!(
            msgs.len(),
            1,
            "only the manual source published a remove; the peer was a no-op"
        );
        let v: serde_json::Value = serde_json::from_str(msgs[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(v["id"], manual_id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn begin_edit_seeds_from_the_manual_source_and_ignores_non_manual() {
        let mut named = manual_source("10.0.0.5", 3389, Protocol::Rdp);
        named.name = "OfficePC".to_string();
        let manual_id = named.id.clone();
        let mut state = state_with(Some(roster(vec![
            named,
            source("peer:oak", "oak", &[Protocol::Rdp]),
        ])));
        let sources = state.sources_snapshot();

        // A discovered source never opens the edit form.
        state.begin_edit(&sources, "peer:oak");
        assert!(
            state.manual_edit.is_none(),
            "only a manual source is editable"
        );

        // The manual source seeds the form from its current fields.
        state.begin_edit(&sources, &manual_id);
        let edit = state.manual_edit.as_ref().expect("the edit form opened");
        assert_eq!(edit.original_id, manual_id);
        assert_eq!(edit.name, "OfficePC");
        assert_eq!(edit.host, "10.0.0.5");
        assert_eq!(edit.port, "3389");
        assert_eq!(edit.protocol, Protocol::Rdp);
    }

    #[test]
    fn begin_edit_blanks_a_default_host_port_name() {
        // A manual source whose name is the `host:port` default seeds an EMPTY name
        // field, so an unchanged Save lets the worker re-default it.
        let m = manual_source("h", 5900, Protocol::Vnc);
        let id = m.id.clone();
        let mut state = state_with(Some(roster(vec![m])));
        let sources = state.sources_snapshot();
        state.begin_edit(&sources, &id);
        let edit = state.manual_edit.as_ref().expect("form open");
        assert!(
            edit.name.is_empty(),
            "a default host:port name seeds an empty field"
        );
        assert_eq!(edit.protocol, Protocol::Vnc);
    }

    #[test]
    fn save_manual_edit_validates_host_and_port_without_publishing() {
        let m = manual_source("10.0.0.5", 3389, Protocol::Rdp);
        let id = m.id.clone();
        let mut state = state_with(Some(roster(vec![m])));
        let sources = state.sources_snapshot();
        state.begin_edit(&sources, &id);

        // Empty host → an inline error; the publish is never reached (no Bus error).
        state.manual_edit.as_mut().unwrap().host = "   ".to_string();
        state.save_manual_edit(&sources);
        assert!(state
            .manual_edit
            .as_ref()
            .and_then(|e| e.error.as_deref())
            .is_some_and(|e| e.contains("Host")));
        assert!(
            state.last_error.is_none(),
            "a validation stop never reaches the publish"
        );

        // Non-numeric port → an inline error.
        {
            let e = state.manual_edit.as_mut().unwrap();
            e.host = "10.0.0.9".to_string();
            e.port = "not-a-port".to_string();
            e.error = None;
        }
        state.save_manual_edit(&sources);
        assert!(state
            .manual_edit
            .as_ref()
            .and_then(|e| e.error.as_deref())
            .is_some_and(|e| e.contains("Port")));
        assert!(state.last_error.is_none());
    }

    #[test]
    fn save_manual_edit_republishes_via_remove_then_add() {
        let dir = temp_bus_dir("edit");
        let m = manual_source("10.0.0.5", 3389, Protocol::Rdp);
        let original_id = m.id.clone();
        let mut state = state_with_bus(Some(roster(vec![m])), Some(dir.clone()));
        let sources = state.sources_snapshot();
        state.begin_edit(&sources, &original_id);
        {
            let e = state.manual_edit.as_mut().unwrap();
            e.name = "Reception".to_string();
            e.host = "10.0.0.9".to_string();
            e.port = "5900".to_string();
            e.protocol = Protocol::Vnc;
        }
        state.save_manual_edit(&sources);
        assert!(
            state.last_error.is_none(),
            "both verbs published: {:?}",
            state.last_error
        );
        assert!(
            state.manual_edit.is_none(),
            "the form closes on a successful save"
        );

        let persist = mde_bus::persist::Persist::open(dir.clone()).expect("open bus");
        // The old id is removed …
        let rm = persist.list_since(REMOVE_SOURCE_TOPIC, None).expect("list");
        assert_eq!(rm.len(), 1);
        let rv: serde_json::Value = serde_json::from_str(rm[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(rv["id"], original_id);
        // … and the edited endpoint added over the worker's typed add-source verb.
        let add = persist.list_since(ADD_SOURCE_TOPIC, None).expect("list");
        assert_eq!(add.len(), 1);
        let av: serde_json::Value = serde_json::from_str(add[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(av["name"], "Reception");
        assert_eq!(av["host"], "10.0.0.9");
        assert_eq!(av["port"], 5900);
        assert_eq!(av["protocol"], "vnc");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── CHOOSER-8 headless renders ──

    #[test]
    fn the_filter_bar_and_grid_render_together() {
        let mut state = state_with(Some(fixture_state()));
        assert!(
            run_panel(&mut state),
            "the find bar + card grid produced no draw primitives"
        );
    }

    #[test]
    fn a_fully_filtered_out_roster_renders_the_no_match_note() {
        let mut state = state_with(Some(fixture_state()));
        state.filter.search = "no-such-desktop".to_string();
        assert!(
            run_panel(&mut state),
            "the no-match note produced no draw primitives"
        );
        assert!(state.take_connect().is_none(), "rendering never connects");
    }

    #[test]
    fn the_manual_edit_form_renders() {
        let m = manual_source("10.0.0.5", 3389, Protocol::Rdp);
        let id = m.id.clone();
        let mut state = state_with(Some(roster(vec![m])));
        let sources = state.sources_snapshot();
        state.begin_edit(&sources, &id);
        assert!(
            run_panel(&mut state),
            "the manual-source edit form produced no draw primitives"
        );
    }

    #[test]
    fn a_favorited_card_renders_the_pin_marker() {
        let mut state = state_with(Some(roster(vec![source(
            "peer:oak",
            "oak",
            &[Protocol::Rdp],
        )])));
        state.toggle_favorite("peer:oak");
        assert!(
            run_panel(&mut state),
            "the pinned card produced no draw primitives"
        );
    }

    // ── CHOOSER-9: mesh-synced favorites / recents / manual sources ──

    /// A unique temp workgroup root (the crate's `std::env::temp_dir()` idiom — no
    /// `tempfile` dep), created + cleaned up by each sync test that uses it.
    fn temp_prefs_root(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!("mde-chooser9-shell-{tag}-{n}"));
        std::fs::create_dir_all(&root).expect("mkroot");
        root
    }

    #[test]
    fn a_pin_at_seat_a_syncs_and_appears_at_seat_b() {
        // THE ACCEPTANCE (two-seat): pin at seat A → the per-identity record syncs
        // over the shared workgroup root → seat B shows the pin. This drives the
        // sync mechanism directly (the live cross-seat is the gated leg).
        let root = temp_prefs_root("pin");
        let oak = source("peer:oak", "oak", &[Protocol::Rdp]);

        // ── Seat A pins the desktop. ──
        let mut seat_a = state_with_prefs(
            Some(roster(vec![oak.clone()])),
            None,
            root.clone(),
            "seat-a",
        );
        assert!(!seat_a.favorites.contains("peer:oak"), "not pinned yet");
        seat_a.toggle_favorite("peer:oak");
        assert!(seat_a.favorites.contains("peer:oak"), "pinned at seat A");

        // ── Seat B opens fresh over the SAME workgroup root. ──
        let seat_b = state_with_prefs(Some(roster(vec![oak])), None, root.clone(), "seat-b");
        assert!(
            seat_b.favorites.contains("peer:oak"),
            "seat A's pin roamed to seat B"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_recent_and_an_unpin_roam_between_seats() {
        let root = temp_prefs_root("recent");
        let oak = source("peer:oak", "oak", &[Protocol::Rdp]);

        // Seat A pins + connects (a genuine "recently used").
        let mut seat_a = state_with_prefs(
            Some(roster(vec![oak.clone()])),
            None,
            root.clone(),
            "seat-a",
        );
        let sources_a = seat_a.sources_snapshot();
        seat_a.toggle_favorite("peer:oak");
        seat_a.activate(&sources_a, "peer:oak");
        seat_a.confirm_connect(&sources_a); // records the recent
        assert!(seat_a.recents.contains("peer:oak"), "recorded at seat A");

        // Seat B sees both the pin and the recent.
        let mut seat_b = state_with_prefs(
            Some(roster(vec![oak.clone()])),
            None,
            root.clone(),
            "seat-b",
        );
        assert!(seat_b.favorites.contains("peer:oak"), "pin roamed");
        assert!(seat_b.recents.contains("peer:oak"), "recent roamed");

        // Seat B un-pins; seat A re-reads and the un-pin has converged (LWW, so the
        // newer un-pin beats the older pin — never a grow-only set).
        seat_b.toggle_favorite("peer:oak");
        assert!(
            !seat_b.favorites.contains("peer:oak"),
            "un-pinned at seat B"
        );
        let seat_a2 = state_with_prefs(Some(roster(vec![oak])), None, root.clone(), "seat-a");
        assert!(
            !seat_a2.favorites.contains("peer:oak"),
            "seat B's newer un-pin roamed back to seat A"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn compact_reconnect_uses_the_newest_recent_source() {
        let root = temp_prefs_root("compact-reconnect");
        let older = source("peer:ash", "ash", &[Protocol::Vnc]);
        let newer = source("peer:oak", "oak", &[Protocol::Rdp]);
        let mut state = state_with_prefs(
            Some(roster(vec![older.clone(), newer.clone()])),
            None,
            root.clone(),
            "seat-a",
        );

        state.prefs.record_recent("peer:ash", "ash", 10);
        state.prefs.record_recent("peer:oak", "oak", 20);
        state.refresh_prefs_cache();

        let request = state
            .connect_last_recent()
            .expect("newest recent reconnects");
        assert_eq!(request.target.name, "oak");
        assert_eq!(request.protocol, VdiProtocol::Rdp);
        assert!(
            state.take_connect().is_none(),
            "the compact reconnect returns and drains the request"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn compact_source_rows_reuse_chooser_state_and_selected_source_connects() {
        let root = temp_prefs_root("compact-source-rows");
        let plain = source("peer:ash", "ash", &[Protocol::Vnc]);
        let pinned = source("peer:oak", "oak", &[Protocol::Rdp]);
        let mut state = state_with_prefs(
            Some(roster(vec![plain.clone(), pinned.clone()])),
            None,
            root.clone(),
            "seat-a",
        );
        state.toggle_favorite("peer:oak");
        state.refresh_prefs_cache();

        let rows = state.rail_sources();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "peer:oak", "pinned source floats first");
        assert_eq!(rows[0].label, "oak");
        assert!(rows[0].connectable);

        let request = state
            .connect_source_id("peer:ash")
            .expect("selected compact row connects through chooser");
        assert_eq!(request.target.name, "ash");
        assert_eq!(request.protocol, VdiProtocol::Vnc);
        assert!(
            state.take_connect().is_none(),
            "compact source connect returns and drains the request"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn compact_selection_and_expanded_panel_share_the_same_pending_picker() {
        // NAVBAR-U4 — the compact rail face and the expanded chooser face are one
        // `ChooserState`: selecting an external source from compact mode raises the
        // credential picker in the same state the expanded panel renders.
        let (mut state, id) = external_state(Box::new(RecordingStore::default()));
        assert!(
            state.connect_source_id(&id).is_none(),
            "no sealed external credential means compact selection must not connect blind"
        );
        assert!(
            state
                .pending
                .as_ref()
                .is_some_and(|d| d.source_id == id && d.cred_prompt.is_some()),
            "compact selection raised the expanded chooser's pending credential prompt"
        );
        assert!(
            run_panel(&mut state),
            "the expanded chooser renders the pending prompt from the compact pick"
        );
        assert!(
            state
                .pending
                .as_ref()
                .is_some_and(|d| d.source_id == id && d.cred_prompt.is_some()),
            "rendering the expanded face keeps the same pending picker state"
        );
    }

    #[test]
    fn a_manual_source_roams_and_rematerializes_at_a_new_seat() {
        // A manual desktop added on seat A is captured into the synced prefs; seat B
        // (whose worker doesn't know it yet) re-publishes it over the ONE existing
        // add-source verb, so it appears there too — reusing the CHOOSER-8 seam.
        let prefs_root = temp_prefs_root("manual");
        let bus_a = temp_bus_dir("manual-a");
        let bus_b = temp_bus_dir("manual-b");
        let manual = manual_source("10.0.0.5", 3389, Protocol::Rdp);
        let manual_id = manual.id.clone();

        // ── Seat A folds a roster carrying the manual source → captured into prefs.
        let seat_a = state_with_prefs(
            Some(roster(vec![manual])),
            Some(bus_a.clone()),
            prefs_root.clone(),
            "seat-a",
        );
        assert!(
            seat_a
                .prefs
                .merged()
                .manual
                .iter()
                .any(|m| m.id == manual_id),
            "the manual source was captured into the synced prefs"
        );

        // ── Seat B has an EMPTY roster (its worker hasn't heard of the endpoint) but
        // shares the workgroup root → it re-materializes the roamed manual source
        // onto its own worker via the add-source verb.
        let seat_b = state_with_prefs(
            Some(roster(vec![])),
            Some(bus_b.clone()),
            prefs_root.clone(),
            "seat-b",
        );
        assert!(
            seat_b.last_error.is_none(),
            "the re-materialize publish succeeded: {:?}",
            seat_b.last_error
        );
        let persist = mde_bus::persist::Persist::open(bus_b.clone()).expect("open bus");
        let adds = persist.list_since(ADD_SOURCE_TOPIC, None).expect("list");
        assert_eq!(
            adds.len(),
            1,
            "the roamed manual source is re-published once"
        );
        let v: serde_json::Value = serde_json::from_str(adds[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(v["host"], "10.0.0.5");
        assert_eq!(v["port"], 3389);
        assert_eq!(v["protocol"], "rdp");

        let _ = std::fs::remove_dir_all(&prefs_root);
        let _ = std::fs::remove_dir_all(&bus_a);
        let _ = std::fs::remove_dir_all(&bus_b);
    }

    #[test]
    fn removing_a_manual_source_tombstones_it_so_it_does_not_reappear() {
        // Remove on seat A tombstones the synced register; a fresh seat B must NOT
        // re-materialize it (a grow-only set would resurrect a removed desktop).
        let prefs_root = temp_prefs_root("manual-rm");
        let bus_a = temp_bus_dir("manual-rm-a");
        let bus_b = temp_bus_dir("manual-rm-b");
        let manual = manual_source("10.0.0.5", 3389, Protocol::Rdp);
        let manual_id = manual.id.clone();

        let mut seat_a = state_with_prefs(
            Some(roster(vec![manual])),
            Some(bus_a.clone()),
            prefs_root.clone(),
            "seat-a",
        );
        let sources_a = seat_a.sources_snapshot();
        seat_a.remove_source(&sources_a, &manual_id);
        assert!(
            !seat_a
                .prefs
                .merged()
                .manual
                .iter()
                .any(|m| m.id == manual_id),
            "the removed manual source is tombstoned in the synced prefs"
        );

        // Seat B, empty roster, shared root: the tombstone means nothing to
        // re-materialize — no add-source verb published.
        let seat_b = state_with_prefs(
            Some(roster(vec![])),
            Some(bus_b.clone()),
            prefs_root.clone(),
            "seat-b",
        );
        let persist = mde_bus::persist::Persist::open(bus_b.clone()).expect("open bus");
        let adds = persist.list_since(ADD_SOURCE_TOPIC, None).expect("list");
        assert!(
            adds.is_empty(),
            "a removed manual source does not roam back to a new seat"
        );
        assert!(seat_b.favorites.is_empty());

        let _ = std::fs::remove_dir_all(&prefs_root);
        let _ = std::fs::remove_dir_all(&bus_a);
        let _ = std::fs::remove_dir_all(&bus_b);
    }

    // ── TESTVM-4: pinned endpoints — selectable with NO mesh discovery ──

    #[test]
    fn a_pinned_endpoint_is_added_rendered_and_counted_with_no_roster_and_no_bus() {
        let root = temp_prefs_root("pin-endpoint");
        // No roster ever published, no Bus root — the mesh-less seat.
        let mut state = state_with_prefs(None, None, root.clone(), "seat-a");
        assert!(state.sources_snapshot().is_empty(), "nothing pinned yet");

        // The operator pins the live VNC test endpoint through the ADD form.
        state.begin_add();
        {
            let e = state.manual_edit.as_mut().expect("the add form opened");
            assert!(e.original_id.is_empty(), "ADD mode has no original id");
            e.name = "testvm-lin".to_string();
            e.host = "172.20.146.144".to_string();
            e.port = "5900".to_string();
            e.protocol = Protocol::Vnc;
            e.password = "testvm".to_string();
        }
        state.save_manual_edit(&[]);
        assert!(state.manual_edit.is_none(), "the form closes on save");
        assert!(
            state.last_error.is_none(),
            "a mesh-less pin is not an error: {:?}",
            state.last_error
        );

        // The pin renders as a card from the prefs register alone (§7 honest
        // fields: manual origin, never-probed Unknown, still connectable).
        let sources = state.sources_snapshot();
        assert_eq!(
            sources.len(),
            1,
            "the pinned endpoint renders with no roster"
        );
        let card = &sources[0];
        assert_eq!(card.id, "manual:172.20.146.144:5900:vnc");
        assert_eq!(card.name, "testvm-lin");
        assert_eq!(
            (card.node.as_str(), card.host.as_str()),
            ("172.20.146.144", "172.20.146.144")
        );
        assert_eq!(card.origin, SourceOrigin::Manual);
        assert_eq!(card.reachability, Reachability::Unknown);
        assert_eq!(
            card.protocols,
            vec![ProtocolOffer {
                protocol: Protocol::Vnc,
                port: Some(5900)
            }]
        );
        assert!(card.connectable());
        assert_eq!(
            state.source_count(),
            1,
            "the menubar count matches the cards"
        );
        assert!(
            run_panel(&mut state),
            "the pinned card produced no draw primitives"
        );

        // …and the pin roams: a fresh seat over the same root shows it too.
        let seat_b = state_with_prefs(None, None, root.clone(), "seat-b");
        assert_eq!(
            seat_b.sources_snapshot().len(),
            1,
            "the pinned endpoint roamed to a second mesh-less seat"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pinned_endpoint_with_a_stored_credential_connects_without_a_prompt() {
        // THE TESTVM-4 connect acceptance: the stored register credential drives
        // Connect straight through — no prompt, and no credential is sealed.
        let root = temp_prefs_root("pin-connect");
        let store = RecordingStore::default();
        let mut state = ChooserState::with_client(
            Box::new(FakeSources(None)),
            None,
            "client-node".to_string(),
            Box::new(store.clone()),
            prefs_at(root.clone(), "seat-a"),
        );
        state.prefs.set_manual(ManualEntry {
            id: "manual:172.20.146.54:3389:rdp".to_string(),
            present: true,
            host: "172.20.146.54".to_string(),
            port: 3389,
            protocol: "rdp".to_string(),
            name: Some("testvm-win".to_string()),
            username: Some("root".to_string()),
            password: Some("testvm".to_string()),
            updated_ms: 1,
        });
        state.refresh();

        let sources = state.sources_snapshot();
        state.activate(&sources, "manual:172.20.146.54:3389:rdp");
        assert!(
            state.pending.is_some(),
            "the always-ask picker still opens (lock 6)"
        );
        state.confirm_connect(&sources);

        let request = state
            .take_connect()
            .expect("the stored credential connects straight through");
        assert_eq!(
            store.seal_count(),
            0,
            "the stored register cred is not re-sealed"
        );
        assert_eq!(request.protocol, VdiProtocol::Rdp);
        assert_eq!(request.target.name, "testvm-win");
        assert_eq!(
            request
                .target
                .endpoint
                .as_ref()
                .map(|e| (e.host.as_str(), e.port)),
            Some(("172.20.146.54", 3389))
        );
        let DesktopAuth::Sealed {
            credential,
            store_ref,
        } = &request.auth
        else {
            unreachable!("expected the stored register credential")
        };
        assert_eq!(store_ref, "desktop/172.20.146.54/rdp");
        assert_eq!(credential.username, "root");
        assert_eq!(credential.secret.expose(), "testvm");
        assert!(state.pending.is_none(), "the picker closed on connect");
        assert!(
            state.recents.contains("manual:172.20.146.54:3389:rdp"),
            "a genuine connect records the recent"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pinned_endpoint_without_a_stored_password_still_prompts_once() {
        // No stored password → the CHOOSER-6 fold is untouched: the one-time
        // credential prompt raises and nothing connects until it's filled.
        let root = temp_prefs_root("pin-prompt");
        let mut state = state_with_prefs(None, None, root.clone(), "seat-a");
        state.prefs.set_manual(ManualEntry {
            id: "manual:172.20.146.144:5900:vnc".to_string(),
            present: true,
            host: "172.20.146.144".to_string(),
            port: 5900,
            protocol: "vnc".to_string(),
            name: None,
            username: None,
            password: None,
            updated_ms: 1,
        });
        state.refresh();
        let sources = state.sources_snapshot();
        state.activate(&sources, "manual:172.20.146.144:5900:vnc");
        state.confirm_connect(&sources);
        assert!(
            state.take_connect().is_none(),
            "nothing connects before the prompt is filled"
        );
        assert!(
            state
                .pending
                .as_ref()
                .is_some_and(|d| d.cred_prompt.is_some()),
            "the one-time credential prompt raised"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_add_endpoint_form_renders_over_an_empty_grid() {
        // The ADD form must paint even with no roster at all (the empty branch),
        // or a mesh-less seat could never enter its first pin.
        let mut state = state_with(None);
        state.begin_add();
        assert!(
            run_panel(&mut state),
            "the add-endpoint form produced no draw primitives over the empty grid"
        );
    }

    #[test]
    fn an_edit_round_trips_the_stored_credential_and_a_remove_tombstones_the_pin() {
        let root = temp_prefs_root("pin-edit");
        let mut state = state_with_prefs(None, None, root.clone(), "seat-a");
        state.prefs.set_manual(ManualEntry {
            id: "manual:172.20.146.144:5900:vnc".to_string(),
            present: true,
            host: "172.20.146.144".to_string(),
            port: 5900,
            protocol: "vnc".to_string(),
            name: Some("testvm-lin".to_string()),
            username: None,
            password: Some("testvm".to_string()),
            updated_ms: 1,
        });
        state.refresh();
        let sources = state.sources_snapshot();

        // Edit seeds the stored credential; an untouched Save keeps it.
        state.begin_edit(&sources, "manual:172.20.146.144:5900:vnc");
        assert_eq!(
            state.manual_edit.as_ref().map(|e| e.password.as_str()),
            Some("testvm"),
            "the edit form seeds the stored password"
        );
        state.save_manual_edit(&sources);
        assert!(state.manual_edit.is_none());
        assert_eq!(
            state
                .manual_cache
                .iter()
                .find(|m| m.id == "manual:172.20.146.144:5900:vnc")
                .and_then(|m| m.password.as_deref()),
            Some("testvm"),
            "an untouched Save keeps the stored credential"
        );

        // Remove tombstones the register — the card is gone with no Bus error.
        let sources = state.sources_snapshot();
        state.remove_source(&sources, "manual:172.20.146.144:5900:vnc");
        assert!(
            state.last_error.is_none(),
            "a mesh-less remove is not an error: {:?}",
            state.last_error
        );
        assert!(
            state.sources_snapshot().is_empty(),
            "the removed pin no longer renders"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_recently_used_card_renders_its_marker() {
        let root = temp_prefs_root("recent-render");
        let mut state = state_with_prefs(
            Some(roster(vec![source("peer:oak", "oak", &[Protocol::Rdp])])),
            None,
            root.clone(),
            "seat-a",
        );
        let sources = state.sources_snapshot();
        state.activate(&sources, "peer:oak");
        state.confirm_connect(&sources);
        assert!(state.recents.contains("peer:oak"));
        assert!(
            run_panel(&mut state),
            "the recently-used card produced no draw primitives"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
