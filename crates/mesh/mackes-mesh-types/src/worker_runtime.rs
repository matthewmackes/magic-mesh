//! WL-ARCH-009 — neutral, bounded worker-runtime contracts.
//!
//! This module is the shared wire boundary between worker producers and the
//! Workers surface.  It intentionally contains no daemon implementation,
//! process handle, command, path, URL, log body, credential, or secret.  A
//! consumer must admit a record after decoding it; the custom deserializers
//! below make that rule difficult to accidentally skip for the top-level
//! records.

#![allow(
    missing_docs,
    reason = "public field names are the documented versioned wire contract"
)]
#![allow(
    clippy::missing_errors_doc,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::too_long_first_doc_paragraph,
    reason = "the versioned worker wire contract keeps established constructors and validation boundaries stable"
)]

use serde::{de, Deserialize, Deserializer, Serialize};
use std::collections::BTreeSet;
use std::fmt;

/// The only worker-runtime schema currently admitted by this crate.
pub const WORKER_RUNTIME_SCHEMA_VERSION: u16 = 1;
/// Alias naming the version as a contract rather than a publication.
pub const WORKER_RUNTIME_CONTRACT_SCHEMA_VERSION: u16 = WORKER_RUNTIME_SCHEMA_VERSION;
/// Maximum JSON body admitted by the bounded `from_json` helpers.
pub const MAX_WORKER_RUNTIME_WIRE_BYTES: usize = 2 * 1024 * 1024;
/// Maximum stable worker/node/event/request identifier size in bytes.
pub const MAX_WORKER_IDENTIFIER_BYTES: usize = 128;
/// Maximum topic or publication name size in bytes.
pub const MAX_WORKER_TOPIC_BYTES: usize = 256;
/// Maximum display-name size in bytes.
pub const MAX_WORKER_DISPLAY_NAME_BYTES: usize = 256;
/// Maximum description, impact, recovery, and event-detail size in bytes.
pub const MAX_WORKER_TEXT_BYTES: usize = 1_024;
/// Maximum number of role/capability applicability entries.
pub const MAX_WORKER_APPLICABILITY_ENTRIES: usize = 16;
/// Maximum number of dependency identifiers on one worker contract.
pub const MAX_WORKER_DEPENDENCIES: usize = 64;
/// Maximum number of publications or subscriptions on one worker contract.
pub const MAX_WORKER_TOPICS: usize = 64;
/// Maximum number of typed actions on one worker contract.
pub const MAX_WORKER_ACTIONS: usize = 32;
/// Maximum number of relations on one runtime snapshot.
pub const MAX_WORKER_RELATIONS: usize = 256;
/// Maximum number of retained timeline events on one runtime snapshot.
pub const MAX_WORKER_TIMELINE_EVENTS: usize = 512;
/// Maximum number of items in one staged change set.
pub const MAX_WORKER_CHANGE_SET_ITEMS: usize = 64;
/// Maximum freshness interval represented by the v1 contract.
pub const MAX_WORKER_FRESHNESS_MS: u64 = 24 * 60 * 60 * 1_000;
/// Maximum lifetime of a staged change-set preview.
pub const MAX_WORKER_CHANGE_SET_TTL_MS: u64 = 10 * 60 * 1_000;
/// Canonical authenticated Bus action lane for staged Workers change sets.
pub const WORKER_CHANGE_SET_ACTION_TOPIC_PREFIX: &str = "action/workers/change-set";
/// Canonical latest-wins result lane for staged Workers change sets.
pub const WORKER_CHANGE_SET_RESULT_TOPIC_PREFIX: &str = "state/workers/change-set";
/// Capability verb used by the existing root-shell action mint authority.
pub const WORKER_CHANGE_SET_AUTH_VERB: &str = "workers-change-set";
/// Maximum encoded size of the short-lived action capability.
pub const MAX_WORKER_ARMED_TOKEN_BYTES: usize = 4 * 1024;
/// Maximum retained restart count before a producer must roll its generation.
pub const MAX_WORKER_RESTART_COUNT: u32 = 1_000_000;

/// A validation or admission failure at the worker-runtime boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerRuntimeContractError {
    /// The encoded body exceeds the bounded wire allocation.
    PayloadTooLarge,
    /// The body is not valid JSON for the closed contract.
    MalformedWire,
    /// A top-level or nested discriminator uses an unknown schema version.
    UnsupportedSchema {
        /// Contract field carrying the unsupported version.
        field: &'static str,
        /// Version found on the wire.
        found: u16,
    },
    /// A bounded value is empty, malformed, or uses a forbidden grammar.
    InvalidField(&'static str),
    /// A bounded string exceeds its wire limit.
    FieldTooLong(&'static str),
    /// A bounded collection exceeds its v1 capacity.
    CapacityExceeded {
        /// Collection that exceeded its bound.
        field: &'static str,
        /// Maximum number of entries admitted.
        max: usize,
    },
    /// A set-like collection contains a repeated identity.
    Duplicate(&'static str),
    /// A timestamp is zero or ordered inconsistently.
    InvalidTimestamp(&'static str),
    /// A freshness interval is empty, reversed, or too large.
    InvalidFreshness(&'static str),
    /// A generation or sequence number is not positive.
    InvalidGeneration(&'static str),
    /// A state/reason, relation, or target pairing is not admitted.
    InvalidRelationship(&'static str),
    /// A state that requires a closed reason omitted it or used the wrong one.
    InvalidStateReason(&'static str),
    /// A free-form field resembles a credential, secret, raw path, URL, or
    /// command and therefore is not redaction-safe.
    SecretShapedValue(&'static str),
    /// A digest is not the lowercase `sha256:<64 hex>` form.
    InvalidDigest(&'static str),
    /// A time-bound request has expired at the supplied admission time.
    Expired(&'static str),
}

impl fmt::Display for WorkerRuntimeContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge => formatter.write_str("worker-runtime body is too large"),
            Self::MalformedWire => formatter.write_str("malformed worker-runtime body"),
            Self::UnsupportedSchema { field, found } => {
                write!(
                    formatter,
                    "unsupported worker-runtime schema {found} in {field}"
                )
            }
            Self::InvalidField(field) => write!(formatter, "invalid worker-runtime field {field}"),
            Self::FieldTooLong(field) => {
                write!(formatter, "worker-runtime field is too long: {field}")
            }
            Self::CapacityExceeded { field, max } => {
                write!(formatter, "worker-runtime collection {field} exceeds {max}")
            }
            Self::Duplicate(field) => {
                write!(formatter, "duplicate worker-runtime value in {field}")
            }
            Self::InvalidTimestamp(field) => {
                write!(formatter, "invalid worker-runtime timestamp {field}")
            }
            Self::InvalidFreshness(field) => {
                write!(formatter, "invalid worker-runtime freshness {field}")
            }
            Self::InvalidGeneration(field) => {
                write!(
                    formatter,
                    "invalid worker-runtime generation/sequence {field}"
                )
            }
            Self::InvalidRelationship(field) => {
                write!(formatter, "invalid worker-runtime relationship {field}")
            }
            Self::InvalidStateReason(field) => {
                write!(formatter, "invalid worker-runtime state reason {field}")
            }
            Self::SecretShapedValue(field) => {
                write!(formatter, "secret-shaped worker-runtime value in {field}")
            }
            Self::InvalidDigest(field) => {
                write!(formatter, "invalid worker-runtime digest {field}")
            }
            Self::Expired(field) => write!(formatter, "expired worker-runtime request {field}"),
        }
    }
}

impl std::error::Error for WorkerRuntimeContractError {}

/// The six process-isolation groups owned by the worker architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerGroup {
    Control,
    Observation,
    Actions,
    Data,
    Compute,
    Integrations,
}

impl WorkerGroup {
    /// Stable registry and presentation order.
    pub const ALL: [Self; 6] = [
        Self::Control,
        Self::Observation,
        Self::Actions,
        Self::Data,
        Self::Compute,
        Self::Integrations,
    ];

    /// Stable wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::Observation => "observation",
            Self::Actions => "actions",
            Self::Data => "data",
            Self::Compute => "compute",
            Self::Integrations => "integrations",
        }
    }
}

/// Runtime state vocabulary.  The variants are intentionally closed so an
/// unknown state cannot be rendered as if it were current.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRuntimeState {
    NotApplicable,
    Unconfigured,
    Starting,
    Running,
    Backoff,
    Paused,
    Stopped,
    Failed,
    Stale,
    Unavailable,
}

impl WorkerRuntimeState {
    /// Stable wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::Unconfigured => "unconfigured",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Backoff => "backoff",
            Self::Paused => "paused",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Stale => "stale",
            Self::Unavailable => "unavailable",
        }
    }

    /// Whether the state must carry a closed reason in a snapshot.
    #[must_use]
    pub const fn requires_reason(self) -> bool {
        matches!(
            self,
            Self::NotApplicable
                | Self::Unconfigured
                | Self::Backoff
                | Self::Paused
                | Self::Failed
                | Self::Stale
                | Self::Unavailable
        )
    }
}

/// Role classes a worker may apply to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRole {
    Lighthouse,
    Workstation,
}

/// Operational importance used by restart and degraded-mode policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerCriticality {
    Essential,
    Important,
    Optional,
}

/// Closed restart policy advertised by a worker contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRestartPolicy {
    Never,
    OnFailure,
    Always,
}

/// Cadence contract for worker-owned work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerCadence {
    Continuous,
    EventDriven,
    OnDemand,
    Periodic {
        min_interval_ms: u64,
        max_interval_ms: u64,
    },
}

