//! Messages mode — a Markdown conversation timeline
//! ([`ConversationTimeline`](mde_collab_types::ConversationTimeline)) with
//! anchored threads ([`ThreadTimeline`](mde_collab_types::ThreadTimeline)), a
//! composer whose <kbd>Enter</kbd> emits
//! [`SendMessage`](mde_collab_types::CollabCommand::SendMessage) with a
//! locally-persisted draft, honest delivery state, and an edit/delete affordance
//! that reflects the core's five-minute author window (spec §3).

use mde_egui::egui;
use mde_egui::Style;

use mde_collab_types::{CollabCommand, DeliveryState, MessageBody, MessageView, SpaceId, ThreadId};

use crate::icons::CommsHoverExt;
use crate::{amend_affordance, icons, relative_age, AmendAffordance, CommunicationsSurface};

impl CommunicationsSurface {
    /// Render Messages mode for the selected space: the conversation column,
    /// plus an anchored thread column when a thread is open.
    pub(crate) fn messages_body(
        &mut self,
        ui: &mut egui::Ui,
        data: &dyn crate::CollabData,
        sink: &mut crate::CommandSink,
    ) {
        let Some(space) = self.selected_space() else {
            ui.label(
                egui::RichText::new("Select a space to open its messages.").color(Style::TEXT_DIM),
            );
            return;
        };
        match self.open_thread {
            Some(thread) => {
                ui.columns(2, |cols| {
                    self.conversation_column(&mut cols[0], data, sink, space);
                    self.thread_column(&mut cols[1], data, sink, space, thread);
                });
            }
            None => self.conversation_column(ui, data, sink, space),
        }
    }

    /// The main conversation column: the scrolling timeline over a reserved
    /// composer pinned beneath it.
    fn conversation_column(
        &mut self,
        ui: &mut egui::Ui,
        data: &dyn crate::CollabData,
        sink: &mut crate::CommandSink,
        space: SpaceId,
    ) {
        let composer_h = Style::SP_XL + Style::SP_M;
        egui::ScrollArea::vertical()
            .id_salt("collab-timeline")
            .auto_shrink([false, false])
            .max_height((ui.available_height() - composer_h).max(Style::SP_XL))
            .show(ui, |ui| match data.conversation(space) {
                Some(conv) if !conv.messages.is_empty() => {
                    // A newly-appearing row fades up on the shared staggered list
                    // entrance (lock #4) — only genuinely new event ids animate; a row
                    // already on screen is settled at full opacity.
                    for (i, msg) in conv.messages.iter().enumerate() {
                        crate::anim::entrance(ui, "msg", msg.event_id, i, |ui| {
                            self.message_row(ui, data, sink, space, msg);
                        });
                        ui.add_space(Style::SP_XS);
                    }
                }
                _ => {
                    ui.label(
                        egui::RichText::new("No messages in this space yet.")
                            .color(Style::TEXT_DIM),
                    );
                }
            });
        ui.separator();
        self.composer(ui, sink, space);
    }

