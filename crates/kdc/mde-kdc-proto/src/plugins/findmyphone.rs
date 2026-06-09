//! KDC2-2.10 findmyphone plugin — `kdeconnect.findmyphone.request`.
//!
//! Body is empty — receipt of the packet itself is the signal to
//! ring. The `.request` suffix is upstream's convention for
//! action-trigger packets.

use serde::{Deserialize, Serialize};

use crate::wire::Packet;

/// `kdeconnect.findmyphone.request` body — empty by design.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FindMyPhoneBody;

/// Build a findmyphone trigger packet.
#[must_use]
pub fn find_my_phone_packet(id_ms: i64) -> Packet {
    Packet {
        id: id_ms,
        kind: "kdeconnect.findmyphone.request".to_string(),
        body: serde_json::json!({}),
        mde_caps: None,
        payload_size: None,
        payload_transfer_info: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn findmyphone_packet_kind_includes_request_suffix() {
        let p = find_my_phone_packet(1);
        assert_eq!(p.kind, "kdeconnect.findmyphone.request");
        assert_eq!(
            p.kind,
            crate::plugins::PluginKind::FindMyPhone.packet_kind(),
        );
    }

    #[test]
    fn findmyphone_body_serializes_as_empty_object() {
        let p = find_my_phone_packet(1);
        let s = serde_json::to_string(&p).unwrap();
        // Body is `{}` — the trigger semantic is "packet arrived,
        // ring the phone." No metadata needed.
        assert!(s.contains(r#""body":{}"#));
    }

    #[test]
    fn findmyphone_body_round_trips_via_wire() {
        let p = find_my_phone_packet(42);
        let wire = serde_json::to_string(&p).unwrap();
        let decoded: Packet = serde_json::from_str(&wire).unwrap();
        assert_eq!(decoded.id, 42);
        assert_eq!(decoded.kind, "kdeconnect.findmyphone.request");
    }

    // KDC2-2.16 — FindMyPhonePlugin Plugin trait impl
    use crate::plugins::{Plugin, PluginContext, PluginKind};

    #[test]
    fn findmyphone_plugin_records_trigger() {
        let mut plugin = FindMyPhonePlugin::new();
        let ctx = PluginContext::new("alice", true);
        plugin.process(&find_my_phone_packet(1), &ctx);
        assert_eq!(plugin.trigger_count(), 1);
        // Drain resets the counter.
        let _ = plugin.take_triggers();
        assert_eq!(plugin.trigger_count(), 0);
    }
}

/// KDC2-2.16 — FindMyPhonePlugin. Body is empty; we record
/// trigger COUNT rather than queuing bodies, since each trigger
/// is interchangeable.
#[derive(Debug, Default)]
pub struct FindMyPhonePlugin {
    triggers: u32,
    handles: [&'static str; 1],
}

impl FindMyPhonePlugin {
    /// New empty plugin.
    #[must_use]
    pub fn new() -> Self {
        Self {
            triggers: 0,
            handles: ["kdeconnect.findmyphone.request"],
        }
    }
    /// Return the pending trigger count + reset to zero.
    #[must_use]
    pub fn take_triggers(&mut self) -> u32 {
        std::mem::replace(&mut self.triggers, 0)
    }
    /// Triggers currently queued.
    #[must_use]
    pub fn trigger_count(&self) -> u32 {
        self.triggers
    }
}

impl crate::plugins::Plugin for FindMyPhonePlugin {
    fn kind(&self) -> crate::plugins::PluginKind {
        crate::plugins::PluginKind::FindMyPhone
    }
    fn handles(&self) -> &[&'static str] {
        &self.handles
    }
    fn process(
        &mut self,
        _packet: &crate::wire::Packet,
        _ctx: &crate::plugins::PluginContext,
    ) -> Vec<crate::wire::Packet> {
        self.triggers = self.triggers.saturating_add(1);
        Vec::new()
    }
}
