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
//! AUD3 S-2/S-5 (2026-06-12): the KDC2-1.9 scorer + KDC2-1.12
//! audit-chain feed are WIRED — `tick_once` runs
//! [`mackes_transport::scorer::select`] per tracked peer (Control
//! class drives the primary; the CV-1 encryption floor gates
//! content classes inside the scorer) and every primary flip
//! emits a [`PathSwitchEvent`] into the hash-chained events table
//! via [`crate::events::append_and_alert`] (so `[[alert_hooks]]`
//! with `kind = "lifecycle"` fire on path switches too).

#![cfg(feature = "async-services")]

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use mackes_transport::peer_path::{PeerPath, SwitchReason};
use mackes_transport::scorer::Policy;
use mackes_transport::{Connection, MessageClass, Transport, TransportError, TransportKind};
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

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

/// Bound each peer's received fallback frames so a stalled consumer cannot
/// turn an otherwise healthy TLS stream into unbounded daemon memory.
const HTTPS_INBOX_CAPACITY: usize = 256;

/// Stable loopback underlay endpoint rendered into Nebula's static host map for
/// the configured HTTPS relay. Nebula sends encrypted UDP packets here after
/// the public UDP endpoint is unavailable; the router moves them over TLS.
pub const DEFAULT_HTTPS_UDP_BRIDGE_BIND: &str = "127.0.0.1:4244";
/// Optional override for [`DEFAULT_HTTPS_UDP_BRIDGE_BIND`].
pub const HTTPS_UDP_BRIDGE_BIND_ENV: &str = "MDE_HTTPS_FALLBACK_UDP_BIND";
/// Nebula's local underlay socket. Only datagrams from this exact loopback
/// source may enter the authenticated fallback carrier.
pub const DEFAULT_NEBULA_UDP_SOURCE: &str = "127.0.0.1:4242";

/// Runtime binding between one configured lighthouse and Nebula's local UDP
/// underlay socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpsUdpBridgeConfig {
    /// Local UDP address advertised to Nebula.
    pub bind_addr: SocketAddr,
    /// Exact local Nebula socket allowed to source and receive packets.
    pub nebula_source: SocketAddr,
    /// Lighthouse node id whose retained TLS channel carries the datagrams.
    pub peer_id: PeerId,
}

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

type HttpsConnections = Arc<RwLock<HashMap<PeerId, Arc<dyn Connection>>>>;
type HttpsInboxes = Arc<RwLock<HashMap<PeerId, VecDeque<Vec<u8>>>>>;
type HttpsReaders = Arc<RwLock<HashMap<PeerId, JoinHandle<()>>>>;

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
    /// Live, payload-capable HTTPS fallback channels. A peer may only be in
    /// `HttpsFallbackState::Active` while it has an entry here.
    https_connections: HttpsConnections,
    /// Frames received by the connection reader, waiting for the mesh packet
    /// dispatcher to consume them.
    https_inboxes: HttpsInboxes,
    /// Per-peer reader tasks. Kept so recovery, replacement, and worker
    /// shutdown can terminate the old stream deterministically.
    https_readers: HttpsReaders,
    /// Optional live Nebula UDP source/sink for the selected relay.
    https_udp_bridge: Option<HttpsUdpBridgeConfig>,
    tick: Duration,
    /// KDC2-1.12.b — optional handle to the
    /// `kdc2_router_decision_us` histogram. When `Some`, every
    /// `tick_once` records its decision microseconds via
    /// `Histogram::observe`. `None` for tests + bootstrap paths
    /// that don't care about telemetry.
    metrics: Option<RouterMetrics>,
    /// KDC2-1.9 (AUD3 S-2) — routing policy the scorer consumes.
    /// Loaded from `/etc/mde/connect/policy.toml` (+ user
    /// override) at spawn; defaults to baseline.
    policy: Policy,
    /// KDC2-1.12 (AUD3 S-5) — audit sink: `(db_path, node_id)`.
    /// When `Some`, every primary flip appends a hash-chained
    /// `PathSwitchEvent` row (kind = `lifecycle`) and fires the
    /// configured alert hooks. `None` for tests.
    audit_sink: Option<(PathBuf, String)>,
}

