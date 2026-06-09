//! KDC2-2.5 clipboard plugin — `kdeconnect.clipboard` packet body.
//!
//! Stock KDE Connect's clipboard plugin sends a packet of kind
//! `kdeconnect.clipboard` with a single body field `content`
//! (UTF-8 string). KDC2 ships the matching body type plus the
//! generic [`from_packet_body`] downcast helper that other plugins
//! reuse.
//!
//! Wire compatibility note: upstream sometimes also emits
//! `kdeconnect.clipboard.connect` — the same body shape, but only
//! sent on connection-handshake to push the current clipboard
//! contents at the new peer. The body is identical so the same
//! [`ClipboardBody`] type covers both packet kinds.

use serde::{Deserialize, Serialize};

use crate::wire::Packet;

/// `kdeconnect.clipboard` (+ `.connect`) packet body. UTF-8 text
/// payload, no length cap on the wire — receivers enforce their
/// own size limit before applying.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardBody {
    /// The clipboard content. UTF-8 only; binary payloads use
    /// the `share.request` plugin (file transfer).
    pub content: String,
}

/// Generic downcast helper: extract a typed body `B` from a
/// [`Packet`]. Used by every plugin's `on_packet` implementation
/// to interpret the wire's `serde_json::Value` body without
/// reimplementing the same JSON re-serialize → deserialize dance
/// every time.
///
/// The function pattern (rather than a `Packet::body_as::<B>()`
/// method) keeps the wire module pluginsuncoupled — see the
/// crate-level doc on the `protocol → router → daemon → surface`
/// layering rule.
pub fn from_packet_body<B>(packet: &Packet) -> Result<B, serde_json::Error>
where
    B: for<'de> Deserialize<'de>,
{
    serde_json::from_value(packet.body.clone())
}

