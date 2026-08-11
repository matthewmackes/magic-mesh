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

use std::collections::VecDeque;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

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

/// Maximum UTF-8 bytes accepted for one clipboard text value.
///
/// This is the existing KDC/guest transport bound. It is measured in
/// encoded bytes, not Unicode scalar values, so a value accepted here is safe
/// to carry through the JSON/KDC text lane without silently truncating UTF-8.
pub const MAX_CLIPBOARD_TEXT_BYTES: usize = 1024 * 1024;

/// Maximum serialized JSON body inspected by the clipboard admission gate.
///
/// The text limit is the authoritative content bound. This smaller framing
/// allowance prevents an attacker from presenting an arbitrarily large JSON
/// object to the plugin before the typed body can be decoded.
pub const MAX_CLIPBOARD_BODY_BYTES: usize = MAX_CLIPBOARD_TEXT_BYTES * 6 + 256;

/// Maximum UTF-8 bytes in a local seat identifier.
pub const MAX_CLIPBOARD_SEAT_BYTES: usize = 128;

/// A typed reason why clipboard text was not admitted to a queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardRejection {
    /// The dispatch context does not identify a paired peer.
    Unauthorized {
        /// Peer that attempted the inbound clipboard ingress.
        peer_id: String,
    },
    /// Empty or whitespace-only text is not a clipboard event.
    Blank,
    /// The text exceeds [`MAX_CLIPBOARD_TEXT_BYTES`].
    TooLarge {
        /// Actual UTF-8 byte length.
        bytes: usize,
        /// Bound that was exceeded.
        max_bytes: usize,
    },
    /// The serialized packet body is larger than the clipboard admission
    /// framing allowance.
    BodyTooLarge {
        /// Actual serialized JSON byte length.
        bytes: usize,
        /// Bound that was exceeded.
        max_bytes: usize,
    },
    /// A clipboard packet attempted to use a secondary payload channel.
    /// Clipboard ingress is text-only; files and other binary data use share.
    UnexpectedPayload,
    /// The packet kind was sent directly to this plugin without registry
    /// dispatch, or was otherwise not one of the two supported clipboard kinds.
    UnexpectedKind,
    /// The packet was identified as an echo of text this plugin queued locally.
    Echo {
        /// Envelope id of the locally-originated packet.
        packet_id: i64,
        /// Peer that returned the locally-originated packet.
        peer_id: String,
    },
    /// The packet body was not the KDE Connect clipboard body shape.
    MalformedBody,
    /// The same packet id from the same peer was already admitted.
    Duplicate {
        /// Envelope id that was already admitted.
        packet_id: i64,
        /// Peer-scoped source identity used for deduplication.
        peer_id: String,
    },
}

impl fmt::Display for ClipboardRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unauthorized { peer_id } => {
                write!(f, "clipboard ingress from {peer_id:?} is unauthorized")
            }
            Self::Blank => f.write_str("clipboard text is blank"),
            Self::TooLarge { bytes, max_bytes } => {
                write!(f, "clipboard text is {bytes} bytes; maximum is {max_bytes}")
            }
            Self::BodyTooLarge { bytes, max_bytes } => write!(
                f,
                "clipboard packet body is {bytes} bytes; maximum is {max_bytes}"
            ),
            Self::UnexpectedPayload => {
                f.write_str("clipboard packets cannot carry a secondary payload")
            }
            Self::UnexpectedKind => f.write_str("packet kind is not a clipboard kind"),
            Self::Echo { packet_id, peer_id } => write!(
                f,
                "clipboard packet {packet_id} from {peer_id:?} is an echo"
            ),
            Self::MalformedBody => f.write_str("clipboard packet body is malformed"),
            Self::Duplicate { packet_id, peer_id } => write!(
                f,
                "clipboard packet {packet_id} from {peer_id:?} is a duplicate"
            ),
        }
    }
}

impl std::error::Error for ClipboardRejection {}

