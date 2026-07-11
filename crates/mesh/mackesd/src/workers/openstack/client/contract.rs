//! IAC-1 — **contract tests** for the `OpenStack` REST + CLI control plane.
//!
//! The rest of the `openstack` suite exercises the client against its own
//! in-repo fakes (`client/testkit.rs`, `testkit.rs`); those prove the wiring but
//! can't prove the client's request shapes and response parsers still match the
//! **real** `OpenStack` wire format — a fake can drift from the API and every
//! fake-backed test still passes (test-obs-5).
//!
//! This module closes that gap with **canonical, spec-accurate fixtures**
//! (`contract_fixtures/*.json`, real captured/reference `OpenStack` payloads) and
//! three layers of assertion, none of which needs a live cloud:
//!
//! 1. **Request-shape contracts (pure)** — the pure request builders
//!    ([`token_url`], [`build_password_auth_body`], [`build_resource_url`] +
//!    [`resolve_endpoint`] + [`default_collection`], [`Verb::http_method`]) emit
//!    the spec-correct URLs/methods/bodies.
//! 2. **Request+response contracts (loopback)** — a tiny in-process HTTP server
//!    drives the **production** [`KeystoneHttp`], [`TokenRestApi`], and Heat
//!    request builders end-to-end, capturing the real request (method, path,
//!    `X-Auth-Token`/`Content-Type` headers, JSON body) and answering a canonical
//!    fixture, so the request shape **and** the inline Heat body assembly are
//!    pinned against the wire.
//! 3. **Response-parser contracts** — the parsers
//!    ([`ServiceCatalog::from_keystone_token_json`],
//!    [`ResourceTable::from_collection_json`], [`shape_health`],
//!    [`HeatStackDetail`], [`HeatPreview`], and the `verbs` CLI parsers) accept
//!    the canonical fixtures and extract the right values.
//!
//! A live-gated skeleton ([`live_openstack_catalog_and_resources`]) exercises the
//! real API when `MDE_OPENSTACK_LIVE_TARGET` points at a `clouds.yaml`, mirroring
//! the env-gated VDI/console live suites; it is `#[ignore]` so CI stays offline.
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::missing_panics_doc,
    reason = "test-only: a contract-probe failure must abort with typed evidence, \
              and unwrap/panic IS the test failure mechanism"
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use mackes_mesh_types::openstack::{
    default_collection, shape_health, EndpointInterface, HealthState, HeatPreview, HeatStackDetail,
    ProbeOutcome, ResourceTable, ServiceCatalog,
};

use super::config::CloudConfig;
use super::keystone::{build_password_auth_body, token_url, KeystoneAuth, KeystoneHttp, Session};
use super::resource::{
    build_resource_url, resolve_endpoint, ResourceApi, ResourceRef, ResourceRequest, TokenRestApi,
    Verb,
};
use super::testkit::{FakeKeystone, FakeProbe};
use super::{HeatSource, OpenStackClient};

// ───────────────────────────── canonical fixtures ─────────────────────────────

mod fx {
    //! Canonical, spec-accurate `OpenStack` API payloads (real reference shapes).
    pub const KEYSTONE_V3_TOKEN: &str = include_str!("contract_fixtures/keystone_v3_token.json");
    pub const KEYSTONE_VERSION: &str = include_str!("contract_fixtures/keystone_version.json");
    pub const NOVA_VERSION: &str = include_str!("contract_fixtures/nova_version.json");
    pub const NOVA_SERVERS_DETAIL: &str =
        include_str!("contract_fixtures/nova_servers_detail.json");
    pub const NEUTRON_NETWORKS: &str = include_str!("contract_fixtures/neutron_networks.json");
    pub const GLANCE_IMAGES: &str = include_str!("contract_fixtures/glance_images.json");
    pub const CINDER_VOLUMES_DETAIL: &str =
        include_str!("contract_fixtures/cinder_volumes_detail.json");
    pub const HEAT_STACK_SHOW: &str = include_str!("contract_fixtures/heat_stack_show.json");
    pub const HEAT_STACK_RESOURCES: &str =
        include_str!("contract_fixtures/heat_stack_resources.json");
    pub const HEAT_STACK_EVENTS: &str = include_str!("contract_fixtures/heat_stack_events.json");
    pub const HEAT_STACK_TEMPLATE: &str =
        include_str!("contract_fixtures/heat_stack_template.json");
    pub const HEAT_PREVIEW_UPDATE: &str =
        include_str!("contract_fixtures/heat_preview_update.json");
    pub const HEAT_STACK_CREATE: &str = include_str!("contract_fixtures/heat_stack_create.json");
    pub const CLI_SERVER_LIST: &str = include_str!("contract_fixtures/cli_server_list.json");
    pub const CLI_CONSOLE_URL: &str = include_str!("contract_fixtures/cli_console_url.json");
}

