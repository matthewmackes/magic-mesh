//! Bounded, read-only local hardware observations.
//!
//! This provider deliberately has no mutation verbs. It reads only the fixed
//! kernel class roots for thermal zones, hwmon fan inputs, and block-device
//! inventory. Shared snapshots keep only aggregate storage facts; detailed
//! device identities stay inside this trusted local seat provider.

use std::fs;
use std::path::Path;

use crate::error::{Backend, SeatError};

const THERMAL_ROOT: &str = "/sys/class/thermal";
const HWMON_ROOT: &str = "/sys/class/hwmon";
const BLOCK_ROOT: &str = "/sys/block";
const DMI_ROOT: &str = "/sys/class/dmi/id";
const THUNDERBOLT_ROOT: &str = "/sys/bus/thunderbolt/devices";
const PLATFORM_PROFILE: &str = "/sys/firmware/acpi/platform_profile";
const PLATFORM_PROFILES: &str = "/sys/firmware/acpi/platform_profile_choices";
const MAX_ZONES: usize = 16;
const MAX_FANS: u8 = 32;
const MAX_LABEL_CHARS: usize = 64;
const MAX_PROFILE_CHARS: usize = 32;
const MAX_PROFILES: usize = 8;
const MAX_STORAGE_DEVICES: usize = 32;
const MAX_DEVICE_NAME_CHARS: usize = 64;
const SECTOR_BYTES: u64 = 512;
const MAX_THUNDERBOLT_DEVICES: usize = 16;

/// One thermal-zone observation. The kernel path and device identity never
/// cross this typed boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThermalZone {
    /// A bounded kernel-provided type label, or a stable generic label.
    pub label: String,
    /// Millidegrees Celsius when the provider returned a bounded integer.
    pub temperature_milli_c: Option<i32>,
}

/// A bounded local block-device observation. This is intentionally available
/// only through the trusted seat snapshot; the world-readable mesh projection
/// continues to publish aggregate count/capacity/removable facts only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageDevice {
    /// Kernel block-device name, bounded to a safe display label.
    pub name: String,
    /// Capacity derived from the kernel's 512-byte sector count.
    pub size_bytes: Option<u64>,
    /// Whether the kernel marks this device removable.
    pub removable: bool,
    /// Whether the kernel reports rotational media.
    pub rotational: Option<bool>,
}

/// Bounded local firmware identity from the fixed DMI class root.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FirmwareStatus {
    /// Product label, when the kernel publishes one.
    pub product_name: Option<String>,
    /// Firmware/BIOs version, when the kernel publishes one.
    pub bios_version: Option<String>,
}

/// A bounded Thunderbolt/dock observation from the kernel device class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThunderboltDevice {
    /// Kernel-provided device label, sanitized for local display.
    pub name: String,
    /// Kernel authorization state, when the device publishes one.
    pub authorized: Option<bool>,
}

/// Aggregate local hardware observations used by the This Node Hardware page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HardwareStatus {
    /// Unix epoch milliseconds when this bounded provider observation was read.
    /// Zero means the fixture/provider did not supply a timestamp.
    pub observed_at_ms: u64,
    /// At most the first sixteen thermal zones in stable directory order.
    pub thermal_zones: Vec<ThermalZone>,
    /// Count of bounded `fan*_input` readings, not their names or paths.
    pub fan_count: u8,
    /// Bounded local block-device inventory for the trusted detail view.
    pub storage_devices: Vec<StorageDevice>,
    /// Fixed-root firmware identity, kept local to the trusted seat.
    pub firmware: Option<FirmwareStatus>,
    /// Bounded Thunderbolt/dock inventory, kept local to the trusted seat.
    pub thunderbolt_devices: Vec<ThunderboltDevice>,
    /// Current standard kernel platform profile, when advertised.
    pub platform_profile: Option<String>,
    /// Bounded standard kernel platform-profile choices.
    pub platform_profile_choices: Vec<String>,
}

/// Typed seam for local read-only hardware observation.
pub trait HardwareClient: Send + Sync {
    /// Read bounded thermal/fan facts.
    fn status(&self) -> Result<HardwareStatus, SeatError>;

    /// Apply one provider-advertised standard kernel platform profile.
    fn set_platform_profile(&self, profile: &str) -> Result<(), SeatError>;
}

/// Production provider over fixed kernel class roots.
#[derive(Debug, Clone, Copy, Default)]
pub struct SysfsHardware;

