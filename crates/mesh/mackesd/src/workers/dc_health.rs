//! DATACENTER-24 — the passive `dc_health` care-and-feeding health checker.
//!
//! A read-only companion to [`super::datacenter_orchestrator`] +
//! [`super::dc_auditor`]: where the orchestrator publishes per-resource state and
//! the auditor records each action request, this worker periodically probes the
//! datacenter substrate's **liveness** — each configured Xen dom0's SSH
//! reachability, the SUBSTRATE-V2 etcd store's `/health`, and the mesh
//! secret-store helper — and publishes one `event/dc/health/<check>` per check,
//! WITHOUT touching the action handlers or the substrate it watches. It is a pure
//! side-observer: nothing depends on it, so it can never wedge an action.
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

/// Days an issued-but-unredeemed join bearer may sit pending before the
/// `token-expiry` check flags it `warn`: a dangling join token is a live
/// credential the operator should redeem or revoke. A week is a generous window
/// for a normal enrollment to complete.
pub const TOKEN_STALE_WARN_DAYS: i64 = 7;

/// How many recent warning+ dom0 journal lines [`aggregate_dom0_logs`] pulls per
/// pass (bounded so the SSH stays cheap; dedup keeps re-fetched lines from
/// re-appending).
pub const DOM0_LOG_TAIL: usize = 25;

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

// ---- pure token-expiry helpers (the issued-bearer join-token ledger) ----

/// Map a pending join bearer's age-in-days to a health status: `"warn"` once it
/// has sat unredeemed for at least [`TOKEN_STALE_WARN_DAYS`], else `"ok"`. A
/// dangling token never `fail`s — it is a credential to clean up, not an outage.
#[must_use]
pub fn token_status_from_age_days(age_days: i64) -> &'static str {
    if age_days >= TOKEN_STALE_WARN_DAYS {
        "warn"
    } else {
        "ok"
    }
}

/// Extract `issued_at_ms` out of one bearer-ledger entry's JSON
/// (`{"issued_at_ms":<u64>,"note":…}`, the shape [`crate::bearer_ledger::issue`]
/// writes). PURE. `None` when absent/unparseable, and `0` (the `record_issued`
/// sentinel) maps to `Some(0)` so a recorded-without-timestamp token reads as
/// "epoch old" → always stale-warned (it is a real dangling credential).
#[must_use]
pub fn parse_issued_at_ms(entry_json: &str) -> Option<u64> {
    serde_json::from_str::<serde_json::Value>(entry_json)
        .ok()?
        .get("issued_at_ms")
        .and_then(serde_json::Value::as_u64)
}

// ---- pure VM-crash + pool-degraded helpers (parse `xl`/`xe` output) ----

/// Parse `xl list` output and return the names of domains in the **crashed**
/// state. PURE.
///
/// `xl list` prints a header (`Name ID Mem VCPUs State Time(s)`) then one row per
/// domain; the `State` column is the 5th whitespace field, a 6-char flag string
/// (`r-----`, `--p---`, …) where a `c` in the 5th position means *crashed*. This
/// skips the header + the control domain (`Domain-0`) and returns every other
/// domain whose `State` field contains a `c`.
#[must_use]
pub fn parse_crashed_vms(xl_list: &str) -> Vec<String> {
    let mut crashed = Vec::new();
    for line in xl_list.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        // Need at least Name, ID, Mem, VCPUs, State.
        if cols.len() < 5 {
            continue;
        }
        let name = cols[0];
        // Skip the header row and the control domain.
        if name == "Name" || name == "Domain-0" {
            continue;
        }
        let state = cols[4];
        // The state flag string carries 'c' when the domain has crashed.
        if state.contains('c') {
            crashed.push(name.to_string());
        }
    }
    crashed
}

