//! CHOOSER-1 — the mackesd **desktop-source discovery aggregator**.
//!
//! Design: `docs/design/desktop-chooser.md` (§Architecture, locks 5/14). The
//! Chooser surface (CHOOSER-2, `mde-shell-egui`) renders ONE list of every
//! discovered desktop source; this worker is the mesh-side (§6) collector
//! that builds it. Four discovery lanes, folded into one deduped roster
//! published to [`SOURCES_TOPIC`] (`state/desktops/sources`):
//!
//! 1. **Mesh registry (peer-advertised).** Every node ALREADY advertises what
//!    desktops it serves through the replicated peers plane
//!    (`mackes_mesh_types::peers::PeerRecord`, PD-2): its own seat's RDP/VNC
//!    listeners (`descriptors.remote_access`) and the VM desktops it hosts
//!    (`descriptors.vms`). The small advertised shape is
//!    [`AdvertisedDesktop`]; the pure fold [`advertised_from_peer`] lifts it
//!    from a peer's published record — no second advertisement channel is
//!    minted (§6 glue over the existing plane).
//! 2. **mDNS (LAN).** RDP (`_rdp._tcp`), VNC (`_rfb._tcp`) and Spice
//!    (`_spice._tcp`) endpoints browsed with the SAME `mdns-sd` machinery the
//!    `mdns_relay` worker uses — including its anti-loop `mde-relay-origin`
//!    TXT guard, so a peer-republished service never double-counts against
//!    the mesh-registry lane.
//! 3. **Local KVM.** This node's VM workloads via the authoritative
//!    `state/workloads/<node>` projection. The chooser never invokes `virsh`
//!    or owns a second lifecycle roster; every VM row is derived from the
//!    Workload status and carries its independent power/readiness state.
//! 4. **Manual.** Operator-added `host:port` + protocol endpoints, drained
//!    off the typed `action/desktops/{add-source,remove-source}` verbs (§9 —
//!    a typed body, never a command string) and persisted node-locally;
//!    `action/desktops/refresh` forces a re-enumerate + republish.
//!
//! **Reachability is derived, never probed** (lock 14): peer sources fold
//! roster presence + health, VM sources fold power state, mDNS entries are
//! live-by-presence (the daemon's TTL expiry removes them), and manual
//! entries are honestly `Unknown`. **Live KVM enumeration is honestly gated**
//! (§7, mirroring `mesh_mount`): no `virsh` on the box → a typed
//! [`VmEnumerateError::Gated`], surfaced in the published per-lane status —
//! never a faked (or silently missing) source. The `thumbnail_ref` field
//! ships now, honestly empty (`null`), for CHOOSER-3 to fill.

#![cfg(feature = "async-services")]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::peers::{peers_dir, read_peers, PeerRecord};
use mackes_mesh_types::resources::{
    ActionAvailability, ActionAvailabilityStatus, AuthMethod, AuthState, AuthStatus,
    ClientBoundary, ClientCapability, ClientCapabilityLimits, ClientFeature, DiscoverySource,
    FailureCode, FailureReason, HealthState, HealthStatus, IdentityAuthority, ProvenanceTrust,
    ResourceAction, ResourceActionTarget, ResourceActionVerb, ResourceAlias, ResourceAliasKind,
    ResourceCard, ResourceClass, ResourceIdentity, ResourceScope, ResourceValidationError,
    SourceProvenance, TransportCandidate, TransportEndpoint, TransportProtocol,
    MIN_RESOURCE_TTL_MS, RESOURCE_CONTRACT_VERSION,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use super::{ShutdownToken, Worker};
use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};
use mackes_mesh_types::workloads::{
    workload_state_topic, WorkloadBackend, WorkloadOperationPhase, WorkloadPowerState,
    WorkloadStateSnapshot,
};

/// The retained-latest state topic the merged source roster is published to.
/// The Chooser surface (CHOOSER-2) reads the newest record off this topic.
pub const SOURCES_TOPIC: &str = "state/desktops/sources";

/// Typed verb: add a manual desktop source (`action/<domain>/<verb>`, §9).
pub const ADD_SOURCE_TOPIC: &str = "action/desktops/add-source";

/// Typed verb: remove a previously-added manual source by its id.
pub const REMOVE_SOURCE_TOPIC: &str = "action/desktops/remove-source";

/// Typed verb: force a re-enumerate + republish (the operator's refresh).
pub const REFRESH_TOPIC: &str = "action/desktops/refresh";

/// Shared-Bus capability verbs for the two manual-source mutations. Refresh is
/// intentionally not listed: it only re-enumerates read-only discovery lanes
/// and republishes the derived roster, so it remains an open harmless nudge.
const DESKTOP_ADD_SOURCE_AUTH_VERB: &str = "desktop-add-source";
const DESKTOP_REMOVE_SOURCE_AUTH_VERB: &str = "desktop-remove-source";

/// Action-drain cadence. Discovery is human-paced; a 2 s poll keeps verb
/// latency imperceptible without spinning virsh or the peers plane.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(2);

/// Republish heartbeat.
///
/// Between heartbeats the roster publishes only when the fold changed; once
/// elapsed it republishes unconditionally so a late subscriber /
/// freshly-pruned topic still finds a recent record (mirrors
/// `vm_lifecycle`'s publish gating).
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(30);

/// Bound the recurring-scan phase so a desktop-source refresh remains within
/// the existing two-second action-poll deadline. The phase is deterministic
/// per node rather than process-random, so seats that restart together do not
/// repeatedly recreate the same common-mode Workload/read-publication burst.
pub const MAX_INITIAL_PHASE: Duration = Duration::from_millis(1_500);

/// A peer record older than this is treated as gone (belt-and-braces over the
/// health-reconciler's `health` field, which is the primary authority).
pub const PEER_STALE_MS: u64 = 10 * 60 * 1000;

/// The mDNS service types the desktop lanes browse (design lock 5): RDP,
/// VNC (`_rfb` is the RFB protocol's registered type), and Spice.
pub const DESKTOP_MDNS_TYPES: &[&str] = &["_rdp._tcp", "_rfb._tcp", "_spice._tcp"];

/// Filename of the node-local manual-source store (under the store root).
/// CHOOSER-9 later lifts manual sources onto the mesh-synced plane; the
/// node-local file keeps them durable across restarts today.
pub const MANUAL_STORE_FILE: &str = "manual-sources.json";

/// Manual sources are compact endpoint records. Keep the persisted collection
/// bounded before JSON parsing while leaving ample room for a large operator
/// roster.
const MAX_MANUAL_STORE_BYTES: usize = 1024 * 1024;

// ───────────────────────────── data model ─────────────────────────────

/// A desktop-session protocol a source can be connected over.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum DesktopProtocol {
    /// Remote Desktop Protocol (`mde-vdi-rdp`).
    Rdp,
    /// VNC / RFB (`mde-vdi-vnc`).
    Vnc,
    /// Spice (`mde-vdi-spice`, CHOOSER-5).
    Spice,
}

impl DesktopProtocol {
    /// Stable wire/log tag.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Rdp => "rdp",
            Self::Vnc => "vnc",
            Self::Spice => "spice",
        }
    }

    /// The protocol's well-known default port, when one exists. Spice has no
    /// canonical default (libvirt autoports it), so it is honestly `None` —
    /// a Spice endpoint's port must come from discovery or the operator.
    #[must_use]
    pub const fn default_port(self) -> Option<u16> {
        match self {
            Self::Rdp => Some(3389),
            Self::Vnc => Some(5900),
            Self::Spice => None,
        }
    }

    /// Map a bare mDNS service type onto its desktop protocol (`None` for a
    /// non-desktop type).
    #[must_use]
    pub fn from_mdns_type(bare: &str) -> Option<Self> {
        match bare {
            "_rdp._tcp" => Some(Self::Rdp),
            "_rfb._tcp" => Some(Self::Vnc),
            "_spice._tcp" => Some(Self::Spice),
            _ => None,
        }
    }
}

// ───────────────────── future SSDP/UPnP adapter seam ─────────────────────

/// Maximum raw SSDP header block accepted by the pure MCNF adapter seam.
pub const MAX_SSDP_HEADER_BLOCK_BYTES: usize = 16 * 1024;
/// Maximum number of headers admitted in one SSDP block or typed map.
pub const MAX_SSDP_HEADERS: usize = 32;

/// Closed MCNF service types accepted by the future trusted-LAN SSDP lane.
///
/// These are deliberately not arbitrary UPnP service types. A future `rupnp`
/// worker must translate a packet into this vocabulary before this seam is
/// called; it must not pass through a `LOCATION` URL or invent a new desktop
/// protocol at this boundary.
pub const MCNF_SSDP_RDP_SERVICE_TYPE: &str = "urn:mcnf:desktop:rdp:1";
/// Closed MCNF VNC service type.
pub const MCNF_SSDP_VNC_SERVICE_TYPE: &str = "urn:mcnf:desktop:vnc:1";
/// Closed MCNF Spice service type.
pub const MCNF_SSDP_SPICE_SERVICE_TYPE: &str = "urn:mcnf:desktop:spice:1";

/// A typed header map accepted by [`normalize_ssdp_header_map`].
///
/// Header names are normalized case-insensitively by the adapter. A map can
/// still contain both `NT` and `nt`; if their values conflict, normalization
/// fails instead of letting a later entry win.
pub type SsdpHeaderMap = BTreeMap<String, String>;

/// Explicit state supplied by a trusted caller after its own interface and
/// policy checks. The parser never derives either field from SSDP presence.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SsdpObservation {
    /// Trust selected by the caller's trusted-LAN/mesh policy, if any.
    pub trust: Option<ProvenanceTrust>,
    /// Reachability selected by the caller, if it has an independent result.
    pub reachability: Option<Reachability>,
}

/// A normalized MCNF desktop advertisement.
///
/// This is intentionally not a [`DesktopSource`]. `SourceOrigin` has no SSDP
/// variant, and publication into the desktop roster remains gated until a
/// future `rupnp` worker supplies bounded interface scope, trust policy, TTL
/// handling, and a reviewed catalog projection. In particular, parsing this
/// value is not live discovery and performs no socket, scan, retry, or launch
/// operation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SsdpDesktopAdvertisement {
    /// Stable UUID root from the SSDP `USN`; a service suffix is discarded.
    pub source_id: String,
    /// Operator/display label from `X-MCNF-NAME`.
    pub display_name: String,
    /// Hostname or literal IP from `X-MCNF-HOST` (never a URL or host:port).
    pub host: String,
    /// Explicit desktop listener port from `X-MCNF-PORT`.
    pub port: u16,
    /// One of the three closed MCNF desktop protocols.
    pub protocol: DesktopProtocol,
    /// Caller-supplied trust, never inferred from an advertisement.
    pub trust: Option<ProvenanceTrust>,
    /// Caller-supplied reachability, never inferred from an advertisement.
    pub reachability: Option<Reachability>,
}

impl SsdpDesktopAdvertisement {
    /// Revalidate a decoded advertisement before it crosses into publication.
    ///
    /// The parser already performs these checks, but the struct is public and
    /// callers may construct it directly. Publication must therefore validate
    /// the closed identity/host/name/port boundary again instead of treating a
    /// deserialized value as trusted merely because its Rust type is known.
    pub fn validate(&self) -> Result<(), SsdpAdvertisementError> {
        let service_type = match self.protocol {
            DesktopProtocol::Rdp => MCNF_SSDP_RDP_SERVICE_TYPE,
            DesktopProtocol::Vnc => MCNF_SSDP_VNC_SERVICE_TYPE,
            DesktopProtocol::Spice => MCNF_SSDP_SPICE_SERVICE_TYPE,
        };
        parse_ssdp_identity(&self.source_id, service_type)?;
        validate_ssdp_text("display name", &self.display_name, SSDP_NAME_MAX_BYTES)?;
        validate_ssdp_host(&self.host)?;
        if self.port == 0 {
            return Err(SsdpAdvertisementError::InvalidPort);
        }
        Ok(())
    }
}

/// Maximum freshness window accepted for a trusted-LAN SSDP publication.
///
/// The shared resource contract permits a longer generic TTL, but SSDP
/// presence is a local-network observation and must expire quickly unless a
/// future reviewed runtime supplies a stronger retention policy.
pub const MAX_SSDP_PUBLICATION_TTL_MS: u64 = 10 * 60 * 1_000;

/// Explicit caller evidence required before an SSDP advertisement can become
/// resource provenance. The parser never derives interface, clock, or policy
/// state from packet headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsdpPublicationContext {
    /// The interface on which the trusted-LAN observation was received.
    pub interface: String,
    /// Unix epoch milliseconds when the advertisement was observed.
    pub observed_at_ms: u64,
    /// Unix epoch milliseconds when this publication expires.
    pub expires_at_ms: u64,
    /// Caller-supplied current time used for an explicit freshness decision.
    pub now_ms: u64,
}

/// An SSDP advertisement admitted for the typed resource adapter below.
///
/// This is deliberately not a [`DesktopSource`]. The publication gate creates
/// valid shared provenance but does not silently map SSDP into the existing
/// mDNS roster. The adapter below is deliberately a non-I/O boundary: a
/// future `rupnp` worker may supply these records, but it still has to provide
/// its own bounded interface and observation policy before calling it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsdpPublishedAdvertisement {
    /// The bounded advertisement that supplied the identity and endpoint.
    pub advertisement: SsdpDesktopAdvertisement,
    /// Shared resource provenance for the trusted-LAN observation.
    pub provenance: SourceProvenance,
    /// Reachability remains explicit: unknown is preserved rather than probed.
    pub reachability: Reachability,
}

/// Typed rejection from the SSDP trusted-LAN publication gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SsdpPublicationError {
    /// The advertisement was malformed when revalidated at the publication
    /// boundary.
    MalformedAdvertisement(SsdpAdvertisementError),
    /// Only caller-supplied trusted-LAN observation may publish this lane.
    TrustRequired,
    /// A trusted interface scope is mandatory for SSDP provenance.
    InterfaceRequired,
    /// An explicitly unreachable observation is not publishable as fresh LAN
    /// presence. Offline retention belongs to a later catalog policy.
    Unreachable,
    /// The context timestamps are zero, reversed, or from the future.
    InvalidTimestamp,
    /// The requested publication has already expired at the caller's `now_ms`.
    Expired,
    /// The shared resource contract's minimum TTL was not met.
    TtlTooShort,
    /// The SSDP lane's bounded TTL was exceeded.
    TtlTooLong,
    /// The published reachability disagreed with the caller-supplied
    /// advertisement observation.
    ReachabilityMismatch,
    /// The resulting shared provenance failed its own strict contract.
    InvalidProvenance(ResourceValidationError),
}

impl std::fmt::Display for SsdpPublicationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedAdvertisement(error) => {
                write!(f, "malformed SSDP advertisement: {error}")
            }
            Self::TrustRequired => write!(f, "SSDP publication requires observed-LAN trust"),
            Self::InterfaceRequired => write!(f, "SSDP publication requires an interface"),
            Self::Unreachable => write!(f, "unreachable SSDP observation is not publishable"),
            Self::InvalidTimestamp => write!(f, "invalid SSDP publication timestamp"),
            Self::Expired => write!(f, "SSDP publication is expired"),
            Self::TtlTooShort => write!(f, "SSDP publication TTL is too short"),
            Self::TtlTooLong => write!(f, "SSDP publication TTL exceeds the bounded window"),
            Self::ReachabilityMismatch => {
                write!(
                    f,
                    "SSDP publication reachability disagrees with its observation"
                )
            }
            Self::InvalidProvenance(error) => write!(f, "invalid SSDP provenance: {error:?}"),
        }
    }
}

impl std::error::Error for SsdpPublicationError {}

impl SsdpPublishedAdvertisement {
    /// Revalidate a publication at the instant it is about to be used.
    ///
    /// `SsdpPublishedAdvertisement` is intentionally non-exhaustive because
    /// its fields are evidence, not an authorization to bypass this gate.
    /// This method still rechecks every cross-field relationship for values
    /// assembled inside this crate: advertisement grammar, trusted-LAN
    /// provenance, explicit reachability, identity binding, bounded TTL, and
    /// expiry at `now_ms`.
    pub fn validate_at(&self, now_ms: u64) -> Result<(), SsdpPublicationError> {
        self.advertisement
            .validate()
            .map_err(SsdpPublicationError::MalformedAdvertisement)?;
        if self.advertisement.trust != Some(ProvenanceTrust::ObservedLan) {
            return Err(SsdpPublicationError::TrustRequired);
        }
        if self.advertisement.reachability == Some(Reachability::Unreachable) {
            return Err(SsdpPublicationError::Unreachable);
        }
        if self.reachability
            != self
                .advertisement
                .reachability
                .unwrap_or(Reachability::Unknown)
        {
            return Err(SsdpPublicationError::ReachabilityMismatch);
        }

        if self
            .provenance
            .interface
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
        {
            return Err(SsdpPublicationError::InterfaceRequired);
        }
        if self.provenance.observed_at_ms == 0
            || self.provenance.expires_at_ms <= self.provenance.observed_at_ms
            || now_ms == 0
            || self.provenance.observed_at_ms > now_ms
        {
            return Err(SsdpPublicationError::InvalidTimestamp);
        }
        if self.provenance.expires_at_ms <= now_ms {
            return Err(SsdpPublicationError::Expired);
        }
        let ttl = self.provenance.expires_at_ms - self.provenance.observed_at_ms;
        if ttl < MIN_RESOURCE_TTL_MS {
            return Err(SsdpPublicationError::TtlTooShort);
        }
        if ttl > MAX_SSDP_PUBLICATION_TTL_MS {
            return Err(SsdpPublicationError::TtlTooLong);
        }
        if self.provenance.source_id != self.advertisement.source_id {
            return Err(SsdpPublicationError::InvalidProvenance(
                ResourceValidationError::InvalidRelationship("ssdp.provenance_source_id"),
            ));
        }
        self.provenance
            .validate()
            .map_err(SsdpPublicationError::InvalidProvenance)
    }
}

/// Admit one parsed advertisement into the shared trusted-LAN provenance
/// vocabulary. This function is pure and performs no socket, scan, retry,
/// interface lookup, URL fetch, or launch operation.
pub fn admit_ssdp_publication(
    advertisement: SsdpDesktopAdvertisement,
    context: SsdpPublicationContext,
) -> Result<SsdpPublishedAdvertisement, SsdpPublicationError> {
    let provenance = SourceProvenance {
        schema_version: RESOURCE_CONTRACT_VERSION,
        source: DiscoverySource::SsdpUpnp,
        source_id: advertisement.source_id.clone(),
        scope: ResourceScope::TrustedLan,
        trust: ProvenanceTrust::ObservedLan,
        interface: Some(context.interface.trim().to_string()),
        observed_at_ms: context.observed_at_ms,
        expires_at_ms: context.expires_at_ms,
    };
    let published = SsdpPublishedAdvertisement {
        reachability: advertisement.reachability.unwrap_or(Reachability::Unknown),
        advertisement,
        provenance,
    };
    published.validate_at(context.now_ms)?;
    Ok(published)
}

/// Maximum number of already-admitted SSDP records accepted by one adapter
/// snapshot. The runtime caller must obtain a bounded snapshot before calling
/// the adapter; this guard prevents a future worker from turning one catalog
/// cycle into an unbounded roster fold.
pub const MAX_SSDP_ADAPTER_RECORDS: usize = 64;
/// Maximum number of explicitly trusted interfaces in one adapter policy.
const MAX_SSDP_ADAPTER_INTERFACES: usize = 8;

/// Construction failures for the trusted-LAN SSDP resource policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SsdpResourceAdapterPolicyError {
    /// An adapter without an explicit interface allowlist cannot admit LAN
    /// observations.
    NoInterfaces,
    /// The allowlist itself is bounded independently of the record snapshot.
    TooManyInterfaces { max: usize },
    /// Interface names are configuration identities, never arbitrary strings.
    InvalidInterface,
    /// A zero or oversized record limit would not establish a useful bound.
    InvalidRecordLimit { max: usize },
}

