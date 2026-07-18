//! The Chat surface — the authentic **ICQ** roster + conversation panes
//! (NOTIFY-CHAT-3; design: `docs/design/mesh-chat-icq.md`, locks 4/5/19).
//!
//! Under Mesh Chat every host is a contact and every one of its alerts is a
//! message from that contact (lock 2). This surface is the ICQ face of that
//! model: the **roster** (Online / Offline groups, a per-contact presence dot,
//! a role tag, an ICQ status line, unread bold + count) with a selected
//! contact opening its **conversation pane** — a focused in-shell pane (the DRM
//! shell has no floating windows), showing the merged ring timeline and a
//! composer that emits `action/chat/send`.
//!
//! It is a **pure renderer** over the NOTIFY-CHAT-2 worker's read-model on the
//! LOCAL Bus (the same JSON-boundary discipline as `clipboard.rs` — the shell
//! never depends on the mackesd crate, §6):
//!   * `state/chat/roster` — the full [`Roster`] (presence groups), latest-wins.
//!   * `state/chat/conversation/<key>` — one conversation's ring as a
//!     `Vec<Message>` array, latest-wins. Keys are canonical: `dm:<a>|<b>` for a
//!     1:1 (order-independent), `alert:<host>` for a host's folded alerts.
//!
//! A message is sent by writing `action/chat/send`
//! `{scope:"peer", to:"<host>", text}` back to the Bus.
//!
//! There is **no demo data** — an empty roster on a solo host is the honest
//! render (§7). Live 2-peer delivery + the true per-message worker ack are
//! integration-gated (they need the running worker federation); the delivery
//! checkmark here is the honest presence-derived approximation (see
//! [`Delivery`]).

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use crate::bus_reader::BusReader;
use mde_chat::{
    AlertAction, AlertActionKind, Contact, Conversation, Message, MessageKind, NotifyPrefs,
    Presence, RoomDescriptor, RoomKind, Roster, Severity,
};
use mde_egui::egui::{self, Align, Color32, Layout, RichText, ScrollArea};
use mde_egui::Style;

use crate::discovery::request_host_desktop;
use crate::toast_bridge::{resolve_action, TOAST_TOPIC};

/// Poll cadence — matches the chat worker's own 2s tick so the roster and open
/// conversation stay live without a cold-start wait.
const REFRESH: Duration = Duration::from_secs(2);

/// The presence roster mirror the worker publishes (latest-wins).
const ROSTER_TOPIC: &str = "state/chat/roster";
/// The room-registry mirror the worker publishes — every known room + its
/// membership descriptor (NOTIFY-CHAT-5), latest-wins as a JSON array.
const ROOMS_TOPIC: &str = "state/chat/rooms";
/// The seat's notification-policy mirror the worker publishes (mute state) so the
/// per-contact / per-room mute toggles read the TRUE persisted policy, not a
/// local guess — latest-wins.
const NOTIFY_TOPIC: &str = "state/chat/notify";
/// Prefix for local alert acknowledgements written by the chat worker.
const ALERT_ACK_PREFIX: &str = "state/chat/alert-ack/";
/// Prefix for local alert snoozes written by the chat worker.
const ALERT_SNOOZE_PREFIX: &str = "state/chat/alert-snooze/";
/// Prefix for the per-conversation read-model the worker republishes each change.
const CONVERSATION_PREFIX: &str = "state/chat/conversation/";
/// The UI's outbound verb — a chat message to send.
const ACTION_CHAT_SEND: &str = "action/chat/send";
/// The UI's room-lifecycle verb (NOTIFY-CHAT-5): create / self-join / dissolve a
/// room. `{op:"create"|"join"|"dissolve", id, name?}` — the worker replicates the
/// signed descriptor and enforces creator-only dissolve.
const ACTION_CHAT_ROOM: &str = "action/chat/room";
/// The UI's presence verb (lock 5/21): set this seat's manual presence and/or its
/// free-text status. `{presence?: <manual|null>, status?: <text|null>}` — the
/// worker drains it, updates its self-presence, gossips it, and republishes the
/// self entry on `state/chat/roster`.
const ACTION_CHAT_PRESENCE: &str = "action/chat/presence";
/// The UI's mute verb (NOTIFY-CHAT-5): mute/unmute a contact or room.
/// `{target:"contact"|"room", id, muted}` — the worker updates its `NotifyPrefs`
/// and republishes [`NOTIFY_TOPIC`].
const ACTION_CHAT_MUTE: &str = "action/chat/mute";
/// The UI's notification-policy verb for fields outside the mute target matrix.
const ACTION_CHAT_NOTIFY_PREFS: &str = "action/chat/notify-prefs";
/// The UI's typed alert action verb. The chat worker validates the action kind
/// and forwards safe/armed verbs to the real mackesd action lane.
const ACTION_CHAT_ALERT_ACTION: &str = "action/chat/alert-action";
/// The voice worker's dial verb (lock 15 — Call hands off to `mde-voice`). Chat is
/// the launch point; a running SIP agent draining this is integration-gated.
const ACTION_VOICE_DIAL: &str = "action/voice/dial";
/// The `mde-files` Send-To verb (lock 15) — the sender's mackesd copies the source
/// into the target peer's replicated inbox (the exact wire `bus_backend::send_to`
/// publishes; a §6 JSON boundary, not a crate dep).
const ACTION_FILE_SEND_TO: &str = "action/file-ops/send-to";

/// The `state/chat/conversation/<key>` topic for one conversation key.
fn conversation_topic(key: &str) -> String {
    format!("{CONVERSATION_PREFIX}{key}")
}

/// The canonical order-independent 1:1 key for two hosts — mirrors the worker's
/// `dm_key` so both name the same conversation (a string boundary, not a mackesd
/// dep — §6).
fn dm_key(a: &str, b: &str) -> String {
    if a <= b {
        format!("dm:{a}|{b}")
    } else {
        format!("dm:{b}|{a}")
    }
}

/// The conversation key for a host's folded-alert timeline (mirrors the worker).
fn alert_key(host: &str) -> String {
    format!("alert:{host}")
}

/// The conversation key for a room's shared log (mirrors the worker's `room_key`).
fn room_key(id: &str) -> String {
    format!("room:{id}")
}

/// The in-memory unread watermark key for the aggregate Notifications lane. It is
/// session-only UI state over the live per-host alert conversations.
const NOTIFICATIONS_SEEN_KEY: &str = "__notifications__";

/// One folded alert in the aggregate Notifications lane, carrying its originating
/// contact alongside the message so host attribution survives aggregation.
#[derive(Debug, Clone, Copy)]
struct NotificationItem<'a> {
    host: &'a str,
    msg: &'a Message,
}

/// What the operator has open in the conversation pane: a 1:1 contact (its merged
/// human+alert timeline) or a room (its shared log). A single selection so exactly
/// one pane is open at a time (the ICQ single-window idiom on a DRM seat).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Selection {
    /// Aggregate alert-only lane, newest-first across all host timelines.
    Notifications,
    /// A contact host — its `dm:` ∪ `alert:` timeline.
    Contact(String),
    /// A room id — its `room:<id>` shared log.
    Room(String),
}

/// Every conversation key that folds into a contact's one ICQ timeline (lock 2 —
/// human chat + machine alerts share one timeline per contact).
///
/// A peer contact merges its 1:1 DM ring with the host's folded-alert ring; the
/// self-contact carries only its local alerts/clips (lock 17 — no notes-to-self).
fn keys_for_contact(self_host: &str, contact_host: &str) -> Vec<String> {
    if self_host == contact_host {
        vec![alert_key(self_host)]
    } else {
        vec![dm_key(self_host, contact_host), alert_key(contact_host)]
    }
}

/// The honest three-state delivery indicator for one of *my* outgoing messages
/// (lock 19). Derived from the recipient contact's live presence — an available
/// peer received the live Bus fast-path relay ([`Delivery::Delivered`]); an
/// unavailable peer keeps it in my Syncthing log to backfill on return
/// ([`Delivery::Queued`]); an unknown target is merely [`Delivery::Sent`]. A
/// true per-message worker ack is integration-gated (design lock 19), so this is
/// the presence-derived approximation, never a fabricated "read receipt" (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Delivery {
    /// Emitted, recipient reachability unknown.
    Sent,
    /// The recipient is available — the live relay reached an online worker.
    Delivered,
    /// The recipient is unavailable — queued in the log, backfills on reconnect.
    Queued,
}

impl Delivery {
    /// Derive the state from the recipient contact (its presence), if known.
    const fn for_recipient(recipient: Option<&Contact>) -> Self {
        match recipient {
            None => Self::Sent,
            Some(c) if c.presence.is_available() => Self::Delivered,
            Some(_) => Self::Queued,
        }
    }

    /// Compact text label used in the Chat chrome. This deliberately avoids
    /// pseudo-icon glyphs so delivery state reads consistently across fonts.
    const fn label(self) -> &'static str {
        match self {
            Self::Sent => "Sent",
            Self::Delivered => "Delivered",
            Self::Queued => "Queued",
        }
    }

    const fn hover_text(self) -> &'static str {
        match self {
            Self::Sent => "Sent",
            Self::Delivered => "Delivered",
            Self::Queued => "Queued offline",
        }
    }

    /// The tone: delivered reads OK, queued reads muted (not an error — it will
    /// arrive), sent is the neutral accent.
    const fn color(self) -> Color32 {
        match self {
            Self::Sent => Style::ACCENT,
            Self::Delivered => Style::OK,
            Self::Queued => Style::TEXT_DIM,
        }
    }
}

const CHAT_HINT_SET_STATUS: &str = "Set a status...";
const CHAT_HINT_NEW_ROOM: &str = "New room name...";
const CHAT_HINT_MESSAGE: &str = "Message...";
const CHAT_HINT_ATTACH_FILE: &str = "File path or drop a file here...";
const CHAT_HINT_ROOM_MESSAGE: &str = "Message the room...";
const CHAT_ATTACH_LABEL: &str = "File";
const CHAT_ALERT_GO_TO_LABEL: &str = "Go to";
const CHAT_SENT_FILE_PREFIX: &str = "Sent file";

fn delivery_preview_text(delivery: Delivery, presence: &str) -> String {
    format!("{} - {presence}", delivery.label())
}

fn room_member_note(members: usize) -> String {
    format!("{members} members")
}

/// Map a contact's [`Presence`] to its roster status-dot color (§4 — no raw hex).
const fn presence_color(p: Presence) -> Color32 {
    match p {
        Presence::Online | Presence::FreeForChat => Style::OK,
        Presence::Away | Presence::ManualAway => Style::WARN,
        Presence::Dnd => Style::DANGER,
        Presence::Offline | Presence::Invisible => Style::TEXT_DIM,
    }
}

/// Map a folded-alert [`Severity`] to its `Style` color (§4).
const fn severity_color(s: Severity) -> Color32 {
    match s {
        Severity::Critical => Style::SUPPORT_ERROR,
        Severity::Warning => Style::SUPPORT_WARNING,
        Severity::Info => Style::SUPPORT_INFO,
    }
}

struct ChatMetric<'a> {
    label: &'a str,
    value: String,
    detail: String,
    tone: Color32,
}

fn chat_metric_columns(ui: &mut egui::Ui, metrics: &[ChatMetric<'_>]) {
    let columns = if ui.available_width() < 420.0 {
        1
    } else if ui.available_width() < 760.0 {
        2
    } else {
        metrics.len().clamp(1, 4)
    };
    ui.columns(columns, |cols| {
        for (idx, metric) in metrics.iter().enumerate() {
            let col = &mut cols[idx % columns];
            chat_metric_tile(
                col,
                metric.label,
                &metric.value,
                &metric.detail,
                metric.tone,
            );
            col.add_space(Style::SP_S);
        }
    });
}

fn chat_metric_tile(ui: &mut egui::Ui, label: &str, value: &str, detail: &str, tone: Color32) {
    egui::Frame::group(ui.style())
        .shadow(card_shadow())
        .show(ui, |ui| {
            ui.set_min_height(Style::SP_XL * 3.0);
            ui.label(
                RichText::new(label)
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL)
                    .strong(),
            );
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new(value)
                    .color(tone)
                    .size(Style::HEADING)
                    .strong(),
            );
            ui.add_space(Style::SP_XS);
            mde_egui::muted_note(ui, detail);
        });
}

