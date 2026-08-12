//! Fleet · Datacenter — live per-node KVM reality (MV-6).
//!
//! Wires the Workbench Fleet plane to the authoritative Workload projection
//! published by `mackesd`, and routes VM power intent through the typed
//! Workload API:
//!
//! * `event/kvm/services` (MV-2 `kvm_health`) — each node's KVM service-health
//!   summary (libvirtd / podman / … up or down).
//! * `state/workloads/<node>` (WL-ARCH-010) — each node's typed VM/container
//!   roster, power, readiness, health, pressure, progress, and failure state.
//! * `action/workload/operation` — start / stop intent, **host-targeted** so a
//!   request can only ever act on the one node it names.
//!
//! The Workload snapshot is a versioned JSON boundary shared by the mesh-types
//! crate. The shell remains a client: it reads the retained projection and
//! never probes libvirt, Podman, or a legacy inventory topic.
//!
//! The container roster is rendered **read-only** (name / image / state) beside the
//! VM roster: it completes MV-6's "VMs *and* containers" surface. Container
//! run/stop lifecycle-drive is a
//! deliberate follow-up — the container worker has no "start an existing container"
//! verb to mirror the VM Start/Stop toggle, so the read-only roster lands cleanly
//! first rather than half-wiring a drive.
//!
//! `project` is pure (no Bus, no GPU) and unit-tested directly; the only IO is
//! `poll` (a cheap local `Persist` read) and the typed Workload publisher (a
//! `Persist` write recorded locally and replicated to the target node by the
//! Bus).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mde_egui::egui::{self, Color32, RichText};
use mde_egui::Style;
use serde::Deserialize;

use mde_bus::persist::Persist;

use mackes_mesh_types::workloads::{
    WorkloadBackend, WorkloadOperationAction, WorkloadOperationStatus, WorkloadPowerState,
    WorkloadProfile, WorkloadStateSnapshot, WORKLOAD_OPERATION_TOPIC,
};

use crate::bus_reader::BusReader;

/// Per-node KVM service-health summary topic (MV-2 `kvm_health`).
const SERVICES_TOPIC: &str = "event/kvm/services";
/// Per-node authoritative VM/container Workload projection.
const WORKLOAD_STATE_PREFIX: &str = "state/workloads/";
/// BOOKMARKS-7 — the per-node ad-block stats topic prefix (`state/adfilter/<node>`,
/// published by the `adfilter` worker). The Fleet view folds these into per-host
/// ad-block rows (enabled lists / rules / allowlist / staleness).
const ADFILTER_STATE_PREFIX: &str = "state/adfilter/";
/// BOOKMARKS-8 — the per-node browser-policy topic prefix
/// (`state/browser-policy/<node>`, published by the `browser_policy` worker). The
/// Fleet view folds these into the per-host browser-governance row (enabled /
/// forced ad-blocker / allowlist size / policy source + enforcement counters).
const BROWSER_POLICY_STATE_PREFIX: &str = "state/browser-policy/";
const DATACENTER_TOOLTIP_MAX_W: f32 = Style::SP_XL * 12.0;

/// Poll cadence for the health and Workload projections — a node's health flip
/// or a new VM/container
/// surfaces within this window. Matches the panel shell's 5 s refresh; the read
/// is a cheap local `SQLite` scan so the cadence can stay tight.
const REFRESH: Duration = Duration::from_secs(5);

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

// ───────────────────────── JSON boundary (read side) ─────────────────────────
// Local mirrors of the `mackesd` worker payloads. serde ignores any wire fields
// we don't render, so these carry only what the Fleet view uses.

/// One KVM service's liveness — mirrors `kvm_health::ServiceHealth`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct ServiceHealth {
    /// Canonical service id (e.g. `libvirtd`, `podman`) — the row label.
    id: String,
    /// The systemd unit probed (e.g. `libvirtd.service`) — shown as a row tooltip.
    unit: String,
    /// `true` when `systemctl is-active` reported the unit active.
    active: bool,
}

/// Whole-host KVM stack health — mirrors `kvm_health::KvmHealth`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct KvmHealth {
    /// Publishing node id.
    host: String,
    /// Per-service liveness, in catalog order.
    services: Vec<ServiceHealth>,
    /// Count of active services.
    active: usize,
    /// Total services in the probed catalog.
    total: usize,
    /// `true` iff every catalog service is active.
    all_healthy: bool,
    /// Publish time (ms since the Unix epoch) — the latest-wins fold key.
    published_at_ms: u64,
}

/// One VM row derived from an authoritative Workload status.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Instance {
    /// Domain/workload name — the stable lifecycle key.
    name: String,
    /// Display state derived from the typed Workload power dimension.
    state: String,
}

/// One container row derived from an authoritative Workload status.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Container {
    /// Container/workload name — the stable lifecycle key.
    name: String,
    /// Approved catalog image reference, if one was carried into the status.
    image: String,
    /// Display state derived from the typed Workload power dimension.
    state: String,
}

// ─────────────── JSON boundary: browser + ad-block fleet state ───────────────
// BOOKMARKS-8 — mirrors of the `adfilter` (BOOKMARKS-7) + `browser_policy` worker
// status payloads. serde ignores the wire fields we don't render.

/// How fresh the filter lists are — mirrors `mde_adblock::Staleness` (externally
/// tagged: `"Fresh"` / `"NeverSynced"` / `{"Stale":{"age_ms":N}}`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
enum Staleness {
    /// Synced from upstream within the freshness window.
    Fresh,
    /// Last upstream sync is older than the window.
    Stale {
        /// How long since the last successful sync (ms).
        age_ms: u64,
    },
    /// Never synced — running on the bundled seed.
    NeverSynced,
}

/// Per-node ad-block stats — mirrors `adfilter::AdfilterStatus` (BOOKMARKS-7).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct AdblockStat {
    /// Publishing node id.
    node: String,
    /// Enabled filter sources (the engine compiles these).
    enabled_sources: usize,
    /// Total filter sources.
    total_sources: usize,
    /// Network block+allow rules the compiled engine holds.
    network_rules: usize,
    /// Cosmetic hide+unhide rules the compiled engine holds.
    cosmetic_rules: usize,
    /// Sites currently allowlisted (blocking off) mesh-wide.
    allowlisted_sites: usize,
    /// How fresh the lists are (the honest staleness indicator).
    staleness: Staleness,
    /// Wall-clock ms of the last flush (latest-wins fold key).
    last_flush_ms: u64,
}

/// One custom filter list — mirrors `browser_policy::CustomFilterList`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct CustomFilterList {
    /// The list's stable name.
    name: String,
}

/// Per-node browser-governance state — mirrors `browser_policy::BrowserPolicyStatus`
/// (BOOKMARKS-8).
// A read-side status mirror is legitimately bool-heavy (enabled/hidden/forced/…);
// each bool is an independent honest flag the Fleet view renders.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct BrowserPolicyStat {
    /// Publishing node id.
    node: String,
    /// The role the policy was folded for.
    role: String,
    /// Whether the browser is enabled on this node's role.
    browser_enabled: bool,
    /// Whether the surface is hidden (== the browser being disabled).
    surface_hidden: bool,
    /// Whether the ad-blocker is forced on.
    force_adblock: bool,
    /// The enforced URL navigation allowlist (empty = unrestricted).
    url_allowlist: Vec<String>,
    /// The custom filter lists injected on launch.
    custom_filter_lists: Vec<CustomFilterList>,
    /// The node that authored the converged policy (empty = the default baseline).
    policy_source: String,
    /// How many launches this node has refused (disallowed role).
    launches_refused: u64,
    /// How many navigations this node has rejected (out of allowlist).
    navigations_rejected: u64,
    /// How many ad-block toggle-offs this node has rejected (force-on).
    adblock_toggles_rejected: u64,
    /// Whether the node-local browser data survives a disable (never wiped).
    local_data_retained: bool,
    /// Wall-clock ms of the last flush (latest-wins fold key).
    last_flush_ms: u64,
}

/// One node's browser + ad-block fleet reality, folded from the latest
/// `state/adfilter/*` + `state/browser-policy/*` messages seen for that host.
#[derive(Debug, Clone)]
struct BrowserFleetRow {
    /// Node id.
    host: String,
    /// Latest ad-block stats, once any has arrived.
    adblock: Option<AdblockStat>,
    /// Latest browser-policy state, once any has arrived.
    policy: Option<BrowserPolicyStat>,
}

