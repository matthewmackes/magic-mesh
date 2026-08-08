//! WL-ARCH-010 — the single workload operation and state contract.
//!
//! This is deliberately a small, closed wire boundary.  The shell submits one
//! idempotent operation, `mackesd-compute` persists and reconciles it, and the
//! resulting projection is published on `state/workloads/<node>`.  VM/container
//! implementation details (libvirt, Quadlet, QEMU, or a recovery transport) do
//! not cross this boundary.

#![allow(
    missing_docs,
    reason = "public field names and closed variants are the versioned wire contract"
)]

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use std::collections::BTreeSet;
use std::fmt;

/// The only workload contract admitted by this crate.
pub const WORKLOAD_CONTRACT_SCHEMA_VERSION: u16 = 1;
/// Operations are submitted to this topic and deduplicated by `request_id`.
pub const WORKLOAD_OPERATION_TOPIC: &str = "action/workload/operation";
/// Prefix for the authoritative per-node workload projection.
pub const WORKLOAD_STATE_TOPIC_PREFIX: &str = "state/workloads/";
/// Maximum JSON body admitted by the bounded decoders.
pub const MAX_WORKLOAD_WIRE_BYTES: usize = 256 * 1024;
/// Maximum identifier size in bytes.
pub const MAX_WORKLOAD_IDENTIFIER_BYTES: usize = 128;
/// Maximum human diagnostic size in bytes.
pub const MAX_WORKLOAD_TEXT_BYTES: usize = 512;
/// Maximum number of workloads in one node projection.
pub const MAX_WORKLOADS_PER_NODE: usize = 256;
/// Maximum number of attachment leases in one workload projection.
pub const MAX_WORKLOAD_ATTACHMENTS: usize = 8;
/// Maximum operation deadline admitted by the API.
pub const MAX_WORKLOAD_DEADLINE_MS: u64 = 15 * 60 * 1_000;
/// Minimum host CPU reserve, even on a one-thread node.
pub const MIN_HOST_CPU_RESERVE: u16 = 1;
/// Minimum host memory reserve for the shell and daemon.
pub const MIN_HOST_MEMORY_RESERVE_MB: u32 = 2_048;

/// A validation/admission failure at the workload wire boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkloadContractError {
    /// The encoded body exceeded the bounded decoder allocation.
    PayloadTooLarge,
    /// The body was not valid JSON for the closed contract.
    MalformedWire,
    /// A top-level record used a schema this binary does not understand.
    UnsupportedSchema(u16),
    /// A bounded value was empty, malformed, or used a forbidden grammar.
    InvalidField(&'static str),
    /// A bounded value exceeded its wire limit.
    FieldTooLong(&'static str),
    /// A collection exceeded its wire limit.
    CapacityExceeded(&'static str),
    /// A generation/deadline/expiry was not admitted.
    InvalidNumber(&'static str),
    /// The requested operation cannot follow the observed phase.
    InvalidTransition,
}

impl fmt::Display for WorkloadContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge => f.write_str("workload body is too large"),
            Self::MalformedWire => f.write_str("malformed workload body"),
            Self::UnsupportedSchema(version) => {
                write!(f, "unsupported workload schema version {version}")
            }
            Self::InvalidField(field) => write!(f, "invalid workload field: {field}"),
            Self::FieldTooLong(field) => write!(f, "workload field is too long: {field}"),
            Self::CapacityExceeded(field) => write!(f, "workload collection is too large: {field}"),
            Self::InvalidNumber(field) => write!(f, "invalid workload number: {field}"),
            Self::InvalidTransition => f.write_str("invalid workload operation transition"),
        }
    }
}

impl std::error::Error for WorkloadContractError {}

fn check_schema(version: u16) -> Result<(), WorkloadContractError> {
    (version == WORKLOAD_CONTRACT_SCHEMA_VERSION)
        .then_some(())
        .ok_or(WorkloadContractError::UnsupportedSchema(version))
}

fn check_identifier(value: &str, field: &'static str) -> Result<(), WorkloadContractError> {
    if value.is_empty() {
        return Err(WorkloadContractError::InvalidField(field));
    }
    if value.len() > MAX_WORKLOAD_IDENTIFIER_BYTES {
        return Err(WorkloadContractError::FieldTooLong(field));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
    {
        return Err(WorkloadContractError::InvalidField(field));
    }
    Ok(())
}

fn check_text(value: &str, field: &'static str) -> Result<(), WorkloadContractError> {
    if value.trim().is_empty() {
        return Err(WorkloadContractError::InvalidField(field));
    }
    if value.len() > MAX_WORKLOAD_TEXT_BYTES {
        return Err(WorkloadContractError::FieldTooLong(field));
    }
    if value.chars().any(char::is_control) {
        return Err(WorkloadContractError::InvalidField(field));
    }
    Ok(())
}

struct DuplicateKeySeed;

impl<'de> DeserializeSeed<'de> for DuplicateKeySeed {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(DuplicateKeyVisitor)
    }
}

struct DuplicateKeyVisitor;

impl<'de> Visitor<'de> for DuplicateKeyVisitor {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_string<E>(self, _value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(self)
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut keys = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key) {
                return Err(de::Error::custom("duplicate JSON object key"));
            }
            map.next_value_seed(DuplicateKeySeed)?;
        }
        Ok(())
    }

    fn visit_seq<S>(self, mut sequence: S) -> Result<Self::Value, S::Error>
    where
        S: SeqAccess<'de>,
    {
        while sequence.next_element_seed(DuplicateKeySeed)?.is_some() {}
        Ok(())
    }
}

