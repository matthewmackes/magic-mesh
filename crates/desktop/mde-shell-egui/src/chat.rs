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
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_chat::{
    Contact, Conversation, Message, MessageKind, NotifyPrefs, Presence, RoomDescriptor, RoomKind,
    Roster, Severity,
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

/// What the operator has open in the conversation pane: a 1:1 contact (its merged
/// human+alert timeline) or a room (its shared log). A single selection so exactly
/// one pane is open at a time (the ICQ single-window idiom on a DRM seat).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Selection {
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

    /// The ICQ-style checkmark glyph + label.
    const fn badge(self) -> (&'static str, &'static str) {
        match self {
            Self::Sent => ("✓", "Sent"),
            Self::Delivered => ("✓✓", "Delivered"),
            Self::Queued => ("⧗", "Queued — offline"),
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
        Severity::Critical => Style::DANGER,
        Severity::Warning => Style::WARN,
        Severity::Info => Style::ACCENT,
    }
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
    last_poll: Option<Instant>,
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
            show_alerts: true,
            show_clips: true,
            show_messages: true,
            unread_only: false,
            last_poll: None,
        }
    }
}

impl ChatState {
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
        let Some(root) = self.bus_root.clone() else {
            return;
        };
        let Ok(persist) = Persist::open(root) else {
            return;
        };
        if let Some(roster) = latest_json::<Roster>(&persist, ROSTER_TOPIC) {
            self.roster = Some(roster);
        }
        if let Some(prefs) = latest_json::<NotifyPrefs>(&persist, NOTIFY_TOPIC) {
            self.notify = Some(prefs);
        }
        let Some(roster) = &self.roster else {
            return;
        };
        let self_host = roster.self_host().to_string();
        let mut convos = BTreeMap::new();
        for contact in roster.contacts() {
            let mut conv = Conversation::new(contact.host.as_str());
            for key in keys_for_contact(&self_host, &contact.host) {
                if let Some(ring) = latest_json::<Vec<Message>>(&persist, &conversation_topic(&key))
                {
                    for msg in ring {
                        conv.insert(msg);
                    }
                }
            }
            // Watermark a first-seen contact at its current length so existing
            // backfill isn't flagged unread; keep an established watermark.
            self.seen.entry(contact.host.clone()).or_insert(conv.len());
            convos.insert(contact.host.clone(), conv);
        }
        self.convos = convos;

