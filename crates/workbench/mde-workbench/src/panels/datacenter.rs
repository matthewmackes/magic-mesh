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

use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use cosmic::iced::widget::{
    column, container, mouse_area, pick_list, row, scrollable, text, text_input,
};
use cosmic::iced::{Length, Task};
use cosmic::Element;
use mde_theme::animation::{lerp_f32, slide_in, Animator};
use mde_theme::motion::Motion;
use mde_theme::{spacing, Palette};
use serde::{Deserialize, Serialize};

use crate::controls::{variant_button, ButtonVariant};
// Brings the `.colr(..)` text extension + `Rgba::into_cosmic_color()` into scope
// (same import the other token-styled panels use). mde-theme tokens only.
use crate::cosmic_compat::prelude::*;

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

    /// The mde-theme palette token for this row's status dot. Maps the raw status
    /// string (across DO droplets and Xen VMs/hosts) onto one of the three
    /// semantic color roles — `success` (up/running), `danger` (off/halted), or
    /// `warning` (transitional / unknown). Never a raw hex — the caller reads the
    /// concrete `Rgba` off the live palette. Pure + testable.
    #[must_use]
    pub fn status_dot(&self, palette: Palette) -> mde_theme::Rgba {
        // Lower-cased so "Running" / "RUNNING" / "running" all match; the worker
        // emits DO ("active"/"off") + Xen ("running"/"halted") vocabularies.
        match self.status.to_ascii_lowercase().as_str() {
            "running" | "active" | "up" | "online" | "ready" => palette.success,
            "halted" | "off" | "stopped" | "shutoff" | "down" | "error" => palette.danger,
            "paused" | "suspended" | "rebooting" | "starting" | "pending"
            | "provisioning" => palette.warning,
            // Unknown / empty — a muted dot rather than a misleading green/red.
            _ => palette.text_muted,
        }
    }

    /// Whether this row matches a free-text filter `needle` (case-insensitive
    /// substring over name / id / kind). An empty/whitespace needle matches every
    /// row, so an empty search box never hides anything. Pure + testable.
    #[must_use]
    pub fn matches_filter(&self, needle: &str) -> bool {
        let needle = needle.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return true;
        }
        self.name.to_ascii_lowercase().contains(&needle)
            || self.id.to_ascii_lowercase().contains(&needle)
            || self.kind.to_ascii_lowercase().contains(&needle)
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

/// One stage of the **Build → Eagle → DO** promotion pipeline as last seen on the
/// Bus (`event/dc/promote/<stage>`). The Overview view renders these three stages
/// as a horizontal version matrix so the operator can see, at a glance, which
/// version each promotion target is pinned to and whether it's ready or pending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromoteStage {
    /// The pipeline stage — "build" | "eagle" | "do" (canonical order).
    pub stage: String,
    /// The version pinned at this stage — e.g. "11.0.1". "—" for an absent stage.
    pub version: String,
    /// The stage's readiness — "ready" | "pending" (or "unknown" for a filled
    /// placeholder). Drives the status chip's color token.
    pub status: String,
}

/// Parse one `event/dc/promote/<stage>` message body into a [`PromoteStage`].
/// Returns `None` for unparseable JSON or a body missing the `stage` field. Pure +
/// testable. Mirrors [`parse_audit_event`]'s tolerant string extraction: `version`
/// and `status` default when absent.
#[must_use]
pub fn parse_promote_event(body: &str) -> Option<PromoteStage> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let stage = v.get("stage")?.as_str()?.to_string();
    let version = v
        .get("version")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let status = v
        .get("status")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    Some(PromoteStage {
        stage,
        version,
        status,
    })
}

/// Project a set of `(topic, latest-body)` Bus reads into promotion stages —
/// `event/dc/promote/*` topics only. Order/fill is left to [`promote_matrix`]; this
/// just parses the matching topics. Pure + testable.
#[must_use]
pub fn project_promote(events: &[(String, String)]) -> Vec<PromoteStage> {
    events
        .iter()
        .filter(|(topic, _)| topic.starts_with("event/dc/promote/"))
        .filter_map(|(_, body)| parse_promote_event(body))
        .collect()
}

/// Return the three promotion stages in canonical order — **build, eagle, do** —
/// filling any absent stage with a placeholder (`version: "—"`, `status:
/// "unknown"`) so the Overview strip always renders exactly three cards. A
/// duplicate stage in the input keeps the first seen. Pure + testable.
#[must_use]
pub fn promote_matrix(stages: &[PromoteStage]) -> Vec<PromoteStage> {
    ["build", "eagle", "do"]
        .iter()
        .map(|canon| {
            stages
                .iter()
                .find(|s| s.stage == *canon)
                .cloned()
                .unwrap_or_else(|| PromoteStage {
                    stage: (*canon).to_string(),
                    version: "—".to_string(),
                    status: "unknown".to_string(),
                })
        })
        .collect()
}

/// One datacenter health check as last seen on the Bus (`event/dc/health/*`).
/// The `datacenter_orchestrator` worker publishes a check per probe (Bus
/// reachable, dom0 SSH, doctl auth, …); the Overview view rolls these into a
/// one-line ok/warn/fail summary plus an alert list of any non-ok checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthCheck {
    /// The check name — e.g. "bus" | "dom0-a" | "doctl". Identifies the probe.
    pub check: String,
    /// The check's outcome — "ok" | "warn" | "fail" (anything not ok/warn counts
    /// as a failure in [`health_summary`]). Drives the alert's color token.
    pub status: String,
    /// A human detail line for a non-ok check — the reason it warned/failed.
    /// Empty when the event didn't name one. Shown beside the check in the alert
    /// list.
    pub detail: String,
}

/// Parse one `event/dc/health/<check>` message body into a [`HealthCheck`].
/// Returns `None` for unparseable JSON or a body missing the `check` field. Pure +
/// testable. Mirrors [`parse_audit_event`]'s tolerant string extraction: `status`
/// and `detail` default to empty when absent.
#[must_use]
pub fn parse_health_event(body: &str) -> Option<HealthCheck> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let check = v.get("check")?.as_str()?.to_string();
    let status = v
        .get("status")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let detail = v
        .get("detail")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    Some(HealthCheck {
        check,
        status,
        detail,
    })
}

/// Project a set of `(topic, latest-body)` Bus reads into health checks —
/// `event/dc/health/*` topics only, sorted by check name for a stable render
/// order. Pure + testable.
#[must_use]
pub fn project_health(events: &[(String, String)]) -> Vec<HealthCheck> {
    let mut checks: Vec<HealthCheck> = events
        .iter()
        .filter(|(topic, _)| topic.starts_with("event/dc/health/"))
        .filter_map(|(_, body)| parse_health_event(body))
        .collect();
    checks.sort_by(|a, b| a.check.cmp(&b.check));
    checks
}

/// Tally a set of health checks into `(ok, warn, fail)` counts. A check counts as
/// `ok` when its status is exactly "ok", `warn` when exactly "warn", and `fail`
/// for anything else (incl. an empty/unknown status — fail-safe). Pure + testable.
#[must_use]
pub fn health_summary(checks: &[HealthCheck]) -> (usize, usize, usize) {
    let mut ok = 0;
    let mut warn = 0;
    let mut fail = 0;
    for c in checks {
        match c.status.as_str() {
            "ok" => ok += 1,
            "warn" => warn += 1,
            _ => fail += 1,
        }
    }
    (ok, warn, fail)
}

/// One datacenter-action **job** as last seen on the Bus (`event/dc/job/<ulid>`).
/// The `dc_jobs` worker publishes one of these for every datacenter action RPC —
/// `{"action":"dc/<verb>","ulid":..,"status":"pending|ok|error"}`. The Overview's
/// "Recent Tofu runs" section filters these to the tofu verbs and renders a
/// run-log (DATACENTER-9/15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRow {
    /// The action performed — e.g. "dc/tofu-plan" | "dc/tofu-apply" | "dc/vm-power".
    pub action: String,
    /// The job's ULID (the `<ulid>` of the `event/dc/job/<ulid>` topic, echoed in
    /// the body). Time-ordered, so a descending sort is newest-first.
    pub ulid: String,
    /// The job's outcome — "pending" | "ok" | "error". Drives the status chip's
    /// color token. Empty when the event didn't name one.
    pub status: String,
}

/// Parse one `event/dc/job/<ulid>` message body into a [`JobRow`]. Returns `None`
/// for unparseable JSON or a body missing the `action` field. Pure + testable.
/// Mirrors [`parse_audit_event`]'s tolerant string extraction: `ulid` and `status`
/// default to empty when absent.
#[must_use]
pub fn parse_job_event(body: &str) -> Option<JobRow> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let action = v.get("action")?.as_str()?.to_string();
    let ulid = v
        .get("ulid")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let status = v
        .get("status")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    Some(JobRow {
        action,
        ulid,
        status,
    })
}

/// Maximum number of recent Tofu runs the Overview shows.
const RECENT_TOFU_CAP: usize = 8;

/// Filter a set of job rows to the **Tofu** runs (action contains "tofu" —
/// tofu-plan / tofu-apply / tofu-destroy / tofu-state), newest-first (descending
/// `ulid`, which is time-ordered), capped at [`RECENT_TOFU_CAP`]. Pure + testable.
#[must_use]
pub fn recent_tofu_runs(jobs: &[JobRow]) -> Vec<JobRow> {
    let mut runs: Vec<JobRow> = jobs
        .iter()
        .filter(|j| j.action.contains("tofu"))
        .cloned()
        .collect();
    // Newest-first: ULIDs are lexicographically time-ordered, so descending by
    // `ulid` is newest-first. Stable order for ties.
    runs.sort_by(|a, b| b.ulid.cmp(&a.ulid));
    runs.truncate(RECENT_TOFU_CAP);
    runs
}

/// Project a set of `(topic, latest-body)` Bus reads into job rows —
/// `event/dc/job/*` topics only. Order/filter/cap is left to [`recent_tofu_runs`];
/// this just parses the matching topics. Pure + testable.
#[must_use]
pub fn project_jobs(events: &[(String, String)]) -> Vec<JobRow> {
    events
        .iter()
        .filter(|(topic, _)| topic.starts_with("event/dc/job/"))
        .filter_map(|(_, body)| parse_job_event(body))
        .collect()
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

/// Group projected rows into the topology map the `Topology` view renders: one
/// `(header, children)` tuple per Dev host (`kind == "host"`), with that host's
/// VMs / SRs / networks (any non-host row whose `r.host` equals the host `id`)
/// nested underneath. Everything left over — the Prod droplets, the gateway, and
/// any orphan whose `host` names no known host — lands in a single synthetic
/// trailing group whose header carries `kind == ""` and `id == ""` (a sentinel
/// the view recognizes to label it "Prod / Gateway / unattached" rather than as a
/// host). Hosts come first in `id` order (stable); the synthetic group, when it
/// has children, is always last. Pure + testable.
#[must_use]
pub fn group_by_host(rows: &[DcRow]) -> Vec<(DcRow, Vec<DcRow>)> {
    // The host headers, in stable `id` order.
    let mut hosts: Vec<&DcRow> = rows.iter().filter(|r| r.kind == "host").collect();
    hosts.sort_by(|a, b| a.id.cmp(&b.id));
    let host_ids: BTreeSet<&str> = hosts.iter().map(|h| h.id.as_str()).collect();

    let mut groups: Vec<(DcRow, Vec<DcRow>)> = Vec::with_capacity(hosts.len() + 1);
    for host in &hosts {
        let children: Vec<DcRow> = rows
            .iter()
            .filter(|r| r.kind != "host" && r.host == host.id)
            .cloned()
            .collect();
        groups.push(((*host).clone(), children));
    }

    // Orphans: non-host rows that no known host claims (Prod droplets carry no
    // `host`; the gateway / any dangling resource lands here too).
    let orphans: Vec<DcRow> = rows
        .iter()
        .filter(|r| r.kind != "host" && !host_ids.contains(r.host.as_str()))
        .cloned()
        .collect();
    if !orphans.is_empty() {
        // Synthetic header — the empty `kind`/`id` is the sentinel the view keys
        // on to render the "Prod · DO / Gateway / unattached" label.
        let synthetic = DcRow {
            kind: String::new(),
            id: String::new(),
            name: "Prod · DO / Gateway".to_string(),
            status: String::new(),
            zone: String::new(),
            host: String::new(),
            size: String::new(),
            used: String::new(),
            bridge: String::new(),
            cpu: String::new(),
            mem_total_mb: String::new(),
            mem_free_mb: String::new(),
            load: String::new(),
        };
        groups.push((synthetic, orphans));
    }
    groups
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

/// How many rolling samples the Overview keeps for its sparklines — one per Bus
/// load. A short window so the sparkline reads "recent trend" rather than full
/// history; the oldest sample is evicted when a fresh one would overflow.
pub const HISTORY_CAP: usize = 24;

/// One point of the Overview's short rolling history — a compact snapshot of the
/// fleet taken on each Bus load. The ring buffer of these
/// ([`DatacenterPanel::history`]) feeds the Overview's [`sparkline`]s so the
/// operator can see, at a glance, whether resource / health counts are trending.
/// Pure data; carries only the few scalars the sparklines plot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HistorySample {
    /// Total datacenter resources at this sample (`rows.len()`). Plotted as the
    /// "resources" trend line.
    pub resources: usize,
    /// Running VMs + active droplets at this sample — the compute footprint.
    pub running: usize,
    /// Health checks reporting `ok` at this sample. Plotted as the "ok" trend.
    pub health_ok: usize,
    /// Health checks reporting `warn`-or-`fail` at this sample. Plotted as the
    /// "alerts" trend so a rising line flags a degrading fleet.
    pub health_alerts: usize,
}

impl HistorySample {
    /// Snapshot the current projected rows + health checks into one history
    /// point. Pure — derives every field from the inputs, takes no clock (the
    /// ring buffer is ordered by insertion, not timestamp). "Running" counts VMs
    /// whose status is `running` plus droplets whose status is `active` (the two
    /// live-compute vocabularies the worker emits).
    #[must_use]
    pub fn capture(rows: &[DcRow], health: &[HealthCheck]) -> Self {
        let running = rows
            .iter()
            .filter(|r| {
                (r.kind == "vm" && r.status.eq_ignore_ascii_case("running"))
                    || (r.kind == "droplet" && r.status.eq_ignore_ascii_case("active"))
            })
            .count();
        let (ok, warn, fail) = health_summary(health);
        Self {
            resources: rows.len(),
            running,
            health_ok: ok,
            health_alerts: warn + fail,
        }
    }
}

/// The eight Unicode block-element glyphs, lowest (`▁`) to highest (`█`), used by
/// [`sparkline`] to map a normalized sample onto a single-cell bar.
const SPARK_GLYPHS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Project a series of sample values into a single-line block-glyph sparkline —
/// each point becomes one of the eight `▁▂▃▄▅▆▇█` bars, scaled across the series'
/// own min..max so the relative shape reads at a glance. An empty series yields an
/// empty string; an all-equal series yields a flat mid-height line (no spurious
/// slope). Pure and testable; the projection is value-only (no color/widget), so
/// it composes into any text element. Used by the Overview to draw the rolling
/// history of the resource / health counts beside their last-value readouts.
#[must_use]
pub fn sparkline(points: &[f32]) -> String {
    if points.is_empty() {
        return String::new();
    }
    let min = points.iter().copied().fold(f32::INFINITY, f32::min);
    let max = points.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let span = max - min;
    // A flat series (or a single point) has no slope to show — render a flat
    // mid-height line rather than dividing by zero / pinning everything to the
    // floor glyph.
    if span <= f32::EPSILON {
        return SPARK_GLYPHS[SPARK_GLYPHS.len() / 2]
            .to_string()
            .repeat(points.len());
    }
    points
        .iter()
        .map(|&p| {
            // Normalize into 0..=1, then index the eight glyphs. The `min(7)`
            // guards the exact-max point (norm == 1.0 → index 8) back into range.
            let norm = (p - min) / span;
            #[allow(
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss
            )]
            let idx = (norm * (SPARK_GLYPHS.len() as f32 - 1.0)).round() as usize;
            SPARK_GLYPHS[idx.min(SPARK_GLYPHS.len() - 1)]
        })
        .collect()
}

/// One row of the Overview's **version matrix** — `farm / Eagle / each lighthouse`.
/// Where the [`promote_matrix`] strip shows only the three *pipeline stages*, this
/// projects a per-target row: the build farm, the Eagle staging host, and one row
/// per live lighthouse (a Prod droplet), each pinned to the version it's expected
/// to run and a readiness state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionRow {
    /// The target's display label — "Farm (build)", "Eagle", or a lighthouse name.
    pub target: String,
    /// The version this target is pinned to (`"—"` when unknown / unobserved).
    pub version: String,
    /// Readiness — `"ready"` | `"pending"` | `"unknown"` (drives the chip color).
    pub status: String,
}

/// The full version matrix: the farm + Eagle stage rows followed by one row per
/// lighthouse. Built purely from the promote stages + the projected droplet rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionMatrix {
    pub rows: Vec<VersionRow>,
}

impl VersionMatrix {
    /// Project the `farm / Eagle / each-lighthouse` matrix off the promote stages
    /// (`build`/`eagle`/`do`) + the resource rows. The **farm** row takes the
    /// `build` stage; **Eagle** the `eagle` stage; and every Prod **droplet** row
    /// becomes a lighthouse pinned to the `do` stage's version (the version DO is
    /// being promoted to), its status the droplet's own readiness — `ready` for an
    /// `active` droplet, else `pending`. Lighthouses are ordered by name for a
    /// stable render. Pure and testable. An absent promote stage fills `"—"` /
    /// `"unknown"` so the matrix always renders the farm + Eagle rows.
    #[must_use]
    pub fn project(stages: &[PromoteStage], rows: &[DcRow]) -> Self {
        // Reuse the canonical build/eagle/do fill so absent stages already carry
        // the "—" / "unknown" placeholders — no second copy of that logic here.
        let canon = promote_matrix(stages);
        let labelled = |stage: &str, label: &str| {
            let s = canon
                .iter()
                .find(|s| s.stage == stage)
                .expect("promote_matrix always yields build/eagle/do");
            VersionRow {
                target: label.to_string(),
                version: if s.version.is_empty() {
                    "—".to_string()
                } else {
                    s.version.clone()
                },
                status: s.status.clone(),
            }
        };

        let mut out = vec![
            labelled("build", "Farm (build)"),
            labelled("eagle", "Eagle"),
        ];

        // The version DO is promoting toward — every lighthouse is expected to
        // converge on it. The canonical fill substitutes "—" for an unobserved
        // `do` stage; an observed-but-empty version maps to "—" too.
        let do_version = canon
            .iter()
            .find(|s| s.stage == "do")
            .map(|s| s.version.clone())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "—".to_string());

        // One row per live lighthouse — the Prod droplets carry lighthouse
        // identity. Sorted by name for a stable matrix.
        let mut lighthouses: Vec<&DcRow> = rows
            .iter()
            .filter(|r| r.kind == "droplet" && r.zone == "prod")
            .collect();
        lighthouses.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        for lh in lighthouses {
            let name = if lh.name.is_empty() {
                lh.id.clone()
            } else {
                lh.name.clone()
            };
            // A droplet that's `active` is up + reachable → it's running the DO
            // target (ready); anything else is mid-flight (pending).
            let status = if lh.status.eq_ignore_ascii_case("active") {
                "ready"
            } else {
                "pending"
            }
            .to_string();
            out.push(VersionRow {
                target: name,
                version: do_version.clone(),
                status,
            });
        }

        Self { rows: out }
    }
}

// ── DATACENTER-8/1813 — per-kind sub-tabs within a zone ───────────────────────

/// The per-kind sub-tabs the Zone view splits its single card grid into (the
/// DATACENTER-8/1813 "Hosts / VMs / Storage / Network within the zone" lock).
/// Each tab maps to the resource `kind`s it shows; the Zone view filters the
/// rendered cards to the active tab on top of the zone + global search. Pure +
/// testable — `matches` is a total function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindTab {
    Hosts,
    Vms,
    Storage,
    Network,
}

impl KindTab {
    /// The four sub-tabs in render order.
    #[must_use]
    pub const fn all() -> [KindTab; 4] {
        [
            KindTab::Hosts,
            KindTab::Vms,
            KindTab::Storage,
            KindTab::Network,
        ]
    }

    /// The sub-tab's button label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            KindTab::Hosts => "Hosts",
            KindTab::Vms => "VMs",
            KindTab::Storage => "Storage",
            KindTab::Network => "Network",
        }
    }

    /// A stable lowercase slug — persisted inside a saved view alongside the
    /// [`ViewMode`] slug so a restored Zone view lands on the same sub-tab.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            KindTab::Hosts => "hosts",
            KindTab::Vms => "vms",
            KindTab::Storage => "storage",
            KindTab::Network => "network",
        }
    }

    /// Recover a sub-tab from its slug; an unknown/empty slug falls back to VMs
    /// (the busiest tab) rather than dropping the saved view. Pure.
    #[must_use]
    pub fn from_slug(slug: &str) -> KindTab {
        match slug {
            "hosts" => KindTab::Hosts,
            "storage" => KindTab::Storage,
            "network" => KindTab::Network,
            _ => KindTab::Vms,
        }
    }

    /// Whether a resource of `kind` belongs under this sub-tab. The compute tab
    /// (VMs) holds both Xen `vm`s and Prod `droplet`s; Storage holds SRs / VDIs /
    /// ISOs; Network holds nets / PIFs / VLANs; Hosts holds the dom0 `host`s.
    /// Pure + total.
    #[must_use]
    pub fn matches(self, kind: &str) -> bool {
        match self {
            KindTab::Hosts => kind == "host",
            KindTab::Vms => kind == "vm" || kind == "droplet",
            KindTab::Storage => matches!(kind, "sr" | "vdi" | "iso"),
            KindTab::Network => matches!(kind, "net" | "pif" | "vlan"),
        }
    }
}

/// DATACENTER-16 — one host wake/power phase as last seen on the Bus
/// (`event/dc/power/<host>`). The energy-aware power worker times each wake
/// (WOL/IPMI → dom0 ready), keeps a rolling per-host average, and publishes the
/// live phase + percent-complete + a learned ETA. The Hosts tab renders these as a
/// phased progress bar (POST → XCP → toolstack) with a live ETA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerStatus {
    /// The host (dom0 IP) being woken.
    pub host: String,
    /// The current phase — "post" | "xcp" | "toolstack" | "ready" (or empty).
    pub phase: String,
    /// Percent-complete (0..=100) as a string; parsed defensively.
    pub pct: String,
    /// The live ETA-to-ready in seconds as a string (empty when unknown).
    pub eta_secs: String,
}

impl PowerStatus {
    /// The percent-complete clamped to 0..=100, or 0 when the field is missing /
    /// unparseable. Drives the progress bar fill. Pure.
    #[must_use]
    pub fn pct_clamped(&self) -> u8 {
        // Parse → clamp into 0..=100; the clamped value always fits a u8.
        let v = self.pct.parse::<u32>().unwrap_or(0).min(100);
        u8::try_from(v).unwrap_or(100)
    }

    /// A human phase label for the progress bar caption. Pure.
    #[must_use]
    pub fn phase_label(&self) -> &'static str {
        match self.phase.as_str() {
            "post" => "POST",
            "xcp" => "XCP boot",
            "toolstack" => "toolstack",
            "ready" => "ready",
            _ => "waking",
        }
    }
}

/// Parse one `event/dc/power/<host>` body into a [`PowerStatus`]. Returns `None`
/// for unparseable JSON or a body missing the `host` field. Pure + testable.
#[must_use]
pub fn parse_power_event(body: &str) -> Option<PowerStatus> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    if v.get("gone").and_then(serde_json::Value::as_bool) == Some(true) {
        return None;
    }
    let host = v.get("host")?.as_str()?.to_string();
    let phase = v
        .get("phase")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let pct = v
        .get("pct")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let eta_secs = v
        .get("eta_secs")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    Some(PowerStatus {
        host,
        phase,
        pct,
        eta_secs,
    })
}

/// Project the `event/dc/power/*` topics into power-status rows, sorted by host
/// for a stable render. A host that has reached `ready` is dropped (its wake is
/// done — no lingering 100% bar). Pure + testable.
#[must_use]
pub fn project_power(events: &[(String, String)]) -> Vec<PowerStatus> {
    let mut rows: Vec<PowerStatus> = events
        .iter()
        .filter(|(topic, _)| topic.starts_with("event/dc/power/"))
        .filter_map(|(_, body)| parse_power_event(body))
        .filter(|p| p.phase != "ready")
        .collect();
    rows.sort_by(|a, b| a.host.cmp(&b.host));
    rows
}

/// DATACENTER-13 — one correlated row from the unified `action/dc/ipdns` read: a
/// single host seen across the three name/address sources — the UniFi DHCP lease,
/// the DigitalOcean DNS record, and the Nebula overlay IP. Empty fields render as
/// "—". Pure data.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IpDnsEntry {
    /// The host's name (the join key across the three sources).
    pub name: String,
    /// The UniFi DHCP lease address, or empty when the host has no lease.
    pub lease_ip: String,
    /// The DigitalOcean DNS record, or empty when there is none.
    pub dns: String,
    /// The Nebula overlay IP, or empty when the host isn't on the overlay.
    pub overlay_ip: String,
}

/// Parse the `action/dc/ipdns` reply's `entries` array into [`IpDnsEntry`] rows.
/// Tolerant: each entry's fields default to empty; a non-array / absent `entries`
/// yields an empty list. Pure + testable.
#[must_use]
pub fn parse_ipdns(v: &serde_json::Value) -> Vec<IpDnsEntry> {
    let Some(arr) = v.get("entries").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    let field = |e: &serde_json::Value, k: &str| {
        e.get(k)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    arr.iter()
        .map(|e| IpDnsEntry {
            name: field(e, "name"),
            lease_ip: field(e, "lease_ip"),
            dns: field(e, "dns"),
            overlay_ip: field(e, "overlay_ip"),
        })
        .collect()
}

/// DATACENTER-19 — one DigitalOcean region from the `action/dc/do-regions` read.
/// The region picker shows the slug + human name + availability; the geo group
/// (inferred from the slug prefix) drives the multi-region-spread nudge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoRegion {
    /// The region slug (e.g. `nyc3`) — the only knob for the fixed lighthouse
    /// profile.
    pub slug: String,
    /// The human region name (e.g. `New York 3`).
    pub name: String,
    /// Whether DigitalOcean currently allows new droplets in this region.
    pub available: bool,
}

impl DoRegion {
    /// The geo group inferred from the slug's datacenter prefix (the letters before
    /// the trailing digit — `nyc3` → `nyc`). Two lighthouses in different groups is
    /// the spread the picker nudges toward. Pure.
    #[must_use]
    pub fn geo(&self) -> String {
        self.slug
            .trim_end_matches(|c: char| c.is_ascii_digit())
            .to_string()
    }
}

/// Parse the `action/dc/do-regions` reply's `regions` array into [`DoRegion`]s,
/// sorted by slug for a stable picker. Tolerant: a region missing `slug` is
/// dropped; `name` defaults to the slug; `available` defaults to true. Pure +
/// testable.
#[must_use]
pub fn parse_do_regions(v: &serde_json::Value) -> Vec<DoRegion> {
    let Some(arr) = v.get("regions").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    let mut out: Vec<DoRegion> = arr
        .iter()
        .filter_map(|e| {
            let slug = e.get("slug").and_then(serde_json::Value::as_str)?.to_string();
            let name = e
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&slug)
                .to_string();
            let available = e
                .get("available")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);
            Some(DoRegion {
                slug,
                name,
                available,
            })
        })
        .collect();
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    out
}

/// DATACENTER-19 — the count of distinct geo groups across a set of chosen region
/// slugs. The picker nudges toward a multi-region spread: ≥2 groups for the (soon
/// two) lighthouses. Pure + testable.
#[must_use]
pub fn distinct_geos(slugs: &[String]) -> usize {
    let mut geos: Vec<String> = slugs
        .iter()
        .map(|s| {
            s.trim_end_matches(|c: char| c.is_ascii_digit())
                .to_string()
        })
        .collect();
    geos.sort();
    geos.dedup();
    geos.len()
}

