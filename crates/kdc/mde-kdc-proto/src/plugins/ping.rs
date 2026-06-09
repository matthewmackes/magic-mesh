//! KDC2-2.8 ping plugin — `kdeconnect.ping` body.
//!
//! Smallest plugin body shape in KDE Connect. Used as a liveness
//! check between peers: a `ping` is sent, the receiver may surface
//! a notification ("Hello from <peer>") and the sender confirms
//! the path is still alive.
//!
//! Upstream's body has a single optional `message` field. KDC2's
//! [`PingBody`] matches.

use serde::{Deserialize, Serialize};

use crate::wire::Packet;

/// `kdeconnect.ping` body. The single optional `message` field
/// carries the user-visible string the receiver surfaces (e.g.
/// `"Hello from Bob's laptop"`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PingBody {
    /// Optional message to surface on the receiver. Empty string
    /// is a "bare ping" — receivers may display the default
    /// `"Ping from <peer>"` text.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
}

/// Build a `kdeconnect.ping` packet.
#[must_use]
pub fn ping_packet(id_ms: i64, message: String) -> Packet {
    Packet {
        id: id_ms,
        kind: "kdeconnect.ping".to_string(),
        body: serde_json::to_value(PingBody { message })
            .expect("PingBody is always JSON-serializable"),
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
    fn ping_with_message_serializes_message_field() {
        let p = ping_packet(1, "hi".to_string());
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains(r#""message":"hi""#));
    }

    #[test]
    fn bare_ping_omits_message_field_from_wire() {
        // skip_serializing_if = "String::is_empty" lock: an empty
        // message must NOT serialize as `"message":""` because the
        // upstream Android client treats the presence of the field
        // as "show this exact text" — a `""` would surface as a
        // blank notification.
        let p = ping_packet(1, String::new());
        let s = serde_json::to_string(&p).unwrap();
        assert!(
            !s.contains(r#""message""#),
            "bare ping must omit message; got {s}"
        );
    }

    #[test]
    fn ping_body_round_trips_via_wire() {
        let p = ping_packet(99, "round-trip".to_string());
        let wire = serde_json::to_string(&p).unwrap();
        let decoded: Packet = serde_json::from_str(&wire).unwrap();
        let body: PingBody = from_packet_body(&decoded).unwrap();
        assert_eq!(body.message, "round-trip");
    }

    #[test]
    fn ping_body_deserializes_without_message_field() {
        // A bare ping from upstream looks like `{}` — must
        // deserialize to default (empty string).
        let body: PingBody = serde_json::from_str("{}").unwrap();
        assert_eq!(body.message, "");
    }

    #[test]
    fn ping_packet_kind_matches_plugin_token() {
        let p = ping_packet(1, "x".to_string());
        assert_eq!(p.kind, crate::plugins::PluginKind::Ping.packet_kind());
    }

    // KDC2-2.16 — PingPlugin Plugin trait impl

    use crate::plugins::{Plugin, PluginContext, PluginKind};

    #[test]
    fn ping_plugin_queues_received_message() {
        let mut plugin = PingPlugin::new();
        let ctx = PluginContext::new("alice", true);
        plugin.process(&ping_packet(1, "ping from alice".into()), &ctx);
        let drained = plugin.take_received();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].message, "ping from alice");
    }
}

/// KDC2-2.16 — PingPlugin (adapter pattern). Inbound pings get
/// queued for host-side notification surfacing.
#[derive(Debug, Default)]
pub struct PingPlugin {
    received: Vec<PingBody>,
    handles: [&'static str; 1],
}

impl PingPlugin {
    /// New empty plugin.
    #[must_use]
    pub fn new() -> Self {
        Self {
            received: Vec::new(),
            handles: ["kdeconnect.ping"],
        }
    }
    /// Drain every queued inbound ping body.
    #[must_use]
    pub fn take_received(&mut self) -> Vec<PingBody> {
        std::mem::take(&mut self.received)
    }
    /// Items currently queued.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.received.len()
    }
}

impl crate::plugins::Plugin for PingPlugin {
    fn kind(&self) -> crate::plugins::PluginKind {
        crate::plugins::PluginKind::Ping
    }
    fn handles(&self) -> &[&'static str] {
        &self.handles
    }
    fn process(
        &mut self,
        packet: &crate::wire::Packet,
        _ctx: &crate::plugins::PluginContext,
    ) -> Vec<crate::wire::Packet> {
        if let Ok(body) = crate::plugins::from_packet_body::<PingBody>(packet) {
            self.received.push(body);
        }
        Vec::new()
    }
}
