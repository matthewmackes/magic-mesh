//! KDC2-1.4 — `ProbeOutcome` + `HealthSnapshot` + expanded
//! `TransportError` variants.
//!
//! Where `crate::HealthState` is the coarse "is this sendable
//! right now" predicate, the types in this module carry the
//! detailed metrics the router uses to RANK candidates +
//! diagnose why one transport fell below another. KDC2-1.9's
//! `select_best_transport` scorer reads these.

use serde::{Deserialize, Serialize};

/// One probe result — what `Transport::probe` (the next iteration
/// will refactor to return this) measured against a peer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeOutcome {
    /// Round-trip time in milliseconds. `None` when the probe
    /// failed to complete (the matching `TransportError` is the
    /// reason).
    pub rtt_ms: Option<u32>,
    /// Throughput estimate in megabits per second. Optional;
    /// some transports (KDC ping) don't sample throughput at
    /// probe time.
    pub throughput_mbps_estimate: Option<f32>,
    /// Packet loss observed during this probe, 0.0..=1.0.
    /// `0.0` is best, `1.0` is total loss.
    pub packet_loss: f32,
    /// Age of the most recent successful handshake in seconds.
    /// Lower is fresher — `0` means "this probe IS the
    /// handshake." High values mean the path is up but the
    /// session is old + may need re-keying.
    pub last_handshake_age_s: u32,
}

impl ProbeOutcome {
    /// Compose a [`HealthSnapshot::score`] from this probe's
    /// metrics. Pure function — the router calls this after
    /// each probe to update the running health score.
    ///
    /// Score model (locked here as the canonical formula so
    /// the scorer doesn't drift between commits):
    ///
    ///   * Start at 1.0.
    ///   * Multiply by `(1.0 - packet_loss)`.
    ///   * Halve again when `last_handshake_age_s > 300` (5 min).
    ///   * Floor at `0.0`.
    #[must_use]
    pub fn compose_score(&self) -> f32 {
        let mut score = (1.0 - self.packet_loss).clamp(0.0, 1.0);
        if self.last_handshake_age_s > 300 {
            score *= 0.5;
        }
        score.clamp(0.0, 1.0)
    }
}

/// Running health metrics for a single (peer, transport) pair.
/// Updated on every probe; consumed by the router to rank
/// candidates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthSnapshot {
    /// Composite score 0.0..=1.0. 1.0 is "perfect," 0.0 is "as
    /// good as down." [`HealthState::Down`] takes precedence —
    /// if state is Down, score is ignored.
    pub score: f32,
    /// Consecutive failed probes since the last success. The
    /// router uses this to apply a flap penalty + escalating
    /// back-off.
    pub recent_failures: u32,
    /// Unix epoch seconds of the most recent successful probe.
    /// `0` means "never succeeded."
    pub last_success_at: i64,
}

impl HealthSnapshot {
    /// Construct a fresh "first probe" snapshot from a
    /// [`ProbeOutcome`].
    #[must_use]
    pub fn from_probe(probe: &ProbeOutcome, now_epoch_s: i64) -> Self {
        Self {
            score: probe.compose_score(),
            recent_failures: 0,
            last_success_at: now_epoch_s,
        }
    }

    /// Apply a failed-probe observation. Increments
    /// `recent_failures` + decays score by 25 %.
    pub fn observe_failure(&mut self) {
        self.recent_failures = self.recent_failures.saturating_add(1);
        self.score = (self.score * 0.75).clamp(0.0, 1.0);
    }
}

/// Expanded set of router-relevant transport errors. KDC2-1.4
/// adds `PolicyDenied / BackendBusy / Timeout` on top of the
/// `crate::TransportError` variants that already cover network-
/// level failures.
///
/// Kept separate from `crate::TransportError` (which is the
/// `Transport::open` return type) because these are
/// router-level failure categories — the router synthesizes them
/// from underlying transport errors + policy decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouterError {
    /// Peer not reachable via any registered transport.
    Unreachable,
    /// Pairing handshake failed — peer responded but credentials
    /// rejected.
    HandshakeFailed,
    /// Operator policy in `policy.toml` denied this message
    /// class on this transport.
    PolicyDenied,
    /// Backend (TLS session, ring task, etc.) is busy / saturated.
    /// Retry later.
    BackendBusy,
    /// Probe / send exceeded its time budget.
    Timeout,
}

impl RouterError {
    /// Stable audit-token for the audit chain.
    #[must_use]
    pub fn audit_token(&self) -> &'static str {
        match self {
            RouterError::Unreachable => "unreachable",
            RouterError::HandshakeFailed => "handshake_failed",
            RouterError::PolicyDenied => "policy_denied",
            RouterError::BackendBusy => "backend_busy",
            RouterError::Timeout => "timeout",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perfect_probe() -> ProbeOutcome {
        ProbeOutcome {
            rtt_ms: Some(5),
            throughput_mbps_estimate: Some(940.0),
            packet_loss: 0.0,
            last_handshake_age_s: 0,
        }
    }

    #[test]
    fn perfect_probe_composes_score_one() {
        let p = perfect_probe();
        assert!((p.compose_score() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn high_packet_loss_drops_score() {
        let mut p = perfect_probe();
        p.packet_loss = 0.5;
        assert!((p.compose_score() - 0.5).abs() < 1e-4);
        p.packet_loss = 1.0;
        assert!((p.compose_score() - 0.0).abs() < 1e-4);
    }

    #[test]
    fn stale_handshake_halves_score() {
        // last_handshake_age_s > 300 (5 min) halves the score.
        let mut p = perfect_probe();
        p.last_handshake_age_s = 301;
        assert!((p.compose_score() - 0.5).abs() < 1e-4);
        // Combined with packet loss — both multipliers apply.
        p.packet_loss = 0.2;
        // (1.0 - 0.2) * 0.5 = 0.4
        assert!((p.compose_score() - 0.4).abs() < 1e-4);
    }

    #[test]
    fn health_snapshot_from_probe_at_first_observation() {
        let now = 1_700_000_000;
        let snap = HealthSnapshot::from_probe(&perfect_probe(), now);
        assert!((snap.score - 1.0).abs() < 1e-4);
        assert_eq!(snap.recent_failures, 0);
        assert_eq!(snap.last_success_at, now);
    }

    #[test]
    fn observe_failure_increments_count_and_decays_score() {
        let mut snap = HealthSnapshot::from_probe(&perfect_probe(), 1_700_000_000);
        let before = snap.score;
        snap.observe_failure();
        assert_eq!(snap.recent_failures, 1);
        assert!(snap.score < before, "score must decay on failure");
        assert!((snap.score - 0.75).abs() < 1e-4);
        snap.observe_failure();
        assert_eq!(snap.recent_failures, 2);
        assert!((snap.score - 0.5625).abs() < 1e-4); // 0.75^2
    }

    #[test]
    fn router_error_audit_tokens_are_stable() {
        // Audit-log readers grep on these. Stability is the lock.
        assert_eq!(RouterError::Unreachable.audit_token(), "unreachable");
        assert_eq!(
            RouterError::HandshakeFailed.audit_token(),
            "handshake_failed"
        );
        assert_eq!(RouterError::PolicyDenied.audit_token(), "policy_denied");
        assert_eq!(RouterError::BackendBusy.audit_token(), "backend_busy");
        assert_eq!(RouterError::Timeout.audit_token(), "timeout");
    }
}
