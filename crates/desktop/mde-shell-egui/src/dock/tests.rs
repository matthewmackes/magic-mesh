use super::{
    action_center_cell_id, clock_cell_id, clock_date_text, desktop_source_toggle_id,
    focus_ring_rect, notification_rail_with_sources, pinned_app_cell_id, session_entry_id,
    session_hover_preview_id, session_hover_protocol_badge_id, session_preview_texture_for_entry,
    session_preview_texture_rect, show_desktop_nub_id, start_cell_id, status_detail_toggle_id,
    taskbar_reveal, tray_overflow_id, tray_overflow_popup_id, tray_overflow_row_id,
    truncate_session_label, win11_tray_island_rect, DesktopRailSource, DockState,
    FileOperationProgress, SessionPreviewTexture, SessionRailEntry, Surface, ACTION_CENTER_ICON,
    CELL_W, DESKTOP_SOURCE_TOGGLE_ICON, DOCK_W, FOCUS_RING_W, NOTIFICATION_RAIL_H,
    STATUS_DETAIL_ICON, TASKBAR_TRAY_ISLAND_ACTIVE_FILL, TASKBAR_TRAY_ISLAND_FILL,
    TRAY_OVERFLOW_ICON, TRAY_OVERFLOW_ROW_H, TRAY_OVERFLOW_W,
};
use crate::chrome::{GradeRow, GradeTrend, MeshSummary, NodeGrades};
use crate::status::{self, StatusSegments};
use mde_egui::Style;
use mde_egui::{egui, Density};
use mde_lighthouse_health::LighthouseHealth;
use mde_theme::brand::icons::{icon_image, IconId};

/// One grade row at a chosen host / score / pin / staleness (steady trend).
fn grade(host: &str, score: u8, is_local: bool, stale: bool) -> GradeRow {
    GradeRow {
        host: host.to_owned(),
        score,
        trend: GradeTrend::Steady,
        is_local,
        stale,
    }
}

/// A seen grade set in the given (already-sorted) render order — the render
/// preserves the order `chrome::NodeGrades::fold` produced.
fn grades(rows: Vec<GradeRow>) -> NodeGrades {
    NodeGrades { rows, seen: true }
}

#[test]
fn taskbar_session_label_truncation_uses_ascii_ellipsis() {
    let label = truncate_session_label("This is a very long production desktop session name");
    assert!(
        label.ends_with("..."),
        "long labels use ASCII ellipsis: {label}"
    );
    assert!(
        !label.contains('…'),
        "taskbar chrome must not render a Unicode ellipsis in truncated session labels: {label}"
    );
    assert_eq!(
        truncate_session_label("Short VM"),
        "Short VM",
        "short labels are left unchanged"
    );
}

/// a11y-03 (WCAG 2.4.7 *Focus Visible*): the shared keyboard-focus-ring seam
/// [`focus_ring_rect`] produces a ring rect ONLY for the focused cell, and never
/// for an unfocused one — the pure decision every raw-painted dock/taskbar/picker
/// cell routes its `resp.has_focus()` through before [`super::paint_focus_ring`]
/// strokes it. Exercised without a live painter so the "focused rings, rest don't"
/// contract is guarded directly.
#[test]
fn focus_ring_only_rings_the_focused_cell() {
    let cell = egui::Rect::from_min_size(egui::pos2(40.0, 120.0), egui::vec2(48.0, 48.0));

    // Unfocused: no ring at all (the WCAG regression this fix closes — the focus
    // was invisible because nothing was painted).
    assert_eq!(
        focus_ring_rect(cell, false),
        None,
        "an unfocused cell must not paint a focus ring"
    );

    // Focused: a ring rect, inset by half the stroke so a FOCUS_RING_W-wide stroke
    // lands fully INSIDE the cell (never bleeds into a neighbouring cell).
    let ring = focus_ring_rect(cell, true).expect("a focused cell must ring");
    let inset = FOCUS_RING_W / 2.0;
    assert!(
        (ring.min.x - (cell.min.x + inset)).abs() < f32::EPSILON
            && (ring.min.y - (cell.min.y + inset)).abs() < f32::EPSILON
            && (ring.max.x - (cell.max.x - inset)).abs() < f32::EPSILON
            && (ring.max.y - (cell.max.y - inset)).abs() < f32::EPSILON,
        "the ring must be the cell inset by half the stroke, got {ring:?}"
    );
    // And it stays within the cell on every edge (the "never bleeds" guarantee).
    assert!(
        cell.contains_rect(ring),
        "the focus ring must sit inside its cell"
    );
    // The ring wears the theme's lifted brand accent (no dedicated focus token
    // exists in the Construct palette) — one rung brighter than the resting accent.
    assert_ne!(Style::ACCENT_HI, Style::ACCENT);
}

#[test]
fn the_dock_lists_the_workbench_vm_surfaces_app_surfaces_and_info_surfaces() {
    // Nineteen entries: Workbench first, the live Mesh Map (OW-10, `mde-mesh-view`),
    // the brokered Desktop surface, the app surfaces (Music / Media — the
    // full media player, MEDIA-18 / Files / Voice / Browser — the sandboxed Servo
    // browser, BOOKMARKS-6 / Bookmarks / Maps & Location / Terminal — the
    // Terminator-class terminal over a real PTY, TERM-16 / Editor — the native
    // Zed-style code editor, EDITOR-1), the
    // unified Chat surface (the ONE notification interface — the standalone
    // Notifications + Clipboard surfaces are retired, NOTIFY-CHAT-6), the Phones
    // hub (KDC-MESH-9 — the desktop-side paired-phone manager), the host-controls
    // System surface, the Storage surface (GParted-authentic disk mgmt, E12-21),
    // and the About surface (the platform-identity screen, QBRAND-6).
    assert_eq!(Surface::ALL.len(), 19);
    assert_eq!(Surface::ALL[0], Surface::Workbench);
    for s in [
        Surface::MeshView,
        Surface::Explorer,
        Surface::InfraCode,
        Surface::Desktop,
        Surface::Music,
        Surface::Media,
        Surface::Files,
        Surface::Voice,
        Surface::Browser,
        Surface::Bookmarks,
        Surface::MapsLocation,
        Surface::Terminal,
        Surface::Editor,
        Surface::Chat,
        // The Phones hub (KDC-MESH-9) — the desktop-side paired-phone manager.
        Surface::Phones,
        Surface::System,
        Surface::Storage,
        Surface::About,
    ] {
        assert!(Surface::ALL.contains(&s), "{s:?} missing from the dock");
    }
}

#[test]
fn the_shell_opens_on_remote_sessions() {
    assert_eq!(Surface::default(), Surface::Desktop);
}

// --- QBRAND-7: every dock surface renders a brand::icons glyph ----------------

#[test]
fn every_surface_maps_to_a_named_brand_glyph() {
    // The map is 1:1 by name (Workbench→Workbench … MeshView→MeshView), and no
    // surface folds onto the blank text wordmark.
    let cases = [
        (Surface::Workbench, IconId::Workbench),
        (Surface::MeshView, IconId::MeshView),
        (Surface::Explorer, IconId::Instances),
        (Surface::InfraCode, IconId::Server),
        (Surface::Desktop, IconId::Desktop),
        (Surface::Music, IconId::Music),
        (Surface::Media, IconId::Media),
        (Surface::Files, IconId::Files),
        (Surface::Voice, IconId::Voice),
        (Surface::Browser, IconId::Browser),
        (Surface::Bookmarks, IconId::Bookmarks),
        (Surface::MapsLocation, IconId::MapsLocation),
        (Surface::Terminal, IconId::Terminal),
        (Surface::Editor, IconId::Editor),
        (Surface::Chat, IconId::Chat),
        // The Phones hub wears its own smartphone glyph (KDC-MESH-9).
        (Surface::Phones, IconId::Phones),
        // The System surface is the right-side Settings button — the cog glyph.
        (Surface::System, IconId::Settings),
        (Surface::Storage, IconId::Storage),
        (Surface::About, IconId::Mark),
    ];
    assert_eq!(cases.len(), Surface::ALL.len(), "a surface is unmapped");
    for (surface, id) in cases {
        assert_eq!(surface.icon_id(), id, "{surface:?} → wrong glyph");
        assert_ne!(
            id,
            IconId::Wordmark,
            "{surface:?} maps to the blank wordmark"
        );
    }
    // The map is injective — 19 surfaces, 19 distinct glyph names (IaC wears
    // the Server badge, Explorer the stacked-cards Instances glyph, each
    // unshared by any other surface).
    let mut names: Vec<&str> = Surface::ALL.iter().map(|s| s.icon_id().name()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), Surface::ALL.len(), "surface→glyph map not 1:1");
}

