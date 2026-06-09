//! NF-4.5 (v2.5, 2026-05-24) — HTTPS-fallback bridge layer.
//!
//! This file was a 644-LOC parallel copy of the activation
//! state machine until NF-1.4 ported the canonical logic to
//! `mackes-nebula-https-tunnel::activation`. The duplicated
//! state-machine body + its 350+ lines of tests retired in
//! NF-4.5 (2026-05-24); what's left is the **per-peer wrapper**
//! that mesh_router needs:
//!
//!   * re-exports of the activation enums + state-machine fn
//!     so existing `crate::https_fallback::*` imports keep
//!     working unchanged
//!   * `observe_peer` — the per-tick wrapper that bridges
//!     `PeerPath` (mackes-transport) into the pure-function
//!     `transition` call. Lives here because it's the only
//!     `PeerPath`-aware glue and mesh_router consumes it
//!     directly.
//!
//! The single source of truth for the state machine + the
//! per-state-transition tests + the `From<HttpsFallbackState>`
//! bridges to `mackes_transport::peer_path::HttpsFallbackState`
//! is now `crates/mackes-nebula-https-tunnel/src/activation.rs`.
//! This file's tests cover the `observe_peer` `PeerPath`
//! mutation only.

pub use mackes_nebula_https_tunnel::activation::{
    transition, FailureWindow, HttpsFallbackState, ProbePairOutcome, TransitionInput,
};

/// Apply one probe-pair outcome (or handshake / tunnel signal)
/// to a peer's HTTPS-fallback state. Updates
/// `peer_path.consecutive_udp_failures` + `peer_path.https_state`
/// in place. The caller is the mesh-router worker, which
/// observes UDP probe outcomes per tick + drives this for each
/// peer it tracks.
///
/// Returns the new state for convenient logging / audit-emit.
pub fn observe_peer(
    peer_path: &mut mackes_transport::peer_path::PeerPath,
    input: TransitionInput,
) -> HttpsFallbackState {
    let mut window = FailureWindow::from_consecutive_failures(peer_path.consecutive_udp_failures);
    let new_state = transition(peer_path.https_state.into(), &mut window, input);
    peer_path.consecutive_udp_failures = window.consecutive_failures();
    peer_path.https_state = new_state.into();
    new_state
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_transport::peer_path::PeerPath as MtPath;
    use mackes_transport::TransportKind;

    fn fresh_path() -> MtPath {
        MtPath::initial("peer:test".into(), TransportKind::NebulaDirect)
    }

    #[test]
    fn observe_peer_progresses_inactive_to_activating_after_threshold() {
        let mut p = fresh_path();
        for _ in 0..2 {
            observe_peer(
                &mut p,
                TransitionInput::Probe(ProbePairOutcome::BothUdpFailed),
            );
        }
        // Threshold is 3 consecutive failures for activation.
        assert_eq!(
            HttpsFallbackState::from(p.https_state),
            HttpsFallbackState::Inactive
        );
        observe_peer(
            &mut p,
            TransitionInput::Probe(ProbePairOutcome::BothUdpFailed),
        );
        assert_eq!(
            HttpsFallbackState::from(p.https_state),
            HttpsFallbackState::Activating
        );
    }

    #[test]
    fn observe_peer_writes_consecutive_failures_back_to_path() {
        let mut p = fresh_path();
        observe_peer(
            &mut p,
            TransitionInput::Probe(ProbePairOutcome::BothUdpFailed),
        );
        assert_eq!(p.consecutive_udp_failures, 1);
        observe_peer(
            &mut p,
            TransitionInput::Probe(ProbePairOutcome::BothUdpFailed),
        );
        assert_eq!(p.consecutive_udp_failures, 2);
        // UDP success resets the counter.
        observe_peer(
            &mut p,
            TransitionInput::Probe(ProbePairOutcome::AnyUdpSucceeded),
        );
        assert_eq!(p.consecutive_udp_failures, 0);
    }

    #[test]
    fn observe_peer_handshake_ok_transitions_active_to_inactive() {
        let mut p = fresh_path();
        // Drive into Activating, then Active by a tunnel signal.
        for _ in 0..3 {
            observe_peer(
                &mut p,
                TransitionInput::Probe(ProbePairOutcome::BothUdpFailed),
            );
        }
        assert_eq!(
            HttpsFallbackState::from(p.https_state),
            HttpsFallbackState::Activating
        );
        observe_peer(&mut p, TransitionInput::HandshakeOk);
        assert_eq!(
            HttpsFallbackState::from(p.https_state),
            HttpsFallbackState::Active
        );
        // A successful UDP probe demotes back to Inactive.
        observe_peer(
            &mut p,
            TransitionInput::Probe(ProbePairOutcome::AnyUdpSucceeded),
        );
        assert_eq!(
            HttpsFallbackState::from(p.https_state),
            HttpsFallbackState::Inactive
        );
    }
}
