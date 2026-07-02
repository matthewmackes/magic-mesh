//! NOTIFY-CHAT-2 — the mackesd `chat` worker (design: `docs/design/mesh-chat-icq.md`).
//!
//! The live plumbing behind the pure [`mde_chat`] model: it runs on **every**
//! node (headless included — it emits + relays, it just has no UI) and owns four
//! jobs, all folding into one per-conversation ring and one presence roster:
//!
//!   1. **Live Bus send/recv (lock 1/10).** Drains `action/chat/send` (a UI/CLI
//!      request), mints + **Ed25519-signs** a [`Message`] with this node's
//!      identity key, appends it to this node's own Syncthing log, and relays it
//!      on `event/chat/message`. Inbound `event/chat/message` is **verified**
//!      (bad signature ⇒ dropped) and folded into the right conversation ring.
//!   2. **Syncthing history + offline queue (lock 8/9/22).** Each node writes
//!      only its OWN outbound messages, under
//!      `<workgroup_root>/<self>/chat/out/<key>.json`; a conversation is the
//!      **import-union** of every host's per-key log. A send to an offline peer
//!      stays in the sender's log → the peer backfills when Syncthing replicates
//!      the file. The [`Conversation`] ring dedups by id + orders by
//!      `(ts,sig,id)`, so a live Bus copy and its later backfill fold
//!      identically.
//!   3. **The alert fold (lock 11/20).** It subscribes EVERY existing alert/event
//!      Bus lane ([`ALERT_LANE_PREFIXES`]) and folds each message via
//!      [`mde_chat::fold_alert`] into a [`Message`] from the **originating host**,
//!      dropped into that host's `alert:<host>` conversation — with **no emitter
//!      changes**. Fleet-wide: every node folds every host's alerts.
//!   4. **Presence (lock 5/6/21).** It derives Online/Away/Offline from the
//!      existing mesh-status snapshot (the replicated peer directory), overlays
//!      the operator's manual status (Away/DND/Invisible/Free-for-Chat) gossiped
//!      through a per-host `presence.json`, and republishes a `state/chat/roster`
//!      mirror the `Surface::Chat` UI (NOTIFY-CHAT-3) renders.
//!
//! **State contract the NOTIFY-CHAT-3 UI consumes** (all on the LOCAL Bus, the
//! GUI is a pure renderer):
//!   * `action/chat/send` — the UI's outbound verb: a JSON
//!     `{scope:"peer"|"room", to:"<host|roomid>", text?, kind?}`.
//!   * `event/chat/message` — the signed [`ChatEnvelope`] delivery lane (the fast
//!     path; the durable copy is the Syncthing log).
//!   * `state/chat/roster` — the full [`Roster`] JSON (presence groups).
//!   * `state/chat/conversation/<key>` — the merged ring for one conversation, a
//!     JSON array of [`Message`] (`dm:<a>|<b>`, `room:<id>`, or `alert:<host>`).
//!
//! **Testability (`DoD` §7).** The two seams are the Bus root and the Syncthing
//! (workgroup) root, both injectable to a tempdir; publishing is an in-process
//! [`Persist::write`] so a test drives the whole worker headless with no live
//! mesh. The live 2-node delivery + real Syncthing backfill are integration-gated
//! (they need a running broker federation + Syncthing); the worker logic, the
//! fold, and the offline queue are what land here with unit tests.

#![cfg(feature = "async-services")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use mde_bus::hooks::config::Priority;
use mde_bus::persist::{Persist, StoredMessage};
use mde_chat::{
    fold_alert, sign, Contact, Conversation, Message, MessageId, MessageKind, NodeRole, Presence,
    Roster,
};
use serde::{Deserialize, Serialize};

use super::{ShutdownToken, Worker};

/// The UI's outbound verb: a chat message to send.
pub const ACTION_CHAT_SEND: &str = "action/chat/send";
/// The signed-envelope delivery lane (fast path; the log is the durable copy).
pub const EVENT_CHAT_MESSAGE: &str = "event/chat/message";
/// The presence roster mirror the UI reads.
pub const STATE_CHAT_ROSTER: &str = "state/chat/roster";
/// Prefix for the per-conversation read-model the UI renders.
pub const STATE_CHAT_CONVERSATION_PREFIX: &str = "state/chat/conversation/";

/// The `state/chat/conversation/<key>` topic for one conversation key.
#[must_use]
pub fn conversation_topic(key: &str) -> String {
    format!("{STATE_CHAT_CONVERSATION_PREFIX}{key}")
}

/// Poll cadence — responsive for chat without hammering the Bus index.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// The Bus topic **prefixes** the worker folds into chat (lock 11).
///
/// This is the "subscribe every alert/event lane" set — extending fleet-wide
/// alert coverage is adding a prefix here (no emitter ever changes). Chat's own
/// lanes are excluded by [`is_alert_lane`] so a folded alert never loops back in.
pub const ALERT_LANE_PREFIXES: &[&str] = &[
    "event/security/",
    "fleet/sec",
    "event/firewall",
    "compute/event/",
    "event/compute/",
    "event/kvm/",
    "event/dc/",
    "event/vm/",
    "event/podman/",
    "fdo/",
];

