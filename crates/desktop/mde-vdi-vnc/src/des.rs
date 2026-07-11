//! DES + RFB "VNC Authentication" (security type 2) challenge/response.
//!
//! RFB security type 2 authenticates the client with a shared password. The
//! server sends a 16-byte random challenge; the client encrypts it with single
//! DES (ECB, two independent 8-byte blocks) keyed by the password and returns
//! the 16-byte cipher as the response. The password → key step has the classic
//! RFB quirk: each key byte's bit order is reversed before it is used as the DES
//! key (a side effect of the original `d3des` reference reading the key
//! LSB-first). Passwords are truncated/zero-padded to the DES 8-byte key.
//!
//! This is a pure-Rust, dependency-free implementation — the crate ships no
//! external protocol/crypto dependency (§4). It is validated against the
//! canonical FIPS-81 / "DES Algorithm Illustrated" test vector and, through the
//! RFB bit-reversal quirk, against reference (OpenSSL) DES-ECB output (see the
//! unit tests).
//!
//! DES alone is broken for confidentiality, but RFB type 2 is the wire protocol
//! XCP-ng `Xvnc` consoles and classic VNC servers speak; interoperating with
//! them is the point. TLS/VeNCrypt wrapping is the transport-security story
//! layered on top of this handshake.

#![allow(
    clippy::cast_possible_truncation,
    reason = "DES operates on fixed-width bit fields; each u64->u32 cast narrows a \
              value already masked or shifted into a 32-bit sub-word (a block half \
              or the 32-bit S-box result)"
)]

/// Compute the 16-byte RFB "VNC Authentication" response for `challenge` under
/// `password`.
///
/// `password` is truncated to 8 bytes (or zero-padded if shorter) and each byte
/// is bit-reversed to form the DES key (the RFB quirk). The 16-byte challenge is
/// then encrypted as two independent single-DES ECB blocks.
#[must_use]
pub fn vnc_auth_response(password: &[u8], challenge: &[u8; 16]) -> [u8; 16] {
    let key = rfb_des_key(password);
    let mut response = [0u8; 16];
    for (chunk, out) in challenge.chunks_exact(8).zip(response.chunks_exact_mut(8)) {
        let mut block = [0u8; 8];
        block.copy_from_slice(chunk);
        out.copy_from_slice(&des_encrypt_block(key, block));
    }
    response
}

/// Derive the 8-byte DES key from an RFB password: take up to 8 bytes
/// (zero-padded if shorter, truncated if longer) and reverse each byte's bits.
fn rfb_des_key(password: &[u8]) -> [u8; 8] {
    let mut key = [0u8; 8];
    for (slot, &byte) in key.iter_mut().zip(password.iter()) {
        *slot = byte.reverse_bits();
    }
    key
}

/// Encrypt one 8-byte block with single DES (ECB) under `key`.
fn des_encrypt_block(key: [u8; 8], block: [u8; 8]) -> [u8; 8] {
    let subkeys = key_schedule(key);
    let permuted = permute(u64::from_be_bytes(block), &IP, 64);
    let mut left = (permuted >> 32) as u32;
    let mut right = permuted as u32;
    for subkey in subkeys {
        let next = left ^ feistel(right, subkey);
        left = right;
        right = next;
    }
    // The DES pre-output block swaps the halves: R16 || L16.
    let preoutput = (u64::from(right) << 32) | u64::from(left);
    permute(preoutput, &FP, 64).to_be_bytes()
}

/// The Feistel round function `f(R, K)`: expand, key-mix, substitute, permute.
fn feistel(right: u32, subkey: u64) -> u32 {
    let mixed = permute(u64::from(right), &E, 32) ^ subkey; // 48 bits
    let mut substituted = 0u32;
    for (i, sbox) in SBOXES.iter().enumerate() {
        let six = ((mixed >> (42 - 6 * i)) & 0x3f) as usize;
        // Row = outer two bits (1 and 6); column = the middle four bits.
        let row = ((six & 0x20) >> 4) | (six & 0x01);
        let col = (six >> 1) & 0x0f;
        substituted |= u32::from(sbox[row * 16 + col]) << (28 - 4 * i);
    }
    permute(u64::from(substituted), &P, 32) as u32
}

