//! PLANES-19 (W79/W80) — the overlay-reachability validation suite mount.
//!
//! Drives [`magic_fleet::validation`] at runtime. Two jobs on each tick:
//!
//! 1. **Participate.** For every run that names THIS node and lacks its
//!    row, probe every *other* participant over the overlay (reusing
//!    [`crate::transport_probe`] — a TCP handshake through the tunnel, the
//!    same primitive `mesh_latency` uses) and write the node's own row
//!    (own-row authority, W79).
//! 2. **Lead (leader only).** Mint a **nightly** run (once per ~24 h) and
//!    pick up a **Run-now** nudge (`validation/runnow`), then aggregate the
//!    reported rows of finished runs into a `verdict.json` so a partition
//!    surfaces as a fleet fact. Failed edges are logged loudly and recorded
//!    in the verdict (the W80 drift feed reads this verdict; full
//!    drift-row injection rides the reconcile tick that owns the drift
//!    store).
//!
//! The leader proxy is the role-host marker file's existence, the same
//! proxy `netdata_aggregator` / `nebula_supervisor` use.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use magic_fleet::validation::{
    aggregate, list_run_ids, read_rows, row_pending_for, write_row, NodeReachability, PeerReach,
    ValidationRun,
};

use super::{ShutdownToken, Worker};

/// Poll cadence — a Run-now nudge lands within this.
pub const POLL: Duration = Duration::from_secs(20);

/// Nightly cadence (the leader mints at most this often).
pub const NIGHTLY: Duration = Duration::from_secs(24 * 3600);

/// Reachability probe — isolates the network shell-out so the worker's
/// orchestration is unit-tested with no overlay.
pub trait Reachability {
    /// Probe an overlay IP; return `(reachable, rtt_ms)`.
    fn probe(&self, overlay_ip: &str) -> (bool, Option<f64>);
}

/// The real probe over the Nebula transport.
pub struct SystemReach;
impl Reachability for SystemReach {
    fn probe(&self, overlay_ip: &str) -> (bool, Option<f64>) {
        let r = crate::transport_probe::probe_rtt(overlay_ip);
        (r.reachable, r.rtt_ms)
    }
}

/// The validation-suite worker.
pub struct ValidationSuiteWorker {
    workgroup_root: PathBuf,
    store_db: Option<PathBuf>,
    hostname: String,
    role_marker_path: PathBuf,
}

impl ValidationSuiteWorker {
    #[must_use]
    pub fn new(
        workgroup_root: PathBuf,
        store_db: Option<PathBuf>,
        hostname: String,
        role_marker_path: PathBuf,
    ) -> Self {
        Self {
            workgroup_root,
            store_db,
            hostname,
            role_marker_path,
        }
    }

