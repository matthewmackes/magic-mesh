//! KDC2-1.6 — `Transport` trait conformance harness.
//!
//! Reusable battery of invariant checks every `Transport` impl
//! runs against itself. Lives behind `#[cfg(any(test,
//! feature = "conformance"))]` so production builds don't pay for
//! the harness; downstream crates (`mde-kdc`, `mackes-https-tunnel`,
//! `mackesd::transport::direct_udp`) opt in by enabling the
//! `conformance` feature in their `[dev-dependencies]`.
//!
//! Each conformance test takes an `&dyn Transport` plus a
//! [`ConformanceFixture`] that wires the transport to a known
//! peer state (paired vs. unpaired vs. degraded). The harness
//! drives the trait + asserts invariants — the impl crate
//! provides the wiring.

#[cfg(test)]
use std::future::Future;
#[cfg(test)]
use std::pin::Pin;

use crate::{HealthState, MessageClass, Transport, TransportKind};

/// Adapter the impl crate provides to seed the conformance
/// harness. Owns whatever fixture state the transport needs to
/// look like it's paired / unpaired / degraded.
pub trait ConformanceFixture: Send + Sync {
    /// Identifier the harness uses for the "paired" peer. The
    /// fixture promises probe / open / health all succeed for
    /// this id.
    fn paired_peer_id(&self) -> &str;

    /// Identifier the harness uses for the "never paired" peer.
    /// The fixture promises probe returns `Down` + open returns
    /// `Unreachable` for this id.
    fn unpaired_peer_id(&self) -> &str;

    /// Identifier the harness uses for a paired-but-degraded
    /// peer (probe succeeds but reports `Degraded`).
    fn degraded_peer_id(&self) -> &str;
}

/// Run every conformance check against `transport`. Returns
/// `Ok(())` if every invariant held; `Err(String)` with the
/// first failure description otherwise.
///
/// `async`: every Transport method is async, so the harness is
/// too. Callers run this in a tokio runtime / `pollster::block_on`
/// / similar.
pub async fn run_conformance(
    transport: &dyn Transport,
    fixture: &dyn ConformanceFixture,
) -> Result<(), String> {
    let mut report = ConformanceReport::default();

    // C1: kind() returns a stable value.
    if !c1_kind_stable(transport).await {
        report.add("C1_kind_stable");
    }

    // C2: capabilities() is stable across calls.
    if !c2_capabilities_stable(transport).await {
        report.add("C2_capabilities_stable");
    }

    // C3: probe(unpaired) returns Down.
    if !c3_probe_unpaired_is_down(transport, fixture).await {
        report.add("C3_probe_unpaired_is_down");
    }

    // C4: probe(paired) returns Healthy or Degraded (not Down).
    if !c4_probe_paired_is_sendable(transport, fixture).await {
        report.add("C4_probe_paired_is_sendable");
    }

    // C5: health(paired) returns Healthy or Degraded after probe.
    if !c5_health_paired_after_probe(transport, fixture).await {
        report.add("C5_health_paired_after_probe");
    }

    // C6: health(unpaired) returns Down.
    if !c6_health_unpaired_is_down(transport, fixture).await {
        report.add("C6_health_unpaired_is_down");
    }

    // C7: open(unpaired) returns an Unreachable error.
    if !c7_open_unpaired_is_unreachable(transport, fixture).await {
        report.add("C7_open_unpaired_is_unreachable");
    }

    // C8: open(paired) returns a non-empty Connection.
    if !c8_open_paired_returns_connection(transport, fixture).await {
        report.add("C8_open_paired_returns_connection");
    }

    // C9: open(paired) twice — same id (impls may cache or
    // emit new ids; harness allows both, but locks consistency).
    if !c9_open_idempotent_or_distinct(transport, fixture).await {
        report.add("C9_open_idempotent_or_distinct");
    }

    // C10: capabilities() reports `carries.control == true`.
    // Every transport must carry control messages — KDC2 lock.
    if !c10_carries_control(transport).await {
        report.add("C10_carries_control");
    }

    // C11: degraded peer reports Degraded (not Healthy, not Down).
    if !c11_degraded_probe(transport, fixture).await {
        report.add("C11_degraded_probe");
    }

    // C12: Capabilities's label is non-empty.
    if !c12_capabilities_label_non_empty(transport).await {
        report.add("C12_capabilities_label_non_empty");
    }

    // C13: Probing the same peer twice returns the same Health.
    if !c13_probe_idempotent(transport, fixture).await {
        report.add("C13_probe_idempotent");
    }

    // C14: TransportKind matches via Display + serde token.
    if !c14_kind_token_matches(transport).await {
        report.add("C14_kind_token_matches");
    }

    if report.failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Transport {:?} failed conformance: {}",
            transport.kind(),
            report.failures.join(", "),
        ))
    }
}