impl WorkerCadence {
    fn validate(&self) -> Result<(), WorkerRuntimeContractError> {
        if let Self::Periodic {
            min_interval_ms,
            max_interval_ms,
        } = self
        {
            if *min_interval_ms == 0
                || *max_interval_ms == 0
                || min_interval_ms > max_interval_ms
                || *max_interval_ms > MAX_WORKER_FRESHNESS_MS
            {
                return Err(WorkerRuntimeContractError::InvalidField(
                    "cadence.periodic.interval",
                ));
            }
        }
        Ok(())
    }
}

/// Queue overflow behavior exposed by a worker contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerQueueOverflow {
    RejectNew,
    LatestWins,
    Drain,
}

/// Explicit queue capacity; an unbounded queue cannot be represented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerQueueContract {
    pub max_items: u32,
    pub max_bytes: u64,
    pub overflow: WorkerQueueOverflow,
}

impl WorkerQueueContract {
    const fn validate(&self, field: &'static str) -> Result<(), WorkerRuntimeContractError> {
        if self.max_items == 0 || self.max_bytes == 0 {
            return Err(WorkerRuntimeContractError::InvalidField(field));
        }
        if self.max_items > 1_000_000 || self.max_bytes > 64 * 1024 * 1024 {
            return Err(WorkerRuntimeContractError::InvalidField(field));
        }
        Ok(())
    }
}

/// Cache policy with an explicit disabled or bounded form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerCachePolicy {
    Disabled,
    Bounded {
        max_items: u32,
        max_bytes: u64,
        ttl_ms: u64,
    },
}

impl WorkerCachePolicy {
    const fn validate(&self) -> Result<(), WorkerRuntimeContractError> {
        if let Self::Bounded {
            max_items,
            max_bytes,
            ttl_ms,
        } = self
        {
            if *max_items == 0
                || *max_bytes == 0
                || *ttl_ms == 0
                || *ttl_ms > MAX_WORKER_FRESHNESS_MS
            {
                return Err(WorkerRuntimeContractError::InvalidField("cache.bounded"));
            }
        }
        Ok(())
    }
}

/// Per-worker memory, CPU, and task admission budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerResourceBudget {
    pub memory_high_bytes: u64,
    pub memory_max_bytes: u64,
    pub cpu_millis_per_second: u16,
    pub max_tasks: u16,
}

impl WorkerResourceBudget {
    const fn validate(&self) -> Result<(), WorkerRuntimeContractError> {
        if self.memory_high_bytes == 0
            || self.memory_max_bytes == 0
            || self.memory_high_bytes > self.memory_max_bytes
            || self.memory_max_bytes > 64 * 1024 * 1024 * 1024
            || self.cpu_millis_per_second == 0
            || self.cpu_millis_per_second > 10_000
            || self.max_tasks == 0
            || self.max_tasks > 4_096
        {
            return Err(WorkerRuntimeContractError::InvalidField("resources"));
        }
        Ok(())
    }
}

/// Component accountable for cleanup of worker-owned resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerCleanupOwner {
    Worker,
    GroupSupervisor,
}

/// Ownership of the state, health, action, and cleanup lanes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerOwnership {
    pub state_group: WorkerGroup,
    pub health_group: WorkerGroup,
    pub action_group: WorkerGroup,
    pub cleanup_owner: WorkerCleanupOwner,
}

impl WorkerOwnership {
    fn validate(self, declared_group: WorkerGroup) -> Result<(), WorkerRuntimeContractError> {
        if self.state_group != declared_group
            || self.health_group != declared_group
            || self.action_group != declared_group
        {
            return Err(WorkerRuntimeContractError::InvalidRelationship(
                "ownership.group",
            ));
        }
        Ok(())
    }
}

/// Role and capability activation predicate. Empty role/capability lists mean
/// that the worker applies to every node role unless configuration says otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(Default)]
pub struct WorkerApplicability {
    pub roles: Vec<WorkerRole>,
    pub capabilities: Vec<String>,
    pub requires_configuration: bool,
}

impl WorkerApplicability {
    fn validate(&self) -> Result<(), WorkerRuntimeContractError> {
        if self.roles.len() > MAX_WORKER_APPLICABILITY_ENTRIES
            || self.capabilities.len() > MAX_WORKER_APPLICABILITY_ENTRIES
        {
            return Err(WorkerRuntimeContractError::CapacityExceeded {
                field: "applicability",
                max: MAX_WORKER_APPLICABILITY_ENTRIES,
            });
        }
        let mut roles = BTreeSet::new();
        for role in &self.roles {
            if !roles.insert(*role) {
                return Err(WorkerRuntimeContractError::Duplicate("applicability.roles"));
            }
        }
        validate_unique_identifiers("applicability.capabilities", &self.capabilities)?;
        Ok(())
    }
}

/// Typed action that may be staged by the global Action Console.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerAction {
    Start,
    Stop,
    Restart,
    Pause,
    Resume,
    Refresh,
}

/// Closed arming requirement; no bearer token or confirmation text is carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerArmingRequirement {
    None,
    Confirmation,
    Reauthentication,
}

/// Metadata for one action admitted by a worker contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerActionDescriptor {
    pub action: WorkerAction,
    pub label: String,
    pub arming: WorkerArmingRequirement,
}

impl WorkerActionDescriptor {
    fn validate(&self) -> Result<(), WorkerRuntimeContractError> {
        validate_text(
            "actions.label",
            &self.label,
            MAX_WORKER_DISPLAY_NAME_BYTES,
            false,
        )
    }
}

/// A versioned worker declaration consumed by shell and daemon adapters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerContract {
    pub schema_version: u16,
    pub worker_id: String,
    pub group: WorkerGroup,
    pub display_name: String,
    pub description: String,
    pub applicability: WorkerApplicability,
    pub criticality: WorkerCriticality,
    pub restart_policy: WorkerRestartPolicy,
    pub cadence: WorkerCadence,
    pub queue: WorkerQueueContract,
    pub cache: WorkerCachePolicy,
    pub resources: WorkerResourceBudget,
    pub ownership: WorkerOwnership,
    pub dependencies: Vec<String>,
    pub publications: Vec<String>,
    pub subscriptions: Vec<String>,
    pub actions: Vec<WorkerActionDescriptor>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerContractWire {
    schema_version: u16,
    worker_id: String,
    group: WorkerGroup,
    display_name: String,
    description: String,
    applicability: WorkerApplicability,
    criticality: WorkerCriticality,
    restart_policy: WorkerRestartPolicy,
    cadence: WorkerCadence,
    queue: WorkerQueueContract,
    cache: WorkerCachePolicy,
    resources: WorkerResourceBudget,
    ownership: WorkerOwnership,
    dependencies: Vec<String>,
    publications: Vec<String>,
    subscriptions: Vec<String>,
    actions: Vec<WorkerActionDescriptor>,
}

impl<'de> Deserialize<'de> for WorkerContract {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = WorkerContractWire::deserialize(deserializer)?;
        let contract = Self {
            schema_version: wire.schema_version,
            worker_id: wire.worker_id,
            group: wire.group,
            display_name: wire.display_name,
            description: wire.description,
            applicability: wire.applicability,
            criticality: wire.criticality,
            restart_policy: wire.restart_policy,
            cadence: wire.cadence,
            queue: wire.queue,
            cache: wire.cache,
            resources: wire.resources,
            ownership: wire.ownership,
            dependencies: wire.dependencies,
            publications: wire.publications,
            subscriptions: wire.subscriptions,
            actions: wire.actions,
        };
        contract.validate().map_err(de::Error::custom)?;
        Ok(contract)
    }
}

impl WorkerContract {
    /// Construct a useful contract with conservative bounded defaults.
    pub fn new(
        worker_id: impl Into<String>,
        group: WorkerGroup,
        display_name: impl Into<String>,
    ) -> Result<Self, WorkerRuntimeContractError> {
        let contract = Self {
            schema_version: WORKER_RUNTIME_SCHEMA_VERSION,
            worker_id: worker_id.into(),
            group,
            display_name: display_name.into(),
            description: String::new(),
            applicability: WorkerApplicability::default(),
            criticality: WorkerCriticality::Important,
            restart_policy: WorkerRestartPolicy::OnFailure,
            cadence: WorkerCadence::OnDemand,
            queue: WorkerQueueContract {
                max_items: 64,
                max_bytes: 1024 * 1024,
                overflow: WorkerQueueOverflow::RejectNew,
            },
            cache: WorkerCachePolicy::Disabled,
            resources: WorkerResourceBudget {
                memory_high_bytes: 64 * 1024 * 1024,
                memory_max_bytes: 128 * 1024 * 1024,
                cpu_millis_per_second: 250,
                max_tasks: 16,
            },
            ownership: WorkerOwnership {
                state_group: group,
                health_group: group,
                action_group: group,
                cleanup_owner: WorkerCleanupOwner::Worker,
            },
            dependencies: Vec::new(),
            publications: Vec::new(),
            subscriptions: Vec::new(),
            actions: Vec::new(),
        };
        contract.validate()?;
        Ok(contract)
    }