    /// One message row: header (author · age · delivery), the Markdown body (or
    /// the inline editor / tombstone), and the action row (thread + amend).
    fn message_row(
        &mut self,
        ui: &mut egui::Ui,
        data: &dyn crate::CollabData,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        msg: &MessageView,
    ) {
        let now = data.now_unix_ms();
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(msg.author.as_str())
                    .small()
                    .strong()
                    .color(Style::TEXT_STRONG),
            );
            ui.label(
                egui::RichText::new(relative_age(now, msg.created_unix_ms))
                    .small()
                    .color(Style::TEXT_DIM),
            );
            if msg.edited && !msg.deleted {
                ui.label(
                    egui::RichText::new("edited")
                        .small()
                        .italics()
                        .color(Style::TEXT_DIM),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                icons::icon(
                    ui,
                    icons::delivery_icon(msg.delivery),
                    Style::SP_M,
                    delivery_color(msg.delivery),
                )
                .comms_hover_text(delivery_label(msg.delivery));
            });
        });

        let editing_this = matches!(&self.editing, Some((id, _)) if *id == msg.event_id);
        if msg.deleted {
            ui.label(
                egui::RichText::new("This message was deleted.")
                    .italics()
                    .color(Style::TEXT_DIM),
            );
        } else if editing_this {
            self.edit_editor(ui, sink, space);
        } else {
            render_markdown(ui, msg.body.as_str());
        }

        if !msg.deleted && !editing_this {
            self.message_actions(ui, data, sink, space, msg);
        }
    }

    /// The per-message action row: the thread affordance (open existing / start
    /// new) and the amend affordance (edit + delete, shown enabled inside the
    /// author window, *denied* past it, hidden for others).
    fn message_actions(
        &mut self,
        ui: &mut egui::Ui,
        data: &dyn crate::CollabData,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        msg: &MessageView,
    ) {
        let affordance = amend_affordance(data.me(), data.now_unix_ms(), msg);
        ui.horizontal(|ui| {
            if msg.reply_count > 0 {
                if icons::icon_button(ui, icons::THREAD, Style::SP_M, Style::ACCENT, "Open thread")
                    .clicked()
                {
                    if let Some(thread) = data.thread_for_root(space, msg.event_id) {
                        self.open_thread = Some(thread);
                    }
                }
                let plural = if msg.reply_count == 1 {
                    "reply"
                } else {
                    "replies"
                };
                ui.label(
                    egui::RichText::new(format!("{} {plural}", msg.reply_count))
                        .small()
                        .color(Style::TEXT_DIM),
                );
            } else if icons::icon_button(
                ui,
                icons::THREAD,
                Style::SP_M,
                Style::TEXT_DIM,
                "Start thread",
            )
            .clicked()
            {
                sink.emit(CollabCommand::StartThread {
                    space,
                    root: msg.event_id,
                    title: None,
                });
            }

            match affordance {
                AmendAffordance::Allowed => {
                    if icons::icon_button(ui, icons::EDIT, Style::SP_M, Style::TEXT_DIM, "Edit")
                        .clicked()
                    {
                        self.editing = Some((msg.event_id, msg.body.as_str().to_owned()));
                    }
                    if icons::icon_button(ui, icons::DELETE, Style::SP_M, Style::DANGER, "Delete")
                        .clicked()
                    {
                        sink.emit(CollabCommand::DeleteMessage {
                            space,
                            target: msg.event_id,
                        });
                    }
                }
                AmendAffordance::DeniedExpired => {
                    icons::icon(ui, icons::EDIT, Style::SP_M, Style::DISABLED)
                        .comms_hover_text("Edit window passed (5 min)");
                    icons::icon(ui, icons::DELETE, Style::SP_M, Style::DISABLED)
                        .comms_hover_text("Delete window passed (5 min)");
                }
                AmendAffordance::Hidden => {}
            }
        });
    }

    /// The inline edit editor for the message currently in [`Self::editing`].
    fn edit_editor(&mut self, ui: &mut egui::Ui, sink: &mut crate::CommandSink, space: SpaceId) {
        let mut result: Option<bool> = None;
        let mut input_was_capped = false;
        if let Some((_, buf)) = self.editing.as_mut() {
            input_was_capped = cap_message_input(buf);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(buf)
                        .desired_width(f32::INFINITY)
                        .char_limit(MAX_MESSAGE_BODY_BYTES)
                        .hint_text("Edit message"),
                );
                if ui.button("Save").clicked() {
                    result = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    result = Some(false);
                }
            });
            input_was_capped |= cap_message_input(buf);
        }
        match result {
            Some(true) if input_was_capped => {}
            Some(true) => {
                if let Some((target, buf)) = self.editing.take() {
                    let text = buf.trim().to_owned();
                    if !text.is_empty() {
                        sink.emit(CollabCommand::EditMessage {
                            space,
                            target,
                            body: MessageBody::new(text),
                        });
                    }
                }
            }
            Some(false) => self.editing = None,
            None => {}
        }
        if input_was_capped {
            message_input_notice(ui);
        }
    }

    /// The main-timeline composer. <kbd>Enter</kbd> (or the Send glyph) emits
    /// [`SendMessage`](CollabCommand::SendMessage); the draft persists locally,
    /// keyed by space, so switching away and back never loses it, and it clears
    /// only on a real emit.
    fn composer(&mut self, ui: &mut egui::Ui, sink: &mut crate::CommandSink, space: SpaceId) {
        let edit_id = self.composer_edit_id(space);
        let mut buf = self.drafts.get(&space).cloned().unwrap_or_default();
        let mut input_was_capped = cap_message_input(&mut buf);
        let mut send = false;
        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut buf)
                    .id(edit_id)
                    .desired_width(f32::INFINITY)
                    .char_limit(MAX_MESSAGE_BODY_BYTES)
                    .hint_text("Message  ·  Enter to send"),
            );
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (resp.lost_focus() || resp.has_focus()) && enter {
                send = true;
            }
            if icons::icon_button(ui, icons::SEND, Style::SP_M, Style::ACCENT, "Send").clicked() {
                send = true;
            }
        });
        input_was_capped |= cap_message_input(&mut buf);
        let text = buf.trim().to_owned();
        if send && !input_was_capped && !text.is_empty() {
            sink.emit(CollabCommand::SendMessage {
                space,
                thread: None,
                body: MessageBody::new(text),
            });
            buf.clear();
        }
        if input_was_capped {
            message_input_notice(ui);
        }
        self.drafts.insert(space, buf);
    }

    /// The anchored thread column: the thread's root + replies, a resolved
    /// marker, a reply composer emitting
    /// [`ReplyInThread`](CollabCommand::ReplyInThread), and a close control.
    fn thread_column(
        &mut self,
        ui: &mut egui::Ui,
        data: &dyn crate::CollabData,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        thread: ThreadId,
    ) {
        let mut close = false;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Thread").strong().color(Style::TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::icon_button(
                    ui,
                    "window-close",
                    Style::SP_M,
                    Style::TEXT_DIM,
                    "Close thread",
                )
                .clicked()
                {
                    close = true;
                }
            });
        });
        ui.separator();
        if close {
            self.open_thread = None;
            return;
        }

        let now = data.now_unix_ms();
        let composer_h = Style::SP_XL + Style::SP_M;
        egui::ScrollArea::vertical()
            .id_salt("collab-thread")
            .auto_shrink([false, false])
            .max_height((ui.available_height() - composer_h).max(Style::SP_XL))
            .show(ui, |ui| match data.thread(space, thread) {
                Some(timeline) => {
                    thread_message(ui, &timeline.root, now);
                    for reply in &timeline.replies {
                        ui.indent("collab-thread-reply", |ui| thread_message(ui, reply, now));
                    }
                    if timeline.resolved {
                        ui.add_space(Style::SP_XS);
                        ui.label(
                            egui::RichText::new("Thread resolved")
                                .small()
                                .color(Style::OK),
                        );
                    }
                }
                None => {
                    ui.label(egui::RichText::new("Thread not loaded.").color(Style::TEXT_DIM));
                }
            });
        ui.separator();
        self.thread_composer(ui, sink, space, thread);
    }

    /// The thread reply composer. <kbd>Enter</kbd> (or the Send glyph) emits
    /// [`ReplyInThread`](CollabCommand::ReplyInThread) with a per-thread draft.
    fn thread_composer(
        &mut self,
        ui: &mut egui::Ui,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        thread: ThreadId,
    ) {
        let edit_id = egui::Id::new(("mde-collab-thread-composer", thread.as_uuid()));
        let mut buf = self.thread_drafts.get(&thread).cloned().unwrap_or_default();
        let mut input_was_capped = cap_message_input(&mut buf);
        let mut send = false;
        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut buf)
                    .id(edit_id)
                    .desired_width(f32::INFINITY)
                    .char_limit(MAX_MESSAGE_BODY_BYTES)
                    .hint_text("Reply in thread"),
            );
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (resp.lost_focus() || resp.has_focus()) && enter {
                send = true;
            }
            if icons::icon_button(ui, icons::SEND, Style::SP_M, Style::ACCENT, "Send reply")
                .clicked()
            {
                send = true;
            }
        });
        input_was_capped |= cap_message_input(&mut buf);
        let text = buf.trim().to_owned();
        if send && !input_was_capped && !text.is_empty() {
            sink.emit(CollabCommand::ReplyInThread {
                space,
                thread,
                body: MessageBody::new(text),
            });
            buf.clear();
        }
        if input_was_capped {
            message_input_notice(ui);
        }
        self.thread_drafts.insert(thread, buf);
    }
}