impl std::fmt::Display for SsdpResourceAdapterPolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoInterfaces => write!(f, "SSDP policy requires an interface allowlist"),
            Self::TooManyInterfaces { max } => {
                write!(f, "SSDP policy exceeds the {max}-interface limit")
            }
            Self::InvalidInterface => write!(f, "SSDP policy contains an invalid interface"),
            Self::InvalidRecordLimit { max } => {
                write!(f, "SSDP policy record limit must be between 1 and {max}")
            }
        }
    }
}

impl std::error::Error for SsdpResourceAdapterPolicyError {}

/// Explicit trusted-LAN policy owned by the SSDP-to-resource adapter.
///
/// The policy is intentionally supplied by the runtime owner rather than
/// inferred from an advertisement. A `rupnp` integration can construct this
/// from its reviewed interface configuration and then hand only admitted
/// records to [`SsdpResourceAdapter::adapt`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsdpResourceAdapterPolicy {
    allowed_interfaces: BTreeSet<String>,
    max_records: usize,
}

impl SsdpResourceAdapterPolicy {
    /// Build a bounded policy from the explicitly trusted interface names and
    /// per-snapshot record limit.
    pub fn new(
        allowed_interfaces: Vec<String>,
        max_records: usize,
    ) -> Result<Self, SsdpResourceAdapterPolicyError> {
        if allowed_interfaces.is_empty() {
            return Err(SsdpResourceAdapterPolicyError::NoInterfaces);
        }
        if allowed_interfaces.len() > MAX_SSDP_ADAPTER_INTERFACES {
            return Err(SsdpResourceAdapterPolicyError::TooManyInterfaces {
                max: MAX_SSDP_ADAPTER_INTERFACES,
            });
        }
        if !(1..=MAX_SSDP_ADAPTER_RECORDS).contains(&max_records) {
            return Err(SsdpResourceAdapterPolicyError::InvalidRecordLimit {
                max: MAX_SSDP_ADAPTER_RECORDS,
            });
        }

        let mut interfaces = BTreeSet::new();
        for interface in allowed_interfaces {
            if !valid_ssdp_interface_name(&interface) {
                return Err(SsdpResourceAdapterPolicyError::InvalidInterface);
            }
            interfaces.insert(interface);
        }
        if interfaces.is_empty() {
            return Err(SsdpResourceAdapterPolicyError::NoInterfaces);
        }
        Ok(Self {
            allowed_interfaces: interfaces,
            max_records,
        })
    }

    fn allows_interface(&self, interface: &str) -> bool {
        self.allowed_interfaces.contains(interface)
    }
}

fn valid_ssdp_interface_name(interface: &str) -> bool {
    !interface.is_empty()
        && interface.len() <= 64
        && interface.trim() == interface
        && interface.is_ascii()
        && interface
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'%'))
}

/// Typed failures from the non-I/O SSDP resource adapter boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SsdpResourceAdapterError {
    /// The supplied runtime snapshot exceeded the explicit policy limit.
    TooManyRecords { count: usize, max: usize },
    /// The publication was not observed on an explicitly trusted interface.
    InterfaceNotAllowed { interface: String },
    /// A record failed the use-time publication gate.
    PublicationRejected {
        source_id: String,
        error: SsdpPublicationError,
    },
    /// One stable SSDP identity changed its endpoint or observation context in
    /// the same snapshot; silently choosing a winner would lose provenance.
    ConflictingIdentity { source_id: String },
    /// The resulting typed card failed the shared resource contract.
    InvalidCard(ResourceValidationError),
}

impl std::fmt::Display for SsdpResourceAdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyRecords { count, max } => {
                write!(
                    f,
                    "SSDP snapshot contains {count} records; maximum is {max}"
                )
            }
            Self::InterfaceNotAllowed { interface } => {
                write!(
                    f,
                    "SSDP interface is outside the trusted policy: {interface}"
                )
            }
            Self::PublicationRejected { source_id, error } => {
                write!(
                    f,
                    "SSDP publication {source_id} failed revalidation: {error}"
                )
            }
            Self::ConflictingIdentity { source_id } => {
                write!(f, "SSDP identity has conflicting records: {source_id}")
            }
            Self::InvalidCard(error) => write!(f, "SSDP resource card is invalid: {error:?}"),
        }
    }
}

impl std::error::Error for SsdpResourceAdapterError {}

/// Non-I/O adapter from admitted SSDP observations into universal resource
/// cards.
///
/// This is the integration seam for a future `rupnp` runtime. It accepts only
/// records that have already crossed [`admit_ssdp_publication`], revalidates
/// them at use time, enforces the caller's interface allowlist, caps one
/// snapshot, and emits no socket, URL, retry, scan, or launch operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsdpResourceAdapter {
    policy: SsdpResourceAdapterPolicy,
}

impl SsdpResourceAdapter {
    /// Construct an adapter with an explicit trusted-LAN policy.
    #[must_use]
    pub const fn new(policy: SsdpResourceAdapterPolicy) -> Self {
        Self { policy }
    }

    /// Adapt one bounded runtime snapshot into deterministically ordered cards.
    ///
    /// Records for one UUID may advertise multiple closed MCNF desktop
    /// protocols. Exact duplicate packets are folded; conflicting endpoint or
    /// provenance fields fail closed rather than choosing a winner. The card
    /// and every transport/action retain the publication's exact observed and
    /// expiry timestamps.
    pub fn adapt(
        &self,
        advertisements: &[SsdpPublishedAdvertisement],
        now_ms: u64,
    ) -> Result<Vec<ResourceCard>, SsdpResourceAdapterError> {
        if advertisements.len() > self.policy.max_records {
            return Err(SsdpResourceAdapterError::TooManyRecords {
                count: advertisements.len(),
                max: self.policy.max_records,
            });
        }

        let mut by_source: BTreeMap<String, Vec<&SsdpPublishedAdvertisement>> = BTreeMap::new();
        for advertisement in advertisements {
            advertisement.validate_at(now_ms).map_err(|error| {
                SsdpResourceAdapterError::PublicationRejected {
                    source_id: advertisement.advertisement.source_id.clone(),
                    error,
                }
            })?;
            let interface = advertisement
                .provenance
                .interface
                .as_deref()
                .expect("validate_at requires an SSDP interface");
            if !self.policy.allows_interface(interface) {
                return Err(SsdpResourceAdapterError::InterfaceNotAllowed {
                    interface: interface.to_owned(),
                });
            }
            by_source
                .entry(advertisement.advertisement.source_id.clone())
                .or_default()
                .push(advertisement);
        }

        let mut cards = Vec::with_capacity(by_source.len());
        for (source_id, records) in by_source {
            let mut unique: Vec<&SsdpPublishedAdvertisement> = Vec::with_capacity(records.len());
            for record in records {
                if let Some(existing) = unique.iter().find(|existing| {
                    existing.advertisement.protocol == record.advertisement.protocol
                }) {
                    if *existing != record {
                        return Err(SsdpResourceAdapterError::ConflictingIdentity { source_id });
                    }
                    continue;
                }
                unique.push(record);
            }

            let first = unique
                .first()
                .expect("source groups are created from at least one record");
            if unique.iter().skip(1).any(|record| {
                record.provenance != first.provenance
                    || record.advertisement.display_name != first.advertisement.display_name
                    || record.advertisement.host != first.advertisement.host
                    || record.advertisement.trust != first.advertisement.trust
                    || record.advertisement.reachability != first.advertisement.reachability
            }) {
                return Err(SsdpResourceAdapterError::ConflictingIdentity { source_id });
            }

            unique.sort_unstable_by_key(|record| record.advertisement.protocol);
            cards.push(
                ssdp_resource_card_from_records(&unique)
                    .map_err(SsdpResourceAdapterError::InvalidCard)?,
            );
        }
        cards.sort_unstable_by(|left, right| left.resource_id().cmp(right.resource_id()));
        Ok(cards)
    }
}

/// Typed failures at the bounded SSDP advertisement boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SsdpAdvertisementError {
    /// The raw block or typed map exceeded the bounded byte budget.
    HeaderBlockTooLarge {
        /// Supplied byte count.
        bytes: usize,
        /// Maximum admitted byte count.
        max: usize,
    },
    /// More than [`MAX_SSDP_HEADERS`] entries were supplied.
    TooManyHeaders {
        /// Maximum admitted header count.
        max: usize,
    },
    /// One header line exceeded the bounded line budget.
    HeaderLineTooLong {
        /// Maximum admitted line length.
        max: usize,
    },
    /// A control character appeared where the wire grammar does not allow it.
    MalformedControlCharacter,
    /// The raw block had an invalid start line, header line, or blank-line
    /// layout.
    MalformedHeaderBlock,
    /// A header name was not an ASCII token.
    InvalidHeaderName,
    /// A header is outside the closed MCNF vocabulary.
    UnsupportedHeader,
    /// `LOCATION` is explicitly forbidden; this seam never parses URLs.
    UrlHeaderForbidden,
    /// A raw SSDP start line was not one of the bounded accepted forms.
    InvalidStartLine,
    /// A required MCNF field was absent.
    MissingField(&'static str),
    /// Case-insensitive duplicate headers carried different values.
    ConflictingDuplicate(&'static str),
    /// A bounded field was empty, trimmed differently, or otherwise invalid.
    InvalidField(&'static str),
    /// A bounded field exceeded its field-specific limit.
    FieldTooLong(&'static str),
    /// The `USN` did not carry a stable MCNF UUID identity.
    InvalidIdentity,
    /// The service type is not one of the three MCNF desktop types.
    UnsupportedServiceType,
    /// The optional MCNF protocol declaration is not supported.
    UnsupportedProtocol,
    /// The service type and optional protocol declaration disagree.
    ProtocolMismatch,
    /// The host was not a literal IP or bounded DNS name.
    InvalidHost,
    /// The port was not a decimal non-zero `u16`.
    InvalidPort,
    /// A value resembled a command, URL, or filesystem path.
    CommandOrPathShapedValue(&'static str),
}

impl std::fmt::Display for SsdpAdvertisementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HeaderBlockTooLarge { bytes, max } => {
                write!(f, "SSDP header block is {bytes} bytes; maximum is {max}")
            }
            Self::TooManyHeaders { max } => write!(f, "SSDP header count exceeds {max}"),
            Self::HeaderLineTooLong { max } => {
                write!(f, "SSDP header line exceeds {max} bytes")
            }
            Self::MalformedControlCharacter => {
                write!(f, "SSDP header contains a control character")
            }
            Self::MalformedHeaderBlock => write!(f, "malformed SSDP header block"),
            Self::InvalidHeaderName => write!(f, "invalid SSDP header name"),
            Self::UnsupportedHeader => write!(f, "unsupported SSDP header"),
            Self::UrlHeaderForbidden => write!(f, "SSDP LOCATION/URL headers are forbidden"),
            Self::InvalidStartLine => write!(f, "invalid SSDP start line"),
            Self::MissingField(field) => write!(f, "missing MCNF SSDP field {field}"),
            Self::ConflictingDuplicate(field) => {
                write!(f, "conflicting duplicate MCNF SSDP field {field}")
            }
            Self::InvalidField(field) => write!(f, "invalid MCNF SSDP field {field}"),
            Self::FieldTooLong(field) => write!(f, "MCNF SSDP field is too long: {field}"),
            Self::InvalidIdentity => write!(f, "invalid MCNF SSDP identity"),
            Self::UnsupportedServiceType => write!(f, "unsupported MCNF SSDP service type"),
            Self::UnsupportedProtocol => write!(f, "unsupported MCNF SSDP protocol"),
            Self::ProtocolMismatch => write!(f, "MCNF SSDP service/protocol mismatch"),
            Self::InvalidHost => write!(f, "invalid MCNF SSDP host"),
            Self::InvalidPort => write!(f, "invalid MCNF SSDP port"),
            Self::CommandOrPathShapedValue(field) => {
                write!(f, "MCNF SSDP field is command/path-shaped: {field}")
            }
        }
    }
}

impl std::error::Error for SsdpAdvertisementError {}

const SSDP_HEADER_VALUE_MAX_BYTES: usize = 1_024;
const SSDP_ID_MAX_BYTES: usize = 128;
const SSDP_NAME_MAX_BYTES: usize = 512;
const SSDP_HOST_MAX_BYTES: usize = 255;

/// Parse a bounded raw SSDP header block with no caller-derived state.
///
/// The accepted block may include `NOTIFY * HTTP/1.1` or `HTTP/1.1 200 OK` as
/// its first line, or may contain only headers. `LOCATION` and all headers
/// outside the MCNF vocabulary are rejected.
pub fn parse_ssdp_advertisement(
    raw: &str,
) -> Result<SsdpDesktopAdvertisement, SsdpAdvertisementError> {
    parse_ssdp_advertisement_with_observation(raw, SsdpObservation::default())
}

/// Parse a bounded raw SSDP header block while retaining only explicit caller
/// trust/reachability context.
pub fn parse_ssdp_advertisement_with_observation(
    raw: &str,
    observation: SsdpObservation,
) -> Result<SsdpDesktopAdvertisement, SsdpAdvertisementError> {
    let pairs = parse_ssdp_header_block(raw)?;
    normalize_ssdp_pairs(
        pairs
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        observation,
    )
}

/// Normalize a typed, case-insensitive SSDP header map with no live-network
/// behavior and no inferred trust/reachability.
pub fn normalize_ssdp_header_map(
    headers: &SsdpHeaderMap,
) -> Result<SsdpDesktopAdvertisement, SsdpAdvertisementError> {
    normalize_ssdp_header_map_with_observation(headers, SsdpObservation::default())
}

/// Normalize a typed SSDP header map while retaining only explicit caller
/// trust/reachability context.
pub fn normalize_ssdp_header_map_with_observation(
    headers: &SsdpHeaderMap,
    observation: SsdpObservation,
) -> Result<SsdpDesktopAdvertisement, SsdpAdvertisementError> {
    normalize_ssdp_pairs(
        headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        observation,
    )
}

fn parse_ssdp_header_block(raw: &str) -> Result<Vec<(String, String)>, SsdpAdvertisementError> {
    if raw.len() > MAX_SSDP_HEADER_BLOCK_BYTES {
        return Err(SsdpAdvertisementError::HeaderBlockTooLarge {
            bytes: raw.len(),
            max: MAX_SSDP_HEADER_BLOCK_BYTES,
        });
    }
    let bytes = raw.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        match *byte {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => {}
            b'\n' => {}
            b'\r' | 0..=0x1f | 0x7f => {
                return Err(SsdpAdvertisementError::MalformedControlCharacter);
            }
            _ => {}
        }
    }

    let mut pairs = Vec::new();
    let mut saw_start_line = false;
    let mut ended = false;
    for line in raw.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            ended = true;
            continue;
        }
        if ended {
            return Err(SsdpAdvertisementError::MalformedHeaderBlock);
        }
        if line.len() > SSDP_HEADER_VALUE_MAX_BYTES {
            return Err(SsdpAdvertisementError::HeaderLineTooLong {
                max: SSDP_HEADER_VALUE_MAX_BYTES,
            });
        }
        if !saw_start_line && pairs.is_empty() && !line.contains(':') {
            if !line.eq_ignore_ascii_case("NOTIFY * HTTP/1.1")
                && !line.eq_ignore_ascii_case("HTTP/1.1 200 OK")
            {
                return Err(SsdpAdvertisementError::InvalidStartLine);
            }
            saw_start_line = true;
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(SsdpAdvertisementError::MalformedHeaderBlock);
        };
        pairs.push((name.to_string(), value.to_string()));
    }
    Ok(pairs)
}

fn normalize_ssdp_pairs<'a>(
    pairs: impl IntoIterator<Item = (&'a str, &'a str)>,
    observation: SsdpObservation,
) -> Result<SsdpDesktopAdvertisement, SsdpAdvertisementError> {
    let mut fields = BTreeMap::<&'static str, String>::new();
    let mut total_bytes = 0usize;
    let mut count = 0usize;
    for (raw_name, raw_value) in pairs {
        count = count.saturating_add(1);
        if count > MAX_SSDP_HEADERS {
            return Err(SsdpAdvertisementError::TooManyHeaders {
                max: MAX_SSDP_HEADERS,
            });
        }
        total_bytes = total_bytes
            .saturating_add(raw_name.len())
            .saturating_add(raw_value.len());
        if total_bytes > MAX_SSDP_HEADER_BLOCK_BYTES {
            return Err(SsdpAdvertisementError::HeaderBlockTooLarge {
                bytes: total_bytes,
                max: MAX_SSDP_HEADER_BLOCK_BYTES,
            });
        }
        let name = canonical_ssdp_header_name(raw_name)?;
        if raw_value.len() > SSDP_HEADER_VALUE_MAX_BYTES {
            return Err(SsdpAdvertisementError::HeaderLineTooLong {
                max: SSDP_HEADER_VALUE_MAX_BYTES,
            });
        }
        if raw_value.chars().any(char::is_control) {
            return Err(SsdpAdvertisementError::MalformedControlCharacter);
        }
        let value = raw_value.trim();
        if value.is_empty() {
            return Err(SsdpAdvertisementError::InvalidField(name));
        }
        if let Some(existing) = fields.get(name) {
            if existing != value {
                return Err(SsdpAdvertisementError::ConflictingDuplicate(name));
            }
        } else {
            fields.insert(name, value.to_string());
        }
    }

    let service_type = required_ssdp_field(&fields, "service-type", "service type")?;
    let protocol = protocol_from_ssdp_service_type(service_type)?;
    if let Some(declared) = fields.get("protocol") {
        validate_ssdp_text("protocol", declared, 16)?;
        let declared = match declared.to_ascii_lowercase().as_str() {
            "rdp" => DesktopProtocol::Rdp,
            "vnc" => DesktopProtocol::Vnc,
            "spice" => DesktopProtocol::Spice,
            _ => return Err(SsdpAdvertisementError::UnsupportedProtocol),
        };
        if declared != protocol {
            return Err(SsdpAdvertisementError::ProtocolMismatch);
        }
    }

    let identity = required_ssdp_field(&fields, "usn", "USN")?;
    let source_id = parse_ssdp_identity(identity, service_type)?;
    let display_name = required_ssdp_field(&fields, "name", "X-MCNF-NAME")?;
    validate_ssdp_text("display name", display_name, SSDP_NAME_MAX_BYTES)?;
    let host = required_ssdp_field(&fields, "host", "X-MCNF-HOST")?;
    validate_ssdp_host(host)?;
    let port = required_ssdp_port(required_ssdp_field(&fields, "port", "X-MCNF-PORT")?)?;

    Ok(SsdpDesktopAdvertisement {
        source_id,
        display_name: display_name.to_string(),
        host: host.to_string(),
        port,
        protocol,
        trust: observation.trust,
        reachability: observation.reachability,
    })
}

fn canonical_ssdp_header_name(name: &str) -> Result<&'static str, SsdpAdvertisementError> {
    if name.is_empty()
        || name.trim() != name
        || !name.is_ascii()
        || name.chars().any(char::is_control)
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(SsdpAdvertisementError::InvalidHeaderName);
    }
    match name.to_ascii_uppercase().as_str() {
        "USN" => Ok("usn"),
        "NT" | "ST" => Ok("service-type"),
        "X-MCNF-NAME" => Ok("name"),
        "X-MCNF-HOST" => Ok("host"),
        "X-MCNF-PORT" => Ok("port"),
        "X-MCNF-PROTOCOL" => Ok("protocol"),
        // Trust and reachability are caller context, never packet claims.
        "X-MCNF-TRUST" | "X-MCNF-REACHABILITY" => Err(SsdpAdvertisementError::UnsupportedHeader),
        // Do not accept or inspect arbitrary device-description URLs.
        "LOCATION" => Err(SsdpAdvertisementError::UrlHeaderForbidden),
        _ => Err(SsdpAdvertisementError::UnsupportedHeader),
    }
}

fn required_ssdp_field<'a>(
    fields: &'a BTreeMap<&'static str, String>,
    key: &'static str,
    label: &'static str,
) -> Result<&'a str, SsdpAdvertisementError> {
    fields
        .get(key)
        .map(String::as_str)
        .ok_or(SsdpAdvertisementError::MissingField(label))
}

fn validate_ssdp_text(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), SsdpAdvertisementError> {
    if value.len() > max_bytes {
        return Err(SsdpAdvertisementError::FieldTooLong(field));
    }
    if value.is_empty()
        || value.trim() != value
        || value.chars().any(char::is_control)
        || looks_like_command_or_path(value)
    {
        return Err(if looks_like_command_or_path(value) {
            SsdpAdvertisementError::CommandOrPathShapedValue(field)
        } else {
            SsdpAdvertisementError::InvalidField(field)
        });
    }
    Ok(())
}

