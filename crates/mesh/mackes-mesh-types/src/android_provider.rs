//! WL-FUNC-020 — bounded Cuttlefish Android VM provider/lifecycle contract.
//!
//! This module is the provider-owned seam around the package inventory in
//! [`crate::android_apps`]. It carries only stable identities, immutable image
//! provenance, guest-owned readiness evidence, and a closed set of lifecycle
//! operations. It deliberately has no executable, path, URL, ADB, socket, or
//! arbitrary intent fields.

#![allow(clippy::missing_errors_doc)]

use serde::{de, Deserialize, Serialize};

/// The only Cuttlefish provider contract schema currently admitted.
pub const CUTTLEFISH_PROVIDER_SCHEMA_VERSION: u16 = 1;

/// The only Cuttlefish lifecycle request schema currently admitted.
pub const CUTTLEFISH_LIFECYCLE_SCHEMA_VERSION: u16 = 1;

/// Schema for Android placement preflight rows folded into `state/cloud/<node>`.
pub const ANDROID_PROVIDER_ADMISSION_SCHEMA_VERSION: u16 = 1;
/// Schema for a guest-owned Android VDI source.
pub const ANDROID_VDI_SOURCE_SCHEMA_VERSION: u16 = 1;

const MAX_ID_BYTES: usize = 128;

/// Closed display protocol exported by the Cuttlefish guest session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AndroidVdiProtocol {
    /// Cuttlefish's guest-owned WebRTC display surface.
    WebRtc,
}

/// Truthful, generation-bound VDI source reported by the guest agent.
///
/// This is discovery data, not a shell command or an attach ticket. Consumers
/// must still acquire session authorization through the VDI authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidVdiSource {
    /// Wire schema discriminator.
    pub schema_version: u16,
    /// Stable Android workload identity.
    pub workload_id: String,
    /// Exact admitted image provenance served by this session.
    pub image_provenance: CuttlefishImageProvenanceRef,
    /// SHA-256 digest of the validated catalog payload.
    pub catalog_digest: String,
    /// Exact lifecycle generation that owns the session.
    pub generation: u64,
    /// Closed display protocol.
    pub protocol: AndroidVdiProtocol,
    /// Mesh DNS identity of the outer VM. Raw IPs and URLs are not admitted.
    pub mesh_host: String,
    /// Guest-owned display port.
    pub port: u16,
    /// Opaque bounded guest session identity.
    pub session_id: String,
    /// Observation time in Unix epoch milliseconds.
    pub observed_at_unix_ms: u64,
    /// Expiry time in Unix epoch milliseconds.
    pub expires_at_unix_ms: u64,
}

impl AndroidVdiSource {
    /// Validate identity, provenance, generation, host, and freshness bounds.
    pub fn validate(&self) -> Result<(), CuttlefishContractError> {
        const MAX_VDI_TTL_MS: u64 = 5 * 60 * 1_000;
        if self.schema_version != ANDROID_VDI_SOURCE_SCHEMA_VERSION {
            return Err(CuttlefishContractError::UnsupportedSchema(
                self.schema_version,
            ));
        }
        CuttlefishVmId::new(self.workload_id.clone())?;
        self.image_provenance.validate()?;
        if !is_valid_sha256_digest(&self.catalog_digest) {
            return Err(CuttlefishContractError::InvalidImageDigest);
        }
        if self.generation == 0 || self.port == 0 {
            return Err(CuttlefishContractError::InvalidGeneration);
        }
        validate_identity("session_id", &self.session_id)?;
        if !is_valid_mesh_host(&self.mesh_host) {
            return Err(CuttlefishContractError::InvalidField("mesh_host"));
        }
        if self.observed_at_unix_ms == 0
            || self.expires_at_unix_ms <= self.observed_at_unix_ms
            || self
                .expires_at_unix_ms
                .saturating_sub(self.observed_at_unix_ms)
                > MAX_VDI_TTL_MS
        {
            return Err(CuttlefishContractError::InvalidTimestamp);
        }
        Ok(())
    }

    /// Admit a source only when it belongs to the exact running contract.
    pub fn admitted_against(
        self,
        target: &CuttlefishVmTarget,
        catalog_digest: &str,
        generation: u64,
    ) -> Result<Self, CuttlefishContractError> {
        self.validate()?;
        if self.workload_id != target.vm_id.as_str() {
            return Err(CuttlefishContractError::TargetIdentityMismatch);
        }
        if self.image_provenance != target.image_provenance {
            return Err(CuttlefishContractError::ImageProvenanceMismatch);
        }
        if self.catalog_digest != catalog_digest || self.generation != generation {
            return Err(CuttlefishContractError::GenerationMismatch {
                expected: generation,
                actual: self.generation,
            });
        }
        Ok(self)
    }
}

/// Closed readiness result for Android provider placement admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AndroidProviderReadiness {
    /// Every image-integrity, host-capability, capacity, and provider-health check passed.
    Ready,
    /// Placement is refused; [`AndroidProviderAdmission::refusal`] names why.
    Unavailable,
}

