//! VFIO GPU/device passthrough (E12-10, lock 13 — dGPU passthrough is an
//! **operator choice per host**).
//!
//! cloud-hypervisor passes host PCI devices straight into a guest via **VFIO**:
//! each [`VfioDevice`] on a [`VmSpec`] becomes one entry in the `VmConfig`
//! `devices` array (its `OpenAPI` `DeviceConfig`: `path` = the device's sysfs
//! path), emitted by [`build_ch_config`](crate::build_ch_config). Passthrough
//! hands the guest raw DMA-capable hardware, so it is **refused unless the
//! operator explicitly opted the host in** ([`VmSpec::vfio_allowed`]) — both
//! [`ensure_vfio_opt_in`] (pure, enforced by [`Vm::create`](crate::Vm::create))
//! and [`preflight_vfio`] return a typed [`VfioError::NotOptedIn`] otherwise.
//!
//! Before a passthrough VM can boot, the *host* must be ready: the IOMMU on,
//! the device bound to `vfio-pci`, and the device's whole IOMMU group viable
//! (cloud-hypervisor's vfio doc: devices sharing a group — e.g. a dGPU and its
//! embedded audio function — must **all** be bound to VFIO and passed through,
//! "otherwise this could cause some functional and security issues").
//! [`preflight_vfio`] checks exactly that, through the injectable [`VfioProbe`]
//! seam — the same pattern as [`ChTransport`](crate::ChTransport) /
//! [`VirtiofsLauncher`](crate::VirtiofsLauncher): the pure checks are
//! unit-tested against a fake probe, while the production [`SysfsVfioProbe`]
//! reads the real sysfs surface (`/sys/kernel/iommu_groups`,
//! `/sys/bus/pci/devices/...`). The full live preflight is **gated on real
//! IOMMU hardware** (none on the build farm); the sysfs reader itself is
//! unit-tested against a scratch sysfs tree via [`SysfsVfioProbe::with_root`].

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use thiserror::Error;

use crate::spec::VmSpec;

/// The driver name a passthrough device must be bound to on the host.
pub const VFIO_PCI_DRIVER: &str = "vfio-pci";

/// A VFIO passthrough failure — typed, so the shell/`vm-lifecycle` worker can
/// tell the operator exactly which host prerequisite is missing.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VfioError {
    /// The string is not a full PCI address (`DDDD:BB:DD.F`, hex).
    #[error("invalid PCI address '{0}' (expected DDDD:BB:DD.F, e.g. 0000:01:00.0)")]
    InvalidPciAddress(String),
    /// The spec carries passthrough devices but the operator has not opted the
    /// host in ([`VmSpec::vfio_allowed`]). Passthrough hands the guest raw
    /// DMA-capable hardware, so it is opt-in per host (lock 13) — refused with
    /// this typed error, never silently dropped.
    #[error(
        "VM '{vm}' requests {devices} VFIO passthrough device(s) but the host is not \
         opted in — GPU/device passthrough is an explicit operator choice per host \
         (lock 13); set VmSpec::allow_vfio(true) from the host policy first"
    )]
    NotOptedIn {
        /// The VM whose spec requested passthrough.
        vm: String,
        /// How many passthrough devices the spec carries.
        devices: usize,
    },
    /// The host IOMMU is off (`/sys/kernel/iommu_groups` is empty/absent) —
    /// enable it in firmware + kernel cmdline (`intel_iommu=on` / `amd_iommu=on`).
    #[error(
        "host IOMMU is not active (no /sys/kernel/iommu_groups entries) — enable \
         VT-d/AMD-Vi in firmware and boot with intel_iommu=on / amd_iommu=on"
    )]
    IommuDisabled,
    /// The PCI device does not exist on this host.
    #[error("PCI device {address} not present on this host (no sysfs node)")]
    DeviceNotFound {
        /// The missing device.
        address: PciAddress,
    },
    /// The device is not bound to `vfio-pci` (still on its native driver, or
    /// unbound). `driver` is the currently-bound driver, or `"(unbound)"`.
    #[error(
        "PCI device {address} is bound to {driver}, not {VFIO_PCI_DRIVER} — unbind it \
         and bind {VFIO_PCI_DRIVER} before passthrough"
    )]
    NotBoundToVfioPci {
        /// The device that fails the check.
        address: PciAddress,
        /// The driver it is actually bound to (`"(unbound)"` when none).
        driver: String,
    },
    /// The device has no IOMMU group (the IOMMU does not cover it).
    #[error("PCI device {address} has no IOMMU group — the IOMMU does not cover it")]
    NoIommuGroup {
        /// The group-less device.
        address: PciAddress,
    },
    /// The device's live IOMMU group differs from the group pinned in the spec
    /// — the host topology changed since the operator recorded it.
    #[error(
        "PCI device {address} is in IOMMU group {actual}, but the spec pins group \
         {expected} — host topology changed; re-verify before passthrough"
    )]
    GroupMismatch {
        /// The device whose group moved.
        address: PciAddress,
        /// The group the spec pinned.
        expected: u32,
        /// The group sysfs reports now.
        actual: u32,
    },
    /// Another device in the same IOMMU group is still bound to a non-VFIO
    /// driver, so the group is not viable for passthrough (per the
    /// cloud-hypervisor vfio doc, every device sharing the group must be bound
    /// to VFIO and passed through).
    #[error(
        "IOMMU group {group} is not viable: member {member} is bound to {driver} — \
         every device in the group must be bound to {VFIO_PCI_DRIVER} (or unbound) \
         and passed through together"
    )]
    GroupNotViable {
        /// The IOMMU group that fails viability.
        group: u32,
        /// The offending group member.
        member: PciAddress,
        /// The non-VFIO driver it is bound to.
        driver: String,
    },
    /// A sysfs probe failed for a concrete runtime reason (unreadable node,
    /// unparseable link target, …).
    #[error("vfio probe {what}: {detail}")]
    Probe {
        /// What was being probed.
        what: String,
        /// The failure detail.
        detail: String,
    },
}