/// The ICQ self-presence picker options (lock 5) — the operator-settable subset
/// of [`Presence`]. **Available** clears the manual override (→ auto presence),
/// the other four map to a manual [`Presence`]. Kept separate from the model enum
/// so the picker shows exactly the five ICQ choices, never an auto/derived state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PresenceChoice {
    /// Free-for-Chat (manual).
    FreeForChat,
    /// Available — clears the manual override (auto presence).
    Available,
    /// Away (manual).
    Away,
    /// Do-Not-Disturb (manual).
    Dnd,
    /// Invisible (manual).
    Invisible,
}

impl PresenceChoice {
    /// The five options, in ICQ menu order.
    const ALL: [Self; 5] = [
        Self::FreeForChat,
        Self::Available,
        Self::Away,
        Self::Dnd,
        Self::Invisible,
    ];

    /// The picker selection that best represents `p` (an auto state maps to the
    /// closest operator-facing option so the current presence always shows).
    const fn from_presence(p: Presence) -> Self {
        match p {
            Presence::FreeForChat => Self::FreeForChat,
            Presence::Away | Presence::ManualAway => Self::Away,
            Presence::Dnd => Self::Dnd,
            Presence::Invisible => Self::Invisible,
            Presence::Online | Presence::Offline => Self::Available,
        }
    }

    /// The wire tag posted on `action/chat/presence` — matches the worker's
    /// `PresenceSet` `snake_case` names (Available ⇒ clear to auto presence).
    const fn wire(self) -> &'static str {
        match self {
            Self::FreeForChat => "free_for_chat",
            Self::Available => "available",
            Self::Away => "away",
            Self::Dnd => "dnd",
            Self::Invisible => "invisible",
        }
    }

    /// The menu / selected-text label.
    const fn label(self) -> &'static str {
        match self {
            Self::FreeForChat => "Free for Chat",
            Self::Available => "Available",
            Self::Away => "Away",
            Self::Dnd => "Do Not Disturb",
            Self::Invisible => "Invisible",
        }
    }
}

/// The Chat surface state: the last roster + each contact's merged conversation
/// (rebuilt from the latest-wins Bus mirrors each poll), the selected contact,
/// the composer draft, and the per-contact read watermark for unread counts.
#[allow(
    clippy::struct_excessive_bools,
    reason = "the three MENU-2 feed-filter bands + Unread Only + the status-editor \
              toggle are independent view flags, not a state machine"
)]
pub(crate) struct ChatState {
    bus_root: Option<PathBuf>,
    /// The latest roster the worker published, if any.
    roster: Option<Roster>,
    /// host → the contact's merged (DM ∪ alert) ring, rebuilt each refresh.
    convos: BTreeMap<String, Conversation>,
    /// The room registry the worker publishes (system + ad-hoc), latest-wins.
    rooms: Vec<RoomDescriptor>,
    /// The seat's notification policy (mute state) the worker publishes on
    /// [`NOTIFY_TOPIC`], so the mute toggles reflect the TRUE persisted policy
    /// (`None` until the worker first publishes it — a fresh solo host).
    notify: Option<NotifyPrefs>,
    /// The inline self-status editor buffer (lock 21), populated when the editor
    /// opens; committing posts it on `action/chat/presence`.
    status_draft: String,
    /// Whether the self-status inline editor is open (vs. the read-only display).
    editing_status: bool,
    /// room id → its shared log ring, rebuilt each refresh from `room:<id>`.
    room_convos: BTreeMap<String, Conversation>,
    /// The selected contact or room (its conversation pane is open).
    selected: Option<Selection>,
    /// The composer buffer for the open conversation.
    draft: String,
    /// The new-room name buffer for the roster's create affordance (NOTIFY-CHAT-5).
    new_room: String,
    /// The Send-To composer's file path buffer (typed or drag-dropped) — an empty
    /// string hides the attach row's Send button (NOTIFY-CHAT-4, file kind).
    attach_path: String,
    /// host → message count when last viewed; unread = current − watermark. A
    /// host first seen is watermarked at its current length so pre-existing
    /// backfilled history isn't flagged unread (unread = new since you looked).
    seen: BTreeMap<String, usize>,
    /// Alert ids acknowledged on this seat; they stay in host history but leave
    /// the aggregate Notifications lane until a new live alert arrives.
    acked_alerts: BTreeSet<String>,
    /// Alert ids snoozed on this seat; same local lane suppression as ack.
    snoozed_alerts: BTreeSet<String>,
    /// View → feed filter (MENU-2): show folded system alerts in the timeline.
    show_alerts: bool,
    /// View → feed filter: show clipboard clips in the timeline.
    show_clips: bool,
    /// View → feed filter: show human messages (text / files / call + remote
    /// records) in the timeline.
    show_messages: bool,
    /// View → Unread Only (MENU-2): the roster rail lists only conversations
    /// carrying unread (self + the open one stay visible).
    unread_only: bool,
    /// shell-ux-11 — the last Bus-publish failure, surfaced as an inline DANGER
    /// note under the composer (mirrors the datacenter `last_error` idiom). Set
    /// when a send / file-send / mute publish fails so the draft is KEPT and the
    /// operator sees why, instead of the message being silently destroyed; cleared
    /// on the next successful send.
    last_error: Option<String>,
    last_poll: Option<Instant>,
    /// perf-5 — per-contact conversation cursors: each constituent topic's latest
    /// ULID at the last rebuild, in `keys_for_contact` order (keyed by contact
    /// host, matching `convos`). The `state/chat/conversation/<key>` topics are
    /// latest-wins **full-ring** blobs — the worker republishes the whole
    /// `Vec<Message>` on every change — so a naive refresh re-decodes each
    /// contact's entire history on every 2 s poll. Comparing the cheap per-topic
    /// `latest_ulid` against this lets refresh REUSE the already-built
    /// `Conversation` in place and skip the JSON re-parse whenever a contact's
    /// blobs are unchanged; a changed blob (incl. a retention trim, which writes a
    /// new-ULID ring) mismatches the cursor and rebuilds from scratch → resync.
    convo_cursors: BTreeMap<String, Vec<Option<String>>>,
    /// perf-5 — the room-log twin of [`Self::convo_cursors`], keyed by room id
    /// (matching `room_convos`); each room has the single `room:<id>` topic.
    room_cursors: BTreeMap<String, Vec<Option<String>>>,
}

impl Default for ChatState {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            roster: None,
            convos: BTreeMap::new(),
            rooms: Vec::new(),
            notify: None,
            status_draft: String::new(),
            editing_status: false,
            room_convos: BTreeMap::new(),
            selected: None,
            draft: String::new(),
            new_room: String::new(),
            attach_path: String::new(),
            seen: BTreeMap::new(),
            acked_alerts: BTreeSet::new(),
            snoozed_alerts: BTreeSet::new(),
            show_alerts: true,
            show_clips: true,
            show_messages: true,
            unread_only: false,
            last_error: None,
            last_poll: None,
            convo_cursors: BTreeMap::new(),
            room_cursors: BTreeMap::new(),
        }
    }
}

impl ChatState {
    /// Test seam for shell-level fixture integration.
    #[cfg(test)]
    pub(crate) fn with_bus_root(bus_root: PathBuf) -> Self {
        Self {
            bus_root: Some(bus_root),
            ..Self::default()
        }
    }

    /// Poll the bus on the shared cadence and keep the repaint heartbeat alive.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            self.refresh();
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// Rebuild the roster + every contact's conversation from the latest-wins Bus
    /// mirrors. The worker republishes the FULL ring array on each change, so the
    /// newest message on a `state/chat/*` topic is the current state.
    fn refresh(&mut self) {
        // arch-11: open through the shared BusReader seam.
        let Some(persist) = BusReader::new(self.bus_root.clone()).open() else {
            return;
        };
        if let Some(roster) = latest_json::<Roster>(&persist, ROSTER_TOPIC) {
            self.roster = Some(roster);
        }
        if let Some(prefs) = latest_json::<NotifyPrefs>(&persist, NOTIFY_TOPIC) {
            self.notify = Some(prefs);
        }
        self.acked_alerts = latest_topic_suffixes(&persist, ALERT_ACK_PREFIX);
        self.snoozed_alerts = latest_topic_suffixes(&persist, ALERT_SNOOZE_PREFIX);
        let Some(roster) = &self.roster else {
            return;
        };
        let self_host = roster.self_host().to_string();
        // perf-5 — keep `convos` ACROSS refreshes and rebuild only the
        // conversations whose latest-wins ring blob actually moved this tick,
        // instead of re-decoding every contact's full history on every 2 s poll.
        // A conversation whose constituent topics' `latest_ulid`s all match the
        // cursors from its last build is reused in place (no JSON re-parse); any
        // change (incl. a retention trim, which republishes a new-ULID ring)
        // mismatches and rebuilds from the current blobs → behaviour-identical.
        let live_hosts: BTreeSet<String> = roster.contacts().map(|c| c.host.clone()).collect();
        for contact in roster.contacts() {
            let topics: Vec<String> = keys_for_contact(&self_host, &contact.host)
                .iter()
                .map(|key| conversation_topic(key))
                .collect();
            let cursors: Vec<Option<String>> = topics
                .iter()
                .map(|topic| persist.latest_ulid(topic).ok().flatten())
                .collect();
            if !conversation_is_current(
                self.convo_cursors.get(&contact.host),
                &cursors,
                self.convos.contains_key(&contact.host),
            ) {
                let conv = fold_rings(
                    contact.host.as_str(),
                    topics.iter().map(|topic| {
                        latest_json::<Vec<Message>>(&persist, topic).unwrap_or_default()
                    }),
                );
                self.convos.insert(contact.host.clone(), conv);
                self.convo_cursors.insert(contact.host.clone(), cursors);
            }
            // Watermark a first-seen contact at its current length so existing
            // backfill isn't flagged unread; keep an established watermark. (A
            // first-seen host always rebuilt above, so this length is the fresh
            // one, exactly as the old full-rebuild refresh set it.)
            let now_len = self.convos.get(&contact.host).map_or(0, Conversation::len);
            self.seen.entry(contact.host.clone()).or_insert(now_len);
        }
        // Drop conversations for contacts no longer on the roster — the old
        // full-rebuild dropped them implicitly by building a fresh map each tick.
        // `seen` is intentionally NOT pruned (a rejoining contact keeps its
        // established read watermark, matching the old `or_insert` behaviour).
        self.convos.retain(|host, _| live_hosts.contains(host));
        self.convo_cursors
            .retain(|host, _| live_hosts.contains(host));

        // Rooms (NOTIFY-CHAT-5): the registry mirror + each room's shared log —
        // the same per-conversation cursor reuse as the contact loop above.
        if let Some(rooms) = latest_json::<Vec<RoomDescriptor>>(&persist, ROOMS_TOPIC) {
            self.rooms = rooms;
        }
        let live_rooms: BTreeSet<String> = self.rooms.iter().map(|d| d.id.clone()).collect();
        for descriptor in &self.rooms {
            let topic = conversation_topic(&room_key(&descriptor.id));
            let cursors = vec![persist.latest_ulid(&topic).ok().flatten()];
            if !conversation_is_current(
                self.room_cursors.get(&descriptor.id),
                &cursors,
                self.room_convos.contains_key(&descriptor.id),
            ) {
                let conv = fold_rings(
                    descriptor.id.as_str(),
                    std::iter::once(
                        latest_json::<Vec<Message>>(&persist, &topic).unwrap_or_default(),
                    ),
                );
                self.room_convos.insert(descriptor.id.clone(), conv);
                self.room_cursors.insert(descriptor.id.clone(), cursors);
            }
            let now_len = self
                .room_convos
                .get(&descriptor.id)
                .map_or(0, Conversation::len);
            self.seen.entry(room_key(&descriptor.id)).or_insert(now_len);
        }
        self.room_convos.retain(|id, _| live_rooms.contains(id));
        self.room_cursors.retain(|id, _| live_rooms.contains(id));
    }

    /// Unread count for `host` — new messages since the read watermark, clamped so
    /// a ring eviction can't underflow.
    fn unread(&self, host: &str) -> usize {
        let now = self.convos.get(host).map_or(0, Conversation::len);
        let seen = self.seen.get(host).copied().unwrap_or(now);
        now.saturating_sub(seen)
    }

    /// Unread count for room `id` — same watermark logic, keyed by the `room:<id>`
    /// conversation key so it never collides with a contact hostname.
    fn room_unread(&self, id: &str) -> usize {
        let now = self.room_convos.get(id).map_or(0, Conversation::len);
        let seen = self.seen.get(&room_key(id)).copied().unwrap_or(now);
        now.saturating_sub(seen)
    }

