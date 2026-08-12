//! WL-FUNC-019 S2 approved-source adapters for the universal resource catalog.
//!
//! The adapters consume only the canonical peer directory and fixed typed
//! retained projections. They never discover node topics with wildcards and
//! never copy provider commands, host paths, URLs, or credentials into cards.
//! Every source is bounded before projection; malformed or unavailable sources
//! contribute a payload-free status row and cannot suppress cards admitted from
//! another source.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use mackes_mesh_types::android_apps::{android_catalog_state_topic, AndroidSignedCatalog};
use mackes_mesh_types::app_catalog::is_valid_flatpak_app_id;
use mackes_mesh_types::cloud::{cloud_state_topic, CloudState, DeliveryType};
use mackes_mesh_types::media_sources::{
    MediaKind, MediaProtocol, Reachability as MediaReachability, SourceOrigin as MediaSourceOrigin,
    MEDIA_SOURCES_TOPIC,
};
use mackes_mesh_types::peers::PeerRecord;
use mackes_mesh_types::resources::{
    ActionAvailability, ActionAvailabilityStatus, AuthState, AuthStatus, DiscoverySource,
    FailureCode, FailureReason, HealthState, HealthStatus, IdentityAuthority, ProvenanceTrust,
    ResourceAction, ResourceActionTarget, ResourceActionVerb, ResourceCard, ResourceCatalog,
    ResourceClass, ResourceOperatingRole, ResourceScope, SourceProvenance,
    RESOURCE_CONTRACT_VERSION,
};
use mackes_mesh_types::workloads::{
    workload_state_topic, WorkloadOperationPhase, WorkloadOperationStatus, WorkloadPowerState,
    WorkloadReadiness, WorkloadStateSnapshot, MAX_WORKLOAD_WIRE_BYTES,
};
use serde::{Deserialize, Serialize};

use crate::workers::app_catalog::{AdmittedFlatpakAppProjection, AdmittedFlatpakCatalogProjection};

/// Retained status for the bounded source-adapter fold.
pub const RESOURCE_ADAPTER_STATUS_TOPIC: &str = "state/resources/adapters";

const MAX_PEER_ROWS: usize = 64;
const MAX_APPROVED_NODES: usize = 64;
const MAX_ADAPTED_CARDS: usize = 1_024;
const SOURCE_TTL_MS: u64 = 120_000;
const MAX_APP_CATALOG_WIRE_BYTES: usize = 512 * 1024;
const MAX_ANDROID_CATALOG_WIRE_BYTES: usize = 2 * 1024 * 1024;
const MAX_CLOUD_STATE_WIRE_BYTES: usize = 2 * 1024 * 1024;
const MAX_MEDIA_WIRE_BYTES: usize = 2 * 1024 * 1024;
const MAX_APP_ROWS: usize = 512;
const MAX_ANDROID_ROWS: usize = 64;
const MAX_MEDIA_ROWS: usize = 1_024;
const MAX_MEDIA_LANES: usize = 64;
const MAX_SAFE_ID_BYTES: usize = 255;
const MAX_SAFE_TEXT_BYTES: usize = 1_024;

/// Closed source identities admitted by this S2 slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceAdapterKind {
    /// Canonical mesh peer directory.
    PeerDirectory,
    /// Authoritative typed Workload state projection.
    Workload,
    /// Node-scoped projection emitted only after signed App VM catalog admission.
    AppVmCatalog,
    /// Node-scoped signed Android catalog retained after importer admission.
    AndroidCatalog,
    /// Canonical retained media roster, excluding its file-share subset.
    Media,
    /// File/share subset of the canonical retained media roster.
    FileShare,
    /// Stable-identity merge boundary.
    Deduplication,
}

/// Payload-free source outcome rendered by diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceAdapterAvailability {
    /// Source was admitted and current.
    Available,
    /// A last observation was admitted but has exceeded its freshness window.
    Stale,
    /// Source was absent or its authority was temporarily unavailable.
    Unavailable,
    /// Source body or typed fields failed admission.
    Malformed,
    /// Multiple non-identical source observations claimed one stable resource ID.
    Conflict,
}

/// One bounded, non-secret source result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceAdapterStatus {
    /// Approved adapter kind.
    pub source: ResourceAdapterKind,
    /// Stable source identity, never a locator.
    pub source_id: String,
    /// Current outcome.
    pub availability: ResourceAdapterAvailability,
    /// Number of cards admitted from this source.
    pub admitted_cards: u16,
}

/// Retained bounded status projection for one catalog fold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceAdapterStatusProjection {
    /// Closed projection schema.
    pub schema_version: u16,
    /// Catalog revision produced by the same fold.
    pub catalog_revision: String,
    /// Fold timestamp.
    pub observed_at_ms: u64,
    /// Deterministically ordered source rows.
    pub sources: Vec<ResourceAdapterStatus>,
}

#[derive(Debug, Default)]
struct AdaptedSources {
    cards: Vec<ResourceCard>,
    statuses: Vec<ResourceAdapterStatus>,
}

/// Read both production sources and merge their admitted cards into `catalog`.
///
/// The peer directory supplies the exact approved node set. Workload topics are
/// therefore never discovered by wildcard scanning. A configured-but-failed
/// etcd directory may provide filesystem fallback rows for unavailable cards,
/// but the status remains unavailable so fallback cannot masquerade as current
/// membership authority.
pub fn augment_from_production(
    catalog: ResourceCatalog,
    workgroup_root: &Path,
    bus_root: Option<&Path>,
) -> Result<(ResourceCatalog, ResourceAdapterStatusProjection), String> {
    let now_ms = catalog.generated_at_ms;
    let mut adapted = AdaptedSources::default();
    let endpoints_configured = !crate::substrate::etcd::default_endpoints().is_empty();
    let (peers, directory_source) =
        crate::substrate::peers::read_directory_with_source(workgroup_root);
    let directory_current = !endpoints_configured
        || matches!(
            directory_source,
            crate::substrate::peers::DirectorySource::Etcd
        );
    adapt_peers(&peers, directory_current, now_ms, &mut adapted);

    match approved_nodes_for_resources(
        &peers,
        &catalog.publisher,
        directory_current,
        now_ms,
    ) {
        Some(approved_nodes) => {
            adapt_workloads(bus_root, &approved_nodes, now_ms, &mut adapted);
            adapt_app_vm_catalogs(bus_root, &approved_nodes, now_ms, &mut adapted);
            adapt_android_catalogs(bus_root, &approved_nodes, now_ms, &mut adapted);
        }
        None => refuse_invalid_approved_nodes(&mut adapted),
    }
    adapt_media(bus_root, &catalog.publisher, now_ms, &mut adapted);

    merge_catalog(catalog, adapted, now_ms)
}

fn refuse_invalid_approved_nodes(adapted: &mut AdaptedSources) {
    for kind in [
        ResourceAdapterKind::Workload,
        ResourceAdapterKind::AppVmCatalog,
        ResourceAdapterKind::AndroidCatalog,
    ] {
        adapted.statuses.push(status(
            kind,
            "approved-nodes",
            ResourceAdapterAvailability::Malformed,
            0,
        ));
    }
}

/// Return the node identities that are safe to use as downstream source
/// authorities. A peer directory is a set of self-owned rows, but a stale or
/// malicious replicated view can contain two non-identical rows claiming one
/// hostname. That hostname may still be rendered as a visible conflict card;
/// it must not authorize Workload/App/Android reads until its identity is
/// unambiguous and its directory observation must still be current. Exact
/// duplicate rows are harmless and collapse here. The local publisher remains
/// an approved authority independently of remote-directory availability.
fn approved_nodes_for_resources(
    peers: &[PeerRecord],
    publisher: &str,
    directory_current: bool,
    now_ms: u64,
) -> Option<BTreeSet<String>> {
    if peers.len() > MAX_PEER_ROWS || !is_safe_id(publisher) {
        return None;
    }

    let mut rows = BTreeMap::<String, PeerRecord>::new();
    let mut ambiguous = BTreeSet::new();
    for peer in peers {
        if !is_safe_id(&peer.hostname) {
            return None;
        }
        match rows.entry(peer.hostname.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(peer.clone());
            }
            std::collections::btree_map::Entry::Occupied(entry) => {
                if entry.get() != peer {
                    ambiguous.insert(peer.hostname.clone());
                }
            }
        }
    }

    let mut approved = rows
        .into_iter()
        .filter(|(hostname, peer)| {
            directory_current
                && !ambiguous.contains(hostname)
                && peer.last_seen_ms > 0
                && peer.last_seen_ms <= now_ms
                && now_ms.saturating_sub(peer.last_seen_ms) < SOURCE_TTL_MS
                && peer_card(peer, true, now_ms).is_ok()
        })
        .map(|(hostname, _)| hostname)
        .collect::<BTreeSet<_>>();
    approved.insert(publisher.to_owned());
    (approved.len() <= MAX_APPROVED_NODES).then_some(approved)
}