/// Fold `state/adfilter/*` + `state/browser-policy/*` bodies into a
/// sorted-by-host per-node view.
/// Latest message wins per host (each stream tracked by its own publish time).
/// Pure — no Bus, no GPU.
fn project_browser(adfilter_bodies: &[String], policy_bodies: &[String]) -> Vec<BrowserFleetRow> {
    let mut rows: BTreeMap<String, BrowserFleetRow> = BTreeMap::new();

    for body in adfilter_bodies {
        let Ok(s) = serde_json::from_str::<AdblockStat>(body) else {
            continue;
        };
        let entry = rows
            .entry(s.node.clone())
            .or_insert_with(|| BrowserFleetRow {
                host: s.node.clone(),
                adblock: None,
                policy: None,
            });
        if entry
            .adblock
            .as_ref()
            .is_none_or(|cur| s.last_flush_ms >= cur.last_flush_ms)
        {
            entry.adblock = Some(s);
        }
    }

    for body in policy_bodies {
        let Ok(s) = serde_json::from_str::<BrowserPolicyStat>(body) else {
            continue;
        };
        let entry = rows
            .entry(s.node.clone())
            .or_insert_with(|| BrowserFleetRow {
                host: s.node.clone(),
                adblock: None,
                policy: None,
            });
        if entry
            .policy
            .as_ref()
            .is_none_or(|cur| s.last_flush_ms >= cur.last_flush_ms)
        {
            entry.policy = Some(s);
        }
    }

    rows.into_values().collect()
}

/// A compact staleness label + tone for the ad-block row.
fn staleness_label(s: &Staleness) -> (Color32, String) {
    match s {
        Staleness::Fresh => (Style::OK, "lists fresh".to_string()),
        Staleness::NeverSynced => (Style::TEXT_DIM, "bundled seed (never synced)".to_string()),
        Staleness::Stale { age_ms } => {
            let days = age_ms / (24 * 60 * 60 * 1000);
            (Style::WARN, format!("lists stale ({days}d old)"))
        }
    }
}

// ──────────────────────────── projected view ────────────────────────────

/// One node's live datacenter reality, folded from the latest health and
/// authoritative Workload snapshot seen for that host.
#[derive(Debug, Clone)]
struct NodeView {
    /// Node id (the Bus `host`).
    host: String,
    /// Latest KVM health summary, once any has arrived.
    health: Option<KvmHealth>,
    /// Latest VM rows (also empty for a genuinely empty Workload snapshot — see
    /// `roster_seen`).
    instances: Vec<Instance>,
    /// `true` once a Workload snapshot has been seen for this host — distinguishes
    /// "no VMs defined" from "Workload state not yet reported".
    roster_seen: bool,
    /// Publish time of the roster currently held (latest-wins fold key).
    roster_at_ms: u64,
    /// Latest container rows (also empty for a genuinely empty Workload snapshot
    /// — see `containers_seen`).
    containers: Vec<Container>,
    /// `true` once a Workload snapshot has been seen for this host — distinguishes
    /// "no containers" from "Workload state not yet reported".
    containers_seen: bool,
    /// Publish time of the container roster currently held (latest-wins fold key).
    containers_at_ms: u64,
}

impl NodeView {
    fn new(host: &str) -> Self {
        Self {
            host: host.to_string(),
            health: None,
            instances: Vec::new(),
            roster_seen: false,
            roster_at_ms: 0,
            containers: Vec::new(),
            containers_seen: false,
            containers_at_ms: 0,
        }
    }
}

/// The Datacenter roster kind Front Door may expose as a service lifecycle row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrontDoorLifecycleKind {
    /// A Quadlet/systemd container reported by the Workload projection.
    Container,
    /// A libvirt/KVM guest reported by the Workload projection.
    Vm,
}

impl FrontDoorLifecycleKind {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Container => "container",
            Self::Vm => "vm",
        }
    }
}

/// One cached Datacenter roster row that can safely become a Front Door service
/// result. This is intentionally a plain data transfer shape so the launcher can
/// use cached Fleet facts without polling the Bus or borrowing private node rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrontDoorLifecycleCandidate {
    pub(crate) host: String,
    pub(crate) kind: FrontDoorLifecycleKind,
    pub(crate) name: String,
    pub(crate) state: String,
    pub(crate) detail: String,
}

fn workload_name(snapshot_node: &str, status: &WorkloadOperationStatus) -> Option<String> {
    let prefix = match status.backend {
        WorkloadBackend::LibvirtVirtqemud => "vm:",
        WorkloadBackend::QuadletSystemd => "container:",
    };
    let expected = format!("{prefix}{snapshot_node}:");
    status
        .workload_id
        .as_str()
        .strip_prefix(&expected)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

fn power_label(power: WorkloadPowerState) -> &'static str {
    match power {
        WorkloadPowerState::Defined => "defined",
        WorkloadPowerState::Starting => "starting",
        WorkloadPowerState::Running => "running",
        WorkloadPowerState::Paused => "paused",
        WorkloadPowerState::Stopping => "stopping",
        WorkloadPowerState::Stopped => "shut off",
        WorkloadPowerState::Failed => "failed",
    }
}

fn workload_rows(snapshot: &WorkloadStateSnapshot) -> (Vec<Instance>, Vec<Container>) {
    let mut instances = Vec::new();
    let mut containers = Vec::new();
    for status in &snapshot.workloads {
        let Some(name) = workload_name(&snapshot.node, status) else {
            continue;
        };
        let state = power_label(status.power).to_owned();
        match status.backend {
            WorkloadBackend::LibvirtVirtqemud => instances.push(Instance { name, state }),
            WorkloadBackend::QuadletSystemd => containers.push(Container {
                name,
                image: status
                    .image_ref
                    .clone()
                    .unwrap_or_else(|| "managed by Workloads".to_owned()),
                state,
            }),
        }
    }
    instances.sort_by(|left, right| left.name.cmp(&right.name));
    containers.sort_by(|left, right| left.name.cmp(&right.name));
    (instances, containers)
}

/// Fold raw topic bodies into a sorted-by-host per-node view. Latest message wins
/// per host (health and Workload snapshot each tracked by their own timestamp),
/// so a growing topic collapses to one row per node. Pure — no Bus, no GPU.
fn project(health_bodies: &[String], workload_bodies: &[String]) -> Vec<NodeView> {
    let mut nodes: BTreeMap<String, NodeView> = BTreeMap::new();

    for body in health_bodies {
        let Ok(h) = serde_json::from_str::<KvmHealth>(body) else {
            continue;
        };
        let entry = nodes
            .entry(h.host.clone())
            .or_insert_with(|| NodeView::new(&h.host));
        // Latest health wins (>= so a same-ms republish still refreshes).
        if entry
            .health
            .as_ref()
            .is_none_or(|cur| h.published_at_ms >= cur.published_at_ms)
        {
            entry.health = Some(h);
        }
    }

    for body in workload_bodies {
        let Ok(snapshot) = serde_json::from_str::<WorkloadStateSnapshot>(body) else {
            continue;
        };
        if snapshot.validate(unix_millis()).is_err() {
            continue;
        }
        let (instances, containers) = workload_rows(&snapshot);
        let entry = nodes
            .entry(snapshot.node.clone())
            .or_insert_with(|| NodeView::new(&snapshot.node));
        if !entry.roster_seen || snapshot.observed_at_ms >= entry.roster_at_ms {
            entry.roster_at_ms = snapshot.observed_at_ms;
            entry.instances = instances;
            entry.roster_seen = true;
            entry.containers_at_ms = snapshot.observed_at_ms;
            entry.containers = containers;
            entry.containers_seen = true;
        }
    }

    nodes.into_values().collect()
}

/// The KVM header summary line + its tone (OK when all up, DANGER when degraded).
/// Mirrors `kvm_health::KvmHealth::status_line` semantics.
fn health_summary(h: &KvmHealth) -> (Color32, String) {
    if h.all_healthy {
        (Style::OK, format!("all {} KVM services up", h.total))
    } else {
        let down = h.total.saturating_sub(h.active);
        (
            Style::DANGER,
            format!("{}/{} up ({down} down)", h.active, h.total),
        )
    }
}

// ─────────────────────── JSON boundary (write / action) ───────────────────────

/// The Fleet form's legacy create fields. Creation is intentionally not
/// serialized here; the Workloads stepper owns definition and image approval.
#[derive(Debug)]
struct VmSpec {
    name: String,
    vcpus: u32,
    ram_mb: u64,
    disk_gb: u64,
}

/// A pending Fleet power intent. The Fleet roster remains an observation view;
/// `publish` below translates Start/Stop into the sole typed Workload operation
/// lane. Create is retained only as a visible migration affordance and is
/// rejected with an actionable Workloads redirect.
#[derive(Debug)]
enum Lifecycle {
    /// Define a new VM (leaves it shut off).
    Create,
    /// Boot a defined VM.
    Start { host: String, name: String },
    /// Stop a running VM (graceful unless `force`).
    Stop {
        host: String,
        name: String,
        force: bool,
    },
}

