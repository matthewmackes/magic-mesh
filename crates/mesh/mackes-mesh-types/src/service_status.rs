//! UNIFY-14 — per-node mesh service-status map.
//!
//! `mackesd`'s `service_status` worker samples which of the canonical mesh
//! service units are live on THIS node and publishes a [`ServiceStatusMap`] so
//! every peer can render the Unified Workbench's node×service matrix
//! (`docs/design/workbench/Workbench.dc.html`) with real per-node data. The
//! reporting node is authoritative for its own row.
//!
//! The nine canonical services are heterogeneous — some are systemd units
//! (`etcd`, `syncthing`, `nebula`, voice, music), some are embedded in `mackesd`
//! (the Bus broker), and some are daemonless (`<host>.mesh` DNS is per-link
//! `systemd-resolved` config, KDE Connect + the Workbench are in-process /
//! desktop surfaces). Each carries a real liveness signal on the producer side;
//! where a node genuinely can't determine one (e.g. `systemctl` absent on a
//! non-systemd host) the service reports [`ServiceState::Unknown`] rather than a
//! fabricated up/down (§7 — honest unknowns only).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The nine canonical mesh service units the node×service matrix renders.
///
/// [`MeshService::id`] is the stable wire key used in
/// [`ServiceStatusMap::services`] (never change these — they are the cross-node
/// contract); [`MeshService::label`] is the human column header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MeshService {
    /// Mackes Bus broker (embedded in `mackesd`; no standalone systemd unit).
    Bus,
    /// etcd coordination plane.
    Etcd,
    /// Syncthing file-replication plane.
    Syncthing,
    /// Nebula overlay transport.
    Nebula,
    /// Mesh DNS — daemonless `<host>.mesh` resolution.
    Dns,
    /// Voice (SIP signalling + RTP media).
    Voice,
    /// Music (Navidrome, reachable mesh-wide as `music.mesh`).
    Music,
    /// KDE Connect host (in-process LAN transport on the KDC port).
    Kdc,
    /// Workbench desktop surface.
    Workbench,
}

impl MeshService {
    /// Every canonical service, in matrix-column order.
    pub const ALL: [MeshService; 9] = [
        MeshService::Bus,
        MeshService::Etcd,
        MeshService::Syncthing,
        MeshService::Nebula,
        MeshService::Dns,
        MeshService::Voice,
        MeshService::Music,
        MeshService::Kdc,
        MeshService::Workbench,
    ];

    /// Stable wire id (the [`ServiceStatusMap::services`] map key). These are the
    /// cross-node contract — never rename them.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            MeshService::Bus => "bus",
            MeshService::Etcd => "etcd",
            MeshService::Syncthing => "syncthing",
            MeshService::Nebula => "nebula",
            MeshService::Dns => "dns",
            MeshService::Voice => "voice",
            MeshService::Music => "music",
            MeshService::Kdc => "kdc",
            MeshService::Workbench => "workbench",
        }
    }

    /// Human column header for the matrix.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            MeshService::Bus => "Bus",
            MeshService::Etcd => "etcd",
            MeshService::Syncthing => "Syncthing",
            MeshService::Nebula => "Nebula",
            MeshService::Dns => "DNS",
            MeshService::Voice => "Voice",
            MeshService::Music => "Music",
            MeshService::Kdc => "KDE Connect",
            MeshService::Workbench => "Workbench",
        }
    }

    /// Parse a wire [`id`](MeshService::id) back into the enum.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.id() == id)
    }
}

/// Tri-state liveness of one service on one node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceState {
    /// The unit / listener is up.
    Active,
    /// The unit / listener is installed-but-down or absent on this node.
    Inactive,
    /// No determinable signal on this node (e.g. `systemctl` absent) — an honest
    /// "—", never a fabricated up/down (§7).
    Unknown,
}

impl ServiceState {
    /// `true` only for [`ServiceState::Active`].
    #[must_use]
    pub fn is_active(self) -> bool {
        matches!(self, ServiceState::Active)
    }
}

/// One node's full service-status map: identity + per-service state. This is the
/// payload `mackesd` publishes (`state/service-status/<overlay_ip>` on the Bus +
/// the replicated `<host>/service-status.json` cross-node mirror) and peers
/// aggregate into the node×service matrix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceStatusMap {
    /// Reporting node's hostname (matrix row label).
    pub hostname: String,
    /// Reporting node's Nebula overlay IP — `""` until enrolled (matrix row key).
    pub overlay_ip: String,
    /// Wall-clock sample time, ms since the Unix epoch (reader staleness check).
    pub ts_ms: u64,
    /// Per-service state keyed by [`MeshService::id`]. A missing key is read as
    /// [`ServiceState::Unknown`] by [`Self::state`].
    pub services: BTreeMap<String, ServiceState>,
}

