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
//! over the live transport once a second, so ring/sms/clipboard/share actually
//! reach a paired device (end-to-end byte delivery is the 2-device bench).
//!
//! **KDC-MESH-1 (2026-07-04) — overlay-only transport.** `run_host` now runs the
//! Nebula-overlay-only [`OverlayTransport`] instead of the LAN transport: the
//! inbound TLS listener binds 1716 on this node's overlay IP (never `0.0.0.0`),
//! peers are dialed by overlay IP, and there is no UDP broadcast discovery — all
//! KDC traffic rides the encrypted overlay (design lock #3/#15). If the overlay
//! IP can't be resolved the host serves the static roster (honest gate, §7).
//!
//! **KDC-MESH-2 (2026-07-04) — directed discovery over the mesh roster.** Each
//! shunt tick now folds neighbors' published overlay IPs (every host's + every
//! relayed phone's, off `kdc-phones/<host>.json`) into the `OverlayTransport`
//! peer directory, and republishes THIS host's overlay identity + paired phones
//! carrying their overlay IPs. The host then **directed-announces** — a unicast
//! of our identity to each phone's overlay IP over the overlay (design #2) —
//! never a UDP broadcast (which Nebula doesn't carry). A phone whose overlay IP
//! isn't in the roster is honestly `not_discovered` (no broadcast fallback, §7).
//!
//! **KDC-MESH-5 (2026-07-04) — bidirectional mesh notifications.** A phone
//! notification (`kdeconnect.notification`) received on any node is fanned out to
//! EVERY node's desktop feed (design #6): the receiving node republishes it onto
//! its local `event/notify/phone` lane (the CHAT-FIX-2 producer lane the chat
//! worker folds into `alert:<self>`) AND relays it to peers over the mesh-shunt
//! substrate (`<root>/kdc-notify/<host>.json`), which each peer republishes onto
//! its own feed. A bounded per-node seen-set de-dups so one phone notification
//! isn't N toasts on a single desktop. The reverse direction (design #9) forwards
//! the local `event/notify/*` feed to the paired phone as KDE Connect
//! notifications over the overlay transport. Every forwarded action is audited in
//! the hash-chained event log (#16); an unpaired / unreachable phone is an honest
//! no-op (no fake delivery, §7).

#![cfg(feature = "async-services")]

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use mackes_mesh_types::peers::default_workgroup_root;
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::{publish_request, reply_topic};
use mde_kdc_host::error::HostError;
use mde_kdc_host::fanout::{self, FanoutAction, FanoutRequest, FanoutResponse};
use mde_kdc_host::file_browse::SharedRoot;
use mde_kdc_host::pairing::{DeviceRecord, PairingStore};
use mde_kdc_host::service_directory::{self, NodeServices, PublishedRoot};
use mde_kdc_host::sftp::{SftpMount, SshfsMount};
use mde_kdc_host::{EventStream, HostEvent, MeshPairing, OverlayTransport, PeerId, Transport};
use mde_kdc_proto::discovery::{Announce, DeviceType};
use mde_kdc_proto::plugins::battery::BatteryBody;
use mde_kdc_proto::plugins::mousepad::{MouseModifiers, MousepadBody, MousepadEvent};
use mde_kdc_proto::plugins::mpris::{MprisBody, MprisKind, MprisRequestBody};
use mde_kdc_proto::plugins::notification::{notification_packet, NotificationBody};
use serde_json::{json, Value};
use tracing::{debug, error, info, warn};

use super::{ShutdownToken, Worker};
// KDC-MESH-8 — consume the QUASAR-CLOUD openstack worker's PUBLIC Bus verb
// interface (design #12) to drive fleet instance lifecycle from the phone. This
// is a read-only consumer of the public `verbs` surface — the openstack worker
// itself (its state, its responder) is never touched.
use crate::workers::openstack::verbs::{
    cloud_action_topic, CloudInstance, CloudReply, LifecycleAction,
};

// ARCH: this worker was split out of a 5.3K-line god-file into a directory
// module (behavior-preserving relocation). The `Worker` impl + the public entry
// type `KdcHostWorker` + the Connect responder stay here; two cohesive leaf
// clusters live in the child modules. `use <mod>::*` re-exports the `pub(super)`
// items so intra-worker call sites (and the `tests` child) resolve unchanged.
mod cloud;
mod media;
use cloud::*;
use media::*;

/// The Connect verbs served over `action/connect/<verb>` (E2.2 — replacing
/// the retired `dev.mackes.MDE.Connect` D-Bus surface). `version`/`list`/
/// `get` read the store; `pair`/`unpair` mutate it; `ring`/`sms`/
/// `clipboard` enqueue a `Packet` onto the outbound queue.
const CONNECT_VERBS: [&str; 11] = [
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
    // KDC-MESH-7 — ask the phone to start SFTP browsing; the phone replies with a
    // `kdeconnect.sftp` mount-info packet this host mounts (design #11a).
    "sftp",
];

/// Bus topic the worker answers with the live device roster (E2.3 — the same
/// topic the Connect clients (the Workbench panel) already query via
/// `connect::devices()`). Distinct from the `<verb>` action topics.
const DEVICES_TOPIC: &str = "action/connect/devices";

/// KDC-MESH-6 — handoff lane for phone-originated remote input. The KDC worker
/// parses/authorizes KDE Connect packets; a later seat worker owns evdev/uinput.
const REMOTE_INPUT_TOPIC: &str = "action/seat/remote-input";

/// Per-packet event cap so one buggy phone packet cannot flood the local Bus lane
/// before the seat injector has its own backpressure.
const REMOTE_INPUT_EVENT_CAP: usize = 8;

/// KDC-MESH-7 — Bus topic the worker answers with a listing of THIS node's shared
/// files (the node-targeted file browse, design #7/#11b). Served separately from
/// the pure `<verb>` responder because the listing needs the node's shared-roots
/// config. Request body `{"path": "<path>"}` (empty ⇒ list the roots themselves).
const BROWSE_TOPIC: &str = "action/connect/browse";

/// KDC-MESH-4 — Bus request that mints + publishes a fresh mesh invite for the
/// Phones hub Pair QR. The daemon owns credentials; the shell only consumes state.
const MESH_ENROLL_TOKEN_ACTION: &str = "action/connect/mesh-enroll-token";

/// Latest-wins short-TTL mesh invite published for desktop surfaces.
const MESH_ENROLL_TOKEN_TOPIC: &str = "state/connect/mesh-enroll-token";

const MESH_ENROLL_TOKEN_SOURCE: &str = "kdc-host";

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

// ── KDC-MESH-5: bidirectional mesh notifications ─────────────────────────────
//
// Design #6/#9: phone notifications appear on EVERY node's desktop feed (a
// de-duped fan-out over the mesh-shunt substrate), and the CHAT-FIX-2 local-event
// feed (`event/notify/*`) is forwarded to the paired phone as KDE Connect
// notifications over the overlay. Every forwarded action is audited (#16).

/// The CHAT-FIX-2 producer lane a relayed phone notification republishes onto so
/// the chat worker folds it into this node's `alert:<self>` desktop feed. Mirrors
/// [`crate::workers::notify::NOTIFY_TOPIC_PREFIX`]`+ "phone"`.
const NOTIFY_TOPIC_PHONE: &str = "event/notify/phone";

/// The `event/notify/<source>` lanes the CHAT-FIX-2 producer emits that we forward
/// to the paired phone. `phone` is deliberately excluded — it carries inbound
/// phone notifications, and echoing them back would loop.
const MESH_NOTIFY_SOURCES: [&str; 5] = ["peer", "updates", "service", "disk", "journal"];

/// Cap on THIS host's own relay row (newest N phone notifications).
const NOTIFY_RELAY_CAP: usize = 64;

/// A relayed notification older than this (ms) is dropped by the collector so a
/// rejoining node doesn't replay ancient notifications — 5 minutes.
const NOTIFY_RELAY_STALE_MS: i64 = 300_000;

/// Bound on the per-node de-dup seen-set of notification keys.
const NOTIFY_SEEN_CAP: usize = 512;

/// How often the mesh→phone forwarder drains the local `event/notify/*` lanes.
const NOTIFY_FORWARD_TICK: Duration = Duration::from_secs(5);

// ── KDC-MESH-9: the mesh-fanout endpoint (design #8) ─────────────────────────
//
// The designated endpoint advertises as "Quazar Mesh" (one device to stock KDE
// Connect) and relays each follow-everywhere action (a phone clipboard copy / a
// find-my-device ring) to EVERY node, aggregating their responses. The relay rides
// the same own-row replicated substrate as the phone roster + notification relay
// (`mde_kdc_host::fanout`); this worker is the glue that classifies the inbound
// packet, publishes the request when it's the endpoint, and — on every node —
// drains + applies + responds, then aggregates on the endpoint. Every fanned-out
// action audits (#16).

/// Cap on a node's own fanout request/response row (newest N).
const FANOUT_ROW_CAP: usize = 64;

/// A fanout request/response older than this (ms) is ignored — 5 minutes, the same
/// window the notification relay ages out at (a rejoining node doesn't replay
/// ancient actions).
const FANOUT_STALE_MS: i64 = 300_000;

/// Bound on the per-node apply-once seen-set of fanout request ids + the endpoint's
/// recent-request ring it aggregates over.
const FANOUT_SEEN_CAP: usize = 256;

/// The bus root the CHAT-FIX-2 notify producer + the chat folder run on — the
/// per-HOME `data_dir/mde/bus`, identical for every worker in THIS mackesd
/// process (mirrors `notify::default_bus_root` + `chat::default_bus_root`). Using
/// it directly guarantees a republished `event/notify/phone` is folded by chat and
/// a forwarded notify is read from the same lanes the producer writes.
fn notify_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

/// A bounded de-dup ring of notification keys (phone id + notif id + cancel). A
/// key admitted once is suppressed thereafter so one phone notification isn't N
/// toasts on a single desktop — whether it arrives directly from the phone AND via
/// a peer relay, or twice from the phone. Bounded so it can't grow without limit.
#[derive(Default)]
struct NotifySeen {
    recent: VecDeque<String>,
    set: BTreeSet<String>,
}