// --- WIN7-3: every dock surface has a tile-facing display label ---------------

#[test]
fn every_surface_maps_to_a_nonempty_display_label() {
    // `label()` is new data (WIN7-3) — the picker itself deliberately has no
    // per-icon caption to inherit (PICKER-1's own "no per-icon captions, no
    // tooltips anywhere" lock), so this is its own exhaustive, injective map,
    // the same shape as `every_surface_maps_to_a_named_brand_glyph` above.
    let cases = [
        (Surface::Workbench, "Workbench"),
        (Surface::MeshView, "Mesh Map"),
        (Surface::Explorer, "Explorer"),
        (Surface::InfraCode, "Infra as Code"),
        (Surface::Desktop, "Remote Sessions"),
        (Surface::Music, "Music"),
        (Surface::Media, "Media"),
        (Surface::Files, "Files"),
        (Surface::Voice, "Voice"),
        (Surface::Browser, "Browser"),
        (Surface::Bookmarks, "Bookmarks"),
        (Surface::MapsLocation, "Maps & Location"),
        (Surface::Terminal, "Terminal"),
        (Surface::Editor, "Editor"),
        (Surface::Chat, "Chat"),
        (Surface::Phones, "Phones"),
        (Surface::System, "System"),
        (Surface::Storage, "Storage"),
        (Surface::About, "About"),
    ];
    assert_eq!(cases.len(), Surface::ALL.len(), "a surface is unlabelled");
    for (surface, label) in cases {
        assert_eq!(surface.label(), label, "{surface:?} → wrong label");
        assert!(!label.is_empty(), "{surface:?} has a blank label");
    }
    // Injective over Surface::ALL — 19 surfaces, 19 distinct labels.
    let mut labels: Vec<&str> = Surface::ALL.iter().map(|s| s.label()).collect();
    labels.sort_unstable();
    labels.dedup();
    assert_eq!(
        labels.len(),
        Surface::ALL.len(),
        "surface→label map not 1:1"
    );
    // Timers sits outside `ALL` (lock #20 — the clock-cell glyph, never a
    // tile) but `label()` stays exhaustive over the full enum like `icon_id`.
    assert_eq!(Surface::Timers.label(), "Timers & Alarms");
}

#[test]
fn every_surface_glyph_rasterizes_nonempty() {
    // Each surface's glyph resolves to real ink through the shared loader,
    // tinted by a Style token (no raw hex) — so the bar never draws an empty
    // square.
    let tint = Style::TEXT_DIM.to_array();
    for surface in Surface::ALL {
        let img = icon_image(surface.icon_id(), 32, tint).expect("surface glyph rasterizes");
        let inked = img.rgba.chunks_exact(4).filter(|px| px[3] > 0).count();
        assert!(inked > 0, "{surface:?} glyph rasterized empty");
    }
}

// --- PICKER-1: the group table + rotated labels + hairline dividers -----------

// ── VDOCK-1: the left vertical dock frame + auto-hide ─────────────────────

/// Drive ONE headless frame of the vertical dock over a stand-in surface at a
/// given screen `size`, feeding `events` — the routing/overflow harness core
/// (the same `Context::run` path the DRM runner drives, minus the GPU).
fn drive_vdock(
    ctx: &egui::Context,
    state: &mut DockState,
    events: Vec<egui::Event>,
    size: egui::Vec2,
) -> egui::FullOutput {
    drive_vdock_with_sources(ctx, state, events, size, &[])
}

/// Returns the frame's [`egui::FullOutput`] (WIN7-7: the accesskit tests
/// below need `platform_output.accesskit_update`, the `start_menu.rs`
/// `drive` precedent) — existing callers that ignore it keep compiling
/// unchanged (`FullOutput` isn't `#[must_use]`).
fn drive_vdock_with_sources(
    ctx: &egui::Context,
    state: &mut DockState,
    events: Vec<egui::Event>,
    size: egui::Vec2,
    sources: &[DesktopRailSource],
) -> egui::FullOutput {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), size)),
        events,
        ..Default::default()
    };
    ctx.run(input, |ctx| {
        // A stand-in surface beneath the dock (the background layer).
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = ui.button("surface");
        });
        let _ = notification_rail_with_sources(ctx, state, sources);
    })
}

fn capture_taskbar_screenshot(
    name: &str,
    state: &mut DockState,
    size: egui::Vec2,
    sources: &[DesktopRailSource],
) -> crate::screenshot::Canvas {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        ..Default::default()
    };
    let mut cap = crate::screenshot::Capture::new();
    let _settle = cap.frame(&ctx, input(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label("surface");
        });
        let _ = notification_rail_with_sources(ctx, state, sources);
    });
    let canvas = cap.frame(&ctx, input(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label("surface");
        });
        let _ = notification_rail_with_sources(ctx, state, sources);
    });

    assert_eq!(
        (canvas.width(), canvas.height()),
        (size.x.round() as usize, size.y.round() as usize),
        "taskbar screenshot canvas must match the driven viewport"
    );
    assert!(!canvas.is_blank(), "taskbar screenshot must not be blank");

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("screenshots")
        .join(name);
    canvas
        .write_png(&path)
        .expect("write the taskbar visual-proof screenshot");
    println!(
        "taskbar visual-proof screenshot written to {}",
        path.display()
    );
    canvas
}

#[test]
fn the_vertical_dock_is_a_48px_full_height_column() {
    // Locks #2/#23 — the dock is one 48px-wide column, sharing the horizontal
    // taskbar's 48px icon-cell module (so VDOCK-2/3/4 inherit the grid).
    assert!((DOCK_W - 48.0).abs() < f32::EPSILON, "dock width ~48px");
    assert!(
        (DOCK_W - CELL_W).abs() < f32::EPSILON,
        "dock shares the taskbar cell module"
    );
}

// ── WIN7-2: the Start Menu's dock cell (CONSOLE-1's original front door) ──

// ── DOCK-OVERLAP: the shell reserves a gutter so the dock never overlaps ──

// ── VDOCK-2: the vertical app picker (top + middle zones) ─────────────────

fn key(k: egui::Key) -> egui::Event {
    egui::Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::default(),
    }
}

// ── VDOCK-5: the clock strip (Timers & Alarms home, locks #16/#20) ─────────

#[test]
fn the_win10_clock_second_line_is_the_civil_date() {
    // WIN10-HYBRID — the tray clock's second line is the M/D/YYYY civil date via the
    // crate's ONE calendar. Anchor on the Unix epoch + a known later day.
    assert_eq!(clock_date_text(0), "1/1/1970", "epoch is 1970-01-01");
    // 2026-07-12 00:00 UTC = 20_646 days since the epoch.
    assert_eq!(clock_date_text(20_646 * 86_400), "7/12/2026");
    // Time-of-day within a day does not roll the date.
    assert_eq!(clock_date_text(20_646 * 86_400 + 23 * 3600), "7/12/2026");
}

// ── NOTIF-3: the status strip wired into the dock's bottom zone ────────────

// ── Bottom taskbar layout ────────────────────────────────────────────────
// The left edge starts with Desktop Sources now that the taskbar Start button is
// retired; the right edge carries the Windows 11-style tray island.

#[test]
fn taskbar_reads_desktop_sources_sessions_tray_clock_left_to_right() {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    let entry = SessionRailEntry::with_session_id("session-1", "Accounting VM", "RDP");
    s.set_status_inputs(
        MeshSummary::default(),
        None,
        0,
        true,
        vec![entry.clone()],
        NodeGrades::default(),
        StatusSegments::default(),
    );
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let source = ctx
        .read_response(desktop_source_toggle_id())
        .expect("Desktop Sources cell registered")
        .rect;
    let session = ctx
        .read_response(session_entry_id(0, &entry))
        .expect("session entry registered")
        .rect;
    let tray = ctx
        .read_response(status::segment_pip_id(status::StatusSegment::Alerts))
        .expect("tray pip registered")
        .rect;
    let clock = ctx
        .read_response(clock_cell_id())
        .expect("clock registered")
        .rect;

    assert!(
        source.right() <= session.left() + 1.0,
        "Desktop Sources sits left of the running-sessions run"
    );
    assert!(
        session.right() <= tray.left() + 1.0,
        "sessions sit left of the tray"
    );
    assert!(
        tray.right() <= clock.left() + 1.0,
        "the tray sits left of the clock"
    );
    for (label, r) in [("session", session), ("tray", tray), ("clock", clock)] {
        assert!(
            (r.center().y - source.center().y).abs() < 2.0,
            "{label} shares the Desktop Sources row"
        );
    }
}

