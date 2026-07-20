//! Alerts mode — the fleet-wide alert inbox (WL-FUNC-011).
//!
//! Renders the [`AlertInbox`](mde_collab_types::AlertInbox) projection: one card
//! per folded alert with its severity band, originating source, headline, key
//! detail fields, and honest state (acknowledged / snoozed). The worker folds the
//! truthful Bus alert lanes into
//! [`AlertRaised`](mde_collab_types::CollabEventKind::AlertRaised) events — the
//! emitters keep publishing their own events unchanged; this surface only reads
//! the rollup and emits typed [`CollabCommand`]s:
//!
//! * **Acknowledge** ([`AckAlert`](CollabCommand::AckAlert)) / **Snooze**
//!   ([`SnoozeAlert`](CollabCommand::SnoozeAlert)).
//! * **Run action** ([`RunAlertAction`](CollabCommand::RunAlertAction)) — a safe
//!   inline action fires immediately; a **destructive** one is gated behind an
//!   explicit *arm → confirm* step, mirroring the core's `DestructiveNotArmed`
//!   guard (a mis-armed destructive action is refused, not silently run).
//! * **Mute source** ([`SetAlertMute`](CollabCommand::SetAlertMute)), **severity
//!   threshold** ([`SetSeverityThreshold`](CollabCommand::SetSeverityThreshold)),
//!   and **Do-Not-Disturb** ([`SetDoNotDisturb`](CollabCommand::SetDoNotDisturb))
//!   — the per-seat notification preferences. A hushed alert (muted source, below
//!   threshold, or below Critical under DND) is **dimmed**, never hidden: a
//!   silenced alert is still a real fact (§7).

use mde_egui::egui;
use mde_egui::Style;

use mde_collab_types::{AlertActionKind, AlertView, CollabCommand, EventId, Severity, SpaceId};

use crate::{icons, CommunicationsSurface};

/// How long a snooze hushes an alert — a fixed hour from the injected now.
const SNOOZE_MS: i64 = 60 * 60 * 1_000;

