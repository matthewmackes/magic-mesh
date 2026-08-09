//! Versioned universal resource-catalog contract for WL-FUNC-019.
//!
//! This module is deliberately data-only. Discovery workers will eventually
//! normalize their observations into [`ResourceCatalog`], and shell/client
//! consumers may admit that catalog after validation. Nothing here accepts a
//! command line, executable path, environment, public-network scope, or
//! free-form URL. A launch-capable action must bind a structured transport to a
//! fingerprinted, registered client capability.

use serde::{de, Deserialize, Deserializer, Serialize};
use sha2::{Digest as _, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};

/// Canonical retained-latest Bus topic for the universal resource catalog.
pub const RESOURCE_CATALOG_TOPIC: &str = "state/resources/catalog";
/// Canonical retained-latest Bus topic for the safe resource discovery projection.
pub const RESOURCE_DISCOVERY_TOPIC: &str = "state/resources/discovery";
/// Prefix for the publisher-attestation topic keyed by catalog publisher.
///
/// Catalogs and proofs are retained separately for backward-compatible
/// consumers, but a publisher-specific topic prevents one node's proof from
/// being mistaken for another node's latest catalog.
pub const RESOURCE_PUBLISHER_ATTESTATION_TOPIC_PREFIX: &str =
    "state/resources/publisher-attestation";
/// Secret-store key identifier used for resource-catalog HMAC publication.
pub const RESOURCE_PUBLISHER_ATTESTATION_KEY_ID: &str = "resource-publisher-hmac-v1";
/// Freshness window for a resource-publisher HMAC proof.
pub const RESOURCE_PUBLISHER_ATTESTATION_TTL_MS: u64 = 5 * 60 * 1_000;
/// The only resource-contract schema currently admitted by consumers.
pub const RESOURCE_CONTRACT_VERSION: u16 = 1;
/// Minimum useful freshness lifetime for a published observation.
pub const MIN_RESOURCE_TTL_MS: u64 = 1_000;
/// Maximum freshness lifetime or offline-card retention represented by v1.
pub const MAX_RESOURCE_TTL_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
/// Maximum encoded catalog body accepted by [`ResourceCatalog::from_json`].
pub const MAX_RESOURCE_CATALOG_BYTES: usize = 2 * 1024 * 1024;

const MAX_IDENTIFIER_BYTES: usize = 255;
const MAX_DISPLAY_NAME_BYTES: usize = 512;
const MAX_SUMMARY_BYTES: usize = 1_024;
const MAX_FAILURE_MESSAGE_BYTES: usize = 1_024;
const MAX_ENDPOINT_PATH_BYTES: usize = 512;
const MAX_CARDS: usize = 4_096;
const MAX_ALIASES: usize = 32;
const MAX_TRANSPORTS: usize = 32;
const MAX_CAPABILITIES: usize = 32;
const MAX_PROVENANCE: usize = 16;
const MAX_ACTIONS: usize = 16;
const MAX_AUTH_METHODS: usize = 8;
const MAX_FEATURES: usize = 32;
const MAX_SAFE_ACTIONS: usize = 16;
const MAX_OPERATING_ROLES: usize = 3;
const MAX_CONFIGURATION_FIELDS: usize = 32;
const MAX_CONFIGURATION_CHOICES: usize = 32;
const MAX_SERVICE_BUS_TOPICS: usize = 32;
const MAX_SERVICE_HOSTING_NODES: usize = 64;
const MAX_SERVICE_DEPENDENCIES: usize = 64;
const MAX_DISCOVERY_ENTRIES: usize = MAX_CARDS;
const MAX_DISCOVERY_SOURCES: usize = MAX_PROVENANCE;
const MAX_DISCOVERY_SCOPES: usize = 4;
const MAX_DISCOVERY_PROTOCOLS: usize = MAX_TRANSPORTS;
const MAX_DISCOVERY_ACTIONS: usize = MAX_ACTIONS;
const RESOURCE_ID_PREFIX: &str = "resource:v1:";
const CAPABILITY_FINGERPRINT_PREFIX: &str = "capability:v1:";
const TRANSPORT_FINGERPRINT_PREFIX: &str = "transport:v1:";
/// Prefix for the deterministic catalog content digest.
pub const RESOURCE_CATALOG_CONTENT_DIGEST_PREFIX: &str = "catalog:v1:";
/// Prefix for a detached HMAC-SHA256 publisher attestation.
pub const RESOURCE_PUBLISHER_ATTESTATION_PREFIX: &str = "publisher-attestation:v1:";

/// Return the retained publisher-attestation topic for one validated publisher.
#[must_use]
pub fn resource_publisher_attestation_topic(publisher: &str) -> String {
    format!("{RESOURCE_PUBLISHER_ATTESTATION_TOPIC_PREFIX}/{publisher}")
}
const SECRET_SHAPE_MARKERS: &[&str] = &[
    "authorization:",
    "proxy-authorization:",
    "bearer ",
    "password=",
    "password:",
    "passwd=",
    "token=",
    "access_token=",
    "refresh_token=",
    "api_key=",
    "apikey=",
    "client_secret=",
    "private_key=",
    "-----begin private key-----",
    "-----begin rsa private key-----",
    "-----begin openssh private key-----",
    "\"password\":",
    "\"token\":",
];

/// A validation failure at the untrusted catalog boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceValidationError {
    /// A nested value uses a schema this consumer does not implement.
    UnsupportedSchema {
        /// Contract component containing the unsupported discriminator.
        component: &'static str,
        /// Version found on the wire.
        found: u16,
    },
    /// A required value is blank, malformed, or outside its typed grammar.
    InvalidField(&'static str),
    /// A bounded string exceeds its wire limit.
    FieldTooLong(&'static str),
    /// A bounded collection contains too many entries.
    CapacityExceeded {
        /// Collection that exceeded its bound.
        field: &'static str,
        /// Maximum entries admitted by v1.
        max: usize,
    },
    /// A set-like collection contains a repeated value.
    Duplicate(&'static str),
    /// A supplied stable identifier does not match its canonical fields.
    FingerprintMismatch(&'static str),
    /// A timestamp is zero, reversed, or inconsistent with its parent record.
    InvalidTimestamp(&'static str),
    /// A freshness interval is outside the bounded v1 range.
    InvalidTtl(&'static str),
    /// A state that requires a diagnostic failure reason omitted it.
    MissingFailure(&'static str),
    /// A healthy/ready state carried a contradictory failure reason.
    UnexpectedFailure(&'static str),
    /// Diagnostic text resembles plaintext credential or key material.
    SecretShapedValue(&'static str),
    /// Individually valid values form an invalid cross-field relationship.
    InvalidRelationship(&'static str),
}

impl fmt::Display for ResourceValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema { component, found } => {
                write!(f, "unsupported {component} schema version {found}")
            }
            Self::InvalidField(field) => write!(f, "invalid resource field: {field}"),
            Self::FieldTooLong(field) => write!(f, "resource field is too long: {field}"),
            Self::CapacityExceeded { field, max } => {
                write!(f, "resource collection {field} exceeds {max} entries")
            }
            Self::Duplicate(field) => write!(f, "duplicate resource value: {field}"),
            Self::FingerprintMismatch(field) => {
                write!(f, "resource fingerprint does not match: {field}")
            }
            Self::InvalidTimestamp(field) => write!(f, "invalid resource timestamp: {field}"),
            Self::InvalidTtl(field) => write!(f, "invalid resource TTL: {field}"),
            Self::MissingFailure(field) => write!(f, "missing resource failure reason: {field}"),
            Self::UnexpectedFailure(field) => {
                write!(f, "unexpected resource failure reason: {field}")
            }
            Self::SecretShapedValue(field) => {
                write!(f, "resource field resembles secret material: {field}")
            }
            Self::InvalidRelationship(field) => {
                write!(f, "invalid resource relationship: {field}")
            }
        }
    }
}

impl std::error::Error for ResourceValidationError {}

/// Failure returned while decoding and semantically admitting a catalog body.
#[derive(Debug)]
pub enum ResourceCatalogDecodeError {
    /// The body exceeded [`MAX_RESOURCE_CATALOG_BYTES`] before serde allocation.
    BodyTooLarge {
        /// Number of bytes supplied by the caller.
        bytes: usize,
        /// Maximum bytes admitted by this contract version.
        max: usize,
    },
    /// JSON was malformed or violated strict serde field/variant behavior.
    Json(serde_json::Error),
    /// JSON decoded, but the resulting contract was semantically invalid.
    Validation(ResourceValidationError),
}

impl fmt::Display for ResourceCatalogDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BodyTooLarge { bytes, max } => {
                write!(
                    f,
                    "resource catalog body is {bytes} bytes; maximum is {max}"
                )
            }
            Self::Json(error) => write!(f, "invalid resource catalog JSON: {error}"),
            Self::Validation(error) => write!(f, "invalid resource catalog: {error}"),
        }
    }
}

impl std::error::Error for ResourceCatalogDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::Validation(error) => Some(error),
            Self::BodyTooLarge { .. } => None,
        }
    }
}

/// Broad resource class used for grouping without implying launch support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceClass {
    /// A physical or virtual mesh/LAN node.
    Node,
    /// A full remote desktop or roaming desktop session.
    Desktop,
    /// A guest-owned application surface.
    Application,
    /// A virtual-machine workload.
    VirtualMachine,
    /// A container workload.
    Container,
    /// A media server or player service.
    MediaServer,
    /// A browsable file share.
    FileShare,
    /// A router, radio, display, or other network-visible device.
    NetworkDevice,
    /// A typed cloud workload or provider resource.
    CloudWorkload,
    /// Another typed service with no more-specific v1 class.
    Service,
}

/// Construct's supported relationship to a cataloged resource.
///
/// These roles are deliberately orthogonal: one service may be consumed by
/// Construct, provisioned/loaded by Construct, hosted by Construct, or support
/// all three. Cards advertise the roles instead of the UI guessing from a
/// provider or protocol name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceOperatingRole {
    /// Construct can consume or connect to the resource.
    Client,
    /// Construct can provision, launch, or load the resource.
    Loader,
    /// Construct can host and supervise the resource locally or on a mesh node.
    Host,
}

/// First-class service-card grouping. The service identity remains an open
/// registry key; this bounded category only drives filters and visual grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceCategory {
    /// Messaging, voice, conferencing, and related communication services.
    Communications,
    /// Routing, naming, VPN, and other network services.
    Network,
    /// Audio, video, library, and playback services.
    Media,
    /// Shared editing, synchronization, and teamwork services.
    Collaboration,
    /// File storage, sharing, and transfer services.
    Files,
    /// Platform control-plane and operational infrastructure services.
    Infrastructure,
    /// Services hosted outside the managed mesh boundary.
    External,
    /// A typed service that does not fit another v1 category.
    Other,
}

/// Architectural tier occupied by a service in the Local Service Stack hero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStackTier {
    /// User-facing clients and desktop shell integrations.
    DesktopShell,
    /// Typed adapters, workers, brokers, and application services.
    PlatformServices,
    /// Nebula, Bus, replicated storage, identity, and secret substrate.
    MeshSubstrate,
}

/// Functional plane used to place a service within its architectural tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStackPlane {
    /// User/client presentation and interaction.
    Experience,
    /// Discovery, cataloging, and service configuration.
    Control,
    /// Messaging, event, and state distribution.
    Coordination,
    /// Streams, calls, file bytes, and other payload transport.
    Data,
    /// Credentials, identity, policy, and trust enforcement.
    Trust,
}

/// Detailed, live placement used by the expandable Local Service Stack graphic.
/// It contains references and topology only; credential values never ride it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalServiceStack {
    /// Architectural tier containing the service's primary integration point.
    pub tier: ServiceStackTier,
    /// Functional plane containing the service's primary runtime path.
    pub plane: ServiceStackPlane,
    /// Whether the actual service is hosted beyond the Construct boundary.
    pub external: bool,
    /// Local typed adapter/worker that connects the service to Construct.
    pub adapter_worker: Option<String>,
    /// Bus topics consumed or published by the adapter.
    #[serde(default)]
    pub bus_topics: Vec<String>,
    /// Human-readable, non-secret typed transport summary.
    pub transport: Option<String>,
    /// Opaque sealed-credential reference, never the credential value.
    pub credential_ref: Option<String>,
    /// All current mesh hosting nodes; the UI renders these in one peer view.
    #[serde(default)]
    pub hosting_nodes: Vec<String>,
    /// Resource IDs of upstream dependencies; selecting one navigates to it.
    #[serde(default)]
    pub dependencies: Vec<String>,
}

impl LocalServiceStack {
    fn validate(&self) -> Result<(), ResourceValidationError> {
        if let Some(worker) = &self.adapter_worker {
            validate_identifier("service.stack.adapter_worker", worker)?;
        }
        if self.external && self.adapter_worker.is_none() {
            return Err(ResourceValidationError::InvalidRelationship(
                "service.stack.external_adapter",
            ));
        }
        validate_capacity(
            "service.stack.bus_topics",
            self.bus_topics.len(),
            MAX_SERVICE_BUS_TOPICS,
        )?;
        let mut topics = BTreeSet::new();
        for topic in &self.bus_topics {
            validate_text("service.stack.bus_topic", topic, MAX_IDENTIFIER_BYTES)?;
            if topic.chars().any(char::is_whitespace) || !topics.insert(topic) {
                return Err(ResourceValidationError::InvalidField(
                    "service.stack.bus_topic",
                ));
            }
        }
        if let Some(transport) = &self.transport {
            validate_text("service.stack.transport", transport, MAX_DISPLAY_NAME_BYTES)?;
            if looks_like_secret(transport) {
                return Err(ResourceValidationError::SecretShapedValue(
                    "service.stack.transport",
                ));
            }
        }
        if let Some(reference) = &self.credential_ref {
            validate_secret_reference(reference)?;
        }
        validate_unique(
            "service.stack.hosting_nodes",
            &self.hosting_nodes,
            MAX_SERVICE_HOSTING_NODES,
        )?;
        for node in &self.hosting_nodes {
            validate_identifier("service.stack.hosting_node", node)?;
        }
        validate_unique(
            "service.stack.dependencies",
            &self.dependencies,
            MAX_SERVICE_DEPENDENCIES,
        )?;
        for dependency in &self.dependencies {
            validate_fingerprint_field("service.stack.dependency", dependency, RESOURCE_ID_PREFIX)?;
        }
        Ok(())
    }
}

/// Shared lifecycle language rendered by every service card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceLifecycleStatus {
    /// Required configuration has not been supplied.
    Unconfigured,
    /// The adapter is establishing or validating a connection.
    Connecting,
    /// The service is configured and operating normally.
    Healthy,
    /// The service is usable but reports a bounded fault or reduced capability.
    Degraded,
    /// The configured service cannot currently be reached.
    Offline,
    /// The service is intentionally disabled by policy or operator choice.
    Disabled,
}

/// Generic, non-secret field type for a registry-provided configuration form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceConfigurationFieldKind {
    /// Bounded non-secret text.
    Text,
    /// A secret whose value is sealed outside the resource catalog.
    Secret,
    /// A structured service endpoint.
    Endpoint,
    /// A true/false setting.
    Boolean,
    /// One value from a bounded declared choice set.
    Choice,
}

/// One field in a service adapter's configuration schema. Values never ride
/// the resource catalog: secret values are sealed by the action responder and
/// non-secret values are submitted through typed service actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceConfigurationField {
    /// Stable adapter-owned field identity.
    pub key: String,
    /// User-facing field label.
    pub label: String,
    /// Typed input control and validation class.
    pub kind: ServiceConfigurationFieldKind,
    /// Whether configuration is incomplete without this field.
    pub required: bool,
    /// Admitted values for a [`ServiceConfigurationFieldKind::Choice`] field.
    #[serde(default)]
    pub choices: Vec<String>,
}

impl ServiceConfigurationField {
    fn validate(&self) -> Result<(), ResourceValidationError> {
        validate_identifier("service.configuration.key", &self.key)?;
        validate_text(
            "service.configuration.label",
            &self.label,
            MAX_DISPLAY_NAME_BYTES,
        )?;
        validate_capacity(
            "service.configuration.choices",
            self.choices.len(),
            MAX_CONFIGURATION_CHOICES,
        )?;
        if self.kind == ServiceConfigurationFieldKind::Choice && self.choices.is_empty() {
            return Err(ResourceValidationError::InvalidRelationship(
                "service.configuration.choice_values",
            ));
        }
        if self.kind != ServiceConfigurationFieldKind::Choice && !self.choices.is_empty() {
            return Err(ResourceValidationError::InvalidRelationship(
                "service.configuration.non_choice_values",
            ));
        }
        let mut choices = BTreeSet::new();
        for choice in &self.choices {
            validate_text("service.configuration.choice", choice, MAX_IDENTIFIER_BYTES)?;
            if !choices.insert(choice) {
                return Err(ResourceValidationError::Duplicate(
                    "service.configuration.choices",
                ));
            }
        }
        Ok(())
    }
}

/// Registry-owned service interface projected onto one resource card.
/// `service_kind` is open and adapter-defined, so a future service receives the
/// same card without a shell code change. `provider_id` is attribution only;
/// cards remain one-per-service even when a provider supplies several services.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceInterface {
    /// Stable open-registry key for the service adapter.
    pub service_kind: String,
    /// Optional provider attribution; never used as the service identity.
    pub provider_id: Option<String>,
    /// Product grouping used by Workloads filters.
    pub category: ServiceCategory,
    /// Current adapter lifecycle state.
    pub lifecycle: ServiceLifecycleStatus,
    /// Bounded configuration schema; values never appear in this catalog.
    #[serde(default)]
    pub configuration_fields: Vec<ServiceConfigurationField>,
    /// Detailed placement rendered by the expandable Local Service Stack hero.
    pub stack: LocalServiceStack,
}

impl ServiceInterface {
    fn validate(&self) -> Result<(), ResourceValidationError> {
        validate_identifier("service.service_kind", &self.service_kind)?;
        if let Some(provider_id) = &self.provider_id {
            validate_identifier("service.provider_id", provider_id)?;
        }
        validate_capacity(
            "service.configuration_fields",
            self.configuration_fields.len(),
            MAX_CONFIGURATION_FIELDS,
        )?;
        let mut keys = BTreeSet::new();
        for field in &self.configuration_fields {
            field.validate()?;
            if !keys.insert(field.key.as_str()) {
                return Err(ResourceValidationError::Duplicate(
                    "service.configuration_fields",
                ));
            }
        }
        self.stack.validate()?;
        Ok(())
    }
}

