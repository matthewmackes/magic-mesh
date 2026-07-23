//! Provider-neutral **vehicle-gateway** mirror + command contract — the "Rolling Node".
//!
//! A workstation-side adapter (the mackesd `vehicle` worker) SSH/HTTP-polls a mobile
//! gateway (a Sierra AirLink **MG90** / oMG today) and publishes a latest-wins
//! `state/vehicle/<node>` mirror; the shell's maps-location cockpit folds it into its
//! live models (`Mg90Status`/`CellularLink`/`LocationSample`/`VehicleTelemetry`). Config
//! mutations go out as `action/vehicle/<verb>` and resolve via `reply/<ulid>` — the same
//! Bus idiom as the `cloud` mirror. This crate stays pure data + a pure NMEA parser; the
//! worker owns the SSH/HTTP transport.

use serde::{Deserialize, Serialize};

/// Topic prefix for the per-node vehicle-gateway mirror.
pub const VEHICLE_STATE_PREFIX: &str = "state/vehicle/";

/// The `state/vehicle/<node>` mirror topic for a node.
#[must_use]
pub fn vehicle_state_topic(node: &str) -> String {
    format!("{VEHICLE_STATE_PREFIX}{node}")
}

/// Command prefix for gateway mutations (`action/vehicle/<verb>`).
pub const VEHICLE_ACTION_PREFIX: &str = "action/vehicle/";

/// The `action/vehicle/<verb>` request topic for a verb.
#[must_use]
pub fn vehicle_action_topic(verb: &str) -> String {
    format!("{VEHICLE_ACTION_PREFIX}{verb}")
}

/// A GNSS fix parsed from the gateway's NMEA (oMG `omgtime.g.info` `$GPGGA`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct GpsFix {
    /// Human fix label: `no-fix` | `gps` | `dgps`.
    pub fix_type: String,
    /// Latitude, decimal degrees (N positive).
    pub latitude: f64,
    /// Longitude, decimal degrees (E positive).
    pub longitude: f64,
    /// Altitude, meters MSL.
    pub altitude_m: f32,
    /// Horizontal dilution of precision (lower is better; 99 = no fix).
    pub hdop: f32,
    /// Satellites used in the fix.
    pub satellites: u8,
    /// Ground speed, mph (from RMC/VTG when available; 0 from GGA alone).
    pub speed_mph: f32,
    /// Heading, degrees true (0 from GGA alone).
    pub heading_deg: f32,
    /// Age of this fix, seconds.
    pub age_s: f32,
    /// Observed update rate, Hz.
    pub update_rate_hz: f32,
}

impl GpsFix {
    /// Whether the gateway currently holds a position lock.
    #[must_use]
    pub fn has_fix(&self) -> bool {
        self.fix_type != "no-fix" && self.satellites > 0
    }
}

/// A 6-axis inertial sample from the gateway's built-in IMU (oMG `$PSIWMMPU`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ImuSample {
    /// Acceleration X/Y/Z (g).
    pub accel_g: [f32; 3],
    /// Angular rate X/Y/Z (deg/s).
    pub gyro_dps: [f32; 3],
}

/// One cellular link's live status (mirrors the cockpit `CellularLink`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CellLink {
    /// SIM state (`ready` / `standby` / `absent` / …).
    pub sim_state: String,
    /// Carrier / operator name.
    pub carrier: String,
    /// Received signal strength, dBm (negative; e.g. -72).
    pub signal_dbm: i32,
    /// Radio access technology (`5G/LTE-A` / `LTE` / …).
    pub technology: String,
    /// Assigned WAN IP, or `not active`.
    pub wan_ip: String,
    /// Link health per the gateway.
    pub healthy: bool,
}

/// The gateway's multi-WAN uplink status (mirrors the cockpit `Mg90Status`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct WanStatus {
    /// The currently-active WAN label (e.g. `Cellular A`).
    pub active_wan: String,
    /// Cellular modem A.
    pub cellular_a: CellLink,
    /// Cellular modem B.
    pub cellular_b: CellLink,
    /// Wi-Fi-as-WAN / AP state label.
    pub wifi_state: String,
    /// Ethernet WAN state label.
    pub ethernet_state: String,
    /// VPN state label.
    pub vpn_state: String,
    /// Failover events observed this session.
    pub failover_events: u32,
    /// Uplink latency, ms.
    pub latency_ms: u32,
    /// Uplink packet loss, percent.
    pub packet_loss_percent: f32,
    /// Overall link-quality label.
    pub link_quality: String,
}

