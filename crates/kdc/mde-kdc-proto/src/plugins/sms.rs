//! KDC2-2.9 sms plugin — `kdeconnect.sms.messages` body.
//!
//! Mirrors SMS / MMS messages from an Android phone to a paired
//! peer. Upstream protocol distinguishes:
//!
//!   * `kdeconnect.sms.messages` (plural) — bulk message list,
//!     emitted by the phone after a `request_conversations`
//!     packet.
//!   * `kdeconnect.sms.request` — sender→phone request to
//!     deliver a new outbound message.
//!
//! KDC2-2.1's `PluginKind::Sms` locks the `.messages` suffix as
//! the canonical incoming packet kind (the most common one). The
//! `.request` variant is a separate packet kind handled by the
//! host integration's plugin registry, not a different body type
//! at this layer.

use serde::{Deserialize, Serialize};

use crate::wire::Packet;

/// Single SMS / MMS message as it appears in upstream's
/// `kdeconnect.sms.messages` body's `messages` array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmsMessage {
    /// Stable message identifier on the phone.
    pub id: i64,
    /// Conversation thread identifier — groups messages by
    /// recipient.
    pub thread_id: i64,
    /// Message body text.
    pub body: String,
    /// Sender address — phone number or contact display name.
    /// Empty for sent-by-self messages.
    #[serde(default)]
    pub address: String,
    /// Wall-clock send time (millisecond Unix epoch).
    pub date: i64,
    /// Direction: 1 = inbox (received), 2 = sent. Other values
    /// occur but are vendor-specific.
    #[serde(rename = "type")]
    pub kind: i32,
    /// Read state. True when the message was already read on the
    /// phone before this packet was emitted.
    #[serde(default)]
    pub read: bool,
}

/// `kdeconnect.sms.messages` body — wraps a vector of messages.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmsMessagesBody {
    /// Messages in the snapshot. Receivers append to their local
    /// conversation store after de-duplicating by `(thread_id, id)`.
    pub messages: Vec<SmsMessage>,
}

impl SmsMessagesBody {
    /// Group the messages by `thread_id` so the Workbench panel
    /// can render one row per conversation. Order within each
    /// thread matches insertion order (callers feed in upstream
    /// emission order; receivers re-sort by `date` if needed).
    #[must_use]
    pub fn by_thread(&self) -> std::collections::BTreeMap<i64, Vec<&SmsMessage>> {
        let mut out: std::collections::BTreeMap<i64, Vec<&SmsMessage>> =
            std::collections::BTreeMap::new();
        for m in &self.messages {
            out.entry(m.thread_id).or_default().push(m);
        }
        out
    }
}

