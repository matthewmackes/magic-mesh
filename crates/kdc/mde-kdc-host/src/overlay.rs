//! KDC-MESH-1 — the **overlay (Nebula-only) transport**.
//!
//! [`OverlayTransport`] is the mesh-native replacement for the LAN transport
//! ([`crate::lan::LanTransport`]): it speaks the exact same
//! [`Transport`]/[`Connection`] surface, but binds + dials **only** on the node's
//! Nebula **overlay IP** — never `0.0.0.0`, never the public NIC, never a LAN
//! broadcast. All KDE Connect traffic therefore rides the encrypted Nebula
//! overlay (design lock #3), which means the transport opens **no public port**
//! (design lock #15).
//!
//! Concretely, versus the LAN transport:
//! - **No UDP broadcast discovery.** Stock KDE Connect finds peers by
//!   UDP-broadcasting identity on 1716; Nebula doesn't carry broadcast, so this
//!   transport carries none. Peers/phones are reached by **directed** overlay IP
//!   (the directed-discovery layer, KDC-MESH-2, populates the [peer directory]).
//! - **Bind on the overlay IP.** The inbound TLS listener binds
//!   `<overlay-ip>:1716`, reusing the stock-KDE-Connect inbound handshake shared
//!   with the LAN transport ([`crate::lan::spawn_inbound_listener`]).
//! - **Honest gate.** If the overlay IP can't be resolved — the node isn't on the
//!   mesh yet, or the publish file is missing/empty/unparseable/wildcard — the
//!   transport is **unavailable**: [`start`](Transport::start) returns
//!   [`HostError::OverlayUnresolved`] and surfaces a typed
//!   [`OverlayStatus::Unresolved`] state; it does **not** fall back to a
//!   public/localhost bind (§7 — the same posture as QC-6 / `sshd_overlay_bind`).
//!
//! The overlay IP is resolved from the canonical publish file
//! [`DEFAULT_OVERLAY_IP_PATH`] — the same source
//! `mackesd::workers::sshd_overlay_bind` and `nebula_supervisor::publish_overlay_ip`
//! own — so KDC binds wherever the node's mesh identity actually lives.
//!
//! [peer directory]: OverlayTransport::peer_directory

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use tokio::sync::{oneshot, Mutex as AsyncMutex};
use tokio::task::JoinHandle;

use mde_kdc_proto::discovery::Announce;
use mde_kdc_proto::wire::Packet;

use crate::error::HostError;
use crate::event::{EventSink, HostEvent};
use crate::lan::{spawn_inbound_listener, LanConnection, KDC_TLS_PORT};
use crate::pairing::PairingStore;
use crate::tls;
use crate::transport::{Connection, Transport};
use crate::PeerId;

/// Canonical Nebula overlay-IP publish file.
///
/// Written by `mackesd::workers::nebula_supervisor::publish_overlay_ip` after
/// every CA bundle change and consumed by `sshd_overlay_bind` / QC-6. The overlay
/// transport resolves the node's own overlay IP from here (design #3/#15).
pub const DEFAULT_OVERLAY_IP_PATH: &str = "/var/lib/mackesd/nebula/overlay-ip";

/// Whether the node's Nebula overlay IP has been resolved.
///
/// The overlay transport binds + dials only when
/// [`Resolved`](OverlayStatus::Resolved); while
/// [`Unresolved`](OverlayStatus::Unresolved) it is honestly **unavailable** (no
/// bind at all — never a public/localhost fallback).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayStatus {
    /// The overlay IP resolved; the transport binds/dials on this address.
    Resolved(IpAddr),
    /// The overlay IP could not be resolved (node not on the mesh yet, or the
    /// publish file is missing/empty/unparseable/wildcard). Carries the reason.
    Unresolved(String),
}

impl OverlayStatus {
    /// The resolved overlay IP, or `None` while unresolved.
    #[must_use]
    pub const fn overlay_ip(&self) -> Option<IpAddr> {
        match self {
            Self::Resolved(ip) => Some(*ip),
            Self::Unresolved(_) => None,
        }
    }

    /// Whether the overlay IP is resolved (the transport is available).
    #[must_use]
    pub const fn is_resolved(&self) -> bool {
        matches!(self, Self::Resolved(_))
    }
}

