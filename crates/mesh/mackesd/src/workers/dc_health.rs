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

// ---- DATACENTER-24: enrollment/bearer token-expiry helpers ----

/// Hours after issuance at which a pending enrollment/bearer token is flagged `warn`.
/// Enrollment tokens should be redeemed promptly; a 24-hour-old unredeemed token
/// indicates a node join that never completed and is worth an operator nudge.
pub const TOKEN_WARN_HOURS: i64 = 24;

/// Hours after issuance at which a pending token is flagged `fail`.
/// A 72-hour-old unredeemed token is almost certainly abandoned (or the node
/// that received it is unreachable) and should be investigated.
pub const TOKEN_FAIL_HOURS: i64 = 72;

/// Classify a pending enrollment/bearer token by its age (milliseconds since
/// `issued_at_ms`). Pure — no I/O, unit-tested without the ledger on disk.
///
/// Returns `"fail"` when the age exceeds [`TOKEN_FAIL_HOURS`], `"warn"` when it
/// exceeds [`TOKEN_WARN_HOURS`], and `"ok"` otherwise.
#[must_use]
pub fn classify_token_age_ms(age_ms: i64) -> &'static str {
    let hours = age_ms / 3_600_000;
    if hours >= TOKEN_FAIL_HOURS {
        "fail"
    } else if hours >= TOKEN_WARN_HOURS {
        "warn"
    } else {
        "ok"
    }
}

/// Parse the `issued_at_ms` field from a bearer-ledger entry JSON string.
/// Returns `None` when the JSON is missing, malformed, or the field is absent.
#[must_use]
pub fn parse_ledger_issued_at_ms(json: &str) -> Option<u64> {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()?
        .get("issued_at_ms")?
        .as_u64()
}

