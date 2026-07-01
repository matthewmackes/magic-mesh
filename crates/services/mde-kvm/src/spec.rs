//! The VM spec + the dual-homed NIC model (E12-7, Round-2 locks 11/18/19).
//!
//! [`VmSpec`] is the operator-facing description of a local cloud-hypervisor VM;
//! it is the input to the pure [`crate::config::build_ch_config`] mapping. The
//! [`Nic`] type models the **dual-homing** lock (19): every guest is its own
//! Nebula mesh peer ([`NicRole::Mesh`]) *and* carries a LAN-bridged virtual NIC
//! ([`NicRole::Lan`]). On the cloud-hypervisor side both are simply host taps —
//! the *role* is what tells the broker which host bridge to enslave each tap to.

use std::path::{Path, PathBuf};

use crate::vfio::VfioDevice;

/// The conventional runtime directory for mde-kvm's per-VM unix sockets (the CH
/// api-socket and, when virtio-gpu is enabled, the vhost-user-gpu socket). One
/// sub-directory per VM keeps a fan-out of local VMs from colliding.
pub const RUNTIME_DIR: &str = "/run/mde-kvm";

/// The default cloud-hypervisor guest firmware. A **mesh golden image** (lock 14)
/// is a full bootable disk — Windows 10 included (lock 15) — so cloud-hypervisor
/// boots it through UEFI firmware (the EDK2 `CLOUDHV.fd` build), not a direct
/// kernel payload. The bootc Workstation image (lock 42) ships this firmware; a
/// spec may override it via [`VmSpec::firmware`].
pub const DEFAULT_FIRMWARE: &str = "/usr/share/cloud-hypervisor/CLOUDHV.fd";

/// Which host-side network a guest NIC attaches to — the dual-homing roles
/// (lock 19). cloud-hypervisor sees both as a `tap`; the role tells the broker
/// which host bridge the tap is enslaved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NicRole {
    /// The guest's **own Nebula mesh-peer** interface — its tap is bridged to the
    /// host's mesh/overlay side, so the guest enrolls as a first-class mesh member
    /// with its own cert (lock 19/47). Conventionally the guest's first NIC.
    Mesh,
    /// A **LAN-bridged** virtual NIC — its tap is enslaved to the host's physical
    /// LAN bridge, giving the guest an address on the operator's local network.
    Lan,
}

impl NicRole {
    /// A short stable tag (`"mesh"` / `"lan"`) — handy for tap-name conventions
    /// and operator-facing summaries.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Mesh => "mesh",
            Self::Lan => "lan",
        }
    }
}

/// One guest network interface. `tap` is the host-side tap device cloud-hypervisor
/// attaches to; `mac`/`mtu` are optional (cloud-hypervisor auto-generates a MAC
/// when `None`, and the mesh side typically pins the Nebula overlay MTU).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nic {
    /// The dual-homing role (mesh peer vs LAN bridge).
    pub role: NicRole,
    /// Host-side tap device name cloud-hypervisor binds (e.g. `mvm-web1-mesh`).
    pub tap: String,
    /// Guest MAC address (`aa:bb:cc:dd:ee:ff`); `None` ⇒ cloud-hypervisor auto-gen.
    pub mac: Option<String>,
    /// MTU; `None` ⇒ cloud-hypervisor default (1500). The mesh NIC usually pins
    /// the Nebula overlay MTU here.
    pub mtu: Option<u16>,
}

impl Nic {
    /// A mesh-peer NIC on host tap `tap` (the guest's own Nebula interface).
    #[must_use]
    pub fn mesh(tap: impl Into<String>) -> Self {
        Self {
            role: NicRole::Mesh,
            tap: tap.into(),
            mac: None,
            mtu: None,
        }
    }

    /// A LAN-bridged NIC on host tap `tap`.
    #[must_use]
    pub fn lan(tap: impl Into<String>) -> Self {
        Self {
            role: NicRole::Lan,
            tap: tap.into(),
            mac: None,
            mtu: None,
        }
    }

    /// Pin the guest MAC (builder style).
    #[must_use]
    pub fn with_mac(mut self, mac: impl Into<String>) -> Self {
        self.mac = Some(mac.into());
        self
    }

    /// Pin the MTU (builder style) — e.g. the Nebula overlay MTU on the mesh NIC.
    #[must_use]
    pub fn with_mtu(mut self, mtu: u16) -> Self {
        self.mtu = Some(mtu);
        self
    }
}

