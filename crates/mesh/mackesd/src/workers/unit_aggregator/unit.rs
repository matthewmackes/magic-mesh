//! EXPLORER-1 — the typed `Unit` model + the `state/units/<node>` body.
//!
//! One stream, three kinds (design lock #1): mesh peers (inside), off-mesh LAN
//! hosts (outside), and `OpenStack` objects.
//!
//! Every field an unprobed source can't answer is an explicit `None`/`unknown`
//! (§7, lock #12) — the model never fakes a value. The [`Unit`] carries
//! first/last-seen (E10), an open [`Extras`] block EXPLORER-9 fills (E5
//! enrichment), and an optional [`MeshFacts`] block folded from the mesh mirror
//! for peers (the per-kind detail block, counterpart to the EXPLORER-9 Nova/Cinder
//! detail an instance will carry — E4).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::edges::Edge;

/// The kind of a discovered unit (lock #1: three kinds, one stream, one badge).
///
/// `Peer` comes from the mesh mirror; `LanHost` from the active LAN scan
/// (EXPLORER-2); the four cloud kinds from the `OpenStack` mirror union.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitKind {
    /// An in-mesh Nebula peer (source: the mesh mirror, lock #2).
    Peer,
    /// An off-mesh LAN host discovered by the active scan (EXPLORER-2 producer).
    LanHost,
    /// A Nova compute instance (cloud object, lock #4).
    Instance,
    /// A Cinder volume (cloud object).
    Volume,
    /// A Glance image (cloud object).
    Image,
    /// A Neutron network (cloud object).
    Network,
}

impl UnitKind {
    /// The stable, lowercase token used in the unit id + the type badge.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Peer => "peer",
            Self::LanHost => "lan_host",
            Self::Instance => "instance",
            Self::Volume => "volume",
            Self::Image => "image",
            Self::Network => "network",
        }
    }
}

/// Where a unit sits relative to the mesh (lock #10 — the reachability line).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "where", rename_all = "snake_case")]
pub enum Reachability {
    /// Inside the mesh — a live Nebula peer.
    InMesh,
    /// Seen on the local LAN, outside the mesh (EXPLORER-2).
    OnLan,
    /// A cloud object hosted on `node` (the host-node tag, lock #20).
    CloudObject {
        /// The mesh node that hosts this object (dom0/compute — the DCIM
        /// "rack" relation E2/E7 later renders).
        node: String,
    },
}

/// A unit's coarse health, where a real source reports it.
///
/// Mirrors the `PeerRecord` 3-tier + unreachable/unknown vocabulary the tray/Fleet
/// already read; an unprobed unit carries `None` on the [`Unit`], never a
/// fabricated `Healthy` (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    /// No active alarms.
    Healthy,
    /// A warning-tier alarm is active.
    Degraded,
    /// A critical-tier alarm is active.
    Critical,
    /// Known to the directory but not currently reachable.
    Unreachable,
    /// Reported, but the tier couldn't be classified.
    Unknown,
}

impl Health {
    /// Map a `PeerRecord.health` string (`healthy`/`degraded`/`critical`/
    /// `unreachable`/anything-else) onto the typed tier. An unrecognised value
    /// is `Unknown` (never guessed into a healthy state).
    #[must_use]
    pub fn from_mesh(s: &str) -> Self {
        match s {
            "healthy" => Self::Healthy,
            "degraded" => Self::Degraded,
            "critical" => Self::Critical,
            "unreachable" => Self::Unreachable,
            _ => Self::Unknown,
        }
    }
}

/// Rich telemetry for a unit we can actually read (lock #11 — mesh peers,
/// instances).
///
/// EXPLORER-1 leaves it absent on every unit; EXPLORER-4 folds the live sparkline
/// sources in. Every field is `Option` so a partially-readable unit is honest
/// field-by-field (§7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Telemetry {
    /// 1-minute load average, when readable.
    pub load1: Option<f32>,
    /// Memory-used percentage (0–100), when readable.
    pub mem_used_pct: Option<f32>,
    /// Uptime in seconds, when readable.
    pub uptime_s: Option<u64>,
}

/// Mesh-mirror facts folded onto a `Peer` unit (source (a)).
///
/// The peer's pinned role, its leadership (the `/mesh/leader` lease — "rank"), and
/// its reported `mde` version — all read from the replicated peer directory the
/// Fleet plane already reads (no probe), so this is EXPLORER-1's fold, distinct
/// from the EXPLORER-9 Nebula cert/groups/uptime enrichment (E5). `None` on
/// non-peer units.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MeshFacts {
    /// The peer's pinned deployment role (`lighthouse`/`workstation`), when the
    /// directory row carries one.
    pub role: Option<String>,
    /// Whether this peer currently holds the `/mesh/leader` lease.
    pub leader: bool,
    /// The peer's installed `mde` version, when detected.
    pub mde_version: Option<String>,
}

