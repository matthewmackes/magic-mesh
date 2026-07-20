//! WL-FUNC-008 — the pure merge fold for the unified service view.
//!
//! Takes the three source snapshots (published directory rows + probe-inventory
//! services; enrichment is the pure `service → action` map applied over the merge)
//! and folds them into one deduped [`ServiceRecord`] set keyed by `(host, kind)`,
//! stamping health from source + freshness and **aging out** stale probe entries.
//!
//! No I/O — the impure reads live in [`super::sources`]; this module only merges
//! what they produced, so it is exhaustively fixture-testable off fakes.

use std::collections::BTreeMap;

use mackes_mesh_types::service_record::{ServiceHealth, ServiceProvenance, ServiceRecord};

use crate::workers::unit_aggregator::enrich::service_action;

/// One node's published service-directory row, reduced to the merge's inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedInput {
    /// The publishing node's host.
    pub host: String,
    /// The node's overlay IP, when resolved — the endpoint for a bare token
    /// (no port; a directory token has no wire port). `None` until enroll records it.
    pub endpoint_ip: Option<String>,
    /// The advertised service tokens (`files`, `openstack`, …).
    pub services: Vec<String>,
    /// Unix-ms the node last (re)published its directory row (freshness).
    pub updated_ms: i64,
}

/// One probe-discovered open service, reduced to the merge's inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeInput {
    /// The host the port was found on (hostname when known, else the IP).
    pub host: String,
    /// The resolved IP (the endpoint's host half).
    pub ip: String,
    /// The open port.
    pub port: u16,
    /// The coarse service kind nmap identified (`ssh`, `http`, …); the merge key's
    /// service half.
    pub kind: String,
    /// Unix-ms this service was last seen by a probe (freshness / age-out key).
    pub last_seen_ms: i64,
}