/// The conventional in-guest mount tag for the **mesh-share** folder — the common
/// case ([`SharedFolder::mesh_share`]): a Syncthing-replicated mesh directory the
/// guest mounts read-write (`mount -t virtiofs mesh-share /mnt/mesh-share`). Distinct
/// from the XCP server path's `mesh-storage` tag; this is the E12 desktop bridge.
pub const MESH_SHARE_TAG: &str = "mesh-share";

/// A host directory exported into the guest over **virtio-fs** (E12-9, the mesh-share
/// bridge). Mirrors [`Nic`]: a plain typed field on [`VmSpec`] that
/// [`crate::config::build_ch_config`] folds into the cloud-hypervisor config. A VM
/// can carry zero or more shared folders.
///
/// The host side of each folder is a **virtiofsd** process exporting `host_path` on a
/// per-folder unix socket ([`virtiofs_socket_path`]); cloud-hypervisor's `fs` device
/// connects to that socket and the guest mounts it under `tag`. `read_only` is a
/// *host-side* property (it drives virtiofsd's `--readonly`), so — exactly like a
/// NIC's [`NicRole`] — it does **not** appear in the CH `fs` entry, only in the live
/// launch behind [`VirtiofsLauncher`](crate::VirtiofsLauncher).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SharedFolder {
    /// The in-guest virtio-fs mount tag (e.g. [`MESH_SHARE_TAG`]) — what the guest
    /// passes to `mount -t virtiofs <tag> <mountpoint>`. Also names the per-folder
    /// virtiofsd socket ([`virtiofs_socket_path`]).
    pub tag: String,
    /// The host directory virtiofsd exports — for the mesh-share this is the
    /// Syncthing-replicated mesh dir, so a file dropped in it appears in the guest.
    pub host_path: PathBuf,
    /// Export read-only. `false` ⇒ the guest can write back (the mesh-share default,
    /// so guest edits replicate back out); `true` ⇒ virtiofsd exports `--readonly`.
    pub read_only: bool,
}

impl SharedFolder {
    /// A shared folder exporting `host_path` under guest mount `tag`, read-write by
    /// default (flip with [`SharedFolder::with_read_only`]).
    #[must_use]
    pub fn new(tag: impl Into<String>, host_path: impl Into<PathBuf>) -> Self {
        Self {
            tag: tag.into(),
            host_path: host_path.into(),
            read_only: false,
        }
    }

    /// The common case: the **mesh-share** folder — a Syncthing-replicated mesh
    /// directory `host_path` mounted read-write under [`MESH_SHARE_TAG`], so a file
    /// dropped in the mesh-share appears inside the guest (and guest writes replicate
    /// back out over the mesh).
    #[must_use]
    pub fn mesh_share(host_path: impl Into<PathBuf>) -> Self {
        Self::new(MESH_SHARE_TAG, host_path)
    }

    /// Export read-only (builder style) — the guest sees the folder but cannot write
    /// back into the mesh dir.
    #[must_use]
    pub fn with_read_only(mut self, yes: bool) -> Self {
        self.read_only = yes;
        self
    }
}

/// A local cloud-hypervisor VM spec (lock 11). The exact, minimal description the
/// shell/`vm-lifecycle` worker hands to the broker; [`crate::config::build_ch_config`]
/// turns it into cloud-hypervisor's `VmConfig` JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmSpec {
    /// Stable VM name — drives the per-VM socket paths ([`api_socket_path`] /
    /// [`gpu_socket_path`]) and the running-disk name.
    pub name: String,
    /// Boot vCPU count (also the max — these local desktops are fixed-sized, lock 17).
    pub vcpus: u8,
    /// Guest RAM in MiB (the broker converts to bytes for cloud-hypervisor).
    pub mem_mib: u64,
    /// The disk cloud-hypervisor opens read-write. Per lock 18 this is the VM's
    /// **running disk in `~/Local`** (never mesh-synced); the broker derives it
    /// from a mesh golden base via [`running_disk_path`] before boot.
    pub disk: PathBuf,
    /// UEFI firmware override; `None` ⇒ [`DEFAULT_FIRMWARE`].
    pub firmware: Option<PathBuf>,
    /// Enable the virtio-gpu zero-copy fast path (lock 12). When set, the config
    /// gains a vhost-user-gpu device pointed at [`gpu_socket_path`], where the
    /// shell's GPU backend exports a dmabuf into a wgpu texture.
    pub virtio_gpu: bool,
    /// The guest's NICs. For a dual-homed desktop (lock 19) this is one
    /// [`NicRole::Mesh`] + one [`NicRole::Lan`].
    pub nics: Vec<Nic>,
    /// The guest's **virtio-fs** shared folders (E12-9, the mesh-share bridge). Zero
    /// or more host directories exported into the guest; the mesh-share folder is the
    /// common case ([`SharedFolder::mesh_share`]). Each becomes a cloud-hypervisor
    /// `fs` device backed by a virtiofsd socket.
    pub shared_folders: Vec<SharedFolder>,
    /// Host PCI devices passed through into the guest via **VFIO** (E12-10,
    /// lock 13 — e.g. the dGPU). Each becomes a cloud-hypervisor `devices`
    /// entry; refused at create/preflight unless [`VmSpec::vfio_allowed`].
    pub vfio_devices: Vec<VfioDevice>,
    /// The explicit **operator opt-in** for VFIO passthrough on this host
    /// (lock 13: dGPU passthrough is an operator choice per host). Passthrough
    /// hands the guest raw DMA-capable hardware, so a spec carrying
    /// [`VmSpec::vfio_devices`] without this flag is refused with a typed
    /// [`VfioError::NotOptedIn`](crate::VfioError::NotOptedIn) — never
    /// silently honored or dropped.
    pub vfio_allowed: bool,
}

