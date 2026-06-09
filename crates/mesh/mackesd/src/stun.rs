//! Phase 12.17 — STUN client for ICE candidate gathering.
//!
//! Locked 2026-05-19 (25-Q connectivity survey,
//! `docs/design/v12-connectivity-scope.md`):
//!
//!   * Q8: gather candidates < 1.5 s so total handshake fits the 3 s
//!     first-packet budget.
//!   * Q15: symmetric-NAT edges (~30% of small-business fleets) need
//!     STUN-augmented endpoint advertising to find a hole-punch path
//!     before falling back to DERP.
//!
//! The wire format follows RFC 5389 + 5780 + 8489 (the STUN trio).
//! This module ships:
//!
//!   * [`encode_binding_request`] — pure-fn encoder. Returns the
//!     20-byte fixed header (no attributes). Tests assert the exact
//!     bytes for a known transaction ID.
//!   * [`parse_binding_response`] — pure-fn decoder. Walks the
//!     attribute list and returns the `XOR-MAPPED-ADDRESS` if
//!     present.
//!   * [`gather_endpoint`] — async I/O on top of the encoder/decoder.
//!     Opens a UDP socket, sends one binding request to the given
//!     STUN server, waits up to `timeout` for the response, returns
//!     the public-facing `SocketAddr`. Used by the connectivity
//!     worker to seed Tailscale's endpoint set.
//!
//! All wire parsing is `#[forbid(unsafe_code)]` (workspace rule) and
//! validates lengths before reading — a malformed reply returns an
//! `Err` rather than panicking.

#![cfg(feature = "async-services")]

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use rand::RngCore;
use tokio::net::UdpSocket;

/// STUN Magic Cookie (RFC 5389 §6 — fixed for every message).
pub const MAGIC_COOKIE: u32 = 0x2112_A442;

/// Message type — Binding Request (RFC 5389 §6).
pub const MSG_BINDING_REQUEST: u16 = 0x0001;

/// Message type — Binding Success Response.
pub const MSG_BINDING_SUCCESS: u16 = 0x0101;

/// Message type — Binding Error Response.
pub const MSG_BINDING_ERROR: u16 = 0x0111;

/// Attribute type — XOR-MAPPED-ADDRESS (RFC 5389 §15.2). The
/// authoritative reflexive-address attribute; legacy MAPPED-ADDRESS
/// is intentionally unsupported here (real-world STUN servers all
/// emit XOR-MAPPED-ADDRESS, and parsing only one keeps the surface
/// honest).
pub const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;

/// 12-byte transaction ID. Random per binding request; the response
/// echoes the same bytes so the client can correlate.
pub type TransactionId = [u8; 12];

/// Encode a binding-request message with the given transaction ID
/// and no attributes. Returns the 20-byte header.
///
/// Layout (RFC 5389 §6):
///   [0..2]  Message Type     (0x0001)
///   [2..4]  Message Length   (0 — no attributes)
///   [4..8]  Magic Cookie     (0x2112A442)
///   [8..20] Transaction ID   (12 bytes)
#[must_use]
pub fn encode_binding_request(txid: TransactionId) -> [u8; 20] {
    let mut buf = [0u8; 20];
    buf[0..2].copy_from_slice(&MSG_BINDING_REQUEST.to_be_bytes());
    buf[2..4].copy_from_slice(&0u16.to_be_bytes()); // length
    buf[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
    buf[8..20].copy_from_slice(&txid);
    buf
}

/// Generate a fresh random transaction ID. Uses `rand::thread_rng()`.
#[must_use]
pub fn random_transaction_id() -> TransactionId {
    let mut id = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut id);
    id
}

