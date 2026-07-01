//! Fleet · Datacenter — live per-node KVM reality (MV-6).
//!
//! Wires the Workbench Fleet plane to the two Bus topics the `mackesd` virt
//! workers publish, and drives VM lifecycle back onto the Bus:
//!
//! * `event/kvm/services` (MV-2 `kvm_health`) — each node's KVM service-health
//!   summary (libvirtd / podman / … up or down).
//! * `event/vm/instances` (MV-3 `vm_lifecycle`) — each node's VM roster.
//! * `event/podman/containers` (MV-4 `container`) — each node's Podman container
//!   roster (MV-6b).
//! * `action/vm/lifecycle` (MV-3) — create / start / stop, **host-targeted** so a
//!   request can only ever act on the one node it names (the worker drops any
//!   request that doesn't `targets()` its own id; an empty host never matches).
//!
//! The payloads are a JSON boundary: we mirror them with **local** serde structs
//! (field shapes read from `mackesd`'s `kvm_health.rs` / `vm_lifecycle.rs` /
//! `container.rs`) rather than depending on the daemon crate — the shell stays in
//! the desktop-shell tier and only leans inward on `mde-bus` (§6).
//!
//! The container roster is rendered **read-only** (name / image / state) beside the
//! VM roster: it completes MV-6's "VMs *and* containers" surface. Container
//! run/stop lifecycle-drive (publishing `action/container/lifecycle`) is a
//! deliberate follow-up — the container worker has no "start an existing container"
//! verb to mirror the VM Start/Stop toggle, so the read-only roster lands cleanly
//! first rather than half-wiring a drive.
//!
//! `project` is pure (no Bus, no GPU) and unit-tested directly; the only IO is
//! `poll` (a cheap local `Persist` read) and `publish` (a `Persist` write — the
//! same persist-first path `mde-bus publish` takes, so the request is recorded
//! locally and replicated to the target node by the Bus).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mde_egui::egui::{self, Color32, RichText};
use mde_egui::Style;
use serde::{Deserialize, Serialize};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

/// Per-node KVM service-health summary topic (MV-2 `kvm_health`).
const SERVICES_TOPIC: &str = "event/kvm/services";
/// Per-node VM roster topic (MV-3 `vm_lifecycle`).
const INSTANCES_TOPIC: &str = "event/vm/instances";
/// Per-node Podman container roster topic (MV-4 `container`, read by MV-6b).
const CONTAINERS_TOPIC: &str = "event/podman/containers";
/// VM lifecycle request topic (MV-3). Flat — per-node targeting is the request's
/// `host` field, never the topic.
const ACTION_TOPIC: &str = "action/vm/lifecycle";

/// Poll cadence for the two live topics — a node's health flip or a new VM
/// surfaces within this window. Matches the panel shell's 5 s refresh; the read
/// is a cheap local `SQLite` scan so the cadence can stay tight.
const REFRESH: Duration = Duration::from_secs(5);

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

/// One VM row — mirrors `vm_lifecycle::Instance` (the libvirt numeric `id` on the
/// wire is not rendered, so it's omitted; serde drops it).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Instance {
    /// Domain name — the lifecycle key.
    name: String,
    /// Raw libvirt state string (`running`, `shut off`, `paused`, …).
    state: String,
}

/// Whole-node VM roster — mirrors `vm_lifecycle::InstanceReport`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct InstanceReport {
    /// Publishing node id.
    host: String,
    /// The node's VMs in `virsh list --all` order.
    instances: Vec<Instance>,
    /// Publish time (ms since the Unix epoch) — the latest-wins fold key.
    published_at_ms: u64,
}

/// One container row — mirrors `container::Container` (the podman `id` on the wire
/// is not rendered, so it's omitted; serde drops it).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Container {
    /// Container name (first of podman's `Names`) — the roster key.
    name: String,
    /// Image reference (`docker.io/library/nginx:latest`, `postgres:16`, …).
    image: String,
    /// Raw podman state string (`running`, `exited`, `created`, `paused`, …).
    state: String,
}