/// Render a fixed-width text progress bar for a 0..=100 percent — eight filled
/// (`█`) / empty (`░`) cells. Token-free (pure text; the caller colors it with a
/// palette token), so it composes into any text element. Pure + testable.
#[must_use]
pub fn progress_bar(pct: u8) -> String {
    const CELLS: usize = 8;
    let filled = ((usize::from(pct.min(100)) * CELLS) / 100).min(CELLS);
    let mut s = String::with_capacity(CELLS);
    for i in 0..CELLS {
        s.push(if i < filled { '\u{2588}' } else { '\u{2591}' });
    }
    s
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
    /// The managed-resource names from the latest `action/dc/tofu-state` reply,
    /// rendered in the Tofu view as a "Managed resources (N)" list.
    pub tofu_state_resources: Vec<String>,
    /// Whether the latest `action/dc/tofu-state` reply reported drift (live infra
    /// differs from recorded state). Renders a ⚠ DRIFT / ✓ in-sync badge.
    pub tofu_state_drift: bool,
    /// The workspace the latest `action/dc/tofu-state` reply describes — so the
    /// rendered resource list / drift badge name which workspace they belong to.
    /// Empty until a State button has been clicked and returned.
    pub tofu_state_ws: String,
    /// When `Some(uuid)`, a VM delete is awaiting inline confirmation — its row
    /// renders a "Really delete?" prompt and only the confirm button fires the
    /// destructive `action/dc/vm-delete` RPC. Cleared once a delete is fired or
    /// the load refreshes.
    pub confirm_delete: Option<String>,
    /// The audit-log rows read off `event/dc/audit/*`, newest-first. Rendered by
    /// the `Audit` view. Refreshed alongside `rows` on every load.
    pub audit: Vec<AuditRow>,
    /// The Build → Eagle → DO promotion stages read off `event/dc/promote/*`.
    /// Rendered by the `Overview` view as a version matrix. Refreshed alongside
    /// `rows` on every load.
    pub promote: Vec<PromoteStage>,
    /// The datacenter health checks read off `event/dc/health/*`. Rendered by the
    /// `Overview` view as an ok/warn/fail summary + alert list. Refreshed
    /// alongside `rows` on every load.
    pub health: Vec<HealthCheck>,
    /// The datacenter action jobs read off `event/dc/job/*`. The `Overview` view
    /// filters these to the Tofu verbs (via [`recent_tofu_runs`]) for the "Recent
    /// Tofu runs" run-log. Refreshed alongside `rows` on every load.
    pub jobs: Vec<JobRow>,
    /// When `Some(workspace)`, a Tofu apply is awaiting typed confirmation — the
    /// workspace's row renders a "Type APPLY to confirm" prompt and only the
    /// confirm button fires the `action/dc/tofu-apply` RPC. Cleared once the
    /// apply is fired or cancelled.
    pub tofu_confirm: Option<String>,
    /// Which `Topology`-view group headers are currently expanded — a set of host
    /// `id`s (the synthetic Prod/Gateway group uses the empty-string key). A host
    /// header is rendered expanded (children shown) iff its id is present; the
    /// `HeaderClicked` message toggles membership. Defaults expanded (the set is
    /// seeded on the first Topology render via [`Self::ensure_topology_seeded`]),
    /// so the v1 map opens fully drilled-down.
    pub expanded: BTreeSet<String>,
    /// Tracks whether [`Self::expanded`] has been seeded for the current row set —
    /// so a fresh load re-seeds (all groups open) but a manual collapse sticks
    /// across re-renders. Reset to `false` on every `Loaded`.
    pub topology_seeded: bool,
    /// The latest DR / backup status line, rendered under the "Back up now"
    /// button on the Overview view — the in-flight text, the returned
    /// `"backed up: <path>"` on success, or the error text. Empty until a backup
    /// has been run.
    pub dr_status: String,
    /// When `true`, a DR backup is awaiting typed confirmation — the Overview
    /// renders a "Backup state + secrets? [Confirm]" prompt and only the confirm
    /// button fires the `action/dc/dr-backup` RPC. Cleared once the backup is
    /// fired.
    pub dr_confirm: bool,
    /// The global resource filter — a free-text needle matched case-insensitively
    /// against each row's name / id / kind (see [`DcRow::matches_filter`]). Empty
    /// shows everything. Applied on top of the per-tab (zone / topology / card-
    /// grid) views so the search narrows whatever set is currently rendered.
    pub filter: String,
    /// MOTION-FEEDBACK-2 — when the card grid last (re)loaded, driving the capped
    /// staggered reveal. `Some(start)` while a reveal is animating; the view reads
    /// each card's eased fade+slide off this origin and the per-card delay. Stamped
    /// on `Loaded(Ok)`/`RefreshClicked`; once the reveal has elapsed the tick loop
    /// clears it so a settled grid does no per-frame work.
    pub reveal_start: Option<Instant>,
    /// MOTION-FEEDBACK-2 — the id of the selected/focused card, or `None`. The
    /// selected card draws an animated accent ring; clicking a card sets this and
    /// arms the [`Self::selection`] tween.
    pub selected_card: Option<String>,
    /// MOTION-FEEDBACK-2 — the animated accent on the selected card. Keyed by the
    /// card id; [`Animator::value`] gives the eased 0→1 grow-in of the ring, and
    /// [`Animator::is_idle`] lets the tick loop stop once it settles.
    pub selection: Animator,
    /// MOTION-FEEDBACK-2 — the id of the currently hovered card (`None` = no hover),
    /// plus when the hover last changed — drives the per-card hover-lift tween.
    pub hovered_card: Option<String>,
    /// When [`Self::hovered_card`] last toggled — the `start` for the hover-lift.
    pub hover_since: Instant,
    /// MOTION-FEEDBACK-2 — `true` while a self-re-arming [`Message::MotionTick`]
    /// chain is running, so concurrent state changes don't spawn a second chain.
    /// Cleared when the reveal + selection tweens have all settled (no idle
    /// wakeups — the chain stops ticking at rest).
    pub motion_ticking: bool,
    /// DATACENTER-9 — a short rolling history of the fleet, one [`HistorySample`]
    /// per Bus load, capped at [`HISTORY_CAP`] (oldest evicted). Pushed on every
    /// `Loaded(Ok)` via [`Self::push_sample`]; the Overview reads it back with
    /// [`Self::history`] to draw the resource / health [`sparkline`]s. A ring
    /// buffer so a long-running session keeps only the recent trend, not unbounded
    /// growth.
    pub history: VecDeque<HistorySample>,
    /// DATACENTER-8 (saved views) — the operator's named saved views (each a view
    /// mode, zone tab, and search needle), hydrated from the local config file by
    /// the first panel-open `load()`. The header renders a restore chip per view;
    /// saving the current view persists the file.
    pub saved_views: SavedViews,
    /// DATACENTER-8 (saved views) — the in-progress name in the "Save view as…"
    /// box. Cleared once a view is saved. Pure UI state.
    pub save_view_name: String,
    /// DATACENTER-8 (saved views) — whether [`Self::saved_views`] has been
    /// hydrated from disk yet. The first panel-open's `load()` reads the file
    /// off-thread (keeping `Default`/init pure + non-blocking, matching the
    /// panel's lazy-load convention); subsequent reloads are ignored so a Bus
    /// refresh never clobbers in-memory saved-view edits.
    pub views_loaded: bool,

    // ── DATACENTER-8/1813 — per-kind sub-tabs ─────────────────────────────────
    /// Which per-kind sub-tab the Zone view shows (Hosts / VMs / Storage /
    /// Network). The card grid is filtered to this tab on top of the zone +
    /// search. Defaults to VMs (the busiest tab).
    pub kind_tab: KindTab,

    // ── DATACENTER-11 — VM lifecycle (suspend / migrate / console / bulk / create)
    /// When `Some((uuid, dom0))`, a VM migrate is being targeted — the row renders
    /// a host picker + Confirm/Cancel; only Confirm fires `action/dc/vm-migrate`.
    pub migrate: Option<(String, String)>,
    /// The migrate target host picked in the dropdown (a dom0 IP), or `None`.
    pub migrate_target: Option<String>,
    /// DATACENTER-11 — whether the VMs tab is in multi-select bulk mode. Each VM
    /// card then renders a Select toggle and a bulk action bar appears.
    pub bulk_mode: bool,
    /// The set of VM uuids selected for a bulk action.
    pub bulk_sel: BTreeSet<String>,
    /// Per-item bulk progress, keyed by uuid → a short status word ("ok" /
    /// "error" / "…"). Populated from the `action/dc/vm-bulk` reply's per-item
    /// results so the operator sees which VMs succeeded.
    pub bulk_progress: std::collections::BTreeMap<String, String>,
    /// DATACENTER-11 — the golden-template create wizard form, open when `true`.
    pub create_open: bool,
    /// The golden template name the new VM clones from.
    pub create_template: String,
    /// The new VM's name.
    pub create_name: String,

    // ── DATACENTER-10/16 — host lifecycle + energy-aware power ────────────────
    /// When `Some((host, op))`, a destructive host op (evacuate) is awaiting
    /// confirmation — only Confirm fires the RPC.
    pub host_confirm: Option<(String, String)>,
    /// When `Some((host, vms))`, a host patch impact-preview is shown — the VMs
    /// that would move (evacuate-first) before the patch proceeds.
    pub host_patch_preview: Option<(String, Vec<String>)>,
    /// DATACENTER-16 — the idle-shutdown policy master toggle (a host with zero
    /// running VMs auto-powers-down when on).
    pub idle_policy_on: bool,
    /// DATACENTER-16 — live host wake/power phases read off `event/dc/power/*`,
    /// rendered as phased progress bars on the Hosts tab. Refreshed each load.
    pub power: Vec<PowerStatus>,

    // ── DATACENTER-12 — storage ───────────────────────────────────────────────
    /// The scheduled-snapshot retention (count) input for an SR.
    pub sr_retention: String,
    /// When `Some(sr)`, an SR destroy is awaiting confirmation.
    pub sr_confirm_destroy: Option<String>,

    // ── DATACENTER-13 — network + unified IP/DNS ──────────────────────────────
    /// The VLAN tag input for the `action/dc/vlan-set` control.
    pub vlan_input: String,
    /// The correlated UniFi-lease ↔ DO-DNS ↔ overlay-IP rows from the last
    /// `action/dc/ipdns` read, rendered as a unified table on the Network tab.
    pub ipdns: Vec<IpDnsEntry>,

    // ── DATACENTER-14 — gateway firewall / port-forward edits ─────────────────
    /// The firewall-rule text being authored for `action/dc/gateway-firewall`.
    pub gw_fw_rule: String,
    /// The port-forward text being authored for `action/dc/gateway-portforward`.
    pub gw_pf_fwd: String,

    // ── DATACENTER-15 — Tofu prod-arm + persisted run-log ─────────────────────
    /// The prod-arm master switch — Tofu apply against the Prod (zone1-do)
    /// workspace is refused while disarmed (DATACENTER-15/20 prod guardrail).
    pub tofu_armed: bool,
    /// The persisted run-log text from the last `action/dc/tofu-runlog` read.
    pub tofu_runlog: String,

    // ── DATACENTER-20 — promotion controls ────────────────────────────────────
    /// Auto-promote-on-green toggle (Build→Eagle advances automatically).
    pub auto_promote: bool,
    /// The DO-step prod-arm toggle (armed = green auto-promotes to DO).
    pub promote_armed: bool,

    // ── DATACENTER-18/19/21 — provisioning (regions / genesis / test-mesh) ────
    /// The DO regions from the last `action/dc/do-regions` read (region picker).
    pub do_regions: Vec<DoRegion>,
    /// The region slug picked for the guided new-lighthouse flow.
    pub region_slug: Option<String>,
    /// The new-mesh (genesis) name input.
    pub genesis_name: String,
    /// The new-mesh (genesis) region picked.
    pub genesis_region: Option<String>,
    /// The N-node count input for the ephemeral test-mesh spin.
    pub testmesh_n: String,
    /// The desired build-VM count input for the farm-scale control.
    pub farm_scale_n: String,
    /// A shared status line for the Provision view's long-running flows.
    pub provision_status: String,
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
    /// The structured infrastructure map: resources grouped by their owning
    /// host/zone, with collapsible host group headers (DATACENTER-13).
    Topology,
    /// DATACENTER-14 — the UniFi gateway controls (status + firewall + port-
    /// forward edits + reboot).
    Gateway,
    /// DATACENTER-18/19/21 — provisioning flows: the DO region picker + guided
    /// new-lighthouse, the New-Mesh genesis wizard, and the ephemeral test-mesh +
    /// build-farm scale controls.
    Provision,
}

impl ViewMode {
    /// A stable lowercase slug for persistence — the on-disk saved-view record
    /// names the view mode by this string (NOT the `Debug` name, so a future
    /// rename of the variant can't silently invalidate a saved file). Pure.
    #[must_use]
    pub fn slug(self) -> &'static str {
        match self {
            ViewMode::Overview => "overview",
            ViewMode::Zone => "resources",
            ViewMode::Tofu => "tofu",
            ViewMode::Audit => "audit",
            ViewMode::Topology => "topology",
            ViewMode::Gateway => "gateway",
            ViewMode::Provision => "provision",
        }
    }

    /// The inverse of [`ViewMode::slug`] — recover a view mode from a persisted
    /// slug. An unrecognized slug (a file from a newer build, or corruption)
    /// falls back to `Overview`, the safe default landing view, rather than
    /// dropping the saved view. Pure.
    #[must_use]
    pub fn from_slug(slug: &str) -> ViewMode {
        match slug {
            "resources" => ViewMode::Zone,
            "tofu" => ViewMode::Tofu,
            "audit" => ViewMode::Audit,
            "topology" => ViewMode::Topology,
            "gateway" => ViewMode::Gateway,
            "provision" => ViewMode::Provision,
            // "overview" and anything unknown.
            _ => ViewMode::Overview,
        }
    }
}

/// DATACENTER-8 (saved views) — the largest number of saved views kept. A view is
/// a tiny record (a name + three short strings), so the cap is generous; it only
/// bounds an accidental unbounded-growth footgun and keeps the restore bar from
/// overflowing the header. The oldest (front) entry is evicted when a new save
/// would exceed it.
pub const SAVED_VIEW_CAP: usize = 12;

/// DATACENTER-8 (saved views) — one named, restorable snapshot of the panel's
/// view selectors: the top-level [`ViewMode`], the active zone tab, and the global
/// search needle. Saving the current view captures these three; applying a saved
/// view restores them. This is purely the operator's local UI state (which slice
/// of the datacenter they like to land on) — it carries no infra coupling and no
/// Bus data, so it persists in the local config dir, not on the mesh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedView {
    /// The operator-given name shown on the restore chip (e.g. "Prod VMs").
    pub name: String,
    /// The view mode this snapshot lands on, by [`ViewMode::slug`].
    pub view_mode: String,
    /// The zone tab this snapshot selects ("prod" | "dev").
    pub zone_tab: String,
    /// The global search needle this snapshot restores (may be empty).
    pub filter: String,
}

impl SavedView {
    /// The [`ViewMode`] this saved view lands on, decoded from its slug.
    #[must_use]
    pub fn mode(&self) -> ViewMode {
        ViewMode::from_slug(&self.view_mode)
    }
}

/// DATACENTER-8 (saved views) — the operator's collection of named saved views,
/// in save order. Pure value type: add/remove/find are total functions with no
/// I/O (persistence lives in [`load_saved_views`]/[`save_saved_views`]), so the
/// whole thing is unit-testable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedViews {
    pub views: Vec<SavedView>,
}

impl SavedViews {
    /// Upsert a saved view by name (case-insensitive): a save under an existing
    /// name overwrites it in place (so re-saving "Prod VMs" updates rather than
    /// duplicates); a new name appends. Appending past [`SAVED_VIEW_CAP`] evicts
    /// the oldest. A blank/whitespace name is rejected (returns `false`,
    /// unchanged) so the restore bar never grows an unnamed chip. Pure.
    pub fn upsert(&mut self, view: SavedView) -> bool {
        let name = view.name.trim();
        if name.is_empty() {
            return false;
        }
        if let Some(existing) = self
            .views
            .iter_mut()
            .find(|v| v.name.eq_ignore_ascii_case(name))
        {
            *existing = view;
        } else {
            self.views.push(view);
            while self.views.len() > SAVED_VIEW_CAP {
                self.views.remove(0);
            }
        }
        true
    }

    /// Remove the saved view with this name (case-insensitive). Returns whether a
    /// view was removed. Pure.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.views.len();
        self.views.retain(|v| !v.name.eq_ignore_ascii_case(name));
        self.views.len() != before
    }

    /// Find a saved view by name (case-insensitive). Pure.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&SavedView> {
        self.views
            .iter()
            .find(|v| v.name.eq_ignore_ascii_case(name))
    }

    /// Whether there are no saved views (drives the restore bar's empty hint).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }
}

/// DATACENTER-8 (saved views) — the local config-file path the saved views
/// persist to, mirroring the workbench's established `$XDG_CONFIG_HOME/mde/…`
/// convention (the same root `panels/mesh_bus.rs` uses for its bus-hooks file).
/// `None` only when neither `XDG_CONFIG_HOME` nor `HOME` is set (a degenerate
/// headless env), in which case saved views are session-only.
#[must_use]
pub fn saved_views_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|d| d.join("mde").join("datacenter-views.json"))
}

/// DATACENTER-8 (saved views) — read the persisted saved views from the local
/// config file. The file is tiny (a handful of short records).
///
/// The two "no data" cases yield an empty collection, NOT an error — a first run
/// (no file yet) and an unparseable/corrupt body both legitimately mean "no saved
/// views". But a genuine read failure on an *existing* file (a permission error,
/// a transient I/O fault) returns `Err`: the caller must NOT treat that as "empty"
/// and then overwrite the still-on-disk file on the next save, which would lose
/// the operator's views (the code-review data-loss path). The caller keeps its
/// current (unloaded) collection and surfaces the error instead.
pub fn load_saved_views() -> Result<SavedViews, String> {
    let Some(path) = saved_views_path() else {
        // No config dir at all (HOME unset) — there can be no file, so "empty".
        return Ok(SavedViews::default());
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            // An empty or unparseable body is "no saved views", not an error —
            // a corrupt file shouldn't block the panel or wedge a save.
            Ok(serde_json::from_str(&text).unwrap_or_default())
        }
        // A missing file is the normal first-run case → empty.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(SavedViews::default()),
        // A real read error on an existing file → surface it; do NOT silently
        // empty (which a later save would overwrite as data loss).
        Err(e) => Err(e.to_string()),
    }
}

/// DATACENTER-8 (saved views) — persist the saved views to the local config file,
/// creating the `mde/` config dir if needed. Best-effort: returns the error text
/// on failure so the caller can surface it in the panel status line, but a failed
/// write never loses the in-memory collection (the next save retries).
pub fn save_saved_views(views: &SavedViews) -> Result<(), String> {
    let path = saved_views_path().ok_or_else(|| "no config dir (HOME unset)".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(views).map_err(|e| e.to_string())?;
    std::fs::write(&path, json.as_bytes()).map_err(|e| e.to_string())
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
            tofu_state_resources: Vec::new(),
            tofu_state_drift: false,
            tofu_state_ws: String::new(),
            confirm_delete: None,
            audit: Vec::new(),
            promote: Vec::new(),
            health: Vec::new(),
            jobs: Vec::new(),
            tofu_confirm: None,
            expanded: BTreeSet::new(),
            topology_seeded: false,
            dr_status: String::new(),
            dr_confirm: false,
            filter: String::new(),
            reveal_start: None,
            selected_card: None,
            selection: Animator::new(),
            hovered_card: None,
            hover_since: Instant::now(),
            motion_ticking: false,
            history: VecDeque::new(),
            // DATACENTER-8 — saved views hydrate lazily off-thread on the first
            // panel-open `load()` (see `Message::SavedViewsLoaded`), keeping the
            // constructor pure + non-blocking like the rest of the panel.
            saved_views: SavedViews::default(),
            save_view_name: String::new(),
            views_loaded: false,
            kind_tab: KindTab::Vms,
            migrate: None,
            migrate_target: None,
            bulk_mode: false,
            bulk_sel: BTreeSet::new(),
            bulk_progress: std::collections::BTreeMap::new(),
            create_open: false,
            create_template: String::new(),
            create_name: String::new(),
            host_confirm: None,
            host_patch_preview: None,
            idle_policy_on: false,
            power: Vec::new(),
            sr_retention: String::new(),
            sr_confirm_destroy: None,
            vlan_input: String::new(),
            ipdns: Vec::new(),
            gw_fw_rule: String::new(),
            gw_pf_fwd: String::new(),
            tofu_armed: false,
            tofu_runlog: String::new(),
            auto_promote: false,
            promote_armed: false,
            do_regions: Vec::new(),
            region_slug: None,
            genesis_name: String::new(),
            genesis_region: None,
            testmesh_n: String::new(),
            farm_scale_n: String::new(),
            provision_status: String::new(),
        }
    }
}

/// The payload of a successful [`Message::Loaded`] — the projected resource rows
/// plus the audit-log rows, both read from the Bus in one pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DcLoad {
    pub rows: Vec<DcRow>,
    pub audit: Vec<AuditRow>,
    /// The Build → Eagle → DO promotion stages read off `event/dc/promote/*`.
    /// Rendered as a version matrix on the Overview view. Refreshed alongside
    /// `rows` on every load.
    pub promote: Vec<PromoteStage>,
    /// The datacenter health checks read off `event/dc/health/*`. Rendered as an
    /// ok/warn/fail summary + alert list on the Overview view. Refreshed
    /// alongside `rows` on every load.
    pub health: Vec<HealthCheck>,
    /// The datacenter action jobs read off `event/dc/job/*`. Filtered to the Tofu
    /// verbs for the "Recent Tofu runs" run-log on the Overview view. Refreshed
    /// alongside `rows` on every load.
    pub jobs: Vec<JobRow>,
    /// DATACENTER-16 — the live host wake/power phases read off `event/dc/power/*`.
    /// Rendered as phased progress bars on the Hosts tab. Refreshed alongside
    /// `rows` on every load.
    pub power: Vec<PowerStatus>,
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
    /// A `Topology`-view group header was clicked — toggle that group's
    /// expanded/collapsed state. The payload is the group key (a host `id`, or
    /// the empty string for the synthetic Prod/Gateway group).
    HeaderClicked(String),
    /// A Tofu "State" button was clicked. The payload is the workspace name
    /// ("xen-xapi" | "zone1-do"). Fires the `action/dc/tofu-state` RPC, which
    /// returns the workspace's managed resources + a drift flag.
    TofuStateClicked(String),
    /// The `action/dc/tofu-state` RPC came back — `Ok` carries the managed
    /// resource names + the drift flag, `Err` the error text. Routes here as a
    /// panel-scoped message.
    TofuStateDone(Result<(Vec<String>, bool), String>),
    /// The Overview "Back up now" button was clicked. This only *arms* the
    /// typed-confirm (`dr_confirm = true`); no RPC fires until the inline
    /// "Backup state + secrets? [Confirm]" button is pressed.
    DrBackupClicked,
    /// The inline confirm for a DR backup was pressed — only this fires the
    /// `action/dc/dr-backup` RPC (with `"confirm":true`).
    DrBackup,
    /// The pending DR-backup confirmation was dismissed (the "Cancel" button) —
    /// clears `dr_confirm` without firing any RPC.
    DrBackupCancelled,
    /// The `action/dc/dr-backup` RPC came back — `Ok` carries the backup path,
    /// `Err` the error text. Routes here as a panel-scoped message.
    DrBackupDone(Result<String, String>),
    /// The global search box's contents changed — store the new needle, which the
    /// view applies as a case-insensitive name/id/kind filter across the rendered
    /// resources. Pure state update; fires no RPC.
    FilterChanged(String),
    /// MOTION-FEEDBACK-2 — a resource card was clicked: select it (and re-arm its
    /// animated accent ring). `String` is the card's resource id. Pure state +
    /// motion update; fires no RPC.
    CardSelected(String),
    /// MOTION-FEEDBACK-2 — the pointer entered (`Some(id)`) or left (`None`) a card.
    /// Drives the per-card hover-lift tween. Pure state + motion update.
    CardHovered(Option<String>),
    /// MOTION-FEEDBACK-2 — one frame of the card-grid reveal / selection tween. A
    /// self-re-arming tick (see [`DatacenterPanel::tick_motion`]) that runs ONLY
    /// while a reveal or the selection accent is in flight, then stops (no idle
    /// wakeups). Pure: re-renders + advances/garbage-collects the tweens.
    MotionTick,
    /// DATACENTER-8 (saved views) — the "Save view as…" name box changed. Pure
    /// state; fires no I/O.
    SaveViewNameChanged(String),
    /// DATACENTER-8 (saved views) — the "Save" button was clicked: capture the
    /// current view mode + zone tab + search needle under the box's name, persist
    /// the collection, and clear the box. A blank name is a no-op.
    SaveCurrentView,
    /// DATACENTER-8 (saved views) — a saved-view restore chip was clicked: apply
    /// that view's mode + zone tab + filter. Payload is the view's name. Pure
    /// state restore; fires no RPC (it re-points the existing Bus reads).
    ApplyView(String),
    /// DATACENTER-8 (saved views) — a saved view's delete affordance was clicked:
    /// remove it and persist. Payload is the view's name.
    DeleteView(String),
    /// DATACENTER-8 (saved views) — the off-thread saved-views file read finished
    /// (fired once by the first panel-open `load()`). `Ok` carries the loaded
    /// collection; `Err` carries a real read error (an existing file that couldn't
    /// be read), which is surfaced rather than silently emptied — so a later save
    /// can't overwrite a still-on-disk file. Only the FIRST load is applied.
    SavedViewsLoaded(Result<SavedViews, String>),

    // ── DATACENTER-8/1813 — per-kind sub-tabs ─────────────────────────────────
    /// Switch the Zone view's per-kind sub-tab (Hosts / VMs / Storage / Network).
    KindTab(KindTab),

    // ── DATACENTER-11 — VM lifecycle ──────────────────────────────────────────
    /// A VM Suspend (or Resume, when `resume`) button — fires `action/dc/vm-suspend`
    /// / `action/dc/vm-resume`.
    SuspendClicked {
        uuid: String,
        dom0: String,
        resume: bool,
    },
    /// The vm-suspend / vm-resume RPC came back.
    SuspendDone(Result<String, String>),
    /// A VM Migrate button — arms the target picker for this VM (no RPC yet).
    MigrateClicked {
        uuid: String,
        dom0: String,
    },
    /// The migrate target host was picked in the dropdown.
    MigrateTargetPicked(String),
    /// The migrate Confirm button — fires `action/dc/vm-migrate {uuid,target_host}`.
    MigrateConfirmed,
    /// The migrate target picker was dismissed.
    MigrateCancelled,
    /// The vm-migrate RPC came back.
    MigrateDone(Result<String, String>),
    /// A VM Console button — fires `action/dc/vm-console {uuid}` → a noVNC URL,
    /// opened via `xdg-open`.
    VmConsoleClicked {
        uuid: String,
        dom0: String,
    },
    /// The vm-console RPC came back — `Ok(url)` is opened in the browser.
    VmConsoleDone(Result<String, String>),
    /// Toggle the VMs tab's multi-select bulk mode on/off.
    BulkModeToggled,
    /// Toggle a VM uuid's membership in the bulk selection.
    BulkSelectToggled(String),
    /// A bulk action button — fires `action/dc/vm-bulk {uuids[],op}` for the
    /// selected VMs (op = "start" | "shutdown" | "snapshot" | "tag").
    BulkOp(String),
    /// The vm-bulk RPC came back — `Ok` carries the per-item `(uuid,status)` results.
    BulkDone(Result<Vec<(String, String)>, String>),
    /// Open / close the golden-template create wizard.
    CreateToggled,
    /// The create wizard's template field changed.
    CreateTemplateChanged(String),
    /// The create wizard's name field changed.
    CreateNameChanged(String),
    /// The create Submit button — fires `action/dc/vm-create {template,name,zone}`.
    CreateSubmit,
    /// The vm-create RPC came back.
    CreateDone(Result<String, String>),

    // ── DATACENTER-10/16 — host lifecycle + power ─────────────────────────────
    /// A host maintenance / reboot button — fires `action/dc/host-power {host,op}`
    /// (op = "maintenance-on" | "maintenance-off" | "reboot").
    HostPowerClicked {
        host: String,
        op: String,
    },
    /// A host Evacuate button — arms the confirm (no RPC yet).
    HostEvacuateClicked(String),
    /// A host Patch button — fetches the impact preview, then `action/dc/host-patch`.
    HostPatchClicked(String),
    /// The host-patch impact preview came back (`(host, vms-that-would-move)`).
    HostPatchPreviewDone(Result<(String, Vec<String>), String>),
    /// A host pool op — fires `action/dc/host-pool {op,host}` (op = "join" |
    /// "eject" | "designate-master").
    HostPoolClicked {
        host: String,
        op: String,
    },
    /// A host SSH console button — launches a terminal to the host (reuses the
    /// shared launcher).
    HostConsoleClicked(String),
    /// An IPMI op button — fires `action/dc/power-ipmi {host,op}` (op = "on" |
    /// "off" | "cycle" | "status").
    PowerIpmiClicked {
        host: String,
        op: String,
    },
    /// Toggle the idle-shutdown policy — fires `action/dc/host-idle-policy {on}`.
    IdlePolicyToggled,
    /// The host-confirm Confirm button (evacuate) — fires the armed RPC.
    HostConfirmed,
    /// The host-confirm / patch-preview was dismissed.
    HostCancelled,
    /// A generic host-action RPC came back (evacuate / patch / pool / ipmi /
    /// idle-policy).
    HostActionDone(Result<String, String>),

    // ── DATACENTER-12 — storage ───────────────────────────────────────────────
    /// An SR create button — fires `action/dc/sr-create`.
    SrCreateClicked,
    /// An SR destroy button — arms the confirm (no RPC yet).
    SrDestroyClicked(String),
    /// The SR destroy Confirm button — fires `action/dc/sr-destroy`.
    SrDestroyConfirmed(String),
    /// The SR destroy was dismissed.
    SrDestroyCancelled,
    /// A VDI attach / detach button — fires `action/dc/vdi-attach` / `vdi-detach`.
    VdiClicked {
        sr: String,
        attach: bool,
    },
    /// The scheduled-snapshot retention input changed.
    SrRetentionChanged(String),
    /// The Schedule button — fires `action/dc/sr-snapshot-schedule {sr,retention}`.
    SrScheduleClicked(String),
    /// A generic storage-action RPC came back.
    StorageActionDone(Result<String, String>),

    // ── DATACENTER-13 — network + IP/DNS ──────────────────────────────────────
    /// A network create button — fires `action/dc/net-create`.
    NetCreateClicked,
    /// The VLAN tag input changed.
    VlanInputChanged(String),
    /// The VLAN set button — fires `action/dc/vlan-set {net,vlan}`.
    VlanSetClicked(String),
    /// A PIF config button — fires `action/dc/pif-config {net}`.
    PifConfigClicked(String),
    /// The "Refresh IP/DNS" button — fires `action/dc/ipdns`.
    IpDnsRefresh,
    /// The ipdns RPC came back — the correlated lease/DNS/overlay rows.
    IpDnsDone(Result<Vec<IpDnsEntry>, String>),
    /// A generic network-action RPC came back.
    NetActionDone(Result<String, String>),

    // ── DATACENTER-14 — gateway edits ─────────────────────────────────────────
    /// The gateway firewall-rule input changed.
    GwFwRuleChanged(String),
    /// The Add-firewall-rule button — fires `action/dc/gateway-firewall {rule}`.
    GwFirewallClicked,
    /// The gateway port-forward input changed.
    GwPfFwdChanged(String),
    /// The Add-port-forward button — fires `action/dc/gateway-portforward {fwd}`.
    GwPortForwardClicked,
    /// The gateway Reboot button — fires `action/dc/gateway-reboot` (confirm-gated
    /// reuse of the host-confirm path).
    GwRebootClicked,
    /// A generic gateway-action RPC came back.
    GwActionDone(Result<String, String>),

    // ── DATACENTER-15 — Tofu prod-arm + run-log ───────────────────────────────
    /// Toggle the prod-arm master switch — fires `action/dc/tofu-arm {on}`.
    TofuArmToggled,
    /// The tofu-arm RPC came back (`Ok(armed)`).
    TofuArmDone(Result<bool, String>),
    /// A workspace Run-log button — fires `action/dc/tofu-runlog {workspace}`.
    TofuRunlogClicked(String),
    /// The tofu-runlog RPC came back — the persisted log text.
    TofuRunlogDone(Result<String, String>),

    // ── DATACENTER-20 — promotion controls ────────────────────────────────────
    /// Toggle auto-promote-on-green — fires `action/dc/promote-arm {on}` (auto).
    AutoPromoteToggled,
    /// Toggle the DO-step prod-arm — fires `action/dc/promote-arm {on}` (prod).
    PromoteArmToggled,
    /// The "Promote now" button — fires `action/dc/promote-now`.
    PromoteNowClicked,
    /// A promote-control RPC came back (`Ok(state-summary)`).
    PromoteCtlDone(Result<String, String>),

    // ── DATACENTER-18/19/21 — provisioning ────────────────────────────────────
    /// The "Load regions" button — fires `action/dc/do-regions`.
    RegionsRefresh,
    /// The do-regions RPC came back.
    RegionsDone(Result<Vec<DoRegion>, String>),
    /// A DO region was picked for the guided new-lighthouse flow.
    RegionPicked(String),
    /// The "Guided new lighthouse" button — fires `action/dc/do-guided-lighthouse`.
    GuidedLighthouseClicked,
    /// The genesis name input changed.
    GenesisNameChanged(String),
    /// A genesis region was picked.
    GenesisRegionPicked(String),
    /// The genesis "Give birth" button — fires `action/dc/genesis-new-mesh`.
    GenesisSubmit,
    /// The test-mesh N-node input changed.
    TestmeshNChanged(String),
    /// The test-mesh Spin button — fires `action/dc/testmesh-spin {n}`.
    TestmeshSpinClicked,
    /// A test-mesh Teardown button — fires `action/dc/testmesh-teardown {id}`.
    TestmeshTeardownClicked(String),
    /// The farm-scale count input changed.
    FarmScaleChanged(String),
    /// The farm-scale Apply button — fires `action/dc/farm-scale {count}`.
    FarmScaleClicked,
    /// A generic provisioning-action RPC came back.
    ProvisionDone(Result<String, String>),
}