/// Publish one Fleet power intent through the sole typed Workload lane. No
/// retired lifecycle topic or direct backend publisher remains reachable from
/// the shell; VM creation belongs to the Workloads stepper.
fn publish(bus_root: Option<&Path>, last_error: &mut Option<String>, action: &Lifecycle) {
    let Some(root) = bus_root else {
        *last_error = Some("No mesh Bus directory — VM actions unavailable.".to_string());
        return;
    };
    let (host, name, operation) = match action {
        Lifecycle::Start { host, name } => (host, name, WorkloadOperationAction::Start),
        Lifecycle::Stop { host, name, .. } => (host, name, WorkloadOperationAction::Stop),
        Lifecycle::Create => {
            *last_error = Some(
                "VM creation is managed by Workloads — open Workloads → New workload to continue."
                    .into(),
            );
            return;
        }
    };
    let workload_id = format!("vm:{host}:{name}");
    let request = match crate::workload_api::request(
        &workload_id,
        host,
        WorkloadBackend::LibvirtVirtqemud,
        WorkloadProfile::Small.resources(),
        operation,
        None,
        0,
        unix_millis(),
    ) {
        Ok(request) => request,
        Err(error) => {
            *last_error = Some(format!("VM Workload authorization unavailable: {error}"));
            return;
        }
    };
    match crate::workload_api::publish(root, &request) {
        Ok(_) => *last_error = None,
        Err(error) => *last_error = Some(error),
    }
}

// ──────────────────────────── the Fleet state ────────────────────────────

/// The create form's raw text fields (parsed + validated on Create).
#[derive(Default)]
struct CreateForm {
    name: String,
    vcpus: String,
    ram_mb: String,
    disk_gb: String,
    /// Inline validation error for the open form (honest; never a panic).
    error: Option<String>,
}

impl CreateForm {
    /// Parse + validate the raw fields into a spec, or a human-readable message.
    fn to_spec(&self) -> Result<VmSpec, String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("VM name is required.".to_string());
        }
        let vcpus: u32 = self
            .vcpus
            .trim()
            .parse()
            .map_err(|_| "vCPUs must be a whole number.".to_string())?;
        if vcpus == 0 {
            return Err("vCPUs must be at least 1.".to_string());
        }
        let ram_mb: u64 = self
            .ram_mb
            .trim()
            .parse()
            .map_err(|_| "RAM (MiB) must be a whole number.".to_string())?;
        if ram_mb == 0 {
            return Err("RAM (MiB) must be greater than 0.".to_string());
        }
        let disk_gb: u64 = self
            .disk_gb
            .trim()
            .parse()
            .map_err(|_| "Disk (GiB) must be a whole number.".to_string())?;
        if disk_gb == 0 {
            return Err("Disk (GiB) must be greater than 0.".to_string());
        }
        Ok(VmSpec {
            name: name.to_string(),
            vcpus,
            ram_mb,
            disk_gb,
        })
    }
}

/// The Fleet plane's live datacenter state: the projected per-node view plus the
/// small IO/form context to refresh it and drive lifecycle.
pub(crate) struct DatacenterState {
    /// Desktop-client Bus spool (resolved once). `None` on a box with no Bus dir
    /// — the view then shows its empty state, never panics.
    bus_root: Option<PathBuf>,
    /// The latest projection, sorted by host. Empty until the first message lands
    /// (drives the loading state).
    nodes: Vec<NodeView>,
    /// BOOKMARKS-8 — the latest browser + ad-block fleet projection, sorted by host.
    browser: Vec<BrowserFleetRow>,
    /// The host whose inline "New VM" create form is open, if any.
    create_for: Option<String>,
    /// The (single, one-open-at-a-time) create form's fields.
    form: CreateForm,
    /// The last lifecycle-publish error, surfaced inline.
    last_error: Option<String>,
    /// shell-ux-5 — the single VM row whose Stop is armed (awaiting Confirm), as
    /// `(host, name)`. Only one row arms at a time; a Stop elsewhere re-arms to it.
    stop_arm: Option<(String, String)>,
    /// shell-ux-5 — VMs with a confirmed Stop in flight → when it was dispatched.
    /// The row shows an optimistic "stopping…" until the roster fold reflects the
    /// stop (the key leaves `running`) or `STOP_PENDING_TIMEOUT` re-exposes Stop.
    stopping: BTreeMap<(String, String), Instant>,
    /// When the Bus was last polled (drives the fixed cadence).
    last_poll: Option<Instant>,
}

impl Default for DatacenterState {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            nodes: Vec::new(),
            browser: Vec::new(),
            create_for: None,
            form: CreateForm::default(),
            last_error: None,
            stop_arm: None,
            stopping: BTreeMap::new(),
            last_poll: None,
        }
    }
}

impl DatacenterState {
    /// The bus-poll seam: refresh the projection from the Bus when the cadence has
    /// elapsed, then keep the repaint heartbeat alive so a health flip or new VM
    /// surfaces without input. Cheap enough to call every frame — it self-gates.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            self.refresh();
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// Queue an immediate re-read: the next `poll` refreshes regardless of the
    /// cadence. The MENU-1 "State of the Mesh" bar's Fleet → Refresh verb — the
    /// mouse twin of waiting out the poll cadence (§6, no second read path).
    pub(crate) const fn refresh_now(&mut self) {
        self.last_poll = None;
    }

    /// Cached service lifecycle candidates for Front Door. The launcher uses
    /// this read-only projection only; it does not trigger Fleet polling while
    /// opening, so local app/search paths stay responsive when the mesh is slow.
    pub(crate) fn front_door_lifecycle_candidates(&self) -> Vec<FrontDoorLifecycleCandidate> {
        let mut out = Vec::new();
        for node in &self.nodes {
            for inst in &node.instances {
                out.push(FrontDoorLifecycleCandidate {
                    host: node.host.clone(),
                    kind: FrontDoorLifecycleKind::Vm,
                    name: inst.name.clone(),
                    state: inst.state.clone(),
                    detail: "Virtual machine".to_owned(),
                });
            }
            for container in &node.containers {
                out.push(FrontDoorLifecycleCandidate {
                    host: node.host.clone(),
                    kind: FrontDoorLifecycleKind::Container,
                    name: container.name.clone(),
                    state: container.state.clone(),
                    detail: container.image.clone(),
                });
            }
        }
        out
    }

    /// Read the health stream and authoritative Workload projections and
    /// re-project. Split from the cadence gate so the pure
    /// projection stays testable; a missing dir / unreadable topic yields an empty
    /// or last-known projection, never a panic.
    fn refresh(&mut self) {
        let Some(root) = self.bus_root.clone() else {
            self.nodes = Vec::new();
            return;
        };
        // arch-11: open through the shared BusReader seam. The no-root case above
        // clears the roster (honest off-mesh empty); a transient open failure
        // keeps the last-known projection.
        let Some(persist) = BusReader::new(Some(root)).open() else {
            return;
        };
        let health = read_bodies(&persist, SERVICES_TOPIC);
        let topics = persist.list_topics().unwrap_or_default();
        let workloads = read_bodies_by_prefix(&persist, &topics, WORKLOAD_STATE_PREFIX);
        self.nodes = project(&health, &workloads);
        // BOOKMARKS-8 — the per-node fan-out state topics (one topic per node) are
        // enumerated by prefix, not a fixed name.
        let adfilter = read_bodies_by_prefix(&persist, &topics, ADFILTER_STATE_PREFIX);
        let policy = read_bodies_by_prefix(&persist, &topics, BROWSER_POLICY_STATE_PREFIX);
        self.browser = project_browser(&adfilter, &policy);
    }

    /// Render the Fleet plane's live datacenter content: per-node KVM
    /// service-health rows + Workload roster, with host-targeted create/start/stop
    /// controls. Shows an honest loading state before the first Bus message.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        // Disjoint field borrows so the render closures can hold `&nodes`
        // (shared) and `&mut form` / `&mut create_for` at once (egui idiom).
        let Self {
            bus_root,
            nodes,
            browser,
            create_for,
            form,
            last_error,
            stop_arm,
            stopping,
            ..
        } = self;

        // shell-ux-5: drop optimistic "stopping…" markers the roster now reflects
        // (the VM left `running`) or that outlived the timeout, so a stop that never
        // took re-exposes its Stop control instead of hanging on "stopping…".
        stopping.retain(|(h, n), since| {
            since.elapsed() < STOP_PENDING_TIMEOUT && roster_shows_running(nodes.as_slice(), h, n)
        });

        if let Some(err) = last_error.as_deref() {
            ui.colored_label(Style::DANGER, err);
            ui.add_space(Style::SP_S);
        }

        // docs-consistency-8 / WL-ARCH-006 — name this lens so it reads distinctly
        // from the Workloads surface: the Fleet view is the raw per-node
        // libvirt/KVM (and Podman) reality, NOT the tenant cloud (whose
        // *instances* live in the Workloads surface). Same word discipline both
        // ways: here a guest is a "VM"; in Workloads it is an "instance".
        mde_egui::muted_note(
            ui,
            "Raw per-node libvirt/KVM and Podman reality across the fleet. \
             Tenant cloud instances live in the Workloads surface.",
        );
        ui.add_space(Style::SP_XS);