/// A full PCI address (`DDDD:BB:DD.F` — domain:bus:device.function, hex).
///
/// Validated + normalized to lowercase. This is the address cloud-hypervisor's
/// `DeviceConfig.path` names under `/sys/bus/pci/devices/`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PciAddress(String);

impl PciAddress {
    /// Parse + validate a PCI address (accepts upper/lowercase hex; stores
    /// lowercase).
    ///
    /// # Errors
    /// [`VfioError::InvalidPciAddress`] when the string is not `DDDD:BB:DD.F`
    /// (device ≤ `1f`, function ≤ `7`).
    pub fn parse(s: &str) -> Result<Self, VfioError> {
        s.parse()
    }

    /// The address as a string (`0000:01:00.0`).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The device's sysfs node (`/sys/bus/pci/devices/<addr>`) — exactly what
    /// [`build_ch_config`](crate::build_ch_config) emits as the VFIO
    /// `DeviceConfig.path`.
    #[must_use]
    pub fn sysfs_path(&self) -> PathBuf {
        PathBuf::from("/sys/bus/pci/devices").join(&self.0)
    }
}

impl fmt::Display for PciAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for PciAddress {
    type Err = VfioError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let norm = s.trim().to_ascii_lowercase();
        let invalid = || VfioError::InvalidPciAddress(s.to_string());
        // DDDD:BB:DD.F — split the function off first, then domain:bus:device.
        let (dbd, func) = norm.split_once('.').ok_or_else(invalid)?;
        let mut parts = dbd.split(':');
        let (Some(domain), Some(bus), Some(device), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(invalid());
        };
        let hex = |field: &str, width: usize| {
            (field.len() == width && field.chars().all(|c| c.is_ascii_hexdigit()))
                .then(|| u32::from_str_radix(field, 16).ok())
                .flatten()
                .ok_or_else(invalid)
        };
        hex(domain, 4)?;
        hex(bus, 2)?;
        // PCI device number is 5 bits (≤ 0x1f), function is 3 bits (≤ 7).
        if hex(device, 2)? > 0x1f || hex(func, 1)? > 0x7 {
            return Err(invalid());
        }
        Ok(Self(norm))
    }
}

