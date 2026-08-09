//! WL-FUNC-019 — the mackesd `service_aggregator` worker: the unified
//! service-provenance/health view.
//!
//! Three service sources existed and were never unified — the published KDC
//! directory (`kdc-services/<host>.json`), the nmap probe inventory
//! (`probe-inventory.json`), and the Explorer's `service → openable-action`
//! enrichment map. This worker merges all three into one deduped
//! [`ServiceRecord`](mackes_mesh_types::service_record::ServiceRecord) set (with
//! stale-entry TTL age-out) and publishes it on `state/services/<node>`, so the
//! shell's Services view stays a thin renderer (§6 — scanning + privilege live in
//! the daemon, never the GUI).
//!
//! ## Shape (mirrors the EXPLORER-1 `unit_aggregator` / QC-2 `openstack` workers)
//!
//! - **Two injectable source seams** ([`PublishedSource`] / [`ProbeSource`]), each
//!   headless-testable with a fake: [`DirectoryPublished`] reads the replicated KDC
//!   directory; [`InventoryProbe`] reads the merged probe inventory. Enrichment is
//!   the pure `service → action` map applied inside [`aggregate::aggregate`].
//! - **A pure fold** ([`aggregate::aggregate`]): merge by `(host, kind)`, stamp
//!   health from source + freshness, age out stale probe-only entries.
//! - **The `state/services/<node>` mirror** ([`ServicesState`]) — published on
//!   change + a heartbeat via the in-process `mde-bus` publish path (the same idiom
//!   `state/units/<node>` / `state/storage/<node>` use).
//!
//! Universal (rank 0) like `unit_aggregator` / `storage` / `openstack`: every node
//! folds + publishes its OWN mesh-wide merge of the (replicated) sources, no center.

#![cfg(feature = "async-services")]

pub mod aggregate;
pub mod resource_actions;
pub mod resource_adapters;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mackes_mesh_types::resources::{
    resource_publisher_attestation_topic, ClientCapabilityRegistry, ResourceCatalog,
    ResourcePublisherAttestation, RESOURCE_CATALOG_TOPIC, RESOURCE_DISCOVERY_TOPIC,
    RESOURCE_PUBLISHER_ATTESTATION_KEY_ID, RESOURCE_PUBLISHER_ATTESTATION_TTL_MS,
};
use mackes_mesh_types::service_record::ServicesState;
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use aggregate::{aggregate, ProbeInput, PublishedInput};

use super::desktop_sources::{
    DesktopProtocol, DesktopSource, DesktopSourcesState, LaneStatus, ProtocolOffer, Reachability,
    SourceOrigin, SOURCES_TOPIC,
};
use super::ssh_x11_sources::{decode_sources_state, SshX11SourcesState, SSH_X11_SOURCES_TOPIC};
use super::upnp_sources::{
    decode_sources_state as decode_upnp_sources_state, UpnpSourcesState, UPNP_SOURCES_TOPIC,
};
use super::{ShutdownToken, Worker};
use crate::ipc::secret_store::{repo_root, SecretStore};

const RESOURCE_PUBLISHER_KEY_REF: &str = "resource/publisher-hmac";

/// A missing/unreachable publisher-key backend must not turn every catalog
/// fold into a process-spawning retry storm. Keep the first retry reasonably
/// fresh, then widen the interval while the failure persists; a successful
/// lookup resets the delay immediately.
const PUBLISHER_RETRY_BASE: Duration = Duration::from_secs(30);
const PUBLISHER_RETRY_MAX: Duration = Duration::from_secs(300);

/// Startup retry lower bound for an unresolved, unopenable, or unsafe Bus.
const MIN_BUS_RETRY_INTERVAL: Duration = Duration::from_millis(10);

/// Keep recovery responsive without allowing an absent spool to spin.
const MAX_BUS_RETRY_INTERVAL: Duration = Duration::from_secs(2);

#[cfg(test)]
type BusReadGateFn = dyn Fn(&str, usize) -> Result<(), String> + Send + Sync;

#[cfg(test)]
type BusWriteGateFn = dyn Fn(&str, usize) -> Result<(), String> + Send + Sync;

#[derive(Debug, Default)]
struct PublisherRetryState {
    next_attempt: Option<Instant>,
    backoff: Duration,
}

impl PublisherRetryState {
    fn allowed(&self, now: Instant) -> bool {
        self.next_attempt.is_none_or(|next| now >= next)
    }

    fn record_failure(&mut self, now: Instant) {
        let delay = if self.backoff.is_zero() {
            PUBLISHER_RETRY_BASE
        } else {
            self.backoff
                .checked_mul(2)
                .unwrap_or(PUBLISHER_RETRY_MAX)
                .min(PUBLISHER_RETRY_MAX)
        };
        self.backoff = delay;
        self.next_attempt = Some(now + delay);
    }

    fn record_success(&mut self) {
        self.next_attempt = None;
        self.backoff = Duration::ZERO;
    }
}

/// Fold cadence — one directory + inventory read per interval. Same order of cost
/// as the sibling `unit_aggregator` worker's heartbeat.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(15);

/// Bound the first full directory/inventory fold so seats do not all perform
/// the expensive source reads and catalog projection at daemon start.
pub const MAX_INITIAL_PHASE: Duration = Duration::from_millis(1_500);

/// Unconditional mirror republish cadence (between change-driven publishes).
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(60);

/// Default stale-entry age-out window: a probe-only service not re-seen within it
/// expires from the set (see [`aggregate::aggregate`]).
pub const DEFAULT_TTL: Duration = Duration::from_secs(300);

/// Bound the retained desktop roster before serde allocates nested source rows.
/// This is intentionally aligned with the resource contract's 2 MiB body
/// ceiling while leaving the catalog validator authoritative for card capacity.
const MAX_RETAINED_DESKTOP_STATE_BYTES: usize = 2 * 1024 * 1024;
const MAX_RETAINED_DESKTOP_SOURCES: usize = 4_096;
const MAX_RETAINED_DESKTOP_LANES: usize = 32;
const MAX_RETAINED_DESKTOP_PROTOCOLS: usize = 8;

/// Derive a stable, bounded startup phase from the node identity. The phase is
/// intentionally independent of wall-clock time so a restart does not make a
/// fleet converge back onto one common first-fold instant.
#[must_use]
pub fn initial_phase(host: &str, poll: Duration) -> Duration {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in host.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let bound = MAX_INITIAL_PHASE.min(poll);
    if bound.is_zero() {
        return Duration::ZERO;
    }
    Duration::from_nanos(hash % bound.as_nanos() as u64)
}

/// The per-node mirror topic: `state/services/<node>`.
#[must_use]
pub fn state_topic(node: &str) -> String {
    format!("state/services/{node}")
}

