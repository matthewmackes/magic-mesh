//! Messages mode — a Markdown conversation timeline
//! ([`ConversationTimeline`](mde_collab_types::ConversationTimeline)) with
//! anchored threads ([`ThreadTimeline`](mde_collab_types::ThreadTimeline)), a
//! multiline composer whose <kbd>Ctrl</kbd>+<kbd>Enter</kbd> emits
//! [`SendMessage`](mde_collab_types::CollabCommand::SendMessage) with a
//! locally-persisted draft, honest delivery state, and an edit/delete affordance
//! that reflects the core's five-minute author window (spec §3). Shared message
//! pins and private saved-message controls read their retained projections and
//! emit typed commands; no mesh or private state is fabricated locally.

use mde_egui::egui;
use mde_egui::Style;

use mde_collab_types::{
    CollabCommand, DeliveryState, EventId, MessageBody, MessageView, SpaceId, TaskView, ThreadId,
};

use crate::icons::CommsHoverExt;
use crate::{amend_affordance, icons, relative_age, AmendAffordance, CommunicationsSurface};

const MAX_TASK_TITLE_BYTES: usize = 512;

/// A constrained quick reaction held only in this surface's local view state.
///
/// This deliberately is not a command, event, or read-model field: the operator
/// requested local-only reactions while the larger emoji/GIF/sticker expression
/// system is out of scope. These three labels cover fast acknowledgement without
/// introducing a mesh-visible reaction protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalReaction {
    /// Acknowledge that the message was seen.
    Ack,
    /// Mark the message as checked/handled locally.
    Check,
    /// Keep watching this message locally.
    Watch,
}

impl LocalReaction {
    /// The compact ordered quick-reaction set.
    pub(crate) const ALL: [Self; 3] = [Self::Ack, Self::Check, Self::Watch];