impl NotifySeen {
    /// Admit `key`: `true` (act on it) the first time, `false` once seen. Keeps the
    /// ring ≤ [`NOTIFY_SEEN_CAP`], evicting the oldest key.
    fn admit(&mut self, key: &str) -> bool {
        if self.set.contains(key) {
            return false;
        }
        self.set.insert(key.to_string());
        self.recent.push_back(key.to_string());
        while self.recent.len() > NOTIFY_SEEN_CAP {
            if let Some(old) = self.recent.pop_front() {
                self.set.remove(&old);
            }
        }
        true
    }

    /// Mark `key` seen without acting (the startup prime, so a restart doesn't
    /// re-toast notifications already on the substrate).
    fn prime(&mut self, key: String) {
        let _ = self.admit(&key);
    }
}

/// A local mesh notification pulled off an `event/notify/<source>` lane, ready to
/// forward to the phone. Parsed from the CHAT-FIX-2 producer's alert-shaped body.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MeshNotify {
    /// The bus ULID — the phone's dedup id for the outbound notification.
    id: String,
    /// The lane suffix (`service`/`disk`/…), shown as the notification title.
    source: String,
    /// The originating host (from the body), part of the ticker line.
    host: String,
    /// The one-line human summary.
    summary: String,
}

/// One inbound phone notification, parsed + pre-rendered for the fan-out.
#[derive(Debug, Clone, PartialEq, Eq)]
struct InboundNotification {
    key: String,
    phone_id: String,
    phone_name: String,
    app_name: String,
    summary: String,
    severity: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Worker-local outbound queue
//
// E2.2 — the canonical `mde-kdc-host` is the lower-level host (LAN transport,
// pairing, TLS) and owns no operator-action send queue, so the queue the
// retired legacy `mde_kdc::outbound` provided lives here, over the canonical
// `mde_kdc_proto::wire::Packet`. Intentionally simple — a `Mutex<Vec<...>>` —
// because the throughput target is operator-scale (clicks per minute). The
// `ring`/`sms`/`clipboard` verbs push here; a future `kdc_outbound` worker (or
// the `OverlayTransport::send_to` path at the 2-device bench) drains it.
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
    /// + delivers each over the live `OverlayTransport`). Poison-tolerant.
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
// The worker runs the canonical `OverlayTransport` (overlay-bound inbound TLS
// listener; no UDP broadcast) and folds its `HostEvent`s into this roster —
// online/battery/name — which it publishes on `action/connect/devices`. The
// shell surfaces become pure clients of the daemon's roster (one host, owned +
// supervised by mackesd).
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
                // KDC-MESH-8 — telephony alert (#12): a phone call/SMS state event
                // reaches the Alert Center as a call notification. A cancel (the call
                // ended) isn't a new alert; disconnected/talking are presence churn we
                // don't toast — ringing + missed are the notable events.
                "kdeconnect.telephony" => telephony_alert(&packet.body),
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
/// `fdo/KDE Connect`; the `chat` worker folds it into the ONE notification
/// interface and federates it mesh-wide over the replicated chat log
/// (NOTIFY-CHAT). Best-effort (open+write+drop; `Persist` is `!Send`).
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

fn publish_kdc_remote_input_events(peer: &PeerId, events: &[MousepadEvent]) -> usize {
    if events.is_empty() {
        return 0;
    }
    let Some(dir) = mde_bus::default_data_dir() else {
        return 0;
    };
    let Ok(persist) = Persist::open(dir) else {
        return 0;
    };
    let ts_ms = now_ms();
    let mut published = 0;
    for event in events.iter().take(REMOTE_INPUT_EVENT_CAP) {
        let body = kdc_remote_input_body(peer, event, ts_ms).to_string();
        if persist
            .write(REMOTE_INPUT_TOPIC, Priority::Default, None, Some(&body))
            .is_ok()
        {
            published += 1;
        }
    }
    published
}

fn kdc_remote_input_body(peer: &PeerId, event: &MousepadEvent, ts_ms: i64) -> Value {
    let mut body = json!({
        "op": "kdc_remote_input",
        "source": "kdc_host",
        "phone": peer.as_str(),
        "ts_unix_ms": ts_ms,
    });
    if let Some(map) = body.as_object_mut() {
        match event {
            MousepadEvent::Move { dx, dy } => {
                map.insert("kind".into(), json!("move"));
                map.insert("dx".into(), json!(dx));
                map.insert("dy".into(), json!(dy));
            }
            MousepadEvent::Scroll { delta } => {
                map.insert("kind".into(), json!("scroll"));
                map.insert("delta".into(), json!(delta));
            }
            MousepadEvent::Button { button, clicks } => {
                map.insert("kind".into(), json!("button"));
                map.insert("button".into(), json!(button.as_str()));
                map.insert("clicks".into(), json!(clicks));
            }
            MousepadEvent::Text { text, modifiers } => {
                map.insert("kind".into(), json!("text"));
                map.insert("text".into(), json!(text));
                map.insert("modifiers".into(), mouse_modifiers_body(*modifiers));
            }
            MousepadEvent::SpecialKey { code, modifiers } => {
                map.insert("kind".into(), json!("special_key"));
                map.insert("special_key".into(), json!(code));
                map.insert("modifiers".into(), mouse_modifiers_body(*modifiers));
            }
        }
    }
    body
}

fn mouse_modifiers_body(modifiers: MouseModifiers) -> Value {
    json!({
        "shift": modifiers.shift,
        "ctrl": modifiers.ctrl,
        "alt": modifiers.alt,
        "super": modifiers.super_key,
    })
}

// ─────────────────────── KDC-MESH-5: notification I/O ctx ─────────────────────

/// The I/O the KDC-MESH-5 notification paths touch, made injectable so the fan-out
/// and forward logic is hermetically testable (tests point `bus_root`/`db_path` at
/// tempdirs). Production reads the CHAT-FIX-2 bus + the hash-chained event store.
struct NotifyCtx {
    /// This node's hostname — the `host` field on republished feed items + the
    /// audit `node_id`.
    hostname: String,
    /// The CHAT-FIX-2 bus root (`event/notify/*`); `None` disables bus I/O.
    bus_root: Option<PathBuf>,
    /// The hash-chained event store (`events::append_and_alert`).
    db_path: PathBuf,
}

impl NotifyCtx {
    /// Production ctx: the per-HOME notify bus + the default event store.
    fn production(hostname: &str) -> Self {
        Self {
            hostname: hostname.to_string(),
            bus_root: notify_bus_root(),
            db_path: crate::default_db_path(),
        }
    }

    /// Publish one notification onto THIS node's local `event/notify/phone` bus lane
    /// — the CHAT-FIX-2 producer lane the chat worker folds into `alert:<self>` (the
    /// desktop notify feed). Same alert-shaped body [`crate::workers::notify`] emits.
    /// Best-effort (open+write+drop; `Persist` is `!Send`).
    fn publish_phone_notify(&self, summary: &str, severity: &str, ts_ms: i64) {
        let Some(root) = self.bus_root.clone() else {
            return;
        };
        let Ok(persist) = Persist::open(root) else {
            return;
        };
        let body = json!({
            "severity": severity,
            "source": "phone",
            "summary": summary,
            "host": self.hostname,
            "ts_unix_ms": ts_ms,
        })
        .to_string();
        if let Err(e) = persist.write(NOTIFY_TOPIC_PHONE, Priority::Default, None, Some(&body)) {
            debug!(error = %e, "kdc-host: phone-notify publish failed");
        }
    }

    /// Audit one forwarded notification action through the hash-chained event log
    /// (#16). Best-effort — [`crate::events::append_and_alert`] logs + swallows a
    /// store fault, so an audit hiccup never wedges the notify path.
    fn audit(&self, detail: Value) {
        crate::events::append_and_alert(
            &self.db_path,
            &self.hostname,
            crate::events::EventKind::Lifecycle,
            detail,
        );
    }

    /// Handle one inbound phone `kdeconnect.notification` (design #6): de-dup, then
    /// republish onto THIS node's desktop feed, relay it to peers over the
    /// mesh-shunt substrate, and audit. A key already seen (a duplicate from the
    /// phone, or one we already surfaced via a peer relay) is a silent no-op — the
    /// de-dup that keeps one phone notification from becoming N toasts on a single
    /// desktop.
    fn fanout_inbound(
        &self,
        root: &std::path::Path,
        seen: &mut NotifySeen,
        n: &InboundNotification,
        now: i64,
    ) {
        if !seen.admit(&n.key) {
            return;
        }
        // 1) THIS node's desktop feed.
        self.publish_phone_notify(&n.summary, &n.severity, now);
        // 2) Relay to every other node over the replicated substrate (they
        //    republish onto their own feeds on their next relay tick).
        let entry = super::mesh_shunt::RelayedNotification {
            key: n.key.clone(),
            phone_id: n.phone_id.clone(),
            phone_name: n.phone_name.clone(),
            app_name: n.app_name.clone(),
            summary: n.summary.clone(),
            severity: n.severity.clone(),
            origin_host: self.hostname.clone(),
            ts_ms: now,
        };
        if let Err(e) =
            super::mesh_shunt::append_notify_relay(root, &self.hostname, &entry, NOTIFY_RELAY_CAP)
        {
            warn!(error = %e, "kdc-host: phone-notify relay publish failed");
        }
        // 3) Audit the forwarded action (#16).
        self.audit(json!({
            "action": "kdc_notify_fanout",
            "direction": "phone_to_desktops",
            "phone": n.phone_id,
            "app": n.app_name,
        }));
    }

    /// Drain neighbors' relayed phone notifications off the substrate and republish
    /// any not-yet-seen one onto THIS node's desktop feed (design #6 fan-out on the
    /// receiving side). Own-row authority + the seen-set keep it loop-free and
    /// de-duped: a node never re-relays what it read (it only writes its OWN row),
    /// and a notification it already surfaced (direct or via another relay) is
    /// skipped. Returns how many it surfaced (observability + tests).
    fn drain_relayed(&self, root: &std::path::Path, seen: &mut NotifySeen, now: i64) -> usize {
        let mut surfaced = 0;
        for entry in super::mesh_shunt::collect_notify_relay(
            root,
            &self.hostname,
            now,
            NOTIFY_RELAY_STALE_MS,
        ) {
            if !seen.admit(&entry.key) {
                continue;
            }
            self.publish_phone_notify(&entry.summary, &entry.severity, now);
            self.audit(json!({
                "action": "kdc_notify_fanout",
                "direction": "relayed_to_desktop",
                "phone": entry.phone_id,
                "origin": entry.origin_host,
            }));
            surfaced += 1;
        }
        surfaced
    }

