//! IAC-1 — the standard resource + verb call seam: the boundary the later IAC
//! units (IAC-3 Resources tab, IAC-4 Heat) build on.
//!
//! Design #4/#14/#15: every cataloged service (Nova/Neutron/Glance/Cinder/Heat/…)
//! exposes the standard REST **list / show / create / update / delete** over its
//! catalog endpoint, Keystone-token-authed. IAC-1 defines the **seam** (the
//! [`ResourceApi`] trait + the typed [`ResourceRequest`]/[`ResourceResponse`]
//! envelopes) and a real production impl [`TokenRestApi`]; IAC-3 drives it from
//! the catalog-driven menu bar + resource tables. There are **no stubs** — the
//! production impl issues the real HTTP call; it is simply not yet UI-wired
//! (that is IAC-3, gated on the live cloud smoke IAC-6).
//!
//! The pure pieces — [`Verb::http_method`] + [`build_resource_url`] — are
//! fixture-tested so the REST surface can't silently drift; tests inject
//! [`super::testkit::FakeResourceApi`].

use std::time::Duration;

use mackes_mesh_types::openstack::EndpointInterface;

use super::keystone::Session;
use super::ClientError;

/// A standard CRUD verb against an `OpenStack` collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// `GET <collection>` — list.
    List,
    /// `GET <collection>/<id>` — show one.
    Show,
    /// `POST <collection>` — create.
    Create,
    /// `PUT <collection>/<id>` — replace/update.
    Update,
    /// `DELETE <collection>/<id>` — delete.
    Delete,
}

impl Verb {
    /// The HTTP method this verb issues.
    #[must_use]
    pub const fn http_method(self) -> &'static str {
        match self {
            Self::List | Self::Show => "GET",
            Self::Create => "POST",
            Self::Update => "PUT",
            Self::Delete => "DELETE",
        }
    }

    /// Whether the verb targets a single resource (needs an `id`).
    #[must_use]
    pub const fn needs_id(self) -> bool {
        matches!(self, Self::Show | Self::Update | Self::Delete)
    }

    /// Whether the verb is a mutation (create/update/delete) — the ops IAC-3
    /// typed-arms + audits (#22/#23).
    #[must_use]
    pub const fn is_mutation(self) -> bool {
        matches!(self, Self::Create | Self::Update | Self::Delete)
    }
}

/// Which resource a call targets: the service type (catalog key), the collection
/// path segment, and an optional resource id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceRef {
    /// The Keystone service **type** (`compute`, `network`, `image`,
    /// `volumev3`, `orchestration`, …) — resolved against the session catalog to
    /// find the endpoint base.
    pub service_type: String,
    /// The collection path under the endpoint base (`servers`, `networks`,
    /// `images`, `volumes`, `stacks`, …).
    pub collection: String,
    /// The resource id, for single-resource verbs.
    pub id: Option<String>,
}

/// A typed resource call: the verb, the target, an optional JSON body (create/
/// update), and query params (list filters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceRequest {
    /// The CRUD verb.
    pub verb: Verb,
    /// What it targets.
    pub target: ResourceRef,
    /// The request body (create/update), as JSON. `None` for reads/deletes.
    pub body: Option<serde_json::Value>,
    /// Query params (list filters), applied verbatim.
    pub query: Vec<(String, String)>,
}

/// A resource call's response — the raw HTTP status + body the caller parses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceResponse {
    /// The HTTP status code.
    pub status: u16,
    /// The response body (JSON, usually).
    pub body: String,
}

impl ResourceResponse {
    /// Whether the response is a 2xx success.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        self.status >= 200 && self.status < 300
    }
}

/// The injectable resource-call seam. [`TokenRestApi`] is the production impl;
/// tests inject [`super::testkit::FakeResourceApi`], and IAC-3 drives the real
/// one from the surface.
pub trait ResourceApi {
    /// Issue `req` against the endpoint resolved from `session`'s catalog,
    /// Keystone-token-authed.
    ///
    /// # Errors
    /// [`ClientError::Config`] when the service/interface isn't in the catalog,
    /// [`ClientError::Config`] when a single-resource verb lacks an id, or
    /// [`ClientError::Transport`] on an HTTP failure.
    fn call(
        &self,
        session: &Session,
        req: &ResourceRequest,
    ) -> Result<ResourceResponse, ClientError>;
}

/// Build the request URL from an endpoint base + collection (+ id).
///
/// Joins with exactly one `/` between segments regardless of a trailing slash on
/// the base or a leading slash on the collection.
#[must_use]
pub fn build_resource_url(endpoint_base: &str, collection: &str, id: Option<&str>) -> String {
    let base = endpoint_base.trim_end_matches('/');
    let coll = collection.trim_matches('/');
    match id {
        Some(id) if !id.trim().is_empty() => format!("{base}/{coll}/{}", id.trim()),
        _ => format!("{base}/{coll}"),
    }
}

/// Resolve the endpoint base URL for `service_type` on `interface` from the
/// session catalog.
///
/// # Errors
/// [`ClientError::Config`] when the service isn't cataloged or advertises no
/// endpoint for the interface (honest — the caller can't reach it).
pub fn resolve_endpoint(
    session: &Session,
    service_type: &str,
    interface: EndpointInterface,
) -> Result<String, ClientError> {
    let svc = session.catalog.service(service_type).ok_or_else(|| {
        ClientError::Config(format!(
            "service type `{service_type}` is not in the Keystone catalog"
        ))
    })?;
    let ep = svc.endpoint(interface).or_else(|| {
        // Fall back to any advertised interface so a call still resolves when a
        // deployment only publishes, say, an admin endpoint.
        EndpointInterface::ALL.iter().find_map(|i| svc.endpoint(*i))
    });
    ep.map(|e| e.url.clone()).ok_or_else(|| {
        ClientError::Config(format!(
            "service `{service_type}` advertises no endpoint to reach"
        ))
    })
}

