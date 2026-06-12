//! KDC2-1.8 — mesh-router worker.
//!
//! Long-running worker that holds the live per-peer routing
//! state + a registry of transport impls. On every tick it:
//!
//!   1. Walks every known peer.
//!   2. Probes each transport (cheap per-probe call).
//!   3. Updates the peer's [`PeerPath`] health + considers a
//!      transport switch.
//!   4. Emits a `PathSwitch` audit-chain entry whenever the
//!      primary transport flips (with the [`SwitchReason`]).
//!
//! Concrete scoring + transport selection (KDC2-1.9) +
//! audit-chain feed (KDC2-1.12) land as follow-ups. This commit
//! ships the worker scaffold: trait impl, tick loop, registry,
//! state-map.

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use mackes_transport::peer_path::{PeerPath, SwitchReason};
use mackes_transport::{Transport, TransportKind};
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::metrics::Histogram;
use crate::transport::audit::PathSwitchEvent;

/// Shared handle to the `kdc2_router_decision_us` histogram —
/// the textfile-flush worker reads from this when assembling
/// the `.prom` snapshot.
pub type RouterMetrics = Arc<StdMutex<Histogram>>;

use super::{ShutdownToken, Worker};

/// Default tick cadence for the router. Matches the v12
/// connectivity-scope lock's "10s roaming switch budget" — the
/// router probes once per tick so a transport degradation gets
/// noticed within one cadence interval.
const DEFAULT_TICK: Duration = Duration::from_secs(10);

/// Identifier for one peer in the mesh.
pub type PeerId = String;

/// Per-peer routing state map. Behind a `tokio::sync::RwLock` so
/// the supervisor's tick task + any future API readers (zbus
/// `dev.mackes.MDE.Mesh.PathFor()`) can share access.
pub type RouterState = Arc<RwLock<HashMap<PeerId, PeerPath>>>;

/// Registered transport implementations. `Vec<Arc<dyn Transport>>`
/// so the worker can hold multiple references (clone the Arc into
/// the tick loop) without giving up ownership of the slice.
pub type TransportRegistry = Arc<Vec<Arc<dyn Transport>>>;

/// Async worker that ticks the mesh router on a fixed cadence.
///
/// State + registry are passed in at construction so the
/// supervisor's restart logic can hand the same router state
/// back after a worker restart — losing the in-memory PeerPath
/// table on every restart would defeat the whole point of
/// tracking health history.
pub struct MeshRouterWorker {
    state: RouterState,
    registry: TransportRegistry,
    tick: Duration,
    /// KDC2-1.12.b — optional handle to the
    /// `kdc2_router_decision_us` histogram. When `Some`, every
    /// `tick_once` records its decision microseconds via
    /// `Histogram::observe`. `None` for tests + bootstrap paths
    /// that don't care about telemetry.
    metrics: Option<RouterMetrics>,
}

impl MeshRouterWorker {
    /// Construct a new mesh-router worker with the default
    /// 10s tick cadence.
    #[must_use]
    pub fn new(state: RouterState, registry: TransportRegistry) -> Self {
        Self {
            state,
            registry,
            tick: DEFAULT_TICK,
            metrics: None,
        }
    }

    /// Override the tick cadence. Useful for tests (set to
    /// 100 ms) and the future operator-tunable
    /// `/etc/mde/connect/policy.toml` (KDC2-1.10).
    #[must_use]
    pub fn with_tick(mut self, tick: Duration) -> Self {
        self.tick = tick;
        self
    }

    /// KDC2-1.12.b — attach a shared
    /// `kdc2_router_decision_us` histogram. Subsequent ticks
    /// observe their decision microseconds into the supplied
    /// handle.
    #[must_use]
    pub fn with_metrics(mut self, metrics: RouterMetrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Total number of registered transports. Used by tests +
    /// `mackesd healthz` to confirm the worker has the expected
    /// transport set wired.
    #[must_use]
    pub fn transport_count(&self) -> usize {
        self.registry.len()
    }

    /// Total number of peers currently tracked. Cheap async
    /// read; exposed for instrumentation.
    pub async fn peer_count(&self) -> usize {
        self.state.read().await.len()
    }
}

#[async_trait::async_trait]
impl Worker for MeshRouterWorker {
    fn name(&self) -> &'static str {
        "mesh-router"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        info!(
            transport_count = self.transport_count(),
            tick_ms = self.tick.as_millis() as u64,
            "mesh-router: starting",
        );