/// Exact fail-closed reason an Android placement preflight was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AndroidProviderRefusal {
    /// No validated catalog is available from the catalog worker.
    CatalogUnavailable,
    /// The admitted catalog validity window has elapsed.
    CatalogExpired,
    /// Catalog image provenance is internally invalid.
    CatalogImageMismatch,
    /// The host-local exact package manifest is absent or invalid.
    PackageManifestUnavailable,
    /// The package manifest does not equal the catalog binding.
    PackageManifestMismatch,
    /// Desired image identity, digest, or resources do not match policy.
    DesiredImageMismatch,
    /// No readable immutable image artifact is configured.
    ImageArtifactUnavailable,
    /// The artifact bytes do not match the declared digest.
    ImageDigestMismatch,
    /// The KVM device is unavailable.
    KvmUnavailable,
    /// The active KVM module does not report nested virtualization enabled.
    NestedVirtualizationUnavailable,
    /// Available logical CPUs are below the admitted profile.
    InsufficientVcpu,
    /// Available host memory is below the admitted profile.
    InsufficientMemory,
    /// Available artifact-filesystem space is below the admitted profile.
    InsufficientDisk,
    /// The existing libvirt provider health probe is not up.
    ProviderUnavailable,
}

/// One bounded Android provider preflight row in the existing cloud authority.
///
/// This is placement evidence, not guest readiness. A `Ready` row permits a
/// Cuttlefish adapter to be considered for placement; only guest-owned evidence
/// may later claim that Android itself is running or launchable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidProviderAdmission {
    /// Wire schema discriminator.
    pub schema_version: u16,
    /// Stable Android workload identity.
    pub workload_id: String,
    /// Signed immutable image binding, absent when no catalog was admitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_provenance: Option<CuttlefishImageProvenanceRef>,
    /// Closed placement readiness result.
    pub readiness: AndroidProviderReadiness,
    /// Exact reason when readiness is unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<AndroidProviderRefusal>,
    /// Whether `/dev/kvm` is a real character device.
    pub kvm_available: bool,
    /// Whether the active KVM vendor module enables nesting.
    pub nested_virtualization: bool,
    /// Whether the existing libvirt health authority is up.
    pub provider_healthy: bool,
    /// Required logical CPUs from Cuttlefish and signed policy.
    pub required_vcpus: u16,
    /// Logical CPUs observed on the host.
    pub available_vcpus: u16,
    /// Required memory in MiB.
    pub required_memory_mib: u64,
    /// Available host memory in MiB.
    pub available_memory_mib: u64,
    /// Required disk space in MiB.
    pub required_disk_mib: u64,
    /// Available image-filesystem space in MiB.
    pub available_disk_mib: u64,
    /// Host observation time in Unix epoch milliseconds.
    pub observed_at_unix_ms: u64,
}

impl AndroidProviderAdmission {
    /// Validate bounds and prevent a producer from attaching a fake-ready label
    /// to incomplete host evidence.
    pub fn validate(&self) -> Result<(), CuttlefishContractError> {
        if self.schema_version != ANDROID_PROVIDER_ADMISSION_SCHEMA_VERSION {
            return Err(CuttlefishContractError::UnsupportedSchema(
                self.schema_version,
            ));
        }
        CuttlefishVmId::new(self.workload_id.clone())?;
        if let Some(provenance) = &self.image_provenance {
            provenance.validate()?;
        }
        if self.required_vcpus == 0
            || self.required_memory_mib == 0
            || self.required_disk_mib == 0
            || self.observed_at_unix_ms == 0
        {
            return Err(CuttlefishContractError::InvalidField("provider_admission"));
        }
        let evidence_ready = self.image_provenance.is_some()
            && self.kvm_available
            && self.nested_virtualization
            && self.provider_healthy
            && self.available_vcpus >= self.required_vcpus
            && self.available_memory_mib >= self.required_memory_mib
            && self.available_disk_mib >= self.required_disk_mib;
        match (self.readiness, self.refusal, evidence_ready) {
            (AndroidProviderReadiness::Ready, None, true)
            | (AndroidProviderReadiness::Unavailable, Some(_), _) => Ok(()),
            _ => Err(CuttlefishContractError::InvalidGuestEvidence),
        }
    }

    #[must_use]
    /// Whether the row is internally valid and placement-ready.
    pub fn is_ready(&self) -> bool {
        self.validate().is_ok() && self.readiness == AndroidProviderReadiness::Ready
    }
}

/// The provider implementation represented by this contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CuttlefishProvider {
    /// The nested Android guest is served by Cuttlefish.
    Cuttlefish,
}

