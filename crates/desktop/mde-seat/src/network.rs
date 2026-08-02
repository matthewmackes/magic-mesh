//! Typed NetworkManager link observations and the bounded Wi-Fi radio control.
//!
//! This client deliberately stops at provider/link state. It never reads or
//! writes connection secrets, SSIDs, APNs, routes, or DNS settings. It may list
//! bounded profile labels/types for operator inventory; activation still requires
//! a separate credential-aware SecretAgent workflow.

use std::collections::HashMap;

use crate::bus::SysBus;
use crate::error::{Backend, SeatError};
use crate::props::{bool_prop, str_prop, u32_prop, PropMap};
use zbus::zvariant::OwnedObjectPath;

const NETWORK_MANAGER: &str = "org.freedesktop.NetworkManager";
const NETWORK_MANAGER_PATH: &str = "/org/freedesktop/NetworkManager";
const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const SETTINGS: &str = "org.freedesktop.NetworkManager.Settings";
const SETTINGS_CONNECTION: &str = "org.freedesktop.NetworkManager.Settings.Connection";
const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const DEVICE_CONTROL: &str = "org.freedesktop.NetworkManager.Device";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// Provider family for a NetworkManager device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkKind {
    /// NetworkManager Ethernet device.
    Ethernet,
    /// NetworkManager Wi-Fi device.
    Wifi,
    /// ModemManager-backed cellular device exposed through NetworkManager.
    Cellular,
}

/// Provider state normalized from NetworkManager's numeric device state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkState {
    /// No usable state was published.
    Unknown,
    /// Device is unavailable.
    Unavailable,
    /// Device is disconnected.
    Disconnected,
    /// Device is negotiating.
    Connecting,
    /// Device is activated.
    Connected,
}

/// Credential-free NetworkManager connection-profile family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkProfileKind {
    /// Wired Ethernet profile.
    Ethernet,
    /// Wi-Fi profile.
    Wifi,
    /// ModemManager cellular profile.
    Cellular,
    /// Imported WireGuard profile.
    Wireguard,
    /// Imported OpenVPN or another NetworkManager VPN profile.
    Vpn,
    /// A provider profile type not yet mapped by this client.
    Other,
}

impl NetworkProfileKind {
    fn from_nm(value: &str) -> Self {
        match value {
            "802-3-ethernet" => Self::Ethernet,
            "802-11-wireless" => Self::Wifi,
            "gsm" => Self::Cellular,
            "wireguard" => Self::Wireguard,
            "vpn" => Self::Vpn,
            _ => Self::Other,
        }
    }

    /// Operator-facing profile family label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ethernet => "Ethernet",
            Self::Wifi => "Wi-Fi",
            Self::Cellular => "Cellular",
            Self::Wireguard => "WireGuard",
            Self::Vpn => "VPN",
            Self::Other => "Other",
        }
    }
}

/// A bounded NetworkManager profile inventory record. The provider path is an
/// internal target; the visible label is sanitized and no profile settings,
/// UUIDs, SSIDs, APNs, or secrets cross this type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkProfile {
    /// NetworkManager Settings.Connection object path.
    pub path: String,
    /// Sanitized operator-facing profile label.
    pub label: String,
    /// Coarse provider profile family.
    pub kind: NetworkProfileKind,
}

impl NetworkState {
    fn from_nm(value: u32) -> Self {
        match value {
            20 => Self::Unavailable,
            30 => Self::Disconnected,
            40..=90 => Self::Connecting,
            100 => Self::Connected,
            _ => Self::Unknown,
        }
    }
}

/// A safe device/link observation. The object path is retained only as the
/// provider-issued mutation target; no profile or credential data is included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkLink {
    /// NetworkManager device object path.
    pub path: String,
    /// Kernel interface name.
    pub interface: String,
    /// Provider family.
    pub kind: NetworkKind,
    /// Current link state.
    pub state: NetworkState,
}

/// The bounded NetworkManager status consumed by the typed seat snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NetworkStatus {
    /// Recognized provider devices.
    pub links: Vec<NetworkLink>,
    /// NetworkManager's global Wi-Fi radio state, when the property exists.
    pub wifi_enabled: Option<bool>,
    /// Credential-free saved-profile inventory.
    pub profiles: Vec<NetworkProfile>,
}

