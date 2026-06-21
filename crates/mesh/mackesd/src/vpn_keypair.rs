//! VPN-GW-5 — local WireGuard x25519 keypair generation.
//!
//! WireGuard keys are Curve25519 (x25519). A provider tunnel's private key is
//! minted **locally** (never fetched, never reused across tunnels) so only the
//! sealed [`TunnelSecret`](mackes_mesh_types::vpn::TunnelSecret) ever holds it.
//! This module is the §3 crypto boundary the otherwise dependency-light
//! `mackes_mesh_types::vpn_provider` layer relies on: it owns the dalek
//! ecosystem (`x25519-dalek`, sharing `curve25519-dalek` with the node's
//! Ed25519 identity) + the WireGuard-style standard-base64 encoding (the
//! workspace `base64` crate). No OpenSSL.
//!
//! The private key is a SECRET: it leaves this module only inside a
//! [`WgKeypair`](mackes_mesh_types::vpn_provider::WgKeypair) bound for the GW-2
//! seal path — never logged, never on argv. The 32 secret bytes are zeroized
//! after encoding.

use base64::Engine as _;
use mackes_mesh_types::vpn_provider::WgKeypair;
use rand::rngs::OsRng;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroize as _;

/// Generate a fresh WireGuard x25519 keypair, base64-encoded WireGuard-style
/// (standard alphabet, padded — exactly what `wg-quick` reads). The private
/// key is drawn from the OS CSPRNG ([`OsRng`]). The raw 32 secret bytes are
/// zeroized before return; only the base64 strings (the private one bound for
/// the sealed secret) leave this function.
///
/// The returned [`WgKeypair::private_b64`] is a SECRET — the caller seals it
/// via the GW-2 `vpn_secret` path and never logs/argv's it.
#[must_use]
pub fn generate() -> WgKeypair {
    let secret = StaticSecret::random_from_rng(OsRng);
    let public = PublicKey::from(&secret);
    let mut secret_bytes = secret.to_bytes();
    let private_b64 = base64::engine::general_purpose::STANDARD.encode(secret_bytes);
    // Zeroize the raw private bytes the moment they're encoded; the StaticSecret
    // itself zeroizes on drop (x25519-dalek's ZeroizeOnDrop).
    secret_bytes.zeroize();
    let public_b64 = base64::engine::general_purpose::STANDARD.encode(public.as_bytes());
    WgKeypair {
        private_b64,
        public_b64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WireGuard base64 key is 32 bytes → 44 base64 chars (43 + `=`).
    fn is_wg_b64(s: &str) -> bool {
        let decoded = base64::engine::general_purpose::STANDARD.decode(s);
        matches!(decoded, Ok(b) if b.len() == 32)
    }

    #[test]
    fn generate_produces_valid_32_byte_x25519_keys() {
        let kp = generate();
        assert!(
            is_wg_b64(&kp.private_b64),
            "private not 32-byte b64: {}",
            kp.private_b64
        );
        assert!(
            is_wg_b64(&kp.public_b64),
            "public not 32-byte b64: {}",
            kp.public_b64
        );
        assert_eq!(kp.private_b64.len(), 44);
        assert_eq!(kp.public_b64.len(), 44);
    }

    #[test]
    fn each_keypair_is_unique() {
        let a = generate();
        let b = generate();
        assert_ne!(a.private_b64, b.private_b64, "CSPRNG must not repeat");
        assert_ne!(a.public_b64, b.public_b64);
    }

    #[test]
    fn public_key_derives_from_the_private_key() {
        // Re-derive the public key from the decoded private key and confirm it
        // matches — the keypair is internally consistent (a real x25519 pair).
        let kp = generate();
        let priv_bytes: [u8; 32] = base64::engine::general_purpose::STANDARD
            .decode(&kp.private_b64)
            .unwrap()
            .try_into()
            .unwrap();
        let secret = StaticSecret::from(priv_bytes);
        let public = PublicKey::from(&secret);
        let derived = base64::engine::general_purpose::STANDARD.encode(public.as_bytes());
        assert_eq!(derived, kp.public_b64);
    }
}
