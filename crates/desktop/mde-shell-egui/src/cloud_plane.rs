//! QC-12 (QUASAR-CLOUD) — the Workbench **Cloud** plane, the Controller plane's
//! successor (design Q70: "the Controller plane BECOMES the Cloud plane").
//!
//! One plane for **every mesh member** (Q82 — admin + self-service, no separate
//! "My Cloud"): the five cloud resource kinds — **instances · volumes+snapshots
//! · images · networks · stacks** (Q85) — a **full launch picker**
//! (image/flavor/network/volume, Q83), launch **presets rendered from
//! fleet-state template records** any node can author (Q84,
//! `<workgroup_root>/cloud/templates/*.json` on the Syncthing share — the same
//! replicated-record idiom the node-grade / device-inventory surfaces read),
//! and a live per-user **usage** fold.
//!
//! ## Renderer, never an authority (§9 / Q40)
//!
//! The shell **never speaks raw `OpenStack`**. Every read and every mutation
//! rides the mackesd `openstack` worker's typed QC-11 Bus verbs
//! (`action/cloud/*` → `reply/<ulid>`, the same non-blocking request/reply
//! idiom the `IaC` surface drives):
//!
//! - **reads** — `get-status` (the node's converge mirror), `get-catalog` (the
//!   Keystone directory), `list-instances` (the Nova roster), and
//!   `list-resources` per kind (volumes/snapshots/images/networks/stacks/
//!   flavors — the explicit-`collection` form of the IAC-3 verb);
//! - **mutations** — `instance-start`/`instance-stop` (direct),
//!   `instance-reboot`/`instance-delete` (typed-armed), and `heat-create`/
//!   `heat-delete` (typed-armed). **Creation is stacks-as-code** (Q61 — fleet
//!   renders Heat, Heat executes): the launch picker and the per-kind New
//!   forms compose a HOT template and drive the audited `heat-create` verb, so
//!   a launched instance / volume / network / registered image is a managed
//!   stack the Stacks kind can later tear down with `heat-delete`. No new verb
//!   surface is invented here (§6 — glue over the existing contract).
//!
//! Every state is honest (§7): no Bus → an honest degrade; an unconfigured
//! node reads the worker's gated reason ("`OpenStack` not configured"); a
//! transport failure reads unreachable; an empty roster is a real `EmptyState` —
//! never demo rows, never a dead button (destructive ops always pass the typed
//! arming echo first).
//!
//! ## Where the plane's state lives
//!
//! [`CloudPlaneState`] is a plain field on the shell's `Shell` struct, like
//! every other surface's state — single-threaded UI state the Workbench borrows
//! (`&mut`) while the Cloud plane is in view, and the shell drains its one-shot
//! console-attach hand-off ([`CloudPlaneState::take_console_attach`]) after the
//! Workbench frame renders. Pure logic (reply folds, HOT composition, preset
//! records, the usage fold, the arming gate) is egui-free and unit-tested
//! directly.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use mde_egui::egui::{self, Color32, RichText};
use mde_egui::Style;

use mackes_mesh_types::openstack::{ResourceTable, ServiceCatalog, HOT_TEMPLATE_VERSION};
use mackes_mesh_types::peers::default_workgroup_root;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::{publish_request, reply_topic};

use crate::bus_reader::BusReader;

use crate::auth::DesktopAuth;
use crate::vdi::{
    ConnectRequest, DesktopEndpoint, DisplayMode, MonitorSpan, RequestedTarget, VdiProtocol,
};

/// The `action/cloud/` namespace every QC-11 verb request rides.
const CLOUD_ACTION_PREFIX: &str = "action/cloud/";

/// The QC-11 read verbs this plane consumes.
const STATUS_VERB: &str = "get-status";
const CATALOG_VERB: &str = "get-catalog";
const INSTANCES_VERB: &str = "list-instances";
const RESOURCES_VERB: &str = "list-resources";

/// The QC-11 mutation verbs this plane dispatches. Creation is stacks-as-code
/// (`heat-create`); teardown of a managed stack is `heat-delete`; the four
/// `instance-*` lifecycle verbs drive the Nova roster.
const HEAT_CREATE_VERB: &str = "heat-create";
const HEAT_DELETE_VERB: &str = "heat-delete";
const ENSURE_MESH_KEYPAIR_VERB: &str = "ensure-mesh-keypair";
const INSTANCE_CONSOLE_VERB: &str = "get-instance-console";
const MESH_KEYPAIR_NAME: &str = "mcnf-mesh";

/// The auto-poll cadence — matches the `IaC` surface's catalog cadence.
const REFRESH: Duration = Duration::from_secs(15);

/// How long a published request waits before it reads as unanswered (§7 — an
/// honest "the cloud didn't respond", distinct from a gated/failed reply).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);

/// The in-view repaint heartbeat (the Chat / `IaC` tail idiom).
const POLL_REPAINT: Duration = Duration::from_secs(1);

/// The shared filled-circle status dot (the datacenter / Instances glyph).
const DOT: &str = "\u{25CF}";

/// The fleet-state launch-preset records' directory on the Syncthing share
/// (Q84) — `<workgroup_root>/cloud/templates/*.json`, beside the cloud
/// doctrine's `cloud/doctrine.toml` companion. Any node authors records (§0).
fn presets_dir(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("cloud").join("templates")
}

// ─────────────────────────── the Bus reply mirror ───────────────────────────
// Local serde mirrors of the mackesd `CloudReply` / mirror payloads (§6 — the
// shell reads the JSON boundary without depending on the daemon crate; only the
// mesh-neutral `mackes_mesh_types::openstack` shapes are shared).

/// The shell-side mirror of the worker's unified `CloudReply` — only the fields
/// this plane folds are named; `ok` + `gated` + `error` are the honest
/// tri-state every verb answers with.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct CloudReply {
    /// `true` when a payload answers; `false` on gate/failure/rejection.
    ok: bool,
    /// `get-status` — the node's converge mirror.
    status: Option<MirrorStatus>,
    /// `get-catalog` — the Keystone service directory.
    catalog: Option<ServiceCatalog>,
    /// `list-instances` — the Nova roster.
    instances: Option<Vec<InstanceRow>>,
    /// `list-resources` — one kind's resource table.
    resources: Option<ResourceTable>,
    /// The instance a lifecycle verb acted on, on success.
    instance: Option<String>,
    /// `ensure-mesh-keypair` — the Nova keypair backing mesh SSH injection.
    keypair: Option<String>,
    /// `get-instance-console` — the Nova SPICE console descriptor.
    console: Option<ConsoleInfo>,
    /// The stack a Heat mutation acted on / created, on success.
    stack: Option<String>,
    /// An honest gate reason (no clouds.yaml / doctrine off / nova down).
    gated: Option<String>,
    /// A rejection or a seam failure (auth / transport / CLI error).
    error: Option<String>,
}

/// The doctrine leg of the node mirror (mirrors the worker's `DoctrineStatus`;
/// an unrecognized future tag folds to the honest [`Doctrine::Unknown`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Doctrine {
    /// The cloud is declared; this node converges on its service set.
    Enabled {
        /// This node holds the leader lease (hosts `MariaDB` — Q15).
        leader: bool,
        /// The pinned Kolla release the doctrine names (Q69).
        #[serde(default)]
        kolla_release: String,
    },
    /// The fleet state declares no cloud here.
    Disabled,
    /// The doctrine couldn't be read — the typed reason.
    Gated {
        /// Why the doctrine read gated.
        reason: String,
    },
    /// Not carried / an unrecognized future variant — honest unknown.
    #[default]
    #[serde(other)]
    Unknown,
}

/// The container-runtime leg of the node mirror (mirrors `RuntimeStatus`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Runtime {
    /// Podman answered.
    Available,
    /// Podman is absent/unreachable — the typed reason.
    Unavailable {
        /// Why the runtime is unavailable.
        reason: String,
    },
    /// Not carried / unrecognized — honest unknown.
    #[default]
    #[serde(other)]
    Unknown,
}

/// One desired service's honest state (mirrors the worker's `ServiceStatus`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum ServiceState {
    /// The container is running.
    Running,
    /// The container exists but isn't running.
    NotRunning {
        /// Podman's raw state.
        #[serde(default)]
        podman_state: String,
    },
    /// The service is gated behind a missing prerequisite.
    Gated {
        /// Why.
        #[serde(default)]
        reason: String,
    },
    /// The converge step for this service failed.
    Failed {
        /// Why.
        #[serde(default)]
        reason: String,
    },
    /// The worker itself reported unknown.
    Unknown {
        /// Why.
        #[serde(default)]
        reason: String,
    },
    /// An unrecognized future variant — honest unknown.
    #[default]
    #[serde(other)]
    Unrecognized,
}

/// One desired-service row of the node mirror.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct MirrorServiceRow {
    /// The service container name (`nova_api`, …).
    service: String,
    /// Its honest state.
    status: ServiceState,
}

/// The node's `OpenStack` converge mirror — the `get-status` payload (a local
/// mirror of the worker's `OpenStackState`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct MirrorStatus {
    /// The publishing node.
    host: String,
    /// What the fleet doctrine said this tick.
    doctrine: Doctrine,
    /// Whether the container runtime answered.
    runtime: Runtime,
    /// One row per desired service.
    services: Vec<MirrorServiceRow>,
}

impl MirrorStatus {
    /// `(running, total)` across the desired services.
    fn service_tally(&self) -> (usize, usize) {
        let running = self
            .services
            .iter()
            .filter(|r| r.status == ServiceState::Running)
            .count();
        (running, self.services.len())
    }
}

/// One Nova instance as `list-instances` reports it (a local mirror of the
/// worker's `CloudInstance`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
struct InstanceRow {
    /// The Nova server id.
    id: String,
    /// The server name.
    name: String,
    /// The Nova status (`ACTIVE` / `SHUTOFF` / `ERROR` / …).
    status: String,
    /// The flavor, when the listing carried it.
    flavor: Option<String>,
    /// The image, when the listing carried it.
    image: Option<String>,
    /// The networks column, when present.
    networks: Option<String>,
}

/// A Nova SPICE console descriptor mirrored from the daemon reply.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
struct ConsoleInfo {
    /// The Nova server id/name the console belongs to.
    instance: String,
    /// The console protocol/type Nova returned.
    protocol: String,
    /// The URL Nova returned.
    url: String,
}

// ─────────────────────────── poll lanes ───────────────────────────

/// One in-flight request awaiting its `reply/<ulid>`.
#[derive(Debug, Clone)]
struct Pending {
    /// The request ULID (the reply correlation key).
    ulid: String,
    /// When it was published (drives [`REQUEST_TIMEOUT`]).
    sent: Instant,
}

/// The honest outcome of one lane's last request (§7 — never fabricated).
#[derive(Debug, Clone)]
enum LaneOutcome<T> {
    /// The live payload.
    Ready(T),
    /// The worker's gated reply — the node isn't configured for the cloud.
    NotConfigured(String),
    /// A real failure (auth / transport / rejection / no responder).
    Failed(String),
}

/// One verb lane's poll bookkeeping + last honest outcome.
#[derive(Debug)]
struct Lane<T> {
    /// The in-flight request, if any.
    pending: Option<Pending>,
    /// When the last request settled (the cadence anchor).
    settled_at: Option<Instant>,
    /// A refresh is queued — fire on the next drive regardless of cadence.
    forced: bool,
    /// The last honest outcome (`None` before the first answer = "querying").
    outcome: Option<LaneOutcome<T>>,
}

impl<T> Default for Lane<T> {
    fn default() -> Self {
        Self {
            pending: None,
            settled_at: None,
            forced: false,
            outcome: None,
        }
    }
}

impl<T> Lane<T> {
    /// Whether a fresh request is due: nothing in flight, and either a queued
    /// refresh, the first fetch, or the cadence elapsed.
    fn due(&self, now: Instant) -> bool {
        self.pending.is_none()
            && (self.forced
                || self
                    .settled_at
                    .is_none_or(|t| now.duration_since(t) >= REFRESH))
    }

    /// The lane's `Ready` payload, if the last answer carried one.
    const fn ready(&self) -> Option<&T> {
        match &self.outcome {
            Some(LaneOutcome::Ready(t)) => Some(t),
            _ => None,
        }
    }
}

/// Open the Bus persist mirror at `bus_root`, if reachable.
/// arch-11: opens through the shared [`BusReader`] seam.
fn open_persist(bus_root: Option<&PathBuf>) -> Option<Persist> {
    BusReader::new(bus_root.cloned()).open()
}

/// Read the reply on `reply/<ulid>`, if one has landed (oldest wins — the RPC
/// convention).
fn read_reply(bus_root: Option<&PathBuf>, ulid: &str) -> Option<CloudReply> {
    let persist = open_persist(bus_root)?;
    let msgs = persist.list_since(&reply_topic(ulid), None).ok()?;
    let body = msgs.first()?.body.as_deref()?;
    serde_json::from_str::<CloudReply>(body).ok()
}