impl DatacenterPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Push one rolling-history sample onto the capped ring buffer, evicting the
    /// oldest when it would overflow [`HISTORY_CAP`]. Called on each Bus load so
    /// the Overview's sparklines plot the recent trend. Pure state — no I/O.
    pub fn push_sample(&mut self, sample: HistorySample) {
        if self.history.len() >= HISTORY_CAP {
            self.history.pop_front();
        }
        self.history.push_back(sample);
    }

    /// The rolling-history samples, oldest-first — the series the Overview's
    /// sparklines plot. Borrowed read-only.
    #[must_use]
    pub fn history(&self) -> &VecDeque<HistorySample> {
        &self.history
    }

    /// Read the `event/dc/*` topics off the Bus + project them into rows.
    pub fn load() -> Task<crate::Message> {
        // The Bus `event/dc/*` read, plus a one-shot saved-views file read — both
        // off the GUI thread (the panel's lazy-load convention; `Default`/init
        // stays pure). The saved-views handler ignores all but the first load, so
        // batching it on every panel-open is harmless.
        let bus = Task::perform(
            async move { Message::Loaded(read_dc_events()) },
            crate::Message::Datacenter,
        );
        let views = Task::perform(
            async move { Message::SavedViewsLoaded(load_saved_views()) },
            crate::Message::Datacenter,
        );
        Task::batch([bus, views])
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(Ok(load)) => {
                self.rows = load.rows;
                self.audit = load.audit;
                self.promote = load.promote;
                self.health = load.health;
                self.jobs = load.jobs;
                self.power = load.power;
                self.busy = false;
                self.load_error = None;
                self.status.clear();
                // DATACENTER-9 — record this load as one rolling-history sample so
                // the Overview's sparklines plot the recent resource / health
                // trend. Captured off the just-assigned rows + health checks.
                self.push_sample(HistorySample::capture(&self.rows, &self.health));
                // A fresh projection may not include the row pending a delete —
                // drop the stale confirm prompt rather than leave it dangling.
                self.confirm_delete = None;
                // Likewise drop a stale tofu-apply confirm on a refresh.
                self.tofu_confirm = None;
                // A fresh row set: re-seed the Topology expansion so newly-arrived
                // host groups open by default. If we're already on the Topology
                // view, seed eagerly (the view borrows `&self` and can't); other-
                // wise it seeds when the view is next selected.
                self.topology_seeded = false;
                if self.view_mode == ViewMode::Topology {
                    self.ensure_topology_seeded();
                }
                // MOTION-FEEDBACK-2 — a fresh row set re-reveals the card grid: stamp
                // the reveal origin + arm the tick loop so the cards stagger in. A
                // selection on a now-absent resource is dropped so a stale accent
                // never lingers.
                if self
                    .selected_card
                    .as_deref()
                    .is_some_and(|id| !self.rows.iter().any(|r| r.id == id))
                {
                    self.selected_card = None;
                    self.selection.gc(Instant::now());
                }
                self.begin_reveal()
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
                if mode == ViewMode::Topology {
                    self.ensure_topology_seeded();
                }
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
                // DATACENTER-15 prod guardrail — a Prod (zone1-do) apply is refused
                // while the prod-arm master switch is disarmed.
                if ws == "zone1-do" && !self.tofu_armed {
                    self.tofu_confirm = None;
                    self.status =
                        "Prod is disarmed — arm prod (Tofu tab) before applying zone1-do.".into();
                    return Task::none();
                }
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
            Message::HeaderClicked(key) => {
                // Toggle the group's expanded state. Membership = expanded.
                if !self.expanded.remove(&key) {
                    self.expanded.insert(key);
                }
                Task::none()
            }
            Message::TofuStateClicked(ws) => {
                // Record which workspace the in-flight state read is for so the
                // rendered list / drift badge can name it once the reply lands.
                self.tofu_state_ws = ws.clone();
                self.status = format!("Reading {ws} state…");
                Task::perform(
                    async move {
                        // Same shape as `tofu_plan`: the Bus RPC borrows a
                        // non-Send Persist across its internal await, so run the
                        // whole round trip on a blocking thread with a local
                        // runtime.
                        tokio::task::spawn_blocking(move || tofu_state(&ws))
                            .await
                            .unwrap_or_else(|e| Err(format!("tofu task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::TofuStateDone(result)),
                )
            }
            Message::TofuStateDone(Ok((resources, drift))) => {
                self.status = "State read complete".into();
                self.tofu_state_resources = resources;
                self.tofu_state_drift = drift;
                Task::none()
            }
            Message::TofuStateDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::DrBackupClicked => {
                // First click only arms the typed-confirm — no RPC fires until
                // the operator confirms.
                self.dr_confirm = true;
                self.dr_status = "Confirm backup below.".into();
                Task::none()
            }
            Message::DrBackup => {
                self.dr_confirm = false;
                self.dr_status = "Backing up…".into();
                Task::perform(
                    async move {
                        // Same shape as `tofu_apply`: the Bus RPC borrows a
                        // non-Send Persist across its internal await, so run the
                        // whole round trip on a blocking thread with a local
                        // runtime.
                        tokio::task::spawn_blocking(dr_backup)
                            .await
                            .unwrap_or_else(|e| Err(format!("dr-backup task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::DrBackupDone(result)),
                )
            }
            Message::DrBackupCancelled => {
                self.dr_confirm = false;
                self.dr_status.clear();
                Task::none()
            }
            Message::DrBackupDone(Ok(path)) => {
                self.dr_status = format!("backed up: {path}");
                Task::none()
            }
            Message::DrBackupDone(Err(e)) => {
                self.dr_status = e;
                Task::none()
            }
            Message::FilterChanged(needle) => {
                self.filter = needle;
                Task::none()
            }
            Message::CardSelected(id) => {
                // Toggle the selection (a second click on the focused card clears
                // it) and (re)arm its animated accent ring from now.
                if self.selected_card.as_deref() == Some(id.as_str()) {
                    self.selected_card = None;
                    // Drop the accent tween for this card so the deselect is a clean
                    // instant pop-out (the view already gates `ring_t` on
                    // `selected`) and no stale tween lingers in the Animator.
                    self.selection.gc(Instant::now());
                    Task::none()
                } else {
                    let now = Instant::now();
                    self.selection.start(
                        id.clone(),
                        now,
                        Motion::focus(),
                        crate::live_theme::reduce_motion(),
                    );
                    self.selected_card = Some(id);
                    self.arm_motion()
                }
            }
            Message::CardHovered(id) => {
                if self.hovered_card != id {
                    self.hovered_card = id;
                    self.hover_since = Instant::now();
                    // Animate the hover-lift in/out (a no-op tween under
                    // reduce-motion, where the lift is dropped anyway).
                    return self.arm_motion();
                }
                Task::none()
            }
            Message::MotionTick => self.tick_motion(),
            Message::SaveViewNameChanged(name) => {
                self.save_view_name = name;
                Task::none()
            }
            Message::SaveCurrentView => {
                let view = SavedView {
                    name: self.save_view_name.trim().to_string(),
                    view_mode: self.view_mode.slug().to_string(),
                    zone_tab: self.zone_tab.clone(),
                    filter: self.filter.clone(),
                };
                if self.saved_views.upsert(view) {
                    self.save_view_name.clear();
                    self.persist_saved_views();
                }
                Task::none()
            }
            Message::ApplyView(name) => {
                if let Some(v) = self.saved_views.find(&name) {
                    self.view_mode = v.mode();
                    self.zone_tab = v.zone_tab.clone();
                    self.filter = v.filter.clone();
                    // A restored Topology view needs its group headers seeded, the
                    // same as a direct ViewMode switch into Topology.
                    if self.view_mode == ViewMode::Topology {
                        self.ensure_topology_seeded();
                    }
                }
                Task::none()
            }
            Message::DeleteView(name) => {
                if self.saved_views.remove(&name) {
                    self.persist_saved_views();
                }
                Task::none()
            }
            Message::SavedViewsLoaded(result) => {
                // Apply only the FIRST load: once the operator has the panel open,
                // a Bus refresh (which re-batches this read) must not clobber any
                // in-memory saved-view edits they've made since.
                if self.views_loaded {
                    return Task::none();
                }
                match result {
                    Ok(views) => {
                        self.saved_views = views;
                        self.views_loaded = true;
                    }
                    // A real read error on an existing file: keep the (empty)
                    // in-memory set but do NOT mark loaded, so a later save can't
                    // overwrite the still-on-disk file as a side effect — and a
                    // subsequent panel-open retries the read.
                    Err(e) => {
                        self.status = format!("Couldn't read saved views: {e}");
                    }
                }
                Task::none()
            }

            // ── DATACENTER-8/1813 — per-kind sub-tabs ─────────────────────────
            Message::KindTab(t) => {
                self.kind_tab = t;
                // Leaving the VMs tab drops any in-progress bulk selection so a
                // stale set can't act on the wrong tab.
                if t != KindTab::Vms {
                    self.bulk_mode = false;
                    self.bulk_sel.clear();
                }
                Task::none()
            }

            // ── DATACENTER-11 — VM lifecycle ──────────────────────────────────
            Message::SuspendClicked {
                uuid,
                dom0,
                resume,
            } => {
                self.status = if resume { "Resuming…" } else { "Suspending…" }.into();
                let verb = if resume { "vm-resume" } else { "vm-suspend" };
                self.run_action(
                    verb,
                    serde_json::json!({"uuid": uuid, "dom0": dom0}),
                    Duration::from_secs(60),
                    Message::SuspendDone,
                )
            }
            Message::SuspendDone(r) => {
                self.status = flatten(r);
                Task::none()
            }
            Message::MigrateClicked { uuid, dom0 } => {
                self.migrate = Some((uuid, dom0));
                self.migrate_target = None;
                self.status = "Pick a migrate target host.".into();
                Task::none()
            }
            Message::MigrateTargetPicked(h) => {
                self.migrate_target = Some(h);
                Task::none()
            }
            Message::MigrateConfirmed => {
                let Some((uuid, _dom0)) = self.migrate.clone() else {
                    return Task::none();
                };
                let Some(target) = self.migrate_target.clone() else {
                    self.status = "Pick a target host first.".into();
                    return Task::none();
                };
                self.migrate = None;
                self.migrate_target = None;
                self.status = format!("Migrating to {target}…");
                self.run_action(
                    "vm-migrate",
                    serde_json::json!({"uuid": uuid, "target_host": target}),
                    Duration::from_secs(600),
                    Message::MigrateDone,
                )
            }
            Message::MigrateCancelled => {
                self.migrate = None;
                self.migrate_target = None;
                self.status.clear();
                Task::none()
            }
            Message::MigrateDone(r) => {
                self.status = flatten(r);
                Task::none()
            }
            Message::VmConsoleClicked { uuid, dom0 } => {
                self.status = "Opening console…".into();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || vm_console(&uuid, &dom0))
                            .await
                            .unwrap_or_else(|e| Err(format!("console task panicked: {e}")))
                    },
                    |r| crate::Message::Datacenter(Message::VmConsoleDone(r)),
                )
            }
            Message::VmConsoleDone(Ok(url)) => {
                // Open the noVNC console URL in the browser (detached, best-effort —
                // a missing xdg-open simply no-ops).
                let _ = std::process::Command::new("xdg-open")
                    .arg(&url)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                self.status = format!("console: {url}");
                Task::none()
            }
            Message::VmConsoleDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::BulkModeToggled => {
                self.bulk_mode = !self.bulk_mode;
                if !self.bulk_mode {
                    self.bulk_sel.clear();
                }
                self.bulk_progress.clear();
                Task::none()
            }
            Message::BulkSelectToggled(uuid) => {
                if !self.bulk_sel.remove(&uuid) {
                    self.bulk_sel.insert(uuid);
                }
                Task::none()
            }
            Message::BulkOp(op) => {
                if self.bulk_sel.is_empty() {
                    self.status = "No VMs selected.".into();
                    return Task::none();
                }
                let uuids: Vec<String> = self.bulk_sel.iter().cloned().collect();
                // Seed per-item progress so each selected VM reads "…" until its
                // result lands.
                self.bulk_progress = uuids.iter().map(|u| (u.clone(), "…".to_string())).collect();
                self.status = format!("Bulk {op} on {} VM(s)…", uuids.len());
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || vm_bulk(&uuids, &op))
                            .await
                            .unwrap_or_else(|e| Err(format!("bulk task panicked: {e}")))
                    },
                    |r| crate::Message::Datacenter(Message::BulkDone(r)),
                )
            }
            Message::BulkDone(Ok(results)) => {
                for (uuid, status) in results {
                    self.bulk_progress.insert(uuid, status);
                }
                self.status = "Bulk action complete".into();
                Task::none()
            }
            Message::BulkDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::CreateToggled => {
                self.create_open = !self.create_open;
                Task::none()
            }
            Message::CreateTemplateChanged(v) => {
                self.create_template = v;
                Task::none()
            }
            Message::CreateNameChanged(v) => {
                self.create_name = v;
                Task::none()
            }
            Message::CreateSubmit => {
                let template = self.create_template.trim().to_string();
                let name = self.create_name.trim().to_string();
                if template.is_empty() || name.is_empty() {
                    self.status = "Template and name are both required.".into();
                    return Task::none();
                }
                let zone = self.zone_tab.clone();
                self.status = format!("Creating {name} from {template}…");
                self.create_open = false;
                self.run_action(
                    "vm-create",
                    serde_json::json!({"template": template, "name": name, "zone": zone}),
                    Duration::from_secs(600),
                    Message::CreateDone,
                )
            }
            Message::CreateDone(r) => {
                self.status = flatten(r);
                Task::none()
            }

            // ── DATACENTER-10/16 — host lifecycle + power ─────────────────────
            Message::HostPowerClicked { host, op } => {
                self.status = format!("Host {op}…");
                self.run_action(
                    "host-power",
                    serde_json::json!({"host": host, "op": op}),
                    Duration::from_secs(120),
                    Message::HostActionDone,
                )
            }
            Message::HostEvacuateClicked(host) => {
                self.host_confirm = Some((host, "evacuate".to_string()));
                self.host_patch_preview = None;
                self.status = "Confirm evacuate below.".into();
                Task::none()
            }
            Message::HostPatchClicked(host) => {
                self.status = format!("Computing patch impact for {host}…");
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || host_patch_preview(&host))
                            .await
                            .unwrap_or_else(|e| Err(format!("patch task panicked: {e}")))
                    },
                    |r| crate::Message::Datacenter(Message::HostPatchPreviewDone(r)),
                )
            }
            Message::HostPatchPreviewDone(Ok((host, vms))) => {
                self.host_patch_preview = Some((host.clone(), vms));
                self.host_confirm = Some((host, "patch".to_string()));
                self.status = "Review the patch impact, then Apply.".into();
                Task::none()
            }
            Message::HostPatchPreviewDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::HostPoolClicked { host, op } => {
                self.status = format!("Pool {op}…");
                self.run_action(
                    "host-pool",
                    serde_json::json!({"host": host, "op": op}),
                    Duration::from_secs(120),
                    Message::HostActionDone,
                )
            }
            Message::HostConsoleClicked(host) => {
                self.status = format!("Opening SSH to {host}…");
                Task::perform(
                    async move {
                        let ok =
                            crate::launcher::launch(&host, crate::launcher::Protocol::Ssh).await;
                        if ok {
                            Ok(format!("opened SSH to {host}"))
                        } else {
                            Err(format!("couldn't launch a terminal for {host}"))
                        }
                    },
                    |r| crate::Message::Datacenter(Message::HostActionDone(r)),
                )
            }
            Message::PowerIpmiClicked { host, op } => {
                self.status = format!("IPMI {op}…");
                self.run_action(
                    "power-ipmi",
                    serde_json::json!({"host": host, "op": op}),
                    Duration::from_secs(120),
                    Message::HostActionDone,
                )
            }
            Message::IdlePolicyToggled => {
                let on = !self.idle_policy_on;
                self.idle_policy_on = on;
                self.run_action(
                    "host-idle-policy",
                    serde_json::json!({"on": on}),
                    Duration::from_secs(30),
                    Message::HostActionDone,
                )
            }
            Message::HostConfirmed => {
                let Some((host, op)) = self.host_confirm.take() else {
                    return Task::none();
                };
                self.host_patch_preview = None;
                let verb = if op == "patch" {
                    "host-patch"
                } else {
                    "host-evacuate"
                };
                self.status = format!("{op} {host}…");
                self.run_action(
                    verb,
                    serde_json::json!({"host": host, "confirm": true}),
                    Duration::from_secs(600),
                    Message::HostActionDone,
                )
            }
            Message::HostCancelled => {
                self.host_confirm = None;
                self.host_patch_preview = None;
                self.status.clear();
                Task::none()
            }
            Message::HostActionDone(r) => {
                self.status = flatten(r);
                Task::none()
            }

            // ── DATACENTER-12 — storage ───────────────────────────────────────
            Message::SrCreateClicked => {
                self.status = "Creating SR…".into();
                self.run_action(
                    "sr-create",
                    serde_json::json!({"zone": self.zone_tab}),
                    Duration::from_secs(120),
                    Message::StorageActionDone,
                )
            }
            Message::SrDestroyClicked(sr) => {
                self.sr_confirm_destroy = Some(sr);
                self.status = "Confirm SR destroy below.".into();
                Task::none()
            }
            Message::SrDestroyConfirmed(sr) => {
                self.sr_confirm_destroy = None;
                self.status = format!("Destroying {sr}…");
                self.run_action(
                    "sr-destroy",
                    serde_json::json!({"sr": sr, "confirm": true}),
                    Duration::from_secs(120),
                    Message::StorageActionDone,
                )
            }
            Message::SrDestroyCancelled => {
                self.sr_confirm_destroy = None;
                self.status.clear();
                Task::none()
            }
            Message::VdiClicked { sr, attach } => {
                let verb = if attach { "vdi-attach" } else { "vdi-detach" };
                self.status = if attach {
                    "Attaching VDI…"
                } else {
                    "Detaching VDI…"
                }
                .into();
                self.run_action(
                    verb,
                    serde_json::json!({"sr": sr}),
                    Duration::from_secs(120),
                    Message::StorageActionDone,
                )
            }
            Message::SrRetentionChanged(v) => {
                self.sr_retention = v;
                Task::none()
            }
            Message::SrScheduleClicked(sr) => {
                let retention = self.sr_retention.trim().parse::<u32>().unwrap_or(7);
                self.status = format!("Scheduling snapshots (keep {retention})…");
                self.run_action(
                    "sr-snapshot-schedule",
                    serde_json::json!({"sr": sr, "retention": retention}),
                    Duration::from_secs(60),
                    Message::StorageActionDone,
                )
            }
            Message::StorageActionDone(r) => {
                self.status = flatten(r);
                Task::none()
            }

            // ── DATACENTER-13 — network + IP/DNS ──────────────────────────────
            Message::NetCreateClicked => {
                self.status = "Creating network…".into();
                self.run_action(
                    "net-create",
                    serde_json::json!({"zone": self.zone_tab}),
                    Duration::from_secs(120),
                    Message::NetActionDone,
                )
            }
            Message::VlanInputChanged(v) => {
                self.vlan_input = v;
                Task::none()
            }
            Message::VlanSetClicked(net) => {
                let vlan = self.vlan_input.trim().parse::<u32>().unwrap_or(0);
                self.status = format!("Setting VLAN {vlan}…");
                self.run_action(
                    "vlan-set",
                    serde_json::json!({"net": net, "vlan": vlan}),
                    Duration::from_secs(60),
                    Message::NetActionDone,
                )
            }
            Message::PifConfigClicked(net) => {
                self.status = "Configuring PIF…".into();
                self.run_action(
                    "pif-config",
                    serde_json::json!({"net": net}),
                    Duration::from_secs(60),
                    Message::NetActionDone,
                )
            }
            Message::IpDnsRefresh => {
                self.status = "Reading IP/DNS…".into();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(ipdns_read)
                            .await
                            .unwrap_or_else(|e| Err(format!("ipdns task panicked: {e}")))
                    },
                    |r| crate::Message::Datacenter(Message::IpDnsDone(r)),
                )
            }
            Message::IpDnsDone(Ok(entries)) => {
                self.ipdns = entries;
                self.status = "IP/DNS updated".into();
                Task::none()
            }
            Message::IpDnsDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::NetActionDone(r) => {
                self.status = flatten(r);
                Task::none()
            }

            // ── DATACENTER-14 — gateway edits ─────────────────────────────────
            Message::GwFwRuleChanged(v) => {
                self.gw_fw_rule = v;
                Task::none()
            }
            Message::GwFirewallClicked => {
                let rule = self.gw_fw_rule.trim().to_string();
                if rule.is_empty() {
                    self.status = "Enter a firewall rule first.".into();
                    return Task::none();
                }
                self.status = "Adding firewall rule…".into();
                self.gw_fw_rule.clear();
                self.run_action(
                    "gateway-firewall",
                    serde_json::json!({"rule": rule}),
                    Duration::from_secs(60),
                    Message::GwActionDone,
                )
            }
            Message::GwPfFwdChanged(v) => {
                self.gw_pf_fwd = v;
                Task::none()
            }
            Message::GwPortForwardClicked => {
                let fwd = self.gw_pf_fwd.trim().to_string();
                if fwd.is_empty() {
                    self.status = "Enter a port-forward first.".into();
                    return Task::none();
                }
                self.status = "Adding port-forward…".into();
                self.gw_pf_fwd.clear();
                self.run_action(
                    "gateway-portforward",
                    serde_json::json!({"fwd": fwd}),
                    Duration::from_secs(60),
                    Message::GwActionDone,
                )
            }
            Message::GwRebootClicked => {
                self.status = "Rebooting gateway…".into();
                self.run_action(
                    "gateway-reboot",
                    serde_json::json!({"confirm": true}),
                    Duration::from_secs(120),
                    Message::GwActionDone,
                )
            }
            Message::GwActionDone(r) => {
                self.status = flatten(r);
                Task::none()
            }

            // ── DATACENTER-15 — Tofu prod-arm + run-log ───────────────────────
            Message::TofuArmToggled => {
                let on = !self.tofu_armed;
                self.status = if on { "Arming prod…" } else { "Disarming prod…" }.into();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || tofu_arm(on))
                            .await
                            .unwrap_or_else(|e| Err(format!("tofu-arm task panicked: {e}")))
                    },
                    |r| crate::Message::Datacenter(Message::TofuArmDone(r)),
                )
            }
            Message::TofuArmDone(Ok(armed)) => {
                self.tofu_armed = armed;
                self.status = if armed { "Prod armed" } else { "Prod disarmed" }.into();
                Task::none()
            }
            Message::TofuArmDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::TofuRunlogClicked(ws) => {
                self.status = format!("Reading {ws} run-log…");
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || tofu_runlog(&ws))
                            .await
                            .unwrap_or_else(|e| Err(format!("runlog task panicked: {e}")))
                    },
                    |r| crate::Message::Datacenter(Message::TofuRunlogDone(r)),
                )
            }
            Message::TofuRunlogDone(Ok(log)) => {
                self.tofu_runlog = log;
                self.status = "Run-log loaded".into();
                Task::none()
            }
            Message::TofuRunlogDone(Err(e)) => {
                self.status = e;
                Task::none()
            }

            // ── DATACENTER-20 — promotion controls ────────────────────────────
            Message::AutoPromoteToggled => {
                let on = !self.auto_promote;
                self.auto_promote = on;
                self.run_action(
                    "promote-arm",
                    serde_json::json!({"auto": on}),
                    Duration::from_secs(30),
                    Message::PromoteCtlDone,
                )
            }
            Message::PromoteArmToggled => {
                let on = !self.promote_armed;
                self.promote_armed = on;
                self.run_action(
                    "promote-arm",
                    serde_json::json!({"prod": on}),
                    Duration::from_secs(30),
                    Message::PromoteCtlDone,
                )
            }
            Message::PromoteNowClicked => {
                self.status = "Promoting…".into();
                self.run_action(
                    "promote-now",
                    serde_json::json!({}),
                    Duration::from_secs(300),
                    Message::PromoteCtlDone,
                )
            }
            Message::PromoteCtlDone(r) => {
                self.status = flatten(r);
                Task::none()
            }

            // ── DATACENTER-18/19/21 — provisioning ────────────────────────────
            Message::RegionsRefresh => {
                self.provision_status = "Loading regions…".into();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(do_regions_read)
                            .await
                            .unwrap_or_else(|e| Err(format!("regions task panicked: {e}")))
                    },
                    |r| crate::Message::Datacenter(Message::RegionsDone(r)),
                )
            }
            Message::RegionsDone(Ok(rs)) => {
                self.do_regions = rs;
                self.provision_status = format!("{} region(s) loaded", self.do_regions.len());
                Task::none()
            }
            Message::RegionsDone(Err(e)) => {
                self.provision_status = e;
                Task::none()
            }
            Message::RegionPicked(s) => {
                self.region_slug = Some(s);
                Task::none()
            }
            Message::GuidedLighthouseClicked => {
                let Some(region) = self.region_slug.clone() else {
                    self.provision_status = "Pick a region first.".into();
                    return Task::none();
                };
                self.provision_status = format!("Provisioning a lighthouse in {region}…");
                self.run_action(
                    "do-guided-lighthouse",
                    serde_json::json!({"region": region}),
                    Duration::from_secs(600),
                    Message::ProvisionDone,
                )
            }
            Message::GenesisNameChanged(v) => {
                self.genesis_name = v;
                Task::none()
            }
            Message::GenesisRegionPicked(s) => {
                self.genesis_region = Some(s);
                Task::none()
            }
            Message::GenesisSubmit => {
                let name = self.genesis_name.trim().to_string();
                let Some(region) = self.genesis_region.clone() else {
                    self.provision_status = "Pick a region for the new mesh.".into();
                    return Task::none();
                };
                if name.is_empty() {
                    self.provision_status = "Name the new mesh first.".into();
                    return Task::none();
                }
                self.provision_status = format!("Giving birth to {name} in {region}…");
                self.run_action(
                    "genesis-new-mesh",
                    serde_json::json!({"name": name, "region": region}),
                    Duration::from_secs(600),
                    Message::ProvisionDone,
                )
            }
            Message::TestmeshNChanged(v) => {
                self.testmesh_n = v;
                Task::none()
            }
            Message::TestmeshSpinClicked => {
                let n = self.testmesh_n.trim().parse::<u32>().unwrap_or(3);
                self.provision_status = format!("Spinning a {n}-node test mesh…");
                self.run_action(
                    "testmesh-spin",
                    serde_json::json!({"n": n}),
                    Duration::from_secs(600),
                    Message::ProvisionDone,
                )
            }
            Message::TestmeshTeardownClicked(id) => {
                self.provision_status = format!("Tearing down {id}…");
                self.run_action(
                    "testmesh-teardown",
                    serde_json::json!({"id": id}),
                    Duration::from_secs(300),
                    Message::ProvisionDone,
                )
            }
            Message::FarmScaleChanged(v) => {
                self.farm_scale_n = v;
                Task::none()
            }
            Message::FarmScaleClicked => {
                let count = self.farm_scale_n.trim().parse::<u32>().unwrap_or(2);
                self.provision_status = format!("Scaling the build farm to {count}…");
                self.run_action(
                    "farm-scale",
                    serde_json::json!({"count": count}),
                    Duration::from_secs(600),
                    Message::ProvisionDone,
                )
            }
            Message::ProvisionDone(r) => {
                self.provision_status = flatten(r);
                Task::none()
            }
        }
    }

    /// Fire a datacenter action verb on a blocking thread and route the ok/err
    /// status string back through `done`. Shared by the many "click → RPC →
    /// status" handlers so each arm stays a one-liner (§6 — no per-verb copy of the
    /// `spawn_blocking` + [`dc_action`] boilerplate). `verb` is a `'static` literal
    /// so it captures cleanly into the async block.
    fn run_action(
        &self,
        verb: &'static str,
        body: serde_json::Value,
        timeout: Duration,
        done: fn(Result<String, String>) -> Message,
    ) -> Task<crate::Message> {
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || dc_action(verb, &body, timeout))
                    .await
                    .unwrap_or_else(|e| Err(format!("{verb} task panicked: {e}")))
            },
            move |r| crate::Message::Datacenter(done(r)),
        )
    }

    /// MOTION-FEEDBACK-2 — stamp the card-grid reveal origin and arm the motion
    /// tick so the cards stagger in. Called when a fresh row set lands.
    fn begin_reveal(&mut self) -> Task<crate::Message> {
        self.reveal_start = Some(Instant::now());
        self.arm_motion()
    }

    /// MOTION-FEEDBACK-2 — start the self-re-arming [`Message::MotionTick`] chain
    /// if one isn't already running. Idempotent: concurrent state changes (select +
    /// hover + reveal) share the single in-flight tick chain rather than each
    /// spawning its own, and at rest no chain runs (zero idle wakeups).
    fn arm_motion(&mut self) -> Task<crate::Message> {
        if self.motion_ticking {
            return Task::none();
        }
        self.motion_ticking = true;
        tick_motion_later()
    }

    /// MOTION-FEEDBACK-2 — advance one motion frame: garbage-collect the settled
    /// selection tween, retire an elapsed reveal, then either re-arm the tick (a
    /// reveal/selection/hover is still in flight) or stop the chain (everything
    /// settled). Pure state; the view reads the live tween values each frame.
    fn tick_motion(&mut self) -> Task<crate::Message> {
        let now = Instant::now();
        let reduce_motion = crate::live_theme::reduce_motion();
        self.selection.gc(now);
        // Retire a reveal once its last *visible* card has finished sliding in (the
        // duration is reduce-motion-aware, matching `slide_in`), so a settled grid
        // renders statically with no per-frame easing — and a small grid doesn't
        // keep ticking for the absent cap slots.
        if self
            .reveal_start
            .is_some_and(|start| reveal_is_complete(start, now, self.visible_card_count(), reduce_motion))
        {
            self.reveal_start = None;
        }
        if self.motion_in_flight(now, reduce_motion) {
            tick_motion_later()
        } else {
            self.motion_ticking = false;
            Task::none()
        }
    }

    /// MOTION-FEEDBACK-2 — is any card-grid motion (reveal, selection accent, or
    /// hover-lift) still animating at `now`? Drives the tick-stop guard. Under
    /// reduce-motion the hover-lift is dropped (no movement), so it's never counted
    /// as in flight.
    fn motion_in_flight(&self, now: Instant, reduce_motion: bool) -> bool {
        let reveal = self.reveal_start.is_some_and(|start| {
            !reveal_is_complete(start, now, self.visible_card_count(), reduce_motion)
        });
        // The hover-lift (enter rise / leave settle) runs over `Motion::hover()`
        // from `hover_since`; it's in flight until that tween elapses. Skipped under
        // reduce-motion, where there is no movement to settle.
        let hover = !reduce_motion
            && !mde_theme::animation::Tween::resolved(
                self.hover_since,
                Motion::hover().duration,
                false,
            )
            .is_complete(now);
        reveal || hover || !self.selection.is_idle(now)
    }

    /// MOTION-FEEDBACK-2 — the number of resource cards currently rendered in the
    /// card grid (the active zone tab AND the global search needle). The reveal
    /// completion check keys off the *last visible* card, not the stagger cap, so a
    /// small grid stops ticking the moment its real cards have settled.
    fn visible_card_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| r.zone == self.zone_tab && r.matches_filter(&self.filter))
            .count()
    }

    /// Seed [`Self::expanded`] so every current Topology group starts expanded —
    /// run once per row set (guarded by [`Self::topology_seeded`]) the first time
    /// the Topology view renders. A manual collapse afterwards sticks because the
    /// guard stays set until the next `Loaded`.
    fn ensure_topology_seeded(&mut self) {
        if self.topology_seeded {
            return;
        }
        for (header, _) in group_by_host(&self.rows) {
            self.expanded.insert(header.id.clone());
        }
        self.topology_seeded = true;
    }

    /// DATACENTER-8 (saved views) — persist the current saved-views collection to
    /// disk, surfacing a write failure in the status line (the in-memory edit is
    /// kept; the next save retries). Refuses to write when the views were never
    /// successfully loaded (`!views_loaded`) — that state means a real read error
    /// on an existing file, so writing the (empty / partial) in-memory set would
    /// overwrite the still-on-disk views as data loss (the code-review path).
    fn persist_saved_views(&mut self) {
        if !self.views_loaded {
            self.status =
                "Saved view kept in this session — the saved-views file couldn't be read, \
                 so it wasn't written (to avoid overwriting it)."
                    .into();
            return;
        }
        if let Err(e) = save_saved_views(&self.saved_views) {
            self.status = format!("Saved view kept, but couldn't write the file: {e}");
        }
    }

    /// DATACENTER-8 (saved views) — the "Saved views" bar: a "Save view as…" name
    /// box + a Save button, then one restore chip per saved view (click to apply)
    /// each paired with a "✕" delete affordance. Renders entirely through the
    /// shared Carbon controls (`variant_button` / `text_input`) so it matches the
    /// rest of the panel (§4). When there are no saved views the chip row shows a
    /// muted hint instead.
    fn saved_views_bar(&self, palette: Palette) -> Element<'_, crate::Message> {
        // Name box + Save. Save is enabled only when the box has a non-blank name
        // (a blank save is a no-op anyway, but disabling it reads honestly).
        let name_box = text_input("Save view as…", &self.save_view_name)
            .on_input(|v| crate::Message::Datacenter(Message::SaveViewNameChanged(v)))
            .on_submit(crate::Message::Datacenter(Message::SaveCurrentView))
            .width(Length::FillPortion(2));
        let save_enabled = !self.save_view_name.trim().is_empty();
        let save_btn = variant_button(
            "Save".to_string(),
            ButtonVariant::Secondary,
            save_enabled.then_some(crate::Message::Datacenter(Message::SaveCurrentView)),
            palette,
        );

        let mut bar = row![
            text("Saved views").colr(palette.text_muted.into_cosmic_color()),
            name_box,
            save_btn,
        ]
        .spacing(f32::from(spacing::BASE[2]))
        .align_y(cosmic::iced::alignment::Vertical::Center);

        if self.saved_views.is_empty() {
            bar = bar.push(
                text("— none yet")
                    .colr(palette.text_muted.into_cosmic_color())
                    .size(f32::from(spacing::BASE[4])),
            );
        } else {
            for v in &self.saved_views.views {
                // The restore chip: clicking it applies the saved view. The
                // currently-applied view (same mode + zone + filter) reads as
                // Primary so the operator sees which one is active.
                let is_active = self.view_mode == v.mode()
                    && self.zone_tab == v.zone_tab
                    && self.filter == v.filter;
                let variant = if is_active {
                    ButtonVariant::Primary
                } else {
                    ButtonVariant::Secondary
                };
                let apply = variant_button(
                    v.name.clone(),
                    variant,
                    Some(crate::Message::Datacenter(Message::ApplyView(
                        v.name.clone(),
                    ))),
                    palette,
                );
                // A small Ghost "✕" to delete the saved view.
                let del = variant_button(
                    "✕".to_string(),
                    ButtonVariant::Ghost,
                    Some(crate::Message::Datacenter(Message::DeleteView(
                        v.name.clone(),
                    ))),
                    palette,
                );
                bar = bar.push(
                    row![apply, del]
                        .spacing(f32::from(spacing::BASE[1]))
                        .align_y(cosmic::iced::alignment::Vertical::Center),
                );
            }
        }

        scrollable(bar)
            .direction(scrollable::Direction::Horizontal(
                scrollable::Scrollbar::new(),
            ))
            .width(Length::Fill)
            .into()
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
            mode_btn("Topology", ViewMode::Topology),
            mode_btn("Resources", ViewMode::Zone),
            mode_btn("Tofu", ViewMode::Tofu),
            mode_btn("Gateway", ViewMode::Gateway),
            mode_btn("Provision", ViewMode::Provision),
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

        // Global search — a free-text needle matched case-insensitively against
        // each rendered resource's name / id / kind (see `DcRow::matches_filter`).
        // Lives above the view body so it narrows whichever view (Resources /
        // Topology) is showing. Empty box = no filtering.
        let search = row![
            text("Search").colr(palette.text_muted.into_cosmic_color()),
            text_input("name / id / kind", &self.filter)
                .on_input(|v| crate::Message::Datacenter(Message::FilterChanged(v)))
                .width(Length::FillPortion(3)),
        ]
        .spacing(f32::from(spacing::BASE[2]))
        .align_y(cosmic::iced::alignment::Vertical::Center);
        col = col.push(search);

        // DATACENTER-8 (saved views) — a bar to name + save the current view
        // (mode + zone tab + search needle) and restore/delete saved ones. Sits
        // under the search box so the operator saves exactly what they've filtered
        // down to. Carbon tokens only (§4).
        col = col.push(self.saved_views_bar(palette));

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
                // DATACENTER-9 — rolling-history sparklines: a block-glyph trend
                // line per tracked series (resources / running / health-ok /
                // alerts) off the capped Bus-load history. Sits beside the
                // last-value rollup/health readouts so the operator reads "now"
                // and "trending" together.
                col = col.push(text("Trend").size(f32::from(spacing::BASE[5])));
                for el in sparklines_view(self.history(), palette) {
                    col = col.push(el);
                }
                // Build → Eagle → DO promotion strip — a version matrix fed by
                // `event/dc/promote/*`. Always renders all three stages (absent
                // ones show "—") so the pipeline reads left-to-right.
                col = col.push(text("Promotion").size(f32::from(spacing::BASE[5])));
                col = col.push(promote_strip_view(&self.promote, palette));
                // DATACENTER-20 — promotion controls: auto-promote-on-green +
                // the DO-step prod-arm gate + a manual "Promote now".
                let auto_label = if self.auto_promote {
                    "Auto-promote: ON"
                } else {
                    "Auto-promote: OFF"
                };
                let arm_label = if self.promote_armed {
                    "Prod-arm: ARMED"
                } else {
                    "Prod-arm: disarmed"
                };
                col = col.push(
                    row![
                        variant_button(
                            auto_label.to_string(),
                            if self.auto_promote {
                                ButtonVariant::Primary
                            } else {
                                ButtonVariant::Secondary
                            },
                            Some(crate::Message::Datacenter(Message::AutoPromoteToggled)),
                            palette,
                        ),
                        variant_button(
                            arm_label.to_string(),
                            if self.promote_armed {
                                ButtonVariant::Primary
                            } else {
                                ButtonVariant::Secondary
                            },
                            Some(crate::Message::Datacenter(Message::PromoteArmToggled)),
                            palette,
                        ),
                        // "Promote now" only fires when the DO step is armed.
                        variant_button(
                            "Promote now".to_string(),
                            ButtonVariant::Secondary,
                            self.promote_armed
                                .then_some(crate::Message::Datacenter(Message::PromoteNowClicked)),
                            palette,
                        ),
                    ]
                    .spacing(f32::from(spacing::BASE[2])),
                );
                // DATACENTER-9 — the per-target version matrix: farm / Eagle /
                // each lighthouse. Where the strip above shows only the three
                // pipeline stages, this adds a row per live lighthouse (a Prod
                // droplet) so per-host version drift reads at a glance. Projected
                // off the same `event/dc/promote/*` + the droplet rows.
                col = col.push(text("Version matrix").size(f32::from(spacing::BASE[5])));
                let vmatrix = VersionMatrix::project(&self.promote, &self.rows);
                for el in version_matrix_view(&vmatrix, palette) {
                    col = col.push(el);
                }
                // Health summary — a one-line ok/warn/fail tally fed by
                // `event/dc/health/*`, plus an alert list of any non-ok checks.
                col = col.push(text("Health").size(f32::from(spacing::BASE[5])));
                for el in health_section_view(&self.health, palette) {
                    col = col.push(el);
                }
                // Recent Tofu runs — a newest-first run-log fed by
                // `event/dc/job/*`, filtered to the tofu verbs (plan/apply/
                // destroy/state) and capped. Each row = the verb + a status chip
                // (ok = success / error = danger / pending = warning).
                col = col.push(text("Recent Tofu runs").size(f32::from(spacing::BASE[5])));
                for el in recent_tofu_runs_view(&self.jobs, palette) {
                    col = col.push(el);
                }
                // DR / Backup control — "Back up now" arms a typed-confirm before
                // firing the `action/dc/dr-backup` RPC, which snapshots the Tofu
                // state + secrets. dr_status renders under the button.
                col = col.push(text("DR / Backup").size(f32::from(spacing::BASE[5])));
                if self.dr_confirm {
                    // Armed: surface the typed-confirm — only the confirm button
                    // carries the `DrBackup` message that fires the RPC.
                    col = col.push(
                        row![
                            text("Backup state + secrets?").colr(palette.text.into_cosmic_color()),
                            variant_button(
                                "Confirm".to_string(),
                                ButtonVariant::Primary,
                                Some(crate::Message::Datacenter(Message::DrBackup)),
                                palette,
                            ),
                            variant_button(
                                "Cancel".to_string(),
                                ButtonVariant::Secondary,
                                Some(crate::Message::Datacenter(Message::DrBackupCancelled)),
                                palette,
                            ),
                        ]
                        .spacing(f32::from(spacing::BASE[2])),
                    );
                } else {
                    // Unarmed: the first click only arms the confirm (no RPC).
                    col = col.push(variant_button(
                        "Back up now".to_string(),
                        ButtonVariant::Primary,
                        Some(crate::Message::Datacenter(Message::DrBackupClicked)),
                        palette,
                    ));
                }
                if !self.dr_status.is_empty() {
                    col = col.push(
                        text(self.dr_status.clone()).colr(palette.text_muted.into_cosmic_color()),
                    );
                }
            }
            ViewMode::Tofu => {
                // DATACENTER-15 — the prod-arm master switch. A Prod (zone1-do)
                // apply is refused while disarmed (the guard in `TofuApply`); the
                // toggle reads ARMED / disarmed and is the only path to enable it.
                let arm_label = if self.tofu_armed {
                    "Prod: ARMED"
                } else {
                    "Prod: disarmed"
                };
                col = col.push(
                    row![
                        text("Prod-arm").colr(palette.text_muted.into_cosmic_color()),
                        variant_button(
                            arm_label.to_string(),
                            if self.tofu_armed {
                                ButtonVariant::Primary
                            } else {
                                ButtonVariant::Secondary
                            },
                            Some(crate::Message::Datacenter(Message::TofuArmToggled)),
                            palette,
                        ),
                    ]
                    .spacing(f32::from(spacing::BASE[2]))
                    .align_y(cosmic::iced::alignment::Vertical::Center),
                );
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
                    let state_btn = variant_button(
                        format!("State {ws}"),
                        ButtonVariant::Secondary,
                        Some(crate::Message::Datacenter(Message::TofuStateClicked(
                            ws.to_string(),
                        ))),
                        palette,
                    );
                    // DATACENTER-15 — the persisted run-log button per workspace.
                    let runlog_btn = variant_button(
                        format!("Run-log {ws}"),
                        ButtonVariant::Secondary,
                        Some(crate::Message::Datacenter(Message::TofuRunlogClicked(
                            ws.to_string(),
                        ))),
                        palette,
                    );
                    let mut ws_row = row![
                        text(ws.to_string()).width(Length::FillPortion(2)),
                        plan_btn,
                        state_btn,
                        runlog_btn
                    ]
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
                        // The Prod (zone1-do) Apply stays disabled until prod-arm is
                        // ARMED (DATACENTER-15 prod guardrail).
                        let prod_blocked = ws == "zone1-do" && !self.tofu_armed;
                        let apply_label = if prod_blocked {
                            "Apply (arm prod)".to_string()
                        } else {
                            format!("Apply {ws}")
                        };
                        ws_row = ws_row.push(variant_button(
                            apply_label,
                            ButtonVariant::Primary,
                            (!prod_blocked).then_some(crate::Message::Datacenter(
                                Message::TofuApplyClicked(ws.to_string()),
                            )),
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
                // DATACENTER-15 — the persisted run-log, shown after a Run-log read.
                if !self.tofu_runlog.is_empty() {
                    col = col.push(text("Run log").size(f32::from(spacing::BASE[5])));
                    col = col.push(
                        container(text(self.tofu_runlog.clone()))
                            .padding(f32::from(spacing::BASE[3]))
                            .width(Length::Fill),
                    );
                }
                // The managed-state browser: once a State read has returned for a
                // workspace, list its managed resources + a drift badge.
                if !self.tofu_state_ws.is_empty() {
                    let header = format!(
                        "Managed resources ({}) · {}",
                        self.tofu_state_resources.len(),
                        self.tofu_state_ws
                    );
                    col = col.push(text(header).size(f32::from(spacing::BASE[5])));
                    // Drift badge — color from mde-theme tokens, never raw hex.
                    if self.tofu_state_drift {
                        col = col.push(
                            text("⚠ DRIFT — live differs from state")
                                .colr(palette.danger.into_cosmic_color()),
                        );
                    } else {
                        col = col.push(text("✓ in sync").colr(palette.success.into_cosmic_color()));
                    }
                    if self.tofu_state_resources.is_empty() {
                        col = col.push(
                            text("No managed resources recorded for this workspace.")
                                .colr(palette.text_muted.into_cosmic_color()),
                        );
                    } else {
                        for res in &self.tofu_state_resources {
                            col = col.push(
                                container(text(res.clone()).colr(palette.text.into_cosmic_color()))
                                    .padding(f32::from(spacing::BASE[2]))
                                    .width(Length::Fill),
                            );
                        }
                    }
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
            ViewMode::Topology => {
                col = col.push(text("Topology").size(f32::from(spacing::BASE[5])));
                // Honor the global search: group only the rows that match the
                // needle (a host header is itself a row, so a search by host
                // name/id keeps its group; otherwise the children carry it).
                let filtered: Vec<DcRow> = self
                    .rows
                    .iter()
                    .filter(|r| r.matches_filter(&self.filter))
                    .cloned()
                    .collect();
                let groups = group_by_host(&filtered);
                if groups.is_empty() {
                    col = col.push(
                        text(
                            "No datacenter topology yet. Hosts, their VMs / storage \
                             / networks, and the Prod zone appear here as the \
                             orchestrator publishes them.",
                        )
                        .colr(palette.text_muted.into_cosmic_color()),
                    );
                } else {
                    for (header, children) in &groups {
                        // The synthetic Prod/Gateway group is keyed on the empty
                        // host id; real host groups key on the host's id.
                        let key = header.id.clone();
                        let is_open = self.expanded.contains(&key);
                        col = col.push(topology_header_view(
                            header,
                            children.len(),
                            is_open,
                            palette,
                        ));
                        if is_open {
                            let n = children.len();
                            for (i, child) in children.iter().enumerate() {
                                let last = i + 1 == n;
                                col = col.push(topology_child_view(child, last, palette));
                            }
                        }
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
                    // DATACENTER-8/1813 — per-kind sub-tabs (Hosts / VMs / Storage /
                    // Network) within the active zone.
                    col = col.push(self.kind_subtabs_view(palette));

                    // The rendered set = active zone AND active kind sub-tab AND the
                    // global search needle (name / id / kind).
                    let visible: Vec<&DcRow> = self
                        .rows
                        .iter()
                        .filter(|r| {
                            r.zone == self.zone_tab
                                && self.kind_tab.matches(&r.kind)
                                && r.matches_filter(&self.filter)
                        })
                        .collect();
                    for el in self.kind_body_view(palette, &visible) {
                        col = col.push(el);
                    }
                }
            }
            ViewMode::Gateway => {
                col = col.push(text("Gateway (UniFi)").size(f32::from(spacing::BASE[5])));
                for el in self.gateway_view(palette) {
                    col = col.push(el);
                }
            }
            ViewMode::Provision => {
                col = col.push(text("Provisioning").size(f32::from(spacing::BASE[5])));
                for el in self.provision_view(palette) {
                    col = col.push(el);
                }
            }
        }

        scrollable(col).into()
    }

    // ── DATACENTER-8/1813 — per-kind sub-tabs + per-tab bodies ────────────────

    /// The Hosts / VMs / Storage / Network sub-tab selector for the active zone —
    /// each chip carries that kind's count in the active zone. The selected tab is
    /// the Primary (filled) variant.
    fn kind_subtabs_view(&self, palette: Palette) -> Element<'_, crate::Message> {
        let mut r = row![].spacing(f32::from(spacing::BASE[2]));
        for t in KindTab::all() {
            let count = self
                .rows
                .iter()
                .filter(|row| row.zone == self.zone_tab && t.matches(&row.kind))
                .count();
            let variant = if self.kind_tab == t {
                ButtonVariant::Primary
            } else {
                ButtonVariant::Secondary
            };
            r = r.push(variant_button(
                format!("{} ({count})", t.label()),
                variant,
                Some(crate::Message::Datacenter(Message::KindTab(t))),
                palette,
            ));
        }
        r.into()
    }

    /// Dispatch the active sub-tab to its purpose-built body. `visible` is the
    /// zone + kind + search filtered row set.
    fn kind_body_view(
        &self,
        palette: Palette,
        visible: &[&DcRow],
    ) -> Vec<Element<'_, crate::Message>> {
        match self.kind_tab {
            KindTab::Vms => self.vms_body(palette, visible),
            KindTab::Hosts => self.hosts_body(palette, visible),
            KindTab::Storage => self.storage_body(palette, visible),
            KindTab::Network => self.network_body(palette, visible),
        }
    }

    /// The empty-state hint shared by every sub-tab — distinguishes a genuinely
    /// empty tab from a search that hid everything.
    fn empty_zone_hint(&self, palette: Palette) -> Element<'_, crate::Message> {
        if self.filter.trim().is_empty() {
            text("Nothing in this sub-tab yet.")
                .colr(palette.text_muted.into_cosmic_color())
                .into()
        } else {
            text(format!(
                "No resources match \u{201c}{}\u{201d} here.",
                self.filter.trim()
            ))
            .colr(palette.text_muted.into_cosmic_color())
            .into()
        }
    }

    /// DATACENTER-11 — the VMs sub-tab: a New-VM (golden-template → Tofu) wizard
    /// toggle + a bulk-select toggle, the optional create wizard / bulk bar /
    /// migrate picker, then the motion card grid (each VM card carries power,
    /// suspend, migrate, console, snapshot, clone, delete, and a bulk Select).
    fn vms_body(&self, palette: Palette, visible: &[&DcRow]) -> Vec<Element<'_, crate::Message>> {
        let mut out: Vec<Element<'_, crate::Message>> = Vec::new();
        let bulk_label = if self.bulk_mode {
            "Exit select"
        } else {
            "Select…"
        };
        out.push(
            row![
                pbtn("New VM…", Message::CreateToggled, palette),
                sbtn(bulk_label, Message::BulkModeToggled, palette),
            ]
            .spacing(f32::from(spacing::BASE[2]))
            .into(),
        );
        if self.create_open {
            out.push(self.create_wizard_view(palette));
        }
        if self.bulk_mode {
            out.push(self.bulk_bar_view(palette));
        }
        if let Some((uuid, _dom0)) = &self.migrate {
            out.push(self.migrate_picker_view(palette, uuid));
        }
        if visible.is_empty() {
            out.push(self.empty_zone_hint(palette));
            return out;
        }
        let now = Instant::now();
        let reduce_motion = crate::live_theme::reduce_motion();
        let cards: Vec<Element<'_, crate::Message>> = visible
            .iter()
            .copied()
            .enumerate()
            .map(|(i, r)| {
                let confirming = self.confirm_delete.as_deref() == Some(r.id.as_str());
                let bulk = self.bulk_mode.then(|| self.bulk_sel.contains(r.id.as_str()));
                let motion = CardMotion {
                    index: i,
                    reveal_start: self.reveal_start,
                    selected: self.selected_card.as_deref() == Some(r.id.as_str()),
                    selection: &self.selection,
                    hovered: self.hovered_card.as_deref() == Some(r.id.as_str()),
                    hover_since: self.hover_since,
                    now,
                    reduce_motion,
                };
                dc_card_view(r, palette, confirming, bulk, motion)
            })
            .collect();
        out.push(card_grid(cards));
        out
    }

    /// DATACENTER-11 — the golden-template create wizard: template + name inputs and
    /// a Create button that fires `action/dc/vm-create` (the worker writes a Tofu
    /// resource + applies, so structural changes don't drift).
    fn create_wizard_view(&self, palette: Palette) -> Element<'_, crate::Message> {
        let template = text_input(
            "golden template (e.g. MDE-VM-golden)",
            &self.create_template,
        )
        .on_input(|v| crate::Message::Datacenter(Message::CreateTemplateChanged(v)))
        .width(Length::FillPortion(2));
        let name = text_input("new VM name", &self.create_name)
            .on_input(|v| crate::Message::Datacenter(Message::CreateNameChanged(v)))
            .width(Length::FillPortion(2));
        let enabled =
            !self.create_template.trim().is_empty() && !self.create_name.trim().is_empty();
        let submit = variant_button(
            "Create (Tofu)".to_string(),
            ButtonVariant::Primary,
            enabled.then_some(crate::Message::Datacenter(Message::CreateSubmit)),
            palette,
        );
        let cancel = variant_button(
            "Cancel".to_string(),
            ButtonVariant::Ghost,
            Some(crate::Message::Datacenter(Message::CreateToggled)),
            palette,
        );
        let body = column![
            text(format!(
                "New VM in {} — clones a golden template via Tofu.",
                self.zone_tab
            ))
            .colr(palette.text_muted.into_cosmic_color()),
            row![template, name].spacing(f32::from(spacing::BASE[2])),
            row![submit, cancel].spacing(f32::from(spacing::BASE[2])),
        ]
        .spacing(f32::from(spacing::BASE[2]));
        surface_card(body, palette)
    }

    /// DATACENTER-11 — the bulk action bar (visible in select mode): the selected
    /// count, the four bulk ops (start / stop / snapshot / tag) firing
    /// `action/dc/vm-bulk`, and per-item progress as results land.
    fn bulk_bar_view(&self, palette: Palette) -> Element<'_, crate::Message> {
        let n = self.bulk_sel.len();
        let op_btn = |label: &str, op: &str| {
            let msg = (n > 0).then_some(crate::Message::Datacenter(Message::BulkOp(op.to_string())));
            variant_button(label.to_string(), ButtonVariant::Secondary, msg, palette)
        };
        let mut body = column![row![
            text(format!("{n} selected")).colr(palette.text.into_cosmic_color()),
            op_btn("Start", "start"),
            op_btn("Stop", "shutdown"),
            op_btn("Snapshot", "snapshot"),
            op_btn("Tag", "tag"),
        ]
        .spacing(f32::from(spacing::BASE[2]))
        .align_y(cosmic::iced::alignment::Vertical::Center)]
        .spacing(f32::from(spacing::BASE[2]));
        for (uuid, status) in &self.bulk_progress {
            let color = match status.as_str() {
                "ok" => palette.success,
                "error" => palette.danger,
                _ => palette.warning,
            };
            body = body.push(
                row![
                    text(uuid.clone())
                        .colr(palette.text_muted.into_cosmic_color())
                        .width(Length::FillPortion(3)),
                    text(status.clone())
                        .colr(color.into_cosmic_color())
                        .width(Length::FillPortion(1)),
                ]
                .spacing(f32::from(spacing::BASE[2])),
            );
        }
        surface_card(body, palette)
    }

    /// DATACENTER-11 — the migrate target picker (armed for one VM): a dropdown of
    /// candidate dom0 hosts + Migrate / Cancel. Migrate fires `action/dc/vm-migrate`.
    fn migrate_picker_view(&self, palette: Palette, uuid: &str) -> Element<'_, crate::Message> {
        let targets: Vec<String> = self
            .rows
            .iter()
            .filter(|r| r.kind == "host")
            .map(|r| r.id.clone())
            .collect();
        let picker: Element<'_, crate::Message> = if targets.is_empty() {
            text("no target hosts known")
                .colr(palette.text_muted.into_cosmic_color())
                .into()
        } else {
            pick_list(targets, self.migrate_target.clone(), |v| {
                crate::Message::Datacenter(Message::MigrateTargetPicked(v))
            })
            .into()
        };
        let confirm = variant_button(
            "Migrate".to_string(),
            ButtonVariant::Primary,
            self.migrate_target
                .is_some()
                .then_some(crate::Message::Datacenter(Message::MigrateConfirmed)),
            palette,
        );
        let cancel = sbtn("Cancel", Message::MigrateCancelled, palette);
        let body = column![
            text(format!("Migrate VM {uuid} to:")).colr(palette.text.into_cosmic_color()),
            row![picker, confirm, cancel]
                .spacing(f32::from(spacing::BASE[2]))
                .align_y(cosmic::iced::alignment::Vertical::Center),
        ]
        .spacing(f32::from(spacing::BASE[2]));
        surface_card(body, palette)
    }

    /// DATACENTER-10/16 — the Hosts sub-tab: the idle-shutdown policy toggle, any
    /// live wake/power progress bars, then one action card per host.
    fn hosts_body(&self, palette: Palette, visible: &[&DcRow]) -> Vec<Element<'_, crate::Message>> {
        let mut out: Vec<Element<'_, crate::Message>> = Vec::new();
        let idle_label = if self.idle_policy_on {
            "Idle-shutdown: ON"
        } else {
            "Idle-shutdown: OFF"
        };
        out.push(
            row![
                text("Energy policy").colr(palette.text_muted.into_cosmic_color()),
                variant_button(
                    idle_label.to_string(),
                    if self.idle_policy_on {
                        ButtonVariant::Primary
                    } else {
                        ButtonVariant::Secondary
                    },
                    Some(crate::Message::Datacenter(Message::IdlePolicyToggled)),
                    palette,
                ),
            ]
            .spacing(f32::from(spacing::BASE[2]))
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .into(),
        );
        for ps in &self.power {
            out.push(power_progress_view(ps, palette));
        }
        if visible.is_empty() {
            out.push(self.empty_zone_hint(palette));
            return out;
        }
        for r in visible.iter().copied() {
            out.push(self.host_row_view(palette, r));
        }
        out
    }

    /// DATACENTER-10/16 — one host action card: header (status dot + metrics), the
    /// lifecycle row (maintenance/reboot/evacuate/patch/SSH), the pool + IPMI row,
    /// and an inline evacuate confirm / patch-impact preview when armed.
    fn host_row_view(&self, palette: Palette, r: &DcRow) -> Element<'_, crate::Message> {
        let host = r.id.clone();
        let name = if r.name.is_empty() {
            r.id.clone()
        } else {
            r.name.clone()
        };
        let mut meta = String::new();
        if !r.cpu.is_empty() {
            meta.push_str(&format!(" · {} vCPU", r.cpu));
        }
        if !r.mem_free_mb.is_empty() && !r.mem_total_mb.is_empty() {
            meta.push_str(&format!(" · {}/{} MB free", r.mem_free_mb, r.mem_total_mb));
        }
        if !r.load.is_empty() {
            meta.push_str(&format!(" · load {}", r.load));
        }
        let header = row![
            status_dot_view(r, palette),
            text(format!("{name} ({host}){meta}")).colr(palette.text.into_cosmic_color()),
        ]
        .spacing(f32::from(spacing::BASE[2]))
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let actions = row![
            sbtn(
                "Maint-on",
                Message::HostPowerClicked {
                    host: host.clone(),
                    op: "maintenance-on".into()
                },
                palette
            ),
            sbtn(
                "Maint-off",
                Message::HostPowerClicked {
                    host: host.clone(),
                    op: "maintenance-off".into()
                },
                palette
            ),
            sbtn(
                "Reboot",
                Message::HostPowerClicked {
                    host: host.clone(),
                    op: "reboot".into()
                },
                palette
            ),
            sbtn("Evacuate", Message::HostEvacuateClicked(host.clone()), palette),
            sbtn("Patch…", Message::HostPatchClicked(host.clone()), palette),
            sbtn("SSH", Message::HostConsoleClicked(host.clone()), palette),
        ]
        .spacing(f32::from(spacing::BASE[1]));
        let pool = row![
            text("Pool").colr(palette.text_muted.into_cosmic_color()),
            sbtn(
                "Join",
                Message::HostPoolClicked {
                    host: host.clone(),
                    op: "join".into()
                },
                palette
            ),
            sbtn(
                "Eject",
                Message::HostPoolClicked {
                    host: host.clone(),
                    op: "eject".into()
                },
                palette
            ),
            sbtn(
                "Master",
                Message::HostPoolClicked {
                    host: host.clone(),
                    op: "designate-master".into()
                },
                palette
            ),
            text("IPMI").colr(palette.text_muted.into_cosmic_color()),
            sbtn(
                "On",
                Message::PowerIpmiClicked {
                    host: host.clone(),
                    op: "on".into()
                },
                palette
            ),
            sbtn(
                "Off",
                Message::PowerIpmiClicked {
                    host: host.clone(),
                    op: "off".into()
                },
                palette
            ),
            sbtn(
                "Cycle",
                Message::PowerIpmiClicked {
                    host: host.clone(),
                    op: "cycle".into()
                },
                palette
            ),
        ]
        .spacing(f32::from(spacing::BASE[1]))
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let mut body = column![header, actions, pool].spacing(f32::from(spacing::BASE[2]));

        if let Some((chost, op)) = &self.host_confirm {
            if chost == &host {
                if op == "patch" {
                    if let Some((_, vms)) = &self.host_patch_preview {
                        let impact = if vms.is_empty() {
                            "no VMs would move".to_string()
                        } else {
                            format!("{} VM(s) evacuate first: {}", vms.len(), vms.join(", "))
                        };
                        body = body.push(
                            text(format!("Patch impact — {impact}"))
                                .colr(palette.warning.into_cosmic_color()),
                        );
                    }
                }
                let verb_label = if op == "patch" {
                    "Apply patch"
                } else {
                    "Confirm evacuate"
                };
                body = body.push(
                    row![
                        pbtn(verb_label, Message::HostConfirmed, palette),
                        sbtn("Cancel", Message::HostCancelled, palette),
                    ]
                    .spacing(f32::from(spacing::BASE[2])),
                );
            }
        }
        surface_card(body, palette)
    }

    /// DATACENTER-12 — the Storage sub-tab: a Create-SR + retention toolbar, the
    /// ISO/template library (absorbs the `images` panel), then one card per SR/VDI
    /// with attach/detach/schedule/destroy.
    fn storage_body(
        &self,
        palette: Palette,
        visible: &[&DcRow],
    ) -> Vec<Element<'_, crate::Message>> {
        let mut out: Vec<Element<'_, crate::Message>> = Vec::new();
        out.push(
            row![
                pbtn("Create SR", Message::SrCreateClicked, palette),
                text("Snapshot retention (keep N):").colr(palette.text_muted.into_cosmic_color()),
                text_input("7", &self.sr_retention)
                    .on_input(|v| crate::Message::Datacenter(Message::SrRetentionChanged(v)))
                    .width(Length::Fixed(60.0)),
            ]
            .spacing(f32::from(spacing::BASE[2]))
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .into(),
        );
        // ISO / template library (absorbs the images panel content).
        let isos: Vec<&DcRow> = visible.iter().copied().filter(|r| r.kind == "iso").collect();
        out.push(
            text(format!("ISO / template library ({})", isos.len()))
                .colr(palette.text_muted.into_cosmic_color())
                .into(),
        );
        for iso in isos {
            let label = if iso.name.is_empty() {
                iso.id.clone()
            } else {
                iso.name.clone()
            };
            out.push(
                text(format!("• {label}"))
                    .colr(palette.text.into_cosmic_color())
                    .into(),
            );
        }
        let srs: Vec<&DcRow> = visible
            .iter()
            .copied()
            .filter(|r| r.kind == "sr" || r.kind == "vdi")
            .collect();
        if srs.is_empty() {
            out.push(self.empty_zone_hint(palette));
        } else {
            for r in srs {
                out.push(self.sr_row_view(palette, r));
            }
        }
        out
    }

    /// DATACENTER-12 — one SR/VDI action card: capacity header + attach/detach VDI,
    /// scheduled-snapshot, and a confirm-gated destroy.
    fn sr_row_view(&self, palette: Palette, r: &DcRow) -> Element<'_, crate::Message> {
        let sr = r.id.clone();
        let name = if r.name.is_empty() {
            r.id.clone()
        } else {
            r.name.clone()
        };
        let cap = r.capacity_readout().unwrap_or_else(|| r.status.clone());
        let header = row![
            status_dot_view(r, palette),
            text(format!("{name} — {cap}")).colr(palette.text.into_cosmic_color()),
        ]
        .spacing(f32::from(spacing::BASE[2]))
        .align_y(cosmic::iced::alignment::Vertical::Center);
        let mut body = column![
            header,
            row![
                sbtn(
                    "Attach VDI",
                    Message::VdiClicked {
                        sr: sr.clone(),
                        attach: true
                    },
                    palette
                ),
                sbtn(
                    "Detach VDI",
                    Message::VdiClicked {
                        sr: sr.clone(),
                        attach: false
                    },
                    palette
                ),
                sbtn(
                    "Schedule snaps",
                    Message::SrScheduleClicked(sr.clone()),
                    palette
                ),
                pbtn("Destroy", Message::SrDestroyClicked(sr.clone()), palette),
            ]
            .spacing(f32::from(spacing::BASE[1])),
        ]
        .spacing(f32::from(spacing::BASE[2]));
        if self.sr_confirm_destroy.as_deref() == Some(sr.as_str()) {
            body = body.push(
                row![
                    text("Really destroy this SR?").colr(palette.danger.into_cosmic_color()),
                    pbtn("Confirm", Message::SrDestroyConfirmed(sr.clone()), palette),
                    sbtn("Cancel", Message::SrDestroyCancelled, palette),
                ]
                .spacing(f32::from(spacing::BASE[2])),
            );
        }
        surface_card(body, palette)
    }

    /// DATACENTER-13 — the Network sub-tab: a Create-network + VLAN toolbar, one
    /// card per net/PIF/VLAN, and the unified IP/DNS correlation table.
    fn network_body(
        &self,
        palette: Palette,
        visible: &[&DcRow],
    ) -> Vec<Element<'_, crate::Message>> {
        let mut out: Vec<Element<'_, crate::Message>> = Vec::new();
        out.push(
            row![
                pbtn("Create network", Message::NetCreateClicked, palette),
                text("VLAN tag:").colr(palette.text_muted.into_cosmic_color()),
                text_input("0", &self.vlan_input)
                    .on_input(|v| crate::Message::Datacenter(Message::VlanInputChanged(v)))
                    .width(Length::Fixed(60.0)),
            ]
            .spacing(f32::from(spacing::BASE[2]))
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .into(),
        );
        if visible.is_empty() {
            out.push(self.empty_zone_hint(palette));
        } else {
            for r in visible.iter().copied() {
                out.push(self.net_row_view(palette, r));
            }
        }
        out.push(text("Unified IP / DNS").size(f32::from(spacing::BASE[5])).into());
        out.push(
            row![
                sbtn("Refresh IP/DNS", Message::IpDnsRefresh, palette),
                text("UniFi lease \u{2194} DO DNS \u{2194} overlay IP")
                    .colr(palette.text_muted.into_cosmic_color()),
            ]
            .spacing(f32::from(spacing::BASE[2]))
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .into(),
        );
        for el in ipdns_table_view(&self.ipdns, palette) {
            out.push(el);
        }
        out
    }

    /// DATACENTER-13 — one network action card: header (status dot + bridge) +
    /// Set-VLAN / Config-PIF.
    fn net_row_view(&self, palette: Palette, r: &DcRow) -> Element<'_, crate::Message> {
        let net = r.id.clone();
        let name = if r.name.is_empty() {
            r.id.clone()
        } else {
            r.name.clone()
        };
        let bridge = if r.bridge.is_empty() {
            String::new()
        } else {
            format!(" · {}", r.bridge)
        };
        let header = row![
            status_dot_view(r, palette),
            text(format!("{name}{bridge}")).colr(palette.text.into_cosmic_color()),
        ]
        .spacing(f32::from(spacing::BASE[2]))
        .align_y(cosmic::iced::alignment::Vertical::Center);
        let body = column![
            header,
            row![
                sbtn("Set VLAN", Message::VlanSetClicked(net.clone()), palette),
                sbtn("Config PIF", Message::PifConfigClicked(net.clone()), palette),
            ]
            .spacing(f32::from(spacing::BASE[1])),
        ]
        .spacing(f32::from(spacing::BASE[2]));
        surface_card(body, palette)
    }

    /// DATACENTER-14 — the Gateway view: gateway status rows, a firewall-rule add
    /// form, a port-forward add form, and a reboot button.
    fn gateway_view(&self, palette: Palette) -> Vec<Element<'_, crate::Message>> {
        let mut out: Vec<Element<'_, crate::Message>> = Vec::new();
        let gws: Vec<&DcRow> = self.rows.iter().filter(|r| r.kind == "gateway").collect();
        if gws.is_empty() {
            out.push(
                text("No gateway data yet — the orchestrator publishes event/dc/gateway/*.")
                    .colr(palette.text_muted.into_cosmic_color())
                    .into(),
            );
        } else {
            for g in gws {
                let label = if g.name.is_empty() {
                    g.id.clone()
                } else {
                    g.name.clone()
                };
                out.push(
                    row![
                        status_dot_view(g, palette),
                        text(format!("{label} ({})", g.status))
                            .colr(palette.text.into_cosmic_color()),
                    ]
                    .spacing(f32::from(spacing::BASE[2]))
                    .align_y(cosmic::iced::alignment::Vertical::Center)
                    .into(),
                );
            }
        }
        out.push(text("Firewall rule").size(f32::from(spacing::BASE[5])).into());
        out.push(
            row![
                text_input("e.g. allow tcp 443 from any", &self.gw_fw_rule)
                    .on_input(|v| crate::Message::Datacenter(Message::GwFwRuleChanged(v)))
                    .width(Length::FillPortion(3)),
                pbtn("Add rule", Message::GwFirewallClicked, palette),
            ]
            .spacing(f32::from(spacing::BASE[2]))
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .into(),
        );
        out.push(text("Port-forward").size(f32::from(spacing::BASE[5])).into());
        out.push(
            row![
                text_input("e.g. tcp 8443 -> 10.42.0.2:443", &self.gw_pf_fwd)
                    .on_input(|v| crate::Message::Datacenter(Message::GwPfFwdChanged(v)))
                    .width(Length::FillPortion(3)),
                pbtn("Add forward", Message::GwPortForwardClicked, palette),
            ]
            .spacing(f32::from(spacing::BASE[2]))
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .into(),
        );
        out.push(sbtn("Reboot gateway", Message::GwRebootClicked, palette));
        out
    }

    /// DATACENTER-18/19/21 — the Provision view: the DO region picker + guided new-
    /// lighthouse, the New-Mesh genesis wizard, and the ephemeral test-mesh +
    /// build-farm scale controls.
    fn provision_view(&self, palette: Palette) -> Vec<Element<'_, crate::Message>> {
        let mut out: Vec<Element<'_, crate::Message>> = Vec::new();
        if !self.provision_status.is_empty() {
            out.push(
                text(self.provision_status.clone())
                    .colr(palette.text_muted.into_cosmic_color())
                    .into(),
            );
        }
        // ── DO region picker + guided new-lighthouse (DATACENTER-19) ──
        out.push(
            text("DigitalOcean regions")
                .size(f32::from(spacing::BASE[5]))
                .into(),
        );
        let region_slugs: Vec<String> = self
            .do_regions
            .iter()
            .filter(|r| r.available)
            .map(|r| r.slug.clone())
            .collect();
        let do_picker: Element<'_, crate::Message> = if region_slugs.is_empty() {
            text("Load regions to pick one.")
                .colr(palette.text_muted.into_cosmic_color())
                .into()
        } else {
            pick_list(region_slugs.clone(), self.region_slug.clone(), |v| {
                crate::Message::Datacenter(Message::RegionPicked(v))
            })
            .into()
        };
        out.push(
            row![
                sbtn("Load regions", Message::RegionsRefresh, palette),
                do_picker,
                pbtn(
                    "Guided new lighthouse",
                    Message::GuidedLighthouseClicked,
                    palette
                ),
            ]
            .spacing(f32::from(spacing::BASE[2]))
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .into(),
        );
        let lh_count = self
            .rows
            .iter()
            .filter(|r| r.kind == "droplet" && r.zone == "prod")
            .count();
        let geos = distinct_geos(
            &self
                .do_regions
                .iter()
                .map(|r| r.slug.clone())
                .collect::<Vec<_>>(),
        );
        let nudge = if lh_count < 2 {
            format!(
                "Spread: {lh_count} lighthouse(s) — add a 2nd in a DIFFERENT geo for relay + \
                 etcd-quorum redundancy ({geos} geo(s) available)."
            )
        } else {
            format!("{lh_count} lighthouses — keep them spread across distinct geos ({geos} available).")
        };
        out.push(text(nudge).colr(palette.warning.into_cosmic_color()).into());

        // ── New-Mesh genesis wizard (DATACENTER-18) ──
        out.push(
            text("New mesh (genesis)")
                .size(f32::from(spacing::BASE[5]))
                .into(),
        );
        let genesis_picker: Element<'_, crate::Message> = if region_slugs.is_empty() {
            text("Load regions first.")
                .colr(palette.text_muted.into_cosmic_color())
                .into()
        } else {
            pick_list(region_slugs, self.genesis_region.clone(), |v| {
                crate::Message::Datacenter(Message::GenesisRegionPicked(v))
            })
            .into()
        };
        out.push(
            row![
                text_input("new mesh name", &self.genesis_name)
                    .on_input(|v| crate::Message::Datacenter(Message::GenesisNameChanged(v)))
                    .width(Length::FillPortion(2)),
                genesis_picker,
                pbtn("Give birth", Message::GenesisSubmit, palette),
            ]
            .spacing(f32::from(spacing::BASE[2]))
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .into(),
        );
        out.push(
            text("Flow: name \u{2192} region \u{2192} CA \u{2192} found lighthouse \u{2192} DNS \u{2192} first join token.")
                .colr(palette.text_muted.into_cosmic_color())
                .into(),
        );

        // ── Ephemeral test-mesh + build-farm scale (DATACENTER-21) ──
        out.push(
            text("Ephemeral test-mesh")
                .size(f32::from(spacing::BASE[5]))
                .into(),
        );
        out.push(
            row![
                text("Nodes:").colr(palette.text_muted.into_cosmic_color()),
                text_input("3", &self.testmesh_n)
                    .on_input(|v| crate::Message::Datacenter(Message::TestmeshNChanged(v)))
                    .width(Length::Fixed(60.0)),
                pbtn("Spin", Message::TestmeshSpinClicked, palette),
            ]
            .spacing(f32::from(spacing::BASE[2]))
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .into(),
        );
        for m in self.rows.iter().filter(|r| r.kind == "testmesh") {
            out.push(
                row![
                    text(format!("test-mesh {}", m.id)).colr(palette.text.into_cosmic_color()),
                    sbtn(
                        "Teardown",
                        Message::TestmeshTeardownClicked(m.id.clone()),
                        palette
                    ),
                ]
                .spacing(f32::from(spacing::BASE[2]))
                .align_y(cosmic::iced::alignment::Vertical::Center)
                .into(),
            );
        }
        out.push(
            text("Build-farm scale")
                .size(f32::from(spacing::BASE[5]))
                .into(),
        );
        out.push(
            row![
                text("Build VMs:").colr(palette.text_muted.into_cosmic_color()),
                text_input("2", &self.farm_scale_n)
                    .on_input(|v| crate::Message::Datacenter(Message::FarmScaleChanged(v)))
                    .width(Length::Fixed(60.0)),
                pbtn("Apply scale", Message::FarmScaleClicked, palette),
            ]
            .spacing(f32::from(spacing::BASE[2]))
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .into(),
        );
        out
    }
}

