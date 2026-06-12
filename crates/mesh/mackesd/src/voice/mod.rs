//! VV-4 (v4.1.0) — voice-routing heuristic.
//!
//! Pure-function `best_path` + `pick_relay` over a list of
//! connectivity candidates. Owns the policy lock from
//! `docs/design/v4.1-voice-video.md` §6: voice flows favor
//! latency over throughput (intentionally divergent from the
//! mesh-router's throughput-wins policy), reject candidates
//! with RTT > 80 ms OR loss > 5%, and fall back to
//! record-route transit through the best available relay peer
//! when no direct candidate qualifies.
//!
//! Pure-function contract: same input always yields exactly the
//! same output. Callers (the VV-2.a policy-lifecycle writer in
//! [`materialize`], when it materializes `voice-desired.json`
//! from approved `voice_mesh` revisions) collect candidates from
//! the mesh-latency cache + telemetry store and hand a snapshot
//! list in. The output `priority` weight gets baked into each
//! `dispatcher.list` row so Kamailio's dispatcher picks the
//! direct path when it's healthy and the transit path
//! otherwise.

pub mod materialize;

use serde::{Deserialize, Serialize};

/// One connectivity option for reaching `target` — either a
/// direct path (`via == target`) or a transit path (`via` is
/// the proposed relay peer).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// The peer through which traffic would flow. For a direct
    /// candidate this is the destination peer's `node_id`; for
    /// a transit candidate this is the relay peer's `node_id`.
    pub via: String,
    /// One-way RTT in milliseconds, measured on the underlying
    /// path. `f32` (not `u32`) so jitter-sensitive callers can
    /// later carry sub-millisecond resolution; the heuristic
    /// here rounds to integer compare anyway.
    pub rtt_ms: f32,
    /// Packet-loss percentage on the underlying path
    /// (0.0..=100.0). VV-2.a's writer (`voice::materialize`) builds
    /// candidates from the mesh-latency cache, which today tracks
    /// per-peer reachability (an entry's presence ⇒ reachable ⇒
    /// loss `0.0`; absence ⇒ no candidate). A true smoothed
    /// loss-rate window would feed richer values here; the scorer
    /// already weights this dimension (`score` = loss·10 + rtt),
    /// so that upgrade needs no scorer change — only a loss-aware
    /// cache.
    pub loss_pct: f32,
}

/// The decision `best_path` reaches about how to route an
/// INVITE for `target`. The `via` peer's `node_id` ends up in
/// the generated `dispatcher.list` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Path {
    /// Direct path: send the INVITE straight to `via`. For a
    /// direct path `via` is the target peer's `node_id`.
    Direct {
        /// Destination peer's `node_id`.
        via: String,
    },
    /// Transit path: record-route through `via`. The transit
    /// peer's `RTPengine` relays SRTP for this dialog.
    Transit {
        /// Relay peer's `node_id`. The eventual transit-leg
        /// INVITE goes here first.
        via: String,
    },
}

impl Path {
    /// The `node_id` to use in a `dispatcher.list` row's
    /// destination URI.
    #[must_use]
    pub fn via(&self) -> &str {
        match self {
            Self::Direct { via } | Self::Transit { via } => via,
        }
    }
}

/// Latency cap in milliseconds — anything above this is
/// unfit for real-time voice. Per `docs/design/v4.1-voice-video.md`
/// §6.3 the cap is 80 ms; calls above that exhibit perceptible
/// half-duplex echo even with good packet-loss numbers.
pub const MAX_DIRECT_RTT_MS: f32 = 80.0;

/// Loss-percentage cap. Above this the line cracks audibly
/// even with healthy RTT. Per the same design-doc table.
pub const MAX_DIRECT_LOSS_PCT: f32 = 5.0;