/// Reject duplicate object keys recursively before decoding a workload record.
pub fn reject_duplicate_json_keys(body: &str) -> Result<(), WorkloadContractError> {
    let mut deserializer = serde_json::Deserializer::from_str(body);
    deserializer
        .deserialize_any(DuplicateKeyVisitor)
        .map_err(|_| WorkloadContractError::MalformedWire)?;
    deserializer
        .end()
        .map_err(|_| WorkloadContractError::MalformedWire)
}

/// Stable workload identity.  Raw paths, URLs, commands, and secrets are not
/// valid identities.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkloadId(String);

impl WorkloadId {
    /// Construct and validate an identity.
    pub fn new(value: impl Into<String>) -> Result<Self, WorkloadContractError> {
        let value = value.into();
        check_identifier(&value, "workload_id")?;
        Ok(Self(value))
    }

    /// Borrow the stable wire spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the identity into its wire spelling.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl Serialize for WorkloadId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for WorkloadId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        check_identifier(&value, "workload_id").map_err(de::Error::custom)?;
        Ok(Self(value))
    }
}

/// The two runtime authorities admitted by the workload API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadKind {
    /// A libvirt/virtqemud domain.
    Vm,
    /// A systemd/Quadlet-managed container.
    Container,
}

/// The sole host authority allowed to actuate a workload kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadBackend {
    /// libvirt/virtqemud owns the domain definition and power state.
    LibvirtVirtqemud,
    /// Quadlet/systemd owns the managed container unit and power state.
    QuadletSystemd,
}

impl WorkloadBackend {
    /// Whether this backend realizes a virtual machine.
    #[must_use]
    pub const fn is_vm(self) -> bool {
        matches!(self, Self::LibvirtVirtqemud)
    }
}

/// Desired power state.  Readiness is reported independently by the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadPowerState {
    Defined,
    Starting,
    Running,
    Paused,
    Stopping,
    Stopped,
    Failed,
}

/// Guest/service readiness, never inferred from a transport connection alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadReadiness {
    #[default]
    Unknown,
    WaitingForPlacement,
    WaitingForGuest,
    WaitingForService,
    PreparingDisplay,
    Ready,
    Degraded,
    Unavailable,
    Failed,
}

/// Independent health signal for a workload projection.  Health is not
/// inferred from power or transport attachment; the adapter may report a
/// degraded guest while its VM is still running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadHealth {
    #[default]
    Unknown,
    Healthy,
    Degraded,
    Failed,
}

/// Host-pressure signal kept separate from workload progress and failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadPressure {
    #[default]
    Normal,
    Constrained,
    Saturated,
}

/// Explicit runtime observations exposed in the authoritative projection.
/// Each dimension is independently replaceable by an adapter; a UI must not
/// collapse guest-agent, network, service, display, application, health, or
/// host-pressure state into one optimistic "running" bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct WorkloadRuntimeSignals {
    pub guest_agent: WorkloadReadiness,
    pub network: WorkloadReadiness,
    pub service: WorkloadReadiness,
    pub display: WorkloadReadiness,
    pub application: WorkloadReadiness,
    pub health: WorkloadHealth,
    pub pressure: WorkloadPressure,
    pub progress_percent: u8,
}

impl WorkloadRuntimeSignals {
    /// Derive a conservative baseline from an adapter readiness result.  Real
    /// adapters may replace individual dimensions; unknown remains honest.
    #[must_use]
    pub fn from_readiness(phase: WorkloadOperationPhase, readiness: WorkloadReadiness) -> Self {
        let health = match readiness {
            WorkloadReadiness::Ready => WorkloadHealth::Healthy,
            WorkloadReadiness::Degraded => WorkloadHealth::Degraded,
            WorkloadReadiness::Failed | WorkloadReadiness::Unavailable => {
                WorkloadHealth::Failed
            }
            _ => WorkloadHealth::Unknown,
        };
        let progress_percent = match phase {
            WorkloadOperationPhase::Queued => 0,
            WorkloadOperationPhase::Validating => 10,
            WorkloadOperationPhase::Admitting => 20,
            WorkloadOperationPhase::Defining => 35,
            WorkloadOperationPhase::Starting => 50,
            WorkloadOperationPhase::WaitingForGuest => 60,
            WorkloadOperationPhase::WaitingForService => 72,
            WorkloadOperationPhase::PreparingDisplay => 84,
            WorkloadOperationPhase::WaitingForFirstFrame => 92,
            WorkloadOperationPhase::Ready | WorkloadOperationPhase::Completed => 100,
            WorkloadOperationPhase::Stopping => 60,
            WorkloadOperationPhase::Failed | WorkloadOperationPhase::Cancelled => 100,
        };
        let service = if matches!(phase, WorkloadOperationPhase::WaitingForService) {
            WorkloadReadiness::WaitingForService
        } else {
            readiness
        };
        let display = if matches!(
            phase,
            WorkloadOperationPhase::PreparingDisplay | WorkloadOperationPhase::WaitingForFirstFrame
        ) {
            readiness
        } else {
            WorkloadReadiness::Unknown
        };
        Self {
            guest_agent: readiness,
            network: WorkloadReadiness::Unknown,
            service,
            display,
            application: readiness,
            health,
            pressure: WorkloadPressure::Normal,
            progress_percent,
        }
    }
}

/// API operation accepted by the sole reconciler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadOperationAction {
    /// Start/reconcile the workload and return a usable attachment lease.
    StartAndAttach,
    Start,
    Stop,
    Restart,
    /// Permanently stop the managed domain/unit; cleanup remains adapter-owned.
    Destroy,
    Pause,
    Resume,
    Open,
    Reconcile,
    Cancel,
}