/// Match the command pipeline's 256 KiB UTF-8 body contract at the visible
/// input boundary. `TextEdit::char_limit` bounds ordinary insertion, while
/// this byte-based guard also handles multi-byte pastes and restored drafts.
/// The prefix remains valid UTF-8 and is capped before the next frame can lay it
/// out, so a hostile draft cannot expand the composer or emit a rejected action.
const MAX_MESSAGE_BODY_BYTES: usize = 256 * 1024;

fn cap_message_input(value: &mut String) -> bool {
    if value.len() <= MAX_MESSAGE_BODY_BYTES {
        return false;
    }

    let boundary = value
        .char_indices()
        .take_while(|(offset, character)| {
            offset.saturating_add(character.len_utf8()) <= MAX_MESSAGE_BODY_BYTES
        })
        .last()
        .map_or(0, |(offset, character)| offset + character.len_utf8());
    value.truncate(boundary);
    true
}

fn message_input_notice(ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new("Message limited to 256 KiB — review before sending.")
            .small()
            .color(Style::WARN),
    );
}

/// Render one message inside a thread column (read-only: header + body).
fn thread_message(ui: &mut egui::Ui, msg: &MessageView, now_unix_ms: i64) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(msg.author.as_str())
                .small()
                .strong()
                .color(Style::TEXT_STRONG),
        );
        ui.label(
            egui::RichText::new(relative_age(now_unix_ms, msg.created_unix_ms))
                .small()
                .color(Style::TEXT_DIM),
        );
    });
    if msg.deleted {
        ui.label(
            egui::RichText::new("This message was deleted.")
                .italics()
                .color(Style::TEXT_DIM),
        );
    } else {
        render_markdown(ui, msg.body.as_str());
    }
    ui.add_space(Style::SP_XS);
}

