//! The `BlueZ` client — adapter + device enumeration over the system D-Bus.
//!
//! `BlueZ` publishes its whole world as one `ObjectManager` tree on `org.bluez`:
//! adapters (`org.bluez.Adapter1`), devices (`org.bluez.Device1`), and — for
//! peripherals that report charge — `org.bluez.Battery1` on the same device
//! object. One `GetManagedObjects` round-trip is therefore the entire read; the
//! fold from that tree into [`BtStatus`] is pure and unit-tested headless.
//!
//! E12-15 gave this client **enumeration** (the snapshot feeds the System
//! surface's Bluetooth section + the chrome icon). E12-17 layers the full
//! pairing manager on top: the adapter/scan/pair/trust/connect/forget verbs and
//! the seat-start auto-reconnect below, the [`ScanTracker`] proximity-announce
//! fold, and the PIN/passkey pairing agent in [`crate::pairing`].

use crate::bus::SysBus;
use crate::error::{Backend, SeatError};
use crate::props::{bool_prop, str_prop, u8_prop, PropMap};

/// The `BlueZ` well-known bus name.
const BLUEZ: &str = "org.bluez";
/// The adapter interface (`StartDiscovery`, `RemoveDevice`, `Powered`…).
const ADAPTER1: &str = "org.bluez.Adapter1";
/// The device interface (`Pair`, `Connect`, `Trusted`…).
const DEVICE1: &str = "org.bluez.Device1";
/// The FDO properties interface — how a `bool` toggle (`Powered`, `Trusted`,
/// `Discoverable`, `Pairable`) is written on either object.
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// One Bluetooth adapter (an `org.bluez.Adapter1` object).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtAdapter {
    /// The D-Bus object path (e.g. `/org/bluez/hci0`) — the stable identity.
    pub path: String,
    /// The operator-facing name (`Alias`, falling back to `Name`, then the
    /// path tail).
    pub name: String,
    /// Whether the radio is powered on.
    pub powered: bool,
    /// Whether a device discovery scan is running.
    pub discovering: bool,
}

/// One remote Bluetooth device (an `org.bluez.Device1` object).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtDevice {
    /// The D-Bus object path — the stable identity.
    pub path: String,
    /// The operator-facing name (`Alias` → `Name` → `Address` → path tail).
    pub alias: String,
    /// Paired (bonded) with this host.
    pub paired: bool,
    /// Currently connected.
    pub connected: bool,
    /// Trusted for auto-reconnect.
    pub trusted: bool,
    /// The peripheral's own battery charge (`org.bluez.Battery1`), when the
    /// device reports one. `None` = not reported — never a fabricated value.
    pub battery_percent: Option<u8>,
    /// The `BlueZ` icon hint (`"input-keyboard"`, `"audio-headset"`, …), when set.
    pub icon: Option<String>,
}

/// The folded Bluetooth world: every adapter and every known device.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BtStatus {
    /// Adapters, sorted by object path.
    pub adapters: Vec<BtAdapter>,
    /// Known devices, connected first, then by alias.
    pub devices: Vec<BtDevice>,
}

impl BtStatus {
    /// Is any adapter radio powered on?
    #[must_use]
    pub fn any_adapter_powered(&self) -> bool {
        self.adapters.iter().any(|a| a.powered)
    }

    /// How many devices are currently connected.
    #[must_use]
    pub fn connected_devices(&self) -> usize {
        self.devices.iter().filter(|d| d.connected).count()
    }
}

/// One trusted device the auto-reconnect pass tried to bring back at seat start.
///
/// [`BluezClient::reconnect_trusted`] never fails as a whole — a per-device
/// failure lands here so the shell can log "couldn't reach the keyboard" while
/// the rest reconnect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconnectAttempt {
    /// The device object path.
    pub path: String,
    /// Whether the `Connect` call succeeded.
    pub connected: bool,
    /// The typed error message when it did not — `None` on success.
    pub error: Option<String>,
}

/// The device paths a seat-start reconnect should target: trusted (the operator
/// keeps them) but not currently connected. Pure — the drive logic in
/// [`BluezClient::reconnect_trusted`] folds over this.
#[must_use]
pub fn trusted_reconnect_targets(status: &BtStatus) -> Vec<String> {
    status
        .devices
        .iter()
        .filter(|d| d.trusted && !d.connected)
        .map(|d| d.path.clone())
        .collect()
}

