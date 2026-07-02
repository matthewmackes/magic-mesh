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
    path::PathBuf,
    time::{Duration, Instant},
};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_chat::{Contact, Conversation, Message, MessageKind, Presence, Roster, Severity};
use mde_egui::egui::{self, Align, Color32, Layout, RichText, ScrollArea};
use mde_egui::Style;

/// Poll cadence — matches the chat worker's own 2s tick so the roster and open
/// conversation stay live without a cold-start wait.
const REFRESH: Duration = Duration::from_secs(2);

/// The presence roster mirror the worker publishes (latest-wins).
const ROSTER_TOPIC: &str = "state/chat/roster";
/// Prefix for the per-conversation read-model the worker republishes each change.
const CONVERSATION_PREFIX: &str = "state/chat/conversation/";
/// The UI's outbound verb — a chat message to send.
const ACTION_CHAT_SEND: &str = "action/chat/send";

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
                        message_row(ui, msg, self_host, recipient);
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
        ui.add_space(Style::SP_XS);

        let text = self.draft.trim().to_string();
        if send && !text.is_empty() {
            self.send(host, &text);
            self.draft.clear();
        }
    }

    /// Publish `action/chat/send` `{scope:"peer", to, text}` to the local Bus —
    /// the worker signs, persists, and relays it (best-effort; a missing Bus is a
    /// silent no-op, the honest solo-host state).
    fn send(&self, to: &str, text: &str) {
        let Some(root) = self.bus_root.clone() else {
            return;
        };
        let Ok(persist) = Persist::open(root) else {
            return;
        };
        let body = serde_json::json!({ "scope": "peer", "to": to, "text": text }).to_string();
        let _ = persist.write(ACTION_CHAT_SEND, Priority::Default, None, Some(&body));
    }
}

/// Read the newest (latest-wins) message on `topic` and deserialize its body.
fn latest_json<T: serde::de::DeserializeOwned>(persist: &Persist, topic: &str) -> Option<T> {
    let msgs = persist.list_since(topic, None).ok()?;
    let body = msgs.last()?.body.as_deref()?;
    serde_json::from_str::<T>(body).ok()
}

/// Render one message row (human text, a clipboard copy, a folded alert card, or
/// a file/call/remote hand-off). Rich kind interaction (re-copy, launch Call /
/// Remote) is NOTIFY-CHAT-4; here every kind renders honestly read-only, and my
/// own outgoing text carries its delivery checkmark (lock 19).
fn message_row(ui: &mut egui::Ui, msg: &Message, self_host: &str, recipient: Option<&Contact>) {
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
        message_body(ui, msg);
    });
}

/// The body of a message row, by kind.
fn message_body(ui: &mut egui::Ui, msg: &Message) {
    match &msg.kind {
        MessageKind::Text(text) => {
            ui.label(RichText::new(text).color(Style::TEXT).size(Style::BODY));
        }
        MessageKind::Clipboard { preview, .. } => {
            ui.horizontal(|ui| {
                mde_egui::muted_note(ui, "clipboard");
                ui.label(
                    RichText::new(preview)
                        .color(Style::TEXT)
                        .size(Style::BODY)
                        .monospace(),
                );
            });
        }
        MessageKind::Alert {
            severity,
            flag,
            fields,
            ..
        } => {
            let title = fields
                .get("summary")
                .or_else(|| fields.get("title"))
                .map_or(flag.as_str(), String::as_str);
            ui.colored_label(
                severity_color(*severity),
                RichText::new(title).size(Style::BODY).strong(),
            );
            mde_egui::muted_note(ui, format!("alert · {flag}"));
        }
        MessageKind::File {
            name, size_bytes, ..
        } => {
            ui.horizontal(|ui| {
                mde_egui::muted_note(ui, "file");
                ui.label(RichText::new(name).color(Style::TEXT).size(Style::BODY));
                mde_egui::muted_note(ui, format!("{size_bytes} bytes"));
            });
        }
        MessageKind::CallAction { target_host } => {
            mde_egui::field(ui, "call", target_host, Style::ACCENT);
        }
        MessageKind::RemoteAction { target_host } => {
            mde_egui::field(ui, "remote control", target_host, Style::ACCENT);
        }
    }
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
}
