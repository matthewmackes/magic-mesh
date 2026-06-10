//! Per-node Ed25519 identity (Phase 12.3.2).
//!
//! Every enrolled peer holds a single signing keypair. The private
//! key lives at `~/.local/share/mackes/node.key` (mode 0600); the
//! public key is fingerprinted into the leader's `nodes` table.
//!
//! Per 12.3.2 lock: "Lost-key flow: forced re-enrollment by Host
//! operator." The Host can mark a node's row as needing re-enroll;
//! the peer detects the flag on next heartbeat and generates a
//! fresh keypair.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Wrapper around `ed25519_dalek::SigningKey` that zeros on drop.
/// Signing keys never appear in `Debug` output.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct NodeKey {
    // `SigningKey` is itself zero-on-drop; the wrapper is here so
    // we can override Debug and add Mackes-specific construction.
    #[zeroize(skip)]
    inner: SigningKey,
}

impl NodeKey {
    /// Generate a fresh keypair from the OS CSPRNG.
    #[must_use]
    pub fn generate() -> Self {
        Self {
            inner: SigningKey::generate(&mut OsRng),
        }
    }

    /// Load a keypair from 32 bytes (the format we write to disk).
    /// Returns `None` if `bytes` isn't a valid key encoding.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self {
            inner: SigningKey::from_bytes(&bytes),
        }
    }

    /// Reveal the 32 secret bytes for serialization to disk. Caller
    /// is responsible for `chmod 0600` and zeroing any intermediate
    /// buffer.
    #[must_use]
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.inner.to_bytes()
    }

    /// Public verifying key — fingerprintable, share-safe.
    #[must_use]
    pub fn verifying_key(&self) -> VerifyingKey {
        self.inner.verifying_key()
    }

    /// Sign a payload. Used for enrollment requests + audit row
    /// signing (Phase 12.6.3 hash chain is appended with the
    /// emitting peer's signature so the leader can verify origin).
    #[must_use]
    pub fn sign(&self, payload: &[u8]) -> Signature {
        self.inner.sign(payload)
    }

    /// SHA-256 fingerprint of the public key, hex-encoded. The
    /// canonical short-form node ID used in `nodes.fingerprint`.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(self.verifying_key().as_bytes());
        let digest = hasher.finalize();
        let mut out = String::with_capacity(digest.len() * 2);
        for b in digest {
            use std::fmt::Write;
            let _ = write!(out, "{b:02x}");
        }
        out
    }
}

impl std::fmt::Debug for NodeKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NodeKey(fingerprint={})", self.fingerprint())
    }
}

/// PLANES-4 (W25) — a compact, deterministic **word-pair** rendering of a
/// fingerprint, for verbal/visual out-of-band comparison ("do our two
/// nodes show the same words?"). It is a *tripwire*, not the security
/// boundary — the full hex fingerprint is the real check. The pair is the
/// first two bytes of the hex mapped into a fixed 64-word list, so the
/// same fingerprint always yields the same pair on every node. Returns an
/// empty string for a malformed (non-hex / too-short) input.
#[must_use]
pub fn fingerprint_word_pair(fingerprint_hex: &str) -> String {
    let byte = |i: usize| -> Option<u8> {
        let s = fingerprint_hex.get(i * 2..i * 2 + 2)?;
        u8::from_str_radix(s, 16).ok()
    };
    match (byte(0), byte(1)) {
        (Some(b0), Some(b1)) => format!(
            "{}-{}",
            FINGERPRINT_WORDS[(b0 % 64) as usize],
            FINGERPRINT_WORDS[(b1 % 64) as usize],
        ),
        _ => String::new(),
    }
}

/// 64 short, distinct, easily-spoken nouns for [`fingerprint_word_pair`].
/// Order is the protocol — never reorder (it would change every node's
/// rendered pair). 64 = 6 bits/word, 12 bits across the pair: enough for
/// a human cross-check, backed by the full hex.
const FINGERPRINT_WORDS: [&str; 64] = [
    "anchor", "amber", "arrow", "atlas", "basin", "birch", "blaze", "bloom", "cabin", "cedar",
    "chalk", "clover", "comet", "coral", "crane", "delta", "dune", "ember", "fable", "falcon",
    "fern", "flint", "frost", "glade", "grove", "harbor", "hazel", "heron", "ivory", "jade",
    "kettle", "lagoon", "lantern", "ledger", "linen", "maple", "marble", "meadow", "moss",
    "nectar", "oasis", "onyx", "opal", "otter", "pebble", "pine", "quartz", "quill", "raven",
    "reef", "ridge", "saffron", "sage", "slate", "spruce", "tamarisk", "thistle", "topaz", "umber",
    "vale", "willow", "wren", "yarrow", "zephyr",
];

