//! Shared mesh-resource types, consumed by `mackesd`, `mde-workbench`,
//! and `mackes-config`.
//!
//! A `MeshResource` is anything the mackes mesh exposes that can be rendered
//! as a first-class dock item — a peer, a mounted share, or an advertised
//! service. Per the 50-question lock (Q9 / Q10 / Q33), these interleave
//! with apps in the bottom dock.
//!
//! ## Peer-probe schema (PC-2)
//!
//! [`peer_probe::PeerProbe`] + its section types live here as
//! their production home (PC-2 lock, 2026-05-21). Consumers
//! (`mded`'s peer-join worker, `mde-peer-card`, future tooling)
//! import via `use mackes_mesh_types::peer_probe::*;`.

#![forbid(unsafe_code)]

pub mod cap_tags;
pub mod connect;
// CONNECT-1 (2026-06-19) — unified connectivity / exposure policy model + state.
pub mod ddns;
pub mod exposure;
// LIGHTHOUSE-2 (2026-06-18) — shared lighthouse discovery + binary health
// (beacon) derivation from the replicated peer directory. One pure source for
// the Hub footer, the Workbench Lighthouses tab, and the panel applet so the
// "healthy/unhealthy" rule (docs/design/lighthouse-hero.md Q1/Q2/Q3/Q15) lives
// in exactly one place.
pub mod lighthouse;
// NF-11.1 (v2.5) — Nebula facts surface for the peer card.
pub mod nebula;
pub mod peer_probe;
// PEERVER-1 (v2.7, 2026-05-29) — peer-data convergence records.
// Shared home so mackesd (writer, heartbeat tick) + mde-installer
// (reader) use one path; docs/design/v2.7-peer-data-convergence.md.
pub mod peers;
/// ROUTE-TRACE-1 — the typed PathGraph model for `action/route/trace`.
pub mod route_trace;
// Portal-18.a (v6.0 R12 lock 2026-05-26) — universal tag schema +
// per-peer storage layer. Lands here (rather than in a fresh crate)
// because every existing consumer of `mackes-mesh-types` is also a
// consumer of tags (Peer / Workspace / Container members reference
// mesh-domain identifiers).
pub mod tags;
/// VPN-GW-1 — the VPN tunnel definition model + pure wg-quick/openvpn helpers.
pub mod vpn;
/// VPN-GW-3 — selective egress: fwmark/ip-rule policy routing + nftables
/// masquerade + a leak-proof kill-switch, with the Nebula overlay carved out so
/// mesh traffic never tunnels. Pure argv builders applied by the `vpn_gw`
/// responder on tunnel up/down.
pub mod vpn_egress;
/// VPN-GW-5 — first-class provider adapters (Mullvad/Proton/IVPN/Nord/Surfshark)
/// + the generic "paste WG config" / "import .ovpn" config-generation paths.
pub mod vpn_providers;

pub use connect::{BatterySnapshot, ConnectFacts, PairingState, PeerKind};
pub use nebula::{NebulaFacts, NebulaRole};
pub use peer_probe::{BusTopology, Descriptors, KernelDriver, NatClass, PeerProbe, PowerThermal};
pub use tags::{Tag, TagFlavor, TagMember, TagStore, TagStoreError};

use serde::{Deserialize, Serialize};

/// One thing the mesh exposes that the panel can render as a dock item.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MeshResource {
    /// A mesh peer (Nebula-enrolled machine). Click → action popover (Q34):
    /// Files / SSH / RDP / VNC / Services / Send file.
    Peer {
        /// Hostname / mesh node name. Stable across reboots.
        name: String,
        /// Mesh IP (Nebula overlay address, e.g. 10.42.x.x).
        mesh_ip: Option<String>,
        /// Whether the peer has been seen as online in the last poll.
        online: bool,
    },

    /// A QNM-Shared bucket exposed by a peer. Click → Thunar at the share.
    MountedShare {
        /// Owning peer's name.
        peer: String,
        /// Bucket path under `~/QNM-Shared/`.
        bucket: String,
    },

    /// A service the mesh advertises (Sublime Music, Delfin, Caddy, …).
    /// Click → opens the service's URL or launches its client.
    Service {
        /// Owning peer's name (or `local` if this peer hosts it).
        peer: String,
        /// Service slug (`sublime-music`, `delfin`, `caddy`, …).
        slug: String,
        /// Service URL the dock click should open.
        url: String,
    },
}