        // Rooms (NOTIFY-CHAT-5): the registry mirror + each room's shared log.
        if let Some(rooms) = latest_json::<Vec<RoomDescriptor>>(&persist, ROOMS_TOPIC) {
            self.rooms = rooms;
        }
        let mut room_convos = BTreeMap::new();
        for descriptor in &self.rooms {
            let mut conv = Conversation::new(descriptor.id.as_str());
            if let Some(ring) = latest_json::<Vec<Message>>(
                &persist,
                &conversation_topic(&room_key(&descriptor.id)),
            ) {
                for msg in ring {
                    conv.insert(msg);
                }
            }
            self.seen
                .entry(room_key(&descriptor.id))
                .or_insert(conv.len());
            room_convos.insert(descriptor.id.clone(), conv);
        }
        self.room_convos = room_convos;
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
            let (title, subtitle) = empty_copy(self.bus_root.is_some());
            crate::session::empty_state(ui, title, subtitle);
            return;
        };

        egui::SidePanel::left("chat-roster")
            .resizable(true)
            .default_width(Style::SP_XL * 7.0)
            .show_inside(ui, |ui| {
                self.roster_rail(ui, &roster);
            });

        match self.selected.clone() {
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
            _ => {
                crate::session::empty_state(
                    ui,
                    "Pick a contact or room",
                    "Select a host or a room on the left to open its conversation — a contact's \
                     messages and its alerts share one timeline.",
                );
            }
        }
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
                    .hint_text("Set a status…");
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
                .hint_text("New room name…");
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
                contact_actions(ui, bus_root.as_deref(), host, muted);
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

    /// The message composer — a text field + Send that writes `action/chat/send`.
    fn composer(&mut self, ui: &mut egui::Ui, host: &str, recipient: Option<&Contact>) {
        ui.add_space(Style::SP_XS);
        let mut send = false;
        ui.horizontal(|ui| {
            let field = egui::TextEdit::singleline(&mut self.draft)
                .desired_width(f32::INFINITY)
                .hint_text("Message…");
            let resp = ui.add(field);
            send = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("Send").clicked() {
                send = true;
            }
        });
        // The recipient's presence previews how this message will deliver.
        if let Some(c) = recipient {
            let (glyph, label) = Delivery::for_recipient(Some(c)).badge();
            mde_egui::muted_note(ui, format!("{glyph} {label} → {}", c.presence.label()));
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
            mde_egui::muted_note(ui, "\u{1F4CE}"); // 📎
            let field = egui::TextEdit::singleline(&mut self.attach_path)
                .desired_width(f32::INFINITY)
                .hint_text("Attach a file — path, or drop one here…");
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
            self.send(host, &text);
            self.draft.clear();
        }
        let path = self.attach_path.trim().to_string();
        if send_file && !path.is_empty() {
            self.send_file(host, Path::new(&path));
            self.attach_path.clear();
        }
    }

    /// Publish `action/chat/send` `{scope:"peer", to, text}` to the local Bus —
    /// the worker signs, persists, and relays it (best-effort; a missing Bus is a
    /// silent no-op, the honest solo-host state).
    fn send(&self, to: &str, text: &str) {
        let body = serde_json::json!({ "scope": "peer", "to": to, "text": text }).to_string();
        publish(self.bus_root.as_deref(), ACTION_CHAT_SEND, &body);
    }

    /// Offer `path` to the `to` contact (lock 15, file kind): fire the real
    /// `mde-files` Send-To so the bytes copy into the peer's replicated inbox
    /// (reachable now), AND post the offer into the conversation as a chat message
    /// carrying an inline `file` descriptor. The worker relaying the descriptor into
    /// a rich [`MessageKind::File`] card is the integration seam — until then the
    /// offer still shows as its human-readable `text`, never faked.
    fn send_file(&self, to: &str, path: &Path) {
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
        publish(self.bus_root.as_deref(), ACTION_FILE_SEND_TO, &send_to);
        // 2) The conversation offer — human text now, an upgradeable `file` field for
        //    the worker's File-card fold.
        let offer = serde_json::json!({
            "scope": "peer",
            "to": to,
            "text": format!("\u{1F4CE} sent file {name} ({size_bytes} bytes)"),
            "file": { "name": name, "size_bytes": size_bytes },
        })
        .to_string();
        publish(self.bus_root.as_deref(), ACTION_CHAT_SEND, &offer);
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
            mute_button(ui, bus_root.as_deref(), "room", id, room_muted);
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
                .hint_text("Message the room…");
            let resp = ui.add(field);
            send = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("Send").clicked() {
                send = true;
            }
        });
        mde_egui::muted_note(ui, format!("\u{2192} {members} members"));
        ui.add_space(Style::SP_XS);
        let text = self.draft.trim().to_string();
        if send && !text.is_empty() {
            self.send_room(id, &text);
            self.draft.clear();
        }
    }

    /// Publish `action/chat/send` `{scope:"room", to:<id>, text}` — the worker signs
    /// it, appends to the room's shared Syncthing log, and fans it out to each online
    /// member (best-effort; a missing Bus is a silent no-op).
    fn send_room(&self, id: &str, text: &str) {
        let body = serde_json::json!({ "scope": "room", "to": id, "text": text }).to_string();
        publish(self.bus_root.as_deref(), ACTION_CHAT_SEND, &body);
    }

    /// Create an ad-hoc room named `name`: derive a stable id from the name and fire
    /// the worker's `action/chat/room` create op (the worker owns + joins it).
    fn create_room(&self, name: &str) {
        let id = room_id_from_name(name);
        if id.is_empty() {
            return;
        }
        let body = serde_json::json!({ "op": "create", "id": id, "name": name }).to_string();
        publish(self.bus_root.as_deref(), ACTION_CHAT_ROOM, &body);
    }

    /// Fire an `action/chat/room` lifecycle op (`join` / `dissolve`) for room `id`.
    fn room_action(&self, op: &str, id: &str, name: Option<&str>) {
        let mut obj = serde_json::json!({ "op": op, "id": id });
        if let Some(n) = name {
            obj["name"] = serde_json::Value::String(n.to_string());
        }
        publish(self.bus_root.as_deref(), ACTION_CHAT_ROOM, &obj.to_string());
    }

    /// Post `action/chat/presence` to set this seat's presence (Available ⇒ clear
    /// to auto). The worker updates its self-presence, gossips it, and republishes
    /// the self roster entry the roster rail then reads back.
    fn set_presence(&self, choice: PresenceChoice) {
        let body = serde_json::json!({ "presence": choice.wire() }).to_string();
        publish(self.bus_root.as_deref(), ACTION_CHAT_PRESENCE, &body);
    }

    /// Post `action/chat/presence` to set this seat's free-text status (empty ⇒
    /// clear at the worker). Carried on the same action the worker republishes.
    fn set_status(&self, status: Option<&str>) {
        let body = serde_json::json!({ "status": status }).to_string();
        publish(self.bus_root.as_deref(), ACTION_CHAT_PRESENCE, &body);
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
            Item::new(MenuAction::EditStatus, "Edit Status\u{2026}").enabled(!state.editing_status),
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
                    "Contacts \u{2014} every mesh host is a contact; its alerts and clipboard \
                     clips arrive as its messages."
                        .to_owned(),
                ),
                Entry::Caption(
                    "Delivery marks are presence-derived (\u{2713} sent \u{00B7} \u{2713}\u{2713} \
                     delivered \u{00B7} \u{29D7} queued) \u{2014} never a fabricated read receipt."
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
                    Selection::Contact(host) => {
                        ("contact", host.clone(), state.is_contact_muted(host))
                    }
                    Selection::Room(id) => ("room", id.clone(), state.is_room_muted(id)),
                };
                // The SAME publish the row mute button uses — flip the persisted policy.
                super::publish_mute(state.bus_root.as_deref(), target, &id, !muted);
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
                            state.send(&host, &text);
                            state.draft.clear();
                        }
                    }
                    Some(Selection::Room(id)) => {
                        if state.room_descriptor(&id).is_some() {
                            state.send_room(&id, &text);
                            state.draft.clear();
                        }
                    }
                    None => {}
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
        fn send_draft_gates_like_the_composer_and_clears_on_send() {
            let mut state = peered_state();
            assert!(!can_send(&state), "empty draft can't send");
            state.draft = "hello".into();
            assert!(!can_send(&state), "no open conversation can't send");
            state.selected = Some(Selection::Contact("eagle".into()));
            assert!(!can_send(&state), "the self alert timeline has no composer");
            state.selected = Some(Selection::Contact("nyc3".into()));
            assert!(can_send(&state));
            // The publish is best-effort (no Bus dir in a test) — the draft still
            // clears, proving the send seam ran.
            state.bus_root = None;
            super::apply(&mut state, MenuAction::SendDraft);
            assert!(state.draft.is_empty(), "a sent draft clears");
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
/// discipline as [`ChatState::send`] and `discovery::publish`). Best-effort — a
/// missing Bus directory / open failure is a silent no-op (the honest solo-host
/// state), never a panic.
fn publish(bus_root: Option<&Path>, topic: &str, body: &str) {
    let Some(root) = bus_root else {
        return;
    };
    let Ok(persist) = Persist::open(root.to_path_buf()) else {
        return;
    };
    let _ = persist.write(topic, Priority::Default, None, Some(body));
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
    publish(bus_root, TOAST_TOPIC, &body);
}

/// Read the newest (latest-wins) message on `topic` and deserialize its body.
fn latest_json<T: serde::de::DeserializeOwned>(persist: &Persist, topic: &str) -> Option<T> {
    let msgs = persist.list_since(topic, None).ok()?;
    let body = msgs.last()?.body.as_deref()?;
    serde_json::from_str::<T>(body).ok()
}

/// Render a conversation's messages with a muted **day separator** whenever the
/// civil (UTC) date changes — the authentic chat idiom — each row carrying its own
/// HH:MM timestamp ([`message_row`]). Shared by the contact + room panes so both
/// read the same way. Takes the panes' already-feed-filtered slice (MENU-2), so
/// the day separators track only what actually renders.
fn render_timeline(
    ui: &mut egui::Ui,
    messages: &[&Message],
    self_host: &str,
    recipient: Option<&Contact>,
    bus_root: Option<&Path>,
) {
    let mut last_date: Option<String> = None;
    for &msg in messages {
        let date = fmt_date(msg.ts_unix_ms);
        if last_date.as_deref() != Some(date.as_str()) {
            day_separator(ui, &date);
            last_date = Some(date);
        }
        message_row(ui, msg, self_host, recipient, bus_root);
        ui.add_space(Style::SP_XS);
    }
}

/// A centered, token-muted day-separator chip in the timeline.
fn day_separator(ui: &mut egui::Ui, date: &str) {
    ui.add_space(Style::SP_XS);
    ui.vertical_centered(|ui| {
        mde_egui::muted_note(ui, date);
    });
    ui.add_space(Style::SP_XS);
}

/// Compact wall-clock `HH:MM` (UTC) for a message's injected send time. Pure — no
/// external time crate (there is none in this DRM seat's deps); UTC so it never
/// claims a local zone it can't know. A non-positive timestamp yields "".
fn fmt_hh_mm(ts_unix_ms: i64) -> String {
    if ts_unix_ms <= 0 {
        return String::new();
    }
    let tod = (ts_unix_ms / 1000).rem_euclid(86_400);
    format!("{:02}:{:02}", tod / 3600, (tod % 3600) / 60)
}

/// A full `YYYY-MM-DD HH:MM UTC` stamp — the message-row hover.
fn fmt_full_datetime(ts_unix_ms: i64) -> String {
    if ts_unix_ms <= 0 {
        return "unknown time".to_string();
    }
    let secs = ts_unix_ms / 1000;
    let tod = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        tod / 3600,
        (tod % 3600) / 60
    )
}