/// The `BlueZ` client seam — enumeration (E12-15) plus the full pairing-manager
/// verb set (E12-17). The production impl is [`ZbusBluez`]; tests inject a fake.
///
/// Every verb is fire-and-observe over the standard `org.bluez` interfaces: a
/// missing adapter / dead bus comes back as a typed [`SeatError::Unavailable`]
/// (the honest not-available render, §7 / interlock 4), never a silent no-op.
/// The pairing *agent* that answers PIN/passkey prompts during [`Self::pair`] is
/// registered separately — see [`crate::pairing::PairingAgent`].
pub trait BluezClient: Send {
    /// Enumerate adapters + devices.
    ///
    /// # Errors
    /// Typed: [`SeatError::Unavailable`] when `BlueZ` / the system bus is absent.
    fn status(&self) -> Result<BtStatus, SeatError>;

    /// Power an adapter radio on or off (`Adapter1.Powered`).
    ///
    /// # Errors
    /// Typed: absent adapter / bus → [`SeatError::Unavailable`], else `Backend`.
    fn set_adapter_powered(&self, adapter: &str, on: bool) -> Result<(), SeatError>;

    /// Make an adapter discoverable to nearby devices (`Adapter1.Discoverable`).
    ///
    /// # Errors
    /// Typed as the other adapter verbs.
    fn set_discoverable(&self, adapter: &str, on: bool) -> Result<(), SeatError>;

    /// Allow an adapter to accept incoming pairings (`Adapter1.Pairable`).
    ///
    /// # Errors
    /// Typed as the other adapter verbs.
    fn set_pairable(&self, adapter: &str, on: bool) -> Result<(), SeatError>;

    /// Start a device-discovery scan (`Adapter1.StartDiscovery`). The shell then
    /// polls [`Self::status`] and folds it through a [`ScanTracker`] to surface
    /// newly-found devices (the proximity-announce popups).
    ///
    /// # Errors
    /// Typed as the other adapter verbs.
    fn start_discovery(&self, adapter: &str) -> Result<(), SeatError>;

    /// Stop a device-discovery scan (`Adapter1.StopDiscovery`).
    ///
    /// # Errors
    /// Typed as the other adapter verbs.
    fn stop_discovery(&self, adapter: &str) -> Result<(), SeatError>;

    /// Pair (bond) with a device (`Device1.Pair`). PIN/passkey prompts are
    /// answered by the registered [`crate::pairing::PairingAgent`]; without one
    /// registered, `BlueZ` fails the pairing typed rather than hanging.
    ///
    /// # Errors
    /// Typed: absent device / bus → `Unavailable`, a rejected/failed pairing →
    /// `Backend`.
    fn pair(&self, device: &str) -> Result<(), SeatError>;

    /// Abort an in-flight pairing (`Device1.CancelPairing`).
    ///
    /// # Errors
    /// Typed as [`Self::pair`].
    fn cancel_pairing(&self, device: &str) -> Result<(), SeatError>;

    /// Trust or untrust a device for auto-reconnect (`Device1.Trusted`).
    ///
    /// # Errors
    /// Typed: absent device / bus → `Unavailable`, else `Backend`.
    fn set_trusted(&self, device: &str, trusted: bool) -> Result<(), SeatError>;

    /// Connect to a paired device (`Device1.Connect`).
    ///
    /// # Errors
    /// Typed as [`Self::set_trusted`].
    fn connect(&self, device: &str) -> Result<(), SeatError>;

    /// Disconnect a connected device (`Device1.Disconnect`).
    ///
    /// # Errors
    /// Typed as [`Self::set_trusted`].
    fn disconnect(&self, device: &str) -> Result<(), SeatError>;

    /// Forget a device — drop the bond + link keys (`Adapter1.RemoveDevice`).
    /// `adapter` is the owning adapter path; `device` the device path to remove.
    ///
    /// # Errors
    /// Typed: an invalid `device` path → [`SeatError::Protocol`]; absent adapter
    /// / bus → `Unavailable`, else `Backend`.
    fn remove_device(&self, adapter: &str, device: &str) -> Result<(), SeatError>;