fn adapt_peers(
    peers: &[PeerRecord],
    directory_current: bool,
    now_ms: u64,
    adapted: &mut AdaptedSources,
) {
    if peers.len() > MAX_PEER_ROWS {
        adapted.statuses.push(status(
            ResourceAdapterKind::PeerDirectory,
            "mesh-directory",
            ResourceAdapterAvailability::Malformed,
            0,
        ));
        return;
    }
    let mut cards = Vec::with_capacity(peers.len());
    for peer in peers {
        match peer_card(peer, directory_current, now_ms) {
            Ok(card) => cards.push(card),
            Err(()) => {
                adapted.statuses.push(status(
                    ResourceAdapterKind::PeerDirectory,
                    "mesh-directory",
                    ResourceAdapterAvailability::Malformed,
                    0,
                ));
                return;
            }
        }
    }
    let availability = if directory_current {
        ResourceAdapterAvailability::Available
    } else {
        ResourceAdapterAvailability::Unavailable
    };
    adapted.statuses.push(status(
        ResourceAdapterKind::PeerDirectory,
        "mesh-directory",
        availability,
        cards.len(),
    ));
    adapted.cards.extend(cards);
}

fn adapt_workloads(
    bus_root: Option<&Path>,
    approved_nodes: &BTreeSet<String>,
    now_ms: u64,
    adapted: &mut AdaptedSources,
) {
    let persist =
        bus_root.and_then(|root| mde_bus::persist::Persist::open(root.to_path_buf()).ok());
    for node in approved_nodes {
        let source_id = format!("workload/{node}");
        let Some(persist) = persist.as_ref() else {
            adapted.statuses.push(status(
                ResourceAdapterKind::Workload,
                &source_id,
                ResourceAdapterAvailability::Unavailable,
                0,
            ));
            continue;
        };
        let message = match persist.read_latest(&workload_state_topic(node)) {
            Ok(Some(message)) => message,
            _ => {
                adapted.statuses.push(status(
                    ResourceAdapterKind::Workload,
                    &source_id,
                    ResourceAdapterAvailability::Unavailable,
                    0,
                ));
                continue;
            }
        };
        let Some(body) = message.body else {
            adapted.statuses.push(status(
                ResourceAdapterKind::Workload,
                &source_id,
                ResourceAdapterAvailability::Malformed,
                0,
            ));
            continue;
        };
        let snapshot = match decode_workload_snapshot(&body, node, now_ms) {
            Ok(snapshot) => snapshot,
            Err(()) => {
                adapted.statuses.push(status(
                    ResourceAdapterKind::Workload,
                    &source_id,
                    ResourceAdapterAvailability::Malformed,
                    0,
                ));
                continue;
            }
        };
        let stale = now_ms.saturating_sub(snapshot.observed_at_ms) >= SOURCE_TTL_MS;
        let mut cards = Vec::with_capacity(snapshot.workloads.len());
        let mut malformed = false;
        for workload in &snapshot.workloads {
            match workload_card(&snapshot.node, snapshot.observed_at_ms, workload, stale) {
                Ok(card) => cards.push(card),
                Err(()) => {
                    malformed = true;
                    break;
                }
            }
        }
        if malformed {
            adapted.statuses.push(status(
                ResourceAdapterKind::Workload,
                &source_id,
                ResourceAdapterAvailability::Malformed,
                0,
            ));
            continue;
        }
        if adapted.cards.len().saturating_add(cards.len()) > MAX_ADAPTED_CARDS {
            adapted.statuses.push(status(
                ResourceAdapterKind::Workload,
                &source_id,
                ResourceAdapterAvailability::Malformed,
                0,
            ));
            continue;
        }
        adapted.statuses.push(status(
            ResourceAdapterKind::Workload,
            &source_id,
            if stale {
                ResourceAdapterAvailability::Stale
            } else {
                ResourceAdapterAvailability::Available
            },
            cards.len(),
        ));
        adapted.cards.extend(cards);
    }
}

fn adapt_app_vm_catalogs(
    bus_root: Option<&Path>,
    approved_nodes: &BTreeSet<String>,
    now_ms: u64,
    adapted: &mut AdaptedSources,
) {
    let persist =
        bus_root.and_then(|root| mde_bus::persist::Persist::open(root.to_path_buf()).ok());
    for node in approved_nodes {
        let source_id = format!("app-vm-catalog/{node}");
        let topic = format!("state/app-catalog/{node}");
        let body = retained_body(persist.as_ref(), &topic);
        adapt_app_vm_body(node, &source_id, body.as_deref(), now_ms, adapted);
    }
}

fn adapt_app_vm_body(
    node: &str,
    source_id: &str,
    body: Option<&str>,
    now_ms: u64,
    adapted: &mut AdaptedSources,
) {
    let Some(body) = body else {
        adapted.statuses.push(status(
            ResourceAdapterKind::AppVmCatalog,
            source_id,
            ResourceAdapterAvailability::Unavailable,
            0,
        ));
        return;
    };
    let projection = if body.len() <= MAX_APP_CATALOG_WIRE_BYTES {
        serde_json::from_str::<AdmittedFlatpakCatalogProjection>(body).ok()
    } else {
        None
    };
    let Some(projection) = projection else {
        adapted.statuses.push(status(
            ResourceAdapterKind::AppVmCatalog,
            source_id,
            ResourceAdapterAvailability::Malformed,
            0,
        ));
        return;
    };
    let valid = projection.schema_version == 1
        && projection.host == node
        && projection.revision > 0
        && projection.issued_at_unix_ms > 0
        && projection.issued_at_unix_ms <= now_ms
        && projection.expires_at_unix_ms > now_ms
        && projection
            .expires_at_unix_ms
            .saturating_sub(projection.issued_at_unix_ms)
            <= 24 * 60 * 60 * 1_000
        && projection.entries.len() <= MAX_APP_ROWS
        && is_safe_id(&projection.catalog_id)
        && is_safe_id(&projection.provider_id)
        && is_safe_id(&projection.repository_id)
        && is_sha256_digest(&projection.content_digest)
        && projection.entries.iter().all(valid_app_projection)
        && projection
            .entries
            .windows(2)
            .all(|pair| pair[0].app_id < pair[1].app_id);
    if !valid {
        adapted.statuses.push(status(
            ResourceAdapterKind::AppVmCatalog,
            source_id,
            ResourceAdapterAvailability::Malformed,
            0,
        ));
        return;
    }
    let expires = projection
        .expires_at_unix_ms
        .min(now_ms.saturating_add(SOURCE_TTL_MS));
    if expires.saturating_sub(now_ms) < 1_000 {
        adapted.statuses.push(status(
            ResourceAdapterKind::AppVmCatalog,
            source_id,
            ResourceAdapterAvailability::Stale,
            0,
        ));
        return;
    }
    let cards = projection
        .entries
        .iter()
        .map(|entry| {
            projection_card(
                ResourceClass::Application,
                format!("app-vm/{node}/{}", entry.app_id),
                entry.display_name.clone(),
                format!("Signed App VM catalog on {node}"),
                now_ms,
                expires,
                HealthStatus::Available,
                None,
                format!("app-vm/{node}/{}", entry.app_id),
                vec![ResourceOperatingRole::Client, ResourceOperatingRole::Loader],
            )
        })
        .collect::<Result<Vec<_>, _>>();
    admit_source_cards(
        ResourceAdapterKind::AppVmCatalog,
        source_id,
        ResourceAdapterAvailability::Available,
        cards,
        adapted,
    );
}

fn valid_app_projection(entry: &AdmittedFlatpakAppProjection) -> bool {
    is_valid_flatpak_app_id(&entry.app_id)
        && is_safe_text(&entry.display_name)
        && is_safe_text(&entry.summary)
        && is_safe_id(&entry.version)
        && is_safe_id(&entry.icon_id)
        && is_safe_id(&entry.guest_profile)
        && entry.permissions.len() <= 32
        && entry.supported_actions.len() <= 32
        && entry.search_terms.len() <= 24
        && entry.permissions.iter().all(|value| is_safe_id(value))
        && entry
            .supported_actions
            .iter()
            .all(|value| is_safe_id(value))
        && entry.search_terms.iter().all(|value| is_safe_text(value))
        && entry.search_weight <= 1_000
}

fn adapt_android_catalogs(
    bus_root: Option<&Path>,
    approved_nodes: &BTreeSet<String>,
    now_ms: u64,
    adapted: &mut AdaptedSources,
) {
    let persist =
        bus_root.and_then(|root| mde_bus::persist::Persist::open(root.to_path_buf()).ok());
    for node in approved_nodes {
        let source_id = format!("android-catalog/{node}");
        let catalog_body = android_catalog_state_topic(node)
            .ok()
            .and_then(|topic| retained_body(persist.as_ref(), &topic));
        let cloud_body = retained_body(persist.as_ref(), &cloud_state_topic(node));
        adapt_android_body(
            node,
            &source_id,
            catalog_body.as_deref(),
            cloud_body.as_deref(),
            now_ms,
            adapted,
        );
    }
}