/// Encode a binding-success response with one XOR-MAPPED-ADDRESS
/// attribute pointing at `reflexive`. Used by the
/// `stun_gather` worker's loopback test responder + by any
/// future operator-mode "be the STUN server" smoke. The encoder
/// matches the parser in [`parse_binding_response`] +
/// [`decode_xor_mapped_address`] so a round-trip lands the
/// caller's address back unchanged.
///
/// IPv4 only; the v12 connectivity-scope lock (Q9) defers IPv6.
#[must_use]
pub fn encode_binding_success_with_xor_mapped(
    txid: TransactionId,
    reflexive: SocketAddr,
) -> Vec<u8> {
    let ip = match reflexive {
        SocketAddr::V4(v4) => v4.ip().octets(),
        SocketAddr::V6(_) => panic!("encode_binding_success_with_xor_mapped: IPv4 only"),
    };
    // X-Port = port XOR high 16 bits of MAGIC_COOKIE.
    let x_port = reflexive.port() ^ ((MAGIC_COOKIE >> 16) as u16);
    // X-Address = addr XOR MAGIC_COOKIE (network-byte-order
    // when read as u32).
    let addr_u32 = u32::from_be_bytes(ip);
    let x_addr_u32 = addr_u32 ^ MAGIC_COOKIE;
    let x_addr = x_addr_u32.to_be_bytes();

    // Attribute body: family (0x01 = IPv4, 1 byte zero-pad +
    // family byte) + X-Port + X-Address.
    let mut attr_value = Vec::with_capacity(8);
    attr_value.push(0x00); // reserved
    attr_value.push(0x01); // family = IPv4
    attr_value.extend_from_slice(&x_port.to_be_bytes());
    attr_value.extend_from_slice(&x_addr);

    // Attribute = type + length + value (no padding since 8 is
    // already 4-aligned).
    let mut attrs = Vec::with_capacity(4 + attr_value.len());
    attrs.extend_from_slice(&ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
    attrs.extend_from_slice(&(attr_value.len() as u16).to_be_bytes());
    attrs.extend_from_slice(&attr_value);

    let mut msg = Vec::with_capacity(20 + attrs.len());
    msg.extend_from_slice(&MSG_BINDING_SUCCESS.to_be_bytes());
    msg.extend_from_slice(&(attrs.len() as u16).to_be_bytes());
    msg.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
    msg.extend_from_slice(&txid);
    msg.extend_from_slice(&attrs);
    msg
}

/// One parsed STUN response. We only model the success case (the
/// caller treats an Err as "try the next STUN server").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingResponse {
    /// Transaction ID echoed back from the request. Caller MUST
    /// verify this matches what they sent before trusting the
    /// reflexive address (defense against spoofed responses).
    pub txid: TransactionId,
    /// Reflexive address as decoded from the XOR-MAPPED-ADDRESS
    /// attribute. `None` if the server didn't include one (which
    /// means the response is unusable for ICE).
    pub reflexive: Option<SocketAddr>,
}

/// Errors that can come out of the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StunError {
    /// Header was too short or malformed.
    Truncated,
    /// Magic cookie didn't match. Either a v0 (pre-RFC-5389) STUN
    /// reply or junk.
    BadMagic,
    /// Message-type field wasn't a binding success response.
    NotBindingSuccess {
        /// The unexpected STUN message-type the server returned.
        kind: u16,
    },
    /// Message-length field didn't agree with the actual buffer
    /// size.
    LengthMismatch {
        /// Length declared in the message header.
        declared: u16,
        /// Actual attribute-section length (`buf.len() - 20`).
        actual: usize,
    },
    /// XOR-MAPPED-ADDRESS attribute had an unknown family byte
    /// (only 0x01 IPv4 + 0x02 IPv6 are valid).
    BadFamily(u8),
    /// XOR-MAPPED-ADDRESS attribute had the wrong length for its
    /// address family.
    BadAddressLength {
        /// Address family declared in the attribute.
        family: u8,
        /// Attribute length the server claimed.
        length: u16,
    },
}