/// Reconciler phase, used for honest progress and recovery after a restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadOperationPhase {
    Queued,
    Validating,
    Admitting,
    Defining,
    Starting,
    WaitingForGuest,
    WaitingForService,
    PreparingDisplay,
    WaitingForFirstFrame,
    Ready,
    Stopping,
    Completed,
    Failed,
    Cancelled,
}

impl WorkloadOperationPhase {
    /// Whether no further side effect should be attempted for this phase.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// Attachment type returned by `StartAndAttach`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadAttachmentProtocol {
    /// Local QEMU Display1 peer-to-peer D-Bus + DMA-BUF attachment.
    QemuDisplay1Dmabuf,
    /// Independent remote/recovery transports.
    Rdp,
    Spice,
    Vnc,
    Sunshine,
    WebRtc,
    Logs,
    Terminal,
    Ports,
}

/// Bounded resource request. Admission is host-owned; callers cannot select
/// arbitrary CPU pinning, host paths, or cgroup names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadResources {
    /// Guest vCPU count (the host reserve is applied separately).
    pub vcpu: u16,
    /// Guest memory in MiB.
    pub memory_mb: u32,
    /// Root disk size in GiB.
    pub disk_gb: u32,
}

impl WorkloadResources {
    /// Validate basic resource bounds before host-specific admission.
    pub fn validate(&self) -> Result<(), WorkloadContractError> {
        if !(1..=64).contains(&self.vcpu) {
            return Err(WorkloadContractError::InvalidNumber("vcpu"));
        }
        if !(512..=262_144).contains(&self.memory_mb) {
            return Err(WorkloadContractError::InvalidNumber("memory_mb"));
        }
        if !(1..=4_096).contains(&self.disk_gb) {
            return Err(WorkloadContractError::InvalidNumber("disk_gb"));
        }
        Ok(())
    }
}

/// Product profiles exposed by the one-click GUI.  The profile owns the safe
/// starting point; a later resize is a typed operation, not arbitrary fields in
/// a shell command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadProfile {
    /// Small-host default: two vCPUs and four GiB.
    Small,
    /// Interactive desktop/browser profile: four vCPUs and eight GiB.
    Standard,
}

impl WorkloadProfile {
    /// Safe resources for this profile.
    #[must_use]
    pub const fn resources(self) -> WorkloadResources {
        match self {
            Self::Small => WorkloadResources {
                vcpu: 2,
                memory_mb: 4_096,
                disk_gb: 32,
            },
            Self::Standard => WorkloadResources {
                vcpu: 4,
                memory_mb: 8_192,
                disk_gb: 64,
            },
        }
    }
}

/// Host capacity used by admission.  The reconciler supplies live values from
/// the node probe; no caller may claim capacity by editing this record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostCapacity {
    pub logical_cpus: u16,
    pub memory_mb: u32,
    pub allocated_vcpu: u16,
    pub allocated_memory_mb: u32,
    /// Total usable bytes in the managed `mde-vms` pool, in GiB.
    #[serde(default)]
    pub storage_gb: u32,
    /// Disk already reserved by non-terminal operations, in GiB.
    #[serde(default)]
    pub allocated_storage_gb: u32,
}

/// Backend-specific storage pools used by typed workload admission.
///
/// VM disks and Quadlet container state do not necessarily live on the same
/// filesystem.  A reconciler must probe and reserve the pool belonging to the
/// requested backend rather than applying the VM pool measurement to every
/// workload kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadStorageCapacity {
    /// Total usable VM-disk pool capacity, in GiB.
    pub vm_storage_gb: u32,
    /// VM-disk capacity already reserved by non-terminal operations, in GiB.
    pub allocated_vm_storage_gb: u32,
    /// Total usable container state/image pool capacity, in GiB.
    pub container_storage_gb: u32,
    /// Container capacity already reserved by non-terminal operations, in GiB.
    pub allocated_container_storage_gb: u32,
}

impl WorkloadStorageCapacity {
    /// Select the total and reserved storage for one typed backend.
    #[must_use]
    pub const fn for_backend(self, backend: WorkloadBackend) -> (u32, u32) {
        match backend {
            WorkloadBackend::LibvirtVirtqemud => {
                (self.vm_storage_gb, self.allocated_vm_storage_gb)
            }
            WorkloadBackend::QuadletSystemd => (
                self.container_storage_gb,
                self.allocated_container_storage_gb,
            ),
        }
    }
}

/// Why a workload was refused by host admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionDenial {
    InvalidHost,
    InvalidRequest,
    CpuReserve,
    MemoryReserve,
    StorageReserve,
}

/// Honest admission result, including capacity left after the mandatory host
/// reserve.  The GUI can render this directly without guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkloadAdmission {
    pub admitted: bool,
    pub denial: Option<AdmissionDenial>,
    pub available_vcpu: u16,
    pub available_memory_mb: u32,
    pub available_storage_gb: u32,
}

/// Calculate the host reserve required by WL-ARCH-010: at least one logical
/// CPU and 25% of CPUs, plus at least 2 GiB and 20% of memory.
#[must_use]
pub const fn host_reserve(logical_cpus: u16, memory_mb: u32) -> (u16, u32) {
    let cpu_quarter = (logical_cpus.saturating_add(3)) / 4;
    let cpu = if cpu_quarter > MIN_HOST_CPU_RESERVE {
        cpu_quarter
    } else {
        MIN_HOST_CPU_RESERVE
    };
    let memory_fifth = memory_mb.saturating_add(4) / 5;
    let memory = if memory_fifth > MIN_HOST_MEMORY_RESERVE_MB {
        memory_fifth
    } else {
        MIN_HOST_MEMORY_RESERVE_MB
    };
    (cpu, memory)
}

