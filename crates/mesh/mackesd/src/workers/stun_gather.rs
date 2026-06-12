//! Phase 12.17 — STUN candidate-gather worker.
//!
//! Closes the v3.0.3 [!] Blocked entry for 12.17 wire by tying
//! the pure-fn `crate::stun::gather_endpoint` to a daemon-level
//! periodic tick that:
//!
//!   1. Reads the configured STUN server list (defaults to
//!      Google's public STUN cluster — see [`DEFAULT_SERVERS`]).
//!   2. Probes each server in parallel with a tight per-server
//!      timeout (Q8 budget: 1.5 s total, so each STUN attempt
//!      lives inside ~500 ms).
//!   3. Aggregates successes into `Vec<StunCandidate>`.
//!   4. Calls [`RouterState::with_each_peer`] to update every
//!      tracked peer's `PeerPath::candidates`. The mesh-router
//!      reads this on its next tick.
//!
//! Symmetric-NAT edges (~30 % of small-business fleets per Q15
//! / `v12-connectivity-scope.md`) only learn their reflexive
//! address through STUN — Tailscale's own NAT-traversal still
//! works without our help in most cases, but the explicit
//! gather lets the router shave the candidate-gathering tail off
//! first-packet latency.
//!
//! ## Acceptance (12.17, code-side)
//!
//! - On each tick, every peer in `RouterState` has its
//!   `candidates` set to the STUN gather result.
//! - When all STUN servers time out, candidates are cleared
//!   (operator sees "no STUN responses" via the audit log;
//!   the router falls back to direct/DERP without a stale
//!   candidate biasing the choice).
//! - The per-tick critical path stays inside the 1.5 s budget
//!   even when 3-of-3 STUN servers are slow.

#![cfg(feature = "async-services")]

use std::net::SocketAddr;
use std::time::{Duration, SystemTime};

use mackes_transport::peer_path::StunCandidate;
use tracing::{debug, info, warn};

use super::mesh_router::RouterState;
use super::{ShutdownToken, Worker};
use crate::stun::gather_endpoint;

/// Default STUN server pool. Per Q8 the gather must complete
/// in < 1.5 s so the overall handshake fits the 3 s first-packet
/// budget. Three globally-distributed Google servers — a
/// realistic baseline that survives the absence of operator
/// configuration. Operators can override via the future
/// `/etc/mde/connect/stun.toml`; for now this is the lock.
pub const DEFAULT_SERVERS: &[&str] = &[
    "74.125.250.129:19302", // stun.l.google.com (IP-pinned so the
    "142.250.27.127:19302", // worker doesn't hit DNS on the hot
    "142.251.32.127:19302", // path; DNS-aware override lands with
                            // the operator config file).
];

/// Per-server probe budget. The whole gather must finish under
/// 1.5 s (Q8); with 3 servers probed concurrently, each can
/// take up to ~1.4 s without breaching the budget. We give a
/// little slack so the worker's tokio scheduler overhead +
/// stack growth doesn't push us over.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(1_400);

/// Gather cadence. Per Q13 the connectivity workers use a
/// gentle exponential backoff; STUN starts at 30 s steady-state
/// so a healthy fleet doesn't burn bandwidth on broadcast
/// servers every 5 seconds. The gather worker doesn't model
/// backoff yet (KDC2-1.13 brings the per-worker backoff
/// scheduler); 30 s is a fine fixed cadence for now.
pub const GATHER_TICK: Duration = Duration::from_secs(30);

/// Async worker that periodically gathers STUN candidates +
/// publishes them onto each peer's `PeerPath::candidates`.
pub struct StunGatherWorker {
    state: RouterState,
    servers: Vec<SocketAddr>,
    tick: Duration,
    probe_timeout: Duration,
}

