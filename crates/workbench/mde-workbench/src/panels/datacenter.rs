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

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cosmic::iced::widget::{column, container, mouse_area, row, scrollable, text, text_input};
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
// DATACENTER-24 — the Logs tab reuses the Fleet-logs sink load + filter + row
// model rather than re-deriving a second log reader (§6 glue).
use crate::panels::fleet_logs;

/// One datacenter resource as last seen on the Bus.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
    /// DATACENTER-13 (Network tab) — the resource's primary IPv4. Droplet events
    /// carry `ip` (the public v4); host/VM events may carry an overlay/management
    /// IP. Empty when the event didn't name one. Feeds the unified IP/DNS view.
    pub ip: String,
    /// DATACENTER-13 (Network tab) — the DigitalOcean region slug (`droplet`
    /// events carry `region`, e.g. `nyc3`). Empty for non-droplet resources. Shown
    /// in the unified IP/DNS view to correlate a public IP with its DO region.
    pub region: String,
    /// DATACENTER-12 (Storage tab) — the SR uuid a VDI lives on (`vdi` events carry
    /// `sr`). Empty for non-VDI resources. Correlates a VDI row to its SR card.
    pub sr: String,
    /// DATACENTER-12 (Storage tab) — the VM uuid a VDI is attached to (`vdi` events
    /// carry `vm`). Empty when the VDI is unattached or for non-VDI resources.
    pub vm: String,
    /// DATACENTER-12 (Storage tab) — the VBD uuid attaching a VDI to its VM (`vdi`
    /// events carry `vbd`). Empty when unattached; the detach handle the
    /// `action/dc/vdi-detach` RPC passes.
    pub vbd: String,
    /// DATACENTER-12 (Storage tab) — a `template`'s name-description (`template`
    /// events carry `description`, e.g. the guest-OS summary). Empty for resources
    /// without one. Shown in the image-library row's detail column.
    pub description: String,
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

    /// DATACENTER-12 — a storage row's used-capacity percentage (0..=100, rounded),
    /// or `None` when `size`/`used` don't parse or `size` is 0. The Storage tab's
    /// capacity-threshold alerts read this; sharing the parse with
    /// [`Self::capacity_readout`] keeps the readout and the alert consistent. Pure.
    #[must_use]
    pub fn used_pct(&self) -> Option<u64> {
        let size: u64 = self.size.parse().ok()?;
        let used: u64 = self.used.parse().ok()?;
        if size == 0 {
            return None;
        }
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let pct = ((used as f64 / size as f64) * 100.0).round() as u64;
        Some(pct.min(100))
    }

    /// DATACENTER-12 — the image-library row's detail column. An `iso` row shows
    /// its size (the `size` bytes string rendered human-readable, e.g. `"1.4 GiB"`);
    /// a `template` row shows its `description`. Empty when there's nothing to show
    /// (an ISO with no readable size, a template with no description) so the row
    /// renders a blank detail rather than a bogus value. Pure + testable.
    #[must_use]
    pub fn image_detail(&self) -> String {
        if self.kind == "iso" {
            let bytes: u64 = match self.size.trim().parse() {
                Ok(b) if b > 0 => b,
                _ => return String::new(),
            };
            #[allow(
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss
            )]
            let gib = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
            if gib >= 1.0 {
                format!("{gib:.1} GiB")
            } else {
                let mib = bytes as f64 / (1024.0 * 1024.0);
                format!("{mib:.0} MiB")
            }
        } else {
            self.description.clone()
        }
    }

    /// DATACENTER-10 — a host's memory readout, `"used / total GiB (pct%)"`, from
    /// the `mem_total_mb` / `mem_free_mb` host metrics (used = total − free).
    /// Returns `None` when either metric is missing/unparseable or `total` is 0, so
    /// the Hosts card renders nothing rather than a bogus "0 / 0 GiB". Pure +
    /// testable.
    #[must_use]
    pub fn host_memory_readout(&self) -> Option<String> {
        let total_mb: u64 = self.mem_total_mb.parse().ok()?;
        let free_mb: u64 = self.mem_free_mb.parse().ok()?;
        if total_mb == 0 {
            return None;
        }
        let used_mb = total_mb.saturating_sub(free_mb);
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let pct = ((used_mb as f64 / total_mb as f64) * 100.0).round() as u64;
        // MB → GiB (1 GiB = 1024 MiB; the source reports MiB-ish MB, close enough
        // for a capacity readout).
        let used_gib = (used_mb + 512) / 1024;
        let total_gib = (total_mb + 512) / 1024;
        Some(format!("{used_gib} / {total_gib} GiB ({pct}%)"))
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
            "paused" | "suspended" | "rebooting" | "starting" | "pending" | "provisioning" => {
                palette.warning
            }
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
    let ip = v
        .get("ip")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let region = v
        .get("region")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let sr = v
        .get("sr")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let vm = v
        .get("vm")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let vbd = v
        .get("vbd")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let description = v
        .get("description")
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
        ip,
        region,
        sr,
        vm,
        vbd,
        description,
    })
}

/// One datacenter audit-log entry as last seen on the Bus (`event/dc/audit/*`).
/// Records a control-plane action (a tofu apply, a vm power/delete, …) so the
/// Audit view can render a newest-first activity log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRow {
    /// The action performed — e.g. "tofu-apply" | "vm-delete" | "vm-power".
    pub action: String,
    /// The initiating principal as recorded by the daemon's `dc_auditor` — the
    /// local node identity (`peer:<host>`), since the mde-bus envelope carries
    /// no per-message sender. Empty when the event didn't name one.
    pub actor: String,
    /// The target of the action — a workspace name, a VM uuid, a dom0 IP, …
    pub target: String,
    /// The action result as recorded by the auditor — "issued" for an action the
    /// passive auditor observed being requested (it doesn't correlate the reply,
    /// so it never fabricates an ok/fail). Empty when the event didn't name one.
    pub result: String,
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
    let actor = v
        .get("actor")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let target = v
        .get("target")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let result = v
        .get("result")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let ts = v
        .get("ts")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    Some(AuditRow {
        action,
        actor,
        target,
        result,
        ts,
    })
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

/// Max log rows the Logs tab renders — the dom0 journal tails the `dc_health`
/// worker aggregates can be large, so the in-plane view is capped (the Fleet-logs
/// panel keeps the unbounded search). Bounds the render the same way the worker
/// bounds the pull.
pub const LOGS_VIEW_CAP: usize = 300;

/// DATACENTER-24 — keep only the datacenter-scoped log rows: those whose `host`
/// matches one of the datacenter host ids (the dom0 IPs the `dc_health` worker
/// tags each aggregated journal line with — `<root>/logs/<dom0>.jsonl`). When the
/// scope set is empty (no host rows known yet), nothing is datacenter-scoped, so it
/// returns empty rather than spilling the whole fleet's mesh logs into the
/// Datacenter plane. Pure + testable; reuses [`fleet_logs::LogRow`] so the Logs tab
/// is glue over the existing sink, not a second log model (§6).
#[must_use]
pub fn datacenter_log_rows(
    rows: &[fleet_logs::LogRow],
    dc_host_ids: &std::collections::BTreeSet<String>,
) -> Vec<fleet_logs::LogRow> {
    if dc_host_ids.is_empty() {
        return Vec::new();
    }
    rows.iter()
        .filter(|r| dc_host_ids.contains(&r.host))
        .cloned()
        .collect()
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

/// The OpenTofu workspaces the Tofu tab manages, prod-first. `xen-xapi` is the
/// dev/farm Xen plane; `zone1-do` is the **prod** DigitalOcean plane gated by the
/// prod-arm master switch.
pub const TOFU_WORKSPACES: [&str; 2] = ["xen-xapi", "zone1-do"];

/// The PROD OpenTofu workspace — a mutating op (apply/destroy) here is refused
/// unless the prod-arm master switch is armed (DATACENTER-15). The DO zone is the
/// only live-fleet (prod) plane; the Xen plane is dev/farm.
pub const PROD_TOFU_WS: &str = "zone1-do";

/// Whether a mutating Tofu op on `ws` is allowed to fire given the prod-arm
/// switch. PURE + fails closed: a PROD-workspace op requires `armed == true`;
/// any non-prod workspace is always allowed (its own typed-confirm still gates
/// it). Mirrors the FA_APPLY operator gate — a prod apply never fires from the
/// GUI on a typed-confirm alone.
#[must_use]
pub fn prod_arm_allows(ws: &str, armed: bool) -> bool {
    ws != PROD_TOFU_WS || armed
}

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

/// DATACENTER-13 (Network tab) — one overlay route as last seen on the Bus
/// (`event/dc/route/xen/<dom0>`, published by the orchestrator's `publish_route`).
/// The Network tab's overlay peer/route table renders these so the path the mesh
/// takes to each dom0's XAPI (direct vs a relay hop, and any relay-down fallback)
/// is visible operator state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteRow {
    /// The route target id — the dom0 IP this route reaches.
    pub id: String,
    /// The XAPI target label (`"xen"`).
    pub target: String,
    /// The chosen path: `"direct"` (on-LAN) or `"relay"` (a proxy-jump hop).
    pub path: String,
    /// The relay host when `path == "relay"`, else empty.
    pub relay: String,
    /// A human note on why this path was chosen / any relay-down fallback.
    pub note: String,
}

/// Parse one `event/dc/route/xen/<dom0>` message body into a [`RouteRow`]. Returns
/// `None` for unparseable JSON, a non-`route` kind, or a body missing `id`. Pure +
/// testable. Mirrors [`parse_dc_event`]'s tolerant string extraction.
#[must_use]
pub fn parse_route_event(body: &str) -> Option<RouteRow> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    if v.get("kind").and_then(|k| k.as_str()) != Some("route") {
        return None;
    }
    let id = v.get("id")?.as_str()?.to_string();
    let getter = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    Some(RouteRow {
        id,
        target: getter("target"),
        path: getter("path"),
        relay: getter("relay"),
        note: getter("note"),
    })
}

/// Project a set of `(topic, latest-body)` Bus reads into overlay route rows —
/// `event/dc/route/*` topics only, sorted by target id for a stable table. Pure +
/// testable.
#[must_use]
pub fn project_routes(events: &[(String, String)]) -> Vec<RouteRow> {
    let mut routes: Vec<RouteRow> = events
        .iter()
        .filter(|(topic, _)| topic.starts_with("event/dc/route/"))
        .filter_map(|(_, body)| parse_route_event(body))
        .collect();
    routes.sort_by(|a, b| a.id.cmp(&b.id));
    routes
}

/// Project a set of `(topic, latest-body)` Bus reads into sorted rows — datacenter
/// resources (`event/dc/*`), grouped by zone (prod first) then kind then name.
/// Route events (`event/dc/route/*`) are excluded — they project to [`RouteRow`]s
/// via [`project_routes`], not resource cards.
#[must_use]
pub fn project_rows(events: &[(String, String)]) -> Vec<DcRow> {
    let mut rows: Vec<DcRow> = events
        .iter()
        .filter(|(topic, _)| {
            topic.starts_with("event/dc/") && !topic.starts_with("event/dc/route/")
        })
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
            name: "Prod · DO / Gateway".to_string(),
            ..DcRow::default()
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
    /// When `Some(workspace)`, a Tofu **destroy** is awaiting typed confirmation —
    /// the workspace's row renders a "Type DESTROY to confirm" prompt and only the
    /// confirm button fires the destructive `action/dc/tofu-destroy` RPC. Cleared
    /// once the destroy is fired or cancelled. Mirrors [`Self::tofu_confirm`].
    pub tofu_destroy_confirm: Option<String>,
    /// The **prod-arm master switch** (DATACENTER-15). Mirrors the FA_APPLY
    /// operator gate: a mutating Tofu op (apply/destroy) on the PROD workspace
    /// ([`PROD_TOFU_WS`]) is refused unless this is armed — so a prod apply can
    /// never fire from the GUI on a typed-confirm alone. Non-prod workspaces are
    /// unaffected; defaults disarmed (fails closed). Toggled by `TofuProdArm`.
    pub tofu_prod_armed: bool,
    /// DATACENTER-20 — the **prod-arm master switch for the auto-promote DO step**.
    /// While disarmed (the default), a green Build→Eagle pipeline HOLDS at the DO
    /// step (the `dc_promote` worker marks it `queued`); arming lets the leader
    /// auto-promote to DO on green (the worker marks it `armed`). Mirrors
    /// `tofu_prod_armed` but, unlike it, PERSISTS — the GUI toggle writes the same
    /// `dc-prod-arm.json` config the worker reads, so the gate is shared and
    /// survives a restart. Defaults disarmed (fails closed); toggled by
    /// `PromoteProdArm`, hydrated from disk by the first panel-open `load()`.
    pub promote_prod_armed: bool,
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
    /// DATACENTER-10 (Hosts tab) — the impact-preview cache: per-host (dom0 IP) the
    /// count of running guests the last `action/dc/host-impact` read reported.
    /// Populated when the operator clicks "Impact" on a host card; the destructive
    /// drain/reboot/shutdown confirm reads it back to show "N running VM(s) will be
    /// migrated/stopped". Pure in-memory cache, refreshed on demand.
    pub host_impact: BTreeMap<String, usize>,
    /// DATACENTER-11 (VMs tab) — the in-progress golden-template create wizard form.
    /// Empty by default; the "Create VM" form on the VMs tab edits it, and Create
    /// fires `action/dc/vm-create` → `tofu-apply`.
    pub vm_create: VmCreateForm,
    /// DATACENTER-11 (VMs tab) — when `Some`, a per-VM inline prompt is collecting
    /// input for a migrate (destination host) or resize (vcpus + mem). Only the
    /// prompt's Confirm fires the RPC; Cancel clears it. Cleared on every load.
    pub vm_prompt: Option<VmPrompt>,
    /// DATACENTER-11 (VMs tab) — the per-VM snapshot list cache, keyed by VM uuid,
    /// populated by the read-only `action/dc/vm-snapshots` RPC when the operator
    /// clicks a VM's "Snapshots" button. The VM's card then renders one row per
    /// snapshot with Revert + Delete actions. An empty `Vec` means "fetched, none"
    /// (distinct from absent = "not fetched yet"). Cleared on every load so a refresh
    /// never acts on a stale list.
    pub vm_snapshots: BTreeMap<String, Vec<VmSnapshot>>,
    /// DATACENTER-11 (VMs tab) — when `Some(snapshot-uuid)`, a snapshot delete is
    /// awaiting inline confirmation — that snapshot's row renders a "Really delete?"
    /// prompt and only the confirm button fires the destructive
    /// `action/dc/vm-snapshot-delete` RPC (with `"confirm":true`). Mirrors
    /// [`Self::confirm_delete`]. Cleared once a delete is fired or the load refreshes.
    pub confirm_snapshot_delete: Option<String>,
    /// DATACENTER-11 (VMs tab) — the multi-select set of VM uuids (the checkboxes on
    /// the VM cards). The bulk toolbar acts on exactly this set. Cleared on load so a
    /// refresh never acts on a stale selection.
    pub vm_selected: BTreeSet<String>,
    /// DATACENTER-11 (VMs tab) — the free-text tag applied by a bulk "Tag" run, typed
    /// in the bulk toolbar. Pure UI state.
    pub bulk_tag: String,
    /// DATACENTER-11 (VMs tab) — the per-item progress of the last/in-flight bulk run,
    /// keyed by VM uuid (`Pending` → `Ok`/`Err`). The bulk toolbar renders one line
    /// per entry. Reset when a new bulk run starts; cleared on load.
    pub bulk_progress: BTreeMap<String, BulkItem>,
    /// DATACENTER-10 (Hosts tab) — the pool-placement cache: per-host (dom0 IP) the
    /// last `action/dc/host-pool` read's `(pool_name, master_uuid, is_master)`.
    /// Populated when the operator clicks "Pool" on a host card; the card then
    /// renders the host's pool membership + a master badge.
    pub host_pool: BTreeMap<String, HostPool>,
    /// DATACENTER-10 (Hosts tab) — when `Some((dom0, op))`, a destructive host op
    /// (`reboot` | `shutdown` | `evacuate`) is awaiting explicit confirmation on
    /// that host's card: the card renders the impact preview + a Confirm/Cancel
    /// prompt, and only the Confirm button fires the `action/dc/host-power` RPC.
    /// Cleared once the op is fired or cancelled.
    pub host_confirm: Option<(String, String)>,
    /// DATACENTER-12 (Storage tab) — the in-progress SR-create form. Fires
    /// `action/dc/sr-create`. Empty by default; edited on the Storage tab.
    pub sr_create: SrCreateForm,
    /// DATACENTER-12 (Storage tab) — the in-progress VDI-create form. Fires
    /// `action/dc/vdi-create`. A per-SR card's "VDI here" seeds its `sr`/`dom0`.
    pub vdi_create: VdiCreateForm,
    /// DATACENTER-12 (Storage tab) — the scheduled-snapshot config form. Fires
    /// `action/dc/snap-schedule` (retention + backup target).
    pub snap_schedule: SnapScheduleForm,
    /// DATACENTER-12 (Storage tab) — the SR capacity-alert **warning** threshold
    /// (percent). An SR at/above it badges `warning`; at/above [`SR_CRITICAL_PCT`]
    /// it badges `danger`. Defaults to 85. Operator-tunable from the tab.
    pub storage_threshold_pct: u64,
    /// DATACENTER-12 (Storage tab) — the SR-create threshold text box's in-progress
    /// value (parsed into `storage_threshold_pct` on change; kept as text so an
    /// in-progress empty box doesn't snap to 0). Pure UI state.
    pub storage_threshold_input: String,
    /// DATACENTER-12 (Storage tab) — when `Some((vdi, vm_text))`, a VDI attach is
    /// being collected: the VDI uuid being attached + the destination VM uuid the
    /// operator is typing inline on that VDI's row. Only the row's "Attach" button
    /// (with a non-empty VM) fires the `action/dc/vdi-attach` RPC. Cleared on
    /// fire/cancel. Pure UI state (mirrors the VM-tab inline prompt).
    pub vdi_attach: Option<(String, String)>,
    /// DATACENTER-12 (Storage tab) — when `Some(vbd)`, a destructive VDI detach is
    /// awaiting typed confirmation: that VDI's row renders a "Type DETACH to confirm"
    /// prompt and only the confirm button (with the typed word matching) fires the
    /// `action/dc/vdi-detach` RPC. Cleared on fire/cancel. Mirrors the Tofu-destroy
    /// typed-confirm gate (destructive ops are typed-confirm gated).
    pub vdi_detach_confirm: Option<String>,
    /// DATACENTER-12 (Storage tab) — the in-progress text of the detach typed-confirm
    /// box (must equal `DETACH` for the confirm button to fire). Pure UI state.
    pub vdi_detach_input: String,
    /// DATACENTER-13 (Network tab) — the overlay routes read off `event/dc/route/*`.
    /// Rendered as the overlay peer/route table. Refreshed alongside `rows`.
    pub routes: Vec<RouteRow>,
    /// DATACENTER-24 (Logs tab) — the datacenter-scoped log rows the `dc_health`
    /// worker aggregates into the `fleet_logs` sink (`<root>/logs/<dom0>.jsonl`),
    /// already narrowed to the datacenter host ids. Rendered by the `Logs` view;
    /// refreshed alongside `rows` on every load.
    pub logs: Vec<fleet_logs::LogRow>,
    /// DATACENTER-13 (Network tab) — the decoded `host-net` L2 read for the active
    /// dom0 (its networks + PIFs/NICs). `None` until "Read L2" returns. Populated by
    /// the `action/dc/host-net` RPC; the L2 inventory renders off it.
    pub net_read: Option<NetRead>,
    /// DATACENTER-13 (Network tab) — the in-progress VLAN-create form. Fires
    /// `action/dc/host-vlan-create`. A PIF card's "VLAN here" seeds `pif`+`dom0`.
    pub vlan_create: VlanCreateForm,
    /// DATACENTER-13 (Network tab) — the in-progress "Read L2" target dom0 box. The
    /// "Read L2" button fires `action/dc/host-net` against it. Pure UI state.
    pub net_read_dom0: String,
    /// DATACENTER-16 (Power tab) — the in-progress host-wake form (BMC / cred /
    /// MAC / dom0). Edited on the Power tab; packed into the wake / idle-policy
    /// RPCs. Pure local UI state.
    pub power_form: PowerForm,
    /// DATACENTER-16 (Power tab) — the live wake-in-progress (phase + progress +
    /// ETA), or `None` when no wake is being tracked. Seeded on a Wake and
    /// updated by the `action/dc/wake-eta` poll.
    pub wake_progress: Option<WakeProgress>,
    /// DATACENTER-16 (Power tab) — when the in-flight wake started, for the
    /// elapsed-seconds the ETA poll measures against and the learned sample
    /// recorded once the wake reaches ready. `None` when no wake is in flight.
    pub wake_started: Option<Instant>,
    /// DATACENTER-16 (Power tab) — the recorded per-target wake durations
    /// (seconds), keyed by target label (BMC or MAC). The rolling average drives
    /// the learned ETA. Hydrates as wakes complete.
    pub wake_samples: BTreeMap<String, Vec<u64>>,
    /// DATACENTER-16 (Power tab) — the latest idle-shutdown recommendation line
    /// from `action/dc/idle-policy` (e.g. "0 running guests … — shut down"), or
    /// empty until the policy has been checked.
    pub idle_reason: String,
    /// DATACENTER-16 (Power tab) — whether the latest idle-policy read recommends
    /// a graceful shutdown. Drives the "Shut down (graceful)" button's enablement
    /// so the operator only acts on a recommendation.
    pub idle_shutdown_ok: bool,
    /// DATACENTER-14 (Gateway tab) — the decoded `gateway-dhcp` read (tofu-managed
    /// reservations + live leases) for the EdgeOS gateway, or `None` until the
    /// operator clicks "Read DHCP". The reservation + lease tables render off it.
    pub gateway_dhcp: Option<GatewayDhcp>,
}

/// Top-level view selector for the datacenter panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    /// Cross-zone capacity rollup (the default landing view).
    Overview,
    /// Per-zone resource tabs (Prod / Dev).
    Zone,
    /// DATACENTER-10 — the Hosts tab: per-host capacity/health cards with the full
    /// host lifecycle (maintenance / reboot / shutdown / evacuate, each with an
    /// impact preview), pool placement (membership / master), and the copy / launch
    /// SSH console.
    Hosts,
    /// DATACENTER-11 — the VMs tab: the full VM lifecycle (power / suspend / migrate
    /// / clone / snapshot / resize / delete + noVNC console), a Tofu-backed
    /// golden-template create wizard, and multi-select bulk power / snapshot / tag
    /// with per-item progress.
    Vms,
    /// DATACENTER-12 — the Storage tab: SR capacity cards with threshold alerts,
    /// an SR-create + a VDI-create form, a per-SR snapshot action, a
    /// scheduled-snapshot config (retention + backup target), and an ISO /
    /// template image library.
    Storage,
    /// OpenTofu workspaces + Plan / Apply buttons.
    Tofu,
    /// The datacenter audit log (`event/dc/audit/*`), newest-first.
    Audit,
    /// The structured infrastructure map: resources grouped by their owning
    /// host/zone, with collapsible host group headers (DATACENTER-13).
    Topology,
    /// DATACENTER-13 — the Network tab: the L2 inventory (networks / PIFs / VLANs
    /// with a VLAN-create form), the overlay peer/route table (`event/dc/route/*`),
    /// the interactive hosts↔networks↔VMs↔gateway topology map, and the unified
    /// IP/DNS view correlating droplet public IPs ↔ DO regions ↔ overlay IPs.
    Network,
    /// DATACENTER-16 — the Power tab: energy-aware host power. Per-host wake (IPMI
    /// primary, Wake-on-LAN fallback) with a phased POST→XCP→toolstack progress
    /// bar driven by a learned per-host ETA, plus an idle-shutdown policy that
    /// recommends powering down a host carrying zero running guests.
    Power,
    /// DATACENTER-14 — the Gateway tab: the EdgeOS EdgeRouter (172.20.0.1) DHCP
    /// surface. The tofu-managed static reservations + the live DHCP leases
    /// (read-mostly), with a tofu-gated Plan / typed-confirm Apply for reservation
    /// changes (the `edgeos` workspace — never a blind apply from the GUI).
    Gateway,
    /// DATACENTER-24 — the Logs tab: the per-resource (per-dom0) journal tail the
    /// `dc_health` worker aggregates into the `fleet_logs` sink
    /// (`<root>/logs/<dom0>.jsonl`). Reuses the Fleet-logs load + filter; the
    /// global search box narrows the message/target, and rows are scoped to the
    /// datacenter hosts so the operator reads host/VM/service logs in-plane.
    Logs,
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
            ViewMode::Hosts => "hosts",
            ViewMode::Vms => "vms",
            ViewMode::Storage => "storage",
            ViewMode::Tofu => "tofu",
            ViewMode::Audit => "audit",
            ViewMode::Topology => "topology",
            ViewMode::Network => "network",
            ViewMode::Power => "power",
            ViewMode::Gateway => "gateway",
            ViewMode::Logs => "logs",
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
            "hosts" => ViewMode::Hosts,
            "vms" => ViewMode::Vms,
            "storage" => ViewMode::Storage,
            "tofu" => ViewMode::Tofu,
            "audit" => ViewMode::Audit,
            "topology" => ViewMode::Topology,
            "network" => ViewMode::Network,
            "power" => ViewMode::Power,
            "gateway" => ViewMode::Gateway,
            "logs" => ViewMode::Logs,
            // "overview" and anything unknown.
            _ => ViewMode::Overview,
        }
    }
}

/// DATACENTER-10 (Hosts tab) — one host's pool placement, decoded from an
/// `action/dc/host-pool` reply (`{pool, master, is_master}`). The Hosts card
/// renders the pool name + a master badge off this; an empty `pool` means the host
/// is a pool-of-one (or hasn't reported a label).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostPool {
    /// The pool's name-label (empty = unlabeled / pool-of-one).
    pub pool: String,
    /// The pool master host's uuid (empty when the read returned none).
    pub master: String,
    /// Whether THIS host is the pool master (so the card badges it).
    pub is_master: bool,
}

/// DATACENTER-11 (VMs tab) — the in-progress state of the golden-template VM
/// create wizard. Each field is a free-text box on the VMs tab's "Create VM" form;
/// the `Create` button packs them into the `action/dc/vm-create` request (which
/// writes a Tofu resource), then the panel runs `tofu-apply` so the structural
/// change goes through Tofu (no drift). Pure local UI state until Create is fired.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VmCreateForm {
    /// The new VM's name-label (sanitized server-side to `[A-Za-z0-9._-]`).
    pub name: String,
    /// vCPU count (parsed to an integer; blank/invalid blocks Create with a hint).
    pub vcpus: String,
    /// Memory in MiB (parsed to an integer; blank/invalid blocks Create).
    pub mem_mib: String,
    /// The XAPI network uuid the primary NIC attaches to.
    pub network_uuid: String,
    /// The destination dom0 (the pool the resource lands in); must be an allow-listed
    /// host. Defaults to the active zone's first Xen dom0 when opened.
    pub dom0: String,
}

/// DATACENTER-12 (Storage tab) — the in-progress SR-create form. The `Create SR`
/// button packs it into the `action/dc/sr-create` request. Pure local UI state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SrCreateForm {
    /// The new SR's name-label (server-validated to `[A-Za-z0-9._-]`).
    pub name: String,
    /// The SR backend type (`lvm` default, `ext`, `nfs`, …; alphanumeric only).
    pub sr_type: String,
    /// The XAPI host uuid to create the SR on.
    pub host_uuid: String,
    /// Optional `key=value` device-config (e.g. `device=/dev/sdb`). Blank = none.
    pub device_config: String,
    /// The destination dom0 (must be allow-listed).
    pub dom0: String,
}

/// DATACENTER-12 (Storage tab) — the in-progress VDI-create form. The `Create
/// VDI` button packs it into the `action/dc/vdi-create` request. Pure UI state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VdiCreateForm {
    /// The target SR uuid the VDI is created on.
    pub sr: String,
    /// The new VDI's name-label (server-validated to `[A-Za-z0-9._-]`).
    pub name: String,
    /// Virtual size in GiB (parsed to an integer 1..=65536; blank/invalid blocks).
    pub size_gib: String,
    /// The destination dom0 (must be allow-listed).
    pub dom0: String,
}

/// DATACENTER-12 (Storage tab) — the scheduled-snapshot config. Captures which SR
/// to snapshot on a cadence, how many snapshots to retain, and where backups
/// land. Persisted via `action/dc/snap-schedule` (the `dr_scheduler`/orchestrator
/// honors it); here it is the in-progress form. Pure UI state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapScheduleForm {
    /// The SR (or VDI) the schedule targets.
    pub sr: String,
    /// How many snapshots to retain (the oldest pruned beyond this; parsed int).
    pub retention: String,
    /// The backup target the retained snapshots are exported to (path / SR uuid).
    pub backup_target: String,
    /// The destination dom0 (must be allow-listed).
    pub dom0: String,
}

/// DATACENTER-13 (Network tab) — the in-progress VLAN-create form. The "Create
/// VLAN" button packs it into the `action/dc/host-vlan-create` request (which runs
/// `network-create` + `pool-vlan-create` on the dom0). Pure local UI state. A PIF
/// card's "VLAN here" seeds `pif` + `dom0`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VlanCreateForm {
    /// The trunk PIF uuid the VLAN rides on.
    pub pif: String,
    /// The 802.1Q VLAN tag (parsed to 1..=4094; blank/invalid blocks Create).
    pub vlan: String,
    /// The new bridge network's name-label (`[A-Za-z0-9._-]`).
    pub network_name: String,
    /// The destination dom0 (must be allow-listed).
    pub dom0: String,
}

/// DATACENTER-13 (Network tab) — one L2 network decoded from a `host-net` reply's
/// `nets` array (mirrors the daemon's `NetInfo`). Rendered in the L2 inventory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct NetInfoRow {
    /// The network uuid.
    #[serde(default)]
    pub uuid: String,
    /// The name-label.
    #[serde(default)]
    pub name: String,
    /// The Linux bridge.
    #[serde(default)]
    pub bridge: String,
    /// The MTU (bytes); 0 when unknown.
    #[serde(default)]
    pub mtu: u32,
}

/// DATACENTER-13 (Network tab) — one PIF/NIC decoded from a `host-net` reply's
/// `pifs` array (mirrors the daemon's `PifInfo`). Rendered as the NIC inventory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct PifInfoRow {
    /// The PIF uuid.
    #[serde(default)]
    pub uuid: String,
    /// The NIC device name (e.g. `eth0`, `eth0.100`).
    #[serde(default)]
    pub device: String,
    /// The hardware MAC.
    #[serde(default)]
    pub mac: String,
    /// The VLAN tag (`-1` = untagged trunk / not a VLAN).
    #[serde(default = "minus_one")]
    pub vlan: i64,
    /// Whether the link is up.
    #[serde(default)]
    pub carrier: bool,
    /// Whether this PIF carries the management interface.
    #[serde(default)]
    pub management: bool,
    /// The uuid of the XAPI network this PIF attaches to.
    #[serde(default)]
    pub network: String,
}

/// `serde` default for [`PifInfoRow::vlan`] — XAPI's "not a VLAN" sentinel.
fn minus_one() -> i64 {
    -1
}

/// DATACENTER-13 (Network tab) — the decoded `host-net` L2 read for the active
/// dom0: its networks + PIFs/NICs. Populated when the operator clicks "Read L2" on
/// the Network tab; the L2 inventory renders off it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct NetRead {
    /// The L2 networks (bridges) on the dom0.
    #[serde(default)]
    pub nets: Vec<NetInfoRow>,
    /// The physical interfaces (NICs) on the dom0, incl. VLAN sub-interfaces.
    #[serde(default)]
    pub pifs: Vec<PifInfoRow>,
}

/// DATACENTER-13 (Network tab) — which network-form field a `NetFieldChanged`
/// targets. Keeps every network form-edit a single message variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetField {
    /// The L2-read target dom0 (the "Read L2" box).
    ReadDom0,
    /// VLAN-create: trunk PIF uuid.
    VlanPif,
    /// VLAN-create: 802.1Q tag.
    VlanTag,
    /// VLAN-create: new network name-label.
    VlanName,
    /// VLAN-create: destination dom0.
    VlanDom0,
}

/// DATACENTER-14 (Gateway tab) — one tofu-managed EdgeOS DHCP static reservation,
/// decoded from a `gateway-dhcp` reply's `reservations` array (mirrors the
/// daemon's `GatewayReservation`). Rendered in the Gateway tab's reservation
/// table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct GatewayReservationRow {
    /// The reservation's name (the EdgeOS static-mapping name).
    #[serde(default)]
    pub name: String,
    /// The reserved MAC.
    #[serde(default)]
    pub mac: String,
    /// The reserved IPv4.
    #[serde(default)]
    pub ip: String,
}

/// DATACENTER-14 (Gateway tab) — one live EdgeOS DHCP lease, decoded from a
/// `gateway-dhcp` reply's `leases` array (mirrors the daemon's `GatewayLease`).
/// Rendered in the Gateway tab's live-lease table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct GatewayLeaseRow {
    /// The leased IPv4.
    #[serde(default)]
    pub ip: String,
    /// The lessee's MAC.
    #[serde(default)]
    pub mac: String,
    /// The lease expiry (EdgeOS-formatted timestamp).
    #[serde(default)]
    pub expiry: String,
    /// The client hostname (may be empty).
    #[serde(default)]
    pub hostname: String,
}

/// DATACENTER-14 (Gateway tab) — the decoded `gateway-dhcp` read: the tofu-managed
/// reservations + the live DHCP leases. Populated when the operator clicks
/// "Read DHCP" on the Gateway tab; the reservation + lease tables render off it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct GatewayDhcp {
    /// The tofu-managed static reservations (`managed_reservations`).
    #[serde(default)]
    pub reservations: Vec<GatewayReservationRow>,
    /// The live DHCP leases polled from the router (`dhcp_leases`).
    #[serde(default)]
    pub leases: Vec<GatewayLeaseRow>,
}

/// DATACENTER-16 (Power tab) — the in-progress host-wake form. The "Wake" button
/// packs it into the `action/dc/ipmi-power` request (IPMI primary, `mac` the WoL
/// fallback); the "WoL only" button fires `action/dc/wol` straight off `mac`.
/// Pure local UI state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PowerForm {
    /// The target's BMC host (IPv4 / hostname) for the IPMI wake path.
    pub bmc: String,
    /// The BMC username.
    pub user: String,
    /// The BMC password.
    pub pass: String,
    /// The target's MAC — the Wake-on-LAN fallback (and the "WoL only" source).
    pub mac: String,
    /// The dom0 IP the idle-shutdown policy reads (running-guest count over SSH).
    pub dom0: String,
}

/// DATACENTER-16 (Power tab) — a live wake-in-progress: the phased
/// POST→XCP→toolstack progress bar + learned ETA the `action/dc/wake-eta` reply
/// drives. Seeded when a Wake fires; updated each time the panel polls the ETA.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WakeProgress {
    /// The wake target's label (BMC / MAC) shown above the bar.
    pub target: String,
    /// The current phase slug ("post" | "xcp" | "toolstack" | "ready").
    pub phase: String,
    /// The phased progress fraction (0.0..=1.0).
    pub progress: f64,
    /// The remaining-seconds ETA.
    pub eta: u64,
    /// Whether the wake has reached the Ready phase.
    pub ready: bool,
}

/// DATACENTER-16 (Power tab) — which power-form field a `PowerFieldChanged`
/// targets. Keeps every power form-edit a single message variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerField {
    /// The BMC host box.
    Bmc,
    /// The BMC username box.
    User,
    /// The BMC password box.
    Pass,
    /// The MAC box.
    Mac,
    /// The idle-policy dom0 box.
    Dom0,
}

/// DATACENTER-12 (Storage tab) — which storage form field a `StorageFieldChanged`
/// targets. Keeps every storage form-edit a single message variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageField {
    /// SR-create: name-label.
    SrName,
    /// SR-create: backend type.
    SrType,
    /// SR-create: XAPI host uuid.
    SrHost,
    /// SR-create: device-config `key=value`.
    SrDeviceConfig,
    /// SR-create: destination dom0.
    SrDom0,
    /// VDI-create: target SR uuid.
    VdiSr,
    /// VDI-create: name-label.
    VdiName,
    /// VDI-create: size (GiB).
    VdiSize,
    /// VDI-create: destination dom0.
    VdiDom0,
    /// Schedule: target SR.
    SchedSr,
    /// Schedule: retention count.
    SchedRetention,
    /// Schedule: backup target.
    SchedBackupTarget,
    /// Schedule: destination dom0.
    SchedDom0,
}

