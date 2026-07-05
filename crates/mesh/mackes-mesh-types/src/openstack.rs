//! IAC — the `OpenStack` service-directory + API-health schema
//! (`docs/design/iac-workspace.md`, locked 2026-07-04).
//!
//! **This JSON is the §6 contract** between the mesh-side producer (`mackesd`'s
//! `openstack` worker + its IAC-1 client foundation,
//! [`crate::workers::openstack::client`](../../mackesd) — the clouds.yaml →
//! Keystone → catalog → per-endpoint health path) and the desktop-side consumer
//! (the IAC-2 Infra-as-Code surface's `OpenStack` API status band +
//! merged service directory). Neither crate may depend on the other (the
//! layered-tiers boundary gate, §6), so the shape lives here in the
//! mesh-neutral shared crate — alongside [`crate::device_inventory`], the DEVMGR
//! §6 exemplar — and both sides `use mackes_mesh_types::openstack::*`.
//!
//! ## What lives here (pure, no I/O)
//!
//! - [`ServiceCatalog`] — the **authoritative service directory** the IAC surface
//!   consumes: every service the live Keystone catalog advertises, each with its
//!   public/internal/admin endpoints + region. [`ServiceCatalog::from_keystone_token_json`]
//!   parses a real Keystone **v3 token response** (`token.catalog[]`) into it.
//! - [`ServiceHealth`] — a per-endpoint **API health** row: `{ state (up/down/
//!   absent), latency_ms, microversion, version_id }`. [`shape_health`] turns a
//!   raw [`ProbeOutcome`] (the HTTP result of a version/ping probe, or a
//!   transport failure) into it — **honestly**: an unreachable endpoint reads
//!   [`HealthState::Down`], a service with no endpoint for the interface reads
//!   [`HealthState::Absent`], never a fabricated `up` (§7).
//!
//! The **I/O** (loading clouds.yaml off disk, minting the Keystone token,
//! issuing the probe/resource HTTP calls) is the producer's, in `mackesd`; only
//! these pure types + the pure parse/shape functions are shared, so the
//! producer can be swapped without the consumer knowing.

use serde::{Deserialize, Serialize};

// ─────────────────────────── the service catalog ───────────────────────────

/// A Keystone endpoint interface (`public` / `internal` / `admin`) — the three
/// URLs a service advertises for one region.
///
/// The mesh cloud reaches every API over its Nebula overlay, so in practice the
/// three interfaces resolve to the same overlay URL; the distinction is
/// preserved because it is part of the standard catalog the surface renders and
/// a real deployment may split them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EndpointInterface {
    /// The tenant-facing URL (what a mesh client uses).
    Public,
    /// The service-to-service URL.
    Internal,
    /// The admin URL.
    Admin,
}

impl EndpointInterface {
    /// Every interface, in the canonical (catalog) order.
    pub const ALL: [Self; 3] = [Self::Public, Self::Internal, Self::Admin];

    /// The lowercase catalog token (`"public"` / `"internal"` / `"admin"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Internal => "internal",
            Self::Admin => "admin",
        }
    }

    /// Parse a Keystone interface token, tolerating case + surrounding space.
    /// `None` for an unrecognized interface (never guessed).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "public" => Some(Self::Public),
            "internal" => Some(Self::Internal),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }
}

/// One advertised endpoint of a cataloged service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogEndpoint {
    /// Which interface this URL serves.
    pub interface: EndpointInterface,
    /// The endpoint URL (may carry a version suffix, e.g. `.../v2.1`).
    pub url: String,
    /// The region this endpoint lives in (the catalog's `region`/`region_id`),
    /// when advertised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}

/// One service in the Keystone catalog — its type, its human name, and its
/// per-interface endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogService {
    /// The service **type** — the stable key the surface groups + drills on
    /// (`compute` / `network` / `image` / `volumev3` / `orchestration` /
    /// `identity` / …).
    #[serde(rename = "type")]
    pub service_type: String,
    /// The service's human name when the catalog carries one (`nova`, `neutron`,
    /// `glance`, …). `None` when the deployment left it unset (honest, not
    /// guessed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The advertised endpoints, in catalog order.
    pub endpoints: Vec<CatalogEndpoint>,
}

