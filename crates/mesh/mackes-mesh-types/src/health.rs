//! Canonical System and Mesh Health contracts.
//!
//! The daemon owns evaluation and remediation authorization. Desktop surfaces
//! only render these records and submit allowlisted [`HealthActionRequest`]s.

#![allow(
    missing_docs,
    reason = "field names are the documented versioned health wire contract"
)]
#![allow(
    clippy::items_after_statements,
    clippy::missing_const_for_fn,
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::redundant_closure_for_method_calls,
    clippy::too_long_first_doc_paragraph,
    clippy::match_same_arms,
    clippy::match_like_matches_macro,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "health wire contracts preserve their established public shape"
)]

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use std::fmt;

/// Current wire schema.
pub const HEALTH_SCHEMA_VERSION: u16 = 1;
/// Current typed KIRON health-alert wire schema.
pub const HEALTH_KIRON_SCHEMA_VERSION: u16 = 1;
/// The only schema currently admitted for node availability intent records.
pub const NODE_AVAILABILITY_INTENT_SCHEMA_VERSION: u16 = 1;
/// The only schema currently admitted for an expected-return declaration.
pub const EXPECTED_RETURN_SCHEMA_VERSION: u16 = 1;

/// Keep rendered elapsed durations finite even when a hostile timestamp spans
/// an implausible number of years. The formatter still uses the surveyed
/// duration grammar after clamping to this bound.
pub const MAX_HEALTH_DURATION_MS: u64 = 100 * 365 * 24 * 60 * 60 * 1_000;
/// Maximum validity window for one node-declared availability intent.
pub const MAX_NODE_AVAILABILITY_INTENT_TTL_MS: u64 = 30 * 24 * 60 * 60 * 1_000;
/// Maximum bytes for stable availability identities and event/source names.
pub const MAX_NODE_AVAILABILITY_ID_BYTES: usize = 128;
/// Maximum bytes for an operator-readable availability reason.
pub const MAX_NODE_AVAILABILITY_REASON_BYTES: usize = 256;
/// Maximum bytes for a connectivity interface identity.
pub const MAX_NODE_CONNECTIVITY_INTERFACE_BYTES: usize = 64;
/// Maximum lifetime admitted for one node-health publication.
pub const MAX_NODE_HEALTH_PUBLICATION_TTL_MS: u64 = 10 * 60 * 1_000;
/// Maximum age of resolved condition history admitted into a publication.
///
/// This enforces the fleet privacy epoch at the shared contract boundary so an
/// offline or hostile publisher cannot restore expired history by republishing
/// it inside an otherwise fresh node-health record.
pub const MAX_HEALTH_HISTORY_RETENTION_MS: u64 = 6 * 60 * 60 * 1_000;
/// Maximum conditions retained in either publication lifecycle lane.
pub const MAX_NODE_HEALTH_CONDITIONS: usize = 256;
/// Maximum remediations attached to one condition.
pub const MAX_HEALTH_REMEDIATIONS: usize = 16;
/// Maximum structured facts attached to one evidence record.
pub const MAX_HEALTH_EVIDENCE_FACTS: usize = 32;
/// Maximum aggregate UTF-8 bytes admitted for one evidence record.
pub const MAX_HEALTH_EVIDENCE_BYTES: usize = 16 * 1_024;
/// Maximum bytes for health contract identities and routing labels.
pub const MAX_HEALTH_ID_BYTES: usize = 128;
/// Maximum bytes for one operator-readable health string.
pub const MAX_HEALTH_TEXT_BYTES: usize = 1_024;

/// Format elapsed time using the locale-independent health contract.
///
/// Intervals below one hour use readable minutes and seconds. Intervals from
/// one hour through exactly 24 hours use `HH:MM:SS`; day notation begins only
/// after 24 hours. Sub-second precision is intentionally discarded because
/// health history is an elapsed-time presentation, not a wall-clock display.
#[must_use]
pub fn format_health_duration_ms(milliseconds: u64) -> String {
    let milliseconds = milliseconds.min(MAX_HEALTH_DURATION_MS);
    const HOUR_MS: u64 = 60 * 60 * 1_000;
    const DAY_MS: u64 = 24 * HOUR_MS;

    if milliseconds < HOUR_MS {
        let seconds = milliseconds / 1_000;
        let minutes = seconds / 60;
        let remainder = seconds % 60;
        return if minutes == 0 {
            format!("{remainder}s")
        } else {
            format!("{minutes}m {remainder:02}s")
        };
    }

    let seconds = milliseconds / 1_000;
    if milliseconds <= DAY_MS {
        let hours = seconds / 3_600;
        let minutes = (seconds % 3_600) / 60;
        let seconds = seconds % 60;
        return format!("{hours:02}:{minutes:02}:{seconds:02}");
    }

    let days = seconds / 86_400;
    let remainder = seconds % 86_400;
    let hours = remainder / 3_600;
    let minutes = (remainder % 3_600) / 60;
    let seconds = remainder % 60;
    format!("{days}d {hours:02}:{minutes:02}:{seconds:02}")
}

/// Compatibility spelling for consumers that already use the health modal's
/// duration terminology. It delegates to the shared formatter above.
#[must_use]
pub fn format_duration_ms(milliseconds: u64) -> String {
    format_health_duration_ms(milliseconds)
}

/// Closed node lifecycle states. `Unknown` is an explicit producer report; it
/// is never synthesized as a planned absence by a consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeAvailabilityState {
    Awake,
    Sleeping,
    ShuttingDown,
    ShutDown,
    ScheduledReboot,
    Rebooting,
    Maintenance,
    AdapterMigration,
    Returned,
    Unknown,
}

impl NodeAvailabilityState {
    /// Whether this state explicitly declares an expected absence.
    #[must_use]
    pub const fn expects_return(self) -> bool {
        matches!(
            self,
            Self::Sleeping
                | Self::ShuttingDown
                | Self::ShutDown
                | Self::ScheduledReboot
                | Self::Rebooting
                | Self::Maintenance
                | Self::AdapterMigration
        )
    }
}

/// Device class used by the later health policy layer for escalation defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeDeviceClass {
    Desktop,
    Laptop,
    WirelessDevice,
    Server,
    Lighthouse,
    Unknown,
}

/// Closed physical/overlay connection vocabulary. Raw addresses and
/// credentials do not belong in the health intent contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeConnectionType {
    Ethernet,
    Wifi,
    Cellular,
    Mesh,
    Disconnected,
    Unknown,
}

/// Address-family summary retained with a connectivity transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeAddressFamily {
    None,
    Ipv4,
    Ipv6,
    DualStack,
    Unknown,
}

/// Bounded, credential-free connectivity facts before or after an adapter
/// migration. It deliberately carries no address, URL, path, or raw provider
/// output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeConnectivitySummary {
    pub connection_type: NodeConnectionType,
    pub interface_id: Option<String>,
    pub address_family: NodeAddressFamily,
    pub reachable: bool,
}

impl NodeConnectivitySummary {
    /// Validate a connectivity summary before accepting it into an intent.
    pub fn validate(&self) -> Result<(), NodeAvailabilityValidationError> {
        if let Some(interface_id) = &self.interface_id {
            validate_identifier(
                "connectivity.interface_id",
                interface_id,
                MAX_NODE_CONNECTIVITY_INTERFACE_BYTES,
            )?;
        }
        if self.connection_type == NodeConnectionType::Disconnected {
            if self.interface_id.is_some()
                || self.address_family != NodeAddressFamily::None
                || self.reachable
            {
                return Err(NodeAvailabilityValidationError::Contradictory(
                    "disconnected connectivity carries live details",
                ));
            }
        } else if self.reachable && self.address_family == NodeAddressFamily::None {
            return Err(NodeAvailabilityValidationError::Contradictory(
                "reachable connectivity has no address family",
            ));
        }
        Ok(())
    }
}

/// A node-declared time at which an expected absence should have ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedReturn {
    pub schema_version: u16,
    pub expected_at_ms: u64,
}

impl ExpectedReturn {
    /// Construct a versioned expected-return record without inventing any
    /// policy default. Callers still need to validate the containing intent.
    #[must_use]
    pub const fn new(expected_at_ms: u64) -> Self {
        Self {
            schema_version: EXPECTED_RETURN_SCHEMA_VERSION,
            expected_at_ms,
        }
    }

    fn validate(
        &self,
        observed_at_ms: u64,
        expires_at_ms: u64,
    ) -> Result<(), NodeAvailabilityValidationError> {
        if self.schema_version != EXPECTED_RETURN_SCHEMA_VERSION {
            return Err(
                NodeAvailabilityValidationError::UnsupportedExpectedReturnSchema(
                    self.schema_version,
                ),
            );
        }
        if self.expected_at_ms < observed_at_ms || self.expected_at_ms > expires_at_ms {
            return Err(NodeAvailabilityValidationError::InvalidTimestamp(
                "expected_return.expected_at_ms",
            ));
        }
        Ok(())
    }
}

