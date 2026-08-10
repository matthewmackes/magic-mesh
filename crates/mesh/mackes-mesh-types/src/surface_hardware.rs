//! Bounded Microsoft Surface Pro 5/6 hardware observation and action contracts.
//!
//! `mackesd` owns observation and effects; the shell renders these records and
//! publishes intent. Keeping the wire schema here prevents the two tiers from
//! maintaining byte-compatible private mirrors.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Current Surface hardware wire schema.
pub const SURFACE_HARDWARE_SCHEMA_VERSION: u64 = 1;
/// Maximum accepted encoded record size.
pub const MAX_SURFACE_WIRE_BYTES: usize = 64 * 1024;
/// Maximum identifier length.
pub const MAX_SURFACE_ID_BYTES: usize = 128;
/// Maximum product/model label length.
pub const MAX_SURFACE_MODEL_BYTES: usize = 256;
/// Maximum human-readable reason length.
pub const MAX_SURFACE_REASON_BYTES: usize = 1_024;
/// Maximum probe rows in one board.
pub const MAX_SURFACE_PROBE_ROWS: usize = 16;
/// Maximum firmware devices in one inventory.
pub const MAX_SURFACE_FIRMWARE_DEVICES: usize = 64;
/// Observation publications older than this are stale for interactive use.
pub const MAX_SURFACE_STATE_AGE_MS: u64 = 90_000;
/// Privileged action requests older than this are refused before effects.
pub const MAX_SURFACE_ACTION_AGE_MS: u64 = 30_000;
/// Small tolerated wall-clock skew for action publishers.
pub const MAX_SURFACE_ACTION_FUTURE_SKEW_MS: u64 = 5_000;
/// Exact local phrase an operator must type before one camera functional proof.
pub const SURFACE_CAMERA_PROOF_ARM_TOKEN: &str = "PROVE CAMERA";
/// Firmware-apply result schema. Version 2 explicitly replaces the former
/// private daemon JSON shape that carried unbounded free-form reasons.
pub const SURFACE_FIRMWARE_APPLY_RESULT_SCHEMA_VERSION: u64 = 2;

/// Contract admission failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceContractError {
    /// The record exceeds its byte ceiling.
    Oversized,
    /// JSON is malformed or contains a duplicate/unknown field.
    Malformed,
    /// A field violates a semantic bound.
    Invalid(&'static str),
    /// The request targets a different node.
    ForeignNode,
    /// The request timestamp is stale or implausibly future-dated.
    Stale,
}

impl std::fmt::Display for SurfaceContractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Oversized => formatter.write_str("Surface record exceeds the wire limit"),
            Self::Malformed => formatter.write_str("Surface record is malformed"),
            Self::Invalid(field) => write!(formatter, "invalid Surface field: {field}"),
            Self::ForeignNode => formatter.write_str("Surface action targets a different node"),
            Self::Stale => formatter.write_str("Surface action is stale or future-dated"),
        }
    }
}

impl std::error::Error for SurfaceContractError {}

/// Explicitly supported Surface generations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceProGeneration {
    /// Surface Pro (5th generation / 2017).
    Pro5,
    /// Surface Pro 6.
    Pro6,
    /// A genuine Surface that is detected but not admitted as a supported seat.
    Unsupported,
}

/// Bounded model identity carried by every observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceModelIdentity {
    /// Exact DMI product label.
    pub product: String,
    /// Admitted generation.
    pub generation: SurfaceProGeneration,
}

impl SurfaceModelIdentity {
    /// Validate model identity bounds.
    pub fn validate(&self) -> Result<(), SurfaceContractError> {
        validate_text(&self.product, MAX_SURFACE_MODEL_BYTES, "model.product")?;
        match (&*self.product, self.generation) {
            ("Surface Pro 5", SurfaceProGeneration::Pro5)
            | ("Surface Pro 6", SurfaceProGeneration::Pro6) => Ok(()),
            ("Surface Pro 5" | "Surface Pro 6", SurfaceProGeneration::Unsupported)
            | (_, SurfaceProGeneration::Pro5 | SurfaceProGeneration::Pro6) => {
                Err(SurfaceContractError::Invalid("model generation"))
            }
            (_, SurfaceProGeneration::Unsupported) => Ok(()),
        }
    }
}

fn validate_exact_pro56_model(model: &SurfaceModelIdentity) -> Result<(), SurfaceContractError> {
    match (&*model.product, model.generation) {
        ("Surface Pro 5", SurfaceProGeneration::Pro5)
        | ("Surface Pro 6", SurfaceProGeneration::Pro6) => Ok(()),
        _ => Err(SurfaceContractError::Invalid("Surface Pro 5/6 model")),
    }
}

/// Authority that produced an observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceObservationSource {
    /// Kernel sysfs/evdev/IIO observation.
    Kernel,
    /// iptsd digitizer observation.
    Iptsd,
    /// fwupd/LVFS observation.
    Fwupd,
    /// Direct DRM/KMS observation.
    Drm,
    /// A physical operator gesture completed the proof.
    OperatorGesture,
}

/// Honest availability/freshness state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum SurfaceAvailability {
    /// The provider completed successfully at `published_at_ms`.
    Fresh,
    /// A previous fact is retained but no longer fresh.
    Stale {
        /// Why the retained fact is stale.
        reason: String,
    },
    /// No usable provider fact exists.
    Unavailable {
        /// Why no current fact is available.
        reason: String,
    },
}

impl SurfaceAvailability {
    fn validate(&self) -> Result<(), SurfaceContractError> {
        match self {
            Self::Fresh => Ok(()),
            Self::Stale { reason } | Self::Unavailable { reason } => {
                validate_text(reason, MAX_SURFACE_REASON_BYTES, "availability.reason")
            }
        }
    }
}

/// Header shared by every Surface state publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfacePublication {
    /// Contract schema.
    pub schema_version: u64,
    /// Publishing node identity.
    pub node: String,
    /// Detected model identity.
    pub model: SurfaceModelIdentity,
    /// Provider authority.
    pub source: SurfaceObservationSource,
    /// Wall-clock publication time.
    pub published_at_ms: u64,
    /// Honest availability state.
    pub availability: SurfaceAvailability,
}