impl ResourceClass {
    const fn token(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Desktop => "desktop",
            Self::Application => "application",
            Self::VirtualMachine => "virtual_machine",
            Self::Container => "container",
            Self::MediaServer => "media_server",
            Self::FileShare => "file_share",
            Self::NetworkDevice => "network_device",
            Self::CloudWorkload => "cloud_workload",
            Self::Service => "service",
        }
    }
}

/// Namespace that owns a canonical resource key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityAuthority {
    /// Stable identity assigned by the enrolled mesh.
    Mesh,
    /// Stable identity assigned by this local node.
    Local,
    /// DNS/DNS-SD identity with a normalized canonical key.
    Dns,
    /// Device-issued UUID or equivalent immutable identifier.
    Device,
    /// Identity assigned by a configured gateway registry.
    Gateway,
    /// Identity assigned by a typed provider adapter.
    Provider,
    /// Identity explicitly assigned by an operator/manual source.
    Operator,
}

impl IdentityAuthority {
    const fn token(self) -> &'static str {
        match self {
            Self::Mesh => "mesh",
            Self::Local => "local",
            Self::Dns => "dns",
            Self::Device => "device",
            Self::Gateway => "gateway",
            Self::Provider => "provider",
            Self::Operator => "operator",
        }
    }
}

/// Kind of alternate identity claim used while discovery lanes converge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceAliasKind {
    /// Enrolled mesh node identifier.
    MeshNode,
    /// Normalized DNS hostname.
    DnsName,
    /// Device-issued UUID.
    DeviceUuid,
    /// DNS-SD or provider service-instance identifier.
    ServiceInstance,
    /// Provider-native identifier.
    ProviderId,
    /// Identifier emitted by a compatibility desktop/media projection.
    LegacyId,
}

/// A typed alternate identity claim; aliases never perturb the stable ID.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceAlias {
    /// Alias namespace.
    pub kind: ResourceAliasKind,
    /// Bounded normalized value in that namespace.
    pub value: String,
}

impl ResourceAlias {
    fn validate(&self) -> Result<(), ResourceValidationError> {
        validate_text("identity.alias.value", &self.value, MAX_IDENTIFIER_BYTES)?;
        if looks_like_secret(&self.value) {
            return Err(ResourceValidationError::SecretShapedValue(
                "identity.alias.value",
            ));
        }
        Ok(())
    }
}

/// Stable, deduplicable identity shared by every discovery lane.
///
/// `resource_id` is `resource:v1:<sha256>` over `(schema, class, authority,
/// canonical_key)`. Aliases are deliberately excluded so learning or losing an
/// endpoint-specific alias does not replace the card or its session context.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceIdentity {
    /// Resource-contract schema discriminator.
    pub schema_version: u16,
    /// Derived stable ID used as the catalog deduplication key.
    pub resource_id: String,
    /// Broad resource class.
    pub class: ResourceClass,
    /// Namespace that owns `canonical_key`.
    pub authority: IdentityAuthority,
    /// Stable normalized key within `authority`.
    pub canonical_key: String,
    /// Alternate claims used to merge observations from other lanes.
    pub aliases: Vec<ResourceAlias>,
}

impl ResourceIdentity {
    /// Build and validate a v1 identity, deriving its stable resource ID.
    ///
    /// # Errors
    ///
    /// Returns an error when the canonical key or aliases violate their bounds
    /// or typed grammar.
    pub fn new(
        class: ResourceClass,
        authority: IdentityAuthority,
        canonical_key: impl Into<String>,
        aliases: Vec<ResourceAlias>,
    ) -> Result<Self, ResourceValidationError> {
        let mut identity = Self {
            schema_version: RESOURCE_CONTRACT_VERSION,
            resource_id: String::new(),
            class,
            authority,
            canonical_key: canonical_key.into(),
            aliases,
        };
        identity.validate_shape()?;
        identity.resource_id = identity.computed_resource_id();
        Ok(identity)
    }

    /// Recompute the stable, source-independent resource ID.
    #[must_use]
    pub fn computed_resource_id(&self) -> String {
        let mut canonical = String::new();
        push_canonical(&mut canonical, "resource-identity");
        push_canonical(&mut canonical, &self.schema_version.to_string());
        push_canonical(&mut canonical, self.class.token());
        push_canonical(&mut canonical, self.authority.token());
        push_canonical(&mut canonical, &self.canonical_key);
        format!("{RESOURCE_ID_PREFIX}{}", sha256_hex(&canonical))
    }

    /// Validate the identity and verify its supplied stable ID.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsupported version, malformed field, duplicate
    /// alias, or a stable ID that does not match the canonical identity fields.
    pub fn validate(&self) -> Result<(), ResourceValidationError> {
        self.validate_shape()?;
        if !valid_prefixed_fingerprint(&self.resource_id, RESOURCE_ID_PREFIX)
            || self.resource_id != self.computed_resource_id()
        {
            return Err(ResourceValidationError::FingerprintMismatch(
                "identity.resource_id",
            ));
        }
        Ok(())
    }

    fn validate_shape(&self) -> Result<(), ResourceValidationError> {
        validate_version("resource_identity", self.schema_version)?;
        validate_identifier("identity.canonical_key", &self.canonical_key)?;
        validate_capacity("identity.aliases", self.aliases.len(), MAX_ALIASES)?;
        let mut aliases = BTreeSet::new();
        for alias in &self.aliases {
            alias.validate()?;
            if !aliases.insert(alias) {
                return Err(ResourceValidationError::Duplicate("identity.aliases"));
            }
        }
        Ok(())
    }
}

/// Typed, non-secret diagnostic explaining degraded or unavailable state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureReason {
    /// Machine-readable failure class.
    pub code: FailureCode,
    /// Bounded operator-facing explanation with secret-shaped values rejected.
    pub message: String,
}

impl FailureReason {
    fn validate(&self, field: &'static str) -> Result<(), ResourceValidationError> {
        validate_text(field, &self.message, MAX_FAILURE_MESSAGE_BYTES)?;
        if looks_like_secret(&self.message) {
            return Err(ResourceValidationError::SecretShapedValue(field));
        }
        Ok(())
    }
}

/// Closed reason codes for unavailable evidence and action gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    /// No successful observation exists yet.
    NotObserved,
    /// A bounded operation timed out.
    Timeout,
    /// The endpoint could not be reached.
    Unreachable,
    /// The observation exceeded its freshness lifetime.
    Stale,
    /// Authentication is required before the operation can proceed.
    AuthenticationRequired,
    /// Authentication failed without exposing credential material.
    AuthenticationFailed,
    /// Local or remote approval is required.
    ApprovalRequired,
    /// Policy denies this action or transport.
    PolicyDenied,
    /// No registered native client or approved adapter matches.
    MissingClient,
    /// Required device/hardware support is absent.
    MissingHardware,
    /// Required workload/provider support is absent.
    MissingProvider,
    /// The active seat cannot supply the required display path.
    MissingDisplay,
    /// The required codec/decode path is absent.
    MissingCodec,
    /// The approved secret-store reference does not resolve.
    CredentialsUnavailable,
    /// Pairing or approval expired.
    PairingExpired,
    /// Authorization or trust was revoked.
    Revoked,
    /// Advertisement failed typed admission.
    MalformedAdvertisement,
    /// Advertisement names an unsupported protocol.
    UnsupportedProtocol,
    /// A bounded diagnostic does not fit a more specific v1 code.
    Other,
}

/// Coarse health classification for a resource or transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// No reliable reachability evidence exists.
    Unknown,
    /// Current typed probe/roster evidence says the resource is usable.
    Available,
    /// The resource is usable with a known impairment.
    Degraded,
    /// Current evidence says the resource is unusable.
    Unavailable,
    /// Last known evidence is retained but outside freshness bounds.
    Stale,
}

/// Versioned health observation with explicit freshness and failure context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthState {
    /// Resource-contract schema discriminator.
    pub schema_version: u16,
    /// Coarse status.
    pub status: HealthStatus,
    /// Unix epoch milliseconds when this state was observed.
    pub observed_at_ms: u64,
    /// Unix epoch milliseconds when this observation ceases to be fresh.
    pub expires_at_ms: u64,
    /// Last bounded probe latency, when measured.
    pub latency_ms: Option<u32>,
    /// Required for degraded/unavailable/stale states; forbidden for available.
    pub failure: Option<FailureReason>,
}

impl HealthState {
    /// Validate version, TTL, latency, and status/reason consistency.
    ///
    /// # Errors
    ///
    /// Returns an error when freshness is unbounded, latency is invalid, or the
    /// health status contradicts its safe failure reason.
    pub fn validate(&self) -> Result<(), ResourceValidationError> {
        validate_version("health_state", self.schema_version)?;
        validate_freshness("health_state", self.observed_at_ms, self.expires_at_ms)?;
        if self.latency_ms.is_some_and(|latency| latency > 120_000) {
            return Err(ResourceValidationError::InvalidField("health.latency_ms"));
        }
        if let Some(failure) = &self.failure {
            failure.validate("health.failure.message")?;
        }
        match self.status {
            HealthStatus::Available if self.failure.is_some() => {
                Err(ResourceValidationError::UnexpectedFailure("health"))
            }
            HealthStatus::Degraded | HealthStatus::Unavailable | HealthStatus::Stale
                if self.failure.is_none() =>
            {
                Err(ResourceValidationError::MissingFailure("health"))
            }
            _ => Ok(()),
        }
    }
}

/// Discovery lane represented by one provenance attestation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySource {
    /// Local service/session enumeration.
    Local,
    /// Authenticated replicated mesh peer directory.
    MeshDirectory,
    /// Trusted-LAN mDNS/DNS-SD observation.
    MdnsDnsSd,
    /// Trusted-LAN SSDP/UPnP observation.
    SsdpUpnp,
    /// Configured gateway registry.
    GatewayRegistry,
    /// Typed cloud or workload provider registry.
    ProviderRegistry,
    /// Typed operator-authored source.
    Manual,
}

/// Reachability boundary for a source or transport; public exposure is absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceScope {
    /// Loopback/local machine only.
    Local,
    /// Authenticated Nebula mesh.
    Mesh,
    /// Explicitly trusted local network interface.
    TrustedLan,
    /// Indirectly reached through a configured typed gateway.
    Gateway,
}

impl ResourceScope {
    const fn token(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Mesh => "mesh",
            Self::TrustedLan => "trusted_lan",
            Self::Gateway => "gateway",
        }
    }
}

/// Trust basis attached to one discovery attestation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceTrust {
    /// Local service reports its own identity.
    SelfReported,
    /// Identity arrived over authenticated mesh state.
    AuthenticatedMesh,
    /// Identity was observed on an explicitly trusted LAN interface.
    ObservedLan,
    /// An authorized operator supplied the typed record.
    OperatorDeclared,
}

/// Versioned source attestation with bounded freshness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProvenance {
    /// Resource-contract schema discriminator.
    pub schema_version: u16,
    /// Discovery lane.
    pub source: DiscoverySource,
    /// Stable producer/lane-local identifier, never a credential or URL.
    pub source_id: String,
    /// Reachability boundary in which the source was admitted.
    pub scope: ResourceScope,
    /// Trust basis for this observation.
    pub trust: ProvenanceTrust,
    /// Interface name for interface-scoped LAN/mesh evidence, when applicable.
    pub interface: Option<String>,
    /// Unix epoch milliseconds when observed.
    pub observed_at_ms: u64,
    /// Unix epoch milliseconds when this attestation expires.
    pub expires_at_ms: u64,
}

impl SourceProvenance {
    /// Validate source grammar, freshness, and trust/scope consistency.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed IDs, unbounded freshness, or a discovery
    /// source whose trust and reachability scope do not match.
    pub fn validate(&self) -> Result<(), ResourceValidationError> {
        validate_version("source_provenance", self.schema_version)?;
        validate_identifier("provenance.source_id", &self.source_id)?;
        if let Some(interface) = &self.interface {
            validate_identifier_with_limit("provenance.interface", interface, 64)?;
        }
        validate_freshness("source_provenance", self.observed_at_ms, self.expires_at_ms)?;
        let consistent = match self.source {
            DiscoverySource::Local => {
                self.scope == ResourceScope::Local
                    && matches!(
                        self.trust,
                        ProvenanceTrust::SelfReported | ProvenanceTrust::OperatorDeclared
                    )
            }
            DiscoverySource::MeshDirectory => {
                self.scope == ResourceScope::Mesh
                    && self.trust == ProvenanceTrust::AuthenticatedMesh
            }
            DiscoverySource::MdnsDnsSd | DiscoverySource::SsdpUpnp => {
                self.scope == ResourceScope::TrustedLan
                    && self.trust == ProvenanceTrust::ObservedLan
                    && self.interface.is_some()
            }
            DiscoverySource::GatewayRegistry => {
                self.scope == ResourceScope::Gateway
                    && matches!(
                        self.trust,
                        ProvenanceTrust::AuthenticatedMesh | ProvenanceTrust::OperatorDeclared
                    )
            }
            DiscoverySource::ProviderRegistry => {
                matches!(self.scope, ResourceScope::Mesh | ResourceScope::Gateway)
                    && matches!(
                        self.trust,
                        ProvenanceTrust::AuthenticatedMesh | ProvenanceTrust::OperatorDeclared
                    )
            }
            DiscoverySource::Manual => self.trust == ProvenanceTrust::OperatorDeclared,
        };
        if !consistent {
            return Err(ResourceValidationError::InvalidRelationship(
                "provenance.source_scope_trust",
            ));
        }
        Ok(())
    }
}

/// Typed transport protocol. Unknown advertisements cannot deserialize as a
/// launchable transport and may instead be represented by unavailable evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportProtocol {
    /// Microsoft Remote Desktop Protocol.
    Rdp,
    /// Remote Framebuffer/VNC.
    Vnc,
    /// SPICE desktop transport.
    Spice,
    /// Sunshine/GameStream transport consumed by Moonlight.
    Moonlight,
    /// Typed SSH terminal/session transport.
    Ssh,
    /// One SSH-forwarded X11 application.
    SshX11Application,
    /// Explicit full remote X11 display/session endpoint.
    X11Desktop,
    /// Jellyfin API/media transport.
    Jellyfin,
    /// OpenSubsonic-compatible API/media transport.
    OpenSubsonic,
    /// DLNA/UPnP browse/control transport.
    DlnaUpnp,
    /// Music Player Daemon protocol.
    Mpd,
    /// SMB/CIFS file-share transport.
    Smb,
    /// NFS file-share transport.
    Nfs,
    /// `WebDAV` file-share transport.
    WebDav,
}

impl TransportProtocol {
    /// Stable protocol token used by fingerprints and diagnostics.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Rdp => "rdp",
            Self::Vnc => "vnc",
            Self::Spice => "spice",
            Self::Moonlight => "moonlight",
            Self::Ssh => "ssh",
            Self::SshX11Application => "ssh_x11_application",
            Self::X11Desktop => "x11_desktop",
            Self::Jellyfin => "jellyfin",
            Self::OpenSubsonic => "open_subsonic",
            Self::DlnaUpnp => "dlna_upnp",
            Self::Mpd => "mpd",
            Self::Smb => "smb",
            Self::Nfs => "nfs",
            Self::WebDav => "web_dav",
        }
    }
}

/// Structured, non-executable endpoint admitted by a transport adapter.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TransportEndpoint {
    /// Network host/port and an optional query-free API base path.
    Network {
        /// DNS name or IP literal without user information.
        host: String,
        /// Non-zero protocol port.
        port: u16,
        /// Optional absolute path without query, fragment, or traversal.
        base_path: Option<String>,
    },
    /// Explicit remote X11 display endpoint.
    ///
    /// The display and screen are numeric so a catalog consumer never has to
    /// parse or execute an arbitrary `DISPLAY` string. SSH-forwarded X11
    /// applications use [`Self::Network`] because the server allocates their
    /// forwarded display at session creation time.
    X11 {
        /// DNS name or IP literal without user information.
        host: String,
        /// SSH or X11 transport port.
        port: u16,
        /// Numeric X11 display number.
        display: u16,
        /// Numeric X11 screen number.
        screen: u8,
    },
    /// Named local platform service; never a filesystem/socket path.
    LocalService {
        /// Typed service registry identity.
        service_id: String,
    },
    /// Opaque typed target reached through another cataloged gateway resource.
    Gateway {
        /// Stable ID of the gateway resource card.
        gateway_resource_id: String,
        /// Gateway-registry target identity, never a URL or command.
        target_id: String,
    },
}

impl TransportEndpoint {
    fn validate(&self) -> Result<(), ResourceValidationError> {
        match self {
            Self::Network {
                host,
                port,
                base_path,
            } => {
                validate_host(host)?;
                if *port == 0 {
                    return Err(ResourceValidationError::InvalidField("endpoint.port"));
                }
                if let Some(path) = base_path {
                    validate_endpoint_path(path)?;
                }
            }
            Self::X11 {
                host,
                port,
                display,
                screen,
            } => {
                validate_host(host)?;
                if *port == 0 {
                    return Err(ResourceValidationError::InvalidField("endpoint.port"));
                }
                if *display > 255 {
                    return Err(ResourceValidationError::InvalidField(
                        "endpoint.x11.display",
                    ));
                }
                if *screen > 31 {
                    return Err(ResourceValidationError::InvalidField("endpoint.x11.screen"));
                }
            }
            Self::LocalService { service_id } => {
                validate_identifier("endpoint.service_id", service_id)?;
            }
            Self::Gateway {
                gateway_resource_id,
                target_id,
            } => {
                if !valid_prefixed_fingerprint(gateway_resource_id, RESOURCE_ID_PREFIX) {
                    return Err(ResourceValidationError::InvalidField(
                        "endpoint.gateway_resource_id",
                    ));
                }
                validate_identifier("endpoint.target_id", target_id)?;
            }
        }
        Ok(())
    }

    fn append_canonical(&self, canonical: &mut String) {
        match self {
            Self::Network {
                host,
                port,
                base_path,
            } => {
                push_canonical(canonical, "network");
                push_canonical(canonical, host);
                push_canonical(canonical, &port.to_string());
                push_optional_canonical(canonical, base_path.as_deref());
            }
            Self::X11 {
                host,
                port,
                display,
                screen,
            } => {
                push_canonical(canonical, "x11");
                push_canonical(canonical, host);
                push_canonical(canonical, &port.to_string());
                push_canonical(canonical, &display.to_string());
                push_canonical(canonical, &screen.to_string());
            }
            Self::LocalService { service_id } => {
                push_canonical(canonical, "local_service");
                push_canonical(canonical, service_id);
            }
            Self::Gateway {
                gateway_resource_id,
                target_id,
            } => {
                push_canonical(canonical, "gateway");
                push_canonical(canonical, gateway_resource_id);
                push_canonical(canonical, target_id);
            }
        }
    }
}