/// Build a `kdeconnect.clipboard` packet from clipboard text.
/// Used by the host integration (KDC2-3) when the user copies
/// text on a local MDE peer.
///
/// `id_ms` is the millisecond Unix timestamp the receiver uses
/// for deduplication; callers should pass `chrono::Utc::now()
/// .timestamp_millis()` (or equivalent) so paired devices can
/// dedup dual-sent copies via the mesh router.
#[must_use]
pub fn clipboard_packet(id_ms: i64, content: String) -> Packet {
    Packet {
        id: id_ms,
        kind: "kdeconnect.clipboard".to_string(),
        body: serde_json::json!({"content": content}),
        mde_caps: None,
        payload_size: None,
        payload_transfer_info: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_packet_serializes_with_upstream_field_names() {
        let p = clipboard_packet(123, "hello".to_string());
        let s = serde_json::to_string(&p).unwrap();
        // Wire compatibility: upstream Android client deserializes
        // `content` verbatim.
        assert!(s.contains(r#""content":"hello""#));
        assert!(s.contains(r#""type":"kdeconnect.clipboard""#));
        assert!(s.contains(r#""id":123"#));
    }

    #[test]
    fn from_packet_body_extracts_clipboard_payload() {
        let p = clipboard_packet(1, "extracted".to_string());
        let body: ClipboardBody = from_packet_body(&p).unwrap();
        assert_eq!(body.content, "extracted");
    }

    #[test]
    fn from_packet_body_round_trips_via_wire() {
        // Encode → decode through serde_json::to_string + from_str
        // (simulating a real send/recv hop) then downcast.
        let p = clipboard_packet(42, "round-trip".to_string());
        let wire = serde_json::to_string(&p).unwrap();
        let decoded: Packet = serde_json::from_str(&wire).unwrap();
        let body: ClipboardBody = from_packet_body(&decoded).unwrap();
        assert_eq!(body.content, "round-trip");
    }

    #[test]
    fn from_packet_body_rejects_mismatched_shape() {
        // Body that's the wrong shape (missing `content`) surfaces
        // a serde error, not a panic. Plugins use this to detect
        // a malformed peer + drop the packet.
        let p = Packet {
            id: 1,
            kind: "kdeconnect.clipboard".to_string(),
            body: serde_json::json!({"wrong_field": 42}),
            mde_caps: None,
            payload_size: None,
            payload_transfer_info: None,
        };
        let result: Result<ClipboardBody, _> = from_packet_body(&p);
        assert!(result.is_err());
    }

    #[test]
    fn clipboard_body_round_trips_through_json() {
        let b = ClipboardBody {
            content: "with newlines\n and tabs\t and unicode 🦀".to_string(),
        };
        let s = serde_json::to_string(&b).unwrap();
        let back: ClipboardBody = serde_json::from_str(&s).unwrap();
        assert_eq!(back, b);
    }

    #[test]
    fn clipboard_packet_id_lands_in_dedup_field() {
        // The `id` is the dedup key — two packets with the same
        // id from the same peer are the same logical clipboard
        // event (mesh-router dual-send relies on this).
        let p1 = clipboard_packet(7, "x".to_string());
        let p2 = clipboard_packet(7, "x".to_string());
        assert_eq!(p1.id, p2.id);
        assert_eq!(p1.body, p2.body);
    }

    // ─────────────────────────────────────────────────────────
    // KDC2-2.14 — ClipboardPlugin (Plugin trait impl)
    // ─────────────────────────────────────────────────────────

    use crate::plugins::{Plugin, PluginContext, PluginKind};

    #[test]
    fn clipboard_plugin_kind_and_handles_match_token() {
        let p = ClipboardPlugin::new();
        assert_eq!(p.kind(), PluginKind::Clipboard);
        assert_eq!(p.handles(), &["kdeconnect.clipboard"]);
    }

    #[test]
    fn clipboard_plugin_queues_inbound_content() {
        let mut plugin = ClipboardPlugin::new();
        let ctx = PluginContext::new("alice", true);
        plugin.process(&clipboard_packet(1, "hello".into()), &ctx);
        let drained = plugin.take_received();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].content, "hello");
    }

    #[test]
    fn clipboard_plugin_drops_malformed_without_panic() {
        let mut plugin = ClipboardPlugin::new();
        let ctx = PluginContext::new("alice", true);
        let bad = Packet {
            id: 1,
            kind: "kdeconnect.clipboard".to_string(),
            body: serde_json::json!({"not_content": 42}),
            mde_caps: None,
            payload_size: None,
            payload_transfer_info: None,
        };
        plugin.process(&bad, &ctx);
        assert_eq!(plugin.pending_count(), 0);
    }

    // BUS-5.9 — mesh→phone outbound direction.

    #[test]
    fn push_clipboard_queues_kdeconnect_packet() {
        let mut plugin = ClipboardPlugin::new();
        plugin.push_clipboard("from mesh".to_string());
        assert_eq!(plugin.outbound_count(), 1);
        let pkts = plugin.take_outbound();
        assert_eq!(pkts.len(), 1);
        assert_eq!(pkts[0].kind, "kdeconnect.clipboard");
        let body: ClipboardBody = from_packet_body(&pkts[0]).unwrap();
        assert_eq!(body.content, "from mesh");
    }

    #[test]
    fn take_outbound_drains_queue() {
        let mut plugin = ClipboardPlugin::new();
        plugin.push_clipboard("a".to_string());
        plugin.push_clipboard("b".to_string());
        assert_eq!(plugin.outbound_count(), 2);
        let first = plugin.take_outbound();
        assert_eq!(first.len(), 2);
        assert_eq!(plugin.outbound_count(), 0);
    }

    #[test]
    fn outbound_and_received_are_independent_queues() {
        let mut plugin = ClipboardPlugin::new();
        let ctx = PluginContext::new("phone", true);
        // Simulate a phone-to-mesh inbound.
        plugin.process(&clipboard_packet(1, "from phone".into()), &ctx);
        // Queue a mesh-to-phone outbound.
        plugin.push_clipboard("from mesh".to_string());

        assert_eq!(plugin.pending_count(), 1);
        assert_eq!(plugin.outbound_count(), 1);

        let received = plugin.take_received();
        assert_eq!(received[0].content, "from phone");
        let outbound = plugin.take_outbound();
        let out_body: ClipboardBody = from_packet_body(&outbound[0]).unwrap();
        assert_eq!(out_body.content, "from mesh");
    }

    #[test]
    fn push_clipboard_id_is_recent_unix_ms() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let mut plugin = ClipboardPlugin::new();
        plugin.push_clipboard("ts-check".to_string());
        let pkts = plugin.take_outbound();
        let after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert!(pkts[0].id >= before, "id must be at or after call time");
        assert!(pkts[0].id <= after, "id must be at or before now");
    }
}

// ────────────────────────────────────────────────────────────────
// KDC2-2.14 — ClipboardPlugin (Plugin trait impl, adapter pattern)
// ────────────────────────────────────────────────────────────────

/// `Plugin` impl that mirrors inbound clipboard content and queues
/// outbound clipboard packets for the mesh→phone direction.
///
/// Adapter pattern (same as `NotificationPlugin`): the protocol
/// crate stays pure. Host (`mde-kdc`) drains via:
/// - `take_received()` — phone→mesh direction; host writes each body
///   to the `clipboard/sync` Bus topic (`clip_bridge::phone_to_bus`).
/// - `take_outbound()` — mesh→phone direction; host sends each
///   queued packet to the paired phone over TLS. Packets are added
///   via `push_clipboard()` when the host's `clip_bridge` detects a
///   new Bus entry from another mesh peer.
#[derive(Debug, Default)]
pub struct ClipboardPlugin {
    received: Vec<ClipboardBody>,
    /// Proactive outbound packets queued for the paired phone
    /// (mesh clipboard → phone). Drained by the host on each tick.
    outbound: Vec<crate::wire::Packet>,
    handles: [&'static str; 1],
}

impl ClipboardPlugin {
    /// New empty plugin.
    #[must_use]
    pub fn new() -> Self {
        Self {
            received: Vec::new(),
            outbound: Vec::new(),
            handles: ["kdeconnect.clipboard"],
        }
    }

    /// Drain every received clipboard body (phone → mesh).
    #[must_use]
    pub fn take_received(&mut self) -> Vec<ClipboardBody> {
        std::mem::take(&mut self.received)
    }

    /// Items currently queued (phone → mesh).
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.received.len()
    }

    /// Queue `content` as a `kdeconnect.clipboard` packet to send to
    /// the paired phone (mesh → phone). Caller passes new mesh
    /// clipboard text detected by `mde_kdc::clip_bridge`. The host
    /// drains via `take_outbound()` and writes to the phone's TLS
    /// socket.
    pub fn push_clipboard(&mut self, content: String) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let id_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        self.outbound.push(clipboard_packet(id_ms, content));
    }

    /// Drain every outbound clipboard packet (mesh → phone).
    #[must_use]
    pub fn take_outbound(&mut self) -> Vec<crate::wire::Packet> {
        std::mem::take(&mut self.outbound)
    }

    /// Outbound items currently queued.
    #[must_use]
    pub fn outbound_count(&self) -> usize {
        self.outbound.len()
    }
}

impl crate::plugins::Plugin for ClipboardPlugin {
    fn kind(&self) -> crate::plugins::PluginKind {
        crate::plugins::PluginKind::Clipboard
    }

    fn handles(&self) -> &[&'static str] {
        &self.handles
    }

    fn process(
        &mut self,
        packet: &crate::wire::Packet,
        _ctx: &crate::plugins::PluginContext,
    ) -> Vec<crate::wire::Packet> {
        if let Ok(body) = from_packet_body::<ClipboardBody>(packet) {
            self.received.push(body);
        }
        // Proactive outbound packets are drained by the host via
        // take_outbound(), not as responses to inbound packets.
        Vec::new()
    }
}