impl MeshResource {
    /// Stable identifier used to look up the resource's Material Symbols icon
    /// and to dedupe entries in the dock's pin list.
    #[must_use]
    pub fn id(&self) -> String {
        match self {
            Self::Peer { name, .. } => format!("peer:{name}"),
            Self::MountedShare { peer, bucket } => format!("share:{peer}:{bucket}"),
            Self::Service { peer, slug, .. } => format!("svc:{peer}:{slug}"),
        }
    }

    /// Human-readable label rendered in the dock tooltip.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Peer {
                name, online: true, ..
            } => format!("{name} (online)"),
            Self::Peer {
                name,
                online: false,
                ..
            } => format!("{name} (offline)"),
            Self::MountedShare { peer, bucket } => format!("{peer}: {bucket}"),
            Self::Service { peer, slug, .. } => format!("{peer}: {slug}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_id_is_stable() {
        let r = MeshResource::Peer {
            name: "anvil".into(),
            mesh_ip: Some("100.64.0.7".into()),
            online: true,
        };
        assert_eq!(r.id(), "peer:anvil");
    }

    #[test]
    fn service_id_includes_peer_and_slug() {
        let r = MeshResource::Service {
            peer: "anvil".into(),
            slug: "sublime-music".into(),
            url: "http://anvil.mesh:4040".into(),
        };
        assert_eq!(r.id(), "svc:anvil:sublime-music");
    }

    #[test]
    fn label_reflects_online_state() {
        let online = MeshResource::Peer {
            name: "anvil".into(),
            mesh_ip: None,
            online: true,
        };
        let offline = MeshResource::Peer {
            name: "anvil".into(),
            mesh_ip: None,
            online: false,
        };
        assert!(online.label().contains("online"));
        assert!(offline.label().contains("offline"));
    }

    #[test]
    fn mounted_share_id_and_label() {
        let r = MeshResource::MountedShare {
            peer: "anvil".into(),
            bucket: "code".into(),
        };
        assert_eq!(r.id(), "share:anvil:code");
        let l = r.label();
        assert!(l.contains("anvil"));
        assert!(l.contains("code"));
    }

    #[test]
    fn service_label_carries_peer_and_slug() {
        let r = MeshResource::Service {
            peer: "anvil".into(),
            slug: "sublime-music".into(),
            url: "http://anvil.mesh:4040".into(),
        };
        let l = r.label();
        assert!(l.contains("anvil"));
        assert!(l.contains("sublime-music"));
    }

    #[test]
    fn round_trips_through_json_for_every_variant() {
        let cases = vec![
            MeshResource::Peer {
                name: "anvil".into(),
                mesh_ip: Some("100.64.0.7".into()),
                online: true,
            },
            MeshResource::MountedShare {
                peer: "anvil".into(),
                bucket: "code".into(),
            },
            MeshResource::Service {
                peer: "anvil".into(),
                slug: "sublime-music".into(),
                url: "http://example.test".into(),
            },
        ];
        for r in cases {
            let s = serde_json::to_string(&r).expect("serialize");
            let back: MeshResource = serde_json::from_str(&s).expect("deserialize");
            assert_eq!(back, r);
        }
    }

    #[test]
    fn equal_resources_hash_equal_and_clone() {
        use std::collections::HashSet;
        let a = MeshResource::Peer {
            name: "anvil".into(),
            mesh_ip: None,
            online: true,
        };
        let b = a.clone();
        let mut set: HashSet<MeshResource> = HashSet::new();
        set.insert(a);
        // Same variant + fields → dedupe.
        assert!(set.contains(&b));
        // Different variant → distinct entry.
        let svc = MeshResource::Service {
            peer: "anvil".into(),
            slug: "x".into(),
            url: "u".into(),
        };
        set.insert(svc.clone());
        assert_eq!(set.len(), 2);
    }
}
