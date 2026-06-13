//! PLANES-15 (W66/W77/W78) — the netstate engine mount.
//!
//! The runtime-reachable side of [`magic_fleet::netstate`]: on a cadence
//! (and right after a fleet nudge would have landed a new revision) this
//! worker reads the **elected** fleet revision's `netstate` desired-state
//! and converges the box's network to it — but ALWAYS through the
//! checkpoint-guarded apply ([`apply_with_self_test`]) so a bad
//! address/route can never strand the node off its own overlay (W77/W78).
//!
//! The reachability self-test targets are derived live from the roster
//! mirror: the lighthouse's overlay IP plus one other peer's (never this
//! box). If after apply the node can't still reach BOTH, the nmstate
//! checkpoint rolls it back and the worker logs the rollback loudly. With
//! no `netstate` declared (the common case) this is a cheap no-op.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::time::Duration;

use magic_fleet::netstate::{apply_with_self_test, ApplyOutcome, NetOps, SystemNetOps};

use super::{ShutdownToken, Worker};

/// Converge cadence — paced with `fleet_reconcile`'s full tick.
pub const CADENCE: Duration = Duration::from_secs(900);

/// The netstate engine mount worker.
pub struct NetstateApplyWorker {
    workgroup_root: PathBuf,
    store_db: Option<PathBuf>,
    hostname: String,
}

impl NetstateApplyWorker {
    /// Create the worker. `store_db` is the roster mirror used to derive
    /// post-apply self-test probe targets (lighthouse + one peer).
    #[must_use]
    pub fn new(workgroup_root: PathBuf, store_db: Option<PathBuf>, hostname: String) -> Self {
        Self {
            workgroup_root,
            store_db,
            hostname,
        }
    }

    /// The overlay IPs the post-apply self-test must still reach: the
    /// lighthouse (role `host`) and one other peer, never this box. An
    /// empty list (e.g. a lone lighthouse) means "no peers to lose" — the
    /// self-test then trivially passes, which is correct: there is no
    /// overlay path to sever.
    fn probe_targets(&self) -> Vec<String> {
        let Some(db) = &self.store_db else {
            return Vec::new();
        };
        let Ok(conn) =
            rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        else {
            return Vec::new();
        };
        let Ok(rows) = crate::nebula_roster::export_roster(&conn) else {
            return Vec::new();
        };
        let others: Vec<&crate::nebula_roster::RosterRow> = rows
            .iter()
            .filter(|r| r.name != self.hostname && !r.overlay_ip.is_empty())
            .collect();
        let mut targets = Vec::new();
        // The lighthouse first (groups carries the role; `host` = lighthouse).
        if let Some(lh) = others.iter().find(|r| r.groups.contains("host")) {
            targets.push(lh.overlay_ip.clone());
        }
        // Plus one more distinct peer.
        if let Some(peer) = others.iter().find(|r| !targets.contains(&r.overlay_ip)) {
            targets.push(peer.overlay_ip.clone());
        }
        targets
    }

    /// One converge pass. Returns the outcome (for tests / logging).
    fn converge(&self, ops: &dyn NetOps) -> ApplyOutcome {
        let dir = magic_fleet::store::revisions_dir(&self.workgroup_root);
        let Some(head) = magic_fleet::store::elect_head(&dir) else {
            return ApplyOutcome::NoChange;
        };
        if head.spec.netstate.is_empty() {
            return ApplyOutcome::NoChange;
        }
        apply_with_self_test(ops, &head.spec.netstate, &self.probe_targets())
    }

    fn tick(&self) {
        let outcome = self.converge(&SystemNetOps);
        match &outcome {
            ApplyOutcome::NoChange => {}
            ApplyOutcome::Committed => {
                tracing::info!("netstate_apply: network converged + self-test passed (PLANES-15)");
            }
            ApplyOutcome::RolledBack { unreachable } => {
                tracing::warn!(
                    ?unreachable,
                    "netstate_apply: self-test FAILED — checkpoint reverted the box (W78)"
                );
            }
            ApplyOutcome::Failed { error } => {
                tracing::warn!(%error, "netstate_apply: apply errored — reverted");
            }
        }
    }
}