#[test]
fn taskbar_no_longer_renders_or_latches_a_far_left_start_button() {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    assert!(
        ctx.read_response(start_cell_id()).is_none(),
        "the retired far-left Start button must not register a taskbar response"
    );
    let source = ctx
        .read_response(desktop_source_toggle_id())
        .expect("Desktop Sources cell becomes the first taskbar control")
        .rect
        .left();
    assert!(
        source <= CELL_W,
        "Desktop Sources should occupy the left-edge taskbar slot after Start removal: {source}"
    );
    assert!(
        !s.take_start_menu_toggle(),
        "the taskbar must not synthesize a retired Start launcher toggle"
    );
}

#[test]
fn win10_the_taskbar_is_a_fixed_48px_height_across_densities() {
    // WIN10-HYBRID — the bottom taskbar matches the Windows-10 taskbar: a fixed 48px
    // height, density-independent (density scales spacing + the hit-target floor,
    // never this chrome dimension — lock #7 / UX-24). Pins the value so a future
    // change is a conscious edit here.
    assert!(
        (NOTIFICATION_RAIL_H - 48.0).abs() < f32::EPSILON,
        "the Win10 taskbar is a fixed 48px"
    );
    for d in [
        Density::Compact,
        Density::Mouse,
        Density::Comfortable,
        Density::Touch,
    ] {
        let mut s = DockState::default();
        s.set_density(d);
        assert!(
            (s.rail_height() - 48.0).abs() < f32::EPSILON,
            "{d:?} density still drives the fixed 48px taskbar"
        );
    }
    // At 48px the bar equals the DOCK_W left-dock column (both 48) — the left dock
    // retires into this single taskbar in B4.
    assert!(
        (NOTIFICATION_RAIL_H - DOCK_W).abs() < f32::EPSILON,
        "the taskbar and the (retiring) left dock column share the 48px module"
    );
}

// ── WIN10-HYBRID #31: the Win10 tray affordances (action-center + nub) ─────
// The right cluster grows two Win10 idioms: an **action-center** cell that
// routes to the unified Chat notification feed, and a far-right **show-desktop
// nub** that minimizes to the Desktop surface. These pin their routing targets
// + the non-overlap contract that keeps them clear of the running-sessions run.

/// Press-then-release a primary click at `pos` over the driven bottom rail,
/// Prime, move+press one frame, release the next so egui registers a real rail
/// click.
fn click_rail_cell(ctx: &egui::Context, s: &mut DockState, pos: egui::Pos2, sz: egui::Vec2) {
    let press = egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::default(),
    };
    let release = egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::default(),
    };
    drive_vdock(ctx, s, vec![egui::Event::PointerMoved(pos), press], sz);
    drive_vdock(ctx, s, vec![egui::Event::PointerMoved(pos), release], sz);
}

#[test]
fn pinned_apps_from_the_start_menu_render_on_the_application_bar() {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.set_active(Surface::Workbench);
    s.set_pinned_surfaces(&[Surface::Browser, Surface::Files]);
    let entry = SessionRailEntry::with_session_id("session-1", "Accounting VM", "RDP");
    s.set_status_inputs(
        MeshSummary::default(),
        None,
        0,
        true,
        vec![entry.clone()],
        NodeGrades::default(),
        StatusSegments::default(),
    );
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let source = ctx
        .read_response(desktop_source_toggle_id())
        .expect("Desktop Sources cell registered")
        .rect;
    let browser = ctx
        .read_response(pinned_app_cell_id(Surface::Browser))
        .expect("pinned Browser app registered on the application bar")
        .rect;
    let files = ctx
        .read_response(pinned_app_cell_id(Surface::Files))
        .expect("pinned Files app registered on the application bar")
        .rect;
    let session = ctx
        .read_response(session_entry_id(0, &entry))
        .expect("session entry registered")
        .rect;

    assert!(
        source.right() <= browser.left() + 1.0,
        "pinned apps sit after the Desktop Sources control"
    );
    assert!(
        browser.right() <= files.left() + 1.0,
        "pinned apps keep Start Menu pin order"
    );
    assert!(
        files.right() <= session.left() + 1.0,
        "running sessions remain after pinned applications"
    );

    let center = browser.center();
    click_rail_cell(&ctx, &mut s, center, sz);
    assert_eq!(
        s.active(),
        Surface::Browser,
        "clicking a pinned application bar cell routes to that app surface"
    );
}

#[test]
fn win10_hybrid_31_the_action_center_cell_routes_to_chat() {
    // The action-center tray cell IS the Win10 notification button: a click
    // routes the shell body to the unified Chat feed (NOTIFY-CHAT).
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle(); // reveal the rail so its cells mount
    let sz = egui::vec2(1280.0, 800.0);
    // Prime two frames so egui registers the cell's rect.
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    assert_ne!(s.active, Surface::Chat, "start off the Chat surface");
    let center = ctx
        .read_response(action_center_cell_id())
        .expect("the action-center cell is registered")
        .rect
        .center();
    click_rail_cell(&ctx, &mut s, center, sz);
    assert_eq!(
        s.active,
        Surface::Chat,
        "clicking the action-center cell opens the Chat notification feed"
    );
}

#[test]
fn win10_hybrid_31_the_show_desktop_nub_routes_to_desktop() {
    // The far-right show-desktop nub minimizes to the Desktop (VDI) surface,
    // Win10's "show desktop" corner. Start from another active surface so a nub
    // click is an observable route back to Remote Sessions.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.set_active(Surface::Browser);
    s.toggle(); // reveal the rail so its cells mount
    let sz = egui::vec2(1280.0, 800.0);
    // Prime two frames so egui registers the nub's rect (matching the pin/action
    // tests — one frame is not enough for the click to land).
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    assert_ne!(s.active, Surface::Desktop, "start off Remote Sessions");
    let center = ctx
        .read_response(show_desktop_nub_id())
        .expect("the show-desktop nub is registered")
        .rect
        .center();
    click_rail_cell(&ctx, &mut s, center, sz);
    assert_eq!(
        s.active,
        Surface::Desktop,
        "clicking the show-desktop nub minimizes to the Desktop surface"
    );
}

#[test]
fn win10_hybrid_31_the_new_tray_cells_do_not_overlap_the_sessions_run() {
    // The action-center cell + the show-desktop nub extend the right cluster;
    // `right_cluster_w` must grow to match so the running-sessions run (bounded by
    // `session_right`) never slides under them — the same overlap contract the
    // clock/pips/detail already rely on. Driven at the locked 48px taskbar height.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    assert!(
        (s.rail_height() - 48.0).abs() < f32::EPSILON,
        "the default taskbar is the locked 48px"
    );
    let entry = SessionRailEntry::with_session_id("session-1", "Accounting VM", "RDP");
    s.set_status_inputs(
        MeshSummary::default(),
        None,
        3, // unread > 0 → the action-center wears its accent cue
        true,
        vec![entry.clone()],
        NodeGrades::default(),
        StatusSegments::default(),
    );
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let session = ctx
        .read_response(session_entry_id(0, &entry))
        .expect("session entry registered")
        .rect;
    let status_detail = ctx
        .read_response(status_detail_toggle_id())
        .expect("status-detail toggle registered")
        .rect;
    let action = ctx
        .read_response(action_center_cell_id())
        .expect("action-center registered")
        .rect;
    let nub = ctx
        .read_response(show_desktop_nub_id())
        .expect("show-desktop nub registered")
        .rect;
    let clock = ctx
        .read_response(clock_cell_id())
        .expect("clock registered")
        .rect;

    // The leftmost right-cluster cell is the status-detail toggle; the sessions
    // run must end to its left (session_right reserves the WHOLE cluster).
    assert!(
        session.right() <= status_detail.left() + 1.0,
        "the sessions run clears the leftmost right-cluster cell"
    );
    assert!(
        session.right() <= action.left() + 1.0,
        "the sessions run never slides under the action-center cell"
    );
    // The right-edge cells slot in cleanly: clock · action-center · nub, and the
    // nub hugs the taskbar's very right edge (Win10's show-desktop corner).
    assert!(
        clock.right() <= action.left() + 1.0,
        "the action-center sits right of the clock"
    );
    assert!(
        action.right() <= nub.left() + 1.0,
        "the show-desktop nub trails past the action-center"
    );
    assert!(
        (nub.right() - sz.x).abs() < 1.0,
        "the show-desktop nub is pinned to the taskbar's very right edge"
    );
    // All on the one 48px row.
    for (label, r) in [("action-center", action), ("nub", nub)] {
        assert!(
            (r.center().y - session.center().y).abs() < 2.0,
            "{label} shares the taskbar row"
        );
    }
}

