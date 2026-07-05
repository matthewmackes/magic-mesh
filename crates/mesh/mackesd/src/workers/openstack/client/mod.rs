//! IAC-1 — the OpenStack **client foundation**: authenticate via `clouds.yaml`,
//! read the Keystone service catalog, probe per-service API health, and expose
//! the standard resource + verb call seam (`docs/design/iac-workspace.md`,
//! locked 2026-07-04).
//!
//! This is the client half of the OpenStack integration. Its server half — the
//! QUASAR-CLOUD [`super`] worker — *runs* the Kolla service containers; this
//! module *talks to* the resulting cloud as a standard-API client, the seam the
//! later IAC surface units (IAC-2 Overview / IAC-3 Resources / IAC-4 Heat) build
//! on. It reuses the established worker idioms — pure builders/parsers + injectable
//! seams + a testkit of fakes + honest typed degrade (§7), exactly like
//! [`super::verbs`].
//!
//! ## The pieces
//!
//! - [`config`] — the **`clouds.yaml`** loader (Q20): parse + select the single
//!   default context (Q19); the password rides the file, never argv.
//! - [`keystone`] — **Keystone v3 auth** ([`KeystoneAuth`]): one `POST
//!   /auth/tokens` yields the bearer token **and** the service catalog.
//! - [`health`] — **per-service API health** ([`EndpointProbe`]): a real
//!   version/ping GET per cataloged endpoint → `{ up/down, latency_ms,
//!   microversion }`.
//! - [`resource`] — the **standard resource + verb call seam** ([`ResourceApi`]):
//!   list/show/create/update/delete over the catalog endpoint, Keystone-token-
//!   authed — the boundary IAC-3 drives.
//!
//! ## The §6 wire contract
//!
//! The catalog + health the surface renders are the **mesh-neutral shared types**
//! in [`mackes_mesh_types::openstack`] (`ServiceCatalog` / `ServiceHealth`), so
//! IAC-2's `mde-shell-egui` consumes them off the Bus without depending on
//! `mackesd`. The [`CatalogSource`] seam is what the QC-11 `action/cloud/get-catalog`
//! verb ([`super::verbs`]) drives to publish that contract; a node with no
//! `clouds.yaml` answers an honest [`ClientError::Unconfigured`] gate, never a
//! fabricated catalog.

pub mod config;
pub mod health;
pub mod keystone;
pub mod resource;
#[cfg(test)]
pub(crate) mod testkit;

use std::sync::Arc;

use mackes_mesh_types::openstack::{ResourceTable, ServiceCatalog, ServiceHealth};

use config::CloudConfig;
use health::{probe_service_health, EndpointProbe, HttpProbe};
use keystone::{KeystoneAuth, KeystoneHttp, Session};

pub use resource::{
    build_resource_url, resolve_endpoint, ResourceApi, ResourceRef, ResourceRequest,
    ResourceResponse, TokenRestApi, Verb,
};

/// A typed, honest client failure — every degrade path names its cause; none is
/// a fabricated success (§7).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClientError {
    /// No `OpenStack` context is configured on this node (no `clouds.yaml`). The
    /// caller surfaces this as a **gate** (retry once the operator configures
    /// the cloud), distinct from a real failure.
    #[error("no OpenStack context configured — {0}")]
    Unconfigured(String),
    /// A `clouds.yaml` exists but is malformed / incomplete / ambiguous.
    #[error("clouds.yaml — {0}")]
    Config(String),
    /// Keystone authentication failed (bad credentials, no token).
    #[error("Keystone auth — {0}")]
    Auth(String),
    /// The token response carried no parseable catalog.
    #[error("Keystone catalog — {0}")]
    Catalog(String),
    /// An HTTP transport failure (unreachable identity/endpoint, timeout).
    #[error("transport — {0}")]
    Transport(String),
}

impl ClientError {
    /// Whether this is the "no cloud configured here" gate (vs a real failure) —
    /// the QC-11 responder maps it to a gated reply, not a failed one.
    #[must_use]
    pub const fn is_unconfigured(&self) -> bool {
        matches!(self, Self::Unconfigured(_))
    }
}

/// The catalog + per-service health together — the payload the IAC status band
/// consumes (both come from one authenticate + probe pass).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogHealth {
    /// The authoritative service directory.
    pub catalog: ServiceCatalog,
    /// One health row per cataloged service, in catalog order.
    pub health: Vec<ServiceHealth>,
}

