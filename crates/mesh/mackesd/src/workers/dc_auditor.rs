//! DATACENTER-7 (audit half) — the passive `dc_auditor` worker.
//!
//! The companion to [`super::datacenter_orchestrator`]: where the orchestrator
//! *publishes* datacenter state, this worker is a **read-only audit subscriber**.
//! It watches the registered Bus action lanes (`action/dc/*` — host power,
//! storage, gateway changes, …) and emits one append-only audit record per request to
//! `event/dc/audit/<ulid>`, WITHOUT touching the action handlers themselves. The
//! audit trail is therefore a pure side-observer: nothing the handlers do depends
//! on it, and it can never wedge an action.
//!
//! Design (mirrors `datacenter_orchestrator` + `compute_event_toast`): the *brain*
//! ([`DcAuditor`]) is a pure, deduped sieve — fed `(topic, ulid, body)` it returns
//! an [`AuditRecord`] only the first time it sees a given ulid, so a re-poll never
//! double-audits. The worker is thin I/O around it: list topics, walk each
//! `action/dc/` lane, feed every message through the sieve, publish what survives.
//! It is **leader-gated** so a multi-node mesh writes each audit record once.

#![cfg(feature = "async-services")]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(test)]
use std::sync::Arc;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::{Persist, StoredMessage};

use super::{ShutdownToken, Worker};

/// Sweep cadence — 5 s (audit records should trail actions closely without
/// hammering the Bus index).
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(5);

/// Lower bound for retrying an unresolved or unopenable Bus without spinning.
const MIN_BUS_RETRY_INTERVAL: Duration = Duration::from_millis(10);

/// Upper bound for startup retry backoff. The same passive projection worker
/// must recover when the canonical Bus appears.
const MAX_BUS_RETRY_INTERVAL: Duration = Duration::from_secs(2);

#[cfg(test)]
type BusOpenFn = dyn Fn(&Path) -> Result<Option<Persist>, String> + Send + Sync;

/// The Bus prefix containing registered datacenter action lanes.
pub const ACTION_DC_PREFIX: &str = "action/dc/";

/// The Bus prefix containing this projection's durable output lanes.
const AUDIT_DC_PREFIX: &str = "event/dc/audit/";

/// Admit only topics that still have a production responder. Retained rows for
/// retired VM verbs must not become fresh audit projections after an upgrade.
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

/// Max characters of the request body carried into the audit record's
/// `body_summary`. Keeps the audit lane compact; the full body stays on the
/// original action message.
pub const BODY_SUMMARY_LEN: usize = 120;

/// Bus topic an audit record for `ulid` is published to: `event/dc/audit/<ulid>`.
#[must_use]
pub fn audit_topic(ulid: &str) -> String {
    format!("{AUDIT_DC_PREFIX}{ulid}")
}

/// Recover a request ULID only from the exact stable audit-topic shape emitted
/// by this worker. Bus-generated ULIDs are 26-character uppercase Crockford
/// base32 values whose leading digit cannot exceed seven.
fn projected_ulid_from_audit_topic(topic: &str) -> Option<&str> {
    let ulid = topic.strip_prefix(AUDIT_DC_PREFIX)?;
    let bytes = ulid.as_bytes();
    if bytes.len() != 26 || !matches!(bytes[0], b'0'..=b'7') {
        return None;
    }
    bytes
        .iter()
        .all(|byte| matches!(byte, b'0'..=b'9' | b'A'..=b'H' | b'J' | b'K' | b'M' | b'N' | b'P'..=b'T' | b'V'..=b'Z'))
        .then_some(ulid)
}

/// The audited action name for a Bus topic: strips the leading `action/` so
/// `action/dc/host-power` → `dc/host-power`. Topics without the prefix pass through
/// unchanged.
#[must_use]
pub fn audit_action_name(topic: &str) -> String {
    topic.strip_prefix("action/").unwrap_or(topic).to_string()
}

/// First [`BODY_SUMMARY_LEN`] characters of a request body (char-boundary safe).
#[must_use]
fn body_summary(body: &str) -> String {
    body.chars().take(BODY_SUMMARY_LEN).collect()
}