/// Versioned endpoint observation linked to an optional admitted client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransportCandidate {
    /// Resource-contract schema discriminator.
    pub schema_version: u16,
    /// Derived endpoint fingerprint used to deduplicate transport observations.
    pub fingerprint: String,
    /// Typed wire protocol.
    pub protocol: TransportProtocol,
    /// Structured non-executable endpoint.
    pub endpoint: TransportEndpoint,
    /// Boundary over which the endpoint may be reached.
    pub scope: ResourceScope,
    /// Lower values are preferred; bounded to avoid arithmetic/sorting abuse.
    pub priority: u16,
    /// Unix epoch milliseconds when this endpoint was last seen.
    pub last_seen_at_ms: u64,
    /// Unix epoch milliseconds when this endpoint observation expires.
    pub expires_at_ms: u64,
    /// Current endpoint health.
    pub health: HealthState,
    /// Matching registered client fingerprint, absent for unsupported evidence.
    pub client_capability_fingerprint: Option<String>,
}

impl TransportCandidate {
    /// Construct and fingerprint a validated v1 transport observation.
    ///
    /// # Errors
    ///
    /// Returns an error for a malformed endpoint, invalid scope, unbounded
    /// freshness, bad health state, or malformed capability reference.
    #[allow(
        clippy::too_many_arguments,
        reason = "wire constructor mirrors bounded fields"
    )]
    pub fn new(
        protocol: TransportProtocol,
        endpoint: TransportEndpoint,
        scope: ResourceScope,
        priority: u16,
        last_seen_at_ms: u64,
        expires_at_ms: u64,
        health: HealthState,
        client_capability_fingerprint: Option<String>,
    ) -> Result<Self, ResourceValidationError> {
        let mut candidate = Self {
            schema_version: RESOURCE_CONTRACT_VERSION,
            fingerprint: String::new(),
            protocol,
            endpoint,
            scope,
            priority,
            last_seen_at_ms,
            expires_at_ms,
            health,
            client_capability_fingerprint,
        };
        candidate.validate_shape()?;
        candidate.fingerprint = candidate.computed_fingerprint();
        Ok(candidate)
    }

    /// Fingerprint only stable endpoint identity, excluding health/freshness and
    /// the currently selected client so those changes update one candidate.
    #[must_use]
    pub fn computed_fingerprint(&self) -> String {
        let mut canonical = String::new();
        push_canonical(&mut canonical, "resource-transport");
        push_canonical(&mut canonical, &self.schema_version.to_string());
        push_canonical(&mut canonical, self.protocol.token());
        push_canonical(&mut canonical, self.scope.token());
        self.endpoint.append_canonical(&mut canonical);
        format!("{TRANSPORT_FINGERPRINT_PREFIX}{}", sha256_hex(&canonical))
    }

    /// Validate endpoint, freshness, health, and fingerprint integrity.
    ///
    /// # Errors
    ///
    /// Returns an error when any typed field is invalid or the supplied
    /// transport fingerprint does not match its stable endpoint fields.
    pub fn validate(&self) -> Result<(), ResourceValidationError> {
        self.validate_shape()?;
        if !valid_prefixed_fingerprint(&self.fingerprint, TRANSPORT_FINGERPRINT_PREFIX)
            || self.fingerprint != self.computed_fingerprint()
        {
            return Err(ResourceValidationError::FingerprintMismatch(
                "transport.fingerprint",
            ));
        }
        Ok(())
    }

    fn validate_shape(&self) -> Result<(), ResourceValidationError> {
        validate_version("transport_candidate", self.schema_version)?;
        self.endpoint.validate()?;
        if matches!(self.endpoint, TransportEndpoint::LocalService { .. })
            && self.scope != ResourceScope::Local
            || matches!(self.endpoint, TransportEndpoint::Gateway { .. })
                && self.scope != ResourceScope::Gateway
        {
            return Err(ResourceValidationError::InvalidRelationship(
                "transport.endpoint_scope",
            ));
        }
        if self.priority > 1_000 {
            return Err(ResourceValidationError::InvalidField("transport.priority"));
        }
        validate_freshness(
            "transport_candidate",
            self.last_seen_at_ms,
            self.expires_at_ms,
        )?;
        self.health.validate()?;
        if self.health.observed_at_ms > self.last_seen_at_ms
            || self.health.expires_at_ms > self.expires_at_ms
        {
            return Err(ResourceValidationError::InvalidRelationship(
                "transport.health_freshness",
            ));
        }
        if let Some(fingerprint) = &self.client_capability_fingerprint {
            if !valid_prefixed_fingerprint(fingerprint, CAPABILITY_FINGERPRINT_PREFIX) {
                return Err(ResourceValidationError::InvalidField(
                    "transport.client_capability_fingerprint",
                ));
            }
        }
        Ok(())
    }
}

/// Execution/rendering boundary owned by a registered client capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientBoundary {
    /// Native in-shell client implementation or narrowly owned FFI core.
    ShellNative,
    /// Typed platform service/adapter; never an arbitrary process launcher.
    PlatformAdapter,
    /// Rendering/execution remains inside an admitted VM guest.
    Guest,
}

impl ClientBoundary {
    const fn token(self) -> &'static str {
        match self {
            Self::ShellNative => "shell_native",
            Self::PlatformAdapter => "platform_adapter",
            Self::Guest => "guest",
        }
    }
}

/// Authentication mechanism declared by a typed client adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    /// Existing authenticated mesh identity.
    MeshIdentity,
    /// Explicit local approval with no credential serialized in the catalog.
    LocalApproval,
    /// Ephemeral pairing flow; the code itself is never in this contract.
    PairingCode,
    /// Password resolved only through an opaque secret-store reference.
    Password,
    /// SSH key resolved only through an opaque secret-store reference.
    SshKey,
    /// API bearer token resolved only through an opaque secret-store reference.
    BearerToken,
    /// Client certificate resolved through the platform credential store.
    ClientCertificate,
}

impl AuthMethod {
    const fn token(self) -> &'static str {
        match self {
            Self::MeshIdentity => "mesh_identity",
            Self::LocalApproval => "local_approval",
            Self::PairingCode => "pairing_code",
            Self::Password => "password",
            Self::SshKey => "ssh_key",
            Self::BearerToken => "bearer_token",
            Self::ClientCertificate => "client_certificate",
        }
    }

    /// Whether successful authorization must name an opaque secret-store entry.
    #[must_use]
    pub const fn requires_secret_reference(self) -> bool {
        matches!(
            self,
            Self::Password | Self::SshKey | Self::BearerToken | Self::ClientCertificate
        )
    }
}

/// Client feature used for typed capability admission and limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientFeature {
    /// Display/video surface.
    Display,
    /// Audio playback from remote resource.
    AudioPlayback,
    /// Audio capture forwarded to remote resource.
    AudioCapture,
    /// Keyboard input.
    KeyboardInput,
    /// Pointer input.
    PointerInput,
    /// Touch input.
    TouchInput,
    /// Native UTF-8 text clipboard.
    ClipboardText,
    /// Typed file browsing.
    FileBrowse,
    /// Typed media browsing/playback.
    MediaBrowse,
    /// Pairing lifecycle.
    Pairing,
    /// Session reconnect/resume.
    Reconnect,
    /// SSH X11 forwarding.
    X11Forwarding,
}

impl ClientFeature {
    const fn token(self) -> &'static str {
        match self {
            Self::Display => "display",
            Self::AudioPlayback => "audio_playback",
            Self::AudioCapture => "audio_capture",
            Self::KeyboardInput => "keyboard_input",
            Self::PointerInput => "pointer_input",
            Self::TouchInput => "touch_input",
            Self::ClipboardText => "clipboard_text",
            Self::FileBrowse => "file_browse",
            Self::MediaBrowse => "media_browse",
            Self::Pairing => "pairing",
            Self::Reconnect => "reconnect",
            Self::X11Forwarding => "x11_forwarding",
        }
    }
}

/// Closed action verbs supported by the universal resource surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceActionVerb {
    /// Show typed diagnostics and provenance.
    Inspect,
    /// Enter a protocol-specific pairing flow.
    Pair,
    /// Open through a registered capability and structured transport.
    Connect,
    /// Retry a known typed transport.
    Retry,
    /// Forget a retained identity/manual record.
    Forget,
    /// Request explicit local or remote approval.
    RequestApproval,
    /// Open the registry-provided configuration form.
    Configure,
    /// Test the configured endpoint and sealed credential reference.
    Test,
    /// Launch or load the service through its typed adapter.
    Launch,
    /// Start a stopped typed workload or provider-owned resource.
    Start,
    /// Resume an existing typed workload or session generation.
    Resume,
    /// Transfer through an already-admitted typed clipboard/file capability.
    Transfer,
    /// Enable a configured service.
    Enable,
    /// Disable a service without deleting its sealed configuration.
    Disable,
    /// Remove the service registration and its scoped configuration.
    Remove,
}

impl ResourceActionVerb {
    const fn token(self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::Pair => "pair",
            Self::Connect => "connect",
            Self::Retry => "retry",
            Self::Forget => "forget",
            Self::RequestApproval => "request_approval",
            Self::Configure => "configure",
            Self::Test => "test",
            Self::Launch => "launch",
            Self::Start => "start",
            Self::Resume => "resume",
            Self::Transfer => "transfer",
            Self::Enable => "enable",
            Self::Disable => "disable",
            Self::Remove => "remove",
        }
    }
}

/// Bounded client limits. Feature absence, not a zero sentinel, represents an
/// unsupported dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientCapabilityLimits {
    /// Maximum display width, paired with `max_height`.
    pub max_width: Option<u16>,
    /// Maximum display height, paired with `max_width`.
    pub max_height: Option<u16>,
    /// Maximum admitted frame rate.
    pub max_fps: Option<u16>,
    /// Maximum admitted audio channels.
    pub max_audio_channels: Option<u8>,
    /// Maximum simultaneous sessions owned by this adapter.
    pub max_parallel_sessions: u16,
}

impl ClientCapabilityLimits {
    fn validate(&self, features: &[ClientFeature]) -> Result<(), ResourceValidationError> {
        if !(1..=32).contains(&self.max_parallel_sessions) {
            return Err(ResourceValidationError::InvalidField(
                "capability.limits.max_parallel_sessions",
            ));
        }
        match (self.max_width, self.max_height) {
            (Some(width), Some(height))
                if (320..=16_384).contains(&width) && (200..=16_384).contains(&height) => {}
            (None, None) => {}
            _ => {
                return Err(ResourceValidationError::InvalidRelationship(
                    "capability.limits.display_dimensions",
                ));
            }
        }
        if self.max_fps.is_some_and(|fps| !(1..=240).contains(&fps)) {
            return Err(ResourceValidationError::InvalidField(
                "capability.limits.max_fps",
            ));
        }
        if self
            .max_audio_channels
            .is_some_and(|channels| !(1..=32).contains(&channels))
        {
            return Err(ResourceValidationError::InvalidField(
                "capability.limits.max_audio_channels",
            ));
        }
        let has_display = features.contains(&ClientFeature::Display);
        if !has_display
            && (self.max_width.is_some() || self.max_height.is_some() || self.max_fps.is_some())
        {
            return Err(ResourceValidationError::InvalidRelationship(
                "capability.limits.require_display_feature",
            ));
        }
        let has_audio = features.iter().any(|feature| {
            matches!(
                feature,
                ClientFeature::AudioPlayback | ClientFeature::AudioCapture
            )
        });
        if !has_audio && self.max_audio_channels.is_some() {
            return Err(ResourceValidationError::InvalidRelationship(
                "capability.limits.require_audio_feature",
            ));
        }
        Ok(())
    }

    fn append_canonical(&self, canonical: &mut String) {
        push_optional_number(canonical, self.max_width);
        push_optional_number(canonical, self.max_height);
        push_optional_number(canonical, self.max_fps);
        push_optional_number(canonical, self.max_audio_channels);
        push_canonical(canonical, &self.max_parallel_sessions.to_string());
    }
}

/// Versioned registered native client or approved platform adapter capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientCapability {
    /// Resource-contract schema discriminator.
    pub schema_version: u16,
    /// Derived fingerprint used for admission and transport/action references.
    pub fingerprint: String,
    /// Stable adapter registry identity, never a binary/path/command.
    pub adapter_id: String,
    /// Version of the adapter implementation contract.
    pub adapter_version: String,
    /// Protocol consumed by this adapter.
    pub protocol: TransportProtocol,
    /// Protocol version or bounded version range token.
    pub protocol_version: String,
    /// Host/service/guest execution boundary.
    pub boundary: ClientBoundary,
    /// Authentication methods understood by this adapter.
    pub auth_methods: Vec<AuthMethod>,
    /// Typed feature set.
    pub features: Vec<ClientFeature>,
    /// Bounded feature limits.
    pub limits: ClientCapabilityLimits,
    /// Closed action policy this adapter implements safely.
    pub safe_actions: Vec<ResourceActionVerb>,
}

impl ClientCapability {
    /// Construct and fingerprint a validated v1 client capability.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed adapter/version tokens, repeated sets,
    /// inconsistent limits, or an unsafe action policy.
    #[allow(
        clippy::too_many_arguments,
        reason = "wire constructor mirrors bounded fields"
    )]
    pub fn new(
        adapter_id: impl Into<String>,
        adapter_version: impl Into<String>,
        protocol: TransportProtocol,
        protocol_version: impl Into<String>,
        boundary: ClientBoundary,
        auth_methods: Vec<AuthMethod>,
        features: Vec<ClientFeature>,
        limits: ClientCapabilityLimits,
        safe_actions: Vec<ResourceActionVerb>,
    ) -> Result<Self, ResourceValidationError> {
        let mut capability = Self {
            schema_version: RESOURCE_CONTRACT_VERSION,
            fingerprint: String::new(),
            adapter_id: adapter_id.into(),
            adapter_version: adapter_version.into(),
            protocol,
            protocol_version: protocol_version.into(),
            boundary,
            auth_methods,
            features,
            limits,
            safe_actions,
        };
        capability.validate_shape()?;
        capability.fingerprint = capability.computed_fingerprint();
        Ok(capability)
    }

    /// Compute a stable capability fingerprint independent of set ordering.
    #[must_use]
    pub fn computed_fingerprint(&self) -> String {
        let mut canonical = String::new();
        push_canonical(&mut canonical, "resource-client-capability");
        push_canonical(&mut canonical, &self.schema_version.to_string());
        push_canonical(&mut canonical, &self.adapter_id);
        push_canonical(&mut canonical, &self.adapter_version);
        push_canonical(&mut canonical, self.protocol.token());
        push_canonical(&mut canonical, &self.protocol_version);
        push_canonical(&mut canonical, self.boundary.token());

        let mut auth_methods = self.auth_methods.clone();
        auth_methods.sort_unstable();
        for method in auth_methods {
            push_canonical(&mut canonical, method.token());
        }
        push_canonical(&mut canonical, "features");
        let mut features = self.features.clone();
        features.sort_unstable();
        for feature in features {
            push_canonical(&mut canonical, feature.token());
        }
        push_canonical(&mut canonical, "limits");
        self.limits.append_canonical(&mut canonical);
        push_canonical(&mut canonical, "safe-actions");
        let mut actions = self.safe_actions.clone();
        actions.sort_unstable();
        for action in actions {
            push_canonical(&mut canonical, action.token());
        }
        format!("{CAPABILITY_FINGERPRINT_PREFIX}{}", sha256_hex(&canonical))
    }

    /// Validate capability grammar, limits, set uniqueness, and fingerprint.
    ///
    /// # Errors
    ///
    /// Returns an error when the capability shape is invalid or its supplied
    /// fingerprint differs from the canonical semantic fingerprint.
    pub fn validate(&self) -> Result<(), ResourceValidationError> {
        self.validate_shape()?;
        if !valid_prefixed_fingerprint(&self.fingerprint, CAPABILITY_FINGERPRINT_PREFIX)
            || self.fingerprint != self.computed_fingerprint()
        {
            return Err(ResourceValidationError::FingerprintMismatch(
                "capability.fingerprint",
            ));
        }
        Ok(())
    }

    fn validate_shape(&self) -> Result<(), ResourceValidationError> {
        validate_version("client_capability", self.schema_version)?;
        validate_identifier("capability.adapter_id", &self.adapter_id)?;
        validate_identifier("capability.adapter_version", &self.adapter_version)?;
        validate_identifier("capability.protocol_version", &self.protocol_version)?;
        validate_unique(
            "capability.auth_methods",
            &self.auth_methods,
            MAX_AUTH_METHODS,
        )?;
        validate_unique("capability.features", &self.features, MAX_FEATURES)?;
        validate_unique(
            "capability.safe_actions",
            &self.safe_actions,
            MAX_SAFE_ACTIONS,
        )?;
        if !self.safe_actions.contains(&ResourceActionVerb::Connect) {
            return Err(ResourceValidationError::InvalidRelationship(
                "capability.safe_actions.require_connect",
            ));
        }
        self.limits.validate(&self.features)
    }
}

/// Deterministic, bounded admission index for typed client capabilities.
///
/// The index is intentionally an in-memory helper rather than a wire type:
/// callers continue to publish [`ClientCapability`] values exactly as before,
/// while transport and action consumers can resolve a referenced capability
/// fingerprint without accepting an arbitrary executable, path, or URL.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClientCapabilityRegistry {
    by_fingerprint: BTreeMap<String, ClientCapability>,
}

impl ClientCapabilityRegistry {
    /// Admit a bounded set of validated, uniquely fingerprinted capabilities.
    ///
    /// Entries are copied into a [`BTreeMap`], so iteration and fingerprint
    /// lookup are independent of discovery-worker arrival order. The registry
    /// does not implement `Serialize` or `Deserialize`; the existing strict
    /// capability contract remains the only wire representation.
    ///
    /// # Errors
    ///
    /// Returns the capability's validation error, a duplicate-fingerprint
    /// error, or a capacity error before more than the bounded number of
    /// entries is retained.
    pub fn admitted<I>(capabilities: I) -> Result<Self, ResourceValidationError>
    where
        I: IntoIterator<Item = ClientCapability>,
    {
        let mut by_fingerprint = BTreeMap::new();
        let mut count = 0;
        for capability in capabilities {
            count += 1;
            validate_capacity(
                "client_capability_registry.entries",
                count,
                MAX_CAPABILITIES,
            )?;
            capability.validate()?;
            if by_fingerprint.contains_key(&capability.fingerprint) {
                return Err(ResourceValidationError::Duplicate(
                    "client_capability_registry.entries",
                ));
            }
            by_fingerprint.insert(capability.fingerprint.clone(), capability);
        }
        Ok(Self { by_fingerprint })
    }

