//! Reconcile engine — pure logic for drift detection, lifecycle
//! state machine, auto-repair dispatch, and retry/backoff
//! (Phase 12.5.1 through 12.5.4).
//!
//! Each piece is a pure function or small typed value with no I/O,
//! so the reconcile loop (which lives in the binary's main loop, not
//! here) can call into it deterministically and the same logic
//! powers `mackesd apply --dry-run`.

use crate::topology::TopologyDiff;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Severity of a drift row. Drives whether the reconciler can
/// auto-repair (12.5.3) or must surface the row in the Pending
/// Changes inbox for operator approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftSeverity {
    /// Reconciler can re-push the desired state without operator
    /// approval (e.g. transient WireGuard route loss).
    AutoRepairable,
    /// Operator must explicitly approve before the reconciler acts
    /// (e.g. identity drift, missing policy that used to exist).
    ManualReview,
}

/// One drift row, as the detector emits + the GUI's inbox consumes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriftRow {
    /// Severity per the 12.5.1 lock.
    pub severity: DriftSeverity,
    /// Which subsystem noticed the drift — used for filtering /
    /// grouping in the inbox UI.
    pub detector: &'static str,
    /// Free-form reason chain. The format is "X because Y" so the
    /// row reads naturally in the GUI.
    pub reason: String,
}

/// Detect drift between desired and actual topology snapshots and
/// classify each finding by severity.
///
/// Pure function over the diff input — the worker that calls this
/// owns the cadence (default 30 s per 12.5.1).
#[must_use]
pub fn detect_drift(diff: &TopologyDiff) -> Vec<DriftRow> {
    let mut out = Vec::new();
    for edge in &diff.missing {
        // Missing edges are usually a transient Tailscale / network
        // hiccup → auto-repairable.
        out.push(DriftRow {
            severity: DriftSeverity::AutoRepairable,
            detector: "topology",
            reason: format!(
                "desired peer adjacency [{} ↔ {}] missing in observed_telemetry",
                edge.a, edge.b
            ),
        });
    }
    for edge in &diff.extra {
        // Extra edges are unexpected — could be tampering, could be
        // a stale Tailscale ACL. Surface for review.
        out.push(DriftRow {
            severity: DriftSeverity::ManualReview,
            detector: "topology",
            reason: format!(
                "observed peer adjacency [{} ↔ {}] not in desired_config",
                edge.a, edge.b
            ),
        });
    }
    out
}

/// Lifecycle state for one in-flight revision (Phase 12.5.2).
/// Transitions land in `applied_changes` (happy path) or
/// `failed_changes` (FailedValidation / RolledBack).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    /// Operator drafted a new desired config.
    Draft,
    /// Schema + policy + topology validators passed.
    Validated,
    /// Operator clicked Approve in the inbox.
    Approved,
    /// Reconciler is pushing changes out to peers.
    Deploying,
    /// All peers acknowledged the deploy.
    Applied,
    /// Next reconcile tick confirmed runtime matches desired.
    Verified,
    /// One of the validators rejected the draft.
    FailedValidation,
    /// Deploy started but failed; the prior revision is back in
    /// place.
    RolledBack,
}

/// Legal transitions per the locked FSM. Each row is `(from, to,
/// kind)` where `kind` is "happy" or "error".
pub const TRANSITIONS: &[(LifecycleState, LifecycleState)] = &[
    (LifecycleState::Draft, LifecycleState::Validated),
    (LifecycleState::Draft, LifecycleState::FailedValidation),
    (LifecycleState::Validated, LifecycleState::Approved),
    (LifecycleState::Approved, LifecycleState::Deploying),
    (LifecycleState::Deploying, LifecycleState::Applied),
    (LifecycleState::Deploying, LifecycleState::RolledBack),
    (LifecycleState::Applied, LifecycleState::Verified),
];

/// True when `from → to` is a legal transition per `TRANSITIONS`.
#[must_use]
pub fn is_legal_transition(from: LifecycleState, to: LifecycleState) -> bool {
    TRANSITIONS.iter().any(|(f, t)| *f == from && *t == to)
}

/// Compute the next retry delay for a failed deploy (Phase 12.5.4).
/// Exponential backoff starting at 1 s, doubling each attempt, capped
/// at 60 s. Attempt 0 is the immediate first try (no delay).
#[must_use]
pub fn backoff_delay(attempt: u32) -> Duration {
    if attempt == 0 {
        return Duration::from_secs(0);
    }
    let secs = 1u64
        .checked_shl(attempt.saturating_sub(1))
        .unwrap_or(60)
        .min(60);
    Duration::from_secs(secs)
}

/// Drives the auto-repair dispatcher decision (Phase 12.5.3): given
/// a drift row + a policy flag, return whether the reconciler should
/// attempt repair without operator approval.
#[must_use]
pub const fn should_auto_repair(row: &DriftRow, auto_repair_enabled: bool) -> bool {
    matches!(row.severity, DriftSeverity::AutoRepairable) && auto_repair_enabled
}

