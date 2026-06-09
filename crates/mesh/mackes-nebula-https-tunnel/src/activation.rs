//! NF-1.4 — activation state machine.
//!
//! Port of `mackesd/src/https_fallback.rs`'s `HttpsFallbackState`
//! + `FailureWindow` into the new tunnel crate. The original
//! module stays in mackesd until NF-4.5 retires it; both
//! implementations are kept identical (same locks, same test
//! surface) so the eventual switchover is a one-line
//! re-export change in mackesd's lib.rs, not a behaviour
//! change.
//!
//! **Locked invariants** (carried verbatim from the v12.18
//! design that locked the policy in 2026-05-19, reconfirmed
//! 2026-05-23 for the v2.5 Nebula Fabric cut):
//!
//!   * Activate after **3 consecutive failed direct-UDP +
//!     lighthouse-relay probe pairs** within a 30 s window.
//!     One "failure cycle" = direct UDP probe AND lighthouse
//!     relay probe both failing in the same observation
//!     window. Two cycles = wait; three = activate.
//!   * Activated transport stays active until a fresh
//!     direct-UDP OR lighthouse-relay probe succeeds, at
//!     which point we revert to the upstream path.
//!   * Pure-fn / pure-data — testable in microseconds. No
//!     IO, no async, no time source (the failure-window
//!     observation window is driven by the calling worker's
//!     existing tick cadence).
//!
//! The state machine is a separate concern from the actual
//! TLS handshake (lives in [`crate::tls`]) so the worker
//! that drives transitions can swap in a stub for tests.

/// Observed outcome of one probe pair (direct-UDP +
/// lighthouse-relay) in a single observation window. The
/// connectivity worker emits one of these per probe cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbePairOutcome {
    /// At least one of (direct-UDP, lighthouse-relay)
    /// succeeded — the peer is reachable via a UDP path.
    AnyUdpSucceeded,
    /// Both direct-UDP and lighthouse-relay failed in the
    /// same window — the UDP-only path is wholly down.
    BothUdpFailed,
}

/// Locked failure threshold. Three consecutive
/// `BothUdpFailed` outcomes = activate the HTTPS-tunnel
/// transport.
pub const FAILURE_THRESHOLD: u32 = 3;

/// Sliding-window counter that tracks consecutive UDP-only
/// failures. Resets to 0 on any `AnyUdpSucceeded`
/// observation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FailureWindow {
    consecutive_failures: u32,
}

impl FailureWindow {
    /// Construct a fresh window with no failures yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// NF-4.5 (v2.5, 2026-05-24) — seed the window from an
    /// existing per-peer failure counter so the policy layer
    /// (`crate::https_fallback::observe_peer` in `mackesd`)
    /// can hydrate state from a `PeerPath` snapshot without
    /// replaying every previous probe. The retired
    /// `mackesd::https_fallback` v1.x copy did this with a
    /// direct field-init; we expose it as a constructor so
    /// the field can stay private.
    #[must_use]
    pub fn from_consecutive_failures(n: u32) -> Self {
        Self {
            consecutive_failures: n,
        }
    }

    /// Feed one probe-pair outcome. Returns the new failure
    /// count.
    pub fn observe(&mut self, outcome: ProbePairOutcome) -> u32 {
        match outcome {
            ProbePairOutcome::BothUdpFailed => {
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
            }
            ProbePairOutcome::AnyUdpSucceeded => {
                self.consecutive_failures = 0;
            }
        }
        self.consecutive_failures
    }

    /// Current consecutive failure count.
    #[must_use]
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// `true` when the failure count has reached
    /// [`FAILURE_THRESHOLD`] — caller should activate the
    /// HTTPS-tunnel transport.
    #[must_use]
    pub fn threshold_met(&self) -> bool {
        self.consecutive_failures >= FAILURE_THRESHOLD
    }
}

