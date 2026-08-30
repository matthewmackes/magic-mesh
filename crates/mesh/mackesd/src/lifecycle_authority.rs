//! WL-FUNC-023 — the local-first, resumable lifecycle authority.
//!
//! This module owns only lifecycle admission and checkpoints. Callers must
//! perform each typed step and persist progress through this authority; a
//! renderer or CLI cannot bypass the per-target lock.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use ed25519_dalek::VerifyingKey;
use fs2::FileExt;
use mackes_mesh_types::lifecycle::{
    canonical_lifecycle_baseline, CommissioningCapsuleV1, FleetLifecycleReportV1,
    LifecycleArtifactSelectionV1, LifecycleConfirmationAction, LifecycleConfirmationV1,
    LifecycleCorrectionPlanV1, LifecycleCorrectionV1, LifecycleIntentKind, LifecyclePhase,
    LifecyclePlanV1, LifecycleProgressV1, LifecycleRequirementCheckV1, LifecycleStepKind,
    OffboardingReceiptV1, SeatReadinessV1,
};
use serde::{Deserialize, Serialize};

const CHECKPOINT: &str = "checkpoint.json";
const JOURNAL: &str = "journal.jsonl";
const RECEIPT: &str = "receipt.json";
const LOCK: &str = "lifecycle.lock";
/// VerifyAndCorrect may retry a failed correction this many times before
/// the generation is terminal. Destructive intents fail closed on the first
/// error so a partial wipe cannot be silently replayed.
const MAX_VAC_STEP_RETRIES: u8 = 3;
/// Audit and peek stay concurrent. Mutation walks this many seats at a
/// time so a later wave cannot shrink the already-confirmed scope.
pub const FLEET_MUTATION_WAVE: usize = 2;
/// The enroll-command placeholder. Bootstrap must mint a real bearer instead
/// of substituting this into argv or the checkpoint.
const JOIN_TOKEN_TEMPLATE: &str = "{{JOIN_TOKEN}}";

fn enrollment_bearer_digest_hex(bearer: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bearer.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleCheckpointV1 {
    pub plan: LifecyclePlanV1,
    pub progress: LifecycleProgressV1,
    #[serde(default)]
    pub checks: Vec<LifecycleRequirementCheckV1>,
    #[serde(default)]
    pub confirmation: Option<LifecycleConfirmationV1>,
    #[serde(default)]
    pub consumed_capsule_ids: Vec<String>,
    #[serde(default)]
    pub pending_capsule_ids: Vec<String>,
    #[serde(default)]
    pub revoked_capsule_ids: Vec<String>,
    #[serde(default)]
    pub artifact_selection: Option<LifecycleArtifactSelectionV1>,
    #[serde(default)]
    pub retry_count: u8,
    #[serde(default)]
    pub last_error: Option<String>,
    /// SHA-256 digests of lighthouse enrollment bearers minted for this
    /// target. The raw bearer is returned once to the caller and is never
    /// stored here — only the digest is durable so a later confirm/revoke
    /// can bind the handoff without turning the checkpoint into a secret store.
    #[serde(default)]
    pub pending_enrollment_bearer_digests: Vec<String>,
    /// Durable fleet coordinator. Empty on first claim; later handoffs
    /// must match this id so a disconnected initiator cannot invent one.
    #[serde(default)]
    pub coordinator_id: Option<String>,
    /// Immutable VerifyAndCorrect DAG for this generation. A later admit
    /// cannot substitute a different correction order.
    #[serde(default)]
    pub correction_plan: Option<LifecycleCorrectionPlanV1>,
}

impl LifecycleCheckpointV1 {
    /// Derive readiness solely from authority-owned checks. Shared by the
    /// locked authority and the read-only peek path so a renderer cannot
    /// invent a different ready/blocked answer.
    pub fn readiness(&self) -> Result<SeatReadinessV1, LifecycleAuthorityError> {
        SeatReadinessV1::from_requirement_checks(
            self.plan.schema_version,
            self.plan.target_id.clone(),
            self.plan.generation,
            &self.checks,
        )
        .map_err(|_| LifecycleAuthorityError::InvalidPlan("readiness"))
    }

    /// Build the immutable VerifyAndCorrect DAG from currently blocking
    /// checks. Order follows declared plan steps, then remaining check ids.
    /// Prerequisite edges come from the canonical baseline; a renderer cannot
    /// invent a different repair order.
    pub fn propose_correction_plan(
        &self,
    ) -> Result<LifecycleCorrectionPlanV1, LifecycleAuthorityError> {
        let baseline = canonical_lifecycle_baseline();
        let mut blocking: Vec<&LifecycleRequirementCheckV1> = self
            .checks
            .iter()
            .filter(|check| check.blocks_progress())
            .collect();
        if blocking.is_empty() {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "no blocking checks",
            ));
        }
        blocking.sort_by_key(|check| {
            let step_rank = self
                .plan
                .steps
                .iter()
                .position(|step| {
                    step == &check.check_id
                        || baseline.iter().any(|entry| {
                            entry.requirement_id == check.check_id
                                && entry.correction_step.as_str() == step
                        })
                })
                .unwrap_or(usize::MAX);
            (step_rank, check.check_id.clone())
        });
        let corrections: Vec<LifecycleCorrectionV1> = blocking
            .iter()
            .map(|check| {
                let entry = baseline
                    .iter()
                    .find(|entry| entry.requirement_id == check.check_id);
                let step = entry
                    .map(|entry| entry.correction_step.as_str().to_string())
                    .or_else(|| {
                        LifecycleStepKind::parse(&check.check_id)
                            .map(|kind| kind.as_str().to_string())
                    })
                    .unwrap_or_else(|| check.check_id.clone());
                let prerequisites = entry
                    .map(|entry| {
                        entry
                            .prerequisites
                            .iter()
                            .filter(|id| {
                                blocking.iter().any(|candidate| candidate.check_id == **id)
                            })
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                LifecycleCorrectionV1 {
                    check_id: check.check_id.clone(),
                    step,
                    reason: if check.observed.is_empty() {
                        "blocked".into()
                    } else {
                        check.observed.clone()
                    },
                    prerequisites,
                }
            })
            .collect();
        let edges = corrections
            .iter()
            .flat_map(|correction| {
                correction
                    .prerequisites
                    .iter()
                    .map(|from| (from.clone(), correction.check_id.clone()))
            })
            .collect();
        let plan = LifecycleCorrectionPlanV1 {
            schema_version: self.plan.schema_version,
            request_id: self.plan.request_id.clone(),
            target_id: self.plan.target_id.clone(),
            generation: self.plan.generation,
            corrections,
            edges,
            rollback_forbidden: true,
        };
        plan.validate()
            .map_err(|_| LifecycleAuthorityError::InvalidPlan("correction plan"))?;
        Ok(plan)
    }
}

#[derive(Debug)]
pub enum LifecycleAuthorityError {
    Io(io::Error),
    Json(serde_json::Error),
    InvalidPlan(&'static str),
    RequestMismatch,
    TargetMismatch,
    InvalidTransition(&'static str),
    StepFailed(String),
}

impl From<io::Error> for LifecycleAuthorityError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for LifecycleAuthorityError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// VerifyAndCorrect may walk correction steps while required checks still
/// block. The final verify remains the gate so a blocked seat cannot claim
/// Succeeded.
fn declared_step_allows_blocked_progress(checkpoint: &LifecycleCheckpointV1) -> bool {
    let index = checkpoint.progress.completed_steps as usize;
    let Some(step) = checkpoint.plan.steps.get(index) else {
        return false;
    };
    let final_verify = step == "verify" && index + 1 == checkpoint.plan.steps.len();
    match checkpoint.plan.intent {
        LifecycleIntentKind::VerifyAndCorrect => !final_verify,
        LifecycleIntentKind::Onboard | LifecycleIntentKind::ResetAndOnboard => {
            matches!(step.as_str(), "mesh" | "configuration") && !final_verify
        }
        LifecycleIntentKind::Upgrade => {
            matches!(
                step.as_str(),
                "packages" | "mesh" | "configuration" | "verify"
            ) && !final_verify
        }
        _ => false,
    }
}

/// One held lock and one atomic checkpoint stream for one target.
pub struct LifecycleAuthority {
    dir: PathBuf,
    lock: File,
    checkpoint: LifecycleCheckpointV1,
}

/// Aggregate target checkpoints without allowing cross-request or
/// cross-generation state to masquerade as one fleet operation.
pub fn fleet_report(
    request_id: &str,
    generation: u64,
    checkpoints: &[LifecycleCheckpointV1],
) -> Result<FleetLifecycleReportV1, LifecycleAuthorityError> {
    if checkpoints.is_empty() || generation == 0 || request_id.is_empty() {
        return Err(LifecycleAuthorityError::InvalidPlan("fleet inputs"));
    }
    let mut succeeded = 0u32;
    let mut failed = 0u32;
    let mut blocked = false;
    for checkpoint in checkpoints {
        if checkpoint.plan.request_id != request_id || checkpoint.plan.generation != generation {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "mixed fleet generation",
            ));
        }
        match checkpoint.progress.phase {
            LifecyclePhase::Succeeded => succeeded += 1,
            LifecyclePhase::Failed | LifecyclePhase::Cancelled => failed += 1,
            LifecyclePhase::Planned
            | LifecyclePhase::Running
            | LifecyclePhase::WaitingForOperator => {}
        }
        blocked |= checkpoint
            .checks
            .iter()
            .any(LifecycleRequirementCheckV1::blocks_progress);
    }
    let target_count = checkpoints.len() as u32;
    let phase = if failed > 0 {
        LifecyclePhase::Failed
    } else if succeeded == target_count && !blocked {
        LifecyclePhase::Succeeded
    } else if blocked {
        LifecyclePhase::WaitingForOperator
    } else {
        LifecyclePhase::Running
    };
    let coordinator_id = durable_coordinator(checkpoints)?.unwrap_or("").to_owned();
    let report = FleetLifecycleReportV1 {
        schema_version: checkpoints[0].plan.schema_version,
        request_id: request_id.to_owned(),
        generation,
        phase,
        target_count,
        succeeded,
        failed,
        coordinator_id,
        signature_hex: String::new(),
    };
    report
        .validate()
        .map_err(|_| LifecycleAuthorityError::InvalidPlan("fleet report"))?;
    Ok(report)
}

/// Admit a coordinator handoff only when the durable fleet checkpoints
/// already exist. The initiator identity is not the job; wiping or
/// disconnecting it cannot invent a new generation.
fn coordinator_id_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= mackes_mesh_types::lifecycle::MAX_LIFECYCLE_IDENTIFIER_BYTES
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

fn durable_coordinator(
    checkpoints: &[LifecycleCheckpointV1],
) -> Result<Option<&str>, LifecycleAuthorityError> {
    let mut seen = None;
    for checkpoint in checkpoints {
        if let Some(id) = checkpoint
            .coordinator_id
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            match seen {
                None => seen = Some(id),
                Some(existing) if existing != id => {
                    return Err(LifecycleAuthorityError::InvalidTransition(
                        "mixed coordinator",
                    ));
                }
                Some(_) => {}
            }
        }
    }
    Ok(seen)
}

pub fn transfer_fleet_coordination(
    request_id: &str,
    generation: u64,
    from_coordinator: &str,
    to_coordinator: &str,
    checkpoints: &[LifecycleCheckpointV1],
) -> Result<FleetLifecycleReportV1, LifecycleAuthorityError> {
    if !coordinator_id_ok(from_coordinator) || !coordinator_id_ok(to_coordinator) {
        return Err(LifecycleAuthorityError::InvalidPlan("coordinator"));
    }
    if from_coordinator == to_coordinator {
        return Err(LifecycleAuthorityError::InvalidTransition(
            "coordinator unchanged",
        ));
    }
    if let Some(held) = durable_coordinator(checkpoints)? {
        if held != from_coordinator {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "coordinator mismatch",
            ));
        }
    }
    fleet_report(request_id, generation, checkpoints)
}

/// Reconstruct a fleet session from durable per-seat checkpoints.
/// A coordinator reboot, wipe, or disconnect cannot lose the job: this
/// path never takes the mutation lock, skips missing seats, and never
/// invents a replacement checkpoint.
pub fn peek_fleet_session(
    root: &Path,
    target_ids: &[impl AsRef<str>],
) -> Result<(FleetLifecycleReportV1, Vec<LifecycleCheckpointV1>), LifecycleAuthorityError> {
    if target_ids.is_empty() {
        return Err(LifecycleAuthorityError::InvalidPlan("fleet inputs"));
    }
    let mut checkpoints = Vec::with_capacity(target_ids.len());
    for target_id in target_ids {
        let target_id = target_id.as_ref();
        if target_id.is_empty() {
            return Err(LifecycleAuthorityError::TargetMismatch);
        }
        match LifecycleAuthority::peek(root, target_id) {
            Ok(checkpoint) => checkpoints.push(checkpoint),
            Err(LifecycleAuthorityError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    if checkpoints.is_empty() {
        return Err(LifecycleAuthorityError::InvalidPlan("fleet inputs"));
    }
    let _ = durable_coordinator(&checkpoints)?;
    let report = fleet_report(
        &checkpoints[0].plan.request_id,
        checkpoints[0].plan.generation,
        &checkpoints,
    )?;
    Ok((report, checkpoints))
}

/// Target ids that share one durable fleet generation. Peek-only.
pub fn peek_matching_fleet_targets(
    root: &Path,
    request_id: &str,
    generation: u64,
) -> Result<Vec<String>, LifecycleAuthorityError> {
    if request_id.is_empty() || generation == 0 {
        return Err(LifecycleAuthorityError::InvalidPlan("fleet inputs"));
    }
    let dir = root.join("lifecycle");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut targets = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(target_id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(checkpoint) = LifecycleAuthority::peek(root, &target_id) else {
            continue;
        };
        if checkpoint.plan.request_id == request_id && checkpoint.plan.generation == generation {
            targets.push(target_id);
        }
    }
    targets.sort();
    Ok(targets)
}

/// Resume durable fleet seats. A wiped or never-published target is
/// skipped so coordinator wipe cannot lose the remaining job.
pub fn resume_fleet(
    root: &Path,
    target_ids: &[impl AsRef<str>],
) -> Result<Vec<LifecycleAuthority>, LifecycleAuthorityError> {
    let mut authorities = Vec::with_capacity(target_ids.len());
    for target_id in target_ids {
        let target_id = target_id.as_ref();
        if target_id.is_empty() {
            return Err(LifecycleAuthorityError::TargetMismatch);
        }
        match LifecycleAuthority::resume(root, target_id) {
            Ok(authority) => authorities.push(authority),
            Err(LifecycleAuthorityError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    if authorities.is_empty() {
        return Err(LifecycleAuthorityError::InvalidPlan("fleet inputs"));
    }
    Ok(authorities)
}

/// Persist `to_coordinator` on every durable checkpoint. A later disconnect
/// cannot invent a different generation or initiator.
pub fn execute_fleet_handoff(
    authorities: &mut [LifecycleAuthority],
    from_coordinator: &str,
    to_coordinator: &str,
) -> Result<FleetLifecycleReportV1, LifecycleAuthorityError> {
    if authorities.is_empty() {
        return Err(LifecycleAuthorityError::InvalidPlan("fleet inputs"));
    }
    let request_id = authorities[0].checkpoint.plan.request_id.clone();
    let generation = authorities[0].checkpoint.plan.generation;
    let checkpoints = authorities
        .iter()
        .map(|authority| authority.checkpoint.clone())
        .collect::<Vec<_>>();
    transfer_fleet_coordination(
        &request_id,
        generation,
        from_coordinator,
        to_coordinator,
        &checkpoints,
    )?;
    mutate_fleet_in_waves(authorities, |authority| {
        authority.set_coordinator(to_coordinator)
    })?;
    let checkpoints = authorities
        .iter()
        .map(|authority| authority.checkpoint().clone())
        .collect::<Vec<_>>();
    fleet_report(&request_id, generation, &checkpoints)
}

/// Digest of the sorted unique fleet target set. A confirmation whose
/// `scope_digest_hex` does not match this cannot cover a different seat list.
#[must_use]
pub fn fleet_scope_digest(target_ids: &[impl AsRef<str>]) -> String {
    LifecycleConfirmationV1::fleet_scope_digest(target_ids)
}

fn mutate_fleet_in_waves(
    authorities: &mut [LifecycleAuthority],
    mut mutate: impl FnMut(&mut LifecycleAuthority) -> Result<(), LifecycleAuthorityError>,
) -> Result<(), LifecycleAuthorityError> {
    for wave in authorities.chunks_mut(FLEET_MUTATION_WAVE) {
        for authority in wave.iter_mut() {
            mutate(authority)?;
        }
    }
    Ok(())
}

fn erasure_observed(checkpoint: &LifecycleCheckpointV1) -> bool {
    checkpoint.checks.iter().any(|check| {
        check.check_id == "erasure"
            && check.generation == checkpoint.plan.generation
            && check.status == mackes_mesh_types::lifecycle::LifecycleCheckStatus::Pass
            && !check.blocks_progress()
    })
}

const STAGED_CAPSULE_DIR: &str = "capsule";

fn staged_capsule_path(dir: &Path, capsule_id: &str) -> PathBuf {
    dir.join(STAGED_CAPSULE_DIR).join(capsule_id)
}

fn refuse_capsule_symlink(path: &Path) -> Result<(), LifecycleAuthorityError> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "capsule must not be a symlink",
            ));
        }
    }
    Ok(())
}

fn persist_staged_capsule(
    dir: &Path,
    capsule: &CommissioningCapsuleV1,
) -> Result<(), LifecycleAuthorityError> {
    let path = staged_capsule_path(dir, &capsule.capsule_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    refuse_capsule_symlink(&path)?;
    std::fs::write(&path, serde_json::to_vec(capsule)?)?;
    Ok(())
}

fn erase_staged_capsule(dir: &Path, capsule_id: &str) -> Result<(), LifecycleAuthorityError> {
    let path = staged_capsule_path(dir, capsule_id);
    refuse_capsule_symlink(&path)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn refuse_receipt_symlink(path: &Path) -> Result<(), LifecycleAuthorityError> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "receipt must not be a symlink",
            ));
        }
    }
    Ok(())
}