impl MeshRouterWorker {
    /// Construct a new mesh-router worker with the default
    /// 10s tick cadence.
    #[must_use]
    pub fn new(state: RouterState, registry: TransportRegistry) -> Self {
        Self {
            state,
            registry,
            https_connections: Arc::new(RwLock::new(HashMap::new())),
            https_inboxes: Arc::new(RwLock::new(HashMap::new())),
            https_readers: Arc::new(RwLock::new(HashMap::new())),
            https_udp_bridge: None,
            tick: DEFAULT_TICK,
            metrics: None,
            policy: Policy::baseline(),
            audit_sink: None,
        }
    }

    /// Bind the router's selected HTTPS channel to a local Nebula UDP endpoint.
    #[must_use]
    pub fn with_https_udp_bridge(mut self, config: HttpsUdpBridgeConfig) -> Self {
        self.https_udp_bridge = Some(config);
        self
    }

    /// KDC2-1.9 (AUD3 S-2) — override the routing policy (loaded
    /// from policy.toml by the daemon; baseline otherwise).
    #[must_use]
    pub fn with_policy(mut self, policy: Policy) -> Self {
        self.policy = policy;
        self
    }

    /// KDC2-1.12 (AUD3 S-5) — attach the audit sink so primary
    /// flips land in the hash-chained events table (+ alert hooks).
    #[must_use]
    pub fn with_audit_sink(mut self, db_path: PathBuf, node_id: String) -> Self {
        self.audit_sink = Some((db_path, node_id));
        self
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

        let bridge = match self.https_udp_bridge.clone() {
            Some(config) => Some((
                UdpSocket::bind(config.bind_addr).await.map_err(|e| {
                    anyhow::anyhow!("HTTPS UDP bridge bind {}: {e}", config.bind_addr)
                })?,
                config.peer_id,
            )),
            None => None,
        };
        let mut interval = tokio::time::interval(self.tick);
        // First tick fires immediately; skip it so the first
        // observation isn't done before any transport has had a
        // chance to settle after worker startup.
        interval.tick().await;

        if let Some((socket, peer_id)) = bridge {
            return self
                .run_https_udp_bridge(
                    socket,
                    peer_id,
                    self.https_udp_bridge
                        .as_ref()
                        .expect("bridge config exists")
                        .nebula_source,
                    shutdown,
                    interval,
                )
                .await;
        }

        loop {
            tokio::select! {
                _ = shutdown.wait() => {
                    info!("mesh-router: shutdown requested; exiting");
                    self.shutdown_https_connections().await;
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
    pub(crate) async fn run_https_udp_bridge(
        &self,
        socket: UdpSocket,
        peer_id: PeerId,
        nebula_source: SocketAddr,
        mut shutdown: ShutdownToken,
        mut router_tick: tokio::time::Interval,
    ) -> anyhow::Result<()> {
        if !nebula_source.ip().is_loopback() {
            anyhow::bail!("HTTPS UDP bridge source must be loopback: {nebula_source}");
        }
        let mut udp_buf = vec![0_u8; mackes_nebula_https_tunnel::MAX_FRAME_SIZE];
        let mut inbound_tick = tokio::time::interval(Duration::from_millis(5));
        inbound_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        info!(bind = %socket.local_addr()?, source = %nebula_source, peer = %peer_id, "mesh-router: HTTPS UDP bridge active");

        loop {
            tokio::select! {
                _ = shutdown.wait() => {
                    self.shutdown_https_connections().await;
                    return Ok(());
                }
                _ = router_tick.tick() => self.tick_once().await,
                received = socket.recv_from(&mut udp_buf) => {
                    let (length, source) = received?;
                    if source != nebula_source {
                        warn!(source = %source, expected = %nebula_source, "mesh-router: dropping untrusted local HTTPS UDP source");
                        continue;
                    }
                    if let Err(error) = self
                        .send_https_payload_with_reconnect(&peer_id, &udp_buf[..length])
                        .await
                    {
                        info!(peer = %peer_id, code = error.code(), "mesh-router: HTTPS UDP packet unavailable");
                    }
                }
                _ = inbound_tick.tick() => {
                    while let Some(payload) = self.try_recv_https_payload(&peer_id).await {
                        socket.send_to(&payload, nebula_source).await?;
                    }
                }
            }
        }
    }

    async fn send_https_payload_with_reconnect(
        &self,
        peer_id: &str,
        payload: &[u8],
    ) -> Result<(), TransportError> {
        if self.send_https_payload(peer_id, payload).await.is_ok() {
            return Ok(());
        }
        self.force_https_activation(peer_id).await;
        self.drive_https_fallback_activations().await;
        self.send_https_payload(peer_id, payload).await
    }

    async fn force_https_activation(&self, peer_id: &str) {
        let has_connection = self.https_connections.read().await.contains_key(peer_id);
        let mut state = self.state.write().await;
        let path = state
            .entry(peer_id.to_string())
            .or_insert_with(|| PeerPath::initial(peer_id.to_string(), TransportKind::NebulaDirect));
        if path.https_state == mackes_transport::peer_path::HttpsFallbackState::Active
            && !has_connection
        {
            crate::https_fallback::observe_peer(
                path,
                crate::https_fallback::TransitionInput::TunnelLost,
            );
        }
        for _ in 0..mackes_nebula_https_tunnel::activation::FAILURE_THRESHOLD {
            if path.https_state == mackes_transport::peer_path::HttpsFallbackState::Activating {
                break;
            }
            crate::https_fallback::observe_peer(
                path,
                crate::https_fallback::TransitionInput::Probe(
                    crate::https_fallback::ProbePairOutcome::BothUdpFailed,
                ),
            );
        }
    }

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
            transport_count, "mesh-router: tick (12.18 D.3 + KDC2-1.9 scorer active)",
        );
        // 12.18 D.3 — drive HTTPS-fallback activation. Reads
        // the state map under a snapshot lock so the registry
        // walk + open() don't hold the write lock across a
        // network-IO await.
        self.drive_https_fallback_activations().await;
        // KDC2-1.9 (AUD3 S-2) — run the scorer over the registry
        // per tracked peer; flip primaries + emit the KDC2-1.12
        // audit entries on change.
        self.select_paths().await;
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

    /// For every peer in `Activating`, open and retain a payload-capable
    /// `NebulaHttps443` connection, then feed the result into the state machine.
    ///
    /// Behavior:
    ///   * Snapshots the `Activating` peer-id list under a read
    ///     lock (released before any network IO).
    ///   * Finds the first `TransportKind::NebulaHttps443` in the
    ///     registry (today there's at most one; the registry
    ///     intentionally allows multiple impls for future
    ///     experimentation).
    ///   * For each Activating peer, calls `transport.open(peer_id)` and rejects
    ///     handles that do not implement framed I/O.
    ///   * Stores the channel before marking the peer Active, then starts a
    ///     reader that clears Active immediately on EOF or framing failure.
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
            match https443.open(&peer_id).await {
                Ok(connection) if connection.supports_framed_io() => {
                    let connection: Arc<dyn Connection> = Arc::from(connection);
                    self.install_https_connection(peer_id.clone(), Arc::clone(&connection))
                        .await;
                    let state = self.observe_handshake_outcome(&peer_id, true).await;
                    if state.is_some_and(crate::https_fallback::HttpsFallbackState::is_active) {
                        self.spawn_https_reader(peer_id.clone(), connection).await;
                        info!(
                            peer = %peer_id,
                            "mesh-router: HTTPS fallback channel owned and Active",
                        );
                    } else {
                        self.remove_https_connection(&peer_id).await;
                    }
                }
                Ok(connection) => {
                    info!(
                        peer = %peer_id,
                        connection = %connection.id(),
                        "mesh-router: HTTPS handshake returned no framed channel; marking peer Failing",
                    );
                    let _ = self.observe_handshake_outcome(&peer_id, false).await;
                }
                Err(e) => {
                    info!(
                        peer = %peer_id,
                        code = e.code(),
                        "mesh-router: NebulaHttps443::open failed; marking peer Failing",
                    );
                    let _ = self.observe_handshake_outcome(&peer_id, false).await;
                }
            }
        }
        attempts
    }

    async fn install_https_connection(&self, peer_id: PeerId, connection: Arc<dyn Connection>) {
        self.remove_https_connection(&peer_id).await;
        self.https_connections
            .write()
            .await
            .insert(peer_id, connection);
    }

    async fn spawn_https_reader(&self, peer_id: PeerId, connection: Arc<dyn Connection>) {
        let state = Arc::clone(&self.state);
        let connections = Arc::clone(&self.https_connections);
        let inboxes = Arc::clone(&self.https_inboxes);
        let task_peer = peer_id.clone();
        let task = tokio::spawn(async move {
            loop {
                match connection.recv_frame().await {
                    Ok(payload) => {
                        let mut inboxes = inboxes.write().await;
                        let inbox = inboxes.entry(task_peer.clone()).or_default();
                        if inbox.len() == HTTPS_INBOX_CAPACITY {
                            inbox.pop_front();
                        }
                        inbox.push_back(payload);
                    }
                    Err(e) => {
                        info!(
                            peer = %task_peer,
                            code = e.code(),
                            "mesh-router: HTTPS fallback stream lost; clearing Active",
                        );
                        Self::clear_lost_https_connection(
                            &state,
                            &connections,
                            &task_peer,
                            &connection,
                        )
                        .await;
                        return;
                    }
                }
            }
        });
        if let Some(old) = self.https_readers.write().await.insert(peer_id, task) {
            old.abort();
        }
    }

    async fn clear_lost_https_connection(
        state: &RouterState,
        connections: &HttpsConnections,
        peer_id: &str,
        lost: &Arc<dyn Connection>,
    ) {
        let removed = {
            let mut connections = connections.write().await;
            if connections
                .get(peer_id)
                .is_some_and(|current| Arc::ptr_eq(current, lost))
            {
                connections.remove(peer_id);
                true
            } else {
                false
            }
        };
        if removed {
            // Keep already authenticated complete frames queued. The peer may
            // close immediately after its reply, and discarding that inbox here
            // races the UDP bridge's next drain tick.
            if let Some(path) = state.write().await.get_mut(peer_id) {
                crate::https_fallback::observe_peer(
                    path,
                    crate::https_fallback::TransitionInput::TunnelLost,
                );
            }
        }
    }

    async fn remove_https_connection(&self, peer_id: &str) {
        self.https_connections.write().await.remove(peer_id);
        self.https_inboxes.write().await.remove(peer_id);
        if let Some(reader) = self.https_readers.write().await.remove(peer_id) {
            reader.abort();
        }
    }

    async fn shutdown_https_connections(&self) {
        let peers: Vec<_> = self
            .https_connections
            .read()
            .await
            .keys()
            .cloned()
            .collect();
        for peer_id in peers {
            self.remove_https_connection(&peer_id).await;
            if let Some(path) = self.state.write().await.get_mut(&peer_id) {
                crate::https_fallback::observe_peer(
                    path,
                    crate::https_fallback::TransitionInput::TunnelLost,
                );
            }
        }
    }

    /// Send one intact Nebula packet through the retained HTTPS channel.
    /// Stream errors atomically evict the channel and clear `Active`.
    pub async fn send_https_payload(
        &self,
        peer_id: &str,
        payload: &[u8],
    ) -> Result<(), TransportError> {
        let connection = self
            .https_connections
            .read()
            .await
            .get(peer_id)
            .cloned()
            .ok_or(TransportError::Unreachable {
                code: "no_active_https_channel",
            })?;
        if let Err(e) = connection.send_frame(payload).await {
            Self::clear_lost_https_connection(
                &self.state,
                &self.https_connections,
                peer_id,
                &connection,
            )
            .await;
            if let Some(reader) = self.https_readers.write().await.remove(peer_id) {
                reader.abort();
            }
            return Err(e);
        }
        Ok(())
    }

    /// Take the next inbound Nebula packet received over HTTPS, if one is ready.
    pub async fn try_recv_https_payload(&self, peer_id: &str) -> Option<Vec<u8>> {
        self.https_inboxes
            .write()
            .await
            .get_mut(peer_id)
            .and_then(VecDeque::pop_front)
    }

    /// Number of peers with an owned, payload-capable fallback channel.
    pub async fn active_https_connection_count(&self) -> usize {
        self.https_connections.read().await.len()
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
        let new_state = {
            let mut state = self.state.write().await;
            let path = state.get_mut(peer_id)?;
            crate::https_fallback::observe_peer(
                path,
                crate::https_fallback::TransitionInput::Probe(outcome),
            )
        };
        if !new_state.is_active() {
            self.remove_https_connection(peer_id).await;
        }
        Some(new_state)
    }

    /// Feed a TLS-handshake-completion signal into the per-peer fallback
    /// machine. A successful handshake is accepted only when the router already
    /// owns a payload-capable connection for the peer.
    pub async fn observe_handshake_outcome(
        &self,
        peer_id: &str,
        ok: bool,
    ) -> Option<crate::https_fallback::HttpsFallbackState> {
        let owns_channel = ok && self.https_connections.read().await.contains_key(peer_id);
        let mut state = self.state.write().await;
        let path = state.get_mut(peer_id)?;
        let input = if owns_channel {
            crate::https_fallback::TransitionInput::HandshakeOk
        } else {
            crate::https_fallback::TransitionInput::HandshakeFailed
        };
        Some(crate::https_fallback::observe_peer(path, input))
    }

    /// KDC2-1.9 (AUD3 S-2) — run the scorer over the registry for
    /// every tracked peer and apply the selection. The `Control`
    /// class drives the primary (control reachability is the
    /// router's baseline guarantee; per-class dispatch reads
    /// `PeerPath::transport_for`, and the CV-1 encryption floor
    /// gates content classes inside the scorer itself).
    ///
    /// On a primary flip: update `primary`/`fallback`/`last_switch_*`
    /// and emit the KDC2-1.12 [`PathSwitchEvent`] through
    /// [`crate::events::append_and_alert`] (hash-chained events row,
    /// kind = `lifecycle`, + the configured 12.6.4 alert hooks).
    /// Quiet ticks only refresh the fallback. Returns the number of
    /// switches applied this tick.
    ///
    /// Probing happens outside the state lock (scorer::select awaits
    /// each transport's `probe`); the write lock is held only for the
    /// in-memory flip.
    pub async fn select_paths(&self) -> usize {
        if self.registry.is_empty() {
            return 0;
        }
        let peer_ids: Vec<String> = {
            let state = self.state.read().await;
            state.keys().cloned().collect()
        };
        let mut switches = 0;
        for peer_id in peer_ids {
            let Some(selection) = mackes_transport::scorer::select(
                self.registry.as_slice(),
                &peer_id,
                MessageClass::Control,
                &self.policy,
            )
            .await
            else {
                // No sendable transport this tick — leave the prior
                // path in place (the https_fallback machine handles
                // degradation; flapping to "nothing" helps no one).
                continue;
            };
            let event = {
                let mut state = self.state.write().await;
                let Some(path) = state.get_mut(&peer_id) else {
                    continue; // peer evicted between snapshot + lock
                };
                if path.primary == selection.primary {
                    // Quiet tick — refresh the fallback only.
                    path.fallback = selection.fallback;
                    continue;
                }
                let from = path.primary;
                // Prior-state-aware reason: a pin/deny decision
                // surfaces as Policy from the scorer; any other flip
                // with a prior primary means the old one lost on
                // health/rank.
                let reason = match selection.reason {
                    SwitchReason::Initial => SwitchReason::HealthDegraded(from),
                    other => other,
                };
                path.primary = selection.primary;
                path.fallback = selection.fallback;
                path.last_switch_at = Some(std::time::SystemTime::now());
                path.last_switch_reason = reason.clone();
                let now_ms = chrono::Utc::now().timestamp_millis();
                PathSwitchEvent::switch(
                    peer_id.clone(),
                    Some(from),
                    selection.primary,
                    reason,
                    now_ms,
                )
            };
            switches += 1;
            info!("mesh-router: {}", event.summary());
            // KDC2-1.12 (AUD3 S-5) — append to the hash-chained
            // events table + fire alert hooks, off the runtime
            // thread (sync sqlite). Best-effort by design.
            if let Some((db_path, node_id)) = self.audit_sink.clone() {
                let detail = serde_json::to_value(&event)
                    .unwrap_or_else(|_| serde_json::json!({"event": "path_switch"}));
                let _ = tokio::task::spawn_blocking(move || {
                    crate::events::append_and_alert(
                        &db_path,
                        &node_id,
                        crate::events::EventKind::Lifecycle,
                        detail,
                    );
                })
                .await;
            }
        }
        switches
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_transport::conformance::MockTransport;
    use mackes_transport::{Capabilities, HealthState, MessageClassSet, TransportKind};

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
    async fn select_paths_flips_primary_to_the_scored_choice() {
        // AUD3 S-2 — a peer whose primary lost the scoring is flipped
        // to the scorer's choice; the switch metadata is recorded.
        let state = new_state();
        {
            let mut s = state.write().await;
            // Seed with KdcTls primary; the scorer prefers
            // NebulaDirect (preference-order rank 0) for Control.
            s.insert(
                "paired".into(),
                PeerPath::initial("paired".into(), TransportKind::KdcTls),
            );
        }
        let w = MeshRouterWorker::new(state.clone(), new_registry());
        let switches = w.select_paths().await;
        assert_eq!(switches, 1);
        let s = state.read().await;
        let path = s.get("paired").unwrap();
        assert_eq!(path.primary, TransportKind::NebulaDirect);
        assert_eq!(path.fallback, Some(TransportKind::KdcTls));
        assert!(path.last_switch_at.is_some());
        assert_eq!(
            path.last_switch_reason,
            SwitchReason::HealthDegraded(TransportKind::KdcTls),
            "prior-state-aware reason names the bumped transport"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn select_paths_quiet_tick_keeps_primary_and_refreshes_fallback() {
        let state = new_state();
        {
            let mut s = state.write().await;
            s.insert(
                "paired".into(),
                PeerPath::initial("paired".into(), TransportKind::NebulaDirect),
            );
        }
        let w = MeshRouterWorker::new(state.clone(), new_registry());
        assert_eq!(w.select_paths().await, 0, "already-best primary: no switch");
        let s = state.read().await;
        let path = s.get("paired").unwrap();
        assert_eq!(path.primary, TransportKind::NebulaDirect);
        assert_eq!(
            path.fallback,
            Some(TransportKind::KdcTls),
            "fallback refreshed"
        );
        assert!(path.last_switch_at.is_none(), "no switch recorded");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn select_paths_leaves_path_alone_when_nothing_sendable() {
        // The mock probes Down for unknown peers → scorer returns
        // None → the prior path must remain untouched (no flap to
        // nothing).
        let state = new_state();
        {
            let mut s = state.write().await;
            s.insert(
                "unknown-peer".into(),
                PeerPath::initial("unknown-peer".into(), TransportKind::KdcTls),
            );
        }
        let w = MeshRouterWorker::new(state.clone(), new_registry());
        assert_eq!(w.select_paths().await, 0);
        let s = state.read().await;
        assert_eq!(
            s.get("unknown-peer").unwrap().primary,
            TransportKind::KdcTls
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tick_once_records_decision_us_when_metrics_attached() {
        // KDC2-1.12.b lock — the wired-in histogram must
        // see a sample after one tick_once.
        let metrics = Arc::new(StdMutex::new(crate::metrics::kdc2_router_decision_us()));
        let w =
            MeshRouterWorker::new(new_state(), new_registry()).with_metrics(Arc::clone(&metrics));
        w.tick_once().await;
        let snapshot = metrics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    async fn handshake_success_without_owned_channel_fails_closed() {
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
        // A bare handshake result can no longer create Active; the router must
        // already own the payload channel installed by the activation driver.
        let s = w.observe_handshake_outcome("alice", true).await.unwrap();
        assert_eq!(s, HttpsFallbackState::Failing);
    }

    // --- Phase 12.18 D.3 — drive_https_fallback_activations ------------

    #[derive(Debug)]
    struct FramedMockConnection {
        id: String,
        lose_immediately: bool,
        first_payload: tokio::sync::Mutex<Option<Vec<u8>>>,
    }

    #[async_trait::async_trait]
    impl Connection for FramedMockConnection {
        fn id(&self) -> &str {
            &self.id
        }

        fn supports_framed_io(&self) -> bool {
            true
        }

        async fn send_frame(&self, _payload: &[u8]) -> Result<(), TransportError> {
            Ok(())
        }

        async fn recv_frame(&self) -> Result<Vec<u8>, TransportError> {
            if self.lose_immediately {
                return Err(TransportError::Io {
                    code: "mock_stream_lost",
                });
            }
            if let Some(payload) = self.first_payload.lock().await.take() {
                return Ok(payload);
            }
            std::future::pending().await
        }
    }

    #[derive(Debug)]
    struct FramedMockTransport;

    #[async_trait::async_trait]
    impl Transport for FramedMockTransport {
        fn kind(&self) -> TransportKind {
            TransportKind::NebulaHttps443
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                max_frame_bytes: Some(1408),
                health_window: Duration::from_secs(30),
                carries: MessageClassSet::all(),
                label: "framed-mock-https443".into(),
            }
        }

        async fn probe(&self, peer_id: &str) -> HealthState {
            if matches!(peer_id, "paired" | "lost" | "payload") {
                HealthState::Healthy
            } else {
                HealthState::Down
            }
        }

        async fn open(&self, peer_id: &str) -> Result<Box<dyn Connection>, TransportError> {
            if matches!(peer_id, "paired" | "lost" | "payload") {
                Ok(Box::new(FramedMockConnection {
                    id: format!("framed-mock:{peer_id}"),
                    lose_immediately: peer_id == "lost",
                    first_payload: tokio::sync::Mutex::new(
                        (peer_id == "payload").then(|| b"inbound nebula packet".to_vec()),
                    ),
                }))
            } else {
                Err(TransportError::Unreachable {
                    code: "mock_unpaired",
                })
            }
        }

        async fn health(&self, peer_id: &str) -> HealthState {
            self.probe(peer_id).await
        }
    }

    fn https443_registry() -> TransportRegistry {
        Arc::new(vec![Arc::new(FramedMockTransport) as Arc<dyn Transport>])
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
        assert_eq!(w.active_https_connection_count().await, 1);
        w.send_https_payload("paired", b"nebula packet")
            .await
            .expect("owned channel sends");
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
        assert_eq!(w.active_https_connection_count().await, 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn drive_rejects_handshake_only_connection_without_framed_io() {
        let state = new_state();
        {
            let mut g = state.write().await;
            let mut path = PeerPath::initial("paired".into(), TransportKind::NebulaDirect);
            path.https_state = mackes_transport::peer_path::HttpsFallbackState::Activating;
            g.insert("paired".into(), path);
        }
        let registry = Arc::new(vec![
            Arc::new(MockTransport::new(TransportKind::NebulaHttps443)) as Arc<dyn Transport>,
        ]);
        let worker = MeshRouterWorker::new(Arc::clone(&state), registry);

        assert_eq!(worker.drive_https_fallback_activations().await, 1);
        assert_eq!(worker.active_https_connection_count().await, 0);
        assert_eq!(
            state.read().await.get("paired").unwrap().https_state,
            mackes_transport::peer_path::HttpsFallbackState::Failing,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reader_stream_loss_evicts_channel_and_clears_active() {
        let state = new_state();
        {
            let mut g = state.write().await;
            let mut path = PeerPath::initial("lost".into(), TransportKind::NebulaDirect);
            path.https_state = mackes_transport::peer_path::HttpsFallbackState::Activating;
            g.insert("lost".into(), path);
        }
        let worker = MeshRouterWorker::new(Arc::clone(&state), https443_registry());
        assert_eq!(worker.drive_https_fallback_activations().await, 1);

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if state.read().await.get("lost").unwrap().https_state
                    == mackes_transport::peer_path::HttpsFallbackState::Failing
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("reader detects stream loss");
        assert_eq!(worker.active_https_connection_count().await, 0);
        assert!(matches!(
            worker.send_https_payload("lost", b"packet").await,
            Err(TransportError::Unreachable {
                code: "no_active_https_channel"
            })
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reader_delivers_inbound_payload_from_owned_channel() {
        let state = new_state();
        {
            let mut g = state.write().await;
            let mut path = PeerPath::initial("payload".into(), TransportKind::NebulaDirect);
            path.https_state = mackes_transport::peer_path::HttpsFallbackState::Activating;
            g.insert("payload".into(), path);
        }
        let worker = MeshRouterWorker::new(Arc::clone(&state), https443_registry());
        assert_eq!(worker.drive_https_fallback_activations().await, 1);

        let payload = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(payload) = worker.try_recv_https_payload("payload").await {
                    break payload;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("reader queues inbound frame");
        assert_eq!(payload, b"inbound nebula packet");
        assert_eq!(worker.active_https_connection_count().await, 1);
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
