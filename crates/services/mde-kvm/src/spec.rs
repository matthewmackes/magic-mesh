//! The VM spec + the dual-homed NIC model (E12-7, Round-2 locks 11/18/19).
//!
//! [`VmSpec`] is the operator-facing description of a local cloud-hypervisor VM;
//! it is the input to the pure [`crate::config::build_ch_config`] mapping. The
//! [`Nic`] type models the **dual-homing** lock (19): every guest is its own
//! Nebula mesh peer ([`NicRole::Mesh`]) *and* carries a LAN-bridged virtual NIC
//! ([`NicRole::Lan`]). On the cloud-hypervisor side both are simply host taps —
//! the *role* is what tells the broker which host bridge to enslave each tap to.

use std::path::{Path, PathBuf};

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

/// The running-disk path under a `~/Local` directory for a VM named `name`
/// (`<local_dir>/<name>.img`). Per lock 18 the broker copies a mesh golden base
/// to this local, never-synced path and points [`VmSpec::disk`] at it before boot.
#[must_use]
pub fn running_disk_path(local_dir: &Path, name: &str) -> PathBuf {
    local_dir.join(format!("{name}.img"))
}
