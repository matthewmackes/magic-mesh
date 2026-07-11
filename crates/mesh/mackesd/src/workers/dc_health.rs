//! DATACENTER-24 — the passive `dc_health` care-and-feeding health checker.
//!
//! A read-only companion to [`super::datacenter_orchestrator`] +
//! [`super::dc_auditor`]: where the orchestrator publishes per-resource state and
//! the auditor records each action request, this worker periodically probes the
//! datacenter substrate's **liveness** — each configured Xen dom0's SSH
//! reachability, the SUBSTRATE-V2 etcd store's `/health`, the mesh secret-store
//! helper, the Nebula CA cert's expiry, each dom0's VMs for crashes (a guest the
//! dom0 expects up but which is halted/paused), and each dom0's pool for degraded
//! hosts (disabled / not live) — and publishes one `event/dc/health/<check>` per
//! check, WITHOUT touching the action handlers or the substrate it watches. It
//! also aggregates each dom0's recent warnings-and-up journal tail into the
//! `fleet_logs` sink so the Datacenter Logs view reads host/VM/service logs beside
//! the mesh's own. It is a pure side-observer: nothing depends on it, so it can
//! never wedge an action.
//!
//! Design (mirrors `dc_jobs` + `dc_auditor`): the *brain* ([`DcHealth`]) is a
//! pure, deduped sieve — fed `(check, status)` it returns a [`HealthRecord`] only
//! when a check's status changes (first sight, or e.g. `ok`→`fail`), so a tick
//! that re-observes the same status never re-publishes. The worker is thin I/O
//! around it: run each best-effort probe, feed the `(check, status)` pair through
//! the sieve, publish what survives. It is **leader-gated** so a multi-node mesh
//! writes each health transition once.

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use super::{ShutdownToken, Worker};

/// Sweep cadence — 30 s (care-and-feeding health is coarse; the probes shell out
/// over SSH/HTTP and shouldn't hammer the substrate).
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(30);

/// Default SUBSTRATE-V2 etcd endpoint (overridable via `MCNF_ETCD`).
pub const DEFAULT_ETCD: &str = "http://172.20.145.192:2379";

/// Days-until-expiry at or below which the Nebula CA cert is flagged `warn`:
/// a CA-cert turnover invalidates every peer cert under it at once, so operators
/// need lead time to `mackesd ca rotate`. 30 days is a full ops cycle of warning
/// (matches [`crate::ca::expiry::CERT_EXPIRY_WARN_DAYS`]).
pub const CERT_WARN_DAYS: i64 = 30;

/// Max characters of a check's `detail` string carried into the record. Keeps the
/// health lane compact.
pub const DETAIL_LEN: usize = 160;

/// Bus topic a health event for `check` is published to: `event/dc/health/<check>`.
#[must_use]
pub fn health_topic(check: &str) -> String {
    format!("event/dc/health/{check}")
}

/// First [`DETAIL_LEN`] characters of a detail string (char-boundary safe).
#[must_use]
fn detail_summary(detail: &str) -> String {
    detail.chars().take(DETAIL_LEN).collect()
}

/// One health event the checker decided to emit (a check's status changed —
/// first sight, or a transition such as `ok`→`fail`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HealthRecord {
    /// The check name (`dom0:172.20.0.9`, `etcd`, `secret-store`, …).
    pub check: String,
    /// The check status: `"ok"`, `"warn"`, or `"fail"`.
    pub status: &'static str,
    /// A short human detail (truncated to [`DETAIL_LEN`] chars).
    pub detail: String,
}

impl HealthRecord {
    /// Bus topic this record publishes to: `event/dc/health/<check>`.
    #[must_use]
    pub fn topic(&self) -> String {
        health_topic(&self.check)
    }

    /// JSON body for `mde-bus publish`.
    #[must_use]
    pub fn body(&self) -> String {
        serde_json::json!({
            "check": self.check,
            "status": self.status,
            "detail": self.detail,
        })
        .to_string()
    }
}

/// Pure health core: tracks the last-published status per check and returns a
/// record ONLY on a status transition (first sight, or a change such as
/// `ok`→`fail`). A tick that re-observes the same status emits nothing, so the Bus
/// never sees a duplicate for an unchanged check.
#[derive(Default)]
pub struct DcHealth {
    last_status: BTreeMap<String, &'static str>,
}

impl DcHealth {
    /// Fresh checker with no observed checks.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe one `check` with its current `status` + `detail`. Returns a
    /// [`HealthRecord`] when the status differs from the last one published for
    /// this check (or on first sight), and `None` when the status is unchanged.
    /// Advances internal state on a transition.
    pub fn observe(
        &mut self,
        check: &str,
        status: &'static str,
        detail: &str,
    ) -> Option<HealthRecord> {
        if self.last_status.get(check) == Some(&status) {
            return None;
        }
        self.last_status.insert(check.to_string(), status);
        Some(HealthRecord {
            check: check.to_string(),
            status,
            detail: detail_summary(detail),
        })
    }
}

// ---- pure cert-expiry helpers (unit-tested without a subprocess) ----

/// Map days-until-expiry to a health status: `"fail"` once the cert is past its
/// `Not after` (negative days), `"warn"` inside the [`CERT_WARN_DAYS`] lead-time
/// window, else `"ok"`.
#[must_use]
pub fn cert_status_from_days(days_until_expiry: i64) -> &'static str {
    if days_until_expiry < 0 {
        "fail"
    } else if days_until_expiry < CERT_WARN_DAYS {
        "warn"
    } else {
        "ok"
    }
}

/// Extract the `Not after` date out of a `nebula-cert print -path <crt>` text
/// document. The Go tool prints the cert as an indented block whose Details
/// section carries a `Not after: <date> UTC` line; this returns the trimmed
/// `<date>` portion (everything after the first `Not after:`), or `None` when no
/// such line is present (garbage / unexpected output). Pure — the parse path is
/// unit-tested against captured output.
#[must_use]
pub fn parse_not_after(nebula_cert_output: &str) -> Option<String> {
    nebula_cert_output.lines().find_map(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix("Not after:")
            .map(|rest| rest.trim().to_string())
            .filter(|s| !s.is_empty())
    })
}

