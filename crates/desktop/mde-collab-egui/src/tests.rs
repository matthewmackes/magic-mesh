//! Headless fixture tests for the Communications surface: the frame renders from
//! a fixture directory, the Messages composer's <kbd>Ctrl+Enter</kbd> emits a
//! `SendMessage`, the amend affordance follows the author window, the Activity
//! feed filters, and every icon paints as a real Carbon image mesh (not glyph
//! text) — mirroring the browser chrome's Carbon idiom.

#![allow(clippy::unwrap_used, clippy::panic, clippy::float_cmp)]

use mde_egui::egui;
use mde_egui::Style;

use std::collections::BTreeMap;

use mde_collab_types::{
    ActivityFeed, ActorId, AlertAction, AlertActionKind, AlertInbox, AlertPayload, AlertView,
    CallId, CallKind, CallParticipantState, CallParticipantView, CallView, ChannelTasks,
    ClipItemKind, ClipboardLane, ClipboardView, CollabCommand, ConversationTimeline, DeliveryState,
    DiscordBridgeBoard, DiscordBridgeConfigStatus, DiscordBridgeFlowStatus,
    DiscordBridgeProvenance, DiscordBridgeProvenanceSource, DiscordBridgeView, DocumentId,
    DocumentSession, DocumentSessions, EventId, FileRef, FileRefId, FileReferenceView,
    FileReferences, MessagePins, ReviewVerdict, SavedMessageView, SavedMessages, Severity, SpaceId,
    SpaceKind, SpaceRole, TaskView, ThreadId, ThreadTimeline, TransferControl, TransferDirection,
    TransferId, TransferJobView, TransferJobs, TransferMethod, TransferState,
};

use crate::activity::{activity_rows, filtered_activity_entries};
use crate::fixture::{activity, message, space_summary, FixtureData};
use crate::{
    amend_affordance, file_ref_of_path, ActivityFilter, AmendAffordance, ChannelTab, CollabData,
    CommandSink, CommunicationsSurface, DocSubMode, DocTemplate, DocView, MeshTeamsApp, Mode,
    ALL_COLLAB_ICONS, EDIT_WINDOW_MS,
};

/// A `1000 x 700` headless input with the given events.
fn sized_input(events: Vec<egui::Event>) -> egui::RawInput {
    sized_input_with_modifiers(events, egui::Modifiers::default())
}

/// A `1000 x 700` headless input with explicit currently-held modifiers.
fn sized_input_with_modifiers(
    events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1000.0, 700.0),
        )),
        events,
        modifiers,
        time: Some(0.0),
        ..Default::default()
    }
}

/// A pressed key event with no modifiers.
fn key(k: egui::Key) -> egui::Event {
    egui::Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::default(),
    }
}

/// A pressed key event with explicit modifiers.
fn modified_key(k: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn ctrl_enter() -> egui::Event {
    modified_key(
        egui::Key::Enter,
        egui::Modifiers {
            ctrl: true,
            ..Default::default()
        },
    )
}

/// Render one frame of `surface` against `data` and return the painted shapes.
fn render_shapes(
    surface: &mut CommunicationsSurface,
    data: &dyn CollabData,
) -> Vec<egui::epaint::ClippedShape> {
    render_shapes_with_size(surface, data, egui::vec2(1000.0, 700.0))
}

fn render_shapes_with_size(
    surface: &mut CommunicationsSurface,
    data: &dyn CollabData,
    size: egui::Vec2,
) -> Vec<egui::epaint::ClippedShape> {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut sink = CommandSink::new();
    let out = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events: Vec::new(),
            time: Some(0.0),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| surface.ui(ui, data, &mut sink));
        },
    );
    out.shapes
}

fn render_shapes_after_animation(
    surface: &mut CommunicationsSurface,
    data: &dyn CollabData,
    size: egui::Vec2,
) -> Vec<egui::epaint::ClippedShape> {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut sink = CommandSink::new();
    let mut shapes = Vec::new();
    for time in [0.0, 1.0] {
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                events: Vec::new(),
                time: Some(time),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| surface.ui(ui, data, &mut sink));
            },
        );
        shapes = out.shapes;
    }
    shapes
}

/// Render one frame with the Ford SYNC 3 **Auto (Car Mode)** skin installed, so
/// the surface sees [`StyleColorScheme::AutoSync3`] at [`Density::Touch`] and
/// takes its glanceable car branch (the shell installs exactly this while Car
/// Mode is active).
fn render_shapes_car(
    surface: &mut CommunicationsSurface,
    data: &dyn CollabData,
) -> Vec<egui::epaint::ClippedShape> {
    let ctx = egui::Context::default();
    Style::install_color_scheme_with_density(
        &ctx,
        mde_egui::StyleColorScheme::AutoSync3,
        mde_egui::Density::Touch,
    );
    let mut sink = CommandSink::new();
    let out = ctx.run(sized_input(vec![]), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| surface.ui(ui, data, &mut sink));
    });
    out.shapes
}