        if nodes.is_empty() && browser.is_empty() {
            ui.add_space(Style::SP_S);
            ui.colored_label(Style::TEXT_DIM, "Waiting for KVM host health…");
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new(
                    "Each mesh node publishes its libvirt/Podman stack health, typed Workload \
                     roster, and browser/ad-block policy state to the Bus.",
                )
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            return;
        }

        // Collect at most one action from this frame, applied after the render
        // borrow of `nodes`/`form`/`create_for` ends.
        let mut pending: Option<Lifecycle> = None;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for node in nodes.iter() {
                    // Each mesh node is a genuinely raised Fleet card via the shared
                    // `mde_egui::card()` primitive — the single source of the surface
                    // fill, hairline, radius, padding, and the `Elevation::Raised`
                    // soft shadow (§4).
                    mde_egui::card().show(ui, |ui| {
                        show_node(ui, node, create_for, form, &mut pending, stop_arm, stopping);
                    });
                    ui.add_space(Style::SP_S);
                }
                if !nodes.is_empty() {
                    // MV-6b lands the container roster without half-wired run/stop
                    // actions. Keep the visible note operator-facing; the wiring
                    // detail stays in the module docs.
                    mde_egui::muted_note(
                        ui,
                        "Containers are inventory-only here. Start and stop containers from their owning service.",
                    );
                }
                // BOOKMARKS-8 — the browser + ad-block fleet section (its own per-node
                // rows; the state comes from the `adfilter` + `browser_policy` workers).
                if !browser.is_empty() {
                    ui.add_space(Style::SP_M);
                    ui.label(
                        RichText::new("Browser & ad-block policy")
                            .color(Style::TEXT)
                            .size(Style::BODY)
                            .strong(),
                    );
                    ui.add_space(Style::SP_XS);
                    for row in browser.iter() {
                        // Same shared `mde_egui::card()` primitive as the node rows above.
                        mde_egui::card().show(ui, |ui| {
                            show_browser_row(ui, row);
                        });
                        ui.add_space(Style::SP_S);
                    }
                }
            });

        if let Some(action) = pending {
            publish(bus_root.as_deref(), last_error, &action);
        }
    }
}

/// Read the newest JSON body on a retained latest-value `topic`.
///
/// Health and per-node Workload/browser projections publish complete snapshots,
/// so replaying their entire history only creates stale allocations for the
/// render fold. The latest-row probe keeps the Fleet reader bounded.
fn read_bodies(persist: &Persist, topic: &str) -> Vec<String> {
    persist
        .read_latest(topic)
        .ok()
        .and_then(|message| message.and_then(|message| message.body))
        .into_iter()
        .collect()
}

/// Read the newest JSON body on every topic under `prefix` (the per-node
/// `state/<service>/<node>` fan-out). `topics` is the already-enumerated topic
/// list so a single `list_topics` serves several prefixes.
fn read_bodies_by_prefix(persist: &Persist, topics: &[String], prefix: &str) -> Vec<String> {
    topics
        .iter()
        .filter(|t| t.starts_with(prefix))
        .flat_map(|t| read_bodies(persist, t))
        .collect()
}

fn datacenter_tooltip(ui: &mut egui::Ui, text: &str) {
    let ctx = ui.ctx().clone();
    let surface = Style::resolve_color(&ctx, Style::SURFACE);
    let border = Style::resolve_color(&ctx, Style::BORDER);
    let text_color = Style::resolve_color(&ctx, Style::TEXT);
    egui::Frame::NONE
        .fill(surface)
        .stroke(egui::Stroke::new(1.0, border))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(Style::tooltip_margin())
        .show(ui, |ui| {
            ui.set_max_width(DATACENTER_TOOLTIP_MAX_W);
            ui.add(
                egui::Label::new(RichText::new(text).size(Style::SMALL).color(text_color)).wrap(),
            );
        });
}

fn datacenter_hover_text(response: egui::Response, text: impl Into<String>) -> egui::Response {
    let text = text.into();
    response.on_hover_ui(move |ui| datacenter_tooltip(ui, text.as_str()))
}

/// Render one node's section: header + health summary, KVM service rows, VM
/// roster with per-VM start/stop, and the inline create control. Any button
/// click sets `pending` (the host is always this node — the fan-out guard).
fn show_node(
    ui: &mut egui::Ui,
    node: &NodeView,
    create_for: &mut Option<String>,
    form: &mut CreateForm,
    pending: &mut Option<Lifecycle>,
    stop_arm: &mut Option<(String, String)>,
    stopping: &mut BTreeMap<(String, String), Instant>,
) {
    // Header — host + KVM health summary.
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(&node.host)
                .color(Style::TEXT)
                .size(Style::BODY)
                .strong(),
        );
        ui.add_space(Style::SP_S);
        match &node.health {
            Some(h) => {
                let (color, text) = health_summary(h);
                ui.colored_label(color, RichText::new(text).size(Style::SMALL));
            }
            None => {
                mde_egui::muted_note(ui, "KVM health not yet reported");
            }
        }
    });

    // KVM service rows.
    if let Some(h) = &node.health {
        ui.add_space(Style::SP_XS);
        ui.indent((node.host.as_str(), "svc"), |ui| {
            for svc in &h.services {
                ui.horizontal(|ui| {
                    let (dot, label) = if svc.active {
                        (Style::OK, "up")
                    } else {
                        (Style::DANGER, "down")
                    };
                    ui.label(RichText::new("\u{25CF}").color(dot).size(Style::SMALL));
                    ui.add_space(Style::SP_XS);
                    let service_response =
                        ui.label(RichText::new(&svc.id).color(Style::TEXT).size(Style::SMALL));
                    let _ = datacenter_hover_text(
                        service_response,
                        format!("systemd unit: {}", svc.unit),
                    );
                    ui.add_space(Style::SP_XS);
                    let tone = if svc.active {
                        Style::TEXT_DIM
                    } else {
                        Style::DANGER
                    };
                    ui.colored_label(tone, RichText::new(label).size(Style::SMALL));
                });
            }
        });
    }

    // VM roster + per-VM controls.
    ui.add_space(Style::SP_XS);
    ui.label(
        RichText::new("Virtual machines")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
    );
    ui.indent((node.host.as_str(), "vms"), |ui| {
        if node.instances.is_empty() {
            let msg = if node.roster_seen {
                "No VMs defined on this node."
            } else {
                "VM roster not yet reported."
            };
            mde_egui::muted_note(ui, msg);
        } else {
            for inst in &node.instances {
                ui.horizontal(|ui| {
                    show_instance_row(ui, &node.host, inst, pending, stop_arm, stopping);
                });
            }
        }
    });

    // Create control (host-targeted to this node).
    ui.add_space(Style::SP_XS);
    show_create(ui, &node.host, create_for, form, pending);

    // Container roster (read-only — mirrors the VM roster; MV-6b).
    ui.add_space(Style::SP_XS);
    ui.label(
        RichText::new("Containers")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
    );
    ui.indent((node.host.as_str(), "containers"), |ui| {
        if node.containers.is_empty() {
            let msg = if node.containers_seen {
                "No containers on this node."
            } else {
                "Container roster not yet reported."
            };
            mde_egui::muted_note(ui, msg);
        } else {
            for c in &node.containers {
                ui.horizontal(|ui| {
                    show_container_row(ui, c);
                });
            }
        }
    });
}

/// One container roster row (read-only): a state pip + name + approved image +
/// typed power state. Mirrors [`show_instance_row`] without lifecycle buttons —
/// container run/stop drive remains in the Workloads surface.
fn show_container_row(ui: &mut egui::Ui, c: &Container) {
    let running = c.state.trim() == "running";
    let dot = if running { Style::OK } else { Style::TEXT_DIM };
    ui.label(RichText::new("\u{25CF}").color(dot).size(Style::SMALL));
    ui.add_space(Style::SP_XS);
    ui.label(RichText::new(&c.name).color(Style::TEXT).size(Style::SMALL));
    ui.add_space(Style::SP_S);
    mde_egui::muted_note(ui, &c.image);
    ui.add_space(Style::SP_S);
    mde_egui::muted_note(ui, &c.state);
}

