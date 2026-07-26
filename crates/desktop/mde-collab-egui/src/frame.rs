//! The persistent Mesh Teams **frame**: the app rail (far left), Teams +
//! Channels rail, channel header/tabs, and call bar. Every body renders inside
//! it, so the frame is what makes the surface feel like one place — the channel
//! rail is the selection key, the app rail switches global tools, the Teams tab
//! strip switches Posts / Files / Calls / Tasks for the selected channel, and the call
//! bar is pinned and survives every app/channel switch (spec §1).
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
    CallId, CallKind, CallParticipantState, CallView, CollabCommand, DiscordBridgeBoard,
    DiscordBridgeConfigStatus, DiscordBridgeFlowStatus, DiscordBridgeProvenance,
    DiscordBridgeProvenanceSource, DiscordBridgeView, SpaceDirectory, SpaceId, SpaceKind,
    SpaceRole, SpaceSummary,
};

use crate::{icons, icons::CommsHoverExt, ChannelTab, CommunicationsSurface, MeshTeamsApp};

/// The Teams-style app rail width.
pub const APP_RAIL_W: f32 = Style::SP_XL * 2.25;
/// The Teams + Channels rail width — a fixed, non-resizable gutter wide enough
/// for an icon + a channel name + an unread badge.
pub const CHANNEL_RAIL_W: f32 = Style::SP_XL * 6.0;
/// The selected-channel details rail width.
pub const DETAILS_W: f32 = Style::SP_XL * 6.25;

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

/// Pure app-rail model in the operator-locked WL-UX-010 order.
#[must_use]
pub(crate) fn app_rail_model() -> Vec<(MeshTeamsApp, &'static str, &'static str)> {
    MeshTeamsApp::ALL
        .iter()
        .map(|app| (*app, app.label(), icons::app_icon(*app)))
        .collect()
}

/// Return the selected space only while it is still present in the current
/// directory. The read model can advance between frames (for example, after a
/// membership removal), while the surface deliberately retains view state; a
/// stale selection must not become an actionable call target.
#[must_use]
pub(crate) fn selected_space_in_directory(
    selected: Option<SpaceId>,
    directory: &SpaceDirectory,
) -> Option<SpaceId> {
    selected.filter(|selected| directory.spaces.iter().any(|space| space.id == *selected))
}

fn selected_space_summary(
    selected: Option<SpaceId>,
    directory: &SpaceDirectory,
) -> Option<&SpaceSummary> {
    selected.and_then(|selected| directory.spaces.iter().find(|space| space.id == selected))
}

/// Pure UI row for Discord bridge state.
///
/// It is deliberately derived from a read model or from the explicit
/// "unconfigured" fallback. It never names a fake server and never calls a
/// provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscordBridgeRowModel {
    pub(crate) label: String,
    pub(crate) status: &'static str,
    pub(crate) inbound: &'static str,
    pub(crate) outbound: &'static str,
    pub(crate) provenance: String,
    pub(crate) detail: Option<String>,
}

/// Build Settings-pane Discord bridge rows. Missing/empty read models become one
/// honest unconfigured row so the operator sees the seam without a fake server.
#[must_use]
pub(crate) fn discord_bridge_rows_for_settings(
    board: Option<&DiscordBridgeBoard>,
) -> Vec<DiscordBridgeRowModel> {
    match board {
        Some(board) if !board.bridges.is_empty() => {
            board.bridges.iter().map(discord_bridge_row_model).collect()
        }
        _ => vec![discord_bridge_unconfigured_row()],
    }
}

/// Build selected-channel Discord bridge rows. Rows scoped to another channel do
/// not leak into this channel's Details pane; sparse/missing projections render
/// an honest unconfigured row.
#[must_use]
pub(crate) fn discord_bridge_rows_for_space(
    space: SpaceId,
    board: Option<&DiscordBridgeBoard>,
) -> Vec<DiscordBridgeRowModel> {
    let rows: Vec<_> = board
        .into_iter()
        .flat_map(|board| board.bridges.iter())
        .filter(|row| row.space == Some(space))
        .map(discord_bridge_row_model)
        .collect();
    if rows.is_empty() {
        vec![discord_bridge_unconfigured_row()]
    } else {
        rows
    }
}