fn adapt_android_body(
    node: &str,
    source_id: &str,
    catalog_body: Option<&str>,
    cloud_body: Option<&str>,
    now_ms: u64,
    adapted: &mut AdaptedSources,
) {
    let Some(catalog_body) = catalog_body else {
        adapted.statuses.push(status(
            ResourceAdapterKind::AndroidCatalog,
            source_id,
            ResourceAdapterAvailability::Unavailable,
            0,
        ));
        return;
    };
    let catalog = if catalog_body.len() <= MAX_ANDROID_CATALOG_WIRE_BYTES {
        serde_json::from_str::<AndroidSignedCatalog>(catalog_body).ok()
    } else {
        None
    };
    let Some(catalog) = catalog else {
        adapted.statuses.push(status(
            ResourceAdapterKind::AndroidCatalog,
            source_id,
            ResourceAdapterAvailability::Malformed,
            0,
        ));
        return;
    };
    let valid = catalog.payload.validate().is_ok()
        && catalog.payload.image_manifest.validate_at(now_ms).is_ok()
        && catalog.payload.issued_at_unix_ms <= now_ms
        && catalog.payload.expires_at_unix_ms > now_ms
        && catalog.payload.app_policies.len() <= MAX_ANDROID_ROWS
        && is_safe_id(&catalog.signer_id)
        && is_lower_hex(&catalog.signature, 128);
    if !valid {
        adapted.statuses.push(status(
            ResourceAdapterKind::AndroidCatalog,
            source_id,
            ResourceAdapterAvailability::Malformed,
            0,
        ));
        return;
    }
    let Some(cloud_body) = cloud_body else {
        adapted.statuses.push(status(
            ResourceAdapterKind::AndroidCatalog,
            source_id,
            ResourceAdapterAvailability::Unavailable,
            0,
        ));
        return;
    };
    let workload_ids = match android_workload_ids(cloud_body, node, now_ms) {
        Ok(workload_ids) if !workload_ids.is_empty() => workload_ids,
        Ok(_) => {
            adapted.statuses.push(status(
                ResourceAdapterKind::AndroidCatalog,
                source_id,
                ResourceAdapterAvailability::Unavailable,
                0,
            ));
            return;
        }
        Err(()) => {
            adapted.statuses.push(status(
                ResourceAdapterKind::AndroidCatalog,
                source_id,
                ResourceAdapterAvailability::Malformed,
                0,
            ));
            return;
        }
    };
    if workload_ids
        .len()
        .saturating_mul(catalog.payload.app_policies.len())
        > MAX_ADAPTED_CARDS
    {
        adapted.statuses.push(status(
            ResourceAdapterKind::AndroidCatalog,
            source_id,
            ResourceAdapterAvailability::Malformed,
            0,
        ));
        return;
    }
    let expires = catalog
        .payload
        .expires_at_unix_ms
        .min(now_ms.saturating_add(SOURCE_TTL_MS));
    if expires.saturating_sub(now_ms) < 1_000 {
        adapted.statuses.push(status(
            ResourceAdapterKind::AndroidCatalog,
            source_id,
            ResourceAdapterAvailability::Stale,
            0,
        ));
        return;
    }
    // A signed image/catalog proves governed availability, not live guest
    // installation or launcher readiness. Keep cards inspectable but unknown.
    let cards = workload_ids
        .iter()
        .flat_map(|workload_id| {
            catalog.payload.app_policies.iter().map(move |policy| {
                let package = policy.app.package_id().as_str();
                let mut card = projection_card(
                    ResourceClass::Application,
                    format!("android-app/{node}/{workload_id}/{package}"),
                    policy.app.display_name().to_owned(),
                    format!(
                        "Signed Android catalog for workload {workload_id} on {node}; runtime readiness unknown"
                    ),
                    now_ms,
                    expires,
                    HealthStatus::Unknown,
                    None,
                    format!("android/{node}/{workload_id}/{package}"),
                    vec![ResourceOperatingRole::Client, ResourceOperatingRole::Loader],
                )?;
                card.actions = vec![android_start_action(now_ms, expires)];
                card.validate().map_err(|error| error.to_string())?;
                Ok(card)
            })
        })
        .collect::<Result<Vec<_>, String>>();
    admit_source_cards(
        ResourceAdapterKind::AndroidCatalog,
        source_id,
        ResourceAdapterAvailability::Available,
        cards,
        adapted,
    );
}

fn android_workload_ids(body: &str, node: &str, now_ms: u64) -> Result<Vec<String>, ()> {
    if body.len() > MAX_CLOUD_STATE_WIRE_BYTES {
        return Err(());
    }
    let state: CloudState = serde_json::from_str(body).map_err(|_| ())?;
    let published_at_ms = u64::try_from(state.published_at_ms).map_err(|_| ())?;
    if state.host != node
        || published_at_ms == 0
        || published_at_ms > now_ms
        || now_ms.saturating_sub(published_at_ms) >= SOURCE_TTL_MS
    {
        return Err(());
    }

    let mut workload_ids = BTreeSet::new();
    for workload in state
        .workloads
        .iter()
        .filter(|workload| workload.delivery_type == DeliveryType::AndroidVm)
    {
        if workload.node != node
            || !is_safe_id(&workload.name)
            || !workload_ids.insert(workload.name.clone())
        {
            return Err(());
        }
    }
    let inventory_ids = state
        .android_inventories
        .iter()
        .map(|inventory| inventory.workload_id.as_str())
        .collect::<BTreeSet<_>>();
    if inventory_ids.len() != state.android_inventories.len()
        || workload_ids
            .iter()
            .any(|workload_id| !inventory_ids.contains(workload_id.as_str()))
    {
        return Err(());
    }
    Ok(workload_ids.into_iter().collect())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedMediaState {
    node: String,
    sources: Vec<RetainedMediaSource>,
    lanes: Vec<RetainedMediaLane>,
    published_at_ms: u64,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedMediaSource {
    id: String,
    name: String,
    node: String,
    kind: MediaKind,
    host: String,
    #[serde(default)]
    port: Option<u16>,
    endpoint: String,
    protocols: Vec<MediaProtocol>,
    origin: MediaSourceOrigin,
    reachability: MediaReachability,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    gateway_node: Option<String>,
    #[serde(default)]
    upstream_key: Option<String>,
    #[serde(default)]
    credential_ref: Option<String>,
    #[serde(default)]
    mesh_default: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedMediaLane {
    lane: String,
    status: String,
}

fn adapt_media(
    bus_root: Option<&Path>,
    publisher: &str,
    now_ms: u64,
    adapted: &mut AdaptedSources,
) {
    let persist =
        bus_root.and_then(|root| mde_bus::persist::Persist::open(root.to_path_buf()).ok());
    let body = retained_body(persist.as_ref(), MEDIA_SOURCES_TOPIC);
    adapt_media_body(publisher, body.as_deref(), now_ms, adapted);
}

fn adapt_media_body(
    publisher: &str,
    body: Option<&str>,
    now_ms: u64,
    adapted: &mut AdaptedSources,
) {
    let unavailable = |adapted: &mut AdaptedSources, availability| {
        adapted.statuses.push(status(
            ResourceAdapterKind::Media,
            "media-roster",
            availability,
            0,
        ));
        adapted.statuses.push(status(
            ResourceAdapterKind::FileShare,
            "media-roster/file-shares",
            availability,
            0,
        ));
    };
    let Some(body) = body else {
        unavailable(adapted, ResourceAdapterAvailability::Unavailable);
        return;
    };
    let state = if body.len() <= MAX_MEDIA_WIRE_BYTES {
        serde_json::from_str::<RetainedMediaState>(body).ok()
    } else {
        None
    };
    let Some(state) = state else {
        unavailable(adapted, ResourceAdapterAvailability::Malformed);
        return;
    };
    if state.node != publisher
        || state.published_at_ms == 0
        || state.published_at_ms > now_ms
        || state.sources.len() > MAX_MEDIA_ROWS
        || state.lanes.len() > MAX_MEDIA_LANES
        || !state.sources.iter().all(valid_media_source)
        || !state
            .lanes
            .iter()
            .all(|lane| is_safe_id(&lane.lane) && is_safe_text(&lane.status))
    {
        unavailable(adapted, ResourceAdapterAvailability::Malformed);
        return;
    }
    let is_stale = now_ms.saturating_sub(state.published_at_ms) >= SOURCE_TTL_MS;
    let expires = state.published_at_ms.saturating_add(SOURCE_TTL_MS);
    let conflicts = conflicting_media_keys(&state.sources);
    let mut media_cards = Vec::new();
    let mut share_cards = Vec::new();
    for source in &state.sources {
        if conflicts.contains(&media_resource_key(source)) {
            continue;
        }
        let (health, reason) = media_health(source.reachability, is_stale);
        let class = if source.kind == MediaKind::FileShare {
            ResourceClass::FileShare
        } else {
            ResourceClass::MediaServer
        };
        let prefix = if class == ResourceClass::FileShare {
            "file-share"
        } else {
            "media"
        };
        let display = if class == ResourceClass::FileShare {
            format!("File share on {}", source.node)
        } else {
            format!("{} media on {}", media_kind_label(source.kind), source.node)
        };
        let card = projection_card(
            class,
            format!("{prefix}/{}", source.id),
            display,
            format!("Admitted {} resource", media_kind_label(source.kind)),
            state.published_at_ms,
            expires,
            health,
            reason,
            format!("media/{}", source.id),
            vec![ResourceOperatingRole::Client],
        );
        match (class, card) {
            (ResourceClass::FileShare, Ok(card)) => share_cards.push(card),
            (_, Ok(card)) => media_cards.push(card),
            (_, Err(error)) => {
                unavailable(adapted, ResourceAdapterAvailability::Malformed);
                let _ = error;
                return;
            }
        }
    }
    let current_availability = if is_stale {
        ResourceAdapterAvailability::Stale
    } else {
        ResourceAdapterAvailability::Available
    };
    admit_source_cards(
        ResourceAdapterKind::Media,
        "media-roster",
        if conflicts
            .iter()
            .any(|(kind, _)| *kind == ResourceAdapterKind::Media)
        {
            ResourceAdapterAvailability::Conflict
        } else {
            current_availability
        },
        Ok(media_cards),
        adapted,
    );
    admit_source_cards(
        ResourceAdapterKind::FileShare,
        "media-roster/file-shares",
        if conflicts
            .iter()
            .any(|(kind, _)| *kind == ResourceAdapterKind::FileShare)
        {
            ResourceAdapterAvailability::Conflict
        } else {
            current_availability
        },
        Ok(share_cards),
        adapted,
    );
}

fn media_resource_key(source: &RetainedMediaSource) -> (ResourceAdapterKind, String) {
    let kind = if source.kind == MediaKind::FileShare {
        ResourceAdapterKind::FileShare
    } else {
        ResourceAdapterKind::Media
    };
    (kind, source.id.clone())
}

/// Detect equivocation before redaction can make conflicting raw rows look
/// like one identical catalog card. Exact duplicate observations are harmless;
/// non-identical rows claiming one stable projected identity are not.
fn conflicting_media_keys(
    sources: &[RetainedMediaSource],
) -> BTreeSet<(ResourceAdapterKind, String)> {
    let mut first = BTreeMap::<(ResourceAdapterKind, String), &RetainedMediaSource>::new();
    let mut conflicts = BTreeSet::new();
    for source in sources {
        let key = media_resource_key(source);
        match first.entry(key.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(source);
            }
            std::collections::btree_map::Entry::Occupied(entry) if *entry.get() != source => {
                conflicts.insert(key);
            }
            std::collections::btree_map::Entry::Occupied(_) => {}
        }
    }
    conflicts
}

fn valid_media_source(source: &RetainedMediaSource) -> bool {
    is_safe_id(&source.id)
        && is_safe_text(&source.name)
        && is_safe_id(&source.node)
        && is_safe_text(&source.host)
        && source.port != Some(0)
        && source.endpoint.len() <= MAX_SAFE_TEXT_BYTES
        && source.protocols.len() <= 8
        && source.protocols == MediaProtocol::for_kind(source.kind)
        && source
            .reason
            .as_ref()
            .is_none_or(|value| is_safe_text(value))
        && source
            .gateway_node
            .as_ref()
            .is_none_or(|value| is_safe_id(value))
        && source
            .upstream_key
            .as_ref()
            .is_none_or(|value| value.len() <= MAX_SAFE_TEXT_BYTES)
        && source
            .credential_ref
            .as_ref()
            .is_none_or(|value| value.len() <= MAX_SAFE_TEXT_BYTES)
        && match source.origin {
            MediaSourceOrigin::Gateway => source.gateway_node.is_some(),
            _ => source.gateway_node.is_none(),
        }
        && source
            .mesh_default
            .is_none_or(|_| source.origin == MediaSourceOrigin::Gateway)
}

fn media_health(
    reachability: MediaReachability,
    stale: bool,
) -> (HealthStatus, Option<FailureReason>) {
    if stale {
        return (
            HealthStatus::Stale,
            Some(failure(FailureCode::Stale, "media roster is stale")),
        );
    }
    match reachability {
        MediaReachability::Reachable => (HealthStatus::Available, None),
        MediaReachability::Unreachable => (
            HealthStatus::Unavailable,
            Some(failure(
                FailureCode::Unreachable,
                "media source is unavailable",
            )),
        ),
        MediaReachability::Unknown => (HealthStatus::Unknown, None),
    }
}

const fn media_kind_label(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Jellyfin => "Jellyfin",
        MediaKind::Dlna => "DLNA",
        MediaKind::MeshPlayer => "mesh player",
        MediaKind::FileShare => "file share",
    }
}

fn retained_body(persist: Option<&mde_bus::persist::Persist>, topic: &str) -> Option<String> {
    persist
        .and_then(|persist| persist.read_latest(topic).ok().flatten())
        .and_then(|message| message.body)
}

fn admit_source_cards(
    kind: ResourceAdapterKind,
    source_id: &str,
    availability: ResourceAdapterAvailability,
    cards: Result<Vec<ResourceCard>, String>,
    adapted: &mut AdaptedSources,
) {
    let Ok(cards) = cards else {
        adapted.statuses.push(status(
            kind,
            source_id,
            ResourceAdapterAvailability::Malformed,
            0,
        ));
        return;
    };
    if adapted.cards.len().saturating_add(cards.len()) > MAX_ADAPTED_CARDS {
        adapted.statuses.push(status(
            kind,
            source_id,
            ResourceAdapterAvailability::Malformed,
            0,
        ));
        return;
    }
    adapted
        .statuses
        .push(status(kind, source_id, availability, cards.len()));
    adapted.cards.extend(cards);
}

#[allow(clippy::too_many_arguments)]
fn projection_card(
    class: ResourceClass,
    resource_id: String,
    display_name: String,
    summary: String,
    observed: u64,
    expires: u64,
    health_status: HealthStatus,
    health_failure: Option<FailureReason>,
    source_id: String,
    operating_roles: Vec<ResourceOperatingRole>,
) -> Result<ResourceCard, String> {
    let card = ResourceCard {
        schema_version: RESOURCE_CONTRACT_VERSION,
        identity: mackes_mesh_types::resources::ResourceIdentity::new(
            class,
            IdentityAuthority::Provider,
            resource_id,
            vec![],
        )
        .map_err(|error| error.to_string())?,
        display_name,
        summary: Some(summary),
        first_seen_at_ms: observed,
        last_seen_at_ms: observed,
        expires_at_ms: expires,
        health: HealthState {
            schema_version: RESOURCE_CONTRACT_VERSION,
            status: health_status,
            observed_at_ms: observed,
            expires_at_ms: expires,
            latency_ms: None,
            failure: health_failure,
        },
        auth: open_mesh_auth(observed),
        provenance: vec![provenance(
            DiscoverySource::ProviderRegistry,
            source_id,
            observed,
            expires,
        )],
        transports: vec![],
        client_capabilities: vec![],
        actions: vec![],
        operating_roles,
        service: None,
    };
    card.validate().map_err(|error| error.to_string())?;
    Ok(card)
}

fn is_safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_SAFE_ID_BYTES
        && value.trim() == value
        && !value.contains("//")
        && !value.contains("..")
        && !value.contains("://")
        && !value.contains(['/', '\\'])
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '-' | '_' | '.' | ':' | '@' | '+')
        })
}

