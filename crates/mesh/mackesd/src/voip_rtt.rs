//! VOIP-4 (v5.0.0) — Vitelity-link RTT telemetry.
//!
//! Measures the network round-trip to the Vitelity SIP edge
//! (`out.vitelity.net:5061`, per `mde-voice-config`'s `uacreg` row
//! `sip:out.vitelity.net:5061;transport=tls`) and publishes it to
//! `voip/link-rtt/<peer>` on the Mackes Bus, so the dialer (VOIP-4.b)
//! can compare links across peers and offer an operator-explicit
//! "place via `<peer>`" route override (auto-routing stays off).
//!
//! The measurement is a TCP-connect RTT to the TLS edge — a tractable
//! proxy for the SIP registration RTT; the literal REGISTER round-trip
//! is a future refinement once the PJSIP layer lands. This module is the
//! VOIP-4.a primitive (measure + publish), exercised by the
//! `mackesd voip-rtt` CLI; the 60 s broadcast worker is VOIP-4.b.

use std::net::{TcpStream, ToSocketAddrs};
use std::process::Command;
use std::time::{Duration, Instant};

/// The Vitelity outbound SIP edge host (TLS).
pub const VITELITY_PROXY_HOST: &str = "out.vitelity.net";
/// The Vitelity TLS SIP port.
pub const VITELITY_PROXY_PORT: u16 = 5061;
/// Default per-measurement connect timeout (ms).
pub const RTT_TIMEOUT_MS: u64 = 3000;

/// One link-RTT sample, published to `voip/link-rtt/<peer>`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LinkRtt {
    /// This peer's Nebula overlay IP (the topic suffix).
    pub peer: String,
    /// Measured RTT in milliseconds; `None` when the edge was unreachable.
    pub rtt_ms: Option<u64>,
    /// Unix-epoch ms the sample was taken.
    pub ts_ms: i64,
}

/// The Bus topic for a peer's link-RTT samples.
#[must_use]
pub fn rtt_topic(peer: &str) -> String {
    format!("voip/link-rtt/{peer}")
}

/// Measure the TCP-connect RTT to `host:port`, capped at `timeout_ms`.
///
/// Returns the elapsed milliseconds, or `None` on resolution failure,
/// connection refusal, or timeout. The only side effect is the socket
/// connect attempt (no data is sent).
#[must_use]
pub fn measure_tcp_rtt(host: &str, port: u16, timeout_ms: u64) -> Option<u64> {
    let timeout = Duration::from_millis(timeout_ms);
    let addrs = (host, port).to_socket_addrs().ok()?;
    for addr in addrs {
        let start = Instant::now();
        if TcpStream::connect_timeout(&addr, timeout).is_ok() {
            return Some(u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX));
        }
    }
    None
}

/// Parse `ip -4 addr show nebula1` for this peer's overlay IP. Pure.
#[must_use]
pub fn parse_nebula_ip(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        if let Some(rest) = line.trim().strip_prefix("inet ") {
            // e.g. "inet 10.42.0.5/17 scope global nebula1"
            let cidr = rest.split_whitespace().next()?;
            return Some(cidr.split('/').next()?.to_string());
        }
    }
    None
}

/// This peer's Nebula overlay IP (via `ip -4 addr show nebula1`).
/// `None` when the interface is absent (pre-enrollment).
#[must_use]
pub fn own_nebula_ip() -> Option<String> {
    let out = Command::new("ip")
        .args(["-4", "addr", "show", "nebula1"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_nebula_ip(&String::from_utf8_lossy(&out.stdout))
}

/// Measure the Vitelity-edge RTT + assemble the [`LinkRtt`] for `peer`.
#[must_use]
pub fn sample_link_rtt(peer: &str) -> LinkRtt {
    LinkRtt {
        peer: peer.to_string(),
        rtt_ms: measure_tcp_rtt(VITELITY_PROXY_HOST, VITELITY_PROXY_PORT, RTT_TIMEOUT_MS),
        ts_ms: chrono::Local::now().timestamp_millis(),
    }
}

/// Publish a [`LinkRtt`] to `voip/link-rtt/<peer>` via the `mde-bus` CLI
/// (the same path `compute_registry::publish_inventory` uses).
/// Fire-and-forget; a no-op on a peer with no overlay IP.
pub fn publish_link_rtt(sample: &LinkRtt) {
    if sample.peer.is_empty() {
        return;
    }
    let Ok(body) = serde_json::to_string(sample) else {
        return;
    };
    let topic = rtt_topic(&sample.peer);
    let mut cmd = Command::new("mde-bus");
    cmd.args(["publish", &topic, "--body-flag", &body]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// Measure this peer's Vitelity-link RTT + publish it to
/// `voip/link-rtt/<peer>` (a no-op without a Nebula overlay IP). The
/// convenience glue the VOIP-4.b broadcast worker ticks every 60 s.
pub fn sample_and_publish() {
    if let Some(peer) = own_nebula_ip() {
        publish_link_rtt(&sample_link_rtt(&peer));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtt_topic_format() {
        assert_eq!(rtt_topic("10.42.0.5"), "voip/link-rtt/10.42.0.5");
    }

    #[test]
    fn link_rtt_serializes_round_trip() {
        let s = LinkRtt {
            peer: "10.42.0.5".into(),
            rtt_ms: Some(42),
            ts_ms: 1_700_000_000_000,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"rtt_ms\":42"));
        let back: LinkRtt = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
        // Unreachable sample serializes rtt_ms as null.
        let down = LinkRtt {
            peer: "p".into(),
            rtt_ms: None,
            ts_ms: 0,
        };
        assert!(serde_json::to_string(&down)
            .unwrap()
            .contains("\"rtt_ms\":null"));
    }

    #[test]
    fn parse_nebula_ip_extracts_overlay_addr() {
        let out = "5: nebula1: <POINTOPOINT,MULTICAST,NOARP,UP,LOWER_UP> mtu 1300\n    \
                   inet 10.42.0.5/17 scope global nebula1\n       valid_lft forever\n";
        assert_eq!(parse_nebula_ip(out).as_deref(), Some("10.42.0.5"));
        assert_eq!(parse_nebula_ip("no inet here").as_deref(), None);
    }

    #[test]
    fn measure_tcp_rtt_none_on_refused() {
        // Port 1 on loopback refuses fast → None (no external network).
        assert_eq!(measure_tcp_rtt("127.0.0.1", 1, 500), None);
    }
}