impl CatalogService {
    /// The endpoint serving `interface`, if this service advertises one.
    #[must_use]
    pub fn endpoint(&self, interface: EndpointInterface) -> Option<&CatalogEndpoint> {
        self.endpoints.iter().find(|e| e.interface == interface)
    }

    /// The public endpoint URL — what a mesh client connects to. Falls back to
    /// the internal then admin URL when a deployment advertises only those, so
    /// the surface always has a URL to show when *any* endpoint exists.
    #[must_use]
    pub fn primary_url(&self) -> Option<&str> {
        EndpointInterface::ALL
            .iter()
            .find_map(|i| self.endpoint(*i))
            .map(|e| e.url.as_str())
    }
}

/// The whole Keystone service catalog — the authoritative directory the IAC
/// surface consumes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceCatalog {
    /// The cloud's region, when the catalog advertises a single one (the design
    /// is a single default context/region, Q19). `None` when the catalog is
    /// empty or spans several regions (the per-endpoint [`CatalogEndpoint::region`]
    /// stays authoritative in that case).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Every advertised service, in catalog order.
    pub services: Vec<CatalogService>,
}

/// A Keystone catalog couldn't be parsed from a token response — the typed,
/// honest failure (never a fabricated empty catalog).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogParseError(pub String);

impl std::fmt::Display for CatalogParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "parsing the Keystone token catalog failed: {}", self.0)
    }
}

impl std::error::Error for CatalogParseError {}

impl ServiceCatalog {
    /// Parse a Keystone **v3 token response** body into the service directory.
    ///
    /// The body is the JSON `POST /v3/auth/tokens` returns — `{ "token": {
    /// "catalog": [ { "type": …, "name": …, "endpoints": [ { "interface": …,
    /// "url": …, "region": … } ] } ] } }`. An endpoint whose `interface` isn't
    /// one of public/internal/admin is skipped (never mis-mapped); a service
    /// with no usable endpoint is still listed (its presence is real, §7).
    ///
    /// # Errors
    /// [`CatalogParseError`] when the body isn't valid JSON or has no
    /// `token.catalog` array.
    pub fn from_keystone_token_json(body: &str) -> Result<Self, CatalogParseError> {
        #[derive(Deserialize)]
        struct Root {
            token: Token,
        }
        #[derive(Deserialize)]
        struct Token {
            #[serde(default)]
            catalog: Option<Vec<RawService>>,
        }
        #[derive(Deserialize)]
        struct RawService {
            #[serde(rename = "type")]
            service_type: String,
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            endpoints: Vec<RawEndpoint>,
        }
        #[derive(Deserialize)]
        struct RawEndpoint {
            #[serde(default)]
            interface: String,
            #[serde(default)]
            url: String,
            #[serde(default)]
            region: Option<String>,
            #[serde(default)]
            region_id: Option<String>,
        }

        let root: Root =
            serde_json::from_str(body.trim()).map_err(|e| CatalogParseError(e.to_string()))?;
        let raw = root
            .token
            .catalog
            .ok_or_else(|| CatalogParseError("token.catalog is absent".to_string()))?;

        let services: Vec<CatalogService> = raw
            .into_iter()
            .map(|s| {
                let endpoints = s
                    .endpoints
                    .into_iter()
                    .filter_map(|e| {
                        let interface = EndpointInterface::parse(&e.interface)?;
                        if e.url.trim().is_empty() {
                            return None;
                        }
                        Some(CatalogEndpoint {
                            interface,
                            url: e.url,
                            region: e.region.or(e.region_id).filter(|r| !r.trim().is_empty()),
                        })
                    })
                    .collect();
                CatalogService {
                    service_type: s.service_type,
                    name: s.name.filter(|n| !n.trim().is_empty()),
                    endpoints,
                }
            })
            .collect();

        // Derive a catalog-wide region only when every endpoint agrees on one
        // (the single-context design); otherwise leave it None and let the
        // per-endpoint region stay authoritative.
        let all_regions: Vec<&str> = services
            .iter()
            .flat_map(|s| s.endpoints.iter())
            .filter_map(|e| e.region.as_deref())
            .collect();
        let region = match all_regions.first() {
            Some(first) if all_regions.iter().all(|r| r == first) => Some((*first).to_string()),
            _ => None,
        };

        Ok(Self { region, services })
    }