/// Wall-clock milliseconds since the Unix epoch (i64 to match the wire types).
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Resolve the configured/user Bus root, falling back to the canonical system
/// spool when no user data root exists.
fn service_aggregator_bus_root(resolved: Option<PathBuf>) -> PathBuf {
    resolved.unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn default_bus_root() -> PathBuf {
    service_aggregator_bus_root(mde_bus::default_data_dir())
}

fn next_bus_retry_interval(current: Duration) -> Duration {
    current
        .saturating_mul(2)
        .clamp(MIN_BUS_RETRY_INTERVAL, MAX_BUS_RETRY_INTERVAL)
}

fn require_live_bus_index(persist: &Persist, bus_root: &Path) -> Result<(), String> {
    let index = bus_root.join("index.sqlite");
    let metadata = std::fs::metadata(&index)
        .map_err(|error| format!("inspect live Bus index {}: {error}", index.display()))?;
    if !metadata.is_file() {
        return Err(format!("live Bus index is not a file: {}", index.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if persist.index_inode() != Some(metadata.ino()) {
            return Err(format!(
                "Bus index identity changed without a successful reopen: {}",
                index.display()
            ));
        }
    }
    Ok(())
}

/// Strict retained-wire form for the desktop-source roster. The published
/// worker's data types predate this catalog boundary and do not all carry
/// `deny_unknown_fields`, so the aggregator decodes a closed local envelope
/// before converting back to the shared worker types.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedDesktopSourcesWire {
    node: String,
    sources: Vec<RetainedDesktopSourceWire>,
    lanes: Vec<RetainedLaneStatusWire>,
    published_at_ms: u64,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedDesktopSourceWire {
    id: String,
    name: String,
    node: String,
    host: String,
    protocols: Vec<RetainedProtocolOfferWire>,
    origin: SourceOrigin,
    reachability: Reachability,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    os_hint: Option<String>,
    #[serde(default)]
    power_state: Option<String>,
    thumbnail_ref: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedProtocolOfferWire {
    protocol: DesktopProtocol,
    #[serde(default)]
    port: Option<u16>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedLaneStatusWire {
    lane: String,
    status: String,
}

fn validate_retained_text(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), String> {
    if value.len() > max_bytes {
        return Err(format!("{field} exceeds {max_bytes} bytes"));
    }
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(format!(
            "{field} is empty, padded, or contains control characters"
        ));
    }
    Ok(())
}

fn decode_retained_desktop_sources(body: &str) -> Result<DesktopSourcesState, String> {
    if body.len() > MAX_RETAINED_DESKTOP_STATE_BYTES {
        return Err(format!(
            "desktop source state is {} bytes; maximum is {}",
            body.len(),
            MAX_RETAINED_DESKTOP_STATE_BYTES
        ));
    }
    let wire: RetainedDesktopSourcesWire = serde_json::from_str(body)
        .map_err(|error| format!("strict desktop state decode: {error}"))?;
    if wire.sources.len() > MAX_RETAINED_DESKTOP_SOURCES {
        return Err(format!(
            "desktop source state contains {} sources; maximum is {}",
            wire.sources.len(),
            MAX_RETAINED_DESKTOP_SOURCES
        ));
    }
    if wire.lanes.len() > MAX_RETAINED_DESKTOP_LANES {
        return Err(format!(
            "desktop source state contains {} lanes; maximum is {}",
            wire.lanes.len(),
            MAX_RETAINED_DESKTOP_LANES
        ));
    }
    if wire.published_at_ms == 0 {
        return Err("desktop source state has a zero published_at_ms".into());
    }
    validate_retained_text("desktop_sources.node", &wire.node, 255)?;

    let mut lane_names = BTreeSet::new();
    let lanes = wire
        .lanes
        .into_iter()
        .map(|lane| {
            validate_retained_text("desktop_sources.lane", &lane.lane, 64)?;
            validate_retained_text("desktop_sources.status", &lane.status, 512)?;
            if !lane_names.insert(lane.lane.clone()) {
                return Err(format!("duplicate desktop source lane: {}", lane.lane));
            }
            Ok(LaneStatus {
                lane: lane.lane,
                status: lane.status,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let sources = wire
        .sources
        .into_iter()
        .map(|source| {
            validate_retained_text("desktop_source.id", &source.id, 512)?;
            validate_retained_text("desktop_source.node", &source.node, 255)?;
            if source.protocols.len() > MAX_RETAINED_DESKTOP_PROTOCOLS {
                return Err(format!(
                    "desktop source {} contains {} protocols; maximum is {}",
                    source.id,
                    source.protocols.len(),
                    MAX_RETAINED_DESKTOP_PROTOCOLS
                ));
            }
            for value in [
                source.os_hint.as_deref(),
                source.power_state.as_deref(),
                source.thumbnail_ref.as_deref(),
            ]
            .into_iter()
            .flatten()
            {
                validate_retained_text("desktop_source.optional_text", value, 1_024)?;
            }
            Ok(DesktopSource {
                id: source.id,
                name: source.name,
                node: source.node,
                host: source.host,
                protocols: source
                    .protocols
                    .into_iter()
                    .map(|offer| ProtocolOffer::new(offer.protocol, offer.port))
                    .collect(),
                origin: source.origin,
                reachability: source.reachability,
                reason: source.reason,
                os_hint: source.os_hint,
                power_state: source.power_state,
                thumbnail_ref: source.thumbnail_ref,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(DesktopSourcesState {
        node: wire.node,
        sources,
        lanes,
        published_at_ms: wire.published_at_ms,
    })
}

/// The published-directory source seam — the KDC service directory rows.
pub trait PublishedSource: Send + Sync {
    /// Every node's published service row, reduced to the merge inputs.
    fn read(&self) -> Vec<PublishedInput>;
}

/// The probe-inventory source seam — the nmap-discovered open services.
pub trait ProbeSource: Send + Sync {
    /// Every probed `(host, service)` open port, reduced to the merge inputs.
    fn read(&self) -> Vec<ProbeInput>;
}

/// Production published source: reads `<workgroup>/kdc-services/*.json` via
/// [`mde_kdc_host::service_directory::collect_all_services`].
pub struct DirectoryPublished {
    workgroup_root: PathBuf,
}

impl DirectoryPublished {
    /// Read the directory under `workgroup_root`.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self { workgroup_root }
    }
}

impl PublishedSource for DirectoryPublished {
    fn read(&self) -> Vec<PublishedInput> {
        mde_kdc_host::service_directory::collect_all_services(&self.workgroup_root)
            .into_iter()
            .map(|n| PublishedInput {
                host: n.node_host,
                endpoint_ip: n.overlay_ip,
                services: n.services,
                updated_ms: n.updated_ms,
            })
            .collect()
    }
}

/// Production probe source: reads the merged `probe-inventory.json` cards via
/// [`crate::probe_nmap::inventory`], flattening each host's open-service children.
pub struct InventoryProbe {
    workgroup_root: PathBuf,
}

impl InventoryProbe {
    /// Read the inventory under `workgroup_root`.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self { workgroup_root }
    }
}

impl ProbeSource for InventoryProbe {
    fn read(&self) -> Vec<ProbeInput> {
        let mut out = Vec::new();
        for host in crate::probe_nmap::inventory(&self.workgroup_root) {
            let Some(hf) = crate::card::probe::host_facts(&host) else {
                continue;
            };
            let host_name = if hf.hostname.is_empty() {
                hf.ip.clone()
            } else {
                hf.hostname.clone()
            };
            // Probe last-seen is Unix seconds; the merge keys on Unix-ms.
            let last_seen_ms = i64::try_from(hf.last_seen)
                .unwrap_or(i64::MAX)
                .saturating_mul(1000);
            for child in &host.children {
                let Some(sf) = crate::card::probe::service_facts(child) else {
                    continue;
                };
                let kind = if sf.service_kind.is_empty() {
                    format!("port/{}", sf.port)
                } else {
                    sf.service_kind.clone()
                };
                out.push(ProbeInput {
                    host: host_name.clone(),
                    ip: hf.ip.clone(),
                    port: sf.port,
                    kind,
                    last_seen_ms,
                });
            }
        }
        out
    }
}

struct RetainedSource<T> {
    value: Option<T>,
    ulid: Option<String>,
}

#[derive(Clone)]
struct LastPublication {
    services: ServicesState,
    desktop_ulid: Option<String>,
    ssh_x11_ulid: Option<String>,
    upnp_ulid: Option<String>,
}

impl LastPublication {
    fn same_inputs(&self, other: &Self) -> bool {
        self.services.same_ignoring_time(&other.services)
            && self.desktop_ulid == other.desktop_ulid
            && self.ssh_x11_ulid == other.ssh_x11_ulid
            && self.upnp_ulid == other.upnp_ulid
    }
}

struct StagedPublication {
    identity: LastPublication,
    outputs: Vec<(String, String)>,
}

/// The WL-FUNC-019 `service_aggregator` worker.
pub struct ServiceAggregatorWorker {
    /// This node's id — the mirror `host` stamp + topic namespace.
    host: String,
    /// Shared root holding non-secret desired service configuration.
    workgroup_root: PathBuf,
    /// The published-directory half.
    published: Arc<dyn PublishedSource>,
    /// The probe-inventory half.
    probe: Arc<dyn ProbeSource>,
    /// Concrete Bus root. Resolution uses the configured/user root and falls
    /// back to the canonical system spool.
    bus_root: PathBuf,
    /// Fold cadence.
    poll: Duration,
    /// Mirror republish heartbeat.
    heartbeat: Duration,
    /// Stale-entry age-out window.
    ttl: Duration,
    /// Approved secret-store backend for detached catalog publisher proofs.
    publisher_store: SecretStore,
    /// Negative-result cache for the optional publisher-proof lookup.
    publisher_retry: std::sync::Mutex<PublisherRetryState>,
    #[cfg(test)]
    bus_read_gate: Option<Arc<BusReadGateFn>>,
    #[cfg(test)]
    bus_write_gate: Option<Arc<BusWriteGateFn>>,
}

impl ServiceAggregatorWorker {
    /// Construct with production defaults: the replicated KDC directory + the merged
    /// probe inventory under `workgroup_root`, the persisted Bus tree, and the
    /// default cadences. `host` is this node's id.
    #[must_use]
    pub fn new(host: String, workgroup_root: PathBuf) -> Self {
        Self {
            host,
            workgroup_root: workgroup_root.clone(),
            published: Arc::new(DirectoryPublished::new(workgroup_root.clone())),
            probe: Arc::new(InventoryProbe::new(workgroup_root.clone())),
            bus_root: default_bus_root(),
            poll: DEFAULT_POLL_INTERVAL,
            heartbeat: PUBLISH_HEARTBEAT,
            ttl: DEFAULT_TTL,
            publisher_store: SecretStore::resolve(&repo_root(), &workgroup_root),
            publisher_retry: std::sync::Mutex::new(PublisherRetryState::default()),
            #[cfg(test)]
            bus_read_gate: None,
            #[cfg(test)]
            bus_write_gate: None,
        }
    }

    /// Inject the published source (tests).
    #[must_use]
    pub fn with_published(mut self, published: Arc<dyn PublishedSource>) -> Self {
        self.published = published;
        self
    }

    /// Inject the probe source (tests).
    #[must_use]
    pub fn with_probe(mut self, probe: Arc<dyn ProbeSource>) -> Self {
        self.probe = probe;
        self
    }

    /// Override the Bus root (tests point it at a tempdir / `None`).
    #[must_use]
    pub fn with_bus_root(mut self, bus_root: Option<PathBuf>) -> Self {
        self.bus_root = service_aggregator_bus_root(bus_root);
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_bus_read_gate(mut self, gate: Arc<BusReadGateFn>) -> Self {
        self.bus_read_gate = Some(gate);
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_bus_write_gate(mut self, gate: Arc<BusWriteGateFn>) -> Self {
        self.bus_write_gate = Some(gate);
        self
    }

    /// Override the fold cadence (tests, to avoid multi-second waits).
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Override the stale age-out window (tests).
    #[must_use]
    pub const fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Override the publisher key store for deterministic tests or a controlled
    /// deployment. The key value itself never enters the catalog or logs.
    #[must_use]
    pub fn with_publisher_store(mut self, publisher_store: SecretStore) -> Self {
        self.publisher_store = publisher_store;
        self
    }

    /// Read both sources and fold them into the current [`ServicesState`]. No
    /// publish — the pure step the tick + tests share.
    fn fold_state(&self) -> ServicesState {
        let published = self.published.read();
        let probes = self.probe.read();
        let now = now_ms();
        let ttl_ms = i64::try_from(self.ttl.as_millis()).unwrap_or(i64::MAX);
        let records = aggregate(&published, &probes, now, ttl_ms);
        ServicesState {
            host: self.host.clone(),
            records,
            published_at_ms: now,
        }
    }

    fn gate_bus_read(&self, topic: &str, index: usize) -> Result<(), String> {
        #[cfg(test)]
        if let Some(gate) = self.bus_read_gate.as_ref() {
            return gate(topic, index);
        }
        Ok(())
    }

    fn read_retained_desktop_sources(
        &self,
        persist: &Persist,
    ) -> Result<RetainedSource<DesktopSourcesState>, String> {
        self.gate_bus_read(SOURCES_TOPIC, 0)?;
        let Some(message) = persist
            .read_latest(SOURCES_TOPIC)
            .map_err(|error| format!("read retained desktop sources: {error}"))?
        else {
            return Ok(RetainedSource {
                value: None,
                ulid: None,
            });
        };
        let body = message
            .body
            .as_deref()
            .ok_or_else(|| "retained desktop sources row has no body".to_owned())?;
        Ok(RetainedSource {
            value: Some(decode_retained_desktop_sources(body)?),
            ulid: Some(message.ulid),
        })
    }

    fn read_retained_ssh_x11_sources(
        &self,
        persist: &Persist,
    ) -> Result<RetainedSource<SshX11SourcesState>, String> {
        self.gate_bus_read(SSH_X11_SOURCES_TOPIC, 1)?;
        let Some(message) = persist
            .read_latest(SSH_X11_SOURCES_TOPIC)
            .map_err(|error| format!("read retained SSH/X11 sources: {error}"))?
        else {
            return Ok(RetainedSource {
                value: None,
                ulid: None,
            });
        };
        let body = message
            .body
            .as_deref()
            .ok_or_else(|| "retained SSH/X11 source row has no body".to_owned())?;
        Ok(RetainedSource {
            value: Some(decode_sources_state(body)?),
            ulid: Some(message.ulid),
        })
    }

    fn read_retained_upnp_sources(
        &self,
        persist: &Persist,
    ) -> Result<RetainedSource<UpnpSourcesState>, String> {
        self.gate_bus_read(UPNP_SOURCES_TOPIC, 2)?;
        let Some(message) = persist
            .read_latest(UPNP_SOURCES_TOPIC)
            .map_err(|error| format!("read retained UPnP sources: {error}"))?
        else {
            return Ok(RetainedSource {
                value: None,
                ulid: None,
            });
        };
        let body = message
            .body
            .as_deref()
            .ok_or_else(|| "retained UPnP source row has no body".to_owned())?;
        Ok(RetainedSource {
            value: Some(decode_upnp_sources_state(body).map_err(|error| error.to_string())?),
            ulid: Some(message.ulid),
        })
    }

    fn prepare_resource_mirror_outputs(
        &self,
        catalog: &ResourceCatalog,
    ) -> Result<Vec<(String, String)>, String> {
        let catalog = catalog
            .clone()
            .with_content_digest()
            .map_err(|error| format!("validate resource catalog content digest: {error}"))?;
        let capability_count = catalog
            .cards
            .iter()
            .map(|card| card.client_capabilities.len())
            .sum::<usize>();
        let registry = ClientCapabilityRegistry::admitted(
            catalog
                .cards
                .iter()
                .flat_map(|card| card.client_capabilities.iter().cloned()),
        )
        .map_err(|error| {
            format!(
                "admit {capability_count} resource client capabilities before publication: {error}"
            )
        })?;

        tracing::debug!(
            host = %self.host,
            admitted_client_capabilities = registry.len(),
            "client capabilities admitted for resource publication"
        );

        let publisher_attestation = self.publisher_attestation(&catalog)?;
        let discovery = catalog
            .discovery_projection()
            .map_err(|error| format!("derive resource discovery projection: {error}"))?;
        discovery
            .validate()
            .map_err(|error| format!("validate resource discovery projection: {error}"))?;

        let mut outputs = vec![(
            RESOURCE_CATALOG_TOPIC.to_owned(),
            serde_json::to_string(&catalog)
                .map_err(|error| format!("encode resource catalog: {error}"))?,
        )];
        if let Some(attestation) = publisher_attestation {
            outputs.push((
                resource_publisher_attestation_topic(&catalog.publisher),
                serde_json::to_string(&attestation)
                    .map_err(|error| format!("encode resource publisher attestation: {error}"))?,
            ));
        }
        outputs.push((
            RESOURCE_DISCOVERY_TOPIC.to_owned(),
            serde_json::to_string(&discovery)
                .map_err(|error| format!("encode resource discovery projection: {error}"))?,
        ));
        Ok(outputs)
    }

    fn stage_cycle(&self, persist: &mut Persist) -> Result<StagedPublication, String> {
        persist.reopen_if_index_changed();
        require_live_bus_index(persist, &self.bus_root)?;

        let services = self.fold_state();
        let desktop = self.read_retained_desktop_sources(persist)?;
        let ssh_x11 = self.read_retained_ssh_x11_sources(persist)?;
        let upnp = self.read_retained_upnp_sources(persist)?;
        let catalog = super::service_catalog::catalog_from_services_with_root_and_desktops_and_ssh_x11_and_upnp(
            &services,
            &self.workgroup_root,
            desktop.value.as_ref(),
            ssh_x11.value.as_ref(),
            upnp.value.as_ref(),
        )
        .map_err(|error| format!("derive universal resource catalog: {error}"))?;
        let (catalog, adapter_status) = resource_adapters::augment_from_production(
            catalog,
            &self.workgroup_root,
            Some(&self.bus_root),
        )
        .map_err(|error| format!("derive resource adapter catalog: {error}"))?;

        let mut outputs = vec![
            (
                state_topic(&self.host),
                serde_json::to_string(&services)
                    .map_err(|error| format!("encode services state: {error}"))?,
            ),
            (
                resource_adapters::RESOURCE_ADAPTER_STATUS_TOPIC.to_owned(),
                serde_json::to_string(&adapter_status)
                    .map_err(|error| format!("encode resource adapter status: {error}"))?,
            ),
        ];
        outputs.extend(self.prepare_resource_mirror_outputs(&catalog)?);
        require_live_bus_index(persist, &self.bus_root)?;

        Ok(StagedPublication {
            identity: LastPublication {
                services,
                desktop_ulid: desktop.ulid,
                ssh_x11_ulid: ssh_x11.ulid,
                upnp_ulid: upnp.ulid,
            },
            outputs,
        })
    }

    fn publish_staged(
        &self,
        persist: &mut Persist,
        staged: &StagedPublication,
    ) -> Result<(), String> {
        persist.reopen_if_index_changed();
        require_live_bus_index(persist, &self.bus_root)?;
        for (index, (topic, body)) in staged.outputs.iter().enumerate() {
            #[cfg(test)]
            if let Some(gate) = self.bus_write_gate.as_ref() {
                gate(topic, index)?;
            }
            persist
                .write(topic, Priority::Default, None, Some(body))
                .map_err(|error| format!("publish {topic}: {error}"))?;
        }
        require_live_bus_index(persist, &self.bus_root)
    }

    fn cycle_and_publish(
        &self,
        persist: &mut Persist,
        last: &mut Option<LastPublication>,
        last_pub_at: &mut Option<Instant>,
    ) -> Result<bool, String> {
        let staged = self.stage_cycle(persist)?;
        let now = Instant::now();
        let changed = last
            .as_ref()
            .is_none_or(|previous| !previous.same_inputs(&staged.identity));
        let heartbeat_due = last_pub_at.is_none_or(|at| now.duration_since(at) >= self.heartbeat);
        if !changed && !heartbeat_due {
            return Ok(false);
        }
        self.publish_staged(persist, &staged)?;
        *last = Some(staged.identity);
        *last_pub_at = Some(now);
        Ok(true)
    }

    #[cfg(test)]
    fn publish_resource_mirrors(&self, catalog: &ResourceCatalog) -> Result<(), String> {
        let outputs = self.prepare_resource_mirror_outputs(catalog)?;
        let mut persist = Persist::open(self.bus_root.clone())
            .map_err(|error| format!("open Bus for resource mirror test: {error}"))?;
        self.publish_staged(
            &mut persist,
            &StagedPublication {
                identity: LastPublication {
                    services: ServicesState {
                        host: self.host.clone(),
                        records: Vec::new(),
                        published_at_ms: now_ms(),
                    },
                    desktop_ulid: None,
                    ssh_x11_ulid: None,
                    upnp_ulid: None,
                },
                outputs,
            },
        )
    }

    /// Mint a detached proof from the approved secret store. A missing key is
    /// an honest compatibility state: the catalog remains available to legacy
    /// consumers, but no authenticated publication is claimed.
    fn publisher_attestation(
        &self,
        catalog: &ResourceCatalog,
    ) -> Result<Option<ResourcePublisherAttestation>, String> {
        let now = Instant::now();
        let allowed = self
            .publisher_retry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .allowed(now);
        if !allowed {
            tracing::debug!(
                host = %self.host,
                key_ref = RESOURCE_PUBLISHER_KEY_REF,
                "publisher key lookup still in bounded retry backoff"
            );
            return Ok(None);
        }

        let record_failure = |state: &std::sync::Mutex<PublisherRetryState>| {
            state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .record_failure(now);
        };
        let key = match self.publisher_store.get(RESOURCE_PUBLISHER_KEY_REF) {
            Ok(Some(key)) => {
                self.publisher_retry
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .record_success();
                key
            }
            Ok(None) => {
                record_failure(&self.publisher_retry);
                tracing::warn!(
                    host = %self.host,
                    key_ref = RESOURCE_PUBLISHER_KEY_REF,
                    "resource catalog publisher key is not distributed; authenticated proof withheld"
                );
                return Ok(None);
            }
            Err(error) => {
                record_failure(&self.publisher_retry);
                return Err(format!(
                    "resource catalog publisher key lookup failed: {error}"
                ));
            }
        };
        let issued_at_ms = u64::try_from(now_ms()).unwrap_or(1).max(1);
        let expires_at_ms = issued_at_ms.saturating_add(RESOURCE_PUBLISHER_ATTESTATION_TTL_MS);
        let attestation = ResourcePublisherAttestation::mint(
            catalog,
            RESOURCE_PUBLISHER_ATTESTATION_KEY_ID,
            key.as_bytes(),
            issued_at_ms,
            expires_at_ms,
        )
        .map_err(|error| format!("mint resource publisher attestation: {error}"))?;
        catalog
            .validate_publisher_attestation(&attestation, key.as_bytes(), issued_at_ms)
            .map_err(|error| format!("validate staged resource publisher attestation: {error}"))?;
        Ok(Some(attestation))
    }
}

#[async_trait::async_trait]
impl Worker for ServiceAggregatorWorker {
    fn name(&self) -> &'static str {
        "service_aggregator"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let mut last: Option<LastPublication> = None;
        let mut last_pub_at: Option<Instant> = None;
        let phase = initial_phase(&self.host, self.poll);
        tokio::select! {
            () = tokio::time::sleep(phase) => {}
            () = shutdown.wait() => return Ok(()),
        }
        let mut retry_interval = MIN_BUS_RETRY_INTERVAL;
        let mut persist = loop {
            match Persist::open(self.bus_root.clone()) {
                Ok(persist) => break persist,
                Err(error) => tracing::warn!(
                    target: "mackesd::service_aggregator",
                    host = %self.host,
                    %error,
                    "Service Aggregator Bus unavailable; startup will retry"
                ),
            }
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                _ = tokio::time::sleep(retry_interval) => {}
            }
            retry_interval = next_bus_retry_interval(retry_interval);
        };
        // Fold + publish after a bounded host-specific phase so a fleet does not
        // perform the first source reads and resource projection in lockstep.
        if let Err(error) = self.cycle_and_publish(&mut persist, &mut last, &mut last_pub_at) {
            tracing::warn!(
                target: "mackesd::service_aggregator",
                host = %self.host,
                %error,
                "Service Aggregator cycle deferred"
            );
        }
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await; // consume the immediate first tick
        let mut action_worker = resource_actions::ResourceActionWorker::production(
            Some(self.bus_root.clone()),
            self.publisher_store.clone(),
        );
        let mut action_tick = tokio::time::interval(mde_bus::rpc::CONTROL_POLL_INTERVAL);
        action_tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if let Err(error) = self.cycle_and_publish(&mut persist, &mut last, &mut last_pub_at) {
                        tracing::warn!(
                            target: "mackesd::service_aggregator",
                            host = %self.host,
                            %error,
                            "Service Aggregator cycle deferred"
                        );
                    }
                }
                _ = action_tick.tick() => {
                    action_worker.tick(u64::try_from(now_ms()).unwrap_or(0));
                }
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::resources::{
        AuthMethod, ClientBoundary, ClientCapability, ClientCapabilityLimits, ClientFeature,
        ResourceActionVerb, ResourceCatalog, ResourceClass, ResourceDiscoveryProjection,
        ResourcePublisherAttestation, TransportProtocol,
    };
    use mackes_mesh_types::service_record::{ServiceHealth, ServiceProvenance};
    use std::sync::atomic::{AtomicBool, Ordering};

    struct FakePublished(Vec<PublishedInput>);
    impl PublishedSource for FakePublished {
        fn read(&self) -> Vec<PublishedInput> {
            self.0.clone()
        }
    }

    struct FakeProbe(Vec<ProbeInput>);
    impl ProbeSource for FakeProbe {
        fn read(&self) -> Vec<ProbeInput> {
            self.0.clone()
        }
    }

    fn worker_with(
        published: Vec<PublishedInput>,
        probes: Vec<ProbeInput>,
    ) -> ServiceAggregatorWorker {
        ServiceAggregatorWorker::new("me".into(), PathBuf::from("/tmp"))
            .with_bus_root(None)
            .with_published(Arc::new(FakePublished(published)))
            .with_probe(Arc::new(FakeProbe(probes)))
            // A generous TTL so a fixed test timestamp stays fresh.
            .with_ttl(Duration::from_secs(3_600 * 24 * 365 * 100))
    }

    #[test]
    fn initial_phase_is_stable_and_bounded_by_poll() {
        let poll = Duration::from_secs(15);
        let first = initial_phase("seat-oak", poll);
        assert_eq!(first, initial_phase("seat-oak", poll));
        assert!(first < MAX_INITIAL_PHASE);
        assert!(initial_phase("seat-oak", Duration::from_millis(100)) < Duration::from_millis(100));
        assert_eq!(initial_phase("seat-oak", Duration::ZERO), Duration::ZERO);
    }

    fn local_publisher_store(root: &Path) -> SecretStore {
        let key_path = root.join("mesh-age-key");
        std::fs::write(
            &key_path,
            "AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQSXKLP0E\n",
        )
        .expect("write test age key");
        SecretStore::LocalAead {
            dir: root.join("sealed"),
            key_path,
        }
    }

    fn valid_desktop_state() -> DesktopSourcesState {
        DesktopSourcesState {
            node: "desktop-discovery".into(),
            sources: vec![DesktopSource {
                id: "peer:oak".into(),
                name: "Oak Seat".into(),
                node: "oak".into(),
                host: "10.42.0.7".into(),
                protocols: vec![ProtocolOffer::new(DesktopProtocol::Rdp, Some(3389))],
                origin: SourceOrigin::MeshPeer,
                reachability: Reachability::Reachable,
                reason: None,
                os_hint: None,
                power_state: None,
                thumbnail_ref: None,
            }],
            lanes: vec![],
            published_at_ms: 1_700_000_000_000,
        }
    }

    fn write_desktop_state(root: &std::path::Path, state: &DesktopSourcesState) {
        let persist = mde_bus::persist::Persist::open(root.to_path_buf()).expect("persist");
        let body = serde_json::to_string(state).expect("desktop state JSON");
        persist
            .write(
                SOURCES_TOPIC,
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(&body),
            )
            .expect("write desktop state");
    }

    async fn wait_for_catalog_card(root: &Path, display_name: &str) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let found = Persist::open(root.to_path_buf())
                    .ok()
                    .and_then(|persist| persist.read_latest(RESOURCE_CATALOG_TOPIC).ok().flatten())
                    .and_then(|message| message.body)
                    .and_then(|body| ResourceCatalog::from_json(&body).ok())
                    .is_some_and(|catalog| {
                        catalog
                            .cards
                            .iter()
                            .any(|card| card.display_name == display_name)
                    });
                if found {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("catalog never contained {display_name}"));
    }

    fn write_upnp_state(root: &std::path::Path) {
        let persist = mde_bus::persist::Persist::open(root.to_path_buf()).expect("persist");
        let state = super::super::upnp_sources::UpnpSourcesState {
            node: "upnp-discovery".into(),
            sources: vec![super::super::upnp_sources::UpnpSourceRecord {
                source_id: "upnp/2f402f80-da50-11e1-9b23-00025b00a001/media_server".into(),
                kind: super::super::upnp_sources::UpnpResourceKind::MediaServer,
                interface: "enp0s31f6".into(),
                source_ip: "172.20.146.20".parse().unwrap(),
                location: super::super::upnp_sources::UpnpLocation {
                    scheme: super::super::upnp_sources::UpnpHttpScheme::Http,
                    host: "172.20.146.20".parse().unwrap(),
                    port: 8200,
                    base_path: Some("/rootDesc.xml".into()),
                },
                observed_at_ms: 1_700_000_000_000,
                expires_at_ms: 1_700_000_120_000,
            }],
            published_at_ms: 1_700_000_000_000,
        };
        let body = serde_json::to_string(&state).expect("UPnP state JSON");
        persist
            .write(
                UPNP_SOURCES_TOPIC,
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(&body),
            )
            .expect("write UPnP state");
    }

    #[test]
    fn name_and_topic_match_the_census_and_convention() {
        let w = ServiceAggregatorWorker::new("node".into(), PathBuf::from("/tmp"));
        assert_eq!(w.name(), "service_aggregator");
        assert_eq!(state_topic("node-a"), "state/services/node-a");
        assert!(state_topic("x").starts_with("state/"));
    }

    #[test]
    fn fold_state_merges_both_seams_into_the_mirror() {
        let now = now_ms();
        let published = vec![PublishedInput {
            host: "alpha".into(),
            endpoint_ip: Some("10.42.0.5".into()),
            services: vec!["ssh".into()],
            updated_ms: now,
        }];
        let probes = vec![ProbeInput {
            host: "alpha".into(),
            ip: "10.42.0.5".into(),
            port: 22,
            kind: "ssh".into(),
            last_seen_ms: now,
        }];
        let w = worker_with(published, probes);
        let state = w.fold_state();
        assert_eq!(state.host, "me");
        assert_eq!(state.records.len(), 1, "the two seams fold into one record");
        let r = &state.records[0];
        assert_eq!(r.endpoint.as_deref(), Some("10.42.0.5:22"));
        assert_eq!(r.health, ServiceHealth::Up);
        assert!(r.attested_by(ServiceProvenance::Published));
        assert!(r.attested_by(ServiceProvenance::Probe));
        assert!(r.attested_by(ServiceProvenance::Enrichment));
    }

    #[test]
    fn publish_cycle_writes_validated_catalog_and_discovery_projection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let worker = worker_with(vec![], vec![]).with_bus_root(Some(dir.path().to_path_buf()));
        let mut last = None;
        let mut last_pub_at = None;
        let mut persist = Persist::open(dir.path().to_path_buf()).expect("persist");

        worker
            .cycle_and_publish(&mut persist, &mut last, &mut last_pub_at)
            .expect("publish staged cycle");

        let body = persist
            .read_latest(RESOURCE_CATALOG_TOPIC)
            .expect("read catalog")
            .and_then(|message| message.body)
            .expect("catalog body");
        let catalog = ResourceCatalog::from_json(&body).expect("validated catalog");
        assert_eq!(catalog.publisher, "me");
        assert!(catalog.content_digest.is_some());
        assert!(catalog
            .cards
            .iter()
            .any(|card| card.display_name == "Jellyfin"));

        let projection_body = persist
            .read_latest(RESOURCE_DISCOVERY_TOPIC)
            .expect("read discovery projection")
            .and_then(|message| message.body)
            .expect("discovery projection body");
        let projection: ResourceDiscoveryProjection =
            serde_json::from_str(&projection_body).expect("projection JSON");
        projection
            .validate()
            .expect("validated discovery projection");
        assert_eq!(projection.catalog_content_digest, catalog.content_digest);
        assert_eq!(
            projection,
            catalog
                .discovery_projection()
                .expect("projection derived from validated catalog")
        );
    }

    #[test]
    fn no_retained_desktop_state_preserves_existing_resource_cards() {
        let dir = tempfile::tempdir().expect("tempdir");
        let worker = worker_with(vec![], vec![]).with_bus_root(Some(dir.path().to_path_buf()));
        let mut last = None;
        let mut last_pub_at = None;
        let mut persist = Persist::open(dir.path().to_path_buf()).expect("persist");

        worker
            .cycle_and_publish(&mut persist, &mut last, &mut last_pub_at)
            .expect("publish staged cycle");

        let body = persist
            .read_latest(RESOURCE_CATALOG_TOPIC)
            .expect("read catalog")
            .and_then(|message| message.body)
            .expect("catalog body");
        let catalog = ResourceCatalog::from_json(&body).expect("validated catalog");
        assert!(catalog
            .cards
            .iter()
            .any(|card| card.display_name == "Jellyfin"));
        assert!(!catalog
            .cards
            .iter()
            .any(|card| card.identity.class == ResourceClass::Desktop));
    }

    #[test]
    fn valid_retained_desktop_row_appears_in_catalog_and_discovery() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_desktop_state(dir.path(), &valid_desktop_state());
        let worker = worker_with(vec![], vec![]).with_bus_root(Some(dir.path().to_path_buf()));
        let mut last = None;
        let mut last_pub_at = None;
        let mut persist = Persist::open(dir.path().to_path_buf()).expect("persist");

        worker
            .cycle_and_publish(&mut persist, &mut last, &mut last_pub_at)
            .expect("publish staged cycle");

        let catalog_body = persist
            .read_latest(RESOURCE_CATALOG_TOPIC)
            .expect("read catalog")
            .and_then(|message| message.body)
            .expect("catalog body");
        let catalog = ResourceCatalog::from_json(&catalog_body).expect("validated catalog");
        let card = catalog
            .cards
            .iter()
            .find(|card| card.display_name == "Oak Seat")
            .expect("desktop card");
        assert_eq!(card.identity.class, ResourceClass::Desktop);
        assert_eq!(card.identity.canonical_key, "peer:oak");
        let resource_id = card.resource_id().to_owned();

        let projection_body = persist
            .read_latest(RESOURCE_DISCOVERY_TOPIC)
            .expect("read discovery")
            .and_then(|message| message.body)
            .expect("discovery body");
        let projection: ResourceDiscoveryProjection =
            serde_json::from_str(&projection_body).expect("discovery JSON");
        projection.validate().expect("validated discovery");
        assert!(projection
            .entries
            .iter()
            .any(|entry| entry.resource_id == resource_id));
    }

    #[test]
    fn valid_retained_upnp_row_appears_in_catalog_and_discovery() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_upnp_state(dir.path());
        let worker = worker_with(vec![], vec![]).with_bus_root(Some(dir.path().to_path_buf()));
        let mut last = None;
        let mut last_pub_at = None;
        let mut persist = Persist::open(dir.path().to_path_buf()).expect("persist");

        worker
            .cycle_and_publish(&mut persist, &mut last, &mut last_pub_at)
            .expect("publish staged cycle");

        let catalog_body = persist
            .read_latest(RESOURCE_CATALOG_TOPIC)
            .expect("read catalog")
            .and_then(|message| message.body)
            .expect("catalog body");
        let catalog = ResourceCatalog::from_json(&catalog_body).expect("validated catalog");
        let card = catalog
            .cards
            .iter()
            .find(|card| card.display_name == "UPnP media server at 172.20.146.20")
            .expect("UPnP card");
        let resource_id = card.resource_id().to_owned();

        let projection_body = persist
            .read_latest(RESOURCE_DISCOVERY_TOPIC)
            .expect("read discovery")
            .and_then(|message| message.body)
            .expect("discovery body");
        let projection: ResourceDiscoveryProjection =
            serde_json::from_str(&projection_body).expect("discovery JSON");
        projection.validate().expect("validated discovery");
        assert!(projection
            .entries
            .iter()
            .any(|entry| entry.resource_id == resource_id));
    }

    #[test]
    fn malformed_or_invalid_retained_desktop_state_suppresses_resource_mirrors() {
        let assert_suppressed = |body: &str| {
            let dir = tempfile::tempdir().expect("tempdir");
            let mut persist =
                mde_bus::persist::Persist::open(dir.path().to_path_buf()).expect("persist");
            persist
                .write(
                    SOURCES_TOPIC,
                    mde_bus::hooks::config::Priority::Default,
                    None,
                    Some(body),
                )
                .expect("write retained state");
            let worker = worker_with(vec![], vec![]).with_bus_root(Some(dir.path().to_path_buf()));
            let mut last = None;
            let mut last_pub_at = None;

            assert!(worker
                .cycle_and_publish(&mut persist, &mut last, &mut last_pub_at)
                .is_err());

            assert!(persist
                .read_latest(&state_topic("me"))
                .expect("read service state")
                .is_none());
            assert!(persist
                .read_latest(RESOURCE_CATALOG_TOPIC)
                .expect("read catalog")
                .is_none());
            assert!(persist
                .read_latest(RESOURCE_DISCOVERY_TOPIC)
                .expect("read discovery")
                .is_none());
            assert!(last.is_none());
            assert!(last_pub_at.is_none());
        };

        assert_suppressed(r#"{"node":"desktop-discovery","sources":[]}"#);

        let mut invalid = valid_desktop_state();
        invalid.sources[0].host = "not a valid host".into();
        assert_suppressed(&serde_json::to_string(&invalid).expect("invalid state JSON"));
    }

    fn test_capability() -> ClientCapability {
        ClientCapability::new(
            "test.rdp",
            "1.0.0",
            TransportProtocol::Rdp,
            "10.7",
            ClientBoundary::ShellNative,
            vec![AuthMethod::MeshIdentity],
            vec![ClientFeature::Display, ClientFeature::KeyboardInput],
            ClientCapabilityLimits {
                max_width: Some(3_840),
                max_height: Some(2_160),
                max_fps: Some(60),
                max_audio_channels: None,
                max_parallel_sessions: 1,
            },
            vec![ResourceActionVerb::Connect],
        )
        .expect("valid test capability")
    }

    fn catalog_with_capabilities(capabilities: Vec<ClientCapability>) -> ResourceCatalog {
        let state = ServicesState {
            host: "me".into(),
            records: vec![],
            published_at_ms: now_ms(),
        };
        let mut catalog = super::super::service_catalog::catalog_from_services(&state)
            .expect("normal service catalog fixture");
        catalog.cards[0].client_capabilities = capabilities;
        catalog
    }

    #[test]
    fn publish_resource_mirrors_rejects_duplicate_capabilities_before_discovery() {
        let dir = tempfile::tempdir().expect("tempdir");
        let worker = worker_with(vec![], vec![]).with_bus_root(Some(dir.path().to_path_buf()));
        let capability = test_capability();
        let catalog = catalog_with_capabilities(vec![capability.clone(), capability]);

        assert!(worker.publish_resource_mirrors(&catalog).is_err());

        let persist = mde_bus::persist::Persist::open(dir.path().to_path_buf()).expect("persist");
        assert!(persist
            .read_latest(RESOURCE_CATALOG_TOPIC)
            .expect("read catalog")
            .is_none());
        assert!(persist
            .read_latest(RESOURCE_DISCOVERY_TOPIC)
            .expect("read discovery projection")
            .is_none());
    }

    #[test]
    fn publication_populates_a_missing_digest_on_both_mirrors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let worker = worker_with(vec![], vec![]).with_bus_root(Some(dir.path().to_path_buf()));
        let mut catalog = catalog_with_capabilities(vec![]);
        catalog.content_digest = None;

        worker
            .publish_resource_mirrors(&catalog)
            .expect("publish resource mirrors");

        let persist = mde_bus::persist::Persist::open(dir.path().to_path_buf()).expect("persist");
        let catalog_body = persist
            .read_latest(RESOURCE_CATALOG_TOPIC)
            .expect("read catalog")
            .and_then(|message| message.body)
            .expect("catalog body");
        let published = ResourceCatalog::from_json(&catalog_body).expect("published catalog");
        assert!(published.content_digest.is_some());

        let projection_body = persist
            .read_latest(RESOURCE_DISCOVERY_TOPIC)
            .expect("read discovery")
            .and_then(|message| message.body)
            .expect("discovery body");
        let projection: ResourceDiscoveryProjection =
            serde_json::from_str(&projection_body).expect("projection JSON");
        projection.validate().expect("published projection");
        assert_eq!(projection.catalog_content_digest, published.content_digest);
    }

    #[test]
    fn publication_mints_and_retains_a_publisher_attestation_from_secret_store() {
        let bus = tempfile::tempdir().expect("bus tempdir");
        let secrets = tempfile::tempdir().expect("secret tempdir");
        let store = local_publisher_store(secrets.path());
        store
            .put(RESOURCE_PUBLISHER_KEY_REF, "publisher-test-key")
            .expect("seal publisher key");
        let worker = worker_with(vec![], vec![])
            .with_bus_root(Some(bus.path().to_path_buf()))
            .with_publisher_store(store);
        let catalog = catalog_with_capabilities(vec![]);

        worker
            .publish_resource_mirrors(&catalog)
            .expect("publish resource mirrors");

        let persist = mde_bus::persist::Persist::open(bus.path().to_path_buf()).expect("persist");
        let catalog_body = persist
            .read_latest(RESOURCE_CATALOG_TOPIC)
            .expect("read catalog")
            .and_then(|message| message.body)
            .expect("catalog body");
        let published = ResourceCatalog::from_json(&catalog_body).expect("published catalog");
        let attestation_body = persist
            .read_latest(&resource_publisher_attestation_topic(&published.publisher))
            .expect("read publisher attestation")
            .and_then(|message| message.body)
            .expect("publisher attestation body");
        let attestation: ResourcePublisherAttestation =
            serde_json::from_str(&attestation_body).expect("publisher attestation JSON");
        published
            .validate_publisher_attestation(&attestation, b"publisher-test-key", now_ms() as u64)
            .expect("publisher attestation validates");
        assert_eq!(attestation.publisher, published.publisher);
        assert_eq!(
            attestation.key_id,
            mackes_mesh_types::resources::RESOURCE_PUBLISHER_ATTESTATION_KEY_ID
        );
    }

    #[test]
    fn publication_without_a_distributed_publisher_key_withholds_only_the_proof() {
        let bus = tempfile::tempdir().expect("bus tempdir");
        let worker = worker_with(vec![], vec![]).with_bus_root(Some(bus.path().to_path_buf()));
        let catalog = catalog_with_capabilities(vec![]);

        worker
            .publish_resource_mirrors(&catalog)
            .expect("publish resource mirrors");

        let persist = mde_bus::persist::Persist::open(bus.path().to_path_buf()).expect("persist");
        let catalog_body = persist
            .read_latest(RESOURCE_CATALOG_TOPIC)
            .expect("read catalog")
            .and_then(|message| message.body)
            .expect("legacy catalog remains available");
        let published = ResourceCatalog::from_json(&catalog_body).expect("published catalog");
        assert!(persist
            .read_latest(&resource_publisher_attestation_topic(&published.publisher))
            .expect("read missing publisher attestation")
            .is_none());
    }

    #[test]
    fn publisher_retry_state_is_bounded_and_resets_after_success() {
        let mut state = PublisherRetryState::default();
        let start = Instant::now();
        assert!(state.allowed(start));

        state.record_failure(start);
        assert!(!state.allowed(start + PUBLISHER_RETRY_BASE - Duration::from_nanos(1)));
        assert!(state.allowed(start + PUBLISHER_RETRY_BASE));

        for _ in 0..16 {
            state.record_failure(start + PUBLISHER_RETRY_MAX);
        }
        assert_eq!(state.backoff, PUBLISHER_RETRY_MAX);
        assert!(!state.allowed(start + PUBLISHER_RETRY_MAX));

        state.record_success();
        assert!(state.allowed(start));
        assert_eq!(state.backoff, Duration::ZERO);
        assert!(state.next_attempt.is_none());
    }

    #[test]
    fn final_retained_source_failure_publishes_nothing_and_advances_nothing() {
        let bus = tempfile::tempdir().expect("bus tempdir");
        let mut persist = Persist::open(bus.path().to_path_buf()).expect("persist");
        let worker = worker_with(vec![], vec![])
            .with_bus_root(Some(bus.path().to_path_buf()))
            .with_bus_read_gate(Arc::new(|topic, index| {
                (index != 2)
                    .then_some(())
                    .ok_or_else(|| format!("injected final-source read failure for {topic}"))
            }));
        let mut last = None;
        let mut last_pub_at = None;

        assert!(worker
            .cycle_and_publish(&mut persist, &mut last, &mut last_pub_at)
            .is_err());
        for topic in [
            state_topic("me"),
            resource_adapters::RESOURCE_ADAPTER_STATUS_TOPIC.to_owned(),
            RESOURCE_CATALOG_TOPIC.to_owned(),
            RESOURCE_DISCOVERY_TOPIC.to_owned(),
        ] {
            assert!(persist
                .read_latest(&topic)
                .expect("read output topic")
                .is_none());
        }
        assert!(last.is_none());
        assert!(last_pub_at.is_none());
    }

    #[test]
    fn write_failure_keeps_cycle_immediately_retryable() {
        let bus = tempfile::tempdir().expect("bus tempdir");
        let mut persist = Persist::open(bus.path().to_path_buf()).expect("persist");
        let fail_write = Arc::new(AtomicBool::new(true));
        let worker = worker_with(vec![], vec![])
            .with_bus_root(Some(bus.path().to_path_buf()))
            .with_bus_write_gate({
                let fail_write = Arc::clone(&fail_write);
                Arc::new(move |topic, index| {
                    (index != 3 || !fail_write.load(Ordering::SeqCst))
                        .then_some(())
                        .ok_or_else(|| format!("injected write failure for {topic}"))
                })
            });
        let mut last = None;
        let mut last_pub_at = None;

        assert!(worker
            .cycle_and_publish(&mut persist, &mut last, &mut last_pub_at)
            .is_err());
        assert!(last.is_none());
        assert!(last_pub_at.is_none());
        assert!(persist
            .read_latest(RESOURCE_DISCOVERY_TOPIC)
            .expect("read failed discovery output")
            .is_none());

        fail_write.store(false, Ordering::SeqCst);
        assert!(worker
            .cycle_and_publish(&mut persist, &mut last, &mut last_pub_at)
            .expect("immediate corrected-forward retry"));
        assert!(last.is_some());
        assert!(last_pub_at.is_some());
        assert_eq!(
            persist
                .list_since(RESOURCE_DISCOVERY_TOPIC, None)
                .expect("list discovery outputs")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn same_worker_recovers_late_external_and_replaced_bus() {
        let base = tempfile::tempdir().expect("base tempdir");
        let bus_root = base.path().join("late-bus");
        std::fs::write(&bus_root, b"temporarily unopenable Bus root").expect("block Bus root");
        let share = tempfile::tempdir().expect("share tempdir");
        let mut worker = ServiceAggregatorWorker::new("me".into(), share.path().to_path_buf())
            .with_bus_root(Some(bus_root.clone()))
            .with_published(Arc::new(FakePublished(vec![])))
            .with_probe(Arc::new(FakeProbe(vec![])))
            .with_poll(Duration::from_millis(5));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );

        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(
            !task.is_finished(),
            "late Bus must not terminate the worker"
        );
        std::fs::remove_file(&bus_root).expect("unblock Bus root");
        let mut state = valid_desktop_state();
        write_desktop_state(&bus_root, &state);
        wait_for_catalog_card(&bus_root, "Oak Seat").await;

        state.sources[0].name = "External Forward Seat".into();
        write_desktop_state(&bus_root, &state);
        wait_for_catalog_card(&bus_root, "External Forward Seat").await;

        let detached = base.path().join("detached-bus");
        std::fs::rename(&bus_root, &detached).expect("detach active Bus");
        state.sources[0].name = "Replacement Bus Seat".into();
        write_desktop_state(&bus_root, &state);
        wait_for_catalog_card(&bus_root, "Replacement Bus Seat").await;

        shutdown_tx.send(true).expect("signal shutdown");
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("worker shutdown timeout")
            .expect("join worker")
            .expect("worker result");
        assert_eq!(
            service_aggregator_bus_root(None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
    }

    #[tokio::test]
    async fn tick_loop_exits_promptly_on_shutdown() {
        let base = tempfile::tempdir().expect("base tempdir");
        let unavailable = base.path().join("unavailable-bus");
        std::fs::write(&unavailable, b"not a directory").expect("block Bus root");
        let mut w = worker_with(vec![], vec![])
            .with_bus_root(Some(unavailable))
            .with_poll(Duration::from_millis(10));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        assert!(joined.unwrap().expect("join").is_ok());
    }
}
