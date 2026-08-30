//! WL-FUNC-023 S4 — Construct reads the same lifecycle session the TUI does.
//!
//! The shell never takes the authority lock and never mutates a checkpoint.
//! Extra checkpoint fields are ignored so the desktop tier stays off `mackesd`.

use std::path::{Path, PathBuf};

use mackes_mesh_types::lifecycle::{
    staged_package_identity, LifecycleArtifactSelectionV1, LifecycleCorrectionPlanV1,
    LifecyclePlanV1, LifecycleProgressV1, LifecycleRequirementCheckV1, OffboardingReceiptV1,
};
use mackes_mesh_types::lifecycle_view::LifecycleSessionView;
use mackes_mesh_types::peers::default_workgroup_root;
use mde_egui::egui::{self, RichText};
use mde_egui::Style;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct CheckpointProjection {
    plan: LifecyclePlanV1,
    progress: LifecycleProgressV1,
    #[serde(default)]
    checks: Vec<LifecycleRequirementCheckV1>,
    #[serde(default)]
    artifact_selection: Option<LifecycleArtifactSelectionV1>,
    #[serde(default)]
    coordinator_id: Option<String>,
    #[serde(default)]
    correction_plan: Option<LifecycleCorrectionPlanV1>,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    pending_capsule_ids: Vec<String>,
}

/// Known local authority roots. Workgroup first (join/found), then the
/// mackesd state tree. Neither path is a dest and neither is treated as ready
/// just because the directory exists.
pub fn default_lifecycle_authority_roots() -> [PathBuf; 2] {
    [default_workgroup_root(), PathBuf::from("/var/lib/mackesd")]
}

pub fn load_lifecycle_session_from_root(root: &Path) -> Option<LifecycleSessionView> {
    let dir = root.join("lifecycle");
    let entries = std::fs::read_dir(&dir).ok()?;
    let mut projections: Vec<CheckpointProjection> = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let path = entry.path().join("checkpoint.json");
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(checkpoint) = serde_json::from_slice::<CheckpointProjection>(&bytes) else {
            continue;
        };
        if checkpoint.plan.validate().is_err() || checkpoint.progress.validate().is_err() {
            continue;
        }
        projections.push(checkpoint);
    }
    let best_idx = projections
        .iter()
        .enumerate()
        .max_by(|(_, left), (_, right)| {
            left.progress
                .generation
                .cmp(&right.progress.generation)
                .then_with(|| left.plan.request_id.cmp(&right.plan.request_id))
        })?
        .0;
    let checkpoint = &projections[best_idx];
    let matching: Vec<&CheckpointProjection> = projections
        .iter()
        .filter(|projection| {
            projection.plan.request_id == checkpoint.plan.request_id
                && projection.plan.generation == checkpoint.plan.generation
        })
        .collect();
    let targets: Vec<String> = matching
        .iter()
        .map(|projection| projection.plan.target_id.clone())
        .collect();
    let last_error = checkpoint
        .last_error
        .as_deref()
        .filter(|error| !error.is_empty())
        .or_else(|| {
            matching.iter().find_map(|projection| {
                projection
                    .last_error
                    .as_deref()
                    .filter(|error| !error.is_empty())
            })
        });
    let mut view = LifecycleSessionView::from_authority_parts(
        &checkpoint.plan,
        &checkpoint.progress,
        &checkpoint.checks,
    )
    .ok()?;
    view = view
        .with_artifact_selection(checkpoint.artifact_selection.as_ref())
        .with_coordinator(checkpoint.coordinator_id.as_deref())
        .with_correction_plan(checkpoint.correction_plan.as_ref(), &checkpoint.checks)
        .with_last_error(last_error)
        .with_fleet_targets(targets)
        .with_onboard_nag(&checkpoint.checks);
    if view.correction_line().is_none() {
        for sibling in matching
            .iter()
            .filter(|projection| projection.plan.target_id != checkpoint.plan.target_id)
        {
            view = view.with_correction_plan(sibling.correction_plan.as_ref(), &sibling.checks);
            if view.correction_line().is_some() {
                break;
            }
        }
    }
    if view.onboard_nag_line().is_none() {
        for sibling in matching
            .iter()
            .filter(|projection| projection.plan.target_id != checkpoint.plan.target_id)
        {
            view = view.with_onboard_nag(&sibling.checks);
            if view.onboard_nag_line().is_some() {
                break;
            }
        }
    }
    for projection in &matching {
        if let Some(receipt) = load_offboarding_receipt(&dir, &projection.plan.target_id) {
            view = view.with_offboarding_receipt(Some(&receipt));
            break;
        }
    }
    for projection in &matching {
        if let Some(identity) = load_staged_package(&dir, &projection.plan.target_id) {
            view = view.with_staged_package(Some(&identity));
            break;
        }
    }
    for projection in &matching {
        if let Some(capsule_id) = projection
            .pending_capsule_ids
            .iter()
            .find(|id| !id.is_empty())
        {
            if load_staged_capsule(&dir, &projection.plan.target_id, capsule_id).is_some() {
                view = view.with_staged_capsule(Some(capsule_id));
                break;
            }
        }
    }
    Some(view)
}

