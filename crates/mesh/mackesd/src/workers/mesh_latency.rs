//! v4.0.1 AF-NET-2 (2026-05-23) — peer-mesh latency sniffer.
//!
//! Periodically pings every enrolled non-local peer and writes
//! the result to `~/.cache/mde/mesh-latency.json`. The
//! WB-2.k.a Cairo topology canvas + the panel Mesh-status
//! tray badge both read this cache to vary edge thickness and
//! the badge "degraded" indicator.
//!
//! Best-choice deviation from the spec's "via the chosen
//! Transport from KDC2-4.x" wording: TransportRegistry
//! concrete impls are still blocked on the KDC2 pairing-
//! handshake epic; using `ping` directly hits the same wire
//! (ICMP) the underlying Transport would, with zero
//! additional Cargo deps and an observable outcome
//! indistinguishable from the routed version. When the
//! Transport stack lands, swap the sync `ping` call for the
//! Transport's `probe()` and delete the shell-out.
//!
//! Cadence: 30 s between full sweeps. Per-peer ping timeout
//! 1 s. The cache file is written as a single JSON object:
//!
//! ```json
//! {
//!   "checked_at": 1716499200,
//!   "peers": {
//!     "anvil": { "rtt_ms": 14.3, "ok": true },
//!     "forge": { "rtt_ms": null, "ok": false }
//!   }
//! }
//! ```

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::{ShutdownToken, Worker};
use crate::store::{list_nodes, NodeRow};

/// Default sweep cadence — 30 s between full peer scans.
pub const DEFAULT_SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// Per-peer ping deadline. ping(8) `-W` accepts seconds.
pub const PING_TIMEOUT_SECS: u32 = 1;

/// One peer's measured latency for a single ping pass.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PeerLatency {
    /// Round-trip latency in milliseconds. `None` when the
    /// ping timed out or the host didn't resolve.
    pub rtt_ms: Option<f64>,
    /// Convenience flag — `true` iff `rtt_ms.is_some()`.
    pub ok: bool,
}

/// One pass of the mesh-latency worker — every peer measured
/// at the same wall-clock instant. Serialized to
/// `~/.cache/mackes/mesh-latency.json` for the panel UI to
/// render + for QNM-Shared drift detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencySnapshot {
    /// Unix-epoch seconds when the snapshot was written.
    pub checked_at: i64,
    /// Map of peer name → latency. BTreeMap so the JSON
    /// serialization is deterministic (helps QNM-Shared
    /// drift detection avoid spurious diffs).
    pub peers: BTreeMap<String, PeerLatency>,
}

/// AF-NET-2 worker. Loops every `interval` ticks, pings each
/// non-local non-decommissioned peer, writes the snapshot.
pub struct MeshLatencyWorker {
    store: Arc<Mutex<rusqlite::Connection>>,
    local_node_id: String,
    cache_path: PathBuf,
    interval: Duration,
}

impl MeshLatencyWorker {
    /// Construct a new worker. `cache_path` is normally the
    /// XDG cache `~/.cache/mde/mesh-latency.json`; the
    /// constructor takes it as a path so tests can point at
    /// a tempdir.
    #[must_use]
    pub fn new(
        store: Arc<Mutex<rusqlite::Connection>>,
        local_node_id: String,
        cache_path: PathBuf,
    ) -> Self {
        Self {
            store,
            local_node_id,
            cache_path,
            interval: DEFAULT_SWEEP_INTERVAL,
        }
    }

    /// Override the sweep interval for tests / fast-cadence
    /// debugging. Production callers should stick with the
    /// 30 s default.
    #[must_use]
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }
}

#[async_trait::async_trait]
impl Worker for MeshLatencyWorker {
    fn name(&self) -> &'static str {
        "mesh-latency"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // Run one sweep immediately so the cache file lands on
        // boot (operators reading the cache on a fresh login
        // see something other than "file not found").
        let _ = sweep_once(
            Arc::clone(&self.store),
            &self.local_node_id,
            &self.cache_path,
        )
        .await;

        loop {
            tokio::select! {
                _ = shutdown.wait() => break,
                _ = tokio::time::sleep(self.interval) => {
                    let _ = sweep_once(
                        Arc::clone(&self.store),
                        &self.local_node_id,
                        &self.cache_path,
                    ).await;
                }
            }
        }
        Ok(())
    }
}

/// One full sweep. Reads the live nodes table, pings each
/// non-local peer in parallel (with a per-peer 1 s deadline),
/// writes the snapshot.
async fn sweep_once(
    store: Arc<Mutex<rusqlite::Connection>>,
    local_node_id: &str,
    cache_path: &PathBuf,
) -> anyhow::Result<()> {
    let nodes: Vec<NodeRow> = {
        let conn = store.lock().await;
        list_nodes(&conn).map_err(|e| anyhow::anyhow!("list_nodes: {e}"))?
    };
    let targets: Vec<String> = nodes
        .into_iter()
        .filter(|n| n.node_id != local_node_id && n.role != "decommissioned")
        .map(|n| n.name)
        .collect();

    let mut handles = Vec::with_capacity(targets.len());
    for name in targets {
        handles.push(tokio::spawn(async move {
            let rtt = tokio::task::spawn_blocking({
                let n = name.clone();
                move || ping_host(&n)
            })
            .await
            .ok()
            .flatten();
            (name, rtt)
        }));
    }

    let mut peers = BTreeMap::new();
    for h in handles {
        if let Ok((name, rtt)) = h.await {
            peers.insert(
                name,
                PeerLatency {
                    ok: rtt.is_some(),
                    rtt_ms: rtt,
                },
            );
        }
    }

    let snapshot = LatencySnapshot {
        checked_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        peers,
    };
    write_snapshot(cache_path, &snapshot)?;
    tracing::debug!(
        peer_count = snapshot.peers.len(),
        cache = %cache_path.display(),
        "mesh-latency: snapshot written"
    );
    Ok(())
}

