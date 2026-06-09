//! Audit log + hash chain (Phase 12.6.3 + 12.10.3).
//!
//! The `events` table is append-only. Every row carries a
//! `prev_hash` field computed as `SHA-256(prev_row.hash || payload
//! || ts_le_bytes)`. `mackesd audit verify` walks the chain forward
//! and reports the first row whose `prev_hash` doesn't match the
//! previous row's hash.
//!
//! Per 12.10.3, a broken chain is a serious finding — either the
//! store was tampered with or there's a `mackesd` bug. The verify
//! routine returns the first break point so the operator can scope
//! their forensics.

use sha2::{Digest, Sha256};

/// Hash chain element. Each row has a payload (serialized event
/// data), a timestamp, and a 32-byte SHA-256 hash computed from
/// `previous.hash || payload || timestamp.to_le_bytes()`.
#[derive(Debug, Clone)]
pub struct AuditRow {
    /// Stable monotonic identifier — column `event_id` in the SQL
    /// schema, but exposed as `u64` here so the verify routine
    /// doesn't need a SQL dependency.
    pub event_id: u64,
    /// Serialized event payload (the full event JSON as bytes).
    pub payload: Vec<u8>,
    /// Unix epoch milliseconds. Stored little-endian in the hash
    /// preimage so encoding is deterministic across architectures.
    pub timestamp_ms: i64,
    /// 32-byte SHA-256 of `prev_hash || payload || timestamp_le_bytes`.
    /// On the genesis row this is the SHA-256 of an all-zero
    /// 32-byte prefix.
    pub hash: [u8; 32],
}

/// Compute the hash for a row given its predecessor. The genesis
/// case (no predecessor) is handled by passing `&[0u8; 32]` as
/// `prev_hash`.
#[must_use]
pub fn next_hash(prev_hash: &[u8; 32], payload: &[u8], timestamp_ms: i64) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(prev_hash);
    hasher.update(payload);
    hasher.update(timestamp_ms.to_le_bytes());
    hasher.finalize().into()
}

/// Outcome of an audit-chain verification pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyOutcome {
    /// Every row's hash matches the recomputation.
    Intact {
        /// Number of rows checked.
        verified: usize,
        /// Hash of the most recent row.
        head: [u8; 32],
    },
    /// First broken row. Anything *after* this row is suspect.
    Break {
        /// `event_id` of the offending row.
        at_event: u64,
        /// What the hash should have been.
        expected: [u8; 32],
        /// What the row actually carried.
        actual: [u8; 32],
    },
    /// Empty chain — no rows to verify.
    Empty,
}