fn load_regular_file(path: &Path) -> Option<Vec<u8>> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    meta.file_type()
        .is_file()
        .then(|| std::fs::read(path).ok())?
}

fn load_staged_capsule(dir: &Path, target_id: &str, capsule_id: &str) -> Option<()> {
    let path = dir.join(target_id).join("capsule").join(capsule_id);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    meta.file_type().is_file().then_some(())
}

fn load_staged_package(dir: &Path, target_id: &str) -> Option<String> {
    let seat = dir.join(target_id);
    let digest =
        String::from_utf8(load_regular_file(&seat.join("staged-artifact.digest"))?).ok()?;
    let shape = String::from_utf8(load_regular_file(&seat.join("staged-artifact.shape"))?).ok()?;
    let bytes = load_regular_file(&seat.join("staged-artifact"))?;
    staged_package_identity(digest.trim(), shape.trim(), &bytes)
}

fn load_offboarding_receipt(dir: &Path, target_id: &str) -> Option<OffboardingReceiptV1> {
    let path = dir.join(target_id).join("receipt.json");
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.file_type().is_file() {
        return None;
    }
    let receipt: OffboardingReceiptV1 = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
    receipt.validate().ok()?;
    (receipt.target_id == target_id).then_some(receipt)
}

pub fn load_lifecycle_session() -> Option<LifecycleSessionView> {
    for root in default_lifecycle_authority_roots() {
        if let Some(view) = load_lifecycle_session_from_root(&root) {
            return Some(view);
        }
    }
    None
}

