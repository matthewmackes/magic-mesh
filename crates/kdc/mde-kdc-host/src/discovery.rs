//! UDP/1716 LAN discovery — the discovery half of the live LAN transport.
//!
//! Stock KDE Connect announces identity by UDP-broadcasting a
//! `kdeconnect.identity` packet to port 1716; a peer that hears it learns the
//! sender's address and opens a TCP+TLS link back. This module is the host side
//! of that broadcast: [`UdpDiscovery`] binds the socket, periodically broadcasts
//! our own [`Announce`], and drains inbound announces into a
//! [`DiscoveryRegistry`], emitting [`HostEvent::PeerDiscovered`] for newcomers and
//! [`HostEvent::PeerLost`] as peers age out of the [`STALE_WINDOW_MS`] window.
//!
//! The protocol crate owns the wire format (`encode`/`decode_announce_datagram`)
//! and the registry; this module owns only the socket and the timing loop. The
//! registry also caches each peer's source address, which the TCP+TLS pairing
//! handshake (`Transport::open`, a later increment) will use to find where to
//! connect.

use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mde_kdc_proto::discovery::{
    decode_announce_datagram, encode_announce_datagram, Announce, DiscoveryRegistry, KDC_UDP_PORT,
};
use tokio::net::UdpSocket;
use tokio::sync::oneshot;

use crate::error::HostError;
use crate::event::{EventSink, HostEvent};
use crate::PeerId;

/// How often we re-broadcast our own identity while running. Well under the
/// protocol's `STALE_WINDOW_MS` (90s) so a steady peer never spuriously ages out.
pub const DEFAULT_ANNOUNCE_INTERVAL: Duration = Duration::from_secs(30);

/// How often we age out peers we've stopped hearing from.
const PRUNE_INTERVAL: Duration = Duration::from_secs(15);

/// Inbound datagram buffer. Anything larger than the protocol's
/// `MAX_BROADCAST_BYTES` (8 KiB) is rejected by the decoder anyway.
const RECV_BUF_BYTES: usize = 16 * 1024;

/// Current wall-clock as Unix milliseconds (the registry's freshness clock).
/// A pre-epoch clock (only possible if the system clock is badly wrong) maps to
/// 0, which simply makes everything look freshly received.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A live UDP/1716 discovery socket plus the cadence at which it re-announces.
pub struct UdpDiscovery {
    socket: UdpSocket,
    local: Announce,
    broadcast_dest: SocketAddr,
    announce_interval: Duration,
    /// The address cache, shared so the LAN transport can resolve a discovered
    /// peer's source address for the TCP+TLS connect ([`shared_registry`]). The
    /// `run` loop folds inbound announces into it; brief, non-async-held locks.
    ///
    /// [`shared_registry`]: Self::shared_registry
    registry: Arc<Mutex<DiscoveryRegistry>>,
}

impl UdpDiscovery {
    /// Bind the discovery socket on `listen` and advertise `local`. Enables
    /// `SO_BROADCAST` and defaults the broadcast destination to
    /// `255.255.255.255:1716`. Bind `0.0.0.0:KDC_UDP_PORT` in production; a
    /// hermetic test binds `127.0.0.1:0` and points [`with_broadcast_dest`] at a
    /// peer's loopback address.
    ///
    /// [`with_broadcast_dest`]: Self::with_broadcast_dest
    pub async fn bind(listen: SocketAddr, local: Announce) -> Result<Self, HostError> {
        let socket = UdpSocket::bind(listen).await?;
        socket.set_broadcast(true)?;
        Ok(Self {
            socket,
            local,
            broadcast_dest: SocketAddr::from((Ipv4Addr::BROADCAST, KDC_UDP_PORT)),
            announce_interval: DEFAULT_ANNOUNCE_INTERVAL,
            registry: Arc::new(Mutex::new(DiscoveryRegistry::new())),
        })
    }

    /// A handle to the shared address cache. Clone this **before** [`run`] consumes
    /// the discovery (e.g. into a `LanTransport`) so the transport can call
    /// [`peer_addr_in`] while `run` keeps folding announces into the same registry.
    ///
    /// [`run`]: Self::run
    /// [`peer_addr_in`]: Self::peer_addr_in
    #[must_use]
    pub fn shared_registry(&self) -> Arc<Mutex<DiscoveryRegistry>> {
        Arc::clone(&self.registry)
    }