impl SurfacePublication {
    /// Validate the shared state header.
    pub fn validate(&self) -> Result<(), SurfaceContractError> {
        if self.schema_version != SURFACE_HARDWARE_SCHEMA_VERSION {
            return Err(SurfaceContractError::Invalid("schema_version"));
        }
        validate_id(&self.node, "node")?;
        self.model.validate()?;
        if self.published_at_ms == 0 {
            return Err(SurfaceContractError::Invalid("published_at_ms"));
        }
        self.availability.validate()
    }
}

/// One line-item subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceSubsystem {
    /// Capacitive touchscreen.
    Touch,
    /// Active Surface Pen digitizer.
    Pen,
    /// Detachable keyboard and trackpad.
    TypeCover,
    /// Surface Aggregator Module.
    Sam,
    /// Accelerometer and rotation path.
    RotationAccel,
    /// Front/rear/IR camera devices.
    Cameras,
    /// Wi-Fi and Bluetooth radios.
    WifiBt,
    /// Modern-standby residency.
    S0ix,
    /// Fingerprint hardware, when present.
    Fingerprint,
}

/// Probe verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceProbeState {
    /// Directly verified healthy.
    Ok,
    /// Present but partially impaired.
    Degraded,
    /// Expected hardware is absent or broken.
    Failed,
    /// Physical operator interaction is still required.
    NeedsGesture,
}

/// One bounded probe row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceProbeVerdict {
    /// Subsystem being reported.
    pub subsystem: SurfaceSubsystem,
    /// Current verdict.
    pub state: SurfaceProbeState,
    /// Evidence-backed reason for the verdict.
    pub reason: String,
}

/// Shared verify-board publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceVerifyBoard {
    /// Shared source, model, and freshness metadata.
    pub publication: SurfacePublication,
    /// Honest reason verification was skipped.
    #[serde(default)]
    pub skipped: Option<String>,
    /// Bounded, unique subsystem verdicts.
    #[serde(default)]
    pub rows: Vec<SurfaceProbeVerdict>,
}

impl SurfaceVerifyBoard {
    /// Decode untrusted JSON with duplicate-key, size, and semantic admission.
    pub fn from_json(body: &[u8]) -> Result<Self, SurfaceContractError> {
        decode(body, |value: &Self| value.validate())
    }

    /// Validate board bounds and unique subsystem rows.
    pub fn validate(&self) -> Result<(), SurfaceContractError> {
        self.publication.validate()?;
        validate_exact_pro56_model(&self.publication.model)?;
        if self.publication.source != SurfaceObservationSource::Kernel {
            return Err(SurfaceContractError::Invalid("board source"));
        }
        validate_optional_reason(&self.skipped, "skipped")?;
        if self.skipped.is_some()
            && matches!(self.publication.availability, SurfaceAvailability::Fresh)
        {
            return Err(SurfaceContractError::Invalid("fresh skipped board"));
        }
        if self.skipped.is_some() && !self.rows.is_empty() {
            return Err(SurfaceContractError::Invalid("skipped board rows"));
        }
        if !matches!(self.publication.availability, SurfaceAvailability::Fresh)
            && !self.rows.is_empty()
        {
            return Err(SurfaceContractError::Invalid("unavailable board rows"));
        }
        if self.rows.len() > MAX_SURFACE_PROBE_ROWS {
            return Err(SurfaceContractError::Invalid("rows"));
        }
        let mut seen = HashSet::new();
        for row in &self.rows {
            if !seen.insert(row.subsystem) {
                return Err(SurfaceContractError::Invalid("duplicate subsystem"));
            }
            validate_text(&row.reason, MAX_SURFACE_REASON_BYTES, "row.reason")?;
        }
        Ok(())
    }
}

/// Compact fleet rollup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceFleetSummary {
    /// Shared source, model, and freshness metadata.
    pub publication: SurfacePublication,
    /// Percentage of expected subsystems directly verified healthy.
    pub enablement_pct: u8,
    /// Number of failed subsystems.
    pub red_count: usize,
    /// Unique failed subsystem identities.
    #[serde(default)]
    pub red_subsystems: Vec<SurfaceSubsystem>,
}

impl SurfaceFleetSummary {
    /// Decode untrusted JSON with duplicate-key, size, and semantic admission.
    pub fn from_json(body: &[u8]) -> Result<Self, SurfaceContractError> {
        decode(body, |value: &Self| value.validate())
    }

    /// Validate counts, percentage, and duplicate-free red rows.
    pub fn validate(&self) -> Result<(), SurfaceContractError> {
        self.publication.validate()?;
        validate_exact_pro56_model(&self.publication.model)?;
        if self.publication.source != SurfaceObservationSource::Kernel {
            return Err(SurfaceContractError::Invalid("fleet summary source"));
        }
        if !matches!(self.publication.availability, SurfaceAvailability::Fresh)
            && (self.enablement_pct != 0 || self.red_count != 0 || !self.red_subsystems.is_empty())
        {
            return Err(SurfaceContractError::Invalid("unavailable fleet summary"));
        }
        if self.enablement_pct > 100
            || self.red_count != self.red_subsystems.len()
            || self.red_subsystems.len() > MAX_SURFACE_PROBE_ROWS
        {
            return Err(SurfaceContractError::Invalid("fleet summary"));
        }
        let unique: HashSet<_> = self.red_subsystems.iter().copied().collect();
        if unique.len() != self.red_subsystems.len() {
            return Err(SurfaceContractError::Invalid("duplicate red subsystem"));
        }
        Ok(())
    }
}

/// Firmware device observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceFirmwareDevice {
    /// Stable fwupd device identity.
    pub device_id: String,
    /// Human-readable device label.
    pub name: String,
    /// fwupd plugin authority.
    pub plugin: String,
    /// Installed firmware version.
    pub current_version: String,
    /// Newest admitted release, when present.
    #[serde(default)]
    pub available_version: Option<String>,
    /// SHA-256 of the exact available firmware cabinet, when fwupd publishes
    /// one. A missing checksum keeps the row visible but not safely actionable.
    #[serde(default)]
    pub available_checksum: Option<String>,
    /// Whether the admitted release is newer.
    pub update_available: bool,
}