/// Probe all pending enrollment/bearer tokens in the bearer ledger for age-based
/// expiry. Reads every `.json` file under `<workgroup_root>/ca/issued-bearers/`,
/// parses `issued_at_ms`, and reduces them to the worst status across all entries.
///
/// - `ok`   — ledger is empty, or all tokens are fresh (< [`TOKEN_WARN_HOURS`]).
/// - `warn` — at least one token is ≥ [`TOKEN_WARN_HOURS`] old (unredeemed).
/// - `fail` — at least one token is ≥ [`TOKEN_FAIL_HOURS`] old (abandoned).
///
/// Uses the system clock for "now"; a `issued_at_ms = 0` sentinel (recorded by
/// `record_issued` during tests / migration seeding) is treated as age = 0 so it
/// never triggers a spurious alert.
fn probe_token_expiry(workgroup_root: &std::path::Path) -> Probe {
    let ledger_dir = crate::bearer_ledger::ledger_dir(workgroup_root);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);

    let entries = match std::fs::read_dir(&ledger_dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Ledger dir not yet created — no tokens issued, ok.
            return Probe::ok("no enrollment tokens issued");
        }
        Err(e) => {
            return Probe::warn(format!("token-expiry: ledger dir unreadable: {e}"));
        }
    };

    let mut worst: &'static str = "ok";
    let mut oldest_hours: i64 = 0;
    let mut count = 0usize;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let json = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let issued_at_ms = match parse_ledger_issued_at_ms(&json) {
            Some(v) => v,
            None => continue,
        };
        // issued_at_ms == 0 is the `record_issued` migration sentinel — skip.
        if issued_at_ms == 0 {
            continue;
        }
        count += 1;
        let age_ms = now_ms.saturating_sub(issued_at_ms) as i64;
        let hours = age_ms / 3_600_000;
        if hours > oldest_hours {
            oldest_hours = hours;
        }
        let status = classify_token_age_ms(age_ms);
        if status == "fail" || (status == "warn" && worst == "ok") {
            worst = status;
        }
    }

    if count == 0 {
        return Probe::ok("no pending enrollment tokens");
    }
    match worst {
        "fail" => Probe::fail(format!(
            "{count} pending token(s); oldest ~{oldest_hours}h — investigate abandoned join"
        )),
        "warn" => Probe::warn(format!(
            "{count} pending token(s); oldest ~{oldest_hours}h — join not yet completed"
        )),
        _ => Probe::ok(format!("{count} pending token(s), all fresh")),
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

/// One health pass: run each best-effort probe, feed its `(check, status)`
/// through the dedup core, and publish the records that survive (status
/// transitions). Every probe is independent — a failed/absent tool degrades that
/// one check to `fail`/`warn` and never aborts the pass.
fn run_checks(core: &mut DcHealth, workgroup_root: &std::path::Path) {
    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    for dom0 in crate::workers::datacenter_orchestrator::xen_dom0s() {
        let check = format!("dom0:{dom0}");
        let p = probe_dom0(&key, &dom0);
        if let Some(rec) = core.observe(&check, p.status, &p.detail) {
            publish(&rec);
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

    // DATACENTER-24: enrollment/bearer token-expiry check.
    // Warn when pending join tokens are stale (unredeemed) to catch abandoned
    // joins before they become a security concern. Uses the same event/dc/health/*
    // shape and bearer_ledger plumbing as the rest of the dc_health checks.
    let p = probe_token_expiry(workgroup_root);
    if let Some(rec) = core.observe("token-expiry", p.status, &p.detail) {
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

    // ---- DATACENTER-24: token-expiry classification (pure fn) ----

    #[test]
    fn classify_token_age_fresh_is_ok() {
        // 0 ms and any age under TOKEN_WARN_HOURS → ok.
        assert_eq!(classify_token_age_ms(0), "ok");
        // Just under the warn threshold (1 ms under 24 h).
        let just_under = (TOKEN_WARN_HOURS * 3_600_000) - 1;
        assert_eq!(classify_token_age_ms(just_under), "ok");
    }

    #[test]
    fn classify_token_age_stale_is_warn() {
        // Exactly 24 h → warn.
        assert_eq!(classify_token_age_ms(TOKEN_WARN_HOURS * 3_600_000), "warn");
        // 25 h → still warn (below the fail threshold).
        assert_eq!(classify_token_age_ms(25 * 3_600_000), "warn");
        // One ms under the fail threshold → warn.
        let just_under_fail = (TOKEN_FAIL_HOURS * 3_600_000) - 1;
        assert_eq!(classify_token_age_ms(just_under_fail), "warn");
    }

    #[test]
    fn classify_token_age_abandoned_is_fail() {
        // Exactly TOKEN_FAIL_HOURS → fail.
        assert_eq!(classify_token_age_ms(TOKEN_FAIL_HOURS * 3_600_000), "fail");
        // Well past → still fail.
        assert_eq!(classify_token_age_ms(7 * 24 * 3_600_000), "fail");
    }

    #[test]
    fn parse_ledger_issued_at_ms_extracts_field() {
        // Happy path: well-formed ledger entry.
        let json = r#"{"issued_at_ms":1_000_000,"note":"test"}"#;
        // serde_json does not support _ separators in integer literals —
        // use the un-separated form.
        let json = r#"{"issued_at_ms":1000000,"note":"test"}"#;
        assert_eq!(parse_ledger_issued_at_ms(json), Some(1_000_000));
        // Zero sentinel (record_issued migration path).
        let json_zero = r#"{"issued_at_ms":0,"note":"recorded"}"#;
        assert_eq!(parse_ledger_issued_at_ms(json_zero), Some(0));
        // Malformed JSON → None.
        assert!(parse_ledger_issued_at_ms("{not json}").is_none());
        // Missing field → None.
        assert!(parse_ledger_issued_at_ms(r#"{"note":"x"}"#).is_none());
    }

    #[test]
    fn probe_token_expiry_empty_ledger_is_ok() {
        // No ledger dir → ok ("no enrollment tokens issued").
        let tmp = tempfile::tempdir().unwrap();
        let p = probe_token_expiry(tmp.path());
        assert_eq!(p.status, "ok");
        assert!(p.detail.contains("no enrollment"), "got: {}", p.detail);
    }

    #[test]
    fn probe_token_expiry_fresh_token_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = tmp.path().join("ca").join("issued-bearers");
        std::fs::create_dir_all(&ledger).unwrap();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        // Write a fresh token (issued right now).
        let body = format!(r#"{{"issued_at_ms":{now_ms},"note":"test"}}"#);
        std::fs::write(ledger.join("aabbcc.json"), body).unwrap();
        let p = probe_token_expiry(tmp.path());
        assert_eq!(p.status, "ok", "fresh token must be ok, got: {}", p.detail);
    }

    #[test]
    fn probe_token_expiry_old_token_warns() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = tmp.path().join("ca").join("issued-bearers");
        std::fs::create_dir_all(&ledger).unwrap();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        // Token issued 30 hours ago (past warn, before fail).
        let old_ms = now_ms - 30 * 3_600_000;
        let body = format!(r#"{{"issued_at_ms":{old_ms},"note":"old"}}"#);
        std::fs::write(ledger.join("aabbdd.json"), body).unwrap();
        let p = probe_token_expiry(tmp.path());
        assert_eq!(p.status, "warn", "30h token must warn, got: {}", p.detail);
    }

    #[test]
    fn probe_token_expiry_very_old_token_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = tmp.path().join("ca").join("issued-bearers");
        std::fs::create_dir_all(&ledger).unwrap();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        // Token issued 96 hours ago (past fail threshold).
        let old_ms = now_ms - 96 * 3_600_000;
        let body = format!(r#"{{"issued_at_ms":{old_ms},"note":"abandoned"}}"#);
        std::fs::write(ledger.join("aabbee.json"), body).unwrap();
        let p = probe_token_expiry(tmp.path());
        assert_eq!(p.status, "fail", "96h token must fail, got: {}", p.detail);
    }

    #[test]
    fn probe_token_expiry_sentinel_zero_is_skipped() {
        // issued_at_ms == 0 is the `record_issued` migration sentinel and must
        // never trigger a health alert.
        let tmp = tempfile::tempdir().unwrap();
        let ledger = tmp.path().join("ca").join("issued-bearers");
        std::fs::create_dir_all(&ledger).unwrap();
        let body = r#"{"issued_at_ms":0,"note":"recorded"}"#;
        std::fs::write(ledger.join("sentinel.json"), body).unwrap();
        let p = probe_token_expiry(tmp.path());
        assert_eq!(
            p.status, "ok",
            "sentinel (issued_at_ms=0) must never alert, got: {}",
            p.detail
        );
    }

    #[test]
    fn probe_token_expiry_worst_across_multiple_tokens() {
        // When one token is fresh and another is ancient, fail wins.
        let tmp = tempfile::tempdir().unwrap();
        let ledger = tmp.path().join("ca").join("issued-bearers");
        std::fs::create_dir_all(&ledger).unwrap();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        // Fresh token.
        let fresh = format!(r#"{{"issued_at_ms":{now_ms},"note":"fresh"}}"#);
        std::fs::write(ledger.join("fresh.json"), fresh).unwrap();
        // Ancient token (100h old).
        let old_ms = now_ms - 100 * 3_600_000;
        let old = format!(r#"{{"issued_at_ms":{old_ms},"note":"old"}}"#);
        std::fs::write(ledger.join("old.json"), old).unwrap();
        let p = probe_token_expiry(tmp.path());
        assert_eq!(
            p.status, "fail",
            "worst status across tokens must be fail, got: {}",
            p.detail
        );
    }

    #[test]
    fn token_expiry_health_topic_is_under_event_dc_health() {
        // Verify the probe integrates into the existing event/dc/health/* namespace.
        assert_eq!(
            health_topic("token-expiry"),
            "event/dc/health/token-expiry"
        );
    }
}