    /// Alert messages folded across all host contacts, newest first. This is a
    /// view over live contact timelines, not another persisted feed.
    fn notification_items(&self) -> Vec<NotificationItem<'_>> {
        let mut items: Vec<NotificationItem<'_>> = self
            .convos
            .iter()
            .flat_map(|(host, conv)| {
                conv.messages()
                    .iter()
                    .filter(|m| matches!(m.kind, MessageKind::Alert { .. }))
                    .filter(|m| {
                        !self.acked_alerts.contains(m.id.as_str())
                            && !self.snoozed_alerts.contains(m.id.as_str())
                    })
                    .map(|msg| NotificationItem {
                        host: host.as_str(),
                        msg,
                    })
            })
            .collect();
        items.sort_by(|a, b| {
            b.msg
                .ts_unix_ms
                .cmp(&a.msg.ts_unix_ms)
                .then_with(|| b.host.cmp(a.host))
                .then_with(|| b.msg.id.as_str().cmp(a.msg.id.as_str()))
        });
        items
    }

    /// Test seam for shell-level integration fixtures: prove the aggregate
    /// Notifications lane has folded live contact alert messages without relying
    /// on first-seen unread watermarks.
    #[cfg(test)]
    pub(crate) fn notification_count_for_test(&self) -> usize {
        self.notification_items().len()
    }

    /// Test seam for shell-level integration fixtures that need the aggregate
    /// Notifications pane mounted in the rendered frame.
    #[cfg(test)]
    pub(crate) fn select_notifications_for_test(&mut self) {
        self.selected = Some(Selection::Notifications);
    }

    /// Session-only unread count for the aggregate Notifications lane.
    fn notifications_unread(&self) -> usize {
        let now = self.notification_items().len();
        let seen = self
            .seen
            .get(NOTIFICATIONS_SEEN_KEY)
            .copied()
            .unwrap_or(now);
        now.saturating_sub(seen)
    }

    /// The total unread across every contact **and** room — the count the shell's
    /// chrome unread indicator shows (NOTIFY-CHAT-6). Because Chat is the ONE
    /// notification interface, this is the whole-mesh unread tally (folded alerts +
    /// clipboard clips + human chat), summed over the same per-conversation
    /// watermarks the roster badges use, so the chrome badge can't diverge from the
    /// surface. Zero on a solo host with a quiet mesh (the honest empty state).
    pub(crate) fn total_unread(&self) -> usize {
        let contacts: usize = self.convos.keys().map(|h| self.unread(h)).sum();
        let rooms: usize = self.rooms.iter().map(|d| self.room_unread(&d.id)).sum();
        contacts + rooms
    }

    /// WIN7-4 — the host whose 1:1 contact timeline carries the most recent
    /// message, across every contact's already-loaded merged DM+alert ring
    /// (`self.convos` — the SAME live store [`Self::total_unread`] sums, just a
    /// different fold of the identical already-loaded state; no second read,
    /// no new Bus subscription, §7). Backs the Start Menu Chat tile's "recent
    /// sender" live fact (design lock #5's own example). `None` on a quiet
    /// mesh with no contact conversation yet (the honest empty state).
    pub(crate) fn most_recent_sender(&self) -> Option<&str> {
        self.convos
            .iter()
            .filter_map(|(host, conv)| conv.latest().map(|msg| (host.as_str(), msg.ts_unix_ms)))
            .max_by_key(|&(_, ts)| ts)
            .map(|(host, _)| host)
    }

    /// Render the ICQ surface: the roster rail on the left, the selected
    /// contact's conversation pane filling the rest.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        // MENU-2 — the shared top bar, titled **Contacts** (the operator's name
        // for the roster workspace), above the roster + pane. Its menus drive the
        // surface's own seams (§6): **Contacts** opens a 1:1 / marks read / mutes
        // (real roster ops only), **View** holds the feed filters, **Chat** sends
        // the draft + clears unread, **Presence** posts this seat's presence, and
        // **Help** carries the honest surface identity. Rendered before the
        // no-roster early-return so the bar is always present (its Presence menu
        // honestly omits itself until a roster lands, §7).
        if let Some(action) = menubar::show(self, ui) {
            menubar::apply(self, action);
        }
        ui.separator();

        let Some(roster) = self.roster.clone() else {
            self.waiting_pane(ui);
            return;
        };

        egui::SidePanel::left("chat-roster")
            .resizable(true)
            .default_width(Style::SP_XL * 7.0)
            .show_inside(ui, |ui| {
                self.roster_rail(ui, &roster);
            });

        match self.selected.clone() {
            Some(Selection::Notifications) => {
                let now = self.notification_items().len();
                self.seen.insert(NOTIFICATIONS_SEEN_KEY.to_string(), now);
                self.notifications_pane(ui);
            }
            Some(Selection::Contact(host)) if roster.get(&host).is_some() => {
                // Opening the pane marks it read (watermark → current length).
                let now = self.convos.get(&host).map_or(0, Conversation::len);
                self.seen.insert(host.clone(), now);
                self.conversation_pane(ui, &roster, &host);
            }
            Some(Selection::Room(id)) if self.room_descriptor(&id).is_some() => {
                let now = self.room_convos.get(&id).map_or(0, Conversation::len);
                self.seen.insert(room_key(&id), now);
                self.room_pane(ui, &roster, &id);
            }
            _ => self.home_pane(ui, &roster),
        }
    }

    /// The no-roster state is still honest about missing data, but it must read
    /// as a live surface waiting on its real worker mirror rather than a blank
    /// desktop placeholder.
    fn waiting_pane(&self, ui: &mut egui::Ui) {
        let (title, subtitle) = empty_copy(self.bus_root.is_some());
        ui.add_space(Style::SP_L);
        egui::Frame::group(ui.style())
            .shadow(card_shadow())
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(title)
                            .color(Style::TEXT)
                            .size(Style::HEADING)
                            .strong(),
                    );
                    ui.add_space(Style::SP_XS);
                    mde_egui::muted_note(ui, subtitle);
                    ui.add_space(Style::SP_M);
                    chat_metric_columns(
                        ui,
                        &[
                            ChatMetric {
                                label: "Bus",
                                value: if self.bus_root.is_some() {
                                    "visible".to_string()
                                } else {
                                    "missing".to_string()
                                },
                                detail: "local read path".to_string(),
                                tone: if self.bus_root.is_some() {
                                    Style::OK
                                } else {
                                    Style::SUPPORT_WARNING
                                },
                            },
                            ChatMetric {
                                label: "Roster",
                                value: "waiting".to_string(),
                                detail: ROSTER_TOPIC.to_string(),
                                tone: Style::TEXT_DIM,
                            },
                            ChatMetric {
                                label: "Alerts",
                                value: "waiting".to_string(),
                                detail: NOTIFY_TOPIC.to_string(),
                                tone: Style::TEXT_DIM,
                            },
                        ],
                    );
                });
            });
    }

    /// A real activity overview for the loaded-roster/no-selection state. This
    /// keeps a quiet Chat surface visibly alive without selecting a lane or
    /// clearing unread watermarks on the operator's behalf.
    fn home_pane(&mut self, ui: &mut egui::Ui, roster: &Roster) {
        let peer_count = roster
            .contacts()
            .filter(|contact| !roster.is_self(&contact.host))
            .count();
        let online_peers = roster
            .online()
            .into_iter()
            .filter(|contact| !roster.is_self(&contact.host))
            .count();
        let room_count = self.rooms.len();
        let alert_count = self.notification_items().len();
        let unread_count = self.home_unread_count();
        let latest = self.latest_activity_label();
        let mut open_notifications = false;

        ui.add_space(Style::SP_M);
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Chat activity")
                        .color(Style::TEXT)
                        .size(Style::HEADING)
                        .strong(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let summary = if unread_count > 0 {
                        format!("{unread_count} unread")
                    } else {
                        "caught up".to_string()
                    };
                    mde_egui::muted_note(ui, summary);
                });
            });
            mde_egui::muted_note(
                ui,
                "Contacts, rooms, and folded alerts are live from the local mesh read model.",
            );
            ui.add_space(Style::SP_M);
            chat_metric_columns(
                ui,
                &[
                    ChatMetric {
                        label: "Peers",
                        value: peer_count.to_string(),
                        detail: "enrolled contacts".to_string(),
                        tone: Style::TEXT,
                    },
                    ChatMetric {
                        label: "Online",
                        value: online_peers.to_string(),
                        detail: "available now".to_string(),
                        tone: if online_peers > 0 {
                            Style::OK
                        } else {
                            Style::TEXT_DIM
                        },
                    },
                    ChatMetric {
                        label: "Alerts",
                        value: alert_count.to_string(),
                        detail: "folded notifications".to_string(),
                        tone: if alert_count > 0 {
                            Style::SUPPORT_WARNING
                        } else {
                            Style::TEXT_DIM
                        },
                    },
                    ChatMetric {
                        label: "Rooms",
                        value: room_count.to_string(),
                        detail: latest.unwrap_or_else(|| "no timeline activity".to_string()),
                        tone: Style::ACCENT,
                    },
                ],
            );
            ui.add_space(Style::SP_M);

            {
                let items = self.notification_items();
                egui::Frame::group(ui.style())
                    .shadow(card_shadow())
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Latest notifications")
                                    .color(Style::TEXT)
                                    .size(Style::BODY)
                                    .strong(),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if !items.is_empty()
                                    && ui.button("Open notifications").clicked()
                                {
                                    open_notifications = true;
                                }
                            });
                        });
                        ui.add_space(Style::SP_XS);
                        if items.is_empty() {
                            mde_egui::muted_note(
                                ui,
                                "No folded alerts are active. Select a contact or room to inspect its timeline.",
                            );
                        } else {
                            for item in items.iter().take(3) {
                                notification_row(ui, *item, self.bus_root.as_deref());
                                ui.add_space(Style::SP_XS);
                            }
                        }
                    });
            }
        });

        if open_notifications {
            self.selected = Some(Selection::Notifications);
            self.draft.clear();
        }
    }

    fn latest_activity_label(&self) -> Option<String> {
        self.convos
            .values()
            .flat_map(|conv| conv.messages())
            .chain(self.room_convos.values().flat_map(|conv| conv.messages()))
            .map(|msg| msg.ts_unix_ms)
            .max()
            .map(|ts| {
                let clock = fmt_hh_mm(ts);
                if clock.is_empty() {
                    "latest activity recorded".to_string()
                } else {
                    format!("latest {clock}")
                }
            })
    }

    fn home_unread_count(&self) -> usize {
        // The aggregate Notifications lane is a view over contact timelines, so
        // use the larger watermark count rather than summing and double-counting
        // the same folded alert.
        self.total_unread().max(self.notifications_unread())
    }

    /// The registry descriptor for room `id`, if the worker has published it.
    fn room_descriptor(&self, id: &str) -> Option<&RoomDescriptor> {
        self.rooms.iter().find(|d| d.id == id)
    }

    /// The pinned self line (lock 17): presence dot + name, an ICQ **presence
    /// picker** to set your own presence (lock 5), and an inline **status editor**
    /// (lock 21). Both post real `action/chat/presence` actions the chat worker
    /// drains + republishes on the self roster entry — never local-only UI state.
    fn self_line(&mut self, ui: &mut egui::Ui, roster: &Roster) {
        let me = roster.self_contact();
        let current = PresenceChoice::from_presence(me.presence);
        ui.horizontal(|ui| {
            mde_egui::status_dot(ui, presence_color(me.presence));
            ui.label(
                RichText::new(me.display_name())
                    .color(Style::TEXT)
                    .size(Style::BODY)
                    .strong(),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let mut choice = current;
                egui::ComboBox::from_id_salt("chat-self-presence")
                    .selected_text(choice.label())
                    .show_ui(ui, |ui| {
                        for opt in PresenceChoice::ALL {
                            ui.selectable_value(&mut choice, opt, opt.label());
                        }
                    });
                if choice != current {
                    self.set_presence(choice);
                }
            });
        });
        // Status: a muted read-only line + an ✎ edit affordance, or the inline
        // editor when open. Committing posts the status (empty ⇒ clear) via the
        // same presence action the worker republishes on the self roster entry.
        if self.editing_status {
            ui.horizontal(|ui| {
                let field = egui::TextEdit::singleline(&mut self.status_draft)
                    .desired_width(f32::INFINITY)
                    .hint_text(CHAT_HINT_SET_STATUS);
                let resp = ui.add(field);
                let commit = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("Save").clicked() || commit {
                    self.set_status(Some(self.status_draft.trim()));
                    self.editing_status = false;
                }
                if ui.button("Cancel").clicked() {
                    self.editing_status = false;
                }
            });
        } else {
            ui.horizontal(|ui| {
                match &me.status_message {
                    Some(status) => {
                        mde_egui::muted_note(ui, status);
                    }
                    None => {
                        mde_egui::muted_note(ui, "No status set");
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .small_button("\u{270E}")
                        .on_hover_text("Edit your status")
                        .clicked()
                    {
                        self.status_draft = me.status_message.clone().unwrap_or_default();
                        self.editing_status = true;
                    }
                });
            });
        }
        ui.separator();
    }

    /// The roster rail — the ICQ Online / Offline groups (lock 4).
    fn roster_rail(&mut self, ui: &mut egui::Ui, roster: &Roster) {
        // Self line, pinned at the top with its own presence (lock 17).
        self.self_line(ui, roster);
        self.notifications_row(ui);

        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // View → Unread Only (MENU-2) prunes the groups to the
                // conversations that need attention (self + the open one stay).
                let online: Vec<&Contact> = roster
                    .online()
                    .into_iter()
                    .filter(|c| self.roster_shows(roster, c))
                    .collect();
                let offline: Vec<&Contact> = roster
                    .offline()
                    .into_iter()
                    .filter(|c| self.roster_shows(roster, c))
                    .collect();
                self.roster_group(ui, roster, "Online", &online);
                ui.add_space(Style::SP_S);
                self.roster_group(ui, roster, "Offline", &offline);
                ui.add_space(Style::SP_S);
                self.rooms_group(ui);
            });
    }

    /// Aggregate Notifications lane row pinned with the roster controls. It is a
    /// session-only alert view; per-host timelines remain the durable source.
    fn notifications_row(&mut self, ui: &mut egui::Ui) {
        let unread = self.notifications_unread();
        let selected = self.selected == Some(Selection::Notifications);
        let label = if unread > 0 {
            RichText::new("Notifications")
                .color(Style::TEXT)
                .size(Style::BODY)
                .strong()
        } else {
            RichText::new("Notifications")
                .color(Style::TEXT_DIM)
                .size(Style::BODY)
        };
        let clicked = ui
            .horizontal(|ui| {
                ui.label(
                    RichText::new("!")
                        .color(Style::ACCENT)
                        .size(Style::BODY)
                        .strong(),
                );
                let clicked = ui.selectable_label(selected, label).clicked();
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if unread > 0 {
                        ui.label(
                            RichText::new(unread.to_string())
                                .color(Style::ACCENT)
                                .size(Style::SMALL)
                                .strong(),
                        );
                    }
                    mde_egui::muted_note(ui, "alerts");
                });
                clicked
            })
            .inner;
        if clicked {
            self.selected = Some(Selection::Notifications);
            self.draft.clear();
        }
        ui.separator();
    }

    /// The Rooms group under the contact roster (NOTIFY-CHAT-5): the auto system
    /// rooms (All Fleet + per-severity alert bands) and any ad-hoc rooms, each a
    /// selectable row whose shared log opens in the pane. Rendered even when empty
    /// only if the worker has published a registry — a solo host with no room
    /// mirror shows nothing (no fabricated rooms, §7).
    fn rooms_group(&mut self, ui: &mut egui::Ui) {
        if self.rooms.is_empty() {
            return;
        }
        // System rooms first (the always-present fleet bands), then ad-hoc —
        // pruned to the unread ones under View → Unread Only (MENU-2).
        let ids: Vec<(String, String, RoomKind)> = self
            .rooms
            .iter()
            .filter(|d| self.room_shows(&d.id))
            .map(|d| (d.id.clone(), d.name.clone(), d.kind))
            .collect();
        ui.label(
            RichText::new(format!("ROOMS ({})", ids.len()))
                .color(Style::TEXT_DIM)
                .size(Style::SMALL)
                .strong(),
        );
        ui.add_space(Style::SP_XS);
        for (id, name, kind) in ids {
            self.room_row(ui, &id, &name, kind);
        }
        // Create an ad-hoc room (open-join, lock 25): a name field + a Create button
        // that fires the worker's `action/chat/room` create op. The worker seeds the
        // id from the name and replicates the signed descriptor.
        ui.add_space(Style::SP_XS);
        let mut create = false;
        ui.horizontal(|ui| {
            let field = egui::TextEdit::singleline(&mut self.new_room)
                .desired_width(f32::INFINITY)
                .hint_text(CHAT_HINT_NEW_ROOM);
            let resp = ui.add(field);
            create = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let ready = !self.new_room.trim().is_empty();
            if ui.add_enabled(ready, egui::Button::new("Create")).clicked() {
                create = true;
            }
        });
        let name = self.new_room.trim().to_string();
        if create && !name.is_empty() {
            self.create_room(&name);
            self.new_room.clear();
        }
    }

    /// One room row: a room glyph, the name (bold when unread), a kind tag
    /// (system / ad-hoc), and an unread count badge.
    fn room_row(&mut self, ui: &mut egui::Ui, id: &str, name: &str, kind: RoomKind) {
        let unread = self.room_unread(id);
        let selected = self.selected == Some(Selection::Room(id.to_string()));
        let label = RichText::new(name).size(Style::BODY);
        let label = if unread > 0 {
            label.color(Style::TEXT).strong()
        } else {
            label.color(Style::TEXT_DIM)
        };
        let tag = match kind {
            RoomKind::System => "system",
            RoomKind::AdHoc => "room",
        };
        let clicked = ui
            .horizontal(|ui| {
                ui.label(
                    RichText::new("\u{0023}")
                        .color(Style::ACCENT)
                        .size(Style::BODY),
                ); // #
                let clicked = ui.selectable_label(selected, label).clicked();
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if unread > 0 {
                        ui.label(
                            RichText::new(unread.to_string())
                                .color(Style::ACCENT)
                                .size(Style::SMALL)
                                .strong(),
                        );
                    }
                    mde_egui::muted_note(ui, tag);
                });
                clicked
            })
            .inner;
        if clicked {
            self.selected = Some(Selection::Room(id.to_string()));
            self.draft.clear();
        }
        ui.add_space(Style::SP_XS);
    }

    /// One ICQ group header + its contact rows.
    fn roster_group(
        &mut self,
        ui: &mut egui::Ui,
        roster: &Roster,
        title: &str,
        group: &[&Contact],
    ) {
        ui.label(
            RichText::new(format!("{} ({})", title.to_uppercase(), group.len()))
                .color(Style::TEXT_DIM)
                .size(Style::SMALL)
                .strong(),
        );
        ui.add_space(Style::SP_XS);
        for &contact in group {
            if roster.is_self(&contact.host) {
                continue; // self is pinned above, not in a group
            }
            self.contact_row(ui, contact);
        }
    }

    /// One roster contact row: presence dot, name (bold when unread), role tag,
    /// status message, and an unread count badge.
    fn contact_row(&mut self, ui: &mut egui::Ui, contact: &Contact) {
        let unread = self.unread(&contact.host);
        let selected = self.selected == Some(Selection::Contact(contact.host.clone()));
        let name = RichText::new(contact.display_name()).size(Style::BODY);
        let name = if unread > 0 {
            name.color(Style::TEXT).strong()
        } else {
            name.color(Style::TEXT_DIM)
        };

        let resp = ui.horizontal(|ui| {
            mde_egui::status_dot(ui, presence_color(contact.presence));
            let clicked = ui
                .selectable_label(selected, name)
                .on_hover_text(contact.presence.label())
                .clicked();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if unread > 0 {
                    ui.label(
                        RichText::new(unread.to_string())
                            .color(Style::ACCENT)
                            .size(Style::SMALL)
                            .strong(),
                    );
                }
                mde_egui::muted_note(ui, contact.role.tag());
            });
            clicked
        });
        if resp.inner {
            self.selected = Some(Selection::Contact(contact.host.clone()));
            self.draft.clear();
        }
        if let Some(status) = &contact.status_message {
            ui.horizontal(|ui| {
                ui.add_space(Style::SP_M);
                mde_egui::muted_note(ui, status);
            });
        }
        ui.add_space(Style::SP_XS);
    }

    /// The conversation pane for the open contact: header, the ring timeline, and
    /// a composer (peers only — the self-contact's alert timeline is read-only).
    fn conversation_pane(&mut self, ui: &mut egui::Ui, roster: &Roster, host: &str) {
        let is_self = roster.is_self(host);
        let bus_root = self.bus_root.clone();
        let muted = self.is_contact_muted(host);
        // Header.
        if let Some(contact) = roster.get(host) {
            ui.horizontal(|ui| {
                mde_egui::status_dot(ui, presence_color(contact.presence));
                ui.label(
                    RichText::new(contact.display_name())
                        .color(Style::TEXT)
                        .size(Style::HEADING),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    mde_egui::muted_note(ui, contact.presence.label());
                });
            });
            if contact.host.as_str() != contact.display_name() {
                mde_egui::field(ui, "host", &contact.host, Style::TEXT_DIM);
            }
            if let Some(status) = &contact.status_message {
                mde_egui::muted_note(ui, status);
            }
            // Per-contact actions (lock 15) — chat is the launch point for voice +
            // remote desktop. Only for a peer: you don't Call / Remote-Control your
            // own node (the self-contact is alerts/clips only, lock 17).
            if !is_self {
                contact_actions(ui, bus_root.as_deref(), host, muted, &mut self.last_error);
            }
        }
        ui.separator();

        // Composer pinned to the bottom; the timeline fills the rest above it.
        if !is_self {
            egui::TopBottomPanel::bottom("chat-composer")
                .resizable(false)
                .show_inside(ui, |ui| {
                    self.composer(ui, host, roster.get(host));
                });
        }

        let self_host = roster.self_host();
        let recipient = roster.get(host);
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| match self.convos.get(host) {
                Some(conv) if !conv.is_empty() => {
                    // View → feed filters (MENU-2): prune by kind before render.
                    let shown: Vec<&Message> = conv
                        .messages()
                        .iter()
                        .filter(|m| self.feed_shows(&m.kind))
                        .collect();
                    if shown.is_empty() {
                        crate::session::empty_state(
                            ui,
                            "All messages filtered",
                            "Everything in this timeline is hidden by the View feed filters — \
                             re-enable Alerts / Clipboard Clips / Messages to see it.",
                        );
                    } else {
                        render_timeline(ui, &shown, self_host, recipient, bus_root.as_deref());
                    }
                }
                _ => {
                    let subtitle = if is_self {
                        "This node's local alerts and clipboard copies land here."
                    } else {
                        "No messages yet — say hello, or wait for this host's alerts to arrive."
                    };
                    crate::session::empty_state(ui, "No messages", subtitle);
                }
            });
    }

    /// The aggregate Notifications pane: folded alert messages only, newest first,
    /// with inline controls for DND, minimum severity, and per-source mute.
    fn notifications_pane(&mut self, ui: &mut egui::Ui) {
        let bus_root = self.bus_root.clone();
        let items = self.notification_items();
        let unread = self.notifications_unread();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Notifications")
                    .color(Style::TEXT)
                    .size(Style::HEADING),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let count = if unread > 0 {
                    format!("{unread} unread · {} alerts", items.len())
                } else {
                    format!("{} alerts", items.len())
                };
                mde_egui::muted_note(ui, count);
            });
        });
        mde_egui::muted_note(
            ui,
            "Session lane over host alert messages; open a contact for its full timeline.",
        );
        self.notification_controls(ui, &items);
        ui.separator();

        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if items.is_empty() {
                    crate::session::empty_state(
                        ui,
                        "No notifications",
                        "Folded system alerts from mesh hosts appear here as messages from their contacts.",
                    );
                    return;
                }
                for item in items {
                    notification_row(ui, item, bus_root.as_deref());
                    ui.add_space(Style::SP_XS);
                }
            });
    }

    /// Inline notification controls (NOTIF-10). These controls silence surfaces;
    /// folded alerts still fill the feed and contact timelines.
    fn notification_controls(&self, ui: &mut egui::Ui, items: &[NotificationItem<'_>]) {
        ui.add_space(Style::SP_XS);
        ui.horizontal_wrapped(|ui| {
            let mut dnd = self.dnd_active();
            let dnd_resp = ui
                .checkbox(&mut dnd, "Do Not Disturb")
                .on_hover_text("Mute notification surfaces; the feed and Alerts pip still fill");
            let _stable = ui.interact(
                dnd_resp.rect,
                notification_dnd_toggle_id(),
                egui::Sense::hover(),
            );
            if dnd_resp.changed() {
                self.set_dnd(dnd);
            }

            let current_threshold = self.notify_threshold();
            let mut threshold = current_threshold;
            egui::ComboBox::from_id_salt("chat-notify-threshold")
                .selected_text(format!("Min {}", threshold.tag()))
                .show_ui(ui, |ui| {
                    for sev in [Severity::Info, Severity::Warning, Severity::Critical] {
                        ui.selectable_value(&mut threshold, sev, sev.tag());
                    }
                });
            if threshold != current_threshold {
                self.set_notify_threshold(threshold);
            }
        });

        let sources = notification_sources(items);
        if !sources.is_empty() {
            ui.horizontal_wrapped(|ui| {
                mde_egui::muted_note(ui, "sources");
                for source in sources {
                    let mut muted = self.is_source_muted(&source);
                    if ui
                        .checkbox(&mut muted, source.as_str())
                        .on_hover_text(format!("Mute {source} notification surfaces"))
                        .changed()
                    {
                        // Best-effort: this &self control holds a borrow of `items`,
                        // so it can't reach `last_error`; the toggle is retriable and
                        // the worker republishes the true policy (the per-contact and
                        // per-room mute buttons DO surface via `mute_button`).
                        let _ = publish_mute(self.bus_root.as_deref(), "source", &source, muted);
                    }
                }
            });
        }
        ui.add_space(Style::SP_XS);
    }

    /// The message composer — a text field + Send that writes `action/chat/send`.
    fn composer(&mut self, ui: &mut egui::Ui, host: &str, recipient: Option<&Contact>) {
        ui.add_space(Style::SP_XS);
        let mut send = false;
        ui.horizontal(|ui| {
            let field = egui::TextEdit::singleline(&mut self.draft)
                .desired_width(f32::INFINITY)
                .hint_text(CHAT_HINT_MESSAGE);
            let resp = ui.add(field);
            send = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("Send").clicked() {
                send = true;
            }
        });
        // The recipient's presence previews how this message will deliver.
        if let Some(c) = recipient {
            let delivery = Delivery::for_recipient(Some(c));
            mde_egui::muted_note(ui, delivery_preview_text(delivery, c.presence.label()));
        }

        // Send-To affordance (lock 15, file kind): a drag-dropped or typed path is
        // handed to `mde-files` over the mesh. A DRM seat has no native file dialog,
        // so drag-drop + a path field is the honest attach path.
        if let Some(dropped) = ui
            .ctx()
            .input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone()))
        {
            self.attach_path = dropped.to_string_lossy().into_owned();
        }
        let mut send_file = false;
        ui.horizontal(|ui| {
            mde_egui::muted_note(ui, CHAT_ATTACH_LABEL);
            let field = egui::TextEdit::singleline(&mut self.attach_path)
                .desired_width(f32::INFINITY)
                .hint_text(CHAT_HINT_ATTACH_FILE);
            ui.add(field);
            let ready = !self.attach_path.trim().is_empty();
            if ui
                .add_enabled(ready, egui::Button::new("Send file"))
                .clicked()
            {
                send_file = true;
            }
        });
        ui.add_space(Style::SP_XS);

        let text = self.draft.trim().to_string();
        if send && !text.is_empty() {
            // shell-ux-11: clear the draft ONLY on a successful publish; a Bus
            // failure KEEPS the message + surfaces why, never silently destroys it.
            match self.send(host, &text) {
                Ok(()) => {
                    self.draft.clear();
                    self.last_error = None;
                }
                Err(e) => self.last_error = Some(e),
            }
        }
        let path = self.attach_path.trim().to_string();
        if send_file && !path.is_empty() {
            match self.send_file(host, Path::new(&path)) {
                Ok(()) => {
                    self.attach_path.clear();
                    self.last_error = None;
                }
                Err(e) => self.last_error = Some(e),
            }
        }
        // The inline DANGER note (mirrors the datacenter `last_error` idiom): a
        // failed send stays visible right under the composer with the draft intact.
        if let Some(err) = self.last_error.as_deref() {
            ui.add_space(Style::SP_XS);
            ui.colored_label(Style::DANGER, err);
        }
    }

    /// Publish `action/chat/send` `{scope:"peer", to, text}` to the local Bus —
    /// the worker signs, persists, and relays it. Returns the publish result so the
    /// composer keeps the draft + surfaces the error on failure (shell-ux-11).
    fn send(&self, to: &str, text: &str) -> Result<(), String> {
        let body = serde_json::json!({ "scope": "peer", "to": to, "text": text }).to_string();
        publish(self.bus_root.as_deref(), ACTION_CHAT_SEND, &body)
    }

    /// Offer `path` to the `to` contact (lock 15, file kind): fire the real
    /// `mde-files` Send-To so the bytes copy into the peer's replicated inbox
    /// (reachable now), AND post the offer into the conversation as a chat message
    /// carrying an inline `file` descriptor. The worker relaying the descriptor into
    /// a rich [`MessageKind::File`] card is the integration seam — until then the
    /// offer still shows as its human-readable `text`, never faked.
    fn send_file(&self, to: &str, path: &Path) -> Result<(), String> {
        let name = path.file_name().map_or_else(
            || path.to_string_lossy().into_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        let size_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        // 1) The real transfer — the exact `bus_backend::send_to` wire (§6 boundary).
        let send_to = serde_json::json!({
            "sources": [path.to_string_lossy()],
            "selector": format!("peer:{to}"),
            "mode": "copy",
            "conflict": "rename",
        })
        .to_string();
        publish(self.bus_root.as_deref(), ACTION_FILE_SEND_TO, &send_to)?;
        // 2) The conversation offer — human text now, an upgradeable `file` field for
        //    the worker's File-card fold.
        let offer = serde_json::json!({
            "scope": "peer",
            "to": to,
            "text": format!("{CHAT_SENT_FILE_PREFIX} {name} ({size_bytes} bytes)"),
            "file": { "name": name, "size_bytes": size_bytes },
        })
        .to_string();
        // The offer publish carries the user's intent; surface its failure so the
        // attach path is kept for a retry (shell-ux-11).
        publish(self.bus_root.as_deref(), ACTION_CHAT_SEND, &offer)
    }

    /// The conversation pane for the open room (NOTIFY-CHAT-5): header (name, kind,
    /// member count), a Join / Dissolve action bar, the shared room log timeline,
    /// and a composer that sends `{scope:"room"}`.
    fn room_pane(&mut self, ui: &mut egui::Ui, roster: &Roster, id: &str) {
        let bus_root = self.bus_root.clone();
        let room_muted = self.is_room_muted(id);
        let self_host = roster.self_host().to_string();
        let Some((name, kind, members, is_member, can_dissolve)) =
            self.room_descriptor(id).map(|d| {
                let is_member = d.members.iter().any(|m| m == &self_host);
                let can_dissolve = d.kind == RoomKind::AdHoc && d.creator == self_host;
                (
                    d.name.clone(),
                    d.kind,
                    d.members.len(),
                    is_member,
                    can_dissolve,
                )
            })
        else {
            return;
        };

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("\u{0023}")
                    .color(Style::ACCENT)
                    .size(Style::HEADING),
            );
            ui.label(RichText::new(&name).color(Style::TEXT).size(Style::HEADING));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let tag = match kind {
                    RoomKind::System => "system room",
                    RoomKind::AdHoc => "room",
                };
                mde_egui::muted_note(ui, format!("{tag} · {members} members"));
            });
        });
        mde_egui::field(ui, "id", id, Style::TEXT_DIM);

        // Lifecycle actions: self-join an open room, or dissolve one you created.
        ui.horizontal(|ui| {
            if is_member {
                mde_egui::muted_note(ui, "joined");
            } else if ui
                .button("Join")
                .on_hover_text("Self-join this open room")
                .clicked()
            {
                self.room_action("join", id, None);
            }
            if can_dissolve
                && ui
                    .button("Dissolve")
                    .on_hover_text("Dissolve this room you created")
                    .clicked()
            {
                self.room_action("dissolve", id, None);
                self.selected = None;
            }
            mute_button(
                ui,
                bus_root.as_deref(),
                "room",
                id,
                room_muted,
                &mut self.last_error,
            );
        });
        ui.separator();

        // Composer pinned to the bottom; the shared log fills the rest above it.
        egui::TopBottomPanel::bottom("chat-room-composer")
            .resizable(false)
            .show_inside(ui, |ui| {
                self.room_composer(ui, id, members);
            });

        ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| match self.room_convos.get(id) {
                Some(conv) if !conv.is_empty() => {
                    // View → feed filters (MENU-2) prune the shared log too. A
                    // room has no single recipient presence — pass None so my
                    // outgoing line reads a neutral "Sent" (the honest room state;
                    // per-member delivery is the worker's fan-out, lock 22).
                    let shown: Vec<&Message> = conv
                        .messages()
                        .iter()
                        .filter(|m| self.feed_shows(&m.kind))
                        .collect();
                    if shown.is_empty() {
                        crate::session::empty_state(
                            ui,
                            "All messages filtered",
                            "Everything in this room's log is hidden by the View feed filters — \
                             re-enable Alerts / Clipboard Clips / Messages to see it.",
                        );
                    } else {
                        render_timeline(ui, &shown, &self_host, None, bus_root.as_deref());
                    }
                }
                _ => {
                    crate::session::empty_state(
                        ui,
                        "No messages",
                        "This room's shared log is empty — say hello, or wait for a fleet alert to \
                         land here.",
                    );
                }
            });
    }

    /// The room composer — a text field + Send that writes `action/chat/send`
    /// `{scope:"room"}`. A room fans out to each online member, so the honest hint
    /// is the member count, not a single-recipient delivery checkmark (lock 22).
    fn room_composer(&mut self, ui: &mut egui::Ui, id: &str, members: usize) {
        ui.add_space(Style::SP_XS);
        let mut send = false;
        ui.horizontal(|ui| {
            let field = egui::TextEdit::singleline(&mut self.draft)
                .desired_width(f32::INFINITY)
                .hint_text(CHAT_HINT_ROOM_MESSAGE);
            let resp = ui.add(field);
            send = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("Send").clicked() {
                send = true;
            }
        });
        mde_egui::muted_note(ui, room_member_note(members));
        ui.add_space(Style::SP_XS);
        let text = self.draft.trim().to_string();
        if send && !text.is_empty() {
            // shell-ux-11: clear only on a successful publish; keep + surface on fail.
            match self.send_room(id, &text) {
                Ok(()) => {
                    self.draft.clear();
                    self.last_error = None;
                }
                Err(e) => self.last_error = Some(e),
            }
        }
        if let Some(err) = self.last_error.as_deref() {
            ui.add_space(Style::SP_XS);
            ui.colored_label(Style::DANGER, err);
        }
    }

    /// Publish `action/chat/send` `{scope:"room", to:<id>, text}` — the worker signs
    /// it, appends to the room's shared Syncthing log, and fans it out to each online
    /// member (best-effort; a missing Bus is a silent no-op).
    fn send_room(&self, id: &str, text: &str) -> Result<(), String> {
        let body = serde_json::json!({ "scope": "room", "to": id, "text": text }).to_string();
        publish(self.bus_root.as_deref(), ACTION_CHAT_SEND, &body)
    }

    /// Create an ad-hoc room named `name`: derive a stable id from the name and fire
    /// the worker's `action/chat/room` create op (the worker owns + joins it).
    fn create_room(&self, name: &str) {
        let id = room_id_from_name(name);
        if id.is_empty() {
            return;
        }
        let body = serde_json::json!({ "op": "create", "id": id, "name": name }).to_string();
        // Best-effort: the create op is retriable and the worker owns room state.
        let _ = publish(self.bus_root.as_deref(), ACTION_CHAT_ROOM, &body);
    }

    /// Fire an `action/chat/room` lifecycle op (`join` / `dissolve`) for room `id`.
    fn room_action(&self, op: &str, id: &str, name: Option<&str>) {
        let mut obj = serde_json::json!({ "op": op, "id": id });
        if let Some(n) = name {
            obj["name"] = serde_json::Value::String(n.to_string());
        }
        // Best-effort: a retriable lifecycle op the worker republishes.
        let _ = publish(self.bus_root.as_deref(), ACTION_CHAT_ROOM, &obj.to_string());
    }

    /// Post `action/chat/presence` to set this seat's presence (Available ⇒ clear
    /// to auto). The worker updates its self-presence, gossips it, and republishes
    /// the self roster entry the roster rail then reads back.
    fn set_presence(&self, choice: PresenceChoice) {
        let body = serde_json::json!({ "presence": choice.wire() }).to_string();
        // Best-effort: presence is a retriable toggle the worker republishes.
        let _ = publish(self.bus_root.as_deref(), ACTION_CHAT_PRESENCE, &body);
    }

    /// Post `action/chat/presence` to set this seat's free-text status (empty ⇒
    /// clear at the worker). Carried on the same action the worker republishes.
    fn set_status(&self, status: Option<&str>) {
        let body = serde_json::json!({ "status": status }).to_string();
        // Best-effort: a retriable status the worker republishes.
        let _ = publish(self.bus_root.as_deref(), ACTION_CHAT_PRESENCE, &body);
    }

    /// Whether the View feed filters admit a message of `kind` (MENU-2). The
    /// three bands mirror the notification model: folded **alerts**, clipboard
    /// **clips**, and everything human-authored (**messages** — text, file
    /// offers, call/remote records). A pure view choice — never a data mutation.
    const fn feed_shows(&self, kind: &MessageKind) -> bool {
        match kind {
            MessageKind::Alert { .. } => self.show_alerts,
            MessageKind::Clipboard { .. } => self.show_clips,
            _ => self.show_messages,
        }
    }

    /// Whether the roster rail lists `contact` under View → Unread Only: self
    /// stays pinned, the open conversation stays visible (opening a pane
    /// watermarks it read, so it would otherwise vanish mid-read), everything
    /// else needs unread.
    fn roster_shows(&self, roster: &Roster, contact: &Contact) -> bool {
        if !self.unread_only || roster.is_self(&contact.host) {
            return true;
        }
        if self.selected == Some(Selection::Contact(contact.host.clone())) {
            return true;
        }
        self.unread(&contact.host) > 0
    }

    /// Whether the roster rail lists room `id` under View → Unread Only (same
    /// rule as [`Self::roster_shows`]: the open room stays visible).
    fn room_shows(&self, id: &str) -> bool {
        if !self.unread_only || self.selected == Some(Selection::Room(id.to_string())) {
            return true;
        }
        self.room_unread(id) > 0
    }

    /// Whether contact `host` is muted per the worker's published policy (a
    /// missing mirror — a fresh solo host — reads as not-muted).
    fn is_contact_muted(&self, host: &str) -> bool {
        self.notify
            .as_ref()
            .is_some_and(|n| n.is_contact_muted(host))
    }

    /// Whether room `id` is muted per the worker's published policy.
    fn is_room_muted(&self, id: &str) -> bool {
        self.notify.as_ref().is_some_and(|n| n.is_room_muted(id))
    }

    /// Whether folded-alert source `source` is muted per the worker's published
    /// policy.
    fn is_source_muted(&self, source: &str) -> bool {
        self.notify
            .as_ref()
            .is_some_and(|n| n.is_source_muted(source))
    }

    /// The current alert severity threshold from the worker mirror, or the
    /// model's default before the first mirror arrives.
    fn notify_threshold(&self) -> Severity {
        self.notify
            .as_ref()
            .map_or_else(|| NotifyPrefs::new().threshold(), NotifyPrefs::threshold)
    }

    /// Post `action/chat/notify-prefs` to update the persisted alert threshold.
    fn set_notify_threshold(&self, threshold: Severity) {
        let body = serde_json::json!({ "threshold": threshold.tag() }).to_string();
        // Best-effort: a retriable pref the worker republishes.
        let _ = publish(self.bus_root.as_deref(), ACTION_CHAT_NOTIFY_PREFS, &body);
    }

    /// Read the fleet-wide DND toggle. Missing Bus state honestly reads as off.
    fn dnd_active(&self) -> bool {
        self.bus_root
            .as_deref()
            .is_some_and(|root| mde_bus::dnd::load_default(root).active)
    }

    /// Write the fleet-wide DND toggle without dropping active topic snoozes.
    fn set_dnd(&self, active: bool) {
        let Some(root) = self.bus_root.as_deref() else {
            return;
        };
        let existing = mde_bus::dnd::load_default(root);
        let state = mde_bus::dnd::DndState {
            active,
            since_unix_ms: now_unix_ms(),
            set_by_peer: local_hostname(),
            snoozes: existing.snoozes,
        };
        let _ = mde_bus::dnd::save_default(root, &state);
    }
}