/// The civil `YYYY-MM-DD` (UTC) date for a message — the day-separator key.
fn fmt_date(ts_unix_ms: i64) -> String {
    if ts_unix_ms <= 0 {
        return "unknown date".to_string();
    }
    let (year, month, day) = civil_from_days((ts_unix_ms / 1000).div_euclid(86_400));
    format!("{year:04}-{month:02}-{day:02}")
}

/// Civil `(year, month, day)` from a day-count since the Unix epoch
/// (1970-01-01), proleptic Gregorian. Howard Hinnant's `civil_from_days` — the
/// one piece of calendar math the DRM seat needs with no time crate on the deps.
/// Crate-visible: the taskbar tray's stacked clock (`tray::clock_lines`) folds
/// its date line through this same fn, so the shell has ONE calendar (§6).
/// (`pub`, not `pub(crate)`, is the `clippy::redundant_pub_crate` form here.)
pub fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = (if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    }) / 146_097;
    let day_of_era = shifted - era * 146_097; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153; // [0, 11]
    let day = day_of_year - (153 * month_index + 2) / 5 + 1; // [1, 31]
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    }; // [1, 12]
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Render one message row (human text, a clipboard copy, a folded alert card, or
/// a file/call/remote hand-off). Each kind renders **and acts** (NOTIFY-CHAT-4 —
/// re-copy, run an alert verb, download a file, re-launch Call / Remote); my own
/// outgoing text carries its delivery checkmark (lock 19).
fn message_row(
    ui: &mut egui::Ui,
    msg: &Message,
    self_host: &str,
    recipient: Option<&Contact>,
    bus_root: Option<&Path>,
) {
    let mine = msg.sender == self_host;
    ui.group(|ui| {
        ui.horizontal(|ui| {
            let who = if mine { "you" } else { msg.sender.as_str() };
            ui.label(
                RichText::new(who)
                    .color(if mine { Style::ACCENT } else { Style::TEXT })
                    .size(Style::SMALL)
                    .strong(),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if mine && matches!(&msg.kind, MessageKind::Text(_)) {
                    let delivery = Delivery::for_recipient(recipient);
                    let (glyph, label) = delivery.badge();
                    ui.colored_label(delivery.color(), RichText::new(glyph).size(Style::SMALL))
                        .on_hover_text(label);
                }
                // Compact HH:MM (UTC) send time, token-muted, full date on hover —
                // every message carries its injected timestamp (lock 22), so the
                // row is no longer time-blind (the biggest "looks incomplete" tell).
                let hhmm = fmt_hh_mm(msg.ts_unix_ms);
                if !hhmm.is_empty() {
                    ui.label(
                        RichText::new(hhmm)
                            .color(Style::TEXT_DIM)
                            .size(Style::SMALL),
                    )
                    .on_hover_text(fmt_full_datetime(msg.ts_unix_ms));
                }
            });
        });
        message_body(ui, msg, bus_root);
    });
}