    /// Number of admitted capabilities.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_fingerprint.len()
    }

    /// Whether no capabilities have been admitted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_fingerprint.is_empty()
    }

    /// Resolve a capability fingerprint carried by a typed transport or action
    /// reference. Malformed fingerprints are rejected by returning `None`.
    #[must_use]
    pub fn lookup(&self, fingerprint: &str) -> Option<&ClientCapability> {
        if !valid_prefixed_fingerprint(fingerprint, CAPABILITY_FINGERPRINT_PREFIX) {
            return None;
        }
        self.by_fingerprint.get(fingerprint)
    }

    /// Iterate over admitted fingerprints in canonical byte order.
    pub fn fingerprints(&self) -> impl Iterator<Item = &str> {
        self.by_fingerprint.keys().map(String::as_str)
    }

    /// Iterate over admitted capabilities in fingerprint order.
    pub fn capabilities(&self) -> impl Iterator<Item = &ClientCapability> {
        self.by_fingerprint.values()
    }
}

/// Opaque reference into the approved secret store.
///
/// This type intentionally exposes no constructor that accepts arbitrary
/// credential material. References use the existing `namespace/key` or
/// `secret:key` forms and reject whitespace, traversal, URI/query syntax, and
/// secret-value delimiters.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SecretReference(String);

impl SecretReference {
    /// Validate and construct an opaque secret-store reference.
    ///
    /// # Errors
    ///
    /// Returns an error for a blank, overlong, traversal-shaped, URI-shaped, or
    /// otherwise malformed secret-store name.
    pub fn new(value: impl Into<String>) -> Result<Self, ResourceValidationError> {
        let value = value.into();
        validate_secret_reference(&value)?;
        Ok(Self(value))
    }

    /// Borrow the opaque reference for a secret-store lookup.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for SecretReference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Coarse authorization state for a resource card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStatus {
    /// No authentication is required.
    NotRequired,
    /// One of the advertised methods is required.
    Required,
    /// Pairing/approval/authentication is in progress and time-bounded.
    Pending,
    /// An admitted session is authorized.
    Authorized,
    /// An authentication attempt was denied.
    Denied,
    /// Prior authorization was explicitly revoked.
    Revoked,
    /// Required credential/provider support is unavailable.
    Unavailable,
}

/// Versioned auth state carrying only methods and an optional opaque reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthState {
    /// Resource-contract schema discriminator.
    pub schema_version: u16,
    /// Coarse state.
    pub status: AuthStatus,
    /// Methods accepted by the selected client/resource, deduplicated.
    pub accepted_methods: Vec<AuthMethod>,
    /// Method currently pending/authorized/failed, when applicable.
    pub active_method: Option<AuthMethod>,
    /// Opaque secret-store reference; never credential material.
    pub credential_ref: Option<SecretReference>,
    /// Unix epoch milliseconds when this state changed.
    pub updated_at_ms: u64,
    /// Pairing/approval/session expiry, when applicable.
    pub expires_at_ms: Option<u64>,
    /// Required for denied/revoked/unavailable states.
    pub failure: Option<FailureReason>,
}

impl AuthState {
    /// Validate state transitions' static shape, expiry, and secret-reference use.
    ///
    /// # Errors
    ///
    /// Returns an error for inconsistent methods/state, unbounded expiry,
    /// missing opaque references, or unsafe diagnostic text.
    pub fn validate(&self) -> Result<(), ResourceValidationError> {
        validate_version("auth_state", self.schema_version)?;
        if self.updated_at_ms == 0 {
            return Err(ResourceValidationError::InvalidTimestamp(
                "auth.updated_at_ms",
            ));
        }
        validate_unique(
            "auth.accepted_methods",
            &self.accepted_methods,
            MAX_AUTH_METHODS,
        )?;
        if let Some(active) = self.active_method {
            if !self.accepted_methods.contains(&active) {
                return Err(ResourceValidationError::InvalidRelationship(
                    "auth.active_method",
                ));
            }
        }
        if let Some(expires_at_ms) = self.expires_at_ms {
            validate_freshness("auth_state", self.updated_at_ms, expires_at_ms)?;
        }
        if let Some(failure) = &self.failure {
            failure.validate("auth.failure.message")?;
        }

        let valid = match self.status {
            AuthStatus::NotRequired => {
                self.accepted_methods.is_empty()
                    && self.active_method.is_none()
                    && self.credential_ref.is_none()
                    && self.expires_at_ms.is_none()
                    && self.failure.is_none()
            }
            AuthStatus::Required => {
                !self.accepted_methods.is_empty()
                    && self.active_method.is_none()
                    && self.credential_ref.is_none()
                    && self.expires_at_ms.is_none()
                    && self.failure.is_none()
            }
            AuthStatus::Pending => {
                !self.accepted_methods.is_empty()
                    && self.active_method.is_some()
                    && self.credential_ref.is_none()
                    && self.expires_at_ms.is_some()
                    && self.failure.is_none()
            }
            AuthStatus::Authorized => {
                !self.accepted_methods.is_empty()
                    && self.active_method.is_some_and(|method| {
                        !method.requires_secret_reference() || self.credential_ref.is_some()
                    })
                    && self.failure.is_none()
            }
            AuthStatus::Denied | AuthStatus::Revoked | AuthStatus::Unavailable => {
                self.credential_ref.is_none() && self.failure.is_some()
            }
        };
        if !valid {
            return Err(ResourceValidationError::InvalidRelationship(
                "auth.state_fields",
            ));
        }
        Ok(())
    }
}

/// Availability gate on a typed action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionAvailabilityStatus {
    /// Action may execute through its typed adapter.
    Ready,
    /// Action is gated until authentication succeeds.
    RequiresAuth,
    /// Action is gated until explicit approval succeeds.
    RequiresApproval,
    /// Action is retained as unavailable evidence only.
    Unavailable,
}

/// Typed action availability and safe diagnostic context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionAvailability {
    /// Coarse gate state.
    pub status: ActionAvailabilityStatus,
    /// Required for every non-ready state and forbidden for ready state.
    pub failure: Option<FailureReason>,
}

impl ActionAvailability {
    fn validate(&self) -> Result<(), ResourceValidationError> {
        if let Some(failure) = &self.failure {
            failure.validate("action.availability.failure.message")?;
        }
        match self.status {
            ActionAvailabilityStatus::Ready if self.failure.is_some() => Err(
                ResourceValidationError::UnexpectedFailure("action.availability"),
            ),
            ActionAvailabilityStatus::RequiresAuth
            | ActionAvailabilityStatus::RequiresApproval
            | ActionAvailabilityStatus::Unavailable
                if self.failure.is_none() =>
            {
                Err(ResourceValidationError::MissingFailure(
                    "action.availability",
                ))
            }
            _ => Ok(()),
        }
    }
}

/// Closed action target; there is intentionally no command, path, or URL case.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceActionTarget {
    /// The resource card itself.
    Resource,
    /// One known typed transport.
    Transport {
        /// Fingerprint of the target transport candidate.
        transport_fingerprint: String,
    },
    /// One typed transport admitted through one registered client capability.
    TransportClient {
        /// Fingerprint of the target transport candidate.
        transport_fingerprint: String,
        /// Fingerprint of the client capability authorized to consume it.
        capability_fingerprint: String,
    },
}

impl ResourceActionTarget {
    fn validate(&self) -> Result<(), ResourceValidationError> {
        match self {
            Self::Resource => Ok(()),
            Self::Transport {
                transport_fingerprint,
            } => validate_fingerprint_field(
                "action.target.transport_fingerprint",
                transport_fingerprint,
                TRANSPORT_FINGERPRINT_PREFIX,
            ),
            Self::TransportClient {
                transport_fingerprint,
                capability_fingerprint,
            } => {
                validate_fingerprint_field(
                    "action.target.transport_fingerprint",
                    transport_fingerprint,
                    TRANSPORT_FINGERPRINT_PREFIX,
                )?;
                validate_fingerprint_field(
                    "action.target.capability_fingerprint",
                    capability_fingerprint,
                    CAPABILITY_FINGERPRINT_PREFIX,
                )
            }
        }
    }
}

/// Versioned, expiring action declaration with no arbitrary execution payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceAction {
    /// Resource-contract schema discriminator.
    pub schema_version: u16,
    /// Stable card-local action identity.
    pub action_id: String,
    /// Closed action verb.
    pub verb: ResourceActionVerb,
    /// Typed resource/transport/client binding.
    pub target: ResourceActionTarget,
    /// Current auth/approval/availability gate.
    pub availability: ActionAvailability,
    /// Unix epoch milliseconds when this declaration was issued.
    pub issued_at_ms: u64,
    /// Unix epoch milliseconds when the control must be discarded/refreshed.
    pub expires_at_ms: u64,
}

impl ResourceAction {
    /// Validate action grammar, TTL, availability, and verb/target binding.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed IDs, unbounded expiry, unsafe failure
    /// context, or a verb bound to the wrong typed target.
    pub fn validate(&self) -> Result<(), ResourceValidationError> {
        validate_version("resource_action", self.schema_version)?;
        validate_identifier("action.action_id", &self.action_id)?;
        validate_freshness("resource_action", self.issued_at_ms, self.expires_at_ms)?;
        self.target.validate()?;
        self.availability.validate()?;
        let target_is_valid = match self.verb {
            ResourceActionVerb::Inspect
            | ResourceActionVerb::Forget
            | ResourceActionVerb::Configure
            | ResourceActionVerb::Test
            | ResourceActionVerb::Enable
            | ResourceActionVerb::Disable
            | ResourceActionVerb::Remove => {
                matches!(self.target, ResourceActionTarget::Resource)
            }
            ResourceActionVerb::Pair | ResourceActionVerb::Retry => {
                matches!(self.target, ResourceActionTarget::Transport { .. })
            }
            ResourceActionVerb::Connect => {
                matches!(self.target, ResourceActionTarget::TransportClient { .. })
            }
            ResourceActionVerb::Launch => matches!(
                self.target,
                ResourceActionTarget::Resource | ResourceActionTarget::TransportClient { .. }
            ),
            ResourceActionVerb::Start | ResourceActionVerb::Resume => {
                matches!(self.target, ResourceActionTarget::Resource)
            }
            ResourceActionVerb::Transfer => {
                matches!(self.target, ResourceActionTarget::TransportClient { .. })
            }
            ResourceActionVerb::RequestApproval => {
                !matches!(self.target, ResourceActionTarget::TransportClient { .. })
            }
        };
        if !target_is_valid {
            return Err(ResourceValidationError::InvalidRelationship(
                "action.verb_target",
            ));
        }
        Ok(())
    }
}

/// One deduplicated resource card projected into Remote Sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceCard {
    /// Resource-contract schema discriminator.
    pub schema_version: u16,
    /// Stable identity and aliases.
    pub identity: ResourceIdentity,
    /// Bounded user-facing title.
    pub display_name: String,
    /// Optional bounded, non-secret summary.
    pub summary: Option<String>,
    /// Earliest retained observation of this stable identity.
    pub first_seen_at_ms: u64,
    /// Freshest retained observation from any source/transport.
    pub last_seen_at_ms: u64,
    /// Card retention expiry; offline cards may remain until this deadline.
    pub expires_at_ms: u64,
    /// Coarse aggregate health.
    pub health: HealthState,
    /// Resource authorization state.
    pub auth: AuthState,
    /// Source attestations, deduplicated by source/scope/source ID.
    pub provenance: Vec<SourceProvenance>,
    /// Typed endpoint observations, deduplicated by fingerprint.
    pub transports: Vec<TransportCandidate>,
    /// Registered clients admitted for this resource, deduplicated by fingerprint.
    pub client_capabilities: Vec<ClientCapability>,
    /// Closed, expiring controls rendered for this card.
    pub actions: Vec<ResourceAction>,
    /// Construct roles admitted for this resource. A card is never ornamental:
    /// at least one of client, loader, or host must be declared.
    pub operating_roles: Vec<ResourceOperatingRole>,
    /// First-class service interface. Present for service-like cards and absent
    /// for plain nodes/desktops/applications.
    pub service: Option<ServiceInterface>,
}

impl ResourceCard {
    /// Stable deduplication key for this card.
    #[must_use]
    pub fn resource_id(&self) -> &str {
        &self.identity.resource_id
    }

    /// Whether the card has exceeded its bounded retention lifetime.
    #[must_use]
    pub const fn is_expired(&self, now_ms: u64) -> bool {
        now_ms >= self.expires_at_ms
    }

    /// Validate the complete card, including all cross-references.
    ///
    /// # Errors
    ///
    /// Returns an error when a nested contract is invalid, freshness is
    /// inconsistent, a deduplication key repeats, or an action/reference does
    /// not bind an admitted transport and client capability.
    pub fn validate(&self) -> Result<(), ResourceValidationError> {
        validate_version("resource_card", self.schema_version)?;
        self.identity.validate()?;
        validate_text(
            "resource_card.display_name",
            &self.display_name,
            MAX_DISPLAY_NAME_BYTES,
        )?;
        if let Some(summary) = &self.summary {
            validate_text("resource_card.summary", summary, MAX_SUMMARY_BYTES)?;
            if looks_like_secret(summary) {
                return Err(ResourceValidationError::SecretShapedValue(
                    "resource_card.summary",
                ));
            }
        }
        if self.first_seen_at_ms == 0
            || self.last_seen_at_ms < self.first_seen_at_ms
            || self.expires_at_ms <= self.last_seen_at_ms
        {
            return Err(ResourceValidationError::InvalidTimestamp(
                "resource_card.freshness",
            ));
        }
        validate_freshness("resource_card", self.last_seen_at_ms, self.expires_at_ms)?;

        self.health.validate()?;
        if self.health.observed_at_ms > self.last_seen_at_ms
            || self.health.expires_at_ms > self.expires_at_ms
        {
            return Err(ResourceValidationError::InvalidRelationship(
                "resource_card.health_freshness",
            ));
        }
        self.auth.validate()?;
        if self.auth.updated_at_ms > self.expires_at_ms
            || self
                .auth
                .expires_at_ms
                .is_some_and(|expires| expires > self.expires_at_ms)
        {
            return Err(ResourceValidationError::InvalidRelationship(
                "resource_card.auth_freshness",
            ));
        }

        validate_unique(
            "resource_card.operating_roles",
            &self.operating_roles,
            MAX_OPERATING_ROLES,
        )?;
        if self.operating_roles.is_empty() {
            return Err(ResourceValidationError::InvalidField(
                "resource_card.operating_roles",
            ));
        }
        if let Some(service) = &self.service {
            service.validate()?;
            if !matches!(
                self.identity.class,
                ResourceClass::MediaServer
                    | ResourceClass::FileShare
                    | ResourceClass::CloudWorkload
                    | ResourceClass::Service
            ) {
                return Err(ResourceValidationError::InvalidRelationship(
                    "resource_card.service_class",
                ));
            }
        }

        self.validate_provenance()?;
        let capability_fingerprints = self.validate_capabilities()?;
        let transport_fingerprints = self.validate_transports(&capability_fingerprints)?;
        self.validate_actions(&transport_fingerprints, &capability_fingerprints)
    }

    fn validate_provenance(&self) -> Result<(), ResourceValidationError> {
        validate_capacity(
            "resource_card.provenance",
            self.provenance.len(),
            MAX_PROVENANCE,
        )?;
        if self.provenance.is_empty() {
            return Err(ResourceValidationError::InvalidField(
                "resource_card.provenance",
            ));
        }
        let mut provenance_keys = BTreeSet::new();
        for provenance in &self.provenance {
            provenance.validate()?;
            let observation_is_future = provenance.observed_at_ms > self.last_seen_at_ms;
            let expiry_outlives_card = provenance.expires_at_ms > self.expires_at_ms;
            if observation_is_future || expiry_outlives_card {
                return Err(ResourceValidationError::InvalidRelationship(
                    "resource_card.provenance_freshness",
                ));
            }
            let key = (provenance.source, provenance.scope, &provenance.source_id);
            if !provenance_keys.insert(key) {
                return Err(ResourceValidationError::Duplicate(
                    "resource_card.provenance",
                ));
            }
        }
        Ok(())
    }

    fn validate_capabilities(&self) -> Result<BTreeSet<&str>, ResourceValidationError> {
        validate_capacity(
            "resource_card.client_capabilities",
            self.client_capabilities.len(),
            MAX_CAPABILITIES,
        )?;
        let mut fingerprints = BTreeSet::new();
        for capability in &self.client_capabilities {
            capability.validate()?;
            if !fingerprints.insert(capability.fingerprint.as_str()) {
                return Err(ResourceValidationError::Duplicate(
                    "resource_card.client_capabilities",
                ));
            }
        }
        Ok(fingerprints)
    }

