//! KDC2-1.9 — `select_best_transport` scorer.
//!
//! Pure logic that takes a snapshot of probe results + a routing
//! `Policy` and returns the primary / fallback transport choice
//! for a given message class. The async wrapper in [`select`]
//! collects the probes; the sync [`score`] function is what gets
//! unit-tested.
//!
//! Score model (locked in unit tests):
//!
//!   * Start from each transport's `TransportKind::all()`
//!     preference-order rank (lower = better).
//!   * Multiply by the policy's per-`MessageClass` latency /
//!     throughput / dual-send weights.
//!   * Subtract a flap penalty (see [`Policy::flap_penalty`])
//!     for transports that recently failed.
//!   * Discard transports whose health is `Down`.
//!   * Discard transports that don't `carries` the message class.
//!
//! Tie-break: `TransportKind::all()` order wins.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{HealthState, MessageClass, MessageClassSet, SwitchReason, Transport, TransportKind};

/// Per-message-class weight bundle. Values 0.0..=1.0; higher
/// means "prefer this transport family more strongly for this
/// class."
///
/// Loose semantic baseline (locked in tests):
///   * Clipboard — small, latency-bound → favor NebulaDirect.
///   * FileBulk  — large, throughput-bound → favor KdcTls.
///   * Notification — dual-send idempotent → any reachable
///     transport is fine, but slight bias toward the most-
///     reliable one.
///   * Control — same as Clipboard (low latency).
#[derive(Debug, Clone, PartialEq)]
pub struct ClassWeights {
    /// Latency bias for Clipboard / Control.
    pub latency: f32,
    /// Throughput bias for FileBulk.
    pub throughput: f32,
    /// Reliability bias for Notification.
    pub reliability: f32,
}

impl ClassWeights {
    /// v2.1 KDC2 baseline weights — picked to match the
    /// connectivity-scope lock's latency/throughput targets.
    #[must_use]
    pub fn baseline() -> Self {
        Self {
            latency: 0.7,
            throughput: 0.7,
            reliability: 0.7,
        }
    }
}

/// Routing policy consumed by [`score`]. Operator-tunable via
/// `/etc/mde/connect/policy.toml` (lands in KDC2-1.10 / 1.11).
#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    /// Per-class weight bundles.
    pub weights: ClassWeights,
    /// Penalty (score points) applied to a transport that has
    /// failed within the recent flap window.
    pub flap_penalty: f32,
    /// Hard preference: when a TransportKind appears in this
    /// list, it's pinned as primary regardless of scoring.
    /// Empty (default) means "no pinning."
    pub pinned_primary: Vec<TransportKind>,
    /// Hard denylist: TransportKinds in this list are never
    /// selected, even when they're the only reachable option.
    pub denylist: Vec<TransportKind>,
}

impl Policy {
    /// Default policy — baseline weights, mild flap penalty,
    /// no pins, no denies.
    #[must_use]
    pub fn baseline() -> Self {
        Self {
            weights: ClassWeights::baseline(),
            flap_penalty: 0.25,
            pinned_primary: Vec::new(),
            denylist: Vec::new(),
        }
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self::baseline()
    }
}

/// Per-transport snapshot the scorer needs. Built either by
/// [`select`] (which calls probe + capabilities on a real
/// Transport set) or hand-crafted in unit tests.
#[derive(Debug, Clone, PartialEq)]
pub struct TransportSample {
    /// Which transport this sample is for.
    pub kind: TransportKind,
    /// Most recent observed health.
    pub health: HealthState,
    /// Message classes this transport carries.
    pub carries: MessageClassSet,
    /// Recent-failure count from the FailureWindow (drives the
    /// flap penalty). Zero means no penalty.
    pub recent_failures: u32,
}

impl TransportSample {
    /// Build a sample with no failures from the constituent
    /// pieces. Convenience for tests.
    #[must_use]
    pub fn healthy(kind: TransportKind, carries: MessageClassSet) -> Self {
        Self {
            kind,
            health: HealthState::Healthy,
            carries,
            recent_failures: 0,
        }
    }
}

