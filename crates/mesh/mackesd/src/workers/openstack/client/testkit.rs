//! IAC-1 — in-memory fakes for the client's three seams (Keystone auth, endpoint
//! probe, resource call) + the catalog-source seam the QC-11 `get-catalog` verb
//! drives, so the whole client foundation runs headless against fixtures (no
//! live cloud). One fake per seam, shared across the client's test modules.

use std::collections::BTreeMap;
use std::sync::Mutex;

use mackes_mesh_types::openstack::{
    HeatPreview, HeatStackDetail, ProbeOutcome, ResourceTable, ServiceCatalog, ServiceHealth,
};

use super::config::CloudConfig;
use super::health::EndpointProbe;
use super::keystone::{KeystoneAuth, Session};
use super::resource::{ResourceApi, ResourceRequest, ResourceResponse};
use super::{CatalogHealth, CatalogSource, ClientError, HeatSource, ResourceSource};

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

/// An in-memory [`CloudClient`](super::CloudClient) for the cloud-verb tests: a
/// canned catalog+health (`get-catalog`), a canned resource table
/// (`list-resources`), **and** canned Heat answers (`heat-*`, IAC-4) — an honest
/// unconfigured gate or a typed failure on every seam. One fake drives every seam
/// the responder threads, and records the Heat calls it received.
pub struct FakeCatalogSource {
    result: Result<CatalogHealth, ClientError>,
    resources: Result<ResourceTable, ClientError>,
    heat_detail: Result<HeatStackDetail, ClientError>,
    heat_preview: Result<HeatPreview, ClientError>,
    heat_reverse: Result<String, ClientError>,
    heat_mutation: Result<String, ClientError>,
    heat_calls: Mutex<Vec<String>>,
}

impl FakeCatalogSource {
    /// Answer `catalog` + `health` on every `get-catalog`; an empty table on
    /// every `list-resources`; and honest empty Heat answers (override the Heat
    /// ones with the `with_heat_*` builders).
    #[must_use]
    pub fn ok(catalog: ServiceCatalog, health: Vec<ServiceHealth>) -> Self {
        Self {
            result: Ok(CatalogHealth { catalog, health }),
            resources: Ok(ResourceTable::default()),
            heat_detail: Ok(HeatStackDetail::default()),
            heat_preview: Ok(HeatPreview::default()),
            heat_reverse: Ok(String::new()),
            heat_mutation: Ok(String::new()),
            heat_calls: Mutex::new(Vec::new()),
        }
    }

    /// Answer the honest "no clouds.yaml on this node" gate on every seam.
    #[must_use]
    pub fn unconfigured() -> Self {
        let gate = || ClientError::Unconfigured("test fake: no clouds.yaml".to_string());
        Self {
            result: Err(gate()),
            resources: Err(gate()),
            heat_detail: Err(gate()),
            heat_preview: Err(gate()),
            heat_reverse: Err(gate()),
            heat_mutation: Err(gate()),
            heat_calls: Mutex::new(Vec::new()),
        }
    }

    /// Answer a typed failure (auth/transport) on every seam — a real error, not
    /// a gate.
    #[must_use]
    pub fn failing(err: ClientError) -> Self {
        Self {
            result: Err(err.clone()),
            resources: Err(err.clone()),
            heat_detail: Err(err.clone()),
            heat_preview: Err(err.clone()),
            heat_reverse: Err(err.clone()),
            heat_mutation: Err(err),
            heat_calls: Mutex::new(Vec::new()),
        }
    }

    /// Override the canned `list-resources` answer (a fixture resource table).
    #[must_use]
    pub fn with_resources(mut self, table: ResourceTable) -> Self {
        self.resources = Ok(table);
        self
    }

    /// Override the canned `heat-show` detail.
    #[must_use]
    pub fn with_heat_detail(mut self, detail: HeatStackDetail) -> Self {
        self.heat_detail = Ok(detail);
        self
    }

    /// Override the canned `heat-preview` diff.
    #[must_use]
    pub fn with_heat_preview(mut self, preview: HeatPreview) -> Self {
        self.heat_preview = Ok(preview);
        self
    }

    /// Override the canned `heat-reverse` HOT template.
    #[must_use]
    pub fn with_heat_reverse(mut self, hot: impl Into<String>) -> Self {
        self.heat_reverse = Ok(hot.into());
        self
    }

    /// The recorded Heat call log (`<verb> <args>`).
    #[must_use]
    pub fn heat_calls(&self) -> Vec<String> {
        self.heat_calls.lock().unwrap().clone()
    }

    fn record(&self, call: impl Into<String>) {
        self.heat_calls.lock().unwrap().push(call.into());
    }
}

impl CatalogSource for FakeCatalogSource {
    fn catalog_and_health(&self) -> Result<CatalogHealth, ClientError> {
        self.result.clone()
    }
}

impl ResourceSource for FakeCatalogSource {
    fn list_resources(
        &self,
        _service_type: &str,
        _collection: &str,
        _query: &[(String, String)],
    ) -> Result<ResourceTable, ClientError> {
        self.resources.clone()
    }
}

impl HeatSource for FakeCatalogSource {
    fn heat_show(&self, stack: &str) -> Result<HeatStackDetail, ClientError> {
        self.record(format!("show:{stack}"));
        self.heat_detail.clone()
    }

    fn heat_preview(
        &self,
        stack_name: &str,
        stack_id: &str,
        _template: &str,
    ) -> Result<HeatPreview, ClientError> {
        self.record(format!("preview:{stack_name}/{stack_id}"));
        self.heat_preview.clone()
    }

    fn heat_check(&self, stack_name: &str, stack_id: &str) -> Result<(), ClientError> {
        self.record(format!("check:{stack_name}/{stack_id}"));
        self.heat_mutation.clone().map(|_| ())
    }

    fn heat_create(&self, stack_name: &str, _template: &str) -> Result<String, ClientError> {
        self.record(format!("create:{stack_name}"));
        self.heat_mutation.clone()
    }

    fn heat_update(
        &self,
        stack_name: &str,
        stack_id: &str,
        _template: &str,
    ) -> Result<(), ClientError> {
        self.record(format!("update:{stack_name}/{stack_id}"));
        self.heat_mutation.clone().map(|_| ())
    }

    fn heat_delete(&self, stack_name: &str, stack_id: &str) -> Result<(), ClientError> {
        self.record(format!("delete:{stack_name}/{stack_id}"));
        self.heat_mutation.clone().map(|_| ())
    }

    fn heat_reverse(&self, services: &[(String, String)]) -> Result<String, ClientError> {
        self.record(format!("reverse:{}", services.len()));
        self.heat_reverse.clone()
    }
}
