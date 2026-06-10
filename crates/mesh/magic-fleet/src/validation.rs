//! PLANES-19 (W79/W80) — the overlay-reachability validation suite.
//!
//! "Is the mesh actually connected end-to-end?" answered as fleet state,
//! not a one-shot CLI: a **validation run** is minted (post-apply, by the
//! leader nightly, or on a Run-now nudge), every participating node
//! probes every *other* participant over the overlay and writes its **own
//! row** (own-row authority, the FPG-2 replicated-store pattern), and the
//! union of rows is a directed reachability matrix. A directed edge that
//! never came back reachable is a **failure**, and failures feed the
//! drift loop (W80) so a partitioned peer surfaces the same way a config
//! drift does.
//!
//! This module is the model + the replicated store + the aggregation
//! (matrix + failed-edge extraction). The probing itself reuses
//! `mackesd`'s `transport_probe` (a TCP handshake through the tunnel) in
//! the worker that drives this; nothing here shells out, so the
//! aggregation is fully unit-tested.
//!
//! Absorbs **ENT-10** ("test connectivity"): the same run, minted on
//! demand, is the operator's connectivity check.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Why a validation run was minted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    /// Fired right after a config / netstate apply converged.
    PostApply,
    /// The leader's scheduled nightly sweep.
    Nightly,
    /// An operator pressed "Run now".
    RunNow,
}

/// A validation run manifest (`validation/runs/<run-id>/run.json`): who
/// should report and why, with the participant list resolved at mint time
/// (the directory snapshot — so a late joiner doesn't retroactively fail
/// an older run).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationRun {
    /// Stable run id (the run dir name).
    pub run_id: String,
    /// Why this run exists.
    pub kind: RunKind,
    /// Minting node (advisory).
    #[serde(default)]
    pub launched_by: String,
    /// Mint time, Unix seconds.
    pub at: u64,
    /// Hostnames expected to report a row. Every directed edge between two
    /// participants is expected reachable.
    pub participants: Vec<String>,
}

/// One peer's reachability as seen FROM the reporting node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeerReach {
    /// The probed peer's hostname.
    pub peer: String,
    /// Its overlay IP at probe time.
    pub overlay_ip: String,
    /// Did the peer's stack answer through the tunnel?
    pub reachable: bool,
    /// Measured overlay RTT (ms), `None` when unreachable.
    #[serde(default)]
    pub rtt_ms: Option<f64>,
}

/// One node's row in a run (`validation/runs/<run-id>/<host>.json`): what
/// this node could reach. Own-row authority — a node only ever writes its
/// own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeReachability {
    /// The reporting node's hostname.
    pub from: String,
    /// Report time, Unix seconds.
    pub at: u64,
    /// One entry per other participant this node probed.
    pub results: Vec<PeerReach>,
}

/// The runs directory.
#[must_use]
pub fn runs_dir(root: &Path) -> PathBuf {
    root.join("validation").join("runs")
}

/// One run's directory.
#[must_use]
pub fn run_dir(root: &Path, run_id: &str) -> PathBuf {
    runs_dir(root).join(run_id)
}

/// Write a run manifest (atomic).
///
/// # Errors
/// IO / serialization failures.
pub fn write_run(root: &Path, run: &ValidationRun) -> io::Result<PathBuf> {
    let dir = run_dir(root, &run.run_id);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("run.json");
    let tmp = dir.join(".run.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(run)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Read a run manifest if present.
#[must_use]
pub fn read_run(root: &Path, run_id: &str) -> Option<ValidationRun> {
    let raw = std::fs::read_to_string(run_dir(root, run_id).join("run.json")).ok()?;
    serde_json::from_str(&raw).ok()
}

/// List every run id present (unsorted-tolerant; sorted on return).
#[must_use]
pub fn list_run_ids(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(runs_dir(root)) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .collect();
    ids.sort();
    ids
}

/// Write this node's reachability row (own-row authority, atomic).
///
/// # Errors
/// IO / serialization failures.
pub fn write_row(root: &Path, run_id: &str, row: &NodeReachability) -> io::Result<()> {
    let dir = run_dir(root, run_id);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", row.from));
    let tmp = dir.join(format!(".{}.json.tmp", row.from));
    std::fs::write(&tmp, serde_json::to_string_pretty(row)?)?;
    std::fs::rename(&tmp, &path)
}

/// Read every reported row for a run (junk-tolerant, sorted by `from`).
#[must_use]
pub fn read_rows(root: &Path, run_id: &str) -> Vec<NodeReachability> {
    let Ok(entries) = std::fs::read_dir(run_dir(root, run_id)) else {
        return Vec::new();
    };
    let mut out: Vec<NodeReachability> = entries
        .filter_map(Result::ok)
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.ends_with(".json") && n != "run.json"
        })
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|raw| serde_json::from_str(&raw).ok())
        .collect();
    out.sort_by(|a, b| a.from.cmp(&b.from));
    out
}

/// Whether THIS node still owes a row for `run` (it's a participant and
/// hasn't written its row yet) — the worker's "is there work for me".
#[must_use]
pub fn row_pending_for(root: &Path, run: &ValidationRun, self_hostname: &str) -> bool {
    run.participants.iter().any(|p| p == self_hostname)
        && !run_dir(root, &run.run_id)
            .join(format!("{self_hostname}.json"))
            .exists()
}

/// A single directed reachability edge `from → to`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Edge {
    /// Reporting node.
    pub from: String,
    /// Probed node.
    pub to: String,
}

