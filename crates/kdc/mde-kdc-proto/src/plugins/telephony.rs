//! KDC2-2.10 telephony plugin — `kdeconnect.telephony` body.
//!
//! Mirrors phone-call state from a paired Android device:
//! ringing / talking / missed. Used by the Workbench Mesh panel
//! to flash an indicator + (optionally) pause local media when
//! the phone rings.

use serde::{Deserialize, Serialize};

use crate::wire::Packet;

/// Phone-call event types upstream KDE Connect emits via
/// `kdeconnect.telephony`. Stable across upstream releases — the
/// Android client's source code (libqcoro / plasma-mobile) is
/// the reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TelephonyEvent {
    /// Inbound call ringing — phone not yet answered.
    Ringing,
    /// Inbound or outbound call in progress.
    Talking,
    /// Missed call notification.
    Missed,
    /// Call disconnected (no longer ringing / talking).
    Disconnected,
}

/// `kdeconnect.telephony` body. Event-driven: every state
/// transition emits one packet with the new state + the
/// associated caller info (when available).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelephonyBody {
    /// Event type.
    pub event: TelephonyEvent,
    /// Caller phone number (raw — strip / format for display).
    /// Empty for outbound calls or when caller-ID is suppressed.
    #[serde(default)]
    pub phone_number: String,
    /// Caller name from the phone's contact book. Empty when
    /// unknown.
    #[serde(default)]
    pub contact_name: String,
    /// True when this packet cancels a previous Ringing/Talking
    /// event (the call ended). Some upstream clients emit
    /// `Disconnected` + `is_cancel = true`; others emit only one
    /// of the two. Receivers should treat either as call-ended.
    #[serde(default)]
    pub is_cancel: bool,
}

/// Build a telephony event packet.
#[must_use]
pub fn telephony_packet(id_ms: i64, body: TelephonyBody) -> Packet {
    Packet {
        id: id_ms,
        kind: "kdeconnect.telephony".to_string(),
        body: serde_json::to_value(body).expect("TelephonyBody is always JSON-serializable"),
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
    fn telephony_event_serializes_lowercase() {
        // Matches upstream's `"event":"ringing"` token.
        assert_eq!(
            serde_json::to_string(&TelephonyEvent::Ringing).unwrap(),
            r#""ringing""#,
        );
        assert_eq!(
            serde_json::to_string(&TelephonyEvent::Talking).unwrap(),
            r#""talking""#,
        );
        assert_eq!(
            serde_json::to_string(&TelephonyEvent::Missed).unwrap(),
            r#""missed""#,
        );
        assert_eq!(
            serde_json::to_string(&TelephonyEvent::Disconnected).unwrap(),
            r#""disconnected""#,
        );
    }

    #[test]
    fn telephony_body_serializes_with_camel_case_keys() {
        let body = TelephonyBody {
            event: TelephonyEvent::Ringing,
            phone_number: "+15551234".to_string(),
            contact_name: "Alice".to_string(),
            is_cancel: false,
        };
        let s = serde_json::to_string(&body).unwrap();
        assert!(s.contains(r#""phoneNumber":"+15551234""#));
        assert!(s.contains(r#""contactName":"Alice""#));
        assert!(s.contains(r#""isCancel":false"#));
        assert!(s.contains(r#""event":"ringing""#));
    }

    #[test]
    fn telephony_body_round_trips_via_wire() {
        let body = TelephonyBody {
            event: TelephonyEvent::Missed,
            phone_number: "+15551234".to_string(),
            contact_name: String::new(),
            is_cancel: false,
        };
        let p = telephony_packet(1, body.clone());
        let wire = serde_json::to_string(&p).unwrap();
        let decoded: Packet = serde_json::from_str(&wire).unwrap();
        let back: TelephonyBody = from_packet_body(&decoded).unwrap();
        assert_eq!(back, body);
    }

    #[test]
    fn telephony_packet_kind_matches_plugin_token() {
        let p = telephony_packet(
            1,
            TelephonyBody {
                event: TelephonyEvent::Ringing,
                phone_number: String::new(),
                contact_name: String::new(),
                is_cancel: false,
            },
        );
        assert_eq!(p.kind, crate::plugins::PluginKind::Telephony.packet_kind());
    }

    #[test]
    fn telephony_body_deserializes_missing_optional_fields() {
        // Minimum packet upstream may emit: just `event`. Caller
        // / contact / cancel default cleanly.
        let raw = r#"{"event":"ringing"}"#;
        let body: TelephonyBody = serde_json::from_str(raw).unwrap();
        assert_eq!(body.event, TelephonyEvent::Ringing);
        assert_eq!(body.phone_number, "");
        assert_eq!(body.contact_name, "");
        assert!(!body.is_cancel);
    }

    // KDC2-2.18 — TelephonyPlugin Plugin trait impl
    use crate::plugins::{Plugin, PluginContext, PluginKind};

    #[test]
    fn telephony_plugin_queues_inbound_event() {
        let mut plugin = TelephonyPlugin::new();
        let ctx = PluginContext::new("phone", true);
        let body = TelephonyBody {
            event: TelephonyEvent::Ringing,
            phone_number: "+15551234".to_string(),
            contact_name: "Alice".to_string(),
            is_cancel: false,
        };
        plugin.process(&telephony_packet(1, body.clone()), &ctx);
        assert_eq!(plugin.take_received(), vec![body]);
    }
}

/// KDC2-2.18a — TelephonyPlugin (Plugin trait impl)
#[derive(Debug, Default)]
pub struct TelephonyPlugin {
    received: Vec<TelephonyBody>,
    handles: [&'static str; 1],
}

impl TelephonyPlugin {
    /// New empty plugin.
    #[must_use]
    pub fn new() -> Self {
        Self {
            received: Vec::new(),
            handles: ["kdeconnect.telephony"],
        }
    }
    /// Drain every queued telephony body.
    #[must_use]
    pub fn take_received(&mut self) -> Vec<TelephonyBody> {
        std::mem::take(&mut self.received)
    }
    /// Items currently queued.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.received.len()
    }
}

impl crate::plugins::Plugin for TelephonyPlugin {
    fn kind(&self) -> crate::plugins::PluginKind {
        crate::plugins::PluginKind::Telephony
    }
    fn handles(&self) -> &[&'static str] {
        &self.handles
    }
    fn process(
        &mut self,
        packet: &crate::wire::Packet,
        _ctx: &crate::plugins::PluginContext,
    ) -> Vec<crate::wire::Packet> {
        if let Ok(body) = crate::plugins::from_packet_body::<TelephonyBody>(packet) {
            self.received.push(body);
        }
        Vec::new()
    }
}