    /// Validate every field and bounded collection before publication.
    pub fn validate(&self) -> Result<(), WorkerRuntimeContractError> {
        validate_schema("worker_contract.schema_version", self.schema_version)?;
        validate_identifier("worker_contract.worker_id", &self.worker_id)?;
        validate_text(
            "worker_contract.display_name",
            &self.display_name,
            MAX_WORKER_DISPLAY_NAME_BYTES,
            false,
        )?;
        validate_text(
            "worker_contract.description",
            &self.description,
            MAX_WORKER_TEXT_BYTES,
            true,
        )?;
        self.applicability.validate()?;
        self.cadence.validate()?;
        self.queue.validate("worker_contract.queue")?;
        self.cache.validate()?;
        self.resources.validate()?;
        self.ownership.validate(self.group)?;
        validate_unique_identifiers("worker_contract.dependencies", &self.dependencies)?;
        if self.dependencies.len() > MAX_WORKER_DEPENDENCIES {
            return Err(WorkerRuntimeContractError::CapacityExceeded {
                field: "worker_contract.dependencies",
                max: MAX_WORKER_DEPENDENCIES,
            });
        }
        validate_unique_topics("worker_contract.publications", &self.publications)?;
        validate_unique_topics("worker_contract.subscriptions", &self.subscriptions)?;
        if self.publications.len() > MAX_WORKER_TOPICS {
            return Err(WorkerRuntimeContractError::CapacityExceeded {
                field: "worker_contract.publications",
                max: MAX_WORKER_TOPICS,
            });
        }
        if self.subscriptions.len() > MAX_WORKER_TOPICS {
            return Err(WorkerRuntimeContractError::CapacityExceeded {
                field: "worker_contract.subscriptions",
                max: MAX_WORKER_TOPICS,
            });
        }
        if self.actions.len() > MAX_WORKER_ACTIONS {
            return Err(WorkerRuntimeContractError::CapacityExceeded {
                field: "worker_contract.actions",
                max: MAX_WORKER_ACTIONS,
            });
        }
        let mut actions = BTreeSet::new();
        for action in &self.actions {
            action.validate()?;
            if !actions.insert(action.action) {
                return Err(WorkerRuntimeContractError::Duplicate(
                    "worker_contract.actions",
                ));
            }
        }
        Ok(())
    }

    /// Admit a contract received over an untrusted boundary.
    pub fn admitted(self) -> Result<Self, WorkerRuntimeContractError> {
        self.validate()?;
        Ok(self)
    }

    /// Decode and admit a bounded JSON contract.
    pub fn from_json(body: &str) -> Result<Self, WorkerRuntimeContractError> {
        bounded_json::<Self>(body)
    }

    /// Validate and encode this contract as bounded JSON.
    pub fn to_json(&self) -> Result<String, WorkerRuntimeContractError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|_| WorkerRuntimeContractError::MalformedWire)
    }
}

/// Closed reasons for non-running worker states.  The enum prevents a
/// diagnostic string from becoming a covert log or credential channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRuntimeReason {
    NotApplicable,
    NotConfigured,
    CapabilityMissing,
    DependencyUnavailable,
    ProviderUnavailable,
    ResourceLimit,
    CrashLoop,
    OperatorPaused,
    ObservationStale,
    Unknown,
}

/// Endpoint of a typed worker relationship.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerRelationEndpoint {
    Worker {
        worker_id: String,
    },
    Node {
        node_id: String,
    },
    Output {
        worker_id: String,
        output_kind: String,
    },
    Topic {
        topic: String,
    },
}

impl WorkerRelationEndpoint {
    fn validate(&self) -> Result<(), WorkerRuntimeContractError> {
        match self {
            Self::Worker { worker_id } => validate_identifier("relation.worker_id", worker_id),
            Self::Node { node_id } => validate_identifier("relation.node_id", node_id),
            Self::Output {
                worker_id,
                output_kind,
            } => {
                validate_identifier("relation.output.worker_id", worker_id)?;
                validate_identifier("relation.output.kind", output_kind)
            }
            Self::Topic { topic } => validate_topic("relation.topic", topic),
        }
    }
}

/// Typed edge kind used by the Workers graph and inspector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRelationKind {
    Owns,
    DependsOn,
    Publishes,
    Subscribes,
    ActionTarget,
    Supports,
    Contains,
}

/// One bounded, typed relationship between worker/runtime entities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRelation {
    pub schema_version: u16,
    pub relation_id: String,
    pub relation: WorkerRelationKind,
    pub source: WorkerRelationEndpoint,
    pub target: WorkerRelationEndpoint,
    pub label: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerRelationWire {
    schema_version: u16,
    relation_id: String,
    relation: WorkerRelationKind,
    source: WorkerRelationEndpoint,
    target: WorkerRelationEndpoint,
    label: Option<String>,
}

impl<'de> Deserialize<'de> for WorkerRelation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = WorkerRelationWire::deserialize(deserializer)?;
        let relation = Self {
            schema_version: wire.schema_version,
            relation_id: wire.relation_id,
            relation: wire.relation,
            source: wire.source,
            target: wire.target,
            label: wire.label,
        };
        relation.validate().map_err(de::Error::custom)?;
        Ok(relation)
    }
}

impl WorkerRelation {
    /// Construct and validate a typed relationship.
    pub fn new(
        relation_id: impl Into<String>,
        relation: WorkerRelationKind,
        source: WorkerRelationEndpoint,
        target: WorkerRelationEndpoint,
        label: Option<String>,
    ) -> Result<Self, WorkerRuntimeContractError> {
        let relation = Self {
            schema_version: WORKER_RUNTIME_SCHEMA_VERSION,
            relation_id: relation_id.into(),
            relation,
            source,
            target,
            label,
        };
        relation.validate()?;
        Ok(relation)
    }

    /// Validate schema, endpoint grammars, relationship semantics, and redaction.
    pub fn validate(&self) -> Result<(), WorkerRuntimeContractError> {
        validate_schema("worker_relation.schema_version", self.schema_version)?;
        validate_identifier("worker_relation.relation_id", &self.relation_id)?;
        self.source.validate()?;
        self.target.validate()?;
        if self.source == self.target {
            return Err(WorkerRuntimeContractError::InvalidRelationship(
                "worker_relation.self_edge",
            ));
        }
        match self.relation {
            WorkerRelationKind::DependsOn => {
                if !matches!(self.source, WorkerRelationEndpoint::Worker { .. })
                    || !matches!(self.target, WorkerRelationEndpoint::Worker { .. })
                {
                    return Err(WorkerRuntimeContractError::InvalidRelationship(
                        "worker_relation.depends_on",
                    ));
                }
            }
            WorkerRelationKind::Publishes | WorkerRelationKind::Subscribes => {
                if !matches!(self.source, WorkerRelationEndpoint::Worker { .. })
                    || !matches!(
                        self.target,
                        WorkerRelationEndpoint::Output { .. }
                            | WorkerRelationEndpoint::Topic { .. }
                    )
                {
                    return Err(WorkerRuntimeContractError::InvalidRelationship(
                        "worker_relation.publication",
                    ));
                }
            }
            WorkerRelationKind::Owns | WorkerRelationKind::Supports => {
                if !matches!(self.source, WorkerRelationEndpoint::Worker { .. }) {
                    return Err(WorkerRuntimeContractError::InvalidRelationship(
                        "worker_relation.owner",
                    ));
                }
            }
            WorkerRelationKind::ActionTarget | WorkerRelationKind::Contains => {}
        }
        if let Some(label) = &self.label {
            validate_text("worker_relation.label", label, MAX_WORKER_TEXT_BYTES, false)?;
        }
        Ok(())
    }

    /// Decode and admit a bounded JSON relationship.
    pub fn from_json(body: &str) -> Result<Self, WorkerRuntimeContractError> {
        bounded_json(body)
    }

    /// Validate and encode a relationship as bounded JSON.
    pub fn to_json(&self) -> Result<String, WorkerRuntimeContractError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|_| WorkerRuntimeContractError::MalformedWire)
    }
}

/// Closed event vocabulary for the bounded worker timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerTimelineEventKind {
    Registered,
    StateChanged,
    Started,
    Stopped,
    Restarted,
    BackoffEntered,
    Paused,
    Failure,
    Recovered,
    ActionStaged,
    ActionCompleted,
    OutputPublished,
    ProviderUnavailable,
}

/// One redacted, bounded timeline event.  It is not a raw log record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerTimelineEvent {
    pub schema_version: u16,
    pub event_id: String,
    pub sequence: u64,
    pub worker_id: String,
    pub occurred_at_ms: u64,
    pub kind: WorkerTimelineEventKind,
    pub state: Option<WorkerRuntimeState>,
    pub summary: String,
    pub detail: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerTimelineEventWire {
    schema_version: u16,
    event_id: String,
    sequence: u64,
    worker_id: String,
    occurred_at_ms: u64,
    kind: WorkerTimelineEventKind,
    state: Option<WorkerRuntimeState>,
    summary: String,
    detail: Option<String>,
}

impl<'de> Deserialize<'de> for WorkerTimelineEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = WorkerTimelineEventWire::deserialize(deserializer)?;
        let event = Self {
            schema_version: wire.schema_version,
            event_id: wire.event_id,
            sequence: wire.sequence,
            worker_id: wire.worker_id,
            occurred_at_ms: wire.occurred_at_ms,
            kind: wire.kind,
            state: wire.state,
            summary: wire.summary,
            detail: wire.detail,
        };
        event.validate().map_err(de::Error::custom)?;
        Ok(event)
    }
}

impl WorkerTimelineEvent {
    /// Construct and validate a bounded timeline event.
    pub fn new(
        event_id: impl Into<String>,
        sequence: u64,
        worker_id: impl Into<String>,
        occurred_at_ms: u64,
        kind: WorkerTimelineEventKind,
        state: Option<WorkerRuntimeState>,
        summary: impl Into<String>,
        detail: Option<String>,
    ) -> Result<Self, WorkerRuntimeContractError> {
        let event = Self {
            schema_version: WORKER_RUNTIME_SCHEMA_VERSION,
            event_id: event_id.into(),
            sequence,
            worker_id: worker_id.into(),
            occurred_at_ms,
            kind,
            state,
            summary: summary.into(),
            detail,
        };
        event.validate()?;
        Ok(event)
    }

