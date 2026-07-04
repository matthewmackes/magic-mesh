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
//! config, QC-6 binds each API entry to the Nebula interface, and the wave-2
//! services (Q25 — Designate, Octavia, Heat, Horizon) land as new variants.

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
    /// LVM (Q51), memcached (Q17), a `RabbitMQ` cluster member (Q16)).
    EveryNode,
    /// Rides the etcd leader and re-places on failover (Q15 — `MariaDB` is a
    /// workload on the leader, never a permanently-special node).
    LeaderOnly,
}

/// One Kolla-packaged `OpenStack` service the `openstack` worker supervises.
///
/// The variant set is the QC-2 skeleton catalog: the QC-4 foundation trio +
/// the QC-5/6 identity + core-API set. Names follow the Kolla conventions the
/// mirrored archives carry: container names use underscores (`nova_api`),
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
    /// Block-storage API (Q51).
    CinderApi,
    /// Block-storage scheduler.
    CinderScheduler,
    /// Node-local LVM volume service (Q51/59).
    CinderVolume,
}

impl ServiceKind {
    /// Every catalogued service, in the canonical (enum-order) sequence the
    /// mirror rows + reconcile folds iterate.
    pub const ALL: [Self; 14] = [
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
        Self::CinderApi,
        Self::CinderScheduler,
        Self::CinderVolume,
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
            Self::CinderApi => "cinder_api",
            Self::CinderScheduler => "cinder_scheduler",
            Self::CinderVolume => "cinder_volume",
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
            Self::CinderApi => "cinder-api",
            Self::CinderScheduler => "cinder-scheduler",
            Self::CinderVolume => "cinder-volume",
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
    #[must_use]
    pub const fn placement(self) -> Placement {
        match self {
            Self::Mariadb => Placement::LeaderOnly,
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
    fn only_mariadb_is_leader_hosted() {
        // Q15 — MariaDB rides the leader; everything else is every-node (Q22:
        // APIs on every node, no controller box).
        let leader_only: Vec<ServiceKind> = ServiceKind::ALL
            .iter()
            .copied()
            .filter(|k| k.placement() == Placement::LeaderOnly)
            .collect();
        assert_eq!(leader_only, vec![ServiceKind::Mariadb]);
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
        // The foundation trio + the agent services carry no tenant-facing API.
        for kind in [
            ServiceKind::Mariadb,
            ServiceKind::Rabbitmq,
            ServiceKind::Memcached,
            ServiceKind::NovaScheduler,
            ServiceKind::NovaConductor,
            ServiceKind::NovaCompute,
            ServiceKind::CinderScheduler,
            ServiceKind::CinderVolume,
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
    fn serde_names_are_snake_case() {
        let json = serde_json::to_string(&ServiceKind::GlanceApi).unwrap();
        assert_eq!(json, "\"glance_api\"");
        let back: ServiceKind = serde_json::from_str("\"cinder_volume\"").unwrap();
        assert_eq!(back, ServiceKind::CinderVolume);
    }
}