fn looks_like_command_or_path(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.contains("://")
        || value.contains('/')
        || value.contains('\\')
        || value.contains(';')
        || value.contains('|')
        || value.contains('`')
        || value.contains("&&")
        || value.contains("||")
        || value.contains("$(")
        || value.contains("${")
        || lower.starts_with("sh ")
        || lower.starts_with("bash ")
        || lower.starts_with("zsh ")
        || lower.starts_with("fish ")
        || lower.starts_with("cmd ")
        || lower.starts_with("powershell ")
        || lower.starts_with("pwsh ")
        || lower.starts_with("python ")
        || lower.starts_with("python3 ")
        || lower.starts_with("curl ")
        || lower.starts_with("wget ")
        || lower.starts_with("ssh ")
        || lower.starts_with("nc ")
        || lower.starts_with("netcat ")
        || lower.starts_with("exec ")
        || lower.contains(" -c ")
}

fn protocol_from_ssdp_service_type(
    service_type: &str,
) -> Result<DesktopProtocol, SsdpAdvertisementError> {
    if looks_like_command_or_path(service_type) {
        return Err(SsdpAdvertisementError::CommandOrPathShapedValue(
            "service type",
        ));
    }
    if service_type.eq_ignore_ascii_case(MCNF_SSDP_RDP_SERVICE_TYPE) {
        Ok(DesktopProtocol::Rdp)
    } else if service_type.eq_ignore_ascii_case(MCNF_SSDP_VNC_SERVICE_TYPE) {
        Ok(DesktopProtocol::Vnc)
    } else if service_type.eq_ignore_ascii_case(MCNF_SSDP_SPICE_SERVICE_TYPE) {
        Ok(DesktopProtocol::Spice)
    } else {
        Err(SsdpAdvertisementError::UnsupportedServiceType)
    }
}

fn parse_ssdp_identity(value: &str, service_type: &str) -> Result<String, SsdpAdvertisementError> {
    validate_ssdp_text("USN", value, SSDP_HEADER_VALUE_MAX_BYTES)?;
    if !value.is_ascii() || value.len() < 6 || !value[..5].eq_ignore_ascii_case("uuid:") {
        return Err(SsdpAdvertisementError::InvalidIdentity);
    }
    let rest = &value[5..];
    let (token, suffix) = rest
        .split_once("::")
        .map_or((rest, None), |parts| (parts.0, Some(parts.1)));
    if token.is_empty()
        || token.len() > SSDP_ID_MAX_BYTES
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(SsdpAdvertisementError::InvalidIdentity);
    }
    if let Some(suffix) = suffix {
        if suffix.is_empty() || suffix.contains("::") || !suffix.eq_ignore_ascii_case(service_type)
        {
            return Err(SsdpAdvertisementError::InvalidIdentity);
        }
    }
    Ok(format!("uuid:{}", token.to_ascii_lowercase()))
}

fn validate_ssdp_host(host: &str) -> Result<(), SsdpAdvertisementError> {
    if host.len() > SSDP_HOST_MAX_BYTES
        || host.is_empty()
        || host.trim() != host
        || host.chars().any(char::is_control)
        || looks_like_command_or_path(host)
    {
        return Err(if looks_like_command_or_path(host) {
            SsdpAdvertisementError::CommandOrPathShapedValue("host")
        } else {
            SsdpAdvertisementError::InvalidHost
        });
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if ip.is_unspecified() || ip.is_multicast() {
            return Err(SsdpAdvertisementError::InvalidHost);
        }
        return Ok(());
    }
    if !host.is_ascii() || host.contains(':') {
        return Err(SsdpAdvertisementError::InvalidHost);
    }
    for label in host.split('.') {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(SsdpAdvertisementError::InvalidHost);
        }
    }
    Ok(())
}

fn required_ssdp_port(value: &str) -> Result<u16, SsdpAdvertisementError> {
    if value.is_empty() || value.len() > 5 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(SsdpAdvertisementError::InvalidPort);
    }
    let port = value
        .parse::<u16>()
        .map_err(|_| SsdpAdvertisementError::InvalidPort)?;
    if port == 0 {
        return Err(SsdpAdvertisementError::InvalidPort);
    }
    Ok(port)
}

/// One protocol a source offers, with the port when it is known. A `None`
/// port means the transport is brokered (a local VM's Spice console) or
/// defaulted at connect time — never a guessed number.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct ProtocolOffer {
    /// The protocol.
    pub protocol: DesktopProtocol,
    /// The advertised/known port, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

impl ProtocolOffer {
    /// Construct an offer.
    #[must_use]
    pub const fn new(protocol: DesktopProtocol, port: Option<u16>) -> Self {
        Self { protocol, port }
    }
}

/// Derived (never live-probed — lock 14) reachability of a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reachability {
    /// Roster/VM state says the source should answer.
    Reachable,
    /// Roster/VM state says it won't (the card greys with `reason`).
    Unreachable,
    /// Nothing derivable (a manual endpoint is never probed) — honest.
    Unknown,
}

/// Which discovery lane produced a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceOrigin {
    /// Peer-advertised via the replicated peers plane.
    MeshPeer,
    /// Discovered on the local LAN via mDNS.
    Mdns,
    /// A local libvirt/KVM guest console.
    LocalVm,
    /// Operator-added.
    Manual,
}

/// One merged desktop source — a row of the published roster.
///
/// The per-source shape the CHOOSER-1 acceptance pins: id, display name,
/// node/host, protocols offered, derived reachability (+ a human reason when
/// greyed), OS hint when genuinely known, power state for VMs, and the
/// thumbnail ref CHOOSER-3 will fill (always serialized; honestly `null`
/// today).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DesktopSource {
    /// Stable id (`peer:<node>` / `peer-vm:<node>:<vm>` / `vm:<node>:<name>`
    /// / `mdns:<host>:<port>:<proto>` / `manual:<host>:<port>:<proto>`).
    pub id: String,
    /// Display name for the card.
    pub name: String,
    /// The node/host the Chooser groups by (design lock 3).
    pub node: String,
    /// The address a client connects to (overlay IP / `<host>.mesh` / LAN
    /// address); for a local VM the serving node (the console is brokered).
    pub host: String,
    /// Protocols offered, deduped + sorted.
    pub protocols: Vec<ProtocolOffer>,
    /// The discovery lane this source came from.
    pub origin: SourceOrigin,
    /// Derived reachability (lock 14 — never a blocking probe).
    pub reachability: Reachability,
    /// Human-readable reason when not reachable (the greyed card's caption).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// OS hint when genuinely known (a mesh peer's seat is an MCNF Linux
    /// desktop); `None` rather than a guess.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_hint: Option<String>,
    /// Live power state for VM sources (`running` / `shut off` / …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_state: Option<String>,
    /// Thumbnail reference — the key CHOOSER-3 fills with periodic previews.
    /// ALWAYS serialized (no skip) so consumers see the field; honestly
    /// `null` until a thumbnail pipeline exists.
    pub thumbnail_ref: Option<String>,
}

// ─────────────────── lane 1: mesh-registry (peer-advertised) ───────────────────

/// The small peer-advertised desktop shape.
///
/// What one node's published state says it serves — lifted from the peer's
/// replicated [`PeerRecord`] by [`advertised_from_peer`]: the node's own
/// seat (its RDP/VNC listeners, `vm == None`) and each VM desktop it hosts
/// (`vm == Some(name)`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AdvertisedDesktop {
    /// The advertising node's hostname.
    pub node: String,
    /// The address clients dial (overlay IP, else `<node>.mesh`).
    pub host: String,
    /// `None` = the node's own seat desktop; `Some(name)` = a VM it serves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vm: Option<String>,
    /// Protocols the desktop is served over.
    pub protocols: Vec<ProtocolOffer>,
    /// The VM's advertised power state (`None` for the seat).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_state: Option<String>,
    /// Derived from roster presence/health (+ VM power state).
    pub reachability: Reachability,
    /// Human reason when not reachable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Derive a peer's reachability from its roster row — the health-reconciler's
/// `health` verdict (the primary authority) plus a staleness belt-and-braces.
/// Pure; never a probe (lock 14).
#[must_use]
pub fn peer_reachability(health: &str, stale: bool) -> (Reachability, Option<String>) {
    if stale {
        return (
            Reachability::Unreachable,
            Some("peer heartbeat stale".to_string()),
        );
    }
    match health {
        "unreachable" => (
            Reachability::Unreachable,
            Some("peer unreachable".to_string()),
        ),
        // A degraded/critical peer still answers on the network — the desktop
        // may well connect; only a hard unreachable greys the card.
        "healthy" | "degraded" | "critical" => (Reachability::Reachable, None),
        _ => (Reachability::Unknown, None),
    }
}

/// Lift the advertised desktops out of one peer's published record.
///
/// Yields the seat (when its RDP/VNC listeners are advertised) + each hosted
/// VM. The local node's own record is skipped — its VMs come from the richer
/// live KVM lane, and its own seat is not a remote desktop to itself.
#[must_use]
pub fn advertised_from_peer(rec: &PeerRecord, self_node: &str) -> Vec<AdvertisedDesktop> {
    if rec.hostname.eq_ignore_ascii_case(self_node) {
        return Vec::new();
    }
    let Some(desc) = rec.descriptors.as_ref() else {
        return Vec::new(); // a pre-PD-2 writer advertises nothing
    };
    let host = rec
        .overlay_ip
        .clone()
        .unwrap_or_else(|| format!("{}.{}", rec.hostname, super::mesh_dns::MESH_SUFFIX));
    let (reachability, reason) = peer_reachability(&rec.health, rec.is_stale(PEER_STALE_MS));

    let mut out = Vec::new();
    let mut seat = Vec::new();
    if desc.remote_access.rdp {
        seat.push(ProtocolOffer::new(
            DesktopProtocol::Rdp,
            DesktopProtocol::Rdp.default_port(),
        ));
    }
    if desc.remote_access.vnc {
        seat.push(ProtocolOffer::new(
            DesktopProtocol::Vnc,
            DesktopProtocol::Vnc.default_port(),
        ));
    }
    if !seat.is_empty() {
        out.push(AdvertisedDesktop {
            node: rec.hostname.clone(),
            host: host.clone(),
            vm: None,
            protocols: seat,
            power_state: None,
            reachability,
            reason: reason.clone(),
        });
    }
    for vm in &desc.vms {
        let live = matches!(
            vm_state_from_str(&vm.state),
            VmState::Running | VmState::Paused
        );
        let (r, why) = if reachability == Reachability::Reachable && !live {
            (
                Reachability::Unreachable,
                Some(format!("vm {}", vm.state.trim())),
            )
        } else {
            (reachability, reason.clone())
        };
        out.push(AdvertisedDesktop {
            node: rec.hostname.clone(),
            host: host.clone(),
            vm: Some(vm.name.clone()),
            // MV-3 domains carry Spice graphics; the console is brokered by
            // the serving peer (E12 VDI), so no port is claimed here.
            protocols: vec![ProtocolOffer::new(DesktopProtocol::Spice, None)],
            power_state: Some(vm.state.clone()),
            reachability: r,
            reason: why,
        });
    }
    out
}

/// Fold one advertised desktop into a roster row.
#[must_use]
pub fn source_from_advertised(ad: &AdvertisedDesktop) -> DesktopSource {
    let (id, name, os_hint) = ad.vm.as_ref().map_or_else(
        || {
            (
                format!("peer:{}", ad.node),
                ad.node.clone(),
                // A mesh peer's seat is an MCNF (Linux) desktop — genuinely
                // known, not a guess.
                Some("linux".to_string()),
            )
        },
        |vm| (format!("peer-vm:{}:{vm}", ad.node), vm.clone(), None),
    );
    DesktopSource {
        id,
        name,
        node: ad.node.clone(),
        host: ad.host.clone(),
        protocols: ad.protocols.clone(),
        origin: SourceOrigin::MeshPeer,
        reachability: ad.reachability,
        reason: ad.reason.clone(),
        os_hint,
        power_state: ad.power_state.clone(),
        thumbnail_ref: None,
    }
}

// ───────────────────────── lane 2: mDNS (LAN) ─────────────────────────

/// One mDNS-discovered desktop endpoint on the local LAN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdnsEndpoint {
    /// The mDNS fullname (the daemon's removal key).
    pub fullname: String,
    /// Instance name (e.g. `Office PC`).
    pub instance: String,
    /// Resolved address (deterministically the lowest IPv4, else IPv6).
    pub host: String,
    /// Advertised port.
    pub port: u16,
    /// The desktop protocol the service type maps to.
    pub protocol: DesktopProtocol,
}

/// Lift a resolved mDNS service into an endpoint.
///
/// `None` when it isn't a desktop type, carries the `mdns_relay` anti-loop
/// origin TXT (a service a mesh peer republished — the registry lane already
/// carries that peer), or resolved no address.
#[must_use]
pub fn endpoint_from_service_info(bare: &str, info: &ServiceInfo) -> Option<MdnsEndpoint> {
    let protocol = DesktopProtocol::from_mdns_type(bare)?;
    if info
        .get_property_val_str(super::mdns_relay::RELAY_ORIGIN_TXT)
        .is_some()
    {
        return None;
    }
    let mut addrs: Vec<std::net::IpAddr> = info.get_addresses().iter().copied().collect();
    addrs.sort_by_key(|ip| (ip.is_ipv6(), *ip));
    let host = addrs.first()?.to_string();
    Some(MdnsEndpoint {
        fullname: info.get_fullname().to_string(),
        instance: super::mdns_relay::instance_name(info, bare),
        host,
        port: info.get_port(),
        protocol,
    })
}

/// Fold an mDNS endpoint into a roster row. Presence in the live mDNS cache
/// IS the reachability signal (the daemon expires dead services) — no probe.
#[must_use]
pub fn source_from_mdns(ep: &MdnsEndpoint) -> DesktopSource {
    DesktopSource {
        id: format!("mdns:{}:{}:{}", ep.host, ep.port, ep.protocol.tag()),
        name: ep.instance.clone(),
        node: ep.host.clone(),
        host: ep.host.clone(),
        protocols: vec![ProtocolOffer::new(ep.protocol, Some(ep.port))],
        origin: SourceOrigin::Mdns,
        reachability: Reachability::Reachable,
        reason: None,
        os_hint: None,
        power_state: None,
        thumbnail_ref: None,
    }
}

// ─────────────────────── lane 3: local KVM guests ───────────────────────

/// Minimal presentation projection for one VM Workload.  This is deliberately
/// not a lifecycle command or an actuator type; it is an internal view built
/// from the typed Workload state snapshot.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Instance {
    /// Stable Workload id (used only as a diagnostic/source identity).
    pub id: String,
    /// Libvirt-facing domain name derived from the Workload id.
    pub name: String,
    /// Lowercase power-state spelling for the existing Chooser card contract.
    pub state: String,
}

/// Power states understood by the Chooser's reachability fold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmState {
    /// The guest is running.
    Running,
    /// The guest is paused.
    Paused,
    /// Any non-interactive or unknown state.
    Other,
}

/// Parse the stable lowercase state spelling used by the Workload projection.
#[must_use]
pub fn vm_state_from_str(value: &str) -> VmState {
    match value.trim().to_ascii_lowercase().as_str() {
        "running" => VmState::Running,
        "paused" => VmState::Paused,
        _ => VmState::Other,
    }
}

/// A typed local-VM enumeration failure — the honest-gate discipline
/// (mirrors `mesh_mount::MountError::Gated`, §7): a box without a hypervisor
/// toolchain refuses cleanly, never fakes a source list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmEnumerateError {
    /// The prerequisites aren't on this box (no `virsh`). The honest
    /// headless/CI gate — surfaced in the published lane status.
    Gated(String),
    /// libvirt answered with an error (surfaced verbatim, no sources).
    Backend(String),
}

impl std::fmt::Display for VmEnumerateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gated(m) => write!(f, "gated: {m}"),
            Self::Backend(m) => write!(f, "error: {m}"),
        }
    }
}

impl std::error::Error for VmEnumerateError {}

/// The injectable local-VM enumeration seam. Production is
/// [`WorkloadEnumerator`] over the authoritative Workload projection; tests
/// inject a fake.
pub trait VmEnumerator: Send + Sync {
    /// This node's defined VMs (every one is a console source), or a typed
    /// gate/error — NEVER a fabricated list.
    ///
    /// # Errors
    /// [`VmEnumerateError::Gated`] on a box without the toolchain;
    /// [`VmEnumerateError::Backend`] when libvirt errors.
    fn enumerate(&self) -> Result<Vec<Instance>, VmEnumerateError>;
}

/// The production enumerator: reads the newest node-local
/// `state/workloads/<node>` snapshot. It has no privileged process seam and
/// cannot mutate VM state.
pub struct WorkloadEnumerator {
    node_id: String,
    bus_root: Option<PathBuf>,
}

impl Default for WorkloadEnumerator {
    fn default() -> Self {
        Self::new(String::new())
    }
}

impl WorkloadEnumerator {
    /// Production wiring for one node's Workload projection.
    #[must_use]
    pub fn new(node_id: String) -> Self {
        Self {
            node_id: node_id.clone(),
            bus_root: mde_bus::default_data_dir(),
        }
    }

    /// Override the Bus root for an isolated service instance or test.
    #[must_use]
    pub fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root = Some(root);
        self
    }
}

impl VmEnumerator for WorkloadEnumerator {
    fn enumerate(&self) -> Result<Vec<Instance>, VmEnumerateError> {
        let Some(root) = &self.bus_root else {
            return Err(VmEnumerateError::Gated(
                "Workload state Bus is unavailable on this node".to_string(),
            ));
        };
        let persist = Persist::open(root.clone())
            .map_err(|error| VmEnumerateError::Backend(format!("open Workload state: {error}")))?;
        let topic = workload_state_topic(&self.node_id);
        let messages = persist
            .list_since(&topic, None)
            .map_err(|error| VmEnumerateError::Backend(format!("read Workload state: {error}")))?;
        let Some(message) = messages.last() else {
            return Ok(Vec::new());
        };
        let Some(body) = message.body.as_deref() else {
            return Ok(Vec::new());
        };
        let snapshot: WorkloadStateSnapshot = serde_json::from_str(body).map_err(|error| {
            VmEnumerateError::Backend(format!("decode Workload state: {error}"))
        })?;
        Ok(snapshot
            .workloads
            .into_iter()
            .filter(|status| status.backend == WorkloadBackend::LibvirtVirtqemud)
            .map(|status| {
                let name = status
                    .workload_id
                    .as_str()
                    .rsplit(':')
                    .next()
                    .unwrap_or(status.workload_id.as_str())
                    .to_string();
                let state = match status.power {
                    WorkloadPowerState::Running => "running",
                    WorkloadPowerState::Paused => "paused",
                    WorkloadPowerState::Starting
                    | WorkloadPowerState::Defined
                    | WorkloadPowerState::Stopping => "shut off",
                    WorkloadPowerState::Stopped => "shut off",
                    WorkloadPowerState::Failed => "crashed",
                };
                let state = if status.phase == WorkloadOperationPhase::Cancelled {
                    "shut off"
                } else {
                    state
                };
                Instance {
                    id: status.workload_id.as_str().to_string(),
                    name,
                    state: state.to_string(),
                }
            })
            .collect())
    }
}

/// Fold one local VM into a roster row.
///
/// The console is a Spice recovery source (the native Display1 attachment is
/// represented separately by the Workload projection); no port is claimed.
/// Reachability derives from the power state (running/paused
/// consoles answer; a shut-off VM greys with its state as the reason —
/// CHOOSER-7 starts it from the card).
#[must_use]
pub fn source_from_vm(node: &str, inst: &Instance) -> DesktopSource {
    let state = inst.state.trim().to_string();
    let live = matches!(
        vm_state_from_str(&state),
        VmState::Running | VmState::Paused
    );
    DesktopSource {
        id: format!("vm:{node}:{}", inst.name),
        name: inst.name.clone(),
        node: node.to_string(),
        host: node.to_string(),
        protocols: vec![ProtocolOffer::new(DesktopProtocol::Spice, None)],
        origin: SourceOrigin::LocalVm,
        reachability: if live {
            Reachability::Reachable
        } else {
            Reachability::Unreachable
        },
        reason: (!live).then(|| format!("vm {state}")),
        os_hint: None,
        power_state: Some(state),
        thumbnail_ref: None,
    }
}