/// Admit one resource request without allowing the guest pool to consume the
/// shell/daemon reserve.
#[must_use]
pub fn admit_workload(resources: WorkloadResources, host: HostCapacity) -> WorkloadAdmission {
    admit_workload_with_storage(
        resources,
        host,
        host.storage_gb,
        host.allocated_storage_gb,
    )
}

/// Admit a workload against the storage pool owned by its typed backend.
///
/// `HostCapacity` remains the compatibility shape for the existing VM
/// admission call sites. New reconciler paths must use this function with a
/// `WorkloadStorageCapacity` probe so Quadlet/container admission cannot be
/// coupled accidentally to `/var/lib/mde-vms`.
#[must_use]
pub fn admit_workload_for_backend(
    resources: WorkloadResources,
    backend: WorkloadBackend,
    host: HostCapacity,
    storage: WorkloadStorageCapacity,
) -> WorkloadAdmission {
    let (storage_gb, allocated_storage_gb) = storage.for_backend(backend);
    admit_workload_with_storage(resources, host, storage_gb, allocated_storage_gb)
}

fn admit_workload_with_storage(
    resources: WorkloadResources,
    host: HostCapacity,
    storage_gb: u32,
    allocated_storage_gb: u32,
) -> WorkloadAdmission {
    let (cpu_reserve, memory_reserve) = host_reserve(host.logical_cpus, host.memory_mb);
    if resources.validate().is_err() {
        return WorkloadAdmission {
            admitted: false,
            denial: Some(AdmissionDenial::InvalidRequest),
            available_vcpu: 0,
            available_memory_mb: 0,
            available_storage_gb: 0,
        };
    }
    if host.logical_cpus == 0 || host.memory_mb == 0 {
        return WorkloadAdmission {
            admitted: false,
            denial: Some(AdmissionDenial::InvalidHost),
            available_vcpu: 0,
            available_memory_mb: 0,
            available_storage_gb: 0,
        };
    }
    let available_vcpu = host
        .logical_cpus
        .saturating_sub(cpu_reserve)
        .saturating_sub(host.allocated_vcpu);
    let available_memory_mb = host
        .memory_mb
        .saturating_sub(memory_reserve)
        .saturating_sub(host.allocated_memory_mb);
    let available_storage_gb = storage_gb.saturating_sub(allocated_storage_gb);
    let denial = if resources.vcpu > available_vcpu {
        Some(AdmissionDenial::CpuReserve)
    } else if resources.memory_mb > available_memory_mb {
        Some(AdmissionDenial::MemoryReserve)
    } else if resources.disk_gb > available_storage_gb {
        Some(AdmissionDenial::StorageReserve)
    } else {
        None
    };
    WorkloadAdmission {
        admitted: denial.is_none(),
        denial,
        available_vcpu,
        available_memory_mb,
        available_storage_gb,
    }
}

/// Persisted desired state.  `generation` is assigned by the reconciler and
/// must advance only after the journal record is durable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadDesiredState {
    pub schema_version: u16,
    pub workload_id: WorkloadId,
    pub kind: WorkloadKind,
    pub backend: WorkloadBackend,
    pub node: String,
    pub desired_power: WorkloadPowerState,
    pub resources: WorkloadResources,
    pub generation: u64,
    /// Approved image/catalog reference, never a host path.
    pub image_ref: Option<String>,
}

impl WorkloadDesiredState {
    /// Validate the persisted desired-state record.
    pub fn validate(&self) -> Result<(), WorkloadContractError> {
        check_schema(self.schema_version)?;
        check_identifier(&self.node, "node")?;
        self.resources.validate()?;
        if self.generation == 0 {
            return Err(WorkloadContractError::InvalidNumber("generation"));
        }
        if let Some(image_ref) = &self.image_ref {
            check_identifier(image_ref, "image_ref")?;
        }
        Ok(())
    }
}

/// One idempotent API operation.  Replaying the same `request_id` must return
/// the persisted status and must not repeat a side effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadOperationRequest {
    pub schema_version: u16,
    pub request_id: String,
    pub workload_id: WorkloadId,
    pub backend: WorkloadBackend,
    pub resources: WorkloadResources,
    /// Approved catalog image reference (name:version), never a host path.
    /// StartAndAttach requires this when the adapter must define a new VM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<String>,
    /// Node-local reconciler target; a worker must ignore operations for peers.
    pub target_node: String,
    /// Zero means “create the first desired generation”; otherwise this is a
    /// compare-and-swap guard against stale GUI state.
    pub expected_generation: u64,
    pub action: WorkloadOperationAction,
    /// Existing operation targeted by `Cancel`; cancellation never applies to
    /// the cancel request itself or to an implicit "current" operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_request_id: Option<String>,
    /// Absolute wall-clock deadline in milliseconds since Unix epoch.
    pub deadline_at_ms: u64,
    /// Optional preference; the reconciler may fall back to recovery.
    pub preferred_attachment: Option<WorkloadAttachmentProtocol>,
    /// Short-lived exact-body capability.  The digest excludes this field;
    /// the reconciler still requires and consumes it before mutation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub armed_token: Option<String>,
}