/// A Secondary (outline) action button wired to a panel [`Message`] — the concise
/// builder the per-tab action rows use (the message is owned, so the element is
/// `'static`). mde-theme tokens only (via `variant_button`).
fn sbtn(label: &str, msg: Message, palette: Palette) -> Element<'static, crate::Message> {
    variant_button(
        label.to_string(),
        ButtonVariant::Secondary,
        Some(crate::Message::Datacenter(msg)),
        palette,
    )
}

/// A Primary (filled) action button wired to a panel [`Message`].
fn pbtn(label: &str, msg: Message, palette: Palette) -> Element<'static, crate::Message> {
    variant_button(
        label.to_string(),
        ButtonVariant::Primary,
        Some(crate::Message::Datacenter(msg)),
        palette,
    )
}

/// Wrap a body element in the panel's standard bordered surface card (surface
/// background + 1px border + small radius). mde-theme tokens only — no raw hex.
fn surface_card<'a>(
    body: impl Into<Element<'a, crate::Message>>,
    palette: Palette,
) -> Element<'a, crate::Message> {
    container(body)
        .padding(f32::from(spacing::BASE[3]))
        .width(Length::Fill)
        .style(move |_theme| container::Style {
            background: Some(cosmic::iced::Background::Color(
                palette.surface.into_cosmic_color(),
            )),
            border: cosmic::iced::Border {
                color: palette.border.into_cosmic_color(),
                width: 1.0,
                radius: f32::from(spacing::BASE[1]).into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// DATACENTER-16 — render one host's phased wake progress bar: the host, a filled
/// text bar, and a `pct% · phase · ETA` caption. mde-theme tokens only.
fn power_progress_view(ps: &PowerStatus, palette: Palette) -> Element<'static, crate::Message> {
    let bar = progress_bar(ps.pct_clamped());
    let eta = if ps.eta_secs.is_empty() {
        String::new()
    } else {
        format!(" \u{00b7} ETA {}s", ps.eta_secs)
    };
    row![
        text(ps.host.clone())
            .colr(palette.text.into_cosmic_color())
            .width(Length::FillPortion(2)),
        text(bar)
            .colr(palette.accent.into_cosmic_color())
            .width(Length::FillPortion(2)),
        text(format!("{}% \u{00b7} {}{}", ps.pct_clamped(), ps.phase_label(), eta))
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::FillPortion(3)),
    ]
    .spacing(f32::from(spacing::BASE[2]))
    .into()
}

/// DATACENTER-13 — render the unified IP/DNS correlation table (UniFi lease ↔ DO
/// DNS ↔ overlay IP). Empty fields show "—"; an empty set shows a load hint.
/// mde-theme tokens only.
fn ipdns_table_view(
    entries: &[IpDnsEntry],
    palette: Palette,
) -> Vec<Element<'static, crate::Message>> {
    let mut out: Vec<Element<'static, crate::Message>> = Vec::new();
    if entries.is_empty() {
        out.push(
            text("No IP/DNS correlation loaded — Refresh to read it.")
                .colr(palette.text_muted.into_cosmic_color())
                .into(),
        );
        return out;
    }
    out.push(
        row![
            text("Host")
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::FillPortion(2)),
            text("Lease")
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::FillPortion(2)),
            text("DNS")
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::FillPortion(2)),
            text("Overlay")
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::FillPortion(2)),
        ]
        .spacing(f32::from(spacing::BASE[2]))
        .into(),
    );
    for e in entries {
        let f = |s: &str| {
            if s.is_empty() {
                "\u{2014}".to_string()
            } else {
                s.to_string()
            }
        };
        out.push(
            row![
                text(f(&e.name))
                    .colr(palette.text.into_cosmic_color())
                    .width(Length::FillPortion(2)),
                text(f(&e.lease_ip))
                    .colr(palette.text_muted.into_cosmic_color())
                    .width(Length::FillPortion(2)),
                text(f(&e.dns))
                    .colr(palette.text_muted.into_cosmic_color())
                    .width(Length::FillPortion(2)),
                text(f(&e.overlay_ip))
                    .colr(palette.accent.into_cosmic_color())
                    .width(Length::FillPortion(2)),
            ]
            .spacing(f32::from(spacing::BASE[2]))
            .into(),
        );
    }
    out
}

