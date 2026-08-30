//! WL-FUNC-023 — one declared-step executor behind the lifecycle authority.
//!
//! CLI verbs keep specialized `run_next` closures. This module is the shared
//! path for the canonical step vocabulary so a renderer cannot invent a
//! package, enrollment, or offboard effect. Dest material is never implied.

use std::path::Path;

use mackes_mesh_types::lifecycle::{
    canonical_lifecycle_baseline, LifecycleCheckStatus, LifecycleConfirmationAction,
    LifecycleIntentKind, LifecycleRequirementCheckV1, LifecycleStepKind,
};
use sha2::{Digest, Sha256};

use crate::lifecycle_authority::LifecycleCheckpointV1;

/// Optional inputs a caller already holds. Missing inputs fail closed.
pub struct StepInputs<'a> {
    pub artifact_bytes_path: Option<&'a Path>,
    pub artifact_shape: Option<ArtifactShape>,
    pub marker_dir: Option<&'a Path>,
}

impl Default for StepInputs<'static> {
    fn default() -> Self {
        Self {
            artifact_bytes_path: None,
            artifact_shape: None,
            marker_dir: None,
        }
    }
}

/// Pinned package form. The executor never silently substitutes another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactShape {
    Rpm,
    Bootc,
    Kickstart,
    NoCloud,
    Usb,
}

pub fn execute_declared_step(
    step: &str,
    checkpoint: &LifecycleCheckpointV1,
    inputs: &StepInputs<'_>,
) -> Result<(), String> {
    match step {
        "packages" => apply_packages(checkpoint, inputs),
        "identity" => apply_identity(checkpoint, inputs),
        "configuration" => apply_plane(checkpoint, inputs, "configuration"),
        "mesh" => apply_plane(checkpoint, inputs, "mesh"),
        "compute" => require_passed_check(checkpoint, "compute"),
        "ui" => require_passed_check(checkpoint, "ui"),
        "hardware" => require_passed_check(checkpoint, "hardware"),
        "verify" => apply_verify(checkpoint, inputs),
        "offboard" => apply_offboard(checkpoint, inputs),
        other => Err(format!("unknown lifecycle step {other}")),
    }
}

fn infer_artifact_shape(path: &Path) -> Option<ArtifactShape> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    if name.ends_with(".rpm") {
        Some(ArtifactShape::Rpm)
    } else if name.contains("bootc") || name.ends_with(".oci") {
        Some(ArtifactShape::Bootc)
    } else if name.ends_with(".ks") || name.contains("kickstart") {
        Some(ArtifactShape::Kickstart)
    } else if name.contains("nocloud") || name == "user-data" {
        Some(ArtifactShape::NoCloud)
    } else if name.ends_with(".iso") || name.contains("usb") {
        Some(ArtifactShape::Usb)
    } else {
        None
    }
}

fn apply_packages(
    checkpoint: &LifecycleCheckpointV1,
    inputs: &StepInputs<'_>,
) -> Result<(), String> {
    let selection = checkpoint
        .artifact_selection
        .as_ref()
        .ok_or_else(|| "artifact selection missing".to_owned())?;
    if selection.target_id != checkpoint.plan.target_id {
        return Err("artifact target is outside plan scope".into());
    }
    if selection.generation != checkpoint.plan.generation {
        return Err("artifact generation does not match plan".into());
    }
    if selection.unverified_build && checkpoint.confirmation.is_none() {
        return Err("unsigned artifact requires digest-bound confirmation".into());
    }
    let path = inputs.artifact_bytes_path.ok_or_else(|| {
        "package bytes not supplied; RPM/bootc/USB install is not implied".to_owned()
    })?;
    let inferred = infer_artifact_shape(path).ok_or_else(|| {
        "unsupported package shape; RPM/bootc/Kickstart/NoCloud/USB only".to_owned()
    })?;
    if let Some(declared) = inputs.artifact_shape {
        if declared != inferred {
            return Err("artifact shape does not match the pinned package form".into());
        }
    }
    let bytes =
        std::fs::read(path).map_err(|error| format!("package bytes unreadable: {error}"))?;
    let digest = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if digest != selection.artifact_digest_hex {
        return Err("package bytes do not match the pinned artifact digest".into());
    }
    // Pin is not install. Stage the exact bytes and queue first-boot
    // convergence so RPM/bootc/USB apply stays dest-gated. Upgrade must
    // leave already-valid join dests untouched.
    let upgrade_snapshot = if checkpoint.plan.intent == LifecycleIntentKind::Upgrade {
        inputs.marker_dir.map(snapshot_upgrade_state).transpose()?
    } else {
        None
    };
    stage_pinned_artifact(inputs, &bytes, &digest, inferred)?;
    queue_pending_convergence(inputs)?;
    if let (Some(dir), Some(snapshot)) = (inputs.marker_dir, upgrade_snapshot.as_deref()) {
        assert_upgrade_state_preserved(dir, snapshot)?;
    }
    Ok(())
}

fn upgrade_state_names() -> [&'static str; 5] {
    [
        crate::onboard::firstboot::STAGED_OVERLAY_IP,
        crate::onboard::firstboot::STAGED_ETCD_ENDPOINTS,
        crate::onboard::firstboot::JOIN_OVERLAY_IP_PIN,
        crate::onboard::firstboot::JOIN_ETCD_ENDPOINTS_PIN,
        crate::onboard::firstboot::STAGED_GROUPED_PLANE,
    ]
}

fn snapshot_upgrade_state(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut kept = Vec::new();
    for name in upgrade_state_names() {
        let path = dir.join(name);
        refuse_marker_symlink(&path)?;
        if path.is_file() {
            let bytes = std::fs::read(&path)
                .map_err(|error| format!("cannot read valid {name}: {error}"))?;
            kept.push((name.to_owned(), bytes));
        }
    }
    Ok(kept)
}

fn assert_upgrade_state_preserved(
    dir: &Path,
    snapshot: &[(String, Vec<u8>)],
) -> Result<(), String> {
    for (name, bytes) in snapshot {
        let path = dir.join(name);
        refuse_marker_symlink(&path)?;
        let observed = std::fs::read(&path)
            .map_err(|_| format!("upgrade deleted valid {name}; dest wipe is not implied"))?;
        if observed.as_slice() != bytes.as_slice() {
            return Err(format!(
                "upgrade mutated valid {name}; dest wipe is not implied"
            ));
        }
    }
    Ok(())
}

fn artifact_shape_name(shape: ArtifactShape) -> &'static str {
    match shape {
        ArtifactShape::Rpm => "rpm",
        ArtifactShape::Bootc => "bootc",
        ArtifactShape::Kickstart => "kickstart",
        ArtifactShape::NoCloud => "nocloud",
        ArtifactShape::Usb => "usb",
    }
}

fn refuse_marker_symlink(path: &Path) -> Result<(), String> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("marker");
            return Err(format!(
                "{name} must not be a symlink; dest apply is not implied"
            ));
        }
    }
    Ok(())
}