    /// Validate identity, sequence, timestamp, event vocabulary, and redaction.
    pub fn validate(&self) -> Result<(), WorkerRuntimeContractError> {
        validate_schema("worker_timeline_event.schema_version", self.schema_version)?;
        validate_identifier("worker_timeline_event.event_id", &self.event_id)?;
        if self.sequence == 0 {
            return Err(WorkerRuntimeContractError::InvalidGeneration(
                "worker_timeline_event.sequence",
            ));
        }
        validate_identifier("worker_timeline_event.worker_id", &self.worker_id)?;
        validate_timestamp("worker_timeline_event.occurred_at_ms", self.occurred_at_ms)?;
        if self.kind == WorkerTimelineEventKind::StateChanged && self.state.is_none() {
            return Err(WorkerRuntimeContractError::InvalidRelationship(
                "worker_timeline_event.state_changed_state",
            ));
        }
        validate_text(
            "worker_timeline_event.summary",
            &self.summary,
            MAX_WORKER_TEXT_BYTES,
            false,
        )?;
        if let Some(detail) = &self.detail {
            validate_text(
                "worker_timeline_event.detail",
                detail,
                MAX_WORKER_TEXT_BYTES,
                false,
            )?;
        }
        Ok(())
    }

    /// Decode and admit a bounded JSON event.
    pub fn from_json(body: &str) -> Result<Self, WorkerRuntimeContractError> {
        bounded_json(body)
    }

    /// Validate and encode an event as bounded JSON.
    pub fn to_json(&self) -> Result<String, WorkerRuntimeContractError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|_| WorkerRuntimeContractError::MalformedWire)
    }
}

/// One worker's node-scoped runtime observation and bounded history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRuntimeSnapshot {
    pub schema_version: u16,
    pub snapshot_id: String,
    pub node_id: String,
    pub worker_id: String,
    pub group: WorkerGroup,
    pub generation: u64,
    pub state: WorkerRuntimeState,
    pub state_since_ms: u64,
    pub observed_at_ms: u64,
    pub published_at_ms: u64,
    pub fresh_until_ms: u64,
    pub restart_count: u32,
    pub backoff_until_ms: Option<u64>,
    pub state_reason: Option<WorkerRuntimeReason>,
    pub relations: Vec<WorkerRelation>,
    pub timeline: Vec<WorkerTimelineEvent>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerRuntimeSnapshotWire {
    schema_version: u16,
    snapshot_id: String,
    node_id: String,
    worker_id: String,
    group: WorkerGroup,
    generation: u64,
    state: WorkerRuntimeState,
    state_since_ms: u64,
    observed_at_ms: u64,
    published_at_ms: u64,
    fresh_until_ms: u64,
    restart_count: u32,
    backoff_until_ms: Option<u64>,
    state_reason: Option<WorkerRuntimeReason>,
    relations: Vec<WorkerRelation>,
    timeline: Vec<WorkerTimelineEvent>,
}

impl<'de> Deserialize<'de> for WorkerRuntimeSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = WorkerRuntimeSnapshotWire::deserialize(deserializer)?;
        let snapshot = Self {
            schema_version: wire.schema_version,
            snapshot_id: wire.snapshot_id,
            node_id: wire.node_id,
            worker_id: wire.worker_id,
            group: wire.group,
            generation: wire.generation,
            state: wire.state,
            state_since_ms: wire.state_since_ms,
            observed_at_ms: wire.observed_at_ms,
            published_at_ms: wire.published_at_ms,
            fresh_until_ms: wire.fresh_until_ms,
            restart_count: wire.restart_count,
            backoff_until_ms: wire.backoff_until_ms,
            state_reason: wire.state_reason,
            relations: wire.relations,
            timeline: wire.timeline,
        };
        snapshot.validate().map_err(de::Error::custom)?;
        Ok(snapshot)
    }
}

impl WorkerRuntimeSnapshot {
    /// Construct a snapshot without optional relations/history.
    pub fn new(
        snapshot_id: impl Into<String>,
        node_id: impl Into<String>,
        worker_id: impl Into<String>,
        group: WorkerGroup,
        generation: u64,
        state: WorkerRuntimeState,
        state_since_ms: u64,
        observed_at_ms: u64,
        published_at_ms: u64,
        fresh_until_ms: u64,
    ) -> Result<Self, WorkerRuntimeContractError> {
        let snapshot = Self {
            schema_version: WORKER_RUNTIME_SCHEMA_VERSION,
            snapshot_id: snapshot_id.into(),
            node_id: node_id.into(),
            worker_id: worker_id.into(),
            group,
            generation,
            state,
            state_since_ms,
            observed_at_ms,
            published_at_ms,
            fresh_until_ms,
            restart_count: 0,
            backoff_until_ms: None,
            state_reason: None,
            relations: Vec::new(),
            timeline: Vec::new(),
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    /// Validate all bounded snapshot fields and nested records.
    pub fn validate(&self) -> Result<(), WorkerRuntimeContractError> {
        validate_schema(
            "worker_runtime_snapshot.schema_version",
            self.schema_version,
        )?;
        validate_identifier("worker_runtime_snapshot.snapshot_id", &self.snapshot_id)?;
        validate_identifier("worker_runtime_snapshot.node_id", &self.node_id)?;
        validate_identifier("worker_runtime_snapshot.worker_id", &self.worker_id)?;
        if self.generation == 0 {
            return Err(WorkerRuntimeContractError::InvalidGeneration(
                "worker_runtime_snapshot.generation",
            ));
        }
        if self.restart_count > MAX_WORKER_RESTART_COUNT {
            return Err(WorkerRuntimeContractError::InvalidField(
                "worker_runtime_snapshot.restart_count",
            ));
        }
        validate_timestamp(
            "worker_runtime_snapshot.state_since_ms",
            self.state_since_ms,
        )?;
        validate_timestamp(
            "worker_runtime_snapshot.observed_at_ms",
            self.observed_at_ms,
        )?;
        validate_timestamp(
            "worker_runtime_snapshot.published_at_ms",
            self.published_at_ms,
        )?;
        validate_timestamp(
            "worker_runtime_snapshot.fresh_until_ms",
            self.fresh_until_ms,
        )?;
        if self.state_since_ms > self.observed_at_ms {
            return Err(WorkerRuntimeContractError::InvalidTimestamp(
                "worker_runtime_snapshot.state_since_ms",
            ));
        }
        if self.observed_at_ms > self.published_at_ms {
            return Err(WorkerRuntimeContractError::InvalidTimestamp(
                "worker_runtime_snapshot.observed_before_published",
            ));
        }
        if self.fresh_until_ms <= self.published_at_ms
            || self.fresh_until_ms - self.published_at_ms > MAX_WORKER_FRESHNESS_MS
        {
            return Err(WorkerRuntimeContractError::InvalidFreshness(
                "worker_runtime_snapshot.fresh_until_ms",
            ));
        }
        if let Some(backoff_until_ms) = self.backoff_until_ms {
            validate_timestamp("worker_runtime_snapshot.backoff_until_ms", backoff_until_ms)?;
            if self.state != WorkerRuntimeState::Backoff {
                return Err(WorkerRuntimeContractError::InvalidRelationship(
                    "worker_runtime_snapshot.backoff_until_state",
                ));
            }
        } else if self.state == WorkerRuntimeState::Backoff {
            return Err(WorkerRuntimeContractError::InvalidStateReason(
                "worker_runtime_snapshot.backoff_until_ms",
            ));
        }
        validate_state_reason(self.state, self.state_reason)?;
        if self.relations.len() > MAX_WORKER_RELATIONS {
            return Err(WorkerRuntimeContractError::CapacityExceeded {
                field: "worker_runtime_snapshot.relations",
                max: MAX_WORKER_RELATIONS,
            });
        }
        let mut relation_ids = BTreeSet::new();
        for relation in &self.relations {
            relation.validate()?;
            if !relation_ids.insert(relation.relation_id.as_str()) {
                return Err(WorkerRuntimeContractError::Duplicate(
                    "worker_runtime_snapshot.relations",
                ));
            }
        }
        if self.timeline.len() > MAX_WORKER_TIMELINE_EVENTS {
            return Err(WorkerRuntimeContractError::CapacityExceeded {
                field: "worker_runtime_snapshot.timeline",
                max: MAX_WORKER_TIMELINE_EVENTS,
            });
        }
        let mut event_ids = BTreeSet::new();
        let mut previous_sequence = 0;
        let mut previous_time = 0;
        for event in &self.timeline {
            event.validate()?;
            if event.worker_id != self.worker_id {
                return Err(WorkerRuntimeContractError::InvalidRelationship(
                    "worker_runtime_snapshot.timeline_worker",
                ));
            }
            if event.occurred_at_ms > self.observed_at_ms {
                return Err(WorkerRuntimeContractError::InvalidTimestamp(
                    "worker_runtime_snapshot.timeline_future_event",
                ));
            }
            if !event_ids.insert(event.event_id.as_str()) {
                return Err(WorkerRuntimeContractError::Duplicate(
                    "worker_runtime_snapshot.timeline",
                ));
            }
            if event.sequence <= previous_sequence || event.occurred_at_ms < previous_time {
                return Err(WorkerRuntimeContractError::InvalidRelationship(
                    "worker_runtime_snapshot.timeline_order",
                ));
            }
            previous_sequence = event.sequence;
            previous_time = event.occurred_at_ms;
        }
        Ok(())
    }

    /// Validate the snapshot against a current clock for stale-state semantics.
    pub fn validate_at(&self, now_ms: u64) -> Result<(), WorkerRuntimeContractError> {
        self.validate()?;
        validate_timestamp("worker_runtime_snapshot.now_ms", now_ms)?;
        if now_ms < self.observed_at_ms {
            return Err(WorkerRuntimeContractError::InvalidFreshness(
                "worker_runtime_snapshot.future_observation",
            ));
        }
        if self.state == WorkerRuntimeState::Stale && self.is_fresh(now_ms) {
            return Err(WorkerRuntimeContractError::InvalidRelationship(
                "worker_runtime_snapshot.stale_while_fresh",
            ));
        }
        Ok(())
    }

    /// Whether the publication envelope is fresh at `now_ms`.
    #[must_use]
    pub const fn is_fresh(&self, now_ms: u64) -> bool {
        now_ms >= self.observed_at_ms && now_ms <= self.fresh_until_ms
    }

    /// Whether the publication envelope has expired at `now_ms`.
    #[must_use]
    pub const fn is_stale_at(&self, now_ms: u64) -> bool {
        now_ms > self.fresh_until_ms
    }

    /// Return the honest presentation state, converting expired non-stale
    /// observations to `stale` without mutating the retained record.
    #[must_use]
    pub fn effective_state(&self, now_ms: u64) -> WorkerRuntimeState {
        if self.is_stale_at(now_ms) && self.state != WorkerRuntimeState::Stale {
            WorkerRuntimeState::Stale
        } else {
            self.state
        }
    }

    /// Admit a snapshot received over an untrusted boundary.
    pub fn admitted(self) -> Result<Self, WorkerRuntimeContractError> {
        self.validate()?;
        Ok(self)
    }

    /// Decode and admit a bounded JSON snapshot.
    pub fn from_json(body: &str) -> Result<Self, WorkerRuntimeContractError> {
        bounded_json(body)
    }

    /// Validate and encode a snapshot as bounded JSON.
    pub fn to_json(&self) -> Result<String, WorkerRuntimeContractError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|_| WorkerRuntimeContractError::MalformedWire)
    }
}