/// Publish an `action/cloud/<verb>` request, answering the pending handle or an
/// honest error string (a missing Bus degrades, never panics — §7).
fn publish_verb(
    bus_root: Option<&PathBuf>,
    verb: &str,
    body: Option<&str>,
) -> Result<Pending, String> {
    let persist =
        open_persist(bus_root).ok_or_else(|| "the local mesh Bus is unavailable".to_string())?;
    let topic = format!("{CLOUD_ACTION_PREFIX}{verb}");
    publish_request(&persist, &topic, Priority::Default, None, body)
        .map(|ulid| Pending {
            ulid,
            sent: Instant::now(),
        })
        .map_err(|e| e.to_string())
}

/// Drive one lane a step: resolve its in-flight reply (or an honest timeout),
/// then issue a fresh request when due. Non-blocking — the same
/// publish-then-poll idiom every Bus surface uses. A prior good outcome is
/// never clobbered by a transient miss.
fn drive_lane<T>(
    bus_root: Option<&PathBuf>,
    lane: &mut Lane<T>,
    now: Instant,
    verb: &str,
    body: Option<&str>,
    fold: impl FnOnce(CloudReply) -> LaneOutcome<T>,
    what: &str,
) {
    if let Some((ulid, sent)) = lane.pending.as_ref().map(|p| (p.ulid.clone(), p.sent)) {
        if let Some(reply) = read_reply(bus_root, &ulid) {
            lane.outcome = Some(fold(reply));
            lane.pending = None;
            lane.settled_at = Some(now);
        } else if sent.elapsed() >= REQUEST_TIMEOUT {
            if lane.outcome.is_none() {
                lane.outcome = Some(LaneOutcome::Failed(format!(
                    "the cloud did not answer the {what} request — OpenStack may not be \
                     running on this node"
                )));
            }
            lane.pending = None;
            lane.settled_at = Some(now);
        }
    }
    if lane.due(now) {
        match publish_verb(bus_root, verb, body) {
            Ok(p) => {
                lane.pending = Some(p);
                lane.forced = false;
            }
            Err(e) => {
                if lane.outcome.is_none() {
                    lane.outcome = Some(LaneOutcome::Failed(e));
                }
                lane.settled_at = Some(now);
                lane.forced = false;
            }
        }
    }
}

/// Fold a reply into a lane outcome via `take` (the payload extractor) —
/// `ok`-without-payload is a real failure, a gate reads not-configured, an
/// error reads failed. The one honest tri-state every lane shares.
fn fold_with<T>(
    reply: CloudReply,
    take: impl FnOnce(CloudReply) -> Option<T>,
    missing: &str,
) -> LaneOutcome<T> {
    if reply.ok {
        take(reply).map_or_else(
            || LaneOutcome::Failed(missing.to_string()),
            LaneOutcome::Ready,
        )
    } else if let Some(gated) = reply.gated {
        LaneOutcome::NotConfigured(gated)
    } else {
        LaneOutcome::Failed(
            reply
                .error
                .unwrap_or_else(|| "unknown cloud error".to_string()),
        )
    }
}

// ─────────────────────────── the resource kinds (Q85) ───────────────────────────

/// One listable cloud resource kind — the `list-resources` lanes behind the
/// five Q85 kinds plus the flavor list the launch picker consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ResKind {
    /// Cinder volumes.
    Volumes,
    /// Cinder volume snapshots (the "+snapshots" half of the volume kind).
    Snapshots,
    /// Glance images.
    Images,
    /// Neutron networks.
    Networks,
    /// Heat stacks — the management handle for everything launched here.
    Stacks,
    /// Nova flavors (picker input — not a top-level kind).
    Flavors,
}

impl ResKind {
    /// The display label.
    const fn label(self) -> &'static str {
        match self {
            Self::Volumes => "Volumes",
            Self::Snapshots => "Snapshots",
            Self::Images => "Images",
            Self::Networks => "Networks",
            Self::Stacks => "Stacks",
            Self::Flavors => "Flavors",
        }
    }

    /// The REST collection the kind lists (the explicit-`collection` form of
    /// the `list-resources` verb — the same Kolla-convention paths the shared
    /// `default_collection` uses, plus the snapshot/flavor sub-collections).
    const fn collection(self) -> &'static str {
        match self {
            Self::Volumes => "volumes/detail",
            Self::Snapshots => "snapshots/detail",
            Self::Images => "v2/images",
            Self::Networks => "v2.0/networks",
            Self::Stacks => "stacks",
            Self::Flavors => "flavors/detail",
        }
    }

    /// The cataloged Keystone service **type** carrying this kind, resolved
    /// against the live catalog (`None` when the cloud doesn't advertise one —
    /// rendered honestly, never guessed).
    fn service_type(self, catalog: &ServiceCatalog) -> Option<String> {
        let family: &[&str] = match self {
            Self::Volumes | Self::Snapshots => &[
                "volumev3",
                "volumev2",
                "volume",
                "block-storage",
                "block-store",
            ],
            Self::Images => &["image"],
            Self::Networks => &["network"],
            Self::Stacks => &["orchestration"],
            Self::Flavors => &["compute", "compute_legacy"],
        };
        family
            .iter()
            .find_map(|t| catalog.service(t).map(|s| s.service_type.clone()))
    }
}

/// Which view of the plane is showing: the five Q85 kinds + the usage fold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CloudTab {
    /// The Nova roster + the full launch picker (Q83).
    #[default]
    Instances,
    /// Cinder volumes + snapshots.
    Volumes,
    /// Glance images.
    Images,
    /// Neutron networks.
    Networks,
    /// Heat stacks (the management handle for launched resources).
    Stacks,
    /// The live per-user usage fold (Q82).
    Usage,
}

impl CloudTab {
    /// Tab order.
    const ALL: [Self; 6] = [
        Self::Instances,
        Self::Volumes,
        Self::Images,
        Self::Networks,
        Self::Stacks,
        Self::Usage,
    ];

    /// The tab label.
    const fn label(self) -> &'static str {
        match self {
            Self::Instances => "Instances",
            Self::Volumes => "Volumes",
            Self::Images => "Images",
            Self::Networks => "Networks",
            Self::Stacks => "Stacks",
            Self::Usage => "Usage",
        }
    }

    /// The resource lanes this tab reads (the Instances lane is separate).
    const fn kinds(self) -> &'static [ResKind] {
        match self {
            Self::Instances => &[],
            Self::Volumes => &[ResKind::Volumes, ResKind::Snapshots],
            Self::Images => &[ResKind::Images],
            Self::Networks => &[ResKind::Networks],
            Self::Stacks => &[ResKind::Stacks],
            Self::Usage => &[
                ResKind::Volumes,
                ResKind::Snapshots,
                ResKind::Images,
                ResKind::Networks,
                ResKind::Stacks,
            ],
        }
    }
}

// ─────────────────────────── HOT composition (stacks-as-code) ───────────────────────────

/// Quote a scalar for a YAML value when it needs it (mirrors the shared
/// reverse-generator's rule); a plain token is emitted bare.
fn yaml_scalar(value: &str) -> String {
    if value.is_empty() || value.contains(':') || value.contains('#') || value != value.trim() {
        format!("{value:?}")
    } else {
        value.to_string()
    }
}

/// The shared HOT header every composed stack opens with.
fn hot_header(what: &str) -> String {
    format!(
        "heat_template_version: {HOT_TEMPLATE_VERSION}\n\ndescription: >-\n  {what} launched \
         from the MCNF Workbench Cloud plane (QC-12).\n\nresources:\n"
    )
}

/// Compose the launch picker's HOT template (Q83): an `OS::Nova::Server` from
/// the picked image + flavor, optionally on the picked network, optionally with
/// an attached new Cinder volume. Driven through the audited `heat-create`
/// verb, so the launched instance is a managed stack (Q61).
fn launch_hot(
    name: &str,
    image: &str,
    flavor: &str,
    network: Option<&str>,
    volume_gb: Option<u32>,
) -> String {
    use std::fmt::Write as _;
    let mut out = hot_header("An instance");
    out.push_str("  server:\n    type: OS::Nova::Server\n    properties:\n");
    let _ = writeln!(out, "      name: {}", yaml_scalar(name));
    let _ = writeln!(out, "      image: {}", yaml_scalar(image));
    let _ = writeln!(out, "      flavor: {}", yaml_scalar(flavor));
    let _ = writeln!(out, "      key_name: {MESH_KEYPAIR_NAME}");
    out.push_str("      metadata:\n        mcnf:ssh-key-source: mesh-ssh-key\n");
    if let Some(net) = network {
        out.push_str("      networks:\n");
        let _ = writeln!(out, "        - network: {}", yaml_scalar(net));
    }
    if let Some(gb) = volume_gb {
        out.push_str("  data_volume:\n    type: OS::Cinder::Volume\n    properties:\n");
        let _ = writeln!(out, "      name: {}", yaml_scalar(&format!("{name}-data")));
        let _ = writeln!(out, "      size: {gb}");
        out.push_str(
            "  data_volume_attachment:\n    type: OS::Cinder::VolumeAttachment\n    \
             properties:\n      instance_uuid: { get_resource: server }\n      volume_id: \
             { get_resource: data_volume }\n",
        );
    }
    out
}

/// Compose a standalone-volume HOT stack (the Volumes tab's New form).
fn volume_hot(name: &str, gb: u32) -> String {
    use std::fmt::Write as _;
    let mut out = hot_header("A volume");
    out.push_str("  volume:\n    type: OS::Cinder::Volume\n    properties:\n");
    let _ = writeln!(out, "      name: {}", yaml_scalar(name));
    let _ = writeln!(out, "      size: {gb}");
    out
}

/// Compose a network+subnet HOT stack (the Networks tab's New form).
fn network_hot(name: &str, cidr: &str) -> String {
    use std::fmt::Write as _;
    let mut out = hot_header("A network");
    out.push_str("  net:\n    type: OS::Neutron::Net\n    properties:\n");
    let _ = writeln!(out, "      name: {}", yaml_scalar(name));
    out.push_str("  subnet:\n    type: OS::Neutron::Subnet\n    properties:\n");
    out.push_str("      network: { get_resource: net }\n");
    let _ = writeln!(out, "      cidr: {}", yaml_scalar(cidr));
    out
}

/// Compose a web-image registration HOT stack (the Images tab's Register
/// form) — `OS::Glance::WebImage` fetches `url` into Glance.
fn image_hot(name: &str, url: &str) -> String {
    use std::fmt::Write as _;
    let mut out = hot_header("An image");
    out.push_str("  image:\n    type: OS::Glance::WebImage\n    properties:\n");
    let _ = writeln!(out, "      name: {}", yaml_scalar(name));
    let _ = writeln!(out, "      location: {}", yaml_scalar(url));
    out.push_str("      container_format: bare\n      disk_format: qcow2\n");
    out
}

// ─────────────────────────── launch presets (Q84) ───────────────────────────

/// One fleet-state launch-preset record (Q84) — a JSON file on the replicated
/// share any node can author; the picker renders each as a one-click preset.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct LaunchPreset {
    /// The preset's display name (also the saved instance-name prefill).
    name: String,
    /// Optional operator-facing description.
    description: String,
    /// The Glance image to boot.
    image: String,
    /// The Nova flavor.
    flavor: String,
    /// The Neutron network (empty ⇒ unpinned).
    network: String,
    /// An attached new data volume, GiB (absent ⇒ none).
    volume_gb: Option<u32>,
}

/// Read every `*.json` preset record in `dir`, name-sorted, plus an honest
/// error line per malformed record (§7 — a broken record is named, never
/// silently dropped). An absent directory is a clean empty set (no node has
/// authored a preset yet).
fn read_presets(dir: &Path) -> (Vec<LaunchPreset>, Vec<String>) {
    let mut presets: Vec<(String, LaunchPreset)> = Vec::new();
    let mut errors = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (Vec::new(), errors);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let file = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|body| serde_json::from_str::<LaunchPreset>(&body).map_err(|e| e.to_string()))
        {
            Ok(p) => presets.push((file, p)),
            Err(e) => errors.push(format!("preset {file}: {e}")),
        }
    }
    presets.sort_by(|a, b| a.0.cmp(&b.0));
    (presets.into_iter().map(|(_, p)| p).collect(), errors)
}

/// Write `preset` as a new record in `dir` (any node authors fleet state, §0).
/// The filename is a slug of the preset name.
fn save_preset(dir: &Path, preset: &LaunchPreset) -> Result<PathBuf, String> {
    if preset.name.trim().is_empty()
        || preset.image.trim().is_empty()
        || preset.flavor.trim().is_empty()
    {
        return Err("a preset needs a name, an image, and a flavor".to_string());
    }
    let slug: String = preset
        .name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        return Err("the preset name has no usable characters for a record name".to_string());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{slug}.json"));
    let body = serde_json::to_string_pretty(preset).map_err(|e| e.to_string())?;
    std::fs::write(&path, body).map_err(|e| e.to_string())?;
    Ok(path)
}

// ─────────────────────────── the usage fold (Q82) ───────────────────────────

/// The live usage fold the Usage tab renders — real counts off the live lanes,
/// with a per-user rollup wherever a listing carried user attribution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UsageView {
    /// Total instances / active / errored, from the Nova roster.
    instances: Option<(usize, usize, usize)>,
    /// `(kind label, row count, extra)` per settled resource lane — `extra`
    /// carries the volumes' summed GiB when the listing had a `size` column.
    kinds: Vec<(&'static str, usize, Option<String>)>,
    /// user → kind label → owned-row count, from every table carrying a
    /// `user_id`/`owner` column. Empty when no listing attributed users.
    per_user: BTreeMap<String, BTreeMap<&'static str, usize>>,
}