    /// The cached source address of a discovered peer (the IP a TLS connect dials),
    /// looked up in a shared registry handle. `None` until the peer has been heard
    /// from. Free fn over the handle so the transport (which holds only the handle,
    /// not the `UdpDiscovery`) can resolve addresses after `run` is spawned.
    #[must_use]
    pub fn peer_addr_in(
        registry: &Arc<Mutex<DiscoveryRegistry>>,
        device_id: &str,
    ) -> Option<SocketAddr> {
        registry
            .lock()
            .expect("discovery registry mutex poisoned")
            .source_addr_for(device_id)
    }

    /// Override the broadcast destination. Production uses the default LAN
    /// broadcast; tests unicast to a peer's loopback address.
    #[must_use]
    pub fn with_broadcast_dest(mut self, dest: SocketAddr) -> Self {
        self.broadcast_dest = dest;
        self
    }

    /// Override the re-announce cadence (tests use a short interval).
    #[must_use]
    pub fn with_announce_interval(mut self, interval: Duration) -> Self {
        self.announce_interval = interval;
        self
    }

    /// The address the socket is actually bound to (resolves an ephemeral `:0`).
    pub fn local_addr(&self) -> Result<SocketAddr, HostError> {
        Ok(self.socket.local_addr()?)
    }

    /// Broadcast our identity once.
    pub async fn announce(&self) -> Result<(), HostError> {
        let datagram = encode_announce_datagram(&self.local, now_ms())
            .map_err(|e| HostError::Transport(format!("encode announce: {e}")))?;
        self.socket.send_to(&datagram, self.broadcast_dest).await?;
        Ok(())
    }

    /// Run the discovery loop until `shutdown` fires (or its sender drops):
    /// broadcast our identity immediately and every `announce_interval`, fold
    /// inbound announces into the registry (emitting `PeerDiscovered` for each new
    /// device id), and prune stale peers every [`PRUNE_INTERVAL`] (emitting
    /// `PeerLost`). Datagrams that don't decode as a `kdeconnect.identity` announce
    /// — the port also sees unrelated LAN traffic — are silently dropped; a
    /// socket-level receive error ends the loop after emitting `TransportError`.
    pub async fn run(self, sink: EventSink, mut shutdown: oneshot::Receiver<()>) {
        // The address cache is the shared registry (so a transport holding a
        // `shared_registry` handle sees the same announces). Locked briefly per
        // event; never held across an await.
        let registry = Arc::clone(&self.registry);
        // Edge-trigger mirror: the set of device ids we've emitted `PeerDiscovered`
        // for and not yet `PeerLost`. Kept beside the registry (which exists for
        // its address cache) so repeat announces don't re-fire `PeerDiscovered`.
        let mut known: HashSet<String> = HashSet::new();
        let mut buf = vec![0u8; RECV_BUF_BYTES];
        // Both intervals fire their first tick immediately: an instant initial
        // announce, and a harmless prune of the empty registry.
        let mut announce_tick = tokio::time::interval(self.announce_interval);
        let mut prune_tick = tokio::time::interval(PRUNE_INTERVAL);

        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                _ = announce_tick.tick() => {
                    if let Err(e) = self.announce().await {
                        let _ = sink.send(HostEvent::TransportError(e.to_string()));
                    }
                }
                _ = prune_tick.tick() => {
                    let mut reg = registry.lock().expect("discovery registry mutex poisoned");
                    prune_and_emit_lost(&mut reg, &mut known, &sink, now_ms());
                }
                recv = self.socket.recv_from(&mut buf) => match recv {
                    Ok((n, src)) => {
                        let mut reg = registry.lock().expect("discovery registry mutex poisoned");
                        ingest(
                            &mut reg,
                            &mut known,
                            &sink,
                            &buf[..n],
                            src,
                            &self.local.device_id,
                            now_ms(),
                        );
                    }
                    Err(e) => {
                        let _ = sink.send(HostEvent::TransportError(format!("udp recv: {e}")));
                        break;
                    }
                },
            }
        }
    }
}