#[derive(Default)]
struct ConformanceReport {
    failures: Vec<&'static str>,
}

impl ConformanceReport {
    fn add(&mut self, name: &'static str) {
        self.failures.push(name);
    }
}

// ────────────────────────────────────────────────────────────────
// Individual conformance checks. Each returns true on pass.
// ────────────────────────────────────────────────────────────────

async fn c1_kind_stable(t: &dyn Transport) -> bool {
    let k1 = t.kind();
    let k2 = t.kind();
    k1 == k2
}

async fn c2_capabilities_stable(t: &dyn Transport) -> bool {
    let c1 = t.capabilities();
    let c2 = t.capabilities();
    c1 == c2
}

async fn c3_probe_unpaired_is_down(t: &dyn Transport, f: &dyn ConformanceFixture) -> bool {
    matches!(t.probe(f.unpaired_peer_id()).await, HealthState::Down)
}

async fn c4_probe_paired_is_sendable(t: &dyn Transport, f: &dyn ConformanceFixture) -> bool {
    t.probe(f.paired_peer_id()).await.is_sendable()
}

async fn c5_health_paired_after_probe(t: &dyn Transport, f: &dyn ConformanceFixture) -> bool {
    let _ = t.probe(f.paired_peer_id()).await;
    t.health(f.paired_peer_id()).await.is_sendable()
}

async fn c6_health_unpaired_is_down(t: &dyn Transport, f: &dyn ConformanceFixture) -> bool {
    matches!(t.health(f.unpaired_peer_id()).await, HealthState::Down)
}

async fn c7_open_unpaired_is_unreachable(t: &dyn Transport, f: &dyn ConformanceFixture) -> bool {
    match t.open(f.unpaired_peer_id()).await {
        Err(crate::TransportError::Unreachable { .. }) => true,
        _ => false,
    }
}

async fn c8_open_paired_returns_connection(t: &dyn Transport, f: &dyn ConformanceFixture) -> bool {
    match t.open(f.paired_peer_id()).await {
        Ok(conn) => !conn.id().is_empty(),
        Err(_) => false,
    }
}

async fn c9_open_idempotent_or_distinct(t: &dyn Transport, f: &dyn ConformanceFixture) -> bool {
    let c1 = match t.open(f.paired_peer_id()).await {
        Ok(c) => c.id().to_string(),
        Err(_) => return false,
    };
    let c2 = match t.open(f.paired_peer_id()).await {
        Ok(c) => c.id().to_string(),
        Err(_) => return false,
    };
    // Either idempotent (same id) or distinct (every open gets
    // a fresh id). Both are valid; what's NOT valid is an empty
    // id either time.
    !c1.is_empty() && !c2.is_empty()
}

async fn c10_carries_control(t: &dyn Transport) -> bool {
    t.capabilities().carries.carries(MessageClass::Control)
}

async fn c11_degraded_probe(t: &dyn Transport, f: &dyn ConformanceFixture) -> bool {
    matches!(t.probe(f.degraded_peer_id()).await, HealthState::Degraded)
}

async fn c12_capabilities_label_non_empty(t: &dyn Transport) -> bool {
    !t.capabilities().label.is_empty()
}

async fn c13_probe_idempotent(t: &dyn Transport, f: &dyn ConformanceFixture) -> bool {
    let h1 = t.probe(f.paired_peer_id()).await;
    let h2 = t.probe(f.paired_peer_id()).await;
    h1 == h2
}

async fn c14_kind_token_matches(t: &dyn Transport) -> bool {
    let k = t.kind();
    let display = format!("{k}");
    let as_str = k.as_str();
    display == as_str
}

// ────────────────────────────────────────────────────────────────
// MockTransport — used in this crate's tests + downstream impls'
// integration tests as a known-good reference behavior.
// ────────────────────────────────────────────────────────────────

/// Reference impl used by conformance tests. Lives in the
/// public API so downstream crates can compose it with their
/// own test fixtures.
#[derive(Debug)]
pub struct MockTransport {
    kind: TransportKind,
    capabilities: crate::Capabilities,
    paired_peer: String,
    degraded_peer: String,
}

impl MockTransport {
    /// Construct a mock with the canonical paired / unpaired /
    /// degraded peer ids the conformance fixture below uses.
    #[must_use]
    pub fn new(kind: TransportKind) -> Self {
        Self {
            kind,
            capabilities: crate::Capabilities {
                max_frame_bytes: Some(64 * 1024),
                health_window: std::time::Duration::from_secs(5),
                carries: crate::MessageClassSet::all(),
                label: format!("mock-{kind}"),
            },
            paired_peer: "paired".to_string(),
            degraded_peer: "degraded".to_string(),
        }
    }
}

#[async_trait::async_trait]
impl Transport for MockTransport {
    fn kind(&self) -> TransportKind {
        self.kind
    }