/// Fold the Usage view from the live lanes (pure — unit-tested directly).
fn fold_usage(
    instances: Option<&[InstanceRow]>,
    tables: &[(&'static str, &ResourceTable)],
) -> UsageView {
    let instances = instances.map(|rows| {
        let active = rows
            .iter()
            .filter(|r| r.status.eq_ignore_ascii_case("ACTIVE"))
            .count();
        let errored = rows
            .iter()
            .filter(|r| r.status.eq_ignore_ascii_case("ERROR"))
            .count();
        (rows.len(), active, errored)
    });

    let mut kinds = Vec::new();
    let mut per_user: BTreeMap<String, BTreeMap<&'static str, usize>> = BTreeMap::new();
    for (label, table) in tables {
        let extra = table.column_index("size").map(|i| {
            let total: u64 = table
                .rows
                .iter()
                .filter_map(|r| r.cells.get(i))
                .filter_map(|c| c.parse::<u64>().ok())
                .sum();
            format!("{total} GiB")
        });
        kinds.push((*label, table.rows.len(), extra));

        if let Some(i) = table
            .column_index("user_id")
            .or_else(|| table.column_index("owner"))
        {
            for row in &table.rows {
                if let Some(user) = row.cells.get(i).filter(|u| !u.trim().is_empty()) {
                    *per_user
                        .entry(user.clone())
                        .or_default()
                        .entry(*label)
                        .or_default() += 1;
                }
            }
        }
    }
    UsageView {
        instances,
        kinds,
        per_user,
    }
}

// ─────────────────────────── typed arming ───────────────────────────

/// The typed-arming gate: the operator's echo, trimmed, must equal the target
/// exactly before a destructive mutation may publish (the `IaC` #22 idiom).
fn armed(typed: &str, target: &str) -> bool {
    typed.trim() == target
}

/// What an armed confirm will publish once the echo matches.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ArmAction {
    /// An `instance-reboot` / `instance-delete` on a Nova instance.
    Lifecycle {
        /// The verb.
        verb: &'static str,
        /// The Nova server id.
        instance_id: String,
    },
    /// A `heat-create` of a composed stack (launch / new volume / new network /
    /// image registration — creation is stacks-as-code).
    HeatCreate {
        /// The stack name.
        stack_name: String,
        /// The composed HOT template.
        template: String,
    },
    /// A `heat-delete` of a managed stack.
    HeatDelete {
        /// The stack id.
        stack_id: String,
        /// The stack name.
        stack_name: String,
    },
}

impl ArmAction {
    /// The confirm-button word.
    fn word(&self) -> &'static str {
        match self {
            Self::Lifecycle { verb, .. } => match *verb {
                "instance-reboot" => "Reboot",
                _ => "Delete",
            },
            Self::HeatCreate { .. } => "Create",
            Self::HeatDelete { .. } => "Delete",
        }
    }

    /// What the confirm acts on, for the arming copy.
    const fn subject(&self) -> &'static str {
        match self {
            Self::Lifecycle { .. } => "instance",
            Self::HeatCreate { .. } | Self::HeatDelete { .. } => "stack",
        }
    }
}

/// A pending typed-arming confirm — the echo the operator must type plus the
/// action it releases.
#[derive(Debug, Clone)]
struct CloudArming {
    /// What confirming publishes.
    action: ArmAction,
    /// The name the operator must type.
    target: String,
    /// The operator's echo so far.
    typed: String,
}

// ─────────────────────────── the plane state ───────────────────────────

/// The Cloud plane's state: the verb lanes, the launch picker + per-kind New
/// forms, the fleet-state presets, the typed-arming confirm, and the one
/// in-flight mutation. A plain `Shell` field, borrowed by the Workbench while
/// the plane is in view (see the module docs).
pub struct CloudPlaneState {
    /// The Bus persist root (`None` = no Bus — an honest degrade).
    bus_root: Option<PathBuf>,
    /// The replicated workgroup root the Q84 preset records ride.
    workgroup_root: PathBuf,
    /// Which tab is showing.
    tab: CloudTab,
    /// The `get-status` lane (the node's converge mirror — the status band).
    status: Lane<MirrorStatus>,
    /// The `get-catalog` lane (resolves each kind's cataloged service type).
    catalog: Lane<ServiceCatalog>,
    /// The `list-instances` lane (the Nova roster).
    instances: Lane<Vec<InstanceRow>>,
    /// The per-kind `list-resources` lanes.
    resources: BTreeMap<ResKind, Lane<ResourceTable>>,
    /// The launch picker (Q83).
    picker: LaunchPicker,
    /// The Volumes tab's New-volume form: `(open, name, size)`.
    volume_form: (bool, String, String),
    /// The Networks tab's New-network form: `(open, name, cidr)`.
    network_form: (bool, String, String),
    /// The Images tab's Register-image form: `(open, name, url)`.
    image_form: (bool, String, String),
    /// The loaded Q84 preset records + honest per-record errors.
    presets: Vec<LaunchPreset>,
    /// Malformed-preset error lines (named honestly, never dropped).
    preset_errors: Vec<String>,
    /// When the presets were last (re)read from the share.
    presets_loaded_at: Option<Instant>,
    /// A pending typed-arming confirm, if any.
    arming: Option<CloudArming>,
    /// The one in-flight mutation (`instance-*` / `heat-*`) — its reply lands
    /// in the note. A newly-issued mutation replaces an unresolved one (the
    /// abandoned reply is simply never read).
    mutation_pending: Option<Pending>,
    /// One-shot native Desktop attach request produced by a dialable console
    /// descriptor. Drained by the shell after the Workbench frame renders.
    console_attach: Option<ConnectRequest>,
    /// A transient one-line action note — honest feedback, never a silent op.
    note: Option<String>,
}

/// The full launch picker (Q83): name + image/flavor/network picks + an
/// optional new data volume, validated into a HOT template on Launch.
#[derive(Debug, Default)]
struct LaunchPicker {
    /// Whether the picker is open.
    open: bool,
    /// The instance (and stack) name.
    name: String,
    /// The picked Glance image.
    image: String,
    /// The picked Nova flavor.
    flavor: String,
    /// The picked Neutron network (empty ⇒ omitted from the template).
    network: String,
    /// Optional new data-volume size, GiB (empty ⇒ none).
    volume_gb: String,
    /// Inline validation error (honest; never a panic).
    error: Option<String>,
}

impl LaunchPicker {
    /// Validate and compose the launch stack: `(stack name, HOT template)`.
    fn compose(&self) -> Result<(String, String), String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("an instance name is required".to_string());
        }
        let image = self.image.trim();
        if image.is_empty() {
            return Err("pick an image to boot".to_string());
        }
        let flavor = self.flavor.trim();
        if flavor.is_empty() {
            return Err("pick a flavor".to_string());
        }
        let network = Some(self.network.trim()).filter(|n| !n.is_empty());
        let volume_gb = match self.volume_gb.trim() {
            "" => None,
            raw => Some(
                raw.parse::<u32>()
                    .ok()
                    .filter(|gb| *gb > 0)
                    .ok_or_else(|| "volume size must be a whole number of GiB".to_string())?,
            ),
        };
        Ok((
            name.to_string(),
            launch_hot(name, image, flavor, network, volume_gb),
        ))
    }

    /// Prefill the picker from a Q84 preset record.
    fn apply_preset(&mut self, preset: &LaunchPreset) {
        self.open = true;
        self.name.clone_from(&preset.name);
        self.image.clone_from(&preset.image);
        self.flavor.clone_from(&preset.flavor);
        self.network.clone_from(&preset.network);
        self.volume_gb = preset.volume_gb.map(|g| g.to_string()).unwrap_or_default();
        self.error = None;
    }

    /// The picker's current fields as a Q84 preset record (Save as preset).
    fn as_preset(&self) -> LaunchPreset {
        LaunchPreset {
            name: self.name.trim().to_string(),
            description: String::new(),
            image: self.image.trim().to_string(),
            flavor: self.flavor.trim().to_string(),
            network: self.network.trim().to_string(),
            volume_gb: self.volume_gb.trim().parse::<u32>().ok().filter(|g| *g > 0),
        }
    }
}

impl Default for CloudPlaneState {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            workgroup_root: default_workgroup_root(),
            tab: CloudTab::default(),
            status: Lane::default(),
            catalog: Lane::default(),
            instances: Lane::default(),
            resources: BTreeMap::new(),
            picker: LaunchPicker::default(),
            volume_form: (false, String::new(), String::new()),
            network_form: (false, String::new(), String::new()),
            image_form: (false, String::new(), String::new()),
            presets: Vec::new(),
            preset_errors: Vec::new(),
            presets_loaded_at: None,
            arming: None,
            mutation_pending: None,
            console_attach: None,
            note: None,
        }
    }
}

impl CloudPlaneState {
    /// Drain a native SPICE attach request produced by the Cloud plane, if the
    /// latest Nova console descriptor was directly dialable. The shell calls
    /// this after rendering Workbench and routes the request into
    /// [`crate::vdi::VdiState`].
    pub(crate) fn take_console_attach(&mut self) -> Option<ConnectRequest> {
        self.console_attach.take()
    }

    /// Poll the Bus lanes on the shared cadence + keep the repaint heartbeat
    /// alive — called each frame while the plane is in view (the Explorer-lens
    /// visibility idiom: an off-screen plane costs nothing).
    fn poll(&mut self, ctx: &egui::Context) {
        let now = Instant::now();

        self.resolve_mutation();

        drive_lane(
            self.bus_root.as_ref(),
            &mut self.status,
            now,
            STATUS_VERB,
            None,
            |r| fold_with(r, |r| r.status, "the status reply carried no mirror"),
            "status",
        );
        drive_lane(
            self.bus_root.as_ref(),
            &mut self.catalog,
            now,
            CATALOG_VERB,
            None,
            |r| fold_with(r, |r| r.catalog, "the catalog reply carried no directory"),
            "catalog",
        );

        // The active tab's lanes (plus the picker's inputs while it is open).
        match self.tab {
            CloudTab::Instances => {
                drive_lane(
                    self.bus_root.as_ref(),
                    &mut self.instances,
                    now,
                    INSTANCES_VERB,
                    None,
                    |r| fold_with(r, |r| r.instances, "the roster reply carried no instances"),
                    "instance roster",
                );
                if self.picker.open {
                    for kind in [ResKind::Images, ResKind::Flavors, ResKind::Networks] {
                        self.drive_kind(now, kind);
                    }
                }
            }
            CloudTab::Usage => {
                drive_lane(
                    self.bus_root.as_ref(),
                    &mut self.instances,
                    now,
                    INSTANCES_VERB,
                    None,
                    |r| fold_with(r, |r| r.instances, "the roster reply carried no instances"),
                    "instance roster",
                );
                for kind in self.tab.kinds() {
                    self.drive_kind(now, *kind);
                }
            }
            _ => {
                for kind in self.tab.kinds() {
                    self.drive_kind(now, *kind);
                }
            }
        }

        // The Q84 preset records, re-read on the same cadence (a cheap local
        // dir scan of the replicated share).
        if self
            .presets_loaded_at
            .is_none_or(|t| now.duration_since(t) >= REFRESH)
        {
            let (presets, errors) = read_presets(&presets_dir(&self.workgroup_root));
            self.presets = presets;
            self.preset_errors = errors;
            self.presets_loaded_at = Some(now);
        }

        ctx.request_repaint_after(POLL_REPAINT);
    }

    /// Drive one kind's `list-resources` lane (needs the live catalog to know
    /// the cataloged service type; an unadvertised kind simply doesn't fire —
    /// its render says so honestly).
    fn drive_kind(&mut self, now: Instant, kind: ResKind) {
        let service = self
            .catalog
            .ready()
            .and_then(|catalog| kind.service_type(catalog));
        let Some(service) = service else {
            return;
        };
        let body = serde_json::json!({
            "service": service,
            "collection": kind.collection(),
        })
        .to_string();
        let Self {
            bus_root,
            resources,
            ..
        } = self;
        drive_lane(
            bus_root.as_ref(),
            resources.entry(kind).or_default(),
            now,
            RESOURCES_VERB,
            Some(&body),
            |r| fold_with(r, |r| r.resources, "the resource reply carried no table"),
            kind.label(),
        );
    }

    /// Resolve the in-flight mutation's reply into the note and nudge every
    /// lane to re-list so the change reflects (never a silent op, §7).
    fn resolve_mutation(&mut self) {
        let Some((ulid, sent)) = self
            .mutation_pending
            .as_ref()
            .map(|p| (p.ulid.clone(), p.sent))
        else {
            return;
        };
        if let Some(reply) = read_reply(self.bus_root.as_ref(), &ulid) {
            self.note = Some(self.apply_mutation_reply(&reply));
            self.mutation_pending = None;
            if reply.ok {
                self.instances.forced = true;
                for lane in self.resources.values_mut() {
                    lane.forced = true;
                }
            }
        } else if sent.elapsed() >= REQUEST_TIMEOUT {
            self.note = Some("the cloud did not answer the request".to_string());
            self.mutation_pending = None;
        }
    }