impl WanStatus {
    /// The active cellular link, when the active WAN is cellular.
    #[must_use]
    pub fn active_cellular(&self) -> Option<&CellLink> {
        match self.active_wan.as_str() {
            "Cellular A" => Some(&self.cellular_a),
            "Cellular B" => Some(&self.cellular_b),
            _ => None,
        }
    }
}

/// Vehicle power + OBD/CAN telemetry (mirrors the cockpit `VehicleTelemetry`, plus the
/// MCU-sourced board temp). Power fields (`battery_v`/`internal_temp_c`/`ignition_on`)
/// come from the gateway MCU; the rest from OBD-II when `obd_present`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct VehicleTelem {
    /// Main/charging bus voltage, volts (MCU).
    pub battery_v: f32,
    /// Gateway internal board temperature, °C (MCU).
    pub internal_temp_c: f32,
    /// Ignition-sense line state (MCU, `IGNTHRESH`).
    pub ignition_on: bool,
    /// Motion state (from GNSS speed or IMU).
    pub moving: bool,
    /// Whether an OBD-II source is present (the fields below are meaningful).
    pub obd_present: bool,
    /// Vehicle speed, mph (OBD).
    pub speed_mph: f32,
    /// Engine RPM (OBD).
    pub rpm: u32,
    /// Coolant temperature, °C (OBD).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coolant_c: Option<f32>,
    /// Fuel level, percent (OBD).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fuel_percent: Option<f32>,
    /// Diagnostic trouble code count (OBD).
    pub dtc_count: u32,
    /// Odometer, miles (OBD).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub odometer_mi: Option<u32>,
    /// Engine runtime, minutes (OBD).
    pub runtime_min: u32,
}

/// The per-node `state/vehicle/<node>` mirror — one gateway's live snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VehicleState {
    /// This node's id (the mirror `host` stamp + topic namespace).
    pub host: String,
    /// Gateway model (e.g. `MG90`).
    pub model: String,
    /// Gateway electronic serial number.
    pub esn: String,
    /// Gateway firmware version (e.g. `4.3.0.1`).
    pub mgos_version: String,
    /// Whether the adapter currently reaches the gateway.
    pub online: bool,
    /// Latest GNSS fix.
    pub gps: GpsFix,
    /// Latest IMU sample, when the gateway exposes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imu: Option<ImuSample>,
    /// Multi-WAN uplink status.
    pub wan: WanStatus,
    /// Vehicle power + OBD telemetry.
    pub telem: VehicleTelem,
    /// What this adapter could NOT report (honest-partial note; empty when full).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<String>,
    /// Wall-clock publish time (ms since the Unix epoch).
    pub published_at_ms: i64,
}

impl VehicleState {
    /// An honest offline snapshot for a node whose gateway is unreachable.
    #[must_use]
    pub fn offline(host: &str) -> Self {
        Self {
            host: host.to_string(),
            model: String::new(),
            esn: String::new(),
            mgos_version: String::new(),
            online: false,
            gps: GpsFix::default(),
            imu: None,
            wan: WanStatus::default(),
            telem: VehicleTelem::default(),
            gaps: vec!["gateway unreachable".to_string()],
            published_at_ms: 0,
        }
    }
}

/// The typed reply published to `reply/<ulid>` for an `action/vehicle/*` verb.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VehicleReply {
    /// `true` when the verb was applied.
    pub ok: bool,
    /// The verb this reply answers.
    #[serde(default)]
    pub verb: String,
    /// An honest gate reason (nothing performed; retry later).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gated: Option<String>,
    /// A rejection or backend failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// A summary of what was applied (e.g. the committed config file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied: Option<String>,
    /// Whether a destructive op (reset/reboot/failover) was performed + audited.
    #[serde(default)]
    pub audited: bool,
}

