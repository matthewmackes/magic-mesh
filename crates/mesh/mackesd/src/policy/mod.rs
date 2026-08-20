//! Policy DSL + validation (Phase 12.7.2).
//!
//! Policies are JSON documents validated against a known schema.
//! This module ships the typed representation + the conflict
//! detector. The reconciler (Phase 12.5) consumes the validated
//! policy list to drive route + access decisions.

use serde::{Deserialize, Serialize};

/// One policy rule. The enum tag drives which kind of constraint
/// applies; each variant carries its own payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Policy {
    /// Allow east-west traffic between two regions.
    AllowEastWest {
        /// Stable rule identifier (used in conflict reports).
        id: String,
        /// Source region tag.
        from_region: String,
        /// Destination region tag.
        to_region: String,
    },
    /// Forbid east-west traffic between two regions. Conflicts
    /// with an `AllowEastWest` over the same pair.
    DenyEastWest {
        /// Stable rule identifier.
        id: String,
        /// Source region tag.
        from_region: String,
        /// Destination region tag.
        to_region: String,
    },
    /// Cap maximum bandwidth between two regions (Mbps).
    BandwidthCap {
        /// Stable rule identifier.
        id: String,
        /// Source region tag.
        from_region: String,
        /// Destination region tag.
        to_region: String,
        /// Maximum bandwidth in Mbps.
        mbps: u32,
    },
}

impl Policy {
    /// The stable identifier embedded in every variant.
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::AllowEastWest { id, .. }
            | Self::DenyEastWest { id, .. }
            | Self::BandwidthCap { id, .. } => id,
        }
    }
}

/// Conflict report — names the two rule IDs whose effects can't
/// coexist on the same (from_region, to_region) pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyConflict {
    /// First conflicting rule.
    pub rule_a: String,
    /// Second conflicting rule.
    pub rule_b: String,
    /// Human reason — printed by the CLI.
    pub reason: String,
}

/// Lint a policy list for direct contradictions. Today this catches
/// the simple case: an `AllowEastWest` and a `DenyEastWest` over the
/// same region pair. Returns every detected conflict (not just the
/// first) so the operator can fix them all in one pass.
#[must_use]
pub fn detect_conflicts(rules: &[Policy]) -> Vec<PolicyConflict> {
    let mut out = Vec::new();
    for (i, a) in rules.iter().enumerate() {
        for b in rules.iter().skip(i + 1) {
            if let Some(reason) = pair_conflict(a, b) {
                out.push(PolicyConflict {
                    rule_a: a.id().to_owned(),
                    rule_b: b.id().to_owned(),
                    reason,
                });
            }
        }
    }
    out
}