fn stage_pinned_artifact(
    inputs: &StepInputs<'_>,
    bytes: &[u8],
    digest: &str,
    shape: ArtifactShape,
) -> Result<(), String> {
    let marker_dir = inputs.marker_dir.ok_or_else(|| {
        "staged-artifact dir is missing; dest RPM/bootc/USB install is not implied".to_owned()
    })?;
    std::fs::create_dir_all(marker_dir)
        .map_err(|error| format!("cannot stage pinned artifact: {error}"))?;
    let staged = marker_dir.join("staged-artifact");
    let digest_path = marker_dir.join("staged-artifact.digest");
    let shape_path = marker_dir.join("staged-artifact.shape");
    refuse_marker_symlink(&staged)?;
    refuse_marker_symlink(&digest_path)?;
    refuse_marker_symlink(&shape_path)?;
    std::fs::write(&staged, bytes)
        .map_err(|error| format!("cannot stage pinned artifact: {error}"))?;
    std::fs::write(&digest_path, format!("{digest}\n"))
        .map_err(|error| format!("cannot record staged digest: {error}"))?;
    std::fs::write(&shape_path, format!("{}\n", artifact_shape_name(shape)))
        .map_err(|error| format!("cannot record staged shape: {error}"))
}

fn queue_pending_convergence(inputs: &StepInputs<'_>) -> Result<(), String> {
    let marker_dir = inputs.marker_dir.ok_or_else(|| {
        "pending-convergence marker dir is missing; dest repair is not implied".to_owned()
    })?;
    std::fs::create_dir_all(marker_dir)
        .map_err(|error| format!("cannot queue pending-convergence: {error}"))?;
    let pending = marker_dir.join(crate::onboard::firstboot::FIRSTBOOT_PENDING);
    refuse_marker_symlink(&pending)?;
    std::fs::write(&pending, b"queued\n")
        .map_err(|error| format!("cannot queue pending-convergence: {error}"))
}

fn is_final_step(checkpoint: &LifecycleCheckpointV1) -> bool {
    checkpoint.progress.completed_steps + 1 == checkpoint.progress.total_steps
}

fn apply_plane(
    checkpoint: &LifecycleCheckpointV1,
    inputs: &StepInputs<'_>,
    check_id: &str,
) -> Result<(), String> {
    let observed = observed_plane_checks(checkpoint, check_id);
    if observed.is_empty() {
        return Err(format!(
            "{check_id} not observed; not implied repaired and dest material is not invented"
        ));
    }
    if !observed.iter().any(|check| check.blocks_progress()) {
        return Ok(());
    }
    if matches!(check_id, "mesh" | "configuration") {
        if let Some(dir) = inputs.marker_dir {
            if crate::onboard::firstboot::stage_mesh_join_dests(dir)
                .map_err(|error| format!("cannot stage join dests: {error}"))?
            {
                queue_pending_convergence(inputs)?;
                return Ok(());
            }
        }
    }
    if matches!(
        checkpoint.plan.intent,
        LifecycleIntentKind::VerifyAndCorrect | LifecycleIntentKind::Upgrade
    ) {
        return queue_pending_convergence(inputs);
    }
    let check = observed
        .iter()
        .find(|check| check.blocks_progress())
        .expect("blocking check present");
    Err(format!("{} not ready: {}", check.check_id, check.observed))
}

fn observed_plane_checks<'a>(
    checkpoint: &'a LifecycleCheckpointV1,
    step: &str,
) -> Vec<&'a LifecycleRequirementCheckV1> {
    let owner_ids = LifecycleStepKind::parse(step).map(|kind| {
        canonical_lifecycle_baseline()
            .into_iter()
            .filter(|entry| entry.owner_step == kind || entry.correction_step == kind)
            .map(|entry| entry.requirement_id)
            .collect::<Vec<_>>()
    });
    checkpoint
        .checks
        .iter()
        .filter(|check| {
            check.check_id == step
                || owner_ids
                    .as_ref()
                    .is_some_and(|ids| ids.iter().any(|id| id == &check.check_id))
        })
        .collect()
}

fn apply_identity(
    checkpoint: &LifecycleCheckpointV1,
    inputs: &StepInputs<'_>,
) -> Result<(), String> {
    if checkpoint.plan.intent == LifecycleIntentKind::ResetAndOnboard {
        if let Some(dir) = inputs.marker_dir {
            refuse_marker_symlink(&dir.join("erasure"))?;
            refuse_marker_symlink(&dir.join("wiped"))?;
        }
        if checkpoint.progress.completed_steps == 0 {
            return Err("reset identity cannot run before offboard".into());
        }
        match checkpoint
            .checks
            .iter()
            .find(|check| check.check_id == "old_identity")
        {
            Some(check) if check.status == LifecycleCheckStatus::Pass => {
                return Err("old identity still present; reset cannot coexist".into());
            }
            None => {
                return Err("old identity revocation not observed".into());
            }
            Some(_) => {}
        }
        if !checkpoint.checks.iter().any(|check| {
            check.check_id == "erasure"
                && check.generation == checkpoint.plan.generation
                && check.status == LifecycleCheckStatus::Pass
                && !check.blocks_progress()
        }) {
            return Err("erasure not observed; dest wipe is not implied".into());
        }
        if let Some(dir) = inputs.marker_dir {
            refuse_leftover_identity(dir)?;
        }
    }
    require_passed_check(checkpoint, "identity")
}

fn leftover_identity_names() -> [&'static str; 4] {
    [
        crate::onboard::firstboot::STAGED_OVERLAY_IP,
        crate::onboard::firstboot::STAGED_ETCD_ENDPOINTS,
        crate::onboard::firstboot::JOIN_OVERLAY_IP_PIN,
        crate::onboard::firstboot::JOIN_ETCD_ENDPOINTS_PIN,
    ]
}

/// ResetAndOnboard may start ordinary onboard only after wipe is observed
/// and no previous join identity remains under the supplied root.
fn refuse_leftover_identity(dir: &Path) -> Result<(), String> {
    for name in leftover_identity_names() {
        let path = dir.join(name);
        refuse_marker_symlink(&path)?;
        if path.is_file() {
            return Err(format!(
                "old identity leftover {name}; reset cannot coexist"
            ));
        }
    }
    Ok(())
}

fn apply_verify(checkpoint: &LifecycleCheckpointV1, inputs: &StepInputs<'_>) -> Result<(), String> {
    if let Some(dir) = inputs.marker_dir {
        let pending = dir.join(crate::onboard::firstboot::FIRSTBOOT_PENDING);
        refuse_marker_symlink(&pending)?;
        if pending.is_file() && is_final_step(checkpoint) {
            return Err(
                "pending-convergence queued; dest RPM/bootc/USB apply is not implied".into(),
            );
        }
    }
    let blocked = checkpoint
        .checks
        .iter()
        .any(mackes_mesh_types::lifecycle::LifecycleRequirementCheckV1::blocks_progress);
    if matches!(
        checkpoint.plan.intent,
        LifecycleIntentKind::VerifyAndCorrect | LifecycleIntentKind::Upgrade
    ) && !is_final_step(checkpoint)
    {
        if blocked {
            queue_pending_convergence(inputs)?;
        }
        return Ok(());
    }
    if blocked {
        return Err("verify blocked by required checks".into());
    }
    Ok(())
}

