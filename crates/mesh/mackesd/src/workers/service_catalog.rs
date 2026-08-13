//! First-class universal service-card projection.
//!
//! This adapter turns the existing unified service inventory into the versioned
//! resource catalog and adds operator-configurable provider adapters even before
//! they are configured. The shell therefore renders one generic card per
//! service, never one special-case screen per provider.

#![cfg(feature = "async-services")]

use mackes_mesh_types::android_apps::AospStarterApp;
use mackes_mesh_types::resources::*;
use mackes_mesh_types::service_record::{
    ServiceHealth, ServiceProvenance, ServiceRecord, ServicesState,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read as _, Write as _};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::desktop_sources::{
    resource_card_from_desktop_source, DesktopProtocol, DesktopSource, DesktopSourcesState,
    ProtocolOffer, Reachability, SourceOrigin,
};
use super::ssh_x11_sources::{append_ssh_x11_cards, SshX11SourcesState};
use super::upnp_sources::{append_upnp_cards, UpnpSourcesState};

const FRESH_MS: u64 = 60_000;
const CARD_MS: u64 = 120_000;
// Probe-only service records remain authoritative for the service aggregator's
// five-minute TTL. Keep RDP promotion on that same boundary: a full bounded LAN
// scan can take longer than CARD_MS, and dropping the typed card earlier makes
// it disappear between otherwise healthy inventory publications.
const PROBED_RDP_MAX_AGE_MS: u64 = 300_000;
// The service aggregator applies the same five-minute lease before marking a
// retained row stale. Revalidate it here because a replayed or inconsistent
// ServicesState must not regain Available health merely by being republished.
const SERVICE_RECORD_MAX_AGE_MS: u64 = 300_000;
// Retained desktop-source cards use the same five-minute freshness lease as
// the desktop-source adapter. Do not let a delayed or replayed roster revive
// an RDP endpoint after that lease has elapsed.
const DESKTOP_ROSTER_MAX_AGE_MS: u64 = 300_000;
const SERVICE_CONFIG_VERSION: u16 = 1;
const MAX_CONFIGURATION_BYTES: u64 = 64 * 1024;
const MAX_HOSTNAME_BYTES: usize = 255;

/// Local, stdin-only submission consumed by `mackesd service-card save`.
/// Values are never published to the Bus or resource catalog. Secret fields
/// are separated and sealed before the non-secret desired-state record lands.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceConfigurationSubmission {
    /// Open registry key of the first-class service card.
    pub service_kind: String,
    /// Schema-admitted values. Secret entries are sealed before persistence.
    #[serde(default)]
    pub values: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct StoredServiceConfiguration {
    schema_version: u16,
    service_kind: String,
    non_secret_values: BTreeMap<String, String>,
    secret_fields: Vec<String>,
    credential_ref: String,
    enabled: bool,
    last_test_ok: Option<bool>,
    updated_at_ms: u64,
}

/// Safe result returned to the shell. It never contains submitted values.
#[derive(Debug, Serialize)]
pub struct ServiceCardResult {
    /// Service adapter that handled the operation.
    pub service_kind: String,
    /// Completed operation name.
    pub operation: &'static str,
    /// Whether the operation completed successfully.
    pub ok: bool,
    /// Bounded, non-secret result summary.
    pub detail: &'static str,
}

struct RegisteredService {
    kind: &'static str,
    name: &'static str,
    provider: &'static str,
    category: ServiceCategory,
    worker: &'static str,
    transport: &'static str,
    fields: &'static [(&'static str, &'static str, ServiceConfigurationFieldKind)],
}

const REGISTERED: &[RegisteredService] = &[
    RegisteredService {
        kind: "discord-bridge",
        name: "Discord Bridge",
        provider: "discord",
        category: ServiceCategory::Communications,
        worker: "service-catalog",
        transport: "Discord Gateway + REST",
        fields: &[
            (
                "endpoint",
                "Bridge endpoint",
                ServiceConfigurationFieldKind::Endpoint,
            ),
            (
                "bot-token",
                "Bot token",
                ServiceConfigurationFieldKind::Secret,
            ),
        ],
    },
    RegisteredService {
        kind: "sip-itsp",
        name: "SIP ITSP",
        provider: "sip-itsp",
        category: ServiceCategory::Communications,
        worker: "service-catalog",
        transport: "SIP TLS + RTP",
        fields: &[
            (
                "registrar",
                "Registrar",
                ServiceConfigurationFieldKind::Endpoint,
            ),
            ("username", "Username", ServiceConfigurationFieldKind::Text),
            (
                "credential",
                "Credential",
                ServiceConfigurationFieldKind::Secret,
            ),
        ],
    },
    RegisteredService {
        kind: "airsonic",
        name: "Airsonic",
        provider: "airsonic",
        category: ServiceCategory::Media,
        worker: "media_airsonic_proxy",
        transport: "OpenSubsonic HTTPS",
        fields: &[
            (
                "endpoint",
                "Server endpoint",
                ServiceConfigurationFieldKind::Endpoint,
            ),
            ("username", "Username", ServiceConfigurationFieldKind::Text),
            (
                "credential",
                "Credential",
                ServiceConfigurationFieldKind::Secret,
            ),
        ],
    },
    RegisteredService {
        kind: "jellyfin",
        name: "Jellyfin",
        provider: "jellyfin",
        category: ServiceCategory::Media,
        worker: "media_jellyfin_proxy",
        transport: "Jellyfin HTTPS",
        fields: &[
            (
                "endpoint",
                "Server endpoint",
                ServiceConfigurationFieldKind::Endpoint,
            ),
            ("api-key", "API key", ServiceConfigurationFieldKind::Secret),
        ],
    },
];

/// Project the unified service mirror plus all registered provider adapters into
/// the generic resource-card contract.
pub fn catalog_from_services(
    state: &ServicesState,
) -> Result<ResourceCatalog, ResourceValidationError> {
    catalog_from_services_and_root(state, None, None, None, None)
}

/// Project the catalog with persisted first-class service lifecycle state.
pub fn catalog_from_services_with_root(
    state: &ServicesState,
    workgroup_root: &Path,
) -> Result<ResourceCatalog, ResourceValidationError> {
    catalog_from_services_and_root(state, Some(workgroup_root), None, None, None)
}

/// Project the catalog with persisted first-class service lifecycle state and
/// the latest retained desktop-source roster, when one is available.
pub fn catalog_from_services_with_root_and_desktops(
    state: &ServicesState,
    workgroup_root: &Path,
    desktop_state: Option<&DesktopSourcesState>,
) -> Result<ResourceCatalog, ResourceValidationError> {
    catalog_from_services_and_root(state, Some(workgroup_root), desktop_state, None, None)
}

/// Project the catalog with retained desktop and typed SSH/X11 source rosters.
/// The SSH/X11 roster is optional so older producers remain compatible while a
/// typed source worker is rolled out.
pub fn catalog_from_services_with_root_and_desktops_and_ssh_x11(
    state: &ServicesState,
    workgroup_root: &Path,
    desktop_state: Option<&DesktopSourcesState>,
    ssh_x11_state: Option<&SshX11SourcesState>,
) -> Result<ResourceCatalog, ResourceValidationError> {
    catalog_from_services_and_root(
        state,
        Some(workgroup_root),
        desktop_state,
        ssh_x11_state,
        None,
    )
}

/// Project the catalog with every retained typed source roster currently
/// owned by the universal aggregator.
pub fn catalog_from_services_with_root_and_desktops_and_ssh_x11_and_upnp(
    state: &ServicesState,
    workgroup_root: &Path,
    desktop_state: Option<&DesktopSourcesState>,
    ssh_x11_state: Option<&SshX11SourcesState>,
    upnp_state: Option<&UpnpSourcesState>,
) -> Result<ResourceCatalog, ResourceValidationError> {
    catalog_from_services_and_root(
        state,
        Some(workgroup_root),
        desktop_state,
        ssh_x11_state,
        upnp_state,
    )
}