fn is_safe_text(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_SAFE_TEXT_BYTES
        && !value.chars().any(char::is_control)
}

fn is_sha256_digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|digest| is_lower_hex(digest, 64))
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn decode_workload_snapshot(
    body: &str,
    expected_node: &str,
    now_ms: u64,
) -> Result<WorkloadStateSnapshot, ()> {
    if body.len() > MAX_WORKLOAD_WIRE_BYTES {
        return Err(());
    }
    let snapshot: WorkloadStateSnapshot = serde_json::from_str(body).map_err(|_| ())?;
    if snapshot.node != expected_node
        || snapshot.observed_at_ms > now_ms
        || snapshot.validate(now_ms).is_err()
    {
        return Err(());
    }
    Ok(snapshot)
}

fn peer_card(peer: &PeerRecord, directory_current: bool, now_ms: u64) -> Result<ResourceCard, ()> {
    if peer.hostname.is_empty()
        || peer.hostname.len() > 255
        || peer.last_seen_ms == 0
        || peer.last_seen_ms > now_ms
    {
        return Err(());
    }
    let observed = peer.last_seen_ms;
    let expires = observed.saturating_add(SOURCE_TTL_MS);
    let stale = now_ms.saturating_sub(observed) >= SOURCE_TTL_MS;
    let (health, failure) = if !directory_current {
        (
            HealthStatus::Unavailable,
            Some(failure(
                FailureCode::Unreachable,
                "peer membership authority is unavailable",
            )),
        )
    } else if stale {
        (
            HealthStatus::Stale,
            Some(failure(
                FailureCode::Stale,
                "peer membership observation is stale",
            )),
        )
    } else {
        match peer.health.as_str() {
            "healthy" => (HealthStatus::Available, None),
            "degraded" => (
                HealthStatus::Degraded,
                Some(failure(FailureCode::Other, "peer reports degraded health")),
            ),
            "critical" | "unreachable" => (
                HealthStatus::Unavailable,
                Some(failure(FailureCode::Unreachable, "peer is unavailable")),
            ),
            "unknown" => (HealthStatus::Unknown, None),
            _ => (
                HealthStatus::Unavailable,
                Some(failure(
                    FailureCode::MalformedAdvertisement,
                    "peer health value was not admitted",
                )),
            ),
        }
    };
    let card = ResourceCard {
        schema_version: RESOURCE_CONTRACT_VERSION,
        identity: mackes_mesh_types::resources::ResourceIdentity::new(
            ResourceClass::Node,
            IdentityAuthority::Mesh,
            format!("node/{}", peer.hostname),
            vec![],
        )
        .map_err(|_| ())?,
        display_name: peer.hostname.clone(),
        summary: peer.role.as_ref().map(|role| format!("Mesh peer · {role}")),
        first_seen_at_ms: observed,
        last_seen_at_ms: observed,
        expires_at_ms: expires,
        health: HealthState {
            schema_version: RESOURCE_CONTRACT_VERSION,
            status: health,
            observed_at_ms: observed,
            expires_at_ms: expires,
            latency_ms: None,
            failure,
        },
        auth: open_mesh_auth(observed),
        provenance: vec![provenance(
            DiscoverySource::MeshDirectory,
            format!("peer/{}", peer.hostname),
            observed,
            expires,
        )],
        transports: vec![],
        client_capabilities: vec![],
        actions: vec![inspect_action(observed, expires)],
        operating_roles: vec![ResourceOperatingRole::Client, ResourceOperatingRole::Host],
        service: None,
    };
    card.validate().map_err(|_| ())?;
    Ok(card)
}