/// The open enrichment block EXPLORER-9 fills (E5).
///
/// Reverse-DNS/mDNS names, offline MAC-OUI vendor, a service/port fingerprint →
/// type guess, and mesh cert/role metadata. EXPLORER-1 leaves every field empty
/// (unprobed ⇒ honest unknown, §7). The trailing `extra` map keeps the struct open
/// so a later slice can attach discovered key/values without a model migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Extras {
    /// Reverse-DNS / mDNS name (E5). EXPLORER-9.
    pub rdns: Option<String>,
    /// MAC OUI vendor from the offline table (E5). EXPLORER-9.
    pub oui_vendor: Option<String>,
    /// Service/port fingerprint → type guess (E5). EXPLORER-9.
    pub fingerprint: Option<String>,
    /// Mesh cert identity / groups metadata (E5). EXPLORER-9.
    pub cert_role: Option<String>,
    /// Free-form discovered key/values — the open tail (§7 honest: absent keys
    /// simply aren't present).
    pub extra: BTreeMap<String, String>,
}

/// One discovered unit — the Hero card's data (design "Architecture").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Unit {
    /// Stable, source-namespaced id (see [`peer_unit_id`] / [`lan_unit_id`] /
    /// the cloud id built from [`UnitKind`]). The dedup + first/last-seen key.
    pub id: String,
    /// The kind badge.
    pub kind: UnitKind,
    /// Big display name (lock #10).
    pub name: String,
    /// In-mesh / on-LAN / cloud-object+node (lock #10).
    pub reachability: Reachability,
    /// Best-known address (a peer's Nebula overlay IP; `None` until EXPLORER-2
    /// probes a LAN host — honest unknown, §7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// Coarse health where a real source reports it; `None` ⇒ unprobed (§7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<Health>,
    /// Rich telemetry where readable (EXPLORER-4 fills); `None` ⇒ unprobed (§7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<Telemetry>,
    /// Mesh-mirror facts for a `Peer` (role/leader/version); `None` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<MeshFacts>,
    /// First observation, ms since the Unix epoch — preserved across a
    /// vanish→return (E10, lock).
    pub first_seen_ms: u64,
    /// Most-recent observation, ms since the Unix epoch (E10).
    pub last_seen_ms: u64,
    /// Enrichment block EXPLORER-9 fills; empty here (§7).
    #[serde(default)]
    pub extras: Extras,
}

impl Unit {
    /// Equality ignoring the first/last-seen timestamps — the per-unit half of
    /// the publish-on-change gate. `last_seen_ms` bumps every tick for a present
    /// unit, so comparing it would defeat publish-on-change; `first_seen_ms` is
    /// stable but excluded for symmetry. Everything that constitutes an
    /// observable *change* (identity, reachability, address, health, telemetry,
    /// mesh facts, enrichment) is compared.
    #[must_use]
    pub fn same_ignoring_time(&self, other: &Self) -> bool {
        self.id == other.id
            && self.kind == other.kind
            && self.name == other.name
            && self.reachability == other.reachability
            && self.address == other.address
            && self.health == other.health
            && self.telemetry == other.telemetry
            && self.mesh == other.mesh
            && self.extras == other.extras
    }
}

/// The stable unit id for a mesh peer (or self): `peer:<hostname>`.
#[must_use]
pub fn peer_unit_id(hostname: &str) -> String {
    format!("peer:{hostname}")
}

/// The stable unit id for a LAN host (EXPLORER-2 producer): `lan:<key>`, where
/// `key` is the host's MAC (preferred, stable) or its IP (fallback).
#[must_use]
pub fn lan_unit_id(key: &str) -> String {
    format!("lan:{key}")
}

/// The body published to `state/units/<node>` — the same per-node Bus mirror
/// idiom `state/openstack/<node>` (QC-2) and `state/storage/<node>`
/// (BUG-STORAGE-1) use.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitsState {
    /// The publishing node id (the mirror `host` stamp + topic namespace).
    pub host: String,
    /// Every unit this node folded, in proximity order: self first (lock #23),
    /// then mesh peers by name, then LAN hosts, then cloud objects (lock #7).
    pub units: Vec<Unit>,
    /// The typed relationships derived from the SAME unioned sources (EXPLORER-7,
    /// E2/E8) — published alongside the units so a client renders connectivity
    /// chips without recomputing them. Empty when no source yields an edge (§7).
    #[serde(default)]
    pub edges: Vec<Edge>,
    /// Wall-clock publish time, ms since the Unix epoch.
    pub published_at_ms: u64,
}