impl ServiceStatusMap {
    /// New empty map for `hostname` / `overlay_ip`, stamped at `ts_ms`.
    #[must_use]
    pub fn new(hostname: impl Into<String>, overlay_ip: impl Into<String>, ts_ms: u64) -> Self {
        Self {
            hostname: hostname.into(),
            overlay_ip: overlay_ip.into(),
            ts_ms,
            services: BTreeMap::new(),
        }
    }

    /// Record `state` for `service` (builder-style, consuming `self`).
    #[must_use]
    pub fn with(mut self, service: MeshService, state: ServiceState) -> Self {
        self.set(service, state);
        self
    }

    /// Set `state` for `service` in place.
    pub fn set(&mut self, service: MeshService, state: ServiceState) {
        self.services.insert(service.id().to_string(), state);
    }

    /// Look up a service's state; a missing entry is the honest
    /// [`ServiceState::Unknown`].
    #[must_use]
    pub fn state(&self, service: MeshService) -> ServiceState {
        self.services
            .get(service.id())
            .copied()
            .unwrap_or(ServiceState::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_ids_are_unique_and_stable() {
        // The wire contract: nine distinct, lowercase, kebab-free ids.
        let ids: Vec<&str> = MeshService::ALL.iter().map(|s| s.id()).collect();
        assert_eq!(ids.len(), 9);
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 9, "service ids must be unique");
        assert_eq!(
            ids,
            [
                "bus",
                "etcd",
                "syncthing",
                "nebula",
                "dns",
                "voice",
                "music",
                "kdc",
                "workbench",
            ]
        );
    }

    #[test]
    fn from_id_round_trips_every_service() {
        for svc in MeshService::ALL {
            assert_eq!(MeshService::from_id(svc.id()), Some(svc));
            assert!(!svc.label().is_empty());
        }
        assert_eq!(MeshService::from_id("nope"), None);
    }

    #[test]
    fn service_state_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&ServiceState::Active).unwrap(),
            "\"active\""
        );
        assert_eq!(
            serde_json::to_string(&ServiceState::Inactive).unwrap(),
            "\"inactive\""
        );
        assert_eq!(
            serde_json::to_string(&ServiceState::Unknown).unwrap(),
            "\"unknown\""
        );
        // …and decodes back.
        let back: ServiceState = serde_json::from_str("\"active\"").unwrap();
        assert_eq!(back, ServiceState::Active);
        assert!(ServiceState::Active.is_active());
        assert!(!ServiceState::Unknown.is_active());
    }

    #[test]
    fn missing_service_reads_as_unknown() {
        // §7 — a node that didn't sample a service yields an honest Unknown,
        // never a defaulted Inactive.
        let map = ServiceStatusMap::new("anvil", "10.42.0.7", 42);
        for svc in MeshService::ALL {
            assert_eq!(map.state(svc), ServiceState::Unknown);
        }
    }

    #[test]
    fn builder_records_per_service_state() {
        let map = ServiceStatusMap::new("anvil", "10.42.0.7", 100)
            .with(MeshService::Nebula, ServiceState::Active)
            .with(MeshService::Etcd, ServiceState::Inactive)
            .with(MeshService::Workbench, ServiceState::Unknown);
        assert_eq!(map.state(MeshService::Nebula), ServiceState::Active);
        assert_eq!(map.state(MeshService::Etcd), ServiceState::Inactive);
        assert_eq!(map.state(MeshService::Workbench), ServiceState::Unknown);
        // An unset service stays Unknown.
        assert_eq!(map.state(MeshService::Music), ServiceState::Unknown);
    }

    #[test]
    fn full_map_encode_decode_round_trips() {
        let mut map = ServiceStatusMap::new("forge", "10.42.0.3", 1_700_000_000_000);
        for (i, svc) in MeshService::ALL.into_iter().enumerate() {
            // Deterministically spread the three states across the nine services.
            let state = match i % 3 {
                0 => ServiceState::Active,
                1 => ServiceState::Inactive,
                _ => ServiceState::Unknown,
            };
            map.set(svc, state);
        }
        let json = serde_json::to_string(&map).unwrap();
        // Identity + every service key present on the wire.
        assert!(json.contains("\"hostname\":\"forge\""));
        assert!(json.contains("\"overlay_ip\":\"10.42.0.3\""));
        assert!(json.contains("\"ts_ms\":1700000000000"));
        for svc in MeshService::ALL {
            assert!(
                json.contains(&format!("\"{}\"", svc.id())),
                "missing {}",
                svc.id()
            );
        }
        let back: ServiceStatusMap = serde_json::from_str(&json).unwrap();
        assert_eq!(back, map);
        assert_eq!(back.services.len(), 9);
    }
}
