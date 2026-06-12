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
    /// ChaCha20-Poly1305 (some Nebula relay paths).
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

    /// CV-1 — comparable strength rank for the router's
    /// content-class encryption floor. `None` (0) < AES-128 (1)
    /// < {AES-256, ChaCha20-Poly1305} (2 — the §3 floor pair,
    /// deliberately equal).
    #[must_use]
    pub const fn strength_rank(self) -> u8 {
        match self {
            EncryptionKind::None => 0,
            EncryptionKind::Aes128Gcm => 1,
            EncryptionKind::Aes256Gcm | EncryptionKind::ChaCha20Poly1305 => 2,
        }
    }

    /// CV-1 — the transport-level encryption each
    /// [`crate::TransportKind`] guarantees in this stack. Every
    /// production transport rides the Nebula overlay (AES-256-GCM
    /// per the §3 lock) or TLS 1.3 (AES-256-GCM); the scorer reads
    /// this to enforce the content-class floor so a future weaker
    /// transport can never silently carry clipboard/file/SMS
    /// payloads.
    #[must_use]
    pub const fn for_transport(kind: crate::TransportKind) -> Self {
        match kind {
            // Nebula tunnel cipher (§3): AES-256-GCM (relay paths may
            // negotiate ChaCha20-Poly1305 — equal rank either way).
            crate::TransportKind::NebulaDirect => EncryptionKind::Aes256Gcm,
            crate::TransportKind::NebulaLighthouseRelay => EncryptionKind::ChaCha20Poly1305,
            // TLS 1.3 inside the overlay.
            crate::TransportKind::NebulaHttps443 | crate::TransportKind::KdcTls => {
                EncryptionKind::Aes256Gcm
            }
        }
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
    /// Default direct-UDP transport capabilities (Nebula tunnel:
    /// ~1280-byte MTU, AES-256-GCM per the §3 lock, no broadcast,
    /// no streaming framing). CV-1 fix: previously claimed the
    /// WireGuard-era AES-128 default — this mesh's substrate is
    /// Nebula, whose cipher is AES-256-GCM.
    #[must_use]
    pub fn direct_udp_default() -> Self {
        Self {
            supports_bulk: false,
            supports_streaming: false,
            supports_broadcast: false,
            mtu: Some(1280),
            encryption_kind: EncryptionKind::Aes256Gcm,
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

    /// Default lighthouse-relay capabilities (Nebula lighthouse-relay: small
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
    fn direct_udp_default_capabilities_match_nebula_tunnel() {
        let c = TransportCapabilities::direct_udp_default();
        assert!(!c.supports_bulk);
        assert!(!c.supports_streaming);
        assert!(!c.supports_broadcast);
        assert_eq!(c.mtu, Some(1280));
        // CV-1 — the Nebula substrate cipher (§3), not the old
        // WireGuard-era AES-128 claim.
        assert_eq!(c.encryption_kind, EncryptionKind::Aes256Gcm);
    }

    #[test]
    fn strength_rank_orders_none_128_then_floor_pair() {
        assert!(EncryptionKind::None.strength_rank() < EncryptionKind::Aes128Gcm.strength_rank());
        assert!(
            EncryptionKind::Aes128Gcm.strength_rank()
                < EncryptionKind::Aes256Gcm.strength_rank()
        );
        assert_eq!(
            EncryptionKind::Aes256Gcm.strength_rank(),
            EncryptionKind::ChaCha20Poly1305.strength_rank(),
            "the §3 floor pair ranks equal"
        );
    }

    #[test]
    fn every_production_transport_meets_the_content_floor() {
        // CV-1 — all four registered TransportKinds ride Nebula or
        // TLS 1.3; none may fall below the AES-256-class floor.
        for kind in crate::TransportKind::all() {
            assert!(
                EncryptionKind::for_transport(kind).strength_rank()
                    >= EncryptionKind::Aes256Gcm.strength_rank(),
                "{kind:?} below the content-class encryption floor"
            );
        }
    }
}