/// HTTPS-tunnel activation state machine. The connectivity
/// worker drives transitions; the tunnel transport reads
/// [`HttpsFallbackState::is_active`] to decide whether to
/// spray packets over the HTTPS path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HttpsFallbackState {
    /// Default state. Direct-UDP / lighthouse-relay paths are
    /// healthy.
    #[default]
    Inactive,
    /// Failure threshold met; TLS handshake in flight.
    /// Routing layer treats as "soon-to-be-active"; the panel
    /// shows a brief "connecting via HTTPS…" toast.
    Activating,
    /// Tunnel up + carrying traffic.
    Active,
    /// Tunnel was up but TLS handshake or TCP failed;
    /// reverting. From here, `AnyUdpSucceeded` → Inactive;
    /// another threshold cycle → Activating (retry).
    Failing,
}

impl HttpsFallbackState {
    /// `true` when the routing layer should send packets over
    /// the HTTPS tunnel.
    #[must_use]
    pub fn is_active(self) -> bool {
        matches!(self, Self::Active)
    }

    /// `true` when the UI should surface the "connecting via
    /// HTTPS…" toast.
    #[must_use]
    pub fn is_activating(self) -> bool {
        matches!(self, Self::Activating)
    }
}

/// One input to the state machine. The connectivity worker
/// emits `Probe(_)`; the TLS-handshake task emits
/// `HandshakeOk` / `HandshakeFailed`; the active-tunnel task
/// emits `TunnelLost` on a broken socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionInput {
    /// One probe-pair outcome.
    Probe(ProbePairOutcome),
    /// TLS handshake completed successfully.
    HandshakeOk,
    /// TLS handshake failed.
    HandshakeFailed,
    /// Active tunnel's TCP connection broke.
    TunnelLost,
}

/// Apply one input to the (state, window) pair. Returns the
/// new state; the window is mutated in place.
///
/// Transition table:
///
/// | From → Input            | New state    | Window |
/// |-------------------------|--------------|--------|
/// | Inactive + Probe(Both) ×3 | Activating | reset  |
/// | Activating + HandshakeOk | Active      | —      |
/// | Activating + HandshakeFailed | Failing | —      |
/// | Active + Probe(Any)     | Inactive     | reset  |
/// | Active + TunnelLost     | Failing      | —      |
/// | Failing + Probe(Any)    | Inactive     | reset  |
/// | Failing + Probe(Both) ×3 | Activating  | reset  |
#[must_use]
pub fn transition(
    state: HttpsFallbackState,
    window: &mut FailureWindow,
    input: TransitionInput,
) -> HttpsFallbackState {
    match (state, input) {
        // From Inactive — tally failures, activate on threshold.
        (HttpsFallbackState::Inactive, TransitionInput::Probe(outcome)) => {
            window.observe(outcome);
            if window.threshold_met() {
                *window = FailureWindow::new();
                HttpsFallbackState::Activating
            } else {
                HttpsFallbackState::Inactive
            }
        }
        (HttpsFallbackState::Inactive, _) => HttpsFallbackState::Inactive,

        // From Activating — wait for handshake outcome.
        (HttpsFallbackState::Activating, TransitionInput::HandshakeOk) => {
            HttpsFallbackState::Active
        }
        (HttpsFallbackState::Activating, TransitionInput::HandshakeFailed) => {
            HttpsFallbackState::Failing
        }
        (HttpsFallbackState::Activating, _) => HttpsFallbackState::Activating,

        // From Active — revert on UDP recovery; flip to Failing
        // on tunnel loss.
        (HttpsFallbackState::Active, TransitionInput::Probe(ProbePairOutcome::AnyUdpSucceeded)) => {
            *window = FailureWindow::new();
            HttpsFallbackState::Inactive
        }
        (HttpsFallbackState::Active, TransitionInput::TunnelLost) => HttpsFallbackState::Failing,
        (HttpsFallbackState::Active, _) => HttpsFallbackState::Active,

        // From Failing — recovery → Inactive; re-threshold →
        // Activating (retry).
        (
            HttpsFallbackState::Failing,
            TransitionInput::Probe(ProbePairOutcome::AnyUdpSucceeded),
        ) => {
            *window = FailureWindow::new();
            HttpsFallbackState::Inactive
        }
        (HttpsFallbackState::Failing, TransitionInput::Probe(ProbePairOutcome::BothUdpFailed)) => {
            window.observe(ProbePairOutcome::BothUdpFailed);
            if window.threshold_met() {
                *window = FailureWindow::new();
                HttpsFallbackState::Activating
            } else {
                HttpsFallbackState::Failing
            }
        }
        (HttpsFallbackState::Failing, _) => HttpsFallbackState::Failing,
    }
}

