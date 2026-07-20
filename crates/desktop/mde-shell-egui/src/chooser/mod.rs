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
use crate::bus_reader::BusReader;
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

    /// The status-pip tone, sourced from the shared support-state tokens: a
    /// reachable desktop reads as SUCCESS, an offline one as ERROR, and an
    /// unverified endpoint stays neutral/dim (it is honestly not a state, §7).
    const fn pip(self) -> egui::Color32 {
        match self {
            Self::Reachable => Style::SUPPORT_SUCCESS,
            Self::Unreachable => Style::SUPPORT_ERROR,
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
    ///
    /// `None` for a mesh-brokered VM whose console has no advertised port (a local
    /// or peer VM binds SPICE to loopback — [`ProtocolOffer::port`] is absent).
    /// That endpoint is NOT guessed here: the serving peer's console broker
    /// (VDI-VM-1) relays the loopback console onto the overlay and publishes the
    /// overlay `host:port` back on the session record, and the Desktop surface
    /// resolves it from there ([`crate::vdi::resolve_brokered_console`]).
    fn endpoint_for(&self, protocol: VdiProtocol) -> Option<DesktopEndpoint> {
        let port = self
            .protocols
            .iter()
            .find(|offer| offer.protocol.route() == Some(protocol))
            .and_then(|offer| offer.port)
            .or_else(|| port_from_host(&self.host))?;
        DesktopEndpoint::new(host_without_matching_port(&self.host, port), port)
    }

    /// Whether a connect to this source must resolve its dialable endpoint from the
    /// serving peer's broker record rather than a discovery-time port: a
    /// mesh-brokered source (peer seat / peer VM / local VM) whose chosen protocol
    /// carries no advertised port. Drives the honest connect note so the operator is
    /// told the console endpoint is being brokered, not that frames are already live
    /// (§7) — and the honesty gate: a VM that can't be brokered surfaces the truth at
    /// the transport rather than presenting a connectable card that can't attach.
    fn needs_broker_resolution(&self, protocol: VdiProtocol) -> bool {
        self.origin.is_mesh_brokered() && self.endpoint_for(protocol).is_none()
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
    // arch-11: writer — the shared BusReader seam is read-only; this publish keeps
    // Persist::open because it needs the write Result to set `last_error`.
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
    // arch-11: writer — the shared BusReader seam is read-only; this publish keeps
    // Persist::open because it needs the write Result to set `last_error`.
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
        // arch-11: open through the shared BusReader seam.
        let persist = BusReader::new(self.bus_root.clone()).open()?;
        // The worker writes one record per change (+ heartbeat) and ALWAYS
        // carries a body, so the newest row is the live roster. perf-4 — read
        // just that newest row (bounded `read_latest`) instead of loading the
        // whole retained history and taking the last body; behaviour-identical
        // to the old `list_since(None).filter_map(body).next_back()`.
        persist
            .read_latest(SOURCES_TOPIC)
            .ok()?
            .and_then(|m| m.body)
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
            // VDI-VM-1 — a port-less brokered VM console has its overlay endpoint
            // resolved from the serving peer's broker record (not a discovery port),
            // so say so honestly rather than implying frames are already live (§7).
            let broker_detail = if source.needs_broker_resolution(protocol) {
                " (resolving the console endpoint from the serving peer's broker record)"
            } else {
                ""
            };
            self.note = Some(format!(
                "Requested {} from {} via {} ({} \u{00B7} {}) — brokering over the mesh\
                 {broker_detail}; authenticating with {auth_summary}.",
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
    let status = status.as_ref().map(|(t, d)| (t.as_str(), d.as_str()));
    if empty {
        crate::backdrop::show_centered_status(ui, coverage, status);
    } else {
        crate::backdrop::show(ui, coverage, status);
    }

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

mod render;
use render::*;

#[cfg(test)]
mod tests;
