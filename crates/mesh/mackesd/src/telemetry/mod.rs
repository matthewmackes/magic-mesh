//! Heartbeat + link telemetry (Phase 12.6.1 + 12.6.2).
//!
//! Per the 12.6.1 lock, every peer's `mackesd` writes health + agent
//! version + last-applied revision into its local
//! `observed_telemetry` table AND copies the row into
//! `~/QNM-Shared/<peer>/mackesd/heartbeat.json` (the shared
//! mesh-FS, the only "transport" without a networked API). The
//! Host's reconciler aggregates the per-peer files on its tick.
//!
//! Per 12.6.2, link telemetry (latency + packet loss + throughput
//! per peer-pair) lands at `~/QNM-Shared/<peer>/mackesd/links.json`
//! every 30 s. Aggregated per-link in `topology_link_health`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One heartbeat row, as written by a peer's `mackesd` into
/// `<peer>/mackesd/heartbeat.json` and ingested by the leader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heartbeat {
    /// Stable node id (matches `nodes.id` on the leader's side).
    pub node_id: String,
    /// Unix epoch milliseconds the peer's `mackesd` recorded this row.
    pub at_ms: i64,
    /// Agent version (Cargo package version of the writing `mackesd`).
    pub agent_version: String,
    /// Most recent applied revision id this peer has reconciled to,
    /// or `None` if no revision has applied yet.
    pub applied_revision: Option<String>,
    /// One of `healthy` / `degraded` / `unreachable`, per the
    /// 12.3.3 threshold table.
    pub health: HealthState,
}

/// Health-state tri-state. Stored as snake_case strings in JSON to
/// match the column the SQL store uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    /// Heartbeat lag under one cycle (10 s).
    Healthy,
    /// Heartbeat missed exactly one cycle.
    Degraded,
    /// Heartbeat missed 3+ cycles.
    Unreachable,
}

/// One link-telemetry row covering one peer's view of one other peer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkSample {
    /// The peer this sample was measured FROM.
    pub from_id: String,
    /// The peer it was measured TO.
    pub to_id: String,
    /// Round-trip time in milliseconds (median over the measurement
    /// window). `None` when the probe couldn't reach.
    pub rtt_ms: Option<u32>,
    /// Packet loss fraction `0.0..=1.0`. `None` when unmeasured.
    pub loss: Option<f32>,
    /// Throughput in Mbps. `None` when unmeasured.
    pub throughput_mbps: Option<f32>,
    /// Unix epoch milliseconds the row was sampled.
    pub at_ms: i64,
}

/// Compute the right `HealthState` for a given heartbeat-age in
/// milliseconds. Per 12.3.3: 1 missed cycle (10–20 s) = degraded;
/// 3+ missed (≥ 30 s) = unreachable.
#[must_use]
pub const fn health_state_from_age(age_ms: u64) -> HealthState {
    if age_ms >= 30_000 {
        HealthState::Unreachable
    } else if age_ms >= 10_000 {
        HealthState::Degraded
    } else {
        HealthState::Healthy
    }
}

/// Build the on-disk path a peer's heartbeat JSON lives at.
#[must_use]
pub fn heartbeat_path(workgroup_root: &Path, node_id: &str) -> PathBuf {
    workgroup_root
        .join(node_id)
        .join("mackesd")
        .join("heartbeat.json")
}

/// Build the on-disk path a peer's link-sample JSON lives at.
#[must_use]
pub fn links_path(workgroup_root: &Path, node_id: &str) -> PathBuf {
    workgroup_root
        .join(node_id)
        .join("mackesd")
        .join("links.json")
}

/// 12.3.3 heartbeat cadence. Locked at 10 s per the lock.
pub const HEARTBEAT_INTERVAL_S: u64 = 10;

/// Build the canonical "this peer is healthy right now" heartbeat
/// using the current process's agent version + an `applied_revision`
/// the caller supplies. Convenience wrapper around the struct
/// literal so worker code stays one line.
#[must_use]
pub fn build_heartbeat(node_id: &str, applied_revision: Option<&str>) -> Heartbeat {
    let at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    Heartbeat {
        node_id: node_id.to_owned(),
        at_ms,
        agent_version: env!("CARGO_PKG_VERSION").to_owned(),
        applied_revision: applied_revision.map(str::to_owned),
        health: HealthState::Healthy,
    }
}

