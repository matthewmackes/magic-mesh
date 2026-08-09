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

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;

use super::{ShutdownToken, Worker};

/// Sweep cadence — 3 s (job status should trail the request/reply closely).
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(3);

const MIN_BUS_RETRY_INTERVAL: Duration = Duration::from_millis(10);
const MAX_BUS_RETRY_INTERVAL: Duration = Duration::from_secs(2);

/// The Bus prefix containing registered datacenter action lanes.
pub const ACTION_DC_PREFIX: &str = "action/dc/";

/// Admit only topics that still have a production responder. Retained rows for
/// retired VM verbs must not become fresh job projections after an upgrade.
#[must_use]
fn is_registered_action_topic(topic: &str) -> bool {
    let Some(verb) = topic.strip_prefix(ACTION_DC_PREFIX) else {
        return false;
    };
    crate::ipc::datacenter::ACTION_VERBS.contains(&verb)
        || crate::ipc::dc_power::ACTION_VERBS.contains(&verb)
        || crate::ipc::host_ops::ACTION_VERBS.contains(&verb)
        || crate::ipc::tofu::ACTION_VERBS.contains(&verb)
}

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
/// `action/dc/host-power` → `dc/host-power`. Topics without the prefix pass through
/// unchanged.
#[must_use]
fn job_action_name(topic: &str) -> String {
    topic.strip_prefix("action/").unwrap_or(topic).to_string()
}

/// One job-status event the tracker decided to emit (a request's status changed —
/// first sight, or a pending→ok/error transition).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct JobRecord {
    /// The action name (`dc/host-power`, …) — the source topic minus `action/`.
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

/// Read the current reply body for a request ulid, if any. The reply lane
/// (`reply/<ulid>`) carries at most the single RPC reply; we take the last
/// message's body. Best-effort: a failed read is treated as "no reply yet".
fn reply_body(persist: &Persist, ulid: &str) -> Result<Option<String>, String> {
    let topic = reply_topic(ulid);
    let msgs = persist
        .list_since(&topic, None)
        .map_err(|error| format!("read {topic}: {error}"))?;
    Ok(msgs.into_iter().last().and_then(|message| message.body))
}

/// One poll pass: enumerate registered `action/dc/*` topics, and for each request
/// message look up its `reply/<ulid>` and feed the pair through the dedup core,
/// publishing the records that survive (status transitions). Best-effort: a
/// failed `list_topics`/`list_since` is logged + skipped.
fn poll_and_track(persist: &Persist, core: &mut DcJobs) -> Result<(), String> {
    let topics = persist
        .list_topics()
        .map_err(|error| format!("list datacenter action topics: {error}"))?;
    let mut observations = Vec::new();
    for topic in topics.iter().filter(|t| is_registered_action_topic(t)) {
        let msgs = persist
            .list_since(topic, None)
            .map_err(|error| format!("read {topic}: {error}"))?;
        let action = job_action_name(topic);
        for msg in msgs {
            let reply = reply_body(persist, &msg.ulid)?;
            observations.push((msg.ulid, action.clone(), reply));
        }
    }

    // Every request and reply lane has been read successfully. Only now may a
    // status transition be published or remembered; an unreadable reply can
    // never masquerade as a fresh `pending` regression.
    for (ulid, action, reply) in observations {
        let status = classify_reply(reply.as_deref());
        if core.last_status.get(&ulid) == Some(&status) {
            continue;
        }
        let record = JobRecord {
            action,
            ulid: ulid.clone(),
            status,
        };
        persist
            .write(
                &record.topic(),
                Priority::Default,
                None,
                Some(&record.body()),
            )
            .map_err(|error| format!("publish {}: {error}", record.topic()))?;
        core.last_status.insert(ulid, status);
    }
    Ok(())
}

fn default_bus_root() -> Option<PathBuf> {
    mde_bus::default_data_dir()
}