/// MENU-2 (Contacts) — the shared top bar over the ICQ roster workspace.
///
/// Titled **Contacts** (the operator's name for the surface — the roster IS the
/// workspace). Every item drives a seam the surface already owns (§6, one
/// publish/toggle path):
///
/// * **Contacts** — the real roster ops only: *Open 1:1* (one entry per peer
///   contact, the pane-open seam), *Mark Read* (one entry per unread
///   conversation — the same watermark the pane writes on open), and
///   *Mute Contact / Room* (`action/chat/mute`, the row toggle's publish).
/// * **View** — the feed filters (Alerts / Clipboard Clips / Messages) pruning
///   the open timeline by kind, and *Unread Only* pruning the roster rail.
/// * **Chat** — *Send Message* (the composer's `action/chat/send` on the open
///   conversation), *Clear Unread* (watermark everything read), *Close
///   Conversation* (deselect).
/// * **Presence** — this seat's `action/chat/presence` (the self-line picker's
///   seam) + *Edit Status…* (the ✎ editor). Omitted until a roster exists.
/// * **Help** — the honest surface identity + the delivery-semantics truth
///   (captions, never a dead activatable entry — §7/§8).
///
/// A context-gated item (Mute / Send / Close with nothing applicable) renders
/// **disabled**; an absent seam is omitted or captioned — never a dead entry
/// (§7). The status cluster shows this seat's live presence, the contacts
/// online, and the whole-mesh unread tally (MENU-2 chips).
mod menubar {
    use super::{ChatState, Contact, Conversation, PresenceChoice, Selection};
    use mde_egui::egui::Ui;
    use mde_egui::menubar::{Entry, Item, Menu, MenuBar, MenuBarModel};
    use mde_egui::{ChipTone, StatusChip, Style};

