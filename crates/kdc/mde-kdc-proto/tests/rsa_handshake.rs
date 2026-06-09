//! KDC2-2.4b RSA-2048 pairing-handshake integration tests.
//!
//! Uses pre-generated RSA-2048 key fixtures (PKCS#8 private +
//! PKCS#1 raw public) under `tests/fixtures/`. ring 0.17 does not
//! expose RSA keypair generation; the host integration (KDC2-3)
//! pulls in the pure-Rust `rsa = "0.9"` crate just for keygen, so
//! the protocol crate ships pre-baked fixtures for its own tests.
//!
//! Fixtures were generated 2026-05-22 via:
//!
//! ```text
//! openssl genrsa -out rsa.pem 2048
//! openssl pkcs8 -topk8 -inform PEM -outform DER -nocrypt \
//!   -in rsa.pem -out rsa_2048_pkcs8.der
//! openssl rsa -in rsa.pem -RSAPublicKey_out -outform DER \
//!   -out rsa_2048_pub_pkcs1.der
//! ```

use mde_kdc_proto::crypto::{verify_signature, CryptoError, PairingKeyPair};

fn private_key_der() -> Vec<u8> {
    std::fs::read("tests/fixtures/rsa_2048_pkcs8.der").expect("rsa_2048_pkcs8.der fixture missing")
}

fn public_key_der() -> Vec<u8> {
    std::fs::read("tests/fixtures/rsa_2048_pub_pkcs1.der")
        .expect("rsa_2048_pub_pkcs1.der fixture missing")
}

#[test]
fn pairing_keypair_loads_from_pkcs8() {
    let kp = PairingKeyPair::from_pkcs8(&private_key_der());
    assert!(kp.is_ok(), "fixture should load via from_pkcs8");
}

#[test]
fn pairing_keypair_rejects_garbage_pkcs8() {
    let err =
        PairingKeyPair::from_pkcs8(b"not a real PKCS#8 blob").expect_err("garbage must not load");
    assert!(matches!(err, CryptoError::WrongAlgorithm));
}

#[test]
fn pkcs8_bytes_round_trip() {
    let der = private_key_der();
    let kp = PairingKeyPair::from_pkcs8(&der).unwrap();
    // PairingKeyPair retains the input bytes verbatim — the host's
    // persistence layer (KDC2-3) relies on this for write-back.
    assert_eq!(kp.pkcs8_bytes(), der.as_slice());
}

#[test]
fn sign_then_verify_succeeds_for_matched_pair() {
    let kp = PairingKeyPair::from_pkcs8(&private_key_der()).unwrap();
    let message = b"kdc pairing challenge";
    let sig = kp.sign(message).expect("sign succeeds with valid keypair");
    verify_signature(&public_key_der(), message, &sig).expect("verify succeeds for matched pair");
}

#[test]
fn verify_rejects_tampered_message() {
    let kp = PairingKeyPair::from_pkcs8(&private_key_der()).unwrap();
    let sig = kp.sign(b"original message").unwrap();
    // Same signature, different message — verification must fail.
    let err = verify_signature(&public_key_der(), b"tampered message", &sig)
        .expect_err("tampered message must fail verification");
    assert!(matches!(err, CryptoError::SignatureInvalid));
}

#[test]
fn verify_rejects_tampered_signature() {
    let kp = PairingKeyPair::from_pkcs8(&private_key_der()).unwrap();
    let message = b"kdc pairing challenge";
    let mut sig = kp.sign(message).unwrap();
    // Flip a byte in the signature; verify must fail.
    let i = sig.len() / 2;
    sig[i] ^= 0xff;
    let err = verify_signature(&public_key_der(), message, &sig)
        .expect_err("tampered signature must fail verification");
    assert!(matches!(err, CryptoError::SignatureInvalid));
}

#[test]
fn verify_rejects_garbage_public_key() {
    let kp = PairingKeyPair::from_pkcs8(&private_key_der()).unwrap();
    let sig = kp.sign(b"x").unwrap();
    // Garbage public key — ring surfaces a verify failure.
    let err = verify_signature(b"not a public key", b"x", &sig)
        .expect_err("garbage public key must fail verification");
    assert!(matches!(err, CryptoError::SignatureInvalid));
}

#[test]
fn pairing_keypair_debug_does_not_leak_key_material() {
    let kp = PairingKeyPair::from_pkcs8(&private_key_der()).unwrap();
    let dbg = format!("{kp:?}");
    // PKCS#8 DER bytes are 1218 bytes for a 2048-bit key. The
    // Debug output must contain none of them.
    assert!(!dbg.contains("der_bytes"), "Debug leaks der_bytes field");
    assert!(dbg.contains("PairingKeyPair"));
}
