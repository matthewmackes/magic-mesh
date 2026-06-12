//! TUNE-16.d — Q22 8-peer cap counter.
//!
//! Periodically reads the enrolled peer count from the `nodes` store,
//! applies the Q22 8-peer cap rule, writes a JSON snapshot to
//! `~/.cache/mde/peer-cap.json`, and publishes a live update to the
//! `mesh/peer-cap/updated` Bus topic so the Portal + Workbench Mesh
//! panel can render real-time cap utilization.
//!
//! **Counting rules (per Q22 + TUNE-16.d + federation-pairing §6):**
//!
//! - `role = 'peer'` nodes count. This includes phones enrolled as
//!   Nebula peers via TUNE-16.b/.c — at the store layer, a phone is
//!   indistinguishable from a desktop peer.
//! - `role = 'host'` (the local node) does NOT count — you don't
//!   consume a slot in your own mesh.
//! - `role = 'observer'` / `'decommissioned'` do NOT count.
//! - Federated peers from external paired meshes (TUNE-15.b/.c) do
//!   NOT appear in the local `nodes` store at all — they hold cross-
//!   signed Nebula certs rather than enrollment records, so they are
//!   naturally excluded without any special check.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::Mutex;

use super::{ShutdownToken, Worker};
use crate::store::{list_nodes, NodeRow};

/// Q22 hard cap: maximum enrolled non-local, non-observer peers
/// (including phones) in one Mackes mesh.
pub const PEER_CAP: u8 = 8;

/// Default sweep cadence.
pub const DEFAULT_SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// Bus topic for live cap-utilization updates.
pub const CAP_TOPIC: &str = "mesh/peer-cap/updated";

/// Snapshot of the current cap utilization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerCapSnapshot {
    /// Unix-epoch seconds at snapshot time.
    pub checked_at: i64,
    /// Enrolled non-local peers counted toward the cap. Phones
    /// (enrolled via TUNE-16.b/.c) count; federated external-mesh
    /// peers don't appear in the local store so they don't count.
    pub cap_used: u8,
    /// Q22 hard limit.
    pub cap_limit: u8,
    /// True when `cap_used >= cap_limit`.
    pub cap_full: bool,
}

impl PeerCapSnapshot {
    /// Build a snapshot from the cap-eligible peer count.
    #[must_use]
    pub fn from_count(cap_used: u8) -> Self {
        Self {
            checked_at: epoch_secs(),
            cap_used,
            cap_limit: PEER_CAP,
            cap_full: cap_used >= PEER_CAP,
        }
    }

    /// How many additional peers can join before the cap is reached.
    #[must_use]
    pub fn remaining_slots(&self) -> u8 {
        self.cap_limit.saturating_sub(self.cap_used)
    }
}

fn epoch_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// TUNE-16.d worker — counts enrolled peers, writes cap snapshot, and
/// publishes to the Bus.
pub struct PeerCapWorker {
    store: Arc<Mutex<rusqlite::Connection>>,
    cache_path: PathBuf,
    interval: Duration,
}

impl PeerCapWorker {
    /// Construct the worker. `cache_path` normally points to
    /// `~/.cache/mde/peer-cap.json`; tests use a tempdir path.
    #[must_use]
    pub fn new(store: Arc<Mutex<rusqlite::Connection>>, cache_path: PathBuf) -> Self {
        Self {
            store,
            cache_path,
            interval: DEFAULT_SWEEP_INTERVAL,
        }
    }

    /// Override the sweep interval (useful in tests / fast-cadence debug).
    #[must_use]
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }
}

#[async_trait::async_trait]
impl Worker for PeerCapWorker {
    fn name(&self) -> &'static str {
        "peer-cap"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // First tick immediately — cache must be available on boot.
        tick_once(Arc::clone(&self.store), &self.cache_path).await;

        loop {
            tokio::select! {
                _ = shutdown.wait() => break,
                _ = tokio::time::sleep(self.interval) => {
                    tick_once(Arc::clone(&self.store), &self.cache_path).await;
                }
            }
        }
        Ok(())
    }
}

async fn tick_once(store: Arc<Mutex<rusqlite::Connection>>, cache_path: &PathBuf) {
    let cap_used = {
        let conn = store.lock().await;
        match list_nodes(&conn) {
            Ok(nodes) => count_cap_peers(&nodes),
            Err(e) => {
                tracing::warn!(error = %e, "peer-cap: list_nodes failed");
                return;
            }
        }
    };
    let snapshot = PeerCapSnapshot::from_count(cap_used);
    write_cache(cache_path, &snapshot).await;
    publish_cap(&snapshot).await;
}

async fn write_cache(path: &PathBuf, snapshot: &PeerCapSnapshot) {
    match serde_json::to_string(snapshot) {
        Ok(json) => {
            if let Err(e) = tokio::fs::write(path, json.as_bytes()).await {
                tracing::warn!(
                    error = %e,
                    path = %path.display(),
                    "peer-cap: cache write failed"
                );
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "peer-cap: snapshot serialization failed");
        }
    }
}