/// Validate clipboard text before either direction queues it.
///
/// The value is a Rust `str`, so it is already valid UTF-8. This shared path
/// intentionally validates without trimming or rewriting accepted text: the
/// exact content is preserved on the KDE Connect wire.
///
/// # Errors
///
/// Returns `ClipboardRejection::Blank` for empty/whitespace-only text
/// or `ClipboardRejection::TooLarge` when the UTF-8 byte bound is exceeded.
pub fn validate_clipboard_text(content: &str) -> Result<(), ClipboardRejection> {
    if content.trim().is_empty() {
        return Err(ClipboardRejection::Blank);
    }
    if content.len() > MAX_CLIPBOARD_TEXT_BYTES {
        return Err(ClipboardRejection::TooLarge {
            bytes: content.len(),
            max_bytes: MAX_CLIPBOARD_TEXT_BYTES,
        });
    }
    Ok(())
}

/// Validate a local seat identifier before materialization.
///
/// Seat names are opaque to the protocol, but rejecting control characters,
/// separators, and empty values prevents path or command interpretation.
///
/// # Errors
///
/// Returns the reason the seat is empty, too long, or contains invalid bytes.
pub fn validate_clipboard_seat(seat: &str) -> Result<(), ClipboardSeatError> {
    if seat.is_empty() {
        return Err(ClipboardSeatError::Empty);
    }
    if seat.len() > MAX_CLIPBOARD_SEAT_BYTES {
        return Err(ClipboardSeatError::TooLong {
            bytes: seat.len(),
            max_bytes: MAX_CLIPBOARD_SEAT_BYTES,
        });
    }
    if !seat.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':' | b'@')
    }) {
        return Err(ClipboardSeatError::InvalidCharacters);
    }
    Ok(())
}

/// Configuration error for an active-seat destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardSeatError {
    /// No seat was supplied.
    Empty,
    /// The seat exceeds [`MAX_CLIPBOARD_SEAT_BYTES`].
    TooLong {
        /// Actual UTF-8 byte length.
        bytes: usize,
        /// Bound that was exceeded.
        max_bytes: usize,
    },
    /// The seat contains a separator, control character, or other unsafe byte.
    InvalidCharacters,
}

impl fmt::Display for ClipboardSeatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("clipboard seat is empty"),
            Self::TooLong { bytes, max_bytes } => {
                write!(f, "clipboard seat is {bytes} bytes; maximum is {max_bytes}")
            }
            Self::InvalidCharacters => f.write_str("clipboard seat contains invalid characters"),
        }
    }
}

impl std::error::Error for ClipboardSeatError {}

/// Source and dedup/echo facts retained for an admitted clipboard item.
///
/// `packet_id` is the KDE Connect envelope id. For inbound packets, the
/// peer/context fields are copied from the dispatch inputs. Outbound items
/// have no peer or pairing context at this protocol layer. `is_echo` is
/// deliberately optional: the stock clipboard body has no echo marker, so
/// the plugin must not claim that an inbound packet is or is not an echo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardMetadata {
    /// KDE Connect envelope id, when a packet existed.
    pub packet_id: Option<i64>,
    /// Dispatch peer identity, when supplied by `PluginContext`.
    pub peer_id: Option<String>,
    /// Dispatch pairing state, when supplied by `PluginContext`.
    pub paired: Option<bool>,
    /// Explicit echo marker; None means the wire/context did not provide one.
    pub is_echo: Option<bool>,
}

impl ClipboardMetadata {
    fn inbound(packet: &Packet, ctx: &crate::plugins::PluginContext) -> Self {
        Self {
            packet_id: Some(packet.id),
            peer_id: Some(ctx.peer_id.clone()),
            paired: Some(ctx.paired),
            is_echo: None,
        }
    }

    const fn outbound(packet_id: i64) -> Self {
        Self {
            packet_id: Some(packet_id),
            peer_id: None,
            paired: None,
            is_echo: Some(false),
        }
    }

    const fn local_rejection() -> Self {
        Self {
            packet_id: None,
            peer_id: None,
            paired: None,
            is_echo: Some(false),
        }
    }
}