/// Every fixture must at least be well-formed JSON — a malformed fixture would
/// make a contract test lie about what the real API returns.
#[test]
fn all_fixtures_are_valid_json() {
    for (name, body) in [
        ("keystone_v3_token", fx::KEYSTONE_V3_TOKEN),
        ("keystone_version", fx::KEYSTONE_VERSION),
        ("nova_version", fx::NOVA_VERSION),
        ("nova_servers_detail", fx::NOVA_SERVERS_DETAIL),
        ("neutron_networks", fx::NEUTRON_NETWORKS),
        ("glance_images", fx::GLANCE_IMAGES),
        ("cinder_volumes_detail", fx::CINDER_VOLUMES_DETAIL),
        ("heat_stack_show", fx::HEAT_STACK_SHOW),
        ("heat_stack_resources", fx::HEAT_STACK_RESOURCES),
        ("heat_stack_events", fx::HEAT_STACK_EVENTS),
        ("heat_stack_template", fx::HEAT_STACK_TEMPLATE),
        ("heat_preview_update", fx::HEAT_PREVIEW_UPDATE),
        ("heat_stack_create", fx::HEAT_STACK_CREATE),
        ("cli_server_list", fx::CLI_SERVER_LIST),
        ("cli_console_url", fx::CLI_CONSOLE_URL),
    ] {
        serde_json::from_str::<serde_json::Value>(body)
            .unwrap_or_else(|e| panic!("fixture `{name}.json` is not valid JSON: {e}"));
    }
}

// ─────────────────────── 1. request-shape contracts (pure) ───────────────────────

fn cfg_scoped() -> CloudConfig {
    CloudConfig {
        cloud: "mesh".into(),
        auth_url: "http://keystone.mesh:5000/v3".into(),
        username: "operator".into(),
        password: "s3cr3t".into(),
        project_name: Some("mesh".into()),
        project_domain: "Default".into(),
        user_domain: "Default".into(),
        region_name: Some("RegionOne".into()),
        interface: EndpointInterface::Public,
    }
}

#[test]
fn keystone_token_url_matches_the_v3_spec_path() {
    // Keystone v3 authenticates at `POST {identity_root}/auth/tokens`.
    assert_eq!(
        token_url("http://keystone.mesh:5000/v3"),
        "http://keystone.mesh:5000/v3/auth/tokens"
    );
    // An unversioned identity root gets `/v3` inserted (openstacksdk convention).
    assert_eq!(
        token_url("http://keystone.mesh:5000"),
        "http://keystone.mesh:5000/v3/auth/tokens"
    );
}

#[test]
fn keystone_password_body_matches_the_v3_identity_spec() {
    // The exact scoped-password request body the Keystone Identity v3 API
    // documents for `POST /v3/auth/tokens` (methods=[password], user w/ domain,
    // project scope w/ domain). A drift here (renamed key, wrong nesting) would
    // make every real auth 400 while the fakes stayed green.
    let body = build_password_auth_body(&cfg_scoped());
    let expected = serde_json::json!({
        "auth": {
            "identity": {
                "methods": ["password"],
                "password": {
                    "user": {
                        "name": "operator",
                        "domain": { "name": "Default" },
                        "password": "s3cr3t"
                    }
                }
            },
            "scope": {
                "project": {
                    "name": "mesh",
                    "domain": { "name": "Default" }
                }
            }
        }
    });
    assert_eq!(body, expected);
}