fn catalog_from_services_and_root(
    state: &ServicesState,
    workgroup_root: Option<&Path>,
    desktop_state: Option<&DesktopSourcesState>,
    ssh_x11_state: Option<&SshX11SourcesState>,
    upnp_state: Option<&UpnpSourcesState>,
) -> Result<ResourceCatalog, ResourceValidationError> {
    let publisher = admitted_catalog_publisher(&state.host)?;
    let now = u64::try_from(state.published_at_ms).unwrap_or(1).max(1);
    let configured: BTreeMap<_, _> = workgroup_root
        .map(load_configurations)
        .unwrap_or_default()
        .into_iter()
        .map(|config| (config.service_kind.clone(), config))
        .collect();
    let mut cards = Vec::with_capacity(
        REGISTERED.len()
            + AospStarterApp::ALL.len()
            + state.records.len()
            + desktop_state.map_or(0, |desktop| desktop.sources.len())
            + ssh_x11_state.map_or(0, |ssh_x11| ssh_x11.sources.len())
            + upnp_state.map_or(0, |upnp| upnp.sources.len()),
    );
    for app in AospStarterApp::ALL {
        cards.push(application_card(app, publisher, now)?);
    }
    for spec in REGISTERED {
        cards.push(registered_card(
            spec,
            &state.host,
            now,
            configured.get(spec.kind),
        )?);
    }
    for record in &state.records {
        cards.push(observed_card(record, &state.host, now)?);
    }
    if let Some(desktop_state) = desktop_state {
        append_desktop_cards(&mut cards, desktop_state, now)?;
    }
    if let Some(ssh_x11_state) = ssh_x11_state {
        append_ssh_x11_cards(&mut cards, ssh_x11_state)?;
    }
    if let Some(upnp_state) = upnp_state {
        append_upnp_cards(&mut cards, upnp_state)?;
    }
    let catalog = ResourceCatalog {
        schema_version: RESOURCE_CONTRACT_VERSION,
        revision: format!("{publisher}-{now}"),
        publisher: publisher.to_owned(),
        generated_at_ms: now,
        content_digest: None,
        cards,
    };
    catalog.with_content_digest()
}

/// Admit the exact node identity that owns provider-declared catalog rows.
///
/// App cards derive their provenance from this publisher. Lossily normalizing
/// an untrusted mirror identity here would let values such as `seat/15` and
/// `seat-15` mint the same fresh provider source, so require the source to
/// already be in the canonical grammar before any App/profile row is built.
fn admitted_catalog_publisher(host: &str) -> Result<&str, ResourceValidationError> {
    let canonical = safe_id(host);
    if host.is_empty() || canonical != host {
        return Err(ResourceValidationError::InvalidField(
            "service_catalog.publisher",
        ));
    }
    Ok(host)
}

/// Append desktop-source cards in stable resource-ID order, collapsing exact
/// duplicate observations while rejecting conflicting rows for the same ID.
/// The final catalog validation remains authoritative for all cross-card
/// relationships and capacity limits.
fn append_desktop_cards(
    cards: &mut Vec<ResourceCard>,
    desktop_state: &DesktopSourcesState,
    now: u64,
) -> Result<(), ResourceValidationError> {
    // The roster is retained across discovery cycles, so its publication
    // timestamp is the authority for whether its source rows may still be
    // projected. A future timestamp is also withheld: accepting it would let
    // clock-skewed or replayed state obtain a fresh five-minute card lease.
    if desktop_state.published_at_ms > now
        || now.saturating_sub(desktop_state.published_at_ms) > DESKTOP_ROSTER_MAX_AGE_MS
    {
        return Ok(());
    }
    let mut desktop_cards = BTreeMap::<String, ResourceCard>::new();
    for source in &desktop_state.sources {
        let card = resource_card_from_desktop_source(source, desktop_state.published_at_ms)?;
        let resource_id = card.resource_id().to_owned();
        match desktop_cards.entry(resource_id) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(card);
            }
            std::collections::btree_map::Entry::Occupied(entry) => {
                if entry.get() != &card {
                    return Err(ResourceValidationError::InvalidRelationship(
                        "desktop_source.conflicting_duplicate",
                    ));
                }
            }
        }
    }

    let mut existing_ids: BTreeSet<String> = cards
        .iter()
        .map(|card| card.resource_id().to_owned())
        .collect();
    for (resource_id, card) in desktop_cards {
        if !existing_ids.insert(resource_id) {
            return Err(ResourceValidationError::InvalidRelationship(
                "desktop_source.catalog_identity_collision",
            ));
        }
        cards.push(card);
    }
    Ok(())
}

fn application_card(
    app: AospStarterApp,
    publisher: &str,
    now: u64,
) -> Result<ResourceCard, ResourceValidationError> {
    let package = app.package_id().as_str();
    Ok(ResourceCard {
        schema_version: RESOURCE_CONTRACT_VERSION,
        identity: ResourceIdentity::new(
            ResourceClass::Application,
            IdentityAuthority::Provider,
            format!("application/aosp/{}", safe_id(package)),
            vec![],
        )?,
        display_name: app.display_name().to_owned(),
        summary: Some(format!(
            "AOSP application · {package} · guest inventory pending"
        )),
        first_seen_at_ms: now,
        last_seen_at_ms: now,
        expires_at_ms: now + CARD_MS,
        health: HealthState {
            schema_version: RESOURCE_CONTRACT_VERSION,
            status: HealthStatus::Unknown,
            observed_at_ms: now,
            expires_at_ms: now + FRESH_MS,
            latency_ms: None,
            failure: Some(failure(
                FailureCode::NotObserved,
                "guest package inventory has not reported availability",
            )),
        },
        auth: AuthState {
            schema_version: RESOURCE_CONTRACT_VERSION,
            status: AuthStatus::NotRequired,
            accepted_methods: vec![],
            active_method: None,
            credential_ref: None,
            updated_at_ms: now,
            expires_at_ms: None,
            failure: None,
        },
        provenance: vec![provenance(
            DiscoverySource::ProviderRegistry,
            ResourceScope::Mesh,
            ProvenanceTrust::OperatorDeclared,
            format!("aosp/{}/{}", safe_id(publisher), safe_id(package)),
            now,
        )],
        transports: vec![],
        client_capabilities: vec![],
        actions: vec![action("inspect", ResourceActionVerb::Inspect, true, now)],
        operating_roles: vec![ResourceOperatingRole::Client, ResourceOperatingRole::Loader],
        service: None,
    })
}

fn registered_card(
    spec: &RegisteredService,
    publisher: &str,
    now: u64,
    configured: Option<&StoredServiceConfiguration>,
) -> Result<ResourceCard, ResourceValidationError> {
    let identity = ResourceIdentity::new(
        ResourceClass::Service,
        IdentityAuthority::Provider,
        format!("provider/{}/{}", spec.provider, spec.kind),
        vec![],
    )?;
    let credential_ref = format!("service/{}/{}", spec.provider, spec.kind);
    let auth_credential_ref = SecretReference::new(credential_ref.clone())?;
    let is_configured = configured.is_some();
    let is_enabled = configured.is_some_and(|config| config.enabled);
    let is_launchable = configured
        .is_some_and(|config| config.enabled && config.last_test_ok == Some(true));
    let health_failure = configured
        .is_some_and(|config| config.last_test_ok == Some(false))
        .then(|| failure(FailureCode::Unreachable, "latest service endpoint test failed"));
    let actions = [
        ("inspect", ResourceActionVerb::Inspect, true),
        ("configure", ResourceActionVerb::Configure, true),
        ("test", ResourceActionVerb::Test, is_configured),
        ("launch", ResourceActionVerb::Launch, is_launchable),
        (
            "enable",
            ResourceActionVerb::Enable,
            is_configured && !is_enabled,
        ),
        ("disable", ResourceActionVerb::Disable, is_enabled),
        ("remove", ResourceActionVerb::Remove, is_configured),
    ]
    .into_iter()
    .map(|(id, verb, ready)| action(id, verb, ready, now))
    .collect();
    Ok(ResourceCard {
        schema_version: RESOURCE_CONTRACT_VERSION,
        identity,
        display_name: spec.name.to_owned(),
        summary: Some(if is_configured {
            "Registered provider adapter · sealed configuration available".into()
        } else {
            "Registered provider adapter · configuration required".into()
        }),
        first_seen_at_ms: now,
        last_seen_at_ms: now,
        expires_at_ms: now + CARD_MS,
        health: HealthState {
            schema_version: RESOURCE_CONTRACT_VERSION,
            status: match configured.and_then(|config| config.last_test_ok) {
                Some(true) => HealthStatus::Available,
                Some(false) => HealthStatus::Unavailable,
                None => HealthStatus::Unknown,
            },
            observed_at_ms: now,
            expires_at_ms: now + FRESH_MS,
            latency_ms: None,
            failure: health_failure,
        },
        auth: AuthState {
            schema_version: RESOURCE_CONTRACT_VERSION,
            status: if is_configured {
                AuthStatus::Authorized
            } else {
                AuthStatus::Required
            },
            accepted_methods: vec![AuthMethod::Password, AuthMethod::BearerToken],
            active_method: is_configured.then_some(AuthMethod::BearerToken),
            credential_ref: is_configured.then_some(auth_credential_ref),
            updated_at_ms: now,
            expires_at_ms: None,
            failure: None,
        },
        provenance: vec![provenance(
            DiscoverySource::ProviderRegistry,
            ResourceScope::Gateway,
            ProvenanceTrust::OperatorDeclared,
            format!("registry/{}", spec.kind),
            now,
        )],
        transports: vec![],
        client_capabilities: vec![],
        actions,
        operating_roles: vec![
            ResourceOperatingRole::Client,
            ResourceOperatingRole::Loader,
            ResourceOperatingRole::Host,
        ],
        service: Some(ServiceInterface {
            service_kind: spec.kind.to_owned(),
            provider_id: Some(spec.provider.to_owned()),
            category: spec.category,
            lifecycle: match configured {
                None => ServiceLifecycleStatus::Unconfigured,
                Some(config) if !config.enabled => ServiceLifecycleStatus::Disabled,
                Some(config) if config.last_test_ok == Some(true) => {
                    ServiceLifecycleStatus::Healthy
                }
                Some(config) if config.last_test_ok == Some(false) => {
                    ServiceLifecycleStatus::Offline
                }
                Some(_) => ServiceLifecycleStatus::Connecting,
            },
            configuration_fields: spec
                .fields
                .iter()
                .map(|(key, label, kind)| ServiceConfigurationField {
                    key: (*key).to_owned(),
                    label: (*label).to_owned(),
                    kind: *kind,
                    required: true,
                    choices: vec![],
                })
                .collect(),
            stack: LocalServiceStack {
                tier: ServiceStackTier::PlatformServices,
                plane: category_plane(spec.category),
                external: true,
                adapter_worker: Some(spec.worker.to_owned()),
                bus_topics: vec![
                    RESOURCE_CATALOG_TOPIC.to_owned(),
                    "action/resources/execute".to_owned(),
                ],
                transport: Some(spec.transport.to_owned()),
                credential_ref: Some(credential_ref),
                // Every Construct can client/load/host the local adapter. The
                // publisher is the live placement candidate until an enabled
                // configuration advertises a narrower placement.
                hosting_nodes: vec![safe_id(publisher)],
                dependencies: vec![],
            },
        }),
    })
}