impl VmSpec {
    /// A spec with the required fields; `firmware` defaults, `virtio_gpu` off, no
    /// NICs. Add NICs/firmware/GPU with the builder methods.
    #[must_use]
    pub fn new(name: impl Into<String>, vcpus: u8, mem_mib: u64, disk: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            vcpus,
            mem_mib,
            disk: disk.into(),
            firmware: None,
            virtio_gpu: false,
            nics: Vec::new(),
            shared_folders: Vec::new(),
            vfio_devices: Vec::new(),
            vfio_allowed: false,
        }
    }

    /// Enable the virtio-gpu fast path (builder style).
    #[must_use]
    pub fn with_virtio_gpu(mut self, on: bool) -> Self {
        self.virtio_gpu = on;
        self
    }

    /// Override the UEFI firmware (builder style).
    #[must_use]
    pub fn with_firmware(mut self, firmware: impl Into<PathBuf>) -> Self {
        self.firmware = Some(firmware.into());
        self
    }

    /// Attach a NIC (builder style). Order is preserved — put the mesh NIC first
    /// so it becomes the guest's primary (eth0) interface.
    #[must_use]
    pub fn with_nic(mut self, nic: Nic) -> Self {
        self.nics.push(nic);
        self
    }

    /// Attach a virtio-fs shared folder (builder style). Order is preserved; a VM
    /// can carry several (e.g. the mesh-share plus a read-only reference export).
    #[must_use]
    pub fn with_shared_folder(mut self, folder: SharedFolder) -> Self {
        self.shared_folders.push(folder);
        self
    }

    /// Attach a VFIO passthrough device (builder style). Order is preserved.
    /// Remember the opt-in: without [`VmSpec::allow_vfio`] the spec is refused
    /// at create/preflight with a typed error.
    #[must_use]
    pub fn with_vfio_device(mut self, device: VfioDevice) -> Self {
        self.vfio_devices.push(device);
        self
    }

    /// Record the operator's per-host VFIO opt-in (builder style, lock 13).
    /// The shell/`vm-lifecycle` worker sets this from host policy — it is
    /// never defaulted on.
    #[must_use]
    pub const fn allow_vfio(mut self, allowed: bool) -> Self {
        self.vfio_allowed = allowed;
        self
    }

    /// The firmware this spec boots (its override, or [`DEFAULT_FIRMWARE`]).
    #[must_use]
    pub fn firmware_path(&self) -> PathBuf {
        self.firmware
            .clone()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_FIRMWARE))
    }
}

/// The cloud-hypervisor api-socket path for a VM named `name`
/// (`/run/mde-kvm/<name>/api.sock`). cloud-hypervisor is launched with
/// `--api-socket <this>`, and [`crate::Vm::connect`] dials it.
#[must_use]
pub fn api_socket_path(name: &str) -> PathBuf {
    PathBuf::from(RUNTIME_DIR).join(name).join("api.sock")
}

/// The vhost-user-gpu socket path for a VM named `name`
/// (`/run/mde-kvm/<name>/gpu.sock`). The shell's GPU backend listens here; the
/// `gpu` device in the [`crate::config::build_ch_config`] output points at it.
#[must_use]
pub fn gpu_socket_path(name: &str) -> PathBuf {
    PathBuf::from(RUNTIME_DIR).join(name).join("gpu.sock")
}