/// Whether `topic` is one of chat's own lanes (never fold these — it would loop).
fn is_chat_lane(topic: &str) -> bool {
    topic.starts_with("event/chat/")
        || topic.starts_with("state/chat/")
        || topic.starts_with("action/chat/")
}

/// Whether `topic` is an alert/event lane the worker folds into a chat message.
#[must_use]
pub fn is_alert_lane(topic: &str) -> bool {
    !is_chat_lane(topic) && ALERT_LANE_PREFIXES.iter().any(|p| topic.starts_with(p))
}

// ── conversation keys ──────────────────────────────────────────────────────

/// The canonical 1:1 conversation key for two hosts — **order-independent** so
/// both parties (and every replicated file) name the same conversation.
#[must_use]
pub fn dm_key(a: &str, b: &str) -> String {
    if a <= b {
        format!("dm:{a}|{b}")
    } else {
        format!("dm:{b}|{a}")
    }
}

/// The conversation key for a room id.
#[must_use]
pub fn room_key(id: &str) -> String {
    format!("room:{id}")
}

/// The conversation key for a host's folded-alert timeline.
#[must_use]
pub fn alert_key(host: &str) -> String {
    format!("alert:{host}")
}

/// Message scope: a 1:1 peer conversation or a multi-party room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// A 1:1 conversation with a peer host.
    Peer,
    /// A named room.
    Room,
}

const fn default_scope() -> Scope {
    Scope::Peer
}

/// The on-Bus delivery envelope for one chat message (`event/chat/message`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatEnvelope {
    /// Whether `to` is a peer host or a room id.
    pub scope: Scope,
    /// The addressee: a peer hostname (1:1) or a room id.
    pub to: String,
    /// The signed message.
    pub message: Message,
}

/// The UI's `action/chat/send` request body.
#[derive(Debug, Clone, Deserialize)]
struct SendRequest {
    #[serde(default = "default_scope")]
    scope: Scope,
    to: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    kind: Option<MessageKind>,
}

impl SendRequest {
    /// Resolve the request into `(scope, to, kind)`; a `kind` wins over `text`,
    /// and a request with neither is rejected (`None`).
    fn resolve(self) -> Option<(Scope, String, MessageKind)> {
        let kind = match self.kind {
            Some(k) => k,
            None => MessageKind::Text(self.text?),
        };
        Some((self.scope, self.to, kind))
    }
}

/// The local conversation key for a peer/room envelope.
///
/// Also encodes whether THIS node is a participant: a 1:1 not involving
/// `self_host` returns `None` (a node stores only conversations it is in). Rooms
/// are open, so every node keys them.
#[must_use]
pub fn local_convo_key(self_host: &str, scope: Scope, to: &str, sender: &str) -> Option<String> {
    match scope {
        Scope::Room => Some(room_key(to)),
        Scope::Peer => {
            if self_host != sender && self_host != to {
                return None;
            }
            let other = if sender == self_host { to } else { sender };
            Some(dm_key(self_host, other))
        }
    }
}

// ── alert fold ─────────────────────────────────────────────────────────────

/// The host an alert is *about*: the payload `host`/`hostname` field, else the
/// local node (so an alert with no host still lands somewhere honest).
fn alert_origin(body: &str, self_host: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("host")
                .or_else(|| v.get("hostname"))
                .and_then(|h| h.as_str())
                .map(str::to_string)
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| self_host.to_string())
}

/// Fold one Bus alert into a chat [`Message`] from its origin host (lock 11).
///
/// The id is made **deterministic** from the source Bus ulid (`alert-<ulid>`) so
/// a re-poll or a restart re-folding the same alert dedups in the ring rather
/// than doubling. It is left **UNSIGNED**: a folded alert is derived from the
/// trusted local Bus, not authored + signed by the origin host, and the model
/// never pretends otherwise (chat human messages are the signed ones). When the
/// payload carries no `ts_unix_ms`, the Bus write-time is used so it orders.
#[must_use]
pub fn alert_message(
    topic: &str,
    bus_ulid: &str,
    body: &str,
    ts_unix_ms: i64,
    self_host: &str,
) -> Message {
    let origin = alert_origin(body, self_host);
    let mut msg = fold_alert(topic, body, &origin);
    if msg.ts_unix_ms == 0 {
        msg.ts_unix_ms = ts_unix_ms;
    }
    msg.id = MessageId::new(format!("alert-{bus_ulid}"));
    msg
}

// ── presence ───────────────────────────────────────────────────────────────

/// Map the mesh-status heartbeat tier (`crate::ipc::directory::presence_tier`)
/// to an auto [`Presence`].
fn presence_from_tier(tier: &str) -> Presence {
    match tier {
        "online" => Presence::Online,
        "idle" => Presence::Away,
        _ => Presence::Offline,
    }
}

/// Map a peer's pinned deployment role to its roster badge.
fn role_from_str(role: Option<&str>) -> NodeRole {
    match role {
        Some("lighthouse") => NodeRole::Lighthouse,
        Some("server") => NodeRole::Headless,
        _ => NodeRole::Workstation,
    }
}

