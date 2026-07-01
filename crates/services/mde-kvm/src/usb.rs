//! USB redirection (E12-10, lock 48) — **honest about what cloud-hypervisor
//! can do**.
//!
//! Researched against the targeted cloud-hypervisor (its `OpenAPI`
//! `VmConfig`/`DeviceConfig` surface and `docs/device_model.md`): the VMM has
//! **no USB support at all** — no emulated controller (xHCI/EHCI/UHCI), no
//! guest USB device model, no USB-related config knob anywhere in its API.
//! That is a deliberate reduced-device-model choice, so this broker does
//! **not** fabricate a config knob the VMM won't accept. A per-device
//! VMM-level attach request gets a typed [`UsbError::UnsupportedByVmm`] —
//! never a fake success (§7).
//!
//! What actually works, modeled here as [`UsbRedirect`]:
//!
//! 1. **Whole-controller passthrough** ([`UsbRedirect::Controller`]): a host
//!    USB *controller* is a plain PCI device, so it rides the E12-10 **VFIO**
//!    slice — [`plan_usb_redirect`] maps it to a [`VfioDevice`] the caller
//!    adds to the [`VmSpec`](crate::VmSpec) (the operator opt-in +
//!    [`preflight_vfio`](crate::preflight_vfio) then apply as for any
//!    passthrough). Every port on that controller — hence any device plugged
//!    into it — belongs to the guest.
//! 2. **Protocol-side redirection** (the doc note, not this crate's seam):
//!    for *remote* desktops the USB device rides the display protocol, not
//!    the VMM — RDP device redirection (the `RDPEUSB`/redirection channels)
//!    in the `mde-vdi-rdp`/ironrdp path. That is where per-device,
//!    hot-pluggable redirection lives in the Quasar design; the VMM-level
//!    error text points operators there.

use std::fmt;

use thiserror::Error;

use crate::vfio::{PciAddress, VfioDevice};

/// A USB redirection failure — typed, so the shell can route the operator to
/// the path that actually works instead of pretending.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UsbError {
    /// cloud-hypervisor cannot attach individual USB devices: it has no
    /// emulated USB controller and no USB device model (verified against its
    /// `OpenAPI` — no USB schema/property exists). The supported routes are
    /// named in the message.
    #[error(
        "cloud-hypervisor cannot redirect USB device {device} into the guest: the VMM \
         has no USB support (no emulated xHCI controller, no USB config in its API). \
         Use whole-controller VFIO passthrough (UsbRedirect::Controller — the guest \
         owns every port on that controller), or protocol-side RDP USB redirection \
         (mde-vdi-rdp) for remote desktops"
    )]
    UnsupportedByVmm {
        /// The USB device whose per-device attach was requested.
        device: UsbDeviceId,
    },
}

/// A USB device identity (`vendor:product`, e.g. `046d:c52b`) — how an
/// operator names the device they want redirected (lsusb's ID column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct UsbDeviceId {
    /// The USB vendor id (`046d` = Logitech).
    pub vendor_id: u16,
    /// The USB product id.
    pub product_id: u16,
}

impl UsbDeviceId {
    /// A device id from its vendor/product pair.
    #[must_use]
    pub const fn new(vendor_id: u16, product_id: u16) -> Self {
        Self {
            vendor_id,
            product_id,
        }
    }
}

impl fmt::Display for UsbDeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04x}:{:04x}", self.vendor_id, self.product_id)
    }
}

/// A USB redirection request, at the two granularities that exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsbRedirect {
    /// Redirect one USB device into the guest **at the VMM level**. This is
    /// what cloud-hypervisor cannot do — [`plan_usb_redirect`] refuses it
    /// with the typed [`UsbError::UnsupportedByVmm`] naming the real routes.
    Device(UsbDeviceId),
    /// Pass a whole host USB **controller** (a PCI device) into the guest via
    /// VFIO — the supported VMM-level route. The guest owns every port on the
    /// controller.
    Controller(PciAddress),
}

