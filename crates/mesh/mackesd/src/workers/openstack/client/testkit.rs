//! IAC-1 — in-memory fakes for the client's three seams (Keystone auth, endpoint
//! probe, resource call) + the catalog-source seam the QC-11 `get-catalog` verb
//! drives, so the whole client foundation runs headless against fixtures (no
//! live cloud). One fake per seam, shared across the client's test modules.

use std::collections::BTreeMap;
use std::sync::Mutex;

use mackes_mesh_types::openstack::{ProbeOutcome, ServiceCatalog, ServiceHealth};

use super::config::CloudConfig;
use super::health::EndpointProbe;
use super::keystone::{KeystoneAuth, Session};
use super::resource::{ResourceApi, ResourceRequest, ResourceResponse};
use super::{CatalogHealth, CatalogSource, ClientError};

/// An in-memory [`KeystoneAuth`]: answers a canned [`Session`] or a typed error.
pub struct FakeKeystone {
    result: Result<Session, ClientError>,
}

impl FakeKeystone {
    /// Answer `session` on every authenticate.
    #[must_use]
    pub fn ok(session: Session) -> Self {
        Self {
            result: Ok(session),
        }
    }

    /// Answer a typed error on every authenticate.
    #[must_use]
    pub fn failing(err: ClientError) -> Self {
        Self { result: Err(err) }
    }
}

impl KeystoneAuth for FakeKeystone {
    fn authenticate(&self, _cfg: &CloudConfig) -> Result<Session, ClientError> {
        self.result.clone()
    }
}

/// An in-memory [`EndpointProbe`]: a per-URL canned [`ProbeOutcome`]; an unseeded
/// URL answers `Unreachable` (an endpoint that simply didn't respond).
pub struct FakeProbe {
    outcomes: BTreeMap<String, ProbeOutcome>,
}

impl FakeProbe {
    /// A probe with no seeded outcomes (every URL unreachable).
    #[must_use]
    pub fn new() -> Self {
        Self {
            outcomes: BTreeMap::new(),
        }
    }

    /// Seed `url` as reachable with `body` and `elapsed_ms` (HTTP 200).
    #[must_use]
    pub fn up(mut self, url: &str, body: &str, elapsed_ms: u64) -> Self {
        self.outcomes.insert(
            url.to_string(),
            ProbeOutcome::Reachable {
                http_status: 200,
                body: body.to_string(),
                elapsed_ms,
            },
        );
        self
    }

    /// Seed `url` as unreachable with `reason` and `elapsed_ms`.
    #[must_use]
    pub fn unreachable(mut self, url: &str, reason: &str, elapsed_ms: u64) -> Self {
        self.outcomes.insert(
            url.to_string(),
            ProbeOutcome::Unreachable {
                elapsed_ms,
                reason: reason.to_string(),
            },
        );
        self
    }
}

impl EndpointProbe for FakeProbe {
    fn probe(&self, url: &str) -> ProbeOutcome {
        self.outcomes
            .get(url)
            .cloned()
            .unwrap_or_else(|| ProbeOutcome::Unreachable {
                elapsed_ms: 0,
                reason: "no route (test fake: unseeded URL)".to_string(),
            })
    }
}

/// An in-memory [`ResourceApi`] recording each typed call, enforcing the same
/// id-required gate the production impl does, and answering a canned response.
pub struct FakeResourceApi {
    status: u16,
    body: String,
    calls: Mutex<Vec<String>>,
}

impl FakeResourceApi {
    /// A fake answering `HTTP 200 {}`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            status: 200,
            body: "{}".to_string(),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Set the canned response.
    #[must_use]
    pub fn answer(mut self, status: u16, body: &str) -> Self {
        self.status = status;
        self.body = body.to_string();
        self
    }

    /// The recorded call log (`<METHOD> <service>/<collection>[/<id>][?q=v]`).
    #[must_use]
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl ResourceApi for FakeResourceApi {
    fn call(
        &self,
        _session: &Session,
        req: &ResourceRequest,
    ) -> Result<ResourceResponse, ClientError> {
        if req.verb.needs_id() && req.target.id.as_deref().unwrap_or("").trim().is_empty() {
            return Err(ClientError::Config(format!(
                "the `{}` verb requires a resource id",
                req.verb.http_method()
            )));
        }
        let mut path = format!("{}/{}", req.target.service_type, req.target.collection);
        if let Some(id) = req.target.id.as_deref().filter(|s| !s.trim().is_empty()) {
            path.push('/');
            path.push_str(id);
        }
        if !req.query.is_empty() {
            let q = req
                .query
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");
            path.push('?');
            path.push_str(&q);
        }
        self.calls
            .lock()
            .unwrap()
            .push(format!("{} {path}", req.verb.http_method()));
        Ok(ResourceResponse {
            status: self.status,
            body: self.body.clone(),
        })
    }
}

/// An in-memory [`CatalogSource`] for the QC-11 `get-catalog` verb tests: a
/// canned catalog+health, an honest unconfigured gate, or a typed failure.
pub struct FakeCatalogSource {
    result: Result<CatalogHealth, ClientError>,
}

impl FakeCatalogSource {
    /// Answer `catalog` + `health` on every call.
    #[must_use]
    pub fn ok(catalog: ServiceCatalog, health: Vec<ServiceHealth>) -> Self {
        Self {
            result: Ok(CatalogHealth { catalog, health }),
        }
    }

    /// Answer the honest "no clouds.yaml on this node" gate.
    #[must_use]
    pub fn unconfigured() -> Self {
        Self {
            result: Err(ClientError::Unconfigured(
                "test fake: no clouds.yaml".to_string(),
            )),
        }
    }

    /// Answer a typed failure (auth/transport) — a real error, not a gate.
    #[must_use]
    pub fn failing(err: ClientError) -> Self {
        Self { result: Err(err) }
    }
}

impl CatalogSource for FakeCatalogSource {
    fn catalog_and_health(&self) -> Result<CatalogHealth, ClientError> {
        self.result.clone()
    }
}