fn discord_bridge_unconfigured_row() -> DiscordBridgeRowModel {
    DiscordBridgeRowModel {
        label: "Discord bridge".to_owned(),
        status: discord_bridge_status_label(DiscordBridgeConfigStatus::Unconfigured),
        inbound: discord_bridge_flow_label(DiscordBridgeFlowStatus::NotConfigured),
        outbound: discord_bridge_flow_label(DiscordBridgeFlowStatus::NotConfigured),
        provenance: discord_bridge_provenance_label(&DiscordBridgeProvenance {
            source: DiscordBridgeProvenanceSource::None,
            authority: None,
            observed_by: None,
            config_digest: None,
        }),
        detail: Some(
            "No Discord bridge configuration has been projected for this channel.".to_owned(),
        ),
    }
}

fn discord_bridge_row_model(row: &DiscordBridgeView) -> DiscordBridgeRowModel {
    DiscordBridgeRowModel {
        label: row.label.clone(),
        status: discord_bridge_status_label(row.status),
        inbound: discord_bridge_flow_label(row.inbound),
        outbound: discord_bridge_flow_label(row.outbound),
        provenance: discord_bridge_provenance_label(&row.provenance),
        detail: row.detail.clone(),
    }
}

const fn discord_bridge_status_label(status: DiscordBridgeConfigStatus) -> &'static str {
    match status {
        DiscordBridgeConfigStatus::Unconfigured => "Unconfigured",
        DiscordBridgeConfigStatus::ProviderUnavailable => "Provider unavailable",
        DiscordBridgeConfigStatus::Configured => "Configured",
    }
}

const fn discord_bridge_flow_label(status: DiscordBridgeFlowStatus) -> &'static str {
    match status {
        DiscordBridgeFlowStatus::NotConfigured => "Not configured",
        DiscordBridgeFlowStatus::ProviderUnavailable => "Provider unavailable",
        DiscordBridgeFlowStatus::Degraded => "Degraded",
        DiscordBridgeFlowStatus::Ready => "Ready",
    }
}

fn discord_bridge_provenance_label(provenance: &DiscordBridgeProvenance) -> String {
    let mut parts = vec![match provenance.source {
        DiscordBridgeProvenanceSource::None => "No bridge projection".to_owned(),
        DiscordBridgeProvenanceSource::OperatorConfig => "Operator config".to_owned(),
        DiscordBridgeProvenanceSource::WorkerState => "Bridge worker state".to_owned(),
        DiscordBridgeProvenanceSource::ProviderAdapter => "Provider adapter".to_owned(),
    }];
    if let Some(authority) = provenance.authority.as_deref() {
        parts.push(format!("authority {authority}"));
    }
    if let Some(observed_by) = provenance.observed_by.as_deref() {
        parts.push(format!("observed by {observed_by}"));
    }
    if let Some(digest) = provenance.config_digest.as_deref() {
        parts.push(format!("config {digest}"));
    }
    parts.join(" · ")
}

/// Pure model for the selected-channel Details pane.
///
/// Every count is read from the existing retained projections. Missing
/// projections are rendered as zero/none — an honest "not projected yet" state,
/// not provider-shaped fixture data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChannelDetailsModel {
    pub(crate) space: SpaceId,
    pub(crate) name: String,
    pub(crate) kind: &'static str,
    pub(crate) role: &'static str,
    pub(crate) members: u32,
    pub(crate) unread: u32,
    pub(crate) last_activity: String,
    pub(crate) messages: usize,
    pub(crate) tasks: usize,
    pub(crate) files: usize,
    pub(crate) transfers: usize,
    pub(crate) documents: usize,
    pub(crate) active_calls: usize,
    pub(crate) clips: usize,
    pub(crate) discord_bridges: Vec<DiscordBridgeRowModel>,
}

