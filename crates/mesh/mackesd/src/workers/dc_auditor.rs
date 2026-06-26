//! DATACENTER-7 (audit half) — the passive `dc_auditor` worker.
//!
//! The companion to [`super::datacenter_orchestrator`]: where the orchestrator
//! *publishes* datacenter state, this worker is a **read-only audit subscriber**.
//! It watches the Bus action lanes (`action/dc/*` — VM power, droplet lifecycle,
//! gateway changes, …) and emits one append-only audit record per request to
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
use std::path::PathBuf;
use std::time::Duration;

use mde_bus::persist::Persist;

use super::{ShutdownToken, Worker};

/// Sweep cadence — 5 s (audit records should trail actions closely without
/// hammering the Bus index).
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(5);

/// The Bus prefix the auditor watches — every datacenter action lane.
pub const ACTION_DC_PREFIX: &str = "action/dc/";

/// Max characters of the request body carried into the audit record's
/// `body_summary`. Keeps the audit lane compact; the full body stays on the
/// original action message.
pub const BODY_SUMMARY_LEN: usize = 120;

/// Bus topic an audit record for `ulid` is published to: `event/dc/audit/<ulid>`.
#[must_use]
pub fn audit_topic(ulid: &str) -> String {
    format!("event/dc/audit/{ulid}")
}

/// The audited action name for a Bus topic: strips the leading `action/` so
/// `action/dc/vm-power` → `dc/vm-power`. Topics without the prefix pass through
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
    /// The audited action name (`dc/vm-power`, …) — the source topic minus
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
        if !self.seen.insert(ulid.to_string()) {
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
}

// ---- thin I/O: watch the action lanes, emit audit records via the Bus ----

/// Publish one audit record onto the Bus (best-effort, fire-and-reap — same lane
/// shape as the datacenter_orchestrator's events).
fn publish(rec: &AuditRecord) {
    let mut cmd = std::process::Command::new("mde-bus");
    cmd.args(["publish", &rec.topic(), "--body-flag", &rec.body()]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// One poll pass: enumerate every `action/dc/*` topic and feed each message
/// through the dedup core, publishing the records that survive (first-sight
/// ulids). Best-effort: a failed `list_topics`/`list_since` is logged + skipped.
fn poll_and_audit(persist: &Persist, core: &mut DcAuditor) {
    let topics = match persist.list_topics() {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!(error = %e, "dc_auditor: list_topics failed");
            return;
        }
    };
    for topic in topics.iter().filter(|t| t.starts_with(ACTION_DC_PREFIX)) {
        let msgs = match persist.list_since(topic, None) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "dc_auditor: list_since failed");
                continue;
            }
        };
        for msg in msgs {
            let body = msg.body.as_deref().unwrap_or("");
            if let Some(rec) = core.observe(topic, &msg.ulid, body, msg.ts_unix_ms) {
                publish(&rec);
            }
        }
    }
}

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

/// The supervised worker. Leader-gated (only the elected node writes the audit
/// trail, so a multi-node mesh doesn't multi-audit) and best-effort.
pub struct DcAuditorWorker {
    core: DcAuditor,
    tick_interval: Duration,
    node_id: String,
    leader_lock: PathBuf,
    bus_root_override: Option<PathBuf>,
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
        }
    }

    /// Override the Bus root directory. Used in tests.
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    /// Only the directory leader audits (no-fixed-center: any eligible node can
    /// be it, the elected one writes the trail). Reuses the shared leader lock.
    fn is_leader(&self) -> bool {
        matches!(
            crate::leader::try_acquire(&self.leader_lock, &self.node_id),
            Ok(crate::leader::AcquireResult::Acquired)
        )
    }
}

#[async_trait::async_trait]
impl Worker for DcAuditorWorker {
    fn name(&self) -> &'static str {
        "dc_auditor"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let bus_root = match self.bus_root_override.clone().or_else(default_bus_root) {
            Some(r) => r,
            None => {
                tracing::debug!("dc_auditor: no bus root; worker idle");
                return Ok(());
            }
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(error = %e, "dc_auditor: persist open failed; worker idle");
                return Ok(());
            }
        };
        loop {
            if self.is_leader() {
                poll_and_audit(&persist, &mut self.core);
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
        assert!(a
            .observe("action/dc/vm-power", "ulid-1", "{}", 0)
            .is_none());
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
            .observe("action/dc/vm-power", "t1", r#"{"uuid":"vm-uuid","op":"on"}"#, 1)
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
            .observe("action/dc/host-power", "t3", r#"{"dom0":"10.0.0.9","op":"reboot"}"#, 1)
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
        let r = a
            .observe("action/dc/do-regions", "t5", "{}", 1)
            .unwrap();
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