    /// (hostname → overlay_ip) from the roster mirror.
    fn roster(&self) -> std::collections::BTreeMap<String, String> {
        self.store_db
            .as_ref()
            .and_then(|db| {
                rusqlite::Connection::open_with_flags(
                    db,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .ok()
            })
            .and_then(|conn| crate::nebula_roster::export_roster(&conn).ok())
            .map(|rows| rows.into_iter().map(|r| (r.name, r.overlay_ip)).collect())
            .unwrap_or_default()
    }

    fn am_leader(&self) -> bool {
        self.role_marker_path.exists()
    }

    /// Probe every other participant of `run` and write this node's row.
    /// `at` is the report timestamp (Unix seconds).
    fn participate(&self, run: &ValidationRun, reach: &dyn Reachability, at: u64) {
        let roster = self.roster();
        let results: Vec<PeerReach> = run
            .participants
            .iter()
            .filter(|p| **p != self.hostname)
            .map(|peer| {
                let overlay_ip = roster.get(peer).cloned().unwrap_or_default();
                let (reachable, rtt_ms) = if overlay_ip.is_empty() {
                    (false, None) // no overlay IP known → can't reach
                } else {
                    reach.probe(&overlay_ip)
                };
                PeerReach {
                    peer: peer.clone(),
                    overlay_ip,
                    reachable,
                    rtt_ms,
                }
            })
            .collect();
        let row = NodeReachability {
            from: self.hostname.clone(),
            at,
            results,
        };
        if let Err(e) = write_row(&self.workgroup_root, &run.run_id, &row) {
            tracing::warn!(run = %run.run_id, error = %e, "validation_suite: row write failed");
        } else {
            tracing::info!(
                run = %run.run_id, probed = row.results.len(),
                "validation_suite: reported overlay reachability (PLANES-19)"
            );
        }
    }

    /// Leader-side aggregation: for each run, write the current verdict and
    /// log any failed edges (W80). Idempotent — re-writing the same verdict
    /// is harmless.
    fn write_verdicts(&self) {
        for id in list_run_ids(&self.workgroup_root) {
            let Some(run) = magic_fleet::validation::read_run(&self.workgroup_root, &id) else {
                continue;
            };
            let rows = read_rows(&self.workgroup_root, &id);
            let agg = aggregate(&run, &rows);
            let verdict = serde_json::json!({
                "run_id": id,
                "passed": agg.passed(),
                "reachable_edges": agg.reachable.len(),
                "failed_edges": agg.failed.iter()
                    .map(|e| format!("{}->{}", e.from, e.to)).collect::<Vec<_>>(),
                "missing_reporters": agg.missing_reporters.iter().cloned().collect::<Vec<_>>(),
            });
            let path =
                magic_fleet::validation::run_dir(&self.workgroup_root, &id).join("verdict.json");
            let _ = std::fs::write(
                &path,
                serde_json::to_string_pretty(&verdict).unwrap_or_default(),
            );
            if !agg.passed() && agg.missing_reporters.is_empty() {
                tracing::warn!(
                    run = %id, failed = agg.failed.len(),
                    "validation_suite: overlay reachability FAILED — drift (W80)"
                );
            }
        }
    }

    /// One pass: participate in pending runs, then (leader) mint nightly /
    /// pick up Run-now and write verdicts. `now` is injected for tests.
    fn tick_with(&self, reach: &dyn Reachability, now: u64) {
        for id in list_run_ids(&self.workgroup_root) {
            if let Some(run) = magic_fleet::validation::read_run(&self.workgroup_root, &id) {
                if row_pending_for(&self.workgroup_root, &run, &self.hostname) {
                    self.participate(&run, reach, now);
                }
            }
        }
        if self.am_leader() {
            self.lead(now);
            self.write_verdicts();
        }
    }

    /// Leader minting: a Run-now nudge mints immediately; otherwise a
    /// nightly run is minted when >`NIGHTLY` has elapsed since the last.
    fn lead(&self, now: u64) {
        let runnow = self.workgroup_root.join("validation").join("runnow");
        if runnow.exists() {
            let _ = std::fs::remove_file(&runnow);
            self.mint(magic_fleet::validation::RunKind::RunNow, now);
            return;
        }
        let stamp = self.workgroup_root.join("validation").join(".last-nightly");
        let last: u64 = std::fs::read_to_string(&stamp)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        if now.saturating_sub(last) >= NIGHTLY.as_secs()
            && self
                .mint(magic_fleet::validation::RunKind::Nightly, now)
                .is_some()
        {
            // The run dir mint already created `validation/`; stamp beside it.
            if let Some(parent) = stamp.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&stamp, now.to_string());
        }
    }

    /// Mint a run with the current roster as participants. Returns `None`
    /// when there are fewer than two participants (nothing to validate).
    fn mint(&self, kind: magic_fleet::validation::RunKind, now: u64) -> Option<()> {
        let mut participants: Vec<String> = self.roster().into_keys().collect();
        if !participants.contains(&self.hostname) {
            participants.push(self.hostname.clone());
        }
        participants.sort();
        participants.dedup();
        if participants.len() < 2 {
            return None;
        }
        let run = ValidationRun {
            run_id: format!("v-{now}"),
            kind,
            launched_by: format!("peer:{}", self.hostname),
            at: now,
            participants,
        };
        magic_fleet::validation::write_run(&self.workgroup_root, &run)
            .ok()
            .map(|_| ())
    }
}

fn now_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[async_trait::async_trait]
impl Worker for ValidationSuiteWorker {
    fn name(&self) -> &'static str {
        "validation_suite"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            let this = ValidationSuiteWorker {
                workgroup_root: self.workgroup_root.clone(),
                store_db: self.store_db.clone(),
                hostname: self.hostname.clone(),
                role_marker_path: self.role_marker_path.clone(),
            };
            // transport_probe is blocking; keep the sweep off the scheduler.
            let _ =
                tokio::task::spawn_blocking(move || this.tick_with(&SystemReach, now_secs())).await;
            tokio::select! {
                _ = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(POLL) => {}
            }
        }
    }
}