// ─────────────────────── lane 4: manual sources + verbs ───────────────────────

/// One operator-added source — also the typed body of an
/// `action/desktops/add-source` request (§9: host + port + protocol, never a
/// command string).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManualSource {
    /// Optional display name (defaults to `host:port`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Host/IP to connect to.
    pub host: String,
    /// Port to connect to.
    pub port: u16,
    /// The protocol to connect over.
    pub protocol: DesktopProtocol,
}

impl ManualSource {
    /// The stable source id (`manual:<host>:<port>:<proto>`) — also the
    /// remove-source key.
    #[must_use]
    pub fn id(&self) -> String {
        format!("manual:{}:{}:{}", self.host, self.port, self.protocol.tag())
    }

    /// Display name (the operator's, else `host:port`).
    #[must_use]
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("{}:{}", self.host, self.port))
    }
}

/// Fold one manual source into a roster row. A manual endpoint is never
/// probed (lock 14), so its reachability is an honest `Unknown`.
#[must_use]
pub fn source_from_manual(m: &ManualSource) -> DesktopSource {
    DesktopSource {
        id: m.id(),
        name: m.display_name(),
        node: m.host.clone(),
        host: m.host.clone(),
        protocols: vec![ProtocolOffer::new(m.protocol, Some(m.port))],
        origin: SourceOrigin::Manual,
        reachability: Reachability::Unknown,
        reason: None,
        os_hint: None,
        power_state: None,
        thumbnail_ref: None,
    }
}

/// Typed body of an `action/desktops/remove-source` request.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemoveSourceRequest {
    /// The manual source id ([`ManualSource::id`]) to remove.
    pub id: String,
}

/// Parse + validate an add-source body.
///
/// # Errors
/// A human-readable message on malformed JSON, an empty host, or port 0.
pub fn parse_add_source(body: &str) -> Result<ManualSource, String> {
    let req: ManualSource =
        serde_json::from_str(body).map_err(|e| format!("malformed add-source request: {e}"))?;
    if req.host.trim().is_empty() {
        return Err("add-source: host must not be empty".to_string());
    }
    if req.port == 0 {
        return Err("add-source: port must be non-zero".to_string());
    }
    Ok(req)
}

/// Parse + validate a remove-source body.
///
/// # Errors
/// A human-readable message on malformed JSON or an empty id.
pub fn parse_remove_source(body: &str) -> Result<RemoveSourceRequest, String> {
    let req: RemoveSourceRequest =
        serde_json::from_str(body).map_err(|e| format!("malformed remove-source request: {e}"))?;
    if req.id.trim().is_empty() {
        return Err("remove-source: id must not be empty".to_string());
    }
    Ok(req)
}

fn manual_store_path(store_root: &Path) -> PathBuf {
    store_root.join(MANUAL_STORE_FILE)
}

/// Read the node-local manual-source store through the descriptor that will be
/// parsed. Refuse final symlinks, blocking special files, oversized input,
/// invalid UTF-8, and files that change while being materialized.
fn read_bounded_manual_store(path: &Path) -> std::io::Result<String> {
    #[cfg(unix)]
    let file: std::fs::File = {
        use rustix::fs::{Mode, OFlags};

        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?
        .into()
    };
    #[cfg(not(unix))]
    let file = {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "manual source store is a final symlink",
            ));
        }
        std::fs::File::open(path)?
    };

    let initial = file.metadata()?;
    if !initial.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "manual source store is not a regular file",
        ));
    }
    let initial_len = initial.len();
    if initial_len > MAX_MANUAL_STORE_BYTES as u64 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("manual source store exceeds {MAX_MANUAL_STORE_BYTES}-byte limit"),
        ));
    }

    let capacity = usize::try_from(initial_len)
        .unwrap_or(MAX_MANUAL_STORE_BYTES)
        .min(MAX_MANUAL_STORE_BYTES)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(capacity);
    let mut limited = file.take((MAX_MANUAL_STORE_BYTES as u64).saturating_add(1));
    limited.read_to_end(&mut bytes)?;
    let file = limited.into_inner();
    let final_len = file.metadata()?.len();
    if bytes.len() > MAX_MANUAL_STORE_BYTES
        || bytes.len() as u64 != initial_len
        || final_len != initial_len
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "manual source store changed or exceeds its byte limit",
        ));
    }

    String::from_utf8(bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

/// Load the node-local manual-source store (absent/corrupt → empty, never
/// fatal — a half-written file must not kill the worker).
#[must_use]
pub fn load_manual_sources(store_root: &Path) -> Vec<ManualSource> {
    read_bounded_manual_store(&manual_store_path(store_root))
        .ok()
        .and_then(|data| serde_json::from_str(&data).ok())
        .unwrap_or_default()
}

/// Persist the manual-source store atomically (temp + rename, the peers-plane
/// idiom).
///
/// # Errors
/// Filesystem/serialization failures.
pub fn save_manual_sources(store_root: &Path, sources: &[ManualSource]) -> std::io::Result<()> {
    std::fs::create_dir_all(store_root)?;
    let json = serde_json::to_string_pretty(sources)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = store_root.join(format!(".{MANUAL_STORE_FILE}.tmp"));
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, manual_store_path(store_root))
}

/// Resolve the node-local store root for manual sources
/// (`<XDG_DATA_HOME>/mde/desktops`, or `/var/lib/mde/desktops` headless) —
/// the `bookmarks::resolve_local_root` idiom.
#[must_use]
pub fn resolve_store_root() -> PathBuf {
    dirs::data_dir().map_or_else(
        || PathBuf::from("/var/lib/mde/desktops"),
        |d| d.join("mde").join("desktops"),
    )
}

// ───────────────────────────── the merge fold ─────────────────────────────

/// Fold the four lanes into ONE deduped, stably-ordered source list — the
/// load-bearing merge the acceptance pins. Rules:
///
/// 1. Peer-advertised desktops seed the list (the roster is the reachability
///    authority for mesh nodes).
/// 2. An mDNS endpoint that resolves to a known peer **seat** (same address,
///    or its instance name matches the node) folds its protocol into that
///    card instead of duplicating it; an unknown LAN endpoint becomes its own
///    card.
/// 3. Local VM sources append as-is (unique per `(node, name)` by
///    construction).
/// 4. A manual source whose `(host, port, protocol)` is already offered is
///    deduped away; the rest append with honest `Unknown` reachability.
///
/// Output is sorted `(node, name, id)` case-insensitively so the published
/// roster is stable across ticks (grouping by node — design lock 3).
#[must_use]
pub fn merge_sources(
    advertised: &[AdvertisedDesktop],
    mdns: &[MdnsEndpoint],
    local_vms: &[DesktopSource],
    manual: &[ManualSource],
) -> Vec<DesktopSource> {
    let mut out: Vec<DesktopSource> = advertised.iter().map(source_from_advertised).collect();

    for ep in mdns {
        let seat = out.iter().position(|s| {
            s.id.starts_with("peer:")
                && (s.host == ep.host || s.node.eq_ignore_ascii_case(&ep.instance))
        });
        match seat {
            Some(i) => {
                if !out[i].protocols.iter().any(|p| p.protocol == ep.protocol) {
                    out[i]
                        .protocols
                        .push(ProtocolOffer::new(ep.protocol, Some(ep.port)));
                }
            }
            None => out.push(source_from_mdns(ep)),
        }
    }

    out.extend(local_vms.iter().cloned());

    for m in manual {
        let dup = out.iter().any(|s| {
            s.host == m.host
                && s.protocols
                    .iter()
                    .any(|p| p.protocol == m.protocol && p.port == Some(m.port))
        });
        if !dup {
            out.push(source_from_manual(m));
        }
    }

    for s in &mut out {
        s.protocols.sort_unstable();
        s.protocols.dedup();
    }
    out.sort_by(|a, b| {
        a.node
            .to_lowercase()
            .cmp(&b.node.to_lowercase())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

// ─────────────────── universal ResourceCard adapter seam ───────────────────

/// Keep the adapter's retained observation window comfortably inside the
/// shared resource contract's bounded freshness range. The desktop roster is
/// still published on its compatibility topic; this function is a pure seam
/// for consumers migrating to the shared ResourceCard contract.
const DESKTOP_CARD_TTL_MS: u64 = 5 * 60 * 1_000;
/// Desktop discovery currently has three closed protocol variants. Keep the
/// adapter bounded even if a malformed in-memory roster bypasses the normal
/// merge fold.
const MAX_DESKTOP_ADAPTER_PROTOCOLS: usize = 8;
const MAX_DESKTOP_ADAPTER_HOST_BYTES: usize = 255;
const MAX_DESKTOP_ADAPTER_NAME_BYTES: usize = 512;
const MAX_DESKTOP_ADAPTER_REASON_BYTES: usize = 1_024;
/// MdnsEndpoint does not carry the OS interface name, but the shared
/// provenance contract requires a bounded interface token for mDNS evidence.
const DESKTOP_MDNS_PROVENANCE_INTERFACE: &str = "mdns";

#[derive(Debug, Clone, Copy)]
struct DesktopResourceMetadata {
    discovery_source: DiscoverySource,
    scope: ResourceScope,
    trust: ProvenanceTrust,
    authority: IdentityAuthority,
    label: &'static str,
    interface: Option<&'static str>,
}

fn desktop_resource_metadata(origin: SourceOrigin) -> DesktopResourceMetadata {
    match origin {
        SourceOrigin::MeshPeer => DesktopResourceMetadata {
            discovery_source: DiscoverySource::MeshDirectory,
            scope: ResourceScope::Mesh,
            trust: ProvenanceTrust::AuthenticatedMesh,
            authority: IdentityAuthority::Mesh,
            label: "mesh",
            interface: None,
        },
        SourceOrigin::Mdns => DesktopResourceMetadata {
            discovery_source: DiscoverySource::MdnsDnsSd,
            scope: ResourceScope::TrustedLan,
            trust: ProvenanceTrust::ObservedLan,
            authority: IdentityAuthority::Dns,
            label: "mDNS",
            interface: Some(DESKTOP_MDNS_PROVENANCE_INTERFACE),
        },
        SourceOrigin::LocalVm => DesktopResourceMetadata {
            discovery_source: DiscoverySource::Local,
            scope: ResourceScope::Local,
            trust: ProvenanceTrust::SelfReported,
            authority: IdentityAuthority::Local,
            label: "local",
            interface: None,
        },
        SourceOrigin::Manual => DesktopResourceMetadata {
            discovery_source: DiscoverySource::Manual,
            scope: ResourceScope::TrustedLan,
            trust: ProvenanceTrust::OperatorDeclared,
            authority: IdentityAuthority::Operator,
            label: "manual",
            interface: None,
        },
    }
}

fn validate_adapter_text(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), ResourceValidationError> {
    if value.len() > max_bytes {
        return Err(ResourceValidationError::FieldTooLong(field));
    }
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(ResourceValidationError::InvalidField(field));
    }
    Ok(())
}

fn validate_adapter_host(host: &str) -> Result<(), ResourceValidationError> {
    validate_adapter_text("desktop_source.host", host, MAX_DESKTOP_ADAPTER_HOST_BYTES)?;
    if !host.is_ascii()
        || host.chars().any(|character| {
            !(character.is_ascii_alphanumeric()
                || matches!(character, '-' | '_' | '.' | ':' | '[' | ']' | '%'))
        })
    {
        return Err(ResourceValidationError::InvalidField("desktop_source.host"));
    }
    Ok(())
}

fn desktop_transport_protocol(protocol: DesktopProtocol) -> TransportProtocol {
    match protocol {
        DesktopProtocol::Rdp => TransportProtocol::Rdp,
        DesktopProtocol::Vnc => TransportProtocol::Vnc,
        DesktopProtocol::Spice => TransportProtocol::Spice,
    }
}

fn desktop_endpoint_for_offer(
    source: &DesktopSource,
    offer: ProtocolOffer,
) -> Option<TransportEndpoint> {
    let port = offer.port.or_else(|| offer.protocol.default_port());
    match port {
        Some(port) => Some(TransportEndpoint::Network {
            host: source.host.clone(),
            port,
            base_path: None,
        }),
        // Local VM Spice is a typed brokered platform service, not a guessed
        // network port. A remote brokered Spice offer has no concrete endpoint
        // in the current DesktopSource contract, so it remains evidence-only.
        None if source.origin == SourceOrigin::LocalVm
            && offer.protocol == DesktopProtocol::Spice =>
        {
            Some(TransportEndpoint::LocalService {
                service_id: source.id.clone(),
            })
        }
        None => None,
    }
}

fn desktop_client_capability(
    protocol: TransportProtocol,
) -> Result<ClientCapability, ResourceValidationError> {
    let adapter_id = match protocol {
        TransportProtocol::Rdp => "construct.mde-vdi-rdp",
        TransportProtocol::Vnc => "construct.mde-vdi-vnc",
        TransportProtocol::Spice => "construct.mde-vdi-spice",
        _ => {
            return Err(ResourceValidationError::InvalidRelationship(
                "desktop_source.protocol",
            ));
        }
    };
    ClientCapability::new(
        adapter_id,
        "1",
        protocol,
        "1",
        ClientBoundary::ShellNative,
        vec![AuthMethod::MeshIdentity, AuthMethod::LocalApproval],
        vec![
            ClientFeature::Display,
            ClientFeature::KeyboardInput,
            ClientFeature::PointerInput,
        ],
        ClientCapabilityLimits {
            max_width: Some(3_840),
            max_height: Some(2_160),
            max_fps: Some(60),
            max_audio_channels: None,
            max_parallel_sessions: 1,
        },
        vec![ResourceActionVerb::Connect],
    )
}

fn desktop_failure(
    source: &DesktopSource,
    code: FailureCode,
    fallback: &'static str,
) -> FailureReason {
    FailureReason {
        code,
        message: source
            .reason
            .clone()
            .unwrap_or_else(|| fallback.to_string()),
    }
}

fn desktop_health(source: &DesktopSource, observed_at_ms: u64, expires_at_ms: u64) -> HealthState {
    let (status, failure) = match source.reachability {
        Reachability::Reachable => (HealthStatus::Available, None),
        Reachability::Unreachable => (
            HealthStatus::Unavailable,
            Some(desktop_failure(
                source,
                FailureCode::Unreachable,
                "desktop source is unreachable",
            )),
        ),
        Reachability::Unknown => (
            HealthStatus::Unknown,
            Some(desktop_failure(
                source,
                FailureCode::NotObserved,
                "desktop source reachability is unknown",
            )),
        ),
    };
    HealthState {
        schema_version: RESOURCE_CONTRACT_VERSION,
        status,
        observed_at_ms,
        expires_at_ms,
        latency_ms: None,
        failure,
    }
}

fn desktop_auth_state(origin: SourceOrigin, observed_at_ms: u64) -> AuthState {
    match origin {
        SourceOrigin::MeshPeer => AuthState {
            schema_version: RESOURCE_CONTRACT_VERSION,
            status: AuthStatus::Authorized,
            accepted_methods: vec![AuthMethod::MeshIdentity],
            active_method: Some(AuthMethod::MeshIdentity),
            credential_ref: None,
            updated_at_ms: observed_at_ms,
            expires_at_ms: None,
            failure: None,
        },
        SourceOrigin::LocalVm => AuthState {
            schema_version: RESOURCE_CONTRACT_VERSION,
            status: AuthStatus::NotRequired,
            accepted_methods: vec![],
            active_method: None,
            credential_ref: None,
            updated_at_ms: observed_at_ms,
            expires_at_ms: None,
            failure: None,
        },
        SourceOrigin::Mdns | SourceOrigin::Manual => AuthState {
            schema_version: RESOURCE_CONTRACT_VERSION,
            status: AuthStatus::Required,
            accepted_methods: vec![AuthMethod::LocalApproval],
            active_method: None,
            credential_ref: None,
            updated_at_ms: observed_at_ms,
            expires_at_ms: None,
            failure: None,
        },
    }
}

fn desktop_connect_availability(source: &DesktopSource) -> ActionAvailability {
    match source.reachability {
        Reachability::Reachable => match source.origin {
            SourceOrigin::MeshPeer | SourceOrigin::LocalVm => ActionAvailability {
                status: ActionAvailabilityStatus::Ready,
                failure: None,
            },
            SourceOrigin::Mdns | SourceOrigin::Manual => ActionAvailability {
                status: ActionAvailabilityStatus::RequiresApproval,
                failure: Some(FailureReason {
                    code: FailureCode::ApprovalRequired,
                    message: "desktop source requires local approval".to_string(),
                }),
            },
        },
        Reachability::Unreachable => ActionAvailability {
            status: ActionAvailabilityStatus::Unavailable,
            failure: Some(desktop_failure(
                source,
                FailureCode::Unreachable,
                "desktop source is unreachable",
            )),
        },
        Reachability::Unknown => ActionAvailability {
            status: ActionAvailabilityStatus::Unavailable,
            failure: Some(desktop_failure(
                source,
                FailureCode::NotObserved,
                "desktop source reachability is unknown",
            )),
        },
    }
}

fn ssdp_health(reachability: Reachability, observed_at_ms: u64, expires_at_ms: u64) -> HealthState {
    let (status, failure) = match reachability {
        Reachability::Reachable => (HealthStatus::Available, None),
        Reachability::Unknown => (
            HealthStatus::Unknown,
            Some(FailureReason {
                code: FailureCode::NotObserved,
                message: "SSDP desktop reachability is unknown".into(),
            }),
        ),
        Reachability::Unreachable => (
            HealthStatus::Unavailable,
            Some(FailureReason {
                code: FailureCode::Unreachable,
                message: "SSDP desktop is unreachable".into(),
            }),
        ),
    };
    HealthState {
        schema_version: RESOURCE_CONTRACT_VERSION,
        status,
        observed_at_ms,
        expires_at_ms,
        latency_ms: None,
        failure,
    }
}

fn ssdp_connect_availability(reachability: Reachability) -> ActionAvailability {
    match reachability {
        Reachability::Reachable => ActionAvailability {
            status: ActionAvailabilityStatus::RequiresApproval,
            failure: Some(FailureReason {
                code: FailureCode::ApprovalRequired,
                message: "SSDP desktop source requires local approval".into(),
            }),
        },
        Reachability::Unknown => ActionAvailability {
            status: ActionAvailabilityStatus::Unavailable,
            failure: Some(FailureReason {
                code: FailureCode::NotObserved,
                message: "SSDP desktop reachability is unknown".into(),
            }),
        },
        Reachability::Unreachable => ActionAvailability {
            status: ActionAvailabilityStatus::Unavailable,
            failure: Some(FailureReason {
                code: FailureCode::Unreachable,
                message: "SSDP desktop is unreachable".into(),
            }),
        },
    }
}

fn ssdp_resource_card_from_records(
    records: &[&SsdpPublishedAdvertisement],
) -> Result<ResourceCard, ResourceValidationError> {
    let first = records
        .first()
        .expect("SSDP adapter never builds an empty source group");
    let advertisement = &first.advertisement;
    let observed_at_ms = first.provenance.observed_at_ms;
    let expires_at_ms = first.provenance.expires_at_ms;
    let identity = ResourceIdentity::new(
        ResourceClass::Desktop,
        IdentityAuthority::Device,
        advertisement.source_id.clone(),
        vec![ResourceAlias {
            kind: ResourceAliasKind::DeviceUuid,
            value: advertisement.source_id.clone(),
        }],
    )?;
    let health = ssdp_health(
        advertisement.reachability.unwrap_or(first.reachability),
        observed_at_ms,
        expires_at_ms,
    );
    let mut capabilities = Vec::new();
    let mut transports = Vec::new();
    let mut transport_fingerprints = BTreeSet::new();
    for record in records {
        let protocol = desktop_transport_protocol(record.advertisement.protocol);
        let capability = if let Some(capability) = capabilities
            .iter()
            .find(|capability: &&ClientCapability| capability.protocol == protocol)
        {
            capability.clone()
        } else {
            let capability = desktop_client_capability(protocol)?;
            capabilities.push(capability.clone());
            capability
        };
        let transport = TransportCandidate::new(
            protocol,
            TransportEndpoint::Network {
                host: record.advertisement.host.clone(),
                port: record.advertisement.port,
                base_path: None,
            },
            ResourceScope::TrustedLan,
            0,
            observed_at_ms,
            expires_at_ms,
            health.clone(),
            Some(capability.fingerprint.clone()),
        )?;
        if transport_fingerprints.insert(transport.fingerprint.clone()) {
            transports.push(transport);
        }
    }
    transports.sort_unstable_by(|left, right| {
        left.protocol
            .cmp(&right.protocol)
            .then_with(|| left.fingerprint.cmp(&right.fingerprint))
    });
    capabilities.sort_unstable_by(|left, right| left.fingerprint.cmp(&right.fingerprint));

    let mut actions = vec![ResourceAction {
        schema_version: RESOURCE_CONTRACT_VERSION,
        action_id: "inspect".into(),
        verb: ResourceActionVerb::Inspect,
        target: ResourceActionTarget::Resource,
        availability: ActionAvailability {
            status: ActionAvailabilityStatus::Ready,
            failure: None,
        },
        issued_at_ms: observed_at_ms,
        expires_at_ms,
    }];
    for (index, transport) in transports.iter().enumerate() {
        let capability_fingerprint = transport.client_capability_fingerprint.clone().ok_or(
            ResourceValidationError::InvalidRelationship("ssdp.transport_capability"),
        )?;
        actions.push(ResourceAction {
            schema_version: RESOURCE_CONTRACT_VERSION,
            action_id: format!("connect-{}-{index}", transport.protocol.token()),
            verb: ResourceActionVerb::Connect,
            target: ResourceActionTarget::TransportClient {
                transport_fingerprint: transport.fingerprint.clone(),
                capability_fingerprint,
            },
            availability: ssdp_connect_availability(first.reachability),
            issued_at_ms: observed_at_ms,
            expires_at_ms,
        });
    }

    let card = ResourceCard {
        schema_version: RESOURCE_CONTRACT_VERSION,
        identity,
        display_name: advertisement.display_name.clone(),
        summary: Some("SSDP trusted-LAN desktop".into()),
        first_seen_at_ms: observed_at_ms,
        last_seen_at_ms: observed_at_ms,
        expires_at_ms,
        health,
        auth: AuthState {
            schema_version: RESOURCE_CONTRACT_VERSION,
            status: AuthStatus::Required,
            accepted_methods: vec![AuthMethod::LocalApproval],
            active_method: None,
            credential_ref: None,
            updated_at_ms: observed_at_ms,
            expires_at_ms: None,
            failure: None,
        },
        provenance: vec![first.provenance.clone()],
        transports,
        client_capabilities: capabilities,
        actions,
        operating_roles: vec![mackes_mesh_types::resources::ResourceOperatingRole::Client],
        service: None,
    };
    card.validate()?;
    Ok(card)
}

/// Project one validated desktop discovery row into the shared resource-card
/// contract without probing, launching, or executing any endpoint data.
///
/// Only the existing typed ProtocolOffer variants can produce a transport
/// and registered client capability. RDP/VNC use their typed well-known port
/// when the offer leaves it to connect time; local brokered Spice uses a typed
/// LocalService endpoint. A source that is unreachable or not observed never
/// receives a ready action, and this discovery seam never emits Launch.
///
/// # Errors
///
/// Returns a ResourceValidationError when the source exceeds the adapter
/// bounds or the resulting card fails the shared contract validation.
pub fn resource_card_from_desktop_source(
    source: &DesktopSource,
    observed_at_ms: u64,
) -> Result<ResourceCard, ResourceValidationError> {
    if source.protocols.len() > MAX_DESKTOP_ADAPTER_PROTOCOLS {
        return Err(ResourceValidationError::CapacityExceeded {
            field: "desktop_source.protocols",
            max: MAX_DESKTOP_ADAPTER_PROTOCOLS,
        });
    }
    validate_adapter_host(&source.host)?;
    validate_adapter_text(
        "desktop_source.name",
        &source.name,
        MAX_DESKTOP_ADAPTER_NAME_BYTES,
    )?;
    if let Some(reason) = &source.reason {
        validate_adapter_text(
            "desktop_source.reason",
            reason,
            MAX_DESKTOP_ADAPTER_REASON_BYTES,
        )?;
    }
    let expires_at_ms = observed_at_ms.checked_add(DESKTOP_CARD_TTL_MS).ok_or(
        ResourceValidationError::InvalidTimestamp("desktop_resource_card.freshness"),
    )?;
    let metadata = desktop_resource_metadata(source.origin);
    let identity = ResourceIdentity::new(
        ResourceClass::Desktop,
        metadata.authority,
        source.id.clone(),
        vec![ResourceAlias {
            kind: ResourceAliasKind::LegacyId,
            value: source.id.clone(),
        }],
    )?;
    let provenance = SourceProvenance {
        schema_version: RESOURCE_CONTRACT_VERSION,
        source: metadata.discovery_source,
        source_id: source.id.clone(),
        scope: metadata.scope,
        trust: metadata.trust,
        interface: metadata.interface.map(str::to_string),
        observed_at_ms,
        expires_at_ms,
    };
    let health = desktop_health(source, observed_at_ms, expires_at_ms);
    let mut capabilities: Vec<ClientCapability> = Vec::new();
    let mut transports = Vec::new();
    let mut transport_fingerprints = BTreeSet::new();
    let mut unsupported_protocols = BTreeSet::new();
    for offer in &source.protocols {
        let Some(endpoint) = desktop_endpoint_for_offer(source, *offer) else {
            unsupported_protocols.insert(offer.protocol);
            continue;
        };
        let protocol = desktop_transport_protocol(offer.protocol);
        let capability = if let Some(capability) = capabilities
            .iter()
            .find(|capability: &&ClientCapability| capability.protocol == protocol)
        {
            capability.clone()
        } else {
            let capability = desktop_client_capability(protocol)?;
            capabilities.push(capability.clone());
            capability
        };
        let transport = TransportCandidate::new(
            protocol,
            endpoint,
            metadata.scope,
            0,
            observed_at_ms,
            expires_at_ms,
            health.clone(),
            Some(capability.fingerprint.clone()),
        )?;
        if transport_fingerprints.insert(transport.fingerprint.clone()) {
            transports.push(transport);
        }
    }
    transports.sort_unstable_by(|left, right| left.fingerprint.cmp(&right.fingerprint));
    capabilities.sort_unstable_by(|left, right| left.fingerprint.cmp(&right.fingerprint));

    let summary = if unsupported_protocols.is_empty() {
        format!("{} desktop source", metadata.label)
    } else {
        let protocols = unsupported_protocols
            .iter()
            .map(|protocol| protocol.tag())
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{} desktop source; brokered {} offer has no concrete endpoint",
            metadata.label, protocols
        )
    };
    let mut actions = vec![ResourceAction {
        schema_version: RESOURCE_CONTRACT_VERSION,
        action_id: "inspect".to_string(),
        verb: ResourceActionVerb::Inspect,
        target: ResourceActionTarget::Resource,
        availability: ActionAvailability {
            status: ActionAvailabilityStatus::Ready,
            failure: None,
        },
        issued_at_ms: observed_at_ms,
        expires_at_ms,
    }];
    for (index, transport) in transports.iter().enumerate() {
        let capability_fingerprint = transport.client_capability_fingerprint.clone().ok_or(
            ResourceValidationError::InvalidRelationship("desktop_source.transport_capability"),
        )?;
        actions.push(ResourceAction {
            schema_version: RESOURCE_CONTRACT_VERSION,
            action_id: format!("connect-{}-{index}", transport.protocol.token()),
            verb: ResourceActionVerb::Connect,
            target: ResourceActionTarget::TransportClient {
                transport_fingerprint: transport.fingerprint.clone(),
                capability_fingerprint,
            },
            availability: desktop_connect_availability(source),
            issued_at_ms: observed_at_ms,
            expires_at_ms,
        });
    }
    let card = ResourceCard {
        schema_version: RESOURCE_CONTRACT_VERSION,
        identity,
        display_name: source.name.clone(),
        summary: Some(summary),
        first_seen_at_ms: observed_at_ms,
        last_seen_at_ms: observed_at_ms,
        expires_at_ms,
        health,
        auth: desktop_auth_state(source.origin, observed_at_ms),
        provenance: vec![provenance],
        transports,
        client_capabilities: capabilities,
        actions,
        operating_roles: vec![mackes_mesh_types::resources::ResourceOperatingRole::Client],
        service: None,
    };
    card.validate()?;
    Ok(card)
}

// ───────────────────────── the published record ─────────────────────────

/// One discovery lane's honest status (`ok …` / `gated: …` / `error: …`) —
/// so the Chooser can say WHY a lane is empty instead of silently omitting
/// sources (§7).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaneStatus {
    /// Lane name (`mesh-registry` / `mdns` / `local-kvm` / `manual`).
    pub lane: String,
    /// Status string.
    pub status: String,
}