    /// Reconnect every trusted-but-disconnected device — the seat-start
    /// auto-reconnect the shell calls once on init. Drives [`Self::status`] +
    /// [`Self::connect`]; never fails as a whole, capturing each device's outcome
    /// in a [`ReconnectAttempt`].
    ///
    /// # Errors
    /// Only the initial [`Self::status`] read can fail the call as a whole
    /// (no adapter / bus → `Unavailable`); per-device connect failures are
    /// returned inline, not raised.
    fn reconnect_trusted(&self) -> Result<Vec<ReconnectAttempt>, SeatError> {
        let status = self.status()?;
        Ok(trusted_reconnect_targets(&status)
            .into_iter()
            .map(|path| match self.connect(&path) {
                Ok(()) => ReconnectAttempt {
                    path,
                    connected: true,
                    error: None,
                },
                Err(e) => ReconnectAttempt {
                    path,
                    connected: false,
                    error: Some(e.to_string()),
                },
            })
            .collect())
    }
}

/// Tracks which devices an active scan has already surfaced, so each poll yields
/// only the devices that newly appeared.
///
/// This is the data behind the Android-style proximity-announce popup (lock 5).
/// The shell drives one tracker per scan:
/// after [`BluezClient::start_discovery`], poll [`BluezClient::status`] on a
/// cadence and feed each snapshot to [`ScanTracker::poll`]; every returned device
/// is a fresh "Found X — Pair?" candidate. Already-paired devices are never
/// announced (they are already known). Pure + unit-tested.
#[derive(Debug, Default)]
pub struct ScanTracker {
    seen: std::collections::HashSet<String>,
}

impl ScanTracker {
    /// A fresh tracker that has surfaced nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold this poll's snapshot: return the not-yet-surfaced, un-paired devices
    /// that appeared since the last poll (announce candidates), and remember
    /// them so they do not re-announce.
    pub fn poll(&mut self, status: &BtStatus) -> Vec<BtDevice> {
        status
            .devices
            .iter()
            .filter(|d| !d.paired && self.seen.insert(d.path.clone()))
            .cloned()
            .collect()
    }

    /// Forget every surfaced device — call when a new scan starts so devices
    /// still nearby announce again.
    pub fn reset(&mut self) {
        self.seen.clear();
    }
}

/// The production `BlueZ` client — one `ObjectManager.GetManagedObjects` call,
/// folded by the pure [`fold_bluez`].
pub struct ZbusBluez {
    bus: SysBus,
}

impl ZbusBluez {
    /// A client over the system bus. No I/O until the first call.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            bus: SysBus::new(Backend::Bluetooth),
        }
    }
}

impl Default for ZbusBluez {
    fn default() -> Self {
        Self::new()
    }
}

impl ZbusBluez {
    /// Write a `bool` property on a `BlueZ` object via the FDO `Properties.Set`
    /// (`ssv`) — the one path for every radio/link toggle (`Powered`, `Trusted`,
    /// `Discoverable`, `Pairable`).
    fn set_bool_prop(
        &self,
        path: &str,
        interface: &str,
        name: &str,
        value: bool,
    ) -> Result<(), SeatError> {
        self.bus.call_unit(
            BLUEZ,
            path,
            PROPERTIES,
            "Set",
            &(interface, name, zbus::zvariant::Value::from(value)),
        )
    }
}

impl BluezClient for ZbusBluez {
    fn status(&self) -> Result<BtStatus, SeatError> {
        let objects: zbus::fdo::ManagedObjects = self.bus.call(
            BLUEZ,
            "/",
            "org.freedesktop.DBus.ObjectManager",
            "GetManagedObjects",
            &(),
        )?;
        Ok(fold_bluez(&objects))
    }

    fn set_adapter_powered(&self, adapter: &str, on: bool) -> Result<(), SeatError> {
        self.set_bool_prop(adapter, ADAPTER1, "Powered", on)
    }

    fn set_discoverable(&self, adapter: &str, on: bool) -> Result<(), SeatError> {
        self.set_bool_prop(adapter, ADAPTER1, "Discoverable", on)
    }