/// Count the painted image meshes (tinted Carbon glyphs) in `shapes`, mirroring
/// the browser chrome's `painted_image_mesh_count`.
fn image_mesh_count(shapes: &[egui::epaint::ClippedShape]) -> usize {
    fn walk(shape: &egui::Shape, out: &mut usize) {
        match shape {
            egui::Shape::Mesh(mesh) if !mesh.vertices.is_empty() => *out += 1,
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            _ => {}
        }
    }
    let mut out = 0;
    for clipped in shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

/// Collect every painted text run with its resolved colour (mirrors the Files
/// surface's `painted_text`), so a tooltip test can assert the themed text colour
/// and prove no raw black-on-light default popup leaked.
fn painted_text(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Color32)> {
    fn text_color(text: &egui::epaint::TextShape) -> egui::Color32 {
        if let Some(color) = text.override_text_color {
            return color;
        }
        text.galley
            .job
            .sections
            .iter()
            .find_map(|section| {
                (section.format.color != egui::Color32::PLACEHOLDER).then_some(section.format.color)
            })
            .unwrap_or(text.fallback_color)
    }

    fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Color32)>) {
        match shape {
            egui::Shape::Text(text) => {
                out.push((text.galley.text().to_owned(), text_color(text)));
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

/// Collect every opaque rect fill (mirrors the Files surface's `rect_fills`), so a
/// tooltip test can assert the themed surface plate is actually painted.
fn rect_fills(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Color32> {
    fn walk(shape: &egui::Shape, out: &mut Vec<egui::Color32>) {
        match shape {
            egui::Shape::Rect(rect) if rect.fill != egui::Color32::TRANSPARENT => {
                out.push(rect.fill);
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

/// The Communications hover hints route through the themed [`comms_tooltip`] rather
/// than egui's raw default popup: it paints its own Quazar-dark surface plate and
/// renders the hint in the themed text colour, leaking no black-on-light default.
/// Mirrors `mde-files-egui`'s `files_hover_tooltip_uses_themed_text_and_surface`.
///
/// [`comms_tooltip`]: crate::icons::comms_tooltip
#[test]
fn comms_hover_tooltip_uses_themed_text_and_surface() {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(320.0, 120.0),
        )),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                crate::icons::comms_tooltip(ui, "Inbound");
            });
    });

    let texts = painted_text(&out.shapes);
    assert!(
        texts
            .iter()
            .any(|(text, color)| text == "Inbound" && *color == Style::TEXT),
        "Comms tooltip should paint its hint in the themed text colour: {texts:?}"
    );
    assert!(
        !texts
            .iter()
            .any(|(text, color)| text == "Inbound" && *color == egui::Color32::BLACK),
        "Comms tooltip leaked raw black popup text: {texts:?}"
    );

    let fills = rect_fills(&out.shapes);
    assert!(
        fills.contains(&Style::SURFACE),
        "Comms tooltip should paint its own themed surface: {fills:?}"
    );
}

#[test]
fn frame_renders_from_fixture_directory() {
    // The frame (rail + tabs + call bar + body) renders headless from a fixture
    // SpaceDirectory, and the fixture has real spaces + both core tabs.
    let data = FixtureData::demo();
    assert!(
        data.space_directory().spaces.len() >= 2,
        "demo fixture must populate the rail"
    );
    assert!(Mode::TABS.contains(&Mode::Activity) && Mode::TABS.contains(&Mode::Messages));

    let mut surface = CommunicationsSurface::new();
    let shapes = render_shapes(&mut surface, &data);
    assert!(!shapes.is_empty(), "the frame painted no primitives");
    // Selecting the first rail row is automatic — the surface is now usable.
    assert!(
        surface.selected_space().is_some(),
        "the frame must auto-select the first rail space"
    );
}

#[test]
fn rail_row_model_maps_the_directory_in_order() {
    // PLATFORM-INTERFACES Q19 — the rail renders through the shared
    // nav_chrome::Sidebar off this pure row model (the U19 Settings idiom:
    // tests assert the model, not paint): one row per directory space, in
    // directory order, carrying the id, the name, the kind's Carbon glyph, and
    // the unread count the overlay badge paints.
    let data = FixtureData::demo();
    let directory = data.space_directory();
    let model = crate::frame::rail_row_model(directory);

    assert_eq!(model.len(), directory.spaces.len());
    assert_eq!(model.len(), 2, "the demo fixture populates two rail rows");

    let (id, name, glyph, unread, members) = model[0];
    assert_eq!(id, directory.spaces[0].id);
    assert_eq!(name, "Team Ops");
    assert_eq!(glyph, crate::icons::space_kind_icon(SpaceKind::Team));
    assert_eq!(unread, 3, "Team Ops carries the demo unread count");
    assert_eq!(members, 4, "Team Ops carries the demo member count");

    let (id, name, glyph, unread, members) = model[1];
    assert_eq!(id, directory.spaces[1].id);
    assert_eq!(name, "Incident 42");
    assert_eq!(glyph, crate::icons::space_kind_icon(SpaceKind::Incident));
    assert_eq!(unread, 0, "Incident 42 is read — no badge to paint");
    assert_eq!(members, 6, "Incident 42 carries the demo member count");
}

#[test]
fn details_pane_model_reads_selected_channel_facts() {
    // The Details pane is a read-model summary, not a provider stub: each count
    // comes from the same selected-space projections the existing bodies render.
    let space = SpaceId::new();
    let other = SpaceId::new();
    let file = FileRefId::new();
    let document = DocumentId::new();
    let transfer = TransferId::new();
    let peer = ActorId::new("falcon");

    let data = FixtureData::new("eagle", 1_000_000)
        .with_space(space_summary(
            space,
            SpaceKind::Team,
            "Team Ops",
            SpaceRole::Owner,
            5,
            4,
            990_000,
        ))
        .with_space(space_summary(
            other,
            SpaceKind::Incident,
            "Incident 42",
            SpaceRole::Member,
            0,
            2,
            980_000,
        ))
        .with_conversation(ConversationTimeline {
            space,
            thread: None,
            messages: vec![
                message(
                    EventId::new(),
                    &peer,
                    900_000,
                    "Deploy is green.",
                    DeliveryState::Delivered,
                    0,
                ),
                message(
                    EventId::new(),
                    &peer,
                    910_000,
                    "Review is queued.",
                    DeliveryState::Delivered,
                    0,
                ),
            ],
        })
        .with_file_references(FileReferences {
            space,
            files: vec![FileReferenceView {
                file,
                reference: FileRef {
                    name: "deploy.log".to_owned(),
                    size: 2048,
                    sha256_hex: "a".repeat(64),
                    mime: Some("text/plain".to_owned()),
                },
                linked_by: peer.clone(),
                linked_unix_ms: 920_000,
            }],
        })
        .with_transfer_jobs(TransferJobs {
            jobs: vec![TransferJobView {
                transfer,
                file,
                method: TransferMethod::Node,
                direction: TransferDirection::Outbound,
                state: TransferState::Active,
                moved: 1024,
                total: 2048,
            }],
        })
        .with_document_sessions(
            space,
            DocumentSessions {
                sessions: vec![DocumentSession {
                    document,
                    space,
                    title: "Runbook".to_owned(),
                    participants: vec![ActorId::new("eagle"), peer.clone()],
                    call: None,
                }],
            },
        )
        .with_clipboard_lane(ClipboardLane {
            space,
            items: vec![ClipboardView {
                event_id: EventId::new(),
                kind: ClipItemKind::Text,
                preview: "deploy token".to_owned(),
                sha256_hex: "b".repeat(64),
                source: "falcon".to_owned(),
                at_unix_ms: 930_000,
                pinned: false,
            }],
        })
        .with_call(CallView {
            call: CallId::new(),
            space,
            kind: CallKind::Audio,
            started_unix_ms: 940_000,
            participants: vec![CallParticipantView {
                actor: ActorId::new("eagle"),
                state: CallParticipantState::Connected,
                muted: false,
            }],
        });

    let details =
        crate::frame::channel_details_model(Some(space), &data).expect("selected channel details");
    assert_eq!(details.space, space);
    assert_eq!(details.name, "Team Ops");
    assert_eq!(details.kind, "Team");
    assert_eq!(details.role, "Owner");
    assert_eq!(details.members, 4);
    assert_eq!(details.unread, 5);
    assert_eq!(details.last_activity, "990000:0");
    assert_eq!(details.messages, 2);
    assert_eq!(details.files, 1);
    assert_eq!(details.transfers, 1);
    assert_eq!(details.documents, 1);
    assert_eq!(details.active_calls, 1);
    assert_eq!(details.clips, 1);
    assert_eq!(details.discord_bridges.len(), 1);
    assert_eq!(details.discord_bridges[0].status, "Unconfigured");
    assert_eq!(details.discord_bridges[0].inbound, "Not configured");
    assert_eq!(details.discord_bridges[0].outbound, "Not configured");

    assert!(
        crate::frame::channel_details_model(Some(other), &data).is_some(),
        "a selected directory member with sparse projections still gets honest zero counts"
    );
    assert!(
        crate::frame::channel_details_model(Some(SpaceId::new()), &data).is_none(),
        "a stale selection must not produce a Details pane target"
    );
}

#[test]
fn details_pane_model_scopes_discord_bridge_to_selected_channel() {
    let space = SpaceId::new();
    let other = SpaceId::new();
    let data = FixtureData::new("eagle", 1_000_000)
        .with_space(space_summary(
            space,
            SpaceKind::Team,
            "Team Ops",
            SpaceRole::Owner,
            0,
            4,
            990_000,
        ))
        .with_space(space_summary(
            other,
            SpaceKind::Incident,
            "Incident 42",
            SpaceRole::Member,
            0,
            2,
            980_000,
        ))
        .with_discord_bridge_board(DiscordBridgeBoard {
            bridges: vec![
                DiscordBridgeView {
                    bridge_id: "ops-bridge".to_owned(),
                    space: Some(space),
                    label: "Ops Discord bridge".to_owned(),
                    status: DiscordBridgeConfigStatus::Configured,
                    inbound: DiscordBridgeFlowStatus::Ready,
                    outbound: DiscordBridgeFlowStatus::Ready,
                    provenance: DiscordBridgeProvenance {
                        source: DiscordBridgeProvenanceSource::OperatorConfig,
                        authority: Some("mesh-team-revision:42".to_owned()),
                        observed_by: Some("seat-15".to_owned()),
                        config_digest: Some("sha256:ops".to_owned()),
                    },
                    detail: None,
                    updated_unix_ms: 995_000,
                },
                DiscordBridgeView {
                    bridge_id: "incident-provider".to_owned(),
                    space: Some(other),
                    label: "Incident Discord bridge".to_owned(),
                    status: DiscordBridgeConfigStatus::ProviderUnavailable,
                    inbound: DiscordBridgeFlowStatus::ProviderUnavailable,
                    outbound: DiscordBridgeFlowStatus::Degraded,
                    provenance: DiscordBridgeProvenance {
                        source: DiscordBridgeProvenanceSource::WorkerState,
                        authority: Some("mesh-team-revision:43".to_owned()),
                        observed_by: Some("seat-15".to_owned()),
                        config_digest: Some("sha256:incident".to_owned()),
                    },
                    detail: Some("Discord provider adapter unavailable.".to_owned()),
                    updated_unix_ms: 996_000,
                },
            ],
        });

    let details =
        crate::frame::channel_details_model(Some(space), &data).expect("selected channel details");
    assert_eq!(details.discord_bridges.len(), 1);
    assert_eq!(details.discord_bridges[0].label, "Ops Discord bridge");
    assert_eq!(details.discord_bridges[0].status, "Configured");
    assert_eq!(details.discord_bridges[0].inbound, "Ready");
    assert_eq!(details.discord_bridges[0].outbound, "Ready");
    assert!(
        details.discord_bridges[0]
            .provenance
            .contains("Operator config"),
        "configured rows must expose provenance: {:?}",
        details.discord_bridges[0]
    );

    let other_details =
        crate::frame::channel_details_model(Some(other), &data).expect("other channel details");
    assert_eq!(other_details.discord_bridges.len(), 1);
    assert_eq!(
        other_details.discord_bridges[0].status,
        "Provider unavailable"
    );
    assert_eq!(other_details.discord_bridges[0].outbound, "Degraded");
}

#[test]
fn details_pane_paints_inside_the_frame() {
    let data = FixtureData::demo();
    let mut surface = CommunicationsSurface::new();
    let shapes = render_shapes(&mut surface, &data);
    let texts = painted_text(&shapes);

    for expected in [
        "Details",
        "Channel facts",
        "Live projections",
        "Messages",
        "Files",
        "Documents",
        "Active calls",
        "Clip items",
        "Discord bridge",
        "Discord → Mesh",
        "Mesh → Discord",
        "Provenance",
    ] {
        assert!(
            texts.iter().any(|(text, _)| text == expected),
            "the Details pane must paint {expected:?}: {texts:?}"
        );
    }
}

#[test]
fn app_rail_model_exposes_operator_apps_in_order() {
    let model = crate::frame::app_rail_model();
    let labels: Vec<_> = model.iter().map(|(_, label, _)| *label).collect();
    assert_eq!(
        labels,
        vec![
            "Activity",
            "Teams",
            "Calls",
            "Files",
            "Alerts",
            "Transfers",
            "Clipboard",
            "Settings",
        ],
        "the far-left app rail must expose the Teams-like app order"
    );
    assert!(
        model
            .iter()
            .all(|(_, _, glyph)| ALL_COLLAB_ICONS.contains(glyph)),
        "every app-rail glyph must be registered in the surface icon set: {model:?}"
    );
}

#[test]
fn channel_tabs_include_posts_files_calls_and_tasks() {
    let tabs: Vec<_> = ChannelTab::ALL
        .iter()
        .map(|tab| {
            (
                tab.label(),
                tab.mode(),
                crate::icons::channel_tab_icon(*tab),
            )
        })
        .collect();
    assert_eq!(
        tabs,
        vec![
            ("Posts", Mode::Messages, "share"),
            ("Files", Mode::Files, "download"),
            ("Calls", Mode::Calls, "audio-volume-high"),
            ("Tasks", Mode::Tasks, "emblem-ok"),
        ]
    );
}

#[test]
fn channel_tasks_mode_renders_projected_rows() {
    let space = SpaceId::new();
    let task = EventId::new();
    let data = FixtureData::new("eagle", 1_000_000)
        .with_space(space_summary(
            space,
            SpaceKind::Team,
            "Team Ops",
            SpaceRole::Owner,
            0,
            4,
            990_000,
        ))
        .with_channel_tasks(ChannelTasks {
            space,
            tasks: vec![TaskView {
                task,
                space,
                title: "Rotate gateway".to_owned(),
                created_by: ActorId::new("falcon"),
                created_unix_ms: 990_000,
                source: None,
                checked: false,
                completed: false,
                completed_by: None,
                completed_unix_ms: None,
            }],
        });
    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.set_channel_tab(ChannelTab::Tasks);

    let texts = painted_text(&render_shapes_after_animation(
        &mut surface,
        &data,
        egui::vec2(1000.0, 700.0),
    ));
    for expected in [
        "Tasks",
        "Channel tasks",
        "operator-authored action items",
        "Rotate gateway",
    ] {
        assert!(
            texts.iter().any(|(text, _)| text == expected),
            "Tasks mode must paint {expected:?}: {texts:?}"
        );
    }
    assert!(
        texts
            .iter()
            .any(|(text, _)| text.starts_with("by falcon ·")),
        "Tasks mode must paint task authorship metadata: {texts:?}"
    );
}

#[test]
fn channel_task_actions_emit_create_update_check_complete_and_reopen_commands() {
    let space = SpaceId::new();
    let task = EventId::new();
    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);

    let mut sink = CommandSink::new();
    surface.set_task_draft(space, " rotate gateway ");
    surface.create_task_from_draft(&mut sink, space);
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::CreateTask {
                space: command_space,
                title,
                source
            }) if *command_space == space && title == "rotate gateway" && source.is_none()
        ),
        "creating a task must emit a typed CreateTask command: {:?}",
        sink.queued()
    );
    assert_eq!(
        surface.task_draft(space),
        "",
        "the draft clears after a successful create"
    );

    let mut sink = CommandSink::new();
    surface.update_task(&mut sink, space, task, " rotate gateway v2 ");
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::UpdateTask {
                space: command_space,
                task: command_task,
                title
            }) if *command_space == space && *command_task == task && title == "rotate gateway v2"
        ),
        "updating a task must emit a bounded typed UpdateTask command: {:?}",
        sink.queued()
    );

    let mut sink = CommandSink::new();
    surface.set_task_checked(&mut sink, space, task, true);
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::SetTaskChecked {
                space: command_space,
                task: command_task,
                checked: true
            }) if *command_space == space && *command_task == task
        ),
        "checking a task must emit a typed SetTaskChecked command: {:?}",
        sink.queued()
    );

    let mut sink = CommandSink::new();
    surface.complete_task(&mut sink, space, task);
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::CompleteTask {
                space: command_space,
                task: command_task
            }) if *command_space == space && *command_task == task
        ),
        "completing a task must emit a typed CompleteTask command: {:?}",
        sink.queued()
    );

    let mut sink = CommandSink::new();
    surface.reopen_task(&mut sink, space, task);
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::ReopenTask {
                space: command_space,
                task: command_task
            }) if *command_space == space && *command_task == task
        ),
        "reopening a task must emit a typed ReopenTask command: {:?}",
        sink.queued()
    );
}

#[test]
fn set_app_preserves_channel_selection_and_routes_existing_bodies() {
    let space = SpaceId::new();
    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.set_channel_tab(ChannelTab::Files);
    surface.set_app(MeshTeamsApp::Calls);

    assert_eq!(surface.selected_space(), Some(space));
    assert_eq!(surface.app(), MeshTeamsApp::Calls);
    assert_eq!(surface.mode(), Mode::Calls);
    assert_eq!(
        surface.channel_tab(),
        ChannelTab::Files,
        "global app hops must not clear the active Teams channel tab"
    );

    surface.set_app(MeshTeamsApp::Teams);
    assert_eq!(surface.app(), MeshTeamsApp::Teams);
    assert_eq!(
        surface.mode(),
        Mode::Files,
        "returning to Teams must restore the remembered channel tab body"
    );
}

