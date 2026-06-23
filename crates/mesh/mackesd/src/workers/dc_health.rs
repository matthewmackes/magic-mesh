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

/// One health pass: run each best-effort probe, feed its `(check, status)`
/// through the dedup core, and publish the records that survive (status
/// transitions). Every probe is independent — a failed/absent tool degrades that
/// one check to `fail`/`warn` and never aborts the pass.
fn run_checks(core: &mut DcHealth) {
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
}

/// The supervised worker. Leader-gated (only the elected node probes + publishes,
/// so a multi-node mesh doesn't multi-publish) and best-effort.
pub struct DcHealthWorker {
    core: DcHealth,
    tick_interval: Duration,
    node_id: String,
    leader_lock: PathBuf,
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
                run_checks(&mut self.core);
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