fn observed_card(
    record: &ServiceRecord,
    publisher: &str,
    now: u64,
) -> Result<ResourceCard, ResourceValidationError> {
    if let Some(card) = probed_rdp_card(record, publisher, now)? {
        return Ok(card);
    }

    let host = safe_id(&record.host);
    let kind = safe_id(&record.kind);
    let identity = ResourceIdentity::new(
        ResourceClass::Service,
        IdentityAuthority::Mesh,
        format!("service/{host}/{kind}"),
        vec![],
    )?;
    let record_is_fresh = u64::try_from(record.last_seen_ms).ok().is_some_and(|observed| {
        observed > 0
            && observed <= now
            && now.saturating_sub(observed) <= SERVICE_RECORD_MAX_AGE_MS
    });
    let effective_health = if record_is_fresh {
        record.health
    } else {
        ServiceHealth::Stale
    };
    let (health, lifecycle, failure) = match effective_health {
        ServiceHealth::Up => (
            HealthStatus::Available,
            ServiceLifecycleStatus::Healthy,
            None,
        ),
        ServiceHealth::Unknown => (
            HealthStatus::Degraded,
            ServiceLifecycleStatus::Degraded,
            Some(failure(
                FailureCode::NotObserved,
                "service is advertised but not probe-confirmed",
            )),
        ),
        ServiceHealth::Stale => (
            HealthStatus::Stale,
            ServiceLifecycleStatus::Degraded,
            Some(failure(FailureCode::Stale, "service observation is stale")),
        ),
        ServiceHealth::Down => (
            HealthStatus::Unavailable,
            ServiceLifecycleStatus::Offline,
            Some(failure(
                FailureCode::Unreachable,
                "service probe reports unavailable",
            )),
        ),
    };
    Ok(ResourceCard {
        schema_version: RESOURCE_CONTRACT_VERSION,
        identity,
        display_name: format!("{} · {}", record.kind, record.host),
        summary: record
            .endpoint
            .as_ref()
            .map(|endpoint| format!("Endpoint {endpoint}")),
        first_seen_at_ms: now,
        last_seen_at_ms: now,
        expires_at_ms: now + CARD_MS,
        health: HealthState {
            schema_version: RESOURCE_CONTRACT_VERSION,
            status: health,
            observed_at_ms: now,
            expires_at_ms: now + FRESH_MS,
            latency_ms: None,
            failure,
        },
        auth: AuthState {
            schema_version: RESOURCE_CONTRACT_VERSION,
            status: AuthStatus::NotRequired,
            accepted_methods: vec![],
            active_method: None,
            credential_ref: None,
            updated_at_ms: now,
            expires_at_ms: None,
            failure: None,
        },
        provenance: vec![provenance(
            DiscoverySource::MeshDirectory,
            ResourceScope::Mesh,
            ProvenanceTrust::AuthenticatedMesh,
            format!("services/{publisher}/{host}/{kind}"),
            now,
        )],
        transports: vec![],
        client_capabilities: vec![],
        actions: vec![action("inspect", ResourceActionVerb::Inspect, true, now)],
        operating_roles: vec![ResourceOperatingRole::Client, ResourceOperatingRole::Host],
        service: Some(ServiceInterface {
            service_kind: kind,
            provider_id: None,
            category: category_for(&record.kind),
            lifecycle,
            configuration_fields: vec![],
            stack: LocalServiceStack {
                tier: ServiceStackTier::PlatformServices,
                plane: category_plane(category_for(&record.kind)),
                external: false,
                adapter_worker: Some("service-aggregator".into()),
                bus_topics: vec![format!("state/services/{}", safe_id(publisher))],
                transport: record.endpoint.clone(),
                credential_ref: None,
                hosting_nodes: vec![host],
                dependencies: vec![],
            },
        }),
    })
}

/// Promote only a fresh, probe-attested trusted-LAN TCP/3389 observation into
/// the universal desktop contract. `ServicesState` does not retain the nmap
/// host-scope enum, so public, malformed, stale, and merely advertised
/// endpoints deliberately remain generic non-connectable service cards.
fn probed_rdp_card(
    record: &ServiceRecord,
    publisher: &str,
    now: u64,
) -> Result<Option<ResourceCard>, ResourceValidationError> {
    if !record.attested_by(ServiceProvenance::Probe) {
        return Ok(None);
    }
    let Some(observed_at_ms) = u64::try_from(record.last_seen_ms)
        .ok()
        .filter(|observed| *observed > 0 && *observed <= now)
    else {
        return Ok(None);
    };
    if now.saturating_sub(observed_at_ms) > PROBED_RDP_MAX_AGE_MS {
        return Ok(None);
    }
    let Some(address) = record
        .endpoint
        .as_deref()
        .and_then(|endpoint| endpoint.parse::<SocketAddr>().ok())
        .filter(|address| address.port() == 3389 && is_trusted_lan_ip(address.ip()))
    else {
        return Ok(None);
    };

    let ip = address.ip().to_string();
    let reachability = match record.health {
        ServiceHealth::Up => Reachability::Reachable,
        ServiceHealth::Down => Reachability::Unreachable,
        ServiceHealth::Unknown | ServiceHealth::Stale => Reachability::Unknown,
    };
    let reason = match record.health {
        ServiceHealth::Up => None,
        ServiceHealth::Down => Some("bounded TCP/3389 probe reports unavailable".into()),
        ServiceHealth::Unknown => Some("TCP/3389 observation is not probe-confirmed".into()),
        ServiceHealth::Stale => Some("TCP/3389 observation is stale".into()),
    };
    let source_id = format!("probe-rdp:{ip}");
    let source = DesktopSource {
        id: source_id.clone(),
        name: format!("Remote Desktop · {ip}"),
        node: safe_id(&record.host),
        host: ip.clone(),
        protocols: vec![ProtocolOffer::new(DesktopProtocol::Rdp, Some(3389))],
        // The compatibility converter's Manual policy is the fail-closed LAN
        // policy: credentials are absent and Connect requires local approval.
        origin: SourceOrigin::Manual,
        reachability,
        reason,
        os_hint: None,
        power_state: None,
        thumbnail_ref: None,
    };
    let mut card = resource_card_from_desktop_source(&source, observed_at_ms)?;
    card.identity = ResourceIdentity::new(
        ResourceClass::Desktop,
        IdentityAuthority::Dns,
        format!("probe-rdp/{ip}"),
        vec![ResourceAlias {
            kind: ResourceAliasKind::LegacyId,
            value: source_id,
        }],
    )?;
    card.summary = Some("Bounded nmap RDP observation · local approval required".into());
    let mut rdp_provenance = provenance(
        // The universal catalog consumes this observation through the
        // authenticated mesh service mirror. `ServiceRecord` does not retain
        // the probing interface, so claiming direct LAN-source provenance here
        // would fabricate the interface required by the resource contract.
        DiscoverySource::MeshDirectory,
        ResourceScope::Mesh,
        ProvenanceTrust::AuthenticatedMesh,
        format!("services/{}/probe-rdp/{}", safe_id(publisher), safe_id(&ip)),
        observed_at_ms,
    );
    // The probe observation is the authority for this card and remains valid
    // for the same bounded five-minute window as the probe-only service row.
    // Leaving the generic one-minute provenance TTL here made the UI present a
    // still-connectable RDP card whose only provenance had already expired.
    rdp_provenance.expires_at_ms = observed_at_ms
        .checked_add(PROBED_RDP_MAX_AGE_MS)
        .ok_or(ResourceValidationError::InvalidTimestamp(
            "probed_rdp.provenance_freshness",
        ))?;
    card.provenance = vec![rdp_provenance];
    card.validate()?;
    Ok(Some(card))
}

