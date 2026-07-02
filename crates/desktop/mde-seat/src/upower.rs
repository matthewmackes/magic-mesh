//! The `UPower` client — battery enumeration over the system D-Bus (lock 6:
//! **multi-battery**, incl. UPSes and Bluetooth-peripheral batteries).
//!
//! `UPower`'s `EnumerateDevices` lists every power device it tracks; each is one
//! `org.freedesktop.UPower.Device` property bag. The fold from that bag into a
//! typed [`Battery`] is pure and unit-tested; line-power adjacents (the AC
//! adapter) and devices without a charge reading are skipped honestly rather
//! than rendered as fake batteries (§7).

use crate::bus::SysBus;
use crate::error::{Backend, SeatError};
use crate::props::{bool_prop, f64_prop, str_prop, u32_prop, PropMap};

/// The `UPower` well-known bus name (also the manager interface name).
const UPOWER: &str = "org.freedesktop.UPower";

/// What kind of power device a [`Battery`] is — folded from `UPower`'s `Type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryKind {
    /// An internal system battery (a laptop pack).
    Internal,
    /// An uninterruptible power supply.
    Ups,
    /// A peripheral's own battery (mouse, keyboard, headset, phone, …) with
    /// `UPower`'s device-class label.
    Peripheral(&'static str),
    /// A type code this fold does not know — shown as-is, never guessed.
    Unknown(u32),
}

impl BatteryKind {
    /// The operator-facing kind label.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Internal => "internal battery".to_owned(),
            Self::Ups => "UPS".to_owned(),
            Self::Peripheral(class) => (*class).to_owned(),
            Self::Unknown(code) => format!("power device (type {code})"),
        }
    }
}

/// Charging state — folded from `UPower`'s `State` code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryState {
    /// Charging.
    Charging,
    /// Discharging.
    Discharging,
    /// Empty.
    Empty,
    /// Fully charged.
    FullyCharged,
    /// Pending charge (held below full by policy).
    PendingCharge,
    /// Pending discharge.
    PendingDischarge,
    /// `UPower` reported no usable state.
    Unknown,
}

impl BatteryState {
    /// The operator-facing state label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Charging => "charging",
            Self::Discharging => "discharging",
            Self::Empty => "empty",
            Self::FullyCharged => "full",
            Self::PendingCharge => "pending charge",
            Self::PendingDischarge => "pending discharge",
            Self::Unknown => "state unknown",
        }
    }
}

/// One battery `UPower` tracks.
#[derive(Debug, Clone, PartialEq)]
pub struct Battery {
    /// The operator-facing name (`Model` → `NativePath` → the object path tail).
    pub model: String,
    /// What kind of device carries this battery.
    pub kind: BatteryKind,
    /// Charge percentage (0–100, as `UPower` reports it).
    pub percentage: f64,
    /// Charging state.
    pub state: BatteryState,
    /// Whether this battery powers the whole system (`PowerSupply`) — `false`
    /// for peripheral batteries.
    pub power_supply: bool,
}

/// The `UPower` client seam. Production impl: [`ZbusUPower`]; tests inject fakes.
pub trait UPowerClient: Send {
    /// Enumerate every battery `UPower` tracks.
    ///
    /// # Errors
    /// Typed: [`SeatError::Unavailable`] when `UPower` / the system bus is absent.
    fn batteries(&self) -> Result<Vec<Battery>, SeatError>;
}

/// The production `UPower` client: `EnumerateDevices`, then one `GetAll` per
/// device, folded by the pure [`fold_battery`].
pub struct ZbusUPower {
    bus: SysBus,
}

impl ZbusUPower {
    /// A client over the system bus. No I/O until the first call.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            bus: SysBus::new(Backend::UPower),
        }
    }
}

impl Default for ZbusUPower {
    fn default() -> Self {
        Self::new()
    }
}

