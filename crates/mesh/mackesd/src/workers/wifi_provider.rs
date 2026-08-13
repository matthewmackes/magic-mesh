//! Credential-free Wi-Fi state provider for WL-UX-011.
//!
//! NetworkManager owns connection readiness; the kernel owns whether wireless
//! interfaces actually exist.  This provider requires those sources to agree
//! before publishing a healthy state. It never scans SSIDs, reads profiles or
//! secrets, or exposes mutation authority.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_INTERFACES: usize = 64;

/// Truthful readiness of the node's Wi-Fi provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WifiReadiness {
    /// NetworkManager and kernel facts agree that a Wi-Fi link is connected.
    Ready,
    /// Wi-Fi is present and enabled but no interface is connected.
    Disconnected,
    /// NetworkManager explicitly reports the Wi-Fi radio disabled, or the
    /// kernel exposes no wireless interface and NetworkManager exposes no Wi-Fi device.
    Disabled,
    /// A required source failed, was malformed, or contradicted another source.
    Unknown,
}

/// Bounded, credential-free provider projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WifiSnapshot {
    pub schema_version: u16,
    pub node_id: String,
    pub observed_unix_ms: u64,
    pub readiness: WifiReadiness,
    pub interfaces: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RadioState {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NmDevice {
    name: String,
    connected: bool,
}

fn parse_radio(raw: &str) -> Option<RadioState> {
    match raw.trim() {
        "enabled" => Some(RadioState::Enabled),
        "disabled" => Some(RadioState::Disabled),
        _ => None,
    }
}

fn parse_devices(raw: &str) -> Option<Vec<NmDevice>> {
    let mut devices = Vec::new();
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let mut fields = line.splitn(4, ':');
        let (Some(name), Some(kind), Some(state), Some(_connection)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return None;
        };
        if name.is_empty() || name.len() > 128 {
            return None;
        }
        if kind != "wifi" {
            continue;
        }
        let connected = match state {
            "connected" => true,
            "disconnected" | "unavailable" => false,
            _ => return None,
        };
        devices.push(NmDevice {
            name: name.to_owned(),
            connected,
        });
        if devices.len() > MAX_INTERFACES {
            return None;
        }
    }
    devices.sort_unstable_by(|left, right| left.name.cmp(&right.name));
    if devices.windows(2).any(|pair| pair[0].name == pair[1].name) {
        return None;
    }
    Some(devices)
}

fn classify(
    radio: Option<RadioState>,
    devices: Option<Vec<NmDevice>>,
    mut kernel_interfaces: Vec<String>,
) -> (WifiReadiness, Vec<String>, &'static str) {
    kernel_interfaces.sort_unstable();
    kernel_interfaces.dedup();
    if kernel_interfaces.len() > MAX_INTERFACES {
        return (
            WifiReadiness::Unknown,
            vec![],
            "kernel Wi-Fi inventory exceeded bound",
        );
    }
    let (Some(radio), Some(devices)) = (radio, devices) else {
        return (
            WifiReadiness::Unknown,
            vec![],
            "NetworkManager Wi-Fi facts unavailable or malformed",
        );
    };
    let nm_names = devices
        .iter()
        .map(|device| device.name.clone())
        .collect::<Vec<_>>();
    if nm_names != kernel_interfaces {
        if nm_names.is_empty() && kernel_interfaces.is_empty() {
            return (
                WifiReadiness::Disabled,
                vec![],
                "no Wi-Fi hardware is exposed",
            );
        }
        return (
            WifiReadiness::Unknown,
            vec![],
            "NetworkManager and kernel Wi-Fi inventories disagree",
        );
    }
    match radio {
        RadioState::Disabled if devices.iter().any(|device| device.connected) => (
            WifiReadiness::Unknown,
            nm_names,
            "disabled radio contradicts connected device",
        ),
        RadioState::Disabled => (WifiReadiness::Disabled, nm_names, "Wi-Fi radio is disabled"),
        RadioState::Enabled if devices.iter().any(|device| device.connected) => {
            (WifiReadiness::Ready, nm_names, "Wi-Fi link is connected")
        }
        RadioState::Enabled => (
            WifiReadiness::Disconnected,
            nm_names,
            "Wi-Fi is enabled without a connected link",
        ),
    }
}