/// Build the Details pane model for the currently selected channel, if that
/// selection still exists in the live directory.
#[must_use]
pub(crate) fn channel_details_model(
    selected: Option<SpaceId>,
    data: &dyn crate::CollabData,
) -> Option<ChannelDetailsModel> {
    let summary = selected_space_summary(selected, data.space_directory())?;
    let file_refs = data.file_references(summary.id);
    let files = file_refs.map_or(0, |refs| refs.files.len());
    let transfers = match (file_refs, data.transfer_jobs()) {
        (Some(refs), Some(jobs)) => jobs
            .jobs
            .iter()
            .filter(|job| refs.files.iter().any(|file| file.file == job.file))
            .count(),
        _ => 0,
    };

    Some(ChannelDetailsModel {
        space: summary.id,
        name: summary.name.clone(),
        kind: space_kind_label(summary.kind),
        role: space_role_label(summary.role),
        members: summary.members,
        unread: summary.unread,
        last_activity: summary.last_activity.to_string(),
        messages: data
            .conversation(summary.id)
            .map_or(0, |timeline| timeline.messages.len()),
        tasks: data
            .channel_tasks(summary.id)
            .map_or(0, |tasks| tasks.tasks.len()),
        files,
        transfers,
        documents: data
            .document_sessions(summary.id)
            .map_or(0, |sessions| sessions.sessions.len()),
        active_calls: data
            .call_state()
            .active
            .iter()
            .filter(|call| call.space == summary.id)
            .count(),
        clips: data
            .clipboard_lane(summary.id)
            .map_or(0, |lane| lane.items.len()),
        discord_bridges: discord_bridge_rows_for_space(summary.id, data.discord_bridge_board()),
    })
}

const fn space_kind_label(kind: SpaceKind) -> &'static str {
    match kind {
        SpaceKind::Direct => "Direct",
        SpaceKind::Team => "Team",
        SpaceKind::Incident => "Incident",
        SpaceKind::Project => "Project",
    }
}

const fn space_role_label(role: SpaceRole) -> &'static str {
    match role {
        SpaceRole::Owner => "Owner",
        SpaceRole::Member => "Member",
    }
}

const fn short_app_label(app: MeshTeamsApp) -> &'static str {
    match app {
        MeshTeamsApp::Activity => "Activity",
        MeshTeamsApp::Teams => "Teams",
        MeshTeamsApp::Calls => "Calls",
        MeshTeamsApp::Files => "Files",
        MeshTeamsApp::Alerts => "Alerts",
        MeshTeamsApp::Transfers => "Xfer",
        MeshTeamsApp::Clipboard => "Clip",
        MeshTeamsApp::Settings => "Settings",
    }
}

/// The rail's pure row model: one row per
/// [`SpaceSummary`](mde_collab_types::SpaceSummary) in directory order — the
/// selection id, the drawn name, the kind's Carbon glyph, and the unread count
/// the overlay badge paints. Pure, so the tests assert the model (the U19
/// Settings-sidebar idiom), and the render below only translates it.
pub(crate) fn rail_row_model(
    directory: &SpaceDirectory,
) -> Vec<(SpaceId, &str, &'static str, u32, u32)> {
    directory
        .spaces
        .iter()
        .map(|s| {
            (
                s.id,
                s.name.as_str(),
                icons::space_kind_icon(s.kind),
                s.unread,
                s.members,
            )
        })
        .collect()
}