/// Why a node availability intent was rejected at the shared contract
/// boundary. The three terminal relationship variants are intentionally
/// distinct so callers cannot silently turn a replay or stale report into a
/// fresh outage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeAvailabilityValidationError {
    UnsupportedSchema(u16),
    UnsupportedExpectedReturnSchema(u16),
    InvalidField(&'static str),
    FieldTooLong(&'static str),
    InvalidGeneration,
    InvalidTimestamp(&'static str),
    ExpiryTooFar,
    ExpectedReturnRequired,
    ExpectedReturnForbidden,
    ConnectivityRequired(&'static str),
    ConnectivityForbidden(&'static str),
    Replay,
    Stale,
    Contradictory(&'static str),
}

impl fmt::Display for NodeAvailabilityValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema(version) => {
                write!(formatter, "unsupported node availability schema {version}")
            }
            Self::UnsupportedExpectedReturnSchema(version) => {
                write!(formatter, "unsupported expected-return schema {version}")
            }
            Self::InvalidField(field) => {
                write!(formatter, "invalid node availability field {field}")
            }
            Self::FieldTooLong(field) => {
                write!(formatter, "node availability field is too long: {field}")
            }
            Self::InvalidGeneration => formatter.write_str("invalid node availability generation"),
            Self::InvalidTimestamp(field) => {
                write!(formatter, "invalid node availability timestamp {field}")
            }
            Self::ExpiryTooFar => formatter.write_str("node availability expiry exceeds the bound"),
            Self::ExpectedReturnRequired => {
                formatter.write_str("this node availability state requires an expected return")
            }
            Self::ExpectedReturnForbidden => {
                formatter.write_str("this node availability state cannot carry an expected return")
            }
            Self::ConnectivityRequired(state) => {
                write!(formatter, "connectivity summaries are required for {state}")
            }
            Self::ConnectivityForbidden(state) => {
                write!(
                    formatter,
                    "connectivity summaries are forbidden for {state}"
                )
            }
            Self::Replay => formatter.write_str("replayed node availability event"),
            Self::Stale => formatter.write_str("stale node availability event"),
            Self::Contradictory(detail) => {
                write!(formatter, "contradictory node availability event: {detail}")
            }
        }
    }
}

impl std::error::Error for NodeAvailabilityValidationError {}

/// Versioned, bounded, node-owned expected-state intent. Consumers must call
/// [`NodeAvailabilityIntent::validate_at`] or
/// [`NodeAvailabilityIntent::validate_transition`] before folding it into
/// health; absence without one of the explicit expected states is unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeAvailabilityIntent {
    pub schema_version: u16,
    pub node_id: String,
    pub device_id: String,
    pub device_class: NodeDeviceClass,
    pub connection_type: NodeConnectionType,
    pub state: NodeAvailabilityState,
    pub reason: String,
    pub source: String,
    pub event_id: String,
    pub generation: u64,
    pub observed_at_ms: u64,
    pub expires_at_ms: u64,
    pub expected_return: Option<ExpectedReturn>,
    pub old_connectivity: Option<NodeConnectivitySummary>,
    pub new_connectivity: Option<NodeConnectivitySummary>,
}

impl NodeAvailabilityIntent {
    /// Validate the shape and state relationships without comparing it to a
    /// previous event or assuming a wall-clock time.
    pub fn validate(&self) -> Result<(), NodeAvailabilityValidationError> {
        if self.schema_version != NODE_AVAILABILITY_INTENT_SCHEMA_VERSION {
            return Err(NodeAvailabilityValidationError::UnsupportedSchema(
                self.schema_version,
            ));
        }
        validate_identifier("node_id", &self.node_id, MAX_NODE_AVAILABILITY_ID_BYTES)?;
        validate_identifier("device_id", &self.device_id, MAX_NODE_AVAILABILITY_ID_BYTES)?;
        validate_identifier("source", &self.source, MAX_NODE_AVAILABILITY_ID_BYTES)?;
        validate_identifier("event_id", &self.event_id, MAX_NODE_AVAILABILITY_ID_BYTES)?;
        validate_reason(&self.reason)?;
        if self.generation == 0 {
            return Err(NodeAvailabilityValidationError::InvalidGeneration);
        }
        if self.observed_at_ms == 0 {
            return Err(NodeAvailabilityValidationError::InvalidTimestamp(
                "observed_at_ms",
            ));
        }
        if self.expires_at_ms <= self.observed_at_ms {
            return Err(NodeAvailabilityValidationError::InvalidTimestamp(
                "expires_at_ms",
            ));
        }
        if self.expires_at_ms - self.observed_at_ms > MAX_NODE_AVAILABILITY_INTENT_TTL_MS {
            return Err(NodeAvailabilityValidationError::ExpiryTooFar);
        }

        if self.state.expects_return() {
            let Some(expected_return) = &self.expected_return else {
                return Err(NodeAvailabilityValidationError::ExpectedReturnRequired);
            };
            expected_return.validate(self.observed_at_ms, self.expires_at_ms)?;
        } else if self.expected_return.is_some() {
            return Err(NodeAvailabilityValidationError::ExpectedReturnForbidden);
        }

        match self.state {
            NodeAvailabilityState::AdapterMigration => {
                let (Some(old), Some(new)) = (&self.old_connectivity, &self.new_connectivity)
                else {
                    return Err(NodeAvailabilityValidationError::ConnectivityRequired(
                        "adapter_migration",
                    ));
                };
                old.validate()?;
                new.validate()?;
                if old == new {
                    return Err(NodeAvailabilityValidationError::Contradictory(
                        "adapter migration has no connectivity change",
                    ));
                }
                if old.connection_type != self.connection_type {
                    return Err(NodeAvailabilityValidationError::Contradictory(
                        "top-level connection does not match old connectivity",
                    ));
                }
            }
            NodeAvailabilityState::Returned => {
                if let Some(old) = &self.old_connectivity {
                    old.validate()?;
                    if self.new_connectivity.is_none() {
                        return Err(NodeAvailabilityValidationError::ConnectivityRequired(
                            "returned.new_connectivity",
                        ));
                    }
                }
                if let Some(new) = &self.new_connectivity {
                    new.validate()?;
                }
            }
            _ => {
                if self.old_connectivity.is_some() || self.new_connectivity.is_some() {
                    return Err(NodeAvailabilityValidationError::ConnectivityForbidden(
                        "state",
                    ));
                }
            }
        }
        Ok(())
    }

    /// Validate that the intent is current at `now_ms`. Expiry is a stale
    /// report, not evidence of an outage and never creates an inferred state.
    pub fn validate_at(&self, now_ms: u64) -> Result<(), NodeAvailabilityValidationError> {
        self.validate()?;
        if now_ms < self.observed_at_ms {
            return Err(NodeAvailabilityValidationError::InvalidTimestamp(
                "observed_at_ms",
            ));
        }
        if now_ms > self.expires_at_ms {
            return Err(NodeAvailabilityValidationError::Stale);
        }
        Ok(())
    }

    /// Validate this event against the immediately preceding event for the
    /// same node. Generation and event identity make duplicate delivery
    /// explicit; no consumer is allowed to repair a contradictory sequence by
    /// guessing which state the node intended.
    pub fn validate_transition(
        &self,
        previous: Option<&Self>,
        now_ms: u64,
    ) -> Result<(), NodeAvailabilityValidationError> {
        self.validate_at(now_ms)?;
        let Some(previous) = previous else {
            return Ok(());
        };
        previous.validate()?;
        if self.node_id != previous.node_id || self.device_id != previous.device_id {
            return Err(NodeAvailabilityValidationError::Contradictory(
                "node identity changed",
            ));
        }
        if self.event_id == previous.event_id {
            return Err(NodeAvailabilityValidationError::Replay);
        }
        if self.generation < previous.generation || self.observed_at_ms < previous.observed_at_ms {
            return Err(NodeAvailabilityValidationError::Stale);
        }
        if self.generation == previous.generation {
            return Err(NodeAvailabilityValidationError::Contradictory(
                "generation reused for a different event",
            ));
        }
        if !availability_transition_allowed(previous.state, self.state) {
            return Err(NodeAvailabilityValidationError::Contradictory(
                "invalid lifecycle transition",
            ));
        }
        Ok(())
    }

    /// Admit a record only after shape, current-time, replay, stale, and
    /// lifecycle-transition checks have passed.
    pub fn admitted(
        self,
        previous: Option<&Self>,
        now_ms: u64,
    ) -> Result<Self, NodeAvailabilityValidationError> {
        self.validate_transition(previous, now_ms)?;
        Ok(self)
    }

    /// Whether this record explicitly declares an expected absence.
    #[must_use]
    pub const fn expects_return(&self) -> bool {
        self.state.expects_return()
    }

    /// Return the producer-declared expected return time, if one exists.
    #[must_use]
    pub const fn expected_return_at_ms(&self) -> Option<u64> {
        match &self.expected_return {
            Some(expected_return) => Some(expected_return.expected_at_ms),
            None => None,
        }
    }
}

/// Result of applying a device-class availability policy to one node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeAvailabilityAssessment {
    /// The node has a fresh awake/returned report.
    Available,
    /// The node declared an absence and is still within its policy grace.
    ExpectedAbsence,
    /// A declared return was missed long enough to warn.
    WarningMissedReturn,
    /// A declared return was missed long enough to escalate.
    CriticalMissedReturn,
    /// No current intent is available and an observed node has gone stale.
    WarningUnannounced,
    /// No current intent is available and the stale node is critically late.
    CriticalUnannounced,
    /// No safe policy conclusion can be made from the supplied evidence.
    Unknown,
}

/// Bounded escalation defaults for one device class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeAvailabilityPolicy {
    /// Delay after an expected return before a warning.
    pub missed_return_warning_after_ms: u64,
    /// Delay after an expected return before a critical state.
    pub missed_return_critical_after_ms: u64,
    /// Delay after the last observation before an unannounced warning.
    pub unannounced_warning_after_ms: u64,
    /// Delay after the last observation before an unannounced critical state.
    pub unannounced_critical_after_ms: u64,
}

impl NodeAvailabilityPolicy {
    /// Return the governed defaults for a node/device class.
    #[must_use]
    pub const fn for_device_class(device_class: NodeDeviceClass) -> Self {
        match device_class {
            NodeDeviceClass::Desktop => Self::new(30_000, 120_000, 30_000, 120_000),
            NodeDeviceClass::Laptop => Self::new(60_000, 300_000, 60_000, 300_000),
            NodeDeviceClass::WirelessDevice => Self::new(60_000, 300_000, 30_000, 180_000),
            NodeDeviceClass::Server => Self::new(15_000, 60_000, 15_000, 60_000),
            NodeDeviceClass::Lighthouse => Self::new(15_000, 60_000, 10_000, 30_000),
            NodeDeviceClass::Unknown => Self::new(60_000, 300_000, 60_000, 300_000),
        }
    }

    /// Construct explicit policy values for tests or a later governed config.
    #[must_use]
    pub const fn new(
        missed_return_warning_after_ms: u64,
        missed_return_critical_after_ms: u64,
        unannounced_warning_after_ms: u64,
        unannounced_critical_after_ms: u64,
    ) -> Self {
        Self {
            missed_return_warning_after_ms,
            missed_return_critical_after_ms,
            unannounced_warning_after_ms,
            unannounced_critical_after_ms,
        }
    }

    /// Evaluate a current intent without inferring a state from its absence.
    ///
    /// `last_observed_at_ms` is an independent, typed heartbeat observation.
    /// It is required before an unannounced outage can be reported; a missing
    /// heartbeat timestamp remains [`NodeAvailabilityAssessment::Unknown`].
    #[must_use]
    pub fn assess(
        &self,
        intent: Option<&NodeAvailabilityIntent>,
        now_ms: u64,
        last_observed_at_ms: Option<u64>,
    ) -> NodeAvailabilityAssessment {
        let Some(intent) = intent else {
            return self.assess_unannounced(now_ms, last_observed_at_ms);
        };
        if intent.validate().is_err() || now_ms < intent.observed_at_ms {
            return NodeAvailabilityAssessment::Unknown;
        }
        if intent.state == NodeAvailabilityState::Unknown {
            return NodeAvailabilityAssessment::Unknown;
        }
        if intent.expects_return() {
            if now_ms > intent.expires_at_ms {
                return self.assess_unannounced(now_ms, last_observed_at_ms);
            }
            let Some(expected_at_ms) = intent.expected_return_at_ms() else {
                return NodeAvailabilityAssessment::Unknown;
            };
            return self.assess_expected_return(now_ms.saturating_sub(expected_at_ms));
        }
        if now_ms <= intent.expires_at_ms
            && matches!(
                intent.state,
                NodeAvailabilityState::Awake | NodeAvailabilityState::Returned
            )
        {
            NodeAvailabilityAssessment::Available
        } else {
            self.assess_unannounced(now_ms, last_observed_at_ms)
        }
    }

    fn assess_expected_return(&self, elapsed_ms: u64) -> NodeAvailabilityAssessment {
        if elapsed_ms >= self.missed_return_critical_after_ms {
            NodeAvailabilityAssessment::CriticalMissedReturn
        } else if elapsed_ms >= self.missed_return_warning_after_ms {
            NodeAvailabilityAssessment::WarningMissedReturn
        } else {
            NodeAvailabilityAssessment::ExpectedAbsence
        }
    }

    fn assess_unannounced(
        &self,
        now_ms: u64,
        last_observed_at_ms: Option<u64>,
    ) -> NodeAvailabilityAssessment {
        let Some(last_observed_at_ms) = last_observed_at_ms else {
            return NodeAvailabilityAssessment::Unknown;
        };
        if last_observed_at_ms > now_ms {
            return NodeAvailabilityAssessment::Unknown;
        }
        let elapsed_ms = now_ms.saturating_sub(last_observed_at_ms);
        if elapsed_ms >= self.unannounced_critical_after_ms {
            NodeAvailabilityAssessment::CriticalUnannounced
        } else if elapsed_ms >= self.unannounced_warning_after_ms {
            NodeAvailabilityAssessment::WarningUnannounced
        } else {
            NodeAvailabilityAssessment::Unknown
        }
    }
}