#[test]
fn resource_urls_match_the_per_service_rest_spec() {
    // The catalog endpoint bases a standard Kolla cloud advertises + the
    // `default_collection` the client appends → the exact REST URL per service.
    let session = session_with_catalog(
        r#"{"token":{"catalog":[
            {"type":"compute","endpoints":[{"interface":"public","url":"http://nova.mesh:8774/v2.1"}]},
            {"type":"network","endpoints":[{"interface":"public","url":"http://neutron.mesh:9696"}]},
            {"type":"image","endpoints":[{"interface":"public","url":"http://glance.mesh:9292"}]},
            {"type":"volumev3","endpoints":[{"interface":"public","url":"http://cinder.mesh:8776/v3/proj"}]},
            {"type":"orchestration","endpoints":[{"interface":"public","url":"http://heat.mesh:8004/v1/proj"}]}
        ]}}"#,
    );
    let url_for = |service: &str| -> String {
        let base = resolve_endpoint(&session, service, EndpointInterface::Public).unwrap();
        let collection = default_collection(service).unwrap();
        build_resource_url(&base, collection, None)
    };
    assert_eq!(
        url_for("compute"),
        "http://nova.mesh:8774/v2.1/servers/detail"
    );
    assert_eq!(url_for("network"), "http://neutron.mesh:9696/v2.0/networks");
    assert_eq!(url_for("image"), "http://glance.mesh:9292/v2/images");
    assert_eq!(
        url_for("volumev3"),
        "http://cinder.mesh:8776/v3/proj/volumes/detail"
    );
    // Heat's list collection is the bare `stacks` under the versioned base.
    assert_eq!(
        url_for("orchestration"),
        "http://heat.mesh:8004/v1/proj/stacks"
    );
}

#[test]
fn crud_verbs_map_to_the_spec_http_methods() {
    // The standard OpenStack REST CRUD → HTTP method mapping.
    assert_eq!(Verb::List.http_method(), "GET");
    assert_eq!(Verb::Show.http_method(), "GET");
    assert_eq!(Verb::Create.http_method(), "POST");
    assert_eq!(Verb::Update.http_method(), "PUT");
    assert_eq!(Verb::Delete.http_method(), "DELETE");
}

// ───────────────────── 2. request+response contracts (loopback) ─────────────────────

/// One HTTP request the loopback server captured.
struct Captured {
    method: String,
    /// Path + query, as sent on the wire (origin-form).
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Captured {
    fn header(&self, name: &str) -> Option<&str> {
        let want = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == want)
            .map(|(_, v)| v.as_str())
    }
    /// The path with any query string stripped.
    fn path_only(&self) -> &str {
        self.path.split('?').next().unwrap_or(&self.path)
    }
    fn json_body(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).expect("request body is JSON")
    }
}

/// A canned response the loopback server returns for one request.
struct Canned {
    status: u16,
    body: String,
    extra_headers: Vec<(String, String)>,
}

impl Canned {
    fn ok(body: &str) -> Self {
        Self {
            status: 200,
            body: body.to_string(),
            extra_headers: Vec::new(),
        }
    }
    fn with_header(mut self, name: &str, value: &str) -> Self {
        self.extra_headers
            .push((name.to_string(), value.to_string()));
        self
    }
}

/// A minimal in-process HTTP/1.1 server that answers a fixed, ordered sequence of
/// canned responses — one per incoming request — capturing each request for
/// assertion. It needs no external deps and lets the **production** reqwest-based
/// request builders run end-to-end against canonical fixtures with no live cloud.
struct MockServer {
    base_url: String,
    handle: Option<thread::JoinHandle<Vec<Captured>>>,
}

impl MockServer {
    /// Bind a loopback listener and serve `responses` in order on a background
    /// thread. The client (a fresh `reqwest::blocking::Client` per call) opens one
    /// connection per request, so the responses are consumed in request order.
    fn start(responses: Vec<Canned>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let handle = thread::spawn(move || {
            let mut captured = Vec::new();
            let start = Instant::now();
            let mut i = 0;
            while i < responses.len() {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).ok();
                        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
                        let cap = read_request(&stream);
                        write_response(&stream, &responses[i]);
                        captured.push(cap);
                        i += 1;
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        // Never hang forever if the client makes fewer requests
                        // than expected — bail so the test fails on the captured
                        // count instead of deadlocking.
                        if start.elapsed() > Duration::from_secs(15) {
                            break;
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
            captured
        });
        Self {
            base_url,
            handle: Some(handle),
        }
    }

    fn finish(mut self) -> Vec<Captured> {
        self.handle
            .take()
            .unwrap()
            .join()
            .expect("mock server thread")
    }
}

fn read_request(stream: &TcpStream) -> Captured {
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).expect("request line");
    let mut parts = request_line.trim_end().split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();