// ---- pure VM-crash helpers (unit-tested without a subprocess) ----

/// One non-control VM's liveness as read from `xe vm-list`: its name + its current
/// power-state + whether it's flagged to auto-power-on (so the dom0 expects it
/// running after a boot). Decoded by [`parse_vm_states`] from the remote helper's
/// pipe-delimited lines.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct VmState {
    /// The VM's name-label (for the alert detail).
    pub name: String,
    /// The XAPI power-state: `running` | `halted` | `paused` | `suspended`.
    pub power: String,
    /// `other-config:auto_poweron` — the dom0 brings this VM up on boot, so a VM
    /// that ISN'T running is an unexpected-down (a crash), not a deliberate stop.
    pub auto_poweron: bool,
}

/// Parse the remote VM-crash helper's `name|power-state|auto_poweron` lines into
/// [`VmState`]s. `auto_poweron` is true only for the literal `true` token (XAPI
/// prints the `other-config` value verbatim; absent/empty ⇒ false). Skips blank
/// lines + lines with an empty name. Pure — fed the raw stdout.
#[must_use]
pub fn parse_vm_states(output: &str) -> Vec<VmState> {
    output
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(3, '|');
            let name = p.next()?.trim();
            if name.is_empty() {
                return None;
            }
            let power = p.next().unwrap_or("").trim().to_string();
            let auto = p.next().unwrap_or("").trim().eq_ignore_ascii_case("true");
            Some(VmState {
                name: name.to_string(),
                power,
                auto_poweron: auto,
            })
        })
        .collect()
}

/// Map one VM's `(power-state, auto_poweron)` to a health status:
/// * `paused` ⇒ `"warn"` — an unexpected pause (XAPI pauses a guest on certain
///   faults / OOM); always worth surfacing regardless of the auto-poweron flag.
/// * not-`running` **and** `auto_poweron` ⇒ `"fail"` — a VM the dom0 expects up
///   (it auto-powers it on at boot) but which is `halted`/`suspended`: a crash.
/// * anything else (`running`, or a deliberately-stopped non-auto VM) ⇒ `"ok"`.
///
/// Pure — the whole VM-crash decision is data, not I/O.
#[must_use]
pub fn vm_status_from_state(power: &str, auto_poweron: bool) -> &'static str {
    if power == "paused" {
        "warn"
    } else if power != "running" && auto_poweron {
        "fail"
    } else {
        "ok"
    }
}

/// Roll a dom0's VMs into one `(status, detail)` for its `vm-crash:<dom0>` health
/// check: the WORST per-VM status wins (`fail` > `warn` > `ok`), and the detail
/// names the offending VMs (or "N VM(s) healthy" when all are ok). Pure +
/// testable; mirrors [`worst_host_status`].
#[must_use]
pub fn worst_vm_status(vms: &[VmState]) -> (&'static str, String) {
    let mut worst = "ok";
    let mut offenders: Vec<String> = Vec::new();
    for vm in vms {
        let s = vm_status_from_state(&vm.power, vm.auto_poweron);
        if s != "ok" {
            offenders.push(format!("{} ({})", vm.name, vm.power));
        }
        if status_rank(s) > status_rank(worst) {
            worst = s;
        }
    }
    let detail = if offenders.is_empty() {
        format!("{} VM(s) healthy", vms.len())
    } else {
        offenders.join(", ")
    };
    (worst, detail)
}

// ---- pure pool-degraded helpers (unit-tested without a subprocess) ----

/// One pool host's placement liveness as read from `xe host-list`: its name + its
/// `enabled` flag (false ⇒ in maintenance / disabled) + its `host-metrics-live`
/// flag (false ⇒ not live in the pool — unreachable / fenced). Decoded by
/// [`parse_host_states`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PoolHostState {
    /// The host's name-label (for the alert detail).
    pub name: String,
    /// `enabled` — a disabled host takes no new VMs (maintenance mode).
    pub enabled: bool,
    /// `host-metrics-live` — false means XAPI no longer hears from the host: it's
    /// not live in the pool (fenced / down / partitioned).
    pub live: bool,
}

/// Parse the remote pool helper's `name|enabled|host-metrics-live` lines into
/// [`PoolHostState`]s. Each flag is true only for the literal `true` token (XAPI
/// prints booleans as `true`/`false`); absent/empty ⇒ false (fail-safe: an
/// unreadable flag reads as the degraded value). Skips lines with an empty name.
/// Pure — fed the raw stdout.
#[must_use]
pub fn parse_host_states(output: &str) -> Vec<PoolHostState> {
    output
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(3, '|');
            let name = p.next()?.trim();
            if name.is_empty() {
                return None;
            }
            let enabled = p.next().unwrap_or("").trim().eq_ignore_ascii_case("true");
            let live = p.next().unwrap_or("").trim().eq_ignore_ascii_case("true");
            Some(PoolHostState {
                name: name.to_string(),
                enabled,
                live,
            })
        })
        .collect()
}

/// Map one pool host's `(enabled, host-metrics-live)` to a health status:
/// * not live ⇒ `"fail"` — XAPI lost the host (fenced / down): the pool is
///   degraded, a guest on it is unreachable.
/// * live but disabled ⇒ `"warn"` — maintenance mode: it takes no new VMs but is
///   still reachable; surface it but don't page.
/// * live + enabled ⇒ `"ok"`.
///
/// Pure — the pool-degraded decision is data, not I/O.
#[must_use]
pub fn host_status_from_flags(enabled: bool, live: bool) -> &'static str {
    if !live {
        "fail"
    } else if !enabled {
        "warn"
    } else {
        "ok"
    }
}