    fn validate_transports<'a>(
        &'a self,
        capability_fingerprints: &BTreeSet<&str>,
    ) -> Result<BTreeSet<&'a str>, ResourceValidationError> {
        validate_capacity(
            "resource_card.transports",
            self.transports.len(),
            MAX_TRANSPORTS,
        )?;
        let mut fingerprints = BTreeSet::new();
        for transport in &self.transports {
            transport.validate()?;
            if transport.last_seen_at_ms > self.last_seen_at_ms
                || transport.expires_at_ms > self.expires_at_ms
            {
                return Err(ResourceValidationError::InvalidRelationship(
                    "resource_card.transport_freshness",
                ));
            }
            if !fingerprints.insert(transport.fingerprint.as_str()) {
                return Err(ResourceValidationError::Duplicate(
                    "resource_card.transports",
                ));
            }
            if let Some(fingerprint) = &transport.client_capability_fingerprint {
                if !capability_fingerprints.contains(fingerprint.as_str()) {
                    return Err(ResourceValidationError::InvalidRelationship(
                        "transport.capability_reference",
                    ));
                }
                let capability = self
                    .client_capabilities
                    .iter()
                    .find(|capability| &capability.fingerprint == fingerprint)
                    .ok_or(ResourceValidationError::InvalidRelationship(
                        "transport.capability_reference",
                    ))?;
                if capability.protocol != transport.protocol {
                    return Err(ResourceValidationError::InvalidRelationship(
                        "transport.capability_protocol",
                    ));
                }
            }
        }
        Ok(fingerprints)
    }

    fn validate_actions(
        &self,
        transport_fingerprints: &BTreeSet<&str>,
        capability_fingerprints: &BTreeSet<&str>,
    ) -> Result<(), ResourceValidationError> {
        validate_capacity("resource_card.actions", self.actions.len(), MAX_ACTIONS)?;
        let mut action_ids = BTreeSet::new();
        for action in &self.actions {
            action.validate()?;
            if action.expires_at_ms > self.expires_at_ms {
                return Err(ResourceValidationError::InvalidRelationship(
                    "resource_card.action_freshness",
                ));
            }
            if !action_ids.insert(action.action_id.as_str()) {
                return Err(ResourceValidationError::Duplicate("resource_card.actions"));
            }
            self.validate_action_references(
                action,
                transport_fingerprints,
                capability_fingerprints,
            )?;
        }
        Ok(())
    }

    fn validate_action_references(
        &self,
        action: &ResourceAction,
        transports: &BTreeSet<&str>,
        capabilities: &BTreeSet<&str>,
    ) -> Result<(), ResourceValidationError> {
        let (transport_fingerprint, capability_fingerprint) = match &action.target {
            ResourceActionTarget::Resource => (None, None),
            ResourceActionTarget::Transport {
                transport_fingerprint,
            } => (Some(transport_fingerprint.as_str()), None),
            ResourceActionTarget::TransportClient {
                transport_fingerprint,
                capability_fingerprint,
            } => (
                Some(transport_fingerprint.as_str()),
                Some(capability_fingerprint.as_str()),
            ),
        };
        if transport_fingerprint.is_some_and(|fingerprint| !transports.contains(fingerprint)) {
            return Err(ResourceValidationError::InvalidRelationship(
                "action.transport_reference",
            ));
        }
        if capability_fingerprint.is_some_and(|fingerprint| !capabilities.contains(fingerprint)) {
            return Err(ResourceValidationError::InvalidRelationship(
                "action.capability_reference",
            ));
        }
        if let (Some(transport_fingerprint), Some(capability_fingerprint)) =
            (transport_fingerprint, capability_fingerprint)
        {
            let transport = self
                .transports
                .iter()
                .find(|candidate| candidate.fingerprint == transport_fingerprint)
                .ok_or(ResourceValidationError::InvalidRelationship(
                    "action.transport_reference",
                ))?;
            let capability = self
                .client_capabilities
                .iter()
                .find(|candidate| candidate.fingerprint == capability_fingerprint)
                .ok_or(ResourceValidationError::InvalidRelationship(
                    "action.capability_reference",
                ))?;
            if transport.protocol != capability.protocol
                || transport.client_capability_fingerprint.as_deref()
                    != Some(capability_fingerprint)
                || !capability.safe_actions.contains(&action.verb)
            {
                return Err(ResourceValidationError::InvalidRelationship(
                    "action.transport_client_binding",
                ));
            }
            if matches!(
                action.verb,
                ResourceActionVerb::Connect | ResourceActionVerb::Launch
            ) && action.availability.status == ActionAvailabilityStatus::Ready
                && (!matches!(
                    self.health.status,
                    HealthStatus::Available | HealthStatus::Degraded
                ) || !matches!(
                    transport.health.status,
                    HealthStatus::Available | HealthStatus::Degraded
                ) || !matches!(
                    self.auth.status,
                    AuthStatus::NotRequired | AuthStatus::Authorized
                ))
            {
                return Err(ResourceValidationError::InvalidRelationship(
                    "action.ready_connect_state",
                ));
            }
        }
        Ok(())
    }
}

/// Safe, bounded browser-facing summary of one admitted resource card.
///
/// This projection deliberately omits endpoints, credential references,
/// diagnostic messages, and executable details. A consumer can use it to
/// group and filter resources before selecting the full card for a typed
/// action. All set-like fields are emitted in their enum order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceDiscoveryEntry {
    /// Resource-contract schema discriminator.
    pub schema_version: u16,
    /// Stable identity used to retrieve the full card and bind an action.
    pub resource_id: String,
    /// Broad resource class used for browser grouping.
    pub class: ResourceClass,
    /// Bounded user-facing title.
    pub display_name: String,
    /// Optional non-secret summary retained from the full card.
    pub summary: Option<String>,
    /// Current aggregate resource health.
    pub health_status: HealthStatus,
    /// Current resource authorization state.
    pub auth_status: AuthStatus,
    /// Freshness metadata for stale/offline presentation.
    pub last_seen_at_ms: u64,
    /// Retention deadline for the discovery entry.
    pub expires_at_ms: u64,
    /// Discovery lanes that contributed this identity.
    pub discovery_sources: Vec<DiscoverySource>,
    /// Reachability boundaries represented by provenance or transports.
    pub reachability_scopes: Vec<ResourceScope>,
    /// Typed protocols observed for this resource.
    pub transport_protocols: Vec<TransportProtocol>,
    /// Closed action verbs currently declared ready on the card.
    pub ready_actions: Vec<ResourceActionVerb>,
    /// Service grouping, when the resource exposes a service interface.
    pub service_category: Option<ServiceCategory>,
}

impl ResourceDiscoveryEntry {
    fn from_card(card: &ResourceCard) -> Self {
        let mut discovery_sources = BTreeSet::new();
        let mut reachability_scopes = BTreeSet::new();
        for provenance in &card.provenance {
            discovery_sources.insert(provenance.source);
            reachability_scopes.insert(provenance.scope);
        }

        let mut transport_protocols = BTreeSet::new();
        for transport in &card.transports {
            transport_protocols.insert(transport.protocol);
            reachability_scopes.insert(transport.scope);
        }

        let mut ready_actions = BTreeSet::new();
        for action in &card.actions {
            if action.availability.status == ActionAvailabilityStatus::Ready {
                ready_actions.insert(action.verb);
            }
        }

        Self {
            schema_version: RESOURCE_CONTRACT_VERSION,
            resource_id: card.resource_id().to_owned(),
            class: card.identity.class,
            display_name: card.display_name.clone(),
            summary: card.summary.clone(),
            health_status: card.health.status,
            auth_status: card.auth.status,
            last_seen_at_ms: card.last_seen_at_ms,
            expires_at_ms: card.expires_at_ms,
            discovery_sources: discovery_sources.into_iter().collect(),
            reachability_scopes: reachability_scopes.into_iter().collect(),
            transport_protocols: transport_protocols.into_iter().collect(),
            ready_actions: ready_actions.into_iter().collect(),
            service_category: card.service.as_ref().map(|service| service.category),
        }
    }

    /// Validate a decoded discovery entry before it is rendered or acted on.
    pub fn validate(&self) -> Result<(), ResourceValidationError> {
        validate_version("resource_discovery_entry", self.schema_version)?;
        validate_fingerprint_field(
            "resource_discovery_entry.resource_id",
            &self.resource_id,
            RESOURCE_ID_PREFIX,
        )?;
        validate_text(
            "resource_discovery_entry.display_name",
            &self.display_name,
            MAX_DISPLAY_NAME_BYTES,
        )?;
        if let Some(summary) = &self.summary {
            validate_text(
                "resource_discovery_entry.summary",
                summary,
                MAX_SUMMARY_BYTES,
            )?;
            if looks_like_secret(summary) {
                return Err(ResourceValidationError::SecretShapedValue(
                    "resource_discovery_entry.summary",
                ));
            }
        }
        if self.last_seen_at_ms == 0 || self.expires_at_ms <= self.last_seen_at_ms {
            return Err(ResourceValidationError::InvalidTimestamp(
                "resource_discovery_entry.freshness",
            ));
        }
        validate_freshness(
            "resource_discovery_entry",
            self.last_seen_at_ms,
            self.expires_at_ms,
        )?;
        validate_unique(
            "resource_discovery_entry.discovery_sources",
            &self.discovery_sources,
            MAX_DISCOVERY_SOURCES,
        )?;
        validate_unique(
            "resource_discovery_entry.reachability_scopes",
            &self.reachability_scopes,
            MAX_DISCOVERY_SCOPES,
        )?;
        validate_unique(
            "resource_discovery_entry.transport_protocols",
            &self.transport_protocols,
            MAX_DISCOVERY_PROTOCOLS,
        )?;
        validate_unique(
            "resource_discovery_entry.ready_actions",
            &self.ready_actions,
            MAX_DISCOVERY_ACTIONS,
        )?;
        Ok(())
    }
}

/// Deterministically ordered, bounded projection for resource/session
/// discovery clients.
///
/// The projection is derived only from an admitted [`ResourceCatalog`]. Its
/// entries are sorted by resource class, display name, and stable resource ID;
/// callers therefore do not depend on discovery-worker arrival order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceDiscoveryProjection {
    /// Resource-contract schema discriminator.
    pub schema_version: u16,
    /// Source catalog revision represented by this projection.
    pub revision: String,
    /// Optional integrity reference to the source catalog's content digest.
    ///
    /// This is a deterministic integrity digest, not a cryptographic
    /// signature. It detects content drift under an honest publisher; use the
    /// catalog's detached publisher attestation for authenticated admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_content_digest: Option<String>,
    /// Node that published the source catalog.
    pub publisher: String,
    /// Generation time copied from the source catalog.
    pub generated_at_ms: u64,
    /// Deterministically ordered browser entries.
    pub entries: Vec<ResourceDiscoveryEntry>,
}

impl ResourceDiscoveryProjection {
    /// Validate the projection's bounded entries and source metadata.
    pub fn validate(&self) -> Result<(), ResourceValidationError> {
        validate_version("resource_discovery_projection", self.schema_version)?;
        validate_identifier("resource_discovery_projection.revision", &self.revision)?;
        if let Some(digest) = &self.catalog_content_digest {
            validate_fingerprint_field(
                "resource_discovery_projection.catalog_content_digest",
                digest,
                RESOURCE_CATALOG_CONTENT_DIGEST_PREFIX,
            )?;
        }
        validate_identifier("resource_discovery_projection.publisher", &self.publisher)?;
        if self.generated_at_ms == 0 {
            return Err(ResourceValidationError::InvalidTimestamp(
                "resource_discovery_projection.generated_at_ms",
            ));
        }
        validate_capacity(
            "resource_discovery_projection.entries",
            self.entries.len(),
            MAX_DISCOVERY_ENTRIES,
        )?;
        let mut resource_ids = BTreeSet::new();
        let mut previous = None;
        for entry in &self.entries {
            entry.validate()?;
            if entry.last_seen_at_ms > self.generated_at_ms {
                return Err(ResourceValidationError::InvalidRelationship(
                    "resource_discovery_projection.entry_generated_at",
                ));
            }
            if !resource_ids.insert(entry.resource_id.as_str()) {
                return Err(ResourceValidationError::Duplicate(
                    "resource_discovery_projection.entries",
                ));
            }
            if previous.is_some_and(|previous| discovery_entry_cmp(previous, entry).is_gt()) {
                return Err(ResourceValidationError::InvalidRelationship(
                    "resource_discovery_projection.entry_order",
                ));
            }
            previous = Some(entry);
        }
        Ok(())
    }

    /// Consume and return only a fully validated discovery projection.
    pub fn admitted(self) -> Result<Self, ResourceValidationError> {
        self.validate()?;
        Ok(self)
    }
}

fn discovery_entry_cmp(left: &ResourceDiscoveryEntry, right: &ResourceDiscoveryEntry) -> Ordering {
    left.class
        .cmp(&right.class)
        .then_with(|| left.display_name.cmp(&right.display_name))
        .then_with(|| left.resource_id.cmp(&right.resource_id))
}

/// Versioned retained-latest body published on [`RESOURCE_CATALOG_TOPIC`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceCatalog {
    /// Resource-contract schema discriminator.
    pub schema_version: u16,
    /// Monotonic/latest-wins publisher revision token.
    pub revision: String,
    /// Publishing node identity.
    pub publisher: String,
    /// Unix epoch milliseconds when this snapshot was generated.
    pub generated_at_ms: u64,
    /// Optional deterministic content digest over the catalog metadata and
    /// cards, excluding this field itself.
    ///
    /// The digest is an integrity reference, not a cryptographic signature or
    /// proof of publisher identity. A detached
    /// [`ResourcePublisherAttestation`] provides that stronger admission
    /// boundary when a trusted key is available. It is optional only for
    /// backward-compatible decoding of legacy retained catalog JSON; new
    /// publication paths attach it before emitting a catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_digest: Option<String>,
    /// One card per stable resource ID.
    pub cards: Vec<ResourceCard>,
}

impl ResourceCatalog {
    /// Decode strict JSON with a pre-allocation wire bound, then validate every
    /// nested record and reference.
    ///
    /// # Errors
    ///
    /// Returns [`ResourceCatalogDecodeError::BodyTooLarge`] before parsing an
    /// oversized body, [`ResourceCatalogDecodeError::Json`] for malformed or
    /// structurally unknown JSON, or [`ResourceCatalogDecodeError::Validation`]
    /// when semantic admission fails.
    pub fn from_json(body: &str) -> Result<Self, ResourceCatalogDecodeError> {
        if body.len() > MAX_RESOURCE_CATALOG_BYTES {
            return Err(ResourceCatalogDecodeError::BodyTooLarge {
                bytes: body.len(),
                max: MAX_RESOURCE_CATALOG_BYTES,
            });
        }
        let catalog: Self = serde_json::from_str(body).map_err(ResourceCatalogDecodeError::Json)?;
        catalog
            .validate()
            .map_err(ResourceCatalogDecodeError::Validation)?;
        Ok(catalog)
    }

    /// Validate a decoded catalog before publication or consumption.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsupported schema, malformed publisher metadata,
    /// duplicate identities, invalid nested cards, future observations, or
    /// dangling gateway-card references.
    pub fn validate(&self) -> Result<(), ResourceValidationError> {
        validate_version("resource_catalog", self.schema_version)?;
        validate_identifier("resource_catalog.revision", &self.revision)?;
        validate_identifier("resource_catalog.publisher", &self.publisher)?;
        if self.generated_at_ms == 0 {
            return Err(ResourceValidationError::InvalidTimestamp(
                "resource_catalog.generated_at_ms",
            ));
        }
        if let Some(digest) = &self.content_digest {
            validate_fingerprint_field(
                "resource_catalog.content_digest",
                digest,
                RESOURCE_CATALOG_CONTENT_DIGEST_PREFIX,
            )?;
        }
        validate_capacity("resource_catalog.cards", self.cards.len(), MAX_CARDS)?;
        let mut resource_ids = BTreeSet::new();
        for card in &self.cards {
            card.validate()?;
            if card.last_seen_at_ms > self.generated_at_ms
                || card.auth.updated_at_ms > self.generated_at_ms
                || card
                    .actions
                    .iter()
                    .any(|action| action.issued_at_ms > self.generated_at_ms)
            {
                return Err(ResourceValidationError::InvalidRelationship(
                    "resource_catalog.card_generated_at",
                ));
            }
            if !resource_ids.insert(card.resource_id()) {
                return Err(ResourceValidationError::Duplicate("resource_catalog.cards"));
            }
        }
        for card in &self.cards {
            if let Some(service) = &card.service {
                for dependency in &service.stack.dependencies {
                    if dependency == card.resource_id()
                        || !resource_ids.contains(dependency.as_str())
                    {
                        return Err(ResourceValidationError::InvalidRelationship(
                            "service.stack.dependency_reference",
                        ));
                    }
                }
            }
        }
        for card in &self.cards {
            for transport in &card.transports {
                if let TransportEndpoint::Gateway {
                    gateway_resource_id,
                    ..
                } = &transport.endpoint
                {
                    if gateway_resource_id == card.resource_id()
                        || !resource_ids.contains(gateway_resource_id.as_str())
                    {
                        return Err(ResourceValidationError::InvalidRelationship(
                            "transport.gateway_resource_reference",
                        ));
                    }
                }
            }
        }
        if let Some(digest) = &self.content_digest {
            if digest != &self.computed_content_digest() {
                return Err(ResourceValidationError::FingerprintMismatch(
                    "resource_catalog.content_digest",
                ));
            }
        }
        Ok(())
    }

    /// Compute the deterministic integrity digest for this catalog.
    ///
    /// Metadata is included explicitly and cards are serialized in stable
    /// resource-ID order, so card arrival order does not affect the result.
    /// The catalog's own `content_digest` field is intentionally excluded.
    /// This is not a cryptographic signature; use
    /// [`ResourcePublisherAttestation`] when publisher authentication is
    /// required.
    #[must_use]
    pub fn computed_content_digest(&self) -> String {
        let mut cards: Vec<_> = self.cards.iter().collect();
        cards.sort_unstable_by(|left, right| left.resource_id().cmp(right.resource_id()));

        let mut canonical = String::new();
        push_canonical(&mut canonical, "resource-catalog-content");
        push_canonical(&mut canonical, &self.schema_version.to_string());
        push_canonical(&mut canonical, &self.revision);
        push_canonical(&mut canonical, &self.publisher);
        push_canonical(&mut canonical, &self.generated_at_ms.to_string());
        push_canonical(&mut canonical, &cards.len().to_string());
        for card in cards {
            let card_json =
                serde_json::to_string(card).expect("ResourceCard's derived serializer cannot fail");
            push_canonical(&mut canonical, &card_json);
        }
        format!(
            "{RESOURCE_CATALOG_CONTENT_DIGEST_PREFIX}{}",
            sha256_hex(&canonical)
        )
    }

    /// Validate this catalog and return it with a content digest attached.
    ///
    /// A missing digest is filled for a new publication. An existing digest is
    /// validated before it can be retained, so this helper cannot silently
    /// repair a forged or stale attestation.
    pub fn with_content_digest(mut self) -> Result<Self, ResourceValidationError> {
        self.validate()?;
        self.content_digest = Some(self.computed_content_digest());
        self.validate()?;
        Ok(self)
    }

    /// Build the safe, deterministically ordered browser projection used by
    /// resource/session discovery clients.
    ///
    /// The source catalog is validated first, so the projection cannot carry
    /// a forged identity, stale cross-reference, or unadmitted action summary.
    pub fn discovery_projection(
        &self,
    ) -> Result<ResourceDiscoveryProjection, ResourceValidationError> {
        self.validate()?;
        let mut entries: Vec<_> = self
            .cards
            .iter()
            .map(ResourceDiscoveryEntry::from_card)
            .collect();
        entries.sort_unstable_by(discovery_entry_cmp);
        let projection = ResourceDiscoveryProjection {
            schema_version: RESOURCE_CONTRACT_VERSION,
            revision: self.revision.clone(),
            catalog_content_digest: self.content_digest.clone(),
            publisher: self.publisher.clone(),
            generated_at_ms: self.generated_at_ms,
            entries,
        };
        projection.validate()?;
        Ok(projection)
    }