/// One inbound clipboard body together with its honest packet/context facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedClipboard {
    /// Validated clipboard body.
    pub body: ClipboardBody,
    /// Packet and dispatch facts associated with the body.
    pub metadata: ClipboardMetadata,
}

/// An accepted inbound clipboard item targeted at the active local seat.
///
/// This is deliberately a separate queue from [`ReceivedClipboard`]: disabling
/// local publishing or losing the active seat must not hide or discard remote
/// clipboard history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardMaterialization {
    /// Validated clipboard body.
    pub body: ClipboardBody,
    /// Honest source and packet attribution.
    pub metadata: ClipboardMetadata,
    /// Exact local seat selected when the packet was admitted.
    pub target_seat: String,
}

/// One outbound KDE Connect packet together with its available metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundClipboard {
    /// Wire-compatible KDE Connect packet.
    pub packet: Packet,
    /// Facts available when the packet was queued.
    pub metadata: ClipboardMetadata,
}

/// A rejection retained for diagnostics without changing the `Plugin` trait's
/// existing response-only process signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardRejectionRecord {
    /// Typed admission failure.
    pub reason: ClipboardRejection,
    /// Packet/context facts available for the rejected input.
    pub metadata: ClipboardMetadata,
}

const MAX_RECORDED_REJECTIONS: usize = 64;
const MAX_SEEN_INBOUND: usize = 256;

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
///
/// # Errors
///
/// Returns the underlying `serde_json` deserialization error when the packet
/// body does not match B.
#[allow(clippy::too_long_first_doc_paragraph)]
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
///
/// This low-level constructor preserves the historical wire-building API;
/// queued plugin traffic uses `validate_clipboard_text` first.
#[allow(clippy::needless_pass_by_value)]
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
        assert_eq!(
            p.handles(),
            &["kdeconnect.clipboard", "kdeconnect.clipboard.connect"]
        );
    }

    #[test]
    fn clipboard_plugin_queues_connect_variant() {
        // The `.connect` push (current contents at link-up) routes
        // through the same body path and queues like a live copy.
        let mut plugin = ClipboardPlugin::new();
        let ctx = PluginContext::new("phone", true);
        let pkt = Packet {
            id: 1,
            kind: "kdeconnect.clipboard.connect".to_string(),
            body: serde_json::json!({ "content": "from connect" }),
            mde_caps: None,
            payload_size: None,
            payload_transfer_info: None,
        };
        plugin.process(&pkt, &ctx);
        let drained = plugin.take_received();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].content, "from connect");
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

    #[test]
    fn validation_is_utf8_byte_bounded_and_preserves_exact_text() {
        let at_limit = "🦀".repeat(MAX_CLIPBOARD_TEXT_BYTES / "🦀".len());
        assert_eq!(at_limit.len(), MAX_CLIPBOARD_TEXT_BYTES);
        assert!(validate_clipboard_text(&at_limit).is_ok());

        let over_limit = format!("{at_limit}x");
        assert_eq!(
            validate_clipboard_text(&over_limit),
            Err(ClipboardRejection::TooLarge {
                bytes: MAX_CLIPBOARD_TEXT_BYTES + 1,
                max_bytes: MAX_CLIPBOARD_TEXT_BYTES,
            })
        );
        assert_eq!(
            validate_clipboard_text(" \n\t"),
            Err(ClipboardRejection::Blank)
        );
        let p = clipboard_packet(9, " leading and trailing ".to_string());
        let body: ClipboardBody = from_packet_body(&p).unwrap();
        assert_eq!(body.content, " leading and trailing ");
    }

    #[test]
    fn inbound_validation_rejects_before_queueing_and_keeps_reasons_typed() {
        let mut plugin = ClipboardPlugin::new();
        let ctx = PluginContext::new("phone", true);
        plugin.process(&clipboard_packet(1, "  ".to_string()), &ctx);
        plugin.process(
            &clipboard_packet(2, "x".repeat(MAX_CLIPBOARD_TEXT_BYTES + 1)),
            &ctx,
        );

        assert_eq!(plugin.pending_count(), 0);
        assert_eq!(
            plugin
                .take_rejections()
                .into_iter()
                .map(|record| record.reason)
                .collect::<Vec<_>>(),
            vec![
                ClipboardRejection::Blank,
                ClipboardRejection::TooLarge {
                    bytes: MAX_CLIPBOARD_TEXT_BYTES + 1,
                    max_bytes: MAX_CLIPBOARD_TEXT_BYTES,
                }
            ]
        );
    }

    #[test]
    fn inbound_metadata_preserves_packet_context_without_inventing_echo() {
        let mut plugin = ClipboardPlugin::new();
        let ctx = PluginContext::new("paired-phone", true);
        plugin.process(&clipboard_packet(77, "from phone".to_string()), &ctx);

        let records = plugin.take_received_with_metadata();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].body.content, "from phone");
        assert_eq!(records[0].metadata.packet_id, Some(77));
        assert_eq!(records[0].metadata.peer_id.as_deref(), Some("paired-phone"));
        assert_eq!(records[0].metadata.paired, Some(true));
        assert_eq!(records[0].metadata.is_echo, None);
    }

    #[test]
    fn unauthorized_inbound_is_rejected_before_validation_and_queueing() {
        let mut plugin = ClipboardPlugin::new();
        let ctx = PluginContext::new("unpaired-phone", false);
        let packet = clipboard_packet(78, "valid text".to_string());
        plugin.process(&packet, &ctx);

        assert_eq!(plugin.pending_count(), 0);
        let rejection = plugin.last_rejection().expect("unauthorized rejection");
        assert_eq!(
            rejection.reason,
            ClipboardRejection::Unauthorized {
                peer_id: "unpaired-phone".to_string(),
            }
        );
        assert_eq!(rejection.metadata.packet_id, Some(78));
        assert_eq!(
            rejection.metadata.peer_id.as_deref(),
            Some("unpaired-phone")
        );
        assert_eq!(rejection.metadata.paired, Some(false));
    }

    #[test]
    fn inbound_duplicate_is_rejected_by_peer_and_packet_id() {
        let mut plugin = ClipboardPlugin::new();
        let ctx = PluginContext::new("phone", true);
        let packet = clipboard_packet(88, "same event".to_string());
        plugin.process(&packet, &ctx);
        plugin.process(&packet, &ctx);

        assert_eq!(plugin.pending_count(), 1);
        let rejection = plugin.last_rejection().expect("duplicate rejection");
        assert_eq!(
            rejection.reason,
            ClipboardRejection::Duplicate {
                packet_id: 88,
                peer_id: "phone".to_string(),
            }
        );
        assert_eq!(rejection.metadata.packet_id, Some(88));
        assert_eq!(rejection.metadata.peer_id.as_deref(), Some("phone"));
    }

    #[test]
    fn malformed_inbound_records_a_typed_rejection() {
        let mut plugin = ClipboardPlugin::new();
        let ctx = PluginContext::new("phone", true);
        let packet = Packet {
            id: 99,
            kind: "kdeconnect.clipboard".to_string(),
            body: serde_json::json!({"content": 42}),
            mde_caps: None,
            payload_size: None,
            payload_transfer_info: None,
        };
        plugin.process(&packet, &ctx);

        assert_eq!(plugin.pending_count(), 0);
        let rejection = plugin.last_rejection().expect("malformed rejection");
        assert_eq!(rejection.reason, ClipboardRejection::MalformedBody);
        assert_eq!(rejection.metadata.packet_id, Some(99));
    }

    #[test]
    fn outbound_validation_rejects_before_queueing_and_typed_path_is_available() {
        let mut plugin = ClipboardPlugin::new();
        assert_eq!(
            plugin.try_push_clipboard("\n\t".to_string()),
            Err(ClipboardRejection::Blank)
        );
        assert_eq!(plugin.outbound_count(), 0);

        // The legacy void API remains safe: invalid text is simply not
        // admitted, while the typed rejection is retained for diagnostics.
        plugin.push_clipboard("x".repeat(MAX_CLIPBOARD_TEXT_BYTES + 1));
        assert_eq!(plugin.outbound_count(), 0);
        assert_eq!(plugin.take_rejections().len(), 2);
    }

    #[test]
    fn outbound_metadata_has_packet_id_but_no_fake_peer_context() {
        let mut plugin = ClipboardPlugin::new();
        plugin.try_push_clipboard("from mesh".to_string()).unwrap();
        let records = plugin.take_outbound_with_metadata();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].metadata.packet_id, Some(records[0].packet.id));
        assert_eq!(records[0].metadata.peer_id, None);
        assert_eq!(records[0].metadata.paired, None);
        assert_eq!(records[0].metadata.is_echo, Some(false));
        assert_eq!(records[0].packet.kind, "kdeconnect.clipboard");
    }

    #[test]
    fn active_seat_materialization_is_separate_and_attributed() {
        let mut plugin = ClipboardPlugin::new();
        plugin
            .set_active_seat(Some("seat:dell".to_string()))
            .unwrap();
        let ctx = PluginContext::new("paired-phone", true);
        plugin.process(&clipboard_packet(101, "remote copy".into()), &ctx);

        let history = plugin.take_received_with_metadata();
        assert_eq!(history.len(), 1);
        assert_eq!(plugin.pending_count(), 0);
        let materialized = plugin.take_materializations();
        assert_eq!(materialized.len(), 1);
        assert_eq!(materialized[0].target_seat, "seat:dell");
        assert_eq!(materialized[0].body.content, "remote copy");
        assert_eq!(
            materialized[0].metadata.peer_id.as_deref(),
            Some("paired-phone")
        );
        assert_eq!(materialized[0].metadata.packet_id, Some(101));
    }

    #[test]
    fn no_active_seat_keeps_history_without_claiming_materialization() {
        let mut plugin = ClipboardPlugin::new();
        plugin.process(
            &clipboard_packet(102, "remote history".into()),
            &PluginContext::new("paired-phone", true),
        );
        assert_eq!(plugin.pending_count(), 1);
        assert!(plugin.take_materializations().is_empty());
    }

    #[test]
    fn active_seat_rejects_path_like_and_oversized_values() {
        let mut plugin = ClipboardPlugin::new();
        assert_eq!(
            plugin.set_active_seat(Some("../seat".into())),
            Err(ClipboardSeatError::InvalidCharacters)
        );
        assert_eq!(
            plugin.set_active_seat(Some("x".repeat(MAX_CLIPBOARD_SEAT_BYTES + 1))),
            Err(ClipboardSeatError::TooLong {
                bytes: MAX_CLIPBOARD_SEAT_BYTES + 1,
                max_bytes: MAX_CLIPBOARD_SEAT_BYTES,
            })
        );
        assert_eq!(plugin.active_seat(), None);
    }

    #[test]
    fn locally_queued_packet_returned_by_peer_is_rejected_as_echo() {
        let mut plugin = ClipboardPlugin::new();
        plugin.try_push_clipboard("local copy".into()).unwrap();
        let packet = plugin.take_outbound().pop().unwrap();
        plugin.process(&packet, &PluginContext::new("paired-phone", true));
        assert_eq!(plugin.pending_count(), 0);
        assert!(matches!(
            plugin.last_rejection().map(|record| &record.reason),
            Some(ClipboardRejection::Echo { packet_id: _, peer_id }) if peer_id == "paired-phone"
        ));
    }

    #[test]
    fn secondary_payload_and_wrong_kind_are_rejected_before_body_admission() {
        let mut plugin = ClipboardPlugin::new();
        let mut packet = clipboard_packet(103, "text".into());
        packet.payload_size = Some(1);
        plugin.process(&packet, &PluginContext::new("paired-phone", true));
        assert_eq!(plugin.pending_count(), 0);
        assert_eq!(
            plugin.last_rejection().map(|record| &record.reason),
            Some(&ClipboardRejection::UnexpectedPayload)
        );

        let mut packet = clipboard_packet(104, "text".into());
        packet.kind = "kdeconnect.ping".into();
        plugin.process(&packet, &PluginContext::new("paired-phone", true));
        assert_eq!(
            plugin.last_rejection().map(|record| &record.reason),
            Some(&ClipboardRejection::UnexpectedKind)
        );
    }

    #[test]
    fn unpaired_oversized_body_is_rejected_before_deserialization() {
        let mut plugin = ClipboardPlugin::new();
        let packet = Packet {
            id: 105,
            kind: "kdeconnect.clipboard".into(),
            body: serde_json::json!({ "content": "x" }),
            mde_caps: None,
            payload_size: None,
            payload_transfer_info: None,
        };
        plugin.process(&packet, &PluginContext::new("unpaired-phone", false));
        assert_eq!(plugin.pending_count(), 0);
        assert!(matches!(
            plugin.last_rejection().map(|record| &record.reason),
            Some(ClipboardRejection::Unauthorized { peer_id }) if peer_id == "unpaired-phone"
        ));
    }

    #[test]
    fn oversized_serialized_body_is_rejected_even_when_content_field_is_small() {
        let mut plugin = ClipboardPlugin::new();
        let packet = Packet {
            id: 106,
            kind: "kdeconnect.clipboard".into(),
            body: serde_json::json!({
                "content": "small",
                "unexpected": "x".repeat(MAX_CLIPBOARD_BODY_BYTES + 1)
            }),
            mde_caps: None,
            payload_size: None,
            payload_transfer_info: None,
        };
        plugin.process(&packet, &PluginContext::new("paired-phone", true));
        assert_eq!(plugin.pending_count(), 0);
        assert!(matches!(
            plugin.last_rejection().map(|record| &record.reason),
            Some(ClipboardRejection::BodyTooLarge { bytes, max_bytes })
                if *bytes > MAX_CLIPBOARD_BODY_BYTES && *max_bytes == MAX_CLIPBOARD_BODY_BYTES
        ));
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
#[derive(Debug)]
pub struct ClipboardPlugin {
    received: Vec<ReceivedClipboard>,
    materializations: Vec<ClipboardMaterialization>,
    active_seat: Option<String>,
    /// Proactive outbound packets queued for the paired phone
    /// (mesh clipboard → phone). Drained by the host on each tick.
    outbound: Vec<OutboundClipboard>,
    handles: [&'static str; 2],
    seen_inbound: VecDeque<(String, i64)>,
    seen_outbound: VecDeque<i64>,
    rejections: VecDeque<ClipboardRejectionRecord>,
    last_outbound_id: i64,
}

impl Default for ClipboardPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipboardPlugin {
    /// New empty plugin.
    ///
    /// Handles both `kdeconnect.clipboard` (a live copy on the peer)
    /// AND `kdeconnect.clipboard.connect` (the current-contents push
    /// a peer sends at connection time). Both carry the identical
    /// [`ClipboardBody`] shape, so the same `process` path queues
    /// them — closing the advertised-but-unrouted `.connect`
    /// incoming capability.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            received: Vec::new(),
            materializations: Vec::new(),
            active_seat: None,
            outbound: Vec::new(),
            handles: ["kdeconnect.clipboard", "kdeconnect.clipboard.connect"],
            seen_inbound: VecDeque::new(),
            seen_outbound: VecDeque::new(),
            rejections: VecDeque::new(),
            last_outbound_id: 0,
        }
    }

    /// Drain every received clipboard body (phone → mesh).
    #[must_use]
    pub fn take_received(&mut self) -> Vec<ClipboardBody> {
        self.take_received_with_metadata()
            .into_iter()
            .map(|item| item.body)
            .collect()
    }

    /// Drain received bodies while retaining packet id and dispatch context.
    #[must_use]
    pub fn take_received_with_metadata(&mut self) -> Vec<ReceivedClipboard> {
        std::mem::take(&mut self.received)
    }

    /// Drain accepted clipboard items that were targeted at the active seat.
    ///
    /// Remote history remains available through [`Self::take_received`]. An
    /// absent active seat therefore produces no materialization rather than a
    /// fake success or an accidental write to an arbitrary seat.
    #[must_use]
    pub fn take_materializations(&mut self) -> Vec<ClipboardMaterialization> {
        std::mem::take(&mut self.materializations)
    }

    /// Select the local seat for future inbound materialization.
    ///
    /// Passing `None` explicitly disables seat delivery while retaining remote
    /// history. The destination is validated before it is stored.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested seat is not a valid local seat
    /// identifier.
    pub fn set_active_seat(&mut self, seat: Option<String>) -> Result<(), ClipboardSeatError> {
        if let Some(ref seat) = seat {
            validate_clipboard_seat(seat)?;
        }
        self.active_seat = seat;
        Ok(())
    }

    /// The currently selected local seat, if materialization is available.
    #[must_use]
    pub fn active_seat(&self) -> Option<&str> {
        self.active_seat.as_deref()
    }

    /// Items currently queued (phone → mesh).
    #[must_use]
    pub const fn pending_count(&self) -> usize {
        self.received.len()
    }

    /// Queue `content` as a `kdeconnect.clipboard` packet to send to
    /// the paired phone (mesh → phone). Caller passes new mesh
    /// clipboard text detected by `mde_kdc::clip_bridge`. The host
    /// drains via `take_outbound()` and writes to the phone's TLS
    /// socket.
    pub fn push_clipboard(&mut self, content: String) {
        let _ = self.try_push_clipboard(content);
    }

    /// Validate and queue mesh text for the paired phone.
    ///
    /// This typed companion preserves the historical infallible
    /// [`Self::push_clipboard`] API for existing callers while making a
    /// rejection observable to new callers.
    ///
    /// # Errors
    ///
    /// Returns the typed validation rejection when content is blank or
    /// exceeds the UTF-8 byte bound.
    pub fn try_push_clipboard(&mut self, content: String) -> Result<(), ClipboardRejection> {
        validate_clipboard_text(&content).inspect_err(|reason| {
            self.record_rejection(reason.clone(), ClipboardMetadata::local_rejection());
        })?;

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or(0);
        // KDE Connect uses the envelope id for deduplication. A wall-clock
        // millisecond alone can collide when two local copies happen in one
        // tick, so make the local sequence strictly monotonic.
        let id_ms = now_ms.max(self.last_outbound_id.saturating_add(1));
        self.last_outbound_id = id_ms;
        if self.seen_outbound.len() == MAX_SEEN_INBOUND {
            let _ = self.seen_outbound.pop_front();
        }
        self.seen_outbound.push_back(id_ms);
        self.outbound.push(OutboundClipboard {
            packet: clipboard_packet(id_ms, content),
            metadata: ClipboardMetadata::outbound(id_ms),
        });
        Ok(())
    }

    /// Drain every outbound clipboard packet (mesh → phone).
    #[must_use]
    pub fn take_outbound(&mut self) -> Vec<crate::wire::Packet> {
        self.take_outbound_with_metadata()
            .into_iter()
            .map(|item| item.packet)
            .collect()
    }

    /// Drain outbound packets while retaining the packet id metadata.
    #[must_use]
    pub fn take_outbound_with_metadata(&mut self) -> Vec<OutboundClipboard> {
        std::mem::take(&mut self.outbound)
    }

    /// Outbound items currently queued.
    #[must_use]
    pub const fn outbound_count(&self) -> usize {
        self.outbound.len()
    }

    /// Drain typed rejection records from malformed or refused clipboard
    /// inputs. The oldest records are discarded after a bounded diagnostic
    /// window so an abusive peer cannot grow this queue without limit.
    #[must_use]
    pub fn take_rejections(&mut self) -> Vec<ClipboardRejectionRecord> {
        self.rejections.drain(..).collect()
    }

    /// Return the newest rejection without draining it.
    #[must_use]
    pub fn last_rejection(&self) -> Option<&ClipboardRejectionRecord> {
        self.rejections.back()
    }

    fn record_rejection(&mut self, reason: ClipboardRejection, metadata: ClipboardMetadata) {
        if self.rejections.len() == MAX_RECORDED_REJECTIONS {
            let _ = self.rejections.pop_front();
        }
        self.rejections
            .push_back(ClipboardRejectionRecord { reason, metadata });
    }

    fn admit_inbound(
        &mut self,
        packet: &Packet,
        body: ClipboardBody,
        ctx: &crate::plugins::PluginContext,
    ) -> Result<(), ClipboardRejection> {
        let metadata = ClipboardMetadata::inbound(packet, ctx);
        validate_clipboard_text(&body.content).inspect_err(|reason| {
            self.record_rejection(reason.clone(), metadata.clone());
        })?;

        let dedup_key = (ctx.peer_id.clone(), packet.id);
        if self.seen_inbound.iter().any(|key| key == &dedup_key) {
            let reason = ClipboardRejection::Duplicate {
                packet_id: packet.id,
                peer_id: ctx.peer_id.clone(),
            };
            self.record_rejection(reason.clone(), metadata);
            return Err(reason);
        }
        if self.seen_inbound.len() == MAX_SEEN_INBOUND {
            let _ = self.seen_inbound.pop_front();
        }
        self.seen_inbound.push_back(dedup_key);
        self.received.push(ReceivedClipboard {
            body: body.clone(),
            metadata: metadata.clone(),
        });
        if let Some(target_seat) = self.active_seat.clone() {
            self.materializations.push(ClipboardMaterialization {
                body,
                metadata,
                target_seat,
            });
        }
        Ok(())
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
        ctx: &crate::plugins::PluginContext,
    ) -> Vec<crate::wire::Packet> {
        let metadata = ClipboardMetadata::inbound(packet, ctx);
        // Apply the trust gate before body deserialization. This keeps an
        // unpaired peer from purchasing CPU/memory with an otherwise valid or
        // oversized clipboard body.
        if !ctx.paired || ctx.peer_id.is_empty() || ctx.peer_id.len() > MAX_CLIPBOARD_SEAT_BYTES {
            self.record_rejection(
                ClipboardRejection::Unauthorized {
                    peer_id: ctx.peer_id.clone(),
                },
                metadata,
            );
            return Vec::new();
        }
        if !self.handles.contains(&packet.kind.as_str()) {
            self.record_rejection(ClipboardRejection::UnexpectedKind, metadata);
            return Vec::new();
        }
        if packet.payload_size.is_some() || packet.payload_transfer_info.is_some() {
            self.record_rejection(ClipboardRejection::UnexpectedPayload, metadata);
            return Vec::new();
        }
        if self.seen_outbound.iter().any(|id| *id == packet.id) {
            self.record_rejection(
                ClipboardRejection::Echo {
                    packet_id: packet.id,
                    peer_id: ctx.peer_id.clone(),
                },
                metadata,
            );
            return Vec::new();
        }
        let Ok(body_bytes) = serde_json::to_vec(&packet.body) else {
            self.record_rejection(ClipboardRejection::MalformedBody, metadata);
            return Vec::new();
        };
        if body_bytes.len() > MAX_CLIPBOARD_BODY_BYTES {
            self.record_rejection(
                ClipboardRejection::BodyTooLarge {
                    bytes: body_bytes.len(),
                    max_bytes: MAX_CLIPBOARD_BODY_BYTES,
                },
                metadata,
            );
            return Vec::new();
        }
        match from_packet_body::<ClipboardBody>(packet) {
            Ok(body) => {
                let _ = self.admit_inbound(packet, body, ctx);
            }
            Err(_) => {
                self.record_rejection(
                    ClipboardRejection::MalformedBody,
                    ClipboardMetadata::inbound(packet, ctx),
                );
            }
        }
        // Proactive outbound packets are drained by the host via
        // take_outbound(), not as responses to inbound packets.
        Vec::new()
    }
}
