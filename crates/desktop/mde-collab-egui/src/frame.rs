//! The persistent Communications **frame**: the spaces rail (left), the mode
//! tabs (top), and the call bar (bottom). Every mode body renders inside it, so
//! the frame is what makes the surface feel like one place — the rail is the
//! selection key, the tabs switch the per-space view, and the call bar is pinned
//! and survives every mode/space switch (spec §1).
//!
//! PLATFORM-INTERFACES Q19 — the rail is the shared
//! [`nav_chrome::Sidebar`](mde_egui::nav_chrome::Sidebar) (a pure
//! [`rail_row_model`] translated into rows, the unread badges painted over the
//! registered row rects). The mode tabs and the call bar stay custom by design:
//! the tabs are a fused icon+label strip wearing the surface's five-state
//! chrome motion, and the call bar is live call state with Auto-Mode sizing —
//! neither is a plain list or a plain title bar.

use mde_egui::egui;
use mde_egui::nav_chrome::{Sidebar, SidebarRow, SidebarSection};
use mde_egui::Style;

use mde_collab_types::{
    CallId, CallKind, CallParticipantState, CallView, CollabCommand, SpaceDirectory, SpaceId,
};

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

/// The one id salt the spaces rail renders under — shared by the rail, its
/// unread-badge overlay pass, and the tests, so [`Sidebar::row_id`] stays
/// deterministic.
pub(crate) const RAIL_SIDEBAR_SALT: &str = "collab-rail";

/// The rail's pure row model: one row per
/// [`SpaceSummary`](mde_collab_types::SpaceSummary) in directory order — the
/// selection id, the drawn name, the kind's Carbon glyph, and the unread count
/// the overlay badge paints. Pure, so the tests assert the model (the U19
/// Settings-sidebar idiom), and the render below only translates it.
pub(crate) fn rail_row_model(
    directory: &SpaceDirectory,
) -> Vec<(SpaceId, &str, &'static str, u32)> {
    directory
        .spaces
        .iter()
        .map(|s| {
            (
                s.id,
                s.name.as_str(),
                icons::space_kind_icon(s.kind),
                s.unread,
            )
        })
        .collect()
}

impl CommunicationsSurface {
    /// The persistent spaces rail — the shared Q19 [`Sidebar`]
    /// (PLATFORM-INTERFACES Q19): one selectable row per
    /// [`SpaceSummary`](mde_collab_types::SpaceSummary) with the kind's Carbon
    /// glyph, under a "Spaces" section header, with click / arrow-walk / Enter
    /// all routing through the one [`select_space`](Self::select_space) seam.
    /// The live unread badges ride the **overlay bridge**: each row's count is
    /// painted into the rect the row registered under [`Sidebar::row_id`]
    /// (the U19 glyph-pass idiom), so the shared component stays generic and
    /// the rail keeps its live fact. Selecting a row is the key every other
    /// pane keys off.
    pub(crate) fn rail(
        &mut self,
        ui: &mut egui::Ui,
        data: &dyn crate::CollabData,
        _sink: &mut crate::CommandSink,
    ) {
        let directory = data.space_directory();
        if directory.spaces.is_empty() {
            ui.label(
                egui::RichText::new("Spaces")
                    .size(Style::SMALL)
                    .strong()
                    .color(Style::TEXT_DIM),
            );
            ui.add_space(Style::SP_XS);
            ui.label(egui::RichText::new("No spaces yet").color(Style::TEXT_DIM));
            return;
        }

        let model = rail_row_model(directory);
        let rows: Vec<SidebarRow<'_, SpaceId>> = model
            .iter()
            .map(|(id, name, glyph, _)| SidebarRow::new(*id, name).with_icon(glyph))
            .collect();
        let sections = [SidebarSection {
            header: Some("Spaces"),
            rows: rows.as_slice(),
        }];
        // `ui()` defaults the selection to the first rail row before the rail
        // renders, so a non-empty directory always has a selected space; the
        // fallback only guards a stale selection over a changed directory.
        let selected = self.selected_space().unwrap_or(model[0].0);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if let Some(picked) = Sidebar::show(ui, RAIL_SIDEBAR_SALT, &sections, &selected) {
                    self.select_space(picked);
                }
                // The unread-badge overlay pass: paint each row's live count
                // into the slot its row registered just above, read back
                // through the Sidebar's deterministic row ids.
                for (index, (_, _, _, unread)) in model.iter().enumerate() {
                    if *unread == 0 {
                        continue;
                    }
                    let Some(row) = ui
                        .ctx()
                        .read_response(Sidebar::row_id(RAIL_SIDEBAR_SALT, index))
                    else {
                        continue;
                    };
                    paint_unread_badge(ui, row.rect, *unread);
                }
            });
    }

    /// The per-space mode tabs. Every tab is selectable and implemented; the
    /// selected tab is accent-tinted, the rest read as dim-but-live.
    pub(crate) fn mode_tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for mode in Mode::TABS {
                let selected = self.mode() == mode;
                // Every mode is implemented, so the tint reads accent-when-selected
                // else dim-but-live; the tab itself carries the shared hover / press /
                // focus motion through the one interactive-cell helper.
                let tint = if selected {
                    Style::ACCENT
                } else {
                    Style::TEXT_DIM
                };
                let label_color = if selected {
                    Style::TEXT_STRONG
                } else {
                    Style::TEXT
                };
                let clicked = crate::anim::interactive_cell(
                    ui,
                    ("collab-tab", mode.label()),
                    selected,
                    false,
                    |ui| {
                        icons::icon(ui, icons::mode_icon(mode), Style::SP_M, tint);
                        ui.label(egui::RichText::new(mode.label()).color(label_color));
                    },
                )
                .clicked();
                if clicked {
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
            // A call appearing in the persistent bar fades up on the shared list
            // entrance (lock #4) rather than popping in — the bar itself stays pinned.
            for (i, call) in calls.iter().enumerate() {
                crate::anim::entrance(ui, "call", call.call, i, |ui| {
                    self.call_row(ui, data, sink, call);
                });
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

/// A small unread-count badge (accent fill, bright count, capped at `99+`)
/// painted into a rail row's registered rect — the overlay bridge over the
/// shared [`Sidebar`] row (PLATFORM-INTERFACES Q19), right-aligned inside the
/// row's selection plate. Pure paint: layout stays the shared component's.
fn paint_unread_badge(ui: &egui::Ui, row_rect: egui::Rect, unread: u32) {
    let text = if unread > 99 {
        "99+".to_owned()
    } else {
        unread.to_string()
    };
    let painter = ui.painter();
    let galley = painter.layout_no_wrap(
        text,
        egui::FontId::proportional(Style::SMALL),
        Style::TEXT_STRONG,
    );
    // Mirror the shared row's own plate inset so the pill hugs the same edge
    // the selection plate does.
    let plate = row_rect.shrink2(egui::vec2(Style::SP_XS, Style::STROKE_HAIRLINE));
    let size = galley.size() + egui::vec2(Style::SP_XS * 2.0, Style::SP_XS);
    let rect = egui::Rect::from_min_size(
        egui::pos2(
            plate.right() - Style::SP_S - size.x,
            plate.center().y - size.y * 0.5,
        ),
        size,
    );
    if !ui.is_rect_visible(rect) {
        return;
    }
    painter.rect_filled(rect, Style::RADIUS_S, Style::ACCENT);
    painter.galley(
        egui::pos2(
            rect.center().x - galley.size().x * 0.5,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        Style::TEXT_STRONG,
    );
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