/// The body of a message row, by kind — each kind now *acts*, not just renders
/// (NOTIFY-CHAT-4): a clipboard re-copies, an alert card runs its inline verb, a
/// file offers a download, a Call / Remote row re-launches its session.
fn message_body(ui: &mut egui::Ui, msg: &Message, bus_root: Option<&Path>) {
    match &msg.kind {
        // Text (emoji is just text — the font renders the glyphs verbatim).
        MessageKind::Text(text) => {
            ui.label(RichText::new(text).color(Style::TEXT).size(Style::BODY));
        }
        // Clipboard — monospace preview + a one-click re-copy onto the local
        // clipboard (egui's output command; the DRM/windowed backend owns the wire).
        MessageKind::Clipboard { preview, full } => {
            ui.horizontal(|ui| {
                mde_egui::muted_note(ui, "clipboard");
                ui.label(
                    RichText::new(preview)
                        .color(Style::TEXT)
                        .size(Style::BODY)
                        .monospace(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .button("Copy")
                        .on_hover_text("Re-copy to clipboard")
                        .clicked()
                    {
                        ui.ctx().copy_text(full.clone());
                    }
                });
            });
        }
        // System alert card — severity-colored, its fields listed, and (when the
        // fold carried a resolvable `action/shell/goto/<surface>`) an inline action
        // button that runs the verb through the shell's one navigation authority.
        MessageKind::Alert {
            severity,
            flag,
            fields,
            action_verb,
        } => {
            let title = fields
                .get("summary")
                .or_else(|| fields.get("title"))
                .map_or(flag.as_str(), String::as_str);
            ui.colored_label(
                severity_color(*severity),
                RichText::new(title).size(Style::BODY).strong(),
            );
            for (k, v) in fields {
                if k == "summary" || k == "title" {
                    continue; // already the card title
                }
                mde_egui::field(ui, k, v, Style::TEXT_DIM);
            }
            ui.horizontal(|ui| {
                mde_egui::muted_note(ui, format!("alert · {flag}"));
                if let Some(verb) = alert_nav_verb(action_verb.as_deref()) {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Go to \u{2192}").clicked() {
                            navigate_via_toast(bus_root, "chat", title, &verb);
                        }
                    });
                }
            });
        }
        // File offer — name + size and a download affordance. The bytes already
        // replicated into this node's mesh inbox (the sender's Send-To); "Save"
        // jumps to Files where they landed.
        MessageKind::File {
            name, size_bytes, ..
        } => {
            ui.horizontal(|ui| {
                mde_egui::muted_note(ui, "file");
                ui.label(RichText::new(name).color(Style::TEXT).size(Style::BODY));
                mde_egui::muted_note(ui, format!("{size_bytes} bytes"));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .button("Save")
                        .on_hover_text("Open the mesh inbox in Files")
                        .clicked()
                    {
                        navigate_via_toast(bus_root, "chat", name, "shell/goto/files");
                    }
                });
            });
        }
        // Call / Remote rows are a record of a launched session — re-launchable.
        MessageKind::CallAction { target_host } => {
            ui.horizontal(|ui| {
                mde_egui::field(ui, "call", target_host, Style::ACCENT);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Call again").clicked() {
                        dial_peer(bus_root, target_host);
                    }
                });
            });
        }
        MessageKind::RemoteAction { target_host } => {
            ui.horizontal(|ui| {
                mde_egui::field(ui, "remote control", target_host, Style::ACCENT);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Connect again").clicked() {
                        request_host_desktop(bus_root, target_host);
                    }
                });
            });
        }
    }
}

/// The per-contact action bar under the conversation header: **Call** (SIP),
/// **Remote Control** (VDI) — the two per-contact hand-offs of lock 15 — and a
/// **Mute** toggle (NOTIFY-CHAT-5) that silences this contact's messages + alerts.
/// Call/Remote fire their owning crate's Bus verb (the live SIP register+call and
/// VDI connect are integration-gated — the honest reachable launch, never a faked
/// session); Mute posts `action/chat/mute`, which the worker drains into its
/// `NotifyPrefs` and republishes so `muted` reflects the true persisted policy.
fn contact_actions(ui: &mut egui::Ui, bus_root: Option<&Path>, host: &str, muted: bool) {
    ui.horizontal(|ui| {
        if ui
            .button("\u{1F4DE} Call")
            .on_hover_text(format!("Place a SIP call to {host}"))
            .clicked()
        {
            dial_peer(bus_root, host);
        }
        if ui
            .button("\u{1F5A5} Remote Control")
            .on_hover_text(format!("Open {host}'s remote desktop"))
            .clicked()
        {
            request_host_desktop(bus_root, host);
        }
        mute_button(ui, bus_root, "contact", host, muted);
    });
}

/// A **Mute / Unmute** toggle for a contact or room. `muted` is the current
/// (worker-published) state; clicking posts `action/chat/mute` with the flipped
/// value so the worker updates + republishes the policy (the round-trip that
/// makes the toggle real, not a local-only switch — §7).
fn mute_button(ui: &mut egui::Ui, bus_root: Option<&Path>, target: &str, id: &str, muted: bool) {
    let (glyph, hint) = if muted {
        (
            "\u{1F514} Unmute",
            format!("Unmute {id} — let it ring again"),
        )
    } else {
        (
            "\u{1F515} Mute",
            format!("Mute {id} — silence its messages + alerts"),
        )
    };
    if ui.button(glyph).on_hover_text(hint).clicked() {
        publish_mute(bus_root, target, id, !muted);
    }
}

/// Publish `action/chat/mute` `{target, id, muted}` to the local Bus — the worker
/// drains it into this seat's `NotifyPrefs` (best-effort; a missing Bus is a
/// silent no-op, the honest solo-host state).
fn publish_mute(bus_root: Option<&Path>, target: &str, id: &str, muted: bool) {
    let body = serde_json::json!({ "target": target, "id": id, "muted": muted }).to_string();
    publish(bus_root, ACTION_CHAT_MUTE, &body);
}