/// Resolve the node's Nebula overlay IP from the publish file at `path`.
///
/// `path` is the QC-6 canonical source. Returns [`OverlayStatus::Unresolved`] —
/// never an error that coerces to a wildcard bind — when the file is missing
/// (pre-enrollment), empty (deferring), unparseable, the wildcard `0.0.0.0`/`::`
/// (which would open a public port), or loopback (the node isn't on the mesh).
#[must_use]
pub fn resolve_overlay_ip(path: &Path) -> OverlayStatus {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            return OverlayStatus::Unresolved(format!(
                "overlay-ip unreadable ({}): {e}",
                path.display()
            ))
        }
    };
    let text = raw.trim();
    if text.is_empty() {
        return OverlayStatus::Unresolved(format!("overlay-ip file empty: {}", path.display()));
    }
    match text.parse::<IpAddr>() {
        Ok(ip) if ip.is_unspecified() => OverlayStatus::Unresolved(format!(
            "overlay-ip is the wildcard {ip}; refusing to bind a public/0.0.0.0 address"
        )),
        Ok(ip) if ip.is_loopback() => OverlayStatus::Unresolved(format!(
            "overlay-ip is loopback {ip}; the node is not on the mesh yet"
        )),
        Ok(ip) => OverlayStatus::Resolved(ip),
        Err(e) => OverlayStatus::Unresolved(format!("overlay-ip {text:?} is not a valid IP: {e}")),
    }
}

/// Compute the inbound-listener bind address for a resolved overlay `status`.
///
/// Returns `<overlay-ip>:port`, or the human-readable reason it's unavailable. A
/// defensive final guard rejects the wildcard even if a `Resolved` status somehow
/// carries it — the overlay transport can **never** bind `0.0.0.0`/`::` (the
/// honest gate, §7).
///
/// # Errors
/// Returns the unavailability reason when the status is unresolved or wildcard.
pub fn overlay_bind_addr(status: &OverlayStatus, port: u16) -> Result<SocketAddr, String> {
    match status {
        OverlayStatus::Resolved(ip) if ip.is_unspecified() => Err(format!(
            "overlay-ip is the wildcard {ip}; refusing to bind a public/0.0.0.0 address"
        )),
        OverlayStatus::Resolved(ip) => Ok(SocketAddr::new(*ip, port)),
        OverlayStatus::Unresolved(reason) => Err(reason.clone()),
    }
}

/// The Nebula-overlay-only KDE Connect transport (KDC-MESH-1).
///
/// Binds the inbound TLS listener on the node's overlay IP and dials peers/phones
/// by their overlay IP (looked up in the [peer directory]) — overlay-only, no LAN
/// direct, no broadcast. Exposes the same `send_to` / `inbound_fingerprint` /
/// `local_listen_addr` surface the LAN transport did, so `kdc_outbound` and the
/// feature senders switch to it unchanged.
///
/// [peer directory]: Self::peer_directory
pub struct OverlayTransport {
    announce: Announce,
    pairing: Arc<PairingStore>,
    /// Where to read the node's overlay IP (default [`DEFAULT_OVERLAY_IP_PATH`]).
    overlay_ip_path: PathBuf,
    /// Explicit overlay IP override (tests / a caller that already resolved it),
    /// used verbatim instead of reading the publish file.
    overlay_ip_override: Option<IpAddr>,
    /// TCP port the inbound listener binds on the overlay IP. Default
    /// [`KDC_TLS_PORT`]; tests pass `0` for an ephemeral port.
    listen_port: u16,
    /// TCP port `open` dials on a peer's overlay IP. Default [`KDC_TLS_PORT`].
    dial_port: u16,
    /// `device_id` → overlay IP, populated by the directed-discovery layer
    /// (KDC-MESH-2, off the mesh-shunt roster). `open` resolves a peer's dial
    /// address here — there is no UDP broadcast to learn it from.
    peers: Arc<Mutex<HashMap<String, IpAddr>>>,
    /// The resolved overlay status, set by `start`. Unresolved until then.
    status: AsyncMutex<OverlayStatus>,
    /// The event sink captured in `start`, so `open`'s connection read loop emits
    /// onto the same stream.
    sink: AsyncMutex<Option<EventSink>>,
    /// The address the inbound listener actually bound (overlay IP + resolved
    /// port), or `None` when unavailable / not started / shut down.
    bound_addr: AsyncMutex<Option<SocketAddr>>,
    /// Accepted inbound connections keyed by peer id (one live link per peer),
    /// kept alive so `send_to` can write and their read loop keeps surfacing
    /// packets.
    inbound: Arc<AsyncMutex<HashMap<String, Box<dyn Connection>>>>,
    /// Cert fingerprints captured from completed inbound TLS handshakes
    /// (`device_id` → `SHA-256`), so the pairing flow can pin the exact cert a
    /// phone presented when its `kdeconnect.pair` request arrives.
    fingerprints: Arc<AsyncMutex<HashMap<String, String>>>,
    /// Fires on `shutdown` to stop the accept loop; its join handle is awaited.
    listen_shutdown: AsyncMutex<Option<oneshot::Sender<()>>>,
    listen_task: AsyncMutex<Option<JoinHandle<()>>>,
}