/// Target bound to a staged change set.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerChangeSetTarget {
    pub node_id: String,
    pub worker_id: Option<String>,
}

impl WorkerChangeSetTarget {
    fn validate(&self) -> Result<(), WorkerRuntimeContractError> {
        validate_identifier("change_set.target.node_id", &self.node_id)?;
        if let Some(worker_id) = &self.worker_id {
            validate_identifier("change_set.target.worker_id", worker_id)?;
        }
        Ok(())
    }
}

/// Phase of the global staged Action Console protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerChangeSetOperation {
    Preview,
    Commit,
    Cancel,
}

/// One normalized, typed worker mutation.  There is no command, path, or
/// arbitrary property map in this item.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerChangeSetItem {
    pub item_id: String,
    pub worker_id: String,
    pub action: WorkerAction,
}

/// Build the node-scoped authenticated request topic for the Action Console.
pub fn worker_change_set_action_topic(node_id: &str) -> Result<String, WorkerRuntimeContractError> {
    validate_identifier("change_set.topic.node_id", node_id)?;
    Ok(format!("{WORKER_CHANGE_SET_ACTION_TOPIC_PREFIX}/{node_id}"))
}

/// Build the node-scoped latest-wins result topic for the Action Console.
pub fn worker_change_set_result_topic(node_id: &str) -> Result<String, WorkerRuntimeContractError> {
    validate_identifier("change_set.topic.node_id", node_id)?;
    Ok(format!("{WORKER_CHANGE_SET_RESULT_TOPIC_PREFIX}/{node_id}"))
}

/// Bind a staged change set to its exact target, generation, typed items, and
/// operator-facing impact/recovery contract. The operation and request clock
/// are deliberately excluded so Preview, Commit, and Cancel can refer to the
/// same immutable staged intent.
pub fn worker_change_set_digest(
    target: &WorkerChangeSetTarget,
    expected_generation: u64,
    items: &[WorkerChangeSetItem],
    impact: &str,
    recovery: &str,
    arming: WorkerArmingRequirement,
) -> Result<String, WorkerRuntimeContractError> {
    use sha2::{Digest as _, Sha256};

    target.validate()?;
    if expected_generation == 0 {
        return Err(WorkerRuntimeContractError::InvalidGeneration(
            "change_set_digest.expected_generation",
        ));
    }
    if items.is_empty() || items.len() > MAX_WORKER_CHANGE_SET_ITEMS {
        return Err(WorkerRuntimeContractError::CapacityExceeded {
            field: "change_set_digest.items",
            max: MAX_WORKER_CHANGE_SET_ITEMS,
        });
    }
    let mut item_ids = BTreeSet::new();
    for item in items {
        item.validate()?;
        if target
            .worker_id
            .as_deref()
            .is_some_and(|worker_id| worker_id != item.worker_id)
        {
            return Err(WorkerRuntimeContractError::InvalidRelationship(
                "change_set_digest.target_worker",
            ));
        }
        if !item_ids.insert(item.item_id.as_str()) {
            return Err(WorkerRuntimeContractError::Duplicate(
                "change_set_digest.items",
            ));
        }
    }
    validate_text(
        "change_set_digest.impact",
        impact,
        MAX_WORKER_TEXT_BYTES,
        false,
    )?;
    validate_text(
        "change_set_digest.recovery",
        recovery,
        MAX_WORKER_TEXT_BYTES,
        false,
    )?;
    let mut canonical_items = items.to_vec();
    canonical_items.sort_by(|left, right| left.item_id.cmp(&right.item_id));
    let canonical = serde_json::to_vec(&(
        target,
        expected_generation,
        canonical_items,
        impact,
        recovery,
        arming,
    ))
    .map_err(|_| WorkerRuntimeContractError::MalformedWire)?;
    let digest = Sha256::digest(canonical);
    Ok(format!("sha256:{digest:x}"))
}

impl WorkerChangeSetItem {
    fn validate(&self) -> Result<(), WorkerRuntimeContractError> {
        validate_identifier("change_set.item.item_id", &self.item_id)?;
        validate_identifier("change_set.item.worker_id", &self.worker_id)
    }
}

/// Request for preview, commit, or cancellation of a typed worker change set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerChangeSetRequest {
    pub schema_version: u16,
    pub request_id: String,
    pub operation: WorkerChangeSetOperation,
    pub target: WorkerChangeSetTarget,
    pub expected_generation: u64,
    pub items: Vec<WorkerChangeSetItem>,
    pub impact: String,
    pub recovery: String,
    pub arming: WorkerArmingRequirement,
    pub digest: String,
    pub requested_at_ms: u64,
    pub expires_at_ms: u64,
    /// Short-lived exact-body capability minted by the existing shell action
    /// authority. Producers leave this absent until the validated body is
    /// authorized; consumers must verify it before dispatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub armed_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerChangeSetRequestWire {
    schema_version: u16,
    request_id: String,
    operation: WorkerChangeSetOperation,
    target: WorkerChangeSetTarget,
    expected_generation: u64,
    items: Vec<WorkerChangeSetItem>,
    impact: String,
    recovery: String,
    arming: WorkerArmingRequirement,
    digest: String,
    requested_at_ms: u64,
    expires_at_ms: u64,
    #[serde(default)]
    armed_token: Option<String>,
}

impl<'de> Deserialize<'de> for WorkerChangeSetRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = WorkerChangeSetRequestWire::deserialize(deserializer)?;
        let request = Self {
            schema_version: wire.schema_version,
            request_id: wire.request_id,
            operation: wire.operation,
            target: wire.target,
            expected_generation: wire.expected_generation,
            items: wire.items,
            impact: wire.impact,
            recovery: wire.recovery,
            arming: wire.arming,
            digest: wire.digest,
            requested_at_ms: wire.requested_at_ms,
            expires_at_ms: wire.expires_at_ms,
            armed_token: wire.armed_token,
        };
        request.validate().map_err(de::Error::custom)?;
        Ok(request)
    }
}

