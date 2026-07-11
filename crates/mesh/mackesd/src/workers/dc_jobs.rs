//! DATACENTER-6 — the passive `dc_jobs` async job-status tracker.
//!
//! A read-only companion to [`super::dc_auditor`]: where the auditor emits one
//! append-only record on first sight of a datacenter action request, this worker
//! tracks each action RPC's **lifecycle** — pending → ok/error — and publishes a
//! per-job status event to `event/dc/job/<ulid>`, WITHOUT touching the action
//! handlers themselves. Nothing the handlers do depends on it, so it can never
//! wedge an action; it is a pure side-observer of the request/reply lanes.
//!
//! Design (mirrors `dc_auditor` + `datacenter_orchestrator`): the *brain*
//! ([`DcJobs`]) is a pure, deduped state machine — fed `(ulid, action, reply)` it
//! returns a [`JobRecord`] ONLY on a status transition (first sight, or a change
//! pending→ok/error), so a re-poll of the same lane never re-publishes an
//! unchanged status. The worker is thin I/O around it: list topics, walk each
//! `action/dc/` lane, look up the matching `reply/<ulid>`, feed the pair through
//! the sieve, publish what survives. It is **leader-gated** so a multi-node mesh
//! writes each job-status transition once.

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;

use super::{ShutdownToken, Worker};

/// Sweep cadence — 3 s (job status should trail the request/reply closely).
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(3);

/// The Bus prefix the tracker watches — every datacenter action lane.
pub const ACTION_DC_PREFIX: &str = "action/dc/";

/// Bus topic a job-status event for `ulid` is published to: `event/dc/job/<ulid>`.
#[must_use]
pub fn job_topic(ulid: &str) -> String {
    format!("event/dc/job/{ulid}")
}

/// Classify an RPC reply body into a job status:
/// * `None` (no reply yet) → `"pending"`,
/// * a reply whose JSON body has `"ok": true` → `"ok"`,
/// * any other reply (no `ok`, `ok:false`, or unparseable) → `"error"`.
#[must_use]
pub fn classify_reply(reply_body: Option<&str>) -> &'static str {
    let Some(body) = reply_body else {
        return "pending";
    };
    let ok = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("ok").and_then(serde_json::Value::as_bool))
        .unwrap_or(false);
    if ok {
        "ok"
    } else {
        "error"
    }
}

/// The audited action name for a Bus topic: strips the leading `action/` so
/// `action/dc/vm-power` → `dc/vm-power`. Topics without the prefix pass through
/// unchanged.
#[must_use]
fn job_action_name(topic: &str) -> String {
    topic.strip_prefix("action/").unwrap_or(topic).to_string()
}

/// One job-status event the tracker decided to emit (a request's status changed —
/// first sight, or a pending→ok/error transition).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct JobRecord {
    /// The action name (`dc/vm-power`, …) — the source topic minus `action/`.
    pub action: String,
    /// The request message's ULID (also the job topic's leaf).
    pub ulid: String,
    /// The job status: `"pending"`, `"ok"`, or `"error"`.
    pub status: &'static str,
}

impl JobRecord {
    /// Bus topic this record publishes to: `event/dc/job/<ulid>`.
    #[must_use]
    pub fn topic(&self) -> String {
        job_topic(&self.ulid)
    }

    /// JSON body for `mde-bus publish`.
    #[must_use]
    pub fn body(&self) -> String {
        serde_json::json!({
            "action": self.action,
            "ulid": self.ulid,
            "status": self.status,
        })
        .to_string()
    }
}

/// Pure job-status core: tracks the last-published status per request ULID and
/// returns a record ONLY on a status transition (first sight, or a change such as
/// pending→ok). A re-poll that observes the same status emits nothing, so the Bus
/// never sees a duplicate for an unchanged job.
#[derive(Default)]
pub struct DcJobs {
    last_status: BTreeMap<String, &'static str>,
}

impl DcJobs {
    /// Fresh tracker with no observed jobs.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe one request `ulid` on `action` together with its current reply
    /// body (`None` ⇒ no reply yet). Returns a [`JobRecord`] when the classified
    /// status differs from the last one published for this ulid (or on first
    /// sight), and `None` when the status is unchanged. Advances internal state on
    /// a transition.
    pub fn observe(
        &mut self,
        ulid: &str,
        action: &str,
        reply_opt: Option<&str>,
    ) -> Option<JobRecord> {
        let status = classify_reply(reply_opt);
        if self.last_status.get(ulid) == Some(&status) {
            return None;
        }
        self.last_status.insert(ulid.to_string(), status);
        Some(JobRecord {
            action: action.to_string(),
            ulid: ulid.to_string(),
            status,
        })
    }
}