    /// Apply a mutation/read reply that was issued from an instance card. Console
    /// replies get one extra fold: if Nova returned a native `spice://host:port`
    /// descriptor, queue a Desktop attach for the shell to drain; if it returned
    /// the common HTML5 proxy URL, keep the descriptor visible and explain the
    /// native attach gate honestly.
    fn apply_mutation_reply(&mut self, reply: &CloudReply) -> String {
        if reply.ok {
            if let Some(console) = &reply.console {
                match console_attach_request(console) {
                    Ok(request) => {
                        self.console_attach = Some(request);
                        return format!(
                            "Console ready for {}: opening native SPICE Desktop attach.",
                            console.instance
                        );
                    }
                    Err(reason) => {
                        return format!(
                            "Console ready for {}: {} {}. Native attach gated: {reason}.",
                            console.instance, console.protocol, console.url
                        );
                    }
                }
            }
        }
        mutation_note(reply)
    }

    /// Publish a mutation verb and track its reply (the honest outcome lands
    /// in the note). A missing Bus is an honest note, never a panic.
    fn issue_mutation(&mut self, verb: &str, body: &str, label: &str) {
        match publish_verb(self.bus_root.as_ref(), verb, Some(body)) {
            Ok(pending) => {
                self.mutation_pending = Some(pending);
                self.note = Some(format!("Requested {label}\u{2026}"));
            }
            Err(e) => self.note = Some(format!("Could not request {label}: {e}")),
        }
    }

    /// Perform a confirmed armed action — called only past the typed-arming
    /// gate ([`armed`]).
    fn perform_armed(&mut self, action: ArmAction, target: &str) {
        match action {
            ArmAction::Lifecycle { verb, instance_id } => {
                let body = serde_json::json!({ "instance": instance_id }).to_string();
                self.issue_mutation(
                    verb,
                    &body,
                    &format!("{} of instance {target}", action_word(verb)),
                );
            }
            ArmAction::HeatCreate {
                stack_name,
                template,
            } => {
                let body = serde_json::json!({
                    "stack_name": stack_name,
                    "template": template,
                })
                .to_string();
                self.issue_mutation(HEAT_CREATE_VERB, &body, &format!("create of {target}"));
            }
            ArmAction::HeatDelete {
                stack_id,
                stack_name,
            } => {
                let body = serde_json::json!({
                    "stack_name": stack_name,
                    "stack_id": stack_id,
                })
                .to_string();
                self.issue_mutation(
                    HEAT_DELETE_VERB,
                    &body,
                    &format!("delete of stack {target}"),
                );
            }
        }
    }
}

/// One Cloud verb the MENU-1 "State of the Mesh" bar offers while the Cloud
/// plane is active — each the mouse twin of an affordance the plane body
/// already renders (§6, one seam; applied via
/// [`CloudPlaneState::apply_menu_verb`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudMenuVerb {
    /// Queue an immediate re-poll of every lane (status/catalog/roster/kinds)
    /// plus a preset re-read.
    Refresh,
    /// Jump to Instances and open the full launch picker (Q83).
    LaunchInstance,
    /// Jump to Volumes and open the New-volume form.
    NewVolume,
    /// Jump to Images and open the Register-image form.
    RegisterImage,
    /// Jump to Networks and open the New-network form.
    NewNetwork,
}

impl CloudPlaneState {
    /// Apply a MENU-1 Cloud verb — the same fields the in-plane toggles flip
    /// (a menu pick opens the exact affordance the body's own button opens).
    pub fn apply_menu_verb(&mut self, verb: CloudMenuVerb) {
        match verb {
            CloudMenuVerb::Refresh => {
                self.status.forced = true;
                self.catalog.forced = true;
                self.instances.forced = true;
                for lane in self.resources.values_mut() {
                    lane.forced = true;
                }
                self.presets_loaded_at = None;
            }
            CloudMenuVerb::LaunchInstance => {
                self.tab = CloudTab::Instances;
                self.picker.open = true;
                self.picker.error = None;
            }
            CloudMenuVerb::NewVolume => {
                self.tab = CloudTab::Volumes;
                self.volume_form.0 = true;
            }
            CloudMenuVerb::RegisterImage => {
                self.tab = CloudTab::Images;
                self.image_form.0 = true;
            }
            CloudMenuVerb::NewNetwork => {
                self.tab = CloudTab::Networks;
                self.network_form.0 = true;
            }
        }
    }
}

/// The lifecycle word for the mutation note.
fn action_word(verb: &str) -> &'static str {
    match verb {
        "instance-start" => "start",
        "instance-stop" => "stop",
        "instance-reboot" => "reboot",
        _ => "delete",
    }
}

/// Convert a Nova console descriptor into the native Desktop attach request, but
/// only when the descriptor names a direct SPICE endpoint. Nova's common
/// `--spice-html5` result is an HTTP proxy page; the native `mde-vdi-spice`
/// transport cannot consume that browser URL, so it remains an honest gate.
fn console_attach_request(console: &ConsoleInfo) -> Result<ConnectRequest, String> {
    if !console.protocol.to_ascii_lowercase().contains("spice") {
        return Err(format!(
            "Nova returned a {} console, but the native Desktop attach expects SPICE",
            console.protocol
        ));
    }
    let (host, port) = parse_native_spice_url(&console.url)?;
    let endpoint = DesktopEndpoint::new(host.clone(), port)
        .ok_or_else(|| "the SPICE descriptor did not contain a dialable endpoint".to_string())?;
    let request = ConnectRequest::new(
        RequestedTarget::new("openstack", console.instance.clone()).with_endpoint(Some(endpoint)),
        VdiProtocol::Spice,
        DisplayMode::Fullscreen,
        MonitorSpan::Single,
        DesktopAuth::mesh_identity("openstack"),
    );
    Ok(request)
}

/// Parse the native direct SPICE URL shapes the shell can dial:
/// `spice://host:port`, `spice://host`, and bracketed IPv6. HTML5 proxy URLs are
/// rejected with a message that names the missing native endpoint instead of
/// pretending a web console URL is a SPICE socket.
fn parse_native_spice_url(url: &str) -> Result<(String, u16), String> {
    let trimmed = url.trim();
    let Some(rest) = trimmed.strip_prefix("spice://") else {
        let scheme = trimmed.split_once(':').map(|(s, _)| s).unwrap_or("unknown");
        return Err(format!(
            "Nova returned a {scheme} URL; native attach needs a direct spice://host:port endpoint"
        ));
    };
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .trim();
    if authority.is_empty() {
        return Err("the SPICE URL did not include a host".to_string());
    }
    if let Some(after_bracket) = authority.strip_prefix('[') {
        let Some((host, tail)) = after_bracket.split_once(']') else {
            return Err("the SPICE IPv6 host is missing its closing bracket".to_string());
        };
        let port = match tail.strip_prefix(':') {
            Some(raw) if !raw.is_empty() => parse_console_port(raw)?,
            Some(_) => return Err("the SPICE URL has an empty port".to_string()),
            None => 5900,
        };
        return Ok((host.to_string(), port));
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, raw_port)) if !host.is_empty() && !raw_port.is_empty() => {
            (host, parse_console_port(raw_port)?)
        }
        Some((_host, _raw_port)) => {
            return Err("the SPICE URL did not include a valid host:port".to_string());
        }
        None => (authority, 5900),
    };
    Ok((host.to_string(), port))
}

fn parse_console_port(raw: &str) -> Result<u16, String> {
    raw.parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| format!("the SPICE URL port `{raw}` is invalid"))
}

/// Render a settled mutation reply to the honest note line.
fn mutation_note(reply: &CloudReply) -> String {
    if reply.ok {
        if let Some(console) = &reply.console {
            return format!(
                "Console ready for {}: {} {}.",
                console.instance, console.protocol, console.url
            );
        }
        if let Some(keypair) = &reply.keypair {
            return format!("Mesh SSH keypair {keypair} is present in Nova.");
        }
        let subject = reply
            .instance
            .as_deref()
            .or(reply.stack.as_deref())
            .unwrap_or("the cloud");
        format!("Completed on {subject}.")
    } else if let Some(gated) = &reply.gated {
        format!("Gated: {gated}")
    } else {
        format!(
            "Failed: {}",
            reply.error.as_deref().unwrap_or("unknown error")
        )
    }
}

// ─────────────────────────── the render ───────────────────────────

/// Render the Cloud plane: the live status band, the kind tabs, the typed
/// arming / action note, the active tab's body, and — collapsed below — the
/// mesh control plane the cloud rides on (the old Controller content, still
/// live and §6-reused: the leader lease the leader-hosted `MariaDB` follows).
pub fn show(
    ui: &mut egui::Ui,
    state: &mut CloudPlaneState,
    controller: &crate::controller::ControllerState,
) {
    state.poll(ui.ctx());

    render_status_band(ui, &state.status);
    ui.add_space(Style::SP_S);
    render_tab_bar(ui, state);
    ui.add_space(Style::SP_S);
    render_arming(ui, state);
    render_note(ui, state);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            match state.tab {
                CloudTab::Instances => render_instances_tab(ui, state),
                CloudTab::Volumes => render_volumes_tab(ui, state),
                CloudTab::Images => render_images_tab(ui, state),
                CloudTab::Networks => render_networks_tab(ui, state),
                CloudTab::Stacks => render_stacks_tab(ui, state),
                CloudTab::Usage => render_usage_tab(ui, state),
            }

            // The mesh control plane the cloud rides on — the old Controller
            // plane's live view (leader lease + control-service rollup), kept
            // reachable here because the leader-hosted DB (Q15) makes it part
            // of the cloud story (§6 reuse — same state, second placement).
            ui.add_space(Style::SP_M);
            ui.separator();
            ui.add_space(Style::SP_S);
            egui::CollapsingHeader::new(
                RichText::new("Mesh control plane")
                    .color(Style::TEXT)
                    .size(Style::BODY)
                    .strong(),
            )
            .default_open(false)
            .show(ui, |ui| controller.show(ui));
        });
}

/// The live status band from the `get-status` mirror: doctrine · runtime ·
/// services running, or the honest querying/absent line.
fn render_status_band(ui: &mut egui::Ui, status: &Lane<MirrorStatus>) {
    match &status.outcome {
        None => {
            mde_egui::muted_note(ui, "Querying the node's cloud converge mirror\u{2026}");
        }
        Some(LaneOutcome::NotConfigured(reason) | LaneOutcome::Failed(reason)) => {
            ui.colored_label(
                Style::WARN,
                RichText::new(reason.as_str()).size(Style::SMALL),
            );
        }
        Some(LaneOutcome::Ready(mirror)) => {
            ui.horizontal_wrapped(|ui| {
                match &mirror.doctrine {
                    Doctrine::Enabled {
                        leader,
                        kolla_release,
                    } => {
                        ui.colored_label(Style::OK, RichText::new(DOT).size(Style::SMALL));
                        let mut line = format!("cloud enabled ({kolla_release})");
                        if *leader {
                            line.push_str(" · this node leads");
                        }
                        ui.colored_label(Style::TEXT, RichText::new(line).size(Style::SMALL));
                    }
                    Doctrine::Disabled => {
                        ui.colored_label(Style::TEXT_DIM, RichText::new(DOT).size(Style::SMALL));
                        ui.colored_label(
                            Style::TEXT_DIM,
                            RichText::new("no cloud declared in the fleet state")
                                .size(Style::SMALL),
                        );
                    }
                    Doctrine::Gated { reason } => {
                        ui.colored_label(Style::WARN, RichText::new(DOT).size(Style::SMALL));
                        ui.colored_label(
                            Style::WARN,
                            RichText::new(format!("doctrine gated — {reason}")).size(Style::SMALL),
                        );
                    }
                    Doctrine::Unknown => {
                        ui.colored_label(Style::TEXT_DIM, RichText::new(DOT).size(Style::SMALL));
                        ui.colored_label(
                            Style::TEXT_DIM,
                            RichText::new("doctrine unknown").size(Style::SMALL),
                        );
                    }
                }
                ui.add_space(Style::SP_S);
                if let Runtime::Unavailable { reason } = &mirror.runtime {
                    ui.colored_label(
                        Style::WARN,
                        RichText::new(format!("runtime unavailable — {reason}")).size(Style::SMALL),
                    );
                    ui.add_space(Style::SP_S);
                }
                let (running, total) = mirror.service_tally();
                if total > 0 {
                    let color = if running == total {
                        Style::OK
                    } else {
                        Style::WARN
                    };
                    ui.colored_label(
                        color,
                        RichText::new(format!("{running}/{total} services running"))
                            .size(Style::SMALL),
                    );
                    ui.add_space(Style::SP_S);
                }
                mde_egui::muted_note(ui, format!("on {}", mirror.host));
            });
        }
    }
}

/// The kind tab strip (Instances | Volumes | Images | Networks | Stacks |
/// Usage), on the shared `Style` accents.
fn render_tab_bar(ui: &mut egui::Ui, state: &mut CloudPlaneState) {
    ui.horizontal(|ui| {
        for tab in CloudTab::ALL {
            let selected = state.tab == tab;
            let color = if selected {
                Style::ACCENT
            } else {
                Style::TEXT_DIM
            };
            let text = RichText::new(tab.label())
                .size(Style::BODY)
                .color(color)
                .strong();
            if ui.selectable_label(selected, text).clicked() {
                state.tab = tab;
            }
        }
    });
}

