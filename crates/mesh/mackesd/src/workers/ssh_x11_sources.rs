//! WL-FUNC-019 — universal resource-card projection for SSH/X11 resources.
//!
//! The mesh-type contract admits SSH/SFTP and two distinct X11 shapes. This
//! adapter is the reachable daemon-side seam that turns those admitted forms
//! into the same card/transport/capability/action grammar used by desktop and
//! SSDP discovery. It never creates a command, URL, raw `DISPLAY` value, or
//! credential payload. X11 cards remain useful evidence when the local DRM
//! seat has no X server, but their connect action is unavailable in that state.

use mackes_mesh_types::resources::{
    ActionAvailability, ActionAvailabilityStatus, AuthMethod, AuthState, AuthStatus,
    ClientBoundary, ClientCapability, ClientCapabilityLimits, ClientFeature, DiscoverySource,
    FailureCode, FailureReason, HealthState, HealthStatus, IdentityAuthority, ProvenanceTrust,
    RESOURCE_CONTRACT_VERSION, ResourceAction, ResourceActionTarget, ResourceActionVerb,
    ResourceCard, ResourceClass, ResourceIdentity, ResourceOperatingRole, ResourceScope,
    ResourceValidationError, SourceProvenance, TransportCandidate, TransportEndpoint,
    TransportProtocol,
};
use mackes_mesh_types::ssh_x11::{
    SftpBrowseRoot, SshAuthentication, SshEndpoint, SshX11Resource, X11DisplayBinding,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Retained typed source roster consumed by the universal catalog fold.
pub const SSH_X11_SOURCES_TOPIC: &str = "state/resources/ssh-x11";
/// Maximum retained source-roster body accepted before JSON decoding.
pub const MAX_SSH_X11_SOURCES_STATE_BYTES: usize = 2 * 1024 * 1024;
/// Maximum number of source records admitted in one retained roster.
pub const MAX_SSH_X11_SOURCE_RECORDS: usize = 1_024;

/// One stable operator/provider identity paired with an admitted SSH/X11 form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshX11SourceRecord {
    /// Stable identity within the publisher's typed source registry.
    pub source_id: String,
    /// The already-admitted SSH/SFTP/X11 resource.
    pub resource: SshX11Resource,
}

/// Retained source state read by `service_aggregator`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshX11SourcesState {
    /// Publishing node/provider identity.
    pub node: String,
    /// Reachability boundary selected by the source producer.
    pub scope: ResourceScope,
    /// Typed source rows.
    pub sources: Vec<SshX11SourceRecord>,
    /// Millisecond observation time for the source roster.
    pub published_at_ms: u64,
}

