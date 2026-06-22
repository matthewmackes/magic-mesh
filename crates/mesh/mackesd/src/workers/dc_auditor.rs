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

/// One append-only audit record the auditor decided to emit (one datacenter
/// action observed for the first time).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AuditRecord {
    /// The audited action name (`dc/vm-power`, …) — the source topic minus
    /// the `action/` prefix.
    pub action: String,
    /// The request message's ULID (also the audit topic's leaf).
    pub ulid: String,
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
            "body_summary": self.body_summary,
        })
        .to_string()
    }
}

/// Pure audit core: tracks which request ULIDs have already been audited and
/// returns a record ONLY on first sight of a ulid — so a re-poll of the same
/// Bus lane never emits a duplicate audit record.
#[derive(Default)]
pub struct DcAuditor {
    seen: BTreeSet<String>,
}

impl DcAuditor {
    /// Fresh sieve with an empty seen-ulid set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe one request message on an `action/dc/*` lane. Returns an
    /// [`AuditRecord`] the first time `ulid` is seen, and `None` on every
    /// subsequent sight. Advances internal state on first sight.
    pub fn observe(&mut self, topic: &str, ulid: &str, body: &str) -> Option<AuditRecord> {
        if !self.seen.insert(ulid.to_string()) {
            return None;
        }
        Some(AuditRecord {
            action: audit_action_name(topic),
            ulid: ulid.to_string(),
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
            if let Some(rec) = core.observe(topic, &msg.ulid, body) {
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
            core: DcAuditor::new(),
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
            .observe("action/dc/vm-power", "ulid-1", r#"{"vm":"web1","op":"on"}"#)
            .expect("first sight emits");
        assert_eq!(rec.action, "dc/vm-power");
        assert_eq!(rec.ulid, "ulid-1");
        assert_eq!(rec.topic(), "event/dc/audit/ulid-1");
        assert!(rec.body_summary.contains(r#""vm":"web1""#));
        // The published body carries action + ulid + summary.
        let body = rec.body();
        assert!(body.contains(r#""action":"dc/vm-power""#));
        assert!(body.contains(r#""ulid":"ulid-1""#));
        assert!(body.contains("body_summary"));
        // Second sight of the SAME ulid → no record (deduped).
        assert!(a.observe("action/dc/vm-power", "ulid-1", "{}").is_none());
        // A different ulid → a fresh record.
        assert!(a
            .observe("action/dc/droplet-create", "ulid-2", "{}")
            .is_some());
    }

    #[test]
    fn body_summary_truncates_at_the_cap_on_char_boundary() {
        let long = "x".repeat(500);
        let mut a = DcAuditor::new();
        let rec = a.observe("action/dc/vm-power", "u", &long).unwrap();
        assert_eq!(rec.body_summary.chars().count(), BODY_SUMMARY_LEN);
        // Multibyte body — truncation must not split a char (no panic, valid utf8).
        let multi = "é".repeat(500);
        let rec2 = a.observe("action/dc/vm-power", "u2", &multi).unwrap();
        assert_eq!(rec2.body_summary.chars().count(), BODY_SUMMARY_LEN);
    }
}