    fn set_pairable(&self, adapter: &str, on: bool) -> Result<(), SeatError> {
        self.set_bool_prop(adapter, ADAPTER1, "Pairable", on)
    }

    fn start_discovery(&self, adapter: &str) -> Result<(), SeatError> {
        self.bus
            .call_unit(BLUEZ, adapter, ADAPTER1, "StartDiscovery", &())
    }

    fn stop_discovery(&self, adapter: &str) -> Result<(), SeatError> {
        self.bus
            .call_unit(BLUEZ, adapter, ADAPTER1, "StopDiscovery", &())
    }

    fn pair(&self, device: &str) -> Result<(), SeatError> {
        self.bus.call_unit(BLUEZ, device, DEVICE1, "Pair", &())
    }

    fn cancel_pairing(&self, device: &str) -> Result<(), SeatError> {
        self.bus
            .call_unit(BLUEZ, device, DEVICE1, "CancelPairing", &())
    }

    fn set_trusted(&self, device: &str, trusted: bool) -> Result<(), SeatError> {
        self.set_bool_prop(device, DEVICE1, "Trusted", trusted)
    }

    fn connect(&self, device: &str) -> Result<(), SeatError> {
        self.bus.call_unit(BLUEZ, device, DEVICE1, "Connect", &())
    }

    fn disconnect(&self, device: &str) -> Result<(), SeatError> {
        self.bus
            .call_unit(BLUEZ, device, DEVICE1, "Disconnect", &())
    }

    fn remove_device(&self, adapter: &str, device: &str) -> Result<(), SeatError> {
        // RemoveDevice's arg is an `o` (object path), not a string — an invalid
        // path is a typed Protocol error, never a mis-typed wire call.
        let dev =
            zbus::zvariant::ObjectPath::try_from(device).map_err(|e| SeatError::Protocol {
                backend: Backend::Bluetooth,
                reason: format!("invalid device path {device:?}: {e}"),
            })?;
        self.bus
            .call_unit(BLUEZ, adapter, ADAPTER1, "RemoveDevice", &(dev,))
    }
}

/// The interface map of one managed object, keyed by interface name string.
fn iface<'a>(
    interfaces: &'a std::collections::HashMap<zbus::names::OwnedInterfaceName, PropMap>,
    name: &str,
) -> Option<&'a PropMap> {
    interfaces
        .iter()
        .find_map(|(k, v)| (k.as_str() == name).then_some(v))
}