impl TryFrom<String> for PciAddress {
    type Error = VfioError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<PciAddress> for String {
    fn from(addr: PciAddress) -> Self {
        addr.0
    }
}

/// One host PCI device passed through into the guest via VFIO — e.g. the dGPU
/// (lock 13).
///
/// The `iommu_group` pin is the operator's recorded expectation;
/// [`preflight_vfio`] refuses with [`VfioError::GroupMismatch`] if the live
/// topology moved. Like a NIC's role / a folder's `host_path`, the pin is a
/// *host-side* property: it drives the preflight, not the emitted CH config
/// (which carries only the device's sysfs `path`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VfioDevice {
    /// The host PCI address of the device to pass through.
    pub address: PciAddress,
    /// The IOMMU group the operator recorded when binding the device to
    /// `vfio-pci`; `None` skips the pin check (the group must still exist and
    /// be viable). Pin it — a silent topology change is exactly what the
    /// preflight is for.
    pub iommu_group: Option<u32>,
}

impl VfioDevice {
    /// A passthrough device at `address`, with no IOMMU-group pin (add one with
    /// [`VfioDevice::with_iommu_group`]).
    #[must_use]
    pub const fn new(address: PciAddress) -> Self {
        Self {
            address,
            iommu_group: None,
        }
    }

    /// Pin the expected IOMMU group (builder style).
    #[must_use]
    pub const fn with_iommu_group(mut self, group: u32) -> Self {
        self.iommu_group = Some(group);
        self
    }
}

/// Refuse a spec that requests VFIO passthrough without the explicit operator
/// opt-in ([`VmSpec::vfio_allowed`], lock 13).
///
/// Pure — no host probe — so
/// [`Vm::create`](crate::Vm::create) enforces it on every create, even when the
/// caller skipped [`preflight_vfio`].
///
/// # Errors
/// [`VfioError::NotOptedIn`] when the spec carries passthrough devices but
/// `vfio_allowed` is false.
pub fn ensure_vfio_opt_in(spec: &VmSpec) -> Result<(), VfioError> {
    if spec.vfio_devices.is_empty() || spec.vfio_allowed {
        Ok(())
    } else {
        Err(VfioError::NotOptedIn {
            vm: spec.name.clone(),
            devices: spec.vfio_devices.len(),
        })
    }
}

/// The injectable host-readiness probe behind [`preflight_vfio`].
///
/// The seam that keeps the preflight logic unit-testable without IOMMU
/// hardware (mirrors [`ChTransport`](crate::ChTransport)). Production is
/// [`SysfsVfioProbe`], which reads the real sysfs surface.
pub trait VfioProbe {
    /// Whether the host IOMMU is active (any group under
    /// `/sys/kernel/iommu_groups`).
    ///
    /// # Errors
    /// [`VfioError::Probe`] on an unreadable sysfs surface.
    fn iommu_active(&self) -> Result<bool, VfioError>;

    /// The driver `address` is bound to (`None` = unbound).
    ///
    /// # Errors
    /// [`VfioError::DeviceNotFound`] when the device has no sysfs node;
    /// [`VfioError::Probe`] on an unreadable surface.
    fn driver(&self, address: &PciAddress) -> Result<Option<String>, VfioError>;

    /// The IOMMU group `address` belongs to (`None` = the IOMMU does not cover
    /// it).
    ///
    /// # Errors
    /// [`VfioError::DeviceNotFound`] / [`VfioError::Probe`] as for
    /// [`VfioProbe::driver`].
    fn iommu_group(&self, address: &PciAddress) -> Result<Option<u32>, VfioError>;

    /// All PCI devices in IOMMU group `group`
    /// (`/sys/kernel/iommu_groups/<group>/devices`).
    ///
    /// # Errors
    /// [`VfioError::Probe`] on an unreadable surface or a non-PCI member.
    fn group_members(&self, group: u32) -> Result<Vec<PciAddress>, VfioError>;
}

/// The production [`VfioProbe`]: reads the live sysfs surface
/// (`/sys/kernel/iommu_groups`, `/sys/bus/pci/devices/...`).
///
/// The root is
/// injectable ([`SysfsVfioProbe::with_root`]) so the reader itself is
/// unit-tested against a scratch sysfs tree; a **real** preflight — IOMMU on,
/// a device bound to `vfio-pci` — is live-gated on passthrough hardware (none
/// on the build farm).
#[derive(Debug, Clone)]
pub struct SysfsVfioProbe {
    root: PathBuf,
}

