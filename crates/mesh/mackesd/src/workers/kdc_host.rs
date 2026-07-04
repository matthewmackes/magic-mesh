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
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use mde_kdc_host::error::HostError;
use mde_kdc_host::pairing::{DeviceRecord, PairingStore};
use mde_kdc_host::{EventStream, HostEvent, MeshPairing, OverlayTransport, PeerId, Transport};
use mde_kdc_proto::discovery::{Announce, DeviceType};
use mde_kdc_proto::plugins::battery::BatteryBody;
use mde_kdc_proto::plugins::notification::{notification_packet, NotificationBody};
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
/// notification — appears on the phone as a "Quasar Mesh" notification. Pure.
fn mesh_notify_packet(n: &MeshNotify, ts_ms: i64) -> mde_kdc_proto::wire::Packet {
    let title = if n.host.is_empty() {
        format!("Mesh · {}", n.source)
    } else {
        format!("{} · {}", n.host, n.source)
    };
    let ticker = format!("Quasar Mesh: {}", n.summary);
    notification_packet(
        ts_ms,
        NotificationBody {
            id: n.id.clone(),
            app_name: "Quasar Mesh".to_string(),
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
    let announce = local_announce();
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
    // SEC-5 — the mesh-shunt: publish this peer's paired phones to the
    // replicated volume + relay neighbors' phones into the roster, so a
    // phone paired on another peer shows up here (and is outbound-
    // pairable) without a direct LAN broadcast.
    let shunt_root = crate::default_qnm_shared_root();
    let shunt_host = hostname_for_shunt();
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
    transport: &OverlayTransport,
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

    // ── KDC-MESH-5: bidirectional mesh notifications ─────────────────────────

    fn notif_pkt(id: &str, app: &str, ticker: &str) -> mde_kdc_proto::wire::Packet {
        build_packet(
            "kdeconnect.notification",
            json!({ "id": id, "appName": app, "ticker": ticker }),
        )
    }

    fn test_ctx(host: &str, tmp: &std::path::Path) -> NotifyCtx {
        NotifyCtx {
            hostname: host.to_string(),
            bus_root: Some(tmp.join(format!("bus-{host}"))),
            db_path: tmp.join(format!("mded-{host}.db")),
        }
    }

    fn phone_lane_len(bus_root: &std::path::Path) -> usize {
        Persist::open(bus_root.to_path_buf())
            .unwrap()
            .list_since(NOTIFY_TOPIC_PHONE, None)
            .map(|v| v.len())
            .unwrap_or(0)
    }

    fn audit_row_count(db_path: &std::path::Path) -> usize {
        let conn = crate::store::open(db_path).unwrap();
        crate::store::load_audit_rows(&conn).unwrap().len()
    }

    #[test]
    fn phone_notify_summary_collapses_empty_parts() {
        assert_eq!(phone_notify_summary("Signal", "hi"), "Signal: hi");
        assert_eq!(phone_notify_summary("", "hi"), "hi");
        assert_eq!(phone_notify_summary("Signal", ""), "Signal");
        assert_eq!(phone_notify_summary("  ", "  "), "");
    }

    #[test]
    fn parse_inbound_notification_parses_skips_cancel_and_keys_distinctly() {
        let peer = PeerId::from("moto");
        let n =
            parse_inbound_notification(&peer, &notif_pkt("m1", "Signal", "new message"), "Moto")
                .expect("a content notification parses");
        assert_eq!(n.summary, "Signal: new message");
        assert_eq!(n.phone_id, "moto");
        assert_eq!(n.severity, "info");
        assert!(n.key.contains("moto") && n.key.contains("m1"));
        // A cancel (dismissal) is skipped — no desktop toast.
        let cancel = build_packet(
            "kdeconnect.notification",
            json!({ "id": "m1", "isCancel": true }),
        );
        assert!(parse_inbound_notification(&peer, &cancel, "Moto").is_none());
        // A content-less notification is skipped.
        assert!(parse_inbound_notification(&peer, &notif_pkt("m2", "", ""), "Moto").is_none());
        // Distinct notification ids ⇒ distinct de-dup keys.
        let a = parse_inbound_notification(&peer, &notif_pkt("m1", "S", "x"), "Moto").unwrap();
        let b = parse_inbound_notification(&peer, &notif_pkt("m2", "S", "x"), "Moto").unwrap();
        assert_ne!(a.key, b.key);
        // A wrong packet kind isn't a notification.
        assert!(parse_inbound_notification(
            &peer,
            &build_packet("kdeconnect.ping", json!({})),
            "Moto"
        )
        .is_none());
    }

    #[test]
    fn notify_seen_dedups_and_is_bounded() {
        let mut seen = NotifySeen::default();
        assert!(seen.admit("k1"), "first sight acts");
        assert!(!seen.admit("k1"), "second sight suppressed");
        // Flood past the cap; the ring stays bounded and old keys are evicted.
        for i in 0..(NOTIFY_SEEN_CAP * 2) {
            seen.admit(&format!("f{i}"));
        }
        assert!(seen.recent.len() <= NOTIFY_SEEN_CAP, "seen ring is bounded");
    }

    #[test]
    fn mesh_notify_packet_builds_a_kdeconnect_notification() {
        let n = MeshNotify {
            id: "01ULID".into(),
            source: "service".into(),
            host: "nyc3".into(),
            summary: "service sshd.service failed".into(),
        };
        let pkt = mesh_notify_packet(&n, 42);
        assert_eq!(pkt.kind, "kdeconnect.notification");
        let body: NotificationBody =
            mde_kdc_proto::plugins::from_packet_body(&pkt).expect("decodes");
        assert_eq!(body.app_name, "Quasar Mesh");
        assert_eq!(body.id, "01ULID");
        assert_eq!(body.text, "service sshd.service failed");
        assert!(body.title.contains("nyc3") && body.title.contains("service"));
        assert!(!body.is_cancel);
    }

    #[test]
    fn phone_notification_fans_out_and_dedups_across_nodes() {
        // Two nodes (A + B) share ONE replicated relay root (the substrate) but each
        // has its own local bus + audit store. A phone notification received on A
        // must appear on B's desktop feed exactly once — and never twice even if B
        // also receives it directly or drains the relay again.
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("shared");
        let ctx_a = test_ctx("nodeA", tmp.path());
        let ctx_b = test_ctx("nodeB", tmp.path());
        let mut seen_a = NotifySeen::default();
        let mut seen_b = NotifySeen::default();

        let peer = PeerId::from("moto");
        let n = parse_inbound_notification(&peer, &notif_pkt("m1", "Signal", "ping!"), "Moto")
            .expect("parses");

        // Node A receives it: republishes to A's feed + relays it + audits.
        ctx_a.fanout_inbound(&root, &mut seen_a, &n, 1_000);
        assert_eq!(
            phone_lane_len(ctx_a.bus_root.as_deref().unwrap()),
            1,
            "the notification is on node A's desktop feed"
        );
        assert!(
            audit_row_count(&ctx_a.db_path) >= 1,
            "the fan-out is audited (#16)"
        );
        // It's on the replicated substrate for peers to pick up.
        assert_eq!(
            crate::workers::mesh_shunt::collect_notify_relay(
                &root,
                "nodeB",
                1_200,
                NOTIFY_RELAY_STALE_MS
            )
            .len(),
            1,
            "node B sees A's relayed notification on the substrate"
        );

        // Node B drains the relay: the notification appears on B's feed (fan-out).
        assert_eq!(ctx_b.drain_relayed(&root, &mut seen_b, 1_200), 1);
        assert_eq!(
            phone_lane_len(ctx_b.bus_root.as_deref().unwrap()),
            1,
            "the phone notification fanned out to node B's desktop feed"
        );

        // De-dup: draining again surfaces nothing (already seen).
        assert_eq!(ctx_b.drain_relayed(&root, &mut seen_b, 1_300), 0);
        // De-dup across paths: if B ALSO receives it directly from the phone, it's a
        // no-op — one phone notification is never N toasts on a single desktop.
        ctx_b.fanout_inbound(&root, &mut seen_b, &n, 1_400);
        assert_eq!(
            phone_lane_len(ctx_b.bus_root.as_deref().unwrap()),
            1,
            "still exactly one toast on node B after a duplicate direct receipt"
        );
    }

    #[test]
    fn mesh_notify_forward_is_forward_only_and_skips_the_prime() {
        // The mesh→phone drainer forwards only notifications produced AFTER it first
        // saw a lane (no backlog replay), and never forwards the benign prime.
        let tmp = tempdir().unwrap();
        let bus = tmp.path().join("bus");
        let persist = Persist::open(bus.clone()).unwrap();
        let service = format!("{}service", crate::workers::notify::NOTIFY_TOPIC_PREFIX);
        let body = |summary: &str| {
            json!({ "severity": "warning", "source": "service", "summary": summary, "host": "nyc3" })
                .to_string()
        };
        // A backlog message exists before the first drain.
        persist
            .write(
                &service,
                Priority::Default,
                None,
                Some(&body("old failure")),
            )
            .unwrap();
        let mut cursors: HashMap<String, String> = HashMap::new();
        // First drain: forward-only — seeds the cursor, replays nothing.
        assert!(collect_local_notifies(&persist, &mut cursors).is_empty());
        // Now a prime + a real notification land.
        persist
            .write(
                &service,
                Priority::Default,
                None,
                Some(&body("notify monitor online")),
            )
            .unwrap();
        persist
            .write(
                &service,
                Priority::Default,
                None,
                Some(&body("nginx.service failed")),
            )
            .unwrap();
        let got = collect_local_notifies(&persist, &mut cursors);
        assert_eq!(
            got.len(),
            1,
            "only the real, post-cursor notification is forwarded"
        );
        assert_eq!(got[0].summary, "nginx.service failed");
        assert_eq!(got[0].source, "service");
    }

    #[tokio::test]
    async fn mesh_forward_is_honest_noop_when_unpaired() {
        // No paired phone: the forwarder drains nothing (a later pairing seeds
        // forward-only — no backlog dump) and never fakes a delivery or an audit.
        let tmp = tempdir().unwrap();
        let ctx = test_ctx("nodeA", tmp.path());
        let store = Arc::new(PairingStore::open(tmp.path().join("pair-unpaired")).unwrap());
        let transport = OverlayTransport::new(announce("nodeA", "Node A"), Arc::clone(&store));
        // A notification exists on the bus, but with no phone paired nothing forwards.
        let bus = ctx.bus_root.clone().unwrap();
        let persist = Persist::open(bus.clone()).unwrap();
        let service = format!("{}service", crate::workers::notify::NOTIFY_TOPIC_PREFIX);
        persist
            .write(&service, Priority::Default, None, Some(
                &json!({"severity":"warning","source":"service","summary":"x failed","host":"nodeA"}).to_string(),
            ))
            .unwrap();
        let mut cursors: HashMap<String, String> = HashMap::new();
        ctx.forward_to_phones(&transport, &store, &mut cursors)
            .await;
        assert!(
            cursors.is_empty(),
            "no draining occurs with no paired phone"
        );
        assert_eq!(audit_row_count(&ctx.db_path), 0, "no fake-delivery audit");
    }

    #[tokio::test]
    async fn mesh_forward_to_a_paired_but_unreachable_phone_is_an_honest_noop() {
        // A paired phone with no live link and no known overlay IP: the forwarder
        // tries the overlay, fails honestly, and audits NOTHING (no fake delivery).
        let tmp = tempdir().unwrap();
        let ctx = test_ctx("nodeA", tmp.path());
        let store = Arc::new(PairingStore::open(tmp.path().join("pair-unreach")).unwrap());
        store
            .pair(DeviceRecord {
                device_id: "moto".into(),
                device_name: "Moto".into(),
                paired_at_ms: 1,
                fingerprint: String::new(),
            })
            .unwrap();
        let transport = OverlayTransport::new(announce("nodeA", "Node A"), Arc::clone(&store));
        let bus = ctx.bus_root.clone().unwrap();
        let persist = Persist::open(bus.clone()).unwrap();
        let service = format!("{}service", crate::workers::notify::NOTIFY_TOPIC_PREFIX);
        let write_notify = |summary: &str| {
            persist
                .write(
                    &service,
                    Priority::Default,
                    None,
                    Some(
                        &json!({"severity":"warning","source":"service","summary":summary,"host":"nodeA"})
                            .to_string(),
                    ),
                )
                .unwrap();
        };
        let mut cursors: HashMap<String, String> = HashMap::new();
        // A first notification seeds the cursor forward-only (the first drain skips
        // it); a second, post-cursor notification IS drained + delivery attempted —
        // but the phone is unreachable, so nothing is delivered and nothing audited.
        write_notify("old failure");
        ctx.forward_to_phones(&transport, &store, &mut cursors)
            .await;
        write_notify("nginx failed");
        ctx.forward_to_phones(&transport, &store, &mut cursors)
            .await;
        assert_eq!(
            audit_row_count(&ctx.db_path),
            0,
            "an unreachable phone is delivered nothing → nothing audited"
        );
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

    // ── KDC-MESH-2: directed discovery over the mesh roster ──────────────────

    #[test]
    fn kdc_mesh2_roster_feeds_directory_and_selects_the_phone() {
        use std::net::{IpAddr, Ipv4Addr};
        let tmp = tempdir().unwrap();
        let cfg = tmp.path().join("cfg");
        std::fs::create_dir_all(&cfg).unwrap();
        let pairing = Arc::new(PairingStore::open(&cfg).unwrap());
        // Locally pair phone-1 (known mesh-wide) + phone-ghost (no overlay IP).
        for (id, name) in [("phone-1", "Pixel"), ("phone-ghost", "Ghost")] {
            pairing
                .pair(DeviceRecord {
                    device_id: id.into(),
                    device_name: name.into(),
                    paired_at_ms: 1,
                    fingerprint: "AB:CD".into(),
                })
                .unwrap();
        }
        let transport = OverlayTransport::new(local_announce(), Arc::clone(&pairing))
            .with_overlay_ip(IpAddr::V4(Ipv4Addr::LOCALHOST))
            .with_listen_port(0);
        let roster: Roster = Arc::new(Mutex::new(HashMap::new()));
        let registry = std::sync::Mutex::new(mde_kdc_proto::discovery::DiscoveryRegistry::new());

        // A neighbor ("hostA") publishes its overlay identity + phone-1's IP.
        let shared = tmp.path().join("shared");
        super::super::mesh_shunt::publish_roster(
            &shared,
            "hostA",
            "hostA-devid",
            Some("10.42.0.9".into()),
            &[super::super::mesh_shunt::PublishedDevice {
                device_id: "phone-1".into(),
                device_name: "Pixel".into(),
                overlay_ip: Some("10.42.0.77".into()),
                ..Default::default()
            }],
        )
        .unwrap();

        // THIS host's shunt tick: relay + fold the overlay directory + republish.
        let host_ip = IpAddr::V4(Ipv4Addr::new(10, 42, 0, 5));
        let phone_ids = run_shunt_tick(
            &pairing,
            &roster,
            &shared,
            "hostB",
            &registry,
            &transport,
            HostOverlay {
                device_id: "hostB-devid",
                overlay_ip: Some(host_ip),
            },
        );

        // (1) The roster fed the directory: phone-1 + the neighbor host resolve.
        let dir = transport.peer_directory();
        let guard = dir.lock().unwrap();
        assert_eq!(
            guard.get("phone-1"),
            Some(&IpAddr::V4(Ipv4Addr::new(10, 42, 0, 77))),
            "the phone's overlay IP flowed roster→directory"
        );
        assert_eq!(
            guard.get("hostA-devid"),
            Some(&IpAddr::V4(Ipv4Addr::new(10, 42, 0, 9))),
            "the neighbor host's overlay IP flowed roster→directory"
        );

        // (2) Directed-announce target selection: phone-1 at its overlay IP;
        // phone-ghost (no roster IP) is honestly excluded (not_discovered); a
        // host is never a directed-announce target.
        let targets = directed_announce_targets(&phone_ids, &guard);
        assert_eq!(
            targets,
            vec![(
                PeerId::from("phone-1"),
                IpAddr::V4(Ipv4Addr::new(10, 42, 0, 77))
            )],
            "only the known-IP phone is a directed-announce target"
        );
        assert!(
            !targets.iter().any(|(p, _)| p.as_str() == "phone-ghost"),
            "an unknown-IP phone is not_discovered, never a target (no broadcast)"
        );
        assert!(
            !targets.iter().any(|(p, _)| p.as_str() == "hostA-devid"),
            "a host is never a directed-announce target"
        );
        drop(guard);

        // (3) We republished our own overlay identity so a third host learns us.
        let raw = std::fs::read_to_string(
            super::super::mesh_shunt::phones_dir(&shared).join("hostB.json"),
        )
        .unwrap();
        let back = super::super::mesh_shunt::parse_roster(&raw).expect("our roster parses");
        assert_eq!(back.host_device_id, "hostB-devid");
        assert_eq!(back.host_overlay_ip.as_deref(), Some("10.42.0.5"));
    }

    // ── KDC-MESH-3 (#5): mesh-wide pairing replicates through the shunt tick ──

    #[test]
    fn kdc_mesh3_shunt_tick_replicates_a_neighbor_pairing_into_the_store() {
        use std::net::{IpAddr, Ipv4Addr};
        let tmp = tempdir().unwrap();
        let cfg = tmp.path().join("cfg");
        std::fs::create_dir_all(&cfg).unwrap();
        // THIS node has paired NOTHING locally (an honest, unsynced start).
        let pairing = Arc::new(PairingStore::open(&cfg).unwrap());
        assert!(
            !pairing.is_paired("phone-1"),
            "unsynced: honest gate, no trust"
        );

        let transport = OverlayTransport::new(local_announce(), Arc::clone(&pairing))
            .with_overlay_ip(IpAddr::V4(Ipv4Addr::LOCALHOST))
            .with_listen_port(0);
        let roster: Roster = Arc::new(Mutex::new(HashMap::new()));
        let registry = std::sync::Mutex::new(mde_kdc_proto::discovery::DiscoveryRegistry::new());

        // A neighbor (hostA) publishes phone-1 as a PAIRED device (carrying the pin
        // from ITS TOFU) plus a pin-less name-relay phone-ghost.
        let shared = tmp.path().join("shared");
        super::super::mesh_shunt::publish_phones(
            &shared,
            "hostA",
            &[
                super::super::mesh_shunt::PublishedDevice {
                    device_id: "phone-1".into(),
                    device_name: "Pixel".into(),
                    fingerprint: "AA:BB:CC".into(),
                    paired_at_ms: 77,
                    ..Default::default()
                },
                super::super::mesh_shunt::PublishedDevice {
                    device_id: "phone-ghost".into(),
                    device_name: "Ghost".into(),
                    ..Default::default()
                },
            ],
        )
        .unwrap();

        // THIS host's shunt tick folds neighbors' pairings into the local store.
        let _ = run_shunt_tick(
            &pairing,
            &roster,
            &shared,
            "hostB",
            &registry,
            &transport,
            HostOverlay {
                device_id: "hostB-devid",
                overlay_ip: Some(IpAddr::V4(Ipv4Addr::new(10, 42, 0, 5))),
            },
        );

        // phone-1 is now recognized mesh-wide WITHOUT a local pairing, carrying
        // hostA's pin (so the transport enforces the same cert). phone-ghost (no
        // pin) is NOT trusted — the honest gate.
        assert!(
            pairing.is_paired("phone-1"),
            "the neighbor's pairing replicated in"
        );
        assert!(pairing.is_synced("phone-1"));
        assert!(
            !pairing.is_locally_paired("phone-1"),
            "recognized via mesh, not own-row"
        );
        assert_eq!(pairing.get("phone-1").unwrap().fingerprint, "AA:BB:CC");
        assert_eq!(
            pairing.synced_pairing("phone-1").unwrap().origin_host,
            "hostA"
        );
        assert!(
            !pairing.is_paired("phone-ghost"),
            "a pin-less relay stays untrusted"
        );
        // Own-row authority: we never republished the synced pairing as our own.
        let raw = std::fs::read_to_string(
            super::super::mesh_shunt::phones_dir(&shared).join("hostB.json"),
        )
        .unwrap();
        let back = super::super::mesh_shunt::parse_roster(&raw).expect("our roster parses");
        assert!(
            back.devices.is_empty(),
            "synced pairings are not republished"
        );
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
