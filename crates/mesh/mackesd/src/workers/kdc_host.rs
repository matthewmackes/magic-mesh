//! KDC2-3.10 — the KDE Connect host registered as a `mackesd` worker.
//!
//! Owns the `Arc<Mutex<PairingStore>>` + the operator-facing **Connect**
//! surface over the Bus (`action/connect/<verb>`: version / list / get /
//! pair / unpair / ring / sms / clipboard) + the pending-sends queue.
//!
//! **E2.2 (2026-06-05) — KDC host convergence (complete).**
//! *Step 1* dropped the held-but-unused `mde_kdc::transport::KdcHost`
//! orchestrator + `mde_kdc_proto::discovery::DiscoveryRegistry`
//! scaffolding (nothing consumed the `host()`/`discovery()` accessors —
//! §3 dead code for never-built workers).
//! *Step 2* retired the legacy `mde_kdc::dbus::DbusServer`
//! (`dev.mackes.MDE.Connect` D-Bus) in favour of a **Bus responder**
//! ([`serve_connect_bus`] + the pure [`handle_connect_verb`]) over
//! `action/connect/<verb>` request → `reply/<ulid>`, per the
//! EPIC-RETIRE-DBUS lock — which also advanced E0.3.7's D-Bus sweep.
//! *Step 3 (this file)* converged off the legacy `mde-kdc` host entirely:
//! the store is the canonical [`mde_kdc_host::pairing::PairingStore`]
//! (one store across the monorepo — folds into E2.3), the outbound queue
//! is a small worker-local [`PendingSends`] over the canonical
//! `mde_kdc_proto::wire::Packet`, and the plugin-dispatch policy trait now
//! lives in the canonical `mde_kdc_proto::dispatch`. With that the legacy
//! `crates/legacy/mde-kdc{,-proto}` path-deps are gone and `cargo tree`
//! shows one KDE Connect host (E2.2 acceptance #1/#2). **AUD-2 (2026-06-11):**
//! the outbound drainer is live — `run_host` drains the `PendingSends` queue
//! over the `LanTransport` once a second, so ring/sms/clipboard/share actually
//! reach a paired device (end-to-end byte delivery is the 2-device bench).

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use mde_kdc_host::error::HostError;
use mde_kdc_host::pairing::{DeviceRecord, PairingStore};
use mde_kdc_host::{EventStream, HostEvent, LanTransport, PeerId, Transport, UdpDiscovery};
use mde_kdc_proto::discovery::{Announce, DeviceType};
use mde_kdc_proto::plugins::battery::BatteryBody;
use serde_json::{json, Value};
use tracing::{debug, error, info, warn};

use super::{ShutdownToken, Worker};

/// The Connect verbs served over `action/connect/<verb>` (E2.2 — replacing
/// the retired `dev.mackes.MDE.Connect` D-Bus surface). `version`/`list`/
/// `get` read the store; `pair`/`unpair` mutate it; `ring`/`sms`/
/// `clipboard` enqueue a `Packet` onto the outbound queue.
const CONNECT_VERBS: [&str; 10] = [
    "version",
    "list",
    "get",
    "pair",
    "pair-device",
    "unpair",
    "ring",
    "sms",
    "clipboard",
    // PD-3/L6 — the Peers "Devices" group's Send-file action enqueues a
    // `kdeconnect.share.request` (file or URL/text) onto the outbound queue,
    // same delivery path as ring/sms/clipboard.
    "share",
];

/// Bus topic the worker answers with the live device roster (E2.3 — the same
/// topic the Connect clients (the Workbench panel) already query via
/// `connect::devices()`). Distinct from the `<verb>` action topics.
const DEVICES_TOPIC: &str = "action/connect/devices";

/// KDE Connect's stock UDP/TCP port (identity broadcast + TLS link).
const KDC_PORT: u16 = 1716;

/// Poll cadence for the Connect action topics (operator-scale — clicks).
const CONNECT_POLL: Duration = Duration::from_millis(400);

/// AUD-2 — how often the host drains the outbound queue over the live
/// transport. 1 s keeps a clicked Ring/Send-file imperceptibly prompt.
const OUTBOUND_DRAIN: Duration = Duration::from_secs(1);

/// Health-tick cadence. 30s is the same window
/// `lan_discovery` uses for its idle scan.
const TICK: Duration = Duration::from_secs(30);

// ─────────────────────────────────────────────────────────────────────────────
// Worker-local outbound queue
//
// E2.2 — the canonical `mde-kdc-host` is the lower-level host (LAN transport,
// pairing, TLS) and owns no operator-action send queue, so the queue the
// retired legacy `mde_kdc::outbound` provided lives here, over the canonical
// `mde_kdc_proto::wire::Packet`. Intentionally simple — a `Mutex<Vec<...>>` —
// because the throughput target is operator-scale (clicks per minute). The
// `ring`/`sms`/`clipboard` verbs push here; a future `kdc_outbound` worker (or
// the `LanTransport::send_to` path at the 2-device bench) drains it.
// ─────────────────────────────────────────────────────────────────────────────

/// One pending outbound send: a built `Packet` addressed to a paired device id.
#[derive(Debug, Clone, PartialEq)]
struct OutboundSend {
    /// Paired-device id (KDC UUID) — picks the per-peer transport at drain.
    device_id: String,
    /// Already-built `Packet` (type-tagged + body-serialized).
    packet: mde_kdc_proto::wire::Packet,
}

/// Shared outbound queue handle. Cloneable cheaply via `Arc`; the Bus
/// responder pushes, the future drainer takes.
#[derive(Debug, Clone, Default)]
struct PendingSends {
    inner: Arc<Mutex<Vec<OutboundSend>>>,
}

impl PendingSends {
    /// Empty queue.
    fn new() -> Self {
        Self::default()
    }

    /// Enqueue one outbound send. Poison-tolerant (operator-scale, single op).
    fn push(&self, send: OutboundSend) {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(send);
    }

