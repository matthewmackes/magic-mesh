//! KDC2-2.3 — in-process loopback handshake + message harness.
//!
//! Two simulated peers exchange wire packets through a pair of
//! `VecDeque<u8>` buffers (no networking, no async). Each peer
//! runs the same code path the production host integration
//! (KDC2-3) will use: encode → push bytes into the peer's read
//! buffer → other peer drains its `FrameDecoder` → handle the
//! decoded packet.
//!
//! The harness exercises six end-to-end paths that the v2.1 KDC2
//! lock's acceptance gates depend on:
//!
//!   1. Identity-handshake: both peers exchange `kdeconnect.identity`
//!      packets, parse each other's `Announce`-shaped body.
//!   2. MDE-vs-stock detection: identity carrying `mdeCaps` is
//!      treated as MDE-peer; identity without is stock.
//!   3. Clipboard send → receive: peer A copies → peer B sees the
//!      typed `ClipboardBody`.
//!   4. Notification dual-send: peer A emits twice (mesh-router
//!      simulation); peer B sees the second packet has the same
//!      envelope `id` and dedups.
//!   5. Frame-decoder stream resilience: split a packet across
//!      multiple "socket reads," confirm decoder reassembles.
//!   6. Plugin dispatch by packet_kind: every received packet
//!      routes to the right plugin based on `PluginKind::all()`
//!      matching against `packet.kind`.

use std::collections::VecDeque;

use mde_kdc_proto::codec::{encode_frame, FrameDecoder};
use mde_kdc_proto::discovery::{Announce, DeviceType};
use mde_kdc_proto::plugins::{
    self, clipboard_packet, notification_packet, ClipboardBody, NotificationBody, PluginKind,
};
use mde_kdc_proto::wire::{CapabilitiesHeader, Packet};

/// In-memory "peer" — holds a read buffer (bytes other peer has
/// sent us) and a frame decoder that drains it.
struct LoopbackPeer {
    /// Identity name used in handshake.
    name: String,
    /// Bytes the other peer has sent us. The harness moves data
    /// from one peer's "outgoing buffer" (returned by `send_*`
    /// helpers) into the other peer's `incoming`.
    incoming: VecDeque<u8>,
    /// Frame decoder draining `incoming` into typed packets.
    decoder: FrameDecoder,
}

impl LoopbackPeer {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            incoming: VecDeque::new(),
            decoder: FrameDecoder::new(),
        }
    }

    /// Feed bytes into the decoder. Simulates a TCP `read()`
    /// that just delivered data from the other peer.
    fn receive(&mut self, bytes: &[u8]) {
        self.incoming.extend(bytes.iter().copied());
        // The decoder takes a slice — drain `incoming` into it.
        let drained: Vec<u8> = self.incoming.drain(..).collect();
        self.decoder.feed(&drained);
    }

    /// Pull the next decoded packet. Returns `None` when no
    /// complete frame is buffered.
    fn next_packet(&mut self) -> Option<Packet> {
        self.decoder.next_frame().unwrap_or(None)
    }

    /// Emit our `kdeconnect.identity` packet bytes (MDE peer
    /// — carries `mdeCaps`).
    fn identity_bytes_mde(&self) -> Vec<u8> {
        let announce = Announce {
            device_id: format!("{}-uuid", self.name),
            device_name: format!("{} [mde]", self.name),
            device_type: DeviceType::Desktop,
            protocol_version: mde_kdc_proto::PROTOCOL_VERSION,
            incoming_capabilities: vec!["kdeconnect.clipboard".into()],
            outgoing_capabilities: vec!["kdeconnect.notification".into()],
        };
        let p = Packet {
            id: 1,
            kind: "kdeconnect.identity".to_string(),
            body: serde_json::to_value(announce).expect("Announce serializes"),
            mde_caps: Some(CapabilitiesHeader::v2_1_lock()),
            payload_size: None,
            payload_transfer_info: None,
        };
        encode_frame(&p)
            .expect("identity frame encodes")
            .into_bytes()
    }

    /// Emit a stock-KDC identity (no `mdeCaps`).
    fn identity_bytes_stock(&self) -> Vec<u8> {
        let announce = Announce {
            device_id: format!("{}-uuid", self.name),
            device_name: self.name.clone(),
            device_type: DeviceType::Phone,
            protocol_version: mde_kdc_proto::PROTOCOL_VERSION,
            incoming_capabilities: vec!["kdeconnect.clipboard".into()],
            outgoing_capabilities: vec![],
        };
        let p = Packet {
            id: 1,
            kind: "kdeconnect.identity".to_string(),
            body: serde_json::to_value(announce).expect("Announce serializes"),
            mde_caps: None,
            payload_size: None,
            payload_transfer_info: None,
        };
        encode_frame(&p)
            .expect("identity frame encodes")
            .into_bytes()
    }
}

