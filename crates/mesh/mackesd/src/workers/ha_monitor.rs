//! HA-5 — etcd-quorum + leadership monitor (the data/alert half of Mesh Control).
//!
//! The Network → Mesh Control panel already *renders* the etcd-quorum +
//! lighthouse-HA picture, but it sourced everything from a live healthz RPC:
//! nothing **published** the coordination-plane state onto the Bus, and nothing
//! **alerted** the operator when the mesh lost quorum or changed leaders. This
//! worker closes that half. Every tick it reads the live mesh directory — the
//! same etcd-or-fs source `healthz` enrichment uses (`DirectoryService`, §6
//! reuse, not a re-derived read) — and extracts an [`HaSnapshot`] (etcd member
//! count + quorum-OK + current leader). Then it:
//!
//!   * publishes the snapshot to the retained data lane [`STATUS_TOPIC`]
//!     (`mesh/ha/status`) — **on change only**, at `high` priority so the lone
//!     current-state row survives retention — so the Mesh Control panel can
//!     consume the authoritative published view (`fetch_ha_status`); and
//!   * fires an **edge-triggered** alert on the Hub alert lane [`ALERT_TOPIC`]
//!     (`mackesd::alert`, the mesh-internal System lane the Notification Hub
//!     tails) when quorum is **LOST**/restored or **leadership changes** — never
//!     a per-tick repeat (the transition is computed against the prior snapshot).
//!
//! `DirectoryService::build_directory` does a blocking etcd read (the SUBSTRATE
//! `block_on` bridge), so the gather runs under `spawn_blocking` — off the tokio
//! executor, where the bridge takes its simple current-thread-runtime path.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::ipc::directory::DirectoryService;
use crate::workers::{ShutdownToken, Worker};

/// Poll cadence — quorum/leadership are not latency-critical; the directory read
/// is moderately heavy, so a 30 s tick keeps it cheap while staying responsive
/// enough for an operator watching a failover.
pub const DEFAULT_TICK: std::time::Duration = std::time::Duration::from_secs(30);

/// Retained data lane the panel consumes (NOT an alert lane — see
/// `mde_notify::topic_is_alert_lane`, which excludes it, so it never toasts).
pub const STATUS_TOPIC: &str = "mesh/ha/status";

/// The Hub alert lane for mesh-internal alerts (`mde_notify::classify_source`
/// maps it to the System group; `topic_is_alert_lane` tails it).
pub const ALERT_TOPIC: &str = "mackesd::alert";

/// The HA-relevant slice of the coordination-plane state: the etcd member count
/// (the quorum runs on the lighthouses, so members == lighthouse roster), whether
/// the plane currently has an elected leader (quorum-OK — etcd only serves the
/// leader lease while it holds a majority), and that leader's bare hostname.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HaSnapshot {
    /// etcd-quorum member count (lighthouse roster size).
    pub member_count: u32,
    /// `true` when a leader is currently elected — a proxy for "etcd has quorum"
    /// (no quorum ⇒ the lease can't be held ⇒ the key vanishes ⇒ no leader).
    pub quorum_ok: bool,
    /// Current mesh leader's bare hostname, or `None` when no leader is elected.
    pub leader: Option<String>,
}

/// The JSON document published to [`STATUS_TOPIC`]. The panel parses the same
/// field names into its own `HaStatus` (shape shared by convention, like
/// `HealthReport` ↔ the panel's `HealthSummary` — no cross-crate dep).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HaStatusDoc {
    /// etcd-quorum member count.
    pub member_count: u32,
    /// Whether a leader is elected (quorum healthy).
    pub quorum_ok: bool,
    /// Current leader's bare hostname (omitted/`null` when none).
    #[serde(default)]
    pub leader: Option<String>,
    /// Epoch ms the snapshot was taken.
    pub ts_unix_ms: i64,
}

impl HaStatusDoc {
    #[must_use]
    fn from_snapshot(snap: &HaSnapshot) -> Self {
        Self {
            member_count: snap.member_count,
            quorum_ok: snap.quorum_ok,
            leader: snap.leader.clone(),
            ts_unix_ms: now_ms(),
        }
    }
}

/// One edge-triggered HA alert. `severity` is a `mde_notify` token
/// (`crit`/`warn`/`ok`); the worker maps it to a bus priority for retention/DND.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HaAlert {
    /// `crit` | `warn` | `ok` (consumed by `mde_notify::classify_severity`).
    pub severity: &'static str,
    /// Short alert title (the `alert` field the Hub reads).
    pub title: String,
    /// Operator-facing one-line summary.
    pub summary: String,
}