/// Firmware inventory publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceFirmwareInventory {
    /// Shared source, model, and freshness metadata.
    pub publication: SurfacePublication,
    /// Honest reason inventory was skipped.
    #[serde(default)]
    pub skipped: Option<String>,
    /// Bounded, unique fwupd devices.
    #[serde(default)]
    pub devices: Vec<SurfaceFirmwareDevice>,
}

impl SurfaceFirmwareInventory {
    /// Decode untrusted JSON with duplicate-key, size, and semantic admission.
    pub fn from_json(body: &[u8]) -> Result<Self, SurfaceContractError> {
        decode(body, |value: &Self| value.validate())
    }

    /// Validate inventory bounds and stable unique device identities.
    pub fn validate(&self) -> Result<(), SurfaceContractError> {
        self.publication.validate()?;
        validate_exact_pro56_model(&self.publication.model)?;
        if self.publication.source != SurfaceObservationSource::Fwupd {
            return Err(SurfaceContractError::Invalid("firmware source"));
        }
        validate_optional_reason(&self.skipped, "skipped")?;
        if self.skipped.is_some()
            && matches!(self.publication.availability, SurfaceAvailability::Fresh)
        {
            return Err(SurfaceContractError::Invalid("fresh skipped firmware"));
        }
        if self.skipped.is_some() && !self.devices.is_empty() {
            return Err(SurfaceContractError::Invalid("skipped firmware devices"));
        }
        if !matches!(self.publication.availability, SurfaceAvailability::Fresh)
            && !self.devices.is_empty()
        {
            return Err(SurfaceContractError::Invalid(
                "unavailable firmware devices",
            ));
        }
        if self.devices.len() > MAX_SURFACE_FIRMWARE_DEVICES {
            return Err(SurfaceContractError::Invalid("devices"));
        }
        let mut seen = HashSet::new();
        for device in &self.devices {
            validate_id(&device.device_id, "device_id")?;
            validate_text(&device.name, MAX_SURFACE_MODEL_BYTES, "device.name")?;
            validate_id(&device.plugin, "device.plugin")?;
            validate_id(&device.current_version, "device.current_version")?;
            if let Some(version) = &device.available_version {
                validate_id(version, "device.available_version")?;
            }
            if let Some(checksum) = &device.available_checksum {
                validate_sha256(checksum, "device.available_checksum")?;
                if device.available_version.is_none() {
                    return Err(SurfaceContractError::Invalid(
                        "checksum without available version",
                    ));
                }
            }
            if device.update_available != device.available_version.is_some() {
                return Err(SurfaceContractError::Invalid(
                    "firmware update availability",
                ));
            }
            if !seen.insert(device.device_id.as_str()) {
                return Err(SurfaceContractError::Invalid("duplicate device_id"));
            }
        }
        Ok(())
    }
}

/// Display mode choices already exposed by the Surface card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceDisplayMode {
    /// Panel-native mode.
    Native,
    /// Connector-advertised 1920×1080 compatibility mode.
    Hd1080,
}

/// Enable/MOK action request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceEnableRequest {
    /// Shared action identity and freshness metadata.
    #[serde(flatten)]
    pub header: SurfaceActionHeader,
}

/// Firmware apply action request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceFirmwareApplyRequest {
    /// Shared action identity and freshness metadata.
    #[serde(flatten)]
    pub header: SurfaceActionHeader,
    /// Exact fwupd device target.
    pub device_id: String,
    /// Publication generation the operator selected. The daemon must re-read
    /// fwupd and prove this exact inventory is still fresh before any effect.
    pub inventory_published_at_ms: u64,
    /// Exact release version selected from that inventory.
    pub release_version: String,
    /// Exact SHA-256 cabinet checksum selected from that inventory.
    pub release_checksum: String,
    /// Human-entered firmware apply token.
    #[serde(default)]
    pub arm_token: Option<String>,
}

/// Exact firmware selection carried from an admitted apply request into its
/// result. No result can substitute a different release identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceFirmwareApplyTarget {
    /// Exact fwupd device identity.
    pub device_id: String,
    /// Exact inventory generation selected by the operator.
    pub inventory_published_at_ms: u64,
    /// Exact selected release version.
    pub release_version: String,
    /// Exact selected cabinet SHA-256.
    pub release_checksum: String,
}

impl SurfaceFirmwareApplyTarget {
    fn validate(&self) -> Result<(), SurfaceContractError> {
        validate_id(&self.device_id, "target.device_id")?;
        if self.inventory_published_at_ms == 0 {
            return Err(SurfaceContractError::Invalid(
                "target.inventory_published_at_ms",
            ));
        }
        validate_id(&self.release_version, "target.release_version")?;
        validate_sha256(&self.release_checksum, "target.release_checksum")
    }
}

/// Closed refusal reason for a firmware apply. Arbitrary parser, authorization,
/// provider, and subprocess text never enters the shared result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceFirmwareApplyRefusal {
    /// The Bus message had no request body.
    MissingBody,
    /// The request failed the bounded shared request contract.
    Contract,
    /// The exact-body privileged capability was absent, invalid, or replayed.
    Authorization,
    /// The explicit operator confirmation phrase was absent or incorrect.
    OperatorArm,
    /// The inventory timestamp or selected release binding was invalid/stale.
    SelectionBinding,
    /// A fresh inventory no longer contained the exact selected release.
    ReleaseChanged,
    /// Local DMI was not an admitted exact Surface Pro 5/6 identity.
    UnsupportedModel,
}

/// Closed unavailable reason for a firmware apply provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceFirmwareApplyUnavailable {
    /// The production fwupd apply integration was unavailable.
    ProviderUnavailable,
}

/// Closed failure reason after the admitted provider was invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceFirmwareApplyFailure {
    /// The exact fwupd install/stage operation failed.
    ProviderFailed,
}

/// Bounded outcome of one Surface firmware apply request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum SurfaceFirmwareApplyOutcome {
    /// fwupd accepted the exact cabinet for install/staging.
    Applied,
    /// Admission or explicit arming refused the apply before its effect.
    Refused(SurfaceFirmwareApplyRefusal),
    /// The production provider was not available.
    Unavailable(SurfaceFirmwareApplyUnavailable),
    /// The admitted provider ran but failed.
    Failed(SurfaceFirmwareApplyFailure),
}