impl std::fmt::Display for StunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => write!(f, "stun: truncated message"),
            Self::BadMagic => write!(f, "stun: bad magic cookie"),
            Self::NotBindingSuccess { kind } => {
                write!(
                    f,
                    "stun: message type 0x{kind:04x} is not a binding success"
                )
            }
            Self::LengthMismatch { declared, actual } => {
                write!(f, "stun: declared length {declared} ≠ actual {actual}")
            }
            Self::BadFamily(b) => write!(f, "stun: bad address family 0x{b:02x}"),
            Self::BadAddressLength { family, length } => write!(
                f,
                "stun: bad XOR-MAPPED-ADDRESS length {length} for family 0x{family:02x}"
            ),
        }
    }
}

impl std::error::Error for StunError {}

/// Parse a binding-response buffer. Validates the magic cookie,
/// walks the attribute list, returns the XOR-MAPPED-ADDRESS if
/// present.
///
/// # Errors
///
/// Returns a [`StunError`] when the wire format is malformed or the
/// message isn't a binding success.
pub fn parse_binding_response(buf: &[u8]) -> Result<BindingResponse, StunError> {
    if buf.len() < 20 {
        return Err(StunError::Truncated);
    }
    let kind = u16::from_be_bytes([buf[0], buf[1]]);
    let length = u16::from_be_bytes([buf[2], buf[3]]);
    let magic = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
    if magic != MAGIC_COOKIE {
        return Err(StunError::BadMagic);
    }
    if kind != MSG_BINDING_SUCCESS {
        return Err(StunError::NotBindingSuccess { kind });
    }
    let body_len = length as usize;
    let expected_total = 20 + body_len;
    if expected_total != buf.len() {
        // RFC 5389 §15: padded attributes are part of the declared
        // length, so the buffer must match exactly.
        return Err(StunError::LengthMismatch {
            declared: length,
            actual: buf.len() - 20,
        });
    }

    let mut txid = [0u8; 12];
    txid.copy_from_slice(&buf[8..20]);

    let mut cursor = 20usize;
    let mut reflexive = None;
    while cursor + 4 <= buf.len() {
        let attr_type = u16::from_be_bytes([buf[cursor], buf[cursor + 1]]);
        let attr_len = u16::from_be_bytes([buf[cursor + 2], buf[cursor + 3]]) as usize;
        let value_start = cursor + 4;
        let value_end = value_start + attr_len;
        if value_end > buf.len() {
            return Err(StunError::Truncated);
        }
        if attr_type == ATTR_XOR_MAPPED_ADDRESS {
            let value = &buf[value_start..value_end];
            reflexive = Some(decode_xor_mapped_address(value, &txid)?);
        }
        // Attributes are 32-bit aligned (RFC 5389 §15) — pad to the
        // next multiple of 4.
        let padded_end = value_end + ((4 - (attr_len % 4)) % 4);
        cursor = padded_end;
    }

    Ok(BindingResponse { txid, reflexive })
}

/// Decode the XOR-MAPPED-ADDRESS attribute body (RFC 5389 §15.2).
fn decode_xor_mapped_address(value: &[u8], txid: &TransactionId) -> Result<SocketAddr, StunError> {
    if value.len() < 4 {
        return Err(StunError::Truncated);
    }
    let family = value[1];
    // [2..4] is the X-Port, XOR'd with the high 16 bits of the magic
    // cookie.
    let x_port = u16::from_be_bytes([value[2], value[3]]);
    let port = x_port ^ ((MAGIC_COOKIE >> 16) as u16);

    match family {
        0x01 => {
            if value.len() != 8 {
                return Err(StunError::BadAddressLength {
                    family,
                    length: value.len() as u16,
                });
            }
            let mut x_addr = [0u8; 4];
            x_addr.copy_from_slice(&value[4..8]);
            let addr_u32 = u32::from_be_bytes(x_addr) ^ MAGIC_COOKIE;
            Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(addr_u32)), port))
        }
        0x02 => {
            if value.len() != 20 {
                return Err(StunError::BadAddressLength {
                    family,
                    length: value.len() as u16,
                });
            }
            let mut xor_key = [0u8; 16];
            xor_key[0..4].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
            xor_key[4..16].copy_from_slice(txid);
            let mut addr = [0u8; 16];
            for i in 0..16 {
                addr[i] = value[4 + i] ^ xor_key[i];
            }
            Ok(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(addr)), port))
        }
        other => Err(StunError::BadFamily(other)),
    }
}