#[test]
fn call_bar_rejects_a_selection_removed_from_the_directory() {
    // The read model may remove a space while the surface still holds its
    // previous view selection. The persistent call bar must not offer a
    // StartCall target that is no longer a member of the directory.
    let removed = SpaceId::new();
    let data = FixtureData::new("eagle", 1_000);

    assert_eq!(
        crate::frame::selected_space_in_directory(Some(removed), data.space_directory()),
        None,
        "an empty directory must invalidate a retained selection"
    );

    let present = SpaceId::new();
    let data = FixtureData::new("eagle", 1_000).with_space(space_summary(
        present,
        SpaceKind::Team,
        "Team Ops",
        SpaceRole::Owner,
        0,
        1,
        1_000,
    ));
    assert_eq!(
        crate::frame::selected_space_in_directory(Some(present), data.space_directory()),
        Some(present),
        "a current directory member remains a valid call target"
    );
}

#[test]
fn rail_paints_the_shared_sidebar_with_the_unread_badge_overlay() {
    // PLATFORM-INTERFACES Q19 — the painted rail is the shared Sidebar: the
    // "Teams & Channels" section header and both row labels paint, the auto-selected
    // first row wears the shared selection plate, and the live unread count
    // rides the overlay bridge as an accent pill with the bright count.
    let data = FixtureData::demo();
    let mut surface = CommunicationsSurface::new();
    let shapes = render_shapes(&mut surface, &data);

    let texts = painted_text(&shapes);
    for expected in ["Teams & Channels", "Team Ops", "Incident 42"] {
        assert!(
            texts.iter().any(|(text, _)| text == expected),
            "the sidebar must paint {expected:?}: {texts:?}"
        );
    }
    assert!(
        texts
            .iter()
            .any(|(text, color)| text == "3" && *color == Style::TEXT_STRONG),
        "the unread overlay badge must paint the bright count: {texts:?}"
    );

    let fills = rect_fills(&shapes);
    assert!(
        fills.contains(&Style::selection_fill()),
        "the auto-selected row must wear the shared Sidebar selection plate: {fills:?}"
    );
    assert!(
        fills.contains(&Style::ACCENT),
        "the unread badge must paint its accent pill: {fills:?}"
    );
}

#[test]
fn rail_click_routes_through_the_shared_sidebar() {
    // PLATFORM-INTERFACES Q19 — a click on a shared-Sidebar rail row routes
    // through the SAME select_space seam the old hand-rolled rows drove
    // (behaviour sacred): clicking the second row's registered rect moves the
    // selection to the second space.
    let data = FixtureData::demo();
    let mut surface = CommunicationsSurface::new();
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut sink = CommandSink::new();

    // Frame 1 registers the rows (and auto-selects the first space).
    let _ = ctx.run(sized_input(vec![]), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| surface.ui(ui, &data, &mut sink));
    });
    let first = data.space_directory().spaces[0].id;
    let second = data.space_directory().spaces[1].id;
    assert_eq!(surface.selected_space(), Some(first));

    let row = ctx
        .read_response(mde_egui::nav_chrome::Sidebar::row_id(
            crate::frame::RAIL_SIDEBAR_SALT,
            1,
        ))
        .expect("the second rail row registered under the shared Sidebar row id");
    let at = row.rect.center();

    // Frame 2 clicks the second row.
    let _ = ctx.run(
        sized_input(vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ]),
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| surface.ui(ui, &data, &mut sink));
        },
    );
    assert_eq!(
        surface.selected_space(),
        Some(second),
        "a Sidebar row click must route through select_space"
    );
}

#[test]
fn frame_paints_carbon_image_meshes_not_glyph_text() {
    // Every surface icon (rail kind glyphs, mode-tab glyphs, call-bar glyphs)
    // paints through the shared Mackes-Carbon loader as a tinted image mesh, not
    // as glyph text — the icon-standard invariant the browser chrome also holds.
    let data = FixtureData::demo();
    let mut surface = CommunicationsSurface::new();
    let shapes = render_shapes(&mut surface, &data);
    assert!(
        image_mesh_count(&shapes) > 0,
        "the frame must paint Carbon icons as image meshes"
    );
}

#[test]
fn every_collab_icon_is_registered_and_rasterizes() {
    // Mirror of `every_chrome_icon_maps_to_a_registered_carbon_glyph`: every glyph
    // this surface can paint is embedded in the shared loader and rasterizes to a
    // non-blank tinted mask.
    for name in ALL_COLLAB_ICONS {
        assert!(
            mde_egui::carbon::carbon_svg_bytes(name).is_some(),
            "{name:?} must be embedded in the Carbon loader registry"
        );
        let raster = mde_egui::carbon::carbon_raster(name, 32, Style::TEXT);
        assert!(
            raster
                .as_ref()
                .is_some_and(|r| r.rgba.chunks_exact(4).any(|px| px[3] > 0)),
            "{name:?} must rasterize to a non-blank mask"
        );
    }
}

#[test]
fn call_bar_renders_with_an_empty_call_state() {
    // The persistent call bar must render (no active call → the honest
    // placeholder), never panic, even when CallState is empty.
    let space = SpaceId::new();
    let data = FixtureData::new("eagle", 1_000).with_space(space_summary(
        space,
        SpaceKind::Team,
        "Team Ops",
        SpaceRole::Owner,
        0,
        2,
        1_000,
    ));
    assert!(data.call_state().active.is_empty());
    let mut surface = CommunicationsSurface::new();
    let shapes = render_shapes(&mut surface, &data);
    assert!(!shapes.is_empty(), "the empty-call frame painted nothing");
}

#[test]
fn messages_timeline_renders_a_fixture_conversation() {
    let space = SpaceId::new();
    let peer = ActorId::new("falcon");
    let data = FixtureData::new("eagle", 1_000_000)
        .with_space(space_summary(
            space,
            SpaceKind::Team,
            "Team Ops",
            SpaceRole::Owner,
            1,
            2,
            1_000_000,
        ))
        .with_conversation(ConversationTimeline {
            space,
            thread: None,
            messages: vec![message(
                EventId::new(),
                &peer,
                900_000,
                "Deploy is green.",
                DeliveryState::Delivered,
                0,
            )],
        });
    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.set_mode(Mode::Messages);
    let shapes = render_shapes(&mut surface, &data);
    assert!(!shapes.is_empty(), "the Messages timeline painted nothing");
}

#[test]
fn channel_find_is_local_to_each_channel() {
    let first = SpaceId::new();
    let second = SpaceId::new();
    let mut surface = CommunicationsSurface::new();

    surface.set_channel_find(first, "deploy");
    surface.set_channel_find(second, "incident");
    surface.select_space(first);
    assert_eq!(surface.channel_find(first), "deploy");

    surface.select_space(second);
    assert_eq!(
        surface.channel_find(second),
        "incident",
        "current-channel find must be keyed by channel, not global Mesh Teams state"
    );
    assert_eq!(
        surface.channel_find(first),
        "deploy",
        "switching channels must not destroy the prior local find text"
    );
}

#[test]
fn channel_find_filters_the_posts_timeline() {
    let space = SpaceId::new();
    let peer = ActorId::new("falcon");
    let data = FixtureData::new("eagle", 1_000_000)
        .with_space(space_summary(
            space,
            SpaceKind::Team,
            "Team Ops",
            SpaceRole::Owner,
            1,
            2,
            1_000_000,
        ))
        .with_conversation(ConversationTimeline {
            space,
            thread: None,
            messages: vec![
                message(
                    EventId::new(),
                    &peer,
                    900_000,
                    "Deploy is green.",
                    DeliveryState::Delivered,
                    0,
                ),
                message(
                    EventId::new(),
                    &peer,
                    910_000,
                    "Incident bridge is noisy.",
                    DeliveryState::Delivered,
                    0,
                ),
            ],
        });
    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.set_mode(Mode::Messages);
    surface.set_channel_find(space, "deploy");

    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut sink = CommandSink::new();
    let mut shapes = Vec::new();
    for time in [0.0, 1.0] {
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 700.0),
                )),
                time: Some(time),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    surface.ui(ui, &data, &mut sink);
                });
            },
        );
        shapes = out.shapes;
    }
    let texts = painted_text(&shapes);
    assert!(
        texts
            .iter()
            .any(|(text, _)| text == "1 current-channel match"),
        "the current-channel find summary must paint the match count: {texts:?}"
    );
    assert!(
        !texts
            .iter()
            .any(|(text, _)| text == "Incident bridge is noisy."),
        "a non-matching post must be filtered from the visible Posts timeline: {texts:?}"
    );
}

#[test]
fn local_quick_reactions_are_view_state_only() {
    let message = EventId::new();
    let mut surface = CommunicationsSurface::new();
    let sink = CommandSink::new();

    surface.toggle_local_reaction(message, crate::messages::LocalReaction::Ack);
    assert_eq!(
        surface.local_reaction(message),
        Some(crate::messages::LocalReaction::Ack)
    );
    assert!(
        sink.is_empty(),
        "local reactions must not enqueue any collaboration command"
    );

    surface.toggle_local_reaction(message, crate::messages::LocalReaction::Check);
    assert_eq!(
        surface.local_reaction(message),
        Some(crate::messages::LocalReaction::Check),
        "choosing a different quick reaction replaces the local state"
    );

    surface.toggle_local_reaction(message, crate::messages::LocalReaction::Check);
    assert_eq!(
        surface.local_reaction(message),
        None,
        "clicking the selected local reaction clears it"
    );
}

#[test]
fn messages_timeline_renders_constrained_local_reaction_chips() {
    let mut surface = CommunicationsSurface::new();
    let message = EventId::new();
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let out = ctx.run(sized_input(vec![]), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            surface.local_reaction_buttons(ui, message);
        });
    });
    let texts = painted_text(&out.shapes);

    for expected in ["Local", "Ack", "Check", "Watch"] {
        assert!(
            texts.iter().any(|(text, _)| text == expected),
            "Messages must render constrained local quick reaction chip {expected:?}: {texts:?}"
        );
    }
}

#[test]
fn message_pin_save_affordances_render_projected_state() {
    let space = SpaceId::new();
    let peer = ActorId::new("falcon");
    let message_id = EventId::new();
    let data = FixtureData::new("eagle", 1_000_000)
        .with_space(space_summary(
            space,
            SpaceKind::Team,
            "Team Ops",
            SpaceRole::Owner,
            0,
            2,
            1_000_000,
        ))
        .with_conversation(ConversationTimeline {
            space,
            thread: None,
            messages: vec![message(
                message_id,
                &peer,
                900_000,
                "Pin this once the real contract lands.",
                DeliveryState::Delivered,
                0,
            )],
        })
        .with_message_pins(MessagePins {
            space,
            messages: vec![message_id],
        })
        .with_saved_messages(SavedMessages {
            actor: ActorId::new("eagle"),
            messages: vec![SavedMessageView {
                space,
                message: message_id,
                saved_unix_ms: 950_000,
            }],
        });
    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.set_mode(Mode::Messages);

    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut sink = CommandSink::new();
    let mut shapes = Vec::new();
    for time in [0.0, 1.0] {
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 700.0),
                )),
                time: Some(time),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| surface.ui(ui, &data, &mut sink));
            },
        );
        shapes = out.shapes;
    }
    let texts = painted_text(&shapes);

    for expected in ["Keep", "Unpin", "Unsave"] {
        assert!(
            texts.iter().any(|(text, _)| text == expected),
            "Messages must paint projected pin/save state {expected:?}: {texts:?}"
        );
    }
    assert!(
        sink.is_empty(),
        "rendering must not emit commands without a click"
    );
}