/// HA-5 — the alert-on-transition logic (pure + unit-tested). Compares the prior
/// snapshot to the current one and returns the alerts the edge warrants:
///   * **quorum LOST** — had a leader, now none (critical);
///   * **quorum restored** — no leader, now one (ok/success);
///   * **leadership change** — both have a leader but a different host (warning).
///
/// Edge-triggered by construction: an unchanged snapshot (or a member-count-only
/// change) yields no alerts, so a steady mesh never spams the Hub. The
/// None→Some(leader) restore and the Some→Some leadership-change are mutually
/// exclusive (the change arm requires a leader on *both* sides), so a recovery
/// never double-fires.
#[must_use]
pub fn ha_transition_alerts(prev: &HaSnapshot, cur: &HaSnapshot) -> Vec<HaAlert> {
    let mut out = Vec::new();

    // Quorum LOST — the coordination plane had a leader and now has none.
    if prev.quorum_ok && !cur.quorum_ok {
        out.push(HaAlert {
            severity: "crit",
            title: "etcd quorum lost".to_string(),
            summary: format!(
                "the mesh coordination plane lost its leader — etcd quorum is down \
                 or no node holds the lease ({} member(s))",
                cur.member_count
            ),
        });
    }

    // Quorum RESTORED — re-elected a leader after having none.
    if !prev.quorum_ok && cur.quorum_ok {
        let who = cur.leader.as_deref().unwrap_or("?");
        out.push(HaAlert {
            severity: "ok",
            title: "etcd quorum restored".to_string(),
            summary: format!(
                "the coordination plane re-elected a leader: {who} ({} member(s))",
                cur.member_count
            ),
        });
    }

    // Leadership CHANGE — a live leader handed off to a different host (both
    // sides Some, so a None→Some recovery is handled by the restore arm above,
    // not here — no double-fire).
    if let (Some(prev_l), Some(cur_l)) = (prev.leader.as_deref(), cur.leader.as_deref()) {
        if prev_l != cur_l {
            out.push(HaAlert {
                severity: "warn",
                title: "mesh leadership changed".to_string(),
                summary: format!("leadership moved from {prev_l} to {cur_l}"),
            });
        }
    }

    out
}

/// Extract the [`HaSnapshot`] from an `action/mesh/directory` reply value (the
/// shape `DirectoryService::build_directory` produces). Pure + testable: the
/// member count is the lighthouse-tagged peer rows (the etcd quorum runs on the
/// lighthouses), the leader is the directory's `leader` field, and quorum-OK is
/// simply "a leader is elected".
#[must_use]
pub fn extract_ha(dir: &serde_json::Value) -> HaSnapshot {
    let leader = dir
        .get("leader")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let members = dir
        .get("peers")
        .and_then(serde_json::Value::as_array)
        .map(|peers| {
            peers
                .iter()
                .filter(|p| p.get("role").and_then(serde_json::Value::as_str) == Some("lighthouse"))
                .count()
        })
        .unwrap_or(0);
    HaSnapshot {
        member_count: u32::try_from(members).unwrap_or(u32::MAX),
        quorum_ok: leader.is_some(),
        leader,
    }
}

/// Build the `mackesd::alert` body for one HA alert. JSON with the fields the
/// Notification Hub reads (`mde_notify::alert_from_message`):
/// `severity`/`alert`/`summary`/`host`. Pure + testable.
#[must_use]
pub fn ha_alert_body(host: &str, a: &HaAlert) -> String {
    serde_json::json!({
        "host": host,
        "severity": a.severity,
        "alert": a.title,
        "summary": a.summary,
    })
    .to_string()
}

