//! MV-1 — the per-node **KVM virtualization service catalog**: the Fedora + KVM
//! replacement for the xcp-ng toolstack catalog the dead `Xcpng` role carried.
//!
//! Every mesh node — Lighthouse OR Workstation — runs one identical KVM
//! virtualization stack (`docs/design/mesh-virt-management.md`: "same stack on
//! every machine; role is configuration"). The provisioning recipe lives in
//! `infra/ansible/node-virt.yml`; this module is the single source of the
//! *catalog* it stands up — each service, its systemd unit, and what it does —
//! so the host-health worker ([`crate::workers::kvm_health`]), the Datacenter
//! panels, and any future provisioning all agree on what a KVM host provides.
//!
//! Deliberately small: KVM lives in the kernel, and the mesh (Nebula = stunnel,
//! the overlay routing around a dead node = xha) + systemd/D-Bus (=
//! message-switch/forkexecd) + virtio-balloon (= squeezed) cover most of what
//! the 16-daemon xcp-ng toolstack needed — so only a handful of services are
//! load-bearing (~4 packages added per the design). The whole-host health fold
//! + the `event/kvm/services` publish is MV-2; this is the pure data it folds.
//!
//! Mirrors the *shape* of the old `mde_role::xcpng` module, but lives in
//! `mackesd` (the universal core that owns the management layer), not `mde-role`.

/// One service in the per-node KVM virtualization stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KvmService {
    /// Short canonical id — the key under which the service is reported in host
    /// health (e.g. `"libvirtd"`, `"libvirt-network"`).
    pub id: &'static str,
    /// The primary systemd unit whose liveness backs it
    /// (`systemctl is-active <unit>`).
    ///
    /// The default libvirt network and storage pool have **no own systemd unit**
    /// under the monolithic `libvirtd` that `node-virt.yml` enables — they are
    /// served in-process by libvirtd — so they carry `libvirtd.service` as their
    /// primary backing unit (its liveness *is* their availability). Fedora's
    /// modular libvirt layout is exposed by [`Self::alternative_units`] and is
    /// probed after this compatibility unit.
    pub unit: &'static str,
    /// One-line role in the stack.
    pub summary: &'static str,
}

impl KvmService {
    const fn new(id: &'static str, unit: &'static str, summary: &'static str) -> Self {
        Self { id, unit, summary }
    }

    /// Additional systemd units that can provide this canonical service.
    ///
    /// The published service ids intentionally remain the stable, legacy
    /// libvirt ids. Keeping the alternatives here (next to the canonical
    /// catalog) lets both the older monolithic `libvirtd` layout and Fedora's
    /// modular `virt*` daemons feed the same health rows without changing the
    /// consumer contract.
    #[must_use]
    pub fn alternative_units(&self) -> &'static [&'static str] {
        match self.id {
            "libvirtd" => &["virtqemud.service", "virtqemud.socket"],
            "libvirt-network" => &["virtnetworkd.service", "virtnetworkd.socket"],
            "libvirt-storage" => &["virtstoraged.service", "virtstoraged.socket"],
            _ => &[],
        }
    }

    /// Candidate units in probe order: the legacy-compatible primary unit,
    /// followed by any Fedora modular alternatives.
    #[must_use]
    pub fn probe_units(&self) -> impl Iterator<Item = &'static str> + '_ {
        std::iter::once(self.unit).chain(self.alternative_units().iter().copied())
    }
}