#[test]
fn message_pin_save_toggles_emit_typed_commands() {
    let surface = CommunicationsSurface::new();
    let mut sink = CommandSink::new();
    let space = SpaceId::new();
    let message = EventId::new();

    surface.toggle_message_pin(&mut sink, space, message, false);
    surface.toggle_message_save(&mut sink, space, message, false);
    surface.toggle_message_pin(&mut sink, space, message, true);
    surface.toggle_message_save(&mut sink, space, message, true);

    assert_eq!(
        sink.drain(),
        vec![
            CollabCommand::PinMessage {
                space,
                target: message,
            },
            CollabCommand::SaveMessage {
                space,
                target: message,
            },
            CollabCommand::UnpinMessage {
                space,
                target: message,
            },
            CollabCommand::UnsaveMessage {
                space,
                target: message,
            },
        ]
    );
}

fn focused_messages_surface(
    ctx: &egui::Context,
    surface: &mut CommunicationsSurface,
    data: &FixtureData,
    edit_id: egui::Id,
) {
    let mut sink = CommandSink::new();
    let _ = ctx.run(sized_input(Vec::new()), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| surface.ui(ui, data, &mut sink));
    });
    ctx.memory_mut(|memory| memory.request_focus(edit_id));
}

#[test]
fn typing_then_ctrl_enter_emits_send_message() {
    let ctx = egui::Context::default();
    Style::install(&ctx);

    let space = SpaceId::new();
    let data = FixtureData::new("eagle", 1_000_000)
        .with_space(space_summary(
            space,
            SpaceKind::Team,
            "Team Ops",
            SpaceRole::Owner,
            0,
            2,
            1_000_000,
        ))
        .with_conversation(ConversationTimeline {
            space,
            thread: None,
            messages: Vec::new(),
        });

    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.set_mode(Mode::Messages);
    let edit_id = surface.composer_edit_id(space);
    focused_messages_surface(&ctx, &mut surface, &data, edit_id);

    // Pass 2: type into the focused composer and press Ctrl+Enter.
    let mut sink = CommandSink::new();
    let modifiers = egui::Modifiers {
        ctrl: true,
        ..Default::default()
    };
    let events = vec![egui::Event::Text("hello mesh".to_owned()), ctrl_enter()];
    let _ = ctx.run(sized_input_with_modifiers(events, modifiers), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| surface.ui(ui, &data, &mut sink));
    });

    let sent = sink.queued().iter().find_map(|c| match c {
        CollabCommand::SendMessage {
            space: s,
            thread,
            body,
        } => Some((*s, *thread, body.as_str().to_owned())),
        _ => None,
    });
    assert_eq!(
        sent,
        Some((space, None, "hello mesh".to_owned())),
        "typing then Ctrl+Enter must emit SendMessage with the typed body in the selected space"
    );
}

#[test]
fn plain_enter_inserts_newline_without_sending_message() {
    let ctx = egui::Context::default();
    Style::install(&ctx);

    let space = SpaceId::new();
    let data = FixtureData::new("eagle", 1_000_000)
        .with_space(space_summary(
            space,
            SpaceKind::Team,
            "Team Ops",
            SpaceRole::Owner,
            0,
            2,
            1_000_000,
        ))
        .with_conversation(ConversationTimeline {
            space,
            thread: None,
            messages: Vec::new(),
        });

    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.set_mode(Mode::Messages);
    let edit_id = surface.composer_edit_id(space);
    focused_messages_surface(&ctx, &mut surface, &data, edit_id);

    let mut sink = CommandSink::new();
    let events = vec![
        egui::Event::Text("line one".to_owned()),
        key(egui::Key::Enter),
        egui::Event::Text("line two".to_owned()),
    ];
    let _ = ctx.run(sized_input(events), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| surface.ui(ui, &data, &mut sink));
    });

    assert!(
        sink.queued()
            .iter()
            .all(|command| !matches!(command, CollabCommand::SendMessage { .. })),
        "plain Enter must edit the multiline draft, not send: {:?}",
        sink.queued()
    );
    assert_eq!(
        surface.draft(space),
        "line one\nline two",
        "plain Enter must insert a newline in the persisted composer draft"
    );
}

#[test]
fn thread_ctrl_enter_emits_reply() {
    let ctx = egui::Context::default();
    Style::install(&ctx);

    let space = SpaceId::new();
    let thread = ThreadId::new();
    let actor = ActorId::new("eagle");
    let root = EventId::new();
    let data = FixtureData::new("eagle", 1_000_000)
        .with_space(space_summary(
            space,
            SpaceKind::Team,
            "Team Ops",
            SpaceRole::Owner,
            0,
            2,
            1_000_000,
        ))
        .with_conversation(ConversationTimeline {
            space,
            thread: None,
            messages: vec![message(
                root,
                &actor,
                900_000,
                "Root message",
                DeliveryState::Sent,
                1,
            )],
        })
        .with_thread(
            root,
            ThreadTimeline {
                space,
                thread,
                root: message(
                    root,
                    &actor,
                    900_000,
                    "Root message",
                    DeliveryState::Sent,
                    1,
                ),
                replies: Vec::new(),
                resolved: false,
            },
        );
    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.set_mode(Mode::Messages);
    surface.open_thread_for_test(thread);
    let edit_id = surface.thread_composer_edit_id(thread);
    focused_messages_surface(&ctx, &mut surface, &data, edit_id);

    let mut sink = CommandSink::new();
    let modifiers = egui::Modifiers {
        ctrl: true,
        ..Default::default()
    };
    let events = vec![egui::Event::Text("thread reply".to_owned()), ctrl_enter()];
    let _ = ctx.run(sized_input_with_modifiers(events, modifiers), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| surface.ui(ui, &data, &mut sink));
    });

    let sent = sink.queued().iter().find_map(|command| match command {
        CollabCommand::ReplyInThread {
            space: s,
            thread: t,
            body,
        } => Some((*s, *t, body.as_str().to_owned())),
        _ => None,
    });
    assert_eq!(
        sent,
        Some((space, thread, "thread reply".to_owned())),
        "thread composer must use Ctrl+Enter for replies"
    );
    assert_eq!(
        surface.thread_draft_for_test(thread),
        "",
        "a successful thread reply clears only that thread draft"
    );
}

#[test]
fn thread_resolution_actions_emit_commands() {
    let space = SpaceId::new();
    let thread = ThreadId::new();
    let surface = CommunicationsSurface::new();
    let mut sink = CommandSink::new();

    surface.set_thread_resolved(&mut sink, space, thread, true);
    surface.set_thread_resolved(&mut sink, space, thread, false);

    assert!(matches!(
        sink.queued().first(),
        Some(CollabCommand::ResolveThread { space: s, thread: t }) if *s == space && *t == thread
    ));
    assert!(matches!(
        sink.queued().get(1),
        Some(CollabCommand::ReopenThread { space: s, thread: t }) if *s == space && *t == thread
    ));
}

#[test]
fn thread_resolution_control_reflects_timeline_state() {
    let space = SpaceId::new();
    let thread = ThreadId::new();
    let actor = ActorId::new("eagle");
    let root = EventId::new();
    let root_message = message(
        root,
        &actor,
        900_000,
        "Root message",
        DeliveryState::Sent,
        1,
    );
    let base = FixtureData::new("eagle", 1_000_000)
        .with_space(space_summary(
            space,
            SpaceKind::Team,
            "Team Ops",
            SpaceRole::Owner,
            0,
            2,
            1_000_000,
        ))
        .with_conversation(ConversationTimeline {
            space,
            thread: None,
            messages: vec![root_message.clone()],
        });

    let unresolved = base.clone().with_thread(
        root,
        ThreadTimeline {
            space,
            thread,
            root: root_message.clone(),
            replies: Vec::new(),
            resolved: false,
        },
    );
    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.set_mode(Mode::Messages);
    surface.open_thread_for_test(thread);
    let texts = painted_text(&render_shapes(&mut surface, &unresolved));
    assert!(texts.iter().any(|(text, _)| text == "Resolve"));
    assert!(!texts.iter().any(|(text, _)| text == "Reopen"));

    let resolved = base.with_thread(
        root,
        ThreadTimeline {
            space,
            thread,
            root: root_message,
            replies: Vec::new(),
            resolved: true,
        },
    );
    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.set_mode(Mode::Messages);
    surface.open_thread_for_test(thread);
    let texts = painted_text(&render_shapes(&mut surface, &resolved));
    assert!(texts.iter().any(|(text, _)| text == "Reopen"));
    assert!(texts.iter().any(|(text, _)| text == "Thread resolved"));
}

#[test]
fn amend_affordance_follows_the_author_window() {
    let me = ActorId::new("eagle");
    let peer = ActorId::new("falcon");
    let now = 10_000_000;

    let mine_fresh = message(
        EventId::new(),
        &me,
        now - 1_000,
        "fresh",
        DeliveryState::Sent,
        0,
    );
    let mine_old = message(
        EventId::new(),
        &me,
        now - EDIT_WINDOW_MS - 1,
        "old",
        DeliveryState::Sent,
        0,
    );
    let theirs = message(
        EventId::new(),
        &peer,
        now - 1_000,
        "theirs",
        DeliveryState::Sent,
        0,
    );
    let mut mine_deleted = mine_fresh.clone();
    mine_deleted.deleted = true;

    assert_eq!(
        amend_affordance(&me, now, &mine_fresh),
        AmendAffordance::Allowed
    );
    assert!(amend_affordance(&me, now, &mine_fresh).is_enabled());

    // Past the window: still shown, but denied — never silently hidden.
    assert_eq!(
        amend_affordance(&me, now, &mine_old),
        AmendAffordance::DeniedExpired
    );
    assert!(amend_affordance(&me, now, &mine_old).is_visible());
    assert!(!amend_affordance(&me, now, &mine_old).is_enabled());

    // Someone else's message, or a tombstone: no affordance at all.
    assert_eq!(amend_affordance(&me, now, &theirs), AmendAffordance::Hidden);
    assert_eq!(
        amend_affordance(&me, now, &mine_deleted),
        AmendAffordance::Hidden
    );
    assert!(!amend_affordance(&me, now, &theirs).is_visible());
}

#[test]
fn activity_filter_narrows_the_feed() {
    let space = SpaceId::new();
    let actor = ActorId::new("eagle");
    let entries = vec![
        activity(
            EventId::new(),
            space,
            &actor,
            5,
            "message_posted",
            "a message",
        ),
        activity(
            EventId::new(),
            space,
            &actor,
            4,
            "thread_started",
            "a thread",
        ),
        activity(EventId::new(), space, &actor, 3, "alert_raised", "an alert"),
        activity(EventId::new(), space, &actor, 2, "call_started", "a call"),
        activity(EventId::new(), space, &actor, 1, "file_linked", "a file"),
    ];
    let feed = ActivityFeed {
        space: Some(space),
        entries,
    };

    let count = |filter: ActivityFilter| {
        feed.entries
            .iter()
            .filter(|e| filter.matches(&e.kind_tag))
            .count()
    };

    assert_eq!(count(ActivityFilter::All), 5, "All admits every entry");
    let messages = count(ActivityFilter::Messages);
    assert_eq!(messages, 2, "Messages admits message + thread bands");
    assert!(
        messages < count(ActivityFilter::All),
        "a filter must narrow the feed"
    );
    assert_eq!(count(ActivityFilter::Alerts), 1);
    assert_eq!(count(ActivityFilter::Calls), 1);
    assert_eq!(count(ActivityFilter::Files), 1);
    assert_eq!(count(ActivityFilter::People), 0);
}

