//! KDC2-2.4c AES-256-GCM session-encryption tests.
//!
//! seal / open round-trip, tampering detection, nonce-reuse
//! contract (caller-side; library can't detect it but tests lock
//! the documented invariant), key-length validation.

use mde_kdc_proto::crypto::{
    generate_session_key, open_session, seal_session, CryptoError, SESSION_KEY_LEN,
    SESSION_NONCE_LEN,
};

fn fixed_key() -> [u8; SESSION_KEY_LEN] {
    [0x42; SESSION_KEY_LEN]
}

fn fixed_nonce() -> [u8; SESSION_NONCE_LEN] {
    [0x11; SESSION_NONCE_LEN]
}

#[test]
fn seal_then_open_round_trips_with_matching_inputs() {
    let key = fixed_key();
    let nonce = fixed_nonce();
    let aad = b"id=42 kind=kdeconnect.clipboard";
    let plaintext = b"hello clipboard world";

    let ciphertext = seal_session(&key, nonce, aad, plaintext).expect("seal succeeds");
    let recovered = open_session(&key, nonce, aad, &ciphertext).expect("open succeeds");
    assert_eq!(recovered, plaintext);
}

#[test]
fn ciphertext_is_longer_than_plaintext_by_gcm_tag() {
    // AES-256-GCM appends a 16-byte authentication tag.
    let key = fixed_key();
    let nonce = fixed_nonce();
    let plaintext = b"abc";
    let ciphertext = seal_session(&key, nonce, b"", plaintext).unwrap();
    assert_eq!(
        ciphertext.len(),
        plaintext.len() + 16,
        "ciphertext = plaintext + 16-byte GCM tag",
    );
}

#[test]
fn open_rejects_tampered_ciphertext() {
    let key = fixed_key();
    let nonce = fixed_nonce();
    let mut ciphertext = seal_session(&key, nonce, b"meta", b"plaintext").unwrap();
    // Flip a body byte.
    ciphertext[0] ^= 0xff;
    let err =
        open_session(&key, nonce, b"meta", &ciphertext).expect_err("tampered ciphertext must fail");
    assert!(matches!(err, CryptoError::AeadAuthFailed));
}

#[test]
fn open_rejects_tampered_tag() {
    let key = fixed_key();
    let nonce = fixed_nonce();
    let mut ciphertext = seal_session(&key, nonce, b"meta", b"plaintext").unwrap();
    // Flip a tag byte (last 16 bytes are the tag).
    let last = ciphertext.len() - 1;
    ciphertext[last] ^= 0xff;
    let err = open_session(&key, nonce, b"meta", &ciphertext).expect_err("tampered tag must fail");
    assert!(matches!(err, CryptoError::AeadAuthFailed));
}

#[test]
fn open_rejects_swapped_aad() {
    // AAD covers the wire packet's metadata (envelope id +
    // kind). If an attacker swaps it, the GCM tag check fails.
    let key = fixed_key();
    let nonce = fixed_nonce();
    let ciphertext = seal_session(&key, nonce, b"id=1 kind=clipboard", b"x").unwrap();
    let err = open_session(&key, nonce, b"id=2 kind=clipboard", &ciphertext)
        .expect_err("swapped aad must fail");
    assert!(matches!(err, CryptoError::AeadAuthFailed));
}

#[test]
fn open_rejects_wrong_session_key() {
    let nonce = fixed_nonce();
    let ciphertext = seal_session(&fixed_key(), nonce, b"", b"x").unwrap();
    let wrong_key = [0xff_u8; SESSION_KEY_LEN];
    let err = open_session(&wrong_key, nonce, b"", &ciphertext).expect_err("wrong key must fail");
    assert!(matches!(err, CryptoError::AeadAuthFailed));
}

#[test]
fn seal_rejects_wrong_key_length() {
    let short_key = [0x42_u8; 16]; // AES-128 length, not AES-256
    let err = seal_session(&short_key, fixed_nonce(), b"", b"x")
        .expect_err("non-32-byte session key must reject");
    assert!(matches!(err, CryptoError::WrongAlgorithm));
}

#[test]
fn open_rejects_wrong_key_length() {
    let short_key = [0x42_u8; 16];
    let err = open_session(&short_key, fixed_nonce(), b"", b"xxxxxxxxxxxxxxxx")
        .expect_err("non-32-byte session key must reject");
    assert!(matches!(err, CryptoError::WrongAlgorithm));
}

#[test]
fn generate_session_key_returns_32_bytes() {
    let k = generate_session_key().expect("ring RNG works");
    assert_eq!(k.len(), SESSION_KEY_LEN);
    assert_eq!(SESSION_KEY_LEN, 32);
}

#[test]
fn two_generated_session_keys_differ() {
    // Catastrophically unlikely collision — ring RNG must
    // produce distinct keys on consecutive calls.
    let k1 = generate_session_key().unwrap();
    let k2 = generate_session_key().unwrap();
    assert_ne!(k1, k2, "ring RNG must not repeat session keys");
}

#[test]
fn empty_plaintext_seals_and_opens_to_just_the_tag() {
    // Edge case: zero-byte body. ciphertext == 16-byte tag only.
    let key = fixed_key();
    let nonce = fixed_nonce();
    let ciphertext = seal_session(&key, nonce, b"meta", b"").unwrap();
    assert_eq!(ciphertext.len(), 16);
    let plaintext = open_session(&key, nonce, b"meta", &ciphertext).unwrap();
    assert!(plaintext.is_empty());
}

#[test]
fn different_nonces_produce_different_ciphertexts() {
    // Same key + plaintext + AAD; different nonce → ciphertexts
    // must differ. (The nonce counter contract relies on this.)
    let key = fixed_key();
    let mut n1 = [0x11; SESSION_NONCE_LEN];
    let mut n2 = [0x11; SESSION_NONCE_LEN];
    n1[0] = 0x01;
    n2[0] = 0x02;
    let c1 = seal_session(&key, n1, b"", b"same plaintext").unwrap();
    let c2 = seal_session(&key, n2, b"", b"same plaintext").unwrap();
    assert_ne!(
        c1, c2,
        "different nonces must produce different ciphertexts"
    );
}
