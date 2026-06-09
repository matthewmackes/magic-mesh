//! EPIC-MESH-PROBE (Q7) — typed probe facts over the `mde-card`
//! schema.
//!
//! The probe inventory is a stream of Cards: one [`CardKind::Host`]
//! Card per discovered host, with one [`CardKind::Service`] child Card
//! per open port/service. Rather than grow the locked 12-field `Card`
//! schema (the R5-Q3 `card_field_count_is_twelve` lock), the probe
//! facts ride the purpose-built `metadata` bucket under a single
//! `PROBE_KEY` entry — its documented role is exactly this ("drains
//! known fields into typed places"). [`host_card`] / [`service_card`]
//! write the facts; [`host_facts`] / [`service_facts`] read them back.
//!
//! This keeps probe entries first-class Cards (so the Portal-31
//! `card_index` renders them with no probe-specific transform) while
//! the schema-version + field-count locks stay untouched.

use serde::{Deserialize, Serialize};

use crate::schema::{Card, CardKind};

/// Metadata key the probe facts serialize under (one nested object,
/// so adding a fact field never touches the top-level `Card` schema).
pub const PROBE_KEY: &str = "probe";

/// Where a probed host was found. Mirrors the EPIC-MESH-PROBE Q5
/// scope (mesh peers / local LAN / operator-arbitrary target).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostSource {
    /// An enrolled Nebula mesh peer.
    Mesh,
    /// A device on the local LAN segment.
    Lan,
    /// An operator-declared arbitrary target.
    Arbitrary,
}

/// Probe facts for a [`CardKind::Host`] Card.
///
/// `trust_state` is a free-form string here on purpose — the 3-state
/// trust taxonomy is owned by MESH-A-4 (R8-Q10); this module doesn't
/// prematurely lock it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostFacts {
    /// Resolved IP address.
    pub ip: String,
    /// Hostname (peer node-id, reverse-DNS, or mDNS name; may be empty).
    #[serde(default)]
    pub hostname: String,
    /// Where this host was discovered.
    pub source: HostSource,
    /// Trust classification (taxonomy owned by MESH-A-4).
    #[serde(default)]
    pub trust_state: String,
    /// Unix epoch seconds this host was last seen by a probe.
    #[serde(default)]
    pub last_seen: u64,
}

/// Probe facts for a [`CardKind::Service`] child Card — one open port
/// plus its nmap `-sV`/NSE identification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceFacts {
    /// Open port.
    pub port: u16,
    /// Coarse service kind (e.g. `airsonic`, `jellyfin`, `ssh`, `http`).
    pub service_kind: String,
    /// nmap-identified product name (may be empty).
    #[serde(default)]
    pub product: String,
    /// nmap-identified version string (may be empty).
    #[serde(default)]
    pub version: String,
    /// Service fingerprint / banner (may be empty).
    #[serde(default)]
    pub fingerprint: String,
}

/// Build a [`CardKind::Host`] Card carrying `facts`, with `services`
/// as child Cards. Title is the hostname when present, else the IP.
#[must_use]
pub fn host_card(facts: &HostFacts, services: Vec<Card>, now_ts: u64) -> Card {
    let title = if facts.hostname.is_empty() {
        facts.ip.clone()
    } else {
        facts.hostname.clone()
    };
    let mut card = Card::new(CardKind::Host, title, now_ts);
    card.subtitle = Some(facts.ip.clone());
    card.children = services;
    card.metadata.insert(
        PROBE_KEY.to_owned(),
        serde_json::to_value(facts).expect("HostFacts is plain JSON"),
    );
    card
}

/// Build a [`CardKind::Service`] child Card carrying `facts`. Title is
/// the service kind when present, else `port/<n>`.
#[must_use]
pub fn service_card(facts: &ServiceFacts, now_ts: u64) -> Card {
    let title = if facts.service_kind.is_empty() {
        format!("port/{}", facts.port)
    } else {
        facts.service_kind.clone()
    };
    let mut card = Card::new(CardKind::Service, title, now_ts);
    card.metadata.insert(
        PROBE_KEY.to_owned(),
        serde_json::to_value(facts).expect("ServiceFacts is plain JSON"),
    );
    card
}