fn dc_jobs_bus_root(override_root: Option<PathBuf>) -> PathBuf {
    override_root
        .or_else(default_bus_root)
        .unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
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
        let bus_root = dc_jobs_bus_root(self.bus_root_override.clone());
        let mut retry_interval = MIN_BUS_RETRY_INTERVAL;
        let persist = loop {
            match Persist::open(bus_root.clone()) {
                Ok(persist) => break persist,
                Err(error) => tracing::warn!(
                    %error,
                    "dc_jobs: Persist open failed; startup will retry"
                ),
            }
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(retry_interval) => {}
            }
            retry_interval = retry_interval
                .saturating_mul(2)
                .clamp(MIN_BUS_RETRY_INTERVAL, MAX_BUS_RETRY_INTERVAL);
        };
        loop {
            if self.is_leader() {
                if let Err(error) = poll_and_track(&persist, &mut self.core) {
                    tracing::warn!(
                        %error,
                        "dc_jobs: incomplete Bus sweep deferred all new status transitions"
                    );
                }
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
    fn service_bus_root_falls_back_to_the_shared_system_spool() {
        assert_eq!(
            dc_jobs_bus_root(None),
            default_bus_root().unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
        );
        assert_eq!(
            dc_jobs_bus_root(Some(PathBuf::from("/tmp/dc-jobs-explicit-bus"))),
            PathBuf::from("/tmp/dc-jobs-explicit-bus")
        );
    }

    #[tokio::test]
    async fn late_bus_is_opened_by_the_same_worker() {
        let temp = tempfile::tempdir().unwrap();
        let bus_root = temp.path().join("late-bus");
        std::fs::write(&bus_root, b"not a directory").unwrap();
        let mut worker = DcJobsWorker::new(temp.path().to_path_buf(), "nodeA".into())
            .with_bus_root(bus_root.clone());
        worker.tick_interval = Duration::from_millis(5);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );

        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!task.is_finished(), "late Bus is retryable");
        std::fs::remove_file(&bus_root).unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while !bus_root.is_dir() {
                assert!(!task.is_finished(), "worker exited before opening late Bus");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("same worker must open the recovered Bus path");

        shutdown_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("shutdown completes")
            .expect("worker joins")
            .expect("worker exits cleanly");
    }

    #[test]
    fn retained_job_history_folds_pending_then_terminal_reply() {
        let temp = tempfile::tempdir().unwrap();
        let persist = Persist::open(temp.path().to_path_buf()).unwrap();
        let request = persist
            .write(
                "action/dc/host-power",
                Priority::Default,
                None,
                Some(r#"{"host":"nodeA","action":"reboot"}"#),
            )
            .unwrap();
        let mut core = DcJobs::new();

        poll_and_track(&persist, &mut core).unwrap();
        let topic = job_topic(&request.ulid);
        let pending = persist.list_since(&topic, None).unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0]
            .body
            .as_deref()
            .unwrap()
            .contains(r#""status":"pending""#));

        persist
            .write(
                &reply_topic(&request.ulid),
                Priority::Default,
                None,
                Some(r#"{"ok":true}"#),
            )
            .unwrap();
        poll_and_track(&persist, &mut core).unwrap();
        let terminal = persist.list_since(&topic, None).unwrap();
        assert_eq!(terminal.len(), 2);
        assert!(terminal[1]
            .body
            .as_deref()
            .unwrap()
            .contains(r#""status":"ok""#));
    }

    #[test]
    fn job_topic_formats_under_event_dc_job() {
        assert_eq!(job_topic("01HZX5"), "event/dc/job/01HZX5");
    }

    #[test]
    fn retained_vm_topics_are_not_projected_as_jobs() {
        assert!(is_registered_action_topic("action/dc/host-power"));
        assert!(!is_registered_action_topic("action/dc/vm-power"));
        assert!(!is_registered_action_topic("action/dc/vm-create"));
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