impl WorkloadOperationRequest {
    /// Validate admission-independent request invariants.
    pub fn validate(&self, now_ms: u64) -> Result<(), WorkloadContractError> {
        check_schema(self.schema_version)?;
        check_identifier(&self.request_id, "request_id")?;
        check_identifier(&self.target_node, "target_node")?;
        self.resources.validate()?;
        if let Some(image_ref) = &self.image_ref {
            check_identifier(image_ref, "image_ref")?;
        }
        match self.action {
            WorkloadOperationAction::Cancel => {
                let target = self
                    .target_request_id
                    .as_deref()
                    .ok_or(WorkloadContractError::InvalidField("target_request_id"))?;
                check_identifier(target, "target_request_id")?;
                if target == self.request_id {
                    return Err(WorkloadContractError::InvalidTransition);
                }
                if self.expected_generation == 0 {
                    return Err(WorkloadContractError::InvalidNumber("expected_generation"));
                }
            }
            _ if self.target_request_id.is_some() => {
                return Err(WorkloadContractError::InvalidField("target_request_id"));
            }
            _ => {}
        }
        if self.deadline_at_ms <= now_ms
            || self.deadline_at_ms.saturating_sub(now_ms) > MAX_WORKLOAD_DEADLINE_MS
        {
            return Err(WorkloadContractError::InvalidNumber("deadline_at_ms"));
        }
        Ok(())
    }

    /// Decode and validate a bounded JSON request.
    pub fn from_json(body: &str, now_ms: u64) -> Result<Self, WorkloadContractError> {
        if body.len() > MAX_WORKLOAD_WIRE_BYTES {
            return Err(WorkloadContractError::PayloadTooLarge);
        }
        reject_duplicate_json_keys(body)?;
        let request: Self =
            serde_json::from_str(body).map_err(|_| WorkloadContractError::MalformedWire)?;
        request.validate(now_ms)?;
        Ok(request)
    }
}

/// A lease for a client attachment.  Endpoint addresses and file descriptors
/// stay node-local; the wire record only names the brokered lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadAttachmentLease {
    pub schema_version: u16,
    pub lease_id: String,
    /// One-use broker nonce bound to this lease and never reused after attach.
    pub nonce: String,
    pub workload_id: WorkloadId,
    pub generation: u64,
    pub protocol: WorkloadAttachmentProtocol,
    pub expires_at_ms: u64,
}

impl WorkloadAttachmentLease {
    /// Validate a lease before it is published or handed to the shell.
    pub fn validate(&self, now_ms: u64) -> Result<(), WorkloadContractError> {
        check_schema(self.schema_version)?;
        check_identifier(&self.lease_id, "lease_id")?;
        check_identifier(&self.nonce, "nonce")?;
        if self.generation == 0 || self.expires_at_ms <= now_ms {
            return Err(WorkloadContractError::InvalidNumber("lease"));
        }
        Ok(())
    }
}

/// Persisted operation status.  This is the only state the GUI needs to drive
/// its one-click/no-scroll workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadOperationStatus {
    pub schema_version: u16,
    pub request_id: String,
    pub workload_id: WorkloadId,
    pub backend: WorkloadBackend,
    pub resources: WorkloadResources,
    /// Approved catalog image carried through the authoritative projection.
    /// This is presentation metadata only; the reconciler still owns the
    /// backend definition and power side effects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<String>,
    pub generation: u64,
    pub phase: WorkloadOperationPhase,
    pub power: WorkloadPowerState,
    pub readiness: WorkloadReadiness,
    /// Independent guest/network/service/display/application/health/pressure
    /// observations and bounded progress for the current operation.
    #[serde(default)]
    pub signals: WorkloadRuntimeSignals,
    pub retryable: bool,
    /// Number of adapter attempts already made for this operation.
    #[serde(default)]
    pub attempt: u16,
    /// Earliest wall-clock time at which a retry may run. Zero means now.
    #[serde(default)]
    pub next_retry_at_ms: u64,
    pub reason: Option<String>,
    pub remediation: Option<String>,
    pub attachment: Option<WorkloadAttachmentLease>,
}

impl WorkloadOperationStatus {
    /// Validate bounded status data before publication.
    pub fn validate(&self, now_ms: u64) -> Result<(), WorkloadContractError> {
        check_schema(self.schema_version)?;
        check_identifier(&self.request_id, "request_id")?;
        self.resources.validate()?;
        if let Some(image_ref) = &self.image_ref {
            check_identifier(image_ref, "image_ref")?;
        }
        if self.generation == 0 {
            return Err(WorkloadContractError::InvalidNumber("generation"));
        }
        if self.attempt > 32 {
            return Err(WorkloadContractError::InvalidNumber("attempt"));
        }
        if self.signals.progress_percent > 100 {
            return Err(WorkloadContractError::InvalidNumber("progress_percent"));
        }
        if self.phase == WorkloadOperationPhase::Failed && self.reason.is_none() {
            return Err(WorkloadContractError::InvalidField("reason"));
        }
        for (field, value) in [("reason", &self.reason), ("remediation", &self.remediation)] {
            if let Some(value) = value {
                check_text(value, field)?;
            }
        }
        if let Some(attachment) = &self.attachment {
            attachment.validate(now_ms)?;
            if attachment.generation != self.generation {
                return Err(WorkloadContractError::InvalidNumber(
                    "attachment_generation",
                ));
            }
        }
        Ok(())
    }
}

