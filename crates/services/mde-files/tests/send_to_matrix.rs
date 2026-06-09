//! Phase 6.3 — Send-To matrix tests.
//!
//! Exercises every (destination × mode × conflict-policy) triple
//! against `DemoBackend` and asserts the audit row that lands has
//! the expected shape. Today's matrix size:
//!
//!   4 Destination variants × 5 SendMode variants × 4 ConflictPolicy
//!   = 80 triples.
//!
//! Each triple is one test invocation; failures point at the
//! specific tuple that broke so regressions are diagnosable.

use std::path::PathBuf;

use mde_files::backend::{Backend, ConflictPolicy, DemoBackend, Destination, SendMode};

fn destinations() -> Vec<Destination> {
    vec![
        Destination::Peer("pine".into()),
        Destination::Group("audio".into()),
        Destination::Role("host".into()),
        Destination::Site("lab".into()),
    ]
}

fn modes() -> Vec<SendMode> {
    vec![
        SendMode::Copy,
        SendMode::Move,
        SendMode::Sync,
        SendMode::Deploy,
        SendMode::Stage,
    ]
}

fn policies() -> Vec<ConflictPolicy> {
    vec![
        ConflictPolicy::Ask,
        ConflictPolicy::Skip,
        ConflictPolicy::Overwrite,
        ConflictPolicy::Rename,
    ]
}

#[test]
fn every_send_to_triple_records_one_audit_row() {
    let mut backend = DemoBackend::new();
    let mut sent = 0;
    for d in destinations() {
        for m in modes() {
            for c in policies() {
                let r = backend.send_to(&[PathBuf::from("/tmp/src")], d.clone(), m, c);
                assert!(
                    r.is_ok(),
                    "send_to({d:?}, {m:?}, {c:?}) returned Err: {r:?}"
                );
                sent += 1;
            }
        }
    }
    let log = backend.audit_log();
    assert_eq!(log.len(), sent, "audit log must carry one row per triple");
    assert_eq!(sent, 4 * 5 * 4, "matrix size = 4*5*4 = 80");
}

#[test]
fn audit_row_destination_matches_call() {
    let mut backend = DemoBackend::new();
    for d in destinations() {
        let id = backend
            .send_to(
                &[PathBuf::from("/tmp/x")],
                d.clone(),
                SendMode::Copy,
                ConflictPolicy::Ask,
            )
            .expect("send_to");
        let row = backend
            .audit_log()
            .into_iter()
            .find(|a| a.op_id == id)
            .expect("audit row");
        assert_eq!(row.destination, d, "audit row destination mismatch");
    }
}

#[test]
fn audit_row_mode_matches_call() {
    let mut backend = DemoBackend::new();
    for m in modes() {
        let id = backend
            .send_to(
                &[PathBuf::from("/tmp/x")],
                Destination::Peer("p".into()),
                m,
                ConflictPolicy::Ask,
            )
            .expect("send_to");
        let row = backend
            .audit_log()
            .into_iter()
            .find(|a| a.op_id == id)
            .expect("audit row");
        assert_eq!(row.mode, m, "audit row mode mismatch");
    }
}

#[test]
fn op_ids_are_unique_across_every_triple() {
    let mut backend = DemoBackend::new();
    let mut seen = std::collections::HashSet::new();
    for d in destinations() {
        for m in modes() {
            for c in policies() {
                let id = backend
                    .send_to(&[PathBuf::from("/tmp/src")], d.clone(), m, c)
                    .expect("send_to");
                assert!(seen.insert(id), "op_id {id} reused across triples");
            }
        }
    }
    assert_eq!(seen.len(), 4 * 5 * 4);
}

#[test]
fn rollback_round_trip_works_for_every_destination() {
    for d in destinations() {
        let mut backend = DemoBackend::new();
        let original = backend
            .send_to(
                &[PathBuf::from("/tmp/r")],
                d.clone(),
                SendMode::Move,
                ConflictPolicy::Overwrite,
            )
            .expect("send_to");
        let rb = backend.rollback(original).expect("rollback");
        assert_ne!(original, rb, "rollback must allocate a fresh op_id");
        let log = backend.audit_log();
        assert_eq!(log[0].op_id, rb);
        assert_eq!(log[0].kind, "rollback");
        assert_eq!(log[0].destination, d);
    }
}