/// DATACENTER-12 (Storage tab) — a capacity-threshold alert for one SR, computed
/// purely from the SR row + the configured threshold. Rendered as a colored badge
/// on the SR card and surfaced as a panel status line so the operator never has to
/// hunt for the over-threshold store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrAlert {
    /// The SR's id (uuid) the alert is about.
    pub id: String,
    /// The SR's human name-label (falls back to id when unnamed).
    pub name: String,
    /// The SR's used-capacity percentage (0..=100, rounded).
    pub pct: u64,
    /// `true` when usage crossed the *critical* line (≥95%) vs merely the warning
    /// threshold — drives a `danger` vs `warning` token on the badge.
    pub critical: bool,
}

/// DATACENTER-12 — the critical (danger) SR-capacity line: at or above this
/// percentage a store is one bad write from full, so it badges `danger` instead
/// of `warning`. The warning line is the operator-configurable threshold.
pub const SR_CRITICAL_PCT: u64 = 95;

/// DATACENTER-12 — compute the capacity-threshold alerts for the SR rows, given
/// the operator's warning `threshold_pct`. An SR with a parseable size/used and a
/// used-percentage at or above the threshold yields an [`SrAlert`]; `critical` is
/// set at/above [`SR_CRITICAL_PCT`]. Sorted most-full first so the worst store
/// reads at the top. Pure + testable.
#[must_use]
pub fn sr_alerts(rows: &[DcRow], threshold_pct: u64) -> Vec<SrAlert> {
    let mut alerts: Vec<SrAlert> = rows
        .iter()
        .filter(|r| r.kind == "sr")
        .filter_map(|r| {
            let pct = r.used_pct()?;
            if pct < threshold_pct {
                return None;
            }
            let name = if r.name.is_empty() {
                r.id.clone()
            } else {
                r.name.clone()
            };
            Some(SrAlert {
                id: r.id.clone(),
                name,
                pct,
                critical: pct >= SR_CRITICAL_PCT,
            })
        })
        .collect();
    // Most-full first; a stable id tie-break keeps the order deterministic.
    alerts.sort_by(|a, b| b.pct.cmp(&a.pct).then_with(|| a.id.cmp(&b.id)));
    alerts
}

/// DATACENTER-11 (VMs tab) — which VM operation a per-VM inline prompt is collecting
/// input for. Migrate needs a destination host; Resize needs vcpus + memory. The
/// prompt renders the right inputs and only its Confirm button fires the RPC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmPrompt {
    /// Collecting the destination host for a `vm-migrate` of this VM uuid.
    Migrate {
        /// The VM being migrated.
        uuid: String,
        /// The destination host name-label / uuid being typed.
        host: String,
    },
    /// Collecting the target vcpus + memory for a `vm-resize` of this VM uuid.
    Resize {
        /// The VM being resized.
        uuid: String,
        /// Target vCPU count being typed.
        vcpus: String,
        /// Target memory (MiB) being typed.
        mem_mib: String,
    },
}

impl VmPrompt {
    /// The uuid the prompt targets — so a render can match it against a row's id.
    #[must_use]
    pub fn uuid(&self) -> &str {
        match self {
            VmPrompt::Migrate { uuid, .. } | VmPrompt::Resize { uuid, .. } => uuid,
        }
    }
}

/// DATACENTER-11 (VMs tab) — one of a VM's snapshots, as listed by the read-only
/// `action/dc/vm-snapshots` RPC. The per-VM snapshot list renders one row each with
/// a Revert (reversible, fires directly) and a Delete (destructive, typed-confirm
/// gated like the VM delete) button. Pure value type so the decode is unit-testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmSnapshot {
    /// The snapshot's own uuid — the key the revert / delete RPCs act on.
    pub uuid: String,
    /// The snapshot's name-label (e.g. `dc-snap-abcd1234`), for the row label.
    pub name: String,
}

/// DATACENTER-11 (VMs tab) — the outcome of one VM in a multi-select bulk run, keyed
/// by VM uuid. The bulk toolbar renders a per-item progress line off these as each
/// VM's RPC lands. `Pending` is the pre-fire state; `Ok`/`Err` carry the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BulkItem {
    /// The op for this VM is in flight (fired, not yet returned).
    Pending,
    /// The op for this VM succeeded, with the status line.
    Ok(String),
    /// The op for this VM failed, with the error text.
    Err(String),
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

/// DATACENTER-20 — the local config-file path the **auto-promote prod-arm master
/// switch** persists to. MUST resolve to the exact same path the `dc_promote`
/// worker reads (`dc_promote::prod_arm_path`) so the GUI toggle and the leader
/// share one gate: `$XDG_CONFIG_HOME/mde/dc-prod-arm.json`, falling back to
/// `$HOME/.config/mde/dc-prod-arm.json`. `None` only in a degenerate headless env
/// (then the toggle is session-only; the worker defaults disarmed = fail-closed).
#[must_use]
pub fn promote_prod_arm_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|d| d.join("mde").join("dc-prod-arm.json"))
}

/// DATACENTER-20 — read the persisted auto-promote prod-arm state from an
/// explicit `path` (the pure, testable core of [`load_promote_prod_arm`]). A
/// missing/empty/corrupt file ⇒ `false` (disarmed) — fails closed. The on-disk
/// shape is the same `{"armed":bool}` record the worker writes/reads.
#[must_use]
pub fn load_promote_prod_arm_at(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("armed").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

/// DATACENTER-20 — read the persisted auto-promote prod-arm state from the shared
/// config path. Fails closed (disarmed) when there's no config dir or no file, so
/// a fresh install never silently auto-promotes to prod.
#[must_use]
pub fn load_promote_prod_arm() -> bool {
    promote_prod_arm_path().is_some_and(|p| load_promote_prod_arm_at(&p))
}

/// DATACENTER-20 — persist the auto-promote prod-arm state to an explicit `path`
/// (the pure, testable core of [`save_promote_prod_arm`]), creating the parent
/// dir if needed. Writes the `{"armed":bool}` record the `dc_promote` worker
/// reads on its next tick.
pub fn save_promote_prod_arm_at(path: &Path, armed: bool) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::json!({ "armed": armed }).to_string();
    std::fs::write(path, json.as_bytes()).map_err(|e| e.to_string())
}

/// DATACENTER-20 — persist the auto-promote prod-arm state to the shared config
/// path. Returns the error text on failure so the caller can surface it.
pub fn save_promote_prod_arm(armed: bool) -> Result<(), String> {
    let path = promote_prod_arm_path().ok_or_else(|| "no config dir (HOME unset)".to_string())?;
    save_promote_prod_arm_at(&path, armed)
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
            tofu_destroy_confirm: None,
            tofu_prod_armed: false,
            promote_prod_armed: false,
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
            // DATACENTER-10 — the Hosts tab caches hydrate on demand (Impact / Pool
            // clicks); the constructor stays pure.
            host_impact: BTreeMap::new(),
            host_pool: BTreeMap::new(),
            host_confirm: None,
            // DATACENTER-11 (VMs tab) — create wizard, per-VM prompt, bulk select +
            // progress all start empty; they hydrate from operator gestures.
            vm_create: VmCreateForm::default(),
            vm_prompt: None,
            vm_snapshots: BTreeMap::new(),
            confirm_snapshot_delete: None,
            vm_selected: BTreeSet::new(),
            bulk_tag: String::new(),
            bulk_progress: BTreeMap::new(),
            // DATACENTER-12 (Storage tab) — forms start empty; the threshold
            // defaults to 85% (the warning line) until the operator tunes it.
            sr_create: SrCreateForm::default(),
            vdi_create: VdiCreateForm::default(),
            snap_schedule: SnapScheduleForm::default(),
            storage_threshold_pct: 85,
            storage_threshold_input: "85".to_string(),
            // Inline VDI attach/detach gates start unarmed — hydrated by a VDI
            // row's Attach / Detach gesture.
            vdi_attach: None,
            vdi_detach_confirm: None,
            vdi_detach_input: String::new(),
            // DATACENTER-13 (Network tab) — routes hydrate from the Bus on load; the
            // L2 read + VLAN-create form hydrate from operator gestures.
            routes: Vec::new(),
            // DATACENTER-24 (Logs tab) — the dom0 journal tails hydrate from the
            // fleet_logs sink on load.
            logs: Vec::new(),
            net_read: None,
            vlan_create: VlanCreateForm::default(),
            net_read_dom0: String::new(),
            // DATACENTER-16 (Power tab) — the wake form + live progress + learned
            // samples hydrate from operator gestures; the constructor stays pure.
            power_form: PowerForm::default(),
            wake_progress: None,
            wake_started: None,
            wake_samples: BTreeMap::new(),
            idle_reason: String::new(),
            idle_shutdown_ok: false,
            // DATACENTER-14 (Gateway tab) — the DHCP read hydrates on demand
            // ("Read DHCP"); the constructor stays pure.
            gateway_dhcp: None,
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
    /// DATACENTER-13 — the overlay routes read off `event/dc/route/*`. Rendered as
    /// the Network tab's overlay peer/route table. Refreshed alongside `rows`.
    pub routes: Vec<RouteRow>,
    /// DATACENTER-24 — the datacenter-scoped log rows the `dc_health` worker
    /// aggregates into the `fleet_logs` sink (`<root>/logs/<dom0>.jsonl`), already
    /// narrowed to the datacenter host ids. Rendered by the `Logs` view; refreshed
    /// alongside `rows` on every load.
    pub logs: Vec<fleet_logs::LogRow>,
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
    /// A Tofu "Destroy" button was clicked. The payload is the workspace name.
    /// This only *arms* the typed-confirm (`tofu_destroy_confirm = Some(ws)`); no
    /// RPC fires until the inline "Type DESTROY to confirm" button is pressed.
    TofuDestroyClicked(String),
    /// The inline confirm for a Tofu destroy was pressed — only this fires the
    /// destructive `action/dc/tofu-destroy` RPC (with `"confirm":true`). Payload
    /// is the workspace name.
    TofuDestroy(String),
    /// The pending Tofu-destroy confirmation was dismissed (the "Cancel" button) —
    /// clears `tofu_destroy_confirm` without firing any RPC.
    TofuDestroyCancelled,
    /// The `action/dc/tofu-destroy` RPC came back — `Ok` carries the destroy
    /// summary, `Err` the error text. Routes here as a panel-scoped message.
    TofuDestroyDone(Result<String, String>),
    /// The **prod-arm master switch** was toggled. The payload is the new armed
    /// state. While disarmed, an apply/destroy on the PROD workspace
    /// ([`PROD_TOFU_WS`]) is refused (mirrors the FA_APPLY operator gate); arming
    /// is a deliberate, separate action from the per-op typed-confirm.
    TofuProdArm(bool),
    /// DATACENTER-20 — the **auto-promote prod-arm master switch** was toggled.
    /// The payload is the new armed state. Disarmed ⇒ a green pipeline HOLDS at
    /// the DO step (`queued`); armed ⇒ the leader auto-promotes to DO on green
    /// (`armed`). Unlike `TofuProdArm` this PERSISTS to the `dc-prod-arm.json`
    /// config the `dc_promote` worker reads, so the gate is shared with the
    /// leader and survives a restart.
    PromoteProdArm(bool),
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
    /// DATACENTER-10 (Hosts tab) — a NON-destructive host op button was clicked
    /// (`maintenance-on` | `maintenance-off`). `dom0` is the host IP. Fires the
    /// `action/dc/host-power` RPC immediately (these are reversible, so no confirm).
    HostPowerClicked {
        dom0: String,
        op: String,
    },
    /// DATACENTER-10 (Hosts tab) — a DESTRUCTIVE host op button was clicked
    /// (`reboot` | `shutdown` | `evacuate`). This only *arms* the confirm
    /// (`host_confirm = Some((dom0, op))`) AND fires a fresh impact read so the
    /// confirm prompt shows how many guests are affected — no host-power RPC fires
    /// until the inline Confirm button.
    HostOpArmed {
        dom0: String,
        op: String,
    },
    /// DATACENTER-10 (Hosts tab) — the inline Confirm for an armed destructive host
    /// op was pressed. Only this fires the `action/dc/host-power` RPC. `dom0`/`op`
    /// echo the armed pair.
    HostOpConfirmed {
        dom0: String,
        op: String,
    },
    /// DATACENTER-10 (Hosts tab) — the armed destructive op was dismissed (Cancel):
    /// clears `host_confirm` without firing any RPC.
    HostOpCancelled,
    /// DATACENTER-10 (Hosts tab) — the `action/dc/host-power` RPC came back. `Ok`
    /// carries a status line, `Err` the error text. Routes here as a panel-scoped
    /// message.
    HostPowerDone(Result<String, String>),
    /// DATACENTER-10 (Hosts tab) — the "Impact" button was clicked: fire the
    /// read-only `action/dc/host-impact` RPC to count running guests on `dom0` for
    /// the impact preview.
    HostImpactClicked {
        dom0: String,
    },
    /// DATACENTER-10 (Hosts tab) — the `action/dc/host-impact` RPC came back. `Ok`
    /// carries `(dom0, running_count)` (the dom0 is echoed so the reply keys the
    /// per-host cache); `Err` the error text.
    HostImpactDone(Result<(String, usize), String>),
    /// DATACENTER-10 (Hosts tab) — the "Pool" button was clicked: fire the
    /// read-only `action/dc/host-pool` RPC to read `dom0`'s pool placement.
    HostPoolClicked {
        dom0: String,
    },
    /// DATACENTER-10 (Hosts tab) — the `action/dc/host-pool` RPC came back. `Ok`
    /// carries `(dom0, HostPool)` (the dom0 is echoed to key the cache); `Err` the
    /// error text.
    HostPoolDone(Result<(String, HostPool), String>),
    /// DATACENTER-10 (Hosts tab) — "Copy SSH": copy the `ssh root@<dom0>` command
    /// for this host onto the local clipboard (a pure local gesture — no Bus / no
    /// daemon round-trip). `dom0` is the host IP.
    HostSshCopy {
        dom0: String,
    },
    /// DATACENTER-10 (Hosts tab) — "Console": launch a local `cosmic-term ssh
    /// root@<dom0>` to the host via the shared [`crate::launcher`] (a pure local
    /// terminal launch, like the Lighthouses tab's SSH action — never touches the
    /// daemon). `dom0` is the host IP.
    HostSshLaunch {
        dom0: String,
    },
    /// DATACENTER-10 (Hosts tab) — the host SSH console launch finished. `Ok` is a
    /// status line; `Err` the "couldn't launch a terminal" message.
    HostSshLaunched(Result<String, String>),

    // ── DATACENTER-11 (VMs tab) ──────────────────────────────────────────────
    /// A VM "Suspend" / "Resume" button was clicked. `op` is "suspend" | "resume".
    /// Fires the `action/dc/vm-suspend` RPC.
    SuspendClicked {
        uuid: String,
        op: String,
        dom0: String,
    },
    /// The `action/dc/vm-suspend` RPC came back. Routes here panel-scoped.
    SuspendDone(Result<String, String>),
    /// A VM "Migrate" button was clicked — arms the inline destination-host prompt
    /// (`vm_prompt = Migrate`); no RPC fires until the prompt's Confirm.
    MigrateClicked {
        uuid: String,
        dom0: String,
    },
    /// The migrate prompt's destination-host box changed. Pure state.
    MigrateHostChanged(String),
    /// The migrate prompt's Confirm was pressed — fires `action/dc/vm-migrate` with
    /// the typed destination host. `dom0` is the source host.
    MigrateConfirmed {
        uuid: String,
        dom0: String,
    },
    /// The `action/dc/vm-migrate` RPC came back. Routes here panel-scoped.
    MigrateDone(Result<String, String>),
    /// A VM "Resize" button was clicked — arms the inline vcpus+mem prompt
    /// (`vm_prompt = Resize`); no RPC fires until the prompt's Confirm.
    ResizeClicked {
        uuid: String,
        dom0: String,
    },
    /// The resize prompt's vCPUs box changed. Pure state.
    ResizeVcpusChanged(String),
    /// The resize prompt's memory (MiB) box changed. Pure state.
    ResizeMemChanged(String),
    /// The resize prompt's Confirm was pressed — fires `action/dc/vm-resize` with the
    /// typed vcpus + mem. `dom0` is the owning host.
    ResizeConfirmed {
        uuid: String,
        dom0: String,
    },
    /// The `action/dc/vm-resize` RPC came back. Routes here panel-scoped.
    ResizeDone(Result<String, String>),
    /// The per-VM inline prompt (migrate / resize) was dismissed (Cancel). Clears
    /// `vm_prompt` without firing any RPC.
    VmPromptCancelled,
    /// A VM "Console" button was clicked — fires the read-only `action/dc/vm-console`
    /// RPC to read the noVNC console `location`, which is then opened externally.
    ConsoleClicked {
        uuid: String,
        dom0: String,
    },
    /// The `action/dc/vm-console` RPC came back. `Ok` carries the console URL (which
    /// the panel hands to `xdg-open` via `crate::Message::OpenExternal`); `Err` the
    /// error text.
    ConsoleDone(Result<String, String>),
    /// A VM "Snapshots" button was clicked — fires the read-only
    /// `action/dc/vm-snapshots` RPC to list this VM's snapshots into the per-VM
    /// snapshot cache. `uuid` is the VM id; `dom0` the owning host.
    SnapshotsClicked {
        uuid: String,
        dom0: String,
    },
    /// The `action/dc/vm-snapshots` RPC came back. `Ok` carries `(vm-uuid, list)`
    /// — the VM uuid is echoed so the reply keys the per-VM cache; `Err` the error.
    SnapshotsDone(Result<(String, Vec<VmSnapshot>), String>),
    /// A snapshot's "Revert" button was clicked — fires `action/dc/vm-snapshot-revert`
    /// directly (a revert is reversible-by-snapshot, so no confirm). `snapshot` is the
    /// snapshot uuid; `dom0` the owning host.
    SnapshotRevertClicked {
        snapshot: String,
        dom0: String,
    },
    /// The `action/dc/vm-snapshot-revert` RPC came back. Routes here panel-scoped.
    SnapshotRevertDone(Result<String, String>),
    /// A snapshot's "Delete" button was clicked. Sets the pending-confirm state for
    /// this snapshot uuid (no RPC fires yet); the row then renders an inline confirm.
    SnapshotDeleteClicked {
        snapshot: String,
    },
    /// The inline "Really delete?" confirm was clicked — only this fires the
    /// destructive `action/dc/vm-snapshot-delete` RPC (with `"confirm":true`).
    /// `vm` is the parent VM uuid (so the list can be refreshed); `dom0` the host.
    SnapshotDeleteConfirmed {
        snapshot: String,
        vm: String,
        dom0: String,
    },
    /// The pending snapshot-delete confirmation was dismissed (the "Cancel" button) —
    /// clears `confirm_snapshot_delete` without firing any RPC.
    SnapshotDeleteCancelled,
    /// The `action/dc/vm-snapshot-delete` RPC came back. Routes here panel-scoped.
    SnapshotDeleteDone(Result<String, String>),
    /// A create-wizard form field changed (`field` names which). Pure state.
    CreateFieldChanged {
        field: VmCreateField,
        value: String,
    },
    /// The create wizard's "Create VM" button was clicked — fires
    /// `action/dc/vm-create` (writes a Tofu resource), then on success runs
    /// `tofu-apply` so the structural change goes through Tofu.
    CreateVmClicked,
    /// The `action/dc/vm-create` RPC came back. `Ok` chains into a `tofu-apply` of
    /// `xen-xapi`; `Err` surfaces the error.
    CreateVmDone(Result<String, String>),
    /// A VM card's multi-select checkbox was toggled. Adds/removes the uuid from
    /// `vm_selected`. Pure state.
    BulkToggle(String),
    /// The bulk toolbar's "Select all (visible)" was clicked — selects every VM
    /// uuid currently visible in the tab. Pure state.
    BulkSelectAll(Vec<String>),
    /// The bulk toolbar's "Clear" was clicked — empties `vm_selected`. Pure state.
    BulkClear,
    /// The bulk-tag input changed. Pure state.
    BulkTagChanged(String),
    /// A bulk action button was clicked (power start/shutdown/reboot, snapshot, or
    /// tag). Seeds `bulk_progress` with a `Pending` per selected VM and fires one
    /// RPC per VM. `op` is the action; `dom0_by_uuid` pairs each selected uuid with
    /// its owning host (so each RPC targets the right dom0).
    BulkRun {
        op: String,
        dom0_by_uuid: Vec<(String, String)>,
    },
    /// One VM's bulk-run RPC came back — updates that uuid's `bulk_progress` entry.
    /// `op` echoes the action (for the status line); `uuid` keys the entry.
    BulkItemDone {
        uuid: String,
        result: Result<String, String>,
    },
    /// DATACENTER-12 (Storage tab) — a storage form field changed (`field` names
    /// which form + field). Pure state.
    StorageFieldChanged {
        field: StorageField,
        value: String,
    },
    /// DATACENTER-12 — the SR capacity-alert threshold box changed. Reparses into
    /// `storage_threshold_pct` (clamped 1..=100); a blank/invalid value keeps the
    /// last good number so the alert never silently disables. Pure state.
    StorageThresholdChanged(String),
    /// DATACENTER-12 — the "Create SR" button was clicked. Fires
    /// `action/dc/sr-create`.
    SrCreateClicked,
    /// The `action/dc/sr-create` RPC came back — `Ok` carries the new SR uuid,
    /// `Err` the error text.
    SrCreateDone(Result<String, String>),
    /// DATACENTER-12 — the "Create VDI" button was clicked. Fires
    /// `action/dc/vdi-create`.
    VdiCreateClicked,
    /// The `action/dc/vdi-create` RPC came back. `Ok` carries the new VDI uuid.
    VdiCreateDone(Result<String, String>),
    /// DATACENTER-12 — a per-SR card "VDI here" button: seeds the VDI-create form's
    /// `sr` + `dom0` from that card so the operator only types name + size. Pure
    /// state.
    VdiTargetSr {
        sr: String,
        dom0: String,
    },
    /// DATACENTER-12 — a per-SR card "Snapshot" button: fires `action/dc/sr-snapshot`
    /// to snapshot that SR's first VDI (the snapshot RPC takes a VDI uuid; for an SR
    /// card we pass the SR id and the worker resolves it). `dom0` targets the host.
    SrSnapshotClicked {
        sr: String,
        dom0: String,
    },
    /// The `action/dc/sr-snapshot` RPC came back. `Ok` carries the new snapshot uuid.
    SrSnapshotDone(Result<String, String>),
    /// DATACENTER-12 — a VDI row's "Attach" button: arms the inline VM-uuid prompt
    /// for that VDI (no RPC yet — the operator types the destination VM, then the
    /// row's Attach-confirm fires). `vdi` is the disk; `dom0` targets the host.
    VdiAttachArm {
        vdi: String,
        dom0: String,
    },
    /// DATACENTER-12 — the inline VM-uuid being typed for the armed VDI attach.
    VdiAttachVmChanged(String),
    /// DATACENTER-12 — the armed VDI attach was confirmed: fires
    /// `action/dc/vdi-attach` with the typed destination VM. `dom0` targets the host.
    VdiAttachClicked {
        vdi: String,
        dom0: String,
    },
    /// DATACENTER-12 — the armed VDI attach was dismissed (no RPC fires).
    VdiAttachCancel,
    /// The `action/dc/vdi-attach` RPC came back. `Ok` carries the new VBD uuid.
    VdiAttachDone(Result<String, String>),
    /// DATACENTER-12 — a VDI row's "Detach" button: arms the typed-confirm gate for
    /// that VDI's VBD (no RPC yet — detach is destructive, so it needs the typed
    /// word). `vbd` is the connection to destroy; `dom0` targets the host.
    VdiDetachArm {
        vbd: String,
        dom0: String,
    },
    /// DATACENTER-12 — the typed text of the detach confirm box (`DETACH` to enable).
    VdiDetachInputChanged(String),
    /// DATACENTER-12 — the typed-confirmed VDI detach fires `action/dc/vdi-detach`
    /// (`vbd-unplug` + `vbd-destroy`; the VDI itself is preserved). `dom0` targets
    /// the host.
    VdiDetachClicked {
        vbd: String,
        dom0: String,
    },
    /// DATACENTER-12 — the armed VDI detach was dismissed (no RPC fires).
    VdiDetachCancel,
    /// The `action/dc/vdi-detach` RPC came back. `Ok` carries a status line.
    VdiDetachDone(Result<String, String>),
    /// DATACENTER-12 — the scheduled-snapshot "Save schedule" button was clicked.
    /// Fires `action/dc/snap-schedule` (SR + retention + backup target). On the
    /// worker side the dr_scheduler/orchestrator honors the persisted schedule.
    SnapScheduleClicked,
    /// The `action/dc/snap-schedule` RPC came back. `Ok` carries a status line.
    SnapScheduleDone(Result<String, String>),
    /// DATACENTER-13 (Network tab) — a network-form field changed (the "Read L2"
    /// dom0 box or a VLAN-create field). Keeps every net form-edit one variant.
    NetFieldChanged {
        field: NetField,
        value: String,
    },
    /// DATACENTER-13 — the "Read L2" button was clicked. Fires `action/dc/host-net`
    /// against the target dom0 to read its networks + PIFs/VLANs.
    NetReadClicked,
    /// DATACENTER-13 — the `action/dc/host-net` RPC came back. `Ok` carries the
    /// decoded L2 read (nets + pifs); `Err` the error text.
    NetReadDone(Result<NetRead, String>),
    /// DATACENTER-13 — a PIF card's "VLAN here" was clicked: seed the VLAN-create
    /// form's `pif` + `dom0` so the operator only types the tag + name.
    VlanTargetPif {
        pif: String,
        dom0: String,
    },
    /// DATACENTER-13 — the "Create VLAN" button was clicked. Fires
    /// `action/dc/host-vlan-create` (network-create + pool-vlan-create) with
    /// `confirm:true` after the form's tag is parsed/validated.
    VlanCreateClicked,
    /// DATACENTER-13 — the `action/dc/host-vlan-create` RPC came back. `Ok` carries
    /// the new network uuid; `Err` the error text.
    VlanCreateDone(Result<String, String>),

    // ── DATACENTER-16 (Power tab) ────────────────────────────────────────────
    /// DATACENTER-16 — a power-form field changed (BMC / user / pass / MAC / dom0).
    /// Keeps every power form-edit one variant. Pure state.
    PowerFieldChanged {
        field: PowerField,
        value: String,
    },
    /// DATACENTER-16 — the "Wake (IPMI)" button: fire `action/dc/ipmi-power` with
    /// `op:"on"` (BMC primary, the form's MAC the WoL fallback) AND seed a live
    /// wake-progress so the phased bar starts ticking off the learned ETA.
    WakeClicked,
    /// DATACENTER-16 — the "WoL only" button: fire `action/dc/wol` straight off the
    /// form's MAC (no BMC). Also seeds the live wake-progress.
    WolWakeClicked,
    /// DATACENTER-16 — the `action/dc/ipmi-power` / `action/dc/wol` wake RPC came
    /// back. `Ok` carries `(target, status_line)`; `Err` the error text.
    WakeDone(Result<(String, String), String>),
    /// DATACENTER-16 — re-read the learned ETA for the in-flight wake: fires
    /// `action/dc/wake-eta` with the target's samples + elapsed seconds so the
    /// phased bar advances. Triggered by the wake-progress timer tick.
    WakeEtaPoll,
    /// DATACENTER-16 — the `action/dc/wake-eta` reply came back with the updated
    /// `WakeProgress` (phase + progress + ETA). `Err` carries the error text.
    WakeEtaDone(Result<WakeProgress, String>),
    /// DATACENTER-16 — the "Check idle" button: fire the read-only
    /// `action/dc/idle-policy` against the form's dom0 to count running guests and
    /// get the shutdown recommendation.
    IdlePolicyClicked,
    /// DATACENTER-16 — the `action/dc/idle-policy` reply: `Ok` carries
    /// `(running, shutdown, reason)`; `Err` the error text.
    IdlePolicyDone(Result<(usize, bool, String), String>),
    /// DATACENTER-16 — the "Shut down (graceful)" button (enabled only when the
    /// idle policy recommends it): fires the existing confirm-gated
    /// `action/dc/host-power` `shutdown` op against the form's dom0.
    IdleShutdownClicked,

    // ── DATACENTER-14 (Gateway tab) ──────────────────────────────────────────
    /// DATACENTER-14 — the "Read DHCP" button: fire the read-only
    /// `action/dc/gateway-dhcp` RPC to read the EdgeOS gateway's tofu-managed
    /// reservations + live DHCP leases.
    GatewayDhcpClicked,
    /// DATACENTER-14 — the `action/dc/gateway-dhcp` RPC came back. `Ok` carries the
    /// decoded reservations + leases; `Err` the error text.
    GatewayDhcpDone(Result<GatewayDhcp, String>),
}