/// What a supportable USB redirection request maps onto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsbPlan {
    /// Add this [`VfioDevice`] to the spec
    /// ([`VmSpec::with_vfio_device`](crate::VmSpec::with_vfio_device)) — the
    /// VFIO opt-in ([`VmSpec::allow_vfio`](crate::VmSpec::allow_vfio)) and
    /// [`preflight_vfio`](crate::preflight_vfio) then gate it exactly like a
    /// GPU passthrough.
    ControllerPassthrough(VfioDevice),
}

/// Plan a USB redirection against what cloud-hypervisor actually supports.
///
/// - [`UsbRedirect::Controller`] → the whole-controller VFIO passthrough plan.
/// - [`UsbRedirect::Device`] → a typed refusal: the VMM has no per-device USB
///   attach; the error names the two real routes (controller passthrough, or
///   RDP-side redirection for remote desktops).
///
/// # Errors
/// [`UsbError::UnsupportedByVmm`] for a per-device VMM attach request.
pub fn plan_usb_redirect(redirect: &UsbRedirect) -> Result<UsbPlan, UsbError> {
    match redirect {
        UsbRedirect::Device(device) => Err(UsbError::UnsupportedByVmm { device: *device }),
        UsbRedirect::Controller(address) => Ok(UsbPlan::ControllerPassthrough(VfioDevice::new(
            address.clone(),
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::build_ch_config;
    use crate::spec::VmSpec;
    use crate::vfio::ensure_vfio_opt_in;

    #[test]
    fn per_device_redirect_is_typed_unsupported_naming_the_real_routes() {
        // THE honesty acceptance: CH has no USB device model, so a per-device
        // attach is refused with a typed error — never a fabricated knob.
        let mouse = UsbDeviceId::new(0x046d, 0xc52b);
        let err = plan_usb_redirect(&UsbRedirect::Device(mouse)).expect_err("must refuse");
        assert!(
            matches!(err, UsbError::UnsupportedByVmm { device } if device == mouse),
            "{err:?}"
        );
        // the rendered error routes the operator to what actually works.
        let msg = err.to_string();
        assert!(msg.contains("046d:c52b"), "{msg}");
        assert!(msg.contains("no USB support"), "{msg}");
        assert!(msg.contains("VFIO passthrough"), "{msg}");
        assert!(msg.contains("mde-vdi-rdp"), "{msg}");
    }

    #[test]
    fn controller_redirect_plans_a_vfio_passthrough_of_the_pci_controller() {
        let xhci = PciAddress::parse("0000:00:14.0").expect("xhci addr");
        let UsbPlan::ControllerPassthrough(device) =
            plan_usb_redirect(&UsbRedirect::Controller(xhci.clone())).expect("plan");
        assert_eq!(device.address, xhci);
        // no group pin yet — the operator pins it when binding vfio-pci.
        assert_eq!(device.iommu_group, None);
    }

    #[test]
    fn planned_controller_rides_the_vfio_slice_gates_and_config() {
        // end-to-end through the VFIO slice: the planned controller lands in
        // the spec, is refused without the operator opt-in, and — once opted
        // in — emits the CH `devices` entry for the controller's sysfs node.
        let xhci = PciAddress::parse("0000:00:14.0").expect("xhci addr");
        let UsbPlan::ControllerPassthrough(device) =
            plan_usb_redirect(&UsbRedirect::Controller(xhci)).expect("plan");
        let spec = VmSpec::new("usb1", 2, 2048, "/u.img").with_vfio_device(device);
        assert!(ensure_vfio_opt_in(&spec).is_err(), "opt-in still gates USB");
        let cfg = build_ch_config(&spec.allow_vfio(true));
        assert_eq!(
            cfg["devices"][0]["path"],
            serde_json::json!("/sys/bus/pci/devices/0000:00:14.0")
        );
    }

    #[test]
    fn usb_device_id_displays_as_lsusb_style_hex() {
        assert_eq!(UsbDeviceId::new(0x046d, 0xc52b).to_string(), "046d:c52b");
        // zero-padded to 4 hex digits, like lsusb.
        assert_eq!(UsbDeviceId::new(0x1, 0x2).to_string(), "0001:0002");
    }

    #[test]
    fn usb_device_id_serde_round_trips() {
        let id = UsbDeviceId::new(0x046d, 0xc52b);
        let json = serde_json::to_string(&id).expect("serialize");
        let back: UsbDeviceId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }
}
