//! PD-11 — the lifecycle executor.
//!
//! Polls this host's `<root>/fleet/lifecycle/<self>/` for requests
//! (written by any peer's directory verb, carried by replication),
//! validates each against **what this box actually offers right
//! now** — the requested container/VM name must appear in the local
//! probe, the L9 descriptor-gate rail: no arbitrary `podman`/`virsh`
//! passthrough — executes, and writes the result file back for the
//! requester to poll.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::time::Duration;

use super::{ShutdownToken, Worker};
use crate::lifecycle::{command_plan, take_requests, write_result, LifecycleResult};

/// Request poll cadence — an op lands within ~3 s of replication.
pub const POLL: Duration = Duration::from_secs(3);

/// The executor worker.
pub struct LifecycleExecWorker {
    workgroup_root: PathBuf,
    self_hostname: String,
}

impl LifecycleExecWorker {
    #[must_use]
    pub fn new(workgroup_root: PathBuf, self_hostname: String) -> Self {
        Self {
            workgroup_root,
            self_hostname,
        }
    }

    /// Is `name` actually offered by this box right now? Containers
    /// validate against `podman ps --all`, VMs against `virsh list
    /// --all` — the same probes the descriptors publish.
    fn offered(&self, kind: &str, name: &str) -> bool {
        match kind {
            "container" => crate::descriptors::probe_podman()
                .iter()
                .any(|c| c.name == name),
            "vm" => crate::descriptors::probe_libvirt()
                .iter()
                .any(|v| v.name == name),
            _ => false,
        }
    }

    async fn execute_pending(&self) {
        for req in take_requests(&self.workgroup_root, &self.self_hostname) {
            let result = if !self.offered(&req.kind, &req.name) {
                LifecycleResult {
                    id: req.id.clone(),
                    ok: false,
                    error: format!(
                        "{} `{}` is not in this box's published inventory — refused (L9 rail)",
                        req.kind, req.name
                    ),
                }
            } else if let Some((bin, args)) = command_plan(&req) {
                match tokio::process::Command::new(bin).args(&args).output().await {
                    Ok(out) if out.status.success() => LifecycleResult {
                        id: req.id.clone(),
                        ok: true,
                        error: String::new(),
                    },
                    Ok(out) => LifecycleResult {
                        id: req.id.clone(),
                        ok: false,
                        error: String::from_utf8_lossy(&out.stderr).trim().to_string(),
                    },
                    Err(e) => LifecycleResult {
                        id: req.id.clone(),
                        ok: false,
                        error: format!("{bin} unavailable: {e}"),
                    },
                }
            } else {
                LifecycleResult {
                    id: req.id.clone(),
                    ok: false,
                    error: "invalid kind/op".into(),
                }
            };
            tracing::info!(
                id = %req.id, kind = %req.kind, name = %req.name, op = %req.op,
                ok = result.ok, "lifecycle_exec: request handled (PD-11)"
            );
            let _ = write_result(&self.workgroup_root, &self.self_hostname, &result);
        }
    }
}

#[async_trait::async_trait]
impl Worker for LifecycleExecWorker {
    fn name(&self) -> &'static str {
        "lifecycle_exec"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            self.execute_pending().await;
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
    use crate::lifecycle::{write_request, LifecycleRequest};

    #[tokio::test]
    async fn unoffered_targets_are_refused_with_the_l9_rail() {
        let tmp = tempfile::tempdir().unwrap();
        write_request(
            tmp.path(),
            "pine",
            &LifecycleRequest {
                id: "r1".into(),
                kind: "container".into(),
                name: "definitely-not-running-here".into(),
                op: "stop".into(),
                from: "peer:oak".into(),
            },
        )
        .unwrap();
        let w = LifecycleExecWorker::new(tmp.path().to_path_buf(), "pine".into());
        w.execute_pending().await;
        let r = crate::lifecycle::take_result(tmp.path(), "pine", "r1").expect("result written");
        assert!(!r.ok);
        assert!(r.error.contains("refused"), "{}", r.error);
    }

    #[tokio::test]
    async fn worker_name_is_locked() {
        let w = LifecycleExecWorker::new(PathBuf::from("/tmp/x"), "pine".into());
        assert_eq!(w.name(), "lifecycle_exec");
    }
}
