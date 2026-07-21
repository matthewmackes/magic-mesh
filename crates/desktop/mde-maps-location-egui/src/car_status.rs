//! Auto Mode driver's-strip status catalog (the selectable readouts below the
//! speedometer in the left instrument strip).
//!
//! [`CarStatusItem`] is a ~50-entry catalog of live vehicle / connectivity /
//! location readouts, each resolvable to a `(label, value)` from the live
//! [`MapsLocationSurface`] fold (the MG90 mirror). [`CarStatusSelection`] is the
//! operator's chosen, persisted subset shown in the strip — a driver taps a tile
//! to cycle it to the next catalog entry, so any readout can occupy any slot.

use serde::{Deserialize, Serialize};

use crate::MapsLocationSurface;

/// The file (under the client data dir) the strip selection persists to.
const CAR_STATUS_CONFIG_FILE: &str = "settings-car-status.json";

/// One selectable readout in the driver's status strip. Serialized by its
/// `snake_case` name so the persisted selection is stable across reordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CarStatusItem {
    // ── Motion / drivetrain ──────────────────────────────────────────────
    SpeedMph,
    SpeedKph,
    Heading,
    HeadingCardinal,
    Rpm,
    CoolantC,
    CoolantF,
    BatteryV,
    FuelPercent,
    OdometerMi,
    RuntimeMin,
    Ignition,
    Moving,
    FaultCodes,
    // ── GPS / location ───────────────────────────────────────────────────
    GpsFix,
    Satellites,
    AccuracyM,
    Latitude,
    Longitude,
    AltitudeM,
    AltitudeFt,
    UpdateRate,
    FixAge,
    LocationSource,
    // ── Connectivity (MG90 WAN) ──────────────────────────────────────────
    ActiveWan,
    CellASignal,
    CellABars,
    CellACarrier,
    CellATech,
    CellASim,
    CellAIp,
    CellAHealth,
    CellBSignal,
    CellBCarrier,
    CellBTech,
    CellBSim,
    CellBHealth,
    WifiState,
    EthernetState,
    VpnState,
    LinkQuality,
    LatencyMs,
    PacketLoss,
    Failovers,
    DataUsed,
    // ── Telematics / meta ────────────────────────────────────────────────
    TelemetrySource,
    TelemetryAge,
    // ── Navigation ───────────────────────────────────────────────────────
    NavEta,
    NavSummary,
}

impl CarStatusItem {
    /// The full catalog, in menu order (grouped by domain).
    pub const ALL: [Self; 48] = [
        Self::SpeedMph,
        Self::SpeedKph,
        Self::Heading,
        Self::HeadingCardinal,
        Self::Rpm,
        Self::CoolantC,
        Self::CoolantF,
        Self::BatteryV,
        Self::FuelPercent,
        Self::OdometerMi,
        Self::RuntimeMin,
        Self::Ignition,
        Self::Moving,
        Self::FaultCodes,
        Self::GpsFix,
        Self::Satellites,
        Self::AccuracyM,
        Self::Latitude,
        Self::Longitude,
        Self::AltitudeM,
        Self::AltitudeFt,
        Self::UpdateRate,
        Self::FixAge,
        Self::LocationSource,
        Self::ActiveWan,
        Self::CellASignal,
        Self::CellABars,
        Self::CellACarrier,
        Self::CellATech,
        Self::CellASim,
        Self::CellAIp,
        Self::CellAHealth,
        Self::CellBSignal,
        Self::CellBCarrier,
        Self::CellBTech,
        Self::CellBSim,
        Self::CellBHealth,
        Self::WifiState,
        Self::EthernetState,
        Self::VpnState,
        Self::LinkQuality,
        Self::LatencyMs,
        Self::PacketLoss,
        Self::Failovers,
        Self::DataUsed,
        Self::TelemetrySource,
        Self::TelemetryAge,
        Self::NavEta,
    ];

