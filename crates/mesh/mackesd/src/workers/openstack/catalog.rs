//! QC-2 — the Kolla service catalog: which `OpenStack` services exist, their
//! Red-Hat-convention Kolla container/image names, and where each one places.
//!
//! The catalog is the pure vocabulary every QUASAR-CLOUD slice shares
//! (`docs/design/quasar-cloud.md`): the MVP service set (Q24 — Nova+Placement,
//! Neutron, Glance, Cinder, +Keystone) over the foundation trio (Q15/16/17 —
//! leader-hosted `MariaDB`, clustered `RabbitMQ`, per-node memcached). QC-3
//! mapped each entry to its Syncthing-mirrored image archive
//! ([`ServiceKind::archive_file_name`] — the [`super::images`] lane). Later
//! QC tasks keep extending it in place: QC-4 renders each entry's Kolla
//! config, QC-6 binds each API entry to the Nebula interface, QC-19 landed
//! the wave-2 Heat/Octavia/Horizon variants (Q25/47/61), and QC-17 lands the
//! wave-2 Designate naming plane (Q46 — Designate replaces DNS/naming).

use serde::{Deserialize, Serialize};

/// Where a service places across the fleet (the design's "no controller box"
/// doctrine, Q5).
///
/// The fold in [`super::fleet::desired_services`] turns this + the node's
/// doctrine view into the node's desired service set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Runs on every OpenStack-carrying node (Q22 "APIs on every node" + the
    /// per-node data/compute plane — nova-compute, cinder-volume's node-local
    /// LVM (Q51) + its cinder-backup agent (Q57), memcached (Q17), a `RabbitMQ`
    /// cluster member (Q16)).
    EveryNode,
    /// Rides the etcd leader and re-places on failover (Q15 — `MariaDB` is a
    /// workload on the leader, never a permanently-special node).
    LeaderOnly,
}