    let mut headers = Vec::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).expect("header line");
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if n == 0 || trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            let name = k.trim().to_ascii_lowercase();
            let value = v.trim().to_string();
            if name == "content-length" {
                content_length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
        }
    }
    let mut body = String::new();
    if content_length > 0 {
        let mut buf = vec![0u8; content_length];
        reader.read_exact(&mut buf).expect("request body");
        body = String::from_utf8_lossy(&buf).into_owned();
    }
    Captured {
        method,
        path,
        headers,
        body,
    }
}

fn write_response(stream: &TcpStream, canned: &Canned) {
    let mut head = format!(
        "HTTP/1.1 {} OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n",
        canned.status,
        canned.body.len()
    );
    for (k, v) in &canned.extra_headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let mut w: &TcpStream = stream;
    w.write_all(head.as_bytes()).expect("write head");
    w.write_all(canned.body.as_bytes()).expect("write body");
    w.flush().ok();
}

/// A [`Session`] whose catalog is parsed from `catalog_json` — the seam that
/// resolves the endpoint the request goes to.
fn session_with_catalog(catalog_json: &str) -> Session {
    Session {
        token: "gAAAAA_contract_token".into(),
        catalog: ServiceCatalog::from_keystone_token_json(catalog_json).unwrap(),
        expires_at: None,
    }
}

/// A [`Session`] cataloging one `service_type` at `url` (the loopback base).
fn session_pointing(service_type: &str, url: &str) -> Session {
    let catalog = format!(
        r#"{{"token":{{"catalog":[{{"type":"{service_type}","endpoints":[
            {{"interface":"public","url":"{url}"}}]}}]}}}}"#
    );
    session_with_catalog(&catalog)
}

#[test]
fn keystone_authenticate_emits_the_spec_request_and_parses_the_catalog() {
    // Drive the PRODUCTION KeystoneHttp against a loopback answering the canonical
    // v3 token response (token in the X-Subject-Token header, catalog in the body).
    let server = MockServer::start(vec![
        Canned::ok(fx::KEYSTONE_V3_TOKEN).with_header("X-Subject-Token", "gAAAAA_live_token")
    ]);
    let mut cfg = cfg_scoped();
    cfg.auth_url = format!("{}/v3", server.base_url);

    let session = KeystoneHttp::new()
        .authenticate(&cfg)
        .expect("authenticate");
    let reqs = server.finish();

    // The request the client actually put on the wire.
    assert_eq!(reqs.len(), 1);
    let req = &reqs[0];
    assert_eq!(req.method, "POST");
    assert_eq!(req.path_only(), "/v3/auth/tokens");
    assert_eq!(
        req.header("content-type").map(str::to_ascii_lowercase),
        Some("application/json".into())
    );
    let sent = req.json_body();
    assert_eq!(sent["auth"]["identity"]["methods"][0], "password");
    assert_eq!(
        sent["auth"]["identity"]["password"]["user"]["name"],
        "operator"
    );
    assert_eq!(sent["auth"]["scope"]["project"]["name"], "mesh");

    // The response the client parsed: token from the header, catalog from the body.
    assert_eq!(session.token, "gAAAAA_live_token");
    assert_eq!(
        session.expires_at.as_deref(),
        Some("2026-07-10T13:00:00.000000Z")
    );
    assert_eq!(session.catalog.services.len(), 6);
    let nova = session
        .catalog
        .service("compute")
        .expect("compute cataloged");
    assert_eq!(nova.endpoints.len(), 3, "public/internal/admin");
    assert_eq!(
        nova.endpoint(EndpointInterface::Public).unwrap().url,
        "http://10.0.0.5:8774/v2.1"
    );
}

