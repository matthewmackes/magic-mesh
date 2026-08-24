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

/// The `node-virt.yml` default dir pool. Guest disks live here.
pub const DEFAULT_POOL_NAME: &str = "default";
/// Host path backing [`DEFAULT_POOL_NAME`].
pub const DEFAULT_POOL_TARGET: &str = "/var/lib/libvirt/images";
/// Accepted existing pool names. Do not define `default` over these.
pub const STORAGE_POOL_CANDIDATES: &[&str] = &["mde-vms", "default", "images"];

/// Result of applying the `node-virt.yml` default-pool recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolPrepare {
    /// `virsh pool-info default` already succeeded.
    AlreadyPresent,
    /// Pool was defined, marked autostart, and started.
    Defined,
}

/// One `virsh` invocation result consumed by [`prepare_default_storage_pool`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolCmd {
    /// Process exit 0.
    pub success: bool,
    /// Combined stdout used to detect an inactive but defined pool.
    pub stdout: String,
    /// Combined stderr used to detect "Storage pool not found".
    pub stderr: String,
}

impl PoolCmd {
    /// True when libvirt reports the default pool is absent.
    #[must_use]
    pub fn not_found(&self) -> bool {
        self.stderr.contains("Storage pool not found")
            || self.stderr.contains("no storage pool")
            || self.stderr.contains("failed to get pool")
    }
}

/// Pure fold of the `node-virt.yml` default-pool recipe over an injectable
/// `virsh` runner. Production wires [`ensure_default_storage_pool`].
pub fn prepare_default_storage_pool<F>(mut run: F) -> Result<PoolPrepare, String>
where
    F: FnMut(&[&str]) -> Result<PoolCmd, String>,
{
    for name in STORAGE_POOL_CANDIDATES {
        let info = run(&["--connect", "qemu:///system", "pool-info", name])?;
        if info.success {
            let _ = run(&["--connect", "qemu:///system", "pool-autostart", name]);
            if info.stdout.contains("State:") && !info.stdout.contains("State:          running") {
                let started = run(&["--connect", "qemu:///system", "pool-start", name])?;
                if !started.success && !started.stderr.contains("already active") {
                    return Err(started.stderr);
                }
            }
            return Ok(PoolPrepare::AlreadyPresent);
        }
        if !info.not_found() {
            return Err(if info.stderr.is_empty() {
                format!("pool-info {name} failed without an authoritative not-found")
            } else {
                info.stderr
            });
        }
    }
    let defined = run(&[
        "--connect",
        "qemu:///system",
        "pool-define-as",
        DEFAULT_POOL_NAME,
        "dir",
        "--target",
        DEFAULT_POOL_TARGET,
    ])?;
    if !defined.success {
        return Err(defined.stderr);
    }
    let autostart = run(&[
        "--connect",
        "qemu:///system",
        "pool-autostart",
        DEFAULT_POOL_NAME,
    ])?;
    if !autostart.success && !autostart.stderr.contains("already") {
        return Err(autostart.stderr);
    }
    let started = run(&[
        "--connect",
        "qemu:///system",
        "pool-start",
        DEFAULT_POOL_NAME,
    ])?;
    if !started.success && !started.stderr.contains("already active") {
        return Err(started.stderr);
    }
    Ok(PoolPrepare::Defined)
}

/// Create `/var/lib/libvirt/images` and apply [`prepare_default_storage_pool`]
/// through live `virsh`. Best-effort: callers log and continue.
pub fn ensure_default_storage_pool() -> Result<PoolPrepare, String> {
    use std::os::unix::fs::PermissionsExt;
    let target = std::path::Path::new(DEFAULT_POOL_TARGET);
    std::fs::create_dir_all(target).map_err(|error| error.to_string())?;
    let mut permissions = std::fs::metadata(target)
        .map_err(|error| error.to_string())?
        .permissions();
    permissions.set_mode(0o711);
    std::fs::set_permissions(target, permissions).map_err(|error| error.to_string())?;
    prepare_default_storage_pool(|args| {
        let output = std::process::Command::new("virsh")
            .args(args)
            .output()
            .map_err(|error| error.to_string())?;
        Ok(PoolCmd {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::{
        find_by_id, prepare_default_storage_pool, PoolCmd, PoolPrepare, DEFAULT_POOL_NAME,
        DEFAULT_POOL_TARGET, KVM_SERVICES,
    };

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

    #[test]
    fn prepare_defines_the_node_virt_default_dir_pool_when_missing() {
        let mut calls = Vec::new();
        let result = prepare_default_storage_pool(|args| {
            calls.push(args.join(" "));
            if args.contains(&"pool-info") {
                Ok(PoolCmd {
                    success: false,
                    stdout: String::new(),
                    stderr: "error: Storage pool not found: no storage pool with matching name 'default'"
                        .into(),
                })
            } else {
                Ok(PoolCmd {
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
        })
        .expect("missing default pool is defined");
        assert_eq!(result, PoolPrepare::Defined);
        assert_eq!(
            calls,
            vec![
                "--connect qemu:///system pool-info mde-vms".into(),
                format!("--connect qemu:///system pool-info {DEFAULT_POOL_NAME}"),
                "--connect qemu:///system pool-info images".into(),
                format!(
                    "--connect qemu:///system pool-define-as {DEFAULT_POOL_NAME} dir --target {DEFAULT_POOL_TARGET}"
                ),
                format!("--connect qemu:///system pool-autostart {DEFAULT_POOL_NAME}"),
                format!("--connect qemu:///system pool-start {DEFAULT_POOL_NAME}"),
            ]
        );
    }

    #[test]
    fn prepare_leaves_an_existing_default_pool_in_place() {
        let result = prepare_default_storage_pool(|args| {
            assert!(args.contains(&"pool-info") || args.contains(&"pool-autostart"));
            Ok(PoolCmd {
                success: true,
                stdout: "Name:           mde-vms\nState:          running\n".into(),
                stderr: String::new(),
            })
        })
        .expect("existing pool is reused");
        assert_eq!(result, PoolPrepare::AlreadyPresent);
    }

    #[test]
    fn prepare_refuses_ambiguous_pool_info_failure() {
        let error = prepare_default_storage_pool(|_| {
            Ok(PoolCmd {
                success: false,
                stdout: String::new(),
                stderr: "error: authentication unavailable".into(),
            })
        })
        .expect_err("polkit failure is not a missing pool");
        assert!(error.contains("authentication unavailable"));
    }
}
