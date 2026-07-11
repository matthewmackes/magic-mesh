//! SUBSTRATE-V2 — the shared, substrate-aware leadership gate the leader-gated
//! ACTION workers consult (mackesd-01 / mackesd-04 fix).
//!
//! The mesh has exactly ONE leader, elected + continuously renewed by
//! [`crate::workers::leader_election`]: on a SUBSTRATE-V2 (etcd) fleet it holds a
//! lease on [`crate::substrate::etcd::LEADER_KEY`] (`/mesh/leader`); pre-cutover
//! (no `/etc/mackesd/etcd-endpoints`) it holds the `.mackesd-leader.lock` advisory
//! lease ([`crate::leader`]).
//!
//! Every leader-gated worker (DR backups, datacenter audit, billable SIP/PSTN
//! provisioning, VDI session brokering, snapshot retention, upgrade-intent, …)
//! must gate its exactly-once work on THAT SAME election. Historically each worker
//! hand-rolled an `is_leader()` that called [`crate::leader::try_acquire`] on the
//! fs lock ONLY — substrate-blind (the mackesd-04 copy-paste). On a cut-over fleet
//! that lock lives under the **Syncthing-replicated** workgroup root, which is NOT
//! a single shared POSIX filesystem: every node has its own local copy, so the
//! `O_CREAT|O_EXCL` atomicity that `try_acquire` relies on no longer serializes
//! across nodes and each node "acquires" its own copy → **split-brain**. Every node
//! believes it is leader and runs the exactly-once work N times (N billable PSTN
//! provisions, N concurrent DR backups, …).
//!
//! `LeaderGate` closes that: it is the ONE resolver every gated worker consults.
//! When etcd endpoints are configured it OBSERVES the shared election
//! ([`crate::substrate::leader::current_leader_blocking`]) — it does NOT run a
//! second campaign, so there is one leader per mesh, seen consistently by all
//! gates. With no endpoints it falls back to the exact fs `try_acquire` behavior,
//! so non-etcd / airgapped deployments are byte-for-byte unchanged. It is
//! **fail-closed**: an unreachable/absent substrate resolves to "not leader" — we
//! never run exactly-once work we cannot prove we own.
//!
//! Endpoint discovery is self-contained (read from `/etc/mackesd/etcd-endpoints`
//! via [`crate::substrate::etcd::default_endpoints`]), matching the established
//! [`crate::workers::leader_election`] / `health_reconciler` convention — no config
//! threading through the supervisor.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};

use crate::leader::{try_acquire, AcquireResult};

/// The single leadership resolver the leader-gated ACTION workers share.
///
/// Construct one per gating check with [`LeaderGate::from_lock_path`] (the workers
/// already hold a joined `.mackesd-leader.lock`) or [`LeaderGate::new`], then ask
/// [`LeaderGate::is_leader`].
pub struct LeaderGate {
    /// The fs advisory lock (pre-cutover / airgapped path).
    leader_lock: PathBuf,
    /// This node's mesh identity — compared against the elected leader.
    node_id: String,
    /// SUBSTRATE-V2 etcd client endpoints. Non-empty ⇒ observe the shared etcd
    /// election; empty ⇒ the fs lock path. Same source + contract as
    /// [`crate::workers::leader_election`] and `health_reconciler`.
    endpoints: Vec<String>,
    /// Test-only shared election state — the in-process stand-in for the single
    /// `/mesh/leader` etcd key. Two gates reading the SAME handle prove the
    /// split-brain is closed headlessly (no live etcd). `None` in production.
    #[cfg(test)]
    test_election: Option<std::sync::Arc<std::sync::Mutex<Option<String>>>>,
}

impl LeaderGate {
    /// Build from the already-joined `.mackesd-leader.lock` path a worker holds.
    /// Reads the etcd endpoints from `/etc/mackesd/etcd-endpoints` exactly like
    /// [`crate::workers::leader_election::LeaderElection::new`] does — self-contained,
    /// no config threading.
    #[must_use]
    pub fn from_lock_path(leader_lock: PathBuf, node_id: String) -> Self {
        Self {
            leader_lock,
            node_id,
            endpoints: crate::substrate::etcd::default_endpoints(),
            #[cfg(test)]
            test_election: None,
        }
    }

    /// Build from a `workgroup_root`, deriving the `.mackesd-leader.lock` path
    /// (mirrors the worker constructors + `LeaderElection::new`).
    #[must_use]
    pub fn new(workgroup_root: &Path, node_id: String) -> Self {
        Self::from_lock_path(workgroup_root.join(".mackesd-leader.lock"), node_id)
    }