/// Expand the 8-byte key into the 16 round subkeys (each 48 bits, low-aligned).
fn key_schedule(key: [u8; 8]) -> [u64; 16] {
    let permuted = permute(u64::from_be_bytes(key), &PC1, 64); // 56 bits
    let mut c = ((permuted >> 28) & 0x0fff_ffff) as u32;
    let mut d = (permuted & 0x0fff_ffff) as u32;
    let mut subkeys = [0u64; 16];
    for (subkey, &shift) in subkeys.iter_mut().zip(SHIFTS.iter()) {
        c = rotl28(c, shift);
        d = rotl28(d, shift);
        let cd = (u64::from(c) << 28) | u64::from(d);
        *subkey = permute(cd, &PC2, 56);
    }
    subkeys
}

/// Rotate a 28-bit value left by `shift` bits.
const fn rotl28(value: u32, shift: u32) -> u32 {
    ((value << shift) | (value >> (28 - shift))) & 0x0fff_ffff
}

/// Apply a DES bit-permutation table.
///
/// `input` holds a big-endian bit field whose most-significant bit is DES "bit
/// 1"; `in_bits` is that field's width. Each `table` entry is a 1-indexed source
/// bit position; the output is `table.len()` bits wide, left-aligned MSB-first.
fn permute(input: u64, table: &[u8], in_bits: u32) -> u64 {
    let mut out = 0u64;
    for &pos in table {
        let bit = (input >> (in_bits - u32::from(pos))) & 1;
        out = (out << 1) | bit;
    }
    out
}

/// Initial permutation.
#[rustfmt::skip]
const IP: [u8; 64] = [
    58, 50, 42, 34, 26, 18, 10, 2,
    60, 52, 44, 36, 28, 20, 12, 4,
    62, 54, 46, 38, 30, 22, 14, 6,
    64, 56, 48, 40, 32, 24, 16, 8,
    57, 49, 41, 33, 25, 17,  9, 1,
    59, 51, 43, 35, 27, 19, 11, 3,
    61, 53, 45, 37, 29, 21, 13, 5,
    63, 55, 47, 39, 31, 23, 15, 7,
];

/// Final permutation (inverse of [`IP`]).
#[rustfmt::skip]
const FP: [u8; 64] = [
    40, 8, 48, 16, 56, 24, 64, 32,
    39, 7, 47, 15, 55, 23, 63, 31,
    38, 6, 46, 14, 54, 22, 62, 30,
    37, 5, 45, 13, 53, 21, 61, 29,
    36, 4, 44, 12, 52, 20, 60, 28,
    35, 3, 43, 11, 51, 19, 59, 27,
    34, 2, 42, 10, 50, 18, 58, 26,
    33, 1, 41,  9, 49, 17, 57, 25,
];

/// Expansion permutation (32 → 48 bits) in the Feistel function.
#[rustfmt::skip]
const E: [u8; 48] = [
    32,  1,  2,  3,  4,  5,
     4,  5,  6,  7,  8,  9,
     8,  9, 10, 11, 12, 13,
    12, 13, 14, 15, 16, 17,
    16, 17, 18, 19, 20, 21,
    20, 21, 22, 23, 24, 25,
    24, 25, 26, 27, 28, 29,
    28, 29, 30, 31, 32,  1,
];

/// Permutation applied to the S-box output.
#[rustfmt::skip]
const P: [u8; 32] = [
    16,  7, 20, 21,
    29, 12, 28, 17,
     1, 15, 23, 26,
     5, 18, 31, 10,
     2,  8, 24, 14,
    32, 27,  3,  9,
    19, 13, 30,  6,
    22, 11,  4, 25,
];

/// Permuted choice 1 (64 → 56 key bits, dropping parity bits).
#[rustfmt::skip]
const PC1: [u8; 56] = [
    57, 49, 41, 33, 25, 17,  9,
     1, 58, 50, 42, 34, 26, 18,
    10,  2, 59, 51, 43, 35, 27,
    19, 11,  3, 60, 52, 44, 36,
    63, 55, 47, 39, 31, 23, 15,
     7, 62, 54, 46, 38, 30, 22,
    14,  6, 61, 53, 45, 37, 29,
    21, 13,  5, 28, 20, 12,  4,
];

/// Permuted choice 2 (56 → 48 subkey bits).
#[rustfmt::skip]
const PC2: [u8; 48] = [
    14, 17, 11, 24,  1,  5,
     3, 28, 15,  6, 21, 10,
    23, 19, 12,  4, 26,  8,
    16,  7, 27, 20, 13,  2,
    41, 52, 31, 37, 47, 55,
    30, 40, 51, 45, 33, 48,
    44, 49, 39, 56, 34, 53,
    46, 42, 50, 36, 29, 32,
];