/// Single reconcile tick over a snapshot pair — wires detection,
/// dispatch, and inbox-population into one pure function so the
/// real worker thread is a 5-line loop on top of this.
///
/// Returns a `TickPlan` summarizing what would happen — `repair_now`
/// holds the auto-repairable drift rows; `inbox` holds the
/// manual-review ones. The reconcile loop's actual work is
/// (a) applying every row in `repair_now`, (b) writing every row
/// in `inbox` to the `pending_changes` table.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TickPlan {
    /// Rows the reconciler will attempt to repair this tick.
    pub repair_now: Vec<DriftRow>,
    /// Rows that need operator approval — surface in the GUI inbox.
    pub inbox: Vec<DriftRow>,
}

/// Compute the plan for one reconcile tick. Pure function:
/// no I/O, no clock, no global state.
#[must_use]
pub fn plan_tick(diff: &TopologyDiff, auto_repair_enabled: bool) -> TickPlan {
    let rows = detect_drift(diff);
    let mut plan = TickPlan::default();
    for row in rows {
        if should_auto_repair(&row, auto_repair_enabled) {
            plan.repair_now.push(row);
        } else {
            plan.inbox.push(row);
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::{Edge, EdgeKind, TopologyDiff};
    use std::collections::BTreeSet;

    fn mk_diff(missing: Vec<(&str, &str)>, extra: Vec<(&str, &str)>) -> TopologyDiff {
        TopologyDiff {
            missing: missing
                .into_iter()
                .map(|(a, b)| Edge {
                    a: a.to_owned(),
                    b: b.to_owned(),
                    kind: EdgeKind::NebulaDirect,
                })
                .collect(),
            extra: extra
                .into_iter()
                .map(|(a, b)| Edge {
                    a: a.to_owned(),
                    b: b.to_owned(),
                    kind: EdgeKind::NebulaDirect,
                })
                .collect(),
            healthy: BTreeSet::new(),
        }
    }

    #[test]
    fn no_diff_yields_no_drift() {
        assert!(detect_drift(&mk_diff(vec![], vec![])).is_empty());
    }

    #[test]
    fn missing_edges_are_auto_repairable() {
        let drift = detect_drift(&mk_diff(vec![("a", "b")], vec![]));
        assert_eq!(drift.len(), 1);
        assert_eq!(drift[0].severity, DriftSeverity::AutoRepairable);
    }

    #[test]
    fn extra_edges_require_manual_review() {
        let drift = detect_drift(&mk_diff(vec![], vec![("a", "b")]));
        assert_eq!(drift.len(), 1);
        assert_eq!(drift[0].severity, DriftSeverity::ManualReview);
    }

    #[test]
    fn happy_path_lifecycle_transitions() {
        assert!(is_legal_transition(
            LifecycleState::Draft,
            LifecycleState::Validated
        ));
        assert!(is_legal_transition(
            LifecycleState::Validated,
            LifecycleState::Approved
        ));
        assert!(is_legal_transition(
            LifecycleState::Approved,
            LifecycleState::Deploying
        ));
        assert!(is_legal_transition(
            LifecycleState::Deploying,
            LifecycleState::Applied
        ));
        assert!(is_legal_transition(
            LifecycleState::Applied,
            LifecycleState::Verified
        ));
    }

    #[test]
    fn error_path_lifecycle_transitions() {
        assert!(is_legal_transition(
            LifecycleState::Draft,
            LifecycleState::FailedValidation
        ));
        assert!(is_legal_transition(
            LifecycleState::Deploying,
            LifecycleState::RolledBack
        ));
    }

    #[test]
    fn illegal_transitions_rejected() {
        assert!(!is_legal_transition(
            LifecycleState::Draft,
            LifecycleState::Applied
        ));
        assert!(!is_legal_transition(
            LifecycleState::Verified,
            LifecycleState::Draft
        ));
        assert!(!is_legal_transition(
            LifecycleState::FailedValidation,
            LifecycleState::Applied
        ));
    }

    #[test]
    fn backoff_curve_doubles_to_cap() {
        assert_eq!(backoff_delay(0), Duration::from_secs(0));
        assert_eq!(backoff_delay(1), Duration::from_secs(1));
        assert_eq!(backoff_delay(2), Duration::from_secs(2));
        assert_eq!(backoff_delay(3), Duration::from_secs(4));
        assert_eq!(backoff_delay(4), Duration::from_secs(8));
        assert_eq!(backoff_delay(10), Duration::from_secs(60)); // capped
        assert_eq!(backoff_delay(100), Duration::from_secs(60));
    }

    #[test]
    fn plan_tick_routes_auto_to_repair_and_manual_to_inbox() {
        let diff = mk_diff(vec![("a", "b"), ("c", "d")], vec![("e", "f")]);
        // With auto-repair enabled, 2 missing → repair_now, 1 extra → inbox.
        let plan = plan_tick(&diff, true);
        assert_eq!(plan.repair_now.len(), 2);
        assert_eq!(plan.inbox.len(), 1);
    }

    #[test]
    fn plan_tick_disables_auto_when_policy_off() {
        let diff = mk_diff(vec![("a", "b")], vec![]);
        let plan = plan_tick(&diff, false);
        // Auto-repair off → even the auto-repairable row lands in inbox.
        assert!(plan.repair_now.is_empty());
        assert_eq!(plan.inbox.len(), 1);
    }

    #[test]
    fn auto_repair_dispatch_respects_severity_and_policy() {
        let auto = DriftRow {
            severity: DriftSeverity::AutoRepairable,
            detector: "topology",
            reason: String::new(),
        };
        let manual = DriftRow {
            severity: DriftSeverity::ManualReview,
            detector: "policy",
            reason: String::new(),
        };
        // Auto-repairable + enabled → repair.
        assert!(should_auto_repair(&auto, true));
        // Auto-repairable but policy disabled → no repair.
        assert!(!should_auto_repair(&auto, false));
        // Manual-review → never repair regardless of policy.
        assert!(!should_auto_repair(&manual, true));
        assert!(!should_auto_repair(&manual, false));
    }

    #[test]
    fn backoff_intermediate_doublings() {
        // Cover attempt 5/6/7 against the cap-at-60 break point.
        assert_eq!(backoff_delay(5), Duration::from_secs(16));
        assert_eq!(backoff_delay(6), Duration::from_secs(32));
        // Attempt 7: 1u64 << 6 = 64 → min(64, 60) = 60.
        assert_eq!(backoff_delay(7), Duration::from_secs(60));
    }

    #[test]
    fn drift_reason_strings_carry_edge_endpoints() {
        let drift = detect_drift(&mk_diff(vec![("peer:anvil", "peer:yew")], vec![]));
        assert_eq!(drift.len(), 1);
        assert!(drift[0].reason.contains("peer:anvil"));
        assert!(drift[0].reason.contains("peer:yew"));
        assert_eq!(drift[0].detector, "topology");
    }

    #[test]
    fn extra_edges_drift_reason_says_observed() {
        let drift = detect_drift(&mk_diff(vec![], vec![("peer:a", "peer:b")]));
        assert_eq!(drift[0].severity, DriftSeverity::ManualReview);
        assert!(drift[0].reason.contains("not in desired_config"));
    }

    #[test]
    fn missing_edges_drift_reason_says_missing() {
        let drift = detect_drift(&mk_diff(vec![("peer:a", "peer:b")], vec![]));
        assert_eq!(drift[0].severity, DriftSeverity::AutoRepairable);
        assert!(drift[0].reason.contains("missing"));
    }

    #[test]
    fn lifecycle_state_serializes_snake_case() {
        let s = serde_json::to_string(&LifecycleState::FailedValidation).unwrap();
        assert_eq!(s, "\"failed_validation\"");
        let s = serde_json::to_string(&LifecycleState::RolledBack).unwrap();
        assert_eq!(s, "\"rolled_back\"");
        let s = serde_json::to_string(&LifecycleState::Draft).unwrap();
        assert_eq!(s, "\"draft\"");
    }

    #[test]
    fn transitions_contain_seven_legal_edges() {
        // Lock from the FSM comment block — guard against accidental
        // additions/removals.
        assert_eq!(TRANSITIONS.len(), 7);
    }

    #[test]
    fn drift_severity_round_trips_through_json() {
        for sev in [DriftSeverity::AutoRepairable, DriftSeverity::ManualReview] {
            let s = serde_json::to_string(&sev).unwrap();
            let back: DriftSeverity = serde_json::from_str(&s).unwrap();
            assert_eq!(back, sev);
        }
    }

    #[test]
    fn plan_tick_empty_diff_yields_empty_plan() {
        let plan = plan_tick(&mk_diff(vec![], vec![]), true);
        assert!(plan.repair_now.is_empty());
        assert!(plan.inbox.is_empty());
    }

    #[test]
    fn drift_row_serializes_with_expected_fields() {
        // DriftRow's `detector` is &'static str so it doesn't
        // deserialize back into a non-'static buffer — assert the
        // serialized shape directly instead of full round-trip.
        let row = DriftRow {
            severity: DriftSeverity::AutoRepairable,
            detector: "topology",
            reason: "x".into(),
        };
        let s = serde_json::to_string(&row).unwrap();
        assert!(s.contains("\"severity\":\"auto_repairable\""));
        assert!(s.contains("\"detector\":\"topology\""));
        assert!(s.contains("\"reason\":\"x\""));
    }
}