impl SysfsHardware {
    /// Construct the fixed-root provider.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl HardwareClient for SysfsHardware {
    fn status(&self) -> Result<HardwareStatus, SeatError> {
        let thermal = fs::read_dir(THERMAL_ROOT).ok();
        let hwmon = fs::read_dir(HWMON_ROOT).ok();
        let platform_profile = read_profile(PLATFORM_PROFILE);
        let platform_profile_choices = read_profile_choices(PLATFORM_PROFILES);
        let storage = read_storage_devices();
        let firmware = read_firmware_status();
        let thunderbolt_devices = read_thunderbolt_devices();
        if thermal.is_none()
            && hwmon.is_none()
            && storage.is_empty()
            && firmware.is_none()
            && thunderbolt_devices.is_empty()
            && platform_profile.is_none()
            && platform_profile_choices.is_empty()
        {
            return Err(SeatError::Unavailable {
                backend: Backend::Hardware,
                reason: "thermal and hwmon class roots are unavailable".to_owned(),
            });
        }

        let mut zones = Vec::new();
        if let Some(entries) = thermal {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if !name.starts_with("thermal_zone") || zones.len() >= MAX_ZONES {
                    continue;
                }
                let root = entry.path();
                let label = read_label(&root.join("type"))
                    .unwrap_or_else(|| format!("Thermal zone {}", zones.len() + 1));
                let temperature_milli_c = read_bounded_temperature(&root.join("temp"));
                zones.push(ThermalZone {
                    label,
                    temperature_milli_c,
                });
            }
        }

        let mut fan_count: u8 = 0;
        if let Some(entries) = hwmon {
            'devices: for entry in entries.flatten() {
                let Ok(inputs) = fs::read_dir(entry.path()) else {
                    continue;
                };
                for input in inputs.flatten() {
                    let name = input.file_name();
                    let name = name.to_string_lossy();
                    if name.starts_with("fan") && name.ends_with("_input") {
                        fan_count = fan_count.saturating_add(1);
                        if fan_count == MAX_FANS {
                            break 'devices;
                        }
                    }
                }
            }
        }

        Ok(HardwareStatus {
            observed_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .and_then(|duration| u64::try_from(duration.as_millis()).ok())
                .unwrap_or(0),
            thermal_zones: zones,
            fan_count,
            storage_devices: storage,
            firmware,
            thunderbolt_devices,
            platform_profile,
            platform_profile_choices,
        })
    }

    fn set_platform_profile(&self, profile: &str) -> Result<(), SeatError> {
        if bounded_profile(profile).as_deref() != Some(profile) {
            return Err(SeatError::Protocol {
                backend: Backend::Hardware,
                reason: "platform profile name is malformed".to_owned(),
            });
        }
        let status = self.status()?;
        if !status
            .platform_profile_choices
            .iter()
            .any(|choice| choice == profile)
        {
            return Err(SeatError::Unavailable {
                backend: Backend::Hardware,
                reason: "platform profile is not advertised by the kernel provider".to_owned(),
            });
        }
        fs::write(PLATFORM_PROFILE, profile).map_err(|error| SeatError::Backend {
            backend: Backend::Hardware,
            reason: format!("platform profile write refused: {error}"),
        })
    }
}

fn read_firmware_status() -> Option<FirmwareStatus> {
    let product_name = read_label(&Path::new(DMI_ROOT).join("product_name"));
    let bios_version = read_label(&Path::new(DMI_ROOT).join("bios_version"));
    (product_name.is_some() || bios_version.is_some()).then_some(FirmwareStatus {
        product_name,
        bios_version,
    })
}

fn read_thunderbolt_devices() -> Vec<ThunderboltDevice> {
    let Ok(entries) = fs::read_dir(THUNDERBOLT_ROOT) else {
        return Vec::new();
    };
    let mut devices = entries
        .flatten()
        .filter_map(|entry| {
            let raw_name = entry.file_name();
            let name = bounded_device_name(raw_name.to_str()?.trim())?;
            let authorized = read_bool_flag(&entry.path().join("authorized"));
            Some(ThunderboltDevice { name, authorized })
        })
        .take(MAX_THUNDERBOLT_DEVICES)
        .collect::<Vec<_>>();
    devices.sort_by(|left, right| left.name.cmp(&right.name));
    devices
}