fn is_trusted_lan_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_private() || ip.is_link_local(),
        IpAddr::V6(ip) => {
            let first = ip.segments()[0];
            first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80
        }
    }
}

fn action(id: &str, verb: ResourceActionVerb, ready: bool, now: u64) -> ResourceAction {
    ResourceAction {
        schema_version: RESOURCE_CONTRACT_VERSION,
        action_id: id.to_owned(),
        verb,
        target: ResourceActionTarget::Resource,
        availability: ActionAvailability {
            status: if ready {
                ActionAvailabilityStatus::Ready
            } else {
                ActionAvailabilityStatus::Unavailable
            },
            failure: (!ready).then(|| {
                failure(
                    FailureCode::AuthenticationRequired,
                    "configure and test the service before this action",
                )
            }),
        },
        issued_at_ms: now,
        expires_at_ms: now + FRESH_MS,
    }
}

fn provenance(
    source: DiscoverySource,
    scope: ResourceScope,
    trust: ProvenanceTrust,
    source_id: String,
    now: u64,
) -> SourceProvenance {
    SourceProvenance {
        schema_version: RESOURCE_CONTRACT_VERSION,
        source,
        source_id,
        scope,
        trust,
        interface: None,
        observed_at_ms: now,
        expires_at_ms: now + FRESH_MS,
    }
}

fn failure(code: FailureCode, message: &str) -> FailureReason {
    FailureReason {
        code,
        message: message.to_owned(),
    }
}

fn category_for(kind: &str) -> ServiceCategory {
    let lower = kind.to_ascii_lowercase();
    if lower.contains("sip") || lower.contains("discord") || lower.contains("chat") {
        ServiceCategory::Communications
    } else if lower.contains("jellyfin")
        || lower.contains("airsonic")
        || lower.contains("subsonic")
        || lower.contains("media")
    {
        ServiceCategory::Media
    } else if lower.contains("file") || lower.contains("sftp") || lower.contains("rsync") {
        ServiceCategory::Files
    } else if lower.contains("vpn") || lower.contains("dns") || lower.contains("ssh") {
        ServiceCategory::Network
    } else {
        ServiceCategory::Infrastructure
    }
}

const fn category_plane(category: ServiceCategory) -> ServiceStackPlane {
    match category {
        ServiceCategory::Communications | ServiceCategory::Collaboration => {
            ServiceStackPlane::Coordination
        }
        ServiceCategory::Media | ServiceCategory::Files => ServiceStackPlane::Data,
        ServiceCategory::Network | ServiceCategory::Infrastructure => ServiceStackPlane::Control,
        ServiceCategory::External | ServiceCategory::Other => ServiceStackPlane::Experience,
    }
}

fn safe_id(value: &str) -> String {
    let sanitized: String = value
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".into()
    } else {
        sanitized
    }
}

fn registered_service(kind: &str) -> Result<&'static RegisteredService, String> {
    REGISTERED
        .iter()
        .find(|spec| spec.kind == kind)
        .ok_or_else(|| format!("unknown registered service kind '{kind}'"))
}

fn configuration_dir(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("resources").join("services")
}

fn configuration_path(workgroup_root: &Path, kind: &str) -> PathBuf {
    configuration_dir(workgroup_root).join(format!("{kind}.json"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(1)
        .max(1)
}

fn load_configurations(workgroup_root: &Path) -> Vec<StoredServiceConfiguration> {
    let Ok(entries) = std::fs::read_dir(configuration_dir(workgroup_root)) else {
        return vec![];
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).ok()?;
            if !metadata.is_file() || metadata.len() > MAX_CONFIGURATION_BYTES {
                return None;
            }
            let bytes = std::fs::read(path).ok()?;
            let config: StoredServiceConfiguration = serde_json::from_slice(&bytes).ok()?;
            let canonical_name = format!("{}.json", config.service_kind);
            (config.schema_version == SERVICE_CONFIG_VERSION
                && registered_service(&config.service_kind).is_ok()
                && entry.file_name() == std::ffi::OsStr::new(&canonical_name))
            .then_some(config)
        })
        .collect()
}

fn load_configuration(
    workgroup_root: &Path,
    kind: &str,
) -> Result<StoredServiceConfiguration, String> {
    registered_service(kind)?;
    let path = configuration_path(workgroup_root, kind);
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| format!("service is not configured: {error}"))?;
    if !metadata.is_file() || metadata.len() > MAX_CONFIGURATION_BYTES {
        return Err("service configuration is not a bounded regular file".into());
    }
    let config: StoredServiceConfiguration = serde_json::from_slice(
        &std::fs::read(&path).map_err(|error| format!("read service configuration: {error}"))?,
    )
    .map_err(|error| format!("decode service configuration: {error}"))?;
    if config.schema_version != SERVICE_CONFIG_VERSION || config.service_kind != kind {
        return Err("service configuration identity/version mismatch".into());
    }
    Ok(config)
}

fn persist_configuration(
    workgroup_root: &Path,
    config: &StoredServiceConfiguration,
) -> Result<(), String> {
    let dir = configuration_dir(workgroup_root);
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("create service configuration directory: {error}"))?;
    let destination = configuration_path(workgroup_root, &config.service_kind);
    let temporary = dir.join(format!(
        ".{}.{}.tmp",
        config.service_kind,
        std::process::id()
    ));
    let bytes = serde_json::to_vec_pretty(config)
        .map_err(|error| format!("encode service configuration: {error}"))?;
    if bytes.len() as u64 > MAX_CONFIGURATION_BYTES {
        return Err("service configuration exceeds bounded storage limit".into());
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o640);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("create temporary service configuration: {error}"))?;
    let write_result = file
        .write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("write service configuration: {error}"));
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    std::fs::rename(&temporary, &destination)
        .map_err(|error| format!("install service configuration: {error}"))?;
    Ok(())
}