fn kernel_interfaces(root: &Path) -> std::io::Result<Vec<String>> {
    let mut interfaces = std::fs::read_dir(root)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("wireless").is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect::<Vec<_>>();
    interfaces.sort_unstable();
    if interfaces.len() > MAX_INTERFACES {
        interfaces.truncate(MAX_INTERFACES + 1);
    }
    Ok(interfaces)
}

fn nmcli(args: &[&str]) -> Option<String> {
    let mut command = std::process::Command::new("nmcli");
    command.args(args);
    let output = super::proc::output_with_timeout(command, COMMAND_TIMEOUT).ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())
        .flatten()
}

/// Gather current state from NetworkManager and `/sys/class/net`.
#[must_use]
pub fn gather(node_id: &str) -> WifiSnapshot {
    let radio = nmcli(&["-t", "-f", "WIFI", "general"])
        .as_deref()
        .and_then(parse_radio);
    let devices = nmcli(&["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device"])
        .as_deref()
        .and_then(parse_devices);
    let kernel = kernel_interfaces(Path::new("/sys/class/net")).ok();
    let (readiness, interfaces, reason) = match kernel {
        Some(kernel) => classify(radio, devices, kernel),
        None => (
            WifiReadiness::Unknown,
            vec![],
            "kernel Wi-Fi facts unavailable",
        ),
    };
    WifiSnapshot {
        schema_version: 1,
        node_id: node_id.to_owned(),
        observed_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
        readiness,
        interfaces,
        reason: reason.to_owned(),
    }
}

/// Stable replicated projection path consumed by Workers hardware views.
#[must_use]
pub fn snapshot_path(workgroup_root: &Path, node_id: &str) -> PathBuf {
    workgroup_root
        .join("wifi-provider")
        .join(format!("{node_id}.json"))
}

/// Publish one current Wi-Fi observation atomically.
pub fn publish_system(workgroup_root: &Path, node_id: &str) -> std::io::Result<PathBuf> {
    let path = snapshot_path(workgroup_root, node_id);
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("Wi-Fi snapshot has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".{node_id}.json.tmp"));
    let bytes = serde_json::to_vec_pretty(&gather(node_id)).map_err(std::io::Error::other)?;
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_or_contradictory_wifi_facts_fail_unknown() {
        let malformed = classify(
            parse_radio("enabled"),
            parse_devices("wlan0:wifi:connected\n"),
            vec!["wlan0".into()],
        );
        assert_eq!(malformed.0, WifiReadiness::Unknown);

        let contradiction = classify(
            parse_radio("disabled"),
            parse_devices("wlan0:wifi:connected:home"),
            vec!["wlan0".into()],
        );
        assert_eq!(contradiction.0, WifiReadiness::Unknown);

        let substituted_inventory = classify(
            parse_radio("enabled"),
            parse_devices("wlan1:wifi:connected:home"),
            vec!["wlan0".into()],
        );
        assert_eq!(substituted_inventory.0, WifiReadiness::Unknown);
        assert!(substituted_inventory.1.is_empty());
    }

    #[test]
    fn explicit_radio_and_link_states_remain_distinct() {
        let disabled = classify(
            parse_radio("disabled"),
            parse_devices("wlan0:wifi:unavailable:--"),
            vec!["wlan0".into()],
        );
        assert_eq!(disabled.0, WifiReadiness::Disabled);

        let disconnected = classify(
            parse_radio("enabled"),
            parse_devices("wlan0:wifi:disconnected:--"),
            vec!["wlan0".into()],
        );
        assert_eq!(disconnected.0, WifiReadiness::Disconnected);

        let ready = classify(
            parse_radio("enabled"),
            parse_devices("wlan0:wifi:connected:home"),
            vec!["wlan0".into()],
        );
        assert_eq!(ready.0, WifiReadiness::Ready);
    }
}