        let mut interval = tokio::time::interval(self.tick);
        // First tick fires immediately; skip it so the first
        // observation isn't done before any transport has had a
        // chance to settle after worker startup.
        interval.tick().await;

        loop {
            tokio::select! {
                _ = shutdown.wait() => {
                    info!("mesh-router: shutdown requested; exiting");
                    return Ok(());
                }
                _ = interval.tick() => {
                    self.tick_once().await;
                }
            }
        }
    }
}

impl MeshRouterWorker {
    /// One iteration of the router's main loop. Pure-async — no
    /// shared mutable state outside the locked `state` map.
    ///
    /// KDC2-1.8 scaffolds the loop; KDC2-1.12 wires the audit-
    /// chain emission seam. The actual scorer call is folded
    /// into [`Self::scored_primary_for`] so unit tests can
    /// exercise the path-switch detection logic without running
    /// the full tick loop.
    ///
    /// Phase 12.18 D.3 (2026-05-23) — `tick_once` now drives the
    /// HTTPS-fallback activation slice: for every peer in the
    /// `Activating` state, locate the `NebulaHttps443` transport in
    /// the registry, call its `open(peer_id)`, and feed the
    /// `Ok` / `Err` result back through
    /// `observe_handshake_outcome` so the state machine advances
    /// to `Active` / `Failing`. This closes the v3.0.3 12.18 D.3
    /// follow-up; the scorer integration (KDC2-1.9) is a
    /// separate concern.
    async fn tick_once(&self) {
        let started = Instant::now();
        let peer_count = self.peer_count().await;
        let transport_count = self.transport_count();
        debug!(
            peer_count,
            transport_count,
            "mesh-router: tick (12.18 D.3 active; full scorer integration KDC2-1.9 follow-up)",
        );
        // 12.18 D.3 — drive HTTPS-fallback activation. Reads
        // the state map under a snapshot lock so the registry
        // walk + open() don't hold the write lock across a
        // network-IO await.
        self.drive_https_fallback_activations().await;
        // KDC2-1.12.b — record decision time. Use saturating
        // cast so a freakishly long tick (clock skew, stall)
        // bucket-saturates rather than panics.
        if let Some(m) = &self.metrics {
            let us = started.elapsed().as_micros() as f64;
            if let Ok(mut guard) = m.lock() {
                guard.observe(us);
            }
        }
    }

    /// Phase 12.18 D.3 — for every peer in `Activating`, open
    /// the registered `NebulaHttps443` transport + feed the result
    /// back into the state machine.
    ///
    /// Behavior:
    ///   * Snapshots the `Activating` peer-id list under a read
    ///     lock (released before any network IO).
    ///   * Finds the first `TransportKind::NebulaHttps443` in the
    ///     registry (today there's at most one; the registry
    ///     intentionally allows multiple impls for future
    ///     experimentation).
    ///   * For each Activating peer, calls `transport.open(
    ///     peer_id)` + `observe_handshake_outcome(peer_id, ok)`.
    ///   * Logs `Active` / `Failing` transitions at info level
    ///     so operators see the activation cycle in the
    ///     `mackesd serve` log.
    ///
    /// Returns the count of activation attempts driven this
    /// tick. Exposed so tests + operator-mode smokes can
    /// exercise the drive without owning the full tick loop.
    pub async fn drive_https_fallback_activations(&self) -> usize {
        // Look up the NebulaHttps443 transport; bail without an
        // attempt count if none is registered (D.2 hasn't
        // landed in the registry yet, or the deployment chose
        // not to register one).
        let https443 = match self.find_transport(TransportKind::NebulaHttps443) {
            Some(t) => t,
            None => return 0,
        };
        // Snapshot the Activating peer-id list. Hold the read
        // lock only as long as the iteration; drop before any
        // open() awaits to keep the per-tick write lock contention
        // sub-millisecond.
        let activating: Vec<String> = {
            let state = self.state.read().await;
            state
                .iter()
                .filter(|(_, path)| {
                    path.https_state == mackes_transport::peer_path::HttpsFallbackState::Activating
                })
                .map(|(id, _)| id.clone())
                .collect()
        };
        let attempts = activating.len();
        for peer_id in activating {
            let result = https443.open(&peer_id).await;
            let ok = result.is_ok();
            if let Err(ref e) = result {
                info!(
                    peer = %peer_id,
                    code = e.code(),
                    "mesh-router: NebulaHttps443::open failed; marking peer Failing",
                );
            } else {
                info!(
                    peer = %peer_id,
                    "mesh-router: NebulaHttps443::open succeeded; peer Active",
                );
            }
            // Drop the live Connection — D.3 closes the
            // activation acceptance loop. The connection-keeping
            // slice (D.4: hold the conn across sends, drive
            // packet writes through it) lands separately.
            drop(result);
            // Feed the outcome back into the state machine.
            // `None` when the peer was concurrently removed
            // from the state map; that's a benign race.
            let _ = self.observe_handshake_outcome(&peer_id, ok).await;
        }
        attempts
    }