/// The seam the QC-11 `action/cloud/get-catalog` verb drives to produce the §6
/// catalog+health contract. Production: [`LiveOpenStack`]; tests inject
/// [`testkit::FakeCatalogSource`].
pub trait CatalogSource {
    /// Load the context, authenticate, and probe every cataloged endpoint's
    /// health.
    ///
    /// # Errors
    /// [`ClientError::Unconfigured`] when no `clouds.yaml` exists (an honest
    /// gate); a typed [`ClientError`] on a config/auth/transport failure.
    fn catalog_and_health(&self) -> Result<CatalogHealth, ClientError>;
}

/// The seam the IAC-3 `action/cloud/list-resources` verb drives to list one
/// cataloged service's resources as the §6 [`ResourceTable`] the Resources tab
/// renders.
///
/// Lists `servers`/`networks`/`stacks`/… . Production: [`LiveOpenStack`]
/// (authenticate → the REST `list` seam → parse); tests inject
/// [`testkit::FakeCatalogSource`].
pub trait ResourceSource {
    /// List the `collection` of `service_type`, applying `query` filters, into a
    /// typed [`ResourceTable`].
    ///
    /// # Errors
    /// [`ClientError::Unconfigured`] when no `clouds.yaml` exists (an honest
    /// gate); a typed [`ClientError`] on a config/auth/transport failure or an
    /// unparseable (non-list) response — never a fabricated table (§7).
    fn list_resources(
        &self,
        service_type: &str,
        collection: &str,
        query: &[(String, String)],
    ) -> Result<ResourceTable, ClientError>;
}

/// The unified `OpenStack` client seam the cloud-verb responder ([`super::verbs`])
/// drives — the catalog+health read **and** the per-service resource list as one
/// injected object.
///
/// A single seam threads the drain (IAC-1's §6 glue). Blanket-implemented for
/// anything that is both a [`CatalogSource`] and a [`ResourceSource`]: production
/// [`LiveOpenStack`], tests [`testkit::FakeCatalogSource`].
pub trait CloudClient: CatalogSource + ResourceSource + Send + Sync {}

impl<T: CatalogSource + ResourceSource + Send + Sync> CloudClient for T {}

/// The composed client: a resolved [`CloudConfig`] + the auth & probe seams.
///
/// Injectable for tests (fake auth + probe); [`OpenStackClient::live`] wires the
/// production HTTP seams.
pub struct OpenStackClient {
    config: CloudConfig,
    auth: Box<dyn KeystoneAuth + Send + Sync>,
    probe: Box<dyn EndpointProbe + Send + Sync>,
}

impl OpenStackClient {
    /// Compose a client from a resolved config + the two seams (tests inject
    /// fakes).
    #[must_use]
    pub fn new(
        config: CloudConfig,
        auth: Box<dyn KeystoneAuth + Send + Sync>,
        probe: Box<dyn EndpointProbe + Send + Sync>,
    ) -> Self {
        Self {
            config,
            auth,
            probe,
        }
    }

    /// Compose the production client (real Keystone HTTP + real endpoint probe)
    /// over an already-resolved config.
    #[must_use]
    pub fn live(config: CloudConfig) -> Self {
        Self::new(
            config,
            Box::new(KeystoneHttp::new()),
            Box::new(HttpProbe::new()),
        )
    }

    /// The resolved context (password redacted in its [`std::fmt::Debug`]).
    #[must_use]
    pub const fn config(&self) -> &CloudConfig {
        &self.config
    }

    /// Authenticate and return the Keystone session (token + catalog).
    ///
    /// # Errors
    /// A typed [`ClientError`] on an auth/transport failure.
    pub fn authenticate(&self) -> Result<Session, ClientError> {
        self.auth.authenticate(&self.config)
    }

    /// The production resource-call API bound to this config's interface — the
    /// seam IAC-3 drives (a real Keystone-token-authed REST caller).
    #[must_use]
    pub const fn resource_api(&self) -> TokenRestApi {
        TokenRestApi::new(self.config.interface)
    }
}

impl CatalogSource for OpenStackClient {
    fn catalog_and_health(&self) -> Result<CatalogHealth, ClientError> {
        let session = self.authenticate()?;
        let health = probe_service_health(&session.catalog, &*self.probe, self.config.interface);
        Ok(CatalogHealth {
            catalog: session.catalog,
            health,
        })
    }
}