impl WorkerChangeSetRequest {
    /// Construct and validate a typed preview/commit/cancel request.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        request_id: impl Into<String>,
        operation: WorkerChangeSetOperation,
        target: WorkerChangeSetTarget,
        expected_generation: u64,
        items: Vec<WorkerChangeSetItem>,
        impact: impl Into<String>,
        recovery: impl Into<String>,
        arming: WorkerArmingRequirement,
        digest: impl Into<String>,
        requested_at_ms: u64,
        expires_at_ms: u64,
    ) -> Result<Self, WorkerRuntimeContractError> {
        let request = Self {
            schema_version: WORKER_RUNTIME_SCHEMA_VERSION,
            request_id: request_id.into(),
            operation,
            target,
            expected_generation,
            items,
            impact: impact.into(),
            recovery: recovery.into(),
            arming,
            digest: digest.into(),
            requested_at_ms,
            expires_at_ms,
            armed_token: None,
        };
        request.validate()?;
        Ok(request)
    }

    /// Validate the request's schema, typed items, freshness, and redaction.
    pub fn validate(&self) -> Result<(), WorkerRuntimeContractError> {
        validate_schema("change_set_request.schema_version", self.schema_version)?;
        validate_identifier("change_set_request.request_id", &self.request_id)?;
        self.target.validate()?;
        if self.expected_generation == 0 {
            return Err(WorkerRuntimeContractError::InvalidGeneration(
                "change_set_request.expected_generation",
            ));
        }
        if self.items.is_empty() || self.items.len() > MAX_WORKER_CHANGE_SET_ITEMS {
            return Err(WorkerRuntimeContractError::CapacityExceeded {
                field: "change_set_request.items",
                max: MAX_WORKER_CHANGE_SET_ITEMS,
            });
        }
        let mut item_ids = BTreeSet::new();
        for item in &self.items {
            item.validate()?;
            if !item_ids.insert(item.item_id.as_str()) {
                return Err(WorkerRuntimeContractError::Duplicate(
                    "change_set_request.items",
                ));
            }
            if self
                .target
                .worker_id
                .as_deref()
                .is_some_and(|worker_id| worker_id != item.worker_id)
            {
                return Err(WorkerRuntimeContractError::InvalidRelationship(
                    "change_set_request.target_worker",
                ));
            }
        }
        validate_text(
            "change_set_request.impact",
            &self.impact,
            MAX_WORKER_TEXT_BYTES,
            false,
        )?;
        validate_text(
            "change_set_request.recovery",
            &self.recovery,
            MAX_WORKER_TEXT_BYTES,
            false,
        )?;
        validate_digest("change_set_request.digest", &self.digest)?;
        let expected_digest = worker_change_set_digest(
            &self.target,
            self.expected_generation,
            &self.items,
            &self.impact,
            &self.recovery,
            self.arming,
        )?;
        if self.digest != expected_digest {
            return Err(WorkerRuntimeContractError::InvalidDigest(
                "change_set_request.digest_mismatch",
            ));
        }
        validate_timestamp("change_set_request.requested_at_ms", self.requested_at_ms)?;
        validate_timestamp("change_set_request.expires_at_ms", self.expires_at_ms)?;
        if self.expires_at_ms <= self.requested_at_ms
            || self.expires_at_ms - self.requested_at_ms > MAX_WORKER_CHANGE_SET_TTL_MS
        {
            return Err(WorkerRuntimeContractError::InvalidFreshness(
                "change_set_request.expires_at_ms",
            ));
        }
        if let Some(token) = &self.armed_token {
            if token.len() > MAX_WORKER_ARMED_TOKEN_BYTES {
                return Err(WorkerRuntimeContractError::FieldTooLong(
                    "change_set_request.armed_token",
                ));
            }
            if token.is_empty()
                || token.trim() != token
                || !token.is_ascii()
                || token.chars().any(char::is_whitespace)
            {
                return Err(WorkerRuntimeContractError::InvalidField(
                    "change_set_request.armed_token",
                ));
            }
        }
        Ok(())
    }

    /// Validate the request against a current clock, including expiry.
    pub fn validate_at(&self, now_ms: u64) -> Result<(), WorkerRuntimeContractError> {
        self.validate()?;
        validate_timestamp("change_set_request.now_ms", now_ms)?;
        if now_ms < self.requested_at_ms {
            return Err(WorkerRuntimeContractError::InvalidFreshness(
                "change_set_request.future_request",
            ));
        }
        if now_ms > self.expires_at_ms {
            return Err(WorkerRuntimeContractError::Expired(
                "change_set_request.expires_at_ms",
            ));
        }
        Ok(())
    }

    /// Decode and admit a bounded JSON request.
    pub fn from_json(body: &str) -> Result<Self, WorkerRuntimeContractError> {
        bounded_json(body)
    }

    /// Validate and encode a request as bounded JSON.
    pub fn to_json(&self) -> Result<String, WorkerRuntimeContractError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|_| WorkerRuntimeContractError::MalformedWire)
    }
}

/// Per-item outcome in a change-set result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerChangeSetItemOutcome {
    Applied,
    Refused,
    Failed,
    NotApplicable,
    Unavailable,
    Cancelled,
}

/// Honest result for one normalized change-set item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerChangeSetItemResult {
    pub item_id: String,
    pub outcome: WorkerChangeSetItemOutcome,
    pub detail: Option<String>,
}

impl WorkerChangeSetItemResult {
    fn validate(&self) -> Result<(), WorkerRuntimeContractError> {
        validate_identifier("change_set_result.item.item_id", &self.item_id)?;
        if let Some(detail) = &self.detail {
            validate_text(
                "change_set_result.item.detail",
                detail,
                MAX_WORKER_TEXT_BYTES,
                false,
            )?;
        }
        Ok(())
    }
}

/// Aggregate outcome of the staged Action Console protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerChangeSetOutcome {
    Previewed,
    Committed,
    Cancelled,
    Refused,
    StaleGeneration,
    Expired,
    Partial,
    Failed,
}

/// Typed result; partial success is represented per item, never as atomic
/// cross-node rollback or a free-form diagnostic body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerChangeSetResult {
    pub schema_version: u16,
    pub request_id: String,
    pub operation: WorkerChangeSetOperation,
    pub outcome: WorkerChangeSetOutcome,
    pub target: WorkerChangeSetTarget,
    pub expected_generation: u64,
    pub actual_generation: u64,
    pub items: Vec<WorkerChangeSetItemResult>,
    pub audit_id: Option<String>,
    pub completed_at_ms: u64,
    pub detail: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerChangeSetResultWire {
    schema_version: u16,
    request_id: String,
    operation: WorkerChangeSetOperation,
    outcome: WorkerChangeSetOutcome,
    target: WorkerChangeSetTarget,
    expected_generation: u64,
    actual_generation: u64,
    items: Vec<WorkerChangeSetItemResult>,
    audit_id: Option<String>,
    completed_at_ms: u64,
    detail: Option<String>,
}

impl<'de> Deserialize<'de> for WorkerChangeSetResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = WorkerChangeSetResultWire::deserialize(deserializer)?;
        let result = Self {
            schema_version: wire.schema_version,
            request_id: wire.request_id,
            operation: wire.operation,
            outcome: wire.outcome,
            target: wire.target,
            expected_generation: wire.expected_generation,
            actual_generation: wire.actual_generation,
            items: wire.items,
            audit_id: wire.audit_id,
            completed_at_ms: wire.completed_at_ms,
            detail: wire.detail,
        };
        result.validate().map_err(de::Error::custom)?;
        Ok(result)
    }
}

impl WorkerChangeSetResult {
    /// Validate a typed result and every bounded per-item outcome.
    pub fn validate(&self) -> Result<(), WorkerRuntimeContractError> {
        validate_schema("change_set_result.schema_version", self.schema_version)?;
        validate_identifier("change_set_result.request_id", &self.request_id)?;
        self.target.validate()?;
        if self.expected_generation == 0 || self.actual_generation == 0 {
            return Err(WorkerRuntimeContractError::InvalidGeneration(
                "change_set_result.generation",
            ));
        }
        validate_timestamp("change_set_result.completed_at_ms", self.completed_at_ms)?;
        if self.items.len() > MAX_WORKER_CHANGE_SET_ITEMS {
            return Err(WorkerRuntimeContractError::CapacityExceeded {
                field: "change_set_result.items",
                max: MAX_WORKER_CHANGE_SET_ITEMS,
            });
        }
        let mut item_ids = BTreeSet::new();
        for item in &self.items {
            item.validate()?;
            if !item_ids.insert(item.item_id.as_str()) {
                return Err(WorkerRuntimeContractError::Duplicate(
                    "change_set_result.items",
                ));
            }
        }
        if let Some(audit_id) = &self.audit_id {
            validate_identifier("change_set_result.audit_id", audit_id)?;
        }
        if let Some(detail) = &self.detail {
            validate_text(
                "change_set_result.detail",
                detail,
                MAX_WORKER_TEXT_BYTES,
                false,
            )?;
        }
        let outcome_is_admitted = match self.operation {
            WorkerChangeSetOperation::Preview => matches!(
                self.outcome,
                WorkerChangeSetOutcome::Previewed
                    | WorkerChangeSetOutcome::Refused
                    | WorkerChangeSetOutcome::Expired
                    | WorkerChangeSetOutcome::Failed
            ),
            WorkerChangeSetOperation::Commit => matches!(
                self.outcome,
                WorkerChangeSetOutcome::Committed
                    | WorkerChangeSetOutcome::Partial
                    | WorkerChangeSetOutcome::Refused
                    | WorkerChangeSetOutcome::StaleGeneration
                    | WorkerChangeSetOutcome::Expired
                    | WorkerChangeSetOutcome::Failed
            ),
            WorkerChangeSetOperation::Cancel => matches!(
                self.outcome,
                WorkerChangeSetOutcome::Cancelled
                    | WorkerChangeSetOutcome::Refused
                    | WorkerChangeSetOutcome::StaleGeneration
                    | WorkerChangeSetOutcome::Expired
                    | WorkerChangeSetOutcome::Failed
            ),
        };
        if !outcome_is_admitted {
            return Err(WorkerRuntimeContractError::InvalidRelationship(
                "change_set_result.operation_outcome",
            ));
        }
        Ok(())
    }

    /// Decode and admit a bounded JSON result.
    pub fn from_json(body: &str) -> Result<Self, WorkerRuntimeContractError> {
        bounded_json(body)
    }

    /// Validate and encode a result as bounded JSON.
    pub fn to_json(&self) -> Result<String, WorkerRuntimeContractError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|_| WorkerRuntimeContractError::MalformedWire)
    }
}

fn bounded_json<T>(body: &str) -> Result<T, WorkerRuntimeContractError>
where
    T: for<'de> Deserialize<'de>,
{
    if body.len() > MAX_WORKER_RUNTIME_WIRE_BYTES {
        return Err(WorkerRuntimeContractError::PayloadTooLarge);
    }
    serde_json::from_str(body).map_err(|_| WorkerRuntimeContractError::MalformedWire)
}

const fn validate_schema(
    field: &'static str,
    found: u16,
) -> Result<(), WorkerRuntimeContractError> {
    if found != WORKER_RUNTIME_SCHEMA_VERSION {
        return Err(WorkerRuntimeContractError::UnsupportedSchema { field, found });
    }
    Ok(())
}

fn validate_identifier(field: &'static str, value: &str) -> Result<(), WorkerRuntimeContractError> {
    if value.len() > MAX_WORKER_IDENTIFIER_BYTES {
        return Err(WorkerRuntimeContractError::FieldTooLong(field));
    }
    if value.is_empty()
        || value.trim() != value
        || !value.is_ascii()
        || !value
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphanumeric())
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
        })
    {
        return Err(WorkerRuntimeContractError::InvalidField(field));
    }
    Ok(())
}

fn validate_topic(field: &'static str, value: &str) -> Result<(), WorkerRuntimeContractError> {
    if value.len() > MAX_WORKER_TOPIC_BYTES {
        return Err(WorkerRuntimeContractError::FieldTooLong(field));
    }
    if value.is_empty()
        || value.trim() != value
        || !value.is_ascii()
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains("//")
        || value.contains("..")
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':' | '/')
        })
    {
        return Err(WorkerRuntimeContractError::InvalidField(field));
    }
    Ok(())
}