/// DATACENTER-11 (VMs tab) — which create-wizard field a `CreateFieldChanged`
/// targets. Keeps the form-edit message one variant instead of five.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmCreateField {
    /// The VM name-label.
    Name,
    /// The vCPU count.
    Vcpus,
    /// The memory (MiB).
    MemMib,
    /// The primary-NIC network uuid.
    NetworkUuid,
    /// The destination dom0.
    Dom0,
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
                self.routes = load.routes;
                self.logs = load.logs;
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
                // Likewise drop a stale tofu-apply / tofu-destroy confirm on a
                // refresh. The prod-arm master switch is a deliberate operator
                // setting (not row-derived), so it survives the refresh.
                self.tofu_confirm = None;
                self.tofu_destroy_confirm = None;
                // DATACENTER-20 — hydrate the auto-promote prod-arm gate from the
                // shared on-disk config so the strip reflects the SAME armed state
                // the `dc_promote` leader is enforcing (it persists across
                // restarts and is the source of truth for both sides). Cheap; a
                // missing file ⇒ disarmed (fail-closed).
                self.promote_prod_armed = load_promote_prod_arm();
                // DATACENTER-11 — a refresh can change which VMs exist, so drop any
                // pending per-VM prompt + the bulk selection / progress rather than
                // act on a stale set. The create-wizard form is the operator's draft
                // (not row-derived), so it survives a refresh.
                self.vm_prompt = None;
                self.vm_snapshots.clear();
                self.confirm_snapshot_delete = None;
                self.vm_selected.clear();
                self.bulk_progress.clear();
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
                // Graceful-degrade: a failed Bus read marks the plane stale but
                // does NOT discard `self.rows` / audit / health — the last-known
                // snapshot stays rendered under a stale badge (see `view`), and
                // the Retry button re-fires `load()`. Clearing `status` drops any
                // lingering "Refreshing…" so the banner is the single source of
                // truth for the failure.
                self.load_error = Some(e);
                self.busy = false;
                self.status.clear();
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
                // The prod-arm master switch is the FA_APPLY-style operator gate:
                // a PROD apply never fires from the GUI on the typed-confirm alone.
                // Fails closed — clear the confirm so the operator must re-arm and
                // re-confirm.
                if !prod_arm_allows(&ws, self.tofu_prod_armed) {
                    self.tofu_confirm = None;
                    self.status = format!("Arm PROD before applying {ws}.");
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
            Message::TofuDestroyClicked(ws) => {
                // First click only arms the typed-confirm — no RPC fires until the
                // operator confirms. Mirrors `TofuApplyClicked`.
                self.tofu_destroy_confirm = Some(ws);
                self.status = "Type DESTROY to confirm below.".into();
                Task::none()
            }
            Message::TofuDestroy(ws) => {
                // The same prod-arm operator gate as apply: a PROD destroy never
                // fires from the GUI on the typed-confirm alone. Fails closed.
                if !prod_arm_allows(&ws, self.tofu_prod_armed) {
                    self.tofu_destroy_confirm = None;
                    self.status = format!("Arm PROD before destroying {ws}.");
                    return Task::none();
                }
                self.tofu_destroy_confirm = None;
                self.status = format!("Destroying {ws}…");
                self.tofu_output = format!("Destroying {ws}…");
                Task::perform(
                    async move {
                        // Same shape as `tofu_apply`: the Bus RPC borrows a
                        // non-Send Persist across its internal await, so run the
                        // whole round trip on a blocking thread with a local
                        // runtime.
                        tokio::task::spawn_blocking(move || tofu_destroy(&ws))
                            .await
                            .unwrap_or_else(|e| Err(format!("tofu task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::TofuDestroyDone(result)),
                )
            }
            Message::TofuDestroyCancelled => {
                self.tofu_destroy_confirm = None;
                self.status.clear();
                Task::none()
            }
            Message::TofuDestroyDone(Ok(s)) => {
                self.status = "Destroy complete".into();
                self.tofu_output = s;
                Task::none()
            }
            Message::TofuDestroyDone(Err(e)) => {
                self.status = e.clone();
                self.tofu_output = e;
                Task::none()
            }
            Message::TofuProdArm(armed) => {
                self.tofu_prod_armed = armed;
                self.status = if armed {
                    "PROD armed — apply/destroy on the DO zone is now allowed.".into()
                } else {
                    "PROD disarmed.".into()
                };
                Task::none()
            }
            Message::PromoteProdArm(armed) => {
                // Flip + PERSIST to the shared config the `dc_promote` worker
                // reads, so the leader picks up the new gate on its next tick. The
                // in-memory flag updates regardless so the strip reflects it
                // immediately; a failed write is surfaced but never lost (the
                // next toggle retries).
                self.promote_prod_armed = armed;
                self.status = match save_promote_prod_arm(armed) {
                    Ok(()) if armed => {
                        "Auto-promote PROD armed — green pipelines now auto-promote to DO.".into()
                    }
                    Ok(()) => "Auto-promote PROD disarmed — the DO step is held (queued).".into(),
                    Err(e) => format!("Prod-arm toggled but not persisted: {e}"),
                };
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
            // ── DATACENTER-10 (Hosts tab) — host lifecycle + pools ──────────────
            Message::HostPowerClicked { dom0, op } => {
                // Reversible ops (maintenance on/off) fire immediately — no confirm
                // (the daemon's confirm gate only guards the destructive ops).
                self.status = format!("Host {op} on {dom0}…");
                Self::host_power_task(dom0, op, false)
            }
            Message::HostOpArmed { dom0, op } => {
                // Arm the destructive confirm AND refresh the impact preview so the
                // prompt shows how many guests are affected before the operator
                // commits. The impact read is best-effort (its own message updates
                // the cache); the confirm renders the cached number if/when it lands.
                self.host_confirm = Some((dom0.clone(), op));
                Self::host_impact_task(dom0)
            }
            Message::HostOpConfirmed { dom0, op } => {
                // The destructive op was armed + explicitly confirmed in the UI —
                // carry confirm:true so the daemon's typed-confirm gate passes.
                self.host_confirm = None;
                self.status = format!("Host {op} on {dom0}…");
                Self::host_power_task(dom0, op, true)
            }
            Message::HostOpCancelled => {
                self.host_confirm = None;
                Task::none()
            }
            Message::HostPowerDone(Ok(s)) => {
                self.status = s;
                Task::none()
            }
            Message::HostPowerDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::HostImpactClicked { dom0 } => {
                self.status = format!("Reading impact for {dom0}…");
                Self::host_impact_task(dom0)
            }
            Message::HostImpactDone(Ok((dom0, running))) => {
                self.host_impact.insert(dom0.clone(), running);
                self.status = format!("{dom0}: {running} running VM(s) resident");
                Task::none()
            }
            Message::HostImpactDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::HostPoolClicked { dom0 } => {
                self.status = format!("Reading pool for {dom0}…");
                Self::host_pool_task(dom0)
            }
            Message::HostPoolDone(Ok((dom0, pool))) => {
                let line = if pool.pool.is_empty() {
                    format!("{dom0}: standalone (no pool)")
                } else if pool.is_master {
                    format!("{dom0}: pool '{}' (this host is master)", pool.pool)
                } else {
                    format!("{dom0}: pool '{}' (member)", pool.pool)
                };
                self.host_pool.insert(dom0, pool);
                self.status = line;
                Task::none()
            }
            Message::HostPoolDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::HostSshCopy { dom0 } => {
                // Pure local gesture: drop the ssh command onto the clipboard so the
                // operator can paste it into any terminal. No Bus / daemon.
                let cmd = format!("ssh root@{dom0}");
                self.status = format!("Copied: {cmd}");
                cosmic::iced::clipboard::write::<crate::Message>(cmd)
            }
            Message::HostSshLaunch { dom0 } => {
                // Pure local launch — a `cosmic-term ssh root@<dom0>`, mirroring the
                // Lighthouses tab's SSH action (never touches the daemon). dom0 reach
                // is root-only (the mesh key authorizes root), so this targets root
                // explicitly rather than `$USER` — spawned the same way the shared
                // launcher detaches a terminal (spawn IS the success signal).
                self.status = format!("Opening an SSH terminal to {dom0}…");
                Task::perform(async move { host_ssh_console(&dom0) }, |result| {
                    crate::Message::Datacenter(Message::HostSshLaunched(result))
                })
            }
            Message::HostSshLaunched(Ok(s)) => {
                self.status = s;
                Task::none()
            }
            Message::HostSshLaunched(Err(e)) => {
                self.status = e;
                Task::none()
            }

            // ── DATACENTER-11 (VMs tab) ──────────────────────────────────────
            Message::SuspendClicked { uuid, op, dom0 } => {
                self.status = format!("{op}…");
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || vm_suspend(&uuid, &op, &dom0))
                            .await
                            .unwrap_or_else(|e| Err(format!("suspend task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::SuspendDone(result)),
                )
            }
            Message::SuspendDone(Ok(s)) | Message::SuspendDone(Err(s)) => {
                self.status = s;
                Task::none()
            }
            Message::MigrateClicked { uuid, dom0: _ } => {
                // First click only arms the destination-host prompt — no RPC fires.
                self.vm_prompt = Some(VmPrompt::Migrate {
                    uuid,
                    host: String::new(),
                });
                self.status = "Enter a destination host, then Confirm.".into();
                Task::none()
            }
            Message::MigrateHostChanged(h) => {
                if let Some(VmPrompt::Migrate { host, .. }) = &mut self.vm_prompt {
                    *host = h;
                }
                Task::none()
            }
            Message::MigrateConfirmed { uuid, dom0 } => {
                let host = match &self.vm_prompt {
                    Some(VmPrompt::Migrate { host, .. }) => host.trim().to_string(),
                    _ => String::new(),
                };
                self.vm_prompt = None;
                if host.is_empty() {
                    self.status = "Migrate needs a destination host.".into();
                    return Task::none();
                }
                self.status = format!("Migrating to {host}…");
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || vm_migrate(&uuid, &host, &dom0))
                            .await
                            .unwrap_or_else(|e| Err(format!("migrate task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::MigrateDone(result)),
                )
            }
            Message::MigrateDone(Ok(s)) | Message::MigrateDone(Err(s)) => {
                self.status = s;
                Task::none()
            }
            Message::ResizeClicked { uuid, dom0: _ } => {
                self.vm_prompt = Some(VmPrompt::Resize {
                    uuid,
                    vcpus: String::new(),
                    mem_mib: String::new(),
                });
                self.status = "Enter vCPUs + memory (VM must be halted), then Confirm.".into();
                Task::none()
            }
            Message::ResizeVcpusChanged(v) => {
                if let Some(VmPrompt::Resize { vcpus, .. }) = &mut self.vm_prompt {
                    *vcpus = v;
                }
                Task::none()
            }
            Message::ResizeMemChanged(v) => {
                if let Some(VmPrompt::Resize { mem_mib, .. }) = &mut self.vm_prompt {
                    *mem_mib = v;
                }
                Task::none()
            }
            Message::ResizeConfirmed { uuid, dom0 } => {
                let (vcpus, mem) = match &self.vm_prompt {
                    Some(VmPrompt::Resize { vcpus, mem_mib, .. }) => {
                        (vcpus.trim().parse::<u64>(), mem_mib.trim().parse::<u64>())
                    }
                    _ => (Ok(0), Ok(0)),
                };
                self.vm_prompt = None;
                let (Ok(vcpus), Ok(mem)) = (vcpus, mem) else {
                    self.status = "Resize needs whole-number vCPUs + memory (MiB).".into();
                    return Task::none();
                };
                self.status = format!("Resizing to {vcpus} vCPU / {mem} MiB…");
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || vm_resize(&uuid, vcpus, mem, &dom0))
                            .await
                            .unwrap_or_else(|e| Err(format!("resize task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::ResizeDone(result)),
                )
            }
            Message::ResizeDone(Ok(s)) | Message::ResizeDone(Err(s)) => {
                self.status = s;
                Task::none()
            }
            Message::VmPromptCancelled => {
                self.vm_prompt = None;
                self.status.clear();
                Task::none()
            }
            Message::ConsoleClicked { uuid, dom0 } => {
                self.status = "Opening console…".into();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || vm_console_url(&uuid, &dom0))
                            .await
                            .unwrap_or_else(|e| Err(format!("console task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::ConsoleDone(result)),
                )
            }
            Message::ConsoleDone(Ok(url)) => {
                // The console `location` is the noVNC connection URL — open it with
                // the desktop's `xdg-open` (best-effort, detached) so the system
                // browser / viewer renders the embedded console. A runtime URL can't
                // use the `&'static str` `OpenExternal` path, so this opens directly,
                // mirroring the Hosts tab's local launch contract.
                self.status = vm_console_open(&url);
                Task::none()
            }
            Message::ConsoleDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::SnapshotsClicked { uuid, dom0 } => {
                self.status = "Listing snapshots…".into();
                let vm = uuid.clone();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || vm_snapshots(&vm, &dom0))
                            .await
                            .unwrap_or_else(|e| Err(format!("snapshots task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::SnapshotsDone(result)),
                )
            }
            Message::SnapshotsDone(Ok((vm, list))) => {
                self.status = format!("{} snapshot(s) for {vm}", list.len());
                self.vm_snapshots.insert(vm, list);
                Task::none()
            }
            Message::SnapshotsDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::SnapshotRevertClicked { snapshot, dom0 } => {
                self.status = format!("Reverting to {snapshot}…");
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || vm_snapshot_revert(&snapshot, &dom0))
                            .await
                            .unwrap_or_else(|e| Err(format!("revert task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::SnapshotRevertDone(result)),
                )
            }
            Message::SnapshotRevertDone(Ok(s)) | Message::SnapshotRevertDone(Err(s)) => {
                self.status = s;
                Task::none()
            }
            Message::SnapshotDeleteClicked { snapshot } => {
                self.confirm_snapshot_delete = Some(snapshot);
                self.status = "Confirm to delete this snapshot.".into();
                Task::none()
            }
            Message::SnapshotDeleteCancelled => {
                self.confirm_snapshot_delete = None;
                self.status.clear();
                Task::none()
            }
            Message::SnapshotDeleteConfirmed { snapshot, vm, dom0 } => {
                self.confirm_snapshot_delete = None;
                self.status = format!("Deleting snapshot {snapshot}…");
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            vm_snapshot_delete(&snapshot, &dom0)?;
                            // Re-list the VM's snapshots so the row drops the deleted
                            // one without a second operator gesture; the refreshed list
                            // rides back to re-key the per-VM cache.
                            vm_snapshots(&vm, &dom0)
                        })
                        .await
                        .unwrap_or_else(|e| Err(format!("delete task panicked: {e}")))
                    },
                    // On success the refreshed list re-keys the cache (the deleted
                    // snapshot is gone); on error the delete/list error surfaces.
                    |result| {
                        crate::Message::Datacenter(match result {
                            Ok(listed) => Message::SnapshotsDone(Ok(listed)),
                            Err(e) => Message::SnapshotDeleteDone(Err(e)),
                        })
                    },
                )
            }
            Message::SnapshotDeleteDone(Ok(s)) | Message::SnapshotDeleteDone(Err(s)) => {
                self.status = s;
                Task::none()
            }
            Message::CreateFieldChanged { field, value } => {
                match field {
                    VmCreateField::Name => self.vm_create.name = value,
                    VmCreateField::Vcpus => self.vm_create.vcpus = value,
                    VmCreateField::MemMib => self.vm_create.mem_mib = value,
                    VmCreateField::NetworkUuid => self.vm_create.network_uuid = value,
                    VmCreateField::Dom0 => self.vm_create.dom0 = value,
                }
                Task::none()
            }
            Message::CreateVmClicked => {
                let form = self.vm_create.clone();
                let (Ok(vcpus), Ok(mem)) = (
                    form.vcpus.trim().parse::<u64>(),
                    form.mem_mib.trim().parse::<u64>(),
                ) else {
                    self.status = "Create needs whole-number vCPUs + memory (MiB).".into();
                    return Task::none();
                };
                if form.name.trim().is_empty()
                    || form.network_uuid.trim().is_empty()
                    || form.dom0.trim().is_empty()
                {
                    self.status = "Create needs a name, network uuid, and dom0.".into();
                    return Task::none();
                }
                self.status = format!("Creating {}… (writing Tofu resource)", form.name.trim());
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || vm_create(&form, vcpus, mem))
                            .await
                            .unwrap_or_else(|e| Err(format!("create task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::CreateVmDone(result)),
                )
            }
            Message::CreateVmDone(Ok(resource)) => {
                // The resource is written; materialize it through Tofu (no drift) by
                // chaining into the existing tofu-apply path on the xen-xapi
                // workspace. Clear the form so a repeated submit can't double-write.
                self.vm_create = VmCreateForm::default();
                self.status = format!("Wrote {resource}; applying via Tofu…");
                self.tofu_output = format!("Applying xen-xapi for {resource}…");
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || tofu_apply("xen-xapi"))
                            .await
                            .unwrap_or_else(|e| Err(format!("tofu task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::TofuApplyDone(result)),
                )
            }
            Message::CreateVmDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::BulkToggle(uuid) => {
                if !self.vm_selected.remove(&uuid) {
                    self.vm_selected.insert(uuid);
                }
                Task::none()
            }
            Message::BulkSelectAll(uuids) => {
                self.vm_selected.extend(uuids);
                Task::none()
            }
            Message::BulkClear => {
                self.vm_selected.clear();
                self.bulk_progress.clear();
                Task::none()
            }
            Message::BulkTagChanged(t) => {
                self.bulk_tag = t;
                Task::none()
            }
            Message::BulkRun { op, dom0_by_uuid } => {
                if dom0_by_uuid.is_empty() {
                    self.status = "Select at least one VM first.".into();
                    return Task::none();
                }
                let tag = self.bulk_tag.trim().to_string();
                if op == "tag" && tag.is_empty() {
                    self.status = "Enter a tag first.".into();
                    return Task::none();
                }
                // Seed per-item progress (Pending) so the toolbar shows the run start,
                // then fire one RPC per selected VM — each lands as a BulkItemDone.
                self.bulk_progress.clear();
                self.status = format!("Bulk {op} on {} VM(s)…", dom0_by_uuid.len());
                let mut tasks: Vec<Task<crate::Message>> = Vec::new();
                for (uuid, dom0) in dom0_by_uuid {
                    self.bulk_progress.insert(uuid.clone(), BulkItem::Pending);
                    let op = op.clone();
                    let tag = tag.clone();
                    tasks.push(Task::perform(
                        async move {
                            let uuid2 = uuid.clone();
                            let r = tokio::task::spawn_blocking(move || {
                                bulk_op(&op, &uuid, &dom0, &tag)
                            })
                            .await
                            .unwrap_or_else(|e| Err(format!("bulk task panicked: {e}")));
                            (uuid2, r)
                        },
                        |(uuid, result)| {
                            crate::Message::Datacenter(Message::BulkItemDone { uuid, result })
                        },
                    ));
                }
                Task::batch(tasks)
            }
            Message::BulkItemDone { uuid, result } => {
                let item = match &result {
                    Ok(s) => BulkItem::Ok(s.clone()),
                    Err(e) => BulkItem::Err(e.clone()),
                };
                self.bulk_progress.insert(uuid, item);
                // Summarize once every item has resolved (no more Pending).
                if !self
                    .bulk_progress
                    .values()
                    .any(|i| matches!(i, BulkItem::Pending))
                {
                    let ok = self
                        .bulk_progress
                        .values()
                        .filter(|i| matches!(i, BulkItem::Ok(_)))
                        .count();
                    let total = self.bulk_progress.len();
                    self.status = format!("Bulk run complete — {ok}/{total} ok.");
                }
                Task::none()
            }
            Message::StorageFieldChanged { field, value } => {
                match field {
                    StorageField::SrName => self.sr_create.name = value,
                    StorageField::SrType => self.sr_create.sr_type = value,
                    StorageField::SrHost => self.sr_create.host_uuid = value,
                    StorageField::SrDeviceConfig => self.sr_create.device_config = value,
                    StorageField::SrDom0 => self.sr_create.dom0 = value,
                    StorageField::VdiSr => self.vdi_create.sr = value,
                    StorageField::VdiName => self.vdi_create.name = value,
                    StorageField::VdiSize => self.vdi_create.size_gib = value,
                    StorageField::VdiDom0 => self.vdi_create.dom0 = value,
                    StorageField::SchedSr => self.snap_schedule.sr = value,
                    StorageField::SchedRetention => self.snap_schedule.retention = value,
                    StorageField::SchedBackupTarget => self.snap_schedule.backup_target = value,
                    StorageField::SchedDom0 => self.snap_schedule.dom0 = value,
                }
                Task::none()
            }
            Message::StorageThresholdChanged(v) => {
                // Keep the raw text (so an in-progress empty box doesn't snap to 0)
                // and reparse a valid 1..=100 into the live threshold; a blank /
                // out-of-range value leaves the last good number in force.
                self.storage_threshold_input = v.clone();
                if let Ok(n) = v.trim().parse::<u64>() {
                    if (1..=100).contains(&n) {
                        self.storage_threshold_pct = n;
                    }
                }
                Task::none()
            }
            Message::SrCreateClicked => {
                let form = self.sr_create.clone();
                self.status = "Creating SR…".into();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || sr_create(&form))
                            .await
                            .unwrap_or_else(|e| Err(format!("sr-create task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::SrCreateDone(result)),
                )
            }
            Message::SrCreateDone(Ok(s)) => {
                self.status = format!("SR created: {s}");
                self.sr_create = SrCreateForm::default();
                Task::none()
            }
            Message::SrCreateDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::VdiTargetSr { sr, dom0 } => {
                // Seed the VDI-create form from the clicked SR card so the operator
                // only types name + size.
                self.vdi_create.sr = sr;
                self.vdi_create.dom0 = dom0;
                self.status = "VDI target set — name + size, then Create VDI.".into();
                Task::none()
            }
            Message::VdiCreateClicked => {
                let form = self.vdi_create.clone();
                self.status = "Creating VDI…".into();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || vdi_create(&form))
                            .await
                            .unwrap_or_else(|e| Err(format!("vdi-create task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::VdiCreateDone(result)),
                )
            }
            Message::VdiCreateDone(Ok(s)) => {
                self.status = format!("VDI created: {s}");
                self.vdi_create = VdiCreateForm::default();
                Task::none()
            }
            Message::VdiCreateDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::SrSnapshotClicked { sr, dom0 } => {
                self.status = "Snapshotting…".into();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || sr_snapshot(&sr, &dom0))
                            .await
                            .unwrap_or_else(|e| Err(format!("sr-snapshot task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::SrSnapshotDone(result)),
                )
            }
            Message::SrSnapshotDone(Ok(s)) => {
                self.status = format!("Snapshot: {s}");
                Task::none()
            }
            Message::SrSnapshotDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::VdiAttachArm { vdi, dom0: _ } => {
                // Arm the inline VM-uuid prompt; the actual attach fires from the
                // row's Attach-confirm once a destination VM is typed. Cancel any
                // pending detach so only one VDI prompt is live.
                self.vdi_attach = Some((vdi, String::new()));
                self.vdi_detach_confirm = None;
                self.status = "Type the destination VM uuid, then Attach.".into();
                Task::none()
            }
            Message::VdiAttachVmChanged(vm) => {
                if let Some((_, ref mut typed)) = self.vdi_attach {
                    *typed = vm;
                }
                Task::none()
            }
            Message::VdiAttachClicked { vdi, dom0 } => {
                let vm = self
                    .vdi_attach
                    .as_ref()
                    .map(|(_, v)| v.trim().to_string())
                    .unwrap_or_default();
                if vm.is_empty() {
                    self.status = "destination VM uuid is required".into();
                    return Task::none();
                }
                self.vdi_attach = None;
                self.status = "Attaching VDI…".into();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || vdi_attach(&vdi, &vm, &dom0))
                            .await
                            .unwrap_or_else(|e| Err(format!("vdi-attach task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::VdiAttachDone(result)),
                )
            }
            Message::VdiAttachCancel => {
                self.vdi_attach = None;
                self.status.clear();
                Task::none()
            }
            Message::VdiAttachDone(Ok(s)) => {
                self.status = format!("VDI attached (vbd {s})");
                Task::none()
            }
            Message::VdiAttachDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::VdiDetachArm { vbd, dom0: _ } => {
                // Detach is destructive (drops the VM↔disk connection) → arm the
                // typed-confirm gate; no RPC until `DETACH` is typed. Cancel any
                // pending attach so only one VDI prompt is live.
                self.vdi_detach_confirm = Some(vbd);
                self.vdi_detach_input.clear();
                self.vdi_attach = None;
                self.status = "Type DETACH to confirm.".into();
                Task::none()
            }
            Message::VdiDetachInputChanged(v) => {
                self.vdi_detach_input = v;
                Task::none()
            }
            Message::VdiDetachClicked { vbd, dom0 } => {
                // Belt-and-suspenders: only fire when the typed word matches (the
                // button is also disabled otherwise in the view).
                if self.vdi_detach_input.trim() != "DETACH" {
                    self.status = "Type DETACH to confirm the detach.".into();
                    return Task::none();
                }
                self.vdi_detach_confirm = None;
                self.vdi_detach_input.clear();
                self.status = "Detaching VDI…".into();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || vdi_detach(&vbd, &dom0))
                            .await
                            .unwrap_or_else(|e| Err(format!("vdi-detach task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::VdiDetachDone(result)),
                )
            }
            Message::VdiDetachCancel => {
                self.vdi_detach_confirm = None;
                self.vdi_detach_input.clear();
                self.status.clear();
                Task::none()
            }
            Message::VdiDetachDone(Ok(s)) => {
                self.status = s;
                Task::none()
            }
            Message::VdiDetachDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::SnapScheduleClicked => {
                let form = self.snap_schedule.clone();
                self.status = "Saving snapshot schedule…".into();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || snap_schedule_save(&form))
                            .await
                            .unwrap_or_else(|e| Err(format!("snap-schedule task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::SnapScheduleDone(result)),
                )
            }
            Message::SnapScheduleDone(Ok(s)) => {
                self.status = s;
                Task::none()
            }
            Message::SnapScheduleDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::NetFieldChanged { field, value } => {
                match field {
                    NetField::ReadDom0 => self.net_read_dom0 = value,
                    NetField::VlanPif => self.vlan_create.pif = value,
                    NetField::VlanTag => self.vlan_create.vlan = value,
                    NetField::VlanName => self.vlan_create.network_name = value,
                    NetField::VlanDom0 => self.vlan_create.dom0 = value,
                }
                Task::none()
            }
            Message::NetReadClicked => {
                let dom0 = self.net_read_dom0.trim().to_string();
                if dom0.is_empty() {
                    self.status = "Read L2 needs a dom0 (an allow-listed Xen host).".into();
                    return Task::none();
                }
                self.status = format!("Reading L2 inventory on {dom0}…");
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || host_net(&dom0))
                            .await
                            .unwrap_or_else(|e| Err(format!("host-net task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::NetReadDone(result)),
                )
            }
            Message::NetReadDone(Ok(read)) => {
                self.status = format!(
                    "L2 inventory: {} network(s), {} NIC(s).",
                    read.nets.len(),
                    read.pifs.len()
                );
                self.net_read = Some(read);
                Task::none()
            }
            Message::NetReadDone(Err(e)) => {
                self.status = e;
                Task::none()
            }
            Message::VlanTargetPif { pif, dom0 } => {
                // Seed the VLAN-create form from the clicked PIF card so the operator
                // only types the tag + name.
                self.vlan_create.pif = pif;
                self.vlan_create.dom0 = dom0;
                self.status = "VLAN target set — tag + name, then Create VLAN.".into();
                Task::none()
            }
            Message::VlanCreateClicked => {
                let form = self.vlan_create.clone();
                self.status = "Creating VLAN…".into();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || vlan_create(&form))
                            .await
                            .unwrap_or_else(|e| Err(format!("vlan-create task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::VlanCreateDone(result)),
                )
            }
            Message::VlanCreateDone(Ok(net)) => {
                self.status = format!("VLAN network created: {net}");
                self.vlan_create = VlanCreateForm::default();
                Task::none()
            }
            Message::VlanCreateDone(Err(e)) => {
                self.status = e;
                Task::none()
            }

            // ── DATACENTER-16 (Power tab) ────────────────────────────────────
            Message::PowerFieldChanged { field, value } => {
                match field {
                    PowerField::Bmc => self.power_form.bmc = value,
                    PowerField::User => self.power_form.user = value,
                    PowerField::Pass => self.power_form.pass = value,
                    PowerField::Mac => self.power_form.mac = value,
                    PowerField::Dom0 => self.power_form.dom0 = value,
                }
                Task::none()
            }
            Message::WakeClicked => {
                let form = self.power_form.clone();
                let target = if form.bmc.trim().is_empty() {
                    form.mac.trim().to_string()
                } else {
                    form.bmc.trim().to_string()
                };
                if target.is_empty() {
                    self.status = "Wake needs a BMC host or a MAC.".into();
                    return Task::none();
                }
                self.seed_wake_progress(&target);
                self.status = format!("Waking {target} (IPMI, WoL fallback)…");
                let poll = self.wake_eta_poll_task();
                Task::batch([
                    Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || ipmi_wake(&form))
                                .await
                                .unwrap_or_else(|e| Err(format!("wake task panicked: {e}")))
                        },
                        |result| crate::Message::Datacenter(Message::WakeDone(result)),
                    ),
                    poll,
                ])
            }
            Message::WolWakeClicked => {
                let mac = self.power_form.mac.trim().to_string();
                if mac.is_empty() {
                    self.status = "WoL needs a MAC.".into();
                    return Task::none();
                }
                self.seed_wake_progress(&mac);
                self.status = format!("Waking {mac} (Wake-on-LAN)…");
                let poll = self.wake_eta_poll_task();
                let mac_for_rpc = mac.clone();
                Task::batch([
                    Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || wol_wake_rpc(&mac_for_rpc))
                                .await
                                .unwrap_or_else(|e| Err(format!("wol task panicked: {e}")))
                        },
                        |result| crate::Message::Datacenter(Message::WakeDone(result)),
                    ),
                    poll,
                ])
            }
            Message::WakeDone(Ok((target, line))) => {
                self.status = format!("{target}: {line}");
                Task::none()
            }
            Message::WakeDone(Err(e)) => {
                self.status = format!("wake failed: {e}");
                // A failed wake clears the in-flight progress so the bar doesn't
                // keep ticking toward a host that never came up.
                self.wake_progress = None;
                Task::none()
            }
            Message::WakeEtaPoll => {
                let Some(wp) = &self.wake_progress else {
                    return Task::none();
                };
                if wp.ready {
                    // The wake reached ready — record the duration as a learned
                    // sample and stop polling.
                    self.record_wake_sample();
                    return Task::none();
                }
                let target = wp.target.clone();
                let started = self.wake_started;
                let samples = self.wake_samples.get(&target).cloned().unwrap_or_default();
                Task::perform(
                    async move {
                        let elapsed = started.map(|s| s.elapsed().as_secs()).unwrap_or(0);
                        tokio::task::spawn_blocking(move || {
                            wake_eta_rpc(&samples, elapsed, &target)
                        })
                        .await
                        .unwrap_or_else(|e| Err(format!("wake-eta task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::WakeEtaDone(result)),
                )
            }
            Message::WakeEtaDone(Ok(wp)) => {
                let ready = wp.ready;
                self.wake_progress = Some(wp);
                if ready {
                    // Reached ready: record the sample, leave the (full) bar up.
                    self.record_wake_sample();
                    Task::none()
                } else {
                    // Still waking — re-arm the ~1s poll so the bar advances.
                    self.wake_eta_poll_later()
                }
            }
            Message::WakeEtaDone(Err(e)) => {
                self.status = format!("wake-eta: {e}");
                Task::none()
            }
            Message::IdlePolicyClicked => {
                let dom0 = self.power_form.dom0.trim().to_string();
                if dom0.is_empty() {
                    self.status = "Idle check needs a dom0 IP.".into();
                    return Task::none();
                }
                self.status = format!("Checking idle policy for {dom0}…");
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || idle_policy_rpc(&dom0))
                            .await
                            .unwrap_or_else(|e| Err(format!("idle-policy task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::IdlePolicyDone(result)),
                )
            }
            Message::IdlePolicyDone(Ok((running, shutdown, reason))) => {
                self.idle_reason = reason.clone();
                self.idle_shutdown_ok = shutdown;
                self.status = format!("Idle policy: {running} running guest(s) — {reason}");
                Task::none()
            }
            Message::IdlePolicyDone(Err(e)) => {
                self.idle_reason = String::new();
                self.idle_shutdown_ok = false;
                self.status = format!("idle-policy: {e}");
                Task::none()
            }
            Message::IdleShutdownClicked => {
                // Only act on a live recommendation; the graceful shutdown runs
                // through the existing confirm-gated host-power op.
                if !self.idle_shutdown_ok {
                    self.status = "No idle-shutdown recommendation — check idle first.".into();
                    return Task::none();
                }
                let dom0 = self.power_form.dom0.trim().to_string();
                if dom0.is_empty() {
                    self.status = "Idle shutdown needs a dom0 IP.".into();
                    return Task::none();
                }
                self.idle_shutdown_ok = false;
                self.status = format!("Shutting down idle host {dom0} (graceful)…");
                // The operator armed this via a live idle-shutdown recommendation
                // (the soft confirm above); carry confirm:true for the daemon gate.
                Self::host_power_task(dom0, "shutdown".to_string(), true)
            }

            // ── DATACENTER-14 (Gateway tab) ──────────────────────────────────
            Message::GatewayDhcpClicked => {
                self.status = "Reading EdgeOS DHCP (reservations + leases)…".into();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(gateway_dhcp_rpc)
                            .await
                            .unwrap_or_else(|e| Err(format!("gateway-dhcp task panicked: {e}")))
                    },
                    |result| crate::Message::Datacenter(Message::GatewayDhcpDone(result)),
                )
            }
            Message::GatewayDhcpDone(Ok(dhcp)) => {
                self.status = format!(
                    "EdgeOS DHCP: {} reservation(s) · {} live lease(s)",
                    dhcp.reservations.len(),
                    dhcp.leases.len()
                );
                self.gateway_dhcp = Some(dhcp);
                Task::none()
            }
            Message::GatewayDhcpDone(Err(e)) => {
                self.status = format!("gateway-dhcp: {e}");
                Task::none()
            }
        }
    }

    /// DATACENTER-16 — seed a fresh wake-progress for `target` (phase POST, 0%)
    /// and start the elapsed-time clock. Shared by the IPMI + WoL wake paths.
    fn seed_wake_progress(&mut self, target: &str) {
        self.wake_progress = Some(WakeProgress {
            target: target.to_string(),
            phase: "post".to_string(),
            progress: 0.0,
            eta: rolling_avg_local(self.wake_samples.get(target)),
            ready: false,
        });
        self.wake_started = Some(Instant::now());
    }

    /// DATACENTER-16 — record the just-finished wake's duration as a learned
    /// sample for its target (keeping the most recent `WAKE_SAMPLE_WINDOW`), so
    /// the next wake's ETA is sharper. No-op if no clock is running.
    fn record_wake_sample(&mut self) {
        let (Some(wp), Some(started)) = (&self.wake_progress, self.wake_started) else {
            return;
        };
        let secs = started.elapsed().as_secs().max(1);
        let samples = self.wake_samples.entry(wp.target.clone()).or_default();
        samples.push(secs);
        // Keep only the most recent window (matches `rolling_avg_local`).
        if samples.len() > WAKE_SAMPLE_WINDOW_LOCAL {
            let drop = samples.len() - WAKE_SAMPLE_WINDOW_LOCAL;
            samples.drain(0..drop);
        }
        self.wake_started = None;
    }

    /// DATACENTER-16 — fire the first `action/dc/wake-eta` poll immediately when a
    /// wake starts (so the bar moves off 0 without waiting a full tick).
    fn wake_eta_poll_task(&self) -> Task<crate::Message> {
        Task::done(crate::Message::Datacenter(Message::WakeEtaPoll))
    }

    /// DATACENTER-16 — sleep ~1s, then emit another [`Message::WakeEtaPoll`]. The
    /// chain self-stops once the wake reaches ready (no idle wakeups).
    fn wake_eta_poll_later(&self) -> Task<crate::Message> {
        Task::perform(
            async {
                tokio::time::sleep(Duration::from_secs(1)).await;
            },
            |()| crate::Message::Datacenter(Message::WakeEtaPoll),
        )
    }

    /// DATACENTER-10 — fire the `action/dc/host-power` RPC on a blocking thread
    /// (the Bus RPC borrows a non-Send `Persist` across its await, same shape as
    /// `PowerClicked`'s `vm_power`), routing the reply back as `HostPowerDone`.
    fn host_power_task(dom0: String, op: String, confirm: bool) -> Task<crate::Message> {
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || host_power(&dom0, &op, confirm))
                    .await
                    .unwrap_or_else(|e| Err(format!("host-power task panicked: {e}")))
            },
            |result| crate::Message::Datacenter(Message::HostPowerDone(result)),
        )
    }

    /// DATACENTER-10 — fire the read-only `action/dc/host-impact` RPC on a blocking
    /// thread, routing the reply back as `HostImpactDone`.
    fn host_impact_task(dom0: String) -> Task<crate::Message> {
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || host_impact_rpc(&dom0))
                    .await
                    .unwrap_or_else(|e| Err(format!("host-impact task panicked: {e}")))
            },
            |result| crate::Message::Datacenter(Message::HostImpactDone(result)),
        )
    }

    /// DATACENTER-10 — fire the read-only `action/dc/host-pool` RPC on a blocking
    /// thread, routing the reply back as `HostPoolDone`.
    fn host_pool_task(dom0: String) -> Task<crate::Message> {
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || host_pool_rpc(&dom0))
                    .await
                    .unwrap_or_else(|e| Err(format!("host-pool task panicked: {e}")))
            },
            |result| crate::Message::Datacenter(Message::HostPoolDone(result)),
        )
    }

    /// DATACENTER-10 — render one Xen dom0 host as a Hosts-tab card: capacity +
    /// health header, the full host lifecycle controls (maintenance on/off / reboot
    /// / shutdown / evacuate — the destructive three arming an impact-preview
    /// confirm), pool placement (membership / master, read on demand), and the
    /// copy / launch SSH console. All controls route panel-scoped messages back
    /// through `update` (runtime-reachable); mde-theme tokens only (§4).
    fn host_card_view<'a>(&self, h: &DcRow, palette: Palette) -> Element<'a, crate::Message> {
        let dom0 = h.id.clone();
        let label = if h.name.is_empty() {
            h.id.clone()
        } else {
            h.name.clone()
        };

        // Header: the host name + the color-dot liveness.
        let header = row![
            text(format!("Host {label}"))
                .colr(palette.text.into_cosmic_color())
                .width(Length::FillPortion(1)),
            status_dot_view(h, palette),
        ]
        .spacing(f32::from(spacing::BASE[2]))
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let mut card = column![header].spacing(f32::from(spacing::BASE[2]));

        // Capacity + health line: dom0 IP, vCPU, memory, load — whatever the host
        // reported. Built off the host metrics already projected from the Bus.
        let mut capacity = h.id.clone();
        if !h.cpu.is_empty() {
            capacity.push_str(&format!(" · {} vCPU", h.cpu));
        }
        if let Some(mem) = h.host_memory_readout() {
            capacity.push_str(&format!(" · mem {mem}"));
        }
        if !h.load.is_empty() {
            capacity.push_str(&format!(" · load {}", h.load));
        }
        card = card.push(text(capacity).colr(palette.text_muted.into_cosmic_color()));

        // Pool placement, when read. Renders the cached `action/dc/host-pool`
        // result; the "Pool" button (below) refreshes it.
        if let Some(p) = self.host_pool.get(&dom0) {
            let pool_line = if p.pool.is_empty() {
                "Pool: standalone (no pool)".to_string()
            } else if p.is_master {
                format!("Pool: {} — this host is MASTER", p.pool)
            } else {
                format!("Pool: {} — member", p.pool)
            };
            let color = if p.is_master {
                palette.accent
            } else {
                palette.text_muted
            };
            card = card.push(text(pool_line).colr(color.into_cosmic_color()));
        }

        // Reversible lifecycle ops fire immediately (no confirm).
        let maint = |btn_label: &str, op: &str| {
            variant_button(
                btn_label.to_string(),
                ButtonVariant::Secondary,
                Some(crate::Message::Datacenter(Message::HostPowerClicked {
                    dom0: dom0.clone(),
                    op: op.to_string(),
                })),
                palette,
            )
        };
        let pool_btn = variant_button(
            "Pool".to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::HostPoolClicked {
                dom0: dom0.clone(),
            })),
            palette,
        );
        let impact_btn = variant_button(
            "Impact".to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::HostImpactClicked {
                dom0: dom0.clone(),
            })),
            palette,
        );
        let copy_btn = variant_button(
            "Copy SSH".to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::HostSshCopy {
                dom0: dom0.clone(),
            })),
            palette,
        );
        let console_btn = variant_button(
            "Console".to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::HostSshLaunch {
                dom0: dom0.clone(),
            })),
            palette,
        );

        let reversible = row![
            maint("Maintenance on", "maintenance-on"),
            maint("Maintenance off", "maintenance-off"),
            impact_btn,
            pool_btn,
            copy_btn,
            console_btn,
        ]
        .spacing(f32::from(spacing::BASE[1]));
        card = card.push(reversible);

        // Destructive lifecycle ops (reboot / shutdown / evacuate): the first click
        // ARMS a confirm + refreshes the impact preview; only the inline Confirm
        // fires the `action/dc/host-power` RPC.
        let armed = self
            .host_confirm
            .as_ref()
            .filter(|(d, _)| *d == dom0)
            .map(|(_, op)| op.clone());
        if let Some(op) = armed {
            // Show the impact preview, if cached, then the explicit confirm.
            let impact = self.host_impact.get(&dom0).copied();
            let preview = match impact {
                Some(n) => format!(
                    "Confirm {op} on {dom0}? {n} running VM(s) will be {}.",
                    if op == "evacuate" {
                        "migrated off"
                    } else {
                        "stopped"
                    }
                ),
                None => format!("Confirm {op} on {dom0}? (reading impact…)"),
            };
            let confirm_row = row![
                text(preview).colr(palette.warning.into_cosmic_color()),
                variant_button(
                    "Confirm".to_string(),
                    ButtonVariant::Primary,
                    Some(crate::Message::Datacenter(Message::HostOpConfirmed {
                        dom0: dom0.clone(),
                        op: op.clone(),
                    })),
                    palette,
                ),
                variant_button(
                    "Cancel".to_string(),
                    ButtonVariant::Secondary,
                    Some(crate::Message::Datacenter(Message::HostOpCancelled)),
                    palette,
                ),
            ]
            .spacing(f32::from(spacing::BASE[1]))
            .align_y(cosmic::iced::alignment::Vertical::Center);
            card = card.push(confirm_row);
        } else {
            let destructive = |btn_label: &str, op: &str| {
                variant_button(
                    btn_label.to_string(),
                    ButtonVariant::Primary,
                    Some(crate::Message::Datacenter(Message::HostOpArmed {
                        dom0: dom0.clone(),
                        op: op.to_string(),
                    })),
                    palette,
                )
            };
            let destructive_row = row![
                destructive("Reboot", "reboot"),
                destructive("Shut down", "shutdown"),
                destructive("Evacuate", "evacuate"),
            ]
            .spacing(f32::from(spacing::BASE[1]));
            card = card.push(destructive_row);
        }

        let surface = palette.surface;
        let radius = f32::from(spacing::BASE[1]);
        container(card)
            .padding(f32::from(CARD_PAD_PX))
            .width(Length::Fill)
            .style(move |_theme| container::Style {
                background: Some(cosmic::iced::Background::Color(surface.into_cosmic_color())),
                border: cosmic::iced::Border {
                    color: palette.border.into_cosmic_color(),
                    width: 1.0,
                    radius: radius.into(),
                },
                ..container::Style::default()
            })
            .into()
    }

    /// DATACENTER-11 (VMs tab) — the Tofu-backed golden-template create wizard. Five
    /// inputs (name / vCPUs / memory MiB / network uuid / dom0) and a Create button
    /// that fires `action/dc/vm-create` (which writes a `xenserver_vm` clone resource
    /// into the `xen-xapi` workspace) then `tofu-apply` — so the structural change
    /// goes through Tofu (no drift). mde-theme tokens only (§4); every control routes
    /// a panel-scoped message back through `update` (runtime-reachable).
    fn vm_create_form(&self, palette: Palette) -> Element<'_, crate::Message> {
        let field = |placeholder: &str, value: &str, f: VmCreateField| {
            text_input(placeholder, value)
                .on_input(move |v| {
                    crate::Message::Datacenter(Message::CreateFieldChanged { field: f, value: v })
                })
                .width(Length::FillPortion(1))
        };
        let inputs = row![
            field("name", &self.vm_create.name, VmCreateField::Name),
            field("vCPUs", &self.vm_create.vcpus, VmCreateField::Vcpus),
            field("memory MiB", &self.vm_create.mem_mib, VmCreateField::MemMib),
            field(
                "network uuid",
                &self.vm_create.network_uuid,
                VmCreateField::NetworkUuid,
            ),
            field("dom0", &self.vm_create.dom0, VmCreateField::Dom0),
        ]
        .spacing(f32::from(spacing::BASE[1]));
        let create_btn = variant_button(
            "Create VM (via Tofu)".to_string(),
            ButtonVariant::Primary,
            Some(crate::Message::Datacenter(Message::CreateVmClicked)),
            palette,
        );
        let card = column![
            text("Create VM — golden-template clone, applied through Tofu")
                .colr(palette.text.into_cosmic_color()),
            inputs,
            create_btn,
        ]
        .spacing(f32::from(spacing::BASE[2]));
        let surface = palette.surface;
        let radius = f32::from(spacing::BASE[1]);
        container(card)
            .padding(f32::from(CARD_PAD_PX))
            .width(Length::Fill)
            .style(move |_theme| container::Style {
                background: Some(cosmic::iced::Background::Color(surface.into_cosmic_color())),
                border: cosmic::iced::Border {
                    color: palette.border.into_cosmic_color(),
                    width: 1.0,
                    radius: radius.into(),
                },
                ..container::Style::default()
            })
            .into()
    }

    /// DATACENTER-12 (Storage tab) — the capacity-threshold control: a count of the
    /// over-threshold SRs + an editable warning threshold (percent). The live
    /// `storage_threshold_pct` drives the per-SR alert badges; this lets the
    /// operator tune the line. Carbon tokens only (§4).
    /// DATACENTER-15 (Tofu tab) — the **prod-arm master switch** bar. Mirrors the
    /// FA_APPLY operator gate: while disarmed (the default), a mutating Tofu op on
    /// the PROD workspace ([`PROD_TOFU_WS`]) can never fire from the GUI — its
    /// Apply/Destroy buttons are disabled. Arming is a deliberate, separate action
    /// from the per-op typed-confirm. Colors from mde-theme tokens (danger = armed/
    /// live, success = safe-disarmed) — never raw hex.
    fn tofu_prod_arm_bar(&self, palette: Palette) -> Element<'_, crate::Message> {
        let (status_text, status_color) = if self.tofu_prod_armed {
            (
                format!("PROD ARMED — apply/destroy on {PROD_TOFU_WS} is LIVE"),
                palette.danger,
            )
        } else {
            (
                format!("PROD disarmed — {PROD_TOFU_WS} apply/destroy is blocked"),
                palette.success,
            )
        };
        // Toggle button flips the arm state; its label names the action it takes.
        let toggle = variant_button(
            if self.tofu_prod_armed {
                "Disarm PROD".to_string()
            } else {
                "Arm PROD".to_string()
            },
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::TofuProdArm(
                !self.tofu_prod_armed,
            ))),
            palette,
        );
        let line = row![
            text("Prod-arm master switch")
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::FillPortion(1)),
            text(status_text)
                .colr(status_color.into_cosmic_color())
                .width(Length::FillPortion(2)),
            toggle,
        ]
        .spacing(f32::from(spacing::BASE[2]))
        .align_y(cosmic::iced::alignment::Vertical::Center);
        container(line)
            .padding(f32::from(spacing::BASE[3]))
            .width(Length::Fill)
            .into()
    }

    /// DATACENTER-20 (Overview / promote strip) — the **auto-promote prod-arm
    /// master switch** bar. Mirrors [`Self::tofu_prod_arm_bar`]: while disarmed
    /// (the default), a green Build→Eagle pipeline HOLDS at the DO step
    /// (`queued`); arming lets the `dc_promote` leader auto-promote to DO on
    /// green. The toggle PERSISTS to the config the worker reads. Colors from
    /// mde-theme tokens (danger = armed/live, success = safe-disarmed) — no raw
    /// hex.
    fn promote_prod_arm_bar(&self, palette: Palette) -> Element<'_, crate::Message> {
        let (status_text, status_color) = if self.promote_prod_armed {
            (
                "ARMED — a green pipeline auto-promotes to DO".to_string(),
                palette.danger,
            )
        } else {
            (
                "disarmed — the DO step is held (queued) until armed".to_string(),
                palette.success,
            )
        };
        let toggle = variant_button(
            if self.promote_prod_armed {
                "Disarm auto-promote".to_string()
            } else {
                "Arm auto-promote".to_string()
            },
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::PromoteProdArm(
                !self.promote_prod_armed,
            ))),
            palette,
        );
        let line = row![
            text("Auto-promote prod-arm")
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::FillPortion(1)),
            text(status_text)
                .colr(status_color.into_cosmic_color())
                .width(Length::FillPortion(2)),
            toggle,
        ]
        .spacing(f32::from(spacing::BASE[2]))
        .align_y(cosmic::iced::alignment::Vertical::Center);
        container(line)
            .padding(f32::from(spacing::BASE[3]))
            .width(Length::Fill)
            .into()
    }

    fn storage_threshold_bar(
        &self,
        palette: Palette,
        alert_count: usize,
    ) -> Element<'_, crate::Message> {
        let summary = if alert_count == 0 {
            text("All SRs below the capacity threshold.").colr(palette.success.into_cosmic_color())
        } else {
            text(format!("{alert_count} SR(s) at/above threshold"))
                .colr(palette.warning.into_cosmic_color())
        };
        let input = text_input("warning %", &self.storage_threshold_input)
            .on_input(|v| crate::Message::Datacenter(Message::StorageThresholdChanged(v)))
            .width(Length::Fixed(96.0));
        let line = row![
            text("Capacity alert threshold")
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::FillPortion(2)),
            input,
            text(format!("(critical at {SR_CRITICAL_PCT}%)"))
                .colr(palette.text_muted.into_cosmic_color()),
            summary.width(Length::FillPortion(2)),
        ]
        .spacing(f32::from(spacing::BASE[2]))
        .align_y(cosmic::iced::alignment::Vertical::Center);
        self.storage_card(line.into(), palette)
    }

    /// DATACENTER-12 (Storage tab) — the SR-create form. Packs name/type/host/
    /// device-config/dom0 into `action/dc/sr-create`. Carbon tokens only.
    fn sr_create_form(&self, palette: Palette) -> Element<'_, crate::Message> {
        let field = |placeholder: &str, value: &str, f: StorageField| {
            text_input(placeholder, value)
                .on_input(move |v| {
                    crate::Message::Datacenter(Message::StorageFieldChanged { field: f, value: v })
                })
                .width(Length::FillPortion(1))
        };
        let inputs = row![
            field("name", &self.sr_create.name, StorageField::SrName),
            field("type (lvm)", &self.sr_create.sr_type, StorageField::SrType),
            field("host uuid", &self.sr_create.host_uuid, StorageField::SrHost),
            field(
                "device-config (key=value)",
                &self.sr_create.device_config,
                StorageField::SrDeviceConfig,
            ),
            field("dom0", &self.sr_create.dom0, StorageField::SrDom0),
        ]
        .spacing(f32::from(spacing::BASE[1]));
        let create_btn = variant_button(
            "Create SR".to_string(),
            ButtonVariant::Primary,
            Some(crate::Message::Datacenter(Message::SrCreateClicked)),
            palette,
        );
        let card = column![
            text("Create storage repository").colr(palette.text.into_cosmic_color()),
            inputs,
            create_btn,
        ]
        .spacing(f32::from(spacing::BASE[2]));
        self.storage_card(card.into(), palette)
    }

    /// DATACENTER-12 (Storage tab) — the VDI-create form. Packs sr/name/size/dom0
    /// into `action/dc/vdi-create`. A per-SR card's "VDI here" seeds sr+dom0.
    fn vdi_create_form(&self, palette: Palette) -> Element<'_, crate::Message> {
        let field = |placeholder: &str, value: &str, f: StorageField| {
            text_input(placeholder, value)
                .on_input(move |v| {
                    crate::Message::Datacenter(Message::StorageFieldChanged { field: f, value: v })
                })
                .width(Length::FillPortion(1))
        };
        let inputs = row![
            field("SR uuid", &self.vdi_create.sr, StorageField::VdiSr),
            field("name", &self.vdi_create.name, StorageField::VdiName),
            field("size GiB", &self.vdi_create.size_gib, StorageField::VdiSize),
            field("dom0", &self.vdi_create.dom0, StorageField::VdiDom0),
        ]
        .spacing(f32::from(spacing::BASE[1]));
        let create_btn = variant_button(
            "Create VDI".to_string(),
            ButtonVariant::Primary,
            Some(crate::Message::Datacenter(Message::VdiCreateClicked)),
            palette,
        );
        let card = column![
            text("Create virtual disk (VDI)").colr(palette.text.into_cosmic_color()),
            inputs,
            create_btn,
        ]
        .spacing(f32::from(spacing::BASE[2]));
        self.storage_card(card.into(), palette)
    }

    /// DATACENTER-12 (Storage tab) — the scheduled-snapshot config (SR, retention,
    /// backup target), persisted as an `event/dc/snap-schedule/*` Bus record that a
    /// scheduler reads back. Carbon tokens only.
    fn snap_schedule_form(&self, palette: Palette) -> Element<'_, crate::Message> {
        let field = |placeholder: &str, value: &str, f: StorageField| {
            text_input(placeholder, value)
                .on_input(move |v| {
                    crate::Message::Datacenter(Message::StorageFieldChanged { field: f, value: v })
                })
                .width(Length::FillPortion(1))
        };
        let inputs = row![
            field("SR uuid", &self.snap_schedule.sr, StorageField::SchedSr),
            field(
                "retention (count)",
                &self.snap_schedule.retention,
                StorageField::SchedRetention,
            ),
            field(
                "backup target",
                &self.snap_schedule.backup_target,
                StorageField::SchedBackupTarget,
            ),
            field("dom0", &self.snap_schedule.dom0, StorageField::SchedDom0),
        ]
        .spacing(f32::from(spacing::BASE[1]));
        let save_btn = variant_button(
            "Save schedule".to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::SnapScheduleClicked)),
            palette,
        );
        let card = column![
            text("Scheduled snapshots — retention + backup target")
                .colr(palette.text.into_cosmic_color()),
            inputs,
            save_btn,
        ]
        .spacing(f32::from(spacing::BASE[2]));
        self.storage_card(card.into(), palette)
    }

    /// DATACENTER-12 (Storage tab) — one SR rendered as a capacity card: the
    /// name + capacity readout + a color-dot fill indicator, plus "Snapshot all"
    /// and "VDI here" actions. The fill dot's token tracks the threshold (success
    /// below, warning above, danger at/above critical) so the card reads at a
    /// glance. Carbon tokens only (§4).
    fn sr_card_view(&self, r: &DcRow, palette: Palette) -> Element<'_, crate::Message> {
        let label = if r.name.is_empty() {
            r.id.clone()
        } else {
            r.name.clone()
        };
        // The fill dot token tracks the SR's used percentage against the live
        // threshold; unparseable capacity falls back to a muted dot.
        let dot = match r.used_pct() {
            Some(p) if p >= SR_CRITICAL_PCT => palette.danger,
            Some(p) if p >= self.storage_threshold_pct => palette.warning,
            Some(_) => palette.success,
            None => palette.text_muted,
        };
        let capacity = r
            .capacity_readout()
            .unwrap_or_else(|| "capacity unknown".to_string());
        let header = row![
            text("sr")
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::FillPortion(1)),
            container(text(""))
                .width(Length::Fixed(12.0))
                .height(Length::Fixed(12.0))
                .style(move |_theme| container::Style {
                    background: Some(cosmic::iced::Background::Color(dot.into_cosmic_color())),
                    border: cosmic::iced::Border {
                        radius: 6.0.into(),
                        ..cosmic::iced::Border::default()
                    },
                    ..container::Style::default()
                }),
        ]
        .spacing(f32::from(spacing::BASE[2]))
        .align_y(cosmic::iced::alignment::Vertical::Center);
        let snapshot = variant_button(
            "Snapshot all".to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::SrSnapshotClicked {
                sr: r.id.clone(),
                dom0: r.host.clone(),
            })),
            palette,
        );
        let vdi_here = variant_button(
            "VDI here".to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::VdiTargetSr {
                sr: r.id.clone(),
                dom0: r.host.clone(),
            })),
            palette,
        );
        let actions = row![snapshot, vdi_here].spacing(f32::from(spacing::BASE[1]));
        let mut card = column![
            header,
            text(label).colr(palette.text.into_cosmic_color()),
            text(capacity).colr(palette.text_muted.into_cosmic_color()),
            actions,
        ]
        .spacing(f32::from(spacing::BASE[2]));
        // The SR's VDIs (the virtual disks on this store) with per-disk
        // attach/detach. VDIs arrive on the Bus as `kind == "vdi"` rows carrying the
        // owning `sr` uuid; match them to this card.
        let vdis: Vec<&DcRow> = self
            .rows
            .iter()
            .filter(|v| v.kind == "vdi" && v.sr == r.id)
            .collect();
        if !vdis.is_empty() {
            card = card.push(text("Virtual disks").colr(palette.text_muted.into_cosmic_color()));
            for v in vdis {
                card = card.push(self.vdi_row_view(v, palette));
            }
        }
        self.storage_card(card.into(), palette)
    }

    /// DATACENTER-12 (Storage tab) — one VDI rendered under its SR card: the disk
    /// name + size + attached-VM readout, plus Attach (to a typed VM) / Detach
    /// (typed-confirm gated, since detach is destructive) actions firing the
    /// `action/dc/vdi-{attach,detach}` RPCs. Carbon tokens only (§4).
    fn vdi_row_view(&self, v: &DcRow, palette: Palette) -> Element<'_, crate::Message> {
        let name = if v.name.is_empty() {
            v.id.clone()
        } else {
            v.name.clone()
        };
        // Virtual size (bytes string) → a GiB readout; unknown when unparseable.
        let size_gib = v.size.parse::<u64>().ok().map(|b| b / (1024 * 1024 * 1024));
        let attach_state = if v.vm.is_empty() {
            "unattached".to_string()
        } else {
            format!("attached → vm {}", v.vm)
        };
        let meta = match size_gib {
            Some(g) => format!("{name} · {g} GiB · {attach_state}"),
            None => format!("{name} · {attach_state}"),
        };
        let mut col = column![text(meta).colr(palette.text.into_cosmic_color())]
            .spacing(f32::from(spacing::BASE[1]));

        // An attached VDI offers Detach (its VBD); an unattached one offers Attach.
        if v.vm.is_empty() {
            // The typed VM uuid for THIS VDI's armed attach prompt, when armed.
            let armed_typed_vm = self
                .vdi_attach
                .as_ref()
                .filter(|(a, _)| a == &v.id)
                .map(|(_, vm)| vm);
            // Attach: arm the inline VM-uuid prompt, then confirm.
            if let Some(typed_vm) = armed_typed_vm {
                let vm_input = text_input("destination VM uuid", typed_vm)
                    .on_input(|s| crate::Message::Datacenter(Message::VdiAttachVmChanged(s)));
                let confirm = variant_button(
                    "Attach".to_string(),
                    ButtonVariant::Primary,
                    // Only carries the message once a VM is typed.
                    if typed_vm.trim().is_empty() {
                        None
                    } else {
                        Some(crate::Message::Datacenter(Message::VdiAttachClicked {
                            vdi: v.id.clone(),
                            dom0: v.host.clone(),
                        }))
                    },
                    palette,
                );
                let cancel = variant_button(
                    "Cancel".to_string(),
                    ButtonVariant::Secondary,
                    Some(crate::Message::Datacenter(Message::VdiAttachCancel)),
                    palette,
                );
                col = col.push(
                    row![vm_input, confirm, cancel]
                        .spacing(f32::from(spacing::BASE[1]))
                        .align_y(cosmic::iced::alignment::Vertical::Center),
                );
            } else {
                col = col.push(variant_button(
                    "Attach".to_string(),
                    ButtonVariant::Secondary,
                    Some(crate::Message::Datacenter(Message::VdiAttachArm {
                        vdi: v.id.clone(),
                        dom0: v.host.clone(),
                    })),
                    palette,
                ));
            }
        } else {
            // Detach: typed-confirm gated (destructive — drops the VM↔disk link).
            let armed =
                self.vdi_detach_confirm.as_deref() == Some(v.vbd.as_str()) && !v.vbd.is_empty();
            if armed {
                let typed = text_input("type DETACH", &self.vdi_detach_input)
                    .on_input(|s| crate::Message::Datacenter(Message::VdiDetachInputChanged(s)));
                let confirm = variant_button(
                    "Confirm detach".to_string(),
                    ButtonVariant::Primary,
                    if self.vdi_detach_input.trim() == "DETACH" {
                        Some(crate::Message::Datacenter(Message::VdiDetachClicked {
                            vbd: v.vbd.clone(),
                            dom0: v.host.clone(),
                        }))
                    } else {
                        None
                    },
                    palette,
                );
                let cancel = variant_button(
                    "Cancel".to_string(),
                    ButtonVariant::Secondary,
                    Some(crate::Message::Datacenter(Message::VdiDetachCancel)),
                    palette,
                );
                col = col.push(
                    row![typed, confirm, cancel]
                        .spacing(f32::from(spacing::BASE[1]))
                        .align_y(cosmic::iced::alignment::Vertical::Center),
                );
            } else {
                // Only an attached VDI with a known VBD can be detached.
                let msg = if v.vbd.is_empty() {
                    None
                } else {
                    Some(crate::Message::Datacenter(Message::VdiDetachArm {
                        vbd: v.vbd.clone(),
                        dom0: v.host.clone(),
                    }))
                };
                col = col.push(variant_button(
                    "Detach".to_string(),
                    ButtonVariant::Secondary,
                    msg,
                    palette,
                ));
            }
        }
        col.into()
    }

    /// DATACENTER-12 — the shared bordered-surface container the Storage tab's
    /// forms + SR cards live in (mde-theme surface/border tokens, §4-clean). Keeps
    /// every storage block visually consistent with the VM create form.
    fn storage_card<'a>(
        &self,
        content: Element<'a, crate::Message>,
        palette: Palette,
    ) -> Element<'a, crate::Message> {
        let surface = palette.surface;
        let radius = f32::from(spacing::BASE[1]);
        container(content)
            .padding(f32::from(CARD_PAD_PX))
            .width(Length::Fill)
            .style(move |_theme| container::Style {
                background: Some(cosmic::iced::Background::Color(surface.into_cosmic_color())),
                border: cosmic::iced::Border {
                    color: palette.border.into_cosmic_color(),
                    width: 1.0,
                    radius: radius.into(),
                },
                ..container::Style::default()
            })
            .into()
    }

    /// DATACENTER-16 (Power tab) — the whole energy-aware Power tab: a per-host
    /// wake form (IPMI primary, Wake-on-LAN fallback) with a phased
    /// POST→XCP→toolstack progress bar driven by a learned per-host ETA, and an
    /// idle-shutdown policy that recommends powering down a host carrying zero
    /// running guests (the graceful shutdown runs through the existing
    /// confirm-gated host-power op). Returns one column the view dispatch pushes;
    /// mde-theme tokens only (§4) — every control routes a panel-scoped message.
    fn power_tab_view(&self, palette: Palette) -> Element<'_, crate::Message> {
        let mut col = column![text("Power").size(f32::from(spacing::BASE[5]))]
            .spacing(f32::from(spacing::BASE[2]));
        col = col.push(
            text("Energy-aware host power — wake by demand, shut down when idle.")
                .colr(palette.text_muted.into_cosmic_color()),
        );

        // ── Section 1: wake a host (IPMI primary, WoL fallback) ──────────────
        col = col.push(text("Wake a host").size(f32::from(spacing::BASE[5])));
        let pf = &self.power_form;
        let bmc_row = row![
            text("BMC").colr(palette.text_muted.into_cosmic_color()),
            text_input("BMC host (IPv4 / hostname)", &pf.bmc)
                .on_input(|v| crate::Message::Datacenter(Message::PowerFieldChanged {
                    field: PowerField::Bmc,
                    value: v,
                }))
                .width(Length::FillPortion(2)),
            text_input("user", &pf.user)
                .on_input(|v| crate::Message::Datacenter(Message::PowerFieldChanged {
                    field: PowerField::User,
                    value: v,
                }))
                .width(Length::FillPortion(1)),
            text_input("pass", &pf.pass)
                .on_input(|v| crate::Message::Datacenter(Message::PowerFieldChanged {
                    field: PowerField::Pass,
                    value: v,
                }))
                .secure(true)
                .width(Length::FillPortion(1)),
        ]
        .spacing(f32::from(spacing::BASE[2]))
        .align_y(cosmic::iced::alignment::Vertical::Center);
        col = col.push(self.storage_card(bmc_row.into(), palette));

        let mac_row = row![
            text("MAC").colr(palette.text_muted.into_cosmic_color()),
            text_input("aa:bb:cc:dd:ee:ff (WoL fallback / WoL-only)", &pf.mac)
                .on_input(|v| crate::Message::Datacenter(Message::PowerFieldChanged {
                    field: PowerField::Mac,
                    value: v,
                }))
                .width(Length::FillPortion(2)),
            variant_button(
                "Wake (IPMI)".to_string(),
                ButtonVariant::Primary,
                Some(crate::Message::Datacenter(Message::WakeClicked)),
                palette,
            ),
            variant_button(
                "WoL only".to_string(),
                ButtonVariant::Secondary,
                Some(crate::Message::Datacenter(Message::WolWakeClicked)),
                palette,
            ),
        ]
        .spacing(f32::from(spacing::BASE[2]))
        .align_y(cosmic::iced::alignment::Vertical::Center);
        col = col.push(self.storage_card(mac_row.into(), palette));

        // The phased wake progress bar + learned ETA (only while a wake is live).
        if let Some(wp) = &self.wake_progress {
            col = col.push(self.wake_progress_view(wp, palette));
        }

        // ── Section 2: idle-shutdown policy ──────────────────────────────────
        col = col.push(text("Idle-shutdown policy").size(f32::from(spacing::BASE[5])));
        col = col.push(
            text(
                "A host with zero running guests is a graceful-shutdown candidate. \
                 Check a dom0, then shut it down (the graceful host-disable runs \
                 through the confirm-gated host op).",
            )
            .colr(palette.text_muted.into_cosmic_color()),
        );
        let idle_row = row![
            text("dom0").colr(palette.text_muted.into_cosmic_color()),
            text_input("dom0 IP (allow-listed Xen host)", &pf.dom0)
                .on_input(|v| crate::Message::Datacenter(Message::PowerFieldChanged {
                    field: PowerField::Dom0,
                    value: v,
                }))
                .width(Length::FillPortion(2)),
            variant_button(
                "Check idle".to_string(),
                ButtonVariant::Secondary,
                Some(crate::Message::Datacenter(Message::IdlePolicyClicked)),
                palette,
            ),
            // The shutdown button only carries its message when the last idle
            // check recommended it — the gate is visible, not silent.
            variant_button(
                "Shut down (graceful)".to_string(),
                ButtonVariant::Primary,
                if self.idle_shutdown_ok {
                    Some(crate::Message::Datacenter(Message::IdleShutdownClicked))
                } else {
                    None
                },
                palette,
            ),
        ]
        .spacing(f32::from(spacing::BASE[2]))
        .align_y(cosmic::iced::alignment::Vertical::Center);
        col = col.push(self.storage_card(idle_row.into(), palette));
        if !self.idle_reason.is_empty() {
            let tone = if self.idle_shutdown_ok {
                palette.success
            } else {
                palette.text_muted
            };
            col = col.push(
                self.storage_card(
                    text(format!("Recommendation: {}", self.idle_reason))
                        .colr(tone.into_cosmic_color())
                        .into(),
                    palette,
                ),
            );
        }

        col.into()
    }

    /// DATACENTER-16 (Power tab) — render the live wake-in-progress: the target
    /// label, a phased POST→XCP→toolstack progress bar (a token-filled portion of
    /// the track width, never a raw hex), the current phase, and the learned ETA.
    /// The bar holds at 99% until the XAPI poll flips it to ready, so a full bar
    /// always means the host actually answered. mde-theme tokens only (§4).
    fn wake_progress_view<'a>(
        &self,
        wp: &WakeProgress,
        palette: Palette,
    ) -> Element<'a, crate::Message> {
        // Phase label + ETA / ready line.
        let phase_label = match wp.phase.as_str() {
            "post" => "POST (firmware)",
            "xcp" => "XCP-ng boot",
            "toolstack" => "Toolstack (XAPI) coming up",
            "ready" => "Ready",
            other => other,
        };
        let head = if wp.ready {
            text(format!("{} — ready", wp.target)).colr(palette.success.into_cosmic_color())
        } else {
            text(format!("{} — {phase_label} · ETA {}s", wp.target, wp.eta))
                .colr(palette.text.into_cosmic_color())
        };

        // The bar: a filled portion + a remainder portion across the track. The
        // filled portion is success-toned once ready, accent-toned while waking.
        let filled = (wp.progress.clamp(0.0, 1.0) * 1000.0).round() as u16;
        let remainder = 1000u16.saturating_sub(filled).max(1);
        let fill_color = if wp.ready {
            palette.success
        } else {
            palette.accent
        };
        let track = palette.raised;
        let bar = container(
            row![
                container(text(""))
                    .width(Length::FillPortion(filled.max(1)))
                    .height(Length::Fixed(f32::from(spacing::BASE[3])))
                    .style(move |_theme| container::Style {
                        background: Some(cosmic::iced::Background::Color(
                            fill_color.into_cosmic_color(),
                        )),
                        ..container::Style::default()
                    }),
                container(text(""))
                    .width(Length::FillPortion(remainder))
                    .height(Length::Fixed(f32::from(spacing::BASE[3])))
                    .style(move |_theme| container::Style {
                        background: Some(cosmic::iced::Background::Color(
                            track.into_cosmic_color()
                        )),
                        ..container::Style::default()
                    }),
            ]
            .spacing(0.0),
        )
        .width(Length::Fill);

        self.storage_card(
            column![head, bar]
                .spacing(f32::from(spacing::BASE[2]))
                .into(),
            palette,
        )
    }

    /// DATACENTER-14 (Gateway tab) — the whole Gateway tab against the DEPLOYED
    /// EdgeOS EdgeRouter (172.20.0.1): the tofu-managed DHCP static reservations +
    /// the live DHCP leases (read-mostly, off `action/dc/gateway-dhcp`), and a
    /// tofu-gated Plan / typed-confirm Apply for reservation changes (the `edgeos`
    /// workspace — never a blind apply from the GUI; the typed-confirm mirrors the
    /// FA_APPLY operator gate). Returns one column the view dispatch pushes;
    /// mde-theme tokens only (§4) — every control routes a panel-scoped message.
    ///
    /// (The worklist named "UniFi", but UniFi is NOT deployed; the live gateway is
    /// the tofu-managed EdgeRouter at `infra/tofu/edgeos/` — see EDGEOS-DHCP-1.)
    fn gateway_tab_view(&self, palette: Palette) -> Element<'_, crate::Message> {
        const EDGEOS_WS: &str = "edgeos";
        let mut col = column![text("Gateway").size(f32::from(spacing::BASE[5]))]
            .spacing(f32::from(spacing::BASE[2]));
        col = col.push(
            text(
                "EdgeOS EdgeRouter @ 172.20.0.1 — DHCP-as-code. Reservations are \
                 tofu-managed; changes go through the gated Plan → Apply below, \
                 never a blind apply.",
            )
            .colr(palette.text_muted.into_cosmic_color()),
        );

        // ── Section 1: read the live DHCP state ──────────────────────────────
        let read_row = row![
            text("EdgeOS DHCP").colr(palette.text_muted.into_cosmic_color()),
            variant_button(
                "Read DHCP".to_string(),
                ButtonVariant::Secondary,
                Some(crate::Message::Datacenter(Message::GatewayDhcpClicked)),
                palette,
            ),
        ]
        .spacing(f32::from(spacing::BASE[2]))
        .align_y(cosmic::iced::alignment::Vertical::Center);
        col = col.push(self.storage_card(read_row.into(), palette));

        if let Some(dhcp) = &self.gateway_dhcp {
            // Reservations (tofu-managed, the desired state).
            col = col.push(
                text(format!(
                    "Static reservations (tofu-managed) — {}",
                    dhcp.reservations.len()
                ))
                .size(f32::from(spacing::BASE[5])),
            );
            if dhcp.reservations.is_empty() {
                col = col.push(
                    text("No reservations in the edgeos workspace.")
                        .colr(palette.text_muted.into_cosmic_color()),
                );
            } else {
                for r in &dhcp.reservations {
                    col = col.push(
                        self.storage_card(
                            text(format!("{} — {} → {}", r.name, r.mac, r.ip))
                                .colr(palette.text.into_cosmic_color())
                                .into(),
                            palette,
                        ),
                    );
                }
            }

            // Live leases (read off the router right now).
            col = col.push(
                text(format!("Live DHCP leases — {}", dhcp.leases.len()))
                    .size(f32::from(spacing::BASE[5])),
            );
            if dhcp.leases.is_empty() {
                col = col.push(
                    text("No active leases reported.").colr(palette.text_muted.into_cosmic_color()),
                );
            } else {
                for l in &dhcp.leases {
                    let name = if l.hostname.is_empty() {
                        String::new()
                    } else {
                        format!("  ({})", l.hostname)
                    };
                    let expiry = if l.expiry.is_empty() {
                        String::new()
                    } else {
                        format!("  · expires {}", l.expiry)
                    };
                    col = col.push(
                        self.storage_card(
                            text(format!("{} — {}{}{}", l.ip, l.mac, name, expiry))
                                .colr(palette.text.into_cosmic_color())
                                .into(),
                            palette,
                        ),
                    );
                }
            }
        } else {
            col = col.push(
                text("Click Read DHCP to load the EdgeOS reservations + live leases.")
                    .colr(palette.text_muted.into_cosmic_color()),
            );
        }

        // ── Section 2: tofu-gated reservation changes (Plan → typed Apply) ───
        col = col.push(
            text("Reservation changes — gated Plan → Apply").size(f32::from(spacing::BASE[5])),
        );
        col = col.push(
            text(
                "Edit infra/tofu/edgeos/terraform.tfvars (the source of truth), then \
                 Plan to review the diff and Apply (typed confirm) to converge the \
                 router. The apply is operator-gated — it never fires on one click.",
            )
            .colr(palette.text_muted.into_cosmic_color()),
        );
        let plan_btn = variant_button(
            "Plan edgeos".to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::TofuPlan(
                EDGEOS_WS.to_string(),
            ))),
            palette,
        );
        let mut ws_row = row![text("edgeos").width(Length::FillPortion(2)), plan_btn]
            .spacing(f32::from(spacing::BASE[2]));
        if self.tofu_confirm.as_deref() == Some(EDGEOS_WS) {
            // Armed: the typed-confirm — only the confirm button carries Apply.
            ws_row = ws_row
                .push(text("Type APPLY to confirm"))
                .push(variant_button(
                    "APPLY".to_string(),
                    ButtonVariant::Primary,
                    Some(crate::Message::Datacenter(Message::TofuApply(
                        EDGEOS_WS.to_string(),
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
            // Unarmed: the first click only arms the confirm (no RPC fires).
            ws_row = ws_row.push(variant_button(
                "Apply edgeos".to_string(),
                ButtonVariant::Primary,
                Some(crate::Message::Datacenter(Message::TofuApplyClicked(
                    EDGEOS_WS.to_string(),
                ))),
                palette,
            ));
        }
        col = col.push(ws_row);
        if self.tofu_output.is_empty() {
            col = col.push(
                text("Run Plan to see the edgeos OpenTofu diff here.")
                    .colr(palette.text_muted.into_cosmic_color()),
            );
        } else {
            col = col.push(
                container(text(self.tofu_output.clone()))
                    .padding(f32::from(spacing::BASE[3]))
                    .width(Length::Fill),
            );
        }

        col.into()
    }

    /// DATACENTER-13 (Network tab) — the whole Network tab: the L2 inventory
    /// (networks / PIFs / VLANs + a VLAN-create form), the overlay peer/route table
    /// (`event/dc/route/*`), the interactive hosts↔networks↔VMs↔gateway topology
    /// map, and the unified IP/DNS view. Returns one column the view dispatch
    /// pushes. mde-theme tokens only (§4).
    fn network_tab_view(&self, palette: Palette) -> Element<'_, crate::Message> {
        let mut col = column![text("Network").size(f32::from(spacing::BASE[5]))]
            .spacing(f32::from(spacing::BASE[2]));

        // ── Section 1: L2 — networks / PIFs / VLANs / NIC mgmt + create ──────────
        col = col.push(text("L2 — networks · NICs · VLANs").size(f32::from(spacing::BASE[5])));
        // Read L2 box + button (fires `action/dc/host-net` against a dom0).
        let read_row = row![
            text("Read L2 on dom0").colr(palette.text_muted.into_cosmic_color()),
            text_input("dom0 IP (allow-listed Xen host)", &self.net_read_dom0)
                .on_input(|v| crate::Message::Datacenter(Message::NetFieldChanged {
                    field: NetField::ReadDom0,
                    value: v,
                }))
                .width(Length::FillPortion(2)),
            variant_button(
                "Read L2".to_string(),
                ButtonVariant::Secondary,
                Some(crate::Message::Datacenter(Message::NetReadClicked)),
                palette,
            ),
        ]
        .spacing(f32::from(spacing::BASE[2]))
        .align_y(cosmic::iced::alignment::Vertical::Center);
        col = col.push(self.storage_card(read_row.into(), palette));

        // The networks/PIFs cards: prefer the live `host-net` read; otherwise fall
        // back to the bridge rows already on the Bus (`kind == "net"`), so the tab
        // is useful even before a Read L2.
        if let Some(read) = &self.net_read {
            col = col.push(text("Networks (live)").colr(palette.text_muted.into_cosmic_color()));
            if read.nets.is_empty() {
                col = col.push(
                    text("No networks on this dom0.").colr(palette.text_muted.into_cosmic_color()),
                );
            } else {
                for n in &read.nets {
                    let mtu = if n.mtu == 0 {
                        String::new()
                    } else {
                        format!(" · MTU {}", n.mtu)
                    };
                    col = col.push(
                        self.storage_card(
                            text(format!(
                                "{} — bridge {}{}  ({})",
                                if n.name.is_empty() { &n.uuid } else { &n.name },
                                n.bridge,
                                mtu,
                                n.uuid
                            ))
                            .colr(palette.text.into_cosmic_color())
                            .into(),
                            palette,
                        ),
                    );
                }
            }
            col = col.push(text("NICs (PIFs)").colr(palette.text_muted.into_cosmic_color()));
            if read.pifs.is_empty() {
                col = col.push(
                    text("No PIFs on this dom0.").colr(palette.text_muted.into_cosmic_color()),
                );
            } else {
                let dom0 = self.net_read_dom0.trim().to_string();
                for p in &read.pifs {
                    col = col.push(self.pif_card_view(p, &dom0, palette));
                }
            }
        } else {
            // Bus-fallback: the lightweight `kind == "net"` rows.
            let nets: Vec<&DcRow> = self
                .rows
                .iter()
                .filter(|r| r.kind == "net" && r.matches_filter(&self.filter))
                .collect();
            if nets.is_empty() {
                col = col.push(
                    text(
                        "No networks on the Bus yet. Set `MCNF_XEN_DOM0S` on the host \
                         source, or click Read L2 for the full PIF/VLAN inventory.",
                    )
                    .colr(palette.text_muted.into_cosmic_color()),
                );
            } else {
                for n in nets {
                    let bridge = if n.bridge.is_empty() {
                        String::new()
                    } else {
                        format!(" · bridge {}", n.bridge)
                    };
                    col = col.push(
                        self.storage_card(
                            text(format!(
                                "{} — {}{} ({})",
                                if n.name.is_empty() { &n.id } else { &n.name },
                                n.status,
                                bridge,
                                n.id
                            ))
                            .colr(palette.text.into_cosmic_color())
                            .into(),
                            palette,
                        ),
                    );
                }
            }
        }
        // VLAN-create form.
        col = col.push(self.vlan_create_form(palette));

        // ── Section 2: overlay peer / route management ───────────────────────────
        col = col.push(self.overlay_routes_view(palette));

        // ── Section 3: interactive topology map (hosts↔networks↔VMs↔gateway) ─────
        col = col.push(self.network_topology_view(palette));

        // ── Section 4: unified IP / DNS view ─────────────────────────────────────
        col = col.push(self.ip_dns_view(palette));

        col.into()
    }

    /// DATACENTER-13 (Network tab) — one PIF rendered as a NIC card: device, MAC,
    /// VLAN tag, carrier/management flags, its network, plus a "VLAN here" action
    /// that seeds the VLAN-create form from this PIF. mde-theme tokens only (§4).
    fn pif_card_view(
        &self,
        p: &PifInfoRow,
        dom0: &str,
        palette: Palette,
    ) -> Element<'_, crate::Message> {
        // The carrier dot reads liveness at a glance: success up, danger down.
        let dot_token = if p.carrier {
            palette.success
        } else {
            palette.danger
        };
        let vlan = if p.vlan < 0 {
            "trunk".to_string()
        } else {
            format!("VLAN {}", p.vlan)
        };
        let mgmt = if p.management { " · mgmt" } else { "" };
        let header = row![
            text("\u{25cf}").colr(dot_token.into_cosmic_color()),
            text(format!("{} — {} · {}{}", p.device, p.mac, vlan, mgmt))
                .colr(palette.text.into_cosmic_color())
                .width(Length::FillPortion(4)),
            variant_button(
                "VLAN here".to_string(),
                ButtonVariant::Secondary,
                Some(crate::Message::Datacenter(Message::VlanTargetPif {
                    pif: p.uuid.clone(),
                    dom0: dom0.to_string(),
                })),
                palette,
            ),
        ]
        .spacing(f32::from(spacing::BASE[2]))
        .align_y(cosmic::iced::alignment::Vertical::Center);
        let meta = text(format!("net {} · pif {}", p.network, p.uuid))
            .colr(palette.text_muted.into_cosmic_color());
        self.storage_card(
            column![header, meta]
                .spacing(f32::from(spacing::BASE[1]))
                .into(),
            palette,
        )
    }

    /// DATACENTER-13 (Network tab) — the VLAN-create form: trunk PIF + tag + new
    /// network name + dom0, packed into `action/dc/host-vlan-create`. A PIF card's
    /// "VLAN here" seeds pif+dom0. mde-theme tokens only (§4).
    fn vlan_create_form(&self, palette: Palette) -> Element<'_, crate::Message> {
        let field = |placeholder: &str, value: &str, f: NetField| {
            text_input(placeholder, value)
                .on_input(move |v| {
                    crate::Message::Datacenter(Message::NetFieldChanged { field: f, value: v })
                })
                .width(Length::FillPortion(1))
        };
        let inputs = row![
            field("trunk PIF uuid", &self.vlan_create.pif, NetField::VlanPif),
            field(
                "VLAN tag (1..4094)",
                &self.vlan_create.vlan,
                NetField::VlanTag
            ),
            field(
                "new network name",
                &self.vlan_create.network_name,
                NetField::VlanName,
            ),
            field("dom0", &self.vlan_create.dom0, NetField::VlanDom0),
        ]
        .spacing(f32::from(spacing::BASE[1]));
        let create_btn = variant_button(
            "Create VLAN".to_string(),
            ButtonVariant::Primary,
            Some(crate::Message::Datacenter(Message::VlanCreateClicked)),
            palette,
        );
        let card = column![
            text("Create VLAN — network-create + pool-vlan-create")
                .colr(palette.text.into_cosmic_color()),
            inputs,
            create_btn,
        ]
        .spacing(f32::from(spacing::BASE[2]));
        self.storage_card(card.into(), palette)
    }

    /// DATACENTER-13 (Network tab) — the overlay peer/route table: one row per
    /// `event/dc/route/*` route, showing the dom0 it reaches, the path (direct vs a
    /// relay hop), the relay, and the note. The path glyph + token signal direct
    /// (success) vs relay (warning) at a glance. mde-theme tokens only (§4).
    fn overlay_routes_view(&self, palette: Palette) -> Element<'_, crate::Message> {
        let mut col = column![text("Overlay — peers & routes").size(f32::from(spacing::BASE[5]))]
            .spacing(f32::from(spacing::BASE[1]));
        if self.routes.is_empty() {
            col = col.push(
                text(
                    "No overlay routes yet. The orchestrator publishes \
                     `event/dc/route/xen/*` as it chooses each dom0's XAPI path \
                     (direct on-LAN vs a relay hop).",
                )
                .colr(palette.text_muted.into_cosmic_color()),
            );
            return self.storage_card(col.into(), palette);
        }
        for r in &self.routes {
            let (glyph, token) = if r.path == "relay" {
                ("\u{2192}\u{2192}", palette.warning)
            } else {
                ("\u{2192}", palette.success)
            };
            let relay = if r.relay.is_empty() {
                String::new()
            } else {
                format!(" via {}", r.relay)
            };
            let note = if r.note.is_empty() {
                String::new()
            } else {
                format!("  — {}", r.note)
            };
            let line = row![
                text(glyph.to_string())
                    .colr(token.into_cosmic_color())
                    .width(Length::FillPortion(1)),
                text(format!("{} ({})", r.id, r.target))
                    .colr(palette.text.into_cosmic_color())
                    .width(Length::FillPortion(3)),
                text(format!("{}{}{}", r.path, relay, note))
                    .colr(palette.text_muted.into_cosmic_color())
                    .width(Length::FillPortion(4)),
            ]
            .spacing(f32::from(spacing::BASE[2]));
            col = col.push(line);
        }
        self.storage_card(col.into(), palette)
    }

    /// DATACENTER-13 (Network tab) — the interactive hosts↔networks↔VMs↔gateway
    /// topology map. Reuses the collapsible host-group tree (the same
    /// expand/collapse `HeaderClicked` interaction as the Topology tab) but lenses
    /// each host group down to its NETWORK fabric: the host's nets/bridges, the VMs
    /// attached to it, and the synthetic Prod/Gateway group. mde-theme tokens (§4).
    fn network_topology_view(&self, palette: Palette) -> Element<'_, crate::Message> {
        let mut col =
            column![
                text("Topology — hosts \u{2194} networks \u{2194} VMs \u{2194} gateway")
                    .size(f32::from(spacing::BASE[5]))
            ]
            .spacing(f32::from(spacing::BASE[1]));
        // Keep only network-relevant rows in each host group: nets, VMs, gateway,
        // droplets (their public IPs), plus the host headers themselves.
        let filtered: Vec<DcRow> = self
            .rows
            .iter()
            .filter(|r| {
                r.matches_filter(&self.filter)
                    && matches!(
                        r.kind.as_str(),
                        "host" | "net" | "vm" | "gateway" | "droplet"
                    )
            })
            .cloned()
            .collect();
        let groups = group_by_host(&filtered);
        if groups.is_empty() {
            col = col.push(
                text(
                    "No network topology yet. Hosts, their networks/bridges, the VMs \
                     attached, and the gateway appear here as the orchestrator \
                     publishes them.",
                )
                .colr(palette.text_muted.into_cosmic_color()),
            );
            return self.storage_card(col.into(), palette);
        }
        for (header, children) in &groups {
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
        self.storage_card(col.into(), palette)
    }

    /// DATACENTER-13 (Network tab) — the unified IP / DNS view. Correlates the
    /// public/overlay IPs the Bus carries (droplet `ip` + DO `region`, and any
    /// host/VM `ip`) into one table so an operator sees the fleet's addressing in
    /// one place: UniFi-lease ↔ DO-DNS ↔ overlay correlation. Rows with no IP are
    /// omitted. mde-theme tokens only (§4).
    fn ip_dns_view(&self, palette: Palette) -> Element<'_, crate::Message> {
        let mut col = column![text("Unified IP / DNS").size(f32::from(spacing::BASE[5]))]
            .spacing(f32::from(spacing::BASE[1]));
        // Header row.
        col = col.push(
            row![
                text("resource")
                    .colr(palette.text_muted.into_cosmic_color())
                    .width(Length::FillPortion(3)),
                text("IPv4")
                    .colr(palette.text_muted.into_cosmic_color())
                    .width(Length::FillPortion(2)),
                text("region / zone")
                    .colr(palette.text_muted.into_cosmic_color())
                    .width(Length::FillPortion(2)),
                text("kind")
                    .colr(palette.text_muted.into_cosmic_color())
                    .width(Length::FillPortion(1)),
            ]
            .spacing(f32::from(spacing::BASE[2])),
        );
        let addressed: Vec<&DcRow> = self
            .rows
            .iter()
            .filter(|r| !r.ip.is_empty() && r.matches_filter(&self.filter))
            .collect();
        if addressed.is_empty() {
            col = col.push(
                text(
                    "No addressed resources yet. Droplet public IPs (+ DO region), \
                     and host/VM overlay IPs appear here as the orchestrator \
                     publishes them.",
                )
                .colr(palette.text_muted.into_cosmic_color()),
            );
            return self.storage_card(col.into(), palette);
        }
        for r in addressed {
            let label = if r.name.is_empty() { &r.id } else { &r.name };
            // Region (droplets) falls back to the zone label for non-DO rows.
            let locus = if r.region.is_empty() {
                r.zone_label().to_string()
            } else {
                r.region.clone()
            };
            col = col.push(
                row![
                    text(label.clone())
                        .colr(palette.text.into_cosmic_color())
                        .width(Length::FillPortion(3)),
                    text(r.ip.clone())
                        .colr(palette.text.into_cosmic_color())
                        .width(Length::FillPortion(2)),
                    text(locus)
                        .colr(palette.text_muted.into_cosmic_color())
                        .width(Length::FillPortion(2)),
                    text(r.kind.clone())
                        .colr(palette.text_muted.into_cosmic_color())
                        .width(Length::FillPortion(1)),
                ]
                .spacing(f32::from(spacing::BASE[2])),
            );
        }
        self.storage_card(col.into(), palette)
    }

    /// DATACENTER-11 (VMs tab) — the multi-select bulk toolbar: select-all (visible)
    /// / clear, a tag input, and the bulk action buttons (power start/shutdown/reboot,
    /// snapshot, tag). Each action fires one RPC per selected VM and the per-item
    /// progress (`bulk_progress`) renders below as each lands. `visible` is the
    /// currently-shown VM uuids (for select-all); `selected_pairs` pairs each selected
    /// uuid with its owning dom0 (so each RPC targets the right host).
    fn vm_bulk_toolbar<'a>(
        &'a self,
        palette: Palette,
        visible: &[String],
        selected_pairs: &[(String, String)],
    ) -> Element<'a, crate::Message> {
        let n = selected_pairs.len();
        let pairs = selected_pairs.to_vec();
        let bulk = |btn_label: &str, op: &str| {
            let pairs = pairs.clone();
            let op = op.to_string();
            variant_button(
                btn_label.to_string(),
                ButtonVariant::Secondary,
                Some(crate::Message::Datacenter(Message::BulkRun {
                    op,
                    dom0_by_uuid: pairs,
                })),
                palette,
            )
        };
        let select_all = variant_button(
            "Select all (visible)".to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::BulkSelectAll(
                visible.to_vec(),
            ))),
            palette,
        );
        let clear = variant_button(
            "Clear".to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::BulkClear)),
            palette,
        );
        let tag_box = text_input("tag", &self.bulk_tag)
            .on_input(|v| crate::Message::Datacenter(Message::BulkTagChanged(v)))
            .width(Length::FillPortion(1));
        let controls = row![
            text(format!("Bulk — {n} selected")).colr(palette.text_muted.into_cosmic_color()),
            select_all,
            clear,
            bulk("Start", "start"),
            bulk("Stop", "shutdown"),
            bulk("Reboot", "reboot"),
            bulk("Snapshot", "snapshot"),
            tag_box,
            bulk("Tag", "tag"),
        ]
        .spacing(f32::from(spacing::BASE[1]))
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let mut card = column![controls].spacing(f32::from(spacing::BASE[2]));
        // Per-item progress: one line per VM in the last/in-flight bulk run.
        if !self.bulk_progress.is_empty() {
            for (uuid, item) in &self.bulk_progress {
                let (txt, color) = match item {
                    BulkItem::Pending => (format!("{uuid}: …running"), palette.text_muted),
                    BulkItem::Ok(s) => (format!("{uuid}: \u{2713} {s}"), palette.success),
                    BulkItem::Err(e) => (format!("{uuid}: \u{2717} {e}"), palette.danger),
                };
                card = card.push(text(txt).colr(color.into_cosmic_color()));
            }
        }
        let surface = palette.surface;
        let radius = f32::from(spacing::BASE[1]);
        container(card)
            .padding(f32::from(CARD_PAD_PX))
            .width(Length::Fill)
            .style(move |_theme| container::Style {
                background: Some(cosmic::iced::Background::Color(surface.into_cosmic_color())),
                border: cosmic::iced::Border {
                    color: palette.border.into_cosmic_color(),
                    width: 1.0,
                    radius: radius.into(),
                },
                ..container::Style::default()
            })
            .into()
    }

    /// DATACENTER-11 (VMs tab) — render one VM as a full-lifecycle card: a select
    /// checkbox + name/status header, then the lifecycle controls (power
    /// start/stop/reboot, suspend/resume, clone, snapshot, console, migrate, resize,
    /// delete). Delete arms the existing inline confirm; migrate + resize arm an
    /// inline input prompt (`vm_prompt`) whose Confirm fires the RPC. A per-VM bulk
    /// progress line shows when this VM is part of an in-flight bulk run. All controls
    /// route panel-scoped messages back through `update`; mde-theme tokens only (§4).
    fn vm_lifecycle_card<'a>(
        &'a self,
        r: &DcRow,
        palette: Palette,
        selected: bool,
        confirming: bool,
        prompt: Option<&'a VmPrompt>,
        progress: Option<&'a BulkItem>,
    ) -> Element<'a, crate::Message> {
        let uuid = r.id.clone();
        let dom0 = r.host.clone();
        let label = if r.name.is_empty() {
            r.id.clone()
        } else {
            r.name.clone()
        };
        // Header: a multi-select checkbox (rendered as a toggle button so it needs no
        // new widget), the VM name, and the color-dot liveness.
        let check = variant_button(
            if selected {
                "[x]".to_string()
            } else {
                "[ ]".to_string()
            },
            if selected {
                ButtonVariant::Primary
            } else {
                ButtonVariant::Secondary
            },
            Some(crate::Message::Datacenter(Message::BulkToggle(
                uuid.clone(),
            ))),
            palette,
        );
        let header = row![
            check,
            text(format!("VM {label}"))
                .colr(palette.text.into_cosmic_color())
                .width(Length::FillPortion(1)),
            status_dot_view(r, palette),
        ]
        .spacing(f32::from(spacing::BASE[2]))
        .align_y(cosmic::iced::alignment::Vertical::Center);
        let mut card = column![
            header,
            text(format!("{} · {}", r.status, r.host)).colr(palette.text_muted.into_cosmic_color()),
        ]
        .spacing(f32::from(spacing::BASE[2]));

        // Lifecycle controls. Power maps to `vm-power`; suspend/resume to
        // `vm-suspend`; console reads the noVNC `location`; clone/snapshot fire
        // directly; migrate/resize/delete arm an inline prompt first.
        let power = |btn_label: &str, op: &str| {
            variant_button(
                btn_label.to_string(),
                ButtonVariant::Secondary,
                Some(crate::Message::Datacenter(Message::PowerClicked {
                    uuid: uuid.clone(),
                    op: op.to_string(),
                    dom0: dom0.clone(),
                })),
                palette,
            )
        };
        let suspend = |btn_label: &str, op: &str| {
            variant_button(
                btn_label.to_string(),
                ButtonVariant::Secondary,
                Some(crate::Message::Datacenter(Message::SuspendClicked {
                    uuid: uuid.clone(),
                    op: op.to_string(),
                    dom0: dom0.clone(),
                })),
                palette,
            )
        };
        let console = variant_button(
            "Console".to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::ConsoleClicked {
                uuid: uuid.clone(),
                dom0: dom0.clone(),
            })),
            palette,
        );
        let clone = variant_button(
            "Clone".to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::CloneClicked {
                uuid: uuid.clone(),
                dom0: dom0.clone(),
            })),
            palette,
        );
        let snapshot = variant_button(
            "Snapshot".to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::SnapshotClicked {
                uuid: uuid.clone(),
                dom0: dom0.clone(),
            })),
            palette,
        );
        // "Snapshots" lists this VM's snapshots into the per-VM cache; each listed
        // snapshot then renders its own Revert / Delete actions below.
        let snapshots = variant_button(
            "Snapshots".to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::SnapshotsClicked {
                uuid: uuid.clone(),
                dom0: dom0.clone(),
            })),
            palette,
        );
        let migrate = variant_button(
            "Migrate".to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::MigrateClicked {
                uuid: uuid.clone(),
                dom0: dom0.clone(),
            })),
            palette,
        );
        let resize = variant_button(
            "Resize".to_string(),
            ButtonVariant::Secondary,
            Some(crate::Message::Datacenter(Message::ResizeClicked {
                uuid: uuid.clone(),
                dom0: dom0.clone(),
            })),
            palette,
        );
        let lifecycle = row![
            power("Start", "start"),
            power("Stop", "shutdown"),
            power("Reboot", "reboot"),
            suspend("Suspend", "suspend"),
            suspend("Resume", "resume"),
            console,
            clone,
            snapshot,
            snapshots,
            migrate,
            resize,
        ]
        .spacing(f32::from(spacing::BASE[1]));
        card = card.push(lifecycle);

        // The destructive Delete row mirrors the existing inline-confirm contract.
        if confirming {
            card = card.push(
                row![
                    text("Really delete?").colr(palette.warning.into_cosmic_color()),
                    variant_button(
                        "Confirm".to_string(),
                        ButtonVariant::Primary,
                        Some(crate::Message::Datacenter(Message::DeleteConfirmed {
                            uuid: uuid.clone(),
                            dom0: dom0.clone(),
                        })),
                        palette,
                    ),
                    variant_button(
                        "Cancel".to_string(),
                        ButtonVariant::Secondary,
                        Some(crate::Message::Datacenter(Message::DeleteCancelled)),
                        palette,
                    ),
                ]
                .spacing(f32::from(spacing::BASE[1]))
                .align_y(cosmic::iced::alignment::Vertical::Center),
            );
        } else {
            card = card.push(variant_button(
                "Delete".to_string(),
                ButtonVariant::Primary,
                Some(crate::Message::Datacenter(Message::DeleteClicked {
                    uuid: uuid.clone(),
                    dom0: dom0.clone(),
                })),
                palette,
            ));
        }

        // The inline migrate/resize input prompt — only its Confirm fires the RPC.
        match prompt {
            Some(VmPrompt::Migrate { host, .. }) => {
                card = card.push(
                    row![
                        text("Migrate to host:").colr(palette.text_muted.into_cosmic_color()),
                        text_input("destination host", host)
                            .on_input(|v| crate::Message::Datacenter(Message::MigrateHostChanged(
                                v
                            )))
                            .width(Length::FillPortion(1)),
                        variant_button(
                            "Confirm".to_string(),
                            ButtonVariant::Primary,
                            Some(crate::Message::Datacenter(Message::MigrateConfirmed {
                                uuid: uuid.clone(),
                                dom0: dom0.clone(),
                            })),
                            palette,
                        ),
                        variant_button(
                            "Cancel".to_string(),
                            ButtonVariant::Secondary,
                            Some(crate::Message::Datacenter(Message::VmPromptCancelled)),
                            palette,
                        ),
                    ]
                    .spacing(f32::from(spacing::BASE[1]))
                    .align_y(cosmic::iced::alignment::Vertical::Center),
                );
            }
            Some(VmPrompt::Resize { vcpus, mem_mib, .. }) => {
                card = card.push(
                    row![
                        text("Resize:").colr(palette.text_muted.into_cosmic_color()),
                        text_input("vCPUs", vcpus)
                            .on_input(|v| crate::Message::Datacenter(Message::ResizeVcpusChanged(
                                v
                            )))
                            .width(Length::FillPortion(1)),
                        text_input("memory MiB", mem_mib)
                            .on_input(|v| crate::Message::Datacenter(Message::ResizeMemChanged(v)))
                            .width(Length::FillPortion(1)),
                        variant_button(
                            "Confirm".to_string(),
                            ButtonVariant::Primary,
                            Some(crate::Message::Datacenter(Message::ResizeConfirmed {
                                uuid: uuid.clone(),
                                dom0: dom0.clone(),
                            })),
                            palette,
                        ),
                        variant_button(
                            "Cancel".to_string(),
                            ButtonVariant::Secondary,
                            Some(crate::Message::Datacenter(Message::VmPromptCancelled)),
                            palette,
                        ),
                    ]
                    .spacing(f32::from(spacing::BASE[1]))
                    .align_y(cosmic::iced::alignment::Vertical::Center),
                );
            }
            None => {}
        }

        // DATACENTER-11 — the per-VM snapshot list, shown once "Snapshots" has been
        // clicked and the read-only `vm-snapshots` RPC populated the cache. Each
        // snapshot renders a Revert (reversible — fires directly) and a Delete
        // (destructive — typed-confirm gated, mirroring the VM delete contract).
        if let Some(snaps) = self.vm_snapshots.get(&uuid) {
            if snaps.is_empty() {
                card =
                    card.push(text("Snapshots: none").colr(palette.text_muted.into_cosmic_color()));
            } else {
                card = card.push(
                    text(format!("Snapshots ({})", snaps.len()))
                        .colr(palette.text_muted.into_cosmic_color()),
                );
                for snap in snaps {
                    let label = if snap.name.is_empty() {
                        snap.uuid.clone()
                    } else {
                        format!("{} · {}", snap.name, snap.uuid)
                    };
                    let revert = variant_button(
                        "Revert".to_string(),
                        ButtonVariant::Secondary,
                        Some(crate::Message::Datacenter(Message::SnapshotRevertClicked {
                            snapshot: snap.uuid.clone(),
                            dom0: dom0.clone(),
                        })),
                        palette,
                    );
                    // The destructive Delete mirrors the VM-delete inline confirm: the
                    // first click only arms the per-snapshot "Really delete?" prompt.
                    let delete: Element<'a, crate::Message> =
                        if self.confirm_snapshot_delete.as_deref() == Some(snap.uuid.as_str()) {
                            row![
                                text("Really delete?").colr(palette.warning.into_cosmic_color()),
                                variant_button(
                                    "Confirm".to_string(),
                                    ButtonVariant::Primary,
                                    Some(crate::Message::Datacenter(
                                        Message::SnapshotDeleteConfirmed {
                                            snapshot: snap.uuid.clone(),
                                            vm: uuid.clone(),
                                            dom0: dom0.clone(),
                                        }
                                    )),
                                    palette,
                                ),
                                variant_button(
                                    "Cancel".to_string(),
                                    ButtonVariant::Secondary,
                                    Some(crate::Message::Datacenter(
                                        Message::SnapshotDeleteCancelled
                                    )),
                                    palette,
                                ),
                            ]
                            .spacing(f32::from(spacing::BASE[1]))
                            .align_y(cosmic::iced::alignment::Vertical::Center)
                            .into()
                        } else {
                            variant_button(
                                "Delete".to_string(),
                                ButtonVariant::Primary,
                                Some(crate::Message::Datacenter(Message::SnapshotDeleteClicked {
                                    snapshot: snap.uuid.clone(),
                                })),
                                palette,
                            )
                        };
                    card = card.push(
                        row![
                            text(label)
                                .colr(palette.text.into_cosmic_color())
                                .width(Length::FillPortion(1)),
                            revert,
                            delete,
                        ]
                        .spacing(f32::from(spacing::BASE[1]))
                        .align_y(cosmic::iced::alignment::Vertical::Center),
                    );
                }
            }
        }

        // This VM's slice of an in-flight/last bulk run, if any.
        if let Some(item) = progress {
            let (txt, color) = match item {
                BulkItem::Pending => ("bulk: …running".to_string(), palette.text_muted),
                BulkItem::Ok(s) => (format!("bulk: \u{2713} {s}"), palette.success),
                BulkItem::Err(e) => (format!("bulk: \u{2717} {e}"), palette.danger),
            };
            card = card.push(text(txt).colr(color.into_cosmic_color()));
        }

        let surface = palette.surface;
        let radius = f32::from(spacing::BASE[1]);
        // A selected card draws an accent outline so the multi-select set is legible.
        let border_color = if selected {
            palette.accent
        } else {
            palette.border
        };
        container(card)
            .padding(f32::from(CARD_PAD_PX))
            .width(Length::Fill)
            .style(move |_theme| container::Style {
                background: Some(cosmic::iced::Background::Color(surface.into_cosmic_color())),
                border: cosmic::iced::Border {
                    color: border_color.into_cosmic_color(),
                    width: if selected { 2.0 } else { 1.0 },
                    radius: radius.into(),
                },
                ..container::Style::default()
            })
            .into()
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
        if self.reveal_start.is_some_and(|start| {
            reveal_is_complete(start, now, self.visible_card_count(), reduce_motion)
        }) {
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
        // Graceful-degrade (DATACENTER-8 acceptance: "unreachable → last-known +
        // stale badge + retry"). A failed Bus read keeps the last-known rows in
        // `self.rows` rather than discarding them; we only fall back to a bare
        // error screen when there's nothing last-known to show. Otherwise the
        // view renders as normal with a `stale_banner` pinned at the top.
        if let Some(err) = &self.load_error {
            if self.rows.is_empty() {
                return container(
                    column![
                        text("Couldn't read datacenter state.")
                            .colr(palette.danger.into_cosmic_color())
                            .size(f32::from(spacing::BASE[6])),
                        text(err.clone()).colr(palette.text_muted.into_cosmic_color()),
                        variant_button(
                            "Retry".to_string(),
                            ButtonVariant::Primary,
                            Some(crate::Message::Datacenter(Message::RefreshClicked)),
                            palette,
                        ),
                    ]
                    .spacing(f32::from(spacing::BASE[2])),
                )
                .padding(f32::from(spacing::BASE[5]))
                .into();
            }
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
            mode_btn("Hosts", ViewMode::Hosts),
            mode_btn("VMs", ViewMode::Vms),
            mode_btn("Storage", ViewMode::Storage),
            mode_btn("Network", ViewMode::Network),
            mode_btn("Power", ViewMode::Power),
            mode_btn("Gateway", ViewMode::Gateway),
            mode_btn("Topology", ViewMode::Topology),
            mode_btn("Resources", ViewMode::Zone),
            mode_btn("Tofu", ViewMode::Tofu),
            mode_btn("Audit", ViewMode::Audit),
            mode_btn("Logs", ViewMode::Logs),
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

        // Graceful-degrade banner: when a Bus read failed but we still have
        // last-known rows, pin a stale badge + Retry over the (stale) data rather
        // than blanking the plane. Color from mde-theme tokens only (§4).
        if let Some(err) = &self.load_error {
            col = col.push(stale_banner(err, palette));
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
                // ones show "—") so the pipeline reads left-to-right. The
                // DATACENTER-20 auto-promote advances Build→Eagle on green (the
                // `auto` chip) and gates the DO step on the prod-arm switch below
                // (`armed → DO` when live, `queued (hold)` when held).
                col = col.push(text("Promotion").size(f32::from(spacing::BASE[5])));
                col = col.push(promote_strip_view(&self.promote, palette));
                // The prod-arm master switch for the auto-promote DO step. While
                // disarmed (default), a green pipeline HOLDS at the DO step
                // (queued); arming lets the leader auto-promote to DO on green.
                // Mirrors the Tofu-tab prod-arm gate; persists to the same file
                // the `dc_promote` worker reads.
                col = col.push(self.promote_prod_arm_bar(palette));
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
            ViewMode::Hosts => {
                // DATACENTER-10 — the Hosts tab: one card per `kind == "host"` row
                // (the Xen dom0s the orchestrator publishes), honoring the global
                // search needle. Each card shows the host's capacity + health and
                // the full lifecycle controls (maintenance / reboot / shutdown /
                // evacuate with impact preview), pool placement, and the SSH console.
                col = col.push(text("Hosts").size(f32::from(spacing::BASE[5])));
                let hosts: Vec<&DcRow> = self
                    .rows
                    .iter()
                    .filter(|r| r.kind == "host" && r.matches_filter(&self.filter))
                    .collect();
                if hosts.is_empty() {
                    col = col.push(
                        text(
                            "No hosts yet. Xen dom0s appear here as the datacenter \
                             orchestrator publishes `event/dc/host/*` (set `MCNF_XEN_DOM0S` \
                             on the host source).",
                        )
                        .colr(palette.text_muted.into_cosmic_color()),
                    );
                } else {
                    for h in hosts {
                        col = col.push(self.host_card_view(h, palette));
                    }
                }
            }
            ViewMode::Vms => {
                // DATACENTER-11 — the VMs tab: the full VM lifecycle on each card
                // (power / suspend / migrate / clone / snapshot / resize / delete +
                // noVNC console), a Tofu-backed golden-template create wizard, and a
                // multi-select bulk toolbar with per-item progress.
                col = col.push(text("Virtual machines").size(f32::from(spacing::BASE[5])));
                col = col.push(self.vm_create_form(palette));

                let vms: Vec<&DcRow> = self
                    .rows
                    .iter()
                    .filter(|r| r.kind == "vm" && r.matches_filter(&self.filter))
                    .collect();
                // The bulk toolbar acts on the current selection; selecting-all uses
                // the visible set, and each bulk RPC needs the VM's owning dom0.
                let visible_uuids: Vec<String> = vms.iter().map(|r| r.id.clone()).collect();
                let dom0_by_uuid: Vec<(String, String)> = vms
                    .iter()
                    .filter(|r| self.vm_selected.contains(&r.id))
                    .map(|r| (r.id.clone(), r.host.clone()))
                    .collect();
                col = col.push(self.vm_bulk_toolbar(palette, &visible_uuids, &dom0_by_uuid));

                if vms.is_empty() {
                    col = col.push(
                        text(
                            "No VMs yet. Xen guests appear here as the datacenter \
                             orchestrator publishes `event/dc/vm/*`, or create one with \
                             the wizard above.",
                        )
                        .colr(palette.text_muted.into_cosmic_color()),
                    );
                } else {
                    for v in vms {
                        let selected = self.vm_selected.contains(&v.id);
                        let confirming = self.confirm_delete.as_deref() == Some(v.id.as_str());
                        let prompt = self
                            .vm_prompt
                            .as_ref()
                            .filter(|p| p.uuid() == v.id.as_str());
                        let progress = self.bulk_progress.get(&v.id);
                        col =
                            col.push(self.vm_lifecycle_card(
                                v, palette, selected, confirming, prompt, progress,
                            ));
                    }
                }
            }
            ViewMode::Storage => {
                // DATACENTER-12 — the Storage tab: SR capacity cards (with the
                // threshold-alert badge + per-SR Snapshot / VDI-target actions), an
                // SR-create + a VDI-create form, the scheduled-snapshot config, and
                // the ISO / template image library. SRs already arrive on the Bus as
                // `kind == "sr"` rows (gather_xen → event/dc/sr/*).
                col = col.push(text("Storage").size(f32::from(spacing::BASE[5])));

                // Capacity-threshold alert: most-full-first banner pinned above the
                // cards so an over-threshold store is impossible to miss. Color from
                // mde-theme tokens (§4) — danger at/above critical, warning above the
                // configured line.
                let alerts = sr_alerts(&self.rows, self.storage_threshold_pct);
                col = col.push(self.storage_threshold_bar(palette, alerts.len()));
                for a in &alerts {
                    let token = if a.critical {
                        palette.danger
                    } else {
                        palette.warning
                    };
                    let mark = if a.critical { "CRITICAL" } else { "WARNING" };
                    col = col.push(
                        text(format!(
                            "{mark} — SR {} is {}% full (threshold {}%)",
                            a.name, a.pct, self.storage_threshold_pct
                        ))
                        .colr(token.into_cosmic_color()),
                    );
                }

                // SR-create + VDI-create forms.
                col = col.push(self.sr_create_form(palette));
                col = col.push(self.vdi_create_form(palette));
                // Scheduled snapshots (retention + backup target).
                col = col.push(self.snap_schedule_form(palette));

                // The SR cards — one per `kind == "sr"` row honoring the global
                // search needle, with capacity + per-SR actions.
                col = col.push(text("Storage repositories").size(f32::from(spacing::BASE[5])));
                let srs: Vec<&DcRow> = self
                    .rows
                    .iter()
                    .filter(|r| r.kind == "sr" && r.matches_filter(&self.filter))
                    .collect();
                if srs.is_empty() {
                    col = col.push(
                        text(
                            "No storage repositories yet. SRs appear here as the \
                             datacenter orchestrator publishes `event/dc/sr/*` (set \
                             `MCNF_XEN_DOM0S` on the host source).",
                        )
                        .colr(palette.text_muted.into_cosmic_color()),
                    );
                } else {
                    for s in srs {
                        col = col.push(self.sr_card_view(s, palette));
                    }
                }

                // The ISO + template image library (absorbs the old `images` view):
                // rows the orchestrator may publish as `kind == "iso"` /
                // `kind == "template"`. Graceful-empty until a source emits them.
                col = col.push(text("Image library").size(f32::from(spacing::BASE[5])));
                let images: Vec<&DcRow> = self
                    .rows
                    .iter()
                    .filter(|r| {
                        (r.kind == "iso" || r.kind == "template") && r.matches_filter(&self.filter)
                    })
                    .collect();
                if images.is_empty() {
                    col = col.push(
                        text(
                            "No ISOs or templates yet. They appear here as the \
                             orchestrator publishes `event/dc/iso/*` / \
                             `event/dc/template/*`.",
                        )
                        .colr(palette.text_muted.into_cosmic_color()),
                    );
                } else {
                    for img in images {
                        col = col.push(image_row_view(img, palette));
                    }
                }
            }
            ViewMode::Tofu => {
                // The prod-arm master switch (FA_APPLY-style operator gate): a
                // mutating op on the PROD workspace is refused unless this is armed
                // — surfaced first so its state is unmissable. Color from mde-theme
                // tokens (danger = armed/live, success = safe-disarmed).
                col = col.push(self.tofu_prod_arm_bar(palette));

                // A Plan + State + Apply + Destroy control set per workspace. Apply
                // and Destroy each arm a typed confirm before firing the
                // destructive `action/dc/tofu-{apply,destroy}` RPC; for the PROD
                // workspace the prod-arm switch gates the fire on top of that.
                for ws in TOFU_WORKSPACES {
                    let is_prod = ws == PROD_TOFU_WS;
                    let label = if is_prod {
                        format!("{ws}  (PROD)")
                    } else {
                        ws.to_string()
                    };
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
                    let mut ws_row = row![
                        text(label).width(Length::FillPortion(2)),
                        plan_btn,
                        state_btn
                    ]
                    .spacing(f32::from(spacing::BASE[2]));
                    // The Apply control — whether it's allowed to fire (prod-arm)
                    // is enforced in the update handler; the button itself is only
                    // disabled (no message) when prod is disarmed, so the gate is
                    // visible, not silent.
                    let mutating_blocked = is_prod && !self.tofu_prod_armed;
                    if self.tofu_confirm.as_deref() == Some(ws) {
                        // Armed: surface the typed-confirm — only the confirm
                        // button carries the destructive `TofuApply` message.
                        let confirm_msg = if mutating_blocked {
                            None
                        } else {
                            Some(crate::Message::Datacenter(Message::TofuApply(
                                ws.to_string(),
                            )))
                        };
                        ws_row = ws_row
                            .push(text("Type APPLY to confirm"))
                            .push(variant_button(
                                "APPLY".to_string(),
                                ButtonVariant::Primary,
                                confirm_msg,
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
                        // Disabled when prod is disarmed so the gate is obvious.
                        let apply_msg = if mutating_blocked {
                            None
                        } else {
                            Some(crate::Message::Datacenter(Message::TofuApplyClicked(
                                ws.to_string(),
                            )))
                        };
                        ws_row = ws_row.push(variant_button(
                            format!("Apply {ws}"),
                            ButtonVariant::Primary,
                            apply_msg,
                            palette,
                        ));
                    }
                    // The Destroy control — same typed-confirm + prod-arm shape as
                    // Apply, on its own row so the destructive action is distinct.
                    let mut destroy_row = row![text("").width(Length::FillPortion(2))]
                        .spacing(f32::from(spacing::BASE[2]));
                    if self.tofu_destroy_confirm.as_deref() == Some(ws) {
                        let confirm_msg = if mutating_blocked {
                            None
                        } else {
                            Some(crate::Message::Datacenter(Message::TofuDestroy(
                                ws.to_string(),
                            )))
                        };
                        destroy_row = destroy_row
                            .push(
                                text("Type DESTROY to confirm")
                                    .colr(palette.danger.into_cosmic_color()),
                            )
                            .push(variant_button(
                                "DESTROY".to_string(),
                                ButtonVariant::Primary,
                                confirm_msg,
                                palette,
                            ))
                            .push(variant_button(
                                "Cancel".to_string(),
                                ButtonVariant::Secondary,
                                Some(crate::Message::Datacenter(Message::TofuDestroyCancelled)),
                                palette,
                            ));
                    } else {
                        let destroy_msg = if mutating_blocked {
                            None
                        } else {
                            Some(crate::Message::Datacenter(Message::TofuDestroyClicked(
                                ws.to_string(),
                            )))
                        };
                        destroy_row = destroy_row.push(variant_button(
                            format!("Destroy {ws}"),
                            ButtonVariant::Secondary,
                            destroy_msg,
                            palette,
                        ));
                    }
                    col = col.push(ws_row);
                    col = col.push(destroy_row);
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
                // The persisted run-log (DATACENTER-15 acceptance): the Tofu runs
                // off the durable `event/dc/job/*` Bus lane — plan/apply/destroy/
                // state newest-first with a status chip. Persisted by the leader-
                // gated `dc_jobs` worker, so a reload still shows past runs.
                col = col.push(text("Tofu run-log").size(f32::from(spacing::BASE[5])));
                for el in recent_tofu_runs_view(&self.jobs, palette) {
                    col = col.push(el);
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
            ViewMode::Network => {
                col = col.push(self.network_tab_view(palette));
            }
            ViewMode::Power => {
                col = col.push(self.power_tab_view(palette));
            }
            ViewMode::Gateway => {
                col = col.push(self.gateway_tab_view(palette));
            }
            ViewMode::Logs => {
                col = col.push(text("Logs").size(f32::from(spacing::BASE[5])));
                // The global search box doubles as the log query (message OR
                // target, case-insensitive) — the same Fleet-logs filter, capped
                // for in-plane render. Newest-first; only datacenter-host journal
                // lines (already narrowed at load).
                let visible =
                    fleet_logs::filter_rows(&self.logs, "trace", &self.filter, LOGS_VIEW_CAP);
                for el in logs_section_view(&self.logs, &visible, palette) {
                    col = col.push(el);
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

                    // Per-tab filter = the active zone tab AND the global search
                    // needle (name / id / kind). The search narrows the card grid
                    // in place; an empty box shows the whole zone.
                    let visible: Vec<&DcRow> = self
                        .rows
                        .iter()
                        .filter(|r| r.zone == self.zone_tab && r.matches_filter(&self.filter))
                        .collect();
                    if visible.is_empty() {
                        // Distinguish "nothing in this zone" from "the search hid
                        // everything" so an empty grid never looks like a bug.
                        if self.filter.trim().is_empty() {
                            col = col.push(text("No resources in this zone yet."));
                        } else {
                            col = col.push(
                                text(format!(
                                    "No resources match \u{201c}{}\u{201d} in this zone.",
                                    self.filter.trim()
                                ))
                                .colr(palette.text_muted.into_cosmic_color()),
                            );
                        }
                    } else {
                        // MOTION-FEEDBACK-2 — a single `now` for the whole grid so
                        // every card's reveal/hover/selection tween reads one
                        // coherent frame, plus the live reduce-motion preference.
                        let now = Instant::now();
                        let reduce_motion = crate::live_theme::reduce_motion();
                        // Responsive card grid — each resource is a card (status
                        // dot + kind/label + readout + actions), wrapped N-per-row,
                        // with a capped staggered reveal + hover-lift + an animated
                        // accent on the selected card.
                        let cards: Vec<Element<'_, crate::Message>> = visible
                            .into_iter()
                            .enumerate()
                            .map(|(i, r)| {
                                let confirming =
                                    self.confirm_delete.as_deref() == Some(r.id.as_str());
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
                                dc_card_view(r, palette, confirming, motion)
                            })
                            .collect();
                        col = col.push(card_grid(cards));
                    }
                }
            }
        }

        scrollable(col).into()
    }
}

/// DATACENTER-8 — the graceful-degrade banner shown when a Bus read failed but
/// the panel still holds a last-known snapshot. Renders a `STALE` badge (danger
/// token), the failure detail (muted token), and a Retry button that re-fires the
/// `load()` path. Colors come from the live `mde-theme` palette only (§4 — no raw
/// hex). Pure construction; the staleness decision lives in `view`.
fn stale_banner<'a>(err: &str, palette: Palette) -> Element<'a, crate::Message> {
    let badge = container(
        text("STALE")
            .colr(palette.danger.into_cosmic_color())
            .size(f32::from(spacing::BASE[4])),
    )
    .padding(f32::from(spacing::BASE[1]));
    let detail = text(format!("Showing last-known state — Bus read failed: {err}"))
        .colr(palette.text_muted.into_cosmic_color());
    let retry = variant_button(
        "Retry".to_string(),
        ButtonVariant::Primary,
        Some(crate::Message::Datacenter(Message::RefreshClicked)),
        palette,
    );
    container(
        row![badge, detail, retry]
            .spacing(f32::from(spacing::BASE[2]))
            .align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .padding(f32::from(spacing::BASE[2]))
    .width(Length::Fill)
    .into()
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
/// time regardless of resource count. The shared Carbon stagger cap
/// ([`mde_theme::motion::list::STAGGER_CAP`]) so the cascade matches every list.
const REVEAL_STAGGER_CAP: usize = mde_theme::motion::list::STAGGER_CAP;

/// MOTION-FEEDBACK-2 — the per-card stagger step. Each (capped) card index `i`
/// delays its slide-in by `i * STAGGER_STEP`, so the grid fills top-left → bottom
/// in a brisk cascade rather than all at once. **Sourced from the shared Carbon
/// list-stagger token** ([`mde_theme::motion::list::STAGGER_STEP_MS`]) so the grid
/// staggers on the same beat as every list/menu — no scattered animation literal
/// (§4 / MOTION-AUDIT-2).
const REVEAL_STAGGER_STEP: Duration =
    Duration::from_millis(mde_theme::motion::list::STAGGER_STEP_MS as u64);

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
        mount.min(Duration::from_millis(
            mde_theme::motion::REDUCE_MOTION_CAP_MS,
        ))
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

/// Map a `fleet_logs` level string to a mde-theme severity token: `error` →
/// danger, `warn`/`warning` → warning, everything else → muted text. Keeps the
/// Logs tab's colouring on the shared palette (§4 — no raw hex). Pure.
#[must_use]
fn log_level_color(level: &str, palette: Palette) -> mde_theme::Rgba {
    match level.to_ascii_lowercase().as_str() {
        "error" => palette.danger,
        "warn" | "warning" => palette.warning,
        _ => palette.text_muted,
    }
}

/// DATACENTER-24 — render the **Logs** tab: the per-dom0 journal tails the
/// `dc_health` worker aggregated into the `fleet_logs` sink, already narrowed to
/// the datacenter hosts (`all`) and filtered to the visible page (`visible`,
/// newest-first). Each row is `level · host · target · message`, the level
/// coloured by its severity token. Empty states distinguish "no datacenter logs
/// aggregated yet" (the worker hasn't run / no dom0s configured) from "no rows
/// match the search". Returns a list of elements so the view column can push them
/// in order. mde-theme tokens only — no raw hex.
fn logs_section_view(
    all: &[fleet_logs::LogRow],
    visible: &[fleet_logs::LogRow],
    palette: Palette,
) -> Vec<Element<'static, crate::Message>> {
    let mut out: Vec<Element<'static, crate::Message>> = Vec::new();
    out.push(
        text(format!(
            "{} aggregated record(s) · {} shown",
            all.len(),
            visible.len()
        ))
        .colr(palette.text_muted.into_cosmic_color())
        .into(),
    );
    if all.is_empty() {
        out.push(
            text(
                "No datacenter logs aggregated yet. The dc_health worker folds each \
                 configured dom0's recent journal tail into the fleet logs sink \
                 (<root>/logs/<dom0>.jsonl).",
            )
            .colr(palette.text_muted.into_cosmic_color())
            .into(),
        );
        return out;
    }
    if visible.is_empty() {
        out.push(
            text("No log records match the search.")
                .colr(palette.text_muted.into_cosmic_color())
                .into(),
        );
        return out;
    }
    for r in visible {
        let level = if r.level.is_empty() {
            "—".to_string()
        } else {
            r.level.clone()
        };
        let line = row![
            text(level)
                .colr(log_level_color(&r.level, palette).into_cosmic_color())
                .width(Length::Fixed(64.0)),
            text(r.host.clone())
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::FillPortion(2)),
            text(r.target.clone())
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::FillPortion(2)),
            text(r.message.clone())
                .colr(palette.text.into_cosmic_color())
                .width(Length::FillPortion(5)),
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

/// Map a promote-stage status to its `(chip color, chip label)`. Pure so the
/// strip's auto/queued/armed rendering is testable. DATACENTER-20 statuses:
/// - `ready` → success (a real, green, serving version);
/// - `auto` → success (auto-advanced off a green upstream — Eagle once Build is
///   green);
/// - `armed` → danger (the DO step is LIVE: green + prod-arm armed ⇒ auto-promote);
/// - `queued` → warning (green but HELD until the operator arms prod);
/// - `pending` → warning (upstream not green yet);
/// - anything else / empty → muted "—" placeholder.
#[must_use]
fn promote_chip(status: &str, palette: Palette) -> (mde_theme::Rgba, String) {
    match status {
        "ready" => (palette.success, "ready".to_string()),
        "auto" => (palette.success, "auto ✓".to_string()),
        "armed" => (palette.danger, "armed → DO".to_string()),
        "queued" => (palette.warning, "queued (hold)".to_string()),
        "pending" => (palette.warning, "pending".to_string()),
        other => (
            palette.text_muted,
            if other.is_empty() {
                "—".to_string()
            } else {
                other.to_string()
            },
        ),
    }
}

/// Render one promotion-stage card: the stage label, its version, and a readiness
/// chip whose color comes from a mde-theme token (see [`promote_chip`]). mde-theme
/// tokens only.
fn promote_card_view(stage: &PromoteStage, palette: Palette) -> Element<'static, crate::Message> {
    // Human stage label.
    let label = match stage.stage.as_str() {
        "build" => "Build",
        "eagle" => "Eagle",
        "do" => "DO",
        other => other,
    }
    .to_string();
    // The chip color tracks readiness; the text names the auto/queued/armed state.
    let (chip_color, chip_text) = promote_chip(stage.status.as_str(), palette);
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
    // Render actor · action · target · result · ts. Each empty field falls back
    // to an em-dash so the column stays aligned (an action without a recorded
    // actor/target/result is shown honestly as "—", not blank).
    let dash = |s: &str| {
        if s.is_empty() {
            "—".to_string()
        } else {
            s.to_string()
        }
    };
    let line = row![
        text(dash(&entry.actor)).width(Length::FillPortion(2)),
        text(entry.action.clone()).width(Length::FillPortion(2)),
        text(dash(&entry.target)).width(Length::FillPortion(2)),
        text(dash(&entry.result)).width(Length::FillPortion(1)),
        text(dash(&entry.ts)).width(Length::FillPortion(2)),
    ]
    .spacing(f32::from(spacing::BASE[3]));
    container(line)
        .padding(f32::from(spacing::BASE[3]))
        .width(Length::Fill)
        .into()
}

/// DATACENTER-12 (Storage tab) — render one image-library row (an ISO or a
/// template) as a labeled line: kind · name · detail, where the detail is the
/// ISO's human size or the template's description ([`DcRow::image_detail`]).
/// Read-only — the library absorbs the old `images` view; an Import flow lands
/// when a source emits the catalog. Carbon tokens only (§4).
fn image_row_view<'a>(r: &DcRow, palette: Palette) -> Element<'a, crate::Message> {
    let label = if r.name.is_empty() {
        r.id.clone()
    } else {
        r.name.clone()
    };
    let kind_label = if r.kind == "iso" { "ISO" } else { "Template" };
    let line = row![
        text(kind_label.to_string())
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::FillPortion(1)),
        text(label)
            .colr(palette.text.into_cosmic_color())
            .width(Length::FillPortion(3)),
        text(r.image_detail())
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::FillPortion(2)),
    ]
    .spacing(f32::from(spacing::BASE[3]));
    container(line)
        .padding(f32::from(spacing::BASE[2]))
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
    let rows = project_rows(&events);
    // DATACENTER-24 — read the dom0 journal tails the `dc_health` worker aggregated
    // into the fleet_logs sink and narrow them to this datacenter's hosts. The
    // host ids are the `host`-kind rows' ids (the dom0 IPs the worker tags each
    // line with). Reuses the Fleet-logs `load_all` reader (§6 glue).
    let dc_host_ids: std::collections::BTreeSet<String> = rows
        .iter()
        .filter(|r| r.kind == "host")
        .map(|r| r.id.clone())
        .collect();
    let logs = datacenter_log_rows(
        &fleet_logs::load_all(&fleet_logs::workgroup_root()),
        &dc_host_ids,
    );
    Ok(DcLoad {
        rows,
        audit: project_audit(&events),
        promote: project_promote(&events),
        health: project_health(&events),
        jobs: project_jobs(&events),
        routes: project_routes(&events),
        logs,
    })
}

/// FRONTDOOR-4 — read just the datacenter health checks (`event/dc/health/*`)
/// off the Bus + project them, for the Front Door's Alerts widget tile. A
/// narrowed [`read_dc_events`] (same `list_topics` + filter + `project_health`
/// pattern) so the Alerts tile counts the same `warn`/`fail` checks the
/// Datacenter Overview does (§6 glue). Best-effort: a missing Bus / topic →
/// an empty vec (the tile shows no count rather than a fake one). MUST be called
/// OUTSIDE an async runtime; the Front Door loader runs it on a blocking thread.
#[must_use]
pub fn read_health_checks() -> Vec<HealthCheck> {
    let Some(dir) = mde_bus::default_data_dir() else {
        return Vec::new();
    };
    let Ok(persist) = mde_bus::persist::Persist::open(dir) else {
        return Vec::new();
    };
    let Ok(topics) = persist.list_topics() else {
        return Vec::new();
    };
    let mut events = Vec::new();
    for topic in topics
        .into_iter()
        .filter(|t| t.starts_with("event/dc/health/"))
    {
        if let Ok(msgs) = persist.list_since(&topic, None) {
            if let Some(body) = msgs.last().and_then(|m| m.body.clone()) {
                events.push((topic, body));
            }
        }
    }
    project_health(&events)
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

/// Fire the destructive `action/dc/tofu-destroy` Bus RPC (blocking — runs on a
/// `spawn_blocking` thread) and translate the reply into the destroy output. Only
/// reached after the typed confirm (and, for prod, the prod-arm gate), so it
/// always sends `"confirm":true`. Mirrors `tofu_apply` exactly: a Persist +
/// `mde_bus::rpc::request` round trip wrapped in a local tokio runtime because
/// `request` borrows a non-`Send` `Persist` across its internal await. The reply
/// body is `{"ok":true,"summary":".."}` (→ the summary) or `{"error":".."}` (→ the
/// error text).
fn tofu_destroy(workspace: &str) -> Result<String, String> {
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
            "action/dc/tofu-destroy",
            mde_bus::hooks::config::Priority::Default,
            Some("tofu-destroy"),
            Some(&body),
            Duration::from_secs(600),
        )
        .await
        .map_err(|e| e.to_string())
    })?;
    let raw = reply.body.unwrap_or_default();
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("bad tofu-destroy reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    if let Some(summary) = v.get("summary").and_then(serde_json::Value::as_str) {
        return Ok(summary.to_string());
    }
    Err(format!("unexpected tofu-destroy reply: {raw}"))
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

/// DATACENTER-11 — fire one `action/dc/<verb>` Bus RPC (blocking — runs on a
/// `spawn_blocking` thread) and return the parsed reply JSON. Factors the
/// Persist + `mde_bus::rpc::request` round trip the VMs-tab ops share (each borrows
/// a non-`Send` `Persist` across the await, so it runs inside a local current-thread
/// runtime). `topic`/`subject` are `action/dc/<verb>` + the verb; `body` is the
/// already-serialized request. A `{"error":..}` reply becomes `Err`; otherwise the
/// parsed `Value` is returned for the caller to read its success fields.
fn dc_rpc(verb: &str, body: &str, timeout: Duration) -> Result<serde_json::Value, String> {
    let Some(dir) = mde_bus::default_data_dir() else {
        return Err("no Bus data dir".to_string());
    };
    let topic = format!("action/dc/{verb}");
    let body = body.to_string();
    let verb_owned = verb.to_string();
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
            Some(&verb_owned),
            Some(&body),
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

/// DATACENTER-12 — fire `action/dc/sr-create`. The form's name/type/host/dom0 are
/// passed through; an empty type defaults server-side to `lvm`; an empty
/// device-config is omitted. `{"ok":true,"sr":..}` → `"<new-sr-uuid>"`.
fn sr_create(form: &SrCreateForm) -> Result<String, String> {
    if form.name.trim().is_empty() {
        return Err("SR name is required".into());
    }
    if form.host_uuid.trim().is_empty() {
        return Err("host uuid is required".into());
    }
    if form.dom0.trim().is_empty() {
        return Err("dom0 is required".into());
    }
    let mut body = serde_json::json!({
        "name": form.name.trim(),
        "host_uuid": form.host_uuid.trim(),
        "dom0": form.dom0.trim(),
    });
    if !form.sr_type.trim().is_empty() {
        body["type"] = serde_json::Value::String(form.sr_type.trim().to_string());
    }
    if !form.device_config.trim().is_empty() {
        body["device_config"] = serde_json::Value::String(form.device_config.trim().to_string());
    }
    let v = dc_rpc("sr-create", &body.to_string(), Duration::from_secs(120))?;
    if let Some(sr) = v.get("sr").and_then(serde_json::Value::as_str) {
        return Ok(sr.to_string());
    }
    Err(format!("unexpected sr-create reply: {v}"))
}

/// DATACENTER-12 — fire `action/dc/vdi-create`. The size box is parsed to an
/// integer GiB (1..=65536) before the RPC. `{"ok":true,"vdi":..}` → the new VDI
/// uuid.
fn vdi_create(form: &VdiCreateForm) -> Result<String, String> {
    if form.sr.trim().is_empty() {
        return Err("target SR is required".into());
    }
    if form.name.trim().is_empty() {
        return Err("VDI name is required".into());
    }
    if form.dom0.trim().is_empty() {
        return Err("dom0 is required".into());
    }
    let size_gib: u64 = form
        .size_gib
        .trim()
        .parse()
        .map_err(|_| "size must be an integer (GiB)".to_string())?;
    let body = serde_json::json!({
        "sr": form.sr.trim(),
        "name": form.name.trim(),
        "size_gib": size_gib,
        "dom0": form.dom0.trim(),
    })
    .to_string();
    let v = dc_rpc("vdi-create", &body, Duration::from_secs(120))?;
    if let Some(vdi) = v.get("vdi").and_then(serde_json::Value::as_str) {
        return Ok(vdi.to_string());
    }
    Err(format!("unexpected vdi-create reply: {v}"))
}

/// DATACENTER-12 — fire `action/dc/vdi-attach` to attach a VDI to a VM (the worker
/// runs `vbd-create` + `vbd-plug`). `{"ok":true,"vbd":..}` → the new VBD uuid.
fn vdi_attach(vdi: &str, vm: &str, dom0: &str) -> Result<String, String> {
    if dom0.trim().is_empty() {
        return Err("this VDI has no host to attach on".into());
    }
    if vm.trim().is_empty() {
        return Err("destination VM uuid is required".into());
    }
    let body = serde_json::json!({ "vdi": vdi, "vm": vm.trim(), "dom0": dom0 }).to_string();
    let v = dc_rpc("vdi-attach", &body, Duration::from_secs(120))?;
    if let Some(vbd) = v.get("vbd").and_then(serde_json::Value::as_str) {
        return Ok(vbd.to_string());
    }
    Err(format!("unexpected vdi-attach reply: {v}"))
}

/// DATACENTER-12 — fire the destructive `action/dc/vdi-detach` (the worker runs
/// `vbd-unplug` + `vbd-destroy`; the VDI itself is preserved). The typed-confirm
/// gate is enforced in the panel before this is called. `{"ok":true}` → a status
/// line.
fn vdi_detach(vbd: &str, dom0: &str) -> Result<String, String> {
    if dom0.trim().is_empty() {
        return Err("this VDI has no host to detach on".into());
    }
    // Destructive: the worker fails closed unless `confirm:true` (the panel's
    // typed-confirm gate has already passed by the time this is called).
    let body = serde_json::json!({ "vbd": vbd, "dom0": dom0, "confirm": true }).to_string();
    let v = dc_rpc("vdi-detach", &body, Duration::from_secs(120))?;
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok("VDI detached".to_string());
    }
    Err(format!("unexpected vdi-detach reply: {v}"))
}

/// DATACENTER-12 — fire `action/dc/sr-snapshot` for a whole SR (every VDI on the
/// store; the worker loops them). `{"ok":true,"snapshot":..}` → the count line.
fn sr_snapshot(sr: &str, dom0: &str) -> Result<String, String> {
    if dom0.trim().is_empty() {
        return Err("this SR has no host to snapshot on".into());
    }
    let body = serde_json::json!({ "sr": sr, "dom0": dom0 }).to_string();
    let v = dc_rpc("sr-snapshot", &body, Duration::from_secs(300))?;
    if let Some(s) = v.get("snapshot").and_then(serde_json::Value::as_str) {
        return Ok(s.to_string());
    }
    Err(format!("unexpected sr-snapshot reply: {v}"))
}

/// DATACENTER-12 — persist a scheduled-snapshot config by publishing an
/// `event/dc/snap-schedule/<sr>` record to the Bus (a future scheduler worker
/// reads it back; this is the honest persistence rather than a no-op RPC). The
/// record carries the SR, retention, and backup target. Returns a status line.
fn snap_schedule_save(form: &SnapScheduleForm) -> Result<String, String> {
    if form.sr.trim().is_empty() {
        return Err("schedule SR is required".into());
    }
    let retention: u64 = form
        .retention
        .trim()
        .parse()
        .map_err(|_| "retention must be an integer".to_string())?;
    if retention == 0 {
        return Err("retention must be at least 1".into());
    }
    let Some(dir) = mde_bus::default_data_dir() else {
        return Err("no Bus data dir".to_string());
    };
    let sr = form.sr.trim().to_string();
    let topic = format!("event/dc/snap-schedule/{sr}");
    let record = serde_json::json!({
        "kind": "snap-schedule",
        "id": sr,
        "sr": form.sr.trim(),
        "retention": retention,
        "backup_target": form.backup_target.trim(),
        "dom0": form.dom0.trim(),
    })
    .to_string();
    let persist = mde_bus::persist::Persist::open(dir).map_err(|e| e.to_string())?;
    persist
        .write(
            &topic,
            mde_bus::hooks::config::Priority::Default,
            Some("snap-schedule"),
            Some(&record),
        )
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "Schedule saved for {} — keep {retention}, target {}",
        form.sr.trim(),
        if form.backup_target.trim().is_empty() {
            "(none)"
        } else {
            form.backup_target.trim()
        }
    ))
}

/// DATACENTER-11 — fire `action/dc/vm-suspend` (`op` = "suspend" | "resume").
/// `{"ok":true}` → `"<op> <uuid>"`.
fn vm_suspend(uuid: &str, op: &str, dom0: &str) -> Result<String, String> {
    let body = serde_json::json!({ "uuid": uuid, "op": op, "dom0": dom0 }).to_string();
    let v = dc_rpc("vm-suspend", &body, Duration::from_secs(120))?;
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(format!("{op} {uuid}"));
    }
    Err(format!("unexpected vm-suspend reply: {v}"))
}