/// Spawn a background thread that writes one heartbeat every
/// `interval` until `shutdown` flips true. Returns the join handle
/// so the caller can wait on shutdown.
///
/// `interval` is the operator-tunable cadence (E1.3 #3, sourced from
/// `/etc/mackesd/mackesd.toml`); pass
/// `Duration::from_secs(HEARTBEAT_INTERVAL_S)` for the locked default.
///
/// Used by the `mackesd` reconcile loop's bootstrap to keep the
/// peer's heartbeat fresh even while the rest of the reconciler is
/// processing a long-running deploy.
pub fn spawn_heartbeat_worker(
    workgroup_root: PathBuf,
    node_id: String,
    interval: std::time::Duration,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        use std::sync::atomic::Ordering;
        // PEERVER-2 — publish this peer's convergence record to the GFS
        // peers/ dir (read by mde-update / mde-install; mirrored into
        // nodes by PEERVER-4). Detect the mde-core RPM version once;
        // cap the write to ~once/min (§3.1 slow-state budget) rather
        // than every heartbeat. See docs/design/v2.7-peer-data-convergence.md.
        let peer_hostname = node_id
            .strip_prefix("peer:")
            .unwrap_or(&node_id)
            .to_string();
        let mde_version = detect_mde_core_version();
        let peers_dir = mackes_mesh_types::peers::peers_dir(&workgroup_root);
        let peer_write_min = std::time::Duration::from_secs(60);
        let mut last_peer_write: Option<std::time::Instant> = None;
        // Check the shutdown flag every 100 ms instead of sleeping the
        // full interval between checks — otherwise a shutdown request
        // mid-interval isn't honored until the next wake (up to the
        // full HEARTBEAT_INTERVAL_S), which both stretched the
        // supervisor's SIGTERM→exit latency and raced the worker
        // shutdown test (DEAD-FLAKY-HEARTBEAT, 2026-05-28). Chunked
        // sleep makes shutdown responsive within ~100 ms.
        const CHECK_CHUNK: std::time::Duration = std::time::Duration::from_millis(100);
        while !shutdown.load(Ordering::Relaxed) {
            let hb = build_heartbeat(&node_id, None);
            if let Err(e) = write_heartbeat(&workgroup_root, &hb) {
                eprintln!("heartbeat: write failed: {e}");
            }
            // PEERVER-2 — refresh the peer-convergence record at most
            // once/min (own-row authority: we are the sole writer of
            // our own <hostname>.json).
            let due = last_peer_write.map_or(true, |t| t.elapsed() >= peer_write_min);
            if due {
                // PD-2 — probe + publish the service descriptors on the
                // record-write cycle (L13: one cycle, one write); health
                // derives from the Netdata alarm tier (L15) instead of a
                // hardcoded "healthy".
                let descriptors = crate::descriptors::probe_local();
                let health = if descriptors.alarms.tier.is_empty() {
                    "healthy".to_string()
                } else {
                    descriptors.alarms.tier.clone()
                };
                let mut rec = mackes_mesh_types::peers::PeerRecord::now(
                    peer_hostname.clone(),
                    mde_version.clone(),
                    health,
                );
                rec.descriptors = Some(descriptors);
                match mackes_mesh_types::peers::write_peer_record(&peers_dir, &rec) {
                    Ok(_) => last_peer_write = Some(std::time::Instant::now()),
                    Err(e) => eprintln!("peer-record: write failed: {e}"),
                }
            }
            // Interruptible interval sleep.
            let mut slept = std::time::Duration::ZERO;
            while slept < interval && !shutdown.load(Ordering::Relaxed) {
                std::thread::sleep(CHECK_CHUNK);
                slept += CHECK_CHUNK;
            }
        }
    })
}

/// This node's installed `mde-core` RPM version (PEERVER-2), or
/// `None` when the package isn't installed / `rpm` is unavailable
/// (e.g. a dev checkout). Cheap: queried once per heartbeat-worker
/// spawn, not per tick.
#[must_use]
pub fn detect_mde_core_version() -> Option<String> {
    let out = std::process::Command::new("rpm")
        .args(["-q", "--qf", "%{VERSION}", "mde-core"])
        .output()
        .ok()?;
    if out.status.success() {
        let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!v.is_empty()).then_some(v)
    } else {
        None
    }
}