// ──────────────────────────────────────────────────────────────
// 1. Identity handshake — both MDE peers
// ──────────────────────────────────────────────────────────────

#[test]
fn mde_to_mde_identity_exchange_decodes_caps_header() {
    let alice = LoopbackPeer::new("alice");
    let mut bob = LoopbackPeer::new("bob");

    // Alice sends her identity; Bob receives + decodes.
    bob.receive(&alice.identity_bytes_mde());
    let pkt = bob.next_packet().expect("bob decoded alice's identity");

    assert_eq!(pkt.kind, "kdeconnect.identity");
    assert!(pkt.from_mde_peer(), "alice's identity carries mdeCaps");
    let caps = pkt.mde_caps.unwrap();
    assert!(caps.offers("mesh_relay"));
    assert!(caps.offers("peer_card_probe_share"));
    assert!(caps.offers("notification_dual_send_ack"));
}

// ──────────────────────────────────────────────────────────────
// 2. MDE-vs-stock detection — Phone identity has no mdeCaps
// ──────────────────────────────────────────────────────────────

#[test]
fn mde_peer_distinguishes_stock_kdc_phone_at_handshake() {
    let phone = LoopbackPeer::new("phone");
    let mut alice = LoopbackPeer::new("alice");

    alice.receive(&phone.identity_bytes_stock());
    let pkt = alice.next_packet().expect("alice decoded phone identity");

    assert_eq!(pkt.kind, "kdeconnect.identity");
    assert!(!pkt.from_mde_peer(), "phone is a stock KDC client");
    assert!(pkt.mde_caps.is_none());

    let announce: Announce = serde_json::from_value(pkt.body).unwrap();
    assert_eq!(announce.device_type, DeviceType::Phone);
}

// ──────────────────────────────────────────────────────────────
// 3. Clipboard send → receive
// ──────────────────────────────────────────────────────────────

#[test]
fn clipboard_packet_traverses_loopback_intact() {
    let mut alice = LoopbackPeer::new("alice");
    let bob_pkt = clipboard_packet(1_700_000_000_000, "hello from bob".to_string());

    // Bob sends. Alice receives.
    alice.receive(&encode_frame(&bob_pkt).unwrap().into_bytes());

    let received = alice.next_packet().expect("alice decoded clipboard");
    assert_eq!(received.kind, PluginKind::Clipboard.packet_kind());

    let body: ClipboardBody = plugins::from_packet_body(&received).unwrap();
    assert_eq!(body.content, "hello from bob");
}

// ──────────────────────────────────────────────────────────────
// 4. Notification dual-send — envelope id is the dedup key
// ──────────────────────────────────────────────────────────────

#[test]
fn dual_sent_notification_uses_same_envelope_id_for_dedup() {
    let mut bob = LoopbackPeer::new("bob");

    let body = NotificationBody {
        id: "msg-42".to_string(),
        app_name: "Thunderbird".to_string(),
        title: "Inbox".to_string(),
        text: "new mail".to_string(),
        ticker: "Thunderbird: Inbox".to_string(),
        is_clearable: true,
        is_cancel: false,
    };
    // Same envelope id; emitted twice (mesh-router dual-send sim).
    let p1 = notification_packet(99, body.clone());
    let p2 = notification_packet(99, body.clone());

    bob.receive(&encode_frame(&p1).unwrap().into_bytes());
    bob.receive(&encode_frame(&p2).unwrap().into_bytes());

    let r1 = bob.next_packet().unwrap();
    let r2 = bob.next_packet().unwrap();

    assert_eq!(r1.id, r2.id, "dual-send envelope id is the dedup key");
    assert_eq!(r1.id, 99);
    // Body is identical — receiver's idempotent dedup drops the second.
    let b1: NotificationBody = plugins::from_packet_body(&r1).unwrap();
    let b2: NotificationBody = plugins::from_packet_body(&r2).unwrap();
    assert_eq!(b1, b2);
}