impl StunGatherWorker {
    /// Construct with the default server pool + 30 s cadence.
    /// The router state must be the same `Arc` the
    /// `mesh_router` worker holds — that's what makes the
    /// gather visible to the routing tick.
    #[must_use]
    pub fn new(state: RouterState) -> Self {
        let servers = DEFAULT_SERVERS
            .iter()
            .filter_map(|s| s.parse::<SocketAddr>().ok())
            .collect();
        Self {
            state,
            servers,
            tick: GATHER_TICK,
            probe_timeout: PROBE_TIMEOUT,
        }
    }

    /// Override the server list — used by integration tests
    /// that point at a loopback STUN responder.
    #[must_use]
    pub fn with_servers(mut self, servers: Vec<SocketAddr>) -> Self {
        self.servers = servers;
        self
    }

    /// Override the tick cadence — tests dial this down to
    /// 100 ms so the loop fires inside a single test run.
    #[must_use]
    pub fn with_tick(mut self, tick: Duration) -> Self {
        self.tick = tick;
        self
    }

    /// Override the per-server probe timeout — defaults to
    /// 1.4 s.
    #[must_use]
    pub fn with_probe_timeout(mut self, t: Duration) -> Self {
        self.probe_timeout = t;
        self
    }

    /// One gather pass: probe every configured server in
    /// parallel with the configured per-server timeout, return
    /// the successes as `StunCandidate`s. Public so the
    /// integration tests can exercise the gather logic without
    /// touching the worker loop.
    pub async fn gather_once(&self) -> Vec<StunCandidate> {
        let mut futs = Vec::with_capacity(self.servers.len());
        for server in &self.servers {
            let s = *server;
            let timeout = self.probe_timeout;
            futs.push(async move {
                let result = gather_endpoint(s, timeout).await;
                (s, result)
            });
        }
        // Polling all probes in parallel; we use futures::join_all
        // equivalent via a Vec drain. Tokio's `JoinSet` would be
        // more elegant but pulls a dependency feature we'd have
        // to enable workspace-wide.
        let mut handles: Vec<_> = futs.into_iter().map(tokio::spawn).collect();
        let mut out = Vec::new();
        for h in handles.drain(..) {
            if let Ok((server, Ok(candidate))) = h.await {
                out.push(StunCandidate {
                    reflexive: candidate.reflexive,
                    server,
                    observed_at: SystemTime::now(),
                });
            }
        }
        out
    }

    /// Run one gather pass + push the result onto every tracked
    /// peer's `PeerPath`. Public for tests; the worker `run`
    /// loop calls this on every tick.
    pub async fn tick_once(&self) -> usize {
        let candidates = self.gather_once().await;
        let n = candidates.len();
        if n == 0 {
            debug!(
                servers = self.servers.len(),
                "stun_gather: no responses; clearing peer candidates",
            );
        } else {
            debug!(
                servers = self.servers.len(),
                responses = n,
                "stun_gather: published reflexive candidates to all peers",
            );
        }
        let mut state = self.state.write().await;
        for path in state.values_mut() {
            path.set_candidates(candidates.clone());
        }
        n
    }
}