/// The full record published to [`SOURCES_TOPIC`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DesktopSourcesState {
    /// Publishing node id.
    pub node: String,
    /// The merged, deduped source roster.
    pub sources: Vec<DesktopSource>,
    /// Per-lane discovery status.
    pub lanes: Vec<LaneStatus>,
    /// Wall-clock publish time (ms since the Unix epoch).
    pub published_at_ms: u64,
}

// ───────────────────────────── the worker ─────────────────────────────

/// The live mDNS browse handles. The daemon handle is held for the worker's
/// lifetime (dropping it would tear the browse down).
struct MdnsBrowse {
    _daemon: ServiceDaemon,
    browsers: Vec<(&'static str, mdns_sd::Receiver<ServiceEvent>)>,
}

/// CHOOSER-1 — the desktop-source discovery aggregator worker.
pub struct DesktopSourcesWorker {
    /// This node's id (the publish stamp + the local-VM `node`).
    node_id: String,
    /// The replicated workgroup root the peers plane lives under.
    workgroup_root: PathBuf,
    /// Node-local root the manual-source store persists under.
    store_root: PathBuf,
    /// The injectable local-VM enumeration seam.
    vms: Arc<dyn VmEnumerator>,
    /// Action-drain cadence.
    tick: Duration,
    /// Unconditional-republish heartbeat.
    heartbeat: Duration,
    /// Bus root override (tests). `None` ⇒ `mde_bus::default_data_dir`.
    bus_root_override: Option<PathBuf>,
    /// The manual sources (mirrors the on-disk store).
    manual: Vec<ManualSource>,
    /// Live mDNS endpoints, keyed by fullname (the daemon's removal key).
    mdns_seen: HashMap<String, MdnsEndpoint>,
    /// mDNS lane status for the published record.
    mdns_lane: String,
    /// local-kvm lane status for the published record.
    vm_lane: String,
    /// Per-action-topic drain cursors.
    cursors: HashMap<&'static str, String>,
    /// Fingerprint of the last published fold (publish-on-change gate).
    last_fingerprint: Option<String>,
    /// Shared, fail-closed authorization gate for manual-source mutations.
    authorizer: Arc<ActionAuthorizer>,
}

impl DesktopSourcesWorker {
    /// Construct with production seams: the [`WorkloadEnumerator`] VM lane and
    /// the default cadences. `node_id` stamps the publish; `workgroup_root`
    /// locates the peers plane; `store_root` holds the manual-source store.
    #[must_use]
    pub fn new(node_id: String, workgroup_root: PathBuf, store_root: PathBuf) -> Self {
        Self {
            node_id: node_id.clone(),
            workgroup_root,
            store_root,
            vms: Arc::new(WorkloadEnumerator::new(node_id.clone())),
            tick: DEFAULT_TICK_INTERVAL,
            heartbeat: PUBLISH_HEARTBEAT,
            bus_root_override: None,
            manual: Vec::new(),
            mdns_seen: HashMap::new(),
            mdns_lane: "idle".to_string(),
            vm_lane: "idle".to_string(),
            cursors: HashMap::new(),
            last_fingerprint: None,
            authorizer: Arc::new(ActionAuthorizer::production()),
        }
    }

    /// Inject the VM-enumeration seam (tests).
    #[must_use]
    pub fn with_enumerator(mut self, vms: Arc<dyn VmEnumerator>) -> Self {
        self.vms = vms;
        self
    }

    /// Override the Bus spool root (tests).
    #[must_use]
    pub fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    /// Override the action-drain cadence (tests avoid multi-second waits).
    #[must_use]
    pub const fn with_tick(mut self, d: Duration) -> Self {
        self.tick = d;
        self
    }

    /// Inject an isolated verifier and replay ledger for hostile action tests.
    /// Production always uses the systemd-credential-backed authorizer.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_authorizer(mut self, authorizer: Arc<ActionAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    /// Add a manual source (idempotent on the id). Returns whether the set
    /// changed; a change persists the store.
    fn handle_add(&mut self, req: ManualSource) -> bool {
        if self.manual.iter().any(|m| m.id() == req.id()) {
            return false;
        }
        self.manual.push(req);
        self.persist_manual();
        true
    }

    /// Remove a manual source by id. Only manual sources are removable —
    /// discovered sources reappear by discovery, so removing one would be a
    /// lie; a non-manual id is a logged no-op.
    fn handle_remove(&mut self, id: &str) -> bool {
        let before = self.manual.len();
        self.manual.retain(|m| m.id() != id);
        let changed = self.manual.len() != before;
        if changed {
            self.persist_manual();
        } else {
            tracing::warn!(
                target: "mackesd::desktop_sources",
                id,
                "remove-source: not a manual source id; ignored",
            );
        }
        changed
    }

    fn persist_manual(&self) {
        if let Err(e) = save_manual_sources(&self.store_root, &self.manual) {
            tracing::warn!(
                target: "mackesd::desktop_sources",
                error = %e,
                "manual-source store write failed",
            );
        }
    }

    /// Authenticate one raw manual-source mutation before parsing it into a
    /// typed request or touching the node-local store. Targets are the stable
    /// manual-source id, so a capability cannot be replayed for another
    /// endpoint. The refresh verb deliberately stays outside this helper: it
    /// performs no mutation, only read-only discovery and a derived publish.
    fn authorize_mutation(&self, topic: &'static str, body: &str) -> Result<(), String> {
        let (verb, target) = match topic {
            ADD_SOURCE_TOPIC => {
                let target = parse_add_source(body)
                    .map(|request| request.id())
                    .unwrap_or_default();
                (DESKTOP_ADD_SOURCE_AUTH_VERB, target)
            }
            REMOVE_SOURCE_TOPIC => {
                let target = parse_remove_source(body)
                    .map(|request| request.id)
                    .unwrap_or_default();
                (DESKTOP_REMOVE_SOURCE_AUTH_VERB, target)
            }
            other => return Err(format!("unknown desktop mutation topic: {other}")),
        };
        self.authorizer.authorize(
            body,
            MutationContext {
                verb,
                node: &self.node_id,
                target: &target,
            },
        )
    }

    /// Drain one action topic since its cursor, returning the new bodies.
    fn drain_topic(
        persist: &Persist,
        topic: &'static str,
        cursors: &mut HashMap<&'static str, String>,
    ) -> Vec<String> {
        let cursor = cursors.get(topic).cloned();
        let Ok(msgs) = persist.list_since(topic, cursor.as_deref()) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for msg in msgs {
            cursors.insert(topic, msg.ulid.clone());
            out.push(msg.body.unwrap_or_default());
        }
        out
    }

    /// Drain the three typed verbs. Returns `(manual_changed, refresh)`.
    fn drain_actions(&mut self, persist: &Persist) -> (bool, bool) {
        let mut changed = false;
        for body in Self::drain_topic(persist, ADD_SOURCE_TOPIC, &mut self.cursors) {
            if let Err(error) = self.authorize_mutation(ADD_SOURCE_TOPIC, &body) {
                tracing::warn!(
                    target: "mackesd::desktop_sources",
                    error = %error,
                    "refused unauthorized add-source mutation"
                );
                continue;
            }
            match parse_add_source(&body) {
                Ok(req) => changed |= self.handle_add(req),
                Err(e) => {
                    tracing::warn!(target: "mackesd::desktop_sources", error = %e, "bad add-source");
                }
            }
        }
        for body in Self::drain_topic(persist, REMOVE_SOURCE_TOPIC, &mut self.cursors) {
            if let Err(error) = self.authorize_mutation(REMOVE_SOURCE_TOPIC, &body) {
                tracing::warn!(
                    target: "mackesd::desktop_sources",
                    error = %error,
                    "refused unauthorized remove-source mutation"
                );
                continue;
            }
            match parse_remove_source(&body) {
                Ok(req) => changed |= self.handle_remove(&req.id),
                Err(e) => {
                    tracing::warn!(target: "mackesd::desktop_sources", error = %e, "bad remove-source");
                }
            }
        }
        // Refresh is an open, harmless read/nudge: it only re-enumerates
        // discovery and republishes the derived state; it never updates the
        // manual store or invokes a privileged mutator.
        let refresh = !Self::drain_topic(persist, REFRESH_TOPIC, &mut self.cursors).is_empty();
        (changed, refresh)
    }

    /// Drain pending mDNS browse events into the live endpoint cache.
    fn drain_mdns(&mut self, browse: Option<&MdnsBrowse>) -> bool {
        let Some(browse) = browse else { return false };
        let mut changed = false;
        for (bare, rx) in &browse.browsers {
            while let Ok(event) = rx.try_recv() {
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        if let Some(ep) = endpoint_from_service_info(bare, &info) {
                            let prev = self.mdns_seen.insert(ep.fullname.clone(), ep.clone());
                            if prev.as_ref() != Some(&ep) {
                                changed = true;
                            }
                        }
                    }
                    ServiceEvent::ServiceRemoved(_ty, fullname) => {
                        changed |= self.mdns_seen.remove(&fullname).is_some();
                    }
                    _ => {}
                }
            }
        }
        changed
    }

    /// Fold a VM enumeration outcome into the lane status + the instance
    /// list (an error contributes NO sources — never a fake, §7).
    fn fold_vm_result(&mut self, res: Result<Vec<Instance>, VmEnumerateError>) -> Vec<Instance> {
        match res {
            Ok(list) => {
                self.vm_lane = format!("ok ({} vms)", list.len());
                list
            }
            Err(e) => {
                tracing::debug!(target: "mackesd::desktop_sources", error = %e, "vm enumeration unavailable");
                self.vm_lane = e.to_string();
                Vec::new()
            }
        }
    }

    /// Enumerate local VMs on a blocking thread (virsh shells out).
    async fn enumerate_vms(&mut self) -> Vec<Instance> {
        let vms = Arc::clone(&self.vms);
        let res = match tokio::task::spawn_blocking(move || vms.enumerate()).await {
            Ok(r) => r,
            Err(e) => Err(VmEnumerateError::Backend(format!("enumerate join: {e}"))),
        };
        self.fold_vm_result(res)
    }

    /// Read the peers plane + fold every lane into the merged roster.
    fn collect_sources(&self, vm_list: &[Instance]) -> Vec<DesktopSource> {
        let peers = read_peers(&peers_dir(&self.workgroup_root));
        let mut advertised = Vec::new();
        for rec in &peers {
            advertised.extend(advertised_from_peer(rec, &self.node_id));
        }
        let mut mdns: Vec<MdnsEndpoint> = self.mdns_seen.values().cloned().collect();
        mdns.sort_by(|a, b| a.fullname.cmp(&b.fullname));
        let vms: Vec<DesktopSource> = vm_list
            .iter()
            .map(|i| source_from_vm(&self.node_id, i))
            .collect();
        merge_sources(&advertised, &mdns, &vms, &self.manual)
    }

    fn lanes(&self) -> Vec<LaneStatus> {
        vec![
            LaneStatus {
                lane: "mesh-registry".to_string(),
                status: "ok".to_string(),
            },
            LaneStatus {
                lane: "mdns".to_string(),
                status: self.mdns_lane.clone(),
            },
            LaneStatus {
                lane: "local-kvm".to_string(),
                status: self.vm_lane.clone(),
            },
            LaneStatus {
                lane: "manual".to_string(),
                status: format!("ok ({} sources)", self.manual.len()),
            },
        ]
    }

    /// Publish the roster when the fold changed (or `force`). Returns whether
    /// a record was written.
    fn publish(&mut self, persist: &Persist, sources: Vec<DesktopSource>, force: bool) -> bool {
        let lanes = self.lanes();
        let fingerprint = serde_json::to_string(&(&sources, &lanes)).unwrap_or_default();
        if !force && self.last_fingerprint.as_deref() == Some(fingerprint.as_str()) {
            return false;
        }
        let state = DesktopSourcesState {
            node: self.node_id.clone(),
            sources,
            lanes,
            published_at_ms: now_ms(),
        };
        let Ok(body) = serde_json::to_string(&state) else {
            return false;
        };
        if let Err(e) = persist.write(SOURCES_TOPIC, Priority::Default, None, Some(&body)) {
            tracing::warn!(target: "mackesd::desktop_sources", error = %e, "sources publish failed");
            return false;
        }
        self.last_fingerprint = Some(fingerprint);
        true
    }

    /// Start the desktop-type mDNS browsers (graceful degrade: no daemon /
    /// no multicast interface → an honest `gated:` lane, worker keeps
    /// aggregating the other lanes).
    fn start_mdns_browsers(&mut self) -> Option<MdnsBrowse> {
        let daemon = match ServiceDaemon::new() {
            Ok(d) => d,
            Err(e) => {
                self.mdns_lane = format!("gated: no mDNS daemon ({e})");
                return None;
            }
        };
        let mut browsers = Vec::new();
        for bare in DESKTOP_MDNS_TYPES {
            match daemon.browse(&super::mdns_relay::browse_type(bare)) {
                Ok(rx) => browsers.push((*bare, rx)),
                Err(e) => {
                    tracing::warn!(target: "mackesd::desktop_sources", service_type = bare, error = %e, "mdns browse failed");
                }
            }
        }
        if browsers.is_empty() {
            self.mdns_lane = "gated: no mDNS browse".to_string();
            return None;
        }
        self.mdns_lane = format!("ok ({} types)", browsers.len());
        Some(MdnsBrowse {
            _daemon: daemon,
            browsers,
        })
    }
}