/// The number of resource cards per row in the [`card_grid`]. A fixed column
/// count keeps the layout deterministic for the tests while still wrapping long
/// resource lists into a grid (vs one tall column of rows). Tuned so a card —
/// status dot + label + actions — has room without crowding.
const CARD_GRID_COLS: usize = 3;

// ── MOTION-FEEDBACK-2 — card-grid reveal / hover / selection motion ───────────

/// MOTION-FEEDBACK-2 — the staggered-reveal cap (Q acceptance: stagger ≤8). Cards
/// past this index share the last delay slot, so a large zone reveals as one
/// quick wave instead of a long crawl, and the reveal always finishes in a bounded
/// time regardless of resource count.
const REVEAL_STAGGER_CAP: usize = 8;

/// MOTION-FEEDBACK-2 — the per-card stagger step. Each (capped) card index `i`
/// delays its slide-in by `i * STAGGER_STEP`, so the grid fills top-left → bottom
/// in a brisk cascade rather than all at once.
const REVEAL_STAGGER_STEP: Duration = Duration::from_millis(40);

/// MOTION-FEEDBACK-2 — how far a card rises into place on reveal (px, from below).
/// A small slide, paired with the fade, reading as "settling in" without layout
/// thrash. Dropped under reduce-motion (the slide collapses to a pure fade).
const CARD_REVEAL_RISE_PX: f32 = 8.0;

/// MOTION-FEEDBACK-2 — how far the selected card's animated accent ring widens at
/// full grow-in (px). Sits on top of the card's existing 1px border.
const CARD_SELECT_RING_PX: f32 = 2.0;

/// MOTION-FEEDBACK-2 — the card's resting padding, also the budget for the
/// vertical-offset nudge. The reveal slide (downward, `+CARD_REVEAL_RISE_PX`) and
/// the hover-lift (upward, `-HOVER_LIFT_PX`) are applied by shifting padding from
/// one side to the other; that only preserves the card's total height while the
/// summed offset stays within this budget (otherwise a side clamps to 0 and the
/// grid reflows). Both directions are individually bounded by this assert, so a
/// future token/constant retune that would break height-preservation fails the
/// build instead of silently thrashing layout.
const CARD_PAD_PX: u16 = spacing::BASE[3];
// Each offset direction is bounded by the padding budget independently: the reveal
// slides the card *down* by up to `CARD_REVEAL_RISE_PX` (shrinks the bottom pad)
// and the hover lifts it *up* by `HOVER_LIFT_PX` (shrinks the top pad). Asserting
// each separately keeps both invariants visible to a future retune.
const _: () = assert!(
    CARD_REVEAL_RISE_PX <= CARD_PAD_PX as f32,
    "reveal rise must stay within the padding budget so the bottom-pad nudge \
     preserves card height (else the grid reflows mid-reveal)"
);
const _: () = assert!(
    mde_theme::feedback::HOVER_LIFT_PX <= CARD_PAD_PX as f32,
    "hover lift must stay within the padding budget so the top-pad nudge \
     preserves card height (else the grid reflows on hover)"
);

/// MOTION-FEEDBACK-2 — the per-card slide-in `start` for card `index` off the
/// grid's `reveal_start`: the reveal origin plus this card's (capped) stagger
/// delay. Cards beyond [`REVEAL_STAGGER_CAP`] all use the cap's delay.
fn reveal_card_start(reveal_start: Instant, index: usize) -> Instant {
    let slot = index.min(REVEAL_STAGGER_CAP);
    reveal_start + REVEAL_STAGGER_STEP * u32::try_from(slot).unwrap_or(u32::MAX)
}

/// MOTION-FEEDBACK-2 — has the whole reveal finished at `now`? The last card to
/// animate is `card_count - 1` (its stagger slot capped at [`REVEAL_STAGGER_CAP`]);
/// once that card's slide-in (its delay plus the reduce-motion-aware mount
/// duration) has elapsed the reveal is done and the tick loop retires it. Keying
/// off the real last card — not the fixed cap slot — means a small grid stops
/// ticking the instant its cards have settled, and the `reduce_motion` duration
/// matches `slide_in`'s ≤80 ms cap so a reduced-motion reveal doesn't over-tick.
fn reveal_is_complete(
    reveal_start: Instant,
    now: Instant,
    card_count: usize,
    reduce_motion: bool,
) -> bool {
    // No cards ⇒ nothing to reveal ⇒ already complete.
    let last_index = match card_count.checked_sub(1) {
        Some(i) => i,
        None => return true,
    };
    let last_start = reveal_card_start(reveal_start, last_index);
    let mount = Motion::panel_mount().duration;
    let dur = if reduce_motion {
        mount.min(Duration::from_millis(mde_theme::motion::REDUCE_MOTION_CAP_MS))
    } else {
        mount
    };
    now.saturating_duration_since(last_start) >= dur
}

/// MOTION-FEEDBACK-2 — sleep one frame (~60 fps), then emit a [`Message::MotionTick`].
/// Re-armed from [`DatacenterPanel::tick_motion`] only while a reveal/selection is
/// in flight, so the chain stops itself at rest (MOTION-PERF-1 — no idle wakeups).
fn tick_motion_later() -> Task<crate::Message> {
    Task::perform(
        async {
            tokio::time::sleep(Duration::from_millis(16)).await;
        },
        |()| crate::Message::Datacenter(Message::MotionTick),
    )
}

/// MOTION-FEEDBACK-2 — the per-card motion inputs the view passes to
/// [`dc_card_view`]. Borrows the panel's selection [`Animator`] (read-only) so the
/// accent ring reads its live eased value without cloning.
struct CardMotion<'a> {
    /// The card's index in the visible grid order (drives the stagger slot).
    index: usize,
    /// The grid's reveal origin, or `None` once the reveal has settled.
    reveal_start: Option<Instant>,
    /// Whether this card is the selected/focused one (draws the accent ring).
    selected: bool,
    /// The selection accent animator (keyed by card id) — read for the ring's
    /// eased grow-in.
    selection: &'a Animator,
    /// Whether the pointer is currently over this card (drives the hover-lift).
    hovered: bool,
    /// When the hover state last changed — the hover-lift tween `start`.
    hover_since: Instant,
    /// One coherent frame timestamp shared across the whole grid.
    now: Instant,
    /// The live reduce-motion preference — collapses every movement to instant.
    reduce_motion: bool,
}

/// Arrange a list of resource cards into a responsive grid: rows of
/// [`CARD_GRID_COLS`] cards, each card claiming an equal portion so the grid
/// flexes with the panel width. A short final row is left-aligned (no phantom
/// padding cards). Pure layout glue — mde-theme spacing tokens only.
fn card_grid(mut cards: Vec<Element<'_, crate::Message>>) -> Element<'_, crate::Message> {
    let mut grid = column![].spacing(f32::from(spacing::BASE[2]));
    while !cards.is_empty() {
        let take = cards.len().min(CARD_GRID_COLS);
        // Drain the first `take` cards into this grid row, each an equal portion.
        let chunk: Vec<Element<'_, crate::Message>> = cards.drain(..take).collect();
        let mut line = row![].spacing(f32::from(spacing::BASE[2]));
        for card in chunk {
            line = line.push(container(card).width(Length::FillPortion(1)));
        }
        grid = grid.push(line);
    }
    grid.into()
}

/// A small colored status dot — the resource's liveness at a glance. The color is
/// an mde-theme palette token resolved by [`DcRow::status_dot`] (success / danger
/// / warning / muted), never a raw hex. Rendered as a filled bullet glyph in that
/// token color, paired with the raw status word so the dot is labelled (§4-clean:
/// color is reinforced by text, not the sole signal).
fn status_dot_view(r: &DcRow, palette: Palette) -> Element<'static, crate::Message> {
    let color = r.status_dot(palette);
    let label = if r.status.is_empty() {
        "unknown".to_string()
    } else {
        r.status.clone()
    };
    row![
        text("\u{25cf}").colr(color.into_cosmic_color()),
        text(label).colr(palette.text_muted.into_cosmic_color()),
    ]
    .spacing(f32::from(spacing::BASE[1]))
    .align_y(cosmic::iced::alignment::Vertical::Center)
    .into()
}

/// Render one datacenter resource as a **card**: a bordered surface with a
/// status-dot header (kind + color-dot liveness), the resource label, and a
/// kind-appropriate readout (sr capacity / net bridge / bare status). VM cards
/// additionally carry Start / Stop / Reboot power buttons (the `action/dc/vm-
/// power` RPC for the card's dom0) plus Snapshot / Clone / Delete. When
/// `confirming` is set, the Delete button is replaced by an inline "Really
/// delete?" confirm + Cancel prompt — only the confirm fires the destructive
/// `action/dc/vm-delete` RPC. mde-theme tokens only (surface / border / status
/// dot all from the palette).
///
/// MOTION-FEEDBACK-2 — `motion` layers the card-grid micro-interactions on top of
/// the static render: a capped staggered fade+slide reveal when the zone loads,
/// a hover-lift while the pointer is over the card, and an animated accent ring on
/// the selected card. All collapse to instant / no-movement under reduce-motion.
/// The whole card is a `mouse_area` so clicking selects it and pointer enter/leave
/// drives the hover state — runtime-reachable through the panel's update.
fn dc_card_view<'a>(
    r: &DcRow,
    palette: Palette,
    confirming: bool,
    bulk: Option<bool>,
    motion: CardMotion<'_>,
) -> Element<'a, crate::Message> {
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
    // Card header: kind label + the color-dot status indicator.
    let header = row![
        text(r.kind.clone())
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::FillPortion(1)),
        status_dot_view(r, palette),
    ]
    .spacing(f32::from(spacing::BASE[2]))
    .align_y(cosmic::iced::alignment::Vertical::Center);
    let mut card = column![
        header,
        text(label).colr(palette.text.into_cosmic_color()),
        text(status_or_capacity).colr(palette.text_muted.into_cosmic_color()),
    ]
    .spacing(f32::from(spacing::BASE[2]));

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
        card = card.push(actions);
        // DATACENTER-11 — lifecycle beyond power: suspend/resume, migrate, console.
        let suspended = matches!(
            r.status.to_ascii_lowercase().as_str(),
            "suspended" | "paused"
        );
        let suspend_btn = variant_button(
            if suspended { "Resume" } else { "Suspend" }.to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::SuspendClicked {
                uuid: r.id.clone(),
                dom0: r.host.clone(),
                resume: suspended,
            })),
            palette,
        );
        let migrate_btn = variant_button(
            "Migrate".to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::MigrateClicked {
                uuid: r.id.clone(),
                dom0: r.host.clone(),
            })),
            palette,
        );
        let console_btn = variant_button(
            "Console".to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::VmConsoleClicked {
                uuid: r.id.clone(),
                dom0: r.host.clone(),
            })),
            palette,
        );
        let mut actions2 =
            row![suspend_btn, migrate_btn, console_btn].spacing(f32::from(spacing::BASE[1]));
        // DATACENTER-11 — multi-select bulk: a Select toggle when bulk mode is on.
        if let Some(selected) = bulk {
            actions2 = actions2.push(variant_button(
                if selected { "✓ Selected" } else { "Select" }.to_string(),
                if selected {
                    ButtonVariant::Primary
                } else {
                    ButtonVariant::Ghost
                },
                Some(crate::Message::Datacenter(Message::BulkSelectToggled(
                    r.id.clone(),
                ))),
                palette,
            ));
        }
        card = card.push(actions2);
    }

    // ── MOTION-FEEDBACK-2 — resolve this frame's motion for the card ──────────
    //
    // Reveal: a capped staggered fade+slide-in off the grid's reveal origin. Under
    // reduce-motion `slide_in` collapses to a pure fade (no movement); once the
    // reveal has settled (or there is none) it returns the static, fully-opaque
    // frame, so a settled grid renders without any transform.
    let reveal = motion.reveal_start.map_or(
        mde_theme::animation::RenderParams {
            alpha: 1.0,
            translate_y: 0.0,
            scale: 1.0,
        },
        |start| {
            slide_in(
                reveal_card_start(start, motion.index),
                motion.now,
                CARD_REVEAL_RISE_PX,
                motion.reduce_motion,
            )
        },
    );
    // Hover-lift: the card rises HOVER_LIFT_PX while hovered, animating in/out over
    // Motion::hover(). Dropped under reduce-motion (hover stays a color/elevation
    // cue, not motion) — that contract lives in `lift_on_hover`.
    let lift = mde_theme::animation::lift_on_hover(
        motion.hover_since,
        motion.now,
        mde_theme::feedback::HOVER_LIFT_PX,
        motion.hovered,
        motion.reduce_motion,
    );
    // The reveal slide and the hover-lift are both vertical offsets — sum them into
    // one `translate_y`, applied as a top-padding nudge (the fork has no transform
    // widget; offsetting padding moves the surface without layout thrash).
    let translate_y = reveal.translate_y + lift.translate_y;
    let base_pad = f32::from(CARD_PAD_PX);
    // translate_y is negative when lifted (up), positive while the reveal slides up
    // from below; shift it from the top pad to the bottom (and vice-versa) so the
    // card's total height is preserved. The `.max(0.0)` is a defensive floor that
    // never triggers — `CARD_PAD_PX` is statically asserted to exceed both the
    // reveal rise and the hover lift, so neither side can reach 0.
    let top_pad = (base_pad + translate_y).max(0.0);
    let bottom_pad = (base_pad - translate_y).max(0.0);
    let alpha = reveal.alpha;

    // Selection accent: the focused card draws an animated accent ring that grows
    // in over Motion::focus() (instant under reduce-motion). The eased value comes
    // from the shared selection Animator keyed by card id; an unselected card reads
    // a zero-width ring (the base 1px border).
    let ring_t = if motion.selected {
        motion
            .selection
            .value(&r.id, motion.now, Motion::focus().easing)
    } else {
        0.0
    };
    let border_width = 1.0 + CARD_SELECT_RING_PX * ring_t;
    // Blend the resting border toward the accent token by the ring's progress (in
    // the fork's f32 Color space), so the selected card's outline reads as the
    // accent. Never a raw hex — both ends are live palette tokens.
    let base_border = palette.border.into_cosmic_color();
    let accent = palette.accent.into_cosmic_color();
    let border_color = cosmic::iced::Color {
        r: lerp_f32(base_border.r, accent.r, ring_t),
        g: lerp_f32(base_border.g, accent.g, ring_t),
        b: lerp_f32(base_border.b, accent.b, ring_t),
        a: lerp_f32(base_border.a, accent.a, ring_t),
    };
    let surface = palette.surface;
    let radius = f32::from(spacing::BASE[1]);

    let styled = container(card)
        .padding(cosmic::iced::Padding {
            top: top_pad,
            right: base_pad,
            bottom: bottom_pad,
            left: base_pad,
        })
        .width(Length::Fill)
        .style(move |_theme| container::Style {
            background: Some(cosmic::iced::Background::Color(
                crate::cosmic_compat::with_alpha(surface.into_cosmic_color(), alpha),
            )),
            border: cosmic::iced::Border {
                color: crate::cosmic_compat::with_alpha(border_color, alpha),
                width: border_width,
                radius: radius.into(),
            },
            ..container::Style::default()
        });

    // The whole card is clickable (select) + hover-tracked. Selection + hover are
    // pure panel-state messages routed back through `update` — runtime-reachable.
    let id = r.id.clone();
    let enter_id = r.id.clone();
    mouse_area(styled)
        .on_press(crate::Message::Datacenter(Message::CardSelected(id)))
        .on_enter(crate::Message::Datacenter(Message::CardHovered(Some(
            enter_id,
        ))))
        .on_exit(crate::Message::Datacenter(Message::CardHovered(None)))
        .into()
}

/// Render one `Topology`-view group header — a clickable collapse/expand toggle.
/// Real host groups (`header.kind == "host"`) show the host label, its dom0 IP,
/// and a compact CPU/mem readout; the synthetic Prod/Gateway group (empty
/// `kind`/`id`) shows its name. The whole header is a button carrying the
/// `HeaderClicked(key)` message (key = the host id, or "" for the synthetic
/// group). The leading glyph (`▾` open / `▸` collapsed) signals state. mde-theme
/// tokens only — color comes from the button variant, sizes from `spacing::*`.
fn topology_header_view(
    header: &DcRow,
    child_count: usize,
    is_open: bool,
    palette: Palette,
) -> Element<'static, crate::Message> {
    let glyph = if is_open { "▾" } else { "▸" };
    let label = if header.kind == "host" {
        let name = if header.name.is_empty() {
            header.id.clone()
        } else {
            header.name.clone()
        };
        // Compact host metric readout, when the host reported any.
        let mut meta = String::new();
        if !header.cpu.is_empty() {
            meta.push_str(&format!(" · {} vCPU", header.cpu));
        }
        if !header.mem_total_mb.is_empty() {
            meta.push_str(&format!(" · {} MB", header.mem_total_mb));
        }
        format!(
            "{glyph} Host {name} ({})  [{}]{}",
            header.id, child_count, meta
        )
    } else {
        // Synthetic Prod / Gateway group.
        let name = if header.name.is_empty() {
            "Prod · DO / Gateway".to_string()
        } else {
            header.name.clone()
        };
        format!("{glyph} {name}  [{child_count}]")
    };
    let key = header.id.clone();
    variant_button(
        label,
        ButtonVariant::Secondary,
        Some(crate::Message::Datacenter(Message::HeaderClicked(key))),
        palette,
    )
}