fn validate_text(
    field: &'static str,
    value: &str,
    max_bytes: usize,
    allow_empty: bool,
) -> Result<(), WorkerRuntimeContractError> {
    if value.len() > max_bytes {
        return Err(WorkerRuntimeContractError::FieldTooLong(field));
    }
    if (!allow_empty && value.is_empty()) || value.trim() != value {
        return Err(WorkerRuntimeContractError::InvalidField(field));
    }
    if value.chars().any(char::is_control) {
        return Err(WorkerRuntimeContractError::InvalidField(field));
    }
    let lower = value.to_ascii_lowercase();
    let secret_markers = [
        "password=",
        "password:",
        "passwd=",
        "passwd:",
        "secret=",
        "secret:",
        "token=",
        "token:",
        "bearer ",
        "authorization:",
        "proxy-authorization:",
        "cookie:",
        "set-cookie:",
        "api_key=",
        "apikey=",
        "access_token=",
        "refresh_token=",
        "client_secret=",
        "private key",
        "ssh-rsa ",
        "-----begin",
        "-----end",
    ];
    if secret_markers.iter().any(|marker| lower.contains(marker))
        || value.contains("://")
        || value.starts_with('/')
        || value.starts_with("~/")
        || value.contains("../")
        || value.contains("..\\")
        || value.contains("$(")
        || value.contains('`')
    {
        return Err(WorkerRuntimeContractError::SecretShapedValue(field));
    }
    Ok(())
}

fn validate_unique_identifiers(
    field: &'static str,
    values: &[String],
) -> Result<(), WorkerRuntimeContractError> {
    let mut seen = BTreeSet::new();
    for value in values {
        validate_identifier(field, value)?;
        if !seen.insert(value.as_str()) {
            return Err(WorkerRuntimeContractError::Duplicate(field));
        }
    }
    Ok(())
}

fn validate_unique_topics(
    field: &'static str,
    values: &[String],
) -> Result<(), WorkerRuntimeContractError> {
    let mut seen = BTreeSet::new();
    for value in values {
        validate_topic(field, value)?;
        if !seen.insert(value.as_str()) {
            return Err(WorkerRuntimeContractError::Duplicate(field));
        }
    }
    Ok(())
}

const fn validate_timestamp(
    field: &'static str,
    value: u64,
) -> Result<(), WorkerRuntimeContractError> {
    if value == 0 {
        return Err(WorkerRuntimeContractError::InvalidTimestamp(field));
    }
    Ok(())
}