/// Why a Cuttlefish provider record was rejected before backend contact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CuttlefishContractError {
    /// The consumer does not implement the supplied schema version.
    UnsupportedSchema(u16),
    /// A bounded identity or revision field is empty or contains unsafe data.
    InvalidField(&'static str),
    /// A bounded field exceeds the wire limit.
    FieldTooLong(&'static str),
    /// The image digest is not a lowercase, non-zero SHA-256 digest.
    InvalidImageDigest,
    /// A timestamp is zero.
    InvalidTimestamp,
    /// A lifecycle generation is invalid for the requested operation/state.
    InvalidGeneration,
    /// Boot and readiness evidence do not form an admitted pair.
    InvalidGuestEvidence,
    /// A lifecycle state does not agree with its guest evidence.
    InvalidLifecycleState,
    /// A failed or unavailable guest omitted its closed reason.
    MissingUnavailableReason,
    /// A healthy guest supplied an unavailable reason.
    UnexpectedUnavailableReason,
    /// The request target does not identify the observed VM.
    TargetIdentityMismatch,
    /// The request image does not match the observed image provenance.
    ImageProvenanceMismatch,
    /// The request generation is stale or otherwise does not match the VM.
    GenerationMismatch {
        /// Generation supplied by the lifecycle caller.
        expected: u64,
        /// Generation retained by the provider observation.
        actual: u64,
    },
    /// The operation is not valid from the observed lifecycle state.
    OperationNotAllowed {
        /// Requested closed lifecycle operation.
        operation: CuttlefishLifecycleOperation,
        /// Current observed lifecycle state.
        state: CuttlefishVmLifecycleState,
    },
    /// The target provider is not the provider represented by this module.
    InvalidProvider,
}

impl core::fmt::Display for CuttlefishContractError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnsupportedSchema(version) => {
                write!(formatter, "unsupported Cuttlefish schema {version}")
            }
            Self::InvalidField(field) => write!(formatter, "invalid Cuttlefish field {field}"),
            Self::FieldTooLong(field) => {
                write!(
                    formatter,
                    "Cuttlefish field exceeds {MAX_ID_BYTES} bytes: {field}"
                )
            }
            Self::InvalidImageDigest => formatter.write_str("invalid Cuttlefish image digest"),
            Self::InvalidTimestamp => formatter.write_str("invalid Cuttlefish timestamp"),
            Self::InvalidGeneration => formatter.write_str("invalid Cuttlefish generation"),
            Self::InvalidGuestEvidence => formatter.write_str("invalid Cuttlefish guest evidence"),
            Self::InvalidLifecycleState => {
                formatter.write_str("invalid Cuttlefish lifecycle state")
            }
            Self::MissingUnavailableReason => {
                formatter.write_str("missing Cuttlefish unavailable reason")
            }
            Self::UnexpectedUnavailableReason => {
                formatter.write_str("unexpected Cuttlefish unavailable reason")
            }
            Self::TargetIdentityMismatch => {
                formatter.write_str("Cuttlefish lifecycle target identity mismatch")
            }
            Self::ImageProvenanceMismatch => {
                formatter.write_str("Cuttlefish image provenance mismatch")
            }
            Self::GenerationMismatch { expected, actual } => write!(
                formatter,
                "Cuttlefish generation mismatch: expected {expected}, observed {actual}"
            ),
            Self::OperationNotAllowed { operation, state } => {
                write!(
                    formatter,
                    "Cuttlefish operation {operation:?} is not allowed from {state:?}"
                )
            }
            Self::InvalidProvider => formatter.write_str("invalid Cuttlefish provider"),
        }
    }
}

impl std::error::Error for CuttlefishContractError {}

/// Stable identity of one Cuttlefish-backed Android VM.
///
/// This is a workload identity, not a host path, URL, command, or backend
/// locator. The provider must retain it across reboots and reconnects.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct CuttlefishVmId(String);

impl CuttlefishVmId {
    /// Construct a bounded stable VM identity.
    pub fn new(value: impl Into<String>) -> Result<Self, CuttlefishContractError> {
        let value = value.into();
        validate_identity("vm_id", &value)?;
        Ok(Self(value))
    }

    /// Borrow the stable identity as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for CuttlefishVmId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Immutable image identity and build provenance bound to a Cuttlefish VM.
///
/// These fields are references to already-produced image evidence. They are
/// intentionally not registry URLs, filesystem paths, or image-build
/// commands. A real provider must verify the corresponding full manifest
/// before admitting this reduced binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CuttlefishImageProvenanceRef {
    /// Stable image identity from the governed Android image manifest.
    pub image_id: String,
    /// Immutable lowercase `sha256:<64 hex>` image digest.
    pub image_digest: String,
    /// Source/build revision that produced the image.
    pub source_revision: String,
    /// Governed AOSP starter-catalog revision bound into the image.
    pub catalog_revision: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CuttlefishImageProvenanceRefWire {
    image_id: String,
    image_digest: String,
    source_revision: String,
    catalog_revision: String,
}

impl<'de> Deserialize<'de> for CuttlefishImageProvenanceRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = CuttlefishImageProvenanceRefWire::deserialize(deserializer)?;
        Self::new(
            wire.image_id,
            wire.image_digest,
            wire.source_revision,
            wire.catalog_revision,
        )
        .map_err(de::Error::custom)
    }
}

impl CuttlefishImageProvenanceRef {
    /// Construct and validate an immutable image provenance reference.
    pub fn new(
        image_id: impl Into<String>,
        image_digest: impl Into<String>,
        source_revision: impl Into<String>,
        catalog_revision: impl Into<String>,
    ) -> Result<Self, CuttlefishContractError> {
        let reference = Self {
            image_id: image_id.into(),
            image_digest: image_digest.into(),
            source_revision: source_revision.into(),
            catalog_revision: catalog_revision.into(),
        };
        reference.validate()?;
        Ok(reference)
    }

    /// Validate the complete reduced provenance binding.
    pub fn validate(&self) -> Result<(), CuttlefishContractError> {
        validate_identity("image_id", &self.image_id)?;
        if !is_valid_sha256_digest(&self.image_digest) {
            return Err(CuttlefishContractError::InvalidImageDigest);
        }
        validate_identity("source_revision", &self.source_revision)?;
        validate_identity("catalog_revision", &self.catalog_revision)
    }
}