    /// AUD-2 — drain the whole backlog (the `kdc_outbound` drainer takes these
    /// + delivers each over the live `LanTransport`). Poison-tolerant.
    fn take_all(&self) -> Vec<OutboundSend> {
        std::mem::take(&mut *self.inner.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Current backlog length. O(1).
    fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

/// Async worker that owns the KDC host objects.
pub struct KdcHostWorker {
    config_dir: PathBuf,
    /// Shared outbound queue. The Connect Bus responder pushes
    /// here; the future `kdc_outbound` worker drains.
    outbound: PendingSends,
    /// Stop flag for the `action/connect/*` Bus responder thread.
    responder_stop: Arc<AtomicBool>,
}

impl KdcHostWorker {
    /// Construct with the on-disk config directory. The host
    /// itself is constructed lazily inside `run()` so a failed
    /// keygen / load doesn't abort the daemon startup — the
    /// supervisor sees a worker error + restarts according to
    /// `restart_policy`.
    #[must_use]
    pub fn new(config_dir: PathBuf) -> Self {
        Self {
            config_dir,
            outbound: PendingSends::new(),
            responder_stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Open the on-disk pairing store (creating the identity on first
    /// run). Idempotent + cheap once `identity.pkcs8` exists, so `run`
    /// can call it freely after a restart. A single `Arc<PairingStore>`
    /// is shared (E2.3): the canonical store is interior-mutable, so the
    /// verb responder pairs/unpairs through the same `&self` the read-only
    /// LAN host holds — no outer lock, one authoritative store.
    fn open_pairing(&self) -> Result<Arc<PairingStore>, HostError> {
        Ok(Arc::new(PairingStore::open(&self.config_dir)?))
    }
}

/// Build an outbound `Packet` from a kind token + body (id = wall-clock
/// ms, the receiver's dual-send dedupe key).
fn build_packet(kind: &str, body: Value) -> mde_kdc_proto::wire::Packet {
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    mde_kdc_proto::wire::Packet {
        id,
        kind: kind.to_string(),
        body,
        ..Default::default()
    }
}

/// A paired device as the Bus reply renders it. The canonical
/// [`DeviceRecord`] persists the id, friendly name, first-pair timestamp, and
/// the pinned TLS cert fingerprint (the wire/capability fields the legacy
/// store carried are not persisted by the canonical store).
fn device_json(d: &DeviceRecord) -> Value {
    json!({
        "id": d.device_id,
        "name": d.device_name,
        "fingerprint": d.fingerprint,
        "paired_at": d.paired_at_ms,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Live device roster (E2.3 — the host that was the shell's `mde connect` daemon)
//
// The worker runs the canonical `LanTransport` (UDP discovery + the inbound TLS
// listener) and folds its `HostEvent`s into this roster — online/battery/name —
// which it publishes on `action/connect/devices`. The shell surfaces become
// pure clients of the daemon's roster (one host, owned + supervised by mackesd).
// ─────────────────────────────────────────────────────────────────────────────

/// One paired peer as the surfaces see it (the published roster row).
#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceInfo {
    id: String,
    name: String,
    online: bool,
    battery: Option<u8>,
}

impl DeviceInfo {
    fn unknown(id: &str) -> Self {
        Self {
            id: id.to_string(),
            name: id.to_string(),
            online: false,
            battery: None,
        }
    }
}

/// The shared roster the host task writes and the Bus responder reads.
type Roster = Arc<Mutex<HashMap<String, DeviceInfo>>>;

/// NOTIFY-SRC-3 — map a KDC host event to an Alert Center `(summary, severity)`,
/// or `None` to skip noisy/uninteresting events (discovery refreshes, peer-lost,
/// transport errors). Pure + testable. These flow to `fdo/KDE Connect` so KDE
/// Connect device events (pair, file share, find-my-device, phone notifications,
/// low battery, connect/disconnect) reach the global Alert Center + federate.
fn kdc_event_alert(ev: &HostEvent) -> Option<(String, &'static str)> {
    match ev {
        // KDC-NOISE-1 — connect/disconnect is presence churn (it flaps with the
        // KDE-Connect handshake), already reflected in the device roster; don't
        // flood the Alert Center with an info event per flap. Only genuinely
        // notable device events below reach the Alert Center.
        HostEvent::Connected(_) | HostEvent::Disconnected(_) => None,
        HostEvent::Packet { peer, packet } => {
            let who = peer.as_str();
            match packet.kind.as_str() {
                "kdeconnect.pair" => (packet.body.get("pair").and_then(Value::as_bool)
                    == Some(true))
                .then(|| (format!("{who} paired"), "info")),
                "kdeconnect.share.request" => Some((format!("{who} shared a file"), "info")),
                "kdeconnect.findmyphone.request" => {
                    Some((format!("Find-my-device from {who}"), "warn"))
                }
                // KDC-NOISE-1 — a bare ping isn't a notable device event.
                "kdeconnect.ping" => None,
                "kdeconnect.notification" => {
                    if packet.body.get("isCancel").and_then(Value::as_bool) == Some(true) {
                        return None;
                    }
                    let app = packet
                        .body
                        .get("appName")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let text = packet
                        .body
                        .get("ticker")
                        .or_else(|| packet.body.get("text"))
                        .or_else(|| packet.body.get("title"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let sep = if app.is_empty() || text.is_empty() {
                        ""
                    } else {
                        ": "
                    };
                    let s = format!("{app}{sep}{text}");
                    (!s.is_empty()).then_some((s, "info"))
                }
                "kdeconnect.battery" => {
                    let charge = packet
                        .body
                        .get("currentCharge")
                        .and_then(Value::as_i64)
                        .unwrap_or(-1);
                    let threshold =
                        packet.body.get("thresholdEvent").and_then(Value::as_i64) == Some(1);
                    (threshold || (0..=15).contains(&charge))
                        .then(|| (format!("{who} battery low ({charge}%)"), "warn"))
                }
                _ => None,
            }
        }
        // Discovery refreshes repeat every announce (too noisy); peer-lost +
        // transport errors aren't device-facing alerts.
        HostEvent::PeerDiscovered(_) | HostEvent::PeerLost(_) | HostEvent::TransportError(_) => {
            None
        }
    }
}

/// NOTIFY-SRC-3 — publish a KDC device event to the bus alert lane
/// `fdo/KDE Connect` (the panel's DesktopApp group); the `alert-mirror` worker
/// federates it mesh-wide. Best-effort (open+write+drop; `Persist` is `!Send`).
fn publish_kdc_alert(summary: &str, severity: &str) {
    let Some(dir) = mde_bus::default_data_dir() else {
        return;
    };
    let Ok(persist) = Persist::open(dir) else {
        return;
    };
    let host = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".to_string());
    let body = json!({
        "appName": "KDE Connect",
        "title": "KDE Connect",
        "summary": summary,
        "severity": severity,
        "host": host,
    })
    .to_string();
    let prio = if severity == "warn" {
        Priority::High
    } else {
        Priority::Default
    };
    let _ = persist.write("fdo/KDE Connect", prio, Some("KDE Connect"), Some(&body));
}

/// Fold one host event into the roster: connections flip `online`, battery
/// packets update the charge, discovery announces refresh the display name.
/// Pure (no I/O) so the state machine is unit-tested without a bus or a phone.
fn apply_event(map: &mut HashMap<String, DeviceInfo>, ev: HostEvent) {
    match ev {
        HostEvent::Connected(p) => {
            map.entry(p.0.clone())
                .or_insert_with(|| DeviceInfo::unknown(&p.0))
                .online = true;
        }
        HostEvent::Disconnected(p) => {
            if let Some(d) = map.get_mut(p.as_str()) {
                d.online = false;
            }
        }
        HostEvent::PeerDiscovered(a) => {
            let e = map
                .entry(a.device_id.clone())
                .or_insert_with(|| DeviceInfo::unknown(&a.device_id));
            if !a.device_name.is_empty() {
                e.name = a.device_name;
            }
        }
        // A discovered peer ageing out doesn't drop it from the paired roster;
        // it's simply no longer broadcasting (`online` tracks the live link).
        HostEvent::PeerLost(_) => {}
        HostEvent::Packet { peer, packet } => {
            if packet.kind == "kdeconnect.battery" {
                if let Ok(b) = serde_json::from_value::<BatteryBody>(packet.body) {
                    if let Some(d) = map.get_mut(peer.as_str()) {
                        d.battery = b.charge_pct();
                    }
                }
            }
        }
        HostEvent::TransportError(_) => {}
    }
}

/// The display-name prefix every mesh host advertises over KDE Connect, so a
/// paired phone groups them together (e.g. `MDE-MESH UNIT-EAGLE`). Prefixes the
/// `device_name` only — the `device_id` stays the stable machine-id so an
/// existing pairing is never broken by a name change.
const MESH_NAME_PREFIX: &str = "MDE-MESH";

/// This host's identity announce: `device_id` the machine id (stable across
/// boots), `device_name` the `MDE-MESH`-prefixed hostname; type Desktop,
/// protocol 7.
fn local_announce() -> Announce {
    let device_id = std::fs::read_to_string("/etc/machine-id")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "mde-host".to_string());
    let hostname = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "host".to_string());
    let device_name = format!("{MESH_NAME_PREFIX} {hostname}");
    // KDC-INTEROP / KDC-PLUGINS — advertise ONLY the plugin capabilities we
    // actually drive. An identity with EMPTY capabilities makes a stock KDE
    // Connect peer treat the link as having nothing to run and tear it down
    // right after the handshake (observed: a paired phone reconnecting every
    // ~30s, never persistent, no features). But advertising packets we DON'T
    // handle is the false advertising the KDC-PLUGINS epic removes — so
    // `mpris{,.request}` and `notification.request` (no inbound handler:
    // media control + notification-pull are not implemented) are dropped.
    // `incoming` = packets we accept + act on; `outgoing` = packets we send.
    let incoming_capabilities = [
        // Liveness — surfaced to the Alert Center.
        "kdeconnect.ping",
        // Peer battery snapshot (folded into the roster); the request kind is
        // answered with THIS host's battery (handle_battery_request).
        "kdeconnect.battery",
        "kdeconnect.battery.request",
        // Clipboard copy + the connection-time push, applied via wl-copy.
        "kdeconnect.clipboard",
        "kdeconnect.clipboard.connect",
        // Peer notifications, mirrored to the Alert Center.
        "kdeconnect.notification",
        // Inbound file/URL share — surfaced to the Alert Center.
        "kdeconnect.share.request",
        // Ring this host (ring_local_device).
        "kdeconnect.findmyphone.request",
        // Curated command list + execution (handle_runcommand).
        "kdeconnect.runcommand.request",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    let outgoing_capabilities = [
        "kdeconnect.ping",
        "kdeconnect.battery",
        "kdeconnect.battery.request",
        "kdeconnect.clipboard",
        "kdeconnect.clipboard.connect",
        "kdeconnect.notification",
        "kdeconnect.share.request",
        "kdeconnect.findmyphone.request",
        "kdeconnect.sms.request",
        "kdeconnect.telephony.request",
        "kdeconnect.runcommand",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    Announce {
        device_id,
        device_name,
        device_type: DeviceType::Desktop,
        protocol_version: 7,
        incoming_capabilities,
        outgoing_capabilities,
    }
}

/// Seed the roster from the shared pairing store (all paired peers, offline) so
/// the worker answers `action/connect/devices` even before any device connects.
fn seed_roster(store: &PairingStore) -> HashMap<String, DeviceInfo> {
    store
        .records()
        .into_iter()
        .map(|rec| {
            let name = if rec.device_name.is_empty() {
                rec.device_id.clone()
            } else {
                rec.device_name.clone()
            };
            (
                rec.device_id.clone(),
                DeviceInfo {
                    id: rec.device_id,
                    name,
                    online: false,
                    battery: None,
                },
            )
        })
        .collect()
}

/// Wire shape for one roster row over the Bus (JSON carries `battery` as a real
/// `Option<u8>`, unlike the old D-Bus `-1`-for-unknown tuple).
#[derive(serde::Serialize, serde::Deserialize)]
struct WireDevice {
    id: String,
    name: String,
    online: bool,
    battery: Option<u8>,
}

/// Snapshot the roster as a sorted JSON array of [`WireDevice`] — the reply
/// body for an `action/connect/devices` query. `"[]"` on a poisoned lock or
/// encode error (an honest empty roster).
fn roster_json(roster: &Roster) -> String {
    let mut wires: Vec<WireDevice> = roster
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .values()
        .map(|d| WireDevice {
            id: d.id.clone(),
            name: d.name.clone(),
            online: d.online,
            battery: d.battery,
        })
        .collect();
    wires.sort_by(|a, b| a.id.cmp(&b.id));
    serde_json::to_string(&wires).unwrap_or_else(|_| "[]".to_string())
}

/// Run the KDE Connect LAN host (UDP discovery + the inbound TLS listener) over
/// the shared pairing store, folding its events into `roster`. Best-effort: a
/// discovery-bind or transport-start failure logs + returns, leaving the worker
/// serving the seeded (static) roster — never fails worker startup.
async fn run_host(
    pairing: Arc<PairingStore>,
    roster: Roster,
    outbound: PendingSends,
    config_dir: PathBuf,
) {
    let announce = local_announce();
    let bind = SocketAddr::from(([0, 0, 0, 0], KDC_PORT));
    let discovery = match UdpDiscovery::bind(bind, announce.clone()).await {
        Ok(d) => d,
        Err(e) => {
            warn!(error = %e, port = KDC_PORT, "kdc-host: UDP bind failed; serving static roster");
            return;
        }
    };
    let transport =
        LanTransport::new(announce, discovery, Arc::clone(&pairing)).with_listen_addr(bind);
    let (sink, mut stream) = EventStream::channel();
    if let Err(e) = transport.start(sink).await {
        warn!(error = %e, "kdc-host: transport start failed; serving static roster");
        return;
    }
    // SEC-5 — the mesh-shunt: publish this peer's paired phones to the
    // replicated volume + relay neighbors' phones into the roster, so a
    // phone paired on another peer shows up here (and is outbound-
    // pairable) without a direct LAN broadcast.
    let shunt_root = crate::default_qnm_shared_root();
    let shunt_host = hostname_for_shunt();
    let shunt_registry = std::sync::Mutex::new(mde_kdc_proto::discovery::DiscoveryRegistry::new());
    let mut shunt_tick = tokio::time::interval(super::mesh_shunt::TICK);
    // AUD-2 — the kdc_outbound drainer: every second, take the operator-queued
    // ring/sms/clipboard/share packets and deliver each over the live
    // LanTransport to its paired device. Failures (device offline / not yet
    // connected) are logged, not retried — operator actions are fire-and-forget.
    let mut drain_tick = tokio::time::interval(OUTBOUND_DRAIN);
    loop {
        tokio::select! {
            ev = stream.recv() => {
                let Some(ev) = ev else { break };
                // KDC-NOISE-1 — these fire on every packet / failed handshake
                // (identity_eof repeats ~every 3s as clients probe), so they're
                // debug-level diagnostics, not info/warn journal spam. Notable
                // device events still reach the Alert Center via kdc_event_alert.
                match &ev {
                    HostEvent::Packet { peer, packet } => {
                        debug!(peer = %peer.as_str(), kind = %packet.kind, body = %packet.body, "kdc-host: rx packet");
                    }
                    HostEvent::TransportError(e) => {
                        debug!(error = %e, "kdc-host: transport error");
                    }
                    HostEvent::Connected(p) => info!(peer = %p.as_str(), "kdc-host: connected"),
                    HostEvent::Disconnected(p) => info!(peer = %p.as_str(), "kdc-host: disconnected"),
                    HostEvent::PeerDiscovered(a) => {
                        info!(device = %a.device_id, name = %a.device_name, "kdc-host: discovered")
                    }
                    HostEvent::PeerLost(_) => {}
                }
                // NOTIFY-SRC-3 — surface notable device events to the Alert Center.
                if let Some((summary, severity)) = kdc_event_alert(&ev) {
                    publish_kdc_alert(&summary, severity);
                }
                // A phone-initiated `kdeconnect.pair{pair:true}`: pin the cert seen
                // at TLS time, persist the device, and accept.
                if let HostEvent::Packet { peer, packet } = &ev {
                    if packet.kind == "kdeconnect.pair"
                        && packet.body.get("pair").and_then(serde_json::Value::as_bool)
                            == Some(true)
                    {
                        accept_pair(&pairing, &transport, &roster, peer).await;
                    }
                    // KDC-PLUGINS — Run Command: the phone asks for the command list
                    // (`requestCommandList`) or triggers a curated key. Results come
                    // back as a `kdeconnect.ping` notification on the phone.
                    if packet.kind == "kdeconnect.runcommand.request" {
                        handle_runcommand(&transport, &config_dir, peer, &packet.body).await;
                    }
                    // KDC-PLUGINS — Battery request: the peer polls THIS host's
                    // battery. Answer with a `kdeconnect.battery` snapshot read
                    // from `/sys/class/power_supply` (a desktop replies cleanly
                    // with the "-1 / not a battery" sentinel).
                    if packet.kind == "kdeconnect.battery.request" {
                        handle_battery_request(&transport, peer).await;
                    }
                    // KDC-PLUGINS — Find My Phone: the peer rings THIS host. Play
                    // an audible alert through the desktop sound path (the same
                    // canberra/paplay path the notify-toast uses).
                    if packet.kind == "kdeconnect.findmyphone.request" {
                        ring_local_device();
                    }
                    // KDC-PLUGINS — Clipboard: a peer's copy (live or the
                    // connection-time `.connect` push) is applied to THIS host's
                    // Wayland clipboard via `wl-copy` when present.
                    if packet.kind == "kdeconnect.clipboard"
                        || packet.kind == "kdeconnect.clipboard.connect"
                    {
                        if let Some(content) = packet.body.get("content").and_then(Value::as_str) {
                            apply_clipboard(content);
                        }
                    }
                }
                if let Ok(mut m) = roster.lock() {
                    apply_event(&mut m, ev);
                }
            }
            _ = shunt_tick.tick() => {
                run_shunt_tick(&pairing, &roster, &shunt_root, &shunt_host, &shunt_registry);
            }
            _ = drain_tick.tick() => {
                for send in outbound.take_all() {
                    let peer = PeerId::from(send.device_id.as_str());
                    if let Err(e) = transport.send_to(&peer, send.packet).await {
                        warn!(device = %send.device_id, error = %e, "kdc-host: outbound send failed (device offline?)");
                    } else {
                        debug!(device = %send.device_id, "kdc-host: delivered queued packet");
                    }
                }
            }
        }
    }
}

/// KDC-INTEROP — accept a phone-initiated `kdeconnect.pair{pair:true}`: pin the
/// cert fingerprint captured during the inbound TLS handshake, persist the device
/// as paired, and send the `kdeconnect.pair{pair:true}` acceptance back over the
/// live link. Auto-accepts — the operator already initiated the request from the
/// phone, so a second confirm on the desktop would be friction (the "magic" UX);
/// a confirmation prompt can layer on later via the Connect panel.
async fn accept_pair(
    pairing: &Arc<PairingStore>,
    transport: &LanTransport,
    roster: &Roster,
    peer: &PeerId,
) {
    let device_id = peer.as_str().to_string();
    // Re-ack an already-paired device so a repeat request settles cleanly.
    if pairing.is_paired(&device_id) {
        let ack = build_packet("kdeconnect.pair", json!({ "pair": true }));
        let _ = transport.send_to(peer, ack).await;
        return;
    }
    let fingerprint = transport
        .inbound_fingerprint(&device_id)
        .await
        .unwrap_or_default();
    let device_name = roster
        .lock()
        .ok()
        .and_then(|m| m.get(&device_id).map(|d| d.name.clone()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| device_id.clone());
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let record = DeviceRecord {
        device_id: device_id.clone(),
        device_name,
        paired_at_ms: now_ms,
        fingerprint,
    };
    if let Err(e) = pairing.pair(record) {
        warn!(device = %device_id, error = %e, "kdc-host: pairing persist failed");
        return;
    }
    let ack = build_packet("kdeconnect.pair", json!({ "pair": true }));
    if let Err(e) = transport.send_to(peer, ack).await {
        warn!(device = %device_id, error = %e, "kdc-host: pair-accept send failed");
    } else {
        info!(device = %device_id, "kdc-host: paired (accepted phone request)");
    }
}

// ───────────────────────── KDC-PLUGINS: Run Command ──────────────────────
//
// KDE Connect's runcommand plugin: the phone requests the host's curated command
// list, then triggers a command by key. **Curated keys only — never arbitrary
// shell from the phone** (the phone can only invoke pre-defined commands), and
// only over the paired+authenticated link. Results are returned to the phone as a
// `kdeconnect.ping` notification so a "Mesh health check" actually shows its
// output on the phone. Editable via `<config_dir>/runcommands.toml`.

/// One operator-curated command the phone can trigger by `key`.
#[derive(Debug, Clone, serde::Deserialize)]
struct RunCmd {
    key: String,
    name: String,
    command: String,
}

/// `runcommands.toml` document root (`[[command]]` tables).
#[derive(Debug, Default, serde::Deserialize)]
struct RunCommandFile {
    #[serde(default)]
    command: Vec<RunCmd>,
}

/// The built-in **Mesh-ops** bundle (operator survey pick). Each result is sent
/// back to the phone as a ping notification.
fn default_runcommands() -> Vec<RunCmd> {
    [
        // Phone-friendly one-line summaries (the result is shown as a phone
        // notification, so raw JSON is unreadable — parse the key fields out).
        (
            "mesh-health",
            "Mesh health check",
            "h=$(mackesd healthz 2>&1); \
             printf 'Mesh %s/%s nodes healthy · audit=%s · v%s · ready=%s' \
               \"$(printf '%s' \"$h\" | grep -oP '\"healthy_nodes\":\\K[0-9]+')\" \
               \"$(printf '%s' \"$h\" | grep -oP '\"node_count\":\\K[0-9]+')\" \
               \"$(printf '%s' \"$h\" | grep -oP '\"audit_chain_intact\":\\K(true|false)')\" \
               \"$(printf '%s' \"$h\" | grep -oP '\"version\":\"\\K[^\"]+')\" \
               \"$(printf '%s' \"$h\" | grep -oP '\"ready\":\\K(true|false)')\"",
        ),
        (
            "mesh-status",
            "Mesh status (peers)",
            "printf 'Peers: '; mackesd peers --json 2>/dev/null \
             | grep -oP '\"hostname\":\"\\K[^\"]+' | paste -sd ', ' -",
        ),
        (
            "disk-headroom",
            "Disk headroom",
            "df -h --output=target,pcent,avail / /mnt/mesh-storage 2>/dev/null \
             | tail -n +2 | awk '{print $1\": \"$3\" free (\"$2\" used)\"}' | paste -sd ' | ' -",
        ),
        (
            "restart-mesh",
            "Restart mesh service",
            "systemd-run --on-active=2 systemctl restart mackesd >/dev/null 2>&1; \
             echo 'mesh service restarting in 2s'",
        ),
    ]
    .iter()
    .map(|(key, name, command)| RunCmd {
        key: (*key).to_string(),
        name: (*name).to_string(),
        command: (*command).to_string(),
    })
    .collect()
}

/// Load the curated command list: `<config_dir>/runcommands.toml` when present +
/// parseable, else the built-in [`default_runcommands`]. An empty file (no
/// `[[command]]`) also falls back to defaults so the phone is never left with an
/// empty list.
fn load_runcommands(config_dir: &std::path::Path) -> Vec<RunCmd> {
    let path = config_dir.join("runcommands.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => match toml::from_str::<RunCommandFile>(&text) {
            Ok(f) if !f.command.is_empty() => f.command,
            _ => default_runcommands(),
        },
        Err(_) => default_runcommands(),
    }
}

/// Build the `commandList` payload: KDE Connect expects a **stringified** JSON map
/// of `key → {name, command}` inside the `kdeconnect.runcommand` body.
fn command_list_json(cmds: &[RunCmd]) -> String {
    let map: serde_json::Map<String, Value> = cmds
        .iter()
        .map(|c| {
            (
                c.key.clone(),
                json!({ "name": c.name, "command": c.command }),
            )
        })
        .collect();
    Value::Object(map).to_string()
}

/// Run the command bound to `key` and return a phone-friendly result line. Unknown
/// keys (a phone can't invent commands, but be defensive) return an error string.
/// Output is trimmed + truncated for the ping notification.
fn execute_runcommand(cmds: &[RunCmd], key: &str) -> String {
    let Some(cmd) = cmds.iter().find(|c| c.key == key) else {
        return format!("unknown command: {key}");
    };
    match std::process::Command::new("sh")
        .arg("-c")
        .arg(&cmd.command)
        .output()
    {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let body = if stdout.trim().is_empty() {
                String::from_utf8_lossy(&out.stderr).trim().to_string()
            } else {
                stdout.trim().to_string()
            };
            let shown: String = body.chars().take(480).collect();
            if shown.is_empty() {
                format!("{}: done", cmd.name)
            } else {
                format!("{}\n{shown}", cmd.name)
            }
        }
        Err(e) => format!("{}: failed to run ({e})", cmd.name),
    }
}

/// Handle one `kdeconnect.runcommand.request`: either publish the command list
/// (`requestCommandList`) or execute a `key` and ping the result back. Execution
/// runs off the reactor thread (`spawn_blocking`) so a slow command can't stall
/// the host event loop.
async fn handle_runcommand(
    transport: &LanTransport,
    config_dir: &std::path::Path,
    peer: &PeerId,
    body: &Value,
) {
    let cmds = load_runcommands(config_dir);
    if body.get("requestCommandList").and_then(Value::as_bool) == Some(true) {
        let list = command_list_json(&cmds);
        let pkt = build_packet("kdeconnect.runcommand", json!({ "commandList": list }));
        if let Err(e) = transport.send_to(peer, pkt).await {
            warn!(error = %e, "kdc-host: runcommand list send failed");
        }
        return;
    }
    if let Some(key) = body.get("key").and_then(Value::as_str) {
        let key = key.to_string();
        let result = tokio::task::spawn_blocking(move || execute_runcommand(&cmds, &key))
            .await
            .unwrap_or_else(|_| "command execution failed".to_string());
        info!(device = %peer.as_str(), "kdc-host: ran phone command");
        let pkt = build_packet("kdeconnect.ping", json!({ "message": result }));
        if let Err(e) = transport.send_to(peer, pkt).await {
            warn!(error = %e, "kdc-host: runcommand result ping failed");
        }
    }
}

// ───────────────────────── KDC-PLUGINS: Battery ──────────────────────────
//
// A peer (typically a phone) sends `kdeconnect.battery.request` to poll this
// host's power state. We read `/sys/class/power_supply` (the same source the
// hardware-probe worker uses) and reply with a `kdeconnect.battery` snapshot.
// A desktop/server/VM with no battery answers the clean upstream "-1 / not a
// battery" sentinel so the phone renders nothing rather than a bogus 0%.

/// Read a `/sys/class/power_supply` integer file, if present.
fn read_power_supply_u8(path: &str) -> Option<u8> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// This host's battery snapshot for a `battery.request` reply, read from
/// sysfs. Returns the "not a battery" sentinel on a machine with no
/// `BAT0`/`BAT1` capacity node (the common mesh-host case).
fn local_battery_body() -> BatteryBody {
    let charge = read_power_supply_u8("/sys/class/power_supply/BAT0/capacity")
        .or_else(|| read_power_supply_u8("/sys/class/power_supply/BAT1/capacity"));
    charge.map_or_else(BatteryBody::not_a_battery, |pct| {
        // `AC/online == 1` means plugged in (charging). Absent AC node →
        // assume on battery (laptop unplugged) rather than guessing.
        let on_ac = read_power_supply_u8("/sys/class/power_supply/AC/online")
            .or_else(|| read_power_supply_u8("/sys/class/power_supply/ACAD/online"))
            .or_else(|| read_power_supply_u8("/sys/class/power_supply/ADP1/online"))
            .is_some_and(|v| v == 1);
        BatteryBody::from_charge(pct, on_ac)
    })
}

/// Answer a `kdeconnect.battery.request` with this host's live snapshot.
async fn handle_battery_request(transport: &LanTransport, peer: &PeerId) {
    let body = local_battery_body();
    let pkt = build_packet(
        "kdeconnect.battery",
        serde_json::to_value(&body).unwrap_or(Value::Null),
    );
    if let Err(e) = transport.send_to(peer, pkt).await {
        warn!(error = %e, "kdc-host: battery reply send failed");
    }
}

// ──────────────────────── KDC-PLUGINS: Find My Phone ──────────────────────
//
// `kdeconnect.findmyphone.request` rings the receiving device. When a phone
// rings THIS host, play an audible alert through the desktop sound path
// (canberra theme sound, falling back to paplay of the bell .oga). Best-effort
// + non-blocking: a headless/soundless host simply stays silent.

/// Ring this host audibly in response to a Find-My-Device request. Spawns the
/// sound player detached (never blocks the host event loop) and is silent when
/// neither player nor a sound theme is present.
fn ring_local_device() {
    use std::process::{Command, Stdio};
    // Theme-aware bell via canberra; falls back to paplay of the freedesktop
    // sound-theme bell if canberra isn't installed.
    let canberra = Command::new("canberra-gtk-play")
        .args(["-i", "bell"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if canberra.is_ok() {
        return;
    }
    for oga in [
        "/usr/share/sounds/freedesktop/stereo/bell.oga",
        "/usr/share/sounds/freedesktop/stereo/complete.oga",
    ] {
        if std::path::Path::new(oga).exists() {
            let _ = Command::new("paplay")
                .arg(oga)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            return;
        }
    }
    info!("kdc-host: find-my-device ring requested (no audio player available)");
}

// ───────────────────────── KDC-PLUGINS: Clipboard ────────────────────────
//
// A peer's clipboard copy (a live `kdeconnect.clipboard` or the connection-time
// `kdeconnect.clipboard.connect` push) is applied to THIS host's Wayland
// clipboard via `wl-copy`. Best-effort: a host without wl-clipboard installed
// (or no Wayland session) simply skips — no error surfaced to the peer.

/// Apply inbound clipboard `content` to this host's Wayland clipboard via
/// `wl-copy`. Pipes the content over stdin (no shell-quoting hazard) and
/// detaches; silently no-ops when `wl-copy` is absent.
fn apply_clipboard(content: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let child = Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return; // wl-copy not installed / no Wayland — skip cleanly
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(content.as_bytes());
    }
    // Don't block the event loop waiting on wl-copy; it exits promptly.
    drop(child);
}

/// This peer's hostname for the shunt's published filename.
fn hostname_for_shunt() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// One mesh-shunt pass (SEC-5): publish our paired devices, relay
/// neighbors' into the discovery registry, then fold every fresh
/// relayed announce into the roster as a discovered peer.
fn run_shunt_tick(
    pairing: &Arc<PairingStore>,
    roster: &Roster,
    root: &std::path::Path,
    hostname: &str,
    registry: &std::sync::Mutex<mde_kdc_proto::discovery::DiscoveryRegistry>,
) {
    let mine: Vec<super::mesh_shunt::PublishedDevice> = pairing
        .records()
        .iter()
        .map(|r| super::mesh_shunt::PublishedDevice {
            device_id: r.device_id.clone(),
            device_name: r.device_name.clone(),
        })
        .collect();
    if let Err(e) = super::mesh_shunt::publish_phones(root, hostname, &mine) {
        warn!(error = %e, "kdc-host: mesh-shunt publish failed");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let synthetic = super::mesh_shunt::collect_synthetic(root, hostname, now);
    super::mesh_shunt::inject_fresh(registry, synthetic, now);
    if let Ok(reg) = registry.lock() {
        if let Ok(mut m) = roster.lock() {
            for a in reg.take_fresh(now) {
                apply_event(&mut m, HostEvent::PeerDiscovered(a));
            }
        }
    }
}

/// Handle one `action/connect/<verb>` request and return the reply JSON.
/// Pure over (`store`, `outbound`) — the unit tests drive it directly.
/// E2.2 — faithfully serves the operator verbs over the canonical store:
/// `version`/`list`/`get` read; `pair`/`unpair` mutate the store;
/// `ring`/`sms`/`clipboard` enqueue an outbound `Packet`.
fn handle_connect_verb(
    store: &PairingStore,
    outbound: &PendingSends,
    verb: &str,
    body: &Value,
) -> String {
    let dev_id = || {
        body.get("device_id")
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let reply = match verb {
        "version" => json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") }),
        "list" => json!({
            "ok": true,
            "devices": store.records().iter().map(device_json).collect::<Vec<_>>(),
        }),
        "get" => match dev_id().and_then(|id| store.get(&id)) {
            Some(rec) => json!({ "ok": true, "device": device_json(&rec) }),
            None => json!({ "ok": false, "error": "NoSuchDevice" }),
        },
        "pair" => {
            let record = DeviceRecord {
                device_id: body
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                device_name: body
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                paired_at_ms: body.get("paired_at").and_then(Value::as_i64).unwrap_or(0),
                fingerprint: body
                    .get("fingerprint")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            };
            match store.pair(record) {
                Ok(()) => json!({ "ok": true }),
                Err(e) => json!({ "ok": false, "error": format!("PersistFailed: {e}") }),
            }
        }
        // SEC-4 — the operator-initiated OUTBOUND first pair: dial the
        // device's TLS port, capture the fingerprint it actually
        // presents, persist the sealed session + the pin. The body
        // carries {id, name, addr} (addr from the discovery roster).
        "pair-device" => 'pd: {
            let (Some(id), Some(addr)) = (
                body.get("id").and_then(Value::as_str).map(str::to_string),
                body.get("addr").and_then(Value::as_str).map(str::to_string),
            ) else {
                break 'pd json!({ "ok": false, "error": "pair-device: need id + addr" });
            };
            let name = body
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&id)
                .to_string();
            let Ok(sock_addr) = addr.parse() else {
                break 'pd json!({ "ok": false, "error": format!("pair-device: bad addr {addr}") });
            };
            // The responder is a sync poller thread — spin a
            // current-thread runtime for the async dial (the same
            // bridge pattern the Bus clients use).
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("runtime: {e}"))
                .and_then(|rt| {
                    rt.block_on(mde_kdc_host::first_pair::first_pair(
                        store, &id, &name, sock_addr,
                    ))
                    .map_err(|e| e.to_string())
                });
            match result {
                Ok(outcome) => json!({
                    "ok": true,
                    "device_id": outcome.device_id,
                    "fingerprint": outcome.fingerprint,
                }),
                Err(e) => json!({ "ok": false, "error": e }),
            }
        }
        "unpair" => match dev_id() {
            Some(id) if store.is_paired(&id) => match store.unpair(&id) {
                Ok(()) => json!({ "ok": true }),
                Err(e) => json!({ "ok": false, "error": format!("PersistFailed: {e}") }),
            },
            _ => json!({ "ok": false, "error": "NoSuchDevice" }),
        },
        "ring" | "sms" | "clipboard" => {
            let Some(id) = dev_id() else {
                return json!({ "ok": false, "error": "NoSuchDevice" }).to_string();
            };
            if !store.is_paired(&id) {
                return json!({ "ok": false, "error": "NoSuchDevice" }).to_string();
            }
            let packet = match verb {
                "ring" => build_packet("kdeconnect.findmyphone.request", json!({})),
                "sms" => build_packet(
                    "kdeconnect.sms.request",
                    json!({
                        "sendSms": true,
                        "phoneNumber": body.get("recipient").and_then(Value::as_str).unwrap_or_default(),
                        "messageBody": body.get("message").and_then(Value::as_str).unwrap_or_default(),
                    }),
                ),
                _ => build_packet(
                    "kdeconnect.clipboard",
                    json!({ "content": body.get("content").and_then(Value::as_str).unwrap_or_default() }),
                ),
            };
            outbound.push(OutboundSend {
                device_id: id,
                packet,
            });
            json!({ "ok": true })
        }
        // PD-3/L6 — Send-file (or paste-share a URL) from the Peers Devices
        // group. A `url` body builds a URL share; otherwise a `filename`
        // (+ optional `payload_size`) builds a file-share announce. The
        // binary payload streams over the KDC file-transfer port — the same
        // KDC2-3 host follow-up that delivers ring/sms (the outbound queue
        // is drained by `kdc_outbound`).
        "share" => {
            let Some(id) = dev_id() else {
                return json!({ "ok": false, "error": "NoSuchDevice" }).to_string();
            };
            if !store.is_paired(&id) {
                return json!({ "ok": false, "error": "NoSuchDevice" }).to_string();
            }
            let id_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
                .unwrap_or(0);
            let url = body.get("url").and_then(Value::as_str).unwrap_or_default();
            let filename = body
                .get("filename")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let packet = if url.is_empty() {
                if filename.is_empty() {
                    return json!({ "ok": false, "error": "share: need url or filename" })
                        .to_string();
                }
                let payload_size = body
                    .get("payload_size")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                mde_kdc_proto::plugins::share::file_share_packet(
                    id_ms,
                    filename.to_string(),
                    payload_size,
                    String::new(),
                )
            } else {
                let open = body.get("open").and_then(Value::as_bool).unwrap_or(true);
                mde_kdc_proto::plugins::share::url_share_packet(id_ms, url.to_string(), open)
            };
            outbound.push(OutboundSend {
                device_id: id,
                packet,
            });
            json!({ "ok": true })
        }
        other => json!({ "ok": false, "error": format!("unknown verb: {other}") }),
    };
    reply.to_string()
}

/// The `action/connect/*` Bus responder loop. Sync (the verb handlers +
/// `Persist` are all sync), so it runs on its own `std::thread` and stops
/// when `stop` is set. Mirrors `mde-session`'s poll responder.
fn serve_connect_bus(
    persist: &Persist,
    store: &PairingStore,
    roster: &Roster,
    outbound: &PendingSends,
    stop: &AtomicBool,
) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !stop.load(Ordering::Relaxed) {
        for verb in CONNECT_VERBS {
            let topic = format!("action/connect/{verb}");
            let since = cursors.get(&topic).map(String::as_str);
            let msgs = match persist.list_since(&topic, since) {
                Ok(m) => m,
                Err(_) => continue,
            };
            for msg in msgs {
                cursors.insert(topic.clone(), msg.ulid.clone());
                let body: Value = msg
                    .body
                    .as_deref()
                    .and_then(|b| serde_json::from_str(b).ok())
                    .unwrap_or(Value::Null);
                // EFF-7 — a panicking verb handler must not kill the Connect
                // responder thread; answer with an error envelope instead.
                let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    handle_connect_verb(store, outbound, verb, &body)
                }))
                .unwrap_or_else(|_| {
                    error!(verb = %verb, "kdc-host: connect verb handler panicked");
                    json!({ "ok": false, "error": "internal error" }).to_string()
                });
                let _ = persist.write(
                    &reply_topic(&msg.ulid),
                    Priority::Default,
                    None,
                    Some(&reply),
                );
            }
        }
        // The live-roster query (`action/connect/devices`) — the shell surfaces'
        // read path. Answered from the host-folded roster (online/battery), not
        // the store, so a connected phone's live status is reflected.
        if let Ok(msgs) = persist.list_since(
            DEVICES_TOPIC,
            cursors.get(DEVICES_TOPIC).map(String::as_str),
        ) {
            for msg in msgs {
                cursors.insert(DEVICES_TOPIC.to_string(), msg.ulid.clone());
                let reply = roster_json(roster);
                let _ = persist.write(
                    &reply_topic(&msg.ulid),
                    Priority::Default,
                    None,
                    Some(&reply),
                );
            }
        }
        std::thread::sleep(CONNECT_POLL);
    }
}

#[async_trait::async_trait]
impl Worker for KdcHostWorker {
    fn name(&self) -> &'static str {
        "kdc-host"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // Open the pairing store (idempotent). On failure, surface to
        // the supervisor so the restart policy can act.
        let pairing_arc = self.open_pairing().map_err(|e| {
            error!(error = %e, "kdc-host: pairing store init failed");
            anyhow::anyhow!("kdc-host init failed: {e}")
        })?;

        // E2.3 — the single, supervised KDE Connect host. Seed the published
        // roster from the store (paired peers, offline), then run the live
        // `LanTransport` (UDP discovery + inbound TLS listener) as a task on the
        // supervisor runtime, folding its events into the roster. Best-effort:
        // a bind/start failure leaves the seeded (static) roster served.
        let roster: Roster = Arc::new(Mutex::new(seed_roster(&pairing_arc)));
        let host_task = tokio::spawn(run_host(
            Arc::clone(&pairing_arc),
            Arc::clone(&roster),
            self.outbound.clone(),
            self.config_dir.clone(),
        ));

        // E2.2/E2.3 — serve the operator Connect actions (`action/connect/<verb>`)
        // + the live roster (`action/connect/devices`) over the Bus, replacing
        // the retired `dev.mackes.MDE.Connect` D-Bus surface. Runs on its own
        // thread (`Persist` is `!Send`) until the stop flag is set on shutdown;
        // a missing Bus dir / open failure degrades the surface to "unavailable"
        // without failing worker startup.
        self.responder_stop.store(false, Ordering::Relaxed);
        let stop = Arc::clone(&self.responder_stop);
        let store = Arc::clone(&pairing_arc);
        let resp_roster = Arc::clone(&roster);
        let outbound = self.outbound.clone();
        let responder = std::thread::Builder::new()
            .name("kdc-connect-bus".into())
            .spawn(move || {
                let Some(bus_root) = mde_bus::default_data_dir() else {
                    warn!("kdc-host: no Bus data dir; Connect actions unavailable");
                    return;
                };
                match Persist::open(bus_root) {
                    Ok(p) => serve_connect_bus(&p, &store, &resp_roster, &outbound, &stop),
                    Err(e) => {
                        warn!(error = %e, "kdc-host: opening Bus store for Connect responder")
                    }
                }
            })
            .ok();
        info!(
            config_dir = %self.config_dir.display(),
            connect_bus = responder.is_some(),
            "kdc-host: started",
        );

        let mut interval = tokio::time::interval(TICK);
        // First tick fires immediately; skip it so we don't
        // double-log "started" + "tick" at startup.
        interval.tick().await;

        loop {
            tokio::select! {
                _ = shutdown.wait() => {
                    info!("kdc-host: shutdown requested; exiting");
                    // Stop the live host task + the Connect Bus responder thread.
                    host_task.abort();
                    self.responder_stop.store(true, Ordering::Relaxed);
                    if let Some(h) = responder {
                        let _ = h.join();
                    }
                    return Ok(());
                }
                _ = interval.tick() => {
                    debug!(
                        roster = roster.lock().map(|m| m.len()).unwrap_or(0),
                        outbound_backlog = self.outbound.len(),
                        "kdc-host: tick",
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn worker_name_matches_module() {
        let w = KdcHostWorker::new(PathBuf::from("/tmp"));
        assert_eq!(w.name(), "kdc-host");
    }

    #[test]
    fn read_power_supply_u8_handles_missing_and_garbage() {
        // A non-existent sysfs node is None (the desktop case).
        assert_eq!(
            read_power_supply_u8("/sys/class/power_supply/__nope__/capacity"),
            None
        );
        // A bogus path never panics.
        assert_eq!(read_power_supply_u8("/definitely/not/a/file"), None);
    }

    #[test]
    fn local_battery_body_is_serializable_and_sane() {
        // Whatever this host is (laptop or desktop), the reply body must be a
        // valid `kdeconnect.battery` JSON object. A desktop yields the -1
        // sentinel; a laptop yields a 0..=100 charge — both are valid.
        let body = local_battery_body();
        let v = serde_json::to_value(&body).expect("battery body serializes");
        assert!(v.get("currentCharge").is_some());
        assert!(v.get("isCharging").is_some());
        // charge_pct() is None (sentinel) or Some(0..=100); never out of range.
        if let Some(p) = body.charge_pct() {
            assert!(p <= 100);
        }
    }

    #[test]
    fn ring_and_clipboard_helpers_never_panic_when_tools_absent() {
        // Best-effort host actions: with no audio player / wl-copy present (CI),
        // these spawn-or-skip without panicking or blocking.
        ring_local_device();
        apply_clipboard("test clipboard content");
    }

    #[test]
    fn default_runcommands_are_the_mesh_ops_bundle() {
        let defaults = default_runcommands();
        let keys: Vec<&str> = defaults.iter().map(|c| c.key.as_str()).collect();
        // The operator-selected Mesh-ops set.
        for k in [
            "mesh-health",
            "mesh-status",
            "disk-headroom",
            "restart-mesh",
        ] {
            assert!(keys.contains(&k), "missing default runcommand {k}");
        }
    }

    #[test]
    fn load_runcommands_falls_back_to_defaults_without_a_toml() {
        let tmp = tempdir().unwrap();
        let cmds = load_runcommands(tmp.path());
        assert_eq!(cmds.len(), default_runcommands().len());
    }

    #[test]
    fn load_runcommands_reads_a_custom_toml() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("runcommands.toml"),
            "[[command]]\nkey=\"lock\"\nname=\"Lock screen\"\ncommand=\"loginctl lock-session\"\n",
        )
        .unwrap();
        let cmds = load_runcommands(tmp.path());
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].key, "lock");
        assert_eq!(cmds[0].name, "Lock screen");
    }

    #[test]
    fn command_list_json_is_a_keyed_name_command_map() {
        let cmds = vec![RunCmd {
            key: "k1".into(),
            name: "N1".into(),
            command: "echo hi".into(),
        }];
        let s = command_list_json(&cmds);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["k1"]["name"], "N1");
        assert_eq!(v["k1"]["command"], "echo hi");
    }

    #[test]
    fn execute_runcommand_runs_known_key_and_rejects_unknown() {
        let cmds = vec![RunCmd {
            key: "say".into(),
            name: "Say hi".into(),
            command: "echo mesh-ok".into(),
        }];
        let out = execute_runcommand(&cmds, "say");
        assert!(out.contains("mesh-ok"), "output should carry stdout: {out}");
        assert!(execute_runcommand(&cmds, "nope").contains("unknown command"));
    }

    #[test]
    fn open_pairing_creates_the_identity() {
        // E2.2 — the worker holds the canonical pairing store now.
        // open_pairing opens it, creating identity.pkcs8 on first run.
        let tmp = tempdir().unwrap();
        let w = KdcHostWorker::new(tmp.path().to_path_buf());
        let store = w.open_pairing().unwrap();
        assert!(Arc::strong_count(&store) >= 1);
        assert!(tmp.path().join("identity.pkcs8").exists());
    }

    fn test_store(dir: &std::path::Path) -> PairingStore {
        PairingStore::open(dir).unwrap()
    }

    fn pair_body(id: &str, name: &str) -> Value {
        json!({
            "id": id, "name": name, "kind": "phone",
            "fingerprint": "AB:CD", "public_key_b64": "", "capabilities": [],
            "paired_at": 123,
        })
    }

    #[test]
    fn connect_verb_version_and_empty_list() {
        let tmp = tempdir().unwrap();
        let store = test_store(tmp.path());
        let outbound = PendingSends::new();
        let v: Value = serde_json::from_str(&handle_connect_verb(
            &store,
            &outbound,
            "version",
            &Value::Null,
        ))
        .unwrap();
        assert_eq!(v["ok"], true);
        assert!(v["version"].is_string());
        let l: Value = serde_json::from_str(&handle_connect_verb(
            &store,
            &outbound,
            "list",
            &Value::Null,
        ))
        .unwrap();
        assert_eq!(l["devices"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn connect_verb_pair_get_unpair_roundtrip() {
        let tmp = tempdir().unwrap();
        let store = test_store(tmp.path());
        let outbound = PendingSends::new();
        // pair
        let r: Value = serde_json::from_str(&handle_connect_verb(
            &store,
            &outbound,
            "pair",
            &pair_body("d1", "Pixel"),
        ))
        .unwrap();
        assert_eq!(r["ok"], true);
        // get
        let g: Value = serde_json::from_str(&handle_connect_verb(
            &store,
            &outbound,
            "get",
            &json!({ "device_id": "d1" }),
        ))
        .unwrap();
        assert_eq!(g["device"]["name"], "Pixel");
        assert_eq!(g["device"]["fingerprint"], "AB:CD");
        // get unknown
        let gx: Value = serde_json::from_str(&handle_connect_verb(
            &store,
            &outbound,
            "get",
            &json!({ "device_id": "nope" }),
        ))
        .unwrap();
        assert_eq!(gx["error"], "NoSuchDevice");
        // unpair, then unpair-again
        let u: Value = serde_json::from_str(&handle_connect_verb(
            &store,
            &outbound,
            "unpair",
            &json!({ "device_id": "d1" }),
        ))
        .unwrap();
        assert_eq!(u["ok"], true);
        let u2: Value = serde_json::from_str(&handle_connect_verb(
            &store,
            &outbound,
            "unpair",
            &json!({ "device_id": "d1" }),
        ))
        .unwrap();
        assert_eq!(u2["error"], "NoSuchDevice");
    }

    #[test]
    fn connect_verb_pair_persists_across_reopen() {
        // E2.2 — the pair verb writes through to the canonical store's
        // devices.toml; a fresh store opened on the same dir sees it.
        let tmp = tempdir().unwrap();
        {
            let store = test_store(tmp.path());
            let outbound = PendingSends::new();
            handle_connect_verb(&store, &outbound, "pair", &pair_body("d1", "Pixel"));
        }
        let reopened = PairingStore::open(tmp.path()).unwrap();
        assert!(reopened.is_paired("d1"));
        assert_eq!(reopened.get("d1").unwrap().device_name, "Pixel");
    }

    #[test]
    fn connect_verb_ring_requires_paired_and_enqueues() {
        let tmp = tempdir().unwrap();
        let store = test_store(tmp.path());
        let outbound = PendingSends::new();
        // ring an unpaired device -> NoSuchDevice, nothing queued.
        let r: Value = serde_json::from_str(&handle_connect_verb(
            &store,
            &outbound,
            "ring",
            &json!({ "device_id": "d1" }),
        ))
        .unwrap();
        assert_eq!(r["error"], "NoSuchDevice");
        assert_eq!(outbound.len(), 0);
        // pair then ring -> ok + one queued packet.
        handle_connect_verb(&store, &outbound, "pair", &pair_body("d1", "Pixel"));
        let r2: Value = serde_json::from_str(&handle_connect_verb(
            &store,
            &outbound,
            "ring",
            &json!({ "device_id": "d1" }),
        ))
        .unwrap();
        assert_eq!(r2["ok"], true);
        assert_eq!(outbound.len(), 1);
    }

    #[test]
    fn outbound_take_all_drains_the_queue() {
        // AUD-2 — the kdc_outbound drainer takes the whole backlog each tick.
        let q = PendingSends::new();
        assert_eq!(q.len(), 0);
        q.push(OutboundSend {
            device_id: "d1".into(),
            packet: build_packet("kdeconnect.findmyphone.request", json!({})),
        });
        q.push(OutboundSend {
            device_id: "d2".into(),
            packet: build_packet("kdeconnect.findmyphone.request", json!({})),
        });
        assert_eq!(q.len(), 2);
        let drained = q.take_all();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].device_id, "d1");
        assert_eq!(q.len(), 0, "queue is empty after a drain");
        // A second drain on the empty queue is a no-op.
        assert!(q.take_all().is_empty());
    }

    #[test]
    fn connect_verb_share_requires_paired_and_enqueues_file_or_url() {
        // PD-3/L6 — Send-file from the Peers Devices group enqueues a
        // share packet; an unpaired device is refused with nothing queued,
        // and a share with neither url nor filename is rejected.
        let tmp = tempdir().unwrap();
        let store = test_store(tmp.path());
        let outbound = PendingSends::new();
        // share to an unpaired device -> NoSuchDevice, nothing queued.
        let r: Value = serde_json::from_str(&handle_connect_verb(
            &store,
            &outbound,
            "share",
            &json!({ "device_id": "d1", "filename": "report.pdf" }),
        ))
        .unwrap();
        assert_eq!(r["error"], "NoSuchDevice");
        assert_eq!(outbound.len(), 0);
        handle_connect_verb(&store, &outbound, "pair", &pair_body("d1", "Pixel"));
        // empty share (no url, no filename) is rejected, still nothing queued.
        let empty: Value = serde_json::from_str(&handle_connect_verb(
            &store,
            &outbound,
            "share",
            &json!({ "device_id": "d1" }),
        ))
        .unwrap();
        assert_eq!(empty["ok"], false);
        assert_eq!(outbound.len(), 0);
        // file share -> ok + one queued packet.
        let f: Value = serde_json::from_str(&handle_connect_verb(
            &store,
            &outbound,
            "share",
            &json!({ "device_id": "d1", "filename": "report.pdf", "payload_size": 2048 }),
        ))
        .unwrap();
        assert_eq!(f["ok"], true);
        assert_eq!(outbound.len(), 1);
        // url share -> ok + a second queued packet.
        let u: Value = serde_json::from_str(&handle_connect_verb(
            &store,
            &outbound,
            "share",
            &json!({ "device_id": "d1", "url": "https://example.com" }),
        ))
        .unwrap();
        assert_eq!(u["ok"], true);
        assert_eq!(outbound.len(), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn worker_exits_on_shutdown_request() {
        let tmp = tempdir().unwrap();
        let mut w = KdcHostWorker::new(tmp.path().to_path_buf());
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = super::super::ShutdownToken::from_receiver(rx);

        let handle = tokio::spawn(async move { w.run(token).await });
        tx.send(true).expect("shutdown channel intact");
        let result = handle.await.expect("worker join");
        assert!(result.is_ok(), "worker must exit Ok on shutdown");
        // identity.pkcs8 was created during init.
        assert!(tmp.path().join("identity.pkcs8").exists());
    }

    // ── E2.3 live-roster folding (the host that moved off the shell daemon) ──

    use mde_kdc_host::PeerId;
    use mde_kdc_proto::plugins::battery::battery_packet;

    fn announce(id: &str, name: &str) -> Announce {
        Announce {
            device_id: id.into(),
            device_name: name.into(),
            device_type: DeviceType::Phone,
            protocol_version: 7,
            incoming_capabilities: vec![],
            outgoing_capabilities: vec![],
        }
    }

    #[test]
    fn kdc_event_alert_classifies_notable_events() {
        use mde_kdc_proto::wire::Packet;
        let pkt = |kind: &str, body: serde_json::Value| HostEvent::Packet {
            peer: PeerId::from("moto"),
            packet: serde_json::from_value::<Packet>(json!({"id":0,"type":kind,"body":body}))
                .expect("packet"),
        };
        // KDC-NOISE-1 — connect/disconnect presence churn + bare pings are NOT
        // surfaced to the Alert Center (too noisy; presence lives in the roster).
        assert!(kdc_event_alert(&HostEvent::Connected(PeerId::from("moto"))).is_none());
        assert!(kdc_event_alert(&HostEvent::Disconnected(PeerId::from("moto"))).is_none());
        assert!(kdc_event_alert(&pkt("kdeconnect.ping", json!({}))).is_none());
        // a phone notification mirrors app + text.
        let (s, sev) = kdc_event_alert(&pkt(
            "kdeconnect.notification",
            json!({"appName":"Signal","ticker":"new message"}),
        ))
        .expect("notification alert");
        assert_eq!(sev, "info");
        assert!(s.contains("Signal") && s.contains("new message"));
        // a cancel is skipped.
        assert!(
            kdc_event_alert(&pkt("kdeconnect.notification", json!({"isCancel":true}))).is_none()
        );
        // low battery warns; healthy battery is silent.
        assert_eq!(
            kdc_event_alert(&pkt("kdeconnect.battery", json!({"currentCharge":9}))).map(|(_, s)| s),
            Some("warn")
        );
        assert!(kdc_event_alert(&pkt("kdeconnect.battery", json!({"currentCharge":80}))).is_none());
        // noisy discovery refreshes are skipped.
        assert!(kdc_event_alert(&HostEvent::PeerLost(PeerId::from("moto"))).is_none());
    }

    #[test]
    fn apply_event_connected_then_disconnected_flips_online() {
        let mut m = HashMap::new();
        apply_event(&mut m, HostEvent::Connected(PeerId::from("p1")));
        assert!(m["p1"].online, "Connected brings the peer online");
        apply_event(&mut m, HostEvent::Disconnected(PeerId::from("p1")));
        assert!(
            !m["p1"].online,
            "Disconnected takes it offline (kept in roster)"
        );
        assert!(m.contains_key("p1"));
    }

    #[test]
    fn apply_event_discovery_refreshes_the_display_name() {
        let mut m = HashMap::new();
        m.insert("p1".to_string(), DeviceInfo::unknown("p1"));
        apply_event(&mut m, HostEvent::PeerDiscovered(announce("p1", "Pixel 8")));
        assert_eq!(m["p1"].name, "Pixel 8");
    }

    #[test]
    fn apply_event_battery_updates_charge_and_clamps_unknown() {
        let mut m = HashMap::new();
        m.insert("p1".to_string(), DeviceInfo::unknown("p1"));
        apply_event(
            &mut m,
            HostEvent::Packet {
                peer: PeerId::from("p1"),
                packet: battery_packet(
                    1,
                    BatteryBody {
                        current_charge: 73,
                        is_charging: false,
                        threshold_event: String::new(),
                    },
                ),
            },
        );
        assert_eq!(m["p1"].battery, Some(73));
        // Upstream's -1 "unknown" sentinel sanitizes to None.
        apply_event(
            &mut m,
            HostEvent::Packet {
                peer: PeerId::from("p1"),
                packet: battery_packet(
                    2,
                    BatteryBody {
                        current_charge: -1,
                        is_charging: false,
                        threshold_event: String::new(),
                    },
                ),
            },
        );
        assert_eq!(m["p1"].battery, None);
    }

    #[test]
    fn roster_json_round_trips_sorted_with_optional_battery() {
        let mut map = HashMap::new();
        map.insert(
            "zeta".to_string(),
            DeviceInfo {
                id: "zeta".into(),
                name: "Zeta".into(),
                online: true,
                battery: Some(80),
            },
        );
        map.insert(
            "alpha".to_string(),
            DeviceInfo {
                id: "alpha".into(),
                name: "Alpha".into(),
                online: false,
                battery: None,
            },
        );
        let roster: Roster = Arc::new(Mutex::new(map));
        let wires: Vec<WireDevice> =
            serde_json::from_str(&roster_json(&roster)).expect("decode roster json");
        assert_eq!(wires.len(), 2);
        assert_eq!(wires[0].id, "alpha");
        assert_eq!(wires[0].battery, None);
        assert_eq!(wires[1].id, "zeta");
        assert!(wires[1].online);
        assert_eq!(wires[1].battery, Some(80));
    }

    #[test]
    fn seed_roster_lists_paired_peers_offline() {
        // A device paired through the store seeds into the roster offline,
        // with no battery, so the worker answers `devices` before any link.
        let tmp = tempdir().unwrap();
        let store = test_store(tmp.path());
        handle_connect_verb(
            &store,
            &PendingSends::new(),
            "pair",
            &pair_body("d1", "Pixel"),
        );
        let seeded = seed_roster(&store);
        assert_eq!(seeded.len(), 1);
        let d = &seeded["d1"];
        assert_eq!(d.name, "Pixel");
        assert!(!d.online);
        assert_eq!(d.battery, None);
    }
}