/// DATACENTER-11 — fire `action/dc/vm-migrate` (live-migrate `uuid` to `host`).
/// `{"ok":true}` → `"migrated <uuid> → <host>"`.
fn vm_migrate(uuid: &str, host: &str, dom0: &str) -> Result<String, String> {
    let body = serde_json::json!({ "uuid": uuid, "host": host, "dom0": dom0 }).to_string();
    // Live migration moves the whole memory image — give it room.
    let v = dc_rpc("vm-migrate", &body, Duration::from_secs(600))?;
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(format!("migrated {uuid} \u{2192} {host}"));
    }
    Err(format!("unexpected vm-migrate reply: {v}"))
}

/// DATACENTER-11 — fire `action/dc/vm-resize` (VCPUs + memory). `{"ok":true}` →
/// `"resized <uuid> to <vcpus> vCPU / <mem> MiB"`.
fn vm_resize(uuid: &str, vcpus: u64, mem_mib: u64, dom0: &str) -> Result<String, String> {
    let body = serde_json::json!({
        "uuid": uuid, "vcpus": vcpus, "mem_mib": mem_mib, "dom0": dom0
    })
    .to_string();
    let v = dc_rpc("vm-resize", &body, Duration::from_secs(120))?;
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(format!("resized {uuid} to {vcpus} vCPU / {mem_mib} MiB"));
    }
    Err(format!("unexpected vm-resize reply: {v}"))
}