fn ping_host(host: &str) -> Option<f64> {
    if host.is_empty() {
        return None;
    }
    let timeout = PING_TIMEOUT_SECS.to_string();
    let output = std::process::Command::new("ping")
        .args(["-c", "1", "-W", &timeout, host])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8(output.stdout).ok()?;
    parse_ping_rtt(&raw)
}

/// Pure parser for ping(8) output. Extracts the first
/// `time=NN.N ms` token (single-shot `ping -c 1` always emits
/// at most one). Returns `None` if no time= is present (host
/// unreachable, parse failed, etc.).
#[must_use]
pub fn parse_ping_rtt(raw: &str) -> Option<f64> {
    for line in raw.lines() {
        if let Some(idx) = line.find("time=") {
            let rest = &line[idx + 5..];
            let val: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(n) = val.parse::<f64>() {
                return Some(n);
            }
        }
    }
    None
}

fn write_snapshot(cache_path: &PathBuf, snapshot: &LatencySnapshot) -> anyhow::Result<()> {
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| anyhow::anyhow!("mkdir cache: {e}"))?;
    }
    let raw = serde_json::to_string_pretty(snapshot)
        .map_err(|e| anyhow::anyhow!("serialize snapshot: {e}"))?;
    std::fs::write(cache_path, raw).map_err(|e| anyhow::anyhow!("write cache: {e}"))?;
    Ok(())
}

/// Resolve the default cache path —
/// `$XDG_CACHE_HOME/mde/mesh-latency.json`, falling back to
/// `$HOME/.cache/mde/mesh-latency.json`.
#[must_use]
pub fn default_cache_path() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").unwrap_or_default();
            PathBuf::from(home).join(".cache")
        });
    base.join("mde").join("mesh-latency.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ping_rtt_extracts_first_time_token() {
        let raw = "PING anvil (10.0.0.5) 56(84) bytes of data.\n\
                   64 bytes from 10.0.0.5: icmp_seq=1 ttl=64 time=14.3 ms\n\
                   \n\
                   --- anvil ping statistics ---\n";
        assert_eq!(parse_ping_rtt(raw), Some(14.3));
    }

    #[test]
    fn parse_ping_rtt_handles_integer_rtt() {
        let raw = "64 bytes from 10.0.0.5: icmp_seq=1 ttl=64 time=42 ms\n";
        assert_eq!(parse_ping_rtt(raw), Some(42.0));
    }

    #[test]
    fn parse_ping_rtt_returns_none_when_missing() {
        assert_eq!(parse_ping_rtt(""), None);
        assert_eq!(
            parse_ping_rtt("ping: anvil: Name or service not known"),
            None
        );
    }

    #[test]
    fn parse_ping_rtt_handles_sub_ms_rtt() {
        // localhost-style sub-millisecond RTTs.
        let raw = "64 bytes from 127.0.0.1: icmp_seq=1 ttl=64 time=0.045 ms\n";
        assert_eq!(parse_ping_rtt(raw), Some(0.045));
    }

    #[test]
    fn ping_host_with_empty_target_returns_none() {
        assert_eq!(ping_host(""), None);
    }

    #[test]
    fn default_cache_path_uses_xdg_when_set() {
        std::env::set_var("XDG_CACHE_HOME", "/tmp/xdg-cache-test");
        let p = default_cache_path();
        assert_eq!(
            p,
            PathBuf::from("/tmp/xdg-cache-test/mde/mesh-latency.json")
        );
        std::env::remove_var("XDG_CACHE_HOME");
    }

    #[test]
    fn write_snapshot_round_trips_through_json() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("snap.json");
        let mut peers = BTreeMap::new();
        peers.insert(
            "anvil".to_string(),
            PeerLatency {
                rtt_ms: Some(12.5),
                ok: true,
            },
        );
        peers.insert(
            "forge".to_string(),
            PeerLatency {
                rtt_ms: None,
                ok: false,
            },
        );
        let snap = LatencySnapshot {
            checked_at: 1_716_499_200,
            peers,
        };
        write_snapshot(&path, &snap).expect("write");
        let raw = std::fs::read_to_string(&path).expect("read back");
        let parsed: LatencySnapshot = serde_json::from_str(&raw).expect("parse back");
        assert_eq!(parsed.checked_at, 1_716_499_200);
        assert_eq!(parsed.peers.len(), 2);
        assert_eq!(parsed.peers["anvil"].rtt_ms, Some(12.5));
        assert!(!parsed.peers["forge"].ok);
    }

    #[tokio::test]
    async fn worker_name_matches_phase_b_lock() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db = tmp.path().join("nodes.sqlite");
        let conn = crate::store::open(&db).expect("open store");
        let w = MeshLatencyWorker::new(
            Arc::new(Mutex::new(conn)),
            "peer:test".to_owned(),
            tmp.path().join("mesh-latency.json"),
        );
        assert_eq!(w.name(), "mesh-latency");
    }

    #[tokio::test]
    async fn worker_exits_on_shutdown_token() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db = tmp.path().join("nodes.sqlite");
        let conn = crate::store::open(&db).expect("open store");
        let mut w = MeshLatencyWorker::new(
            Arc::new(Mutex::new(conn)),
            "peer:test".to_owned(),
            tmp.path().join("mesh-latency.json"),
        )
        .with_interval(Duration::from_millis(50));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let _ = tx.send(true);
        let result = tokio::time::timeout(Duration::from_secs(3), w.run(token))
            .await
            .expect("worker must exit on shutdown");
        assert!(result.is_ok());
    }
}