impl CommunicationsSurface {
    /// Render Alerts mode: the notification-preference bar (severity threshold +
    /// DND), then the fleet-wide inbox, newest-first.
    pub(crate) fn alerts_body(
        &mut self,
        ui: &mut egui::Ui,
        data: &dyn crate::CollabData,
        sink: &mut crate::CommandSink,
    ) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Alerts")
                    .strong()
                    .color(Style::TEXT_STRONG),
            );
            ui.label(
                egui::RichText::new("fleet inbox")
                    .small()
                    .color(Style::TEXT_DIM),
            );
        });
        self.alert_pref_bar(ui, sink);
        ui.separator();

        let now = data.now_unix_ms();
        match data.alert_inbox() {
            Some(inbox) if !inbox.alerts.is_empty() => {
                egui::ScrollArea::vertical()
                    .id_salt("collab-alerts")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for view in &inbox.alerts {
                            self.alert_card(ui, sink, view, now);
                            ui.add_space(Style::SP_XS);
                        }
                    });
            }
            _ => {
                ui.label(egui::RichText::new("No alerts.").color(Style::TEXT_DIM));
                ui.label(
                    egui::RichText::new("Truthful alerts folded from the mesh land here.")
                        .small()
                        .color(Style::TEXT_DIM),
                );
            }
        }
    }

    /// The notification-preference bar: the severity-threshold chips + the DND
    /// toggle (both mirrored out as commands, held locally as view state).
    fn alert_pref_bar(&mut self, ui: &mut egui::Ui, sink: &mut crate::CommandSink) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Ring at")
                    .small()
                    .color(Style::TEXT_DIM),
            );
            for level in [Severity::Info, Severity::Warning, Severity::Critical] {
                let selected = self.alert_threshold == level;
                if ui
                    .selectable_label(
                        selected,
                        egui::RichText::new(severity_label(level)).color(if selected {
                            severity_color(level)
                        } else {
                            Style::TEXT_DIM
                        }),
                    )
                    .clicked()
                {
                    self.set_severity_threshold(sink, level);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (tint, hint) = if self.alert_dnd {
                    (Style::WARN, "Do-Not-Disturb on — only Critical rings")
                } else {
                    (Style::TEXT_DIM, "Do-Not-Disturb off")
                };
                if icons::icon_button(ui, icons::ALERT_DND, Style::SP_M, tint, hint).clicked() {
                    self.set_dnd(sink, !self.alert_dnd);
                }
            });
        });
    }

    /// One alert card: the severity glyph + headline + source + state, the key
    /// detail fields, and the action row (ack / snooze / mute / typed actions).
    fn alert_card(
        &mut self,
        ui: &mut egui::Ui,
        sink: &mut crate::CommandSink,
        view: &AlertView,
        now_unix_ms: i64,
    ) {
        let hushed = self.alert_hushed(view);
        let sev = view.alert.severity;
        mde_egui::card().show(ui, |ui| {
            ui.horizontal(|ui| {
                let glyph_tint = if hushed {
                    Style::BORDER
                } else {
                    severity_color(sev)
                };
                icons::icon(ui, icons::severity_icon(sev), Style::SP_M, glyph_tint);
                ui.label(
                    egui::RichText::new(&view.alert.headline)
                        .strong()
                        .color(if hushed {
                            Style::TEXT_DIM
                        } else {
                            Style::TEXT_STRONG
                        }),
                );
                ui.label(
                    egui::RichText::new(format!("· {}", view.alert.source))
                        .small()
                        .color(Style::TEXT_DIM),
                );
                // Honest state chips — never a faked "handled".
                if view.acknowledged {
                    ui.label(egui::RichText::new("acknowledged").small().color(Style::OK));
                }
                if view.snoozed_until_unix_ms.is_some() {
                    ui.label(egui::RichText::new("snoozed").small().color(Style::WARN));
                }
                if hushed {
                    ui.label(egui::RichText::new("hushed").small().color(Style::TEXT_DIM));
                }
            });

            // A few key detail fields (the folded structured payload).
            for (k, v) in view.alert.fields.iter().take(4) {
                ui.label(
                    egui::RichText::new(format!("{k}: {v}"))
                        .small()
                        .color(Style::TEXT_DIM),
                );
            }

            self.alert_actions(ui, sink, view, now_unix_ms);
        });
    }

    /// The alert action row: ack (until acknowledged), snooze, mute-source, and
    /// each typed inline action (safe fires immediately; destructive arms first).
    fn alert_actions(
        &mut self,
        ui: &mut egui::Ui,
        sink: &mut crate::CommandSink,
        view: &AlertView,
        now_unix_ms: i64,
    ) {
        let space = view.space;
        let alert = view.event_id;
        ui.horizontal(|ui| {
            if !view.acknowledged
                && icons::icon_button(ui, icons::ALERT_ACK, Style::SP_M, Style::OK, "Acknowledge")
                    .clicked()
            {
                self.acknowledge_alert(sink, space, alert);
            }
            if icons::icon_button(
                ui,
                icons::ALERT_SNOOZE,
                Style::SP_M,
                Style::TEXT_DIM,
                "Snooze 1h",
            )
            .clicked()
            {
                self.snooze_alert(sink, space, alert, now_unix_ms + SNOOZE_MS);
            }
            let muted = self.alert_muted_sources.contains(&view.alert.source);
            let (mute_tint, mute_hint) = if muted {
                (Style::WARN, "Unmute this source")
            } else {
                (Style::TEXT_DIM, "Mute this source")
            };
            if icons::icon_button(ui, icons::ALERT_MUTE, Style::SP_M, mute_tint, mute_hint)
                .clicked()
            {
                self.set_alert_mute(sink, view.alert.source.clone(), !muted);
            }

            for action in &view.alert.actions {
                let arming_this = matches!(
                    &self.alert_arming,
                    Some((a, id)) if *a == alert && id == &action.id
                );
                if matches!(action.kind, AlertActionKind::Destructive) {
                    if arming_this {
                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new(format!("Confirm {}", action.label))
                                    .color(Style::DANGER),
                            ))
                            .clicked()
                        {
                            self.confirm_alert_action(sink, space);
                        }
                        if ui.button("Cancel").clicked() {
                            self.cancel_alert_action();
                        }
                    } else if icons::icon_button(
                        ui,
                        icons::ALERT_DESTRUCTIVE,
                        Style::SP_M,
                        Style::DANGER,
                        &format!("{} (destructive — arms first)", action.label),
                    )
                    .clicked()
                    {
                        self.arm_alert_action(alert, action.id.clone());
                    }
                } else if icons::icon_button(
                    ui,
                    icons::ALERT_RUN,
                    Style::SP_M,
                    Style::ACCENT,
                    &action.label,
                )
                .clicked()
                {
                    // Safe / ack / snooze inline actions fire immediately.
                    self.run_alert_action(sink, space, alert, action.id.clone(), false);
                }
            }
        });
    }

    /// Whether `view` is currently hushed for the local seat — a muted source,
    /// below the severity threshold, or (under DND) below Critical. A hushed alert
    /// is dimmed, never hidden (§7).
    #[must_use]
    pub(crate) fn alert_hushed(&self, view: &AlertView) -> bool {
        let sev = view.alert.severity;
        self.alert_muted_sources.contains(&view.alert.source)
            || sev < self.alert_threshold
            || (self.alert_dnd && sev < Severity::Critical)
    }

    // ── testable command seams (the UI above drives these same methods) ──────

    /// Emit [`AckAlert`](CollabCommand::AckAlert).
    pub(crate) fn acknowledge_alert(
        &self,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        alert: EventId,
    ) {
        sink.emit(CollabCommand::AckAlert { space, alert });
    }

    /// Emit [`SnoozeAlert`](CollabCommand::SnoozeAlert) until `until_unix_ms`.
    pub(crate) fn snooze_alert(
        &self,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        alert: EventId,
        until_unix_ms: i64,
    ) {
        sink.emit(CollabCommand::SnoozeAlert {
            space,
            alert,
            until_unix_ms,
        });
    }

    /// Emit [`RunAlertAction`](CollabCommand::RunAlertAction) — `armed` must be
    /// `true` for a destructive action or the worker refuses it.
    pub(crate) fn run_alert_action(
        &self,
        sink: &mut crate::CommandSink,
        space: SpaceId,
        alert: EventId,
        action_id: String,
        armed: bool,
    ) {
        sink.emit(CollabCommand::RunAlertAction {
            space,
            alert,
            action_id,
            armed,
        });
    }

    /// Mute (or unmute) `source` for the local seat: update the view state and
    /// mirror out [`SetAlertMute`](CollabCommand::SetAlertMute).
    pub(crate) fn set_alert_mute(
        &mut self,
        sink: &mut crate::CommandSink,
        source: String,
        muted: bool,
    ) {
        if muted {
            self.alert_muted_sources.insert(source.clone());
        } else {
            self.alert_muted_sources.remove(&source);
        }
        sink.emit(CollabCommand::SetAlertMute { source, muted });
    }

    /// Set the local seat's severity threshold + mirror out
    /// [`SetSeverityThreshold`](CollabCommand::SetSeverityThreshold).
    pub(crate) fn set_severity_threshold(
        &mut self,
        sink: &mut crate::CommandSink,
        threshold: Severity,
    ) {
        self.alert_threshold = threshold;
        sink.emit(CollabCommand::SetSeverityThreshold { threshold });
    }

    /// Toggle Do-Not-Disturb + mirror out
    /// [`SetDoNotDisturb`](CollabCommand::SetDoNotDisturb).
    pub(crate) fn set_dnd(&mut self, sink: &mut crate::CommandSink, enabled: bool) {
        self.alert_dnd = enabled;
        sink.emit(CollabCommand::SetDoNotDisturb { enabled });
    }

    /// Arm the destructive action `action_id` on `alert` — the first step of the
    /// two-step gate (the confirm click then fires it armed).
    pub(crate) fn arm_alert_action(&mut self, alert: EventId, action_id: String) {
        self.alert_arming = Some((alert, action_id));
    }

    /// Fire the pending armed destructive action into `space`, if one is armed.
    /// Emits [`RunAlertAction`](CollabCommand::RunAlertAction) with `armed: true`.
    /// Returns `true` when it fired.
    pub(crate) fn confirm_alert_action(
        &mut self,
        sink: &mut crate::CommandSink,
        space: SpaceId,
    ) -> bool {
        let Some((alert, action_id)) = self.alert_arming.take() else {
            return false;
        };
        self.run_alert_action(sink, space, alert, action_id, true);
        true
    }

    /// Disarm the pending destructive action without firing it.
    pub(crate) fn cancel_alert_action(&mut self) {
        self.alert_arming = None;
    }
}

/// The short label for a severity band.
const fn severity_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "Info",
        Severity::Warning => "Warning",
        Severity::Critical => "Critical",
    }
}

/// The Carbon tint for a severity band.
const fn severity_color(severity: Severity) -> egui::Color32 {
    match severity {
        Severity::Info => Style::ACCENT,
        Severity::Warning => Style::WARN,
        Severity::Critical => Style::DANGER,
    }
}