/// Request-body fields the auditor inspects, in priority order, to name the
/// *target* of an action (the resource the verb acts on). The first present,
/// non-empty string field wins. Covers the VM verbs (`uuid`), the storage verbs
/// (`vbd`/`vdi`/`sr`/`snapshot`/`name`), the host verbs (`dom0`/`host`), the
/// gateway verbs (`host`), and the lighthouse verbs (`node`/`overlay_ip`). A body
/// naming none of these yields an empty target (recorded honestly as such).
const TARGET_FIELDS: [&str; 9] = [
    "uuid",
    "vbd",
    "vdi",
    "snapshot",
    "sr",
    "node",
    "overlay_ip",
    "dom0",
    "host",
];

/// Action result the auditor can honestly record. The auditor is a passive
/// request-lane observer — it sees the action being *issued* on `action/dc/*`
/// but does not (in this single-pass design) correlate the reply, so it never
/// fabricates an `ok`/`fail` it cannot observe. It records [`Issued`] instead.
///
/// [`Issued`]: ActionResult::Issued
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ActionResult {
    /// The action was observed being requested on the Bus; its ok/fail outcome
    /// is not correlated by this passive auditor. Serialized as `"issued"`.
    Issued,
}

impl ActionResult {
    /// The on-the-wire string for the audit record's `result` field.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ActionResult::Issued => "issued",
        }
    }
}

/// Extract the action target (the resource a verb acts on) from a request body,
/// trying [`TARGET_FIELDS`] in priority order. Returns the first present,
/// non-empty string field, or an empty string when the body names none (or is not
/// JSON). PURE.
#[must_use]
fn target_of(body: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return String::new();
    };
    for field in TARGET_FIELDS {
        if let Some(s) = v.get(field).and_then(serde_json::Value::as_str) {
            if !s.is_empty() {
                return s.to_string();
            }
        }
    }
    String::new()
}

/// One append-only audit record the auditor decided to emit (one datacenter
/// action observed for the first time).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AuditRecord {
    /// The audited action name (`dc/host-power`, …) — the source topic minus
    /// the `action/` prefix.
    pub action: String,
    /// The request message's ULID (also the audit topic's leaf).
    pub ulid: String,
    /// The initiating principal. The mde-bus message envelope carries NO sender
    /// identity (`StoredMessage` has `ulid`/`topic`/`priority`/`title`/`body`/
    /// `ts_unix_ms`/`file_path`/`actions`/`reply_to` — none of them a peer/cert
    /// name), so this is the LOCAL node identity (the mesh cert name / node id
    /// this mackesd runs as, form `peer:<host>`) — never an invented value.
    pub actor: String,
    /// The target resource the action acted on, extracted from the request body
    /// ([`target_of`]). Empty when the body named no recognized target field.
    pub target: String,
    /// The action result. The passive auditor observes the request, not the
    /// reply, so it records [`ActionResult::Issued`] honestly rather than a
    /// fabricated ok/fail.
    pub result: ActionResult,
    /// The action's timestamp — the request message's write-time, formatted as a
    /// zero-padded epoch-millis string so a lexical sort is also a time sort (the
    /// panel's `project_audit` sorts on this).
    pub ts: String,
    /// The first [`BODY_SUMMARY_LEN`] chars of the request body.
    pub body_summary: String,
}

impl AuditRecord {
    /// Bus topic this record publishes to: `event/dc/audit/<ulid>`.
    #[must_use]
    pub fn topic(&self) -> String {
        audit_topic(&self.ulid)
    }

    /// JSON body for `mde-bus publish`.
    #[must_use]
    pub fn body(&self) -> String {
        serde_json::json!({
            "action": self.action,
            "ulid": self.ulid,
            "actor": self.actor,
            "target": self.target,
            "result": self.result.as_str(),
            "ts": self.ts,
            "body_summary": self.body_summary,
        })
        .to_string()
    }
}

/// Format a message's write-time (`ts_unix_ms` from the Bus envelope) as a
/// zero-padded 13-digit epoch-millis string. Zero-padding keeps a lexical sort a
/// time sort (the panel's `project_audit` sorts on this); 13 digits covers
/// epoch-ms through year 2286. A non-positive timestamp renders as all-zeros.
#[must_use]
fn format_ts(ts_unix_ms: i64) -> String {
    let ms = ts_unix_ms.max(0);
    format!("{ms:013}")
}