/// Render one node's browser + ad-block fleet section (read-only): the header +
/// the enforced browser-policy state (BOOKMARKS-8) and the ad-block stats
/// (BOOKMARKS-7). Honest empty notes when a stream hasn't reported yet.
fn show_browser_row(ui: &mut egui::Ui, row: &BrowserFleetRow) {
    // Header — host + the browser enabled/disabled tone.
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(&row.host)
                .color(Style::TEXT)
                .size(Style::BODY)
                .strong(),
        );
        ui.add_space(Style::SP_S);
        match &row.policy {
            Some(p) => {
                let (dot, label) = if p.browser_enabled {
                    (Style::OK, "browser enabled")
                } else {
                    (Style::WARN, "browser disabled (surface hidden)")
                };
                ui.label(RichText::new("\u{25CF}").color(dot).size(Style::SMALL));
                ui.add_space(Style::SP_XS);
                let tone = if p.browser_enabled {
                    Style::TEXT_DIM
                } else {
                    Style::WARN
                };
                ui.colored_label(tone, RichText::new(label).size(Style::SMALL));
            }
            None => {
                mde_egui::muted_note(ui, "browser policy not yet reported");
            }
        }
    });

    // Enforced browser-policy detail (BOOKMARKS-8).
    if let Some(p) = &row.policy {
        ui.indent((row.host.as_str(), "policy"), |ui| {
            show_policy_detail(ui, p);
        });
    }

    // Ad-block stats (BOOKMARKS-7).
    ui.add_space(Style::SP_XS);
    ui.indent((row.host.as_str(), "adblock"), |ui| {
        show_adblock_stats(ui, row.adblock.as_ref());
    });
}

/// The enforced browser-policy detail rows: the folded role + forced-ad-blocker
/// state, the navigation allowlist / custom lists, the policy source, the honest
/// data-retention note, and the enforcement counters.
fn show_policy_detail(ui: &mut egui::Ui, p: &BrowserPolicyStat) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("role: {}", p.role))
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        let (dot, label) = if p.force_adblock {
            (Style::OK, "ad-blocker forced on")
        } else {
            (Style::TEXT_DIM, "ad-blocker optional")
        };
        ui.label(RichText::new("\u{25CF}").color(dot).size(Style::SMALL));
        ui.add_space(Style::SP_XS);
        ui.colored_label(Style::TEXT_DIM, RichText::new(label).size(Style::SMALL));
    });
    // Navigation allowlist + custom filter lists.
    let allow = if p.url_allowlist.is_empty() {
        "navigation unrestricted".to_string()
    } else {
        format!("navigation allowlist: {} domain(s)", p.url_allowlist.len())
    };
    let lists = if p.custom_filter_lists.is_empty() {
        String::new()
    } else {
        format!(" · {} custom list(s)", p.custom_filter_lists.len())
    };
    mde_egui::muted_note(ui, format!("{allow}{lists}"));
    // Policy source.
    let src = if p.policy_source.is_empty() {
        "default baseline (no fleet policy authored)".to_string()
    } else {
        format!("policy from {}", p.policy_source)
    };
    mde_egui::muted_note(ui, src);
    // Honest data-retention (a disabled browser retains its local data).
    if !p.browser_enabled && p.local_data_retained {
        ui.colored_label(
            Style::OK,
            RichText::new("local data retained (no wipe)").size(Style::SMALL),
        );
    }
    // Enforcement counters (only when something has been rejected).
    let rejected = p.launches_refused + p.navigations_rejected + p.adblock_toggles_rejected;
    if rejected > 0 {
        ui.colored_label(
            Style::WARN,
            RichText::new(format!(
                "enforced: {} launch · {} navigation · {} ad-block-off rejected",
                p.launches_refused, p.navigations_rejected, p.adblock_toggles_rejected
            ))
            .size(Style::SMALL),
        );
    }
}

/// The ad-block stats rows (BOOKMARKS-7): a staleness pip + the lists / rules /
/// allowlist counts, or an honest "not yet reported" note.
fn show_adblock_stats(ui: &mut egui::Ui, adblock: Option<&AdblockStat>) {
    match adblock {
        Some(a) => {
            ui.horizontal(|ui| {
                let (dot, label) = staleness_label(&a.staleness);
                ui.label(RichText::new("\u{25CF}").color(dot).size(Style::SMALL));
                ui.add_space(Style::SP_XS);
                ui.colored_label(dot, RichText::new(label).size(Style::SMALL));
            });
            mde_egui::muted_note(
                ui,
                format!(
                    "{}/{} lists · {} network + {} cosmetic rules · {} site(s) allowlisted",
                    a.enabled_sources,
                    a.total_sources,
                    a.network_rules,
                    a.cosmetic_rules,
                    a.allowlisted_sites
                ),
            );
        }
        None => {
            mde_egui::muted_note(ui, "ad-block stats not yet reported");
        }
    }
}

/// How long an optimistic "stopping…" marker holds before the Stop control is
/// re-exposed for a confirmed request the roster never reflected.
const STOP_PENDING_TIMEOUT: Duration = Duration::from_secs(30);

/// The three Stop-control clicks in the two-step arm (shell-ux-5).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StopButton {
    /// Idle → armed: the first click, which only arms this row (no dispatch).
    Stop,
    /// Armed → dispatch: the confirm click, which queues the Stop.
    Confirm,
    /// Armed → idle: back out without stopping.
    Cancel,
}

/// Apply a Stop-control click for `(host, name)` to the shared single-row `arm`
/// slot. Mirrors the shell power control's arm-then-confirm gate, scaled to a
/// per-row Stop: the first
/// `Stop` arms this row, a second (`Confirm`) disarms and returns the Stop action
/// to dispatch, `Cancel` disarms. Returns `Some` ONLY on a confirmed click, so a
/// running VM — plausibly a live brokered desktop — is never stopped on one click.
fn apply_stop_click(
    arm: &mut Option<(String, String)>,
    host: &str,
    name: &str,
    click: StopButton,
) -> Option<Lifecycle> {
    match click {
        StopButton::Stop => {
            *arm = Some((host.to_string(), name.to_string()));
            None
        }
        StopButton::Confirm => {
            *arm = None;
            Some(Lifecycle::Stop {
                host: host.to_string(),
                name: name.to_string(),
                force: false,
            })
        }
        StopButton::Cancel => {
            *arm = None;
            None
        }
    }
}

/// Whether the roster projection still shows `name` on `host` in the `running`
/// state — the signal an optimistic "stopping…" marker watches for so it clears
/// itself the moment the next roster fold reflects the stop.
fn roster_shows_running(nodes: &[NodeView], host: &str, name: &str) -> bool {
    nodes.iter().any(|nv| {
        nv.host == host
            && nv
                .instances
                .iter()
                .any(|i| i.name == name && i.state.trim() == "running")
    })
}

/// One VM roster row from the typed Workload projection: a state pip + name +
/// power state, and a Start (when not
/// running) or a two-step Stop (when running) targeted at this node. Stopping a
/// running VM is plausibly someone's live brokered desktop, so the Stop arms first
/// — a DANGER "Confirm stop <name>" + Cancel — matching the typed-arm discipline
/// the disk controls already require (shell-ux-5). Once confirmed, the row shows an
/// optimistic "stopping…" until the roster reflects it (or `STOP_PENDING_TIMEOUT`).
fn show_instance_row(
    ui: &mut egui::Ui,
    host: &str,
    inst: &Instance,
    pending: &mut Option<Lifecycle>,
    stop_arm: &mut Option<(String, String)>,
    stopping: &mut BTreeMap<(String, String), Instant>,
) {
    let running = inst.state.trim() == "running";
    let dot = if running { Style::OK } else { Style::TEXT_DIM };
    ui.label(RichText::new("\u{25CF}").color(dot).size(Style::SMALL));
    ui.add_space(Style::SP_XS);
    ui.label(
        RichText::new(&inst.name)
            .color(Style::TEXT)
            .size(Style::SMALL),
    );
    ui.add_space(Style::SP_S);
    mde_egui::muted_note(ui, &inst.state);
    ui.add_space(Style::SP_S);
    if running {
        let key = (host.to_string(), inst.name.clone());
        if stopping.contains_key(&key) {
            // Optimistic pending — a confirmed stop is in flight; wait for the
            // roster fold (or the timeout) rather than offer Stop again.
            mde_egui::muted_note(ui, "stopping…");
        } else if stop_arm.as_ref() == Some(&key) {
            // Armed — a DANGER Confirm (a disabled-by-arming echo isn't needed for a
            // one-VM stop; the explicit second click is the gate) + a Cancel.
            if ui
                .button(
                    RichText::new(format!("Confirm stop {}", inst.name))
                        .size(Style::SMALL)
                        .color(Style::DANGER),
                )
                .clicked()
            {
                if let Some(action) =
                    apply_stop_click(stop_arm, host, &inst.name, StopButton::Confirm)
                {
                    *pending = Some(action);
                    stopping.insert(key, Instant::now());
                }
            }
            if ui
                .button(RichText::new("Cancel").size(Style::SMALL))
                .clicked()
            {
                apply_stop_click(stop_arm, host, &inst.name, StopButton::Cancel);
            }
        } else if ui
            .button(RichText::new("Stop").size(Style::SMALL))
            .clicked()
        {
            apply_stop_click(stop_arm, host, &inst.name, StopButton::Stop);
        }
    } else if ui
        .button(RichText::new("Start").size(Style::SMALL))
        .clicked()
    {
        *pending = Some(Lifecycle::Start {
            host: host.to_string(),
            name: inst.name.clone(),
        });
    }
}