fn workload_card(
    node: &str,
    observed: u64,
    workload: &WorkloadOperationStatus,
    stale: bool,
) -> Result<ResourceCard, ()> {
    let expires = observed.saturating_add(SOURCE_TTL_MS);
    let (status, health_failure) = if stale {
        (
            HealthStatus::Stale,
            Some(failure(FailureCode::Stale, "workload snapshot is stale")),
        )
    } else {
        workload_health(workload)
    };
    let class = if workload.backend.is_vm() {
        ResourceClass::VirtualMachine
    } else {
        ResourceClass::Container
    };
    let card = ResourceCard {
        schema_version: RESOURCE_CONTRACT_VERSION,
        identity: mackes_mesh_types::resources::ResourceIdentity::new(
            class,
            IdentityAuthority::Mesh,
            format!("workload/{node}/{}", workload.workload_id.as_str()),
            vec![],
        )
        .map_err(|_| ())?,
        display_name: workload.workload_id.as_str().to_owned(),
        summary: Some(match class {
            ResourceClass::VirtualMachine => format!("Virtual machine on {node}"),
            _ => format!("Container on {node}"),
        }),
        first_seen_at_ms: observed,
        last_seen_at_ms: observed,
        expires_at_ms: expires,
        health: HealthState {
            schema_version: RESOURCE_CONTRACT_VERSION,
            status,
            observed_at_ms: observed,
            expires_at_ms: expires,
            latency_ms: None,
            failure: health_failure,
        },
        auth: open_mesh_auth(observed),
        provenance: vec![provenance(
            DiscoverySource::ProviderRegistry,
            format!("workload/{node}/{}", workload.workload_id.as_str()),
            observed,
            expires,
        )],
        transports: vec![],
        client_capabilities: vec![],
        actions: workload_actions(observed, expires, workload, stale),
        operating_roles: vec![ResourceOperatingRole::Client, ResourceOperatingRole::Loader],
        service: None,
    };
    card.validate().map_err(|_| ())?;
    Ok(card)
}

fn workload_health(workload: &WorkloadOperationStatus) -> (HealthStatus, Option<FailureReason>) {
    if workload.power == WorkloadPowerState::Failed
        || workload.readiness == WorkloadReadiness::Failed
    {
        return (
            HealthStatus::Unavailable,
            Some(failure(
                FailureCode::MissingProvider,
                "workload operation failed",
            )),
        );
    }
    match workload.readiness {
        WorkloadReadiness::Ready => (HealthStatus::Available, None),
        WorkloadReadiness::Degraded => (
            HealthStatus::Degraded,
            Some(failure(
                FailureCode::Other,
                "workload reports degraded readiness",
            )),
        ),
        WorkloadReadiness::Unavailable => (
            HealthStatus::Unavailable,
            Some(failure(FailureCode::Unreachable, "workload is unavailable")),
        ),
        _ if matches!(
            workload.power,
            WorkloadPowerState::Stopped | WorkloadPowerState::Defined
        ) =>
        {
            (
                HealthStatus::Unavailable,
                Some(failure(FailureCode::NotObserved, "workload is not running")),
            )
        }
        _ => (HealthStatus::Unknown, None),
    }
}

fn workload_actions(
    observed: u64,
    expires: u64,
    workload: &WorkloadOperationStatus,
    stale: bool,
) -> Vec<ResourceAction> {
    let mut actions = vec![inspect_action(observed, expires)];
    if stale || workload.power == WorkloadPowerState::Failed {
        return actions;
    }

    // Each routed operation is cancellable only after the Workload authority
    // accepts that exact request.  Bind the advertised action to the observed
    // generation so a refreshed card cannot silently reuse an older control.
    let verb = match (workload.phase, workload.power, workload.readiness) {
        (
            WorkloadOperationPhase::Completed | WorkloadOperationPhase::Cancelled,
            WorkloadPowerState::Defined | WorkloadPowerState::Stopped,
            _,
        ) => ResourceActionVerb::Start,
        (
            WorkloadOperationPhase::Ready,
            WorkloadPowerState::Paused,
            WorkloadReadiness::Ready | WorkloadReadiness::Degraded,
        ) => ResourceActionVerb::Resume,
        (
            WorkloadOperationPhase::Ready,
            WorkloadPowerState::Running,
            WorkloadReadiness::Ready | WorkloadReadiness::Degraded,
        ) => ResourceActionVerb::Launch,
        _ => return actions,
    };
    let action_id = match verb {
        ResourceActionVerb::Start => format!("start-g{}", workload.generation),
        ResourceActionVerb::Resume => format!("resume-g{}", workload.generation),
        ResourceActionVerb::Launch => format!("launch-g{}", workload.generation),
        _ => return actions,
    };
    actions.push(ResourceAction {
        schema_version: RESOURCE_CONTRACT_VERSION,
        action_id,
        verb,
        target: ResourceActionTarget::Resource,
        availability: ActionAvailability {
            status: ActionAvailabilityStatus::Ready,
            failure: None,
        },
        issued_at_ms: observed,
        expires_at_ms: expires,
    });
    actions
}

fn merge_catalog(
    mut catalog: ResourceCatalog,
    mut adapted: AdaptedSources,
    now_ms: u64,
) -> Result<(ResourceCatalog, ResourceAdapterStatusProjection), String> {
    if adapted.cards.len() > MAX_ADAPTED_CARDS {
        return Err("adapted resource snapshot exceeds 1024 cards".into());
    }
    catalog.validate().map_err(|error| error.to_string())?;
    for card in &adapted.cards {
        card.validate().map_err(|error| error.to_string())?;
    }
    catalog.cards.append(&mut adapted.cards);
    let mut groups = BTreeMap::<String, Vec<ResourceCard>>::new();
    for card in std::mem::take(&mut catalog.cards) {
        groups
            .entry(card.resource_id().to_owned())
            .or_default()
            .push(card);
    }
    for (resource_id, mut cards) in groups {
        cards.sort_by_key(|card| serde_json::to_string(card).unwrap_or_default());
        cards.dedup();
        if cards.len() == 1 {
            catalog.cards.push(cards.pop().expect("one card remains"));
        } else {
            adapted.statuses.push(status(
                ResourceAdapterKind::Deduplication,
                &resource_id,
                ResourceAdapterAvailability::Conflict,
                1,
            ));
            catalog.cards.push(conflict_card(cards)?);
        }
    }
    catalog.content_digest = None;
    let catalog = catalog
        .with_content_digest()
        .map_err(|error| error.to_string())?;
    adapted.statuses.sort_by(|left, right| {
        (left.source, &left.source_id).cmp(&(right.source, &right.source_id))
    });
    let projection = ResourceAdapterStatusProjection {
        schema_version: RESOURCE_CONTRACT_VERSION,
        catalog_revision: catalog.revision.clone(),
        observed_at_ms: now_ms,
        sources: adapted.statuses,
    };
    Ok((catalog, projection))
}

fn conflict_card(cards: Vec<ResourceCard>) -> Result<ResourceCard, String> {
    let mut card = cards
        .first()
        .cloned()
        .ok_or_else(|| "empty conflict group".to_owned())?;
    card.first_seen_at_ms = cards
        .iter()
        .map(|candidate| candidate.first_seen_at_ms)
        .min()
        .unwrap_or(card.first_seen_at_ms);
    card.last_seen_at_ms = cards
        .iter()
        .map(|candidate| candidate.last_seen_at_ms)
        .max()
        .unwrap_or(card.last_seen_at_ms);
    card.expires_at_ms = cards
        .iter()
        .map(|candidate| candidate.expires_at_ms)
        .max()
        .unwrap_or(card.expires_at_ms);
    card.summary = Some("Conflicting approved source observations".into());
    card.health = HealthState {
        schema_version: RESOURCE_CONTRACT_VERSION,
        status: HealthStatus::Unavailable,
        observed_at_ms: card.last_seen_at_ms,
        expires_at_ms: card.expires_at_ms,
        latency_ms: None,
        failure: Some(failure(
            FailureCode::MalformedAdvertisement,
            "stable resource identity has conflicting source observations",
        )),
    };
    card.auth = open_mesh_auth(card.last_seen_at_ms);
    card.provenance = cards
        .iter()
        .flat_map(|candidate| candidate.provenance.iter().cloned())
        .collect();
    card.provenance.sort_by(|left, right| {
        (
            left.source,
            left.scope,
            &left.source_id,
            left.trust,
            left.observed_at_ms,
        )
            .cmp(&(
                right.source,
                right.scope,
                &right.source_id,
                right.trust,
                right.observed_at_ms,
            ))
    });
    card.provenance.dedup();
    card.transports.clear();
    card.client_capabilities.clear();
    card.actions.clear();
    card.service = None;
    card.validate().map_err(|error| error.to_string())?;
    Ok(card)
}

fn open_mesh_auth(observed: u64) -> AuthState {
    AuthState {
        schema_version: RESOURCE_CONTRACT_VERSION,
        status: AuthStatus::NotRequired,
        accepted_methods: vec![],
        active_method: None,
        credential_ref: None,
        updated_at_ms: observed,
        expires_at_ms: None,
        failure: None,
    }
}

fn provenance(
    source: DiscoverySource,
    source_id: String,
    observed: u64,
    expires: u64,
) -> SourceProvenance {
    SourceProvenance {
        schema_version: RESOURCE_CONTRACT_VERSION,
        source,
        source_id,
        scope: ResourceScope::Mesh,
        trust: ProvenanceTrust::AuthenticatedMesh,
        interface: None,
        observed_at_ms: observed,
        expires_at_ms: expires,
    }
}