/// NetworkManager observation and mutation seam.
pub trait NetworkClient: Send {
    /// Read recognized links and the global Wi-Fi radio state.
    fn status(&self) -> Result<NetworkStatus, SeatError>;
    /// Toggle only the global Wi-Fi radio; connection profiles are untouched.
    fn set_wifi_enabled(&self, enabled: bool) -> Result<(), SeatError>;
    /// Disconnect one provider-issued active device without selecting or
    /// mutating a connection profile.
    fn disconnect_link(&self, path: &str) -> Result<(), SeatError>;
    /// Activate a provider-issued saved profile. Required credentials are
    /// supplied only by a separately registered in-process SecretAgent.
    fn activate_profile(
        &self,
        profile_path: &str,
        device_path: Option<&str>,
    ) -> Result<String, SeatError>;
}

pub(crate) fn safe_settings_path(path: &str) -> bool {
    path.len() <= 160
        && path.starts_with("/org/freedesktop/NetworkManager/Settings/")
        && path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-'))
}

fn safe_profile_label(value: Option<String>) -> String {
    let Some(label) = value else {
        return "Unnamed profile".to_owned();
    };
    let trimmed = label.trim();
    if trimmed.is_empty()
        || trimmed.len() > 96
        || trimmed.chars().any(char::is_control)
        || trimmed.to_ascii_lowercase().contains("password")
        || trimmed.to_ascii_lowercase().contains("secret")
        || trimmed.to_ascii_lowercase().contains("passphrase")
    {
        return "Unnamed profile".to_owned();
    }
    trimmed.to_owned()
}

/// Fold the safe subset of NetworkManager Settings.Connection records.
#[must_use]
pub fn fold_profiles(
    records: impl IntoIterator<Item = (String, Option<String>, Option<String>)>,
) -> Vec<NetworkProfile> {
    let mut profiles = records
        .into_iter()
        .take(32)
        .filter(|(path, _, _)| safe_settings_path(path))
        .map(|(path, label, kind)| NetworkProfile {
            path,
            label: safe_profile_label(label),
            kind: NetworkProfileKind::from_nm(kind.as_deref().unwrap_or("")),
        })
        .collect::<Vec<_>>();
    profiles.sort_by(|left, right| left.label.cmp(&right.label).then_with(|| left.path.cmp(&right.path)));
    profiles
}

fn safe_device_path(path: &str) -> bool {
    path.len() <= 160
        && path.starts_with("/org/freedesktop/NetworkManager/Devices/")
        && path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-'))
}

/// Fold a NetworkManager ObjectManager tree plus its global radio property.
#[must_use]
pub fn fold_network(
    objects: &zbus::fdo::ManagedObjects,
    wifi_enabled: Option<bool>,
) -> NetworkStatus {
    let mut links = objects
        .iter()
        .filter_map(|(path, interfaces)| {
            if !safe_device_path(path.as_str()) {
                return None;
            }
            let props = interfaces.get(DEVICE)?;
            let kind = match u32_prop(props, "DeviceType")? {
                1 => NetworkKind::Ethernet,
                2 => NetworkKind::Wifi,
                8 => NetworkKind::Cellular,
                _ => return None,
            };
            let interface = str_prop(props, "Interface")?;
            if interface.is_empty()
                || interface.len() > 15
                || !interface
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
            {
                return None;
            }
            Some(NetworkLink {
                path: path.to_string(),
                interface,
                kind,
                state: NetworkState::from_nm(u32_prop(props, "State").unwrap_or_default()),
            })
        })
        .collect::<Vec<_>>();
    links.sort_by(|left, right| left.interface.cmp(&right.interface));
    NetworkStatus {
        links,
        wifi_enabled,
        profiles: Vec::new(),
    }
}

/// Production NetworkManager client over the standard system D-Bus service.
pub struct ZbusNetwork {
    bus: SysBus,
}

impl ZbusNetwork {
    /// Construct a lazy system-bus client.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            bus: SysBus::new(Backend::Network),
        }
    }
}

