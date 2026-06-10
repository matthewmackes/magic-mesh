//! PD-6 / ENT-13 — the transport-layer RTT probe.
//!
//! Measures real overlay round-trip by timing a TCP handshake
//! **through the Nebula tunnel** to the peer's overlay address:
//! a SYN→SYN-ACK (connect) or SYN→RST (refused) both traverse the
//! actual transport path, so the elapsed time IS the overlay RTT —
//! no ICMP, no `ping` shell-out, no extra deps. A refused connect is
//! a *successful* measurement (the peer's stack answered); only a
//! timeout means unreachable.
//!
//! Direct-vs-relay / NAT-class detail needs the Nebula admin socket
//! (not yet provisioned in our config) — the probe reports
//! `path: "overlay"` until that introspection lands; it never guesses.

use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Probe deadline — an overlay peer answers in tens of ms; 1.5 s
/// covers a congested relay hop with margin.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

/// The discard port — almost certainly closed, which is fine: the
/// RST still rides the tunnel and times the path.
pub const PROBE_PORT: u16 = 9;

/// One probe outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeResult {
    /// Measured overlay RTT in milliseconds; `None` when unreachable.
    pub rtt_ms: Option<f64>,
    /// Whether the peer's stack answered at all (connect OR refuse).
    pub reachable: bool,
    /// How the path was measured / classified. `"overlay"` until
    /// Nebula admin introspection can say direct vs relay.
    pub path: &'static str,
}

/// Probe `overlay_ip` once. Blocking (≤ [`PROBE_TIMEOUT`]) — call
/// off the async executor.
#[must_use]
pub fn probe_rtt(overlay_ip: &str) -> ProbeResult {
    let Ok(addr) = format!("{overlay_ip}:{PROBE_PORT}").parse::<SocketAddr>() else {
        return ProbeResult {
            rtt_ms: None,
            reachable: false,
            path: "overlay",
        };
    };
    let start = Instant::now();
    match TcpStream::connect_timeout(&addr, PROBE_TIMEOUT) {
        // Connected — something listens on discard (rare). Timed.
        Ok(_) => ProbeResult {
            rtt_ms: Some(elapsed_ms(start)),
            reachable: true,
            path: "overlay",
        },
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => ProbeResult {
            // RST came back through the tunnel — reachable + timed.
            rtt_ms: Some(elapsed_ms(start)),
            reachable: true,
            path: "overlay",
        },
        // Timeout / unreachable / no route — the peer didn't answer.
        Err(_) => ProbeResult {
            rtt_ms: None,
            reachable: false,
            path: "overlay",
        },
    }
}

fn elapsed_ms(start: Instant) -> f64 {
    let us = start.elapsed().as_micros();
    #[allow(clippy::cast_precision_loss)]
    {
        us as f64 / 1000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refused_connect_is_a_successful_measurement() {
        // 127.0.0.1:9 — discard is closed on any dev box; the RST is
        // immediate, so this measures sub-millisecond "RTT".
        let r = probe_rtt("127.0.0.1");
        assert!(r.reachable, "refused = the stack answered");
        let rtt = r.rtt_ms.expect("refused still times the path");
        assert!(rtt < 1000.0, "loopback refusal is fast, got {rtt}ms");
    }

    #[test]
    fn unroutable_address_is_unreachable_not_a_panic() {
        // TEST-NET-1 (RFC 5737) — guaranteed unrouted.
        let r = probe_rtt("192.0.2.1");
        assert!(!r.reachable);
        assert!(r.rtt_ms.is_none());
    }

    #[test]
    fn garbage_input_degrades_honestly() {
        let r = probe_rtt("not-an-ip");
        assert!(!r.reachable);
    }
}