/// Read-only Construct card. Same status/capability lines as `magic-setup`.
pub fn show_lifecycle_session(ui: &mut egui::Ui, view: Option<&LifecycleSessionView>) {
    ui.label(RichText::new("ONBOARD & OFFBOARDING session").strong());
    match view {
        Some(view) => {
            mde_egui::field(ui, "Session", &view.status_line(), Style::TEXT);
            mde_egui::field(ui, "Capabilities", &view.capability_summary(), Style::TEXT);
            if let Some(fleet) = view.fleet_line() {
                mde_egui::field(ui, "Fleet", &fleet, Style::TEXT);
            }
            if let Some(coordinator) = view.coordinator_line() {
                mde_egui::field(ui, "Coordinator", &coordinator, Style::TEXT);
            }
            if let Some(nag) = view.onboard_nag_line() {
                mde_egui::field(ui, "ONBOARD", nag, Style::WARN);
            }
            if let Some(correction) = view.correction_line() {
                mde_egui::field(ui, "Correction", correction, Style::TEXT);
            }
            if let Some(error) = view.last_error_line() {
                mde_egui::field(ui, "Last error", &error, Style::TEXT);
            }
            if let Some(receipt) = view.receipt_line() {
                mde_egui::field(ui, "Receipt", receipt, Style::TEXT);
            }
            if let Some(package) = view.package_line() {
                mde_egui::field(ui, "Packages", &package, Style::TEXT);
            }
            if let Some(capsule) = view.capsule_line() {
                mde_egui::field(ui, "Capsule", &capsule, Style::TEXT);
            }
            if !view.missing_requirements.is_empty() {
                ui.colored_label(
                    Style::WARN,
                    format!("missing: {}", view.missing_requirements.join(", ")),
                );
            }
            for line in view.confirmation_lines() {
                mde_egui::field(ui, "Confirmation", &line, Style::TEXT);
            }
        }
        None => {
            ui.colored_label(Style::TEXT_DIM, "no lifecycle session published");
        }
    }
    ui.label(
        RichText::new(
            "This projection is read-only. Mutation stays with the mackesd lifecycle authority.",
        )
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_root_is_unpublished_not_ready() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-shell-lifecycle-missing-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        assert!(load_lifecycle_session_from_root(&root).is_none());
    }

    #[test]
    fn hydrates_the_same_status_line_as_the_tui() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-shell-lifecycle-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let dir = root.join("lifecycle").join("seat-15");
        std::fs::create_dir_all(&dir).expect("temp checkpoint dir");
        let checkpoint = serde_json::json!({
            "plan": {
                "schema_version": 1,
                "request_id": "request-1",
                "target_id": "seat-15",
                "intent": "onboard",
                "generation": 1,
                "steps": ["identity"]
            },
            "progress": {
                "schema_version": 1,
                "request_id": "request-1",
                "target_id": "seat-15",
                "generation": 1,
                "phase": "waiting_for_operator",
                "completed_steps": 0,
                "total_steps": 1
            },
            "checks": [{
                "schema_version": 1,
                "check_id": "identity",
                "target_id": "seat-15",
                "expected": "present",
                "observed": "missing",
                "status": "fail",
                "required": true,
                "evidence_digest_hex": "2".repeat(64),
                "generation": 1
            }]
        });
        std::fs::write(
            dir.join("checkpoint.json"),
            serde_json::to_vec(&checkpoint).unwrap(),
        )
        .unwrap();
        let view = load_lifecycle_session_from_root(&root).expect("published session");
        assert_eq!(view.status_line(), "request-1: onboard (blocked)");
        assert_eq!(view.missing_requirements, vec!["identity"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn upgrade_checkpoint_shows_the_unsigned_phrase_not_a_seat_digest() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-shell-lifecycle-upgrade-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let dir = root.join("lifecycle").join("seat-15");
        std::fs::create_dir_all(&dir).expect("temp checkpoint dir");
        let digest = "e".repeat(64);
        let checkpoint = serde_json::json!({
            "plan": {
                "schema_version": 1,
                "request_id": "request-2",
                "target_id": "seat-15",
                "intent": "upgrade",
                "generation": 1,
                "steps": ["packages", "verify"]
            },
            "progress": {
                "schema_version": 1,
                "request_id": "request-2",
                "target_id": "seat-15",
                "generation": 1,
                "phase": "planned",
                "completed_steps": 0,
                "total_steps": 2
            },
            "artifact_selection": {
                "schema_version": 1,
                "selection_id": "sel-1",
                "target_id": "seat-15",
                "channel": "dev",
                "artifact_digest_hex": digest,
                "source_revision": "rev-1",
                "signed": false,
                "unverified_build": true,
                "generation": 1
            }
        });
        std::fs::write(
            dir.join("checkpoint.json"),
            serde_json::to_vec(&checkpoint).unwrap(),
        )
        .unwrap();
        let view = load_lifecycle_session_from_root(&root).expect("published session");
        assert_eq!(view.confirmation_lines()[0], "INSTALL UNSIGNED 1 SYSTEMS");
        assert_eq!(view.confirmation_lines()[1], format!("scope {digest}"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reset_checkpoint_shows_the_wipe_phrase_for_recovery_reset() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-shell-lifecycle-reset-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let dir = root.join("lifecycle").join("seat-15");
        std::fs::create_dir_all(&dir).expect("temp checkpoint dir");
        let checkpoint = serde_json::json!({
            "plan": {
                "schema_version": 1,
                "request_id": "request-3",
                "target_id": "seat-15",
                "intent": "reset_and_onboard",
                "generation": 1,
                "steps": ["offboard", "identity", "verify"]
            },
            "progress": {
                "schema_version": 1,
                "request_id": "request-3",
                "target_id": "seat-15",
                "generation": 1,
                "phase": "planned",
                "completed_steps": 0,
                "total_steps": 3
            }
        });
        std::fs::write(
            dir.join("checkpoint.json"),
            serde_json::to_vec(&checkpoint).unwrap(),
        )
        .unwrap();
        let view = load_lifecycle_session_from_root(&root).expect("published session");
        assert_eq!(view.confirmation_lines()[0], "WIPE 1 SYSTEMS");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn offboard_checkpoint_shows_the_force_offboard_phrase() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-shell-lifecycle-offboard-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let dir = root.join("lifecycle").join("seat-15");
        std::fs::create_dir_all(&dir).expect("temp checkpoint dir");
        let checkpoint = serde_json::json!({
            "plan": {
                "schema_version": 1,
                "request_id": "request-4",
                "target_id": "seat-15",
                "intent": "offboard",
                "generation": 1,
                "steps": ["offboard", "verify"]
            },
            "progress": {
                "schema_version": 1,
                "request_id": "request-4",
                "target_id": "seat-15",
                "generation": 1,
                "phase": "planned",
                "completed_steps": 0,
                "total_steps": 2
            }
        });
        std::fs::write(
            dir.join("checkpoint.json"),
            serde_json::to_vec(&checkpoint).unwrap(),
        )
        .unwrap();
        let view = load_lifecycle_session_from_root(&root).expect("published session");
        assert_eq!(view.confirmation_lines()[0], "FORCE OFFBOARD 1 SYSTEMS");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hydrates_the_fleet_offboard_phrase_from_durable_seats() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-shell-lifecycle-fleet-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        for (target, request, last_error) in [
            ("seat-15", "request-4", None),
            ("seat-16", "request-4", Some("wave-2 timeout")),
        ] {
            let dir = root.join("lifecycle").join(target);
            std::fs::create_dir_all(&dir).expect("temp checkpoint dir");
            let mut checkpoint = serde_json::json!({
                "plan": {
                    "schema_version": 1,
                    "request_id": request,
                    "target_id": target,
                    "intent": "offboard",
                    "generation": 1,
                    "steps": ["offboard", "verify"]
                },
                "progress": {
                    "schema_version": 1,
                    "request_id": request,
                    "target_id": target,
                    "generation": 1,
                    "phase": "planned",
                    "completed_steps": 0,
                    "total_steps": 2
                },
                "coordinator_id": "coord-b",
                "last_error": last_error
            });
            if target == "seat-16" {
                checkpoint["checks"] = serde_json::json!([{
                    "schema_version": 1,
                    "check_id": "mesh",
                    "target_id": "seat-16",
                    "expected": "joined",
                    "observed": "absent",
                    "status": "fail",
                    "required": true,
                    "evidence_digest_hex": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "generation": 1
                }]);
                checkpoint["correction_plan"] = serde_json::json!({
                    "schema_version": 1,
                    "request_id": request,
                    "target_id": "seat-16",
                    "generation": 1,
                    "corrections": [{
                        "check_id": "mesh",
                        "step": "mesh",
                        "reason": "absent",
                        "prerequisites": []
                    }],
                    "edges": [],
                    "rollback_forbidden": true
                });
            }
            std::fs::write(
                dir.join("checkpoint.json"),
                serde_json::to_vec(&checkpoint).unwrap(),
            )
            .unwrap();
            std::fs::write(dir.join("lifecycle.lock"), b"").unwrap();
        }
        let other = root.join("lifecycle").join("seat-17");
        std::fs::create_dir_all(&other).expect("other generation dir");
        std::fs::write(
            other.join("checkpoint.json"),
            serde_json::to_vec(&serde_json::json!({
                "plan": {
                    "schema_version": 1,
                    "request_id": "aaa-old",
                    "target_id": "seat-17",
                    "intent": "offboard",
                    "generation": 1,
                    "steps": ["offboard", "verify"]
                },
                "progress": {
                    "schema_version": 1,
                    "request_id": "aaa-old",
                    "target_id": "seat-17",
                    "generation": 1,
                    "phase": "planned",
                    "completed_steps": 0,
                    "total_steps": 2
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let view = load_lifecycle_session_from_root(&root).expect("published session");
        assert_eq!(view.confirmation_lines()[0], "FORCE OFFBOARD 2 SYSTEMS");
        assert_eq!(view.fleet_line().as_deref(), Some("fleet seat-15, seat-16"));
        assert_eq!(
            view.coordinator_line().as_deref(),
            Some("coordinator coord-b")
        );
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
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hydrates_coordinator_after_a_wiped_sibling() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-shell-lifecycle-wiped-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        for target in ["seat-15", "seat-16"] {
            let dir = root.join("lifecycle").join(target);
            std::fs::create_dir_all(&dir).expect("temp checkpoint dir");
            let checkpoint = serde_json::json!({
                "plan": {
                    "schema_version": 1,
                    "request_id": "request-wipe",
                    "target_id": target,
                    "intent": "offboard",
                    "generation": 1,
                    "steps": ["offboard", "verify"]
                },
                "progress": {
                    "schema_version": 1,
                    "request_id": "request-wipe",
                    "target_id": target,
                    "generation": 1,
                    "phase": "planned",
                    "completed_steps": 0,
                    "total_steps": 2
                },
                "coordinator_id": "coord-b"
            });
            std::fs::write(
                dir.join("checkpoint.json"),
                serde_json::to_vec(&checkpoint).unwrap(),
            )
            .unwrap();
        }
        std::fs::remove_dir_all(root.join("lifecycle").join("seat-16")).unwrap();
        let view = load_lifecycle_session_from_root(&root).expect("published session");
        assert_eq!(
            view.coordinator_line().as_deref(),
            Some("coordinator coord-b")
        );
        assert_eq!(view.fleet_line(), None);
        assert_eq!(view.confirmation_lines()[0], "FORCE OFFBOARD 1 SYSTEMS");
        assert_eq!(view.targets, vec!["seat-15".to_string()]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hydrates_the_durable_coordinator_from_the_checkpoint() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-shell-lifecycle-coord-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let dir = root.join("lifecycle").join("seat-15");
        std::fs::create_dir_all(&dir).expect("temp checkpoint dir");
        let checkpoint = serde_json::json!({
            "plan": {
                "schema_version": 1,
                "request_id": "request-5",
                "target_id": "seat-15",
                "intent": "onboard",
                "generation": 1,
                "steps": ["identity", "verify"]
            },
            "progress": {
                "schema_version": 1,
                "request_id": "request-5",
                "target_id": "seat-15",
                "generation": 1,
                "phase": "running",
                "completed_steps": 0,
                "total_steps": 2
            },
            "coordinator_id": "coord-b"
        });
        std::fs::write(
            dir.join("checkpoint.json"),
            serde_json::to_vec(&checkpoint).unwrap(),
        )
        .unwrap();
        let view = load_lifecycle_session_from_root(&root).expect("published session");
        assert_eq!(
            view.coordinator_line().as_deref(),
            Some("coordinator coord-b")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hydrates_the_first_still_blocking_correction() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-shell-lifecycle-vac-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let dir = root.join("lifecycle").join("seat-15");
        std::fs::create_dir_all(&dir).expect("temp checkpoint dir");
        let checkpoint = serde_json::json!({
            "plan": {
                "schema_version": 1,
                "request_id": "request-vac",
                "target_id": "seat-15",
                "intent": "verify_and_correct",
                "generation": 1,
                "steps": ["mesh", "verify"]
            },
            "progress": {
                "schema_version": 1,
                "request_id": "request-vac",
                "target_id": "seat-15",
                "generation": 1,
                "phase": "running",
                "completed_steps": 0,
                "total_steps": 2
            },
            "checks": [{
                "schema_version": 1,
                "check_id": "mesh",
                "target_id": "seat-15",
                "expected": "joined",
                "observed": "absent",
                "status": "fail",
                "required": true,
                "evidence_digest_hex": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "generation": 1
            }],
            "correction_plan": {
                "schema_version": 1,
                "request_id": "request-vac",
                "target_id": "seat-15",
                "generation": 1,
                "corrections": [{
                    "check_id": "mesh",
                    "step": "mesh",
                    "reason": "absent",
                    "prerequisites": []
                }],
                "edges": [],
                "rollback_forbidden": true
            }
        });
        std::fs::write(
            dir.join("checkpoint.json"),
            serde_json::to_vec(&checkpoint).unwrap(),
        )
        .unwrap();
        let view = load_lifecycle_session_from_root(&root).expect("published session");
        assert_eq!(view.correction_line(), Some("correct mesh: mesh (absent)"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hydrates_the_last_error_line() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-shell-lifecycle-err-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let dir = root.join("lifecycle").join("seat-15");
        std::fs::create_dir_all(&dir).expect("temp checkpoint dir");
        let checkpoint = serde_json::json!({
            "plan": {
                "schema_version": 1,
                "request_id": "request-err",
                "target_id": "seat-15",
                "intent": "verify_and_correct",
                "generation": 1,
                "steps": ["verify", "verify"]
            },
            "progress": {
                "schema_version": 1,
                "request_id": "request-err",
                "target_id": "seat-15",
                "generation": 1,
                "phase": "running",
                "completed_steps": 0,
                "total_steps": 2
            },
            "last_error": "provider timeout"
        });
        std::fs::write(
            dir.join("checkpoint.json"),
            serde_json::to_vec(&checkpoint).unwrap(),
        )
        .unwrap();
        let view = load_lifecycle_session_from_root(&root).expect("published session");
        assert_eq!(
            view.last_error_line().as_deref(),
            Some("last error: provider timeout")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hydrates_the_onboard_nag() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-shell-lifecycle-nag-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let dir = root.join("lifecycle").join("seat-15");
        std::fs::create_dir_all(&dir).expect("temp checkpoint dir");
        let checkpoint = serde_json::json!({
            "plan": {
                "schema_version": 1,
                "request_id": "request-nag",
                "target_id": "seat-15",
                "intent": "onboard",
                "generation": 1,
                "steps": ["mesh", "verify"]
            },
            "progress": {
                "schema_version": 1,
                "request_id": "request-nag",
                "target_id": "seat-15",
                "generation": 1,
                "phase": "running",
                "completed_steps": 0,
                "total_steps": 2
            },
            "checks": [{
                "schema_version": 1,
                "check_id": "mesh_identity",
                "target_id": "seat-15",
                "expected": "enrolled mesh identity",
                "observed": "missing: overlay-ip,etcd-endpoints",
                "status": "fail",
                "required": true,
                "evidence_digest_hex": "3333333333333333333333333333333333333333333333333333333333333333",
                "generation": 1
            }]
        });
        std::fs::write(
            dir.join("checkpoint.json"),
            serde_json::to_vec(&checkpoint).unwrap(),
        )
        .unwrap();
        let view = load_lifecycle_session_from_root(&root).expect("published session");
        assert_eq!(
            view.onboard_nag_line(),
            Some("open ONBOARD: missing overlay-ip,etcd-endpoints")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hydrates_the_durable_offboard_receipt() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-shell-lifecycle-receipt-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let dir = root.join("lifecycle").join("seat-15");
        std::fs::create_dir_all(&dir).expect("temp checkpoint dir");
        let checkpoint = serde_json::json!({
            "plan": {
                "schema_version": 1,
                "request_id": "request-receipt",
                "target_id": "seat-15",
                "intent": "offboard",
                "generation": 1,
                "steps": ["offboard", "verify"]
            },
            "progress": {
                "schema_version": 1,
                "request_id": "request-receipt",
                "target_id": "seat-15",
                "generation": 1,
                "phase": "succeeded",
                "completed_steps": 2,
                "total_steps": 2
            }
        });
        std::fs::write(
            dir.join("checkpoint.json"),
            serde_json::to_vec(&checkpoint).unwrap(),
        )
        .unwrap();
        let receipt = serde_json::json!({
            "schema_version": 1,
            "request_id": "request-receipt",
            "target_id": "seat-15",
            "generation": 1,
            "completed": true,
            "retained_resources": []
        });
        std::fs::write(
            dir.join("receipt.json"),
            serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();
        let view = load_lifecycle_session_from_root(&root).expect("published session");
        assert_eq!(view.receipt_line(), Some("offboard receipt completed"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hydrates_the_staged_package_pin() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-shell-lifecycle-staged-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let dir = root.join("lifecycle").join("seat-15");
        std::fs::create_dir_all(&dir).expect("temp checkpoint dir");
        let checkpoint = serde_json::json!({
            "plan": {
                "schema_version": 1,
                "request_id": "request-staged",
                "target_id": "seat-15",
                "intent": "onboard",
                "generation": 1,
                "steps": ["packages", "verify"]
            },
            "progress": {
                "schema_version": 1,
                "request_id": "request-staged",
                "target_id": "seat-15",
                "generation": 1,
                "phase": "running",
                "completed_steps": 1,
                "total_steps": 2
            }
        });
        std::fs::write(
            dir.join("checkpoint.json"),
            serde_json::to_vec(&checkpoint).unwrap(),
        )
        .unwrap();
        let digest = "e262f1de2c38fd96cb1a8a8410f58222f0e0b5681b84217b877e78c114eb9a31";
        std::fs::write(dir.join("staged-artifact"), b"rpm-bytes").unwrap();
        std::fs::write(dir.join("staged-artifact.digest"), format!("{digest}\n")).unwrap();
        std::fs::write(dir.join("staged-artifact.shape"), "rpm\n").unwrap();
        let view = load_lifecycle_session_from_root(&root).expect("published session");
        assert_eq!(
            view.package_line(),
            Some(format!("packages staged:{digest}:rpm (not installed)"))
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn construct_session_names_a_staged_capsule_without_claiming_confirm() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-construct-lifecycle-capsule-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let dir = root.join("lifecycle").join("seat-15");
        std::fs::create_dir_all(dir.join("capsule")).expect("temp capsule dir");
        let checkpoint = serde_json::json!({
            "plan": {
                "schema_version": 1,
                "request_id": "request-capsule",
                "target_id": "seat-15",
                "intent": "onboard",
                "generation": 1,
                "steps": ["identity", "verify"]
            },
            "progress": {
                "schema_version": 1,
                "request_id": "request-capsule",
                "target_id": "seat-15",
                "generation": 1,
                "phase": "running",
                "completed_steps": 0,
                "total_steps": 2
            },
            "pending_capsule_ids": ["cap-gui"]
        });
        std::fs::write(
            dir.join("checkpoint.json"),
            serde_json::to_vec(&checkpoint).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join("capsule").join("cap-gui"),
            b"{\"capsule_id\":\"cap-gui\"}",
        )
        .unwrap();
        let view = load_lifecycle_session_from_root(&root).expect("published session");
        assert_eq!(
            view.capsule_line().as_deref(),
            Some("capsule cap-gui staged (not confirmed)")
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