impl SurfaceFirmwareApplyOutcome {
    /// Only an accepted fwupd install/stage triggers the verification refresh.
    #[must_use]
    pub const fn triggers_reverify(self) -> bool {
        matches!(self, Self::Applied)
    }
}

/// Versioned, bounded Surface firmware-apply result publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceFirmwareApplyResult {
    /// Result wire schema. Version 2 is an intentional breaking migration from
    /// the former private daemon shape.
    pub result_schema_version: u64,
    /// Shared node, exact model, fwupd source, publication time, and freshness.
    pub publication: SurfacePublication,
    /// Stable request identity, or `unadmitted` when no request decoded.
    pub request_id: String,
    /// Exact request selection. It is absent only when no request was admitted.
    #[serde(default)]
    pub target: Option<SurfaceFirmwareApplyTarget>,
    /// Closed outcome without arbitrary provider or credential text.
    pub outcome: SurfaceFirmwareApplyOutcome,
}

impl SurfaceFirmwareApplyResult {
    /// Decode untrusted JSON with duplicate-key, size, and semantic admission.
    pub fn from_json(body: &[u8]) -> Result<Self, SurfaceContractError> {
        decode(body, |value: &Self| value.validate())
    }

    /// Validate version, exact model/source/freshness, request identity, and
    /// outcome-dependent target presence.
    pub fn validate(&self) -> Result<(), SurfaceContractError> {
        if self.result_schema_version != SURFACE_FIRMWARE_APPLY_RESULT_SCHEMA_VERSION {
            return Err(SurfaceContractError::Invalid("result_schema_version"));
        }
        self.publication.validate()?;
        if self.publication.source != SurfaceObservationSource::Fwupd
            || !matches!(self.publication.availability, SurfaceAvailability::Fresh)
        {
            return Err(SurfaceContractError::Invalid("result publication"));
        }
        validate_id(&self.request_id, "request_id")?;
        match (
            &*self.publication.model.product,
            self.publication.model.generation,
            self.outcome,
        ) {
            ("Surface Pro 5", SurfaceProGeneration::Pro5, _)
            | ("Surface Pro 6", SurfaceProGeneration::Pro6, _) => {}
            (_, SurfaceProGeneration::Unsupported, SurfaceFirmwareApplyOutcome::Refused(_)) => {}
            _ => return Err(SurfaceContractError::Invalid("firmware apply model")),
        }
        match (&self.target, self.outcome) {
            (
                None,
                SurfaceFirmwareApplyOutcome::Refused(
                    SurfaceFirmwareApplyRefusal::MissingBody
                    | SurfaceFirmwareApplyRefusal::Contract,
                ),
            ) if self.request_id == "unadmitted" => Ok(()),
            (Some(target), outcome) => {
                if self.request_id == "unadmitted"
                    || matches!(
                        outcome,
                        SurfaceFirmwareApplyOutcome::Refused(
                            SurfaceFirmwareApplyRefusal::MissingBody
                                | SurfaceFirmwareApplyRefusal::Contract
                        )
                    )
                {
                    return Err(SurfaceContractError::Invalid("firmware apply target"));
                }
                target.validate()
            }
            _ => Err(SurfaceContractError::Invalid("firmware apply target")),
        }
    }
}

/// Explicitly armed request for one privacy-safe camera functional proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceCameraProofRequest {
    /// Shared action identity and freshness metadata.
    #[serde(flatten)]
    pub header: SurfaceActionHeader,
    /// Exact Pro generation the operator inspected and armed.
    pub generation: SurfaceProGeneration,
    /// Human-entered confirmation phrase. This is distinct from the root-minted
    /// exact-body capability in `header.armed_token`.
    #[serde(default)]
    pub arm_token: Option<String>,
}

/// Closed, privacy-safe reason a camera functional proof was unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceCameraProofUnavailable {
    /// The exact local DMI identity is not an admitted Surface Pro 5 or Pro 6.
    UnsupportedModel,
    /// The fixed libcamera proof provider is not installed.
    ProviderMissing,
}

/// Closed failure class from the bounded functional provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceCameraProofFailure {
    /// The provider exceeded its fixed wall-clock deadline and was killed.
    TimedOut,
    /// The provider ran but did not complete one frame successfully.
    CaptureFailed,
}

/// Closed refusal class. No request body, token, camera path, or provider
/// output is copied into the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceCameraProofRefusal {
    /// The typed request failed shared contract admission.
    Contract,
    /// The exact-body privileged capability was absent or invalid.
    Authorization,
    /// The operator confirmation phrase was absent or incorrect.
    OperatorArm,
    /// The requested Pro generation did not equal the exact local DMI result.
    GenerationMismatch,
}

/// Privacy-safe outcome of one camera functional proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum SurfaceCameraProofOutcome {
    /// One frame traversed the libcamera pipeline and was discarded.
    Passed,
    /// The action could not run because its provider/model was unavailable.
    Unavailable(SurfaceCameraProofUnavailable),
    /// The admitted provider ran but failed or timed out.
    Failed(SurfaceCameraProofFailure),
    /// Admission, authorization, or explicit arming refused the action.
    Refused(SurfaceCameraProofRefusal),
}

/// Bounded result publication for one camera functional-proof request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceCameraProofResult {
    /// Current Surface contract schema.
    pub schema_version: u64,
    /// Exact node that consumed the action.
    pub node: String,
    /// Stable request identity, or `unadmitted` when decoding failed.
    pub request_id: String,
    /// Exact admitted local DMI identity. Absent only when the local model is
    /// outside the Pro 5/6 functional-proof allowlist.
    #[serde(default)]
    pub model: Option<SurfaceModelIdentity>,
    /// Completion time; never a claim about frame contents.
    pub completed_at_ms: u64,
    /// Closed result with no camera/device identifier or arbitrary text.
    pub outcome: SurfaceCameraProofOutcome,
}

impl SurfaceCameraProofResult {
    /// Decode untrusted JSON with duplicate-key, size, and semantic admission.
    pub fn from_json(body: &[u8]) -> Result<Self, SurfaceContractError> {
        decode(body, |value: &Self| value.validate())
    }

