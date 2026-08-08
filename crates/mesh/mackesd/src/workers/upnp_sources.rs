//! WL-FUNC-019 — bounded trusted-LAN UPnP/SSDP discovery.
//!
//! This module is the daemon-side handoff point for the approved
//! mdns-sd/rupnp direction. mdns-sd already owns the repository's mDNS
//! lifecycle; SSDP is a separate UDP protocol, and rupnp is not currently
//! present in this checkout. The boundary here is therefore deliberately
//! transport-neutral: a future rupnp socket loop can hand one bounded
//! datagram plus its kernel-selected interface and source address to
//! UpnpDiscoveryAdapter.
//!
//! No socket is opened here and no LOCATION URL is fetched. The adapter
//! performs the security-sensitive work that must remain true when the live
//! rupnp loop is added: explicit interface and source-subnet trust, bounded
//! packet/header/record/concurrency/TTL budgets, strict SSDP grammar, and a
//! typed resource-card projection. The worker is runtime-reachable from the
//! mackesd worker namespace and only prunes its retained roster; live I/O
//! remains an explicit, separately verifiable boundary.

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mackes_mesh_types::resources::{
    ActionAvailability, ActionAvailabilityStatus, AuthMethod, AuthState, AuthStatus,
    ClientBoundary, ClientCapability, ClientCapabilityLimits, ClientFeature, DiscoverySource,
    FailureCode, FailureReason, HealthState, HealthStatus, IdentityAuthority, ProvenanceTrust,
    ResourceAction, ResourceActionTarget, ResourceActionVerb, ResourceAlias, ResourceAliasKind,
    ResourceCard, ResourceClass, ResourceIdentity, ResourceOperatingRole, ResourceScope,
    ResourceValidationError, SourceProvenance, TransportCandidate, TransportEndpoint,
    TransportProtocol, RESOURCE_CONTRACT_VERSION,
};
use serde::{Deserialize, Serialize};

use super::{ShutdownToken, Worker};

/// Retained source topic reserved for the eventual live UPnP publisher.
pub const UPNP_SOURCES_TOPIC: &str = "state/resources/upnp";
/// SSDP's IPv4 discovery group and port.
pub const SSDP_MULTICAST_V4: SocketAddr =
    SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::new(239, 255, 255, 250)), 1_900);

/// Maximum UDP payload accepted by this seam.
pub const MAX_SSDP_PACKET_BYTES: usize = 8 * 1024;
/// Maximum number of header lines accepted in one datagram.
pub const MAX_SSDP_HEADERS: usize = 32;
/// Maximum bytes in one SSDP header line.
pub const MAX_SSDP_HEADER_LINE_BYTES: usize = 1_024;
/// Maximum records retained by one UPnP worker.
pub const MAX_UPNP_RECORDS: usize = 64;
/// Maximum explicitly trusted interfaces in one worker policy.
pub const MAX_UPNP_INTERFACES: usize = 8;
/// Maximum trusted subnets attached to one interface.
pub const MAX_UPNP_SUBNETS_PER_INTERFACE: usize = 16;
/// Maximum simultaneous packet admissions.
pub const MAX_UPNP_IN_FLIGHT: usize = 4;
/// Minimum freshness lifetime accepted from SSDP cache-control.
pub const MIN_UPNP_TTL_MS: u64 = 1_000;
/// Maximum freshness lifetime retained from one SSDP observation.
pub const MAX_UPNP_TTL_MS: u64 = 10 * 60 * 1_000;
/// Maximum encoded retained state accepted before JSON decoding.
pub const MAX_UPNP_STATE_BYTES: usize = 2 * 1024 * 1024;
/// Roster-pruning cadence for the runtime worker.
pub const DEFAULT_PRUNE_INTERVAL: Duration = Duration::from_secs(5);
/// Maximum deterministic startup phase used to avoid synchronized pruning
/// across seats while keeping discovery freshness bounded.
pub const MAX_HOST_PHASE: Duration = Duration::from_millis(1_500);

/// A canonical IPv4/IPv6 network used by the trusted-LAN policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedLanSubnet {
    /// Network address with all host bits clear.
    pub network: IpAddr,
    /// CIDR prefix length.
    pub prefix_len: u8,
}

impl TrustedLanSubnet {
    /// Construct a canonical subnet and reject host bits in the network value.
    pub fn new(network: IpAddr, prefix_len: u8) -> Result<Self, UpnpPolicyError> {
        let valid = match network {
            IpAddr::V4(address) => {
                if prefix_len > 32 {
                    false
                } else {
                    let value = u32::from(address);
                    let mask = ipv4_mask(prefix_len);
                    value & mask == value
                }
            }
            IpAddr::V6(address) => {
                if prefix_len > 128 {
                    false
                } else {
                    let value = u128::from(address);
                    let mask = ipv6_mask(prefix_len);
                    value & mask == value
                }
            }
        };
        if !valid {
            return Err(UpnpPolicyError::InvalidSubnet);
        }
        Ok(Self {
            network,
            prefix_len,
        })
    }

    fn contains(self, candidate: IpAddr) -> bool {
        match (self.network, candidate) {
            (IpAddr::V4(network), IpAddr::V4(candidate)) if self.prefix_len <= 32 => {
                let mask = ipv4_mask(self.prefix_len);
                u32::from(network) & mask == u32::from(candidate) & mask
            }
            (IpAddr::V6(network), IpAddr::V6(candidate)) if self.prefix_len <= 128 => {
                let mask = ipv6_mask(self.prefix_len);
                u128::from(network) & mask == u128::from(candidate) & mask
            }
            _ => false,
        }
    }
}

fn ipv4_mask(prefix_len: u8) -> u32 {
    match prefix_len {
        0 => 0,
        1..=32 => u32::MAX << (32 - u32::from(prefix_len)),
        _ => 0,
    }
}

fn ipv6_mask(prefix_len: u8) -> u128 {
    match prefix_len {
        0 => 0,
        1..=128 => u128::MAX << (128 - u32::from(prefix_len)),
        _ => 0,
    }
}

/// One named interface and its explicitly trusted source networks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedLanInterface {
    /// Kernel interface identity, never inferred from a packet.
    pub name: String,
    /// Source networks permitted on this interface.
    pub subnets: Vec<TrustedLanSubnet>,
}

impl TrustedLanInterface {
    /// Construct a non-empty, bounded interface policy.
    pub fn new(
        name: impl Into<String>,
        subnets: Vec<TrustedLanSubnet>,
    ) -> Result<Self, UpnpPolicyError> {
        let name = name.into();
        if !valid_interface_name(&name) {
            return Err(UpnpPolicyError::InvalidInterface);
        }
        if subnets.is_empty() {
            return Err(UpnpPolicyError::NoSubnets);
        }
        if subnets.len() > MAX_UPNP_SUBNETS_PER_INTERFACE {
            return Err(UpnpPolicyError::TooManySubnets {
                max: MAX_UPNP_SUBNETS_PER_INTERFACE,
            });
        }
        Ok(Self { name, subnets })
    }

