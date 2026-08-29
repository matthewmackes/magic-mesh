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
    CommissioningCapsuleV1, FleetLifecycleReportV1, LifecycleArtifactSelectionV1,
    LifecycleConfirmationAction, LifecycleConfirmationV1, LifecycleCorrectionPlanV1,
    LifecycleIntentKind, LifecyclePhase, LifecyclePlanV1, LifecycleProgressV1,
    LifecycleRequirementCheckV1, OffboardingReceiptV1, SeatReadinessV1,
};
use serde::{Deserialize, Serialize};

const CHECKPOINT: &str = "checkpoint.json";
const JOURNAL: &str = "journal.jsonl";
const LOCK: &str = "lifecycle.lock";
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
    let report = FleetLifecycleReportV1 {
        schema_version: checkpoints[0].plan.schema_version,
        request_id: request_id.to_owned(),
        generation,
        phase,
        target_count,
        succeeded,
        failed,
        signature_hex: String::new(),
    };
    report
        .validate()
        .map_err(|_| LifecycleAuthorityError::InvalidPlan("fleet report"))?;
    Ok(report)
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
        &self,
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
        Ok(())
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
                let mut failed = self.checkpoint.progress.clone();
                failed.phase = LifecyclePhase::Failed;
                self.update(failed)?;
                Err(LifecycleAuthorityError::StepFailed(error))
            }
        }
    }

    /// Project the signed-boundary input for an offboarding receipt only after
    /// every declared step has succeeded. Signing is performed by the caller's
    /// governed evidence boundary; this method never invents a signature.
    pub fn offboarding_receipt(&self) -> Result<OffboardingReceiptV1, LifecycleAuthorityError> {
        if self.checkpoint.plan.intent != LifecycleIntentKind::Offboard {
            return Err(LifecycleAuthorityError::InvalidTransition("not offboard"));
        }
        if self.checkpoint.progress.phase != LifecyclePhase::Succeeded {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "offboard incomplete",
            ));
        }
        if self.checkpoint.confirmation.is_none() {
            return Err(LifecycleAuthorityError::InvalidTransition(
                "offboard confirmation missing",
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
        let correction = LifecycleCorrectionPlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            generation: 1,
            corrections: vec![mackes_mesh_types::lifecycle::LifecycleCorrectionV1 {
                check_id: "mesh".into(),
                step: "mesh".into(),
                reason: "enroll target".into(),
                prerequisites: Vec::new(),
            }],
            edges: Vec::new(),
            rollback_forbidden: true,
        };
        authority.admit_correction_plan(correction).unwrap();
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
        let receipt = authority.offboarding_receipt().unwrap();
        assert!(receipt.completed);
        assert!(receipt.retained_resources.is_empty());
        authority.finish().unwrap();
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
