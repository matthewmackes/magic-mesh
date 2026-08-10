//! Strict, bounded Surface Pro 5/6 enable-result wire contract.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::surface_hardware::SurfaceProGeneration;

/// Current Surface enable-result schema.
pub const SURFACE_ENABLE_RESULT_SCHEMA_VERSION: u64 = 1;
/// Maximum encoded result size.
pub const MAX_SURFACE_ENABLE_RESULT_WIRE_BYTES: usize = 64 * 1024;
/// Maximum age admitted by consumers.
pub const MAX_SURFACE_ENABLE_RESULT_AGE_MS: u64 = 90_000;
/// Maximum publisher/consumer wall-clock skew.
pub const MAX_SURFACE_ENABLE_RESULT_FUTURE_SKEW_MS: u64 = 5_000;
const MAX_ID_BYTES: usize = 128;
const MAX_MODEL_BYTES: usize = 64;
const MAX_REASON_BYTES: usize = 512;
const MAX_PROMPT_BYTES: usize = 2_048;
const MAX_STEPS: usize = 8;
const SHA1_FINGERPRINT_CHARS: usize = 59;

/// Contract admission failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceEnableContractError {
    /// Encoded body exceeded the fixed limit.
    Oversized,
    /// JSON was malformed, duplicated a key, or contained an unknown field.
    Malformed,
    /// A semantic field was invalid.
    Invalid(&'static str),
    /// Publication was stale or future-dated.
    Stale,
}

impl std::fmt::Display for SurfaceEnableContractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Oversized => f.write_str("Surface enable result exceeds the wire limit"),
            Self::Malformed => f.write_str("Surface enable result is malformed"),
            Self::Invalid(field) => write!(f, "invalid Surface enable field: {field}"),
            Self::Stale => f.write_str("Surface enable result is stale or future-dated"),
        }
    }
}

impl std::error::Error for SurfaceEnableContractError {}

/// Exact producer authority for this local result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceEnableSource {
    /// The node-local, action-authorized `surface_enable` worker.
    LocalSurfaceEnableWorker,
}

/// Closed activation targets admitted by the Pro 5/6 contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceEnableUnit {
    /// Package-owned iptsd udev activation.
    Iptsd,
}

/// Closed configuration targets admitted by the Pro 5/6 contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceEnableConfig {
    /// Fixed Surface Aggregator balanced profile.
    SamBalancedProfile,
}

/// Bounded outcome for one activation step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum SurfaceEnableStepOutcome {
    /// Applied by this request.
    Applied,
    /// Already in the requested state.
    AlreadyActive,
    /// A required production integration was unavailable.
    Gated {
        /// Bounded integration diagnostic.
        reason: String,
    },
    /// The fixed operation failed.
    Failed {
        /// Bounded failure diagnostic.
        reason: String,
    },
}

impl SurfaceEnableStepOutcome {
    fn validate(&self) -> Result<(), SurfaceEnableContractError> {
        match self {
            Self::Applied | Self::AlreadyActive => Ok(()),
            Self::Gated { reason } | Self::Failed { reason } => {
                validate_text(reason, MAX_REASON_BYTES, "step.reason")
            }
        }
    }
}

/// One closed unit activation record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceEnableUnitResult {
    /// Fixed unit target.
    pub unit: SurfaceEnableUnit,
    /// Result of activation.
    pub outcome: SurfaceEnableStepOutcome,
}

/// One closed configuration record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceEnableConfigResult {
    /// Fixed configuration target.
    pub config: SurfaceEnableConfig,
    /// Result of configuration.
    pub outcome: SurfaceEnableStepOutcome,
}

/// Bounded activation result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceEnableActivation {
    /// Unit results.
    pub units: Vec<SurfaceEnableUnitResult>,
    /// Configuration results.
    pub configs: Vec<SurfaceEnableConfigResult>,
}