impl OverlayTransport {
    /// Build the overlay transport over the host pairing store, resolving the
    /// overlay IP from [`DEFAULT_OVERLAY_IP_PATH`] at `start`. Ports default to
    /// [`KDC_TLS_PORT`].
    #[must_use]
    pub fn new(announce: Announce, pairing: Arc<PairingStore>) -> Self {
        Self {
            announce,
            pairing,
            overlay_ip_path: PathBuf::from(DEFAULT_OVERLAY_IP_PATH),
            overlay_ip_override: None,
            listen_port: KDC_TLS_PORT,
            dial_port: KDC_TLS_PORT,
            peers: Arc::new(Mutex::new(HashMap::new())),
            status: AsyncMutex::new(OverlayStatus::Unresolved("not started".into())),
            sink: AsyncMutex::new(None),
            bound_addr: AsyncMutex::new(None),
            inbound: Arc::new(AsyncMutex::new(HashMap::new())),
            fingerprints: Arc::new(AsyncMutex::new(HashMap::new())),
            listen_shutdown: AsyncMutex::new(None),
            listen_task: AsyncMutex::new(None),
        }
    }

    /// Override the overlay-IP publish path (tests point it at a temp file).
    #[must_use]
    pub fn with_overlay_ip_path(mut self, path: PathBuf) -> Self {
        self.overlay_ip_path = path;
        self
    }

    /// Use `ip` as the overlay IP verbatim, bypassing the publish file — for a
    /// caller that already resolved it, or a test binding a fixture overlay IP
    /// (e.g. a loopback address standing in for the mesh address).
    #[must_use]
    pub const fn with_overlay_ip(mut self, ip: IpAddr) -> Self {
        self.overlay_ip_override = Some(ip);
        self
    }

    /// Override the inbound-listener port (tests pass `0` for an ephemeral port).
    #[must_use]
    pub const fn with_listen_port(mut self, port: u16) -> Self {
        self.listen_port = port;
        self
    }

    /// Override the TCP port `open` dials (tests point it at a loopback server).
    #[must_use]
    pub const fn with_dial_port(mut self, port: u16) -> Self {
        self.dial_port = port;
        self
    }

    /// A handle to the `device_id → overlay IP` directory. The directed-discovery
    /// layer (KDC-MESH-2) populates this off the mesh-shunt roster so `open` can
    /// dial a phone/peer by overlay IP.
    #[must_use]
    pub fn peer_directory(&self) -> Arc<Mutex<HashMap<String, IpAddr>>> {
        Arc::clone(&self.peers)
    }

    /// Record a peer's overlay IP directly (convenience over [`peer_directory`]).
    ///
    /// [`peer_directory`]: Self::peer_directory
    pub fn set_peer_overlay_ip(&self, device_id: &str, ip: IpAddr) {
        self.peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(device_id.to_string(), ip);
    }

    /// The current overlay status (resolved IP or the unavailability reason).
    pub async fn overlay_status(&self) -> OverlayStatus {
        self.status.lock().await.clone()
    }

    /// The address the inbound listener bound (overlay IP + resolved port), or
    /// `None` when the transport is unavailable / not started / shut down.
    pub async fn local_listen_addr(&self) -> Option<SocketAddr> {
        *self.bound_addr.lock().await
    }

