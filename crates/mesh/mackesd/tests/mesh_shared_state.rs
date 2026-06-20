//! OB6-FIX-1 — multi-node shared-state integration test.
//!
//! The ONBOARD-6 audit gap: the shared-state plane (leader election + the peer
//! directory + healthz counts) was only ever unit-tested with a PER-INSTANCE
//! tempdir, so the cross-node sharing semantics were never asserted — and in
//! production QNM-Shared turned out to never be a shared mount, a failure the
//! green single-instance tests masked entirely (the code behaves identically
//! against a local dir).
//!
//! This test points TWO node-instances at ONE shared directory (standing in for
//! the LizardFS mount both nodes mfsmount) and asserts the invariants that only
//! hold when the substrate is genuinely shared:
//!   * exactly one node wins leadership on the shared lock (the other follows);
//!   * a heartbeat written by node A is visible in node B's directory view;
//!   * healthz `node_count`/`is_leader` reflect the shared mesh, not an empty
//!     local store (the exact regression behind "node_count: 0 / NO LEADER").
//!
//! A shared tempdir is the faithful stand-in for the gap: the bug was the
//! ABSENCE of sharing, so a shared dir exercises precisely what was missing,
//! reliably in normal CI (the real nebula transport is covered by OBS-1's
//! container suite; the LizardFS deploy by install-helpers/setup-qnm-shared.sh).

#![cfg(feature = "async-services")]

use std::time::{SystemTime, UNIX_EPOCH};

use mackes_mesh_types::peers::{peers_dir, write_peer_record, PeerRecord};
use mackesd_core::ipc::directory::DirectoryService;
use mackesd_core::leader::{self, AcquireResult};
use mackesd_core::workers::leader_election::LeaderElection;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn heartbeat(host: &str, now: u64) -> PeerRecord {
    PeerRecord {
        hostname: host.to_string(),
        mde_version: Some("10.0.0".into()),
        last_seen_ms: now,
        health: "healthy".into(),
        descriptors: None,
        overlay_ip: Some("10.42.0.9".into()),
        role: None,
        external_addr: None,
    }
}

#[test]
fn exactly_one_leader_is_elected_across_nodes_on_a_shared_volume() {
    let shared = tempfile::tempdir().unwrap(); // the shared QNM-Shared mount
    let node_a = LeaderElection::new(shared.path().to_path_buf(), "peer:a".into());
    let node_b = LeaderElection::new(shared.path().to_path_buf(), "peer:b".into());

    // A claims the shared lock first; B — contending for the SAME file — must
    // follow, not double-acquire. (A per-instance tempdir would let both win,
    // which is exactly why the old tests missed the substrate requirement.)
    assert!(
        matches!(node_a.tick_once(), Some(AcquireResult::Acquired)),
        "first node must acquire leadership"
    );
    match node_b.tick_once() {
        Some(AcquireResult::HeldBy { leader_id, .. }) => {
            assert_eq!(
                leader_id, "peer:a",
                "second node must follow the real leader"
            );
        }
        other => panic!("second node on the shared lock must follow, got {other:?}"),
    }

    // The read-only lease view (what healthz uses) agrees on the holder.
    let lease = leader::read_current_lease(&shared.path().join(".mackesd-leader.lock"))
        .expect("a fresh lease must be readable");
    assert_eq!(lease.node_id, "peer:a");
}

#[test]
fn a_heartbeat_on_one_node_is_visible_in_another_nodes_directory() {
    let shared = tempfile::tempdir().unwrap();
    let now = now_ms();
    let pdir = peers_dir(shared.path());
    // Two nodes publish heartbeats into the shared peers dir (as their
    // mackesd would over the LizardFS mount).
    write_peer_record(&pdir, &heartbeat("alpha", now)).unwrap();
    write_peer_record(&pdir, &heartbeat("beta", now)).unwrap();

    // Node "beta" builds its directory view off the shared dir and sees BOTH.
    let svc = DirectoryService::new(shared.path(), None);
    let (count, healthy, _deg, _unreach, _leader) = svc.mesh_health_counts("peer:beta", now);
    assert_eq!(
        count, 2,
        "both peers' heartbeats visible across the shared volume"
    );
    assert_eq!(healthy, 2, "both report healthy");
}

#[test]
fn healthz_counts_reflect_the_shared_mesh_and_elected_leader() {
    // The exact regression: with the shared volume populated, node_count is N
    // and is_leader reflects the lease — not node_count:0 / is_leader:false.
    let shared = tempfile::tempdir().unwrap();
    let now = now_ms();
    let pdir = peers_dir(shared.path());
    for host in ["n1", "n2", "n3"] {
        write_peer_record(&pdir, &heartbeat(host, now)).unwrap();
    }
    // n1 holds leadership.
    let n1 = LeaderElection::new(shared.path().to_path_buf(), "peer:n1".into());
    assert!(matches!(n1.tick_once(), Some(AcquireResult::Acquired)));

    let svc = DirectoryService::new(shared.path(), None);
    // From the leader's vantage: 3 nodes, leader true.
    let (count, _h, _d, _u, is_leader) = svc.mesh_health_counts("peer:n1", now);
    assert_eq!(count, 3, "healthz node_count == shared mesh size, not 0");
    assert!(is_leader, "the lease holder reports is_leader=true");
    // From a follower's vantage: same count, leader false.
    let (count_f, _h, _d, _u, is_leader_f) = svc.mesh_health_counts("peer:n2", now);
    assert_eq!(count_f, 3);
    assert!(!is_leader_f, "a non-holder reports is_leader=false");
}