/// The aggregated verdict for a run: which directed edges were reachable,
/// which failed, and which are still missing a report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FleetReachability {
    /// Directed edges confirmed reachable.
    pub reachable: BTreeSet<Edge>,
    /// Directed edges a reporting node tried and could NOT reach (W80 —
    /// these become drift).
    pub failed: BTreeSet<Edge>,
    /// Participants who have not yet reported a row at all.
    pub missing_reporters: BTreeSet<String>,
}

impl FleetReachability {
    /// The run is fully healthy: every participant reported and every
    /// directed edge between participants is reachable.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.failed.is_empty() && self.missing_reporters.is_empty()
    }
}

/// Aggregate the reported rows against the run's participant list into the
/// directed reachability verdict.
///
/// A `from → to` edge is **failed** if the reporter said unreachable, OR
/// if the reporter reported but never mentioned a `to` it was supposed to
/// reach (a silent omission is treated as a failure, not a pass —
/// validation never assumes reachability it didn't observe). Edges from a
/// non-reporting participant aren't counted as failed (the node itself is
/// in `missing_reporters` instead, so we don't double-penalise a node
/// that's simply offline).
#[must_use]
pub fn aggregate(run: &ValidationRun, rows: &[NodeReachability]) -> FleetReachability {
    let participants: BTreeSet<&str> = run.participants.iter().map(String::as_str).collect();
    let reported: BTreeMap<&str, &NodeReachability> =
        rows.iter().map(|r| (r.from.as_str(), r)).collect();

    let mut out = FleetReachability::default();
    for p in &run.participants {
        if !reported.contains_key(p.as_str()) {
            out.missing_reporters.insert(p.clone());
        }
    }

    for (from, row) in &reported {
        // Only consider edges to other declared participants.
        let seen: BTreeMap<&str, &PeerReach> =
            row.results.iter().map(|r| (r.peer.as_str(), r)).collect();
        for to in &participants {
            if to == from {
                continue;
            }
            let edge = Edge {
                from: (*from).to_string(),
                to: (*to).to_string(),
            };
            match seen.get(to) {
                Some(pr) if pr.reachable => {
                    out.reachable.insert(edge);
                }
                // probed-and-unreachable OR not-probed-at-all → failure.
                _ => {
                    out.failed.insert(edge);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(participants: &[&str]) -> ValidationRun {
        ValidationRun {
            run_id: "v-1".into(),
            kind: RunKind::RunNow,
            launched_by: "peer:pine".into(),
            at: 100,
            participants: participants.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    fn row(from: &str, reach: &[(&str, bool)]) -> NodeReachability {
        NodeReachability {
            from: from.into(),
            at: 200,
            results: reach
                .iter()
                .map(|(p, ok)| PeerReach {
                    peer: (*p).to_string(),
                    overlay_ip: "10.42.0.9".into(),
                    reachable: *ok,
                    rtt_ms: ok.then_some(5.0),
                })
                .collect(),
        }
    }

    #[test]
    fn run_and_rows_round_trip_and_pending_clears() {
        let tmp = tempfile::tempdir().unwrap();
        let r = run(&["pine", "oak"]);
        write_run(tmp.path(), &r).unwrap();
        assert_eq!(read_run(tmp.path(), "v-1").unwrap().participants.len(), 2);
        assert_eq!(list_run_ids(tmp.path()), ["v-1"]);
        assert!(row_pending_for(tmp.path(), &r, "pine"));
        assert!(
            !row_pending_for(tmp.path(), &r, "stranger"),
            "non-participant"
        );
        write_row(tmp.path(), "v-1", &row("pine", &[("oak", true)])).unwrap();
        assert!(
            !row_pending_for(tmp.path(), &r, "pine"),
            "row clears pending"
        );
        assert_eq!(read_rows(tmp.path(), "v-1").len(), 1);
    }

    #[test]
    fn all_reachable_passes() {
        let r = run(&["pine", "oak", "elm"]);
        let rows = vec![
            row("pine", &[("oak", true), ("elm", true)]),
            row("oak", &[("pine", true), ("elm", true)]),
            row("elm", &[("pine", true), ("oak", true)]),
        ];
        let agg = aggregate(&r, &rows);
        assert!(agg.passed());
        assert_eq!(agg.reachable.len(), 6); // 3 nodes → 6 directed edges
        assert!(agg.failed.is_empty());
    }

    #[test]
    fn unreachable_edge_is_a_failure_not_a_pass() {
        let r = run(&["pine", "oak"]);
        let rows = vec![
            row("pine", &[("oak", false)]), // pine can't reach oak
            row("oak", &[("pine", true)]),
        ];
        let agg = aggregate(&r, &rows);
        assert!(!agg.passed());
        assert!(agg.failed.contains(&Edge {
            from: "pine".into(),
            to: "oak".into()
        }));
        assert!(agg.reachable.contains(&Edge {
            from: "oak".into(),
            to: "pine".into()
        }));
    }

    #[test]
    fn silent_omission_counts_as_failure() {
        // pine reports but never mentions oak — we do NOT assume reachable.
        let r = run(&["pine", "oak"]);
        let rows = vec![row("pine", &[])];
        let agg = aggregate(&r, &rows);
        assert!(agg.failed.contains(&Edge {
            from: "pine".into(),
            to: "oak".into()
        }));
        // oak never reported at all → missing, not an edge failure from oak.
        assert!(agg.missing_reporters.contains("oak"));
        assert!(!agg.passed());
    }
}