/// The per-node KVM virtualization service set every mesh node provisions
/// (`infra/ansible/node-virt.yml`) — the Fedora + KVM replacement for the
/// xcp-ng toolstack. Ordered management-brain-first: `libvirtd` (the lifecycle +
/// network + storage service, backed by legacy `libvirtd.service` or modular
/// `virtqemud`/`virtnetworkd`/`virtstoraged`) leads, then the container socket,
/// host networking, and the libvirt-served default network + storage pool.
/// Cockpit's interim VM console is retired by the CONSTRUCT-CLOUD/QC-15 cutover
/// and is deliberately not part of the live catalog.
pub static KVM_SERVICES: &[KvmService] = &[
    KvmService::new(
        "libvirtd",
        "libvirtd.service",
        "The libvirt virtualization daemon — KVM/QEMU VM lifecycle plus the \
         in-process network and storage drivers (xapi + xenopsd + sm + \
         xcp-networkd folded into one daemon).",
    ),
    KvmService::new(
        "podman",
        "podman.socket",
        "The Podman API socket — the OCI-container side of the compute plane \
         xcp-ng never had (socket-activated).",
    ),
    KvmService::new(
        "network-manager",
        "NetworkManager.service",
        "Host network configuration — the physical links, bridges, and routes \
         the Nebula overlay and the guest bridges ride on (xcp-networkd's host \
         half).",
    ),
    KvmService::new(
        "libvirt-network",
        "libvirtd.service",
        "The default libvirt NAT network (virbr0) guests get DHCP from — \
         autostarted and served in-process by libvirtd.",
    ),
    KvmService::new(
        "libvirt-storage",
        "libvirtd.service",
        "The default dir storage pool VM disks live in (the sm/SR equivalent) — \
         autostarted and served in-process by libvirtd.",
    ),
];

/// Look up a catalog entry by its canonical [`KvmService::id`]. Pure helper over
/// the static [`KVM_SERVICES`] catalog — no probe, no host state.
#[must_use]
pub fn find_by_id(id: &str) -> Option<&'static KvmService> {
    KVM_SERVICES.iter().find(|s| s.id == id)
}

#[cfg(test)]
mod tests {
    use super::{find_by_id, KVM_SERVICES};

    #[test]
    fn catalog_lists_the_node_virt_service_set() {
        // The catalog must mirror infra/ansible/node-virt.yml — every
        // load-bearing KVM service is present and uniquely identified.
        assert!(
            KVM_SERVICES.len() >= 4,
            "the KVM catalog looks short ({} services)",
            KVM_SERVICES.len()
        );
        for must in [
            "libvirtd",
            "podman",
            "network-manager",
            "libvirt-network",
            "libvirt-storage",
        ] {
            assert!(
                KVM_SERVICES.iter().any(|s| s.id == must),
                "the KVM catalog is missing `{must}`"
            );
        }
        // Every entry is fully populated and the ids are unique. Units are NOT
        // asserted unique: under the monolithic libvirtd node-virt.yml enables,
        // the default network and storage pool legitimately share
        // `libvirtd.service` as their primary backing unit.
        for (i, a) in KVM_SERVICES.iter().enumerate() {
            assert!(!a.id.is_empty() && !a.unit.is_empty() && !a.summary.is_empty());
            for b in &KVM_SERVICES[i + 1..] {
                assert_ne!(a.id, b.id, "duplicate service id `{}`", a.id);
            }
        }
    }

    #[test]
    fn libvirt_network_and_storage_back_onto_libvirtd() {
        // The two libvirt-served items carry libvirtd's unit — their
        // availability IS libvirtd's liveness on a monolithic-libvirtd host;
        // Fedora's modular units are alternatives, not new service ids.
        for id in ["libvirt-network", "libvirt-storage"] {
            let service = find_by_id(id).expect("present in catalog");
            assert_eq!(service.unit, "libvirtd.service");
        }
    }

    #[test]
    fn modular_units_are_alternatives_for_stable_service_ids() {
        assert_eq!(
            find_by_id("libvirtd")
                .expect("present in catalog")
                .alternative_units(),
            &["virtqemud.service", "virtqemud.socket"]
        );
        assert_eq!(
            find_by_id("libvirt-network")
                .expect("present in catalog")
                .alternative_units(),
            &["virtnetworkd.service", "virtnetworkd.socket"]
        );
        assert_eq!(
            find_by_id("libvirt-storage")
                .expect("present in catalog")
                .alternative_units(),
            &["virtstoraged.service", "virtstoraged.socket"]
        );
        for id in ["libvirtd", "libvirt-network", "libvirt-storage"] {
            let service = find_by_id(id).expect("present in catalog");
            assert_eq!(service.probe_units().next(), Some("libvirtd.service"));
        }
    }

    #[test]
    fn find_by_id_round_trips_and_misses_cleanly() {
        let s = find_by_id("podman").expect("podman is in the catalog");
        assert_eq!(s.unit, "podman.socket");
        assert!(find_by_id("not-a-real-service").is_none());
    }
}