/// Fold one inbound datagram into the registry. Emits `PeerDiscovered` only the
/// first time a device id appears (a re-announce from a known peer just refreshes
/// its timestamp + cached address). Our own echoed broadcast and undecodable
/// datagrams are ignored. Split out from the socket loop so it's unit-testable
/// without a socket or a clock.
fn ingest(
    registry: &mut DiscoveryRegistry,
    known: &mut HashSet<String>,
    sink: &EventSink,
    bytes: &[u8],
    src: SocketAddr,
    local_id: &str,
    now_ms: i64,
) {
    let Ok(announce) = decode_announce_datagram(bytes) else {
        return; // not a kdeconnect.identity packet — ignore the noise
    };
    if announce.device_id == local_id {
        return; // our own broadcast, heard on the same subnet
    }
    let is_new = known.insert(announce.device_id.clone());
    registry.inject_real_with_addr(announce.clone(), now_ms, src);
    if is_new {
        let _ = sink.send(HostEvent::PeerDiscovered(announce));
    }
}

/// Drop peers that have aged past `STALE_WINDOW_MS` and emit `PeerLost` for each.
/// Split out so the aging edge is unit-testable with an injected clock.
fn prune_and_emit_lost(
    registry: &mut DiscoveryRegistry,
    known: &mut HashSet<String>,
    sink: &EventSink,
    now_ms: i64,
) {
    registry.prune_stale(now_ms);
    let fresh: HashSet<String> = registry
        .take_fresh(now_ms)
        .into_iter()
        .map(|a| a.device_id)
        .collect();
    let lost: Vec<String> = known.difference(&fresh).cloned().collect();
    for id in lost {
        known.remove(&id);
        let _ = sink.send(HostEvent::PeerLost(PeerId::from(id)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_kdc_proto::discovery::{DeviceType, STALE_WINDOW_MS};
    use tokio::sync::mpsc;

    fn announce(id: &str) -> Announce {
        Announce {
            device_id: id.into(),
            device_name: format!("dev-{id}"),
            device_type: DeviceType::Phone,
            protocol_version: 7,
            incoming_capabilities: vec![],
            outgoing_capabilities: vec![],
        }
    }

    fn datagram(id: &str) -> Vec<u8> {
        encode_announce_datagram(&announce(id), 1).unwrap()
    }

    fn src() -> SocketAddr {
        SocketAddr::from(([192, 168, 1, 42], KDC_UDP_PORT))
    }

    #[test]
    fn ingest_emits_discovered_once_then_dedups_and_caches_addr() {
        let (sink, mut rx) = mpsc::unbounded_channel();
        let mut reg = DiscoveryRegistry::new();
        let mut known = HashSet::new();

        ingest(
            &mut reg,
            &mut known,
            &sink,
            &datagram("phone-1"),
            src(),
            "self",
            1000,
        );
        ingest(
            &mut reg,
            &mut known,
            &sink,
            &datagram("phone-1"),
            src(),
            "self",
            1500,
        );

        // Exactly one PeerDiscovered for the device, despite two announces.
        match rx.try_recv() {
            Ok(HostEvent::PeerDiscovered(a)) => assert_eq!(a.device_id, "phone-1"),
            other => panic!("expected PeerDiscovered, got {other:?}"),
        }
        assert!(rx.try_recv().is_err(), "second announce must not re-emit");
        // The source address is cached for the future TCP connect.
        assert_eq!(reg.source_addr_for("phone-1"), Some(src()));
    }

    #[test]
    fn shared_registry_handle_resolves_peer_addr_after_ingest() {
        // The transport holds a `shared_registry` handle and resolves a discovered
        // peer's dial address through `peer_addr_in` — the seam `LanTransport::open`
        // (3b.2) uses. Drive an ingest through the handle and read it back.
        let (sink, _rx) = mpsc::unbounded_channel();
        let registry = std::sync::Arc::new(std::sync::Mutex::new(DiscoveryRegistry::new()));
        let mut known = HashSet::new();
        {
            let mut reg = registry.lock().unwrap();
            ingest(
                &mut reg,
                &mut known,
                &sink,
                &datagram("phone-1"),
                src(),
                "self",
                1000,
            );
        }
        assert_eq!(
            UdpDiscovery::peer_addr_in(&registry, "phone-1"),
            Some(src())
        );
        assert_eq!(UdpDiscovery::peer_addr_in(&registry, "unknown"), None);
    }

    #[test]
    fn ingest_skips_our_own_echoed_broadcast() {
        let (sink, mut rx) = mpsc::unbounded_channel();
        let mut reg = DiscoveryRegistry::new();
        let mut known = HashSet::new();

        ingest(
            &mut reg,
            &mut known,
            &sink,
            &datagram("self"),
            src(),
            "self",
            1000,
        );

        assert!(rx.try_recv().is_err(), "own announce must not emit");
        assert!(reg.is_empty());
        assert!(known.is_empty());
    }

    #[test]
    fn ingest_ignores_undecodable_datagram() {
        let (sink, mut rx) = mpsc::unbounded_channel();
        let mut reg = DiscoveryRegistry::new();
        let mut known = HashSet::new();

        ingest(
            &mut reg,
            &mut known,
            &sink,
            b"not a kdeconnect packet",
            src(),
            "self",
            1000,
        );

        assert!(rx.try_recv().is_err());
        assert!(reg.is_empty());
    }

    #[test]
    fn prune_emits_lost_when_peer_ages_out() {
        let (sink, mut rx) = mpsc::unbounded_channel();
        let mut reg = DiscoveryRegistry::new();
        let mut known = HashSet::new();

        ingest(
            &mut reg,
            &mut known,
            &sink,
            &datagram("phone-1"),
            src(),
            "self",
            1000,
        );
        let _ = rx.try_recv(); // drain the PeerDiscovered

        // Still fresh one tick later: no PeerLost.
        prune_and_emit_lost(&mut reg, &mut known, &sink, 2000);
        assert!(rx.try_recv().is_err());
        assert!(known.contains("phone-1"));

        // Past the stale window: PeerLost, and the mirror forgets it.
        prune_and_emit_lost(&mut reg, &mut known, &sink, 1000 + STALE_WINDOW_MS + 1);
        match rx.try_recv() {
            Ok(HostEvent::PeerLost(p)) => assert_eq!(p.as_str(), "phone-1"),
            other => panic!("expected PeerLost, got {other:?}"),
        }
        assert!(known.is_empty());
        assert!(reg.is_empty());
    }

    #[tokio::test]
    async fn run_round_trips_discovery_over_loopback() {
        let lo = Ipv4Addr::LOCALHOST;
        // Two discovery sockets on ephemeral loopback ports.
        let a = UdpDiscovery::bind(SocketAddr::from((lo, 0)), announce("dev-a"))
            .await
            .unwrap();
        let b = UdpDiscovery::bind(SocketAddr::from((lo, 0)), announce("dev-b"))
            .await
            .unwrap();
        let a_addr = a.local_addr().unwrap();
        let b_addr = b.local_addr().unwrap();
        // Cross-point each at the other and announce rapidly.
        let a = a
            .with_broadcast_dest(b_addr)
            .with_announce_interval(Duration::from_millis(20));
        let b = b
            .with_broadcast_dest(a_addr)
            .with_announce_interval(Duration::from_millis(20));

        let (a_sink, mut a_rx) = mpsc::unbounded_channel();
        let (b_sink, mut b_rx) = mpsc::unbounded_channel();
        let (a_stop_tx, a_stop_rx) = oneshot::channel();
        let (b_stop_tx, b_stop_rx) = oneshot::channel();
        let a_task = tokio::spawn(a.run(a_sink, a_stop_rx));
        let b_task = tokio::spawn(b.run(b_sink, b_stop_rx));

        // Each side discovers the other within a few announce cycles.
        let got_a = tokio::time::timeout(Duration::from_secs(2), a_rx.recv())
            .await
            .expect("a should discover b before timeout");
        assert!(matches!(got_a, Some(HostEvent::PeerDiscovered(p)) if p.device_id == "dev-b"));
        let got_b = tokio::time::timeout(Duration::from_secs(2), b_rx.recv())
            .await
            .expect("b should discover a before timeout");
        assert!(matches!(got_b, Some(HostEvent::PeerDiscovered(p)) if p.device_id == "dev-a"));

        // Shutdown is honored: both loops exit when their sender fires.
        a_stop_tx.send(()).unwrap();
        b_stop_tx.send(()).unwrap();
        a_task.await.unwrap();
        b_task.await.unwrap();
    }
}