impl SurfaceEnableActivation {
    fn validate(&self) -> Result<(), SurfaceEnableContractError> {
        if self.units.len() > MAX_STEPS || self.configs.len() > MAX_STEPS {
            return Err(SurfaceEnableContractError::Invalid("activation count"));
        }
        let mut units = BTreeSet::new();
        for row in &self.units {
            if !units.insert(row.unit) {
                return Err(SurfaceEnableContractError::Invalid("duplicate unit"));
            }
            row.outcome.validate()?;
        }
        let mut configs = BTreeSet::new();
        for row in &self.configs {
            if !configs.insert(row.config) {
                return Err(SurfaceEnableContractError::Invalid("duplicate config"));
            }
            row.outcome.validate()?;
        }
        Ok(())
    }
}

/// MOK posture after the enable action. Reboot is deliberately absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum SurfaceEnableMokState {
    /// Secure Boot does not require MOK enrollment.
    NotRequired,
    /// The fixed certificate is enrolled.
    Enrolled {
        /// Whether the fixed linux-surface module set is loaded.
        modules_loaded: bool,
    },
    /// The fixed certificate is proven pending; reboot belongs to host-state.
    AwaitingGovernedHostReboot {
        /// Exact bounded firmware guidance.
        firmware_prompt: String,
        /// Complete SHA-1 fingerprint exposed by mokutil for operator matching.
        key_fingerprint: String,
    },
    /// Posture could not be established.
    Undetermined {
        /// Bounded classification diagnostic.
        reason: String,
    },
}

impl SurfaceEnableMokState {
    fn validate(&self) -> Result<(), SurfaceEnableContractError> {
        match self {
            Self::NotRequired | Self::Enrolled { .. } => Ok(()),
            Self::AwaitingGovernedHostReboot {
                firmware_prompt,
                key_fingerprint,
            } => {
                validate_text(firmware_prompt, MAX_PROMPT_BYTES, "mok.firmware_prompt")?;
                validate_sha1_fingerprint(key_fingerprint)
            }
            Self::Undetermined { reason } => validate_text(reason, MAX_REASON_BYTES, "mok.reason"),
        }
    }
}

/// Closed refusal classes, with bounded operator-facing detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceEnableRefusal {
    /// Request contract was not admitted.
    Contract,
    /// Exact-body action authorization failed.
    Authorization,
    /// Local policy refused the operation.
    Policy,
}

/// Top-level action outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum SurfaceEnableOutcome {
    /// Enable action completed with honest per-step/MOK posture.
    Completed {
        /// Bounded activation records.
        activation: SurfaceEnableActivation,
        /// MOK posture without reboot authority.
        mok: SurfaceEnableMokState,
    },
    /// Enable action was refused before effects.
    Refused {
        /// Closed refusal class.
        code: SurfaceEnableRefusal,
        /// Bounded operator-facing diagnostic.
        reason: String,
    },
}

/// Versioned local Surface Pro 5/6 enable result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceEnableResult {
    /// Result schema version.
    pub schema_version: u64,
    /// Exact local node identity.
    pub node: String,
    /// Authorized request identity this result answers.
    pub request_id: String,
    /// Exact admitted model label.
    pub model: String,
    /// Exact admitted generation.
    pub generation: SurfaceProGeneration,
    /// Fixed producer authority.
    pub source: SurfaceEnableSource,
    /// Producer wall-clock time.
    pub published_at_ms: u64,
    /// Closed action result.
    pub outcome: SurfaceEnableOutcome,
}

impl SurfaceEnableResult {
    /// Validate all bounds and exact Pro 5/6 identity bindings.
    pub fn validate(&self) -> Result<(), SurfaceEnableContractError> {
        if self.schema_version != SURFACE_ENABLE_RESULT_SCHEMA_VERSION {
            return Err(SurfaceEnableContractError::Invalid("schema_version"));
        }
        validate_id(&self.node, "node")?;
        validate_id(&self.request_id, "request_id")?;
        validate_text(&self.model, MAX_MODEL_BYTES, "model")?;
        match (&*self.model, self.generation) {
            ("Surface Pro 5", SurfaceProGeneration::Pro5)
            | ("Surface Pro 6", SurfaceProGeneration::Pro6) => {}
            _ => return Err(SurfaceEnableContractError::Invalid("model generation")),
        }
        if self.published_at_ms == 0 {
            return Err(SurfaceEnableContractError::Invalid("published_at_ms"));
        }
        match &self.outcome {
            SurfaceEnableOutcome::Completed { activation, mok } => {
                activation.validate()?;
                mok.validate()
            }
            SurfaceEnableOutcome::Refused { reason, .. } => {
                validate_text(reason, MAX_REASON_BYTES, "refusal.reason")
            }
        }
    }

