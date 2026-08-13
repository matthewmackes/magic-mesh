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

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{ShutdownToken, Worker};
use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};

/// Full-cadence reconcile interval (matches the legacy fleet timer).
pub const CADENCE: Duration = Duration::from_secs(900);

/// Nudge poll interval — how fast an "Apply now" lands.
pub const NUDGE_POLL: Duration = Duration::from_secs(10);

/// A reconcile must finish inside one recovery window. A wedged automation
/// child must not pin daemon shutdown or prevent a corrected-forward revision
/// from being attempted on the next poll.
pub const RECONCILE_TIMEOUT: Duration = Duration::from_secs(600);

/// The reconcile driver worker.
pub struct FleetReconcileWorker {
    workgroup_root: PathBuf,
    hostname: String,
}

impl FleetReconcileWorker {
    /// Create the driver. `hostname` is used to locate and consume
    /// nudge files written by any peer's "Apply now" action.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, hostname: String) -> Self {
        Self {
            workgroup_root,
            hostname,
        }
    }

    async fn run_reconcile(&self) -> bool {
        self.run_reconcile_with_timeout(Path::new("magic-fleet"), RECONCILE_TIMEOUT)
            .await
    }

    #[cfg(test)]
    async fn run_reconcile_with(&self, program: &Path) -> bool {
        self.run_reconcile_with_timeout(program, RECONCILE_TIMEOUT)
            .await
    }

    async fn run_reconcile_with_timeout(&self, program: &Path, timeout: Duration) -> bool {
        let root = self.workgroup_root.display().to_string();
        let mut command = tokio::process::Command::new(program);
        command.args([
            "reconcile",
            &format!("--root={root}"),
            &format!("--hostname={}", self.hostname),
        ]);
        match super::proc::status_with_timeout_async(command, timeout).await {
            Ok(st) if st.success() => {
                tracing::info!("fleet_reconcile: converged (magic-fleet reconcile ok)");
                true
            }
            Ok(st) => {
                tracing::warn!("fleet_reconcile: magic-fleet reconcile exited {st}");
                false
            }
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                tracing::warn!(
                    "fleet_reconcile: magic-fleet exceeded {timeout:?}; killing it so corrected-forward recovery remains due"
                );
                false
            }
            Err(e) => {
                tracing::debug!("fleet_reconcile: magic-fleet unavailable: {e}");
                false
            }
        }
    }

    fn record_attempt(last_full: &mut Instant, reconciled: bool) {
        if reconciled {
            *last_full = Instant::now();
        } else {
            *last_full = Instant::now()
                .checked_sub(CADENCE)
                .unwrap_or_else(Instant::now);
        }
    }

    /// Consume one replicated nudge only after verifying the exact producer
    /// envelope. The replicated file is a transport, not an authority: a
    /// forged marker must not be able to trigger a privileged reconcile.
    fn consume_authorized_nudge(&self, authorizer: &ActionAuthorizer) -> bool {
        let Some(body) =
            magic_fleet::store::take_nudge_payload(&self.workgroup_root, &self.hostname)
        else {
            return false;
        };
        let Ok(request) = serde_json::from_str::<serde_json::Value>(&body) else {
            tracing::warn!(target: "mackesd::fleet_reconcile", "discarded malformed nudge envelope");
            return false;
        };
        let Some(peer) = request.get("peer").and_then(serde_json::Value::as_str) else {
            tracing::warn!(target: "mackesd::fleet_reconcile", "discarded nudge without peer");
            return false;
        };
        if peer != self.hostname {
            tracing::warn!(
                target: "mackesd::fleet_reconcile",
                peer,
                hostname = %self.hostname,
                "discarded nudge for another host"
            );
            return false;
        }
        // The signed token carries the source responder node. Reading that
        // field only selects the verifier context; the HMAC still authenticates
        // it together with the exact body and target before the nonce is claimed.
        let Some(token) = request
            .get("armed_token")
            .and_then(serde_json::Value::as_str)
            .and_then(mackes_mesh_types::cloud::CloudArmedToken::parse)
        else {
            tracing::warn!(target: "mackesd::fleet_reconcile", "discarded nudge without a capability");
            return false;
        };
        match authorizer.authorize(
            &body,
            MutationContext {
                verb: "fleet-nudge",
                node: &token.node,
                target: peer,
            },
        ) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(target: "mackesd::fleet_reconcile", %error, "discarded unauthorized nudge");
                false
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
        let authorizer = ActionAuthorizer::production();
        let mut last_full = Instant::now()
            .checked_sub(CADENCE)
            .unwrap_or_else(Instant::now); // first tick reconciles
        loop {
            let nudged = self.consume_authorized_nudge(&authorizer);
            if nudged || last_full.elapsed() >= CADENCE {
                if nudged {
                    tracing::info!("fleet_reconcile: nudged — reconciling now (PD-9)");
                }
                // Dropping the bounded command future on shutdown also drops
                // its kill-on-drop child, so a wedged reconcile cannot hold a
                // daemon restart open until the command deadline.
                let reconciled = tokio::select! {
                    result = self.run_reconcile() => result,
                    _ = shutdown.wait() => return Ok(()),
                };
                Self::record_attempt(&mut last_full, reconciled);
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
    use crate::ipc::action_auth::authorize_test_body;
    use crate::ipc::action_auth::ActionAuthorizer;

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

    #[test]
    fn replicated_nudge_requires_exact_single_use_capability() {
        const KEY: &[u8] = b"fleet-reconcile-nudge-auth-test-key";
        const NOW: i64 = 1_700_000_000_000;
        let workgroup = tempfile::tempdir().unwrap();
        let auth_root = tempfile::tempdir().unwrap();
        let authorizer = ActionAuthorizer::for_test(KEY, auth_root.path().to_path_buf(), NOW);
        let worker = FleetReconcileWorker::new(workgroup.path().to_path_buf(), "pine".into());
        let raw = r#"{"peer":"pine","schema_version":1}"#;

        magic_fleet::store::write_nudge_payload(workgroup.path(), "pine", raw).unwrap();
        assert!(!worker.consume_authorized_nudge(&authorizer));

        let armed = authorize_test_body(
            KEY,
            raw,
            MutationContext {
                verb: "fleet-nudge",
                node: "source",
                target: "pine",
            },
            "fleet-nudge-once",
            NOW + 30_000,
        );
        magic_fleet::store::write_nudge_payload(workgroup.path(), "pine", &armed).unwrap();
        assert!(worker.consume_authorized_nudge(&authorizer));

        magic_fleet::store::write_nudge_payload(workgroup.path(), "pine", &armed).unwrap();
        assert!(!worker.consume_authorized_nudge(&authorizer));
    }

    #[tokio::test]
    async fn failed_reconcile_remains_due_for_corrected_forward_retry() {
        let worker = FleetReconcileWorker::new(PathBuf::from("/tmp/x"), "pine".into());
        let mut last_full = Instant::now();

        let reconciled = worker.run_reconcile_with(Path::new("/usr/bin/false")).await;
        FleetReconcileWorker::record_attempt(&mut last_full, reconciled);

        assert!(
            last_full.elapsed() >= CADENCE,
            "a failed restart or peer-return reconcile must remain immediately retryable"
        );

        FleetReconcileWorker::record_attempt(&mut last_full, true);
        assert!(last_full.elapsed() < CADENCE);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn wedged_reconcile_cannot_block_restart_or_corrected_forward_generation() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("wedged-magic-fleet");
        std::fs::write(
            &program,
            r#"#!/bin/sh
for argument in "$@"; do
    case "$argument" in
        --root=*) root="${argument#--root=}" ;;
    esac
done
printf started > "$root/wedged-started"
exec /usr/bin/sleep 600
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&program).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&program, permissions).unwrap();

        let worker = FleetReconcileWorker::new(root.path().to_path_buf(), "pine".into());
        let reconciled = worker
            .run_reconcile_with_timeout(&program, Duration::from_millis(500))
            .await;

        assert!(root.path().join("wedged-started").is_file());
        assert!(!reconciled, "a timed-out generation must never count as converged");
        assert!(
            worker
                .run_reconcile_with_timeout(
                    Path::new("/usr/bin/true"),
                    Duration::from_secs(1),
                )
                .await,
            "the corrected-forward generation must run after the wedged child is killed"
        );
    }
}
