//! IAC-1 — per-service API health: a real ping/version probe per cataloged
//! endpoint.
//!
//! Design #3: each cataloged endpoint gets a **real** version/ping probe →
//! `{ up/down, latency_ms, microversion }`. The probe is a plain HTTP `GET` of
//! the endpoint's version-discovery document; the raw result
//! ([`mackes_mesh_types::openstack::ProbeOutcome`]) is shaped by the shared
//! [`mackes_mesh_types::openstack::shape_health`] into a
//! [`ServiceHealth`] — honestly: an unreachable endpoint reads `down`, a service
//! with no endpoint reads `absent`, never a fabricated `up` (§7).
//!
//! The seam is [`EndpointProbe`]; the production impl [`HttpProbe`] does the real
//! GET + latency measurement. [`probe_service_health`] walks the catalog over the
//! seam so the fold is fixture-tested with [`super::testkit::FakeProbe`] (no live
//! cloud needed).

use std::time::{Duration, Instant};

use mackes_mesh_types::openstack::{
    absent_health, shape_health, EndpointInterface, ProbeOutcome, ServiceCatalog, ServiceHealth,
};

/// The injectable endpoint-probe seam. [`HttpProbe`] is the production impl;
/// tests inject [`super::testkit::FakeProbe`].
pub trait EndpointProbe {
    /// Probe `url` (a version/ping GET) into a raw [`ProbeOutcome`].
    fn probe(&self, url: &str) -> ProbeOutcome;
}

/// Probe every service in `catalog` on `interface` and shape the results.
///
/// A service that advertises the interface is probed over `probe` and shaped by
/// [`shape_health`]; a service with **no** endpoint for the interface yields an
/// honest [`absent_health`] row (never dropped — its catalog presence is real).
/// One row per service, in catalog order.
#[must_use]
pub fn probe_service_health(
    catalog: &ServiceCatalog,
    probe: &dyn EndpointProbe,
    interface: EndpointInterface,
) -> Vec<ServiceHealth> {
    catalog
        .services
        .iter()
        .map(|svc| {
            svc.endpoint(interface).map_or_else(
                || absent_health(&svc.service_type, interface),
                |ep| {
                    let outcome = probe.probe(&ep.url);
                    shape_health(&svc.service_type, interface, &ep.url, &outcome)
                },
            )
        })
        .collect()
}

/// The bound on one health probe.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Production [`EndpointProbe`]: a real HTTP `GET` of the endpoint, timing the
/// round-trip.
///
/// Any HTTP answer (even a `401`/`300`) proves the service is reachable → `up`;
/// a `5xx` is shaped to `down` by [`shape_health`]; a transport failure
/// (connection refused / timeout) becomes [`ProbeOutcome::Unreachable`] → `down`
/// with the latency-to-failure. Blocking (`reqwest::blocking` offloads to its own
/// runtime, safe from the worker's `spawn_blocking` drain).
#[derive(Debug, Clone, Default)]
pub struct HttpProbe;

impl HttpProbe {
    /// Construct the production probe.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// Milliseconds elapsed since `started`, saturating (a probe never runs long
/// enough to overflow a `u64` of milliseconds, but saturate rather than cast).
fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

impl EndpointProbe for HttpProbe {
    fn probe(&self, url: &str) -> ProbeOutcome {
        let started = Instant::now();
        let client = match reqwest::blocking::Client::builder()
            .timeout(PROBE_TIMEOUT)
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                return ProbeOutcome::Unreachable {
                    elapsed_ms: elapsed_ms(started),
                    reason: format!("probe client build failed: {e}"),
                }
            }
        };
        match client.get(url).send() {
            Ok(resp) => {
                let http_status = resp.status().as_u16();
                let body = resp.text().unwrap_or_default();
                ProbeOutcome::Reachable {
                    http_status,
                    body,
                    elapsed_ms: elapsed_ms(started),
                }
            }
            Err(e) => ProbeOutcome::Unreachable {
                elapsed_ms: elapsed_ms(started),
                reason: e.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::openstack::client::testkit::FakeProbe;
    use mackes_mesh_types::openstack::HealthState;

    fn catalog() -> ServiceCatalog {
        ServiceCatalog::from_keystone_token_json(
            r#"{"token":{"catalog":[
                {"type":"compute","name":"nova","endpoints":[
                    {"interface":"public","url":"http://nova.mesh:8774/v2.1","region":"RegionOne"}
                ]},
                {"type":"network","name":"neutron","endpoints":[
                    {"interface":"public","url":"http://neutron.mesh:9696/","region":"RegionOne"}
                ]},
                {"type":"dns","name":"designate","endpoints":[
                    {"interface":"admin","url":"http://designate.mesh:9001/","region":"RegionOne"}
                ]}
            ]}}"#,
        )
        .unwrap()
    }

    #[test]
    fn folds_up_down_and_absent_over_the_catalog() {
        let cat = catalog();
        let probe = FakeProbe::new()
            .up(
                "http://nova.mesh:8774/v2.1",
                r#"{"version":{"id":"v2.1","status":"CURRENT","max_version":"2.90"}}"#,
                7,
            )
            .unreachable("http://neutron.mesh:9696/", "connection refused", 2000);
        // designate advertises only an admin endpoint, so a public probe finds
        // nothing → absent (its row is still present).

        let health = probe_service_health(&cat, &probe, EndpointInterface::Public);
        assert_eq!(health.len(), 3, "one row per service, in catalog order");

        assert_eq!(health[0].service_type, "compute");
        assert_eq!(health[0].state, HealthState::Up);
        assert_eq!(health[0].latency_ms, Some(7));
        assert_eq!(health[0].microversion.as_deref(), Some("2.90"));

        assert_eq!(health[1].service_type, "network");
        assert_eq!(health[1].state, HealthState::Down);
        assert_eq!(health[1].detail.as_deref(), Some("connection refused"));

        assert_eq!(health[2].service_type, "dns");
        assert_eq!(health[2].state, HealthState::Absent);
        assert!(health[2].url.is_empty());
    }

    #[test]
    fn an_empty_catalog_yields_no_rows() {
        let health = probe_service_health(
            &ServiceCatalog::default(),
            &FakeProbe::new(),
            EndpointInterface::Public,
        );
        assert!(health.is_empty());
    }
}