/// Map an HA severity token to a bus priority (retention TTL + DND tier). The
/// `severity` field still wins for the Hub's color; this only sets the lane.
#[must_use]
fn alert_priority(severity: &str) -> &'static str {
    match severity {
        "crit" => "urgent",
        "warn" => "high",
        _ => "default",
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Read the live coordination-plane state via the directory service (the same
/// etcd-or-fs source healthz uses). MUST run off the tokio executor (the worker
/// calls it under `spawn_blocking`) because `build_directory`'s etcd read uses
/// the SUBSTRATE blocking bridge.
#[must_use]
pub fn gather_snapshot(workgroup_root: &Path, db_path: Option<PathBuf>) -> HaSnapshot {
    let svc = DirectoryService::new(workgroup_root, db_path);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    extract_ha(&svc.build_directory(now))
}

/// Publish the current snapshot to the retained [`STATUS_TOPIC`] data lane at
/// `high` priority (30-day TTL — the single current-state row outlives long
/// stable periods so the panel always has a value to read).
fn publish_ha_status(snap: &HaSnapshot) {
    let Ok(body) = serde_json::to_string(&HaStatusDoc::from_snapshot(snap)) else {
        return;
    };
    let mut cmd = Command::new("mde-bus");
    cmd.args([
        "publish",
        STATUS_TOPIC,
        "--body-flag",
        &body,
        "--priority",
        "high",
    ]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// Publish one edge-triggered alert to the Hub alert lane [`ALERT_TOPIC`].
fn publish_ha_alert(host: &str, a: &HaAlert) {
    let body = ha_alert_body(host, a);
    let mut cmd = Command::new("mde-bus");
    cmd.args([
        "publish",
        ALERT_TOPIC,
        "--body-flag",
        &body,
        "--priority",
        alert_priority(a.severity),
    ]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// Worker handle. Cheap to construct.
pub struct HaMonitorWorker {
    workgroup_root: PathBuf,
    db_path: Option<PathBuf>,
    /// Hostname tagged into the alert body (the node that observed the edge).
    host: String,
    tick: std::time::Duration,
}

impl HaMonitorWorker {
    /// New worker reading the directory under `workgroup_root` (with the store at
    /// `db_path`) and tagging alerts with `host`.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, db_path: Option<PathBuf>, host: String) -> Self {
        Self {
            workgroup_root,
            db_path,
            host,
            tick: DEFAULT_TICK,
        }
    }

    /// Override the tick cadence (tests use a short value).
    #[must_use]
    pub fn with_tick(mut self, tick: std::time::Duration) -> Self {
        self.tick = tick;
        self
    }

    /// One tick: gather the live snapshot, publish it on change (seeding on the
    /// first observation), and fire any edge-triggered alerts vs `prev`. Returns
    /// the fresh snapshot for the caller to carry as the next `prev`.
    async fn tick_and_publish(&self, prev: Option<&HaSnapshot>) -> HaSnapshot {
        let root = self.workgroup_root.clone();
        let db = self.db_path.clone();
        let cur = tokio::task::spawn_blocking(move || gather_snapshot(&root, db))
            .await
            .unwrap_or_default();

        // Data lane: publish on first observation or any change only (bounded
        // topic; the panel reads the latest). A member-count-only change still
        // refreshes the published view, but does not alert.
        if prev != Some(&cur) {
            publish_ha_status(&cur);
        }

        // Alert lane: edge-triggered transitions only (no baseline on first tick).
        if let Some(prev) = prev {
            for a in ha_transition_alerts(prev, &cur) {
                publish_ha_alert(&self.host, &a);
            }
        }

        cur
    }
}

#[async_trait::async_trait]
impl Worker for HaMonitorWorker {
    fn name(&self) -> &'static str {
        "ha_monitor"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // Establish + publish a baseline immediately (no alerts on the first
        // read — there's no prior snapshot to transition from).
        let mut last = Some(self.tick_and_publish(None).await);
        loop {
            tokio::select! {
                () = tokio::time::sleep(self.tick) => {
                    last = Some(self.tick_and_publish(last.as_ref()).await);
                }
                () = shutdown.wait() => {
                    tracing::info!(target: "mackesd::ha_monitor", "shutdown requested");
                    break;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(members: u32, leader: Option<&str>) -> HaSnapshot {
        HaSnapshot {
            member_count: members,
            quorum_ok: leader.is_some(),
            leader: leader.map(str::to_string),
        }
    }

    // ---- extract_ha (directory → snapshot) -------------------

    #[test]
    fn extract_ha_counts_lighthouses_and_reads_leader() {
        // The exact shape build_directory emits: a `leader` field + `peers` rows
        // carrying a `role`. Two lighthouses + one server ⇒ member_count 2.
        let dir = serde_json::json!({
            "ok": true,
            "leader": "kiln",
            "peers": [
                {"hostname": "anvil", "role": "lighthouse"},
                {"hostname": "forge", "role": "lighthouse"},
                {"hostname": "shop",  "role": "server"},
            ],
        });
        let s = extract_ha(&dir);
        assert_eq!(s.member_count, 2);
        assert!(s.quorum_ok);
        assert_eq!(s.leader.as_deref(), Some("kiln"));
    }

    #[test]
    fn extract_ha_no_leader_is_no_quorum() {
        let dir = serde_json::json!({
            "ok": true,
            "leader": serde_json::Value::Null,
            "peers": [{"hostname": "anvil", "role": "lighthouse"}],
        });
        let s = extract_ha(&dir);
        assert_eq!(s.member_count, 1);
        assert!(!s.quorum_ok);
        assert_eq!(s.leader, None);
        // An empty-string leader is also "no leader".
        let dir2 = serde_json::json!({"leader": "", "peers": []});
        assert!(!extract_ha(&dir2).quorum_ok);
        assert_eq!(extract_ha(&dir2).member_count, 0);
    }

    // ---- ha_transition_alerts (the alert-on-transition logic) ----

    #[test]
    fn no_alert_when_nothing_changed() {
        let a = snap(3, Some("kiln"));
        assert!(ha_transition_alerts(&a, &a).is_empty());
    }

    #[test]
    fn no_alert_on_member_count_only_change() {
        // A lighthouse joined (2 → 3) but the leader is unchanged: the published
        // status refreshes, but no alert fires.
        let prev = snap(2, Some("kiln"));
        let cur = snap(3, Some("kiln"));
        assert!(ha_transition_alerts(&prev, &cur).is_empty());
    }

    #[test]
    fn quorum_lost_fires_one_critical() {
        let prev = snap(3, Some("kiln"));
        let cur = snap(3, None); // leader gone — quorum lost
        let alerts = ha_transition_alerts(&prev, &cur);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, "crit");
        assert!(alerts[0].title.contains("quorum lost"));
        // No leadership-change alert is mixed in (cur has no leader).
        assert!(!alerts.iter().any(|a| a.title.contains("leadership")));
    }

    #[test]
    fn quorum_restored_fires_one_ok_not_a_leadership_change() {
        // Recovery to a DIFFERENT leader must fire only the restore (the change
        // arm requires a leader on both sides), so no double-fire.
        let prev = snap(3, None);
        let cur = snap(3, Some("forge"));
        let alerts = ha_transition_alerts(&prev, &cur);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, "ok");
        assert!(alerts[0].title.contains("quorum restored"));
        assert!(alerts[0].summary.contains("forge"));
    }

    #[test]
    fn leadership_change_fires_one_warning() {
        let prev = snap(3, Some("kiln"));
        let cur = snap(3, Some("anvil"));
        let alerts = ha_transition_alerts(&prev, &cur);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, "warn");
        assert!(alerts[0].title.contains("leadership changed"));
        assert!(alerts[0].summary.contains("kiln"));
        assert!(alerts[0].summary.contains("anvil"));
    }

    // ---- alert body + status doc shape -----------------------

    #[test]
    fn ha_alert_body_is_valid_json_with_hub_fields() {
        let a = HaAlert {
            severity: "crit",
            title: "etcd quorum lost".into(),
            summary: "no leader (3 members)".into(),
        };
        let body = ha_alert_body("UNIT-EAGLE", &a);
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid json");
        assert_eq!(v["severity"], "crit");
        assert_eq!(v["host"], "UNIT-EAGLE");
        assert_eq!(v["alert"], "etcd quorum lost");
        assert!(v["summary"].as_str().unwrap().contains("no leader"));
    }

    #[test]
    fn alert_priority_maps_severity_to_lane() {
        assert_eq!(alert_priority("crit"), "urgent");
        assert_eq!(alert_priority("warn"), "high");
        assert_eq!(alert_priority("ok"), "default");
        assert_eq!(alert_priority("info"), "default");
    }

    #[test]
    fn status_doc_round_trips_with_null_leader() {
        let doc = HaStatusDoc::from_snapshot(&snap(0, None));
        let json = serde_json::to_string(&doc).unwrap();
        let back: HaStatusDoc = serde_json::from_str(&json).unwrap();
        assert_eq!(back.member_count, 0);
        assert!(!back.quorum_ok);
        assert_eq!(back.leader, None);
        // And a populated one.
        let doc2 = HaStatusDoc::from_snapshot(&snap(3, Some("kiln")));
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&doc2).unwrap()).unwrap();
        assert_eq!(v["member_count"], 3);
        assert_eq!(v["quorum_ok"], true);
        assert_eq!(v["leader"], "kiln");
    }

    #[test]
    fn name_is_stable() {
        let w = HaMonitorWorker::new(PathBuf::from("/x"), None, "h".into());
        assert_eq!(w.name(), "ha_monitor");
    }
}
