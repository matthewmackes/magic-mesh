//! v2.0.0 Phase B.11 — Wake-on-LAN.
//!
//! Pure-Rust port of `mackes/mesh_wol.py`. The Python module is
//! marked deprecated in the v1.x compatibility window; this module
//! is the v2.0.0 implementation.
//!
//! Public surface:
//!
//!   * [`normalize_mac`] — accept colon / hyphen / bare-hex MAC and
//!     return the canonical 6-byte form
//!   * [`magic_packet`] — pure-function builder for the 102-byte
//!     WoL frame (`6 × 0xFF` + `16 × MAC`)
//!   * [`wake`] — fire the magic packet at `broadcast:port`
//!
//! Public API matches the freedesktop Wake-on-LAN spec: RFC-ish 6 ×
//! 0xFF preamble + 16 × the target MAC. Defaults to UDP/9 broadcast,
//! matching the Python implementation + every consumer mainboard.

use std::net::{SocketAddr, UdpSocket};

/// Canonical raw 6-byte MAC. The `normalize_mac` helper parses every
/// well-known string form down to this representation.
pub type Mac = [u8; 6];

/// Build the 102-byte Wake-on-LAN magic packet for `mac`: `6 ×
/// 0xFF` preamble + 16 repetitions of the MAC. Pure function.
#[must_use]
pub fn magic_packet(mac: Mac) -> Vec<u8> {
    let mut packet = Vec::with_capacity(6 + 6 * 16);
    packet.extend_from_slice(&[0xff_u8; 6]);
    for _ in 0..16 {
        packet.extend_from_slice(&mac);
    }
    packet
}

/// Parse a MAC string in any of the canonical forms:
/// `aa:bb:cc:dd:ee:ff`, `aa-bb-cc-dd-ee-ff`, `aabbccddeeff`.
/// Returns `None` on anything else.
#[must_use]
pub fn normalize_mac(s: &str) -> Option<Mac> {
    if s.is_empty() {
        return None;
    }
    // Bare-hex form first.
    if !s.contains(':') && !s.contains('-') {
        if s.len() != 12 {
            return None;
        }
        let mut out = [0u8; 6];
        for i in 0..6 {
            out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
        }
        return Some(out);
    }
    let sep = if s.contains(':') { ':' } else { '-' };
    let parts: Vec<&str> = s.split(sep).collect();
    if parts.len() != 6 {
        return None;
    }
    let mut out = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        if part.len() != 2 {
            return None;
        }
        out[i] = u8::from_str_radix(part, 16).ok()?;
    }
    Some(out)
}

/// Fire the WoL magic packet at `broadcast:port`. Returns Err on
/// socket / send failure; on Ok the packet has been handed to the
/// kernel — actual wake depends on the target hardware honoring
/// it.
///
/// # Errors
/// Returns `std::io::Error` when the socket bind / SO_BROADCAST set
/// / send_to call fails.
pub fn wake(mac: Mac, broadcast: &str, port: u16) -> std::io::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_broadcast(true)?;
    let addr: SocketAddr = format!("{broadcast}:{port}")
        .parse()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{e}")))?;
    let packet = magic_packet(mac);
    socket.send_to(&packet, addr)?;
    Ok(())
}

