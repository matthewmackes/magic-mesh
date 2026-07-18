//! The Chat surface's **leaf render / format / action helpers** — the pure
//! drawing, timestamp-formatting, and Bus-action functions the conversation +
//! Notifications panes call, split out of the god-module (pure relocation, no
//! behaviour change; NOTIFY-CHAT design `docs/design/mesh-chat-icq.md`).
//!
//! Everything here is a leaf the parent [`ChatState`] render loop drives: the
//! day-separated timeline + one message row/body (each kind renders **and**
//! acts, NOTIFY-CHAT-4), the aggregate Notifications lane row + source flags,
//! the per-contact action bar + Mute toggle, the `civil_from_days`/`HH:MM`
//! calendar math (the shell's ONE calendar, §6), the `action/chat/mute` +
//! alert-action publishers, and the empty-panel copy.
//!
//! `use super::*` pulls in the parent's private wire types (`NotificationItem`,
//! `Delivery`, `ChatState`), the re-exported `mde_chat` / `egui` names, and the
//! `publish` / `navigate_via_toast` / `resolve_action` seams; as a child module
//! it reads the parent's private items directly, so the items the parent (and
//! the tests) call back into are `pub(super)`. `civil_from_days` stays `pub`
//! and the alert-action / DND button ids stay `pub(crate)` (re-exported by the
//! parent) because the shell chrome + integration tests reach them by name.

use super::*;
use crate::dock::icon_texture;
use mde_theme::brand::icons::IconId;

pub(super) const CHAT_UNMUTE_ICON: IconId = IconId::Notifications;
pub(super) const CHAT_MUTE_ICON: IconId = IconId::NotificationsMuted;

/// Render a conversation's messages with a muted **day separator** whenever the
/// civil (UTC) date changes — the authentic chat idiom — each row carrying its own
/// HH:MM timestamp ([`message_row`]). Shared by the contact + room panes so both
/// read the same way. Takes the panes' already-feed-filtered slice (MENU-2), so
/// the day separators track only what actually renders.
pub(super) fn render_timeline(
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
pub(super) fn day_separator(ui: &mut egui::Ui, date: &str) {
    ui.add_space(Style::SP_XS);
    ui.vertical_centered(|ui| {
        mde_egui::muted_note(ui, date);
    });
    ui.add_space(Style::SP_XS);
}

/// Compact wall-clock `HH:MM` (UTC) for a message's injected send time. Pure — no
/// external time crate (there is none in this DRM seat's deps); UTC so it never
/// claims a local zone it can't know. A non-positive timestamp yields "".
pub(super) fn fmt_hh_mm(ts_unix_ms: i64) -> String {
    if ts_unix_ms <= 0 {
        return String::new();
    }
    let tod = (ts_unix_ms / 1000).rem_euclid(86_400);
    format!("{:02}:{:02}", tod / 3600, (tod % 3600) / 60)
}

/// A full `YYYY-MM-DD HH:MM UTC` stamp — the message-row hover.
pub(super) fn fmt_full_datetime(ts_unix_ms: i64) -> String {
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
pub(super) fn fmt_date(ts_unix_ms: i64) -> String {
    if ts_unix_ms <= 0 {
        return "unknown date".to_string();
    }
    let (year, month, day) = civil_from_days((ts_unix_ms / 1000).div_euclid(86_400));
    format!("{year:04}-{month:02}-{day:02}")
}

/// Civil `(year, month, day)` from a day-count since the Unix epoch
/// (1970-01-01), proleptic Gregorian. Howard Hinnant's `civil_from_days` — the
/// one piece of calendar math the DRM seat needs with no time crate on the deps.
/// Crate-visible: the shell chrome folds date lines through this same fn, so the
/// shell has ONE calendar (§6).
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

/// The Chat card shadow — the surface-side conversion of the shared
/// [`Elevation::Raised`](mde_egui::style::Elevation::Raised) depth token into an
/// [`egui::Shadow`] (the token module stays free of egui's shadow type). Reads the
/// token's offset/blur/spread/umbra, casting the logical-px floats onto epaint's
/// small integer fields; mints **no** colour of its own (the umbra comes straight
/// from the token), so a message / notification card reads as genuinely lifted off
/// the timeline while the look still comes only from `mde_egui` (§4).
pub(super) fn card_shadow() -> egui::Shadow {
    let token = mde_egui::style::Elevation::Raised.shadow();
    egui::Shadow {
        offset: [token.offset[0] as i8, token.offset[1] as i8],
        blur: token.blur as u8,
        spread: token.spread as u8,
        color: token.umbra,
    }
}

/// Render one message row (human text, a clipboard copy, a folded alert card, or
/// a file/call/remote hand-off). Each kind renders **and acts** (NOTIFY-CHAT-4 —
/// re-copy, run an alert verb, download a file, re-launch Call / Remote); my own
/// outgoing text carries its delivery checkmark (lock 19).
pub(super) fn message_row(
    ui: &mut egui::Ui,
    msg: &Message,
    self_host: &str,
    recipient: Option<&Contact>,
    bus_root: Option<&Path>,
) {
    let mine = msg.sender == self_host;
    // The stock group frame, lifted by the shared Raised depth token — same
    // fill/stroke/padding (no layout change), the card just casts the soft shadow.
    egui::Frame::group(ui.style())
        .shadow(card_shadow())
        .show(ui, |ui| {
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
                        ui.colored_label(
                            delivery.color(),
                            RichText::new(delivery.label()).size(Style::SMALL),
                        )
                        .on_hover_text(delivery.hover_text());
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
pub(super) fn message_body(ui: &mut egui::Ui, msg: &Message, bus_root: Option<&Path>) {
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
            actions,
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
                        if ui.button(CHAT_ALERT_GO_TO_LABEL).clicked() {
                            navigate_via_toast(bus_root, "chat", title, &verb);
                        }
                    });
                }
            });
            alert_action_buttons(ui, bus_root, msg, actions);
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

/// Render one item in the aggregate Notifications lane. The lane is newest-first
/// and alert-only; each row shows the originating host contact before the folded
/// alert body so attribution is never lost.
pub(super) fn notification_row(
    ui: &mut egui::Ui,
    item: NotificationItem<'_>,
    bus_root: Option<&Path>,
) {
    let MessageKind::Alert {
        severity,
        flag,
        fields,
        action_verb,
        actions,
    } = &item.msg.kind
    else {
        return;
    };
    let title = fields
        .get("summary")
        .or_else(|| fields.get("title"))
        .map_or(flag.as_str(), String::as_str);
    // Same Raised lift as a message card — the stock group frame + the shared
    // depth token's soft shadow, nothing else changed.
    egui::Frame::group(ui.style())
        .shadow(card_shadow())
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(item.host)
                        .color(Style::TEXT)
                        .size(Style::SMALL)
                        .strong(),
                );
                ui.colored_label(
                    severity_color(*severity),
                    RichText::new(flag).size(Style::SMALL).strong(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let hhmm = fmt_hh_mm(item.msg.ts_unix_ms);
                    if !hhmm.is_empty() {
                        ui.label(
                            RichText::new(hhmm)
                                .color(Style::TEXT_DIM)
                                .size(Style::SMALL),
                        )
                        .on_hover_text(fmt_full_datetime(item.msg.ts_unix_ms));
                    }
                });
            });
            ui.colored_label(
                severity_color(*severity),
                RichText::new(title).size(Style::BODY).strong(),
            );
            for (k, v) in fields {
                if k == "summary" || k == "title" {
                    continue;
                }
                mde_egui::field(ui, k, v, Style::TEXT_DIM);
            }
            if let Some(verb) = alert_nav_verb(action_verb.as_deref()) {
                ui.horizontal(|ui| {
                    mde_egui::muted_note(ui, "actions");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button(CHAT_ALERT_GO_TO_LABEL).clicked() {
                            navigate_via_toast(bus_root, "chat", title, &verb);
                        }
                    });
                });
            }
            alert_action_buttons(ui, bus_root, item.msg, actions);
        });
}