/// A peer's manual-presence gossip, replicated as `<host>/chat/presence.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PresenceGossip {
    /// The operator's manual override on their own node, if any (lock 5).
    #[serde(default)]
    pub manual: Option<Presence>,
    /// An ICQ-style free-text status message (lock 21).
    #[serde(default)]
    pub status_message: Option<String>,
    /// A cosmetic nickname (lock 21).
    #[serde(default)]
    pub nickname: Option<String>,
}

/// One peer's mesh-status snapshot row, fed to [`build_roster`].
#[derive(Debug, Clone)]
pub struct PeerSnapshot {
    /// The peer's hostname (its unforgeable identity + roster key).
    pub host: String,
    /// Its heartbeat tier (`online`/`idle`/`offline`).
    pub tier: String,
    /// Its pinned deployment role, if known.
    pub role: Option<String>,
}

/// Build the [`Roster`] the UI renders (pure).
///
/// Auto presence from the mesh-status tier, overlaid by each host's gossiped
/// manual override + status + nickname (lock 5/21). `self`'s presence is the
/// operator's local manual override, else Online.
#[must_use]
pub fn build_roster(
    self_host: &str,
    peers: &[PeerSnapshot],
    self_manual: Option<Presence>,
    self_status: Option<&str>,
    gossip: &BTreeMap<String, PresenceGossip>,
) -> Roster {
    let mut r = Roster::new(self_host);
    let mut self_c = Contact::new(self_host, NodeRole::Workstation)
        .with_presence(self_manual.unwrap_or(Presence::Online));
    if let Some(s) = self_status {
        self_c = self_c.with_status(s);
    }
    r.upsert(self_c);
    for p in peers {
        if p.host == self_host {
            continue;
        }
        let g = gossip.get(&p.host);
        let presence = g
            .and_then(|g| g.manual)
            .unwrap_or_else(|| presence_from_tier(&p.tier));
        let mut c =
            Contact::new(p.host.as_str(), role_from_str(p.role.as_deref())).with_presence(presence);
        if let Some(g) = g {
            if let Some(s) = &g.status_message {
                c = c.with_status(s.as_str());
            }
            if let Some(n) = &g.nickname {
                c = c.with_nickname(n.as_str());
            }
        }
        r.upsert(c);
    }
    r
}

// ── the Syncthing ring-log store (write-own-file + import-union) ────────────

fn out_dir(root: &Path, host: &str) -> PathBuf {
    root.join(host).join("chat").join("out")
}

fn own_log_path(root: &Path, self_host: &str, key: &str) -> PathBuf {
    out_dir(root, self_host).join(format!("{key}.json"))
}

fn presence_path(root: &Path, host: &str) -> PathBuf {
    root.join(host).join("chat").join("presence.json")
}

/// Read a ring-log file as a message vec (missing/corrupt ⇒ empty — the union
/// tolerates a half-synced or absent file).
fn read_log(path: &Path) -> Vec<Message> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<Message>>(&s).ok())
        .unwrap_or_default()
}

/// Atomic tmp-write + rename, creating the parent dir (the mesh convention).
fn write_atomic(path: &Path, body: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, body.as_bytes())?;
    std::fs::rename(&tmp, path)
}

/// Append my own outbound `msg` to my per-key log, kept bounded + canonically
/// ordered by the [`Conversation`] ring (so my log is itself a valid ring the
/// peer's union folds in identically).
fn append_own(root: &Path, self_host: &str, key: &str, msg: &Message) {
    let path = own_log_path(root, self_host, key);
    let mut conv = Conversation::new(key);
    for m in read_log(&path) {
        conv.insert(m);
    }
    conv.insert(msg.clone());
    let msgs: Vec<&Message> = conv.messages().iter().collect();
    if let Ok(body) = serde_json::to_string(&msgs) {
        if let Err(e) = write_atomic(&path, &body) {
            tracing::warn!(target: "mackesd::chat", key, error = %e, "chat log append failed");
        }
    }
}

/// Load the merged conversation for `key` from the **import-union** of every
/// host's per-key log (lock 22 — one canonical ordered log; the ring dedups +
/// orders so a live copy and a Syncthing backfill fold to the same result).
fn load_conversation(root: &Path, key: &str) -> Conversation {
    let mut conv = Conversation::new(key);
    let Ok(entries) = std::fs::read_dir(root) else {
        return conv;
    };
    for entry in entries.flatten() {
        let path = entry
            .path()
            .join("chat")
            .join("out")
            .join(format!("{key}.json"));
        for m in read_log(&path) {
            conv.insert(m);
        }
    }
    conv
}

/// Every persisted DM/room conversation key across all host subtrees (for the
/// startup rehydrate — alert conversations are forward-only + not persisted).
fn discover_persisted_keys(root: &Path) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    let Ok(hosts) = std::fs::read_dir(root) else {
        return keys;
    };
    for host in hosts.flatten() {
        let Ok(files) = std::fs::read_dir(host.path().join("chat").join("out")) else {
            continue;
        };
        for f in files.flatten() {
            let name = f.file_name();
            let name = name.to_string_lossy();
            if let Some(stem) = name.strip_suffix(".json") {
                keys.insert(stem.to_string());
            }
        }
    }
    keys
}

