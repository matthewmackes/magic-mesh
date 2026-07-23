//! NF-1.5 (v2.5) — server-side frame demux pump.
//!
//! Each peer's TCP/443 covert tunnel terminates at the
//! lighthouse as a single TLS 1.3 stream (NF-1.2). The demux
//! pump reads framed Nebula bytes from that stream (NF-1.3
//! `decode_frame`) + forwards them as UDP packets to the local
//! Nebula process at `nebula_addr` (default `127.0.0.1:4242`).
//! Return traffic flows the opposite way: Nebula's response
//! packets land on the per-stream UDP socket the demux opened,
//! get wrapped via `encode_frame`, and travel back through the
//! TLS stream.
//!
//! Per-stream UDP socket — important. The demux binds a fresh
//! ephemeral UDP socket for EACH accepted TLS stream so Nebula's
//! responses route back to the right tunnel. Nebula identifies
//! peers from the encrypted Nebula handshake inside the
//! payload, not the UDP source — so all forwarded packets
//! sourcing from 127.0.0.1:<ephemeral> stay correctly attributed
//! to the right peer inside Nebula's view of the world.
//!
//! Inner Nebula crypto layer runs unmodified. The demux only
//! shuttles bytes; it never parses or inspects Nebula payloads.
//!
//! Bench acceptance NF-9.4: a peer with UDP/4242 blocked still
//! reaches the lighthouse via the TCP/443 tunnel.

use std::net::SocketAddr;
use std::time::Duration;

use bytes::BytesMut;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;

use crate::framing::{decode_frame, encode_frame, FrameError, MAX_FRAME_SIZE};
use crate::tls::TunnelStream;

/// Read buffer size for the TLS side. One UDP packet's worth +
/// framing header; growing the buffer beyond MAX_FRAME_SIZE +
/// HEADER_LEN wastes memory.
const TLS_READ_CHUNK: usize = MAX_FRAME_SIZE + 32;

/// Default forwarding target — `127.0.0.1:4242` is where the
/// lighthouse's local Nebula process listens for inbound peer
/// traffic per the v2.5 fabric lock.
pub const DEFAULT_NEBULA_ADDR: &str = "127.0.0.1:4242";

/// Default per-stream UDP read timeout. If Nebula stops
/// responding for this long, the pump shuts down rather than
/// holding the TLS stream open indefinitely.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Demux errors. Each variant carries enough context for the
/// worker's tracing line to identify what broke without dumping
/// the whole error chain.
#[derive(Debug, thiserror::Error)]
pub enum DemuxError {
    /// Couldn't bind the per-stream UDP socket. Almost always
    /// "address already in use" when ephemeral ports run out.
    #[error("udp bind: {0}")]
    UdpBind(String),
    /// I/O failure on the TLS stream (peer hung up, read error).
    #[error("tls io: {0}")]
    TlsIo(String),
    /// I/O failure on the UDP socket (network error, Nebula
    /// process exited).
    #[error("udp io: {0}")]
    UdpIo(String),
    /// The TLS reader produced a framing error — peer sent a
    /// frame larger than `MAX_FRAME_SIZE`, which the protocol
    /// forbids. The pump drops the connection.
    #[error("framing: {0}")]
    Framing(#[from] FrameError),
    /// Idle timeout elapsed with neither side speaking.
    /// `IDLE_TIMEOUT` is the budget; the pump exits cleanly.
    #[error("idle timeout after {0:?}")]
    IdleTimeout(Duration),
}

/// Configurable demux options. The CLI / worker constructs this
/// once + clones per accepted stream so changes only need to
/// land in one place.
#[derive(Debug, Clone)]
pub struct DemuxConfig {
    /// Where to forward inbound frames. Default
    /// `127.0.0.1:4242`.
    pub nebula_addr: SocketAddr,
    /// Per-stream idle timeout. Default 120 s.
    pub idle_timeout: Duration,
}

impl Default for DemuxConfig {
    fn default() -> Self {
        Self {
            nebula_addr: DEFAULT_NEBULA_ADDR
                .parse()
                .expect("DEFAULT_NEBULA_ADDR is a valid SocketAddr"),
            idle_timeout: IDLE_TIMEOUT,
        }
    }
}

impl DemuxConfig {
    /// Build a config with a custom Nebula forwarding address —
    /// useful for tests + non-standard deployments where Nebula
    /// listens on something other than 4242.
    #[must_use]
    pub fn with_nebula_addr(mut self, addr: SocketAddr) -> Self {
        self.nebula_addr = addr;
        self
    }