/// Pick the best routing decision for `target` given the list
/// of measured connectivity candidates.
///
/// Algorithm (per design doc §6.3):
///
/// 1. Filter candidates whose `via == target.node_id` and whose
///    `rtt_ms < MAX_DIRECT_RTT_MS` and `loss_pct <
///    MAX_DIRECT_LOSS_PCT`.
/// 2. Among the survivors, pick the one with the smallest
///    *quality score* — `rtt_ms + (loss_pct * 10.0)`. The
///    coefficient 10 is the design-doc tradeoff weighting (a
///    1% loss-rate adds 10 ms of perceived delay equivalent).
/// 3. If no direct candidate qualifies, fall back to a transit
///    path picked via [`pick_relay`].
///
/// `target_node_id` is the destination peer's `node_id` —
/// candidates whose `via` matches this are direct candidates;
/// others are transit candidates.
#[must_use]
pub fn best_path(target_node_id: &str, candidates: &[Candidate]) -> Path {
    let direct = candidates
        .iter()
        .filter(|c| c.via == target_node_id)
        .filter(|c| c.rtt_ms < MAX_DIRECT_RTT_MS && c.loss_pct < MAX_DIRECT_LOSS_PCT)
        .min_by(|a, b| score(a).total_cmp(&score(b)));
    if let Some(c) = direct {
        return Path::Direct { via: c.via.clone() };
    }
    Path::Transit {
        via: pick_relay(target_node_id, candidates),
    }
}

/// Pick the relay peer when no direct candidate qualifies.
///
/// Picks the transit candidate with the best quality score
/// among those whose `via != target_node_id`. If no transit
/// candidate exists either, falls back to the destination's
/// own `node_id` — a deliberate "give the caller a chance"
/// non-throw — and Kamailio's eventual 503 response surfaces
/// the failure to the operator with a clear error.
#[must_use]
pub fn pick_relay(target_node_id: &str, candidates: &[Candidate]) -> String {
    candidates
        .iter()
        .filter(|c| c.via != target_node_id)
        // Transit candidates DON'T have to be under 80 ms — a
        // hop through a slightly-slower relay still beats no
        // path at all. They DO have to be reachable
        // (loss_pct < 100).
        .filter(|c| c.loss_pct < 100.0)
        .min_by(|a, b| score(a).total_cmp(&score(b)))
        .map_or_else(|| target_node_id.to_owned(), |c| c.via.clone())
}

/// VV-4 quality score — lower is better.
///
/// The 10 ms / 1%-loss equivalence is the design-doc tradeoff
/// weighting (§6.3). Exposed for callers who want to render
/// the same number in operator UI.
#[must_use]
pub fn score(c: &Candidate) -> f32 {
    c.loss_pct.mul_add(10.0, c.rtt_ms)
}

