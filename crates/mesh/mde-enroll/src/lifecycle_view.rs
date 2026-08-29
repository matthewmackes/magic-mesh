//! WL-FUNC-023 S4 — TUI glue over the shared lifecycle session projection.
//!
//! The view type itself lives in `mackes-mesh-types` so Construct can consume
//! the same contract without depending on this crate or `mackesd`.

pub use mackes_mesh_types::lifecycle_view::{LifecycleSessionView, ReadinessState};
use mackesd_core::lifecycle_authority::LifecycleCheckpointV1;

/// Project a peeked authority checkpoint through the shared view.
pub fn view_from_checkpoint(
    checkpoint: &LifecycleCheckpointV1,
) -> Result<LifecycleSessionView, String> {
    LifecycleSessionView::from_authority_parts(
        &checkpoint.plan,
        &checkpoint.progress,
        &checkpoint.checks,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::lifecycle::{
        LifecycleCheckStatus, LifecycleIntentKind, LifecyclePhase, LifecyclePlanV1,
        LifecycleProgressV1, LifecycleRequirementCheckV1,
    };

    #[test]
    fn view_from_checkpoint_matches_the_shared_projection() {
        let checkpoint = LifecycleCheckpointV1 {
            plan: LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-1".into(),
                target_id: "seat-15".into(),
                intent: LifecycleIntentKind::Onboard,
                generation: 1,
                steps: vec!["identity".into()],
            },
            progress: LifecycleProgressV1 {
                schema_version: 1,
                request_id: "request-1".into(),
                target_id: "seat-15".into(),
                generation: 1,
                phase: LifecyclePhase::WaitingForOperator,
                completed_steps: 0,
                total_steps: 1,
            },
            checks: vec![LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "identity".into(),
                target_id: "seat-15".into(),
                expected: "present".into(),
                observed: "missing".into(),
                status: LifecycleCheckStatus::Fail,
                required: true,
                evidence_digest_hex: "2".repeat(64),
                warning: None,
                generation: 1,
            }],
            confirmation: None,
            consumed_capsule_ids: Vec::new(),
            pending_capsule_ids: Vec::new(),
            revoked_capsule_ids: Vec::new(),
            artifact_selection: None,
            retry_count: 0,
            last_error: None,
            pending_enrollment_bearer_digests: Vec::new(),
        };
        let view = view_from_checkpoint(&checkpoint).unwrap();
        assert_eq!(view.status_line(), "request-1: onboard (blocked)");
        assert_eq!(view.missing_requirements, vec!["identity"]);
    }
}