    /// The cataloged service of type `service_type`, if advertised.
    #[must_use]
    pub fn service(&self, service_type: &str) -> Option<&CatalogService> {
        self.services
            .iter()
            .find(|s| s.service_type == service_type)
    }
}

// ─────────────────────────── per-service API health ───────────────────────────

/// The honest health of one cataloged endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    /// The endpoint answered a version/ping probe (any HTTP status counts as
    /// reachable — a `401`/`300` still proves the service is up).
    Up,
    /// The endpoint is cataloged but did not answer (connection refused, a
    /// timeout, or a `5xx`) — the service is down, not faked up.
    Down,
    /// The service advertises no endpoint for the probed interface — there is
    /// nothing to reach (honestly absent, distinct from `down`).
    Absent,
}

/// One per-endpoint health row the IAC status band renders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceHealth {
    /// The service type this health is for (`compute`, `network`, …).
    pub service_type: String,
    /// The interface probed.
    pub interface: EndpointInterface,
    /// The endpoint URL probed. Empty when [`HealthState::Absent`] (no endpoint).
    pub url: String,
    /// The honest state.
    pub state: HealthState,
    /// Round-trip latency of the probe in milliseconds. `Some` for a real probe
    /// (`Up` or `Down` after a transport attempt); `None` for `Absent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    /// The service's advertised microversion (the version doc's `max_version`,
    /// falling back to `version`), when the version document carried one. `None`
    /// for a service that doesn't speak microversions or an unreadable body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microversion: Option<String>,
    /// The version-document id (`v2.1`, `v3`, …), when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_id: Option<String>,
    /// A short human reason (the HTTP status, or the transport error) — the
    /// operator's "why" for a `Down`, and context for an `Up`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The raw result of one health probe — the transport layer's honest report.
///
/// [`shape_health`] turns it into a [`ServiceHealth`]. Kept transport-neutral (an
/// HTTP status + body, or a failure) so the producer's client and the tests share
/// one shaping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// The endpoint answered: the HTTP status, the response body (a version
    /// discovery document when the service exposes one), and the round-trip.
    Reachable {
        /// The HTTP status code the endpoint returned.
        http_status: u16,
        /// The response body (parsed for a microversion when it is a version
        /// document; may be empty).
        body: String,
        /// Round-trip in milliseconds.
        elapsed_ms: u64,
    },
    /// The endpoint could not be reached (connection refused / timeout / DNS) —
    /// the elapsed time until the failure and the transport reason.
    Unreachable {
        /// Time spent before the transport gave up, in milliseconds.
        elapsed_ms: u64,
        /// The transport error (connection refused, timed out, …).
        reason: String,
    },
}

/// Shape a raw [`ProbeOutcome`] for `(service_type, interface, url)` into an
/// honest [`ServiceHealth`].
///
/// A **reachable** outcome reads [`HealthState::Up`] with the latency and, when
/// the body is a version document, its microversion + version id — except a
/// `5xx`, which reads **down** (cataloged but erroring); any other status (incl.
/// `2xx`/`3xx`/`401`) is up (it answered). An **unreachable** outcome reads
/// [`HealthState::Down`] with the latency-to-failure and the transport reason.
/// Never a fabricated `up` (§7).
#[must_use]
pub fn shape_health(
    service_type: &str,
    interface: EndpointInterface,
    url: &str,
    outcome: &ProbeOutcome,
) -> ServiceHealth {
    match outcome {
        ProbeOutcome::Reachable {
            http_status,
            body,
            elapsed_ms,
        } => {
            let (version_id, microversion) = parse_version_document(body);
            let is_server_error = (500..600).contains(http_status);
            ServiceHealth {
                service_type: service_type.to_string(),
                interface,
                url: url.to_string(),
                state: if is_server_error {
                    HealthState::Down
                } else {
                    HealthState::Up
                },
                latency_ms: Some(*elapsed_ms),
                microversion,
                version_id,
                detail: Some(format!("HTTP {http_status}")),
            }
        }
        ProbeOutcome::Unreachable { elapsed_ms, reason } => ServiceHealth {
            service_type: service_type.to_string(),
            interface,
            url: url.to_string(),
            state: HealthState::Down,
            latency_ms: Some(*elapsed_ms),
            microversion: None,
            version_id: None,
            detail: Some(reason.clone()),
        },
    }
}