// ─────────────────────────── the HTTP production impl ───────────────────────────

/// The bound on one resource call.
pub const RESOURCE_TIMEOUT: Duration = Duration::from_secs(60);

/// Production [`ResourceApi`]: a Keystone-token-authed REST call against the
/// catalog endpoint.
///
/// Resolves the endpoint from the session catalog for the request's service type
/// and `interface`, builds the URL, sets `X-Auth-Token`, issues the verb's
/// method (attaching the JSON body for create/update), and returns the raw
/// status and body. Real code — IAC-3 wires it to the surface; the live
/// round-trip is smoked in IAC-6. Blocking (safe from the worker's
/// `spawn_blocking` drain).
#[derive(Debug, Clone)]
pub struct TokenRestApi {
    interface: EndpointInterface,
}

impl TokenRestApi {
    /// Construct the production API bound to `interface` (the catalog interface
    /// the client reaches services on — usually `public`).
    #[must_use]
    pub const fn new(interface: EndpointInterface) -> Self {
        Self { interface }
    }
}

impl ResourceApi for TokenRestApi {
    fn call(
        &self,
        session: &Session,
        req: &ResourceRequest,
    ) -> Result<ResourceResponse, ClientError> {
        if req.verb.needs_id() && req.target.id.as_deref().unwrap_or("").trim().is_empty() {
            return Err(ClientError::Config(format!(
                "the `{}` verb requires a resource id",
                req.verb.http_method()
            )));
        }
        let base = resolve_endpoint(session, &req.target.service_type, self.interface)?;
        let url = build_resource_url(&base, &req.target.collection, req.target.id.as_deref());

        let client = reqwest::blocking::Client::builder()
            .timeout(RESOURCE_TIMEOUT)
            .build()
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let method = reqwest::Method::from_bytes(req.verb.http_method().as_bytes())
            .map_err(|e| ClientError::Config(e.to_string()))?;
        let mut builder = client
            .request(method, &url)
            .header("X-Auth-Token", &session.token)
            .query(&req.query);
        if let Some(body) = &req.body {
            builder = builder.json(body);
        }
        let resp = builder.send().map_err(|e| {
            ClientError::Transport(format!("{} {url}: {e}", req.verb.http_method()))
        })?;
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        Ok(ResourceResponse { status, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::openstack::client::testkit::FakeResourceApi;
    use mackes_mesh_types::openstack::ServiceCatalog;

    fn session() -> Session {
        Session {
            token: "gAAAAtok".into(),
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
    fn verb_methods_and_flags() {
        assert_eq!(Verb::List.http_method(), "GET");
        assert_eq!(Verb::Create.http_method(), "POST");
        assert_eq!(Verb::Update.http_method(), "PUT");
        assert_eq!(Verb::Delete.http_method(), "DELETE");
        assert!(Verb::Delete.needs_id() && Verb::Delete.is_mutation());
        assert!(!Verb::List.needs_id() && !Verb::List.is_mutation());
        assert!(Verb::Create.is_mutation() && !Verb::Create.needs_id());
    }

    #[test]
    fn resource_url_joins_cleanly() {
        assert_eq!(
            build_resource_url("http://nova.mesh:8774/v2.1", "servers", None),
            "http://nova.mesh:8774/v2.1/servers"
        );
        // Trailing/leading slashes collapse to one separator.
        assert_eq!(
            build_resource_url("http://nova.mesh:8774/v2.1/", "/servers/", Some("i-9")),
            "http://nova.mesh:8774/v2.1/servers/i-9"
        );
        // A blank id degrades to the collection URL.
        assert_eq!(
            build_resource_url("http://x/v1", "vols", Some("  ")),
            "http://x/v1/vols"
        );
    }

    #[test]
    fn resolves_a_cataloged_endpoint_and_errors_on_an_uncataloged_one() {
        let s = session();
        assert_eq!(
            resolve_endpoint(&s, "compute", EndpointInterface::Public).unwrap(),
            "http://nova.mesh:8774/v2.1"
        );
        // An internal probe falls back to the public endpoint.
        assert_eq!(
            resolve_endpoint(&s, "compute", EndpointInterface::Internal).unwrap(),
            "http://nova.mesh:8774/v2.1"
        );
        let err = resolve_endpoint(&s, "orchestration", EndpointInterface::Public)
            .expect_err("heat isn't cataloged");
        assert!(err.to_string().contains("not in the Keystone catalog"));
    }

    #[test]
    fn the_fake_seam_records_the_typed_request() {
        // IAC-3 drives the real TokenRestApi; the fake proves the seam shape.
        let api = FakeResourceApi::new().answer(200, r#"{"servers":[]}"#);
        let req = ResourceRequest {
            verb: Verb::List,
            target: ResourceRef {
                service_type: "compute".into(),
                collection: "servers".into(),
                id: None,
            },
            body: None,
            query: vec![("status".into(), "ACTIVE".into())],
        };
        let resp = api.call(&session(), &req).expect("call");
        assert!(resp.is_success());
        assert_eq!(
            api.calls(),
            vec!["GET compute/servers?status=ACTIVE".to_string()]
        );
    }

    #[test]
    fn a_single_resource_verb_without_an_id_is_rejected_by_the_fake() {
        let api = FakeResourceApi::new();
        let req = ResourceRequest {
            verb: Verb::Delete,
            target: ResourceRef {
                service_type: "compute".into(),
                collection: "servers".into(),
                id: None,
            },
            body: None,
            query: vec![],
        };
        let err = api.call(&session(), &req).expect_err("delete needs an id");
        assert!(err.to_string().contains("requires a resource id"));
    }
}
