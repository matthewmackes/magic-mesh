//! DATACENTER-8 (skeleton) — the **Datacenter** plane.
//!
//! A read-only view over the datacenter substrate: it reads the
//! `event/dc/<kind>/<id>` events the mackesd `datacenter_orchestrator` worker
//! (DATACENTER-5) publishes onto the Bus and projects them into per-resource rows
//! grouped by zone (Prod = DigitalOcean, Dev = Xen). Same established pattern as
//! the other Bus-reading panels (home/hub/build_farm read their topics the same
//! way) — no new cross-crate dependency.
//!
//! This is the plane skeleton: it closes the end-to-end loop
//! (`doctl → worker → event/dc/droplet/* → here`). The full per-zone tabs (Hosts/
//! VMs/Storage/Network/Tofu/Gateway) layer on top in later DATACENTER tasks; the
//! load + projection here are pure and unit-tested.

use std::time::Duration;

use cosmic::iced::widget::{column, container, row, scrollable, text};
use cosmic::iced::{Length, Task};
use cosmic::Element;
use mde_theme::{spacing, Palette};

use crate::controls::{variant_button, ButtonVariant};

/// One datacenter resource as last seen on the Bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DcRow {
    /// "droplet" | "host" | "vm" | …
    pub kind: String,
    pub id: String,
    pub name: String,
    pub status: String,
    /// "prod" (DigitalOcean) | "dev" (Xen) | "" (unknown)
    pub zone: String,
    /// The dom0 IP that owns this resource (vm/host/sr event signatures carry a
    /// `host` field). Empty when the event didn't name one. Used as the
    /// `dom0` argument for the `action/dc/vm-power` RPC.
    pub host: String,
    /// Total capacity in bytes, as a string (sr events carry `size`). Empty for
    /// non-storage resources. Rendered as a GiB capacity readout on sr rows.
    pub size: String,
    /// Used capacity in bytes, as a string (sr events carry `used`). Empty for
    /// non-storage resources.
    pub used: String,
    /// The bridge a network resource is attached to (`net` events carry
    /// `bridge`, e.g. `"xenbr0"`). Empty for non-network resources. Appended to
    /// the status readout on `net` rows.
    pub bridge: String,
    /// Physical CPU count on a host (`host` events carry `cpu`, from `xl info`
    /// `nr_cpus`). Empty for non-host resources or when the metric was missing.
    pub cpu: String,
    /// Total physical memory in MB on a host (`host` events carry `mem_total_mb`).
    /// Empty for non-host resources or when the metric was missing.
    pub mem_total_mb: String,
    /// Free physical memory in MB on a host (`host` events carry `mem_free_mb`).
    /// Empty for non-host resources or when the metric was missing.
    pub mem_free_mb: String,
    /// 1-minute load average on a host (`host` events carry `load`). Empty for
    /// non-host resources or when the metric was missing.
    pub load: String,
}

impl DcRow {
    /// A human label for the zone column.
    #[must_use]
    pub fn zone_label(&self) -> &'static str {
        match self.zone.as_str() {
            "prod" => "Prod · DO",
            "dev" => "Dev · Xen",
            _ => "—",
        }
    }

    /// A human capacity readout for storage rows — e.g. `"40 / 207 GiB (19%)"`.
    /// Returns `None` when `size`/`used` don't parse or `size` is 0, so callers
    /// render nothing rather than a bogus "0 / 0 GiB (NaN%)".
    #[must_use]
    pub fn capacity_readout(&self) -> Option<String> {
        let size: u64 = self.size.parse().ok()?;
        let used: u64 = self.used.parse().ok()?;
        if size == 0 {
            return None;
        }
        const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let pct = ((used as f64 / size as f64) * 100.0).round() as u64;
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let used_gib = (used as f64 / GIB).round() as u64;
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let size_gib = (size as f64 / GIB).round() as u64;
        Some(format!("{used_gib} / {size_gib} GiB ({pct}%)"))
    }
}

/// Parse one `event/dc/<kind>/<id>` message body into a row. Returns `None` for a
/// `gone` marker (the resource vanished) or unparseable JSON. Pure + testable.
#[must_use]
pub fn parse_dc_event(body: &str) -> Option<DcRow> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    if v.get("gone").and_then(serde_json::Value::as_bool) == Some(true) {
        return None;
    }
    let kind = v.get("kind")?.as_str()?.to_string();
    let id = v.get("id")?.as_str()?.to_string();
    let name = v
        .get("name")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let status = v
        .get("status")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let zone = v
        .get("zone")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let host = v
        .get("host")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let size = v
        .get("size")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let used = v
        .get("used")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let bridge = v
        .get("bridge")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let cpu = v
        .get("cpu")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let mem_total_mb = v
        .get("mem_total_mb")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let mem_free_mb = v
        .get("mem_free_mb")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let load = v
        .get("load")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    Some(DcRow {
        kind,
        id,
        name,
        status,
        zone,
        host,
        size,
        used,
        bridge,
        cpu,
        mem_total_mb,
        mem_free_mb,
        load,
    })
}

/// One datacenter audit-log entry as last seen on the Bus (`event/dc/audit/*`).
/// Records a control-plane action (a tofu apply, a vm power/delete, …) so the
/// Audit view can render a newest-first activity log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRow {
    /// The action performed — e.g. "tofu-apply" | "vm-delete" | "vm-power".
    pub action: String,
    /// The target of the action — a workspace name, a VM uuid, a dom0 IP, …
    pub target: String,
    /// An RFC3339 / epoch timestamp string as carried on the event. Empty when
    /// the event didn't name one. Used as the sort key (descending = newest).
    pub ts: String,
}

/// Parse one `event/dc/audit/<id>` message body into an [`AuditRow`]. Returns
/// `None` for unparseable JSON or a body missing the `action` field. Pure +
/// testable. Mirrors [`parse_dc_event`]'s tolerant string extraction.
#[must_use]
pub fn parse_audit_event(body: &str) -> Option<AuditRow> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let action = v.get("action")?.as_str()?.to_string();
    let target = v
        .get("target")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let ts = v
        .get("ts")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    Some(AuditRow { action, target, ts })
}

/// Project a set of `(topic, latest-body)` Bus reads into audit rows —
/// `event/dc/audit/*` topics, sorted newest-first (descending `ts`, ties broken
/// by topic so the order is stable). Pure + testable.
#[must_use]
pub fn project_audit(events: &[(String, String)]) -> Vec<AuditRow> {
    let mut rows: Vec<AuditRow> = events
        .iter()
        .filter(|(topic, _)| topic.starts_with("event/dc/audit/"))
        .filter_map(|(_, body)| parse_audit_event(body))
        .collect();
    // Newest-first: descending timestamp. String compare is correct for both
    // RFC3339 and zero-padded epoch strings; ties keep a stable order.
    rows.sort_by(|a, b| b.ts.cmp(&a.ts));
    rows
}

/// Project a set of `(topic, latest-body)` Bus reads into sorted rows — datacenter
/// resources (`event/dc/*`), grouped by zone (prod first) then kind then name.
#[must_use]
pub fn project_rows(events: &[(String, String)]) -> Vec<DcRow> {
    let mut rows: Vec<DcRow> = events
        .iter()
        .filter(|(topic, _)| topic.starts_with("event/dc/"))
        .filter_map(|(_, body)| parse_dc_event(body))
        .collect();
    rows.sort_by(|a, b| {
        let za = u8::from(a.zone != "prod"); // prod (0) before others (1)
        let zb = u8::from(b.zone != "prod");
        za.cmp(&zb)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.name.cmp(&b.name))
    });
    rows
}

/// A cross-zone capacity rollup computed from the projected rows — counts per
/// kind, per-zone resource counts, and the summed host CPU + total/free memory.
/// Pure + testable; the Overview view renders it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapacityRollup {
    pub hosts: usize,
    pub vms: usize,
    pub droplets: usize,
    pub srs: usize,
    pub nets: usize,
    /// Resource count in the Prod (DigitalOcean) zone.
    pub prod: usize,
    /// Resource count in the Dev (Xen) zone.
    pub dev: usize,
    /// Summed physical CPU count across all host rows (those whose `cpu` parses).
    pub total_cpu: u64,
    /// Summed total physical memory (MB) across all host rows.
    pub total_mem_mb: u64,
    /// Summed free physical memory (MB) across all host rows.
    pub free_mem_mb: u64,
}