#[test]
fn activity_body_renders_the_feed() {
    let data = FixtureData::demo();
    let first = data.space_directory().spaces.first().map(|s| s.id);
    let mut surface = CommunicationsSurface::new();
    if let Some(space) = first {
        surface.select_space(space);
    }
    surface.set_mode(Mode::Activity);
    let shapes = render_shapes(&mut surface, &data);
    assert!(!shapes.is_empty(), "the Activity feed painted nothing");
}

#[test]
fn activity_app_prefers_cross_space_feed() {
    let space = SpaceId::new();
    let actor = ActorId::new("seat-15");
    let data = FixtureData::new("seat-15", 10_000)
        .with_space(space_summary(
            space,
            SpaceKind::Team,
            "Operations",
            SpaceRole::Owner,
            0,
            1,
            10_000,
        ))
        .with_activity(
            None,
            ActivityFeed {
                space: None,
                entries: vec![activity(
                    EventId::new(),
                    space,
                    &actor,
                    9_000,
                    "alert_raised",
                    "global activity",
                )],
            },
        )
        .with_activity(
            Some(space),
            ActivityFeed {
                space: Some(space),
                entries: vec![activity(
                    EventId::new(),
                    space,
                    &actor,
                    9_100,
                    "message_posted",
                    "selected-channel activity",
                )],
            },
        );
    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.set_app(MeshTeamsApp::Activity);

    let texts = painted_text(&render_shapes(&mut surface, &data));

    assert!(
        texts.iter().any(|(text, _)| text == "global activity"),
        "Activity app must read the cross-space feed when present: {texts:?}"
    );
    assert!(
        !texts
            .iter()
            .any(|(text, _)| text == "selected-channel activity"),
        "Activity app must not silently narrow to the selected channel when a cross-space feed exists: {texts:?}"
    );
}

#[test]
fn activity_body_filters_before_virtualized_rows() {
    let space = SpaceId::new();
    let actor = ActorId::new("seat-15");
    let mut entries = Vec::new();
    for index in 0..2_000 {
        let kind = if index % 4 == 0 {
            "alert_raised"
        } else {
            "message_posted"
        };
        entries.push(activity(
            EventId::new(),
            space,
            &actor,
            index,
            kind,
            "seat 15 activity",
        ));
    }
    let alerts = filtered_activity_entries(&entries, ActivityFilter::Alerts);

    assert_eq!(
        alerts.len(),
        500,
        "filtering builds the virtualized row set without requiring every feed row to paint"
    );
    assert!(alerts.iter().all(|entry| entry.kind_tag == "alert_raised"));
}

#[test]
fn activity_all_filter_keeps_the_source_slice_for_first_open() {
    let space = SpaceId::new();
    let actor = ActorId::new("seat-15");
    let entries = (0..2_000)
        .map(|index| {
            activity(
                EventId::new(),
                space,
                &actor,
                index,
                "message_posted",
                "seat 15 activity",
            )
        })
        .collect::<Vec<_>>();

    let rows = activity_rows(&entries, ActivityFilter::All);

    assert!(
        rows.uses_unfiltered_source(),
        "opening Activity on the default All filter must not build a per-row filter index"
    );
    assert_eq!(rows.len(), entries.len());
}

#[test]
fn activity_body_virtualizes_large_feeds() {
    let space = SpaceId::new();
    let actor = ActorId::new("seat-15");
    let entries = (0..2_000)
        .map(|index| {
            activity(
                EventId::new(),
                space,
                &actor,
                index,
                "message_posted",
                "seat 15 activity",
            )
        })
        .collect();
    let data = FixtureData::new("seat-15", 2_000)
        .with_space(space_summary(
            space,
            SpaceKind::Team,
            "Operations",
            SpaceRole::Owner,
            0,
            1,
            2_000,
        ))
        .with_activity(
            Some(space),
            ActivityFeed {
                space: Some(space),
                entries,
            },
        );
    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.set_mode(Mode::Activity);

    let shapes = render_shapes(&mut surface, &data);

    assert!(
        shapes.len() < 1_000,
        "Activity should paint only visible virtualized rows, not every retained row; got {} shapes",
        shapes.len()
    );
}

#[test]
fn every_mode_is_implemented() {
    // No tab is a labeled-for-later placeholder any more: Documents (WL-FUNC-011
    // Phase 3c foundation) embeds the real editor and emits the collab document
    // commands, joining the other six fully-implemented modes.
    for mode in Mode::TABS {
        assert!(mode.is_implemented(), "{mode:?} must be implemented");
    }
}

#[test]
fn drafts_persist_across_space_switches() {
    let a = SpaceId::new();
    let b = SpaceId::new();
    let mut surface = CommunicationsSurface::new();
    surface.set_draft(a, "half-written");
    surface.select_space(b);
    surface.select_space(a);
    assert_eq!(
        surface.draft(a),
        "half-written",
        "a switched-away draft must survive locally"
    );
}

// ── Files mode (WL-FUNC-011) ─────────────────────────────────────────────────

/// A fixture space with one linked file whose transfer is active — the Files
/// mode's happy-path read model.
fn files_fixture(space: SpaceId, file: FileRefId, transfer: TransferId) -> FixtureData {
    let owner = ActorId::new("eagle");
    FixtureData::new("eagle", 1_000_000)
        .with_space(space_summary(
            space,
            SpaceKind::Team,
            "Team Ops",
            SpaceRole::Owner,
            0,
            2,
            1_000_000,
        ))
        .with_file_references(FileReferences {
            space,
            files: vec![FileReferenceView {
                file,
                reference: FileRef {
                    name: "deploy.log".to_owned(),
                    size: 2048,
                    sha256_hex: "a".repeat(64),
                    mime: Some("text/plain".to_owned()),
                },
                linked_by: owner,
                linked_unix_ms: 900_000,
            }],
        })
        .with_transfer_jobs(TransferJobs {
            jobs: vec![TransferJobView {
                transfer,
                file,
                method: TransferMethod::Node,
                direction: TransferDirection::Outbound,
                state: TransferState::Active,
                moved: 1024,
                total: 2048,
            }],
        })
}

#[test]
fn files_mode_renders_a_fixture_reference_set() {
    let space = SpaceId::new();
    let data = files_fixture(space, FileRefId::new(), TransferId::new());
    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.set_mode(Mode::Files);
    let shapes = render_shapes(&mut surface, &data);
    assert!(
        !shapes.is_empty(),
        "the Files reference list painted nothing"
    );
    // Carbon glyphs (file-row + transfer controls) paint as image meshes.
    assert!(
        image_mesh_count(&shapes) > 0,
        "the Files mode must paint Carbon icons as image meshes"
    );
}

#[test]
fn files_mode_empty_state_is_honest() {
    // No file references projected → an honest empty state, never faked, never a
    // panic. (`Mode::Files` is implemented, so it carries no Phase-3b note.)
    assert!(Mode::Files.is_implemented());
    let space = SpaceId::new();
    let data = FixtureData::new("eagle", 1_000).with_space(space_summary(
        space,
        SpaceKind::Team,
        "Team Ops",
        SpaceRole::Owner,
        0,
        2,
        1_000,
    ));
    assert!(data.file_references(space).is_none());
    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.set_mode(Mode::Files);
    let shapes = render_shapes(&mut surface, &data);
    assert!(!shapes.is_empty(), "the empty Files state painted nothing");
}

#[test]
fn linking_a_picked_file_emits_link_file_with_the_true_content_address() {
    // Picking a canonical file reads + SHA-256-hashes it into a FileRef and emits
    // LinkFile — the honest content address, never a placeholder.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("report.txt");
    std::fs::write(&path, b"hello mesh").expect("write temp file");

    let space = SpaceId::new();
    let mut surface = CommunicationsSurface::new();
    surface.open_file_picker_at(dir.path().to_path_buf());
    assert!(surface.file_picker_open());

    let mut sink = CommandSink::new();
    surface
        .link_file_from_path(&mut sink, space, &path)
        .expect("link the temp file");

    let linked = sink.queued().iter().find_map(|c| match c {
        CollabCommand::LinkFile {
            space: s,
            reference,
            ..
        } => Some((*s, reference.clone())),
        _ => None,
    });
    let (s, reference) = linked.expect("LinkFile emitted");
    assert_eq!(s, space);
    assert_eq!(reference.name, "report.txt");
    assert_eq!(reference.size, 10);
    assert_eq!(
        reference.sha256_hex,
        mde_collab_types::value::sha256_hex(b"hello mesh"),
        "the FileRef carries the real content hash, not a fake"
    );
    assert_eq!(reference.mime.as_deref(), Some("text/plain"));
    // A successful link closes the picker.
    assert!(!surface.file_picker_open());
}

#[test]
fn file_ref_of_path_is_the_real_sha256() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("abc");
    std::fs::write(&path, b"abc").expect("write");
    let (_id, reference) = file_ref_of_path(&path).expect("build ref");
    assert_eq!(
        reference.sha256_hex, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        "known SHA-256('abc')"
    );
    assert_eq!(reference.size, 3);
}

#[test]
fn remove_from_space_emits_unlink_file() {
    // "Remove from space" is a single-click reference removal — UnlinkFile, which
    // removes only the space's reference (the worker leaves the canonical file).
    let space = SpaceId::new();
    let file = FileRefId::new();
    let surface = CommunicationsSurface::new();
    let mut sink = CommandSink::new();
    surface.remove_reference(&mut sink, space, file);
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::UnlinkFile { space: s, file: f }) if *s == space && *f == file
        ),
        "remove-from-space must emit UnlinkFile for the reference"
    );
}

#[test]
fn starting_and_controlling_a_transfer_emits_the_right_commands() {
    let space = SpaceId::new();
    let file = FileRefId::new();
    let surface = CommunicationsSurface::new();

    // Share to members → StartTransfer (outbound, mesh transport).
    let mut sink = CommandSink::new();
    surface.start_transfer_to_members(&mut sink, space, file);
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::StartTransfer {
                space: s,
                file: f,
                direction: TransferDirection::Outbound,
                method: TransferMethod::Node,
                ..
            }) if *s == space && *f == file
        ),
        "share-to-members must emit StartTransfer"
    );

    // A transfer-control action → ControlTransfer (read state from the shared
    // ledger mirror; the control is the collab command).
    let transfer = TransferId::new();
    let mut sink = CommandSink::new();
    surface.control_transfer(&mut sink, transfer, TransferControl::Pause);
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::ControlTransfer {
                transfer: t,
                control: TransferControl::Pause,
            }) if *t == transfer
        ),
        "a transfer-control action must emit ControlTransfer"
    );
}

#[test]
fn permanent_delete_is_typed_confirm_gated() {
    // Permanent delete is distinct from remove-from-space: it fires only after the
    // file's exact name is typed (spec: a separate typed-confirm, not undoable).
    let space = SpaceId::new();
    let file = FileRefId::new();
    let mut surface = CommunicationsSurface::new();
    let mut sink = CommandSink::new();

    surface.request_permanent_delete(file, "secret.txt");
    // Un-typed: must NOT fire.
    assert!(!surface.confirm_permanent_delete(&mut sink, space));
    assert!(
        sink.is_empty(),
        "permanent delete must not fire without the typed confirmation"
    );
    // Wrong text: still must NOT fire.
    surface.set_permanent_delete_typed("wrong.txt");
    assert!(!surface.confirm_permanent_delete(&mut sink, space));
    assert!(
        sink.is_empty(),
        "a mismatched confirmation must not arm the delete"
    );
    // Exact name: fires, as UnlinkFile (the collab primitive; the canonical bytes
    // are then purge-gated once no reference remains).
    surface.set_permanent_delete_typed("secret.txt");
    assert!(surface.confirm_permanent_delete(&mut sink, space));
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::UnlinkFile { space: s, file: f }) if *s == space && *f == file
        ),
        "a confirmed permanent delete must emit UnlinkFile"
    );
}