/// A provider target combining stable VM identity and immutable image intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CuttlefishVmTarget {
    /// Contract schema discriminator.
    pub schema_version: u16,
    /// Closed provider identity.
    pub provider: CuttlefishProvider,
    /// Stable Android VM workload identity.
    pub vm_id: CuttlefishVmId,
    /// Image provenance bound to this VM target.
    pub image_provenance: CuttlefishImageProvenanceRef,
}

impl CuttlefishVmTarget {
    /// Construct and validate a Cuttlefish VM target.
    pub fn new(
        vm_id: CuttlefishVmId,
        image_provenance: CuttlefishImageProvenanceRef,
    ) -> Result<Self, CuttlefishContractError> {
        let target = Self {
            schema_version: CUTTLEFISH_PROVIDER_SCHEMA_VERSION,
            provider: CuttlefishProvider::Cuttlefish,
            vm_id,
            image_provenance,
        };
        target.validate()?;
        Ok(target)
    }

    /// Validate a target before provider lookup or backend contact.
    pub fn validate(&self) -> Result<(), CuttlefishContractError> {
        if self.schema_version != CUTTLEFISH_PROVIDER_SCHEMA_VERSION {
            return Err(CuttlefishContractError::UnsupportedSchema(
                self.schema_version,
            ));
        }
        if self.provider != CuttlefishProvider::Cuttlefish {
            return Err(CuttlefishContractError::InvalidProvider);
        }
        validate_identity("vm_id", self.vm_id.as_str())?;
        self.image_provenance.validate()
    }

    /// Admit a target received across an untrusted provider boundary.
    pub fn admitted(self) -> Result<Self, CuttlefishContractError> {
        self.validate()?;
        Ok(self)
    }
}

/// Closed boot state reported by the Cuttlefish guest provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CuttlefishGuestBootState {
    /// No guest boot observation is available yet.
    Pending,
    /// The outer VM or inner Android guest is booting.
    Booting,
    /// The Android guest completed boot.
    Ready,
    /// The guest is intentionally stopped.
    Stopped,
    /// The provider cannot report a guest boot state.
    Unavailable,
    /// Guest boot failed and cannot currently serve the VM.
    Failed,
}

/// Closed readiness state for the guest-owned Android surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CuttlefishGuestReadiness {
    /// No readiness observation is available.
    Unknown,
    /// The guest exists but is not ready to serve an application surface.
    NotReady,
    /// The guest is ready for the package/session layer.
    Ready,
    /// The provider cannot currently establish guest readiness.
    Unavailable,
}

/// Closed reason for an unavailable or failed Cuttlefish guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CuttlefishUnavailableReason {
    /// The configured Cuttlefish provider did not answer.
    ProviderUnavailable,
    /// The admitted image cannot be supplied.
    ImageUnavailable,
    /// Placement lacks the capacity required by the VM profile.
    CapacityUnavailable,
    /// The guest failed to boot.
    GuestBootFailed,
    /// The guest booted but did not reach its readiness contract.
    GuestNotReady,
    /// The guest display/console transport is unavailable.
    TransportUnavailable,
    /// Retained evidence is outside the provider freshness window.
    ObservationStale,
}

/// Guest boot/readiness evidence retained alongside a VM lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CuttlefishGuestReadinessEvidence {
    /// Closed guest boot state.
    pub boot_state: CuttlefishGuestBootState,
    /// Closed guest readiness state.
    pub readiness: CuttlefishGuestReadiness,
    /// Exact closed reason when readiness is unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<CuttlefishUnavailableReason>,
}

impl CuttlefishGuestReadinessEvidence {
    /// Construct and validate guest boot/readiness evidence.
    pub fn new(
        boot_state: CuttlefishGuestBootState,
        readiness: CuttlefishGuestReadiness,
        unavailable_reason: Option<CuttlefishUnavailableReason>,
    ) -> Result<Self, CuttlefishContractError> {
        let evidence = Self {
            boot_state,
            readiness,
            unavailable_reason,
        };
        evidence.validate()?;
        Ok(evidence)
    }

    /// Validate the cross-field boot/readiness state machine.
    pub fn validate(&self) -> Result<(), CuttlefishContractError> {
        let valid_pair = matches!(
            (self.boot_state, self.readiness),
            (
                CuttlefishGuestBootState::Pending,
                CuttlefishGuestReadiness::Unknown | CuttlefishGuestReadiness::NotReady
            ) | (
                CuttlefishGuestBootState::Booting | CuttlefishGuestBootState::Stopped,
                CuttlefishGuestReadiness::NotReady
            ) | (
                CuttlefishGuestBootState::Ready,
                CuttlefishGuestReadiness::Ready
            ) | (
                CuttlefishGuestBootState::Failed | CuttlefishGuestBootState::Unavailable,
                CuttlefishGuestReadiness::Unavailable
            )
        );
        if !valid_pair {
            return Err(CuttlefishContractError::InvalidGuestEvidence);
        }

        if self.readiness == CuttlefishGuestReadiness::Unavailable
            && self.unavailable_reason.is_none()
        {
            return Err(CuttlefishContractError::MissingUnavailableReason);
        }
        if self.readiness != CuttlefishGuestReadiness::Unavailable
            && self.unavailable_reason.is_some()
        {
            return Err(CuttlefishContractError::UnexpectedUnavailableReason);
        }
        Ok(())
    }