/// Read [`HostFacts`] back from a host Card's metadata. `None` when the
/// card isn't a host or has no (valid) probe facts.
#[must_use]
pub fn host_facts(card: &Card) -> Option<HostFacts> {
    if card.kind != CardKind::Host {
        return None;
    }
    let v = card.metadata.get(PROBE_KEY)?;
    serde_json::from_value(v.clone()).ok()
}

/// Read [`ServiceFacts`] back from a service Card's metadata. `None`
/// when the card isn't a service or has no (valid) probe facts.
#[must_use]
pub fn service_facts(card: &Card) -> Option<ServiceFacts> {
    if card.kind != CardKind::Service {
        return None;
    }
    let v = card.metadata.get(PROBE_KEY)?;
    serde_json::from_value(v.clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_host() -> HostFacts {
        HostFacts {
            ip: "10.42.0.5".into(),
            hostname: "peer-a".into(),
            source: HostSource::Mesh,
            trust_state: "trusted".into(),
            last_seen: 1700,
        }
    }

    fn sample_service(kind: &str, port: u16) -> ServiceFacts {
        ServiceFacts {
            port,
            service_kind: kind.into(),
            product: "Jellyfin".into(),
            version: "10.9".into(),
            fingerprint: "http/jellyfin".into(),
        }
    }

    #[test]
    fn host_card_carries_facts_and_children() {
        let svcs = vec![
            service_card(&sample_service("airsonic", 4040), 1),
            service_card(&sample_service("jellyfin", 8096), 1),
        ];
        let card = host_card(&sample_host(), svcs, 1700);
        assert_eq!(card.kind, CardKind::Host);
        assert_eq!(card.title, "peer-a");
        assert_eq!(card.subtitle.as_deref(), Some("10.42.0.5"));
        assert_eq!(card.children.len(), 2);
        assert_eq!(card.children[0].kind, CardKind::Service);
    }

    #[test]
    fn host_title_falls_back_to_ip_when_no_hostname() {
        let mut f = sample_host();
        f.hostname = String::new();
        let card = host_card(&f, vec![], 0);
        assert_eq!(card.title, "10.42.0.5");
    }

    #[test]
    fn service_title_falls_back_to_port_when_no_kind() {
        let mut f = sample_service("", 9000);
        f.service_kind = String::new();
        let card = service_card(&f, 0);
        assert_eq!(card.title, "port/9000");
    }

    #[test]
    fn host_facts_round_trip_through_metadata() {
        let card = host_card(&sample_host(), vec![], 1700);
        let back = host_facts(&card).expect("host facts present");
        assert_eq!(back, sample_host());
    }

    #[test]
    fn service_facts_round_trip_through_metadata() {
        let f = sample_service("jellyfin", 8096);
        let card = service_card(&f, 1);
        let back = service_facts(&card).expect("service facts present");
        assert_eq!(back, f);
    }

    #[test]
    fn full_inventory_card_round_trips_through_json() {
        // The shape card_index will load off disk: a host with two
        // service children, serialized + read back unchanged.
        let svcs = vec![
            service_card(&sample_service("airsonic", 4040), 1),
            service_card(&sample_service("jellyfin", 8096), 1),
        ];
        let card = host_card(&sample_host(), svcs, 1700);
        let raw = serde_json::to_string(&card).unwrap();
        let back: Card = serde_json::from_str(&raw).unwrap();
        assert_eq!(card, back);
        // And the typed facts survive the JSON round-trip.
        assert_eq!(host_facts(&back), Some(sample_host()));
        assert_eq!(
            service_facts(&back.children[1]),
            Some(sample_service("jellyfin", 8096))
        );
    }

    #[test]
    fn accessors_reject_wrong_kind() {
        let note = Card::new(CardKind::Note, "x", 0);
        assert!(host_facts(&note).is_none());
        assert!(service_facts(&note).is_none());
        // A host card has no service facts, and vice-versa.
        let host = host_card(&sample_host(), vec![], 0);
        assert!(service_facts(&host).is_none());
    }

    #[test]
    fn host_source_serializes_snake_case() {
        let f = sample_host();
        let raw = serde_json::to_value(&f).unwrap();
        assert_eq!(raw["source"], serde_json::json!("mesh"));
    }
}