fn inspect_action(observed: u64, expires: u64) -> ResourceAction {
    ResourceAction {
        schema_version: RESOURCE_CONTRACT_VERSION,
        action_id: "inspect".into(),
        verb: ResourceActionVerb::Inspect,
        target: ResourceActionTarget::Resource,
        availability: ActionAvailability {
            status: ActionAvailabilityStatus::Ready,
            failure: None,
        },
        issued_at_ms: observed,
        expires_at_ms: expires,
    }
}

fn android_start_action(observed: u64, expires: u64) -> ResourceAction {
    ResourceAction {
        schema_version: RESOURCE_CONTRACT_VERSION,
        action_id: "start".into(),
        verb: ResourceActionVerb::Start,
        target: ResourceActionTarget::Resource,
        availability: ActionAvailability {
            // A signed catalog proves that this app/workload pairing is
            // governed, but it does not prove that the guest is booted and
            // launcher-ready. Keep Start visible as evidence while making
            // the browser's executable-action projection truthful.
            status: ActionAvailabilityStatus::Unavailable,
            failure: Some(failure(
                FailureCode::NotObserved,
                "Android guest readiness has not been observed",
            )),
        },
        issued_at_ms: observed,
        expires_at_ms: expires,
    }
}

fn failure(code: FailureCode, message: &'static str) -> FailureReason {
    FailureReason {
        code,
        message: message.into(),
    }
}