impl Default for ZbusNetwork {
    fn default() -> Self {
        Self::new()
    }
}

impl NetworkClient for ZbusNetwork {
    fn status(&self) -> Result<NetworkStatus, SeatError> {
        let objects: zbus::fdo::ManagedObjects = self.bus.call(
            NETWORK_MANAGER,
            "/",
            "org.freedesktop.DBus.ObjectManager",
            "GetManagedObjects",
            &(),
        )?;
        let props: PropMap = self.bus.call(
            NETWORK_MANAGER,
            NETWORK_MANAGER_PATH,
            PROPERTIES,
            "GetAll",
            &(NETWORK_MANAGER,)
        )?;
        let mut records = Vec::new();
        let paths: Vec<zbus::zvariant::OwnedObjectPath> = self.bus.call(
            NETWORK_MANAGER,
            SETTINGS_PATH,
            SETTINGS,
            "ListConnections",
            &(),
        )?;
        for path in paths {
            let path = path.to_string();
            if !safe_settings_path(&path) {
                continue;
            }
            let settings: HashMap<String, PropMap> = self.bus.call(
                NETWORK_MANAGER,
                &path,
                SETTINGS_CONNECTION,
                "GetSettings",
                &(),
            )?;
            let connection = settings.get("connection");
            records.push((
                path,
                connection.and_then(|props| str_prop(props, "id")),
                connection.and_then(|props| str_prop(props, "type")),
            ));
        }
        let mut status = fold_network(&objects, bool_prop(&props, "WirelessEnabled"));
        status.profiles = fold_profiles(records);
        Ok(status)
    }

    fn set_wifi_enabled(&self, enabled: bool) -> Result<(), SeatError> {
        self.bus.call_unit(
            NETWORK_MANAGER,
            NETWORK_MANAGER_PATH,
            PROPERTIES,
            "Set",
            &(
                NETWORK_MANAGER,
                "WirelessEnabled",
                zbus::zvariant::Value::from(enabled),
            ),
        )
    }

    fn disconnect_link(&self, path: &str) -> Result<(), SeatError> {
        if !safe_device_path(path) {
            return Err(SeatError::Protocol {
                backend: Backend::Network,
                reason: "NetworkManager device target is malformed".to_owned(),
            });
        }
        self.bus.call_unit(
            NETWORK_MANAGER,
            path,
            DEVICE_CONTROL,
            "Disconnect",
            &(),
        )
    }