    /// Locate the first registered transport whose `kind()`
    /// matches. `None` when no impl was registered for that
    /// kind. Cheap O(n) over the small (≤ 4) registry.
    #[must_use]
    pub fn find_transport(&self, kind: TransportKind) -> Option<Arc<dyn Transport>> {
        self.registry.iter().find(|t| t.kind() == kind).cloned()
    }

    /// Pure path-switch detector. Given a peer's current
    /// `PeerPath` and a fresh scoring result, return the
    /// `PathSwitchEvent` to emit if the primary changed —
    /// `None` when the new selection matches the old one.
    ///
    /// Exposed for unit tests + future direct callers that want
    /// to drive the scoring + audit side without owning the
    /// tick loop.
    #[must_use]
    pub fn detect_switch(
        prior: &PeerPath,
        new_primary: TransportKind,
        new_reason: SwitchReason,
        now_ms: i64,
    ) -> Option<PathSwitchEvent> {
        if prior.primary == new_primary {
            return None;
        }
        Some(PathSwitchEvent::switch(
            prior.peer_id.clone(),
            Some(prior.primary),
            new_primary,
            new_reason,
            now_ms,
        ))
    }

    /// Phase 12.18 wire — feed one probe-pair outcome into the
    /// per-peer HTTPS-fallback transition machine. Updates
    /// `PeerPath::consecutive_udp_failures` +
    /// `PeerPath::https_state` for `peer_id`. Returns the new
    /// state so the caller can audit-log the transition.
    ///
    /// The future scorer integration (KDC2-1.9 follow-up) will
    /// call this from `tick_once` after observing each peer's
    /// direct-UDP + DERP-UDP probe outcomes; operator smokes +
    /// integration tests call it directly. `None` when the peer
    /// isn't in the state map yet.
    pub async fn observe_probe_outcome(
        &self,
        peer_id: &str,
        outcome: crate::https_fallback::ProbePairOutcome,
    ) -> Option<crate::https_fallback::HttpsFallbackState> {
        let mut state = self.state.write().await;
        let path = state.get_mut(peer_id)?;
        let new_state = crate::https_fallback::observe_peer(
            path,
            crate::https_fallback::TransitionInput::Probe(outcome),
        );
        Some(new_state)
    }