/// The inline "New VM" create control for one node: a toggle that opens a small
/// form (name / vCPUs / RAM / disk); Create publishes a host-targeted request.
fn show_create(
    ui: &mut egui::Ui,
    host: &str,
    create_for: &mut Option<String>,
    form: &mut CreateForm,
    pending: &mut Option<Lifecycle>,
) {
    let open = create_for.as_deref() == Some(host);
    if !open {
        if ui
            .button(RichText::new("\u{FF0B} New VM").size(Style::SMALL))
            .clicked()
        {
            *create_for = Some(host.to_string());
            *form = CreateForm::default();
        }
        return;
    }

    ui.label(
        RichText::new("New VM")
            .color(Style::TEXT)
            .size(Style::SMALL)
            .strong(),
    );
    form_field(ui, "Name", &mut form.name);
    form_field(ui, "vCPUs", &mut form.vcpus);
    form_field(ui, "RAM (MiB)", &mut form.ram_mb);
    form_field(ui, "Disk (GiB)", &mut form.disk_gb);

    if let Some(err) = form.error.as_deref() {
        ui.colored_label(Style::DANGER, RichText::new(err).size(Style::SMALL));
    }

    ui.horizontal(|ui| {
        if ui
            .button(RichText::new("Create").size(Style::SMALL))
            .clicked()
        {
            match form.to_spec() {
                Ok(spec) => {
                    let _ = spec;
                    *pending = Some(Lifecycle::Create);
                    *create_for = None;
                }
                Err(e) => form.error = Some(e),
            }
        }
        if ui
            .button(RichText::new("Cancel").size(Style::SMALL))
            .clicked()
        {
            *create_for = None;
        }
    });
}