/// Render one nested child row under a `Topology` group header — indented with a
/// connector glyph (`└─` for the last child, `├─` otherwise) so the tree reads
/// as a map. Shows the resource kind, its label, and a kind-appropriate readout
/// (sr capacity / net bridge / bare status), matching `dc_card_view`'s logic but
/// read-only (no power/delete controls in the map). mde-theme tokens only.
fn topology_child_view(
    r: &DcRow,
    last: bool,
    palette: Palette,
) -> Element<'static, crate::Message> {
    let connector = if last { "  └─" } else { "  ├─" };
    let label = if r.name.is_empty() {
        r.id.clone()
    } else {
        r.name.clone()
    };
    let status_or_capacity = if r.kind == "sr" {
        r.capacity_readout().unwrap_or_else(|| r.status.clone())
    } else if r.kind == "net" && !r.bridge.is_empty() {
        format!("{} · {}", r.status, r.bridge)
    } else {
        r.status.clone()
    };
    let line = row![
        text(connector.to_string())
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::FillPortion(1)),
        text(r.kind.clone())
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::FillPortion(1)),
        text(label)
            .colr(palette.text.into_cosmic_color())
            .width(Length::FillPortion(3)),
        text(status_or_capacity)
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::FillPortion(2)),
    ]
    .spacing(f32::from(spacing::BASE[3]));
    container(line)
        .padding(f32::from(spacing::BASE[2]))
        .width(Length::Fill)
        .into()
}

/// Render the **Build → Eagle → DO** promotion strip: a horizontal version matrix
/// of the three canonical stages (in that order), each a small card showing the
/// stage name, its version, and a readiness chip, with `→` glyphs between. Fed by
/// [`promote_matrix`] so absent stages render as "—" placeholders rather than
/// vanishing. mde-theme tokens only — card surface / border / chip color all come
/// from the palette, never raw hex.
fn promote_strip_view(
    stages: &[PromoteStage],
    palette: Palette,
) -> Element<'static, crate::Message> {
    let matrix = promote_matrix(stages);
    let mut strip = row![].spacing(f32::from(spacing::BASE[2]));
    let n = matrix.len();
    for (i, stage) in matrix.iter().enumerate() {
        strip = strip.push(promote_card_view(stage, palette));
        if i + 1 < n {
            // Arrow glyph between cards — muted so the cards lead the eye.
            strip = strip.push(
                container(text("→").colr(palette.text_muted.into_cosmic_color()))
                    .padding(f32::from(spacing::BASE[2])),
            );
        }
    }
    strip.into()
}

/// Render the **Health** section of the Overview: a one-line `N ok · M warn · K
/// fail` summary (each count in its mde-theme token — `success` / `warning` /
/// `danger`) followed by an alert row for every non-ok check (its name + detail).
/// When every check is ok (and there's at least one), shows a single "✓ all
/// systems healthy" line; with no checks at all, an empty-state hint. Returns a
/// list of elements so the Overview column can push them in order. mde-theme
/// tokens only — no raw hex.
fn health_section_view(
    checks: &[HealthCheck],
    palette: Palette,
) -> Vec<Element<'static, crate::Message>> {
    let mut out: Vec<Element<'static, crate::Message>> = Vec::new();
    if checks.is_empty() {
        out.push(
            text("No datacenter health checks reported yet.")
                .colr(palette.text_muted.into_cosmic_color())
                .into(),
        );
        return out;
    }
    let (ok, warn, fail) = health_summary(checks);
    // One-line tally — each count colored by its severity token.
    let summary = row![
        text(format!("{ok} ok")).colr(palette.success.into_cosmic_color()),
        text(" · ").colr(palette.text_muted.into_cosmic_color()),
        text(format!("{warn} warn")).colr(palette.warning.into_cosmic_color()),
        text(" · ").colr(palette.text_muted.into_cosmic_color()),
        text(format!("{fail} fail")).colr(palette.danger.into_cosmic_color()),
    ]
    .spacing(f32::from(spacing::BASE[1]));
    out.push(summary.into());

    if warn == 0 && fail == 0 {
        out.push(
            text("✓ all systems healthy")
                .colr(palette.success.into_cosmic_color())
                .into(),
        );
        return out;
    }
    // Alert list — every non-ok check, name + detail, colored by severity.
    for c in checks.iter().filter(|c| c.status != "ok") {
        let color = if c.status == "warn" {
            palette.warning
        } else {
            palette.danger
        };
        let detail = if c.detail.is_empty() {
            c.status.clone()
        } else {
            c.detail.clone()
        };
        let line = row![
            text(c.check.clone())
                .colr(color.into_cosmic_color())
                .width(Length::FillPortion(1)),
            text(detail)
                .colr(palette.text.into_cosmic_color())
                .width(Length::FillPortion(3)),
        ]
        .spacing(f32::from(spacing::BASE[3]));
        out.push(
            container(line)
                .padding(f32::from(spacing::BASE[2]))
                .width(Length::Fill)
                .into(),
        );
    }
    out
}

/// Render the **Recent Tofu runs** section of the Overview: a newest-first
/// run-log of the datacenter action jobs (`event/dc/job/*`) filtered to the Tofu
/// verbs via [`recent_tofu_runs`]. Each row pairs the verb (the `dc/tofu-` prefix
/// stripped → plan / apply / destroy / state) with a status chip whose color comes
/// from a mde-theme token (`success` for ok, `danger` for error, `warning` for
/// pending/anything else). When there are no Tofu runs, a single "no recent Tofu
/// runs" empty-state line. Returns a list of elements so the Overview column can
/// push them in order. mde-theme tokens only — no raw hex.
fn recent_tofu_runs_view(
    jobs: &[JobRow],
    palette: Palette,
) -> Vec<Element<'static, crate::Message>> {
    let mut out: Vec<Element<'static, crate::Message>> = Vec::new();
    let runs = recent_tofu_runs(jobs);
    if runs.is_empty() {
        out.push(
            text("no recent Tofu runs")
                .colr(palette.text_muted.into_cosmic_color())
                .into(),
        );
        return out;
    }
    for run in &runs {
        // Strip the `dc/tofu-` prefix so the verb reads cleanly (plan / apply /
        // destroy / state); fall back to the raw action if it doesn't match.
        let verb = run
            .action
            .strip_prefix("dc/tofu-")
            .unwrap_or(&run.action)
            .to_string();
        // Status chip color tracks the outcome.
        let (chip_color, chip_text) = match run.status.as_str() {
            "ok" => (palette.success, "ok".to_string()),
            "error" => (palette.danger, "error".to_string()),
            "" => (palette.warning, "pending".to_string()),
            other => (palette.warning, other.to_string()),
        };
        let line = row![
            text(verb)
                .colr(palette.text.into_cosmic_color())
                .width(Length::FillPortion(2)),
            text(chip_text)
                .colr(chip_color.into_cosmic_color())
                .width(Length::FillPortion(1)),
        ]
        .spacing(f32::from(spacing::BASE[3]));
        out.push(
            container(line)
                .padding(f32::from(spacing::BASE[2]))
                .width(Length::Fill)
                .into(),
        );
    }
    out
}

/// Render one labelled sparkline row: a muted `label`, the block-glyph
/// [`sparkline`] of `points`, and the last value as a trailing readout. A series
/// with fewer than two points has no trend to plot, so the line falls back to a
/// muted "—". mde-theme tokens only — the label / line / value all read off the
/// palette, never raw hex.
fn sparkline_row(
    label: &str,
    points: &[f32],
    palette: Palette,
) -> Element<'static, crate::Message> {
    let spark = sparkline(points);
    let last = points.last().copied();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let last_text = last.map_or_else(|| "—".to_string(), |v| format!("{}", v.round() as i64));
    // A single sample has no trend to plot — show just the value so the row reads
    // honestly rather than drawing a one-cell "line".
    let line_text = if spark.is_empty() || points.len() < 2 {
        "—".to_string()
    } else {
        spark
    };
    row![
        text(label.to_string())
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::FillPortion(2)),
        text(line_text)
            .colr(palette.accent.into_cosmic_color())
            .width(Length::FillPortion(3)),
        text(last_text)
            .colr(palette.text.into_cosmic_color())
            .width(Length::FillPortion(1)),
    ]
    .spacing(f32::from(spacing::BASE[2]))
    .into()
}

/// Render the Overview's **rolling-history sparklines** — one labelled
/// [`sparkline`] per tracked series (total resources, running compute, health-ok,
/// health-alerts) computed off the panel's capped history ring buffer. With fewer
/// than two samples there's no trend yet → a single muted hint line. Returns a
/// list of elements so the Overview column can push them in order. mde-theme
/// tokens only.
fn sparklines_view(
    history: &VecDeque<HistorySample>,
    palette: Palette,
) -> Vec<Element<'static, crate::Message>> {
    let mut out: Vec<Element<'static, crate::Message>> = Vec::new();
    if history.len() < 2 {
        out.push(
            text("Trend builds as the Bus is polled — refresh to add samples.")
                .colr(palette.text_muted.into_cosmic_color())
                .into(),
        );
        return out;
    }
    #[allow(clippy::cast_precision_loss)]
    let series = |f: fn(&HistorySample) -> usize| -> Vec<f32> {
        history.iter().map(|s| f(s) as f32).collect()
    };
    out.push(sparkline_row(
        "Resources",
        &series(|s| s.resources),
        palette,
    ));
    out.push(sparkline_row("Running", &series(|s| s.running), palette));
    out.push(sparkline_row(
        "Health ok",
        &series(|s| s.health_ok),
        palette,
    ));
    out.push(sparkline_row(
        "Alerts",
        &series(|s| s.health_alerts),
        palette,
    ));
    out
}

/// Render the Overview's **version matrix** — `farm / Eagle / each lighthouse` —
/// off [`VersionMatrix::project`]. A header row plus one line per target: its
/// label, the version it's pinned to, and a readiness chip whose color comes from
/// a mde-theme token (`success` for ready, `warning` for pending, `text_muted` for
/// unknown). Where the promote strip shows only the three pipeline stages, this
/// adds a row per live lighthouse so the operator sees per-host version drift at a
/// glance. mde-theme tokens only — no raw hex.
fn version_matrix_view(
    matrix: &VersionMatrix,
    palette: Palette,
) -> Vec<Element<'static, crate::Message>> {
    let mut out: Vec<Element<'static, crate::Message>> = Vec::new();
    // Column header so the version / status columns read.
    out.push(
        row![
            text("Target")
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::FillPortion(2)),
            text("Version")
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::FillPortion(2)),
            text("State")
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::FillPortion(1)),
        ]
        .spacing(f32::from(spacing::BASE[3]))
        .into(),
    );
    for vr in &matrix.rows {
        let (chip_color, chip_text) = match vr.status.as_str() {
            "ready" => (palette.success, "ready".to_string()),
            "pending" => (palette.warning, "pending".to_string()),
            other => (
                palette.text_muted,
                if other.is_empty() {
                    "—".to_string()
                } else {
                    other.to_string()
                },
            ),
        };
        let line = row![
            text(vr.target.clone())
                .colr(palette.text.into_cosmic_color())
                .width(Length::FillPortion(2)),
            text(vr.version.clone())
                .colr(palette.accent.into_cosmic_color())
                .width(Length::FillPortion(2)),
            text(chip_text)
                .colr(chip_color.into_cosmic_color())
                .width(Length::FillPortion(1)),
        ]
        .spacing(f32::from(spacing::BASE[3]));
        out.push(
            container(line)
                .padding(f32::from(spacing::BASE[2]))
                .width(Length::Fill)
                .into(),
        );
    }
    out
}

/// Render one promotion-stage card: the stage label, its version, and a readiness
/// chip whose color comes from a mde-theme token (`success` for ready, `warning`
/// for pending, `text_muted` for an unknown/absent placeholder). mde-theme tokens
/// only.
fn promote_card_view(stage: &PromoteStage, palette: Palette) -> Element<'static, crate::Message> {
    // Human stage label.
    let label = match stage.stage.as_str() {
        "build" => "Build",
        "eagle" => "Eagle",
        "do" => "DO",
        other => other,
    }
    .to_string();
    // The chip color tracks readiness; the text is the raw status (or "—" when
    // the stage is an unknown placeholder, so the chip reads cleanly).
    let (chip_color, chip_text) = match stage.status.as_str() {
        "ready" => (palette.success, "ready".to_string()),
        "pending" => (palette.warning, "pending".to_string()),
        other => (
            palette.text_muted,
            if other.is_empty() {
                "—".to_string()
            } else {
                other.to_string()
            },
        ),
    };
    let version = if stage.version.is_empty() {
        "—".to_string()
    } else {
        stage.version.clone()
    };
    let card = column![
        text(label)
            .colr(palette.text.into_cosmic_color())
            .size(f32::from(spacing::BASE[4])),
        text(version).colr(palette.accent.into_cosmic_color()),
        text(chip_text).colr(chip_color.into_cosmic_color()),
    ]
    .spacing(f32::from(spacing::BASE[1]));
    container(card)
        .padding(f32::from(spacing::BASE[3]))
        .style(move |_theme| container::Style {
            background: Some(cosmic::iced::Background::Color(
                palette.surface.into_cosmic_color(),
            )),
            border: cosmic::iced::Border {
                color: palette.border.into_cosmic_color(),
                width: 1.0,
                radius: f32::from(spacing::BASE[1]).into(),
            },
            ..container::Style::default()
        })
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
        promote: project_promote(&events),
        health: project_health(&events),
        jobs: project_jobs(&events),
        power: project_power(&events),
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

/// Fire the `action/dc/tofu-state` Bus RPC (blocking — runs on a `spawn_blocking`
/// thread) and translate the reply into the workspace's managed resources + a
/// drift flag. Mirrors `tofu_plan` exactly: a Persist + `mde_bus::rpc::request`
/// round trip wrapped in a local tokio runtime because `request` borrows a
/// non-`Send` `Persist` across its internal await. The reply body is
/// `{"ok":true,"resources":[..],"drift":bool}` (→ the resource names + drift) or
/// `{"error":".."}` (→ the error text).
fn tofu_state(workspace: &str) -> Result<(Vec<String>, bool), String> {
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
            "action/dc/tofu-state",
            mde_bus::hooks::config::Priority::Default,
            Some("tofu-state"),
            Some(&body),
            Duration::from_secs(120),
        )
        .await
        .map_err(|e| e.to_string())
    })?;
    let raw = reply.body.unwrap_or_default();
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("bad tofu-state reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        let resources = v
            .get("resources")
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();
        let drift = v
            .get("drift")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        return Ok((resources, drift));
    }
    Err(format!("unexpected tofu-state reply: {raw}"))
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

/// Fire the `action/dc/dr-backup` Bus RPC (blocking — runs on a `spawn_blocking`
/// thread) and translate the reply into the backup path. Only reached after the
/// typed confirm, so it always sends `"confirm":true`. Mirrors `tofu_apply`
/// exactly: a Persist + `mde_bus::rpc::request` round trip wrapped in a local
/// tokio runtime because `request` borrows a non-`Send` `Persist` across its
/// internal await. The reply body is `{"ok":true,"path":".."}` (→ the path) or
/// `{"error":".."}` (→ the error text).
fn dr_backup() -> Result<String, String> {
    let Some(dir) = mde_bus::default_data_dir() else {
        return Err("no Bus data dir".to_string());
    };
    let body = serde_json::json!({ "confirm": true }).to_string();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    let reply = rt.block_on(async {
        let persist = mde_bus::persist::Persist::open(dir).map_err(|e| e.to_string())?;
        mde_bus::rpc::request(
            &persist,
            "action/dc/dr-backup",
            mde_bus::hooks::config::Priority::Default,
            Some("dr-backup"),
            Some(&body),
            Duration::from_secs(600),
        )
        .await
        .map_err(|e| e.to_string())
    })?;
    let raw = reply.body.unwrap_or_default();
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("bad dr-backup reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    if let Some(path) = v.get("path").and_then(serde_json::Value::as_str) {
        return Ok(path.to_string());
    }
    Err(format!("unexpected dr-backup reply: {raw}"))
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

/// Collapse an action RPC result into a single status line — both arms are
/// surfaced verbatim to the operator (success text or error text). Shared by the
/// many fire-and-status handlers.
fn flatten(r: Result<String, String>) -> String {
    match r {
        Ok(s) => s,
        Err(e) => e,
    }
}

/// Generic datacenter action RPC: fire `action/dc/<verb>` with `body`, returning
/// the parsed reply JSON (any `{"error":..}` surfaced as `Err`). Blocking — call
/// on a `spawn_blocking` thread. Single-sources the Persist + `mde_bus::rpc::request`
/// round trip the per-verb helpers above each open-code, wrapped in a local
/// current-thread runtime because `request` borrows a non-`Send` `Persist` across
/// its internal await (§6 — one copy of the boilerplate).
fn dc_rpc(
    verb: &str,
    body: &serde_json::Value,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let Some(dir) = mde_bus::default_data_dir() else {
        return Err("no Bus data dir".to_string());
    };
    let topic = format!("action/dc/{verb}");
    let body_s = body.to_string();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    let reply = rt.block_on(async {
        let persist = mde_bus::persist::Persist::open(dir).map_err(|e| e.to_string())?;
        mde_bus::rpc::request(
            &persist,
            &topic,
            mde_bus::hooks::config::Priority::Default,
            Some(verb),
            Some(&body_s),
            timeout,
        )
        .await
        .map_err(|e| e.to_string())
    })?;
    let raw = reply.body.unwrap_or_default();
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("bad {verb} reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    Ok(v)
}

/// Fire a datacenter action verb that just needs an ok/status string back, and
/// translate `{"summary"|"status":..}` / `{"ok":true}` into a status line.
fn dc_action(verb: &str, body: &serde_json::Value, timeout: Duration) -> Result<String, String> {
    let v = dc_rpc(verb, body, timeout)?;
    if let Some(s) = v.get("summary").and_then(serde_json::Value::as_str) {
        return Ok(s.to_string());
    }
    if let Some(s) = v.get("status").and_then(serde_json::Value::as_str) {
        return Ok(s.to_string());
    }
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(format!("{verb}: ok"));
    }
    Err(format!("unexpected {verb} reply: {v}"))
}

/// Fire `action/dc/vm-console {uuid}` and return the noVNC console URL the worker
/// minted. Blocking — call on a `spawn_blocking` thread.
fn vm_console(uuid: &str, dom0: &str) -> Result<String, String> {
    let v = dc_rpc(
        "vm-console",
        &serde_json::json!({"uuid": uuid, "dom0": dom0}),
        Duration::from_secs(60),
    )?;
    v.get("url")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("no console url in reply: {v}"))
}

/// Fire `action/dc/vm-bulk {uuids[],op}` and return the per-item `(uuid,status)`
/// results so the panel can render per-VM progress. When the worker doesn't itemize
/// the reply, every requested uuid is marked `ok` (the call succeeded as a whole).
fn vm_bulk(uuids: &[String], op: &str) -> Result<Vec<(String, String)>, String> {
    let v = dc_rpc(
        "vm-bulk",
        &serde_json::json!({"uuids": uuids, "op": op}),
        Duration::from_secs(600),
    )?;
    let itemized = v
        .get("results")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    let uuid = e.get("uuid").and_then(serde_json::Value::as_str)?.to_string();
                    let status = e
                        .get("status")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("ok")
                        .to_string();
                    Some((uuid, status))
                })
                .collect::<Vec<_>>()
        });
    Ok(itemized.unwrap_or_else(|| uuids.iter().map(|u| (u.clone(), "ok".to_string())).collect()))
}

/// Fire `action/dc/host-patch {host,preview:true}` and return the list of VMs that
/// would be evacuated first (the impact preview). Blocking — `spawn_blocking`.
fn host_patch_preview(host: &str) -> Result<(String, Vec<String>), String> {
    let v = dc_rpc(
        "host-patch",
        &serde_json::json!({"host": host, "preview": true}),
        Duration::from_secs(120),
    )?;
    let vms = v
        .get("impact")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok((host.to_string(), vms))
}

/// Fire `action/dc/ipdns` and project the correlated lease/DNS/overlay rows.
fn ipdns_read() -> Result<Vec<IpDnsEntry>, String> {
    let v = dc_rpc("ipdns", &serde_json::json!({}), Duration::from_secs(60))?;
    Ok(parse_ipdns(&v))
}

/// Fire `action/dc/tofu-arm {on}` and return the resulting armed state.
fn tofu_arm(on: bool) -> Result<bool, String> {
    let v = dc_rpc(
        "tofu-arm",
        &serde_json::json!({"on": on}),
        Duration::from_secs(30),
    )?;
    Ok(v.get("armed")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(on))
}

/// Fire `action/dc/tofu-runlog {workspace}` and return the persisted run-log text.
fn tofu_runlog(ws: &str) -> Result<String, String> {
    let v = dc_rpc(
        "tofu-runlog",
        &serde_json::json!({"workspace": ws}),
        Duration::from_secs(60),
    )?;
    Ok(v.get("log")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string())
}

/// Fire `action/dc/do-regions` and project the region list for the picker.
fn do_regions_read() -> Result<Vec<DoRegion>, String> {
    let v = dc_rpc("do-regions", &serde_json::json!({}), Duration::from_secs(60))?;
    Ok(parse_do_regions(&v))
}

#[cfg(test)]
mod tests {
    use super::*;

    // DATACENTER-8 (saved views) ----------------------------------------------

