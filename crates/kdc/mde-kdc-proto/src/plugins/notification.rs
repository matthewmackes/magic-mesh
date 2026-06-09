//! KDC2-2.6 notification plugin — `kdeconnect.notification` body.
//!
//! Carries a mirrored notification from one peer to another. Stock
//! KDE Connect uses this to surface phone notifications on a
//! paired desktop; MDE peer pairs use it for cross-machine
//! notification mirror (the v2.1 KDC2 lock's
//! `notification_dual_send_ack` capability bounds the dual-send
//! semantics).
//!
//! Upstream's body shape uses camelCase keys (`isClearable`,
//! `appName`, `ticker`). KDC2 matches verbatim for wire compat.

use serde::{Deserialize, Serialize};

use crate::wire::Packet;

/// `kdeconnect.notification` body. All fields use camelCase wire
/// names to stay byte-compatible with stock Android KDE Connect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationBody {
    /// Stable identifier on the source device. Receivers use this
    /// as the dedup key when the same notification is dual-sent
    /// via two transports.
    pub id: String,
    /// Application that emitted the notification (e.g.
    /// `org.thunderbird.Thunderbird`, `com.google.android.gm`).
    pub app_name: String,
    /// Headline / title.
    pub title: String,
    /// Body text.
    pub text: String,
    /// Combined preview line ("AppName: title — body"). Upstream
    /// emits this as a convenience for tray-style renderings; KDC2
    /// keeps the field so older Android clients that read only
    /// `ticker` still get the full content.
    pub ticker: String,
    /// True when the user can dismiss the notification on the
    /// source device. Drives the dismiss button affordance in
    /// the Workbench Notifications panel.
    pub is_clearable: bool,
    /// True when this packet is a removal — receiver should drop
    /// the matching notification from its own UI.
    #[serde(default)]
    pub is_cancel: bool,
}

/// Build a `kdeconnect.notification` packet from a complete body.
///
/// `id_ms` is the wire-level millisecond timestamp the receiver
/// uses for envelope-level deduplication (separate from the body
/// `id` field, which is the per-notification dedup key).
#[must_use]
pub fn notification_packet(id_ms: i64, body: NotificationBody) -> Packet {
    Packet {
        id: id_ms,
        kind: "kdeconnect.notification".to_string(),
        body: serde_json::to_value(body).expect("NotificationBody is always JSON-serializable"),
        mde_caps: None,
        payload_size: None,
        payload_transfer_info: None,
    }
}

// ────────────────────────────────────────────────────────────────
// KDC2-2.13 — NotificationPlugin (Plugin trait impl)
// ────────────────────────────────────────────────────────────────

/// `Plugin` impl that mirrors inbound notification packets.
///
/// Adapter pattern: the protocol crate stays pure (no I/O,
/// no notification-daemon calls). Received packets are decoded
/// + pushed into an internal queue; the host crate
/// (`mde-kdc`) polls `take_received()` from its event loop and
/// forwards each `NotificationBody` to the OS notification
/// daemon (mako on Wayland).
///
/// Outgoing notification packets (e.g. dismiss-request relay)
/// are constructed via the existing `notification_packet()`
/// factory — this plugin's `process()` returns `Vec::new()`
/// because every wire-level notification is one-way (the
/// receiver applies it locally; no reply packet flows back).
#[derive(Debug, Default)]
pub struct NotificationPlugin {
    received: Vec<NotificationBody>,
    handles: [&'static str; 1],
}

impl NotificationPlugin {
    /// New empty plugin.
    #[must_use]
    pub fn new() -> Self {
        Self {
            received: Vec::new(),
            handles: ["kdeconnect.notification"],
        }
    }

    /// Drain every received notification body. The host calls
    /// this on each tick of its event loop + delivers the
    /// bodies to the OS notification daemon. Drains rather than
    /// clones so the queue stays bounded.
    #[must_use]
    pub fn take_received(&mut self) -> Vec<NotificationBody> {
        std::mem::take(&mut self.received)
    }

    /// How many notifications are currently queued. Used by
    /// tests + the host's `mded healthz` instrumentation.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.received.len()
    }
}

impl crate::plugins::Plugin for NotificationPlugin {
    fn kind(&self) -> crate::plugins::PluginKind {
        crate::plugins::PluginKind::Notification
    }

    fn handles(&self) -> &[&'static str] {
        &self.handles
    }

