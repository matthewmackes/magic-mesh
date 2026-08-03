//! First-class universal service-card projection.
//!
//! This adapter turns the existing unified service inventory into the versioned
//! resource catalog and adds operator-configurable provider adapters even before
//! they are configured. The shell therefore renders one generic card per
//! service, never one special-case screen per provider.

#![cfg(feature = "async-services")]

use mackes_mesh_types::android_apps::AospStarterApp;
use mackes_mesh_types::resources::*;
use mackes_mesh_types::service_record::{ServiceHealth, ServiceRecord, ServicesState};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const FRESH_MS: u64 = 60_000;
const CARD_MS: u64 = 120_000;
const SERVICE_CONFIG_VERSION: u16 = 1;
const MAX_CONFIGURATION_BYTES: u64 = 64 * 1024;

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
    catalog_from_services_and_root(state, None)
}

/// Project the catalog with persisted first-class service lifecycle state.
pub fn catalog_from_services_with_root(
    state: &ServicesState,
    workgroup_root: &Path,
) -> Result<ResourceCatalog, ResourceValidationError> {
    catalog_from_services_and_root(state, Some(workgroup_root))
}

fn catalog_from_services_and_root(
    state: &ServicesState,
    workgroup_root: Option<&Path>,
) -> Result<ResourceCatalog, ResourceValidationError> {
    let now = u64::try_from(state.published_at_ms).unwrap_or(1).max(1);
    let configured: BTreeMap<_, _> = workgroup_root
        .map(load_configurations)
        .unwrap_or_default()
        .into_iter()
        .map(|config| (config.service_kind.clone(), config))
        .collect();
    let mut cards =
        Vec::with_capacity(REGISTERED.len() + AospStarterApp::ALL.len() + state.records.len());
    for app in AospStarterApp::ALL {
        cards.push(application_card(app, &state.host, now)?);
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
    let catalog = ResourceCatalog {
        schema_version: RESOURCE_CONTRACT_VERSION,
        revision: format!("{}-{now}", safe_id(&state.host)),
        publisher: safe_id(&state.host),
        generated_at_ms: now,
        cards,
    };
    catalog.validate()?;
    Ok(catalog)
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
    let actions = [
        ("inspect", ResourceActionVerb::Inspect, true),
        ("configure", ResourceActionVerb::Configure, true),
        ("test", ResourceActionVerb::Test, is_configured),
        ("launch", ResourceActionVerb::Launch, is_enabled),
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
            failure: None,
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
    let host = safe_id(&record.host);
    let kind = safe_id(&record.kind);
    let identity = ResourceIdentity::new(
        ResourceClass::Service,
        IdentityAuthority::Mesh,
        format!("service/{host}/{kind}"),
        vec![],
    )?;
    let (health, lifecycle, failure) = match record.health {
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
            (config.schema_version == SERVICE_CONFIG_VERSION
                && registered_service(&config.service_kind).is_ok())
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
    let hostname = std::fs::read_to_string("/etc/hostname")
        .map_err(|error| format!("read local hostname: {error}"))?;
    let hostname = hostname.trim();
    if hostname.is_empty() || hostname.chars().any(char::is_whitespace) {
        return Err("local hostname is invalid for a media gateway registration".into());
    }
    Ok(hostname.to_owned())
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