/// One Kolla-packaged `OpenStack` service the `openstack` worker supervises.
///
/// The variant set is the QC-2 skeleton catalog: the QC-4 foundation trio +
/// the QC-5/6 identity + core-API set + the QC-7/8 OVN/Cinder plane, extended
/// by QC-19 with the wave-2 services (Heat, Octavia, and the optional Horizon —
/// Q25/47/61), by QC-17 with the Designate naming plane (Q46), and by QC-18
/// with the Swift object tier (Q54/55/57). Names follow the Kolla conventions
/// the mirrored archives carry: container names use underscores (`nova_api`),
/// image basenames use dashes (`nova-api`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceKind {
    // ── Foundation (QC-4) ──
    /// The control-plane database — leader-hosted (Q15).
    Mariadb,
    /// OpenStack-internal RPC broker, quorum queues (Q16). Strictly internal —
    /// mde-bus stays THE platform bus (Q67).
    Rabbitmq,
    /// Per-node cache (Q17).
    Memcached,
    // ── Identity + core APIs (QC-5/QC-6, on every node — Q22) ──
    /// Identity — the mesh account IS the cloud account (Q21/62/87).
    Keystone,
    /// Image service API (Q36/53).
    GlanceApi,
    /// Placement API (Q31).
    PlacementApi,
    /// Compute API (Q31).
    NovaApi,
    /// Compute scheduler.
    NovaScheduler,
    /// Compute conductor.
    NovaConductor,
    /// The hypervisor driver — libvirt/QEMU-KVM host bits ride the image
    /// (QC-1/Q32); this container is the Nova agent over them.
    NovaCompute,
    /// Networking API — ML2/OVN (Q42), one flat provider net (Q43).
    NeutronServer,
    // ── OVN control plane (QC-7 — the ML2/OVN backend for the flat mesh net) ──
    /// OVN northbound OVSDB (Q42) — Neutron writes the logical network here.
    /// Leader-hosted like `MariaDB` (Q15): one central DB, re-placed on failover.
    OvnNbDb,
    /// OVN southbound OVSDB — `ovn-northd` compiles logical → physical flows
    /// here and every chassis's `ovn-controller` reads them. Leader-hosted.
    OvnSbDb,
    /// OVN northbound daemon — translates NB → SB. Single active instance, so
    /// leader-only (Q15).
    OvnNorthd,
    /// The per-chassis OVN agent (Q42/43) — programs the host Open vSwitch (the
    /// OVS datapath rides the image, Q12) for the flat provider net. Every node.
    OvnController,
    /// Block-storage API (Q51).
    CinderApi,
    /// Block-storage scheduler.
    CinderScheduler,
    /// Node-local LVM volume service (Q51/59).
    CinderVolume,
    /// Node-local volume-backup service → the Swift object tier (QC-8/Q57).
    /// Rides every node beside `cinder-volume`: the volumes it streams to the
    /// object store are node-local LVM (Q51/59), so the backup agent runs where
    /// they live — never a controller box (Q5/22).
    CinderBackup,
    // ── Object tier (QC-18, Q54/55/57) ──
    /// Swift proxy API — Keystone-native hot object tier at `swift.mesh:8080`;
    /// Cinder backups land here and the off-site DO Spaces leg is Swift
    /// replication, not a second Cinder driver.
    SwiftProxyServer,
    /// Swift account server — ring member for account DB partitions.
    SwiftAccountServer,
    /// Swift container server — ring member for container DB partitions.
    SwiftContainerServer,
    /// Swift object server — ring member for object partitions on the node's
    /// writable object-store directory.
    SwiftObjectServer,
    // ── Wave-2 orchestration + load-balancing + dashboard (QC-19, Q25/47/61) ──
    /// Heat orchestration API (Q61) — serves `openstack stack {list,create}`.
    /// The fleet is authoritative: the worker renders stacks from fleet state
    /// ([`super::config_render::render_fleet_heat_stack`]) and Heat executes
    /// them; APIs on every node (Q22).
    HeatApi,
    /// Heat CloudFormation-compatible API (the `heat-api-cfn` endpoint the
    /// wait-condition/metadata resources call back to).
    HeatApiCfn,
    /// Heat engine — the orchestration worker that realizes a stack's resources.
    HeatEngine,
    /// Octavia load-balancing API (Q47) — instance-workload LBs (platform
    /// ingress keeps the Lighthouse Caddy).
    OctaviaApi,
    /// Octavia worker — drives the amphora lifecycle for a load balancer.
    OctaviaWorker,
    /// Octavia health-manager — the amphora heartbeat listener + failover.
    OctaviaHealthManager,
    /// Octavia housekeeping — amphora cert rotation + spares-pool/DB cleanup.
    OctaviaHousekeeping,
    /// Horizon dashboard (Q25/26/66) — the **OPTIONAL** web console, desired
    /// only when the doctrine opts in (`horizon = true`); Workbench is the
    /// primary Cloud UI (Q26), so absent-by-default is honest, not a gap.
    Horizon,
    // ── Wave-2 naming (QC-17, Q25/46 — Designate replaces DNS/naming) ──
    /// Designate REST API (Q46) — serves `openstack zone`/`recordset`; the
    /// peer directory feeds (and can re-seed) the zones it fronts.
    DesignateApi,
    /// Designate central — the brains: owns the zone/recordset state machine
    /// the API and workers coordinate through.
    DesignateCentral,
    /// Designate producer — the periodic-task emitter (zone refresh, delayed
    /// NOTIFY, …).
    DesignateProducer,
    /// Designate worker — pushes zone changes to the DNS backends (this
    /// fleet's per-node bind9 targets) over rndc.
    DesignateWorker,
    /// Designate mini-DNS — the AXFR/NOTIFY master the backend bind9s
    /// transfer zones from.
    DesignateMdns,
    /// The bind9 backend that actually answers DNS on this node's overlay
    /// (Q46 — nodes, instances, services resolve mesh-wide). Every node runs
    /// one (no fixed center); the peer-directory-fed pool lists them all.
    DesignateBackendBind9,
}

impl ServiceKind {
    /// Every catalogued service, in the canonical (enum-order) sequence the
    /// mirror rows + reconcile folds iterate.
    pub const ALL: [Self; 37] = [
        Self::Mariadb,
        Self::Rabbitmq,
        Self::Memcached,
        Self::Keystone,
        Self::GlanceApi,
        Self::PlacementApi,
        Self::NovaApi,
        Self::NovaScheduler,
        Self::NovaConductor,
        Self::NovaCompute,
        Self::NeutronServer,
        Self::OvnNbDb,
        Self::OvnSbDb,
        Self::OvnNorthd,
        Self::OvnController,
        Self::CinderApi,
        Self::CinderScheduler,
        Self::CinderVolume,
        Self::CinderBackup,
        Self::SwiftProxyServer,
        Self::SwiftAccountServer,
        Self::SwiftContainerServer,
        Self::SwiftObjectServer,
        Self::HeatApi,
        Self::HeatApiCfn,
        Self::HeatEngine,
        Self::OctaviaApi,
        Self::OctaviaWorker,
        Self::OctaviaHealthManager,
        Self::OctaviaHousekeeping,
        Self::Horizon,
        Self::DesignateApi,
        Self::DesignateCentral,
        Self::DesignateProducer,
        Self::DesignateWorker,
        Self::DesignateMdns,
        Self::DesignateBackendBind9,
    ];