pub(super) fn alert_action_buttons(
    ui: &mut egui::Ui,
    bus_root: Option<&Path>,
    msg: &Message,
    actions: &[AlertAction],
) {
    if actions.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        mde_egui::muted_note(ui, "actions");
        for action in actions {
            let mut label = action.label.clone();
            let armed = action.kind == AlertActionKind::Destructive;
            if armed {
                label = format!("Arm {label}");
            }
            let id = alert_action_button_id(msg.id.as_str(), action.id.as_str());
            let resp = ui.push_id(id, |ui| ui.button(label)).inner;
            let _stable = ui.interact(resp.rect, id, egui::Sense::hover());
            if resp.clicked() {
                publish_alert_action(bus_root, msg, action, armed);
            }
        }
    });
}

/// Stable ID for typed alert action buttons so integration tests can prove the
/// action surface mounted without depending on button text layout.
pub(crate) fn alert_action_button_id(message_id: &str, action_id: &str) -> egui::Id {
    egui::Id::new(("chat-alert-action", message_id, action_id))
}

/// Stable ID for the aggregate Notifications lane DND toggle.
pub(crate) fn notification_dnd_toggle_id() -> egui::Id {
    egui::Id::new("chat-notifications-dnd-toggle")
}

/// Unique folded-alert source flags present in the aggregate Notifications lane.
pub(super) fn notification_sources(items: &[NotificationItem<'_>]) -> Vec<String> {
    let mut sources = BTreeSet::new();
    for item in items {
        if let MessageKind::Alert { flag, .. } = &item.msg.kind {
            sources.insert(flag.clone());
        }
    }
    sources.into_iter().collect()
}

/// The per-contact action bar under the conversation header: **Call** (SIP),
/// **Remote Control** (VDI) — the two per-contact hand-offs of lock 15 — and a
/// **Mute** toggle (NOTIFY-CHAT-5) that silences this contact's messages + alerts.
/// Call/Remote fire their owning crate's Bus verb (the live SIP register+call and
/// VDI connect are integration-gated — the honest reachable launch, never a faked
/// session); Mute posts `action/chat/mute`, which the worker drains into its
/// `NotifyPrefs` and republishes so `muted` reflects the true persisted policy.
pub(super) fn contact_actions(
    ui: &mut egui::Ui,
    bus_root: Option<&Path>,
    host: &str,
    muted: bool,
    err: &mut Option<String>,
) {
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
        mute_button(ui, bus_root, "contact", host, muted, err);
    });
}