/// Build a `kdeconnect.sms.messages` packet from a list of
/// messages.
#[must_use]
pub fn sms_messages_packet(id_ms: i64, messages: Vec<SmsMessage>) -> Packet {
    Packet {
        id: id_ms,
        kind: "kdeconnect.sms.messages".to_string(),
        body: serde_json::to_value(SmsMessagesBody { messages })
            .expect("SmsMessagesBody is always JSON-serializable"),
        mde_caps: None,
        payload_size: None,
        payload_transfer_info: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::from_packet_body;

    fn sample_msg(id: i64, thread_id: i64, body: &str) -> SmsMessage {
        SmsMessage {
            id,
            thread_id,
            body: body.to_string(),
            address: "+1234567890".to_string(),
            date: 1_700_000_000_000,
            kind: 1,
            read: false,
        }
    }

    #[test]
    fn sms_message_serializes_type_field_under_rename() {
        // Upstream uses `type` (reserved Rust keyword) so the body
        // struct uses #[serde(rename = "type")] for `kind`.
        let m = sample_msg(1, 1, "hi");
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains(r#""type":1"#));
        assert!(!s.contains(r#""kind""#), "Rust field name leaked: {s}");
    }

    #[test]
    fn sms_message_serializes_thread_id_as_camel_case() {
        let m = sample_msg(1, 42, "hi");
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains(r#""threadId":42"#));
    }

    #[test]
    fn sms_messages_body_groups_by_thread() {
        let body = SmsMessagesBody {
            messages: vec![
                sample_msg(1, 100, "a"),
                sample_msg(2, 200, "b"),
                sample_msg(3, 100, "c"),
                sample_msg(4, 100, "d"),
            ],
        };
        let by = body.by_thread();
        assert_eq!(by.len(), 2);
        assert_eq!(by[&100].len(), 3);
        assert_eq!(by[&200].len(), 1);
        // Order preserved within thread.
        assert_eq!(by[&100][0].body, "a");
        assert_eq!(by[&100][1].body, "c");
        assert_eq!(by[&100][2].body, "d");
    }

    #[test]
    fn sms_packet_round_trips_via_wire() {
        let msgs = vec![sample_msg(1, 1, "hi"), sample_msg(2, 1, "back")];
        let p = sms_messages_packet(1, msgs.clone());
        let wire = serde_json::to_string(&p).unwrap();
        let decoded: Packet = serde_json::from_str(&wire).unwrap();
        let body: SmsMessagesBody = from_packet_body(&decoded).unwrap();
        assert_eq!(body.messages, msgs);
    }

    #[test]
    fn sms_packet_kind_matches_upstream_messages_suffix() {
        let p = sms_messages_packet(1, vec![]);
        assert_eq!(p.kind, "kdeconnect.sms.messages");
        assert_eq!(p.kind, crate::plugins::PluginKind::Sms.packet_kind());
    }

    #[test]
    fn sms_message_address_defaults_to_empty() {
        // Sent-by-self messages on the phone have no `address`
        // field — must deserialize without error.
        let raw = r#"{"id":1,"threadId":1,"body":"hi","date":0,"type":2}"#;
        let m: SmsMessage = serde_json::from_str(raw).unwrap();
        assert_eq!(m.address, "");
        assert!(!m.read);
    }

    // KDC2-2.18 — SmsPlugin Plugin trait impl
    use crate::plugins::{Plugin, PluginContext, PluginKind};

    #[test]
    fn sms_plugin_queues_inbound_message_list() {
        let mut plugin = SmsPlugin::new();
        let ctx = PluginContext::new("phone", true);
        let msgs = vec![sample_msg(1, 1, "hi"), sample_msg(2, 1, "back")];
        plugin.process(&sms_messages_packet(1, msgs.clone()), &ctx);
        let drained = plugin.take_received();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].messages.len(), 2);
    }
}

/// KDC2-2.18 — SmsPlugin. Queues inbound SmsMessagesBody packets;
/// host's SMS view groups + renders by thread.
#[derive(Debug, Default)]
pub struct SmsPlugin {
    received: Vec<SmsMessagesBody>,
    handles: [&'static str; 1],
}

impl SmsPlugin {
    /// New empty plugin.
    #[must_use]
    pub fn new() -> Self {
        Self {
            received: Vec::new(),
            handles: ["kdeconnect.sms.messages"],
        }
    }
    /// Drain every queued SMS message-list body.
    #[must_use]
    pub fn take_received(&mut self) -> Vec<SmsMessagesBody> {
        std::mem::take(&mut self.received)
    }
    /// Items currently queued.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.received.len()
    }
}

impl crate::plugins::Plugin for SmsPlugin {
    fn kind(&self) -> crate::plugins::PluginKind {
        crate::plugins::PluginKind::Sms
    }
    fn handles(&self) -> &[&'static str] {
        &self.handles
    }
    fn process(
        &mut self,
        packet: &crate::wire::Packet,
        _ctx: &crate::plugins::PluginContext,
    ) -> Vec<crate::wire::Packet> {
        if let Ok(body) = crate::plugins::from_packet_body::<SmsMessagesBody>(packet) {
            self.received.push(body);
        }
        Vec::new()
    }
}