/// Keep one hostile line from becoming an unbounded egui layout job. Chunks are
/// split only at UTF-8 character boundaries and are all rendered, so this is a
/// layout guard rather than a content limit.
const MAX_MARKDOWN_LAYOUT_CHARS: usize = 1024;

/// Visit bounded UTF-8-safe slices without allocating a normalized copy of the
/// message. An empty slice is significant for empty headings and bullets, which
/// retain the same visible affordance as the previous renderer.
fn for_each_markdown_chunk(mut text: &str, mut visit: impl FnMut(&str)) {
    if text.is_empty() {
        visit(text);
        return;
    }

    while !text.is_empty() {
        let (chunk, rest) = text
            .char_indices()
            .nth(MAX_MARKDOWN_LAYOUT_CHARS)
            .map_or((text, ""), |(byte, _)| text.split_at(byte));
        visit(chunk);
        text = rest;
    }
}

/// Add styled message text with a finite layout job and anywhere-breaking wrap.
/// The latter matters for hostile source such as a long URL or an unbroken hash:
/// the label stays inside its pane instead of expanding the conversation column.
/// Markdown remains source text here; this helper does not interpret links or
/// discard control characters/content.
fn markdown_label(ui: &mut egui::Ui, text: &str, mut style: impl FnMut(&str) -> egui::RichText) {
    for_each_markdown_chunk(text, |chunk| {
        let mut job = egui::text::LayoutJob::default();
        style(chunk).append_to(
            &mut job,
            ui.style(),
            egui::FontSelection::Default,
            ui.text_valign(),
        );
        job.wrap.break_anywhere = true;
        ui.add(egui::Label::new(job).wrap());
    });
}