/// Read a run's persisted verdict (`passed`), if the leader has written
/// one — the panel / `meshctl test connectivity` reads this.
#[must_use]
pub fn read_verdict_passed(root: &Path, run_id: &str) -> Option<bool> {
    let raw = std::fs::read_to_string(
        magic_fleet::validation::run_dir(root, run_id).join("verdict.json"),
    )
    .ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("passed").and_then(serde_json::Value::as_bool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use magic_fleet::validation::{write_run, RunKind};

    struct MockReach {
        reachable: bool,
    }
    impl Reachability for MockReach {
        fn probe(&self, _ip: &str) -> (bool, Option<f64>) {
            (self.reachable, self.reachable.then_some(7.0))
        }
    }

    fn worker(root: &Path, host: &str, leader_marker: &Path) -> ValidationSuiteWorker {
        ValidationSuiteWorker::new(
            root.to_path_buf(),
            None, // no store_db → empty roster; participate uses run.participants
            host.into(),
            leader_marker.to_path_buf(),
        )
    }

    fn seed_run(root: &Path, parts: &[&str]) {
        write_run(
            root,
            &ValidationRun {
                run_id: "v-1".into(),
                kind: RunKind::RunNow,
                launched_by: "peer:oak".into(),
                at: 100,
                participants: parts.iter().map(|s| (*s).to_string()).collect(),
            },
        )
        .unwrap();
    }

    #[test]
    fn participates_and_writes_its_own_row() {
        let tmp = tempfile::tempdir().unwrap();
        seed_run(tmp.path(), &["pine", "oak"]);
        // not leader (marker absent)
        let nomarker = tmp.path().join("not-a-host-marker");
        let w = worker(tmp.path(), "pine", &nomarker);
        w.tick_with(&MockReach { reachable: true }, 200);
        let rows = read_rows(tmp.path(), "v-1");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].from, "pine");
        // oak has no overlay IP (no store_db) → recorded unreachable, but
        // the row IS written (the participation contract).
        assert_eq!(rows[0].results.len(), 1);
        assert_eq!(rows[0].results[0].peer, "oak");
        // Second pass is idempotent — already reported, no duplicate work.
        w.tick_with(&MockReach { reachable: true }, 250);
        assert_eq!(read_rows(tmp.path(), "v-1").len(), 1);
    }

    #[test]
    fn leader_writes_a_verdict_and_flags_failure() {
        let tmp = tempfile::tempdir().unwrap();
        seed_run(tmp.path(), &["pine", "oak"]);
        // Both rows present, one edge failed.
        write_row(
            tmp.path(),
            "v-1",
            &NodeReachability {
                from: "pine".into(),
                at: 200,
                results: vec![PeerReach {
                    peer: "oak".into(),
                    overlay_ip: "10.42.0.3".into(),
                    reachable: false,
                    rtt_ms: None,
                }],
            },
        )
        .unwrap();
        write_row(
            tmp.path(),
            "v-1",
            &NodeReachability {
                from: "oak".into(),
                at: 200,
                results: vec![PeerReach {
                    peer: "pine".into(),
                    overlay_ip: "10.42.0.2".into(),
                    reachable: true,
                    rtt_ms: Some(5.0),
                }],
            },
        )
        .unwrap();
        // Leader marker present.
        let marker = tmp.path().join("role-host");
        std::fs::write(&marker, "host").unwrap();
        let w = worker(tmp.path(), "pine", &marker);
        w.tick_with(&MockReach { reachable: true }, 300);
        // Verdict written, and it's a failure (pine->oak unreachable).
        assert_eq!(read_verdict_passed(tmp.path(), "v-1"), Some(false));
    }

    #[test]
    fn non_leader_mints_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let nomarker = tmp.path().join("absent");
        let w = worker(tmp.path(), "pine", &nomarker);
        w.tick_with(&MockReach { reachable: true }, 1000);
        assert!(list_run_ids(tmp.path()).is_empty(), "no leader, no mint");
    }
}