/// Pure audit core: tracks which request ULIDs have already been audited and
/// returns a record ONLY on first sight of a ulid — so a re-poll of the same
/// Bus lane never emits a duplicate audit record. Carries the LOCAL node identity
/// (the `actor`) it stamps onto every record, since the Bus envelope has no
/// sender.
#[derive(Default)]
pub struct DcAuditor {
    seen: BTreeSet<String>,
    /// The mesh identity this mackesd runs as (`peer:<host>`), stamped as the
    /// `actor` on every audit record. The Bus carries no per-message sender, so
    /// the auditor records WHO it knows for certain: the local node.
    actor: String,
}

impl DcAuditor {
    /// Fresh sieve with an empty seen-ulid set and an empty actor (a record's
    /// `actor` is then the local node id once [`Self::with_actor`] is used; the
    /// bare `new()` is for pure-logic tests of the dedup/target/result paths).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the local node identity ([`DcAuditor::actor`]) stamped onto every
    /// emitted record. The worker supplies its `node_id`; tests can supply a fixed
    /// principal to assert the has-actor branch.
    #[must_use]
    pub fn with_actor(mut self, actor: String) -> Self {
        self.actor = actor;
        self
    }

    /// Observe one request message on an `action/dc/*` lane. Returns an
    /// [`AuditRecord`] the first time `ulid` is seen, and `None` on every
    /// subsequent sight. Advances internal state on first sight. The record's
    /// `actor` is the local node id (the Bus has no sender), `target` is extracted
    /// from `body`, `result` is [`ActionResult::Issued`] (the request is observed,
    /// not its reply), and `ts` derives from the message's write-time `ts_unix_ms`.
    pub fn observe(
        &mut self,
        topic: &str,
        ulid: &str,
        body: &str,
        ts_unix_ms: i64,
    ) -> Option<AuditRecord> {
        let record = self.project(topic, ulid, body, ts_unix_ms)?;
        self.remember(ulid);
        Some(record)
    }

    /// Derive a candidate record without advancing dedup state. The I/O worker
    /// uses this split so a failed Bus write leaves the request retryable.
    fn project(&self, topic: &str, ulid: &str, body: &str, ts_unix_ms: i64) -> Option<AuditRecord> {
        if self.seen.contains(ulid) {
            return None;
        }
        Some(AuditRecord {
            action: audit_action_name(topic),
            ulid: ulid.to_string(),
            actor: self.actor.clone(),
            target: target_of(body),
            result: ActionResult::Issued,
            ts: format_ts(ts_unix_ms),
            body_summary: body_summary(body),
        })
    }

    /// Commit one successfully published ULID to the in-memory dedup state.
    fn remember(&mut self, ulid: &str) {
        self.seen.insert(ulid.to_string());
    }

    /// Reconcile identities recovered from completely read durable output
    /// lanes before deriving any new candidates.
    fn reconcile_projected(&mut self, projected_ulids: BTreeSet<String>) {
        self.seen.extend(projected_ulids);
    }
}

// ---- thin I/O: watch the action lanes, emit audit records via the Bus ----

/// Publish one candidate record through the same already-open Bus used for the
/// complete request snapshot. Failure is explicit so dedup state is not
/// advanced until durable publication succeeds.
fn publish_record(persist: &mut Persist, rec: &AuditRecord) -> Result<(), String> {
    persist
        .write(&rec.topic(), Priority::Default, None, Some(&rec.body()))
        .map(|_| ())
        .map_err(|error| format!("publish {}: {error}", rec.topic()))
}

struct ProjectionSnapshot {
    projected_ulids: BTreeSet<String>,
    request_history: Vec<(String, StoredMessage)>,
}

/// Read every durable audit-output lane and registered request lane into one
/// complete projection snapshot. Output identities are accepted only from the
/// exact `event/dc/audit/<ulid>` shape and only after that lane itself reads
/// successfully. Any discovery or lane read failure rejects the whole snapshot;
/// callers publish and remember nothing from a partial view.
fn read_projection_snapshot(persist: &mut Persist) -> Result<ProjectionSnapshot, String> {
    let topics = persist
        .list_topics()
        .map_err(|error| format!("discover datacenter projection lanes: {error}"))?;
    let mut projected_ulids = BTreeSet::new();
    let mut request_history = Vec::new();
    for topic in &topics {
        let Some(ulid) = projected_ulid_from_audit_topic(topic) else {
            continue;
        };
        let messages = persist
            .list_since(topic, None)
            .map_err(|error| format!("read durable audit output {topic}: {error}"))?;
        if !messages.is_empty() {
            projected_ulids.insert(ulid.to_string());
        }
    }
    for topic in topics.into_iter().filter(|t| is_registered_action_topic(t)) {
        let messages = persist
            .list_since(&topic, None)
            .map_err(|error| format!("read datacenter request {topic}: {error}"))?;
        request_history.extend(messages.into_iter().map(|message| (topic.clone(), message)));
    }
    Ok(ProjectionSnapshot {
        projected_ulids,
        request_history,
    })
}