/// A labelled single-line text field, laid out on the spacing grid.
fn form_field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add(egui::TextEdit::singleline(value).desired_width(Style::SP_XL * 4.0));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_bus::hooks::config::Priority;

    fn health_body(host: &str, all_healthy: bool, at: u64) -> String {
        // A minimal but faithful `event/kvm/services` body.
        format!(
            r#"{{"host":"{host}","services":[{{"id":"libvirtd","unit":"libvirtd.service","active":true}},{{"id":"podman","unit":"podman.socket","active":{}}}],"active":{},"total":2,"all_healthy":{all_healthy},"published_at_ms":{at}}}"#,
            all_healthy,
            if all_healthy { 2 } else { 1 },
        )
    }

    fn roster_body(host: &str, names_states: &[(&str, &str)], at: u64) -> String {
        workload_body(host, names_states, &[], at)
    }

    fn container_body(host: &str, rows: &[(&str, &str, &str)], at: u64) -> String {
        // A minimal authoritative Workload snapshot — each row is
        // (name, approved image, power state).
        workload_body(host, &[], rows, at)
    }

    fn workload_body(
        host: &str,
        vms: &[(&str, &str)],
        containers: &[(&str, &str, &str)],
        at: u64,
    ) -> String {
        let mut workloads: Vec<_> = vms
            .iter()
            .map(|(name, state)| workload_status(host, "vm", name, state, None))
            .collect();
        workloads.extend(containers.iter().map(|(name, image, state)| {
            workload_status(host, "container", name, state, Some(image))
        }));
        workload_snapshot(host, at, workloads)
    }

    fn workload_status(
        host: &str,
        kind: &str,
        name: &str,
        state: &str,
        image: Option<&str>,
    ) -> serde_json::Value {
        let backend = if kind == "vm" {
            "libvirt_virtqemud"
        } else {
            "quadlet_systemd"
        };
        let power = if state == "running" {
            "running"
        } else if matches!(state, "paused") {
            "paused"
        } else {
            "stopped"
        };
        let phase = if state == "running" {
            "ready"
        } else {
            "completed"
        };
        let readiness = if state == "running" {
            "ready"
        } else {
            "unknown"
        };
        let mut value = serde_json::json!({
            "schema_version": 1,
            "request_id": format!("test-{kind}-{name}"),
            "workload_id": format!("{kind}:{host}:{name}"),
            "backend": backend,
            "resources": {"vcpu": 2, "memory_mb": 4096, "disk_gb": 32},
            "generation": 1,
            "phase": phase,
            "power": power,
            "readiness": readiness,
            "retryable": false,
            "attempt": 0,
            "next_retry_at_ms": 0,
            "reason": serde_json::Value::Null,
            "remediation": serde_json::Value::Null,
            "attachment": serde_json::Value::Null,
        });
        if let Some(image) = image {
            value["image_ref"] = serde_json::Value::String((*image).to_owned());
        }
        value
    }

    fn workload_snapshot(host: &str, at: u64, workloads: Vec<serde_json::Value>) -> String {
        serde_json::json!({
            "schema_version": 1,
            "node": host,
            "observed_at_ms": at,
            "workloads": workloads,
        })
        .to_string()
    }

    /// Keep the older test call shape while every body now represents the same
    /// authoritative Workload topic. Production has only the two-argument
    /// `super::project` path.
    fn project(
        health_bodies: &[String],
        workload_bodies: &[String],
        additional_workload_bodies: &[String],
    ) -> Vec<NodeView> {
        let mut bodies = workload_bodies.to_vec();
        bodies.extend(additional_workload_bodies.iter().cloned());
        super::project(health_bodies, &bodies)
    }

    #[test]
    fn front_door_lifecycle_candidates_use_cached_vm_and_container_rosters() {
        let mut state = DatacenterState {
            nodes: project(
                &[],
                &[workload_body(
                    "oak",
                    &[("dispatch-vm", "running")],
                    &[("mesh-api", "construct.mesh-api:latest", "exited")],
                    11,
                )],
                &[],
            ),
            ..DatacenterState::default()
        };

        let rows = state.front_door_lifecycle_candidates();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|row| {
            row.host == "oak"
                && row.kind == FrontDoorLifecycleKind::Vm
                && row.name == "dispatch-vm"
                && row.state == "running"
        }));
        assert!(rows.iter().any(|row| {
            row.host == "oak"
                && row.kind == FrontDoorLifecycleKind::Container
                && row.name == "mesh-api"
                && row.detail == "construct.mesh-api:latest"
        }));

        state.nodes.clear();
        assert!(
            state.front_door_lifecycle_candidates().is_empty(),
            "Front Door uses cached roster facts only and does not poll during launcher open"
        );
    }

    fn painted_text(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
        fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }

        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    fn painted_text_colors(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Color32)> {
        fn text_color(text: &egui::epaint::TextShape) -> egui::Color32 {
            if let Some(color) = text.override_text_color {
                return color;
            }
            text.galley
                .job
                .sections
                .iter()
                .find_map(|section| {
                    (section.format.color != egui::Color32::PLACEHOLDER)
                        .then_some(section.format.color)
                })
                .unwrap_or(text.fallback_color)
        }

        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Color32)>) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push((text.galley.text().to_owned(), text_color(text)))
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }

        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    fn painted_fill_colors(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Color32> {
        fn walk(shape: &egui::Shape, out: &mut Vec<egui::Color32>) {
            match shape {
                egui::Shape::Rect(rect) => {
                    if rect.fill != egui::Color32::TRANSPARENT {
                        out.push(rect.fill);
                    }
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }

        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    fn render_datacenter_tooltip_frame(ctx: &egui::Context, text: &str) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(420.0, 104.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        datacenter_tooltip(ui, text);
                    });
            },
        )
    }

    fn render_datacenter_state(state: &mut DatacenterState) -> egui::FullOutput {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(720.0, 520.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        state.show(ui);
                    });
            },
        )
    }

    #[test]
    fn datacenter_hover_tooltip_uses_themed_text_and_surface_in_light_mode() {
        const SERVICE_TOOLTIP: &str = "virtqemud.service";
        let ctx = egui::Context::default();
        Style::install_color_scheme_with_density(
            &ctx,
            mde_egui::StyleColorScheme::Light,
            mde_egui::Density::Mouse,
        );
        let out = render_datacenter_tooltip_frame(&ctx, SERVICE_TOOLTIP);
        let text_color = Style::resolve_color(&ctx, Style::TEXT);
        let surface = Style::resolve_color(&ctx, Style::SURFACE);

        let texts = painted_text_colors(&out.shapes);
        assert!(
            texts
                .iter()
                .any(|(text, color)| text == SERVICE_TOOLTIP && *color == text_color),
            "Datacenter tooltip should paint themed text: {texts:?}"
        );
        assert!(
            text_color != surface,
            "Datacenter tooltip text and surface must stay distinct in light mode"
        );

        let fills = painted_fill_colors(&out.shapes);
        assert!(
            fills.contains(&surface),
            "Datacenter tooltip should paint its themed surface: {fills:?}"
        );
    }

    /// A faithful `state/adfilter/<node>` body (BOOKMARKS-7 `AdfilterStatus`).
    fn adfilter_body(node: &str, enabled: usize, net_rules: usize, at: u64) -> String {
        format!(
            r#"{{"node":"{node}","enabled_sources":{enabled},"total_sources":{enabled},"network_rules":{net_rules},"cosmetic_rules":2,"allowlisted_sites":1,"staleness":"NeverSynced","age_ms":null,"synced_ms":null,"peers":0,"share_reachable":true,"last_flush_ms":{at}}}"#
        )
    }

    /// A faithful `state/browser-policy/<node>` body (BOOKMARKS-8).
    fn policy_body(node: &str, role: &str, enabled: bool, force: bool, at: u64) -> String {
        format!(
            r#"{{"node":"{node}","role":"{role}","browser_enabled":{enabled},"surface_hidden":{},"force_adblock":{force},"url_allowlist":["example.com"],"custom_filter_lists":[{{"name":"Corp","url":null}}],"policy_updated_ms":1000,"policy_source":"operator@eagle","last_launch_refused":false,"launches_granted":2,"launches_refused":1,"navigations_rejected":0,"adblock_toggles_rejected":0,"peers":0,"share_reachable":true,"local_data_retained":true,"last_flush_ms":{at}}}"#,
            !enabled
        )
    }

    #[test]
    fn project_browser_folds_one_row_per_host_sorted_with_both_streams() {
        let adfilter = vec![
            adfilter_body("node-b", 3, 40, 1),
            adfilter_body("node-a", 4, 55, 1),
        ];
        let policy = vec![policy_body("node-a", "workstation", true, true, 1)];
        let rows = project_browser(&adfilter, &policy);
        assert_eq!(rows.len(), 2, "one row per host");
        assert_eq!(rows[0].host, "node-a"); // BTreeMap sorted
        assert_eq!(rows[1].host, "node-b");
        // node-a folds the ad-block stats and browser policy.
        let a = &rows[0];
        assert!(a.adblock.as_ref().is_some_and(|s| s.enabled_sources == 4));
        assert!(a
            .policy
            .as_ref()
            .is_some_and(|p| p.browser_enabled && p.force_adblock));
        // node-b has ad-block stats but no policy reported yet.
        assert!(rows[1].adblock.is_some());
        assert!(rows[1].policy.is_none());
    }

    #[test]
    fn project_browser_latest_flush_wins_per_stream() {
        let adfilter = vec![
            adfilter_body("node-a", 3, 30, 10),
            adfilter_body("node-a", 5, 60, 20),
        ];
        let rows = project_browser(&adfilter, &[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].adblock.as_ref().map(|s| s.enabled_sources),
            Some(5),
            "the newer flush wins"
        );
    }

    #[test]
    fn project_browser_surfaces_a_disabled_policy_with_retained_data() {
        // A lighthouse node with the browser DISABLED — the fleet view must surface
        // the honest disabled + retained-data state (BOOKMARKS-8 acceptance).
        let policy = vec![policy_body("peer:lh", "lighthouse", false, true, 1)];
        let rows = project_browser(&[], &policy);
        assert_eq!(rows.len(), 1);
        let p = rows[0].policy.as_ref().expect("policy folded");
        assert!(!p.browser_enabled);
        assert!(p.surface_hidden, "a disabled browser hides the surface");
        assert!(p.local_data_retained, "the disable retains local data");
    }

    #[test]
    fn project_browser_skips_malformed_bodies() {
        let adfilter = vec!["not json".to_string(), "{}".to_string()];
        let policy = vec![r#"{"unexpected":true}"#.to_string()];
        assert!(project_browser(&adfilter, &policy).is_empty());
    }

    #[test]
    fn datacenter_container_inventory_note_is_operator_facing() {
        let mut state = DatacenterState {
            bus_root: None,
            nodes: project(
                &[health_body("node-a", true, 1)],
                &[],
                &[container_body(
                    "node-a",
                    &[("cache", "redis:7", "running")],
                    2,
                )],
            ),
            ..DatacenterState::default()
        };
        let out = render_datacenter_state(&mut state);
        let texts = painted_text(&out.shapes);

        assert!(
            texts.iter().any(|text| text.contains(
                "Containers are inventory-only here. Start and stop containers from their owning service."
            )),
            "Fleet container note should explain the operator-facing state: {texts:?}"
        );
        for forbidden in [
            "lifecycle-drive",
            "follow-up",
            "Start container",
            "Stop container",
        ] {
            assert!(
                texts.iter().all(|text| !text.contains(forbidden)),
                "Fleet container UI must not expose {forbidden:?}: {texts:?}"
            );
        }
    }

    #[test]
    fn staleness_label_tones() {
        assert_eq!(staleness_label(&Staleness::Fresh).0, Style::OK);
        assert_eq!(staleness_label(&Staleness::NeverSynced).0, Style::TEXT_DIM);
        let (tone, text) = staleness_label(&Staleness::Stale {
            age_ms: 8 * 24 * 60 * 60 * 1000,
        });
        assert_eq!(tone, Style::WARN);
        assert!(text.contains("8d"));
    }

    #[test]
    fn project_folds_one_row_per_host_sorted() {
        let health = vec![
            health_body("node-b", true, 1),
            health_body("node-a", false, 1),
        ];
        let workloads = vec![workload_body(
            "node-a",
            &[("web1", "running")],
            &[("cache", "redis:7", "running")],
            1,
        )];
        let nodes = project(&health, &workloads, &[]);
        assert_eq!(nodes.len(), 2, "one row per host");
        // BTreeMap key order → sorted by host.
        assert_eq!(nodes[0].host, "node-a");
        assert_eq!(nodes[1].host, "node-b");
        // node-a has a health summary, a VM roster, AND a container roster — the
        // "VMs and containers" surface folds onto one node view.
        assert!(nodes[0].health.is_some());
        assert!(nodes[0].roster_seen);
        assert_eq!(nodes[0].instances.len(), 1);
        assert_eq!(nodes[0].instances[0].name, "web1");
        assert!(nodes[0].containers_seen);
        assert_eq!(nodes[0].containers.len(), 1);
        assert_eq!(nodes[0].containers[0].name, "cache");
        assert_eq!(nodes[0].containers[0].image, "redis:7");
        assert_eq!(nodes[0].containers[0].state, "running");
        // node-b has health but no VM roster / container roster reported yet.
        assert!(nodes[1].health.is_some());
        assert!(!nodes[1].roster_seen);
        assert!(!nodes[1].containers_seen);
    }

    #[test]
    fn project_latest_health_wins_per_host() {
        // Two health messages for the same host; the later publish must win.
        let health = vec![
            health_body("node-a", false, 10),
            health_body("node-a", true, 20),
        ];
        let nodes = project(&health, &[], &[]);
        assert_eq!(nodes.len(), 1);
        let h = &nodes[0].health;
        assert!(
            h.as_ref().is_some_and(|h| h.all_healthy),
            "the newer (all-healthy) summary wins"
        );
        assert_eq!(h.as_ref().map(|h| h.published_at_ms), Some(20));
    }

    #[test]
    fn project_latest_roster_wins_per_host() {
        let instances = vec![
            roster_body("node-a", &[("web1", "running")], 5),
            roster_body("node-a", &[("web1", "shut off"), ("db1", "running")], 9),
        ];
        let nodes = project(&[], &instances, &[]);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].instances.len(), 2, "the newer roster wins");
        assert_eq!(nodes[0].roster_at_ms, 9);
    }

    #[test]
    fn stop_click_arms_then_confirm_dispatches_and_cancel_disarms() {
        // shell-ux-5: a running VM is stopped only on a two-step arm, never a
        // single click.
        let mut arm: Option<(String, String)> = None;

        // First Stop click only arms this row — no action dispatched.
        let dispatched = apply_stop_click(&mut arm, "node-a", "vm1", StopButton::Stop);
        assert!(
            dispatched.is_none(),
            "the first Stop click must not dispatch"
        );
        assert_eq!(
            arm,
            Some(("node-a".to_string(), "vm1".to_string())),
            "the first click arms this row"
        );

        // Confirm on the armed row dispatches a non-forced Stop and disarms.
        let dispatched = apply_stop_click(&mut arm, "node-a", "vm1", StopButton::Confirm);
        match dispatched {
            Some(Lifecycle::Stop { host, name, force }) => {
                assert_eq!(host, "node-a");
                assert_eq!(name, "vm1");
                assert!(!force, "the row Stop is graceful, never forced");
            }
            other => panic!("Confirm should dispatch a Stop, got {other:?}"),
        }
        assert!(arm.is_none(), "Confirm disarms");

        // Re-arm, then Cancel disarms without dispatching anything.
        apply_stop_click(&mut arm, "node-a", "vm1", StopButton::Stop);
        assert!(arm.is_some(), "re-armed");
        let dispatched = apply_stop_click(&mut arm, "node-a", "vm1", StopButton::Cancel);
        assert!(dispatched.is_none(), "Cancel dispatches nothing");
        assert!(arm.is_none(), "Cancel disarms");
    }

    #[test]
    fn stopping_marker_watches_the_roster_running_state() {
        // The optimistic "stopping…" marker clears once the roster fold no longer
        // shows the VM running (or never showed it) — the clear signal in `show`.
        let instances = vec![roster_body(
            "node-a",
            &[("vm1", "running"), ("vm2", "shut off")],
            5,
        )];
        let nodes = project(&[], &instances, &[]);
        assert!(
            roster_shows_running(&nodes, "node-a", "vm1"),
            "vm1 is still running — keep the marker"
        );
        assert!(
            !roster_shows_running(&nodes, "node-a", "vm2"),
            "vm2 left running — clear the marker"
        );
        assert!(
            !roster_shows_running(&nodes, "node-a", "ghost"),
            "an unknown VM is not running"
        );
        assert!(
            !roster_shows_running(&nodes, "other-host", "vm1"),
            "a VM on a different host is not this row"
        );
    }

    #[test]
    fn project_skips_malformed_bodies() {
        let health = vec!["not json".to_string(), health_body("node-a", true, 1)];
        let nodes = project(&health, &["{}".to_string()], &[]);
        // The garbage + the `{}` (missing required fields) are dropped; only the
        // valid node-a survives.
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].host, "node-a");
    }

    #[test]
    fn empty_bodies_project_to_no_nodes() {
        assert!(project(&[], &[], &[]).is_empty());
    }

    #[test]
    fn latest_value_reader_does_not_materialize_retained_projection_history() {
        let dir = tempfile::tempdir().expect("temp Bus");
        let persist = Persist::open(dir.path().to_path_buf()).expect("open Bus");
        for at in 0..=64 {
            persist
                .write(
                    SERVICES_TOPIC,
                    Priority::Default,
                    None,
                    Some(&health_body("node-a", true, at)),
                )
                .expect("write retained health snapshot");
        }

        let bodies = read_bodies(&persist, SERVICES_TOPIC);
        assert_eq!(bodies.len(), 1, "latest-value topics admit one row");
        assert!(
            bodies[0].contains("\"published_at_ms\":64"),
            "the newest complete projection remains visible"
        );
    }

    #[test]
    fn project_folds_containers_per_host_sorted() {
        let containers = vec![
            container_body("node-b", &[("web", "nginx:latest", "running")], 1),
            container_body("node-a", &[("db", "postgres:16", "exited")], 1),
        ];
        let nodes = project(&[], &[], &containers);
        assert_eq!(nodes.len(), 2, "one row per host");
        // BTreeMap key order → sorted by host.
        assert_eq!(nodes[0].host, "node-a");
        assert_eq!(nodes[1].host, "node-b");
        assert!(nodes[0].containers_seen);
        assert_eq!(nodes[0].containers.len(), 1);
        assert_eq!(nodes[0].containers[0].name, "db");
        assert_eq!(nodes[0].containers[0].image, "postgres:16");
        assert_eq!(nodes[0].containers[0].state, "shut off");
    }

    #[test]
    fn project_latest_containers_win_per_host() {
        let containers = vec![
            container_body("node-a", &[("web", "nginx", "running")], 5),
            container_body(
                "node-a",
                &[("web", "nginx", "exited"), ("db", "pg", "running")],
                9,
            ),
        ];
        let nodes = project(&[], &[], &containers);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].containers.len(), 2, "the newer roster wins");
        assert_eq!(nodes[0].containers_at_ms, 9);
    }

    #[test]
    fn project_container_roster_seen_distinguishes_empty_from_unreported() {
        // A host with health but no container report → containers_seen false, so the
        // view can honestly say "not yet reported" rather than "no containers".
        let nodes = project(&[health_body("node-a", true, 1)], &[], &[]);
        assert_eq!(nodes.len(), 1);
        assert!(!nodes[0].containers_seen);
        assert!(nodes[0].containers.is_empty());

        // An explicitly empty roster → seen true, still no containers.
        let empty = container_body("node-a", &[], 3);
        let nodes = project(&[], &[], &[empty]);
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].containers_seen);
        assert!(nodes[0].containers.is_empty());
    }

    #[test]
    fn project_skips_malformed_container_bodies() {
        let containers = vec![
            "not json".to_string(),
            "{}".to_string(), // missing required fields
            container_body("node-a", &[("web", "nginx", "running")], 1),
        ];
        let nodes = project(&[], &[], &containers);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].host, "node-a");
        assert_eq!(nodes[0].containers.len(), 1);
        assert_eq!(nodes[0].containers[0].name, "web");
    }

    #[test]
    fn health_summary_tone_and_text() {
        let ok = serde_json::from_str::<KvmHealth>(&health_body("n", true, 1));
        assert!(ok.is_ok());
        if let Ok(ok) = ok {
            let (c, t) = health_summary(&ok);
            assert_eq!(c, Style::OK);
            assert_eq!(t, "all 2 KVM services up");
        }

        let deg = serde_json::from_str::<KvmHealth>(&health_body("n", false, 1));
        assert!(deg.is_ok());
        if let Ok(deg) = deg {
            let (c, t) = health_summary(&deg);
            assert_eq!(c, Style::DANGER);
            assert_eq!(t, "1/2 up (1 down)");
        }
    }

    #[test]
    fn create_action_is_rejected_in_fleet_and_redirects_to_workloads() {
        let mut error = None;
        publish(
            Some(Path::new("/missing-bus")),
            &mut error,
            &Lifecycle::Create,
        );
        assert!(error.is_some_and(|message| message.contains("Workloads")));
    }

    #[test]
    fn start_and_stop_actions_are_host_targeted_typed_operations() {
        let start = crate::workload_api::request(
            "vm:node-a:web1",
            "node-a",
            WorkloadBackend::LibvirtVirtqemud,
            WorkloadProfile::Small.resources(),
            WorkloadOperationAction::Start,
            None,
            0,
            unix_millis(),
        )
        .expect("start request");
        assert_eq!(start.target_node, "node-a");
        assert_eq!(start.workload_id.as_str(), "vm:node-a:web1");
        assert_eq!(start.action, WorkloadOperationAction::Start);

        let stop = crate::workload_api::request(
            "vm:node-b:db1",
            "node-b",
            WorkloadBackend::LibvirtVirtqemud,
            WorkloadProfile::Small.resources(),
            WorkloadOperationAction::Stop,
            None,
            0,
            unix_millis(),
        )
        .expect("stop request");
        assert_eq!(stop.target_node, "node-b");
        assert_eq!(stop.workload_id.as_str(), "vm:node-b:db1");
        assert_eq!(stop.action, WorkloadOperationAction::Stop);
    }

    #[test]
    fn create_form_validates_fields() {
        let mut f = CreateForm {
            name: "web1".to_string(),
            vcpus: "2".to_string(),
            ram_mb: "2048".to_string(),
            disk_gb: "20".to_string(),
            error: None,
        };
        let spec = f.to_spec();
        assert!(spec.is_ok(), "a fully-specified form parses");
        if let Ok(spec) = spec {
            assert_eq!(spec.name, "web1");
            assert_eq!(spec.vcpus, 2);
            assert_eq!(spec.ram_mb, 2048);
            assert_eq!(spec.disk_gb, 20);
        }

        // Blank name → error.
        f.name = "  ".to_string();
        assert!(f.to_spec().is_err());
        f.name = "web1".to_string();

        // Non-numeric / zero vCPUs → error.
        f.vcpus = "lots".to_string();
        assert!(f.to_spec().is_err());
        f.vcpus = "0".to_string();
        assert!(f.to_spec().is_err());
    }

    #[test]
    fn publish_without_a_bus_root_records_an_error() {
        let mut err = None;
        publish(
            None,
            &mut err,
            &Lifecycle::Start {
                host: "n".to_string(),
                name: "v".to_string(),
            },
        );
        assert!(err.is_some(), "a missing bus dir is surfaced, not panicked");
    }

    #[test]
    fn publish_mints_an_exact_body_bound_direct_libvirt_capability() {
        let tmp = tempfile::tempdir().unwrap();
        let mut error = None;
        publish(
            Some(tmp.path()),
            &mut error,
            &Lifecycle::Stop {
                host: "node-a".to_string(),
                name: "web1".to_string(),
                force: false,
            },
        );
        assert!(error.is_none(), "{error:?}");
        let persist = Persist::open(tmp.path().to_path_buf()).unwrap();
        let messages = persist.list_since(WORKLOAD_OPERATION_TOPIC, None).unwrap();
        assert_eq!(messages.len(), 1);
        let body = messages[0].body.as_deref().unwrap();
        let request =
            mackes_mesh_types::workloads::WorkloadOperationRequest::from_json(body, unix_millis())
                .expect("typed request");
        assert_eq!(request.target_node, "node-a");
        assert_eq!(request.workload_id.as_str(), "vm:node-a:web1");
        assert_eq!(request.action, WorkloadOperationAction::Stop);
    }
}