/// Reduce a `xe host-list params=enabled --minimal` CSV (`true,false,…`) to a
/// pool-health `(status, enabled_count, total)`. PURE.
///
/// XCP's `--minimal` prints the `enabled` field for every host as a
/// comma-separated list. A pool is `"fail"` (degraded) when any host is disabled,
/// `"ok"` when all are enabled, and `"warn"` when the output is empty/unparseable
/// (nothing to assert on — honest, not a false alarm).
#[must_use]
pub fn pool_status_from_enabled(enabled_csv: &str) -> (&'static str, usize, usize) {
    let fields: Vec<&str> = enabled_csv
        .trim()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if fields.is_empty() {
        return ("warn", 0, 0);
    }
    let total = fields.len();
    let enabled = fields.iter().filter(|f| f.eq_ignore_ascii_case("true")).count();
    if enabled == total {
        ("ok", enabled, total)
    } else {
        ("fail", enabled, total)
    }
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

/// Probe the issued-bearer join-token ledger for stale (long-pending) tokens.
/// Reads `<workgroup_root>/ca/issued-bearers/*.json` (the
/// [`crate::bearer_ledger`] store), finds the oldest still-pending bearer, and
/// maps its age via [`token_status_from_age_days`]. `ok` ("no pending tokens")
/// when the ledger is empty/absent — a fresh node has nothing to warn about.
fn probe_token(workgroup_root: &std::path::Path) -> Probe {
    let dir = crate::bearer_ledger::ledger_dir(workgroup_root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Probe::ok("no pending tokens");
    };
    let now_ms = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0);
    let mut oldest_age_days: Option<i64> = None;
    let mut pending = 0_usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(issued_ms) = parse_issued_at_ms(&contents) else {
            continue;
        };
        pending += 1;
        let age_days = i64::try_from(now_ms.saturating_sub(issued_ms) / 86_400_000)
            .unwrap_or(i64::MAX);
        oldest_age_days = Some(oldest_age_days.map_or(age_days, |o| o.max(age_days)));
    }
    match oldest_age_days {
        None => Probe::ok("no pending tokens"),
        Some(age) => {
            let detail = format!("{pending} pending; oldest {age}d");
            match token_status_from_age_days(age) {
                "warn" => Probe::warn(detail),
                _ => Probe::ok(detail),
            }
        }
    }
}

/// Probe a dom0 for crashed guests via `xl list` over the mesh key. `fail` (with
/// the crashed VM names) when any guest is in the crashed state, `ok` otherwise.
/// `warn` when the toolstack/SSH is unreachable — that liveness is the `dom0:*`
/// check's job, so vm-crash stays quiet rather than double-alarming.
fn probe_vm_crash(key: &str, dom0: &str) -> Probe {
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
            "xl list",
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let crashed = parse_crashed_vms(&String::from_utf8_lossy(&o.stdout));
            if crashed.is_empty() {
                Probe::ok("no crashed vms")
            } else {
                Probe::fail(format!("crashed: {}", crashed.join(",")))
            }
        }
        Ok(_) => Probe::warn("xl list failed"),
        Err(e) => Probe::warn(format!("ssh spawn failed: {e}")),
    }
}

/// Probe a dom0's pool health via `xe host-list params=enabled --minimal` over
/// the mesh key, reduced by [`pool_status_from_enabled`]. `fail` when any pool
/// host is disabled (degraded), `ok` when all are enabled, `warn` when the
/// toolstack/SSH is unreachable.
fn probe_pool(key: &str, dom0: &str) -> Probe {
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
            "xe host-list params=enabled --minimal",
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let (status, enabled, total) =
                pool_status_from_enabled(&String::from_utf8_lossy(&o.stdout));
            let detail = format!("{enabled}/{total} hosts enabled");
            match status {
                "fail" => Probe::fail(detail),
                "ok" => Probe::ok(detail),
                _ => Probe::warn("pool state unknown"),
            }
        }
        Ok(_) => Probe::warn("host-list failed"),
        Err(e) => Probe::warn(format!("ssh spawn failed: {e}")),
    }
}