    fn process(&mut self, packet: &Packet, _ctx: &crate::plugins::PluginContext) -> Vec<Packet> {
        // Best-effort decode — a malformed body from a peer
        // drops the packet rather than poisoning the queue.
        if let Ok(body) = crate::plugins::from_packet_body::<NotificationBody>(packet) {
            self.received.push(body);
        }
        // Notifications are one-way at the wire layer — no
        // response packet.
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::from_packet_body;

    fn sample() -> NotificationBody {
        NotificationBody {
            id: "msg-42".to_string(),
            app_name: "Thunderbird".to_string(),
            title: "Inbox — 1 new".to_string(),
            text: "Pull request review requested".to_string(),
            ticker: "Thunderbird: Inbox — 1 new".to_string(),
            is_clearable: true,
            is_cancel: false,
        }
    }

    #[test]
    fn notification_body_serializes_with_camel_case_keys() {
        // Wire compat: stock Android client expects `appName`,
        // `isClearable`, `isCancel` — NOT `app_name` /
        // `is_clearable` / `is_cancel`.
        let s = serde_json::to_string(&sample()).unwrap();
        assert!(s.contains(r#""appName":"Thunderbird""#));
        assert!(s.contains(r#""isClearable":true"#));
        assert!(s.contains(r#""isCancel":false"#));
    }

    #[test]
    fn notification_packet_round_trips_via_wire() {
        let p = notification_packet(1_700_000_000_000, sample());
        let wire = serde_json::to_string(&p).unwrap();
        let decoded: Packet = serde_json::from_str(&wire).unwrap();
        let body: NotificationBody = from_packet_body(&decoded).unwrap();
        assert_eq!(body, sample());
    }

    #[test]
    fn is_cancel_defaults_to_false_for_back_compat() {
        // Older KDE Connect versions don't emit `isCancel` at all
        // — the field must default to `false` so deserialize
        // doesn't fail.
        let raw = r#"{"id":"x","appName":"App","title":"t","text":"b","ticker":"t — b","isClearable":true}"#;
        let body: NotificationBody = serde_json::from_str(raw).unwrap();
        assert!(!body.is_cancel);
    }

    #[test]
    fn notification_packet_kind_matches_plugin_token() {
        // KDC2-2.1's `PluginKind::Notification.packet_kind()` must
        // exactly equal the packet's `kind` field — otherwise the
        // host's dispatch table never routes notifications to
        // their handler.
        let p = notification_packet(1, sample());
        assert_eq!(
            p.kind,
            crate::plugins::PluginKind::Notification.packet_kind()
        );
    }

    // ─────────────────────────────────────────────────────────
    // KDC2-2.13 — NotificationPlugin (Plugin trait impl)
    // ─────────────────────────────────────────────────────────

    use crate::plugins::{Plugin, PluginContext, PluginKind};

    #[test]
    fn plugin_kind_and_handles_match_token() {
        let p = NotificationPlugin::new();
        assert_eq!(p.kind(), PluginKind::Notification);
        assert_eq!(p.handles(), &["kdeconnect.notification"]);
    }

    #[test]
    fn plugin_queues_received_notification() {
        let mut plugin = NotificationPlugin::new();
        let ctx = PluginContext::new("alice", true);
        let pkt = notification_packet(1, sample());
        let response = plugin.process(&pkt, &ctx);
        assert!(response.is_empty(), "notifications are one-way at wire");
        assert_eq!(plugin.pending_count(), 1);
    }

    #[test]
    fn take_received_drains_the_queue() {
        let mut plugin = NotificationPlugin::new();
        let ctx = PluginContext::new("alice", true);
        plugin.process(&notification_packet(1, sample()), &ctx);
        plugin.process(&notification_packet(2, sample()), &ctx);
        let drained = plugin.take_received();
        assert_eq!(drained.len(), 2);
        // Drain → queue empty.
        assert_eq!(plugin.pending_count(), 0);
    }

    #[test]
    fn plugin_drops_malformed_packet_without_panic() {
        let mut plugin = NotificationPlugin::new();
        let ctx = PluginContext::new("alice", true);
        let bad = Packet {
            id: 1,
            kind: "kdeconnect.notification".to_string(),
            body: serde_json::json!({"wrong_field": 42}),
            mde_caps: None,
            payload_size: None,
            payload_transfer_info: None,
        };
        let response = plugin.process(&bad, &ctx);
        assert!(response.is_empty());
        // Malformed → silently dropped, queue stays empty.
        assert_eq!(plugin.pending_count(), 0);
    }
}
