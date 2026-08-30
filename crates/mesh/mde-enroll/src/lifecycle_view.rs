//! WL-FUNC-023 S4 — TUI glue over the shared lifecycle session projection.
//!
//! The view type itself lives in `mackes-mesh-types` so Construct can consume
//! the same contract without depending on this crate or `mackesd`.

use mackes_mesh_types::lifecycle::FleetLifecycleReportV1;
pub use mackes_mesh_types::lifecycle_view::{LifecycleSessionView, ReadinessState};
use mackesd_core::lifecycle_authority::LifecycleCheckpointV1;

/// Project a peeked authority checkpoint through the shared view.
pub fn view_from_checkpoint(
    checkpoint: &LifecycleCheckpointV1,
) -> Result<LifecycleSessionView, String> {
    Ok(LifecycleSessionView::from_authority_parts(
        &checkpoint.plan,
        &checkpoint.progress,
        &checkpoint.checks,
    )?
    .with_artifact_selection(checkpoint.artifact_selection.as_ref())
    .with_coordinator(checkpoint.coordinator_id.as_deref())
    .with_correction_plan(checkpoint.correction_plan.as_ref(), &checkpoint.checks)
    .with_last_error(checkpoint.last_error.as_deref())
    .with_onboard_nag(&checkpoint.checks))
}