/// DATACENTER-24 (logs half) — per-resource log dedup cursor.
///
/// dom0s run no mackesd (XCP-6), so their journals never reach the OBS-5
/// structured-log sink the Fleet-logs panel reads. [`aggregate_dom0_logs`] tails
/// each dom0's warning+ journal and republishes the NEW lines into that same sink
/// under `host = "dom0:<ip>"` (the per-resource view) + an `event/dc/logs/*`
/// digest. This tracks the last line already forwarded per resource so a
/// re-fetched tail never re-appends the same lines every tick.
#[derive(Default)]
pub struct DcLogs {
    last_seen: BTreeMap<String, String>,
}

impl DcLogs {
    /// Fresh aggregator with no forwarded lines.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Given a `resource`'s current ordered log `lines` (oldest→newest), return
    /// the suffix not yet forwarded and advance the cursor to the newest line.
    /// PURE. If the previously-seen line is no longer present (rotation), the
    /// whole batch is treated as new.
    pub fn new_lines(&mut self, resource: &str, lines: &[String]) -> Vec<String> {
        let trimmed: Vec<String> = lines
            .iter()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        if trimmed.is_empty() {
            return Vec::new();
        }
        let fresh: Vec<String> = match self.last_seen.get(resource) {
            Some(last) => match trimmed.iter().rposition(|l| l == last) {
                // Everything after the last-seen line is new.
                Some(idx) => trimmed[idx + 1..].to_vec(),
                // Cursor rotated out of the window → forward the whole batch.
                None => trimmed.clone(),
            },
            None => trimmed.clone(),
        };
        if let Some(newest) = trimmed.last() {
            self.last_seen.insert(resource.to_string(), newest.clone());
        }
        fresh
    }
}

/// Bus topic a logs digest for `resource` is published to: `event/dc/logs/<resource>`.
#[must_use]
pub fn logs_topic(resource: &str) -> String {
    format!("event/dc/logs/{resource}")
}