fn status(
    source: ResourceAdapterKind,
    source_id: &str,
    availability: ResourceAdapterAvailability,
    admitted_cards: usize,
) -> ResourceAdapterStatus {
    ResourceAdapterStatus {
        source,
        source_id: source_id.to_owned(),
        availability,
        admitted_cards: u16::try_from(admitted_cards).unwrap_or(u16::MAX),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use mackes_mesh_types::android_apps::{
        AndroidAppCapability, AndroidAppPermission, AndroidCatalogAppPolicy,
        AndroidCatalogGuestReadiness, AndroidCatalogPayload, AndroidImageManifest,
        AndroidImagePackage, AndroidImagePackageManifest, AndroidImageProvenance,
        AndroidPackageVersion, AndroidResourceClass, AndroidResourceProfile, AospStarterApp,
        AospStarterCatalog, ANDROID_SIGNED_CATALOG_SCHEMA_VERSION,
    };
    use mackes_mesh_types::media_sources::{
        LaneStatus as MediaLaneStatus, MediaSource, MediaSourcesState,
    };
    use mackes_mesh_types::workloads::{
        WorkloadBackend, WorkloadOperationPhase, WorkloadResources, WorkloadRuntimeSignals,
    };

    const NOW: u64 = 10_000_000;

    fn empty_catalog() -> ResourceCatalog {
        ResourceCatalog {
            schema_version: RESOURCE_CONTRACT_VERSION,
            revision: "seat193-1".into(),
            publisher: "seat193".into(),
            generated_at_ms: NOW,
            content_digest: None,
            cards: vec![],
        }
        .with_content_digest()
        .expect("catalog")
    }

    fn peer(host: &str) -> PeerRecord {
        PeerRecord {
            hostname: host.into(),
            mde_version: None,
            last_seen_ms: NOW - 1_000,
            health: "healthy".into(),
            descriptors: None,
            overlay_ip: None,
            role: Some("workstation".into()),
            external_addr: None,
            media: false,
        }
    }

    fn workload(id: &str) -> WorkloadOperationStatus {
        WorkloadOperationStatus {
            schema_version: 1,
            request_id: format!("request-{id}"),
            workload_id: mackes_mesh_types::workloads::WorkloadId::new(id).expect("id"),
            backend: WorkloadBackend::LibvirtVirtqemud,
            resources: WorkloadResources {
                vcpu: 2,
                memory_mb: 4_096,
                disk_gb: 32,
            },
            image_ref: None,
            generation: 1,
            phase: WorkloadOperationPhase::Ready,
            power: WorkloadPowerState::Running,
            readiness: WorkloadReadiness::Ready,
            signals: WorkloadRuntimeSignals::default(),
            retryable: false,
            attempt: 0,
            next_retry_at_ms: 0,
            reason: None,
            remediation: None,
            attachment: None,
        }
    }

    #[test]
    fn ambiguous_peer_identity_cannot_authorize_downstream_resource_reads() {
        let first = peer("alpha");
        let mut conflicting = first.clone();
        conflicting.last_seen_ms += 1;
        conflicting.health = "degraded".into();

        let approved = approved_nodes_for_resources(
            &[first.clone(), conflicting],
            "seat193",
            true,
            NOW,
        )
        .expect("safe peer rows remain a valid directory input");
        assert!(!approved.contains("alpha"));
        assert!(approved.contains("seat193"));

        let exact_duplicates =
            approved_nodes_for_resources(&[first.clone(), first], "seat193", true, NOW)
                .expect("exact duplicate rows are safe to collapse");
        assert!(exact_duplicates.contains("alpha"));
    }

    #[test]
    fn stale_or_unavailable_peer_directory_cannot_authorize_downstream_resource_reads() {
        let current = peer("current");
        let mut stale = peer("stale");
        stale.last_seen_ms = NOW - SOURCE_TTL_MS;
        let mut future = peer("future");
        future.last_seen_ms = NOW + 1;

        let approved = approved_nodes_for_resources(
            &[current.clone(), stale, future],
            "seat193",
            true,
            NOW,
        )
        .expect("bounded directory");
        assert_eq!(
            approved,
            BTreeSet::from(["current".to_owned(), "seat193".to_owned()])
        );

        let unavailable =
            approved_nodes_for_resources(&[current], "seat193", false, NOW)
                .expect("bounded fallback directory");
        assert_eq!(unavailable, BTreeSet::from(["seat193".to_owned()]));
    }

    #[test]
    fn malformed_peer_projection_cannot_authorize_downstream_resource_reads() {
        let mut malformed = peer("hostile");
        malformed.role = Some("workstation\nforged-authority".into());

        let approved = approved_nodes_for_resources(
            &[malformed],
            "seat193",
            true,
            NOW,
        )
        .expect("bounded directory");

        assert_eq!(approved, BTreeSet::from(["seat193".to_owned()]));
    }

    #[test]
    fn stale_peer_heartbeat_cannot_fabricate_current_resource_health() {
        let mut stale = peer("stale");
        stale.last_seen_ms = NOW - SOURCE_TTL_MS;
        stale.health = "healthy".into();

        let card = peer_card(&stale, true, NOW).expect("bounded stale peer card");

        assert_eq!(card.health.status, HealthStatus::Stale);
        assert_eq!(
            card.health.failure,
            Some(FailureReason {
                code: FailureCode::Stale,
                message: "peer membership observation is stale".into(),
            })
        );
    }

    fn app_projection(name: &str) -> AdmittedFlatpakCatalogProjection {
        AdmittedFlatpakCatalogProjection {
            schema_version: 1,
            host: "seat193".into(),
            catalog_id: "flatpak-production".into(),
            revision: 7,
            issued_at_unix_ms: NOW - 1_000,
            expires_at_unix_ms: NOW + 60_000,
            content_digest: format!("sha256:{}", "0".repeat(64)),
            provider_id: "flatpak-provider".into(),
            repository_id: "stable".into(),
            entries: vec![AdmittedFlatpakAppProjection {
                app_id: "org.example.Writer".into(),
                display_name: name.into(),
                summary: "Writer".into(),
                version: "1.0".into(),
                icon_id: "writer".into(),
                permissions: vec!["audio".into()],
                guest_profile: "wayland-standard".into(),
                supported_actions: vec!["launch".into()],
                search_terms: vec!["writer".into()],
                search_weight: 10,
            }],
        }
    }

    fn android_catalog() -> AndroidSignedCatalog {
        let image = AndroidImageManifest::new(
            "aosp-cuttlefish-2026-08",
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "aosp-source-2026-08",
            "starter-catalog-v1",
            NOW - 3_000,
            NOW - 2_000,
            AospStarterCatalog::v1(),
        )
        .unwrap();
        let provenance = AndroidImageProvenance::from_manifest(&image).unwrap();
        let packages = AospStarterApp::ALL
            .into_iter()
            .map(|app| {
                AndroidImagePackage::for_app(
                    app,
                    AndroidPackageVersion::new("2026.08.1", 1).unwrap(),
                )
            })
            .collect();
        let package_manifest = AndroidImagePackageManifest::new(provenance, packages).unwrap();
        let app_policies = AospStarterApp::ALL
            .into_iter()
            .map(|app| AndroidCatalogAppPolicy {
                app,
                permissions: vec![AndroidAppPermission::Network],
                capabilities: vec![
                    AndroidAppCapability::VdiDisplay,
                    AndroidAppCapability::AudioPlayback,
                ],
                resources: AndroidResourceProfile {
                    class: AndroidResourceClass::Standard,
                    vcpus: 4,
                    memory_mib: 4_096,
                    disk_mib: 16_384,
                },
                guest_readiness: AndroidCatalogGuestReadiness::BootedInventoryAndLauncherReady,
            })
            .collect();
        AndroidSignedCatalog::sign(
            "android-release-v1",
            AndroidCatalogPayload {
                schema_version: ANDROID_SIGNED_CATALOG_SCHEMA_VERSION,
                catalog_id: "aosp-starter-production".into(),
                revision: 7,
                issued_at_unix_ms: NOW - 1_000,
                expires_at_unix_ms: NOW + 60_000,
                image_manifest: image,
                package_manifest,
                app_policies,
            },
            &SigningKey::from_bytes(&[7; 32]),
        )
        .unwrap()
    }

    fn android_cloud_state(workloads: &[&str]) -> String {
        let rows = workloads
            .iter()
            .map(|workload_id| {
                serde_json::json!({
                    "name": workload_id,
                    "delivery_type": "android_vm",
                    "node": "seat193",
                    "status": "running",
                    "cpu_pct": 0,
                    "mem_mb": 0,
                    "disk_gb": 0,
                    "reachable": true,
                    "drift": "in_sync"
                })
            })
            .collect::<Vec<_>>();
        let inventories = workloads
            .iter()
            .map(|workload_id| {
                mackes_mesh_types::android_apps::AndroidAppInventory::pending(*workload_id)
            })
            .collect::<Vec<_>>();
        serde_json::to_string(&serde_json::json!({
            "host": "seat193",
            "role": "workstation",
            "adapter": "construct_cloud",
            "health": [],
            "resources": [],
            "apply_armed": true,
            "published_at_ms": NOW - 1_000,
            "workloads": rows,
            "drift_summary": {"drift_count": 0, "last_plan_ms": 0},
            "node_capacity": {
                "vcpu_total": 0,
                "vcpu_used": 0,
                "mem_total_mb": 0,
                "mem_used_mb": 0
            },
            "android_inventories": inventories,
            "android_provider_admissions": [],
            "android_vdi_sources": []
        }))
        .unwrap()
    }

    fn media_state(kind: MediaKind) -> MediaSourcesState {
        let (id, protocols, endpoint) = match kind {
            MediaKind::FileShare => (
                "file-share:oak",
                vec![MediaProtocol::MeshFs],
                "mesh-fs://oak.mesh/private/path",
            ),
            _ => (
                "jellyfin:oak:8096",
                vec![MediaProtocol::Jellyfin],
                "http://oak.mesh:8096/secret-path",
            ),
        };
        MediaSourcesState {
            node: "seat193".into(),
            sources: vec![MediaSource {
                id: id.into(),
                name: "Do not copy this source label".into(),
                node: "oak".into(),
                kind,
                host: "oak.mesh".into(),
                port: (kind != MediaKind::FileShare).then_some(8096),
                endpoint: endpoint.into(),
                protocols,
                origin: MediaSourceOrigin::MeshPeer,
                reachability: MediaReachability::Reachable,
                reason: None,
                gateway_node: None,
                upstream_key: None,
                credential_ref: Some("secret/media-token".into()),
                mesh_default: None,
            }],
            lanes: vec![MediaLaneStatus {
                lane: "mesh-registry".into(),
                status: "ok".into(),
            }],
            published_at_ms: NOW - 1_000,
        }
    }

    fn assert_deterministic_conflict(
        kind: ResourceAdapterKind,
        mut adapted: AdaptedSources,
    ) -> (ResourceCatalog, ResourceAdapterStatusProjection) {
        assert!(adapted.statuses.iter().any(|row| {
            row.source == kind && row.availability == ResourceAdapterAvailability::Available
        }));
        let mut conflicting = adapted.cards[0].clone();
        conflicting.display_name.push_str(" conflict");
        conflicting.validate().unwrap();
        adapted.cards.push(conflicting.clone());
        let reversed = AdaptedSources {
            cards: adapted.cards.iter().cloned().rev().collect(),
            statuses: adapted.statuses.clone(),
        };
        let (catalog, projection) = merge_catalog(empty_catalog(), adapted, NOW).unwrap();
        let (reversed_catalog, reversed_projection) =
            merge_catalog(empty_catalog(), reversed, NOW).unwrap();
        assert_eq!(catalog, reversed_catalog);
        assert_eq!(projection, reversed_projection);
        assert!(projection.sources.iter().any(|row| {
            row.source == ResourceAdapterKind::Deduplication
                && row.availability == ResourceAdapterAvailability::Conflict
        }));
        assert!(catalog.cards.iter().any(|card| {
            card.health.status == HealthStatus::Unavailable && card.actions.is_empty()
        }));
        (catalog, projection)
    }

    #[test]
    fn approved_peer_and_workload_cards_are_bounded_and_stably_ordered() {
        let mut adapted = AdaptedSources::default();
        adapt_peers(&[peer("zeta"), peer("alpha")], true, NOW, &mut adapted);
        adapted
            .cards
            .push(workload_card("alpha", NOW - 1_000, &workload("desktop"), false).unwrap());
        let (first, _) = merge_catalog(empty_catalog(), adapted, NOW).expect("merge");

        let mut reversed = AdaptedSources::default();
        reversed
            .cards
            .push(workload_card("alpha", NOW - 1_000, &workload("desktop"), false).unwrap());
        adapt_peers(&[peer("alpha"), peer("zeta")], true, NOW, &mut reversed);
        let (second, _) = merge_catalog(empty_catalog(), reversed, NOW).expect("merge");

        assert_eq!(first.cards, second.cards);
        assert_eq!(first.content_digest, second.content_digest);
        assert_eq!(first.cards.len(), 3);
    }

    #[test]
    fn workload_cards_advertise_generation_bound_cancellable_routes() {
        let mut running = workload("desktop");
        running.generation = 7;
        let running_card = workload_card("alpha", NOW - 1_000, &running, false).unwrap();
        assert_eq!(
            running_card.actions,
            vec![
                inspect_action(NOW - 1_000, NOW - 1_000 + SOURCE_TTL_MS),
                ResourceAction {
                    schema_version: RESOURCE_CONTRACT_VERSION,
                    action_id: "launch-g7".into(),
                    verb: ResourceActionVerb::Launch,
                    target: ResourceActionTarget::Resource,
                    availability: ActionAvailability {
                        status: ActionAvailabilityStatus::Ready,
                        failure: None,
                    },
                    issued_at_ms: NOW - 1_000,
                    expires_at_ms: NOW - 1_000 + SOURCE_TTL_MS,
                },
            ]
        );

        let mut paused = running.clone();
        paused.generation = 8;
        paused.power = WorkloadPowerState::Paused;
        let paused_card = workload_card("alpha", NOW - 1_000, &paused, false).unwrap();
        assert_eq!(paused_card.actions[1].action_id, "resume-g8");
        assert_eq!(paused_card.actions[1].verb, ResourceActionVerb::Resume);

        let mut stopped = running;
        stopped.generation = 9;
        stopped.phase = WorkloadOperationPhase::Completed;
        stopped.power = WorkloadPowerState::Stopped;
        stopped.readiness = WorkloadReadiness::Unavailable;
        let stopped_card = workload_card("alpha", NOW - 1_000, &stopped, false).unwrap();
        assert_eq!(stopped_card.actions[1].action_id, "start-g9");
        assert_eq!(stopped_card.actions[1].verb, ResourceActionVerb::Start);
    }

    #[test]
    fn workload_cards_fail_closed_when_state_is_not_safely_actionable() {
        let mut transitional = workload("desktop");
        transitional.phase = WorkloadOperationPhase::Starting;
        transitional.power = WorkloadPowerState::Starting;
        transitional.readiness = WorkloadReadiness::WaitingForGuest;
        let transitional_card = workload_card("alpha", NOW - 1_000, &transitional, false).unwrap();
        assert_eq!(
            transitional_card.actions,
            vec![inspect_action(NOW - 1_000, NOW - 1_000 + SOURCE_TTL_MS)]
        );

        let mut failed = workload("desktop");
        failed.phase = WorkloadOperationPhase::Failed;
        failed.power = WorkloadPowerState::Failed;
        failed.readiness = WorkloadReadiness::Failed;
        failed.reason = Some("provider failed".into());
        let failed_card = workload_card("alpha", NOW - 1_000, &failed, false).unwrap();
        assert_eq!(failed_card.actions.len(), 1);

        let stale_card = workload_card("alpha", NOW - 1_000, &workload("desktop"), true).unwrap();
        assert_eq!(stale_card.actions.len(), 1);

        let mut contradictory = workload("desktop");
        contradictory.power = WorkloadPowerState::Stopped;
        let contradictory_card =
            workload_card("alpha", NOW - 1_000, &contradictory, false).unwrap();
        assert_eq!(contradictory_card.actions.len(), 1);
    }

    #[test]
    fn conflicting_identity_yields_one_unavailable_card_and_visible_status() {
        let first = peer_card(&peer("alpha"), true, NOW).unwrap();
        let mut conflicting = first.clone();
        conflicting.display_name = "Alpha workstation".into();
        conflicting.validate().unwrap();
        let adapted = AdaptedSources {
            cards: vec![conflicting.clone(), first.clone()],
            statuses: vec![],
        };
        let (catalog, projection) = merge_catalog(empty_catalog(), adapted, NOW).expect("merge");
        let reversed = AdaptedSources {
            cards: vec![first, conflicting],
            statuses: vec![],
        };
        let (reversed_catalog, _) =
            merge_catalog(empty_catalog(), reversed, NOW).expect("reversed merge");

        assert_eq!(catalog.cards.len(), 1);
        assert_eq!(catalog.cards, reversed_catalog.cards);
        assert_eq!(catalog.content_digest, reversed_catalog.content_digest);
        assert_eq!(catalog.cards[0].health.status, HealthStatus::Unavailable);
        assert!(catalog.cards[0].actions.is_empty());
        assert_eq!(projection.sources.len(), 1);
        assert_eq!(
            projection.sources[0].availability,
            ResourceAdapterAvailability::Conflict
        );
    }

    #[test]
    fn unavailable_authority_and_stale_workload_remain_explicit() {
        let mut adapted = AdaptedSources::default();
        adapt_peers(&[peer("alpha")], false, NOW, &mut adapted);
        let stale_observed = NOW - SOURCE_TTL_MS;
        adapted
            .cards
            .push(workload_card("alpha", stale_observed, &workload("desktop"), true).unwrap());
        adapted.statuses.push(status(
            ResourceAdapterKind::Workload,
            "workload/alpha",
            ResourceAdapterAvailability::Stale,
            1,
        ));
        let (catalog, projection) = merge_catalog(empty_catalog(), adapted, NOW).expect("merge");

        assert_eq!(catalog.cards.len(), 2);
        assert!(catalog.cards.iter().all(|card| matches!(
            card.health.status,
            HealthStatus::Unavailable | HealthStatus::Stale
        )));
        assert!(projection.sources.iter().any(|source| {
            source.source == ResourceAdapterKind::PeerDirectory
                && source.availability == ResourceAdapterAvailability::Unavailable
        }));
        assert!(projection.sources.iter().any(|source| {
            source.source == ResourceAdapterKind::Workload
                && source.availability == ResourceAdapterAvailability::Stale
        }));

        let peers = (0..=MAX_PEER_ROWS)
            .map(|index| peer(&format!("peer-{index}")))
            .collect::<Vec<_>>();
        let mut oversized = AdaptedSources::default();
        adapt_peers(&peers, true, NOW, &mut oversized);
        assert!(oversized.cards.is_empty());
        assert_eq!(
            oversized.statuses[0].availability,
            ResourceAdapterAvailability::Malformed
        );
        refuse_invalid_approved_nodes(&mut oversized);
        assert!(oversized.statuses.iter().any(|row| {
            row.source == ResourceAdapterKind::AppVmCatalog
                && row.availability == ResourceAdapterAvailability::Malformed
        }));
        assert!(oversized.statuses.iter().any(|row| {
            row.source == ResourceAdapterKind::AndroidCatalog
                && row.availability == ResourceAdapterAvailability::Malformed
        }));
    }

    #[test]
    fn app_vm_catalog_merge_conflict_and_status_are_deterministic() {
        let mut adapted = AdaptedSources::default();
        let body = serde_json::to_string(&app_projection("Writer")).unwrap();
        adapt_app_vm_body(
            "seat193",
            "app-vm-catalog/seat193",
            Some(&body),
            NOW,
            &mut adapted,
        );
        let (catalog, _) =
            assert_deterministic_conflict(ResourceAdapterKind::AppVmCatalog, adapted);
        let wire = serde_json::to_string(&catalog).unwrap();
        assert!(!wire.contains("wayland-standard"));
        assert!(!wire.contains("launch"));
    }

    #[test]
    fn android_catalog_merge_conflict_and_status_are_deterministic() {
        let mut adapted = AdaptedSources::default();
        let body = serde_json::to_string(&android_catalog()).unwrap();
        let cloud = android_cloud_state(&["android-vm-a"]);
        adapt_android_body(
            "seat193",
            "android-catalog/seat193",
            Some(&body),
            Some(&cloud),
            NOW,
            &mut adapted,
        );
        let (catalog, _) =
            assert_deterministic_conflict(ResourceAdapterKind::AndroidCatalog, adapted);
        assert!(catalog.cards.iter().any(|card| {
            card.health.status == HealthStatus::Unavailable && card.actions.is_empty()
        }));
        assert!(catalog
            .cards
            .iter()
            .filter(|card| { card.health.status != HealthStatus::Unavailable })
            .all(|card| card.actions == vec![android_start_action(NOW, NOW + 60_000)]));
    }

    #[test]
    fn android_catalog_cards_bind_exact_workload_and_gate_unobserved_start_action() {
        let mut adapted = AdaptedSources::default();
        let body = serde_json::to_string(&android_catalog()).unwrap();
        let cloud = android_cloud_state(&["android-vm-b", "android-vm-a"]);
        adapt_android_body(
            "seat193",
            "android-catalog/seat193",
            Some(&body),
            Some(&cloud),
            NOW,
            &mut adapted,
        );

        assert_eq!(adapted.cards.len(), AospStarterApp::ALL.len() * 2);
        assert!(adapted.cards.iter().all(|card| {
            card.identity.canonical_key.split('/').count() == 4
                && card
                    .identity
                    .canonical_key
                    .starts_with("android-app/seat193/android-vm-")
                && card.actions
                    == vec![ResourceAction {
                        schema_version: RESOURCE_CONTRACT_VERSION,
                        action_id: "start".into(),
                        verb: ResourceActionVerb::Start,
                        target: ResourceActionTarget::Resource,
                        availability: ActionAvailability {
                            status: ActionAvailabilityStatus::Unavailable,
                            failure: Some(FailureReason {
                                code: FailureCode::NotObserved,
                                message: "Android guest readiness has not been observed".into(),
                            }),
                        },
                        issued_at_ms: NOW,
                        expires_at_ms: NOW + 60_000,
                    }]
        }));
        assert!(!serde_json::to_string(&adapted.cards)
            .unwrap()
            .contains("android-app/seat193/com.android"));
    }

    #[test]
    fn android_catalog_rejects_legacy_or_ambiguous_workload_binding() {
        let body = serde_json::to_string(&android_catalog()).unwrap();
        for cloud in [
            android_cloud_state(&[]),
            android_cloud_state(&["android-vm-a", "android-vm-a"]),
            android_cloud_state(&["../android-vm-a"]),
        ] {
            let mut adapted = AdaptedSources::default();
            adapt_android_body(
                "seat193",
                "android-catalog/seat193",
                Some(&body),
                Some(&cloud),
                NOW,
                &mut adapted,
            );
            assert!(adapted.cards.is_empty());
            assert!(matches!(
                adapted.statuses[0].availability,
                ResourceAdapterAvailability::Unavailable | ResourceAdapterAvailability::Malformed
            ));
        }
    }

    #[test]
    fn media_merge_conflict_and_status_are_deterministic_without_locator_leakage() {
        let mut adapted = AdaptedSources::default();
        let body = serde_json::to_string(&media_state(MediaKind::Jellyfin)).unwrap();
        adapt_media_body("seat193", Some(&body), NOW, &mut adapted);
        let (catalog, projection) =
            assert_deterministic_conflict(ResourceAdapterKind::Media, adapted);
        assert!(projection.sources.iter().any(|row| {
            row.source == ResourceAdapterKind::FileShare && row.admitted_cards == 0
        }));
        let wire = serde_json::to_string(&catalog).unwrap();
        assert!(!wire.contains("secret-path"));
        assert!(!wire.contains("secret/media-token"));
        assert!(!wire.contains("Do not copy"));
    }

    #[test]
    fn media_raw_stable_id_equivocation_is_visible_before_redacted_card_deduplication() {
        let mut state = media_state(MediaKind::Jellyfin);
        let mut conflicting = state.sources[0].clone();
        conflicting.endpoint = "http://hostile.mesh:8096/other-secret".into();
        let mut independent = state.sources[0].clone();
        independent.id = "jellyfin:birch:8096".into();
        independent.node = "birch".into();
        state.sources.extend([conflicting, independent]);

        let mut adapted = AdaptedSources::default();
        let body = serde_json::to_string(&state).unwrap();
        adapt_media_body("seat193", Some(&body), NOW, &mut adapted);

        let media_status = adapted
            .statuses
            .iter()
            .find(|row| row.source == ResourceAdapterKind::Media)
            .expect("media status");
        assert_eq!(
            media_status.availability,
            ResourceAdapterAvailability::Conflict
        );
        assert_eq!(media_status.admitted_cards, 1);
        assert_eq!(adapted.cards.len(), 1);
        assert_eq!(
            adapted.cards[0].resource_id(),
            "media/jellyfin:birch:8096"
        );
        let wire = serde_json::to_string(&adapted.cards).unwrap();
        assert!(!wire.contains("jellyfin:oak:8096"));
        assert!(!wire.contains("other-secret"));
    }

    #[test]
    fn file_share_merge_conflict_and_status_are_deterministic_without_path_leakage() {
        let mut adapted = AdaptedSources::default();
        let body = serde_json::to_string(&media_state(MediaKind::FileShare)).unwrap();
        adapt_media_body("seat193", Some(&body), NOW, &mut adapted);
        let (catalog, _) = assert_deterministic_conflict(ResourceAdapterKind::FileShare, adapted);
        let wire = serde_json::to_string(&catalog).unwrap();
        assert!(!wire.contains("private/path"));
        assert!(!wire.contains("secret/media-token"));
    }

    #[test]
    fn absent_named_sources_are_unavailable_not_empty_success() {
        let mut adapted = AdaptedSources::default();
        adapt_app_vm_body("seat193", "app-vm-catalog/seat193", None, NOW, &mut adapted);
        adapt_android_body(
            "seat193",
            "android-catalog/seat193",
            None,
            None,
            NOW,
            &mut adapted,
        );
        adapt_media_body("seat193", None, NOW, &mut adapted);
        assert_eq!(adapted.cards.len(), 0);
        assert_eq!(adapted.statuses.len(), 4);
        assert!(adapted.statuses.iter().all(|row| {
            row.availability == ResourceAdapterAvailability::Unavailable && row.admitted_cards == 0
        }));
    }
}