/// Merge the three sources into one deduped [`ServiceRecord`] set.
///
/// The pipeline, keyed by `(host, kind)`:
/// 1. **Published** rows attest each advertised token (endpoint = the bare overlay
///    IP if the node has one).
/// 2. **Probe** rows attest the same `(host, kind)` and OVERWRITE the endpoint with
///    the richer `ip:port` (a probe knows the wire port a directory token can't).
/// 3. **Enrichment** applies the offline `service → openable-action` map: a record
///    whose kind maps to a launch verb gains that `action` AND an `Enrichment`
///    attestation (the Explorer's third-source label).
///
/// Then health + **TTL age-out**: a record fresher than `ttl_ms` is [`Up`] when a
/// probe confirmed it, else [`Unknown`] (published-only = advertised-unconfirmed).
/// Past the TTL a record is [`Stale`] — UNLESS it is probe-only, in which case it
/// **expires** and is dropped (a directory-less service can't be re-confirmed off a
/// stale probe row). Output is sorted by `(host, kind)`.
///
/// [`Up`]: ServiceHealth::Up
/// [`Unknown`]: ServiceHealth::Unknown
/// [`Stale`]: ServiceHealth::Stale
#[must_use]
pub fn aggregate(
    published: &[PublishedInput],
    probes: &[ProbeInput],
    now_ms: i64,
    ttl_ms: i64,
) -> Vec<ServiceRecord> {
    let mut by_key: BTreeMap<(String, String), ServiceRecord> = BTreeMap::new();

    // (1) Published directory rows.
    for row in published {
        for svc in &row.services {
            let rec = by_key
                .entry((row.host.clone(), svc.clone()))
                .or_insert_with(|| ServiceRecord::new(row.host.clone(), svc.clone()));
            rec.attest(ServiceProvenance::Published);
            if rec.endpoint.is_none() {
                rec.endpoint = row.endpoint_ip.clone();
            }
            rec.last_seen_ms = rec.last_seen_ms.max(row.updated_ms);
        }
    }

    // (2) Probe-discovered services. A probe's ip:port is the richest endpoint, so
    //     it wins over a bare published overlay IP.
    for pr in probes {
        let rec = by_key
            .entry((pr.host.clone(), pr.kind.clone()))
            .or_insert_with(|| ServiceRecord::new(pr.host.clone(), pr.kind.clone()));
        rec.attest(ServiceProvenance::Probe);
        rec.endpoint = Some(format!("{}:{}", pr.ip, pr.port));
        rec.last_seen_ms = rec.last_seen_ms.max(pr.last_seen_ms);
    }

    // (3) Enrichment + health + TTL age-out.
    let mut out = Vec::with_capacity(by_key.len());
    for (_key, mut rec) in by_key {
        // Enrichment (the Explorer's offline service→action map): a mapped kind
        // gains its openable-action verb + an Enrichment attestation.
        if let Some(action) = service_action(&rec.kind) {
            rec.action = Some(action.to_string());
            rec.attest(ServiceProvenance::Enrichment);
        }

        let fresh = now_ms.saturating_sub(rec.last_seen_ms) <= ttl_ms;
        let has_probe = rec.attested_by(ServiceProvenance::Probe);
        let has_published = rec.attested_by(ServiceProvenance::Published);

        if !fresh && !has_published {
            // Probe-only + past the TTL → expired. A service known ONLY from a stale
            // probe row can't be re-confirmed, so it ages out of the set entirely.
            continue;
        }
        rec.health = if !fresh {
            ServiceHealth::Stale
        } else if has_probe {
            ServiceHealth::Up
        } else {
            ServiceHealth::Unknown
        };
        out.push(rec);
    }
    out.sort_by(|a, b| a.key().cmp(&b.key()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TTL: i64 = 300_000; // 5 min

    fn published(host: &str, ip: Option<&str>, svcs: &[&str], updated_ms: i64) -> PublishedInput {
        PublishedInput {
            host: host.into(),
            endpoint_ip: ip.map(str::to_string),
            services: svcs.iter().map(|s| (*s).to_string()).collect(),
            updated_ms,
        }
    }

    fn probe(host: &str, ip: &str, port: u16, kind: &str, last_seen_ms: i64) -> ProbeInput {
        ProbeInput {
            host: host.into(),
            ip: ip.into(),
            port,
            kind: kind.into(),
            last_seen_ms,
        }
    }

    #[test]
    fn mixed_sources_merge_into_one_record_with_provenance_and_health() {
        // The SAME (alpha, ssh) is published, probed, AND enrichment-mapped → ONE
        // record attested by all three, probe-confirmed Up, with the probe's ip:port
        // endpoint and the enrichment openable action.
        let now = 1_000_000;
        let pubs = vec![published("alpha", Some("10.42.0.5"), &["ssh"], now - 1_000)];
        let probes = vec![probe("alpha", "10.42.0.5", 22, "ssh", now - 2_000)];

        let out = aggregate(&pubs, &probes, now, TTL);
        assert_eq!(out.len(), 1, "one merged record, not three");
        let r = &out[0];
        assert_eq!(r.host, "alpha");
        assert_eq!(r.kind, "ssh");
        // All three sources attest, in the canonical order.
        assert_eq!(
            r.provenance,
            vec![
                ServiceProvenance::Published,
                ServiceProvenance::Probe,
                ServiceProvenance::Enrichment,
            ]
        );
        // The probe's ip:port beat the bare published overlay IP.
        assert_eq!(r.endpoint.as_deref(), Some("10.42.0.5:22"));
        // Probe-confirmed + fresh → Up; enrichment supplied the action.
        assert_eq!(r.health, ServiceHealth::Up);
        assert_eq!(r.action.as_deref(), Some("open-ssh"));
    }

    #[test]
    fn stale_age_out_expires_an_unseen_probe_entry() {
        // A probe-only service last seen well beyond the TTL is EXPIRED (dropped),
        // while a fresh probe on the same host survives.
        let now = 1_000_000;
        let probes = vec![
            probe("beta", "10.42.0.9", 80, "http", now - (TTL + 10_000)), // stale → gone
            probe("beta", "10.42.0.9", 22, "ssh", now - 1_000),           // fresh → kept
        ];
        let out = aggregate(&[], &probes, now, TTL);

        assert!(
            !out.iter().any(|r| r.kind == "http"),
            "the unseen probe-only http service must age out of the set"
        );
        let ssh = out
            .iter()
            .find(|r| r.kind == "ssh")
            .expect("the fresh probe service survives");
        assert_eq!(ssh.health, ServiceHealth::Up);
        // Probe-attested (the surviving source); `ssh` also maps to an openable
        // action, so enrichment attests it too — but never a published row here.
        assert!(ssh.attested_by(ServiceProvenance::Probe));
        assert!(!ssh.attested_by(ServiceProvenance::Published));
        assert_eq!(ssh.action.as_deref(), Some("open-ssh"));
    }

    #[test]
    fn published_only_service_is_advertised_unconfirmed() {
        // A published token with no probe is honestly Unknown (advertised, not
        // reachability-confirmed) and carries only the bare overlay-IP endpoint.
        let now = 1_000_000;
        let pubs = vec![published("gamma", Some("10.42.0.7"), &["files"], now - 500)];
        let out = aggregate(&pubs, &[], now, TTL);
        assert_eq!(out.len(), 1);
        let r = &out[0];
        assert_eq!(r.provenance, vec![ServiceProvenance::Published]);
        assert_eq!(r.health, ServiceHealth::Unknown);
        assert_eq!(r.endpoint.as_deref(), Some("10.42.0.7"));
        assert!(
            r.action.is_none(),
            "'files' maps to no openable action (§7)"
        );
    }

    #[test]
    fn a_stale_published_service_is_kept_but_marked_stale() {
        // Own-row authority: a published service past the TTL does NOT expire (unlike
        // a probe-only one) — it stays, honestly marked Stale.
        let now = 1_000_000;
        let pubs = vec![published(
            "delta",
            Some("10.42.0.3"),
            &["ssh"],
            now - (TTL + 60_000),
        )];
        let out = aggregate(&pubs, &[], now, TTL);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].health, ServiceHealth::Stale);
        assert!(out[0].attested_by(ServiceProvenance::Published));
    }

    #[test]
    fn output_is_sorted_by_host_then_kind() {
        let now = 1_000_000;
        let pubs = vec![
            published("zeta", Some("10.0.0.2"), &["ssh"], now),
            published("alpha", Some("10.0.0.1"), &["http", "ssh"], now),
        ];
        let out = aggregate(&pubs, &[], now, TTL);
        let keys: Vec<(String, String)> = out.iter().map(ServiceRecord::key).collect();
        assert_eq!(
            keys,
            vec![
                ("alpha".into(), "http".into()),
                ("alpha".into(), "ssh".into()),
                ("zeta".into(), "ssh".into()),
            ]
        );
    }
}