impl ResourceSource for OpenStackClient {
    fn list_resources(
        &self,
        service_type: &str,
        collection: &str,
        query: &[(String, String)],
    ) -> Result<ResourceTable, ClientError> {
        let session = self.authenticate()?;
        let req = ResourceRequest {
            verb: Verb::List,
            target: ResourceRef {
                service_type: service_type.to_string(),
                collection: collection.to_string(),
                id: None,
            },
            body: None,
            query: query.to_vec(),
        };
        let resp = self.resource_api().call(&session, &req)?;
        if !resp.is_success() {
            // A non-2xx (a wrong endpoint / an API error) is honest transport
            // failure — never parsed into a fake-empty table (§7).
            return Err(ClientError::Transport(format!(
                "HTTP {} listing {service_type}/{collection}",
                resp.status
            )));
        }
        ResourceTable::from_collection_json(service_type, collection, &resp.body)
            .map_err(|e| ClientError::Catalog(e.to_string()))
    }
}

/// The production [`CatalogSource`].
///
/// Loads the standard `clouds.yaml` **on each call** (so a node that gets its
/// context configured later starts working without a restart), then
/// authenticates + probes. A missing `clouds.yaml` is the honest
/// [`ClientError::Unconfigured`] gate.
#[derive(Debug, Clone, Default)]
pub struct LiveOpenStack;

impl LiveOpenStack {
    /// Construct the production catalog source.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Wrap it in an [`Arc`] for the worker seam — the unified [`CloudClient`]
    /// (catalog + resource list).
    #[must_use]
    pub fn shared() -> Arc<dyn CloudClient> {
        Arc::new(Self)
    }
}

impl CatalogSource for LiveOpenStack {
    fn catalog_and_health(&self) -> Result<CatalogHealth, ClientError> {
        let config = config::load_default()?;
        OpenStackClient::live(config).catalog_and_health()
    }
}

impl ResourceSource for LiveOpenStack {
    fn list_resources(
        &self,
        service_type: &str,
        collection: &str,
        query: &[(String, String)],
    ) -> Result<ResourceTable, ClientError> {
        let config = config::load_default()?;
        OpenStackClient::live(config).list_resources(service_type, collection, query)
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::{FakeKeystone, FakeProbe};
    use super::*;
    use mackes_mesh_types::openstack::{EndpointInterface, HealthState};

    fn cfg() -> CloudConfig {
        CloudConfig {
            cloud: "mesh".into(),
            auth_url: "http://keystone.mesh:5000/v3".into(),
            username: "operator".into(),
            password: "pw".into(),
            project_name: Some("mesh".into()),
            project_domain: "Default".into(),
            user_domain: "Default".into(),
            region_name: Some("RegionOne".into()),
            interface: EndpointInterface::Public,
        }
    }

    fn session() -> Session {
        Session {
            token: "tok".into(),
            catalog: ServiceCatalog::from_keystone_token_json(
                r#"{"token":{"catalog":[
                    {"type":"compute","name":"nova","endpoints":[
                        {"interface":"public","url":"http://nova.mesh:8774/v2.1","region":"RegionOne"}
                    ]}
                ]}}"#,
            )
            .unwrap(),
            expires_at: None,
        }
    }

    #[test]
    fn catalog_and_health_authenticates_then_probes() {
        // The composed client: authenticate → catalog, then probe each endpoint.
        let client = OpenStackClient::new(
            cfg(),
            Box::new(FakeKeystone::ok(session())),
            Box::new(FakeProbe::new().up(
                "http://nova.mesh:8774/v2.1",
                r#"{"version":{"id":"v2.1","status":"CURRENT","max_version":"2.90"}}"#,
                9,
            )),
        );
        let ch = client.catalog_and_health().expect("catalog+health");
        assert_eq!(ch.catalog.services.len(), 1);
        assert_eq!(ch.health.len(), 1);
        assert_eq!(ch.health[0].state, HealthState::Up);
        assert_eq!(ch.health[0].microversion.as_deref(), Some("2.90"));
    }

    #[test]
    fn an_auth_failure_propagates_and_never_probes() {
        // §7 — a failed auth is a typed error, and the probe is never reached
        // (no fabricated health for an unauthenticated cloud).
        let client = OpenStackClient::new(
            cfg(),
            Box::new(FakeKeystone::failing(ClientError::Auth("401".into()))),
            Box::new(FakeProbe::new()),
        );
        let err = client.catalog_and_health().expect_err("auth fails");
        assert!(matches!(err, ClientError::Auth(_)));
        assert!(!err.is_unconfigured());
    }

    #[test]
    fn unconfigured_is_flagged_as_a_gate() {
        assert!(ClientError::Unconfigured("no file".into()).is_unconfigured());
        assert!(!ClientError::Transport("refused".into()).is_unconfigured());
    }
}