/// Parse one NMEA `$GPGGA` sentence into a [`GpsFix`] (position/altitude/sats/HDOP).
///
/// GGA carries no speed/heading (those come from RMC/VTG) — those stay `0.0`. Returns
/// `None` when the line is not a well-formed GGA sentence. Coordinates are `ddmm.mmmm`
/// / `dddmm.mmmm` with a hemisphere field, converted to signed decimal degrees.
#[must_use]
pub fn parse_gpgga(line: &str) -> Option<GpsFix> {
    let line = line.trim();
    // Accept "$GPGGA,..." / "$GNGGA,..." (strip any leading noise before the tag).
    let start = line.find("GGA,")?;
    let body = &line[start + 4..];
    // Drop the checksum suffix if present.
    let body = body.split('*').next().unwrap_or(body);
    let f: Vec<&str> = body.split(',').collect();
    // Fields after "GGA,": 0=time,1=lat,2=N/S,3=lon,4=E/W,5=quality,6=numSats,7=HDOP,8=alt
    if f.len() < 9 {
        return None;
    }
    let lat = nmea_coord(f.get(1)?, f.get(2)?);
    let lon = nmea_coord(f.get(3)?, f.get(4)?);
    let quality: u8 = f.get(5).and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    let satellites: u8 = f.get(6).and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    let hdop: f32 = f.get(7).and_then(|s| s.trim().parse().ok()).unwrap_or(99.0);
    let altitude_m: f32 = f.get(8).and_then(|s| s.trim().parse().ok()).unwrap_or(0.0);
    let fix_type = match quality {
        0 => "no-fix",
        2 => "dgps",
        _ => "gps",
    }
    .to_string();
    Some(GpsFix {
        fix_type,
        latitude: lat.unwrap_or(0.0),
        longitude: lon.unwrap_or(0.0),
        altitude_m,
        hdop,
        satellites,
        speed_mph: 0.0,
        heading_deg: 0.0,
        age_s: 0.0,
        update_rate_hz: 0.0,
    })
}

/// Convert an NMEA `ddmm.mmmm` value + hemisphere into signed decimal degrees.
fn nmea_coord(value: &str, hemi: &str) -> Option<f64> {
    let v: f64 = value.trim().parse().ok()?;
    if v == 0.0 && value.trim().is_empty() {
        return None;
    }
    let deg = (v / 100.0).trunc();
    let min = v - deg * 100.0;
    let mut dd = deg + min / 60.0;
    if matches!(hemi.trim(), "S" | "W") {
        dd = -dd;
    }
    Some(dd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topics_are_namespaced() {
        assert_eq!(vehicle_state_topic("eagle"), "state/vehicle/eagle");
        assert_eq!(
            vehicle_action_topic("set-failover"),
            "action/vehicle/set-failover"
        );
    }

    #[test]
    fn parse_real_gpgga_no_lock_sample() {
        // The exact sentence captured from the bench MG90's omgtime.g.info.
        let fix = parse_gpgga(
            "$GPGGA,111504.000,3210.07993,N,09550.95445,W,0,00,99.0,081.94,M,-24.2,M,,*66",
        )
        .expect("valid GGA");
        assert_eq!(fix.fix_type, "no-fix");
        assert_eq!(fix.satellites, 0);
        assert!(!fix.has_fix(), "quality 0 / 0 sats ⇒ no lock");
        assert!(
            (fix.latitude - 32.167_998).abs() < 1e-4,
            "lat {}",
            fix.latitude
        );
        assert!(
            (fix.longitude + 95.849_240).abs() < 1e-4,
            "lon {}",
            fix.longitude
        );
        assert!((fix.altitude_m - 81.94).abs() < 0.01);
        assert!((fix.hdop - 99.0).abs() < 0.01);
    }

    #[test]
    fn parse_gpgga_with_lock() {
        let fix = parse_gpgga("$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47")
            .expect("valid GGA");
        assert_eq!(fix.fix_type, "gps");
        assert_eq!(fix.satellites, 8);
        assert!(fix.has_fix());
        assert!((fix.latitude - 48.117_3).abs() < 1e-3);
        assert!((fix.longitude - 11.516_6).abs() < 1e-3);
    }

    #[test]
    fn parse_rejects_non_gga() {
        assert!(parse_gpgga("$PSIWMMPU,48.850,0.26605").is_none());
        assert!(parse_gpgga("garbage").is_none());
    }

    #[test]
    fn offline_snapshot_is_honest() {
        let s = VehicleState::offline("eagle");
        assert!(!s.online);
        assert!(!s.gps.has_fix());
        assert_eq!(s.gaps, vec!["gateway unreachable".to_string()]);
    }

    #[test]
    fn mirror_round_trips_json() {
        let mut s = VehicleState::offline("rig-1");
        s.online = true;
        s.model = "MG90".to_string();
        s.wan.active_wan = "Cellular A".to_string();
        s.wan.cellular_a.signal_dbm = -72;
        let j = serde_json::to_string(&s).unwrap();
        let back: VehicleState = serde_json::from_str(&j).unwrap();
        assert_eq!(s, back);
        assert_eq!(back.wan.active_cellular().map(|l| l.signal_dbm), Some(-72));
    }
}
