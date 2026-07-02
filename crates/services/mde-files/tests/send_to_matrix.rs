//! Phase 6.3 — Send-To matrix tests.
//!
//! Exercises every (destination × mode × conflict-policy) triple against a
//! backend and asserts the audit row that lands has the expected shape.
//! Today's matrix size:
//!
//!   4 Destination variants × 5 SendMode variants × 4 ConflictPolicy
//!   = 80 triples.
//!
//! FILEMGR-1 deleted the shipped `DemoBackend` mockup (§7 no mockups). This
//! test now drives a `#[cfg(test)]` `RecordingBackend` defined right here — a
//! legitimate in-test double (never shipped) that records one audit row per
//! send and allocates increasing op ids, which is exactly the contract the
//! matrix locks. Each triple is one assertion; failures point at the specific
//! tuple that broke so regressions are diagnosable.

use std::path::PathBuf;

use mde_files::backend::{
    AuditEntry, Backend, BackendError, ConflictPolicy, Destination, OpId, SendMode,
};
use mde_files::model::{Peer, SelfNode};

/// In-test `Backend` that records a send/rollback audit row per call and hands
/// out monotonically increasing op ids. Reproduces just the send-audit contract
/// the matrix asserts, with no filesystem or mesh — never shipped (§7).
#[derive(Default)]
struct RecordingBackend {
    next_op_id: OpId,
    audit: Vec<AuditEntry>,
}

impl RecordingBackend {
    fn new() -> Self {
        Self {
            next_op_id: 1,
            audit: Vec::new(),
        }
    }

    fn alloc_id(&mut self) -> OpId {
        let id = self.next_op_id;
        self.next_op_id += 1;
        id
    }
}

impl Backend for RecordingBackend {
    fn self_node(&self) -> SelfNode {
        SelfNode::default()
    }

    fn peers(&self) -> Vec<Peer> {
        Vec::new()
    }

    fn list(&self, _path: &str) -> Vec<mde_files::model::FileRow> {
        Vec::new()
    }

    fn audit_log(&self) -> Vec<AuditEntry> {
        self.audit.iter().rev().cloned().collect()
    }

    fn send_to(
        &mut self,
        sources: &[PathBuf],
        destination: Destination,
        mode: SendMode,
        _conflict: ConflictPolicy,
    ) -> Result<OpId, BackendError> {
        if sources.is_empty() {
            return Err(BackendError::Rejected("empty source list".into()));
        }
        let id = self.alloc_id();
        self.audit.push(AuditEntry {
            op_id: id,
            kind: "send_to",
            source: sources[0].clone(),
            destination,
            mode,
            bytes: 0,
            at_ms: 0,
            ok: true,
        });
        Ok(id)
    }

    fn rollback(&mut self, op_id: OpId) -> Result<OpId, BackendError> {
        let original = self.audit.iter().find(|a| a.op_id == op_id).cloned();
        let Some(original) = original else {
            return Err(BackendError::NotFound(op_id));
        };
        let id = self.alloc_id();
        self.audit.push(AuditEntry {
            op_id: id,
            kind: "rollback",
            source: original.source.clone(),
            destination: original.destination.clone(),
            mode: original.mode,
            bytes: original.bytes,
            at_ms: 0,
            ok: true,
        });
        Ok(id)
    }
}

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
    let mut backend = RecordingBackend::new();
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
    let mut backend = RecordingBackend::new();
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
    let mut backend = RecordingBackend::new();
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
    let mut backend = RecordingBackend::new();
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
        let mut backend = RecordingBackend::new();
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
