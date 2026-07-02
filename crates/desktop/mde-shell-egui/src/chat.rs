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
use mde_chat::{Contact, Conversation, Message, MessageKind, Presence, Roster, Severity};
use mde_egui::egui::{self, Align, Color32, Layout, RichText, ScrollArea};
use mde_egui::Style;

use crate::discovery::request_host_desktop;
use crate::toast_bridge::{resolve_action, TOAST_TOPIC};

/// Poll cadence — matches the chat worker's own 2s tick so the roster and open
/// conversation stay live without a cold-start wait.
const REFRESH: Duration = Duration::from_secs(2);

/// The presence roster mirror the worker publishes (latest-wins).
const ROSTER_TOPIC: &str = "state/chat/roster";
/// Prefix for the per-conversation read-model the worker republishes each change.
const CONVERSATION_PREFIX: &str = "state/chat/conversation/";
/// The UI's outbound verb — a chat message to send.
const ACTION_CHAT_SEND: &str = "action/chat/send";
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

/// The Chat surface state: the last roster + each contact's merged conversation
/// (rebuilt from the latest-wins Bus mirrors each poll), the selected contact,
/// the composer draft, and the per-contact read watermark for unread counts.
pub(crate) struct ChatState {
    bus_root: Option<PathBuf>,
    /// The latest roster the worker published, if any.
    roster: Option<Roster>,
    /// host → the contact's merged (DM ∪ alert) ring, rebuilt each refresh.
    convos: BTreeMap<String, Conversation>,
    /// The selected contact host (its conversation pane is open).
    selected: Option<String>,
    /// The composer buffer for the open conversation.
    draft: String,
    /// The Send-To composer's file path buffer (typed or drag-dropped) — an empty
    /// string hides the attach row's Send button (NOTIFY-CHAT-4, file kind).
    attach_path: String,
    /// host → message count when last viewed; unread = current − watermark. A
    /// host first seen is watermarked at its current length so pre-existing
    /// backfilled history isn't flagged unread (unread = new since you looked).
    seen: BTreeMap<String, usize>,
    last_poll: Option<Instant>,
}

impl Default for ChatState {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            roster: None,
            convos: BTreeMap::new(),
            selected: None,
            draft: String::new(),
            attach_path: String::new(),
            seen: BTreeMap::new(),
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
    }

    /// Unread count for `host` — new messages since the read watermark, clamped so
    /// a ring eviction can't underflow.
    fn unread(&self, host: &str) -> usize {
        let now = self.convos.get(host).map_or(0, Conversation::len);
        let seen = self.seen.get(host).copied().unwrap_or(now);
        now.saturating_sub(seen)
    }

    /// Render the ICQ surface: the roster rail on the left, the selected
    /// contact's conversation pane filling the rest.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
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
            Some(host) if roster.get(&host).is_some() => {
                // Opening the pane marks it read (watermark → current length).
                let now = self.convos.get(&host).map_or(0, Conversation::len);
                self.seen.insert(host.clone(), now);
                self.conversation_pane(ui, &roster, &host);
            }
            _ => {
                crate::session::empty_state(
                    ui,
                    "Pick a contact",
                    "Select a host on the left to open its conversation — its messages and its \
                     alerts share one timeline.",
                );
            }
        }
    }

    /// The roster rail — the ICQ Online / Offline groups (lock 4).
    fn roster_rail(&mut self, ui: &mut egui::Ui, roster: &Roster) {
        // Self line, pinned at the top with its own presence (lock 17).
        let me = roster.self_contact();
        ui.horizontal(|ui| {
            mde_egui::status_dot(ui, presence_color(me.presence));
            ui.label(
                RichText::new(me.display_name())
                    .color(Style::TEXT)
                    .size(Style::BODY)
                    .strong(),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                mde_egui::muted_note(ui, me.presence.label());
            });
        });
        if let Some(status) = &me.status_message {
            mde_egui::muted_note(ui, status);
        }
        ui.separator();

        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.roster_group(ui, roster, "Online", &roster.online());
                ui.add_space(Style::SP_S);
                self.roster_group(ui, roster, "Offline", &roster.offline());
            });
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
        let selected = self.selected.as_deref() == Some(contact.host.as_str());
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
            self.selected = Some(contact.host.clone());
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
                contact_actions(ui, bus_root.as_deref(), host);
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
                    for msg in conv.messages() {
                        message_row(ui, msg, self_host, recipient, bus_root.as_deref());
                        ui.add_space(Style::SP_XS);
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

/// The per-contact action bar under the conversation header: **Call** (SIP) and
/// **Remote Control** (VDI), the two per-contact hand-offs of lock 15. Both fire
/// their owning crate's Bus verb; the live SIP register+call and live VDI connect
/// are integration-gated (a running agent / broker / guest), so this is the honest
/// reachable near half — the launch, never a faked session.
fn contact_actions(ui: &mut egui::Ui, bus_root: Option<&Path>, host: &str) {
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
    });
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
        state.selected = Some("nyc3".into());

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
        state.selected = Some("nyc3".into());

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
}