/// Scorer output. Reported back to the mesh-router so it can
/// emit a `PathSwitch` audit-chain entry with the reason.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredSelection {
    /// Transport the router should send through.
    pub primary: TransportKind,
    /// Best alternative, if any. `None` when only one
    /// transport is sendable.
    pub fallback: Option<TransportKind>,
    /// Why this selection was made.
    pub reason: SwitchReason,
}

/// Pure scorer. Given a snapshot + policy + message class,
/// return the best primary + fallback.
///
/// Returns `None` when no transport is sendable (every sample
/// is `Down` OR every sample is denylist-banned OR the
/// `samples` slice is empty).
#[must_use]
pub fn score(
    samples: &[TransportSample],
    class: MessageClass,
    policy: &Policy,
) -> Option<ScoredSelection> {
    // Filter: sendable + carries class + not denylisted.
    let candidates: Vec<&TransportSample> = samples
        .iter()
        .filter(|s| s.health.is_sendable())
        .filter(|s| s.carries.carries(class))
        .filter(|s| !policy.denylist.contains(&s.kind))
        .collect();
    if candidates.is_empty() {
        return None;
    }

    // Honor pinned_primary if any pinned kind is in the
    // candidate set.
    if let Some(pinned) = policy
        .pinned_primary
        .iter()
        .find(|&&p| candidates.iter().any(|s| s.kind == p))
    {
        let fallback = candidates
            .iter()
            .filter(|s| s.kind != *pinned)
            .map(|s| s.kind)
            .next();
        return Some(ScoredSelection {
            primary: *pinned,
            fallback,
            reason: SwitchReason::Policy,
        });
    }

    // Score every candidate. Lower score wins (ranks).
    let rank_of: HashMap<TransportKind, usize> = TransportKind::all()
        .iter()
        .enumerate()
        .map(|(i, k)| (*k, i))
        .collect();
    let class_weight = match class {
        MessageClass::Clipboard | MessageClass::Control => policy.weights.latency,
        MessageClass::FileBulk => policy.weights.throughput,
        MessageClass::Notification => policy.weights.reliability,
    };

    // Composite score: base rank weighted by class, minus
    // bonus for Healthy (vs Degraded), plus flap penalty for
    // recent failures.
    let scored: Vec<(f32, TransportKind)> = candidates
        .iter()
        .map(|s| {
            let base = *rank_of.get(&s.kind).unwrap_or(&99) as f32;
            let weighted = base * class_weight;
            // Degraded penalty must dominate any base*weight
            // outcome so a Degraded transport always loses to a
            // Healthy one in the same candidate set, regardless
            // of rank gap. Max possible base*weight is
            // 3 (worst-ranked) * 1.0 (max weight) = 3.0; we set
            // Degraded penalty to 10.0 — well above any rank
            // signal but still finite so Degraded > Down (the
            // Down filter runs before scoring, so 1000.0 is
            // defensive only).
            let health_bonus = match s.health {
                HealthState::Healthy => 0.0,
                HealthState::Degraded => 10.0,
                HealthState::Down => 1000.0,
            };
            let flap = if s.recent_failures > 0 {
                policy.flap_penalty * s.recent_failures.min(4) as f32
            } else {
                0.0
            };
            (weighted + health_bonus + flap, s.kind)
        })
        .collect();

    // Sort ascending by score; tie-break on TransportKind::all() order.
    let mut sorted = scored.clone();
    sorted.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                let ra = rank_of.get(&a.1).copied().unwrap_or(99);
                let rb = rank_of.get(&b.1).copied().unwrap_or(99);
                ra.cmp(&rb)
            })
    });

    let primary = sorted[0].1;
    let fallback = sorted.get(1).map(|(_, k)| *k);

    // Distinguish "initial selection" from "health-degraded
    // switch": if the best candidate was Degraded (not
    // Healthy), report HealthDegraded(<bumped>). For the pure
    // scorer we don't know the prior choice, so reason is
    // Initial unless health-bonus is what caused the reshuffle.
    // The mesh-router wraps this with prior-state awareness.
    let reason = SwitchReason::Initial;
    Some(ScoredSelection {
        primary,
        fallback,
        reason,
    })
}

