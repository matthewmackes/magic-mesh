//! KDC2-2.9 share plugin — `kdeconnect.share.request` body.
//!
//! KDE Connect's file-transfer plugin. The "request" naming is
//! upstream's quirk — the packet kind is literally
//! `kdeconnect.share.request` for both URL and file shares.
//!
//! The wire body has two shapes that share field names but differ
//! in which fields are populated:
//!
//!   * URL/text share — `url` carries the payload; file-specific
//!     fields (`filename`, `payloadSize`, `payloadHash`) are
//!     absent.
//!   * File share — `filename` + `payloadSize` set; `payloadHash`
//!     OPTIONAL for integrity check; the actual binary payload
//!     streams through a separate KDE Connect file-transfer port
//!     (KDC2-3 host integration plumbs that).
//!
//! KDC2 ships both as one body type with optional fields, plus
//! discriminator helpers so callers don't have to inspect field
//! presence by hand.

use serde::{Deserialize, Serialize};

use crate::wire::Packet;

/// `kdeconnect.share.request` body. Shape is intentionally permissive
/// to cover both URL and file shares — call [`ShareBody::kind`] to
/// downcast to a typed variant.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareBody {
    /// Filename when sharing a file. Empty for URL shares.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub filename: String,
    /// Payload size in bytes. Zero / absent for URL shares.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub payload_size: u64,
    /// SHA-256 hex of the payload (optional integrity check).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub payload_hash: String,
    /// URL or text payload. Empty for file shares.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// Open immediately on the receiver. Upstream uses this to
    /// drive "open in browser" for URL shares.
    #[serde(default)]
    pub open: bool,
}

/// Discriminator for the two share-packet variants the protocol
/// supports. Returned by [`ShareBody::kind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareKind {
    /// Empty body — usually a malformed packet but legal on the
    /// wire (some upstream clients send a probe before the real
    /// share packet).
    Empty,
    /// URL or text share — `url` is populated.
    Url,
    /// File share — `filename` and `payload_size` are populated.
    File,
}