    fn allows(&self, source: IpAddr) -> bool {
        self.subnets.iter().copied().any(|subnet| subnet.contains(source))
    }
}

/// Explicit, bounded policy for trusted-LAN SSDP observations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpnpDiscoveryPolicy {
    interfaces: BTreeMap<String, TrustedLanInterface>,
    max_records: usize,
    max_in_flight: usize,
    min_ttl_ms: u64,
    max_ttl_ms: u64,
}

impl UpnpDiscoveryPolicy {
    /// Construct a policy. An empty or unbounded trust policy is rejected.
    pub fn new(
        interfaces: Vec<TrustedLanInterface>,
        max_records: usize,
        max_in_flight: usize,
        min_ttl_ms: u64,
        max_ttl_ms: u64,
    ) -> Result<Self, UpnpPolicyError> {
        if interfaces.is_empty() {
            return Err(UpnpPolicyError::NoInterfaces);
        }
        if interfaces.len() > MAX_UPNP_INTERFACES {
            return Err(UpnpPolicyError::TooManyInterfaces {
                max: MAX_UPNP_INTERFACES,
            });
        }
        if !(1..=MAX_UPNP_RECORDS).contains(&max_records) {
            return Err(UpnpPolicyError::InvalidRecordLimit {
                max: MAX_UPNP_RECORDS,
            });
        }
        if !(1..=MAX_UPNP_IN_FLIGHT).contains(&max_in_flight) {
            return Err(UpnpPolicyError::InvalidConcurrency {
                max: MAX_UPNP_IN_FLIGHT,
            });
        }
        if !(MIN_UPNP_TTL_MS..=MAX_UPNP_TTL_MS).contains(&min_ttl_ms)
            || !(MIN_UPNP_TTL_MS..=MAX_UPNP_TTL_MS).contains(&max_ttl_ms)
            || min_ttl_ms > max_ttl_ms
        {
            return Err(UpnpPolicyError::InvalidTtl);
        }

        let mut by_name = BTreeMap::new();
        for interface in interfaces {
            if by_name.insert(interface.name.clone(), interface).is_some() {
                return Err(UpnpPolicyError::DuplicateInterface);
            }
        }
        Ok(Self {
            interfaces: by_name,
            max_records,
            max_in_flight,
            min_ttl_ms,
            max_ttl_ms,
        })
    }

    /// Build the standard bounded policy for an explicit interface list.
    pub fn default_for(
        interfaces: Vec<TrustedLanInterface>,
    ) -> Result<Self, UpnpPolicyError> {
        Self::new(
            interfaces,
            MAX_UPNP_RECORDS,
            MAX_UPNP_IN_FLIGHT,
            MIN_UPNP_TTL_MS,
            MAX_UPNP_TTL_MS,
        )
    }

    /// Maximum retained records under this policy.
    #[must_use]
    pub const fn max_records(&self) -> usize {
        self.max_records
    }

    /// Maximum concurrent packet admissions under this policy.
    #[must_use]
    pub const fn max_in_flight(&self) -> usize {
        self.max_in_flight
    }

    fn allows_source(&self, interface: &str, source: IpAddr) -> bool {
        self.interfaces
            .get(interface)
            .is_some_and(|policy| policy.allows(source))
    }

    fn ttl_allowed(&self, ttl_ms: u64) -> bool {
        (self.min_ttl_ms..=self.max_ttl_ms).contains(&ttl_ms)
    }
}

/// Policy construction failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpnpPolicyError {
    /// No interface was trusted.
    NoInterfaces,
    /// The interface allowlist exceeded its bound.
    TooManyInterfaces { max: usize },
    /// An interface name was malformed.
    InvalidInterface,
    /// An interface did not carry any source network.
    NoSubnets,
    /// A subnet list exceeded its bound.
    TooManySubnets { max: usize },
    /// A network address or prefix was malformed.
    InvalidSubnet,
    /// The same interface was declared twice.
    DuplicateInterface,
    /// The record cap was zero or too large.
    InvalidRecordLimit { max: usize },
    /// The concurrency cap was zero or too large.
    InvalidConcurrency { max: usize },
    /// The TTL window was outside the shared contract.
    InvalidTtl,
}

impl std::fmt::Display for UpnpPolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoInterfaces => write!(f, "UPnP policy requires an interface"),
            Self::TooManyInterfaces { max } => {
                write!(f, "UPnP policy exceeds the {max}-interface limit")
            }
            Self::InvalidInterface => write!(f, "UPnP policy contains an invalid interface"),
            Self::NoSubnets => write!(f, "UPnP interface requires a trusted subnet"),
            Self::TooManySubnets { max } => {
                write!(f, "UPnP interface exceeds the {max}-subnet limit")
            }
            Self::InvalidSubnet => write!(f, "UPnP policy contains an invalid subnet"),
            Self::DuplicateInterface => write!(f, "UPnP policy repeats an interface"),
            Self::InvalidRecordLimit { max } => {
                write!(f, "UPnP record limit must be between 1 and {max}")
            }
            Self::InvalidConcurrency { max } => {
                write!(f, "UPnP concurrency limit must be between 1 and {max}")
            }
            Self::InvalidTtl => write!(f, "UPnP TTL window is outside the bounded contract"),
        }
    }
}

impl std::error::Error for UpnpPolicyError {}

fn valid_interface_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.trim() == name
        && name.is_ascii()
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'%')
        })
}

/// Metadata a future rupnp receiver must attach to one datagram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsdpPacketContext {
    /// Interface selected by the socket or packet ancillary data.
    pub interface: String,
    /// Kernel-reported sender address.
    pub source: SocketAddr,
    /// Local wall-clock observation time in milliseconds.
    pub observed_at_ms: u64,
}

/// Standard UPnP resource families admitted by this closed adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpnpResourceKind {
    /// The SSDP root-device identity.
    RootDevice,
    /// A UPnP MediaServer device.
    MediaServer,
    /// A UPnP MediaRenderer device.
    MediaRenderer,
    /// An Internet Gateway Device.
    InternetGatewayDevice,
    /// ContentDirectory service.
    ContentDirectory,
    /// ConnectionManager service.
    ConnectionManager,
    /// AVTransport service.
    AvTransport,
    /// WANIPConnection service.
    WanIpConnection,
}

impl UpnpResourceKind {
    fn from_target(target: &str) -> Option<Self> {
        match target.trim().to_ascii_lowercase().as_str() {
            "upnp:rootdevice" => Some(Self::RootDevice),
            "urn:schemas-upnp-org:device:mediaserver:1" => Some(Self::MediaServer),
            "urn:schemas-upnp-org:device:mediarenderer:1" => Some(Self::MediaRenderer),
            "urn:schemas-upnp-org:device:internetgatewaydevice:1" => {
                Some(Self::InternetGatewayDevice)
            }
            "urn:schemas-upnp-org:service:contentdirectory:1" => Some(Self::ContentDirectory),
            "urn:schemas-upnp-org:service:connectionmanager:1" => Some(Self::ConnectionManager),
            "urn:schemas-upnp-org:service:avtransport:1" => Some(Self::AvTransport),
            "urn:schemas-upnp-org:service:wanipconnection:1" => Some(Self::WanIpConnection),
            _ => None,
        }
    }