    /// User-facing chip label.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Ack => "Ack",
            Self::Check => "Check",
            Self::Watch => "Watch",
        }
    }
}

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

    /// Render the basic channel Tasks / action-items pane for the selected space.
    pub(crate) fn tasks_body(
        &mut self,
        ui: &mut egui::Ui,
        data: &dyn crate::CollabData,
        sink: &mut crate::CommandSink,
    ) {
        let Some(space) = self.selected_space() else {
            ui.label(
                egui::RichText::new("Select a space to open its channel tasks.")
                    .color(Style::TEXT_DIM),
            );
            return;
        };

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Channel tasks")
                    .strong()
                    .color(Style::TEXT_STRONG),
            );
            ui.label(
                egui::RichText::new("operator-authored action items")
                    .small()
                    .color(Style::TEXT_DIM),
            );
        });
        ui.add_space(Style::SP_XS);
        self.task_composer(ui, sink, space);
        ui.separator();

        match data.channel_tasks(space) {
            Some(tasks) if !tasks.tasks.is_empty() => {
                egui::ScrollArea::vertical()
                    .id_salt("collab-channel-tasks")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (i, task) in tasks.tasks.iter().enumerate() {
                            crate::anim::entrance(ui, "task", task.task, i, |ui| {
                                self.task_row(ui, sink, task, data.now_unix_ms());
                            });
                            ui.add_space(Style::SP_XS);
                        }
                    });
            }
            _ => {
                ui.label(
                    egui::RichText::new("No tasks in this channel yet.").color(Style::TEXT_DIM),
                );
            }
        }
    }

    fn task_composer(&mut self, ui: &mut egui::Ui, sink: &mut crate::CommandSink, space: SpaceId) {
        let mut buf = self.task_drafts.get(&space).cloned().unwrap_or_default();
        let mut input_was_capped = cap_task_title_input(&mut buf);
        let mut create = false;
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut buf)
                    .desired_width(f32::INFINITY)
                    .char_limit(MAX_TASK_TITLE_BYTES)
                    .hint_text("Add a channel task"),
            );
            if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                create = true;
            }
            if icons::icon_button(
                ui,
                icons::TASK_CREATE,
                Style::SP_M,
                Style::ACCENT,
                "Create task",
            )
            .clicked()
            {
                create = true;
            }
        });
        input_was_capped |= cap_task_title_input(&mut buf);
        self.task_drafts.insert(space, buf);
        if create && !input_was_capped {
            self.create_task_from_draft(sink, space);
        }
        if input_was_capped {
            task_title_notice(ui);
        }
    }

    fn task_row(
        &self,
        ui: &mut egui::Ui,
        sink: &mut crate::CommandSink,
        task: &TaskView,
        now_unix_ms: i64,
    ) {
        let title_id = ui.make_persistent_id(("collab-task-title", task.task));
        let mut title = ui
            .ctx()
            .data_mut(|data| data.get_temp::<String>(title_id))
            .unwrap_or_else(|| task.title.clone());
        let mut title_was_capped = false;
        let mut update = false;
        let mut reopen = false;
        ui.group(|ui| {
            ui.horizontal_wrapped(|ui| {
                let check_hint = if task.checked {
                    "Clear task check"
                } else {
                    "Check task"
                };
                if !task.completed
                    && icons::icon_button(
                        ui,
                        icons::TASK_CHECK,
                        Style::SP_M,
                        if task.checked {
                            Style::OK
                        } else {
                            Style::TEXT_DIM
                        },
                        check_hint,
                    )
                    .clicked()
                {
                    self.set_task_checked(sink, task.space, task.task, !task.checked);
                }
                if task.completed {
                    ui.label(
                        egui::RichText::new(task.title.as_str())
                            .strong()
                            .color(Style::TEXT_DIM),
                    );
                } else {
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut title)
                            .desired_width(220.0)
                            .char_limit(MAX_TASK_TITLE_BYTES),
                    );
                    title_was_capped |= cap_task_title_input(&mut title);
                    if response.changed() {
                        ui.ctx().data_mut(|data| {
                            data.insert_temp(title_id, title.clone());
                        });
                    }
                    if ui
                        .button("Update")
                        .on_hover_text("Publish the updated task title")
                        .clicked()
                    {
                        update = true;
                    }
                }
                ui.label(
                    egui::RichText::new(format!(
                        "by {} · {}",
                        task.created_by.as_str(),
                        relative_age(now_unix_ms, task.created_unix_ms)
                    ))
                    .small()
                    .color(Style::TEXT_DIM),
                );
                if let Some(source) = task.source {
                    ui.label(
                        egui::RichText::new(format!("source {source}"))
                            .small()
                            .color(Style::TEXT_DIM),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if task.completed {
                        let who = task
                            .completed_by
                            .as_ref()
                            .map_or("unknown", mde_collab_types::ActorId::as_str);
                        ui.label(
                            egui::RichText::new(format!("Completed by {who}"))
                                .small()
                                .color(Style::OK),
                        );
                    } else if icons::icon_button(
                        ui,
                        icons::TASK_COMPLETE,
                        Style::SP_M,
                        Style::OK,
                        "Complete task",
                    )
                    .clicked()
                    {
                        self.complete_task(sink, task.space, task.task);
                    }
                    if task.completed
                        && icons::icon_button(
                            ui,
                            icons::THREAD_REOPEN,
                            Style::SP_M,
                            Style::ACCENT,
                            "Reopen task",
                        )
                        .clicked()
                    {
                        reopen = true;
                    }
                });
            });
        });
        if title_was_capped {
            task_title_notice(ui);
        }
        if update {
            self.update_task(sink, task.space, task.task, title.as_str());
            ui.ctx()
                .data_mut(|data| data.remove_temp::<String>(title_id));
        }
        if reopen {
            self.reopen_task(sink, task.space, task.task);
        }
    }

    /// Emit `CreateTask` from this channel's local draft.
    pub(crate) fn create_task_from_draft(&mut self, sink: &mut crate::CommandSink, space: SpaceId) {
        let title = self
            .task_drafts
            .get(&space)
            .map_or("", String::as_str)
            .trim()
            .to_owned();
        if title.is_empty() || title.len() > MAX_TASK_TITLE_BYTES {
            return;
        }
        sink.emit(CollabCommand::CreateTask {
            space,
            title,
            source: None,
        });
        self.task_drafts.insert(space, String::new());
    }

    /// Emit the explicit checked-state command the caller routes to the worker.
    pub(crate) fn set_task_checked(
        &self,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        task: EventId,
        checked: bool,
    ) {
        sink.emit(CollabCommand::SetTaskChecked {
            space,
            task,
            checked,
        });
    }

    /// Emit a bounded title update for an open channel task.
    pub(crate) fn update_task(
        &self,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        task: EventId,
        title: &str,
    ) {
        let title = title.trim();
        if title.is_empty() || title.len() > MAX_TASK_TITLE_BYTES {
            return;
        }
        sink.emit(CollabCommand::UpdateTask {
            space,
            task,
            title: title.to_owned(),
        });
    }

    /// Emit the explicit task-completion command the caller routes to the worker.
    pub(crate) fn complete_task(
        &self,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        task: EventId,
    ) {
        sink.emit(CollabCommand::CompleteTask { space, task });
    }

    /// Emit the explicit task-reopen command the caller routes to the worker.
    pub(crate) fn reopen_task(&self, sink: &mut crate::CommandSink, space: SpaceId, task: EventId) {
        sink.emit(CollabCommand::ReopenTask { space, task });
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
        let find_query = self.channel_find(space).trim().to_owned();
        if !find_query.is_empty() {
            let matches = data.conversation(space).map_or(0, |conv| {
                channel_find_messages(&conv.messages, find_query.as_str()).len()
            });
            let plural = if matches == 1 { "match" } else { "matches" };
            ui.label(
                egui::RichText::new(format!("{matches} current-channel {plural}"))
                    .small()
                    .color(Style::TEXT_DIM),
            );
            ui.add_space(Style::SP_XS);
        }
        egui::ScrollArea::vertical()
            .id_salt("collab-timeline")
            .auto_shrink([false, false])
            .max_height((ui.available_height() - composer_h).max(Style::SP_XL))
            .show(ui, |ui| match data.conversation(space) {
                Some(conv) if !conv.messages.is_empty() => {
                    let messages = channel_find_messages(&conv.messages, find_query.as_str());
                    // A newly-appearing row fades up on the shared staggered list
                    // entrance (lock #4) — only genuinely new event ids animate; a row
                    // already on screen is settled at full opacity.
                    for (i, msg) in messages.iter().enumerate() {
                        crate::anim::entrance(ui, "msg", msg.event_id, i, |ui| {
                            self.message_row(ui, data, sink, space, msg);
                        });
                        ui.add_space(Style::SP_XS);
                    }
                    if messages.is_empty() {
                        ui.label(
                            egui::RichText::new("No current-channel matches.")
                                .color(Style::TEXT_DIM),
                        );
                    }
                }
                _ => {
                    let empty = if find_query.is_empty() {
                        "No messages in this space yet."
                    } else {
                        "No current-channel matches."
                    };
                    ui.label(egui::RichText::new(empty).color(Style::TEXT_DIM));
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

            self.local_reaction_buttons(ui, msg.event_id);
            self.message_keep_affordances(ui, data, sink, space, msg.event_id);

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

    /// Render shared pin and actor-private save controls from the retained
    /// projections and emit the corresponding typed command on activation.
    pub(crate) fn message_keep_affordances(
        &self,
        ui: &mut egui::Ui,
        data: &dyn crate::CollabData,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        message: EventId,
    ) {
        let pinned = data.message_pinned(space, message);
        let saved = data.message_saved(space, message);
        ui.separator();
        ui.label(egui::RichText::new("Keep").small().color(Style::TEXT_DIM));
        if ui
            .add(
                egui::Button::new(
                    egui::RichText::new(if pinned { "Unpin" } else { "Pin" })
                        .small()
                        .color(if pinned { Style::WARN } else { Style::TEXT_DIM }),
                )
                .sense(egui::Sense::click()),
            )
            .on_hover_text(if pinned {
                "Remove the shared pin"
            } else {
                "Pin for everyone in this space"
            })
            .clicked()
        {
            self.toggle_message_pin(sink, space, message, pinned);
        }
        if ui
            .add(
                egui::Button::new(
                    egui::RichText::new(if saved { "Unsave" } else { "Save" })
                        .small()
                        .color(if saved { Style::WARN } else { Style::TEXT_DIM }),
                )
                .sense(egui::Sense::click()),
            )
            .on_hover_text(if saved {
                "Remove your private saved mark"
            } else {
                "Save privately for this seat"
            })
            .clicked()
        {
            self.toggle_message_save(sink, space, message, saved);
        }
    }

    /// Emit the shared pin toggle selected by the current read model.
    pub(crate) fn toggle_message_pin(
        &self,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        message: EventId,
        pinned: bool,
    ) {
        sink.emit(if pinned {
            CollabCommand::UnpinMessage {
                space,
                target: message,
            }
        } else {
            CollabCommand::PinMessage {
                space,
                target: message,
            }
        });
    }

    /// Emit the local private-save toggle selected by the current read model.
    pub(crate) fn toggle_message_save(
        &self,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        message: EventId,
        saved: bool,
    ) {
        sink.emit(if saved {
            CollabCommand::UnsaveMessage {
                space,
                target: message,
            }
        } else {
            CollabCommand::SaveMessage {
                space,
                target: message,
            }
        });
    }

    /// The per-seat quick-reaction chips. These mutate only local view state and
    /// intentionally take no [`CommandSink`], so they cannot publish mesh state.
    pub(crate) fn local_reaction_buttons(&mut self, ui: &mut egui::Ui, message: EventId) {
        ui.separator();
        ui.label(egui::RichText::new("Local").small().color(Style::TEXT_DIM));
        let selected = self.local_reaction(message);
        for reaction in LocalReaction::ALL {
            let active = selected == Some(reaction);
            let label = if active {
                format!("{} ✓", reaction.label())
            } else {
                reaction.label().to_owned()
            };
            if ui
                .add(egui::Button::new(egui::RichText::new(label).small().color(
                    if active {
                        Style::TEXT_STRONG
                    } else {
                        Style::TEXT_DIM
                    },
                )))
                .comms_hover_text("Local-only reaction; not sent to the mesh")
                .clicked()
            {
                self.toggle_local_reaction(message, reaction);
            }
        }
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

    /// The main-timeline composer. <kbd>Ctrl</kbd>+<kbd>Enter</kbd> (or the Send
    /// glyph) emits
    /// [`SendMessage`](CollabCommand::SendMessage); the draft persists locally,
    /// keyed by space, so switching away and back never loses it, and it clears
    /// only on a real emit.
    fn composer(&mut self, ui: &mut egui::Ui, sink: &mut crate::CommandSink, space: SpaceId) {
        let edit_id = self.composer_edit_id(space);
        let mut buf = self.drafts.get(&space).cloned().unwrap_or_default();
        let mut input_was_capped = cap_message_input(&mut buf);
        let mut send = false;
        ui.horizontal(|ui| {
            let newline_count_before = newline_count(&buf);
            let resp = ui.add(
                egui::TextEdit::multiline(&mut buf)
                    .id(edit_id)
                    .desired_width(f32::INFINITY)
                    .desired_rows(3)
                    .char_limit(MAX_MESSAGE_BODY_BYTES)
                    .hint_text("Message  ·  Ctrl+Enter to send"),
            );
            let (plain_enter, ctrl_enter) = composer_enter_state(ui);
            insert_newline_if_text_edit_did_not(&resp, plain_enter, newline_count_before, &mut buf);
            if (resp.lost_focus() || resp.has_focus()) && ctrl_enter {
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
            match data.thread(space, thread).map(|timeline| timeline.resolved) {
                Some(resolved) => {
                    if thread_resolution_button(ui, resolved).clicked() {
                        self.set_thread_resolved(sink, space, thread, !resolved);
                    }
                }
                None => {
                    ui.label(
                        egui::RichText::new("Status unavailable")
                            .small()
                            .color(Style::TEXT_DIM),
                    );
                }
            }
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

    /// Emit the convergent thread-resolution command for the caller to route.
    pub(crate) fn set_thread_resolved(
        &self,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        thread: ThreadId,
        resolved: bool,
    ) {
        if resolved {
            sink.emit(CollabCommand::ResolveThread { space, thread });
        } else {
            sink.emit(CollabCommand::ReopenThread { space, thread });
        }
    }

    /// The thread reply composer. <kbd>Ctrl</kbd>+<kbd>Enter</kbd> (or the Send
    /// glyph) emits
    /// [`ReplyInThread`](CollabCommand::ReplyInThread) with a per-thread draft.
    fn thread_composer(
        &mut self,
        ui: &mut egui::Ui,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        thread: ThreadId,
    ) {
        let edit_id = self.thread_composer_edit_id(thread);
        let mut buf = self.thread_drafts.get(&thread).cloned().unwrap_or_default();
        let mut input_was_capped = cap_message_input(&mut buf);
        let mut send = false;
        ui.horizontal(|ui| {
            let newline_count_before = newline_count(&buf);
            let resp = ui.add(
                egui::TextEdit::multiline(&mut buf)
                    .id(edit_id)
                    .desired_width(f32::INFINITY)
                    .desired_rows(3)
                    .char_limit(MAX_MESSAGE_BODY_BYTES)
                    .hint_text("Reply in thread  ·  Ctrl+Enter to send"),
            );
            let (plain_enter, ctrl_enter) = composer_enter_state(ui);
            insert_newline_if_text_edit_did_not(&resp, plain_enter, newline_count_before, &mut buf);
            if (resp.lost_focus() || resp.has_focus()) && ctrl_enter {
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

fn cap_task_title_input(value: &mut String) -> bool {
    if value.len() <= MAX_TASK_TITLE_BYTES {
        return false;
    }

    let boundary = value
        .char_indices()
        .take_while(|(offset, character)| {
            offset.saturating_add(character.len_utf8()) <= MAX_TASK_TITLE_BYTES
        })
        .last()
        .map_or(0, |(offset, character)| offset + character.len_utf8());
    value.truncate(boundary);
    true
}

fn task_title_notice(ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new("Task titles are limited to 512 bytes.")
            .small()
            .color(Style::WARN),
    );
}

fn composer_enter_state(ui: &egui::Ui) -> (bool, bool) {
    ui.input(|input| {
        let enter = input.key_pressed(egui::Key::Enter);
        let ctrl_enter = enter && input.modifiers.ctrl;
        let plain_enter = enter && !input.modifiers.ctrl && !input.modifiers.command;
        (plain_enter, ctrl_enter)
    })
}

fn newline_count(value: &str) -> usize {
    value
        .as_bytes()
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
}

fn insert_newline_if_text_edit_did_not(
    response: &egui::Response,
    plain_enter: bool,
    newline_count_before: usize,
    value: &mut String,
) {
    if !plain_enter || !(response.has_focus() || response.lost_focus()) {
        return;
    }
    if newline_count(value) <= newline_count_before {
        value.push('\n');
    }
}

/// Whether a main-timeline message should remain visible under the local
/// current-channel find query. Search is intentionally scoped to the selected
/// channel and local view state; it does not index every space and does not
/// publish a collaboration event.
#[must_use]
pub(crate) fn message_matches_channel_find(msg: &MessageView, query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }
    let needle = query.to_lowercase();
    msg.author.as_str().to_lowercase().contains(&needle)
        || (!msg.deleted && msg.body.to_lowercase().contains(&needle))
}

/// The visible main-timeline messages under the local current-channel find.
/// Kept as a pure model so render tests can assert filtering without depending
/// on entrance-animation timing.
#[must_use]
pub(crate) fn channel_find_messages<'a>(
    messages: &'a [MessageView],
    query: &str,
) -> Vec<&'a MessageView> {
    messages
        .iter()
        .filter(|msg| message_matches_channel_find(msg, query))
        .collect()
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

fn thread_resolution_button(ui: &mut egui::Ui, resolved: bool) -> egui::Response {
    let (label, icon, tint, hint) = if resolved {
        (
            "Reopen",
            icons::THREAD_REOPEN,
            Style::ACCENT,
            "Reopen this thread",
        )
    } else {
        (
            "Resolve",
            icons::THREAD_RESOLVE,
            Style::OK,
            "Mark this thread resolved",
        )
    };
    let response = ui
        .horizontal(|ui| {
            let icon = icons::icon_button(ui, icon, Style::SP_M, tint, hint);
            let text = ui
                .add(egui::Button::new(
                    egui::RichText::new(label).small().color(Style::TEXT),
                ))
                .comms_hover_text(hint);
            icon | text
        })
        .inner;
    response
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
        cap_message_input, channel_find_messages, for_each_markdown_chunk,
        message_matches_channel_find, LocalReaction, MAX_MARKDOWN_LAYOUT_CHARS,
        MAX_MESSAGE_BODY_BYTES,
    };
    use mde_collab_types::{ActorId, DeliveryState, EventId, MessageView};

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

    #[test]
    fn channel_find_matches_author_or_visible_body_only() {
        let msg = MessageView {
            event_id: EventId::new(),
            author: ActorId::new("Falcon"),
            created_unix_ms: 1_000,
            body: "Deploy window is green".to_owned(),
            edited: false,
            deleted: false,
            delivery: DeliveryState::Delivered,
            reply_count: 0,
        };
        assert!(message_matches_channel_find(&msg, "deploy"));
        assert!(message_matches_channel_find(&msg, "falcon"));
        assert!(!message_matches_channel_find(&msg, "incident"));

        let deleted = MessageView {
            deleted: true,
            ..msg
        };
        assert!(
            !message_matches_channel_find(&deleted, "deploy"),
            "deleted message bodies must not remain searchable"
        );
        assert!(
            message_matches_channel_find(&deleted, "falcon"),
            "the visible author line remains searchable"
        );
    }

    #[test]
    fn channel_find_message_model_filters_without_global_state() {
        let peer = ActorId::new("falcon");
        let deploy = MessageView {
            event_id: EventId::new(),
            author: peer.clone(),
            created_unix_ms: 1_000,
            body: "Deploy is green.".to_owned(),
            edited: false,
            deleted: false,
            delivery: DeliveryState::Delivered,
            reply_count: 0,
        };
        let incident = MessageView {
            event_id: EventId::new(),
            author: peer,
            created_unix_ms: 1_100,
            body: "Incident bridge is noisy.".to_owned(),
            edited: false,
            deleted: false,
            delivery: DeliveryState::Delivered,
            reply_count: 0,
        };
        let messages = vec![deploy, incident];
        let filtered = channel_find_messages(&messages, "deploy");

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].body, "Deploy is green.");
    }

    #[test]
    fn local_reaction_labels_are_constrained_and_ordered() {
        assert_eq!(
            LocalReaction::ALL.map(LocalReaction::label),
            ["Ack", "Check", "Watch"]
        );
    }
}