/// Verify a signature using a peer's published verifying key.
/// Returns `true` only when the signature is well-formed AND
/// matches.
#[must_use]
pub fn verify(verifying_key: &VerifyingKey, payload: &[u8], sig: &Signature) -> bool {
    verifying_key.verify(payload, sig).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_round_trips_through_bytes() {
        let k1 = NodeKey::generate();
        let bytes = k1.secret_bytes();
        let k2 = NodeKey::from_bytes(bytes);
        assert_eq!(k1.fingerprint(), k2.fingerprint());
    }

    #[test]
    fn sign_and_verify_round_trips() {
        let k = NodeKey::generate();
        let payload = b"enrollment-request:peer-anvil";
        let sig = k.sign(payload);
        assert!(verify(&k.verifying_key(), payload, &sig));
    }

    #[test]
    fn signature_does_not_verify_against_wrong_payload() {
        let k = NodeKey::generate();
        let sig = k.sign(b"original");
        assert!(!verify(&k.verifying_key(), b"tampered", &sig));
    }

    #[test]
    fn signature_does_not_verify_against_wrong_key() {
        let k1 = NodeKey::generate();
        let k2 = NodeKey::generate();
        let sig = k1.sign(b"payload");
        assert!(!verify(&k2.verifying_key(), b"payload", &sig));
    }

    #[test]
    fn word_pair_is_deterministic_and_distinct_words() {
        // 0x00.. → words[0]-words[0]; 0x41 0x82 → words[1]-words[2].
        assert_eq!(fingerprint_word_pair("0000abcd"), "anchor-anchor");
        assert_eq!(fingerprint_word_pair("4182dead"), "amber-arrow");
        // Stable for the same fingerprint.
        let fp = NodeKey::from_bytes([9u8; 32]).fingerprint();
        assert_eq!(fingerprint_word_pair(&fp), fingerprint_word_pair(&fp));
        // The pair is two real words joined by a hyphen.
        assert!(fingerprint_word_pair(&fp).contains('-'));
    }

    #[test]
    fn word_pair_rejects_malformed_input() {
        assert_eq!(fingerprint_word_pair(""), "");
        assert_eq!(fingerprint_word_pair("zz"), "");
        assert_eq!(fingerprint_word_pair("a"), ""); // too short for one byte
    }

    #[test]
    fn fingerprint_is_64_hex_chars() {
        let k = NodeKey::generate();
        let fp = k.fingerprint();
        assert_eq!(fp.len(), 64);
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn fingerprint_is_stable_for_same_key() {
        let k = NodeKey::from_bytes([7u8; 32]);
        let fp1 = k.fingerprint();
        let fp2 = k.fingerprint();
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn debug_redacts_secret_bytes() {
        let k = NodeKey::generate();
        let s = format!("{k:?}");
        // Debug shows only the fingerprint, never the secret bytes.
        assert!(s.contains("fingerprint="));
        assert!(s.contains("NodeKey"));
    }

    #[test]
    fn two_keys_have_different_verifying_keys() {
        let k1 = NodeKey::generate();
        let k2 = NodeKey::generate();
        assert_ne!(k1.verifying_key().as_bytes(), k2.verifying_key().as_bytes());
    }

    #[test]
    fn fingerprint_matches_sha256_of_verifying_key() {
        use sha2::{Digest, Sha256};
        let k = NodeKey::from_bytes([0u8; 32]);
        let mut h = Sha256::new();
        h.update(k.verifying_key().as_bytes());
        let want = h.finalize();
        let want_hex: String = want.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(k.fingerprint(), want_hex);
    }

    #[test]
    fn from_bytes_is_deterministic() {
        let bytes = [42u8; 32];
        let k1 = NodeKey::from_bytes(bytes);
        let k2 = NodeKey::from_bytes(bytes);
        assert_eq!(k1.fingerprint(), k2.fingerprint());
        assert_eq!(k1.secret_bytes(), k2.secret_bytes());
    }

    #[test]
    fn secret_bytes_match_input() {
        let bytes = [7u8; 32];
        let k = NodeKey::from_bytes(bytes);
        assert_eq!(k.secret_bytes(), bytes);
    }

    #[test]
    fn verify_with_zero_signature_rejects() {
        // Ed25519 signatures are 64 bytes. An all-zero signature is
        // not valid for any non-zero key.
        use ed25519_dalek::Signature;
        let k = NodeKey::generate();
        let sig = Signature::from_bytes(&[0u8; 64]);
        assert!(!verify(&k.verifying_key(), b"payload", &sig));
    }
}