// ── Transfers mode (WL-FUNC-011) ─────────────────────────────────────────────

#[test]
fn transfers_mode_renders_the_shared_job_list() {
    let space = SpaceId::new();
    let data = FixtureData::new("eagle", 1_000_000)
        .with_space(space_summary(
            space,
            SpaceKind::Team,
            "Team Ops",
            SpaceRole::Owner,
            0,
            2,
            1_000_000,
        ))
        .with_transfer_jobs(TransferJobs {
            jobs: vec![TransferJobView {
                transfer: TransferId::new(),
                file: FileRefId::new(),
                method: TransferMethod::Node,
                direction: TransferDirection::Outbound,
                state: TransferState::Active,
                moved: 1024,
                total: 4096,
            }],
        });
    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.set_mode(Mode::Transfers);
    let shapes = render_shapes(&mut surface, &data);
    assert!(!shapes.is_empty(), "the Transfers job list painted nothing");
    assert!(
        image_mesh_count(&shapes) > 0,
        "the Transfers mode must paint Carbon icons as image meshes"
    );
}

#[test]
fn transfers_mode_empty_state_is_honest() {
    assert!(Mode::Transfers.is_implemented());
    let space = SpaceId::new();
    let data = FixtureData::new("eagle", 1_000).with_space(space_summary(
        space,
        SpaceKind::Team,
        "Team Ops",
        SpaceRole::Owner,
        0,
        2,
        1_000,
    ));
    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.set_mode(Mode::Transfers);
    let shapes = render_shapes(&mut surface, &data);
    assert!(
        !shapes.is_empty(),
        "the empty Transfers state painted nothing"
    );
}

#[test]
fn transfers_mode_control_emits_control_transfer() {
    // The Transfers mode drives the shared control seam; a cancel emits
    // ControlTransfer (state is read from the mirror, never recomputed).
    let surface = CommunicationsSurface::new();
    let transfer = TransferId::new();
    let mut sink = CommandSink::new();
    surface.control_transfer(&mut sink, transfer, TransferControl::Cancel);
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::ControlTransfer {
                transfer: t,
                control: TransferControl::Cancel,
            }) if *t == transfer
        ),
        "a Transfers-mode control must emit ControlTransfer"
    );
}

// ── Alerts mode (WL-FUNC-011) ────────────────────────────────────────────────

/// Build an [`AlertView`] fixture row.
fn alert_view(
    space: SpaceId,
    severity: Severity,
    source: &str,
    headline: &str,
    actions: Vec<AlertAction>,
) -> AlertView {
    let mut fields = BTreeMap::new();
    fields.insert("disk".to_owned(), "94%".to_owned());
    AlertView {
        event_id: EventId::new(),
        space,
        alert: AlertPayload {
            severity,
            source: source.to_owned(),
            headline: headline.to_owned(),
            fields,
            actions,
            goto: None,
        },
        acknowledged: false,
        snoozed_until_unix_ms: None,
    }
}

#[test]
fn alerts_mode_renders_a_fixture_inbox() {
    let space = SpaceId::new();
    let alert = alert_view(
        space,
        Severity::Warning,
        "nyc3",
        "disk pre-fail",
        vec![AlertAction {
            id: "restart".to_owned(),
            label: "Restart".to_owned(),
            verb: Some("action/node/restart".to_owned()),
            kind: AlertActionKind::Destructive,
        }],
    );
    let data = FixtureData::new("eagle", 1_000_000)
        .with_space(space_summary(
            space,
            SpaceKind::Incident,
            "Incident 42",
            SpaceRole::Owner,
            0,
            3,
            1_000_000,
        ))
        .with_alert_inbox(AlertInbox {
            alerts: vec![alert],
        });
    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.set_mode(Mode::Alerts);
    let shapes = render_shapes(&mut surface, &data);
    assert!(!shapes.is_empty(), "the Alerts inbox painted nothing");
    assert!(
        image_mesh_count(&shapes) > 0,
        "the Alerts mode must paint Carbon icons as image meshes"
    );
}

#[test]
fn acknowledge_and_snooze_emit_the_right_commands() {
    let space = SpaceId::new();
    let alert = EventId::new();
    let surface = CommunicationsSurface::new();

    let mut sink = CommandSink::new();
    surface.acknowledge_alert(&mut sink, space, alert);
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::AckAlert { space: s, alert: a }) if *s == space && *a == alert
        ),
        "acknowledge must emit AckAlert"
    );

    let mut sink = CommandSink::new();
    surface.snooze_alert(&mut sink, space, alert, 5_000);
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::SnoozeAlert {
                space: s,
                alert: a,
                until_unix_ms: 5_000,
            }) if *s == space && *a == alert
        ),
        "snooze must emit SnoozeAlert with the injected expiry"
    );
}

#[test]
fn destructive_alert_action_is_arm_then_confirm_gated() {
    // A destructive inline action must not fire until it is armed AND confirmed —
    // mirroring the core's DestructiveNotArmed guard.
    let space = SpaceId::new();
    let alert = EventId::new();
    let mut surface = CommunicationsSurface::new();
    let mut sink = CommandSink::new();

    // No arm → confirm fires nothing.
    assert!(!surface.confirm_alert_action(&mut sink, space));
    assert!(
        sink.is_empty(),
        "an unarmed destructive action must not fire"
    );

    // Arm then confirm → RunAlertAction with armed:true.
    surface.arm_alert_action(alert, "restart".to_owned());
    assert!(surface.confirm_alert_action(&mut sink, space));
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::RunAlertAction {
                space: s,
                alert: a,
                action_id,
                armed: true,
            }) if *s == space && *a == alert && action_id == "restart"
        ),
        "a confirmed destructive action must emit RunAlertAction armed"
    );

    // A safe action fires immediately, unarmed.
    let mut sink = CommandSink::new();
    surface.run_alert_action(&mut sink, space, alert, "open".to_owned(), false);
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::RunAlertAction { armed: false, .. })
        ),
        "a safe action fires unarmed"
    );
}

#[test]
fn mute_threshold_and_dnd_emit_commands_and_hush() {
    let space = SpaceId::new();
    let info = alert_view(space, Severity::Info, "chatty-node", "fyi", vec![]);
    let critical = alert_view(space, Severity::Critical, "core-1", "meltdown", vec![]);
    let mut surface = CommunicationsSurface::new();

    // Threshold at Warning hushes the Info alert but not the Critical one.
    let mut sink = CommandSink::new();
    surface.set_severity_threshold(&mut sink, Severity::Warning);
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::SetSeverityThreshold {
                threshold: Severity::Warning
            })
        ),
        "threshold change must emit SetSeverityThreshold"
    );
    assert!(
        surface.alert_hushed(&info),
        "below-threshold alert is hushed"
    );
    assert!(
        !surface.alert_hushed(&critical),
        "an at/above-threshold alert still rings"
    );

    // Muting a source hushes it regardless of severity.
    let mut sink = CommandSink::new();
    surface.set_alert_mute(&mut sink, "core-1".to_owned(), true);
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::SetAlertMute { source, muted: true }) if source == "core-1"
        ),
        "muting must emit SetAlertMute"
    );
    assert!(
        surface.alert_hushed(&critical),
        "a muted source is hushed even at Critical"
    );

    // DND emits SetDoNotDisturb.
    let mut sink = CommandSink::new();
    surface.set_dnd(&mut sink, true);
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::SetDoNotDisturb { enabled: true })
        ),
        "DND toggle must emit SetDoNotDisturb"
    );
}

// ── Auto Mode / Car Mode (AUTO-COMMS) ────────────────────────────────────────

#[test]
fn car_mode_defaults_to_alerts_and_renders_glanceably() {
    // Entering the Ford SYNC 3 car dash (AutoSync3) while sitting on a dense
    // manage-heavy pane biases a driver onto the glanceable Alerts inbox, and the
    // enlarged frame still renders its Carbon glyphs (no panic).
    let space = SpaceId::new();
    let alert = alert_view(space, Severity::Critical, "core-1", "meltdown", vec![]);
    let data = FixtureData::new("eagle", 1_000_000)
        .with_space(space_summary(
            space,
            SpaceKind::Incident,
            "Incident 42",
            SpaceRole::Owner,
            0,
            3,
            1_000_000,
        ))
        .with_alert_inbox(AlertInbox {
            alerts: vec![alert],
        });

    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    // Sitting on a dense pane (Messages) when Car Mode turns on.
    surface.set_mode(Mode::Messages);
    assert!(Mode::Messages.is_dense());

    let shapes = render_shapes_car(&mut surface, &data);
    assert!(!shapes.is_empty(), "the car-mode frame painted nothing");
    assert!(
        image_mesh_count(&shapes) > 0,
        "the enlarged car-mode Alerts inbox must still paint Carbon icons"
    );
    assert_eq!(
        surface.mode(),
        Mode::Alerts,
        "entering car mode on a dense pane must land the driver on Alerts"
    );

    // The bias is a one-shot default, not a lock: a driver can still navigate to a
    // dense mode and stay there across subsequent car-mode frames.
    surface.set_mode(Mode::Messages);
    let _ = render_shapes_car(&mut surface, &data);
    assert_eq!(
        surface.mode(),
        Mode::Messages,
        "the Alerts bias is a default, never a lock — other modes stay reachable"
    );
}

#[test]
fn moving_car_call_roster_uses_the_glance_budget() {
    assert_eq!(crate::bounded_car_list_len(true, true, 12), 6);
    assert_eq!(crate::bounded_car_list_len(true, true, 4), 4);
    assert_eq!(crate::bounded_car_list_len(true, false, 12), 12);
    assert_eq!(crate::bounded_car_list_len(false, true, 12), 12);
}

#[test]
fn car_mode_leaves_a_non_dense_mode_untouched_on_entry() {
    // The bias only rescues a driver off a dense pane; entering car mode already on
    // a glanceable (non-dense) mode does not force a switch to Alerts.
    let data = FixtureData::demo();
    let mut surface = CommunicationsSurface::new();
    surface.set_mode(Mode::Calls);
    assert!(!Mode::Calls.is_dense());
    let _ = render_shapes_car(&mut surface, &data);
    assert_eq!(
        surface.mode(),
        Mode::Calls,
        "a non-dense mode is left untouched when entering car mode"
    );
}

// ── Clipboard mode (WL-FUNC-011) ─────────────────────────────────────────────

#[test]
fn clipboard_mode_renders_a_fixture_lane() {
    let space = SpaceId::new();
    let data = FixtureData::new("eagle", 1_000_000)
        .with_space(space_summary(
            space,
            SpaceKind::Team,
            "Team Ops",
            SpaceRole::Owner,
            0,
            2,
            1_000_000,
        ))
        .with_clipboard_lane(ClipboardLane {
            space,
            items: vec![ClipboardView {
                event_id: EventId::new(),
                kind: ClipItemKind::Text,
                preview: "deploy token".to_owned(),
                sha256_hex: "b".repeat(64),
                source: "falcon".to_owned(),
                at_unix_ms: 900_000,
                pinned: false,
            }],
        });
    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.set_mode(Mode::Clipboard);
    let shapes = render_shapes(&mut surface, &data);
    assert!(!shapes.is_empty(), "the Clipboard lane painted nothing");
    assert!(
        image_mesh_count(&shapes) > 0,
        "the Clipboard mode must paint Carbon icons as image meshes"
    );
}