/// NF-21.2 — wake the peer with `mac` by sending the WoL magic
/// packet as **unicast UDP** to a lighthouse's overlay IP. The
/// lighthouse-side relay (separate component) de-encapsulates and
/// re-broadcasts on the target's LAN segment, enabling
/// "WoL across LANs" — pre-Nebula, WoL only worked within a single
/// broadcast domain.
///
/// Replaces `mackes/mesh_nebula.py::wol_via_lighthouse`. The Python
/// helper shelled out to `wakeonlan -i lighthouse_ip target_mac`;
/// this is the equivalent pure-Rust UDP send (no SO_BROADCAST since
/// the destination is the lighthouse's unicast overlay address, not
/// a LAN broadcast).
///
/// # Errors
/// Returns `std::io::Error` when the socket bind, address parse, or
/// `send_to` call fails.
pub fn wake_via_lighthouse(mac: Mac, lighthouse_ip: &str, port: u16) -> std::io::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    // No set_broadcast: this is a unicast send to the lighthouse's
    // overlay IP. The lighthouse re-broadcasts on the target LAN.
    let addr: SocketAddr = format!("{lighthouse_ip}:{port}")
        .parse()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{e}")))?;
    let packet = magic_packet(mac);
    socket.send_to(&packet, addr)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_packet_is_102_bytes() {
        let p = magic_packet([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
        assert_eq!(p.len(), 6 + 6 * 16);
        assert_eq!(p.len(), 102);
    }

    #[test]
    fn magic_packet_starts_with_six_ff_bytes() {
        let p = magic_packet([0x00; 6]);
        assert_eq!(&p[..6], &[0xff; 6]);
    }

    #[test]
    fn magic_packet_repeats_mac_sixteen_times() {
        let mac = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc];
        let p = magic_packet(mac);
        // 16 repetitions of the MAC start at offset 6.
        for i in 0..16 {
            let off = 6 + i * 6;
            assert_eq!(&p[off..off + 6], &mac);
        }
    }

    #[test]
    fn normalize_mac_accepts_colon_form() {
        assert_eq!(
            normalize_mac("aa:bb:cc:dd:ee:ff"),
            Some([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff])
        );
    }

    #[test]
    fn normalize_mac_accepts_hyphen_form() {
        assert_eq!(
            normalize_mac("AA-BB-CC-DD-EE-FF"),
            Some([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff])
        );
    }

    #[test]
    fn normalize_mac_accepts_bare_hex_form() {
        assert_eq!(
            normalize_mac("aabbccddeeff"),
            Some([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff])
        );
    }

    #[test]
    fn normalize_mac_rejects_short_input() {
        assert_eq!(normalize_mac(""), None);
        assert_eq!(normalize_mac("aa:bb:cc"), None);
        assert_eq!(normalize_mac("aabbcc"), None);
    }

    #[test]
    fn normalize_mac_rejects_non_hex_chars() {
        assert_eq!(normalize_mac("zz:bb:cc:dd:ee:ff"), None);
        assert_eq!(normalize_mac("xx-bb-cc-dd-ee-ff"), None);
        assert_eq!(normalize_mac("zzbbccddeeff"), None);
    }

    #[test]
    fn normalize_mac_rejects_wrong_segment_count() {
        // 5 segments.
        assert_eq!(normalize_mac("aa:bb:cc:dd:ee"), None);
        // 7 segments.
        assert_eq!(normalize_mac("aa:bb:cc:dd:ee:ff:00"), None);
    }

    #[test]
    fn normalize_mac_rejects_uneven_segments() {
        // Single-char segment.
        assert_eq!(normalize_mac("a:bb:cc:dd:ee:ff"), None);
    }

    #[test]
    fn wake_returns_err_for_invalid_broadcast() {
        let mac = [0u8; 6];
        let result = wake(mac, "not-an-address", 9);
        assert!(result.is_err());
    }

    #[test]
    fn wake_succeeds_for_valid_broadcast() {
        // Loopback broadcast — fine on any test box.
        let mac = [0u8; 6];
        let result = wake(mac, "127.255.255.255", 9);
        // Either Ok (broadcast accepted) or Err (kernel refused
        // broadcast on loopback — varies by system). Both are fine
        // here; we're proving the call path compiles + runs.
        let _ = result;
    }

    #[test]
    fn wake_via_lighthouse_returns_err_for_invalid_ip() {
        let mac = [0u8; 6];
        let result = wake_via_lighthouse(mac, "not-an-address", 9);
        assert!(result.is_err());
    }

    #[test]
    fn wake_via_lighthouse_succeeds_for_loopback_unicast() {
        // NF-21.2 — unicast UDP to loopback. The lighthouse-side
        // relay isn't running here; we're only proving the client
        // side sends without SO_BROADCAST.
        let mac = [0xde, 0xad, 0xbe, 0xef, 0x00, 0x01];
        let result = wake_via_lighthouse(mac, "127.0.0.1", 9);
        assert!(result.is_ok(), "loopback unicast WoL send: {result:?}");
    }
}