/// One signed phrase covers every target. Each authority then runs the
/// declared offboard/verify steps; dest wipe is not implied.
pub fn execute_fleet_offboard(
    authorities: &mut [LifecycleAuthority],
    confirmation: LifecycleConfirmationV1,
    verifying_key: &VerifyingKey,
) -> Result<FleetLifecycleReportV1, LifecycleAuthorityError> {
    execute_fleet_confirmed(
        authorities,
        confirmation,
        verifying_key,
        LifecycleIntentKind::Offboard,
        true,
    )
}

/// One signed `WIPE <N> SYSTEMS` phrase covers every target. Only the
/// declared offboard step runs; reset identity is not invented.
pub fn execute_fleet_reset(
    authorities: &mut [LifecycleAuthority],
    confirmation: LifecycleConfirmationV1,
    verifying_key: &VerifyingKey,
) -> Result<FleetLifecycleReportV1, LifecycleAuthorityError> {
    execute_fleet_confirmed(
        authorities,
        confirmation,
        verifying_key,
        LifecycleIntentKind::ResetAndOnboard,
        false,
    )
}

fn execute_fleet_confirmed(
    authorities: &mut [LifecycleAuthority],
    confirmation: LifecycleConfirmationV1,
    verifying_key: &VerifyingKey,
    intent: LifecycleIntentKind,
    walk_until_blocked: bool,
) -> Result<FleetLifecycleReportV1, LifecycleAuthorityError> {
    if authorities.is_empty() {
        return Err(LifecycleAuthorityError::InvalidPlan("fleet inputs"));
    }
    let request_id = authorities[0].checkpoint.plan.request_id.clone();
    let generation = authorities[0].checkpoint.plan.generation;
    let target_ids = authorities
        .iter()
        .map(|authority| authority.checkpoint.plan.target_id.clone())
        .collect::<Vec<_>>();
    for authority in authorities.iter() {
        if authority.checkpoint.plan.request_id != request_id
            || authority.checkpoint.plan.generation != generation
            || authority.checkpoint.plan.intent != intent
        {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "mixed fleet generation",
            ));
        }
        if authority.checkpoint.plan.steps.first().map(String::as_str) != Some("offboard") {
            return Err(LifecycleAuthorityError::InvalidTransition("step order"));
        }
    }
    mutate_fleet_in_waves(authorities, |authority| {
        authority.accept_shared_confirmation(confirmation.clone(), verifying_key, &target_ids)?;
        if walk_until_blocked {
            authority.run_declared_until_blocked(None)
        } else {
            authority.run_next_declared(None)
        }
    })?;
    if matches!(
        intent,
        LifecycleIntentKind::Offboard | LifecycleIntentKind::ResetAndOnboard
    ) {
        persist_completed_offboarding_receipts(authorities)?;
    }
    let checkpoints = authorities
        .iter()
        .map(|authority| authority.checkpoint.clone())
        .collect::<Vec<_>>();
    fleet_report(&request_id, generation, &checkpoints)
}

/// Persist a receipt only when erasure was already observed. Plan success
/// does not invent dest wipe.
fn persist_completed_offboarding_receipts(
    authorities: &[LifecycleAuthority],
) -> Result<(), LifecycleAuthorityError> {
    for authority in authorities {
        if erasure_observed(&authority.checkpoint) {
            authority.persist_offboarding_receipt()?;
        }
    }
    Ok(())
}

/// One signed `INSTALL UNSIGNED <N> SYSTEMS` phrase covers every target.
/// The confirmation digest is the artifact bytes, not the seat list.
pub fn execute_fleet_unsigned_select(
    authorities: &mut [LifecycleAuthority],
    selections: &[LifecycleArtifactSelectionV1],
    confirmation: LifecycleConfirmationV1,
    verifying_key: &VerifyingKey,
) -> Result<FleetLifecycleReportV1, LifecycleAuthorityError> {
    if authorities.is_empty() || selections.len() != authorities.len() {
        return Err(LifecycleAuthorityError::InvalidPlan("fleet inputs"));
    }
    let request_id = authorities[0].checkpoint.plan.request_id.clone();
    let generation = authorities[0].checkpoint.plan.generation;
    let digest = selections[0].artifact_digest_hex.clone();
    let target_ids = authorities
        .iter()
        .map(|authority| authority.checkpoint.plan.target_id.clone())
        .collect::<Vec<_>>();
    for (authority, selection) in authorities.iter().zip(selections.iter()) {
        if authority.checkpoint.plan.request_id != request_id
            || authority.checkpoint.plan.generation != generation
            || selection.target_id != authority.checkpoint.plan.target_id
            || selection.generation != generation
            || selection.artifact_digest_hex != digest
        {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "mixed fleet generation",
            ));
        }
    }
    mutate_fleet_in_waves(authorities, |authority| {
        let selection = selections
            .iter()
            .find(|selection| selection.target_id == authority.checkpoint.plan.target_id)
            .ok_or(LifecycleAuthorityError::InvalidPlan("fleet inputs"))?;
        authority.select_unsigned_artifact_shared(
            selection.clone(),
            confirmation.clone(),
            verifying_key,
            &target_ids,
        )
    })?;
    let checkpoints = authorities
        .iter()
        .map(|authority| authority.checkpoint.clone())
        .collect::<Vec<_>>();
    fleet_report(&request_id, generation, &checkpoints)
}

/// Walk an already-admitted fleet upgrade. Every seat must already hold the
/// same artifact selection; dest RPM/bootc install is not implied.
pub fn execute_fleet_upgrade(
    authorities: &mut [LifecycleAuthority],
    artifact_bytes_path: Option<&Path>,
) -> Result<FleetLifecycleReportV1, LifecycleAuthorityError> {
    if authorities.is_empty() {
        return Err(LifecycleAuthorityError::InvalidPlan("fleet inputs"));
    }
    let request_id = authorities[0].checkpoint.plan.request_id.clone();
    let generation = authorities[0].checkpoint.plan.generation;
    let digest = authorities[0]
        .checkpoint
        .artifact_selection
        .as_ref()
        .ok_or(LifecycleAuthorityError::InvalidTransition(
            "artifact selection missing",
        ))?
        .artifact_digest_hex
        .clone();
    for authority in authorities.iter() {
        if authority.checkpoint.plan.request_id != request_id
            || authority.checkpoint.plan.generation != generation
            || authority.checkpoint.plan.intent != LifecycleIntentKind::Upgrade
        {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "mixed fleet generation",
            ));
        }
        match authority.checkpoint.artifact_selection.as_ref() {
            Some(selection) if selection.artifact_digest_hex == digest => {}
            Some(_) => {
                return Err(LifecycleAuthorityError::InvalidTransition(
                    "mixed fleet generation",
                ));
            }
            None => {
                return Err(LifecycleAuthorityError::InvalidTransition(
                    "artifact selection missing",
                ));
            }
        }
    }
    mutate_fleet_in_waves(authorities, |authority| {
        authority.run_declared_until_blocked(artifact_bytes_path)
    })?;
    let checkpoints = authorities
        .iter()
        .map(|authority| authority.checkpoint.clone())
        .collect::<Vec<_>>();
    fleet_report(&request_id, generation, &checkpoints)
}

impl LifecycleAuthority {
    pub fn begin(root: &Path, plan: LifecyclePlanV1) -> Result<Self, LifecycleAuthorityError> {
        plan.validate()
            .map_err(|_| LifecycleAuthorityError::InvalidPlan("plan"))?;
        let dir = root.join("lifecycle").join(&plan.target_id);
        std::fs::create_dir_all(&dir)?;
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .open(dir.join(LOCK))?;
        lock.try_lock_exclusive()?;
        let progress = LifecycleProgressV1 {
            schema_version: plan.schema_version,
            request_id: plan.request_id.clone(),
            target_id: plan.target_id.clone(),
            generation: plan.generation,
            phase: LifecyclePhase::Planned,
            completed_steps: 0,
            total_steps: plan.steps.len() as u32,
        };
        let authority = Self {
            dir,
            lock,
            checkpoint: LifecycleCheckpointV1 {
                plan,
                progress,
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
            },
        };
        authority.persist()?;
        Ok(authority)
    }