#[test]
fn win10_hybrid_31_session_hover_preview_shows_protocol_badge() {
    // Hovering a running session mounts the first static thumbnail slice: label +
    // real protocol badge above the taskbar. Live frame texture plumbing is a
    // later B5-rest item.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    let entry = SessionRailEntry::with_session_id("session-1", "Accounting VM", "RDP");
    s.set_status_inputs(
        MeshSummary::default(),
        None,
        0,
        true,
        vec![entry.clone()],
        NodeGrades::default(),
        StatusSegments::default(),
    );
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let session = ctx
        .read_response(session_entry_id(0, &entry))
        .expect("session entry registered")
        .rect;
    drive_vdock(
        &ctx,
        &mut s,
        vec![egui::Event::PointerMoved(session.center())],
        sz,
    );
    drive_vdock(
        &ctx,
        &mut s,
        vec![egui::Event::PointerMoved(session.center())],
        sz,
    );

    let preview = ctx
        .read_response(session_hover_preview_id(0, &entry))
        .expect("session hover preview registered")
        .rect;
    let badge = ctx
        .read_response(session_hover_protocol_badge_id(0, &entry))
        .expect("session protocol badge registered")
        .rect;
    assert!(
        preview.bottom() <= session.top(),
        "the hover preview opens above the taskbar session tile"
    );
    assert!(
        preview.contains(badge.center()),
        "the protocol badge is mounted inside the hover preview"
    );
    assert!(
        badge.center().x > preview.center().x,
        "the protocol badge sits on the compact right side of the preview"
    );
}

#[test]
fn win10_hybrid_31_session_hover_preview_clips_long_titles_to_card_body() {
    // The hover preview is a fixed-width taskbar card. A long VM name must stay
    // clipped to that card body instead of painting into neighboring chrome.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    let entry =
        SessionRailEntry::with_session_id("session-1", "WWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWW", "RDP");
    let title = entry.label.clone();
    s.set_status_inputs(
        MeshSummary::default(),
        None,
        0,
        true,
        vec![entry.clone()],
        NodeGrades::default(),
        StatusSegments::default(),
    );
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let session = ctx
        .read_response(session_entry_id(0, &entry))
        .expect("session entry registered")
        .rect;
    drive_vdock(
        &ctx,
        &mut s,
        vec![egui::Event::PointerMoved(session.center())],
        sz,
    );
    let out = drive_vdock(
        &ctx,
        &mut s,
        vec![egui::Event::PointerMoved(session.center())],
        sz,
    );

    let preview = ctx
        .read_response(session_hover_preview_id(0, &entry))
        .expect("session hover preview registered")
        .rect;
    let (_, clip) = painted_text_clips(&out.shapes)
        .into_iter()
        .find(|(text, _)| text == &title)
        .unwrap_or_else(|| panic!("preview title text was not painted: {title}"));
    assert!(
        clip.left() >= preview.left() + Style::SP_S - f32::EPSILON
            && clip.right() <= preview.right() - Style::SP_S + f32::EPSILON,
        "the title clip must honor the card's horizontal padding: \
         preview={preview:?} clip={clip:?}"
    );
    assert!(
        clip.width() <= preview.width() - Style::SP_M + f32::EPSILON,
        "the title clip must be narrower than the fixed hover card body: \
         preview={preview:?} clip={clip:?}"
    );
    assert!(
        clip.height() < preview.height(),
        "the title clip should cover only the text band, not the whole hover card: \
         preview={preview:?} clip={clip:?}"
    );
}

#[test]
fn win10_hybrid_31_session_preview_texture_preserves_aspect_inside_thumbnail() {
    let bounds = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(180.0, 72.0));
    let wide = session_preview_texture_rect([160, 90], bounds);
    assert!(
        wide.height() <= bounds.height() && wide.width() <= bounds.width(),
        "the preview image must fit inside the thumbnail plate"
    );
    assert!(
        (wide.height() - 72.0).abs() < f32::EPSILON,
        "16:9 frames should letterbox by width only in the taskbar thumbnail"
    );
    assert!(
        (wide.width() - 128.0).abs() < f32::EPSILON,
        "16:9 frame width should preserve aspect ratio"
    );
    assert_eq!(wide.center(), bounds.center());
}

#[test]
fn win10_hybrid_31_session_preview_texture_matches_the_current_entry_only() {
    let ctx = egui::Context::default();
    let texture = ctx.load_texture(
        "vdi-preview-test",
        egui::ColorImage {
            size: [4, 3],
            pixels: vec![egui::Color32::WHITE; 12],
        },
        egui::TextureOptions::LINEAR,
    );
    let entry = SessionRailEntry::with_session_id("session-1", "Accounting VM", "RDP");
    let preview = SessionPreviewTexture::new(
        Some("session-1".to_string()),
        "Accounting VM",
        "RDP",
        texture,
    );
    assert!(
        session_preview_texture_for_entry(Some(&preview), &entry).is_some(),
        "the broker-matched VDI texture should attach to its rail entry"
    );

    let other = SessionRailEntry::with_session_id("session-2", "Other VM", "RDP");
    assert!(
        session_preview_texture_for_entry(Some(&preview), &other).is_none(),
        "a live VDI thumbnail must not bleed onto another rail session"
    );
}

#[test]
fn win10_hybrid_31_tray_overflow_flyout_routes_status_segments() {
    // The ▲ overflow is the Win10 hidden-icons affordance: it opens a compact
    // status-segment flyout, every segment row is addressable, and row activation
    // routes to that segment's owning full surface.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    s.set_status_inputs(
        MeshSummary::default(),
        None,
        0,
        false,
        Vec::new(),
        NodeGrades::default(),
        StatusSegments::default(),
    );
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let overflow = ctx
        .read_response(tray_overflow_id())
        .expect("the tray overflow control is registered")
        .rect;
    click_rail_cell(&ctx, &mut s, overflow.center(), sz);
    assert!(s.tray_overflow_open, "clicking ▲ opens the tray flyout");

    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let _popup = ctx
        .read_response(tray_overflow_popup_id())
        .expect("the tray overflow flyout is registered")
        .rect;
    for segment in status::StatusSegment::ALL {
        assert!(
            ctx.read_response(tray_overflow_row_id(segment)).is_some(),
            "the tray flyout exposes the {segment:?} segment row"
        );
    }

    let popup_h = status::StatusSegment::ALL.len() as f32 * TRAY_OVERFLOW_ROW_H + Style::SP_S;
    let popup_top = overflow.top() - Style::SP_XS - popup_h;
    let alerts_index = status::StatusSegment::ALL
        .iter()
        .position(|segment| *segment == status::StatusSegment::Alerts)
        .expect("Alerts segment is part of the tray overflow");
    let alerts = egui::pos2(
        overflow.left() + Style::SP_XS + (TRAY_OVERFLOW_W - Style::SP_S) / 2.0,
        popup_top
            + Style::SP_XS / 2.0
            + alerts_index as f32 * TRAY_OVERFLOW_ROW_H
            + TRAY_OVERFLOW_ROW_H / 2.0,
    );
    assert!(
        alerts.y < overflow.top(),
        "the computed Alerts row click target sits above the tray overflow control"
    );
    drive_vdock(&ctx, &mut s, vec![egui::Event::PointerMoved(alerts)], sz);
    drive_vdock(&ctx, &mut s, vec![egui::Event::PointerMoved(alerts)], sz);
    assert!(
        ctx.read_response(tray_overflow_row_id(status::StatusSegment::Alerts))
            .expect("Alerts row still registered")
            .hovered(),
        "the computed screen-space Alerts row target hovers the row"
    );
    click_rail_cell(&ctx, &mut s, alerts, sz);
    assert_eq!(
        s.active,
        Surface::Chat,
        "the Alerts overflow row routes to the Chat notification feed"
    );
    assert!(
        !s.tray_overflow_open,
        "routing from a tray overflow row closes the flyout"
    );
}