/// Roll a dom0's pool hosts into one `(status, detail)` for its `pool:<dom0>`
/// health check: the WORST per-host status wins (`fail` > `warn` > `ok`), and the
/// detail names the offending hosts (or "N host(s) live" when all are ok). Pure +
/// testable.
#[must_use]
pub fn worst_host_status(hosts: &[PoolHostState]) -> (&'static str, String) {
    let mut worst = "ok";
    let mut offenders: Vec<String> = Vec::new();
    for h in hosts {
        let s = host_status_from_flags(h.enabled, h.live);
        if s != "ok" {
            let why = if !h.live { "not-live" } else { "disabled" };
            offenders.push(format!("{} ({why})", h.name));
        }
        if status_rank(s) > status_rank(worst) {
            worst = s;
        }
    }
    let detail = if offenders.is_empty() {
        format!("{} host(s) live", hosts.len())
    } else {
        offenders.join(", ")
    };
    (worst, detail)
}

/// Severity rank shared by the VM-crash + pool-degraded rollups: `fail` > `warn` >
/// `ok`. Pure.
#[must_use]
fn status_rank(status: &str) -> u8 {
    match status {
        "fail" => 2,
        "warn" => 1,
        _ => 0,
    }
}

// ---- pure logs-aggregation fold (unit-tested without a subprocess) ----

/// How many recent journal lines to pull per dom0 each aggregation pass. Keeps the
/// aggregation BOUNDED — a coarse health worker should never slurp a host's whole
/// journal, only its recent tail. `journalctl -n <this>` caps the remote read.
pub const LOG_TAIL_LINES: usize = 200;

/// Map a syslog `PRIORITY` (0-7, emergency..debug) to the `fleet_logs`
/// level string. The aggregator only forwards warnings-and-up (it asks
/// `journalctl -p warning`), but the full mapping keeps the fold honest if the
/// caller widens the window. Pure.
#[must_use]
pub fn syslog_priority_to_level(priority: u8) -> &'static str {
    match priority {
        0..=3 => "error", // emerg/alert/crit/err
        4 => "warn",      // warning
        5 | 6 => "info",  // notice/info
        _ => "debug",     // debug + anything out of range
    }
}

/// Fold one dom0's `journalctl -o json` output (one JSON object per line) into
/// [`magic_fleet::structured_log::LogRecord`]s tagged with this dom0's `host`
/// label, so the controller's Fleet-logs / Datacenter-Logs view reads host + VM +
/// service journal lines beside the mesh's own. Tolerant: a line that isn't a JSON
/// object, or that lacks a `MESSAGE`, is skipped (never aborts the fold). Bounded
/// by the caller's `journalctl -n` tail. Pure — fed the raw stdout, returns the
/// records to append.
///
/// Field mapping (systemd export journal):
/// * `__REALTIME_TIMESTAMP` (microseconds since epoch) → `ts_ms`;
/// * `PRIORITY` (syslog 0-7) → `level` via [`syslog_priority_to_level`];
/// * `_SYSTEMD_UNIT` else `SYSLOG_IDENTIFIER` → `target` (the emitting unit);
/// * `MESSAGE` → `message`.
///
/// Every record carries `fields["dom0"] = <host>` + `fields["source"] = "journal"`
/// so a Logs view can tell aggregated dom0 journal lines from native mesh logs.
#[must_use]
pub fn journal_lines_to_records(
    host: &str,
    output: &str,
) -> Vec<magic_fleet::structured_log::LogRecord> {
    output
        .lines()
        .filter_map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).ok()?;
            let message = journal_str(&v, "MESSAGE")?;
            let ts_ms = journal_str(&v, "__REALTIME_TIMESTAMP")
                .and_then(|s| s.parse::<u64>().ok())
                .map_or(0, |us| us / 1_000);
            let priority = journal_str(&v, "PRIORITY")
                .and_then(|s| s.parse::<u8>().ok())
                .unwrap_or(6);
            let target = journal_str(&v, "_SYSTEMD_UNIT")
                .or_else(|| journal_str(&v, "SYSLOG_IDENTIFIER"))
                .unwrap_or_default();
            let mut fields = BTreeMap::new();
            fields.insert("dom0".to_string(), host.to_string());
            fields.insert("source".to_string(), "journal".to_string());
            Some(magic_fleet::structured_log::LogRecord {
                ts_ms,
                host: host.to_string(),
                level: syslog_priority_to_level(priority).to_string(),
                target,
                message,
                fields,
            })
        })
        .collect()
}