fn pair_conflict(a: &Policy, b: &Policy) -> Option<String> {
    match (a, b) {
        (
            Policy::AllowEastWest {
                from_region: af,
                to_region: at,
                ..
            },
            Policy::DenyEastWest {
                from_region: bf,
                to_region: bt,
                ..
            },
        )
        | (
            Policy::DenyEastWest {
                from_region: af,
                to_region: at,
                ..
            },
            Policy::AllowEastWest {
                from_region: bf,
                to_region: bt,
                ..
            },
        ) => {
            if same_pair(af, at, bf, bt) {
                Some(format!(
                    "AllowEastWest and DenyEastWest both target ({af}, {at})"
                ))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn same_pair(af: &str, at: &str, bf: &str, bt: &str) -> bool {
    (af == bf && at == bt) || (af == bt && at == bf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow(id: &str, from: &str, to: &str) -> Policy {
        Policy::AllowEastWest {
            id: id.to_owned(),
            from_region: from.to_owned(),
            to_region: to.to_owned(),
        }
    }

    fn deny(id: &str, from: &str, to: &str) -> Policy {
        Policy::DenyEastWest {
            id: id.to_owned(),
            from_region: from.to_owned(),
            to_region: to.to_owned(),
        }
    }

    #[test]
    fn empty_list_has_no_conflicts() {
        assert!(detect_conflicts(&[]).is_empty());
    }

    #[test]
    fn allow_alone_is_fine() {
        assert!(detect_conflicts(&[allow("r1", "us-east", "us-west")]).is_empty());
    }

    #[test]
    fn allow_plus_deny_same_pair_is_a_conflict() {
        let conflicts = detect_conflicts(&[
            allow("r1", "us-east", "us-west"),
            deny("r2", "us-east", "us-west"),
        ]);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].rule_a, "r1");
        assert_eq!(conflicts[0].rule_b, "r2");
    }

    #[test]
    fn conflict_detection_is_order_insensitive() {
        // (a → b) and (b → a) describe the same edge for east-west.
        let conflicts = detect_conflicts(&[
            allow("r1", "us-east", "us-west"),
            deny("r2", "us-west", "us-east"),
        ]);
        assert_eq!(conflicts.len(), 1);
    }

    #[test]
    fn allow_plus_deny_different_pairs_no_conflict() {
        let conflicts = detect_conflicts(&[
            allow("r1", "us-east", "us-west"),
            deny("r2", "eu-west", "ap-south"),
        ]);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn json_round_trips() {
        let rules = vec![
            allow("r1", "us-east", "us-west"),
            Policy::BandwidthCap {
                id: "r2".into(),
                from_region: "us-east".into(),
                to_region: "us-west".into(),
                mbps: 100,
            },
        ];
        let json = serde_json::to_string(&rules).unwrap();
        let back: Vec<Policy> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rules);
    }

    #[test]
    fn policy_id_accessor_covers_every_variant() {
        let a = allow("r1", "x", "y");
        let d = deny("r2", "x", "y");
        let bw = Policy::BandwidthCap {
            id: "r3".into(),
            from_region: "x".into(),
            to_region: "y".into(),
            mbps: 10,
        };
        assert_eq!(a.id(), "r1");
        assert_eq!(d.id(), "r2");
        assert_eq!(bw.id(), "r3");
    }

    #[test]
    fn two_allows_over_same_pair_do_not_conflict() {
        // The detector only flags allow/deny on the same edge. Two
        // overlapping allows are redundant but not contradictory.
        let conflicts = detect_conflicts(&[
            allow("r1", "us-east", "us-west"),
            allow("r2", "us-east", "us-west"),
        ]);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn two_denies_over_same_pair_do_not_conflict() {
        let conflicts = detect_conflicts(&[
            deny("r1", "us-east", "us-west"),
            deny("r2", "us-east", "us-west"),
        ]);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn bandwidth_cap_does_not_conflict_with_allow_or_deny() {
        let bw = Policy::BandwidthCap {
            id: "r-bw".into(),
            from_region: "us-east".into(),
            to_region: "us-west".into(),
            mbps: 100,
        };
        let conflicts = detect_conflicts(&[
            allow("r1", "us-east", "us-west"),
            bw.clone(),
            deny("r2", "us-east", "us-west"),
        ]);
        // Only the allow/deny pair counts as a conflict; the bandwidth
        // cap is orthogonal.
        assert_eq!(conflicts.len(), 1);
        assert!(conflicts
            .iter()
            .all(|c| c.rule_a != "r-bw" && c.rule_b != "r-bw"));
    }

    #[test]
    fn detect_conflicts_returns_every_pair_not_just_first() {
        // Two allow/deny pairs over different edges — both surface.
        let conflicts = detect_conflicts(&[
            allow("a1", "us-east", "us-west"),
            deny("d1", "us-east", "us-west"),
            allow("a2", "eu-west", "eu-east"),
            deny("d2", "eu-west", "eu-east"),
        ]);
        assert_eq!(conflicts.len(), 2);
        let ids: Vec<&str> = conflicts
            .iter()
            .flat_map(|c| [c.rule_a.as_str(), c.rule_b.as_str()])
            .collect();
        for expected in ["a1", "d1", "a2", "d2"] {
            assert!(ids.contains(&expected), "missing {expected}");
        }
    }

    #[test]
    fn conflict_reason_mentions_endpoints() {
        let conflicts = detect_conflicts(&[
            allow("a", "us-east", "us-west"),
            deny("d", "us-east", "us-west"),
        ]);
        assert_eq!(conflicts.len(), 1);
        assert!(conflicts[0].reason.contains("us-east"));
        assert!(conflicts[0].reason.contains("us-west"));
    }

    #[test]
    fn deny_followed_by_allow_is_still_detected() {
        // Reverse the order to hit the `(Deny, Allow)` arm of the
        // `pair_conflict` match.
        let conflicts = detect_conflicts(&[
            deny("d", "us-east", "us-west"),
            allow("a", "us-east", "us-west"),
        ]);
        assert_eq!(conflicts.len(), 1);
        // rule_a is whichever came first.
        assert_eq!(conflicts[0].rule_a, "d");
        assert_eq!(conflicts[0].rule_b, "a");
    }
}