#[test]
fn nova_list_emits_a_token_authed_get_and_parses_the_server_detail() {
    // The production TokenRestApi issues the real GET with the X-Auth-Token header
    // + the query, then the parser turns the canonical Nova body into a table.
    let server = MockServer::start(vec![Canned::ok(fx::NOVA_SERVERS_DETAIL)]);
    let session = session_pointing("compute", &format!("{}/v2.1", server.base_url));
    let req = ResourceRequest {
        verb: Verb::List,
        target: ResourceRef {
            service_type: "compute".into(),
            collection: "servers/detail".into(),
            id: None,
        },
        body: None,
        query: vec![("status".into(), "ACTIVE".into())],
    };
    let resp = TokenRestApi::new(EndpointInterface::Public)
        .call(&session, &req)
        .expect("nova list call");
    let reqs = server.finish();

    let got = &reqs[0];
    assert_eq!(got.method, "GET");
    assert_eq!(got.path_only(), "/v2.1/servers/detail");
    assert!(
        got.path.contains("status=ACTIVE"),
        "query on the wire: {}",
        got.path
    );
    // The Keystone bearer rides X-Auth-Token on every resource call (Q20).
    assert_eq!(got.header("x-auth-token"), Some("gAAAAA_contract_token"));
    assert!(resp.is_success());

    let table = ResourceTable::from_collection_json("compute", "servers/detail", &resp.body)
        .expect("parse nova detail");
    assert_eq!(table.rows.len(), 2);
    assert_eq!(table.columns.first().map(String::as_str), Some("name"));
    assert_eq!(table.columns.get(1).map(String::as_str), Some("status"));
    assert_eq!(table.row_label(&table.rows[0]), "web-01");
    assert_eq!(table.rows[0].id, "9168b536-cd40-4630-b43f-b259807c6e87");
    assert_eq!(table.row_label(&table.rows[1]), "db-01");
}

#[test]
fn heat_show_walks_the_stack_subresources_and_folds_the_detail() {
    // heat_show issues four sequential GETs: the stack, then resources/events/
    // template on the canonical name/id path. Pin each request path + the fold.
    let server = MockServer::start(vec![
        Canned::ok(fx::HEAT_STACK_SHOW),
        Canned::ok(fx::HEAT_STACK_RESOURCES),
        Canned::ok(fx::HEAT_STACK_EVENTS),
        Canned::ok(fx::HEAT_STACK_TEMPLATE),
    ]);
    let session = session_pointing("orchestration", &format!("{}/v1/proj", server.base_url));
    let client = OpenStackClient::new(
        cfg_scoped(),
        Box::new(FakeKeystone::ok(session)),
        Box::new(FakeProbe::new()),
    );

    let stack_id = "3095aefc-09fb-4bc7-b1f0-f21a304e864c";
    let detail = client.heat_show(stack_id).expect("heat show");
    let reqs = server.finish();

    let paths: Vec<&str> = reqs.iter().map(Captured::path_only).collect();
    assert_eq!(paths[0], format!("/v1/proj/stacks/{stack_id}"));
    assert_eq!(
        paths[1],
        format!("/v1/proj/stacks/mesh-overlay-net/{stack_id}/resources")
    );
    assert_eq!(
        paths[2],
        format!("/v1/proj/stacks/mesh-overlay-net/{stack_id}/events")
    );
    assert_eq!(
        paths[3],
        format!("/v1/proj/stacks/mesh-overlay-net/{stack_id}/template")
    );
    assert!(reqs.iter().all(|r| r.method == "GET"));
    assert!(reqs
        .iter()
        .all(|r| r.header("x-auth-token") == Some("gAAAAA_contract_token")));

    assert_eq!(detail.stack_name, "mesh-overlay-net");
    assert_eq!(detail.stack_id, stack_id);
    assert_eq!(detail.status, "UPDATE_COMPLETE");
    assert_eq!(detail.outputs.len(), 2);
    assert_eq!(detail.resources.len(), 2);
    assert_eq!(detail.resources[0].resource_type, "OS::Neutron::Net");
    assert_eq!(
        detail.resources[0].physical_id,
        "d32019d3-bc6e-4319-9c1d-6722fc136a22"
    );
    assert_eq!(detail.events.len(), 2);
    assert!(detail.template.contains("heat_template_version"));
}

#[test]
fn heat_create_posts_the_spec_body_and_returns_the_new_stack_id() {
    // Pin the inline POST /stacks body (stack_name + template) + the id parse.
    let server = MockServer::start(vec![Canned::ok(fx::HEAT_STACK_CREATE)]);
    let session = session_pointing("orchestration", &format!("{}/v1/proj", server.base_url));
    let client = OpenStackClient::new(
        cfg_scoped(),
        Box::new(FakeKeystone::ok(session)),
        Box::new(FakeProbe::new()),
    );

    let template = "heat_template_version: 2021-04-16\nresources: {}\n";
    let id = client
        .heat_create("mesh-overlay-net", template)
        .expect("heat create");
    let reqs = server.finish();

    let req = &reqs[0];
    assert_eq!(req.method, "POST");
    assert_eq!(req.path_only(), "/v1/proj/stacks");
    let sent = req.json_body();
    assert_eq!(sent["stack_name"], "mesh-overlay-net");
    // A YAML template rides as a string field Heat parses.
    assert!(sent["template"]
        .as_str()
        .is_some_and(|t| t.contains("heat_template_version")));
    // The create response's stack.id is what the client returns.
    assert_eq!(id, "3095aefc-09fb-4bc7-b1f0-f21a304e864c");
}