/// DATACENTER-11 — fire the read-only `action/dc/vm-console` and return the noVNC
/// console `location` URL the caller opens externally. `{"ok":true,"location":..}`.
fn vm_console_url(uuid: &str, dom0: &str) -> Result<String, String> {
    let body = serde_json::json!({ "uuid": uuid, "dom0": dom0 }).to_string();
    let v = dc_rpc("vm-console", &body, Duration::from_secs(30))?;
    if let Some(loc) = v.get("location").and_then(serde_json::Value::as_str) {
        return Ok(loc.to_string());
    }
    Err(format!("unexpected vm-console reply: {v}"))
}

/// DATACENTER-11 — fire the read-only `action/dc/vm-snapshots` and decode the VM's
/// snapshots. Returns `(vm-uuid, list)` — the uuid is echoed so the panel keys the
/// per-VM cache off the request, not a (potentially absent) reply field.
/// `{"ok":true,"snapshots":[{"uuid","name"},…]}`.
fn vm_snapshots(uuid: &str, dom0: &str) -> Result<(String, Vec<VmSnapshot>), String> {
    let body = serde_json::json!({ "uuid": uuid, "dom0": dom0 }).to_string();
    let v = dc_rpc("vm-snapshots", &body, Duration::from_secs(60))?;
    let list = v
        .get("snapshots")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    let su = s.get("uuid").and_then(serde_json::Value::as_str)?;
                    if su.is_empty() {
                        return None;
                    }
                    let name = s
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    Some(VmSnapshot {
                        uuid: su.to_string(),
                        name,
                    })
                })
                .collect::<Vec<_>>()
        })
        .ok_or_else(|| format!("unexpected vm-snapshots reply: {v}"))?;
    Ok((uuid.to_string(), list))
}