    /// A filled status dot — the same glyph the roster rows use.
    const DOT: &str = "\u{25CF}";

    /// One menu action — each routes to a real Chat/roster seam in [`apply`].
    /// Carries owned targets (a host / room id), so `Clone`, not `Copy`.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) enum MenuAction {
        /// Open a peer contact's 1:1 conversation pane (the roster row's seam).
        OpenContact(String),
        /// Watermark one conversation read (the same seam opening its pane uses).
        MarkRead(Selection),
        /// Toggle the selected conversation's mute policy (`action/chat/mute`).
        MuteConversation,
        /// Deselect the open conversation (local nav).
        CloseConversation,
        /// Toggle the Alerts feed filter (View).
        ToggleAlerts,
        /// Toggle the Clipboard-Clips feed filter (View).
        ToggleClips,
        /// Toggle the Messages feed filter (View).
        ToggleMessages,
        /// Toggle the Unread-Only roster filter (View).
        ToggleUnreadOnly,
        /// Send the composer draft to the open conversation (`action/chat/send`).
        SendDraft,
        /// Watermark every contact + room read (the whole-mesh clear).
        ClearUnread,
        /// Set this seat's presence (`action/chat/presence`).
        SetPresence(PresenceChoice),
        /// Open the inline self-status editor (the self-line ✎ seam).
        EditStatus,
    }

    /// Render the CONTACTS bar and return the action picked this frame, if any.
    pub(super) fn show(state: &ChatState, ui: &mut Ui) -> Option<MenuAction> {
        let menus = build_menus(state);
        let status = build_status(state);
        let model = MenuBarModel {
            // The dock groups this surface under **Comms** (cyan), so the title
            // wears that categorical accent (lock 2). "Contacts" is the operator
            // retitle (MENU-2) — the roster is the workspace.
            title: "Contacts",
            accent: Style::ACCENT_COMMS,
            menus: &menus,
            status: &status,
        };
        MenuBar::show(ui, &model)
    }

    /// This seat's current presence choice, or `None` before a roster lands.
    fn current_presence(state: &ChatState) -> Option<PresenceChoice> {
        state
            .roster
            .as_ref()
            .map(|r| PresenceChoice::from_presence(r.self_contact().presence))
    }

    /// The selected conversation's mute state, or `None` when nothing is open.
    fn selected_muted(state: &ChatState) -> Option<bool> {
        match state.selected.as_ref()? {
            Selection::Notifications => None,
            Selection::Contact(host) => Some(state.is_contact_muted(host)),
            Selection::Room(id) => Some(state.is_room_muted(id)),
        }
    }

    /// Whether Chat → Send Message can fire: a non-empty draft addressed at an
    /// open peer conversation or room. The self-contact's alert timeline has no
    /// composer (lock 17), so it never sends.
    fn can_send(state: &ChatState) -> bool {
        if state.draft.trim().is_empty() {
            return false;
        }
        match (&state.selected, state.roster.as_ref()) {
            (Some(Selection::Contact(host)), Some(roster)) => !roster.is_self(host),
            (Some(Selection::Room(id)), _) => state.room_descriptor(id).is_some(),
            _ => false,
        }
    }

    /// Build the bar from live state: Contacts / View / Chat always present
    /// (items context-gated, §7), Presence only once a roster exists, Help last.
    fn build_menus(state: &ChatState) -> Vec<Menu<MenuAction>> {
        let mut menus = vec![
            build_contacts_menu(state),
            build_view_menu(state),
            build_chat_menu(state),
        ];
        if let Some(current) = current_presence(state) {
            menus.push(build_presence_menu(state, current));
        }
        menus.push(build_help_menu());
        menus
    }

    /// The **Contacts** menu — real roster ops only (MENU-2): Open 1:1 /
    /// Mark Read submenus over the live roster, and the selected conversation's
    /// mute toggle.
    fn build_contacts_menu(state: &ChatState) -> Menu<MenuAction> {
        let muted = selected_muted(state);
        let mute_label = match state.selected {
            Some(Selection::Room(_)) => "Mute Room",
            _ => "Mute Contact",
        };
        Menu::new(
            "Contacts",
            vec![
                Entry::Submenu {
                    label: "Open 1:1".to_owned(),
                    mnemonic: None,
                    entries: open_entries(state),
                },
                Entry::Submenu {
                    label: "Mark Read".to_owned(),
                    mnemonic: None,
                    entries: mark_read_entries(state),
                },
                Entry::Separator,
                Entry::Item(
                    Item::new(MenuAction::MuteConversation, mute_label)
                        .checked(muted.unwrap_or(false))
                        .enabled(muted.is_some()),
                ),
            ],
        )
    }

    /// The Open-1:1 submenu body: one entry per peer contact (its host is its
    /// username, lock 2), the open one checked; an honest caption before a
    /// roster / with no peers (§7 — never a fabricated contact).
    fn open_entries(state: &ChatState) -> Vec<Entry<MenuAction>> {
        let Some(roster) = state.roster.as_ref() else {
            return vec![Entry::Caption(
                "No roster yet — the chat worker publishes it once this node is on the mesh Bus."
                    .to_owned(),
            )];
        };
        let mut entries = Vec::new();
        for contact in roster.contacts() {
            if roster.is_self(&contact.host) {
                continue; // self has no 1:1 (lock 17 — no notes-to-self)
            }
            let open = state.selected == Some(Selection::Contact(contact.host.clone()));
            entries.push(Entry::Item(
                Item::new(
                    MenuAction::OpenContact(contact.host.clone()),
                    contact.display_name(),
                )
                .checked(open),
            ));
        }
        if entries.is_empty() {
            entries.push(Entry::Caption(
                "No peer contacts yet — every enrolled mesh host appears here.".to_owned(),
            ));
        }
        entries
    }

    /// The Mark-Read submenu body: one entry per conversation carrying unread
    /// (contacts, then rooms), labelled with its live count; an honest caption
    /// when nothing is unread.
    fn mark_read_entries(state: &ChatState) -> Vec<Entry<MenuAction>> {
        let mut entries = Vec::new();
        let notifications_unread = state.notifications_unread();
        if notifications_unread > 0 {
            entries.push(Entry::Item(Item::new(
                MenuAction::MarkRead(Selection::Notifications),
                format!("Notifications ({notifications_unread})"),
            )));
        }
        for host in state.convos.keys() {
            let unread = state.unread(host);
            if unread > 0 {
                entries.push(Entry::Item(Item::new(
                    MenuAction::MarkRead(Selection::Contact(host.clone())),
                    format!("{host} ({unread})"),
                )));
            }
        }
        for room in &state.rooms {
            let unread = state.room_unread(&room.id);
            if unread > 0 {
                entries.push(Entry::Item(Item::new(
                    MenuAction::MarkRead(Selection::Room(room.id.clone())),
                    format!("{} ({unread})", room.name),
                )));
            }
        }
        if entries.is_empty() {
            entries.push(Entry::Caption("Nothing unread.".to_owned()));
        }
        entries
    }

    /// The **View** menu — the feed filters (timeline kinds) + Unread Only
    /// (roster pruning), each a live checked toggle over real view state.
    fn build_view_menu(state: &ChatState) -> Menu<MenuAction> {
        Menu::new(
            "View",
            vec![
                Entry::Caption("Feed filters".to_owned()),
                Entry::Item(
                    Item::new(MenuAction::ToggleAlerts, "Alerts").checked(state.show_alerts),
                ),
                Entry::Item(
                    Item::new(MenuAction::ToggleClips, "Clipboard Clips").checked(state.show_clips),
                ),
                Entry::Item(
                    Item::new(MenuAction::ToggleMessages, "Messages").checked(state.show_messages),
                ),
                Entry::Separator,
                Entry::Item(
                    Item::new(MenuAction::ToggleUnreadOnly, "Unread Only")
                        .checked(state.unread_only),
                ),
            ],
        )
    }

    /// The **Chat** menu — send the draft, clear all unread, close the pane.
    fn build_chat_menu(state: &ChatState) -> Menu<MenuAction> {
        Menu::new(
            "Chat",
            vec![
                Entry::Item(
                    Item::new(MenuAction::SendDraft, "Send Message")
                        .shortcut("Enter")
                        .enabled(can_send(state)),
                ),
                Entry::Item(
                    Item::new(MenuAction::ClearUnread, "Clear Unread")
                        .enabled(state.total_unread() > 0),
                ),
                Entry::Separator,
                Entry::Item(
                    Item::new(MenuAction::CloseConversation, "Close Conversation")
                        .enabled(state.selected.is_some()),
                ),
            ],
        )
    }

    /// The **Presence** menu (present only once a roster / self contact exists).
    fn build_presence_menu(state: &ChatState, current: PresenceChoice) -> Menu<MenuAction> {
        let mut entries: Vec<Entry<MenuAction>> = PresenceChoice::ALL
            .iter()
            .map(|&c| {
                Entry::Item(Item::new(MenuAction::SetPresence(c), c.label()).checked(c == current))
            })
            .collect();
        entries.push(Entry::Separator);
        entries.push(Entry::Item(
            Item::new(MenuAction::EditStatus, "Edit Status...").enabled(!state.editing_status),
        ));
        Menu::new("Presence", entries)
    }

    /// The **Help** menu — honest identity captions (this surface has no manual
    /// or note lane, so nothing pretends to be activatable — §8).
    fn build_help_menu() -> Menu<MenuAction> {
        Menu::new(
            "Help",
            vec![
                Entry::Caption(
                    "Contacts - every mesh host is a contact; its alerts and clipboard \
                     clips arrive as its messages."
                        .to_owned(),
                ),
                Entry::Caption(
                    "Delivery marks are presence-derived: Sent, Delivered, or Queued - never a \
                     fabricated read receipt."
                        .to_owned(),
                ),
            ],
        )
    }

    /// The status-chip tone for a presence choice (Ok = reachable, Warn = away,
    /// Danger = DND, Neutral = invisible) — mirrors [`super::presence_color`].
    const fn presence_tone(c: PresenceChoice) -> ChipTone {
        match c {
            PresenceChoice::FreeForChat | PresenceChoice::Available => ChipTone::Ok,
            PresenceChoice::Away => ChipTone::Warn,
            PresenceChoice::Dnd => ChipTone::Danger,
            PresenceChoice::Invisible => ChipTone::Neutral,
        }
    }

    /// The live status cluster (MENU-2 chips): this seat's presence, the peers
    /// online, and the whole-mesh unread tally — all gated on a roster existing
    /// (no roster ⇒ nothing to honestly count, §7).
    fn build_status(state: &ChatState) -> Vec<StatusChip> {
        let Some(roster) = state.roster.as_ref() else {
            return Vec::new();
        };
        let current = PresenceChoice::from_presence(roster.self_contact().presence);
        let online = roster
            .online()
            .into_iter()
            .filter(|c: &&Contact| !roster.is_self(&c.host))
            .count();
        let unread = state.total_unread();
        vec![
            StatusChip::with_icon(DOT, current.label(), presence_tone(current)),
            StatusChip::new(
                format!("{online} online"),
                if online > 0 {
                    ChipTone::Ok
                } else {
                    ChipTone::Neutral
                },
            ),
            StatusChip::new(
                format!("{unread} unread"),
                if unread > 0 {
                    ChipTone::Info
                } else {
                    ChipTone::Neutral
                },
            ),
        ]
    }

    /// Watermark one conversation read — the same `seen` write opening its pane
    /// performs, so the menu and the pane can't diverge.
    fn mark_read(state: &mut ChatState, sel: &Selection) {
        match sel {
            Selection::Notifications => {
                let now = state.notification_items().len();
                state
                    .seen
                    .insert(super::NOTIFICATIONS_SEEN_KEY.to_string(), now);
            }
            Selection::Contact(host) => {
                let now = state.convos.get(host).map_or(0, Conversation::len);
                state.seen.insert(host.clone(), now);
            }
            Selection::Room(id) => {
                let now = state.room_convos.get(id).map_or(0, Conversation::len);
                state.seen.insert(super::room_key(id), now);
            }
        }
    }

    /// Apply a picked action to its real seam (§6, no new behaviour).
    pub(super) fn apply(state: &mut ChatState, action: MenuAction) {
        match action {
            MenuAction::OpenContact(host) => {
                // The roster row's exact open seam: select + fresh draft.
                state.selected = Some(Selection::Contact(host));
                state.draft.clear();
            }
            MenuAction::MarkRead(sel) => mark_read(state, &sel),
            MenuAction::MuteConversation => {
                let Some(sel) = state.selected.clone() else {
                    return;
                };
                let (target, id, muted) = match &sel {
                    Selection::Notifications => return,
                    Selection::Contact(host) => {
                        ("contact", host.clone(), state.is_contact_muted(host))
                    }
                    Selection::Room(id) => ("room", id.clone(), state.is_room_muted(id)),
                };
                // The SAME publish the row mute button uses — flip the persisted policy.
                if let Err(e) = super::publish_mute(state.bus_root.as_deref(), target, &id, !muted)
                {
                    state.last_error = Some(e);
                }
            }
            MenuAction::CloseConversation => state.selected = None,
            MenuAction::ToggleAlerts => state.show_alerts = !state.show_alerts,
            MenuAction::ToggleClips => state.show_clips = !state.show_clips,
            MenuAction::ToggleMessages => state.show_messages = !state.show_messages,
            MenuAction::ToggleUnreadOnly => state.unread_only = !state.unread_only,
            MenuAction::SendDraft => {
                // The composer's exact send seam, gated exactly like `can_send`.
                let text = state.draft.trim().to_string();
                if text.is_empty() {
                    return;
                }
                match state.selected.clone() {
                    Some(Selection::Contact(host)) => {
                        if state.roster.as_ref().is_some_and(|r| !r.is_self(&host)) {
                            // shell-ux-11: keep the draft + surface on a failed send.
                            match state.send(&host, &text) {
                                Ok(()) => {
                                    state.draft.clear();
                                    state.last_error = None;
                                }
                                Err(e) => state.last_error = Some(e),
                            }
                        }
                    }
                    Some(Selection::Room(id)) => {
                        if state.room_descriptor(&id).is_some() {
                            match state.send_room(&id, &text) {
                                Ok(()) => {
                                    state.draft.clear();
                                    state.last_error = None;
                                }
                                Err(e) => state.last_error = Some(e),
                            }
                        }
                    }
                    Some(Selection::Notifications) | None => {}
                }
            }
            MenuAction::ClearUnread => {
                // Watermark everything at its current length — the same `seen`
                // mechanism the pane + Mark Read use, applied mesh-wide.
                let contacts: Vec<(String, usize)> = state
                    .convos
                    .iter()
                    .map(|(h, c)| (h.clone(), c.len()))
                    .collect();
                for (host, len) in contacts {
                    state.seen.insert(host, len);
                }
                let rooms: Vec<(String, usize)> = state
                    .room_convos
                    .iter()
                    .map(|(id, c)| (super::room_key(id), c.len()))
                    .collect();
                for (key, len) in rooms {
                    state.seen.insert(key, len);
                }
            }
            MenuAction::SetPresence(choice) => state.set_presence(choice),
            MenuAction::EditStatus => {
                if let Some(roster) = state.roster.as_ref() {
                    state.status_draft = roster
                        .self_contact()
                        .status_message
                        .clone()
                        .unwrap_or_default();
                }
                state.editing_status = true;
            }
        }
    }

    #[cfg(test)]
    #[allow(clippy::panic)]
    mod tests {
        use super::super::{alert_key, dm_key, ChatState, Selection};
        use super::{
            build_menus, build_status, can_send, presence_tone, MenuAction, PresenceChoice,
        };
        use mde_chat::{Contact, Conversation, Message, NodeRole, Presence, Roster};
        use mde_egui::menubar::Entry;
        use mde_egui::ChipTone;

        /// A roster with self ("eagle") + an online peer ("nyc3") and one unread
        /// message from that peer — the smallest live-looking state.
        fn peered_state() -> ChatState {
            let mut state = ChatState::default();
            let mut roster = Roster::new("eagle");
            roster.upsert(Contact::new("nyc3", NodeRole::Headless).with_presence(Presence::Online));
            state.roster = Some(roster);
            let mut conv = Conversation::new("nyc3");
            conv.insert(Message::text("nyc3", 10, "hi"));
            state.convos.insert("nyc3".into(), conv);
            state.seen.insert("nyc3".into(), 0); // one unread
            state
        }

        fn assert_ascii_entries(entries: &[Entry<MenuAction>]) {
            for entry in entries {
                match entry {
                    Entry::Item(item) => {
                        assert!(
                            item.label.is_ascii(),
                            "menu item copy should stay ASCII: {:?}",
                            item.label
                        );
                        if let Some(shortcut) = &item.shortcut {
                            assert!(
                                shortcut.is_ascii(),
                                "shortcut copy should stay ASCII: {shortcut:?}"
                            );
                        }
                    }
                    Entry::Submenu { label, entries, .. } => {
                        assert!(
                            label.is_ascii(),
                            "submenu copy should stay ASCII: {label:?}"
                        );
                        assert_ascii_entries(entries);
                    }
                    Entry::Separator => {}
                    Entry::Caption(caption) => {
                        assert!(
                            caption.is_ascii(),
                            "caption copy should stay ASCII: {caption:?}"
                        );
                    }
                }
            }
        }

        #[test]
        fn menu_copy_uses_ascii_labels_instead_of_pseudo_icons() {
            let menus = build_menus(&peered_state());
            for menu in &menus {
                assert!(
                    menu.title.is_ascii(),
                    "menu title should stay ASCII: {:?}",
                    menu.title
                );
                assert_ascii_entries(&menu.entries);
            }
        }

        #[test]
        fn bare_state_gates_honestly() {
            // No roster, nothing selected: Mute + the Chat verbs grey, the Open-1:1
            // submenu carries an honest caption, Presence is omitted (not a
            // present-but-dead menu) until a roster lands (§7).
            let state = ChatState::default();
            let menus = build_menus(&state);
            let titles: Vec<&str> = menus.iter().map(|m| m.title.as_str()).collect();
            assert_eq!(titles, ["Contacts", "View", "Chat", "Help"]);
            let contacts = &menus[0];
            let Entry::Submenu { entries, .. } = &contacts.entries[0] else {
                panic!("Contacts[0] is the Open 1:1 submenu");
            };
            assert!(
                matches!(entries.as_slice(), [Entry::Caption(_)]),
                "no roster ⇒ Open 1:1 holds one honest caption"
            );
            for entry in &contacts.entries {
                if let Entry::Item(item) = entry {
                    assert!(!item.enabled, "{} greys with no selection", item.label);
                }
            }
            let chat = menus.iter().find(|m| m.title == "Chat").expect("Chat menu");
            for entry in &chat.entries {
                if let Entry::Item(item) = entry {
                    assert!(!item.enabled, "{} greys on a bare state", item.label);
                }
            }
            assert!(build_status(&state).is_empty(), "no roster ⇒ no chips (§7)");
        }

        #[test]
        fn open_1to1_lists_peers_and_opens_the_pane() {
            let mut state = peered_state();
            let menus = build_menus(&state);
            let Entry::Submenu { entries, .. } = &menus[0].entries[0] else {
                panic!("Contacts[0] is the Open 1:1 submenu");
            };
            let ids: Vec<&MenuAction> = entries
                .iter()
                .filter_map(|e| match e {
                    Entry::Item(i) => Some(&i.id),
                    _ => None,
                })
                .collect();
            assert_eq!(
                ids,
                [&MenuAction::OpenContact("nyc3".into())],
                "one entry per peer; self is excluded (lock 17)"
            );
            super::apply(&mut state, MenuAction::OpenContact("nyc3".into()));
            assert_eq!(
                state.selected,
                Some(Selection::Contact("nyc3".into())),
                "Open 1:1 selects the pane"
            );
        }

        #[test]
        fn mark_read_and_clear_unread_watermark_via_the_seen_map() {
            let mut state = peered_state();
            assert_eq!(state.total_unread(), 1);
            // The Mark-Read submenu lists exactly the unread conversation.
            let menus = build_menus(&state);
            let Entry::Submenu { entries, .. } = &menus[0].entries[1] else {
                panic!("Contacts[1] is the Mark Read submenu");
            };
            assert!(
                entries.iter().any(|e| matches!(
                    e,
                    Entry::Item(i) if i.id == MenuAction::MarkRead(Selection::Contact("nyc3".into()))
                )),
                "the unread contact is listed"
            );
            super::apply(
                &mut state,
                MenuAction::MarkRead(Selection::Contact("nyc3".into())),
            );
            assert_eq!(state.total_unread(), 0, "Mark Read watermarks it");

            // Regrow an unread, then Clear Unread wipes the whole tally.
            state.seen.insert("nyc3".into(), 0);
            assert_eq!(state.total_unread(), 1);
            super::apply(&mut state, MenuAction::ClearUnread);
            assert_eq!(
                state.total_unread(),
                0,
                "Clear Unread watermarks everything"
            );
        }

        #[test]
        fn view_toggles_flip_the_live_filter_state() {
            let mut state = ChatState::default();
            assert!(state.show_alerts && state.show_clips && state.show_messages);
            assert!(!state.unread_only);
            super::apply(&mut state, MenuAction::ToggleAlerts);
            super::apply(&mut state, MenuAction::ToggleUnreadOnly);
            assert!(!state.show_alerts, "Alerts toggled off");
            assert!(state.unread_only, "Unread Only toggled on");
            // The menu reflects the flipped state as its check-marks.
            let menus = build_menus(&state);
            let view = menus.iter().find(|m| m.title == "View").expect("View menu");
            let checks: Vec<Option<bool>> = view
                .entries
                .iter()
                .filter_map(|e| match e {
                    Entry::Item(i) => Some(i.checked),
                    _ => None,
                })
                .collect();
            assert_eq!(
                checks,
                [Some(false), Some(true), Some(true), Some(true)],
                "Alerts off · Clips on · Messages on · Unread Only on"
            );
        }

        #[test]
        fn send_draft_gates_like_the_composer_and_keeps_the_draft_on_bus_failure() {
            let mut state = peered_state();
            assert!(!can_send(&state), "empty draft can't send");
            state.draft = "hello".into();
            assert!(!can_send(&state), "no open conversation can't send");
            state.selected = Some(Selection::Contact("eagle".into()));
            assert!(!can_send(&state), "the self alert timeline has no composer");
            state.selected = Some(Selection::Contact("nyc3".into()));
            assert!(can_send(&state));
            // shell-ux-11: with no Bus dir the publish fails, so the draft is KEPT
            // (never silently destroyed) and the failure is surfaced inline.
            state.bus_root = None;
            super::apply(&mut state, MenuAction::SendDraft);
            assert_eq!(
                state.draft, "hello",
                "a failed send keeps the draft to retry"
            );
            assert!(
                state.last_error.is_some(),
                "a failed send surfaces why it didn't go"
            );
        }

        #[test]
        fn send_draft_clears_the_draft_on_a_successful_publish() {
            // shell-ux-11: the composer's other half — a successful publish clears
            // the draft AND any prior error note (the same seam the composer runs).
            let tmp = tempfile::tempdir().expect("tempdir");
            let mut state = peered_state();
            state.bus_root = Some(tmp.path().join("bus"));
            state.selected = Some(Selection::Contact("nyc3".into()));
            state.draft = "hello".into();
            state.last_error = Some("a stale error".into());
            super::apply(&mut state, MenuAction::SendDraft);
            assert!(state.draft.is_empty(), "a successful send clears the draft");
            assert!(
                state.last_error.is_none(),
                "a successful send clears the prior error note"
            );
        }

        #[test]
        fn close_deselects_and_set_presence_maps_to_a_choice() {
            let mut state = ChatState {
                selected: Some(Selection::Contact("nyc3".into())),
                ..ChatState::default()
            };
            super::apply(&mut state, MenuAction::CloseConversation);
            assert!(state.selected.is_none(), "Close deselects the conversation");
            // Edit Status opens the inline editor (a local toggle seam).
            assert!(!state.editing_status);
            super::apply(&mut state, MenuAction::EditStatus);
            assert!(state.editing_status, "Edit Status opens the editor");
            // set_presence publishes best-effort (no Bus dir in the test) — never a
            // panic, and the action carries the chosen presence.
            super::apply(&mut state, MenuAction::SetPresence(PresenceChoice::Dnd));
        }

        #[test]
        fn chips_read_presence_online_count_and_unread_total() {
            let state = peered_state();
            let chips = build_status(&state);
            assert!(
                chips
                    .iter()
                    .any(|c| c.text == "1 online" && c.tone == ChipTone::Ok),
                "one peer online (self excluded)"
            );
            assert!(
                chips
                    .iter()
                    .any(|c| c.text == "1 unread" && c.tone == ChipTone::Info),
                "the whole-mesh unread tally"
            );
            // The presence chip mirrors the self contact.
            assert!(chips.iter().any(|c| c.text == "Available"));
            // The DM ∪ alert key helpers stay order-stable (the fold contract).
            assert_eq!(dm_key("eagle", "nyc3"), dm_key("nyc3", "eagle"));
            assert_eq!(alert_key("nyc3"), "alert:nyc3");
        }

        #[test]
        fn presence_tones_are_distinct_across_the_reachability_bands() {
            assert_eq!(presence_tone(PresenceChoice::Available), ChipTone::Ok);
            assert_eq!(presence_tone(PresenceChoice::Away), ChipTone::Warn);
            assert_eq!(presence_tone(PresenceChoice::Dnd), ChipTone::Danger);
            assert_eq!(presence_tone(PresenceChoice::Invisible), ChipTone::Neutral);
        }

        #[test]
        fn menu_bar_renders_headless() {
            use mde_egui::egui::{self, pos2, vec2, Rect};
            use mde_egui::Style;
            let ctx = egui::Context::default();
            Style::install(&ctx);
            let state = ChatState::default();
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1024.0, 640.0))),
                ..Default::default()
            };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = super::show(&state, ui);
                });
            });
            let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
            assert!(!prims.is_empty(), "the Contacts bar produced no primitives");
        }
    }
}