/// The last path segment — the fallback identity when `BlueZ` names are absent.
fn path_tail(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

/// Fold the `BlueZ` object tree into [`BtStatus`]. Pure — unit-tested with
/// hand-built trees; tolerant of missing properties (§7: absent reads as the
/// honest default, never an invented value).
pub fn fold_bluez(objects: &zbus::fdo::ManagedObjects) -> BtStatus {
    let mut status = BtStatus::default();
    for (path, interfaces) in objects {
        let path_str = path.as_str();
        if let Some(props) = iface(interfaces, "org.bluez.Adapter1") {
            status.adapters.push(BtAdapter {
                path: path_str.to_owned(),
                name: str_prop(props, "Alias")
                    .or_else(|| str_prop(props, "Name"))
                    .unwrap_or_else(|| path_tail(path_str)),
                powered: bool_prop(props, "Powered").unwrap_or(false),
                discovering: bool_prop(props, "Discovering").unwrap_or(false),
            });
        }
        if let Some(props) = iface(interfaces, "org.bluez.Device1") {
            // A peripheral's charge rides the SAME object as Battery1.
            let battery =
                iface(interfaces, "org.bluez.Battery1").and_then(|b| u8_prop(b, "Percentage"));
            status.devices.push(BtDevice {
                path: path_str.to_owned(),
                alias: str_prop(props, "Alias")
                    .or_else(|| str_prop(props, "Name"))
                    .or_else(|| str_prop(props, "Address"))
                    .unwrap_or_else(|| path_tail(path_str)),
                paired: bool_prop(props, "Paired").unwrap_or(false),
                connected: bool_prop(props, "Connected").unwrap_or(false),
                trusted: bool_prop(props, "Trusted").unwrap_or(false),
                battery_percent: battery,
                icon: str_prop(props, "Icon"),
            });
        }
    }
    status.adapters.sort_by(|a, b| a.path.cmp(&b.path));
    status
        .devices
        .sort_by(|a, b| b.connected.cmp(&a.connected).then(a.alias.cmp(&b.alias)));
    status
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use zbus::names::{InterfaceName, OwnedInterfaceName};
    use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};

    use crate::props::testutil::{props, s};

    use super::*;

    fn opath(p: &str) -> OwnedObjectPath {
        ObjectPath::try_from(p).expect("valid object path").into()
    }

    fn oiface(i: &str) -> OwnedInterfaceName {
        InterfaceName::try_from(i)
            .expect("valid interface name")
            .into()
    }

    /// A `BlueZ` tree: one powered adapter, a connected keyboard reporting 87%
    /// charge, and an unconnected speaker — plus an unrelated object that must
    /// be ignored.
    fn tree() -> zbus::fdo::ManagedObjects {
        let mut objects: zbus::fdo::ManagedObjects = HashMap::new();

        let mut adapter: HashMap<OwnedInterfaceName, PropMap> = HashMap::new();
        adapter.insert(
            oiface("org.bluez.Adapter1"),
            props(vec![
                ("Alias", s("eagle")),
                ("Powered", OwnedValue::from(true)),
                ("Discovering", OwnedValue::from(false)),
            ]),
        );
        objects.insert(opath("/org/bluez/hci0"), adapter);

        let mut keyboard: HashMap<OwnedInterfaceName, PropMap> = HashMap::new();
        keyboard.insert(
            oiface("org.bluez.Device1"),
            props(vec![
                ("Alias", s("MX Keys")),
                ("Paired", OwnedValue::from(true)),
                ("Connected", OwnedValue::from(true)),
                ("Trusted", OwnedValue::from(true)),
                ("Icon", s("input-keyboard")),
            ]),
        );
        keyboard.insert(
            oiface("org.bluez.Battery1"),
            props(vec![("Percentage", OwnedValue::from(87_u8))]),
        );
        objects.insert(opath("/org/bluez/hci0/dev_AA_BB"), keyboard);

        let mut speaker: HashMap<OwnedInterfaceName, PropMap> = HashMap::new();
        speaker.insert(
            oiface("org.bluez.Device1"),
            props(vec![
                ("Alias", s("Anker Motion")),
                ("Paired", OwnedValue::from(true)),
                ("Connected", OwnedValue::from(false)),
            ]),
        );
        objects.insert(opath("/org/bluez/hci0/dev_CC_DD"), speaker);

        // BlueZ also exposes org.bluez.AgentManager1 etc. — not ours to fold.
        let mut agent: HashMap<OwnedInterfaceName, PropMap> = HashMap::new();
        agent.insert(oiface("org.bluez.AgentManager1"), props(vec![]));
        objects.insert(opath("/org/bluez"), agent);

        objects
    }

    #[test]
    fn folds_adapters_devices_and_peripheral_batteries() {
        let status = fold_bluez(&tree());

        assert_eq!(status.adapters.len(), 1);
        let a = &status.adapters[0];
        assert_eq!(a.path, "/org/bluez/hci0");
        assert_eq!(a.name, "eagle");
        assert!(a.powered);
        assert!(!a.discovering);
        assert!(status.any_adapter_powered());

        // Connected first, then by alias.
        assert_eq!(status.devices.len(), 2);
        assert_eq!(status.devices[0].alias, "MX Keys");
        assert!(status.devices[0].connected);
        assert_eq!(status.devices[0].battery_percent, Some(87));
        assert_eq!(status.devices[0].icon.as_deref(), Some("input-keyboard"));
        assert_eq!(status.devices[1].alias, "Anker Motion");
        assert!(!status.devices[1].connected);
        assert_eq!(
            status.devices[1].battery_percent, None,
            "no Battery1 ⇒ no invented charge"
        );
        assert_eq!(status.connected_devices(), 1);
    }

    #[test]
    fn missing_names_fall_back_to_the_path_tail_and_flags_to_false() {
        let mut objects: zbus::fdo::ManagedObjects = HashMap::new();
        let mut bare: HashMap<OwnedInterfaceName, PropMap> = HashMap::new();
        bare.insert(oiface("org.bluez.Adapter1"), props(vec![]));
        objects.insert(opath("/org/bluez/hci1"), bare);

        let status = fold_bluez(&objects);
        assert_eq!(status.adapters.len(), 1);
        assert_eq!(status.adapters[0].name, "hci1");
        assert!(!status.adapters[0].powered);
        assert!(!status.any_adapter_powered());
    }

    #[test]
    fn an_empty_tree_is_the_honest_empty_status() {
        let status = fold_bluez(&HashMap::new());
        assert_eq!(status, BtStatus::default());
        assert_eq!(status.connected_devices(), 0);
    }

    #[test]
    fn the_real_client_on_this_host_answers_typed_never_panics() {
        // Whatever the build host looks like (no bus, no bluetoothd, or a live
        // BlueZ), the production client must return Ok(status) or a typed
        // SeatError tagged Bluetooth — the §7 honest-probe contract.
        match ZbusBluez::new().status() {
            Ok(status) => {
                // A live answer is a real enumeration (possibly empty).
                let _ = status.connected_devices();
            }
            Err(e) => assert_eq!(e.backend(), Backend::Bluetooth),
        }
    }

    #[test]
    fn every_control_verb_on_this_host_answers_typed_never_panics() {
        // Same §7 contract for the write verbs: on a host with no adapter/bus
        // each must fold to a typed SeatError tagged Bluetooth, never a panic
        // and never a silent success.
        let c = ZbusBluez::new();
        let check = |r: Result<(), SeatError>| {
            if let Err(e) = r {
                assert_eq!(e.backend(), Backend::Bluetooth);
            }
        };
        check(c.set_adapter_powered("/org/bluez/hci0", true));
        check(c.set_discoverable("/org/bluez/hci0", true));
        check(c.set_pairable("/org/bluez/hci0", true));
        check(c.start_discovery("/org/bluez/hci0"));
        check(c.stop_discovery("/org/bluez/hci0"));
        check(c.pair("/org/bluez/hci0/dev_AA_BB"));
        check(c.cancel_pairing("/org/bluez/hci0/dev_AA_BB"));
        check(c.set_trusted("/org/bluez/hci0/dev_AA_BB", true));
        check(c.connect("/org/bluez/hci0/dev_AA_BB"));
        check(c.disconnect("/org/bluez/hci0/dev_AA_BB"));
        check(c.remove_device("/org/bluez/hci0", "/org/bluez/hci0/dev_AA_BB"));
    }

    #[test]
    fn remove_device_rejects_a_malformed_object_path_typed() {
        // A non-path device string is caught before any wire call — the honest
        // Protocol error, not a mangled D-Bus message.
        let e = ZbusBluez::new()
            .remove_device("/org/bluez/hci0", "not a path")
            .expect_err("a malformed path must be refused");
        assert!(matches!(e, SeatError::Protocol { .. }), "{e}");
        assert_eq!(e.backend(), Backend::Bluetooth);
    }

    fn dev(path: &str, paired: bool, connected: bool, trusted: bool) -> BtDevice {
        BtDevice {
            path: path.to_owned(),
            alias: path_tail(path),
            paired,
            connected,
            trusted,
            battery_percent: None,
            icon: None,
        }
    }

    #[test]
    fn scan_tracker_surfaces_each_unpaired_device_once() {
        let mut tracker = ScanTracker::new();

        // First poll of an empty scan surfaces nothing.
        assert!(tracker.poll(&BtStatus::default()).is_empty());

        // A speaker appears — surfaced once.
        let mut status = BtStatus {
            adapters: vec![],
            devices: vec![dev("/dev/spk", false, false, false)],
        };
        let fresh = tracker.poll(&status);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].path, "/dev/spk");

        // Same speaker on the next poll is NOT re-announced; a new headset is.
        status.devices.push(dev("/dev/hdst", false, false, false));
        let fresh = tracker.poll(&status);
        assert_eq!(fresh.len(), 1, "only the new device announces");
        assert_eq!(fresh[0].path, "/dev/hdst");

        // A reset re-arms every still-nearby device.
        tracker.reset();
        assert_eq!(tracker.poll(&status).len(), 2);
    }

    #[test]
    fn scan_tracker_never_announces_an_already_paired_device() {
        let mut tracker = ScanTracker::new();
        let status = BtStatus {
            adapters: vec![],
            devices: vec![
                dev("/dev/known", true, true, true),
                dev("/dev/new", false, false, false),
            ],
        };
        let fresh = tracker.poll(&status);
        assert_eq!(fresh.len(), 1);
        assert_eq!(
            fresh[0].path, "/dev/new",
            "paired devices are already known"
        );
    }

    #[test]
    fn reconnect_targets_are_trusted_and_disconnected_only() {
        let status = BtStatus {
            adapters: vec![],
            devices: vec![
                dev("/dev/kbd", true, false, true),    // trusted, offline → target
                dev("/dev/mouse", true, true, true),   // trusted but already up
                dev("/dev/rando", true, false, false), // offline but not trusted
            ],
        };
        assert_eq!(
            trusted_reconnect_targets(&status),
            vec!["/dev/kbd".to_owned()]
        );
    }

    /// A fake client: a canned enumeration + a recording of which devices got a
    /// `connect`, with a chosen device forced to fail — exercises the default
    /// `reconnect_trusted` drive logic headless.
    struct FakeBluez {
        status: BtStatus,
        connects: std::sync::Mutex<Vec<String>>,
        fail: Option<String>,
    }

    impl BluezClient for FakeBluez {
        fn status(&self) -> Result<BtStatus, SeatError> {
            Ok(self.status.clone())
        }
        fn connect(&self, device: &str) -> Result<(), SeatError> {
            self.connects.lock().unwrap().push(device.to_owned());
            if self.fail.as_deref() == Some(device) {
                return Err(SeatError::Backend {
                    backend: Backend::Bluetooth,
                    reason: "device off".into(),
                });
            }
            Ok(())
        }
        // The rest are unused by this test — honest no-ops that never lie Ok.
        fn set_adapter_powered(&self, _: &str, _: bool) -> Result<(), SeatError> {
            unreachable!()
        }
        fn set_discoverable(&self, _: &str, _: bool) -> Result<(), SeatError> {
            unreachable!()
        }
        fn set_pairable(&self, _: &str, _: bool) -> Result<(), SeatError> {
            unreachable!()
        }
        fn start_discovery(&self, _: &str) -> Result<(), SeatError> {
            unreachable!()
        }
        fn stop_discovery(&self, _: &str) -> Result<(), SeatError> {
            unreachable!()
        }
        fn pair(&self, _: &str) -> Result<(), SeatError> {
            unreachable!()
        }
        fn cancel_pairing(&self, _: &str) -> Result<(), SeatError> {
            unreachable!()
        }
        fn set_trusted(&self, _: &str, _: bool) -> Result<(), SeatError> {
            unreachable!()
        }
        fn disconnect(&self, _: &str) -> Result<(), SeatError> {
            unreachable!()
        }
        fn remove_device(&self, _: &str, _: &str) -> Result<(), SeatError> {
            unreachable!()
        }
    }

    #[test]
    fn reconnect_trusted_drives_connect_and_captures_each_outcome() {
        let fake = FakeBluez {
            status: BtStatus {
                adapters: vec![],
                devices: vec![
                    dev("/dev/kbd", true, false, true),  // reconnects OK
                    dev("/dev/spk", true, false, true),  // forced failure
                    dev("/dev/mouse", true, true, true), // already up — skipped
                ],
            },
            connects: std::sync::Mutex::new(vec![]),
            fail: Some("/dev/spk".to_owned()),
        };

        let attempts = fake.reconnect_trusted().expect("status read is fine");

        // Only the two trusted-and-offline devices were dialed.
        assert_eq!(
            *fake.connects.lock().unwrap(),
            vec!["/dev/kbd".to_owned(), "/dev/spk".to_owned()]
        );
        assert_eq!(attempts.len(), 2);
        assert!(attempts[0].connected && attempts[0].error.is_none());
        assert!(!attempts[1].connected);
        assert!(attempts[1].error.as_deref().unwrap().contains("Bluetooth"));
    }
}