    /// Forward every newly-produced local mesh notification to each paired phone
    /// over the overlay (design #9). Honest-gated: with no paired phone it's a no-op
    /// (no draining, so a later pairing seeds forward-only — no backlog dump); a
    /// paired but unreachable phone is an honest no-op (no fake delivery, no audit).
    /// Each ACTUAL delivery is audited (#16).
    async fn forward_to_phones(
        &self,
        transport: &OverlayTransport,
        pairing: &Arc<PairingStore>,
        cursors: &mut HashMap<String, String>,
    ) {
        let phones: Vec<PeerId> = pairing
            .records()
            .into_iter()
            .map(|r| PeerId::from(r.device_id.as_str()))
            .collect();
        if phones.is_empty() {
            return; // honest gate: no phone paired → nothing to forward
        }
        for n in self.drain_local_notifies(cursors) {
            let packet = mesh_notify_packet(&n, now_ms());
            for phone in &phones {
                if forward_packet_to_phone(transport, phone, packet.clone()).await {
                    self.audit(json!({
                        "action": "kdc_notify_to_phone",
                        "direction": "mesh_to_phone",
                        "phone": phone.as_str(),
                        "source": n.source,
                        "summary": n.summary,
                    }));
                }
            }
        }
    }

    /// Drain the local `event/notify/<source>` lanes (the CHAT-FIX-2 producer,
    /// excluding the inbound `phone` lane) for notifications newer than each lane's
    /// cursor. First sight of a lane seeds its cursor forward-only (no backlog
    /// replay), mirroring the chat/notify no-backlog contract. Sync (opens + drops
    /// `Persist` without holding it across an await). The startup benign prime
    /// (`"notify monitor online"`) is filtered so it isn't forwarded to the phone.
    fn drain_local_notifies(&self, cursors: &mut HashMap<String, String>) -> Vec<MeshNotify> {
        let Some(root) = self.bus_root.clone() else {
            return Vec::new();
        };
        let Ok(persist) = Persist::open(root) else {
            return Vec::new();
        };
        collect_local_notifies(&persist, cursors)
    }
}

// ───────────────── KDC-MESH-5: phone → desktops (pure helpers) ────────────────

/// Render a phone notification's one-line feed summary (`"App: text"`), collapsing
/// an empty app or empty text. Pure + testable.
fn phone_notify_summary(app: &str, text: &str) -> String {
    let app = app.trim();
    let text = text.trim();
    let sep = if app.is_empty() || text.is_empty() {
        ""
    } else {
        ": "
    };
    format!("{app}{sep}{text}")
}

/// Parse an inbound `kdeconnect.notification` packet into a fan-outable
/// [`InboundNotification`], or `None` to skip it. Cancels (dismissals) are skipped
/// — the desktop feed is an append-only log with no dismiss affordance — as is a
/// content-less notification. The de-dup key falls back to the summary text when
/// the phone omits a stable notification `id`, so distinct notifications don't
/// collapse to one. Pure (no I/O) so the fan-out decision is unit-tested.
fn parse_inbound_notification(
    peer: &PeerId,
    packet: &mde_kdc_proto::wire::Packet,
    phone_name: &str,
) -> Option<InboundNotification> {
    if packet.kind != "kdeconnect.notification" {
        return None;
    }
    let body = &packet.body;
    // A cancel is a dismissal, not a new desktop toast — skip (matches the
    // Alert-Center path's existing cancel handling).
    if body.get("isCancel").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let app = body
        .get("appName")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let text = body
        .get("ticker")
        .or_else(|| body.get("text"))
        .or_else(|| body.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let summary = phone_notify_summary(&app, &text);
    if summary.is_empty() {
        return None;
    }
    let notif_id = body.get("id").and_then(Value::as_str).unwrap_or("");
    // Fall back to the summary when the phone omits a stable id, so two different
    // notifications with no id don't share a key.
    let id_part = if notif_id.is_empty() {
        summary.as_str()
    } else {
        notif_id
    };
    Some(InboundNotification {
        key: super::mesh_shunt::notify_relay_key(peer.as_str(), id_part, false),
        phone_id: peer.as_str().to_string(),
        phone_name: phone_name.to_string(),
        app_name: app,
        summary,
        // Phone notifications ride the feed at Info — present, but they don't
        // hijack the screen with a Warning+ chyron.
        severity: "info".to_string(),
    })
}

// ───────────────── KDC-MESH-9: the mesh-fanout endpoint ───────────────────────

/// Classify an inbound phone packet as a **follow-everywhere** [`FanoutAction`] the
/// endpoint relays to every node (design #6/#10), or `None` for a packet that isn't
/// fanned out. Pure (no I/O) so the endpoint's relay decision is unit-tested. Only
/// the two v1 follow-everywhere actions are fanned out — a clipboard copy and a
/// find-my-device ring — so a copy / ring on the single "Quazar Mesh" device
/// reaches EVERY desktop, not just the endpoint one.
fn fanout_action_for_packet(kind: &str, body: &Value) -> Option<FanoutAction> {
    match kind {
        "kdeconnect.clipboard" | "kdeconnect.clipboard.connect" => {
            let content = body.get("content").and_then(Value::as_str)?;
            // An empty clipboard push isn't worth fanning out.
            (!content.is_empty()).then(|| FanoutAction::Clipboard {
                content: content.to_string(),
            })
        }
        "kdeconnect.findmyphone.request" => Some(FanoutAction::Ring),
        _ => None,
    }
}

/// Apply one fanned-out action on THIS node (design #6/#10) and return a short
/// human detail for the response row. Reuses the same local seams the direct
/// receive path drives (`apply_clipboard` / `ring_local_device`), so a fanned-out
/// clipboard/ring is byte-identical to a directly-received one.
fn apply_fanout_action(action: &FanoutAction) -> String {
    match action {
        FanoutAction::Clipboard { content } => {
            apply_clipboard(content);
            format!("clipboard set ({} bytes)", content.len())
        }
        FanoutAction::Ring => {
            ring_local_device();
            "rang".to_string()
        }
    }
}

/// The endpoint relays one follow-everywhere action to every node: append a
/// [`FanoutRequest`] to this node's own request row (design #8) + audit (#16).
/// Returns the request id so the endpoint can aggregate the responses later. A
/// substrate write failure is an honest log + `None` (no fake fanout).
fn relay_fanout_action(
    shunt_root: &std::path::Path,
    host: &str,
    action: &FanoutAction,
) -> Option<String> {
    let ts = now_ms();
    let id = fanout::request_id(host, ts, action);
    let req = FanoutRequest {
        id: id.clone(),
        action: action.clone(),
        origin_host: host.to_string(),
        ts_ms: ts,
    };
    match fanout::publish_request(shunt_root, host, &req, FANOUT_ROW_CAP) {
        Ok(_) => {
            audit_kdc_action(json!({
                "action": "kdc_fanout_relay",
                "fanout_action": action.tag(),
                "request_id": id,
            }));
            Some(id)
        }
        Err(e) => {
            warn!(error = %e, "kdc-host: fanout relay publish failed");
            None
        }
    }
}

/// Every node drains peers' pending fanout requests (design #8), applies each
/// not-yet-seen action locally, and writes a response row; each application audits
/// (#16). `seen` de-dups so a request is applied exactly once per node even though
/// its row lingers on the substrate across ticks.
fn drain_fanout_requests(shunt_root: &std::path::Path, host: &str, seen: &mut NotifySeen) {
    let now = now_ms();
    for req in fanout::collect_pending_requests(shunt_root, host, now, FANOUT_STALE_MS) {
        if !seen.admit(&req.id) {
            continue;
        }
        let detail = apply_fanout_action(&req.action);
        let resp = FanoutResponse {
            request_id: req.id.clone(),
            node_host: host.to_string(),
            applied: true,
            detail: detail.clone(),
            ts_ms: now_ms(),
        };
        if let Err(e) = fanout::publish_response(shunt_root, host, &resp, FANOUT_ROW_CAP) {
            warn!(error = %e, "kdc-host: fanout response publish failed");
        }
        audit_kdc_action(json!({
            "action": "kdc_fanout_apply",
            "fanout_action": req.action.tag(),
            "request_id": req.id,
            "origin": req.origin_host,
            "detail": detail,
        }));
    }
}

/// The endpoint aggregates the responses to each of its recent fanout requests
/// (design #8 "aggregating responses") and audits the reach (how many nodes
/// applied). Best-effort + observable — the aggregate lands in the hash-chained
/// audit log (#16), so a follow-everywhere action's fleet reach is recorded.
fn aggregate_fanout(shunt_root: &std::path::Path, recent: &VecDeque<String>) {
    let now = now_ms();
    for id in recent {
        let agg = fanout::aggregate_responses(shunt_root, id, now, FANOUT_STALE_MS);
        if agg.responders.is_empty() {
            continue;
        }
        audit_kdc_action(json!({
            "action": "kdc_fanout_aggregate",
            "request_id": id,
            "applied_nodes": agg.applied,
            "responders": agg.responders,
        }));
    }
}

// ───────────────── KDC-MESH-5: mesh → phone (forward) ─────────────────────────

/// The persist-driven core of [`NotifyCtx::drain_local_notifies`] — factored out so
/// it's hermetically testable against a tempdir `Persist`. Drains the local
/// `event/notify/<source>` lanes (the CHAT-FIX-2 producer, excluding the inbound
/// `phone` lane) for notifications newer than each lane's cursor. First sight of a
/// lane seeds its cursor forward-only (no backlog replay), mirroring the chat/notify
/// no-backlog contract. The startup benign prime (`"notify monitor online"`) is
/// filtered so it isn't forwarded to the phone.
fn collect_local_notifies(
    persist: &Persist,
    cursors: &mut HashMap<String, String>,
) -> Vec<MeshNotify> {
    let mut out = Vec::new();
    for source in MESH_NOTIFY_SOURCES {
        let topic = format!("{}{source}", crate::workers::notify::NOTIFY_TOPIC_PREFIX);
        let first_sight = !cursors.contains_key(&topic);
        let since = cursors.get(&topic).cloned();
        let msgs = persist
            .list_since(&topic, since.as_deref())
            .unwrap_or_default();
        if let Some(last) = msgs.last() {
            cursors.insert(topic.clone(), last.ulid.clone());
        }
        if first_sight {
            // Seed the cursor to the current head; don't replay the backlog.
            continue;
        }
        for m in msgs {
            let Some(body) = m.body.as_deref() else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<Value>(body) else {
                continue;
            };
            let summary = v
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if summary.is_empty() || summary == "notify monitor online" {
                continue; // skip the benign per-lane prime
            }
            out.push(MeshNotify {
                id: m.ulid.clone(),
                source: source.to_string(),
                host: v
                    .get("host")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                summary,
            });
        }
    }
    out
}

/// Build the outbound `kdeconnect.notification` packet for a forwarded mesh
/// notification — appears on the phone as a "Quazar Mesh" notification. Pure.
fn mesh_notify_packet(n: &MeshNotify, ts_ms: i64) -> mde_kdc_proto::wire::Packet {
    let title = if n.host.is_empty() {
        format!("Mesh · {}", n.source)
    } else {
        format!("{} · {}", n.host, n.source)
    };
    let ticker = format!("{}: {}", mde_kdc_host::MESH_ENDPOINT_NAME, n.summary);
    notification_packet(
        ts_ms,
        NotificationBody {
            id: n.id.clone(),
            app_name: mde_kdc_host::MESH_ENDPOINT_NAME.to_string(),
            title,
            text: n.summary.clone(),
            ticker,
            is_clearable: true,
            is_cancel: false,
        },
    )
}

/// Forward one already-built packet to a phone over the overlay: prefer the live
/// inbound link (`send_to`), else dial out by the phone's overlay IP (`open`).
/// Returns whether it was actually delivered — an unreachable phone is an honest
/// `false` (no fake delivery), never a queued or faked send.
async fn forward_packet_to_phone(
    transport: &OverlayTransport,
    peer: &PeerId,
    packet: mde_kdc_proto::wire::Packet,
) -> bool {
    if transport.send_to(peer, packet.clone()).await.is_ok() {
        return true;
    }
    match transport.open(peer).await {
        Ok(conn) => {
            let ok = conn.send(packet).await.is_ok();
            conn.close().await;
            ok
        }
        Err(e) => {
            debug!(phone = %peer.as_str(), error = %e, "kdc-host: mesh→phone forward skipped (unreachable)");
            false
        }
    }
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
    // `notification.request` (no inbound handler for notification-pull yet) is
    // dropped.
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
        // Curated command list + execution (handle_runcommand), incl. the
        // KDC-MESH-8 OpenStack lifecycle commands.
        "kdeconnect.runcommand.request",
        // KDC-MESH-7 — the phone's SFTP mount-info reply (browse the phone's FS
        // from this desktop; the request goes out below). Handled by mounting the
        // phone's SFTP server via the injectable seam.
        "kdeconnect.sftp",
        // KDC-MESH-8 — the phone's call/SMS state (telephony alert, #12), surfaced
        // to THIS host's desktop feed + audited.
        "kdeconnect.telephony",
        // KDC-MESH-6 — phone-as-touchpad/keyboard input. Parsed into a bounded
        // Bus handoff for the seat injector; no direct uinput injection here.
        "kdeconnect.mousepad.request",
        // KDC-MESH-8 — a connectivity-report request; answered with THIS host's
        // connectivity summary (#12).
        "kdeconnect.connectivity_report.request",
        // KDC-MESH-6 — phone media transport keys + state pulls, mapped to the
        // host's active MPRIS player through playerctl.
        "kdeconnect.mpris",
        "kdeconnect.mpris.request",
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
        "kdeconnect.mpris",
        // KDC-MESH-7 — ask the phone to start browsing (it replies with the SFTP
        // mount info above).
        "kdeconnect.sftp.request",
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

/// Run the KDE Connect host over the Nebula-overlay-only [`OverlayTransport`]
/// (the inbound TLS listener bound on this node's overlay IP) against the shared
/// pairing store, folding its events into `roster`. Best-effort: an unresolved
/// overlay IP or a transport-start failure logs + returns, leaving the worker
/// serving the seeded (static) roster — never fails worker startup.
async fn run_host(
    pairing: Arc<PairingStore>,
    roster: Roster,
    outbound: PendingSends,
    config_dir: PathBuf,
) {
    let mut announce = local_announce();
    // SEC-5 — the mesh-shunt root + this host's shunt name. Resolved up front (they
    // were previously computed after transport start) so the KDC-MESH-9 endpoint
    // election can run BEFORE the announce moves into the transport.
    let shunt_root = crate::default_qnm_shared_root();
    let shunt_host = hostname_for_shunt();
    // KDC-MESH-9 — elect the mesh-fanout endpoint (design #8): the stable primary
    // (lexicographically-lowest hostname) among the nodes that have published a KDC
    // service-directory row, plus THIS node. The designated endpoint advertises its
    // KDE Connect identity as "Quazar Mesh" — the single device stock KDE Connect
    // shows for the follow-everywhere features — so its inbound clipboard/ring lands
    // here and fans out to the whole mesh. Elected once at start (the advertised KDC
    // name is the TLS-handshake identity, fixed for the link's life); a roster change
    // settles on the next restart. A first-ever boot (empty directory) elects self,
    // so a lone node is honestly its own "Quazar Mesh".
    let mut mesh_hosts: Vec<String> = service_directory::collect_all_services(&shunt_root)
        .into_iter()
        .map(|n| n.node_host)
        .collect();
    mesh_hosts.push(shunt_host.clone());
    let is_endpoint = fanout::is_designated_endpoint(&shunt_host, &mesh_hosts);
    if is_endpoint {
        announce.device_name = fanout::endpoint_device_name(true, &announce.device_name);
        info!(name = %announce.device_name, "kdc-host: this node is the mesh-fanout endpoint");
    }
    // KDC-MESH-2 — this node's KDC device id (its `/etc/machine-id`): published
    // in the mesh-shunt roster as `host_device_id` so neighbors can resolve +
    // dial THIS host by overlay IP (design #2). Captured before the announce
    // moves into the transport.
    let host_device_id = announce.device_id.clone();
    // KDC-MESH-1 — the Nebula-overlay-ONLY transport. The inbound TLS listener
    // binds 1716 on THIS node's overlay IP (resolved from the canonical
    // `/var/lib/mackesd/nebula/overlay-ip`, the QC-6 / `sshd_overlay_bind`
    // source), never `0.0.0.0` / the public NIC, and it dials peers/phones by
    // overlay IP — no LAN direct, no UDP broadcast (design lock #3/#15). Honest
    // gate: if the overlay IP can't be resolved (node not on the mesh yet) the
    // transport is unavailable and we keep serving the seeded (static) roster
    // rather than fall back to a public/localhost bind.
    let transport =
        OverlayTransport::new(announce, Arc::clone(&pairing)).with_listen_port(KDC_PORT);
    let (sink, mut stream) = EventStream::channel();
    if let Err(e) = transport.start(sink).await {
        warn!(error = %e, "kdc-host: overlay transport unavailable; serving static roster");
        return;
    }
    // KDC-MESH-2 — the overlay IP `start` resolved for THIS node. Published in
    // the roster (above) so the mesh can dial us; `None` only if the transport
    // somehow reported unresolved after a successful start (defensive).
    let host_overlay_ip = transport.overlay_status().await.overlay_ip();
    // SEC-5 — the mesh-shunt: publish this peer's paired phones to the replicated
    // volume + relay neighbors' phones into the roster, so a phone paired on another
    // peer shows up here (and is outbound-pairable) without a direct LAN broadcast.
    // (`shunt_root` + `shunt_host` were resolved up front for the KDC-MESH-9 endpoint
    // election above.)
    let shunt_registry = std::sync::Mutex::new(mde_kdc_proto::discovery::DiscoveryRegistry::new());
    let mut shunt_tick = tokio::time::interval(super::mesh_shunt::TICK);
    // AUD-2 — the kdc_outbound drainer: every second, take the operator-queued
    // ring/sms/clipboard/share packets and deliver each over the live overlay
    // transport to its paired device. Failures (device offline / not yet
    // connected) are logged, not retried — operator actions are fire-and-forget.
    let mut drain_tick = tokio::time::interval(OUTBOUND_DRAIN);
    // KDC-MESH-5 — bidirectional mesh notifications. `notify_seen` de-dups the
    // phone→desktops fan-out (primed with what's already on the substrate so a
    // restart doesn't re-toast); `notify_cursors` drives the mesh→phone forwarder
    // (forward-only per lane); `notify_fwd_tick` paces it.
    let notify_ctx = NotifyCtx::production(&shunt_host);
    let mut notify_seen = NotifySeen::default();
    for key in super::mesh_shunt::all_notify_relay_keys(&shunt_root) {
        notify_seen.prime(key);
    }
    let mut notify_cursors: HashMap<String, String> = HashMap::new();
    let mut notify_fwd_tick = tokio::time::interval(NOTIFY_FORWARD_TICK);
    // KDC-MESH-9 — the mesh-fanout state. `fanout_seen` de-dups the apply-once drain
    // (every node applies each request exactly once, even as the row lingers across
    // ticks); `fanout_recent` is the endpoint's ring of request ids it aggregates
    // over each shunt tick. Primed with what's already on the substrate so a restart
    // doesn't re-apply a request it already handled.
    let mut fanout_seen = NotifySeen::default();
    for req in fanout::collect_pending_requests(&shunt_root, &shunt_host, now_ms(), FANOUT_STALE_MS)
    {
        fanout_seen.prime(req.id);
    }
    let mut fanout_recent: VecDeque<String> = VecDeque::new();
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
                // NOTIFY-SRC-3 — surface notable device events to the Alert Center,
                // EXCEPT phone notifications: KDC-MESH-5 fans those out (de-duped)
                // to EVERY node's desktop feed via `event/notify/phone` + the
                // replicated relay below, so routing them here too would double them.
                let is_phone_notification = matches!(
                    &ev,
                    HostEvent::Packet { packet, .. } if packet.kind == "kdeconnect.notification"
                );
                if !is_phone_notification {
                    if let Some((summary, severity)) = kdc_event_alert(&ev) {
                        publish_kdc_alert(&summary, severity);
                    }
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
                    // KDC-PLUGINS / KDC-MESH-8 — Run Command: the phone asks for the
                    // command list (`requestCommandList`) or triggers a curated key.
                    // Results come back as a `kdeconnect.ping` notification. Cloud
                    // (OpenStack lifecycle) keys drive the QC `action/cloud/*` verbs;
                    // every executed command audits (#16). The cloud path is gated on
                    // a paired device (the auth, #16).
                    if packet.kind == "kdeconnect.runcommand.request" {
                        let paired = pairing.is_paired(peer.as_str());
                        handle_runcommand(&transport, &config_dir, peer, &packet.body, paired).await;
                    }
                    // KDC-PLUGINS / KDC-MESH-8 — Battery request: the peer polls THIS
                    // host's battery. Answer with a `kdeconnect.battery` snapshot read
                    // from `/sys/class/power_supply` (a desktop replies cleanly with
                    // the "-1 / not a battery" sentinel); audited (#16).
                    if packet.kind == "kdeconnect.battery.request" {
                        handle_battery_request(&transport, peer).await;
                        audit_kdc_action(json!({
                            "action": "kdc_battery_report",
                            "phone": peer.as_str(),
                        }));
                    }
                    // KDC-MESH-8 — Connectivity report request: reply with THIS host's
                    // connectivity summary (mesh/overlay up, default route, up links)
                    // as a ping notification; audited (#12/#16). Symmetric with the
                    // battery report — the desktop reports its own connectivity.
                    if packet.kind == "kdeconnect.connectivity_report.request" {
                        handle_connectivity_request(&transport, peer).await;
                        audit_kdc_action(json!({
                            "action": "kdc_connectivity_report",
                            "phone": peer.as_str(),
                        }));
                    }
                    // KDC-PLUGINS / KDC-MESH-8 — Find My Phone (both ways, #12): the
                    // peer rings THIS host through the desktop sound path; audited.
                    // The reverse (this host rings the phone) is the `ring` verb.
                    if packet.kind == "kdeconnect.findmyphone.request" {
                        ring_local_device();
                        audit_kdc_action(json!({
                            "action": "kdc_find_my_device",
                            "direction": "phone_rings_desktop",
                            "phone": peer.as_str(),
                        }));
                    }
                    // KDC-MESH-6 — Media transport controls from the phone. The
                    // raw MPRIS action is parsed through the protocol body and then
                    // mapped to a fixed playerctl allowlist.
                    if packet.kind == "kdeconnect.mpris" && pairing.is_paired(peer.as_str()) {
                        if let Ok(body) =
                            mde_kdc_proto::plugins::from_packet_body::<MprisBody>(packet)
                        {
                            let host = hostname_for_shunt();
                            let bus_root = mde_bus::default_data_dir();
                            let browser_status =
                                browser_media_status_from_bus(bus_root.as_deref(), &host);
                            if let Some(command) = apply_browser_mpris_media_command(
                                bus_root.as_deref(),
                                &host,
                                &body,
                                browser_status.as_ref(),
                            ) {
                                audit_kdc_action(json!({
                                    "action": "kdc_media_control",
                                    "phone": peer.as_str(),
                                    "command": command,
                                    "target": "browser",
                                }));
                            } else if let Some(command) =
                                apply_mpris_media_command(&PlayerctlMediaControl, &body)
                            {
                                audit_kdc_action(json!({
                                    "action": "kdc_media_control",
                                    "phone": peer.as_str(),
                                    "command": command,
                                }));
                            }
                        }
                    }
                    // KDC-MESH-6 — MPRIS state pulls / command requests. Stock KDE
                    // Connect asks `kdeconnect.mpris.request` for player lists,
                    // now-playing, volume, and sometimes transport actions; reply
                    // with `kdeconnect.mpris` reports built from playerctl.
                    if packet.kind == "kdeconnect.mpris.request"
                        && pairing.is_paired(peer.as_str())
                    {
                        if let Ok(body) =
                            mde_kdc_proto::plugins::from_packet_body::<MprisRequestBody>(packet)
                        {
                            let host = hostname_for_shunt();
                            let bus_root = mde_bus::default_data_dir();
                            let browser_status =
                                browser_media_status_from_bus(bus_root.as_deref(), &host);
                            if let Some(command) = apply_browser_mpris_request_command(
                                bus_root.as_deref(),
                                &host,
                                &body,
                                browser_status.as_ref(),
                            ) {
                                audit_kdc_action(json!({
                                    "action": "kdc_media_control",
                                    "phone": peer.as_str(),
                                    "command": command,
                                    "target": "browser",
                                }));
                            } else if let Some(command) =
                                apply_mpris_request_command(&PlayerctlMediaControl, &body)
                            {
                                audit_kdc_action(json!({
                                    "action": "kdc_media_control",
                                    "phone": peer.as_str(),
                                    "command": command,
                                }));
                            }
                            let reports = mpris_response_bodies_for_request_with_browser(
                                &PlayerctlMediaControl,
                                &body,
                                browser_status.as_ref(),
                            );
                            for report in reports {
                                let packet = build_packet(
                                    "kdeconnect.mpris",
                                    serde_json::to_value(report)
                                        .expect("MprisBody is always JSON-serializable"),
                                );
                                if let Err(e) = transport.send_to(peer, packet).await {
                                    debug!(
                                        phone = %peer.as_str(),
                                        error = %e,
                                        "kdc-host: MPRIS state report skipped"
                                    );
                                }
                            }
                        }
                    }
                    // KDC-MESH-6 — phone touchpad/keyboard requests. Pairing is
                    // the auth boundary; the worker parses the KDE packet into
                    // bounded local Bus events and leaves evdev/uinput injection
                    // to the seat-facing consumer.
                    if packet.kind == "kdeconnect.mousepad.request"
                        && pairing.is_paired(peer.as_str())
                    {
                        if let Ok(body) =
                            mde_kdc_proto::plugins::from_packet_body::<MousepadBody>(packet)
                        {
                            let events = body.events();
                            let published = publish_kdc_remote_input_events(peer, &events);
                            if published > 0 {
                                audit_kdc_action(json!({
                                    "action": "kdc_remote_input",
                                    "phone": peer.as_str(),
                                    "events": published,
                                }));
                            }
                        }
                    }
                    // KDC-MESH-8 — Telephony alert (#12): a phone call/SMS state event
                    // (ringing / missed / talking) surfaces on THIS host's desktop
                    // feed via `kdc_event_alert` above; audit it here.
                    if packet.kind == "kdeconnect.telephony" {
                        if let Ok(body) = mde_kdc_proto::plugins::from_packet_body::<
                            mde_kdc_proto::plugins::telephony::TelephonyBody,
                        >(packet)
                        {
                            audit_kdc_action(json!({
                                "action": "kdc_telephony_alert",
                                "phone": peer.as_str(),
                                "event": format!("{:?}", body.event),
                                "caller": if body.contact_name.is_empty() {
                                    body.phone_number.clone()
                                } else {
                                    body.contact_name.clone()
                                },
                            }));
                        }
                    }
                    // KDC-PLUGINS / KDC-MESH-8 — Clipboard: a peer's copy (live or the
                    // connection-time `.connect` push) is applied to THIS host's
                    // Wayland clipboard via `wl-copy` when present; audited (#16).
                    if packet.kind == "kdeconnect.clipboard"
                        || packet.kind == "kdeconnect.clipboard.connect"
                    {
                        if let Some(content) = packet.body.get("content").and_then(Value::as_str) {
                            apply_clipboard(content);
                            audit_kdc_action(json!({
                                "action": "kdc_clipboard_apply",
                                "phone": peer.as_str(),
                                "bytes": content.len(),
                            }));
                        }
                    }
                    // KDC-MESH-9 — the mesh-fanout endpoint (design #8): when THIS
                    // node is the designated "Quazar Mesh" endpoint, a follow-
                    // everywhere action it just applied locally (a clipboard copy or a
                    // find-my-device ring, classified from the packet) is ALSO relayed
                    // to every other node, so a copy/ring on the single "Quazar Mesh"
                    // device reaches the whole mesh (#6/#10). The endpoint remembers
                    // the request id to aggregate the responses on the shunt tick.
                    if is_endpoint {
                        if let Some(action) = fanout_action_for_packet(&packet.kind, &packet.body) {
                            if let Some(id) = relay_fanout_action(&shunt_root, &shunt_host, &action) {
                                fanout_recent.push_back(id);
                                while fanout_recent.len() > FANOUT_SEEN_CAP {
                                    fanout_recent.pop_front();
                                }
                            }
                        }
                    }
                    // KDC-MESH-5 — a phone notification (design #6): fan it out,
                    // de-duped, to EVERY node's desktop feed. Republish onto THIS
                    // node's `event/notify/phone` lane + relay it to peers over the
                    // mesh-shunt substrate; audited (#16). The phone's friendly name
                    // (for the feed line) comes off the live roster.
                    if packet.kind == "kdeconnect.notification" {
                        let phone_name = roster
                            .lock()
                            .ok()
                            .and_then(|m| m.get(peer.as_str()).map(|d| d.name.clone()))
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| peer.as_str().to_string());
                        if let Some(n) = parse_inbound_notification(peer, packet, &phone_name) {
                            notify_ctx.fanout_inbound(&shunt_root, &mut notify_seen, &n, now_ms());
                        }
                    }
                    // KDC-MESH-7 — the phone's SFTP mount-info reply to our
                    // `kdeconnect.sftp.request` (design #11a): mount the phone's
                    // filesystem via the injectable seam so its files appear as a
                    // local directory. The mount shells out (`sshfs`) + honest-
                    // gates when absent, so it runs off the reactor; the outcome
                    // is audited (#16).
                    if packet.kind == "kdeconnect.sftp" {
                        if let Ok(info) = mde_kdc_proto::plugins::from_packet_body::<
                            mde_kdc_proto::plugins::sftp::SftpMountInfo,
                        >(packet)
                        {
                            let device = peer.as_str().to_string();
                            tokio::task::spawn_blocking(move || {
                                mount_phone_sftp(&SshfsMount, &device, &info);
                            });
                        }
                    }
                }
                if let Ok(mut m) = roster.lock() {
                    apply_event(&mut m, ev);
                }
            }
            _ = shunt_tick.tick() => {
                // KDC-MESH-2 — relay the roster (SEC-5 names) AND fold neighbors'
                // published overlay IPs into the transport peer directory, then
                // directed-announce over the overlay (no broadcast, #2) to every
                // phone whose overlay IP we now know.
                let phone_ids = run_shunt_tick(
                    &pairing,
                    &roster,
                    &shunt_root,
                    &shunt_host,
                    &shunt_registry,
                    &transport,
                    HostOverlay {
                        device_id: &host_device_id,
                        overlay_ip: host_overlay_ip,
                    },
                );
                directed_announce(&transport, &phone_ids).await;
                // KDC-MESH-5 — pick up neighbors' relayed phone notifications off
                // the substrate and republish any not-yet-seen one onto THIS node's
                // desktop feed (the phone→desktops fan-out, receiving side, #6).
                notify_ctx.drain_relayed(&shunt_root, &mut notify_seen, now_ms());
                // KDC-MESH-7 — publish THIS node's service set + a shallow snapshot
                // of its shared roots to the mesh service directory so the phone/hub
                // can target any node (design #7). Own-row authority (own file).
                let snapshot = node_services_snapshot(
                    &config_dir,
                    &shunt_host,
                    &host_device_id,
                    host_overlay_ip,
                );
                if let Err(e) = service_directory::publish_services(&shunt_root, &snapshot) {
                    warn!(error = %e, "kdc-host: service-directory publish failed");
                }
                // KDC-MESH-9 — the mesh-fanout, receiving + aggregating side (#8).
                // Every node drains peers' pending follow-everywhere requests, applies
                // each not-yet-seen one locally (clipboard/ring), and responds; the
                // endpoint then aggregates its recent requests' responses so a
                // follow-everywhere action's fleet reach lands in the audit log (#16).
                drain_fanout_requests(&shunt_root, &shunt_host, &mut fanout_seen);
                if is_endpoint {
                    aggregate_fanout(&shunt_root, &fanout_recent);
                }
            }
            _ = notify_fwd_tick.tick() => {
                // KDC-MESH-5 — forward the local `event/notify/*` feed (CHAT-FIX-2)
                // to the paired phone as KDE Connect notifications over the overlay
                // (#9). Honest-gated on a paired + reachable phone; audited (#16).
                notify_ctx.forward_to_phones(&transport, &pairing, &mut notify_cursors).await;
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
    transport: &OverlayTransport,
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
        (
            "presenter-next",
            "Presenter next slide",
            "/usr/libexec/mackesd/seat-remote-input '{\"kind\":\"special_key\",\"special_key\":9,\"modifiers\":{\"shift\":false,\"ctrl\":false,\"alt\":false,\"super\":false}}' >/dev/null 2>&1 \
             && echo 'presenter next slide' || echo 'presenter input unavailable'",
        ),
        (
            "presenter-previous",
            "Presenter previous slide",
            "/usr/libexec/mackesd/seat-remote-input '{\"kind\":\"special_key\",\"special_key\":8,\"modifiers\":{\"shift\":false,\"ctrl\":false,\"alt\":false,\"super\":false}}' >/dev/null 2>&1 \
             && echo 'presenter previous slide' || echo 'presenter input unavailable'",
        ),
        (
            "presenter-start",
            "Presenter start",
            "/usr/libexec/mackesd/seat-remote-input '{\"kind\":\"special_key\",\"special_key\":25,\"modifiers\":{\"shift\":false,\"ctrl\":false,\"alt\":false,\"super\":false}}' >/dev/null 2>&1 \
             && echo 'presenter started' || echo 'presenter input unavailable'",
        ),
        (
            "presenter-exit",
            "Presenter exit",
            "/usr/libexec/mackesd/seat-remote-input '{\"kind\":\"special_key\",\"special_key\":14,\"modifiers\":{\"shift\":false,\"ctrl\":false,\"alt\":false,\"super\":false}}' >/dev/null 2>&1 \
             && echo 'presenter exited' || echo 'presenter input unavailable'",
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
///
/// KDC-MESH-8: the published list carries the shell commands PLUS the fleet
/// OpenStack lifecycle commands ([`cloud_command_entries`]). A `cloud-*` key is
/// routed through the QC `action/cloud/*` Bus verbs ([`handle_cloud_command`]) —
/// gated on a `paired` device (the auth, #16) — instead of the shell. Every
/// executed command audits (#16).
async fn handle_runcommand(
    transport: &OverlayTransport,
    config_dir: &std::path::Path,
    peer: &PeerId,
    body: &Value,
    paired: bool,
) {
    let shell_cmds = load_runcommands(config_dir);
    if body.get("requestCommandList").and_then(Value::as_bool) == Some(true) {
        // The phone-visible list = the shell commands + the cloud lifecycle set.
        let mut all = shell_cmds.clone();
        all.extend(cloud_command_entries());
        let list = command_list_json(&all);
        let pkt = build_packet("kdeconnect.runcommand", json!({ "commandList": list }));
        if let Err(e) = transport.send_to(peer, pkt).await {
            warn!(error = %e, "kdc-host: runcommand list send failed");
        }
        return;
    }
    if let Some(key) = body.get("key").and_then(Value::as_str) {
        // KDC-MESH-8 — a cloud lifecycle key drives the fleet OpenStack verbs over
        // the Bus (paired-gated), not the shell.
        if let Some(cmd) = CloudCommand::from_key(key) {
            if !paired {
                let pkt = build_packet(
                    "kdeconnect.ping",
                    json!({ "message": "Cloud commands require a paired device" }),
                );
                let _ = transport.send_to(peer, pkt).await;
                return;
            }
            handle_cloud_command(transport, peer, cmd).await;
            return;
        }
        let key = key.to_string();
        let audit_key = key.clone();
        let result = tokio::task::spawn_blocking(move || execute_runcommand(&shell_cmds, &key))
            .await
            .unwrap_or_else(|_| "command execution failed".to_string());
        info!(device = %peer.as_str(), "kdc-host: ran phone command");
        audit_kdc_action(json!({
            "action": "kdc_runcommand",
            "phone": peer.as_str(),
            "key": audit_key,
        }));
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
async fn handle_battery_request(transport: &OverlayTransport, peer: &PeerId) {
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

/// Wall-clock milliseconds since the epoch (roster freshness stamps +
/// directed-announce packet ids).
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

// ───────────────── KDC-MESH-7: two-way files + service directory ──────────────
//
// Each node publishes its KDC service set + a shallow snapshot of its shared
// roots to the replicated service directory (`<root>/kdc-services/<host>.json`);
// the phone/hub browse it to target any node (design #7). Files ride both ways:
// browse the phone's FS via the injectable SFTP seam (#11a), and browse a node's
// shared files via the directory snapshot + the live `browse` verb (#11b).

/// The `[[root]] label=… path=…` shared-roots document (`<config>/shared-roots.toml`).
#[derive(Debug, Default, serde::Deserialize)]
struct SharedRootsFile {
    #[serde(default)]
    root: Vec<SharedRootEntry>,
}

/// One operator-configured shared root row in `shared-roots.toml`.
#[derive(Debug, serde::Deserialize)]
struct SharedRootEntry {
    label: String,
    path: String,
}

/// The node's browseable shared roots: `<config_dir>/shared-roots.toml` when
/// present + parseable, else the default `~/Public` (only when it exists, so an
/// empty list honestly means "nothing shared" rather than a phantom root). Only
/// roots whose path actually exists are returned (§7 — never a phantom share).
fn default_shared_roots(config_dir: &std::path::Path) -> Vec<SharedRoot> {
    let configured: Vec<SharedRoot> =
        match std::fs::read_to_string(config_dir.join("shared-roots.toml")) {
            Ok(text) => toml::from_str::<SharedRootsFile>(&text)
                .map(|f| {
                    f.root
                        .into_iter()
                        .map(|r| SharedRoot::new(r.label, r.path))
                        .collect()
                })
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
    let roots = if configured.is_empty() {
        dirs::home_dir()
            .map(|h| {
                vec![SharedRoot::new(
                    "Public",
                    h.join("Public").to_string_lossy().to_string(),
                )]
            })
            .unwrap_or_default()
    } else {
        configured
    };
    // Only expose roots that actually exist (an honest, non-phantom share).
    roots
        .into_iter()
        .filter(|r| std::path::Path::new(&r.path).is_dir())
        .collect()
}

/// The KDC service tokens THIS node advertises in the mesh service directory.
///
/// KDC-MESH-7 tokens: `files` (+ `sftp` for the phone-FS mount) plus the
/// already-live `run-commands` / `battery` / `find-my-device`. KDC-MESH-8 extends
/// this with `openstack` + `telephony`.
fn advertised_services() -> Vec<String> {
    vec![
        service_directory::service::FILES.to_string(),
        service_directory::service::RUN_COMMANDS.to_string(),
        service_directory::service::BATTERY.to_string(),
        service_directory::service::FIND_MY_DEVICE.to_string(),
        // The phone-FS SFTP mount seam is always advertised; the live leg
        // honest-gates when `sshfs` is absent (it's a capability, not a promise
        // the tool is installed).
        service_directory::service::SFTP.to_string(),
        // KDC-MESH-8 — fleet OpenStack lifecycle (drives the QC `action/cloud/*`
        // verbs) + telephony (call/SMS) alerts.
        service_directory::service::OPENSTACK.to_string(),
        service_directory::service::TELEPHONY.to_string(),
    ]
}

/// Build THIS node's service-directory entry: its identity + overlay IP, the
/// advertised services, and a shallow snapshot of each shared root's top level
/// (so a phone/hub browses the first level straight off the substrate).
fn node_services_snapshot(
    config_dir: &std::path::Path,
    host: &str,
    device_id: &str,
    overlay_ip: Option<IpAddr>,
) -> NodeServices {
    let shared_roots = default_shared_roots(config_dir)
        .into_iter()
        .map(|root| {
            let entries = mde_kdc_host::file_browse::list_dir(std::path::Path::new(&root.path))
                .unwrap_or_default();
            PublishedRoot { root, entries }
        })
        .collect();
    NodeServices {
        node_host: host.to_string(),
        node_device_id: device_id.to_string(),
        overlay_ip: overlay_ip.map(|ip| ip.to_string()),
        services: advertised_services(),
        shared_roots,
        updated_ms: now_ms(),
    }
}

/// The local mountpoint the phone's SFTP filesystem is mounted at:
/// `<runtime|cache|tmp>/mde/kdc-sftp/<device>`. The device id is sanitized to a
/// single path segment so a hostile id can't escape the mount dir.
fn kdc_sftp_mountpoint(device_id: &str) -> PathBuf {
    let base = dirs::runtime_dir()
        .or_else(dirs::cache_dir)
        .unwrap_or_else(std::env::temp_dir);
    let safe: String = device_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    base.join("mde").join("kdc-sftp").join(safe)
}

/// Append one phone-triggered action to the KDC hash-chained audit log (design
/// #16 — pairing is the auth, but EVERY action is recorded). Best-effort:
/// [`crate::events::append_and_alert`] logs + swallows a store fault so an audit
/// hiccup never wedges the action path.
fn audit_kdc_action(detail: Value) {
    crate::events::append_and_alert(
        &crate::default_db_path(),
        &hostname_for_shunt(),
        crate::events::EventKind::Lifecycle,
        detail,
    );
}

/// Mount a phone's SFTP server (design #11a) via the injectable `seam`, auditing
/// the outcome. A not-mountable reply / absent `sshfs` is an honest gate — logged
/// + audited as `gated`, never a faked mount. Sync (the seam shells out); the
/// caller runs it off the reactor via `spawn_blocking`.
fn mount_phone_sftp<M: SftpMount>(
    seam: &M,
    device_id: &str,
    info: &mde_kdc_proto::plugins::sftp::SftpMountInfo,
) {
    let mountpoint = kdc_sftp_mountpoint(device_id);
    match seam.mount(info, &mountpoint) {
        Ok(m) => {
            info!(device = %device_id, mountpoint = %m.mountpoint.display(), "kdc-host: mounted phone SFTP");
            audit_kdc_action(json!({
                "action": "kdc_sftp_mount",
                "phone": device_id,
                "mountpoint": m.mountpoint.to_string_lossy(),
                "remote": m.remote_path,
            }));
        }
        Err(e) => {
            debug!(device = %device_id, error = %e, "kdc-host: SFTP mount honest-gated");
            audit_kdc_action(json!({
                "action": "kdc_sftp_mount",
                "phone": device_id,
                "result": "gated",
                "reason": e.to_string(),
            }));
        }
    }
}

/// Serve one `action/connect/browse` request: list THIS node's shared files at
/// the requested `path` (the node-targeted file browse, design #7/#11b), auditing
/// the browse (#16). A path outside the shared roots is refused (the security
/// gate). Returns the reply JSON.
fn serve_browse(config_dir: &std::path::Path, body: &Value) -> String {
    let path = body.get("path").and_then(Value::as_str).unwrap_or_default();
    let roots = default_shared_roots(config_dir);
    audit_kdc_action(json!({
        "action": "kdc_file_browse",
        "path": path,
    }));
    match mde_kdc_host::file_browse::browse(&roots, path) {
        Ok(entries) => json!({ "ok": true, "path": path, "entries": entries }).to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}

// ── KDC-MESH-8: run-commands (OpenStack lifecycle) + telephony + connectivity ──
//
// The phone triggers fleet OpenStack lifecycle commands (design #12) that drive
// the QC `action/cloud/*` typed verbs over the Bus — consuming the openstack
// worker's PUBLIC interface, never touching the worker. Battery + connectivity
// report on desktops, telephony alerts surface, find-my-device works both ways;
// pairing is the auth but EVERY action audits (#16, `audit_kdc_action`).

// ── KDC-MESH-8: telephony alerts + connectivity report ───────────────────────

/// Classify an inbound `kdeconnect.telephony` body into an Alert-Center
/// `(summary, severity)`, or `None` to skip. Ringing + missed are the notable
/// call events (surfaced to the desktop feed); talking/disconnected are presence
/// churn, and a cancel (call ended) isn't a new alert. Pure + testable.
fn telephony_alert(body: &Value) -> Option<(String, &'static str)> {
    use mde_kdc_proto::plugins::telephony::{TelephonyBody, TelephonyEvent};
    let parsed: TelephonyBody = serde_json::from_value(body.clone()).ok()?;
    if parsed.is_cancel {
        return None;
    }
    let who = if !parsed.contact_name.is_empty() {
        parsed.contact_name
    } else if !parsed.phone_number.is_empty() {
        parsed.phone_number
    } else {
        "unknown".to_string()
    };
    match parsed.event {
        TelephonyEvent::Ringing => Some((format!("Incoming call from {who}"), "warn")),
        TelephonyEvent::Missed => Some((format!("Missed call from {who}"), "warn")),
        TelephonyEvent::Talking | TelephonyEvent::Disconnected => None,
    }
}

/// Read a `/sys/class/net/<iface>/operstate` file, returning whether the link is
/// `up`. Best-effort — a missing/unreadable node is not up.
fn iface_is_up(iface: &str) -> bool {
    std::fs::read_to_string(format!("/sys/class/net/{iface}/operstate"))
        .map(|s| s.trim() == "up")
        .unwrap_or(false)
}

/// Count the non-loopback network interfaces reporting `operstate == up`.
fn up_interface_count() -> usize {
    let Ok(entries) = std::fs::read_dir("/sys/class/net") else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name != "lo" && iface_is_up(name))
        .count()
}

/// Whether the host has a default route (IPv4 or IPv6) — parsed from
/// `/proc/net/route` (a `00000000` destination) / `/proc/net/ipv6_route`.
fn has_default_route() -> bool {
    let v4 = std::fs::read_to_string("/proc/net/route")
        .map(|t| {
            t.lines().skip(1).any(|l| {
                let mut cols = l.split_whitespace();
                cols.next(); // iface
                cols.next().map(|dest| dest == "00000000") == Some(true)
            })
        })
        .unwrap_or(false);
    let zeros = "0".repeat(32);
    let v6 = std::fs::read_to_string("/proc/net/ipv6_route")
        .map(|t| {
            t.lines()
                .any(|l| l.split_whitespace().next() == Some(zeros.as_str()))
        })
        .unwrap_or(false);
    v4 || v6
}

/// Render THIS host's connectivity summary (design #12) from its inputs. Pure +
/// testable — the live reader ([`local_connectivity_summary`]) supplies the
/// system state.
fn format_connectivity(overlay_up: bool, default_route: bool, up_ifaces: usize) -> String {
    let mesh = if overlay_up {
        "on the mesh"
    } else {
        "OFF the mesh"
    };
    let route = if default_route {
        "internet routable"
    } else {
        "no default route"
    };
    format!("Connectivity: {mesh}, {route}, {up_ifaces} link(s) up")
}

/// This host's live connectivity summary: overlay/mesh reachability (the QC-6
/// overlay-ip publish file resolves), a default route, and the count of up links.
fn local_connectivity_summary() -> String {
    let overlay_up = mde_kdc_host::resolve_overlay_ip(std::path::Path::new(
        mde_kdc_host::DEFAULT_OVERLAY_IP_PATH,
    ))
    .is_resolved();
    format_connectivity(overlay_up, has_default_route(), up_interface_count())
}

/// Answer a `kdeconnect.connectivity_report.request` with THIS host's
/// connectivity summary as a ping notification (design #12).
async fn handle_connectivity_request(transport: &OverlayTransport, peer: &PeerId) {
    let summary = local_connectivity_summary();
    let pkt = build_packet("kdeconnect.ping", json!({ "message": summary }));
    if let Err(e) = transport.send_to(peer, pkt).await {
        warn!(error = %e, "kdc-host: connectivity report send failed");
    }
}

/// THIS host's overlay identity as it publishes it in the mesh-shunt roster:
/// its KDC device id (`/etc/machine-id`) + the overlay IP `start` resolved.
#[derive(Clone, Copy)]
struct HostOverlay<'a> {
    device_id: &'a str,
    overlay_ip: Option<IpAddr>,
}

/// One mesh-shunt pass (SEC-5 + KDC-MESH-2): fold neighbors' relayed phones
/// into the display roster AND their published overlay IPs (phones + hosts)
/// into the [`OverlayTransport`] peer directory, then republish THIS host's
/// overlay identity + paired phones (each tagged with the phone's overlay IP
/// when the directory now knows it). Returns the set of phone device ids this
/// host may directed-announce to — its locally-paired phones plus every relayed
/// phone (hosts are excluded; we announce to phones, not hosts).
fn run_shunt_tick(
    pairing: &Arc<PairingStore>,
    roster: &Roster,
    root: &std::path::Path,
    hostname: &str,
    registry: &std::sync::Mutex<mde_kdc_proto::discovery::DiscoveryRegistry>,
    transport: &OverlayTransport,
    host: HostOverlay<'_>,
) -> BTreeSet<String> {
    let now = now_ms();
    // SEC-5 name relay (unchanged): neighbors' phones → the display roster.
    let synthetic = super::mesh_shunt::collect_synthetic(root, hostname, now);
    super::mesh_shunt::inject_fresh(registry, synthetic, now);
    if let Ok(reg) = registry.lock() {
        if let Ok(mut m) = roster.lock() {
            for a in reg.take_fresh(now) {
                apply_event(&mut m, HostEvent::PeerDiscovered(a));
            }
        }
    }
    // KDC-MESH-2 — fold neighbors' published overlay IPs (phones AND hosts)
    // into the OverlayTransport peer directory so `open(&PeerId)` dials them
    // directly over the overlay (design #2, no UDP broadcast).
    let overlay = super::mesh_shunt::collect_overlay_directory(root, hostname);
    for (device_id, ip) in overlay.phones.iter().chain(overlay.hosts.iter()) {
        transport.set_peer_overlay_ip(device_id, *ip);
    }
    // KDC-MESH-3 (design #5) — replicate neighbors' PAIRINGS into the local store
    // so THIS node recognizes their phones without re-pairing. Own-row authority:
    // `collect_pairings` skips our own file, `replace_synced` never republishes
    // these, and a pin-less relay is dropped (honest gate). `replace_synced`
    // converges recognition with the live mesh each tick — a pairing that leaves
    // the substrate stops being recognized on the next pass.
    let synced: Vec<MeshPairing> = super::mesh_shunt::collect_pairings(root, hostname)
        .into_iter()
        .map(|c| MeshPairing {
            device_id: c.device_id,
            device_name: c.device_name,
            fingerprint: c.fingerprint,
            paired_at_ms: c.paired_at_ms,
            origin_host: c.origin_host,
        })
        .collect();
    pairing.replace_synced(synced);
    // Republish THIS host's overlay identity + paired phones. Own-row authority:
    // only our own paired phones (`records()` is the local `devices` map, never a
    // synced pairing), each tagged with its overlay IP when the (now updated)
    // directory knows it — else `None`, an honest gate (not dialable). KDC-MESH-3
    // (#5): each row also carries our pinned cert fingerprint + paired-at so a
    // neighbor recognizes the phone without re-pairing (the pin is the public cert
    // hash, not a secret).
    let dir_snapshot: HashMap<String, IpAddr> = transport
        .peer_directory()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let mine: Vec<super::mesh_shunt::PublishedDevice> = pairing
        .records()
        .iter()
        .map(|r| super::mesh_shunt::PublishedDevice {
            device_id: r.device_id.clone(),
            device_name: r.device_name.clone(),
            overlay_ip: dir_snapshot.get(&r.device_id).map(IpAddr::to_string),
            fingerprint: r.fingerprint.clone(),
            paired_at_ms: r.paired_at_ms,
        })
        .collect();
    if let Err(e) = super::mesh_shunt::publish_roster(
        root,
        hostname,
        host.device_id,
        host.overlay_ip.map(|ip| ip.to_string()),
        &mine,
    ) {
        warn!(error = %e, "kdc-host: mesh-shunt publish failed");
    }
    // The phones we may directed-announce to: our locally-paired phones plus
    // every relayed phone. Hosts are deliberately excluded.
    let mut phone_ids: BTreeSet<String> =
        pairing.records().into_iter().map(|r| r.device_id).collect();
    phone_ids.extend(overlay.phones.into_iter().map(|(id, _)| id));
    phone_ids
}

/// Build our `kdeconnect.identity` announce packet — the directed-discovery
/// payload unicast to a phone over the overlay (design #2), the mesh-native
/// replacement for stock KDE Connect's UDP identity broadcast (which Nebula
/// doesn't carry).
fn identity_packet(announce: &Announce) -> mde_kdc_proto::wire::Packet {
    mde_kdc_proto::wire::Packet {
        id: now_ms(),
        kind: "kdeconnect.identity".to_string(),
        body: serde_json::to_value(announce).unwrap_or(Value::Null),
        ..Default::default()
    }
}

/// KDC-MESH-2 directed-announce **target selection** (design #2): of the phones
/// this host knows (`phone_ids`), pick exactly those whose overlay IP the peer
/// directory has resolved — each is dialed directly at that overlay IP (a
/// directed unicast), never a UDP broadcast. A phone absent from the directory
/// is honestly skipped (its `open` would be `not_discovered`, §7); there is no
/// broadcast fallback (KDC-MESH-1's posture). Sorted for a deterministic sweep.
fn directed_announce_targets(
    phone_ids: &BTreeSet<String>,
    directory: &HashMap<String, IpAddr>,
) -> Vec<(PeerId, IpAddr)> {
    let mut out: Vec<(PeerId, IpAddr)> = phone_ids
        .iter()
        .filter_map(|id| directory.get(id).map(|ip| (PeerId::from(id.as_str()), *ip)))
        .collect();
    out.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    out
}

/// KDC-MESH-2 directed announce over the overlay: to every phone whose overlay
/// IP we know ([`directed_announce_targets`]), open a directed connection at
/// that overlay IP and send our identity — the mesh-native replacement for the
/// UDP identity broadcast (design #2). Best-effort: a phone not yet paired on
/// THIS node (`not_paired`) or unreachable is logged at debug and skipped,
/// never broadcast to.
async fn directed_announce(transport: &OverlayTransport, phone_ids: &BTreeSet<String>) {
    let targets = {
        let dir = transport.peer_directory();
        let guard = dir.lock().unwrap_or_else(PoisonError::into_inner);
        directed_announce_targets(phone_ids, &guard)
    };
    if targets.is_empty() {
        return;
    }
    let identity = identity_packet(transport.local_announce());
    for (peer, ip) in targets {
        match transport.open(&peer).await {
            Ok(conn) => {
                if let Err(e) = conn.send(identity.clone()).await {
                    debug!(phone = %peer.as_str(), overlay_ip = %ip, error = %e, "kdc-host: directed announce send failed");
                } else {
                    debug!(phone = %peer.as_str(), overlay_ip = %ip, "kdc-host: directed announce");
                }
                conn.close().await;
            }
            Err(e) => {
                debug!(phone = %peer.as_str(), overlay_ip = %ip, error = %e, "kdc-host: directed announce dial skipped");
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
        // KDC-MESH-7 — ask a paired phone to start SFTP browsing. Enqueues a
        // `kdeconnect.sftp.request{startBrowsing:true}`; the phone replies with a
        // `kdeconnect.sftp` mount-info packet the host mounts (design #11a). Same
        // outbound-queue delivery path as ring/share.
        "sftp" => {
            let Some(id) = dev_id() else {
                return json!({ "ok": false, "error": "NoSuchDevice" }).to_string();
            };
            if !store.is_paired(&id) {
                return json!({ "ok": false, "error": "NoSuchDevice" }).to_string();
            }
            let packet = mde_kdc_proto::plugins::sftp::sftp_request_packet(now_ms(), true);
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

/// The daemon-published mesh enroll token state consumed by the Phones hub Pair QR.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct MeshEnrollTokenState {
    token: String,
    expires_at_ms: i64,
    source: &'static str,
    recorded: bool,
}

fn mesh_enroll_token_state(issued: &crate::onboard::invite::IssuedInvite) -> MeshEnrollTokenState {
    MeshEnrollTokenState {
        token: issued.qr.clone(),
        expires_at_ms: i64::try_from(issued.invite.exp_ms).unwrap_or(i64::MAX),
        source: MESH_ENROLL_TOKEN_SOURCE,
        recorded: issued.recorded,
    }
}

fn mint_mesh_enroll_token_state(
    workgroup_root: &Path,
    node_id: &str,
    ttl: Duration,
) -> io::Result<MeshEnrollTokenState> {
    let mesh_id = crate::onboard::invite::resolve_mesh_id(workgroup_root, node_id);
    let issued = crate::onboard::invite::issue(workgroup_root, &mesh_id, ttl)?;
    Ok(mesh_enroll_token_state(&issued))
}

fn serve_mesh_enroll_token(persist: &Persist, workgroup_root: &Path, node_id: &str) -> String {
    let ttl = Duration::from_secs(crate::onboard::invite::DEFAULT_TTL_MINUTES * 60);
    match mint_mesh_enroll_token_state(workgroup_root, node_id, ttl) {
        Ok(state) => {
            let body = serde_json::to_string(&state).unwrap_or_else(|_| "{}".to_string());
            let _ = persist.write(
                MESH_ENROLL_TOKEN_TOPIC,
                Priority::Default,
                None,
                Some(&body),
            );
            json!({
                "ok": true,
                "token": state.token,
                "expires_at_ms": state.expires_at_ms,
                "source": state.source,
                "recorded": state.recorded,
            })
            .to_string()
        }
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}

/// The `action/connect/*` Bus responder loop. Sync (the verb handlers +
/// `Persist` are all sync), so it runs on its own `std::thread` and stops
/// when `stop` is set. Mirrors `mde-session`'s poll responder.
fn serve_connect_bus(
    persist: &Persist,
    store: &PairingStore,
    roster: &Roster,
    outbound: &PendingSends,
    config_dir: &std::path::Path,
    stop: &AtomicBool,
) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    let workgroup_root = default_workgroup_root();
    let node_id = hostname_for_shunt();
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
        // KDC-MESH-7 — the node-targeted file browse (`action/connect/browse`):
        // list THIS node's shared files at the requested path (security-gated to
        // the shared roots), audited. Served here (not via the pure verb handler)
        // because it needs the node's shared-roots config.
        if let Ok(msgs) =
            persist.list_since(BROWSE_TOPIC, cursors.get(BROWSE_TOPIC).map(String::as_str))
        {
            for msg in msgs {
                cursors.insert(BROWSE_TOPIC.to_string(), msg.ulid.clone());
                let body: Value = msg
                    .body
                    .as_deref()
                    .and_then(|b| serde_json::from_str(b).ok())
                    .unwrap_or(Value::Null);
                let reply = serve_browse(config_dir, &body);
                let _ = persist.write(
                    &reply_topic(&msg.ulid),
                    Priority::Default,
                    None,
                    Some(&reply),
                );
            }
        }
        // KDC-MESH-4 — automatic short-TTL mesh invite minting for the Phones
        // hub Pair QR. The daemon records the invite in the bearer ledger, then
        // publishes latest state and replies to the requesting shell.
        if let Ok(msgs) = persist.list_since(
            MESH_ENROLL_TOKEN_ACTION,
            cursors.get(MESH_ENROLL_TOKEN_ACTION).map(String::as_str),
        ) {
            for msg in msgs {
                cursors.insert(MESH_ENROLL_TOKEN_ACTION.to_string(), msg.ulid.clone());
                let reply = serve_mesh_enroll_token(persist, &workgroup_root, &node_id);
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
        // `OverlayTransport` (overlay-bound inbound TLS listener) as a task on the
        // supervisor runtime, folding its events into the roster. Best-effort:
        // an unresolved overlay / start failure leaves the seeded (static) roster served.
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
        let responder_config_dir = self.config_dir.clone();
        let responder = std::thread::Builder::new()
            .name("kdc-connect-bus".into())
            .spawn(move || {
                let Some(bus_root) = mde_bus::default_data_dir() else {
                    warn!("kdc-host: no Bus data dir; Connect actions unavailable");
                    return;
                };
                match Persist::open(bus_root) {
                    Ok(p) => serve_connect_bus(
                        &p,
                        &store,
                        &resp_roster,
                        &outbound,
                        &responder_config_dir,
                        &stop,
                    ),
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
mod tests;