#[test]
fn publishing_a_clip_emits_publish_clipboard_with_the_real_hash() {
    let space = SpaceId::new();
    let surface = CommunicationsSurface::new();
    let mut sink = CommandSink::new();
    surface.publish_clip_text(&mut sink, space, "https://example.test/page", "eagle");
    let published = sink.queued().iter().find_map(|c| match c {
        CollabCommand::PublishClipboard {
            space: s,
            text,
            item,
        } => Some((*s, text.clone(), item.clone())),
        _ => None,
    });
    let (s, text, item) = published.expect("PublishClipboard emitted");
    assert_eq!(s, space);
    assert_eq!(
        text, "https://example.test/page",
        "PublishClipboard carries the bounded full text for the canonical clipboard event",
    );
    assert_eq!(item.kind, ClipItemKind::Uri, "an http(s) clip is a URI");
    assert_eq!(item.source, "eagle");
    assert_eq!(
        item.sha256_hex,
        mde_collab_types::value::sha256_hex(b"https://example.test/page"),
        "the clip carries the real content hash, not a fake"
    );
    assert_eq!(item.len, "https://example.test/page".len() as u64);
}

#[test]
fn clip_actions_emit_attach_pin_and_delete() {
    let space = SpaceId::new();
    let clip = EventId::new();
    let surface = CommunicationsSurface::new();

    let mut sink = CommandSink::new();
    surface.attach_clip(&mut sink, space, clip);
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::AttachClipboard { space: s, clip: c }) if *s == space && *c == clip
        ),
        "attach must emit AttachClipboard"
    );

    // Not pinned → toggling pins it.
    let mut sink = CommandSink::new();
    surface.toggle_clip_pin(&mut sink, space, clip, false);
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::PinClipboard { space: s, clip: c }) if *s == space && *c == clip
        ),
        "toggling an unpinned clip must emit PinClipboard"
    );

    // Pinned → toggling unpins it.
    let mut sink = CommandSink::new();
    surface.toggle_clip_pin(&mut sink, space, clip, true);
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::UnpinClipboard { .. })
        ),
        "toggling a pinned clip must emit UnpinClipboard"
    );

    let mut sink = CommandSink::new();
    surface.delete_clip(&mut sink, space, clip);
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::DeleteClipboard { space: s, clip: c }) if *s == space && *c == clip
        ),
        "delete must emit DeleteClipboard"
    );
}

// ── Documents mode (WL-FUNC-011 Phase 3c foundation) ─────────────────────────

/// A one-space fixture with a single document session + its resolved body, so the
/// Documents-mode tests exercise the real read models (never faked data).
fn documents_fixture(space: SpaceId, document: DocumentId, body: &str) -> FixtureData {
    FixtureData::new("eagle", 1_000)
        .with_space(space_summary(
            space,
            SpaceKind::Project,
            "Docs",
            SpaceRole::Owner,
            0,
            2,
            1_000,
        ))
        .with_document_sessions(
            space,
            DocumentSessions {
                sessions: vec![DocumentSession {
                    document,
                    space,
                    title: "Runbook".to_owned(),
                    participants: vec![ActorId::new("eagle"), ActorId::new("falcon")],
                    call: None,
                }],
            },
        )
        .with_document_body(document, body)
}

#[test]
fn documents_mode_renders_in_both_sub_modes() {
    // The Documents mode renders headless in both the default Document sub-mode
    // (the one-pane Markdown editor) and the Project sub-mode (the full embedded
    // IDE), and switching sub-mode/view is real view state.
    let space = SpaceId::new();
    let document = DocumentId::new();
    let data = documents_fixture(space, document, "# Runbook\n\nbody\n");
    let mut surface = CommunicationsSurface::new();
    surface.set_mode(Mode::Documents);

    // Default sub-mode is Document, default view is Source.
    assert_eq!(surface.doc_submode(), DocSubMode::Document);
    assert_eq!(surface.doc_view(), DocView::Source);
    let shapes = render_shapes(&mut surface, &data);
    assert!(
        image_mesh_count(&shapes) > 0,
        "Documents mode must paint Carbon icons as image meshes"
    );

    // Visual view renders the rendered Markdown (still the same mode).
    surface.set_doc_view(DocView::Visual);
    let shapes = render_shapes(&mut surface, &data);
    assert!(!shapes.is_empty(), "the Visual view painted nothing");
    assert_eq!(surface.doc_view(), DocView::Visual);

    // Project sub-mode embeds the full IDE editor and renders.
    surface.set_doc_submode(DocSubMode::Project);
    let shapes = render_shapes(&mut surface, &data);
    assert!(!shapes.is_empty(), "the Project sub-mode painted nothing");
    assert_eq!(surface.doc_submode(), DocSubMode::Project);
}

#[test]
fn opening_a_document_session_loads_its_canonical_markdown() {
    // Opening a fixture DocumentSessions entry loads the resolved canonical
    // Markdown body into the embedded editor — a real load, never faked.
    let space = SpaceId::new();
    let document = DocumentId::new();
    let body = "# Runbook\n\n## Steps\n\n1. deploy\n";
    let data = documents_fixture(space, document, body);

    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.open_document(&data, document, "Runbook");

    assert_eq!(surface.active_document(), Some(document));
    assert_eq!(
        surface.document_editor_text().as_deref(),
        Some(body),
        "the editor must hold the session's resolved canonical Markdown"
    );
}

#[test]
fn saving_a_document_emits_update_document_with_the_canonical_markdown() {
    // Editing + save emits UpdateDocument whose change payload IS the content
    // address of the canonical Markdown (text/markdown) — the Markdown path stays
    // source of truth.
    let space = SpaceId::new();
    let document = DocumentId::new();
    let body = "# Runbook\n\nedited body\n";
    let data = documents_fixture(space, document, body);

    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.open_document(&data, document, "Runbook");

    let mut sink = CommandSink::new();
    assert!(surface.save_document(&mut sink, space), "save must emit");

    let update = sink.queued().iter().find_map(|c| match c {
        CollabCommand::UpdateDocument {
            space: s,
            document: d,
            change,
        } => Some((*s, *d, change.clone())),
        _ => None,
    });
    let (s, d, change) = update.expect("UpdateDocument emitted");
    assert_eq!(s, space);
    assert_eq!(d, document);
    assert_eq!(
        change.payload.sha256_hex,
        mde_collab_types::value::sha256_hex(body.as_bytes()),
        "the UpdateDocument payload must be the content address of the canonical Markdown"
    );
    assert_eq!(change.payload.len, body.len() as u64);
    assert_eq!(
        change.payload.content_type.as_deref(),
        Some("text/markdown"),
        "the canonical payload is Markdown"
    );
}

#[test]
fn document_review_actions_emit_peer_request_and_verdict() {
    let space = SpaceId::new();
    let document = DocumentId::new();
    let data = documents_fixture(space, document, "# Runbook\n");

    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.open_document(&data, document, "Runbook");

    let mut sink = CommandSink::new();
    assert!(surface.request_review(&data, &mut sink, space));
    assert!(matches!(
        sink.queued().first(),
        Some(CollabCommand::RequestReview {
            space: s,
            document: d,
            reviewers,
        }) if *s == space && *d == document && reviewers == &vec![ActorId::new("falcon")]
    ));

    let mut sink = CommandSink::new();
    assert!(surface.submit_review(&mut sink, space, ReviewVerdict::Approved));
    assert!(matches!(
        sink.queued().first(),
        Some(CollabCommand::SubmitReview {
            space: s,
            document: d,
            verdict: ReviewVerdict::Approved,
            comment: None,
        }) if *s == space && *d == document
    ));
}

#[test]
fn new_document_from_a_template_emits_create_document_and_seeds_the_rope() {
    // The New affordance emits CreateDocument and seeds the editor with the
    // template's real Markdown skeleton (a real editable rope, never a locked form).
    let space = SpaceId::new();
    let data = documents_fixture(space, DocumentId::new(), "irrelevant");

    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);

    let mut sink = CommandSink::new();
    let created = surface.new_document(&mut sink, space, DocTemplate::Runbook);

    assert_eq!(surface.active_document(), Some(created));
    let create = sink.queued().iter().find_map(|c| match c {
        CollabCommand::CreateDocument {
            space: s,
            document: d,
            title,
        } => Some((*s, *d, title.clone())),
        _ => None,
    });
    let (s, d, title) = create.expect("CreateDocument emitted");
    assert_eq!(s, space);
    assert_eq!(d, created);
    assert_eq!(title, "Runbook");
    // The rope holds the real template skeleton.
    let text = surface.document_editor_text().unwrap_or_default();
    assert!(
        text.contains("# Runbook") && text.contains("## Rollback"),
        "the new document must be seeded with the Runbook template markdown"
    );
    let _ = data; // fixture only needed for the space selection
}

#[test]
fn markdown_export_returns_the_canonical_markdown() {
    // Markdown is the only export: export_markdown returns the editor's canonical
    // Markdown (the same bytes an UpdateDocument would carry).
    let space = SpaceId::new();
    let document = DocumentId::new();
    let body = "# Doc\n\nexport me\n";
    let data = documents_fixture(space, document, body);

    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.open_document(&data, document, "Doc");

    assert_eq!(
        surface.export_markdown().as_deref(),
        Some(body),
        "the Markdown export path must yield the canonical Markdown"
    );
}

#[test]
fn switching_space_resets_the_picked_document() {
    // The picked document is a per-space intent — a space switch clears it so the
    // new space's sessions drive the picker (no stale document leaks across spaces).
    let a = SpaceId::new();
    let b = SpaceId::new();
    let document = DocumentId::new();
    let data = documents_fixture(a, document, "# A\n");

    let mut surface = CommunicationsSurface::new();
    surface.select_space(a);
    surface.open_document(&data, document, "A");
    assert_eq!(surface.active_document(), Some(document));

    surface.select_space(b);
    assert_eq!(
        surface.active_document(),
        None,
        "a space switch must reset the picked document"
    );
    assert_eq!(
        surface.document_editor_text(),
        None,
        "a space switch must close the previous space's editor so its Markdown cannot remain visible"
    );
}

// ── Calls mode (WL-FUNC-011) ─────────────────────────────────────────────────

/// A one-space fixture with a single active call — the seat (`eagle`) connected,
/// a peer (`falcon`) still ringing — so the Calls roster + both control branches
/// (connected vs ringing) render from real projection data.
fn calls_fixture(space: SpaceId, call: CallId) -> FixtureData {
    let me = ActorId::new("eagle");
    let peer = ActorId::new("falcon");
    FixtureData::new(me.clone(), 2_000_000)
        .with_space(space_summary(
            space,
            SpaceKind::Team,
            "Team Ops",
            SpaceRole::Owner,
            0,
            3,
            1_900_000,
        ))
        .with_call(CallView {
            call,
            space,
            kind: CallKind::Audio,
            started_unix_ms: 1_950_000,
            participants: vec![
                CallParticipantView {
                    actor: me,
                    state: CallParticipantState::Connected,
                    muted: false,
                },
                CallParticipantView {
                    actor: peer,
                    state: CallParticipantState::Ringing,
                    muted: false,
                },
            ],
        })
}

