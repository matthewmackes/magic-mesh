//! PD-9 / FPG — the fleet-reconcile driver.
//!
//! The missing engine-mount for FPG-8: drives `magic-fleet reconcile`
//! (elect the head of the replicated revision log → converge
//! host-local → write the apply-ack) on a 15-minute cadence, and
//! **immediately** when this host's nudge file appears
//! (`<root>/fleet/nudges/<hostname>` — written by the directory's
//! "Apply now", carried here by replication, consumed exactly once).
//! The nudge only hurries convergence to the elected head; it can
//! never fork per-peer state (Q16).

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::{ShutdownToken, Worker};

/// Full-cadence reconcile interval (matches the legacy fleet timer).
pub const CADENCE: Duration = Duration::from_secs(900);

/// Nudge poll interval — how fast an "Apply now" lands.
pub const NUDGE_POLL: Duration = Duration::from_secs(10);

/// The reconcile driver worker.
pub struct FleetReconcileWorker {
    workgroup_root: PathBuf,
    hostname: String,
}

impl FleetReconcileWorker {
    #[must_use]
    pub fn new(workgroup_root: PathBuf, hostname: String) -> Self {
        Self {
            workgroup_root,
            hostname,
        }
    }

    async fn run_reconcile(&self) {
        let root = self.workgroup_root.display().to_string();
        match tokio::process::Command::new("magic-fleet")
            .args([
                "reconcile",
                &format!("--root={root}"),
                &format!("--hostname={}", self.hostname),
            ])
            .status()
            .await
        {
            Ok(st) if st.success() => {
                tracing::info!("fleet_reconcile: converged (magic-fleet reconcile ok)");
            }
            Ok(st) => {
                tracing::warn!("fleet_reconcile: magic-fleet reconcile exited {st}");
            }
            Err(e) => {
                tracing::debug!("fleet_reconcile: magic-fleet unavailable: {e}");
            }
        }
    }
}

#[async_trait::async_trait]
impl Worker for FleetReconcileWorker {
    fn name(&self) -> &'static str {
        "fleet_reconcile"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let mut last_full = Instant::now()
            .checked_sub(CADENCE)
            .unwrap_or_else(Instant::now); // first tick reconciles
        loop {
            let nudged = magic_fleet::store::take_nudge(&self.workgroup_root, &self.hostname);
            if nudged || last_full.elapsed() >= CADENCE {
                if nudged {
                    tracing::info!("fleet_reconcile: nudged — reconciling now (PD-9)");
                }
                self.run_reconcile().await;
                last_full = Instant::now();
            }
            tokio::select! {
                _ = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(NUDGE_POLL) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn worker_name_is_locked() {
        let w = FleetReconcileWorker::new(PathBuf::from("/tmp/x"), "pine".into());
        assert_eq!(w.name(), "fleet_reconcile");
    }

    #[tokio::test]
    async fn worker_exits_on_shutdown() {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = FleetReconcileWorker::new(tmp.path().to_path_buf(), "pine".into());
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let _ = tx.send(true);
        let result = tokio::time::timeout(Duration::from_secs(5), w.run(token))
            .await
            .expect("must exit on shutdown");
        assert!(result.is_ok());
    }
}
