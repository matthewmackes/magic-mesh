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

impl BatteryBody {
    /// The body a host with no battery (a desktop / server / VM)
    /// reports: upstream's `-1` "unknown / not a battery" sentinel,
    /// not charging, no threshold event. A stock KDE Connect peer
    /// reads `currentCharge == -1` as "this device has no battery"
    /// and renders nothing — the clean "not a battery" answer the
    /// KDC-PLUGINS epic calls for.
    #[must_use]
    pub fn not_a_battery() -> Self {
        Self {
            current_charge: -1,
            is_charging: false,
            threshold_event: String::new(),
        }
    }

    /// Build a real battery snapshot from a percentage + AC state.
    /// `charge_pct` is clamped to `0..=100`; `threshold_event` is
    /// set to upstream's `"low"` marker when the charge is at or
    /// below 15% while on battery (matches upstream's low-battery
    /// threshold), else empty.
    #[must_use]
    pub fn from_charge(charge_pct: u8, is_charging: bool) -> Self {
        let pct = i32::from(charge_pct.min(100));
        let threshold_event = if !is_charging && pct <= 15 {
            "low".to_string()
        } else {
            String::new()
        };
        Self {
            current_charge: pct,
            is_charging,
            threshold_event,
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

/// Build a `kdeconnect.battery.request` packet — the empty-bodied
/// poll a peer sends to ask the other side for its current battery
/// snapshot. Stock KDE Connect emits `{ "request": true }`; we
/// match that so a phone re-polls our desktop on demand.
#[must_use]
pub fn battery_request_packet(id_ms: i64) -> Packet {
    Packet {
        id: id_ms,
        kind: "kdeconnect.battery.request".to_string(),
        body: serde_json::json!({ "request": true }),
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
        // Handles both the snapshot AND the request kind so an
        // advertised `kdeconnect.battery.request` actually routes.
        assert_eq!(
            p.handles(),
            &["kdeconnect.battery", "kdeconnect.battery.request"]
        );
    }

    #[test]
    fn not_a_battery_is_the_unknown_sentinel() {
        // A desktop reports `-1` so a stock peer renders nothing.
        let b = BatteryBody::not_a_battery();
        assert_eq!(b.current_charge, -1);
        assert!(!b.is_charging);
        assert_eq!(b.charge_pct(), None);
    }

    #[test]
    fn from_charge_clamps_and_flags_low_on_battery() {
        // On battery at 10% → low threshold marker.
        let low = BatteryBody::from_charge(10, false);
        assert_eq!(low.charge_pct(), Some(10));
        assert_eq!(low.threshold_event, "low");
        // Charging at 10% → no low marker (it's recovering).
        let charging = BatteryBody::from_charge(10, true);
        assert_eq!(charging.threshold_event, "");
        // Over-range clamps to 100, no marker.
        let full = BatteryBody::from_charge(200, false);
        assert_eq!(full.charge_pct(), Some(100));
        assert_eq!(full.threshold_event, "");
    }

    #[test]
    fn battery_request_packet_kind_and_body() {
        let p = battery_request_packet(7);
        assert_eq!(p.kind, "kdeconnect.battery.request");
        assert_eq!(p.body, serde_json::json!({ "request": true }));
    }

    #[test]
    fn battery_plugin_answers_request_with_local_snapshot() {
        let mut plugin = BatteryPlugin::new();
        plugin.set_local_battery(BatteryBody::from_charge(64, true));
        let ctx = PluginContext::new("phone", true);
        let out = plugin.process(&battery_request_packet(1), &ctx);
        assert_eq!(out.len(), 1, "a request must produce one battery reply");
        assert_eq!(out[0].kind, "kdeconnect.battery");
        let body: BatteryBody = from_packet_body(&out[0]).unwrap();
        assert_eq!(body.current_charge, 64);
        assert!(body.is_charging);
        // A request must NOT be mistaken for an inbound snapshot.
        assert_eq!(plugin.pending_count(), 0);
    }

    #[test]
    fn battery_plugin_defaults_to_not_a_battery_for_a_request() {
        let mut plugin = BatteryPlugin::new();
        let ctx = PluginContext::new("phone", true);
        let out = plugin.process(&battery_request_packet(1), &ctx);
        let body: BatteryBody = from_packet_body(&out[0]).unwrap();
        assert_eq!(body.current_charge, -1, "desktop answers 'not a battery'");
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

/// `Plugin` impl with both directions of the battery feature:
///
/// - **Inbound `kdeconnect.battery`** — a peer's battery snapshot
///   is decoded + queued; the host drains via `take_received()`
///   and updates the peer's roster row.
/// - **Inbound `kdeconnect.battery.request`** — a peer (typically
///   a phone) polls *this host's* battery. The plugin answers with
///   a `kdeconnect.battery` packet built from the host snapshot set
///   via [`set_local_battery`](BatteryPlugin::set_local_battery)
///   (defaulting to the clean "not a battery" answer for a
///   desktop). This is the advertised-but-previously-unimplemented
///   incoming capability the KDC-PLUGINS epic closes.
#[derive(Debug)]
pub struct BatteryPlugin {
    received: Vec<BatteryBody>,
    /// This host's own battery snapshot, returned in answer to a
    /// `battery.request`. Defaults to "not a battery" (desktop).
    local: BatteryBody,
    handles: [&'static str; 2],
}

impl Default for BatteryPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl BatteryPlugin {
    /// New plugin defaulting the host snapshot to "not a battery"
    /// (a desktop / server — the common mesh-host case).
    #[must_use]
    pub fn new() -> Self {
        Self {
            received: Vec::new(),
            local: BatteryBody::not_a_battery(),
            handles: ["kdeconnect.battery", "kdeconnect.battery.request"],
        }
    }

    /// Set this host's battery snapshot — the body answered on the
    /// next inbound `battery.request`. The host refreshes this from
    /// `/sys/class/power_supply` before (or while) serving requests.
    pub fn set_local_battery(&mut self, body: BatteryBody) {
        self.local = body;
    }

    /// The host snapshot the plugin currently answers requests with.
    #[must_use]
    pub fn local_battery(&self) -> &BatteryBody {
        &self.local
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
        if packet.kind == "kdeconnect.battery.request" {
            // A peer is polling us — answer with this host's snapshot.
            let id_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as i64);
            return vec![battery_packet(id_ms, self.local.clone())];
        }
        if let Ok(body) = crate::plugins::from_packet_body::<BatteryBody>(packet) {
            self.received.push(body);
        }
        Vec::new()
    }
}