/// Transactional projection pass: obtain complete durable output and request
/// histories first, recover already-projected identities, then publish each
/// unseen candidate and remember it only after its Bus write succeeds.
fn poll_and_audit_with<R, W>(
    persist: &mut Persist,
    core: &mut DcAuditor,
    mut read: R,
    mut write: W,
) -> Result<(), String>
where
    R: FnMut(&mut Persist) -> Result<ProjectionSnapshot, String>,
    W: FnMut(&mut Persist, &AuditRecord) -> Result<(), String>,
{
    persist.reopen_if_index_changed();
    let snapshot = read(persist)?;
    core.reconcile_projected(snapshot.projected_ulids);
    for (topic, message) in snapshot.request_history {
        let body = message.body.as_deref().unwrap_or("");
        if let Some(record) = core.project(&topic, &message.ulid, body, message.ts_unix_ms) {
            write(persist, &record)?;
            core.remember(&message.ulid);
        }
    }
    Ok(())
}

fn poll_and_audit(persist: &mut Persist, core: &mut DcAuditor) -> Result<(), String> {
    poll_and_audit_with(persist, core, read_projection_snapshot, publish_record)
}

fn dc_auditor_bus_root(override_root: Option<PathBuf>) -> PathBuf {
    dc_auditor_bus_root_or_system(override_root.or_else(mde_bus::default_data_dir))
}

fn dc_auditor_bus_root_or_system(resolved: Option<PathBuf>) -> PathBuf {
    resolved.unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn next_bus_retry_interval(current: Duration) -> Duration {
    current
        .saturating_mul(2)
        .clamp(MIN_BUS_RETRY_INTERVAL, MAX_BUS_RETRY_INTERVAL)
}

/// The supervised worker. Leader-gated (only the elected node writes the audit
/// trail, so a multi-node mesh doesn't multi-audit) and best-effort.
pub struct DcAuditorWorker {
    core: DcAuditor,
    tick_interval: Duration,
    node_id: String,
    leader_lock: PathBuf,
    bus_root_override: Option<PathBuf>,
    #[cfg(test)]
    bus_open_override: Option<Arc<BusOpenFn>>,
}

impl DcAuditorWorker {
    /// Construct with production defaults (5 s tick, the shared leader lock
    /// under `workgroup_root`, the default Bus root).
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        Self {
            // Seed the sieve's actor with the local node identity — the Bus
            // envelope has no per-message sender, so the actor we can record
            // honestly is this mackesd's own mesh id (`peer:<host>`).
            core: DcAuditor::new().with_actor(node_id.clone()),
            tick_interval: DEFAULT_TICK_INTERVAL,
            leader_lock: workgroup_root.join(".mackesd-leader.lock"),
            node_id,
            bus_root_override: None,
            #[cfg(test)]
            bus_open_override: None,
        }
    }

    /// Override the Bus root directory. Used in tests.
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    /// Override the poll cadence for focused async tests.
    #[must_use]
    pub const fn with_tick_interval(mut self, interval: Duration) -> Self {
        self.tick_interval = interval;
        self
    }

    /// Override Bus opening without changing production retry behavior.
    #[cfg(test)]
    #[must_use]
    fn with_bus_opener(mut self, open: Arc<BusOpenFn>) -> Self {
        self.bus_open_override = Some(open);
        self
    }

    fn open_bus(&self, root: &Path) -> Result<Option<Persist>, String> {
        #[cfg(test)]
        if let Some(open) = self.bus_open_override.as_ref() {
            return open(root);
        }

        Persist::open(root.to_path_buf())
            .map(Some)
            .map_err(|error| error.to_string())
    }

    /// Only the directory leader audits (no-fixed-center: any eligible node can
    /// be it, the elected one writes the trail). Reuses the shared leader lock.
    fn is_leader(&self) -> bool {
        crate::leader_gate::LeaderGate::from_lock_path(
            self.leader_lock.clone(),
            self.node_id.clone(),
        )
        .is_leader()
    }
}