    /// Override the idle timeout. Tests use small values to
    /// avoid 120 s waits.
    #[must_use]
    pub fn with_idle_timeout(mut self, t: Duration) -> Self {
        self.idle_timeout = t;
        self
    }
}

/// Run the bidirectional demux pump for one TLS stream until
/// either side errors or the idle timeout fires.
///
/// Returns `Ok(stats)` when the stream closed cleanly,
/// `Err(DemuxError)` on any other exit. The worker is expected
/// to log the error + drop the connection — there's no retry
/// path; the peer reconnects on its own activation-state
/// machine cycle (NF-1.4).
///
/// # Errors
///
/// Per [`DemuxError`]. The pump returns `IdleTimeout` as an
/// error variant rather than `Ok` so the worker's per-stream
/// log line can distinguish quiet shutdown from a stuck peer
/// (operator-visible signal).
pub async fn pump_one_stream(
    mut tls: TunnelStream,
    config: DemuxConfig,
) -> Result<DemuxStats, DemuxError> {
    // Bind a fresh ephemeral UDP socket. Source IP 127.0.0.1
    // so the forwarded packets land on the loopback path
    // Nebula's UDP listener sees. Port 0 = OS picks.
    let udp = UdpSocket::bind("127.0.0.1:0")
        .await
        .map_err(|e| DemuxError::UdpBind(e.to_string()))?;
    let local = udp.local_addr().ok();
    tracing::debug!(
        local = ?local,
        target = %config.nebula_addr,
        "nebula-https-demux: pump started",
    );

    let mut tls_read_buf = BytesMut::with_capacity(TLS_READ_CHUNK * 2);
    let mut udp_buf = vec![0u8; MAX_FRAME_SIZE];
    let mut tls_chunk = vec![0u8; TLS_READ_CHUNK];
    let mut frames_in: u64 = 0;
    let mut frames_out: u64 = 0;

    loop {
        tokio::select! {
            // Inbound: peer → TLS → unwrap → UDP → Nebula
            read = tokio::time::timeout(
                config.idle_timeout,
                tls.read(&mut tls_chunk),
            ) => {
                let read = read.map_err(|_| DemuxError::IdleTimeout(config.idle_timeout))?;
                let n = read.map_err(|e| DemuxError::TlsIo(e.to_string()))?;
                if n == 0 {
                    // Clean EOF — peer closed.
                    tracing::debug!(
                        frames_in,
                        frames_out,
                        "nebula-https-demux: peer closed TLS stream",
                    );
                    return Ok(DemuxStats { frames_in, frames_out });
                }
                tls_read_buf.extend_from_slice(&tls_chunk[..n]);
                // Drain every complete frame from the buffer.
                while let Some(frame) = decode_frame(&mut tls_read_buf)? {
                    udp.send_to(&frame, config.nebula_addr)
                        .await
                        .map_err(|e| DemuxError::UdpIo(e.to_string()))?;
                    frames_in += 1;
                }
            }

            // Outbound: Nebula → UDP → wrap → TLS → peer
            udp_read = tokio::time::timeout(
                config.idle_timeout,
                udp.recv_from(&mut udp_buf),
            ) => {
                let udp_read = udp_read.map_err(|_| DemuxError::IdleTimeout(config.idle_timeout))?;
                let (n, from) = udp_read.map_err(|e| DemuxError::UdpIo(e.to_string()))?;
                if from != config.nebula_addr {
                    tracing::warn!(
                        source = %from,
                        expected = %config.nebula_addr,
                        "nebula-https-demux: dropping untrusted local UDP source",
                    );
                    continue;
                }
                let payload = &udp_buf[..n];
                let mut out = BytesMut::new();
                encode_frame(payload, &mut out)?;
                tls.write_all(&out)
                    .await
                    .map_err(|e| DemuxError::TlsIo(e.to_string()))?;
                tls.flush()
                    .await
                    .map_err(|e| DemuxError::TlsIo(e.to_string()))?;
                frames_out += 1;
            }
        }
    }
}

/// Per-stream stats returned by [`pump_one_stream`] on clean
/// exit. The worker logs these at info level for operator
/// visibility ("peer X transferred N frames before
/// disconnecting").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DemuxStats {
    /// Frames received over TLS + forwarded as UDP.
    pub frames_in: u64,
    /// Frames received over UDP + forwarded over TLS.
    pub frames_out: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_targets_nebula_localhost_4242() {
        let cfg = DemuxConfig::default();
        assert_eq!(cfg.nebula_addr.to_string(), "127.0.0.1:4242");
        assert_eq!(cfg.idle_timeout, IDLE_TIMEOUT);
    }