    /// The TLS cert fingerprint captured for `device_id` during its most recent
    /// inbound handshake, or `None`. The pairing flow pins this when the peer's
    /// `kdeconnect.pair` request is accepted.
    pub async fn inbound_fingerprint(&self, device_id: &str) -> Option<String> {
        self.fingerprints.lock().await.get(device_id).cloned()
    }

    /// Send `packet` to a peer over its live **inbound** (peer-initiated) overlay
    /// connection — the drain path `kdc_outbound` uses.
    ///
    /// # Errors
    /// A [`HostError::Transport`] with the token `no_inbound_connection` if the
    /// peer has no live inbound link, or an underlying write error from the
    /// connection.
    pub async fn send_to(&self, peer: &PeerId, packet: Packet) -> Result<(), HostError> {
        let map = self.inbound.lock().await;
        match map.get(peer.as_str()) {
            Some(conn) => conn.send(packet).await,
            None => Err(HostError::Transport("no_inbound_connection".into())),
        }
    }

    /// The peers with a live inbound connection (observability + tests).
    pub async fn inbound_peers(&self) -> Vec<PeerId> {
        self.inbound
            .lock()
            .await
            .keys()
            .cloned()
            .map(PeerId::from)
            .collect()
    }
}

#[async_trait]
impl Transport for OverlayTransport {
    async fn start(&self, events: EventSink) -> Result<(), HostError> {
        // Resolve the node's overlay IP — the override (explicit / test) wins,
        // else read the canonical publish file.
        let status = self.overlay_ip_override.map_or_else(
            || resolve_overlay_ip(&self.overlay_ip_path),
            OverlayStatus::Resolved,
        );
        // Honest gate (§7): unresolved (or wildcard) → the transport is
        // UNAVAILABLE. Surface the typed state + a `TransportError`; never bind a
        // public / 0.0.0.0 / localhost fallback.
        let bind = match overlay_bind_addr(&status, self.listen_port) {
            Ok(addr) => addr,
            Err(reason) => {
                *self.status.lock().await = OverlayStatus::Unresolved(reason.clone());
                let _ = events.send(HostEvent::TransportError(format!(
                    "overlay_unresolved: {reason}"
                )));
                return Err(HostError::OverlayUnresolved(reason));
            }
        };
        *self.sink.lock().await = Some(events.clone());
        // Bind the inbound TLS listener on the overlay IP (shared handshake with
        // the LAN transport). A bind failure is fatal here — unlike the LAN
        // transport there is no outbound-only degraded mode; the whole point is
        // the overlay bind.
        let handles = spawn_inbound_listener(
            bind,
            Arc::new(self.announce.clone()),
            Arc::clone(&self.pairing),
            Arc::clone(&self.inbound),
            Arc::clone(&self.fingerprints),
            events,
        )
        .await?;
        // Record the actually-bound address (its IP is the overlay IP; port is
        // resolved if `listen_port` was 0).
        *self.status.lock().await = OverlayStatus::Resolved(handles.bound.ip());
        *self.bound_addr.lock().await = Some(handles.bound);
        *self.listen_shutdown.lock().await = Some(handles.stop_tx);
        *self.listen_task.lock().await = Some(handles.task);
        Ok(())
    }

    async fn open(&self, peer: &PeerId) -> Result<Box<dyn Connection>, HostError> {
        // Must be paired (we need the pinned fingerprint) and known to the peer
        // directory (we need an overlay IP to dial — no broadcast to learn it).
        let pin = {
            let device = self
                .pairing
                .get(peer.as_str())
                .ok_or_else(|| HostError::Transport("not_paired".into()))?;
            // Empty fingerprint = not yet pinned (first pair) → accept any cert;
            // a pinned fingerprint must match. `device` is owned, so move it out.
            if device.fingerprint.is_empty() {
                None
            } else {
                Some(device.fingerprint)
            }
        };
        let ip = self
            .peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(peer.as_str())
            .copied()
            .ok_or_else(|| HostError::Transport("not_discovered".into()))?;
        let dial = SocketAddr::new(ip, self.dial_port);
        let sink = self
            .sink
            .lock()
            .await
            .clone()
            .ok_or_else(|| HostError::Transport("overlay transport not started".into()))?;
        let stream = tls::connect_pinned_tls(dial, peer.as_str(), pin)
            .await
            .map_err(|e| HostError::Transport(format!("connect: {e}")))?;
        let _ = sink.send(HostEvent::Connected(peer.clone()));
        Ok(Box::new(LanConnection::new(stream, peer.clone(), sink)))
    }