/// Derive a stable, canonical room id from an operator-typed name: lowercase, ASCII
/// alphanumerics kept, every other run collapsed to a single `-`, trimmed. So
/// "Build Farm!" → "build-farm" — the same id on every node that types that name.
fn room_id_from_name(name: &str) -> String {
    let mut id = String::new();
    let mut prev_dash = false;
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            id.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !id.is_empty() {
            id.push('-');
            prev_dash = true;
        }
    }
    id.trim_end_matches('-').to_string()
}

/// Publish `body` to `topic` on the local Bus via the persist-first path (the same
/// discipline as [`ChatState::send`] and `discovery::publish`). Returns the failure
/// reason (missing Bus dir / open error / write error) so a caller carrying user
/// intent — a composed message — can KEEP the draft and surface why it didn't send
/// (shell-ux-11), instead of the message being silently destroyed. Never panics.
/// Callers whose intent is a retriable, worker-republished toggle stay best-effort
/// with an explicit `let _ = publish(…)`.
fn publish(bus_root: Option<&Path>, topic: &str, body: &str) -> Result<(), String> {
    let Some(root) = bus_root else {
        return Err("No local Bus — the mesh daemon may be down.".to_string());
    };
    // arch-11: writer — the shared BusReader seam is read-only; this publish keeps
    // Persist::open because it needs the open/write error text for the caller.
    let persist = Persist::open(root.to_path_buf())
        .map_err(|e| format!("Couldn't open the local Bus: {e}"))?;
    persist
        .write(topic, Priority::Default, None, Some(body))
        .map_err(|e| format!("Bus write failed: {e}"))?;
    Ok(())
}