    /// Validate the exact supported model/generation pair and bounded header.
    pub fn validate(&self) -> Result<(), SurfaceContractError> {
        if self.schema_version != SURFACE_HARDWARE_SCHEMA_VERSION {
            return Err(SurfaceContractError::Invalid("schema_version"));
        }
        validate_id(&self.node, "node")?;
        validate_id(&self.request_id, "request_id")?;
        if self.completed_at_ms == 0 {
            return Err(SurfaceContractError::Invalid("completed_at_ms"));
        }
        match (&self.model, self.outcome) {
            (
                None,
                SurfaceCameraProofOutcome::Unavailable(
                    SurfaceCameraProofUnavailable::UnsupportedModel,
                ),
            ) => Ok(()),
            (Some(model), _) => {
                model.validate()?;
                match (&*model.product, model.generation) {
                    ("Surface Pro 5", SurfaceProGeneration::Pro5)
                    | ("Surface Pro 6", SurfaceProGeneration::Pro6) => Ok(()),
                    _ => Err(SurfaceContractError::Invalid("camera proof model")),
                }
            }
            (None, _) => Err(SurfaceContractError::Invalid("camera proof model")),
        }
    }
}

/// Shared privileged-action identity and freshness header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceActionHeader {
    /// Contract schema.
    pub schema_version: u64,
    /// Exact target node.
    pub node: String,
    /// Stable request identity.
    pub request_id: String,
    /// Request creation wall-clock time.
    pub issued_at_ms: u64,
    /// Exact-body HMAC capability added by the root shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub armed_token: Option<String>,
}

impl SurfaceActionHeader {
    /// Validate schema, identity, target, and freshness before effects.
    pub fn validate_at(
        &self,
        expected_node: &str,
        now_ms: u64,
    ) -> Result<(), SurfaceContractError> {
        if self.schema_version != SURFACE_HARDWARE_SCHEMA_VERSION {
            return Err(SurfaceContractError::Invalid("schema_version"));
        }
        validate_id(&self.node, "node")?;
        validate_id(&self.request_id, "request_id")?;
        if self.node != expected_node {
            return Err(SurfaceContractError::ForeignNode);
        }
        if self.issued_at_ms > now_ms.saturating_add(MAX_SURFACE_ACTION_FUTURE_SKEW_MS)
            || now_ms.saturating_sub(self.issued_at_ms) > MAX_SURFACE_ACTION_AGE_MS
        {
            return Err(SurfaceContractError::Stale);
        }
        if let Some(token) = &self.armed_token {
            validate_text(token, MAX_SURFACE_REASON_BYTES, "armed_token")?;
        }
        Ok(())
    }
}

macro_rules! impl_action_decode {
    ($type:ty, $extra:expr) => {
        impl $type {
            /// Decode and validate a bounded action before any effect.
            pub fn from_json_at(
                body: &[u8],
                expected_node: &str,
                now_ms: u64,
            ) -> Result<Self, SurfaceContractError> {
                decode(body, |value: &Self| {
                    value.header.validate_at(expected_node, now_ms)?;
                    ($extra)(value)
                })
            }
        }
    };
}

impl_action_decode!(SurfaceEnableRequest, |_value: &SurfaceEnableRequest| Ok(()));
impl_action_decode!(
    SurfaceCameraProofRequest,
    |value: &SurfaceCameraProofRequest| {
        if !matches!(
            value.generation,
            SurfaceProGeneration::Pro5 | SurfaceProGeneration::Pro6
        ) {
            return Err(SurfaceContractError::Invalid("generation"));
        }
        validate_optional_reason(&value.arm_token, "arm_token")
    }
);
impl_action_decode!(
    SurfaceFirmwareApplyRequest,
    |value: &SurfaceFirmwareApplyRequest| {
        validate_id(&value.device_id, "device_id")?;
        if value.inventory_published_at_ms == 0 {
            return Err(SurfaceContractError::Invalid("inventory_published_at_ms"));
        }
        validate_id(&value.release_version, "release_version")?;
        validate_sha256(&value.release_checksum, "release_checksum")?;
        validate_optional_reason(&value.arm_token, "arm_token")
    }
);
fn decode<T: for<'de> Deserialize<'de>>(
    body: &[u8],
    validate: impl FnOnce(&T) -> Result<(), SurfaceContractError>,
) -> Result<T, SurfaceContractError> {
    if body.len() > MAX_SURFACE_WIRE_BYTES {
        return Err(SurfaceContractError::Oversized);
    }
    let text = std::str::from_utf8(body).map_err(|_| SurfaceContractError::Malformed)?;
    crate::workloads::reject_duplicate_json_keys(text)
        .map_err(|_| SurfaceContractError::Malformed)?;
    let value = serde_json::from_str(text).map_err(|_| SurfaceContractError::Malformed)?;
    validate(&value)?;
    Ok(value)
}

fn validate_optional_reason(
    value: &Option<String>,
    field: &'static str,
) -> Result<(), SurfaceContractError> {
    match value {
        Some(value) => validate_text(value, MAX_SURFACE_REASON_BYTES, field),
        None => Ok(()),
    }
}

fn validate_id(value: &str, field: &'static str) -> Result<(), SurfaceContractError> {
    validate_text(value, MAX_SURFACE_ID_BYTES, field)?;
    if value
        .chars()
        .any(|character| !(character.is_ascii_alphanumeric() || "-_.:".contains(character)))
    {
        return Err(SurfaceContractError::Invalid(field));
    }
    Ok(())
}

fn validate_sha256(value: &str, field: &'static str) -> Result<(), SurfaceContractError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(SurfaceContractError::Invalid(field));
    }
    Ok(())
}

