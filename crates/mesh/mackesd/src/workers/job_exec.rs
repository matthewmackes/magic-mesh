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

use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mackes_mesh_types::cap_tags::{read_tags, CapabilityTag};
use magic_fleet::jobs::{
    normalize_playbook_ref, read_run, resolve_playbook_path, run_pending_for, runs_dir,
    write_target_result, JobRun, TargetResult,
};
use sha2::{Digest, Sha256};

use super::{ShutdownToken, Worker};
use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};

/// Run poll cadence.
pub const POLL: Duration = Duration::from_secs(5);

/// A signed playbook is configuration, not an unbounded payload. Keep the
/// executor's memory and apply latency bounded before hashing or parsing it.
const MAX_PLAYBOOK_BYTES: u64 = 1024 * 1024;

fn read_bounded_playbook(path: impl AsRef<std::path::Path>) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_PLAYBOOK_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PLAYBOOK_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "playbook exceeds the maximum size",
        ));
    }
    String::from_utf8(bytes).map_err(|error| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, error)
    })
}

/// The local job executor.
pub struct JobExecWorker {
    workgroup_root: PathBuf,
    hostname: String,
    authorizer: Arc<ActionAuthorizer>,
}

impl JobExecWorker {
    /// Create the executor. Only runs on boxes where `hostname` carries
    /// the `execution` capability tag (W84 gate).
    #[must_use]
    pub fn new(workgroup_root: PathBuf, hostname: String) -> Self {
        Self {
            workgroup_root,
            hostname,
            authorizer: Arc::new(ActionAuthorizer::production()),
        }
    }

    #[cfg(test)]
    fn with_authorizer(mut self, authorizer: Arc<ActionAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    /// Every run with a pending target slot for this box.
    fn pending_runs(&self) -> Vec<JobRun> {
        let Ok(entries) = std::fs::read_dir(runs_dir(&self.workgroup_root)) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter_map(|id| {
                let run = read_run(&self.workgroup_root, &id)?;
                (run.run_id == id).then_some(run)
            })
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
            let result = self.execute_job(&run);
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
    fn execute_job(&self, run: &JobRun) -> TargetResult {
        let playbook = match normalize_playbook_ref(&run.playbook) {
            Ok(playbook) if playbook == run.playbook => playbook,
            Ok(_) => return self.refused("playbook reference is not normalized"),
            Err(error) => return self.refused(&error),
        };
        if run.playbook_digest.is_empty() {
            return self.refused("run has no signed playbook digest");
        }
        let Some(auth_body) = run.execution_auth.get(&self.hostname) else {
            return self.refused("run has no target execution authorization");
        };
        let envelope: serde_json::Value = match serde_json::from_str(auth_body) {
            Ok(envelope) => envelope,
            Err(_) => return self.refused("target execution authorization is not JSON"),
        };
        let fields_match = envelope.get("run_id").and_then(serde_json::Value::as_str)
            == Some(run.run_id.as_str())
            && envelope.get("node").and_then(serde_json::Value::as_str)
                == Some(self.hostname.as_str())
            && envelope.get("playbook").and_then(serde_json::Value::as_str)
                == Some(playbook.as_str())
            && envelope
                .get("playbook_digest")
                .and_then(serde_json::Value::as_str)
                == Some(run.playbook_digest.as_str());
        if !fields_match {
            return self.refused("target execution authorization does not match the run");
        }
        let auth_target = format!("{}:{}", run.run_id, run.playbook_digest);
        if let Err(error) = self.authorizer.authorize(
            auth_body,
            MutationContext {
                verb: "jobs-execute",
                node: &self.hostname,
                target: &auth_target,
            },
        ) {
            return self.refused(&format!("target execution authorization refused: {error}"));
        }

        let playbook_path = match resolve_playbook_path(&self.workgroup_root, &playbook) {
            Ok(path) => path,
            Err(error) => return self.refused(&error),
        };
        let playbooks_root = self.workgroup_root.join("playbooks");
        let canonical_root = match std::fs::canonicalize(&playbooks_root) {
            Ok(root) => root,
            Err(error) => return self.failed(&format!("playbooks directory unavailable: {error}")),
        };
        let canonical_path = match std::fs::canonicalize(&playbook_path) {
            Ok(path) => path,
            Err(error) => {
                return self.failed(&format!("playbook {} unreadable: {error}", playbook))
            }
        };
        if !canonical_path.starts_with(&canonical_root) {
            return self.refused("playbook resolves outside the replicated playbooks directory");
        }
        let yaml = match read_bounded_playbook(&playbook_path) {
            Ok(y) => y,
            Err(e) => {
                return self.failed(&format!("playbook {} unreadable: {e}", playbook));
            }
        };
        let digest = sha256_hex(yaml.as_bytes());
        if digest != run.playbook_digest {
            return self.refused("playbook digest differs from the signed run");
        }
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

    fn failed(&self, detail: &str) -> TargetResult {
        TargetResult {
            hostname: self.hostname.clone(),
            status: "failed".into(),
            detail: detail.into(),
        }
    }

    fn refused(&self, detail: &str) -> TargetResult {
        self.failed(&format!("refused: {detail}"))
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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
                authorizer: Arc::clone(&self.authorizer),
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
    use magic_fleet::jobs::read_target_results; // test-only reader
    use magic_fleet::jobs::write_run;

    fn seed_run(root: &std::path::Path) {
        write_run(
            root,
            &JobRun {
                run_id: "r-1".into(),
                playbook: "playbooks/noop.yml".into(),
                playbook_digest: String::new(),
                vars: Default::default(),
                targets: vec!["pine".into()],
                launched_by: "peer:oak".into(),
                at: 1,
                execution_auth: Default::default(),
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
        // Missing target capability is refused before a playbook is read or
        // handed to ansible-runner; the result still clears the pending slot.
        let w = JobExecWorker::new(tmp.path().to_path_buf(), "pine".into());
        assert_eq!(w.run_once(), ["r-1"]);
        let results = read_target_results(tmp.path(), "r-1");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].hostname, "pine");
        assert_eq!(results[0].status, "failed");
        assert!(results[0].detail.contains("no signed playbook digest"));
        // Slot cleared — a second pass finds nothing pending.
        assert!(w.run_once().is_empty());
    }

    #[test]
    fn path_traversal_is_rejected_by_the_shared_reference_validator() {
        assert!(magic_fleet::jobs::normalize_playbook_ref("../outside.yml").is_err());
        assert!(magic_fleet::jobs::normalize_playbook_ref("/etc/passwd").is_err());
        assert_eq!(
            magic_fleet::jobs::normalize_playbook_ref("repair.yml").unwrap(),
            "playbooks/repair.yml"
        );
    }

    #[test]
    fn oversized_playbook_is_rejected_before_digest_or_apply() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("oversized.yml");
        std::fs::write(&path, vec![b'x'; MAX_PLAYBOOK_BYTES as usize + 1]).unwrap();
        let error = read_bounded_playbook(&path).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }
}