#[test]
fn win10_hybrid_31_autohide_reveal_contract_is_hot_edge_or_latched() {
    // The auto-hidden taskbar should not pop up merely because the pointer enters
    // the full 48px band; it reveals from the thin hot edge, then stays up while
    // latched and the pointer rides the already-shown bar.
    assert!(
        taskbar_reveal(false, false, false),
        "a docked taskbar is always visible"
    );
    assert!(
        !taskbar_reveal(true, false, false),
        "an auto-hidden taskbar stays hidden away from the hot edge"
    );
    assert!(
        taskbar_reveal(true, true, false),
        "the bottom hot edge summons an auto-hidden taskbar"
    );
    assert!(
        taskbar_reveal(true, false, true),
        "a revealed auto-hidden taskbar remains up while latched"
    );
}

// ── WIN7-DESKTOP-1 regression fix (post-WIN7-SHOT-1) ────────────────────
// Every rail test above this point — including WIN7-1's own two — asserts
// cells RELATIVE to each other (left-to-right order, same-row sharing via
// `center().y` deltas). None of them read the actual driven `screen_rect`,
// so all of them stayed green while `notification_rail_with_sources`
// painted the whole taskbar (and its `ui.interact` hit-rects — the SAME
// rects, so clicks moved with the paint) at literal screen y≈0 instead of
// the bottom. These two tests read the rail back against the screen's own
// true edges — the one check that structurally could not have missed it.

#[test]
fn win7_desktop_1_regression_the_taskbar_anchors_to_the_screens_true_bottom_edge() {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    s.set_status_inputs(
        MeshSummary {
            peers_total: 3,
            peers_online: 2,
            health: LighthouseHealth::Degraded,
            seen: true,
        },
        None,
        0,
        false,
        Vec::new(),
        NodeGrades::default(),
        StatusSegments::default(),
    );
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let source = ctx
        .read_response(desktop_source_toggle_id())
        .expect("Desktop Sources cell registered")
        .rect;
    let clock = ctx
        .read_response(clock_cell_id())
        .expect("clock registered")
        .rect;

    for (label, r) in [("Desktop Sources", source), ("clock", clock)] {
        assert!(
            (r.bottom() - sz.y).abs() < Style::SP_S,
            "{label} cell's bottom edge must sit within one small-spacing \
             token of the screen's TRUE bottom edge ({}), got {} — design \
             lock #1's \"true Win7 bottom taskbar\" anchors to the bottom \
             of the screen, it does not float near the top",
            sz.y,
            r.bottom()
        );
        assert!(
            r.top() > sz.y / 2.0,
            "{label} cell must sit in the bottom half of the screen, not \
             the top half — got top={}",
            r.top()
        );
    }
}

#[test]
fn win7_desktop_1_regression_the_status_panel_opens_above_the_rail_not_the_screen_top() {
    // The SAME `local`/`area_top` coordinate bug the taskbar regression
    // test above catches also fed `notification_panel_rect` (NOTIF-4's
    // slide-out detail panel, computed from the identical `local` rect) —
    // verify the fix covers it too: once open, the panel sits ABOVE the
    // (now correctly bottom-anchored) rail, never pinned up at the
    // screen's literal top edge.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    s.open_status_panel_for_test();
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let panel = ctx
        .read_response(status::status_panel_id())
        .expect("status panel registered while open")
        .rect;
    let clock = ctx
        .read_response(clock_cell_id())
        .expect("clock registered")
        .rect;
    assert!(
        panel.bottom() <= clock.top() + 1.0,
        "the status detail panel must sit ABOVE the bottom rail (panel \
         bottom {}, rail top {}), not float independently of it",
        panel.bottom(),
        clock.top()
    );
    assert!(
        panel.top() > sz.y / 4.0,
        "the panel must not be pinned near the literal screen top (got \
         top={}) — the pre-fix symptom when `area_top`/`local` collapsed \
         to (0,0)-based coordinates",
        panel.top()
    );
}

#[test]
fn health_panel_opens_from_the_right_side_health_cell() {
    // The Health detail panel is triggered from the right taskbar cluster; it should
    // open above that cluster, not from the rail's left edge.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    s.open_status_panel_for_test();
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let health = ctx
        .read_response(status_detail_toggle_id())
        .expect("health panel taskbar cell registered")
        .rect;
    let panel = ctx
        .read_response(status::status_panel_id())
        .expect("status panel registered while open")
        .rect;

    assert!(
        panel.right() <= health.right() + Style::SP_S,
        "health panel should right-align to the Health taskbar cell: panel={panel:?} health={health:?}"
    );
    assert!(
        panel.left() > sz.x / 2.0,
        "health panel should open on the right side of the screen, not at the left edge: {panel:?}"
    );
}

// ── VDOCK-4: the system 2×2 quad + Power menu (design #7/#17/#18) ──────────

// ── NODE-GRADE-2: the grade mini-list band (design #5/#7/#8/#18/#19) ───────

// ── WIN7-7: dock.rs's own accesskit pass (lock #14) ─────────────────────
// Before this unit `dock.rs` exported NOTHING to the accessibility tree —
// every taskbar cell is a hand-rolled `ui.interact` widget, and only
// `status.rs`'s tray pips (already covered by `install_segment_accessibility`,
// reused unchanged from this file) had real accesskit nodes. These tests
// follow the SAME pattern `status.rs`/`console.rs`/`start_menu.rs` already
// use: enable accesskit, drive a frame, read `platform_output.accesskit_update`.

fn accesskit_nodes(
    out: &egui::FullOutput,
) -> Vec<(egui::accesskit::NodeId, egui::accesskit::Node)> {
    out.platform_output
        .accesskit_update
        .as_ref()
        .expect("accesskit update")
        .nodes
        .clone()
}

fn painted_text(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
    fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
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

fn painted_text_clips(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Rect)> {
    fn walk(shape: &egui::Shape, clip: egui::Rect, out: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) => out.push((text.galley.text().to_owned(), clip)),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, clip, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, clipped.clip_rect, &mut out);
    }
    out
}