fn availability_transition_allowed(
    previous: NodeAvailabilityState,
    current: NodeAvailabilityState,
) -> bool {
    if previous == NodeAvailabilityState::Unknown || current == NodeAvailabilityState::Unknown {
        return true;
    }
    if previous == current {
        return previous != NodeAvailabilityState::Returned;
    }
    match (previous, current) {
        (
            NodeAvailabilityState::Awake,
            NodeAvailabilityState::Sleeping
            | NodeAvailabilityState::ShuttingDown
            | NodeAvailabilityState::ScheduledReboot
            | NodeAvailabilityState::Maintenance
            | NodeAvailabilityState::AdapterMigration,
        )
        | (
            NodeAvailabilityState::ShuttingDown | NodeAvailabilityState::ScheduledReboot,
            NodeAvailabilityState::ShutDown,
        )
        | (
            NodeAvailabilityState::ShuttingDown
            | NodeAvailabilityState::ShutDown
            | NodeAvailabilityState::ScheduledReboot,
            NodeAvailabilityState::Rebooting,
        )
        | (
            NodeAvailabilityState::ShuttingDown
            | NodeAvailabilityState::Sleeping
            | NodeAvailabilityState::ShutDown
            | NodeAvailabilityState::ScheduledReboot
            | NodeAvailabilityState::Rebooting
            | NodeAvailabilityState::Maintenance
            | NodeAvailabilityState::AdapterMigration,
            NodeAvailabilityState::Returned,
        )
        | (NodeAvailabilityState::ShutDown, NodeAvailabilityState::ScheduledReboot)
        | (NodeAvailabilityState::Returned, NodeAvailabilityState::Awake) => true,
        _ => false,
    }
}

fn validate_identifier(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), NodeAvailabilityValidationError> {
    if value.len() > max_bytes {
        return Err(NodeAvailabilityValidationError::FieldTooLong(field));
    }
    if value.is_empty()
        || value.trim() != value
        || !value.is_ascii()
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
        })
    {
        return Err(NodeAvailabilityValidationError::InvalidField(field));
    }
    Ok(())
}

fn validate_reason(value: &str) -> Result<(), NodeAvailabilityValidationError> {
    if value.len() > MAX_NODE_AVAILABILITY_REASON_BYTES {
        return Err(NodeAvailabilityValidationError::FieldTooLong("reason"));
    }
    if value.trim().is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(NodeAvailabilityValidationError::InvalidField("reason"));
    }
    Ok(())
}
/// Per-node state topic prefix.
pub const NODE_HEALTH_TOPIC_PREFIX: &str = "state/health/node/";
/// Roster-folded snapshot topic.
pub const SNAPSHOT_TOPIC: &str = "state/health/system-mesh";
/// Remediation request topic.
pub const ACTION_TOPIC: &str = "action/health/remediate";
/// Remediation result topic prefix.
pub const ACTION_RESULT_TOPIC_PREFIX: &str = "state/health/remediation/";
/// Critical notification lane. The chat worker remains the notification ledger.
pub const CRITICAL_NOTIFY_TOPIC: &str = "event/notify/system-mesh-health";

#[must_use]
pub fn node_health_topic(node: &str) -> String {
    format!("{NODE_HEALTH_TOPIC_PREFIX}{node}")
}

#[must_use]
pub fn action_result_topic(request_id: &str) -> String {
    format!("{ACTION_RESULT_TOPIC_PREFIX}{request_id}")
}

/// The only overall grade vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum GradeLetter {
    A,
    B,
    C,
    D,
    E,
    F,
}

impl GradeLetter {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
            Self::E => "E",
            Self::F => "F",
        }
    }
}

/// Wire discriminator for the one typed health lower-third contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthKironKind {
    HealthKiron,
}

/// Presentation urgency admitted by the health authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthKironAttention {
    Informational,
    Warning,
    Critical,
}

/// Grade-bound KIRON dwell behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthKironDwell {
    TimedMs(u64),
    UntilAcknowledged,
}

/// One validated UX-013 health transition projected into UX-014 KIRON.
///
/// The record carries the existing [`GradeLetter`] unchanged. It does not
/// evaluate health: UX-013 remains the sole authority for A-F production
/// state, while this contract only projects the admitted grade into KIRON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthKironAlert {
    pub kind: HealthKironKind,
    pub schema_version: u16,
    pub snapshot_generation: u64,
    pub condition_id: String,
    pub node: String,
    pub device: Option<String>,
    pub grade: GradeLetter,
    pub headline: String,
    pub active_since_ms: u64,
    pub observed_at_ms: u64,
}

impl HealthKironAlert {
    /// Reject malformed, stale-shaped, oversized, or secret-bearing display
    /// payloads before the shell can admit them to `ToastHost`.
    pub fn validate(&self) -> Result<(), NodeHealthValidationError> {
        if self.schema_version != HEALTH_KIRON_SCHEMA_VERSION {
            return Err(NodeHealthValidationError::UnsupportedSchema(
                self.schema_version,
            ));
        }
        if self.snapshot_generation == 0 {
            return Err(NodeHealthValidationError::InvalidGeneration);
        }
        validate_health_identifier("condition_id", &self.condition_id)?;
        validate_health_identifier("node", &self.node)?;
        if let Some(device) = &self.device {
            validate_health_label("device", device)?;
            if contains_secret_material(device) {
                return Err(NodeHealthValidationError::SecretBearing("device"));
            }
        }
        validate_health_text("headline", &self.headline)?;
        if contains_secret_material(&self.headline) {
            return Err(NodeHealthValidationError::SecretBearing("headline"));
        }
        if self.observed_at_ms == 0 || self.active_since_ms > self.observed_at_ms {
            return Err(NodeHealthValidationError::InvalidTimestamp(
                "kiron.lifecycle",
            ));
        }
        if self.active_duration_ms() > MAX_HEALTH_DURATION_MS {
            return Err(NodeHealthValidationError::InvalidTimestamp(
                "kiron.duration",
            ));
        }
        Ok(())
    }

    /// Duration derived from authority timestamps, never supplied separately by
    /// a presentation client.
    #[must_use]
    pub const fn active_duration_ms(&self) -> u64 {
        self.observed_at_ms.saturating_sub(self.active_since_ms)
    }

    /// Grade-bound attention policy consumed by the shell sound/suppression
    /// seam. No shell-local grade interpretation is required.
    #[must_use]
    pub const fn attention(&self) -> HealthKironAttention {
        match self.grade {
            GradeLetter::A | GradeLetter::B => HealthKironAttention::Informational,
            GradeLetter::C | GradeLetter::D => HealthKironAttention::Warning,
            GradeLetter::E | GradeLetter::F => HealthKironAttention::Critical,
        }
    }

    /// Exact dwell for every grade UX-013 can currently produce.
    #[must_use]
    pub const fn dwell(&self) -> HealthKironDwell {
        match self.grade {
            GradeLetter::A => HealthKironDwell::TimedMs(3_000),
            GradeLetter::B => HealthKironDwell::TimedMs(5_000),
            GradeLetter::C => HealthKironDwell::TimedMs(6_000),
            GradeLetter::D => HealthKironDwell::TimedMs(10_000),
            GradeLetter::E => HealthKironDwell::TimedMs(15_000),
            GradeLetter::F => HealthKironDwell::UntilAcknowledged,
        }
    }

    /// Bounded duration text used in the lower-third metadata lane.
    #[must_use]
    pub fn duration_label(&self) -> String {
        format_health_duration_ms(self.active_duration_ms())
    }
}

/// Typed factor breakdown. Values are capability scores, not incident severity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GradeFactors {
    pub cpu: Option<u8>,
    pub memory: Option<u8>,
    pub disk: Option<u8>,
    pub system: Option<u8>,
    pub mesh: Option<u8>,
    pub devices: Option<u8>,
}

/// One node's condition-backed grade.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeGrade {
    pub node: String,
    pub grade: GradeLetter,
    /// A–C capability/headroom score. D/E/F are selected by active conditions.
    pub capability_score: u8,
    pub factors: GradeFactors,
    pub evaluated_at_ms: u64,
}

impl NodeGrade {
    /// Grade invariant: critical => F; at least two distinct warnings => E;
    /// one warning => D; otherwise A–C only.
    #[must_use]
    pub fn evaluate(
        node: impl Into<String>,
        capability_score: u8,
        factors: GradeFactors,
        conditions: &[HealthCondition],
        evaluated_at_ms: u64,
    ) -> Self {
        let node = node.into();
        let (active_warnings, active_critical) =
            actionable_condition_counts(conditions, Some(&node));
        let grade = grade_from_evidence(capability_score, active_warnings, active_critical);
        Self {
            node,
            grade,
            capability_score,
            factors,
            evaluated_at_ms,
        }
    }
}

/// Count distinct active required condition identities, retaining the strongest
/// severity for a repeated identity. This makes the grade policy insensitive to
/// duplicate delivery and keeps optional, informational, resolved, and
/// wrong-node records from escalating a node.
fn actionable_condition_counts(
    conditions: &[HealthCondition],
    node: Option<&str>,
) -> (usize, usize) {
    let mut strongest_by_id = BTreeMap::new();
    for condition in conditions.iter().filter(|condition| {
        condition.is_active()
            && condition.requirement == RequirementClass::Required
            && node.is_none_or(|node| condition.scope.applies_to(node))
    }) {
        strongest_by_id
            .entry((condition.scope.clone(), condition.id.as_str()))
            .and_modify(|severity: &mut HealthSeverity| {
                *severity = (*severity).max(condition.severity);
            })
            .or_insert(condition.severity);
    }
    strongest_by_id
        .values()
        .fold((0, 0), |(warnings, critical), severity| match severity {
            HealthSeverity::Warning => (warnings + 1, critical),
            HealthSeverity::Critical => (warnings, critical + 1),
        })
}

/// The sole A-F production policy used for both node rows and mesh folds.
const fn grade_from_evidence(
    capability_score: u8,
    active_warnings: usize,
    active_critical: usize,
) -> GradeLetter {
    if active_critical > 0 {
        GradeLetter::F
    } else if active_warnings >= 2 {
        GradeLetter::E
    } else if active_warnings == 1 {
        GradeLetter::D
    } else {
        match capability_score {
            90..=u8::MAX => GradeLetter::A,
            80..=89 => GradeLetter::B,
            _ => GradeLetter::C,
        }
    }
}

/// Node or whole-mesh scope.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum HealthScope {
    Node { node: String },
    Mesh,
}