    const fn token(self) -> &'static str {
        match self {
            Self::RootDevice => "root_device",
            Self::MediaServer => "media_server",
            Self::MediaRenderer => "media_renderer",
            Self::InternetGatewayDevice => "internet_gateway_device",
            Self::ContentDirectory => "content_directory",
            Self::ConnectionManager => "connection_manager",
            Self::AvTransport => "av_transport",
            Self::WanIpConnection => "wan_ip_connection",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::RootDevice => "root device",
            Self::MediaServer => "media server",
            Self::MediaRenderer => "media renderer",
            Self::InternetGatewayDevice => "internet gateway",
            Self::ContentDirectory => "content directory",
            Self::ConnectionManager => "connection manager",
            Self::AvTransport => "AV transport",
            Self::WanIpConnection => "WAN IP connection",
        }
    }

    const fn class(self) -> ResourceClass {
        match self {
            Self::MediaServer | Self::ContentDirectory => ResourceClass::MediaServer,
            Self::MediaRenderer
            | Self::RootDevice
            | Self::InternetGatewayDevice
            | Self::ConnectionManager
            | Self::AvTransport
            | Self::WanIpConnection => ResourceClass::NetworkDevice,
        }
    }
}

/// HTTP scheme retained separately from the resource endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpnpHttpScheme {
    /// Plain HTTP device description.
    Http,
    /// TLS device description.
    Https,
}

impl UpnpHttpScheme {
    const fn default_port(self) -> u16 {
        match self {
            Self::Http => 80,
            Self::Https => 443,
        }
    }

}

/// URL-free, typed representation of an SSDP LOCATION.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpnpLocation {
    /// Scheme from the closed HTTP/HTTPS set.
    pub scheme: UpnpHttpScheme,
    /// Literal IP address; DNS names are rejected at this boundary.
    pub host: IpAddr,
    /// Explicit or scheme-default HTTP port.
    pub port: u16,
    /// Query-free, fragment-free device-description path.
    pub base_path: Option<String>,
}

impl UpnpLocation {
    fn endpoint(&self) -> TransportEndpoint {
        TransportEndpoint::Network {
            host: self.host.to_string(),
            port: self.port,
            base_path: self.base_path.clone(),
        }
    }
}

/// A typed retained source emitted by the bounded SSDP adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpnpSourceRecord {
    /// Stable device UUID plus closed resource-family token.
    pub source_id: String,
    /// Admitted device/service family.
    pub kind: UpnpResourceKind,
    /// Interface on which the trusted observation arrived.
    pub interface: String,
    /// Kernel-reported source address.
    pub source_ip: IpAddr,
    /// Strictly parsed device-description endpoint.
    pub location: UpnpLocation,
    /// Observation timestamp.
    pub observed_at_ms: u64,
    /// Expiry derived from CACHE-CONTROL max-age and the observation time.
    pub expires_at_ms: u64,
}

impl UpnpSourceRecord {
    /// Validate a retained record without consulting a live network.
    pub fn validate(&self) -> Result<(), ResourceValidationError> {
        if !valid_upnp_source_id(&self.source_id) {
            return Err(ResourceValidationError::InvalidField("upnp.source_id"));
        }
        if !valid_interface_name(&self.interface) {
            return Err(ResourceValidationError::InvalidField("upnp.interface"));
        }
        if self.source_ip.is_unspecified()
            || self.source_ip.is_loopback()
            || self.source_ip.is_multicast()
        {
            return Err(ResourceValidationError::InvalidField("upnp.source_ip"));
        }
        if self.location.host != self.source_ip || self.location.port == 0 {
            return Err(ResourceValidationError::InvalidRelationship(
                "upnp.location_source",
            ));
        }
        if self.observed_at_ms == 0
            || self.expires_at_ms <= self.observed_at_ms
            || self.expires_at_ms - self.observed_at_ms
                < MIN_UPNP_TTL_MS
            || self.expires_at_ms - self.observed_at_ms > MAX_UPNP_TTL_MS
        {
            return Err(ResourceValidationError::InvalidTtl("upnp.freshness"));
        }
        if let Some(path) = &self.location.base_path {
            if path.len() > MAX_SSDP_HEADER_LINE_BYTES
                || !path.starts_with('/')
                || path.contains(['?', '#', '\\'])
                || path.chars().any(|character| character.is_ascii_control())
                || path
                    .split('/')
                    .any(|segment| segment == "." || segment == "..")
            {
                return Err(ResourceValidationError::InvalidField(
                    "upnp.location.base_path",
                ));
            }
        }
        Ok(())
    }

    /// Whether this source observation remains fresh at the supplied time.
    #[must_use]
    pub const fn is_fresh(&self, now_ms: u64) -> bool {
        now_ms < self.expires_at_ms
    }
}

fn valid_upnp_source_id(source_id: &str) -> bool {
    let Some(rest) = source_id.strip_prefix("upnp/") else {
        return false;
    };
    let mut pieces = rest.split('/');
    let Some(uuid) = pieces.next() else {
        return false;
    };
    let Some(kind) = pieces.next() else {
        return false;
    };
    pieces.next().is_none()
        && valid_uuid(uuid)
        && matches!(
            kind,
            "root_device"
                | "media_server"
                | "media_renderer"
                | "internet_gateway_device"
                | "content_directory"
                | "connection_manager"
                | "av_transport"
                | "wan_ip_connection"
        )
}

fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.as_bytes().iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