#[async_trait::async_trait]
impl Worker for NetstateApplyWorker {
    fn name(&self) -> &'static str {
        "netstate_apply"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            // nmstatectl + ping are blocking; keep them off the scheduler.
            let this = NetstateApplyWorker {
                workgroup_root: self.workgroup_root.clone(),
                store_db: self.store_db.clone(),
                hostname: self.hostname.clone(),
            };
            let _ = tokio::task::spawn_blocking(move || this.tick()).await;
            tokio::select! {
                _ = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(CADENCE) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magic_fleet::netstate::{IpAddress, IpConfig, LinkState, NetInterface, NetState};
    use magic_fleet::store::{revisions_dir, write_revision};
    use magic_fleet::{BaselineSpec, Revision};

    /// A mock that reports an empty actual state and a fixed reachability
    /// verdict — exercises the worker's converge() without root or NICs.
    struct Mock {
        reachable: bool,
    }
    impl NetOps for Mock {
        fn read_actual(&self) -> NetState {
            NetState::default()
        }
        fn checkpoint(&self) -> Result<String, String> {
            Ok("cp".into())
        }
        fn apply(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn unreachable(&self, t: &[String]) -> Vec<String> {
            if self.reachable {
                Vec::new()
            } else {
                t.to_vec()
            }
        }
        fn commit(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn rollback(&self, _: &str) {}
    }

    fn seed_revision_with_netstate(root: &std::path::Path) {
        let mut spec = BaselineSpec::default();
        spec.netstate = NetState {
            interfaces: vec![NetInterface {
                name: "eth0".into(),
                iface_type: "ethernet".into(),
                state: LinkState::Up,
                ipv4: Some(IpConfig {
                    enabled: true,
                    dhcp: false,
                    addresses: vec![IpAddress {
                        ip: "10.42.0.7".into(),
                        prefix_len: 24,
                    }],
                }),
                ipv6: None,
            }],
            ..Default::default()
        };
        let dir = revisions_dir(root);
        std::fs::create_dir_all(&dir).unwrap();
        write_revision(
            &dir,
            &Revision {
                version: 1,
                author: "peer:oak".into(),
                at: 100,
                spec,
            },
        )
        .unwrap();
    }

    #[test]
    fn no_revision_is_a_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let w = NetstateApplyWorker::new(tmp.path().to_path_buf(), None, "pine".into());
        assert_eq!(
            w.converge(&Mock { reachable: true }),
            ApplyOutcome::NoChange
        );
    }

    #[test]
    fn netstate_revision_converges_when_self_test_passes() {
        let tmp = tempfile::tempdir().unwrap();
        seed_revision_with_netstate(tmp.path());
        let w = NetstateApplyWorker::new(tmp.path().to_path_buf(), None, "pine".into());
        assert_eq!(
            w.converge(&Mock { reachable: true }),
            ApplyOutcome::Committed
        );
    }

    #[test]
    fn no_probe_targets_means_nothing_to_lose_so_it_commits() {
        // No store_db → empty roster → no probe targets. The W78
        // "no overlay path to sever" case: even with the mock reporting
        // everything unreachable, an EMPTY target list yields an empty
        // unreachable set, so the apply commits. (The rollback path with
        // real targets is pinned in the engine's own tests.)
        let tmp = tempfile::tempdir().unwrap();
        seed_revision_with_netstate(tmp.path());
        let w = NetstateApplyWorker::new(tmp.path().to_path_buf(), None, "pine".into());
        assert_eq!(
            w.converge(&Mock { reachable: false }),
            ApplyOutcome::Committed
        );
    }
}
