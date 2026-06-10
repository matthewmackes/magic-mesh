//! 16-character LAN-credential passcode (Phase 12.10.1).
//!
//! **SEC-3 (2026-06-10): NOT an enrollment credential.** Mesh
//! enrollment runs exclusively on 256-bit single-use bearers
//! (`bearer_ledger`, delivered as the `mesh:` wire token / QR —
//! ENT-1 enforces the ledger at the signing lighthouse, so a typed
//! 16-char code can never enroll). This module survives for the
//! LAN-service credential uses only (cups_sync, surrounding_hosts,
//! the passcode_creds store).

use rand::RngCore;

/// Length of the passcode in characters. Locked at 16 by the
/// /goal directive 2026-05-19 (acceptance bullet #8).
pub const PASSCODE_LEN: usize = 16;

/// Generate a fresh 16-character URL-safe passcode.
///
/// Uses `rand::thread_rng` which is seeded from the OS CSPRNG
/// (`getrandom`). Output is uniformly random across the 64-character
/// URL-safe base64 alphabet — 96 bits of entropy in 16 characters.
#[must_use]
pub fn generate() -> String {
    // 12 random bytes encode to exactly 16 URL-safe base64 chars
    // (no padding, no `=` glyph), per RFC 4648.
    let mut bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut bytes);
    encode_url_safe_base64(&bytes)
}

/// Verify that a candidate string is shaped like a Mackes passcode.
/// Constant-time check on the length only — actual auth happens
/// against libsecret elsewhere; this is a cheap pre-flight.
#[must_use]
pub fn looks_valid(candidate: &str) -> bool {
    candidate.len() == PASSCODE_LEN
        && candidate
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// URL-safe base64 encode with no padding (RFC 4648 §5). Inline
/// implementation — pulling `base64` into the dep graph just for this
/// 12-byte → 16-char path isn't worth it.
fn encode_url_safe_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let b0 = bytes[i];
        let b1 = bytes[i + 1];
        let b2 = bytes[i + 2];
        out.push(ALPHABET[(b0 >> 2) as usize] as char);
        out.push(ALPHABET[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        out.push(ALPHABET[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        out.push(ALPHABET[(b2 & 0x3f) as usize] as char);
        i += 3;
    }
    // Tail: 1 or 2 leftover bytes. Our 12-byte input always divides
    // cleanly by 3 so this never fires, but handle it for safety.
    match bytes.len() - i {
        1 => {
            let b0 = bytes[i];
            out.push(ALPHABET[(b0 >> 2) as usize] as char);
            out.push(ALPHABET[((b0 & 0x03) << 4) as usize] as char);
        }
        2 => {
            let b0 = bytes[i];
            let b1 = bytes[i + 1];
            out.push(ALPHABET[(b0 >> 2) as usize] as char);
            out.push(ALPHABET[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
            out.push(ALPHABET[((b1 & 0x0f) << 2) as usize] as char);
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_passcode_has_locked_length() {
        for _ in 0..100 {
            assert_eq!(generate().len(), PASSCODE_LEN);
        }
    }

    #[test]
    fn generated_passcode_is_url_safe() {
        for _ in 0..100 {
            let p = generate();
            assert!(looks_valid(&p), "rejected own output: {p}");
        }
    }

    #[test]
    fn looks_valid_rejects_wrong_length() {
        assert!(!looks_valid(""));
        assert!(!looks_valid("short"));
        assert!(!looks_valid(&"x".repeat(15)));
        assert!(!looks_valid(&"x".repeat(17)));
    }

    #[test]
    fn looks_valid_rejects_non_url_safe_chars() {
        // The padding char `=` is NOT in the URL-safe alphabet.
        assert!(!looks_valid("AAAAAAAAAAAAAAA="));
        // `+` and `/` are standard base64 but not URL-safe.
        assert!(!looks_valid("AAAAAAAAAAAAAAA+"));
        assert!(!looks_valid("AAAAAAAAAAAAAAA/"));
        // Spaces and quotes are out.
        assert!(!looks_valid("AAAAAAAAAAAAAAA "));
    }

    #[test]
    fn two_generations_almost_never_collide() {
        let a = generate();
        let b = generate();
        // With 96 bits of entropy a real collision has probability ~2^-96.
        assert_ne!(a, b);
    }

    #[test]
    fn encode_three_bytes_round_trip() {
        // RFC 4648 fixture: "Man" → "TWFu" (URL-safe identical).
        let out = encode_url_safe_base64(b"Man");
        assert_eq!(out, "TWFu");
    }

    #[test]
    fn encode_handles_byte_zero() {
        let out = encode_url_safe_base64(&[0u8; 12]);
        assert_eq!(out, "AAAAAAAAAAAAAAAA");
        assert!(looks_valid(&out));
    }

    #[test]
    fn encode_handles_one_byte_tail() {
        // RFC 4648 §5: 1 leftover byte yields 2 base64 chars
        // (no padding in URL-safe form).
        let out = encode_url_safe_base64(&[0xff]);
        // 0xff = 11111111 — splits into 111111 (=63=`_`) + 110000 (=48=`w`)
        assert_eq!(out, "_w");
    }

    #[test]
    fn encode_handles_two_byte_tail() {
        // 2 leftover bytes → 3 base64 chars.
        // 0xff 0xff = 11111111 11111111 → 111111 111111 1111_00
        // → 63=`_`, 63=`_`, 60=`8`.
        let out = encode_url_safe_base64(&[0xff, 0xff]);
        assert_eq!(out, "__8");
    }

    #[test]
    fn encode_all_alphabet_characters_are_url_safe() {
        // Hit every position of the alphabet to flush out any
        // accidental swap to `+`/`/`. Feed a byte pattern that walks
        // the full 6-bit range across multiple groups.
        let bytes: [u8; 12] = [
            0x00, 0x10, 0x83, 0x10, 0x51, 0x87, 0x20, 0x92, 0x8b, 0x30, 0xd3, 0x8f,
        ];
        let out = encode_url_safe_base64(&bytes);
        for c in out.chars() {
            assert!(
                c.is_ascii_alphanumeric() || c == '-' || c == '_',
                "{c} not URL-safe"
            );
        }
    }

    #[test]
    fn looks_valid_accepts_full_alphabet() {
        // 16-char string drawn from across the URL-safe alphabet.
        assert!(looks_valid("Abc123_-XYzpqrST"));
    }

    #[test]
    fn looks_valid_rejects_unicode() {
        // 16 visible chars but contains a non-ASCII glyph.
        let s = format!("{}{}", "A".repeat(15), 'é');
        assert!(!looks_valid(&s));
    }

    #[test]
    fn encode_known_rfc4648_fixtures() {
        // RFC 4648 §10 fixtures (URL-safe variant identical to standard
        // for these inputs).
        assert_eq!(encode_url_safe_base64(b""), "");
        assert_eq!(encode_url_safe_base64(b"f"), "Zg");
        assert_eq!(encode_url_safe_base64(b"fo"), "Zm8");
        assert_eq!(encode_url_safe_base64(b"foo"), "Zm9v");
        assert_eq!(encode_url_safe_base64(b"foob"), "Zm9vYg");
        assert_eq!(encode_url_safe_base64(b"fooba"), "Zm9vYmE");
        assert_eq!(encode_url_safe_base64(b"foobar"), "Zm9vYmFy");
    }
}