/// Left-rotation schedule for the 28-bit key halves, one entry per round.
const SHIFTS: [u32; 16] = [1, 1, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 2, 2, 2, 1];

/// The eight DES S-boxes, each a flattened 4-row × 16-column lookup.
#[rustfmt::skip]
const SBOXES: [[u8; 64]; 8] = [
    [
        14,  4, 13,  1,  2, 15, 11,  8,  3, 10,  6, 12,  5,  9,  0,  7,
         0, 15,  7,  4, 14,  2, 13,  1, 10,  6, 12, 11,  9,  5,  3,  8,
         4,  1, 14,  8, 13,  6,  2, 11, 15, 12,  9,  7,  3, 10,  5,  0,
        15, 12,  8,  2,  4,  9,  1,  7,  5, 11,  3, 14, 10,  0,  6, 13,
    ],
    [
        15,  1,  8, 14,  6, 11,  3,  4,  9,  7,  2, 13, 12,  0,  5, 10,
         3, 13,  4,  7, 15,  2,  8, 14, 12,  0,  1, 10,  6,  9, 11,  5,
         0, 14,  7, 11, 10,  4, 13,  1,  5,  8, 12,  6,  9,  3,  2, 15,
        13,  8, 10,  1,  3, 15,  4,  2, 11,  6,  7, 12,  0,  5, 14,  9,
    ],
    [
        10,  0,  9, 14,  6,  3, 15,  5,  1, 13, 12,  7, 11,  4,  2,  8,
        13,  7,  0,  9,  3,  4,  6, 10,  2,  8,  5, 14, 12, 11, 15,  1,
        13,  6,  4,  9,  8, 15,  3,  0, 11,  1,  2, 12,  5, 10, 14,  7,
         1, 10, 13,  0,  6,  9,  8,  7,  4, 15, 14,  3, 11,  5,  2, 12,
    ],
    [
         7, 13, 14,  3,  0,  6,  9, 10,  1,  2,  8,  5, 11, 12,  4, 15,
        13,  8, 11,  5,  6, 15,  0,  3,  4,  7,  2, 12,  1, 10, 14,  9,
        10,  6,  9,  0, 12, 11,  7, 13, 15,  1,  3, 14,  5,  2,  8,  4,
         3, 15,  0,  6, 10,  1, 13,  8,  9,  4,  5, 11, 12,  7,  2, 14,
    ],
    [
         2, 12,  4,  1,  7, 10, 11,  6,  8,  5,  3, 15, 13,  0, 14,  9,
        14, 11,  2, 12,  4,  7, 13,  1,  5,  0, 15, 10,  3,  9,  8,  6,
         4,  2,  1, 11, 10, 13,  7,  8, 15,  9, 12,  5,  6,  3,  0, 14,
        11,  8, 12,  7,  1, 14,  2, 13,  6, 15,  0,  9, 10,  4,  5,  3,
    ],
    [
        12,  1, 10, 15,  9,  2,  6,  8,  0, 13,  3,  4, 14,  7,  5, 11,
        10, 15,  4,  2,  7, 12,  9,  5,  6,  1, 13, 14,  0, 11,  3,  8,
         9, 14, 15,  5,  2,  8, 12,  3,  7,  0,  4, 10,  1, 13, 11,  6,
         4,  3,  2, 12,  9,  5, 15, 10, 11, 14,  1,  7,  6,  0,  8, 13,
    ],
    [
         4, 11,  2, 14, 15,  0,  8, 13,  3, 12,  9,  7,  5, 10,  6,  1,
        13,  0, 11,  7,  4,  9,  1, 10, 14,  3,  5, 12,  2, 15,  8,  6,
         1,  4, 11, 13, 12,  3,  7, 14, 10, 15,  6,  8,  0,  5,  9,  2,
         6, 11, 13,  8,  1,  4, 10,  7,  9,  5,  0, 15, 14,  2,  3, 12,
    ],
    [
        13,  2,  8,  4,  6, 15, 11,  1, 10,  9,  3, 14,  5,  0, 12,  7,
         1, 15, 13,  8, 10,  3,  7,  4, 12,  5,  6, 11,  0, 14,  9,  2,
         7, 11,  4,  1,  9, 12, 14,  2,  0,  6, 10, 13, 15,  3,  5,  8,
         2,  1, 14,  7,  4, 10,  8, 13, 15, 12,  9,  0,  3,  5,  6, 11,
    ],
];

