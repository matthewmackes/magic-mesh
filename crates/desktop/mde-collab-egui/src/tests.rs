//! Headless fixture tests for the Communications surface: the frame renders from
//! a fixture directory, the Messages composer's <kbd>Enter</kbd> emits a
//! `SendMessage`, the amend affordance follows the author window, the Activity
//! feed filters, and every icon paints as a real Carbon image mesh (not glyph
//! text) — mirroring the browser chrome's Carbon idiom.

#![allow(clippy::unwrap_used, clippy::panic, clippy::float_cmp)]

use mde_egui::egui;
use mde_egui::Style;

use mde_collab_types::{
    ActivityFeed, ActorId, CollabCommand, ConversationTimeline, DeliveryState, EventId, FileRef,
    FileRefId, FileReferenceView, FileReferences, SpaceId, SpaceKind, SpaceRole, TransferControl,
    TransferDirection, TransferId, TransferJobView, TransferJobs, TransferMethod, TransferState,
};

use crate::fixture::{activity, message, space_summary, FixtureData};
use crate::{
    amend_affordance, file_ref_of_path, ActivityFilter, AmendAffordance, CollabData, CommandSink,
    CommunicationsSurface, Mode, ALL_COLLAB_ICONS, EDIT_WINDOW_MS,
};

/// A `1000 x 700` headless input with the given events.
fn sized_input(events: Vec<egui::Event>) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1000.0, 700.0),
        )),
        events,
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

/// Render one frame of `surface` against `data` and return the painted shapes.
fn render_shapes(
    surface: &mut CommunicationsSurface,
    data: &dyn CollabData,
) -> Vec<egui::epaint::ClippedShape> {
    let ctx = egui::Context::default();
    Style::install(&ctx);
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
fn typing_then_enter_emits_send_message() {
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

    // Pass 1: lay the composer out, then focus it by its stable id.
    let mut sink = CommandSink::new();
    let _ = ctx.run(sized_input(Vec::new()), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| surface.ui(ui, &data, &mut sink));
    });
    ctx.memory_mut(|m| m.request_focus(edit_id));

    // Pass 2: type into the focused composer and press Enter.
    let mut sink = CommandSink::new();
    let events = vec![
        egui::Event::Text("hello mesh".to_owned()),
        key(egui::Key::Enter),
    ];
    let _ = ctx.run(sized_input(events), |ctx| {
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
        "typing then Enter must emit SendMessage with the typed body in the selected space"
    );
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
fn labeled_for_later_modes_are_honest() {
    // Documents/Alerts/Clipboard are placeholders, not faked data — they carry a
    // Phase-3b note and are not marked implemented. (Files graduated to a full
    // mode with WL-FUNC-011 and is asserted implemented below.)
    for mode in [Mode::Documents, Mode::Alerts, Mode::Clipboard] {
        assert!(!mode.is_implemented(), "{mode:?} is a Phase-3b placeholder");
        assert!(
            mode.phase_3b_note().contains("Phase 3b"),
            "{mode:?} must carry an honest labeled-for-later note"
        );
    }
    assert!(
        Mode::Activity.is_implemented()
            && Mode::Messages.is_implemented()
            && Mode::Files.is_implemented()
    );
    // An implemented mode never renders a placeholder note.
    assert_eq!(Mode::Files.phase_3b_note(), "");
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