fn validate_text(value: &str, max: usize, field: &'static str) -> Result<(), SurfaceContractError> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(SurfaceContractError::Invalid(field));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;

    fn publication() -> SurfacePublication {
        SurfacePublication {
            schema_version: SURFACE_HARDWARE_SCHEMA_VERSION,
            node: "surface".into(),
            model: SurfaceModelIdentity {
                product: "Surface Pro 6".into(),
                generation: SurfaceProGeneration::Pro6,
            },
            source: SurfaceObservationSource::Kernel,
            published_at_ms: NOW,
            availability: SurfaceAvailability::Fresh,
        }
    }

    fn action_header() -> SurfaceActionHeader {
        SurfaceActionHeader {
            schema_version: SURFACE_HARDWARE_SCHEMA_VERSION,
            node: "surface".into(),
            request_id: "request-1".into(),
            issued_at_ms: NOW,
            armed_token: None,
        }
    }

    fn probe_row() -> SurfaceProbeVerdict {
        SurfaceProbeVerdict {
            subsystem: SurfaceSubsystem::Touch,
            state: SurfaceProbeState::Ok,
            reason: "touch contacts observed".into(),
        }
    }

    fn firmware_device() -> SurfaceFirmwareDevice {
        SurfaceFirmwareDevice {
            device_id: "uefi-1".into(),
            name: "System Firmware".into(),
            plugin: "uefi_capsule".into(),
            current_version: "1.0".into(),
            available_version: Some("1.1".into()),
            available_checksum: Some("a".repeat(64)),
            update_available: true,
        }
    }

    fn firmware_publication() -> SurfacePublication {
        SurfacePublication {
            source: SurfaceObservationSource::Fwupd,
            ..publication()
        }
    }

    #[test]
    fn pro5_and_pro6_are_explicit_while_other_generations_are_not_claimed() {
        let pro5 = SurfaceModelIdentity {
            product: "Surface Pro 5".into(),
            generation: SurfaceProGeneration::Pro5,
        };
        let pro6 = publication().model;
        assert!(pro5.validate().is_ok());
        assert!(pro6.validate().is_ok());
        assert_ne!(
            SurfaceProGeneration::Pro5,
            SurfaceProGeneration::Unsupported
        );
        assert_ne!(
            SurfaceProGeneration::Pro6,
            SurfaceProGeneration::Unsupported
        );
    }

    #[test]
    fn observations_bind_exact_pro56_identity_and_provider_source() {
        let mut board = SurfaceVerifyBoard {
            publication: publication(),
            skipped: None,
            rows: vec![probe_row()],
        };
        board.publication.source = SurfaceObservationSource::Fwupd;
        assert_eq!(
            board.validate(),
            Err(SurfaceContractError::Invalid("board source"))
        );

        let mut summary = SurfaceFleetSummary {
            publication: publication(),
            enablement_pct: 100,
            red_count: 0,
            red_subsystems: vec![],
        };
        summary.publication.model.generation = SurfaceProGeneration::Pro5;
        assert_eq!(
            summary.validate(),
            Err(SurfaceContractError::Invalid("model generation"))
        );
        summary.publication.model = SurfaceModelIdentity {
            product: "Surface Pro 8".into(),
            generation: SurfaceProGeneration::Unsupported,
        };
        assert_eq!(
            summary.validate(),
            Err(SurfaceContractError::Invalid("Surface Pro 5/6 model"))
        );

        let mut inventory = SurfaceFirmwareInventory {
            publication: SurfacePublication {
                source: SurfaceObservationSource::Fwupd,
                ..publication()
            },
            skipped: None,
            devices: vec![firmware_device()],
        };
        inventory.publication.source = SurfaceObservationSource::Kernel;
        assert_eq!(
            inventory.validate(),
            Err(SurfaceContractError::Invalid("firmware source"))
        );
    }

    #[test]
    fn board_rejects_duplicate_rows_oversized_reasons_and_unknown_fields() {
        let board = SurfaceVerifyBoard {
            publication: publication(),
            skipped: None,
            rows: vec![
                SurfaceProbeVerdict {
                    subsystem: SurfaceSubsystem::Touch,
                    state: SurfaceProbeState::Ok,
                    reason: "touch contacts observed".into(),
                },
                SurfaceProbeVerdict {
                    subsystem: SurfaceSubsystem::Touch,
                    state: SurfaceProbeState::Failed,
                    reason: "duplicate".into(),
                },
            ],
        };
        assert!(board.validate().is_err());

        let mut body = serde_json::to_value(SurfaceVerifyBoard {
            rows: vec![],
            ..board
        })
        .unwrap();
        body.as_object_mut()
            .unwrap()
            .insert("unexpected".into(), true.into());
        assert_eq!(
            SurfaceVerifyBoard::from_json(serde_json::to_string(&body).unwrap().as_bytes()),
            Err(SurfaceContractError::Malformed)
        );
    }

    #[test]
    fn board_rejects_rows_when_skipped_unavailable_or_stale() {
        let fresh_skipped = SurfaceVerifyBoard {
            publication: publication(),
            skipped: Some("operator gesture unavailable".into()),
            rows: vec![],
        };
        assert_eq!(
            fresh_skipped.validate(),
            Err(SurfaceContractError::Invalid("fresh skipped board"))
        );

        let mut skipped_publication = publication();
        skipped_publication.availability = SurfaceAvailability::Unavailable {
            reason: "provider absent".into(),
        };
        let skipped = SurfaceVerifyBoard {
            publication: skipped_publication,
            skipped: Some("operator gesture unavailable".into()),
            rows: vec![probe_row()],
        };
        assert_eq!(
            skipped.validate(),
            Err(SurfaceContractError::Invalid("skipped board rows"))
        );

        for availability in [
            SurfaceAvailability::Unavailable {
                reason: "provider absent".into(),
            },
            SurfaceAvailability::Stale {
                reason: "provider timed out".into(),
            },
        ] {
            let mut publication = publication();
            publication.availability = availability;
            let board = SurfaceVerifyBoard {
                publication,
                skipped: None,
                rows: vec![probe_row()],
            };
            assert_eq!(
                board.validate(),
                Err(SurfaceContractError::Invalid("unavailable board rows"))
            );
        }
    }

    #[test]
    fn fleet_summary_rejects_facts_when_unavailable_or_stale() {
        for availability in [
            SurfaceAvailability::Unavailable {
                reason: "provider absent".into(),
            },
            SurfaceAvailability::Stale {
                reason: "provider timed out".into(),
            },
        ] {
            let mut publication = publication();
            publication.availability = availability;
            let summary = SurfaceFleetSummary {
                publication,
                enablement_pct: 100,
                red_count: 0,
                red_subsystems: vec![],
            };
            assert_eq!(
                summary.validate(),
                Err(SurfaceContractError::Invalid("unavailable fleet summary"))
            );
        }
    }

    #[test]
    fn duplicate_json_keys_and_wire_overflow_fail_closed() {
        let duplicate = br#"{"publication":{"schema_version":1,"schema_version":1}}"#;
        assert_eq!(
            SurfaceVerifyBoard::from_json(duplicate),
            Err(SurfaceContractError::Malformed)
        );
        assert_eq!(
            SurfaceVerifyBoard::from_json(&vec![b' '; MAX_SURFACE_WIRE_BYTES + 1]),
            Err(SurfaceContractError::Oversized)
        );
    }

    #[test]
    fn actions_reject_foreign_stale_future_and_oversized_input_before_effects() {
        let request = SurfaceEnableRequest {
            header: action_header(),
        };
        let body = serde_json::to_vec(&request).unwrap();
        assert!(SurfaceEnableRequest::from_json_at(&body, "surface", NOW).is_ok());
        assert_eq!(
            SurfaceEnableRequest::from_json_at(&body, "other", NOW),
            Err(SurfaceContractError::ForeignNode)
        );
        assert_eq!(
            SurfaceEnableRequest::from_json_at(
                &body,
                "surface",
                NOW + MAX_SURFACE_ACTION_AGE_MS + 1
            ),
            Err(SurfaceContractError::Stale)
        );

        let mut future = request;
        future.header.issued_at_ms = NOW + MAX_SURFACE_ACTION_FUTURE_SKEW_MS + 1;
        assert_eq!(
            SurfaceEnableRequest::from_json_at(
                &serde_json::to_vec(&future).unwrap(),
                "surface",
                NOW
            ),
            Err(SurfaceContractError::Stale)
        );
        assert_eq!(
            SurfaceEnableRequest::from_json_at(
                &vec![b' '; MAX_SURFACE_WIRE_BYTES + 1],
                "surface",
                NOW
            ),
            Err(SurfaceContractError::Oversized)
        );
        let duplicate = br#"{"schema_version":1,"schema_version":1,"node":"surface","request_id":"request-1","issued_at_ms":1800000000000}"#;
        assert_eq!(
            SurfaceEnableRequest::from_json_at(duplicate, "surface", NOW),
            Err(SurfaceContractError::Malformed)
        );
    }

    #[test]
    fn firmware_inventory_rejects_duplicate_devices() {
        let device = firmware_device();
        let inventory = SurfaceFirmwareInventory {
            publication: firmware_publication(),
            skipped: None,
            devices: vec![device.clone(), device],
        };
        assert_eq!(
            inventory.validate(),
            Err(SurfaceContractError::Invalid("duplicate device_id"))
        );
    }

    #[test]
    fn firmware_inventory_rejects_devices_when_skipped_unavailable_or_stale() {
        let fresh_skipped = SurfaceFirmwareInventory {
            publication: firmware_publication(),
            skipped: Some("fwupd unavailable".into()),
            devices: vec![],
        };
        assert_eq!(
            fresh_skipped.validate(),
            Err(SurfaceContractError::Invalid("fresh skipped firmware"))
        );

        let mut skipped_publication = firmware_publication();
        skipped_publication.availability = SurfaceAvailability::Unavailable {
            reason: "provider absent".into(),
        };
        let skipped = SurfaceFirmwareInventory {
            publication: skipped_publication,
            skipped: Some("fwupd unavailable".into()),
            devices: vec![firmware_device()],
        };
        assert_eq!(
            skipped.validate(),
            Err(SurfaceContractError::Invalid("skipped firmware devices"))
        );

        for availability in [
            SurfaceAvailability::Unavailable {
                reason: "provider absent".into(),
            },
            SurfaceAvailability::Stale {
                reason: "provider timed out".into(),
            },
        ] {
            let mut publication = firmware_publication();
            publication.availability = availability;
            let inventory = SurfaceFirmwareInventory {
                publication,
                skipped: None,
                devices: vec![firmware_device()],
            };
            assert_eq!(
                inventory.validate(),
                Err(SurfaceContractError::Invalid(
                    "unavailable firmware devices"
                ))
            );
        }
    }

    #[test]
    fn firmware_update_flag_and_available_version_must_agree() {
        let mut missing_version = firmware_device();
        missing_version.available_version = None;
        missing_version.available_checksum = None;
        assert_eq!(
            SurfaceFirmwareInventory {
                publication: firmware_publication(),
                skipped: None,
                devices: vec![missing_version],
            }
            .validate(),
            Err(SurfaceContractError::Invalid(
                "firmware update availability"
            ))
        );

        let mut false_update = firmware_device();
        false_update.update_available = false;
        assert_eq!(
            SurfaceFirmwareInventory {
                publication: firmware_publication(),
                skipped: None,
                devices: vec![false_update],
            }
            .validate(),
            Err(SurfaceContractError::Invalid(
                "firmware update availability"
            ))
        );

        let mut current = firmware_device();
        current.available_version = None;
        current.available_checksum = None;
        current.update_available = false;
        assert!(SurfaceFirmwareInventory {
            publication: firmware_publication(),
            skipped: None,
            devices: vec![current],
        }
        .validate()
        .is_ok());
    }

    #[test]
    fn firmware_apply_binds_inventory_release_and_sha256() {
        let request = SurfaceFirmwareApplyRequest {
            header: action_header(),
            device_id: "uefi-1".into(),
            inventory_published_at_ms: NOW,
            release_version: "1.1".into(),
            release_checksum: "a".repeat(64),
            arm_token: Some("APPLY-SURFACE-FIRMWARE".into()),
        };
        let body = serde_json::to_vec(&request).unwrap();
        assert!(SurfaceFirmwareApplyRequest::from_json_at(&body, "surface", NOW).is_ok());

        let mut malformed = request;
        malformed.release_checksum = "A".repeat(64);
        assert_eq!(
            SurfaceFirmwareApplyRequest::from_json_at(
                &serde_json::to_vec(&malformed).unwrap(),
                "surface",
                NOW,
            ),
            Err(SurfaceContractError::Invalid("release_checksum"))
        );
    }

    fn firmware_apply_result() -> SurfaceFirmwareApplyResult {
        SurfaceFirmwareApplyResult {
            result_schema_version: SURFACE_FIRMWARE_APPLY_RESULT_SCHEMA_VERSION,
            publication: SurfacePublication {
                source: SurfaceObservationSource::Fwupd,
                ..publication()
            },
            request_id: "firmware-request-1".into(),
            target: Some(SurfaceFirmwareApplyTarget {
                device_id: "uefi-1".into(),
                inventory_published_at_ms: NOW,
                release_version: "1.1".into(),
                release_checksum: "a".repeat(64),
            }),
            outcome: SurfaceFirmwareApplyOutcome::Applied,
        }
    }

    #[test]
    fn firmware_apply_result_v2_binds_exact_target_model_source_and_freshness() {
        let result = firmware_apply_result();
        let body = serde_json::to_vec(&result).unwrap();
        assert_eq!(
            SurfaceFirmwareApplyResult::from_json(&body),
            Ok(result.clone())
        );

        let mut wrong_source = result.clone();
        wrong_source.publication.source = SurfaceObservationSource::Kernel;
        assert_eq!(
            wrong_source.validate(),
            Err(SurfaceContractError::Invalid("result publication"))
        );
        let mut stale = result.clone();
        stale.publication.availability = SurfaceAvailability::Stale {
            reason: "old result".into(),
        };
        assert_eq!(
            stale.validate(),
            Err(SurfaceContractError::Invalid("result publication"))
        );
        let mut substituted = result;
        substituted.publication.model.product = "Surface Pro 5".into();
        assert_eq!(
            substituted.validate(),
            Err(SurfaceContractError::Invalid("model generation"))
        );
    }

    #[test]
    fn firmware_apply_result_rejects_unknown_duplicate_unbounded_and_free_form_reason() {
        let value = serde_json::to_value(firmware_apply_result()).unwrap();
        let mut unknown = value.clone();
        unknown.as_object_mut().unwrap().insert(
            "detail".into(),
            serde_json::Value::String("provider stderr".into()),
        );
        assert_eq!(
            SurfaceFirmwareApplyResult::from_json(&serde_json::to_vec(&unknown).unwrap()),
            Err(SurfaceContractError::Malformed)
        );

        let raw = serde_json::to_string(&value).unwrap();
        let duplicate = raw.replacen(
            "{",
            r#"{"result_schema_version":2,"result_schema_version":2,"#,
            1,
        );
        assert_eq!(
            SurfaceFirmwareApplyResult::from_json(duplicate.as_bytes()),
            Err(SurfaceContractError::Malformed)
        );
        assert_eq!(
            SurfaceFirmwareApplyResult::from_json(&vec![b' '; MAX_SURFACE_WIRE_BYTES + 1]),
            Err(SurfaceContractError::Oversized)
        );

        let hostile = raw.replace(
            r#"{"state":"applied"}"#,
            r#"{"state":"failed","reason":"raw provider stderr"}"#,
        );
        assert_eq!(
            SurfaceFirmwareApplyResult::from_json(hostile.as_bytes()),
            Err(SurfaceContractError::Malformed)
        );
    }

    #[test]
    fn firmware_apply_result_target_presence_tracks_admission_state() {
        let mut missing = firmware_apply_result();
        missing.target = None;
        assert_eq!(
            missing.validate(),
            Err(SurfaceContractError::Invalid("firmware apply target"))
        );

        missing.request_id = "unadmitted".into();
        missing.outcome =
            SurfaceFirmwareApplyOutcome::Refused(SurfaceFirmwareApplyRefusal::Contract);
        assert!(missing.validate().is_ok());

        let mut bad_checksum = firmware_apply_result();
        bad_checksum.target.as_mut().unwrap().release_checksum = "A".repeat(64);
        assert_eq!(
            bad_checksum.validate(),
            Err(SurfaceContractError::Invalid("target.release_checksum"))
        );
    }

    #[test]
    fn camera_proof_request_is_fresh_node_bound_and_pro56_only() {
        let request = SurfaceCameraProofRequest {
            header: action_header(),
            generation: SurfaceProGeneration::Pro6,
            arm_token: Some(SURFACE_CAMERA_PROOF_ARM_TOKEN.into()),
        };
        let body = serde_json::to_vec(&request).unwrap();
        assert!(SurfaceCameraProofRequest::from_json_at(&body, "surface", NOW).is_ok());
        assert_eq!(
            SurfaceCameraProofRequest::from_json_at(&body, "other", NOW),
            Err(SurfaceContractError::ForeignNode)
        );

        let unsupported = SurfaceCameraProofRequest {
            generation: SurfaceProGeneration::Unsupported,
            ..request
        };
        assert_eq!(
            SurfaceCameraProofRequest::from_json_at(
                &serde_json::to_vec(&unsupported).unwrap(),
                "surface",
                NOW,
            ),
            Err(SurfaceContractError::Invalid("generation"))
        );
    }

    #[test]
    fn camera_proof_result_admits_only_exact_pro56_or_unsupported_unavailable() {
        let passed = SurfaceCameraProofResult {
            schema_version: SURFACE_HARDWARE_SCHEMA_VERSION,
            node: "surface".into(),
            request_id: "camera-proof-1".into(),
            model: Some(publication().model),
            completed_at_ms: NOW,
            outcome: SurfaceCameraProofOutcome::Passed,
        };
        assert!(passed.validate().is_ok());

        let mut substituted = passed.clone();
        substituted.model = Some(SurfaceModelIdentity {
            product: "Surface Pro 8".into(),
            generation: SurfaceProGeneration::Unsupported,
        });
        assert_eq!(
            substituted.validate(),
            Err(SurfaceContractError::Invalid("camera proof model"))
        );

        let unavailable = SurfaceCameraProofResult {
            model: None,
            outcome: SurfaceCameraProofOutcome::Unavailable(
                SurfaceCameraProofUnavailable::UnsupportedModel,
            ),
            ..passed
        };
        assert!(unavailable.validate().is_ok());
        let mut missing_model_success = unavailable;
        missing_model_success.outcome = SurfaceCameraProofOutcome::Passed;
        assert!(missing_model_success.validate().is_err());
    }
}