/// Parse the bounded local submission from stdin.
pub fn read_configuration_submission(
    reader: impl std::io::Read,
) -> Result<ServiceConfigurationSubmission, String> {
    let mut bytes = vec![];
    reader
        .take(MAX_CONFIGURATION_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read service configuration from stdin: {error}"))?;
    if bytes.len() as u64 > MAX_CONFIGURATION_BYTES {
        return Err("service configuration exceeds 64 KiB".into());
    }
    serde_json::from_slice(&bytes).map_err(|error| format!("decode service configuration: {error}"))
}

/// Validate, seal secret fields, and persist only non-secret desired state.
pub fn save_configuration(
    workgroup_root: &Path,
    submission: ServiceConfigurationSubmission,
) -> Result<ServiceCardResult, String> {
    let workgroup = workgroup_root.to_path_buf();
    let store = crate::ipc::secret_store::SecretStore::resolve(
        &crate::ipc::secret_store::repo_root(),
        &workgroup,
    );
    save_configuration_with_store(workgroup_root, submission, &store)
}

fn save_configuration_with_store(
    workgroup_root: &Path,
    submission: ServiceConfigurationSubmission,
    store: &crate::ipc::secret_store::SecretStore,
) -> Result<ServiceCardResult, String> {
    let spec = registered_service(submission.service_kind.trim())?;
    let admitted: BTreeSet<_> = spec.fields.iter().map(|(key, _, _)| *key).collect();
    if submission
        .values
        .keys()
        .any(|key| !admitted.contains(key.as_str()))
    {
        return Err("configuration contains a field not admitted by this service adapter".into());
    }
    for (key, _, _) in spec.fields {
        if submission
            .values
            .get(*key)
            .is_none_or(|value| value.trim().is_empty())
        {
            return Err(format!("required service field '{key}' is empty"));
        }
    }

    let mut non_secret_values = BTreeMap::new();
    let mut secrets = BTreeMap::new();
    for (key, _, field_kind) in spec.fields {
        let value = submission.values.get(*key).expect("required field checked");
        if *field_kind == ServiceConfigurationFieldKind::Secret {
            secrets.insert((*key).to_owned(), value.to_owned());
        } else {
            non_secret_values.insert((*key).to_owned(), value.trim().to_owned());
        }
    }
    let credential_ref = format!("service/{}/{}", spec.provider, spec.kind);
    let sealed_values = match spec.kind {
        "airsonic" => BTreeMap::from([
            (
                "username".to_owned(),
                non_secret_values
                    .get("username")
                    .cloned()
                    .unwrap_or_default(),
            ),
            (
                "password".to_owned(),
                secrets.get("credential").cloned().unwrap_or_default(),
            ),
        ]),
        "jellyfin" => BTreeMap::from([(
            "access_token".to_owned(),
            secrets.get("api-key").cloned().unwrap_or_default(),
        )]),
        _ => secrets.clone(),
    };
    let sealed = serde_json::to_string(&sealed_values)
        .map_err(|error| format!("encode sealed service fields: {error}"))?;
    store.put(&credential_ref, &sealed)?;

    let config = StoredServiceConfiguration {
        schema_version: SERVICE_CONFIG_VERSION,
        service_kind: spec.kind.to_owned(),
        non_secret_values,
        secret_fields: secrets.keys().cloned().collect(),
        credential_ref,
        enabled: false,
        last_test_ok: None,
        updated_at_ms: now_ms(),
    };
    persist_configuration(workgroup_root, &config)?;
    Ok(ServiceCardResult {
        service_kind: spec.kind.to_owned(),
        operation: "save",
        ok: true,
        detail: "configuration saved; secret fields sealed",
    })
}

fn endpoint_for<'a>(
    spec: &RegisteredService,
    config: &'a StoredServiceConfiguration,
) -> Result<&'a str, String> {
    let key = if spec.kind == "sip-itsp" {
        "registrar"
    } else {
        "endpoint"
    };
    config
        .non_secret_values
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("configured endpoint field '{key}' is unavailable"))
}

fn probe_endpoint(spec: &RegisteredService, endpoint: &str) -> Result<(), String> {
    if spec.kind != "sip-itsp" {
        let url = reqwest::Url::parse(endpoint)
            .map_err(|_| "endpoint must be an absolute HTTP(S) URL".to_owned())?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err("endpoint must use HTTP or HTTPS".into());
        }
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::limited(3))
            .build()
            .map_err(|error| format!("build endpoint probe: {error}"))?
            .get(url)
            .send()
            .map(|_| ())
            .map_err(|error| format!("service endpoint is unreachable: {error}"))
    } else {
        let authority = endpoint
            .trim()
            .trim_start_matches("sips://")
            .trim_start_matches("sip://")
            .split('/')
            .next()
            .unwrap_or("");
        let address = if authority
            .rsplit_once(':')
            .is_some_and(|(_, port)| port.parse::<u16>().is_ok())
        {
            authority.to_owned()
        } else {
            format!("{authority}:5061")
        };
        let addresses: Vec<SocketAddr> = address
            .to_socket_addrs()
            .map_err(|error| format!("resolve SIP registrar: {error}"))?
            .collect();
        if addresses.is_empty() {
            return Err("SIP registrar resolved to no addresses".into());
        }
        addresses
            .iter()
            .find_map(|address| TcpStream::connect_timeout(address, Duration::from_secs(5)).ok())
            .map(|_| ())
            .ok_or_else(|| "SIP registrar TCP endpoint is unreachable".into())
    }
}

/// Execute the registered adapter's real endpoint reachability probe.
pub fn test_configuration(workgroup_root: &Path, kind: &str) -> Result<ServiceCardResult, String> {
    let spec = registered_service(kind)?;
    let mut config = load_configuration(workgroup_root, kind)?;
    let outcome = probe_endpoint(spec, endpoint_for(spec, &config)?);
    config.last_test_ok = Some(outcome.is_ok());
    config.updated_at_ms = now_ms();
    persist_configuration(workgroup_root, &config)?;
    outcome?;
    Ok(ServiceCardResult {
        service_kind: kind.to_owned(),
        operation: "test",
        ok: true,
        detail: "configured endpoint is reachable",
    })
}

/// Enable only after the same real endpoint probe succeeds.
pub fn enable_configuration(
    workgroup_root: &Path,
    kind: &str,
) -> Result<ServiceCardResult, String> {
    test_configuration(workgroup_root, kind)?;
    let mut config = load_configuration(workgroup_root, kind)?;
    config.enabled = true;
    config.updated_at_ms = now_ms();
    persist_configuration(workgroup_root, &config)?;
    if let Err(error) = publish_media_registration(workgroup_root, &config) {
        config.enabled = false;
        config.updated_at_ms = now_ms();
        let _ = persist_configuration(workgroup_root, &config);
        return Err(error);
    }
    Ok(ServiceCardResult {
        service_kind: kind.to_owned(),
        operation: "enable",
        ok: true,
        detail: "service registration enabled after successful probe",
    })
}

/// Disable without deleting its sealed configuration.
pub fn disable_configuration(
    workgroup_root: &Path,
    kind: &str,
) -> Result<ServiceCardResult, String> {
    let mut config = load_configuration(workgroup_root, kind)?;
    config.enabled = false;
    config.updated_at_ms = now_ms();
    persist_configuration(workgroup_root, &config)?;
    remove_media_registration(workgroup_root, kind)?;
    Ok(ServiceCardResult {
        service_kind: kind.to_owned(),
        operation: "disable",
        ok: true,
        detail: "service adapter disabled; sealed configuration retained",
    })
}

/// Remove desired state. The credential remains sealed for safe rotation/audit;
/// the card returns to Unconfigured and never exposes the stored value.
pub fn remove_configuration(
    workgroup_root: &Path,
    kind: &str,
) -> Result<ServiceCardResult, String> {
    registered_service(kind)?;
    let path = configuration_path(workgroup_root, kind);
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("remove service configuration: {error}")),
    }
    remove_media_registration(workgroup_root, kind)?;
    Ok(ServiceCardResult {
        service_kind: kind.to_owned(),
        operation: "remove",
        ok: true,
        detail: "service configuration removed; sealed credential retained",
    })
}

fn local_hostname() -> Result<String, String> {
    let hostname = read_bounded_hostname(Path::new("/etc/hostname"))?;
    let hostname = hostname.trim();
    if hostname.is_empty() || hostname.chars().any(char::is_whitespace) {
        return Err("local hostname is invalid for a media gateway registration".into());
    }
    Ok(hostname.to_owned())
}

fn read_bounded_hostname(path: &Path) -> Result<String, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("read local hostname metadata: {error}"))?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_HOSTNAME_BYTES as u64 {
        return Err("local hostname is not a bounded regular file".into());
    }
    let mut body = String::new();
    std::fs::File::open(path)
        .map_err(|error| format!("read local hostname: {error}"))?
        .take((MAX_HOSTNAME_BYTES + 1) as u64)
        .read_to_string(&mut body)
        .map_err(|error| format!("read local hostname: {error}"))?;
    if body.len() > MAX_HOSTNAME_BYTES {
        return Err("local hostname exceeds its byte bound".into());
    }
    Ok(body)
}

fn media_registration_dir(workgroup_root: &Path, hostname: &str) -> PathBuf {
    workgroup_root.join(format!("service-cards-{}", safe_id(hostname)))
}