/// A lightweight Markdown line treatment for a message body: ATX headings
/// (`#`/`##`/`###`, sized on the shared type ramp) and `-`/`*` bullets render as
/// such; every other line is body text. Inline spans are shown as their Markdown
/// source in this phase — the honest source, never a misrendered guess.
fn render_markdown(ui: &mut egui::Ui, body: &str) {
    for raw in body.lines() {
        let line = raw.trim_end();
        if let Some(rest) = line.strip_prefix("### ") {
            heading(ui, rest, 3);
        } else if let Some(rest) = line.strip_prefix("## ") {
            heading(ui, rest, 2);
        } else if let Some(rest) = line.strip_prefix("# ") {
            heading(ui, rest, 1);
        } else if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            let mut first = true;
            for_each_markdown_chunk(rest, |chunk| {
                if first {
                    first = false;
                    ui.horizontal(|ui| {
                        markdown_label(ui, "•", |text| {
                            egui::RichText::new(text).color(Style::TEXT_DIM)
                        });
                        markdown_label(ui, chunk, |text| {
                            egui::RichText::new(text).color(Style::TEXT)
                        });
                    });
                } else {
                    markdown_label(ui, chunk, |text| {
                        egui::RichText::new(text).color(Style::TEXT)
                    });
                }
            });
        } else if line.is_empty() {
            ui.add_space(Style::SP_XS);
        } else {
            markdown_label(ui, line, |text| {
                egui::RichText::new(text).color(Style::TEXT)
            });
        }
    }
}

/// A Markdown heading line on the shared type ramp.
fn heading(ui: &mut egui::Ui, text: &str, level: u8) {
    markdown_label(ui, text, |chunk| {
        egui::RichText::new(chunk)
            .size(Style::heading_size(level))
            .strong()
            .color(Style::TEXT_STRONG)
    });
}

/// The Carbon tint for a delivery state.
const fn delivery_color(delivery: DeliveryState) -> egui::Color32 {
    match delivery {
        DeliveryState::Sent => Style::TEXT_DIM,
        DeliveryState::Delivered => Style::OK,
        DeliveryState::Queued => Style::WARN,
    }
}

/// The hover label for a delivery state (honest — never a faked read receipt).
const fn delivery_label(delivery: DeliveryState) -> &'static str {
    match delivery {
        DeliveryState::Sent => "Sent",
        DeliveryState::Delivered => "Delivered",
        DeliveryState::Queued => "Queued — recipient offline",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        cap_message_input, for_each_markdown_chunk, MAX_MARKDOWN_LAYOUT_CHARS,
        MAX_MESSAGE_BODY_BYTES,
    };

    #[test]
    fn message_input_cap_is_utf8_safe_at_the_oversized_action_boundary() {
        let mut input = format!("🙂{}🦀", "x".repeat(MAX_MESSAGE_BODY_BYTES - 7));

        assert!(cap_message_input(&mut input));
        assert_eq!(input.len(), MAX_MESSAGE_BODY_BYTES - 3);
        assert!(input.is_char_boundary(input.len()));
        assert!(input.starts_with('🙂'));
        assert!(!input.contains('🦀'));

        assert!(!cap_message_input(&mut input));
        assert_eq!(input.len(), MAX_MESSAGE_BODY_BYTES - 3);
    }

    #[test]
    fn markdown_chunks_preserve_text_at_utf8_boundaries() {
        let input = format!(
            "{}é🦀{}",
            "a".repeat(MAX_MARKDOWN_LAYOUT_CHARS + 1),
            "z".repeat(MAX_MARKDOWN_LAYOUT_CHARS + 1)
        );
        let mut chunks = Vec::<String>::new();
        for_each_markdown_chunk(&input, |chunk| chunks.push(chunk.to_owned()));

        assert!(chunks.len() >= 3);
        assert_eq!(chunks.concat(), input);
        assert!(chunks
            .iter()
            .all(|chunk| chunk.chars().count() <= MAX_MARKDOWN_LAYOUT_CHARS));
    }

    #[test]
    fn markdown_chunks_keep_empty_markdown_content() {
        let mut chunks = Vec::<String>::new();
        for_each_markdown_chunk("", |chunk| chunks.push(chunk.to_owned()));

        assert_eq!(chunks, vec![String::new()]);
    }
}