// ---- NF-4.5 — peer_path enum <-> activation enum bridge ----
//
// The bridges have to live in either this crate or the
// mackes-transport crate per Rust's orphan rule. This crate
// owns the canonical activation enum, so the impls are a
// better fit here than in mackes-transport (which would
// otherwise need to know about the activation state machine
// it's bridging away from). Added 2026-05-24 as part of
// retiring the parallel `mackesd::https_fallback` copy.

impl From<HttpsFallbackState> for mackes_transport::peer_path::HttpsFallbackState {
    fn from(s: HttpsFallbackState) -> Self {
        match s {
            HttpsFallbackState::Inactive => Self::Inactive,
            HttpsFallbackState::Activating => Self::Activating,
            HttpsFallbackState::Active => Self::Active,
            HttpsFallbackState::Failing => Self::Failing,
        }
    }
}

impl From<mackes_transport::peer_path::HttpsFallbackState> for HttpsFallbackState {
    fn from(s: mackes_transport::peer_path::HttpsFallbackState) -> Self {
        match s {
            mackes_transport::peer_path::HttpsFallbackState::Inactive => Self::Inactive,
            mackes_transport::peer_path::HttpsFallbackState::Activating => Self::Activating,
            mackes_transport::peer_path::HttpsFallbackState::Active => Self::Active,
            mackes_transport::peer_path::HttpsFallbackState::Failing => Self::Failing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- FailureWindow ----

    #[test]
    fn window_starts_at_zero() {
        let w = FailureWindow::new();
        assert_eq!(w.consecutive_failures(), 0);
        assert!(!w.threshold_met());
    }

    #[test]
    fn window_increments_on_both_failed() {
        let mut w = FailureWindow::new();
        assert_eq!(w.observe(ProbePairOutcome::BothUdpFailed), 1);
        assert_eq!(w.observe(ProbePairOutcome::BothUdpFailed), 2);
        assert_eq!(w.observe(ProbePairOutcome::BothUdpFailed), 3);
        assert!(w.threshold_met());
    }

    #[test]
    fn window_resets_on_any_udp_succeeded() {
        let mut w = FailureWindow::new();
        w.observe(ProbePairOutcome::BothUdpFailed);
        w.observe(ProbePairOutcome::BothUdpFailed);
        w.observe(ProbePairOutcome::AnyUdpSucceeded);
        assert_eq!(w.consecutive_failures(), 0);
        assert!(!w.threshold_met());
    }

    #[test]
    fn window_saturates_safely() {
        let mut w = FailureWindow {
            consecutive_failures: u32::MAX - 1,
        };
        w.observe(ProbePairOutcome::BothUdpFailed); // → u32::MAX
        w.observe(ProbePairOutcome::BothUdpFailed); // → still u32::MAX, no panic
        assert_eq!(w.consecutive_failures(), u32::MAX);
    }

    // ---- HttpsFallbackState ----

    #[test]
    fn default_state_is_inactive() {
        assert_eq!(HttpsFallbackState::default(), HttpsFallbackState::Inactive);
        assert!(!HttpsFallbackState::Inactive.is_active());
        assert!(!HttpsFallbackState::Inactive.is_activating());
    }

    #[test]
    fn is_active_only_for_active() {
        assert!(!HttpsFallbackState::Inactive.is_active());
        assert!(!HttpsFallbackState::Activating.is_active());
        assert!(HttpsFallbackState::Active.is_active());
        assert!(!HttpsFallbackState::Failing.is_active());
    }

    #[test]
    fn is_activating_only_for_activating() {
        assert!(!HttpsFallbackState::Inactive.is_activating());
        assert!(HttpsFallbackState::Activating.is_activating());
        assert!(!HttpsFallbackState::Active.is_activating());
        assert!(!HttpsFallbackState::Failing.is_activating());
    }

    // ---- transition() ----

    #[test]
    fn inactive_to_activating_after_three_failures() {
        let mut w = FailureWindow::new();
        let s = HttpsFallbackState::Inactive;
        let s = transition(
            s,
            &mut w,
            TransitionInput::Probe(ProbePairOutcome::BothUdpFailed),
        );
        assert_eq!(s, HttpsFallbackState::Inactive);
        let s = transition(
            s,
            &mut w,
            TransitionInput::Probe(ProbePairOutcome::BothUdpFailed),
        );
        assert_eq!(s, HttpsFallbackState::Inactive);
        let s = transition(
            s,
            &mut w,
            TransitionInput::Probe(ProbePairOutcome::BothUdpFailed),
        );
        assert_eq!(s, HttpsFallbackState::Activating);
        assert_eq!(w.consecutive_failures(), 0, "window resets on activation");
    }

    #[test]
    fn inactive_stays_inactive_when_udp_recovers_mid_window() {
        let mut w = FailureWindow::new();
        let mut s = HttpsFallbackState::Inactive;
        s = transition(
            s,
            &mut w,
            TransitionInput::Probe(ProbePairOutcome::BothUdpFailed),
        );
        s = transition(
            s,
            &mut w,
            TransitionInput::Probe(ProbePairOutcome::AnyUdpSucceeded),
        );
        assert_eq!(s, HttpsFallbackState::Inactive);
        assert_eq!(w.consecutive_failures(), 0);
    }

    #[test]
    fn activating_to_active_on_handshake_ok() {
        let mut w = FailureWindow::new();
        let s = transition(
            HttpsFallbackState::Activating,
            &mut w,
            TransitionInput::HandshakeOk,
        );
        assert_eq!(s, HttpsFallbackState::Active);
    }

    #[test]
    fn activating_to_failing_on_handshake_failed() {
        let mut w = FailureWindow::new();
        let s = transition(
            HttpsFallbackState::Activating,
            &mut w,
            TransitionInput::HandshakeFailed,
        );
        assert_eq!(s, HttpsFallbackState::Failing);
    }

    #[test]
    fn active_to_inactive_on_udp_recovery() {
        let mut w = FailureWindow::new();
        let s = transition(
            HttpsFallbackState::Active,
            &mut w,
            TransitionInput::Probe(ProbePairOutcome::AnyUdpSucceeded),
        );
        assert_eq!(s, HttpsFallbackState::Inactive);
    }

    #[test]
    fn active_to_failing_on_tunnel_lost() {
        let mut w = FailureWindow::new();
        let s = transition(
            HttpsFallbackState::Active,
            &mut w,
            TransitionInput::TunnelLost,
        );
        assert_eq!(s, HttpsFallbackState::Failing);
    }

    #[test]
    fn active_ignores_both_failed_probe() {
        // Already routing around UDP failure; another
        // both-failed observation while Active is a no-op.
        let mut w = FailureWindow::new();
        let s = transition(
            HttpsFallbackState::Active,
            &mut w,
            TransitionInput::Probe(ProbePairOutcome::BothUdpFailed),
        );
        assert_eq!(s, HttpsFallbackState::Active);
    }

    #[test]
    fn failing_to_inactive_on_recovery() {
        let mut w = FailureWindow::new();
        let s = transition(
            HttpsFallbackState::Failing,
            &mut w,
            TransitionInput::Probe(ProbePairOutcome::AnyUdpSucceeded),
        );
        assert_eq!(s, HttpsFallbackState::Inactive);
    }

    #[test]
    fn failing_retries_activating_on_threshold() {
        let mut w = FailureWindow::new();
        let mut s = HttpsFallbackState::Failing;
        s = transition(
            s,
            &mut w,
            TransitionInput::Probe(ProbePairOutcome::BothUdpFailed),
        );
        assert_eq!(s, HttpsFallbackState::Failing);
        s = transition(
            s,
            &mut w,
            TransitionInput::Probe(ProbePairOutcome::BothUdpFailed),
        );
        assert_eq!(s, HttpsFallbackState::Failing);
        s = transition(
            s,
            &mut w,
            TransitionInput::Probe(ProbePairOutcome::BothUdpFailed),
        );
        assert_eq!(s, HttpsFallbackState::Activating);
        assert_eq!(w.consecutive_failures(), 0);
    }

    #[test]
    fn handshake_inputs_in_inactive_are_noops() {
        let mut w = FailureWindow::new();
        let s = transition(
            HttpsFallbackState::Inactive,
            &mut w,
            TransitionInput::HandshakeOk,
        );
        assert_eq!(s, HttpsFallbackState::Inactive);
        let s = transition(s, &mut w, TransitionInput::HandshakeFailed);
        assert_eq!(s, HttpsFallbackState::Inactive);
        let s = transition(s, &mut w, TransitionInput::TunnelLost);
        assert_eq!(s, HttpsFallbackState::Inactive);
    }

    #[test]
    fn activating_ignores_probe_outcomes() {
        let mut w = FailureWindow::new();
        let s = transition(
            HttpsFallbackState::Activating,
            &mut w,
            TransitionInput::Probe(ProbePairOutcome::BothUdpFailed),
        );
        assert_eq!(s, HttpsFallbackState::Activating);
        let s = transition(
            s,
            &mut w,
            TransitionInput::Probe(ProbePairOutcome::AnyUdpSucceeded),
        );
        assert_eq!(s, HttpsFallbackState::Activating);
    }

    #[test]
    fn full_lifecycle_walk_inactive_to_active_to_inactive() {
        let mut w = FailureWindow::new();
        let mut s = HttpsFallbackState::Inactive;
        for _ in 0..FAILURE_THRESHOLD {
            s = transition(
                s,
                &mut w,
                TransitionInput::Probe(ProbePairOutcome::BothUdpFailed),
            );
        }
        assert_eq!(s, HttpsFallbackState::Activating);
        s = transition(s, &mut w, TransitionInput::HandshakeOk);
        assert_eq!(s, HttpsFallbackState::Active);
        s = transition(
            s,
            &mut w,
            TransitionInput::Probe(ProbePairOutcome::AnyUdpSucceeded),
        );
        assert_eq!(s, HttpsFallbackState::Inactive);
    }

    #[test]
    fn full_lifecycle_walk_active_lost_recovered() {
        let mut w = FailureWindow::new();
        let mut s = HttpsFallbackState::Active;
        s = transition(s, &mut w, TransitionInput::TunnelLost);
        assert_eq!(s, HttpsFallbackState::Failing);
        s = transition(
            s,
            &mut w,
            TransitionInput::Probe(ProbePairOutcome::AnyUdpSucceeded),
        );
        assert_eq!(s, HttpsFallbackState::Inactive);
    }

    #[test]
    fn failure_threshold_constant_is_3() {
        // Lock the survey-locked threshold. Bumping it requires
        // a worklist entry + design-doc lock change.
        assert_eq!(FAILURE_THRESHOLD, 3);
    }

    #[test]
    fn activating_ignores_tunnel_lost() {
        let mut w = FailureWindow::new();
        let s = transition(
            HttpsFallbackState::Activating,
            &mut w,
            TransitionInput::TunnelLost,
        );
        assert_eq!(s, HttpsFallbackState::Activating);
    }

    #[test]
    fn active_to_active_on_handshake_inputs_noop() {
        let mut w = FailureWindow::new();
        let s = transition(
            HttpsFallbackState::Active,
            &mut w,
            TransitionInput::HandshakeOk,
        );
        assert_eq!(s, HttpsFallbackState::Active);
        let s = transition(s, &mut w, TransitionInput::HandshakeFailed);
        assert_eq!(s, HttpsFallbackState::Active);
    }

    #[test]
    fn failing_to_failing_on_handshake_inputs_noop() {
        let mut w = FailureWindow::new();
        let s = transition(
            HttpsFallbackState::Failing,
            &mut w,
            TransitionInput::HandshakeOk,
        );
        assert_eq!(s, HttpsFallbackState::Failing);
        let s = transition(s, &mut w, TransitionInput::HandshakeFailed);
        assert_eq!(s, HttpsFallbackState::Failing);
        let s = transition(s, &mut w, TransitionInput::TunnelLost);
        assert_eq!(s, HttpsFallbackState::Failing);
    }
}