impl Default for SysfsVfioProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl SysfsVfioProbe {
    /// A probe over the real `/sys`.
    #[must_use]
    pub fn new() -> Self {
        Self::with_root("/sys")
    }

    /// A probe over an alternate sysfs root (the test seam — point it at a
    /// scratch tree shaped like `/sys`).
    #[must_use]
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `<root>/bus/pci/devices/<addr>`.
    fn device_node(&self, address: &PciAddress) -> PathBuf {
        self.root.join("bus/pci/devices").join(address.as_str())
    }

    /// Read the trailing component of a symlink under a device node
    /// (`driver` → the driver name, `iommu_group` → the group number).
    fn link_target_name(
        &self,
        address: &PciAddress,
        link: &str,
    ) -> Result<Option<String>, VfioError> {
        let node = self.device_node(address);
        if !node.exists() {
            return Err(VfioError::DeviceNotFound {
                address: address.clone(),
            });
        }
        match std::fs::read_link(node.join(link)) {
            Ok(target) => Ok(target.file_name().map(|n| n.to_string_lossy().into_owned())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(VfioError::Probe {
                what: format!("{address}/{link}"),
                detail: e.to_string(),
            }),
        }
    }
}

impl VfioProbe for SysfsVfioProbe {
    fn iommu_active(&self) -> Result<bool, VfioError> {
        match std::fs::read_dir(self.root.join("kernel/iommu_groups")) {
            Ok(mut entries) => Ok(entries.next().is_some()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(VfioError::Probe {
                what: "kernel/iommu_groups".to_string(),
                detail: e.to_string(),
            }),
        }
    }

    fn driver(&self, address: &PciAddress) -> Result<Option<String>, VfioError> {
        self.link_target_name(address, "driver")
    }

    fn iommu_group(&self, address: &PciAddress) -> Result<Option<u32>, VfioError> {
        self.link_target_name(address, "iommu_group")?
            .map(|name| {
                name.parse::<u32>().map_err(|e| VfioError::Probe {
                    what: format!("{address}/iommu_group"),
                    detail: format!("group link '{name}' is not a number: {e}"),
                })
            })
            .transpose()
    }

    fn group_members(&self, group: u32) -> Result<Vec<PciAddress>, VfioError> {
        let dir = self
            .root
            .join("kernel/iommu_groups")
            .join(group.to_string())
            .join("devices");
        let entries = std::fs::read_dir(&dir).map_err(|e| VfioError::Probe {
            what: format!("kernel/iommu_groups/{group}/devices"),
            detail: e.to_string(),
        })?;
        let mut members = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| VfioError::Probe {
                what: format!("kernel/iommu_groups/{group}/devices"),
                detail: e.to_string(),
            })?;
            members.push(entry.file_name().to_string_lossy().parse()?);
        }
        members.sort_unstable_by(|a: &PciAddress, b| a.as_str().cmp(b.as_str()));
        Ok(members)
    }
}