/// DATACENTER-24 (logs half) — tail each reachable dom0's warning+ journal and
/// forward the NEW lines into the OBS-5 structured-log sink (the Fleet-logs
/// panel's source) under `host = "dom0:<ip>"`, plus a per-resource
/// `event/dc/logs/dom0:<ip>` digest. Best-effort + deduped; a failed SSH/append
/// degrades silently.
fn aggregate_dom0_logs(logs: &mut DcLogs, workgroup_root: &std::path::Path, key: &str, dom0: &str) {
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
            &format!("journalctl -p warning -n {DOM0_LOG_TAIL} -o cat --no-pager 2>/dev/null"),
        ])
        .output();
    let Ok(o) = out else { return };
    if !o.status.success() {
        return;
    }
    let resource = format!("dom0:{dom0}");
    let lines: Vec<String> = String::from_utf8_lossy(&o.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    let fresh = logs.new_lines(&resource, &lines);
    if fresh.is_empty() {
        return;
    }
    let now_ms = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0);
    for msg in &fresh {
        let rec = magic_fleet::structured_log::LogRecord {
            ts_ms: now_ms,
            host: resource.clone(),
            level: "warn".to_string(),
            target: "dom0-journal".to_string(),
            message: msg.clone(),
            fields: std::collections::BTreeMap::new(),
        };
        let _ = magic_fleet::structured_log::append(workgroup_root, &rec);
    }
    // Per-resource digest onto the Bus (count + newest line).
    let body = serde_json::json!({
        "resource": resource,
        "new_lines": fresh.len(),
        "newest": fresh.last().cloned().unwrap_or_default(),
    })
    .to_string();
    let mut cmd = std::process::Command::new("mde-bus");
    cmd.args(["publish", &logs_topic(&resource), "--body-flag", &body]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// One health pass: run each best-effort probe, feed its `(check, status)`
/// through the dedup core, and publish the records that survive (status
/// transitions). Every probe is independent — a failed/absent tool degrades that
/// one check to `fail`/`warn` and never aborts the pass.
fn run_checks(core: &mut DcHealth, logs: &mut DcLogs, workgroup_root: &std::path::Path) {
    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    for dom0 in crate::workers::datacenter_orchestrator::xen_dom0s() {
        let check = format!("dom0:{dom0}");
        let p = probe_dom0(&key, &dom0);
        let reachable = p.status == "ok";
        if let Some(rec) = core.observe(&check, p.status, &p.detail) {
            publish(&rec);
        }
        // The per-dom0 Xen checks + log aggregation only make sense when the host
        // is reachable — skip them otherwise (the dom0:* check already alarms).
        if reachable {
            let p = probe_vm_crash(&key, &dom0);
            if let Some(rec) = core.observe(&format!("vm-crash:{dom0}"), p.status, &p.detail) {
                publish(&rec);
            }
            let p = probe_pool(&key, &dom0);
            if let Some(rec) = core.observe(&format!("pool:{dom0}"), p.status, &p.detail) {
                publish(&rec);
            }
            aggregate_dom0_logs(logs, workgroup_root, &key, &dom0);
        }
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

    let p = probe_token(workgroup_root);
    if let Some(rec) = core.observe("token-expiry", p.status, &p.detail) {
        publish(&rec);
    }
}

/// The supervised worker. Leader-gated (only the elected node probes + publishes,
/// so a multi-node mesh doesn't multi-publish) and best-effort.
pub struct DcHealthWorker {
    core: DcHealth,
    logs: DcLogs,
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
            logs: DcLogs::new(),
            tick_interval: DEFAULT_TICK_INTERVAL,
            leader_lock: workgroup_root.join(".mackesd-leader.lock"),
            workgroup_root,
            node_id,
        }
    }

    /// Only the directory leader runs the checks (no-fixed-center: any eligible
    /// node can be it, the elected one publishes). Reuses the shared leader lock.
    fn is_leader(&self) -> bool {
        matches!(
            crate::leader::try_acquire(&self.leader_lock, &self.node_id),
            Ok(crate::leader::AcquireResult::Acquired)
        )
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
                run_checks(&mut self.core, &mut self.logs, &self.workgroup_root);
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
    fn token_status_thresholds_warn_at_the_window() {
        assert_eq!(token_status_from_age_days(0), "ok");
        assert_eq!(token_status_from_age_days(TOKEN_STALE_WARN_DAYS - 1), "ok");
        // At and beyond the window → warn (a dangling credential, never fail).
        assert_eq!(token_status_from_age_days(TOKEN_STALE_WARN_DAYS), "warn");
        assert_eq!(token_status_from_age_days(90), "warn");
    }

    #[test]
    fn parse_issued_at_ms_reads_the_ledger_shape() {
        assert_eq!(
            parse_issued_at_ms(r#"{"issued_at_ms":1700000000000,"note":"box"}"#),
            Some(1_700_000_000_000)
        );
        // The record_issued sentinel (0) is a real, timestamp-less dangling token.
        assert_eq!(parse_issued_at_ms(r#"{"issued_at_ms":0,"note":"recorded"}"#), Some(0));
        // Garbage / missing field → None (skipped, not counted).
        assert_eq!(parse_issued_at_ms("not json"), None);
        assert_eq!(parse_issued_at_ms(r#"{"note":"x"}"#), None);
    }

    #[test]
    fn parse_crashed_vms_finds_only_crashed_guests() {
        // Header + Domain-0 + a running guest + a crashed guest + a paused guest.
        let xl = "Name                                        ID   Mem VCPUs\tState\tTime(s)\n\
                  Domain-0                                     0  4096     4     r-----    1234.5\n\
                  build-vm-1                                   3  8192     4     -b----     567.8\n\
                  test-vm-2                                    5  2048     2     --p---      12.3\n\
                  broken-vm-3                                  7  2048     2     ---c--      99.0\n";
        let crashed = parse_crashed_vms(xl);
        assert_eq!(crashed, vec!["broken-vm-3".to_string()]);
        // No crashed guests → empty.
        let healthy = "Name ID Mem VCPUs State Time(s)\n\
                       Domain-0 0 4096 4 r----- 1.0\n\
                       vm-a 3 8192 4 -b---- 2.0\n";
        assert!(parse_crashed_vms(healthy).is_empty());
        // Domain-0 itself in a crashed-flagged state is ignored (never the guest signal).
        let dom0_only = "Name ID Mem VCPUs State Time(s)\n\
                         Domain-0 0 4096 4 ---c-- 1.0\n";
        assert!(parse_crashed_vms(dom0_only).is_empty());
    }

    #[test]
    fn pool_status_from_enabled_flags_a_disabled_host() {
        // All enabled → ok.
        assert_eq!(pool_status_from_enabled("true,true,true"), ("ok", 3, 3));
        // One disabled → fail (degraded).
        assert_eq!(pool_status_from_enabled("true,false,true"), ("fail", 2, 3));
        // Case-insensitive + whitespace tolerant.
        assert_eq!(pool_status_from_enabled(" True , TRUE "), ("ok", 2, 2));
        // Empty/unparseable → warn (nothing to assert on).
        assert_eq!(pool_status_from_enabled(""), ("warn", 0, 0));
        assert_eq!(pool_status_from_enabled("   "), ("warn", 0, 0));
    }

    #[test]
    fn logs_topic_formats_under_event_dc_logs() {
        assert_eq!(logs_topic("dom0:172.20.0.9"), "event/dc/logs/dom0:172.20.0.9");
    }

    #[test]
    fn dc_logs_forwards_only_new_lines_and_dedups() {
        let mut l = DcLogs::new();
        // First sight → all (non-empty) lines forwarded.
        let batch1 = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(l.new_lines("dom0:x", &batch1), vec!["a", "b", "c"]);
        // Re-fetch the same tail → nothing new (cursor at "c").
        assert!(l.new_lines("dom0:x", &batch1).is_empty());
        // Tail grows by two → only the two new lines.
        let batch2 = vec![
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
            "e".to_string(),
        ];
        assert_eq!(l.new_lines("dom0:x", &batch2), vec!["d", "e"]);
        // A different resource has an independent cursor.
        assert_eq!(l.new_lines("dom0:y", &batch1), vec!["a", "b", "c"]);
    }

    #[test]
    fn dc_logs_rotation_forwards_the_whole_window() {
        let mut l = DcLogs::new();
        let _ = l.new_lines("dom0:x", &["a".to_string(), "b".to_string()]);
        // The previously-seen "b" rolled out of the window entirely → forward all.
        let rotated = vec!["m".to_string(), "n".to_string()];
        assert_eq!(l.new_lines("dom0:x", &rotated), vec!["m", "n"]);
    }

    #[test]
    fn dc_logs_ignores_blank_lines() {
        let mut l = DcLogs::new();
        let batch = vec!["".to_string(), "  ".to_string(), "real".to_string()];
        assert_eq!(l.new_lines("dom0:x", &batch), vec!["real"]);
    }

    #[test]
    fn probe_token_ok_when_no_ledger() {
        // A fresh root with no issued-bearer dir → ok, nothing pending.
        let tmp = tempfile::tempdir().unwrap();
        let p = probe_token(tmp.path());
        assert_eq!(p.status, "ok");
        assert!(p.detail.contains("no pending tokens"), "{}", p.detail);
    }

    #[test]
    fn probe_token_warns_on_a_stale_recorded_bearer() {
        // record_issued writes issued_at_ms:0 (epoch) → always older than the
        // window → warn, and counts the pending token.
        let tmp = tempfile::tempdir().unwrap();
        crate::bearer_ledger::record_issued(tmp.path(), "some-join-token").unwrap();
        let p = probe_token(tmp.path());
        assert_eq!(p.status, "warn");
        assert!(p.detail.contains("1 pending"), "{}", p.detail);
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
}