/// Stable refusal codes for the Workload operation RPC boundary.  Provider
/// diagnostics remain in an accepted operation's bounded status projection;
/// malformed or unauthorized requests never echo raw request/provider text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadOperationErrorCode {
    /// The request body was not a valid bounded Workload envelope.
    MalformedRequest,
    /// The request exceeded the wire-size bound.
    PayloadTooLarge,
    /// The exact-body capability was absent, expired, or invalid.
    Unauthorized,
    /// The request was addressed to another node.
    TargetMismatch,
    /// The request id was reused with a different body.
    Conflict,
    /// The request used a stale generation or an operation is in flight.
    StaleGeneration,
    /// The same operation was already accepted; status is returned instead.
    Replayed,
    /// The durable journal could not be opened or flushed.
    JournalUnavailable,
}

/// Typed reply for `action/workload/operation`, correlated by the Bus action
/// message ULID and written to `reply/<request-ulid>`.  An accepted reply
/// always carries the authoritative persisted status, including terminal
/// adapter failure; `accepted` means the operation was journaled, not that a
/// VM/container was optimistically claimed ready.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadOperationReply {
    /// Contract discriminator.
    pub schema_version: u16,
    /// Idempotency key when it was safely recoverable from the request.
    pub request_id: String,
    /// Whether the operation was durably accepted or replayed.
    pub accepted: bool,
    /// Current authoritative status for an accepted/replayed operation.
    pub status: Option<WorkloadOperationStatus>,
    /// Stable refusal code when the operation was not accepted.
    pub error_code: Option<WorkloadOperationErrorCode>,
}

/// The authoritative per-node projection consumed by the shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadStateSnapshot {
    pub schema_version: u16,
    pub node: String,
    pub observed_at_ms: u64,
    pub workloads: Vec<WorkloadOperationStatus>,
}

impl WorkloadStateSnapshot {
    /// Validate a projection before publishing it to the mesh bus.
    pub fn validate(&self, now_ms: u64) -> Result<(), WorkloadContractError> {
        check_schema(self.schema_version)?;
        check_identifier(&self.node, "node")?;
        if self.observed_at_ms == 0 || self.workloads.len() > MAX_WORKLOADS_PER_NODE {
            return Err(WorkloadContractError::InvalidNumber("snapshot"));
        }
        let mut identities = BTreeSet::new();
        for status in &self.workloads {
            status.validate(now_ms)?;
            if !identities.insert(status.workload_id.clone()) {
                return Err(WorkloadContractError::InvalidField("duplicate_workload_id"));
            }
        }
        Ok(())
    }
}

/// Return the exact per-node publication topic.
#[must_use]
pub fn workload_state_topic(node: &str) -> String {
    format!("{WORKLOAD_STATE_TOPIC_PREFIX}{node}")
}