    /// Override the etcd endpoints (tests / explicit provisioning) — mirrors
    /// [`crate::workers::leader_election::LeaderElection::with_endpoints`].
    #[must_use]
    pub fn with_endpoints(mut self, endpoints: Vec<String>) -> Self {
        self.endpoints = endpoints;
        self
    }

    /// True when leadership resolves from the etcd election rather than the fs lock.
    #[must_use]
    pub fn uses_etcd(&self) -> bool {
        !self.endpoints.is_empty()
    }

    /// Is THIS node the current mesh leader?
    ///
    /// * **etcd fleet** — OBSERVE the shared `/mesh/leader` election that
    ///   `leader_election` maintains (single writer; every gate reads it) and report
    ///   `leader == our node_id`. Fail-closed on an unreachable/absent key. Crucially
    ///   this does NOT campaign or touch the fs lock, so two nodes never both win.
    /// * **pre-cutover** — acquire/renew the fs advisory lease, exactly as the
    ///   retired per-worker `is_leader()` did (behavior preserved byte-for-byte).
    #[must_use]
    pub fn is_leader(&self) -> bool {
        #[cfg(test)]
        {
            if let Some(state) = &self.test_election {
                return state.lock().expect("test_election mutex").as_deref()
                    == Some(self.node_id.as_str());
            }
        }
        if self.uses_etcd() {
            return crate::substrate::leader::current_leader_blocking(&self.endpoints)
                .is_some_and(|l| l.node_id == self.node_id);
        }
        matches!(
            try_acquire(&self.leader_lock, &self.node_id),
            Ok(AcquireResult::Acquired)
        )
    }

    /// The current leader's `node_id`, if any — for status / enrichment surfaces
    /// (e.g. the Copilot mesh context) that show WHO leads without gating on it.
    /// The etcd fleet reads `/mesh/leader`; pre-cutover reads the fs lease. Reads
    /// only — never acquires.
    #[must_use]
    pub fn current_leader_id(&self) -> Option<String> {
        #[cfg(test)]
        {
            if let Some(state) = &self.test_election {
                return state.lock().expect("test_election mutex").clone();
            }
        }
        if self.uses_etcd() {
            return crate::substrate::leader::current_leader_blocking(&self.endpoints)
                .map(|l| l.node_id);
        }
        crate::leader::read_current_lease(&self.leader_lock).map(|l| l.node_id)
    }

