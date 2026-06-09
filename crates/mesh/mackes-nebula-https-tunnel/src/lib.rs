//! NF-1.1 — `mackes-nebula-https-tunnel` crate root.
//!
//! Implements the v2.5 Nebula Fabric covert TCP/443 transport:
//! every Nebula UDP frame is wrapped in a 4-byte length-prefixed
//! payload over a single long-lived rustls TLS 1.3 stream. Locked
//! per the design doc at `docs/design/v2.5-nebula-fabric.md`,
//! section "TCP/443 covert transport — Q4 follow-through."
//!
//! ## Module layout
//!
//! | Module | NF task | Surface |
//! |--------|---------|---------|
//! | [`framing`] | NF-1.3 | `encode_frame` / `decode_frame` + `FrameError` |
//! | [`activation`] | NF-1.4 | `HttpsFallbackState` state machine + `FailureWindow` |
//! | [`tls`] | NF-1.2 | `listen` / `dial` + `TunnelListener` / `TunnelStream` |
//!
//! ## How the pieces fit
//!
//! 1. The connectivity worker observes probe-pair outcomes
//!    and drives `activation::transition(state, &mut window,
//!    input)` on its existing tick cadence. When the state
//!    flips to `Activating`, the worker:
//! 2. Calls `tls::dial(lighthouse_addr, sni, ca_bundle)` to
//!    open the covert socket.
//! 3. On success, calls `activation::transition(_, _,
//!    HandshakeOk)` to enter `Active`. The router then routes
//!    `MessageClass::Control` and `Interactive` traffic
//!    through `framing::encode_frame` on the active stream.
//! 4. On `TunnelLost` or `Probe(AnyUdpSucceeded)`, the worker
//!    flips back to `Failing` or `Inactive` and the router
//!    falls back to the upstream UDP path.
//!
//! ## Server side (NF-1.5)
//!
//! The lighthouse process accepts both `:4242/udp` (native
//! Nebula) and `:443/tcp` (this crate's `listen`). Frame demux
//! happens before the inner Nebula stack sees the packet, so
//! the inner crypto layer is unmodified. NF-1.5 ships
//! [`demux::pump_one_stream`] (the per-stream pure pump) +
//! [`demux::DemuxConfig`] (forward-address + idle-timeout
//! knobs). The mackesd-side worker that runs `tls::listen` +
//! spawns `pump_one_stream` per accepted stream lives in
//! `crates/mackesd/src/workers/nebula_https_listener.rs`.
//!
//! ## Throughput floor (NF-1.6)
//!
//! Bench test `tests/acceptance/test_nebula_fabric.py` runs
//! the localhost throughput test and asserts ≥ 5 Mbps on
//! x86_64 Fedora 44 CI per the Q10 covert-path floor.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod activation;
pub mod demux;
pub mod framing;
pub mod tls;

// Re-exports keep consumer call sites compact:
//   use mackes_nebula_https_tunnel::{
//       HttpsFallbackState, TransitionInput, encode_frame, decode_frame,
//       pump_one_stream, DemuxConfig,
//   };
pub use activation::{
    transition, FailureWindow, HttpsFallbackState, ProbePairOutcome, TransitionInput,
    FAILURE_THRESHOLD,
};
pub use demux::{
    pump_one_stream, DemuxConfig, DemuxError, DemuxStats, DEFAULT_NEBULA_ADDR, IDLE_TIMEOUT,
};
pub use framing::{decode_frame, encode_frame, FrameError, HEADER_LEN, MAX_FRAME_SIZE};
pub use tls::{dial, listen, TunnelClientStream, TunnelError, TunnelListener, TunnelStream};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn re_exports_match_module_paths() {
        // Compile-time check that the lib-root re-exports
        // resolve to the same types. If a future refactor
        // moves a type out of one of the modules without
        // updating the re-export, this test won't compile.
        let _: HttpsFallbackState = activation::HttpsFallbackState::Inactive;
        let _: FailureWindow = activation::FailureWindow::new();
        assert_eq!(FAILURE_THRESHOLD, activation::FAILURE_THRESHOLD);
        assert_eq!(MAX_FRAME_SIZE, framing::MAX_FRAME_SIZE);
        assert_eq!(HEADER_LEN, framing::HEADER_LEN);
    }
}