/// Fire the voice worker's dial verb for `host` (lock 15 — Call hands off to
/// `mde-voice`). The publish is reachable now; a running SIP agent draining it +
/// the live register/call is integration-gated.
fn dial_peer(bus_root: Option<&Path>, host: &str) {
    let body = serde_json::json!({ "peer": host }).to_string();
    publish(bus_root, ACTION_VOICE_DIAL, &body);
}

/// Translate a folded alert's `action_verb` (`action/shell/goto/<surface>`, lock
/// 15) into the KIRON toast/nav grammar (`shell/goto/<surface>`), returning it only
/// when it resolves to a real shell target — the shell's ONE resolver
/// ([`resolve_action`]) is the gate, so a bare/unknown verb offers no button.
fn alert_nav_verb(action_verb: Option<&str>) -> Option<String> {
    let verb = action_verb?;
    let nav = verb.strip_prefix("action/").unwrap_or(verb);
    resolve_action(nav).map(|_| nav.to_string())
}

/// The empty-panel copy — honest about *why* nothing is listed. With no mesh Bus
/// directory the chat mirrors are unreadable (a gated read), which must not read
/// as a live-looking "no contacts" (§7).
const fn empty_copy(has_bus: bool) -> (&'static str, &'static str) {
    if has_bus {
        (
            "No contacts yet",
            "The mesh roster appears here once the chat worker publishes it — every enrolled \
             host is a contact, and its alerts arrive as its messages.",
        )
    } else {
        (
            "Chat unavailable",
            "No mesh Bus directory on this node, so the chat roster can't be read — joining the \
             mesh (the mde-bus spool) unblocks this surface.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_chat::{Message, NodeRole};

    #[test]
    fn dm_key_is_order_independent_and_matches_the_worker() {
        assert_eq!(dm_key("eagle", "nyc3"), dm_key("nyc3", "eagle"));
        assert_eq!(dm_key("a", "b"), "dm:a|b");
    }

    #[test]
    fn keys_for_a_peer_merge_dm_and_alert_but_self_is_alert_only() {
        let peer = keys_for_contact("eagle", "nyc3");
        assert!(peer.contains(&dm_key("eagle", "nyc3")));
        assert!(peer.contains(&alert_key("nyc3")));
        // Self carries only its local alerts (lock 17 — no notes-to-self DM).
        assert_eq!(keys_for_contact("eagle", "eagle"), vec![alert_key("eagle")]);
    }

    #[test]
    fn delivery_state_is_derived_honestly_from_recipient_presence() {
        // Available peer → the live relay reached them (Delivered).
        let online = Contact::new("nyc3", NodeRole::Headless).with_presence(Presence::Online);
        assert_eq!(Delivery::for_recipient(Some(&online)), Delivery::Delivered);
        // Unavailable peer → queued to backfill.
        let off = Contact::new("fra1", NodeRole::Headless).with_presence(Presence::Offline);
        assert_eq!(Delivery::for_recipient(Some(&off)), Delivery::Queued);
        // Unknown recipient → merely Sent.
        assert_eq!(Delivery::for_recipient(None), Delivery::Sent);
    }

    #[test]
    fn presence_and_severity_map_to_style_tokens_not_raw_hex() {
        assert_eq!(presence_color(Presence::Online), Style::OK);
        assert_eq!(presence_color(Presence::Dnd), Style::DANGER);
        assert_eq!(presence_color(Presence::Away), Style::WARN);
        assert_eq!(presence_color(Presence::Offline), Style::TEXT_DIM);
        assert_eq!(severity_color(Severity::Critical), Style::DANGER);
        assert_eq!(severity_color(Severity::Info), Style::ACCENT);
    }

    #[test]
    fn unread_watermarks_a_first_seen_contact_then_counts_new() {
        let mut state = ChatState::default();
        let mut conv = Conversation::new("nyc3");
        conv.insert(Message::text("nyc3", 10, "old"));
        conv.insert(Message::text("nyc3", 20, "history"));
        state.convos.insert("nyc3".into(), conv);
        // First sight: watermark at current length → nothing unread.
        state.seen.insert("nyc3".into(), 2);
        assert_eq!(state.unread("nyc3"), 0);
        // A new message arrives → one unread.
        let mut conv = state.convos.remove("nyc3").unwrap();
        conv.insert(Message::text("nyc3", 30, "new!"));
        state.convos.insert("nyc3".into(), conv);
        assert_eq!(state.unread("nyc3"), 1);
    }

    #[test]
    fn empty_copy_distinguishes_a_missing_bus_from_an_empty_roster() {
        let (title, _) = empty_copy(true);
        assert_eq!(title, "No contacts yet");
        let (title, subtitle) = empty_copy(false);
        assert_eq!(title, "Chat unavailable");
        assert!(subtitle.contains("Bus") && subtitle.contains("unblocks"));
    }

    /// Headless mount + tessellate: build a populated roster + conversation and
    /// render the whole surface (roster rail + open conversation pane) through the
    /// CPU tessellator — the same paint path the DRM runner drives, minus the GPU.
    /// Proves the surface actually draws over real model state (no demo data).
    #[test]
    fn surface_mounts_and_tessellates_over_real_state() {
        use mde_egui::egui::{pos2, vec2, Rect};

        let ctx = egui::Context::default();
        Style::install(&ctx);

        let mut state = ChatState::default();
        // A real roster: self + an online peer + an offline peer.
        let mut roster = Roster::new("eagle");
        roster.upsert(
            Contact::new("nyc3", NodeRole::Lighthouse)
                .with_presence(Presence::Online)
                .with_status("deploying"),
        );
        roster.upsert(Contact::new("fra1", NodeRole::Headless).with_presence(Presence::Offline));
        state.roster = Some(roster);
        // A conversation with my outgoing text + an inbound line.
        let mut conv = Conversation::new("nyc3");
        conv.insert(Message::text("eagle", 10, "ping"));
        conv.insert(Message::text("nyc3", 20, "pong"));
        state.convos.insert("nyc3".into(), conv);
        state.seen.insert("nyc3".into(), 1); // one unread
        state.selected = Some(Selection::Contact("nyc3".into()));

        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.push_id("shell-chat", |ui| state.show(ui));
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the chat surface produced no draw primitives"
        );
    }

    // ── NOTIFY-CHAT-4: message kinds + per-contact actions ────────────────────

    /// The alert action button is offered only for a verb the shell's ONE resolver
    /// accepts — a bare `action/shell/goto` or an unknown surface yields no button.
    #[test]
    fn alert_nav_verb_resolves_only_a_known_shell_target() {
        assert_eq!(
            alert_nav_verb(Some("action/shell/goto/system")).as_deref(),
            Some("shell/goto/system"),
        );
        // Already in the KIRON grammar (no `action/` prefix) still resolves.
        assert_eq!(
            alert_nav_verb(Some("shell/goto/files")).as_deref(),
            Some("shell/goto/files"),
        );
        // A bare verb without a surface, an unknown surface, and None → no button.
        assert!(alert_nav_verb(Some("action/shell/goto")).is_none());
        assert!(alert_nav_verb(Some("action/shell/goto/nope")).is_none());
        assert!(alert_nav_verb(None).is_none());
    }

    /// The Bus-writing action helpers are best-effort: with no Bus directory they
    /// are a silent no-op (the honest solo-host state), never a panic.
    #[test]
    fn action_helpers_are_silent_without_a_bus() {
        publish(None, "action/x", "{}");
        navigate_via_toast(None, "chat", "hi", "shell/goto/files");
        dial_peer(None, "nyc3");
        // send_file opens no Bus either — a ChatState with no bus_root.
        let state = ChatState {
            bus_root: None,
            ..ChatState::default()
        };
        state.send_file("nyc3", Path::new("/tmp/does-not-matter.txt"));
    }

    /// Every one of the six kinds renders *and* draws its action affordance: build a
    /// conversation carrying all six (emoji text, a clipboard clip, an alert card
    /// with a resolvable verb, a file offer, a Call + a Remote row) and tessellate
    /// the open pane. Proves each kind paints geometry over real model state — the
    /// same CPU paint path the DRM runner drives.
    #[test]
    fn every_message_kind_renders_its_action() {
        use mde_egui::egui::{pos2, vec2, Rect};

        let ctx = egui::Context::default();
        Style::install(&ctx);

        let mut state = ChatState::default();
        let mut roster = Roster::new("eagle");
        roster.upsert(Contact::new("nyc3", NodeRole::Workstation).with_presence(Presence::Online));
        state.roster = Some(roster);

        let mut conv = Conversation::new("nyc3");
        conv.insert(Message::text("eagle", 10, "hello 👋 🎉")); // emoji is just text
        conv.insert(Message::new(
            "nyc3",
            20,
            MessageKind::Clipboard {
                preview: "ssh nyc3".into(),
                full: "ssh root@nyc3.mesh".into(),
            },
        ));
        let mut fields = BTreeMap::new();
        fields.insert("summary".to_string(), "disk 92%".to_string());
        fields.insert("host".to_string(), "nyc3".to_string());
        conv.insert(Message::new(
            "nyc3",
            30,
            MessageKind::Alert {
                severity: Severity::Warning,
                flag: "storage".into(),
                fields,
                action_verb: Some("action/shell/goto/system".into()),
            },
        ));
        conv.insert(Message::new(
            "nyc3",
            40,
            MessageKind::File {
                name: "report.pdf".into(),
                size_bytes: 12_345,
                mime: None,
            },
        ));
        conv.insert(Message::new(
            "eagle",
            50,
            MessageKind::CallAction {
                target_host: "nyc3".into(),
            },
        ));
        conv.insert(Message::new(
            "eagle",
            60,
            MessageKind::RemoteAction {
                target_host: "nyc3".into(),
            },
        ));
        state.convos.insert("nyc3".into(), conv);
        state.seen.insert("nyc3".into(), 6);
        state.selected = Some(Selection::Contact("nyc3".into()));

        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1024.0, 720.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.push_id("shell-chat", |ui| state.show(ui));
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the mixed-kind conversation produced no draw primitives"
        );
    }

    // ── NOTIFY-CHAT-5 surfacing: rooms in the roster + the room pane ───────────

    /// A typed room name derives the same canonical id on every node — lowercase,
    /// non-alphanumerics collapsed to single dashes, trimmed.
    #[test]
    fn room_id_from_name_is_a_stable_canonical_slug() {
        assert_eq!(room_id_from_name("Build Farm!"), "build-farm");
        assert_eq!(room_id_from_name("  Ops   Room  "), "ops-room");
        assert_eq!(room_id_from_name("nyc3//fra1"), "nyc3-fra1");
        // Nothing alphanumeric → empty (the caller refuses to create it).
        assert_eq!(room_id_from_name("***"), "");
    }

    /// The room unread watermark is keyed by the `room:<id>` conversation key, so a
    /// room id can never collide with a contact hostname's unread count.
    #[test]
    fn room_unread_watermarks_then_counts_new_without_colliding_with_a_contact() {
        let mut state = ChatState::default();
        let mut room = Conversation::new("sys:all-fleet");
        room.insert(Message::text("nyc3", 10, "fleet up"));
        state.room_convos.insert("sys:all-fleet".into(), room);
        state.seen.insert(room_key("sys:all-fleet"), 1);
        assert_eq!(state.room_unread("sys:all-fleet"), 0);
        // A same-named contact watermark is independent.
        let mut conv = Conversation::new("sys:all-fleet");
        conv.insert(Message::text("x", 5, "dm"));
        conv.insert(Message::text("x", 6, "dm2"));
        state.convos.insert("sys:all-fleet".into(), conv);
        state.seen.insert("sys:all-fleet".into(), 0);
        assert_eq!(state.unread("sys:all-fleet"), 2, "contact key is separate");
        // A new room message → one unread.
        let mut room = state.room_convos.remove("sys:all-fleet").unwrap();
        room.insert(Message::text("fra1", 20, "new"));
        state.room_convos.insert("sys:all-fleet".into(), room);
        assert_eq!(state.room_unread("sys:all-fleet"), 1);
    }

    /// The chrome unread indicator's tally (NOTIFY-CHAT-6) sums every contact AND
    /// room unread, over the same watermarks the roster badges use — so the chrome
    /// badge can't diverge from the surface. A quiet mesh is an honest zero.
    #[test]
    fn total_unread_sums_contacts_and_rooms() {
        let mut state = ChatState::default();
        // A quiet host: nothing unread.
        assert_eq!(state.total_unread(), 0);

        // A contact with 2 new messages since the watermark.
        let mut dm = Conversation::new("nyc3");
        dm.insert(Message::text("nyc3", 10, "hi"));
        dm.insert(Message::text("nyc3", 20, "still there?"));
        state.convos.insert("nyc3".into(), dm);
        state.seen.insert("nyc3".into(), 0);

        // A room with 1 new message; only rooms in the registry are counted.
        let mut room = Conversation::new("ops");
        room.insert(Message::text("fra1", 30, "deploy done"));
        state.room_convos.insert("ops".into(), room);
        state.seen.insert(room_key("ops"), 0);
        state.rooms = vec![RoomDescriptor {
            id: "ops".into(),
            name: "Ops".into(),
            kind: RoomKind::AdHoc,
            creator: "eagle".into(),
            members: vec!["eagle".into()],
        }];

        assert_eq!(state.total_unread(), 3, "2 contact + 1 room unread");
    }

    /// Headless mount + tessellate with a selected room: the roster shows the Rooms
    /// group and the room pane renders the shared log over real model state — proving
    /// rooms surface and open without a live display (no demo data).
    #[test]
    fn room_surfaces_in_the_roster_and_its_pane_tessellates() {
        use mde_egui::egui::{pos2, vec2, Rect};

        let ctx = egui::Context::default();
        Style::install(&ctx);

        let mut state = ChatState::default();
        let mut roster = Roster::new("eagle");
        roster.upsert(Contact::new("nyc3", NodeRole::Headless).with_presence(Presence::Online));
        state.roster = Some(roster);

        // A system room + an ad-hoc room I created (so Dissolve is reachable).
        state.rooms = vec![
            RoomDescriptor {
                id: "sys:all-fleet".into(),
                name: "All Fleet".into(),
                kind: RoomKind::System,
                creator: String::new(),
                members: vec!["eagle".into(), "nyc3".into()],
            },
            RoomDescriptor {
                id: "ops".into(),
                name: "Ops".into(),
                kind: RoomKind::AdHoc,
                creator: "eagle".into(),
                members: vec!["eagle".into()],
            },
        ];
        let mut log = Conversation::new("sys:all-fleet");
        log.insert(Message::text("nyc3", 10, "fleet chatter"));
        state.room_convos.insert("sys:all-fleet".into(), log);
        state.seen.insert(room_key("sys:all-fleet"), 0); // one unread
        state.selected = Some(Selection::Room("sys:all-fleet".into()));

        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1024.0, 720.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.push_id("shell-chat", |ui| state.show(ui));
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the room pane produced no draw primitives"
        );
        // Opening the room watermarked it read.
        assert_eq!(state.room_unread("sys:all-fleet"), 0);
    }

    /// The room lifecycle + send helpers are best-effort: with no Bus directory they
    /// are silent no-ops (the honest solo-host state), never a panic.
    #[test]
    fn room_action_helpers_are_silent_without_a_bus() {
        let state = ChatState {
            bus_root: None,
            ..ChatState::default()
        };
        state.send_room("sys:all-fleet", "hi");
        state.create_room("Build Farm");
        state.create_room("***"); // empty slug → refused, still no panic
        state.room_action("join", "ops", None);
        state.room_action("create", "ops", Some("Ops"));
    }

    // ── timestamps + presence/status/mute (the four closed gaps) ───────────────

    /// The message row's timestamp: a compact HH:MM (UTC) with a full-date hover,
    /// derived by the pure formatters — and the row paints over a real timestamped
    /// message. A non-positive time renders blank (never a fabricated "00:00").
    #[test]
    fn message_row_renders_a_timestamp() {
        use mde_egui::egui::{pos2, vec2, Rect};

        // A known epoch: 1_700_000_000_000 ms = 2023-11-14 22:13:20 UTC.
        assert_eq!(fmt_hh_mm(1_700_000_000_000), "22:13");
        assert_eq!(fmt_full_datetime(1_700_000_000_000), "2023-11-14 22:13 UTC");
        assert_eq!(fmt_date(1_700_000_000_000), "2023-11-14");
        // A civil leap-year day still resolves (2024-02-29).
        assert_eq!(fmt_date(1_709_200_000_000), "2024-02-29");
        // A non-positive timestamp is an honest blank, not a faked clock.
        assert!(fmt_hh_mm(0).is_empty());
        assert!(fmt_hh_mm(-5).is_empty());

        let ctx = egui::Context::default();
        Style::install(&ctx);
        let msg = Message::text("nyc3", 1_700_000_000_000, "hello");
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 200.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                message_row(ui, &msg, "eagle", None, None);
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "a timestamped message row produced no draw primitives"
        );
    }

    /// The self-presence picker maps live presence → the picker selection, and
    /// each option → the wire tag the worker's `PresenceSet` decodes ("available"
    /// clears to auto; the four ICQ manual states set their override).
    #[test]
    fn presence_choice_maps_presence_and_wire_tags() {
        // Auto/derived states map to the closest picker option so the live
        // presence always shows a selection.
        assert_eq!(
            PresenceChoice::from_presence(Presence::Online),
            PresenceChoice::Available
        );
        assert_eq!(
            PresenceChoice::from_presence(Presence::ManualAway),
            PresenceChoice::Away
        );
        assert_eq!(
            PresenceChoice::from_presence(Presence::Dnd),
            PresenceChoice::Dnd
        );
        assert_eq!(
            PresenceChoice::from_presence(Presence::FreeForChat),
            PresenceChoice::FreeForChat
        );
        // Wire tags match the worker's `PresenceSet` snake_case names.
        assert_eq!(PresenceChoice::Available.wire(), "available");
        assert_eq!(PresenceChoice::Dnd.wire(), "dnd");
        assert_eq!(PresenceChoice::Away.wire(), "away");
        assert_eq!(PresenceChoice::FreeForChat.wire(), "free_for_chat");
        assert_eq!(PresenceChoice::Invisible.wire(), "invisible");
    }

    /// The mute toggles read the worker-published `state/chat/notify` mirror — the
    /// round-trip that makes them real, not local-only. No mirror ⇒ nothing muted.
    #[test]
    fn mute_state_reads_the_published_notify_mirror() {
        let mut state = ChatState::default();
        assert!(!state.is_contact_muted("nyc3"));
        assert!(!state.is_room_muted("ops"));
        let mut prefs = NotifyPrefs::new();
        prefs.mute_contact("nyc3");
        prefs.mute_room("ops");
        state.notify = Some(prefs);
        assert!(state.is_contact_muted("nyc3"));
        assert!(state.is_room_muted("ops"));
        assert!(
            !state.is_contact_muted("fra1"),
            "an unmuted contact still rings"
        );
    }

    // ── MENU-2: the View feed filters + Unread Only ────────────────────────────

    /// The three feed-filter bands classify every message kind: alerts, clips,
    /// and everything human-authored (text / file / call / remote) as messages.
    #[test]
    fn feed_filters_classify_every_message_kind() {
        let mut state = ChatState::default();
        let alert = MessageKind::Alert {
            severity: Severity::Info,
            flag: "x".into(),
            fields: BTreeMap::new(),
            action_verb: None,
        };
        let clip = MessageKind::Clipboard {
            preview: "p".into(),
            full: "f".into(),
        };
        let text = MessageKind::Text("hi".into());
        let file = MessageKind::File {
            name: "a.txt".into(),
            size_bytes: 1,
            mime: None,
        };
        // Defaults: everything shows.
        for kind in [&alert, &clip, &text, &file] {
            assert!(state.feed_shows(kind), "all bands default on");
        }
        state.show_alerts = false;
        assert!(!state.feed_shows(&alert), "Alerts off hides an alert");
        assert!(state.feed_shows(&clip), "…but not a clip");
        state.show_clips = false;
        assert!(!state.feed_shows(&clip));
        state.show_messages = false;
        assert!(!state.feed_shows(&text), "Messages off hides text");
        assert!(!state.feed_shows(&file), "…and file offers");
    }

    /// Unread Only prunes the roster to conversations with unread — but self
    /// stays pinned and the OPEN conversation stays visible (opening a pane
    /// watermarks it read, so it must not vanish mid-read).
    #[test]
    fn unread_only_prunes_but_keeps_self_and_the_open_pane() {
        let mut state = ChatState::default();
        let mut roster = Roster::new("eagle");
        roster.upsert(Contact::new("nyc3", NodeRole::Headless).with_presence(Presence::Online));
        roster.upsert(Contact::new("fra1", NodeRole::Headless).with_presence(Presence::Online));
        let me = Contact::new("eagle", NodeRole::Workstation);
        let nyc3 = Contact::new("nyc3", NodeRole::Headless);
        let fra1 = Contact::new("fra1", NodeRole::Headless);

        // Filters off: everything shows.
        assert!(state.roster_shows(&roster, &nyc3));

        state.unread_only = true;
        // nyc3 carries one unread → visible; fra1 is read → pruned.
        let mut conv = Conversation::new("nyc3");
        conv.insert(Message::text("nyc3", 10, "hi"));
        state.convos.insert("nyc3".into(), conv);
        state.seen.insert("nyc3".into(), 0);
        assert!(state.roster_shows(&roster, &nyc3), "unread stays");
        assert!(!state.roster_shows(&roster, &fra1), "read prunes");
        assert!(state.roster_shows(&roster, &me), "self stays pinned");
        // The open (therefore read) conversation stays visible.
        state.selected = Some(Selection::Contact("fra1".into()));
        assert!(state.roster_shows(&roster, &fra1), "the open pane stays");
        // Rooms follow the same rule.
        assert!(!state.room_shows("ops"), "a read room prunes");
        state.selected = Some(Selection::Room("ops".into()));
        assert!(state.room_shows("ops"), "the open room stays");
    }

    /// The presence / status / mute action helpers are best-effort: with no Bus
    /// directory they are silent no-ops (the honest solo-host state), never a panic.
    #[test]
    fn presence_status_and_mute_helpers_are_silent_without_a_bus() {
        let state = ChatState {
            bus_root: None,
            ..ChatState::default()
        };
        state.set_presence(PresenceChoice::Dnd);
        state.set_presence(PresenceChoice::Available); // clear to auto
        state.set_status(Some("brb"));
        state.set_status(Some("")); // empty clears at the worker
        state.set_status(None);
        publish_mute(None, "contact", "nyc3", true);
        publish_mute(None, "room", "ops", false);
    }
}