    /// Whether this evidence is the fully admitted guest-ready pair.
    #[must_use]
    pub const fn is_ready(self) -> bool {
        matches!(
            (self.boot_state, self.readiness),
            (
                CuttlefishGuestBootState::Ready,
                CuttlefishGuestReadiness::Ready
            )
        ) && self.unavailable_reason.is_none()
    }
}

/// Closed lifecycle state of the outer Cuttlefish-backed Android VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CuttlefishVmLifecycleState {
    /// No outer VM has been provisioned for this stable identity.
    Absent,
    /// The provider is provisioning the outer VM or guest image.
    Provisioning,
    /// The outer VM exists but is intentionally stopped.
    Stopped,
    /// The outer VM and guest are booting.
    Starting,
    /// The outer VM and guest are serving the admitted guest contract.
    Running,
    /// The guest failed or the provider cannot currently serve it.
    Unavailable,
    /// A non-recoverable provider/guest failure was observed.
    Failed,
    /// The VM is restarting through the bounded reboot operation.
    Rebooting,
}

/// A provider observation for one stable Cuttlefish VM identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CuttlefishVmObservation {
    /// Provider schema discriminator.
    pub schema_version: u16,
    /// Stable VM target and image provenance being observed.
    pub target: CuttlefishVmTarget,
    /// Closed outer-VM lifecycle state.
    pub lifecycle_state: CuttlefishVmLifecycleState,
    /// Guest-owned boot/readiness evidence.
    pub guest: CuttlefishGuestReadinessEvidence,
    /// Monotonic provider generation for stale-action protection.
    pub generation: u64,
    /// Unix epoch milliseconds when this observation was produced.
    pub observed_at_unix_ms: u64,
}

impl CuttlefishVmObservation {
    /// Construct and validate a provider observation.
    pub fn new(
        target: CuttlefishVmTarget,
        lifecycle_state: CuttlefishVmLifecycleState,
        guest: CuttlefishGuestReadinessEvidence,
        generation: u64,
        observed_at_unix_ms: u64,
    ) -> Result<Self, CuttlefishContractError> {
        let observation = Self {
            schema_version: CUTTLEFISH_PROVIDER_SCHEMA_VERSION,
            target,
            lifecycle_state,
            guest,
            generation,
            observed_at_unix_ms,
        };
        observation.validate()?;
        Ok(observation)
    }

    /// Validate provider evidence without contacting Cuttlefish.
    pub fn validate(&self) -> Result<(), CuttlefishContractError> {
        if self.schema_version != CUTTLEFISH_PROVIDER_SCHEMA_VERSION {
            return Err(CuttlefishContractError::UnsupportedSchema(
                self.schema_version,
            ));
        }
        self.target.validate()?;
        self.guest.validate()?;
        if self.observed_at_unix_ms == 0 {
            return Err(CuttlefishContractError::InvalidTimestamp);
        }

        let valid_state = match self.lifecycle_state {
            CuttlefishVmLifecycleState::Absent => {
                self.generation == 0
                    && self.guest.boot_state == CuttlefishGuestBootState::Pending
                    && self.guest.readiness == CuttlefishGuestReadiness::Unknown
                    && self.guest.unavailable_reason.is_none()
            }
            CuttlefishVmLifecycleState::Provisioning => {
                self.generation > 0
                    && self.guest.boot_state == CuttlefishGuestBootState::Pending
                    && self.guest.readiness == CuttlefishGuestReadiness::NotReady
                    && self.guest.unavailable_reason.is_none()
            }
            CuttlefishVmLifecycleState::Stopped => {
                self.generation > 0
                    && self.guest.boot_state == CuttlefishGuestBootState::Stopped
                    && self.guest.readiness == CuttlefishGuestReadiness::NotReady
                    && self.guest.unavailable_reason.is_none()
            }
            CuttlefishVmLifecycleState::Starting | CuttlefishVmLifecycleState::Rebooting => {
                self.generation > 0
                    && self.guest.boot_state == CuttlefishGuestBootState::Booting
                    && self.guest.readiness == CuttlefishGuestReadiness::NotReady
                    && self.guest.unavailable_reason.is_none()
            }
            CuttlefishVmLifecycleState::Running => self.generation > 0 && self.guest.is_ready(),
            CuttlefishVmLifecycleState::Unavailable => {
                self.generation > 0
                    && self.guest.boot_state == CuttlefishGuestBootState::Unavailable
                    && self.guest.readiness == CuttlefishGuestReadiness::Unavailable
                    && self.guest.unavailable_reason.is_some()
            }
            CuttlefishVmLifecycleState::Failed => {
                self.generation > 0
                    && self.guest.boot_state == CuttlefishGuestBootState::Failed
                    && self.guest.readiness == CuttlefishGuestReadiness::Unavailable
                    && self.guest.unavailable_reason.is_some()
            }
        };
        if valid_state {
            Ok(())
        } else {
            Err(CuttlefishContractError::InvalidLifecycleState)
        }
    }