    /// Verify a detached publisher attestation before trusting this catalog.
    ///
    /// The attestation is HMAC-SHA256 over a canonical envelope containing the
    /// publisher, key ID, exact catalog content digest, and bounded validity
    /// window. The key itself is never serialized into the resource contract.
    /// This is the verification primitive for authenticated admission;
    /// [`Self::admit_authenticated`] produces the typed authenticated state.
    /// [`Self::validate`] remains the compatibility path for legacy catalogs
    /// without a detached proof.
    ///
    /// # Errors
    ///
    /// Returns an error when the catalog, attestation shape, publisher binding,
    /// freshness window, digest, or HMAC does not validate.
    pub fn validate_publisher_attestation(
        &self,
        attestation: &ResourcePublisherAttestation,
        key: &[u8],
        now_ms: u64,
    ) -> Result<(), ResourceValidationError> {
        attestation.verify_at(self, key, now_ms)
    }

    /// Admit this catalog into authenticated resource state.
    ///
    /// The detached proof must use the currently trusted publisher key ID and
    /// must verify against the supplied trusted key at `now_ms`. The ordinary
    /// [`Self::admitted`] path remains available for backward-compatible,
    /// explicitly untrusted consumers; it never produces this wrapper.
    ///
    /// # Errors
    ///
    /// Returns an error when the proof names an unsupported key ID, is stale or
    /// malformed, does not bind this exact catalog, or fails HMAC verification.
    pub fn admit_authenticated(
        self,
        attestation: ResourcePublisherAttestation,
        key: &[u8],
        now_ms: u64,
    ) -> Result<AuthenticatedResourceCatalog, ResourceValidationError> {
        if attestation.key_id != RESOURCE_PUBLISHER_ATTESTATION_KEY_ID {
            return Err(ResourceValidationError::InvalidField(
                "resource_publisher_attestation.key_id",
            ));
        }
        self.validate_publisher_attestation(&attestation, key, now_ms)?;
        Ok(AuthenticatedResourceCatalog {
            catalog: self,
            publisher_attestation: attestation,
        })
    }

    /// Consume and return only a fully validated catalog.
    ///
    /// # Errors
    ///
    /// Returns the same semantic validation failures as [`Self::validate`].
    pub fn admitted(self) -> Result<Self, ResourceValidationError> {
        self.validate()?;
        Ok(self)
    }
}

/// A resource catalog that crossed the cryptographic publisher-admission gate.
///
/// Keeping the proof alongside the catalog makes the stronger state explicit
/// to consumers. A plain [`ResourceCatalog`] is still valid for the legacy
/// untrusted path, but it cannot be mistaken for this authenticated wrapper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedResourceCatalog {
    catalog: ResourceCatalog,
    publisher_attestation: ResourcePublisherAttestation,
}

impl AuthenticatedResourceCatalog {
    /// Borrow the catalog covered by the verified detached proof.
    #[must_use]
    pub fn catalog(&self) -> &ResourceCatalog {
        &self.catalog
    }

    /// Borrow the detached proof that authenticated the catalog.
    #[must_use]
    pub fn publisher_attestation(&self) -> &ResourcePublisherAttestation {
        &self.publisher_attestation
    }

    /// Return the catalog and discard the in-memory proof wrapper.
    #[must_use]
    pub fn into_catalog(self) -> ResourceCatalog {
        self.catalog
    }
}

/// Detached, bounded HMAC-SHA256 proof that a trusted publisher emitted a
/// particular resource catalog.
///
/// The attestation is intentionally separate from [`ResourceCatalog`] so the
/// existing retained JSON shape remains backward compatible. A producer may
/// publish this envelope alongside the catalog, and a consumer should call
/// [`ResourceCatalog::admit_authenticated`] before treating the catalog as
/// authenticated. The shared key is supplied by the surrounding trusted key
/// store and is never present in this value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourcePublisherAttestation {
    /// Resource-contract schema discriminator.
    pub schema_version: u16,
    /// Publisher identity bound to the catalog.
    pub publisher: String,
    /// Trusted-key-store identifier, not the secret key material.
    pub key_id: String,
    /// Exact catalog content digest covered by the HMAC.
    pub catalog_content_digest: String,
    /// Unix epoch milliseconds at which this proof becomes valid.
    pub issued_at_ms: u64,
    /// Unix epoch milliseconds at which this proof expires.
    pub expires_at_ms: u64,
    /// Lowercase hexadecimal HMAC-SHA256 with the attestation prefix.
    pub signature: String,
}

impl ResourcePublisherAttestation {
    /// Mint an attestation for a validated catalog using a trusted shared key.
    ///
    /// The catalog's computed digest is covered even when the optional
    /// `content_digest` field is absent, so this method can authenticate a
    /// legacy-shaped catalog without weakening the proof. The caller remains
    /// responsible for distributing the resulting envelope with the catalog.
    ///
    /// # Errors
    ///
    /// Returns an error when the catalog, key ID, key, or validity window is
    /// malformed.
    pub fn mint(
        catalog: &ResourceCatalog,
        key_id: impl Into<String>,
        key: &[u8],
        issued_at_ms: u64,
        expires_at_ms: u64,
    ) -> Result<Self, ResourceValidationError> {
        catalog.validate()?;
        if key.is_empty() {
            return Err(ResourceValidationError::InvalidField(
                "resource_publisher_attestation.key",
            ));
        }

        let mut attestation = Self {
            schema_version: RESOURCE_CONTRACT_VERSION,
            publisher: catalog.publisher.clone(),
            key_id: key_id.into(),
            catalog_content_digest: catalog.computed_content_digest(),
            issued_at_ms,
            expires_at_ms,
            signature: String::new(),
        };
        attestation.validate_unsigned_shape()?;
        attestation.signature = hmac_signature(key, &attestation.signing_payload()).ok_or(
            ResourceValidationError::InvalidField("resource_publisher_attestation.key"),
        )?;
        attestation.validate_shape()?;
        Ok(attestation)
    }

    /// Return the canonical unsigned envelope covered by the HMAC.
    #[must_use]
    pub fn signing_payload(&self) -> String {
        let mut payload = String::new();
        push_canonical(&mut payload, "resource-publisher-attestation");
        push_canonical(&mut payload, &self.schema_version.to_string());
        push_canonical(&mut payload, &self.publisher);
        push_canonical(&mut payload, &self.key_id);
        push_canonical(&mut payload, &self.catalog_content_digest);
        push_canonical(&mut payload, &self.issued_at_ms.to_string());
        push_canonical(&mut payload, &self.expires_at_ms.to_string());
        payload
    }

    /// Verify this proof against a catalog and a trusted key at `now_ms`.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed or expired proofs, a publisher/digest
    /// mismatch, or a signature that does not verify under `key`.
    pub fn verify_at(
        &self,
        catalog: &ResourceCatalog,
        key: &[u8],
        now_ms: u64,
    ) -> Result<(), ResourceValidationError> {
        use hmac::{Hmac, Mac};

        catalog.validate()?;
        self.validate_shape()?;
        if key.is_empty() {
            return Err(ResourceValidationError::InvalidField(
                "resource_publisher_attestation.key",
            ));
        }
        if self.publisher != catalog.publisher {
            return Err(ResourceValidationError::InvalidRelationship(
                "resource_publisher_attestation.publisher",
            ));
        }
        if self.catalog_content_digest != catalog.computed_content_digest() {
            return Err(ResourceValidationError::FingerprintMismatch(
                "resource_publisher_attestation.catalog_content_digest",
            ));
        }
        if now_ms < self.issued_at_ms || now_ms >= self.expires_at_ms {
            return Err(ResourceValidationError::InvalidTimestamp(
                "resource_publisher_attestation.window",
            ));
        }

        let signature = self
            .signature
            .strip_prefix(RESOURCE_PUBLISHER_ATTESTATION_PREFIX)
            .and_then(decode_hex_32)
            .ok_or(ResourceValidationError::InvalidField(
                "resource_publisher_attestation.signature",
            ))?;
        let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| {
            ResourceValidationError::InvalidField("resource_publisher_attestation.key")
        })?;
        mac.update(self.signing_payload().as_bytes());
        mac.verify_slice(&signature).map_err(|_| {
            ResourceValidationError::FingerprintMismatch("resource_publisher_attestation.signature")
        })
    }

    fn validate_unsigned_shape(&self) -> Result<(), ResourceValidationError> {
        validate_version("resource_publisher_attestation", self.schema_version)?;
        validate_identifier("resource_publisher_attestation.publisher", &self.publisher)?;
        validate_identifier("resource_publisher_attestation.key_id", &self.key_id)?;
        validate_fingerprint_field(
            "resource_publisher_attestation.catalog_content_digest",
            &self.catalog_content_digest,
            RESOURCE_CATALOG_CONTENT_DIGEST_PREFIX,
        )?;
        validate_freshness(
            "resource_publisher_attestation",
            self.issued_at_ms,
            self.expires_at_ms,
        )
    }

    fn validate_shape(&self) -> Result<(), ResourceValidationError> {
        self.validate_unsigned_shape()?;
        validate_fingerprint_field(
            "resource_publisher_attestation.signature",
            &self.signature,
            RESOURCE_PUBLISHER_ATTESTATION_PREFIX,
        )
    }
}

const fn validate_version(
    component: &'static str,
    found: u16,
) -> Result<(), ResourceValidationError> {
    if found == RESOURCE_CONTRACT_VERSION {
        Ok(())
    } else {
        Err(ResourceValidationError::UnsupportedSchema { component, found })
    }
}

const fn validate_capacity(
    field: &'static str,
    len: usize,
    max: usize,
) -> Result<(), ResourceValidationError> {
    if len <= max {
        Ok(())
    } else {
        Err(ResourceValidationError::CapacityExceeded { field, max })
    }
}

fn validate_unique<T: Ord>(
    field: &'static str,
    values: &[T],
    max: usize,
) -> Result<(), ResourceValidationError> {
    validate_capacity(field, values.len(), max)?;
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(ResourceValidationError::Duplicate(field));
        }
    }
    Ok(())
}

fn validate_text(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<(), ResourceValidationError> {
    if value.len() > max {
        return Err(ResourceValidationError::FieldTooLong(field));
    }
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(ResourceValidationError::InvalidField(field));
    }
    Ok(())
}

fn validate_identifier(field: &'static str, value: &str) -> Result<(), ResourceValidationError> {
    validate_identifier_with_limit(field, value, MAX_IDENTIFIER_BYTES)
}

fn validate_identifier_with_limit(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<(), ResourceValidationError> {
    validate_text(field, value, max)?;
    if !value.is_ascii()
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains("//")
        || looks_like_secret(value)
        || value
            .split('/')
            .any(|segment| segment == "." || segment == "..")
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '-' | '_' | '.' | ':' | '/' | '@' | '+')
        })
    {
        return Err(ResourceValidationError::InvalidField(field));
    }
    Ok(())
}

fn validate_freshness(
    field: &'static str,
    observed_at_ms: u64,
    expires_at_ms: u64,
) -> Result<(), ResourceValidationError> {
    if observed_at_ms == 0 || expires_at_ms <= observed_at_ms {
        return Err(ResourceValidationError::InvalidTimestamp(field));
    }
    let ttl = expires_at_ms - observed_at_ms;
    if !(MIN_RESOURCE_TTL_MS..=MAX_RESOURCE_TTL_MS).contains(&ttl) {
        return Err(ResourceValidationError::InvalidTtl(field));
    }
    Ok(())
}

fn validate_host(host: &str) -> Result<(), ResourceValidationError> {
    validate_text("endpoint.host", host, MAX_IDENTIFIER_BYTES)?;
    if !host.is_ascii()
        || host.chars().any(|character| {
            !(character.is_ascii_alphanumeric()
                || matches!(character, '-' | '_' | '.' | ':' | '[' | ']' | '%'))
        })
    {
        return Err(ResourceValidationError::InvalidField("endpoint.host"));
    }
    Ok(())
}

fn validate_endpoint_path(path: &str) -> Result<(), ResourceValidationError> {
    validate_text("endpoint.base_path", path, MAX_ENDPOINT_PATH_BYTES)?;
    if !path.starts_with('/')
        || path.contains('?')
        || path.contains('#')
        || path.contains('\\')
        || path
            .split('/')
            .any(|segment| segment == "." || segment == "..")
        || looks_like_secret(path)
    {
        return Err(ResourceValidationError::InvalidField("endpoint.base_path"));
    }
    Ok(())
}

fn validate_secret_reference(value: &str) -> Result<(), ResourceValidationError> {
    validate_text("auth.credential_ref", value, MAX_IDENTIFIER_BYTES)?;
    if !value.is_ascii()
        || value.contains("__")
        || value.contains("//")
        || value.contains(['=', '?', '#', '\\'])
        || value.chars().any(char::is_whitespace)
    {
        return Err(ResourceValidationError::InvalidField("auth.credential_ref"));
    }
    if let Some(key) = value.strip_prefix("secret:") {
        if key.is_empty()
            || !key.chars().all(|character| {
                character.is_ascii_alphanumeric()
                    || matches!(character, '-' | '_' | '.' | ':' | '@')
            })
        {
            return Err(ResourceValidationError::InvalidField("auth.credential_ref"));
        }
        return Ok(());
    }
    let segments: Vec<_> = value.split('/').collect();
    if segments.len() < 2
        || segments.iter().any(|segment| {
            segment.is_empty()
                || *segment == "."
                || *segment == ".."
                || !segment.chars().all(|character| {
                    character.is_ascii_alphanumeric()
                        || matches!(character, '-' | '_' | '.' | ':' | '@')
                })
        })
    {
        return Err(ResourceValidationError::InvalidField("auth.credential_ref"));
    }
    Ok(())
}

fn looks_like_secret(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    if SECRET_SHAPE_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return true;
    }
    lower.find("://").is_some_and(|scheme_end| {
        let authority = &lower[scheme_end + 3..];
        authority
            .split(['/', '?', '#'])
            .next()
            .is_some_and(|authority| authority.contains('@'))
    })
}

fn validate_fingerprint_field(
    field: &'static str,
    value: &str,
    prefix: &str,
) -> Result<(), ResourceValidationError> {
    if valid_prefixed_fingerprint(value, prefix) {
        Ok(())
    } else {
        Err(ResourceValidationError::InvalidField(field))
    }
}