/// Typed adapter errors. Every malformed or over-budget input is rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpnpDiscoveryError {
    /// The packet exceeded the fixed datagram budget.
    PacketTooLarge { bytes: usize, max: usize },
    /// The packet was not valid UTF-8/ASCII SSDP text.
    InvalidEncoding,
    /// The header block or start line was malformed.
    MalformedPacket(&'static str),
    /// A header was repeated or exceeded the count/line budget.
    InvalidHeaders(&'static str),
    /// A header was outside the closed subset used by this adapter.
    UnsupportedHeader,
    /// A required header was absent.
    MissingHeader(&'static str),
    /// The advertised target family is not admitted.
    UnsupportedTarget,
    /// USN identity did not carry a canonical UUID or target binding.
    InvalidUsn,
    /// LOCATION was not a literal, same-source HTTP endpoint.
    InvalidLocation(&'static str),
    /// CACHE-CONTROL did not carry one bounded max-age.
    InvalidCacheControl,
    /// The advertised TTL was outside policy.
    InvalidTtl { ttl_ms: u64 },
    /// Interface or source address was not trusted by policy.
    UntrustedSource { interface: String, source: IpAddr },
    /// A timestamp was unusable.
    InvalidTimestamp,
    /// The caller exceeded the concurrent admission cap.
    ConcurrencyLimit { max: usize },
    /// The retained roster exceeded its record cap.
    RecordLimit { max: usize },
    /// One source identity changed its stable endpoint or family.
    ConflictingIdentity { source_id: String },
    /// A retained record or projected card violated the shared contract.
    InvalidRecord(ResourceValidationError),
}

impl std::fmt::Display for UpnpDiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PacketTooLarge { bytes, max } => {
                write!(f, "SSDP packet is {bytes} bytes; maximum is {max}")
            }
            Self::InvalidEncoding => write!(f, "SSDP packet is not ASCII UTF-8"),
            Self::MalformedPacket(reason) => write!(f, "malformed SSDP packet: {reason}"),
            Self::InvalidHeaders(reason) => write!(f, "invalid SSDP headers: {reason}"),
            Self::UnsupportedHeader => write!(f, "SSDP header is outside the closed subset"),
            Self::MissingHeader(header) => write!(f, "SSDP header is missing: {header}"),
            Self::UnsupportedTarget => write!(f, "SSDP target is not an admitted UPnP family"),
            Self::InvalidUsn => write!(f, "SSDP USN is not a canonical UUID binding"),
            Self::InvalidLocation(reason) => write!(f, "invalid SSDP LOCATION: {reason}"),
            Self::InvalidCacheControl => write!(f, "SSDP CACHE-CONTROL has no bounded max-age"),
            Self::InvalidTtl { ttl_ms } => write!(f, "SSDP TTL is outside policy: {ttl_ms} ms"),
            Self::UntrustedSource { interface, source } => {
                write!(f, "SSDP source {source} is not trusted on {interface}")
            }
            Self::InvalidTimestamp => write!(f, "SSDP observation timestamp is invalid"),
            Self::ConcurrencyLimit { max } => {
                write!(f, "SSDP concurrent admission limit {max} is full")
            }
            Self::RecordLimit { max } => write!(f, "UPnP record limit {max} is full"),
            Self::ConflictingIdentity { source_id } => {
                write!(f, "UPnP source identity changed: {source_id}")
            }
            Self::InvalidRecord(error) => write!(f, "invalid UPnP record: {error}"),
        }
    }
}

impl std::error::Error for UpnpDiscoveryError {}

/// A bounded permit held while one packet crosses the parser boundary.
#[derive(Debug)]
pub struct UpnpDiscoveryPermit {
    active: Arc<AtomicUsize>,
}