    /// Validate the observation against a consumer admission clock.
    pub fn validate_at(&self, now_unix_ms: u64) -> Result<(), CuttlefishContractError> {
        self.validate()?;
        if now_unix_ms < self.observed_at_unix_ms {
            return Err(CuttlefishContractError::InvalidTimestamp);
        }
        Ok(())
    }

    /// Admit an observation received across the provider state boundary.
    pub fn admitted(self) -> Result<Self, CuttlefishContractError> {
        self.validate()?;
        Ok(self)
    }

    /// Whether the observation is safe to hand to the package/session layer.
    #[must_use]
    pub fn is_guest_ready(&self) -> bool {
        self.validate().is_ok()
            && self.lifecycle_state == CuttlefishVmLifecycleState::Running
            && self.guest.is_ready()
    }
}

/// Closed lifecycle operations exposed by the Cuttlefish provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CuttlefishLifecycleOperation {
    /// Provision one target from its admitted image provenance.
    Provision,
    /// Start an existing stopped VM.
    Start,
    /// Stop an existing running VM.
    Stop,
    /// Reboot an existing running VM.
    Reboot,
    /// Destroy a stopped or failed VM.
    Destroy,
}

impl CuttlefishLifecycleOperation {
    /// Whether the operation destroys or restarts guest state.
    #[must_use]
    pub const fn is_destructive(self) -> bool {
        matches!(self, Self::Reboot | Self::Destroy)
    }

    /// Whether this operation is valid from the supplied observed state.
    #[must_use]
    pub const fn is_allowed_from(self, state: CuttlefishVmLifecycleState) -> bool {
        matches!(
            (self, state),
            (Self::Provision, CuttlefishVmLifecycleState::Absent)
                | (Self::Start, CuttlefishVmLifecycleState::Stopped)
                | (
                    Self::Stop | Self::Reboot,
                    CuttlefishVmLifecycleState::Starting
                        | CuttlefishVmLifecycleState::Running
                        | CuttlefishVmLifecycleState::Rebooting
                )
                | (
                    Self::Destroy,
                    CuttlefishVmLifecycleState::Stopped
                        | CuttlefishVmLifecycleState::Starting
                        | CuttlefishVmLifecycleState::Running
                        | CuttlefishVmLifecycleState::Rebooting
                        | CuttlefishVmLifecycleState::Unavailable
                        | CuttlefishVmLifecycleState::Failed
                )
        )
    }
}

/// A bounded, generation-checked lifecycle request for one Cuttlefish VM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CuttlefishLifecycleRequest {
    /// Lifecycle request schema discriminator.
    pub schema_version: u16,
    /// Stable request correlation identity.
    pub request_id: String,
    /// Stable VM target and image provenance.
    pub target: CuttlefishVmTarget,
    /// One closed lifecycle operation.
    pub operation: CuttlefishLifecycleOperation,
    /// Generation the caller observed before creating this request. Provision
    /// uses zero because the target must still be absent.
    pub expected_generation: u64,
}

impl CuttlefishLifecycleRequest {
    /// Construct and validate a bounded lifecycle request.
    pub fn new(
        request_id: impl Into<String>,
        target: CuttlefishVmTarget,
        operation: CuttlefishLifecycleOperation,
        expected_generation: u64,
    ) -> Result<Self, CuttlefishContractError> {
        let request = Self {
            schema_version: CUTTLEFISH_LIFECYCLE_SCHEMA_VERSION,
            request_id: request_id.into(),
            target,
            operation,
            expected_generation,
        };
        request.validate()?;
        Ok(request)
    }

    /// Validate request shape and operation/generation invariants.
    pub fn validate(&self) -> Result<(), CuttlefishContractError> {
        if self.schema_version != CUTTLEFISH_LIFECYCLE_SCHEMA_VERSION {
            return Err(CuttlefishContractError::UnsupportedSchema(
                self.schema_version,
            ));
        }
        validate_identity("request_id", &self.request_id)?;
        self.target.validate()?;
        match self.operation {
            CuttlefishLifecycleOperation::Provision if self.expected_generation == 0 => Ok(()),
            CuttlefishLifecycleOperation::Provision => {
                Err(CuttlefishContractError::InvalidGeneration)
            }
            _ if self.expected_generation > 0 => Ok(()),
            _ => Err(CuttlefishContractError::InvalidGeneration),
        }
    }

    /// Admit a request only when it is bound to the exact current VM target,
    /// image provenance, generation, and allowed lifecycle state.
    pub fn admitted_against(
        &self,
        current: &CuttlefishVmObservation,
    ) -> Result<(), CuttlefishContractError> {
        self.validate()?;
        current.validate()?;
        if self.target.vm_id != current.target.vm_id {
            return Err(CuttlefishContractError::TargetIdentityMismatch);
        }
        if self.target.image_provenance != current.target.image_provenance {
            return Err(CuttlefishContractError::ImageProvenanceMismatch);
        }
        if self.expected_generation != current.generation {
            return Err(CuttlefishContractError::GenerationMismatch {
                expected: self.expected_generation,
                actual: current.generation,
            });
        }
        if !self.operation.is_allowed_from(current.lifecycle_state) {
            return Err(CuttlefishContractError::OperationNotAllowed {
                operation: self.operation,
                state: current.lifecycle_state,
            });
        }
        Ok(())
    }