#[async_trait::async_trait]
impl Worker for DesktopSourcesWorker {
    fn name(&self) -> &'static str {
        "desktop_sources"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self
            .bus_root_override
            .clone()
            .or_else(mde_bus::default_data_dir)
        else {
            tracing::debug!(target: "mackesd::desktop_sources", "no bus root; worker idle");
            return Ok(());
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(target: "mackesd::desktop_sources", error = %e, "persist open failed; worker idle");
                return Ok(());
            }
        };
        self.manual = load_manual_sources(&self.store_root);
        // Prime each verb cursor at its tail: manual sources are durable in
        // the store, so replaying an old add would resurrect a removed one.
        for topic in [ADD_SOURCE_TOPIC, REMOVE_SOURCE_TOPIC, REFRESH_TOPIC] {
            if let Ok(Some(ulid)) = persist.latest_ulid(topic) {
                self.cursors.insert(topic, ulid);
            }
        }
        let browse = self.start_mdns_browsers();

        // Immediate first publish so the Chooser doesn't wait a heartbeat.
        let vm_list = self.enumerate_vms().await;
        let sources = self.collect_sources(&vm_list);
        self.publish(&persist, sources, true);
        let mut last_pub = Instant::now();

        // Keep the immediate first roster above, but spread the first
        // recurring Workload/peer fold. The subtraction means the first
        // action scan is no later than the old tick boundary; shutdown remains
        // interruptible while the phase is pending.
        let first_delay = self
            .tick
            .saturating_sub(initial_phase_for(&self.node_id, self.tick));
        tokio::select! {
            _ = shutdown.wait() => return Ok(()),
            _ = tokio::time::sleep(first_delay) => {}
        }
        let mut tick = tokio::time::interval(self.tick);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let (changed, refresh) = self.drain_actions(&persist);
                    let mdns_changed = self.drain_mdns(browse.as_ref());
                    let due = last_pub.elapsed() >= self.heartbeat;
                    if changed || refresh || mdns_changed || due {
                        let vm_list = self.enumerate_vms().await;
                        let sources = self.collect_sources(&vm_list);
                        // A refresh/heartbeat republishes unconditionally
                        // (late subscribers); otherwise only on change.
                        if self.publish(&persist, sources, refresh || due) {
                            last_pub = Instant::now();
                        }
                    }
                }
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

/// Return a stable bounded phase for the first recurring desktop-source scan.
/// FNV-1a is sufficient here because this is scheduling spread, not a
/// security primitive. An empty identity deliberately keeps the old timing.
fn initial_phase_for(node_id: &str, tick: Duration) -> Duration {
    let window_ms = tick.as_millis().min(MAX_INITIAL_PHASE.as_millis());
    if node_id.is_empty() || window_ms == 0 {
        return Duration::ZERO;
    }

    let mut hash = 0xcbf29ce484222325_u64;
    for byte in node_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Duration::from_millis((u128::from(hash) % (window_ms + 1)) as u64)
}