/// The host-readiness preflight for a passthrough spec — run it before
/// [`Vm::create`](crate::Vm::create) on any spec carrying [`VfioDevice`]s.
/// Checks, in order, refusing with the first typed failure:
///
/// 1. the operator opt-in ([`ensure_vfio_opt_in`], lock 13);
/// 2. the host IOMMU is active;
/// 3. each device exists and is bound to `vfio-pci`;
/// 4. each device has an IOMMU group, matching the spec's pin when set;
/// 5. each group is **viable**: every member is bound to `vfio-pci` or unbound
///    (the cloud-hypervisor vfio rule for multi-function groups).
///
/// A spec with no passthrough devices passes trivially.
///
/// # Errors
/// The corresponding [`VfioError`] for the first failed check.
pub fn preflight_vfio(spec: &VmSpec, probe: &impl VfioProbe) -> Result<(), VfioError> {
    if spec.vfio_devices.is_empty() {
        return Ok(());
    }
    ensure_vfio_opt_in(spec)?;
    if !probe.iommu_active()? {
        return Err(VfioError::IommuDisabled);
    }
    for device in &spec.vfio_devices {
        let address = &device.address;
        match probe.driver(address)? {
            Some(ref driver) if driver == VFIO_PCI_DRIVER => {}
            other => {
                return Err(VfioError::NotBoundToVfioPci {
                    address: address.clone(),
                    driver: other.unwrap_or_else(|| "(unbound)".to_string()),
                })
            }
        }
        let group = probe
            .iommu_group(address)?
            .ok_or_else(|| VfioError::NoIommuGroup {
                address: address.clone(),
            })?;
        if let Some(expected) = device.iommu_group {
            if expected != group {
                return Err(VfioError::GroupMismatch {
                    address: address.clone(),
                    expected,
                    actual: group,
                });
            }
        }
        // Group viability: every sibling must be vfio-pci-bound or unbound.
        for member in probe.group_members(group)? {
            match probe.driver(&member)? {
                None => {}
                Some(ref driver) if driver == VFIO_PCI_DRIVER => {}
                Some(driver) => {
                    return Err(VfioError::GroupNotViable {
                        group,
                        member,
                        driver,
                    })
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::VmSpec;
    use std::collections::HashMap;

    fn addr(s: &str) -> PciAddress {
        PciAddress::parse(s).expect("valid test address")
    }

    fn gpu_spec() -> VmSpec {
        VmSpec::new("gpu1", 4, 8192, "/home/op/Local/gpu1.img")
            .with_vfio_device(VfioDevice::new(addr("0000:01:00.0")).with_iommu_group(14))
    }

    // ---- PciAddress ----

    #[test]
    fn pci_address_parses_and_normalizes_to_lowercase() {
        let a = addr("0000:01:00.0");
        assert_eq!(a.as_str(), "0000:01:00.0");
        assert_eq!(addr("0000:AF:1F.7").as_str(), "0000:af:1f.7");
        assert_eq!(a.to_string(), "0000:01:00.0");
    }

    #[test]
    fn pci_address_sysfs_path_is_the_device_node() {
        assert_eq!(
            addr("0000:01:00.0").sysfs_path(),
            PathBuf::from("/sys/bus/pci/devices/0000:01:00.0")
        );
    }

    #[test]
    fn pci_address_rejects_malformed_strings() {
        for bad in [
            "",
            "01:00.0",        // no domain
            "0000:01:00",     // no function
            "0000:01:00.8",   // function > 7
            "0000:01:20.0",   // device > 0x1f
            "000:01:00.0",    // short domain
            "0000:zz:00.0",   // not hex
            "0000:01:00.0.0", // extra part
            "0000:01:00:0",   // wrong separator
            "a:0000:01:00.0", // extra colon field
        ] {
            let err = PciAddress::parse(bad).expect_err(bad);
            assert!(
                matches!(err, VfioError::InvalidPciAddress(ref s) if s == bad),
                "{bad}: {err:?}"
            );
        }
    }

    #[test]
    fn pci_address_serde_round_trips_and_validates() {
        let a = addr("0000:01:00.0");
        let json = serde_json::to_string(&a).expect("serialize");
        assert_eq!(json, "\"0000:01:00.0\"");
        let back: PciAddress = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(a, back);
        // deserialization validates — garbage is refused, not smuggled in.
        assert!(serde_json::from_str::<PciAddress>("\"nonsense\"").is_err());
    }

    // ---- opt-in gate ----

    #[test]
    fn passthrough_without_opt_in_is_refused_with_a_typed_error() {
        // THE lock-13 acceptance: devices present, no opt-in → typed refusal.
        let spec = gpu_spec();
        let err = ensure_vfio_opt_in(&spec).expect_err("must refuse");
        assert!(
            matches!(&err, VfioError::NotOptedIn { vm, devices: 1 } if vm == "gpu1"),
            "{err:?}"
        );
        // the rendered error tells the operator exactly what to do.
        let msg = err.to_string();
        assert!(msg.contains("opted in"), "{msg}");
        assert!(msg.contains("allow_vfio"), "{msg}");
    }

    #[test]
    fn opt_in_or_no_devices_passes_the_gate() {
        assert!(ensure_vfio_opt_in(&gpu_spec().allow_vfio(true)).is_ok());
        // no devices ⇒ nothing to refuse, opted-in or not.
        assert!(ensure_vfio_opt_in(&VmSpec::new("plain", 1, 512, "/d.img")).is_ok());
    }

    // ---- preflight over a fake probe ----

    /// An in-memory [`VfioProbe`]: a device→(driver, group) map + group
    /// membership, so every preflight path is unit-tested without hardware.
    #[derive(Default)]
    struct FakeProbe {
        iommu_active: bool,
        /// address → (bound driver, iommu group)
        devices: HashMap<String, (Option<String>, Option<u32>)>,
    }

    impl FakeProbe {
        fn with_device(mut self, a: &str, driver: Option<&str>, group: Option<u32>) -> Self {
            self.devices
                .insert(a.to_string(), (driver.map(str::to_string), group));
            self.iommu_active = true;
            self
        }
    }

    impl VfioProbe for FakeProbe {
        fn iommu_active(&self) -> Result<bool, VfioError> {
            Ok(self.iommu_active)
        }

        fn driver(&self, address: &PciAddress) -> Result<Option<String>, VfioError> {
            self.devices
                .get(address.as_str())
                .map(|(driver, _)| driver.clone())
                .ok_or_else(|| VfioError::DeviceNotFound {
                    address: address.clone(),
                })
        }

        fn iommu_group(&self, address: &PciAddress) -> Result<Option<u32>, VfioError> {
            self.devices
                .get(address.as_str())
                .map(|(_, group)| *group)
                .ok_or_else(|| VfioError::DeviceNotFound {
                    address: address.clone(),
                })
        }

        fn group_members(&self, group: u32) -> Result<Vec<PciAddress>, VfioError> {
            let mut members: Vec<PciAddress> = self
                .devices
                .iter()
                .filter(|(_, (_, g))| *g == Some(group))
                .map(|(a, _)| addr(a))
                .collect();
            members.sort_unstable_by(|a, b| a.as_str().cmp(b.as_str()));
            Ok(members)
        }
    }

    #[test]
    fn preflight_passes_on_a_ready_host() {
        // THE happy path: IOMMU on, the dGPU + its audio function both bound to
        // vfio-pci in the pinned group.
        let probe = FakeProbe::default()
            .with_device("0000:01:00.0", Some(VFIO_PCI_DRIVER), Some(14))
            .with_device("0000:01:00.1", Some(VFIO_PCI_DRIVER), Some(14));
        preflight_vfio(&gpu_spec().allow_vfio(true), &probe).expect("ready host");
    }

    #[test]
    fn preflight_skips_trivially_without_passthrough_devices() {
        // no devices ⇒ no probe calls needed at all (an empty probe suffices).
        let spec = VmSpec::new("plain", 1, 512, "/d.img");
        preflight_vfio(&spec, &FakeProbe::default()).expect("trivial pass");
    }

    #[test]
    fn preflight_refuses_without_opt_in_before_touching_the_host() {
        let err = preflight_vfio(&gpu_spec(), &FakeProbe::default()).expect_err("refuse");
        assert!(matches!(err, VfioError::NotOptedIn { .. }), "{err:?}");
    }

    #[test]
    fn preflight_requires_an_active_iommu() {
        let mut probe =
            FakeProbe::default().with_device("0000:01:00.0", Some(VFIO_PCI_DRIVER), Some(14));
        probe.iommu_active = false;
        let err = preflight_vfio(&gpu_spec().allow_vfio(true), &probe).expect_err("iommu off");
        assert!(matches!(err, VfioError::IommuDisabled), "{err:?}");
        assert!(err.to_string().contains("intel_iommu=on"), "{err}");
    }

    #[test]
    fn preflight_requires_the_device_bound_to_vfio_pci() {
        // still on the native driver…
        let probe = FakeProbe::default().with_device("0000:01:00.0", Some("nvidia"), Some(14));
        let err = preflight_vfio(&gpu_spec().allow_vfio(true), &probe).expect_err("wrong driver");
        assert!(
            matches!(&err, VfioError::NotBoundToVfioPci { driver, .. } if driver == "nvidia"),
            "{err:?}"
        );
        // …or unbound entirely.
        let probe = FakeProbe::default().with_device("0000:01:00.0", None, Some(14));
        let err = preflight_vfio(&gpu_spec().allow_vfio(true), &probe).expect_err("unbound");
        assert!(
            matches!(&err, VfioError::NotBoundToVfioPci { driver, .. } if driver == "(unbound)"),
            "{err:?}"
        );
    }

    #[test]
    fn preflight_reports_a_missing_device() {
        let probe = FakeProbe {
            iommu_active: true,
            ..FakeProbe::default()
        };
        let err = preflight_vfio(&gpu_spec().allow_vfio(true), &probe).expect_err("absent");
        assert!(matches!(err, VfioError::DeviceNotFound { .. }), "{err:?}");
    }

    #[test]
    fn preflight_refuses_a_group_pin_mismatch() {
        // the operator pinned group 14; the live topology says 9.
        let probe =
            FakeProbe::default().with_device("0000:01:00.0", Some(VFIO_PCI_DRIVER), Some(9));
        let err = preflight_vfio(&gpu_spec().allow_vfio(true), &probe).expect_err("moved");
        assert!(
            matches!(
                err,
                VfioError::GroupMismatch {
                    expected: 14,
                    actual: 9,
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn preflight_without_a_pin_accepts_any_group() {
        let spec = VmSpec::new("gpu1", 4, 8192, "/g.img")
            .with_vfio_device(VfioDevice::new(addr("0000:01:00.0"))) // no pin
            .allow_vfio(true);
        let probe =
            FakeProbe::default().with_device("0000:01:00.0", Some(VFIO_PCI_DRIVER), Some(9));
        preflight_vfio(&spec, &probe).expect("unpinned group accepted");
    }

    #[test]
    fn preflight_requires_the_device_to_have_a_group() {
        let probe = FakeProbe::default().with_device("0000:01:00.0", Some(VFIO_PCI_DRIVER), None);
        let err = preflight_vfio(&gpu_spec().allow_vfio(true), &probe).expect_err("no group");
        assert!(matches!(err, VfioError::NoIommuGroup { .. }), "{err:?}");
    }

    #[test]
    fn preflight_refuses_a_non_viable_group() {
        // THE group-viability acceptance: the dGPU is vfio-bound, but its audio
        // sibling in group 14 is still on snd_hda_intel → refused, naming the
        // offender (the CH vfio doc's functional/security rule).
        let probe = FakeProbe::default()
            .with_device("0000:01:00.0", Some(VFIO_PCI_DRIVER), Some(14))
            .with_device("0000:01:00.1", Some("snd_hda_intel"), Some(14));
        let err = preflight_vfio(&gpu_spec().allow_vfio(true), &probe).expect_err("not viable");
        assert!(
            matches!(
                &err,
                VfioError::GroupNotViable { group: 14, member, driver }
                    if member.as_str() == "0000:01:00.1" && driver == "snd_hda_intel"
            ),
            "{err:?}"
        );
        // an UNBOUND sibling is fine (viable) — only a foreign driver refuses.
        let probe = FakeProbe::default()
            .with_device("0000:01:00.0", Some(VFIO_PCI_DRIVER), Some(14))
            .with_device("0000:01:00.1", None, Some(14));
        preflight_vfio(&gpu_spec().allow_vfio(true), &probe).expect("unbound sibling is viable");
    }

    // ---- the live sysfs reader, against a scratch sysfs tree ----

    /// A scratch sysfs-shaped tree under the target dir, removed on drop.
    struct ScratchSysfs(PathBuf);

    impl ScratchSysfs {
        fn new(tag: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("mde-kvm-vfio-{tag}-{}", std::process::id()));
            // stale leftovers from a killed run
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("scratch root");
            Self(root)
        }

        /// Create a device node with an optional bound driver + iommu group,
        /// mirroring the real /sys link shapes.
        fn add_device(&self, a: &str, driver: Option<&str>, group: Option<u32>) {
            let node = self.0.join("bus/pci/devices").join(a);
            std::fs::create_dir_all(&node).expect("device node");
            if let Some(driver) = driver {
                let drv_dir = self.0.join("bus/pci/drivers").join(driver);
                std::fs::create_dir_all(&drv_dir).expect("driver dir");
                std::os::unix::fs::symlink(&drv_dir, node.join("driver")).expect("driver link");
            }
            if let Some(group) = group {
                let grp_dir = self.0.join("kernel/iommu_groups").join(group.to_string());
                let dev_list = grp_dir.join("devices");
                std::fs::create_dir_all(&dev_list).expect("group devices dir");
                std::os::unix::fs::symlink(&grp_dir, node.join("iommu_group")).expect("group link");
                std::os::unix::fs::symlink(&node, dev_list.join(a)).expect("member link");
            }
        }

        fn probe(&self) -> SysfsVfioProbe {
            SysfsVfioProbe::with_root(&self.0)
        }
    }

    impl Drop for ScratchSysfs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn sysfs_probe_reads_driver_group_and_members_from_the_tree() {
        let sys = ScratchSysfs::new("read");
        sys.add_device("0000:01:00.0", Some(VFIO_PCI_DRIVER), Some(14));
        sys.add_device("0000:01:00.1", Some("snd_hda_intel"), Some(14));
        sys.add_device("0000:02:00.0", None, Some(15));
        let probe = sys.probe();

        assert!(probe.iommu_active().expect("active"));
        let gpu = addr("0000:01:00.0");
        assert_eq!(
            probe.driver(&gpu).expect("driver"),
            Some(VFIO_PCI_DRIVER.to_string())
        );
        assert_eq!(probe.iommu_group(&gpu).expect("group"), Some(14));
        // unbound device → no driver; still grouped.
        let nic = addr("0000:02:00.0");
        assert_eq!(probe.driver(&nic).expect("driver"), None);
        assert_eq!(probe.iommu_group(&nic).expect("group"), Some(15));
        // group membership is enumerated (sorted).
        let members = probe.group_members(14).expect("members");
        assert_eq!(members, vec![addr("0000:01:00.0"), addr("0000:01:00.1")]);
    }

    #[test]
    fn sysfs_probe_types_missing_pieces() {
        let sys = ScratchSysfs::new("missing");
        sys.add_device("0000:01:00.0", None, None);
        let probe = sys.probe();
        // no iommu_groups dir at all → IOMMU inactive (not an error).
        assert!(!probe.iommu_active().expect("inactive"));
        // ungrouped device → None.
        assert_eq!(
            probe.iommu_group(&addr("0000:01:00.0")).expect("group"),
            None
        );
        // absent device → typed DeviceNotFound.
        let err = probe.driver(&addr("0000:09:00.0")).expect_err("absent");
        assert!(matches!(err, VfioError::DeviceNotFound { .. }), "{err:?}");
        // unknown group → typed Probe error.
        let err = probe.group_members(99).expect_err("no group");
        assert!(matches!(err, VfioError::Probe { .. }), "{err:?}");
    }

    #[test]
    fn preflight_runs_end_to_end_over_the_sysfs_probe() {
        // the same preflight logic, driven through the REAL sysfs reader over a
        // scratch tree — the closest the farm gets to live hardware.
        let sys = ScratchSysfs::new("e2e");
        sys.add_device("0000:01:00.0", Some(VFIO_PCI_DRIVER), Some(14));
        preflight_vfio(&gpu_spec().allow_vfio(true), &sys.probe()).expect("ready");
    }

    /// Live-gated: a real preflight against the machine's actual `/sys`.
    /// Needs IOMMU hardware + a device already bound to vfio-pci; set
    /// `MDE_KVM_TEST_VFIO_PCI` to that device's full PCI address and run with
    /// `--ignored`.
    #[test]
    #[ignore = "needs real IOMMU hardware; set MDE_KVM_TEST_VFIO_PCI and run --ignored"]
    fn live_preflight_against_real_sysfs() {
        let Ok(pci) = std::env::var("MDE_KVM_TEST_VFIO_PCI") else {
            eprintln!("MDE_KVM_TEST_VFIO_PCI unset; skipping the live preflight");
            return;
        };
        let device = VfioDevice::new(PciAddress::parse(&pci).expect("valid MDE_KVM_TEST_VFIO_PCI"));
        let spec = VmSpec::new("live", 2, 2048, "/tmp/live.img")
            .with_vfio_device(device)
            .allow_vfio(true);
        preflight_vfio(&spec, &SysfsVfioProbe::new()).expect("live host ready for passthrough");
    }
}