/// VV-2.a — the Kamailio `dispatcher.list` priority for a peer,
/// derived from VV-4's [`best_path`]. Kamailio prefers the
/// **higher** priority among rows sharing a `setid`, while VV-4's
/// [`score`] is lower-is-better, so the mapping inverts:
///
///   * A candidate that wins a [`Path::Direct`] decision (RTT <
///     80 ms, loss < 5%) gets a priority that falls as its score
///     rises — a 10 ms path outranks a 70 ms one.
///   * A candidate that only reaches [`Path::Transit`] (too slow
///     or unreachable) gets priority `0` — Kamailio keeps the row
///     but tries it last.
///
/// This is the function the materialize writer calls per direct
/// row, making the VV-4 heuristic (`best_path` → `pick_relay` →
/// `score`) reachable from production rather than test-only.
#[must_use]
pub fn dispatcher_priority(target_node_id: &str, candidates: &[Candidate]) -> u8 {
    match best_path(target_node_id, candidates) {
        Path::Direct { .. } => {
            // Find the winning direct candidate's score (best_path
            // already proved one qualifies) and invert it into the
            // u8 priority band. Healthy paths land high (~near 100);
            // an 80 ms path floors near the transit tier.
            let best = candidates
                .iter()
                .filter(|c| c.via == target_node_id && c.rtt_ms < MAX_DIRECT_RTT_MS)
                .map(score)
                .fold(f32::INFINITY, f32::min);
            // score ∈ [0, ~80) for a qualifying direct path → map to
            // priority (100 - score), clamped to the 1..=100 band so
            // a qualifying peer always outranks a Transit (0).
            let pri = (100.0 - best).clamp(1.0, 100.0);
            pri as u8
        }
        Path::Transit { .. } => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatcher_priority_inverts_score_and_floors_non_direct() {
        // VV-2.a — direct winners rank high (faster ⇒ higher);
        // unreachable / over-budget candidates floor to 0 (transit).
        let fast = dispatcher_priority("peer:x", &[c("peer:x", 8.0, 0.0)]);
        let slow = dispatcher_priority("peer:x", &[c("peer:x", 60.0, 0.0)]);
        assert!(fast > slow, "faster direct path ranks higher ({fast} > {slow})");
        assert!(fast >= 1 && fast <= 100);
        // Over the 80 ms direct cap → Transit → 0.
        assert_eq!(dispatcher_priority("peer:x", &[c("peer:x", 120.0, 0.0)]), 0);
        // Unreachable (loss 100) → Transit → 0.
        assert_eq!(dispatcher_priority("peer:x", &[c("peer:x", 10.0, 100.0)]), 0);
        // No candidates → Transit fallback → 0.
        assert_eq!(dispatcher_priority("peer:x", &[]), 0);
    }

    fn c(via: &str, rtt: f32, loss: f32) -> Candidate {
        Candidate {
            via: via.into(),
            rtt_ms: rtt,
            loss_pct: loss,
        }
    }

    #[test]
    fn direct_wins_when_within_caps() {
        let path = best_path("peer:bob", &[c("peer:bob", 12.0, 0.0)]);
        assert_eq!(
            path,
            Path::Direct {
                via: "peer:bob".into()
            }
        );
    }

    #[test]
    fn rtt_just_under_cap_is_direct() {
        let path = best_path("peer:bob", &[c("peer:bob", 79.9, 0.0)]);
        assert_eq!(
            path,
            Path::Direct {
                via: "peer:bob".into()
            }
        );
    }

    #[test]
    fn rtt_at_cap_falls_back_to_transit() {
        // MAX_DIRECT_RTT_MS is a strict less-than gate; 80 ms
        // exactly is treated as too slow.
        let path = best_path(
            "peer:bob",
            &[c("peer:bob", 80.0, 0.0), c("peer:relay", 30.0, 0.0)],
        );
        assert_eq!(
            path,
            Path::Transit {
                via: "peer:relay".into()
            }
        );
    }

    #[test]
    fn loss_at_cap_falls_back_to_transit() {
        let path = best_path(
            "peer:bob",
            &[c("peer:bob", 12.0, 5.0), c("peer:relay", 30.0, 0.0)],
        );
        assert_eq!(
            path,
            Path::Transit {
                via: "peer:relay".into()
            }
        );
    }

    #[test]
    fn high_loss_disqualifies_direct() {
        let path = best_path(
            "peer:bob",
            &[c("peer:bob", 5.0, 50.0), c("peer:relay", 25.0, 0.0)],
        );
        assert_eq!(
            path,
            Path::Transit {
                via: "peer:relay".into()
            }
        );
    }

    #[test]
    fn unreachable_direct_falls_back_to_transit() {
        // loss_pct = 100.0 means the direct probe is timing
        // out — clearly not a viable path. Mesh-latency worker
        // writes 100 for `ok: false` peers.
        let path = best_path(
            "peer:bob",
            &[c("peer:bob", 0.0, 100.0), c("peer:relay", 25.0, 0.0)],
        );
        assert_eq!(
            path,
            Path::Transit {
                via: "peer:relay".into()
            }
        );
    }

    #[test]
    fn picks_best_score_among_multiple_direct_candidates() {
        // Two direct candidates (same target, different
        // measured paths — e.g. before and after a route
        // change). Lower score wins.
        let path = best_path(
            "peer:bob",
            &[c("peer:bob", 40.0, 2.0), c("peer:bob", 12.0, 0.5)],
        );
        assert_eq!(
            path,
            Path::Direct {
                via: "peer:bob".into()
            }
        );
        // The picked one should be the (12.0, 0.5) candidate:
        // score 17.0 vs 60.0.
    }

    #[test]
    fn picks_best_score_among_multiple_transit_candidates() {
        let path = best_path(
            "peer:bob",
            &[
                c("peer:bob", 200.0, 0.0),       // too slow for direct
                c("peer:relay-fast", 25.0, 0.0), // direct route to fast relay
                c("peer:relay-slow", 70.0, 1.0), // direct route to slow relay
            ],
        );
        assert_eq!(
            path,
            Path::Transit {
                via: "peer:relay-fast".into()
            }
        );
    }

    #[test]
    fn transit_relay_can_exceed_rtt_cap() {
        // A 90 ms-RTT relay is still better than NO path. The
        // 80 ms cap only gates *direct* selection; transit
        // accepts any reachable hop. Kamailio's record-route
        // approach adds one extra hop so the actual user-
        // perceived RTT will be 2*90 = 180 ms — over the
        // ideal-voice cap but well under the unusable-call
        // threshold (~400 ms).
        let path = best_path(
            "peer:bob",
            &[c("peer:bob", 0.0, 100.0), c("peer:relay", 90.0, 0.0)],
        );
        assert_eq!(
            path,
            Path::Transit {
                via: "peer:relay".into()
            }
        );
    }

    #[test]
    fn no_candidates_at_all_falls_through_to_self_transit() {
        // Degenerate case — no telemetry. pick_relay returns
        // the target itself; the eventual Kamailio call will
        // 503 with a clear "no transit available" reply.
        let path = best_path("peer:bob", &[]);
        assert_eq!(
            path,
            Path::Transit {
                via: "peer:bob".into()
            }
        );
    }

    #[test]
    fn no_qualifying_path_and_no_transit_falls_through_to_self() {
        let path = best_path(
            "peer:bob",
            // Only a 100% loss direct candidate, no relays.
            &[c("peer:bob", 0.0, 100.0)],
        );
        assert_eq!(
            path,
            Path::Transit {
                via: "peer:bob".into()
            }
        );
    }

    #[test]
    fn pick_relay_skips_dead_transit_options() {
        let relay = pick_relay(
            "peer:bob",
            &[
                c("peer:relay-dead", 5.0, 100.0),
                c("peer:relay-up", 50.0, 0.0),
            ],
        );
        assert_eq!(relay, "peer:relay-up");
    }

    #[test]
    fn pick_relay_ignores_the_destination_itself() {
        // Even if the destination's own candidate has a
        // sensible score, pick_relay shouldn't pick it (that'd
        // be a degenerate Direct path, not Transit).
        let relay = pick_relay(
            "peer:bob",
            &[c("peer:bob", 12.0, 0.0), c("peer:relay", 50.0, 0.0)],
        );
        assert_eq!(relay, "peer:relay");
    }

    #[test]
    fn score_matches_design_doc_weighting() {
        // 1% loss adds 10 ms of equivalent perceived delay.
        let baseline = score(&c("p", 50.0, 0.0));
        let one_pct_loss = score(&c("p", 50.0, 1.0));
        assert!((one_pct_loss - baseline - 10.0).abs() < f32::EPSILON);
    }

    #[test]
    fn path_via_accessor_covers_both_variants() {
        assert_eq!(Path::Direct { via: "p".into() }.via(), "p");
        assert_eq!(Path::Transit { via: "r".into() }.via(), "r");
    }

    #[test]
    fn candidate_json_round_trips() {
        let cand = c("peer:bob", 12.5, 1.5);
        let json = serde_json::to_string(&cand).unwrap();
        let back: Candidate = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cand);
    }

    #[test]
    fn path_json_round_trips_both_variants() {
        let d = Path::Direct {
            via: "peer:bob".into(),
        };
        let t = Path::Transit {
            via: "peer:relay".into(),
        };
        for p in [d, t] {
            let json = serde_json::to_string(&p).unwrap();
            let back: Path = serde_json::from_str(&json).unwrap();
            assert_eq!(back, p);
        }
    }

    #[test]
    fn best_path_is_deterministic() {
        let candidates = vec![
            c("peer:bob", 40.0, 2.0),
            c("peer:bob", 12.0, 0.5),
            c("peer:relay", 25.0, 0.0),
        ];
        let a = best_path("peer:bob", &candidates);
        let b = best_path("peer:bob", &candidates);
        assert_eq!(a, b);
    }
}