async fn publish_cap(snapshot: &PeerCapSnapshot) {
    let body = cap_payload(snapshot);
    let mut cmd = Command::new("mde-bus");
    cmd.arg("publish")
        .arg(CAP_TOPIC)
        .arg("--body-flag")
        .arg(&body);
    // EFF-20 — bound the publish so a wedged mde-bus can't leave this
    // future pending forever.
    match crate::workers::proc::status_with_timeout_async(
        cmd,
        crate::workers::proc::DEFAULT_CMD_TIMEOUT,
    )
    .await
    {
        Ok(s) if s.success() => {
            tracing::debug!(cap_used = snapshot.cap_used, "peer-cap published");
        }
        Ok(s) => {
            tracing::warn!(exit = ?s.code(), "peer-cap: mde-bus publish exited non-zero");
        }
        Err(e) => {
            tracing::warn!(error = %e, "peer-cap: mde-bus unavailable (graceful-degrade)");
        }
    }
}

/// Count nodes that consume a Q22 cap slot.
///
/// Only `role = 'peer'` counts. The `host` (local node), `observer`,
/// and `decommissioned` roles are excluded. Federated external-mesh
/// peers are never present in the local store and are therefore
/// naturally excluded — no separate federation check is needed.
#[must_use]
pub fn count_cap_peers(nodes: &[NodeRow]) -> u8 {
    let n = nodes.iter().filter(|r| r.role == "peer").count();
    n.min(usize::from(u8::MAX)) as u8
}

/// JSON body for the `mesh/peer-cap/updated` Bus topic.
///
/// Shape: `{"cap_used":<n>,"cap_limit":<n>,"cap_full":<bool>}`
#[must_use]
pub fn cap_payload(snapshot: &PeerCapSnapshot) -> String {
    format!(
        r#"{{"cap_used":{},"cap_limit":{},"cap_full":{}}}"#,
        snapshot.cap_used, snapshot.cap_limit, snapshot.cap_full
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(role: &str) -> NodeRow {
        NodeRow {
            node_id: format!("peer:{role}-test"),
            name: format!("{role}-node"),
            public_key: "testkey".into(),
            role: role.into(),
            health: "healthy".into(),
            region: None,
        }
    }

    #[test]
    fn count_cap_peers_counts_peer_role() {
        let nodes = vec![make_node("peer"), make_node("peer"), make_node("peer")];
        assert_eq!(count_cap_peers(&nodes), 3);
    }

    #[test]
    fn count_cap_peers_excludes_host() {
        // The local 'host' node doesn't consume a cap slot.
        let nodes = vec![make_node("host"), make_node("peer")];
        assert_eq!(count_cap_peers(&nodes), 1);
    }

    #[test]
    fn count_cap_peers_excludes_observer() {
        let nodes = vec![make_node("observer"), make_node("peer"), make_node("peer")];
        assert_eq!(count_cap_peers(&nodes), 2);
    }

    #[test]
    fn count_cap_peers_excludes_decommissioned() {
        let nodes = vec![make_node("decommissioned"), make_node("peer")];
        assert_eq!(count_cap_peers(&nodes), 1);
    }

    #[test]
    fn count_cap_peers_empty_store_is_zero() {
        assert_eq!(count_cap_peers(&[]), 0);
    }

    #[test]
    fn peer_cap_limit_is_8() {
        // Q22 lock — the constant must not drift.
        assert_eq!(PEER_CAP, 8);
    }

    #[test]
    fn snapshot_from_count_sets_fields_correctly() {
        let s = PeerCapSnapshot::from_count(5);
        assert_eq!(s.cap_used, 5);
        assert_eq!(s.cap_limit, 8);
        assert!(!s.cap_full);
        assert_eq!(s.remaining_slots(), 3);
    }

    #[test]
    fn snapshot_cap_full_at_limit() {
        let s = PeerCapSnapshot::from_count(8);
        assert!(s.cap_full);
        assert_eq!(s.remaining_slots(), 0);
    }

    #[test]
    fn snapshot_remaining_slots_saturates_at_zero_when_over_cap() {
        // Should not underflow if somehow cap_used > cap_limit.
        let mut s = PeerCapSnapshot::from_count(8);
        s.cap_used = 9;
        assert_eq!(s.remaining_slots(), 0);
    }

    #[test]
    fn cap_payload_format_is_correct() {
        let s = PeerCapSnapshot::from_count(3);
        let p = cap_payload(&s);
        assert_eq!(p, r#"{"cap_used":3,"cap_limit":8,"cap_full":false}"#);
    }

    #[test]
    fn cap_payload_cap_full_true_when_at_limit() {
        let s = PeerCapSnapshot::from_count(8);
        let p = cap_payload(&s);
        assert!(p.contains(r#""cap_full":true"#));
    }

    #[test]
    fn worker_name_is_peer_cap() {
        // Runtime-reachability: confirms the worker name the supervisor
        // registers matches the module name.
        use std::sync::Arc;
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let w = PeerCapWorker::new(
            Arc::new(tokio::sync::Mutex::new(conn)),
            std::env::temp_dir().join("peer-cap-name-test.json"),
        );
        assert_eq!(w.name(), "peer-cap");
    }
}
