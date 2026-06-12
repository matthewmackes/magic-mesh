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
        std::mem::take(
            &mut *self
                .inner
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
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

/// This host's identity announce: `device_id` the machine id (stable across
/// boots), `device_name` the hostname; type Desktop, protocol 7.
fn local_announce() -> Announce {
    let device_id = std::fs::read_to_string("/etc/machine-id")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "mde-host".to_string());
    let device_name = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "MDE".to_string());
    Announce {
        device_id,
        device_name,
        device_type: DeviceType::Desktop,
        protocol_version: 7,
        incoming_capabilities: Vec::new(),
        outgoing_capabilities: Vec::new(),
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
async fn run_host(pairing: Arc<PairingStore>, roster: Roster, outbound: PendingSends) {
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
                let reply = handle_connect_verb(store, outbound, verb, &body);
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
