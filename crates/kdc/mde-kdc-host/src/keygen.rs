//! RSA-4096 keypair + self-signed identity-cert generation (host increment 3b).
//!
//! `mde-kdc-proto` deliberately ships NO RSA keygen — ring 0.17.x does not expose
//! a stable RSA generator. The host owns it here with the pure-Rust `rsa` crate
//! (one-shot, first-launch only); the hot sign/verify path stays on ring via
//! `mde_kdc_proto::crypto::PairingKeyPair`.
//!
//! Output of [`generate_pkcs8`] is PKCS#8 DER bytes — the same format
//! `PairingKeyPair::from_pkcs8` accepts. [`issue_identity_cert`] binds a
//! self-signed X.509 cert (CN = device id) to that same keypair, so the cert the
//! peer pins and the key the host signs handshakes with are one identity.
//!
//! ## When this fires
//!
//! Once per peer-identity lifetime. The [`PairingStore`](crate::pairing) calls
//! this on first launch when no identity key exists, persists the PKCS#8 to
//! `~/.config/mde/connect/`, and never calls keygen again unless the operator
//! rotates identity.

use rand::rngs::OsRng;
use rsa::pkcs8::EncodePrivateKey;
use rsa::RsaPrivateKey;

/// RSA modulus size in bits. **E11.8 max-crypto: RSA-4096** — the strongest RSA
/// the KDE-Connect-compatible protocol interops with. *Lower* would break
/// stock-client interop, but *higher* does not (the proto verifier accepts
/// `RSA_PKCS1_2048_8192_SHA256`, and peers pin by cert fingerprint, not key
/// size). The earlier 2048 was chosen to avoid "waste"; the operator's
/// maximum-crypto directive supersedes that.
pub const RSA_MODULUS_BITS: usize = 4096;

/// Errors keygen may surface. Stable `Display` tokens for audit-log entries.
#[derive(Debug)]
pub enum KeygenError {
    /// `rsa::RsaPrivateKey::new` failed — practically only when the OS RNG is
    /// broken. Surfaced as an error (not `expect`) so callers choose panic vs retry.
    RsaGenFailed,
    /// PKCS#8 serialization failed — defensive; would imply the `rsa` crate
    /// produced an unserializable key.
    Pkcs8EncodeFailed,
    /// rcgen-based X.509 cert issuance failed; wraps rcgen's own error rendering.
    CertIssueFailed(String),
}

impl std::fmt::Display for KeygenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeygenError::RsaGenFailed => write!(f, "rsa_gen_failed"),
            KeygenError::Pkcs8EncodeFailed => write!(f, "pkcs8_encode_failed"),
            KeygenError::CertIssueFailed(msg) => write!(f, "cert_issue_failed: {msg}"),
        }
    }
}

impl std::error::Error for KeygenError {}

/// Generate a fresh RSA-4096 keypair and return its PKCS#8 DER encoding. Feed the
/// bytes into [`PairingKeyPair::from_pkcs8`](mde_kdc_proto::crypto::PairingKeyPair::from_pkcs8)
/// to get a signable handle backed by ring.
pub fn generate_pkcs8() -> Result<Vec<u8>, KeygenError> {
    let mut rng = OsRng;
    let key =
        RsaPrivateKey::new(&mut rng, RSA_MODULUS_BITS).map_err(|_| KeygenError::RsaGenFailed)?;
    let pkcs8 = key
        .to_pkcs8_der()
        .map_err(|_| KeygenError::Pkcs8EncodeFailed)?;
    Ok(pkcs8.as_bytes().to_vec())
}