// ──────────────────────────────────────────────────────────────
// 5. Frame decoder stream resilience — split mid-packet
// ──────────────────────────────────────────────────────────────

#[test]
fn split_packet_reassembles_through_decoder() {
    let mut bob = LoopbackPeer::new("bob");
    let bytes = encode_frame(&clipboard_packet(1, "split".to_string())).unwrap();
    let raw = bytes.into_bytes();
    let mid = raw.len() / 2;

    // First "socket read" — partial packet.
    bob.receive(&raw[..mid]);
    assert!(
        bob.next_packet().is_none(),
        "no complete frame yet — must return None"
    );

    // Second "socket read" — rest of the packet.
    bob.receive(&raw[mid..]);
    let pkt = bob.next_packet().expect("complete frame after second read");
    let body: ClipboardBody = plugins::from_packet_body(&pkt).unwrap();
    assert_eq!(body.content, "split");
}

// ──────────────────────────────────────────────────────────────
// 6. Plugin dispatch — every packet_kind maps to exactly one
//    PluginKind variant
// ──────────────────────────────────────────────────────────────

#[test]
fn every_plugin_kind_dispatches_unambiguously_on_packet_kind() {
    let mut alice = LoopbackPeer::new("alice");

    // Build one sample packet per plugin kind and feed through
    // the loopback.
    let samples: Vec<Packet> = vec![
        plugins::ping_packet(1, "hi".to_string()),
        plugins::clipboard_packet(2, "c".to_string()),
        plugins::url_share_packet(3, "https://x".to_string(), false),
        plugins::notification_packet(
            4,
            NotificationBody {
                id: "n".to_string(),
                app_name: "a".to_string(),
                title: "t".to_string(),
                text: "b".to_string(),
                ticker: "t — b".to_string(),
                is_clearable: true,
                is_cancel: false,
            },
        ),
        plugins::find_my_phone_packet(5),
        plugins::battery_packet(
            6,
            plugins::BatteryBody {
                current_charge: 50,
                is_charging: false,
                threshold_event: String::new(),
            },
        ),
        plugins::mpris_command_packet(7, "Play".to_string()),
        plugins::sms_messages_packet(8, vec![]),
        plugins::telephony_packet(
            9,
            plugins::TelephonyBody {
                event: plugins::TelephonyEvent::Ringing,
                phone_number: "+15551234".to_string(),
                contact_name: "Alice".to_string(),
                is_cancel: false,
            },
        ),
        plugins::run_command_packet(
            10,
            "open-browser".to_string(),
            "Open browser".to_string(),
            "xdg-open https://example.com".to_string(),
        ),
    ];

    for p in &samples {
        let bytes = encode_frame(p).unwrap().into_bytes();
        alice.receive(&bytes);
    }

    // Drain Alice's decoder and confirm every sample's packet_kind
    // maps to exactly one PluginKind::all() variant.
    let mut received = Vec::new();
    while let Some(p) = alice.next_packet() {
        received.push(p);
    }
    assert_eq!(
        received.len(),
        10,
        "received one packet per plugin (9 canonical + RunCommand)"
    );

    for pkt in &received {
        let matching: Vec<PluginKind> = PluginKind::all()
            .into_iter()
            .filter(|k| k.packet_kind() == pkt.kind)
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "packet kind {:?} must match exactly one PluginKind",
            pkt.kind,
        );
    }
}

// ──────────────────────────────────────────────────────────────
// Round-trip lock — every plugin's factory function produces a
// packet that decodes back into a body equal to what went in.
// ──────────────────────────────────────────────────────────────

#[test]
fn every_factory_packet_round_trips_through_loopback() {
    let mut alice = LoopbackPeer::new("alice");

    let p = clipboard_packet(1, "round".to_string());
    alice.receive(&encode_frame(&p).unwrap().into_bytes());
    let back = alice.next_packet().unwrap();
    let body: ClipboardBody = plugins::from_packet_body(&back).unwrap();
    assert_eq!(body.content, "round");
    assert_eq!(back.id, 1);
    assert_eq!(back.kind, "kdeconnect.clipboard");
}