/// Atomic write of a heartbeat row to disk. Writes via a `.tmp`
/// sibling and renames into place so a reading aggregator never
/// sees a partial file.
///
/// # Errors
/// Returns `std::io::Error` when the parent directory isn't
/// writable or the rename fails.
pub fn write_heartbeat(workgroup_root: &Path, hb: &Heartbeat) -> std::io::Result<PathBuf> {
    let path = heartbeat_path(workgroup_root, &hb.node_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(hb)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Atomic write of a link-sample batch to disk. Same pattern.
///
/// # Errors
/// Returns `std::io::Error` when the parent directory isn't
/// writable or the rename fails.
pub fn write_links(
    workgroup_root: &Path,
    node_id: &str,
    samples: &[LinkSample],
) -> std::io::Result<PathBuf> {
    let path = links_path(workgroup_root, node_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(samples)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_state_thresholds_match_lock() {
        assert_eq!(health_state_from_age(0), HealthState::Healthy);
        assert_eq!(health_state_from_age(5_000), HealthState::Healthy);
        assert_eq!(health_state_from_age(10_000), HealthState::Degraded);
        assert_eq!(health_state_from_age(20_000), HealthState::Degraded);
        assert_eq!(health_state_from_age(30_000), HealthState::Unreachable);
        assert_eq!(health_state_from_age(120_000), HealthState::Unreachable);
    }

    #[test]
    fn heartbeat_path_shape() {
        let p = heartbeat_path(Path::new("/tmp/qnm"), "peer:anvil");
        assert!(p.ends_with("peer:anvil/mackesd/heartbeat.json"));
    }

    #[test]
    fn write_heartbeat_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let hb = Heartbeat {
            node_id: "peer:a".into(),
            at_ms: 1_234_567,
            agent_version: "1.1.0".into(),
            applied_revision: Some("r-2026-05-19-0001".into()),
            health: HealthState::Healthy,
        };
        let path = write_heartbeat(dir.path(), &hb).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let back: Heartbeat = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back, hb);
    }

    #[test]
    fn write_links_round_trips_a_batch() {
        let dir = tempfile::tempdir().unwrap();
        let samples = vec![
            LinkSample {
                from_id: "peer:a".into(),
                to_id: "peer:b".into(),
                rtt_ms: Some(12),
                loss: Some(0.0),
                throughput_mbps: Some(950.0),
                at_ms: 1,
            },
            LinkSample {
                from_id: "peer:a".into(),
                to_id: "peer:c".into(),
                rtt_ms: None,
                loss: None,
                throughput_mbps: None,
                at_ms: 1,
            },
        ];
        let path = write_links(dir.path(), "peer:a", &samples).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let back: Vec<LinkSample> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].rtt_ms, Some(12));
        assert!(back[1].rtt_ms.is_none());
    }

    #[test]
    fn json_round_trips_through_serde() {
        let hb = Heartbeat {
            node_id: "peer:x".into(),
            at_ms: 0,
            agent_version: "1.1.0".into(),
            applied_revision: None,
            health: HealthState::Unreachable,
        };
        let json = serde_json::to_string(&hb).unwrap();
        let back: Heartbeat = serde_json::from_str(&json).unwrap();
        assert_eq!(back, hb);
    }

    #[test]
    fn build_heartbeat_uses_cargo_version() {
        let hb = build_heartbeat("peer:test", None);
        assert_eq!(hb.agent_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(hb.node_id, "peer:test");
        assert_eq!(hb.health, HealthState::Healthy);
        assert!(hb.applied_revision.is_none());
    }

    #[test]
    fn build_heartbeat_carries_applied_revision_when_set() {
        let hb = build_heartbeat("peer:test", Some("r-2026-05-19-0042"));
        assert_eq!(hb.applied_revision.as_deref(), Some("r-2026-05-19-0042"));
    }

    #[test]
    fn heartbeat_worker_exits_on_shutdown_flag() {
        let dir = tempfile::tempdir().unwrap();
        let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let h = spawn_heartbeat_worker(
            dir.path().to_path_buf(),
            "peer:test".into(),
            std::time::Duration::from_secs(HEARTBEAT_INTERVAL_S),
            std::sync::Arc::clone(&shutdown),
        );
        // The worker writes the heartbeat at the start of its first loop
        // iteration. POLL for it rather than assuming a fixed 100 ms tick —
        // under heavy parallel test load the worker thread can take far longer
        // than 100 ms to be scheduled, which made the old fixed-sleep assertion
        // flaky (it raced the very first write). Bound the wait to 5 s.
        let path = heartbeat_path(dir.path(), "peer:test");
        let step = std::time::Duration::from_millis(20);
        let mut waited = std::time::Duration::ZERO;
        while !path.exists() && waited < std::time::Duration::from_secs(5) {
            std::thread::sleep(step);
            waited += step;
        }
        assert!(
            path.exists(),
            "expected {path:?} to exist after one tick (waited {waited:?})"
        );
        // Flip shutdown; the chunked-sleep loop honors it within ~100 ms, so
        // join() returns promptly (no full-interval wait).
        shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = h.join();
    }
}