fn apply_offboard(
    checkpoint: &LifecycleCheckpointV1,
    inputs: &StepInputs<'_>,
) -> Result<(), String> {
    if let Some(dir) = inputs.marker_dir {
        refuse_marker_symlink(&dir.join("erasure"))?;
        refuse_marker_symlink(&dir.join("wiped"))?;
    }
    if checkpoint.plan.intent != LifecycleIntentKind::Offboard
        && checkpoint.plan.intent != LifecycleIntentKind::ResetAndOnboard
    {
        return Err("offboard step is outside this intent".into());
    }
    match &checkpoint.confirmation {
        Some(confirmation)
            if matches!(
                confirmation.action,
                LifecycleConfirmationAction::Offboard | LifecycleConfirmationAction::Reset
            ) =>
        {
            Ok(())
        }
        _ => Err("offboard requires signed confirmation".into()),
    }
}

fn require_passed_check(checkpoint: &LifecycleCheckpointV1, check_id: &str) -> Result<(), String> {
    match checkpoint
        .checks
        .iter()
        .find(|check| check.check_id == check_id)
    {
        Some(check) if check.status == LifecycleCheckStatus::Pass && !check.blocks_progress() => {
            Ok(())
        }
        Some(check) => Err(format!("{check_id} not ready: {}", check.observed)),
        None => Err(format!(
            "{check_id} not observed; not implied ready and dest material is not invented"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle_authority::{LifecycleAuthority, LifecycleAuthorityError};
    use mackes_mesh_types::lifecycle::{
        LifecycleArtifactChannel, LifecycleArtifactSelectionV1, LifecycleIntentKind,
        LifecyclePlanV1, LifecycleRequirementCheckV1,
    };

    fn onboard_packages_plan() -> LifecyclePlanV1 {
        LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::Onboard,
            generation: 1,
            steps: vec!["packages".into(), "verify".into()],
        }
    }

    fn signed_selection() -> LifecycleArtifactSelectionV1 {
        LifecycleArtifactSelectionV1 {
            schema_version: 1,
            selection_id: "sel-1".into(),
            target_id: "seat-15".into(),
            channel: LifecycleArtifactChannel::Candidate,
            artifact_digest_hex: String::new(),
            source_revision: "rev-1".into(),
            signed: true,
            unverified_build: false,
            generation: 1,
        }
    }

    fn digest_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    #[test]
    fn packages_step_pins_supplied_bytes_and_refuses_a_mismatch() {
        let root = tempfile::tempdir().unwrap();
        let mut authority =
            LifecycleAuthority::begin(root.path(), onboard_packages_plan()).unwrap();
        let artifact = root.path().join("role.rpm");
        std::fs::write(&artifact, b"rpm-bytes").unwrap();
        let mut selection = signed_selection();
        selection.artifact_digest_hex = digest_hex(b"rpm-bytes");
        authority.select_artifact(selection.clone()).unwrap();
        authority
            .run_next_declared(Some(artifact.as_path()))
            .unwrap();
        assert_eq!(
            authority.checkpoint().progress.completed_steps,
            1,
            "matching pinned bytes must complete the packages step"
        );
        let staged = root.path().join("lifecycle").join("staged-artifact");
        assert_eq!(
            std::fs::read(&staged).unwrap(),
            b"rpm-bytes",
            "onboard must stage the pinned bytes instead of implying dest install"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("lifecycle").join("staged-artifact.shape"))
                .unwrap()
                .trim(),
            "rpm"
        );
        assert!(
            pending_marker(root.path()).exists(),
            "onboard packages must queue pending-convergence after stage"
        );
        let error = authority
            .run_next_declared(Some(artifact.as_path()))
            .expect_err("final onboard verify must not claim Ready before dest apply");
        assert!(
            matches!(
                error,
                LifecycleAuthorityError::StepFailed(ref message)
                    if message.contains("pending-convergence") && message.contains("dest")
            ),
            "{error:?}"
        );
        authority.finish().unwrap();

        let mut authority = LifecycleAuthority::begin(root.path(), {
            let mut plan = onboard_packages_plan();
            plan.request_id = "request-2".into();
            plan.generation = 2;
            plan
        })
        .unwrap();
        selection.generation = 2;
        selection.selection_id = "sel-2".into();
        authority.select_artifact(selection).unwrap();
        std::fs::write(&artifact, b"other-bytes").unwrap();
        let error = authority
            .run_next_declared(Some(artifact.as_path()))
            .expect_err("changed bytes must not complete packages");
        assert!(matches!(error, LifecycleAuthorityError::StepFailed(_)));
        authority.finish().unwrap();
    }

    #[test]
    fn packages_step_does_not_invent_rpm_or_bootc_bytes() {
        let root = tempfile::tempdir().unwrap();
        let mut authority =
            LifecycleAuthority::begin(root.path(), onboard_packages_plan()).unwrap();
        let mut selection = signed_selection();
        selection.artifact_digest_hex = "ab".repeat(32);
        authority.select_artifact(selection).unwrap();
        let error = authority
            .run_next_declared(None)
            .expect_err("missing bytes must stay unpublished");
        assert!(
            matches!(error, LifecycleAuthorityError::StepFailed(message) if message.contains("not implied"))
        );
        authority.finish().unwrap();
    }

    #[test]
    fn identity_step_refuses_an_unobserved_requirement() {
        let root = tempfile::tempdir().unwrap();
        let plan = LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::Onboard,
            generation: 1,
            steps: vec!["identity".into(), "verify".into()],
        };
        let mut authority = LifecycleAuthority::begin(root.path(), plan).unwrap();
        let error = authority
            .run_next_declared(None)
            .expect_err("identity is not implied ready");
        assert!(
            matches!(error, LifecycleAuthorityError::StepFailed(message) if message.contains("not observed"))
        );
        authority.finish().unwrap();
        let mut plan = onboard_packages_plan();
        plan.steps = vec!["identity".into(), "verify".into()];
        plan.request_id = "request-3".into();
        plan.generation = 3;
        let mut authority = LifecycleAuthority::begin(root.path(), plan).unwrap();
        authority
            .record_check(LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "identity".into(),
                target_id: "seat-15".into(),
                expected: "present".into(),
                observed: "present".into(),
                status: LifecycleCheckStatus::Pass,
                required: true,
                evidence_digest_hex: "2".repeat(64),
                warning: None,
                generation: 3,
            })
            .unwrap();
        authority.run_next_declared(None).unwrap();
        authority.finish().unwrap();
    }

    #[test]
    fn offboard_step_requires_confirmation() {
        let root = tempfile::tempdir().unwrap();
        let plan = LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::Offboard,
            generation: 1,
            steps: vec!["offboard".into()],
        };
        let mut authority = LifecycleAuthority::begin(root.path(), plan).unwrap();
        let error = authority
            .run_next_declared(None)
            .expect_err("offboard is not a silent step");
        assert!(
            matches!(error, LifecycleAuthorityError::StepFailed(message) if message.contains("confirmation"))
        );
        authority.finish().unwrap();
    }

    #[test]
    fn reset_identity_refuses_an_old_identity_that_still_passes() {
        let root = tempfile::tempdir().unwrap();
        let plan = LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::ResetAndOnboard,
            generation: 1,
            steps: vec!["offboard".into(), "identity".into(), "verify".into()],
        };
        let mut authority = LifecycleAuthority::begin(root.path(), plan).unwrap();
        let error =
            execute_declared_step("identity", authority.checkpoint(), &StepInputs::default())
                .expect_err("reset cannot mint identity first");
        assert!(error.contains("before offboard"));
        authority
            .record_check(LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "old_identity".into(),
                target_id: "seat-15".into(),
                expected: "revoked".into(),
                observed: "present".into(),
                status: LifecycleCheckStatus::Pass,
                required: true,
                evidence_digest_hex: "2".repeat(64),
                warning: None,
                generation: 1,
            })
            .unwrap();
        let mut progressed = authority.checkpoint().progress.clone();
        progressed.completed_steps = 1;
        progressed.phase = mackes_mesh_types::lifecycle::LifecyclePhase::Running;
        authority.update(progressed).unwrap();
        let error =
            execute_declared_step("identity", authority.checkpoint(), &StepInputs::default())
                .expect_err("old and new identity cannot coexist");
        assert!(error.contains("cannot coexist"));
        authority.finish().unwrap();
    }

    #[test]
    fn reset_identity_refuses_without_observed_erasure() {
        let root = tempfile::tempdir().unwrap();
        let plan = LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::ResetAndOnboard,
            generation: 1,
            steps: vec!["offboard".into(), "identity".into(), "verify".into()],
        };
        let mut authority = LifecycleAuthority::begin(root.path(), plan).unwrap();
        authority
            .record_check(LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "old_identity".into(),
                target_id: "seat-15".into(),
                expected: "revoked".into(),
                observed: "revoked".into(),
                status: LifecycleCheckStatus::Fail,
                required: true,
                evidence_digest_hex: "2".repeat(64),
                warning: None,
                generation: 1,
            })
            .unwrap();
        let mut progressed = authority.checkpoint().progress.clone();
        progressed.completed_steps = 1;
        progressed.phase = mackes_mesh_types::lifecycle::LifecyclePhase::Running;
        authority.update(progressed).unwrap();
        let error =
            execute_declared_step("identity", authority.checkpoint(), &StepInputs::default())
                .expect_err("reset cannot mint identity without observed erase");
        assert!(error.contains("erasure not observed"));
        authority.finish().unwrap();
    }

    #[test]
    fn reset_identity_refuses_an_erasure_symlink() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("dest-wipe");
        std::fs::write(&dest, b"keep").unwrap();
        let lifecycle = root.path().join("lifecycle");
        std::fs::create_dir_all(&lifecycle).unwrap();
        std::os::unix::fs::symlink(&dest, lifecycle.join("erasure")).unwrap();
        let plan = LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::ResetAndOnboard,
            generation: 1,
            steps: vec!["offboard".into(), "identity".into(), "verify".into()],
        };
        let mut authority = LifecycleAuthority::begin(root.path(), plan).unwrap();
        authority
            .record_check(LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "old_identity".into(),
                target_id: "seat-15".into(),
                expected: "revoked".into(),
                observed: "revoked".into(),
                status: LifecycleCheckStatus::Fail,
                required: true,
                evidence_digest_hex: "2".repeat(64),
                warning: None,
                generation: 1,
            })
            .unwrap();
        let mut progressed = authority.checkpoint().progress.clone();
        progressed.completed_steps = 1;
        progressed.phase = mackes_mesh_types::lifecycle::LifecyclePhase::Running;
        authority.update(progressed).unwrap();
        let error = execute_declared_step(
            "identity",
            authority.checkpoint(),
            &StepInputs {
                artifact_bytes_path: None,
                artifact_shape: None,
                marker_dir: Some(lifecycle.as_path()),
            },
        )
        .expect_err("reset identity must not follow a planted erasure symlink");
        assert!(
            error.contains("erasure") && error.contains("symlink"),
            "{error}"
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"keep");
        authority.finish().unwrap();
    }

    fn prepare_reset_after_wipe(
        authority: &mut LifecycleAuthority,
    ) -> Result<(), LifecycleAuthorityError> {
        authority.record_check(LifecycleRequirementCheckV1 {
            schema_version: 1,
            check_id: "old_identity".into(),
            target_id: "seat-15".into(),
            expected: "revoked".into(),
            observed: "revoked".into(),
            status: LifecycleCheckStatus::Fail,
            required: true,
            evidence_digest_hex: "2".repeat(64),
            warning: None,
            generation: 1,
        })?;
        authority.record_check(LifecycleRequirementCheckV1 {
            schema_version: 1,
            check_id: "erasure".into(),
            target_id: "seat-15".into(),
            expected: "erased".into(),
            observed: "erased".into(),
            status: LifecycleCheckStatus::Pass,
            required: true,
            evidence_digest_hex: "e".repeat(64),
            warning: None,
            generation: 1,
        })?;
        let mut progressed = authority.checkpoint().progress.clone();
        progressed.completed_steps = 1;
        progressed.phase = mackes_mesh_types::lifecycle::LifecyclePhase::Running;
        authority.update(progressed)
    }

    #[test]
    fn reset_identity_refuses_leftover_overlay_ip() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(
            dest.join(crate::onboard::firstboot::STAGED_OVERLAY_IP),
            b"10.0.0.15",
        )
        .unwrap();
        let plan = LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::ResetAndOnboard,
            generation: 1,
            steps: vec!["offboard".into(), "identity".into(), "verify".into()],
        };
        let mut authority = LifecycleAuthority::begin(root.path(), plan).unwrap();
        prepare_reset_after_wipe(&mut authority).unwrap();
        let error = execute_declared_step(
            "identity",
            authority.checkpoint(),
            &StepInputs {
                artifact_bytes_path: None,
                artifact_shape: None,
                marker_dir: Some(dest.as_path()),
            },
        )
        .expect_err("leftover overlay-ip is old identity coexistence");
        assert!(
            error.contains("overlay-ip") && error.contains("cannot coexist"),
            "{error}"
        );
        authority.finish().unwrap();
    }

    #[test]
    fn reset_identity_starts_ordinary_onboard_after_wipe() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        let plan = LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::ResetAndOnboard,
            generation: 1,
            steps: vec!["offboard".into(), "identity".into(), "verify".into()],
        };
        let mut authority = LifecycleAuthority::begin(root.path(), plan).unwrap();
        prepare_reset_after_wipe(&mut authority).unwrap();
        authority
            .record_check(LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "identity".into(),
                target_id: "seat-15".into(),
                expected: "issued".into(),
                observed: "issued".into(),
                status: LifecycleCheckStatus::Pass,
                required: true,
                evidence_digest_hex: "3".repeat(64),
                warning: None,
                generation: 1,
            })
            .unwrap();
        execute_declared_step(
            "identity",
            authority.checkpoint(),
            &StepInputs {
                artifact_bytes_path: None,
                artifact_shape: None,
                marker_dir: Some(dest.as_path()),
            },
        )
        .expect("wipe plus no leftover identity starts ordinary onboard");
        authority.finish().unwrap();
    }

    #[test]
    fn packages_step_refuses_shape_substitution() {
        let root = tempfile::tempdir().unwrap();
        let mut authority =
            LifecycleAuthority::begin(root.path(), onboard_packages_plan()).unwrap();
        let artifact = root.path().join("media.iso");
        std::fs::write(&artifact, b"iso-bytes").unwrap();
        let mut selection = signed_selection();
        selection.artifact_digest_hex = digest_hex(b"iso-bytes");
        authority.select_artifact(selection).unwrap();
        let error = execute_declared_step(
            "packages",
            authority.checkpoint(),
            &StepInputs {
                artifact_bytes_path: Some(artifact.as_path()),
                artifact_shape: Some(ArtifactShape::Rpm),
                marker_dir: None,
            },
        )
        .expect_err("ISO bytes must not masquerade as an RPM");
        assert!(error.contains("does not match"));
        authority.finish().unwrap();
    }

    #[test]
    fn packages_step_refuses_an_unknown_shape() {
        let root = tempfile::tempdir().unwrap();
        let mut authority =
            LifecycleAuthority::begin(root.path(), onboard_packages_plan()).unwrap();
        let artifact = root.path().join("blob.bin");
        std::fs::write(&artifact, b"raw-bytes").unwrap();
        let mut selection = signed_selection();
        selection.artifact_digest_hex = digest_hex(b"raw-bytes");
        authority.select_artifact(selection).unwrap();
        let error = authority
            .run_next_declared(Some(artifact.as_path()))
            .expect_err("unknown package shape is not implied installable");
        assert!(
            matches!(error, LifecycleAuthorityError::StepFailed(message) if message.contains("unsupported package shape"))
        );
        authority.finish().unwrap();
    }

    #[test]
    fn upgrade_packages_queue_pending_convergence() {
        let root = tempfile::tempdir().unwrap();
        let mut plan = onboard_packages_plan();
        plan.intent = LifecycleIntentKind::Upgrade;
        let mut authority = LifecycleAuthority::begin(root.path(), plan).unwrap();
        let artifact = root.path().join("role.rpm");
        std::fs::write(&artifact, b"rpm-bytes").unwrap();
        let mut selection = signed_selection();
        selection.artifact_digest_hex = digest_hex(b"rpm-bytes");
        authority.select_artifact(selection).unwrap();
        authority
            .run_next_declared(Some(artifact.as_path()))
            .unwrap();
        assert!(
            root.path()
                .join("lifecycle")
                .join(crate::onboard::firstboot::FIRSTBOOT_PENDING)
                .exists(),
            "upgrade must queue pending-convergence instead of claiming Ready"
        );
        let error = authority
            .run_next_declared(Some(artifact.as_path()))
            .expect_err("final upgrade verify must not claim Ready before dest apply");
        assert!(
            matches!(
                error,
                LifecycleAuthorityError::StepFailed(ref message)
                    if message.contains("pending-convergence") && message.contains("dest")
            ),
            "{error:?}"
        );
        authority.finish().unwrap();
    }

    #[test]
    fn onboard_packages_refuse_without_a_marker_dir() {
        let root = tempfile::tempdir().unwrap();
        let mut authority =
            LifecycleAuthority::begin(root.path(), onboard_packages_plan()).unwrap();
        let artifact = root.path().join("role.rpm");
        std::fs::write(&artifact, b"rpm-bytes").unwrap();
        let mut selection = signed_selection();
        selection.artifact_digest_hex = digest_hex(b"rpm-bytes");
        authority.select_artifact(selection).unwrap();
        let error = execute_declared_step(
            "packages",
            authority.checkpoint(),
            &StepInputs {
                artifact_bytes_path: Some(artifact.as_path()),
                artifact_shape: None,
                marker_dir: None,
            },
        )
        .expect_err("onboard must not complete without a staging path");
        assert!(error.contains("staged-artifact"));
        authority.finish().unwrap();
    }

    #[test]
    fn packages_step_refuses_a_pending_convergence_symlink() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("dest-apply");
        std::fs::write(&dest, b"keep").unwrap();
        std::fs::create_dir_all(root.path().join("lifecycle")).unwrap();
        std::os::unix::fs::symlink(&dest, pending_marker(root.path())).unwrap();
        let mut authority =
            LifecycleAuthority::begin(root.path(), onboard_packages_plan()).unwrap();
        let artifact = root.path().join("role.rpm");
        std::fs::write(&artifact, b"rpm-bytes").unwrap();
        let mut selection = signed_selection();
        selection.artifact_digest_hex = digest_hex(b"rpm-bytes");
        authority.select_artifact(selection).unwrap();
        let error = authority
            .run_next_declared(Some(artifact.as_path()))
            .expect_err("a planted pending-convergence symlink is not dest apply");
        assert!(
            matches!(
                error,
                LifecycleAuthorityError::StepFailed(ref message)
                    if message.contains("pending-convergence") && message.contains("symlink")
            ),
            "{error:?}"
        );
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            b"keep",
            "queue must not follow a symlink into dest material"
        );
        authority.finish().unwrap();
    }

    #[test]
    fn offboard_step_refuses_an_erasure_symlink() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("dest-wipe");
        std::fs::write(&dest, b"keep").unwrap();
        std::fs::create_dir_all(root.path().join("lifecycle")).unwrap();
        std::os::unix::fs::symlink(&dest, root.path().join("lifecycle").join("erasure")).unwrap();
        let mut authority = LifecycleAuthority::begin(
            root.path(),
            LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-offboard".into(),
                target_id: "seat-15".into(),
                intent: LifecycleIntentKind::Offboard,
                generation: 1,
                steps: vec!["offboard".into(), "verify".into()],
            },
        )
        .unwrap();
        let error = authority
            .run_next_declared(None)
            .expect_err("a planted erasure symlink is not dest wipe");
        assert!(
            matches!(
                error,
                LifecycleAuthorityError::StepFailed(ref message)
                    if message.contains("erasure") && message.contains("symlink")
            ),
            "{error:?}"
        );
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            b"keep",
            "offboard must not follow a symlink into dest material"
        );
        authority.finish().unwrap();
    }

    #[test]
    fn offboard_step_refuses_a_wiped_symlink() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("dest-wipe");
        std::fs::write(&dest, b"keep").unwrap();
        std::fs::create_dir_all(root.path().join("lifecycle")).unwrap();
        std::os::unix::fs::symlink(&dest, root.path().join("lifecycle").join("wiped")).unwrap();
        let mut authority = LifecycleAuthority::begin(
            root.path(),
            LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-offboard-wiped".into(),
                target_id: "seat-15".into(),
                intent: LifecycleIntentKind::Offboard,
                generation: 1,
                steps: vec!["offboard".into(), "verify".into()],
            },
        )
        .unwrap();
        let error = authority
            .run_next_declared(None)
            .expect_err("a planted wiped symlink is not dest wipe");
        assert!(
            matches!(
                error,
                LifecycleAuthorityError::StepFailed(ref message)
                    if message.contains("wiped") && message.contains("symlink")
            ),
            "{error:?}"
        );
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            b"keep",
            "offboard must not follow a wiped symlink into dest material"
        );
        authority.finish().unwrap();
    }

    #[test]
    fn packages_step_refuses_a_staged_shape_symlink() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("dest-shape");
        std::fs::write(&dest, b"keep").unwrap();
        std::fs::create_dir_all(root.path().join("lifecycle")).unwrap();
        std::os::unix::fs::symlink(
            &dest,
            root.path().join("lifecycle").join("staged-artifact.shape"),
        )
        .unwrap();
        let mut authority =
            LifecycleAuthority::begin(root.path(), onboard_packages_plan()).unwrap();
        let artifact = root.path().join("role.rpm");
        std::fs::write(&artifact, b"rpm-bytes").unwrap();
        let mut selection = signed_selection();
        selection.artifact_digest_hex = digest_hex(b"rpm-bytes");
        authority.select_artifact(selection).unwrap();
        let error = authority
            .run_next_declared(Some(artifact.as_path()))
            .expect_err("a planted shape symlink is not dest install");
        assert!(
            matches!(
                error,
                LifecycleAuthorityError::StepFailed(ref message)
                    if message.contains("staged-artifact.shape") && message.contains("symlink")
            ),
            "{error:?}"
        );
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            b"keep",
            "stage must not follow a shape symlink into dest material"
        );
        authority.finish().unwrap();
    }

    #[test]
    fn packages_step_refuses_a_staged_artifact_symlink() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("dest-rpm");
        std::fs::write(&dest, b"keep").unwrap();
        std::fs::create_dir_all(root.path().join("lifecycle")).unwrap();
        std::os::unix::fs::symlink(&dest, root.path().join("lifecycle").join("staged-artifact"))
            .unwrap();
        let mut authority =
            LifecycleAuthority::begin(root.path(), onboard_packages_plan()).unwrap();
        let artifact = root.path().join("role.rpm");
        std::fs::write(&artifact, b"rpm-bytes").unwrap();
        let mut selection = signed_selection();
        selection.artifact_digest_hex = digest_hex(b"rpm-bytes");
        authority.select_artifact(selection).unwrap();
        let error = authority
            .run_next_declared(Some(artifact.as_path()))
            .expect_err("a planted staged-artifact symlink is not dest install");
        assert!(
            matches!(
                error,
                LifecycleAuthorityError::StepFailed(ref message)
                    if message.contains("staged-artifact") && message.contains("symlink")
            ),
            "{error:?}"
        );
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            b"keep",
            "stage must not follow a staged-artifact symlink into dest material"
        );
        authority.finish().unwrap();
    }

    #[test]
    fn packages_step_refuses_a_staged_digest_symlink() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("dest-digest");
        std::fs::write(&dest, b"keep").unwrap();
        std::fs::create_dir_all(root.path().join("lifecycle")).unwrap();
        std::os::unix::fs::symlink(
            &dest,
            root.path().join("lifecycle").join("staged-artifact.digest"),
        )
        .unwrap();
        let mut authority =
            LifecycleAuthority::begin(root.path(), onboard_packages_plan()).unwrap();
        let artifact = root.path().join("role.rpm");
        std::fs::write(&artifact, b"rpm-bytes").unwrap();
        let mut selection = signed_selection();
        selection.artifact_digest_hex = digest_hex(b"rpm-bytes");
        authority.select_artifact(selection).unwrap();
        let error = authority
            .run_next_declared(Some(artifact.as_path()))
            .expect_err("a planted digest symlink is not dest install");
        assert!(
            matches!(
                error,
                LifecycleAuthorityError::StepFailed(ref message)
                    if message.contains("staged-artifact.digest") && message.contains("symlink")
            ),
            "{error:?}"
        );
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            b"keep",
            "stage must not follow a digest symlink into dest material"
        );
        authority.finish().unwrap();
    }

    #[test]
    fn reset_offboard_refuses_an_erasure_symlink() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("dest-wipe");
        std::fs::write(&dest, b"keep").unwrap();
        std::fs::create_dir_all(root.path().join("lifecycle")).unwrap();
        std::os::unix::fs::symlink(&dest, root.path().join("lifecycle").join("erasure")).unwrap();
        let mut authority = LifecycleAuthority::begin(
            root.path(),
            LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-reset".into(),
                target_id: "seat-15".into(),
                intent: LifecycleIntentKind::ResetAndOnboard,
                generation: 1,
                steps: vec!["offboard".into(), "identity".into(), "verify".into()],
            },
        )
        .unwrap();
        let error = authority
            .run_next_declared(None)
            .expect_err("reset must not treat a planted erasure symlink as dest wipe");
        assert!(
            matches!(
                error,
                LifecycleAuthorityError::StepFailed(ref message)
                    if message.contains("erasure") && message.contains("symlink")
            ),
            "{error:?}"
        );
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            b"keep",
            "reset must not follow a symlink into dest material"
        );
        authority.finish().unwrap();
    }

    #[test]
    fn upgrade_packages_preserve_valid_join_dests() {
        let root = tempfile::tempdir().unwrap();
        let mut plan = onboard_packages_plan();
        plan.intent = LifecycleIntentKind::Upgrade;
        let mut authority = LifecycleAuthority::begin(root.path(), plan).unwrap();
        let lifecycle = root.path().join("lifecycle");
        std::fs::create_dir_all(&lifecycle).unwrap();
        std::fs::write(
            lifecycle.join(crate::onboard::firstboot::STAGED_OVERLAY_IP),
            b"10.0.0.15",
        )
        .unwrap();
        std::fs::write(
            lifecycle.join(crate::onboard::firstboot::STAGED_ETCD_ENDPOINTS),
            b"https://10.0.0.1:2379",
        )
        .unwrap();
        let artifact = root.path().join("role.rpm");
        std::fs::write(&artifact, b"rpm-bytes").unwrap();
        let mut selection = signed_selection();
        selection.artifact_digest_hex = digest_hex(b"rpm-bytes");
        authority.select_artifact(selection).unwrap();
        authority
            .run_next_declared(Some(artifact.as_path()))
            .unwrap();
        assert_eq!(
            std::fs::read(lifecycle.join(crate::onboard::firstboot::STAGED_OVERLAY_IP)).unwrap(),
            b"10.0.0.15"
        );
        assert_eq!(
            std::fs::read(lifecycle.join(crate::onboard::firstboot::STAGED_ETCD_ENDPOINTS))
                .unwrap(),
            b"https://10.0.0.1:2379"
        );
        assert!(
            lifecycle
                .join(crate::onboard::firstboot::FIRSTBOOT_PENDING)
                .is_file(),
            "upgrade still queues pending-convergence"
        );
        authority.finish().unwrap();
        let mut resumed = LifecycleAuthority::resume(root.path(), "seat-15").unwrap();
        assert_eq!(
            std::fs::read(lifecycle.join(crate::onboard::firstboot::STAGED_OVERLAY_IP)).unwrap(),
            b"10.0.0.15"
        );
        assert!(
            lifecycle
                .join(crate::onboard::firstboot::FIRSTBOOT_PENDING)
                .is_file(),
            "reboot resume must keep pending-convergence"
        );
        let error = resumed
            .run_next_declared(Some(artifact.as_path()))
            .expect_err("resumed upgrade verify stays dest-gated");
        assert!(
            matches!(
                error,
                LifecycleAuthorityError::StepFailed(ref message)
                    if message.contains("pending-convergence")
            ),
            "dest apply is not implied after resume: {error:?}"
        );
        resumed.finish().unwrap();
    }

    #[test]
    fn upgrade_packages_progress_while_mesh_still_blocks() {
        let root = tempfile::tempdir().unwrap();
        let mut plan = onboard_packages_plan();
        plan.intent = LifecycleIntentKind::Upgrade;
        let mut authority = LifecycleAuthority::begin(root.path(), plan).unwrap();
        authority
            .record_check(LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "mesh".into(),
                target_id: "seat-15".into(),
                expected: "present".into(),
                observed: "missing: overlay-ip".into(),
                status: LifecycleCheckStatus::Fail,
                required: true,
                evidence_digest_hex: "4".repeat(64),
                warning: None,
                generation: 1,
            })
            .unwrap();
        let artifact = root.path().join("role.rpm");
        std::fs::write(&artifact, b"rpm-bytes").unwrap();
        let mut selection = signed_selection();
        selection.artifact_digest_hex = digest_hex(b"rpm-bytes");
        authority.select_artifact(selection).unwrap();
        authority
            .run_next_declared(Some(artifact.as_path()))
            .expect("upgrade packages must stage while dest mesh is still blocked");
        assert!(
            root.path()
                .join("lifecycle")
                .join(crate::onboard::firstboot::FIRSTBOOT_PENDING)
                .is_file(),
            "blocked upgrade still queues pending-convergence"
        );
        let error = authority
            .run_next_declared(Some(artifact.as_path()))
            .expect_err("final upgrade verify stays gated");
        assert!(
            matches!(
                error,
                LifecycleAuthorityError::InvalidTransition("blocking requirement check")
                    | LifecycleAuthorityError::StepFailed(_)
            ),
            "dest apply is not implied: {error:?}"
        );
        authority.finish().unwrap();
    }

    #[test]
    fn upgrade_packages_refuse_without_a_marker_dir() {
        let root = tempfile::tempdir().unwrap();
        let mut plan = onboard_packages_plan();
        plan.intent = LifecycleIntentKind::Upgrade;
        let mut authority = LifecycleAuthority::begin(root.path(), plan).unwrap();
        let artifact = root.path().join("role.rpm");
        std::fs::write(&artifact, b"rpm-bytes").unwrap();
        let mut selection = signed_selection();
        selection.artifact_digest_hex = digest_hex(b"rpm-bytes");
        authority.select_artifact(selection).unwrap();
        let error = execute_declared_step(
            "packages",
            authority.checkpoint(),
            &StepInputs {
                artifact_bytes_path: Some(artifact.as_path()),
                artifact_shape: None,
                marker_dir: None,
            },
        )
        .expect_err("upgrade must not complete without a marker path");
        assert!(
            error.contains("staged-artifact"),
            "upgrade fails at stage before dest install: {error}"
        );
        authority.finish().unwrap();
    }

    #[test]
    fn upgrade_configuration_queues_pending_without_inventing_dest() {
        let root = tempfile::tempdir().unwrap();
        let plan = LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::Upgrade,
            generation: 1,
            steps: vec!["configuration".into(), "verify".into()],
        };
        let mut authority = LifecycleAuthority::begin(root.path(), plan).unwrap();
        authority
            .record_check(required_check(
                "configuration",
                LifecycleCheckStatus::Fail,
                1,
            ))
            .unwrap();
        authority
            .run_next_declared(None)
            .expect("upgrade configuration must queue, not invent dest repair");
        assert!(
            pending_marker(root.path()).is_file(),
            "upgrade configuration must queue pending-convergence"
        );
        let error = authority
            .run_next_declared(None)
            .expect_err("final upgrade verify stays dest-gated");
        assert!(
            matches!(
                error,
                LifecycleAuthorityError::InvalidTransition("blocking requirement check")
                    | LifecycleAuthorityError::StepFailed(_)
            ),
            "dest apply is not implied: {error:?}"
        );
        authority.finish().unwrap();
    }

    #[test]
    fn upgrade_preflight_verify_assesses_while_blocked() {
        let root = tempfile::tempdir().unwrap();
        let plan = LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::Upgrade,
            generation: 1,
            steps: vec!["verify".into(), "packages".into(), "verify".into()],
        };
        let mut authority = LifecycleAuthority::begin(root.path(), plan).unwrap();
        authority
            .record_check(required_check("mesh", LifecycleCheckStatus::Fail, 1))
            .unwrap();
        authority
            .run_next_declared(None)
            .expect("upgrade preflight verify must assess, not abort the generation");
        assert_eq!(authority.checkpoint().progress.completed_steps, 1);
        assert!(
            pending_marker(root.path()).is_file(),
            "blocked upgrade preflight must queue pending-convergence"
        );
        authority.finish().unwrap();
    }

    fn verify_and_correct_plan() -> LifecyclePlanV1 {
        LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::VerifyAndCorrect,
            generation: 1,
            steps: vec![
                "verify".into(),
                "configuration".into(),
                "mesh".into(),
                "verify".into(),
            ],
        }
    }

    fn required_check(
        check_id: &str,
        status: LifecycleCheckStatus,
        generation: u64,
    ) -> LifecycleRequirementCheckV1 {
        LifecycleRequirementCheckV1 {
            schema_version: 1,
            check_id: check_id.into(),
            target_id: "seat-15".into(),
            expected: "ready".into(),
            observed: if status == LifecycleCheckStatus::Pass {
                "ready".into()
            } else {
                "blocked".into()
            },
            status,
            required: true,
            evidence_digest_hex: "3".repeat(64),
            warning: (status != LifecycleCheckStatus::Pass).then(|| "blocked".into()),
            generation,
        }
    }

    fn pending_marker(root: &std::path::Path) -> std::path::PathBuf {
        root.join("lifecycle")
            .join(crate::onboard::firstboot::FIRSTBOOT_PENDING)
    }

    #[test]
    fn verify_and_correct_assesses_then_queues_correction() {
        let root = tempfile::tempdir().unwrap();
        let mut authority =
            LifecycleAuthority::begin(root.path(), verify_and_correct_plan()).unwrap();
        authority
            .record_check(required_check(
                "configuration",
                LifecycleCheckStatus::Fail,
                1,
            ))
            .unwrap();
        authority
            .record_check(required_check("mesh", LifecycleCheckStatus::Fail, 1))
            .unwrap();
        authority
            .run_next_declared(None)
            .expect("first verify must assess, not abort the generation");
        assert!(
            pending_marker(root.path()).exists(),
            "blocked first verify must queue pending-convergence"
        );
        authority
            .run_next_declared(None)
            .expect("configuration correction must queue, not invent dest repair");
        authority
            .run_next_declared(None)
            .expect("mesh correction must queue, not invent dest repair");
        let error = authority
            .run_next_declared(None)
            .expect_err("final verify must not claim Succeeded while blocked");
        assert!(matches!(
            error,
            LifecycleAuthorityError::InvalidTransition("blocking requirement check")
                | LifecycleAuthorityError::StepFailed(_)
        ));
        authority.finish().unwrap();
    }

    #[test]
    fn verify_and_correct_final_verify_succeeds_when_checks_pass() {
        let root = tempfile::tempdir().unwrap();
        let mut authority =
            LifecycleAuthority::begin(root.path(), verify_and_correct_plan()).unwrap();
        authority
            .record_check(required_check(
                "configuration",
                LifecycleCheckStatus::Pass,
                1,
            ))
            .unwrap();
        authority
            .record_check(required_check("mesh", LifecycleCheckStatus::Pass, 1))
            .unwrap();
        authority.run_next_declared(None).unwrap();
        authority.run_next_declared(None).unwrap();
        authority.run_next_declared(None).unwrap();
        authority.run_next_declared(None).unwrap();
        assert_eq!(
            authority.checkpoint().progress.phase,
            mackes_mesh_types::lifecycle::LifecyclePhase::Succeeded
        );
        assert!(
            !pending_marker(root.path()).exists(),
            "healthy verify-and-correct must not invent a pending marker"
        );
        authority.finish().unwrap();
    }

    #[test]
    fn verify_and_correct_does_not_invent_an_unobserved_plane() {
        let root = tempfile::tempdir().unwrap();
        let mut authority =
            LifecycleAuthority::begin(root.path(), verify_and_correct_plan()).unwrap();
        authority.run_next_declared(None).unwrap();
        let error = authority
            .run_next_declared(None)
            .expect_err("unobserved configuration is not implied repaired");
        assert!(
            matches!(error, LifecycleAuthorityError::StepFailed(message) if message.contains("not observed"))
        );
        authority.finish().unwrap();
    }

    #[test]
    fn verify_and_correct_uses_canonical_baseline_check_ids() {
        let root = tempfile::tempdir().unwrap();
        let mut authority =
            LifecycleAuthority::begin(root.path(), verify_and_correct_plan()).unwrap();
        authority
            .record_check(required_check("units", LifecycleCheckStatus::Fail, 1))
            .unwrap();
        authority
            .record_check(required_check(
                "mesh_identity",
                LifecycleCheckStatus::Fail,
                1,
            ))
            .unwrap();
        authority.run_declared_until_blocked(None).unwrap();
        assert!(
            pending_marker(root.path()).exists(),
            "units/mesh_identity must drive configuration/mesh correction"
        );
        assert!(
            authority.checkpoint().progress.completed_steps >= 3,
            "walker must reach final verify instead of aborting on baseline ids"
        );
        authority.finish().unwrap();
    }

    fn onboard_join_plan() -> LifecyclePlanV1 {
        LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::Onboard,
            generation: 1,
            steps: vec!["mesh".into(), "verify".into()],
        }
    }

    #[test]
    fn onboard_mesh_stages_join_dests_and_refuses_ready_while_pending() {
        let root = tempfile::tempdir().unwrap();
        let lifecycle = root.path().join("lifecycle");
        std::fs::create_dir_all(&lifecycle).unwrap();
        std::fs::write(
            lifecycle.join(crate::onboard::firstboot::JOIN_OVERLAY_IP_PIN),
            "10.42.0.15\n",
        )
        .unwrap();
        std::fs::write(
            lifecycle.join(crate::onboard::firstboot::JOIN_ETCD_ENDPOINTS_PIN),
            "https://10.42.0.1:2379\n",
        )
        .unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), onboard_join_plan()).unwrap();
        authority
            .record_check(required_check("mesh", LifecycleCheckStatus::Fail, 1))
            .unwrap();
        authority
            .run_next_declared(None)
            .expect("onboard mesh must stage join dests instead of inventing dest write");
        assert_eq!(
            std::fs::read_to_string(lifecycle.join(crate::onboard::firstboot::STAGED_OVERLAY_IP))
                .unwrap()
                .trim(),
            "10.42.0.15"
        );
        assert_eq!(
            std::fs::read_to_string(
                lifecycle.join(crate::onboard::firstboot::STAGED_ETCD_ENDPOINTS)
            )
            .unwrap()
            .trim(),
            "https://10.42.0.1:2379"
        );
        let plane = std::fs::read_to_string(
            lifecycle.join(crate::onboard::firstboot::STAGED_GROUPED_PLANE),
        )
        .unwrap();
        assert!(
            plane.contains("mackesd-control.service"),
            "join must stage the grouped plane: {plane}"
        );
        assert!(
            pending_marker(root.path()).exists(),
            "staged join dests must queue pending-convergence"
        );
        let error = authority
            .run_next_declared(None)
            .expect_err("final onboard verify must not claim Ready before dest apply");
        assert!(
            matches!(
                error,
                LifecycleAuthorityError::InvalidTransition("blocking requirement check")
            ) || matches!(
                error,
                LifecycleAuthorityError::StepFailed(ref message)
                    if message.contains("pending-convergence")
            ),
            "verify must stay pending after join stage: {error:?}"
        );
        authority.finish().unwrap();
    }

    #[test]
    fn onboard_mesh_without_join_dest_pins_does_not_invent_dests() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), onboard_join_plan()).unwrap();
        authority
            .record_check(required_check("mesh", LifecycleCheckStatus::Fail, 1))
            .unwrap();
        let error = authority
            .run_next_declared(None)
            .expect_err("missing join pins must not invent dest overlay-ip");
        assert!(
            matches!(
                error,
                LifecycleAuthorityError::StepFailed(ref message) if message.contains("not ready")
            ),
            "mesh without pins stays blocked: {error:?}"
        );
        assert!(
            !root
                .path()
                .join("lifecycle")
                .join(crate::onboard::firstboot::STAGED_OVERLAY_IP)
                .exists(),
            "no overlay pin means no staged overlay-ip"
        );
        authority.finish().unwrap();
    }

    #[test]
    fn verify_and_correct_mesh_stages_join_dests_when_pins_exist() {
        let root = tempfile::tempdir().unwrap();
        let lifecycle = root.path().join("lifecycle");
        std::fs::create_dir_all(&lifecycle).unwrap();
        crate::onboard::firstboot::pin_mesh_join_dests(
            &lifecycle,
            "10.42.0.22",
            Some("https://10.42.0.1:2379\n"),
        )
        .unwrap();
        let mut authority =
            LifecycleAuthority::begin(root.path(), verify_and_correct_plan()).unwrap();
        authority
            .record_check(required_check(
                "configuration",
                LifecycleCheckStatus::Fail,
                1,
            ))
            .unwrap();
        authority
            .record_check(required_check("mesh", LifecycleCheckStatus::Fail, 1))
            .unwrap();
        authority.run_declared_until_blocked(None).unwrap();
        assert_eq!(
            std::fs::read_to_string(lifecycle.join(crate::onboard::firstboot::STAGED_OVERLAY_IP))
                .unwrap()
                .trim(),
            "10.42.0.22"
        );
        assert!(
            pending_marker(root.path()).exists(),
            "VAC join stage must still queue pending-convergence"
        );
        authority.finish().unwrap();
    }
}