// ---- thin I/O: watch the action lanes, emit job-status events via the Bus ----

/// Publish one job-status record onto the Bus in-process (perf-10 / arch-6) — no
/// fork+exec of the `mde-bus` CLI per record. Byte-identical stored row to the
/// old `mde-bus publish <topic> --body-flag <body>`.
///
/// Targets [`crate::bus_publish::default_bus_root`] (which honours
/// `MDE_BUS_ROOT`) — the SAME root the fork+exec'd CLI resolved, NOT the
/// worker's own MDE_BUS_ROOT-blind [`default_bus_root`] read root (they diverge
/// on the live daemon; see [`crate::workers::dc_auditor`]).
fn publish(rec: &JobRecord) {
    publish_to(crate::bus_publish::default_bus_root().as_deref(), rec);
}

/// Root-injectable body of [`publish`] — fresh-opens the Bus at `bus_root` and
/// writes the record in-process (mirrors the CLI's per-call open). Best-effort;
/// tests pass a temp root.
fn publish_to(bus_root: Option<&std::path::Path>, rec: &JobRecord) {
    if let Some(mut persist) =
        crate::bus_publish::open_bus(bus_root.map(std::path::Path::to_path_buf))
    {
        crate::bus_publish::publish_body(&mut persist, &rec.topic(), &rec.body());
    }
}

/// Read the current reply body for a request ulid, if any. The reply lane
/// (`reply/<ulid>`) carries at most the single RPC reply; we take the last
/// message's body. Best-effort: a failed read is treated as "no reply yet".
fn reply_body(persist: &Persist, ulid: &str) -> Option<String> {
    let topic = reply_topic(ulid);
    let msgs = persist.list_since(&topic, None).ok()?;
    msgs.into_iter().last().and_then(|m| m.body)
}

/// One poll pass: enumerate every `action/dc/*` topic, and for each request
/// message look up its `reply/<ulid>` and feed the pair through the dedup core,
/// publishing the records that survive (status transitions). Best-effort: a
/// failed `list_topics`/`list_since` is logged + skipped.
fn poll_and_track(persist: &Persist, core: &mut DcJobs) {
    let topics = match persist.list_topics() {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!(error = %e, "dc_jobs: list_topics failed");
            return;
        }
    };
    for topic in topics.iter().filter(|t| t.starts_with(ACTION_DC_PREFIX)) {
        let msgs = match persist.list_since(topic, None) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "dc_jobs: list_since failed");
                continue;
            }
        };
        let action = job_action_name(topic);
        for msg in msgs {
            let reply = reply_body(persist, &msg.ulid);
            if let Some(rec) = core.observe(&msg.ulid, &action, reply.as_deref()) {
                publish(&rec);
            }
        }
    }
}

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

/// The supervised worker. Leader-gated (only the elected node writes the
/// job-status lane, so a multi-node mesh doesn't multi-publish) and best-effort.
pub struct DcJobsWorker {
    core: DcJobs,
    tick_interval: Duration,
    node_id: String,
    leader_lock: PathBuf,
    bus_root_override: Option<PathBuf>,
}

impl DcJobsWorker {
    /// Construct with production defaults (3 s tick, the shared leader lock under
    /// `workgroup_root`, the default Bus root).
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        Self {
            core: DcJobs::new(),
            tick_interval: DEFAULT_TICK_INTERVAL,
            leader_lock: workgroup_root.join(".mackesd-leader.lock"),
            node_id,
            bus_root_override: None,
        }
    }

    /// Override the Bus root directory. Used in tests.
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    /// Only the directory leader tracks (no-fixed-center: any eligible node can be
    /// it, the elected one publishes). Reuses the shared leader lock.
    fn is_leader(&self) -> bool {
        crate::leader_gate::LeaderGate::from_lock_path(
            self.leader_lock.clone(),
            self.node_id.clone(),
        )
        .is_leader()
    }
}