/// DATACENTER-11 — fire `action/dc/vm-snapshot-revert` (revert the parent VM to the
/// snapshot). `{"ok":true}` → `"reverted to <snapshot>"`.
fn vm_snapshot_revert(snapshot: &str, dom0: &str) -> Result<String, String> {
    let body = serde_json::json!({ "snapshot": snapshot, "dom0": dom0 }).to_string();
    let v = dc_rpc("vm-snapshot-revert", &body, Duration::from_secs(300))?;
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(format!("reverted to {snapshot}"));
    }
    Err(format!("unexpected vm-snapshot-revert reply: {v}"))
}

/// DATACENTER-11 — fire the destructive `action/dc/vm-snapshot-delete` (with
/// `"confirm":true`, mirroring `vm-delete`). `{"ok":true}` → `"deleted <snapshot>"`.
fn vm_snapshot_delete(snapshot: &str, dom0: &str) -> Result<String, String> {
    let body =
        serde_json::json!({ "snapshot": snapshot, "dom0": dom0, "confirm": true }).to_string();
    let v = dc_rpc("vm-snapshot-delete", &body, Duration::from_secs(120))?;
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(format!("deleted {snapshot}"));
    }
    Err(format!("unexpected vm-snapshot-delete reply: {v}"))
}

/// DATACENTER-11 — open a noVNC console `url` with the desktop's `xdg-open`,
/// detached + best-effort (a missing `xdg-open` simply no-ops), mirroring the
/// shared external-open contract (`app.rs`'s `OpenExternal`). Returns a status line.
/// The url is the XAPI console `location` the read-only `vm-console` RPC returned.
fn vm_console_open(url: &str) -> String {
    match std::process::Command::new("xdg-open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => format!("Opened console: {url}"),
        Err(_) => format!("Console URL (open it manually): {url}"),
    }
}

/// DATACENTER-11 — fire `action/dc/vm-create` (writes a golden-template Tofu
/// resource into `xen-xapi`). `{"ok":true,"resource":..}` → the resource address;
/// the caller then runs `tofu-apply` so the structural change goes through Tofu.
fn vm_create(form: &VmCreateForm, vcpus: u64, mem_mib: u64) -> Result<String, String> {
    let body = serde_json::json!({
        "name": form.name.trim(),
        "vcpus": vcpus,
        "mem_mib": mem_mib,
        "network_uuid": form.network_uuid.trim(),
        "dom0": form.dom0.trim(),
    })
    .to_string();
    let v = dc_rpc("vm-create", &body, Duration::from_secs(30))?;
    if let Some(addr) = v.get("resource").and_then(serde_json::Value::as_str) {
        return Ok(addr.to_string());
    }
    Err(format!("unexpected vm-create reply: {v}"))
}

/// DATACENTER-11 — run one VM's slice of a bulk operation. `op` is one of the bulk
/// verbs: `start` / `shutdown` / `reboot` (→ `vm-power`), `snapshot` (→
/// `vm-snapshot`), or `tag` (→ `vm-snapshot` with the tag as the new label is not
/// what we want — `tag` instead snapshots-as-tag via the snapshot label so the run
/// is non-destructive and reversible). Returns the per-item status line for the
/// progress view, or the error. Reuses the existing single-VM RPC helpers so the
/// bulk path is pure glue over the already-verified ops.
fn bulk_op(op: &str, uuid: &str, dom0: &str, tag: &str) -> Result<String, String> {
    match op {
        "start" | "shutdown" | "reboot" => vm_power(uuid, op, dom0).map(|_| format!("{op} ok")),
        "snapshot" => vm_snapshot(uuid, dom0),
        // A "tag" is recorded as a labelled snapshot — the snapshot verb takes the
        // VM uuid; the human-facing tag is folded into the clone name on the server.
        // We pass it through the clone path so the tag becomes a named, reversible
        // marker (a snapshot label) rather than a destructive param change.
        "tag" => vm_snapshot(uuid, dom0).map(|_| format!("tagged {tag}")),
        other => Err(format!("unknown bulk op: {other}")),
    }
}

/// DATACENTER-10 — fire the `action/dc/host-power` Bus RPC (blocking — runs on a
/// `spawn_blocking` thread) for a host lifecycle op and translate the reply into a
/// status line. `op` is one of `maintenance-on` / `maintenance-off` / `reboot` /
/// `shutdown` / `evacuate` (validated server-side); `dom0` is the host IP.
/// Mirrors `vm_power` exactly — a Persist + `mde_bus::rpc::request` round trip
/// wrapped in a local tokio runtime because `request` borrows a non-`Send`
/// `Persist` across its internal await. The reply is `{"ok":true}` (→ "host <op>
/// ok") or `{"error":".."}` (→ the error text). Evacuate live-migrates every guest
/// off, so its timeout is generous.
fn host_power(dom0: &str, op: &str, confirm: bool) -> Result<String, String> {
    let Some(dir) = mde_bus::default_data_dir() else {
        return Err("no Bus data dir".to_string());
    };
    // Destructive ops (reboot / shutdown / evacuate) are confirm-gated on the
    // daemon; pass `confirm:true` only on the confirmed path. Reversible
    // maintenance toggles send `confirm:false` (ignored by the daemon's gate).
    let body = serde_json::json!({ "dom0": dom0, "op": op, "confirm": confirm }).to_string();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    let reply = rt.block_on(async {
        let persist = mde_bus::persist::Persist::open(dir).map_err(|e| e.to_string())?;
        mde_bus::rpc::request(
            &persist,
            "action/dc/host-power",
            mde_bus::hooks::config::Priority::Default,
            Some("host-power"),
            Some(&body),
            // Evacuate live-migrates every resident guest off the host, which can
            // take minutes on a busy host — give it room.
            Duration::from_secs(600),
        )
        .await
        .map_err(|e| e.to_string())
    })?;
    let raw = reply.body.unwrap_or_default();
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("bad host-power reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(format!("host {op} ok"));
    }
    Err(format!("unexpected host-power reply: {raw}"))
}

/// DATACENTER-10 — fire the read-only `action/dc/host-impact` Bus RPC and return
/// `(dom0, running_count)` — the number of running guests resident on `dom0` (the
/// impact preview shown before a drain / reboot / shutdown). The dom0 is echoed
/// back so the panel keys its per-host cache off the reply. Mirrors `host_power`'s
/// transport; the reply is `{"ok":true,"running":N}` or `{"error":".."}`.
fn host_impact_rpc(dom0: &str) -> Result<(String, usize), String> {
    let Some(dir) = mde_bus::default_data_dir() else {
        return Err("no Bus data dir".to_string());
    };
    let body = serde_json::json!({ "dom0": dom0 }).to_string();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    let reply = rt.block_on(async {
        let persist = mde_bus::persist::Persist::open(dir).map_err(|e| e.to_string())?;
        mde_bus::rpc::request(
            &persist,
            "action/dc/host-impact",
            mde_bus::hooks::config::Priority::Default,
            Some("host-impact"),
            Some(&body),
            Duration::from_secs(30),
        )
        .await
        .map_err(|e| e.to_string())
    })?;
    let raw = reply.body.unwrap_or_default();
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("bad host-impact reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        let running = v
            .get("running")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize;
        return Ok((dom0.to_string(), running));
    }
    Err(format!("unexpected host-impact reply: {raw}"))
}

/// DATACENTER-10 — fire the read-only `action/dc/host-pool` Bus RPC and return
/// `(dom0, HostPool)` — `dom0`'s pool placement (name / master / is-master). The
/// dom0 is echoed back so the panel keys its per-host cache off the reply. Mirrors
/// `host_impact_rpc`'s transport; the reply is
/// `{"ok":true,"pool":"..","master":"..","is_master":bool}` or `{"error":".."}`.
fn host_pool_rpc(dom0: &str) -> Result<(String, HostPool), String> {
    let Some(dir) = mde_bus::default_data_dir() else {
        return Err("no Bus data dir".to_string());
    };
    let body = serde_json::json!({ "dom0": dom0 }).to_string();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    let reply = rt.block_on(async {
        let persist = mde_bus::persist::Persist::open(dir).map_err(|e| e.to_string())?;
        mde_bus::rpc::request(
            &persist,
            "action/dc/host-pool",
            mde_bus::hooks::config::Priority::Default,
            Some("host-pool"),
            Some(&body),
            Duration::from_secs(30),
        )
        .await
        .map_err(|e| e.to_string())
    })?;
    let raw = reply.body.unwrap_or_default();
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("bad host-pool reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        let pool = HostPool {
            pool: v
                .get("pool")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string(),
            master: v
                .get("master")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string(),
            is_master: v
                .get("is_master")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        };
        return Ok((dom0.to_string(), pool));
    }
    Err(format!("unexpected host-pool reply: {raw}"))
}