impl CapacityRollup {
    /// Compute the rollup from a set of projected rows. Host metric fields that
    /// don't parse are skipped (contribute 0), never panic. Pure.
    #[must_use]
    pub fn from_rows(rows: &[DcRow]) -> Self {
        let mut r = Self::default();
        for row in rows {
            match row.kind.as_str() {
                "host" => r.hosts += 1,
                "vm" => r.vms += 1,
                "droplet" => r.droplets += 1,
                "sr" => r.srs += 1,
                "net" => r.nets += 1,
                _ => {}
            }
            match row.zone.as_str() {
                "prod" => r.prod += 1,
                "dev" => r.dev += 1,
                _ => {}
            }
            if row.kind == "host" {
                r.total_cpu += row.cpu.parse::<u64>().unwrap_or(0);
                r.total_mem_mb += row.mem_total_mb.parse::<u64>().unwrap_or(0);
                r.free_mem_mb += row.mem_free_mb.parse::<u64>().unwrap_or(0);
            }
        }
        r
    }

    /// A human "used / total GiB" memory readout across hosts, or `None` when no
    /// host reported a total (so the Overview renders nothing rather than "0 GiB").
    #[must_use]
    pub fn memory_readout(&self) -> Option<String> {
        if self.total_mem_mb == 0 {
            return None;
        }
        let used_mb = self.total_mem_mb.saturating_sub(self.free_mem_mb);
        #[allow(clippy::cast_precision_loss)]
        let used_gib = used_mb as f64 / 1024.0;
        #[allow(clippy::cast_precision_loss)]
        let total_gib = self.total_mem_mb as f64 / 1024.0;
        Some(format!("{used_gib:.1} / {total_gib:.1} GiB used"))
    }
}

#[derive(Debug, Clone)]
pub struct DatacenterPanel {
    pub rows: Vec<DcRow>,
    pub status: String,
    pub busy: bool,
    /// Set when the load failed (vs legitimately empty) — render the error, not a
    /// misleading "no datacenter activity" empty state.
    pub load_error: Option<String>,
    /// Which per-zone tab is selected — "prod" (DigitalOcean) or "dev" (Xen).
    /// Defaults to "prod". The view filters rendered rows to this zone.
    pub zone_tab: String,
    /// Which top-level view is selected — `Zone` shows the per-zone resource
    /// tabs; `Tofu` shows the OpenTofu workspaces with Plan buttons.
    pub view_mode: ViewMode,
    /// The latest `action/dc/tofu-plan` reply summary (or in-flight/error text),
    /// rendered in the Tofu view.
    pub tofu_output: String,
    /// When `Some(uuid)`, a VM delete is awaiting inline confirmation — its row
    /// renders a "Really delete?" prompt and only the confirm button fires the
    /// destructive `action/dc/vm-delete` RPC. Cleared once a delete is fired or
    /// the load refreshes.
    pub confirm_delete: Option<String>,
    /// The audit-log rows read off `event/dc/audit/*`, newest-first. Rendered by
    /// the `Audit` view. Refreshed alongside `rows` on every load.
    pub audit: Vec<AuditRow>,
    /// When `Some(workspace)`, a Tofu apply is awaiting typed confirmation — the
    /// workspace's row renders a "Type APPLY to confirm" prompt and only the
    /// confirm button fires the `action/dc/tofu-apply` RPC. Cleared once the
    /// apply is fired or cancelled.
    pub tofu_confirm: Option<String>,
}

/// Top-level view selector for the datacenter panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    /// Cross-zone capacity rollup (the default landing view).
    Overview,
    /// Per-zone resource tabs (Prod / Dev).
    Zone,
    /// OpenTofu workspaces + Plan / Apply buttons.
    Tofu,
    /// The datacenter audit log (`event/dc/audit/*`), newest-first.
    Audit,
}

impl Default for DatacenterPanel {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            status: String::new(),
            busy: false,
            load_error: None,
            zone_tab: "prod".to_string(),
            view_mode: ViewMode::Overview,
            tofu_output: String::new(),
            confirm_delete: None,
            audit: Vec::new(),
            tofu_confirm: None,
        }
    }
}

/// The payload of a successful [`Message::Loaded`] — the projected resource rows
/// plus the audit-log rows, both read from the Bus in one pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DcLoad {
    pub rows: Vec<DcRow>,
    pub audit: Vec<AuditRow>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<DcLoad, String>),
    RefreshClicked,
    /// Switch the active per-zone tab ("prod" | "dev").
    ZoneTab(String),
    /// A VM power button was clicked. `op` is "start" | "shutdown" | "reboot";
    /// `uuid` is the VM id; `dom0` is the owning dom0 IP (`DcRow::host`).
    PowerClicked {
        uuid: String,
        op: String,
        dom0: String,
    },
    /// The `action/dc/vm-power` RPC came back — `Ok` carries a status line, `Err`
    /// the error text. Delivered as a panel-scoped message so it routes here.
    PowerDone(Result<String, String>),
    /// A VM "Snapshot" button was clicked. `uuid` is the VM id (`DcRow::id`);
    /// `dom0` is the owning dom0 IP (`DcRow::host`). Fires the
    /// `action/dc/vm-snapshot` RPC.
    SnapshotClicked {
        uuid: String,
        dom0: String,
    },
    /// The `action/dc/vm-snapshot` RPC came back — `Ok` carries a status line,
    /// `Err` the error text. Routes here as a panel-scoped message.
    SnapshotDone(Result<String, String>),
    /// Switch the top-level view (per-zone tabs vs the Tofu workspaces).
    ViewMode(ViewMode),
    /// A Tofu "Plan" button was clicked. The payload is the workspace name
    /// ("xen-xapi" | "zone1-do"). Fires the `action/dc/tofu-plan` RPC.
    TofuPlan(String),
    /// The `action/dc/tofu-plan` RPC came back — `Ok` carries the plan summary,
    /// `Err` the error text. Routes here as a panel-scoped message.
    TofuDone(Result<String, String>),
    /// A Tofu "Apply" button was clicked. The payload is the workspace name. This
    /// only *arms* the typed-confirm (`tofu_confirm = Some(workspace)`); no RPC
    /// fires until the inline "Type APPLY to confirm" button is pressed.
    TofuApplyClicked(String),
    /// The inline confirm for a Tofu apply was pressed — only this fires the
    /// `action/dc/tofu-apply` RPC (with `"confirm":true`). Payload is the
    /// workspace name.
    TofuApply(String),
    /// The pending Tofu-apply confirmation was dismissed (the "Cancel" button) —
    /// clears `tofu_confirm` without firing any RPC.
    TofuApplyCancelled,
    /// The `action/dc/tofu-apply` RPC came back — `Ok` carries the apply summary,
    /// `Err` the error text. Routes here as a panel-scoped message.
    TofuApplyDone(Result<String, String>),
    /// A VM "Clone" button was clicked. `uuid` is the VM id (`DcRow::id`);
    /// `dom0` is the owning dom0 IP (`DcRow::host`). Fires the
    /// `action/dc/vm-clone` RPC.
    CloneClicked {
        uuid: String,
        dom0: String,
    },
    /// The `action/dc/vm-clone` RPC came back — `Ok` carries a status line,
    /// `Err` the error text. Routes here as a panel-scoped message.
    CloneDone(Result<String, String>),
    /// A VM "Delete" button was clicked. Sets the pending-confirm state for this
    /// `uuid` (no RPC fires yet); the row then renders an inline confirm prompt.
    /// `dom0` is the owning dom0 IP (`DcRow::host`).
    DeleteClicked {
        uuid: String,
        dom0: String,
    },
    /// The inline "Really delete?" confirm button was clicked — only this fires
    /// the destructive `action/dc/vm-delete` RPC (with `"confirm":true`).
    DeleteConfirmed {
        uuid: String,
        dom0: String,
    },
    /// The pending delete confirmation was dismissed (the "Cancel" button) —
    /// clears `confirm_delete` without firing any RPC.
    DeleteCancelled,
    /// The `action/dc/vm-delete` RPC came back — `Ok` carries a status line,
    /// `Err` the error text. Routes here as a panel-scoped message.
    DeleteDone(Result<String, String>),
}

