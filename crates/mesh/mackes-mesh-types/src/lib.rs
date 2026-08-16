//! Shared mesh-resource types, consumed by `mackesd`, the GUI
//! surfaces, and `mackes-config`.
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

/// WL-FUNC-012 / OVERLAY-7 — credential-gated US EPA AirNow AQI snapshots.
pub mod air_quality;
/// WL-FUNC-012 / OVERLAY-8 — point-scoped adsb.lol aircraft snapshots shared
/// by the workstation adapter and Maps & Location.
pub mod aircraft;
/// Typed AOSP starter-app catalog and per-Android-VM inventory contracts.
pub mod android_apps;
/// WL-FUNC-020 — bounded Cuttlefish Android VM provider/lifecycle contracts.
pub mod android_provider;
/// WL-FUNC-020 — shared host/guest Cuttlefish relay wire protocol.
pub mod cuttlefish_guest;
/// WL-FUNC-018 — versioned, signed-provenance Flatpak catalog records for App VMs.
pub mod app_catalog;
/// WL-FUNC-012 / OVERLAY-5 — Caltrans CWWP2 camera snapshots shared by the
/// workstation adapter and Maps & Location.
pub mod caltrans_camera;
pub mod cap_tags;
// WL-ARCH-001 (2026-07-18) — provider-neutral Construct Cloud shared contracts.
// The SOLE definition site for the mesh cloud's catalog/resource/health/stack
// shapes + the `action/cloud/*` lifecycle command contract; all consumers bind
// to `cloud::*`.
pub mod clock;
pub mod cloud;
pub mod connect;
// CONNECT-1 (2026-06-19) — unified connectivity / exposure policy model + state.
pub mod ddns;
// DEVMGR-1 (2026-07-04) — the device-inventory schema: the §6 JSON contract
// between the mesh-side producer (mackesd `hardware_probe`) and the desktop-side
// About → Device-Manager surface. Lands here (like `peer_probe`, the other
// hardware schema) so neither side depends on the other.
pub mod device_inventory;
// DEVMGR-8 — the device-control request/result §6 contract: the desktop shell
// dispatches a typed privileged-op request, mackesd's `device_control` worker
// executes it on the target node. Lands here so neither side depends on the other.
/// WL-FUNC-012 / MG90 airspace survey snapshot shared by mackesd and Maps.
pub mod airspace;
pub mod device_control;
/// WL-FUNC-012 / OVERLAY-10 — keyless USGS earthquake latest-wins snapshot
///
/// shared by the workstation-side adapter and the Maps & Location surface.
pub mod earthquake;
pub mod exposure;
/// WL-FUNC-012 / OVERLAY-6 — credential-gated NASA FIRMS hotspot snapshots.
pub mod firms;
/// Canonical system-and-mesh health contracts shared by the daemon and shell.
pub mod health;
/// WL-FUNC-012 / OVERLAY-2 — keyless IEM/NWS animated radar tiles.
pub mod iem_radar;
pub mod traffic;
pub mod wildfire;
// LIGHTHOUSE-2 (2026-06-18) — shared lighthouse discovery + binary health
// (beacon) derivation from the replicated peer directory. One pure source for
// the Hub footer, the Workbench Lighthouses tab, and the panel applet so the
// "healthy/unhealthy" rule (docs/design/lighthouse-hero.md Q1/Q2/Q3/Q15) lives
// in exactly one place.
pub mod lighthouse;
// LIGHTHOUSE-8 (2026-06-24) — the deep-probe result type (handshake / public IP
// / peer count / uptime / CA cert-expiry) the `lighthouse_probe` worker publishes
// to `compute/lighthouse-probe/<name>` and the Workbench Lighthouses tab renders.
// The replicated directory carries only binary health; these live operational
// facts need a per-lighthouse probe lane (LIGHTHOUSE follow-on, now filled).
pub mod lighthouse_probe;
/// WL-FUNC-017 — bounded weather-location preference and effective-location contracts.
pub mod location;
/// WL-FUNC-023 — bounded local ONBOARD & OFFBOARDING intent contract.
pub mod lifecycle;
/// WL-FUNC-015 — shared `state/media/sources` wire records published by
/// `mackesd` and consumed by the Media Workspace without a daemon dependency.
pub mod media_sources;
/// WL-FUNC-021 — public-verifier contract for Music workspace mutations.
pub mod music_auth;
/// Typed, credential-free NetworkManager/ModemManager link observations for
/// the additive `network.interfaces[]` mesh-status field.
pub mod network_status;
/// WL-FUNC-011 / WL-FUNC-019 — bounded OpenSubsonic-compatible media admission.
pub mod subsonic;
// arch-7 (2026-07-11) — the canonical shared-storage mount constant +
// the AUDIT-MESH-15 write-safety guard, relocated out of the `mackesd` bin
// crate so worker crates factored out of the daemon
// reuse the one audited guard. `mackesd` re-exports at its crate root.
pub mod mesh_storage;
// NF-11.1 (v2.5) — Nebula facts surface for the peer card.
/// WL-FUNC-017 S6 — bounded daemon-owned route/navigation contracts.
pub mod navigation;
pub mod nebula;
/// WL-FUNC-012 / OVERLAY-1 — keyless NWS active-alert snapshot shared by the
/// workstation adapter and Maps & Location.
pub mod nws_alert;
pub mod nws_forecast;
pub mod peer_probe;
// PEERVER-1 (v2.7, 2026-05-29) — peer-data convergence records.
// Shared home so mackesd (writer, heartbeat tick) + mde-installer
// (reader) use one path; docs/design/v2.7-peer-data-convergence.md.
pub mod peers;
/// WL-FUNC-019 — versioned universal resource identity, capability, transport,
///
/// provenance, auth/health, catalog, and bounded-action contracts.
pub mod resources;
/// ROUTE-TRACE-1 — the typed PathGraph model for `action/route/trace`.
pub mod route_trace;
/// WL-FUNC-019 — bounded SSH/SFTP browsing and X11 resource admission.
pub mod ssh_x11;
/// Strict versioned Surface Pro 5/6 enable-result publication contract.
pub mod surface_enable;
/// WL-UX-011 — bounded Microsoft Surface Pro 5/6 observation and action contracts.
pub mod surface_hardware;
/// WL-ARCH-009 — neutral, strict-versioned worker runtime and change-set contracts.
pub mod worker_runtime;
// WL-RUN-006 (2026-07-19) — the router firewall-edit verb (`action/router/*`
// `RouterActionRequest`) + its tamper-evident audit schema. The "mutations
// fast-follow" of the router-control read slice: the shell's Device-Manager
// composes an edit; the mackesd `router_action` worker wraps it in Vyatta
// commit-confirm behind a typed-confirm gate. Lands here (like `device_control`)
// so neither side depends on the other.
pub mod router_action;
// WL-FUNC-008 (2026-07-19) — the unified service provenance/health record: the ONE
// type merging published (`kdc-services`) + probe (`probe-inventory`) + enrichment
// service facts. The mackesd `service_aggregator` worker produces it on
// `state/services/<node>`; the shell's Phones-hub Services view renders it. Lands
// here (like `mesh_storage` / `vdi_session`) so neither side depends on the other.
pub mod service_record;
// Portal-18.a (v6.0 R12 lock 2026-05-26) — universal tag schema +
// per-peer storage layer. Lands here (rather than in a fresh crate)
// because every existing consumer of `mackes-mesh-types` is also a
// consumer of tags (Peer / Workspace / Container members reference
// mesh-domain identifiers).
pub mod tags;
/// WL-FUNC-012 / OVERLAY-9 — MBTA GTFS-Realtime vehicle snapshots shared by
/// the workstation adapter and Maps & Location.
pub mod transit;
// arch-2 (2026-07-11) — the VDI session-lifecycle wire verb (`action/vdi/session`
// `SessionRequest`), hoisted out of the `mackesd` session broker so the shell's
// `discovery` / `session_rail` mirrors reuse the one type instead of maintaining
// byte-compatible copies. Lands here (like `mesh_storage` / `device_control`) so
// the desktop tier never depends on the heavy daemon crate.
pub mod vdi_clipboard;
pub mod vdi_session;
/// Rolling Node — the provider-neutral vehicle-gateway (`state/vehicle/<node>`) mirror
/// + `action/vehicle/*` command contract + a pure NMEA GGA parser. A workstation-side
/// adapter (mackesd `vehicle` worker) SSH/HTTP-polls a mobile gateway (AirLink MG90) and
/// the maps-location cockpit folds this mirror into its live models.
pub mod vehicle;
/// VPN-GW-1 — the VPN tunnel definition model + pure wg-quick/openvpn helpers.
pub mod vpn;
/// VPN-GW-3 — selective egress: fwmark/ip-rule policy routing + nftables
///
/// masquerade + a leak-proof kill-switch, with the Nebula overlay carved out so
/// mesh traffic never tunnels. Pure argv builders applied by the `vpn_gw`
/// responder on tunnel up/down. Also holds VPN-GW-4 — the mesh egress *routing*
/// table (per-node / group / ANY) + the ordered failover chain the `vpn_gw`
/// responder serves over `action/vpn/{set,clear,list,…}-route`.
pub mod vpn_egress;
/// VPN-GW-5 — first-class provider adapters (Mullvad/Proton/IVPN/Nord/Surfshark)
/// + the generic "paste WG config" / "import .ovpn" config-generation paths.
pub mod vpn_providers;
/// WL-FUNC-017 — bounded current conditions and general 120-hour/five-day forecast contracts.
pub mod weather;
/// WL-ARCH-010 — the sole versioned workload operation/state contract.
pub mod workloads;