/// Whole-node container roster — mirrors `container::ContainerReport`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct ContainerReport {
    /// Publishing node id.
    host: String,
    /// The node's containers in `podman ps --all` order.
    containers: Vec<Container>,
    /// Publish time (ms since the Unix epoch) — the latest-wins fold key.
    published_at_ms: u64,
}

// ──────────────────────────── projected view ────────────────────────────

/// One node's live datacenter reality, folded from the latest health + roster
/// messages seen for that host.
#[derive(Debug, Clone)]
struct NodeView {
    /// Node id (the Bus `host`).
    host: String,
    /// Latest KVM health summary, once any has arrived.
    health: Option<KvmHealth>,
    /// Latest VM roster (also empty for a genuinely empty node — see `roster_seen`).
    instances: Vec<Instance>,
    /// `true` once an `event/vm/instances` report has been seen for this host —
    /// distinguishes "no VMs defined" from "roster not yet reported".
    roster_seen: bool,
    /// Publish time of the roster currently held (latest-wins fold key).
    roster_at_ms: u64,
    /// Latest container roster (also empty for a genuinely empty node — see
    /// `containers_seen`).
    containers: Vec<Container>,
    /// `true` once an `event/podman/containers` report has been seen for this host
    /// — distinguishes "no containers" from "container roster not yet reported".
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

/// Fold raw topic bodies into a sorted-by-host per-node view. Latest message wins
/// per host (health, VM roster + container roster each tracked independently by
/// their own `published_at_ms`), so a growing topic collapses to one row per node.
/// Pure — no Bus, no GPU.
fn project(
    health_bodies: &[String],
    instance_bodies: &[String],
    container_bodies: &[String],
) -> Vec<NodeView> {
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

    for body in instance_bodies {
        let Ok(r) = serde_json::from_str::<InstanceReport>(body) else {
            continue;
        };
        let entry = nodes
            .entry(r.host.clone())
            .or_insert_with(|| NodeView::new(&r.host));
        if !entry.roster_seen || r.published_at_ms >= entry.roster_at_ms {
            entry.roster_at_ms = r.published_at_ms;
            entry.instances = r.instances;
            entry.roster_seen = true;
        }
    }

    for body in container_bodies {
        let Ok(r) = serde_json::from_str::<ContainerReport>(body) else {
            continue;
        };
        let entry = nodes
            .entry(r.host.clone())
            .or_insert_with(|| NodeView::new(&r.host));
        if !entry.containers_seen || r.published_at_ms >= entry.containers_at_ms {
            entry.containers_at_ms = r.published_at_ms;
            entry.containers = r.containers;
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

/// A VM spec for a create request — the `vm_lifecycle::VmSpec` fields the
/// worker's `parse_action` reads. The worker defaults the optional `image_path`
/// / `network`, so a blank-disk VM only needs these four.
#[derive(Debug, Serialize)]
struct VmSpec {
    name: String,
    vcpus: u32,
    ram_mb: u64,
    disk_gb: u64,
}

/// A lifecycle request published to `action/vm/lifecycle` — internally tagged by
/// `op` exactly like the worker's `LifecycleAction`
/// (`#[serde(tag = "op", rename_all = "snake_case")]`), so the worker's
/// `parse_action` accepts it verbatim. `host` is always a concrete node id.
#[derive(Debug, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Lifecycle {
    /// Define a new VM (leaves it shut off).
    Create { host: String, spec: VmSpec },
    /// Boot a defined VM.
    Start { host: String, name: String },
    /// Stop a running VM (graceful unless `force`).
    Stop {
        host: String,
        name: String,
        force: bool,
    },
}

impl Lifecycle {
    /// Serialize to the request body. A fixed, derive-backed shape → serialization
    /// cannot realistically fail; an empty body (never produced here) would simply
    /// be rejected by the worker's parser rather than acted on.
    fn to_body(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Publish a lifecycle request to `action/vm/lifecycle` via the persist-first
/// path (`mde-bus publish`'s own path): the write is recorded locally and the Bus
/// replicates it to the target node. Records any failure in `last_error` — never
/// panics.
fn publish(bus_root: Option<&Path>, last_error: &mut Option<String>, action: &Lifecycle) {
    let Some(root) = bus_root else {
        *last_error = Some("No mesh Bus directory — VM actions unavailable.".to_string());
        return;
    };
    let body = action.to_body();
    match Persist::open(root.to_path_buf())
        .and_then(|p| p.write(ACTION_TOPIC, Priority::Default, None, Some(&body)))
    {
        Ok(_) => *last_error = None,
        Err(e) => *last_error = Some(format!("Couldn't publish VM action: {e}")),
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
    /// The host whose inline "New VM" create form is open, if any.
    create_for: Option<String>,
    /// The (single, one-open-at-a-time) create form's fields.
    form: CreateForm,
    /// The last lifecycle-publish error, surfaced inline.
    last_error: Option<String>,
    /// When the Bus was last polled (drives the fixed cadence).
    last_poll: Option<Instant>,
}

impl Default for DatacenterState {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            nodes: Vec::new(),
            create_for: None,
            form: CreateForm::default(),
            last_error: None,
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

    /// Read both topics and re-project. Split from the cadence gate so the pure
    /// projection stays testable; a missing dir / unreadable topic yields an empty
    /// or last-known projection, never a panic.
    fn refresh(&mut self) {
        let Some(root) = self.bus_root.clone() else {
            self.nodes = Vec::new();
            return;
        };
        let Ok(persist) = Persist::open(root) else {
            // Keep the last-known projection on a transient open failure.
            return;
        };
        let health = read_bodies(&persist, SERVICES_TOPIC);
        let instances = read_bodies(&persist, INSTANCES_TOPIC);
        let containers = read_bodies(&persist, CONTAINERS_TOPIC);
        self.nodes = project(&health, &instances, &containers);
    }

    /// Render the Fleet plane's live datacenter content: per-node KVM
    /// service-health rows + VM roster, with host-targeted create/start/stop
    /// controls. Shows an honest loading state before the first Bus message.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        // Disjoint field borrows so the render closures can hold `&nodes`
        // (shared) and `&mut form` / `&mut create_for` at once (egui idiom).
        let Self {
            bus_root,
            nodes,
            create_for,
            form,
            last_error,
            ..
        } = self;

        if let Some(err) = last_error.as_deref() {
            ui.colored_label(Style::DANGER, err);
            ui.add_space(Style::SP_S);
        }

        if nodes.is_empty() {
            ui.add_space(Style::SP_S);
            ui.colored_label(Style::TEXT_DIM, "Waiting for KVM host health…");
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new(
                    "Each mesh node publishes its libvirt/Podman stack health, VM roster, \
                     and container roster to the Bus.",
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
                    ui.group(|ui| {
                        show_node(ui, node, create_for, form, &mut pending);
                    });
                    ui.add_space(Style::SP_S);
                }
                // MV-6b lands the container roster read-only; container run/stop
                // lifecycle-drive (action/container/lifecycle) is a follow-up.
                ui.colored_label(
                    Style::TEXT_DIM,
                    RichText::new(
                        "Container rows are read-only — run/stop lifecycle-drive is a follow-up.",
                    )
                    .size(Style::SMALL),
                );
            });

        if let Some(action) = pending {
            publish(bus_root.as_deref(), last_error, &action);
        }
    }
}

/// Read the JSON bodies of every retained message on `topic`, oldest first.
fn read_bodies(persist: &Persist, topic: &str) -> Vec<String> {
    persist
        .list_since(topic, None)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|m| m.body)
        .collect()
}

// ─────────────────────────── shared VM inventory (E12-5b) ───────────────────────────
// The E12-5b remote-desktop picker (`crate::discovery`) reuses this crate's live
// `event/vm/instances` roster rather than opening a second VM source — one
// inventory, projected once here and flattened to per-VM rows.

/// One remote-desktop-connectable VM, flattened from the Fleet [`project`]ion:
/// which `host` (peer) serves a VM of this `name` in this raw libvirt `state`.
/// Reused by [`crate::discovery`] so the picker lists the same VMs the Datacenter
/// view renders — no parallel inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VmRow {
    /// The peer serving the VM (the Bus `host`).
    pub host: String,
    /// The libvirt domain name.
    pub name: String,
    /// The raw libvirt state string (`running`, `shut off`, …).
    pub state: String,
}

/// Flatten the per-node projection to one row per VM (peer × VM), preserving the
/// host-sorted, `virsh list`-order sequence. Pure — the testable core of
/// [`read_inventory`].
fn flatten_inventory(nodes: Vec<NodeView>) -> Vec<VmRow> {
    nodes
        .into_iter()
        .flat_map(|node| {
            let host = node.host;
            node.instances.into_iter().map(move |inst| VmRow {
                host: host.clone(),
                name: inst.name,
                state: inst.state,
            })
        })
        .collect()
}

/// Read the mesh-wide VM inventory off the Bus and flatten it to per-VM rows — the
/// same `event/vm/instances` roster [`DatacenterState::refresh`] projects, lifted
/// so the E12-5b discovery picker reuses one inventory. A missing / unreadable Bus
/// dir yields an empty inventory (never a panic).
pub(crate) fn read_inventory(bus_root: Option<&Path>) -> Vec<VmRow> {
    let Some(root) = bus_root else {
        return Vec::new();
    };
    let Ok(persist) = Persist::open(root.to_path_buf()) else {
        return Vec::new();
    };
    let instances = read_bodies(&persist, INSTANCES_TOPIC);
    flatten_inventory(project(&[], &instances, &[]))
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
                ui.colored_label(
                    Style::TEXT_DIM,
                    RichText::new("KVM health not yet reported").size(Style::SMALL),
                );
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
                    ui.label(RichText::new(&svc.id).color(Style::TEXT).size(Style::SMALL))
                        .on_hover_text(format!("systemd unit: {}", svc.unit));
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
            ui.colored_label(Style::TEXT_DIM, RichText::new(msg).size(Style::SMALL));
        } else {
            for inst in &node.instances {
                ui.horizontal(|ui| {
                    show_instance_row(ui, &node.host, inst, pending);
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
            ui.colored_label(Style::TEXT_DIM, RichText::new(msg).size(Style::SMALL));
        } else {
            for c in &node.containers {
                ui.horizontal(|ui| {
                    show_container_row(ui, c);
                });
            }
        }
    });
}

/// One container roster row (read-only): a state pip + name + image + raw state.
/// Mirrors [`show_instance_row`] without the lifecycle buttons — container run/stop
/// drive is a deliberate follow-up (see the module doc).
fn show_container_row(ui: &mut egui::Ui, c: &Container) {
    let running = c.state.trim() == "running";
    let dot = if running { Style::OK } else { Style::TEXT_DIM };
    ui.label(RichText::new("\u{25CF}").color(dot).size(Style::SMALL));
    ui.add_space(Style::SP_XS);
    ui.label(RichText::new(&c.name).color(Style::TEXT).size(Style::SMALL));
    ui.add_space(Style::SP_S);
    ui.colored_label(Style::TEXT_DIM, RichText::new(&c.image).size(Style::SMALL));
    ui.add_space(Style::SP_S);
    ui.colored_label(Style::TEXT_DIM, RichText::new(&c.state).size(Style::SMALL));
}

/// One VM roster row: a state pip + name + raw state, and a Start (when not
/// running) or Stop (when running) button targeted at this node.
fn show_instance_row(
    ui: &mut egui::Ui,
    host: &str,
    inst: &Instance,
    pending: &mut Option<Lifecycle>,
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
    ui.colored_label(
        Style::TEXT_DIM,
        RichText::new(&inst.state).size(Style::SMALL),
    );
    ui.add_space(Style::SP_S);
    if running {
        if ui
            .button(RichText::new("Stop").size(Style::SMALL))
            .clicked()
        {
            *pending = Some(Lifecycle::Stop {
                host: host.to_string(),
                name: inst.name.clone(),
                force: false,
            });
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
                    *pending = Some(Lifecycle::Create {
                        host: host.to_string(),
                        spec,
                    });
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

    fn health_body(host: &str, all_healthy: bool, at: u64) -> String {
        // A minimal but faithful `event/kvm/services` body.
        format!(
            r#"{{"host":"{host}","services":[{{"id":"libvirtd","unit":"libvirtd.service","active":true}},{{"id":"podman","unit":"podman.socket","active":{}}}],"active":{},"total":2,"all_healthy":{all_healthy},"published_at_ms":{at}}}"#,
            all_healthy,
            if all_healthy { 2 } else { 1 },
        )
    }

    fn roster_body(host: &str, names_states: &[(&str, &str)], at: u64) -> String {
        let insts: Vec<String> = names_states
            .iter()
            .map(|(n, s)| format!(r#"{{"id":"-","name":"{n}","state":"{s}"}}"#))
            .collect();
        format!(
            r#"{{"host":"{host}","instances":[{}],"published_at_ms":{at}}}"#,
            insts.join(",")
        )
    }

    fn container_body(host: &str, rows: &[(&str, &str, &str)], at: u64) -> String {
        // A minimal but faithful `event/podman/containers` body — each row is
        // (name, image, state). `id` is on the wire but not rendered (serde drops it).
        let cs: Vec<String> = rows
            .iter()
            .map(|(n, img, s)| {
                format!(r#"{{"id":"-","name":"{n}","image":"{img}","state":"{s}"}}"#)
            })
            .collect();
        format!(
            r#"{{"host":"{host}","containers":[{}],"published_at_ms":{at}}}"#,
            cs.join(",")
        )
    }

    #[test]
    fn project_folds_one_row_per_host_sorted() {
        let health = vec![
            health_body("node-b", true, 1),
            health_body("node-a", false, 1),
        ];
        let instances = vec![roster_body("node-a", &[("web1", "running")], 1)];
        let containers = vec![container_body(
            "node-a",
            &[("cache", "redis:7", "running")],
            1,
        )];
        let nodes = project(&health, &instances, &containers);
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
    fn read_inventory_flattens_the_projection_to_per_vm_rows() {
        // The E12-5b picker reuses this: two hosts, three VMs → three flat rows,
        // host-sorted (node-a before node-b), `virsh` order within a host.
        let insts = vec![
            roster_body("node-b", &[("db1", "running")], 1),
            roster_body("node-a", &[("web1", "running"), ("web2", "shut off")], 1),
        ];
        let rows = flatten_inventory(project(&[], &insts, &[]));
        assert_eq!(rows.len(), 3, "one row per VM across the mesh");
        assert_eq!(rows[0].host, "node-a");
        assert_eq!(rows[0].name, "web1");
        assert_eq!(rows[0].state, "running");
        assert_eq!(rows[1].host, "node-a");
        assert_eq!(rows[1].name, "web2");
        assert_eq!(rows[1].state, "shut off");
        assert_eq!(rows[2].host, "node-b");
        assert_eq!(rows[2].name, "db1");
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
        assert_eq!(nodes[0].containers[0].state, "exited");
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
    fn create_action_serializes_to_the_worker_shape() {
        let body = Lifecycle::Create {
            host: "node-a".to_string(),
            spec: VmSpec {
                name: "web1".to_string(),
                vcpus: 2,
                ram_mb: 2048,
                disk_gb: 20,
            },
        }
        .to_body();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        assert_eq!(v["op"], "create");
        assert_eq!(v["host"], "node-a");
        assert_eq!(v["spec"]["name"], "web1");
        assert_eq!(v["spec"]["vcpus"], 2);
        assert_eq!(v["spec"]["ram_mb"], 2048);
        assert_eq!(v["spec"]["disk_gb"], 20);
    }

    #[test]
    fn start_and_stop_actions_are_host_targeted() {
        let start = Lifecycle::Start {
            host: "node-a".to_string(),
            name: "web1".to_string(),
        }
        .to_body();
        let v: serde_json::Value = serde_json::from_str(&start).unwrap_or_default();
        assert_eq!(v["op"], "start");
        assert_eq!(v["host"], "node-a");
        assert_eq!(v["name"], "web1");

        let stop = Lifecycle::Stop {
            host: "node-b".to_string(),
            name: "db1".to_string(),
            force: false,
        }
        .to_body();
        let v: serde_json::Value = serde_json::from_str(&stop).unwrap_or_default();
        assert_eq!(v["op"], "stop");
        assert_eq!(v["host"], "node-b");
        assert_eq!(v["name"], "db1");
        assert_eq!(v["force"], false);
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
}