impl ShareBody {
    /// Determine which variant this body represents. Caller-side
    /// dispatch saves every consumer from reimplementing the
    /// "which fields are set" probe.
    #[must_use]
    pub fn kind(&self) -> ShareKind {
        if !self.url.is_empty() {
            ShareKind::Url
        } else if !self.filename.is_empty() && self.payload_size > 0 {
            ShareKind::File
        } else {
            ShareKind::Empty
        }
    }
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// Build a URL-share packet (e.g. operator paste-shares a URL to
/// their phone).
#[must_use]
pub fn url_share_packet(id_ms: i64, url: String, open: bool) -> Packet {
    Packet {
        id: id_ms,
        kind: "kdeconnect.share.request".to_string(),
        body: serde_json::to_value(ShareBody {
            url,
            open,
            ..Default::default()
        })
        .expect("ShareBody is always JSON-serializable"),
        mde_caps: None,
        payload_size: None,
        payload_transfer_info: None,
    }
}

/// Build a file-share request packet. Caller is responsible for
/// streaming the actual binary payload through the KDC file-
/// transfer port — this packet only announces the intent.
#[must_use]
pub fn file_share_packet(
    id_ms: i64,
    filename: String,
    payload_size: u64,
    payload_hash: String,
) -> Packet {
    Packet {
        id: id_ms,
        kind: "kdeconnect.share.request".to_string(),
        body: serde_json::to_value(ShareBody {
            filename,
            payload_size,
            payload_hash,
            ..Default::default()
        })
        .expect("ShareBody is always JSON-serializable"),
        mde_caps: None,
        payload_size: None,
        payload_transfer_info: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_share_kind_is_url() {
        let p = url_share_packet(1, "https://example.com".to_string(), true);
        let body: ShareBody = serde_json::from_value(p.body).unwrap();
        assert_eq!(body.kind(), ShareKind::Url);
    }

    #[test]
    fn file_share_kind_is_file() {
        let p = file_share_packet(
            1,
            "report.pdf".to_string(),
            1024 * 1024,
            "deadbeef".to_string(),
        );
        let body: ShareBody = serde_json::from_value(p.body).unwrap();
        assert_eq!(body.kind(), ShareKind::File);
    }

    #[test]
    fn empty_share_body_kind_is_empty() {
        let body = ShareBody::default();
        assert_eq!(body.kind(), ShareKind::Empty);
    }

    #[test]
    fn url_share_omits_file_fields_on_wire() {
        // skip_serializing_if locks — a URL share that leaks
        // `"filename":""` or `"payloadSize":0` would confuse some
        // upstream clients (they'd try to receive a zero-byte
        // file).
        let p = url_share_packet(1, "https://example.com".to_string(), false);
        let s = serde_json::to_string(&p).unwrap();
        assert!(!s.contains(r#""filename""#));
        assert!(!s.contains(r#""payloadSize""#));
        assert!(!s.contains(r#""payloadHash""#));
        assert!(s.contains(r#""url":"https://example.com""#));
    }

    #[test]
    fn file_share_omits_url_field_on_wire() {
        let p = file_share_packet(1, "x.txt".to_string(), 10, String::new());
        let s = serde_json::to_string(&p).unwrap();
        assert!(!s.contains(r#""url""#));
        assert!(s.contains(r#""filename":"x.txt""#));
        assert!(s.contains(r#""payloadSize":10"#));
        // payload_hash empty → omitted.
        assert!(!s.contains(r#""payloadHash""#));
    }

    #[test]
    fn share_packet_kind_includes_request_suffix() {
        // upstream's quirky `.request` suffix — KDC2-2.1's
        // PluginKind::Share.packet_kind() locks it. Belt and
        // suspenders.
        let p = url_share_packet(1, "x".to_string(), false);
        assert_eq!(p.kind, "kdeconnect.share.request");
        assert_eq!(p.kind, crate::plugins::PluginKind::Share.packet_kind());
    }

    #[test]
    fn share_body_round_trips_via_wire() {
        let body = ShareBody {
            filename: "doc.pdf".to_string(),
            payload_size: 4096,
            payload_hash: "abc".to_string(),
            url: String::new(),
            open: false,
        };
        let s = serde_json::to_string(&body).unwrap();
        let back: ShareBody = serde_json::from_str(&s).unwrap();
        assert_eq!(back, body);
    }

    // KDC2-2.15 — SharePlugin Plugin trait impl
    use crate::plugins::{Plugin, PluginContext, PluginKind};

    #[test]
    fn share_plugin_queues_inbound_url() {
        let mut plugin = SharePlugin::new();
        let ctx = PluginContext::new("alice", true);
        plugin.process(
            &url_share_packet(1, "https://example.com".into(), false),
            &ctx,
        );
        let drained = plugin.take_received();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].url, "https://example.com");
        assert_eq!(drained[0].kind(), ShareKind::Url);
    }

    #[test]
    fn share_plugin_queues_inbound_file_announce() {
        let mut plugin = SharePlugin::new();
        let ctx = PluginContext::new("alice", true);
        plugin.process(
            &file_share_packet(1, "doc.pdf".into(), 1024, "hash".into()),
            &ctx,
        );
        let drained = plugin.take_received();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].kind(), ShareKind::File);
    }
}

/// KDC2-2.15 — SharePlugin. Queues both URL + file-announce
/// bodies; the actual binary payload streaming over the KDC
/// file-transfer port is a separate KDC2-3.x mechanism the host
/// drives off the file-share announcement.
#[derive(Debug, Default)]
pub struct SharePlugin {
    received: Vec<ShareBody>,
    handles: [&'static str; 1],
}

impl SharePlugin {
    /// New empty plugin.
    #[must_use]
    pub fn new() -> Self {
        Self {
            received: Vec::new(),
            handles: ["kdeconnect.share.request"],
        }
    }
    /// Drain every queued share body.
    #[must_use]
    pub fn take_received(&mut self) -> Vec<ShareBody> {
        std::mem::take(&mut self.received)
    }
    /// Items currently queued.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.received.len()
    }
}

impl crate::plugins::Plugin for SharePlugin {
    fn kind(&self) -> crate::plugins::PluginKind {
        crate::plugins::PluginKind::Share
    }
    fn handles(&self) -> &[&'static str] {
        &self.handles
    }
    fn process(
        &mut self,
        packet: &crate::wire::Packet,
        _ctx: &crate::plugins::PluginContext,
    ) -> Vec<crate::wire::Packet> {
        if let Ok(body) = crate::plugins::from_packet_body::<ShareBody>(packet) {
            self.received.push(body);
        }
        Vec::new()
    }
}