#[async_trait::async_trait]
impl Worker for DcAuditorWorker {
    fn name(&self) -> &'static str {
        "dc_auditor"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let bus_root = dc_auditor_bus_root(self.bus_root_override.clone());
        let mut retry_interval = MIN_BUS_RETRY_INTERVAL;
        let mut persist = loop {
            match self.open_bus(&bus_root) {
                Ok(Some(persist)) => break persist,
                Ok(None) => {
                    tracing::debug!("dc_auditor: Bus root unavailable; startup will retry")
                }
                Err(error) => tracing::warn!(
                    %error,
                    "dc_auditor: Bus open failed; startup will retry"
                ),
            }

            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(retry_interval) => {}
            }
            retry_interval = next_bus_retry_interval(retry_interval);
        };
        loop {
            if self.is_leader() {
                if let Err(error) = poll_and_audit(&mut persist, &mut self.core) {
                    tracing::warn!(
                        %error,
                        "dc_auditor: incomplete projection pass deferred"
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
    fn audit_topic_formats_under_event_dc_audit() {
        assert_eq!(audit_topic("01HZX5"), "event/dc/audit/01HZX5");
    }

    #[test]
    fn retained_vm_topics_are_not_projected_as_audit_events() {
        assert!(is_registered_action_topic("action/dc/host-power"));
        assert!(!is_registered_action_topic("action/dc/vm-power"));
        assert!(!is_registered_action_topic("action/dc/vm-delete"));
    }

    #[test]
    fn auditor_bus_root_falls_back_to_the_canonical_system_spool() {
        assert_eq!(
            dc_auditor_bus_root_or_system(None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
        assert_eq!(
            dc_auditor_bus_root_or_system(Some(PathBuf::from("/tmp/dc-auditor-explicit-bus",))),
            PathBuf::from("/tmp/dc-auditor-explicit-bus")
        );
    }

    #[test]
    fn incomplete_reads_and_failed_writes_do_not_advance_projection_state() {
        let bus = tempfile::tempdir().unwrap();
        let mut persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let host = persist
            .write(
                "action/dc/host-power",
                Priority::Default,
                None,
                Some(r#"{"dom0":"10.0.0.9","op":"reboot"}"#),
            )
            .unwrap();
        let wol = persist
            .write(
                "action/dc/wol",
                Priority::Default,
                None,
                Some(r#"{"host":"farm-1"}"#),
            )
            .unwrap();
        let mut core = DcAuditor::new().with_actor("peer:a".into());
        let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let writes_for_incomplete = Arc::clone(&writes);

        let incomplete = poll_and_audit_with(
            &mut persist,
            &mut core,
            |persist| {
                let _first_lane = persist
                    .list_since("action/dc/host-power", None)
                    .map_err(|error| error.to_string())?;
                Err("injected second request-lane read failure".into())
            },
            move |_, _| {
                writes_for_incomplete.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        );
        assert!(incomplete.is_err());
        assert!(core.seen.is_empty());
        assert_eq!(writes.load(std::sync::atomic::Ordering::SeqCst), 0);

        let write_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let write_attempts_for_failure = Arc::clone(&write_attempts);
        let failed_write = poll_and_audit_with(
            &mut persist,
            &mut core,
            |persist| {
                let message = persist
                    .list_since("action/dc/host-power", None)
                    .map_err(|error| error.to_string())?
                    .into_iter()
                    .next()
                    .expect("host action");
                Ok(ProjectionSnapshot {
                    projected_ulids: BTreeSet::new(),
                    request_history: vec![("action/dc/host-power".into(), message)],
                })
            },
            move |_, _| {
                write_attempts_for_failure.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err("injected audit Bus write failure".into())
            },
        );
        assert!(failed_write.is_err());
        assert!(core.seen.is_empty(), "failed publish must remain retryable");
        assert_eq!(write_attempts.load(std::sync::atomic::Ordering::SeqCst), 1);

        poll_and_audit(&mut persist, &mut core).expect("complete retry projects all history");
        assert_eq!(core.seen.len(), 2);
        assert_eq!(
            persist
                .list_since(&audit_topic(&host.ulid), None)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            persist
                .list_since(&audit_topic(&wol.ulid), None)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn durable_output_snapshot_makes_restart_idempotent_and_read_failure_atomic() {
        let bus = tempfile::tempdir().unwrap();
        let mut persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let request = persist
            .write(
                "action/dc/host-power",
                Priority::Default,
                None,
                Some(r#"{"dom0":"10.0.0.9","op":"reboot"}"#),
            )
            .expect("write durable request");

        let mut first_core = DcAuditor::new().with_actor("peer:first".into());
        poll_and_audit(&mut persist, &mut first_core).expect("initial projection");
        let topic = audit_topic(&request.ulid);
        assert_eq!(persist.list_since(&topic, None).unwrap().len(), 1);

        let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let writes_after_restart = Arc::clone(&writes);
        let mut restarted_core = DcAuditor::new().with_actor("peer:restart".into());
        poll_and_audit_with(
            &mut persist,
            &mut restarted_core,
            read_projection_snapshot,
            move |_, _| {
                writes_after_restart.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        )
        .expect("fresh pass recovers durable projection identity");
        assert_eq!(writes.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(restarted_core.seen.contains(&request.ulid));
        assert_eq!(persist.list_since(&topic, None).unwrap().len(), 1);

        assert_eq!(
            projected_ulid_from_audit_topic(&topic),
            Some(request.ulid.as_str())
        );
        assert_eq!(projected_ulid_from_audit_topic("event/dc/audit/"), None);
        assert_eq!(
            projected_ulid_from_audit_topic("event/dc/audit/not-a-ulid"),
            None
        );
        assert_eq!(
            projected_ulid_from_audit_topic(&format!("{topic}/extra")),
            None
        );

        let deferred_writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let deferred_writes_for_pass = Arc::clone(&deferred_writes);
        let mut failed_read_core = DcAuditor::new().with_actor("peer:retry".into());
        let incomplete_output = poll_and_audit_with(
            &mut persist,
            &mut failed_read_core,
            |persist| {
                let _existing_output = persist
                    .list_since(&topic, None)
                    .map_err(|error| error.to_string())?;
                Err("injected second audit-output lane read failure".into())
            },
            move |_, _| {
                deferred_writes_for_pass.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        );
        assert!(incomplete_output.is_err());
        assert!(failed_read_core.seen.is_empty());
        assert_eq!(deferred_writes.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn late_bus_folds_retained_history_and_forward_requests_without_restart() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let root = tempfile::tempdir().unwrap();
        let bus_root = root.path().join("bus");
        let external_bus = Persist::open(bus_root.clone()).expect("prepare delayed Bus");
        let retained = external_bus
            .write(
                "action/dc/host-power",
                Priority::Default,
                None,
                Some(r#"{"dom0":"10.0.0.9","op":"reboot"}"#),
            )
            .expect("write retained audit source");
        let workgroup = root.path().join("workgroup");
        std::fs::create_dir_all(&workgroup).unwrap();
        let open_attempts = Arc::new(AtomicUsize::new(0));
        let open_attempts_for_worker = Arc::clone(&open_attempts);
        let bus_root_for_worker = bus_root.clone();
        let mut worker = DcAuditorWorker::new(workgroup, "peer:a".into())
            .with_bus_root(bus_root.clone())
            .with_tick_interval(Duration::from_millis(5))
            .with_bus_opener(Arc::new(move |_| {
                match open_attempts_for_worker.fetch_add(1, Ordering::SeqCst) {
                    0 => Ok(None),
                    1 => Err("injected unopenable audit Bus".into()),
                    _ => Persist::open(bus_root_for_worker.clone())
                        .map(Some)
                        .map_err(|error| error.to_string()),
                }
            }));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );

        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                assert!(
                    !task.is_finished(),
                    "worker exited during late-Bus recovery"
                );
                if !external_bus
                    .list_since(&audit_topic(&retained.ulid), None)
                    .unwrap()
                    .is_empty()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("retained durable request history must be projected");
        assert!(open_attempts.load(Ordering::SeqCst) >= 3);

        // The worker retains its own handle. Publish after activation through
        // this independently opened handle to prove per-pass refresh observes
        // external Bus writers without restarting the worker.
        let forward = external_bus
            .write(
                "action/dc/wol",
                Priority::Default,
                None,
                Some(r#"{"host":"farm-2"}"#),
            )
            .expect("write forward audit source");
        tokio::time::timeout(Duration::from_secs(3), async {
            while external_bus
                .list_since(&audit_topic(&forward.ulid), None)
                .unwrap()
                .is_empty()
            {
                assert!(
                    !task.is_finished(),
                    "worker exited before forward projection"
                );
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("forward request must be projected");
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            external_bus
                .list_since(&audit_topic(&retained.ulid), None)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            external_bus
                .list_since(&audit_topic(&forward.ulid), None)
                .unwrap()
                .len(),
            1
        );

        shutdown_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("shutdown must interrupt worker promptly")
            .expect("worker task must join")
            .expect("worker must exit cleanly");
    }

    /// perf-10 / arch-6 — `publish_record` writes the audit record in-process (no
    /// fork+exec of `mde-bus`) with EXACTLY the row a
    /// `mde-bus publish event/dc/audit/<ulid> --body-flag <body>` produced: the
    /// topic, default priority, no title/actions/reply, and the record's
    /// `body()` string verbatim.
    #[test]
    fn publish_record_writes_cli_equivalent_row_in_process() {
        let tmp = tempfile::tempdir().unwrap();
        // Build an audit record the dedup core would have emitted.
        let rec = AuditRecord {
            action: "dc/vm-power".to_string(),
            ulid: "ulid-1".to_string(),
            actor: "peer:eagle".to_string(),
            target: "web1".to_string(),
            result: ActionResult::Issued,
            ts: "1700000000000".to_string(),
            body_summary: r#"{"uuid":"web1","op":"on"}"#.to_string(),
        };

        let mut persist = Persist::open(tmp.path().to_path_buf()).unwrap();
        publish_record(&mut persist, &rec).unwrap();

        // Read the row back through a fresh handle, as any Bus consumer does.
        let reader = Persist::open(tmp.path().to_path_buf()).unwrap();
        let audit_topic = super::audit_topic(&rec.ulid);
        let rows = reader.list_since(&audit_topic, None).unwrap();
        assert_eq!(rows.len(), 1, "exactly one audit record published");
        let row = &rows[0];
        assert_eq!(row.topic, audit_topic);
        assert_eq!(row.priority, "default");
        assert!(row.title.is_none());
        assert!(row.actions.is_empty());
        assert!(row.reply_to.is_none());
        // Byte-identical to the record's `body()` — what `--body-flag` carried.
        assert_eq!(row.body.as_deref(), Some(rec.body().as_str()));
    }

    #[test]
    fn audit_action_name_strips_action_prefix() {
        assert_eq!(audit_action_name("action/dc/vm-power"), "dc/vm-power");
        assert_eq!(
            audit_action_name("action/dc/droplet-create"),
            "dc/droplet-create"
        );
        // A topic without the prefix passes through unchanged.
        assert_eq!(audit_action_name("dc/vm-power"), "dc/vm-power");
    }

    #[test]
    fn observe_emits_once_per_ulid_then_dedups() {
        let mut a = DcAuditor::new();
        // First sight → a record on the right topic with the action + summary.
        let rec = a
            .observe(
                "action/dc/vm-power",
                "ulid-1",
                r#"{"uuid":"web1","op":"on"}"#,
                1_700_000_000_000,
            )
            .expect("first sight emits");
        assert_eq!(rec.action, "dc/vm-power");
        assert_eq!(rec.ulid, "ulid-1");
        assert_eq!(rec.topic(), "event/dc/audit/ulid-1");
        assert!(rec.body_summary.contains(r#""uuid":"web1""#));
        // The published body carries action + ulid + actor + target + result + ts.
        let body = rec.body();
        assert!(body.contains(r#""action":"dc/vm-power""#));
        assert!(body.contains(r#""ulid":"ulid-1""#));
        assert!(body.contains("body_summary"));
        assert!(body.contains(r#""result":"issued""#));
        assert!(body.contains(r#""target":"web1""#));
        // Second sight of the SAME ulid → no record (deduped).
        assert!(a.observe("action/dc/vm-power", "ulid-1", "{}", 0).is_none());
        // A different ulid → a fresh record.
        assert!(a
            .observe("action/dc/droplet-create", "ulid-2", "{}", 0)
            .is_some());
    }

    #[test]
    fn body_summary_truncates_at_the_cap_on_char_boundary() {
        let long = "x".repeat(500);
        let mut a = DcAuditor::new();
        let rec = a.observe("action/dc/vm-power", "u", &long, 0).unwrap();
        assert_eq!(rec.body_summary.chars().count(), BODY_SUMMARY_LEN);
        // Multibyte body — truncation must not split a char (no panic, valid utf8).
        let multi = "é".repeat(500);
        let rec2 = a.observe("action/dc/vm-power", "u2", &multi, 0).unwrap();
        assert_eq!(rec2.body_summary.chars().count(), BODY_SUMMARY_LEN);
    }

    #[test]
    fn record_stamps_local_node_as_actor_when_set() {
        // The has-actor branch: the sieve seeded with a node id stamps it as the
        // initiating principal (the Bus envelope carries no sender, so the actor
        // is the local node we run as).
        let mut a = DcAuditor::new().with_actor("peer:anvil".to_string());
        let rec = a
            .observe("action/dc/vm-delete", "u1", r#"{"uuid":"abc-123"}"#, 1)
            .unwrap();
        assert_eq!(rec.actor, "peer:anvil");
        assert!(rec.body().contains(r#""actor":"peer:anvil""#));
    }

    #[test]
    fn record_has_empty_actor_when_unset() {
        // The no-actor branch: a bare sieve (no node id seeded) records an empty
        // actor rather than inventing one — the body still carries the field.
        let mut a = DcAuditor::new();
        let rec = a
            .observe("action/dc/vm-power", "u2", r#"{"uuid":"x"}"#, 1)
            .unwrap();
        assert_eq!(rec.actor, "");
        assert!(rec.body().contains(r#""actor":"""#));
    }

    #[test]
    fn result_is_issued_not_a_fabricated_outcome() {
        // The passive auditor sees the request, not the reply — it records
        // "issued" honestly, never a fabricated ok/fail.
        let mut a = DcAuditor::new();
        let rec = a.observe("action/dc/vm-power", "u3", "{}", 1).unwrap();
        assert_eq!(rec.result, ActionResult::Issued);
        assert_eq!(rec.result.as_str(), "issued");
    }

    #[test]
    fn target_extracted_from_body_across_verb_shapes() {
        let mut a = DcAuditor::new();
        // VM verb → uuid.
        let r = a
            .observe(
                "action/dc/vm-power",
                "t1",
                r#"{"uuid":"vm-uuid","op":"on"}"#,
                1,
            )
            .unwrap();
        assert_eq!(r.target, "vm-uuid");
        // Storage verb → vbd (a detach body).
        let r = a
            .observe(
                "action/dc/vdi-detach",
                "t2",
                r#"{"vbd":"ba5e","dom0":"10.0.0.1","confirm":true}"#,
                1,
            )
            .unwrap();
        assert_eq!(r.target, "ba5e"); // vbd beats dom0 in priority
                                      // Host verb → dom0 (no higher-priority field present).
        let r = a
            .observe(
                "action/dc/host-power",
                "t3",
                r#"{"dom0":"10.0.0.9","op":"reboot"}"#,
                1,
            )
            .unwrap();
        assert_eq!(r.target, "10.0.0.9");
        // Lighthouse verb → node.
        let r = a
            .observe(
                "action/dc/lighthouse-promote",
                "t4",
                r#"{"node":"shadow-1","confirm":true}"#,
                1,
            )
            .unwrap();
        assert_eq!(r.target, "shadow-1");
        // No recognized field / non-JSON → empty target (honest, not invented).
        let r = a.observe("action/dc/do-regions", "t5", "{}", 1).unwrap();
        assert_eq!(r.target, "");
        let r = a
            .observe("action/dc/vm-power", "t6", "not json", 1)
            .unwrap();
        assert_eq!(r.target, "");
    }

    #[test]
    fn ts_is_zero_padded_epoch_ms_so_lexical_sort_is_time_sort() {
        let mut a = DcAuditor::new();
        let early = a.observe("action/dc/vm-power", "e", "{}", 1_700_000_000_000);
        let late = a.observe("action/dc/vm-power", "l", "{}", 1_800_000_000_000);
        let early = early.unwrap().ts;
        let late = late.unwrap().ts;
        assert_eq!(early, "1700000000000");
        assert_eq!(late, "1800000000000");
        // Lexical order matches time order (the panel sorts newest-first on this).
        assert!(early < late);
        // A non-positive timestamp clamps to a zero-padded zero (still sortable).
        let z = a.observe("action/dc/vm-power", "z", "{}", -5).unwrap().ts;
        assert_eq!(z, "0000000000000");
        assert!(z < early);
    }
}