impl UnitsState {
    /// Equality ignoring publish time + per-unit timestamps — the worker's
    /// publish-on-change gate (mirrors `OpenStackState::same_ignoring_time`).
    ///
    /// The edge set is compared in full: edges are time-independent (derived from
    /// content, not timestamps), so an unchanged fold yields identical edges — but
    /// a foreign-key/adjacency change with a stable unit set IS an observable
    /// change worth republishing.
    #[must_use]
    pub fn same_ignoring_time(&self, other: &Self) -> bool {
        self.host == other.host
            && self.edges == other.edges
            && self.units.len() == other.units.len()
            && self
                .units
                .iter()
                .zip(&other.units)
                .all(|(a, b)| a.same_ignoring_time(b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(id: &str, name: &str, first: u64, last: u64) -> Unit {
        Unit {
            id: id.to_string(),
            kind: UnitKind::Peer,
            name: name.to_string(),
            reachability: Reachability::InMesh,
            address: None,
            health: Some(Health::Healthy),
            telemetry: None,
            mesh: None,
            first_seen_ms: first,
            last_seen_ms: last,
            extras: Extras::default(),
        }
    }

    #[test]
    fn ids_are_namespaced_per_source() {
        assert_eq!(peer_unit_id("anvil"), "peer:anvil");
        assert_eq!(lan_unit_id("aa:bb:cc"), "lan:aa:bb:cc");
        // Distinct namespaces never collide across kinds.
        assert_ne!(peer_unit_id("x"), lan_unit_id("x"));
    }

    #[test]
    fn health_maps_the_mesh_tiers_and_unknown_fallback() {
        assert_eq!(Health::from_mesh("healthy"), Health::Healthy);
        assert_eq!(Health::from_mesh("degraded"), Health::Degraded);
        assert_eq!(Health::from_mesh("critical"), Health::Critical);
        assert_eq!(Health::from_mesh("unreachable"), Health::Unreachable);
        // Anything unrecognised is Unknown, never guessed healthy.
        assert_eq!(Health::from_mesh("weird"), Health::Unknown);
        assert_eq!(Health::from_mesh(""), Health::Unknown);
    }

    #[test]
    fn change_gate_ignores_time_but_catches_content() {
        let a = unit("peer:x", "x", 100, 200);
        // Same content, later last_seen (a heartbeat) → NOT a change.
        let mut heartbeat = a.clone();
        heartbeat.last_seen_ms = 999;
        heartbeat.first_seen_ms = 100;
        assert!(a.same_ignoring_time(&heartbeat));
        // A real health change IS a change.
        let mut changed = a.clone();
        changed.health = Some(Health::Critical);
        assert!(!a.same_ignoring_time(&changed));
    }

    #[test]
    fn units_state_change_gate_folds_over_units() {
        let s1 = UnitsState {
            host: "node-a".into(),
            units: vec![unit("peer:a", "a", 1, 2), unit("peer:b", "b", 1, 2)],
            edges: vec![],
            published_at_ms: 10,
        };
        // Same units, fresh stamps → not a change.
        let mut s2 = s1.clone();
        s2.published_at_ms = 20;
        s2.units[0].last_seen_ms = 50;
        s2.units[1].last_seen_ms = 50;
        assert!(s1.same_ignoring_time(&s2));
        // A different unit count IS a change.
        let mut s3 = s1.clone();
        s3.units.pop();
        assert!(!s1.same_ignoring_time(&s3));
        // A renamed unit IS a change.
        let mut s4 = s1.clone();
        s4.units[1].name = "b2".into();
        assert!(!s1.same_ignoring_time(&s4));
    }

    #[test]
    fn state_round_trips_json() {
        let state = UnitsState {
            host: "node-a".into(),
            units: vec![Unit {
                id: peer_unit_id("node-a"),
                kind: UnitKind::Peer,
                name: "node-a".into(),
                reachability: Reachability::InMesh,
                address: Some("10.42.0.1".into()),
                health: Some(Health::Healthy),
                telemetry: None,
                mesh: Some(MeshFacts {
                    role: Some("lighthouse".into()),
                    leader: true,
                    mde_version: Some("12.0.0".into()),
                }),
                first_seen_ms: 1,
                last_seen_ms: 2,
                extras: Extras::default(),
            }],
            edges: vec![Edge {
                kind: super::super::edges::EdgeKind::MeshTunnel,
                from: peer_unit_id("node-a"),
                to: peer_unit_id("node-b"),
                detail: Some("direct".into()),
            }],
            published_at_ms: 3,
        };
        let json = serde_json::to_string(&state).expect("serialize");
        let back: UnitsState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, state);
        // The edge set rides alongside units on the published body (E8/E9).
        assert_eq!(back.edges.len(), 1);
        assert_eq!(
            back.edges[0].kind,
            super::super::edges::EdgeKind::MeshTunnel
        );
        // Cloud reachability round-trips the host-node tag.
        let cloud = Reachability::CloudObject {
            node: "node-b".into(),
        };
        let j = serde_json::to_string(&cloud).expect("serialize");
        assert!(j.contains("node-b"), "{j}");
        assert_eq!(
            serde_json::from_str::<Reachability>(&j).expect("deserialize"),
            cloud
        );
    }
}