/// Wall-clock epoch millis for the published record.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::action_auth::{authorize_test_body, ActionAuthorizer};
    use mackes_mesh_types::peers::{RemoteAccess, ServiceDescriptors, VmInfo};

    const AUTH_KEY: &[u8] = b"desktop-sources-action-auth-key";
    const AUTH_NOW: i64 = 1_700_000_000_000;

    fn peer(
        hostname: &str,
        health: &str,
        overlay_ip: Option<&str>,
        rdp: bool,
        vnc: bool,
        vms: Vec<VmInfo>,
    ) -> PeerRecord {
        let mut rec = PeerRecord::now(hostname, Some("12.0.0".into()), health);
        rec.overlay_ip = overlay_ip.map(str::to_string);
        rec.descriptors = Some(ServiceDescriptors {
            remote_access: RemoteAccess {
                ssh: true,
                rdp,
                vnc,
            },
            vms,
            ..ServiceDescriptors::default()
        });
        rec
    }

    fn vm_info(name: &str, state: &str) -> VmInfo {
        VmInfo {
            name: name.into(),
            state: state.into(),
            vcpus: Some(2),
            memory_mb: Some(2048),
            addresses: vec![],
        }
    }

    // ── lane 1: the advertised shape ──

    #[test]
    fn advertised_from_peer_lifts_seat_and_vm_desktops() {
        let rec = peer(
            "oak",
            "healthy",
            Some("10.42.0.7"),
            true,
            true,
            vec![vm_info("win11", "running"), vm_info("dev", "shut off")],
        );
        let ads = advertised_from_peer(&rec, "elm");
        assert_eq!(ads.len(), 3, "seat + two VMs");
        // The seat: RDP + VNC at their well-known default ports, overlay host.
        let seat = &ads[0];
        assert_eq!(seat.node, "oak");
        assert_eq!(seat.host, "10.42.0.7");
        assert!(seat.vm.is_none());
        assert_eq!(
            seat.protocols,
            vec![
                ProtocolOffer::new(DesktopProtocol::Rdp, Some(3389)),
                ProtocolOffer::new(DesktopProtocol::Vnc, Some(5900)),
            ]
        );
        assert_eq!(seat.reachability, Reachability::Reachable);
        // The running VM: a Spice console, reachable.
        let win = ads
            .iter()
            .find(|a| a.vm.as_deref() == Some("win11"))
            .unwrap();
        assert_eq!(
            win.protocols,
            vec![ProtocolOffer::new(DesktopProtocol::Spice, None)]
        );
        assert_eq!(win.power_state.as_deref(), Some("running"));
        assert_eq!(win.reachability, Reachability::Reachable);
        // The stopped VM: greyed with its state as the reason.
        let dev = ads.iter().find(|a| a.vm.as_deref() == Some("dev")).unwrap();
        assert_eq!(dev.reachability, Reachability::Unreachable);
        assert_eq!(dev.reason.as_deref(), Some("vm shut off"));
        assert_eq!(dev.power_state.as_deref(), Some("shut off"));
    }

    #[test]
    fn advertised_from_peer_skips_self_and_empty_advertisers() {
        let own = peer(
            "elm",
            "healthy",
            None,
            true,
            true,
            vec![vm_info("v", "running")],
        );
        assert!(
            advertised_from_peer(&own, "elm").is_empty(),
            "own record is skipped — local VMs ride the live KVM lane"
        );
        // A peer with no desktop listeners and no VMs advertises nothing
        // (ssh alone is not a desktop).
        let quiet = peer("ash", "healthy", None, false, false, vec![]);
        assert!(advertised_from_peer(&quiet, "elm").is_empty());
        // A pre-PD-2 writer (no descriptors) advertises nothing.
        let bare = PeerRecord::now("older", None, "healthy");
        assert!(advertised_from_peer(&bare, "elm").is_empty());
    }

    #[test]
    fn advertised_host_falls_back_to_mesh_fqdn() {
        let rec = peer("oak", "healthy", None, true, false, vec![]);
        let ads = advertised_from_peer(&rec, "elm");
        assert_eq!(ads[0].host, "oak.mesh");
    }

    #[test]
    fn peer_reachability_derivation_table() {
        assert_eq!(
            peer_reachability("healthy", false),
            (Reachability::Reachable, None)
        );
        assert_eq!(
            peer_reachability("degraded", false),
            (Reachability::Reachable, None)
        );
        assert_eq!(
            peer_reachability("critical", false),
            (Reachability::Reachable, None)
        );
        let (r, why) = peer_reachability("unreachable", false);
        assert_eq!(r, Reachability::Unreachable);
        assert_eq!(why.as_deref(), Some("peer unreachable"));
        // Staleness wins even over a healthy last word.
        let (r, why) = peer_reachability("healthy", true);
        assert_eq!(r, Reachability::Unreachable);
        assert_eq!(why.as_deref(), Some("peer heartbeat stale"));
        assert_eq!(
            peer_reachability("unknown", false),
            (Reachability::Unknown, None)
        );
    }

    #[test]
    fn stale_peer_desktops_grey_with_the_stale_reason() {
        let mut rec = peer("oak", "healthy", Some("10.42.0.7"), true, false, vec![]);
        rec.last_seen_ms = 1; // ancient
        let ads = advertised_from_peer(&rec, "elm");
        assert_eq!(ads[0].reachability, Reachability::Unreachable);
        assert_eq!(ads[0].reason.as_deref(), Some("peer heartbeat stale"));
    }

    // ── lane 2: the mDNS fold ──

    fn svc(bare: &str, instance: &str, port: u16, txt: &[(&str, &str)]) -> ServiceInfo {
        ServiceInfo::new(
            &super::super::mdns_relay::browse_type(bare),
            instance,
            &format!("{instance}.local."),
            "192.168.1.60",
            port,
            txt,
        )
        .unwrap()
    }

    #[test]
    fn mdns_fold_lifts_desktop_types() {
        let rdp = endpoint_from_service_info("_rdp._tcp", &svc("_rdp._tcp", "OfficePC", 3389, &[]))
            .unwrap();
        assert_eq!(rdp.protocol, DesktopProtocol::Rdp);
        assert_eq!(rdp.host, "192.168.1.60");
        assert_eq!(rdp.port, 3389);
        assert_eq!(rdp.instance, "OfficePC");
        let vnc =
            endpoint_from_service_info("_rfb._tcp", &svc("_rfb._tcp", "pi", 5900, &[])).unwrap();
        assert_eq!(vnc.protocol, DesktopProtocol::Vnc);
        let spice =
            endpoint_from_service_info("_spice._tcp", &svc("_spice._tcp", "vmhost", 5930, &[]))
                .unwrap();
        assert_eq!(spice.protocol, DesktopProtocol::Spice);
    }

    #[test]
    fn mdns_fold_skips_non_desktop_and_relayed_services() {
        // A non-desktop type never becomes a source.
        assert!(
            endpoint_from_service_info("_ssh._tcp", &svc("_ssh._tcp", "shell", 22, &[])).is_none()
        );
        // A service a mesh peer republished (mdns_relay's anti-loop TXT) is
        // skipped — the registry lane already carries that peer.
        let relayed = svc(
            "_rdp._tcp",
            "OfficePC-10-42-0-9",
            3389,
            &[(super::super::mdns_relay::RELAY_ORIGIN_TXT, "10.42.0.9")],
        );
        assert!(endpoint_from_service_info("_rdp._tcp", &relayed).is_none());
    }

    // ── future trusted-LAN SSDP/UPnP parser seam ──

    fn ssdp_headers(service_type: &str, protocol: &str, port: u16) -> SsdpHeaderMap {
        BTreeMap::from([
            ("USN".to_string(), format!("uuid:desk-01::{service_type}")),
            ("NT".to_string(), service_type.to_string()),
            ("X-MCNF-NAME".to_string(), "Office PC".to_string()),
            ("X-MCNF-HOST".to_string(), "192.168.1.60".to_string()),
            ("X-MCNF-PORT".to_string(), port.to_string()),
            ("X-MCNF-PROTOCOL".to_string(), protocol.to_string()),
        ])
    }

    #[test]
    fn ssdp_seam_normalizes_the_closed_rdp_vnc_and_spice_vocabulary() {
        for (service_type, protocol, port, expected) in [
            (
                MCNF_SSDP_RDP_SERVICE_TYPE,
                "rdp",
                3389,
                DesktopProtocol::Rdp,
            ),
            (
                MCNF_SSDP_VNC_SERVICE_TYPE,
                "vnc",
                5900,
                DesktopProtocol::Vnc,
            ),
            (
                MCNF_SSDP_SPICE_SERVICE_TYPE,
                "spice",
                5930,
                DesktopProtocol::Spice,
            ),
        ] {
            let ad =
                normalize_ssdp_header_map(&ssdp_headers(service_type, protocol, port)).unwrap();
            assert_eq!(ad.source_id, "uuid:desk-01");
            assert_eq!(ad.display_name, "Office PC");
            assert_eq!(ad.host, "192.168.1.60");
            assert_eq!(ad.port, port);
            assert_eq!(ad.protocol, expected);
            assert_eq!(ad.trust, None);
            assert_eq!(ad.reachability, None);
        }
    }

    #[test]
    fn ssdp_seam_accepts_case_insensitive_headers_and_explicit_observation() {
        let mut headers = SsdpHeaderMap::new();
        headers.insert(
            "uSn".into(),
            format!("UUID:Desk-Case::{MCNF_SSDP_RDP_SERVICE_TYPE}"),
        );
        headers.insert("nT".into(), MCNF_SSDP_RDP_SERVICE_TYPE.into());
        headers.insert("x-mcnf-name".into(), "Office PC".into());
        headers.insert("X-McNf-HoSt".into(), "office-pc.local".into());
        headers.insert("x-mcnf-port".into(), "3389".into());
        headers.insert("x-mcnf-protocol".into(), "RDP".into());
        let ad = normalize_ssdp_header_map_with_observation(
            &headers,
            SsdpObservation {
                trust: Some(ProvenanceTrust::ObservedLan),
                reachability: Some(Reachability::Reachable),
            },
        )
        .unwrap();
        assert_eq!(ad.source_id, "uuid:desk-case");
        assert_eq!(ad.host, "office-pc.local");
        assert_eq!(ad.trust, Some(ProvenanceTrust::ObservedLan));
        assert_eq!(ad.reachability, Some(Reachability::Reachable));
    }

    #[test]
    fn ssdp_seam_has_deterministic_uuid_identity_and_parses_raw_blocks() {
        let service_type = MCNF_SSDP_VNC_SERVICE_TYPE;
        let mut first = ssdp_headers(service_type, "vnc", 5900);
        first.insert("USN".into(), format!("uuid:Desk-Case::{service_type}"));
        let mut second = first.clone();
        second.remove("USN");
        second.insert("usn".into(), format!("UUID:desk-case::{service_type}"));
        // Same UUID with different case is one stable source.
        let one = normalize_ssdp_header_map(&first).unwrap();
        let two = normalize_ssdp_header_map(&second).unwrap();
        assert_eq!(one, two);

        let raw = format!(
            "NOTIFY * HTTP/1.1\r\n\
             USN: uuid:desk-raw::{service_type}\r\n\
             NT: {service_type}\r\n\
             X-MCNF-NAME: Office PC\r\n\
             X-MCNF-HOST: 192.168.1.61\r\n\
             X-MCNF-PORT: 5901\r\n\
             X-MCNF-PROTOCOL: vnc\r\n\
             \r\n"
        );
        let parsed = parse_ssdp_advertisement(&raw).unwrap();
        assert_eq!(parsed.source_id, "uuid:desk-raw");
        assert_eq!(parsed.protocol, DesktopProtocol::Vnc);
        assert_eq!(parsed.port, 5901);
    }

    #[test]
    fn ssdp_seam_rejects_missing_conflicting_and_oversized_input() {
        let mut missing = ssdp_headers(MCNF_SSDP_RDP_SERVICE_TYPE, "rdp", 3389);
        missing.remove("USN");
        assert!(matches!(
            normalize_ssdp_header_map(&missing),
            Err(SsdpAdvertisementError::MissingField("USN"))
        ));

        let mut conflicting = ssdp_headers(MCNF_SSDP_RDP_SERVICE_TYPE, "rdp", 3389);
        conflicting.insert("nt".into(), MCNF_SSDP_VNC_SERVICE_TYPE.into());
        assert!(matches!(
            normalize_ssdp_header_map(&conflicting),
            Err(SsdpAdvertisementError::ConflictingDuplicate("service-type"))
        ));

        let oversized = "x".repeat(MAX_SSDP_HEADER_BLOCK_BYTES + 1);
        assert!(matches!(
            parse_ssdp_advertisement(&oversized),
            Err(SsdpAdvertisementError::HeaderBlockTooLarge { .. })
        ));

        let control = "Office\nPC".to_string();
        let mut malformed = ssdp_headers(MCNF_SSDP_RDP_SERVICE_TYPE, "rdp", 3389);
        malformed.insert("X-MCNF-NAME".into(), control);
        assert!(matches!(
            normalize_ssdp_header_map(&malformed),
            Err(SsdpAdvertisementError::MalformedControlCharacter)
        ));
    }

    #[test]
    fn ssdp_seam_rejects_urls_paths_commands_and_unsupported_endpoints() {
        let mut location = ssdp_headers(MCNF_SSDP_RDP_SERVICE_TYPE, "rdp", 3389);
        location.insert("LOCATION".into(), "http://192.168.1.60/desktop".into());
        assert!(matches!(
            normalize_ssdp_header_map(&location),
            Err(SsdpAdvertisementError::UrlHeaderForbidden)
        ));

        let mut command = ssdp_headers(MCNF_SSDP_RDP_SERVICE_TYPE, "rdp", 3389);
        command.insert("X-MCNF-NAME".into(), "sh -c id".into());
        assert!(matches!(
            normalize_ssdp_header_map(&command),
            Err(SsdpAdvertisementError::CommandOrPathShapedValue(
                "display name"
            ))
        ));

        let mut path = ssdp_headers(MCNF_SSDP_RDP_SERVICE_TYPE, "rdp", 3389);
        path.insert("X-MCNF-HOST".into(), "/tmp/desktop".into());
        assert!(matches!(
            normalize_ssdp_header_map(&path),
            Err(SsdpAdvertisementError::CommandOrPathShapedValue("host"))
        ));

        for (field, value, expected) in [
            (
                "X-MCNF-HOST",
                "192.168.1.60:3389",
                SsdpAdvertisementError::InvalidHost,
            ),
            ("X-MCNF-PORT", "65536", SsdpAdvertisementError::InvalidPort),
            (
                "X-MCNF-PROTOCOL",
                "telnet",
                SsdpAdvertisementError::UnsupportedProtocol,
            ),
        ] {
            let mut invalid = ssdp_headers(MCNF_SSDP_RDP_SERVICE_TYPE, "rdp", 3389);
            invalid.insert(field.into(), value.into());
            assert!(matches!(normalize_ssdp_header_map(&invalid), Err(error) if error == expected));
        }
        let unsupported = ssdp_headers("urn:mcnf:desktop:telnet:1", "telnet", 23);
        assert!(matches!(
            normalize_ssdp_header_map(&unsupported),
            Err(SsdpAdvertisementError::UnsupportedServiceType)
        ));
    }

    #[test]
    fn ssdp_seam_is_pure_and_does_not_infer_network_state() {
        let headers = ssdp_headers(MCNF_SSDP_SPICE_SERVICE_TYPE, "spice", 5930);
        let before = headers.clone();
        let parsed = normalize_ssdp_header_map(&headers).unwrap();
        assert_eq!(headers, before, "normalization must not mutate caller data");
        assert_eq!(parsed.trust, None);
        assert_eq!(parsed.reachability, None);
        // There is intentionally no socket/probe API in this seam; a caller
        // must supply both results explicitly through SsdpObservation.
    }

    fn trusted_ssdp_advertisement(reachability: Option<Reachability>) -> SsdpDesktopAdvertisement {
        normalize_ssdp_header_map_with_observation(
            &ssdp_headers(MCNF_SSDP_RDP_SERVICE_TYPE, "rdp", 3389),
            SsdpObservation {
                trust: Some(ProvenanceTrust::ObservedLan),
                reachability,
            },
        )
        .expect("valid trusted SSDP advertisement")
    }

    fn ssdp_publication_context() -> SsdpPublicationContext {
        SsdpPublicationContext {
            interface: "enp0s31f6".into(),
            observed_at_ms: 1_786_000_000_000,
            expires_at_ms: 1_786_000_060_000,
            now_ms: 1_786_000_001_000,
        }
    }

    fn ssdp_resource_adapter(max_records: usize) -> SsdpResourceAdapter {
        SsdpResourceAdapter::new(
            SsdpResourceAdapterPolicy::new(vec!["enp0s31f6".into()], max_records)
                .expect("valid SSDP adapter policy"),
        )
    }

    #[test]
    fn ssdp_publication_gate_emits_trusted_lan_provenance() {
        let admitted = admit_ssdp_publication(
            trusted_ssdp_advertisement(Some(Reachability::Reachable)),
            ssdp_publication_context(),
        )
        .expect("trusted SSDP observation should publish");
        assert_eq!(admitted.advertisement.protocol, DesktopProtocol::Rdp);
        assert_eq!(admitted.reachability, Reachability::Reachable);
        assert_eq!(admitted.provenance.source, DiscoverySource::SsdpUpnp);
        assert_eq!(admitted.provenance.scope, ResourceScope::TrustedLan);
        assert_eq!(admitted.provenance.trust, ProvenanceTrust::ObservedLan);
        assert_eq!(admitted.provenance.interface.as_deref(), Some("enp0s31f6"));
        assert!(admitted.provenance.validate().is_ok());
    }

    #[test]
    fn ssdp_publication_gate_preserves_unknown_and_rejects_unreachable() {
        let unknown =
            admit_ssdp_publication(trusted_ssdp_advertisement(None), ssdp_publication_context())
                .expect("unknown reachability remains honest evidence");
        assert_eq!(unknown.reachability, Reachability::Unknown);

        assert_eq!(
            admit_ssdp_publication(
                trusted_ssdp_advertisement(Some(Reachability::Unreachable)),
                ssdp_publication_context(),
            ),
            Err(SsdpPublicationError::Unreachable)
        );
    }

    #[test]
    fn ssdp_publication_gate_rejects_missing_trust_interface_and_malformed_advertisement() {
        let untrusted =
            normalize_ssdp_header_map(&ssdp_headers(MCNF_SSDP_RDP_SERVICE_TYPE, "rdp", 3389))
                .unwrap();
        assert_eq!(
            admit_ssdp_publication(untrusted, ssdp_publication_context()),
            Err(SsdpPublicationError::TrustRequired)
        );

        let mut missing_interface = ssdp_publication_context();
        missing_interface.interface.clear();
        assert_eq!(
            admit_ssdp_publication(
                trusted_ssdp_advertisement(Some(Reachability::Reachable)),
                missing_interface,
            ),
            Err(SsdpPublicationError::InterfaceRequired)
        );

        let mut malformed = trusted_ssdp_advertisement(Some(Reachability::Reachable));
        malformed.host = "/tmp/not-a-host".into();
        assert!(matches!(
            admit_ssdp_publication(malformed, ssdp_publication_context()),
            Err(SsdpPublicationError::MalformedAdvertisement(
                SsdpAdvertisementError::CommandOrPathShapedValue("host")
            ))
        ));
    }

    #[test]
    fn ssdp_publication_gate_rejects_stale_and_unbounded_freshness() {
        let mut expired = ssdp_publication_context();
        expired.now_ms = expired.expires_at_ms;
        assert_eq!(
            admit_ssdp_publication(
                trusted_ssdp_advertisement(Some(Reachability::Reachable)),
                expired,
            ),
            Err(SsdpPublicationError::Expired)
        );

        let mut too_short = ssdp_publication_context();
        too_short.expires_at_ms = too_short.observed_at_ms + MIN_RESOURCE_TTL_MS - 1;
        too_short.now_ms = too_short.observed_at_ms + 1;
        assert_eq!(
            admit_ssdp_publication(
                trusted_ssdp_advertisement(Some(Reachability::Reachable)),
                too_short,
            ),
            Err(SsdpPublicationError::TtlTooShort)
        );

        let mut too_long = ssdp_publication_context();
        too_long.expires_at_ms = too_long.observed_at_ms + MAX_SSDP_PUBLICATION_TTL_MS + 1;
        assert_eq!(
            admit_ssdp_publication(
                trusted_ssdp_advertisement(Some(Reachability::Reachable)),
                too_long,
            ),
            Err(SsdpPublicationError::TtlTooLong)
        );

        let mut future = ssdp_publication_context();
        future.observed_at_ms = future.now_ms + 1;
        assert_eq!(
            admit_ssdp_publication(
                trusted_ssdp_advertisement(Some(Reachability::Reachable)),
                future,
            ),
            Err(SsdpPublicationError::InvalidTimestamp)
        );
    }

    #[test]
    fn ssdp_published_record_revalidates_direct_values_at_use_time() {
        let context = ssdp_publication_context();
        let admitted = admit_ssdp_publication(
            trusted_ssdp_advertisement(Some(Reachability::Reachable)),
            context.clone(),
        )
        .expect("baseline publication");

        let mut wrong_reachability = admitted.clone();
        wrong_reachability.reachability = Reachability::Unreachable;
        assert_eq!(
            wrong_reachability.validate_at(context.now_ms),
            Err(SsdpPublicationError::ReachabilityMismatch)
        );

        let mut expired = admitted.clone();
        expired.provenance.expires_at_ms = context.now_ms;
        assert_eq!(
            expired.validate_at(context.now_ms),
            Err(SsdpPublicationError::Expired)
        );

        let mut too_long = admitted.clone();
        too_long.provenance.expires_at_ms =
            too_long.provenance.observed_at_ms + MAX_SSDP_PUBLICATION_TTL_MS + 1;
        assert_eq!(
            too_long.validate_at(context.now_ms),
            Err(SsdpPublicationError::TtlTooLong)
        );

        let mut wrong_trust = admitted.clone();
        wrong_trust.provenance.trust = ProvenanceTrust::SelfReported;
        assert!(matches!(
            wrong_trust.validate_at(context.now_ms),
            Err(SsdpPublicationError::InvalidProvenance(
                ResourceValidationError::InvalidRelationship("provenance.source_scope_trust")
            ))
        ));

        let mut wrong_identity = admitted;
        wrong_identity.provenance.source_id = "uuid:other".into();
        assert_eq!(
            wrong_identity.validate_at(context.now_ms),
            Err(SsdpPublicationError::InvalidProvenance(
                ResourceValidationError::InvalidRelationship("ssdp.provenance_source_id")
            ))
        );
    }

    #[test]
    fn ssdp_resource_adapter_preserves_trust_interface_reachability_and_exact_ttl() {
        let context = ssdp_publication_context();
        let admitted = admit_ssdp_publication(
            trusted_ssdp_advertisement(Some(Reachability::Reachable)),
            context.clone(),
        )
        .expect("trusted SSDP observation");
        let cards = ssdp_resource_adapter(4)
            .adapt(std::slice::from_ref(&admitted), context.now_ms)
            .expect("trusted SSDP card");
        assert_eq!(cards.len(), 1);
        let card = &cards[0];
        card.validate().expect("adapter emits a valid card");
        assert_eq!(card.identity.authority, IdentityAuthority::Device);
        assert_eq!(card.provenance[0].source, DiscoverySource::SsdpUpnp);
        assert_eq!(card.provenance[0].interface.as_deref(), Some("enp0s31f6"));
        assert_eq!(card.provenance[0].trust, ProvenanceTrust::ObservedLan);
        assert_eq!(card.provenance[0].scope, ResourceScope::TrustedLan);
        assert_eq!(card.health.status, HealthStatus::Available);
        assert_eq!(card.last_seen_at_ms, context.observed_at_ms);
        assert_eq!(card.expires_at_ms, context.expires_at_ms);
        assert_eq!(card.transports[0].last_seen_at_ms, context.observed_at_ms);
        assert_eq!(card.transports[0].expires_at_ms, context.expires_at_ms);
        assert_eq!(
            card.actions
                .iter()
                .find(|action| action.verb == ResourceActionVerb::Connect)
                .expect("connect action")
                .availability
                .status,
            ActionAvailabilityStatus::RequiresApproval
        );
        assert!(card
            .actions
            .iter()
            .all(|action| action.verb != ResourceActionVerb::Launch));
    }

    #[test]
    fn ssdp_resource_adapter_keeps_unknown_reachability_as_unready_evidence() {
        let context = ssdp_publication_context();
        let admitted = admit_ssdp_publication(trusted_ssdp_advertisement(None), context.clone())
            .expect("unknown reachability is valid evidence");
        let cards = ssdp_resource_adapter(4)
            .adapt(std::slice::from_ref(&admitted), context.now_ms)
            .expect("unknown SSDP card");
        assert_eq!(cards[0].health.status, HealthStatus::Unknown);
        assert_eq!(
            cards[0]
                .actions
                .iter()
                .find(|action| action.verb == ResourceActionVerb::Connect)
                .expect("connect evidence")
                .availability
                .status,
            ActionAvailabilityStatus::Unavailable
        );
    }

    #[test]
    fn ssdp_resource_adapter_enforces_interface_scope_and_use_time_expiry() {
        let mut context = ssdp_publication_context();
        context.interface = "wlan0".into();
        let admitted = admit_ssdp_publication(
            trusted_ssdp_advertisement(Some(Reachability::Reachable)),
            context.clone(),
        )
        .expect("wlan0 publication");
        assert!(matches!(
            ssdp_resource_adapter(4).adapt(std::slice::from_ref(&admitted), context.now_ms),
            Err(SsdpResourceAdapterError::InterfaceNotAllowed { .. })
        ));

        let mut expired = admitted;
        expired.provenance.expires_at_ms = context.now_ms;
        assert!(matches!(
            ssdp_resource_adapter(4).adapt(std::slice::from_ref(&expired), context.now_ms),
            Err(SsdpResourceAdapterError::PublicationRejected {
                error: SsdpPublicationError::Expired,
                ..
            })
        ));
    }

    #[test]
    fn ssdp_resource_adapter_folds_exact_duplicates_and_closed_protocols_boundedly() {
        let context = ssdp_publication_context();
        let rdp = admit_ssdp_publication(
            trusted_ssdp_advertisement(Some(Reachability::Reachable)),
            context.clone(),
        )
        .expect("RDP publication");
        let vnc_advertisement = normalize_ssdp_header_map_with_observation(
            &ssdp_headers(MCNF_SSDP_VNC_SERVICE_TYPE, "vnc", 5900),
            SsdpObservation {
                trust: Some(ProvenanceTrust::ObservedLan),
                reachability: Some(Reachability::Reachable),
            },
        )
        .expect("VNC advertisement");
        let vnc =
            admit_ssdp_publication(vnc_advertisement, context.clone()).expect("VNC publication");
        let cards = ssdp_resource_adapter(4)
            .adapt(&[rdp.clone(), rdp.clone(), vnc.clone()], context.now_ms)
            .expect("bounded multi-protocol snapshot");
        assert_eq!(cards.len(), 1, "one device identity");
        assert_eq!(cards[0].transports.len(), 2, "RDP and VNC transports");
        assert_eq!(
            cards[0]
                .transports
                .iter()
                .map(|transport| transport.protocol)
                .collect::<Vec<_>>(),
            vec![TransportProtocol::Rdp, TransportProtocol::Vnc]
        );
        assert_eq!(
            cards[0]
                .actions
                .iter()
                .filter(|action| action.verb == ResourceActionVerb::Connect)
                .count(),
            2
        );

        assert!(matches!(
            ssdp_resource_adapter(2)
                .adapt(&[rdp.clone(), rdp.clone(), vnc.clone()], context.now_ms),
            Err(SsdpResourceAdapterError::TooManyRecords { count: 3, max: 2 })
        ));

        // A different port is valid across protocols (the RDP and VNC
        // records above intentionally use their own listener ports).  The
        // conflict boundary is same-protocol identity: duplicate VNC/RDP
        // advertisements for one UUID may not disagree on their endpoint.
        let mut conflicting = rdp.clone();
        conflicting.advertisement.port = 5901;
        assert!(matches!(
            ssdp_resource_adapter(4).adapt(&[rdp, conflicting], context.now_ms),
            Err(SsdpResourceAdapterError::ConflictingIdentity { .. })
        ));
    }

    // ── lane 3: the local-VM fold + honest gate ──

    #[test]
    fn source_from_vm_derives_reachability_from_power_state() {
        let node = "elm";
        let running = source_from_vm(
            node,
            &Instance {
                id: "3".into(),
                name: "dev".into(),
                state: "running".into(),
            },
        );
        assert_eq!(running.id, "vm:elm:dev");
        assert_eq!(running.reachability, Reachability::Reachable);
        assert_eq!(running.power_state.as_deref(), Some("running"));
        assert_eq!(
            running.protocols,
            vec![ProtocolOffer::new(DesktopProtocol::Spice, None)]
        );
        assert_eq!(running.origin, SourceOrigin::LocalVm);

        let off = source_from_vm(
            node,
            &Instance {
                id: "-".into(),
                name: "win".into(),
                state: "shut off".into(),
            },
        );
        assert_eq!(off.reachability, Reachability::Unreachable);
        assert_eq!(off.reason.as_deref(), Some("vm shut off"));
        assert_eq!(off.power_state.as_deref(), Some("shut off"));

        // A paused console still answers (the qemu process is live).
        let paused = source_from_vm(
            node,
            &Instance {
                id: "4".into(),
                name: "p".into(),
                state: "paused".into(),
            },
        );
        assert_eq!(paused.reachability, Reachability::Reachable);
    }

    #[test]
    fn gated_vm_lane_contributes_no_sources_and_an_honest_status() {
        let mut w = worker_at(
            tempfile::tempdir().unwrap().path(),
            tempfile::tempdir().unwrap().path(),
        );
        let list = w.fold_vm_result(Err(VmEnumerateError::Gated("virsh not found".into())));
        assert!(list.is_empty(), "a gate NEVER fabricates a source");
        assert_eq!(w.vm_lane, "gated: virsh not found");
        let lanes = w.lanes();
        let kvm = lanes.iter().find(|l| l.lane == "local-kvm").unwrap();
        assert!(kvm.status.starts_with("gated:"));

        // A backend error is likewise honest — surfaced, no sources.
        let list = w.fold_vm_result(Err(VmEnumerateError::Backend("libvirtd down".into())));
        assert!(list.is_empty());
        assert_eq!(w.vm_lane, "error: libvirtd down");

        // And a real roster flips the lane back to ok.
        let list = w.fold_vm_result(Ok(vec![Instance {
            id: "1".into(),
            name: "dev".into(),
            state: "running".into(),
        }]));
        assert_eq!(list.len(), 1);
        assert_eq!(w.vm_lane, "ok (1 vms)");
    }

    // ── lane 4: verbs + the manual store ──

    #[test]
    fn add_source_parses_and_validates() {
        let req = parse_add_source(
            r#"{"name":"lab box","host":"192.168.1.50","port":3389,"protocol":"rdp"}"#,
        )
        .unwrap();
        assert_eq!(req.host, "192.168.1.50");
        assert_eq!(req.protocol, DesktopProtocol::Rdp);
        assert_eq!(req.id(), "manual:192.168.1.50:3389:rdp");
        assert_eq!(req.display_name(), "lab box");
        // Name defaults to host:port.
        let unnamed = parse_add_source(r#"{"host":"h","port":5900,"protocol":"vnc"}"#).unwrap();
        assert_eq!(unnamed.display_name(), "h:5900");
        // Rejections are typed + human-readable.
        assert!(parse_add_source("nope").is_err());
        assert!(parse_add_source(r#"{"host":"","port":1,"protocol":"vnc"}"#).is_err());
        assert!(parse_add_source(r#"{"host":"h","port":0,"protocol":"vnc"}"#).is_err());
        assert!(parse_add_source(r#"{"host":"h","port":1,"protocol":"telnet"}"#).is_err());
    }

    #[test]
    fn remove_source_parses_and_validates() {
        let req = parse_remove_source(r#"{"id":"manual:h:5900:vnc"}"#).unwrap();
        assert_eq!(req.id, "manual:h:5900:vnc");
        assert!(parse_remove_source(r#"{"id":""}"#).is_err());
        assert!(parse_remove_source("nope").is_err());
    }

    #[test]
    fn manual_store_round_trips_and_tolerates_absence() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_manual_sources(dir.path()).is_empty());
        let sources = vec![ManualSource {
            name: None,
            host: "h".into(),
            port: 5900,
            protocol: DesktopProtocol::Vnc,
        }];
        save_manual_sources(dir.path(), &sources).unwrap();
        assert_eq!(load_manual_sources(dir.path()), sources);
        // Corrupt store → empty, never fatal.
        std::fs::write(dir.path().join(MANUAL_STORE_FILE), "{ not json").unwrap();
        assert!(load_manual_sources(dir.path()).is_empty());
    }

    #[test]
    fn manual_store_rejects_oversized_invalid_utf8_and_special_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(MANUAL_STORE_FILE);

        std::fs::write(&path, vec![b'x'; MAX_MANUAL_STORE_BYTES + 1]).unwrap();
        assert!(
            read_bounded_manual_store(&path).is_err(),
            "oversized manual stores must be rejected before JSON parsing"
        );

        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        assert_eq!(
            load_manual_sources(dir.path()),
            Vec::<ManualSource>::new(),
            "invalid UTF-8 must fail soft to an empty manual roster"
        );

        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(
            read_bounded_manual_store(&path).is_err(),
            "directories must not be consumed as manual stores"
        );

        #[cfg(unix)]
        {
            std::fs::remove_dir(&path).unwrap();
            let target = dir.path().join("target.json");
            std::fs::write(&target, "[]").unwrap();
            std::os::unix::fs::symlink(&target, &path).unwrap();
            assert!(
                read_bounded_manual_store(&path).is_err(),
                "final symlinks must not be followed"
            );
        }
    }

    // ── the merge fold ──

    fn ad_seat(node: &str, host: &str) -> AdvertisedDesktop {
        AdvertisedDesktop {
            node: node.into(),
            host: host.into(),
            vm: None,
            protocols: vec![ProtocolOffer::new(DesktopProtocol::Rdp, Some(3389))],
            power_state: None,
            reachability: Reachability::Reachable,
            reason: None,
        }
    }

    fn ep(instance: &str, host: &str, port: u16, protocol: DesktopProtocol) -> MdnsEndpoint {
        MdnsEndpoint {
            fullname: format!("{instance}._x._tcp.local."),
            instance: instance.into(),
            host: host.into(),
            port,
            protocol,
        }
    }

    #[test]
    fn merge_folds_a_known_peers_mdns_protocol_into_its_card() {
        // oak's VNC shows up on the LAN via mDNS at oak's address → the offer
        // folds into oak's card instead of a duplicate.
        let merged = merge_sources(
            &[ad_seat("oak", "10.42.0.7")],
            &[ep("oak", "10.42.0.7", 5901, DesktopProtocol::Vnc)],
            &[],
            &[],
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, "peer:oak");
        assert_eq!(
            merged[0].protocols,
            vec![
                ProtocolOffer::new(DesktopProtocol::Rdp, Some(3389)),
                ProtocolOffer::new(DesktopProtocol::Vnc, Some(5901)),
            ]
        );
        // A protocol the card already offers isn't duplicated.
        let merged = merge_sources(
            &[ad_seat("oak", "10.42.0.7")],
            &[ep("OAK", "192.168.1.9", 3390, DesktopProtocol::Rdp)],
            &[],
            &[],
        );
        assert_eq!(merged.len(), 1, "instance-name match (case-insensitive)");
        assert_eq!(merged[0].protocols.len(), 1);
    }

    #[test]
    fn merge_keeps_an_unknown_lan_endpoint_as_its_own_card() {
        let merged = merge_sources(
            &[ad_seat("oak", "10.42.0.7")],
            &[ep("OfficePC", "192.168.1.60", 3389, DesktopProtocol::Rdp)],
            &[],
            &[],
        );
        assert_eq!(merged.len(), 2);
        let lan = merged
            .iter()
            .find(|s| s.origin == SourceOrigin::Mdns)
            .unwrap();
        assert_eq!(lan.id, "mdns:192.168.1.60:3389:rdp");
        assert_eq!(lan.name, "OfficePC");
        assert_eq!(lan.reachability, Reachability::Reachable);
    }

    #[test]
    fn merge_dedups_a_manual_duplicate_and_keeps_a_new_one() {
        let dup = ManualSource {
            name: None,
            host: "10.42.0.7".into(),
            port: 3389,
            protocol: DesktopProtocol::Rdp,
        };
        let fresh = ManualSource {
            name: Some("spare".into()),
            host: "192.168.1.99".into(),
            port: 5900,
            protocol: DesktopProtocol::Vnc,
        };
        let merged = merge_sources(&[ad_seat("oak", "10.42.0.7")], &[], &[], &[dup, fresh]);
        assert_eq!(merged.len(), 2, "the duplicate folded away");
        let manual = merged
            .iter()
            .find(|s| s.origin == SourceOrigin::Manual)
            .unwrap();
        assert_eq!(manual.name, "spare");
        assert_eq!(manual.reachability, Reachability::Unknown, "never probed");
        // Same host, DIFFERENT port → a genuinely distinct endpoint, kept.
        let alt_port = ManualSource {
            name: None,
            host: "10.42.0.7".into(),
            port: 3390,
            protocol: DesktopProtocol::Rdp,
        };
        let merged = merge_sources(&[ad_seat("oak", "10.42.0.7")], &[], &[], &[alt_port]);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn merge_output_is_stably_ordered_by_node_then_name() {
        let vms = vec![
            source_from_vm(
                "elm",
                &Instance {
                    id: "1".into(),
                    name: "zeta".into(),
                    state: "running".into(),
                },
            ),
            source_from_vm(
                "elm",
                &Instance {
                    id: "2".into(),
                    name: "alpha".into(),
                    state: "running".into(),
                },
            ),
        ];
        let merged = merge_sources(
            &[ad_seat("oak", "10.42.0.7"), ad_seat("ash", "10.42.0.8")],
            &[],
            &vms,
            &[],
        );
        let order: Vec<&str> = merged.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            order,
            vec!["peer:ash", "vm:elm:alpha", "vm:elm:zeta", "peer:oak"]
        );
    }

    // ── universal ResourceCard adapter ──

    const RESOURCE_NOW: u64 = 1_700_000_000_000;

    #[test]
    fn resource_card_adapter_preserves_mesh_mdns_local_and_manual_lanes() {
        let mesh = source_from_advertised(&ad_seat("oak", "10.42.0.7"));
        let mdns = source_from_mdns(&ep("OfficePC", "192.168.1.60", 3389, DesktopProtocol::Rdp));
        let local = source_from_vm(
            "elm",
            &Instance {
                id: "1".into(),
                name: "dev".into(),
                state: "running".into(),
            },
        );
        let manual = source_from_manual(&ManualSource {
            name: Some("spare".into()),
            host: "192.168.1.99".into(),
            port: 5900,
            protocol: DesktopProtocol::Vnc,
        });

        let cases = [
            (
                mesh,
                DiscoverySource::MeshDirectory,
                TransportProtocol::Rdp,
                ActionAvailabilityStatus::Ready,
            ),
            (
                mdns,
                DiscoverySource::MdnsDnsSd,
                TransportProtocol::Rdp,
                ActionAvailabilityStatus::RequiresApproval,
            ),
            (
                local,
                DiscoverySource::Local,
                TransportProtocol::Spice,
                ActionAvailabilityStatus::Ready,
            ),
            (
                manual,
                DiscoverySource::Manual,
                TransportProtocol::Vnc,
                ActionAvailabilityStatus::Unavailable,
            ),
        ];
        for (source, discovery_source, protocol, connect_status) in cases {
            let card =
                resource_card_from_desktop_source(&source, RESOURCE_NOW).expect("valid card");
            card.validate().expect("adapter validates its card");
            assert_eq!(card.identity.class, ResourceClass::Desktop);
            assert_eq!(card.provenance[0].source, discovery_source);
            assert_eq!(card.transports[0].protocol, protocol);
            let connect = card
                .actions
                .iter()
                .find(|action| action.verb == ResourceActionVerb::Connect)
                .expect("typed transport has a connect action");
            assert_eq!(connect.availability.status, connect_status);
        }
    }

    #[test]
    fn resource_card_adapter_deduplicates_offers_and_keeps_stable_ids() {
        let mut source = source_from_advertised(&ad_seat("oak", "10.42.0.7"));
        source.protocols.push(source.protocols[0]);
        let first = resource_card_from_desktop_source(&source, RESOURCE_NOW).expect("first card");
        let second =
            resource_card_from_desktop_source(&source, RESOURCE_NOW).expect("same card again");
        assert_eq!(first.resource_id(), second.resource_id());
        assert_eq!(first, second);
        assert_eq!(first.transports.len(), 1, "duplicate offers are folded");
        assert_eq!(first.client_capabilities.len(), 1);
        assert_eq!(
            first
                .actions
                .iter()
                .filter(|action| action.verb == ResourceActionVerb::Connect)
                .count(),
            1
        );

        let merged = merge_sources(
            &[ad_seat("oak", "10.42.0.7")],
            &[],
            &[],
            &[ManualSource {
                name: None,
                host: "10.42.0.7".into(),
                port: 3389,
                protocol: DesktopProtocol::Rdp,
            }],
        );
        assert_eq!(merged.len(), 1, "roster dedupe remains the source of truth");
        let roster_card =
            resource_card_from_desktop_source(&merged[0], RESOURCE_NOW).expect("roster card");
        assert_eq!(roster_card.resource_id(), first.resource_id());
    }

    #[test]
    fn resource_card_adapter_keeps_unavailable_sources_unready() {
        let source = source_from_vm(
            "elm",
            &Instance {
                id: "2".into(),
                name: "offline".into(),
                state: "shut off".into(),
            },
        );
        let card = resource_card_from_desktop_source(&source, RESOURCE_NOW).expect("offline card");
        assert_eq!(card.health.status, HealthStatus::Unavailable);
        assert_eq!(
            card.health
                .failure
                .as_ref()
                .map(|failure| failure.message.as_str()),
            Some("vm shut off")
        );
        let connect = card
            .actions
            .iter()
            .find(|action| action.verb == ResourceActionVerb::Connect)
            .expect("offline source still explains connect");
        assert_eq!(
            connect.availability.status,
            ActionAvailabilityStatus::Unavailable
        );
        assert!(card.actions.iter().all(|action| {
            action.verb != ResourceActionVerb::Launch
                && !(action.verb == ResourceActionVerb::Connect
                    && action.availability.status == ActionAvailabilityStatus::Ready)
        }));
        card.validate()
            .expect("offline card remains valid evidence");
    }

    #[test]
    fn resource_card_adapter_rejects_hostile_endpoint_and_action_shapes() {
        let hostile = source_from_manual(&ManualSource {
            name: None,
            host: "https://evil.example/$(id)".into(),
            port: 3389,
            protocol: DesktopProtocol::Rdp,
        });
        assert_eq!(
            resource_card_from_desktop_source(&hostile, RESOURCE_NOW),
            Err(ResourceValidationError::InvalidField("desktop_source.host"))
        );

        let source = source_from_advertised(&ad_seat("oak", "10.42.0.7"));
        let card = resource_card_from_desktop_source(&source, RESOURCE_NOW).expect("safe card");
        let encoded = serde_json::to_string(&card).expect("card JSON");
        assert!(!encoded.contains("command"));
        assert!(!encoded.contains("://"));
        assert!(card
            .actions
            .iter()
            .all(|action| action.verb != ResourceActionVerb::Launch));

        let mut hostile_action = card.clone();
        let connect = hostile_action
            .actions
            .iter_mut()
            .find(|action| action.verb == ResourceActionVerb::Connect)
            .expect("connect action");
        connect.target = ResourceActionTarget::Resource;
        assert_eq!(
            hostile_action.validate(),
            Err(ResourceValidationError::InvalidRelationship(
                "action.verb_target"
            ))
        );

        let mut hostile_endpoint = card;
        if let TransportEndpoint::Network { host, .. } =
            &mut hostile_endpoint.transports[0].endpoint
        {
            *host = "https://evil.example".into();
        }
        assert_eq!(
            hostile_endpoint.validate(),
            Err(ResourceValidationError::InvalidField("endpoint.host"))
        );
    }

    // ── the published record ──

    #[test]
    fn published_state_carries_an_honestly_empty_thumbnail_field() {
        let state = DesktopSourcesState {
            node: "elm".into(),
            sources: vec![source_from_manual(&ManualSource {
                name: None,
                host: "h".into(),
                port: 5900,
                protocol: DesktopProtocol::Vnc,
            })],
            lanes: vec![],
            published_at_ms: 1,
        };
        let body = serde_json::to_string(&state).unwrap();
        // The CHOOSER-3 key ships now, honestly null — never a fake ref.
        assert!(body.contains("\"thumbnail_ref\":null"));
        let back: DesktopSourcesState = serde_json::from_str(&body).unwrap();
        assert_eq!(back, state);
    }

    // ── worker orchestration over fake seams (no libvirt, no LAN) ──

    struct FakeVms(Result<Vec<Instance>, VmEnumerateError>);
    impl VmEnumerator for FakeVms {
        fn enumerate(&self) -> Result<Vec<Instance>, VmEnumerateError> {
            self.0.clone()
        }
    }

    fn worker_at(workgroup: &Path, store: &Path) -> DesktopSourcesWorker {
        DesktopSourcesWorker::new(
            "elm".to_string(),
            workgroup.to_path_buf(),
            store.to_path_buf(),
        )
        .with_authorizer(Arc::new(ActionAuthorizer::for_test(
            AUTH_KEY,
            store.join(".auth"),
            AUTH_NOW,
        )))
        .with_enumerator(Arc::new(FakeVms(Ok(vec![]))))
    }

    fn authorized_body(unsigned: &str, verb: &str, target: &str, nonce: &str) -> String {
        authorize_test_body(
            AUTH_KEY,
            unsigned,
            MutationContext {
                verb,
                node: "elm",
                target,
            },
            nonce,
            AUTH_NOW + 30_000,
        )
    }

    fn authorized_add_body(unsigned: &str, nonce: &str) -> String {
        let target = parse_add_source(unsigned).unwrap().id();
        authorized_body(unsigned, DESKTOP_ADD_SOURCE_AUTH_VERB, &target, nonce)
    }

    fn authorized_remove_body(unsigned: &str, nonce: &str) -> String {
        let target = parse_remove_source(unsigned).unwrap().id;
        authorized_body(unsigned, DESKTOP_REMOVE_SOURCE_AUTH_VERB, &target, nonce)
    }

    fn temp_persist() -> (tempfile::TempDir, Persist) {
        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().to_path_buf()).unwrap();
        (dir, persist)
    }

    fn latest_state(persist: &Persist) -> DesktopSourcesState {
        let msgs = persist.list_since(SOURCES_TOPIC, None).unwrap();
        let body = msgs.last().unwrap().body.clone().unwrap();
        serde_json::from_str(&body).unwrap()
    }

    #[test]
    fn add_source_verb_adds_persists_and_publishes() {
        let (_bus, persist) = temp_persist();
        let wg = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let mut w = worker_at(wg.path(), store.path());
        persist
            .write(
                ADD_SOURCE_TOPIC,
                Priority::Default,
                None,
                Some(&authorized_add_body(
                    r#"{"host":"192.168.1.50","port":3389,"protocol":"rdp","schema_version":1}"#,
                    "add-one",
                )),
            )
            .unwrap();
        let (changed, refresh) = w.drain_actions(&persist);
        assert!(changed);
        assert!(!refresh);
        assert_eq!(w.manual.len(), 1);
        // Durable: a fresh load sees it.
        assert_eq!(load_manual_sources(store.path()), w.manual);
        // The published roster carries it.
        let sources = w.collect_sources(&[]);
        assert!(w.publish(&persist, sources, false));
        let state = latest_state(&persist);
        assert_eq!(state.node, "elm");
        assert_eq!(state.sources.len(), 1);
        assert_eq!(state.sources[0].id, "manual:192.168.1.50:3389:rdp");
        // Re-adding the same endpoint is idempotent.
        persist
            .write(
                ADD_SOURCE_TOPIC,
                Priority::Default,
                None,
                Some(&authorized_add_body(
                    r#"{"host":"192.168.1.50","port":3389,"protocol":"rdp","schema_version":1}"#,
                    "add-two",
                )),
            )
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(!changed);
        assert_eq!(w.manual.len(), 1);
    }

    #[test]
    fn remove_source_verb_removes_and_persists() {
        let (_bus, persist) = temp_persist();
        let wg = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let mut w = worker_at(wg.path(), store.path());
        persist
            .write(
                ADD_SOURCE_TOPIC,
                Priority::Default,
                None,
                Some(&authorized_add_body(
                    r#"{"host":"h","port":5900,"protocol":"vnc","schema_version":1}"#,
                    "add-three",
                )),
            )
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(changed);
        persist
            .write(
                REMOVE_SOURCE_TOPIC,
                Priority::Default,
                None,
                Some(&authorized_remove_body(
                    r#"{"id":"manual:h:5900:vnc","schema_version":1}"#,
                    "remove-one",
                )),
            )
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(changed);
        assert!(w.manual.is_empty());
        assert!(load_manual_sources(store.path()).is_empty());
        // Removing a non-manual id is a logged no-op, never a panic.
        persist
            .write(
                REMOVE_SOURCE_TOPIC,
                Priority::Default,
                None,
                Some(&authorized_remove_body(
                    r#"{"id":"peer:oak","schema_version":1}"#,
                    "remove-two",
                )),
            )
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(!changed);
    }

    #[test]
    fn refresh_verb_nudges_and_publish_gates_on_change() {
        let (_bus, persist) = temp_persist();
        let wg = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let mut w = worker_at(wg.path(), store.path());
        persist
            .write(REFRESH_TOPIC, Priority::Default, None, Some(""))
            .unwrap();
        let (changed, refresh) = w.drain_actions(&persist);
        assert!(!changed);
        assert!(refresh);
        // publish-on-change: the first publish writes, an identical fold
        // doesn't, a forced (refresh/heartbeat) one does.
        let sources = w.collect_sources(&[]);
        assert!(w.publish(&persist, sources.clone(), false));
        assert!(!w.publish(&persist, sources.clone(), false));
        assert!(w.publish(&persist, sources, true));
    }

    #[test]
    fn initial_phase_is_stable_bounded_and_keeps_first_scan_deadline() {
        let phase = initial_phase_for("peer:seat15", DEFAULT_TICK_INTERVAL);
        assert_eq!(
            phase,
            initial_phase_for("peer:seat15", DEFAULT_TICK_INTERVAL)
        );
        assert!(phase <= MAX_INITIAL_PHASE);
        assert!(phase <= DEFAULT_TICK_INTERVAL);
        let first_delay = DEFAULT_TICK_INTERVAL.saturating_sub(phase);
        assert!(first_delay <= DEFAULT_TICK_INTERVAL);
        assert!(first_delay >= DEFAULT_TICK_INTERVAL - MAX_INITIAL_PHASE);
        assert_eq!(initial_phase_for("", DEFAULT_TICK_INTERVAL), Duration::ZERO);
        assert!(
            initial_phase_for("peer:seat15", Duration::from_millis(100))
                <= Duration::from_millis(100)
        );
    }

    #[test]
    fn add_source_requires_exact_single_use_capability_before_store_write() {
        let (_bus, persist) = temp_persist();
        let wg = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let mut w = worker_at(wg.path(), store.path());
        let unsigned = r#"{"host":"10.0.0.4","port":3389,"protocol":"rdp","schema_version":1}"#;

        persist
            .write(ADD_SOURCE_TOPIC, Priority::Default, None, Some(unsigned))
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(!changed, "unsigned add must not touch the manual store");
        assert!(w.manual.is_empty());
        assert!(load_manual_sources(store.path()).is_empty());

        let armed = authorized_add_body(unsigned, "add-hostile");
        let tampered = armed.replace("3389", "3390");
        persist
            .write(ADD_SOURCE_TOPIC, Priority::Default, None, Some(&tampered))
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(!changed, "tampered add must be refused before persistence");
        assert!(w.manual.is_empty());

        persist
            .write(ADD_SOURCE_TOPIC, Priority::Default, None, Some(&armed))
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(changed);
        assert_eq!(w.manual.len(), 1);

        persist
            .write(ADD_SOURCE_TOPIC, Priority::Default, None, Some(&armed))
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(!changed, "replaying an add capability must be refused");
        assert_eq!(w.manual.len(), 1);
    }

    #[test]
    fn remove_source_requires_exact_single_use_capability_before_store_write() {
        let (_bus, persist) = temp_persist();
        let wg = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let mut w = worker_at(wg.path(), store.path());
        let add = ManualSource {
            name: None,
            host: "10.0.0.5".into(),
            port: 5900,
            protocol: DesktopProtocol::Vnc,
        };
        assert!(w.handle_add(add.clone()));
        let unsigned = format!(r#"{{"id":"{}","schema_version":1}}"#, add.id());

        persist
            .write(
                REMOVE_SOURCE_TOPIC,
                Priority::Default,
                None,
                Some(&unsigned),
            )
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(!changed, "unsigned remove must not touch the manual store");
        assert_eq!(w.manual, vec![add.clone()]);

        let armed = authorized_remove_body(&unsigned, "remove-hostile");
        let tampered = armed.replace("10.0.0.5", "10.0.0.6");
        persist
            .write(
                REMOVE_SOURCE_TOPIC,
                Priority::Default,
                None,
                Some(&tampered),
            )
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(
            !changed,
            "tampered remove must be refused before persistence"
        );
        assert_eq!(w.manual, vec![add.clone()]);

        persist
            .write(REMOVE_SOURCE_TOPIC, Priority::Default, None, Some(&armed))
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(changed);
        assert!(w.manual.is_empty());

        persist
            .write(REMOVE_SOURCE_TOPIC, Priority::Default, None, Some(&armed))
            .unwrap();
        let (changed, _) = w.drain_actions(&persist);
        assert!(!changed, "replaying a remove capability must be refused");
        assert!(w.manual.is_empty());
    }

    #[test]
    fn collect_sources_folds_the_peers_plane_and_local_vms() {
        let (_bus, persist) = temp_persist();
        let wg = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        // A peer advertising an RDP seat, plus our own record (skipped).
        let pdir = peers_dir(wg.path());
        mackes_mesh_types::peers::write_peer_record(
            &pdir,
            &peer("oak", "healthy", Some("10.42.0.7"), true, false, vec![]),
        )
        .unwrap();
        mackes_mesh_types::peers::write_peer_record(
            &pdir,
            &peer("elm", "healthy", Some("10.42.0.2"), true, true, vec![]),
        )
        .unwrap();
        let mut w = worker_at(wg.path(), store.path());
        let vms = vec![Instance {
            id: "1".into(),
            name: "dev".into(),
            state: "running".into(),
        }];
        let sources = w.collect_sources(&vms);
        let ids: Vec<&str> = sources.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["vm:elm:dev", "peer:oak"]);
        assert!(w.publish(&persist, sources, false));
        let state = latest_state(&persist);
        assert_eq!(state.sources.len(), 2);
        assert_eq!(state.lanes.len(), 4);
    }
}