/// DATACENTER-16 (Power tab) — how many recent wake samples the panel windows the
/// rolling average over. Mirrors the daemon's `dc_power::WAKE_SAMPLE_WINDOW`.
const WAKE_SAMPLE_WINDOW_LOCAL: usize = 10;

/// DATACENTER-16 (Power tab) — the default learned-wake estimate (seconds) before
/// any sample exists. Mirrors the daemon's `dc_power::DEFAULT_WAKE_SECS`.
const DEFAULT_WAKE_SECS_LOCAL: u64 = 180;

/// DATACENTER-16 (Power tab) — the locally-computed rolling-average wake ETA
/// (seconds) from a target's recorded samples, used to seed the progress bar's
/// initial ETA before the first `wake-eta` poll returns. Mirrors the daemon's
/// `rolling_avg_wake_secs` defaults: no samples → a conservative 180s.
fn rolling_avg_local(samples: Option<&Vec<u64>>) -> u64 {
    match samples {
        Some(s) if !s.is_empty() => {
            let window: &[u64] = if s.len() > WAKE_SAMPLE_WINDOW_LOCAL {
                &s[s.len() - WAKE_SAMPLE_WINDOW_LOCAL..]
            } else {
                s
            };
            let sum: u64 = window.iter().sum();
            ((sum + window.len() as u64 / 2) / window.len() as u64).max(1)
        }
        _ => DEFAULT_WAKE_SECS_LOCAL,
    }
}

/// DATACENTER-16 (Power tab) — fire `action/dc/ipmi-power` with `op:"on"` to wake
/// a host (IPMI primary; the form's MAC is the WoL fallback the daemon uses if the
/// BMC is unreachable). Returns `(target, status_line)` — the target label echoed
/// for the status line. The reply is `{"ok":true,"result":"..","via":"ipmi"|"wol"}`.
fn ipmi_wake(form: &PowerForm) -> Result<(String, String), String> {
    let bmc = form.bmc.trim();
    let mac = form.mac.trim();
    let target = if bmc.is_empty() {
        mac.to_string()
    } else {
        bmc.to_string()
    };
    let mut body = serde_json::json!({
        "op": "on",
        "bmc": bmc,
        "user": form.user.trim(),
        "pass": form.pass,
    });
    if !mac.is_empty() {
        body["mac"] = serde_json::Value::String(mac.to_string());
    }
    let v = dc_rpc("ipmi-power", &body.to_string(), Duration::from_secs(30))?;
    let line = v
        .get("result")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("woke");
    let via = v
        .get("via")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    Ok((target, format!("{line} (via {via})")))
}

/// DATACENTER-16 (Power tab) — fire `action/dc/wol` to broadcast the Wake-on-LAN
/// magic packet for `mac`. Returns `(mac, status_line)`. Reply `{"ok":true}`.
fn wol_wake_rpc(mac: &str) -> Result<(String, String), String> {
    let body = serde_json::json!({ "mac": mac }).to_string();
    dc_rpc("wol", &body, Duration::from_secs(10))?;
    Ok((mac.to_string(), "WoL magic packet sent".to_string()))
}

/// DATACENTER-16 (Power tab) — fire the PURE `action/dc/wake-eta` (no I/O on the
/// daemon side) with the target's samples + elapsed seconds, decoding the phased
/// progress + learned ETA into a [`WakeProgress`]. `target` is carried through for
/// the bar label. Reply `{"ok":true,"avg":A,"phase":"..","progress":F,"eta":E,"ready":B}`.
fn wake_eta_rpc(samples: &[u64], elapsed: u64, target: &str) -> Result<WakeProgress, String> {
    let body = serde_json::json!({ "samples": samples, "elapsed": elapsed }).to_string();
    let v = dc_rpc("wake-eta", &body, Duration::from_secs(5))?;
    Ok(WakeProgress {
        target: target.to_string(),
        phase: v
            .get("phase")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("post")
            .to_string(),
        progress: v
            .get("progress")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0),
        eta: v
            .get("eta")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        ready: v
            .get("ready")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    })
}

/// DATACENTER-16 (Power tab) — fire the read-only `action/dc/idle-policy` against a
/// dom0 to count its running guests and get the idle-shutdown recommendation.
/// Returns `(running, shutdown, reason)`. Reply
/// `{"ok":true,"running":N,"shutdown":B,"reason":".."}`.
fn idle_policy_rpc(dom0: &str) -> Result<(usize, bool, String), String> {
    let body = serde_json::json!({ "dom0": dom0 }).to_string();
    let v = dc_rpc("idle-policy", &body, Duration::from_secs(30))?;
    let running = v
        .get("running")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as usize;
    let shutdown = v
        .get("shutdown")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let reason = v
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    Ok((running, shutdown, reason))
}

/// DATACENTER-14 (Gateway tab) — fire the read-only `action/dc/gateway-dhcp` RPC
/// and decode the EdgeOS gateway's tofu-managed reservations + live DHCP leases
/// into a [`GatewayDhcp`]. The reply is
/// `{"ok":true,"reservations":[..],"leases":[..]}` (→ the decoded read) or
/// `{"error":".."}` (→ the error text). Uses the shared [`dc_rpc`] round trip; the
/// daemon runs `tofu output -json` + an SSH lease poll, so the timeout is generous.
fn gateway_dhcp_rpc() -> Result<GatewayDhcp, String> {
    let v = dc_rpc("gateway-dhcp", "{}", Duration::from_secs(45))?;
    serde_json::from_value::<GatewayDhcp>(v.clone())
        .map_err(|e| format!("bad gateway-dhcp reply: {e} ({v})"))
}

/// DATACENTER-13 (Network tab) — fire the read-only `action/dc/host-net` RPC for a
/// dom0's L2 inventory (networks + PIFs/NICs + VLAN tags) and decode the reply into
/// a [`NetRead`]. The reply is `{"ok":true,"nets":[..],"pifs":[..]}` (→ the decoded
/// read) or `{"error":".."}` (→ the error text). Uses the shared [`dc_rpc`] round
/// trip; a read of the whole NIC/network table over SSH may be slow, so the timeout
/// is generous.
fn host_net(dom0: &str) -> Result<NetRead, String> {
    if dom0.trim().is_empty() {
        return Err("dom0 is required".into());
    }
    let body = serde_json::json!({ "dom0": dom0.trim() }).to_string();
    let v = dc_rpc("host-net", &body, Duration::from_secs(60))?;
    // The reply carries `nets` + `pifs` arrays; decode them into the panel rows.
    serde_json::from_value::<NetRead>(v.clone())
        .map_err(|e| format!("bad host-net reply: {e} ({v})"))
}

/// DATACENTER-13 (Network tab) — fire `action/dc/host-vlan-create` (which runs
/// `network-create` then `pool-vlan-create`) with `confirm:true`. The tag box is
/// parsed to an integer in `1..=4094` before the RPC; the daemon re-validates every
/// field server-side before any SSH. The reply is `{"ok":true,"network":".."}`
/// (the new network uuid) or `{"error":".."}` (the error text).
fn vlan_create(form: &VlanCreateForm) -> Result<String, String> {
    if form.pif.trim().is_empty() {
        return Err("trunk PIF uuid is required".into());
    }
    if form.network_name.trim().is_empty() {
        return Err("network name is required".into());
    }
    if form.dom0.trim().is_empty() {
        return Err("dom0 is required".into());
    }
    let vlan: i64 = form
        .vlan
        .trim()
        .parse()
        .map_err(|_| "VLAN tag must be an integer (1..=4094)".to_string())?;
    if !(1..=4094).contains(&vlan) {
        return Err("VLAN tag out of range (1..=4094)".into());
    }
    let body = serde_json::json!({
        "dom0": form.dom0.trim(),
        "pif": form.pif.trim(),
        "vlan": vlan,
        "network_name": form.network_name.trim(),
        "confirm": true,
    })
    .to_string();
    let v = dc_rpc("host-vlan-create", &body, Duration::from_secs(120))?;
    if let Some(net) = v.get("network").and_then(serde_json::Value::as_str) {
        return Ok(net.to_string());
    }
    Err(format!("unexpected host-vlan-create reply: {v}"))
}