    /// Resume an interrupted session. The lock is acquired before reading the
    /// checkpoint so two recovery workers cannot act on the same target.
    pub fn resume(root: &Path, target_id: &str) -> Result<Self, LifecycleAuthorityError> {
        let dir = root.join("lifecycle").join(target_id);
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .open(dir.join(LOCK))?;
        lock.try_lock_exclusive()?;
        let result = (|| {
            let checkpoint: LifecycleCheckpointV1 =
                serde_json::from_slice(&std::fs::read(dir.join(CHECKPOINT))?)?;
            checkpoint
                .plan
                .validate()
                .map_err(|_| LifecycleAuthorityError::InvalidPlan("plan"))?;
            checkpoint
                .progress
                .validate()
                .map_err(|_| LifecycleAuthorityError::InvalidPlan("progress"))?;
            if checkpoint.plan.target_id != target_id || checkpoint.progress.target_id != target_id
            {
                return Err(LifecycleAuthorityError::TargetMismatch);
            }
            Ok(Self {
                dir: dir.clone(),
                lock,
                checkpoint,
            })
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(dir.join(LOCK));
        }
        result
    }

    pub fn checkpoint(&self) -> &LifecycleCheckpointV1 {
        &self.checkpoint
    }

    /// Read a target checkpoint without taking the exclusive mutation lock.
    /// Renderers use this so a Status/Lifecycle screen cannot stall or steal
    /// an in-flight authority session.
    pub fn peek(
        root: &Path,
        target_id: &str,
    ) -> Result<LifecycleCheckpointV1, LifecycleAuthorityError> {
        if target_id.is_empty() {
            return Err(LifecycleAuthorityError::TargetMismatch);
        }
        let checkpoint: LifecycleCheckpointV1 = serde_json::from_slice(&std::fs::read(
            root.join("lifecycle").join(target_id).join(CHECKPOINT),
        )?)?;
        checkpoint
            .plan
            .validate()
            .map_err(|_| LifecycleAuthorityError::InvalidPlan("plan"))?;
        checkpoint
            .progress
            .validate()
            .map_err(|_| LifecycleAuthorityError::InvalidPlan("progress"))?;
        if checkpoint.plan.target_id != target_id || checkpoint.progress.target_id != target_id {
            return Err(LifecycleAuthorityError::TargetMismatch);
        }
        Ok(checkpoint)
    }

    /// Peek-only receipt. Missing is `None`. A planted symlink is refused
    /// so dest wipe cannot be implied by following it.
    pub fn peek_offboarding_receipt(
        root: &Path,
        target_id: &str,
    ) -> Result<Option<OffboardingReceiptV1>, LifecycleAuthorityError> {
        if target_id.is_empty() {
            return Err(LifecycleAuthorityError::TargetMismatch);
        }
        let path = root.join("lifecycle").join(target_id).join(RECEIPT);
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(LifecycleAuthorityError::InvalidTransition(
                    "receipt must not be a symlink",
                ));
            }
            Ok(_) => {}
        }
        let receipt: OffboardingReceiptV1 = serde_json::from_slice(&std::fs::read(&path)?)?;
        receipt
            .validate()
            .map_err(|_| LifecycleAuthorityError::InvalidPlan("offboarding receipt"))?;
        if receipt.target_id != target_id {
            return Err(LifecycleAuthorityError::TargetMismatch);
        }
        Ok(Some(receipt))
    }

    /// Newest-generation authority checkpoint under `root/lifecycle/*`.
    /// Missing root or an empty tree is `Ok(None)`, never a fabricated ready
    /// session.
    pub fn peek_latest(
        root: &Path,
    ) -> Result<Option<LifecycleCheckpointV1>, LifecycleAuthorityError> {
        let dir = root.join("lifecycle");
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut best: Option<LifecycleCheckpointV1> = None;
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Some(target_id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(checkpoint) = Self::peek(root, &target_id) else {
                continue;
            };
            let take = match &best {
                None => true,
                Some(prev) => {
                    checkpoint.progress.generation > prev.progress.generation
                        || (checkpoint.progress.generation == prev.progress.generation
                            && checkpoint.plan.request_id > prev.plan.request_id)
                }
            };
            if take {
                best = Some(checkpoint);
            }
        }
        Ok(best)
    }

    /// Derive readiness solely from authority-owned checks.  A live target or
    /// completed step is not sufficient while a required check is failed or
    /// unknown; warnings remain visible but do not falsely withdraw usable
    /// capability.
    pub fn readiness(&self) -> Result<SeatReadinessV1, LifecycleAuthorityError> {
        self.checkpoint.readiness()
    }

    pub fn record_check(
        &mut self,
        check: LifecycleRequirementCheckV1,
    ) -> Result<(), LifecycleAuthorityError> {
        if check.target_id != self.checkpoint.plan.target_id {
            return Err(LifecycleAuthorityError::TargetMismatch);
        }
        if check.generation != self.checkpoint.plan.generation {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "check generation",
            ));
        }
        check
            .validate()
            .map_err(|_| LifecycleAuthorityError::InvalidPlan("requirement check"))?;
        if self
            .checkpoint
            .checks
            .iter()
            .any(|existing| existing.check_id == check.check_id)
        {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "duplicate check",
            ));
        }
        self.checkpoint.checks.push(check);
        self.persist()
    }

    /// Replace the recorded baseline checks without touching capsules, artifacts,
    /// or confirmation. First-boot re-audit uses this so a later boot cannot
    /// duplicate check ids and cannot erase pending enrollment tokens.
    pub fn replace_checks(
        &mut self,
        checks: Vec<LifecycleRequirementCheckV1>,
    ) -> Result<(), LifecycleAuthorityError> {
        let mut seen = std::collections::HashSet::new();
        for check in &checks {
            if check.target_id != self.checkpoint.plan.target_id {
                return Err(LifecycleAuthorityError::TargetMismatch);
            }
            if check.generation != self.checkpoint.plan.generation {
                return Err(LifecycleAuthorityError::InvalidTransition(
                    "check generation",
                ));
            }
            check
                .validate()
                .map_err(|_| LifecycleAuthorityError::InvalidPlan("requirement check"))?;
            if !seen.insert(check.check_id.as_str()) {
                return Err(LifecycleAuthorityError::InvalidTransition(
                    "duplicate check",
                ));
            }
        }
        self.checkpoint.checks = checks;
        self.persist()
    }

    /// Stage one target-bound bootstrap capsule. Repeating the same valid
    /// pending capsule is retry-safe; only confirmation moves it to the
    /// consumed set, which is the durable erase boundary for bootstrap bytes.
    pub fn admit_commissioning_capsule(
        &mut self,
        capsule: CommissioningCapsuleV1,
        now_ms: i64,
        verifying_key: &VerifyingKey,
    ) -> Result<String, LifecycleAuthorityError> {
        if capsule.target_id != self.checkpoint.plan.target_id {
            return Err(LifecycleAuthorityError::TargetMismatch);
        }
        if self
            .checkpoint
            .revoked_capsule_ids
            .iter()
            .any(|id| id == &capsule.capsule_id)
        {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "capsule revoked",
            ));
        }
        if self
            .checkpoint
            .consumed_capsule_ids
            .iter()
            .any(|id| id == &capsule.capsule_id)
        {
            return Err(LifecycleAuthorityError::InvalidTransition("capsule replay"));
        }
        capsule
            .verify_at(now_ms, verifying_key)
            .map_err(|_| LifecycleAuthorityError::InvalidPlan("commissioning capsule"))?;
        persist_staged_capsule(&self.dir, &capsule)?;
        if self
            .checkpoint
            .pending_capsule_ids
            .iter()
            .any(|id| id == &capsule.capsule_id)
        {
            return Ok(capsule.bootstrap_digest_hex);
        }
        self.checkpoint.pending_capsule_ids.push(capsule.capsule_id);
        self.persist()?;
        Ok(capsule.bootstrap_digest_hex)
    }

    /// Confirm enrollment and make the staged capsule single-use durable.
    pub fn confirm_commissioning_capsule(
        &mut self,
        capsule_id: &str,
    ) -> Result<(), LifecycleAuthorityError> {
        if !self
            .checkpoint
            .pending_capsule_ids
            .iter()
            .any(|id| id == capsule_id)
        {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "capsule not pending",
            ));
        }
        erase_staged_capsule(&self.dir, capsule_id)?;
        self.checkpoint
            .pending_capsule_ids
            .retain(|id| id != capsule_id);
        self.checkpoint
            .consumed_capsule_ids
            .push(capsule_id.to_owned());
        self.persist()
    }

    /// Revoke a staged capsule before enrollment confirmation.
    pub fn revoke_commissioning_capsule(
        &mut self,
        capsule_id: &str,
    ) -> Result<(), LifecycleAuthorityError> {
        if self
            .checkpoint
            .consumed_capsule_ids
            .iter()
            .any(|id| id == capsule_id)
        {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "capsule already consumed",
            ));
        }
        if !self
            .checkpoint
            .pending_capsule_ids
            .iter()
            .any(|id| id == capsule_id)
        {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "capsule not pending",
            ));
        }
        erase_staged_capsule(&self.dir, capsule_id)?;
        self.checkpoint
            .pending_capsule_ids
            .retain(|id| id != capsule_id);
        self.checkpoint
            .revoked_capsule_ids
            .push(capsule_id.to_owned());
        self.persist()
    }

    /// Mint a lighthouse-scoped enrollment bearer through the existing
    /// issued-bearer ledger. The raw secret is returned once and never stored
    /// in the checkpoint; only its SHA-256 digest is recorded as pending so a
    /// later confirm/revoke can bind the handoff to this target and generation.
    ///
    /// The command-template placeholder `{{JOIN_TOKEN}}` is refused — bootstrap
    /// must receive a real minted bearer, never the rendered enroll command.
    pub fn mint_lighthouse_enrollment_bearer(
        &mut self,
        workgroup_root: &Path,
        provided: Option<&str>,
    ) -> Result<String, LifecycleAuthorityError> {
        if self.checkpoint.plan.intent != LifecycleIntentKind::Onboard {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "enrollment bearer mint is onboard-only",
            ));
        }
        if matches!(
            self.checkpoint.progress.phase,
            LifecyclePhase::Failed | LifecyclePhase::Cancelled
        ) {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "enrollment bearer mint after terminal failure",
            ));
        }
        let bearer = match provided {
            Some(JOIN_TOKEN_TEMPLATE) | Some("") => {
                return Err(LifecycleAuthorityError::InvalidPlan(
                    "enrollment bearer template",
                ));
            }
            Some(provided)
                if provided.trim() != provided || provided.contains(char::is_whitespace) =>
            {
                return Err(LifecycleAuthorityError::InvalidPlan(
                    "enrollment bearer template",
                ));
            }
            Some(provided) => provided.to_owned(),
            None => crate::bearer_ledger::issue(
                workgroup_root,
                crate::bearer_ledger::LIGHTHOUSE_ROLE_NOTE,
            )?,
        };
        if bearer == JOIN_TOKEN_TEMPLATE || bearer.is_empty() {
            return Err(LifecycleAuthorityError::InvalidPlan(
                "enrollment bearer template",
            ));
        }
        let digest = enrollment_bearer_digest_hex(&bearer);
        if self
            .checkpoint
            .pending_enrollment_bearer_digests
            .iter()
            .any(|existing| existing == &digest)
        {
            return Ok(bearer);
        }
        self.checkpoint
            .pending_enrollment_bearer_digests
            .push(digest.clone());
        self.append_journal("enrollment_bearer_minted", "mesh", &digest)?;
        self.persist()?;
        Ok(bearer)
    }

    /// Erase the minted bearer only after enrollment succeeded. Failed
    /// enroll keeps the pending digest so retry can redeem the same token.
    pub fn confirm_lighthouse_enrollment_bearer(
        &mut self,
        workgroup_root: &Path,
        bearer: &str,
    ) -> Result<(), LifecycleAuthorityError> {
        if bearer == JOIN_TOKEN_TEMPLATE || bearer.is_empty() {
            return Err(LifecycleAuthorityError::InvalidPlan(
                "enrollment bearer template",
            ));
        }
        let digest = enrollment_bearer_digest_hex(bearer);
        if !self
            .checkpoint
            .pending_enrollment_bearer_digests
            .iter()
            .any(|existing| existing == &digest)
        {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "enrollment bearer not pending",
            ));
        }
        if !crate::bearer_ledger::consume(workgroup_root, bearer) {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "enrollment bearer not consumed",
            ));
        }
        self.checkpoint
            .pending_enrollment_bearer_digests
            .retain(|existing| existing != &digest);
        self.append_journal("enrollment_bearer_confirmed", "mesh", &digest)?;
        self.persist()
    }

    /// Persist the exact artifact admission used by this lifecycle request.
    /// Replacing a selection requires a new generation; the current authority
    /// never silently changes bytes after planning.
    pub fn select_artifact(
        &mut self,
        selection: LifecycleArtifactSelectionV1,
    ) -> Result<(), LifecycleAuthorityError> {
        if selection.target_id != self.checkpoint.plan.target_id {
            return Err(LifecycleAuthorityError::TargetMismatch);
        }
        if selection.generation != self.checkpoint.plan.generation {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "artifact generation",
            ));
        }
        selection
            .validate()
            .map_err(|_| LifecycleAuthorityError::InvalidPlan("artifact selection"))?;
        if selection.unverified_build {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "unsigned confirmation required",
            ));
        }
        if self.checkpoint.artifact_selection.is_some() {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "artifact already selected",
            ));
        }
        self.checkpoint.artifact_selection = Some(selection);
        self.persist()
    }

    /// Admit an unsigned artifact only with a signed, digest-bound operator
    /// confirmation. The confirmation phrase carries the short digest so a
    /// valid authorization cannot be redirected to different bytes.
    pub fn select_unsigned_artifact(
        &mut self,
        selection: LifecycleArtifactSelectionV1,
        confirmation: LifecycleConfirmationV1,
        verifying_key: &VerifyingKey,
    ) -> Result<(), LifecycleAuthorityError> {
        if !selection.unverified_build || selection.signed {
            return Err(LifecycleAuthorityError::InvalidPlan("unsigned artifact"));
        }
        if confirmation.action != LifecycleConfirmationAction::InstallUnsigned
            || confirmation.session_id != self.checkpoint.plan.request_id
            || confirmation.target_count != 1
            || confirmation.scope_digest_hex != selection.artifact_digest_hex
        {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "unsigned confirmation scope",
            ));
        }
        self.select_artifact_common(selection, confirmation, verifying_key)
    }

    /// Admit the same unsigned artifact on one seat of a fleet. The phrase
    /// count must match the seat list; the digest still pins the bytes.
    pub fn select_unsigned_artifact_shared(
        &mut self,
        selection: LifecycleArtifactSelectionV1,
        confirmation: LifecycleConfirmationV1,
        verifying_key: &VerifyingKey,
        fleet_target_ids: &[String],
    ) -> Result<(), LifecycleAuthorityError> {
        let mut ids = fleet_target_ids.to_vec();
        ids.sort();
        ids.dedup();
        if !ids
            .iter()
            .any(|target| target == &self.checkpoint.plan.target_id)
        {
            return Err(LifecycleAuthorityError::TargetMismatch);
        }
        if !selection.unverified_build || selection.signed {
            return Err(LifecycleAuthorityError::InvalidPlan("unsigned artifact"));
        }
        if confirmation.action != LifecycleConfirmationAction::InstallUnsigned
            || confirmation.session_id != self.checkpoint.plan.request_id
            || confirmation.generation != self.checkpoint.plan.generation
            || confirmation.target_count as usize != ids.len()
            || confirmation.scope_digest_hex != selection.artifact_digest_hex
        {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "unsigned confirmation scope",
            ));
        }
        self.select_artifact_common(selection, confirmation, verifying_key)
    }

    fn select_artifact_common(
        &mut self,
        selection: LifecycleArtifactSelectionV1,
        confirmation: LifecycleConfirmationV1,
        verifying_key: &VerifyingKey,
    ) -> Result<(), LifecycleAuthorityError> {
        if selection.target_id != self.checkpoint.plan.target_id
            || selection.generation != self.checkpoint.plan.generation
        {
            return Err(LifecycleAuthorityError::TargetMismatch);
        }
        selection
            .validate()
            .map_err(|_| LifecycleAuthorityError::InvalidPlan("artifact selection"))?;
        confirmation
            .verify(verifying_key)
            .map_err(|_| LifecycleAuthorityError::InvalidPlan("unsigned confirmation"))?;
        if self.checkpoint.artifact_selection.is_some() {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "artifact already selected",
            ));
        }
        self.checkpoint.artifact_selection = Some(selection);
        self.checkpoint.confirmation = Some(confirmation);
        self.persist()
    }

    /// Admit a corrected-forward plan only for checks that currently block
    /// progress. The typed contract performs step/rollback validation; this
    /// authority additionally binds every correction to this checkpoint.
    pub fn admit_correction_plan(
        &mut self,
        correction_plan: LifecycleCorrectionPlanV1,
    ) -> Result<(), LifecycleAuthorityError> {
        if correction_plan.request_id != self.checkpoint.plan.request_id {
            return Err(LifecycleAuthorityError::RequestMismatch);
        }
        if correction_plan.target_id != self.checkpoint.plan.target_id {
            return Err(LifecycleAuthorityError::TargetMismatch);
        }
        if correction_plan.generation != self.checkpoint.plan.generation {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "correction generation",
            ));
        }
        correction_plan
            .validate()
            .map_err(|_| LifecycleAuthorityError::InvalidPlan("correction plan"))?;
        for correction in &correction_plan.corrections {
            let check = self
                .checkpoint
                .checks
                .iter()
                .find(|check| check.check_id == correction.check_id)
                .ok_or(LifecycleAuthorityError::InvalidTransition(
                    "correction check missing",
                ))?;
            if !check.blocks_progress() {
                return Err(LifecycleAuthorityError::InvalidTransition(
                    "correction check not blocking",
                ));
            }
        }
        for correction in &correction_plan.corrections {
            for prerequisite in &correction.prerequisites {
                if !correction_plan
                    .corrections
                    .iter()
                    .any(|candidate| candidate.check_id == *prerequisite)
                {
                    return Err(LifecycleAuthorityError::InvalidTransition(
                        "correction prerequisite missing",
                    ));
                }
            }
        }
        if let Some(held) = &self.checkpoint.correction_plan {
            if held != &correction_plan {
                return Err(LifecycleAuthorityError::InvalidTransition(
                    "correction plan immutable",
                ));
            }
            return Ok(());
        }
        self.checkpoint.correction_plan = Some(correction_plan);
        self.persist()
    }

    /// Build the immutable VerifyAndCorrect DAG from currently blocking
    /// checks. Delegates to the checkpoint so a peeking renderer gets the
    /// same order without taking the mutation lock.
    pub fn propose_correction_plan(
        &self,
    ) -> Result<LifecycleCorrectionPlanV1, LifecycleAuthorityError> {
        self.checkpoint.propose_correction_plan()
    }

    pub fn accept_confirmation(
        &mut self,
        confirmation: LifecycleConfirmationV1,
        verifying_key: &VerifyingKey,
    ) -> Result<(), LifecycleAuthorityError> {
        let expected_action = match self.checkpoint.plan.intent {
            LifecycleIntentKind::Offboard => LifecycleConfirmationAction::Offboard,
            LifecycleIntentKind::ResetAndOnboard => LifecycleConfirmationAction::Reset,
            _ => {
                return Err(LifecycleAuthorityError::InvalidTransition(
                    "confirmation not required",
                ));
            }
        };
        if confirmation.session_id != self.checkpoint.plan.request_id
            || confirmation.action != expected_action
            || confirmation.target_count != 1
            || confirmation.generation != self.checkpoint.plan.generation
        {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "confirmation scope",
            ));
        }
        if self.checkpoint.confirmation.is_some() {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "confirmation replay",
            ));
        }
        confirmation
            .verify(verifying_key)
            .map_err(|_| LifecycleAuthorityError::InvalidPlan("confirmation"))?;
        self.checkpoint.confirmation = Some(confirmation);
        self.persist()
    }

    /// Accept one fleet-scoped confirmation. The target must be in the signed
    /// seat list; a different count or digest cannot move this seat.
    pub fn accept_shared_confirmation(
        &mut self,
        confirmation: LifecycleConfirmationV1,
        verifying_key: &VerifyingKey,
        fleet_target_ids: &[String],
    ) -> Result<(), LifecycleAuthorityError> {
        let mut ids = fleet_target_ids.to_vec();
        ids.sort();
        ids.dedup();
        if !ids
            .iter()
            .any(|target| target == &self.checkpoint.plan.target_id)
        {
            return Err(LifecycleAuthorityError::TargetMismatch);
        }
        if confirmation.target_count as usize != ids.len()
            || confirmation.scope_digest_hex != fleet_scope_digest(&ids)
        {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "confirmation scope",
            ));
        }
        let expected_action = match self.checkpoint.plan.intent {
            LifecycleIntentKind::Offboard => LifecycleConfirmationAction::Offboard,
            LifecycleIntentKind::ResetAndOnboard => LifecycleConfirmationAction::Reset,
            _ => {
                return Err(LifecycleAuthorityError::InvalidTransition(
                    "confirmation not required",
                ));
            }
        };
        if confirmation.session_id != self.checkpoint.plan.request_id
            || confirmation.action != expected_action
            || confirmation.generation != self.checkpoint.plan.generation
        {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "confirmation scope",
            ));
        }
        if self.checkpoint.confirmation.is_some() {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "confirmation replay",
            ));
        }
        confirmation
            .verify(verifying_key)
            .map_err(|_| LifecycleAuthorityError::InvalidPlan("confirmation"))?;
        self.checkpoint.confirmation = Some(confirmation);
        self.persist()
    }

    pub fn update(&mut self, progress: LifecycleProgressV1) -> Result<(), LifecycleAuthorityError> {
        if progress.request_id != self.checkpoint.plan.request_id {
            return Err(LifecycleAuthorityError::RequestMismatch);
        }
        if progress.target_id != self.checkpoint.plan.target_id {
            return Err(LifecycleAuthorityError::TargetMismatch);
        }
        progress
            .validate()
            .map_err(|_| LifecycleAuthorityError::InvalidPlan("progress"))?;
        let current = self.checkpoint.progress.phase;
        let next = progress.phase;
        let allowed = match current {
            LifecyclePhase::Planned => matches!(
                next,
                LifecyclePhase::Planned
                    | LifecyclePhase::Running
                    | LifecyclePhase::WaitingForOperator
                    | LifecyclePhase::Failed
                    | LifecyclePhase::Cancelled
            ),
            LifecyclePhase::Running => matches!(
                next,
                LifecyclePhase::Running
                    | LifecyclePhase::WaitingForOperator
                    | LifecyclePhase::Succeeded
                    | LifecyclePhase::Failed
                    | LifecyclePhase::Cancelled
            ),
            LifecyclePhase::WaitingForOperator => matches!(
                next,
                LifecyclePhase::WaitingForOperator
                    | LifecyclePhase::Running
                    | LifecyclePhase::Failed
                    | LifecyclePhase::Cancelled
            ),
            LifecyclePhase::Succeeded | LifecyclePhase::Failed | LifecyclePhase::Cancelled => {
                next == current
            }
        };
        if !allowed {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "phase transition",
            ));
        }
        self.checkpoint.progress = progress;
        self.persist()
    }

    /// Commit exactly the next declared step after its real side effects have
    /// succeeded. Replays and jumps are refused, making retries explicit.
    pub fn complete_step(&mut self, step_index: u32) -> Result<(), LifecycleAuthorityError> {
        let progress = &self.checkpoint.progress;
        if !matches!(
            progress.phase,
            LifecyclePhase::Planned | LifecyclePhase::Running
        ) {
            return Err(LifecycleAuthorityError::InvalidTransition("terminal phase"));
        }
        if step_index != progress.completed_steps || step_index >= progress.total_steps {
            return Err(LifecycleAuthorityError::InvalidTransition("step order"));
        }
        let completed_steps = step_index + 1;
        let phase = if completed_steps == progress.total_steps
            && !self
                .checkpoint
                .checks
                .iter()
                .any(LifecycleRequirementCheckV1::blocks_progress)
        {
            LifecyclePhase::Succeeded
        } else if completed_steps == progress.total_steps {
            LifecyclePhase::WaitingForOperator
        } else {
            LifecyclePhase::Running
        };
        self.update(LifecycleProgressV1 {
            phase,
            completed_steps,
            ..progress.clone()
        })
    }

    /// Execute and commit exactly the next declared step. The checkpoint is
    /// advanced only after `action` succeeds; a failed action is recorded as a
    /// terminal failure so a caller must explicitly start a corrected-forward
    /// generation instead of silently replaying a partial mutation.
    pub fn run_next<F>(&mut self, action: F) -> Result<(), LifecycleAuthorityError>
    where
        F: FnOnce(&str) -> Result<(), String>,
    {
        if self
            .checkpoint
            .checks
            .iter()
            .any(LifecycleRequirementCheckV1::blocks_progress)
            && !declared_step_allows_blocked_progress(&self.checkpoint)
        {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "blocking requirement check",
            ));
        }
        let index = self.checkpoint.progress.completed_steps;
        let step = self
            .checkpoint
            .plan
            .steps
            .get(index as usize)
            .ok_or(LifecycleAuthorityError::InvalidTransition("step order"))?
            .clone();
        if step == "packages" && self.checkpoint.artifact_selection.is_none() {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "artifact selection missing",
            ));
        }
        self.run_next_with_retry(index, step, action)
    }

    /// Run the next declared step through the shared executor. Specialized
    /// CLI verbs keep `run_next`; this is the path that cannot invent RPM,
    /// dest, or offboard effects.
    pub fn run_next_declared(
        &mut self,
        artifact_bytes_path: Option<&Path>,
    ) -> Result<(), LifecycleAuthorityError> {
        let checkpoint = self.checkpoint.clone();
        // Same directory first-boot and meshctl use for pending-convergence
        // (`<root>/lifecycle/`), not the per-target lock dir.
        let marker_dir = self
            .dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.dir.clone());
        self.run_next(|step| {
            crate::lifecycle_step::execute_declared_step(
                step,
                &checkpoint,
                &crate::lifecycle_step::StepInputs {
                    artifact_bytes_path,
                    artifact_shape: None,
                    marker_dir: Some(marker_dir.as_path()),
                },
            )
        })
    }

    /// Walk remaining declared steps until the plan completes or a blocking
    /// check gates the final verify. First-boot uses this so a renderer cannot
    /// invent a different VerifyAndCorrect order.
    pub fn run_declared_until_blocked(
        &mut self,
        artifact_bytes_path: Option<&Path>,
    ) -> Result<(), LifecycleAuthorityError> {
        if matches!(
            self.checkpoint.plan.intent,
            LifecycleIntentKind::VerifyAndCorrect
                | LifecycleIntentKind::Upgrade
                | LifecycleIntentKind::Onboard
                | LifecycleIntentKind::ResetAndOnboard
                | LifecycleIntentKind::Offboard
        ) && self.checkpoint.correction_plan.is_none()
            && self
                .checkpoint
                .checks
                .iter()
                .any(|check| check.blocks_progress())
        {
            let proposed = self.propose_correction_plan()?;
            self.admit_correction_plan(proposed)?;
        }
        loop {
            if self.checkpoint.progress.completed_steps >= self.checkpoint.progress.total_steps {
                return Ok(());
            }
            match self.run_next_declared(artifact_bytes_path) {
                Ok(()) => {}
                Err(LifecycleAuthorityError::InvalidTransition("blocking requirement check"))
                | Err(LifecycleAuthorityError::InvalidTransition("step order")) => return Ok(()),
                Err(LifecycleAuthorityError::StepFailed(ref message))
                    if message.contains("pending-convergence") =>
                {
                    return Ok(());
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Record a failed mutation durably and transition to terminal failure.
    /// A caller must start an explicit corrected-forward generation before a
    /// new provider action may run; this avoids silently replaying a partially
    /// completed destructive mutation.
    pub fn run_next_with_retry<F>(
        &mut self,
        index: u32,
        step: String,
        action: F,
    ) -> Result<(), LifecycleAuthorityError>
    where
        F: FnOnce(&str) -> Result<(), String>,
    {
        match action(&step) {
            Ok(()) => {
                self.checkpoint.retry_count = 0;
                self.checkpoint.last_error = None;
                self.persist()?;
                self.complete_step(index)
            }
            Err(error) => {
                self.checkpoint.retry_count = self.checkpoint.retry_count.saturating_add(1);
                self.checkpoint.last_error = Some(error.clone());
                self.append_journal("step_failed", &step, &error)?;
                self.persist()?;
                if self.checkpoint.plan.intent == LifecycleIntentKind::VerifyAndCorrect
                    && self.checkpoint.retry_count < MAX_VAC_STEP_RETRIES
                {
                    return Err(LifecycleAuthorityError::StepFailed(error));
                }
                let mut failed = self.checkpoint.progress.clone();
                failed.phase = LifecyclePhase::Failed;
                self.update(failed)?;
                Err(LifecycleAuthorityError::StepFailed(error))
            }
        }
    }

    fn set_coordinator(&mut self, coordinator_id: &str) -> Result<(), LifecycleAuthorityError> {
        if !coordinator_id_ok(coordinator_id) {
            return Err(LifecycleAuthorityError::InvalidPlan("coordinator"));
        }
        self.checkpoint.coordinator_id = Some(coordinator_id.to_owned());
        self.append_journal("coordinator", "", coordinator_id)?;
        self.persist()
    }

    /// Project the signed-boundary input for an offboarding receipt only after
    /// the wipe half is observed. Offboard waits for Succeeded; ResetAndOnboard
    /// may persist after the offboard step so later identity cannot hide erase.
    /// Signing is performed by the caller's governed evidence boundary.
    pub fn offboarding_receipt(&self) -> Result<OffboardingReceiptV1, LifecycleAuthorityError> {
        let wipe_complete = match self.checkpoint.plan.intent {
            LifecycleIntentKind::Offboard => {
                self.checkpoint.progress.phase == LifecyclePhase::Succeeded
            }
            LifecycleIntentKind::ResetAndOnboard => self.checkpoint.progress.completed_steps >= 1,
            _ => {
                return Err(LifecycleAuthorityError::InvalidTransition("not offboard"));
            }
        };
        if !wipe_complete {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "offboard incomplete",
            ));
        }
        if self.checkpoint.confirmation.is_none() {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "offboard confirmation missing",
            ));
        }
        if !erasure_observed(&self.checkpoint) {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "erasure not observed",
            ));
        }
        let receipt = OffboardingReceiptV1 {
            schema_version: self.checkpoint.plan.schema_version,
            request_id: self.checkpoint.plan.request_id.clone(),
            target_id: self.checkpoint.plan.target_id.clone(),
            generation: self.checkpoint.plan.generation,
            completed: true,
            retained_resources: Vec::new(),
            signature_hex: String::new(),
        };
        receipt
            .validate()
            .map_err(|_| LifecycleAuthorityError::InvalidPlan("offboarding receipt"))?;
        Ok(receipt)
    }

    /// Write the projected receipt next to the checkpoint. Projection stays
    /// side-effect free; dest wipe is still not implied.
    pub fn persist_offboarding_receipt(
        &self,
    ) -> Result<OffboardingReceiptV1, LifecycleAuthorityError> {
        let receipt = self.offboarding_receipt()?;
        let path = self.dir.join(RECEIPT);
        refuse_receipt_symlink(&path)?;
        let tmp = self.dir.join(".receipt.json.tmp");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp)?;
        use std::io::Write;
        file.write_all(&serde_json::to_vec_pretty(&receipt)?)?;
        file.sync_all()?;
        std::fs::rename(tmp, path)?;
        self.append_journal("receipt", "offboard", "completed")?;
        Ok(receipt)
    }

    fn persist(&self) -> Result<(), LifecycleAuthorityError> {
        let tmp = self.dir.join(".checkpoint.json.tmp");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp)?;
        use std::io::Write;
        file.write_all(&serde_json::to_vec_pretty(&self.checkpoint)?)?;
        file.sync_all()?;
        std::fs::rename(tmp, self.dir.join(CHECKPOINT))?;
        Ok(())
    }

    fn append_journal(
        &self,
        event: &str,
        step: &str,
        detail: &str,
    ) -> Result<(), LifecycleAuthorityError> {
        use std::io::Write;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(JOURNAL))?;
        serde_json::to_writer(
            &mut file,
            &serde_json::json!({"event": event, "step": step, "detail": detail}),
        )?;
        file.write_all(b"\n")?;
        file.sync_data()?;
        Ok(())
    }

    pub fn finish(self) -> Result<(), LifecycleAuthorityError> {
        self.lock.unlock()?;
        drop(self.lock);
        std::fs::remove_file(self.dir.join(LOCK))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::lifecycle::{LifecycleIntentKind, LifecyclePhase};

    fn plan() -> LifecyclePlanV1 {
        LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::Onboard,
            generation: 1,
            steps: vec!["identity".into(), "verify".into()],
        }
    }

    #[test]
    fn authority_is_exclusive_and_checkpoints_atomically() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        assert!(LifecycleAuthority::begin(root.path(), plan()).is_err());
        authority
            .update(LifecycleProgressV1 {
                schema_version: 1,
                request_id: "request-1".into(),
                target_id: "seat-15".into(),
                generation: 1,
                phase: LifecyclePhase::Running,
                completed_steps: 1,
                total_steps: 2,
            })
            .unwrap();
        let saved: LifecycleCheckpointV1 = serde_json::from_slice(
            &std::fs::read(root.path().join("lifecycle/seat-15/checkpoint.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(saved.progress.completed_steps, 1);
        authority.finish().unwrap();
        assert!(!root
            .path()
            .join("lifecycle/seat-15/lifecycle.lock")
            .exists());
    }

    #[test]
    fn readiness_is_false_for_required_failures_but_warns_remain_usable() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        authority
            .record_check(LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "kvm".into(),
                target_id: "seat-15".into(),
                expected: "available".into(),
                observed: "absent".into(),
                status: mackes_mesh_types::lifecycle::LifecycleCheckStatus::Warn,
                required: false,
                evidence_digest_hex: "a".repeat(64),
                warning: Some("optional".into()),
                generation: 1,
            })
            .unwrap();
        let readiness = authority.readiness().unwrap();
        assert!(readiness.ready);
        assert_eq!(readiness.warnings, vec!["optional"]);
        authority
            .record_check(LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "mesh".into(),
                target_id: "seat-15".into(),
                expected: "joined".into(),
                observed: "absent".into(),
                status: mackes_mesh_types::lifecycle::LifecycleCheckStatus::Fail,
                required: true,
                evidence_digest_hex: "b".repeat(64),
                warning: None,
                generation: 1,
            })
            .unwrap();
        let readiness = authority.readiness().unwrap();
        assert!(!readiness.ready);
        assert_eq!(readiness.missing_requirements, vec!["mesh"]);
        authority.finish().unwrap();
    }

    #[test]
    fn commissioning_capsule_is_retryable_until_confirmed_then_single_use() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[8; 32]);
        let capsule = CommissioningCapsuleV1 {
            schema_version: 1,
            capsule_id: "capsule-1".into(),
            target_id: "seat-15".into(),
            expires_at_ms: 2_000,
            bootstrap_digest_hex: "c".repeat(64),
            one_time: true,
            key_id: "commissioning-v1".into(),
            signature_hex: String::new(),
        }
        .sign("commissioning-v1", &signing_key);
        assert_eq!(
            authority
                .admit_commissioning_capsule(capsule.clone(), 1_000, &signing_key.verifying_key())
                .unwrap(),
            "c".repeat(64)
        );
        assert_eq!(
            authority
                .admit_commissioning_capsule(capsule, 1_000, &signing_key.verifying_key())
                .unwrap(),
            "c".repeat(64)
        );
        authority
            .confirm_commissioning_capsule("capsule-1")
            .unwrap();
        let replay = CommissioningCapsuleV1 {
            schema_version: 1,
            capsule_id: "capsule-1".into(),
            target_id: "seat-15".into(),
            expires_at_ms: 2_000,
            bootstrap_digest_hex: "c".repeat(64),
            one_time: true,
            key_id: "commissioning-v1".into(),
            signature_hex: String::new(),
        }
        .sign("commissioning-v1", &signing_key);
        assert!(matches!(
            authority.admit_commissioning_capsule(replay, 1_000, &signing_key.verifying_key()),
            Err(LifecycleAuthorityError::InvalidTransition("capsule replay"))
        ));
        authority.finish().unwrap();
    }

    #[test]
    fn commissioning_capsule_bytes_persist_until_confirm() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[8; 32]);
        let capsule = CommissioningCapsuleV1 {
            schema_version: 1,
            capsule_id: "capsule-bytes".into(),
            target_id: "seat-15".into(),
            expires_at_ms: 2_000,
            bootstrap_digest_hex: "c".repeat(64),
            one_time: true,
            key_id: "commissioning-v1".into(),
            signature_hex: String::new(),
        }
        .sign("commissioning-v1", &signing_key);
        authority
            .admit_commissioning_capsule(capsule.clone(), 1_000, &signing_key.verifying_key())
            .unwrap();
        let staged = staged_capsule_path(&authority.dir, "capsule-bytes");
        let stored: CommissioningCapsuleV1 =
            serde_json::from_slice(&std::fs::read(&staged).unwrap()).unwrap();
        assert_eq!(stored.capsule_id, "capsule-bytes");
        assert_eq!(stored.bootstrap_digest_hex, "c".repeat(64));
        authority
            .admit_commissioning_capsule(capsule, 1_000, &signing_key.verifying_key())
            .unwrap();
        assert!(staged.is_file());
        authority
            .confirm_commissioning_capsule("capsule-bytes")
            .unwrap();
        assert!(!staged.exists());
        authority.finish().unwrap();
    }

    #[test]
    fn commissioning_capsule_revoke_erases_bytes() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        let capsule = CommissioningCapsuleV1 {
            schema_version: 1,
            capsule_id: "capsule-erase".into(),
            target_id: "seat-15".into(),
            expires_at_ms: 2_000,
            bootstrap_digest_hex: "d".repeat(64),
            one_time: true,
            key_id: "commissioning-v1".into(),
            signature_hex: String::new(),
        }
        .sign("commissioning-v1", &signing_key);
        authority
            .admit_commissioning_capsule(capsule, 1_000, &signing_key.verifying_key())
            .unwrap();
        let staged = staged_capsule_path(&authority.dir, "capsule-erase");
        assert!(staged.is_file());
        authority
            .revoke_commissioning_capsule("capsule-erase")
            .unwrap();
        assert!(!staged.exists());
        authority.finish().unwrap();
    }

    #[test]
    fn commissioning_capsule_refuses_symlink() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        let escape = tempfile::NamedTempFile::new().unwrap();
        let staged = staged_capsule_path(&authority.dir, "capsule-link");
        std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(escape.path(), &staged).unwrap();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[10; 32]);
        let capsule = CommissioningCapsuleV1 {
            schema_version: 1,
            capsule_id: "capsule-link".into(),
            target_id: "seat-15".into(),
            expires_at_ms: 2_000,
            bootstrap_digest_hex: "e".repeat(64),
            one_time: true,
            key_id: "commissioning-v1".into(),
            signature_hex: String::new(),
        }
        .sign("commissioning-v1", &signing_key);
        assert!(matches!(
            authority.admit_commissioning_capsule(capsule, 1_000, &signing_key.verifying_key()),
            Err(LifecycleAuthorityError::InvalidTransition(
                "capsule must not be a symlink"
            ))
        ));
        authority.finish().unwrap();
    }

    #[test]
    fn commissioning_capsule_confirm_tolerates_missing_bytes() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[11; 32]);
        let capsule = CommissioningCapsuleV1 {
            schema_version: 1,
            capsule_id: "capsule-legacy".into(),
            target_id: "seat-15".into(),
            expires_at_ms: 2_000,
            bootstrap_digest_hex: "f".repeat(64),
            one_time: true,
            key_id: "commissioning-v1".into(),
            signature_hex: String::new(),
        }
        .sign("commissioning-v1", &signing_key);
        authority
            .admit_commissioning_capsule(capsule, 1_000, &signing_key.verifying_key())
            .unwrap();
        std::fs::remove_file(staged_capsule_path(&authority.dir, "capsule-legacy")).unwrap();
        authority
            .confirm_commissioning_capsule("capsule-legacy")
            .unwrap();
        authority.finish().unwrap();
    }

    #[test]
    fn commissioning_capsule_revocation_blocks_retry() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        let capsule = CommissioningCapsuleV1 {
            schema_version: 1,
            capsule_id: "capsule-revoked".into(),
            target_id: "seat-15".into(),
            expires_at_ms: 2_000,
            bootstrap_digest_hex: "d".repeat(64),
            one_time: true,
            key_id: "commissioning-v1".into(),
            signature_hex: String::new(),
        }
        .sign("commissioning-v1", &signing_key);
        authority
            .admit_commissioning_capsule(capsule.clone(), 1_000, &signing_key.verifying_key())
            .unwrap();
        authority
            .revoke_commissioning_capsule("capsule-revoked")
            .unwrap();
        assert!(matches!(
            authority.admit_commissioning_capsule(capsule, 1_000, &signing_key.verifying_key()),
            Err(LifecycleAuthorityError::InvalidTransition(
                "capsule revoked"
            ))
        ));
        authority.finish().unwrap();
    }

    #[test]
    fn lighthouse_enrollment_mint_records_digest_and_refuses_command_template() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        assert!(matches!(
            authority.mint_lighthouse_enrollment_bearer(root.path(), Some(JOIN_TOKEN_TEMPLATE)),
            Err(LifecycleAuthorityError::InvalidPlan(
                "enrollment bearer template"
            ))
        ));
        assert!(matches!(
            authority.mint_lighthouse_enrollment_bearer(root.path(), Some("{{JOIN_TOKEN}} extra")),
            Err(LifecycleAuthorityError::InvalidPlan(
                "enrollment bearer template"
            ))
        ));
        assert!(authority
            .checkpoint()
            .pending_enrollment_bearer_digests
            .is_empty());

        let bearer = authority
            .mint_lighthouse_enrollment_bearer(root.path(), None)
            .expect("authority mints through the existing ledger");
        assert_ne!(bearer, JOIN_TOKEN_TEMPLATE);
        assert_eq!(bearer.len(), 43);
        assert!(crate::bearer_ledger::is_pending(root.path(), &bearer));
        assert!(crate::bearer_ledger::is_lighthouse_bearer(
            root.path(),
            &bearer
        ));

        let digest = enrollment_bearer_digest_hex(&bearer);
        assert_eq!(
            authority.checkpoint().pending_enrollment_bearer_digests,
            vec![digest.clone()]
        );
        let checkpoint =
            std::fs::read_to_string(root.path().join("lifecycle/seat-15/checkpoint.json")).unwrap();
        assert!(
            !checkpoint.contains(&bearer),
            "raw enrollment bearer must not persist in the lifecycle checkpoint"
        );
        assert!(checkpoint.contains(&digest));

        let replay = authority
            .mint_lighthouse_enrollment_bearer(root.path(), Some(&bearer))
            .expect("retry of the same minted bearer is idempotent");
        assert_eq!(replay, bearer);
        assert_eq!(
            authority
                .checkpoint()
                .pending_enrollment_bearer_digests
                .len(),
            1,
            "retry must not spray a second pending digest"
        );
        authority.finish().unwrap();
    }

    #[test]
    fn lighthouse_enrollment_confirm_erases_the_bearer_only_after_success() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        let bearer = authority
            .mint_lighthouse_enrollment_bearer(root.path(), None)
            .unwrap();
        assert!(crate::bearer_ledger::is_pending(root.path(), &bearer));
        authority
            .confirm_lighthouse_enrollment_bearer(root.path(), &bearer)
            .unwrap();
        assert!(
            !crate::bearer_ledger::is_pending(root.path(), &bearer),
            "confirmed enrollment must erase the ledger token"
        );
        assert!(authority
            .checkpoint()
            .pending_enrollment_bearer_digests
            .is_empty());
        assert!(matches!(
            authority.confirm_lighthouse_enrollment_bearer(root.path(), &bearer),
            Err(LifecycleAuthorityError::InvalidTransition(
                "enrollment bearer not pending"
            ))
        ));
        authority.finish().unwrap();
    }

    #[test]
    fn lighthouse_enrollment_mint_refuses_failed_generation() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        let mut failed = authority.checkpoint().progress.clone();
        failed.phase = LifecyclePhase::Failed;
        authority.update(failed).unwrap();
        assert!(matches!(
            authority.mint_lighthouse_enrollment_bearer(root.path(), None),
            Err(LifecycleAuthorityError::InvalidTransition(
                "enrollment bearer mint after terminal failure"
            ))
        ));
        authority.finish().unwrap();
    }

    #[test]
    fn artifact_selection_is_target_bound_and_immutable() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        let selection = LifecycleArtifactSelectionV1 {
            schema_version: 1,
            selection_id: "selection-1".into(),
            target_id: "seat-15".into(),
            channel: mackes_mesh_types::lifecycle::LifecycleArtifactChannel::Stable,
            artifact_digest_hex: "d".repeat(64),
            source_revision: "a".repeat(40),
            signed: true,
            unverified_build: false,
            generation: 1,
        };
        authority.select_artifact(selection.clone()).unwrap();
        assert!(matches!(
            authority.select_artifact(selection),
            Err(LifecycleAuthorityError::InvalidTransition(
                "artifact already selected"
            ))
        ));
        authority.finish().unwrap();
    }

    #[test]
    fn unsigned_artifact_requires_digest_bound_confirmation() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[10; 32]);
        let selection = LifecycleArtifactSelectionV1 {
            schema_version: 1,
            selection_id: "selection-u".into(),
            target_id: "seat-15".into(),
            channel: mackes_mesh_types::lifecycle::LifecycleArtifactChannel::Dev,
            artifact_digest_hex: "e".repeat(64),
            source_revision: "b".repeat(40),
            signed: false,
            unverified_build: true,
            generation: 1,
        };
        assert!(matches!(
            authority.select_artifact(selection.clone()),
            Err(LifecycleAuthorityError::InvalidTransition(
                "unsigned confirmation required"
            ))
        ));
        let confirmation = LifecycleConfirmationV1 {
            schema_version: 1,
            session_id: "request-1".into(),
            action: LifecycleConfirmationAction::InstallUnsigned,
            target_count: 1,
            scope_digest_hex: "e".repeat(64),
            phrase: "INSTALL UNSIGNED 1 SYSTEMS".into(),
            generation: 1,
            key_id: "authority-v1".into(),
            signature_hex: String::new(),
        }
        .sign("authority-v1", &signing_key);
        authority
            .select_unsigned_artifact(selection, confirmation, &signing_key.verifying_key())
            .unwrap();
        authority.finish().unwrap();
    }

    fn unsigned_selection(target_id: &str, digest: &str) -> LifecycleArtifactSelectionV1 {
        LifecycleArtifactSelectionV1 {
            schema_version: 1,
            selection_id: format!("selection-{target_id}"),
            target_id: target_id.into(),
            channel: mackes_mesh_types::lifecycle::LifecycleArtifactChannel::Dev,
            artifact_digest_hex: digest.to_owned(),
            source_revision: "b".repeat(40),
            signed: false,
            unverified_build: true,
            generation: 1,
        }
    }

    fn unsigned_fleet_confirmation(
        count: u32,
        digest: &str,
        signing_key: &ed25519_dalek::SigningKey,
    ) -> LifecycleConfirmationV1 {
        LifecycleConfirmationV1 {
            schema_version: 1,
            session_id: "request-1".into(),
            action: LifecycleConfirmationAction::InstallUnsigned,
            target_count: count,
            scope_digest_hex: digest.to_owned(),
            phrase: format!("INSTALL UNSIGNED {count} SYSTEMS"),
            generation: 1,
            key_id: "authority-v1".into(),
            signature_hex: String::new(),
        }
        .sign("authority-v1", signing_key)
    }

    #[test]
    fn fleet_unsigned_select_requires_count_and_byte_digest() {
        let root = tempfile::tempdir().unwrap();
        let mut first_plan = plan();
        first_plan.intent = LifecycleIntentKind::Upgrade;
        first_plan.steps = vec!["packages".into(), "verify".into()];
        let mut second_plan = first_plan.clone();
        second_plan.target_id = "seat-16".into();
        let first = LifecycleAuthority::begin(root.path(), first_plan).unwrap();
        let second = LifecycleAuthority::begin(root.path(), second_plan).unwrap();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[13; 32]);
        let digest = "e".repeat(64);
        let selections = [
            unsigned_selection("seat-15", &digest),
            unsigned_selection("seat-16", &digest),
        ];
        assert!(matches!(
            execute_fleet_unsigned_select(
                &mut [first, second],
                &selections,
                unsigned_fleet_confirmation(1, &digest, &signing_key),
                &signing_key.verifying_key()
            ),
            Err(LifecycleAuthorityError::InvalidTransition(
                "unsigned confirmation scope"
            ))
        ));
    }

    #[test]
    fn fleet_unsigned_select_admits_the_same_bytes_on_every_seat() {
        let root = tempfile::tempdir().unwrap();
        let mut first_plan = plan();
        first_plan.intent = LifecycleIntentKind::Upgrade;
        first_plan.steps = vec!["packages".into(), "verify".into()];
        let mut second_plan = first_plan.clone();
        second_plan.target_id = "seat-16".into();
        let first = LifecycleAuthority::begin(root.path(), first_plan).unwrap();
        let second = LifecycleAuthority::begin(root.path(), second_plan).unwrap();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[13; 32]);
        let digest = "e".repeat(64);
        let selections = [
            unsigned_selection("seat-15", &digest),
            unsigned_selection("seat-16", &digest),
        ];
        let mut authorities = [first, second];
        let report = execute_fleet_unsigned_select(
            &mut authorities,
            &selections,
            unsigned_fleet_confirmation(2, &digest, &signing_key),
            &signing_key.verifying_key(),
        )
        .unwrap();
        assert_eq!(report.target_count, 2);
        assert_eq!(report.phase, LifecyclePhase::Running);
        for authority in &authorities {
            assert_eq!(
                authority.checkpoint().progress.phase,
                LifecyclePhase::Planned
            );
            let selection = authority.checkpoint().artifact_selection.as_ref().unwrap();
            assert_eq!(selection.artifact_digest_hex, digest);
            assert!(selection.unverified_build);
        }
        for authority in authorities {
            authority.finish().unwrap();
        }
    }

    fn upgrade_plan(target_id: &str) -> LifecyclePlanV1 {
        LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: target_id.into(),
            intent: LifecycleIntentKind::Upgrade,
            generation: 1,
            steps: vec!["packages".into(), "verify".into()],
        }
    }

    fn signed_selection(target_id: &str, digest: &str) -> LifecycleArtifactSelectionV1 {
        LifecycleArtifactSelectionV1 {
            schema_version: 1,
            selection_id: format!("selection-{target_id}"),
            target_id: target_id.into(),
            channel: mackes_mesh_types::lifecycle::LifecycleArtifactChannel::Candidate,
            artifact_digest_hex: digest.to_owned(),
            source_revision: "rev-1".into(),
            signed: true,
            unverified_build: false,
            generation: 1,
        }
    }

    #[test]
    fn fleet_upgrade_refuses_missing_or_mixed_artifact() {
        let root = tempfile::tempdir().unwrap();
        let first = LifecycleAuthority::begin(root.path(), upgrade_plan("seat-15")).unwrap();
        let second = LifecycleAuthority::begin(root.path(), upgrade_plan("seat-16")).unwrap();
        assert!(matches!(
            execute_fleet_upgrade(&mut [first, second], None),
            Err(LifecycleAuthorityError::InvalidTransition(
                "artifact selection missing"
            ))
        ));
        let mut first = LifecycleAuthority::resume(root.path(), "seat-15").unwrap();
        let mut second = LifecycleAuthority::resume(root.path(), "seat-16").unwrap();
        first
            .select_artifact(signed_selection("seat-15", &"e".repeat(64)))
            .unwrap();
        second
            .select_artifact(signed_selection("seat-16", &"f".repeat(64)))
            .unwrap();
        assert!(matches!(
            execute_fleet_upgrade(&mut [first, second], None),
            Err(LifecycleAuthorityError::InvalidTransition(
                "mixed fleet generation"
            ))
        ));
    }

    #[test]
    fn fleet_upgrade_walks_packages_and_queues_pending_without_inventing_install() {
        let root = tempfile::tempdir().unwrap();
        let artifact = root.path().join("role.rpm");
        std::fs::write(&artifact, b"rpm-bytes").unwrap();
        let digest = enrollment_bearer_digest_hex("rpm-bytes");
        let first = LifecycleAuthority::begin(root.path(), upgrade_plan("seat-15")).unwrap();
        let second = LifecycleAuthority::begin(root.path(), upgrade_plan("seat-16")).unwrap();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[13; 32]);
        let mut authorities = [first, second];
        execute_fleet_unsigned_select(
            &mut authorities,
            &[
                unsigned_selection("seat-15", &digest),
                unsigned_selection("seat-16", &digest),
            ],
            unsigned_fleet_confirmation(2, &digest, &signing_key),
            &signing_key.verifying_key(),
        )
        .unwrap();
        let report = execute_fleet_upgrade(&mut authorities, Some(artifact.as_path())).unwrap();
        assert_eq!(report.target_count, 2);
        assert_eq!(report.succeeded, 0);
        assert_ne!(report.phase, LifecyclePhase::Succeeded);
        assert!(
            root.path()
                .join("lifecycle")
                .join(crate::onboard::firstboot::FIRSTBOOT_PENDING)
                .exists(),
            "fleet upgrade must queue pending-convergence instead of dest install"
        );
        for authority in authorities {
            assert_eq!(
                authority.checkpoint().progress.completed_steps,
                1,
                "final verify must not complete while pending-convergence is queued"
            );
            authority.finish().unwrap();
        }
    }

    #[test]
    fn upgrade_persists_correction_plan_when_packages_block() {
        let root = tempfile::tempdir().unwrap();
        let mut authority =
            LifecycleAuthority::begin(root.path(), upgrade_plan("seat-15")).unwrap();
        authority
            .record_check(LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "packages".into(),
                target_id: "seat-15".into(),
                expected: "present".into(),
                observed: "missing".into(),
                status: mackes_mesh_types::lifecycle::LifecycleCheckStatus::Fail,
                required: true,
                evidence_digest_hex: "c".repeat(64),
                warning: None,
                generation: 1,
            })
            .unwrap();
        authority.run_declared_until_blocked(None).unwrap();
        let plan = authority
            .checkpoint()
            .correction_plan
            .as_ref()
            .expect("upgrade must persist the VAC DAG when packages block");
        assert_eq!(plan.corrections[0].check_id, "packages");
        assert_eq!(plan.corrections[0].step, "packages");
        assert_eq!(authority.checkpoint().progress.completed_steps, 0);
        authority.finish().unwrap();
    }

    #[test]
    fn onboard_persists_correction_plan_when_identity_blocks() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        authority
            .record_check(LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "identity".into(),
                target_id: "seat-15".into(),
                expected: "present".into(),
                observed: "missing".into(),
                status: mackes_mesh_types::lifecycle::LifecycleCheckStatus::Fail,
                required: true,
                evidence_digest_hex: "d".repeat(64),
                warning: None,
                generation: 1,
            })
            .unwrap();
        authority.run_declared_until_blocked(None).unwrap();
        let plan = authority
            .checkpoint()
            .correction_plan
            .as_ref()
            .expect("onboard must persist the correction DAG when identity blocks");
        assert_eq!(plan.corrections[0].check_id, "identity");
        assert_eq!(plan.corrections[0].step, "identity");
        assert_eq!(authority.checkpoint().progress.completed_steps, 0);
        authority.finish().unwrap();
    }

    #[test]
    fn reset_persists_correction_plan_without_implying_wipe() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), reset_plan("seat-15")).unwrap();
        authority
            .record_check(LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "identity".into(),
                target_id: "seat-15".into(),
                expected: "revoked".into(),
                observed: "present".into(),
                status: mackes_mesh_types::lifecycle::LifecycleCheckStatus::Fail,
                required: true,
                evidence_digest_hex: "e".repeat(64),
                warning: None,
                generation: 1,
            })
            .unwrap();
        authority.run_declared_until_blocked(None).unwrap();
        let plan = authority
            .checkpoint()
            .correction_plan
            .as_ref()
            .expect("reset must persist the correction DAG when identity remains");
        assert_eq!(plan.corrections[0].check_id, "identity");
        assert_eq!(authority.checkpoint().progress.completed_steps, 0);
        authority.finish().unwrap();
    }

    #[test]
    fn offboard_persists_correction_plan_without_implying_wipe() {
        let root = tempfile::tempdir().unwrap();
        let mut authority =
            LifecycleAuthority::begin(root.path(), offboard_plan("seat-15")).unwrap();
        authority
            .record_check(LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "offboard".into(),
                target_id: "seat-15".into(),
                expected: "erased".into(),
                observed: "still-present".into(),
                status: mackes_mesh_types::lifecycle::LifecycleCheckStatus::Fail,
                required: true,
                evidence_digest_hex: "a".repeat(64),
                warning: None,
                generation: 1,
            })
            .unwrap();
        authority.run_declared_until_blocked(None).unwrap();
        let plan = authority
            .checkpoint()
            .correction_plan
            .as_ref()
            .expect("offboard must persist the correction DAG when erasure is not observed");
        assert_eq!(plan.corrections[0].check_id, "offboard");
        assert_eq!(authority.checkpoint().progress.completed_steps, 0);
        authority.finish().unwrap();
    }

    #[test]
    fn correction_plan_requires_current_blocking_checks() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        authority
            .record_check(LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "mesh".into(),
                target_id: "seat-15".into(),
                expected: "joined".into(),
                observed: "absent".into(),
                status: mackes_mesh_types::lifecycle::LifecycleCheckStatus::Fail,
                required: true,
                evidence_digest_hex: "f".repeat(64),
                warning: None,
                generation: 1,
            })
            .unwrap();
        let correction = authority.propose_correction_plan().unwrap();
        assert_eq!(correction.corrections[0].check_id, "mesh");
        assert_eq!(correction.corrections[0].step, "mesh");
        assert_eq!(correction.corrections[0].reason, "absent");
        authority.admit_correction_plan(correction.clone()).unwrap();
        authority.finish().unwrap();
        let mut resumed = LifecycleAuthority::resume(root.path(), "seat-15").unwrap();
        assert_eq!(
            resumed.checkpoint().correction_plan.as_ref(),
            Some(&correction)
        );
        let mut reordered = correction.clone();
        reordered.corrections[0].reason = "different order".into();
        assert!(matches!(
            resumed.admit_correction_plan(reordered),
            Err(LifecycleAuthorityError::InvalidTransition(
                "correction plan immutable"
            ))
        ));
        resumed.admit_correction_plan(correction).unwrap();
        resumed.finish().unwrap();
    }

    #[test]
    fn peek_proposes_the_same_correction_plan_without_taking_the_lock() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        authority
            .record_check(LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "mesh".into(),
                target_id: "seat-15".into(),
                expected: "joined".into(),
                observed: "absent".into(),
                status: mackes_mesh_types::lifecycle::LifecycleCheckStatus::Fail,
                required: true,
                evidence_digest_hex: "f".repeat(64),
                warning: None,
                generation: 1,
            })
            .unwrap();
        let held = authority.propose_correction_plan().unwrap();
        let peeked = LifecycleAuthority::peek(root.path(), "seat-15").unwrap();
        assert_eq!(peeked.propose_correction_plan().unwrap(), held);
        assert!(
            LifecycleAuthority::resume(root.path(), "seat-15").is_err(),
            "peek must not release the exclusive authority lock"
        );
        authority.finish().unwrap();
    }

    #[test]
    fn verify_and_correct_retries_before_terminal_failure() {
        let root = tempfile::tempdir().unwrap();
        let plan = LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-vac-retry".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::VerifyAndCorrect,
            generation: 1,
            steps: vec!["verify".into(), "configuration".into(), "verify".into()],
        };
        let mut authority = LifecycleAuthority::begin(root.path(), plan).unwrap();
        for attempt in 1..=2 {
            assert!(matches!(
                authority.run_next(|_| Err("transient".into())),
                Err(LifecycleAuthorityError::StepFailed(message)) if message == "transient"
            ));
            assert_eq!(authority.checkpoint().retry_count, attempt);
            assert_eq!(
                authority.checkpoint().progress.phase,
                LifecyclePhase::Planned
            );
        }
        assert!(matches!(
            authority.run_next(|_| Err("transient".into())),
            Err(LifecycleAuthorityError::StepFailed(message)) if message == "transient"
        ));
        assert_eq!(authority.checkpoint().retry_count, 3);
        assert_eq!(
            authority.checkpoint().progress.phase,
            LifecyclePhase::Failed
        );
        authority.finish().unwrap();
    }

    #[test]
    fn proposed_correction_plan_binds_baseline_prerequisites() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        for (check_id, observed, digest) in
            [("packages", "missing", "1"), ("units", "inactive", "2")]
        {
            authority
                .record_check(LifecycleRequirementCheckV1 {
                    schema_version: 1,
                    check_id: check_id.into(),
                    target_id: "seat-15".into(),
                    expected: "present".into(),
                    observed: observed.into(),
                    status: mackes_mesh_types::lifecycle::LifecycleCheckStatus::Fail,
                    required: true,
                    evidence_digest_hex: digest.repeat(64),
                    warning: None,
                    generation: 1,
                })
                .unwrap();
        }
        let correction = authority.propose_correction_plan().unwrap();
        let units = correction
            .corrections
            .iter()
            .find(|item| item.check_id == "units")
            .expect("units correction");
        assert_eq!(units.step, "configuration");
        assert_eq!(units.prerequisites, vec!["packages"]);
        assert!(correction
            .edges
            .iter()
            .any(|(from, to)| from == "packages" && to == "units"));
        authority.finish().unwrap();
    }

    #[test]
    fn package_step_requires_pinned_artifact_selection() {
        let root = tempfile::tempdir().unwrap();
        let plan = LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-pkg".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::Onboard,
            generation: 1,
            steps: vec!["packages".into()],
        };
        let mut authority = LifecycleAuthority::begin(root.path(), plan).unwrap();
        assert!(matches!(
            authority.run_next(|_| Ok(())),
            Err(LifecycleAuthorityError::InvalidTransition(
                "artifact selection missing"
            ))
        ));
        authority.finish().unwrap();
    }

    #[test]
    fn terminal_progress_cannot_be_reopened() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        authority.complete_step(0).unwrap();
        authority.complete_step(1).unwrap();
        let mut reopened = authority.checkpoint().progress.clone();
        reopened.phase = LifecyclePhase::Running;
        assert!(matches!(
            authority.update(reopened),
            Err(LifecycleAuthorityError::InvalidTransition(
                "phase transition"
            ))
        ));
        authority.finish().unwrap();
    }

    #[test]
    fn confirmation_cannot_cross_generation_or_be_replaced() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(
            root.path(),
            LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-off".into(),
                target_id: "seat-15".into(),
                intent: LifecycleIntentKind::Offboard,
                generation: 3,
                steps: vec!["offboard".into()],
            },
        )
        .unwrap();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[12; 32]);
        let make = |generation| {
            LifecycleConfirmationV1 {
                schema_version: 1,
                session_id: "request-off".into(),
                action: LifecycleConfirmationAction::Offboard,
                target_count: 1,
                scope_digest_hex: "1".repeat(64),
                phrase: "FORCE OFFBOARD 1 SYSTEMS".into(),
                generation,
                key_id: "authority-v1".into(),
                signature_hex: String::new(),
            }
            .sign("authority-v1", &signing_key)
        };
        assert!(matches!(
            authority.accept_confirmation(make(2), &signing_key.verifying_key()),
            Err(LifecycleAuthorityError::InvalidTransition(
                "confirmation scope"
            ))
        ));
        authority
            .accept_confirmation(make(3), &signing_key.verifying_key())
            .unwrap();
        assert!(matches!(
            authority.accept_confirmation(make(3), &signing_key.verifying_key()),
            Err(LifecycleAuthorityError::InvalidTransition(
                "confirmation replay"
            ))
        ));
        authority.finish().unwrap();
    }

    #[test]
    fn direct_completion_with_blocking_check_waits_for_operator() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        authority
            .record_check(LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "identity".into(),
                target_id: "seat-15".into(),
                expected: "present".into(),
                observed: "missing".into(),
                status: mackes_mesh_types::lifecycle::LifecycleCheckStatus::Fail,
                required: true,
                evidence_digest_hex: "2".repeat(64),
                warning: None,
                generation: 1,
            })
            .unwrap();
        authority.complete_step(0).unwrap();
        authority.complete_step(1).unwrap();
        assert_eq!(
            authority.checkpoint().progress.phase,
            LifecyclePhase::WaitingForOperator
        );
        authority.finish().unwrap();
    }

    #[test]
    fn interrupted_session_resumes_and_refuses_replay_or_jump() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        authority.complete_step(0).unwrap();
        drop(authority);
        let mut resumed = LifecycleAuthority::resume(root.path(), "seat-15").unwrap();
        assert!(matches!(
            resumed.complete_step(0),
            Err(LifecycleAuthorityError::InvalidTransition("step order"))
        ));
        assert!(matches!(
            resumed.complete_step(2),
            Err(LifecycleAuthorityError::InvalidTransition("step order"))
        ));
        resumed.complete_step(1).unwrap();
        assert_eq!(
            resumed.checkpoint().progress.phase,
            LifecyclePhase::Succeeded
        );
        resumed.finish().unwrap();
    }

    #[test]
    fn peek_reads_without_taking_the_mutation_lock() {
        let root = tempfile::tempdir().unwrap();
        let authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        let peeked = LifecycleAuthority::peek(root.path(), "seat-15").unwrap();
        assert_eq!(peeked.plan.request_id, "request-1");
        assert_eq!(peeked.progress.phase, LifecyclePhase::Planned);
        let latest = LifecycleAuthority::peek_latest(root.path())
            .unwrap()
            .expect("begun session is visible to a renderer");
        assert_eq!(latest.plan.target_id, "seat-15");
        authority.finish().unwrap();
        assert!(LifecycleAuthority::peek_latest(root.path())
            .unwrap()
            .is_some());
        assert!(
            LifecycleAuthority::peek_latest(&root.path().join("missing"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn peek_latest_prefers_the_newer_generation() {
        let root = tempfile::tempdir().unwrap();
        let first = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        first.finish().unwrap();
        let mut newer = plan();
        newer.request_id = "request-2".into();
        newer.target_id = "seat-16".into();
        newer.generation = 2;
        let second = LifecycleAuthority::begin(root.path(), newer).unwrap();
        second.finish().unwrap();
        let latest = LifecycleAuthority::peek_latest(root.path())
            .unwrap()
            .expect("two checkpoints");
        assert_eq!(latest.plan.target_id, "seat-16");
        assert_eq!(latest.progress.generation, 2);
    }

    #[test]
    fn run_next_commits_success_and_records_failure() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        authority
            .run_next(|step| {
                assert_eq!(step, "identity");
                Ok(())
            })
            .unwrap();
        assert_eq!(authority.checkpoint().progress.completed_steps, 1);
        let error = authority
            .run_next(|step| Err(format!("{step} unavailable")))
            .unwrap_err();
        assert!(matches!(error, LifecycleAuthorityError::StepFailed(_)));
        assert_eq!(
            authority.checkpoint().progress.phase,
            LifecyclePhase::Failed
        );
        authority.finish().unwrap();
    }

    #[test]
    fn offboarding_receipt_requires_completed_offboard_plan() {
        let root = tempfile::tempdir().unwrap();
        let mut offboard = plan();
        offboard.intent = LifecycleIntentKind::Offboard;
        let mut authority = LifecycleAuthority::begin(root.path(), offboard).unwrap();
        assert!(matches!(
            authority.offboarding_receipt(),
            Err(LifecycleAuthorityError::InvalidTransition(
                "offboard incomplete"
            ))
        ));
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
        let confirmation = LifecycleConfirmationV1 {
            schema_version: 1,
            session_id: "request-1".into(),
            action: LifecycleConfirmationAction::Offboard,
            target_count: 1,
            scope_digest_hex: "a".repeat(64),
            phrase: "FORCE OFFBOARD 1 SYSTEMS".into(),
            generation: 1,
            key_id: "authority-v1".into(),
            signature_hex: String::new(),
        }
        .sign("authority-v1", &signing_key);
        authority
            .accept_confirmation(confirmation, &signing_key.verifying_key())
            .unwrap();
        authority.run_next(|_| Ok(())).unwrap();
        authority.run_next(|_| Ok(())).unwrap();
        assert!(
            matches!(
                authority.offboarding_receipt(),
                Err(LifecycleAuthorityError::InvalidTransition(
                    "erasure not observed"
                ))
            ),
            "plan success is not a dest wipe"
        );
        authority
            .record_check(LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "erasure".into(),
                target_id: "seat-15".into(),
                expected: "erased".into(),
                observed: "erased".into(),
                status: mackes_mesh_types::lifecycle::LifecycleCheckStatus::Pass,
                required: true,
                evidence_digest_hex: "e".repeat(64),
                warning: None,
                generation: 1,
            })
            .unwrap();
        let receipt = authority.offboarding_receipt().unwrap();
        assert!(receipt.completed);
        assert!(receipt.retained_resources.is_empty());
        authority.finish().unwrap();
    }

    #[test]
    fn persist_offboarding_receipt_survives_finish() {
        let root = tempfile::tempdir().unwrap();
        let mut offboard = plan();
        offboard.intent = LifecycleIntentKind::Offboard;
        let mut authority = LifecycleAuthority::begin(root.path(), offboard).unwrap();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
        let confirmation = LifecycleConfirmationV1 {
            schema_version: 1,
            session_id: "request-1".into(),
            action: LifecycleConfirmationAction::Offboard,
            target_count: 1,
            scope_digest_hex: "a".repeat(64),
            phrase: "FORCE OFFBOARD 1 SYSTEMS".into(),
            generation: 1,
            key_id: "authority-v1".into(),
            signature_hex: String::new(),
        }
        .sign("authority-v1", &signing_key);
        authority
            .accept_confirmation(confirmation, &signing_key.verifying_key())
            .unwrap();
        authority.run_next(|_| Ok(())).unwrap();
        authority.run_next(|_| Ok(())).unwrap();
        assert!(
            matches!(
                authority.persist_offboarding_receipt(),
                Err(LifecycleAuthorityError::InvalidTransition(
                    "erasure not observed"
                ))
            ),
            "plan success must not persist a dest wipe receipt"
        );
        assert!(
            LifecycleAuthority::peek_offboarding_receipt(root.path(), "seat-15")
                .unwrap()
                .is_none(),
            "projection without erasure must not write receipt.json"
        );
        authority
            .record_check(LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "erasure".into(),
                target_id: "seat-15".into(),
                expected: "erased".into(),
                observed: "erased".into(),
                status: mackes_mesh_types::lifecycle::LifecycleCheckStatus::Pass,
                required: true,
                evidence_digest_hex: "e".repeat(64),
                warning: None,
                generation: 1,
            })
            .unwrap();
        let persisted = authority.persist_offboarding_receipt().unwrap();
        assert!(persisted.completed);
        assert!(persisted.retained_resources.is_empty());
        let held = LifecycleAuthority::peek_offboarding_receipt(root.path(), "seat-15")
            .expect("peek while the mutation lock is held");
        assert_eq!(held.as_ref().map(|receipt| receipt.completed), Some(true));
        authority.finish().unwrap();
        let stolen = LifecycleAuthority::resume(root.path(), "seat-15").unwrap();
        let peeked = LifecycleAuthority::peek_offboarding_receipt(root.path(), "seat-15")
            .unwrap()
            .expect("durable receipt must survive finish");
        assert_eq!(peeked.request_id, "request-1");
        assert_eq!(peeked.target_id, "seat-15");
        assert!(peeked.completed);
        assert!(peeked.retained_resources.is_empty());
        stolen.finish().unwrap();
    }

    #[test]
    fn fleet_report_rejects_mixed_generations_and_false_success() {
        let root = tempfile::tempdir().unwrap();
        let first = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        let mut second_plan = plan();
        second_plan.target_id = "seat-16".into();
        let second = LifecycleAuthority::begin(root.path(), second_plan).unwrap();
        let report = fleet_report(
            "request-1",
            1,
            &[first.checkpoint().clone(), second.checkpoint().clone()],
        )
        .unwrap();
        assert_eq!(report.phase, LifecyclePhase::Running);
        assert_eq!(report.succeeded, 0);
        let mut mixed_plan = plan();
        mixed_plan.target_id = "seat-17".into();
        mixed_plan.generation = 2;
        let mixed = LifecycleAuthority::begin(root.path(), mixed_plan).unwrap();
        assert!(matches!(
            fleet_report(
                "request-1",
                1,
                &[first.checkpoint().clone(), mixed.checkpoint().clone()]
            ),
            Err(LifecycleAuthorityError::InvalidTransition(
                "mixed fleet generation"
            ))
        ));
        first.finish().unwrap();
        second.finish().unwrap();
        mixed.finish().unwrap();
    }

    #[test]
    fn fleet_coordinator_handoff_requires_existing_checkpoints_and_a_new_coordinator() {
        let root = tempfile::tempdir().unwrap();
        let first = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        let mut second_plan = plan();
        second_plan.target_id = "seat-16".into();
        let second = LifecycleAuthority::begin(root.path(), second_plan).unwrap();
        let checkpoints = [first.checkpoint().clone(), second.checkpoint().clone()];
        assert!(matches!(
            transfer_fleet_coordination("request-1", 1, "coord-a", "coord-a", &checkpoints),
            Err(LifecycleAuthorityError::InvalidTransition(
                "coordinator unchanged"
            ))
        ));
        let report =
            transfer_fleet_coordination("request-1", 1, "coord-a", "coord-b", &checkpoints)
                .unwrap();
        assert_eq!(report.target_count, 2);
        assert_eq!(report.phase, LifecyclePhase::Running);
        first.finish().unwrap();
        second.finish().unwrap();
    }

    #[test]
    fn fleet_handoff_persists_coordinator_and_refuses_a_forged_initiator() {
        let root = tempfile::tempdir().unwrap();
        let first = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        let mut second_plan = plan();
        second_plan.target_id = "seat-16".into();
        let second = LifecycleAuthority::begin(root.path(), second_plan).unwrap();
        let mut authorities = [first, second];
        execute_fleet_handoff(&mut authorities, "coord-a", "coord-b").unwrap();
        for authority in &authorities {
            assert_eq!(
                authority.checkpoint().coordinator_id.as_deref(),
                Some("coord-b")
            );
        }
        assert!(
            matches!(
                execute_fleet_handoff(&mut authorities, "coord-forged", "coord-c"),
                Err(LifecycleAuthorityError::InvalidTransition(
                    "coordinator mismatch"
                ))
            ),
            "disconnected initiator cannot invent a new coordinator"
        );
        execute_fleet_handoff(&mut authorities, "coord-b", "coord-c").unwrap();
        assert_eq!(
            authorities[0].checkpoint().coordinator_id.as_deref(),
            Some("coord-c")
        );
        for authority in authorities {
            authority.finish().unwrap();
        }
    }

    #[test]
    fn peek_fleet_session_survives_coordinator_disconnect() {
        let root = tempfile::tempdir().unwrap();
        let first = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        let mut second_plan = plan();
        second_plan.target_id = "seat-16".into();
        let second = LifecycleAuthority::begin(root.path(), second_plan).unwrap();
        let mut authorities = [first, second];
        execute_fleet_handoff(&mut authorities, "coord-a", "coord-b").unwrap();
        let locked = fleet_report(
            "request-1",
            1,
            &[
                authorities[0].checkpoint().clone(),
                authorities[1].checkpoint().clone(),
            ],
        )
        .unwrap();
        let (peeked_report, peeked) =
            peek_fleet_session(root.path(), &["seat-15", "seat-16"]).unwrap();
        assert_eq!(peeked_report, locked);
        assert_eq!(peeked_report.coordinator_id, "coord-b");
        assert_eq!(peeked[0].coordinator_id.as_deref(), Some("coord-b"));
        assert_eq!(peeked[1].coordinator_id.as_deref(), Some("coord-b"));
        assert!(
            LifecycleAuthority::resume(root.path(), "seat-15").is_err(),
            "fleet status peek must not steal the mutation lock"
        );
        for authority in authorities {
            authority.finish().unwrap();
        }
        let (after, after_seats) =
            peek_fleet_session(root.path(), &["seat-15", "seat-16"]).unwrap();
        assert_eq!(after.request_id, "request-1");
        assert_eq!(after.generation, 1);
        assert_eq!(after.target_count, 2);
        assert_eq!(after.coordinator_id, "coord-b");
        assert_eq!(after_seats[0].coordinator_id.as_deref(), Some("coord-b"));
        let (partial, seats) = peek_fleet_session(root.path(), &["seat-15", "seat-99"]).unwrap();
        assert_eq!(partial.target_count, 1);
        assert_eq!(seats[0].plan.target_id, "seat-15");
        assert_eq!(partial.coordinator_id, "coord-b");
        assert!(
            peek_fleet_session(root.path(), &["seat-99"]).is_err(),
            "an all-missing list cannot invent a fleet job"
        );
        std::fs::remove_dir_all(root.path().join("lifecycle").join("seat-16")).unwrap();
        let (after_wipe, remaining) =
            peek_fleet_session(root.path(), &["seat-15", "seat-16"]).unwrap();
        assert_eq!(after_wipe.target_count, 1);
        assert_eq!(remaining[0].plan.target_id, "seat-15");
        assert_eq!(after_wipe.coordinator_id, "coord-b");
    }

    #[test]
    fn resume_fleet_skips_a_wiped_seat_and_keeps_the_job() {
        let root = tempfile::tempdir().unwrap();
        let first = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        let mut second_plan = plan();
        second_plan.target_id = "seat-16".into();
        let second = LifecycleAuthority::begin(root.path(), second_plan).unwrap();
        let mut authorities = [first, second];
        execute_fleet_handoff(&mut authorities, "coord-a", "coord-b").unwrap();
        for authority in authorities {
            authority.finish().unwrap();
        }
        std::fs::remove_dir_all(root.path().join("lifecycle").join("seat-16")).unwrap();
        let mut remaining = resume_fleet(root.path(), &["seat-15", "seat-16"]).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].checkpoint().plan.target_id, "seat-15");
        assert_eq!(
            remaining[0].checkpoint().coordinator_id.as_deref(),
            Some("coord-b")
        );
        execute_fleet_handoff(&mut remaining, "coord-b", "coord-c").unwrap();
        assert_eq!(
            remaining[0].checkpoint().coordinator_id.as_deref(),
            Some("coord-c")
        );
        remaining.pop().unwrap().finish().unwrap();
        assert!(
            matches!(
                resume_fleet(root.path(), &["seat-16"]),
                Err(LifecycleAuthorityError::InvalidPlan("fleet inputs"))
            ),
            "an all-missing list cannot invent a fleet job"
        );
    }

    #[test]
    fn peek_matching_fleet_targets_lists_one_durable_generation() {
        let root = tempfile::tempdir().unwrap();
        let first = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        let mut second_plan = plan();
        second_plan.target_id = "seat-16".into();
        let second = LifecycleAuthority::begin(root.path(), second_plan).unwrap();
        let mut other = plan();
        other.target_id = "seat-17".into();
        other.request_id = "request-other".into();
        let other = LifecycleAuthority::begin(root.path(), other).unwrap();
        first.finish().unwrap();
        second.finish().unwrap();
        other.finish().unwrap();
        let targets = peek_matching_fleet_targets(root.path(), "request-1", 1).unwrap();
        assert_eq!(targets, vec!["seat-15".to_owned(), "seat-16".to_owned()]);
        assert!(
            peek_matching_fleet_targets(root.path(), "request-1", 2)
                .unwrap()
                .is_empty(),
            "a later generation cannot inherit another fleet"
        );
    }

    fn offboard_plan(target_id: &str) -> LifecyclePlanV1 {
        LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: target_id.into(),
            intent: LifecycleIntentKind::Offboard,
            generation: 1,
            steps: vec!["offboard".into(), "verify".into()],
        }
    }

    fn fleet_offboard_confirmation(
        target_ids: &[&str],
        signing_key: &ed25519_dalek::SigningKey,
    ) -> LifecycleConfirmationV1 {
        let ids = target_ids
            .iter()
            .map(|target| (*target).to_owned())
            .collect::<Vec<_>>();
        let target_count = ids.len() as u32;
        LifecycleConfirmationV1 {
            schema_version: 1,
            session_id: "request-1".into(),
            action: LifecycleConfirmationAction::Offboard,
            target_count,
            scope_digest_hex: fleet_scope_digest(&ids),
            phrase: format!("FORCE OFFBOARD {target_count} SYSTEMS"),
            generation: 1,
            key_id: "authority-v1".into(),
            signature_hex: String::new(),
        }
        .sign("authority-v1", signing_key)
    }

    #[test]
    fn fleet_offboard_requires_one_phrase_bound_to_the_seat_list() {
        let root = tempfile::tempdir().unwrap();
        let mut first = LifecycleAuthority::begin(root.path(), offboard_plan("seat-15")).unwrap();
        let mut second = LifecycleAuthority::begin(root.path(), offboard_plan("seat-16")).unwrap();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        let wrong_count = fleet_offboard_confirmation(&["seat-15"], &signing_key);
        assert!(matches!(
            execute_fleet_offboard(
                &mut [first, second],
                wrong_count,
                &signing_key.verifying_key()
            ),
            Err(LifecycleAuthorityError::InvalidTransition(
                "confirmation scope"
            ))
        ));
        first = LifecycleAuthority::resume(root.path(), "seat-15").unwrap();
        second = LifecycleAuthority::resume(root.path(), "seat-16").unwrap();
        let wrong_scope = fleet_offboard_confirmation(&["seat-15", "seat-99"], &signing_key);
        assert!(matches!(
            first.accept_shared_confirmation(
                wrong_scope,
                &signing_key.verifying_key(),
                &["seat-15".into(), "seat-16".into()]
            ),
            Err(LifecycleAuthorityError::InvalidTransition(
                "confirmation scope"
            ))
        ));
        first.finish().unwrap();
        second.finish().unwrap();
    }

    #[test]
    fn fleet_offboard_runs_declared_steps_without_inventing_dest_wipe() {
        let root = tempfile::tempdir().unwrap();
        let first = LifecycleAuthority::begin(root.path(), offboard_plan("seat-15")).unwrap();
        let second = LifecycleAuthority::begin(root.path(), offboard_plan("seat-16")).unwrap();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        let confirmation = fleet_offboard_confirmation(&["seat-15", "seat-16"], &signing_key);
        let mut authorities = [first, second];
        let report =
            execute_fleet_offboard(&mut authorities, confirmation, &signing_key.verifying_key())
                .unwrap();
        assert_eq!(report.target_count, 2);
        assert_eq!(report.succeeded, 2);
        assert_eq!(report.phase, LifecyclePhase::Succeeded);
        for authority in &authorities {
            assert!(
                matches!(
                    authority.offboarding_receipt(),
                    Err(LifecycleAuthorityError::InvalidTransition(
                        "erasure not observed"
                    ))
                ),
                "fleet offboard must not invent a dest wipe receipt"
            );
            assert!(
                LifecycleAuthority::peek_offboarding_receipt(
                    root.path(),
                    &authority.checkpoint().plan.target_id
                )
                .unwrap()
                .is_none(),
                "plan success must not write receipt.json"
            );
        }
        for authority in authorities {
            authority.finish().unwrap();
        }
    }

    #[test]
    fn fleet_offboard_persists_receipts_only_after_erasure() {
        let root = tempfile::tempdir().unwrap();
        let first = LifecycleAuthority::begin(root.path(), offboard_plan("seat-15")).unwrap();
        let second = LifecycleAuthority::begin(root.path(), offboard_plan("seat-16")).unwrap();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        let confirmation = fleet_offboard_confirmation(&["seat-15", "seat-16"], &signing_key);
        let mut authorities = [first, second];
        execute_fleet_offboard(&mut authorities, confirmation, &signing_key.verifying_key())
            .unwrap();
        for authority in &mut authorities {
            authority
                .record_check(LifecycleRequirementCheckV1 {
                    schema_version: 1,
                    check_id: "erasure".into(),
                    target_id: authority.checkpoint().plan.target_id.clone(),
                    expected: "erased".into(),
                    observed: "erased".into(),
                    status: mackes_mesh_types::lifecycle::LifecycleCheckStatus::Pass,
                    required: true,
                    evidence_digest_hex: "e".repeat(64),
                    warning: None,
                    generation: 1,
                })
                .unwrap();
        }
        persist_completed_offboarding_receipts(&authorities).unwrap();
        for target in ["seat-15", "seat-16"] {
            let receipt = LifecycleAuthority::peek_offboarding_receipt(root.path(), target)
                .unwrap()
                .expect("durable receipt after observed erasure");
            assert!(receipt.completed);
            assert!(receipt.retained_resources.is_empty());
            assert_eq!(receipt.target_id, target);
        }
        assert!(
            LifecycleAuthority::resume(root.path(), "seat-15").is_err(),
            "receipt persist must not drop the mutation lock"
        );
        for authority in authorities {
            authority.finish().unwrap();
        }
    }

    #[test]
    fn fleet_mutation_waves_keep_the_full_confirmation_scope() {
        assert!(
            3 > FLEET_MUTATION_WAVE,
            "fixture must span more than one mutation wave"
        );
        let root = tempfile::tempdir().unwrap();
        let first = LifecycleAuthority::begin(root.path(), offboard_plan("seat-15")).unwrap();
        let second = LifecycleAuthority::begin(root.path(), offboard_plan("seat-16")).unwrap();
        let third = LifecycleAuthority::begin(root.path(), offboard_plan("seat-17")).unwrap();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        let ids = ["seat-15", "seat-16", "seat-17"];
        let confirmation = fleet_offboard_confirmation(&ids, &signing_key);
        let mut authorities = [first, second, third];
        let report =
            execute_fleet_offboard(&mut authorities, confirmation, &signing_key.verifying_key())
                .unwrap();
        assert_eq!(report.target_count, 3);
        assert_eq!(report.succeeded, 3);
        for authority in &authorities {
            let confirmation = authority
                .checkpoint()
                .confirmation
                .as_ref()
                .expect("wave must keep the admitted phrase");
            assert_eq!(confirmation.target_count, 3);
            assert_eq!(confirmation.phrase, "FORCE OFFBOARD 3 SYSTEMS");
        }
        for authority in authorities {
            authority.finish().unwrap();
        }
    }

    #[test]
    fn fleet_mutation_second_wave_failure_keeps_first_wave() {
        let root = tempfile::tempdir().unwrap();
        let first = LifecycleAuthority::begin(root.path(), offboard_plan("seat-15")).unwrap();
        let second = LifecycleAuthority::begin(root.path(), offboard_plan("seat-16")).unwrap();
        let mut third = LifecycleAuthority::begin(root.path(), offboard_plan("seat-17")).unwrap();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        let ids = [
            "seat-15".to_owned(),
            "seat-16".to_owned(),
            "seat-17".to_owned(),
        ];
        let confirmation =
            fleet_offboard_confirmation(&["seat-15", "seat-16", "seat-17"], &signing_key);
        third
            .accept_shared_confirmation(confirmation.clone(), &signing_key.verifying_key(), &ids)
            .unwrap();
        let mut authorities = [first, second, third];
        assert!(
            execute_fleet_offboard(&mut authorities, confirmation, &signing_key.verifying_key())
                .is_err(),
            "a replay on the second wave must fail closed"
        );
        assert_eq!(
            authorities[0]
                .checkpoint()
                .confirmation
                .as_ref()
                .map(|confirmation| confirmation.phrase.as_str()),
            Some("FORCE OFFBOARD 3 SYSTEMS")
        );
        assert_eq!(
            authorities[1]
                .checkpoint()
                .confirmation
                .as_ref()
                .map(|confirmation| confirmation.phrase.as_str()),
            Some("FORCE OFFBOARD 3 SYSTEMS")
        );
        for authority in authorities {
            authority.finish().unwrap();
        }
        let (report, peeked) =
            peek_fleet_session(root.path(), &["seat-15", "seat-16", "seat-17"]).unwrap();
        assert_eq!(report.target_count, 3);
        assert_eq!(
            peeked[0]
                .confirmation
                .as_ref()
                .map(|confirmation| confirmation.phrase.as_str()),
            Some("FORCE OFFBOARD 3 SYSTEMS")
        );
        assert_eq!(
            peeked[1]
                .confirmation
                .as_ref()
                .map(|confirmation| confirmation.phrase.as_str()),
            Some("FORCE OFFBOARD 3 SYSTEMS")
        );
        let lines = crate::onboard::firstboot::fleet_session_status_lines(&report, &peeked);
        assert!(
            lines
                .iter()
                .any(|line| line == "fleet seat-15, seat-16, seat-17"),
            "peek after a failed wave must still name every durable seat: {lines:?}"
        );
    }

    fn reset_plan(target_id: &str) -> LifecyclePlanV1 {
        LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: target_id.into(),
            intent: LifecycleIntentKind::ResetAndOnboard,
            generation: 1,
            steps: vec!["offboard".into(), "identity".into(), "verify".into()],
        }
    }

    fn fleet_reset_confirmation(
        target_ids: &[&str],
        signing_key: &ed25519_dalek::SigningKey,
    ) -> LifecycleConfirmationV1 {
        let ids = target_ids
            .iter()
            .map(|target| (*target).to_owned())
            .collect::<Vec<_>>();
        let target_count = ids.len() as u32;
        LifecycleConfirmationV1 {
            schema_version: 1,
            session_id: "request-1".into(),
            action: LifecycleConfirmationAction::Reset,
            target_count,
            scope_digest_hex: fleet_scope_digest(&ids),
            phrase: format!("WIPE {target_count} SYSTEMS"),
            generation: 1,
            key_id: "authority-v1".into(),
            signature_hex: String::new(),
        }
        .sign("authority-v1", signing_key)
    }

    #[test]
    fn fleet_reset_refuses_an_offboard_phrase_and_a_wrong_count() {
        let root = tempfile::tempdir().unwrap();
        let first = LifecycleAuthority::begin(root.path(), reset_plan("seat-15")).unwrap();
        let second = LifecycleAuthority::begin(root.path(), reset_plan("seat-16")).unwrap();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[11; 32]);
        assert!(matches!(
            execute_fleet_reset(
                &mut [first, second],
                fleet_offboard_confirmation(&["seat-15", "seat-16"], &signing_key),
                &signing_key.verifying_key()
            ),
            Err(LifecycleAuthorityError::InvalidTransition(
                "confirmation scope"
            ))
        ));
        let first = LifecycleAuthority::resume(root.path(), "seat-15").unwrap();
        let second = LifecycleAuthority::resume(root.path(), "seat-16").unwrap();
        assert!(matches!(
            execute_fleet_reset(
                &mut [first, second],
                fleet_reset_confirmation(&["seat-15"], &signing_key),
                &signing_key.verifying_key()
            ),
            Err(LifecycleAuthorityError::InvalidTransition(
                "confirmation scope"
            ))
        ));
    }

    #[test]
    fn fleet_reset_runs_offboard_without_inventing_identity() {
        let root = tempfile::tempdir().unwrap();
        let first = LifecycleAuthority::begin(root.path(), reset_plan("seat-15")).unwrap();
        let second = LifecycleAuthority::begin(root.path(), reset_plan("seat-16")).unwrap();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[11; 32]);
        let confirmation = fleet_reset_confirmation(&["seat-15", "seat-16"], &signing_key);
        let mut authorities = [first, second];
        let report =
            execute_fleet_reset(&mut authorities, confirmation, &signing_key.verifying_key())
                .unwrap();
        assert_eq!(report.target_count, 2);
        assert_eq!(report.succeeded, 0);
        assert_eq!(report.phase, LifecyclePhase::Running);
        for authority in &mut authorities {
            assert_eq!(authority.checkpoint().progress.completed_steps, 1);
            assert_eq!(
                authority.checkpoint().progress.phase,
                LifecyclePhase::Running
            );
            let error = authority
                .run_next_declared(None)
                .expect_err("fleet reset must not mint identity without observed erase");
            assert!(
                matches!(
                    error,
                    LifecycleAuthorityError::StepFailed(ref message)
                        if message.contains("old identity") || message.contains("erasure")
                ),
                "dest wipe is not implied: {error:?}"
            );
        }
        for authority in authorities {
            authority.finish().unwrap();
        }
    }

    #[test]
    fn fleet_reset_persists_receipts_only_after_erasure() {
        let root = tempfile::tempdir().unwrap();
        let first = LifecycleAuthority::begin(root.path(), reset_plan("seat-15")).unwrap();
        let second = LifecycleAuthority::begin(root.path(), reset_plan("seat-16")).unwrap();
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[11; 32]);
        let confirmation = fleet_reset_confirmation(&["seat-15", "seat-16"], &signing_key);
        let mut authorities = [first, second];
        execute_fleet_reset(&mut authorities, confirmation, &signing_key.verifying_key()).unwrap();
        for authority in &authorities {
            assert!(
                LifecycleAuthority::peek_offboarding_receipt(
                    root.path(),
                    &authority.checkpoint().plan.target_id
                )
                .unwrap()
                .is_none(),
                "reset offboard without erasure must not write receipt.json"
            );
        }
        for authority in &mut authorities {
            authority
                .record_check(LifecycleRequirementCheckV1 {
                    schema_version: 1,
                    check_id: "erasure".into(),
                    target_id: authority.checkpoint().plan.target_id.clone(),
                    expected: "erased".into(),
                    observed: "erased".into(),
                    status: mackes_mesh_types::lifecycle::LifecycleCheckStatus::Pass,
                    required: true,
                    evidence_digest_hex: "e".repeat(64),
                    warning: None,
                    generation: 1,
                })
                .unwrap();
        }
        persist_completed_offboarding_receipts(&authorities).unwrap();
        for target in ["seat-15", "seat-16"] {
            let receipt = LifecycleAuthority::peek_offboarding_receipt(root.path(), target)
                .unwrap()
                .expect("durable wipe receipt after observed erasure");
            assert!(receipt.completed);
            assert!(receipt.retained_resources.is_empty());
            assert_eq!(receipt.target_id, target);
        }
        assert!(
            LifecycleAuthority::resume(root.path(), "seat-15").is_err(),
            "reset receipt persist must not drop the mutation lock"
        );
        for authority in authorities {
            authority.finish().unwrap();
        }
    }

    #[test]
    fn fleet_report_does_not_claim_success_with_blocking_readiness_check() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        authority
            .record_check(LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "identity".into(),
                target_id: "seat-15".into(),
                expected: "present".into(),
                observed: "missing".into(),
                status: mackes_mesh_types::lifecycle::LifecycleCheckStatus::Unknown,
                required: true,
                evidence_digest_hex: "1".repeat(64),
                warning: Some("not probed".into()),
                generation: 1,
            })
            .unwrap();
        authority.complete_step(0).unwrap();
        authority.complete_step(1).unwrap();
        let report = fleet_report("request-1", 1, &[authority.checkpoint().clone()]).unwrap();
        assert_eq!(report.phase, LifecyclePhase::WaitingForOperator);
        authority.finish().unwrap();
    }

    #[test]
    fn checks_are_bound_to_checkpoint_target_and_generation() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(root.path(), plan()).unwrap();
        let check = LifecycleRequirementCheckV1 {
            schema_version: 1,
            check_id: "identity".into(),
            target_id: "seat-15".into(),
            expected: "present".into(),
            observed: "missing".into(),
            status: mackes_mesh_types::lifecycle::LifecycleCheckStatus::Unknown,
            required: true,
            evidence_digest_hex: "e".repeat(64),
            warning: Some("identity absent".into()),
            generation: 1,
        };
        authority.record_check(check.clone()).unwrap();
        assert_eq!(authority.checkpoint().checks.len(), 1);
        assert!(matches!(
            authority.run_next(|_| Ok(())),
            Err(LifecycleAuthorityError::InvalidTransition(
                "blocking requirement check"
            ))
        ));
        assert!(matches!(
            authority.record_check(check),
            Err(LifecycleAuthorityError::InvalidTransition(
                "duplicate check"
            ))
        ));
        authority.finish().unwrap();
    }
}
