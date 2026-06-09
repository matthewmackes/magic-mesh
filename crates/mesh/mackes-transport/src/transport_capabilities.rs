//! KDC2-1.5 — `TransportCapabilities` per-transport feature
//! advertisement.
//!
//! Where `crate::Capabilities` (the previous type) is about
//! message-class routing + health windows, `TransportCapabilities`
//! is about *what kinds of payload the transport can carry at
//! all*. The router uses it to filter candidates per message
//! class — e.g. `FileBulk` skips any transport with
//! `supports_bulk == false`.
//!
//! Both types coexist intentionally: the router asks
//! `TransportCapabilities` for "can this transport carry the
//! class at all?", then asks `Capabilities` for "what's its
//! current health window / label?". Merging them was rejected
//! because they evolve independently — a transport's payload
//! support is locked at impl time, while its health-window
//! policy is operator-tunable per session.

use serde::{Deserialize, Serialize};

/// Symmetric-encryption algorithm a transport guarantees.
/// Used by the router to gate sensitive message classes (e.g.,
/// SMS bodies) on a minimum-strength transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EncryptionKind {
    /// No transport-level encryption. Loopback / dev only.
    None,
    /// AES-128-GCM (e.g. WireGuard tunnel default).
    Aes128Gcm,
    /// AES-256-GCM (KDC2 + TLS 1.3 default).
    Aes256Gcm,
    /// ChaCha20-Poly1305 (some Tailscale / DERP paths).
    // Serde's snake_case would render this as
    // `cha_cha20_poly1305` (it splits between every capital
    // and digit cluster). Override to the more readable
    // `chacha20_poly1305` audit token.
    #[serde(rename = "chacha20_poly1305")]
    ChaCha20Poly1305,
}

impl EncryptionKind {
    /// Stable audit-token. Same as the serde rendering, but
    /// available without paying for a JSON encode.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            EncryptionKind::None => "none",
            EncryptionKind::Aes128Gcm => "aes128_gcm",
            EncryptionKind::Aes256Gcm => "aes256_gcm",
            EncryptionKind::ChaCha20Poly1305 => "chacha20_poly1305",
        }
    }

    /// True when this kind provides authenticated encryption
    /// (AEAD). All current variants except `None` are AEADs.
    #[must_use]
    pub const fn is_authenticated(self) -> bool {
        !matches!(self, EncryptionKind::None)
    }
}

/// Per-transport capability advertisement consumed by the
/// router's filtering pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportCapabilities {
    /// True when the transport can carry bulk payloads (the
    /// router's `FileBulk` message class requires this).
    pub supports_bulk: bool,
    /// True when the transport supports framed streaming (chunks
    /// arriving in order). Required for video / live SMS thread
    /// updates.
    pub supports_streaming: bool,
    /// True when the transport supports broadcast / multicast
    /// (one sender → many receivers without per-pair sends).
    /// Used by the future mesh announce path.
    pub supports_broadcast: bool,
    /// Maximum transmission unit in bytes. `None` means
    /// "unbounded / streams transparently."
    pub mtu: Option<u32>,
    /// Encryption guarantee this transport provides.
    pub encryption_kind: EncryptionKind,
}

impl TransportCapabilities {
    /// Default direct-UDP transport capabilities (WireGuard
    /// tunnel: ~1280-byte MTU, AES-128-GCM, no broadcast, no
    /// streaming framing).
    #[must_use]
    pub fn direct_udp_default() -> Self {
        Self {
            supports_bulk: false,
            supports_streaming: false,
            supports_broadcast: false,
            mtu: Some(1280),
            encryption_kind: EncryptionKind::Aes128Gcm,
        }
    }

    /// Default KDC-over-TLS capabilities (TLS 1.3 + KDC framing:
    /// streaming yes, bulk yes via `share.request`, unbounded
    /// MTU within the TLS session, AES-256-GCM).
    #[must_use]
    pub fn kdc_tls_default() -> Self {
        Self {
            supports_bulk: true,
            supports_streaming: true,
            supports_broadcast: false,
            mtu: None,
            encryption_kind: EncryptionKind::Aes256Gcm,
        }
    }

    /// Default DERP-relay capabilities (Tailscale DERP: small
    /// frames, no bulk, ChaCha20-Poly1305).
    #[must_use]
    pub fn derp_relay_default() -> Self {
        Self {
            supports_bulk: false,
            supports_streaming: false,
            supports_broadcast: false,
            mtu: Some(1200),
            encryption_kind: EncryptionKind::ChaCha20Poly1305,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encryption_kind_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&EncryptionKind::Aes256Gcm).unwrap(),
            r#""aes256_gcm""#,
        );
        assert_eq!(
            serde_json::to_string(&EncryptionKind::ChaCha20Poly1305).unwrap(),
            r#""chacha20_poly1305""#,
        );
    }

    #[test]
    fn encryption_kind_as_str_matches_serde() {
        for k in [
            EncryptionKind::None,
            EncryptionKind::Aes128Gcm,
            EncryptionKind::Aes256Gcm,
            EncryptionKind::ChaCha20Poly1305,
        ] {
            let serde_token = serde_json::to_string(&k)
                .unwrap()
                .trim_matches('"')
                .to_string();
            assert_eq!(serde_token, k.as_str());
        }
    }

    #[test]
    fn none_is_only_non_authenticated_kind() {
        assert!(!EncryptionKind::None.is_authenticated());
        assert!(EncryptionKind::Aes128Gcm.is_authenticated());
        assert!(EncryptionKind::Aes256Gcm.is_authenticated());
        assert!(EncryptionKind::ChaCha20Poly1305.is_authenticated());
    }

    #[test]
    fn kdc_tls_default_supports_bulk_and_streaming() {
        // FileBulk + streaming SMS rely on this lock — adding
        // a new transport variant must explicitly set these,
        // not inherit the more restrictive direct-UDP defaults.
        let c = TransportCapabilities::kdc_tls_default();
        assert!(c.supports_bulk);
        assert!(c.supports_streaming);
        assert_eq!(c.encryption_kind, EncryptionKind::Aes256Gcm);
        assert_eq!(c.mtu, None);
    }

    #[test]
    fn direct_udp_default_capabilities_match_wireguard_defaults() {
        let c = TransportCapabilities::direct_udp_default();
        assert!(!c.supports_bulk);
        assert!(!c.supports_streaming);
        assert!(!c.supports_broadcast);
        assert_eq!(c.mtu, Some(1280));
        assert_eq!(c.encryption_kind, EncryptionKind::Aes128Gcm);
    }
}
