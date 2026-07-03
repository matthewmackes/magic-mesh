//! Headless proof of the SPICE transport's connect path (`connect.rs`).
//!
//! The unit suite proves the decode→egui and egui→input surfaces on synthetic
//! bytes, and [`SpiceSession::apply_surface`](mde_vdi_spice::SpiceSession) proves
//! "a decoded surface arrives → a frame is available" against a synthetic
//! (fake-source) surface. This integration test proves the *connect* half of the
//! seam runs end-to-end and surfaces failure as a typed error rather than a hang:
//! [`SpiceTransport::connect`] and the sync [`BlockingSpiceTransport`] facade are
//! driven against a closed loopback port, exercising the real `spice-client`
//! connect path (build the client → resolve → attempt TCP) headlessly.
//!
//! The full connect→frame→input round-trip against a real console needs a live
//! SPICE server (a QEMU/KVM guest), so it is the env-gated `tests/live_spice.rs`
//! proof — this file keeps CI honest without one.
//!
//! `spice-client`'s bundled `MockSpiceServer` is deliberately *not* used: its
//! handshake is a minimal stub for the crate's own per-channel message tests and
//! does not complete a full client link, so a `connect()` against it only times
//! out — it would prove nothing.

use std::time::Duration;

use mde_vdi_spice::{BlockingSpiceTransport, SpiceConfig, SpiceTransport};

/// A closed loopback endpoint: connect must surface a typed error promptly, never
/// hang. Port 1 is reserved and unused on loopback.
fn closed_target() -> SpiceConfig {
    SpiceConfig::new("127.0.0.1").with_port(1)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn async_connect_to_a_closed_port_errors_and_does_not_hang() {
    let res = tokio::time::timeout(
        Duration::from_secs(5),
        SpiceTransport::connect(&closed_target()),
    )
    .await;
    let inner = res.expect(
        "connect hung on a closed port — the transport must time out to a typed error, not block",
    );
    assert!(
        inner.is_err(),
        "connect to a closed loopback port should return a typed transport error"
    );
}

#[test]
fn blocking_transport_builds_a_runtime_and_surfaces_the_connect_error() {
    // The sync-shell facade builds its own current-thread runtime and drives the
    // same connect; a closed port must come back as a typed error, proving the
    // runtime + connect wiring runs without an ambient async context.
    let res = BlockingSpiceTransport::connect(&closed_target());
    assert!(
        res.is_err(),
        "blocking connect to a closed loopback port should return a typed transport error"
    );
}