/// The pending typed-arming confirm: the operator must type the target's name
/// before the destructive/creating mutation publishes (never a one-click
/// destroy, §7).
fn render_arming(ui: &mut egui::Ui, state: &mut CloudPlaneState) {
    let mut resolved: Option<Option<(ArmAction, String)>> = None;
    if let Some(arming) = state.arming.as_mut() {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.colored_label(
                Style::WARN,
                RichText::new(format!(
                    "Confirm {} {}",
                    arming.action.word().to_lowercase(),
                    arming.action.subject()
                ))
                .size(Style::BODY)
                .strong(),
            );
            mde_egui::muted_note(
                ui,
                format!(
                    "Type \u{201C}{}\u{201D} to arm this {} \u{2014} it acts on the live cloud.",
                    arming.target,
                    arming.action.word().to_lowercase()
                ),
            );
            ui.add(egui::TextEdit::singleline(&mut arming.typed).hint_text(arming.target.as_str()));
            let is_armed = armed(&arming.typed, &arming.target);
            ui.horizontal(|ui| {
                let confirm = ui.add_enabled(
                    is_armed,
                    egui::Button::new(RichText::new(arming.action.word()).color(Style::DANGER)),
                );
                if confirm.clicked() && is_armed {
                    resolved = Some(Some((arming.action.clone(), arming.target.clone())));
                } else if ui.button("Cancel").clicked() {
                    resolved = Some(None);
                }
            });
        });
        ui.add_space(Style::SP_S);
    }
    if let Some(outcome) = resolved {
        state.arming = None;
        if let Some((action, target)) = outcome {
            state.perform_armed(action, &target);
        }
    }
}

/// The transient one-line action note with a dismiss affordance.
fn render_note(ui: &mut egui::Ui, state: &mut CloudPlaneState) {
    let Some(note) = state.note.clone() else {
        return;
    };
    ui.horizontal(|ui| {
        ui.colored_label(Style::ACCENT, RichText::new(note).size(Style::SMALL));
        if ui.small_button("dismiss").clicked() {
            state.note = None;
        }
    });
    ui.add_space(Style::SP_XS);
}

/// A section header line.
fn section_header(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .color(Style::TEXT)
            .size(Style::BODY)
            .strong(),
    );
    ui.add_space(Style::SP_XS);
}

/// The honest non-Ready lane states, shared by every kind section. Returns the
/// table when the lane is Ready (the caller renders it).
fn lane_absent<'a>(
    ui: &mut egui::Ui,
    lane: Option<&'a Lane<ResourceTable>>,
    what: &str,
) -> Option<&'a ResourceTable> {
    match lane.and_then(|l| l.outcome.as_ref()) {
        None => {
            mde_egui::muted_note(ui, format!("querying {what}\u{2026}"));
            None
        }
        Some(LaneOutcome::NotConfigured(reason)) => {
            mde_egui::muted_note(ui, format!("OpenStack not configured \u{2014} {reason}"));
            None
        }
        Some(LaneOutcome::Failed(reason)) => {
            ui.colored_label(
                Style::DANGER,
                RichText::new(reason.as_str()).size(Style::SMALL),
            );
            None
        }
        Some(LaneOutcome::Ready(table)) => Some(table),
    }
}

/// A per-row action-column renderer (the Stacks tab's armed Delete button).
type RowAction<'a> = &'a mut dyn FnMut(&mut egui::Ui, &ResourceTable, usize);

/// Render one kind's resource table: headers + rows off the live listing, with
/// an optional per-row action column (the Stacks tab's armed Delete). An empty
/// table is the honest "none yet".
fn render_table(
    ui: &mut egui::Ui,
    id_salt: &str,
    table: &ResourceTable,
    mut row_action: Option<RowAction<'_>>,
) {
    if table.rows.is_empty() {
        mde_egui::muted_note(ui, format!("no {} on the cloud yet", table.collection));
        return;
    }
    egui::Grid::new(id_salt)
        .striped(true)
        .min_col_width(Style::SP_XL)
        .show(ui, |ui| {
            ui.label(
                RichText::new("name")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL)
                    .strong(),
            );
            for col in &table.columns {
                ui.label(
                    RichText::new(col.as_str())
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL)
                        .strong(),
                );
            }
            if row_action.is_some() {
                ui.label(RichText::new("").size(Style::SMALL));
            }
            ui.end_row();
            for (idx, row) in table.rows.iter().enumerate() {
                ui.label(
                    RichText::new(table.row_label(row))
                        .color(Style::TEXT)
                        .size(Style::SMALL),
                );
                for cell in &row.cells {
                    ui.label(
                        RichText::new(cell.as_str())
                            .color(Style::TEXT_DIM)
                            .size(Style::SMALL),
                    );
                }
                if let Some(action) = row_action.as_mut() {
                    action(ui, table, idx);
                }
                ui.end_row();
            }
        });
}

/// A labelled single-line text field on the spacing grid.
fn form_field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add(egui::TextEdit::singleline(value).desired_width(Style::SP_XL * 5.0));
    });
}

/// A picker field: a combo over the live options when the source lane listed
/// some, else a plain text field with an honest availability note (§7 — a
/// combo over nothing would be a dead control).
fn pick_field(
    ui: &mut egui::Ui,
    id_salt: &str,
    label: &str,
    value: &mut String,
    options: Option<Vec<String>>,
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        if let Some(options) = options.filter(|o| !o.is_empty()) {
            egui::ComboBox::from_id_salt(id_salt)
                .selected_text(if value.is_empty() {
                    "pick\u{2026}".to_string()
                } else {
                    value.clone()
                })
                .show_ui(ui, |ui| {
                    for opt in options {
                        ui.selectable_value(value, opt.clone(), opt);
                    }
                });
        } else {
            ui.add(egui::TextEdit::singleline(value).desired_width(Style::SP_XL * 5.0));
            mde_egui::muted_note(ui, "(list not available yet \u{2014} type a name)");
        }
    });
}

/// The names a lane's listing offers the picker (row labels, name-sorted).
fn lane_names(lane: Option<&Lane<ResourceTable>>) -> Option<Vec<String>> {
    let table = lane.and_then(Lane::ready)?;
    let mut names: Vec<String> = table
        .rows
        .iter()
        .map(|r| table.row_label(r).to_string())
        .filter(|n| !n.trim().is_empty())
        .collect();
    names.sort_unstable();
    names.dedup();
    Some(names)
}

// ─────────────────────────── tab bodies ───────────────────────────

/// The **Instances** tab: the full launch picker (Q83, presets per Q84) over
/// the live Nova roster with its lifecycle verbs.
fn render_instances_tab(ui: &mut egui::Ui, state: &mut CloudPlaneState) {
    // ── the launch picker ──
    ui.horizontal(|ui| {
        section_header(ui, "Instances");
        ui.add_space(Style::SP_S);
        let toggle = if state.picker.open {
            "Close"
        } else {
            "\u{FF0B} Launch instance\u{2026}"
        };
        if ui
            .button(RichText::new(toggle).size(Style::SMALL))
            .clicked()
        {
            state.picker.open = !state.picker.open;
            if state.picker.open {
                state.picker.error = None;
            }
        }
    });
    // docs-consistency-8 — name this lens distinctly from the Fleet plane's raw
    // per-node KVM view: these are OpenStack tenant *instances* (Nova), not the
    // libvirt guests the Fleet plane lists per node as "VMs".
    mde_egui::muted_note(
        ui,
        "OpenStack tenant instances (Nova). Raw per-node KVM guests live in the Fleet plane.",
    );
    ui.add_space(Style::SP_XS);
    if state.picker.open {
        render_launch_picker(ui, state);
        ui.add_space(Style::SP_S);
    }

    // ── the roster ── (snapshotted so the per-card lifecycle buttons can take
    // `&mut state` without holding the lane borrow)
    let mut roster: Option<Vec<InstanceRow>> = None;
    match &state.instances.outcome {
        None => {
            mde_egui::muted_note(ui, "querying the Nova roster\u{2026}");
        }
        Some(LaneOutcome::NotConfigured(reason)) => {
            crate::session::empty_state(ui, "OpenStack not configured", reason);
        }
        Some(LaneOutcome::Failed(reason)) => {
            ui.colored_label(
                Style::DANGER,
                RichText::new(reason.as_str()).size(Style::SMALL),
            );
        }
        Some(LaneOutcome::Ready(rows)) if rows.is_empty() => {
            crate::session::empty_state(
                ui,
                "No instances yet",
                "Launch one with the full picker above \u{2014} it boots from a Glance image \
                 through the typed cloud verbs.",
            );
        }
        Some(LaneOutcome::Ready(rows)) => roster = Some(rows.clone()),
    }
    if let Some(rows) = roster {
        for row in &rows {
            render_instance_card(ui, state, row);
            ui.add_space(Style::SP_XS);
        }
    }
}

/// Convert the shared [`Elevation::Raised`](mde_egui::style::Elevation::Raised)
/// depth token into an [`egui::Shadow`] (the token module stays free of egui's
/// shadow type). Reads the token's offset/blur/spread/umbra, casting the
/// logical-px floats onto epaint's small integer fields; mints **no** colour of
/// its own (the umbra comes straight from the token), so a Nova roster card reads
/// as genuinely lifted off the page while the look still comes only from
/// `mde_egui` (§4).
fn card_shadow() -> egui::Shadow {
    let token = mde_egui::style::Elevation::Raised.shadow();
    egui::Shadow {
        offset: [token.offset[0] as i8, token.offset[1] as i8],
        blur: token.blur as u8,
        spread: token.spread as u8,
        color: token.umbra,
    }
}

/// One Nova roster card: status pip · name · facts · the lifecycle verbs
/// (start/stop direct; reboot/delete typed-armed).
fn render_instance_card(ui: &mut egui::Ui, state: &mut CloudPlaneState, row: &InstanceRow) {
    // A genuinely raised roster card: the same `Frame::group` `ui.group` builds
    // (identical fill/stroke/radius/margin — no layout change), lifted off the
    // page by the shared `Elevation::Raised` soft shadow.
    egui::Frame::group(ui.style())
        .shadow(card_shadow())
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let color = instance_pip(&row.status);
                ui.colored_label(color, RichText::new(DOT).size(Style::SMALL));
                ui.add_space(Style::SP_XS);
                ui.label(
                    RichText::new(&row.name)
                        .color(Style::TEXT)
                        .size(Style::BODY)
                        .strong(),
                );
                ui.add_space(Style::SP_S);
                ui.colored_label(color, RichText::new(row.status.as_str()).size(Style::SMALL));
            });
            let mut facts = Vec::new();
            if let Some(flavor) = &row.flavor {
                facts.push(flavor.clone());
            }
            if let Some(image) = &row.image {
                facts.push(image.clone());
            }
            if let Some(networks) = &row.networks {
                facts.push(networks.clone());
            }
            facts.push(format!("ssh key {MESH_KEYPAIR_NAME}"));
            facts.push(row.id.clone());
            mde_egui::muted_note(ui, facts.join(" \u{00B7} "));
            ui.horizontal(|ui| {
                if ui
                    .button(RichText::new("Console").size(Style::SMALL))
                    .clicked()
                {
                    let body = serde_json::json!({ "instance": row.id }).to_string();
                    state.issue_mutation(
                        INSTANCE_CONSOLE_VERB,
                        &body,
                        &format!("console for {}", row.name),
                    );
                }
                if ui
                    .button(RichText::new("Ensure SSH key").size(Style::SMALL))
                    .clicked()
                {
                    state.issue_mutation(
                        ENSURE_MESH_KEYPAIR_VERB,
                        "{}",
                        &format!("mesh SSH keypair {MESH_KEYPAIR_NAME}"),
                    );
                }
                if ui
                    .button(RichText::new("Start").size(Style::SMALL))
                    .clicked()
                {
                    let body = serde_json::json!({ "instance": row.id }).to_string();
                    state.issue_mutation(
                        "instance-start",
                        &body,
                        &format!("start of {}", row.name),
                    );
                }
                if ui
                    .button(RichText::new("Stop").size(Style::SMALL))
                    .clicked()
                {
                    let body = serde_json::json!({ "instance": row.id }).to_string();
                    state.issue_mutation("instance-stop", &body, &format!("stop of {}", row.name));
                }
                if ui
                    .button(RichText::new("Reboot").size(Style::SMALL))
                    .clicked()
                {
                    state.arming = Some(CloudArming {
                        action: ArmAction::Lifecycle {
                            verb: "instance-reboot",
                            instance_id: row.id.clone(),
                        },
                        target: row.name.clone(),
                        typed: String::new(),
                    });
                }
                if ui
                    .button(
                        RichText::new("Delete")
                            .size(Style::SMALL)
                            .color(Style::DANGER),
                    )
                    .clicked()
                {
                    state.arming = Some(CloudArming {
                        action: ArmAction::Lifecycle {
                            verb: "instance-delete",
                            instance_id: row.id.clone(),
                        },
                        target: row.name.clone(),
                        typed: String::new(),
                    });
                }
            });
        });
}