fn painted_rect_fills(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Color32> {
    fn walk(shape: &egui::Shape, out: &mut Vec<egui::Color32>) {
        match shape {
            egui::Shape::Mesh(mesh) => {
                out.extend(mesh.vertices.iter().map(|vertex| vertex.color));
            }
            egui::Shape::Path(path) if path.fill != egui::Color32::TRANSPARENT => {
                out.push(path.fill);
            }
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

fn painted_text_colors(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Color32> {
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

    fn walk(shape: &egui::Shape, out: &mut Vec<egui::Color32>) {
        match shape {
            egui::Shape::Text(text) => out.push(text_color(text)),
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

fn painted_mesh_vertex_colors(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Color32> {
    fn walk(shape: &egui::Shape, out: &mut Vec<egui::Color32>) {
        match shape {
            egui::Shape::Mesh(mesh) => out.extend(mesh.vertices.iter().map(|vertex| vertex.color)),
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

fn visible_black(color: egui::Color32) -> bool {
    color.r() == 0 && color.g() == 0 && color.b() == 0 && color.a() >= 0x80
}

fn visible_white(color: egui::Color32) -> bool {
    let full_strength_white =
        color.r() >= 0xF0 && color.g() >= 0xF0 && color.b() >= 0xF0 && color.a() >= 0x80;
    let opacity_applied_white = color.a() >= 0x80
        && color.r().abs_diff(color.a()) <= 1
        && color.g().abs_diff(color.a()) <= 1
        && color.b().abs_diff(color.a()) <= 1;
    full_strength_white || opacity_applied_white
}

fn accesskit_bounds_rect(node: &egui::accesskit::Node) -> egui::Rect {
    let bounds = node.bounds().expect("accesskit node has bounds");
    egui::Rect::from_min_max(
        egui::pos2(bounds.x0 as f32, bounds.y0 as f32),
        egui::pos2(bounds.x1 as f32, bounds.y1 as f32),
    )
}

#[test]
fn taskbar_launch_sources_health_and_overflow_use_distinct_non_chevron_icons() {
    let icons = [
        DESKTOP_SOURCE_TOGGLE_ICON,
        STATUS_DETAIL_ICON,
        TRAY_OVERFLOW_ICON,
        ACTION_CENTER_ICON,
    ];
    assert_eq!(DESKTOP_SOURCE_TOGGLE_ICON, IconId::Desktop);
    assert_eq!(STATUS_DETAIL_ICON, IconId::HealthStatus);
    assert_eq!(TRAY_OVERFLOW_ICON, IconId::MoreHorizontal);
    assert_eq!(ACTION_CENTER_ICON, IconId::Notifications);
    assert!(
        !icons.contains(&IconId::ChevronUp),
        "taskbar actions should not share the old up-arrow glyph"
    );
    assert!(
        !icons.contains(&IconId::Pin),
        "taskbar actions should not include the retired Start-bar pin glyph"
    );
    let mut names: Vec<&str> = icons.iter().map(|icon| icon.name()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(
        names.len(),
        icons.len(),
        "taskbar action icons must be distinct"
    );
}

#[test]
fn taskbar_controls_render_white_icons_on_a_black_bar() {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let out = drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let fills = painted_rect_fills(&out.shapes);
    assert!(
        fills.iter().copied().any(visible_black),
        "taskbar strip should paint the requested black bar: {fills:?}"
    );
    let mesh_colors = painted_mesh_vertex_colors(&out.shapes);
    assert!(
        mesh_colors.iter().copied().any(visible_white),
        "taskbar icon meshes should be tinted white: {mesh_colors:?}"
    );
    let text_colors = painted_text_colors(&out.shapes);
    assert!(
        text_colors.iter().copied().any(visible_white),
        "clock/taskbar text glyphs should be white on the black bar: {text_colors:?}"
    );
}

#[test]
fn win11_tray_clock_and_notification_area_paint_a_grouped_island() {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    s.set_status_inputs(
        MeshSummary::default(),
        None,
        2,
        true,
        Vec::new(),
        NodeGrades::default(),
        StatusSegments::default(),
    );
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let status_detail = ctx
        .read_response(status_detail_toggle_id())
        .expect("status-detail toggle registered")
        .rect;
    let clock = ctx
        .read_response(clock_cell_id())
        .expect("clock registered")
        .rect;
    let action = ctx
        .read_response(action_center_cell_id())
        .expect("action-center registered")
        .rect;

    let rail = egui::Rect::from_min_size(
        egui::pos2(0.0, sz.y - s.rail_height()),
        egui::vec2(sz.x, s.rail_height()),
    );
    let tray_icon_w = s
        .rail_height()
        .min(super::NOTIFICATION_RAIL_EXPANDED_ICON_H)
        - 4.0;
    let status_w = status::notification_rail_width(&s.status.segments, tray_icon_w);
    let island = win11_tray_island_rect(rail, s.rail_height(), s.rail_height() * 2.5, status_w);
    assert!(
        island.contains_rect(status_detail)
            && island.contains_rect(clock)
            && island.contains_rect(action),
        "Windows 11 tray island must contain health, clock, and notification cells: \
         island={island:?} health={status_detail:?} clock={clock:?} action={action:?}"
    );
    let canvas = capture_taskbar_screenshot("taskbar-win11-tray-island.png", &mut s, sz, &[]);
    let island_pixels = canvas.count_near_color_in_rect(island, TASKBAR_TRAY_ISLAND_FILL, 8)
        + canvas.count_near_color_in_rect(island, TASKBAR_TRAY_ISLAND_ACTIVE_FILL, 8);
    assert!(
        island_pixels > 200,
        "tray/clock/notification area should visibly paint the Windows 11 grouped island fill; \
         matched {island_pixels} island pixels"
    );
}

#[test]
fn win7_7_the_taskbar_itself_exports_a_toolbar_landmark() {
    // The task's own question: does the taskbar have a sensible landmark
    // role, not just its contents? `Role::Toolbar` is accesskit's
    // ARIA-toolbar-equivalent — "a container grouping a set of controls."
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    s.set_status_inputs(
        MeshSummary {
            peers_total: 3,
            peers_online: 2,
            health: LighthouseHealth::Degraded,
            seen: true,
        },
        None,
        0,
        false,
        Vec::new(),
        NodeGrades::default(),
        StatusSegments::default(),
    );
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz); // settle (this file's own 2-frame convention)
    let out = drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let nodes = accesskit_nodes(&out);

    let taskbar = nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some("Taskbar"))
        .expect("the taskbar exports its own landmark node");
    assert_eq!(taskbar.role(), egui::accesskit::Role::Toolbar);
}

#[test]
fn win7_7_every_primary_taskbar_cell_exports_a_labelled_button_when_sessions_are_empty() {
    // The sessions-empty state is DockState's default — this sweep proves
    // the whole four-part contract (Start · sessions(fallback) · tray ·
    // clock) plus the action center, show-desktop nub, and Desktop-source caret
    // all export real `Button` nodes, not just the tray pips `status.rs` already
    // covered.
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz); // settle
    let out = drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let nodes = accesskit_nodes(&out);

    for label in ["Sessions", "Health panel", "Clock", "Desktop sources"] {
        let node = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some(label))
            .unwrap_or_else(|| panic!("{label} exports no accesskit node"));
        assert_eq!(
            node.role(),
            egui::accesskit::Role::Button,
            "{label}'s accesskit role"
        );
    }
}

#[test]
fn file_operation_progress_opens_inside_the_bottom_rail_and_routes_to_files() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.set_file_operation_progress(Some(FileOperationProgress::new(
        2,
        Some(0.5),
        "2 transfers",
    )));
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz); // settle
    let out = drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let rect = ctx
        .read_response(status::segment_pip_id(
            status::StatusSegment::FileOperations,
        ))
        .expect("file-operation status segment renders in the bottom rail")
        .rect;
    assert!(
        rect.bottom() > sz.y / 2.0,
        "file-operation status must live in the bottom navigation bar"
    );
    let nodes = accesskit_nodes(&out);
    let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, sz);
    let taskbar = nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some("Taskbar"))
        .expect("the taskbar exports its own landmark node");
    let taskbar_rect = accesskit_bounds_rect(taskbar);
    assert!(
        viewport.contains_rect(taskbar_rect),
        "taskbar landmark must stay inside the viewport: {taskbar_rect:?}"
    );
    assert!(
        taskbar_rect.contains_rect(rect),
        "file-operation status must live inside the bottom taskbar landmark: \
         taskbar={taskbar_rect:?} progress={rect:?}"
    );
    let live = nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some("Notification status"))
        .expect("the progress segment is part of notification status");
    assert!(
        live.value()
            .is_some_and(|value| value.contains("File operations active: 2 active")),
        "the notification live region names active file operations"
    );
    let progress = nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some("File operations status"))
        .expect("the progress segment exports accesskit");
    assert_eq!(progress.role(), egui::accesskit::Role::Button);
    let progress_bounds = accesskit_bounds_rect(progress);
    assert!(
        taskbar_rect.contains_rect(progress_bounds),
        "file-operation accesskit node must stay inside the taskbar landmark: \
         taskbar={taskbar_rect:?} progress={progress_bounds:?}"
    );
    assert_eq!(
        progress.value(),
        Some("File operations active: 2 active file operations, 50% average progress")
    );

    click_rail_cell(&ctx, &mut s, rect.center(), sz);
    assert_eq!(
        s.active(),
        Surface::Files,
        "clicking global file progress opens Files"
    );
    assert!(
        s.take_file_operation_progress_request(),
        "clicking global file progress must request the Files Transfers tab"
    );
    assert!(
        !s.take_file_operation_progress_request(),
        "the file-progress request drains once"
    );
}

#[test]
fn file_operation_progress_renders_in_the_status_panel_and_routes_to_transfers() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    s.open_status_panel_for_test();
    s.set_file_operation_progress(Some(FileOperationProgress::new(
        3,
        Some(0.25),
        "3 file operations",
    )));
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let out = drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let row = ctx
        .read_response(status::status_panel_file_operations_id())
        .expect("active file operations render inside the expanded status panel")
        .rect;
    let panel = ctx
        .read_response(status::status_panel_id())
        .expect("status panel is mounted")
        .rect;
    assert!(
        panel.contains_rect(row),
        "file-operation status row must stay inside the notification status panel"
    );
    let nodes = accesskit_nodes(&out);
    let row_node = nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| {
            n.label() == Some("File operations status")
                && n.value() == Some("3 active file operations, 25% average progress")
        })
        .expect("status panel file-operation row exports accesskit");
    assert_eq!(
        row_node.value(),
        Some("3 active file operations, 25% average progress")
    );

    click_rail_cell(&ctx, &mut s, row.center(), sz);
    assert_eq!(
        s.active(),
        Surface::Files,
        "clicking the status-panel file-operation row opens Files"
    );
    assert!(
        s.take_file_operation_progress_request(),
        "panel activation requests the Files Transfers tab"
    );
    assert!(
        !s.status_panel_open,
        "routing from the status-panel file-operation row closes the panel"
    );
}

fn file_progress_visual_state(panel_open: bool) -> DockState {
    let mut s = DockState::default();
    if panel_open {
        s.open_status_panel_for_test();
    }
    s.set_active(Surface::Desktop);
    s.set_file_operation_progress(Some(FileOperationProgress::new(
        3,
        Some(0.64),
        "3 file operations",
    )));
    s.set_status_inputs(
        MeshSummary::default(),
        None,
        1,
        true,
        vec![
            SessionRailEntry::with_session_id("session-1", "Accounting VM", "RDP"),
            SessionRailEntry::with_session_id("session-2", "Build Desktop", "VNC"),
        ],
        NodeGrades::default(),
        StatusSegments::default(),
    );
    s
}

#[test]
fn file_operation_progress_visual_screenshots_cover_bottom_rail_and_panel() {
    let size = egui::vec2(1280.0, 760.0);
    let sources = vec![DesktopRailSource::new(
        "desktop-accounting",
        "Accounting VM",
        "oak",
        "RDP",
        true,
        true,
        true,
    )];

    let mut rail = file_progress_visual_state(false);
    let rail_canvas =
        capture_taskbar_screenshot("taskbar-file-progress-rail.png", &mut rail, size, &sources);
    assert!(
        rail_canvas.count_exact_color(Style::CAPTURE_CLEAR) < rail_canvas.width(),
        "bottom-rail screenshot should not expose the capture-clear background"
    );
    assert!(
        rail_canvas.count_near_color(Style::ACCENT, 48) > 40,
        "bottom-rail screenshot must paint a compact file-operation progress bar"
    );

    let mut panel = file_progress_visual_state(true);
    let panel_canvas = capture_taskbar_screenshot(
        "taskbar-file-progress-panel.png",
        &mut panel,
        size,
        &sources,
    );
    assert!(
        panel_canvas.count_near_color(Style::ACCENT, 48) > 80,
        "expanded file-operation progress screenshot must paint the shared accent progress bar"
    );
}

#[test]
fn win7_7_a_real_session_entry_exports_its_own_labelled_button() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    // `session_entry`'s `selected` param is `state.active == Surface::Desktop`
    // (unrelated to the `session_active` bool below, which only tints the
    // sessions-EMPTY fallback glyph) — set it explicitly so this test
    // actually exercises the "Active desktop session" value branch rather
    // than silently falling through to the not-selected one.
    s.set_active(Surface::Desktop);
    let entry = SessionRailEntry::with_session_id("session-1", "Accounting VM", "RDP");
    s.set_status_inputs(
        MeshSummary::default(),
        None,
        0,
        true,
        vec![entry],
        NodeGrades::default(),
        StatusSegments::default(),
    );
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz); // settle
    let out = drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let nodes = accesskit_nodes(&out);

    let session = nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some("Accounting VM RDP"))
        .expect("the session entry exports its own accesskit node");
    assert_eq!(session.role(), egui::accesskit::Role::Button);
    assert_eq!(session.value(), Some("Active desktop session"));
}