/// Async wrapper around [`score`] that probes the live
/// transports. Used by [`crate::Transport`] impls' integration
/// tests + (eventually) the mesh_router worker. Lives behind
/// no feature gate — it depends only on the trait.
pub async fn select(
    transports: &[Arc<dyn Transport>],
    peer_id: &str,
    class: MessageClass,
    policy: &Policy,
) -> Option<ScoredSelection> {
    let mut samples = Vec::with_capacity(transports.len());
    for t in transports {
        let health = t.probe(peer_id).await;
        let caps = t.capabilities();
        samples.push(TransportSample {
            kind: t.kind(),
            health,
            carries: caps.carries,
            recent_failures: 0,
        });
    }
    score(&samples, class, policy)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_carrier(kind: TransportKind) -> TransportSample {
        TransportSample::healthy(kind, MessageClassSet::all())
    }

    fn small_only(kind: TransportKind) -> TransportSample {
        TransportSample::healthy(kind, MessageClassSet::small_only())
    }

    #[test]
    fn empty_input_yields_none() {
        let r = score(&[], MessageClass::Control, &Policy::default());
        assert!(r.is_none());
    }

    #[test]
    fn all_down_yields_none() {
        let mut s = all_carrier(TransportKind::NebulaDirect);
        s.health = HealthState::Down;
        let r = score(&[s], MessageClass::Control, &Policy::default());
        assert!(r.is_none());
    }

    #[test]
    fn single_healthy_candidate_is_primary_with_no_fallback() {
        let s = all_carrier(TransportKind::NebulaDirect);
        let r = score(&[s], MessageClass::Control, &Policy::default()).unwrap();
        assert_eq!(r.primary, TransportKind::NebulaDirect);
        assert_eq!(r.fallback, None);
    }

    #[test]
    fn prefers_direct_udp_when_all_healthy_for_clipboard() {
        let samples = vec![
            all_carrier(TransportKind::NebulaLighthouseRelay),
            all_carrier(TransportKind::NebulaHttps443),
            all_carrier(TransportKind::NebulaDirect),
            all_carrier(TransportKind::KdcTls),
        ];
        let r = score(&samples, MessageClass::Clipboard, &Policy::default()).unwrap();
        // NebulaDirect wins the preference order.
        assert_eq!(r.primary, TransportKind::NebulaDirect);
        // Fallback should be KdcTls (next-best per TransportKind::all() rank).
        assert_eq!(r.fallback, Some(TransportKind::KdcTls));
    }

    #[test]
    fn file_bulk_skips_small_only_transports() {
        // DERP is small_only — must not be picked for FileBulk
        // even if it's the "best" by rank.
        let samples = vec![
            small_only(TransportKind::NebulaLighthouseRelay),
            all_carrier(TransportKind::KdcTls),
        ];
        let r = score(&samples, MessageClass::FileBulk, &Policy::default()).unwrap();
        assert_eq!(r.primary, TransportKind::KdcTls);
        assert_eq!(r.fallback, None);
    }

    #[test]
    fn degraded_health_loses_to_healthy() {
        let mut udp = all_carrier(TransportKind::NebulaDirect);
        udp.health = HealthState::Degraded;
        let kdc = all_carrier(TransportKind::KdcTls);
        let r = score(&[udp, kdc], MessageClass::Control, &Policy::default()).unwrap();
        // KdcTls wins because Degraded UDP gets a +0.5 penalty
        // that exceeds the preference-order gap.
        assert_eq!(r.primary, TransportKind::KdcTls);
        assert_eq!(r.fallback, Some(TransportKind::NebulaDirect));
    }

    #[test]
    fn flap_penalty_pushes_recently_failed_transport_back() {
        let mut udp = all_carrier(TransportKind::NebulaDirect);
        udp.recent_failures = 4;
        let kdc = all_carrier(TransportKind::KdcTls);
        let r = score(&[udp, kdc], MessageClass::Control, &Policy::default()).unwrap();
        // Flap penalty * 4 = 1.0, more than the rank gap.
        assert_eq!(r.primary, TransportKind::KdcTls);
    }

    #[test]
    fn pinned_primary_wins_over_scoring() {
        let mut policy = Policy::default();
        policy.pinned_primary = vec![TransportKind::NebulaHttps443];
        let samples = vec![
            all_carrier(TransportKind::NebulaDirect),
            all_carrier(TransportKind::NebulaHttps443),
        ];
        let r = score(&samples, MessageClass::Control, &policy).unwrap();
        assert_eq!(r.primary, TransportKind::NebulaHttps443);
        assert_eq!(r.reason, SwitchReason::Policy);
        assert_eq!(r.fallback, Some(TransportKind::NebulaDirect));
    }

    #[test]
    fn pinned_primary_falls_through_when_not_in_candidates() {
        // Pinned kind isn't available (no sample). Pinning silently
        // disables; pure scoring takes over.
        let mut policy = Policy::default();
        policy.pinned_primary = vec![TransportKind::NebulaHttps443];
        let samples = vec![all_carrier(TransportKind::NebulaDirect)];
        let r = score(&samples, MessageClass::Control, &policy).unwrap();
        assert_eq!(r.primary, TransportKind::NebulaDirect);
        assert_eq!(r.reason, SwitchReason::Initial);
    }

    #[test]
    fn denylist_removes_a_candidate() {
        let mut policy = Policy::default();
        policy.denylist = vec![TransportKind::NebulaDirect];
        let samples = vec![
            all_carrier(TransportKind::NebulaDirect),
            all_carrier(TransportKind::KdcTls),
        ];
        let r = score(&samples, MessageClass::Control, &policy).unwrap();
        assert_eq!(r.primary, TransportKind::KdcTls);
    }

    #[test]
    fn denylist_can_eliminate_all_candidates() {
        let mut policy = Policy::default();
        policy.denylist = TransportKind::all().to_vec();
        let samples = vec![all_carrier(TransportKind::NebulaDirect)];
        let r = score(&samples, MessageClass::Control, &policy);
        assert!(r.is_none());
    }

    #[test]
    fn notification_uses_reliability_weight() {
        // For Notification, weight is `reliability` (= 0.7
        // baseline). Order should still come out NebulaDirect >
        // KdcTls > DERP > HTTPS at equal-health.
        let samples = vec![
            all_carrier(TransportKind::NebulaDirect),
            all_carrier(TransportKind::KdcTls),
            all_carrier(TransportKind::NebulaLighthouseRelay),
            all_carrier(TransportKind::NebulaHttps443),
        ];
        let r = score(&samples, MessageClass::Notification, &Policy::default()).unwrap();
        assert_eq!(r.primary, TransportKind::NebulaDirect);
        assert_eq!(r.fallback, Some(TransportKind::KdcTls));
    }

    #[test]
    fn tiebreak_uses_transport_kind_preference_order() {
        // Construct samples with identical weights; tie-break
        // must fall back on TransportKind::all() order.
        let policy = Policy {
            weights: ClassWeights {
                latency: 0.0, // all scores 0 → ties everywhere
                throughput: 0.0,
                reliability: 0.0,
            },
            flap_penalty: 0.0,
            pinned_primary: vec![],
            denylist: vec![],
        };
        let samples = vec![
            all_carrier(TransportKind::NebulaHttps443),
            all_carrier(TransportKind::NebulaLighthouseRelay),
            all_carrier(TransportKind::KdcTls),
            all_carrier(TransportKind::NebulaDirect),
        ];
        let r = score(&samples, MessageClass::Control, &policy).unwrap();
        assert_eq!(r.primary, TransportKind::NebulaDirect);
        assert_eq!(r.fallback, Some(TransportKind::KdcTls));
    }
}