/// The Nova status pip colour.
fn instance_pip(status: &str) -> Color32 {
    if status.eq_ignore_ascii_case("ACTIVE") {
        Style::OK
    } else if status.eq_ignore_ascii_case("ERROR") {
        Style::DANGER
    } else if status.eq_ignore_ascii_case("BUILD") {
        Style::WARN
    } else {
        Style::TEXT_DIM
    }
}

/// The full launch picker (Q83): presets → fields → armed Launch + Save as
/// preset. Launch composes a HOT stack and arms the audited `heat-create`.
fn render_launch_picker(ui: &mut egui::Ui, state: &mut CloudPlaneState) {
    ui.group(|ui| {
        section_header(ui, "Launch an instance");
        render_preset_strip(ui, state);

        form_field(ui, "Name", &mut state.picker.name);
        let images = lane_names(state.resources.get(&ResKind::Images));
        pick_field(
            ui,
            "cloud-launch-image",
            "Image",
            &mut state.picker.image,
            images,
        );
        let flavors = lane_names(state.resources.get(&ResKind::Flavors));
        pick_field(
            ui,
            "cloud-launch-flavor",
            "Flavor",
            &mut state.picker.flavor,
            flavors,
        );
        let networks = lane_names(state.resources.get(&ResKind::Networks));
        pick_field(
            ui,
            "cloud-launch-network",
            "Network",
            &mut state.picker.network,
            networks,
        );
        form_field(
            ui,
            "Data volume (GiB, optional)",
            &mut state.picker.volume_gb,
        );
        mde_egui::muted_note(
            ui,
            "Launch composes a Heat stack (server + optional volume) and drives the audited \
             heat-create verb. The stack uses the mesh SSH keypair mcnf-mesh automatically.",
        );

        if let Some(err) = state.picker.error.as_deref() {
            ui.colored_label(Style::DANGER, RichText::new(err).size(Style::SMALL));
        }

        ui.horizontal(|ui| {
            if ui
                .button(RichText::new("Launch\u{2026}").size(Style::SMALL))
                .clicked()
            {
                match state.picker.compose() {
                    Ok((stack_name, template)) => {
                        state.picker.error = None;
                        state.arming = Some(CloudArming {
                            action: ArmAction::HeatCreate {
                                stack_name: stack_name.clone(),
                                template,
                            },
                            target: stack_name,
                            typed: String::new(),
                        });
                    }
                    Err(e) => state.picker.error = Some(e),
                }
            }
            if ui
                .button(RichText::new("Save as preset").size(Style::SMALL))
                .clicked()
            {
                let preset = state.picker.as_preset();
                match save_preset(&presets_dir(&state.workgroup_root), &preset) {
                    Ok(path) => {
                        state.note = Some(format!(
                            "Preset saved to the mesh share ({}).",
                            path.display()
                        ));
                        state.presets_loaded_at = None;
                    }
                    Err(e) => state.picker.error = Some(e),
                }
            }
        });
    });
}

/// The Q84 preset strip inside the picker: each fleet-state template record as
/// a one-click prefill, plus an honest error line per malformed record.
fn render_preset_strip(ui: &mut egui::Ui, state: &mut CloudPlaneState) {
    if state.presets.is_empty() {
        mde_egui::muted_note(
            ui,
            "No launch presets on the mesh share yet \u{2014} Save as preset below authors \
             one any node can use (cloud/templates/*.json).",
        );
    } else {
        ui.horizontal_wrapped(|ui| {
            mde_egui::muted_note(ui, "Presets:");
            let presets = state.presets.clone();
            for preset in &presets {
                let label = if preset.description.is_empty() {
                    preset.name.clone()
                } else {
                    format!("{} \u{2014} {}", preset.name, preset.description)
                };
                if ui.button(RichText::new(label).size(Style::SMALL)).clicked() {
                    state.picker.apply_preset(preset);
                }
            }
        });
    }
    for err in &state.preset_errors {
        ui.colored_label(Style::WARN, RichText::new(err.as_str()).size(Style::SMALL));
    }
    ui.add_space(Style::SP_XS);
}

/// The **Volumes** tab: the New-volume (stack) form + the live volumes and
/// snapshots listings.
fn render_volumes_tab(ui: &mut egui::Ui, state: &mut CloudPlaneState) {
    ui.horizontal(|ui| {
        section_header(ui, "Volumes");
        ui.add_space(Style::SP_S);
        let toggle = if state.volume_form.0 {
            "Close"
        } else {
            "\u{FF0B} New volume\u{2026}"
        };
        if ui
            .button(RichText::new(toggle).size(Style::SMALL))
            .clicked()
        {
            state.volume_form.0 = !state.volume_form.0;
        }
    });
    if state.volume_form.0 {
        ui.group(|ui| {
            form_field(ui, "Name", &mut state.volume_form.1);
            form_field(ui, "Size (GiB)", &mut state.volume_form.2);
            mde_egui::muted_note(
                ui,
                "Creates the volume as a managed Heat stack (armed heat-create).",
            );
            if ui
                .button(RichText::new("Create\u{2026}").size(Style::SMALL))
                .clicked()
            {
                let name = state.volume_form.1.trim().to_string();
                match state
                    .volume_form
                    .2
                    .trim()
                    .parse::<u32>()
                    .ok()
                    .filter(|g| *g > 0)
                {
                    Some(gb) if !name.is_empty() => {
                        state.arming = Some(CloudArming {
                            action: ArmAction::HeatCreate {
                                stack_name: name.clone(),
                                template: volume_hot(&name, gb),
                            },
                            target: name,
                            typed: String::new(),
                        });
                        state.volume_form.0 = false;
                    }
                    _ => {
                        state.note = Some("a volume needs a name and a whole-GiB size".to_string());
                    }
                }
            }
        });
        ui.add_space(Style::SP_S);
    }
    if let Some(table) = lane_absent(ui, state.resources.get(&ResKind::Volumes), "volumes") {
        render_table(ui, "cloud-volumes", table, None);
    }
    ui.add_space(Style::SP_M);
    section_header(ui, "Snapshots");
    if let Some(table) = lane_absent(ui, state.resources.get(&ResKind::Snapshots), "snapshots") {
        render_table(ui, "cloud-snapshots", table, None);
    }
    mde_egui::muted_note(
        ui,
        "A volume created here is torn down via its stack (Stacks \u{2192} Delete).",
    );
}

/// The **Images** tab: the Register-image (stack) form + the live listing.
fn render_images_tab(ui: &mut egui::Ui, state: &mut CloudPlaneState) {
    ui.horizontal(|ui| {
        section_header(ui, "Images");
        ui.add_space(Style::SP_S);
        let toggle = if state.image_form.0 {
            "Close"
        } else {
            "\u{FF0B} Register image\u{2026}"
        };
        if ui
            .button(RichText::new(toggle).size(Style::SMALL))
            .clicked()
        {
            state.image_form.0 = !state.image_form.0;
        }
    });
    if state.image_form.0 {
        ui.group(|ui| {
            form_field(ui, "Name", &mut state.image_form.1);
            form_field(ui, "Source URL", &mut state.image_form.2);
            mde_egui::muted_note(
                ui,
                "Registers the image into Glance as a managed Heat stack \
                 (OS::Glance::WebImage, armed heat-create).",
            );
            if ui
                .button(RichText::new("Register\u{2026}").size(Style::SMALL))
                .clicked()
            {
                let name = state.image_form.1.trim().to_string();
                let url = state.image_form.2.trim().to_string();
                if name.is_empty() || url.is_empty() {
                    state.note = Some("an image needs a name and a source URL".to_string());
                } else {
                    state.arming = Some(CloudArming {
                        action: ArmAction::HeatCreate {
                            stack_name: name.clone(),
                            template: image_hot(&name, &url),
                        },
                        target: name,
                        typed: String::new(),
                    });
                    state.image_form.0 = false;
                }
            }
        });
        ui.add_space(Style::SP_S);
    }
    if let Some(table) = lane_absent(ui, state.resources.get(&ResKind::Images), "images") {
        render_table(ui, "cloud-images", table, None);
    }
}

/// The **Networks** tab: the New-network (stack) form + the live listing.
fn render_networks_tab(ui: &mut egui::Ui, state: &mut CloudPlaneState) {
    ui.horizontal(|ui| {
        section_header(ui, "Networks");
        ui.add_space(Style::SP_S);
        let toggle = if state.network_form.0 {
            "Close"
        } else {
            "\u{FF0B} New network\u{2026}"
        };
        if ui
            .button(RichText::new(toggle).size(Style::SMALL))
            .clicked()
        {
            state.network_form.0 = !state.network_form.0;
        }
    });
    if state.network_form.0 {
        ui.group(|ui| {
            form_field(ui, "Name", &mut state.network_form.1);
            form_field(ui, "Subnet CIDR", &mut state.network_form.2);
            mde_egui::muted_note(
                ui,
                "Creates net + subnet as a managed Heat stack (armed heat-create). The mesh's \
                 flat provider network stays fleet-owned.",
            );
            if ui
                .button(RichText::new("Create\u{2026}").size(Style::SMALL))
                .clicked()
            {
                let name = state.network_form.1.trim().to_string();
                let cidr = state.network_form.2.trim().to_string();
                if name.is_empty() || cidr.is_empty() {
                    state.note = Some("a network needs a name and a subnet CIDR".to_string());
                } else {
                    state.arming = Some(CloudArming {
                        action: ArmAction::HeatCreate {
                            stack_name: name.clone(),
                            template: network_hot(&name, &cidr),
                        },
                        target: name,
                        typed: String::new(),
                    });
                    state.network_form.0 = false;
                }
            }
        });
        ui.add_space(Style::SP_S);
    }
    if let Some(table) = lane_absent(ui, state.resources.get(&ResKind::Networks), "networks") {
        render_table(ui, "cloud-networks", table, None);
    }
}

/// The **Stacks** tab: the live stack listing with a typed-armed Delete per
/// row — the management handle for everything launched from this plane.
fn render_stacks_tab(ui: &mut egui::Ui, state: &mut CloudPlaneState) {
    section_header(ui, "Stacks");
    mde_egui::muted_note(
        ui,
        "Every launch from this plane is a managed stack \u{2014} deleting a stack tears down \
         everything it created. Full Heat control (templates, preview, drift) lives in the \
         Infra-as-Code surface.",
    );
    ui.add_space(Style::SP_XS);
    let mut delete: Option<(String, String)> = None;
    if let Some(table) = lane_absent(ui, state.resources.get(&ResKind::Stacks), "stacks") {
        let mut action = |ui: &mut egui::Ui, table: &ResourceTable, idx: usize| {
            if let Some(row) = table.rows.get(idx) {
                if ui
                    .button(
                        RichText::new("Delete")
                            .size(Style::SMALL)
                            .color(Style::DANGER),
                    )
                    .clicked()
                {
                    delete = Some((row.id.clone(), table.row_label(row).to_string()));
                }
            }
        };
        render_table(ui, "cloud-stacks", table, Some(&mut action));
    }
    if let Some((stack_id, stack_name)) = delete {
        state.arming = Some(CloudArming {
            action: ArmAction::HeatDelete {
                stack_id,
                stack_name: stack_name.clone(),
            },
            target: stack_name,
            typed: String::new(),
        });
    }
}

/// The **Usage** tab: real counts off the live lanes + a per-user rollup
/// wherever a listing carried user attribution (Q82 — self-serve visibility;
/// the hard Q89 quota is enforced server-side by Keystone/Nova).
fn render_usage_tab(ui: &mut egui::Ui, state: &CloudPlaneState) {
    use std::fmt::Write as _;
    section_header(ui, "Usage");
    let tables: Vec<(&'static str, &ResourceTable)> = [
        (ResKind::Volumes, "volumes"),
        (ResKind::Snapshots, "snapshots"),
        (ResKind::Images, "images"),
        (ResKind::Networks, "networks"),
        (ResKind::Stacks, "stacks"),
    ]
    .iter()
    .filter_map(|(kind, label)| {
        state
            .resources
            .get(kind)
            .and_then(Lane::ready)
            .map(|t| (*label, t))
    })
    .collect();
    let usage = fold_usage(state.instances.ready().map(Vec::as_slice), &tables);

    match usage.instances {
        Some((total, active, errored)) => {
            let mut line = format!("{total} instances \u{00B7} {active} active");
            if errored > 0 {
                let _ = write!(line, " \u{00B7} {errored} in error");
            }
            ui.colored_label(
                if errored > 0 {
                    Style::WARN
                } else {
                    Style::TEXT
                },
                RichText::new(line).size(Style::SMALL),
            );
        }
        None => {
            mde_egui::muted_note(ui, "instance roster not answered yet");
        }
    }
    for (label, count, extra) in &usage.kinds {
        let mut line = format!("{count} {label}");
        if let Some(extra) = extra {
            let _ = write!(line, " \u{00B7} {extra} total");
        }
        ui.colored_label(Style::TEXT, RichText::new(line).size(Style::SMALL));
    }
    if usage.kinds.is_empty() {
        mde_egui::muted_note(ui, "resource listings not answered yet");
    }

    ui.add_space(Style::SP_M);
    section_header(ui, "By user");
    if usage.per_user.is_empty() {
        mde_egui::muted_note(
            ui,
            "The answered listings carried no user attribution \u{2014} per-user rows appear \
             where the cloud reports owners (volumes/images). The hard per-user quota is \
             enforced by Keystone/Nova (Q89).",
        );
    } else {
        egui::Grid::new("cloud-usage-users")
            .striped(true)
            .show(ui, |ui| {
                for (user, kinds) in &usage.per_user {
                    ui.label(
                        RichText::new(user.as_str())
                            .color(Style::TEXT)
                            .size(Style::SMALL),
                    );
                    let line = kinds
                        .iter()
                        .map(|(k, n)| format!("{n} {k}"))
                        .collect::<Vec<_>>()
                        .join(" \u{00B7} ");
                    ui.label(
                        RichText::new(line)
                            .color(Style::TEXT_DIM)
                            .size(Style::SMALL),
                    );
                    ui.end_row();
                }
            });
    }
}