impl SshX11SourcesState {
    /// Validate roster bounds and every nested admission contract.
    pub fn validate(&self) -> Result<(), String> {
        validate_state_text("ssh_x11.node", &self.node, 255)?;
        if self.published_at_ms == 0 {
            return Err("ssh/x11 source state has a zero published_at_ms".into());
        }
        if self.sources.len() > MAX_SSH_X11_SOURCE_RECORDS {
            return Err(format!(
                "ssh/x11 source state contains {} records; maximum is {}",
                self.sources.len(),
                MAX_SSH_X11_SOURCE_RECORDS
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        for source in &self.sources {
            validate_state_text("ssh_x11.source_id", &source.source_id, 255)?;
            if !ids.insert(&source.source_id) {
                return Err(format!("duplicate SSH/X11 source id: {}", source.source_id));
            }
            source
                .resource
                .validate()
                .map_err(|error| format!("invalid SSH/X11 source {}: {error}", source.source_id))?;
        }
        Ok(())
    }
}

/// Decode a retained source roster with an explicit body bound and semantic
/// admission before it can influence the catalog.
pub fn decode_sources_state(body: &str) -> Result<SshX11SourcesState, String> {
    if body.len() > MAX_SSH_X11_SOURCES_STATE_BYTES {
        return Err(format!(
            "SSH/X11 source state is {} bytes; maximum is {}",
            body.len(),
            MAX_SSH_X11_SOURCES_STATE_BYTES
        ));
    }
    let state: SshX11SourcesState = serde_json::from_str(body)
        .map_err(|error| format!("strict SSH/X11 state decode: {error}"))?;
    state.validate()?;
    Ok(state)
}

/// Append typed SSH/X11 cards to an existing catalog card set, folding exact
/// duplicate observations and rejecting identity collisions/conflicts.
pub fn append_ssh_x11_cards(
    cards: &mut Vec<ResourceCard>,
    state: &SshX11SourcesState,
) -> Result<(), ResourceValidationError> {
    state
        .validate()
        .map_err(|_| ResourceValidationError::InvalidField("ssh_x11.source_state"))?;
    let mut source_cards = BTreeMap::<String, ResourceCard>::new();
    for source in &state.sources {
        let card = resource_card_from_ssh_x11(
            &source.resource,
            &source.source_id,
            state.scope,
            state.published_at_ms,
        )?;
        let resource_id = card.resource_id().to_owned();
        match source_cards.entry(resource_id) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(card);
            }
            std::collections::btree_map::Entry::Occupied(entry) => {
                if entry.get() != &card {
                    return Err(ResourceValidationError::InvalidRelationship(
                        "ssh_x11.conflicting_duplicate",
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
                "ssh_x11.catalog_identity_collision",
            ));
        }
        cards.push(card);
    }
    Ok(())
}

fn validate_state_text(field: &'static str, value: &str, max_bytes: usize) -> Result<(), String> {
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

/// Retain one admitted SSH/X11 card for two minutes before a producer must
/// refresh it. The value is within the shared resource-contract TTL bounds.
pub const SSH_X11_CARD_TTL_MS: u64 = 120_000;

/// Project one admitted SSH/X11 resource into the universal resource catalog.
///
/// `source_id` is a stable operator/provider identity such as
/// `bench/t480/home` or `gateway/lab/x11-editor`; it is validated as part of
/// the canonical resource identity and is never interpreted as a command or
/// path. `scope` records the reachability boundary selected by the producer.
///
/// # Errors
///
/// Returns a shared resource-contract error if the source identity, typed
/// endpoint, capability binding, or card relationship is invalid.
pub fn resource_card_from_ssh_x11(
    resource: &SshX11Resource,
    source_id: &str,
    scope: ResourceScope,
    observed_at_ms: u64,
) -> Result<ResourceCard, ResourceValidationError> {
    resource
        .validate()
        .map_err(|_| ResourceValidationError::InvalidField("ssh_x11.resource"))?;
    let expires_at_ms = observed_at_ms.checked_add(SSH_X11_CARD_TTL_MS).ok_or(
        ResourceValidationError::InvalidTimestamp("ssh_x11.freshness"),
    )?;
    let (class, protocol, display_name, summary, endpoint, available) = describe(resource);
    let capability = client_capability(protocol, auth_method(resource))?;
    let health = health(available, observed_at_ms, expires_at_ms);
    let transport = TransportCandidate::new(
        protocol,
        endpoint,
        scope,
        0,
        observed_at_ms,
        expires_at_ms,
        health.clone(),
        Some(capability.fingerprint.clone()),
    )?;
    let identity = ResourceIdentity::new(
        class,
        IdentityAuthority::Operator,
        format!("ssh-x11/{source_id}"),
        vec![],
    )?;
    let auth = auth_state(resource, observed_at_ms);
    let actions = vec![
        inspect_action(observed_at_ms, expires_at_ms),
        connect_action(
            protocol,
            &transport,
            &capability,
            available,
            observed_at_ms,
            expires_at_ms,
        ),
    ];
    let card = ResourceCard {
        schema_version: RESOURCE_CONTRACT_VERSION,
        identity,
        display_name,
        summary: Some(summary),
        first_seen_at_ms: observed_at_ms,
        last_seen_at_ms: observed_at_ms,
        expires_at_ms,
        health,
        auth,
        provenance: vec![SourceProvenance {
            schema_version: RESOURCE_CONTRACT_VERSION,
            source: DiscoverySource::Manual,
            source_id: format!("ssh-x11/{source_id}"),
            scope,
            trust: ProvenanceTrust::OperatorDeclared,
            interface: None,
            observed_at_ms,
            expires_at_ms,
        }],
        transports: vec![transport],
        client_capabilities: vec![capability],
        actions,
        operating_roles: vec![ResourceOperatingRole::Client],
        service: None,
    };
    card.validate()?;
    Ok(card)
}

fn describe(
    resource: &SshX11Resource,
) -> (
    ResourceClass,
    TransportProtocol,
    String,
    String,
    TransportEndpoint,
    bool,
) {
    match resource {
        SshX11Resource::SftpBrowser(resource) => (
            ResourceClass::FileShare,
            TransportProtocol::Ssh,
            format!(
                "SFTP · {}@{}",
                resource.endpoint.user, resource.endpoint.host
            ),
            format!(
                "SFTP browse root: {}",
                match resource.browse_path.root {
                    SftpBrowseRoot::Home => "home",
                    SftpBrowseRoot::Shared => "shared",
                }
            ),
            network_endpoint(&resource.endpoint),
            true,
        ),
        SshX11Resource::SshForwardedX11Application(resource) => (
            ResourceClass::Application,
            TransportProtocol::SshX11Application,
            format!(
                "SSH X11 · {} · {}@{}",
                resource.application_id.as_str(),
                resource.endpoint.user,
                resource.endpoint.host
            ),
            "SSH allocates the forwarded X11 display at session start".into(),
            network_endpoint(&resource.endpoint),
            resource.is_available(),
        ),
        SshX11Resource::RemoteX11Desktop(resource) => {
            let (display, screen) = explicit_display(&resource.display);
            (
                ResourceClass::Desktop,
                TransportProtocol::X11Desktop,
                format!(
                    "X11 desktop · {}:{} · {}@{}",
                    display, screen, resource.endpoint.user, resource.endpoint.host
                ),
                "Explicit remote X11 display endpoint".into(),
                TransportEndpoint::X11 {
                    host: resource.endpoint.host.clone(),
                    port: resource.endpoint.port,
                    display,
                    screen,
                },
                resource.is_available(),
            )
        }
    }
}

fn network_endpoint(endpoint: &SshEndpoint) -> TransportEndpoint {
    TransportEndpoint::Network {
        host: endpoint.host.clone(),
        port: endpoint.port,
        base_path: None,
    }
}

fn explicit_display(binding: &X11DisplayBinding) -> (u16, u8) {
    match binding {
        X11DisplayBinding::Explicit { display } => (display.number, display.screen),
        X11DisplayBinding::SshForwarded => {
            unreachable!("validated X11 desktop has explicit display")
        }
    }
}

fn auth_method(resource: &SshX11Resource) -> AuthMethod {
    let endpoint = endpoint(resource);
    match &endpoint.auth {
        SshAuthentication::MeshIdentity => AuthMethod::MeshIdentity,
        SshAuthentication::SshKey { .. } => AuthMethod::SshKey,
        SshAuthentication::Password { .. } => AuthMethod::Password,
    }
}

fn endpoint(resource: &SshX11Resource) -> &SshEndpoint {
    match resource {
        SshX11Resource::SftpBrowser(resource) => &resource.endpoint,
        SshX11Resource::SshForwardedX11Application(resource) => &resource.endpoint,
        SshX11Resource::RemoteX11Desktop(resource) => &resource.endpoint,
    }
}

fn auth_state(resource: &SshX11Resource, observed_at_ms: u64) -> AuthState {
    let endpoint = endpoint(resource);
    let (method, credential_ref) = match &endpoint.auth {
        SshAuthentication::MeshIdentity => (AuthMethod::MeshIdentity, None),
        SshAuthentication::SshKey { credential_ref } => {
            (AuthMethod::SshKey, Some(credential_ref.clone()))
        }
        SshAuthentication::Password { credential_ref } => {
            (AuthMethod::Password, Some(credential_ref.clone()))
        }
    };
    AuthState {
        schema_version: RESOURCE_CONTRACT_VERSION,
        status: AuthStatus::Authorized,
        accepted_methods: vec![method],
        active_method: Some(method),
        credential_ref,
        updated_at_ms: observed_at_ms,
        expires_at_ms: None,
        failure: None,
    }
}

fn client_capability(
    protocol: TransportProtocol,
    auth_method: AuthMethod,
) -> Result<ClientCapability, ResourceValidationError> {
    let (adapter_id, features, limits) = match protocol {
        TransportProtocol::Ssh => (
            "construct.sftp",
            vec![ClientFeature::FileBrowse, ClientFeature::Reconnect],
            ClientCapabilityLimits {
                max_width: None,
                max_height: None,
                max_fps: None,
                max_audio_channels: None,
                max_parallel_sessions: 4,
            },
        ),
        TransportProtocol::SshX11Application | TransportProtocol::X11Desktop => (
            "construct.ssh-x11",
            vec![
                ClientFeature::Display,
                ClientFeature::KeyboardInput,
                ClientFeature::PointerInput,
                ClientFeature::X11Forwarding,
                ClientFeature::Reconnect,
            ],
            ClientCapabilityLimits {
                max_width: Some(3_840),
                max_height: Some(2_160),
                max_fps: Some(60),
                max_audio_channels: None,
                max_parallel_sessions: 1,
            },
        ),
        _ => {
            return Err(ResourceValidationError::InvalidRelationship(
                "ssh_x11.transport_protocol",
            ));
        }
    };
    ClientCapability::new(
        adapter_id,
        "1",
        protocol,
        "1",
        ClientBoundary::PlatformAdapter,
        vec![auth_method],
        features,
        limits,
        vec![ResourceActionVerb::Connect],
    )
}

fn health(available: bool, observed_at_ms: u64, expires_at_ms: u64) -> HealthState {
    if available {
        HealthState {
            schema_version: RESOURCE_CONTRACT_VERSION,
            status: HealthStatus::Available,
            observed_at_ms,
            expires_at_ms,
            latency_ms: None,
            failure: None,
        }
    } else {
        HealthState {
            schema_version: RESOURCE_CONTRACT_VERSION,
            status: HealthStatus::Unavailable,
            observed_at_ms,
            expires_at_ms,
            latency_ms: None,
            failure: Some(FailureReason {
                code: FailureCode::MissingDisplay,
                message: "local DRM seat has no X11 display for this resource".into(),
            }),
        }
    }
}

fn inspect_action(observed_at_ms: u64, expires_at_ms: u64) -> ResourceAction {
    ResourceAction {
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
    }
}

fn connect_action(
    protocol: TransportProtocol,
    transport: &TransportCandidate,
    capability: &ClientCapability,
    available: bool,
    observed_at_ms: u64,
    expires_at_ms: u64,
) -> ResourceAction {
    let availability = if available {
        ActionAvailability {
            status: ActionAvailabilityStatus::Ready,
            failure: None,
        }
    } else {
        ActionAvailability {
            status: ActionAvailabilityStatus::Unavailable,
            failure: Some(FailureReason {
                code: FailureCode::MissingDisplay,
                message: "local DRM seat has no X11 display for this resource".into(),
            }),
        }
    };
    ResourceAction {
        schema_version: RESOURCE_CONTRACT_VERSION,
        action_id: format!("connect-{}", protocol.token()),
        verb: ResourceActionVerb::Connect,
        target: ResourceActionTarget::TransportClient {
            transport_fingerprint: transport.fingerprint.clone(),
            capability_fingerprint: capability.fingerprint.clone(),
        },
        availability,
        issued_at_ms: observed_at_ms,
        expires_at_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::resources::{ActionAvailabilityStatus, ResourceScope};
    use mackes_mesh_types::ssh_x11::{
        DrmSeatX11State, RemoteX11DesktopEndpoint, SftpBrowsePath, SshAuthentication, SshEndpoint,
        SshForwardedX11Application, SshSftpBrowser, X11ApplicationId, X11DisplayNumber,
    };

    const NOW: u64 = 1_700_000_000_000;

    fn endpoint() -> SshEndpoint {
        SshEndpoint::new(
            "t480-bench",
            22,
            "operator",
            SshAuthentication::MeshIdentity,
        )
        .expect("valid endpoint")
    }

    #[test]
    fn sftp_card_is_launchable_without_an_x_server() {
        let resource = SshX11Resource::SftpBrowser(
            SshSftpBrowser::new(endpoint(), SftpBrowsePath::home()).expect("valid sftp"),
        );
        let card = resource_card_from_ssh_x11(
            &resource,
            "bench/t480/home",
            ResourceScope::TrustedLan,
            NOW,
        )
        .expect("card");
        assert_eq!(card.identity.class, ResourceClass::FileShare);
        assert_eq!(card.health.status, HealthStatus::Available);
        assert_eq!(card.transports[0].protocol, TransportProtocol::Ssh);
        assert!(
            card.actions
                .iter()
                .any(|action| action.verb == ResourceActionVerb::Connect
                    && action.availability.status == ActionAvailabilityStatus::Ready)
        );
        let encoded = serde_json::to_string(&card).expect("card JSON");
        assert!(!encoded.contains("command"));
        assert!(!encoded.contains("password"));
    }

    #[test]
    fn x11_cards_preserve_honest_missing_drm_display() {
        let resource = SshX11Resource::SshForwardedX11Application(
            SshForwardedX11Application::new(
                endpoint(),
                X11ApplicationId::new("editor").expect("app id"),
                DrmSeatX11State::unavailable_no_x_server(),
            )
            .expect("valid x11 app"),
        );
        let card = resource_card_from_ssh_x11(
            &resource,
            "bench/t480/editor",
            ResourceScope::TrustedLan,
            NOW,
        )
        .expect("card");
        assert_eq!(card.identity.class, ResourceClass::Application);
        assert_eq!(card.health.status, HealthStatus::Unavailable);
        assert_eq!(
            card.health.failure.as_ref().map(|failure| failure.code),
            Some(FailureCode::MissingDisplay)
        );
        assert!(card.actions.iter().any(|action| {
            action.verb == ResourceActionVerb::Connect
                && action.availability.status == ActionAvailabilityStatus::Unavailable
        }));
        assert!(matches!(
            card.transports[0].endpoint,
            TransportEndpoint::Network { .. }
        ));
    }

    #[test]
    fn explicit_x11_desktop_keeps_numeric_display_in_transport() {
        let resource = SshX11Resource::RemoteX11Desktop(
            RemoteX11DesktopEndpoint::new(
                endpoint(),
                X11DisplayNumber::new(7, 2).expect("display"),
                DrmSeatX11State::Available {
                    display: X11DisplayNumber::new(0, 0).expect("local display"),
                },
            )
            .expect("valid desktop"),
        );
        let card =
            resource_card_from_ssh_x11(&resource, "lab/remote/desktop", ResourceScope::Mesh, NOW)
                .expect("card");
        assert!(matches!(
            card.transports[0].endpoint,
            TransportEndpoint::X11 {
                display: 7,
                screen: 2,
                ..
            }
        ));
        assert!(
            card.actions
                .iter()
                .any(|action| action.availability.status == ActionAvailabilityStatus::Ready)
        );
    }

    #[test]
    fn hostile_source_identity_is_rejected_before_card_projection() {
        let resource = SshX11Resource::SftpBrowser(
            SshSftpBrowser::new(endpoint(), SftpBrowsePath::shared()).expect("valid sftp"),
        );
        assert_eq!(
            resource_card_from_ssh_x11(&resource, "../command", ResourceScope::Mesh, NOW),
            Err(ResourceValidationError::InvalidField(
                "identity.canonical_key"
            ))
        );
    }
}