#[test]
fn heat_preview_puts_to_the_preview_path_and_parses_the_change_diff() {
    // Pin the PUT .../preview request + the canonical resource_changes parse.
    let server = MockServer::start(vec![Canned::ok(fx::HEAT_PREVIEW_UPDATE)]);
    let session = session_pointing("orchestration", &format!("{}/v1/proj", server.base_url));
    let client = OpenStackClient::new(
        cfg_scoped(),
        Box::new(FakeKeystone::ok(session)),
        Box::new(FakeProbe::new()),
    );

    let stack_id = "3095aefc-09fb-4bc7-b1f0-f21a304e864c";
    let preview = client
        .heat_preview(
            "mesh-overlay-net",
            stack_id,
            "heat_template_version: 2021-04-16\n",
        )
        .expect("heat preview");
    let reqs = server.finish();

    let req = &reqs[0];
    assert_eq!(req.method, "PUT");
    assert_eq!(
        req.path_only(),
        format!("/v1/proj/stacks/mesh-overlay-net/{stack_id}/preview")
    );
    assert_eq!(req.json_body()["stack_name"], "mesh-overlay-net");

    assert_eq!(preview.added, vec!["overlay_router"]);
    assert_eq!(preview.replaced, vec!["overlay_subnet"]);
    assert_eq!(preview.unchanged, vec!["overlay_net"]);
    assert!(preview.deleted.is_empty() && preview.updated.is_empty());
    // change_count excludes `unchanged`.
    assert_eq!(preview.change_count(), 2);
}

// ─────────────────────── 3. response-parser contracts ───────────────────────

#[test]
fn keystone_catalog_parses_all_six_kolla_services() {
    let cat = ServiceCatalog::from_keystone_token_json(fx::KEYSTONE_V3_TOKEN).expect("parse");
    assert_eq!(cat.services.len(), 6);
    assert_eq!(cat.region.as_deref(), Some("RegionOne"));
    for svc in [
        "compute",
        "network",
        "image",
        "volumev3",
        "orchestration",
        "identity",
    ] {
        let s = cat
            .service(svc)
            .unwrap_or_else(|| panic!("{svc} cataloged"));
        assert!(
            s.endpoint(EndpointInterface::Public).is_some(),
            "{svc} advertises a public endpoint"
        );
    }
}

#[test]
fn nova_version_document_yields_the_current_max_microversion() {
    // Real Nova's version list carries the max microversion in `version` (there is
    // no `max_version` key at the list root), so the parser's fallback must reach
    // it — a canonical fixture proves that path.
    let h = shape_health(
        "compute",
        EndpointInterface::Public,
        "http://10.0.0.5:8774/",
        &ProbeOutcome::Reachable {
            http_status: 200,
            body: fx::NOVA_VERSION.into(),
            elapsed_ms: 6,
        },
    );
    assert_eq!(h.state, HealthState::Up);
    assert_eq!(h.version_id.as_deref(), Some("v2.1"));
    assert_eq!(h.microversion.as_deref(), Some("2.95"));
}

#[test]
fn keystone_version_document_reads_the_id_without_inventing_a_microversion() {
    // Keystone speaks no microversions — an honest `None`, never a guess (§7).
    let h = shape_health(
        "identity",
        EndpointInterface::Public,
        "http://10.0.0.5:5000/",
        &ProbeOutcome::Reachable {
            http_status: 300,
            body: fx::KEYSTONE_VERSION.into(),
            elapsed_ms: 3,
        },
    );
    assert_eq!(h.version_id.as_deref(), Some("v3.14"));
    assert!(h.microversion.is_none());
}

