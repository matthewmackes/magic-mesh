//! PLANES-9 (W32) — the local job executor.
//!
//! Polls the replicated `jobs/runs/` for runs that name THIS box as
//! a pending target and aren't yet done here, then runs the run's
//! playbook locally via the FPG `apply` primitive (no push-SSH — the
//! target executes its own), writing its [`TargetResult`] back into
//! the run dir. **Gated on the `execution` capability tag** (W84):
//! an untagged box ignores every run except ones that name it as a
//! peer explicitly... no — per W84 the gate is hard: a box without
//! the `execution` tag refuses job runs outright. Self-targeted
//! config reconcile is the `fleet_reconcile` worker's job, not this.
//!
//! Concurrency: one job at a time per node (a local guard) so a
//! fleet-wide run can't stampede a box mid-apply (W34).

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::time::Duration;

use mackes_mesh_types::cap_tags::{read_tags, CapabilityTag};
use magic_fleet::jobs::{
    read_run, run_pending_for, runs_dir, write_target_result, JobRun, TargetResult,
};

use super::{ShutdownToken, Worker};

/// Run poll cadence.
pub const POLL: Duration = Duration::from_secs(5);

/// The local job executor.
pub struct JobExecWorker {
    workgroup_root: PathBuf,
    hostname: String,
}

impl JobExecWorker {
    #[must_use]
    pub fn new(workgroup_root: PathBuf, hostname: String) -> Self {
        Self {
            workgroup_root,
            hostname,
        }
    }

    /// Every run with a pending target slot for this box.
    fn pending_runs(&self) -> Vec<JobRun> {
        let Ok(entries) = std::fs::read_dir(runs_dir(&self.workgroup_root)) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter_map(|id| read_run(&self.workgroup_root, &id))
            .filter(|run| run_pending_for(&self.workgroup_root, run, &self.hostname))
            .collect()
    }

    /// One executor pass. Returns the runs it executed (for tests).
    fn run_once(&self) -> Vec<String> {
        // W84 — hard gate: no `execution` tag, no jobs.
        if !read_tags(&self.workgroup_root, &self.hostname).has(CapabilityTag::Execution) {
            return Vec::new();
        }
        let mut executed = Vec::new();
        // Serial per node (W34): handle exactly one pending run per pass so
        // a fleet-wide run can't stampede a box mid-apply.
        if let Some(run) = self.pending_runs().into_iter().next() {
            let result = self.execute(&run);
            let _ = write_target_result(&self.workgroup_root, &run.run_id, &result);
            executed.push(run.run_id.clone());
            tracing::info!(
                run = %run.run_id, status = %result.status,
                "job_exec: ran pending job locally (PLANES-9)"
            );
        }
        executed
    }

    /// Resolve + apply the run's playbook locally.
    fn execute(&self, run: &JobRun) -> TargetResult {
        let playbook_path = self.workgroup_root.join(&run.playbook);
        let yaml = match std::fs::read_to_string(&playbook_path) {
            Ok(y) => y,
            Err(e) => {
                return TargetResult {
                    hostname: self.hostname.clone(),
                    status: "failed".into(),
                    detail: format!("playbook {} unreadable: {e}", run.playbook),
                };
            }
        };
        let work =
            std::env::temp_dir().join(format!("mde-job-{}-{}", run.run_id, std::process::id()));
        match magic_fleet::apply(&yaml, &work) {
            Ok(report) if report.failures == 0 && report.unreachable == 0 => TargetResult {
                hostname: self.hostname.clone(),
                status: if report.changed > 0 { "changed" } else { "ok" }.into(),
                detail: String::new(),
            },
            Ok(report) => TargetResult {
                hostname: self.hostname.clone(),
                status: "failed".into(),
                detail: format!(
                    "failures={} unreachable={}",
                    report.failures, report.unreachable
                ),
            },
            Err(e) => TargetResult {
                hostname: self.hostname.clone(),
                status: "failed".into(),
                detail: e.to_string(),
            },
        }
    }
}

#[async_trait::async_trait]
impl Worker for JobExecWorker {
    fn name(&self) -> &'static str {
        "job_exec"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            // The apply call is blocking; hop it off the scheduler.
            let this = JobExecWorker {
                workgroup_root: self.workgroup_root.clone(),
                hostname: self.hostname.clone(),
            };
            let _ = tokio::task::spawn_blocking(move || this.run_once()).await;
            tokio::select! {
                _ = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(POLL) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::cap_tags::{write_tags, NodeTags};
    use magic_fleet::jobs::write_run;

    fn seed_run(root: &std::path::Path) {
        write_run(
            root,
            &JobRun {
                run_id: "r-1".into(),
                playbook: "playbooks/noop.yml".into(),
                vars: Default::default(),
                targets: vec!["pine".into()],
                launched_by: "peer:oak".into(),
                at: 1,
            },
        )
        .unwrap();
    }

    #[test]
    fn untagged_box_refuses_jobs_w84() {
        let tmp = tempfile::tempdir().unwrap();
        seed_run(tmp.path());
        let w = JobExecWorker::new(tmp.path().to_path_buf(), "pine".into());
        // No execution tag → no jobs run, the slot stays pending.
        assert!(w.run_once().is_empty());
        assert!(read_target_results(tmp.path(), "r-1").is_empty());
    }

    #[test]
    fn execution_tagged_box_runs_and_records_a_result() {
        let tmp = tempfile::tempdir().unwrap();
        seed_run(tmp.path());
        let mut tags = NodeTags::default();
        tags.tags.insert(CapabilityTag::Execution);
        write_tags(tmp.path(), "pine", &tags).unwrap();
        // No playbook file on disk → the apply path reports failed,
        // but the run is HANDLED (result written, slot cleared) — the
        // gate + dispatch + result-write loop is what we're pinning.
        let w = JobExecWorker::new(tmp.path().to_path_buf(), "pine".into());
        assert_eq!(w.run_once(), ["r-1"]);
        let results = read_target_results(tmp.path(), "r-1");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].hostname, "pine");
        assert_eq!(results[0].status, "failed"); // missing playbook
                                                 // Slot cleared — a second pass finds nothing pending.
        assert!(w.run_once().is_empty());
    }
}