#[cfg(test)]
mod tests {
    use super::{des_encrypt_block, rfb_des_key, vnc_auth_response};

    // Canonical FIPS-81 / "The DES Algorithm Illustrated" worked example — the
    // single most widely published single-DES test vector. Independently
    // reproduced with `openssl enc -des-ecb -nopad -K 133457799BBCDFF1`.
    const FIPS_KEY: [u8; 8] = [0x13, 0x34, 0x57, 0x79, 0x9b, 0xbc, 0xdf, 0xf1];
    const FIPS_PLAINTEXT: [u8; 8] = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
    const FIPS_CIPHERTEXT: [u8; 8] = [0x85, 0xe8, 0x13, 0x54, 0x0f, 0x0a, 0xb4, 0x05];

    #[test]
    fn des_core_matches_canonical_fips_vector() {
        assert_eq!(
            des_encrypt_block(FIPS_KEY, FIPS_PLAINTEXT),
            FIPS_CIPHERTEXT,
            "single-DES ECB must match the canonical FIPS worked example"
        );
    }

    #[test]
    fn rfb_key_bit_reverses_each_password_byte_and_pads() {
        // Each byte's bit order is reversed; a short password zero-pads to 8.
        assert_eq!(rfb_des_key(&[0x01]), [0x80, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(rfb_des_key(&[0xff, 0x0f]), [0xff, 0xf0, 0, 0, 0, 0, 0, 0]);
        // "pass" → p=0x70 a=0x61 s=0x73 s=0x73, bit-reversed to 0e 86 ce ce.
        // (Matches `openssl` key derivation: 0e86cece00000000.)
        assert_eq!(rfb_des_key(b"pass"), [0x0e, 0x86, 0xce, 0xce, 0, 0, 0, 0]);
        // A password longer than 8 bytes is truncated to the DES key length.
        assert_eq!(rfb_des_key(b"0123456789"), rfb_des_key(b"01234567"));
    }

    #[test]
    fn vnc_auth_response_reproduces_fips_vector_through_bit_reversal() {
        // The RFB key is the bit-reversed password, so a password whose bytes are
        // the bit-reversed FIPS key drives the DES core with the canonical FIPS
        // key. Both challenge halves are the FIPS plaintext, so both response
        // blocks must equal the canonical FIPS ciphertext.
        let password: Vec<u8> = FIPS_KEY.iter().map(|b| b.reverse_bits()).collect();
        let mut challenge = [0u8; 16];
        challenge[0..8].copy_from_slice(&FIPS_PLAINTEXT);
        challenge[8..16].copy_from_slice(&FIPS_PLAINTEXT);
        let response = vnc_auth_response(&password, &challenge);
        assert_eq!(&response[0..8], &FIPS_CIPHERTEXT);
        assert_eq!(&response[8..16], &FIPS_CIPHERTEXT);
    }

    #[test]
    fn vnc_auth_response_matches_known_rfb_vectors() {
        // Known RFB "VNC Authentication" vectors, each cross-checked against an
        // independent reference DES (OpenSSL `enc -des-ecb -nopad`) with the
        // RFB-quirk bit-reversed key.
        //
        //   password "pass" (key 0e86cece00000000), challenge 0x00..0x0f
        let challenge_a: [u8; 16] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f,
        ];
        assert_eq!(
            vnc_auth_response(b"pass", &challenge_a),
            [
                0x5f, 0xb0, 0x2f, 0x4e, 0x6e, 0xc9, 0xfd, 0xa0, 0x6c, 0x41, 0xdf, 0x1f, 0x35, 0x01,
                0x51, 0x38,
            ]
        );

        //   password "letmein!" (full 8 bytes, key 36a62eb6a6967684), 0x01..0x10
        let challenge_b: [u8; 16] = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
            0x0f, 0x10,
        ];
        assert_eq!(
            vnc_auth_response(b"letmein!", &challenge_b),
            [
                0xe1, 0xbe, 0xf5, 0x8b, 0x8c, 0x01, 0xb4, 0x7a, 0x9b, 0xdc, 0x63, 0x2c, 0x2c, 0xd7,
                0xb2, 0x79,
            ]
        );
    }
}