#[test]
fn nova_server_detail_parses_names_ids_and_status() {
    let t =
        ResourceTable::from_collection_json("compute", "servers/detail", fx::NOVA_SERVERS_DETAIL)
            .expect("parse");
    assert_eq!(t.rows.len(), 2);
    assert!(t.columns.len() <= 7, "columns are capped for readability");
    assert_eq!(t.columns.first().map(String::as_str), Some("name"));
    assert_eq!(t.columns.get(1).map(String::as_str), Some("status"));
    let status = t.column_index("status").unwrap();
    assert_eq!(t.rows[0].cells[status], "ACTIVE");
    assert_eq!(t.rows[1].cells[status], "SHUTOFF");
    assert_eq!(t.row_label(&t.rows[0]), "web-01");
    assert_eq!(t.rows[1].id, "f5dc173b-6804-445a-a6d8-c705dad5b5eb");
    // CONTRACT NOTE (fidelity gap, not a crash): with Nova microversion >= 2.47
    // — the modern default — the embedded `flavor` is `{...,"original_name":...}`
    // with no `id`/`name`, so the column-deriver (which renders a ref object by
    // its name/id) drops it. The parser degrades honestly (name/status/id are
    // right); the flavor cell is simply absent. Tracked as a follow-up.
    assert!(
        t.column_index("flavor").is_none(),
        "modern Nova flavor (original_name) is not rendered — documented gap"
    );
}

#[test]
fn neutron_networks_parse_into_labeled_rows() {
    let t = ResourceTable::from_collection_json("network", "v2.0/networks", fx::NEUTRON_NETWORKS)
        .expect("parse");
    assert_eq!(t.rows.len(), 2);
    assert_eq!(t.row_label(&t.rows[0]), "public");
    assert_eq!(t.row_label(&t.rows[1]), "private");
    assert_eq!(t.rows[0].id, "d32019d3-bc6e-4319-9c1d-6722fc136a22");
}

#[test]
fn glance_v2_images_parse_despite_the_pagination_envelope() {
    // Glance v2 wraps the list alongside top-level `first`/`schema` strings; the
    // parser must still locate the `images` array (not trip on the scalars).
    let t = ResourceTable::from_collection_json("image", "v2/images", fx::GLANCE_IMAGES)
        .expect("parse");
    assert_eq!(t.rows.len(), 2);
    assert_eq!(t.row_label(&t.rows[0]), "cirros-0.6.2-x86_64");
    assert_eq!(t.rows[1].id, "781b3762-9469-4cec-b58d-3349e5de4e9c");
}

#[test]
fn cinder_volumes_detail_parses_into_labeled_rows() {
    let t = ResourceTable::from_collection_json(
        "volumev3",
        "volumes/detail",
        fx::CINDER_VOLUMES_DETAIL,
    )
    .expect("parse");
    assert_eq!(t.rows.len(), 2);
    assert_eq!(t.row_label(&t.rows[0]), "web-data");
    assert_eq!(t.row_label(&t.rows[1]), "backup-vol");
    let status = t.column_index("status").unwrap();
    assert_eq!(t.rows[0].cells[status], "in-use");
}

#[test]
fn heat_stack_detail_and_preview_parse_from_canonical_bodies() {
    let d = HeatStackDetail::from_stack_json(fx::HEAT_STACK_SHOW)
        .expect("stack")
        .with_resources_json(fx::HEAT_STACK_RESOURCES)
        .with_events_json(fx::HEAT_STACK_EVENTS)
        .with_template_json(fx::HEAT_STACK_TEMPLATE);
    assert_eq!(d.stack_name, "mesh-overlay-net");
    assert_eq!(d.status, "UPDATE_COMPLETE");
    assert_eq!(d.resources.len(), 2);
    assert_eq!(d.events.len(), 2);
    assert_eq!(d.outputs.len(), 2);
    assert!(d.template.contains("heat_template_version"));

    let p = HeatPreview::from_json(fx::HEAT_PREVIEW_UPDATE).expect("preview");
    assert_eq!(p.change_count(), 2);
    assert!(!p.is_no_change());
}

// ─────────────────────── CLI (python-openstackclient) contracts ───────────────────────

#[test]
fn cli_argv_builders_match_the_openstack_command_surface() {
    use crate::workers::openstack::verbs::{
        build_console_url_argv, build_lifecycle_argv, build_server_list_argv, LifecycleAction,
    };
    assert_eq!(build_server_list_argv(), ["server", "list", "-f", "json"]);
    assert_eq!(
        build_lifecycle_argv(LifecycleAction::Delete, "i-9"),
        ["server", "delete", "i-9"]
    );
    assert_eq!(
        build_console_url_argv("i-9"),
        [
            "console",
            "url",
            "show",
            "--spice-html5",
            "i-9",
            "-f",
            "json"
        ]
    );
}