// ── the worker ─────────────────────────────────────────────────────────────

/// The mackesd `chat` worker (NOTIFY-CHAT-2). Runs on every node.
pub struct ChatWorker {
    self_host: String,
    workgroup_root: PathBuf,
    signing_key: SigningKey,
    poll_interval: Duration,
    bus_root_override: Option<PathBuf>,
    manual_presence: Option<Presence>,
    status_message: Option<String>,
}

impl ChatWorker {
    /// Construct with production defaults. `self_host` is this node's bare
    /// hostname (the roster/DM identity), `signing_key` its persisted node
    /// identity ([`crate::node_key`]).
    #[must_use]
    pub const fn new(workgroup_root: PathBuf, self_host: String, signing_key: SigningKey) -> Self {
        Self {
            self_host,
            workgroup_root,
            signing_key,
            poll_interval: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
            manual_presence: None,
            status_message: None,
        }
    }

    /// Override the Bus root (tests point it at a tempdir Persist).
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    /// Override the poll cadence (tests use a short value).
    #[must_use]
    pub const fn with_poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    /// Seed the operator's manual presence override (lock 5).
    #[must_use]
    pub const fn with_manual_presence(mut self, p: Presence) -> Self {
        self.manual_presence = Some(p);
        self
    }

    /// Seed the operator's free-text status message (lock 21).
    #[must_use]
    pub fn with_status_message(mut self, s: impl Into<String>) -> Self {
        self.status_message = Some(s.into());
        self
    }

    /// Rehydrate the in-memory conversations from the Syncthing union so history
    /// survives a restart; each rehydrated key is marked dirty so the first tick
    /// republishes its `state/chat/conversation/<key>` mirror.
    fn bootstrap(&self, state: &mut ChatState) {
        for key in discover_persisted_keys(&self.workgroup_root) {
            let conv = load_conversation(&self.workgroup_root, &key);
            if !conv.is_empty() {
                state.dirty.insert(key.clone());
                state.convos.insert(key, conv);
            }
        }
    }

    /// One poll pass — the headless-testable core (drives the whole worker with
    /// an injected Persist + tempdir root, no tokio timer, no live mesh).
    fn tick_once(&self, persist: &Persist, state: &mut ChatState, now_ms: i64) {
        self.drain_sends(persist, state, now_ms);
        self.drain_inbound(persist, state);
        self.drain_alerts(persist, state);
        self.publish_roster(persist, state);
        flush_dirty(persist, state);
    }