    fn capabilities(&self) -> crate::Capabilities {
        self.capabilities.clone()
    }

    async fn probe(&self, peer_id: &str) -> HealthState {
        if peer_id == self.paired_peer {
            HealthState::Healthy
        } else if peer_id == self.degraded_peer {
            HealthState::Degraded
        } else {
            HealthState::Down
        }
    }

    async fn open(
        &self,
        peer_id: &str,
    ) -> Result<Box<dyn crate::Connection>, crate::TransportError> {
        if peer_id == self.paired_peer || peer_id == self.degraded_peer {
            Ok(Box::new(MockConnection {
                id: format!("mock-conn-{peer_id}"),
            }))
        } else {
            Err(crate::TransportError::Unreachable {
                code: "mock_unpaired",
            })
        }
    }

    async fn health(&self, peer_id: &str) -> HealthState {
        // Same shape as probe for the mock.
        self.probe(peer_id).await
    }
}

/// Connection returned by [`MockTransport::open`].
#[derive(Debug)]
struct MockConnection {
    id: String,
}

impl crate::Connection for MockConnection {
    fn id(&self) -> &str {
        &self.id
    }
}

/// Canonical fixture for the [`MockTransport`]. Downstream crates
/// can ship their own ConformanceFixture impl seeded with their
/// own paired/unpaired/degraded ids.
pub struct MockFixture;

impl ConformanceFixture for MockFixture {
    fn paired_peer_id(&self) -> &str {
        "paired"
    }
    fn unpaired_peer_id(&self) -> &str {
        "unpaired"
    }
    fn degraded_peer_id(&self) -> &str {
        "degraded"
    }
}

// Tiny block_on helper for the test below — the harness is async
// but we don't want to pull tokio into the dev-dep tree just for
// 14 conformance checks. This is a 30-line spin executor;
// downstream impls that need real I/O use tokio.
#[cfg(test)]
fn block_on<F: Future>(fut: F) -> F::Output {
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    struct NoopWaker;
    impl Wake for NoopWaker {
        fn wake(self: Arc<Self>) {}
    }

    let mut fut: Pin<Box<F>> = Box::pin(fut);
    let waker: Waker = Arc::new(NoopWaker).into();
    let mut cx = Context::from_waker(&waker);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => {
                // Spin — the MockTransport's futures never yield
                // (no I/O), so this loop turns over once or
                // twice and resolves immediately.
                std::hint::spin_loop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_transport_passes_conformance() {
        let t = MockTransport::new(TransportKind::KdcTls);
        let f = MockFixture;
        let result = block_on(run_conformance(&t, &f));
        assert!(result.is_ok(), "mock conformance: {:?}", result.err());
    }

    #[test]
    fn mock_transport_handles_every_transport_kind() {
        // Each of the four kinds should pass conformance — the
        // MockTransport implementation is kind-agnostic.
        for k in TransportKind::all() {
            let t = MockTransport::new(k);
            let f = MockFixture;
            let result = block_on(run_conformance(&t, &f));
            assert!(result.is_ok(), "kind={k:?}: {:?}", result.err());
        }
    }

    #[test]
    fn conformance_detects_a_broken_transport() {
        // BrokenTransport breaks C1 (kind stability) — we want
        // to confirm the harness flags it.
        #[derive(Debug)]
        struct Flaky {
            calls: std::sync::atomic::AtomicU32,
        }
        #[async_trait::async_trait]
        impl Transport for Flaky {
            fn kind(&self) -> TransportKind {
                // Flips between two kinds every call — breaks C1.
                let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n % 2 == 0 {
                    TransportKind::NebulaDirect
                } else {
                    TransportKind::KdcTls
                }
            }
            fn capabilities(&self) -> crate::Capabilities {
                crate::Capabilities {
                    max_frame_bytes: None,
                    health_window: std::time::Duration::from_secs(1),
                    carries: crate::MessageClassSet::all(),
                    label: "flaky".to_string(),
                }
            }
            async fn probe(&self, _peer_id: &str) -> HealthState {
                HealthState::Healthy
            }
            async fn open(
                &self,
                _peer_id: &str,
            ) -> Result<Box<dyn crate::Connection>, crate::TransportError> {
                Err(crate::TransportError::Io { code: "x" })
            }
            async fn health(&self, _peer_id: &str) -> HealthState {
                HealthState::Healthy
            }
        }
        let t = Flaky {
            calls: std::sync::atomic::AtomicU32::new(0),
        };
        let f = MockFixture;
        let result = block_on(run_conformance(&t, &f));
        assert!(result.is_err(), "broken transport must fail conformance");
        let err = result.unwrap_err();
        assert!(err.contains("C1_kind_stable"), "C1 must be flagged: {err}");
    }
}