    // The saved-views tests that touch the config FILE (and the panel ctor,
    // which loads it) mutate the process-wide `XDG_CONFIG_HOME`. Serialize them
    // behind one lock so they don't observe each other's env/file writes — the
    // same idiom `dbus.rs`'s focus tests use for a process-wide slot.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Point `XDG_CONFIG_HOME` at a fresh tempdir for the test's duration so
    /// `load_saved_views`/`save_saved_views` (and `DatacenterPanel::new`) read +
    /// write an isolated file, never the operator's. The returned `TempDir` keeps
    /// the dir alive until the test ends; the prior env value is restored.
    struct IsolatedConfig {
        _tmp: tempfile::TempDir,
        prev: Option<std::ffi::OsString>,
    }
    impl IsolatedConfig {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let prev = std::env::var_os("XDG_CONFIG_HOME");
            std::env::set_var("XDG_CONFIG_HOME", tmp.path());
            Self { _tmp: tmp, prev }
        }
    }
    impl Drop for IsolatedConfig {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
    }

    #[test]
    fn view_mode_slug_round_trips_every_variant() {
        for mode in [
            ViewMode::Overview,
            ViewMode::Zone,
            ViewMode::Tofu,
            ViewMode::Audit,
            ViewMode::Topology,
            ViewMode::Gateway,
            ViewMode::Provision,
        ] {
            assert_eq!(
                ViewMode::from_slug(mode.slug()),
                mode,
                "{mode:?} must round-trip through its slug"
            );
        }
        // An unknown / future slug falls back to Overview, not a panic or a drop.
        assert_eq!(ViewMode::from_slug("nonsense"), ViewMode::Overview);
        assert_eq!(ViewMode::from_slug(""), ViewMode::Overview);
    }

    fn a_view(name: &str) -> SavedView {
        SavedView {
            name: name.to_string(),
            view_mode: ViewMode::Zone.slug().to_string(),
            zone_tab: "prod".to_string(),
            filter: "web".to_string(),
        }
    }

    #[test]
    fn upsert_appends_then_overwrites_by_name_case_insensitively() {
        let mut s = SavedViews::default();
        assert!(s.upsert(a_view("Prod VMs")));
        assert!(s.upsert(a_view("Dev Hosts")));
        assert_eq!(s.views.len(), 2);
        // Re-saving the same name (different case) overwrites in place — no dup.
        let mut updated = a_view("PROD VMS");
        updated.filter = "db".to_string();
        assert!(s.upsert(updated));
        assert_eq!(s.views.len(), 2, "same-name save must not duplicate");
        assert_eq!(s.find("prod vms").unwrap().filter, "db");
    }

    #[test]
    fn upsert_rejects_a_blank_name() {
        let mut s = SavedViews::default();
        assert!(!s.upsert(a_view("   ")));
        assert!(s.is_empty());
    }

    #[test]
    fn upsert_caps_the_collection_evicting_the_oldest() {
        let mut s = SavedViews::default();
        for i in 0..(SAVED_VIEW_CAP + 3) {
            assert!(s.upsert(a_view(&format!("view-{i}"))));
        }
        assert_eq!(s.views.len(), SAVED_VIEW_CAP);
        // The three oldest were evicted; the newest survives.
        assert!(s.find("view-0").is_none());
        assert!(s.find("view-2").is_none());
        assert!(s.find(&format!("view-{}", SAVED_VIEW_CAP + 2)).is_some());
    }

    #[test]
    fn remove_drops_a_view_by_name() {
        let mut s = SavedViews::default();
        s.upsert(a_view("Keep"));
        s.upsert(a_view("Drop"));
        assert!(s.remove("DROP"));
        assert!(!s.remove("missing"));
        assert!(s.find("Drop").is_none());
        assert!(s.find("Keep").is_some());
    }

    #[test]
    fn saved_view_mode_decodes_its_slug() {
        let v = SavedView {
            name: "x".into(),
            view_mode: "topology".into(),
            zone_tab: "dev".into(),
            filter: String::new(),
        };
        assert_eq!(v.mode(), ViewMode::Topology);
    }

    #[test]
    fn saved_views_serde_round_trips() {
        let mut s = SavedViews::default();
        s.upsert(a_view("Prod VMs"));
        s.upsert(SavedView {
            name: "Dev Topology".into(),
            view_mode: ViewMode::Topology.slug().into(),
            zone_tab: "dev".into(),
            filter: String::new(),
        });
        let json = serde_json::to_string(&s).unwrap();
        let back: SavedViews = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn load_saved_views_treats_a_missing_or_corrupt_file_as_empty() {
        let _guard = lock_env();
        let _cfg = IsolatedConfig::new();
        // No file yet (NotFound) → Ok(empty), never an error.
        assert_eq!(load_saved_views(), Ok(SavedViews::default()));
        // A corrupt (non-JSON) body → Ok(empty) too: "no saved views", not an
        // error that would block the panel.
        let path = saved_views_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not json{").unwrap();
        assert_eq!(load_saved_views(), Ok(SavedViews::default()));
    }

    #[test]
    fn save_then_load_round_trips_through_the_config_file() {
        let _guard = lock_env();
        // Isolate the config dir to a tempdir so the round-trip touches a real
        // file without clobbering the operator's.
        let _cfg = IsolatedConfig::new();

        let mut s = SavedViews::default();
        s.upsert(a_view("Prod VMs"));
        save_saved_views(&s).expect("write saved views");
        // The file lands at <cfg>/mde/datacenter-views.json — round-trips back.
        assert!(saved_views_path().unwrap().exists());
        let loaded = load_saved_views().expect("read back");
        assert_eq!(loaded, s);
    }

    #[test]
    fn save_current_view_message_captures_the_active_view() {
        let _guard = lock_env();
        // Isolated empty config so `new()` starts with no saved views and the
        // save writes to the tempdir, not the operator's file.
        let _cfg = IsolatedConfig::new();
        let mut p = DatacenterPanel::new();
        // The constructor is now pure (no disk read); the first load hydrates the
        // (empty, isolated) file + marks the panel loaded so saves persist.
        assert!(p.saved_views.is_empty(), "constructor starts empty");
        let _ = p.update(Message::SavedViewsLoaded(load_saved_views()));
        assert!(p.views_loaded, "first load marks the panel hydrated");
        // Start from a known view state.
        let _ = p.update(Message::ViewMode(ViewMode::Zone));
        let _ = p.update(Message::ZoneTab("dev".to_string()));
        let _ = p.update(Message::FilterChanged("builder".to_string()));
        let _ = p.update(Message::SaveViewNameChanged("My Builders".to_string()));
        let _ = p.update(Message::SaveCurrentView);
        assert_eq!(p.saved_views.views.len(), 1);
        let v = p.saved_views.find("My Builders").expect("view saved");
        assert_eq!(v.view_mode, ViewMode::Zone.slug());
        assert_eq!(v.zone_tab, "dev");
        assert_eq!(v.filter, "builder");
        // The name box is cleared after a successful save.
        assert!(p.save_view_name.is_empty());
        // The save persisted to disk: a fresh load sees the same view.
        let reloaded = load_saved_views().expect("read back");
        assert!(reloaded.find("My Builders").is_some());
    }

    #[test]
    fn a_real_read_error_blocks_persistence_so_no_overwrite() {
        // The code-review data-loss path: when the saved-views file exists but
        // couldn't be read (a real error, surfaced as `!views_loaded`), a save
        // must NOT write — overwriting the on-disk file would lose the operator's
        // views. Here `views_loaded` stays false (no successful load), so
        // `persist_saved_views` refuses and only the status line changes.
        let _guard = lock_env();
        let _cfg = IsolatedConfig::new();
        // Pre-seed an on-disk file with two views that a save must not clobber.
        let mut on_disk = SavedViews::default();
        on_disk.upsert(a_view("Keep A"));
        on_disk.upsert(a_view("Keep B"));
        save_saved_views(&on_disk).unwrap();

        let mut p = DatacenterPanel::new();
        // Simulate the load failing (the Err arm) — views_loaded stays false.
        let _ = p.update(Message::SavedViewsLoaded(Err("permission denied".into())));
        assert!(
            !p.views_loaded,
            "a failed load leaves the panel un-hydrated"
        );
        // Now a save attempt must not write.
        let _ = p.update(Message::SaveViewNameChanged("New One".to_string()));
        let _ = p.update(Message::SaveCurrentView);
        // The on-disk file is untouched — both originals survive.
        let still = load_saved_views().expect("read back");
        assert!(still.find("Keep A").is_some());
        assert!(still.find("Keep B").is_some());
        assert!(
            still.find("New One").is_none(),
            "the un-hydrated save did not write"
        );
    }

    #[test]
    fn apply_view_message_restores_mode_zone_and_filter() {
        let mut p = DatacenterPanel::new();
        p.saved_views.upsert(SavedView {
            name: "Dev Topology".into(),
            view_mode: ViewMode::Topology.slug().into(),
            zone_tab: "dev".into(),
            filter: "xen".into(),
        });
        // Move away from that view, then restore it.
        let _ = p.update(Message::ViewMode(ViewMode::Overview));
        let _ = p.update(Message::ZoneTab("prod".to_string()));
        let _ = p.update(Message::FilterChanged(String::new()));
        let _ = p.update(Message::ApplyView("Dev Topology".to_string()));
        assert_eq!(p.view_mode, ViewMode::Topology);
        assert_eq!(p.zone_tab, "dev");
        assert_eq!(p.filter, "xen");
    }

    #[test]
    fn save_view_name_changed_updates_the_box() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::SaveViewNameChanged("Prod".to_string()));
        assert_eq!(p.save_view_name, "Prod");
    }

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
        // inline confirm/cancel render branch in dc_card_view.
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
    fn dr_backup_requires_typed_confirm_before_firing() {
        let mut p = DatacenterPanel::new();
        assert!(!p.dr_confirm);
        // First click only arms the typed-confirm — it must NOT fire the RPC.
        let _ = p.update(Message::DrBackupClicked);
        assert!(p.dr_confirm);
        assert_eq!(p.dr_status, "Confirm backup below.");
        // Only the explicit confirm clears the pending state + moves the status
        // to the in-flight "Backing up…" (the RPC then fires).
        let _ = p.update(Message::DrBackup);
        assert!(!p.dr_confirm);
        assert_eq!(p.dr_status, "Backing up…");
    }

    #[test]
    fn dr_backup_cancel_clears_the_pending_confirm() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::DrBackupClicked);
        assert!(p.dr_confirm);
        let _ = p.update(Message::DrBackupCancelled);
        assert!(!p.dr_confirm);
        assert!(p.dr_status.is_empty());
    }

    #[test]
    fn dr_backup_done_writes_status() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::DrBackupDone(Ok(
            "/var/backups/dc/2026-06-22.tar".to_string()
        )));
        assert_eq!(p.dr_status, "backed up: /var/backups/dc/2026-06-22.tar");
        let _ = p.update(Message::DrBackupDone(Err("backup failed".to_string())));
        assert_eq!(p.dr_status, "backup failed");
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
            promote: Vec::new(),
            health: Vec::new(),
            jobs: Vec::new(),
            power: Vec::new(),
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

    // ---- DATACENTER-13: Topology view (host-grouped, collapsible) ----------

    /// A representative cross-zone row set: two Dev hosts (one with a VM + SR +
    /// net child, one childless), plus a Prod droplet and a gateway-ish orphan.
    fn topology_fixture() -> Vec<DcRow> {
        project_rows(&[
            (
                "event/dc/host/a".into(),
                r#"{"kind":"host","id":"172.20.0.9","name":"dom0-a","status":"up","zone":"dev","cpu":"8","mem_total_mb":"16000","mem_free_mb":"9000","load":"0.4"}"#.into(),
            ),
            (
                "event/dc/host/b".into(),
                r#"{"kind":"host","id":"172.20.145.193","name":"dom0-b","status":"up","zone":"dev","cpu":"16","mem_total_mb":"32000","mem_free_mb":"20000","load":"1.0"}"#.into(),
            ),
            (
                "event/dc/vm/9".into(),
                r#"{"kind":"vm","id":"9","name":"builder","status":"running","zone":"dev","host":"172.20.0.9"}"#.into(),
            ),
            (
                "event/dc/sr/1".into(),
                r#"{"kind":"sr","id":"sr-1","name":"local-ext","size":"222330230784","used":"42949672960","host":"172.20.0.9","zone":"dev"}"#.into(),
            ),
            (
                "event/dc/net/0".into(),
                r#"{"kind":"net","id":"net-0","name":"Pool-wide network","status":"up","zone":"dev","bridge":"xenbr0","host":"172.20.0.9"}"#.into(),
            ),
            (
                "event/dc/droplet/2".into(),
                r#"{"kind":"droplet","id":"2","name":"lighthouse-01","status":"active","zone":"prod"}"#.into(),
            ),
            (
                "event/dc/gw/gw0".into(),
                r#"{"kind":"gateway","id":"gw0","name":"nebula-gw","status":"up","zone":"prod"}"#.into(),
            ),
        ])
    }

    #[test]
    fn group_by_host_nests_children_under_their_host() {
        let groups = group_by_host(&topology_fixture());
        // Two host groups (id-sorted) + one synthetic Prod/Gateway group.
        assert_eq!(groups.len(), 3);
        // Host groups come first, in `id` order: "172.20.0.9" < "172.20.145.193".
        assert_eq!(groups[0].0.kind, "host");
        assert_eq!(groups[0].0.id, "172.20.0.9");
        // Its three children: vm, sr, net (all carry host == that dom0).
        assert_eq!(groups[0].1.len(), 3);
        assert!(groups[0].1.iter().all(|c| c.host == "172.20.0.9"));
        assert!(groups[0].1.iter().any(|c| c.kind == "vm"));
        assert!(groups[0].1.iter().any(|c| c.kind == "sr"));
        assert!(groups[0].1.iter().any(|c| c.kind == "net"));
        // Second host is childless.
        assert_eq!(groups[1].0.id, "172.20.145.193");
        assert!(groups[1].1.is_empty());
        // Trailing synthetic group: empty kind/id sentinel, holds the orphans
        // (the Prod droplet + the gateway).
        let (synth, orphans) = &groups[2];
        assert_eq!(synth.kind, "");
        assert_eq!(synth.id, "");
        assert_eq!(orphans.len(), 2);
        assert!(orphans.iter().any(|c| c.kind == "droplet"));
        assert!(orphans.iter().any(|c| c.kind == "gateway"));
    }

    #[test]
    fn group_by_host_orphan_with_unknown_host_lands_in_synthetic_group() {
        // A vm naming a host that doesn't exist must not vanish — it falls into
        // the synthetic group rather than being dropped.
        let rows = project_rows(&[(
            "event/dc/vm/x".into(),
            r#"{"kind":"vm","id":"x","name":"stray","status":"running","zone":"dev","host":"10.0.0.99"}"#.into(),
        )]);
        let groups = group_by_host(&rows);
        assert_eq!(groups.len(), 1); // no host header, just the synthetic group
        assert_eq!(groups[0].0.kind, "");
        assert_eq!(groups[0].1.len(), 1);
        assert_eq!(groups[0].1[0].id, "x");
    }

    #[test]
    fn group_by_host_empty_is_empty() {
        assert!(group_by_host(&[]).is_empty());
    }

    #[test]
    fn topology_view_mode_seeds_all_groups_expanded() {
        let mut p = DatacenterPanel::new();
        p.rows = topology_fixture();
        let _ = p.update(Message::ViewMode(ViewMode::Topology));
        assert_eq!(p.view_mode, ViewMode::Topology);
        // Every group (both host ids + the synthetic "" key) starts expanded.
        assert!(p.expanded.contains("172.20.0.9"));
        assert!(p.expanded.contains("172.20.145.193"));
        assert!(p.expanded.contains("")); // synthetic Prod/Gateway group
    }

    #[test]
    fn topology_view_renders_open_and_collapsed() {
        let mut p = DatacenterPanel::new();
        p.rows = topology_fixture();
        let _ = p.update(Message::ViewMode(ViewMode::Topology));
        // Fully expanded render — exercises header + nested child rows.
        let _ = p.view();
        // Collapse the first host group, then render the collapsed branch.
        let _ = p.update(Message::HeaderClicked("172.20.0.9".to_string()));
        assert!(!p.expanded.contains("172.20.0.9"));
        let _ = p.view();
    }

    #[test]
    fn topology_view_renders_empty_state() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::ViewMode(ViewMode::Topology));
        // No rows → group_by_host empty → the empty-state copy renders.
        let _ = p.view();
    }

    #[test]
    fn header_clicked_toggles_expanded_membership() {
        let mut p = DatacenterPanel::new();
        // Toggle on, then off — membership tracks expanded state.
        let _ = p.update(Message::HeaderClicked("172.20.0.9".to_string()));
        assert!(p.expanded.contains("172.20.0.9"));
        let _ = p.update(Message::HeaderClicked("172.20.0.9".to_string()));
        assert!(!p.expanded.contains("172.20.0.9"));
    }

    #[test]
    fn topology_collapse_sticks_across_a_re_render_but_load_re_seeds() {
        let mut p = DatacenterPanel::new();
        p.rows = topology_fixture();
        let _ = p.update(Message::ViewMode(ViewMode::Topology));
        // Manual collapse of a host group sticks (guard stays set).
        let _ = p.update(Message::HeaderClicked("172.20.0.9".to_string()));
        assert!(!p.expanded.contains("172.20.0.9"));
        p.ensure_topology_seeded(); // a re-render: already seeded → no-op
        assert!(!p.expanded.contains("172.20.0.9"));
        // A fresh Loaded re-seeds: the collapsed group re-opens.
        let load = DcLoad {
            rows: topology_fixture(),
            audit: Vec::new(),
            promote: Vec::new(),
            health: Vec::new(),
            jobs: Vec::new(),
            power: Vec::new(),
        };
        let _ = p.update(Message::Loaded(Ok(load)));
        assert!(p.expanded.contains("172.20.0.9"));
    }

    #[test]
    fn topology_view_mode_is_selectable() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::ViewMode(ViewMode::Topology));
        assert_eq!(p.view_mode, ViewMode::Topology);
    }

    // ---- DATACENTER-15: Tofu state browser + drift badge -------------------

    #[test]
    fn tofu_state_clicked_sets_an_in_flight_status() {
        let mut p = DatacenterPanel::new();
        // First click records the workspace + sets the in-flight status (the
        // real `action/dc/tofu-state` RPC then fires on the blocking thread).
        let _ = p.update(Message::TofuStateClicked("xen-xapi".to_string()));
        assert_eq!(p.status, "Reading xen-xapi state…");
        assert_eq!(p.tofu_state_ws, "xen-xapi");
    }

    #[test]
    fn tofu_state_done_populates_resources_and_drift() {
        let mut p = DatacenterPanel::new();
        // A drift reply: the resource list + drift flag land on the panel.
        let _ = p.update(Message::TofuStateDone(Ok((
            vec![
                "xenorchestra_vm.builder".to_string(),
                "xenorchestra_network.lan".to_string(),
            ],
            true,
        ))));
        assert_eq!(p.status, "State read complete");
        assert_eq!(p.tofu_state_resources.len(), 2);
        assert_eq!(p.tofu_state_resources[0], "xenorchestra_vm.builder");
        assert!(p.tofu_state_drift);
        // A subsequent in-sync reply clears the drift flag.
        let _ = p.update(Message::TofuStateDone(Ok((
            vec!["digitalocean_droplet.lighthouse".to_string()],
            false,
        ))));
        assert_eq!(p.tofu_state_resources.len(), 1);
        assert!(!p.tofu_state_drift);
        // An error reply surfaces to the status without touching the list.
        let _ = p.update(Message::TofuStateDone(Err("tofu state failed".to_string())));
        assert_eq!(p.status, "tofu state failed");
    }

    #[test]
    fn tofu_view_renders_the_managed_state_list_and_drift_badge() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::ViewMode(ViewMode::Tofu));
        // No state read yet → the state browser block is skipped.
        let _ = p.view();
        // Drift reply with resources — exercises the drift badge + resource list
        // render branch.
        let _ = p.update(Message::TofuStateClicked("xen-xapi".to_string()));
        let _ = p.update(Message::TofuStateDone(Ok((
            vec![
                "xenorchestra_vm.builder".to_string(),
                "xenorchestra_network.lan".to_string(),
            ],
            true,
        ))));
        let _ = p.view();
        // In-sync reply with an empty list — exercises the ✓ in-sync badge + the
        // "no managed resources" branch.
        let _ = p.update(Message::TofuStateDone(Ok((Vec::new(), false))));
        let _ = p.view();
    }

    // ---- DATACENTER-20/9: Build → Eagle → DO promotion strip ---------------

    #[test]
    fn parse_promote_event_reads_a_stage() {
        let s = parse_promote_event(r#"{"stage":"eagle","version":"11.0.1","status":"ready"}"#)
            .unwrap();
        assert_eq!(s.stage, "eagle");
        assert_eq!(s.version, "11.0.1");
        assert_eq!(s.status, "ready");
    }

    #[test]
    fn parse_promote_event_defaults_missing_fields_and_drops_garbage() {
        // Missing version/status default to empty.
        let s = parse_promote_event(r#"{"stage":"do"}"#).unwrap();
        assert_eq!(s.stage, "do");
        assert_eq!(s.version, "");
        assert_eq!(s.status, "");
        // Unparseable / missing stage → None.
        assert!(parse_promote_event("not json").is_none());
        assert!(parse_promote_event(r#"{"version":"11.0.1"}"#).is_none());
    }

    #[test]
    fn project_promote_filters_to_promote_topics() {
        let events = vec![
            // Not a promote topic → dropped.
            (
                "event/dc/vm/9".into(),
                r#"{"kind":"vm","id":"9","name":"b","status":"running","zone":"dev"}"#.into(),
            ),
            (
                "event/dc/promote/build".into(),
                r#"{"stage":"build","version":"11.0.2","status":"ready"}"#.into(),
            ),
            (
                "event/dc/promote/do".into(),
                r#"{"stage":"do","version":"11.0.1","status":"pending"}"#.into(),
            ),
        ];
        let stages = project_promote(&events);
        assert_eq!(stages.len(), 2); // non-promote dropped
        assert!(stages.iter().any(|s| s.stage == "build"));
        assert!(stages.iter().any(|s| s.stage == "do"));
    }

    #[test]
    fn promote_event_is_dropped_by_project_rows() {
        // A promote body has no `kind`/`id`, so it must NOT leak into resource
        // rows even though its topic starts with `event/dc/`.
        let rows = project_rows(&[(
            "event/dc/promote/build".into(),
            r#"{"stage":"build","version":"11.0.2","status":"ready"}"#.into(),
        )]);
        assert!(rows.is_empty());
    }

    #[test]
    fn promote_matrix_orders_build_eagle_do() {
        // Supplied out of order — the matrix returns canonical build→eagle→do.
        let stages = vec![
            PromoteStage {
                stage: "do".into(),
                version: "11.0.0".into(),
                status: "ready".into(),
            },
            PromoteStage {
                stage: "build".into(),
                version: "11.0.2".into(),
                status: "ready".into(),
            },
            PromoteStage {
                stage: "eagle".into(),
                version: "11.0.1".into(),
                status: "pending".into(),
            },
        ];
        let m = promote_matrix(&stages);
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].stage, "build");
        assert_eq!(m[0].version, "11.0.2");
        assert_eq!(m[1].stage, "eagle");
        assert_eq!(m[1].version, "11.0.1");
        assert_eq!(m[2].stage, "do");
        assert_eq!(m[2].version, "11.0.0");
    }

    #[test]
    fn promote_matrix_fills_absent_stages_with_placeholder() {
        // Only "build" present → eagle + do are filled with the "—"/"unknown"
        // placeholder, still in canonical order.
        let stages = vec![PromoteStage {
            stage: "build".into(),
            version: "11.0.2".into(),
            status: "ready".into(),
        }];
        let m = promote_matrix(&stages);
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].stage, "build");
        assert_eq!(m[0].version, "11.0.2");
        // Absent eagle + do → placeholder.
        assert_eq!(m[1].stage, "eagle");
        assert_eq!(m[1].version, "—");
        assert_eq!(m[1].status, "unknown");
        assert_eq!(m[2].stage, "do");
        assert_eq!(m[2].version, "—");
        assert_eq!(m[2].status, "unknown");
    }

    #[test]
    fn promote_matrix_empty_is_all_placeholders() {
        let m = promote_matrix(&[]);
        assert_eq!(m.len(), 3);
        assert!(m.iter().all(|s| s.version == "—" && s.status == "unknown"));
        assert_eq!(m[0].stage, "build");
        assert_eq!(m[1].stage, "eagle");
        assert_eq!(m[2].stage, "do");
    }

    #[test]
    fn loaded_populates_promote_stages() {
        let mut p = DatacenterPanel::new();
        let load = DcLoad {
            rows: Vec::new(),
            audit: Vec::new(),
            promote: vec![PromoteStage {
                stage: "build".into(),
                version: "11.0.2".into(),
                status: "ready".into(),
            }],
            health: Vec::new(),
            jobs: Vec::new(),
            power: Vec::new(),
        };
        let _ = p.update(Message::Loaded(Ok(load)));
        assert_eq!(p.promote.len(), 1);
        assert_eq!(p.promote[0].stage, "build");
    }

    #[test]
    fn overview_renders_the_promotion_strip() {
        let mut p = DatacenterPanel::new();
        // Default view is Overview. Populate a partial promote set so the strip
        // renders both real cards (ready/pending chips) and a "—" placeholder.
        p.promote = project_promote(&[
            (
                "event/dc/promote/build".into(),
                r#"{"stage":"build","version":"11.0.2","status":"ready"}"#.into(),
            ),
            (
                "event/dc/promote/eagle".into(),
                r#"{"stage":"eagle","version":"11.0.1","status":"pending"}"#.into(),
            ),
        ]);
        // Exercises promote_strip_view → promote_card_view for ready, pending,
        // and the absent "do" placeholder branch.
        let _ = p.view();
        // And it stays reachable with no promote events at all (all placeholders).
        let empty = DatacenterPanel::new();
        let _ = empty.view();
    }

    // ---- DATACENTER-24: Health summary + alerts on Overview ----------------

    #[test]
    fn parse_health_event_reads_a_check() {
        let c = parse_health_event(r#"{"check":"dom0-a","status":"fail","detail":"ssh timeout"}"#)
            .unwrap();
        assert_eq!(c.check, "dom0-a");
        assert_eq!(c.status, "fail");
        assert_eq!(c.detail, "ssh timeout");
    }

    #[test]
    fn parse_health_event_defaults_missing_fields_and_drops_garbage() {
        // Missing status/detail default to empty.
        let c = parse_health_event(r#"{"check":"bus"}"#).unwrap();
        assert_eq!(c.check, "bus");
        assert_eq!(c.status, "");
        assert_eq!(c.detail, "");
        // Unparseable / missing check → None.
        assert!(parse_health_event("not json").is_none());
        assert!(parse_health_event(r#"{"status":"ok"}"#).is_none());
    }

    #[test]
    fn project_health_filters_to_health_topics_and_sorts() {
        let events = vec![
            // Not a health topic → dropped.
            (
                "event/dc/vm/9".into(),
                r#"{"kind":"vm","id":"9","name":"b","status":"running","zone":"dev"}"#.into(),
            ),
            (
                "event/dc/health/dom0-a".into(),
                r#"{"check":"dom0-a","status":"ok"}"#.into(),
            ),
            (
                "event/dc/health/bus".into(),
                r#"{"check":"bus","status":"warn","detail":"lagging"}"#.into(),
            ),
        ];
        let checks = project_health(&events);
        assert_eq!(checks.len(), 2); // non-health dropped
                                     // Sorted by check name: "bus" < "dom0-a".
        assert_eq!(checks[0].check, "bus");
        assert_eq!(checks[1].check, "dom0-a");
    }

    #[test]
    fn health_event_is_dropped_by_project_rows() {
        // A health body has no `kind`/`id`, so it must NOT leak into resource
        // rows even though its topic starts with `event/dc/`.
        let rows = project_rows(&[(
            "event/dc/health/bus".into(),
            r#"{"check":"bus","status":"ok"}"#.into(),
        )]);
        assert!(rows.is_empty());
    }

    #[test]
    fn health_summary_counts_ok_warn_fail() {
        let checks = vec![
            HealthCheck {
                check: "bus".into(),
                status: "ok".into(),
                detail: String::new(),
            },
            HealthCheck {
                check: "doctl".into(),
                status: "ok".into(),
                detail: String::new(),
            },
            HealthCheck {
                check: "dom0-a".into(),
                status: "warn".into(),
                detail: "load high".into(),
            },
            HealthCheck {
                check: "dom0-b".into(),
                status: "fail".into(),
                detail: "ssh timeout".into(),
            },
            // An unknown/empty status is fail-safe → counts as a failure.
            HealthCheck {
                check: "mystery".into(),
                status: String::new(),
                detail: String::new(),
            },
        ];
        assert_eq!(health_summary(&checks), (2, 1, 2));
        // Empty input → all zeroes.
        assert_eq!(health_summary(&[]), (0, 0, 0));
    }

    #[test]
    fn loaded_populates_health_checks() {
        let mut p = DatacenterPanel::new();
        let load = DcLoad {
            rows: Vec::new(),
            audit: Vec::new(),
            promote: Vec::new(),
            health: vec![HealthCheck {
                check: "bus".into(),
                status: "ok".into(),
                detail: String::new(),
            }],
            jobs: Vec::new(),
            power: Vec::new(),
        };
        let _ = p.update(Message::Loaded(Ok(load)));
        assert_eq!(p.health.len(), 1);
        assert_eq!(p.health[0].check, "bus");
    }

    #[test]
    fn panel_defaults_have_no_health_checks() {
        let p = DatacenterPanel::new();
        assert!(p.health.is_empty());
    }

    #[test]
    fn overview_renders_the_health_section_all_ok() {
        let mut p = DatacenterPanel::new();
        // Default view is Overview. All-ok checks → the "✓ all systems healthy"
        // branch renders (no alert rows).
        p.health = project_health(&[
            (
                "event/dc/health/bus".into(),
                r#"{"check":"bus","status":"ok"}"#.into(),
            ),
            (
                "event/dc/health/doctl".into(),
                r#"{"check":"doctl","status":"ok"}"#.into(),
            ),
        ]);
        let _ = p.view();
        // And it stays reachable with no health checks at all (empty-state hint).
        let empty = DatacenterPanel::new();
        let _ = empty.view();
    }

    #[test]
    fn overview_renders_the_health_section_with_failures() {
        let mut p = DatacenterPanel::new();
        // A mixed set → the summary tally + the warn/fail alert rows render,
        // incl. the empty-detail fallback (status used in place of detail).
        p.health = project_health(&[
            (
                "event/dc/health/bus".into(),
                r#"{"check":"bus","status":"ok"}"#.into(),
            ),
            (
                "event/dc/health/dom0-a".into(),
                r#"{"check":"dom0-a","status":"warn","detail":"load high"}"#.into(),
            ),
            (
                "event/dc/health/dom0-b".into(),
                r#"{"check":"dom0-b","status":"fail"}"#.into(),
            ),
        ]);
        let _ = p.view();
    }

    // ---- DATACENTER-9/15: Recent Tofu runs on Overview ---------------------

    #[test]
    fn parse_job_event_reads_a_job() {
        let j = parse_job_event(
            r#"{"action":"dc/tofu-apply","ulid":"01J0000000000000000000APPLY","status":"ok"}"#,
        )
        .unwrap();
        assert_eq!(j.action, "dc/tofu-apply");
        assert_eq!(j.ulid, "01J0000000000000000000APPLY");
        assert_eq!(j.status, "ok");
    }

    #[test]
    fn parse_job_event_defaults_missing_fields_and_drops_garbage() {
        // Missing ulid/status default to empty.
        let j = parse_job_event(r#"{"action":"dc/vm-power"}"#).unwrap();
        assert_eq!(j.action, "dc/vm-power");
        assert_eq!(j.ulid, "");
        assert_eq!(j.status, "");
        // Unparseable / missing action → None.
        assert!(parse_job_event("not json").is_none());
        assert!(parse_job_event(r#"{"ulid":"01J","status":"ok"}"#).is_none());
    }

    #[test]
    fn project_jobs_filters_to_job_topics() {
        let events = vec![
            // Not a job topic → dropped.
            (
                "event/dc/vm/9".into(),
                r#"{"kind":"vm","id":"9","name":"b","status":"running","zone":"dev"}"#.into(),
            ),
            (
                "event/dc/job/01J0001".into(),
                r#"{"action":"dc/tofu-plan","ulid":"01J0001","status":"ok"}"#.into(),
            ),
        ];
        let jobs = project_jobs(&events);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].action, "dc/tofu-plan");
    }

    #[test]
    fn job_event_is_dropped_by_project_rows() {
        // A job body has no `kind`/`id`, so it must NOT leak into resource rows
        // even though its topic starts with `event/dc/`.
        let rows = project_rows(&[(
            "event/dc/job/01J0001".into(),
            r#"{"action":"dc/tofu-plan","ulid":"01J0001","status":"ok"}"#.into(),
        )]);
        assert!(rows.is_empty());
    }

    #[test]
    fn recent_tofu_runs_filters_to_tofu_verbs() {
        let jobs = vec![
            JobRow {
                action: "dc/tofu-apply".into(),
                ulid: "01J0002".into(),
                status: "ok".into(),
            },
            JobRow {
                action: "dc/vm-power".into(),
                ulid: "01J0003".into(),
                status: "ok".into(),
            },
            JobRow {
                action: "dc/tofu-plan".into(),
                ulid: "01J0001".into(),
                status: "ok".into(),
            },
        ];
        let runs = recent_tofu_runs(&jobs);
        // vm-power filtered out; only the two tofu verbs survive.
        assert_eq!(runs.len(), 2);
        assert!(runs.iter().all(|r| r.action.contains("tofu")));
    }

    #[test]
    fn recent_tofu_runs_orders_newest_first_by_ulid() {
        let jobs = vec![
            JobRow {
                action: "dc/tofu-plan".into(),
                ulid: "01J0001".into(),
                status: "ok".into(),
            },
            JobRow {
                action: "dc/tofu-apply".into(),
                ulid: "01J0003".into(),
                status: "ok".into(),
            },
            JobRow {
                action: "dc/tofu-state".into(),
                ulid: "01J0002".into(),
                status: "ok".into(),
            },
        ];
        let runs = recent_tofu_runs(&jobs);
        // Descending ULID = newest first.
        assert_eq!(runs[0].ulid, "01J0003");
        assert_eq!(runs[1].ulid, "01J0002");
        assert_eq!(runs[2].ulid, "01J0001");
    }

    #[test]
    fn recent_tofu_runs_caps_at_eight() {
        let jobs: Vec<JobRow> = (0..20)
            .map(|i| JobRow {
                action: "dc/tofu-plan".into(),
                // Zero-padded so lexical order matches numeric order.
                ulid: format!("01J{i:05}"),
                status: "ok".into(),
            })
            .collect();
        let runs = recent_tofu_runs(&jobs);
        assert_eq!(runs.len(), RECENT_TOFU_CAP);
        // The cap keeps the newest (highest ulid) ones.
        assert_eq!(runs[0].ulid, "01J00019");
        assert_eq!(runs[RECENT_TOFU_CAP - 1].ulid, "01J00012");
    }

    #[test]
    fn loaded_populates_jobs() {
        let mut p = DatacenterPanel::new();
        let load = DcLoad {
            rows: Vec::new(),
            audit: Vec::new(),
            promote: Vec::new(),
            health: Vec::new(),
            jobs: vec![JobRow {
                action: "dc/tofu-apply".into(),
                ulid: "01J0001".into(),
                status: "ok".into(),
            }],
            power: Vec::new(),
        };
        let _ = p.update(Message::Loaded(Ok(load)));
        assert_eq!(p.jobs.len(), 1);
        assert_eq!(p.jobs[0].action, "dc/tofu-apply");
    }

    #[test]
    fn panel_defaults_have_no_jobs() {
        let p = DatacenterPanel::new();
        assert!(p.jobs.is_empty());
    }

    #[test]
    fn overview_renders_the_recent_tofu_runs_with_runs() {
        let mut p = DatacenterPanel::new();
        // Default view is Overview. A mixed set exercises the ok / error /
        // pending chip branches + the `dc/tofu-` prefix strip, and confirms a
        // non-tofu job is filtered out of the rendered run-log.
        p.jobs = project_jobs(&[
            (
                "event/dc/job/01J0003".into(),
                r#"{"action":"dc/tofu-apply","ulid":"01J0003","status":"ok"}"#.into(),
            ),
            (
                "event/dc/job/01J0002".into(),
                r#"{"action":"dc/tofu-destroy","ulid":"01J0002","status":"error"}"#.into(),
            ),
            (
                "event/dc/job/01J0001".into(),
                r#"{"action":"dc/tofu-plan","ulid":"01J0001","status":"pending"}"#.into(),
            ),
            (
                "event/dc/job/01J0000".into(),
                r#"{"action":"dc/vm-power","ulid":"01J0000","status":"ok"}"#.into(),
            ),
        ]);
        let _ = p.view();
    }

    #[test]
    fn overview_renders_the_recent_tofu_runs_empty_state() {
        // No jobs at all → the "no recent Tofu runs" empty-state line renders.
        let p = DatacenterPanel::new();
        let _ = p.view();
    }

    fn row_with(kind: &str, id: &str, name: &str, status: &str, zone: &str) -> DcRow {
        DcRow {
            kind: kind.into(),
            id: id.into(),
            name: name.into(),
            status: status.into(),
            zone: zone.into(),
            host: String::new(),
            size: String::new(),
            used: String::new(),
            bridge: String::new(),
            cpu: String::new(),
            mem_total_mb: String::new(),
            mem_free_mb: String::new(),
            load: String::new(),
        }
    }

    #[test]
    fn status_dot_maps_liveness_onto_semantic_tokens() {
        let p = Palette::dark();
        // Up vocabularies (DO "active" / Xen "running" / "up") → success.
        assert_eq!(row_with("vm", "1", "a", "running", "dev").status_dot(p), p.success);
        assert_eq!(
            row_with("droplet", "2", "b", "active", "prod").status_dot(p),
            p.success
        );
        // Off vocabularies → danger.
        assert_eq!(row_with("vm", "3", "c", "halted", "dev").status_dot(p), p.danger);
        assert_eq!(row_with("droplet", "4", "d", "off", "prod").status_dot(p), p.danger);
        // Transitional → warning.
        assert_eq!(
            row_with("vm", "5", "e", "rebooting", "dev").status_dot(p),
            p.warning
        );
        // Case-insensitive.
        assert_eq!(row_with("vm", "6", "f", "RUNNING", "dev").status_dot(p), p.success);
        // Unknown / empty → muted (never a misleading green/red).
        assert_eq!(row_with("vm", "7", "g", "", "dev").status_dot(p), p.text_muted);
        assert_eq!(
            row_with("vm", "8", "h", "weird", "dev").status_dot(p),
            p.text_muted
        );
    }

    #[test]
    fn matches_filter_is_case_insensitive_over_name_id_kind() {
        let r = row_with("droplet", "579112110", "lighthouse-01", "active", "prod");
        // Empty / whitespace needle matches everything.
        assert!(r.matches_filter(""));
        assert!(r.matches_filter("   "));
        // Name substring, case-insensitive.
        assert!(r.matches_filter("LIGHTHOUSE"));
        assert!(r.matches_filter("house-01"));
        // Id substring.
        assert!(r.matches_filter("5791"));
        // Kind substring.
        assert!(r.matches_filter("drop"));
        // Non-match.
        assert!(!r.matches_filter("xenbr0"));
    }

    #[test]
    fn filter_changed_message_narrows_the_zone_grid() {
        let mut p = DatacenterPanel::new();
        p.rows = vec![
            row_with("vm", "v1", "builder", "running", "dev"),
            row_with("vm", "v2", "tester", "halted", "dev"),
        ];
        let _ = p.update(Message::ViewMode(ViewMode::Zone));
        let _ = p.update(Message::ZoneTab("dev".to_string()));
        // A needle that matches only one row updates state and renders without
        // panicking (the grid renders just the matching card + its status dot).
        let _ = p.update(Message::FilterChanged("builder".to_string()));
        assert_eq!(p.filter, "builder");
        let _ = p.view();
        // A needle that matches nothing → the "no resources match" empty state.
        let _ = p.update(Message::FilterChanged("zzz-none".to_string()));
        let _ = p.view();
    }

    #[test]
    fn card_grid_chunks_into_rows_without_panicking() {
        // More cards than one grid row → exercises the wrap path.
        let palette = Palette::dark();
        let rows: Vec<DcRow> = (0..(CARD_GRID_COLS * 2 + 1))
            .map(|i| row_with("vm", &format!("v{i}"), &format!("vm{i}"), "running", "dev"))
            .collect();
        let selection = Animator::new();
        let now = Instant::now();
        let cards: Vec<Element<'_, crate::Message>> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let motion = CardMotion {
                    index: i,
                    reveal_start: Some(now),
                    selected: false,
                    selection: &selection,
                    hovered: false,
                    hover_since: now,
                    now,
                    reduce_motion: false,
                };
                dc_card_view(r, palette, false, None, motion)
            })
            .collect();
        // Building the grid must not panic for a short final row.
        let _ = card_grid(cards);
    }

    // ── MOTION-FEEDBACK-2 ─────────────────────────────────────────────────────

    #[test]
    fn reveal_stagger_is_capped_at_eight_slots() {
        // MOTION-FEEDBACK-2 acceptance: the staggered reveal caps at ≤8 — cards
        // past the cap share the cap's delay slot, so the reveal finishes in a
        // bounded time regardless of resource count.
        let start = Instant::now();
        // Card 0 starts at the origin; each subsequent (capped) card adds one step.
        assert_eq!(reveal_card_start(start, 0), start);
        assert_eq!(
            reveal_card_start(start, REVEAL_STAGGER_CAP),
            start + REVEAL_STAGGER_STEP * REVEAL_STAGGER_CAP as u32
        );
        // Past the cap, the delay does NOT keep growing — card 8, 20, 200 all share
        // the cap slot's start.
        let capped = reveal_card_start(start, REVEAL_STAGGER_CAP);
        assert_eq!(reveal_card_start(start, REVEAL_STAGGER_CAP + 12), capped);
        assert_eq!(reveal_card_start(start, 200), capped);
    }

    #[test]
    fn reveal_completes_after_the_last_visible_card_settles() {
        // The reveal is "in flight" until the LAST VISIBLE card's slide-in (its
        // delay plus the mount duration) has elapsed; after that the tick loop
        // retires it. Keying off the real last card (not the fixed cap slot) means a
        // small grid stops the instant its cards settle.
        let start = Instant::now();
        let dur = Motion::panel_mount().duration;
        // A full (≥ cap) grid: the last card is the cap slot.
        let big = REVEAL_STAGGER_CAP + 5;
        let last = reveal_card_start(start, REVEAL_STAGGER_CAP);
        assert!(
            !reveal_is_complete(start, start, big, false),
            "fresh reveal is animating"
        );
        assert!(
            !reveal_is_complete(start, last + dur / 2, big, false),
            "still animating mid-mount of the last card"
        );
        assert!(
            reveal_is_complete(start, last + dur, big, false),
            "settled once the last card's mount has elapsed"
        );
        // A small 3-card grid settles at card 2's slot — well before the cap slot,
        // so it does NOT keep ticking for the absent slots 3..=8.
        let small_last = reveal_card_start(start, 2);
        assert!(
            reveal_is_complete(start, small_last + dur, 3, false),
            "a 3-card grid is done at card 2's slot, not the cap slot"
        );
        assert!(
            !reveal_is_complete(start, small_last + dur, big, false),
            "the SAME instant is NOT complete for a full grid (still on later slots)"
        );
        // Zero cards ⇒ nothing to reveal ⇒ immediately complete.
        assert!(reveal_is_complete(start, start, 0, false));
        // Reduce-motion caps the per-card duration to ≤80 ms (matching slide_in), so
        // the reveal settles sooner than the 240 ms full-motion mount.
        let cap = Duration::from_millis(mde_theme::motion::REDUCE_MOTION_CAP_MS);
        assert!(
            reveal_is_complete(start, last + cap, big, true),
            "reduce-motion reveal is done at the ≤80 ms cap"
        );
        assert!(
            !reveal_is_complete(start, last + cap, big, false),
            "full motion still animating at the reduce-motion cap"
        );
    }

    #[test]
    fn hover_lift_keeps_the_tick_chain_alive_until_it_settles() {
        // Regression: motion_in_flight must count the hover-lift tween, or a
        // standalone hover (no reveal/selection) would freeze mid-lift when the tick
        // chain self-stops after one frame.
        let mut p = DatacenterPanel::new();
        p.reveal_start = None;
        let now = Instant::now();
        // Hover just changed ⇒ the lift is mid-tween ⇒ motion is in flight.
        p.hover_since = now;
        assert!(
            p.motion_in_flight(now, false),
            "a fresh hover-lift is in flight under full motion"
        );
        // After the hover tween elapses, it's settled.
        let after = now + Motion::hover().duration + Duration::from_millis(1);
        assert!(
            !p.motion_in_flight(after, false),
            "the hover-lift settles after Motion::hover()"
        );
        // Under reduce-motion the lift is dropped (no movement) ⇒ never in flight.
        assert!(
            !p.motion_in_flight(now, true),
            "no hover-lift to settle under reduce-motion"
        );
    }

    #[test]
    fn card_select_toggles_and_arms_then_settles_the_tick() {
        // Clicking a card selects it + arms the motion tick; clicking the same card
        // again clears the selection. A non-fresh tick is shared, not duplicated.
        let mut p = DatacenterPanel::new();
        assert!(!p.motion_ticking, "rest = no tick chain");
        let _ = p.update(Message::CardSelected("vm-1".into()));
        assert_eq!(p.selected_card.as_deref(), Some("vm-1"));
        assert!(p.motion_ticking, "selecting arms the tick chain");
        // A concurrent hover does not spawn a second chain (idempotent arm).
        let _ = p.update(Message::CardHovered(Some("vm-1".into())));
        assert!(p.motion_ticking);
        // Toggle off.
        let _ = p.update(Message::CardSelected("vm-1".into()));
        assert!(p.selected_card.is_none(), "re-click clears the selection");
        // Once every tween has settled, a tick stops the chain (no idle wakeups).
        p.selection.gc(Instant::now() + Duration::from_secs(1));
        p.reveal_start = None;
        p.hovered_card = None;
        p.hover_since = Instant::now() - Duration::from_secs(1);
        let _ = p.tick_motion();
        assert!(!p.motion_ticking, "settled motion stops the tick chain");
    }

    #[test]
    fn loaded_arms_a_reveal_and_drops_a_stale_selection() {
        // A fresh row set re-reveals the grid (stamps reveal_start + arms the tick)
        // and drops a selection on a resource that's no longer present.
        let mut p = DatacenterPanel::new();
        p.selected_card = Some("gone".into());
        let load = DcLoad {
            rows: vec![row_with("vm", "v0", "vm0", "running", "dev")],
            ..DcLoad::default()
        };
        let _ = p.update(Message::Loaded(Ok(load)));
        assert!(p.reveal_start.is_some(), "a load arms the card-grid reveal");
        assert!(p.motion_ticking, "the reveal arms the tick chain");
        assert!(
            p.selected_card.is_none(),
            "a selection on an absent resource is dropped"
        );
    }

    #[test]
    fn view_renders_with_a_reveal_and_selection_in_flight() {
        // The whole motion path is runtime-reachable through view(): a freshly
        // loaded, selected, hovered grid renders without panicking.
        let mut p = DatacenterPanel::new();
        p.view_mode = ViewMode::Zone;
        p.zone_tab = "dev".into();
        let load = DcLoad {
            rows: (0..10)
                .map(|i| row_with("vm", &format!("v{i}"), &format!("vm{i}"), "running", "dev"))
                .collect(),
            ..DcLoad::default()
        };
        let _ = p.update(Message::Loaded(Ok(load)));
        let _ = p.update(Message::CardSelected("v3".into()));
        let _ = p.update(Message::CardHovered(Some("v5".into())));
        let _ = p.view();
    }

    // ── DATACENTER-9: rolling-history sparklines ──────────────────────────────

    #[test]
    fn sparkline_maps_a_series_onto_block_glyphs() {
        // An ascending ramp climbs the eight glyphs from floor to ceiling.
        let s = sparkline(&[0.0, 1.0, 2.0, 3.0]);
        assert_eq!(s.chars().count(), 4, "one glyph per sample");
        let chars: Vec<char> = s.chars().collect();
        assert_eq!(chars[0], '▁', "the min sample pins to the floor glyph");
        assert_eq!(chars[3], '█', "the max sample pins to the ceiling glyph");
        // Monotone non-decreasing input → monotone non-decreasing glyph heights.
        let heights: Vec<usize> = chars
            .iter()
            .map(|c| SPARK_GLYPHS.iter().position(|g| g == c).unwrap())
            .collect();
        assert!(heights.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn sparkline_handles_empty_and_flat_series() {
        // Empty → empty (the view falls back to a hint).
        assert_eq!(sparkline(&[]), "");
        // A flat series has no slope → a flat mid-height line, not a div-by-zero.
        let flat = sparkline(&[5.0, 5.0, 5.0]);
        assert_eq!(flat.chars().count(), 3);
        let mid = SPARK_GLYPHS[SPARK_GLYPHS.len() / 2];
        assert!(flat.chars().all(|c| c == mid), "flat → all mid-height");
        // A single sample is also "flat" (one mid glyph).
        assert_eq!(sparkline(&[9.0]).chars().count(), 1);
    }

    #[test]
    fn history_sample_captures_resources_running_and_health() {
        let rows = vec![
            row_with("vm", "v1", "a", "running", "dev"),
            row_with("vm", "v2", "b", "halted", "dev"), // not running
            row_with("droplet", "d1", "lh", "active", "prod"), // running compute
            row_with("droplet", "d2", "lh2", "off", "prod"), // not running
        ];
        let health = vec![
            HealthCheck {
                check: "bus".into(),
                status: "ok".into(),
                detail: String::new(),
            },
            HealthCheck {
                check: "dom0".into(),
                status: "fail".into(),
                detail: "ssh".into(),
            },
        ];
        let s = HistorySample::capture(&rows, &health);
        assert_eq!(s.resources, 4);
        assert_eq!(s.running, 2, "one running vm + one active droplet");
        assert_eq!(s.health_ok, 1);
        assert_eq!(s.health_alerts, 1, "the fail counts as an alert");
    }

    #[test]
    fn push_sample_rings_at_the_history_cap() {
        let mut p = DatacenterPanel::new();
        assert!(p.history().is_empty());
        // Overfill the ring buffer by a few samples.
        for i in 0..(HISTORY_CAP + 5) {
            p.push_sample(HistorySample {
                resources: i,
                ..HistorySample::default()
            });
        }
        // Capped — never grows past HISTORY_CAP.
        assert_eq!(p.history().len(), HISTORY_CAP);
        // Oldest evicted: the front is sample #5, the back is the newest.
        assert_eq!(p.history().front().unwrap().resources, 5);
        assert_eq!(p.history().back().unwrap().resources, HISTORY_CAP + 4);
    }

    #[test]
    fn loaded_pushes_a_history_sample() {
        let mut p = DatacenterPanel::new();
        let load = DcLoad {
            rows: vec![row_with("droplet", "d1", "lh", "active", "prod")],
            health: vec![HealthCheck {
                check: "bus".into(),
                status: "ok".into(),
                detail: String::new(),
            }],
            ..DcLoad::default()
        };
        let _ = p.update(Message::Loaded(Ok(load.clone())));
        assert_eq!(p.history().len(), 1, "a load records one sample");
        let _ = p.update(Message::Loaded(Ok(load)));
        assert_eq!(p.history().len(), 2, "each load appends another");
        let s = p.history().back().unwrap();
        assert_eq!(s.resources, 1);
        assert_eq!(s.running, 1);
        assert_eq!(s.health_ok, 1);
    }

    #[test]
    fn overview_renders_the_trend_sparklines() {
        let mut p = DatacenterPanel::new();
        // < 2 samples → the "trend builds" hint branch.
        let _ = p.view();
        // Two loads → real trend lines render.
        let load = DcLoad {
            rows: vec![row_with("droplet", "d1", "lh", "active", "prod")],
            ..DcLoad::default()
        };
        let _ = p.update(Message::Loaded(Ok(load.clone())));
        let _ = p.update(Message::Loaded(Ok(load)));
        assert!(p.history().len() >= 2);
        // Exercises sparklines_view → sparkline_row for every series.
        let _ = p.view();
    }

    // ── DATACENTER-9: farm / Eagle / per-lighthouse version matrix ────────────

    #[test]
    fn version_matrix_projects_farm_eagle_and_lighthouses() {
        let stages = vec![
            PromoteStage {
                stage: "build".into(),
                version: "11.0.2-1".into(),
                status: "ready".into(),
            },
            PromoteStage {
                stage: "eagle".into(),
                version: "11.0.1-1".into(),
                status: "ready".into(),
            },
            PromoteStage {
                stage: "do".into(),
                version: "11.0.1-1".into(),
                status: "pending".into(),
            },
        ];
        let rows = vec![
            // Two Prod lighthouses (unsorted by name) + a Dev VM that must NOT
            // appear in the matrix.
            row_with("droplet", "2", "lighthouse-02", "active", "prod"),
            row_with("droplet", "1", "lighthouse-01", "off", "prod"),
            row_with("vm", "v9", "builder", "running", "dev"),
        ];
        let m = VersionMatrix::project(&stages, &rows);
        // Farm + Eagle + two lighthouses.
        assert_eq!(m.rows.len(), 4);
        assert_eq!(m.rows[0].target, "Farm (build)");
        assert_eq!(m.rows[0].version, "11.0.2-1");
        assert_eq!(m.rows[0].status, "ready");
        assert_eq!(m.rows[1].target, "Eagle");
        assert_eq!(m.rows[1].version, "11.0.1-1");
        // Lighthouses, sorted by name; each pinned to the DO target version.
        assert_eq!(m.rows[2].target, "lighthouse-01");
        assert_eq!(m.rows[2].version, "11.0.1-1");
        assert_eq!(m.rows[2].status, "pending", "an off droplet is mid-flight");
        assert_eq!(m.rows[3].target, "lighthouse-02");
        assert_eq!(m.rows[3].version, "11.0.1-1");
        assert_eq!(m.rows[3].status, "ready", "an active droplet is converged");
    }

    #[test]
    fn version_matrix_fills_absent_stages_and_handles_no_lighthouses() {
        // No promote events + no droplets → farm + Eagle placeholder rows only.
        let m = VersionMatrix::project(&[], &[]);
        assert_eq!(m.rows.len(), 2);
        assert_eq!(m.rows[0].target, "Farm (build)");
        assert_eq!(m.rows[0].version, "—");
        assert_eq!(m.rows[0].status, "unknown");
        assert_eq!(m.rows[1].target, "Eagle");
        assert_eq!(m.rows[1].version, "—");
        // A lighthouse with no `do` stage observed → its version is "—".
        let rows = vec![row_with("droplet", "1", "lh-01", "active", "prod")];
        let m2 = VersionMatrix::project(&[], &rows);
        assert_eq!(m2.rows.len(), 3);
        assert_eq!(m2.rows[2].target, "lh-01");
        assert_eq!(m2.rows[2].version, "—", "no DO target observed yet");
    }

    #[test]
    fn version_matrix_falls_back_to_id_for_an_unnamed_lighthouse() {
        let rows = vec![row_with("droplet", "579112110", "", "active", "prod")];
        let m = VersionMatrix::project(&[], &rows);
        assert_eq!(m.rows[2].target, "579112110", "unnamed droplet → its id");
    }

    #[test]
    fn overview_renders_the_version_matrix() {
        let mut p = DatacenterPanel::new();
        p.promote = vec![PromoteStage {
            stage: "build".into(),
            version: "11.0.2-1".into(),
            status: "ready".into(),
        }];
        p.rows = vec![row_with("droplet", "1", "lh-01", "active", "prod")];
        // Exercises version_matrix_view → its header + lighthouse + chip branches.
        let _ = p.view();
    }

    // ── DATACENTER-8/1813 — per-kind sub-tabs ─────────────────────────────────

    #[test]
    fn kind_tab_matches_maps_kinds_onto_tabs() {
        assert!(KindTab::Hosts.matches("host"));
        assert!(!KindTab::Hosts.matches("vm"));
        assert!(KindTab::Vms.matches("vm"));
        assert!(KindTab::Vms.matches("droplet"));
        assert!(KindTab::Storage.matches("sr"));
        assert!(KindTab::Storage.matches("vdi"));
        assert!(KindTab::Storage.matches("iso"));
        assert!(KindTab::Network.matches("net"));
        assert!(KindTab::Network.matches("vlan"));
        // A kind that belongs to no tab matches none.
        for t in KindTab::all() {
            assert!(!t.matches("gateway"));
        }
    }

    #[test]
    fn kind_tab_slug_round_trips() {
        for t in KindTab::all() {
            assert_eq!(KindTab::from_slug(t.slug()), t);
        }
        // Unknown → VMs (the busy default).
        assert_eq!(KindTab::from_slug("nonsense"), KindTab::Vms);
    }

    #[test]
    fn kind_tab_message_switches_and_clears_bulk() {
        let mut p = DatacenterPanel::new();
        p.bulk_mode = true;
        p.bulk_sel.insert("v1".to_string());
        // Switching to a non-VM tab drops the bulk selection.
        let _ = p.update(Message::KindTab(KindTab::Hosts));
        assert_eq!(p.kind_tab, KindTab::Hosts);
        assert!(!p.bulk_mode);
        assert!(p.bulk_sel.is_empty());
    }

    #[test]
    fn zone_view_renders_each_kind_subtab_without_panicking() {
        let mut p = DatacenterPanel::new();
        p.rows = project_rows(&[
            (
                "event/dc/host/a".into(),
                r#"{"kind":"host","id":"172.20.0.9","name":"dom0-a","status":"up","zone":"dev","cpu":"8","mem_total_mb":"16000","mem_free_mb":"9000","load":"0.4"}"#.into(),
            ),
            (
                "event/dc/vm/9".into(),
                r#"{"kind":"vm","id":"9","name":"builder","status":"running","zone":"dev","host":"172.20.0.9"}"#.into(),
            ),
            (
                "event/dc/sr/1".into(),
                r#"{"kind":"sr","id":"sr-1","name":"local-ext","size":"222330230784","used":"42949672960","host":"172.20.0.9","zone":"dev"}"#.into(),
            ),
            (
                "event/dc/net/0".into(),
                r#"{"kind":"net","id":"net-0","name":"Pool-wide","status":"up","zone":"dev","bridge":"xenbr0"}"#.into(),
            ),
            (
                "event/dc/iso/x".into(),
                r#"{"kind":"iso","id":"iso-1","name":"fedora.iso","status":"ready","zone":"dev"}"#.into(),
            ),
        ]);
        let _ = p.update(Message::ViewMode(ViewMode::Zone));
        let _ = p.update(Message::ZoneTab("dev".to_string()));
        for t in KindTab::all() {
            let _ = p.update(Message::KindTab(t));
            let _ = p.view();
        }
    }

    // ── DATACENTER-11 — VM lifecycle ──────────────────────────────────────────

    #[test]
    fn suspend_clicked_sets_status_for_each_direction() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::SuspendClicked {
            uuid: "v1".into(),
            dom0: "172.20.0.9".into(),
            resume: false,
        });
        assert_eq!(p.status, "Suspending…");
        let _ = p.update(Message::SuspendClicked {
            uuid: "v1".into(),
            dom0: "172.20.0.9".into(),
            resume: true,
        });
        assert_eq!(p.status, "Resuming…");
    }

    #[test]
    fn migrate_arms_picker_and_needs_a_target() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::MigrateClicked {
            uuid: "v1".into(),
            dom0: "172.20.0.9".into(),
        });
        assert_eq!(p.migrate.as_ref().map(|(u, _)| u.as_str()), Some("v1"));
        // Confirm without a target is a no-op that asks for one.
        let _ = p.update(Message::MigrateConfirmed);
        assert!(p.migrate.is_some(), "still armed until a target is picked");
        assert_eq!(p.status, "Pick a target host first.");
        // Pick a target, then confirm clears the picker + sets in-flight status.
        let _ = p.update(Message::MigrateTargetPicked("172.20.145.193".into()));
        let _ = p.update(Message::MigrateConfirmed);
        assert!(p.migrate.is_none());
        assert_eq!(p.status, "Migrating to 172.20.145.193…");
        // Cancel path.
        let _ = p.update(Message::MigrateClicked {
            uuid: "v2".into(),
            dom0: "h".into(),
        });
        let _ = p.update(Message::MigrateCancelled);
        assert!(p.migrate.is_none());
    }

    #[test]
    fn bulk_mode_select_and_op_track_selection() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::BulkModeToggled);
        assert!(p.bulk_mode);
        let _ = p.update(Message::BulkSelectToggled("v1".into()));
        let _ = p.update(Message::BulkSelectToggled("v2".into()));
        assert_eq!(p.bulk_sel.len(), 2);
        // Toggling again deselects.
        let _ = p.update(Message::BulkSelectToggled("v1".into()));
        assert_eq!(p.bulk_sel.len(), 1);
        // A bulk op seeds per-item progress for the selected set.
        let _ = p.update(Message::BulkOp("shutdown".into()));
        assert!(p.bulk_progress.contains_key("v2"));
        // The results land per item.
        let _ = p.update(Message::BulkDone(Ok(vec![("v2".into(), "ok".into())])));
        assert_eq!(p.bulk_progress.get("v2").map(String::as_str), Some("ok"));
        // Exiting bulk mode clears selection.
        let _ = p.update(Message::BulkModeToggled);
        assert!(!p.bulk_mode);
        assert!(p.bulk_sel.is_empty());
    }

    #[test]
    fn bulk_op_with_no_selection_is_a_noop() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::BulkModeToggled);
        let _ = p.update(Message::BulkOp("start".into()));
        assert_eq!(p.status, "No VMs selected.");
        assert!(p.bulk_progress.is_empty());
    }

    #[test]
    fn create_wizard_validates_required_fields() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::CreateToggled);
        assert!(p.create_open);
        // Empty template/name → submit refuses (no in-flight).
        let _ = p.update(Message::CreateSubmit);
        assert_eq!(p.status, "Template and name are both required.");
        assert!(p.create_open, "stays open until a valid submit");
        // Fill both → submit closes the wizard + sets in-flight status.
        let _ = p.update(Message::CreateTemplateChanged("MDE-VM-golden".into()));
        let _ = p.update(Message::CreateNameChanged("builder-2".into()));
        let _ = p.update(Message::CreateSubmit);
        assert!(!p.create_open);
        assert_eq!(p.status, "Creating builder-2 from MDE-VM-golden…");
    }

    #[test]
    fn vm_console_done_ok_sets_status_to_the_url() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::VmConsoleDone(Ok("https://novnc.example/1".into())));
        assert_eq!(p.status, "console: https://novnc.example/1");
        let _ = p.update(Message::VmConsoleDone(Err("no console".into())));
        assert_eq!(p.status, "no console");
    }

    // ── DATACENTER-10/16 — host lifecycle + power ─────────────────────────────

    #[test]
    fn host_evacuate_arms_confirm_then_fires() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::HostEvacuateClicked("172.20.0.9".into()));
        assert_eq!(
            p.host_confirm.as_ref().map(|(h, o)| (h.as_str(), o.as_str())),
            Some(("172.20.0.9", "evacuate"))
        );
        let _ = p.update(Message::HostConfirmed);
        assert!(p.host_confirm.is_none());
        assert_eq!(p.status, "evacuate 172.20.0.9…");
    }

    #[test]
    fn host_patch_preview_arms_a_patch_confirm() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::HostPatchPreviewDone(Ok((
            "172.20.0.9".into(),
            vec!["builder".into(), "tester".into()],
        ))));
        assert_eq!(
            p.host_patch_preview.as_ref().map(|(h, v)| (h.as_str(), v.len())),
            Some(("172.20.0.9", 2))
        );
        assert_eq!(
            p.host_confirm.as_ref().map(|(_, o)| o.as_str()),
            Some("patch")
        );
        // Cancel clears both.
        let _ = p.update(Message::HostCancelled);
        assert!(p.host_confirm.is_none());
        assert!(p.host_patch_preview.is_none());
    }

    #[test]
    fn idle_policy_toggles_optimistically() {
        let mut p = DatacenterPanel::new();
        assert!(!p.idle_policy_on);
        let _ = p.update(Message::IdlePolicyToggled);
        assert!(p.idle_policy_on);
    }

    #[test]
    fn parse_power_event_and_progress() {
        let ps = parse_power_event(
            r#"{"host":"172.20.0.9","phase":"xcp","pct":"45","eta_secs":"30"}"#,
        )
        .unwrap();
        assert_eq!(ps.host, "172.20.0.9");
        assert_eq!(ps.pct_clamped(), 45);
        assert_eq!(ps.phase_label(), "XCP boot");
        // An over-100 pct clamps; a missing pct is 0.
        let hot = parse_power_event(r#"{"host":"h","phase":"post","pct":"250"}"#).unwrap();
        assert_eq!(hot.pct_clamped(), 100);
        let none = parse_power_event(r#"{"host":"h","phase":"post"}"#).unwrap();
        assert_eq!(none.pct_clamped(), 0);
        // Missing host → None.
        assert!(parse_power_event(r#"{"phase":"post"}"#).is_none());
    }

    #[test]
    fn project_power_drops_ready_and_sorts() {
        let events = vec![
            (
                "event/dc/power/172.20.0.9".into(),
                r#"{"host":"172.20.0.9","phase":"xcp","pct":"40"}"#.into(),
            ),
            (
                "event/dc/power/172.20.0.8".into(),
                r#"{"host":"172.20.0.8","phase":"ready","pct":"100"}"#.into(),
            ),
            // Not a power topic → dropped.
            (
                "event/dc/vm/9".into(),
                r#"{"kind":"vm","id":"9","status":"running","zone":"dev"}"#.into(),
            ),
        ];
        let rows = project_power(&events);
        assert_eq!(rows.len(), 1, "ready dropped, non-power dropped");
        assert_eq!(rows[0].host, "172.20.0.9");
    }

    #[test]
    fn progress_bar_fills_proportionally() {
        assert_eq!(progress_bar(0), "░░░░░░░░");
        assert_eq!(progress_bar(100), "████████");
        assert_eq!(progress_bar(50).chars().count(), 8);
        // Over-100 is clamped to a full bar.
        assert_eq!(progress_bar(250), "████████");
    }

    #[test]
    fn loaded_populates_power() {
        let mut p = DatacenterPanel::new();
        let load = DcLoad {
            power: vec![PowerStatus {
                host: "172.20.0.9".into(),
                phase: "post".into(),
                pct: "10".into(),
                eta_secs: "60".into(),
            }],
            ..DcLoad::default()
        };
        let _ = p.update(Message::Loaded(Ok(load)));
        assert_eq!(p.power.len(), 1);
    }

    // ── DATACENTER-12 — storage ───────────────────────────────────────────────

    #[test]
    fn sr_destroy_requires_confirm() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::SrDestroyClicked("sr-1".into()));
        assert_eq!(p.sr_confirm_destroy.as_deref(), Some("sr-1"));
        let _ = p.update(Message::SrDestroyConfirmed("sr-1".into()));
        assert!(p.sr_confirm_destroy.is_none());
        assert_eq!(p.status, "Destroying sr-1…");
        // Cancel path.
        let _ = p.update(Message::SrDestroyClicked("sr-2".into()));
        let _ = p.update(Message::SrDestroyCancelled);
        assert!(p.sr_confirm_destroy.is_none());
    }

    // ── DATACENTER-13 — network + IP/DNS ──────────────────────────────────────

    #[test]
    fn parse_ipdns_reads_correlated_entries() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"entries":[{"name":"eagle","lease_ip":"172.20.0.2","dns":"eagle.mesh","overlay_ip":"10.42.0.2"},{"name":"lh1"}]}"#,
        )
        .unwrap();
        let rows = parse_ipdns(&v);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "eagle");
        assert_eq!(rows[0].overlay_ip, "10.42.0.2");
        // Missing fields default empty.
        assert_eq!(rows[1].lease_ip, "");
        // No entries → empty.
        assert!(parse_ipdns(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn ipdns_done_populates_the_table() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::IpDnsDone(Ok(vec![IpDnsEntry {
            name: "eagle".into(),
            lease_ip: "172.20.0.2".into(),
            dns: "eagle.mesh".into(),
            overlay_ip: "10.42.0.2".into(),
        }])));
        assert_eq!(p.ipdns.len(), 1);
    }

    // ── DATACENTER-14 — gateway ───────────────────────────────────────────────

    #[test]
    fn gateway_firewall_requires_a_rule() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::GwFirewallClicked);
        assert_eq!(p.status, "Enter a firewall rule first.");
        let _ = p.update(Message::GwFwRuleChanged("allow tcp 443".into()));
        let _ = p.update(Message::GwFirewallClicked);
        assert_eq!(p.status, "Adding firewall rule…");
        assert!(p.gw_fw_rule.is_empty(), "the input clears on submit");
    }

    #[test]
    fn gateway_view_renders() {
        let mut p = DatacenterPanel::new();
        p.rows = project_rows(&[(
            "event/dc/gateway/gw0".into(),
            r#"{"kind":"gateway","id":"gw0","name":"unifi-gw","status":"up","zone":"prod"}"#.into(),
        )]);
        let _ = p.update(Message::ViewMode(ViewMode::Gateway));
        let _ = p.view();
    }

    // ── DATACENTER-15 — Tofu prod-arm + run-log ───────────────────────────────

    #[test]
    fn prod_apply_is_refused_while_disarmed() {
        let mut p = DatacenterPanel::new();
        // Disarmed: a prod (zone1-do) apply confirm is refused, no in-flight.
        let _ = p.update(Message::TofuApply("zone1-do".into()));
        assert!(p.status.contains("disarmed"));
        assert_ne!(p.tofu_output, "Applying zone1-do…");
        // Arm prod, then the apply proceeds.
        let _ = p.update(Message::TofuArmDone(Ok(true)));
        assert!(p.tofu_armed);
        let _ = p.update(Message::TofuApply("zone1-do".into()));
        assert_eq!(p.status, "Applying zone1-do…");
    }

    #[test]
    fn dev_apply_is_allowed_while_disarmed() {
        let mut p = DatacenterPanel::new();
        // The Dev (xen-xapi) workspace is never gated by prod-arm.
        let _ = p.update(Message::TofuApply("xen-xapi".into()));
        assert_eq!(p.status, "Applying xen-xapi…");
    }

    #[test]
    fn tofu_runlog_done_populates_the_log() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::TofuRunlogDone(Ok("apply 11:00 ok".into())));
        assert_eq!(p.tofu_runlog, "apply 11:00 ok");
    }

    // ── DATACENTER-19/18/21 — provisioning ────────────────────────────────────

    #[test]
    fn parse_do_regions_and_geo_and_spread() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"regions":[{"slug":"sfo3","name":"San Francisco 3","available":true},{"slug":"nyc1","name":"New York 1"},{"slug":"bad"}]}"#,
        )
        .unwrap();
        // Wait — {"slug":"bad"} is a valid region (name defaults to slug).
        let rs = parse_do_regions(&v);
        assert_eq!(rs.len(), 3);
        // Sorted by slug: bad < nyc1 < sfo3.
        assert_eq!(rs[0].slug, "bad");
        assert_eq!(rs[2].slug, "sfo3");
        assert_eq!(rs[1].geo(), "nyc");
        // available defaults true when absent.
        assert!(rs[1].available);
        // Distinct geos across a spread of slugs.
        assert_eq!(
            distinct_geos(&["sfo3".into(), "sfo2".into(), "nyc1".into()]),
            2
        );
    }

    #[test]
    fn genesis_validates_name_and_region() {
        let mut p = DatacenterPanel::new();
        // No region → asks for one.
        let _ = p.update(Message::GenesisNameChanged("nebula-2".into()));
        let _ = p.update(Message::GenesisSubmit);
        assert_eq!(p.provision_status, "Pick a region for the new mesh.");
        // Region but blank name → asks for a name.
        let _ = p.update(Message::GenesisRegionPicked("fra1".into()));
        let _ = p.update(Message::GenesisNameChanged("   ".into()));
        let _ = p.update(Message::GenesisSubmit);
        assert_eq!(p.provision_status, "Name the new mesh first.");
        // Both → in-flight.
        let _ = p.update(Message::GenesisNameChanged("nebula-2".into()));
        let _ = p.update(Message::GenesisSubmit);
        assert_eq!(p.provision_status, "Giving birth to nebula-2 in fra1…");
    }

    #[test]
    fn guided_lighthouse_needs_a_region() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::GuidedLighthouseClicked);
        assert_eq!(p.provision_status, "Pick a region first.");
        let _ = p.update(Message::RegionPicked("sfo3".into()));
        let _ = p.update(Message::GuidedLighthouseClicked);
        assert_eq!(p.provision_status, "Provisioning a lighthouse in sfo3…");
    }

    #[test]
    fn regions_done_populates_the_picker() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::RegionsDone(Ok(vec![DoRegion {
            slug: "sfo3".into(),
            name: "SF3".into(),
            available: true,
        }])));
        assert_eq!(p.do_regions.len(), 1);
    }

    #[test]
    fn provision_and_gateway_views_render_with_state() {
        let mut p = DatacenterPanel::new();
        p.do_regions = vec![DoRegion {
            slug: "sfo3".into(),
            name: "SF3".into(),
            available: true,
        }];
        p.rows = project_rows(&[
            (
                "event/dc/droplet/2".into(),
                r#"{"kind":"droplet","id":"2","name":"lh-01","status":"active","zone":"prod"}"#.into(),
            ),
            (
                "event/dc/testmesh/t1".into(),
                r#"{"kind":"testmesh","id":"t1","name":"ephemeral","status":"up","zone":"dev"}"#.into(),
            ),
        ]);
        let _ = p.update(Message::ViewMode(ViewMode::Provision));
        let _ = p.view();
    }

    // ── DATACENTER-20 — promotion controls ────────────────────────────────────

    #[test]
    fn promote_controls_toggle_optimistically() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::AutoPromoteToggled);
        assert!(p.auto_promote);
        let _ = p.update(Message::PromoteArmToggled);
        assert!(p.promote_armed);
        let _ = p.update(Message::PromoteNowClicked);
        assert_eq!(p.status, "Promoting…");
        // Overview renders the controls.
        let _ = p.view();
    }
}