impl HealthScope {
    #[must_use]
    pub fn applies_to(&self, node: &str) -> bool {
        matches!(self, Self::Node { node: target } if target == node)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthSeverity {
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementClass {
    Required,
    Optional,
    Informational,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthComponent {
    System,
    Mesh,
    Resources,
    Devices,
    Audio,
    Firmware,
    Evidence,
}

/// Evidence is structured and timestamped so the UI never invents detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthEvidence {
    pub provider: String,
    pub summary: String,
    #[serde(default)]
    pub facts: BTreeMap<String, String>,
    pub observed_at_ms: u64,
}

/// One actionable condition with stable lifecycle identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthCondition {
    pub id: String,
    pub scope: HealthScope,
    pub component: HealthComponent,
    pub source: String,
    pub severity: HealthSeverity,
    pub requirement: RequirementClass,
    pub evidence: HealthEvidence,
    pub active_since_ms: u64,
    pub last_observed_ms: u64,
    pub resolved_at_ms: Option<u64>,
    pub acknowledged_at_ms: Option<u64>,
    pub snoozed_until_ms: Option<u64>,
    #[serde(default)]
    pub remediation: Vec<HealthRemediation>,
}

impl HealthCondition {
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.resolved_at_ms.is_none()
    }

    #[must_use]
    pub fn is_active_for_node(&self, node: &str) -> bool {
        self.is_active() && self.scope.applies_to(node)
    }

    #[must_use]
    pub fn counts_for_badge(&self, now_ms: u64) -> bool {
        self.is_active()
            && self.requirement == RequirementClass::Required
            && self.acknowledged_at_ms.is_none()
            && self.snoozed_until_ms.is_none_or(|until| until <= now_ms)
    }
}

/// Why a node-health publication was rejected at the shared fold boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeHealthValidationError {
    UnsupportedSchema(u16),
    InvalidField(&'static str),
    FieldTooLong(&'static str),
    TooMany(&'static str),
    InvalidGeneration,
    InvalidTimestamp(&'static str),
    ExpiryTooFar,
    SecretBearing(&'static str),
    Contradictory(&'static str),
    Stale,
}

impl fmt::Display for NodeHealthValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema(version) => {
                write!(formatter, "unsupported node-health schema {version}")
            }
            Self::InvalidField(field) => write!(formatter, "invalid health field {field}"),
            Self::FieldTooLong(field) => write!(formatter, "health field is too long: {field}"),
            Self::TooMany(field) => write!(formatter, "too many health records: {field}"),
            Self::InvalidGeneration => formatter.write_str("invalid node-health generation"),
            Self::InvalidTimestamp(field) => {
                write!(formatter, "invalid health timestamp {field}")
            }
            Self::ExpiryTooFar => formatter.write_str("node-health expiry exceeds the bound"),
            Self::SecretBearing(field) => {
                write!(formatter, "secret-bearing health field rejected: {field}")
            }
            Self::Contradictory(detail) => {
                write!(formatter, "contradictory node-health publication: {detail}")
            }
            Self::Stale => formatter.write_str("stale node-health publication"),
        }
    }
}

impl std::error::Error for NodeHealthValidationError {}

/// An exact allowlist: arbitrary units, paths, and commands cannot be encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthAction {
    Acknowledge,
    SnoozeOneHour,
    RefreshProvider,
    RestoreWorkstationAudio,
    RefreshFirmwareMetadata,
    RestartMackesd,
    RestartMeshBus,
    RestartNebula,
    RestartSyncthing,
    RestartDns,
    RestartKdc,
    RestartShell,
    ExpandSeat15Root,
    PublishOverlayIp,
    SetupEtcdClient,
    RecoverXdgBinds,
    RunLifecycleFirstboot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthRemediation {
    pub action: HealthAction,
    pub target: HealthScope,
    pub expected_snapshot_generation: u64,
    pub impact: String,
    pub confirmation_required: bool,
    pub workspace_route: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthActionRequest {
    pub schema_version: u16,
    pub request_id: String,
    pub condition_id: String,
    pub action: HealthAction,
    pub target: HealthScope,
    pub expected_snapshot_generation: u64,
    pub requester: String,
    pub authorization: String,
    pub confirmation: Option<String>,
    pub requested_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthActionOutcome {
    Applied,
    Refused,
    StaleGeneration,
    NotApplicable,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthActionResult {
    pub schema_version: u16,
    pub request_id: String,
    pub condition_id: String,
    pub action: HealthAction,
    pub outcome: HealthActionOutcome,
    pub detail: String,
    pub audit_id: String,
    pub completed_at_ms: u64,
    pub snapshot_generation: u64,
    pub refreshed_evidence: Option<HealthEvidence>,
}

impl HealthActionResult {
    /// Validate one remediation result before durable publication, replay, or
    /// presentation. Result timestamps are bounded by the consumer's current
    /// clock, and refreshed evidence cannot claim an observation after the
    /// remediation completed.
    pub fn validate_at(&self, now_ms: u64) -> Result<(), NodeHealthValidationError> {
        if self.schema_version != HEALTH_SCHEMA_VERSION {
            return Err(NodeHealthValidationError::UnsupportedSchema(
                self.schema_version,
            ));
        }
        validate_health_identifier("action_result.request_id", &self.request_id)?;
        validate_health_identifier("action_result.condition_id", &self.condition_id)?;
        validate_health_text("action_result.detail", &self.detail)?;
        if contains_secret_material(&self.detail) {
            return Err(NodeHealthValidationError::SecretBearing(
                "action_result.detail",
            ));
        }
        validate_health_identifier("action_result.audit_id", &self.audit_id)?;
        if self.completed_at_ms == 0 || self.completed_at_ms > now_ms {
            return Err(NodeHealthValidationError::InvalidTimestamp(
                "action_result.completed_at_ms",
            ));
        }
        if self.snapshot_generation == 0 {
            return Err(NodeHealthValidationError::InvalidGeneration);
        }
        if let Some(evidence) = &self.refreshed_evidence {
            validate_evidence(evidence, self.completed_at_ms)?;
        }
        Ok(())
    }
}

/// Node-owned publication folded by every observer against the live roster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeHealthState {
    pub schema_version: u16,
    pub publisher: String,
    pub roster_revision: String,
    pub generation: u64,
    pub published_at_ms: u64,
    pub valid_until_ms: u64,
    pub grade: NodeGrade,
    #[serde(default)]
    pub active_conditions: Vec<HealthCondition>,
    #[serde(default)]
    pub resolved_conditions: Vec<HealthCondition>,
}

impl NodeHealthState {
    /// Validate a complete node-owned publication before it enters a roster
    /// fold. Nested evidence is bounded and credential-shaped content is
    /// rejected instead of relying on a renderer to redact it later.
    pub fn validate_at(&self, now_ms: u64) -> Result<(), NodeHealthValidationError> {
        if self.schema_version != HEALTH_SCHEMA_VERSION {
            return Err(NodeHealthValidationError::UnsupportedSchema(
                self.schema_version,
            ));
        }
        validate_health_identifier("publisher", &self.publisher)?;
        validate_health_identifier("roster_revision", &self.roster_revision)?;
        if self.generation == 0 {
            return Err(NodeHealthValidationError::InvalidGeneration);
        }
        if self.published_at_ms == 0 || self.published_at_ms > now_ms {
            return Err(NodeHealthValidationError::InvalidTimestamp(
                "published_at_ms",
            ));
        }
        if self.valid_until_ms <= self.published_at_ms {
            return Err(NodeHealthValidationError::InvalidTimestamp(
                "valid_until_ms",
            ));
        }
        if self.valid_until_ms - self.published_at_ms > MAX_NODE_HEALTH_PUBLICATION_TTL_MS {
            return Err(NodeHealthValidationError::ExpiryTooFar);
        }
        if self.valid_until_ms < now_ms {
            return Err(NodeHealthValidationError::Stale);
        }
        if self.grade.node != self.publisher {
            return Err(NodeHealthValidationError::Contradictory(
                "grade node does not match publisher",
            ));
        }
        validate_grade(&self.grade, self.published_at_ms)?;
        if self.active_conditions.len() > MAX_NODE_HEALTH_CONDITIONS {
            return Err(NodeHealthValidationError::TooMany("active_conditions"));
        }
        if self.resolved_conditions.len() > MAX_NODE_HEALTH_CONDITIONS {
            return Err(NodeHealthValidationError::TooMany("resolved_conditions"));
        }
        let mut condition_identities = BTreeSet::new();
        for condition in &self.active_conditions {
            validate_condition(condition, &self.publisher, self.published_at_ms, true)?;
            if !condition_identities.insert((condition.scope.clone(), condition.id.clone())) {
                return Err(NodeHealthValidationError::Contradictory(
                    "duplicate active condition identity",
                ));
            }
        }
        for condition in &self.resolved_conditions {
            validate_condition(condition, &self.publisher, self.published_at_ms, false)?;
            if !condition_identities.insert((condition.scope.clone(), condition.id.clone())) {
                return Err(NodeHealthValidationError::Contradictory(
                    "condition identity appears in both lifecycle lanes",
                ));
            }
        }
        let evaluated = NodeGrade::evaluate(
            self.publisher.clone(),
            self.grade.capability_score,
            self.grade.factors,
            &self.active_conditions,
            self.grade.evaluated_at_ms,
        );
        if evaluated.grade != self.grade.grade {
            return Err(NodeHealthValidationError::Contradictory(
                "grade does not match active conditions",
            ));
        }
        Ok(())
    }
}

fn validate_grade(
    grade: &NodeGrade,
    published_at_ms: u64,
) -> Result<(), NodeHealthValidationError> {
    validate_health_identifier("grade.node", &grade.node)?;
    if grade.capability_score > 100 {
        return Err(NodeHealthValidationError::InvalidField(
            "grade.capability_score",
        ));
    }
    if [
        grade.factors.cpu,
        grade.factors.memory,
        grade.factors.disk,
        grade.factors.system,
        grade.factors.mesh,
        grade.factors.devices,
    ]
    .into_iter()
    .flatten()
    .any(|score| score > 100)
    {
        return Err(NodeHealthValidationError::InvalidField("grade.factors"));
    }
    if grade.evaluated_at_ms == 0 || grade.evaluated_at_ms > published_at_ms {
        return Err(NodeHealthValidationError::InvalidTimestamp(
            "grade.evaluated_at_ms",
        ));
    }
    Ok(())
}

fn validate_condition(
    condition: &HealthCondition,
    publisher: &str,
    published_at_ms: u64,
    expected_active: bool,
) -> Result<(), NodeHealthValidationError> {
    validate_health_identifier("condition.id", &condition.id)?;
    validate_health_label("condition.source", &condition.source)?;
    match &condition.scope {
        HealthScope::Node { node } if node == publisher => {
            validate_health_identifier("condition.scope.node", node)?;
        }
        HealthScope::Node { .. } => {
            return Err(NodeHealthValidationError::Contradictory(
                "condition scope does not match publisher",
            ));
        }
        HealthScope::Mesh => {
            return Err(NodeHealthValidationError::Contradictory(
                "node publication carries a mesh-scoped condition",
            ));
        }
    }
    if condition.is_active() != expected_active {
        return Err(NodeHealthValidationError::Contradictory(
            "condition is in the wrong lifecycle lane",
        ));
    }
    if condition.active_since_ms == 0
        || condition.active_since_ms > condition.last_observed_ms
        || condition.last_observed_ms > published_at_ms
    {
        return Err(NodeHealthValidationError::InvalidTimestamp(
            "condition.lifecycle",
        ));
    }
    if condition.evidence.observed_at_ms < condition.active_since_ms
        || condition.evidence.observed_at_ms > condition.last_observed_ms
    {
        return Err(NodeHealthValidationError::InvalidTimestamp(
            "condition.evidence_lifecycle",
        ));
    }
    for (field, timestamp) in [
        ("condition.resolved_at_ms", condition.resolved_at_ms),
        ("condition.acknowledged_at_ms", condition.acknowledged_at_ms),
    ] {
        if timestamp.is_some_and(|timestamp| {
            timestamp < condition.active_since_ms || timestamp > published_at_ms
        }) {
            return Err(NodeHealthValidationError::InvalidTimestamp(field));
        }
    }
    if condition
        .resolved_at_ms
        .is_some_and(|resolved_at_ms| resolved_at_ms < condition.last_observed_ms)
    {
        return Err(NodeHealthValidationError::InvalidTimestamp(
            "condition.resolved_at_ms",
        ));
    }
    if !expected_active
        && condition.resolved_at_ms.is_some_and(|resolved_at_ms| {
            published_at_ms.saturating_sub(resolved_at_ms) > MAX_HEALTH_HISTORY_RETENTION_MS
        })
    {
        return Err(NodeHealthValidationError::InvalidTimestamp(
            "condition.resolved_at_ms",
        ));
    }
    if condition
        .snoozed_until_ms
        .is_some_and(|timestamp| timestamp < condition.active_since_ms)
    {
        return Err(NodeHealthValidationError::InvalidTimestamp(
            "condition.snoozed_until_ms",
        ));
    }
    if condition.remediation.len() > MAX_HEALTH_REMEDIATIONS {
        return Err(NodeHealthValidationError::TooMany("condition.remediation"));
    }
    validate_evidence(&condition.evidence, published_at_ms)?;
    for remediation in &condition.remediation {
        validate_health_text("remediation.impact", &remediation.impact)?;
        if let Some(route) = &remediation.workspace_route {
            validate_health_text("remediation.workspace_route", route)?;
        }
        if remediation.target != condition.scope {
            return Err(NodeHealthValidationError::Contradictory(
                "remediation target does not match condition scope",
            ));
        }
    }
    Ok(())
}

fn validate_evidence(
    evidence: &HealthEvidence,
    published_at_ms: u64,
) -> Result<(), NodeHealthValidationError> {
    validate_health_label("evidence.provider", &evidence.provider)?;
    validate_health_text("evidence.summary", &evidence.summary)?;
    if evidence.observed_at_ms == 0 || evidence.observed_at_ms > published_at_ms {
        return Err(NodeHealthValidationError::InvalidTimestamp(
            "evidence.observed_at_ms",
        ));
    }
    if evidence.facts.len() > MAX_HEALTH_EVIDENCE_FACTS {
        return Err(NodeHealthValidationError::TooMany("evidence.facts"));
    }
    let mut bytes = evidence.provider.len() + evidence.summary.len();
    if contains_secret_material(&evidence.summary) {
        return Err(NodeHealthValidationError::SecretBearing("evidence.summary"));
    }
    for (key, value) in &evidence.facts {
        validate_health_identifier("evidence.facts.key", key)?;
        validate_health_text("evidence.facts.value", value)?;
        bytes = bytes.saturating_add(key.len()).saturating_add(value.len());
        if secret_field_name(key) || contains_secret_material(value) {
            return Err(NodeHealthValidationError::SecretBearing("evidence.facts"));
        }
    }
    if bytes > MAX_HEALTH_EVIDENCE_BYTES {
        return Err(NodeHealthValidationError::FieldTooLong("evidence"));
    }
    Ok(())
}

fn validate_health_identifier(
    field: &'static str,
    value: &str,
) -> Result<(), NodeHealthValidationError> {
    if value.len() > MAX_HEALTH_ID_BYTES {
        return Err(NodeHealthValidationError::FieldTooLong(field));
    }
    if value.is_empty()
        || value.trim() != value
        || !value.is_ascii()
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
        })
    {
        return Err(NodeHealthValidationError::InvalidField(field));
    }
    Ok(())
}

fn validate_health_label(
    field: &'static str,
    value: &str,
) -> Result<(), NodeHealthValidationError> {
    if value.len() > MAX_HEALTH_ID_BYTES {
        return Err(NodeHealthValidationError::FieldTooLong(field));
    }
    if value.is_empty()
        || value.trim() != value
        || !value.is_ascii()
        || value.chars().any(char::is_control)
    {
        return Err(NodeHealthValidationError::InvalidField(field));
    }
    Ok(())
}

fn validate_health_text(field: &'static str, value: &str) -> Result<(), NodeHealthValidationError> {
    if value.len() > MAX_HEALTH_TEXT_BYTES {
        return Err(NodeHealthValidationError::FieldTooLong(field));
    }
    if value.trim().is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(NodeHealthValidationError::InvalidField(field));
    }
    Ok(())
}

fn secret_field_name(value: &str) -> bool {
    let normalized: String = value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect();
    [
        "authorization",
        "cookie",
        "credential",
        "password",
        "privatekey",
        "secret",
        "token",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

fn contains_secret_material(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("-----begin ")
        || lower.contains("authorization: bearer ")
        || lower.contains("password=")
        || lower.contains("private_key=")
        || lower.contains("secret=")
        || lower.contains("token=")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshHealthSummary {
    pub grade: GradeLetter,
    pub canonical_nodes: usize,
    pub fresh_nodes: usize,
    pub reachable_lighthouses: usize,
    pub active_warnings: usize,
    pub active_critical: usize,
    pub unacknowledged_actionable: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemMeshHealthSnapshot {
    pub schema_version: u16,
    pub observer: String,
    pub roster_revision: String,
    pub generation: u64,
    pub generated_at_ms: u64,
    pub fresh_until_ms: u64,
    #[serde(default)]
    pub current_node_grades: Vec<NodeGrade>,
    #[serde(default)]
    pub active_conditions: Vec<HealthCondition>,
    #[serde(default)]
    pub resolved_conditions: Vec<HealthCondition>,
    pub mesh_summary: MeshHealthSummary,
}

impl SystemMeshHealthSnapshot {
    #[must_use]
    pub fn active_issue_count(&self, now_ms: u64) -> usize {
        self.active_conditions
            .iter()
            .filter(|condition| condition.counts_for_badge(now_ms))
            .count()
    }

    #[must_use]
    pub fn is_fresh(&self, now_ms: u64) -> bool {
        // A projection cannot be current before it was generated.  The
        // nonzero check also keeps the wire-level zero timestamp sentinel
        // from becoming fresh when a test or hostile caller supplies
        // `now_ms == 0`.
        self.generated_at_ms != 0 && self.generated_at_ms <= now_ms && now_ms <= self.fresh_until_ms
    }
}

/// Reject non-roster, duplicate, mismatched, and expired publications.
#[must_use]
pub fn fold_snapshot(
    observer: impl Into<String>,
    roster_revision: impl Into<String>,
    canonical_nodes: &BTreeSet<String>,
    publications: Vec<NodeHealthState>,
    generation: u64,
    now_ms: u64,
    validity_ms: u64,
    reachable_lighthouses: usize,
) -> SystemMeshHealthSnapshot {
    fold_snapshot_with_availability(
        observer,
        roster_revision,
        canonical_nodes,
        publications,
        &BTreeMap::new(),
        generation,
        now_ms,
        validity_ms,
        reachable_lighthouses,
    )
}

/// Fold canonical health publications while applying governed availability
/// assessments to roster nodes whose publisher is currently absent.
///
/// Availability never manufactures a fresh node row. It only explains a
/// missing publisher as an expected absence or replaces the generic freshness
/// warning with the shared missed-return severity. Nodes without a conclusive
/// assessment retain the ordinary missing-publisher warning.
#[must_use]
pub fn fold_snapshot_with_availability(
    observer: impl Into<String>,
    roster_revision: impl Into<String>,
    canonical_nodes: &BTreeSet<String>,
    publications: Vec<NodeHealthState>,
    availability: &BTreeMap<String, NodeAvailabilityAssessment>,
    generation: u64,
    now_ms: u64,
    validity_ms: u64,
    reachable_lighthouses: usize,
) -> SystemMeshHealthSnapshot {
    let roster_revision = roster_revision.into();
    let mut by_publisher: BTreeMap<String, Option<NodeHealthState>> = BTreeMap::new();
    for state in publications {
        if state.validate_at(now_ms).is_err()
            || !canonical_nodes.contains(&state.publisher)
            || state.roster_revision != roster_revision
        {
            continue;
        }
        match by_publisher.entry(state.publisher.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(Some(state));
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                entry.insert(None);
            }
        }
    }
    let mut accepted: Vec<_> = by_publisher.into_values().flatten().collect();
    accepted.sort_by(|left, right| left.publisher.cmp(&right.publisher));

    let mut active_conditions: Vec<_> = accepted
        .iter()
        .flat_map(|state| {
            state
                .active_conditions
                .iter()
                .filter(|condition| condition.scope.applies_to(&state.publisher))
                .cloned()
        })
        .filter(HealthCondition::is_active)
        .collect();
    let resolved_conditions = accepted
        .iter()
        .flat_map(|state| {
            state
                .resolved_conditions
                .iter()
                .filter(|condition| condition.scope.applies_to(&state.publisher))
                .cloned()
        })
        .collect();
    if accepted.len() < canonical_nodes.len() {
        let missing: Vec<_> = canonical_nodes
            .iter()
            .filter(|node| !accepted.iter().any(|state| &state.publisher == *node))
            .cloned()
            .collect();
        let mut unexplained = Vec::new();
        for node in missing {
            let (severity, requirement, summary) = match availability.get(&node) {
                Some(NodeAvailabilityAssessment::ExpectedAbsence) => (
                    HealthSeverity::Warning,
                    RequirementClass::Informational,
                    "Health publication is absent during a declared lifecycle window.",
                ),
                Some(NodeAvailabilityAssessment::WarningMissedReturn) => (
                    HealthSeverity::Warning,
                    RequirementClass::Required,
                    "Health publication is absent after the declared return time.",
                ),
                Some(NodeAvailabilityAssessment::CriticalMissedReturn) => (
                    HealthSeverity::Critical,
                    RequirementClass::Required,
                    "Health publication remains absent beyond the governed return grace.",
                ),
                _ => {
                    unexplained.push(node);
                    continue;
                }
            };
            active_conditions.push(HealthCondition {
                id: format!("{node}:publisher-availability"),
                scope: HealthScope::Node { node: node.clone() },
                component: HealthComponent::Evidence,
                source: "health-availability-fold".into(),
                severity,
                requirement,
                evidence: HealthEvidence {
                    provider: "health-availability-fold".into(),
                    summary: summary.to_string(),
                    facts: BTreeMap::from([("missing_node".into(), node)]),
                    observed_at_ms: now_ms,
                },
                active_since_ms: now_ms,
                last_observed_ms: now_ms,
                resolved_at_ms: None,
                acknowledged_at_ms: None,
                snoozed_until_ms: None,
                remediation: Vec::new(),
            });
        }
        if !unexplained.is_empty() {
            active_conditions.push(HealthCondition {
                id: "mesh:publisher-freshness".into(),
                scope: HealthScope::Mesh,
                component: HealthComponent::Evidence,
                source: "health-roster-fold".into(),
                severity: HealthSeverity::Warning,
                requirement: RequirementClass::Required,
                evidence: HealthEvidence {
                    provider: "health-roster-fold".into(),
                    summary: format!(
                        "Current health evidence is missing for: {}.",
                        unexplained.join(", ")
                    ),
                    facts: BTreeMap::from([("missing_nodes".into(), unexplained.join(","))]),
                    observed_at_ms: now_ms,
                },
                active_since_ms: now_ms,
                last_observed_ms: now_ms,
                resolved_at_ms: None,
                acknowledged_at_ms: None,
                snoozed_until_ms: None,
                remediation: Vec::new(),
            });
        }
    }
    let current_node_grades: Vec<NodeGrade> = accepted
        .iter()
        .map(|state| {
            let conditions: Vec<_> = active_conditions
                .iter()
                .filter(|condition| condition.scope.applies_to(&state.publisher))
                .cloned()
                .collect();
            NodeGrade::evaluate(
                state.publisher.clone(),
                state.grade.capability_score,
                state.grade.factors,
                &conditions,
                state.grade.evaluated_at_ms,
            )
        })
        .collect();
    let (active_warnings, active_critical) = actionable_condition_counts(&active_conditions, None);
    let unacknowledged_actionable = active_conditions
        .iter()
        .filter(|condition| condition.counts_for_badge(now_ms))
        .count();
    let mesh_capability_score = current_node_grades
        .iter()
        .map(|grade| grade.capability_score)
        .min()
        .unwrap_or(70);
    let grade = grade_from_evidence(mesh_capability_score, active_warnings, active_critical);
    // A roster fold is a projection of admitted publications, not a new
    // observation that can extend their lifetime. Bound an empty/synthetic
    // fold to the contract maximum, and never let a populated projection
    // outlive its earliest-expiring source publication.
    let mut fresh_until_ms =
        now_ms.saturating_add(validity_ms.min(MAX_NODE_HEALTH_PUBLICATION_TTL_MS));
    if let Some(source_expiry_ms) = accepted.iter().map(|state| state.valid_until_ms).min() {
        fresh_until_ms = fresh_until_ms.min(source_expiry_ms);
    }
    SystemMeshHealthSnapshot {
        schema_version: HEALTH_SCHEMA_VERSION,
        observer: observer.into(),
        roster_revision,
        generation,
        generated_at_ms: now_ms,
        fresh_until_ms,
        current_node_grades,
        active_conditions,
        resolved_conditions,
        mesh_summary: MeshHealthSummary {
            grade,
            canonical_nodes: canonical_nodes.len(),
            fresh_nodes: accepted.len(),
            reachable_lighthouses,
            active_warnings,
            active_critical,
            unacknowledged_actionable,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn condition(node: &str, severity: HealthSeverity) -> HealthCondition {
        HealthCondition {
            id: format!("{node}:disk"),
            scope: HealthScope::Node { node: node.into() },
            component: HealthComponent::Resources,
            source: "test".into(),
            severity,
            requirement: RequirementClass::Required,
            evidence: HealthEvidence {
                provider: "fixture".into(),
                summary: "threshold breached".into(),
                facts: BTreeMap::new(),
                observed_at_ms: 100,
            },
            active_since_ms: 100,
            last_observed_ms: 100,
            resolved_at_ms: None,
            acknowledged_at_ms: None,
            snoozed_until_ms: None,
            remediation: Vec::new(),
        }
    }

    fn state(node: &str, generation: u64, now: u64) -> NodeHealthState {
        NodeHealthState {
            schema_version: HEALTH_SCHEMA_VERSION,
            publisher: node.into(),
            roster_revision: "r1".into(),
            generation,
            published_at_ms: now,
            valid_until_ms: now + 100,
            grade: NodeGrade::evaluate(node.to_string(), 95, GradeFactors::default(), &[], now),
            active_conditions: Vec::new(),
            resolved_conditions: Vec::new(),
        }
    }

    fn connectivity(
        connection_type: NodeConnectionType,
        interface_id: Option<&str>,
        address_family: NodeAddressFamily,
        reachable: bool,
    ) -> NodeConnectivitySummary {
        NodeConnectivitySummary {
            connection_type,
            interface_id: interface_id.map(str::to_owned),
            address_family,
            reachable,
        }
    }

    fn availability(
        state: NodeAvailabilityState,
        generation: u64,
        event_id: &str,
        observed_at_ms: u64,
    ) -> NodeAvailabilityIntent {
        let (expected_return, old_connectivity, new_connectivity, connection_type) =
            if state == NodeAvailabilityState::AdapterMigration {
                (
                    Some(ExpectedReturn::new(observed_at_ms + 5_000)),
                    Some(connectivity(
                        NodeConnectionType::Ethernet,
                        Some("eno1"),
                        NodeAddressFamily::Ipv4,
                        true,
                    )),
                    Some(connectivity(
                        NodeConnectionType::Wifi,
                        Some("wlan0"),
                        NodeAddressFamily::DualStack,
                        true,
                    )),
                    NodeConnectionType::Ethernet,
                )
            } else {
                (
                    state
                        .expects_return()
                        .then(|| ExpectedReturn::new(observed_at_ms + 5_000)),
                    None,
                    None,
                    NodeConnectionType::Ethernet,
                )
            };
        NodeAvailabilityIntent {
            schema_version: NODE_AVAILABILITY_INTENT_SCHEMA_VERSION,
            node_id: "seat-15".into(),
            device_id: "device-15".into(),
            device_class: NodeDeviceClass::Desktop,
            connection_type,
            state,
            reason: "operator-directed lifecycle transition".into(),
            source: "mackesd.health".into(),
            event_id: event_id.into(),
            generation,
            observed_at_ms,
            expires_at_ms: observed_at_ms + 10_000,
            expected_return,
            old_connectivity,
            new_connectivity,
        }
    }

    #[test]
    fn shared_duration_formatter_uses_exact_locale_independent_boundaries() {
        assert_eq!(format_health_duration_ms(0), "0s");
        assert_eq!(format_health_duration_ms(999), "0s");
        assert_eq!(format_health_duration_ms(59_999), "59s");
        assert_eq!(format_health_duration_ms(60_000), "1m 00s");
        assert_eq!(format_health_duration_ms(3_599_999), "59m 59s");
        assert_eq!(format_health_duration_ms(3_600_000), "01:00:00");
        assert_eq!(format_health_duration_ms(86_400_000), "24:00:00");
        assert_eq!(format_health_duration_ms(86_400_001), "1d 00:00:00");
        assert_eq!(
            format_health_duration_ms(2 * 86_400_000 + 3_600_000 + 12 * 60_000 + 8_000,),
            "2d 01:12:08"
        );
        assert_eq!(
            format_duration_ms(MAX_HEALTH_DURATION_MS + 1),
            format_duration_ms(MAX_HEALTH_DURATION_MS)
        );
    }

    #[test]
    fn availability_states_are_closed_versioned_and_explicit() {
        let states = [
            NodeAvailabilityState::Awake,
            NodeAvailabilityState::Sleeping,
            NodeAvailabilityState::ShuttingDown,
            NodeAvailabilityState::ShutDown,
            NodeAvailabilityState::ScheduledReboot,
            NodeAvailabilityState::Rebooting,
            NodeAvailabilityState::Maintenance,
            NodeAvailabilityState::AdapterMigration,
            NodeAvailabilityState::Returned,
            NodeAvailabilityState::Unknown,
        ];
        for (generation, state) in states.into_iter().enumerate() {
            let intent = availability(state, generation as u64 + 1, "event-state", 1_000);
            assert_eq!(intent.validate(), Ok(()), "state {state:?}");
            assert_eq!(intent.expects_return(), state.expects_return());
            assert_eq!(
                intent.expected_return_at_ms(),
                state.expects_return().then_some(6_000)
            );
            let encoded = serde_json::to_value(&intent).expect("availability serializes");
            assert_eq!(encoded["state"], serde_json::to_value(state).unwrap());
        }

        let mut unknown = availability(NodeAvailabilityState::Unknown, 1, "event-unknown", 1_000);
        unknown.expected_return = Some(ExpectedReturn::new(2_000));
        assert_eq!(
            unknown.validate(),
            Err(NodeAvailabilityValidationError::ExpectedReturnForbidden)
        );

        let mut sleeping = availability(NodeAvailabilityState::Sleeping, 1, "event-sleep", 1_000);
        sleeping.expected_return = None;
        assert_eq!(
            sleeping.validate(),
            Err(NodeAvailabilityValidationError::ExpectedReturnRequired)
        );
    }

    #[test]
    fn availability_contract_rejects_bounds_schema_and_connectivity_contradictions() {
        let mut intent = availability(
            NodeAvailabilityState::AdapterMigration,
            1,
            "event-adapter",
            1_000,
        );
        intent.old_connectivity = intent.new_connectivity.clone();
        assert!(matches!(
            intent.validate(),
            Err(NodeAvailabilityValidationError::Contradictory(_))
        ));

        let mut wrong_old_connection = availability(
            NodeAvailabilityState::AdapterMigration,
            1,
            "event-adapter-2",
            1_000,
        );
        wrong_old_connection.connection_type = NodeConnectionType::Wifi;
        assert!(matches!(
            wrong_old_connection.validate(),
            Err(NodeAvailabilityValidationError::Contradictory(_))
        ));

        let mut oversized = availability(NodeAvailabilityState::Awake, 1, "event-long", 1_000);
        oversized.reason = "r".repeat(MAX_NODE_AVAILABILITY_REASON_BYTES + 1);
        assert_eq!(
            oversized.validate(),
            Err(NodeAvailabilityValidationError::FieldTooLong("reason"))
        );

        let mut unsupported = availability(NodeAvailabilityState::Awake, 1, "event-schema", 1_000);
        unsupported.schema_version += 1;
        assert_eq!(
            unsupported.validate(),
            Err(NodeAvailabilityValidationError::UnsupportedSchema(
                NODE_AVAILABILITY_INTENT_SCHEMA_VERSION + 1
            ))
        );

        let mut expected_schema = availability(
            NodeAvailabilityState::Sleeping,
            1,
            "event-return-schema",
            1_000,
        );
        expected_schema
            .expected_return
            .as_mut()
            .expect("sleeping has an expected return")
            .schema_version += 1;
        assert!(matches!(
            expected_schema.validate(),
            Err(NodeAvailabilityValidationError::UnsupportedExpectedReturnSchema(_))
        ));

        let mut unknown_fields = serde_json::to_value(availability(
            NodeAvailabilityState::Awake,
            1,
            "event-unknown-field",
            1_000,
        ))
        .expect("availability serializes");
        unknown_fields["unadmitted"] = serde_json::json!(true);
        assert!(serde_json::from_value::<NodeAvailabilityIntent>(unknown_fields).is_err());
    }

    #[test]
    fn availability_transition_validation_distinguishes_replay_stale_and_contradiction() {
        let previous = availability(NodeAvailabilityState::Awake, 4, "event-4", 1_000);
        assert_eq!(
            previous.validate_transition(Some(&previous), 2_000),
            Err(NodeAvailabilityValidationError::Replay)
        );

        let mut stale = availability(NodeAvailabilityState::Awake, 3, "event-3", 1_100);
        stale.expires_at_ms = 20_000;
        assert_eq!(
            stale.validate_transition(Some(&previous), 2_000),
            Err(NodeAvailabilityValidationError::Stale)
        );

        let same_generation = availability(NodeAvailabilityState::Awake, 4, "event-other", 1_100);
        assert!(matches!(
            same_generation.validate_transition(Some(&previous), 2_000),
            Err(NodeAvailabilityValidationError::Contradictory(_))
        ));

        let sleeping = availability(NodeAvailabilityState::Sleeping, 5, "event-sleep", 2_100);
        let contradictory = availability(
            NodeAvailabilityState::ShuttingDown,
            6,
            "event-shutdown",
            2_200,
        );
        assert!(matches!(
            contradictory.validate_transition(Some(&sleeping), 2_500),
            Err(NodeAvailabilityValidationError::Contradictory(_))
        ));

        let returned = availability(NodeAvailabilityState::Returned, 6, "event-return", 2_200);
        assert_eq!(returned.validate_transition(Some(&sleeping), 2_500), Ok(()));
        assert!(returned.admitted(Some(&sleeping), 2_500).is_ok());

        let expired = availability(NodeAvailabilityState::Awake, 7, "event-expired", 3_000);
        assert_eq!(
            expired.validate_at(13_001),
            Err(NodeAvailabilityValidationError::Stale)
        );
    }

    #[test]
    fn availability_history_handles_max_timestamp_boundary_and_rejects_oversized_ttl() {
        let previous = availability(
            NodeAvailabilityState::Sleeping,
            1,
            "event-sleep-max",
            u64::MAX - 20_000,
        );
        let returned = availability(
            NodeAvailabilityState::Returned,
            2,
            "event-return-max",
            u64::MAX - 10_000,
        );
        assert_eq!(
            returned.validate_transition(Some(&previous), u64::MAX),
            Ok(())
        );

        let mut oversized = availability(NodeAvailabilityState::Awake, 1, "event-ttl", 1_000);
        oversized.expires_at_ms = oversized
            .observed_at_ms
            .saturating_add(MAX_NODE_AVAILABILITY_INTENT_TTL_MS + 1);
        assert_eq!(
            oversized.validate(),
            Err(NodeAvailabilityValidationError::ExpiryTooFar)
        );
    }

    #[test]
    fn availability_policy_keeps_expected_absence_informational_then_escalates() {
        let policy = NodeAvailabilityPolicy::new(100, 200, 100, 200);
        let sleeping = availability(NodeAvailabilityState::Sleeping, 1, "event-sleep", 1_000);
        assert_eq!(
            policy.assess(Some(&sleeping), 5_999, None),
            NodeAvailabilityAssessment::ExpectedAbsence
        );
        assert_eq!(
            policy.assess(Some(&sleeping), 6_100, None),
            NodeAvailabilityAssessment::WarningMissedReturn
        );
        assert_eq!(
            policy.assess(Some(&sleeping), 6_200, None),
            NodeAvailabilityAssessment::CriticalMissedReturn
        );

        let awake = availability(NodeAvailabilityState::Awake, 2, "event-awake", 1_000);
        assert_eq!(
            policy.assess(Some(&awake), 2_000, None),
            NodeAvailabilityAssessment::Available
        );
        assert_eq!(
            policy.assess(Some(&awake), 1_050, None),
            NodeAvailabilityAssessment::Available
        );
        assert_eq!(
            policy.assess(None, 1_050, None),
            NodeAvailabilityAssessment::Unknown
        );
        assert_eq!(
            policy.assess(None, 1_100, Some(1_000)),
            NodeAvailabilityAssessment::WarningUnannounced
        );
        assert_eq!(
            policy.assess(None, 1_200, Some(1_000)),
            NodeAvailabilityAssessment::CriticalUnannounced
        );
    }

    #[test]
    fn availability_policy_defaults_are_device_aware_and_bounded() {
        assert_eq!(
            NodeAvailabilityPolicy::for_device_class(NodeDeviceClass::Server),
            NodeAvailabilityPolicy::new(15_000, 60_000, 15_000, 60_000)
        );
        assert_eq!(
            NodeAvailabilityPolicy::for_device_class(NodeDeviceClass::Laptop),
            NodeAvailabilityPolicy::new(60_000, 300_000, 60_000, 300_000)
        );
        assert!(
            NodeAvailabilityPolicy::for_device_class(NodeDeviceClass::Lighthouse)
                .unannounced_critical_after_ms
                < NodeAvailabilityPolicy::for_device_class(NodeDeviceClass::Laptop)
                    .unannounced_critical_after_ms
        );
    }

    #[test]
    fn condition_backed_grades_cover_d_e_f_without_duplicate_escalation() {
        let c = NodeGrade::evaluate("n", 1, GradeFactors::default(), &[], 100);
        assert_eq!(c.grade, GradeLetter::C);
        let warning = condition("n", HealthSeverity::Warning);
        let d = NodeGrade::evaluate("n", 99, GradeFactors::default(), &[warning.clone()], 100);
        assert_eq!(d.grade, GradeLetter::D);
        let duplicate = NodeGrade::evaluate(
            "n",
            99,
            GradeFactors::default(),
            &[warning.clone(), warning.clone()],
            100,
        );
        assert_eq!(
            duplicate.grade,
            GradeLetter::D,
            "duplicate delivery of one condition identity cannot fabricate E"
        );
        let first_warning = warning;
        let mut second_warning = first_warning.clone();
        second_warning.id = "n:memory".into();
        let e = NodeGrade::evaluate(
            "n",
            99,
            GradeFactors::default(),
            &[first_warning, second_warning],
            100,
        );
        assert_eq!(e.grade, GradeLetter::E);
        let critical = condition("n", HealthSeverity::Critical);
        let f = NodeGrade::evaluate("n", 99, GradeFactors::default(), &[critical], 100);
        assert_eq!(f.grade, GradeLetter::F);

        let mut informational = condition("n", HealthSeverity::Critical);
        informational.requirement = RequirementClass::Informational;
        let mut optional = condition("n", HealthSeverity::Warning);
        optional.requirement = RequirementClass::Optional;
        let wrong_node = condition("other", HealthSeverity::Warning);
        let mut resolved = condition("n", HealthSeverity::Warning);
        resolved.resolved_at_ms = Some(100);
        let a = NodeGrade::evaluate(
            "n",
            99,
            GradeFactors::default(),
            &[informational, optional, wrong_node, resolved],
            100,
        );
        assert_eq!(
            a.grade,
            GradeLetter::A,
            "non-actionable, resolved, and wrong-node records cannot lower the grade"
        );
    }

    #[test]
    fn mesh_fold_uses_the_same_compounded_warning_policy() {
        let roster = BTreeSet::from(["node".to_string()]);
        let mut node = state("node", 1, 100);
        let first = condition("node", HealthSeverity::Warning);
        let mut second = first.clone();
        second.id = "node:memory".into();
        node.active_conditions = vec![first, second];
        node.grade = NodeGrade::evaluate(
            "node",
            node.grade.capability_score,
            node.grade.factors,
            &node.active_conditions,
            100,
        );

        let snapshot = fold_snapshot("node", "r1", &roster, vec![node], 2, 100, 100, 1);
        assert_eq!(snapshot.current_node_grades[0].grade, GradeLetter::E);
        assert_eq!(snapshot.mesh_summary.active_warnings, 2);
        assert_eq!(snapshot.mesh_summary.grade, GradeLetter::E);
    }

    #[test]
    fn mesh_fold_keeps_equal_condition_ids_distinct_across_node_scopes() {
        let roster = BTreeSet::from(["node-a".to_string(), "node-b".to_string()]);
        let mut states = Vec::new();
        for node in ["node-a", "node-b"] {
            let mut state = state(node, 1, 100);
            let mut warning = condition(node, HealthSeverity::Warning);
            warning.id = "disk-pressure".into();
            state.active_conditions.push(warning);
            state.grade = NodeGrade::evaluate(
                node,
                state.grade.capability_score,
                state.grade.factors,
                &state.active_conditions,
                100,
            );
            states.push(state);
        }

        let snapshot = fold_snapshot("node-a", "r1", &roster, states, 2, 100, 100, 1);
        assert_eq!(snapshot.mesh_summary.active_warnings, 2);
        assert_eq!(snapshot.mesh_summary.grade, GradeLetter::E);
    }

    #[test]
    fn badge_excludes_acknowledged_snoozed_optional_and_resolved() {
        let base = condition("n", HealthSeverity::Warning);
        assert!(base.counts_for_badge(100));
        let mut acknowledged = base.clone();
        acknowledged.acknowledged_at_ms = Some(100);
        assert!(!acknowledged.counts_for_badge(100));
        let mut snoozed = base.clone();
        snoozed.snoozed_until_ms = Some(200);
        assert!(!snoozed.counts_for_badge(100));
        let mut optional = base.clone();
        optional.requirement = RequirementClass::Optional;
        assert!(!optional.counts_for_badge(100));
        let mut resolved = base;
        resolved.resolved_at_ms = Some(100);
        assert!(!resolved.counts_for_badge(100));
    }

    #[test]
    fn fold_rejects_noncanonical_duplicate_mismatch_and_expired_rows() {
        let roster = BTreeSet::from(["a".to_string(), "b".to_string()]);
        let mut mismatch = state("b", 1, 100);
        mismatch.grade.node = "alias".into();
        let mut expired = state("b", 2, 0);
        expired.valid_until_ms = 99;
        let snapshot = fold_snapshot(
            "a",
            "r1",
            &roster,
            vec![
                state("a", 1, 100),
                state("a", 2, 100),
                state("retired", 1, 100),
                mismatch,
                expired,
            ],
            5,
            100,
            100,
            1,
        );
        assert!(
            snapshot.current_node_grades.is_empty(),
            "all duplicate rows are rejected"
        );
        assert_eq!(snapshot.mesh_summary.reachable_lighthouses, 1);
        assert_eq!(snapshot.active_conditions[0].id, "mesh:publisher-freshness");
    }

    #[test]
    fn fold_freshness_never_outlives_contract_or_admitted_source() {
        let roster = BTreeSet::from(["node".to_string()]);
        let snapshot = fold_snapshot(
            "node",
            "r1",
            &roster,
            vec![state("node", 1, 100)],
            2,
            100,
            u64::MAX,
            0,
        );
        assert_eq!(snapshot.fresh_until_ms, 200);
        assert!(snapshot.is_fresh(200));
        assert!(!snapshot.is_fresh(201));

        let empty = fold_snapshot(
            "node",
            "r1",
            &BTreeSet::new(),
            Vec::new(),
            3,
            100,
            u64::MAX,
            0,
        );
        assert_eq!(
            empty.fresh_until_ms,
            100 + MAX_NODE_HEALTH_PUBLICATION_TTL_MS,
            "a caller cannot fabricate an unbounded fresh projection"
        );
    }

    #[test]
    fn snapshot_freshness_rejects_future_and_zero_timestamp_projections() {
        let mut future = fold_snapshot(
            "node",
            "r1",
            &BTreeSet::from(["node".to_string()]),
            vec![state("node", 1, 100)],
            2,
            100,
            100,
            0,
        );
        future.generated_at_ms = 101;
        future.fresh_until_ms = 200;
        assert!(
            !future.is_fresh(100),
            "future evidence must not appear current"
        );

        let mut zero = future;
        zero.generated_at_ms = 0;
        zero.fresh_until_ms = 0;
        assert!(
            !zero.is_fresh(0),
            "zero timestamp is an invalid freshness sentinel"
        );
    }

    #[test]
    fn node_health_publication_rejects_schema_skew_and_malformed_timestamps() {
        let valid = state("node", 1, 100);
        assert_eq!(valid.validate_at(100), Ok(()));

        let mut future = valid.clone();
        future.published_at_ms = 101;
        assert_eq!(
            future.validate_at(100),
            Err(NodeHealthValidationError::InvalidTimestamp(
                "published_at_ms"
            ))
        );

        let mut reversed = valid.clone();
        reversed.valid_until_ms = reversed.published_at_ms;
        assert_eq!(
            reversed.validate_at(100),
            Err(NodeHealthValidationError::InvalidTimestamp(
                "valid_until_ms"
            ))
        );

        let mut oversized_ttl = valid.clone();
        oversized_ttl.valid_until_ms = oversized_ttl
            .published_at_ms
            .saturating_add(MAX_NODE_HEALTH_PUBLICATION_TTL_MS + 1);
        assert_eq!(
            oversized_ttl.validate_at(100),
            Err(NodeHealthValidationError::ExpiryTooFar)
        );

        let mut stale = valid.clone();
        stale.valid_until_ms = 150;
        assert_eq!(
            stale.validate_at(151),
            Err(NodeHealthValidationError::Stale)
        );

        let mut skewed = serde_json::to_value(valid).expect("health state serializes");
        skewed["grade"]["future_factor"] = serde_json::json!(100);
        assert!(
            serde_json::from_value::<NodeHealthState>(skewed).is_err(),
            "unknown nested schema fields fail closed"
        );
    }

    #[test]
    fn node_health_publication_rejects_secrets_oversized_evidence_and_bad_lifecycle() {
        let mut producer_shape = state("node", 1, 100);
        let mut producer_condition = condition("node", HealthSeverity::Warning);
        producer_condition.source = "mesh-status/services".into();
        producer_condition.evidence.provider = producer_condition.source.clone();
        producer_shape.active_conditions.push(producer_condition);
        producer_shape.grade = NodeGrade::evaluate(
            "node",
            95,
            GradeFactors::default(),
            &producer_shape.active_conditions,
            100,
        );
        assert_eq!(producer_shape.validate_at(100), Ok(()));

        let mut secret = state("node", 1, 100);
        let mut secret_condition = condition("node", HealthSeverity::Warning);
        secret_condition
            .evidence
            .facts
            .insert("api_token".into(), "not-for-health".into());
        secret.active_conditions.push(secret_condition);
        assert_eq!(
            secret.validate_at(100),
            Err(NodeHealthValidationError::SecretBearing("evidence.facts"))
        );

        let mut pem = state("node", 1, 100);
        let mut pem_condition = condition("node", HealthSeverity::Warning);
        pem_condition.evidence.summary =
            "-----BEGIN PRIVATE KEY----- redacted-looking-but-still-forbidden".into();
        pem.active_conditions.push(pem_condition);
        assert_eq!(
            pem.validate_at(100),
            Err(NodeHealthValidationError::SecretBearing("evidence.summary"))
        );

        let mut oversized = state("node", 1, 100);
        let mut oversized_condition = condition("node", HealthSeverity::Warning);
        oversized_condition.evidence.summary = "x".repeat(MAX_HEALTH_TEXT_BYTES + 1);
        oversized.active_conditions.push(oversized_condition);
        assert_eq!(
            oversized.validate_at(100),
            Err(NodeHealthValidationError::FieldTooLong("evidence.summary"))
        );

        let mut malformed = state("node", 1, 100);
        let mut malformed_condition = condition("node", HealthSeverity::Warning);
        malformed_condition.last_observed_ms = 101;
        malformed.active_conditions.push(malformed_condition);
        assert_eq!(
            malformed.validate_at(100),
            Err(NodeHealthValidationError::InvalidTimestamp(
                "condition.lifecycle"
            ))
        );

        let mut evidence_before_activation = state("node", 1, 100);
        let mut lifecycle_condition = condition("node", HealthSeverity::Warning);
        lifecycle_condition.active_since_ms = 60;
        lifecycle_condition.last_observed_ms = 90;
        lifecycle_condition.evidence.observed_at_ms = 59;
        evidence_before_activation
            .active_conditions
            .push(lifecycle_condition.clone());
        evidence_before_activation.grade = NodeGrade::evaluate(
            "node",
            evidence_before_activation.grade.capability_score,
            evidence_before_activation.grade.factors,
            &evidence_before_activation.active_conditions,
            evidence_before_activation.grade.evaluated_at_ms,
        );
        assert_eq!(
            evidence_before_activation.validate_at(100),
            Err(NodeHealthValidationError::InvalidTimestamp(
                "condition.evidence_lifecycle"
            ))
        );

        lifecycle_condition.evidence.observed_at_ms = 91;
        let mut evidence_after_observation = state("node", 2, 100);
        evidence_after_observation
            .active_conditions
            .push(lifecycle_condition);
        evidence_after_observation.grade = NodeGrade::evaluate(
            "node",
            evidence_after_observation.grade.capability_score,
            evidence_after_observation.grade.factors,
            &evidence_after_observation.active_conditions,
            evidence_after_observation.grade.evaluated_at_ms,
        );
        assert_eq!(
            evidence_after_observation.validate_at(100),
            Err(NodeHealthValidationError::InvalidTimestamp(
                "condition.evidence_lifecycle"
            ))
        );

        let mut wrong_lane = state("node", 1, 100);
        wrong_lane
            .resolved_conditions
            .push(condition("node", HealthSeverity::Warning));
        assert_eq!(
            wrong_lane.validate_at(100),
            Err(NodeHealthValidationError::Contradictory(
                "condition is in the wrong lifecycle lane"
            ))
        );
    }

    #[test]
    fn node_health_publication_rejects_duplicate_active_condition_identity() {
        let mut duplicate = state("node", 1, 100);
        let condition = condition("node", HealthSeverity::Warning);
        duplicate.active_conditions = vec![condition.clone(), condition];
        duplicate.grade = NodeGrade::evaluate(
            "node",
            duplicate.grade.capability_score,
            duplicate.grade.factors,
            &duplicate.active_conditions,
            duplicate.grade.evaluated_at_ms,
        );

        assert_eq!(
            duplicate.validate_at(100),
            Err(NodeHealthValidationError::Contradictory(
                "duplicate active condition identity"
            ))
        );
    }

    #[test]
    fn node_health_publication_rejects_condition_identity_split_across_lifecycle_lanes() {
        let mut ambiguous = state("node", 1, 100);
        let active = condition("node", HealthSeverity::Warning);
        let mut resolved = active.clone();
        resolved.resolved_at_ms = Some(100);
        ambiguous.active_conditions.push(active);
        ambiguous.resolved_conditions.push(resolved);

        assert_eq!(
            ambiguous.validate_at(100),
            Err(NodeHealthValidationError::Contradictory(
                "condition identity appears in both lifecycle lanes"
            ))
        );
    }

    #[test]
    fn node_health_publication_rejects_resolved_history_outside_privacy_epoch() {
        let published_at_ms = MAX_HEALTH_HISTORY_RETENTION_MS + 101;
        let mut publication = state("node", 1, published_at_ms);
        let mut expired_history = condition("node", HealthSeverity::Warning);
        expired_history.resolved_at_ms = Some(100);
        publication.resolved_conditions.push(expired_history);

        assert_eq!(
            publication.validate_at(published_at_ms),
            Err(NodeHealthValidationError::InvalidTimestamp(
                "condition.resolved_at_ms"
            )),
            "a fresh envelope cannot restore condition history older than the privacy epoch"
        );
    }

    #[test]
    fn hostile_resolved_history_cannot_end_before_its_final_observation() {
        let mut publication = state("node", 1, 200);
        let mut contradictory_history = condition("node", HealthSeverity::Warning);
        contradictory_history.last_observed_ms = 180;
        contradictory_history.evidence.observed_at_ms = 180;
        contradictory_history.resolved_at_ms = Some(150);
        publication.resolved_conditions.push(contradictory_history);

        assert_eq!(
            publication.validate_at(200),
            Err(NodeHealthValidationError::InvalidTimestamp(
                "condition.resolved_at_ms"
            )),
            "a hostile publisher cannot move resolution before the condition's final observation"
        );
    }

    #[test]
    fn health_action_result_contract_rejects_malformed_oversized_and_unknown_rows() {
        let valid = HealthActionResult {
            schema_version: HEALTH_SCHEMA_VERSION,
            request_id: "request-1".into(),
            condition_id: "node:disk".into(),
            action: HealthAction::RefreshProvider,
            outcome: HealthActionOutcome::Applied,
            detail: "provider refresh completed".into(),
            audit_id: "health:node:01J00000000000000000000001".into(),
            completed_at_ms: 200,
            snapshot_generation: 2,
            refreshed_evidence: Some(HealthEvidence {
                provider: "fixture".into(),
                summary: "provider is current".into(),
                facts: BTreeMap::new(),
                observed_at_ms: 200,
            }),
        };
        valid.validate_at(200).expect("valid result contract");

        let malformed = HealthActionResult {
            request_id: "../escaped request".into(),
            ..valid.clone()
        };
        assert!(matches!(
            malformed.validate_at(200),
            Err(NodeHealthValidationError::InvalidField(
                "action_result.request_id"
            ))
        ));

        let oversized = HealthActionResult {
            detail: "x".repeat(MAX_HEALTH_TEXT_BYTES + 1),
            ..valid.clone()
        };
        assert!(matches!(
            oversized.validate_at(200),
            Err(NodeHealthValidationError::FieldTooLong(
                "action_result.detail"
            ))
        ));

        let mut unknown = serde_json::to_value(&valid).expect("encode result");
        unknown
            .as_object_mut()
            .expect("result object")
            .insert("untrusted_extension".into(), serde_json::json!(true));
        assert!(serde_json::from_value::<HealthActionResult>(unknown).is_err());

        let future_completion = HealthActionResult {
            completed_at_ms: 201,
            ..valid.clone()
        };
        assert!(future_completion.validate_at(200).is_err());

        let mut future_evidence = valid;
        future_evidence
            .refreshed_evidence
            .as_mut()
            .expect("evidence")
            .observed_at_ms = 201;
        assert!(future_evidence.validate_at(200).is_err());
    }

    #[test]
    fn fold_rejects_contradictory_or_secret_bearing_publications() {
        let roster = BTreeSet::from(["node".to_string()]);
        let mut secret = state("node", 1, 100);
        let mut secret_condition = condition("node", HealthSeverity::Warning);
        secret_condition.evidence.facts.insert(
            "safe_name".into(),
            "authorization: bearer should-not-cross-health".into(),
        );
        secret.active_conditions.push(secret_condition);

        let snapshot = fold_snapshot("node", "r1", &roster, vec![secret], 2, 100, 100, 0);
        assert_eq!(snapshot.mesh_summary.fresh_nodes, 0);
        assert_eq!(snapshot.active_conditions[0].id, "mesh:publisher-freshness");

        let mut contradictory_grade = state("node", 2, 100);
        contradictory_grade.grade.grade = GradeLetter::F;
        assert!(matches!(
            contradictory_grade.validate_at(100),
            Err(NodeHealthValidationError::Contradictory(_))
        ));
    }

    fn kiron_alert(grade: GradeLetter) -> HealthKironAlert {
        HealthKironAlert {
            kind: HealthKironKind::HealthKiron,
            schema_version: HEALTH_KIRON_SCHEMA_VERSION,
            snapshot_generation: 42,
            condition_id: "disk-pressure".into(),
            node: "node-9".into(),
            device: Some("nvme0n1".into()),
            grade,
            headline: "Storage pressure remains active".into(),
            active_since_ms: 10_000,
            observed_at_ms: 80_000,
        }
    }

    #[test]
    fn health_kiron_contract_round_trips_only_authoritative_grades() {
        let cases = [
            (
                GradeLetter::A,
                HealthKironAttention::Informational,
                HealthKironDwell::TimedMs(3_000),
            ),
            (
                GradeLetter::B,
                HealthKironAttention::Informational,
                HealthKironDwell::TimedMs(5_000),
            ),
            (
                GradeLetter::C,
                HealthKironAttention::Warning,
                HealthKironDwell::TimedMs(6_000),
            ),
            (
                GradeLetter::D,
                HealthKironAttention::Warning,
                HealthKironDwell::TimedMs(10_000),
            ),
            (
                GradeLetter::E,
                HealthKironAttention::Critical,
                HealthKironDwell::TimedMs(15_000),
            ),
            (
                GradeLetter::F,
                HealthKironAttention::Critical,
                HealthKironDwell::UntilAcknowledged,
            ),
        ];

        for (grade, attention, dwell) in cases {
            let alert = kiron_alert(grade);
            alert.validate().expect("authoritative alert validates");
            assert_eq!(alert.attention(), attention);
            assert_eq!(alert.dwell(), dwell);
            assert_eq!(alert.duration_label(), "1m 10s");
            let body = serde_json::to_vec(&alert).expect("serialize typed alert");
            let decoded: HealthKironAlert =
                serde_json::from_slice(&body).expect("deserialize typed alert");
            assert_eq!(decoded, alert);
        }
    }

    #[test]
    fn health_kiron_contract_rejects_unbound_or_secret_bearing_payloads() {
        let mut zero_generation = kiron_alert(GradeLetter::F);
        zero_generation.snapshot_generation = 0;
        assert_eq!(
            zero_generation.validate(),
            Err(NodeHealthValidationError::InvalidGeneration)
        );

        let mut reversed_time = kiron_alert(GradeLetter::D);
        reversed_time.active_since_ms = reversed_time.observed_at_ms + 1;
        assert_eq!(
            reversed_time.validate(),
            Err(NodeHealthValidationError::InvalidTimestamp(
                "kiron.lifecycle"
            ))
        );

        let mut secret = kiron_alert(GradeLetter::F);
        secret.headline = "authorization: bearer should-not-render".into();
        assert_eq!(
            secret.validate(),
            Err(NodeHealthValidationError::SecretBearing("headline"))
        );

        let mut secret_device = kiron_alert(GradeLetter::D);
        secret_device.device = Some("token=must-not-render".into());
        assert_eq!(
            secret_device.validate(),
            Err(NodeHealthValidationError::SecretBearing("device"))
        );
    }
}