    /// The short label shown above the value in the strip tile.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::SpeedMph => "SPEED",
            Self::SpeedKph => "SPEED KPH",
            Self::Heading => "HEADING",
            Self::HeadingCardinal => "COMPASS",
            Self::Rpm => "RPM",
            Self::CoolantC => "COOLANT",
            Self::CoolantF => "COOLANT F",
            Self::BatteryV => "BATTERY",
            Self::FuelPercent => "FUEL",
            Self::OdometerMi => "ODOMETER",
            Self::RuntimeMin => "RUNTIME",
            Self::Ignition => "IGNITION",
            Self::Moving => "MOTION",
            Self::FaultCodes => "FAULTS",
            Self::GpsFix => "GPS FIX",
            Self::Satellites => "SATS",
            Self::AccuracyM => "ACCURACY",
            Self::Latitude => "LAT",
            Self::Longitude => "LON",
            Self::AltitudeM => "ALTITUDE",
            Self::AltitudeFt => "ALT FT",
            Self::UpdateRate => "GPS RATE",
            Self::FixAge => "FIX AGE",
            Self::LocationSource => "SOURCE",
            Self::ActiveWan => "WAN",
            Self::CellASignal => "CELL A",
            Self::CellABars => "SIGNAL A",
            Self::CellACarrier => "CARRIER A",
            Self::CellATech => "TECH A",
            Self::CellASim => "SIM A",
            Self::CellAIp => "IP A",
            Self::CellAHealth => "LINK A",
            Self::CellBSignal => "CELL B",
            Self::CellBCarrier => "CARRIER B",
            Self::CellBTech => "TECH B",
            Self::CellBSim => "SIM B",
            Self::CellBHealth => "LINK B",
            Self::WifiState => "WI-FI",
            Self::EthernetState => "ETHERNET",
            Self::VpnState => "VPN",
            Self::LinkQuality => "QUALITY",
            Self::LatencyMs => "LATENCY",
            Self::PacketLoss => "LOSS",
            Self::Failovers => "FAILOVERS",
            Self::DataUsed => "DATA",
            Self::TelemetrySource => "TELEMETRY",
            Self::TelemetryAge => "TELEM AGE",
            Self::NavEta => "ETA",
            Self::NavSummary => "NAV",
        }
    }

    /// Resolve the live display value from the surface fold. Honest empty ("—")
    /// when the source is not present — never fabricated.
    #[must_use]
    pub fn value(self, s: &MapsLocationSurface) -> String {
        let t = &s.vehicle.telemetry;
        let w = &s.mg90.status;
        let sample = s.locations.primary_source().map(|src| &src.sample);
        let g = |f: fn(&crate::model::LocationSample) -> String| {
            sample.map_or_else(|| "—".to_string(), |x| f(x))
        };
        match self {
            Self::SpeedMph => format!("{:.0} mph", t.speed_mph),
            Self::SpeedKph => format!("{:.0} kph", t.speed_mph * 1.60934),
            Self::Heading => g(|x| format!("{:.0}°", x.heading_deg)),
            Self::HeadingCardinal => g(|x| cardinal(x.heading_deg).to_string()),
            Self::Rpm => format!("{}", t.rpm),
            Self::CoolantC => format!("{:.0} °C", t.coolant_c),
            Self::CoolantF => format!("{:.0} °F", t.coolant_c * 9.0 / 5.0 + 32.0),
            Self::BatteryV => format!("{:.1} V", t.battery_v),
            Self::FuelPercent => t
                .fuel_percent
                .map_or_else(|| "—".to_string(), |f| format!("{f:.0}%")),
            Self::OdometerMi => t
                .odometer_mi
                .map_or_else(|| "—".to_string(), |o| format!("{o} mi")),
            Self::RuntimeMin => format!("{} min", t.runtime_min),
            Self::Ignition => on_off(t.ignition_on),
            Self::Moving => {
                if t.moving {
                    "moving".into()
                } else {
                    "parked".into()
                }
            }
            Self::FaultCodes => format!("{}", t.dtc_count),
            Self::GpsFix => g(|x| x.fix_type.clone()),
            Self::Satellites => g(|x| {
                x.satellites
                    .map_or_else(|| "—".to_string(), |n| n.to_string())
            }),
            Self::AccuracyM => g(|x| format!("{:.0} m", x.accuracy_m)),
            Self::Latitude => g(|x| format!("{:.4}", x.latitude)),
            Self::Longitude => g(|x| format!("{:.4}", x.longitude)),
            Self::AltitudeM => g(|x| format!("{:.0} m", x.altitude_m)),
            Self::AltitudeFt => g(|x| format!("{:.0} ft", x.altitude_m * 3.28084)),
            Self::UpdateRate => g(|x| format!("{:.0} Hz", x.update_rate_hz)),
            Self::FixAge => g(|x| format!("{:.0} s", x.update_age_s)),
            Self::LocationSource => s.locations.primary.label().to_string(),
            Self::ActiveWan => empty_dash(&w.active_wan),
            Self::CellASignal => format!("{} dBm", w.cellular_a.signal_dbm),
            Self::CellABars => bars(w.cellular_a.signal_dbm),
            Self::CellACarrier => empty_dash(&w.cellular_a.carrier),
            Self::CellATech => empty_dash(&w.cellular_a.technology),
            Self::CellASim => empty_dash(&w.cellular_a.sim_state),
            Self::CellAIp => empty_dash(&w.cellular_a.wan_ip),
            Self::CellAHealth => healthy(w.cellular_a.healthy),
            Self::CellBSignal => format!("{} dBm", w.cellular_b.signal_dbm),
            Self::CellBCarrier => empty_dash(&w.cellular_b.carrier),
            Self::CellBTech => empty_dash(&w.cellular_b.technology),
            Self::CellBSim => empty_dash(&w.cellular_b.sim_state),
            Self::CellBHealth => healthy(w.cellular_b.healthy),
            Self::WifiState => empty_dash(&w.wifi_state),
            Self::EthernetState => empty_dash(&w.ethernet_state),
            Self::VpnState => empty_dash(&w.vpn_state),
            Self::LinkQuality => empty_dash(&w.link_quality),
            Self::LatencyMs => format!("{} ms", w.latency_ms),
            Self::PacketLoss => format!("{:.1}%", w.packet_loss_percent),
            Self::Failovers => format!("{}", w.failover_events),
            Self::DataUsed => empty_dash(&w.data_transferred),
            Self::TelemetrySource => empty_dash(&t.confidence),
            Self::TelemetryAge => format!("{:.0} s", t.last_update_age_s),
            Self::NavEta => s.vehicle_glance().unwrap_or_else(|| "—".to_string()),
            Self::NavSummary => s.vehicle_glance().unwrap_or_else(|| "—".to_string()),
        }
    }
}