fn publish_media_registration(
    workgroup_root: &Path,
    config: &StoredServiceConfiguration,
) -> Result<(), String> {
    use crate::mesh_media::{
        AirsonicGatewayRegistration, GatewayHealth, JellyfinGatewayRegistration,
        AIRSONIC_GATEWAY_REGISTRY_FILE, JELLYFIN_GATEWAY_REGISTRY_FILE,
    };
    let hostname = local_hostname()?;
    let directory = media_registration_dir(workgroup_root, &hostname);
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("create media gateway registration directory: {error}"))?;
    let (file, body) = match config.service_kind.as_str() {
        "airsonic" => {
            let endpoint = config
                .non_secret_values
                .get("endpoint")
                .ok_or_else(|| "Airsonic endpoint is missing".to_owned())?;
            let registration = AirsonicGatewayRegistration::new(
                &hostname,
                endpoint,
                &config.credential_ref,
                GatewayHealth::Healthy,
                true,
            )
            .ok_or_else(|| "Airsonic gateway registration is invalid".to_owned())?;
            (
                AIRSONIC_GATEWAY_REGISTRY_FILE,
                serde_json::to_vec_pretty(&vec![registration])
                    .map_err(|error| format!("encode Airsonic registration: {error}"))?,
            )
        }
        "jellyfin" => {
            let endpoint = config
                .non_secret_values
                .get("endpoint")
                .ok_or_else(|| "Jellyfin endpoint is missing".to_owned())?;
            let registration = JellyfinGatewayRegistration::new(
                &hostname,
                endpoint,
                &config.credential_ref,
                GatewayHealth::Healthy,
                true,
            )
            .ok_or_else(|| "Jellyfin gateway registration is invalid".to_owned())?;
            (
                JELLYFIN_GATEWAY_REGISTRY_FILE,
                serde_json::to_vec_pretty(&vec![registration])
                    .map_err(|error| format!("encode Jellyfin registration: {error}"))?,
            )
        }
        _ => return Ok(()),
    };
    let path = directory.join(file);
    let temporary = directory.join(format!(".{file}.{}.tmp", std::process::id()));
    std::fs::write(&temporary, body)
        .map_err(|error| format!("write temporary media gateway registration: {error}"))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("install media gateway registration: {error}"))
}