    fn local_announce(&self) -> &Announce {
        &self.announce
    }

    async fn shutdown(&self) {
        // Take the guard-held values out first so the mutex guard is dropped
        // before the `if let` body (not held across the send/await).
        let stop = self.listen_shutdown.lock().await.take();
        if let Some(tx) = stop {
            let _ = tx.send(());
        }
        let task = self.listen_task.lock().await.take();
        if let Some(task) = task {
            let _ = task.await;
        }
        for (_id, conn) in self.inbound.lock().await.drain() {
            conn.close().await;
        }
        *self.bound_addr.lock().await = None;
        *self.sink.lock().await = None;
        *self.status.lock().await = OverlayStatus::Unresolved("shut down".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventStream;
    use crate::pairing::DeviceRecord;
    use crate::tls::compute_fingerprint;
    use mde_kdc_proto::discovery::{Announce, DeviceType};
    use mde_kdc_proto::plugins;
    use std::net::{Ipv4Addr, SocketAddr};
    use std::sync::Arc;
    use tokio::net::TcpListener;

    fn announce(id: &str) -> Announce {
        Announce {
            device_id: id.into(),
            device_name: format!("dev-{id}"),
            device_type: DeviceType::Desktop,
            protocol_version: 7,
            incoming_capabilities: vec![],
            outgoing_capabilities: vec![],
        }
    }

    fn store() -> (tempfile::TempDir, Arc<PairingStore>) {
        let tmp = tempfile::tempdir().unwrap();
        let store = PairingStore::open(tmp.path()).unwrap();
        (tmp, Arc::new(store))
    }

    // ── overlay-IP resolution (pure) ─────────────────────────────────────────

    #[test]
    fn resolve_overlay_ip_reads_a_valid_mesh_ip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("overlay-ip");
        std::fs::write(&path, "10.42.0.5\n").unwrap();
        assert_eq!(
            resolve_overlay_ip(&path),
            OverlayStatus::Resolved(IpAddr::V4(Ipv4Addr::new(10, 42, 0, 5)))
        );
    }

    #[test]
    fn resolve_overlay_ip_is_unresolved_when_missing_empty_or_garbage() {
        let tmp = tempfile::tempdir().unwrap();
        // Missing file (pre-enrollment).
        let missing = tmp.path().join("nope");
        assert!(matches!(
            resolve_overlay_ip(&missing),
            OverlayStatus::Unresolved(_)
        ));
        // Empty file (deferring).
        let empty = tmp.path().join("empty");
        std::fs::write(&empty, "\n").unwrap();
        assert!(matches!(
            resolve_overlay_ip(&empty),
            OverlayStatus::Unresolved(_)
        ));
        // Garbage.
        let garbage = tmp.path().join("garbage");
        std::fs::write(&garbage, "not-an-ip").unwrap();
        assert!(matches!(
            resolve_overlay_ip(&garbage),
            OverlayStatus::Unresolved(_)
        ));
    }

    #[test]
    fn resolve_overlay_ip_rejects_wildcard_and_loopback() {
        // The honest gate: a wildcard/loopback overlay-ip is treated as
        // unresolved — the transport must NEVER open a public/localhost bind.
        let tmp = tempfile::tempdir().unwrap();
        let wildcard = tmp.path().join("wildcard");
        std::fs::write(&wildcard, "0.0.0.0").unwrap();
        assert!(matches!(
            resolve_overlay_ip(&wildcard),
            OverlayStatus::Unresolved(_)
        ));
        let loopback = tmp.path().join("loopback");
        std::fs::write(&loopback, "127.0.0.1").unwrap();
        assert!(matches!(
            resolve_overlay_ip(&loopback),
            OverlayStatus::Unresolved(_)
        ));
    }

    #[test]
    fn overlay_bind_addr_is_the_overlay_ip_never_the_wildcard() {
        // The computed bind address is the overlay IP on the KDC port — for a
        // REAL mesh fixture (10.42.0.5), proving bind != 0.0.0.0/public.
        let mesh_ip = IpAddr::V4(Ipv4Addr::new(10, 42, 0, 5));
        let addr = overlay_bind_addr(&OverlayStatus::Resolved(mesh_ip), KDC_TLS_PORT)
            .expect("resolved overlay binds");
        assert_eq!(addr, SocketAddr::new(mesh_ip, 1716));
        assert_eq!(addr.ip(), mesh_ip);
        assert!(!addr.ip().is_unspecified(), "must never be 0.0.0.0/::");
        // Unresolved → no bind address, carries the reason.
        assert!(overlay_bind_addr(&OverlayStatus::Unresolved("x".into()), KDC_TLS_PORT).is_err());
        // Defensive: even a Resolved wildcard is refused.
        assert!(overlay_bind_addr(
            &OverlayStatus::Resolved(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            KDC_TLS_PORT
        )
        .is_err());
    }

    // ── start: bind on the overlay IP, honest unavailable gate ───────────────

    #[tokio::test]
    async fn start_binds_on_the_resolved_overlay_ip_never_wildcard() {
        // A fixture overlay IP (loopback here, standing in for the mesh address
        // so the OS actually binds it in a hermetic test): the listener binds
        // THAT IP, never 0.0.0.0. Port 0 → ephemeral, so no privilege needed.
        let (_tmp, pairing) = store();
        let fixture_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let transport = OverlayTransport::new(announce("self"), pairing)
            .with_overlay_ip(fixture_ip)
            .with_listen_port(0);
        let (sink, _stream) = EventStream::channel();
        transport.start(sink).await.expect("overlay start binds");

        let bound = transport
            .local_listen_addr()
            .await
            .expect("listener bound after start");
        assert_eq!(
            bound.ip(),
            fixture_ip,
            "bind addr = the resolved overlay IP"
        );
        assert!(
            !bound.ip().is_unspecified(),
            "the overlay transport must never bind 0.0.0.0/::"
        );
        assert_eq!(
            transport.overlay_status().await,
            OverlayStatus::Resolved(fixture_ip)
        );
        transport.shutdown().await;
        // After shutdown the transport is unavailable again.
        assert!(transport.local_listen_addr().await.is_none());
        assert!(!transport.overlay_status().await.is_resolved());
    }

    #[tokio::test]
    async fn start_is_unavailable_when_overlay_unresolved() {
        // No overlay IP (node not on the mesh): start returns the typed
        // OverlayUnresolved, surfaces a TransportError, and binds NOTHING —
        // never a public/localhost fallback.
        let (tmp, pairing) = store();
        let missing = tmp.path().join("no-overlay-ip");
        let transport = OverlayTransport::new(announce("self"), pairing)
            .with_overlay_ip_path(missing)
            .with_listen_port(0);
        let (sink, mut stream) = EventStream::channel();
        let err = transport
            .start(sink)
            .await
            .expect_err("must be unavailable");
        assert!(
            matches!(err, HostError::OverlayUnresolved(_)),
            "unresolved overlay is a typed unavailable state, got {err:?}"
        );
        // Nothing bound; status is unresolved.
        assert!(transport.local_listen_addr().await.is_none());
        assert!(matches!(
            transport.overlay_status().await,
            OverlayStatus::Unresolved(_)
        ));
        // A typed unavailability event was surfaced on the stream.
        let ev = stream.recv().await;
        assert!(
            matches!(&ev, Some(HostEvent::TransportError(m)) if m.contains("overlay_unresolved")),
            "expected an overlay_unresolved TransportError, got {ev:?}"
        );
    }

    // ── open: dial a peer by overlay IP ──────────────────────────────────────

    /// A one-shot TLS server presenting `cert`/`pkcs8` on a loopback port —
    /// stands in for a peer reachable at an overlay IP. Returns its addr.
    async fn spawn_peer_server(cert: Vec<u8>, pkcs8: Vec<u8>, sink: EventSink) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let config = crate::tls::build_server_config(&cert, &pkcs8).expect("server config");
            let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
            let tls = acceptor.accept(tcp).await.expect("server tls accept");
            let conn = LanConnection::new(tls, PeerId::from("client"), sink);
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            drop(conn);
        });
        addr
    }

    #[tokio::test]
    async fn open_dials_a_paired_peer_by_its_overlay_ip() {
        // The outbound overlay dial: a paired phone whose overlay IP is in the
        // directory is dialed at that IP; the returned connection's `send`
        // surfaces on the peer's stream. (Dial IP is loopback here so it binds
        // hermetically; the code path resolves it from the peer directory, not a
        // LAN broadcast.)
        let peer_pkcs8 = crate::keygen::generate_pkcs8().unwrap();
        let peer_cert = crate::keygen::issue_identity_cert(&peer_pkcs8, "phone-1").unwrap();
        let peer_fp = compute_fingerprint(&peer_cert);

        let (srv_sink, mut srv_stream) = EventStream::channel();
        let server_addr = spawn_peer_server(peer_cert, peer_pkcs8, srv_sink).await;

        let (_tmp, pairing) = store();
        pairing
            .pair(DeviceRecord {
                device_id: "phone-1".into(),
                device_name: "Phone".into(),
                paired_at_ms: 1,
                fingerprint: peer_fp,
            })
            .unwrap();

        let transport = OverlayTransport::new(announce("self"), pairing)
            .with_overlay_ip(IpAddr::V4(Ipv4Addr::LOCALHOST))
            .with_listen_port(0)
            .with_dial_port(server_addr.port());
        // The directed-discovery layer would populate this off the mesh roster;
        // here we inject the phone's overlay IP directly.
        transport.set_peer_overlay_ip("phone-1", server_addr.ip());

        let (host_sink, mut host_stream) = EventStream::channel();
        transport.start(host_sink).await.expect("overlay start");

        let conn = transport
            .open(&PeerId::from("phone-1"))
            .await
            .expect("open dials the paired peer at its overlay IP");
        assert_eq!(conn.peer().as_str(), "phone-1");

        let connected = tokio::time::timeout(std::time::Duration::from_secs(2), host_stream.recv())
            .await
            .expect("connected before timeout");
        assert!(matches!(connected, Some(HostEvent::Connected(p)) if p.as_str() == "phone-1"));

        conn.send(plugins::ping_packet(42, "hi".into()))
            .await
            .unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(2), srv_stream.recv())
            .await
            .expect("peer receives the ping before timeout");
        assert!(matches!(got, Some(HostEvent::Packet { packet, .. }) if packet.id == 42));

        transport.shutdown().await;
    }

    #[tokio::test]
    async fn open_errors_when_unpaired_or_overlay_ip_unknown() {
        let (_tmp, pairing) = store();
        let transport = OverlayTransport::new(announce("self"), pairing.clone())
            .with_overlay_ip(IpAddr::V4(Ipv4Addr::LOCALHOST))
            .with_listen_port(0);
        let (sink, _stream) = EventStream::channel();
        transport.start(sink).await.expect("overlay start");

        // Unpaired → not_paired.
        let r = transport.open(&PeerId::from("nobody")).await;
        assert!(matches!(r, Err(HostError::Transport(ref m)) if m.contains("not_paired")));

        // Paired but no overlay IP known (directed discovery hasn't run) →
        // not_discovered. Overlay-only: there is no broadcast to fall back on.
        pairing
            .pair(DeviceRecord {
                device_id: "phone-2".into(),
                device_name: "Phone2".into(),
                paired_at_ms: 1,
                fingerprint: "AB:CD".into(),
            })
            .unwrap();
        let r2 = transport.open(&PeerId::from("phone-2")).await;
        assert!(matches!(r2, Err(HostError::Transport(ref m)) if m.contains("not_discovered")));

        transport.shutdown().await;
    }

    #[tokio::test]
    async fn send_to_unconnected_peer_errors() {
        let (_tmp, pairing) = store();
        let transport = OverlayTransport::new(announce("self"), pairing)
            .with_overlay_ip(IpAddr::V4(Ipv4Addr::LOCALHOST))
            .with_listen_port(0);
        let (sink, _stream) = EventStream::channel();
        transport.start(sink).await.expect("overlay start");
        let r = transport
            .send_to(&PeerId::from("ghost"), plugins::ping_packet(1, "x".into()))
            .await;
        assert!(
            matches!(r, Err(HostError::Transport(ref m)) if m.contains("no_inbound_connection"))
        );
        transport.shutdown().await;
    }
}