impl UPowerClient for ZbusUPower {
    fn batteries(&self) -> Result<Vec<Battery>, SeatError> {
        let devices: Vec<zbus::zvariant::OwnedObjectPath> = self.bus.call(
            UPOWER,
            "/org/freedesktop/UPower",
            UPOWER,
            "EnumerateDevices",
            &(),
        )?;
        let mut out = Vec::new();
        for path in devices {
            let props: PropMap = self.bus.call(
                UPOWER,
                path.as_str(),
                "org.freedesktop.DBus.Properties",
                "GetAll",
                &("org.freedesktop.UPower.Device",),
            )?;
            if let Some(b) = fold_battery(path.as_str(), &props) {
                out.push(b);
            }
        }
        out.sort_by(|a, b| {
            b.power_supply
                .cmp(&a.power_supply)
                .then(a.model.cmp(&b.model))
        });
        Ok(out)
    }
}

/// `UPower` `Type` code → [`BatteryKind`]. `LinePower`/unset are not batteries.
const fn kind_from_type(code: u32) -> Option<BatteryKind> {
    Some(match code {
        0 | 1 => return None, // Unknown / LinePower — not a battery.
        2 => BatteryKind::Internal,
        3 => BatteryKind::Ups,
        4 => BatteryKind::Peripheral("monitor"),
        5 => BatteryKind::Peripheral("mouse"),
        6 => BatteryKind::Peripheral("keyboard"),
        7 => BatteryKind::Peripheral("PDA"),
        8 => BatteryKind::Peripheral("phone"),
        9 => BatteryKind::Peripheral("media player"),
        10 => BatteryKind::Peripheral("tablet"),
        11 => BatteryKind::Peripheral("computer"),
        12 => BatteryKind::Peripheral("gaming input"),
        13 => BatteryKind::Peripheral("pen"),
        14 => BatteryKind::Peripheral("touchpad"),
        15 => BatteryKind::Peripheral("modem"),
        16 => BatteryKind::Peripheral("network device"),
        17 => BatteryKind::Peripheral("headset"),
        18 => BatteryKind::Peripheral("speakers"),
        19 => BatteryKind::Peripheral("headphones"),
        20 => BatteryKind::Peripheral("video device"),
        21 => BatteryKind::Peripheral("audio device"),
        22 => BatteryKind::Peripheral("remote control"),
        23 => BatteryKind::Peripheral("printer"),
        24 => BatteryKind::Peripheral("scanner"),
        25 => BatteryKind::Peripheral("camera"),
        26 => BatteryKind::Peripheral("wearable"),
        27 => BatteryKind::Peripheral("toy"),
        28 => BatteryKind::Peripheral("Bluetooth device"),
        other => BatteryKind::Unknown(other),
    })
}

/// `UPower` `State` code → [`BatteryState`].
const fn state_from_code(code: u32) -> BatteryState {
    match code {
        1 => BatteryState::Charging,
        2 => BatteryState::Discharging,
        3 => BatteryState::Empty,
        4 => BatteryState::FullyCharged,
        5 => BatteryState::PendingCharge,
        6 => BatteryState::PendingDischarge,
        _ => BatteryState::Unknown,
    }
}