fn cardinal(deg: f32) -> &'static str {
    const DIRS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    let idx = (((deg.rem_euclid(360.0)) / 45.0).round() as usize) % 8;
    DIRS[idx]
}

fn on_off(v: bool) -> String {
    if v {
        "ON".into()
    } else {
        "OFF".into()
    }
}

fn healthy(v: bool) -> String {
    if v {
        "up".into()
    } else {
        "down".into()
    }
}

fn empty_dash(s: &str) -> String {
    if s.trim().is_empty() {
        "—".to_string()
    } else {
        s.to_string()
    }
}

/// Signal-strength bars from a cellular dBm reading (5-bar scale).
fn bars(dbm: i32) -> String {
    let n = match dbm {
        d if d >= -70 => 5,
        d if d >= -85 => 4,
        d if d >= -100 => 3,
        d if d >= -110 => 2,
        d if d >= -120 => 1,
        _ => 0,
    };
    format!("{}{}", "▮".repeat(n), "▯".repeat(5 - n))
}

/// The operator's chosen strip readouts (ordered), persisted to
/// `settings-car-status.json`. A driver taps a slot to cycle its readout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CarStatusSelection {
    slots: Vec<CarStatusItem>,
}

impl Default for CarStatusSelection {
    fn default() -> Self {
        Self::defaults()
    }
}

impl CarStatusSelection {
    /// The factory strip: the readouts a driver reaches for first. Speed lives in
    /// the gauge above, so it is not repeated here.
    #[must_use]
    pub fn defaults() -> Self {
        Self {
            slots: vec![
                CarStatusItem::HeadingCardinal,
                CarStatusItem::GpsFix,
                CarStatusItem::Satellites,
                CarStatusItem::BatteryV,
                CarStatusItem::ActiveWan,
                CarStatusItem::CellABars,
                CarStatusItem::LinkQuality,
                CarStatusItem::LatencyMs,
                CarStatusItem::Ignition,
                CarStatusItem::AltitudeFt,
            ],
        }
    }

    /// The selected readouts, in strip order.
    #[must_use]
    pub fn slots(&self) -> &[CarStatusItem] {
        &self.slots
    }

    /// Cycle the readout in `slot` to the next catalog entry (the driver tapped
    /// it). Out-of-range slots are ignored.
    pub fn cycle(&mut self, slot: usize) {
        if let Some(cur) = self.slots.get_mut(slot) {
            let idx = CarStatusItem::ALL
                .iter()
                .position(|i| i == cur)
                .unwrap_or(0);
            *cur = CarStatusItem::ALL[(idx + 1) % CarStatusItem::ALL.len()];
        }
    }

    fn default_path() -> Option<std::path::PathBuf> {
        mde_bus::client_data_dir().map(|d| d.join(CAR_STATUS_CONFIG_FILE))
    }

    /// Load from the default path (factory strip when absent / unparsable).
    #[must_use]
    pub fn load() -> Self {
        let Some(path) = Self::default_path() else {
            return Self::default();
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Self>(&s).ok())
            .filter(|s| !s.slots.is_empty())
            .unwrap_or_default()
    }

    /// Persist to the default path (atomic temp + rename; silent no-op headless).
    pub fn save(&self) {
        let Some(path) = Self::default_path() else {
            return;
        };
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_broad_and_every_item_resolves_without_panic() {
        let s = MapsLocationSurface::simulated();
        assert!(
            CarStatusItem::ALL.len() >= 40,
            "a rich catalog to choose from"
        );
        for item in CarStatusItem::ALL {
            let v = item.value(&s);
            assert!(!item.label().is_empty());
            assert!(!v.is_empty(), "{item:?} resolves to a value");
        }
    }

    #[test]
    fn selection_defaults_cycle_and_round_trip() {
        let mut sel = CarStatusSelection::defaults();
        assert!(!sel.slots().is_empty());
        let first = sel.slots()[0];
        sel.cycle(0);
        assert_ne!(sel.slots()[0], first, "cycling advances the slot");
        // Out-of-range cycle is a no-op, not a panic.
        sel.cycle(999);
        let json = serde_json::to_string(&sel).expect("serialize");
        let back: CarStatusSelection = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, sel);
    }

    #[test]
    fn cardinal_and_bars_are_sane() {
        assert_eq!(cardinal(0.0), "N");
        assert_eq!(cardinal(90.0), "E");
        assert_eq!(cardinal(180.0), "S");
        assert_eq!(bars(-60).chars().filter(|c| *c == '▮').count(), 5);
        assert_eq!(bars(-130).chars().filter(|c| *c == '▮').count(), 0);
    }
}
