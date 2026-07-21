//! The persistent Communications **frame**: the spaces rail (left), the mode
//! tabs (top), and the call bar (bottom). Every mode body renders inside it, so
//! the frame is what makes the surface feel like one place — the rail is the
//! selection key, the tabs switch the per-space view, and the call bar is pinned
//! and survives every mode/space switch (spec §1).

use mde_egui::egui;
use mde_egui::Style;

use mde_collab_types::{CallId, CallKind, CallParticipantState, CallView, CollabCommand};

use crate::{icons, CommunicationsSurface, Mode};

/// The spaces rail width — a fixed, non-resizable gutter (~192 px on the 8 px
/// grid) wide enough for an icon + a space name + an unread badge.
pub const RAIL_W: f32 = Style::SP_XL * 6.0;

/// The rail's raised (layer-01) frame.
pub fn rail_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(Style::LAYER_01)
        .inner_margin(Style::SP_S)
}

/// A top/bottom chrome bar frame — a raised strip with the refined toolbar inset.
pub fn bar_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(Style::LAYER_01)
        .inner_margin(Style::toolbar_margin())
}

/// The mode body frame — the deep page ground the content rests on.
pub fn body_frame() -> egui::Frame {
    egui::Frame::NONE.fill(Style::BG).inner_margin(Style::SP_M)
}