/// Walk the chain forward and verify every row's hash.
///
/// Rows are assumed to be in `event_id` ascending order (the SQL
/// query that feeds this routine orders by `event_id ASC`). The
/// first row's `prev_hash` is treated as all-zeros (genesis).
#[must_use]
pub fn verify(rows: &[AuditRow]) -> VerifyOutcome {
    if rows.is_empty() {
        return VerifyOutcome::Empty;
    }

    let mut prev_hash = [0u8; 32];
    for row in rows {
        let expected = next_hash(&prev_hash, &row.payload, row.timestamp_ms);
        if expected != row.hash {
            return VerifyOutcome::Break {
                at_event: row.event_id,
                expected,
                actual: row.hash,
            };
        }
        prev_hash = row.hash;
    }
    VerifyOutcome::Intact {
        verified: rows.len(),
        head: prev_hash,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_row(event_id: u64, payload: &[u8], ts: i64, prev: &[u8; 32]) -> AuditRow {
        AuditRow {
            event_id,
            payload: payload.to_vec(),
            timestamp_ms: ts,
            hash: next_hash(prev, payload, ts),
        }
    }

    #[test]
    fn empty_chain_reports_empty() {
        assert_eq!(verify(&[]), VerifyOutcome::Empty);
    }

    #[test]
    fn single_row_chain_verifies() {
        let row = make_row(1, b"hello", 1_000, &[0u8; 32]);
        let head = row.hash;
        match verify(&[row]) {
            VerifyOutcome::Intact { verified, head: h } => {
                assert_eq!(verified, 1);
                assert_eq!(h, head);
            }
            other => panic!("expected Intact, got {other:?}"),
        }
    }

    #[test]
    fn multi_row_chain_verifies() {
        let r1 = make_row(1, b"first", 1_000, &[0u8; 32]);
        let r2 = make_row(2, b"second", 2_000, &r1.hash);
        let r3 = make_row(3, b"third", 3_000, &r2.hash);
        match verify(&[r1, r2, r3]) {
            VerifyOutcome::Intact { verified, .. } => assert_eq!(verified, 3),
            other => panic!("expected Intact, got {other:?}"),
        }
    }

    #[test]
    fn tampered_row_detected() {
        let r1 = make_row(1, b"first", 1_000, &[0u8; 32]);
        let mut r2 = make_row(2, b"second", 2_000, &r1.hash);
        // Tamper: rewrite the payload but keep the old hash.
        r2.payload = b"hacked".to_vec();
        let r3 = make_row(3, b"third", 3_000, &r2.hash);
        match verify(&[r1, r2.clone(), r3]) {
            VerifyOutcome::Break { at_event, .. } => {
                assert_eq!(at_event, 2);
            }
            other => panic!("expected Break at event 2, got {other:?}"),
        }
    }

    #[test]
    fn next_hash_is_deterministic() {
        let h1 = next_hash(&[0u8; 32], b"payload", 1_234);
        let h2 = next_hash(&[0u8; 32], b"payload", 1_234);
        assert_eq!(h1, h2);
    }

    #[test]
    fn next_hash_changes_on_any_input_change() {
        let baseline = next_hash(&[0u8; 32], b"payload", 1_234);
        assert_ne!(baseline, next_hash(&[1u8; 32], b"payload", 1_234));
        assert_ne!(baseline, next_hash(&[0u8; 32], b"payload2", 1_234));
        assert_ne!(baseline, next_hash(&[0u8; 32], b"payload", 1_235));
    }

    #[test]
    fn first_row_break_detected_at_event_one() {
        // Single row with corrupted hash should report Break at event 1.
        let mut r1 = make_row(1, b"first", 1_000, &[0u8; 32]);
        r1.hash = [0xffu8; 32]; // tamper
        match verify(&[r1]) {
            VerifyOutcome::Break {
                at_event,
                expected,
                actual,
            } => {
                assert_eq!(at_event, 1);
                assert_eq!(actual, [0xffu8; 32]);
                assert_ne!(expected, actual);
            }
            other => panic!("expected Break at 1, got {other:?}"),
        }
    }

    #[test]
    fn intact_chain_returns_head_matching_last_hash() {
        let r1 = make_row(1, b"a", 100, &[0u8; 32]);
        let r2 = make_row(2, b"b", 200, &r1.hash);
        let last_hash = r2.hash;
        match verify(&[r1, r2]) {
            VerifyOutcome::Intact { verified, head } => {
                assert_eq!(verified, 2);
                assert_eq!(head, last_hash);
            }
            other => panic!("expected Intact, got {other:?}"),
        }
    }

    #[test]
    fn tampered_timestamp_detected() {
        // Recompute with a different timestamp than what the row carries.
        let mut r1 = make_row(1, b"first", 1_000, &[0u8; 32]);
        r1.timestamp_ms = 9_999; // tamper after-the-fact
        match verify(&[r1]) {
            VerifyOutcome::Break { at_event, .. } => assert_eq!(at_event, 1),
            other => panic!("expected Break, got {other:?}"),
        }
    }

    #[test]
    fn verify_outcome_equality_for_intact() {
        let a = VerifyOutcome::Intact {
            verified: 2,
            head: [1u8; 32],
        };
        let b = VerifyOutcome::Intact {
            verified: 2,
            head: [1u8; 32],
        };
        let c = VerifyOutcome::Intact {
            verified: 3,
            head: [1u8; 32],
        };
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn empty_payload_still_hashes() {
        // Empty payload is a legitimate audit row (e.g. a heartbeat
        // event with no body). Verify it round-trips.
        let r = make_row(1, b"", 1_000, &[0u8; 32]);
        match verify(&[r]) {
            VerifyOutcome::Intact { verified, .. } => assert_eq!(verified, 1),
            other => panic!("expected Intact, got {other:?}"),
        }
    }
}