    /// Decode untrusted JSON with size, UTF-8, duplicate-key, unknown-field,
    /// semantic, source, and freshness admission.
    pub fn from_json_at(
        body: &[u8],
        expected_node: &str,
        expected_request_id: &str,
        now_ms: u64,
    ) -> Result<Self, SurfaceEnableContractError> {
        let value = Self::from_json_for_node_at(body, expected_node, now_ms)?;
        if value.request_id != expected_request_id {
            return Err(SurfaceEnableContractError::Invalid("request_id binding"));
        }
        Ok(value)
    }

    /// Decode for a local observation consumer that did not originate the
    /// action. This binds exact node/source/freshness/model and validates the
    /// embedded request identity without requiring the caller to preparse it.
    pub fn from_json_for_node_at(
        body: &[u8],
        expected_node: &str,
        now_ms: u64,
    ) -> Result<Self, SurfaceEnableContractError> {
        if body.len() > MAX_SURFACE_ENABLE_RESULT_WIRE_BYTES {
            return Err(SurfaceEnableContractError::Oversized);
        }
        let text = std::str::from_utf8(body).map_err(|_| SurfaceEnableContractError::Malformed)?;
        crate::workloads::reject_duplicate_json_keys(text)
            .map_err(|_| SurfaceEnableContractError::Malformed)?;
        let value: Self =
            serde_json::from_str(text).map_err(|_| SurfaceEnableContractError::Malformed)?;
        value.validate()?;
        if value.node != expected_node {
            return Err(SurfaceEnableContractError::Invalid("node binding"));
        }
        if value.source != SurfaceEnableSource::LocalSurfaceEnableWorker
            || value.published_at_ms
                > now_ms.saturating_add(MAX_SURFACE_ENABLE_RESULT_FUTURE_SKEW_MS)
            || now_ms.saturating_sub(value.published_at_ms) > MAX_SURFACE_ENABLE_RESULT_AGE_MS
        {
            return Err(SurfaceEnableContractError::Stale);
        }
        Ok(value)
    }

    /// Encode only after semantic and wire-size validation.
    pub fn to_json(&self) -> Result<String, SurfaceEnableContractError> {
        self.validate()?;
        let body =
            serde_json::to_string(self).map_err(|_| SurfaceEnableContractError::Malformed)?;
        if body.len() > MAX_SURFACE_ENABLE_RESULT_WIRE_BYTES {
            return Err(SurfaceEnableContractError::Oversized);
        }
        Ok(body)
    }
}

fn validate_id(value: &str, field: &'static str) -> Result<(), SurfaceEnableContractError> {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(SurfaceEnableContractError::Invalid(field));
    }
    Ok(())
}

fn validate_text(
    value: &str,
    max: usize,
    field: &'static str,
) -> Result<(), SurfaceEnableContractError> {
    if value.is_empty()
        || value.len() > max
        || value
            .chars()
            .any(|character| character == '\0' || (character.is_control() && character != '\n'))
    {
        return Err(SurfaceEnableContractError::Invalid(field));
    }
    Ok(())
}