/// Fold one `UPower` device property bag into a [`Battery`]. Pure. Returns `None`
/// for non-batteries (line power), devices explicitly not present, and devices
/// without a charge reading — skipped honestly, never fabricated (§7).
pub fn fold_battery(path: &str, props: &PropMap) -> Option<Battery> {
    let kind = kind_from_type(u32_prop(props, "Type").unwrap_or(0))?;
    if bool_prop(props, "IsPresent") == Some(false) {
        return None;
    }
    let percentage = f64_prop(props, "Percentage")?;
    let model = str_prop(props, "Model")
        .filter(|m| !m.trim().is_empty())
        .or_else(|| str_prop(props, "NativePath").filter(|m| !m.trim().is_empty()))
        .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path).to_owned());
    Some(Battery {
        model,
        kind,
        percentage,
        state: state_from_code(u32_prop(props, "State").unwrap_or(0)),
        power_supply: bool_prop(props, "PowerSupply").unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use zbus::zvariant::OwnedValue;

    use crate::props::testutil::{props, s};

    use super::*;

    #[test]
    fn folds_an_internal_battery() {
        let bag = props(vec![
            ("Type", OwnedValue::from(2_u32)),
            ("Model", s("XPS 13 pack")),
            ("Percentage", OwnedValue::from(74.5_f64)),
            ("State", OwnedValue::from(1_u32)),
            ("IsPresent", OwnedValue::from(true)),
            ("PowerSupply", OwnedValue::from(true)),
        ]);
        let b = fold_battery("/org/freedesktop/UPower/devices/battery_BAT0", &bag)
            .expect("an internal battery folds");
        assert_eq!(b.model, "XPS 13 pack");
        assert_eq!(b.kind, BatteryKind::Internal);
        assert!((b.percentage - 74.5).abs() < f64::EPSILON);
        assert_eq!(b.state, BatteryState::Charging);
        assert!(b.power_supply);
        assert_eq!(b.state.label(), "charging");
    }

    #[test]
    fn folds_a_bluetooth_peripheral_battery() {
        // Lock 6: peripheral batteries (a BT mouse) are first-class.
        let bag = props(vec![
            ("Type", OwnedValue::from(5_u32)),
            ("Model", s("MX Master 3")),
            ("Percentage", OwnedValue::from(41.0_f64)),
            ("State", OwnedValue::from(2_u32)),
        ]);
        let b = fold_battery("/org/freedesktop/UPower/devices/mouse_x", &bag)
            .expect("a peripheral battery folds");
        assert_eq!(b.kind, BatteryKind::Peripheral("mouse"));
        assert_eq!(b.kind.label(), "mouse");
        assert!(!b.power_supply, "a peripheral never claims PowerSupply");
        assert_eq!(b.state, BatteryState::Discharging);
    }

    #[test]
    fn skips_line_power_absent_packs_and_chargeless_devices() {
        // The AC adapter is Type=1 LinePower — not a battery.
        let ac = props(vec![
            ("Type", OwnedValue::from(1_u32)),
            ("Percentage", OwnedValue::from(0.0_f64)),
        ]);
        assert_eq!(fold_battery("/u/line_power_AC", &ac), None);

        // A bay with no pack inserted (IsPresent=false) must not render.
        let absent = props(vec![
            ("Type", OwnedValue::from(2_u32)),
            ("Percentage", OwnedValue::from(0.0_f64)),
            ("IsPresent", OwnedValue::from(false)),
        ]);
        assert_eq!(fold_battery("/u/battery_BAT1", &absent), None);

        // No Percentage ⇒ nothing honest to show ⇒ skipped, not invented (§7).
        let mute = props(vec![("Type", OwnedValue::from(6_u32))]);
        assert_eq!(fold_battery("/u/keyboard_x", &mute), None);
    }

    #[test]
    fn unknown_type_codes_stay_typed_and_named() {
        let bag = props(vec![
            ("Type", OwnedValue::from(99_u32)),
            ("Percentage", OwnedValue::from(10.0_f64)),
        ]);
        let b = fold_battery("/u/mystery", &bag).expect("unknown kinds still fold");
        assert_eq!(b.kind, BatteryKind::Unknown(99));
        assert!(b.kind.label().contains("99"));
        // Model falls back to the object-path tail — a real identity.
        assert_eq!(b.model, "mystery");
        assert_eq!(b.state, BatteryState::Unknown);
    }

    #[test]
    fn the_real_client_on_this_host_answers_typed_never_panics() {
        match ZbusUPower::new().batteries() {
            Ok(batteries) => {
                for b in batteries {
                    assert!((0.0..=100.0).contains(&b.percentage), "{b:?}");
                }
            }
            Err(e) => assert_eq!(e.backend(), Backend::UPower),
        }
    }
}