impl Drop for UpnpDiscoveryPermit {
    fn drop(&mut self) {
        let _ = self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Runtime-reachable, non-I/O UPnP/SSDP adapter.
#[derive(Debug, Clone)]
pub struct UpnpDiscoveryAdapter {
    policy: UpnpDiscoveryPolicy,
    active: Arc<AtomicUsize>,
}

impl UpnpDiscoveryAdapter {
    /// Construct an adapter with explicit interface, subnet, and budget policy.
    #[must_use]
    pub fn new(policy: UpnpDiscoveryPolicy) -> Self {
        Self {
            active: Arc::new(AtomicUsize::new(0)),
            policy,
        }
    }

    /// Acquire a concurrency permit for a future rupnp receive/parse task.
    pub fn try_acquire(&self) -> Result<UpnpDiscoveryPermit, UpnpDiscoveryError> {
        let mut current = self.active.load(Ordering::Acquire);
        loop {
            if current >= self.policy.max_in_flight {
                return Err(UpnpDiscoveryError::ConcurrencyLimit {
                    max: self.policy.max_in_flight,
                });
            }
            match self.active.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Ok(UpnpDiscoveryPermit {
                        active: Arc::clone(&self.active),
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }

    /// Admit one datagram supplied by a future rupnp socket loop.
    pub fn admit_packet(
        &self,
        packet: &[u8],
        context: &SsdpPacketContext,
    ) -> Result<UpnpSourceRecord, UpnpDiscoveryError> {
        let _permit = self.try_acquire()?;
        self.admit_packet_inner(packet, context)
    }

    fn admit_packet_inner(
        &self,
        packet: &[u8],
        context: &SsdpPacketContext,
    ) -> Result<UpnpSourceRecord, UpnpDiscoveryError> {
        if packet.len() > MAX_SSDP_PACKET_BYTES {
            return Err(UpnpDiscoveryError::PacketTooLarge {
                bytes: packet.len(),
                max: MAX_SSDP_PACKET_BYTES,
            });
        }
        if context.observed_at_ms == 0 {
            return Err(UpnpDiscoveryError::InvalidTimestamp);
        }
        let source = context.source.ip();
        if source.is_unspecified() || source.is_loopback() || source.is_multicast() {
            return Err(UpnpDiscoveryError::UntrustedSource {
                interface: context.interface.clone(),
                source,
            });
        }
        if !self.policy.allows_source(&context.interface, source) {
            return Err(UpnpDiscoveryError::UntrustedSource {
                interface: context.interface.clone(),
                source,
            });
        }
        let (message_kind, headers) = parse_headers(packet)?;
        let target_name = match message_kind {
            SsdpMessageKind::Response => headers
                .get("ST")
                .ok_or(UpnpDiscoveryError::MissingHeader("ST"))?,
            SsdpMessageKind::Alive => {
                if headers.get("NTS").map(String::as_str) != Some("ssdp:alive") {
                    return Err(UpnpDiscoveryError::MalformedPacket("NTS is not ssdp:alive"));
                }
                headers
                    .get("NT")
                    .ok_or(UpnpDiscoveryError::MissingHeader("NT"))?
            }
        };
        let kind =
            UpnpResourceKind::from_target(target_name).ok_or(UpnpDiscoveryError::UnsupportedTarget)?;
        let usn = headers
            .get("USN")
            .ok_or(UpnpDiscoveryError::MissingHeader("USN"))?;
        let source_id = parse_usn(usn, target_name, kind)?;
        let location = parse_location(
            headers
                .get("LOCATION")
                .ok_or(UpnpDiscoveryError::MissingHeader("LOCATION"))?,
            source,
        )?;
        let ttl_ms = parse_max_age(
            headers
                .get("CACHE-CONTROL")
                .ok_or(UpnpDiscoveryError::MissingHeader("CACHE-CONTROL"))?,
        )?;
        if !self.policy.ttl_allowed(ttl_ms) {
            return Err(UpnpDiscoveryError::InvalidTtl { ttl_ms });
        }
        let expires_at_ms = context
            .observed_at_ms
            .checked_add(ttl_ms)
            .ok_or(UpnpDiscoveryError::InvalidTimestamp)?;
        Ok(UpnpSourceRecord {
            source_id,
            kind,
            interface: context.interface.clone(),
            source_ip: source,
            location,
            observed_at_ms: context.observed_at_ms,
            expires_at_ms,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SsdpMessageKind {
    Response,
    Alive,
}

fn parse_headers(
    packet: &[u8],
) -> Result<(SsdpMessageKind, BTreeMap<String, String>), UpnpDiscoveryError> {
    let text = std::str::from_utf8(packet).map_err(|_| UpnpDiscoveryError::InvalidEncoding)?;
    if !text.is_ascii() {
        return Err(UpnpDiscoveryError::InvalidEncoding);
    }
    let header_end = text
        .find("\r\n\r\n")
        .ok_or(UpnpDiscoveryError::MalformedPacket("missing header terminator"))?;
    if header_end + 4 != text.len() {
        return Err(UpnpDiscoveryError::MalformedPacket("SSDP body is not admitted"));
    }
    let header_block = &text[..header_end];
    let mut lines = header_block.split("\r\n");
    let start = lines
        .next()
        .ok_or(UpnpDiscoveryError::MalformedPacket("missing start line"))?;
    let message_kind = if start.eq_ignore_ascii_case("HTTP/1.1 200 OK") {
        SsdpMessageKind::Response
    } else if start.eq_ignore_ascii_case("NOTIFY * HTTP/1.1") {
        SsdpMessageKind::Alive
    } else {
        return Err(UpnpDiscoveryError::MalformedPacket("unsupported start line"));
    };

    let mut headers = BTreeMap::new();
    for (index, line) in lines.enumerate() {
        if index >= MAX_SSDP_HEADERS {
            return Err(UpnpDiscoveryError::InvalidHeaders("header count exceeded"));
        }
        if line.len() > MAX_SSDP_HEADER_LINE_BYTES {
            return Err(UpnpDiscoveryError::InvalidHeaders("header line exceeded"));
        }
        let (raw_name, raw_value) = line
            .split_once(':')
            .ok_or(UpnpDiscoveryError::MalformedPacket("header has no colon"))?;
        if raw_name.is_empty()
            || !raw_name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(UpnpDiscoveryError::InvalidHeaders("invalid header name"));
        }
        let name = raw_name.to_ascii_uppercase();
        if !matches!(
            name.as_str(),
            "CACHE-CONTROL"
                | "LOCATION"
                | "ST"
                | "NT"
                | "NTS"
                | "USN"
                | "SERVER"
                | "DATE"
                | "EXT"
                | "BOOTID.UPNP.ORG"
                | "CONFIGID.UPNP.ORG"
                | "SEQ"
        ) {
            return Err(UpnpDiscoveryError::UnsupportedHeader);
        }
        let value = raw_value.trim();
        if value.is_empty()
            || value.chars().any(|character| character.is_ascii_control())
            || value.len() > MAX_SSDP_HEADER_LINE_BYTES
        {
            return Err(UpnpDiscoveryError::InvalidHeaders("invalid header value"));
        }
        if headers.insert(name, value.to_owned()).is_some() {
            return Err(UpnpDiscoveryError::InvalidHeaders("duplicate header"));
        }
    }
    Ok((message_kind, headers))
}

fn parse_usn(
    usn: &str,
    target: &str,
    kind: UpnpResourceKind,
) -> Result<String, UpnpDiscoveryError> {
    if usn.len() > 128 || usn.trim() != usn || usn.chars().any(char::is_control) {
        return Err(UpnpDiscoveryError::InvalidUsn);
    }
    let (uuid, suffix) = match usn.split_once("::") {
        Some((uuid, suffix)) => (uuid, Some(suffix)),
        None => (usn, None),
    };
    let uuid = uuid
        .strip_prefix("uuid:")
        .ok_or(UpnpDiscoveryError::InvalidUsn)?;
    if !valid_uuid(uuid) {
        return Err(UpnpDiscoveryError::InvalidUsn);
    }
    if suffix.is_some_and(|suffix| !suffix.eq_ignore_ascii_case(target)) {
        return Err(UpnpDiscoveryError::InvalidUsn);
    }
    Ok(format!("upnp/{uuid}/{}", kind.token()))
}

fn parse_max_age(value: &str) -> Result<u64, UpnpDiscoveryError> {
    let mut result = None;
    for directive in value.split(',') {
        let (name, raw_seconds) = directive
            .trim()
            .split_once('=')
            .ok_or(UpnpDiscoveryError::InvalidCacheControl)?;
        if !name.trim().eq_ignore_ascii_case("max-age") || result.is_some() {
            return Err(UpnpDiscoveryError::InvalidCacheControl);
        }
        let seconds: u64 = raw_seconds
            .trim()
            .parse()
            .map_err(|_| UpnpDiscoveryError::InvalidCacheControl)?;
        let ttl_ms = seconds
            .checked_mul(1_000)
            .ok_or(UpnpDiscoveryError::InvalidCacheControl)?;
        if ttl_ms == 0 {
            return Err(UpnpDiscoveryError::InvalidCacheControl);
        }
        result = Some(ttl_ms);
    }
    result.ok_or(UpnpDiscoveryError::InvalidCacheControl)
}

fn parse_location(value: &str, source: IpAddr) -> Result<UpnpLocation, UpnpDiscoveryError> {
    if value.len() > MAX_SSDP_HEADER_LINE_BYTES
        || value.chars().any(|character| character.is_ascii_control() || character.is_whitespace())
    {
        return Err(UpnpDiscoveryError::InvalidLocation("length or whitespace"));
    }
    let (scheme, remainder) = if let Some(remainder) = value.strip_prefix("http://") {
        (UpnpHttpScheme::Http, remainder)
    } else if let Some(remainder) = value.strip_prefix("https://") {
        (UpnpHttpScheme::Https, remainder)
    } else {
        return Err(UpnpDiscoveryError::InvalidLocation("scheme"));
    };
    let authority_end = remainder.find(['/', '?', '#']).unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    if authority.is_empty() || authority.contains('@') {
        return Err(UpnpDiscoveryError::InvalidLocation("authority"));
    }
    let (host, port) = if authority.starts_with('[') {
        let close = authority
            .find(']')
            .ok_or(UpnpDiscoveryError::InvalidLocation("IPv6 bracket"))?;
        let host: IpAddr = authority[1..close]
            .parse()
            .map_err(|_| UpnpDiscoveryError::InvalidLocation("IPv6 host"))?;
        let port = authority[close + 1..]
            .strip_prefix(':')
            .map_or(Ok(scheme.default_port()), |raw| {
                raw.parse()
                    .map_err(|_| UpnpDiscoveryError::InvalidLocation("port"))
            })?;
        (host, port)
    } else {
        if authority.matches(':').count() > 1 {
            return Err(UpnpDiscoveryError::InvalidLocation("unbracketed IPv6"));
        }
        let (raw_host, raw_port) = authority
            .split_once(':')
            .map_or((authority, None), |(host, port)| (host, Some(port)));
        let host: IpAddr = raw_host
            .parse()
            .map_err(|_| UpnpDiscoveryError::InvalidLocation("literal host"))?;
        let port = raw_port.map_or(Ok(scheme.default_port()), |raw| {
            raw.parse()
                .map_err(|_| UpnpDiscoveryError::InvalidLocation("port"))
        })?;
        (host, port)
    };
    if port == 0 || host != source || host.is_unspecified() || host.is_loopback() || host.is_multicast()
    {
        return Err(UpnpDiscoveryError::InvalidLocation("source binding"));
    }
    let suffix = &remainder[authority_end..];
    let base_path = if suffix.is_empty() {
        None
    } else {
        if !suffix.starts_with('/')
            || suffix.contains(['?', '#', '\\'])
            || suffix
                .split('/')
                .any(|segment| segment == "." || segment == "..")
        {
            return Err(UpnpDiscoveryError::InvalidLocation("path"));
        }
        Some(suffix.to_owned())
    };
    Ok(UpnpLocation {
        scheme,
        host,
        port,
        base_path,
    })
}

/// Convert one admitted record into the shared universal resource card.
pub fn resource_card_from_upnp(
    record: &UpnpSourceRecord,
) -> Result<ResourceCard, ResourceValidationError> {
    record.validate()?;
    let capability = ClientCapability::new(
        "construct.dlna-upnp",
        "1",
        TransportProtocol::DlnaUpnp,
        "1",
        ClientBoundary::PlatformAdapter,
        vec![AuthMethod::LocalApproval],
        vec![ClientFeature::MediaBrowse, ClientFeature::Reconnect],
        ClientCapabilityLimits {
            max_width: None,
            max_height: None,
            max_fps: None,
            max_audio_channels: None,
            max_parallel_sessions: 1,
        },
        vec![ResourceActionVerb::Connect],
    )?;
    let health = HealthState {
        schema_version: RESOURCE_CONTRACT_VERSION,
        status: HealthStatus::Available,
        observed_at_ms: record.observed_at_ms,
        expires_at_ms: record.expires_at_ms,
        latency_ms: None,
        failure: None,
    };
    let transport = TransportCandidate::new(
        TransportProtocol::DlnaUpnp,
        record.location.endpoint(),
        ResourceScope::TrustedLan,
        0,
        record.observed_at_ms,
        record.expires_at_ms,
        health.clone(),
        Some(capability.fingerprint.clone()),
    )?;
    let uuid = record
        .source_id
        .strip_prefix("upnp/")
        .and_then(|value| value.split('/').next())
        .ok_or(ResourceValidationError::InvalidField("upnp.source_id"))?;
    let identity = ResourceIdentity::new(
        record.kind.class(),
        IdentityAuthority::Device,
        record.source_id.clone(),
        vec![ResourceAlias {
            kind: ResourceAliasKind::DeviceUuid,
            value: uuid.to_owned(),
        }],
    )?;
    let display_name = format!("UPnP {} at {}", record.kind.label(), record.location.host);
    let summary = format!(
        "Trusted-LAN SSDP observation · {}:{}",
        record.location.host, record.location.port
    );
    let connect_action = ResourceAction {
        schema_version: RESOURCE_CONTRACT_VERSION,
        action_id: "connect-dlna-upnp".to_owned(),
        verb: ResourceActionVerb::Connect,
        target: ResourceActionTarget::TransportClient {
            transport_fingerprint: transport.fingerprint.clone(),
            capability_fingerprint: capability.fingerprint.clone(),
        },
        availability: ActionAvailability {
            status: ActionAvailabilityStatus::Unavailable,
            failure: Some(FailureReason {
                code: FailureCode::MissingClient,
                message: "UPnP discovery is admitted; mackesd session handoff is not wired".into(),
            }),
        },
        issued_at_ms: record.observed_at_ms,
        expires_at_ms: record.expires_at_ms,
    };
    let card = ResourceCard {
        schema_version: RESOURCE_CONTRACT_VERSION,
        identity,
        display_name,
        summary: Some(summary),
        first_seen_at_ms: record.observed_at_ms,
        last_seen_at_ms: record.observed_at_ms,
        expires_at_ms: record.expires_at_ms,
        health,
        auth: AuthState {
            schema_version: RESOURCE_CONTRACT_VERSION,
            status: AuthStatus::NotRequired,
            accepted_methods: vec![],
            active_method: None,
            credential_ref: None,
            updated_at_ms: record.observed_at_ms,
            expires_at_ms: None,
            failure: None,
        },
        provenance: vec![SourceProvenance {
            schema_version: RESOURCE_CONTRACT_VERSION,
            source: DiscoverySource::SsdpUpnp,
            source_id: record.source_id.clone(),
            scope: ResourceScope::TrustedLan,
            trust: ProvenanceTrust::ObservedLan,
            interface: Some(record.interface.clone()),
            observed_at_ms: record.observed_at_ms,
            expires_at_ms: record.expires_at_ms,
        }],
        transports: vec![transport],
        client_capabilities: vec![capability],
        actions: vec![
            ResourceAction {
                schema_version: RESOURCE_CONTRACT_VERSION,
                action_id: "inspect".to_owned(),
                verb: ResourceActionVerb::Inspect,
                target: ResourceActionTarget::Resource,
                availability: ActionAvailability {
                    status: ActionAvailabilityStatus::Ready,
                    failure: None,
                },
                issued_at_ms: record.observed_at_ms,
                expires_at_ms: record.expires_at_ms,
            },
            connect_action,
        ],
        operating_roles: vec![ResourceOperatingRole::Client],
        service: None,
    };
    card.validate()?;
    Ok(card)
}

/// Append retained UPnP cards to the universal catalog, folding exact
/// duplicate observations and rejecting identity collisions with existing
/// resource cards.
pub fn append_upnp_cards(
    cards: &mut Vec<ResourceCard>,
    state: &UpnpSourcesState,
) -> Result<(), ResourceValidationError> {
    state
        .validate()
        .map_err(|_| ResourceValidationError::InvalidField("upnp.source_state"))?;
    let mut source_cards = BTreeMap::<String, ResourceCard>::new();
    for source in &state.sources {
        let card = resource_card_from_upnp(source)?;
        let resource_id = card.resource_id().to_owned();
        match source_cards.entry(resource_id) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(card);
            }
            std::collections::btree_map::Entry::Occupied(entry) => {
                if entry.get() != &card {
                    return Err(ResourceValidationError::InvalidRelationship(
                        "upnp.conflicting_duplicate",
                    ));
                }
            }
        }
    }
    let mut existing_ids: std::collections::BTreeSet<String> = cards
        .iter()
        .map(|card| card.resource_id().to_owned())
        .collect();
    for (resource_id, card) in source_cards {
        if !existing_ids.insert(resource_id) {
            return Err(ResourceValidationError::InvalidRelationship(
                "upnp.catalog_identity_collision",
            ));
        }
        cards.push(card);
    }
    Ok(())
}

/// Retained latest UPnP source roster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpnpSourcesState {
    /// Publishing node identity.
    pub node: String,
    /// Bounded source records.
    pub sources: Vec<UpnpSourceRecord>,
    /// State publication time.
    pub published_at_ms: u64,
}

impl UpnpSourcesState {
    /// Validate state and every nested typed card boundary.
    pub fn validate(&self) -> Result<(), UpnpDiscoveryError> {
        if !valid_interface_name(&self.node) || self.published_at_ms == 0 {
            return Err(UpnpDiscoveryError::InvalidRecord(
                ResourceValidationError::InvalidField("upnp.state"),
            ));
        }
        if self.sources.len() > MAX_UPNP_RECORDS {
            return Err(UpnpDiscoveryError::RecordLimit {
                max: MAX_UPNP_RECORDS,
            });
        }
        let mut ids = std::collections::BTreeSet::new();
        for source in &self.sources {
            if !ids.insert(&source.source_id) {
                return Err(UpnpDiscoveryError::ConflictingIdentity {
                    source_id: source.source_id.clone(),
                });
            }
            if source.observed_at_ms > self.published_at_ms {
                return Err(UpnpDiscoveryError::InvalidTimestamp);
            }
            source
                .validate()
                .map_err(UpnpDiscoveryError::InvalidRecord)?;
            resource_card_from_upnp(source).map_err(UpnpDiscoveryError::InvalidRecord)?;
        }
        Ok(())
    }
}

/// Decode a retained roster with a pre-allocation body bound.
pub fn decode_sources_state(body: &str) -> Result<UpnpSourcesState, UpnpDiscoveryError> {
    if body.len() > MAX_UPNP_STATE_BYTES {
        return Err(UpnpDiscoveryError::PacketTooLarge {
            bytes: body.len(),
            max: MAX_UPNP_STATE_BYTES,
        });
    }
    let state: UpnpSourcesState =
        serde_json::from_str(body).map_err(|_| UpnpDiscoveryError::InvalidEncoding)?;
    state.validate()?;
    Ok(state)
}

/// In-memory bounded retained roster used by the worker and future publisher.
#[derive(Debug, Clone)]
pub struct UpnpRoster {
    max_records: usize,
    records: BTreeMap<String, UpnpSourceRecord>,
}

impl UpnpRoster {
    /// Construct a roster with the same record bound as the adapter policy.
    pub fn new(max_records: usize) -> Result<Self, UpnpDiscoveryError> {
        if !(1..=MAX_UPNP_RECORDS).contains(&max_records) {
            return Err(UpnpDiscoveryError::RecordLimit {
                max: MAX_UPNP_RECORDS,
            });
        }
        Ok(Self {
            max_records,
            records: BTreeMap::new(),
        })
    }

    /// Admit or refresh one record, rejecting identity changes.
    pub fn admit(&mut self, record: UpnpSourceRecord) -> Result<(), UpnpDiscoveryError> {
        record
            .validate()
            .map_err(UpnpDiscoveryError::InvalidRecord)?;
        if let Some(existing) = self.records.get(&record.source_id) {
            let same_identity = existing.kind == record.kind
                && existing.interface == record.interface
                && existing.source_ip == record.source_ip
                && existing.location == record.location;
            if !same_identity {
                return Err(UpnpDiscoveryError::ConflictingIdentity {
                    source_id: record.source_id,
                });
            }
            if record.observed_at_ms >= existing.observed_at_ms {
                self.records.insert(record.source_id.clone(), record);
            }
            return Ok(());
        }
        if self.records.len() >= self.max_records {
            return Err(UpnpDiscoveryError::RecordLimit {
                max: self.max_records,
            });
        }
        self.records.insert(record.source_id.clone(), record);
        Ok(())
    }

    /// Remove observations whose TTL has elapsed.
    pub fn prune_expired(&mut self, now_ms: u64) {
        self.records
            .retain(|_, record| record.is_fresh(now_ms));
    }

    /// Return deterministic source order.
    #[must_use]
    pub fn snapshot(&self) -> Vec<UpnpSourceRecord> {
        self.records.values().cloned().collect()
    }
}

/// Runtime worker wrapper. It owns only bounded retained state; a future
/// rupnp receiver can call ingest_packet through the same worker instance.
#[derive(Debug)]
pub struct UpnpSourcesWorker {
    node: String,
    adapter: UpnpDiscoveryAdapter,
    roster: UpnpRoster,
}

impl UpnpSourcesWorker {
    /// Construct a worker with an explicit node identity and trust policy.
    pub fn new(node: impl Into<String>, policy: UpnpDiscoveryPolicy) -> Result<Self, UpnpDiscoveryError> {
        let adapter = UpnpDiscoveryAdapter::new(policy.clone());
        let roster = UpnpRoster::new(policy.max_records())?;
        Ok(Self {
            node: node.into(),
            adapter,
            roster,
        })
    }

    /// Admit one future-rupnp datagram into the retained roster.
    pub fn ingest_packet(
        &mut self,
        packet: &[u8],
        context: &SsdpPacketContext,
    ) -> Result<(), UpnpDiscoveryError> {
        let record = self.adapter.admit_packet(packet, context)?;
        self.roster.prune_expired(context.observed_at_ms);
        self.roster.admit(record)
    }

    /// Build the strict retained state snapshot.
    pub fn state(&self, published_at_ms: u64) -> Result<UpnpSourcesState, UpnpDiscoveryError> {
        let state = UpnpSourcesState {
            node: self.node.clone(),
            sources: self.roster.snapshot(),
            published_at_ms,
        };
        state.validate()?;
        Ok(state)
    }

    /// Number of currently retained records.
    #[must_use]
    pub fn source_count(&self) -> usize {
        self.roster.records.len()
    }
}

#[async_trait::async_trait]
impl Worker for UpnpSourcesWorker {
    fn name(&self) -> &'static str {
        "upnp_sources"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let first_delay = initial_phase_for(&self.node);
        tokio::select! {
            _ = shutdown.wait() => return Ok(()),
            _ = tokio::time::sleep(first_delay) => {}
        }
        let mut tick = tokio::time::interval(DEFAULT_PRUNE_INTERVAL);
        loop {
            tokio::select! {
                _ = shutdown.wait() => break,
                _ = tick.tick() => {
                    self.roster.prune_expired(now_ms());
                }
            }
        }
        Ok(())
    }
}

/// Stable FNV-1a phase derived from the node identity. A stable phase keeps a
/// seat's cadence predictable while distributing a fleet's first prune.
#[must_use]
pub fn initial_phase_for(node: &str) -> Duration {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in node.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3_u64);
    }
    let cap_ms = u64::try_from(MAX_HOST_PHASE.as_millis()).unwrap_or(0);
    let phase_ms = if cap_ms == 0 { 0 } else { hash % (cap_ms + 1) };
    Duration::from_millis(phase_ms)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::resources::{
        ActionAvailabilityStatus, FailureCode, ResourceClass, TransportProtocol,
    };

    const NOW: u64 = 1_700_000_000_000;
    const DEVICE_UUID: &str = "2f402f80-da50-11e1-9b23-00025b00a001";

    fn policy() -> UpnpDiscoveryPolicy {
        let subnet = TrustedLanSubnet::new("172.20.146.0".parse().unwrap(), 24).unwrap();
        UpnpDiscoveryPolicy::default_for(vec![
            TrustedLanInterface::new("enp0s31f6", vec![subnet]).unwrap(),
        ])
        .unwrap()
    }

    fn context(observed_at_ms: u64) -> SsdpPacketContext {
        SsdpPacketContext {
            interface: "enp0s31f6".into(),
            source: "172.20.146.20:1900".parse().unwrap(),
            observed_at_ms,
        }
    }

    fn media_server_packet(cache_control: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 200 OK\r\n\
CACHE-CONTROL: {cache_control}\r\n\
LOCATION: http://172.20.146.20:8200/rootDesc.xml\r\n\
ST: urn:schemas-upnp-org:device:MediaServer:1\r\n\
USN: uuid:{DEVICE_UUID}::urn:schemas-upnp-org:device:MediaServer:1\r\n\
SERVER: Linux/6.1 UPnP/1.0 MCNF/1.0\r\n\r\n"
        )
        .into_bytes()
    }

    #[test]
    fn trusted_response_becomes_typed_card_without_claiming_connectivity() {
        let adapter = UpnpDiscoveryAdapter::new(policy());
        let record = adapter
            .admit_packet(
                &media_server_packet("max-age=120"),
                &context(NOW),
            )
            .unwrap();
        assert_eq!(record.kind, UpnpResourceKind::MediaServer);
        assert_eq!(record.location.port, 8200);
        assert_eq!(record.expires_at_ms, NOW + 120_000);

        let card = resource_card_from_upnp(&record).unwrap();
        assert_eq!(card.identity.class, ResourceClass::MediaServer);
        assert_eq!(card.transports[0].protocol, TransportProtocol::DlnaUpnp);
        assert_eq!(card.health.status, HealthStatus::Available);
        let connect = card
            .actions
            .iter()
            .find(|action| action.verb == ResourceActionVerb::Connect)
            .unwrap();
        assert_eq!(
            connect.availability.status,
            ActionAvailabilityStatus::Unavailable
        );
        assert_eq!(
            connect.availability.failure.as_ref().unwrap().code,
            FailureCode::MissingClient
        );
        card.validate().unwrap();
    }

    #[test]
    fn interface_and_source_subnet_are_both_required() {
        let adapter = UpnpDiscoveryAdapter::new(policy());
        let mut wrong_interface = context(NOW);
        wrong_interface.interface = "wlan0".into();
        assert!(matches!(
            adapter.admit_packet(&media_server_packet("max-age=120"), &wrong_interface),
            Err(UpnpDiscoveryError::UntrustedSource { .. })
        ));

        let mut wrong_source = context(NOW);
        wrong_source.source = "192.168.1.20:1900".parse().unwrap();
        assert!(matches!(
            adapter.admit_packet(&media_server_packet("max-age=120"), &wrong_source),
            Err(UpnpDiscoveryError::UntrustedSource { .. })
        ));
    }

    #[test]
    fn malformed_headers_and_locations_fail_closed() {
        let adapter = UpnpDiscoveryAdapter::new(policy());
        let mut duplicate = media_server_packet("max-age=120");
        duplicate.splice(
            duplicate.len() - 2..,
            b"LOCATION: http://172.20.146.20:8200/rootDesc.xml\r\n\r\n"
                .iter()
                .copied(),
        );
        assert!(matches!(
            adapter.admit_packet(&duplicate, &context(NOW)),
            Err(UpnpDiscoveryError::InvalidHeaders("duplicate header"))
        ));

        let mut bad_location = media_server_packet("max-age=120");
        let old = b"http://172.20.146.20:8200/rootDesc.xml";
        let new = b"http://172.20.146.20:8200/rootDesc.xml?secret=1";
        let start = bad_location
            .windows(old.len())
            .position(|window| window == old)
            .unwrap();
        bad_location.splice(start..start + old.len(), new.iter().copied());
        assert!(matches!(
            adapter.admit_packet(&bad_location, &context(NOW)),
            Err(UpnpDiscoveryError::InvalidLocation("path"))
        ));

        let oversized = vec![b'x'; MAX_SSDP_PACKET_BYTES + 1];
        assert!(matches!(
            adapter.admit_packet(&oversized, &context(NOW)),
            Err(UpnpDiscoveryError::PacketTooLarge { .. })
        ));
    }

    #[test]
    fn ttl_record_and_concurrency_budgets_are_enforced() {
        let subnet = TrustedLanSubnet::new("172.20.146.0".parse().unwrap(), 24).unwrap();
        let strict = UpnpDiscoveryPolicy::new(
            vec![TrustedLanInterface::new("enp0s31f6", vec![subnet]).unwrap()],
            1,
            1,
            5_000,
            30_000,
        )
        .unwrap();
        let adapter = UpnpDiscoveryAdapter::new(strict.clone());
        assert!(matches!(
            adapter.admit_packet(&media_server_packet("max-age=1"), &context(NOW)),
            Err(UpnpDiscoveryError::InvalidTtl { .. })
        ));
        assert!(matches!(
            adapter.admit_packet(&media_server_packet("max-age=31"), &context(NOW)),
            Err(UpnpDiscoveryError::InvalidTtl { .. })
        ));

        let permit = adapter.try_acquire().unwrap();
        assert!(matches!(
            adapter.try_acquire(),
            Err(UpnpDiscoveryError::ConcurrencyLimit { max: 1 })
        ));
        drop(permit);
        let first = adapter
            .admit_packet(&media_server_packet("max-age=10"), &context(NOW))
            .unwrap();
        let mut roster = UpnpRoster::new(1).unwrap();
        roster.admit(first).unwrap();
        let mut second_packet = media_server_packet("max-age=10");
        let old = DEVICE_UUID.as_bytes();
        let new = b"3f402f80-da50-11e1-9b23-00025b00a001";
        let start = second_packet
            .windows(old.len())
            .position(|window| window == old)
            .unwrap();
        second_packet.splice(start..start + old.len(), new.iter().copied());
        let second = adapter
            .admit_packet(&second_packet, &context(NOW + 1))
            .unwrap();
        assert!(matches!(
            roster.admit(second),
            Err(UpnpDiscoveryError::RecordLimit { max: 1 })
        ));

        roster.prune_expired(NOW + 11_000);
        assert_eq!(roster.snapshot().len(), 0);
    }

    #[test]
    fn worker_state_round_trip_is_strict_and_deterministic() {
        let mut worker = UpnpSourcesWorker::new("t480", policy()).unwrap();
        worker
            .ingest_packet(&media_server_packet("max-age=120"), &context(NOW))
            .unwrap();
        assert_eq!(worker.source_count(), 1);
        let state = worker.state(NOW + 1).unwrap();
        let encoded = serde_json::to_string(&state).unwrap();
        let decoded = decode_sources_state(&encoded).unwrap();
        assert_eq!(decoded, state);
        assert!(decode_sources_state(
            &encoded.replace(
                "\"published_at_ms\":1700000000001",
                "\"published_at_ms\":1700000000001,\"unexpected\":true"
            )
        )
        .is_err());
    }

    #[test]
    fn startup_phase_is_stable_and_bounded() {
        let first = initial_phase_for("seat-15");
        assert_eq!(first, initial_phase_for("seat-15"));
        assert!(first <= MAX_HOST_PHASE);
        assert_ne!(first, initial_phase_for("seat-16"));
    }
}
