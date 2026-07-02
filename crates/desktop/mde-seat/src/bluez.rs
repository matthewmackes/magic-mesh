//! The BlueZ client — adapter + device enumeration over the system D-Bus.
//!
//! BlueZ publishes its whole world as one `ObjectManager` tree on `org.bluez`:
//! adapters (`org.bluez.Adapter1`), devices (`org.bluez.Device1`), and — for
//! peripherals that report charge — `org.bluez.Battery1` on the same device
//! object. One `GetManagedObjects` round-trip is therefore the entire read; the
//! fold from that tree into [`BtStatus`] is pure and unit-tested headless.
//!
//! E12-15 scope is **enumeration** (this snapshot feeds the System surface's
//! Bluetooth section + the chrome icon); the pairing agent, scan/pair/trust/
//! connect verbs and proximity announce are E12-17, layered on this client.

use crate::bus::SysBus;
use crate::error::{Backend, SeatError};
use crate::props::{bool_prop, str_prop, u8_prop, PropMap};

/// The BlueZ well-known bus name.
const BLUEZ: &str = "org.bluez";

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
    /// The BlueZ icon hint (`"input-keyboard"`, `"audio-headset"`, …), when set.
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

/// The BlueZ client seam. The production impl is [`ZbusBluez`]; tests (and the
/// shell's headless tests) inject a fake.
pub trait BluezClient: Send {
    /// Enumerate adapters + devices.
    ///
    /// # Errors
    /// Typed: [`SeatError::Unavailable`] when BlueZ / the system bus is absent.
    fn status(&self) -> Result<BtStatus, SeatError>;
}

/// The production BlueZ client — one `ObjectManager.GetManagedObjects` call,
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

/// The last path segment — the fallback identity when BlueZ names are absent.
fn path_tail(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

/// Fold the BlueZ object tree into [`BtStatus`]. Pure — unit-tested with
/// hand-built trees; tolerant of missing properties (§7: absent reads as the
/// honest default, never an invented value).
pub(crate) fn fold_bluez(objects: &zbus::fdo::ManagedObjects) -> BtStatus {
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
            let battery = iface(interfaces, "org.bluez.Battery1")
                .and_then(|b| u8_prop(b, "Percentage"));
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

    /// A BlueZ tree: one powered adapter, a connected keyboard reporting 87%
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
}