pub use android_apps::{
    android_catalog_import_topic, android_catalog_state_topic, AndroidAppAvailability,
    AndroidAppCapability, AndroidAppCategory, AndroidAppInventory, AndroidAppInventoryEntry,
    AndroidAppPermission, AndroidAppReadiness, AndroidCatalogAdmissionError,
    AndroidCatalogAppPolicy, AndroidCatalogGuestReadiness, AndroidCatalogPayload,
    AndroidImageManifest, AndroidImagePackage, AndroidImagePackageManifest, AndroidImageProvenance,
    AndroidLaunchIntent, AndroidLaunchReadiness, AndroidPackageVersion, AndroidResourceClass,
    AndroidResourceProfile, AndroidSignedCatalog, AndroidStarterAppDescriptor, AospPackageId,
    AospStarterApp, AospStarterCatalog, ANDROID_CATALOG_IMPORT_TOPIC_PREFIX,
    ANDROID_CATALOG_SIGNATURE_DOMAIN, ANDROID_CATALOG_STATE_TOPIC_PREFIX,
    ANDROID_SIGNED_CATALOG_SCHEMA_VERSION, MAX_ANDROID_CATALOG_TTL_MS,
};
pub use connect::{BatterySnapshot, ConnectFacts, PairingState, PeerKind};
pub use nebula::{NebulaFacts, NebulaRole};
pub use peer_probe::{BusTopology, Descriptors, KernelDriver, NatClass, PeerProbe, PowerThermal};
pub use resources::{
    ActionAvailability, ActionAvailabilityStatus, AuthState, ClientCapability, HealthState,
    LocalServiceStack, ResourceAction, ResourceActionTarget, ResourceActionVerb, ResourceCard,
    ResourceCatalog, ResourceClass, ResourceIdentity, ResourceOperatingRole, ServiceCategory,
    ServiceConfigurationField, ServiceConfigurationFieldKind, ServiceInterface,
    ServiceLifecycleStatus, ServiceStackPlane, ServiceStackTier, SourceProvenance,
    TransportCandidate, RESOURCE_CATALOG_TOPIC,
};
pub use tags::{Tag, TagFlavor, TagMember, TagStore, TagStoreError};
pub use worker_runtime::{
    WorkerAction, WorkerChangeSetItem, WorkerChangeSetItemOutcome, WorkerChangeSetOperation,
    WorkerChangeSetOutcome, WorkerChangeSetRequest, WorkerChangeSetResult, WorkerChangeSetTarget,
    WorkerContract, WorkerGroup, WorkerRelation, WorkerRelationEndpoint, WorkerRelationKind,
    WorkerRuntimeContractError, WorkerRuntimeReason, WorkerRuntimeSnapshot, WorkerRuntimeState,
    WorkerTimelineEvent, WorkerTimelineEventKind,
};
pub use workloads::{
    admit_workload, host_reserve, valid_phase_transition, workload_state_topic, AdmissionDenial,
    HostCapacity, WorkloadAdmission, WorkloadAttachmentLease, WorkloadAttachmentProtocol,
    WorkloadBackend, WorkloadContractError, WorkloadDesiredState, WorkloadHealth, WorkloadId,
    WorkloadKind, WorkloadOperationAction, WorkloadOperationPhase, WorkloadOperationRequest,
    WorkloadOperationStatus, WorkloadPowerState, WorkloadPressure, WorkloadProfile,
    WorkloadReadiness, WorkloadResources, WorkloadRuntimeSignals, WorkloadStateSnapshot,
    MAX_WORKLOADS_PER_NODE, MAX_WORKLOAD_ATTACHMENTS, MAX_WORKLOAD_DEADLINE_MS,
    MAX_WORKLOAD_IDENTIFIER_BYTES, MAX_WORKLOAD_TEXT_BYTES, MAX_WORKLOAD_WIRE_BYTES,
    MIN_HOST_CPU_RESERVE, MIN_HOST_MEMORY_RESERVE_MB, WORKLOAD_CONTRACT_SCHEMA_VERSION,
    WORKLOAD_OPERATION_TOPIC, WORKLOAD_STATE_TOPIC_PREFIX,
};

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