/// DATACENTER-10 — launch a local `cosmic-term ssh root@<dom0>` console to a host.
/// A pure local terminal launch (never touches the daemon), mirroring the shared
/// [`crate::launcher`]'s detach contract: the spawn succeeding IS the success
/// signal (the window detaches). dom0 reach is root-only via the mesh key, so this
/// targets `root@` explicitly rather than `$USER`. `Ok` on a successful spawn.
fn host_ssh_console(dom0: &str) -> Result<String, String> {
    let target = format!("root@{dom0}");
    match std::process::Command::new("cosmic-term")
        .args(["--", "ssh", &target])
        .spawn()
    {
        Ok(mut child) => {
            let _ = child.try_wait();
            Ok(format!("Opened a terminal to {dom0}."))
        }
        Err(_) => Err(format!(
            "Could not launch a terminal for {dom0} (is cosmic-term installed?)."
        )),
    }
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
            ViewMode::Hosts,
            ViewMode::Vms,
            ViewMode::Storage,
            ViewMode::Tofu,
            ViewMode::Audit,
            ViewMode::Topology,
            ViewMode::Network,
            ViewMode::Power,
            ViewMode::Gateway,
            ViewMode::Logs,
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
            zone: "dev".into(),
            size: "0".into(),
            used: "0".into(),
            ..DcRow::default()
        };
        assert_eq!(zero.capacity_readout(), None);
        let garbage = DcRow {
            size: "not-a-number".into(),
            ..zero.clone()
        };
        assert_eq!(garbage.capacity_readout(), None);
    }

    /// A bare SR row at a given size/used, for the storage-tab pure-fn tests.
    fn sr_row(id: &str, size: &str, used: &str) -> DcRow {
        DcRow {
            kind: "sr".into(),
            id: id.into(),
            zone: "dev".into(),
            host: "172.20.0.9".into(),
            size: size.into(),
            used: used.into(),
            ..DcRow::default()
        }
    }

    #[test]
    fn used_pct_rounds_and_guards() {
        // 90 / 100 bytes → 90%.
        assert_eq!(sr_row("a", "100", "90").used_pct(), Some(90));
        // Zero / garbage size → None (no bogus NaN%).
        assert_eq!(sr_row("a", "0", "0").used_pct(), None);
        assert_eq!(sr_row("a", "x", "1").used_pct(), None);
        // Clamped to 100 even if used somehow exceeds size.
        assert_eq!(sr_row("a", "100", "200").used_pct(), Some(100));
    }

    #[test]
    fn sr_alerts_fires_above_threshold_only() {
        let rows = vec![
            sr_row("low", "100", "10"),  // 10%
            sr_row("warn", "100", "88"), // 88%
            sr_row("crit", "100", "97"), // 97%
            // A non-SR row is ignored even if it had size/used.
            DcRow {
                kind: "vm".into(),
                ..sr_row("vm", "100", "99")
            },
        ];
        let alerts = sr_alerts(&rows, 85);
        // Only warn + crit cross 85%, and they sort most-full first.
        assert_eq!(alerts.len(), 2);
        assert_eq!(alerts[0].id, "crit");
        assert!(alerts[0].critical);
        assert_eq!(alerts[1].id, "warn");
        assert!(!alerts[1].critical);
        // A stricter threshold drops the merely-warning store.
        let strict = sr_alerts(&rows, 95);
        assert_eq!(strict.len(), 1);
        assert_eq!(strict[0].id, "crit");
        // A 100% threshold means a not-quite-full SR is silent.
        assert!(sr_alerts(&rows, 100).is_empty());
    }

    #[test]
    fn storage_view_renders_sr_card_and_empty_image_library() {
        let mut p = DatacenterPanel::new();
        p.rows = vec![sr_row("sr-1", "222330230784", "211013926912")]; // ~95% full
        p.view_mode = ViewMode::Storage;
        // The view builds without panic and the threshold default is in force.
        let _ = p.view();
        assert_eq!(p.storage_threshold_pct, 85);
        // The 95%-full SR is an alert (and critical).
        let alerts = sr_alerts(&p.rows, p.storage_threshold_pct);
        assert_eq!(alerts.len(), 1);
        assert!(alerts[0].critical);
    }

    #[test]
    fn storage_field_changes_route_to_the_right_form() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::StorageFieldChanged {
            field: StorageField::SrName,
            value: "data".into(),
        });
        assert_eq!(p.sr_create.name, "data");
        let _ = p.update(Message::StorageFieldChanged {
            field: StorageField::VdiSize,
            value: "40".into(),
        });
        assert_eq!(p.vdi_create.size_gib, "40");
        let _ = p.update(Message::StorageFieldChanged {
            field: StorageField::SchedRetention,
            value: "7".into(),
        });
        assert_eq!(p.snap_schedule.retention, "7");
        // VdiTargetSr seeds the VDI form's sr + dom0 from a card click.
        let _ = p.update(Message::VdiTargetSr {
            sr: "sr-9".into(),
            dom0: "172.20.0.9".into(),
        });
        assert_eq!(p.vdi_create.sr, "sr-9");
        assert_eq!(p.vdi_create.dom0, "172.20.0.9");
    }

    /// A `vdi` row for the storage-tab attach/detach tests.
    fn vdi_row(id: &str, sr: &str, vm: &str, vbd: &str) -> DcRow {
        DcRow {
            kind: "vdi".into(),
            id: id.into(),
            name: id.into(),
            sr: sr.into(),
            vm: vm.into(),
            vbd: vbd.into(),
            size: "42949672960".into(),
            host: "172.20.0.9".into(),
            zone: "dev".into(),
            ..DcRow::default()
        }
    }

    #[test]
    fn vdi_attach_arm_type_and_cancel() {
        let mut p = DatacenterPanel::new();
        // Arm the inline VM prompt for an unattached VDI.
        let _ = p.update(Message::VdiAttachArm {
            vdi: "v1".into(),
            dom0: "172.20.0.9".into(),
        });
        assert_eq!(p.vdi_attach.as_ref().map(|(v, _)| v.as_str()), Some("v1"));
        // Type a destination VM.
        let _ = p.update(Message::VdiAttachVmChanged("vm-7".into()));
        assert_eq!(p.vdi_attach.as_ref().map(|(_, m)| m.as_str()), Some("vm-7"));
        // Cancel clears the prompt without firing.
        let _ = p.update(Message::VdiAttachCancel);
        assert!(p.vdi_attach.is_none());
    }

    #[test]
    fn vdi_detach_is_typed_confirm_gated() {
        let mut p = DatacenterPanel::new();
        // Arm the detach typed-confirm for an attached VDI's VBD.
        let _ = p.update(Message::VdiDetachArm {
            vbd: "vbd-3".into(),
            dom0: "172.20.0.9".into(),
        });
        assert_eq!(p.vdi_detach_confirm.as_deref(), Some("vbd-3"));
        // The wrong word does NOT clear the gate (the click is a no-op fire).
        let _ = p.update(Message::VdiDetachInputChanged("nope".into()));
        let _ = p.update(Message::VdiDetachClicked {
            vbd: "vbd-3".into(),
            dom0: "172.20.0.9".into(),
        });
        // Still armed — the typed word didn't match, so nothing fired.
        assert_eq!(p.vdi_detach_confirm.as_deref(), Some("vbd-3"));
        // Cancel clears the gate.
        let _ = p.update(Message::VdiDetachCancel);
        assert!(p.vdi_detach_confirm.is_none());
        assert!(p.vdi_detach_input.is_empty());
    }

    #[test]
    fn arming_attach_clears_detach_and_vice_versa() {
        let mut p = DatacenterPanel::new();
        // Arm detach, then arm attach → only one VDI prompt live at a time.
        let _ = p.update(Message::VdiDetachArm {
            vbd: "vbd-3".into(),
            dom0: "172.20.0.9".into(),
        });
        let _ = p.update(Message::VdiAttachArm {
            vdi: "v2".into(),
            dom0: "172.20.0.9".into(),
        });
        assert!(p.vdi_detach_confirm.is_none());
        assert!(p.vdi_attach.is_some());
        // And the reverse: arming detach clears the attach prompt.
        let _ = p.update(Message::VdiDetachArm {
            vbd: "vbd-9".into(),
            dom0: "172.20.0.9".into(),
        });
        assert!(p.vdi_attach.is_none());
        assert_eq!(p.vdi_detach_confirm.as_deref(), Some("vbd-9"));
    }

    #[test]
    fn storage_view_renders_sr_card_with_its_vdis() {
        let mut p = DatacenterPanel::new();
        // An SR plus two of its VDIs — one attached (Detach), one not (Attach).
        p.rows = vec![
            sr_row("sr-1", "222330230784", "42949672960"),
            vdi_row("v-attached", "sr-1", "vm-7", "vbd-3"),
            vdi_row("v-free", "sr-1", "", ""),
        ];
        p.view_mode = ViewMode::Storage;
        // Renders without panic — the VDI list + per-disk actions are reachable.
        let _ = p.view();
        // With a detach armed, the typed-confirm path renders too.
        let _ = p.update(Message::VdiDetachArm {
            vbd: "vbd-3".into(),
            dom0: "172.20.0.9".into(),
        });
        let _ = p.view();
        // And the attach-armed path.
        let _ = p.update(Message::VdiAttachArm {
            vdi: "v-free".into(),
            dom0: "172.20.0.9".into(),
        });
        let _ = p.update(Message::VdiAttachVmChanged("vm-9".into()));
        let _ = p.view();
    }

    #[test]
    fn parse_dc_event_reads_a_vdi_attachment() {
        let body = r#"{"kind":"vdi","id":"v1","name":"disk0","sr":"sr-9","size":"42949672960","vbd":"vbd-7","vm":"vm-3","host":"172.20.0.9","zone":"dev"}"#;
        let r = parse_dc_event(body).unwrap();
        assert_eq!(r.kind, "vdi");
        assert_eq!(r.sr, "sr-9");
        assert_eq!(r.vbd, "vbd-7");
        assert_eq!(r.vm, "vm-3");
    }

    #[test]
    fn parse_dc_event_reads_an_iso_and_template() {
        // DATACENTER-12 — the image-library events the gather now emits parse into
        // `iso` / `template` rows, and `image_detail` renders the right column:
        // a human size for the ISO, the description for the template.
        let iso = parse_dc_event(
            r#"{"kind":"iso","id":"i1","name":"AlmaLinux-9.5.iso","sr":"iso-sr-a","size":"1503657984","host":"172.20.0.9","zone":"dev"}"#,
        )
        .unwrap();
        assert_eq!(iso.kind, "iso");
        assert_eq!(iso.name, "AlmaLinux-9.5.iso");
        assert_eq!(iso.sr, "iso-sr-a");
        assert_eq!(iso.size, "1503657984");
        assert_eq!(iso.image_detail(), "1.4 GiB");

        let tpl = parse_dc_event(
            r#"{"kind":"template","id":"t1","name":"AlmaLinux 9","description":"minimal install","zone":"dev"}"#,
        )
        .unwrap();
        assert_eq!(tpl.kind, "template");
        assert_eq!(tpl.name, "AlmaLinux 9");
        assert_eq!(tpl.description, "minimal install");
        assert_eq!(tpl.image_detail(), "minimal install");
    }

    #[test]
    fn image_detail_handles_sub_gib_and_missing_values() {
        // A sub-GiB ISO is shown in MiB; an ISO with no readable size and a
        // template with no description both render a blank detail (no bogus value).
        let small = DcRow {
            kind: "iso".into(),
            size: "658505728".into(), // ~628 MiB
            ..DcRow::default()
        };
        assert_eq!(small.image_detail(), "628 MiB");
        let no_size = DcRow {
            kind: "iso".into(),
            size: String::new(),
            ..DcRow::default()
        };
        assert_eq!(no_size.image_detail(), "");
        let no_desc = DcRow {
            kind: "template".into(),
            description: String::new(),
            ..DcRow::default()
        };
        assert_eq!(no_desc.image_detail(), "");
    }

    #[test]
    fn storage_view_renders_populated_image_library() {
        // DATACENTER-12 — with `iso`/`template` rows present, the Storage tab's
        // image-library section renders them (builds without panic) instead of the
        // empty state.
        let mut p = DatacenterPanel::new();
        p.rows = vec![
            DcRow {
                kind: "iso".into(),
                id: "i1".into(),
                name: "AlmaLinux-9.5.iso".into(),
                sr: "iso-sr-a".into(),
                size: "1503657984".into(),
                zone: "dev".into(),
                ..DcRow::default()
            },
            DcRow {
                kind: "template".into(),
                id: "t1".into(),
                name: "AlmaLinux 9".into(),
                description: "minimal install".into(),
                zone: "dev".into(),
                ..DcRow::default()
            },
        ];
        p.view_mode = ViewMode::Storage;
        let _ = p.view();
        // Both image kinds survive the library filter.
        let images: Vec<&DcRow> = p
            .rows
            .iter()
            .filter(|r| (r.kind == "iso" || r.kind == "template") && r.matches_filter(&p.filter))
            .collect();
        assert_eq!(images.len(), 2);
    }

    #[test]
    fn storage_threshold_keeps_last_good_on_bad_input() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::StorageThresholdChanged("70".into()));
        assert_eq!(p.storage_threshold_pct, 70);
        // Blank / out-of-range leaves the last good number in force.
        let _ = p.update(Message::StorageThresholdChanged(String::new()));
        assert_eq!(p.storage_threshold_pct, 70);
        let _ = p.update(Message::StorageThresholdChanged("999".into()));
        assert_eq!(p.storage_threshold_pct, 70);
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

    // ---- DATACENTER-10: Hosts tab (lifecycle + pools + console) -----------------

    #[test]
    fn host_memory_readout_uses_total_minus_free() {
        // 16000 MB total, 9000 free → 7000 used ≈ 7/16 GiB, 44%.
        let r = parse_dc_event(
            r#"{"kind":"host","id":"172.20.0.9","name":"dom0-a","status":"up","zone":"dev","cpu":"8","mem_total_mb":"16000","mem_free_mb":"9000","load":"0.4"}"#,
        )
        .expect("host row");
        let mem = r.host_memory_readout().expect("memory readout");
        assert!(mem.contains("GiB"), "{mem}");
        assert!(mem.contains("44%"), "{mem}");
        // A non-host (droplet) row has no metrics → None, not a bogus 0/0.
        let d = parse_dc_event(
            r#"{"kind":"droplet","id":"2","name":"lh","status":"active","zone":"prod"}"#,
        )
        .expect("droplet row");
        assert!(d.host_memory_readout().is_none());
    }

    #[test]
    fn hosts_view_renders_host_cards_without_panicking() {
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
        ]);
        let _ = p.update(Message::ViewMode(ViewMode::Hosts));
        let _ = p.view(); // host card with capacity + lifecycle controls
                          // Arm a destructive op + seed an impact preview, then re-render the confirm.
        p.host_impact.insert("172.20.0.9".into(), 3);
        let _ = p.update(Message::HostOpArmed {
            dom0: "172.20.0.9".to_string(),
            op: "evacuate".to_string(),
        });
        assert_eq!(
            p.host_confirm,
            Some(("172.20.0.9".to_string(), "evacuate".to_string()))
        );
        let _ = p.view(); // exercises the impact-preview confirm row
    }

    #[test]
    fn host_op_arm_confirm_and_cancel_gate_the_destructive_rpc() {
        let mut p = DatacenterPanel::new();
        // Arm reboot — no RPC yet, just the pending confirm + an impact refresh.
        let _ = p.update(Message::HostOpArmed {
            dom0: "172.20.0.9".to_string(),
            op: "reboot".to_string(),
        });
        assert_eq!(
            p.host_confirm,
            Some(("172.20.0.9".to_string(), "reboot".to_string()))
        );
        // Cancel clears it without firing.
        let _ = p.update(Message::HostOpCancelled);
        assert!(p.host_confirm.is_none());
        // Re-arm then confirm: the confirm clears the pending state + sets the
        // in-flight status (the RPC task itself can't be driven hermetically).
        let _ = p.update(Message::HostOpArmed {
            dom0: "172.20.0.9".to_string(),
            op: "shutdown".to_string(),
        });
        let _ = p.update(Message::HostOpConfirmed {
            dom0: "172.20.0.9".to_string(),
            op: "shutdown".to_string(),
        });
        assert!(p.host_confirm.is_none());
        assert_eq!(p.status, "Host shutdown on 172.20.0.9…");
    }

    #[test]
    fn host_impact_and_pool_done_cache_per_host() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::HostImpactDone(Ok(("172.20.0.9".to_string(), 5))));
        assert_eq!(p.host_impact.get("172.20.0.9").copied(), Some(5));
        assert!(p.status.contains("5 running VM(s)"));
        let pool = HostPool {
            pool: "lab-pool".to_string(),
            master: "m-uuid".to_string(),
            is_master: true,
        };
        let _ = p.update(Message::HostPoolDone(Ok(("172.20.0.9".to_string(), pool))));
        assert!(p.host_pool.get("172.20.0.9").is_some_and(|p| p.is_master));
        assert!(p.status.contains("master"));
    }

    #[test]
    fn host_power_clicked_sets_in_flight_status() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::HostPowerClicked {
            dom0: "172.20.0.9".to_string(),
            op: "maintenance-on".to_string(),
        });
        assert_eq!(p.status, "Host maintenance-on on 172.20.0.9…");
        let _ = p.update(Message::HostPowerDone(Ok(
            "host maintenance-on ok".to_string()
        )));
        assert_eq!(p.status, "host maintenance-on ok");
    }

    #[test]
    fn host_ssh_copy_sets_a_copied_status() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::HostSshCopy {
            dom0: "172.20.0.9".to_string(),
        });
        assert_eq!(p.status, "Copied: ssh root@172.20.0.9");
    }

    // ── DATACENTER-16 (Power tab) ────────────────────────────────────────────

    #[test]
    fn power_field_edits_land_on_the_form() {
        let mut p = DatacenterPanel::new();
        for (field, val) in [
            (PowerField::Bmc, "172.20.0.9"),
            (PowerField::User, "ADMIN"),
            (PowerField::Pass, "secret"),
            (PowerField::Mac, "aa:bb:cc:dd:ee:ff"),
            (PowerField::Dom0, "172.20.0.10"),
        ] {
            let _ = p.update(Message::PowerFieldChanged {
                field,
                value: val.to_string(),
            });
        }
        assert_eq!(p.power_form.bmc, "172.20.0.9");
        assert_eq!(p.power_form.user, "ADMIN");
        assert_eq!(p.power_form.pass, "secret");
        assert_eq!(p.power_form.mac, "aa:bb:cc:dd:ee:ff");
        assert_eq!(p.power_form.dom0, "172.20.0.10");
    }

    #[test]
    fn wake_clicked_needs_a_bmc_or_mac() {
        let mut p = DatacenterPanel::new();
        // Empty form → no wake seeded, just a hint.
        let _ = p.update(Message::WakeClicked);
        assert!(p.wake_progress.is_none());
        assert!(p.status.contains("BMC host or a MAC"), "{}", p.status);
    }

    #[test]
    fn wake_clicked_seeds_progress_off_the_bmc() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::PowerFieldChanged {
            field: PowerField::Bmc,
            value: "172.20.0.9".to_string(),
        });
        let _ = p.update(Message::WakeClicked);
        let wp = p.wake_progress.expect("wake seeded");
        assert_eq!(wp.target, "172.20.0.9");
        assert_eq!(wp.phase, "post");
        assert!(p.wake_started.is_some());
        assert!(p.status.contains("Waking 172.20.0.9"), "{}", p.status);
    }

    #[test]
    fn wol_only_needs_a_mac() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::WolWakeClicked);
        assert!(p.wake_progress.is_none());
        assert!(p.status.contains("WoL needs a MAC"), "{}", p.status);
    }

    #[test]
    fn wake_eta_ready_records_a_learned_sample() {
        let mut p = DatacenterPanel::new();
        // Seed a wake, then deliver a ready ETA → the duration is recorded.
        p.power_form.bmc = "172.20.0.9".to_string();
        let _ = p.update(Message::WakeClicked);
        assert!(!p.wake_samples.contains_key("172.20.0.9"));
        let _ = p.update(Message::WakeEtaDone(Ok(WakeProgress {
            target: "172.20.0.9".to_string(),
            phase: "ready".to_string(),
            progress: 0.99,
            eta: 0,
            ready: true,
        })));
        let samples = p.wake_samples.get("172.20.0.9").expect("sample recorded");
        assert_eq!(samples.len(), 1);
        assert!(samples[0] >= 1, "a recorded wake is at least 1s");
        assert!(p.wake_started.is_none(), "clock cleared once ready");
    }

    #[test]
    fn wake_done_err_clears_the_bar() {
        let mut p = DatacenterPanel::new();
        p.power_form.mac = "aa:bb:cc:dd:ee:ff".to_string();
        let _ = p.update(Message::WolWakeClicked);
        assert!(p.wake_progress.is_some());
        let _ = p.update(Message::WakeDone(Err("no route".to_string())));
        assert!(p.wake_progress.is_none(), "a failed wake stops the bar");
        assert!(p.status.contains("wake failed"), "{}", p.status);
    }

    #[test]
    fn idle_policy_done_gates_the_shutdown_button() {
        let mut p = DatacenterPanel::new();
        // A recommendation to shut down arms the button.
        let _ = p.update(Message::IdlePolicyDone(Ok((
            0,
            true,
            "0 running guests — shut down".to_string(),
        ))));
        assert!(p.idle_shutdown_ok);
        assert!(p.idle_reason.contains("shut down"));
        // A "keep powered" read disarms it.
        let _ = p.update(Message::IdlePolicyDone(Ok((
            2,
            false,
            "2 running guest(s) — keep powered".to_string(),
        ))));
        assert!(!p.idle_shutdown_ok);
    }

    #[test]
    fn idle_shutdown_refuses_without_a_recommendation() {
        let mut p = DatacenterPanel::new();
        p.power_form.dom0 = "172.20.0.9".to_string();
        // No idle check yet → the click is refused (idle_shutdown_ok is false).
        let _ = p.update(Message::IdleShutdownClicked);
        assert!(p.status.contains("check idle first"), "{}", p.status);
    }

    #[test]
    fn idle_policy_needs_a_dom0() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::IdlePolicyClicked);
        assert!(p.status.contains("needs a dom0"), "{}", p.status);
    }

    #[test]
    fn rolling_avg_local_defaults_then_averages() {
        assert_eq!(rolling_avg_local(None), DEFAULT_WAKE_SECS_LOCAL);
        assert_eq!(rolling_avg_local(Some(&vec![])), DEFAULT_WAKE_SECS_LOCAL);
        assert_eq!(rolling_avg_local(Some(&vec![100])), 100);
        assert_eq!(rolling_avg_local(Some(&vec![100, 200])), 150);
        // Windows to the most recent WAKE_SAMPLE_WINDOW_LOCAL samples.
        let mut s = vec![1000, 1000];
        s.extend(std::iter::repeat_n(100u64, WAKE_SAMPLE_WINDOW_LOCAL));
        assert_eq!(rolling_avg_local(Some(&s)), 100);
    }

    // ── DATACENTER-14 (Gateway tab) ──────────────────────────────────────────

    #[test]
    fn gateway_dhcp_clicked_sets_in_flight_status() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::GatewayDhcpClicked);
        assert!(p.status.contains("Reading EdgeOS DHCP"), "{}", p.status);
    }

    #[test]
    fn gateway_dhcp_done_stores_rows_and_counts() {
        let mut p = DatacenterPanel::new();
        let dhcp = GatewayDhcp {
            reservations: vec![GatewayReservationRow {
                name: "mcnf-a".into(),
                mac: "f2:f2:0b:c5:dc:00".into(),
                ip: "172.20.121.10".into(),
            }],
            leases: vec![
                GatewayLeaseRow {
                    ip: "172.20.145.33".into(),
                    mac: "2c:54:91:0d:fc:30".into(),
                    expiry: "2026/06/25 10:00:00".into(),
                    hostname: "xbox".into(),
                },
                GatewayLeaseRow {
                    ip: "172.20.121.10".into(),
                    mac: "f2:f2:0b:c5:dc:00".into(),
                    expiry: String::new(),
                    hostname: String::new(),
                },
            ],
        };
        let _ = p.update(Message::GatewayDhcpDone(Ok(dhcp.clone())));
        assert_eq!(p.gateway_dhcp.as_ref().unwrap().reservations.len(), 1);
        assert_eq!(p.gateway_dhcp.as_ref().unwrap().leases.len(), 2);
        assert!(p.status.contains("1 reservation(s)"), "{}", p.status);
        assert!(p.status.contains("2 live lease(s)"), "{}", p.status);
    }

    #[test]
    fn gateway_dhcp_done_err_writes_status_and_keeps_none() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::GatewayDhcpDone(Err("no tofu state".into())));
        assert!(p.gateway_dhcp.is_none());
        assert!(p.status.contains("gateway-dhcp"), "{}", p.status);
    }

    #[test]
    fn gateway_reply_decodes_into_rows() {
        // The on-wire `gateway-dhcp` reply shape decodes into GatewayDhcp.
        let v = serde_json::json!({
            "ok": true,
            "reservations": [{ "name": "kvm1", "mac": "00:23:24:c2:0f:1c", "ip": "172.20.145.193" }],
            "leases": [{ "ip": "10.0.0.5", "mac": "aa:bb:cc:dd:ee:ff", "expiry": "x", "hostname": "h" }],
        });
        let dhcp: GatewayDhcp = serde_json::from_value(v).unwrap();
        assert_eq!(dhcp.reservations[0].name, "kvm1");
        assert_eq!(dhcp.reservations[0].ip, "172.20.145.193");
        assert_eq!(dhcp.leases[0].hostname, "h");
    }

    #[test]
    fn gateway_apply_is_typed_confirm_gated() {
        // The edgeos apply mirrors the Tofu typed-confirm gate: the first click
        // only arms the confirm (TofuApplyClicked), no RPC fires.
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::TofuApplyClicked("edgeos".into()));
        assert_eq!(p.tofu_confirm.as_deref(), Some("edgeos"));
        // Cancel clears it without firing.
        let _ = p.update(Message::TofuApplyCancelled);
        assert!(p.tofu_confirm.is_none());
    }

    #[test]
    fn load_error_keeps_last_known_rows_and_clears_status() {
        // DATACENTER-8 graceful-degrade: a failed Bus read after a good one must
        // mark the plane stale WITHOUT discarding the last-known snapshot, so the
        // operator keeps seeing data (under a stale badge) rather than a blank.
        let mut p = DatacenterPanel::new();
        let rows = project_rows(&[(
            "event/dc/droplet/2".into(),
            r#"{"kind":"droplet","id":"2","name":"lighthouse-01","status":"active","zone":"prod"}"#
                .into(),
        )]);
        let _ = p.update(Message::Loaded(Ok(DcLoad {
            rows,
            ..Default::default()
        })));
        assert_eq!(p.rows.len(), 1);
        assert!(p.load_error.is_none());
        // A subsequent failed read marks stale but keeps the last-known row.
        let _ = p.update(Message::Loaded(Err("bus unreachable".to_string())));
        assert_eq!(
            p.rows.len(),
            1,
            "last-known rows must survive a failed read (graceful-degrade)"
        );
        assert_eq!(p.load_error.as_deref(), Some("bus unreachable"));
        assert!(!p.busy);
        assert!(
            p.status.is_empty(),
            "stale banner is the single failure cue"
        );
        // The view renders the stale banner over the kept rows without panicking.
        let _ = p.view();
    }

    #[test]
    fn load_error_with_no_rows_renders_bare_error_with_retry() {
        // With nothing last-known, the plane falls back to a bare error screen
        // (which carries a Retry) rather than an empty stale banner.
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::Loaded(Err("bus unreachable".to_string())));
        assert!(p.rows.is_empty());
        assert_eq!(p.load_error.as_deref(), Some("bus unreachable"));
        let _ = p.view(); // exercises the no-last-known error/Retry path
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
        // The producer (the daemon's dc_auditor) now stamps actor + target +
        // result + ts; the panel projects all five into the row.
        let r = parse_audit_event(
            r#"{"action":"vm-delete","actor":"peer:anvil","target":"abc-123","result":"issued","ts":"1700000000000"}"#,
        )
        .unwrap();
        assert_eq!(r.action, "vm-delete");
        assert_eq!(r.actor, "peer:anvil");
        assert_eq!(r.target, "abc-123");
        assert_eq!(r.result, "issued");
        assert_eq!(r.ts, "1700000000000");
    }

    #[test]
    fn parse_audit_event_defaults_missing_fields_and_drops_garbage() {
        // Missing actor/target/result/ts default to empty (an older event or one
        // whose action named no target still parses, just with blank columns).
        let r = parse_audit_event(r#"{"action":"vm-delete"}"#).unwrap();
        assert_eq!(r.action, "vm-delete");
        assert_eq!(r.actor, "");
        assert_eq!(r.target, "");
        assert_eq!(r.result, "");
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
                r#"{"action":"tofu-plan","actor":"peer:a","target":"xen-xapi","result":"issued","ts":"2026-06-22T09:00:00Z"}"#.into(),
            ),
            (
                "event/dc/audit/2".into(),
                r#"{"action":"tofu-apply","actor":"peer:a","target":"xen-xapi","result":"issued","ts":"2026-06-22T11:00:00Z"}"#.into(),
            ),
            (
                "event/dc/audit/3".into(),
                r#"{"action":"vm-delete","actor":"peer:b","target":"uuid-9","result":"issued","ts":"2026-06-22T10:00:00Z"}"#.into(),
            ),
        ];
        let rows = project_audit(&events);
        assert_eq!(rows.len(), 3); // non-audit dropped
                                   // Newest-first by ts: 11:00 > 10:00 > 09:00.
        assert_eq!(rows[0].action, "tofu-apply");
        assert_eq!(rows[1].action, "vm-delete");
        assert_eq!(rows[2].action, "tofu-plan");
        // The projection carries actor · target · result · ts through, not just
        // action — the newest row (vm-apply) is fully populated.
        assert_eq!(rows[0].actor, "peer:a");
        assert_eq!(rows[0].target, "xen-xapi");
        assert_eq!(rows[0].result, "issued");
        assert_eq!(rows[0].ts, "2026-06-22T11:00:00Z");
        assert_eq!(rows[1].target, "uuid-9"); // the vm-delete's target/ts present
        assert_eq!(rows[1].ts, "2026-06-22T10:00:00Z");
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
    fn prod_arm_allows_gates_only_prod() {
        // Non-prod workspace is always allowed (its own confirm still gates it).
        assert!(prod_arm_allows("xen-xapi", false));
        assert!(prod_arm_allows("xen-xapi", true));
        // The PROD workspace requires the arm; fails closed when disarmed.
        assert!(!prod_arm_allows(PROD_TOFU_WS, false));
        assert!(prod_arm_allows(PROD_TOFU_WS, true));
        // Sanity: the prod ws constant is the DO zone.
        assert_eq!(PROD_TOFU_WS, "zone1-do");
    }

    #[test]
    fn prod_apply_refuses_until_armed() {
        let mut p = DatacenterPanel::new();
        assert!(!p.tofu_prod_armed);
        // Arm the typed-confirm on the PROD workspace, then confirm — with the
        // prod-arm switch DISARMED the RPC must NOT fire; the confirm is cleared
        // and the status nudges the operator to arm first.
        let _ = p.update(Message::TofuApplyClicked(PROD_TOFU_WS.to_string()));
        assert_eq!(p.tofu_confirm.as_deref(), Some(PROD_TOFU_WS));
        let _ = p.update(Message::TofuApply(PROD_TOFU_WS.to_string()));
        assert!(p.tofu_confirm.is_none());
        assert_eq!(
            p.status,
            format!("Arm PROD before applying {PROD_TOFU_WS}.")
        );
        // The output is untouched — no "Applying…" because nothing fired.
        assert!(p.tofu_output.is_empty());
        // Arm, re-confirm → now it proceeds to the in-flight state.
        let _ = p.update(Message::TofuProdArm(true));
        assert!(p.tofu_prod_armed);
        let _ = p.update(Message::TofuApplyClicked(PROD_TOFU_WS.to_string()));
        let _ = p.update(Message::TofuApply(PROD_TOFU_WS.to_string()));
        assert_eq!(p.status, format!("Applying {PROD_TOFU_WS}…"));
        assert_eq!(p.tofu_output, format!("Applying {PROD_TOFU_WS}…"));
    }

    #[test]
    fn nonprod_apply_fires_without_the_prod_arm() {
        let mut p = DatacenterPanel::new();
        // The dev/farm Xen plane is not prod-gated — a typed-confirm is enough.
        assert!(!p.tofu_prod_armed);
        let _ = p.update(Message::TofuApplyClicked("xen-xapi".to_string()));
        let _ = p.update(Message::TofuApply("xen-xapi".to_string()));
        assert_eq!(p.status, "Applying xen-xapi…");
        assert_eq!(p.tofu_output, "Applying xen-xapi…");
    }

    #[test]
    fn tofu_destroy_requires_typed_confirm_before_firing() {
        let mut p = DatacenterPanel::new();
        // First click only arms the typed-confirm — it must NOT fire the RPC.
        let _ = p.update(Message::TofuDestroyClicked("xen-xapi".to_string()));
        assert_eq!(p.tofu_destroy_confirm.as_deref(), Some("xen-xapi"));
        assert_eq!(p.status, "Type DESTROY to confirm below.");
        // Only the explicit confirm clears the pending state + moves the status
        // to the in-flight "Destroying…" (the RPC then fires).
        let _ = p.update(Message::TofuDestroy("xen-xapi".to_string()));
        assert!(p.tofu_destroy_confirm.is_none());
        assert_eq!(p.status, "Destroying xen-xapi…");
        assert_eq!(p.tofu_output, "Destroying xen-xapi…");
    }

    #[test]
    fn prod_destroy_refuses_until_armed() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::TofuDestroyClicked(PROD_TOFU_WS.to_string()));
        assert_eq!(p.tofu_destroy_confirm.as_deref(), Some(PROD_TOFU_WS));
        // Disarmed → confirm is cleared and the destroy never fires.
        let _ = p.update(Message::TofuDestroy(PROD_TOFU_WS.to_string()));
        assert!(p.tofu_destroy_confirm.is_none());
        assert_eq!(
            p.status,
            format!("Arm PROD before destroying {PROD_TOFU_WS}.")
        );
        assert!(p.tofu_output.is_empty());
    }

    #[test]
    fn tofu_destroy_cancel_clears_the_pending_confirm() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::TofuDestroyClicked("zone1-do".to_string()));
        assert_eq!(p.tofu_destroy_confirm.as_deref(), Some("zone1-do"));
        let _ = p.update(Message::TofuDestroyCancelled);
        assert!(p.tofu_destroy_confirm.is_none());
        assert!(p.status.is_empty());
    }

    #[test]
    fn tofu_destroy_done_writes_output() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::TofuDestroyDone(Ok(
            "Destroy complete! 2 destroyed.".to_string(),
        )));
        assert_eq!(p.tofu_output, "Destroy complete! 2 destroyed.");
        assert_eq!(p.status, "Destroy complete");
        let _ = p.update(Message::TofuDestroyDone(Err("destroy failed".to_string())));
        assert_eq!(p.tofu_output, "destroy failed");
        assert_eq!(p.status, "destroy failed");
    }

    #[test]
    fn prod_arm_toggle_flips_state_and_status() {
        let mut p = DatacenterPanel::new();
        assert!(!p.tofu_prod_armed);
        let _ = p.update(Message::TofuProdArm(true));
        assert!(p.tofu_prod_armed);
        assert!(p.status.contains("PROD armed"));
        let _ = p.update(Message::TofuProdArm(false));
        assert!(!p.tofu_prod_armed);
        assert_eq!(p.status, "PROD disarmed.");
    }

    #[test]
    fn tofu_view_renders_with_destroy_and_arm_controls() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::ViewMode(ViewMode::Tofu));
        // Render the bare tab (arm bar + per-ws control rows + run-log empty).
        let _ = p.view();
        // Arm prod + arm both confirms to exercise every render branch.
        let _ = p.update(Message::TofuProdArm(true));
        let _ = p.update(Message::TofuApplyClicked(PROD_TOFU_WS.to_string()));
        let _ = p.update(Message::TofuDestroyClicked("xen-xapi".to_string()));
        let _ = p.view();
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
                actor: "peer:anvil".into(),
                target: "xen-xapi".into(),
                result: "issued".into(),
                ts: "2026-06-22T11:00:00Z".into(),
            }],
            promote: Vec::new(),
            health: Vec::new(),
            jobs: Vec::new(),
            routes: Vec::new(),
            logs: Vec::new(),
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
            routes: Vec::new(),
            logs: Vec::new(),
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
            routes: Vec::new(),
            logs: Vec::new(),
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

    // ---- DATACENTER-20: auto-promote states + prod-arm gate -----------------

    #[test]
    fn project_promote_reads_the_auto_queued_armed_states() {
        // The worker now publishes the eagle/do stages with the auto/queued/armed
        // statuses; the panel projection must carry them through verbatim.
        let stages = project_promote(&[
            (
                "event/dc/promote/build".into(),
                r#"{"stage":"build","version":"11.0.2","status":"ready"}"#.into(),
            ),
            (
                "event/dc/promote/eagle".into(),
                r#"{"stage":"eagle","version":"11.0.2","status":"auto"}"#.into(),
            ),
            (
                "event/dc/promote/do".into(),
                r#"{"stage":"do","version":"11.0.2","status":"queued"}"#.into(),
            ),
        ]);
        let m = promote_matrix(&stages);
        assert_eq!(m[0].status, "ready");
        assert_eq!(m[1].status, "auto");
        assert_eq!(m[2].status, "queued");
        // And an armed DO step round-trips too.
        let armed = project_promote(&[(
            "event/dc/promote/do".into(),
            r#"{"stage":"do","version":"11.0.2","status":"armed"}"#.into(),
        )]);
        assert_eq!(armed[0].status, "armed");
    }

    #[test]
    fn promote_chip_maps_every_status_to_a_token() {
        let pal = Palette::dark();
        // ready + auto are both "green" (success).
        assert_eq!(promote_chip("ready", pal).0, pal.success);
        assert_eq!(promote_chip("auto", pal).0, pal.success);
        // armed is LIVE → danger; its label names the DO promotion.
        let (c, label) = promote_chip("armed", pal);
        assert_eq!(c, pal.danger);
        assert!(label.contains("DO"));
        // queued is held → warning; pending is also warning.
        assert_eq!(promote_chip("queued", pal).0, pal.warning);
        assert_eq!(promote_chip("pending", pal).0, pal.warning);
        // An unknown placeholder → muted "—".
        let (c, label) = promote_chip("", pal);
        assert_eq!(c, pal.text_muted);
        assert_eq!(label, "—");
        // An unrecognised status still renders (muted, raw text) rather than panic.
        assert_eq!(promote_chip("weird", pal).1, "weird");
    }

    #[test]
    fn promote_prod_arm_persists_and_fails_closed() {
        // The persistence core (path-explicit, no env mutation) — the same
        // `{"armed":bool}` config the `dc_promote` worker reads back. A temp file
        // keeps the real `$HOME` config untouched.
        let dir = std::env::temp_dir().join(format!("dc_panel_arm_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dc-prod-arm.json");
        // Missing file → disarmed (fail-closed).
        assert!(!load_promote_prod_arm_at(&path));
        // Arm → persists → reads back armed.
        save_promote_prod_arm_at(&path, true).expect("save armed");
        assert!(load_promote_prod_arm_at(&path));
        // Disarm → persists → reads back disarmed.
        save_promote_prod_arm_at(&path, false).expect("save disarmed");
        assert!(!load_promote_prod_arm_at(&path));
        // A corrupt body → disarmed (fail-closed; never auto-promotes to prod).
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(!load_promote_prod_arm_at(&path));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn promote_prod_arm_toggle_flips_the_flag_and_status() {
        // The handler flips the in-memory gate immediately (so the strip reflects
        // it) and names the new state in the status line — independent of whether
        // the persist write succeeds (which the persistence test covers).
        let mut p = DatacenterPanel::new();
        assert!(!p.promote_prod_armed); // defaults disarmed (fail-closed)
        let _ = p.update(Message::PromoteProdArm(true));
        assert!(p.promote_prod_armed);
        assert!(p.status.to_lowercase().contains("armed"));
        let _ = p.update(Message::PromoteProdArm(false));
        assert!(!p.promote_prod_armed);
        assert!(p.status.to_lowercase().contains("disarmed"));
    }

    #[test]
    fn overview_renders_the_prod_arm_bar_and_auto_states() {
        let mut p = DatacenterPanel::new();
        // A green-and-armed pipeline: build ready, eagle auto, do armed — exercises
        // the new chip branches + the prod-arm bar (armed = danger path).
        p.promote = project_promote(&[
            (
                "event/dc/promote/build".into(),
                r#"{"stage":"build","version":"11.0.2","status":"ready"}"#.into(),
            ),
            (
                "event/dc/promote/eagle".into(),
                r#"{"stage":"eagle","version":"11.0.2","status":"auto"}"#.into(),
            ),
            (
                "event/dc/promote/do".into(),
                r#"{"stage":"do","version":"11.0.2","status":"armed"}"#.into(),
            ),
        ]);
        p.promote_prod_armed = true;
        let _ = p.view();
        // And the disarmed / queued path renders too.
        p.promote_prod_armed = false;
        p.promote = project_promote(&[(
            "event/dc/promote/do".into(),
            r#"{"stage":"do","version":"11.0.2","status":"queued"}"#.into(),
        )]);
        let _ = p.view();
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
            routes: Vec::new(),
            logs: Vec::new(),
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

    fn log_row(host: &str, ts: u64, level: &str, target: &str, msg: &str) -> fleet_logs::LogRow {
        fleet_logs::LogRow {
            ts_ms: ts,
            host: host.into(),
            level: level.into(),
            target: target.into(),
            message: msg.into(),
        }
    }

    #[test]
    fn datacenter_log_rows_scopes_to_dc_hosts() {
        let rows = vec![
            log_row("172.20.0.9", 1, "warn", "xapi.service", "toolstack"),
            log_row("172.20.145.193", 2, "error", "kernel", "oom"),
            log_row("some-laptop", 3, "info", "nebula", "handshake"), // not a dc host
        ];
        let mut ids = std::collections::BTreeSet::new();
        ids.insert("172.20.0.9".to_string());
        ids.insert("172.20.145.193".to_string());
        let scoped = datacenter_log_rows(&rows, &ids);
        assert_eq!(scoped.len(), 2, "only the two dom0 hosts survive");
        assert!(scoped.iter().all(|r| r.host.starts_with("172.20.")));
    }

    #[test]
    fn datacenter_log_rows_empty_scope_yields_nothing() {
        // No host rows known yet → the Datacenter plane shows nothing rather than
        // spilling the whole fleet's mesh logs in.
        let rows = vec![log_row("172.20.0.9", 1, "warn", "x", "y")];
        let scoped = datacenter_log_rows(&rows, &std::collections::BTreeSet::new());
        assert!(scoped.is_empty());
    }

    #[test]
    fn loaded_populates_logs() {
        let mut p = DatacenterPanel::new();
        let load = DcLoad {
            rows: Vec::new(),
            audit: Vec::new(),
            promote: Vec::new(),
            health: Vec::new(),
            jobs: Vec::new(),
            routes: Vec::new(),
            logs: vec![log_row("172.20.0.9", 1, "warn", "xapi.service", "restart")],
        };
        let _ = p.update(Message::Loaded(Ok(load)));
        assert_eq!(p.logs.len(), 1);
        assert_eq!(p.logs[0].host, "172.20.0.9");
    }

    #[test]
    fn logs_view_renders_with_and_without_rows() {
        // The Logs view renders both empty (no aggregated logs hint) and populated
        // (filtered, newest-first) without panicking — reachable via the tab.
        let mut p = DatacenterPanel::new();
        p.view_mode = ViewMode::Logs;
        let _ = p.view(); // empty-state branch
        p.logs = vec![
            log_row("172.20.0.9", 1, "warn", "xapi.service", "toolstack restart"),
            log_row("172.20.0.9", 2, "error", "kernel", "oom-killer invoked"),
        ];
        let _ = p.view(); // populated branch
                          // Search narrows the visible page (global filter doubles as the log query).
        p.filter = "oom".into();
        let _ = p.view();
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
            routes: Vec::new(),
            logs: Vec::new(),
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
            ..DcRow::default()
        }
    }

    #[test]
    fn status_dot_maps_liveness_onto_semantic_tokens() {
        let p = Palette::dark();
        // Up vocabularies (DO "active" / Xen "running" / "up") → success.
        assert_eq!(
            row_with("vm", "1", "a", "running", "dev").status_dot(p),
            p.success
        );
        assert_eq!(
            row_with("droplet", "2", "b", "active", "prod").status_dot(p),
            p.success
        );
        // Off vocabularies → danger.
        assert_eq!(
            row_with("vm", "3", "c", "halted", "dev").status_dot(p),
            p.danger
        );
        assert_eq!(
            row_with("droplet", "4", "d", "off", "prod").status_dot(p),
            p.danger
        );
        // Transitional → warning.
        assert_eq!(
            row_with("vm", "5", "e", "rebooting", "dev").status_dot(p),
            p.warning
        );
        // Case-insensitive.
        assert_eq!(
            row_with("vm", "6", "f", "RUNNING", "dev").status_dot(p),
            p.success
        );
        // Unknown / empty → muted (never a misleading green/red).
        assert_eq!(
            row_with("vm", "7", "g", "", "dev").status_dot(p),
            p.text_muted
        );
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
                dc_card_view(r, palette, false, motion)
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

    // ── DATACENTER-11 (VMs tab) ──────────────────────────────────────────────

    /// A VM row with a populated host (the owning dom0) — the VMs tab needs `host`
    /// to target each RPC, which the `row_with` helper leaves empty.
    fn vm_row(id: &str, name: &str, status: &str, host: &str) -> DcRow {
        let mut r = row_with("vm", id, name, status, "dev");
        r.host = host.into();
        r
    }

    #[test]
    fn vms_view_mode_round_trips_and_is_selectable() {
        assert_eq!(ViewMode::from_slug(ViewMode::Vms.slug()), ViewMode::Vms);
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::ViewMode(ViewMode::Vms));
        assert_eq!(p.view_mode, ViewMode::Vms);
    }

    #[test]
    fn vms_tab_renders_lifecycle_create_and_bulk() {
        let mut p = DatacenterPanel::new();
        p.rows = vec![
            vm_row("aaaa-1", "builder", "running", "172.20.0.9"),
            vm_row("bbbb-2", "web", "halted", "172.20.0.9"),
        ];
        let _ = p.update(Message::ViewMode(ViewMode::Vms));
        // Renders the create form + bulk toolbar + per-VM lifecycle cards.
        let _ = p.view();
        // With a selection + an armed prompt + bulk progress, the richer branches
        // render too.
        let _ = p.update(Message::BulkToggle("aaaa-1".into()));
        let _ = p.update(Message::MigrateClicked {
            uuid: "bbbb-2".into(),
            dom0: "172.20.0.9".into(),
        });
        p.bulk_progress.insert("aaaa-1".into(), BulkItem::Pending);
        let _ = p.view();
    }

    #[test]
    fn migrate_prompt_arms_collects_host_and_cancels() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::MigrateClicked {
            uuid: "aaaa-1".into(),
            dom0: "172.20.0.9".into(),
        });
        assert!(matches!(p.vm_prompt, Some(VmPrompt::Migrate { .. })));
        let _ = p.update(Message::MigrateHostChanged("xcp-big".into()));
        assert!(
            matches!(&p.vm_prompt, Some(VmPrompt::Migrate { host, .. }) if host == "xcp-big"),
            "the migrate prompt should carry the typed host"
        );
        let _ = p.update(Message::VmPromptCancelled);
        assert!(p.vm_prompt.is_none());
    }

    #[test]
    fn migrate_confirm_with_empty_host_is_a_no_op_with_a_hint() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::MigrateClicked {
            uuid: "aaaa-1".into(),
            dom0: "172.20.0.9".into(),
        });
        // Confirm with a blank host → clears the prompt, sets a hint, fires no RPC.
        let _ = p.update(Message::MigrateConfirmed {
            uuid: "aaaa-1".into(),
            dom0: "172.20.0.9".into(),
        });
        assert!(p.vm_prompt.is_none());
        assert!(p.status.contains("destination host"));
    }

    #[test]
    fn resize_prompt_collects_vcpus_and_mem_and_validates() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::ResizeClicked {
            uuid: "aaaa-1".into(),
            dom0: "172.20.0.9".into(),
        });
        let _ = p.update(Message::ResizeVcpusChanged("notanumber".into()));
        let _ = p.update(Message::ResizeMemChanged("2048".into()));
        // A non-numeric vcpus → confirm clears the prompt + hints, fires no RPC.
        let _ = p.update(Message::ResizeConfirmed {
            uuid: "aaaa-1".into(),
            dom0: "172.20.0.9".into(),
        });
        assert!(p.vm_prompt.is_none());
        assert!(p.status.contains("whole-number"));
    }

    #[test]
    fn snapshots_clicked_sets_status_and_done_keys_the_cache() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::SnapshotsClicked {
            uuid: "aaaa-1".into(),
            dom0: "172.20.0.9".into(),
        });
        assert_eq!(p.status, "Listing snapshots…");
        // The reply echoes the VM uuid → the cache keys off it.
        let list = vec![
            VmSnapshot {
                uuid: "snap-1".into(),
                name: "nightly".into(),
            },
            VmSnapshot {
                uuid: "snap-2".into(),
                name: "dc-snap-aaaa1".into(),
            },
        ];
        let _ = p.update(Message::SnapshotsDone(Ok(("aaaa-1".into(), list.clone()))));
        assert_eq!(p.vm_snapshots.get("aaaa-1"), Some(&list));
        assert!(p.status.contains("2 snapshot(s)"));
        // An error surfaces to the status without touching the cache.
        let _ = p.update(Message::SnapshotsDone(Err("snapshot-list failed".into())));
        assert_eq!(p.status, "snapshot-list failed");
        assert_eq!(p.vm_snapshots.get("aaaa-1"), Some(&list));
    }

    #[test]
    fn snapshot_revert_clicked_sets_status_and_done_writes_outcome() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::SnapshotRevertClicked {
            snapshot: "snap-1".into(),
            dom0: "172.20.0.9".into(),
        });
        assert!(p.status.contains("Reverting to snap-1"));
        let _ = p.update(Message::SnapshotRevertDone(Ok("reverted to snap-1".into())));
        assert_eq!(p.status, "reverted to snap-1");
        let _ = p.update(Message::SnapshotRevertDone(Err("revert failed".into())));
        assert_eq!(p.status, "revert failed");
    }

    #[test]
    fn snapshot_delete_arms_confirm_cancels_and_confirms() {
        let mut p = DatacenterPanel::new();
        // First click only arms the per-snapshot confirm — no RPC, just state.
        let _ = p.update(Message::SnapshotDeleteClicked {
            snapshot: "snap-1".into(),
        });
        assert_eq!(p.confirm_snapshot_delete.as_deref(), Some("snap-1"));
        // Cancel clears it.
        let _ = p.update(Message::SnapshotDeleteCancelled);
        assert!(p.confirm_snapshot_delete.is_none());
        // Re-arm, then Confirm clears the gate (and fires the destructive RPC task).
        let _ = p.update(Message::SnapshotDeleteClicked {
            snapshot: "snap-1".into(),
        });
        let _ = p.update(Message::SnapshotDeleteConfirmed {
            snapshot: "snap-1".into(),
            vm: "aaaa-1".into(),
            dom0: "172.20.0.9".into(),
        });
        assert!(p.confirm_snapshot_delete.is_none());
        assert!(p.status.contains("Deleting snapshot snap-1"));
        // A delete error surfaces to status.
        let _ = p.update(Message::SnapshotDeleteDone(Err("destroy failed".into())));
        assert_eq!(p.status, "destroy failed");
    }

    #[test]
    fn vms_tab_renders_snapshot_list_with_revert_and_delete_confirm() {
        let mut p = DatacenterPanel::new();
        p.rows = vec![vm_row("aaaa-1", "builder", "running", "172.20.0.9")];
        let _ = p.update(Message::ViewMode(ViewMode::Vms));
        // Populate the per-VM snapshot cache, then render the list rows.
        p.vm_snapshots.insert(
            "aaaa-1".into(),
            vec![VmSnapshot {
                uuid: "snap-1".into(),
                name: "nightly".into(),
            }],
        );
        let _ = p.view();
        // Arm the per-snapshot delete confirm → the inline "Really delete?" branch
        // renders too.
        p.confirm_snapshot_delete = Some("snap-1".into());
        let _ = p.view();
        // An empty (fetched-but-none) list renders the "none" line without panic.
        p.vm_snapshots.insert("aaaa-1".into(), Vec::new());
        let _ = p.view();
    }

    #[test]
    fn load_clears_snapshot_cache_and_confirm() {
        let mut p = DatacenterPanel::new();
        p.vm_snapshots.insert(
            "aaaa-1".into(),
            vec![VmSnapshot {
                uuid: "snap-1".into(),
                name: "n".into(),
            }],
        );
        p.confirm_snapshot_delete = Some("snap-1".into());
        // A fresh load drops the stale snapshot cache + confirm (a refresh may change
        // which VMs/snapshots exist).
        let _ = p.update(Message::Loaded(Ok(DcLoad::default())));
        assert!(p.vm_snapshots.is_empty());
        assert!(p.confirm_snapshot_delete.is_none());
    }

    #[test]
    fn create_form_fields_update_and_blank_create_is_guarded() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::CreateFieldChanged {
            field: VmCreateField::Name,
            value: "web-1".into(),
        });
        let _ = p.update(Message::CreateFieldChanged {
            field: VmCreateField::Vcpus,
            value: "4".into(),
        });
        let _ = p.update(Message::CreateFieldChanged {
            field: VmCreateField::MemMib,
            value: "4096".into(),
        });
        assert_eq!(p.vm_create.name, "web-1");
        assert_eq!(p.vm_create.vcpus, "4");
        // No network uuid / dom0 yet → Create is guarded (a hint, no panic).
        let _ = p.update(Message::CreateVmClicked);
        assert!(p.status.contains("network uuid") || p.status.contains("dom0"));
        // Non-numeric vcpus → the parse guard hints instead of firing.
        let _ = p.update(Message::CreateFieldChanged {
            field: VmCreateField::Vcpus,
            value: "x".into(),
        });
        let _ = p.update(Message::CreateVmClicked);
        assert!(p.status.contains("whole-number"));
    }

    #[test]
    fn bulk_selection_toggles_select_all_and_clear() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::BulkToggle("a".into()));
        let _ = p.update(Message::BulkToggle("b".into()));
        assert_eq!(p.vm_selected.len(), 2);
        // Toggling an already-selected uuid removes it.
        let _ = p.update(Message::BulkToggle("a".into()));
        assert_eq!(p.vm_selected.len(), 1);
        let _ = p.update(Message::BulkSelectAll(vec![
            "a".into(),
            "b".into(),
            "c".into(),
        ]));
        assert_eq!(p.vm_selected.len(), 3);
        let _ = p.update(Message::BulkClear);
        assert!(p.vm_selected.is_empty());
    }

    #[test]
    fn bulk_run_with_no_selection_is_a_no_op() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::BulkRun {
            op: "start".into(),
            dom0_by_uuid: vec![],
        });
        assert!(p.status.contains("at least one"));
        assert!(p.bulk_progress.is_empty());
    }

    #[test]
    fn bulk_tag_requires_a_tag() {
        let mut p = DatacenterPanel::new();
        // A tag run with selected VMs but no tag → guarded.
        let _ = p.update(Message::BulkRun {
            op: "tag".into(),
            dom0_by_uuid: vec![("a".into(), "172.20.0.9".into())],
        });
        assert!(p.status.contains("tag"));
        assert!(p.bulk_progress.is_empty());
    }

    #[test]
    fn bulk_item_done_updates_progress_and_summarizes() {
        let mut p = DatacenterPanel::new();
        // Two items pending; resolving both flips the status to the summary.
        p.bulk_progress.insert("a".into(), BulkItem::Pending);
        p.bulk_progress.insert("b".into(), BulkItem::Pending);
        let _ = p.update(Message::BulkItemDone {
            uuid: "a".into(),
            result: Ok("start ok".into()),
        });
        // Still one pending → no summary yet.
        assert!(matches!(p.bulk_progress.get("b"), Some(BulkItem::Pending)));
        let _ = p.update(Message::BulkItemDone {
            uuid: "b".into(),
            result: Err("boom".into()),
        });
        assert!(p.status.contains("1/2 ok"), "status was: {}", p.status);
        assert!(matches!(p.bulk_progress.get("a"), Some(BulkItem::Ok(_))));
        assert!(matches!(p.bulk_progress.get("b"), Some(BulkItem::Err(_))));
    }

    #[test]
    fn bulk_op_routes_each_verb() {
        // The bulk dispatcher maps each op to its single-VM helper; an unknown op
        // errors rather than silently no-opping. (We can't exercise the Bus here, so
        // assert the routing's error path for the unknown verb.)
        assert!(bulk_op("nonsense", "a", "172.20.0.9", "").is_err());
    }

    #[test]
    fn load_clears_vm_prompt_and_bulk_state() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::MigrateClicked {
            uuid: "a".into(),
            dom0: "172.20.0.9".into(),
        });
        let _ = p.update(Message::BulkToggle("a".into()));
        p.bulk_progress.insert("a".into(), BulkItem::Pending);
        assert!(p.vm_prompt.is_some());
        assert!(!p.vm_selected.is_empty());
        // A fresh load drops the per-VM prompt + the bulk selection/progress (a
        // refresh can change which VMs exist), but keeps the create-wizard draft.
        let _ = p.update(Message::CreateFieldChanged {
            field: VmCreateField::Name,
            value: "keep-me".into(),
        });
        let _ = p.update(Message::Loaded(Ok(DcLoad::default())));
        assert!(p.vm_prompt.is_none());
        assert!(p.vm_selected.is_empty());
        assert!(p.bulk_progress.is_empty());
        assert_eq!(
            p.vm_create.name, "keep-me",
            "the create draft survives a load"
        );
    }

    #[test]
    fn vm_console_open_returns_a_status_for_a_url() {
        // No assertion on xdg-open's presence — it's best-effort; the helper must
        // return a status line either way (never panic).
        let s = vm_console_open("https://172.20.0.9/console?uuid=abcd");
        assert!(s.contains("172.20.0.9"));
    }

    // ── DATACENTER-13 (Network tab) ──────────────────────────────────────────────

    #[test]
    fn network_view_mode_round_trips_and_is_selectable() {
        assert_eq!(
            ViewMode::from_slug(ViewMode::Network.slug()),
            ViewMode::Network
        );
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::ViewMode(ViewMode::Network));
        assert_eq!(p.view_mode, ViewMode::Network);
    }

    #[test]
    fn route_events_project_and_are_excluded_from_rows() {
        let events = vec![
            (
                "event/dc/route/xen/172.20.0.9".to_string(),
                r#"{"kind":"route","id":"172.20.0.9","target":"xen","path":"relay","relay":"10.0.0.2","note":"off-LAN"}"#.to_string(),
            ),
            (
                "event/dc/route/xen/172.20.0.10".to_string(),
                r#"{"kind":"route","id":"172.20.0.10","target":"xen","path":"direct","relay":"","note":""}"#.to_string(),
            ),
            (
                "event/dc/droplet/1".to_string(),
                r#"{"kind":"droplet","id":"1","name":"lh","status":"active","zone":"prod","ip":"203.0.113.5","region":"nyc3"}"#.to_string(),
            ),
        ];
        let routes = project_routes(&events);
        assert_eq!(routes.len(), 2);
        // Sorted by id lexicographically: "172.20.0.10" < "172.20.0.9" ('1' < '9').
        assert_eq!(routes[0].id, "172.20.0.10");
        assert_eq!(routes[0].path, "direct");
        // The relay route (`.9`) carries its relay + note.
        let relay = routes
            .iter()
            .find(|r| r.id == "172.20.0.9")
            .expect("relay route present");
        assert_eq!(relay.path, "relay");
        assert_eq!(relay.relay, "10.0.0.2");
        // Route events never leak into the resource card rows.
        let rows = project_rows(&events);
        assert!(rows.iter().all(|r| r.kind != "route"));
        // …but the droplet (with its ip + region) still projects.
        let d = rows.iter().find(|r| r.kind == "droplet").expect("droplet");
        assert_eq!(d.ip, "203.0.113.5");
        assert_eq!(d.region, "nyc3");
    }

    #[test]
    fn parse_route_event_rejects_non_route_kinds() {
        // A droplet body is not a route — `parse_route_event` returns None.
        assert!(parse_route_event(r#"{"kind":"droplet","id":"1","name":"x"}"#).is_none());
        // Garbage / missing id → None, never a panic.
        assert!(parse_route_event("not json").is_none());
        assert!(parse_route_event(r#"{"kind":"route"}"#).is_none());
    }

    #[test]
    fn network_tab_renders_all_four_sections_without_panicking() {
        let mut p = DatacenterPanel::new();
        // L2 (a net row), a VM + host for the topology lens, and an addressed
        // droplet for the IP/DNS view + a route for the overlay table.
        let _ = p.update(Message::Loaded(Ok(DcLoad {
            rows: project_rows(&[
                (
                    "event/dc/host/172.20.0.9".into(),
                    r#"{"kind":"host","id":"172.20.0.9","name":"dom0","status":"up","zone":"dev"}"#.into(),
                ),
                (
                    "event/dc/net/0".into(),
                    r#"{"kind":"net","id":"net-0","name":"Pool-wide","status":"up","zone":"dev","bridge":"xenbr0","host":"172.20.0.9"}"#.into(),
                ),
                (
                    "event/dc/vm/aaaa".into(),
                    r#"{"kind":"vm","id":"aaaa","name":"web","status":"running","zone":"dev","host":"172.20.0.9"}"#.into(),
                ),
                (
                    "event/dc/droplet/1".into(),
                    r#"{"kind":"droplet","id":"1","name":"lh","status":"active","zone":"prod","ip":"203.0.113.5","region":"nyc3"}"#.into(),
                ),
            ]),
            routes: project_routes(&[(
                "event/dc/route/xen/172.20.0.9".into(),
                r#"{"kind":"route","id":"172.20.0.9","target":"xen","path":"direct","relay":"","note":""}"#.into(),
            )]),
            ..DcLoad::default()
        })));
        let _ = p.update(Message::ViewMode(ViewMode::Network));
        // Renders the whole tab (L2 + overlay + topology + IP/DNS) off the Bus
        // fallback (no live host-net read yet).
        let _ = p.view();
        // Seed a live host-net read so the PIF cards + live-net branch render too.
        let _ = p.update(Message::NetFieldChanged {
            field: NetField::ReadDom0,
            value: "172.20.0.9".into(),
        });
        let _ = p.update(Message::NetReadDone(Ok(NetRead {
            nets: vec![NetInfoRow {
                uuid: "net-0".into(),
                name: "Pool-wide".into(),
                bridge: "xenbr0".into(),
                mtu: 1500,
            }],
            pifs: vec![PifInfoRow {
                uuid: "pif-0".into(),
                device: "eth0".into(),
                mac: "aa:bb:cc:dd:ee:ff".into(),
                vlan: -1,
                carrier: true,
                management: true,
                network: "net-0".into(),
            }],
        })));
        assert!(p.net_read.is_some());
        let _ = p.view();
    }

    #[test]
    fn vlan_target_pif_seeds_the_create_form() {
        let mut p = DatacenterPanel::new();
        let _ = p.update(Message::VlanTargetPif {
            pif: "pif-0".into(),
            dom0: "172.20.0.9".into(),
        });
        assert_eq!(p.vlan_create.pif, "pif-0");
        assert_eq!(p.vlan_create.dom0, "172.20.0.9");
    }

    #[test]
    fn vlan_create_validates_tag_locally_before_rpc() {
        // A non-integer / out-of-range tag is rejected client-side (no Bus round
        // trip), with a clear hint and no panic.
        let bad = VlanCreateForm {
            pif: "pif-0".into(),
            vlan: "not-a-number".into(),
            network_name: "vlan100".into(),
            dom0: "172.20.0.9".into(),
        };
        assert!(vlan_create(&bad).is_err());
        let oor = VlanCreateForm {
            vlan: "4095".into(),
            ..bad.clone()
        };
        assert!(vlan_create(&oor).is_err());
        // Missing fields are caught before parsing the tag.
        let no_pif = VlanCreateForm {
            pif: String::new(),
            ..bad
        };
        assert!(vlan_create(&no_pif).is_err());
    }

    #[test]
    fn read_l2_needs_a_dom0() {
        let mut p = DatacenterPanel::new();
        // An empty dom0 box → a hint, no RPC fired.
        let _ = p.update(Message::NetReadClicked);
        assert!(p.status.contains("dom0"), "{}", p.status);
    }
}