    /// Phase 12.18 wire — feed a TLS-handshake-completion signal
    /// into the per-peer HTTPS-fallback machine. The NebulaHttps443
    /// transport's open() result wires here once the v3.0.3
    /// 12.18 closure ships the transport (D.2 follow-up).
    pub async fn observe_handshake_outcome(
        &self,
        peer_id: &str,
        ok: bool,
    ) -> Option<crate::https_fallback::HttpsFallbackState> {
        let mut state = self.state.write().await;
        let path = state.get_mut(peer_id)?;
        let input = if ok {
            crate::https_fallback::TransitionInput::HandshakeOk
        } else {
            crate::https_fallback::TransitionInput::HandshakeFailed
        };
        Some(crate::https_fallback::observe_peer(path, input))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_transport::conformance::MockTransport;
    use mackes_transport::TransportKind;

    fn new_state() -> RouterState {
        Arc::new(RwLock::new(HashMap::new()))
    }

    fn new_registry() -> TransportRegistry {
        Arc::new(vec![
            Arc::new(MockTransport::new(TransportKind::NebulaDirect)) as Arc<dyn Transport>,
            Arc::new(MockTransport::new(TransportKind::KdcTls)) as Arc<dyn Transport>,
        ])
    }

    #[test]
    fn worker_construction_records_transport_count() {
        let w = MeshRouterWorker::new(new_state(), new_registry());
        assert_eq!(w.transport_count(), 2);
    }

    #[test]
    fn worker_with_tick_overrides_default_cadence() {
        let w =
            MeshRouterWorker::new(new_state(), new_registry()).with_tick(Duration::from_millis(50));
        assert_eq!(w.tick, Duration::from_millis(50));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn peer_count_starts_at_zero() {
        let w = MeshRouterWorker::new(new_state(), new_registry());
        assert_eq!(w.peer_count().await, 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn peer_count_reflects_inserted_peers() {
        let state = new_state();
        let w = MeshRouterWorker::new(state.clone(), new_registry());
        {
            let mut s = state.write().await;
            s.insert(
                "peer-A".into(),
                PeerPath::initial("peer-A".into(), TransportKind::NebulaDirect),
            );
            s.insert(
                "peer-B".into(),
                PeerPath::initial("peer-B".into(), TransportKind::KdcTls),
            );
        }
        assert_eq!(w.peer_count().await, 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tick_once_records_decision_us_when_metrics_attached() {
        // KDC2-1.12.b lock — the wired-in histogram must
        // see a sample after one tick_once.
        let metrics = Arc::new(StdMutex::new(crate::metrics::kdc2_router_decision_us()));
        let w =
            MeshRouterWorker::new(new_state(), new_registry()).with_metrics(Arc::clone(&metrics));
        w.tick_once().await;
        let snapshot = metrics.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(snapshot.count, 1, "tick_once must record one sample");
        // Some bucket must be non-zero — concrete value depends
        // on machine speed; the test_loop is well under 50 ms.
        assert!(snapshot.buckets.iter().any(|b| b.count > 0));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tick_once_without_metrics_is_a_noop_observation() {
        // Default constructor doesn't attach metrics; tick must
        // still run cleanly (regression guard against panics).
        let w = MeshRouterWorker::new(new_state(), new_registry());
        w.tick_once().await;
        // No metrics handle to assert on; reaching this line is
        // the lock.
    }

    #[tokio::test(flavor = "current_thread")]
    async fn worker_name_matches_module() {
        let w = MeshRouterWorker::new(new_state(), new_registry());
        assert_eq!(w.name(), "mesh-router");
    }

    #[test]
    fn detect_switch_returns_none_when_primary_unchanged() {
        let prior = PeerPath::initial("p".into(), TransportKind::NebulaDirect);
        let r = MeshRouterWorker::detect_switch(
            &prior,
            TransportKind::NebulaDirect,
            SwitchReason::Initial,
            0,
        );
        assert!(r.is_none(), "primary unchanged → no audit emission");
    }

    #[test]
    fn detect_switch_emits_when_primary_flips() {
        let prior = PeerPath::initial("peer-A".into(), TransportKind::NebulaDirect);
        let event = MeshRouterWorker::detect_switch(
            &prior,
            TransportKind::KdcTls,
            SwitchReason::HealthDegraded(TransportKind::NebulaDirect),
            1_700_000_000_000,
        )
        .expect("primary flipped → event emitted");
        let summary = event.summary();
        assert!(summary.contains("peer=peer-A"));
        assert!(summary.contains("from=nebula_direct"));
        assert!(summary.contains("to=kdc_tls"));
        assert!(summary.contains("reason=health_degraded_nebula_direct"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn worker_exits_on_shutdown_request() {
        // Construct + spawn the worker. Trigger shutdown
        // immediately. Worker must exit cleanly (Ok(())) without
        // waiting for a tick.
        let state = new_state();
        let registry = new_registry();
        let mut w = MeshRouterWorker::new(state, registry).with_tick(Duration::from_secs(60));

        // Build a fresh shutdown-token pair the same way every
        // other worker test does (heartbeat.rs, app_sync.rs).
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = super::super::ShutdownToken::from_receiver(rx);

        let handle = tokio::spawn(async move { w.run(token).await });
        // Flip the shutdown flag.
        tx.send(true).expect("shutdown channel intact");
        let result = handle.await.expect("worker join");
        assert!(result.is_ok(), "worker must exit Ok on shutdown");
    }

    // --- Phase 12.18 wire — observe_probe_outcome / handshake ----------

    #[tokio::test(flavor = "current_thread")]
    async fn observe_probe_outcome_walks_per_peer_state() {
        let state = new_state();
        {
            let mut g = state.write().await;
            g.insert(
                "alice".into(),
                PeerPath::initial("alice".into(), TransportKind::NebulaDirect),
            );
        }
        let w = MeshRouterWorker::new(Arc::clone(&state), new_registry());
        use crate::https_fallback::{HttpsFallbackState, ProbePairOutcome};
        // 3 consecutive failures → Activating + counter reset to 0.
        for _ in 0..3 {
            assert!(w
                .observe_probe_outcome("alice", ProbePairOutcome::BothUdpFailed)
                .await
                .is_some());
        }
        let g = state.read().await;
        let path = g.get("alice").unwrap();
        assert_eq!(
            path.https_state,
            mackes_transport::peer_path::HttpsFallbackState::Activating,
        );
        assert_eq!(path.consecutive_udp_failures, 0);
        // The returned state matches.
        drop(g);
        let s = w
            .observe_probe_outcome("alice", ProbePairOutcome::AnyUdpSucceeded)
            .await
            .unwrap();
        assert_eq!(s, HttpsFallbackState::Activating);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn observe_probe_outcome_unknown_peer_returns_none() {
        let w = MeshRouterWorker::new(new_state(), new_registry());
        let s = w
            .observe_probe_outcome(
                "ghost",
                crate::https_fallback::ProbePairOutcome::BothUdpFailed,
            )
            .await;
        assert!(s.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn observe_handshake_outcome_walks_active_or_failing() {
        let state = new_state();
        {
            let mut g = state.write().await;
            g.insert(
                "alice".into(),
                PeerPath::initial("alice".into(), TransportKind::NebulaDirect),
            );
        }
        let w = MeshRouterWorker::new(Arc::clone(&state), new_registry());
        // First push to Activating.
        use crate::https_fallback::{HttpsFallbackState, ProbePairOutcome};
        for _ in 0..3 {
            w.observe_probe_outcome("alice", ProbePairOutcome::BothUdpFailed)
                .await;
        }
        // HandshakeOk → Active.
        let s = w.observe_handshake_outcome("alice", true).await.unwrap();
        assert_eq!(s, HttpsFallbackState::Active);
        // Subsequent fail → Failing.
        let s = w.observe_handshake_outcome("alice", false).await.unwrap();
        // From Active, handshake outcomes are no-ops per the
        // transition table — only TunnelLost/Probe transitions
        // out of Active. So the state stays Active.
        assert_eq!(s, HttpsFallbackState::Active);
    }

    // --- Phase 12.18 D.3 — drive_https_fallback_activations ------------

    /// Build a router whose registry holds a MockTransport
    /// pretending to be NebulaHttps443. MockTransport's `open` returns
    /// Ok for `peer_id == "paired"` and Err otherwise — which
    /// matches the activation drive's call shape: feed the
    /// per-peer peer_id, get a result back.
    fn https443_registry() -> TransportRegistry {
        Arc::new(vec![
            Arc::new(MockTransport::new(TransportKind::NebulaHttps443)) as Arc<dyn Transport>,
        ])
    }

    #[tokio::test(flavor = "current_thread")]
    async fn drive_returns_zero_when_no_https443_registered() {
        // Default test registry has NebulaDirect + KdcTls only; no
        // NebulaHttps443 impl. drive() must return 0 without panicking.
        let state = new_state();
        {
            let mut g = state.write().await;
            let mut p = PeerPath::initial("alice".into(), TransportKind::NebulaDirect);
            p.https_state = mackes_transport::peer_path::HttpsFallbackState::Activating;
            g.insert("alice".into(), p);
        }
        let w = MeshRouterWorker::new(Arc::clone(&state), new_registry());
        let attempts = w.drive_https_fallback_activations().await;
        assert_eq!(attempts, 0);
        // Peer state untouched — still Activating.
        let g = state.read().await;
        assert_eq!(
            g.get("alice").unwrap().https_state,
            mackes_transport::peer_path::HttpsFallbackState::Activating,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn drive_skips_peers_not_in_activating() {
        // Peers in Inactive / Active / Failing aren't touched.
        let state = new_state();
        {
            let mut g = state.write().await;
            // Inactive (default).
            g.insert(
                "inactive".into(),
                PeerPath::initial("inactive".into(), TransportKind::NebulaDirect),
            );
            // Active.
            let mut active = PeerPath::initial("active".into(), TransportKind::NebulaDirect);
            active.https_state = mackes_transport::peer_path::HttpsFallbackState::Active;
            g.insert("active".into(), active);
            // Failing.
            let mut failing = PeerPath::initial("failing".into(), TransportKind::NebulaDirect);
            failing.https_state = mackes_transport::peer_path::HttpsFallbackState::Failing;
            g.insert("failing".into(), failing);
        }
        let w = MeshRouterWorker::new(Arc::clone(&state), https443_registry());
        let attempts = w.drive_https_fallback_activations().await;
        assert_eq!(attempts, 0, "no Activating peers ⇒ no open() calls");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn drive_advances_activating_peer_to_active_on_open_ok() {
        // Mock transport returns Ok for peer_id == "paired";
        // drive() should call observe_handshake_outcome(_, true)
        // → peer transitions Activating → Active.
        let state = new_state();
        {
            let mut g = state.write().await;
            let mut p = PeerPath::initial("paired".into(), TransportKind::NebulaDirect);
            p.https_state = mackes_transport::peer_path::HttpsFallbackState::Activating;
            g.insert("paired".into(), p);
        }
        let w = MeshRouterWorker::new(Arc::clone(&state), https443_registry());
        let attempts = w.drive_https_fallback_activations().await;
        assert_eq!(attempts, 1);
        let g = state.read().await;
        assert_eq!(
            g.get("paired").unwrap().https_state,
            mackes_transport::peer_path::HttpsFallbackState::Active,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn drive_advances_activating_peer_to_failing_on_open_err() {
        // Mock transport returns Err for peer_id == "ghost";
        // drive() should call observe_handshake_outcome(_, false)
        // → peer transitions Activating → Failing.
        let state = new_state();
        {
            let mut g = state.write().await;
            let mut p = PeerPath::initial("ghost".into(), TransportKind::NebulaDirect);
            p.https_state = mackes_transport::peer_path::HttpsFallbackState::Activating;
            g.insert("ghost".into(), p);
        }
        let w = MeshRouterWorker::new(Arc::clone(&state), https443_registry());
        let attempts = w.drive_https_fallback_activations().await;
        assert_eq!(attempts, 1);
        let g = state.read().await;
        assert_eq!(
            g.get("ghost").unwrap().https_state,
            mackes_transport::peer_path::HttpsFallbackState::Failing,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn drive_handles_multiple_activating_peers_in_one_tick() {
        // Two peers both in Activating: one will get Ok ("paired"),
        // the other Err. Both transition correctly in a single
        // drive() call.
        let state = new_state();
        {
            let mut g = state.write().await;
            let mut p = PeerPath::initial("paired".into(), TransportKind::NebulaDirect);
            p.https_state = mackes_transport::peer_path::HttpsFallbackState::Activating;
            g.insert("paired".into(), p);
            let mut q = PeerPath::initial("ghost".into(), TransportKind::NebulaDirect);
            q.https_state = mackes_transport::peer_path::HttpsFallbackState::Activating;
            g.insert("ghost".into(), q);
        }
        let w = MeshRouterWorker::new(Arc::clone(&state), https443_registry());
        let attempts = w.drive_https_fallback_activations().await;
        assert_eq!(attempts, 2);
        let g = state.read().await;
        assert_eq!(
            g.get("paired").unwrap().https_state,
            mackes_transport::peer_path::HttpsFallbackState::Active,
        );
        assert_eq!(
            g.get("ghost").unwrap().https_state,
            mackes_transport::peer_path::HttpsFallbackState::Failing,
        );
    }

    #[test]
    fn find_transport_returns_some_for_known_kind() {
        let w = MeshRouterWorker::new(new_state(), https443_registry());
        assert!(w.find_transport(TransportKind::NebulaHttps443).is_some());
        // Not in registry → None.
        assert!(w.find_transport(TransportKind::KdcTls).is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tick_once_drives_activation_for_activating_peers() {
        // End-to-end: an Activating peer + a registered NebulaHttps443
        // transport + a tick_once call → peer is Active after
        // the tick.
        let state = new_state();
        {
            let mut g = state.write().await;
            let mut p = PeerPath::initial("paired".into(), TransportKind::NebulaDirect);
            p.https_state = mackes_transport::peer_path::HttpsFallbackState::Activating;
            g.insert("paired".into(), p);
        }
        let w = MeshRouterWorker::new(Arc::clone(&state), https443_registry());
        w.tick_once().await;
        let g = state.read().await;
        assert_eq!(
            g.get("paired").unwrap().https_state,
            mackes_transport::peer_path::HttpsFallbackState::Active,
        );
    }
}