fn remove_media_registration(workgroup_root: &Path, kind: &str) -> Result<(), String> {
    use crate::mesh_media::{AIRSONIC_GATEWAY_REGISTRY_FILE, JELLYFIN_GATEWAY_REGISTRY_FILE};
    let file = match kind {
        "airsonic" => AIRSONIC_GATEWAY_REGISTRY_FILE,
        "jellyfin" => JELLYFIN_GATEWAY_REGISTRY_FILE,
        _ => return Ok(()),
    };
    let hostname = local_hostname()?;
    let path = media_registration_dir(workgroup_root, &hostname).join(file);
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove media gateway registration: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::super::desktop_sources::DesktopSource;
    use super::*;

    fn local_secret_store(root: &Path) -> crate::ipc::secret_store::SecretStore {
        let key_path = root.join("mesh-age-key");
        std::fs::write(
            &key_path,
            "AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQSXKLP0E\n",
        )
        .unwrap();
        crate::ipc::secret_store::SecretStore::LocalAead {
            dir: root.join("sealed"),
            key_path,
        }
    }

    #[test]
    fn catalog_always_exposes_named_first_class_service_cards() {
        let catalog = catalog_from_services(&ServicesState {
            host: "seat-15".into(),
            records: vec![],
            published_at_ms: 1_700_000_000_000,
        })
        .expect("valid catalog");
        let kinds: Vec<_> = catalog
            .cards
            .iter()
            .filter_map(|card| {
                card.service
                    .as_ref()
                    .map(|service| service.service_kind.as_str())
            })
            .collect();
        assert_eq!(
            kinds,
            ["discord-bridge", "sip-itsp", "airsonic", "jellyfin"]
        );
        assert!(catalog
            .cards
            .iter()
            .all(|card| !card.operating_roles.is_empty()));
        assert!(catalog.content_digest.is_some());
        catalog.validate().expect("attested catalog");
    }

    #[test]
    fn app_catalog_rejects_lossy_publisher_provenance_before_projection() {
        for host in ["", "seat/15", "seat 15", "seat-15\n", "SEAT-15"] {
            let error = catalog_from_services(&ServicesState {
                host: host.into(),
                records: vec![],
                published_at_ms: 1_700_000_000_000,
            })
            .expect_err("ambiguous publisher must not mint fresh App provenance");
            assert_eq!(
                error,
                ResourceValidationError::InvalidField("service_catalog.publisher"),
                "unexpected admission result for {host:?}"
            );
        }

        let catalog = catalog_from_services(&ServicesState {
            host: "seat-15".into(),
            records: vec![],
            published_at_ms: 1_700_000_000_000,
        })
        .expect("canonical publisher remains admitted");
        assert!(catalog.cards.iter().any(|card| {
            card.identity.class == ResourceClass::Application
                && card.provenance.iter().all(|source| {
                    source.source_id.starts_with("aosp/seat-15/")
                        && source.trust == ProvenanceTrust::OperatorDeclared
                })
        }));
    }

    #[test]
    fn discovered_services_receive_equal_generic_cards() {
        let catalog = catalog_from_services(&ServicesState {
            host: "seat-15".into(),
            records: vec![ServiceRecord {
                host: "dell".into(),
                kind: "ssh".into(),
                endpoint: Some("10.42.0.4:22".into()),
                provenance: vec![],
                health: ServiceHealth::Up,
                action: Some("open-ssh".into()),
                last_seen_ms: 1_700_000_000_000,
            }],
            published_at_ms: 1_700_000_000_000,
        })
        .expect("valid catalog");
        let ssh = catalog
            .cards
            .iter()
            .find(|card| {
                card.service
                    .as_ref()
                    .is_some_and(|service| service.service_kind == "ssh")
            })
            .expect("ssh card");
        assert_eq!(
            ssh.service.as_ref().expect("service").stack.hosting_nodes,
            ["dell"]
        );
    }

    fn observed_service(
        host: &str,
        kind: &str,
        endpoint: &str,
        provenance: Vec<ServiceProvenance>,
        health: ServiceHealth,
        last_seen_ms: i64,
    ) -> ServiceRecord {
        ServiceRecord {
            host: host.into(),
            kind: kind.into(),
            endpoint: Some(endpoint.into()),
            provenance,
            health,
            action: None,
            last_seen_ms,
        }
    }

    #[test]
    fn rdp_bounded_nmap_tcp_3389_becomes_an_approval_gated_remote_session_card() {
        const NOW: i64 = 1_700_000_000_000;
        let catalog = catalog_from_services(&ServicesState {
            host: "seat-15".into(),
            records: vec![observed_service(
                "windows-lab",
                "ms-wbt-server",
                "192.168.40.23:3389",
                vec![ServiceProvenance::Probe],
                ServiceHealth::Up,
                NOW - 1_000,
            )],
            published_at_ms: NOW,
        })
        .expect("valid universal catalog");

        let card = catalog
            .cards
            .iter()
            .find(|card| card.identity.canonical_key == "probe-rdp/192.168.40.23")
            .expect("remote session card");
        assert_eq!(card.identity.class, ResourceClass::Desktop);
        assert!(card.service.is_none(), "RDP is not a generic service card");
        assert_eq!(card.auth.status, AuthStatus::Required);
        assert_eq!(card.auth.accepted_methods, [AuthMethod::LocalApproval]);
        assert!(card.transports.iter().any(|transport| {
            transport.protocol == TransportProtocol::Rdp
                && transport.scope == ResourceScope::TrustedLan
                && matches!(
                    &transport.endpoint,
                    TransportEndpoint::Network { host, port: 3389, .. }
                        if host == "192.168.40.23"
                )
        }));
        let connect = card
            .actions
            .iter()
            .find(|action| action.verb == ResourceActionVerb::Connect)
            .expect("typed connect action");
        assert_eq!(
            connect.availability.status,
            ActionAvailabilityStatus::RequiresApproval
        );
        assert_eq!(card.provenance[0].source, DiscoverySource::MeshDirectory);
        assert_eq!(card.provenance[0].scope, ResourceScope::Mesh);
        assert_eq!(card.provenance[0].trust, ProvenanceTrust::AuthenticatedMesh);
        catalog.validate().expect("validated catalog");
    }

    #[test]
    fn rdp_promotion_survives_the_gap_between_card_and_probe_ttls() {
        const NOW: i64 = 1_700_000_000_000;
        let observed = NOW - i64::try_from(CARD_MS).unwrap() - 1;
        let catalog = catalog_from_services(&ServicesState {
            host: "seat-15".into(),
            records: vec![observed_service(
                "quiet-windows",
                "ms-wbt-server",
                "172.20.146.54:3389",
                vec![ServiceProvenance::Probe],
                ServiceHealth::Up,
                observed,
            )],
            published_at_ms: NOW,
        })
        .expect("probe-fresh RDP observation remains a valid catalog input");

        let card = catalog
            .cards
            .iter()
            .find(|card| card.identity.canonical_key == "probe-rdp/172.20.146.54")
            .expect("RDP card remains promoted");
        assert!(card.expires_at_ms > u64::try_from(NOW).unwrap());
        assert!(card.provenance[0].expires_at_ms > u64::try_from(NOW).unwrap());
        catalog.validate().expect("validated catalog");
    }

    #[test]
    fn rdp_promotion_rejects_untrusted_ambiguous_and_stale_records() {
        const NOW: i64 = 1_700_000_000_000;
        let catalog = catalog_from_services(&ServicesState {
            host: "seat-15".into(),
            records: vec![
                observed_service(
                    "published-only",
                    "rdp",
                    "192.168.40.20:3389",
                    vec![ServiceProvenance::Published],
                    ServiceHealth::Unknown,
                    NOW - 1_000,
                ),
                observed_service(
                    "wrong-port",
                    "rdp",
                    "192.168.40.21:3390",
                    vec![ServiceProvenance::Probe],
                    ServiceHealth::Up,
                    NOW - 1_000,
                ),
                observed_service(
                    "malformed",
                    "rdp",
                    "192.168.40.22:not-a-port",
                    vec![ServiceProvenance::Probe],
                    ServiceHealth::Up,
                    NOW - 1_000,
                ),
                observed_service(
                    "public-target",
                    "ms-wbt-server",
                    "203.0.113.8:3389",
                    vec![ServiceProvenance::Probe],
                    ServiceHealth::Up,
                    NOW - 1_000,
                ),
                observed_service(
                    "stale-target",
                    "port/3389",
                    "192.168.40.24:3389",
                    vec![ServiceProvenance::Probe],
                    ServiceHealth::Up,
                    NOW - i64::try_from(PROBED_RDP_MAX_AGE_MS).unwrap() - 1,
                ),
            ],
            published_at_ms: NOW,
        })
        .expect("invalid promotion candidates remain generic cards");

        assert!(catalog
            .cards
            .iter()
            .all(|card| card.identity.class != ResourceClass::Desktop));
        assert!(catalog.cards.iter().all(|card| {
            card.transports
                .iter()
                .all(|transport| transport.protocol != TransportProtocol::Rdp)
        }));
        catalog.validate().expect("validated fail-closed catalog");
    }

    #[test]
    fn rdp_unavailable_probe_is_visible_but_never_connectable() {
        const NOW: i64 = 1_700_000_000_000;
        let catalog = catalog_from_services(&ServicesState {
            host: "seat-15".into(),
            records: vec![observed_service(
                "windows-offline",
                "port/3389",
                "10.20.30.40:3389",
                vec![ServiceProvenance::Probe],
                ServiceHealth::Down,
                NOW - 1_000,
            )],
            published_at_ms: NOW,
        })
        .expect("valid unavailable remote session");
        let card = catalog
            .cards
            .iter()
            .find(|card| card.identity.class == ResourceClass::Desktop)
            .expect("unavailable desktop remains visible");
        assert_eq!(card.health.status, HealthStatus::Unavailable);
        assert!(card.actions.iter().any(|action| {
            action.verb == ResourceActionVerb::Connect
                && action.availability.status == ActionAvailabilityStatus::Unavailable
        }));
        assert!(!card.actions.iter().any(|action| {
            action.verb == ResourceActionVerb::Connect
                && action.availability.status == ActionAvailabilityStatus::Ready
        }));
    }

    #[test]
    fn replayed_service_rows_cannot_regain_available_health_from_catalog_publication() {
        const NOW: i64 = 1_700_000_000_000;
        let catalog = catalog_from_services(&ServicesState {
            host: "seat-15".into(),
            records: vec![
                observed_service(
                    "stale-host",
                    "ssh",
                    "10.42.0.8:22",
                    vec![ServiceProvenance::Probe],
                    ServiceHealth::Up,
                    NOW - i64::try_from(SERVICE_RECORD_MAX_AGE_MS).unwrap() - 1,
                ),
                observed_service(
                    "future-host",
                    "https",
                    "10.42.0.9:443",
                    vec![ServiceProvenance::Probe],
                    ServiceHealth::Up,
                    NOW + 1,
                ),
            ],
            published_at_ms: NOW,
        })
        .expect("invalid source freshness degrades rather than refreshing service rows");

        for host in ["stale-host", "future-host"] {
            let card = catalog
                .cards
                .iter()
                .find(|card| {
                    card.service.as_ref().is_some_and(|service| {
                        service.stack.hosting_nodes == [host.to_owned()]
                    })
                })
                .expect("retained service remains inspectable");
            assert_eq!(card.health.status, HealthStatus::Stale);
            assert_eq!(
                card.service.as_ref().expect("service interface").lifecycle,
                ServiceLifecycleStatus::Degraded
            );
        }
        catalog.validate().expect("validated fail-closed catalog");
    }

    #[test]
    fn optional_desktop_state_adds_stable_deduplicated_cards() {
        let root = tempfile::tempdir().unwrap();
        let services = ServicesState {
            host: "seat-15".into(),
            records: vec![],
            published_at_ms: 1_700_000_000_000,
        };
        let source = DesktopSource {
            id: "peer:oak".into(),
            name: "Oak Seat".into(),
            node: "oak".into(),
            host: "10.42.0.7".into(),
            protocols: vec![super::super::desktop_sources::ProtocolOffer::new(
                super::super::desktop_sources::DesktopProtocol::Rdp,
                Some(3389),
            )],
            origin: super::super::desktop_sources::SourceOrigin::MeshPeer,
            reachability: super::super::desktop_sources::Reachability::Reachable,
            reason: None,
            os_hint: None,
            power_state: None,
            thumbnail_ref: None,
        };
        let desktop = DesktopSourcesState {
            node: "desktop-discovery".into(),
            sources: vec![source.clone(), source],
            lanes: vec![],
            published_at_ms: 1_700_000_000_000,
        };

        let catalog =
            catalog_from_services_with_root_and_desktops(&services, root.path(), Some(&desktop))
                .expect("valid desktop state");
        assert_eq!(
            catalog
                .cards
                .iter()
                .filter(|card| card.identity.canonical_key == "peer:oak")
                .count(),
            1
        );
        assert_eq!(
            catalog
                .cards
                .iter()
                .filter(|card| card.display_name == "Oak Seat")
                .count(),
            1
        );
        catalog.validate().expect("validated catalog");
        assert!(catalog.content_digest.is_some());
    }

    #[test]
    fn stale_or_future_desktop_roster_cannot_revive_rdp_cards() {
        const NOW: i64 = 1_700_000_000_000;
        let root = tempfile::tempdir().unwrap();
        let services = ServicesState {
            host: "seat-15".into(),
            records: vec![],
            published_at_ms: NOW,
        };
        let source = DesktopSource {
            id: "peer:stale-windows".into(),
            name: "Stale Windows".into(),
            node: "windows".into(),
            host: "172.20.146.54".into(),
            protocols: vec![ProtocolOffer::new(DesktopProtocol::Rdp, Some(3389))],
            origin: SourceOrigin::MeshPeer,
            reachability: Reachability::Reachable,
            reason: None,
            os_hint: None,
            power_state: None,
            thumbnail_ref: None,
        };

        for published_at_ms in [
            (NOW - i64::try_from(DESKTOP_ROSTER_MAX_AGE_MS).unwrap() - 1) as u64,
            (NOW + 1) as u64,
        ] {
            let catalog = catalog_from_services_with_root_and_desktops(
                &services,
                root.path(),
                Some(&DesktopSourcesState {
                    node: "desktop-discovery".into(),
                    sources: vec![source.clone()],
                    lanes: vec![],
                    published_at_ms,
                }),
            )
            .expect("stale roster is withheld without invalidating the catalog");
            assert!(!catalog
                .cards
                .iter()
                .any(|card| card.identity.canonical_key == "peer:stale-windows"));
            catalog.validate().expect("catalog remains valid");
        }
    }

    #[test]
    fn optional_upnp_state_adds_trusted_media_cards_to_the_universal_catalog() {
        let root = tempfile::tempdir().unwrap();
        let subnet =
            super::super::upnp_sources::TrustedLanSubnet::new("172.20.146.0".parse().unwrap(), 24)
                .unwrap();
        let policy = super::super::upnp_sources::UpnpDiscoveryPolicy::default_for(vec![
            super::super::upnp_sources::TrustedLanInterface::new("enp0s31f6", vec![subnet])
                .unwrap(),
        ])
        .unwrap();
        let adapter = super::super::upnp_sources::UpnpDiscoveryAdapter::new(policy);
        let packet = b"HTTP/1.1 200 OK\r\n\
CACHE-CONTROL: max-age=120\r\n\
LOCATION: http://172.20.146.20:8200/rootDesc.xml\r\n\
ST: urn:schemas-upnp-org:device:MediaServer:1\r\n\
USN: uuid:2f402f80-da50-11e1-9b23-00025b00a001::urn:schemas-upnp-org:device:MediaServer:1\r\n\
SERVER: MCNF/1.0\r\n\r\n";
        let source = adapter
            .admit_packet(
                packet,
                &super::super::upnp_sources::SsdpPacketContext {
                    interface: "enp0s31f6".into(),
                    source: "172.20.146.20:1900".parse().unwrap(),
                    observed_at_ms: 1_700_000_000_000,
                },
            )
            .unwrap();
        let upnp = super::super::upnp_sources::UpnpSourcesState {
            node: "seat-15".into(),
            sources: vec![source],
            published_at_ms: 1_700_000_000_000,
        };
        let services = ServicesState {
            host: "seat-15".into(),
            records: vec![],
            published_at_ms: 1_700_000_000_000,
        };

        let catalog = catalog_from_services_with_root_and_desktops_and_ssh_x11_and_upnp(
            &services,
            root.path(),
            None,
            None,
            Some(&upnp),
        )
        .expect("valid UPnP source state");
        let card = catalog
            .cards
            .iter()
            .find(|card| card.display_name == "UPnP media server at 172.20.146.20")
            .expect("UPnP media-server card");
        assert_eq!(card.identity.class, ResourceClass::MediaServer);
        assert_eq!(card.provenance[0].source, DiscoverySource::SsdpUpnp);
        catalog.validate().expect("validated UPnP catalog");
    }

    #[test]
    fn every_catalog_constructor_attaches_a_deterministic_content_digest() {
        let state = ServicesState {
            host: "seat-15".into(),
            records: vec![],
            published_at_ms: 1_700_000_000_000,
        };
        let catalog = catalog_from_services(&state).expect("valid catalog");
        assert_eq!(
            catalog.content_digest.as_deref(),
            Some(catalog.computed_content_digest().as_str())
        );
        assert!(catalog
            .discovery_projection()
            .unwrap()
            .catalog_content_digest
            .is_some());
    }

    #[test]
    fn save_seals_secret_and_catalog_projects_real_lifecycle() {
        let root = tempfile::tempdir().unwrap();
        let store = local_secret_store(root.path());
        save_configuration_with_store(
            root.path(),
            ServiceConfigurationSubmission {
                service_kind: "jellyfin".into(),
                values: BTreeMap::from([
                    ("endpoint".into(), "https://media.example.test".into()),
                    ("api-key".into(), "TOP-SECRET-VALUE".into()),
                ]),
            },
            &store,
        )
        .expect("save configuration");

        let raw = std::fs::read(configuration_path(root.path(), "jellyfin")).unwrap();
        assert!(!raw
            .windows(b"TOP-SECRET-VALUE".len())
            .any(|part| part == b"TOP-SECRET-VALUE"));
        let sealed = store
            .get("service/jellyfin/jellyfin")
            .unwrap()
            .expect("sealed secret");
        assert!(sealed.contains("TOP-SECRET-VALUE"));

        let state = ServicesState {
            host: "seat-15".into(),
            records: vec![],
            published_at_ms: 1_700_000_000_000,
        };
        let catalog = catalog_from_services_with_root(&state, root.path()).unwrap();
        let card = catalog
            .cards
            .iter()
            .find(|card| card.display_name == "Jellyfin")
            .unwrap();
        assert_eq!(
            card.service.as_ref().unwrap().lifecycle,
            ServiceLifecycleStatus::Disabled
        );
        assert_eq!(card.auth.status, AuthStatus::Authorized);
        for verb in [
            ResourceActionVerb::Test,
            ResourceActionVerb::Enable,
            ResourceActionVerb::Remove,
        ] {
            assert!(card.actions.iter().any(|action| {
                action.verb == verb && action.availability.status == ActionAvailabilityStatus::Ready
            }));
        }
    }

    #[test]
    fn restart_ignores_uncommitted_service_configuration_staging_inode() {
        let root = tempfile::tempdir().unwrap();
        let directory = configuration_dir(root.path());
        std::fs::create_dir_all(&directory).unwrap();
        let staged = StoredServiceConfiguration {
            schema_version: SERVICE_CONFIG_VERSION,
            service_kind: "jellyfin".into(),
            non_secret_values: BTreeMap::from([(
                "endpoint".into(),
                "https://uncommitted.example.test".into(),
            )]),
            secret_fields: vec!["api-key".into()],
            credential_ref: "service/jellyfin/jellyfin".into(),
            enabled: true,
            last_test_ok: Some(true),
            updated_at_ms: 1_700_000_000_000,
        };
        std::fs::write(
            directory.join(".jellyfin.4242.tmp"),
            serde_json::to_vec_pretty(&staged).unwrap(),
        )
        .unwrap();

        let catalog = catalog_from_services_with_root(
            &ServicesState {
                host: "seat-15".into(),
                records: vec![],
                published_at_ms: 1_700_000_000_000,
            },
            root.path(),
        )
        .expect("abandoned staging state is ignored after restart");
        let card = catalog
            .cards
            .iter()
            .find(|card| card.display_name == "Jellyfin")
            .expect("registered Jellyfin adapter");
        assert_eq!(
            card.service.as_ref().expect("service interface").lifecycle,
            ServiceLifecycleStatus::Unconfigured
        );
        assert!(card.actions.iter().any(|action| {
            action.verb == ResourceActionVerb::Launch
                && action.availability.status == ActionAvailabilityStatus::Unavailable
        }));
        catalog.validate().expect("validated fail-closed catalog");
    }

    #[test]
    fn failed_latest_test_revokes_launch_admission_even_when_enabled() {
        let root = tempfile::tempdir().unwrap();
        let config = StoredServiceConfiguration {
                schema_version: SERVICE_CONFIG_VERSION,
                service_kind: "jellyfin".into(),
                non_secret_values: BTreeMap::from([(
                    "endpoint".into(),
                    "https://media.example.test".into(),
                )]),
                secret_fields: vec!["api-key".into()],
                credential_ref: "service/jellyfin/jellyfin".into(),
                enabled: true,
                last_test_ok: Some(false),
                updated_at_ms: 1_700_000_000_000,
            };
        persist_configuration(root.path(), &config).expect("persist service state");
        let spec = registered_service("jellyfin").expect("registered Jellyfin adapter");
        let card = registered_card(spec, "seat-15", 1_700_000_000_000, Some(&config))
            .expect("valid service card");
        card.validate().expect("failed probe remains a valid card");
        let launch = card
            .actions
            .iter()
            .find(|action| action.verb == ResourceActionVerb::Launch)
            .expect("Launch action");
        assert_eq!(
            launch.availability.status,
            ActionAvailabilityStatus::Unavailable
        );
        assert_eq!(
            card.service.as_ref().expect("service interface").lifecycle,
            ServiceLifecycleStatus::Offline
        );
    }

    #[test]
    fn oversized_hostname_input_fails_closed_before_validation() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("hostname");
        std::fs::write(&path, vec![b'h'; MAX_HOSTNAME_BYTES + 1]).unwrap();
        let error = read_bounded_hostname(&path).expect_err("oversized hostname must fail closed");
        assert!(error.contains("bound"));
    }

    #[test]
    fn http_service_probe_uses_the_configured_endpoint() {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
        });
        let spec = registered_service("jellyfin").unwrap();
        probe_endpoint(spec, &format!("http://{address}/health")).unwrap();
        server.join().unwrap();
    }
}