/// Project a peeked fleet session. Confirmation phrases use every durable
/// seat; a single checkpoint cannot shrink the fleet count.
pub fn view_from_fleet_session(
    report: &FleetLifecycleReportV1,
    checkpoints: &[LifecycleCheckpointV1],
) -> Result<LifecycleSessionView, String> {
    let Some(first) = checkpoints.first() else {
        return Err("fleet session has no seats".to_owned());
    };
    if report.target_count as usize != checkpoints.len() {
        return Err("fleet report target count does not match checkpoints".to_owned());
    }
    let targets: Vec<String> = checkpoints
        .iter()
        .map(|checkpoint| checkpoint.plan.target_id.clone())
        .collect();
    let last_error = checkpoints.iter().find_map(|checkpoint| {
        checkpoint
            .last_error
            .as_deref()
            .filter(|error| !error.is_empty())
    });
    let mut view = view_from_checkpoint(first)?
        .with_fleet_targets(targets)
        .with_fleet_report(report)
        .with_last_error(last_error);
    if view.correction_line().is_none() {
        for sibling in checkpoints.iter().skip(1) {
            view = view.with_correction_plan(sibling.correction_plan.as_ref(), &sibling.checks);
            if view.correction_line().is_some() {
                break;
            }
        }
    }
    if view.onboard_nag_line().is_none() {
        for sibling in checkpoints.iter().skip(1) {
            view = view.with_onboard_nag(&sibling.checks);
            if view.onboard_nag_line().is_some() {
                break;
            }
        }
    }
    Ok(view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::lifecycle::{
        FleetLifecycleReportV1, LifecycleArtifactChannel, LifecycleArtifactSelectionV1,
        LifecycleCheckStatus, LifecycleCorrectionPlanV1, LifecycleCorrectionV1,
        LifecycleIntentKind, LifecyclePhase, LifecyclePlanV1, LifecycleProgressV1,
        LifecycleRequirementCheckV1,
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
            coordinator_id: None,
            correction_plan: None,
        };
        let view = view_from_checkpoint(&checkpoint).unwrap();
        assert_eq!(view.status_line(), "request-1: onboard (blocked)");
        assert_eq!(view.missing_requirements, vec!["identity"]);
    }

    #[test]
    fn view_from_checkpoint_names_the_offboard_phrase() {
        let checkpoint = LifecycleCheckpointV1 {
            plan: LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-offboard".into(),
                target_id: "seat-15".into(),
                intent: LifecycleIntentKind::Offboard,
                generation: 1,
                steps: vec!["offboard".into(), "verify".into()],
            },
            progress: LifecycleProgressV1 {
                schema_version: 1,
                request_id: "request-offboard".into(),
                target_id: "seat-15".into(),
                generation: 1,
                phase: LifecyclePhase::Planned,
                completed_steps: 0,
                total_steps: 2,
            },
            checks: Vec::new(),
            confirmation: None,
            consumed_capsule_ids: Vec::new(),
            pending_capsule_ids: Vec::new(),
            revoked_capsule_ids: Vec::new(),
            artifact_selection: None,
            retry_count: 0,
            last_error: None,
            pending_enrollment_bearer_digests: Vec::new(),
            coordinator_id: None,
            correction_plan: None,
        };
        let view = view_from_checkpoint(&checkpoint).unwrap();
        assert_eq!(view.confirmation_lines()[0], "FORCE OFFBOARD 1 SYSTEMS");
    }

    #[test]
    fn view_from_checkpoint_names_the_reset_wipe_phrase() {
        let checkpoint = LifecycleCheckpointV1 {
            plan: LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-reset".into(),
                target_id: "seat-15".into(),
                intent: LifecycleIntentKind::ResetAndOnboard,
                generation: 1,
                steps: vec!["offboard".into(), "identity".into(), "verify".into()],
            },
            progress: LifecycleProgressV1 {
                schema_version: 1,
                request_id: "request-reset".into(),
                target_id: "seat-15".into(),
                generation: 1,
                phase: LifecyclePhase::Planned,
                completed_steps: 0,
                total_steps: 3,
            },
            checks: Vec::new(),
            confirmation: None,
            consumed_capsule_ids: Vec::new(),
            pending_capsule_ids: Vec::new(),
            revoked_capsule_ids: Vec::new(),
            artifact_selection: None,
            retry_count: 0,
            last_error: None,
            pending_enrollment_bearer_digests: Vec::new(),
            coordinator_id: None,
            correction_plan: None,
        };
        let view = view_from_checkpoint(&checkpoint).unwrap();
        assert_eq!(view.confirmation_lines()[0], "WIPE 1 SYSTEMS");
    }

    #[test]
    fn view_from_checkpoint_names_the_unsigned_phrase() {
        let digest = "e".repeat(64);
        let checkpoint = LifecycleCheckpointV1 {
            plan: LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-unsigned".into(),
                target_id: "seat-15".into(),
                intent: LifecycleIntentKind::Upgrade,
                generation: 1,
                steps: vec!["packages".into(), "verify".into()],
            },
            progress: LifecycleProgressV1 {
                schema_version: 1,
                request_id: "request-unsigned".into(),
                target_id: "seat-15".into(),
                generation: 1,
                phase: LifecyclePhase::Planned,
                completed_steps: 0,
                total_steps: 2,
            },
            checks: Vec::new(),
            confirmation: None,
            consumed_capsule_ids: Vec::new(),
            pending_capsule_ids: Vec::new(),
            revoked_capsule_ids: Vec::new(),
            artifact_selection: Some(LifecycleArtifactSelectionV1 {
                schema_version: 1,
                selection_id: "sel-1".into(),
                target_id: "seat-15".into(),
                channel: LifecycleArtifactChannel::Dev,
                artifact_digest_hex: digest.clone(),
                source_revision: "rev-1".into(),
                signed: false,
                unverified_build: true,
                generation: 1,
            }),
            retry_count: 0,
            last_error: None,
            pending_enrollment_bearer_digests: Vec::new(),
            coordinator_id: None,
            correction_plan: None,
        };
        let view = view_from_checkpoint(&checkpoint).unwrap();
        assert_eq!(view.confirmation_lines()[0], "INSTALL UNSIGNED 1 SYSTEMS");
        assert_eq!(view.confirmation_lines()[1], format!("scope {digest}"));
    }

    #[test]
    fn view_from_checkpoint_names_the_durable_coordinator() {
        let checkpoint = LifecycleCheckpointV1 {
            plan: LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-handoff".into(),
                target_id: "seat-15".into(),
                intent: LifecycleIntentKind::Onboard,
                generation: 1,
                steps: vec!["identity".into(), "verify".into()],
            },
            progress: LifecycleProgressV1 {
                schema_version: 1,
                request_id: "request-handoff".into(),
                target_id: "seat-15".into(),
                generation: 1,
                phase: LifecyclePhase::Running,
                completed_steps: 0,
                total_steps: 2,
            },
            checks: Vec::new(),
            confirmation: None,
            consumed_capsule_ids: Vec::new(),
            pending_capsule_ids: Vec::new(),
            revoked_capsule_ids: Vec::new(),
            artifact_selection: None,
            retry_count: 0,
            last_error: None,
            pending_enrollment_bearer_digests: Vec::new(),
            coordinator_id: Some("coord-b".into()),
            correction_plan: None,
        };
        let view = view_from_checkpoint(&checkpoint).unwrap();
        assert_eq!(
            view.coordinator_line().as_deref(),
            Some("coordinator coord-b")
        );
    }

    #[test]
    fn view_from_checkpoint_names_the_last_error() {
        let checkpoint = LifecycleCheckpointV1 {
            plan: LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-err".into(),
                target_id: "seat-15".into(),
                intent: LifecycleIntentKind::VerifyAndCorrect,
                generation: 1,
                steps: vec!["verify".into(), "verify".into()],
            },
            progress: LifecycleProgressV1 {
                schema_version: 1,
                request_id: "request-err".into(),
                target_id: "seat-15".into(),
                generation: 1,
                phase: LifecyclePhase::Running,
                completed_steps: 0,
                total_steps: 2,
            },
            checks: Vec::new(),
            confirmation: None,
            consumed_capsule_ids: Vec::new(),
            pending_capsule_ids: Vec::new(),
            revoked_capsule_ids: Vec::new(),
            artifact_selection: None,
            retry_count: 1,
            last_error: Some("provider timeout".into()),
            pending_enrollment_bearer_digests: Vec::new(),
            coordinator_id: None,
            correction_plan: None,
        };
        let view = view_from_checkpoint(&checkpoint).unwrap();
        assert_eq!(
            view.last_error_line().as_deref(),
            Some("last error: provider timeout")
        );
    }

    #[test]
    fn view_from_checkpoint_names_the_onboard_nag() {
        let checkpoint = LifecycleCheckpointV1 {
            plan: LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-nag".into(),
                target_id: "seat-15".into(),
                intent: LifecycleIntentKind::Onboard,
                generation: 1,
                steps: vec!["mesh".into(), "verify".into()],
            },
            progress: LifecycleProgressV1 {
                schema_version: 1,
                request_id: "request-nag".into(),
                target_id: "seat-15".into(),
                generation: 1,
                phase: LifecyclePhase::Running,
                completed_steps: 0,
                total_steps: 2,
            },
            checks: vec![LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "mesh_identity".into(),
                target_id: "seat-15".into(),
                expected: "enrolled mesh identity".into(),
                observed: "missing: overlay-ip,etcd-endpoints".into(),
                status: LifecycleCheckStatus::Fail,
                required: true,
                evidence_digest_hex: "3".repeat(64),
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
            coordinator_id: None,
            correction_plan: None,
        };
        let view = view_from_checkpoint(&checkpoint).unwrap();
        assert_eq!(
            view.onboard_nag_line(),
            Some("open ONBOARD: missing overlay-ip,etcd-endpoints")
        );
    }

    #[test]
    fn view_from_fleet_session_names_the_fleet_phrase_and_coordinator() {
        let first = LifecycleCheckpointV1 {
            plan: LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-1".into(),
                target_id: "seat-15".into(),
                intent: LifecycleIntentKind::Offboard,
                generation: 1,
                steps: vec!["offboard".into(), "verify".into()],
            },
            progress: LifecycleProgressV1 {
                schema_version: 1,
                request_id: "request-1".into(),
                target_id: "seat-15".into(),
                generation: 1,
                phase: LifecyclePhase::Planned,
                completed_steps: 0,
                total_steps: 2,
            },
            checks: Vec::new(),
            confirmation: None,
            consumed_capsule_ids: Vec::new(),
            pending_capsule_ids: Vec::new(),
            revoked_capsule_ids: Vec::new(),
            artifact_selection: None,
            retry_count: 0,
            last_error: None,
            pending_enrollment_bearer_digests: Vec::new(),
            coordinator_id: Some("coord-b".into()),
            correction_plan: None,
        };
        let mut second = first.clone();
        second.plan.target_id = "seat-16".into();
        second.progress.target_id = "seat-16".into();
        second.last_error = Some("wave-2 timeout".into());
        second.checks = vec![LifecycleRequirementCheckV1 {
            schema_version: 1,
            check_id: "mesh".into(),
            target_id: "seat-16".into(),
            expected: "joined".into(),
            observed: "absent".into(),
            status: LifecycleCheckStatus::Fail,
            required: true,
            evidence_digest_hex: "a".repeat(64),
            warning: None,
            generation: 1,
        }];
        second.correction_plan = Some(LifecycleCorrectionPlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-16".into(),
            generation: 1,
            corrections: vec![LifecycleCorrectionV1 {
                check_id: "mesh".into(),
                step: "mesh".into(),
                reason: "absent".into(),
                prerequisites: Vec::new(),
            }],
            edges: Vec::new(),
            rollback_forbidden: true,
        });
        let report = FleetLifecycleReportV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            generation: 1,
            phase: LifecyclePhase::Running,
            target_count: 2,
            succeeded: 0,
            failed: 0,
            coordinator_id: "coord-b".into(),
            signature_hex: String::new(),
        };
        let view = view_from_fleet_session(&report, &[first, second]).unwrap();
        assert_eq!(view.confirmation_lines()[0], "FORCE OFFBOARD 2 SYSTEMS");
        assert_eq!(
            view.coordinator_line().as_deref(),
            Some("coordinator coord-b")
        );
        assert_eq!(view.fleet_line().as_deref(), Some("fleet seat-15, seat-16"));
        assert_eq!(
            view.last_error_line().as_deref(),
            Some("last error: wave-2 timeout"),
            "a clean first seat cannot hide another durable last error"
        );
        assert_eq!(
            view.correction_line(),
            Some("correct mesh: mesh (absent)"),
            "a clean first seat cannot hide another durable correction"
        );
        assert!(
            view_from_fleet_session(&report, &[]).is_err(),
            "empty fleet cannot invent a session"
        );
    }
}