    #[test]
    fn with_nebula_addr_replaces_default() {
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let cfg = DemuxConfig::default().with_nebula_addr(addr);
        assert_eq!(cfg.nebula_addr, addr);
    }

    #[test]
    fn with_idle_timeout_overrides_default() {
        let cfg = DemuxConfig::default().with_idle_timeout(Duration::from_secs(5));
        assert_eq!(cfg.idle_timeout, Duration::from_secs(5));
    }

    #[test]
    fn demux_error_display_includes_subsystem_label() {
        let e = DemuxError::UdpBind("address in use".to_string());
        let s = format!("{e}");
        assert!(s.contains("udp bind"));
        assert!(s.contains("address in use"));
        let e = DemuxError::TlsIo("broken pipe".to_string());
        let s = format!("{e}");
        assert!(s.contains("tls io"));
        let e = DemuxError::IdleTimeout(Duration::from_secs(120));
        let s = format!("{e}");
        assert!(s.contains("idle timeout"));
        assert!(s.contains("120"));
    }

    #[test]
    fn demux_error_carries_framing_oversized() {
        let e: DemuxError = FrameError::Oversized(2000).into();
        assert!(matches!(
            e,
            DemuxError::Framing(FrameError::Oversized(2000))
        ));
    }

    #[test]
    fn demux_stats_default_is_zero() {
        let s = DemuxStats::default();
        assert_eq!(s.frames_in, 0);
        assert_eq!(s.frames_out, 0);
    }

    // Integration-style: simulate the demux's inbound path
    // (TLS → UDP) using a Duplex pair to stand in for the TLS
    // stream + a real UdpSocket as the Nebula stand-in. Proves
    // the framing-aware reader correctly drains multiple
    // back-to-back frames from the TLS side and forwards each
    // as a discrete UDP datagram.
    //
    // The bidirectional flow is exercised end-to-end by NF-9.4
    // bench acceptance (hardware bench testing). This module
    // test stays focused on the pure forwarding semantics that
    // can run offline.
    #[tokio::test]
    async fn inbound_frames_forward_as_discrete_udp_datagrams() {
        use bytes::BytesMut;
        // Mock Nebula: a UdpSocket on localhost we recv_from.
        let nebula = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("bind mock nebula");
        let nebula_addr = nebula.local_addr().unwrap();

        // Build two encoded frames + simulate them arriving on
        // a TLS-equivalent byte stream. We don't need a real
        // TLS handshake to exercise the framing decoder + UDP
        // forwarder — they're the part of the pump that has
        // logic; the actual TLS read is std::io.
        let mut wire = BytesMut::new();
        encode_frame(b"first nebula packet", &mut wire).unwrap();
        encode_frame(b"second nebula packet", &mut wire).unwrap();

        // Manually run the inbound path: decode + send via a
        // freshly-bound UDP socket (mirrors what pump_one_stream
        // does on the TLS-readable branch).
        let udp = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("bind demux udp");
        let mut buf = wire.clone();
        while let Some(frame) = decode_frame(&mut buf).expect("decode") {
            udp.send_to(&frame, nebula_addr).await.expect("send");
        }

        // Mock Nebula reads two discrete datagrams.
        let mut rx = vec![0u8; 256];
        let (n1, _) = nebula.recv_from(&mut rx).await.expect("recv first");
        assert_eq!(&rx[..n1], b"first nebula packet");
        let (n2, _) = nebula.recv_from(&mut rx).await.expect("recv second");
        assert_eq!(&rx[..n2], b"second nebula packet");
    }

    #[tokio::test]
    async fn outbound_udp_payloads_wrap_into_frames() {
        use bytes::BytesMut;
        // Mirror the outbound branch: a UdpSocket sends a
        // payload, the demux receives it + encodes a frame, the
        // TLS-equivalent buffer ends up with the framed bytes.
        let demux = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("bind demux");
        let demux_addr = demux.local_addr().unwrap();
        let nebula = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("bind nebula");
        nebula
            .send_to(b"return packet", demux_addr)
            .await
            .expect("send");

        let mut udp_buf = vec![0u8; MAX_FRAME_SIZE];
        let (n, _from) = demux.recv_from(&mut udp_buf).await.expect("recv");
        let payload = &udp_buf[..n];
        let mut tls_wire = BytesMut::new();
        encode_frame(payload, &mut tls_wire).expect("encode");

        // The peer reads + decodes the frame back to the
        // original payload.
        let decoded = decode_frame(&mut tls_wire)
            .expect("decode")
            .expect("complete frame");
        assert_eq!(&decoded[..], b"return packet");
    }
}