impl CommunicationsSurface {
    /// The persistent spaces rail: one selectable row per
    /// [`SpaceSummary`](mde_collab_types::SpaceSummary), with a kind glyph, the
    /// name, and an unread badge. Selecting a row is the key every other pane
    /// keys off.
    pub(crate) fn rail(
        &mut self,
        ui: &mut egui::Ui,
        data: &dyn crate::CollabData,
        _sink: &mut crate::CommandSink,
    ) {
        ui.label(
            egui::RichText::new("Spaces")
                .size(Style::SMALL)
                .strong()
                .color(Style::TEXT_DIM),
        );
        ui.add_space(Style::SP_XS);

        let directory = data.space_directory();
        if directory.spaces.is_empty() {
            ui.label(egui::RichText::new("No spaces yet").color(Style::TEXT_DIM));
            return;
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for space in &directory.spaces {
                    let selected = self.selected_space() == Some(space.id);
                    ui.horizontal(|ui| {
                        let tint = if selected {
                            Style::ACCENT
                        } else {
                            Style::TEXT_DIM
                        };
                        icons::icon(ui, icons::space_kind_icon(space.kind), Style::SP_M, tint);
                        let name = egui::RichText::new(&space.name).color(if selected {
                            Style::TEXT_STRONG
                        } else {
                            Style::TEXT
                        });
                        if ui.selectable_label(selected, name).clicked() {
                            self.select_space(space.id);
                        }
                        if space.unread > 0 {
                            unread_badge(ui, space.unread);
                        }
                    });
                }
            });
    }

    /// The per-space mode tabs. Every tab is selectable and implemented; the
    /// selected tab is accent-tinted, the rest read as dim-but-live.
    pub(crate) fn mode_tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for mode in Mode::TABS {
                let selected = self.mode() == mode;
                let tint = if selected {
                    Style::ACCENT
                } else if mode.is_implemented() {
                    Style::TEXT_DIM
                } else {
                    Style::BORDER
                };
                icons::icon(ui, icons::mode_icon(mode), Style::SP_M, tint);
                let label = egui::RichText::new(mode.label()).color(if selected {
                    Style::TEXT_STRONG
                } else if mode.is_implemented() {
                    Style::TEXT
                } else {
                    Style::TEXT_DIM
                });
                if ui.selectable_label(selected, label).clicked() {
                    self.set_mode(mode);
                }
                ui.add_space(Style::SP_XS);
            }
        });
    }

    /// The persistent call bar: renders the
    /// [`CallState`](mde_collab_types::CallState) read model. Empty → an honest
    /// "no active call" strip with a Start-call affordance for the selected
    /// space; active → one row per call with controls wired to the call commands
    /// (the media plane lands later, but the intent is real today).
    pub(crate) fn call_bar(
        &mut self,
        ui: &mut egui::Ui,
        data: &dyn crate::CollabData,
        sink: &mut crate::CommandSink,
    ) {
        // Auto Mode (Car Mode): the call bar is the driver's always-there call
        // control, so on the Ford SYNC 3 car dash its glyphs + type read larger.
        let car = crate::car_mode(ui);
        let glyph = if car { Style::SP_L } else { Style::SP_M };
        let calls = data.call_state().active.clone();
        ui.horizontal(|ui| {
            icons::icon(ui, icons::CALL_UNMUTE, glyph, Style::TEXT_DIM);
            ui.add_space(Style::SP_XS);
            if calls.is_empty() {
                let none = egui::RichText::new("No active call").color(Style::TEXT_DIM);
                ui.label(if car { none.size(Style::TITLE) } else { none });
                if let Some(space) = self.selected_space() {
                    ui.add_space(Style::SP_S);
                    if icons::icon_button(
                        ui,
                        icons::CALL_START,
                        glyph,
                        Style::OK,
                        "Start audio call",
                    )
                    .clicked()
                    {
                        sink.emit(CollabCommand::StartCall {
                            space,
                            call: CallId::new(),
                            kind: CallKind::Audio,
                        });
                    }
                }
                return;
            }
            for call in &calls {
                self.call_row(ui, data, sink, call);
            }
        });
    }

    /// One active-call row inside the call bar.
    fn call_row(
        &self,
        ui: &mut egui::Ui,
        data: &dyn crate::CollabData,
        sink: &mut crate::CommandSink,
        call: &CallView,
    ) {
        // Auto Mode (Car Mode): a driver's in-call controls read larger — bigger
        // kind label + participant count and larger, easier-to-hit control glyphs.
        let car = crate::car_mode(ui);
        let ctrl = if car { Style::SP_L } else { Style::SP_M };
        let me = data.me();
        let mine = call.participants.iter().find(|p| &p.actor == me);
        let connected = call
            .participants
            .iter()
            .filter(|p| p.state == CallParticipantState::Connected)
            .count();

        let kind = egui::RichText::new(call_kind_label(call.kind))
            .strong()
            .color(Style::TEXT);
        ui.label(if car { kind.size(Style::TITLE) } else { kind });
        let count = egui::RichText::new(format!("{connected} on call")).color(Style::TEXT_DIM);
        ui.label(if car {
            count.size(Style::BODY)
        } else {
            count.small()
        });

        if matches!(mine.map(|p| p.state), Some(CallParticipantState::Ringing)) {
            if icons::icon_button(ui, icons::CALL_ANSWER, ctrl, Style::OK, "Answer").clicked() {
                sink.emit(CollabCommand::AnswerCall { call: call.call });
            }
            if icons::icon_button(ui, icons::CALL_DECLINE, ctrl, Style::DANGER, "Decline").clicked()
            {
                sink.emit(CollabCommand::DeclineCall { call: call.call });
            }
            return;
        }

        if let Some(participant) = mine {
            let (glyph, hint) = if participant.muted {
                (icons::CALL_UNMUTE, "Unmute")
            } else {
                (icons::CALL_MUTE, "Mute")
            };
            if icons::icon_button(ui, glyph, ctrl, Style::TEXT_DIM, hint).clicked() {
                sink.emit(CollabCommand::SetCallMuted {
                    call: call.call,
                    muted: !participant.muted,
                });
            }
        }
        if icons::icon_button(ui, icons::CALL_HANGUP, ctrl, Style::DANGER, "Hang up").clicked() {
            sink.emit(CollabCommand::HangUpCall { call: call.call });
        }
        ui.add_space(Style::SP_S);
    }
}

/// A small unread-count badge (accent fill, bright count) painted after a rail
/// row's name. Caps the display at `99+`.
fn unread_badge(ui: &mut egui::Ui, unread: u32) {
    let text = if unread > 99 {
        "99+".to_owned()
    } else {
        unread.to_string()
    };
    egui::Frame::NONE
        .fill(Style::ACCENT)
        .corner_radius(mde_egui::corner(Style::RADIUS_S))
        .inner_margin(egui::Margin::symmetric(Style::SP_XS as i8, 0))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(text)
                    .small()
                    .strong()
                    .color(Style::TEXT_STRONG),
            );
        });
}

/// A human label for a call's [`CallKind`]. Shared with the Calls mode roster.
pub(crate) const fn call_kind_label(kind: CallKind) -> &'static str {
    match kind {
        CallKind::Audio => "Audio call",
        CallKind::Video => "Video call",
        CallKind::Screen => "Screen share",
        CallKind::CoEdit => "Co-edit",
        CallKind::RemoteDesktop => "Remote desktop",
    }
}