/// A **Mute / Unmute** toggle for a contact or room. `muted` is the current
/// (worker-published) state; clicking posts `action/chat/mute` with the flipped
/// value so the worker updates + republishes the policy (the round-trip that
/// makes the toggle real, not a local-only switch — §7).
pub(super) fn mute_button(
    ui: &mut egui::Ui,
    bus_root: Option<&Path>,
    target: &str,
    id: &str,
    muted: bool,
    err: &mut Option<String>,
) {
    let (icon, label, hint) = if muted {
        (
            CHAT_UNMUTE_ICON,
            "Unmute",
            format!("Unmute {id} — let it ring again"),
        )
    } else {
        (
            CHAT_MUTE_ICON,
            "Mute",
            format!("Mute {id} — silence its messages + alerts"),
        )
    };
    if yamis_icon_button(ui, icon, label)
        .on_hover_text(hint)
        .clicked()
    {
        // shell-ux-11: surface a failed mute rather than silently dropping it.
        if let Err(e) = publish_mute(bus_root, target, id, !muted) {
            *err = Some(e);
        }
    }
}

fn yamis_icon_button(ui: &mut egui::Ui, icon: IconId, label: &str) -> egui::Response {
    let Some(tex) = icon_texture(ui.ctx(), icon, Style::SP_M, Style::TEXT) else {
        return ui.button(label);
    };
    let image = egui::Image::new(egui::load::SizedTexture::new(
        tex.id(),
        egui::vec2(Style::SP_M, Style::SP_M),
    ));
    ui.add(egui::Button::image_and_text(image, label))
}

/// Publish `action/chat/mute` `{target, id, muted}` to the local Bus — the worker
/// drains it into this seat's `NotifyPrefs`. Returns the publish result so the mute
/// controls surface a failure into `last_error` (shell-ux-11) instead of silently
/// dropping it.
pub(super) fn publish_mute(
    bus_root: Option<&Path>,
    target: &str,
    id: &str,
    muted: bool,
) -> Result<(), String> {
    let body = serde_json::json!({ "target": target, "id": id, "muted": muted }).to_string();
    publish(bus_root, ACTION_CHAT_MUTE, &body)
}

pub(super) fn publish_alert_action(
    bus_root: Option<&Path>,
    msg: &Message,
    action: &AlertAction,
    armed: bool,
) {
    let body = serde_json::json!({
        "message_id": msg.id.as_str(),
        "sender": msg.sender,
        "action_id": action.id,
        "label": action.label,
        "kind": action.kind,
        "verb": action.verb,
        "armed": armed,
    })
    .to_string();
    // Best-effort, but no longer a buried silent swallow (shell-ux-11): the action
    // button stays on screen after a failed publish, so nothing is destroyed and the
    // operator can click again — unlike a composed draft, which is why alert actions
    // don't thread through the immediate-mode message tree to `last_error`.
    let _ = publish(bus_root, ACTION_CHAT_ALERT_ACTION, &body);
}

pub(super) fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(super) fn local_hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "local".to_string())
}

/// Fire the voice worker's dial verb for `host` (lock 15 — Call hands off to
/// `mde-voice`). The publish is reachable now; a running SIP agent draining it +
/// the live register/call is integration-gated.
pub(super) fn dial_peer(bus_root: Option<&Path>, host: &str) {
    let body = serde_json::json!({ "peer": host }).to_string();
    // Best-effort: a retriable dial verb, no composed content to lose.
    let _ = publish(bus_root, ACTION_VOICE_DIAL, &body);
}

/// Translate a folded alert's `action_verb` (`action/shell/goto/<surface>`, lock
/// 15) into the KIRON toast/nav grammar (`shell/goto/<surface>`), returning it only
/// when it resolves to a real shell target — the shell's ONE resolver
/// ([`resolve_action`]) is the gate, so a bare/unknown verb offers no button.
pub(super) fn alert_nav_verb(action_verb: Option<&str>) -> Option<String> {
    let verb = action_verb?;
    let nav = verb.strip_prefix("action/").unwrap_or(verb);
    resolve_action(nav).map(|_| nav.to_string())
}

/// The empty-panel copy — honest about *why* nothing is listed. With no mesh Bus
/// directory the chat mirrors are unreadable (a gated read), which must not read
/// as a live-looking "no contacts" (§7).
pub(super) const fn empty_copy(has_bus: bool) -> (&'static str, &'static str) {
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