impl CommunicationsSurface {
    /// The Teams-style app rail. It is separate from the channel list so
    /// Activity, Teams, Calls, Files, Alerts, Transfers, Clipboard, and Settings
    /// are always one click away.
    pub(crate) fn app_rail(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("Mesh")
                    .size(Style::SMALL)
                    .strong()
                    .color(Style::TEXT_DIM),
            );
        });
        ui.add_space(Style::SP_XS);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (app, label, glyph) in app_rail_model() {
                    let selected = self.app() == app;
                    let tint = if selected {
                        Style::ACCENT
                    } else {
                        Style::TEXT_DIM
                    };
                    let clicked = crate::anim::interactive_cell(
                        ui,
                        ("mesh-teams-app", label),
                        selected,
                        true,
                        |ui| {
                            ui.vertical_centered(|ui| {
                                icons::icon(ui, glyph, Style::SP_M, tint);
                                ui.label(egui::RichText::new(short_app_label(app)).small().color(
                                    if selected {
                                        Style::TEXT_STRONG
                                    } else {
                                        Style::TEXT_DIM
                                    },
                                ));
                            });
                        },
                    )
                    .comms_hover_text(label)
                    .clicked();
                    if clicked {
                        self.set_app(app);
                    }
                    ui.add_space(Style::SP_XS);
                }
            });
    }

    /// The persistent Teams + Channels rail — the shared Q19 [`Sidebar`]
    /// (PLATFORM-INTERFACES Q19): one selectable row per
    /// [`SpaceSummary`](mde_collab_types::SpaceSummary) with the kind's Carbon
    /// glyph, under a "Teams & Channels" section header, with click / arrow-walk / Enter
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
                egui::RichText::new("Teams & Channels")
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
            .map(|(id, name, glyph, _, _)| SidebarRow::new(*id, name).with_icon(glyph))
            .collect();
        let sections = [SidebarSection {
            header: Some("Teams & Channels"),
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
                for (index, (_, _, _, unread, _)) in model.iter().enumerate() {
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

    /// The top channel header. In the Teams app it exposes the required
    /// Posts/Files/Calls/Tasks tabs; in other app routes it becomes a compact title
    /// bar so the old eight-tab strip is not a second navigation model.
    pub(crate) fn channel_header(&mut self, ui: &mut egui::Ui, data: &dyn crate::CollabData) {
        let selected = selected_space_summary(self.selected_space(), data.space_directory());
        ui.horizontal_wrapped(|ui| {
            let title = if self.app() == MeshTeamsApp::Teams {
                selected.map_or("No channel selected", |space| space.name.as_str())
            } else {
                self.app().label()
            };
            ui.label(
                egui::RichText::new(title)
                    .strong()
                    .size(Style::TITLE)
                    .color(Style::TEXT_STRONG),
            );
            if let Some(space) = selected {
                ui.add_space(Style::SP_S);
                ui.label(
                    egui::RichText::new(format!("{} members", space.members))
                        .small()
                        .color(Style::TEXT_DIM),
                );
            }
            ui.add_space(Style::SP_M);
            if self.app() == MeshTeamsApp::Teams {
                for tab in ChannelTab::ALL {
                    let selected = self.channel_tab() == tab;
                    let clicked = crate::anim::interactive_cell(
                        ui,
                        ("mesh-teams-channel-tab", tab.label()),
                        selected,
                        false,
                        |ui| {
                            icons::icon(
                                ui,
                                icons::channel_tab_icon(tab),
                                Style::SP_M,
                                if selected {
                                    Style::ACCENT
                                } else {
                                    Style::TEXT_DIM
                                },
                            );
                            ui.label(egui::RichText::new(tab.label()).color(if selected {
                                Style::TEXT_STRONG
                            } else {
                                Style::TEXT
                            }));
                        },
                    )
                    .clicked();
                    if clicked {
                        self.set_channel_tab(tab);
                    }
                    ui.add_space(Style::SP_XS);
                }
            }
            if let Some(space) = selected {
                ui.add_space(Style::SP_M);
                channel_find_editor(ui, self, space.id);
            }
        });
    }

    /// The reserved right-side Details pane. It summarizes the selected channel
    /// from the same read models the bodies use, so it remains accurate while
    /// the operator switches Posts / Files / Calls / Tasks or hops between global apps.
    pub(crate) fn details_pane(&self, ui: &mut egui::Ui, data: &dyn crate::CollabData) {
        ui.label(
            egui::RichText::new("Details")
                .size(Style::BODY)
                .strong()
                .color(Style::TEXT_STRONG),
        );
        ui.add_space(Style::SP_XS);

        let Some(details) = channel_details_model(self.selected_space(), data) else {
            ui.label(egui::RichText::new("No channel selected").color(Style::TEXT_DIM));
            return;
        };

        ui.label(
            egui::RichText::new(details.name.as_str())
                .size(Style::TITLE)
                .strong()
                .color(Style::TEXT_STRONG),
        );
        ui.label(
            egui::RichText::new(format!("{} · {}", details.kind, details.role))
                .small()
                .color(Style::TEXT_DIM),
        );
        ui.add_space(Style::SP_S);
        ui.separator();
        ui.add_space(Style::SP_S);

        ui.label(
            egui::RichText::new("Channel facts")
                .small()
                .strong()
                .color(Style::TEXT_DIM),
        );
        detail_row(ui, "Members", details.members);
        detail_row(ui, "Unread", details.unread);
        detail_text_row(ui, "Activity clock", details.last_activity.as_str());
        ui.add_space(Style::SP_S);

        ui.label(
            egui::RichText::new("Live projections")
                .small()
                .strong()
                .color(Style::TEXT_DIM),
        );
        detail_row(ui, "Messages", details.messages);
        detail_row(ui, "Tasks", details.tasks);
        detail_row(ui, "Files", details.files);
        detail_row(ui, "Transfers", details.transfers);
        detail_row(ui, "Documents", details.documents);
        detail_row(ui, "Active calls", details.active_calls);
        detail_row(ui, "Clip items", details.clips);
        ui.add_space(Style::SP_S);

        ui.label(
            egui::RichText::new("Discord bridge")
                .small()
                .strong()
                .color(Style::TEXT_DIM),
        );
        for bridge in &details.discord_bridges {
            discord_bridge_detail_row(ui, bridge);
        }
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
        let selected_space =
            selected_space_in_directory(self.selected_space(), data.space_directory());
        ui.horizontal(|ui| {
            icons::icon(ui, icons::CALL_UNMUTE, glyph, Style::TEXT_DIM);
            ui.add_space(Style::SP_XS);
            if calls.is_empty() {
                let none = egui::RichText::new("No active call").color(Style::TEXT_DIM);
                ui.label(if car { none.size(Style::TITLE) } else { none });
                if let Some(space) = selected_space {
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

fn detail_row(ui: &mut egui::Ui, label: &str, value: impl ToString) {
    detail_text_row(ui, label, value.to_string().as_str());
}

fn detail_text_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).color(Style::TEXT_DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(value)
                    .strong()
                    .color(Style::TEXT_STRONG),
            );
        });
    });
}

fn discord_bridge_detail_row(ui: &mut egui::Ui, bridge: &DiscordBridgeRowModel) {
    ui.add_space(Style::SP_XS);
    ui.label(
        egui::RichText::new(bridge.label.as_str())
            .strong()
            .color(Style::TEXT),
    );
    detail_text_row(ui, "Status", bridge.status);
    detail_text_row(ui, "Discord → Mesh", bridge.inbound);
    detail_text_row(ui, "Mesh → Discord", bridge.outbound);
    detail_text_row(ui, "Provenance", bridge.provenance.as_str());
    if let Some(detail) = bridge.detail.as_deref() {
        ui.label(egui::RichText::new(detail).small().color(Style::TEXT_DIM));
    }
}

fn channel_find_editor(ui: &mut egui::Ui, surface: &mut CommunicationsSurface, space: SpaceId) {
    ui.label(
        egui::RichText::new("Find")
            .small()
            .strong()
            .color(Style::TEXT_DIM),
    );
    let mut query = surface.channel_find(space).to_owned();
    let response = ui.add_sized(
        [Style::SP_XL * 4.0, Style::SP_L],
        egui::TextEdit::singleline(&mut query)
            .id(egui::Id::new(("mesh-teams-channel-find", space.as_uuid())))
            .hint_text("Current channel")
            .clip_text(true),
    );
    if response.changed() {
        surface.set_channel_find(space, query);
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