/// Issue a self-signed X.509 cert from an existing PKCS#8 RSA keypair. CN =
/// `device_id`; the SHA-256 fingerprint of this cert is the stable identity peers
/// pin in their pairing store. Self-signed + long-lived (100 years) — KDE
/// Connect's model is "the cert IS the identity"; trust is established by
/// fingerprint pinning at first pair, not a CA chain. Returns the cert as DER.
///
/// rcgen 0.13 re-creates the keypair from our PKCS#8 (via a PEM round-trip, the
/// more version-stable path), so the cert binds to the same RSA-4096 keypair the
/// handshake signs with.
pub fn issue_identity_cert(pkcs8_der: &[u8], device_id: &str) -> Result<Vec<u8>, KeygenError> {
    use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_RSA_SHA256};

    let pkcs8_pem = {
        use pkcs8::der::pem::LineEnding;
        use pkcs8::DecodePrivateKey;
        let parsed = rsa::RsaPrivateKey::from_pkcs8_der(pkcs8_der)
            .map_err(|e| KeygenError::CertIssueFailed(format!("decode pkcs8: {e}")))?;
        pkcs8::EncodePrivateKey::to_pkcs8_pem(&parsed, LineEnding::LF)
            .map_err(|e| KeygenError::CertIssueFailed(format!("re-pem pkcs8: {e}")))?
            .to_string()
    };
    let key_pair = KeyPair::from_pkcs8_pem_and_sign_algo(&pkcs8_pem, &PKCS_RSA_SHA256)
        .map_err(|e| KeygenError::CertIssueFailed(format!("rcgen keypair: {e}")))?;

    let mut params = CertificateParams::default();
    params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, device_id.to_string());
        dn
    };
    // 100-year validity — the cert is long-lived; rotation is an operator action,
    // not expiry.
    params.not_before = rcgen::date_time_ymd(2024, 1, 1);
    params.not_after = rcgen::date_time_ymd(2124, 1, 1);

    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| KeygenError::CertIssueFailed(format!("rcgen sign: {e}")))?;

    Ok(cert.der().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_kdc_proto::crypto::{verify_signature, PairingKeyPair};
    use rsa::pkcs8::DecodePrivateKey;
    use std::sync::OnceLock;

    /// One shared production (RSA-4096) PKCS#8, generated once and reused — pure-Rust
    /// 4096 keygen is slow (~a minute), so the cert/round-trip tests share a single
    /// key rather than each paying for a fresh one. (Production keygen is also a
    /// one-time, first-launch-only cost, see the module docs.)
    fn shared_pkcs8() -> &'static [u8] {
        static K: OnceLock<Vec<u8>> = OnceLock::new();
        K.get_or_init(|| generate_pkcs8().expect("keygen succeeds"))
            .as_slice()
    }

    #[test]
    fn keygen_pins_maximum_rsa_4096() {
        // E11.8 max-crypto: the KDC identity key must be RSA-4096 (the strongest RSA
        // the KDE-Connect-compatible protocol interops with — the proto verifier
        // accepts RSA_PKCS1_2048_8192_SHA256, and peers pin by cert fingerprint, so a
        // larger host key still pairs).
        assert_eq!(
            RSA_MODULUS_BITS, 4096,
            "max-crypto: KDC identity key must be RSA-4096"
        );
        // A 4096-bit PKCS#8 DER is ~2.3 KB (vs ~1.2 KB at 2048) — proves the generated
        // key actually doubled, not just the constant.
        let pkcs8 = shared_pkcs8();
        assert!(
            pkcs8.len() > 2000 && pkcs8.len() < 2700,
            "RSA-4096 PKCS#8 DER ~2370 bytes; got {}",
            pkcs8.len()
        );
    }

    #[test]
    fn generate_pkcs8_returns_loadable_keypair() {
        // Round-trip: load the shared key into ring via PairingKeyPair::from_pkcs8 ->
        // sign -> verify against a public key derived from the same private. The
        // bridge between the rsa crate (keygen) and ring (sign/verify).
        let pkcs8 = shared_pkcs8();
        let kp = PairingKeyPair::from_pkcs8(pkcs8).expect("ring accepts our PKCS#8");
        let signature = kp.sign(b"hello").expect("sign succeeds");
        assert!(!signature.is_empty());

        let private = RsaPrivateKey::from_pkcs8_der(pkcs8).unwrap();
        let public = private.to_public_key();
        let pub_der = rsa::pkcs1::EncodeRsaPublicKey::to_pkcs1_der(&public)
            .expect("public key to PKCS#1 DER");

        verify_signature(pub_der.as_bytes(), b"hello", &signature)
            .expect("signature verifies against derived public key");
    }

    #[test]
    fn two_consecutive_keygen_calls_produce_different_keys() {
        // RNG-non-repeat is key-size-independent; generate fast 2048 keys here so the
        // test stays cheap (production size is pinned by keygen_pins_maximum_rsa_4096).
        let fast = || {
            RsaPrivateKey::new(&mut OsRng, 2048)
                .unwrap()
                .to_pkcs8_der()
                .unwrap()
                .as_bytes()
                .to_vec()
        };
        assert_ne!(
            fast(),
            fast(),
            "RNG must not repeat across consecutive calls"
        );
    }

    #[test]
    fn keygen_error_display_is_machine_token() {
        assert_eq!(format!("{}", KeygenError::RsaGenFailed), "rsa_gen_failed");
        assert_eq!(
            format!("{}", KeygenError::Pkcs8EncodeFailed),
            "pkcs8_encode_failed"
        );
        assert!(format!("{}", KeygenError::CertIssueFailed("nope".into()))
            .starts_with("cert_issue_failed: "));
    }

    #[test]
    fn issue_identity_cert_returns_nontrivial_der() {
        let cert_der = issue_identity_cert(shared_pkcs8(), "device-abc-123").unwrap();
        assert!(
            cert_der.len() > 500 && cert_der.len() < 2500,
            "cert DER unexpectedly sized: {}",
            cert_der.len()
        );
    }

    #[test]
    fn issue_identity_cert_embeds_the_device_id_cn() {
        let cert_der = issue_identity_cert(shared_pkcs8(), "device-abc-123").unwrap();
        assert!(
            cert_der.windows(14).any(|w| w == b"device-abc-123"),
            "device-id CN not present in cert DER"
        );
    }

    #[test]
    fn issue_identity_cert_different_device_ids_produce_different_certs() {
        let pkcs8 = shared_pkcs8();
        let c1 = issue_identity_cert(pkcs8, "device-A").unwrap();
        let c2 = issue_identity_cert(pkcs8, "device-B").unwrap();
        assert_ne!(c1, c2, "different device-ids must produce different certs");
        assert!(c1.windows(8).any(|w| w == b"device-A"));
        assert!(c2.windows(8).any(|w| w == b"device-B"));
    }

    #[test]
    fn issue_identity_cert_rejects_invalid_pkcs8() {
        let err = issue_identity_cert(b"not a pkcs8 blob", "device-X")
            .expect_err("garbage PKCS#8 must reject");
        assert!(matches!(err, KeygenError::CertIssueFailed(_)));
    }
}