fn valid_prefixed_fingerprint(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn push_canonical(output: &mut String, value: &str) {
    let _ = write!(output, "{}:", value.len());
    output.push_str(value);
}

fn push_optional_canonical(output: &mut String, value: Option<&str>) {
    match value {
        Some(value) => {
            push_canonical(output, "some");
            push_canonical(output, value);
        }
        None => push_canonical(output, "none"),
    }
}

fn push_optional_number<T: ToString>(output: &mut String, value: Option<T>) {
    match value {
        Some(value) => push_optional_canonical(output, Some(&value.to_string())),
        None => push_optional_canonical(output, None),
    }
}

fn hmac_signature(key: &[u8], payload: &str) -> Option<String> {
    use hmac::{Hmac, Mac};

    let mut mac = Hmac::<Sha256>::new_from_slice(key).ok()?;
    mac.update(payload.as_bytes());
    let signature = mac.finalize().into_bytes();
    Some(format!(
        "{RESOURCE_PUBLISHER_ATTESTATION_PREFIX}{}",
        hex_encode(&signature)
    ))
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn decode_hex_32(value: &str) -> Option<[u8; 32]> {
    let bytes = value.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut decoded = [0_u8; 32];
    for (index, pair) in bytes.chunks_exact(2).enumerate() {
        decoded[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Some(decoded)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn sha256_hex(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut output = String::with_capacity(64);
    for byte in digest {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_000_000;
    const FRESH: u64 = 60_000;
    type CardMutation = (&'static str, Box<dyn Fn(&mut ResourceCard)>);

    fn failure(code: FailureCode, message: &str) -> FailureReason {
        FailureReason {
            code,
            message: message.to_owned(),
        }
    }

    fn available_health() -> HealthState {
        HealthState {
            schema_version: RESOURCE_CONTRACT_VERSION,
            status: HealthStatus::Available,
            observed_at_ms: NOW,
            expires_at_ms: NOW + FRESH,
            latency_ms: Some(12),
            failure: None,
        }
    }

    fn capability() -> ClientCapability {
        ClientCapability::new(
            "construct.ironrdp",
            "12.1.6",
            TransportProtocol::Rdp,
            "10.7",
            ClientBoundary::ShellNative,
            vec![AuthMethod::MeshIdentity],
            vec![
                ClientFeature::Display,
                ClientFeature::AudioPlayback,
                ClientFeature::KeyboardInput,
                ClientFeature::PointerInput,
                ClientFeature::Reconnect,
            ],
            ClientCapabilityLimits {
                max_width: Some(7_680),
                max_height: Some(4_320),
                max_fps: Some(120),
                max_audio_channels: Some(8),
                max_parallel_sessions: 4,
            },
            vec![ResourceActionVerb::Connect],
        )
        .expect("valid capability")
    }

    fn identity() -> ResourceIdentity {
        ResourceIdentity::new(
            ResourceClass::Desktop,
            IdentityAuthority::Mesh,
            "node/dell/browser-vm",
            vec![
                ResourceAlias {
                    kind: ResourceAliasKind::DnsName,
                    value: "browser-vm.dell.mesh".into(),
                },
                ResourceAlias {
                    kind: ResourceAliasKind::LegacyId,
                    value: "desktop:dell:browser-vm".into(),
                },
            ],
        )
        .expect("valid identity")
    }

    fn transport(capability: &ClientCapability) -> TransportCandidate {
        TransportCandidate::new(
            TransportProtocol::Rdp,
            TransportEndpoint::Network {
                host: "browser-vm.dell.mesh".into(),
                port: 3_389,
                base_path: None,
            },
            ResourceScope::Mesh,
            10,
            NOW,
            NOW + FRESH,
            available_health(),
            Some(capability.fingerprint.clone()),
        )
        .expect("valid transport")
    }

    fn valid_card() -> ResourceCard {
        let capability = capability();
        let transport = transport(&capability);
        ResourceCard {
            schema_version: RESOURCE_CONTRACT_VERSION,
            identity: identity(),
            display_name: "Dell Browser VM".into(),
            summary: Some("Guest-owned Chromium desktop".into()),
            first_seen_at_ms: NOW - FRESH,
            last_seen_at_ms: NOW,
            expires_at_ms: NOW + 2 * FRESH,
            health: available_health(),
            auth: AuthState {
                schema_version: RESOURCE_CONTRACT_VERSION,
                status: AuthStatus::Authorized,
                accepted_methods: vec![AuthMethod::MeshIdentity],
                active_method: Some(AuthMethod::MeshIdentity),
                credential_ref: None,
                updated_at_ms: NOW,
                expires_at_ms: Some(NOW + FRESH),
                failure: None,
            },
            provenance: vec![SourceProvenance {
                schema_version: RESOURCE_CONTRACT_VERSION,
                source: DiscoverySource::MeshDirectory,
                source_id: "peer/dell/browser-vm".into(),
                scope: ResourceScope::Mesh,
                trust: ProvenanceTrust::AuthenticatedMesh,
                interface: Some("nebula1".into()),
                observed_at_ms: NOW,
                expires_at_ms: NOW + FRESH,
            }],
            transports: vec![transport.clone()],
            client_capabilities: vec![capability.clone()],
            actions: vec![
                ResourceAction {
                    schema_version: RESOURCE_CONTRACT_VERSION,
                    action_id: "inspect".into(),
                    verb: ResourceActionVerb::Inspect,
                    target: ResourceActionTarget::Resource,
                    availability: ActionAvailability {
                        status: ActionAvailabilityStatus::Ready,
                        failure: None,
                    },
                    issued_at_ms: NOW,
                    expires_at_ms: NOW + FRESH,
                },
                ResourceAction {
                    schema_version: RESOURCE_CONTRACT_VERSION,
                    action_id: "connect-rdp".into(),
                    verb: ResourceActionVerb::Connect,
                    target: ResourceActionTarget::TransportClient {
                        transport_fingerprint: transport.fingerprint,
                        capability_fingerprint: capability.fingerprint,
                    },
                    availability: ActionAvailability {
                        status: ActionAvailabilityStatus::Ready,
                        failure: None,
                    },
                    issued_at_ms: NOW,
                    expires_at_ms: NOW + FRESH,
                },
            ],
            operating_roles: vec![ResourceOperatingRole::Client],
            service: None,
        }
    }

    fn valid_catalog() -> ResourceCatalog {
        ResourceCatalog {
            schema_version: RESOURCE_CONTRACT_VERSION,
            revision: "rev-42".into(),
            publisher: "seat-15".into(),
            generated_at_ms: NOW + 1_000,
            content_digest: None,
            cards: vec![valid_card()],
        }
    }

    #[test]
    fn complete_catalog_round_trips_and_admits() {
        let catalog = valid_catalog();
        catalog.validate().expect("valid catalog");
        let json = serde_json::to_string(&catalog).expect("serialize catalog");
        let decoded = ResourceCatalog::from_json(&json).expect("strictly decode catalog");
        assert_eq!(decoded, catalog);
        assert_eq!(decoded.cards[0].resource_id(), identity().resource_id);
        assert!(!json.contains("command"));
        assert!(!json.contains("password"));
        assert!(!json.contains("://"));
    }

    #[test]
    fn legacy_catalog_without_content_digest_still_decodes() {
        let catalog = valid_catalog();
        let json = serde_json::to_string(&catalog).expect("serialize legacy catalog");
        assert!(!json.contains("content_digest"));

        let decoded = ResourceCatalog::from_json(&json).expect("legacy catalog remains valid");
        assert_eq!(decoded.content_digest, None);
    }

    #[test]
    fn catalog_content_digest_is_independent_of_card_arrival_order() {
        let mut first = valid_card();
        first.display_name = "First Browser VM".into();
        first.identity = ResourceIdentity::new(
            ResourceClass::Desktop,
            IdentityAuthority::Mesh,
            "node/first/browser-vm",
            vec![],
        )
        .expect("first identity");
        let mut second = valid_card();
        second.display_name = "Second Browser VM".into();
        second.identity = ResourceIdentity::new(
            ResourceClass::Desktop,
            IdentityAuthority::Mesh,
            "node/second/browser-vm",
            vec![],
        )
        .expect("second identity");

        let mut ordered = valid_catalog();
        ordered.cards = vec![first.clone(), second.clone()];
        let attested = ordered
            .with_content_digest()
            .expect("ordered catalog attestation");

        let mut reordered = attested.clone();
        reordered.cards = vec![second, first];
        reordered.content_digest = None;
        assert_eq!(
            attested.content_digest,
            Some(reordered.computed_content_digest())
        );
        reordered
            .with_content_digest()
            .expect("reordered catalog remains admissible");
    }

    #[test]
    fn catalog_content_digest_mismatch_is_rejected() {
        let mut catalog = valid_catalog()
            .with_content_digest()
            .expect("catalog attestation");
        catalog.cards[0].display_name = "Tampered Browser VM".into();
        assert_eq!(
            catalog.validate(),
            Err(ResourceValidationError::FingerprintMismatch(
                "resource_catalog.content_digest"
            ))
        );

        let mut malformed = valid_catalog();
        malformed.content_digest = Some("catalog:v1:not-a-sha256".into());
        assert_eq!(
            malformed.validate(),
            Err(ResourceValidationError::InvalidField(
                "resource_catalog.content_digest"
            ))
        );
    }

    #[test]
    fn publisher_attestation_round_trips_and_binds_exact_catalog() {
        const KEY: &[u8] = b"publisher-attestation-test-key";

        let catalog = valid_catalog();
        let attestation = ResourcePublisherAttestation::mint(
            &catalog,
            "mesh/seat-15/catalog",
            KEY,
            NOW,
            NOW + FRESH,
        )
        .expect("publisher attestation");
        assert_eq!(
            attestation.catalog_content_digest,
            catalog.computed_content_digest()
        );
        assert!(attestation
            .signature
            .starts_with(RESOURCE_PUBLISHER_ATTESTATION_PREFIX));

        let json = serde_json::to_string(&attestation).expect("attestation JSON");
        let decoded: ResourcePublisherAttestation =
            serde_json::from_str(&json).expect("attestation JSON round trip");
        assert_eq!(decoded, attestation);
        catalog
            .validate_publisher_attestation(&decoded, KEY, NOW + 1)
            .expect("authenticated publisher admission");
    }

    #[test]
    fn publisher_attestation_rejects_tamper_wrong_key_and_expiry() {
        const KEY: &[u8] = b"publisher-attestation-test-key";

        let catalog = valid_catalog();
        let attestation = ResourcePublisherAttestation::mint(
            &catalog,
            "mesh/seat-15/catalog",
            KEY,
            NOW,
            NOW + FRESH,
        )
        .expect("publisher attestation");

        let mut tampered = catalog.clone();
        tampered.cards[0].display_name = "Forged Browser VM".into();
        assert_eq!(
            tampered.validate_publisher_attestation(&attestation, KEY, NOW + 1),
            Err(ResourceValidationError::FingerprintMismatch(
                "resource_publisher_attestation.catalog_content_digest"
            ))
        );
        let mut wrong_publisher = attestation.clone();
        wrong_publisher.publisher = "attacker".into();
        assert_eq!(
            catalog.validate_publisher_attestation(&wrong_publisher, KEY, NOW + 1),
            Err(ResourceValidationError::InvalidRelationship(
                "resource_publisher_attestation.publisher"
            ))
        );
        assert_eq!(
            catalog.validate_publisher_attestation(&attestation, b"wrong-key", NOW + 1),
            Err(ResourceValidationError::FingerprintMismatch(
                "resource_publisher_attestation.signature"
            ))
        );
        assert_eq!(
            catalog.validate_publisher_attestation(&attestation, KEY, NOW + FRESH),
            Err(ResourceValidationError::InvalidTimestamp(
                "resource_publisher_attestation.window"
            ))
        );
    }

    #[test]
    fn authenticated_catalog_requires_the_current_key_id() {
        const KEY: &[u8] = b"publisher-attestation-test-key";

        let catalog = valid_catalog();
        let legacy_key_id = ResourcePublisherAttestation::mint(
            &catalog,
            "mesh/seat-15/catalog",
            KEY,
            NOW,
            NOW + FRESH,
        )
        .expect("legacy proof");
        assert_eq!(
            catalog
                .clone()
                .admit_authenticated(legacy_key_id, KEY, NOW + 1),
            Err(ResourceValidationError::InvalidField(
                "resource_publisher_attestation.key_id"
            ))
        );

        let current = ResourcePublisherAttestation::mint(
            &catalog,
            RESOURCE_PUBLISHER_ATTESTATION_KEY_ID,
            KEY,
            NOW,
            NOW + FRESH,
        )
        .expect("current proof");
        let authenticated = catalog
            .clone()
            .admit_authenticated(current.clone(), KEY, NOW + 1)
            .expect("authenticated catalog");
        assert_eq!(authenticated.catalog(), &catalog);
        assert_eq!(authenticated.publisher_attestation(), &current);
    }

    #[test]
    fn discovery_projection_is_deterministic_and_extracts_typed_facets() {
        let mut zulu = valid_card();
        zulu.display_name = "Zulu Browser VM".into();
        zulu.identity = ResourceIdentity::new(
            ResourceClass::Desktop,
            IdentityAuthority::Mesh,
            "node/zulu/browser-vm",
            vec![],
        )
        .expect("zulu identity");

        let mut alpha = valid_card();
        alpha.display_name = "Alpha Browser VM".into();
        alpha.identity = ResourceIdentity::new(
            ResourceClass::Desktop,
            IdentityAuthority::Mesh,
            "node/alpha/browser-vm",
            vec![],
        )
        .expect("alpha identity");

        let mut catalog = valid_catalog();
        catalog.cards = vec![zulu, alpha];
        let projection = catalog
            .discovery_projection()
            .expect("valid discovery projection");
        assert_eq!(
            projection
                .entries
                .iter()
                .map(|entry| entry.display_name.as_str())
                .collect::<Vec<_>>(),
            ["Alpha Browser VM", "Zulu Browser VM"]
        );

        let mut reordered = catalog.clone();
        reordered.cards.reverse();
        assert_eq!(
            reordered
                .discovery_projection()
                .expect("reordered projection"),
            projection
        );

        let entry = &projection.entries[0];
        assert_eq!(entry.class, ResourceClass::Desktop);
        assert_eq!(entry.health_status, HealthStatus::Available);
        assert_eq!(entry.auth_status, AuthStatus::Authorized);
        assert_eq!(
            entry.discovery_sources,
            vec![DiscoverySource::MeshDirectory]
        );
        assert_eq!(entry.reachability_scopes, vec![ResourceScope::Mesh]);
        assert_eq!(entry.transport_protocols, vec![TransportProtocol::Rdp]);
        assert_eq!(
            entry.ready_actions,
            vec![ResourceActionVerb::Inspect, ResourceActionVerb::Connect]
        );
        assert!(entry.service_category.is_none());
        assert_eq!(projection.catalog_content_digest, catalog.content_digest);

        let json = serde_json::to_string(&projection).expect("projection JSON");
        assert!(!json.contains("endpoint"));
        let decoded: ResourceDiscoveryProjection =
            serde_json::from_str(&json).expect("projection JSON round trip");
        decoded.validate().expect("decoded projection validates");
        assert_eq!(decoded, projection);
    }

    #[test]
    fn discovery_projection_rejects_duplicate_and_unsorted_entries() {
        let projection = valid_catalog()
            .discovery_projection()
            .expect("valid discovery projection");

        let mut duplicate = projection.clone();
        duplicate.entries.push(duplicate.entries[0].clone());
        assert_eq!(
            duplicate.validate(),
            Err(ResourceValidationError::Duplicate(
                "resource_discovery_projection.entries"
            ))
        );

        let mut unsorted = projection;
        let mut later = unsorted.entries[0].clone();
        later.display_name = "000 Earlier Name".into();
        later.resource_id = ResourceIdentity::new(
            ResourceClass::Desktop,
            IdentityAuthority::Mesh,
            "node/earlier/browser-vm",
            vec![],
        )
        .expect("earlier identity")
        .resource_id;
        unsorted.entries.push(later);
        assert_eq!(
            unsorted.validate(),
            Err(ResourceValidationError::InvalidRelationship(
                "resource_discovery_projection.entry_order"
            ))
        );
    }

    #[test]
    fn service_cards_are_one_per_service_and_never_carry_configuration_values() {
        let mut card = valid_card();
        card.identity = ResourceIdentity::new(
            ResourceClass::Service,
            IdentityAuthority::Provider,
            "provider/acme/sip",
            vec![],
        )
        .expect("service identity");
        card.display_name = "Acme SIP ITSP".into();
        card.operating_roles = vec![
            ResourceOperatingRole::Client,
            ResourceOperatingRole::Loader,
            ResourceOperatingRole::Host,
        ];
        card.service = Some(ServiceInterface {
            service_kind: "sip-itsp".into(),
            provider_id: Some("acme".into()),
            category: ServiceCategory::Communications,
            lifecycle: ServiceLifecycleStatus::Unconfigured,
            configuration_fields: vec![
                ServiceConfigurationField {
                    key: "endpoint".into(),
                    label: "SIP endpoint".into(),
                    kind: ServiceConfigurationFieldKind::Endpoint,
                    required: true,
                    choices: vec![],
                },
                ServiceConfigurationField {
                    key: "credential".into(),
                    label: "Credential".into(),
                    kind: ServiceConfigurationFieldKind::Secret,
                    required: true,
                    choices: vec![],
                },
            ],
            stack: LocalServiceStack {
                tier: ServiceStackTier::PlatformServices,
                plane: ServiceStackPlane::Coordination,
                external: true,
                adapter_worker: Some("sip-gateway".into()),
                bus_topics: vec![
                    "state/services/sip".into(),
                    "action/service/configure".into(),
                ],
                transport: Some("SIP over TLS".into()),
                credential_ref: Some("service/acme/sip".into()),
                hosting_nodes: vec![],
                dependencies: vec![],
            },
        });
        card.transports.clear();
        card.client_capabilities.clear();
        card.actions = vec![ResourceAction {
            schema_version: RESOURCE_CONTRACT_VERSION,
            action_id: "configure".into(),
            verb: ResourceActionVerb::Configure,
            target: ResourceActionTarget::Resource,
            availability: ActionAvailability {
                status: ActionAvailabilityStatus::Ready,
                failure: None,
            },
            issued_at_ms: NOW,
            expires_at_ms: NOW + FRESH,
        }];

        card.validate().expect("first-class service card");
        let encoded = serde_json::to_string(&card).expect("encode service card");
        assert!(encoded.contains("sip-itsp"));
        assert!(!encoded.contains("secret_value"));
        assert!(!encoded.contains("password"));

        let mut vpn = card.clone();
        vpn.identity = ResourceIdentity::new(
            ResourceClass::Service,
            IdentityAuthority::Provider,
            "provider/acme/vpn",
            vec![],
        )
        .expect("vpn identity");
        vpn.service.as_mut().expect("service").service_kind = "vpn".into();
        assert_ne!(card.resource_id(), vpn.resource_id());
    }

    #[test]
    fn configuration_schema_rejects_duplicate_fields_and_choice_mismatches() {
        let field = ServiceConfigurationField {
            key: "region".into(),
            label: "Region".into(),
            kind: ServiceConfigurationFieldKind::Choice,
            required: true,
            choices: vec!["east".into(), "west".into()],
        };
        let mut service = ServiceInterface {
            service_kind: "vpn".into(),
            provider_id: Some("acme".into()),
            category: ServiceCategory::Network,
            lifecycle: ServiceLifecycleStatus::Healthy,
            configuration_fields: vec![field.clone(), field],
            stack: LocalServiceStack {
                tier: ServiceStackTier::PlatformServices,
                plane: ServiceStackPlane::Data,
                external: true,
                adapter_worker: Some("vpn-gateway".into()),
                bus_topics: vec![],
                transport: Some("WireGuard".into()),
                credential_ref: Some("service/acme/vpn".into()),
                hosting_nodes: vec![],
                dependencies: vec![],
            },
        };
        assert_eq!(
            service.validate(),
            Err(ResourceValidationError::Duplicate(
                "service.configuration_fields"
            ))
        );
        service.configuration_fields.truncate(1);
        service.configuration_fields[0].kind = ServiceConfigurationFieldKind::Text;
        assert_eq!(
            service.validate(),
            Err(ResourceValidationError::InvalidRelationship(
                "service.configuration.non_choice_values"
            ))
        );
    }

    #[test]
    fn identity_id_is_stable_across_alias_order_and_churn() {
        let original = identity();
        let mut reordered = original.clone();
        reordered.aliases.reverse();
        reordered.aliases.push(ResourceAlias {
            kind: ResourceAliasKind::ProviderId,
            value: "libvirt:browser-vm".into(),
        });
        assert_eq!(
            original.computed_resource_id(),
            reordered.computed_resource_id()
        );
        assert_eq!(original.resource_id.len(), RESOURCE_ID_PREFIX.len() + 64);

        let mut different = original.clone();
        different.canonical_key = "node/dell/office-vm".into();
        assert_ne!(
            original.computed_resource_id(),
            different.computed_resource_id()
        );

        let mut forged = original;
        forged.resource_id = format!("{RESOURCE_ID_PREFIX}{}", "0".repeat(64));
        assert_eq!(
            forged.validate(),
            Err(ResourceValidationError::FingerprintMismatch(
                "identity.resource_id"
            ))
        );
    }

    #[test]
    fn capability_fingerprint_is_set_order_independent_and_semantic() {
        let original = capability();
        let mut reordered = original.clone();
        reordered.auth_methods.reverse();
        reordered.features.reverse();
        reordered.safe_actions.reverse();
        assert_eq!(
            original.computed_fingerprint(),
            reordered.computed_fingerprint()
        );

        let mut changed = original.clone();
        changed.limits.max_fps = Some(60);
        assert_ne!(
            original.computed_fingerprint(),
            changed.computed_fingerprint()
        );

        let mut forged = original;
        forged.fingerprint = format!("{CAPABILITY_FINGERPRINT_PREFIX}{}", "f".repeat(64));
        assert_eq!(
            forged.validate(),
            Err(ResourceValidationError::FingerprintMismatch(
                "capability.fingerprint"
            ))
        );
    }

    #[test]
    fn capability_registry_lookup_is_deterministic_and_typed() {
        let first = capability();
        let mut second = capability();
        second.adapter_id = "construct.ironrdp.alt".into();
        second.fingerprint = second.computed_fingerprint();
        second.validate().expect("second capability remains valid");

        let forward = ClientCapabilityRegistry::admitted(vec![first.clone(), second.clone()])
            .expect("forward registry admission");
        let reverse = ClientCapabilityRegistry::admitted(vec![second.clone(), first.clone()])
            .expect("reverse registry admission");

        assert_eq!(forward.len(), 2);
        assert!(!forward.is_empty());
        assert_eq!(
            forward.fingerprints().collect::<Vec<_>>(),
            reverse.fingerprints().collect::<Vec<_>>()
        );
        assert_eq!(forward.lookup(&first.fingerprint), Some(&first));
        assert_eq!(forward.lookup(&second.fingerprint), Some(&second));
        assert_eq!(
            forward
                .capabilities()
                .map(|entry| entry.fingerprint.as_str())
                .collect::<Vec<_>>(),
            forward.fingerprints().collect::<Vec<_>>()
        );
        assert!(forward.lookup("not-a-capability-fingerprint").is_none());
    }

    #[test]
    fn capability_registry_rejects_duplicate_invalid_and_oversized_input() {
        let duplicate = ClientCapabilityRegistry::admitted(vec![capability(), capability()]);
        assert_eq!(
            duplicate,
            Err(ResourceValidationError::Duplicate(
                "client_capability_registry.entries"
            ))
        );

        let mut invalid = capability();
        invalid.fingerprint = format!("{CAPABILITY_FINGERPRINT_PREFIX}{}", "0".repeat(64));
        assert_eq!(
            ClientCapabilityRegistry::admitted(vec![invalid]),
            Err(ResourceValidationError::FingerprintMismatch(
                "capability.fingerprint"
            ))
        );

        let mut oversized = Vec::new();
        for index in 0..=MAX_CAPABILITIES {
            let mut entry = capability();
            entry.adapter_id = format!("construct.ironrdp.{index}");
            entry.fingerprint = entry.computed_fingerprint();
            oversized.push(entry);
        }
        assert_eq!(
            ClientCapabilityRegistry::admitted(oversized),
            Err(ResourceValidationError::CapacityExceeded {
                field: "client_capability_registry.entries",
                max: MAX_CAPABILITIES
            })
        );
    }

    #[test]
    fn capability_registry_has_no_arbitrary_command_path_or_url_wire_capability() {
        let encoded = serde_json::to_value(capability()).expect("capability JSON");
        let object = encoded.as_object().expect("capability object");
        assert!(!object.contains_key("command"));
        assert!(!object.contains_key("path"));
        assert!(!object.contains_key("url"));

        for field in ["command", "path", "url"] {
            let mut forged = encoded.clone();
            forged
                .as_object_mut()
                .expect("capability object")
                .insert(field.into(), serde_json::Value::String("arbitrary".into()));
            assert!(
                serde_json::from_value::<ClientCapability>(forged).is_err(),
                "accepted arbitrary capability field {field:?}"
            );
        }

        let registry = ClientCapabilityRegistry::admitted(vec![capability()])
            .expect("typed capability remains admissible");
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn transport_fingerprint_ignores_runtime_health_and_client_selection() {
        let cap = capability();
        let original = transport(&cap);
        let mut changed_runtime = original.clone();
        changed_runtime.priority = 999;
        changed_runtime.health.latency_ms = Some(999);
        changed_runtime.client_capability_fingerprint = None;
        assert_eq!(
            original.computed_fingerprint(),
            changed_runtime.computed_fingerprint()
        );

        let mut changed_endpoint = original.clone();
        changed_endpoint.endpoint = TransportEndpoint::Network {
            host: "browser-vm.dell.mesh".into(),
            port: 3_390,
            base_path: None,
        };
        assert_ne!(
            original.computed_fingerprint(),
            changed_endpoint.computed_fingerprint()
        );
    }

    #[test]
    fn strict_serde_rejects_unknown_fields_and_variants() {
        let mut value = serde_json::to_value(valid_catalog()).expect("catalog value");
        value
            .as_object_mut()
            .expect("catalog object")
            .insert("future_field".into(), serde_json::Value::Bool(true));
        assert!(serde_json::from_value::<ResourceCatalog>(value).is_err());

        let endpoint = r#"{
            "kind":"network","host":"example.test","port":3389,
            "base_path":null,"command":"calc.exe"
        }"#;
        assert!(serde_json::from_str::<TransportEndpoint>(endpoint).is_err());
        assert!(serde_json::from_str::<TransportProtocol>(r#""telnet""#).is_err());
        assert!(serde_json::from_str::<ResourceCatalog>("{}").is_err());
    }

    #[test]
    fn every_nested_contract_version_is_fail_closed() {
        let mutations: Vec<CardMutation> = vec![
            ("resource_card", Box::new(|card| card.schema_version = 2)),
            (
                "resource_identity",
                Box::new(|card| card.identity.schema_version = 2),
            ),
            (
                "health_state",
                Box::new(|card| card.health.schema_version = 2),
            ),
            ("auth_state", Box::new(|card| card.auth.schema_version = 2)),
            (
                "source_provenance",
                Box::new(|card| card.provenance[0].schema_version = 2),
            ),
            (
                "transport_candidate",
                Box::new(|card| card.transports[0].schema_version = 2),
            ),
            (
                "client_capability",
                Box::new(|card| card.client_capabilities[0].schema_version = 2),
            ),
            (
                "resource_action",
                Box::new(|card| card.actions[0].schema_version = 2),
            ),
        ];
        for (component, mutate) in mutations {
            let mut card = valid_card();
            mutate(&mut card);
            assert_eq!(
                card.validate(),
                Err(ResourceValidationError::UnsupportedSchema {
                    component,
                    found: 2
                })
            );
        }
    }

    #[test]
    fn freshness_intervals_are_positive_and_bounded() {
        let mut health = available_health();
        health.expires_at_ms = health.observed_at_ms;
        assert_eq!(
            health.validate(),
            Err(ResourceValidationError::InvalidTimestamp("health_state"))
        );
        health.expires_at_ms = health.observed_at_ms + MIN_RESOURCE_TTL_MS - 1;
        assert_eq!(
            health.validate(),
            Err(ResourceValidationError::InvalidTtl("health_state"))
        );
        health.expires_at_ms = health.observed_at_ms + MAX_RESOURCE_TTL_MS + 1;
        assert_eq!(
            health.validate(),
            Err(ResourceValidationError::InvalidTtl("health_state"))
        );

        let mut card = valid_card();
        card.first_seen_at_ms = card.last_seen_at_ms + 1;
        assert_eq!(
            card.validate(),
            Err(ResourceValidationError::InvalidTimestamp(
                "resource_card.freshness"
            ))
        );
    }

    #[test]
    fn degraded_states_require_safe_failure_reasons() {
        let mut health = available_health();
        health.status = HealthStatus::Unavailable;
        assert_eq!(
            health.validate(),
            Err(ResourceValidationError::MissingFailure("health"))
        );
        health.failure = Some(failure(
            FailureCode::AuthenticationFailed,
            "Authorization: Bearer hunter2",
        ));
        assert_eq!(
            health.validate(),
            Err(ResourceValidationError::SecretShapedValue(
                "health.failure.message"
            ))
        );
        health.failure = Some(failure(
            FailureCode::AuthenticationFailed,
            "credential validation failed",
        ));
        assert!(health.validate().is_ok());

        let mut healthy = available_health();
        healthy.failure = Some(failure(FailureCode::Other, "unexpected warning"));
        assert_eq!(
            healthy.validate(),
            Err(ResourceValidationError::UnexpectedFailure("health"))
        );
    }

    #[test]
    fn secret_references_accept_only_opaque_store_names() {
        for valid in [
            "media/jellyfin/shared-readonly",
            "sip/bob@corp",
            "xcp/dom0.lab.local:22",
            "secret:xcp-host",
        ] {
            let encoded = serde_json::to_string(valid).expect("reference JSON");
            let reference: SecretReference =
                serde_json::from_str(&encoded).expect("valid secret reference");
            assert_eq!(reference.as_str(), valid);
        }
        for invalid in [
            "",
            "plaintext-token",
            "media/../token",
            "media//token",
            "media/token=value",
            "https://user:pass@example.test",
            "media/private key",
        ] {
            let encoded = serde_json::to_string(invalid).expect("reference JSON");
            assert!(
                serde_json::from_str::<SecretReference>(&encoded).is_err(),
                "accepted invalid secret reference {invalid:?}"
            );
        }
    }

    #[test]
    fn endpoints_reject_url_userinfo_queries_and_path_traversal() {
        for host in ["user@example.test", "https://example.test", "host/path"] {
            let candidate = TransportEndpoint::Network {
                host: host.into(),
                port: 22,
                base_path: None,
            };
            assert!(candidate.validate().is_err(), "accepted host {host:?}");
        }
        for path in ["relative", "/api?token=abc", "/api/../admin"] {
            let candidate = TransportEndpoint::Network {
                host: "example.test".into(),
                port: 8_096,
                base_path: Some(path.into()),
            };
            assert!(candidate.validate().is_err(), "accepted path {path:?}");
        }
    }

    #[test]
    fn auth_state_is_cross_field_strict_and_never_needs_plaintext() {
        let pending = AuthState {
            schema_version: RESOURCE_CONTRACT_VERSION,
            status: AuthStatus::Pending,
            accepted_methods: vec![AuthMethod::PairingCode],
            active_method: Some(AuthMethod::PairingCode),
            credential_ref: None,
            updated_at_ms: NOW,
            expires_at_ms: Some(NOW + FRESH),
            failure: None,
        };
        assert!(pending.validate().is_ok());

        let mut invalid = pending;
        invalid.expires_at_ms = None;
        assert_eq!(
            invalid.validate(),
            Err(ResourceValidationError::InvalidRelationship(
                "auth.state_fields"
            ))
        );

        let authorized = AuthState {
            schema_version: RESOURCE_CONTRACT_VERSION,
            status: AuthStatus::Authorized,
            accepted_methods: vec![AuthMethod::BearerToken],
            active_method: Some(AuthMethod::BearerToken),
            credential_ref: Some(
                SecretReference::new("media/jellyfin/shared-readonly").expect("opaque reference"),
            ),
            updated_at_ms: NOW,
            expires_at_ms: Some(NOW + FRESH),
            failure: None,
        };
        assert!(authorized.validate().is_ok());
        let mut missing_reference = authorized.clone();
        missing_reference.credential_ref = None;
        assert_eq!(
            missing_reference.validate(),
            Err(ResourceValidationError::InvalidRelationship(
                "auth.state_fields"
            ))
        );
        let body = serde_json::to_string(&authorized).expect("auth JSON");
        assert!(!body.contains("bearer "));
        assert!(!body.contains("token="));
    }

    #[test]
    fn card_rejects_duplicate_and_dangling_references() {
        let mut duplicate = valid_card();
        duplicate.transports.push(duplicate.transports[0].clone());
        assert_eq!(
            duplicate.validate(),
            Err(ResourceValidationError::Duplicate(
                "resource_card.transports"
            ))
        );

        let mut dangling = valid_card();
        let replacement = format!("{CAPABILITY_FINGERPRINT_PREFIX}{}", "0".repeat(64));
        dangling.transports[0].client_capability_fingerprint = Some(replacement);
        assert_eq!(
            dangling.validate(),
            Err(ResourceValidationError::InvalidRelationship(
                "transport.capability_reference"
            ))
        );

        let mut catalog = valid_catalog();
        catalog.cards.push(catalog.cards[0].clone());
        assert_eq!(
            catalog.validate(),
            Err(ResourceValidationError::Duplicate("resource_catalog.cards"))
        );
    }

    #[test]
    fn catalog_wire_boundary_rejects_identity_collisions_and_bad_provenance() {
        let mut merged = valid_catalog();
        let mut second_source = merged.cards[0].provenance[0].clone();
        second_source.source = DiscoverySource::Manual;
        second_source.source_id = "operator/seat-15".into();
        second_source.scope = ResourceScope::Mesh;
        second_source.trust = ProvenanceTrust::OperatorDeclared;
        second_source.interface = None;
        merged.cards[0].provenance.push(second_source);

        let admitted = ResourceCatalog::from_json(
            &serde_json::to_string(&merged).expect("serialize multi-source catalog"),
        )
        .expect("distinct source observations remain visible on one card");
        assert_eq!(admitted.cards.len(), 1);
        assert_eq!(admitted.cards[0].provenance.len(), 2);
        assert_eq!(
            admitted.cards[0]
                .provenance
                .iter()
                .map(|source| source.source)
                .collect::<Vec<_>>(),
            vec![DiscoverySource::MeshDirectory, DiscoverySource::Manual]
        );

        let mut collision = serde_json::to_value(valid_catalog()).expect("catalog value");
        let cards = collision
            .get_mut("cards")
            .and_then(serde_json::Value::as_array_mut)
            .expect("catalog cards array");
        let duplicate_card = cards[0].clone();
        cards.push(duplicate_card);
        let collision_body = serde_json::to_string(&collision).expect("collision JSON");
        assert!(matches!(
            ResourceCatalog::from_json(&collision_body),
            Err(ResourceCatalogDecodeError::Validation(
                ResourceValidationError::Duplicate("resource_catalog.cards")
            ))
        ));

        let mut malformed_provenance =
            serde_json::to_value(valid_catalog()).expect("catalog value");
        malformed_provenance["cards"][0]["provenance"][0]["scope"] =
            serde_json::Value::String("trusted_lan".into());
        let malformed_body =
            serde_json::to_string(&malformed_provenance).expect("malformed provenance JSON");
        assert!(matches!(
            ResourceCatalog::from_json(&malformed_body),
            Err(ResourceCatalogDecodeError::Validation(
                ResourceValidationError::InvalidRelationship("provenance.source_scope_trust",)
            ))
        ));

        let mut unknown_kind = serde_json::to_value(valid_catalog()).expect("catalog value");
        unknown_kind["cards"][0]["identity"]["class"] =
            serde_json::Value::String("future_resource_kind".into());
        let unknown_kind_body = serde_json::to_string(&unknown_kind).expect("unknown kind JSON");
        assert!(matches!(
            ResourceCatalog::from_json(&unknown_kind_body),
            Err(ResourceCatalogDecodeError::Json(_))
        ));
    }

    #[test]
    fn actions_are_bounded_typed_and_reference_admitted_pairs() {
        let mut wrong_target = valid_card();
        wrong_target.actions[1].target = ResourceActionTarget::Resource;
        assert_eq!(
            wrong_target.validate(),
            Err(ResourceValidationError::InvalidRelationship(
                "action.verb_target"
            ))
        );

        let mut gated = valid_card();
        gated.auth.status = AuthStatus::Required;
        gated.auth.active_method = None;
        gated.auth.expires_at_ms = None;
        assert_eq!(
            gated.validate(),
            Err(ResourceValidationError::InvalidRelationship(
                "action.ready_connect_state"
            ))
        );
        gated.actions[1].availability = ActionAvailability {
            status: ActionAvailabilityStatus::RequiresAuth,
            failure: Some(failure(
                FailureCode::AuthenticationRequired,
                "mesh authorization required",
            )),
        };
        assert!(gated.validate().is_ok());

        let mut too_many = valid_card();
        too_many.actions.clear();
        for index in 0..=MAX_ACTIONS {
            too_many.actions.push(ResourceAction {
                schema_version: RESOURCE_CONTRACT_VERSION,
                action_id: format!("inspect-{index}"),
                verb: ResourceActionVerb::Inspect,
                target: ResourceActionTarget::Resource,
                availability: ActionAvailability {
                    status: ActionAvailabilityStatus::Ready,
                    failure: None,
                },
                issued_at_ms: NOW,
                expires_at_ms: NOW + FRESH,
            });
        }
        assert_eq!(
            too_many.validate(),
            Err(ResourceValidationError::CapacityExceeded {
                field: "resource_card.actions",
                max: MAX_ACTIONS
            })
        );
    }

    #[test]
    fn capability_limits_require_matching_typed_features() {
        let mut no_display = capability();
        no_display
            .features
            .retain(|feature| *feature != ClientFeature::Display);
        no_display.fingerprint = no_display.computed_fingerprint();
        assert_eq!(
            no_display.validate(),
            Err(ResourceValidationError::InvalidRelationship(
                "capability.limits.require_display_feature"
            ))
        );

        let duplicate = ClientCapability::new(
            "construct.ironrdp",
            "12.1.6",
            TransportProtocol::Rdp,
            "10.7",
            ClientBoundary::ShellNative,
            vec![AuthMethod::MeshIdentity, AuthMethod::MeshIdentity],
            vec![ClientFeature::Display],
            ClientCapabilityLimits {
                max_width: Some(1_920),
                max_height: Some(1_080),
                max_fps: Some(60),
                max_audio_channels: None,
                max_parallel_sessions: 1,
            },
            vec![ResourceActionVerb::Connect],
        );
        assert_eq!(
            duplicate,
            Err(ResourceValidationError::Duplicate(
                "capability.auth_methods"
            ))
        );
    }

    #[test]
    fn catalog_decode_bounds_input_before_serde() {
        let oversized = "x".repeat(MAX_RESOURCE_CATALOG_BYTES + 1);
        assert!(matches!(
            ResourceCatalog::from_json(&oversized),
            Err(ResourceCatalogDecodeError::BodyTooLarge {
                bytes,
                max: MAX_RESOURCE_CATALOG_BYTES
            }) if bytes == MAX_RESOURCE_CATALOG_BYTES + 1
        ));
    }

    #[test]
    fn catalog_rejects_future_actions_and_dangling_gateway_cards() {
        let mut future = valid_catalog();
        future.cards[0].actions[0].issued_at_ms = future.generated_at_ms + 1;
        assert_eq!(
            future.validate(),
            Err(ResourceValidationError::InvalidRelationship(
                "resource_catalog.card_generated_at"
            ))
        );

        let mut dangling = valid_catalog();
        let card = &mut dangling.cards[0];
        let gateway_resource_id = format!("{RESOURCE_ID_PREFIX}{}", "0".repeat(64));
        card.transports[0].endpoint = TransportEndpoint::Gateway {
            gateway_resource_id,
            target_id: "desktop/browser-vm".into(),
        };
        card.transports[0].scope = ResourceScope::Gateway;
        card.transports[0].fingerprint = card.transports[0].computed_fingerprint();
        let transport_fingerprint = card.transports[0].fingerprint.clone();
        assert!(matches!(
            card.actions[1].target,
            ResourceActionTarget::TransportClient { .. }
        ));
        if let ResourceActionTarget::TransportClient {
            transport_fingerprint: action_transport,
            ..
        } = &mut card.actions[1].target
        {
            *action_transport = transport_fingerprint;
        }
        card.validate().expect("gateway card is locally valid");
        assert_eq!(
            dangling.validate(),
            Err(ResourceValidationError::InvalidRelationship(
                "transport.gateway_resource_reference"
            ))
        );
    }

    #[test]
    fn source_provenance_requires_explicit_scope_and_trust() {
        let mut source = valid_card().provenance.remove(0);
        source.source = DiscoverySource::MdnsDnsSd;
        source.scope = ResourceScope::TrustedLan;
        source.trust = ProvenanceTrust::ObservedLan;
        source.interface = Some("enp1s0".into());
        assert!(source.validate().is_ok());

        source.interface = None;
        assert_eq!(
            source.validate(),
            Err(ResourceValidationError::InvalidRelationship(
                "provenance.source_scope_trust"
            ))
        );

        source.interface = Some("enp1s0".into());
        source.source_id = "password:plaintext".into();
        assert_eq!(
            source.validate(),
            Err(ResourceValidationError::InvalidField(
                "provenance.source_id"
            ))
        );
    }
}