#[async_trait::async_trait]
impl Worker for DcJobsWorker {
    fn name(&self) -> &'static str {
        "dc_jobs"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let bus_root = match self.bus_root_override.clone().or_else(default_bus_root) {
            Some(r) => r,
            None => {
                tracing::debug!("dc_jobs: no bus root; worker idle");
                return Ok(());
            }
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(error = %e, "dc_jobs: persist open failed; worker idle");
                return Ok(());
            }
        };
        loop {
            if self.is_leader() {
                poll_and_track(&persist, &mut self.core);
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

    /// perf-10 / arch-6 — `publish_to` writes the job-status record in-process
    /// (no fork+exec of `mde-bus`) with EXACTLY the row a
    /// `mde-bus publish event/dc/job/<ulid> --body-flag <body>` produced.
    #[test]
    fn publish_to_writes_cli_equivalent_row_in_process() {
        let tmp = tempfile::tempdir().unwrap();
        let rec = JobRecord {
            action: "dc/vm-power".to_string(),
            ulid: "ulid-9".to_string(),
            status: "ok",
        };

        publish_to(Some(tmp.path()), &rec);

        let reader = Persist::open(tmp.path().to_path_buf()).unwrap();
        let topic = job_topic(&rec.ulid);
        let rows = reader.list_since(&topic, None).unwrap();
        assert_eq!(rows.len(), 1, "exactly one job-status record published");
        let row = &rows[0];
        assert_eq!(row.topic, topic);
        assert_eq!(row.priority, "default");
        assert!(row.title.is_none());
        assert!(row.actions.is_empty());
        assert!(row.reply_to.is_none());
        // Byte-identical to the record's `body()` — what `--body-flag` carried.
        assert_eq!(row.body.as_deref(), Some(rec.body().as_str()));
    }

    #[test]
    fn job_topic_formats_under_event_dc_job() {
        assert_eq!(job_topic("01HZX5"), "event/dc/job/01HZX5");
    }

    #[test]
    fn classify_reply_maps_pending_ok_error() {
        // No reply yet → pending.
        assert_eq!(classify_reply(None), "pending");
        // A reply with "ok":true → ok.
        assert_eq!(classify_reply(Some(r#"{"ok":true}"#)), "ok");
        assert_eq!(
            classify_reply(Some(r#"{"ok":true,"detail":"powered on"}"#)),
            "ok"
        );
        // "ok":false → error.
        assert_eq!(classify_reply(Some(r#"{"ok":false}"#)), "error");
        // A reply with no "ok" field → error.
        assert_eq!(classify_reply(Some(r#"{"detail":"boom"}"#)), "error");
        // Unparseable body → error.
        assert_eq!(classify_reply(Some("not json")), "error");
    }

    #[test]
    fn observe_emits_on_transition_and_dedups_same_status() {
        let mut j = DcJobs::new();
        // First sight with no reply → a pending record on the right topic.
        let rec = j
            .observe("ulid-1", "dc/vm-power", None)
            .expect("first sight emits");
        assert_eq!(rec.action, "dc/vm-power");
        assert_eq!(rec.ulid, "ulid-1");
        assert_eq!(rec.status, "pending");
        assert_eq!(rec.topic(), "event/dc/job/ulid-1");
        let body = rec.body();
        assert!(body.contains(r#""action":"dc/vm-power""#));
        assert!(body.contains(r#""ulid":"ulid-1""#));
        assert!(body.contains(r#""status":"pending""#));
        // Same status (still pending) → no re-emit.
        assert!(j.observe("ulid-1", "dc/vm-power", None).is_none());
        // Reply lands ok → a second record (pending→ok emits twice overall).
        let rec2 = j
            .observe("ulid-1", "dc/vm-power", Some(r#"{"ok":true}"#))
            .expect("status transition emits");
        assert_eq!(rec2.status, "ok");
        // Re-poll of the same ok reply → no re-emit.
        assert!(j
            .observe("ulid-1", "dc/vm-power", Some(r#"{"ok":true}"#))
            .is_none());
    }

    #[test]
    fn observe_tracks_status_per_ulid_independently() {
        let mut j = DcJobs::new();
        // Two distinct jobs each get their own first-sight pending record.
        assert!(j.observe("u1", "dc/droplet-create", None).is_some());
        assert!(j.observe("u2", "dc/vm-power", None).is_some());
        // u1 fails, u2 succeeds — each is one independent transition.
        let r1 = j
            .observe("u1", "dc/droplet-create", Some(r#"{"ok":false}"#))
            .expect("u1 → error");
        assert_eq!(r1.status, "error");
        let r2 = j
            .observe("u2", "dc/vm-power", Some(r#"{"ok":true}"#))
            .expect("u2 → ok");
        assert_eq!(r2.status, "ok");
        // Neither re-emits on the next identical poll.
        assert!(j
            .observe("u1", "dc/droplet-create", Some(r#"{"ok":false}"#))
            .is_none());
        assert!(j
            .observe("u2", "dc/vm-power", Some(r#"{"ok":true}"#))
            .is_none());
    }
}