    /// Drain `action/chat/send`: sign, persist to my own log (the durable +
    /// offline-queue copy), relay on `event/chat/message`, fold into memory.
    fn drain_sends(&self, persist: &Persist, state: &mut ChatState, now_ms: i64) {
        for m in take_new(persist, &mut state.cursors, ACTION_CHAT_SEND) {
            let Some(body) = m.body.as_deref() else {
                continue;
            };
            let req = match serde_json::from_str::<SendRequest>(body) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(target: "mackesd::chat", error = %e, "bad action/chat/send body");
                    continue;
                }
            };
            let Some((scope, to, kind)) = req.resolve() else {
                continue;
            };
            let mut msg = Message::new(self.self_host.as_str(), now_ms, kind);
            sign(&mut msg, &self.signing_key);
            let Some(key) = local_convo_key(&self.self_host, scope, &to, &self.self_host) else {
                continue;
            };
            // Durable + offline queue: stays in MY log until the peer backfills
            // it over Syncthing (lock 9).
            append_own(&self.workgroup_root, &self.self_host, &key, &msg);
            // Live fast-path relay (lock 1).
            let env = ChatEnvelope {
                scope,
                to,
                message: msg.clone(),
            };
            if let Ok(body) = serde_json::to_string(&env) {
                publish(persist, EVENT_CHAT_MESSAGE, &body);
            }
            state
                .convos
                .entry(key.clone())
                .or_insert_with(|| Conversation::new(key.as_str()))
                .insert(msg);
            state.dirty.insert(key);
        }
    }

    /// Drain `event/chat/message`: verify the signature (lock 10 — drop forged),
    /// fold peer/room messages this node participates in into memory. Others'
    /// messages are NOT re-persisted (write-own-file); their durable copy is the
    /// sender's Syncthing log, which the union rehydrates on restart.
    fn drain_inbound(&self, persist: &Persist, state: &mut ChatState) {
        for m in take_new(persist, &mut state.cursors, EVENT_CHAT_MESSAGE) {
            let Some(body) = m.body.as_deref() else {
                continue;
            };
            let Ok(env) = serde_json::from_str::<ChatEnvelope>(body) else {
                continue;
            };
            // Our own relay echo — already folded on send.
            if env.message.sender == self.self_host {
                continue;
            }
            if !env.message.verify() {
                tracing::warn!(
                    target: "mackesd::chat",
                    sender = %env.message.sender,
                    "dropping unverified chat message",
                );
                continue;
            }
            let Some(key) =
                local_convo_key(&self.self_host, env.scope, &env.to, &env.message.sender)
            else {
                continue;
            };
            state
                .convos
                .entry(key.clone())
                .or_insert_with(|| Conversation::new(key.as_str()))
                .insert(env.message);
            state.dirty.insert(key);
        }
    }

    /// Drain every alert/event lane ([`ALERT_LANE_PREFIXES`]) and fold each new
    /// message into its origin host's `alert:<host>` conversation (lock 11/20).
    fn drain_alerts(&self, persist: &Persist, state: &mut ChatState) {
        let topics = match persist.list_topics() {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!(target: "mackesd::chat", error = %e, "list_topics failed");
                return;
            }
        };
        for topic in topics.iter().filter(|t| is_alert_lane(t)) {
            for m in take_new(persist, &mut state.cursors, topic) {
                let body = m.body.as_deref().unwrap_or("");
                let msg = alert_message(topic, &m.ulid, body, m.ts_unix_ms, &self.self_host);
                let key = alert_key(&msg.sender);
                state
                    .convos
                    .entry(key.clone())
                    .or_insert_with(|| Conversation::new(key.as_str()))
                    .insert(msg);
                state.dirty.insert(key);
            }
        }
    }

    /// Gossip my manual status + republish the presence roster (lock 5/6/21),
    /// both **only when they change** — so a steady mesh doesn't rewrite the
    /// gossip file every tick (which would churn Syncthing) or spam the roster
    /// topic. The UI reads latest-wins, so a skipped identical publish is free.
    fn publish_roster(&self, persist: &Persist, state: &mut ChatState) {
        let gossip_self = PresenceGossip {
            manual: self.manual_presence,
            status_message: self.status_message.clone(),
            nickname: None,
        };
        if let Ok(body) = serde_json::to_string(&gossip_self) {
            if state.last_gossip.as_deref() != Some(body.as_str())
                && write_atomic(&presence_path(&self.workgroup_root, &self.self_host), &body)
                    .is_ok()
            {
                state.last_gossip = Some(body);
            }
        }
        let peers = self.peer_snapshot();
        let gossip = self.read_peer_gossip();
        let roster = build_roster(
            &self.self_host,
            &peers,
            self.manual_presence,
            self.status_message.as_deref(),
            &gossip,
        );
        if let Ok(body) = serde_json::to_string(&roster) {
            if state.last_roster.as_deref() != Some(body.as_str()) {
                publish(persist, STATE_CHAT_ROSTER, &body);
                state.last_roster = Some(body);
            }
        }
    }

    /// This node's view of the mesh-status snapshot: each peer's heartbeat tier.
    fn peer_snapshot(&self) -> Vec<PeerSnapshot> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        let dir = mackes_mesh_types::peers::peers_dir(&self.workgroup_root);
        mackes_mesh_types::peers::read_peers(&dir)
            .into_iter()
            .map(|r| PeerSnapshot {
                tier: crate::ipc::directory::presence_tier(now_ms, r.last_seen_ms).to_string(),
                host: r.hostname,
                role: r.role,
            })
            .collect()
    }

    /// Read every peer's gossiped manual presence (`<host>/chat/presence.json`).
    fn read_peer_gossip(&self) -> BTreeMap<String, PresenceGossip> {
        let mut out = BTreeMap::new();
        let Ok(hosts) = std::fs::read_dir(&self.workgroup_root) else {
            return out;
        };
        for h in hosts.flatten() {
            let host = h.file_name().to_string_lossy().to_string();
            if host == self.self_host {
                continue;
            }
            if let Some(g) = std::fs::read_to_string(presence_path(&self.workgroup_root, &host))
                .ok()
                .and_then(|s| serde_json::from_str::<PresenceGossip>(&s).ok())
            {
                out.insert(host, g);
            }
        }
        out
    }
}

/// In-memory per-run worker state, carried across ticks.
#[derive(Default)]
struct ChatState {
    /// Per-topic drain cursor (seeded to head on first sight — forward-only, so
    /// a restart never replays the backlog as re-sends or duplicate alerts;
    /// durable history comes from the Syncthing rehydrate, not the Bus).
    cursors: BTreeMap<String, Option<String>>,
    /// In-memory conversations (DM / room / alert), keyed by conversation key.
    convos: BTreeMap<String, Conversation>,
    /// Keys whose `state/chat/conversation/<key>` mirror needs republishing.
    dirty: BTreeSet<String>,
    /// The last published roster JSON — skip republishing an identical roster.
    last_roster: Option<String>,
    /// The last written self-gossip JSON — skip rewriting an identical file
    /// (avoids churning Syncthing every tick).
    last_gossip: Option<String>,
}

/// New messages on `topic` since the cursor, seeding the cursor to the current
/// head on first sight (no backlog replay), then advancing it.
fn take_new(
    persist: &Persist,
    cursors: &mut BTreeMap<String, Option<String>>,
    topic: &str,
) -> Vec<StoredMessage> {
    match cursors.get(topic) {
        None => {
            let head = persist
                .list_since(topic, None)
                .ok()
                .and_then(|m| m.last().map(|x| x.ulid.clone()));
            cursors.insert(topic.to_string(), head);
            Vec::new()
        }
        Some(cur) => {
            let cur = cur.clone();
            let msgs = persist
                .list_since(topic, cur.as_deref())
                .unwrap_or_default();
            if let Some(last) = msgs.last() {
                cursors.insert(topic.to_string(), Some(last.ulid.clone()));
            }
            msgs
        }
    }
}