/// One result from an ICE candidate gather. The candidates feed
/// Tailscale's endpoint advertiser (per Q15 lock).
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Reflexive address as reported by the STUN server.
    pub reflexive: SocketAddr,
    /// STUN server we asked. Useful for logs.
    pub server: SocketAddr,
}

/// Send one binding request to `server` and wait up to `timeout`
/// for a binding response. Returns the parsed reflexive address.
///
/// # Errors
///
/// * Network errors (DNS, send/recv, timeout) bubble up as
///   `anyhow::Error`.
/// * Malformed response surfaces as [`StunError`] wrapped in
///   `anyhow::Error`.
/// * Transaction-ID mismatch is treated as a hostile reply and
///   raises an error.
pub async fn gather_endpoint(server: SocketAddr, timeout: Duration) -> anyhow::Result<Candidate> {
    let bind_addr: SocketAddr = match server {
        SocketAddr::V4(_) => "0.0.0.0:0".parse().expect("v4 bind"),
        SocketAddr::V6(_) => "[::]:0".parse().expect("v6 bind"),
    };
    let socket = UdpSocket::bind(bind_addr).await?;
    let txid = random_transaction_id();
    let req = encode_binding_request(txid);
    socket.send_to(&req, server).await?;

    let mut buf = [0u8; 512];
    let recv = tokio::time::timeout(timeout, socket.recv_from(&mut buf)).await;
    let (n, _from) = recv
        .map_err(|_| anyhow::anyhow!("stun: timeout after {timeout:?}"))?
        .map_err(|e| anyhow::anyhow!("stun: recv_from failed: {e}"))?;
    let resp =
        parse_binding_response(&buf[..n]).map_err(|e| anyhow::anyhow!("stun: parse error: {e}"))?;
    if resp.txid != txid {
        return Err(anyhow::anyhow!(
            "stun: transaction-id mismatch (request {txid:?} != response {:?})",
            resp.txid
        ));
    }
    let Some(reflexive) = resp.reflexive else {
        return Err(anyhow::anyhow!("stun: response had no XOR-MAPPED-ADDRESS"));
    };
    Ok(Candidate { reflexive, server })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_cookie_matches_rfc_5389() {
        assert_eq!(MAGIC_COOKIE, 0x2112_A442);
    }

    #[test]
    fn binding_request_has_fixed_20_byte_header() {
        let txid = [0xAA; 12];
        let req = encode_binding_request(txid);
        assert_eq!(req.len(), 20);
        // Message type.
        assert_eq!(&req[0..2], &[0x00, 0x01]);
        // Length (no attributes).
        assert_eq!(&req[2..4], &[0x00, 0x00]);
        // Magic.
        assert_eq!(&req[4..8], &[0x21, 0x12, 0xA4, 0x42]);
        // TXID.
        assert_eq!(&req[8..20], &txid);
    }

    #[test]
    fn parse_rejects_short_buffer() {
        assert_eq!(parse_binding_response(&[]), Err(StunError::Truncated));
        assert_eq!(parse_binding_response(&[0; 19]), Err(StunError::Truncated));
    }

    #[test]
    fn parse_rejects_bad_magic() {
        let mut buf = [0u8; 20];
        buf[0..2].copy_from_slice(&MSG_BINDING_SUCCESS.to_be_bytes());
        buf[4..8].copy_from_slice(&[0xFF; 4]); // wrong magic
        assert_eq!(parse_binding_response(&buf), Err(StunError::BadMagic));
    }

    #[test]
    fn parse_rejects_non_success_kind() {
        let mut buf = [0u8; 20];
        buf[0..2].copy_from_slice(&MSG_BINDING_ERROR.to_be_bytes());
        buf[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
        match parse_binding_response(&buf) {
            Err(StunError::NotBindingSuccess { kind }) => {
                assert_eq!(kind, MSG_BINDING_ERROR);
            }
            other => panic!("expected NotBindingSuccess, got {other:?}"),
        }
    }

    #[test]
    fn round_trip_xor_mapped_address_ipv4() {
        // Synthesise a binding success with one XOR-MAPPED-ADDRESS
        // attribute carrying 203.0.113.42:5555.
        let target_ip = Ipv4Addr::new(203, 0, 113, 42);
        let target_port: u16 = 5555;

        let x_port = target_port ^ ((MAGIC_COOKIE >> 16) as u16);
        let x_addr = u32::from(target_ip) ^ MAGIC_COOKIE;

        let mut buf = Vec::new();
        // Header.
        buf.extend_from_slice(&MSG_BINDING_SUCCESS.to_be_bytes());
        // length placeholder.
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        let txid: TransactionId = [0x11; 12];
        buf.extend_from_slice(&txid);
        // XOR-MAPPED-ADDRESS attribute: type 0x0020, len 8.
        buf.extend_from_slice(&ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
        buf.extend_from_slice(&8u16.to_be_bytes());
        // Reserved + family.
        buf.push(0);
        buf.push(0x01);
        buf.extend_from_slice(&x_port.to_be_bytes());
        buf.extend_from_slice(&x_addr.to_be_bytes());
        // Patch the declared length.
        let length = (buf.len() - 20) as u16;
        buf[2..4].copy_from_slice(&length.to_be_bytes());

        let resp = parse_binding_response(&buf).expect("parsed");
        assert_eq!(resp.txid, txid);
        let reflexive = resp.reflexive.expect("reflexive present");
        assert_eq!(reflexive.ip(), IpAddr::V4(target_ip));
        assert_eq!(reflexive.port(), target_port);
    }

    #[test]
    fn round_trip_xor_mapped_address_ipv6() {
        let target_ip = Ipv6Addr::new(
            0x2001, 0x0db8, 0xfeed, 0x0042, 0x0000, 0x0000, 0xcafe, 0x0001,
        );
        let target_port: u16 = 9090;
        let txid: TransactionId = [0x33; 12];

        let mut xor_key = [0u8; 16];
        xor_key[0..4].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
        xor_key[4..16].copy_from_slice(&txid);
        let ip_octets = target_ip.octets();
        let mut x_addr = [0u8; 16];
        for i in 0..16 {
            x_addr[i] = ip_octets[i] ^ xor_key[i];
        }
        let x_port = target_port ^ ((MAGIC_COOKIE >> 16) as u16);

        let mut buf = Vec::new();
        buf.extend_from_slice(&MSG_BINDING_SUCCESS.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        buf.extend_from_slice(&txid);
        buf.extend_from_slice(&ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
        buf.extend_from_slice(&20u16.to_be_bytes());
        buf.push(0);
        buf.push(0x02);
        buf.extend_from_slice(&x_port.to_be_bytes());
        buf.extend_from_slice(&x_addr);
        let length = (buf.len() - 20) as u16;
        buf[2..4].copy_from_slice(&length.to_be_bytes());

        let resp = parse_binding_response(&buf).expect("parsed");
        let reflexive = resp.reflexive.expect("reflexive present");
        assert_eq!(reflexive.ip(), IpAddr::V6(target_ip));
        assert_eq!(reflexive.port(), target_port);
    }

    #[test]
    fn parse_rejects_bad_family() {
        let txid: TransactionId = [0x44; 12];
        let mut buf = Vec::new();
        buf.extend_from_slice(&MSG_BINDING_SUCCESS.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        buf.extend_from_slice(&txid);
        buf.extend_from_slice(&ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
        buf.extend_from_slice(&8u16.to_be_bytes());
        buf.push(0);
        buf.push(0xFF); // bad family
        buf.extend_from_slice(&[0; 6]);
        let length = (buf.len() - 20) as u16;
        buf[2..4].copy_from_slice(&length.to_be_bytes());

        let err = parse_binding_response(&buf).unwrap_err();
        assert_eq!(err, StunError::BadFamily(0xFF));
    }

    #[test]
    fn parse_rejects_length_mismatch() {
        let txid: TransactionId = [0x55; 12];
        let mut buf = Vec::new();
        buf.extend_from_slice(&MSG_BINDING_SUCCESS.to_be_bytes());
        // Claim 16 attribute bytes but ship 0 — body is empty.
        buf.extend_from_slice(&16u16.to_be_bytes());
        buf.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        buf.extend_from_slice(&txid);

        let err = parse_binding_response(&buf).unwrap_err();
        assert!(matches!(
            err,
            StunError::LengthMismatch {
                declared: 16,
                actual: 0
            }
        ));
    }

    #[test]
    fn random_transaction_ids_differ() {
        let a = random_transaction_id();
        let b = random_transaction_id();
        assert_ne!(a, b, "two random txids must differ (12 bytes of entropy)");
    }

    #[test]
    fn stun_error_display_includes_context() {
        let e = StunError::BadFamily(0xFE);
        assert!(format!("{e}").contains("0xfe"));
        let e = StunError::LengthMismatch {
            declared: 4,
            actual: 2,
        };
        let msg = format!("{e}");
        assert!(msg.contains("4"));
        assert!(msg.contains("2"));
    }

    #[test]
    fn attribute_padding_is_skipped() {
        // Craft a response with an unknown 3-byte attribute (padded
        // to 4) followed by a XOR-MAPPED-ADDRESS so the parser must
        // honour padding.
        let target_ip = Ipv4Addr::new(192, 0, 2, 7);
        let target_port: u16 = 22;
        let x_port = target_port ^ ((MAGIC_COOKIE >> 16) as u16);
        let x_addr = u32::from(target_ip) ^ MAGIC_COOKIE;
        let txid: TransactionId = [0x66; 12];

        let mut buf = Vec::new();
        buf.extend_from_slice(&MSG_BINDING_SUCCESS.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        buf.extend_from_slice(&txid);
        // Unknown attr type 0x802B, len 3, padded to 4.
        buf.extend_from_slice(&0x802Bu16.to_be_bytes());
        buf.extend_from_slice(&3u16.to_be_bytes());
        buf.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0x00]); // 3 bytes + 1 pad
                                                          // XOR-MAPPED-ADDRESS.
        buf.extend_from_slice(&ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
        buf.extend_from_slice(&8u16.to_be_bytes());
        buf.push(0);
        buf.push(0x01);
        buf.extend_from_slice(&x_port.to_be_bytes());
        buf.extend_from_slice(&x_addr.to_be_bytes());

        let length = (buf.len() - 20) as u16;
        buf[2..4].copy_from_slice(&length.to_be_bytes());

        let resp = parse_binding_response(&buf).expect("parsed");
        let reflexive = resp.reflexive.expect("reflexive present");
        assert_eq!(reflexive.ip(), IpAddr::V4(target_ip));
        assert_eq!(reflexive.port(), target_port);
    }

    #[tokio::test]
    async fn gather_endpoint_returns_err_on_timeout() {
        // Talk to a port that's almost certainly not listening; the
        // request will be sent + dropped, and the recv times out.
        // Use the documentation-only TEST-NET-1 address so the
        // datagram doesn't accidentally hit real infrastructure.
        let server: SocketAddr = "192.0.2.1:3478".parse().expect("v4");
        let r = gather_endpoint(server, Duration::from_millis(100)).await;
        assert!(r.is_err(), "expected timeout err, got {r:?}");
    }
}