    fn activate_profile(
        &self,
        profile_path: &str,
        device_path: Option<&str>,
    ) -> Result<String, SeatError> {
        if !safe_settings_path(profile_path) {
            return Err(SeatError::Protocol {
                backend: Backend::Network,
                reason: "NetworkManager profile target is malformed".to_owned(),
            });
        }
        let device_path = device_path.unwrap_or("/");
        if device_path != "/" && !safe_device_path(device_path) {
            return Err(SeatError::Protocol {
                backend: Backend::Network,
                reason: "NetworkManager device target is malformed".to_owned(),
            });
        }
        let profile = OwnedObjectPath::try_from(profile_path).map_err(|_| SeatError::Protocol {
            backend: Backend::Network,
            reason: "NetworkManager profile target is not an object path".to_owned(),
        })?;
        let device = OwnedObjectPath::try_from(device_path).map_err(|_| SeatError::Protocol {
            backend: Backend::Network,
            reason: "NetworkManager device target is not an object path".to_owned(),
        })?;
        let specific = OwnedObjectPath::try_from("/").expect("root is a valid object path");
        let active: OwnedObjectPath = self.bus.call(
            NETWORK_MANAGER,
            NETWORK_MANAGER_PATH,
            NETWORK_MANAGER,
            "ActivateConnection",
            &(profile, device, specific),
        )?;
        Ok(active.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use zbus::names::{InterfaceName, OwnedInterfaceName};
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Str};

    fn props(items: Vec<(&str, OwnedValue)>) -> PropMap {
        items.into_iter().map(|(key, value)| (key.to_owned(), value)).collect()
    }

    #[test]
    fn fold_network_keeps_only_safe_recognized_links() {
        let mut objects = zbus::fdo::ManagedObjects::new();
        let device = InterfaceName::try_from(DEVICE).unwrap();
        let path = OwnedObjectPath::try_from("/org/freedesktop/NetworkManager/Devices/1").unwrap();
        let mut interfaces = HashMap::new();
        interfaces.insert(
            OwnedInterfaceName::from(device),
            props(vec![
                ("DeviceType", OwnedValue::from(2_u32)),
                ("Interface", OwnedValue::from(Str::from("wlan0"))),
                ("State", OwnedValue::from(100_u32)),
            ]),
        );
        objects.insert(path, interfaces);
        let status = fold_network(&objects, Some(true));
        assert_eq!(status.wifi_enabled, Some(true));
        assert_eq!(status.links[0].kind, NetworkKind::Wifi);
        assert_eq!(status.links[0].state, NetworkState::Connected);
    }

    #[test]
    fn fold_network_rejects_credential_shaped_interface_names() {
        let mut objects = zbus::fdo::ManagedObjects::new();
        let device = InterfaceName::try_from(DEVICE).unwrap();
        let path = OwnedObjectPath::try_from("/org/freedesktop/NetworkManager/Devices/1").unwrap();
        let mut interfaces = HashMap::new();
        interfaces.insert(
            OwnedInterfaceName::from(device),
            props(vec![
                ("DeviceType", OwnedValue::from(2_u32)),
                ("Interface", OwnedValue::from(Str::from("password=hunter2"))),
                ("State", OwnedValue::from(100_u32)),
            ]),
        );
        objects.insert(path, interfaces);
        assert!(fold_network(&objects, None).links.is_empty());
    }

    #[test]
    fn device_targets_are_limited_to_networkmanager_device_paths() {
        assert!(safe_device_path(
            "/org/freedesktop/NetworkManager/Devices/7"
        ));
        assert!(!safe_device_path("/org/freedesktop/NetworkManager/ActiveConnection/7"));
        assert!(!safe_device_path("/tmp/credential-shaped-target"));
        assert!(!safe_device_path(
            "/org/freedesktop/NetworkManager/Devices/7;rm"
        ));
    }

    #[test]
    fn profile_inventory_is_bounded_and_credential_free() {
        let profiles = fold_profiles([
            (
                "/org/freedesktop/NetworkManager/Settings/2".to_owned(),
                Some("Office wired".to_owned()),
                Some("802-3-ethernet".to_owned()),
            ),
            (
                "/org/freedesktop/NetworkManager/Settings/1".to_owned(),
                Some("password=hunter2".to_owned()),
                Some("802-11-wireless".to_owned()),
            ),
            (
                "/tmp/not-a-settings-profile".to_owned(),
                Some("ignored".to_owned()),
                Some("vpn".to_owned()),
            ),
        ]);
        assert_eq!(profiles.len(), 2);
        let wifi = profiles
            .iter()
            .find(|profile| profile.kind == NetworkProfileKind::Wifi)
            .expect("wifi profile remains visible");
        assert_eq!(wifi.label, "Unnamed profile");
        let ethernet = profiles
            .iter()
            .find(|profile| profile.kind == NetworkProfileKind::Ethernet)
            .expect("ethernet profile remains visible");
        assert_eq!(ethernet.label, "Office wired");
    }

    #[test]
    fn profile_inventory_has_a_fixed_upper_bound() {
        let records = (0..64).map(|index| {
            (
                format!("/org/freedesktop/NetworkManager/Settings/{index}"),
                Some(format!("profile-{index}")),
                Some("vpn".to_owned()),
            )
        });
        assert_eq!(fold_profiles(records).len(), 32);
    }

    #[test]
    fn activation_targets_reject_profile_and_device_path_injection() {
        assert!(safe_settings_path(
            "/org/freedesktop/NetworkManager/Settings/4"
        ));
        assert!(!safe_settings_path("/tmp/profile"));
        assert!(safe_device_path(
            "/org/freedesktop/NetworkManager/Devices/4"
        ));
        assert!(!safe_device_path("/org/freedesktop/NetworkManager/Settings/4"));
    }
}
