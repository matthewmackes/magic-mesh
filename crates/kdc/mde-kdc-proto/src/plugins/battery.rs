//! KDC2-2.7 battery plugin — `kdeconnect.battery` body.
//!
//! Mirrors battery state from a paired phone (or another peer)
//! so the local Workbench Mesh panel can render a battery
//! indicator without polling the device directly.

use serde::{Deserialize, Serialize};

use crate::wire::Packet;

/// `kdeconnect.battery` body. Upstream's field names use a mix
/// of `currentCharge` (camel) and `isCharging` (camel) — KDC2
/// matches verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryBody {
    /// Charge percentage 0..=100. Upstream sometimes sends -1 for
    /// "unknown" — receivers should treat negatives as `None` in
    /// their UI model.
    pub current_charge: i32,
    /// True when the device is plugged in / actively charging.
    pub is_charging: bool,
    /// Threshold event — set by upstream when the battery crosses
    /// a configured threshold (low / critical). Empty string when
    /// no threshold event is firing.
    #[serde(default)]
    pub threshold_event: String,
}

impl BatteryBody {
    /// Sanitized percentage: `None` when upstream sent -1 or an
    /// out-of-range value; `Some(0..=100)` otherwise. Receivers
    /// should call this rather than reading `current_charge`
    /// directly.
    #[must_use]
    pub fn charge_pct(&self) -> Option<u8> {
        if (0..=100).contains(&self.current_charge) {
            Some(self.current_charge as u8)
        } else {
            None
        }
    }
}

/// Build a `kdeconnect.battery` packet.
#[must_use]
pub fn battery_packet(id_ms: i64, body: BatteryBody) -> Packet {
    Packet {
        id: id_ms,
        kind: "kdeconnect.battery".to_string(),
        body: serde_json::to_value(body).expect("BatteryBody is always JSON-serializable"),
        mde_caps: None,
        payload_size: None,
        payload_transfer_info: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::from_packet_body;

    #[test]
    fn battery_body_serializes_with_camel_case_keys() {
        let b = BatteryBody {
            current_charge: 73,
            is_charging: false,
            threshold_event: String::new(),
        };
        let s = serde_json::to_string(&b).unwrap();
        assert!(s.contains(r#""currentCharge":73"#));
        assert!(s.contains(r#""isCharging":false"#));
        assert!(s.contains(r#""thresholdEvent":"""#));
    }

    #[test]
    fn charge_pct_returns_none_for_unknown_sentinel() {
        let b = BatteryBody {
            current_charge: -1,
            is_charging: false,
            threshold_event: String::new(),
        };
        assert_eq!(b.charge_pct(), None);
    }

    #[test]
    fn charge_pct_returns_some_for_valid_range() {
        for pct in [0_i32, 1, 50, 99, 100] {
            let b = BatteryBody {
                current_charge: pct,
                is_charging: false,
                threshold_event: String::new(),
            };
            assert_eq!(b.charge_pct(), Some(pct as u8));
        }
    }

    #[test]
    fn charge_pct_returns_none_for_out_of_range_positive() {
        let b = BatteryBody {
            current_charge: 150,
            is_charging: false,
            threshold_event: String::new(),
        };
        assert_eq!(b.charge_pct(), None);
    }

    #[test]
    fn battery_packet_round_trips_via_wire() {
        let body = BatteryBody {
            current_charge: 42,
            is_charging: true,
            threshold_event: "low".to_string(),
        };
        let p = battery_packet(1, body.clone());
        let wire = serde_json::to_string(&p).unwrap();
        let decoded: Packet = serde_json::from_str(&wire).unwrap();
        let back: BatteryBody = from_packet_body(&decoded).unwrap();
        assert_eq!(back, body);
    }

    #[test]
    fn threshold_event_defaults_to_empty_string() {
        // Older clients don't emit `thresholdEvent` — must
        // default to empty.
        let raw = r#"{"currentCharge":50,"isCharging":false}"#;
        let body: BatteryBody = serde_json::from_str(raw).unwrap();
        assert_eq!(body.threshold_event, "");
    }

    // ─────────────────────────────────────────────────────────
    // KDC2-2.17 — BatteryPlugin (Plugin trait impl)
    // ─────────────────────────────────────────────────────────

    use crate::plugins::{Plugin, PluginContext, PluginKind};

    #[test]
    fn battery_plugin_kind_and_handles_match_token() {
        let p = BatteryPlugin::new();
        assert_eq!(p.kind(), PluginKind::Battery);
        assert_eq!(p.handles(), &["kdeconnect.battery"]);
    }

    #[test]
    fn battery_plugin_queues_inbound_snapshot() {
        let mut plugin = BatteryPlugin::new();
        let ctx = PluginContext::new("phone", true);
        let body = BatteryBody {
            current_charge: 73,
            is_charging: true,
            threshold_event: String::new(),
        };
        plugin.process(&battery_packet(1, body.clone()), &ctx);
        let drained = plugin.take_received();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].current_charge, 73);
        assert!(drained[0].is_charging);
    }
}

// ────────────────────────────────────────────────────────────────
// KDC2-2.17a — BatteryPlugin (Plugin trait impl, adapter pattern)
// ────────────────────────────────────────────────────────────────

/// `Plugin` impl that mirrors inbound battery snapshots. Host
/// (`mde-kdc`) drains via `take_received()` and updates the
/// peer's [`mackes_mesh_types::ConnectFacts.battery`] field.
#[derive(Debug, Default)]
pub struct BatteryPlugin {
    received: Vec<BatteryBody>,
    handles: [&'static str; 1],
}

impl BatteryPlugin {
    /// New empty plugin.
    #[must_use]
    pub fn new() -> Self {
        Self {
            received: Vec::new(),
            handles: ["kdeconnect.battery"],
        }
    }

    /// Drain every received battery body.
    #[must_use]
    pub fn take_received(&mut self) -> Vec<BatteryBody> {
        std::mem::take(&mut self.received)
    }

    /// Items currently queued.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.received.len()
    }
}

impl crate::plugins::Plugin for BatteryPlugin {
    fn kind(&self) -> crate::plugins::PluginKind {
        crate::plugins::PluginKind::Battery
    }

    fn handles(&self) -> &[&'static str] {
        &self.handles
    }

    fn process(
        &mut self,
        packet: &crate::wire::Packet,
        _ctx: &crate::plugins::PluginContext,
    ) -> Vec<crate::wire::Packet> {
        if let Ok(body) = crate::plugins::from_packet_body::<BatteryBody>(packet) {
            self.received.push(body);
        }
        Vec::new()
    }
}