/// Whether a phase transition is legal for a journal replay.
#[must_use]
pub fn valid_phase_transition(from: WorkloadOperationPhase, to: WorkloadOperationPhase) -> bool {
    from == to
        || matches!(
            (from, to),
            (
                WorkloadOperationPhase::Queued,
                WorkloadOperationPhase::Validating
            ) | (
                WorkloadOperationPhase::Validating,
                WorkloadOperationPhase::Admitting
            ) | (
                WorkloadOperationPhase::Admitting,
                WorkloadOperationPhase::Defining
            ) | (
                WorkloadOperationPhase::Defining,
                WorkloadOperationPhase::Starting
            ) | (
                WorkloadOperationPhase::Starting,
                WorkloadOperationPhase::WaitingForGuest
            ) | (
                WorkloadOperationPhase::WaitingForGuest,
                WorkloadOperationPhase::WaitingForService
            ) | (
                WorkloadOperationPhase::WaitingForService,
                WorkloadOperationPhase::PreparingDisplay
            ) | (
                WorkloadOperationPhase::PreparingDisplay,
                WorkloadOperationPhase::WaitingForFirstFrame
            ) | (
                WorkloadOperationPhase::WaitingForFirstFrame,
                WorkloadOperationPhase::Ready
            ) | (
                WorkloadOperationPhase::Ready,
                WorkloadOperationPhase::Completed
            ) | (
                WorkloadOperationPhase::Stopping,
                WorkloadOperationPhase::Completed
            ) | (
                WorkloadOperationPhase::Queued,
                WorkloadOperationPhase::Cancelled
            ) | (
                WorkloadOperationPhase::Queued,
                WorkloadOperationPhase::Failed
            ) | (
                WorkloadOperationPhase::Validating,
                WorkloadOperationPhase::Failed
            ) | (
                WorkloadOperationPhase::Admitting,
                WorkloadOperationPhase::Failed
            ) | (
                WorkloadOperationPhase::Defining,
                WorkloadOperationPhase::Failed
            ) | (
                WorkloadOperationPhase::Starting,
                WorkloadOperationPhase::Failed
            ) | (
                WorkloadOperationPhase::WaitingForGuest,
                WorkloadOperationPhase::Failed
            ) | (
                WorkloadOperationPhase::WaitingForService,
                WorkloadOperationPhase::Failed
            ) | (
                WorkloadOperationPhase::PreparingDisplay,
                WorkloadOperationPhase::Failed
            ) | (
                WorkloadOperationPhase::WaitingForFirstFrame,
                WorkloadOperationPhase::Failed
            ) | (
                WorkloadOperationPhase::Ready,
                WorkloadOperationPhase::Failed
            ) | (
                WorkloadOperationPhase::Queued,
                WorkloadOperationPhase::Stopping
            ) | (
                WorkloadOperationPhase::Ready,
                WorkloadOperationPhase::Stopping
            ) | (
                WorkloadOperationPhase::Validating,
                WorkloadOperationPhase::Cancelled
            ) | (
                WorkloadOperationPhase::Admitting,
                WorkloadOperationPhase::Cancelled
            ) | (
                WorkloadOperationPhase::Defining,
                WorkloadOperationPhase::Cancelled
            ) | (
                WorkloadOperationPhase::Starting,
                WorkloadOperationPhase::Cancelled
            ) | (
                WorkloadOperationPhase::WaitingForGuest,
                WorkloadOperationPhase::Cancelled
            ) | (
                WorkloadOperationPhase::WaitingForService,
                WorkloadOperationPhase::Cancelled
            ) | (
                WorkloadOperationPhase::PreparingDisplay,
                WorkloadOperationPhase::Cancelled
            ) | (
                WorkloadOperationPhase::WaitingForFirstFrame,
                WorkloadOperationPhase::Cancelled
            ) | (
                WorkloadOperationPhase::Ready,
                WorkloadOperationPhase::Cancelled
            ) | (
                WorkloadOperationPhase::Stopping,
                WorkloadOperationPhase::Cancelled
            )
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> WorkloadOperationRequest {
        WorkloadOperationRequest {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            request_id: "req-1".into(),
            workload_id: WorkloadId::new("browser-seat15").expect("valid id"),
            backend: WorkloadBackend::LibvirtVirtqemud,
            resources: WorkloadProfile::Standard.resources(),
            image_ref: None,
            target_node: "seat15".into(),
            expected_generation: 0,
            action: WorkloadOperationAction::StartAndAttach,
            target_request_id: None,
            deadline_at_ms: 10_000,
            preferred_attachment: Some(WorkloadAttachmentProtocol::QemuDisplay1Dmabuf),
            armed_token: None,
        }
    }

    #[test]
    fn request_round_trips_and_rejects_old_schema() {
        let body = serde_json::to_string(&request()).expect("encode");
        let decoded = WorkloadOperationRequest::from_json(&body, 1_000).expect("decode");
        assert_eq!(decoded, request());
        let old = body.replace("\"schema_version\":1", "\"schema_version\":99");
        assert_eq!(
            WorkloadOperationRequest::from_json(&old, 1_000),
            Err(WorkloadContractError::UnsupportedSchema(99))
        );
    }

    #[test]
    fn request_rejects_duplicate_keys_at_every_json_nesting_level() {
        let body = serde_json::to_string(&request()).expect("encode");
        let duplicate_top_level = body.replacen(
            "\"schema_version\":1",
            "\"schema_version\":1,\"schema_version\":99",
            1,
        );
        assert_eq!(
            WorkloadOperationRequest::from_json(&duplicate_top_level, 1_000),
            Err(WorkloadContractError::MalformedWire)
        );

        let duplicate_nested = body.replacen("\"vcpu\":4", "\"vcpu\":4,\"vcpu\":64", 1);
        assert_eq!(
            WorkloadOperationRequest::from_json(&duplicate_nested, 1_000),
            Err(WorkloadContractError::MalformedWire)
        );
    }

    #[test]
    fn identifiers_are_bounded_and_path_free() {
        assert!(WorkloadId::new("seat/15").is_err());
        assert!(WorkloadId::new(" ").is_err());
        assert!(WorkloadId::new("seat-15").is_ok());
    }

    #[test]
    fn operation_deadline_and_transition_are_fail_closed() {
        let mut value = request();
        value.deadline_at_ms = 1_000;
        assert_eq!(
            value.validate(1_000),
            Err(WorkloadContractError::InvalidNumber("deadline_at_ms"))
        );
        assert!(valid_phase_transition(
            WorkloadOperationPhase::WaitingForFirstFrame,
            WorkloadOperationPhase::Ready
        ));
        assert!(!valid_phase_transition(
            WorkloadOperationPhase::Queued,
            WorkloadOperationPhase::Ready
        ));
    }

    #[test]
    fn cancellation_requires_an_explicit_distinct_target_operation() {
        let mut value = request();
        value.action = WorkloadOperationAction::Cancel;
        assert_eq!(
            value.validate(1_000),
            Err(WorkloadContractError::InvalidField("target_request_id"))
        );

        value.expected_generation = 1;
        value.target_request_id = Some(value.request_id.clone());
        assert_eq!(
            value.validate(1_000),
            Err(WorkloadContractError::InvalidTransition)
        );

        value.target_request_id = Some("op-start-1".into());
        assert!(value.validate(1_000).is_ok());
    }

    #[test]
    fn image_reference_is_catalog_identity_not_a_host_path() {
        let mut value = request();
        value.image_ref = Some("fedora:1.0".into());
        assert!(value.validate(1_000).is_ok());
        value.image_ref = Some("/var/lib/images/fedora.img".into());
        assert_eq!(
            value.validate(1_000),
            Err(WorkloadContractError::InvalidField("image_ref"))
        );
    }

    #[test]
    fn status_carries_an_optional_catalog_image_without_breaking_old_snapshots() {
        let mut status = WorkloadOperationStatus {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            request_id: "status-1".into(),
            workload_id: WorkloadId::new("container:seat15:mesh-api").expect("valid id"),
            backend: WorkloadBackend::QuadletSystemd,
            resources: WorkloadProfile::Small.resources(),
            image_ref: Some("mesh-api:1.0".into()),
            generation: 1,
            phase: WorkloadOperationPhase::Ready,
            power: WorkloadPowerState::Running,
            readiness: WorkloadReadiness::Ready,
            signals: WorkloadRuntimeSignals::from_readiness(
                WorkloadOperationPhase::Ready,
                WorkloadReadiness::Ready,
            ),
            retryable: false,
            attempt: 1,
            next_retry_at_ms: 0,
            reason: None,
            remediation: None,
            attachment: None,
        };
        assert!(status.validate(1_000).is_ok());
        let body = serde_json::to_string(&status).expect("encode status");
        let round_trip: WorkloadOperationStatus = serde_json::from_str(&body).expect("decode status");
        assert_eq!(round_trip.image_ref.as_deref(), Some("mesh-api:1.0"));

        status.image_ref = None;
        let old_shape = serde_json::to_string(&status).expect("encode old shape");
        assert!(!old_shape.contains("image_ref"));
        let old_round_trip: WorkloadOperationStatus =
            serde_json::from_str(&old_shape).expect("decode old shape");
        assert_eq!(old_round_trip.image_ref, None);

        status.image_ref = Some("/var/lib/mde/unsafe.img".into());
        assert_eq!(
            status.validate(1_000),
            Err(WorkloadContractError::InvalidField("image_ref"))
        );
    }

    #[test]
    fn status_rejects_a_stale_attachment_generation() {
        let workload_id = WorkloadId::new("browser-seat15").expect("valid workload id");
        let mut status = WorkloadOperationStatus {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            request_id: "status-attachment-1".into(),
            workload_id: workload_id.clone(),
            backend: WorkloadBackend::LibvirtVirtqemud,
            resources: WorkloadProfile::Standard.resources(),
            image_ref: None,
            generation: 1,
            phase: WorkloadOperationPhase::Ready,
            power: WorkloadPowerState::Running,
            readiness: WorkloadReadiness::Ready,
            signals: WorkloadRuntimeSignals::from_readiness(
                WorkloadOperationPhase::Ready,
                WorkloadReadiness::Ready,
            ),
            retryable: false,
            attempt: 1,
            next_retry_at_ms: 0,
            reason: None,
            remediation: None,
            attachment: Some(WorkloadAttachmentLease {
                schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
                lease_id: "lease-1".into(),
                nonce: "nonce-1".into(),
                workload_id,
                generation: 1,
                protocol: WorkloadAttachmentProtocol::QemuDisplay1Dmabuf,
                expires_at_ms: 5_000,
            }),
        };
        assert!(status.validate(1_000).is_ok());

        status.generation = 2;
        assert_eq!(
            status.validate(1_000),
            Err(WorkloadContractError::InvalidNumber("attachment_generation"))
        );
    }

    #[test]
    fn state_topic_is_node_scoped() {
        assert_eq!(workload_state_topic("seat15"), "state/workloads/seat15");
    }

    #[test]
    fn admission_keeps_the_host_reserve_and_supports_two_profiles() {
        assert_eq!(WorkloadProfile::Small.resources().vcpu, 2);
        assert_eq!(WorkloadProfile::Standard.resources().vcpu, 4);
        assert_eq!(WorkloadProfile::Standard.resources().memory_mb, 8_192);
        assert_eq!(host_reserve(4, 16_384), (1, 3_277));
        let host = HostCapacity {
            logical_cpus: 4,
            memory_mb: 16_384,
            allocated_vcpu: 0,
            allocated_memory_mb: 0,
            storage_gb: 128,
            allocated_storage_gb: 0,
        };
        assert!(admit_workload(WorkloadProfile::Small.resources(), host).admitted);
        assert!(!admit_workload(WorkloadProfile::Standard.resources(), host).admitted);
    }

    #[test]
    fn backend_admission_uses_the_matching_storage_pool() {
        let host = HostCapacity {
            logical_cpus: 8,
            memory_mb: 32_768,
            allocated_vcpu: 0,
            allocated_memory_mb: 0,
            storage_gb: 0,
            allocated_storage_gb: 0,
        };
        let storage = WorkloadStorageCapacity {
            vm_storage_gb: 16,
            allocated_vm_storage_gb: 0,
            container_storage_gb: 64,
            allocated_container_storage_gb: 0,
        };

        let vm = admit_workload_for_backend(
            WorkloadProfile::Small.resources(),
            WorkloadBackend::LibvirtVirtqemud,
            host,
            storage,
        );
        let container = admit_workload_for_backend(
            WorkloadProfile::Small.resources(),
            WorkloadBackend::QuadletSystemd,
            host,
            storage,
        );

        assert_eq!(vm.denial, Some(AdmissionDenial::StorageReserve));
        assert!(container.admitted);
        assert_eq!(vm.available_storage_gb, 16);
        assert_eq!(container.available_storage_gb, 64);
    }

    #[test]
    fn backend_admission_counts_only_matching_reservations() {
        let host = HostCapacity {
            logical_cpus: 8,
            memory_mb: 32_768,
            allocated_vcpu: 0,
            allocated_memory_mb: 0,
            storage_gb: 0,
            allocated_storage_gb: 0,
        };
        let storage = WorkloadStorageCapacity {
            vm_storage_gb: 64,
            allocated_vm_storage_gb: 40,
            container_storage_gb: 64,
            allocated_container_storage_gb: 0,
        };

        let vm = admit_workload_for_backend(
            WorkloadProfile::Small.resources(),
            WorkloadBackend::LibvirtVirtqemud,
            host,
            storage,
        );
        let container = admit_workload_for_backend(
            WorkloadProfile::Small.resources(),
            WorkloadBackend::QuadletSystemd,
            host,
            storage,
        );

        assert_eq!(vm.available_storage_gb, 24);
        assert_eq!(vm.denial, Some(AdmissionDenial::StorageReserve));
        assert_eq!(container.available_storage_gb, 64);
        assert!(container.admitted);
    }
}