#[test]
fn win7_7_the_clocks_accesskit_value_carries_the_live_time_reading() {
    // The task's own question: does the clock announce the time in an
    // accessible way? Its `Button` node's VALUE is the same live `HH:MM`
    // reading its glyph paints — a screen reader can navigate to it and
    // hear the time on demand.
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz); // settle
    let out = drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let nodes = accesskit_nodes(&out);

    let clock = nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some("Clock"))
        .expect("the clock exports an accesskit node");
    assert_eq!(clock.role(), egui::accesskit::Role::Button);
    let expected = crate::timers::hhmm(crate::timers::now_unix());
    assert_eq!(
        clock.value(),
        Some(expected.as_str()),
        "the accessible value is the SAME live clock fold the glyph paints"
    );
}

#[test]
fn win7_7_the_notification_toggle_reports_state_and_startbar_pin_is_absent() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    s.set_status_inputs(
        MeshSummary {
            peers_total: 3,
            peers_online: 2,
            health: LighthouseHealth::Degraded,
            seen: true,
        },
        None,
        0,
        false,
        Vec::new(),
        NodeGrades::default(),
        StatusSegments::default(),
    );
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz); // settle
    let out = drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let nodes = accesskit_nodes(&out);
    assert!(
        nodes
            .iter()
            .map(|(_, n)| n)
            .all(|n| n.label() != Some("Pin")),
        "the removed Start bar pin must not export an accesskit node"
    );
    let toggle = nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some("Health panel"))
        .expect("the notification toggle exports an accesskit node");
    assert_eq!(
        toggle.value(),
        Some("Collapsed; 2/3 peers online; mesh degraded")
    );

    // Open the notification panel — the SAME node must now report the opposite
    // state, not a value frozen at first paint.
    s.status_panel_open = true;
    let out2 = drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let nodes2 = accesskit_nodes(&out2);
    assert!(
        nodes2
            .iter()
            .map(|(_, n)| n)
            .all(|n| n.label() != Some("Pin")),
        "the removed Start bar pin must stay absent after state changes"
    );
    let toggle2 = nodes2
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some("Health panel"))
        .expect("the notification toggle still exports an accesskit node");
    assert_eq!(
        toggle2.value(),
        Some("Expanded; 2/3 peers online; mesh degraded")
    );
}

#[test]
fn win7_7_desktop_source_rows_export_accesskit_including_an_unavailable_source() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    s.desktop_sources_open = true;
    let sources = vec![
        DesktopRailSource::new(
            "peer:oak",
            "oak",
            "lighthouse-oak",
            "RDP",
            true,
            true,
            false,
        ),
        DesktopRailSource::new(
            "peer:elm",
            "elm",
            "lighthouse-elm",
            "VNC",
            false,
            false,
            false,
        ),
    ];
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock_with_sources(&ctx, &mut s, Vec::new(), sz, &sources); // settle
    let out = drive_vdock_with_sources(&ctx, &mut s, Vec::new(), sz, &sources);
    let nodes = accesskit_nodes(&out);

    let available = nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some("oak"))
        .expect("the connectable source row exports an accesskit node");
    assert_eq!(available.role(), egui::accesskit::Role::Button);
    assert_eq!(available.value(), Some("lighthouse-oak RDP"));

    let unavailable = nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some("elm"))
        .expect("the unavailable source row still exports an accesskit node");
    assert_eq!(
        unavailable.value(),
        Some("lighthouse-elm VNC (unavailable)"),
        "an unreachable source is still named AND flagged, never silently omitted"
    );
}

#[test]
fn win7_7_the_session_overflow_more_cell_reports_the_real_hidden_count() {
    // The `navbar7_bottom_rail_more_popup_keeps_overflow_sessions_reachable`
    // fixture — a narrow rail with 4 sessions — reused here to prove the
    // More cell's accesskit value carries the REAL hidden count rather
    // than a generic "more" with no number. That precedent only pins
    // "the LAST entry is folded out," not the exact count (session
    // widths clamp up to 180px each, so exactly how many of the 4 fit
    // in a 380px-wide rail isn't hand-computable without duplicating
    // `session_entry_width`'s own arithmetic) — so this test derives the
    // expected count from what's ACTUALLY registered inline (the same
    // `ctx.read_response(session_entry_id(..))` the original precedent
    // uses to prove an entry is hidden), rather than guessing a literal.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    ctx.enable_accesskit();
    let mut s = DockState::default();
    s.toggle();
    let entries = vec![
        SessionRailEntry::with_session_id("s1", "Alpha Desktop", "RDP"),
        SessionRailEntry::with_session_id("s2", "Bravo Desktop", "RDP"),
        SessionRailEntry::with_session_id("s3", "Charlie Desktop", "VNC"),
        SessionRailEntry::with_session_id("s4", "Delta Desktop", "RDP"),
    ];
    s.set_status_inputs(
        MeshSummary::default(),
        None,
        0,
        true,
        entries.clone(),
        NodeGrades::default(),
        StatusSegments::default(),
    );
    let sz = egui::vec2(380.0, 720.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let out = drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let nodes = accesskit_nodes(&out);
    let texts = painted_text(&out.shapes);
    assert!(
        !texts.iter().any(|text| text.contains('⋯')),
        "the session overflow More cell should paint the YAMIS icon, not a text ellipsis: {texts:?}"
    );

    let visible = entries
        .iter()
        .enumerate()
        .filter(|(idx, entry)| ctx.read_response(session_entry_id(*idx, entry)).is_some())
        .count();
    let hidden = entries.len() - visible;
    assert!(
        hidden > 0,
        "the narrow fixture must actually force overflow"
    );

    let more = nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some("More sessions"))
        .expect("the overflow cell exports an accesskit node");
    assert_eq!(
        more.value(),
        Some(format!(
            "{hidden} more session{}",
            if hidden == 1 { "" } else { "s" }
        ))
        .as_deref(),
        "the accesskit value's count must match the REAL number of \
         sessions folded out of the inline rail"
    );
}