#[async_trait::async_trait]
impl Worker for StunGatherWorker {
    fn name(&self) -> &'static str {
        "stun-gather"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        info!(
            servers = self.servers.len(),
            tick_secs = self.tick.as_secs(),
            "stun_gather: started",
        );
        if self.servers.is_empty() {
            warn!("stun_gather: no STUN servers configured; worker idle until config arrives",);
        }
        let mut interval = tokio::time::interval(self.tick);
        loop {
            tokio::select! {
                _ = shutdown.wait() => {
                    info!("stun_gather: shutdown requested; exiting");
                    return Ok(());
                }
                _ = interval.tick() => {
                    if self.servers.is_empty() {
                        continue;
                    }
                    let _ = self.tick_once().await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_transport::TransportKind;
    use std::net::Ipv4Addr;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn make_state_with_peer(peer_id: &str) -> RouterState {
        let mut map = std::collections::HashMap::new();
        map.insert(
            peer_id.to_string(),
            mackes_transport::peer_path::PeerPath::initial(
                peer_id.to_string(),
                TransportKind::NebulaDirect,
            ),
        );
        Arc::new(RwLock::new(map))
    }

    #[tokio::test(flavor = "current_thread")]
    async fn gather_once_returns_empty_when_no_servers_respond() {
        // Point at a deliberately-refused address. Per-server
        // timeout fires; gather_once returns Vec::new().
        let state = make_state_with_peer("alice");
        let worker = StunGatherWorker::new(state)
            .with_servers(vec!["127.0.0.1:1".parse().unwrap()])
            .with_probe_timeout(Duration::from_millis(100));
        let candidates = worker.gather_once().await;
        assert!(candidates.is_empty(), "no STUN responses ⇒ no candidates");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tick_once_clears_candidates_when_no_servers_respond() {
        let state = make_state_with_peer("alice");
        // Seed the peer with stale candidates.
        {
            let mut g = state.write().await;
            let path = g.get_mut("alice").unwrap();
            path.set_candidates(vec![StunCandidate {
                reflexive: SocketAddr::new(Ipv4Addr::new(1, 2, 3, 4).into(), 5000),
                server: SocketAddr::new(Ipv4Addr::new(8, 8, 8, 8).into(), 19302),
                observed_at: SystemTime::UNIX_EPOCH,
            }]);
        }
        let worker = StunGatherWorker::new(Arc::clone(&state))
            .with_servers(vec!["127.0.0.1:1".parse().unwrap()])
            .with_probe_timeout(Duration::from_millis(100));
        let n = worker.tick_once().await;
        assert_eq!(n, 0);
        // Peer's candidate list should now be empty — stale
        // candidates were cleared.
        let g = state.read().await;
        let path = g.get("alice").unwrap();
        assert!(
            path.candidates.is_empty(),
            "stale candidates must be cleared"
        );
    }

    #[test]
    fn default_servers_are_parseable() {
        for s in DEFAULT_SERVERS {
            let _: SocketAddr = s.parse().expect("default STUN server parses");
        }
    }

    #[test]
    fn worker_name_is_stable() {
        let state = make_state_with_peer("alice");
        let w = StunGatherWorker::new(state);
        assert_eq!(w.name(), "stun-gather");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tick_once_publishes_candidates_to_every_peer() {
        // Loopback STUN responder: bind a UDP socket, on receipt
        // of a binding request emit a binding success with our
        // own address as the XOR-MAPPED-ADDRESS. This exercises
        // the gather → publish → PeerPath round-trip.
        use tokio::net::UdpSocket;
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server_addr = socket.local_addr().unwrap();
        let responder_addr = server_addr;
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            if let Ok((n, src)) = socket.recv_from(&mut buf).await {
                // Echo the txid back in a binding success with
                // an XOR-MAPPED-ADDRESS attribute pointing at
                // the sender (a stand-in for the reflexive
                // address; in real STUN this would be `src`).
                if n >= 20 {
                    let txid: [u8; 12] = buf[8..20].try_into().unwrap();
                    if let Some(resp) =
                        crate::stun::encode_binding_success_with_xor_mapped(txid, src)
                    {
                        let _ = socket.send_to(&resp, src).await;
                    }
                }
            }
        });

        let state = make_state_with_peer("alice");
        let worker = StunGatherWorker::new(Arc::clone(&state))
            .with_servers(vec![responder_addr])
            .with_probe_timeout(Duration::from_millis(500));
        let n = worker.tick_once().await;
        assert_eq!(n, 1, "loopback STUN must produce one candidate");
        let g = state.read().await;
        let path = g.get("alice").unwrap();
        assert_eq!(path.candidates.len(), 1);
        assert_eq!(path.candidates[0].server, responder_addr);
    }
}