#[test]
fn cli_server_list_json_parses_the_roster_with_string_or_object_networks() {
    use crate::workers::openstack::verbs::parse_server_list_json;
    let roster = parse_server_list_json(fx::CLI_SERVER_LIST).expect("parse roster");
    assert_eq!(roster.len(), 2);
    assert_eq!(roster[0].id, "9168b536-cd40-4630-b43f-b259807c6e87");
    assert_eq!(roster[0].status, "ACTIVE");
    assert_eq!(roster[0].flavor.as_deref(), Some("m1.tiny"));
    // A modern object-shaped Networks column renders to a compact string.
    assert!(roster[0]
        .networks
        .as_deref()
        .is_some_and(|n| n.contains("10.0.0.7")));
    // A legacy string Networks column is carried verbatim.
    assert_eq!(roster[1].networks.as_deref(), Some("private=10.0.0.8"));
    // A boot-from-volume server (empty Image) is an honest `None`, never guessed.
    assert!(roster[1].image.is_none());
}

#[test]
fn cli_console_url_json_parses_the_spice_descriptor() {
    use crate::workers::openstack::verbs::parse_console_url_json;
    let info = parse_console_url_json("web-01", fx::CLI_CONSOLE_URL).expect("parse console");
    assert_eq!(info.instance, "web-01");
    assert_eq!(info.protocol, "spice-html5");
    assert!(info.url.contains("spice_auto.html"));
}

// ─────────────────────── live-gated integration skeleton ───────────────────────

/// Live-gated end-to-end proof against a REAL `OpenStack` cloud — the offline
/// contract tests prove the request/response shapes on canonical fixtures; this
/// proves the assembled client against a live catalog when the operator points it
/// at a `clouds.yaml`.
///
/// Env-gated + `#[ignore]` (a live cloud cannot exist in CI), mirroring the
/// VDI/console live suites. Run:
///
/// ```text
/// MDE_OPENSTACK_LIVE_TARGET=/etc/openstack/clouds.yaml \
///   cargo test -p mackesd --lib openstack::client::contract::live -- \
///   --ignored --nocapture --test-threads=1
/// ```
///
/// (`MDE_OPENSTACK_LIVE_TARGET` is a path to a `clouds.yaml`; `$OS_CLOUD` selects
/// the context when the file holds more than one.)
#[test]
#[ignore = "live OpenStack cloud required — set MDE_OPENSTACK_LIVE_TARGET=<clouds.yaml path>"]
fn live_openstack_catalog_and_resources() {
    use super::{CatalogSource, LiveOpenStack, ResourceSource};

    let Ok(target) = std::env::var("MDE_OPENSTACK_LIVE_TARGET") else {
        eprintln!("live-openstack: SKIP — MDE_OPENSTACK_LIVE_TARGET not set (path to clouds.yaml)");
        return;
    };
    // Point the standard clouds.yaml resolver at the operator's target file.
    std::env::set_var("OS_CLIENT_CONFIG_FILE", &target);

    let live = LiveOpenStack::new();

    // Authenticate + read the real catalog & per-service health.
    let ch = live
        .catalog_and_health()
        .expect("live authenticate + catalog + health");
    assert!(
        !ch.catalog.services.is_empty(),
        "a live cloud advertises at least one service"
    );
    assert_eq!(
        ch.health.len(),
        ch.catalog.services.len(),
        "one honest health row per cataloged service"
    );
    eprintln!(
        "live-openstack: {} services cataloged; health = {:?}",
        ch.catalog.services.len(),
        ch.health
            .iter()
            .map(|h| format!("{}:{:?}", h.service_type, h.state))
            .collect::<Vec<_>>()
    );

    // List the real Nova servers — an honest table (possibly empty), never fake.
    if ch.catalog.service("compute").is_some() {
        let table = live
            .list_resources("compute", "servers/detail", &[])
            .expect("live Nova server list");
        eprintln!("live-openstack: {} Nova servers", table.rows.len());
        for row in &table.rows {
            assert!(!row.id.trim().is_empty(), "a live server row carries an id");
        }
    }
}