fn validate_sha1_fingerprint(value: &str) -> Result<(), SurfaceEnableContractError> {
    if value.len() != SHA1_FINGERPRINT_CHARS
        || value.as_bytes().iter().enumerate().any(|(index, byte)| {
            if (index + 1) % 3 == 0 {
                *byte != b':'
            } else {
                !byte.is_ascii_hexdigit() || byte.is_ascii_lowercase()
            }
        })
    {
        return Err(SurfaceEnableContractError::Invalid("mok.key_fingerprint"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;

    fn fixture() -> SurfaceEnableResult {
        SurfaceEnableResult {
            schema_version: SURFACE_ENABLE_RESULT_SCHEMA_VERSION,
            node: "surface-6".into(),
            request_id: "01JENABLE".into(),
            model: "Surface Pro 6".into(),
            generation: SurfaceProGeneration::Pro6,
            source: SurfaceEnableSource::LocalSurfaceEnableWorker,
            published_at_ms: NOW,
            outcome: SurfaceEnableOutcome::Completed {
                activation: SurfaceEnableActivation {
                    units: vec![SurfaceEnableUnitResult {
                        unit: SurfaceEnableUnit::Iptsd,
                        outcome: SurfaceEnableStepOutcome::Applied,
                    }],
                    configs: vec![SurfaceEnableConfigResult {
                        config: SurfaceEnableConfig::SamBalancedProfile,
                        outcome: SurfaceEnableStepOutcome::AlreadyActive,
                    }],
                },
                mok: SurfaceEnableMokState::AwaitingGovernedHostReboot {
                    firmware_prompt:
                        "Use MOK Manager, then reboot through System → Power & Battery.".into(),
                    key_fingerprint: "01:23:45:67:89:AB:CD:EF:10:32:54:76:98:BA:DC:FE:11:22:33:44"
                        .into(),
                },
            },
        }
    }

    #[test]
    fn strict_round_trip_has_no_reboot_authority() {
        let value = fixture();
        let body = value.to_json().expect("encode bounded result");
        assert!(!body.contains("arm_token"));
        assert!(!body.contains("RebootArmed"));
        assert_eq!(
            SurfaceEnableResult::from_json_at(body.as_bytes(), "surface-6", "01JENABLE", NOW),
            Ok(value)
        );
        assert_eq!(
            SurfaceEnableResult::from_json_for_node_at(body.as_bytes(), "surface-6", NOW)
                .expect("admit local observation")
                .request_id,
            "01JENABLE"
        );
    }

    #[test]
    fn rejects_unknown_duplicate_oversize_and_non_utf8() {
        let body = fixture().to_json().unwrap();
        let unknown = body.replacen("{", "{\"unknown\":true,", 1);
        assert!(matches!(
            SurfaceEnableResult::from_json_at(unknown.as_bytes(), "surface-6", "01JENABLE", NOW),
            Err(SurfaceEnableContractError::Malformed)
        ));
        let duplicate = body.replacen(
            "\"schema_version\":1",
            "\"schema_version\":1,\"schema_version\":1",
            1,
        );
        assert!(matches!(
            SurfaceEnableResult::from_json_at(duplicate.as_bytes(), "surface-6", "01JENABLE", NOW),
            Err(SurfaceEnableContractError::Malformed)
        ));
        assert_eq!(
            SurfaceEnableResult::from_json_at(
                &vec![b' '; MAX_SURFACE_ENABLE_RESULT_WIRE_BYTES + 1],
                "surface-6",
                "01JENABLE",
                NOW
            ),
            Err(SurfaceEnableContractError::Oversized)
        );
        assert_eq!(
            SurfaceEnableResult::from_json_at(&[0xff], "surface-6", "01JENABLE", NOW),
            Err(SurfaceEnableContractError::Malformed)
        );
    }

    #[test]
    fn binds_exact_model_source_freshness_and_bounded_fields() {
        let body = fixture().to_json().unwrap();
        assert!(SurfaceEnableResult::from_json_at(
            body.as_bytes(),
            "foreign-node",
            "01JENABLE",
            NOW
        )
        .is_err());
        assert!(SurfaceEnableResult::from_json_at(
            body.as_bytes(),
            "surface-6",
            "foreign-request",
            NOW
        )
        .is_err());
        let mut value = fixture();
        value.model = "Surface Pro 5".into();
        assert!(value.validate().is_err());
        value = fixture();
        value.generation = SurfaceProGeneration::Unsupported;
        assert!(value.validate().is_err());
        value = fixture();
        value.published_at_ms = NOW - MAX_SURFACE_ENABLE_RESULT_AGE_MS - 1;
        let body = serde_json::to_vec(&value).unwrap();
        assert_eq!(
            SurfaceEnableResult::from_json_at(&body, "surface-6", "01JENABLE", NOW),
            Err(SurfaceEnableContractError::Stale)
        );
        value = fixture();
        if let SurfaceEnableOutcome::Completed { activation, .. } = &mut value.outcome {
            activation.units.push(activation.units[0].clone());
        }
        assert!(value.validate().is_err());
    }
}