/// The honest "no endpoint to probe" health row for a service that advertises
/// nothing on `interface`.
#[must_use]
pub fn absent_health(service_type: &str, interface: EndpointInterface) -> ServiceHealth {
    ServiceHealth {
        service_type: service_type.to_string(),
        interface,
        url: String::new(),
        state: HealthState::Absent,
        latency_ms: None,
        microversion: None,
        version_id: None,
        detail: Some("no endpoint advertised for this interface".to_string()),
    }
}

/// Extract `(version_id, microversion)` from an `OpenStack` version-discovery
/// document body, tolerating the three real shapes services emit:
/// `{"versions":[…]}` (Nova/Cinder/Glance root), `{"versions":{"values":[…]}}`
/// (Keystone root), and `{"version":{…}}` (a versioned URL). The microversion is
/// the entry's `max_version` (falling back to a non-empty `version`); the id is
/// its `id`. Returns `(None, None)` when the body isn't a recognizable version
/// document — never a guessed version (§7).
fn parse_version_document(body: &str) -> (Option<String>, Option<String>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body.trim()) else {
        return (None, None);
    };
    // Collect the candidate version entries from whichever shape is present.
    let entries: Vec<&serde_json::Value> = if let Some(v) = value.get("version") {
        vec![v]
    } else if let Some(list) = value.get("versions").and_then(|v| v.as_array()) {
        list.iter().collect()
    } else if let Some(list) = value
        .get("versions")
        .and_then(|v| v.get("values"))
        .and_then(|v| v.as_array())
    {
        list.iter().collect()
    } else {
        return (None, None);
    };
    if entries.is_empty() {
        return (None, None);
    }
    // Prefer the CURRENT/stable entry; else the last (highest) advertised.
    let chosen = entries
        .iter()
        .find(|e| {
            e.get("status")
                .and_then(|s| s.as_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("current"))
        })
        .or_else(|| entries.last())
        .copied();
    let Some(entry) = chosen else {
        return (None, None);
    };
    let non_empty = |v: &serde_json::Value, key: &str| -> Option<String> {
        v.get(key)
            .and_then(|s| s.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let version_id = non_empty(entry, "id");
    let microversion = non_empty(entry, "max_version").or_else(|| non_empty(entry, "version"));
    (version_id, microversion)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed but realistic Keystone v3 token response — the shape
    /// `POST /v3/auth/tokens` returns, with a three-interface compute service, a
    /// single-interface identity service, and an image service.
    const V3_TOKEN: &str = r#"{
      "token": {
        "methods": ["password"],
        "expires_at": "2026-07-04T12:00:00.000000Z",
        "project": {"id": "p1", "name": "mesh"},
        "catalog": [
          {
            "type": "compute",
            "id": "svc-nova",
            "name": "nova",
            "endpoints": [
              {"interface": "public",   "url": "http://nova.mesh:8774/v2.1", "region": "RegionOne", "id": "e1"},
              {"interface": "internal", "url": "http://nova.mesh:8774/v2.1", "region_id": "RegionOne", "id": "e2"},
              {"interface": "admin",    "url": "http://nova.mesh:8774/v2.1", "region": "RegionOne", "id": "e3"}
            ]
          },
          {
            "type": "identity",
            "id": "svc-keystone",
            "name": "keystone",
            "endpoints": [
              {"interface": "public", "url": "http://keystone.mesh:5000/v3", "region": "RegionOne", "id": "e4"}
            ]
          },
          {
            "type": "image",
            "name": "glance",
            "endpoints": [
              {"interface": "public", "url": "http://glance.mesh:9292", "region": "RegionOne", "id": "e5"},
              {"interface": "bogus",  "url": "http://glance.mesh:9292", "region": "RegionOne", "id": "e6"},
              {"interface": "internal", "url": "", "region": "RegionOne", "id": "e7"}
            ]
          }
        ]
      }
    }"#;

    #[test]
    fn parses_the_v3_token_catalog() {
        let cat = ServiceCatalog::from_keystone_token_json(V3_TOKEN).expect("parse");
        assert_eq!(cat.services.len(), 3);
        assert_eq!(cat.region.as_deref(), Some("RegionOne"));

        let nova = cat.service("compute").expect("compute service");
        assert_eq!(nova.name.as_deref(), Some("nova"));
        assert_eq!(nova.endpoints.len(), 3);
        assert_eq!(
            nova.endpoint(EndpointInterface::Public).unwrap().url,
            "http://nova.mesh:8774/v2.1"
        );
        // region_id folds into region for the internal endpoint.
        assert_eq!(
            nova.endpoint(EndpointInterface::Internal)
                .unwrap()
                .region
                .as_deref(),
            Some("RegionOne")
        );
        assert_eq!(nova.primary_url(), Some("http://nova.mesh:8774/v2.1"));

        // Glance drops the unrecognized interface + the empty-URL endpoint, but
        // stays listed (its presence is real).
        let glance = cat.service("image").expect("image service");
        assert_eq!(glance.endpoints.len(), 1, "bogus + empty endpoints dropped");
        assert_eq!(glance.endpoints[0].interface, EndpointInterface::Public);
    }

    #[test]
    fn a_body_without_a_catalog_is_a_typed_error_not_an_empty_catalog() {
        // §7 — an absent catalog is honest failure, never a fabricated empty one.
        let err = ServiceCatalog::from_keystone_token_json(r#"{"token":{}}"#)
            .expect_err("no catalog must fail");
        assert!(err.0.contains("catalog is absent"), "{err}");
        assert!(ServiceCatalog::from_keystone_token_json("not json").is_err());
    }

    #[test]
    fn interface_round_trips_and_tolerates_case() {
        for i in EndpointInterface::ALL {
            assert_eq!(EndpointInterface::parse(i.as_str()), Some(i));
        }
        assert_eq!(
            EndpointInterface::parse("  Public "),
            Some(EndpointInterface::Public)
        );
        assert_eq!(EndpointInterface::parse("nope"), None);
    }

    #[test]
    fn health_up_parses_a_nova_version_list_microversion() {
        // Nova's root returns a `versions` array; the CURRENT entry's
        // max_version is the microversion.
        let body = r#"{"versions":[
            {"id":"v2.0","status":"SUPPORTED","version":"","min_version":""},
            {"id":"v2.1","status":"CURRENT","version":"2.1","max_version":"2.90","min_version":"2.1"}
        ]}"#;
        let outcome = ProbeOutcome::Reachable {
            http_status: 300,
            body: body.to_string(),
            elapsed_ms: 12,
        };
        let h = shape_health(
            "compute",
            EndpointInterface::Public,
            "http://nova.mesh:8774/",
            &outcome,
        );
        assert_eq!(h.state, HealthState::Up);
        assert_eq!(h.latency_ms, Some(12));
        assert_eq!(h.version_id.as_deref(), Some("v2.1"));
        assert_eq!(h.microversion.as_deref(), Some("2.90"));
    }

    #[test]
    fn health_up_parses_a_keystone_values_and_a_single_version_doc() {
        // Keystone's root nests the list under `versions.values`.
        let keystone = r#"{"versions":{"values":[
            {"id":"v3.14","status":"stable","version":"","min_version":""}
        ]}}"#;
        let h = shape_health(
            "identity",
            EndpointInterface::Public,
            "http://keystone.mesh:5000/",
            &ProbeOutcome::Reachable {
                http_status: 300,
                body: keystone.to_string(),
                elapsed_ms: 3,
            },
        );
        assert_eq!(h.state, HealthState::Up);
        assert_eq!(h.version_id.as_deref(), Some("v3.14"));
        // No max_version + empty version ⇒ no microversion (never guessed).
        assert_eq!(h.microversion, None);

        // A versioned URL returns a single `version` object.
        let single =
            r#"{"version":{"id":"v2.1","status":"CURRENT","version":"","max_version":"2.90"}}"#;
        let h2 = shape_health(
            "compute",
            EndpointInterface::Public,
            "http://nova.mesh:8774/v2.1",
            &ProbeOutcome::Reachable {
                http_status: 200,
                body: single.to_string(),
                elapsed_ms: 5,
            },
        );
        assert_eq!(h2.microversion.as_deref(), Some("2.90"));
        assert_eq!(h2.version_id.as_deref(), Some("v2.1"));
    }

    #[test]
    fn a_5xx_reads_down_and_an_unreachable_reads_down() {
        // A cataloged-but-erroring service is down, not faked up.
        let err5xx = shape_health(
            "volumev3",
            EndpointInterface::Public,
            "http://cinder.mesh:8776/",
            &ProbeOutcome::Reachable {
                http_status: 503,
                body: String::new(),
                elapsed_ms: 8,
            },
        );
        assert_eq!(err5xx.state, HealthState::Down);
        assert_eq!(err5xx.detail.as_deref(), Some("HTTP 503"));

        let down = shape_health(
            "network",
            EndpointInterface::Public,
            "http://neutron.mesh:9696/",
            &ProbeOutcome::Unreachable {
                elapsed_ms: 2000,
                reason: "connection refused".into(),
            },
        );
        assert_eq!(down.state, HealthState::Down);
        assert_eq!(down.latency_ms, Some(2000));
        assert_eq!(down.detail.as_deref(), Some("connection refused"));
        assert!(down.microversion.is_none());
    }

    #[test]
    fn a_non_version_body_yields_no_microversion_but_still_up() {
        // A service that answers but not with a version doc is up with no
        // fabricated version.
        let h = shape_health(
            "object-store",
            EndpointInterface::Public,
            "http://swift.mesh:8080/",
            &ProbeOutcome::Reachable {
                http_status: 200,
                body: "<html>ok</html>".into(),
                elapsed_ms: 4,
            },
        );
        assert_eq!(h.state, HealthState::Up);
        assert!(h.microversion.is_none());
        assert!(h.version_id.is_none());
    }

    #[test]
    fn absent_health_is_honestly_absent() {
        let a = absent_health("dns", EndpointInterface::Public);
        assert_eq!(a.state, HealthState::Absent);
        assert!(a.url.is_empty());
        assert!(a.latency_ms.is_none());
    }

    #[test]
    fn catalog_and_health_round_trip_json() {
        // The wire contract the shell deserializes.
        let cat = ServiceCatalog::from_keystone_token_json(V3_TOKEN).unwrap();
        let s = serde_json::to_string(&cat).unwrap();
        let back: ServiceCatalog = serde_json::from_str(&s).unwrap();
        assert_eq!(cat, back);

        let health = shape_health(
            "compute",
            EndpointInterface::Public,
            "http://nova.mesh:8774/",
            &ProbeOutcome::Reachable {
                http_status: 200,
                body: String::new(),
                elapsed_ms: 1,
            },
        );
        let hs = serde_json::to_string(&health).unwrap();
        let hback: ServiceHealth = serde_json::from_str(&hs).unwrap();
        assert_eq!(health, hback);
        // The `interface` serializes as the lowercase catalog token.
        assert!(hs.contains(r#""interface":"public""#));
        assert!(hs.contains(r#""state":"up""#));
    }
}