/// The virtiofsd unix-socket path for VM `name`'s shared folder tagged `tag`
/// (`/run/mde-kvm/<name>/fs-<tag>.sock`). One socket per folder (a VM can export
/// several), so the two ends — the [`VirtiofsLauncher`](crate::VirtiofsLauncher) that
/// binds virtiofsd here and the `fs` device [`crate::config::build_ch_config`] points
/// at it — agree by both deriving the path from this one pure function, never
/// colliding across a fan-out of folders.
#[must_use]
pub fn virtiofs_socket_path(name: &str, tag: &str) -> PathBuf {
    PathBuf::from(RUNTIME_DIR)
        .join(name)
        .join(format!("fs-{tag}.sock"))
}

/// The running-disk path under a `~/Local` directory for a VM named `name`
/// (`<local_dir>/<name>.img`). Per lock 18 the broker copies a mesh golden base
/// to this local, never-synced path and points [`VmSpec::disk`] at it before boot.
#[must_use]
pub fn running_disk_path(local_dir: &Path, name: &str) -> PathBuf {
    local_dir.join(format!("{name}.img"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_share_is_the_read_write_syncthing_dir_under_the_mesh_share_tag() {
        let sf = SharedFolder::mesh_share("/home/op/Mesh/Share");
        assert_eq!(sf.tag, MESH_SHARE_TAG);
        assert_eq!(sf.host_path, PathBuf::from("/home/op/Mesh/Share"));
        // the mesh-share defaults read-write so guest edits replicate back out.
        assert!(!sf.read_only);
    }

    #[test]
    fn new_defaults_read_write_and_with_read_only_flips_it() {
        let rw = SharedFolder::new("docs", "/srv/docs");
        assert_eq!(rw.tag, "docs");
        assert!(!rw.read_only);
        let ro = SharedFolder::new("ref", "/srv/ref").with_read_only(true);
        assert!(ro.read_only);
    }

    #[test]
    fn a_spec_carries_zero_or_more_shared_folders_in_order() {
        // zero by default…
        let plain = VmSpec::new("plain", 1, 512, "/d.img");
        assert!(plain.shared_folders.is_empty());
        // …and the builder preserves attach order.
        let spec = VmSpec::new("web1", 2, 2048, "/web1.img")
            .with_shared_folder(SharedFolder::mesh_share("/home/op/Mesh/Share"))
            .with_shared_folder(SharedFolder::new("ref", "/srv/ref").with_read_only(true));
        assert_eq!(spec.shared_folders.len(), 2);
        assert_eq!(spec.shared_folders[0].tag, MESH_SHARE_TAG);
        assert_eq!(spec.shared_folders[1].tag, "ref");
    }

    #[test]
    fn virtiofs_socket_is_per_vm_and_per_tag() {
        assert_eq!(
            virtiofs_socket_path("web1", MESH_SHARE_TAG),
            PathBuf::from("/run/mde-kvm/web1/fs-mesh-share.sock")
        );
        // a second folder on the same VM gets its own socket (no collision).
        assert_ne!(
            virtiofs_socket_path("web1", "mesh-share"),
            virtiofs_socket_path("web1", "ref")
        );
    }

    #[test]
    fn vfio_defaults_off_and_builders_preserve_order_and_opt_in() {
        use crate::vfio::{PciAddress, VfioDevice};
        // passthrough is entirely opt-in: empty + not allowed by default.
        let plain = VmSpec::new("plain", 1, 512, "/d.img");
        assert!(plain.vfio_devices.is_empty());
        assert!(!plain.vfio_allowed);
        // builders attach devices in order and record the operator opt-in.
        let gpu = PciAddress::parse("0000:01:00.0").expect("gpu addr");
        let audio = PciAddress::parse("0000:01:00.1").expect("audio addr");
        let spec = VmSpec::new("gpu1", 4, 8192, "/g.img")
            .with_vfio_device(VfioDevice::new(gpu.clone()).with_iommu_group(14))
            .with_vfio_device(VfioDevice::new(audio.clone()))
            .allow_vfio(true);
        assert_eq!(spec.vfio_devices.len(), 2);
        assert_eq!(spec.vfio_devices[0].address, gpu);
        assert_eq!(spec.vfio_devices[0].iommu_group, Some(14));
        assert_eq!(spec.vfio_devices[1].address, audio);
        assert_eq!(spec.vfio_devices[1].iommu_group, None);
        assert!(spec.vfio_allowed);
    }

    #[test]
    fn shared_folder_serde_round_trips() {
        let sf = SharedFolder::mesh_share("/home/op/Mesh/Share").with_read_only(true);
        let json = serde_json::to_string(&sf).expect("serialize");
        let back: SharedFolder = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(sf, back);
    }
}