// ── DEDUPE restore: live-taskbar routing coverage ─────────────────────────
// The DEDUPE-1/2 sweep (5f4c18d0) deleted the retired vertical-dock/picker
// code and, bundled with it, a handful of tests for STILL-LIVE bottom-taskbar
// features that merely shared a now-deleted picker symbol/helper. These
// re-add focused coverage for those live features using ONLY the surviving
// live idiom — `drive_vdock`/`click_rail_cell` over the live
// `notification_rail_with_sources`, addressing the live cell ids — with no
// reference to any deleted picker symbol.

#[test]
fn the_clock_cell_shows_the_live_time_and_routes_to_timers() {
    // Lock #20 — the clock-glyph cell paints the LIVE wall-clock HH:MM as its
    // glyph (the time IS the icon), rides the taskbar's right tray cluster (no
    // longer in the left rail), and a click opens the Timers & Alarms surface
    // (its ONE home). (Was `the_clock_strip_shows_the_live_time_and_routes_to_timers`.)
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    let sz = egui::vec2(1280.0, 900.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let cell = ctx
        .read_response(clock_cell_id())
        .expect("the clock cell is registered")
        .rect;
    assert!(
        cell.width() > 0.0 && cell.width() < sz.x / 6.0,
        "the clock is a bounded tray item ({}px), not the whole bar — at the \
         48px Win10 height it is wider than a square cell to fit HH:MM (and date)",
        cell.width()
    );
    assert!(
        cell.left() > DOCK_W,
        "the clock rides the right tray cluster, not the left rail"
    );

    // A click routes to Timers & Alarms (the surface's ONE home).
    assert_ne!(s.active, Surface::Timers, "start off the Timers surface");
    click_rail_cell(&ctx, &mut s, cell.center(), sz);
    assert_eq!(
        s.active,
        Surface::Timers,
        "clicking the clock opens Timers & Alarms (lock #20)"
    );
}

#[test]
fn the_status_segment_pips_route_to_their_surfaces() {
    // NOTIF-3 wired end-to-end: the bottom taskbar's status pips route
    // `DockState::active` (lock #15). Each segment carries its own stable pip id
    // and its own route — Device/Power → System, Mesh → MeshView,
    // FileOperations → Files, Alerts → Chat (`status::StatusSegment::route`). Mount the live rail, read
    // each pip by its id, and prove the click lands on the right surface,
    // resetting to the Workbench between pips so every route is proven
    // independently rather than by luck of the prior click.
    // (Was `a_status_segment_pip_routes_through_the_dock_bottom_zone`, which
    // only exercised the single Alerts → Chat leg.)
    use status::StatusSegment;
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle(); // reveal the taskbar so its cells (and the status strip) mount
    s.set_status_inputs(
        MeshSummary::default(),
        None,
        3,
        false,
        Vec::new(),
        NodeGrades::default(),
        StatusSegments::default(),
    );
    let sz = egui::vec2(1280.0, 800.0);
    // Prime so the segment pip rects register + settle under their stable ids.
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    for (segment, expected) in [
        (StatusSegment::Device, Surface::System),
        (StatusSegment::Mesh, Surface::MeshView),
        (StatusSegment::Power, Surface::System),
        (StatusSegment::RemoteControl, Surface::System),
        (StatusSegment::FileOperations, Surface::Files),
        (StatusSegment::Alerts, Surface::Chat),
    ] {
        let center = ctx
            .read_response(status::segment_pip_id(segment))
            .unwrap_or_else(|| panic!("the {segment:?} status pip is registered"))
            .rect
            .center();
        s.set_active(Surface::Workbench);
        click_rail_cell(&ctx, &mut s, center, sz);
        assert_eq!(
            s.active, expected,
            "clicking the {segment:?} pip routes to {expected:?} (lock #15)"
        );
        if segment == StatusSegment::FileOperations {
            assert!(
                s.take_file_operation_progress_request(),
                "FileOperations pip also requests the Files Transfers tab"
            );
        }
    }
}

#[test]
fn the_health_control_opens_and_dismisses_the_detail_panel() {
    // NOTIF-4 — the detail panel mounts from the bottom rail's Health control
    // (`status_detail_toggle`); Escape and click-away both dismiss it.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    s.set_status_inputs(
        MeshSummary::default(),
        None,
        0,
        false,
        Vec::new(),
        grades(vec![grade("me", 95, true, false)]),
        StatusSegments::default(),
    );
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    assert!(!s.status_panel_open, "panel starts closed");
    let caret = ctx
        .read_response(status_detail_toggle_id())
        .expect("bottom-rail Health control renders")
        .rect
        .center();
    click_rail_cell(&ctx, &mut s, caret, sz);
    assert!(
        s.status_panel_open,
        "the Health control opens the detail panel"
    );
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    assert!(
        ctx.read_response(status::status_panel_id()).is_some(),
        "the status detail panel renders after opening"
    );

    drive_vdock(&ctx, &mut s, vec![key(egui::Key::Escape)], sz);
    assert!(!s.status_panel_open, "Escape dismisses the panel");

    s.open_status_panel_for_test();
    assert!(s.status_panel_open, "the test seam reopens the panel");
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    click_rail_cell(&ctx, &mut s, egui::pos2(500.0, 500.0), sz);
    assert!(!s.status_panel_open, "click-away dismisses the panel");
}

#[test]
fn a_requested_desktop_session_renders_as_a_named_bottom_rail_entry() {
    // NAVBAR-U3 / operator rail request — once the Desktop surface has a real
    // requested target, the taskbar shows its own addressable session entry
    // rather than only the generic Sessions fallback glyph. WIN10-HYBRID #31
    // made the tile an icons-only rail-height square whose full name now rides
    // the accesskit node (covered by `win7_7_a_real_session_entry_...`), so this
    // pins the behaviour that distinguishes a real entry from the fallback:
    // it is its OWN addressable rail cell, and clicking it focuses Desktop AND
    // latches the broker session id the shell reconnects to.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    // Desktop is now the default active surface (Construct desktop-first pivot),
    // so start off it — matching
    // `win10_hybrid_31_the_show_desktop_nub_routes_to_desktop` — for the
    // session-entry click below to be an observable route TO Desktop.
    s.set_active(Surface::Browser);
    s.toggle();
    let entry = SessionRailEntry::with_session_id("session-1", "Accounting VM", "RDP");
    s.set_status_inputs(
        MeshSummary::default(),
        None,
        0,
        true,
        vec![entry.clone()],
        NodeGrades::default(),
        StatusSegments::default(),
    );
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let rect = ctx
        .read_response(session_entry_id(0, &entry))
        .expect("the named session entry is registered as its own rail cell")
        .rect;
    assert!(
        rect.width() > 0.0 && rect.height() > 0.0,
        "the session entry renders as a real bottom-rail cell"
    );
    assert!(
        rect.bottom() > sz.y / 2.0,
        "the session entry rides the bottom taskbar (bottom {}), not the top half",
        rect.bottom()
    );

    assert_ne!(s.active, Surface::Desktop, "starts off Desktop");
    click_rail_cell(&ctx, &mut s, rect.center(), sz);
    assert_eq!(
        s.active,
        Surface::Desktop,
        "clicking the session entry focuses the Desktop surface"
    );
    assert_eq!(
        s.take_desktop_session_focus().as_deref(),
        Some("session-1"),
        "the session entry latches its broker session id for the shell to reconnect"
    );
}
