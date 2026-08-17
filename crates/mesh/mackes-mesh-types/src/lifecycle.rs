//! WL-FUNC-023 — the bounded public lifecycle intent contract.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub const LIFECYCLE_CONTRACT_SCHEMA_VERSION: u16 = 1;
pub const LIFECYCLE_INTENT_TOPIC: &str = "action/lifecycle/intent";
pub const MAX_LIFECYCLE_INTENT_BYTES: usize = 64 * 1024;
pub const MAX_LIFECYCLE_IDENTIFIER_BYTES: usize = 128;
pub const MAX_LIFECYCLE_COLLECTION_ITEMS: usize = 256;

/// Decode a lifecycle message only after enforcing its transport bound.  All
/// public lifecycle envelopes should use this entry point rather than an
/// unbounded `serde_json::from_slice`.
pub fn decode_bounded<T: DeserializeOwned>(payload: &[u8]) -> Result<T, LifecycleIntentError> {
    if payload.len() > MAX_LIFECYCLE_INTENT_BYTES {
        return Err(LifecycleIntentError::PayloadTooLarge);
    }
    serde_json::from_slice(payload).map_err(|_| LifecycleIntentError::InvalidField("payload"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleIntentKind {
    Onboard,
    Upgrade,
    VerifyAndCorrect,
    Offboard,
    ResetAndOnboard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleStepKind {
    Identity,
    Packages,
    Configuration,
    Mesh,
    Compute,
    Ui,
    Hardware,
    Verify,
    Offboard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleConfirmationAction {
    Offboard,
    Reset,
    InstallUnsigned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleConfirmationV1 {
    pub schema_version: u16,
    pub session_id: String,
    pub action: LifecycleConfirmationAction,
    pub target_count: u32,
    pub scope_digest_hex: String,
    pub phrase: String,
    pub generation: u64,
    pub key_id: String,
    pub signature_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommissioningCapsuleV1 {
    pub schema_version: u16,
    pub capsule_id: String,
    pub target_id: String,
    pub expires_at_ms: i64,
    pub bootstrap_digest_hex: String,
    pub one_time: bool,
    pub key_id: String,
    pub signature_hex: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleArtifactChannel {
    Stable,
    Candidate,
    Dev,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleArtifactSelectionV1 {
    pub schema_version: u16,
    pub selection_id: String,
    pub target_id: String,
    pub channel: LifecycleArtifactChannel,
    pub artifact_digest_hex: String,
    pub source_revision: String,
    pub signed: bool,
    pub unverified_build: bool,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleCheckStatus {
    Pass,
    Warn,
    Fail,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleRequirementCheckV1 {
    pub schema_version: u16,
    pub check_id: String,
    pub target_id: String,
    pub expected: String,
    pub observed: String,
    pub status: LifecycleCheckStatus,
    pub required: bool,
    pub evidence_digest_hex: String,
    pub warning: Option<String>,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleCorrectionV1 {
    pub check_id: String,
    pub step: String,
    pub reason: String,
    #[serde(default)]
    pub prerequisites: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleCorrectionPlanV1 {
    pub schema_version: u16,
    pub request_id: String,
    pub target_id: String,
    pub generation: u64,
    pub corrections: Vec<LifecycleCorrectionV1>,
    #[serde(default)]
    pub edges: Vec<(String, String)>,
    pub rollback_forbidden: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleUpgradeBindingV1 {
    pub schema_version: u16,
    pub target_id: String,
    pub current_version: String,
    pub target_version: String,
    pub target_artifact_digest_hex: String,
    pub source_revision: String,
    pub generation: u64,
}

impl LifecycleUpgradeBindingV1 {
    pub fn validate(&self) -> Result<(), LifecycleIntentError> {
        validate_common(self.schema_version, self.generation, &[("target_id", &self.target_id), ("current_version", &self.current_version), ("target_version", &self.target_version), ("source_revision", &self.source_revision)])?;
        let current = parse_version(&self.current_version).ok_or(LifecycleIntentError::InvalidField("current_version"))?;
        let target = parse_version(&self.target_version).ok_or(LifecycleIntentError::InvalidField("target_version"))?;
        if target <= current
            || self.target_artifact_digest_hex.len() != 64
            || !self.target_artifact_digest_hex.chars().all(|c| c.is_ascii_hexdigit())
        {
            return Err(LifecycleIntentError::InvalidField("upgrade_binding"));
        }
        Ok(())
    }
}

fn parse_version(value: &str) -> Option<[u64; 3]> {
    let mut parts = value.split('.');
    let parsed = [parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?];
    (parts.next().is_none()).then_some(parsed)
}

impl LifecycleCorrectionPlanV1 {
    pub fn validate(&self) -> Result<(), LifecycleIntentError> {
        validate_common(self.schema_version, self.generation, &[("request_id", &self.request_id), ("target_id", &self.target_id)])?;
        if self.corrections.is_empty()
            || self.corrections.len() > MAX_LIFECYCLE_COLLECTION_ITEMS
            || !self.rollback_forbidden
            || self.corrections.iter().any(|correction| {
                correction.check_id.is_empty()
                    || correction.check_id.len() > MAX_LIFECYCLE_IDENTIFIER_BYTES
                    || LifecycleStepKind::parse(&correction.step).is_none()
                    || correction.reason.is_empty()
                    || correction.reason.len() > 1024
                    || correction.prerequisites.len() > MAX_LIFECYCLE_COLLECTION_ITEMS
            })
        {
            return Err(LifecycleIntentError::InvalidField("correction_plan"));
        }
        let ids = self.corrections.iter().map(|c| c.check_id.as_str()).collect::<std::collections::HashSet<_>>();
        if self.edges.len() > MAX_LIFECYCLE_COLLECTION_ITEMS
            || self.edges.iter().any(|(from, to)| from == to || !ids.contains(from.as_str()) || !ids.contains(to.as_str()))
        {
            return Err(LifecycleIntentError::InvalidField("correction_edges"));
        }
        // Kahn's algorithm rejects cycles and makes execution order explicit.
        let mut indegree = ids.iter().map(|id| (*id, 0usize)).collect::<std::collections::HashMap<_, _>>();
        for (_, to) in &self.edges { *indegree.get_mut(to.as_str()).unwrap() += 1; }
        let mut ready = indegree.iter().filter_map(|(id, count)| (*count == 0).then_some(*id)).collect::<Vec<_>>();
        let mut visited = 0usize;
        while let Some(id) = ready.pop() {
            visited += 1;
            for (from, to) in &self.edges { if from == id { let entry = indegree.get_mut(to.as_str()).unwrap(); *entry -= 1; if *entry == 0 { ready.push(to.as_str()); } } }
        }
        if visited != ids.len() { return Err(LifecycleIntentError::InvalidField("correction_cycle")); }
        Ok(())
    }
}

impl LifecycleRequirementCheckV1 {
    pub fn blocks_progress(&self) -> bool {
        self.required && matches!(self.status, LifecycleCheckStatus::Fail | LifecycleCheckStatus::Unknown)
    }

    pub fn validate(&self) -> Result<(), LifecycleIntentError> {
        validate_common(self.schema_version, self.generation, &[("check_id", &self.check_id), ("target_id", &self.target_id)])?;
        if self.expected.is_empty()
            || self.expected.len() > 1024
            || self.observed.len() > 1024
            || self.evidence_digest_hex.len() != 64
            || !self.evidence_digest_hex.chars().all(|c| c.is_ascii_hexdigit())
            || matches!(self.status, LifecycleCheckStatus::Warn | LifecycleCheckStatus::Unknown) && self.warning.as_deref().unwrap_or("").is_empty()
        {
            return Err(LifecycleIntentError::InvalidField("requirement_check"));
        }
        Ok(())
    }
}

impl LifecycleArtifactSelectionV1 {
    pub fn validate(&self) -> Result<(), LifecycleIntentError> {
        validate_common(self.schema_version, self.generation, &[("selection_id", &self.selection_id), ("target_id", &self.target_id), ("source_revision", &self.source_revision)])?;
        if self.artifact_digest_hex.len() != 64
            || !self.artifact_digest_hex.chars().all(|c| c.is_ascii_hexdigit())
            || (self.signed == self.unverified_build)
        {
            return Err(LifecycleIntentError::InvalidField("artifact_admission"));
        }
        Ok(())
    }
}

impl CommissioningCapsuleV1 {
    const SIGNING_DOMAIN: &'static str = "magic-mesh:commissioning-capsule:v1";

    fn signing_bytes(&self) -> Vec<u8> {
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}",
            Self::SIGNING_DOMAIN,
            self.schema_version,
            self.capsule_id,
            self.target_id,
            self.expires_at_ms,
            self.bootstrap_digest_hex,
            self.one_time,
            self.key_id,
        )
        .into_bytes()
    }

    pub fn sign(mut self, key_id: impl Into<String>, signing_key: &SigningKey) -> Self {
        self.key_id = key_id.into();
        self.signature_hex.clear();
        self.signature_hex = encode_hex(&signing_key.sign(&self.signing_bytes()).to_bytes());
        self
    }

    pub fn validate_at(&self, now_ms: i64) -> Result<(), LifecycleIntentError> {
        validate_common(self.schema_version, self.expires_at_ms.max(1) as u64, &[("capsule_id", &self.capsule_id), ("target_id", &self.target_id), ("key_id", &self.key_id)])?;
        if self.expires_at_ms <= now_ms
            || self.bootstrap_digest_hex.len() != 64
            || !self.bootstrap_digest_hex.chars().all(|c| c.is_ascii_hexdigit())
            || self.signature_hex.len() != 128
            || !self.signature_hex.chars().all(|c| c.is_ascii_hexdigit())
            || !self.one_time
        {
            return Err(LifecycleIntentError::InvalidField("capsule_bounds"));
        }
        Ok(())
    }

    pub fn verify_at(&self, now_ms: i64, verifying_key: &VerifyingKey) -> Result<(), LifecycleIntentError> {
        self.validate_at(now_ms)?;
        let bytes = decode_hex_64(&self.signature_hex).ok_or(LifecycleIntentError::InvalidField("signature_hex"))?;
        verifying_key.verify(&self.signing_bytes(), &Signature::from_bytes(&bytes)).map_err(|_| LifecycleIntentError::InvalidField("signature_hex"))
    }
}

impl LifecycleConfirmationV1 {
    const SIGNING_DOMAIN: &'static str = "magic-mesh:lifecycle-confirmation:v1";

    fn signing_bytes(&self) -> Vec<u8> {
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}",
            Self::SIGNING_DOMAIN,
            self.schema_version,
            self.session_id,
            serde_json::to_string(&self.action).expect("closed enum serializes"),
            self.target_count,
            self.scope_digest_hex,
            self.phrase,
            self.generation,
            self.key_id,
        )
        .into_bytes()
    }

    pub fn sign(mut self, key_id: impl Into<String>, signing_key: &SigningKey) -> Self {
        self.key_id = key_id.into();
        self.signature_hex = encode_hex(&signing_key.sign(&self.signing_bytes()).to_bytes());
        self
    }

    pub fn verify(&self, verifying_key: &VerifyingKey) -> Result<(), LifecycleIntentError> {
        self.validate()?;
        let bytes = decode_hex_64(&self.signature_hex)
            .ok_or(LifecycleIntentError::InvalidField("signature_hex"))?;
        verifying_key
            .verify(&self.signing_bytes(), &Signature::from_bytes(&bytes))
            .map_err(|_| LifecycleIntentError::InvalidField("signature_hex"))
    }

    pub fn expected_phrase(action: LifecycleConfirmationAction, target_count: u32) -> String {
        match action {
            LifecycleConfirmationAction::Offboard => format!("FORCE OFFBOARD {target_count} SYSTEMS"),
            LifecycleConfirmationAction::Reset => format!("WIPE {target_count} SYSTEMS"),
            LifecycleConfirmationAction::InstallUnsigned => format!("INSTALL UNSIGNED {target_count} SYSTEMS"),
        }
    }

    pub fn validate(&self) -> Result<(), LifecycleIntentError> {
        validate_common(self.schema_version, self.generation, &[("session_id", &self.session_id)])?;
        if self.target_count == 0
            || self.scope_digest_hex.len() != 64
            || !self.scope_digest_hex.chars().all(|c| c.is_ascii_hexdigit())
        {
            return Err(LifecycleIntentError::InvalidField("scope_digest_hex"));
        }
        if self.key_id.is_empty() || self.key_id.len() > MAX_LIFECYCLE_IDENTIFIER_BYTES {
            return Err(LifecycleIntentError::InvalidField("key_id"));
        }
        if self.signature_hex.len() != 128 || !self.signature_hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(LifecycleIntentError::InvalidField("signature_hex"));
        }
        if self.phrase != Self::expected_phrase(self.action, self.target_count) {
            return Err(LifecycleIntentError::InvalidField("phrase"));
        }
        Ok(())
    }
}

impl LifecycleStepKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Packages => "packages",
            Self::Configuration => "configuration",
            Self::Mesh => "mesh",
            Self::Compute => "compute",
            Self::Ui => "ui",
            Self::Hardware => "hardware",
            Self::Verify => "verify",
            Self::Offboard => "offboard",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "identity" => Self::Identity,
            "packages" => Self::Packages,
            "configuration" => Self::Configuration,
            "mesh" => Self::Mesh,
            "compute" => Self::Compute,
            "ui" => Self::Ui,
            "hardware" => Self::Hardware,
            "verify" => Self::Verify,
            "offboard" => Self::Offboard,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleIntentV1 {
    pub schema_version: u16,
    pub request_id: String,
    pub target_id: String,
    pub intent: LifecycleIntentKind,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardOffboardSessionV1 {
    pub schema_version: u16,
    pub session_id: String,
    pub operator_id: String,
    pub intent: LifecycleIntentKind,
    pub target_ids: Vec<String>,
    pub generation: u64,
    pub phase: LifecyclePhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecyclePhase {
    Planned,
    Running,
    WaitingForOperator,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecyclePlanV1 {
    pub schema_version: u16,
    pub request_id: String,
    pub target_id: String,
    pub intent: LifecycleIntentKind,
    pub generation: u64,
    pub steps: Vec<String>,
}

/// Canonical ownership map for the readiness surface.  Providers may add
/// checks, but they may not silently move a requirement between lifecycle
/// stages or claim readiness from target activity alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleBaselineEntryV1 {
    pub schema_version: u16,
    pub requirement_id: String,
    pub owner_step: LifecycleStepKind,
    pub required: bool,
    pub provider: String,
    pub critical: bool,
    #[serde(default)]
    pub prerequisites: Vec<String>,
    pub correction_step: LifecycleStepKind,
}

pub fn canonical_lifecycle_baseline() -> Vec<LifecycleBaselineEntryV1> {
    [
        ("packages", LifecycleStepKind::Packages),
        ("units", LifecycleStepKind::Configuration),
        ("configuration", LifecycleStepKind::Configuration),
        ("mesh_identity", LifecycleStepKind::Mesh),
        ("compute", LifecycleStepKind::Compute),
        ("ui", LifecycleStepKind::Ui),
        ("hardware", LifecycleStepKind::Hardware),
        ("verification", LifecycleStepKind::Verify),
    ]
    .into_iter()
        .map(|(requirement_id, owner_step)| LifecycleBaselineEntryV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            requirement_id: requirement_id.into(),
            owner_step,
            required: true,
            provider: "mackesd".into(),
            critical: true,
            prerequisites: Vec::new(),
            correction_step: owner_step,
        })
    .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleProgressV1 {
    pub schema_version: u16,
    pub request_id: String,
    pub target_id: String,
    pub generation: u64,
    pub phase: LifecyclePhase,
    pub completed_steps: u32,
    pub total_steps: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatReadinessV1 {
    pub schema_version: u16,
    pub target_id: String,
    pub generation: u64,
    pub ready: bool,
    pub missing_requirements: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OffboardingReceiptV1 {
    pub schema_version: u16,
    pub request_id: String,
    pub target_id: String,
    pub generation: u64,
    pub completed: bool,
    pub retained_resources: Vec<String>,
    #[serde(default)]
    pub signature_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetLifecycleReportV1 {
    pub schema_version: u16,
    pub request_id: String,
    pub generation: u64,
    pub phase: LifecyclePhase,
    pub target_count: u32,
    pub succeeded: u32,
    pub failed: u32,
    #[serde(default)]
    pub signature_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleIntentError {
    PayloadTooLarge,
    UnsupportedSchema(u16),
    InvalidField(&'static str),
    InvalidNumber(&'static str),
}

impl LifecycleIntentV1 {
    pub fn default_steps(&self) -> Vec<String> {
        let steps: &[LifecycleStepKind] = match self.intent {
            LifecycleIntentKind::Onboard => &[
                LifecycleStepKind::Identity,
                LifecycleStepKind::Packages,
                LifecycleStepKind::Configuration,
                LifecycleStepKind::Mesh,
                LifecycleStepKind::Compute,
                LifecycleStepKind::Ui,
                LifecycleStepKind::Hardware,
                LifecycleStepKind::Verify,
            ],
            LifecycleIntentKind::ResetAndOnboard => &[
                LifecycleStepKind::Offboard,
                LifecycleStepKind::Identity,
                LifecycleStepKind::Packages,
                LifecycleStepKind::Configuration,
                LifecycleStepKind::Mesh,
                LifecycleStepKind::Compute,
                LifecycleStepKind::Ui,
                LifecycleStepKind::Hardware,
                LifecycleStepKind::Verify,
            ],
            LifecycleIntentKind::Upgrade => &[
                LifecycleStepKind::Packages,
                LifecycleStepKind::Configuration,
                LifecycleStepKind::Verify,
            ],
            LifecycleIntentKind::VerifyAndCorrect => &[
                LifecycleStepKind::Verify,
                LifecycleStepKind::Configuration,
                LifecycleStepKind::Mesh,
                LifecycleStepKind::Verify,
            ],
            LifecycleIntentKind::Offboard => &[LifecycleStepKind::Offboard, LifecycleStepKind::Verify],
        };
        steps.iter().map(|step| step.as_str().to_string()).collect()
    }

    pub fn validate(&self) -> Result<(), LifecycleIntentError> {
        if self.schema_version != LIFECYCLE_CONTRACT_SCHEMA_VERSION {
            return Err(LifecycleIntentError::UnsupportedSchema(self.schema_version));
        }
        for (field, value) in [("request_id", &self.request_id), ("target_id", &self.target_id)] {
            if value.is_empty() || value.len() > MAX_LIFECYCLE_IDENTIFIER_BYTES {
                return Err(LifecycleIntentError::InvalidField(field));
            }
            if !value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
            {
                return Err(LifecycleIntentError::InvalidField(field));
            }
        }
        if self.generation == 0 {
            return Err(LifecycleIntentError::InvalidNumber("generation"));
        }
        Ok(())
    }

    pub fn from_json(body: &str) -> Result<Self, LifecycleIntentError> {
        if body.len() > MAX_LIFECYCLE_INTENT_BYTES {
            return Err(LifecycleIntentError::PayloadTooLarge);
        }
        let intent: Self = decode_bounded(body.as_bytes())?;
        intent.validate()?;
        Ok(intent)
    }
}

impl OnboardOffboardSessionV1 {
    pub fn validate(&self) -> Result<(), LifecycleIntentError> {
        validate_common(
            self.schema_version,
            self.generation,
            &[("session_id", &self.session_id), ("operator_id", &self.operator_id)],
        )?;
        if self.target_ids.is_empty()
            || self.target_ids.len() > 256
            || self.target_ids.iter().any(|target| {
                target.is_empty()
                    || target.len() > MAX_LIFECYCLE_IDENTIFIER_BYTES
                    || !target.chars().all(|c| {
                        c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':')
                    })
            })
        {
            return Err(LifecycleIntentError::InvalidField("target_ids"));
        }
        Ok(())
    }
}

fn validate_common(schema_version: u16, generation: u64, fields: &[(&'static str, &str)]) -> Result<(), LifecycleIntentError> {
    if schema_version != LIFECYCLE_CONTRACT_SCHEMA_VERSION {
        return Err(LifecycleIntentError::UnsupportedSchema(schema_version));
    }
    for (field, value) in fields {
        if value.is_empty() || value.len() > MAX_LIFECYCLE_IDENTIFIER_BYTES || !value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':')) {
            return Err(LifecycleIntentError::InvalidField(field));
        }
    }
    if generation == 0 {
        return Err(LifecycleIntentError::InvalidNumber("generation"));
    }
    Ok(())
}

impl LifecyclePlanV1 {
    pub fn validate(&self) -> Result<(), LifecycleIntentError> {
        validate_common(self.schema_version, self.generation, &[("request_id", &self.request_id), ("target_id", &self.target_id)])?;
        if self.steps.is_empty() || self.steps.len() > 256 || self.steps.iter().any(|step| step.is_empty() || step.len() > MAX_LIFECYCLE_IDENTIFIER_BYTES || LifecycleStepKind::parse(step).is_none()) {
            return Err(LifecycleIntentError::InvalidField("steps"));
        }
        Ok(())
    }
}

impl LifecycleBaselineEntryV1 {
    pub fn validate(&self) -> Result<(), LifecycleIntentError> {
        validate_common(self.schema_version, 1, &[("requirement_id", &self.requirement_id), ("provider", &self.provider)])?;
        if self.prerequisites.len() > MAX_LIFECYCLE_COLLECTION_ITEMS
            || self.prerequisites.iter().any(|item| item.is_empty() || item.len() > MAX_LIFECYCLE_IDENTIFIER_BYTES)
        {
            return Err(LifecycleIntentError::InvalidField("baseline_prerequisites"));
        }
        Ok(())
    }
}

impl LifecycleProgressV1 {
    pub fn validate(&self) -> Result<(), LifecycleIntentError> {
        validate_common(self.schema_version, self.generation, &[("request_id", &self.request_id), ("target_id", &self.target_id)])?;
        if self.total_steps == 0 || self.completed_steps > self.total_steps {
            return Err(LifecycleIntentError::InvalidNumber("steps"));
        }
        Ok(())
    }
}

impl SeatReadinessV1 {
    pub fn validate(&self) -> Result<(), LifecycleIntentError> {
        validate_common(self.schema_version, self.generation, &[("target_id", &self.target_id)])?;
        if self.ready && !self.missing_requirements.is_empty() {
            return Err(LifecycleIntentError::InvalidField("missing_requirements"));
        }
        if self.missing_requirements.len() > 256 || self.missing_requirements.iter().any(|item| item.is_empty() || item.len() > MAX_LIFECYCLE_IDENTIFIER_BYTES) {
            return Err(LifecycleIntentError::InvalidField("missing_requirements"));
        }
        if self.warnings.len() > 256 || self.warnings.iter().any(|item| item.is_empty() || item.len() > 1024) {
            return Err(LifecycleIntentError::InvalidField("warnings"));
        }
        Ok(())
    }
}

impl OffboardingReceiptV1 {
    const SIGNING_DOMAIN: &'static str = "magic-mesh:offboarding-receipt:v1";

    fn signing_bytes(&self) -> Vec<u8> {
        format!("{}|{}|{}|{}|{}|{}|{}", Self::SIGNING_DOMAIN, self.schema_version,
            self.request_id, self.target_id, self.generation, self.completed,
            serde_json::to_string(&self.retained_resources).expect("receipt fields serialize"))
            .into_bytes()
    }

    pub fn sign(mut self, signing_key: &SigningKey) -> Self {
        self.signature_hex = encode_hex(&signing_key.sign(&self.signing_bytes()).to_bytes());
        self
    }

    pub fn verify(&self, verifying_key: &VerifyingKey) -> Result<(), LifecycleIntentError> {
        self.validate()?;
        let signature = decode_hex_64(&self.signature_hex)
            .ok_or(LifecycleIntentError::InvalidField("signature_hex"))?;
        verifying_key.verify(&self.signing_bytes(), &Signature::from_bytes(&signature))
            .map_err(|_| LifecycleIntentError::InvalidField("signature_hex"))
    }

    pub fn validate(&self) -> Result<(), LifecycleIntentError> {
        validate_common(self.schema_version, self.generation, &[("request_id", &self.request_id), ("target_id", &self.target_id)])?;
        // A completed offboard receipt is an erasure assertion, not a waiver:
        // any retained reusable state makes the operation incomplete.
        if !self.completed || !self.retained_resources.is_empty() {
            return Err(LifecycleIntentError::InvalidField("retained_resources"));
        }
        if !self.signature_hex.is_empty()
            && (self.signature_hex.len() != 128 || !self.signature_hex.chars().all(|c| c.is_ascii_hexdigit()))
        {
            return Err(LifecycleIntentError::InvalidField("signature_hex"));
        }
        Ok(())
    }
}

impl FleetLifecycleReportV1 {
    const SIGNING_DOMAIN: &'static str = "magic-mesh:fleet-lifecycle-report:v1";

    fn signing_bytes(&self) -> Vec<u8> {
        format!("{}|{}|{}|{}|{}|{}|{}|{}", Self::SIGNING_DOMAIN, self.schema_version,
            self.request_id, self.generation, serde_json::to_string(&self.phase).expect("phase serializes"),
            self.target_count, self.succeeded, self.failed).into_bytes()
    }

    pub fn sign(mut self, signing_key: &SigningKey) -> Self {
        self.signature_hex = encode_hex(&signing_key.sign(&self.signing_bytes()).to_bytes());
        self
    }

    pub fn verify(&self, verifying_key: &VerifyingKey) -> Result<(), LifecycleIntentError> {
        self.validate()?;
        let signature = decode_hex_64(&self.signature_hex)
            .ok_or(LifecycleIntentError::InvalidField("signature_hex"))?;
        verifying_key.verify(&self.signing_bytes(), &Signature::from_bytes(&signature))
            .map_err(|_| LifecycleIntentError::InvalidField("signature_hex"))
    }

    pub fn validate(&self) -> Result<(), LifecycleIntentError> {
        validate_common(self.schema_version, self.generation, &[("request_id", &self.request_id)])?;
        if self.target_count == 0 || self.succeeded > self.target_count || self.failed > self.target_count || self.succeeded.saturating_add(self.failed) > self.target_count {
            return Err(LifecycleIntentError::InvalidNumber("target_count"));
        }
        if !self.signature_hex.is_empty()
            && (self.signature_hex.len() != 128 || !self.signature_hex.chars().all(|c| c.is_ascii_hexdigit()))
        {
            return Err(LifecycleIntentError::InvalidField("signature_hex"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent() -> LifecycleIntentV1 {
        LifecycleIntentV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::Onboard,
            generation: 1,
        }
    }

    #[test]
    fn intent_round_trips_and_validates() {
        let body = serde_json::to_string(&intent()).unwrap();
        assert_eq!(LifecycleIntentV1::from_json(&body).unwrap(), intent());
        assert_eq!(intent().default_steps(), vec!["identity", "packages", "configuration", "mesh", "compute", "ui", "hardware", "verify"]);
    }

    #[test]
    fn reset_and_onboard_must_offboard_before_recommissioning() {
        let mut reset = intent();
        reset.intent = LifecycleIntentKind::ResetAndOnboard;
        let steps = reset.default_steps();
        assert_eq!(steps.first(), Some(&"offboard".to_string()));
        assert_eq!(steps.last(), Some(&"verify".to_string()));
        assert!(steps.contains(&"identity".to_string()));
    }

    #[test]
    fn canonical_baseline_owns_every_readiness_surface() {
        let baseline = canonical_lifecycle_baseline();
        assert_eq!(baseline.len(), 8);
        assert!(baseline.iter().all(|entry| entry.required && entry.validate().is_ok()));
        assert!(baseline.iter().any(|entry| entry.requirement_id == "mesh_identity" && entry.owner_step == LifecycleStepKind::Mesh));
    }

    #[test]
    fn intent_rejects_bad_schema_target_and_generation() {
        let mut value = intent();
        value.schema_version = 2;
        assert!(matches!(value.validate(), Err(LifecycleIntentError::UnsupportedSchema(2))));
        value = intent();
        value.target_id = "../seat".into();
        assert!(matches!(value.validate(), Err(LifecycleIntentError::InvalidField("target_id"))));
        value = intent();
        value.generation = 0;
        assert!(matches!(value.validate(), Err(LifecycleIntentError::InvalidNumber("generation"))));
    }

    #[test]
    fn plan_progress_and_readiness_reject_inconsistent_states() {
        let plan = LifecyclePlanV1 { schema_version: 1, request_id: "request-1".into(), target_id: "seat-15".into(), intent: LifecycleIntentKind::Upgrade, generation: 1, steps: vec!["packages".into(), "verify".into()] };
        assert!(plan.validate().is_ok());
        let progress = LifecycleProgressV1 { schema_version: 1, request_id: "request-1".into(), target_id: "seat-15".into(), generation: 1, phase: LifecyclePhase::Running, completed_steps: 3, total_steps: 2 };
        assert!(matches!(progress.validate(), Err(LifecycleIntentError::InvalidNumber("steps"))));
        let readiness = SeatReadinessV1 { schema_version: 1, target_id: "seat-15".into(), generation: 1, ready: true, missing_requirements: vec!["mesh_identity".into()], warnings: vec![] };
        assert!(matches!(readiness.validate(), Err(LifecycleIntentError::InvalidField("missing_requirements"))));

        let receipt = OffboardingReceiptV1 { schema_version: 1, request_id: "request-1".into(), target_id: "seat-15".into(), generation: 1, completed: true, retained_resources: vec![], signature_hex: String::new() };
        assert!(receipt.validate().is_ok());
        let retained = OffboardingReceiptV1 { retained_resources: vec!["identity".into()], ..receipt.clone() };
        assert!(matches!(retained.validate(), Err(LifecycleIntentError::InvalidField("retained_resources"))));
        let incomplete = OffboardingReceiptV1 { completed: false, ..receipt.clone() };
        assert!(matches!(incomplete.validate(), Err(LifecycleIntentError::InvalidField("retained_resources"))));
        let signing_key = SigningKey::from_bytes(&[14; 32]);
        assert!(receipt.sign(&signing_key).verify(&signing_key.verifying_key()).is_ok());
        let report = FleetLifecycleReportV1 { schema_version: 1, request_id: "request-1".into(), generation: 1, phase: LifecyclePhase::Succeeded, target_count: 2, succeeded: 2, failed: 1, signature_hex: String::new() };
        assert!(matches!(report.validate(), Err(LifecycleIntentError::InvalidNumber("target_count"))));
        let report = FleetLifecycleReportV1 { schema_version: 1, request_id: "request-1".into(), generation: 1, phase: LifecyclePhase::Succeeded, target_count: 2, succeeded: 2, failed: 0, signature_hex: String::new() };
        let key = SigningKey::from_bytes(&[15; 32]);
        assert!(report.sign(&key).verify(&key.verifying_key()).is_ok());
    }

    #[test]
    fn plan_rejects_unowned_step_names() {
        let mut plan = LifecyclePlanV1 { schema_version: 1, request_id: "request-1".into(), target_id: "seat-15".into(), intent: LifecycleIntentKind::Onboard, generation: 1, steps: vec!["identity".into()] };
        assert_eq!(LifecycleStepKind::Identity.as_str(), "identity");
        assert!(plan.validate().is_ok());
        plan.steps = vec!["invented_mutation".into()];
        assert!(matches!(plan.validate(), Err(LifecycleIntentError::InvalidField("steps"))));
    }

    #[test]
    fn session_binds_operator_intent_and_target_scope() {
        let session = OnboardOffboardSessionV1 {
            schema_version: 1,
            session_id: "session-1".into(),
            operator_id: "operator-1".into(),
            intent: LifecycleIntentKind::Offboard,
            target_ids: vec!["seat-15".into(), "seat-16".into()],
            generation: 1,
            phase: LifecyclePhase::Planned,
        };
        assert!(session.validate().is_ok());
        let mut invalid = session;
        invalid.target_ids = vec!["../seat".into()];
        assert!(matches!(invalid.validate(), Err(LifecycleIntentError::InvalidField("target_ids"))));
    }

    #[test]
    fn destructive_confirmation_binds_exact_scope_and_phrase() {
        let confirmation = LifecycleConfirmationV1 {
            schema_version: 1,
            session_id: "session-1".into(),
            action: LifecycleConfirmationAction::Offboard,
            target_count: 2,
            scope_digest_hex: "a".repeat(64),
            phrase: "FORCE OFFBOARD 2 SYSTEMS".into(),
            generation: 1,
            key_id: "lifecycle-authority-v1".into(),
            signature_hex: "0".repeat(128),
        };
        let signed = confirmation.sign("lifecycle-authority-v1", &SigningKey::from_bytes(&[3; 32]));
        assert!(signed.validate().is_ok());
        assert!(signed.verify(&SigningKey::from_bytes(&[3; 32]).verifying_key()).is_ok());
        assert_eq!(LifecycleConfirmationV1::expected_phrase(LifecycleConfirmationAction::Reset, 2), "WIPE 2 SYSTEMS");
        let mut invalid = signed;
        invalid.phrase = "FORCE OFFBOARD 1 SYSTEM".into();
        assert!(matches!(invalid.validate(), Err(LifecycleIntentError::InvalidField("phrase"))));
    }

    #[test]
    fn commissioning_capsule_is_target_bound_expiring_and_one_time() {
        let capsule = CommissioningCapsuleV1 {
            schema_version: 1,
            capsule_id: "capsule-1".into(),
            target_id: "seat-15".into(),
            expires_at_ms: 2_000,
            bootstrap_digest_hex: "b".repeat(64),
            one_time: true,
            key_id: "commissioning-v1".into(),
            signature_hex: String::new(),
        }.sign("commissioning-v1", &SigningKey::from_bytes(&[4; 32]));
        assert!(capsule.verify_at(1_000, &SigningKey::from_bytes(&[4; 32]).verifying_key()).is_ok());
        assert!(matches!(capsule.validate_at(2_000), Err(LifecycleIntentError::InvalidField("capsule_bounds"))));
        let mut replayable = capsule;
        replayable.one_time = false;
        assert!(matches!(replayable.validate_at(1_000), Err(LifecycleIntentError::InvalidField("capsule_bounds"))));
    }

    #[test]
    fn artifact_selection_makes_unsigned_state_explicit() {
        let selection = LifecycleArtifactSelectionV1 {
            schema_version: 1,
            selection_id: "selection-1".into(),
            target_id: "seat-15".into(),
            channel: LifecycleArtifactChannel::Dev,
            artifact_digest_hex: "c".repeat(64),
            source_revision: "revision-1".into(),
            signed: false,
            unverified_build: true,
            generation: 1,
        };
        assert!(selection.validate().is_ok());
        let mut invalid = selection;
        invalid.unverified_build = false;
        assert!(matches!(invalid.validate(), Err(LifecycleIntentError::InvalidField("artifact_admission"))));
    }

    #[test]
    fn requirement_check_requires_evidence_and_honest_warnings() {
        let check = LifecycleRequirementCheckV1 {
            schema_version: 1,
            check_id: "mesh-identity".into(),
            target_id: "seat-15".into(),
            expected: "enrolled".into(),
            observed: "missing".into(),
            status: LifecycleCheckStatus::Unknown,
            required: true,
            evidence_digest_hex: "d".repeat(64),
            warning: Some("enrollment input is absent".into()),
            generation: 1,
        };
        assert!(check.validate().is_ok());
        let mut invalid = check;
        invalid.warning = None;
        assert!(matches!(invalid.validate(), Err(LifecycleIntentError::InvalidField("requirement_check"))));
    }

    #[test]
    fn correction_plan_requires_declared_steps_and_forbids_rollback() {
        let plan = LifecycleCorrectionPlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            generation: 2,
            corrections: vec![LifecycleCorrectionV1 {
                check_id: "mesh-identity".into(),
                step: "mesh".into(),
                reason: "identity is absent".into(),
                prerequisites: Vec::new(),
            }],
            edges: Vec::new(),
            rollback_forbidden: true,
        };
        assert!(plan.validate().is_ok());
        let mut invalid = plan;
        invalid.rollback_forbidden = false;
        assert!(matches!(invalid.validate(), Err(LifecycleIntentError::InvalidField("correction_plan"))));
    }

    #[test]
    fn upgrade_binding_rejects_downgrades_and_unbound_artifacts() {
        let binding = LifecycleUpgradeBindingV1 {
            schema_version: 1,
            target_id: "seat-15".into(),
            current_version: "12.1.5".into(),
            target_version: "13.0.0".into(),
            target_artifact_digest_hex: "f".repeat(64),
            source_revision: "revision-1".into(),
            generation: 1,
        };
        assert!(binding.validate().is_ok());
        let mut invalid = binding;
        invalid.target_version = "12.1.4".into();
        assert!(matches!(invalid.validate(), Err(LifecycleIntentError::InvalidField("upgrade_binding"))));
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex_64(value: &str) -> Option<[u8; 64]> {
    if value.len() != 128 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = [0_u8; 64];
    for (index, slot) in bytes.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}