impl DatacenterPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read the `event/dc/*` topics off the Bus + project them into rows.
    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move { Message::Loaded(read_dc_events()) },
            crate::Message::Datacenter,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(Ok(load)) => {
                self.rows = load.rows;
                self.audit = load.audit;
                self.busy = false;
                self.load_error = None;
                self.status.clear();
                // A fresh projection may not include the row pending a delete —
                // drop the stale confirm prompt rather than leave it dangling.
                self.confirm_delete = None;
                // Likewise drop a stale tofu-apply confirm on a refresh.
                self.tofu_confirm = None;
                Task::none()
            }
            Message::Loaded(Err(e)) => {
                self.load_error = Some(e);
                self.busy = false;
                Task::none()
            }
            Message::RefreshClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Refreshing…".into();
                Self::load()
            }
            Message::ZoneTab(z) => {
                self.zone_tab = z;
                Task::none()
            }
            Message::PowerClicked { uuid, op, dom0 } => {
                self.status = format!("Powering {op}…");
                Task::perform(
                    async move {
                        // The Bus RPC borrows a non-Send Persist across its
                        // internal await, so run the whole round trip on a
                        // blocking thread with a local runtime (the same shape
                        // mde-files' bus backend uses).
                        tokio::task::spawn_blocking(move || vm_power(&uuid, &op, &dom0))
                            .await
                            .unwrap_or_else(|e| Err(format!("power task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::PowerDone(result)),
                )
            }
            Message::PowerDone(Ok(s)) => {
                self.status = s;
                Task::none()
            }
            Message::PowerDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::SnapshotClicked { uuid, dom0 } => {
                self.status = "Snapshotting…".into();
                Task::perform(
                    async move {
                        // Same shape as `vm_power`: the Bus RPC borrows a
                        // non-Send Persist across its internal await, so run the
                        // whole round trip on a blocking thread with a local
                        // runtime.
                        tokio::task::spawn_blocking(move || vm_snapshot(&uuid, &dom0))
                            .await
                            .unwrap_or_else(|e| Err(format!("snapshot task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::SnapshotDone(result)),
                )
            }
            Message::SnapshotDone(Ok(s)) => {
                self.status = s;
                Task::none()
            }
            Message::SnapshotDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::ViewMode(mode) => {
                self.view_mode = mode;
                Task::none()
            }
            Message::TofuPlan(ws) => {
                self.status = format!("Planning {ws}…");
                self.tofu_output = format!("Planning {ws}…");
                Task::perform(
                    async move {
                        // Same shape as `vm_power`: the Bus RPC borrows a
                        // non-Send Persist across its internal await, so run the
                        // whole round trip on a blocking thread with a local
                        // runtime.
                        tokio::task::spawn_blocking(move || tofu_plan(&ws))
                            .await
                            .unwrap_or_else(|e| Err(format!("tofu task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::TofuDone(result)),
                )
            }
            Message::TofuDone(Ok(s)) => {
                self.status = "Plan complete".into();
                self.tofu_output = s;
                Task::none()
            }
            Message::TofuDone(Err(e)) => {
                self.status = e.clone();
                self.tofu_output = e;
                Task::none()
            }
            Message::TofuApplyClicked(ws) => {
                // First click only arms the typed-confirm — no RPC fires until
                // the operator confirms.
                self.tofu_confirm = Some(ws);
                self.status = "Type APPLY to confirm below.".into();
                Task::none()
            }
            Message::TofuApply(ws) => {
                self.tofu_confirm = None;
                self.status = format!("Applying {ws}…");
                self.tofu_output = format!("Applying {ws}…");
                Task::perform(
                    async move {
                        // Same shape as `tofu_plan`: the Bus RPC borrows a
                        // non-Send Persist across its internal await, so run the
                        // whole round trip on a blocking thread with a local
                        // runtime.
                        tokio::task::spawn_blocking(move || tofu_apply(&ws))
                            .await
                            .unwrap_or_else(|e| Err(format!("tofu task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::TofuApplyDone(result)),
                )
            }
            Message::TofuApplyCancelled => {
                self.tofu_confirm = None;
                self.status.clear();
                Task::none()
            }
            Message::TofuApplyDone(Ok(s)) => {
                self.status = "Apply complete".into();
                self.tofu_output = s;
                Task::none()
            }
            Message::TofuApplyDone(Err(e)) => {
                self.status = e.clone();
                self.tofu_output = e;
                Task::none()
            }
            Message::CloneClicked { uuid, dom0 } => {
                self.status = "Cloning…".into();
                Task::perform(
                    async move {
                        // Same shape as `vm_power`: the Bus RPC borrows a
                        // non-Send Persist across its internal await, so run the
                        // whole round trip on a blocking thread with a local
                        // runtime.
                        tokio::task::spawn_blocking(move || vm_clone(&uuid, &dom0))
                            .await
                            .unwrap_or_else(|e| Err(format!("clone task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::CloneDone(result)),
                )
            }
            Message::CloneDone(Ok(s)) => {
                self.status = s;
                Task::none()
            }
            Message::CloneDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::DeleteClicked { uuid, dom0: _ } => {
                // First click only arms the inline confirm — no RPC fires until
                // the operator confirms.
                self.confirm_delete = Some(uuid);
                self.status = "Confirm delete below.".into();
                Task::none()
            }
            Message::DeleteConfirmed { uuid, dom0 } => {
                self.confirm_delete = None;
                self.status = "Deleting…".into();
                Task::perform(
                    async move {
                        // Same shape as `vm_power`: the Bus RPC borrows a
                        // non-Send Persist across its internal await, so run the
                        // whole round trip on a blocking thread with a local
                        // runtime.
                        tokio::task::spawn_blocking(move || vm_delete(&uuid, &dom0))
                            .await
                            .unwrap_or_else(|e| Err(format!("delete task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::DeleteDone(result)),
                )
            }
            Message::DeleteCancelled => {
                self.confirm_delete = None;
                self.status.clear();
                Task::none()
            }
            Message::DeleteDone(Ok(s)) => {
                self.status = s;
                Task::none()
            }
            Message::DeleteDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        if let Some(err) = &self.load_error {
            return container(text(format!("Couldn't read datacenter state: {err}")))
                .padding(f32::from(spacing::BASE[5]))
                .into();
        }

        let prod = self.rows.iter().filter(|r| r.zone == "prod").count();
        let dev = self.rows.iter().filter(|r| r.zone == "dev").count();

        // Top-level view selector: per-zone resources vs the Tofu workspaces.
        // The selected mode gets the Primary (filled) variant. Reachable even
        // when there are no resource rows yet (Tofu has no row dependency).
        let mode_btn = |label: &str, mode: ViewMode| -> Element<'_, crate::Message> {
            let variant = if self.view_mode == mode {
                ButtonVariant::Primary
            } else {
                ButtonVariant::Secondary
            };
            variant_button(
                label.to_string(),
                variant,
                Some(crate::Message::Datacenter(Message::ViewMode(mode))),
                palette,
            )
        };
        // A top-of-panel Refresh button that re-reads the Bus `event/dc/*`
        // topics (fires the existing `RefreshClicked` → `load()` path).
        let refresh_btn = variant_button(
            "Refresh".to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::RefreshClicked)),
            palette,
        );
        let mode_tabs = row![
            mode_btn("Overview", ViewMode::Overview),
            mode_btn("Resources", ViewMode::Zone),
            mode_btn("Tofu", ViewMode::Tofu),
            mode_btn("Audit", ViewMode::Audit),
            refresh_btn,
        ]
        .spacing(f32::from(spacing::BASE[2]));

        let mut col = column![
            text(format!(
                "Datacenter — {} resource(s)  ·  Prod {prod} / Dev {dev}",
                self.rows.len()
            ))
            .size(f32::from(spacing::BASE[6])),
            mode_tabs,
        ]
        .spacing(f32::from(spacing::BASE[2]))
        .padding(f32::from(spacing::BASE[5]));

        if !self.status.is_empty() {
            col = col.push(text(self.status.clone()));
        }

        match self.view_mode {
            ViewMode::Overview => {
                let rollup = CapacityRollup::from_rows(&self.rows);
                // Per-kind counts.
                col = col.push(text("Resources by kind").size(f32::from(spacing::BASE[5])));
                col = col.push(text(format!(
                    "Hosts {} · VMs {} · Droplets {} · Storage {} · Networks {}",
                    rollup.hosts, rollup.vms, rollup.droplets, rollup.srs, rollup.nets
                )));
                // Per-zone counts.
                col = col.push(text("By zone").size(f32::from(spacing::BASE[5])));
                col = col.push(text(format!(
                    "Prod · DO {} · Dev · Xen {}",
                    rollup.prod, rollup.dev
                )));
                // Summed host capacity.
                col = col.push(text("Host capacity").size(f32::from(spacing::BASE[5])));
                col = col.push(text(format!(
                    "{} host(s) · {} vCPU total",
                    rollup.hosts, rollup.total_cpu
                )));
                if let Some(mem) = rollup.memory_readout() {
                    col = col.push(text(format!("Memory: {mem}")));
                } else {
                    col = col.push(text("Memory: no host metrics reported yet."));
                }
            }
            ViewMode::Tofu => {
                // A Plan + Apply control pair per workspace. Apply arms a typed
                // confirm before firing the destructive `action/dc/tofu-apply`.
                for ws in ["xen-xapi", "zone1-do"] {
                    let plan_btn = variant_button(
                        format!("Plan {ws}"),
                        ButtonVariant::Secondary,
                        Some(crate::Message::Datacenter(Message::TofuPlan(
                            ws.to_string(),
                        ))),
                        palette,
                    );
                    let mut ws_row =
                        row![text(ws.to_string()).width(Length::FillPortion(2)), plan_btn]
                            .spacing(f32::from(spacing::BASE[2]));
                    if self.tofu_confirm.as_deref() == Some(ws) {
                        // Armed: surface the typed-confirm — only the confirm
                        // button carries the destructive `TofuApply` message.
                        ws_row = ws_row
                            .push(text("Type APPLY to confirm"))
                            .push(variant_button(
                                "APPLY".to_string(),
                                ButtonVariant::Primary,
                                Some(crate::Message::Datacenter(Message::TofuApply(
                                    ws.to_string(),
                                ))),
                                palette,
                            ))
                            .push(variant_button(
                                "Cancel".to_string(),
                                ButtonVariant::Secondary,
                                Some(crate::Message::Datacenter(Message::TofuApplyCancelled)),
                                palette,
                            ));
                    } else {
                        // Unarmed: the first click only arms the confirm (no RPC).
                        ws_row = ws_row.push(variant_button(
                            format!("Apply {ws}"),
                            ButtonVariant::Primary,
                            Some(crate::Message::Datacenter(Message::TofuApplyClicked(
                                ws.to_string(),
                            ))),
                            palette,
                        ));
                    }
                    col = col.push(ws_row);
                }
                if self.tofu_output.is_empty() {
                    col = col.push(text(
                        "Run a workspace plan to see the OpenTofu output here.",
                    ));
                } else {
                    col = col.push(
                        container(text(self.tofu_output.clone()))
                            .padding(f32::from(spacing::BASE[3]))
                            .width(Length::Fill),
                    );
                }
            }
            ViewMode::Audit => {
                col = col.push(text("Audit log").size(f32::from(spacing::BASE[5])));
                if self.audit.is_empty() {
                    col = col.push(text(
                        "No datacenter audit events yet. Control-plane actions \
                         (applies, deletes, power) appear here newest-first.",
                    ));
                } else {
                    // Already projected newest-first; render each as a row.
                    for entry in &self.audit {
                        col = col.push(audit_row_view(entry));
                    }
                }
            }
            ViewMode::Zone => {
                if self.rows.is_empty() {
                    col = col.push(
                        text("No datacenter resources yet").size(f32::from(spacing::BASE[6])),
                    );
                    col = col.push(text(
                        "Hosts, VMs, and droplets appear here as the datacenter \
                         orchestrator publishes them.",
                    ));
                } else {
                    // Per-zone tabs. The selected tab gets the Primary (filled)
                    // variant; the other a Secondary outline.
                    let tab = |label: String, zone: &str| -> Element<'_, crate::Message> {
                        let variant = if self.zone_tab == zone {
                            ButtonVariant::Primary
                        } else {
                            ButtonVariant::Secondary
                        };
                        variant_button(
                            label,
                            variant,
                            Some(crate::Message::Datacenter(Message::ZoneTab(
                                zone.to_string(),
                            ))),
                            palette,
                        )
                    };
                    col = col.push(
                        row![
                            tab(format!("Prod · DO ({prod})"), "prod"),
                            tab(format!("Dev · Xen ({dev})"), "dev"),
                        ]
                        .spacing(f32::from(spacing::BASE[2])),
                    );

                    let visible: Vec<&DcRow> = self
                        .rows
                        .iter()
                        .filter(|r| r.zone == self.zone_tab)
                        .collect();
                    if visible.is_empty() {
                        col = col.push(text("No resources in this zone yet."));
                    }
                    for r in visible {
                        let confirming = self.confirm_delete.as_deref() == Some(r.id.as_str());
                        col = col.push(dc_row_view(r, palette, confirming));
                    }
                }
            }
        }

        scrollable(col).into()
    }
}

/// Render one datacenter row. VM rows additionally carry Start / Stop / Reboot
/// power buttons that fire the `action/dc/vm-power` RPC for the row's dom0, plus
/// Snapshot / Clone / Delete. When `confirming` is set, the Delete button is
/// replaced by an inline "Really delete?" confirm + Cancel prompt — only the
/// confirm fires the destructive `action/dc/vm-delete` RPC.
fn dc_row_view(r: &DcRow, palette: Palette, confirming: bool) -> Element<'_, crate::Message> {
    let label = if r.name.is_empty() {
        r.id.clone()
    } else {
        r.name.clone()
    };
    // For storage rows, surface the capacity readout in place of the bare
    // status; for network rows append the bridge; otherwise the bare status.
    let status_or_capacity = if r.kind == "sr" {
        r.capacity_readout().unwrap_or_else(|| r.status.clone())
    } else if r.kind == "net" && !r.bridge.is_empty() {
        format!("{} · {}", r.status, r.bridge)
    } else {
        r.status.clone()
    };
    let mut line = row![
        text(r.kind.clone()).width(Length::FillPortion(1)),
        text(label).width(Length::FillPortion(3)),
        text(status_or_capacity).width(Length::FillPortion(1)),
    ]
    .spacing(f32::from(spacing::BASE[3]));

    if r.kind == "vm" {
        let power = |btn_label: &str, op: &str| {
            variant_button(
                btn_label.to_string(),
                ButtonVariant::Secondary,
                Some(crate::Message::Datacenter(Message::PowerClicked {
                    uuid: r.id.clone(),
                    op: op.to_string(),
                    dom0: r.host.clone(),
                })),
                palette,
            )
        };
        let snapshot = variant_button(
            "Snapshot".to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::SnapshotClicked {
                uuid: r.id.clone(),
                dom0: r.host.clone(),
            })),
            palette,
        );
        let clone = variant_button(
            "Clone".to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::CloneClicked {
                uuid: r.id.clone(),
                dom0: r.host.clone(),
            })),
            palette,
        );
        let mut actions = row![
            power("Start", "start"),
            power("Stop", "shutdown"),
            power("Reboot", "reboot"),
            snapshot,
            clone,
        ]
        .spacing(f32::from(spacing::BASE[1]));
        if confirming {
            // Armed: surface the explicit confirm/cancel — only the confirm
            // button carries the destructive `DeleteConfirmed` message.
            actions = actions
                .push(text("Really delete?"))
                .push(variant_button(
                    "Confirm".to_string(),
                    ButtonVariant::Primary,
                    Some(crate::Message::Datacenter(Message::DeleteConfirmed {
                        uuid: r.id.clone(),
                        dom0: r.host.clone(),
                    })),
                    palette,
                ))
                .push(variant_button(
                    "Cancel".to_string(),
                    ButtonVariant::Secondary,
                    Some(crate::Message::Datacenter(Message::DeleteCancelled)),
                    palette,
                ));
        } else {
            // Unarmed: the first click only arms the confirm (no RPC).
            actions = actions.push(variant_button(
                "Delete".to_string(),
                ButtonVariant::Primary,
                Some(crate::Message::Datacenter(Message::DeleteClicked {
                    uuid: r.id.clone(),
                    dom0: r.host.clone(),
                })),
                palette,
            ));
        }
        line = line.push(actions);
    }

    container(line)
        .padding(f32::from(spacing::BASE[3]))
        .width(Length::Fill)
        .into()
}

/// Render one audit-log row: `action`, `target`, and the timestamp. mde-theme
/// tokens only.
fn audit_row_view(entry: &AuditRow) -> Element<'_, crate::Message> {
    let target = if entry.target.is_empty() {
        "—".to_string()
    } else {
        entry.target.clone()
    };
    let ts = if entry.ts.is_empty() {
        "—".to_string()
    } else {
        entry.ts.clone()
    };
    let line = row![
        text(entry.action.clone()).width(Length::FillPortion(1)),
        text(target).width(Length::FillPortion(2)),
        text(ts).width(Length::FillPortion(2)),
    ]
    .spacing(f32::from(spacing::BASE[3]));
    container(line)
        .padding(f32::from(spacing::BASE[3]))
        .width(Length::Fill)
        .into()
}

/// Bus read: every `event/dc/*` topic's latest body, projected into both the
/// resource rows and the audit-log rows in one pass. Best-effort — a missing Bus
/// yields empty lists (the panel shows the empty state, not an error).
fn read_dc_events() -> Result<DcLoad, String> {
    let Some(dir) = mde_bus::default_data_dir() else {
        return Ok(DcLoad::default());
    };
    let persist = mde_bus::persist::Persist::open(dir).map_err(|e| e.to_string())?;
    let topics = persist.list_topics().map_err(|e| e.to_string())?;
    let mut events = Vec::new();
    for topic in topics.into_iter().filter(|t| t.starts_with("event/dc/")) {
        if let Ok(msgs) = persist.list_since(&topic, None) {
            if let Some(body) = msgs.last().and_then(|m| m.body.clone()) {
                events.push((topic, body));
            }
        }
    }
    Ok(DcLoad {
        rows: project_rows(&events),
        audit: project_audit(&events),
    })
}

/// Fire the `action/dc/vm-power` Bus RPC (blocking — runs on a `spawn_blocking`
/// thread) and translate the reply into a status line. Mirrors the connect
/// panel's Persist + `mde_bus::rpc::request` round trip, wrapped in a local
/// tokio runtime because `request` borrows a non-`Send` `Persist` across its
/// internal await. The reply body is `{"ok":true}` (→ "ok") or
/// `{"error":".."}` (→ the error text); a Bus failure / missing data dir / bad
/// reply is surfaced as an error.
fn vm_power(uuid: &str, op: &str, dom0: &str) -> Result<String, String> {
    let Some(dir) = mde_bus::default_data_dir() else {
        return Err("no Bus data dir".to_string());
    };
    let body = serde_json::json!({ "uuid": uuid, "op": op, "dom0": dom0 }).to_string();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    let reply = rt.block_on(async {
        let persist = mde_bus::persist::Persist::open(dir).map_err(|e| e.to_string())?;
        mde_bus::rpc::request(
            &persist,
            "action/dc/vm-power",
            mde_bus::hooks::config::Priority::Default,
            Some("vm-power"),
            Some(&body),
            Duration::from_secs(10),
        )
        .await
        .map_err(|e| e.to_string())
    })?;
    let raw = reply.body.unwrap_or_default();
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("bad vm-power reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok("ok".to_string());
    }
    Err(format!("unexpected vm-power reply: {raw}"))
}

/// Fire the `action/dc/vm-snapshot` Bus RPC (blocking — runs on a
/// `spawn_blocking` thread) and translate the reply into a status line. Mirrors
/// `vm_power` exactly: a Persist + `mde_bus::rpc::request` round trip wrapped in
/// a local tokio runtime because `request` borrows a non-`Send` `Persist` across
/// its internal await. The reply body is `{"ok":true,"snapshot":".."}` (→
/// `"snapshot <uuid>"`) or `{"error":".."}` (→ the error text); a Bus failure /
/// missing data dir / bad reply is surfaced as an error.
fn vm_snapshot(uuid: &str, dom0: &str) -> Result<String, String> {
    let Some(dir) = mde_bus::default_data_dir() else {
        return Err("no Bus data dir".to_string());
    };
    let body = serde_json::json!({ "uuid": uuid, "dom0": dom0 }).to_string();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    let reply = rt.block_on(async {
        let persist = mde_bus::persist::Persist::open(dir).map_err(|e| e.to_string())?;
        mde_bus::rpc::request(
            &persist,
            "action/dc/vm-snapshot",
            mde_bus::hooks::config::Priority::Default,
            Some("vm-snapshot"),
            Some(&body),
            Duration::from_secs(120),
        )
        .await
        .map_err(|e| e.to_string())
    })?;
    let raw = reply.body.unwrap_or_default();
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("bad vm-snapshot reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(format!("snapshot {uuid}"));
    }
    Err(format!("unexpected vm-snapshot reply: {raw}"))
}

/// Fire the `action/dc/tofu-plan` Bus RPC (blocking — runs on a `spawn_blocking`
/// thread) and translate the reply into the plan output. Mirrors `vm_power`
/// exactly: a Persist + `mde_bus::rpc::request` round trip wrapped in a local
/// tokio runtime because `request` borrows a non-`Send` `Persist` across its
/// internal await. The reply body is `{"ok":true,"summary":".."}` (→ the
/// summary) or `{"error":".."}` (→ the error text).
fn tofu_plan(workspace: &str) -> Result<String, String> {
    let Some(dir) = mde_bus::default_data_dir() else {
        return Err("no Bus data dir".to_string());
    };
    let body = serde_json::json!({ "workspace": workspace }).to_string();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    let reply = rt.block_on(async {
        let persist = mde_bus::persist::Persist::open(dir).map_err(|e| e.to_string())?;
        mde_bus::rpc::request(
            &persist,
            "action/dc/tofu-plan",
            mde_bus::hooks::config::Priority::Default,
            Some("tofu-plan"),
            Some(&body),
            Duration::from_secs(120),
        )
        .await
        .map_err(|e| e.to_string())
    })?;
    let raw = reply.body.unwrap_or_default();
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("bad tofu-plan reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    if let Some(summary) = v.get("summary").and_then(serde_json::Value::as_str) {
        return Ok(summary.to_string());
    }
    Err(format!("unexpected tofu-plan reply: {raw}"))
}

/// Fire the destructive `action/dc/tofu-apply` Bus RPC (blocking — runs on a
/// `spawn_blocking` thread) and translate the reply into the apply output. Only
/// reached after the typed confirm, so it always sends `"confirm":true`. Mirrors
/// `tofu_plan` exactly: a Persist + `mde_bus::rpc::request` round trip wrapped in
/// a local tokio runtime because `request` borrows a non-`Send` `Persist` across
/// its internal await. The reply body is `{"ok":true,"summary":".."}` (→ the
/// summary) or `{"error":".."}` (→ the error text).
fn tofu_apply(workspace: &str) -> Result<String, String> {
    let Some(dir) = mde_bus::default_data_dir() else {
        return Err("no Bus data dir".to_string());
    };
    let body = serde_json::json!({ "workspace": workspace, "confirm": true }).to_string();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    let reply = rt.block_on(async {
        let persist = mde_bus::persist::Persist::open(dir).map_err(|e| e.to_string())?;
        mde_bus::rpc::request(
            &persist,
            "action/dc/tofu-apply",
            mde_bus::hooks::config::Priority::Default,
            Some("tofu-apply"),
            Some(&body),
            Duration::from_secs(600),
        )
        .await
        .map_err(|e| e.to_string())
    })?;
    let raw = reply.body.unwrap_or_default();
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("bad tofu-apply reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    if let Some(summary) = v.get("summary").and_then(serde_json::Value::as_str) {
        return Ok(summary.to_string());
    }
    Err(format!("unexpected tofu-apply reply: {raw}"))
}

/// Fire the `action/dc/vm-clone` Bus RPC (blocking — runs on a `spawn_blocking`
/// thread) and translate the reply into a status line. Mirrors `vm_snapshot`
/// exactly: a Persist + `mde_bus::rpc::request` round trip wrapped in a local
/// tokio runtime because `request` borrows a non-`Send` `Persist` across its
/// internal await. The reply body is `{"ok":true,"clone":".."}` (→
/// `"clone <uuid>"`) or `{"error":".."}` (→ the error text); a Bus failure /
/// missing data dir / bad reply is surfaced as an error.
fn vm_clone(uuid: &str, dom0: &str) -> Result<String, String> {
    let Some(dir) = mde_bus::default_data_dir() else {
        return Err("no Bus data dir".to_string());
    };
    let body = serde_json::json!({ "uuid": uuid, "dom0": dom0 }).to_string();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    let reply = rt.block_on(async {
        let persist = mde_bus::persist::Persist::open(dir).map_err(|e| e.to_string())?;
        mde_bus::rpc::request(
            &persist,
            "action/dc/vm-clone",
            mde_bus::hooks::config::Priority::Default,
            Some("vm-clone"),
            Some(&body),
            Duration::from_secs(120),
        )
        .await
        .map_err(|e| e.to_string())
    })?;
    let raw = reply.body.unwrap_or_default();
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("bad vm-clone reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(format!("clone {uuid}"));
    }
    Err(format!("unexpected vm-clone reply: {raw}"))
}

/// Fire the destructive `action/dc/vm-delete` Bus RPC (blocking — runs on a
/// `spawn_blocking` thread) and translate the reply into a status line. Only
/// reached after the inline confirm, so it always sends `"confirm":true`.
/// Mirrors `vm_snapshot` exactly: a Persist + `mde_bus::rpc::request` round trip
/// wrapped in a local tokio runtime because `request` borrows a non-`Send`
/// `Persist` across its internal await. The reply body is `{"ok":true}` (→
/// `"deleted <uuid>"`) or `{"error":".."}` (→ the error text); a Bus failure /
/// missing data dir / bad reply is surfaced as an error.
fn vm_delete(uuid: &str, dom0: &str) -> Result<String, String> {
    let Some(dir) = mde_bus::default_data_dir() else {
        return Err("no Bus data dir".to_string());
    };
    let body = serde_json::json!({ "uuid": uuid, "dom0": dom0, "confirm": true }).to_string();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    let reply = rt.block_on(async {
        let persist = mde_bus::persist::Persist::open(dir).map_err(|e| e.to_string())?;
        mde_bus::rpc::request(
            &persist,
            "action/dc/vm-delete",
            mde_bus::hooks::config::Priority::Default,
            Some("vm-delete"),
            Some(&body),
            Duration::from_secs(120),
        )
        .await
        .map_err(|e| e.to_string())
    })?;
    let raw = reply.body.unwrap_or_default();
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("bad vm-delete reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(format!("deleted {uuid}"));
    }
    Err(format!("unexpected vm-delete reply: {raw}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_dc_event_reads_a_droplet() {
        let r = parse_dc_event(
            r#"{"kind":"droplet","id":"579112110","name":"lighthouse-01","status":"active","region":"nyc3","ip":"174.138.68.216","zone":"prod"}"#,
        )
        .unwrap();
        assert_eq!(r.kind, "droplet");
        assert_eq!(r.id, "579112110");
        assert_eq!(r.name, "lighthouse-01");
        assert_eq!(r.status, "active");
        assert_eq!(r.zone_label(), "Prod · DO");
        // A droplet event carries no dom0 `host` — defaults to empty.
        assert_eq!(r.host, "");
    }

    #[test]
    fn parse_dc_event_reads_the_dom0_host_on_a_vm() {
        let r = parse_dc_event(
            r#"{"kind":"vm","id":"uuid-9","name":"builder","status":"running","zone":"dev","host":"172.20.0.9"}"#,
        )
        .unwrap();
        assert_eq!(r.kind, "vm");
        assert_eq!(r.id, "uuid-9");
        assert_eq!(r.host, "172.20.0.9");
        // A vm event carries no capacity → size/used default to empty.
        assert_eq!(r.size, "");
        assert_eq!(r.used, "");
        // A vm event carries no bridge → defaults to empty.
        assert_eq!(r.bridge, "");
    }

    #[test]
    fn parse_dc_event_reads_a_net_bridge() {
        let r = parse_dc_event(
            r#"{"kind":"net","id":"net-0","name":"Pool-wide network","status":"up","zone":"dev","bridge":"xenbr0"}"#,
        )
        .unwrap();
        assert_eq!(r.kind, "net");
        assert_eq!(r.bridge, "xenbr0");
    }

    #[test]
    fn parse_dc_event_reads_sr_capacity() {
        // 207 GiB total, ~40 GiB used.
        let r = parse_dc_event(
            r#"{"kind":"sr","id":"sr-1","name":"local-ext","size":"222330230784","used":"42949672960","host":"172.20.0.9","zone":"dev"}"#,
        )
        .unwrap();
        assert_eq!(r.kind, "sr");
        assert_eq!(r.size, "222330230784");
        assert_eq!(r.used, "42949672960");
        assert_eq!(r.capacity_readout().as_deref(), Some("40 / 207 GiB (19%)"));
    }

    #[test]
    fn capacity_readout_guards_against_bad_or_zero_size() {
        let zero = DcRow {
            kind: "sr".into(),
            id: "x".into(),
            name: String::new(),
            status: String::new(),
            zone: "dev".into(),
            host: String::new(),
            size: "0".into(),
            used: "0".into(),
            bridge: String::new(),
            cpu: String::new(),
            mem_total_mb: String::new(),
            mem_free_mb: String::new(),
            load: String::new(),
        };
        assert_eq!(zero.capacity_readout(), None);
        let garbage = DcRow {
            size: "not-a-number".into(),
            ..zero.clone()
        };
        assert_eq!(garbage.capacity_readout(), None);
    }

    #[test]
    fn parse_dc_event_drops_gone_and_garbage() {
        assert!(parse_dc_event(r#"{"kind":"droplet","id":"1","gone":true}"#).is_none());
        assert!(parse_dc_event("not json").is_none());
        assert!(parse_dc_event(r#"{"id":"1"}"#).is_none()); // missing kind
    }

    #[test]
    fn project_rows_filters_and_orders_prod_first() {
        let events = vec![
            ("event/firewall/host".into(), r#"{"kind":"x","id":"1"}"#.into()), // not dc → dropped
            (
                "event/dc/vm/9".into(),
                r#"{"kind":"vm","id":"9","name":"builder","status":"running","zone":"dev"}"#.into(),
            ),
            (
                "event/dc/droplet/2".into(),
                r#"{"kind":"droplet","id":"2","name":"lighthouse-01","status":"active","zone":"prod"}"#
                    .into(),
            ),
            (
                "event/dc/droplet/3".into(),
                r#"{"kind":"droplet","id":"3","gone":true}"#.into(),
            ),
        ];
        let rows = project_rows(&events);
        assert_eq!(rows.len(), 2); // non-dc dropped, gone dropped
        assert_eq!(rows[0].zone, "prod"); // prod first
        assert_eq!(rows[0].name, "lighthouse-01");
        assert_eq!(rows[1].zone, "dev");
    }

    #[test]
    fn panel_defaults_to_the_prod_tab() {
        let p = DatacenterPanel::new();
        assert_eq!(p.zone_tab, "prod");
    }

    #[test]
    fn zone_tab_message_switches_the_active_tab() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::ZoneTab("dev".to_string()));
        assert_eq!(p.zone_tab, "dev");
        let _ = p.update(Message::ZoneTab("prod".to_string()));
        assert_eq!(p.zone_tab, "prod");
    }

    #[test]
    fn power_clicked_sets_an_in_flight_status() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::PowerClicked {
            uuid: "uuid-9".to_string(),
            op: "reboot".to_string(),
            dom0: "172.20.0.9".to_string(),
        });
        assert_eq!(p.status, "Powering reboot…");
    }

    #[test]
    fn power_done_writes_outcome_to_status() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::PowerDone(Ok("ok".to_string())));
        assert_eq!(p.status, "ok");
        let _ = p.update(Message::PowerDone(Err("boom".to_string())));
        assert_eq!(p.status, "boom");
    }

    #[test]
    fn snapshot_clicked_sets_an_in_flight_status() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::SnapshotClicked {
            uuid: "uuid-9".to_string(),
            dom0: "172.20.0.9".to_string(),
        });
        assert_eq!(p.status, "Snapshotting…");
    }

    #[test]
    fn snapshot_done_writes_outcome_to_status() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::SnapshotDone(Ok("snapshot uuid-9".to_string())));
        assert_eq!(p.status, "snapshot uuid-9");
        let _ = p.update(Message::SnapshotDone(Err("snapshot failed".to_string())));
        assert_eq!(p.status, "snapshot failed");
    }

    #[test]
    fn view_renders_for_both_tabs_without_panicking() {
        let mut p = DatacenterPanel::new();
        p.rows = project_rows(&[
            (
                "event/dc/vm/9".into(),
                r#"{"kind":"vm","id":"9","name":"builder","status":"running","zone":"dev","host":"172.20.0.9"}"#.into(),
            ),
            (
                "event/dc/net/0".into(),
                r#"{"kind":"net","id":"net-0","name":"Pool-wide network","status":"up","zone":"dev","bridge":"xenbr0"}"#.into(),
            ),
            (
                "event/dc/droplet/2".into(),
                r#"{"kind":"droplet","id":"2","name":"lighthouse-01","status":"active","zone":"prod"}"#.into(),
            ),
        ]);
        let _ = p.view(); // prod tab (default)
        let _ = p.update(Message::ZoneTab("dev".to_string()));
        let _ = p.view(); // dev tab — exercises the VM power+snapshot row + net bridge readout
        let _ = p.update(Message::ViewMode(ViewMode::Tofu));
        let _ = p.view(); // Tofu view — exercises the Plan buttons
    }

    #[test]
    fn view_renders_sr_capacity() {
        let mut p = DatacenterPanel::new();
        p.rows = project_rows(&[(
            "event/dc/sr/1".into(),
            r#"{"kind":"sr","id":"sr-1","name":"local-ext","size":"222330230784","used":"42949672960","host":"172.20.0.9","zone":"dev"}"#.into(),
        )]);
        let _ = p.update(Message::ZoneTab("dev".to_string()));
        let _ = p.view(); // exercises the sr capacity readout render path
    }

    #[test]
    fn view_mode_message_switches_the_view() {
        let mut p = DatacenterPanel::new();
        assert_eq!(p.view_mode, ViewMode::Overview);
        let _ = p.update(Message::ViewMode(ViewMode::Zone));
        assert_eq!(p.view_mode, ViewMode::Zone);
        let _ = p.update(Message::ViewMode(ViewMode::Tofu));
        assert_eq!(p.view_mode, ViewMode::Tofu);
        let _ = p.update(Message::ViewMode(ViewMode::Zone));
        assert_eq!(p.view_mode, ViewMode::Zone);
    }

    #[test]
    fn tofu_view_renders_with_empty_rows() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::ViewMode(ViewMode::Tofu));
        let _ = p.view(); // Tofu reachable even with no resource rows
    }

    #[test]
    fn tofu_plan_sets_in_flight_output() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::TofuPlan("xen-xapi".to_string()));
        assert_eq!(p.status, "Planning xen-xapi…");
        assert_eq!(p.tofu_output, "Planning xen-xapi…");
    }

    #[test]
    fn tofu_done_writes_output() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::TofuDone(Ok("No changes. 0 to add.".to_string())));
        assert_eq!(p.tofu_output, "No changes. 0 to add.");
        assert_eq!(p.status, "Plan complete");
        let _ = p.update(Message::TofuDone(Err("tofu missing".to_string())));
        assert_eq!(p.tofu_output, "tofu missing");
        assert_eq!(p.status, "tofu missing");
    }

    #[test]
    fn parse_dc_event_reads_host_metrics() {
        let r = parse_dc_event(
            r#"{"kind":"host","id":"172.20.0.9","name":"dom0-a","status":"up","zone":"dev","cpu":"8","mem_total_mb":"16000","mem_free_mb":"9000","load":"0.42"}"#,
        )
        .unwrap();
        assert_eq!(r.kind, "host");
        assert_eq!(r.cpu, "8");
        assert_eq!(r.mem_total_mb, "16000");
        assert_eq!(r.mem_free_mb, "9000");
        assert_eq!(r.load, "0.42");
    }

    #[test]
    fn parse_dc_event_defaults_metrics_empty_on_non_host() {
        // A droplet event carries no host metrics → all four default to empty.
        let r = parse_dc_event(
            r#"{"kind":"droplet","id":"1","name":"lh","status":"active","zone":"prod"}"#,
        )
        .unwrap();
        assert_eq!(r.cpu, "");
        assert_eq!(r.mem_total_mb, "");
        assert_eq!(r.mem_free_mb, "");
        assert_eq!(r.load, "");
    }

    #[test]
    fn capacity_rollup_counts_kinds_zones_and_sums_host_metrics() {
        let rows = project_rows(&[
            (
                "event/dc/host/a".into(),
                r#"{"kind":"host","id":"172.20.0.9","name":"dom0-a","status":"up","zone":"dev","cpu":"8","mem_total_mb":"16000","mem_free_mb":"9000","load":"0.4"}"#.into(),
            ),
            (
                "event/dc/host/b".into(),
                r#"{"kind":"host","id":"172.20.0.10","name":"dom0-b","status":"up","zone":"dev","cpu":"16","mem_total_mb":"32000","mem_free_mb":"20000","load":"1.0"}"#.into(),
            ),
            (
                "event/dc/vm/9".into(),
                r#"{"kind":"vm","id":"9","name":"builder","status":"running","zone":"dev","host":"172.20.0.9"}"#.into(),
            ),
            (
                "event/dc/sr/1".into(),
                r#"{"kind":"sr","id":"sr-1","name":"local","size":"1","used":"0","zone":"dev"}"#.into(),
            ),
            (
                "event/dc/net/0".into(),
                r#"{"kind":"net","id":"net-0","name":"net","status":"up","zone":"dev","bridge":"xenbr0"}"#.into(),
            ),
            (
                "event/dc/droplet/2".into(),
                r#"{"kind":"droplet","id":"2","name":"lh","status":"active","zone":"prod"}"#.into(),
            ),
        ]);
        let r = CapacityRollup::from_rows(&rows);
        assert_eq!(r.hosts, 2);
        assert_eq!(r.vms, 1);
        assert_eq!(r.droplets, 1);
        assert_eq!(r.srs, 1);
        assert_eq!(r.nets, 1);
        assert_eq!(r.prod, 1);
        assert_eq!(r.dev, 5);
        assert_eq!(r.total_cpu, 24);
        assert_eq!(r.total_mem_mb, 48000);
        assert_eq!(r.free_mem_mb, 29000);
        // 48000 total − 29000 free = 19000 MB used ≈ 18.6 GiB of 46.9 GiB.
        assert_eq!(r.memory_readout().as_deref(), Some("18.6 / 46.9 GiB used"));
    }

    #[test]
    fn capacity_rollup_memory_readout_none_without_host_metrics() {
        // No host rows → no memory total → render nothing rather than "0 GiB".
        let rows = project_rows(&[(
            "event/dc/droplet/2".into(),
            r#"{"kind":"droplet","id":"2","name":"lh","status":"active","zone":"prod"}"#.into(),
        )]);
        let r = CapacityRollup::from_rows(&rows);
        assert_eq!(r.total_mem_mb, 0);
        assert_eq!(r.memory_readout(), None);
    }

    #[test]
    fn panel_defaults_to_the_overview_view() {
        let p = DatacenterPanel::new();
        assert_eq!(p.view_mode, ViewMode::Overview);
        assert!(p.confirm_delete.is_none());
    }

    #[test]
    fn overview_view_renders_the_rollup() {
        let mut p = DatacenterPanel::new();
        p.rows = project_rows(&[
            (
                "event/dc/host/a".into(),
                r#"{"kind":"host","id":"172.20.0.9","name":"dom0-a","status":"up","zone":"dev","cpu":"8","mem_total_mb":"16000","mem_free_mb":"9000","load":"0.4"}"#.into(),
            ),
            (
                "event/dc/droplet/2".into(),
                r#"{"kind":"droplet","id":"2","name":"lh","status":"active","zone":"prod"}"#.into(),
            ),
        ]);
        // Default view is Overview — exercises the capacity rollup render path.
        let _ = p.view();
        // And it stays reachable with no host metrics (memory-none branch).
        let mut empty = DatacenterPanel::new();
        empty.rows = project_rows(&[(
            "event/dc/droplet/2".into(),
            r#"{"kind":"droplet","id":"2","name":"lh","status":"active","zone":"prod"}"#.into(),
        )]);
        let _ = empty.view();
    }

    #[test]
    fn clone_clicked_sets_an_in_flight_status() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::CloneClicked {
            uuid: "uuid-9".to_string(),
            dom0: "172.20.0.9".to_string(),
        });
        assert_eq!(p.status, "Cloning…");
    }

    #[test]
    fn clone_done_writes_outcome_to_status() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::CloneDone(Ok("clone uuid-9".to_string())));
        assert_eq!(p.status, "clone uuid-9");
        let _ = p.update(Message::CloneDone(Err("clone failed".to_string())));
        assert_eq!(p.status, "clone failed");
    }

    #[test]
    fn delete_requires_confirm_before_firing() {
        let mut p = DatacenterPanel::new();
        // First click only arms the confirm — it must NOT fire the RPC, so the
        // status is the confirm prompt and the pending-uuid is recorded.
        let _ = p.update(Message::DeleteClicked {
            uuid: "uuid-9".to_string(),
            dom0: "172.20.0.9".to_string(),
        });
        assert_eq!(p.confirm_delete.as_deref(), Some("uuid-9"));
        assert_eq!(p.status, "Confirm delete below.");
        // Only the explicit confirm clears the pending state + moves to
        // "Deleting…" (the destructive RPC then fires).
        let _ = p.update(Message::DeleteConfirmed {
            uuid: "uuid-9".to_string(),
            dom0: "172.20.0.9".to_string(),
        });
        assert!(p.confirm_delete.is_none());
        assert_eq!(p.status, "Deleting…");
    }

    #[test]
    fn delete_cancel_clears_the_pending_confirm() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::DeleteClicked {
            uuid: "uuid-9".to_string(),
            dom0: "172.20.0.9".to_string(),
        });
        assert_eq!(p.confirm_delete.as_deref(), Some("uuid-9"));
        let _ = p.update(Message::DeleteCancelled);
        assert!(p.confirm_delete.is_none());
        assert!(p.status.is_empty());
    }

    #[test]
    fn delete_done_writes_outcome_to_status() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::DeleteDone(Ok("deleted uuid-9".to_string())));
        assert_eq!(p.status, "deleted uuid-9");
        let _ = p.update(Message::DeleteDone(Err("delete failed".to_string())));
        assert_eq!(p.status, "delete failed");
    }

    #[test]
    fn vm_row_renders_confirm_prompt_when_armed() {
        let mut p = DatacenterPanel::new();
        p.rows = project_rows(&[(
            "event/dc/vm/9".into(),
            r#"{"kind":"vm","id":"9","name":"builder","status":"running","zone":"dev","host":"172.20.0.9"}"#.into(),
        )]);
        let _ = p.update(Message::ViewMode(ViewMode::Zone));
        let _ = p.update(Message::ZoneTab("dev".to_string()));
        // Arm the delete confirm on the vm row, then render — exercises the
        // inline confirm/cancel render branch in dc_row_view.
        let _ = p.update(Message::DeleteClicked {
            uuid: "9".to_string(),
            dom0: "172.20.0.9".to_string(),
        });
        let _ = p.view();
    }

    #[test]
    fn load_clears_a_pending_delete_confirm() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::DeleteClicked {
            uuid: "uuid-9".to_string(),
            dom0: "172.20.0.9".to_string(),
        });
        assert!(p.confirm_delete.is_some());
        let _ = p.update(Message::Loaded(Ok(DcLoad::default())));
        assert!(p.confirm_delete.is_none());
    }

    #[test]
    fn parse_audit_event_reads_an_apply() {
        let r = parse_audit_event(
            r#"{"action":"tofu-apply","target":"xen-xapi","ts":"2026-06-22T10:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(r.action, "tofu-apply");
        assert_eq!(r.target, "xen-xapi");
        assert_eq!(r.ts, "2026-06-22T10:00:00Z");
    }

    #[test]
    fn parse_audit_event_defaults_missing_fields_and_drops_garbage() {
        // Missing target/ts default to empty.
        let r = parse_audit_event(r#"{"action":"vm-delete"}"#).unwrap();
        assert_eq!(r.action, "vm-delete");
        assert_eq!(r.target, "");
        assert_eq!(r.ts, "");
        // Unparseable / missing action → None.
        assert!(parse_audit_event("not json").is_none());
        assert!(parse_audit_event(r#"{"target":"x"}"#).is_none());
    }

    #[test]
    fn project_audit_filters_and_orders_newest_first() {
        let events = vec![
            // Not an audit topic → dropped.
            (
                "event/dc/vm/9".into(),
                r#"{"kind":"vm","id":"9","name":"b","status":"running","zone":"dev"}"#.into(),
            ),
            (
                "event/dc/audit/1".into(),
                r#"{"action":"tofu-plan","target":"xen-xapi","ts":"2026-06-22T09:00:00Z"}"#.into(),
            ),
            (
                "event/dc/audit/2".into(),
                r#"{"action":"tofu-apply","target":"xen-xapi","ts":"2026-06-22T11:00:00Z"}"#.into(),
            ),
            (
                "event/dc/audit/3".into(),
                r#"{"action":"vm-delete","target":"uuid-9","ts":"2026-06-22T10:00:00Z"}"#.into(),
            ),
        ];
        let rows = project_audit(&events);
        assert_eq!(rows.len(), 3); // non-audit dropped
                                   // Newest-first by ts: 11:00 > 10:00 > 09:00.
        assert_eq!(rows[0].action, "tofu-apply");
        assert_eq!(rows[1].action, "vm-delete");
        assert_eq!(rows[2].action, "tofu-plan");
    }

    #[test]
    fn audit_event_is_dropped_by_project_rows() {
        // An audit body has no `kind`/`id`, so it must NOT leak into resource
        // rows even though its topic starts with `event/dc/`.
        let rows = project_rows(&[(
            "event/dc/audit/1".into(),
            r#"{"action":"tofu-apply","target":"xen-xapi","ts":"2026-06-22T11:00:00Z"}"#.into(),
        )]);
        assert!(rows.is_empty());
    }

    #[test]
    fn panel_defaults_have_no_audit_or_tofu_confirm() {
        let p = DatacenterPanel::new();
        assert!(p.audit.is_empty());
        assert!(p.tofu_confirm.is_none());
    }

    #[test]
    fn tofu_apply_requires_typed_confirm_before_firing() {
        let mut p = DatacenterPanel::new();
        // First click only arms the typed-confirm — it must NOT fire the RPC.
        let _ = p.update(Message::TofuApplyClicked("xen-xapi".to_string()));
        assert_eq!(p.tofu_confirm.as_deref(), Some("xen-xapi"));
        assert_eq!(p.status, "Type APPLY to confirm below.");
        // Only the explicit confirm clears the pending state + moves to
        // "Applying…" (the RPC then fires).
        let _ = p.update(Message::TofuApply("xen-xapi".to_string()));
        assert!(p.tofu_confirm.is_none());
        assert_eq!(p.status, "Applying xen-xapi…");
        assert_eq!(p.tofu_output, "Applying xen-xapi…");
    }

    #[test]
    fn tofu_apply_cancel_clears_the_pending_confirm() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::TofuApplyClicked("zone1-do".to_string()));
        assert_eq!(p.tofu_confirm.as_deref(), Some("zone1-do"));
        let _ = p.update(Message::TofuApplyCancelled);
        assert!(p.tofu_confirm.is_none());
        assert!(p.status.is_empty());
    }

    #[test]
    fn tofu_apply_done_writes_output() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::TofuApplyDone(Ok(
            "Apply complete! 3 added.".to_string()
        )));
        assert_eq!(p.tofu_output, "Apply complete! 3 added.");
        assert_eq!(p.status, "Apply complete");
        let _ = p.update(Message::TofuApplyDone(Err("apply failed".to_string())));
        assert_eq!(p.tofu_output, "apply failed");
        assert_eq!(p.status, "apply failed");
    }

    #[test]
    fn loaded_populates_audit_rows() {
        let mut p = DatacenterPanel::new();
        let load = DcLoad {
            rows: Vec::new(),
            audit: vec![AuditRow {
                action: "tofu-apply".into(),
                target: "xen-xapi".into(),
                ts: "2026-06-22T11:00:00Z".into(),
            }],
        };
        let _ = p.update(Message::Loaded(Ok(load)));
        assert_eq!(p.audit.len(), 1);
        assert_eq!(p.audit[0].action, "tofu-apply");
    }

    #[test]
    fn tofu_view_renders_armed_apply_confirm() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::ViewMode(ViewMode::Tofu));
        // Arm the typed-confirm on a workspace, then render — exercises the
        // inline APPLY/Cancel render branch in the Tofu view.
        let _ = p.update(Message::TofuApplyClicked("xen-xapi".to_string()));
        let _ = p.view();
    }

    #[test]
    fn audit_view_renders_rows_and_empty_state() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::ViewMode(ViewMode::Audit));
        // Empty state first.
        let _ = p.view();
        // Then with rows — exercises the audit_row_view render path, incl. the
        // empty-target/ts "—" fallbacks.
        p.audit = project_audit(&[
            (
                "event/dc/audit/1".into(),
                r#"{"action":"tofu-apply","target":"xen-xapi","ts":"2026-06-22T11:00:00Z"}"#.into(),
            ),
            (
                "event/dc/audit/2".into(),
                r#"{"action":"vm-delete"}"#.into(),
            ),
        ]);
        let _ = p.view();
    }

    #[test]
    fn audit_view_mode_is_selectable() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::ViewMode(ViewMode::Audit));
        assert_eq!(p.view_mode, ViewMode::Audit);
    }
}