/// Republish the `state/chat/conversation/<key>` mirror for each conversation
/// touched this tick (the UI's read-model), draining the dirty set.
fn flush_dirty(persist: &Persist, state: &mut ChatState) {
    for key in std::mem::take(&mut state.dirty) {
        if let Some(conv) = state.convos.get(&key) {
            let msgs: Vec<&Message> = conv.messages().iter().collect();
            if let Ok(body) = serde_json::to_string(&msgs) {
                publish(persist, &conversation_topic(&key), &body);
            }
        }
    }
}

/// In-process Bus publish (best-effort). Writing to the local Persist store is
/// the same store the broker + CLI use; whether it federates to peers is the
/// broker's job (the live 2-node reach is integration-gated).
fn publish(persist: &Persist, topic: &str, body: &str) {
    if let Err(e) = persist.write(topic, Priority::Default, None, Some(body)) {
        tracing::debug!(target: "mackesd::chat", topic, error = %e, "chat publish failed");
    }
}

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[async_trait::async_trait]
impl Worker for ChatWorker {
    fn name(&self) -> &'static str {
        "chat"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self.bus_root_override.clone().or_else(default_bus_root) else {
            tracing::debug!(target: "mackesd::chat", "no bus root; worker idle");
            return Ok(());
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(target: "mackesd::chat", error = %e, "persist open failed; worker idle");
                return Ok(());
            }
        };
        let mut state = ChatState::default();
        // Rehydrate history + publish it (and the initial roster) immediately.
        self.bootstrap(&mut state);
        self.publish_roster(&persist, &mut state);
        flush_dirty(&persist, &mut state);
        let mut tick = tokio::time::interval(self.poll_interval);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    self.tick_once(&persist, &mut state, now_unix_ms());
                }
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    fn key() -> SigningKey {
        SigningKey::generate(&mut OsRng)
    }

    fn persist_at(dir: &Path) -> Persist {
        Persist::open(dir.join("bus")).expect("open persist")
    }

    // ── pure helpers ────────────────────────────────────────────────────

    #[test]
    fn dm_key_is_order_independent() {
        assert_eq!(dm_key("eagle", "nyc3"), dm_key("nyc3", "eagle"));
        assert_eq!(dm_key("a", "b"), "dm:a|b");
    }

    #[test]
    fn local_convo_key_routes_from_each_perspective() {
        // Sender's view: keyed by the target.
        assert_eq!(
            local_convo_key("eagle", Scope::Peer, "nyc3", "eagle"),
            Some(dm_key("eagle", "nyc3"))
        );
        // Recipient's view: keyed by the sender — same canonical key.
        assert_eq!(
            local_convo_key("nyc3", Scope::Peer, "nyc3", "eagle"),
            Some(dm_key("eagle", "nyc3"))
        );
        // A 1:1 that doesn't involve me: not my conversation.
        assert_eq!(local_convo_key("fra1", Scope::Peer, "nyc3", "eagle"), None);
        // Rooms are keyed the same on every node.
        assert_eq!(
            local_convo_key("fra1", Scope::Room, "ops", "eagle"),
            Some(room_key("ops"))
        );
    }

    #[test]
    fn alert_lane_matching_excludes_chats_own_lanes() {
        assert!(is_alert_lane("event/security/alert"));
        assert!(is_alert_lane("fdo/MCNF Alerts"));
        assert!(is_alert_lane("event/firewall/host-a"));
        assert!(!is_alert_lane("event/chat/message"));
        assert!(!is_alert_lane("state/chat/roster"));
        assert!(!is_alert_lane("compute/inventory/10.42.0.5"));
    }

    #[test]
    fn alert_message_is_deterministic_unsigned_and_from_origin() {
        let body = r#"{"severity":"critical","host":"nyc3","summary":"cert revoked","action":"action/shell/goto"}"#;
        let a = alert_message("event/security/alert", "01ABC", body, 1_000, "eagle");
        let b = alert_message("event/security/alert", "01ABC", body, 2_000, "eagle");
        assert_eq!(a.id, b.id, "same Bus ulid ⇒ same id (dedups on re-fold)");
        assert_eq!(a.sender, "nyc3", "folded from the origin host, not self");
        assert!(a.signature.is_none(), "folded alerts are unsigned");
        let MessageKind::Alert {
            severity,
            action_verb,
            ..
        } = &a.kind
        else {
            unreachable!("expected an Alert kind");
        };
        assert_eq!(*severity, mde_chat::Severity::Critical);
        assert_eq!(action_verb.as_deref(), Some("action/shell/goto"));
    }

    #[test]
    fn alert_without_host_folds_from_self() {
        let msg = alert_message("fdo/x", "01Z", "plain text", 42, "eagle");
        assert_eq!(msg.sender, "eagle");
        assert_eq!(
            msg.ts_unix_ms, 42,
            "Bus write-time used when payload has none"
        );
    }

    #[test]
    fn build_roster_overlays_manual_gossip_on_auto_tier() {
        let peers = vec![
            PeerSnapshot {
                host: "nyc3".into(),
                tier: "online".into(),
                role: Some("lighthouse".into()),
            },
            PeerSnapshot {
                host: "fra1".into(),
                tier: "offline".into(),
                role: None,
            },
        ];
        let mut gossip = BTreeMap::new();
        // nyc3 is reachable (online) but set itself DND — the manual override wins.
        gossip.insert(
            "nyc3".to_string(),
            PresenceGossip {
                manual: Some(Presence::Dnd),
                status_message: Some("deploying".into()),
                nickname: None,
            },
        );
        let r = build_roster(
            "eagle",
            &peers,
            Some(Presence::ManualAway),
            Some("brb"),
            &gossip,
        );
        assert_eq!(r.get("nyc3").unwrap().presence, Presence::Dnd);
        assert_eq!(
            r.get("nyc3").unwrap().status_message.as_deref(),
            Some("deploying")
        );
        assert_eq!(r.get("nyc3").unwrap().role, NodeRole::Lighthouse);
        assert_eq!(r.get("fra1").unwrap().presence, Presence::Offline);
        // Self carries the operator's own manual override + status.
        assert_eq!(r.self_contact().presence, Presence::ManualAway);
        assert_eq!(r.self_contact().status_message.as_deref(), Some("brb"));
    }

    // ── store: write-own-file + import-union ────────────────────────────

    #[test]
    fn conversation_is_the_union_of_both_parties_logs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let k = dm_key("eagle", "nyc3");
        // Each party writes only its OWN outbound copy under its own subtree.
        let mut a = Message::text("eagle", 10, "hi from eagle");
        sign(&mut a, &key());
        append_own(root, "eagle", &k, &a);
        let mut b = Message::text("nyc3", 20, "hi back from nyc3");
        sign(&mut b, &key());
        append_own(root, "nyc3", &k, &b);
        // The merged conversation folds both, in timestamp order.
        let conv = load_conversation(root, &k);
        let bodies: Vec<&str> = conv
            .messages()
            .iter()
            .filter_map(|m| match &m.kind {
                MessageKind::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(bodies, vec!["hi from eagle", "hi back from nyc3"]);
    }

    #[test]
    fn backfill_of_a_duplicate_id_folds_idempotently() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let k = dm_key("eagle", "nyc3");
        let mut msg = Message::text("nyc3", 20, "once");
        sign(&mut msg, &key());
        // The same message present in both the live copy and a later backfill.
        append_own(root, "nyc3", &k, &msg);
        append_own(root, "nyc3", &k, &msg);
        assert_eq!(load_conversation(root, &k).len(), 1, "dedup by id");
    }

    // ── worker ticks against a tempdir Bus + tempdir Syncthing root ─────

    fn worker(root: &Path) -> ChatWorker {
        ChatWorker::new(root.to_path_buf(), "eagle".into(), key()).with_bus_root(root.join("bus"))
    }

    #[test]
    fn send_signs_persists_and_relays() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let w = worker(root);
        let persist = persist_at(root);
        let mut state = ChatState::default();
        // Tick 1 seeds cursors to head — the send lane is empty so nothing yet.
        w.tick_once(&persist, &mut state, 100);
        // The UI issues a send.
        persist
            .write(
                ACTION_CHAT_SEND,
                Priority::Default,
                None,
                Some(r#"{"scope":"peer","to":"nyc3","text":"hello mesh"}"#),
            )
            .unwrap();
        w.tick_once(&persist, &mut state, 200);

        // Durable: my own outbound log holds the signed message.
        let k = dm_key("eagle", "nyc3");
        let logged = read_log(&own_log_path(root, "eagle", &k));
        assert_eq!(logged.len(), 1);
        assert!(logged[0].verify(), "persisted message is signed + verifies");
        assert_eq!(logged[0].sender, "eagle");

        // Relayed: an envelope landed on event/chat/message.
        let relayed = persist.list_since(EVENT_CHAT_MESSAGE, None).unwrap();
        assert_eq!(relayed.len(), 1);
        let env: ChatEnvelope = serde_json::from_str(relayed[0].body.as_ref().unwrap()).unwrap();
        assert_eq!(env.to, "nyc3");
        assert!(env.message.verify());

        // Read-model: the conversation mirror the UI reads is published.
        let mirror = persist.list_since(&conversation_topic(&k), None).unwrap();
        let msgs: Vec<Message> =
            serde_json::from_str(mirror.last().unwrap().body.as_ref().unwrap()).unwrap();
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn inbound_verified_message_folds_into_the_peer_conversation() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let w = worker(root); // self = eagle
        let persist = persist_at(root);
        let mut state = ChatState::default();
        w.tick_once(&persist, &mut state, 100); // seed cursors

        // A genuine signed message from nyc3 arrives on the delivery lane.
        let mut m = Message::text("nyc3", 150, "ping from nyc3");
        sign(&mut m, &key());
        let env = ChatEnvelope {
            scope: Scope::Peer,
            to: "eagle".into(),
            message: m,
        };
        persist
            .write(
                EVENT_CHAT_MESSAGE,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&env).unwrap()),
            )
            .unwrap();
        w.tick_once(&persist, &mut state, 200);

        let k = dm_key("eagle", "nyc3");
        assert_eq!(state.convos.get(&k).map(Conversation::len), Some(1));
    }

    #[test]
    fn inbound_with_a_bad_signature_is_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let w = worker(root);
        let persist = persist_at(root);
        let mut state = ChatState::default();
        w.tick_once(&persist, &mut state, 100);

        // Sign, then forge the sender — verify() must now fail (lock 10).
        let mut m = Message::text("nyc3", 150, "spoofed");
        sign(&mut m, &key());
        m.sender = "lighthouse".into();
        let env = ChatEnvelope {
            scope: Scope::Peer,
            to: "eagle".into(),
            message: m,
        };
        persist
            .write(
                EVENT_CHAT_MESSAGE,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&env).unwrap()),
            )
            .unwrap();
        w.tick_once(&persist, &mut state, 200);
        assert!(state.convos.is_empty(), "a forged message never folds");
    }

    #[test]
    fn alert_lane_folds_into_the_origin_hosts_conversation() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let w = worker(root);
        let persist = persist_at(root);
        let mut state = ChatState::default();
        // A lane is discovered only once it already has a message; on first
        // sight the worker seeds its cursor to head (forward-only — the startup
        // backlog isn't replayed). Prime the lane, tick to discover+seed, THEN
        // the real alert folds like a live one.
        persist
            .write(
                "event/security/alert",
                Priority::Min,
                None,
                Some(r#"{"severity":"info","host":"nyc3","summary":"pre-existing"}"#),
            )
            .unwrap();
        w.tick_once(&persist, &mut state, 100); // discovers + seeds the lane

        persist
            .write(
                "event/security/alert",
                Priority::Urgent,
                None,
                Some(r#"{"severity":"critical","host":"nyc3","summary":"intrusion"}"#),
            )
            .unwrap();
        w.tick_once(&persist, &mut state, 200);

        let k = alert_key("nyc3");
        let conv = state.convos.get(&k).expect("alert conversation exists");
        assert_eq!(conv.len(), 1);
        assert!(matches!(
            conv.latest().unwrap().kind,
            MessageKind::Alert { .. }
        ));
        // And the read-model mirror is published for the UI.
        assert!(!persist
            .list_since(&conversation_topic(&k), None)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn re_folding_the_same_alert_does_not_double() {
        // A re-poll / restart re-folding the same Bus alert dedups: same source
        // ulid ⇒ same deterministic id ⇒ the ring drops the duplicate.
        let a = alert_message(
            "event/dc/health/etcd",
            "01XYZ",
            r#"{"host":"fra1"}"#,
            10,
            "eagle",
        );
        let b = alert_message(
            "event/dc/health/etcd",
            "01XYZ",
            r#"{"host":"fra1"}"#,
            10,
            "eagle",
        );
        let k = alert_key("fra1");
        let mut conv = Conversation::new(k.as_str());
        conv.insert(a);
        assert!(!conv.insert(b), "same id ⇒ no double");
        assert_eq!(conv.len(), 1);
    }

    #[test]
    fn roster_is_published_for_the_ui() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let w = worker(root).with_manual_presence(Presence::Dnd);
        let persist = persist_at(root);
        let mut state = ChatState::default();
        w.publish_roster(&persist, &mut state);
        let msgs = persist.list_since(STATE_CHAT_ROSTER, None).unwrap();
        let roster: Roster =
            serde_json::from_str(msgs.last().unwrap().body.as_ref().unwrap()).unwrap();
        assert_eq!(roster.self_host(), "eagle");
        assert_eq!(roster.self_contact().presence, Presence::Dnd);
        // The manual override is gossiped for peers to read.
        let gossip = std::fs::read_to_string(presence_path(root, "eagle")).unwrap();
        let g: PresenceGossip = serde_json::from_str(&gossip).unwrap();
        assert_eq!(g.manual, Some(Presence::Dnd));
    }

    #[test]
    fn bootstrap_rehydrates_persisted_history() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let k = dm_key("eagle", "nyc3");
        let mut m = Message::text("eagle", 10, "earlier");
        sign(&mut m, &key());
        append_own(root, "eagle", &k, &m);
        let w = worker(root);
        let mut state = ChatState::default();
        w.bootstrap(&mut state);
        assert_eq!(state.convos.get(&k).map(Conversation::len), Some(1));
        assert!(state.dirty.contains(&k), "rehydrated key republishes");
    }

    #[tokio::test]
    async fn worker_exits_on_shutdown_token() {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = worker(tmp.path()).with_poll_interval(Duration::from_millis(50));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let _ = tx.send(true);
        let r = tokio::time::timeout(Duration::from_secs(3), w.run(token))
            .await
            .expect("worker must exit on shutdown");
        assert!(r.is_ok());
    }
}