#[test]
fn calls_mode_renders_a_fixture_call_state() {
    // The Calls mode renders the CallState projection headless — the active call,
    // its participants, and the controls — painting Carbon icons as image meshes.
    let space = SpaceId::new();
    let data = calls_fixture(space, CallId::new());
    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.set_mode(Mode::Calls);
    let shapes = render_shapes(&mut surface, &data);
    assert!(!shapes.is_empty(), "the Calls roster painted nothing");
    assert!(
        image_mesh_count(&shapes) > 0,
        "the Calls mode must paint Carbon icons as image meshes"
    );
}

#[test]
fn calls_mode_empty_state_is_honest() {
    // No active call → an honest "No active calls" state (never a faked call),
    // with the start cluster still available. Renders without panic.
    assert!(Mode::Calls.is_implemented());
    let space = SpaceId::new();
    let data = FixtureData::new("eagle", 1_000).with_space(space_summary(
        space,
        SpaceKind::Team,
        "Team Ops",
        SpaceRole::Owner,
        0,
        2,
        1_000,
    ));
    assert!(data.call_state().active.is_empty());
    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.set_mode(Mode::Calls);
    let shapes = render_shapes(&mut surface, &data);
    assert!(!shapes.is_empty(), "the empty Calls state painted nothing");
}

#[test]
fn call_provider_devices_are_visible_disabled_and_honest() {
    let space = SpaceId::new();
    let data = FixtureData::new("eagle", 1_000).with_space(space_summary(
        space,
        SpaceKind::Team,
        "Team Ops",
        SpaceRole::Owner,
        0,
        2,
        1_000,
    ));
    let mut surface = CommunicationsSurface::new();
    surface.select_space(space);
    surface.set_mode(Mode::Calls);
    let texts = painted_text(&render_shapes(&mut surface, &data));

    for expected in [
        "Devices",
        "System default",
        "Showing the system default — live device enumeration and binding a device to the call's media sender arrive with the media plane.",
    ] {
        assert!(
            texts.iter().any(|(text, _)| text == expected),
            "Calls mode must paint the honest disabled provider device row {expected:?}: {texts:?}"
        );
    }
    for fabricated in ["Fake Microphone", "Built-in Camera", "Screen 1"] {
        assert!(
            !texts.iter().any(|(text, _)| text == fabricated),
            "provider devices must not fabricate enumerated hardware: {texts:?}"
        );
    }
}

#[test]
fn settings_surface_shows_provider_devices_without_fake_enumeration() {
    let data = FixtureData::demo();
    let mut surface = CommunicationsSurface::new();
    surface.set_app(MeshTeamsApp::Settings);
    let texts = painted_text(&render_shapes(&mut surface, &data));

    for expected in [
        "Provider devices",
        "Visible but disabled until the live media provider enumerates microphone, camera, and screen sources.",
        "System default",
        "Discord bridge",
        "Unconfigured",
        "No bridge projection",
    ] {
        assert!(
            texts.iter().any(|(text, _)| text == expected),
            "Settings must surface the honest provider state {expected:?}: {texts:?}"
        );
    }
    assert!(
        !texts.iter().any(|(text, _)| text == "Mock Discord Server"),
        "Settings must not fabricate a bridge provider"
    );
}

#[test]
fn settings_surface_shows_discord_bridge_rows_without_provider_calls_or_fake_servers() {
    let space = SpaceId::new();
    let data = FixtureData::new("eagle", 1_000_000)
        .with_space(space_summary(
            space,
            SpaceKind::Team,
            "Team Ops",
            SpaceRole::Owner,
            0,
            4,
            990_000,
        ))
        .with_discord_bridge_board(DiscordBridgeBoard {
            bridges: vec![
                DiscordBridgeView {
                    bridge_id: "unconfigured-row".to_owned(),
                    space: None,
                    label: "Discord bridge not configured".to_owned(),
                    status: DiscordBridgeConfigStatus::Unconfigured,
                    inbound: DiscordBridgeFlowStatus::NotConfigured,
                    outbound: DiscordBridgeFlowStatus::NotConfigured,
                    provenance: DiscordBridgeProvenance {
                        source: DiscordBridgeProvenanceSource::None,
                        authority: None,
                        observed_by: None,
                        config_digest: None,
                    },
                    detail: Some("No operator bridge mapping exists.".to_owned()),
                    updated_unix_ms: 990_000,
                },
                DiscordBridgeView {
                    bridge_id: "provider-unavailable-row".to_owned(),
                    space: Some(space),
                    label: "Ops Discord bridge provider".to_owned(),
                    status: DiscordBridgeConfigStatus::ProviderUnavailable,
                    inbound: DiscordBridgeFlowStatus::ProviderUnavailable,
                    outbound: DiscordBridgeFlowStatus::Degraded,
                    provenance: DiscordBridgeProvenance {
                        source: DiscordBridgeProvenanceSource::WorkerState,
                        authority: Some("mesh-team-revision:43".to_owned()),
                        observed_by: Some("seat-15".to_owned()),
                        config_digest: Some("sha256:provider".to_owned()),
                    },
                    detail: Some("Discord provider adapter unavailable.".to_owned()),
                    updated_unix_ms: 995_000,
                },
                DiscordBridgeView {
                    bridge_id: "configured-row".to_owned(),
                    space: Some(space),
                    label: "Ops Discord bridge".to_owned(),
                    status: DiscordBridgeConfigStatus::Configured,
                    inbound: DiscordBridgeFlowStatus::Ready,
                    outbound: DiscordBridgeFlowStatus::Ready,
                    provenance: DiscordBridgeProvenance {
                        source: DiscordBridgeProvenanceSource::OperatorConfig,
                        authority: Some("mesh-team-revision:44".to_owned()),
                        observed_by: Some("seat-15".to_owned()),
                        config_digest: Some("sha256:configured".to_owned()),
                    },
                    detail: None,
                    updated_unix_ms: 999_000,
                },
            ],
        });
    let mut surface = CommunicationsSurface::new();
    surface.set_app(MeshTeamsApp::Settings);
    let texts = painted_text(&render_shapes_with_size(
        &mut surface,
        &data,
        egui::vec2(1000.0, 1200.0),
    ));

    for expected in [
        "Discord bridge",
        "Discord bridge not configured",
        "Provider unavailable",
        "Configured",
        "Discord → Mesh",
        "Mesh → Discord",
        "Provenance",
        "Operator config · authority mesh-team-revision:44 · observed by seat-15 · config sha256:configured",
        "Discord provider adapter unavailable.",
    ] {
        assert!(
            texts.iter().any(|(text, _)| text == expected),
            "Settings must render the Discord bridge seam row {expected:?}: {texts:?}"
        );
    }
    for fabricated in [
        "Mock Discord Server",
        "Fake Discord Server",
        "General Discord Server",
    ] {
        assert!(
            !texts.iter().any(|(text, _)| text == fabricated),
            "Discord settings must not fabricate external servers: {texts:?}"
        );
    }
}

#[test]
fn call_bar_shows_the_active_call() {
    // The persistent call bar (bottom, mode-independent) renders the active call:
    // in the default Activity mode, a fixture with an active call still paints it.
    let space = SpaceId::new();
    let data = calls_fixture(space, CallId::new());
    let mut surface = CommunicationsSurface::new();
    // Deliberately NOT in Calls mode — the bar is persistent across modes.
    assert_eq!(surface.mode(), Mode::Activity);
    assert!(
        !data.call_state().active.is_empty(),
        "the fixture must carry an active call"
    );
    let shapes = render_shapes(&mut surface, &data);
    assert!(
        image_mesh_count(&shapes) > 0,
        "the persistent call bar must paint the active call's controls"
    );
}

#[test]
fn start_emits_start_call_for_each_kind() {
    // The start cluster emits StartCall with the picked kind for the selected space.
    let space = SpaceId::new();
    let surface = CommunicationsSurface::new();
    for kind in [CallKind::Audio, CallKind::Video, CallKind::Screen] {
        let mut sink = CommandSink::new();
        surface.start_call(&mut sink, space, kind);
        assert!(
            matches!(
                sink.queued().first(),
                Some(CollabCommand::StartCall { space: s, kind: k, .. }) if *s == space && *k == kind
            ),
            "start ({kind:?}) must emit StartCall with that kind"
        );
    }
}

#[test]
fn answer_and_decline_emit_the_right_commands() {
    let call = CallId::new();
    let surface = CommunicationsSurface::new();

    let mut sink = CommandSink::new();
    surface.answer_call(&mut sink, call);
    assert!(
        matches!(sink.queued().first(), Some(CollabCommand::AnswerCall { call: c }) if *c == call),
        "Answer must emit AnswerCall"
    );

    let mut sink = CommandSink::new();
    surface.decline_call(&mut sink, call);
    assert!(
        matches!(sink.queued().first(), Some(CollabCommand::DeclineCall { call: c }) if *c == call),
        "Decline must emit DeclineCall"
    );
}

#[test]
fn hang_up_emits_hang_up_call() {
    let call = CallId::new();
    let surface = CommunicationsSurface::new();
    let mut sink = CommandSink::new();
    surface.hang_up_call(&mut sink, call);
    assert!(
        matches!(sink.queued().first(), Some(CollabCommand::HangUpCall { call: c }) if *c == call),
        "Hang up must emit HangUpCall"
    );
}

#[test]
fn mute_toggle_emits_set_call_muted() {
    let call = CallId::new();
    let surface = CommunicationsSurface::new();
    let mut sink = CommandSink::new();
    surface.set_call_muted(&mut sink, call, true);
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::SetCallMuted { call: c, muted: true }) if *c == call
        ),
        "the mute control must emit SetCallMuted"
    );
}

#[test]
fn dtmf_keypad_emits_send_dtmf() {
    // Opening the in-call keypad is a per-view intent; a press emits SendDtmf.
    let call = CallId::new();
    let mut surface = CommunicationsSurface::new();
    surface.open_dtmf_pad(call);
    assert_eq!(
        surface.dtmf_pad_target(),
        Some(call),
        "opening the keypad must target the call"
    );
    let mut sink = CommandSink::new();
    surface.send_dtmf(&mut sink, call, '5');
    assert!(
        matches!(
            sink.queued().first(),
            Some(CollabCommand::SendDtmf { call: c, digit: '5' }) if *c == call
        ),
        "a DTMF keypad press must emit SendDtmf"
    );
}

#[test]
fn no_recording_or_transcription_control_exists_anywhere() {
    // Spec §7: recording + transcription are deliberately absent from the UI, the
    // icon standard, and the call commands. No glyph names them...
    for name in ALL_COLLAB_ICONS {
        let n = name.to_ascii_lowercase();
        assert!(
            !n.contains("record") && !n.contains("transcri"),
            "no Carbon glyph may name recording/transcription (found {name:?})"
        );
    }
    // ...and every call command the surface can emit is a call-control verb, never
    // a record/transcribe one.
    let call = CallId::new();
    let space = SpaceId::new();
    let commands = [
        CollabCommand::StartCall {
            space,
            call,
            kind: CallKind::Audio,
        },
        CollabCommand::AnswerCall { call },
        CollabCommand::DeclineCall { call },
        CollabCommand::HangUpCall { call },
        CollabCommand::SetCallMuted { call, muted: true },
        CollabCommand::SendDtmf { call, digit: '1' },
    ];
    for c in commands {
        let verb = c.verb();
        assert!(
            !verb.contains("record") && !verb.contains("transcri"),
            "no call command may be a recording/transcription verb (found {verb:?})"
        );
    }
}