/// Raise a click-to-navigate chyron on the shell's ONE toast lane
/// (`event/toast/show`) so KIRON-2's bridge — the shell's single navigation
/// authority (main.rs owns `nav.surface`) — carries the operator to `verb`'s
/// target. The Chat surface never mutates `nav.surface` itself (it must not touch
/// the dock/Surface plumbing); routing the resolved verb through the existing
/// consumer is how a chat action reaches shell navigation.
fn navigate_via_toast(bus_root: Option<&Path>, source_host: &str, headline: &str, verb: &str) {
    let body = serde_json::json!({
        "severity": "info",
        "source_host": source_host,
        "flag": "CHAT",
        "headline": headline,
        "action_label": "Open",
        "action_verb": verb,
    })
    .to_string();
    // Best-effort: a navigation chyron, not user-composed content.
    let _ = publish(bus_root, TOAST_TOPIC, &body);
}

/// perf-5 — fold a conversation's constituent latest-wins ring blobs (a
/// contact's `dm:` ∪ `alert:` rings, or a room's single `room:<id>` ring) into
/// one [`Conversation`], oldest-first and deduped by id ([`Conversation::insert`]
/// is idempotent + canonically ordered, lock 8/22). This is the EXACT per-
/// conversation build the old full-rebuild refresh did inline; `refresh` now only
/// calls it when [`conversation_is_current`] reports a constituent blob moved, so
/// an unchanged conversation is never re-decoded. Factored out as a pure seam so
/// the incremental-vs-full-rebuild equivalence is unit-testable without a bus.
fn fold_rings(id: &str, rings: impl IntoIterator<Item = Vec<Message>>) -> Conversation {
    let mut conv = Conversation::new(id);
    for ring in rings {
        for msg in ring {
            conv.insert(msg);
        }
    }
    conv
}