/// Read one journal field as an owned `String`. systemd's JSON export prints most
/// fields as strings but binary/multiline fields as an array of bytes; this only
/// reads the string form (the fields the fold cares about are always strings),
/// returning `None` for an absent or non-string field. Pure.
#[must_use]
fn journal_str(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

// ---- thin I/O: run the probes, emit health events via the Bus ----

/// Publish one health record onto the Bus (best-effort, fire-and-reap — same lane
/// shape as the other dc workers' events).
fn publish(rec: &HealthRecord) {
    let mut cmd = std::process::Command::new("mde-bus");
    cmd.args(["publish", &rec.topic(), "--body-flag", &rec.body()]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// The etcd endpoint to health-check (`MCNF_ETCD`, else [`DEFAULT_ETCD`]).
fn etcd_endpoint() -> String {
    std::env::var("MCNF_ETCD").unwrap_or_else(|_| DEFAULT_ETCD.to_string())
}

/// One health observation a probe produced: the status + a short detail string.
struct Probe {
    status: &'static str,
    detail: String,
}

impl Probe {
    fn ok(detail: impl Into<String>) -> Self {
        Self {
            status: "ok",
            detail: detail.into(),
        }
    }
    fn warn(detail: impl Into<String>) -> Self {
        Self {
            status: "warn",
            detail: detail.into(),
        }
    }
    fn fail(detail: impl Into<String>) -> Self {
        Self {
            status: "fail",
            detail: detail.into(),
        }
    }
}

/// Probe a single dom0's SSH reachability with the mesh key (`ssh ... true`).
/// `ok` when the no-op command succeeds, `fail` otherwise (unreachable, auth,
/// timeout, or a missing `ssh` binary).
fn probe_dom0(key: &str, dom0: &str) -> Probe {
    let out = std::process::Command::new("ssh")
        .args([
            "-i",
            key,
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            &format!("root@{dom0}"),
            "true",
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => Probe::ok("ssh reachable"),
        Ok(o) => Probe::fail(format!(
            "ssh exit {}",
            o.status
                .code()
                .map_or_else(|| "signal".to_string(), |c| c.to_string())
        )),
        Err(e) => Probe::fail(format!("ssh spawn failed: {e}")),
    }
}

/// Probe the SUBSTRATE-V2 etcd store's `/health` endpoint via `curl`. `ok` when
/// the endpoint returns HTTP 200 with a healthy body, `fail` otherwise (non-200,
/// unhealthy json, unreachable, or a missing `curl` binary).
fn probe_etcd(endpoint: &str) -> Probe {
    let url = format!("{}/health", endpoint.trim_end_matches('/'));
    let out = std::process::Command::new("curl")
        .args([
            "-s",
            "-m",
            "8",
            "-o",
            "/dev/stdout",
            "-w",
            "\n%{http_code}",
            &url,
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout);
            let (body, code) = match text.trim_end().rsplit_once('\n') {
                Some((b, c)) => (b, c.trim()),
                None => ("", text.trim()),
            };
            let healthy = serde_json::from_str::<serde_json::Value>(body)
                .ok()
                .and_then(|v| {
                    v.get("health").and_then(|h| match h {
                        serde_json::Value::String(s) => Some(s == "true"),
                        serde_json::Value::Bool(b) => Some(*b),
                        _ => None,
                    })
                })
                .unwrap_or(false);
            if code == "200" && healthy {
                Probe::ok(format!("etcd healthy ({code})"))
            } else {
                Probe::fail(format!("etcd unhealthy (http {code})"))
            }
        }
        Ok(o) => Probe::fail(format!(
            "curl exit {}",
            o.status
                .code()
                .map_or_else(|| "signal".to_string(), |c| c.to_string())
        )),
        Err(e) => Probe::fail(format!("curl spawn failed: {e}")),
    }
}

/// Probe the mesh secret-store by fetching a known key via the repo's
/// `automation/secrets/mcnf-secret.sh get do-token` helper (best-effort, run from
/// the repo dir = the worker's current dir). `ok` on exit 0, else `warn` (the
/// secret store being down is degraded-but-not-dead — most checks don't need it
/// every tick).
fn probe_secret_store() -> Probe {
    let out = std::process::Command::new("bash")
        .args(["-lc", "automation/secrets/mcnf-secret.sh get do-token"])
        .output();
    match out {
        Ok(o) if o.status.success() => Probe::ok("secret fetch ok"),
        Ok(o) => Probe::warn(format!(
            "secret fetch exit {}",
            o.status
                .code()
                .map_or_else(|| "signal".to_string(), |c| c.to_string())
        )),
        Err(e) => Probe::warn(format!("secret helper spawn failed: {e}")),
    }
}

/// Locate the Nebula CA cert to inspect: the first that exists of `$NEBULA_CA_CRT`,
/// `<workgroup_root>/nebula/ca.crt`, `/etc/nebula/ca.crt`, then
/// `~/.config/nebula/ca.crt`. `None` when none of those exist (a fresh/un-enrolled
/// node) — the probe reports that honestly as `warn`, not `fail`.
fn locate_ca_cert(workgroup_root: &std::path::Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("NEBULA_CA_CRT") {
        if !p.is_empty() {
            candidates.push(PathBuf::from(p));
        }
    }
    candidates.push(workgroup_root.join("nebula").join("ca.crt"));
    candidates.push(PathBuf::from("/etc/nebula/ca.crt"));
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(
            PathBuf::from(home)
                .join(".config")
                .join("nebula")
                .join("ca.crt"),
        );
    }
    candidates.into_iter().find(|p| p.exists())
}

/// Probe the Nebula CA cert's expiry. Locates the CA cert (see
/// [`locate_ca_cert`]), runs `nebula-cert print -path <ca.crt>`, parses its
/// `Not after` date and reduces it to days-remaining via
/// [`cert_status_from_days`]. `warn` (honestly, not `fail`) when no CA cert is
/// found or `nebula-cert` is missing/unparseable — there is nothing to alert on.
fn probe_cert(workgroup_root: &std::path::Path) -> Probe {
    let Some(ca_crt) = locate_ca_cert(workgroup_root) else {
        return Probe::warn("no nebula CA cert found");
    };
    let out = std::process::Command::new("nebula-cert")
        .args(["print", "-path"])
        .arg(&ca_crt)
        .output();
    let stdout = match out {
        Ok(o) if o.status.success() => o.stdout,
        Ok(_) => return Probe::warn("nebula-cert print failed"),
        Err(_) => return Probe::warn("no nebula CA cert found"),
    };
    let text = String::from_utf8_lossy(&stdout);
    let Some(not_after) = parse_not_after(&text) else {
        return Probe::warn("nebula-cert: no Not after line");
    };
    // The Go tool prints e.g. `2027-01-01 00:00:00 +0000 UTC`; chrono parses the
    // `+0000` offset form. Drop the trailing ` UTC` label it appends.
    let to_parse = not_after.trim_end_matches(" UTC").trim();
    let Ok(expiry) = chrono::DateTime::parse_from_str(to_parse, "%Y-%m-%d %H:%M:%S %z") else {
        return Probe::warn(format!("nebula-cert: unparseable Not after '{not_after}'"));
    };
    let days = (expiry.timestamp() - chrono::Utc::now().timestamp()) / 86_400;
    match cert_status_from_days(days) {
        "fail" => Probe::fail(format!("CA expired {not_after}")),
        "warn" => Probe::warn(format!("CA expires {not_after}")),
        _ => Probe::ok(format!("CA expires {not_after} ({days}d)")),
    }
}

// ---- DATACENTER-24 xe-over-ssh probes (VM-crash + pool-degraded + logs) ----
//
// These reuse the orchestrator's DATACENTER-4 route resolution + argv assembly
// (`resolve_xe_route` + `ssh_xe_argv`) so an off-LAN node ProxyJumps through the
// configured relay exactly like the gather does — glue, not a second SSH layer
// (§6). The route is resolved once per dom0 here (the dedicated relay-down latch /
// `event/dc/route` publish lives in the orchestrator's gather; this read-only
// health probe just picks the route + runs).

/// Is this node on the dom0 lab LAN? Reuses the orchestrator's pure
/// [`crate::workers::datacenter_orchestrator::node_on_lan_for`] +
/// [`crate::workers::datacenter_orchestrator::local_ipv4s_from_ip_json`] over
/// `ip -j addr`, honoring the same `MCNF_XEN_ON_LAN` override for tests / odd
/// topologies. Missing `ip`/probe failure ⇒ on-LAN (the conservative Direct
/// default the gather also keeps).
fn dom0_on_lan() -> bool {
    if let Ok(v) = std::env::var("MCNF_XEN_ON_LAN") {
        let v = v.trim();
        if v == "1" || v.eq_ignore_ascii_case("true") {
            return true;
        }
        if v == "0" || v.eq_ignore_ascii_case("false") {
            return false;
        }
    }
    match std::process::Command::new("ip")
        .args(["-j", "addr"])
        .output()
    {
        Ok(o) if o.status.success() => crate::workers::datacenter_orchestrator::node_on_lan_for(
            &crate::workers::datacenter_orchestrator::local_ipv4s_from_ip_json(
                &String::from_utf8_lossy(&o.stdout),
            ),
        ),
        _ => true,
    }
}

/// Run one `xe`-bearing remote command on a dom0 along the DATACENTER-4 route,
/// returning its stdout on success. Best-effort: a non-zero exit / spawn failure /
/// unreachable dom0 ⇒ `None`. Reuses [`crate::workers::datacenter_orchestrator::ssh_xe_argv`]
/// so the ProxyJump shape matches the gather exactly.
fn ssh_run(key: &str, dom0: &str, on_lan: bool, remote: &str) -> Option<String> {
    let route = crate::workers::datacenter_orchestrator::resolve_xe_route(
        dom0,
        on_lan,
        crate::workers::datacenter_orchestrator::xen_relay_peer().as_deref(),
    );
    let argv = crate::workers::datacenter_orchestrator::ssh_xe_argv(key, dom0, &route, remote);
    let out = std::process::Command::new("ssh").args(argv).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Probe one dom0's non-control VMs for crashes. Pulls each VM's
/// `name-label|power-state|auto_poweron` over SSH, parses it ([`parse_vm_states`])
/// and rolls it into a worst-status ([`worst_vm_status`]): a VM the dom0 auto-
/// powers-on but which isn't `running` is a crash (`fail`); a `paused` guest is an
/// unexpected pause (`warn`). `warn` honestly (not `fail`) when the dom0 is
/// unreachable / `xe` is absent — there's nothing to assert.
fn probe_vm_crash(key: &str, dom0: &str, on_lan: bool) -> Probe {
    // One ssh round-trip: for each non-control VM, print
    // `name|power-state|auto_poweron`. `vm-param-get` of an absent other-config key
    // prints empty → parses as auto_poweron=false (a deliberate stop, not a crash).
    let script = "for u in $(xe vm-list is-control-domain=false params=uuid --minimal | tr , ' '); \
         do echo \"$(xe vm-param-get uuid=$u param-name=name-label)|$(xe vm-param-get uuid=$u param-name=power-state)|$(xe vm-param-get uuid=$u param-name=other-config param-key=auto_poweron 2>/dev/null)\"; done";
    match ssh_run(key, dom0, on_lan, script) {
        Some(out) => {
            let vms = parse_vm_states(&out);
            let (status, detail) = worst_vm_status(&vms);
            match status {
                "fail" => Probe::fail(detail),
                "warn" => Probe::warn(detail),
                _ => Probe::ok(detail),
            }
        }
        None => Probe::warn("vm-list unreachable"),
    }
}

/// Probe one dom0's pool for degraded hosts. Pulls each pool host's
/// `name-label|enabled|host-metrics-live` over SSH, parses it
/// ([`parse_host_states`]) and rolls it into a worst-status
/// ([`worst_host_status`]): a host not live in the pool is `fail`; a live-but-
/// disabled (maintenance) host is `warn`. `warn` honestly when the dom0 is
/// unreachable / `xe` is absent.
fn probe_pool(key: &str, dom0: &str, on_lan: bool) -> Probe {
    let script = "for u in $(xe host-list params=uuid --minimal | tr , ' '); \
         do echo \"$(xe host-param-get uuid=$u param-name=name-label)|$(xe host-param-get uuid=$u param-name=enabled)|$(xe host-param-get uuid=$u param-name=host-metrics-live 2>/dev/null)\"; done";
    match ssh_run(key, dom0, on_lan, script) {
        Some(out) => {
            let hosts = parse_host_states(&out);
            let (status, detail) = worst_host_status(&hosts);
            match status {
                "fail" => Probe::fail(detail),
                "warn" => Probe::warn(detail),
                _ => Probe::ok(detail),
            }
        }
        None => Probe::warn("host-list unreachable"),
    }
}

/// Aggregate one dom0's recent warnings-and-up journal tail into the
/// `fleet_logs` sink. Pulls a BOUNDED ([`LOG_TAIL_LINES`]) tail of
/// `journalctl -p warning -o json` over SSH, folds the lines into
/// [`magic_fleet::structured_log::LogRecord`]s tagged with this dom0
/// ([`journal_lines_to_records`]) and appends them under
/// `<workgroup_root>/logs/<dom0>.jsonl` — the same replicated sink the Fleet-logs
/// / Datacenter-Logs view reads. Best-effort + idempotent-enough: re-running
/// re-pulls the recent tail (the view de-dups by ts/host on render), so a missed
/// pass self-heals on the next tick. Returns how many records it appended.
fn aggregate_dom0_logs(
    key: &str,
    dom0: &str,
    on_lan: bool,
    workgroup_root: &std::path::Path,
) -> usize {
    // `-p warning` = warnings-and-up (priority ≤ 4), `--no-pager` so it doesn't
    // block, `-o json` for the structured fold, `-n <tail>` to stay bounded.
    let remote = format!("journalctl -p warning -o json --no-pager -n {LOG_TAIL_LINES}");
    let Some(out) = ssh_run(key, dom0, on_lan, &remote) else {
        return 0;
    };
    let records = journal_lines_to_records(dom0, &out);
    let mut appended = 0;
    for rec in &records {
        if magic_fleet::structured_log::append(workgroup_root, rec).is_ok() {
            appended += 1;
        }
    }
    appended
}

/// One health pass: run each best-effort probe, feed its `(check, status)`
/// through the dedup core, and publish the records that survive (status
/// transitions). Every probe is independent — a failed/absent tool degrades that
/// one check to `fail`/`warn` and never aborts the pass.
fn run_checks(core: &mut DcHealth, workgroup_root: &std::path::Path) {
    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    // DATACENTER-4 route resolution is per-dom0 but the on-LAN decision is one
    // probe per pass (the gather does the same): resolve it once here.
    let on_lan = dom0_on_lan();
    for dom0 in crate::workers::datacenter_orchestrator::xen_dom0s() {
        let check = format!("dom0:{dom0}");
        let p = probe_dom0(&key, &dom0);
        if let Some(rec) = core.observe(&check, p.status, &p.detail) {
            publish(&rec);
        }

        // VM-crash check for this dom0 (event/dc/health/vm-crash:<dom0>).
        let vm_check = format!("vm-crash:{dom0}");
        let p = probe_vm_crash(&key, &dom0, on_lan);
        if let Some(rec) = core.observe(&vm_check, p.status, &p.detail) {
            publish(&rec);
        }

        // Pool-degraded check for this dom0 (event/dc/health/pool:<dom0>).
        let pool_check = format!("pool:{dom0}");
        let p = probe_pool(&key, &dom0, on_lan);
        if let Some(rec) = core.observe(&pool_check, p.status, &p.detail) {
            publish(&rec);
        }

        // Logs aggregation: pull this dom0's recent journal tail into the
        // fleet_logs sink so the Datacenter Logs view reads host/VM/service
        // journal lines beside the mesh's own. Best-effort; bounded.
        aggregate_dom0_logs(&key, &dom0, on_lan, workgroup_root);
    }

    let p = probe_etcd(&etcd_endpoint());
    if let Some(rec) = core.observe("etcd", p.status, &p.detail) {
        publish(&rec);
    }

    let p = probe_secret_store();
    if let Some(rec) = core.observe("secret-store", p.status, &p.detail) {
        publish(&rec);
    }

    let p = probe_cert(workgroup_root);
    if let Some(rec) = core.observe("cert", p.status, &p.detail) {
        publish(&rec);
    }
}

/// The supervised worker. Leader-gated (only the elected node probes + publishes,
/// so a multi-node mesh doesn't multi-publish) and best-effort.
pub struct DcHealthWorker {
    core: DcHealth,
    tick_interval: Duration,
    node_id: String,
    leader_lock: PathBuf,
    workgroup_root: PathBuf,
}

impl DcHealthWorker {
    /// Construct with production defaults (30 s tick, the shared leader lock
    /// under `workgroup_root`).
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        Self {
            core: DcHealth::new(),
            tick_interval: DEFAULT_TICK_INTERVAL,
            leader_lock: workgroup_root.join(".mackesd-leader.lock"),
            workgroup_root,
            node_id,
        }
    }

    /// Only the directory leader runs the checks (no-fixed-center: any eligible
    /// node can be it, the elected one publishes). Reuses the shared leader lock.
    fn is_leader(&self) -> bool {
        crate::leader_gate::LeaderGate::from_lock_path(
            self.leader_lock.clone(),
            self.node_id.clone(),
        )
        .is_leader()
    }
}

#[async_trait::async_trait]
impl Worker for DcHealthWorker {
    fn name(&self) -> &'static str {
        "dc_health"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            if self.is_leader() {
                run_checks(&mut self.core, &self.workgroup_root);
            }
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(self.tick_interval) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_topic_formats_under_event_dc_health() {
        assert_eq!(health_topic("etcd"), "event/dc/health/etcd");
        assert_eq!(
            health_topic("dom0:172.20.0.9"),
            "event/dc/health/dom0:172.20.0.9"
        );
        assert_eq!(health_topic("secret-store"), "event/dc/health/secret-store");
    }

    #[test]
    fn observe_emits_on_transition_and_dedups_same_status() {
        let mut h = DcHealth::new();
        // First sight → a record on the right topic with status + detail.
        let rec = h
            .observe("etcd", "ok", "etcd healthy (200)")
            .expect("first sight emits");
        assert_eq!(rec.check, "etcd");
        assert_eq!(rec.status, "ok");
        assert_eq!(rec.topic(), "event/dc/health/etcd");
        let body = rec.body();
        assert!(body.contains(r#""check":"etcd""#));
        assert!(body.contains(r#""status":"ok""#));
        assert!(body.contains("etcd healthy"));
        // Same status (still ok) → no re-emit even if the detail changed.
        assert!(h
            .observe("etcd", "ok", "etcd healthy (200) again")
            .is_none());
        // Status transition ok→fail → a fresh record.
        let rec2 = h
            .observe("etcd", "fail", "etcd unhealthy (http 503)")
            .expect("status transition emits");
        assert_eq!(rec2.status, "fail");
        // Re-poll of the same fail status → no re-emit.
        assert!(h
            .observe("etcd", "fail", "etcd unhealthy (http 503)")
            .is_none());
    }

    #[test]
    fn observe_tracks_status_per_check_independently() {
        let mut h = DcHealth::new();
        // Three distinct checks each get their own first-sight record.
        assert!(h
            .observe("dom0:172.20.0.9", "ok", "ssh reachable")
            .is_some());
        assert!(h
            .observe("dom0:172.20.145.193", "fail", "ssh exit 255")
            .is_some());
        assert!(h
            .observe("secret-store", "warn", "secret fetch exit 1")
            .is_some());
        // dom0:.9 goes down — its own independent transition.
        let r = h
            .observe("dom0:172.20.0.9", "fail", "ssh exit 255")
            .expect("dom0 ok→fail");
        assert_eq!(r.check, "dom0:172.20.0.9");
        assert_eq!(r.status, "fail");
        // The others are unchanged → no re-emit.
        assert!(h
            .observe("dom0:172.20.145.193", "fail", "ssh exit 255")
            .is_none());
        assert!(h
            .observe("secret-store", "warn", "secret fetch exit 1")
            .is_none());
    }

    #[test]
    fn record_body_is_valid_json_with_all_fields() {
        let mut h = DcHealth::new();
        let rec = h
            .observe("secret-store", "warn", "secret helper spawn failed: nope")
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&rec.body()).unwrap();
        assert_eq!(v["check"], "secret-store");
        assert_eq!(v["status"], "warn");
        assert_eq!(v["detail"], "secret helper spawn failed: nope");
    }

    // A representative `nebula-cert print -path <ca.crt>` text document — the
    // indented block the Go tool emits. Only the `Not after` line is asserted;
    // the rest mirrors the real shape so the line selector is exercised.
    const SAMPLE_CERT_PRINT: &str = "NebulaCertificate {\n\
        \tDetails {\n\
        \t\tName: magic-mesh-ca\n\
        \t\tIps: []\n\
        \t\tSubnets: []\n\
        \t\tGroups: []\n\
        \t\tNot before: 2026-01-01 00:00:00 +0000 UTC\n\
        \t\tNot after: 2027-01-01 00:00:00 +0000 UTC\n\
        \t\tIs CA: true\n\
        \t}\n\
        \tFingerprint: deadbeef\n\
        }\n";

    #[test]
    fn cert_status_thresholds_fail_warn_ok() {
        // Already past Not after → fail.
        assert_eq!(cert_status_from_days(-1), "fail");
        assert_eq!(cert_status_from_days(-365), "fail");
        // Inside the 30-day lead-time window → warn (incl. the 0-day boundary).
        assert_eq!(cert_status_from_days(0), "warn");
        assert_eq!(cert_status_from_days(29), "warn");
        // 30 days and beyond → ok (boundary is exclusive of the warn window).
        assert_eq!(cert_status_from_days(CERT_WARN_DAYS), "ok");
        assert_eq!(cert_status_from_days(31), "ok");
        assert_eq!(cert_status_from_days(400), "ok");
    }

    #[test]
    fn parse_not_after_extracts_the_date_line() {
        assert_eq!(
            parse_not_after(SAMPLE_CERT_PRINT).as_deref(),
            Some("2027-01-01 00:00:00 +0000 UTC")
        );
    }

    #[test]
    fn parse_not_after_rejects_garbage_output() {
        // No `Not after:` line at all.
        assert!(parse_not_after("total garbage, not a cert").is_none());
        assert!(parse_not_after("").is_none());
        // A `Not before:` line must NOT be mistaken for `Not after:`.
        assert!(parse_not_after("\t\tNot before: 2026-01-01 00:00:00 +0000 UTC").is_none());
        // Present but empty value → None (nothing to compare).
        assert!(parse_not_after("Not after:   ").is_none());
    }

    #[test]
    fn parse_not_after_date_is_chrono_parseable() {
        // The extracted line (sans the trailing ` UTC` label) round-trips
        // through the same parse the probe uses, so the threshold mapping is
        // reachable end-to-end.
        let raw = parse_not_after(SAMPLE_CERT_PRINT).unwrap();
        let to_parse = raw.trim_end_matches(" UTC").trim();
        let dt = chrono::DateTime::parse_from_str(to_parse, "%Y-%m-%d %H:%M:%S %z").unwrap();
        // 2027-01-01T00:00:00Z == 1_798_761_600.
        assert_eq!(dt.timestamp(), 1_798_761_600);
    }

    #[test]
    fn detail_truncates_at_the_cap_on_char_boundary() {
        let long = "x".repeat(500);
        let mut h = DcHealth::new();
        let rec = h.observe("etcd", "fail", &long).unwrap();
        assert_eq!(rec.detail.chars().count(), DETAIL_LEN);
        // Multibyte detail — truncation must not split a char (valid utf8, no panic).
        let multi = "é".repeat(500);
        let rec2 = h.observe("dom0:x", "fail", &multi).unwrap();
        assert_eq!(rec2.detail.chars().count(), DETAIL_LEN);
    }

    // ---- VM-crash check ----

    #[test]
    fn vm_status_maps_power_and_auto_poweron() {
        // A running guest is always ok, regardless of the auto-poweron flag.
        assert_eq!(vm_status_from_state("running", true), "ok");
        assert_eq!(vm_status_from_state("running", false), "ok");
        // A paused guest is an unexpected pause → warn, even without auto-poweron.
        assert_eq!(vm_status_from_state("paused", false), "warn");
        assert_eq!(vm_status_from_state("paused", true), "warn");
        // Not-running AND auto-poweron (the dom0 expects it up) → a crash (fail).
        assert_eq!(vm_status_from_state("halted", true), "fail");
        assert_eq!(vm_status_from_state("suspended", true), "fail");
        // Not-running but NOT auto-poweron = a deliberate stop → ok (no alert).
        assert_eq!(vm_status_from_state("halted", false), "ok");
    }

    #[test]
    fn parse_vm_states_decodes_pipe_lines_and_skips_blanks() {
        let out = "web|running|true\n\
                   db|halted|true\n\
                   scratch|halted|false\n\
                   |running|true\n"; // empty name skipped
        let vms = parse_vm_states(out);
        assert_eq!(vms.len(), 3);
        assert_eq!(
            vms[0],
            VmState {
                name: "web".into(),
                power: "running".into(),
                auto_poweron: true
            }
        );
        assert!(
            !vms[2].auto_poweron,
            "auto_poweron only true for the literal token"
        );
    }

    #[test]
    fn worst_vm_status_takes_the_worst_and_names_offenders() {
        // All healthy → ok + a count.
        let healthy = vec![
            VmState {
                name: "a".into(),
                power: "running".into(),
                auto_poweron: true,
            },
            VmState {
                name: "b".into(),
                power: "halted".into(),
                auto_poweron: false,
            },
        ];
        let (s, d) = worst_vm_status(&healthy);
        assert_eq!(s, "ok");
        assert_eq!(d, "2 VM(s) healthy");
        // A warn (paused) plus a fail (auto-poweron down) → fail wins, both named.
        let mixed = vec![
            VmState {
                name: "a".into(),
                power: "running".into(),
                auto_poweron: true,
            },
            VmState {
                name: "p".into(),
                power: "paused".into(),
                auto_poweron: false,
            },
            VmState {
                name: "c".into(),
                power: "halted".into(),
                auto_poweron: true,
            },
        ];
        let (s, d) = worst_vm_status(&mixed);
        assert_eq!(s, "fail");
        assert!(d.contains("p (paused)") && d.contains("c (halted)"));
    }

    // ---- pool-degraded check ----

    #[test]
    fn host_status_maps_enabled_and_live() {
        // Live + enabled → ok.
        assert_eq!(host_status_from_flags(true, true), "ok");
        // Live but disabled (maintenance) → warn.
        assert_eq!(host_status_from_flags(false, true), "warn");
        // Not live (fenced / down) → fail, regardless of enabled.
        assert_eq!(host_status_from_flags(true, false), "fail");
        assert_eq!(host_status_from_flags(false, false), "fail");
    }

    #[test]
    fn parse_host_states_decodes_and_fail_safes_missing_flags() {
        let out = "xcp-a|true|true\n\
                   xcp-b|false|true\n\
                   xcp-c|true|\n"; // missing live flag → false (degraded)
        let hosts = parse_host_states(out);
        assert_eq!(hosts.len(), 3);
        assert_eq!(
            hosts[0],
            PoolHostState {
                name: "xcp-a".into(),
                enabled: true,
                live: true
            }
        );
        assert!(!hosts[2].live, "an absent flag reads as the degraded value");
    }

    #[test]
    fn worst_host_status_takes_the_worst_and_names_offenders() {
        let healthy = vec![PoolHostState {
            name: "xcp-a".into(),
            enabled: true,
            live: true,
        }];
        let (s, d) = worst_host_status(&healthy);
        assert_eq!(s, "ok");
        assert_eq!(d, "1 host(s) live");
        let degraded = vec![
            PoolHostState {
                name: "xcp-a".into(),
                enabled: false,
                live: true,
            }, // warn
            PoolHostState {
                name: "xcp-b".into(),
                enabled: true,
                live: false,
            }, // fail
        ];
        let (s, d) = worst_host_status(&degraded);
        assert_eq!(s, "fail");
        assert!(d.contains("xcp-a (disabled)") && d.contains("xcp-b (not-live)"));
    }

    // ---- logs aggregation fold ----

    #[test]
    fn syslog_priority_maps_to_fleet_level() {
        assert_eq!(syslog_priority_to_level(0), "error"); // emerg
        assert_eq!(syslog_priority_to_level(3), "error"); // err
        assert_eq!(syslog_priority_to_level(4), "warn"); // warning
        assert_eq!(syslog_priority_to_level(5), "info"); // notice
        assert_eq!(syslog_priority_to_level(6), "info"); // info
        assert_eq!(syslog_priority_to_level(7), "debug"); // debug
        assert_eq!(syslog_priority_to_level(99), "debug"); // out of range
    }

    #[test]
    fn journal_fold_maps_fields_and_tags_the_dom0() {
        // Two valid `-o json` lines + one junk line (skipped) + one without MESSAGE.
        let out = "{\"__REALTIME_TIMESTAMP\":\"1700000000000000\",\"PRIORITY\":\"3\",\"_SYSTEMD_UNIT\":\"xapi.service\",\"MESSAGE\":\"toolstack restart\"}\n\
                   not json at all\n\
                   {\"PRIORITY\":\"4\",\"SYSLOG_IDENTIFIER\":\"kernel\",\"MESSAGE\":\"oom-killer invoked\"}\n\
                   {\"PRIORITY\":\"3\"}\n"; // no MESSAGE → skipped
        let recs = journal_lines_to_records("172.20.0.9", out);
        assert_eq!(recs.len(), 2, "junk + the MESSAGE-less line are dropped");
        // First record: microseconds → ms, priority 3 → error, _SYSTEMD_UNIT target.
        assert_eq!(recs[0].ts_ms, 1_700_000_000_000);
        assert_eq!(recs[0].level, "error");
        assert_eq!(recs[0].target, "xapi.service");
        assert_eq!(recs[0].message, "toolstack restart");
        assert_eq!(recs[0].host, "172.20.0.9");
        assert_eq!(
            recs[0].fields.get("dom0").map(String::as_str),
            Some("172.20.0.9")
        );
        assert_eq!(
            recs[0].fields.get("source").map(String::as_str),
            Some("journal")
        );
        // Second: no _SYSTEMD_UNIT → falls back to SYSLOG_IDENTIFIER; priority 4 → warn.
        assert_eq!(recs[1].level, "warn");
        assert_eq!(recs[1].target, "kernel");
        // Missing timestamp → 0 (the fold never panics on a partial record).
        assert_eq!(recs[1].ts_ms, 0);
    }

    #[test]
    fn journal_fold_round_trips_through_the_fleet_logs_sink() {
        // End-to-end reachability: the fold's records append into the SAME
        // `<root>/logs/<host>.jsonl` the Fleet-logs / Datacenter-Logs view reads.
        let tmp = tempfile::tempdir().unwrap();
        let out = "{\"__REALTIME_TIMESTAMP\":\"1700000001000000\",\"PRIORITY\":\"4\",\"_SYSTEMD_UNIT\":\"nebula.service\",\"MESSAGE\":\"handshake retry\"}\n";
        for rec in journal_lines_to_records("172.20.0.9", out) {
            magic_fleet::structured_log::append(tmp.path(), &rec).unwrap();
        }
        let back = magic_fleet::structured_log::read_host(tmp.path(), "172.20.0.9");
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].message, "handshake retry");
        assert_eq!(back[0].level, "warn");
    }
}