    /// Test-only: attach a shared in-memory election (the `/mesh/leader` stand-in),
    /// so two gates can be driven against ONE election state headlessly.
    #[cfg(test)]
    #[must_use]
    fn with_test_election(
        mut self,
        election: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    ) -> Self {
        self.test_election = Some(election);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn shared(leader: Option<&str>) -> Arc<Mutex<Option<String>>> {
        Arc::new(Mutex::new(leader.map(str::to_string)))
    }

    /// A gate on the etcd branch (endpoints present) whose leader read is served by
    /// a shared in-process election — the `/mesh/leader` key both nodes observe.
    fn etcd_gate(node_id: &str, election: &Arc<Mutex<Option<String>>>) -> LeaderGate {
        LeaderGate::new(Path::new("/nonexistent"), node_id.into())
            .with_endpoints(vec!["http://10.42.0.1:2379".into()])
            .with_test_election(Arc::clone(election))
    }

    // ── mackesd-01 / mackesd-04 regression: the etcd path is single-leader ──
    //
    // The whole bug: on a cut-over fleet two nodes each ran the exactly-once work
    // because each acquired its own local fs lock copy. With every gate observing
    // the SAME election, exactly one node leads at any instant.
    #[test]
    fn etcd_election_yields_exactly_one_leader_across_nodes() {
        let election = shared(Some("peer:a"));
        let a = etcd_gate("peer:a", &election);
        let b = etcd_gate("peer:b", &election);

        // Both nodes observe the SAME election ⇒ exactly one leader (was: both).
        assert!(a.is_leader(), "the elected node leads");
        assert!(
            !b.is_leader(),
            "the follower does NOT lead — split-brain closed"
        );

        // Failover: the election moves to B. Still exactly one leader.
        *election.lock().unwrap() = Some("peer:b".into());
        assert!(!a.is_leader());
        assert!(b.is_leader());

        // No leader (key vanished / etcd unreachable): fail-closed — NEITHER node
        // runs the exactly-once work.
        *election.lock().unwrap() = None;
        assert!(!a.is_leader());
        assert!(!b.is_leader());
    }

    // ── the bug this fixes: the fs lock split-brains on per-node local copies ──
    #[test]
    fn fs_lock_split_brains_when_each_node_has_its_own_copy() {
        // Syncthing gives every node its OWN local copy of the workgroup root, so
        // two nodes acquire two DIFFERENT lock files and BOTH win — the exactly-once
        // violation mackesd-01 describes. (Distinct tempdirs = distinct local copies
        // — precisely what Syncthing presents, versus one genuinely shared POSIX fs.)
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let a = LeaderGate::new(dir_a.path(), "peer:a".into()).with_endpoints(vec![]);
        let b = LeaderGate::new(dir_b.path(), "peer:b".into()).with_endpoints(vec![]);
        assert!(a.is_leader());
        assert!(
            b.is_leader(),
            "fs lock on separate local copies = split-brain (the bug the etcd path closes)"
        );
    }

    // ── the real etcd branch selects etcd + never falls back to a per-node fs acquire ──
    #[test]
    fn etcd_path_fails_closed_and_never_touches_the_fs_lock() {
        // Endpoints present but etcd unreachable, and NO injected election ⇒ the real
        // `current_leader_blocking` runs, connect fails, and we resolve NOT-leader
        // (fail-closed). The fs lock must NEVER be created — proof we took the etcd
        // branch, not the per-node fs acquire that caused the split-brain.
        let dir = tempfile::tempdir().unwrap();
        let g = LeaderGate::new(dir.path(), "peer:a".into())
            .with_endpoints(vec!["http://127.0.0.1:1".into()]);
        assert!(!g.is_leader(), "unreachable etcd ⇒ fail-closed, not leader");
        assert!(
            !dir.path().join(".mackesd-leader.lock").exists(),
            "etcd branch must NOT fall back to acquiring the fs lock"
        );
    }

    // ── path selection ──
    #[test]
    fn selects_etcd_path_when_endpoints_present() {
        let g = LeaderGate::new(Path::new("/x"), "n".into())
            .with_endpoints(vec!["http://10.42.0.1:2379".into()]);
        assert!(g.uses_etcd());
    }

    #[test]
    fn selects_fs_path_when_no_endpoints() {
        let g = LeaderGate::new(Path::new("/x"), "n".into()).with_endpoints(vec![]);
        assert!(!g.uses_etcd());
    }

    // ── fs path preserves the retired try_acquire behavior byte-for-byte ──
    #[test]
    fn fs_path_acquires_then_renews_like_try_acquire() {
        let dir = tempfile::tempdir().unwrap();
        let g = LeaderGate::new(dir.path(), "peer:a".into()).with_endpoints(vec![]);
        assert!(g.is_leader(), "uncontended fs lock ⇒ leader");
        assert!(g.is_leader(), "same node renews ⇒ still leader");
        assert!(dir.path().join(".mackesd-leader.lock").exists());
    }

    #[test]
    fn fs_path_follower_is_not_leader_when_another_holds_the_lease() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join(".mackesd-leader.lock");
        // Another node grabs the lease first.
        assert!(matches!(
            try_acquire(&lock, "peer:other"),
            Ok(AcquireResult::Acquired)
        ));
        let g = LeaderGate::new(dir.path(), "peer:us".into()).with_endpoints(vec![]);
        assert!(!g.is_leader());
    }

    // ── current_leader_id shares is_leader's source of truth ──
    #[test]
    fn current_leader_id_reads_the_shared_election_on_etcd() {
        let election = shared(Some("peer:a"));
        let a = etcd_gate("peer:a", &election);
        let b = etcd_gate("peer:b", &election);
        // Both nodes see the SAME leader id regardless of which asks.
        assert_eq!(a.current_leader_id().as_deref(), Some("peer:a"));
        assert_eq!(b.current_leader_id().as_deref(), Some("peer:a"));
    }

    #[test]
    fn current_leader_id_reads_the_fs_lease_when_no_endpoints() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join(".mackesd-leader.lock");
        assert!(matches!(
            try_acquire(&lock, "peer:holder"),
            Ok(AcquireResult::Acquired)
        ));
        let g = LeaderGate::new(dir.path(), "peer:asker".into()).with_endpoints(vec![]);
        assert_eq!(g.current_leader_id().as_deref(), Some("peer:holder"));
    }
}