fn read_storage_devices() -> Vec<StorageDevice> {
    let Ok(entries) = fs::read_dir(BLOCK_ROOT) else {
        return Vec::new();
    };
    let mut devices = entries
        .flatten()
        .filter_map(|entry| {
            let raw_name = entry.file_name();
            let name = raw_name.to_str()?.trim();
            let name = bounded_device_name(name)?;
            let root = entry.path();
            let sectors = fs::read_to_string(root.join("size"))
                .ok()
                .and_then(|value| value.trim().parse::<u64>().ok());
            let size_bytes = sectors.and_then(|value| value.checked_mul(SECTOR_BYTES));
            let removable = read_bool_flag(&root.join("removable")).unwrap_or(false);
            let rotational = read_bool_flag(&root.join("queue/rotational"));
            Some(StorageDevice {
                name,
                size_bytes,
                removable,
                rotational,
            })
        })
        .take(MAX_STORAGE_DEVICES)
        .collect::<Vec<_>>();
    devices.sort_by(|left, right| left.name.cmp(&right.name));
    devices
}

fn read_bool_flag(path: &Path) -> Option<bool> {
    match fs::read_to_string(path).ok()?.trim() {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

fn bounded_device_name(value: &str) -> Option<String> {
    if value.is_empty()
        || value.len() > MAX_DEVICE_NAME_CHARS
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return None;
    }
    Some(value.to_owned())
}

fn read_label(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    bounded_label(raw.trim())
}

fn bounded_label(value: &str) -> Option<String> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }
    let bounded: String = value.chars().take(MAX_LABEL_CHARS).collect();
    (!bounded.is_empty()).then_some(bounded)
}

fn read_bounded_temperature(path: &Path) -> Option<i32> {
    parse_bounded_temperature(fs::read_to_string(path).ok()?.trim())
}

fn parse_bounded_temperature(value: &str) -> Option<i32> {
    let value = value.parse::<i32>().ok()?;
    (value >= -100_000 && value <= 200_000).then_some(value)
}

fn read_profile(path: &str) -> Option<String> {
    let value = fs::read_to_string(path).ok()?;
    bounded_profile(value.trim())
}

fn read_profile_choices(path: &str) -> Vec<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| {
            value
                .split_whitespace()
                .take(MAX_PROFILES)
                .filter_map(bounded_profile)
                .collect()
        })
        .unwrap_or_default()
}

fn bounded_profile(value: &str) -> Option<String> {
    if value.is_empty()
        || value.chars().any(char::is_control)
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return None;
    }
    let bounded: String = value.chars().take(MAX_PROFILE_CHARS).collect();
    (!bounded.is_empty()).then_some(bounded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_is_read_only_and_bounded_on_the_current_host() {
        let result = SysfsHardware::new().status();
        if let Ok(status) = result {
            assert!(status.observed_at_ms > 0);
            assert!(status.thermal_zones.len() <= MAX_ZONES);
            assert!(status.fan_count <= MAX_FANS);
            assert!(status.storage_devices.len() <= MAX_STORAGE_DEVICES);
            assert!(status.thunderbolt_devices.len() <= MAX_THUNDERBOLT_DEVICES);
            assert!(status
                .thunderbolt_devices
                .iter()
                .all(|device| device.name.len() <= MAX_DEVICE_NAME_CHARS));
            assert!(status
                .thermal_zones
                .iter()
                .all(|zone| zone.label.len() <= MAX_LABEL_CHARS));
        }
    }

    #[test]
    fn hostile_sensor_values_fail_closed() {
        assert_eq!(parse_bounded_temperature("not-a-temperature"), None);
        assert_eq!(parse_bounded_temperature("200001"), None);
        assert_eq!(parse_bounded_temperature("-100001"), None);
        assert_eq!(parse_bounded_temperature("42000"), Some(42000));
        assert_eq!(bounded_label(""), None);
        assert_eq!(bounded_label("sensor\nsecret"), None);
        assert_eq!(bounded_label("CPU package"), Some("CPU package".to_owned()));
        assert_eq!(bounded_profile("balanced"), Some("balanced".to_owned()));
        assert_eq!(bounded_profile("high performance"), None);
        assert_eq!(bounded_profile("quiet\nsecret"), None);
        assert_eq!(bounded_device_name("../../secret"), None);
        assert_eq!(bounded_device_name("nvme0n1"), Some("nvme0n1".to_owned()));
        assert_eq!(read_bool_flag(Path::new("/definitely/missing")), None);
    }

    #[test]
    fn profile_write_rejects_untyped_or_unadvertised_targets_before_io() {
        let provider = SysfsHardware::new();
        let malformed = provider.set_platform_profile("/sys/firmware/acpi/platform_profile");
        assert!(matches!(
            malformed,
            Err(SeatError::Protocol {
                backend: Backend::Hardware,
                ..
            })
        ));

        let unknown = provider.set_platform_profile("mde-test-profile-not-advertised");
        assert!(matches!(
            unknown,
            Err(SeatError::Unavailable {
                backend: Backend::Hardware,
                ..
            })
        ));
    }
}