    /// Admit request shape without a current observation. A provider must use
    /// [`Self::admitted_against`] before executing any lifecycle mutation.
    pub fn admitted(self) -> Result<Self, CuttlefishContractError> {
        self.validate()?;
        Ok(self)
    }
}

fn validate_identity(field: &'static str, value: &str) -> Result<(), CuttlefishContractError> {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || value.trim() != value
        || !value.is_ascii()
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
        })
    {
        if value.len() > MAX_ID_BYTES {
            return Err(CuttlefishContractError::FieldTooLong(field));
        }
        return Err(CuttlefishContractError::InvalidField(field));
    }
    Ok(())
}

fn is_valid_sha256_digest(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64
        && hex
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        && hex.bytes().any(|byte| byte != b'0')
}

fn is_valid_mesh_host(value: &str) -> bool {
    value.len() <= MAX_ID_BYTES
        && value
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension == "mesh")
        && value.split('.').all(|label| {
            let bytes = label.as_bytes();
            !bytes.is_empty()
                && bytes.len() <= 63
                && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
                && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
                && bytes
                    .iter()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn image() -> CuttlefishImageProvenanceRef {
        CuttlefishImageProvenanceRef::new(
            "aosp-cuttlefish-2026-08",
            DIGEST,
            "aosp-source-2026-08",
            "starter-catalog-v1",
        )
        .expect("valid image provenance")
    }

    fn target() -> CuttlefishVmTarget {
        CuttlefishVmTarget::new(
            CuttlefishVmId::new("android-vm-01").expect("valid VM id"),
            image(),
        )
        .expect("valid VM target")
    }

    fn evidence(
        boot_state: CuttlefishGuestBootState,
        readiness: CuttlefishGuestReadiness,
        unavailable_reason: Option<CuttlefishUnavailableReason>,
    ) -> CuttlefishGuestReadinessEvidence {
        CuttlefishGuestReadinessEvidence::new(boot_state, readiness, unavailable_reason)
            .expect("valid guest evidence")
    }

    fn observation(
        lifecycle_state: CuttlefishVmLifecycleState,
        guest: CuttlefishGuestReadinessEvidence,
        generation: u64,
    ) -> CuttlefishVmObservation {
        CuttlefishVmObservation::new(
            target(),
            lifecycle_state,
            guest,
            generation,
            1_786_000_000_000,
        )
        .expect("valid provider observation")
    }

    #[test]
    fn lifecycle_request_round_trips_without_command_surface() {
        let request = CuttlefishLifecycleRequest::new(
            "request-01",
            target(),
            CuttlefishLifecycleOperation::Start,
            7,
        )
        .expect("valid lifecycle request");
        let body = serde_json::to_string(&request).expect("serialize lifecycle request");
        assert!(!body.contains("command"));
        assert!(!body.contains("adb"));
        assert!(!body.contains("/"));

        let decoded: CuttlefishLifecycleRequest =
            serde_json::from_str(&body).expect("deserialize lifecycle request");
        assert_eq!(decoded.admitted().expect("admitted request"), request);
    }

    #[test]
    fn identity_and_image_provenance_are_bounded_and_fail_closed() {
        assert_eq!(
            CuttlefishVmId::new("../android-vm"),
            Err(CuttlefishContractError::InvalidField("vm_id"))
        );
        assert_eq!(
            CuttlefishImageProvenanceRef::new(
                "aosp-cuttlefish",
                "sha256:ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
                "aosp-source",
                "starter-catalog-v1",
            ),
            Err(CuttlefishContractError::InvalidImageDigest)
        );
        assert_eq!(
            CuttlefishImageProvenanceRef::new(
                "https://registry.example/aosp",
                DIGEST,
                "aosp-source",
                "starter-catalog-v1",
            ),
            Err(CuttlefishContractError::InvalidField("image_id"))
        );
    }

    #[test]
    fn hostile_vdi_host_alias_cannot_cross_the_canonical_mesh_authority_boundary() {
        let source = |mesh_host: &str| AndroidVdiSource {
            schema_version: ANDROID_VDI_SOURCE_SCHEMA_VERSION,
            workload_id: target().vm_id.as_str().to_owned(),
            image_provenance: image(),
            catalog_digest: DIGEST.to_owned(),
            generation: 7,
            protocol: AndroidVdiProtocol::WebRtc,
            mesh_host: mesh_host.to_owned(),
            port: 8443,
            session_id: "guest-session-7".to_owned(),
            observed_at_unix_ms: 1_786_000_000_000,
            expires_at_unix_ms: 1_786_000_060_000,
        };

        assert_eq!(source("android-01.workstation.mesh").validate(), Ok(()));
        for hostile_host in [
            "Android-01.workstation.mesh",
            "-android-01.workstation.mesh",
            "android-01-.workstation.mesh",
            "android-01.workstation.mesh.evil",
        ] {
            assert_eq!(
                source(hostile_host).validate(),
                Err(CuttlefishContractError::InvalidField("mesh_host")),
                "host alias must fail closed: {hostile_host}"
            );
        }
    }

    #[test]
    fn guest_boot_and_readiness_require_a_consistent_pair() {
        assert_eq!(
            CuttlefishGuestReadinessEvidence::new(
                CuttlefishGuestBootState::Ready,
                CuttlefishGuestReadiness::NotReady,
                None,
            ),
            Err(CuttlefishContractError::InvalidGuestEvidence)
        );
        assert_eq!(
            CuttlefishGuestReadinessEvidence::new(
                CuttlefishGuestBootState::Failed,
                CuttlefishGuestReadiness::Unavailable,
                None,
            ),
            Err(CuttlefishContractError::MissingUnavailableReason)
        );

        let ready = evidence(
            CuttlefishGuestBootState::Ready,
            CuttlefishGuestReadiness::Ready,
            None,
        );
        let running = observation(CuttlefishVmLifecycleState::Running, ready, 7);
        assert!(running.is_guest_ready());

        let unavailable = evidence(
            CuttlefishGuestBootState::Unavailable,
            CuttlefishGuestReadiness::Unavailable,
            Some(CuttlefishUnavailableReason::ProviderUnavailable),
        );
        let provider_down = observation(CuttlefishVmLifecycleState::Unavailable, unavailable, 8);
        assert!(!provider_down.is_guest_ready());
    }

    #[test]
    fn observation_lifecycle_state_cannot_claim_false_guest_readiness() {
        let booting = evidence(
            CuttlefishGuestBootState::Booting,
            CuttlefishGuestReadiness::NotReady,
            None,
        );
        assert_eq!(
            CuttlefishVmObservation::new(
                target(),
                CuttlefishVmLifecycleState::Running,
                booting,
                7,
                1_786_000_000_000,
            ),
            Err(CuttlefishContractError::InvalidLifecycleState)
        );

        let absent = observation(
            CuttlefishVmLifecycleState::Absent,
            evidence(
                CuttlefishGuestBootState::Pending,
                CuttlefishGuestReadiness::Unknown,
                None,
            ),
            0,
        );
        assert!(!absent.is_guest_ready());
    }

    #[test]
    fn lifecycle_admission_binds_target_image_generation_and_state() {
        let current = observation(
            CuttlefishVmLifecycleState::Stopped,
            evidence(
                CuttlefishGuestBootState::Stopped,
                CuttlefishGuestReadiness::NotReady,
                None,
            ),
            7,
        );
        let start = CuttlefishLifecycleRequest::new(
            "request-start",
            target(),
            CuttlefishLifecycleOperation::Start,
            7,
        )
        .expect("valid start request");
        assert_eq!(start.admitted_against(&current), Ok(()));

        let stale = CuttlefishLifecycleRequest::new(
            "request-stale",
            target(),
            CuttlefishLifecycleOperation::Start,
            6,
        )
        .expect("bounded stale request");
        assert_eq!(
            stale.admitted_against(&current),
            Err(CuttlefishContractError::GenerationMismatch {
                expected: 6,
                actual: 7
            })
        );

        let wrong_state = CuttlefishLifecycleRequest::new(
            "request-stop",
            target(),
            CuttlefishLifecycleOperation::Stop,
            7,
        )
        .expect("bounded stop request");
        assert_eq!(
            wrong_state.admitted_against(&current),
            Err(CuttlefishContractError::OperationNotAllowed {
                operation: CuttlefishLifecycleOperation::Stop,
                state: CuttlefishVmLifecycleState::Stopped
            })
        );
    }

    #[test]
    fn lifecycle_admission_rejects_image_drift_and_requires_provision_from_absent() {
        let current = observation(
            CuttlefishVmLifecycleState::Absent,
            evidence(
                CuttlefishGuestBootState::Pending,
                CuttlefishGuestReadiness::Unknown,
                None,
            ),
            0,
        );
        let provision = CuttlefishLifecycleRequest::new(
            "request-provision",
            target(),
            CuttlefishLifecycleOperation::Provision,
            0,
        )
        .expect("valid provision request");
        assert_eq!(provision.admitted_against(&current), Ok(()));

        let mut changed_target = target();
        changed_target.image_provenance.image_id = "aosp-cuttlefish-other".into();
        let drifted = CuttlefishLifecycleRequest::new(
            "request-drifted",
            changed_target,
            CuttlefishLifecycleOperation::Provision,
            0,
        )
        .expect("bounded drifted request");
        assert_eq!(
            drifted.admitted_against(&current),
            Err(CuttlefishContractError::ImageProvenanceMismatch)
        );

        assert_eq!(
            CuttlefishLifecycleRequest::new(
                "request-invalid-provision",
                target(),
                CuttlefishLifecycleOperation::Provision,
                1,
            ),
            Err(CuttlefishContractError::InvalidGeneration)
        );
    }

    #[test]
    fn unknown_wire_fields_and_unsupported_schema_fail_closed() {
        let request = CuttlefishLifecycleRequest::new(
            "request-unknown",
            target(),
            CuttlefishLifecycleOperation::Start,
            7,
        )
        .expect("valid request");
        let body = serde_json::to_string(&request).expect("serialize request");
        let hostile = body.replacen('{', r#"{"command":"adb shell" ,"#, 1);
        assert!(serde_json::from_str::<CuttlefishLifecycleRequest>(&hostile).is_err());

        let mut unsupported = request;
        unsupported.schema_version = CUTTLEFISH_LIFECYCLE_SCHEMA_VERSION + 1;
        assert_eq!(
            unsupported.admitted(),
            Err(CuttlefishContractError::UnsupportedSchema(
                CUTTLEFISH_LIFECYCLE_SCHEMA_VERSION + 1
            ))
        );
    }
}