fn validate_digest(field: &'static str, value: &str) -> Result<(), WorkerRuntimeContractError> {
    if value.len() != "sha256:".len() + 64
        || !value.starts_with("sha256:")
        || !value["sha256:".len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
    {
        return Err(WorkerRuntimeContractError::InvalidDigest(field));
    }
    Ok(())
}

const fn validate_state_reason(
    state: WorkerRuntimeState,
    reason: Option<WorkerRuntimeReason>,
) -> Result<(), WorkerRuntimeContractError> {
    if state.requires_reason() && reason.is_none() {
        return Err(WorkerRuntimeContractError::InvalidStateReason(
            "worker_runtime_snapshot.state_reason.required",
        ));
    }
    if !state.requires_reason() && reason.is_some() {
        return Err(WorkerRuntimeContractError::InvalidStateReason(
            "worker_runtime_snapshot.state_reason.unexpected",
        ));
    }
    let valid = matches!(
        (state, reason),
        (
            WorkerRuntimeState::NotApplicable,
            Some(WorkerRuntimeReason::NotApplicable)
        ) | (
            WorkerRuntimeState::Unconfigured,
            Some(WorkerRuntimeReason::NotConfigured)
        ) | (
            WorkerRuntimeState::Backoff,
            Some(
                WorkerRuntimeReason::CrashLoop
                    | WorkerRuntimeReason::ResourceLimit
                    | WorkerRuntimeReason::DependencyUnavailable,
            ),
        ) | (
            WorkerRuntimeState::Paused,
            Some(WorkerRuntimeReason::OperatorPaused)
        ) | (
            WorkerRuntimeState::Failed,
            Some(
                WorkerRuntimeReason::CrashLoop
                    | WorkerRuntimeReason::DependencyUnavailable
                    | WorkerRuntimeReason::ProviderUnavailable
                    | WorkerRuntimeReason::Unknown,
            ),
        ) | (
            WorkerRuntimeState::Stale,
            Some(WorkerRuntimeReason::ObservationStale)
        ) | (
            WorkerRuntimeState::Unavailable,
            Some(
                WorkerRuntimeReason::CapabilityMissing
                    | WorkerRuntimeReason::DependencyUnavailable
                    | WorkerRuntimeReason::ProviderUnavailable
                    | WorkerRuntimeReason::ResourceLimit
                    | WorkerRuntimeReason::Unknown,
            ),
        ) | (
            WorkerRuntimeState::Starting
                | WorkerRuntimeState::Running
                | WorkerRuntimeState::Stopped,
            None,
        )
    );
    if valid {
        Ok(())
    } else {
        Err(WorkerRuntimeContractError::InvalidStateReason(
            "worker_runtime_snapshot.state_reason.mismatch",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract() -> WorkerContract {
        let mut contract =
            WorkerContract::new("host-state", WorkerGroup::Observation, "Host state")
                .expect("valid contract");
        contract.description = "Credential-free host observations".to_owned();
        contract.cadence = WorkerCadence::Periodic {
            min_interval_ms: 5_000,
            max_interval_ms: 60_000,
        };
        contract.publications = vec!["state/mackesd/observation/workers/host-state".to_owned()];
        contract.actions = vec![WorkerActionDescriptor {
            action: WorkerAction::Refresh,
            label: "Refresh observation".to_owned(),
            arming: WorkerArmingRequirement::None,
        }];
        contract.validate().expect("fixture remains valid");
        contract
    }

    fn relation() -> WorkerRelation {
        WorkerRelation::new(
            "rel-host-state-output",
            WorkerRelationKind::Publishes,
            WorkerRelationEndpoint::Worker {
                worker_id: "host-state".to_owned(),
            },
            WorkerRelationEndpoint::Output {
                worker_id: "host-state".to_owned(),
                output_kind: "host_facts".to_owned(),
            },
            Some("publishes host facts".to_owned()),
        )
        .expect("valid relation")
    }

    fn event(sequence: u64, occurred_at_ms: u64) -> WorkerTimelineEvent {
        WorkerTimelineEvent::new(
            format!("event-{sequence}"),
            sequence,
            "host-state",
            occurred_at_ms,
            if sequence == 1 {
                WorkerTimelineEventKind::Registered
            } else {
                WorkerTimelineEventKind::StateChanged
            },
            (sequence != 1).then_some(WorkerRuntimeState::Running),
            if sequence == 1 {
                "worker registered"
            } else {
                "worker is running"
            },
            None,
        )
        .expect("valid event")
    }

    fn snapshot() -> WorkerRuntimeSnapshot {
        let mut snapshot = WorkerRuntimeSnapshot::new(
            "snapshot-host-state-1",
            "seat-15",
            "host-state",
            WorkerGroup::Observation,
            1,
            WorkerRuntimeState::Running,
            1_000,
            2_000,
            2_100,
            62_100,
        )
        .expect("valid snapshot");
        snapshot.relations = vec![relation()];
        snapshot.timeline = vec![event(1, 1_100), event(2, 1_900)];
        snapshot.validate().expect("fixture remains valid");
        snapshot
    }

    fn request() -> WorkerChangeSetRequest {
        let target = WorkerChangeSetTarget {
            node_id: "seat-15".to_owned(),
            worker_id: Some("host-state".to_owned()),
        };
        let items = vec![WorkerChangeSetItem {
            item_id: "item-1".to_owned(),
            worker_id: "host-state".to_owned(),
            action: WorkerAction::Refresh,
        }];
        let impact = "refresh one bounded observation";
        let recovery = "retain the previous admitted snapshot";
        let arming = WorkerArmingRequirement::None;
        let digest = worker_change_set_digest(&target, 1, &items, impact, recovery, arming)
            .expect("canonical digest");
        WorkerChangeSetRequest::new(
            "request-1",
            WorkerChangeSetOperation::Preview,
            target,
            1,
            items,
            impact,
            recovery,
            arming,
            digest,
            10_000,
            20_000,
        )
        .expect("valid request")
    }

    #[test]
    fn contract_and_snapshot_round_trip_with_nested_history() {
        let contract_body = contract().to_json().expect("encode contract");
        let decoded_contract = WorkerContract::from_json(&contract_body).expect("decode contract");
        assert_eq!(decoded_contract, contract());

        let snapshot_body = snapshot().to_json().expect("encode snapshot");
        let decoded_snapshot =
            WorkerRuntimeSnapshot::from_json(&snapshot_body).expect("decode snapshot");
        assert_eq!(decoded_snapshot, snapshot());
        assert!(decoded_snapshot.is_fresh(10_000));
        assert!(decoded_snapshot.is_stale_at(62_101));
        assert_eq!(
            decoded_snapshot.effective_state(62_101),
            WorkerRuntimeState::Stale
        );
    }

    #[test]
    fn every_runtime_state_has_a_closed_wire_spelling() {
        let states = [
            WorkerRuntimeState::NotApplicable,
            WorkerRuntimeState::Unconfigured,
            WorkerRuntimeState::Starting,
            WorkerRuntimeState::Running,
            WorkerRuntimeState::Backoff,
            WorkerRuntimeState::Paused,
            WorkerRuntimeState::Stopped,
            WorkerRuntimeState::Failed,
            WorkerRuntimeState::Stale,
            WorkerRuntimeState::Unavailable,
        ];
        for state in states {
            let body = serde_json::to_string(&state).expect("encode state");
            assert_eq!(
                serde_json::from_str::<WorkerRuntimeState>(&body).expect("decode state"),
                state
            );
        }
    }

    #[test]
    fn hostile_unknown_schema_unknown_fields_and_open_state_are_rejected() {
        let body = contract().to_json().expect("encode contract");
        let unknown_schema = body.replacen(
            &format!("\"schema_version\":{WORKER_RUNTIME_SCHEMA_VERSION}"),
            "\"schema_version\":99",
            1,
        );
        assert!(serde_json::from_str::<WorkerContract>(&unknown_schema).is_err());

        let unknown_field = format!("{body}}}").replacen("}}", ",\"command\":\"id\"}}", 1);
        assert!(serde_json::from_str::<WorkerContract>(&unknown_field).is_err());
        assert!(serde_json::from_str::<WorkerRuntimeState>("\"running_now\"").is_err());
        assert!(WorkerContract::new("../etc", WorkerGroup::Control, "bad").is_err());
        assert!(WorkerContract::new("worker", WorkerGroup::Control, "password=leak").is_err());
    }

    #[test]
    fn every_versioned_boundary_rejects_unknown_schema_before_admission() {
        let hostile_schema = |body: String| {
            body.replacen(
                &format!("\"schema_version\":{WORKER_RUNTIME_SCHEMA_VERSION}"),
                "\"schema_version\":99",
                1,
            )
        };

        assert_eq!(
            WorkerContract::from_json(&hostile_schema(
                contract().to_json().expect("encode contract"),
            )),
            Err(WorkerRuntimeContractError::MalformedWire)
        );
        assert_eq!(
            WorkerRelation::from_json(&hostile_schema(
                relation().to_json().expect("encode relation"),
            )),
            Err(WorkerRuntimeContractError::MalformedWire)
        );
        assert_eq!(
            WorkerTimelineEvent::from_json(&hostile_schema(
                event(1, 1_100).to_json().expect("encode event"),
            )),
            Err(WorkerRuntimeContractError::MalformedWire)
        );
        assert_eq!(
            WorkerRuntimeSnapshot::from_json(&hostile_schema(
                snapshot().to_json().expect("encode snapshot"),
            )),
            Err(WorkerRuntimeContractError::MalformedWire)
        );
        let request = request();
        assert_eq!(
            WorkerChangeSetRequest::from_json(&hostile_schema(
                request.to_json().expect("encode request"),
            )),
            Err(WorkerRuntimeContractError::MalformedWire)
        );
        let result = WorkerChangeSetResult {
            schema_version: WORKER_RUNTIME_SCHEMA_VERSION,
            request_id: "request-1".to_owned(),
            operation: WorkerChangeSetOperation::Preview,
            outcome: WorkerChangeSetOutcome::Previewed,
            target: request.target,
            expected_generation: 1,
            actual_generation: 1,
            items: vec![WorkerChangeSetItemResult {
                item_id: "item-1".to_owned(),
                outcome: WorkerChangeSetItemOutcome::Applied,
                detail: Some("preview admitted".to_owned()),
            }],
            audit_id: Some("audit-1".to_owned()),
            completed_at_ms: 11_000,
            detail: None,
        };
        assert_eq!(
            WorkerChangeSetResult::from_json(&hostile_schema(
                result.to_json().expect("encode result"),
            )),
            Err(WorkerRuntimeContractError::MalformedWire)
        );
    }

    #[test]
    fn snapshot_enforces_freshness_reasons_order_and_512_event_cap() {
        let mut stale_reason = snapshot();
        stale_reason.state = WorkerRuntimeState::Unavailable;
        assert!(stale_reason.validate().is_err());

        let mut out_of_order = snapshot();
        out_of_order.timeline.swap(0, 1);
        assert!(out_of_order.validate().is_err());

        let mut too_many = snapshot();
        too_many.timeline = (1..=MAX_WORKER_TIMELINE_EVENTS as u64 + 1)
            .map(|sequence| event(sequence, 1_000 + sequence))
            .collect();
        assert_eq!(
            too_many.validate(),
            Err(WorkerRuntimeContractError::CapacityExceeded {
                field: "worker_runtime_snapshot.timeline",
                max: MAX_WORKER_TIMELINE_EVENTS,
            })
        );

        let mut invalid_freshness = snapshot();
        invalid_freshness.fresh_until_ms = invalid_freshness.published_at_ms;
        assert!(invalid_freshness.validate().is_err());
    }

    #[test]
    fn redaction_rejects_secrets_paths_urls_and_raw_command_shaped_text() {
        for value in [
            "Authorization: Bearer abc",
            "password=abc",
            "/var/log/mackesd.log",
            "https://example.invalid",
            "$(id)",
            "`cat secret`",
        ] {
            let mut record = snapshot();
            record.timeline[0].summary = value.to_owned();
            assert!(record.validate().is_err(), "must reject {value}");
        }
        assert!(WorkerTimelineEvent::new(
            "event-secret",
            1,
            "host-state",
            1_000,
            WorkerTimelineEventKind::Failure,
            None,
            "worker failed",
            Some("token=abc".to_owned()),
        )
        .is_err());
    }

    #[test]
    fn relation_and_typed_change_set_round_trip_without_open_mutation_fields() {
        let relation_body = relation().to_json().expect("encode relation");
        assert_eq!(
            WorkerRelation::from_json(&relation_body).expect("decode relation"),
            relation()
        );

        let request = request();
        let request_body = request.to_json().expect("encode request");
        assert_eq!(
            WorkerChangeSetRequest::from_json(&request_body).expect("decode request"),
            request
        );
        assert!(request.validate_at(20_001).is_err());

        let hostile = request_body.replacen(
            "\"items\":[",
            "\"items\":[{\"item_id\":\"item-1\",\"worker_id\":\"host-state\",\"action\":\"refresh\",\"path\":\"/etc/passwd\"}],\n            \"ignored\":[",
            1,
        );
        assert!(serde_json::from_str::<WorkerChangeSetRequest>(&hostile).is_err());

        let result = WorkerChangeSetResult {
            schema_version: WORKER_RUNTIME_SCHEMA_VERSION,
            request_id: "request-1".to_owned(),
            operation: WorkerChangeSetOperation::Preview,
            outcome: WorkerChangeSetOutcome::Previewed,
            target: request.target,
            expected_generation: 1,
            actual_generation: 1,
            items: vec![WorkerChangeSetItemResult {
                item_id: "item-1".to_owned(),
                outcome: WorkerChangeSetItemOutcome::Applied,
                detail: Some("preview admitted".to_owned()),
            }],
            audit_id: Some("audit-1".to_owned()),
            completed_at_ms: 11_000,
            detail: None,
        };
        let result_body = result.to_json().expect("encode result");
        assert_eq!(
            WorkerChangeSetResult::from_json(&result_body).expect("decode result"),
            result
        );
    }

    #[test]
    fn action_console_topics_digest_and_capability_field_are_closed_and_stable() {
        let mut request = request();
        request.digest = worker_change_set_digest(
            &request.target,
            request.expected_generation,
            &request.items,
            &request.impact,
            &request.recovery,
            request.arming,
        )
        .expect("canonical digest");
        request.armed_token = Some("v1.test-capability".to_string());
        let decoded =
            WorkerChangeSetRequest::from_json(&request.to_json().expect("authorized request body"))
                .expect("authorized request admission");
        assert_eq!(decoded, request);
        let mut oversized_token = request.clone();
        oversized_token.armed_token = Some("x".repeat(MAX_WORKER_ARMED_TOKEN_BYTES + 1));
        assert!(oversized_token.validate().is_err());
        assert_eq!(
            worker_change_set_action_topic("seat-15").expect("action topic"),
            "action/workers/change-set/seat-15"
        );
        assert_eq!(
            worker_change_set_result_topic("seat-15").expect("result topic"),
            "state/workers/change-set/seat-15"
        );
        assert!(worker_change_set_action_topic("../../escape").is_err());

        let mut changed_items = request.items.clone();
        changed_items[0].action = WorkerAction::Stop;
        assert_ne!(
            request.digest,
            worker_change_set_digest(
                &request.target,
                request.expected_generation,
                &changed_items,
                &request.impact,
                &request.recovery,
                request.arming,
            )
            .expect("changed digest")
        );

        let mut tampered = request.clone();
        tampered.items[0].action = WorkerAction::Stop;
        assert_eq!(
            tampered.validate(),
            Err(WorkerRuntimeContractError::InvalidDigest(
                "change_set_request.digest_mismatch"
            ))
        );

        let mut first = request.items.clone();
        first.push(WorkerChangeSetItem {
            item_id: "item-0".to_owned(),
            worker_id: "host-state".to_owned(),
            action: WorkerAction::Restart,
        });
        let mut reversed = first.clone();
        reversed.reverse();
        assert_eq!(
            worker_change_set_digest(
                &request.target,
                request.expected_generation,
                &first,
                &request.impact,
                &request.recovery,
                request.arming,
            )
            .expect("first order digest"),
            worker_change_set_digest(
                &request.target,
                request.expected_generation,
                &reversed,
                &request.impact,
                &request.recovery,
                request.arming,
            )
            .expect("reversed order digest")
        );
    }

    #[test]
    fn oversized_wire_is_rejected_before_json_admission() {
        let body = "x".repeat(MAX_WORKER_RUNTIME_WIRE_BYTES + 1);
        assert_eq!(
            WorkerRuntimeSnapshot::from_json(&body),
            Err(WorkerRuntimeContractError::PayloadTooLarge)
        );
    }
}