    /// The Kolla-convention container name (underscored) — the `--name` the
    /// worker runs it under, the key reconcile matches `podman ps` rows on,
    /// and the `/etc/kolla/<name>/` config directory stem.
    #[must_use]
    pub const fn container_name(self) -> &'static str {
        match self {
            Self::Mariadb => "mariadb",
            Self::Rabbitmq => "rabbitmq",
            Self::Memcached => "memcached",
            Self::Keystone => "keystone",
            Self::GlanceApi => "glance_api",
            Self::PlacementApi => "placement_api",
            Self::NovaApi => "nova_api",
            Self::NovaScheduler => "nova_scheduler",
            Self::NovaConductor => "nova_conductor",
            Self::NovaCompute => "nova_compute",
            Self::NeutronServer => "neutron_server",
            Self::OvnNbDb => "ovn_nb_db",
            Self::OvnSbDb => "ovn_sb_db",
            Self::OvnNorthd => "ovn_northd",
            Self::OvnController => "ovn_controller",
            Self::CinderApi => "cinder_api",
            Self::CinderScheduler => "cinder_scheduler",
            Self::CinderVolume => "cinder_volume",
            Self::CinderBackup => "cinder_backup",
            Self::SwiftProxyServer => "swift_proxy_server",
            Self::SwiftAccountServer => "swift_account_server",
            Self::SwiftContainerServer => "swift_container_server",
            Self::SwiftObjectServer => "swift_object_server",
            Self::HeatApi => "heat_api",
            Self::HeatApiCfn => "heat_api_cfn",
            Self::HeatEngine => "heat_engine",
            Self::OctaviaApi => "octavia_api",
            Self::OctaviaWorker => "octavia_worker",
            Self::OctaviaHealthManager => "octavia_health_manager",
            Self::OctaviaHousekeeping => "octavia_housekeeping",
            Self::Horizon => "horizon",
            Self::DesignateApi => "designate_api",
            Self::DesignateCentral => "designate_central",
            Self::DesignateProducer => "designate_producer",
            Self::DesignateWorker => "designate_worker",
            Self::DesignateMdns => "designate_mdns",
            Self::DesignateBackendBind9 => "designate_backend_bind9",
        }
    }

    /// The Kolla image basename (dashed) — the tag stem the operator-mirrored
    /// archives carry.
    #[must_use]
    pub const fn image_name(self) -> &'static str {
        match self {
            Self::Mariadb => "mariadb-server",
            Self::Rabbitmq => "rabbitmq",
            Self::Memcached => "memcached",
            Self::Keystone => "keystone",
            Self::GlanceApi => "glance-api",
            Self::PlacementApi => "placement-api",
            Self::NovaApi => "nova-api",
            Self::NovaScheduler => "nova-scheduler",
            Self::NovaConductor => "nova-conductor",
            Self::NovaCompute => "nova-compute",
            Self::NeutronServer => "neutron-server",
            Self::OvnNbDb => "ovn-nb-db",
            Self::OvnSbDb => "ovn-sb-db",
            Self::OvnNorthd => "ovn-northd",
            Self::OvnController => "ovn-controller",
            Self::CinderApi => "cinder-api",
            Self::CinderScheduler => "cinder-scheduler",
            Self::CinderVolume => "cinder-volume",
            Self::CinderBackup => "cinder-backup",
            Self::SwiftProxyServer => "swift-proxy-server",
            Self::SwiftAccountServer => "swift-account-server",
            Self::SwiftContainerServer => "swift-container-server",
            Self::SwiftObjectServer => "swift-object-server",
            Self::HeatApi => "heat-api",
            Self::HeatApiCfn => "heat-api-cfn",
            Self::HeatEngine => "heat-engine",
            Self::OctaviaApi => "octavia-api",
            Self::OctaviaWorker => "octavia-worker",
            Self::OctaviaHealthManager => "octavia-health-manager",
            Self::OctaviaHousekeeping => "octavia-housekeeping",
            Self::Horizon => "horizon",
            Self::DesignateApi => "designate-api",
            Self::DesignateCentral => "designate-central",
            Self::DesignateProducer => "designate-producer",
            Self::DesignateWorker => "designate-worker",
            Self::DesignateMdns => "designate-mdns",
            Self::DesignateBackendBind9 => "designate-backend-bind9",
        }
    }

    /// The full local image reference for the pinned `release`.
    ///
    /// This names the tag the QC-3 Syncthing lane's `podman load` leaves in
    /// the local store (upstream Kolla archives keep their
    /// `quay.io/openstack.kolla/…` tags) — it is only ever used to check
    /// local presence, **never** to pull: no registry is reachable on the
    /// airgapped fleet (design Q18), and the worker gates honestly when the
    /// image is absent.
    #[must_use]
    pub fn image_ref(self, release: &str) -> String {
        format!("quay.io/openstack.kolla/{}:{release}", self.image_name())
    }

    /// The archive filename this service's image travels the mesh as
    /// (QC-3): `<image-basename>-<release>.tar` — `nova-api-2024.1.tar`.
    /// It lives in the share's `kolla/<release>/` directory beside its
    /// `SHA256SUMS` entry; [`super::images`] owns the full lane layout +
    /// verification.
    #[must_use]
    pub fn archive_file_name(self, release: &str) -> String {
        format!("{}-{release}.tar", self.image_name())
    }

    /// Where this service places (Q5/Q15/Q22).
    ///
    /// Leader-hosted (Q15): `MariaDB` and — QC-7 — the OVN control plane (the
    /// northbound/southbound OVSDBs + `ovn-northd`, one central set re-placed on
    /// failover). Everything else — the APIs (Q22), the per-node agents, and the
    /// per-chassis `ovn-controller` — runs on every node.
    #[must_use]
    pub const fn placement(self) -> Placement {
        match self {
            Self::Mariadb | Self::OvnNbDb | Self::OvnSbDb | Self::OvnNorthd => {
                Placement::LeaderOnly
            }
            _ => Placement::EveryNode,
        }
    }

    /// The public API listen port for an API-serving service (Q22/24) — the
    /// port its overlay-bound listener answers on and its service-catalog
    /// endpoint advertises. `None` for the foundation trio (whose ports are
    /// internal wiring, not tenant-facing APIs) and for the Nova/Cinder agents
    /// that carry no listener (scheduler/conductor/compute/volume).
    #[must_use]
    pub const fn api_port(self) -> Option<u16> {
        match self {
            Self::Keystone => Some(5000),
            Self::GlanceApi => Some(9292),
            Self::PlacementApi => Some(8778),
            Self::NovaApi => Some(8774),
            Self::NeutronServer => Some(9696),
            Self::CinderApi => Some(8776),
            Self::SwiftProxyServer => Some(8080),
            // Wave-2 APIs (QC-19). Heat's two endpoints (orchestration + the
            // CFN-compatible callback API, Q61) and Octavia's LB API (Q47). The
            // Octavia agents + Horizon carry no Keystone-catalog endpoint.
            Self::HeatApi => Some(8004),
            Self::HeatApiCfn => Some(8000),
            Self::OctaviaApi => Some(9876),
            // Wave-2 naming (QC-17): the Designate REST API (Q46). The
            // central/producer/worker/mdns agents and the bind9 backend carry
            // no Keystone-catalog endpoint (bind9 answers DNS on 53, not a
            // tenant-facing HTTP API).
            Self::DesignateApi => Some(9001),
            _ => None,
        }
    }

    /// The Nebula-DNS name this API answers on (Q22/46 — Designate/peer-
    /// directory resolution). `Some` exactly when [`Self::api_port`] is: the
    /// service-catalog endpoint resolves over the mesh to whichever node serves
    /// the API, so tenants reach it without pinning a per-node URL.
    #[must_use]
    pub const fn mesh_dns_name(self) -> Option<&'static str> {
        match self {
            Self::Keystone => Some("keystone.mesh"),
            Self::GlanceApi => Some("glance.mesh"),
            Self::PlacementApi => Some("placement.mesh"),
            Self::NovaApi => Some("nova.mesh"),
            Self::NeutronServer => Some("neutron.mesh"),
            Self::CinderApi => Some("cinder.mesh"),
            Self::SwiftProxyServer => Some("swift.mesh"),
            Self::HeatApi => Some("heat.mesh"),
            Self::HeatApiCfn => Some("heat-cfn.mesh"),
            Self::OctaviaApi => Some("octavia.mesh"),
            Self::DesignateApi => Some("designate.mesh"),
            _ => None,
        }
    }

    /// The service-catalog endpoint URL an API advertises over the mesh
    /// (QC-6, Q22/23): plaintext HTTP to the Nebula-DNS name — the overlay IS
    /// the transport security, so tenants reach every API over the mesh with no
    /// TLS. `None` for a non-API service (no tenant-facing endpoint).
    #[must_use]
    pub fn endpoint_url(self) -> Option<String> {
        Some(format!(
            "http://{}:{}",
            self.mesh_dns_name()?,
            self.api_port()?
        ))
    }

    /// Reverse-map a `podman ps` container name to its catalogued service —
    /// `None` for a container the worker does not manage (an operator's
    /// unrelated container is never touched by the reconcile).
    #[must_use]
    pub fn from_container_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|k| k.container_name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_names_round_trip_and_are_unique() {
        for kind in ServiceKind::ALL {
            assert_eq!(
                ServiceKind::from_container_name(kind.container_name()),
                Some(kind),
                "{kind:?} must round-trip its container name"
            );
        }
        let mut names: Vec<&str> = ServiceKind::ALL
            .iter()
            .map(|k| k.container_name())
            .collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ServiceKind::ALL.len(), "no duplicate names");
    }

    #[test]
    fn unmanaged_names_do_not_map() {
        // The reconcile must never adopt an operator's unrelated container.
        for name in [
            "nginx",
            "mcnf-navidrome",
            "",
            "nova-api", /* dashed ≠ container name */
        ] {
            assert_eq!(ServiceKind::from_container_name(name), None, "{name}");
        }
    }

    #[test]
    fn image_refs_are_kolla_convention() {
        assert_eq!(
            ServiceKind::NovaApi.image_ref("2024.1"),
            "quay.io/openstack.kolla/nova-api:2024.1"
        );
        assert_eq!(
            ServiceKind::Mariadb.image_ref("2024.1"),
            "quay.io/openstack.kolla/mariadb-server:2024.1"
        );
        // Container names underscore, image names dash.
        assert_eq!(
            ServiceKind::NeutronServer.container_name(),
            "neutron_server"
        );
        assert_eq!(ServiceKind::NeutronServer.image_name(), "neutron-server");
    }

    #[test]
    fn archive_names_follow_the_qc3_layout_and_are_unique() {
        // QC-3 — `<image-basename>-<release>.tar`, pinned so the operator's
        // mirrored filenames and the worker's expectations can't drift.
        assert_eq!(
            ServiceKind::NovaApi.archive_file_name("2024.1"),
            "nova-api-2024.1.tar"
        );
        assert_eq!(
            ServiceKind::Mariadb.archive_file_name("2024.1"),
            "mariadb-server-2024.1.tar"
        );
        let mut names: Vec<String> = ServiceKind::ALL
            .iter()
            .map(|k| k.archive_file_name("2024.1"))
            .collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ServiceKind::ALL.len(), "no archive collisions");
    }

    #[test]
    fn mariadb_and_the_ovn_control_plane_are_leader_hosted() {
        // Q15 — MariaDB and (QC-7) the OVN control plane (NB/SB OVSDBs + northd)
        // ride the leader; everything else is every-node (Q22: APIs on every
        // node, no controller box — incl. the per-chassis ovn-controller).
        let leader_only: Vec<ServiceKind> = ServiceKind::ALL
            .iter()
            .copied()
            .filter(|k| k.placement() == Placement::LeaderOnly)
            .collect();
        assert_eq!(
            leader_only,
            vec![
                ServiceKind::Mariadb,
                ServiceKind::OvnNbDb,
                ServiceKind::OvnSbDb,
                ServiceKind::OvnNorthd,
            ]
        );
        // The chassis agent is emphatically NOT leader-only — it programs the
        // host OVS on every node.
        assert_eq!(ServiceKind::OvnController.placement(), Placement::EveryNode);
    }

    #[test]
    fn api_services_carry_a_port_and_mesh_endpoint() {
        // QC-6 — every API service advertises a Nebula-DNS service-catalog
        // endpoint over the mesh (plaintext HTTP; the overlay is the TLS).
        assert_eq!(ServiceKind::Keystone.api_port(), Some(5000));
        assert_eq!(
            ServiceKind::Keystone.endpoint_url().as_deref(),
            Some("http://keystone.mesh:5000")
        );
        assert_eq!(
            ServiceKind::GlanceApi.endpoint_url().as_deref(),
            Some("http://glance.mesh:9292")
        );
        assert_eq!(
            ServiceKind::PlacementApi.endpoint_url().as_deref(),
            Some("http://placement.mesh:8778")
        );
        assert_eq!(
            ServiceKind::NovaApi.endpoint_url().as_deref(),
            Some("http://nova.mesh:8774")
        );
        assert_eq!(
            ServiceKind::NeutronServer.endpoint_url().as_deref(),
            Some("http://neutron.mesh:9696")
        );
        assert_eq!(
            ServiceKind::CinderApi.endpoint_url().as_deref(),
            Some("http://cinder.mesh:8776")
        );
        assert_eq!(
            ServiceKind::SwiftProxyServer.endpoint_url().as_deref(),
            Some("http://swift.mesh:8080")
        );
        // Wave-2 (QC-19): Heat's two endpoints + the Octavia LB API advertise
        // over the mesh like every other API.
        assert_eq!(
            ServiceKind::HeatApi.endpoint_url().as_deref(),
            Some("http://heat.mesh:8004")
        );
        assert_eq!(
            ServiceKind::HeatApiCfn.endpoint_url().as_deref(),
            Some("http://heat-cfn.mesh:8000")
        );
        assert_eq!(
            ServiceKind::OctaviaApi.endpoint_url().as_deref(),
            Some("http://octavia.mesh:9876")
        );
        // The foundation trio + the agent services + the OVN control plane carry
        // no tenant-facing API (the OVN DBs speak OVSDB, not a Keystone-catalog
        // HTTP endpoint).
        for kind in [
            ServiceKind::Mariadb,
            ServiceKind::Rabbitmq,
            ServiceKind::Memcached,
            ServiceKind::NovaScheduler,
            ServiceKind::NovaConductor,
            ServiceKind::NovaCompute,
            ServiceKind::OvnNbDb,
            ServiceKind::OvnSbDb,
            ServiceKind::OvnNorthd,
            ServiceKind::OvnController,
            ServiceKind::CinderScheduler,
            ServiceKind::CinderVolume,
            ServiceKind::CinderBackup,
            ServiceKind::SwiftAccountServer,
            ServiceKind::SwiftContainerServer,
            ServiceKind::SwiftObjectServer,
            // Wave-2 (QC-19): Heat's engine + all four Octavia agents + the
            // Horizon dashboard are not Keystone-catalog API endpoints.
            ServiceKind::HeatEngine,
            ServiceKind::OctaviaWorker,
            ServiceKind::OctaviaHealthManager,
            ServiceKind::OctaviaHousekeeping,
            ServiceKind::Horizon,
            // Wave-2 naming (QC-17): the Designate agents + the bind9 backend
            // carry no tenant-facing HTTP endpoint (bind9 serves DNS on 53).
            ServiceKind::DesignateCentral,
            ServiceKind::DesignateProducer,
            ServiceKind::DesignateWorker,
            ServiceKind::DesignateMdns,
            ServiceKind::DesignateBackendBind9,
        ] {
            assert_eq!(kind.api_port(), None, "{kind:?}");
            assert_eq!(kind.mesh_dns_name(), None, "{kind:?}");
            assert_eq!(kind.endpoint_url(), None, "{kind:?}");
        }
        // The port + mesh name + endpoint stay in lockstep across the catalog.
        for kind in ServiceKind::ALL {
            assert_eq!(
                kind.endpoint_url().is_some(),
                kind.api_port().is_some(),
                "{kind:?}"
            );
            assert_eq!(
                kind.mesh_dns_name().is_some(),
                kind.api_port().is_some(),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn cinder_backup_is_an_every_node_agent_with_kolla_names() {
        // QC-8/Q57 — cinder-backup rides every node beside cinder-volume (the
        // node-local LVM volumes it streams to the object tier, Q51/59), never a
        // controller box (Q5/22), and carries no tenant-facing API listener.
        assert_eq!(ServiceKind::CinderBackup.container_name(), "cinder_backup");
        assert_eq!(ServiceKind::CinderBackup.image_name(), "cinder-backup");
        assert_eq!(ServiceKind::CinderBackup.placement(), Placement::EveryNode);
        assert_eq!(ServiceKind::CinderBackup.api_port(), None);
        assert_eq!(ServiceKind::CinderBackup.mesh_dns_name(), None);
        // It reverse-maps from its podman container name like every catalog entry.
        assert_eq!(
            ServiceKind::from_container_name("cinder_backup"),
            Some(ServiceKind::CinderBackup)
        );
    }

    #[test]
    fn swift_is_the_every_node_object_tier_with_one_keystone_api() {
        // QC-18/Q54/55/57 — Swift is the hot object tier: the proxy advertises a
        // Keystone-catalog object-store endpoint on the mesh, while account /
        // container / object servers are every-node ring members with no
        // tenant-facing HTTP endpoint.
        assert_eq!(
            ServiceKind::SwiftProxyServer.container_name(),
            "swift_proxy_server"
        );
        assert_eq!(
            ServiceKind::SwiftProxyServer.image_name(),
            "swift-proxy-server"
        );
        assert_eq!(ServiceKind::SwiftProxyServer.api_port(), Some(8080));
        assert_eq!(
            ServiceKind::SwiftProxyServer.endpoint_url().as_deref(),
            Some("http://swift.mesh:8080")
        );
        for kind in [
            ServiceKind::SwiftProxyServer,
            ServiceKind::SwiftAccountServer,
            ServiceKind::SwiftContainerServer,
            ServiceKind::SwiftObjectServer,
        ] {
            assert_eq!(kind.placement(), Placement::EveryNode, "{kind:?}");
        }
        for (kind, container, image) in [
            (
                ServiceKind::SwiftAccountServer,
                "swift_account_server",
                "swift-account-server",
            ),
            (
                ServiceKind::SwiftContainerServer,
                "swift_container_server",
                "swift-container-server",
            ),
            (
                ServiceKind::SwiftObjectServer,
                "swift_object_server",
                "swift-object-server",
            ),
        ] {
            assert_eq!(kind.container_name(), container);
            assert_eq!(kind.image_name(), image);
            assert_eq!(kind.api_port(), None, "{kind:?}");
            assert_eq!(ServiceKind::from_container_name(container), Some(kind));
        }
    }

    #[test]
    fn designate_is_a_full_every_node_naming_plane() {
        // QC-17/Q46 — Designate replaces DNS/naming: the API advertises the
        // mesh endpoint like every other API; the agents + the per-node bind9
        // backend ride every node (no fixed center — the peer-directory-fed
        // pool lists every node's backend).
        assert_eq!(ServiceKind::DesignateApi.api_port(), Some(9001));
        assert_eq!(
            ServiceKind::DesignateApi.endpoint_url().as_deref(),
            Some("http://designate.mesh:9001")
        );
        for kind in [
            ServiceKind::DesignateApi,
            ServiceKind::DesignateCentral,
            ServiceKind::DesignateProducer,
            ServiceKind::DesignateWorker,
            ServiceKind::DesignateMdns,
            ServiceKind::DesignateBackendBind9,
        ] {
            assert_eq!(kind.placement(), Placement::EveryNode, "{kind:?}");
        }
        // Kolla naming conventions hold (underscored container, dashed image).
        assert_eq!(
            ServiceKind::DesignateBackendBind9.container_name(),
            "designate_backend_bind9"
        );
        assert_eq!(
            ServiceKind::DesignateBackendBind9.image_name(),
            "designate-backend-bind9"
        );
        assert_eq!(
            ServiceKind::from_container_name("designate_mdns"),
            Some(ServiceKind::DesignateMdns)
        );
    }

    #[test]
    fn serde_names_are_snake_case() {
        let json = serde_json::to_string(&ServiceKind::GlanceApi).unwrap();
        assert_eq!(json, "\"glance_api\"");
        let back: ServiceKind = serde_json::from_str("\"cinder_volume\"").unwrap();
        assert_eq!(back, ServiceKind::CinderVolume);
    }
}