/// perf-5 — the reuse predicate for one conversation: `true` when the already-
/// built [`Conversation`] is still valid and must be reused as-is; `false` when
/// it must be rebuilt. Reuse requires BOTH an existing conversation AND that
/// every constituent topic's latest ULID (`now`) matches the cursors captured at
/// the last build (`prev`). A first load (`prev` is `None` / `has_existing`
/// false) rebuilds; a moved blob — a new message OR a retention trim, both of
/// which republish a new-ULID ring — mismatches and rebuilds, so the model always
/// resyncs to the current blobs (never appends onto a stale-trimmed ring).
fn conversation_is_current(
    prev: Option<&Vec<Option<String>>>,
    now: &[Option<String>],
    has_existing: bool,
) -> bool {
    has_existing && prev.map(Vec::as_slice) == Some(now)
}

/// Read the newest (latest-wins) message on `topic` and deserialize its body.
fn latest_json<T: serde::de::DeserializeOwned>(persist: &Persist, topic: &str) -> Option<T> {
    // perf-4 — a bounded `read_latest` (ORDER BY ulid DESC LIMIT 1) returns the
    // exact same row the old `list_since(topic, None).last()` did, without
    // loading the whole retained history on every render.
    let msg = persist.read_latest(topic).ok()??;
    let body = msg.body.as_deref()?;
    serde_json::from_str::<T>(body).ok()
}

fn latest_topic_suffixes(persist: &Persist, prefix: &str) -> BTreeSet<String> {
    persist
        .list_topics()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|topic| {
            let suffix = topic.strip_prefix(prefix)?;
            (!suffix.is_empty() && latest_json::<serde_json::Value>(persist, &topic).is_some())
                .then(|| suffix.to_string())
        })
        .collect()
}

mod render;
use render::*;
// The shell chrome + integration tests reach these leaf helpers by their
// `chat::…` path (curtain/timers fold dates through the seat's ONE calendar,
// §6; `main.rs` probes the alert-action / DND button ids), so re-export them
// at the surface root with their original visibility after the arch split.
pub use render::civil_from_days;
pub(crate) use render::notification_dnd_toggle_id;
// `alert_action_button_id` is reached only by the `main.rs` integration tests
// (the parent + render loop call it internally), so its re-export is test-only.
#[cfg(test)]
pub(crate) use render::alert_action_button_id;

#[cfg(test)]
mod tests;