// ─────────────────────────── tests ───────────────────────────

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use mackes_mesh_types::openstack::ResourceRow;
    use mde_egui::egui::{pos2, vec2, Rect};

    /// Phase-C depth adoption — each Nova roster card carries the shared
    /// [`Elevation::Raised`](mde_egui::style::Elevation) soft shadow verbatim from
    /// the token (no hand-rolled colour, design lock #2 / §4). The surface-side
    /// [`card_shadow`] conversion must reproduce the token's offset/blur/spread
    /// and its exact translucent umbra, and cast a real (non-zero) shadow so the
    /// instance reads as genuinely lifted off the page.
    #[test]
    fn instance_card_wears_the_raised_elevation_token() {
        let token = mde_egui::style::Elevation::Raised.shadow();
        let shadow = card_shadow();
        assert_eq!(
            shadow.color, token.umbra,
            "the roster card's umbra comes straight from the token — no minted colour"
        );
        assert_eq!(
            shadow.offset,
            [token.offset[0] as i8, token.offset[1] as i8]
        );
        assert_eq!(shadow.blur, token.blur as u8);
        assert_eq!(shadow.spread, token.spread as u8);
        assert!(
            shadow.color.a() > 0 && shadow.color.a() < 255 && shadow.blur > 0,
            "Raised casts a real, soft, translucent shadow — the card is lifted off the page"
        );
    }

    /// A throwaway dir under the system temp dir (this crate does not vendor
    /// `tempfile` — the device-manager `ScratchRoot` idiom), removed on drop.
    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let root = std::env::temp_dir().join(format!("cloud-plane-{tag}-{nanos}"));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A hermetic state: no Bus root, a nonexistent share root — every lane
    /// degrades honestly, nothing reads the build host's real Bus.
    fn test_state() -> CloudPlaneState {
        CloudPlaneState {
            bus_root: None,
            workgroup_root: PathBuf::from("/nonexistent-cloud-plane-test-root"),
            ..CloudPlaneState::default()
        }
    }

    fn table(
        service: &str,
        collection: &str,
        columns: &[&str],
        rows: &[(&str, &[&str])],
    ) -> ResourceTable {
        ResourceTable {
            service_type: service.to_string(),
            collection: collection.to_string(),
            columns: columns.iter().map(ToString::to_string).collect(),
            rows: rows
                .iter()
                .map(|(id, cells)| ResourceRow {
                    id: (*id).to_string(),
                    cells: cells.iter().map(ToString::to_string).collect(),
                })
                .collect(),
        }
    }

    fn catalog(types: &[&str]) -> ServiceCatalog {
        let services = types
            .iter()
            .map(|t| {
                format!(
                    r#"{{"type":"{t}","name":"{t}","endpoints":[{{"interface":"public","url":"http://x.mesh:1/","region":"R","id":"e"}}]}}"#
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let body = format!(
            r#"{{"token":{{"catalog":[{services}],"project":{{"id":"p","name":"mesh"}}}}}}"#
        );
        ServiceCatalog::from_keystone_token_json(&body).expect("fixture catalog parses")
    }

    // ── reply folds ──

    #[test]
    fn fold_status_reads_the_mirror_and_the_honest_tri_state() {
        let body = r#"{"ok":true,"verb":"get-status","status":{"host":"eagle",
            "doctrine":{"status":"enabled","leader":true,"kolla_release":"2024.1"},
            "runtime":{"status":"available"},
            "services":[{"service":"nova_api","status":{"state":"running"}},
                        {"service":"glance_api","status":{"state":"not_running","podman_state":"exited"}}],
            "extras":[],"published_at_ms":1}}"#;
        let reply: CloudReply = serde_json::from_str(body).unwrap();
        let outcome = fold_with(reply, |r| r.status, "missing");
        let LaneOutcome::Ready(mirror) = outcome else {
            panic!("status reply must fold Ready");
        };
        assert_eq!(mirror.host, "eagle");
        assert_eq!(
            mirror.doctrine,
            Doctrine::Enabled {
                leader: true,
                kolla_release: "2024.1".to_string()
            }
        );
        assert_eq!(mirror.service_tally(), (1, 2));

        // Gated → NotConfigured; error → Failed; ok-without-payload → Failed.
        let gated: CloudReply =
            serde_json::from_str(r#"{"ok":false,"verb":"get-status","gated":"no clouds.yaml"}"#)
                .unwrap();
        assert!(matches!(
            fold_with(gated, |r| r.status, "missing"),
            LaneOutcome::NotConfigured(r) if r == "no clouds.yaml"
        ));
        let failed: CloudReply =
            serde_json::from_str(r#"{"ok":false,"verb":"get-status","error":"boom"}"#).unwrap();
        assert!(matches!(
            fold_with(failed, |r| r.status, "missing"),
            LaneOutcome::Failed(r) if r == "boom"
        ));
        let empty: CloudReply = serde_json::from_str(r#"{"ok":true,"verb":"get-status"}"#).unwrap();
        assert!(matches!(
            fold_with(empty, |r| r.status, "missing"),
            LaneOutcome::Failed(r) if r == "missing"
        ));
    }

    #[test]
    fn unknown_doctrine_and_service_tags_fold_to_honest_unknown() {
        // A future worker variant must not break the plane — it reads unknown.
        let body = r#"{"ok":true,"verb":"get-status","status":{"host":"n",
            "doctrine":{"status":"half-enabled"},
            "runtime":{"status":"warming-up"},
            "services":[{"service":"x","status":{"state":"resting"}}]}}"#;
        let reply: CloudReply = serde_json::from_str(body).unwrap();
        let LaneOutcome::Ready(mirror) = fold_with(reply, |r| r.status, "missing") else {
            panic!("must fold Ready");
        };
        assert_eq!(mirror.doctrine, Doctrine::Unknown);
        assert_eq!(mirror.runtime, Runtime::Unknown);
        assert_eq!(mirror.services[0].status, ServiceState::Unrecognized);
        assert_eq!(mirror.service_tally(), (0, 1));
    }

    #[test]
    fn fold_instances_reads_the_roster() {
        let body = r#"{"ok":true,"verb":"list-instances","instances":[
            {"id":"i-1","name":"web1","status":"ACTIVE","flavor":"m1.small"},
            {"id":"i-2","name":"db1","status":"SHUTOFF"}]}"#;
        let reply: CloudReply = serde_json::from_str(body).unwrap();
        let LaneOutcome::Ready(rows) = fold_with(reply, |r| r.instances, "missing") else {
            panic!("roster must fold Ready");
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "web1");
        assert_eq!(rows[0].flavor.as_deref(), Some("m1.small"));
        assert_eq!(rows[1].image, None);
    }

    #[test]
    fn mutation_note_reads_the_honest_tri_state() {
        let ok: CloudReply = serde_json::from_str(
            r#"{"ok":true,"verb":"instance-start","instance":"web1","audited":true}"#,
        )
        .unwrap();
        assert_eq!(mutation_note(&ok), "Completed on web1.");
        let stack: CloudReply = serde_json::from_str(
            r#"{"ok":true,"verb":"heat-create","stack":"s-9","audited":true}"#,
        )
        .unwrap();
        assert_eq!(mutation_note(&stack), "Completed on s-9.");
        let gated: CloudReply =
            serde_json::from_str(r#"{"ok":false,"verb":"heat-delete","gated":"nova down"}"#)
                .unwrap();
        assert_eq!(mutation_note(&gated), "Gated: nova down");
        let failed: CloudReply =
            serde_json::from_str(r#"{"ok":false,"verb":"instance-delete","error":"denied"}"#)
                .unwrap();
        assert_eq!(mutation_note(&failed), "Failed: denied");
        let keypair: CloudReply = serde_json::from_str(
            r#"{"ok":true,"verb":"ensure-mesh-keypair","keypair":"mcnf-mesh","audited":true}"#,
        )
        .unwrap();
        assert_eq!(
            mutation_note(&keypair),
            "Mesh SSH keypair mcnf-mesh is present in Nova."
        );
        let console: CloudReply =
            serde_json::from_str(r#"{"ok":true,"verb":"get-instance-console","console":{"instance":"i-1","protocol":"spice-html5","url":"spice://i-1.mesh:5900"},"audited":false}"#)
                .unwrap();
        assert_eq!(
            mutation_note(&console),
            "Console ready for i-1: spice-html5 spice://i-1.mesh:5900."
        );
    }

    #[test]
    fn console_descriptor_builds_a_native_spice_attach_request() {
        let console = ConsoleInfo {
            instance: "i-1".to_string(),
            protocol: "spice-html5".to_string(),
            url: "spice://i-1.mesh:5930".to_string(),
        };
        let request = console_attach_request(&console).expect("native SPICE URL is dialable");
        assert_eq!(request.target.serving_peer, "openstack");
        assert_eq!(request.target.name, "i-1");
        assert_eq!(request.protocol, VdiProtocol::Spice);
        let endpoint = request.target.endpoint.as_ref().expect("endpoint set");
        assert_eq!(endpoint.host, "i-1.mesh");
        assert_eq!(endpoint.port, 5930);
        assert!(matches!(request.auth, DesktopAuth::MeshIdentity { .. }));
    }

    #[test]
    fn console_descriptor_gates_html5_proxy_urls() {
        let console = ConsoleInfo {
            instance: "i-1".to_string(),
            protocol: "spice-html5".to_string(),
            url: "https://nova.mesh/spice_auto.html?token=abc".to_string(),
        };
        let err =
            console_attach_request(&console).expect_err("HTML5 proxy URL is not native SPICE");
        assert!(err.contains("direct spice://host:port"));
    }

    #[test]
    fn console_reply_queues_native_attach_once() {
        let reply: CloudReply =
            serde_json::from_str(r#"{"ok":true,"verb":"get-instance-console","console":{"instance":"i-1","protocol":"spice-html5","url":"spice://i-1.mesh:5900"},"audited":false}"#)
                .unwrap();
        let mut state = CloudPlaneState::default();
        let note = state.apply_mutation_reply(&reply);
        assert!(note.contains("opening native SPICE Desktop attach"));
        let request = state
            .console_attach
            .take()
            .expect("native console queued a VDI request");
        assert_eq!(request.target.name, "i-1");
        assert!(
            state.console_attach.is_none(),
            "the attach request is one-shot once drained"
        );
    }

    // ── the kinds resolve against the live catalog ──

    #[test]
    fn kinds_resolve_their_cataloged_service_types() {
        let cat = catalog(&["compute", "volumev3", "image", "network", "orchestration"]);
        assert_eq!(
            ResKind::Volumes.service_type(&cat).as_deref(),
            Some("volumev3")
        );
        assert_eq!(
            ResKind::Snapshots.service_type(&cat).as_deref(),
            Some("volumev3")
        );
        assert_eq!(ResKind::Images.service_type(&cat).as_deref(), Some("image"));
        assert_eq!(
            ResKind::Networks.service_type(&cat).as_deref(),
            Some("network")
        );
        assert_eq!(
            ResKind::Stacks.service_type(&cat).as_deref(),
            Some("orchestration")
        );
        assert_eq!(
            ResKind::Flavors.service_type(&cat).as_deref(),
            Some("compute")
        );
        // An unadvertised kind resolves to None — rendered honestly, never guessed.
        let bare = catalog(&["identity"]);
        assert_eq!(ResKind::Volumes.service_type(&bare), None);
        assert_eq!(ResKind::Stacks.service_type(&bare), None);
    }

    #[test]
    fn every_q85_kind_is_a_tab_and_labels_are_distinct() {
        // instances · volumes(+snapshots) · images · networks · stacks (+ usage).
        let mut labels: Vec<&str> = CloudTab::ALL.iter().map(|t| t.label()).collect();
        assert_eq!(labels.len(), 6);
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), 6, "tab labels must be distinct");
        assert!(CloudTab::Volumes.kinds().contains(&ResKind::Snapshots));
        assert!(CloudTab::Stacks.kinds().contains(&ResKind::Stacks));
    }

    // ── HOT composition (stacks-as-code) ──

    #[test]
    fn launch_hot_composes_the_full_picker_stack() {
        let hot = launch_hot("web1", "fedora-42", "m1.small", Some("mesh-flat"), Some(20));
        assert!(hot.starts_with(&format!("heat_template_version: {HOT_TEMPLATE_VERSION}")));
        assert!(hot.contains("type: OS::Nova::Server"));
        assert!(hot.contains("name: web1"));
        assert!(hot.contains("image: fedora-42"));
        assert!(hot.contains("flavor: m1.small"));
        assert!(hot.contains("key_name: mcnf-mesh"));
        assert!(hot.contains("mcnf:ssh-key-source: mesh-ssh-key"));
        assert!(hot.contains("- network: mesh-flat"));
        assert!(hot.contains("type: OS::Cinder::Volume"));
        assert!(hot.contains("size: 20"));
        assert!(hot.contains("type: OS::Cinder::VolumeAttachment"));
        assert!(hot.contains("instance_uuid: { get_resource: server }"));

        // Minimal form: no network property, no volume resources.
        let bare = launch_hot("web1", "fedora-42", "m1.small", None, None);
        assert!(!bare.contains("networks:"));
        assert!(!bare.contains("OS::Cinder::Volume"));
    }

    #[test]
    fn kind_hot_composers_emit_their_resources() {
        let vol = volume_hot("data1", 50);
        assert!(vol.contains("type: OS::Cinder::Volume"));
        assert!(vol.contains("name: data1"));
        assert!(vol.contains("size: 50"));

        let net = network_hot("lab", "10.9.0.0/24");
        assert!(net.contains("type: OS::Neutron::Net"));
        assert!(net.contains("type: OS::Neutron::Subnet"));
        assert!(net.contains("network: { get_resource: net }"));
        assert!(net.contains("cidr: 10.9.0.0/24"));

        let img = image_hot("fedora", "https://x/f.qcow2");
        assert!(img.contains("type: OS::Glance::WebImage"));
        // A URL carries a colon, so the YAML scalar is quoted.
        assert!(img.contains(r#"location: "https://x/f.qcow2""#));
        assert!(img.contains("disk_format: qcow2"));
    }

    #[test]
    fn picker_compose_validates_and_builds_the_stack() {
        let mut picker = LaunchPicker {
            open: true,
            name: "web1".to_string(),
            image: "fedora-42".to_string(),
            flavor: "m1.small".to_string(),
            network: String::new(),
            volume_gb: String::new(),
            error: None,
        };
        let (stack, hot) = picker.compose().expect("a full picker composes");
        assert_eq!(stack, "web1");
        assert!(hot.contains("image: fedora-42"));

        picker.volume_gb = "twenty".to_string();
        assert!(picker.compose().is_err(), "a non-numeric size is rejected");
        picker.volume_gb = "0".to_string();
        assert!(picker.compose().is_err(), "a zero size is rejected");
        picker.volume_gb.clear();
        picker.image.clear();
        assert!(picker.compose().is_err(), "a missing image is rejected");
        picker.image = "fedora-42".to_string();
        picker.name = "  ".to_string();
        assert!(picker.compose().is_err(), "a blank name is rejected");
    }

    // ── the typed-arming gate ──

    #[test]
    fn arming_gate_requires_the_exact_echo() {
        assert!(armed("web1", "web1"));
        assert!(armed("  web1  ", "web1"), "surrounding whitespace trims");
        assert!(!armed("web", "web1"));
        assert!(!armed("", "web1"));
        assert!(!armed("WEB1", "web1"), "the echo is case-exact");
    }

    #[test]
    fn confirmed_arming_publishes_and_unconfirmed_blocks() {
        // With no Bus the publish degrades to the honest note — the seam is
        // still driven only past the gate.
        let mut state = test_state();
        state.perform_armed(
            ArmAction::Lifecycle {
                verb: "instance-delete",
                instance_id: "i-1".to_string(),
            },
            "web1",
        );
        let note = state.note.clone().expect("an armed op always notes");
        assert!(note.contains("Bus is unavailable"), "{note}");
        assert!(state.mutation_pending.is_none());
    }

    // ── presets (Q84) ──

    #[test]
    fn preset_records_round_trip_and_malformed_ones_are_named() {
        let dir = ScratchDir::new("presets");
        let preset = LaunchPreset {
            name: "Dev Box".to_string(),
            description: "small dev VM".to_string(),
            image: "fedora-42".to_string(),
            flavor: "m1.small".to_string(),
            network: "mesh-flat".to_string(),
            volume_gb: Some(20),
        };
        let path = save_preset(dir.path(), &preset).expect("a valid preset saves");
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("dev-box.json"),
            "the record name is the slug of the preset name"
        );
        std::fs::write(dir.path().join("broken.json"), "{not json").unwrap();
        std::fs::write(dir.path().join("ignored.txt"), "not a record").unwrap();

        let (presets, errors) = read_presets(dir.path());
        assert_eq!(presets, vec![preset]);
        assert_eq!(
            errors.len(),
            1,
            "the malformed record is named, not dropped"
        );
        assert!(errors[0].contains("broken.json"), "{}", errors[0]);
    }

    #[test]
    fn absent_preset_dir_reads_a_clean_empty_set() {
        let (presets, errors) = read_presets(Path::new("/nonexistent-cloud-plane-presets"));
        assert!(presets.is_empty());
        assert!(errors.is_empty());
    }

    #[test]
    fn save_preset_rejects_an_incomplete_record() {
        let dir = ScratchDir::new("reject");
        let incomplete = LaunchPreset {
            name: "x".to_string(),
            ..LaunchPreset::default()
        };
        assert!(save_preset(dir.path(), &incomplete).is_err());
    }

    #[test]
    fn preset_fills_the_picker() {
        let mut picker = LaunchPicker::default();
        picker.apply_preset(&LaunchPreset {
            name: "Dev Box".to_string(),
            description: String::new(),
            image: "fedora-42".to_string(),
            flavor: "m1.small".to_string(),
            network: "mesh-flat".to_string(),
            volume_gb: Some(20),
        });
        assert!(picker.open);
        assert_eq!(picker.name, "Dev Box");
        assert_eq!(picker.image, "fedora-42");
        assert_eq!(picker.volume_gb, "20");
        let (stack, hot) = picker.compose().expect("a preset-filled picker composes");
        assert_eq!(stack, "Dev Box");
        assert!(hot.contains("flavor: m1.small"));
    }

    // ── the usage fold (Q82) ──

    #[test]
    fn usage_folds_counts_sizes_and_per_user_attribution() {
        let instances = vec![
            InstanceRow {
                id: "i-1".to_string(),
                name: "web1".to_string(),
                status: "ACTIVE".to_string(),
                ..InstanceRow::default()
            },
            InstanceRow {
                id: "i-2".to_string(),
                name: "db1".to_string(),
                status: "ERROR".to_string(),
                ..InstanceRow::default()
            },
        ];
        let volumes = table(
            "volumev3",
            "volumes/detail",
            &["name", "status", "size", "user_id"],
            &[
                ("v-1", &["data1", "in-use", "20", "alice"]),
                ("v-2", &["data2", "available", "30", "bob"]),
                ("v-3", &["data3", "available", "10", "alice"]),
            ],
        );
        let images = table(
            "image",
            "v2/images",
            &["name", "status", "owner"],
            &[("im-1", &["fedora", "active", "alice"])],
        );
        let usage = fold_usage(
            Some(&instances),
            &[("volumes", &volumes), ("images", &images)],
        );
        assert_eq!(usage.instances, Some((2, 1, 1)));
        assert_eq!(usage.kinds[0], ("volumes", 3, Some("60 GiB".to_string())));
        assert_eq!(usage.kinds[1], ("images", 1, None));
        assert_eq!(usage.per_user["alice"]["volumes"], 2);
        assert_eq!(usage.per_user["alice"]["images"], 1);
        assert_eq!(usage.per_user["bob"]["volumes"], 1);
    }

    #[test]
    fn usage_without_attribution_is_honestly_empty() {
        let networks = table("network", "v2.0/networks", &["name"], &[("n-1", &["flat"])]);
        let usage = fold_usage(None, &[("networks", &networks)]);
        assert!(usage.per_user.is_empty());
        assert_eq!(usage.instances, None);
        assert_eq!(usage.kinds, vec![("networks", 1, None)]);
    }

    // ── MENU-1 verbs ──

    #[test]
    fn menu_verbs_drive_the_same_seams_the_body_toggles() {
        let mut state = test_state();
        state.apply_menu_verb(CloudMenuVerb::LaunchInstance);
        assert_eq!(state.tab, CloudTab::Instances);
        assert!(state.picker.open);
        state.apply_menu_verb(CloudMenuVerb::NewVolume);
        assert_eq!(state.tab, CloudTab::Volumes);
        assert!(state.volume_form.0);
        state.apply_menu_verb(CloudMenuVerb::RegisterImage);
        assert_eq!(state.tab, CloudTab::Images);
        assert!(state.image_form.0);
        state.apply_menu_verb(CloudMenuVerb::NewNetwork);
        assert_eq!(state.tab, CloudTab::Networks);
        assert!(state.network_form.0);
        // Refresh queues every lane + a preset re-read.
        state.resources.entry(ResKind::Stacks).or_default();
        state.presets_loaded_at = Some(Instant::now());
        state.apply_menu_verb(CloudMenuVerb::Refresh);
        assert!(state.status.forced);
        assert!(state.catalog.forced);
        assert!(state.instances.forced);
        assert!(state.resources[&ResKind::Stacks].forced);
        assert!(state.presets_loaded_at.is_none());
    }

    // ── lanes ──

    #[test]
    fn lane_cadence_first_fetch_then_refresh() {
        let now = Instant::now();
        let mut lane: Lane<ResourceTable> = Lane::default();
        assert!(lane.due(now), "the first fetch is always due");
        lane.settled_at = Some(now);
        assert!(!lane.due(now), "a fresh settle waits the cadence");
        lane.forced = true;
        assert!(lane.due(now), "a queued refresh overrides the cadence");
        lane.forced = false;
        lane.pending = Some(Pending {
            ulid: "u".to_string(),
            sent: now,
        });
        assert!(!lane.due(now), "an in-flight request blocks a reissue");
    }

    #[test]
    fn no_bus_degrades_every_lane_honestly() {
        let mut state = test_state();
        let ctx = egui::Context::default();
        state.poll(&ctx);
        assert!(matches!(
            state.status.outcome,
            Some(LaneOutcome::Failed(ref r)) if r.contains("Bus is unavailable")
        ));
        assert!(matches!(
            state.catalog.outcome,
            Some(LaneOutcome::Failed(ref r)) if r.contains("Bus is unavailable")
        ));
        assert!(matches!(
            state.instances.outcome,
            Some(LaneOutcome::Failed(ref r)) if r.contains("Bus is unavailable")
        ));
    }

    // ── the render (headless CPU tessellation, the shell test idiom) ──

    fn run_plane(state: &mut CloudPlaneState) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 760.0))),
            ..Default::default()
        };
        let controller = crate::controller::ControllerState::default();
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, state, &controller));
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        !prims.is_empty()
    }

    #[test]
    fn plane_renders_headless_in_the_degraded_state() {
        let mut state = test_state();
        assert!(run_plane(&mut state), "the degraded plane must still draw");
    }

    #[test]
    fn plane_renders_every_tab_with_live_lanes_seeded() {
        let mut state = test_state();
        state.status.outcome = Some(LaneOutcome::Ready(MirrorStatus {
            host: "eagle".to_string(),
            doctrine: Doctrine::Enabled {
                leader: true,
                kolla_release: "2024.1".to_string(),
            },
            runtime: Runtime::Available,
            services: vec![MirrorServiceRow {
                service: "nova_api".to_string(),
                status: ServiceState::Running,
            }],
        }));
        state.catalog.outcome = Some(LaneOutcome::Ready(catalog(&[
            "compute",
            "volumev3",
            "image",
            "network",
            "orchestration",
        ])));
        state.instances.outcome = Some(LaneOutcome::Ready(vec![InstanceRow {
            id: "i-1".to_string(),
            name: "web1".to_string(),
            status: "ACTIVE".to_string(),
            flavor: Some("m1.small".to_string()),
            image: Some("fedora-42".to_string()),
            networks: Some("mesh-flat=10.9.0.5".to_string()),
        }]));
        for (kind, service) in [
            (ResKind::Volumes, "volumev3"),
            (ResKind::Snapshots, "volumev3"),
            (ResKind::Images, "image"),
            (ResKind::Networks, "network"),
            (ResKind::Stacks, "orchestration"),
            (ResKind::Flavors, "compute"),
        ] {
            let lane = Lane {
                outcome: Some(LaneOutcome::Ready(table(
                    service,
                    kind.collection(),
                    &["name", "status"],
                    &[("r-1", &["one", "ok"])],
                ))),
                ..Lane::default()
            };
            state.resources.insert(kind, lane);
        }
        state.picker.open = true;
        // Pin the preset reload so the poll inside `show` doesn't clobber the
        // seeded presets with a (nonexistent-dir) re-read mid-test.
        state.presets_loaded_at = Some(Instant::now());
        state.presets = vec![LaunchPreset {
            name: "Dev Box".to_string(),
            description: "small".to_string(),
            image: "fedora-42".to_string(),
            flavor: "m1.small".to_string(),
            network: String::new(),
            volume_gb: None,
        }];
        state.arming = Some(CloudArming {
            action: ArmAction::HeatDelete {
                stack_id: "s-1".to_string(),
                stack_name: "web1".to_string(),
            },
            target: "web1".to_string(),
            typed: String::new(),
        });
        state.note = Some("Requested delete of stack web1\u{2026}".to_string());

        for tab in CloudTab::ALL {
            state.tab = tab;
            assert!(run_plane(&mut state), "{tab:?} produced no draw primitives");
        }
    }
}
