use super::{
    clock_cell_id, desktop_source_row_id, desktop_source_toggle_id, dock, focus_ring_rect,
    grade_band_height, grade_overflow_id, grade_row_id, group_height, gutter_width,
    notification_rail, notification_rail_with_sources, overflow_more_id, pick_cell_id,
    power_item_id, rail_more_id, session_entry_id, start_cell_id, status_detail_toggle_id,
    surface_badge_id, surface_context_item_id, sys_cell_id, sys_cell_tint, transfer_badge_id,
    visible_group_count, DesktopRailSource, DockRequest, DockState, PowerItem, PowerMenu,
    SessionRailEntry, Surface, SurfaceContextItem, SysCell, CELL_W, DOCK_AREA, DOCK_W,
    FOCUS_RING_W, GRADE_MAX_ROWS, GROUPS, ICON_LOGICAL, NOTIFICATION_RAIL_EXPANDED_H,
    NOTIFICATION_RAIL_H, POWER_MENU, SYSTEM_QUAD, SYS_QUAD_ICON,
};
use crate::chrome::{GradeRow, GradeTrend, MeshSummary, NodeGrades};
use crate::status::{self, StatusSegments};
use mde_egui::Style;
use mde_egui::{egui, Density};
use mde_seat::PowerVerb;
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
    // exists in the Quasar palette) — one rung brighter than the resting accent.
    assert_ne!(Style::ACCENT_HI, Style::ACCENT);
}

#[test]
fn the_dock_lists_the_workbench_vm_surfaces_app_surfaces_and_info_surfaces() {
    // Seventeen entries: Workbench first, the live Mesh Map (OW-10, `mde-mesh-view`),
    // the brokered Desktop surface, the app surfaces (Music / Media — the
    // full media player, MEDIA-18 / Files / Voice / Browser — the sandboxed Servo
    // browser, BOOKMARKS-6 / Terminal — the Terminator-class terminal over a real
    // PTY, TERM-16 / Editor — the native Zed-style code editor, EDITOR-1), the
    // unified Chat surface (the ONE notification interface — the standalone
    // Notifications + Clipboard surfaces are retired, NOTIFY-CHAT-6), the Phones
    // hub (KDC-MESH-9 — the desktop-side paired-phone manager), the host-controls
    // System surface, the Storage surface (GParted-authentic disk mgmt, E12-21),
    // and the About surface (the platform-identity screen, QBRAND-6).
    assert_eq!(Surface::ALL.len(), 18);
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
fn the_shell_opens_on_the_workbench_surface() {
    assert_eq!(Surface::default(), Surface::Workbench);
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
    // The map is injective — 18 surfaces, 18 distinct glyph names (IaC wears
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
        (Surface::Desktop, "Desktop"),
        (Surface::Music, "Music"),
        (Surface::Media, "Media"),
        (Surface::Files, "Files"),
        (Surface::Voice, "Voice"),
        (Surface::Browser, "Browser"),
        (Surface::Bookmarks, "Bookmarks"),
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
    // Injective over Surface::ALL — 18 surfaces, 18 distinct labels.
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

/// Collect every text shape's `(angle, fallback_color)` in a frame's output,
/// recursing into shape groups. The group labels are rotated (angle ≠ 0),
/// tinted by their group accent; the clock lines are upright (angle 0).
fn collect_text_shapes(shape: &egui::Shape, out: &mut Vec<(f32, egui::Color32)>) {
    match shape {
        egui::Shape::Text(t) => out.push((t.angle, t.fallback_color)),
        egui::Shape::Vec(v) => {
            for s in v {
                collect_text_shapes(s, out);
            }
        }
        _ => {}
    }
}

// --- PICKER-1: the group table + rotated labels + hairline dividers -----------

#[test]
fn the_locked_group_taxonomy_and_order() {
    // L5/L7 — six groups in the locked left-to-right order, each listing its
    // surfaces in Surface::ALL relative order; About lives in the System group.
    // THREE surfaces are in no group: the Workbench (standalone lead), the
    // System surface (right-side Settings button), and Desktop (far-right
    // Show-Desktop sliver).
    use Surface::{
        About, Bookmarks, Browser, Chat, Editor, Explorer, Files, InfraCode, Media, MeshView,
        Music, Phones, Storage, Terminal, Voice, Workbench,
    };
    let expect: [(&str, &[Surface]); 6] = [
        ("Comms", &[Voice, Chat, Phones]),
        ("Workloads", &[InfraCode]),
        ("Terminals", &[Browser, Bookmarks, Terminal, Editor]),
        ("Mesh", &[MeshView, Explorer]),
        ("System", &[Files, Storage, About]),
        ("Media", &[Music, Media]),
    ];
    assert_eq!(GROUPS.len(), expect.len(), "six groups");
    for (g, (label, surfaces)) in GROUPS.iter().zip(expect) {
        assert_eq!(g.label, label, "group order");
        assert_eq!(
            g.surfaces, surfaces,
            "{label} membership + within-group order"
        );
    }
    let system = GROUPS.iter().find(|g| g.label == "System").unwrap();
    assert!(
        system.surfaces.contains(&About),
        "About lives in the System group"
    );
    // The three ungrouped surfaces are placed by the lead / the Settings button
    // / the far-right sliver, never a group.
    for ungrouped in [Workbench, Surface::System, Surface::Desktop] {
        assert!(
            GROUPS.iter().all(|g| !g.surfaces.contains(&ungrouped)),
            "{ungrouped:?} is placed outside every group"
        );
    }
}

#[test]
fn each_group_takes_its_shared_style_accent_token() {
    // PICKER-2: the group labels are keyed by the shared categorical tokens on
    // `mde_egui::Style` (the SAME six EXPLORER-15 consumes for category identity,
    // design O8) — defined once, consumed here. No local placeholder hex survives.
    let expect: [(&str, egui::Color32); 6] = [
        ("Comms", Style::ACCENT_COMMS),
        ("Workloads", Style::ACCENT_WORKLOADS),
        ("Terminals", Style::ACCENT_TERMINALS),
        ("Mesh", Style::ACCENT_MESH),
        ("System", Style::ACCENT_SYSTEM),
        ("Media", Style::ACCENT_MEDIA),
    ];
    for (g, (label, token)) in GROUPS.iter().zip(expect) {
        assert_eq!(g.label, label, "group order");
        assert_eq!(
            g.accent, token,
            "{label} label takes its shared Style token"
        );
    }
}

#[test]
fn the_groups_cover_every_surface_once_in_surface_all_order() {
    // The Workbench lead + the System Settings button + the far-right Desktop
    // sliver + the six groups reproduce all 18 of Surface::ALL, each surface
    // placed exactly once...
    let mut placed: Vec<Surface> = vec![Surface::Workbench, Surface::System, Surface::Desktop];
    for g in &GROUPS {
        placed.extend_from_slice(g.surfaces);
    }
    assert_eq!(
        placed.len(),
        Surface::ALL.len(),
        "every surface placed once"
    );
    for s in Surface::ALL {
        assert_eq!(
            placed.iter().filter(|&&x| x == s).count(),
            1,
            "{s:?} appears once across the lead + Settings + the Desktop sliver + groups"
        );
    }
    // ...and L7: within each group the surfaces keep Surface::ALL relative
    // order (their ALL indices ascend).
    let idx = |s: Surface| Surface::ALL.iter().position(|&x| x == s).unwrap();
    for g in &GROUPS {
        let idxs: Vec<usize> = g.surfaces.iter().map(|&s| idx(s)).collect();
        assert!(
            idxs.is_sorted(),
            "group {} keeps Surface::ALL order",
            g.label
        );
    }
}

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
        let _ = dock(ctx, state);
        let _ = notification_rail_with_sources(ctx, state, sources);
    })
}

/// Drive `frames` quiet headless frames of the vertical dock on a 1280×800
/// screen (the VDOCK-1 passthrough/frame tests' size).
fn run_vdock(ctx: &egui::Context, state: &mut DockState, frames: usize) {
    for _ in 0..frames {
        drive_vdock(ctx, state, Vec::new(), egui::vec2(1280.0, 800.0));
    }
}

/// The dock's floating-Area `LayerId` — `LayerId::new(Foreground, DOCK_AREA)`,
/// the same mapping `egui::Area::layer()` computes.
fn vdock_layer() -> egui::LayerId {
    egui::LayerId::new(egui::Order::Foreground, egui::Id::new(DOCK_AREA))
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

#[test]
fn the_dock_state_super_toggle_and_pin_hold_it_open() {
    // Locks #9/#13 — the pure auto-hide state machine (no GPU): the dock is
    // hidden by default, a Super tap toggles the reveal, and the pin holds it
    // open regardless of the reveal latch.
    let mut s = DockState::default();
    assert!(!s.shown(), "hidden by default (lock #9)");

    s.toggle();
    assert!(s.shown(), "a Super tap reveals it (lock #13)");
    s.toggle();
    assert!(!s.shown(), "a second tap hides it");

    // Pin holds it open even when the reveal latch is off.
    s.toggle_pin();
    assert!(
        s.pinned() && s.shown(),
        "pinning shows + holds it (lock #9)"
    );
    s.toggle();
    assert!(
        s.shown(),
        "a Super tap can't hide a PINNED dock — the pin holds it open"
    );
    // Unpinning (with the reveal latch now off) lets it hide again.
    s.toggle_pin();
    assert!(!s.shown(), "unpinning releases the hold");
}

#[test]
fn a_hidden_dock_mounts_no_layer_so_input_passes_through() {
    // The design's "auto-hide + DRM seat" risk: while hidden the dock must not
    // float a layer over the surface, or it would steal clicks/keys meant for
    // the surface beneath. A hidden dock creates NO Area, so `layer_id_at` over
    // its would-be column finds no dock layer — the click reaches the surface.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut hidden = DockState::default(); // hidden by default
    run_vdock(&ctx, &mut hidden, 2);

    let point = egui::pos2(DOCK_W / 2.0, 400.0); // inside the would-be column
    assert_ne!(
        ctx.layer_id_at(point),
        Some(vdock_layer()),
        "a HIDDEN dock must not float an intercepting layer (input passthrough)"
    );
}

#[test]
fn a_shown_dock_covers_its_column_and_paints_the_carbon_panel() {
    // The mirror of the passthrough test: a shown dock DOES claim its column
    // (so clicks over it land on the dock, not the surface), and its frame draws
    // real primitives (the Carbon-dark fill + the right-edge divider).
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut shown = DockState::default();
    shown.toggle(); // reveal it
    assert!(shown.shown());

    // Prime one frame, then capture the second frame's output.
    run_vdock(&ctx, &mut shown, 1);
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(1280.0, 800.0),
        )),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = ui.button("surface");
        });
        let _ = dock(ctx, &mut shown);
    });

    let point = egui::pos2(DOCK_W / 2.0, 400.0);
    assert_eq!(
        ctx.layer_id_at(point),
        Some(vdock_layer()),
        "a SHOWN dock claims its column so clicks land on the dock chrome"
    );
    let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    assert!(!prims.is_empty(), "the shown dock frame painted nothing");
}

#[test]
fn clicking_the_pin_toggle_pins_the_dock_open() {
    // The pin affordance (lock #9) is reachable: a click in the top cell flips
    // the pin, holding the dock open. Mirrors the taskbar cell-click test —
    // prime the layout, then press one frame + release the next.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle(); // reveal it so the Area (and its pin) is mounted

    let frame = |ctx: &egui::Context, s: &mut DockState, events: Vec<egui::Event>| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1280.0, 800.0),
            )),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = ui.button("surface");
            });
            let _ = dock(ctx, s);
            let _ = notification_rail(ctx, s);
        });
    };
    // Prime two frames so egui has the rail pin's rect registered, then move
    // onto the pin + press, then release the next frame.
    frame(&ctx, &mut s, Vec::new());
    frame(&ctx, &mut s, Vec::new());
    let click = ctx
        .read_response(egui::Id::new("vdock-pin"))
        .expect("the rail pin is registered")
        .rect
        .center();
    let press = egui::Event::PointerButton {
        pos: click,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::default(),
    };
    let release = egui::Event::PointerButton {
        pos: click,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::default(),
    };
    frame(&ctx, &mut s, vec![egui::Event::PointerMoved(click), press]);
    frame(&ctx, &mut s, vec![release]);
    assert!(s.pinned(), "clicking the pin holds the dock open (lock #9)");
}

// ── WIN7-2: the Start Menu's dock cell (CONSOLE-1's original front door) ──

#[test]
fn the_start_cell_anchors_the_bottom_rail_and_latches_the_start_menu_toggle() {
    // Console locks #1/#2 moved to the bottom rail: the Start cell is the
    // far-left rail icon, before Desktop, and a click latches the Start
    // Menu toggle the shell drains — exactly once (WIN7-1 relabelled the
    // rail text "Advanced" → "Start" without changing this click's target;
    // WIN7-2 is the unit that turns the target into the real two-pane
    // Start Menu, `crate::start_menu`, superseding the direct-to-Console
    // toggle this test used to name).
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle(); // reveal the dock so its cells mount
    let sz = egui::vec2(1280.0, 900.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let start = ctx
        .read_response(start_cell_id())
        .expect("the Start cell is registered")
        .rect;
    assert!(start.left() < 8.0, "the Start cell anchors the rail's left");
    assert!(
        start.height() < DOCK_W && start.width() < DOCK_W,
        "the Start cell is a small rail icon"
    );
    let desktop = ctx
        .read_response(pick_cell_id(Surface::Desktop))
        .expect("the Desktop rail cell is registered")
        .rect;
    assert!(
        desktop.left() >= start.right(),
        "Desktop sits immediately to the right of Start"
    );
    let wb = ctx
        .read_response(pick_cell_id(Surface::Workbench))
        .expect("the Workbench lead is registered")
        .rect;
    assert!(
        wb.top() < start.top(),
        "Workbench remains in the left rail while Start lives in the bottom taskbar"
    );

    assert!(!s.take_start_menu_toggle(), "no toggle before a click");
    click_vdock(&ctx, &mut s, start.center(), sz);
    assert!(
        s.take_start_menu_toggle(),
        "a Start-cell click latches the Start Menu toggle"
    );
    assert!(
        !s.take_start_menu_toggle(),
        "the toggle latch drains exactly once"
    );
    assert_eq!(
        s.active,
        Surface::default(),
        "the Start cell routes NO surface — it only toggles the Start Menu"
    );
}

// ── DOCK-OVERLAP: the shell reserves a gutter so the dock never overlaps ──

#[test]
fn a_shown_dock_reserves_a_full_gutter_a_hidden_one_reserves_nothing() {
    // DOCK-OVERLAP — the shell insets the central content by this width so the
    // dock never sits over the surface (except a full-screen remote desktop,
    // gated in main.rs). A fresh context reports the settled slide endpoint on
    // first sight (egui's `animate_bool`), so a shown dock reserves the full
    // DOCK_W and a hidden + settled dock reserves nothing (content fills width).
    let ctx = egui::Context::default();
    let mut shown = DockState::default();
    shown.toggle();
    assert!(
        (gutter_width(&ctx, &shown) - DOCK_W).abs() < f32::EPSILON,
        "a shown dock reserves a full DOCK_W gutter (no overlap)"
    );
    // A separate context so the slide latch starts fresh at the hidden endpoint.
    let ctx2 = egui::Context::default();
    let hidden = DockState::default();
    assert_eq!(
        gutter_width(&ctx2, &hidden),
        0.0,
        "a hidden + settled dock reserves nothing — the content fills full width"
    );
}

// ── VDOCK-2: the vertical app picker (top + middle zones) ─────────────────

/// The picker's surfaces in order — the Workbench lead, then each group's
/// members (`Surface::ALL` order). Excludes System (Settings) + Desktop, which
/// are VDOCK-4's system quad.
fn picker_surfaces() -> Vec<Surface> {
    std::iter::once(Surface::Workbench)
        .chain(GROUPS.iter().flat_map(|g| g.surfaces.iter().copied()))
        .collect()
}

fn press_at(pos: egui::Pos2) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::default(),
    }
}

fn release_at(pos: egui::Pos2) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::default(),
    }
}

fn key(k: egui::Key) -> egui::Event {
    egui::Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::default(),
    }
}

/// Click `center` — press one frame, release the next (the egui click model
/// the taskbar tests use). The caller primes the layout first.
fn click_vdock(ctx: &egui::Context, state: &mut DockState, center: egui::Pos2, size: egui::Vec2) {
    click_vdock_with_sources(ctx, state, center, size, &[]);
}

fn click_vdock_with_sources(
    ctx: &egui::Context,
    state: &mut DockState,
    center: egui::Pos2,
    size: egui::Vec2,
    sources: &[DesktopRailSource],
) {
    drive_vdock_with_sources(
        ctx,
        state,
        vec![egui::Event::PointerMoved(center), press_at(center)],
        size,
        sources,
    );
    drive_vdock_with_sources(ctx, state, vec![release_at(center)], size, sources);
}

fn secondary_click_vdock(
    ctx: &egui::Context,
    state: &mut DockState,
    center: egui::Pos2,
    size: egui::Vec2,
) {
    let press = egui::Event::PointerButton {
        pos: center,
        button: egui::PointerButton::Secondary,
        pressed: true,
        modifiers: egui::Modifiers::default(),
    };
    let release = egui::Event::PointerButton {
        pos: center,
        button: egui::PointerButton::Secondary,
        pressed: false,
        modifiers: egui::Modifiers::default(),
    };
    drive_vdock(
        ctx,
        state,
        vec![egui::Event::PointerMoved(center), press],
        size,
    );
    drive_vdock(ctx, state, vec![release], size);
}

#[test]
fn the_app_zone_fits_all_groups_when_tall_and_overflows_when_short() {
    // #22 — all six groups render inline when the app zone is tall enough; a
    // short zone reserves the '…' cell and shows fewer WHOLE groups.
    let total: f32 = GROUPS.iter().map(group_height).sum();
    assert_eq!(
        visible_group_count(total),
        GROUPS.len(),
        "all six fit when the zone == their total height"
    );
    assert_eq!(
        visible_group_count(total + 100.0),
        GROUPS.len(),
        "all six fit with room to spare"
    );
    // Drop just under the total (by the last group's height) → at least one
    // group folds into the overflow popup.
    let short = total - group_height(&GROUPS[GROUPS.len() - 1]);
    let n = visible_group_count(short);
    assert!(
        n < GROUPS.len(),
        "a short zone overflows — showed {n} of {}",
        GROUPS.len()
    );
    // A zone too small for even one group shows none (everything overflows).
    assert_eq!(visible_group_count(0.0), 0, "no room → all overflow");
}

#[test]
fn the_picker_routes_every_app_surface_and_defers_the_system_quad() {
    // §7 — the Workbench lead + the thirteen group surfaces each route on a click
    // into DockState::active (the carried-over routing). Settings (System) +
    // Show-Desktop are NOT in the picker — they belong to VDOCK-4's system quad.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle(); // reveal the dock so its Area (and cells) mount
                // Tall enough that all six groups render inline above the bottom zone
                // (which VDOCK-5's clock strip grew by CLOCK_CELL_H).
    let sz = egui::vec2(1280.0, 900.0);
    // Prime so every stable-id cell rect is registered + settled.
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let picker = picker_surfaces();
    assert_eq!(
        picker.len(),
        Surface::ALL.len() - 2,
        "the picker holds every surface but System + Desktop"
    );

    // Read every picker cell's settled centre up front (a click shifts no rect).
    let mut centers: Vec<(Surface, egui::Pos2)> = Vec::new();
    for &want in &picker {
        let resp = ctx.read_response(pick_cell_id(want));
        assert!(resp.is_some(), "{want:?} picker cell rect not registered");
        centers.push((want, resp.expect("registered above").rect.center()));
    }

    for (want, center) in centers {
        click_vdock(&ctx, &mut s, center, sz);
        assert_eq!(s.active, want, "clicking {want:?}'s picker cell selects it");
    }

    // System stays out of the picker; Desktop is now the second bottom-rail
    // control in the Windows-style rail.
    assert!(
        ctx.read_response(pick_cell_id(Surface::System)).is_none(),
        "System (Settings) is deferred to VDOCK-4's system quad"
    );
    assert!(
        ctx.read_response(pick_cell_id(Surface::Desktop)).is_some(),
        "Desktop is a bottom-rail control"
    );
}

#[test]
fn the_picker_stacks_the_groups_in_a_single_column() {
    // #2 — the app picker is ONE vertical column: every cell shares the
    // column's x-centre + full width, and the cells march strictly downward.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    // Tall enough for all six groups inline over the clock-grown bottom zone.
    let sz = egui::vec2(1280.0, 900.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let mut prev_bottom = f32::MIN;
    for surface in picker_surfaces() {
        let resp = ctx.read_response(pick_cell_id(surface));
        assert!(resp.is_some(), "{surface:?} cell rect not registered");
        let rect = resp.expect("registered above").rect;
        assert!(
            (rect.center().x - DOCK_W / 2.0).abs() < 1.0,
            "{surface:?} cell off the column centre (cx {})",
            rect.center().x
        );
        assert!(
            (rect.width() - DOCK_W).abs() < 1.0,
            "{surface:?} cell is not the full column width"
        );
        assert!(
            rect.top() >= prev_bottom - 1.0,
            "{surface:?} cell is not stacked below the previous one"
        );
        prev_bottom = rect.bottom();
    }
}

#[test]
fn the_group_labels_paint_horizontally_in_their_group_accent() {
    // #4 — each group carries ONE horizontal (angle 0) accent label above its
    // cells, painted in that group's Style accent token; on a tall screen all
    // six render inline with no '…' overflow. The only other text in the
    // column is VDOCK-5's clock glyph (the live HH:MM, dim — lock #20).
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    let sz = egui::vec2(1280.0, 900.0);
    // Prime a frame, then capture over an EMPTY surface so the only text is the
    // dock's group labels (no stand-in button caption to filter out).
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), sz)),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |_ui| {});
        let _ = dock(ctx, &mut s);
    });
    let mut texts = Vec::new();
    for clipped in &out.shapes {
        collect_text_shapes(&clipped.shape, &mut texts);
    }
    let accents: Vec<egui::Color32> = GROUPS.iter().map(|g| g.accent).collect();
    let (labels, rest): (Vec<_>, Vec<_>) = texts
        .into_iter()
        .partition(|(_, color)| accents.contains(color));
    assert_eq!(
        labels.len(),
        GROUPS.len(),
        "exactly one accent label per group (no captions, no '…' at this height)"
    );
    for (angle, _) in labels {
        assert!(
            angle.abs() < 1e-3,
            "the vertical dock's labels read HORIZONTALLY (angle 0), got {angle}"
        );
    }
    // No picker cell captions are painted; rail icons are glyphs, and the
    // clock lives outside the left-dock label cluster.
    assert_eq!(
        rest.len(),
        0,
        "besides group labels no left-dock captions are painted"
    );
    assert!(
        rest.iter().all(|(angle, _)| angle.abs() < 1e-3),
        "fixed chrome glyphs read upright"
    );
}

#[test]
fn the_active_surface_wears_a_left_edge_accent_bar() {
    // #10 — the active cell wears a left-edge Style::ACCENT bar. Capture the
    // frame's rect_filled shapes and confirm an ACCENT-coloured rect hugs the
    // column's left edge (x≈0) at the active cell — absent for the inactive.
    fn left_edge_accent_bars(shape: &egui::Shape, out: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Rect(r) if r.fill == Style::ACCENT && r.rect.left() < 1.0 => {
                out.push(r.rect);
            }
            egui::Shape::Vec(v) => {
                for s in v {
                    left_edge_accent_bars(s, out);
                }
            }
            _ => {}
        }
    }

    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default(); // active = Workbench (the top lead cell)
    s.toggle();
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), sz)),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |_ui| {});
        let _ = dock(ctx, &mut s);
    });

    let mut bars = Vec::new();
    for clipped in &out.shapes {
        left_edge_accent_bars(&clipped.shape, &mut bars);
    }
    // Exactly the active (Workbench) lead cell shows a left-edge accent bar.
    assert_eq!(
        bars.len(),
        1,
        "one active left-edge accent bar (the Workbench lead), got {}",
        bars.len()
    );
    let wb = ctx
        .read_response(pick_cell_id(Surface::Workbench))
        .expect("the Workbench lead cell is registered")
        .rect;
    let bar = bars[0];
    assert!(
        bar.left() < 1.0,
        "the accent bar hugs the column's left edge"
    );
    assert!(
        (bar.height() - wb.height()).abs() < 1.0,
        "the bar spans the active cell's height"
    );
}

#[test]
fn the_files_cell_badges_active_transfers_only_when_nonzero() {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let sz = egui::vec2(1280.0, 900.0);

    let mut idle = DockState::default();
    idle.toggle();
    drive_vdock(&ctx, &mut idle, Vec::new(), sz);
    assert!(
        ctx.read_response(transfer_badge_id(Surface::Files))
            .is_none(),
        "zero active transfers paints no Files badge"
    );

    let mut active = DockState::default();
    active.toggle();
    active.set_transfer_active_count(15);
    drive_vdock(&ctx, &mut active, Vec::new(), sz);
    let files = ctx
        .read_response(pick_cell_id(Surface::Files))
        .expect("Files cell is registered")
        .rect;
    let badge = ctx
        .read_response(transfer_badge_id(Surface::Files))
        .expect("active transfers register a Files badge")
        .rect;
    assert!(
        files.contains(badge.center()),
        "the transfer badge is anchored inside the Files dock cell"
    );
    assert_eq!(super::badge_label(15), "15");
    assert_eq!(super::badge_label(120), "99+");
}

#[test]
fn navbar5_live_badges_project_unread_peers_and_system_health() {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let sz = egui::vec2(1280.0, 900.0);

    let mut idle = DockState::default();
    idle.toggle();
    drive_vdock(&ctx, &mut idle, Vec::new(), sz);
    assert!(
        ctx.read_response(surface_badge_id(Surface::Chat)).is_none(),
        "zero unread paints no Chat badge"
    );
    assert!(
        ctx.read_response(surface_badge_id(Surface::MeshView))
            .is_none(),
        "unseen mesh paints no Mesh badge"
    );
    assert!(
        ctx.read_response(surface_badge_id(Surface::System))
            .is_none(),
        "unseen health paints no System health dot"
    );

    let mut live = DockState::default();
    live.toggle();
    live.set_status_inputs(
        MeshSummary {
            peers_total: 5,
            peers_online: 4,
            health: mde_lighthouse_health::LighthouseHealth::Degraded,
            seen: true,
        },
        None,
        12,
        false,
        Vec::new(),
        NodeGrades::default(),
        StatusSegments::default(),
    );
    drive_vdock(&ctx, &mut live, Vec::new(), sz);
    drive_vdock(&ctx, &mut live, Vec::new(), sz);

    let chat = ctx
        .read_response(pick_cell_id(Surface::Chat))
        .expect("Chat picker cell is registered")
        .rect;
    let chat_badge = ctx
        .read_response(surface_badge_id(Surface::Chat))
        .expect("Chat unread count registers a badge")
        .rect;
    assert!(
        chat.contains(chat_badge.center()),
        "Chat unread badge is anchored inside the Chat glyph cell"
    );

    let mesh = ctx
        .read_response(pick_cell_id(Surface::MeshView))
        .expect("Mesh picker cell is registered")
        .rect;
    let mesh_badge = ctx
        .read_response(surface_badge_id(Surface::MeshView))
        .expect("Mesh peer count registers a badge")
        .rect;
    assert!(
        mesh.contains(mesh_badge.center()),
        "Mesh peer badge is anchored inside the Mesh glyph cell"
    );

    let system = ctx
        .read_response(sys_cell_id(SysCell::Settings))
        .expect("System settings cell is registered")
        .rect;
    let system_badge = ctx
        .read_response(surface_badge_id(Surface::System))
        .expect("mesh health registers a System health dot")
        .rect;
    assert!(
        system.contains(system_badge.center()),
        "System health dot is anchored inside the Settings glyph cell"
    );
    assert_eq!(super::badge_label(12), "12");
    assert_eq!(super::badge_label(4), "4");
}

#[test]
fn the_overflow_more_popup_routes_a_hidden_group_surface() {
    // #22 — on a short screen the lower groups fold into the '…' more-popup:
    // the '…' cell is present, clicking it opens the popup, and a popup cell
    // still routes to its Surface (then closes the popup).
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    // Short enough that the last groups (incl. Media) overflow the app zone.
    let sz = egui::vec2(1280.0, 600.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let more = ctx
        .read_response(overflow_more_id())
        .expect("the '…' overflow cell is registered on a short screen")
        .rect;
    assert!(!s.overflow_open, "the popup starts closed");
    assert!(
        ctx.read_response(pick_cell_id(Surface::Media)).is_none(),
        "Media is folded into the overflow, not an inline cell yet"
    );

    // Click '…' → the popup opens.
    click_vdock(&ctx, &mut s, more.center(), sz);
    assert!(s.overflow_open, "clicking '…' opens the more-popup");

    // Settle the popup so its cells register, then click Media inside it.
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let media = ctx
        .read_response(pick_cell_id(Surface::Media))
        .expect("the overflowed Media cell renders in the popup")
        .rect
        .center();
    click_vdock(&ctx, &mut s, media, sz);
    assert_eq!(
        s.active,
        Surface::Media,
        "a click in the more-popup routes to its Surface"
    );
    assert!(!s.overflow_open, "routing from the popup closes it");
}

// ── VDOCK-5: the clock strip (Timers & Alarms home, locks #16/#20) ─────────

#[test]
fn the_clock_strip_shows_the_live_time_and_routes_to_timers() {
    // Lock #20 — the clock-glyph cell: it paints the LIVE wall-clock HH:MM
    // as its glyph (the time IS the icon), sits atop the bottom zone above
    // the status strip, and a click opens the Timers & Alarms surface.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    let sz = egui::vec2(1280.0, 900.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let cell = ctx
        .read_response(clock_cell_id())
        .expect("the clock strip is registered")
        .rect;
    assert!(
        cell.width() > 0.0 && cell.width() < sz.x / 6.0,
        "the clock is a bounded tray item ({}px), not the whole bar — at the 48px \
         Win10 height it is wider than a square cell to fit HH:MM (and date)",
        cell.width()
    );
    assert!(
        cell.left() > DOCK_W,
        "the clock is no longer in the left rail"
    );

    // A click routes to Timers & Alarms (the surface's ONE home).
    assert_ne!(s.active, Surface::Timers, "start off the Timers surface");
    click_vdock(&ctx, &mut s, cell.center(), sz);
    assert_eq!(
        s.active,
        Surface::Timers,
        "clicking the clock opens Timers & Alarms (lock #20)"
    );
}

#[test]
fn timers_home_is_the_clock_cell_not_the_picker() {
    // Lock #20 — Timers deliberately sits OUTSIDE `Surface::ALL` (the picker
    // ordering authority) and every group: its one launcher is the clock
    // strip, so the picker/glyph tables stay exactly the 17 picker surfaces.
    assert!(
        !Surface::ALL.contains(&Surface::Timers),
        "Timers is not a picker surface — the clock strip is its home"
    );
    assert!(
        GROUPS
            .iter()
            .all(|g| !g.surfaces.contains(&Surface::Timers)),
        "no group lists Timers"
    );
}

// ── NOTIF-3: the status strip wired into the dock's bottom zone ────────────

#[test]
fn a_status_segment_pip_routes_through_the_dock_bottom_zone() {
    // NOTIF-3 wired end-to-end: the shell feeds the compact status strip via
    // `set_status_inputs` and a click on a bottom-zone segment pip routes
    // `DockState::active` (lock #15). Mount the real dock, read the Alerts pip
    // centre by its stable id, and click it -> `active` follows to Chat.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle(); // reveal the dock so its Area (and the status strip) mount
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

    let alerts = ctx
        .read_response(status::segment_pip_id(status::StatusSegment::Alerts))
        .expect("the Alerts status segment is registered in the dock's bottom zone")
        .rect
        .center();
    assert_ne!(s.active, Surface::Chat, "start off the Chat surface");
    click_vdock(&ctx, &mut s, alerts, sz);
    assert_eq!(
        s.active,
        Surface::Chat,
        "clicking the Alerts segment routes to the Chat surface (lock #15)"
    );
}

#[test]
fn navbar4_status_tray_is_folded_into_the_bottom_rail() {
    // NAVBAR-4 — the separate chrome/status strip is retired: status pips,
    // the detail toggle, and the clock all live in the full-width bottom
    // taskbar. The old `status_bar` local-grade pip id must not register from
    // the dock. WIN7-1 lock #3 adds the pin check: the auto-hide pin trails
    // the clock rather than interrupting Start · sessions · tray · clock.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    s.set_status_inputs(
        MeshSummary {
            peers_total: 4,
            peers_online: 3,
            health: mde_lighthouse_health::LighthouseHealth::AllHealthy,
            seen: true,
        },
        None,
        2,
        true,
        Vec::new(),
        grades(vec![grade("me", 95, true, false)]),
        StatusSegments::default(),
    );
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let start = ctx
        .read_response(start_cell_id())
        .expect("Start cell is in the bottom rail")
        .rect;
    let clock = ctx
        .read_response(clock_cell_id())
        .expect("clock is in the bottom rail")
        .rect;
    let detail = ctx
        .read_response(status_detail_toggle_id())
        .expect("status detail toggle is in the bottom rail")
        .rect;
    let alerts = ctx
        .read_response(status::segment_pip_id(status::StatusSegment::Alerts))
        .expect("status pips are in the bottom rail")
        .rect;
    let pin = ctx
        .read_response(egui::Id::new("vdock-pin"))
        .expect("pin is in the bottom rail")
        .rect;
    assert!(
        [start, detail, alerts, clock, pin]
            .into_iter()
            .all(|r| (r.center().y - start.center().y).abs() < 2.0),
        "Start, status pips, detail toggle, clock, and pin share one bottom rail"
    );
    assert!(
        detail.left() > start.right()
            && alerts.left() > detail.right()
            && clock.left() > alerts.right(),
        "status tray is right-aligned after the left-side launcher/session run"
    );
    assert!(
        pin.left() >= clock.right() - 1.0,
        "WIN7-1 lock #3: the auto-hide pin trails the clock rather than \
         interrupting Start · sessions · tray · clock"
    );
    assert!(
        ctx.read_response(status::local_grade_pip_id()).is_none(),
        "the retired separate status_bar local-grade pip is not mounted"
    );

    click_vdock(&ctx, &mut s, detail.center(), sz);
    assert!(
        s.status_panel_open,
        "bottom-rail detail toggle opens status panel"
    );
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    assert!(
        ctx.read_response(status::status_panel_id()).is_some(),
        "the folded status tray still exposes the old chrome detail content"
    );
}

// ── WIN7-1: the bottom rail is now explicitly the Win7-style taskbar ───────
// (design `docs/design/win7-desktop-survey.md`) — pure relocation + a density
// pass over content that NAVBAR/NOTIF/CONSOLE-1 already folded into one rail;
// these tests pin the locked lock #3 order + the lock #12 density trim as
// their own explicit contract, on top of the pre-existing coverage above.

#[test]
fn win7_1_the_taskbar_reads_start_sessions_tray_clock_left_to_right() {
    // Lock #3: Start · running sessions · tray · clock, left to right. Extends
    // navbar4_status_tray_is_folded_into_the_bottom_rail with a real session
    // entry between Start and the tray, and pins the exact four-segment order
    // end to end (not just "everything shares a row").
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

    let start = ctx
        .read_response(start_cell_id())
        .expect("Start cell registered")
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
    let pin = ctx
        .read_response(egui::Id::new("vdock-pin"))
        .expect("pin registered")
        .rect;

    assert!(
        start.right() <= session.left() + 1.0,
        "Start sits left of the running-sessions run"
    );
    assert!(
        session.right() <= tray.left() + 1.0,
        "sessions sit left of the tray"
    );
    assert!(
        tray.right() <= clock.left() + 1.0,
        "the tray sits left of the clock"
    );
    assert!(
        clock.right() <= pin.left() + 1.0,
        "the auto-hide pin trails the clock rather than interrupting the \
         locked Start · sessions · tray · clock order"
    );
    for (label, r) in [
        ("session", session),
        ("tray", tray),
        ("clock", clock),
        ("pin", pin),
    ] {
        assert!(
            (r.center().y - start.center().y).abs() < 2.0,
            "{label} shares the Start cell's row"
        );
    }
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
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let start = ctx
        .read_response(start_cell_id())
        .expect("Start cell registered")
        .rect;
    let clock = ctx
        .read_response(clock_cell_id())
        .expect("clock registered")
        .rect;
    let pin = ctx
        .read_response(egui::Id::new("vdock-pin"))
        .expect("pin registered")
        .rect;

    for (label, r) in [("Start", start), ("clock", clock), ("pin", pin)] {
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
fn navbar8_shell_density_selects_compact_or_expanded_bottom_rail() {
    // NAVBAR-8 — the rail rides the same density the shell installs from the
    // formfactor/control-surface path. Pointer density keeps the compact
    // icon rail; touch density grows the labelled 48px variant.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let sz = egui::vec2(1280.0, 800.0);

    let mut compact = DockState::default();
    compact.set_density(Density::Mouse);
    drive_vdock(&ctx, &mut compact, Vec::new(), sz);
    drive_vdock(&ctx, &mut compact, Vec::new(), sz);
    let compact_start = ctx
        .read_response(start_cell_id())
        .expect("compact Start cell registers")
        .rect;
    assert!(
        (compact_start.height() - NOTIFICATION_RAIL_H + 4.0).abs() < 1.0,
        "compact density keeps the short icon rail"
    );

    let ctx = egui::Context::default();
    Style::install_with_density(&ctx, Density::Touch);
    let mut expanded = DockState::default();
    expanded.set_density(Density::Touch);
    drive_vdock(&ctx, &mut expanded, Vec::new(), sz);
    drive_vdock(&ctx, &mut expanded, Vec::new(), sz);
    let expanded_start = ctx
        .read_response(start_cell_id())
        .expect("expanded Start cell registers")
        .rect;
    let expanded_desktop = ctx
        .read_response(pick_cell_id(Surface::Desktop))
        .expect("expanded Desktop cell registers through the same surface id")
        .rect;
    assert!(
        expanded_start.height() >= NOTIFICATION_RAIL_EXPANDED_H - 6.0,
        "expanded density selects the 48px rail variant"
    );
    assert!(
        (expanded_start.center().y - expanded_desktop.center().y).abs() < 2.0,
        "expanded Start and Desktop still share one bottom rail"
    );
    assert!(
        (expanded_desktop.width() - compact_start.width()).abs() < 2.0,
        "WIN10-HYBRID: the taskbar cells are the same 48px size regardless of density"
    );
}

#[test]
fn a_requested_desktop_session_renders_as_a_named_bottom_rail_entry() {
    // NAVBAR-U3 / operator rail request — once the Desktop surface has a real
    // requested target, the rail shows a taskbar-style session entry instead of
    // only the generic Sessions glyph. Clicking it focuses Desktop.
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

    let rect = ctx
        .read_response(session_entry_id(0, &entry))
        .expect("the named session entry is registered")
        .rect;
    assert!(
        rect.width() > NOTIFICATION_RAIL_H,
        "session entries render wider than the fallback micro icon"
    );
    assert_ne!(s.active(), Surface::Desktop, "starts off Desktop");
    click_vdock(&ctx, &mut s, rect.center(), sz);
    assert_eq!(
        s.active(),
        Surface::Desktop,
        "session entry focuses Desktop"
    );
    assert_eq!(
        s.take_desktop_session_focus().as_deref(),
        Some("session-1"),
        "session entry latches its broker session id for the shell"
    );
}

#[test]
fn navbar7_bottom_rail_more_popup_keeps_overflow_sessions_reachable() {
    let ctx = egui::Context::default();
    Style::install(&ctx);
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
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    assert!(
        ctx.read_response(rail_more_id()).is_some(),
        "narrow rails render a More cell instead of silently dropping sessions"
    );
    assert!(
        ctx.read_response(session_entry_id(3, &entries[3]))
            .is_none(),
        "the trailing session is folded out of the inline rail"
    );

    let more = ctx
        .read_response(rail_more_id())
        .expect("More cell is registered")
        .rect
        .center();
    click_vdock(&ctx, &mut s, more, sz);
    assert!(s.rail_more_open, "clicking More opens the overflow popup");
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let hidden = ctx
        .read_response(session_entry_id(3, &entries[3]))
        .expect("the hidden session is reachable in the More popup")
        .rect
        .center();
    assert!(hidden.x > 0.0, "hidden popup row has a concrete hit rect");
    ctx.memory_mut(|m| m.request_focus(session_entry_id(3, &entries[3])));
    drive_vdock(&ctx, &mut s, vec![key(egui::Key::Enter)], sz);
    assert_eq!(s.active(), Surface::Desktop);
    assert_eq!(
        s.take_desktop_session_focus().as_deref(),
        Some("s4"),
        "clicking a popup session uses the same focus latch as inline entries"
    );
    assert!(!s.rail_more_open, "routing from More closes the popup");
}

#[test]
fn the_desktop_rail_cell_latches_a_reconnect_request() {
    // NAVBAR-U1 — a Desktop rail click is more than navigation: the shell
    // drains a distinct reconnect request and asks ChooserState for the newest
    // recent desktop. Programmatic Desktop navigation does not set this latch.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    assert!(
        !s.take_desktop_reconnect(),
        "no reconnect latch before a rail click"
    );
    let desktop = ctx
        .read_response(pick_cell_id(Surface::Desktop))
        .expect("the Desktop rail cell is registered")
        .rect
        .center();
    click_vdock(&ctx, &mut s, desktop, sz);
    assert_eq!(
        s.active(),
        Surface::Desktop,
        "Desktop still routes normally"
    );
    assert!(
        s.take_desktop_reconnect(),
        "Desktop rail click latches reconnect"
    );
    assert!(
        !s.take_desktop_reconnect(),
        "the reconnect latch drains once"
    );
}

#[test]
fn the_desktop_rail_caret_opens_sources_and_latches_a_pick() {
    // NAVBAR-U2 — the Desktop rail is a split control: main icon reconnects,
    // caret opens the compact source flyout, and a row click returns only the
    // source id for the shell to hand back to ChooserState.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    let sz = egui::vec2(1280.0, 800.0);
    let sources = vec![DesktopRailSource::new(
        "peer:oak",
        "oak",
        "lighthouse-oak",
        "RDP",
        true,
        true,
        false,
    )];
    drive_vdock_with_sources(&ctx, &mut s, Vec::new(), sz, &sources);
    drive_vdock_with_sources(&ctx, &mut s, Vec::new(), sz, &sources);

    let caret = ctx
        .read_response(desktop_source_toggle_id())
        .expect("the Desktop source caret is registered")
        .rect
        .center();
    click_vdock_with_sources(&ctx, &mut s, caret, sz, &sources);
    assert!(s.desktop_sources_open, "caret opens the source flyout");

    // Three settle frames, not one, before reading the flyout row's rect —
    // verified directly against the vendored egui 0.31.1 source (both
    // `containers/area.rs` and `context.rs`'s own `read_response` doc
    // comment), not guessed:
    //   1. The flyout's `egui::Area` uses a non-default `.pivot(LEFT_
    //      BOTTOM)` (open upward from the Desktop cell). Egui doesn't know
    //      an Area's content size until AFTER it has been laid out once,
    //      so the FIRST frame an Area id is ever shown is a "sizing pass"
    //      that positions it using a hardcoded `(600, 400)` placeholder
    //      size (`Spacing::default_area_size`) — wildly wrong for this
    //      36pt-tall popup. The REAL size gets recorded at the end of
    //      that same frame, so the frame right after already computes the
    //      correct position (confirmed directly: `AreaState::load` goes
    //      from `None` to `Some(size: [240, 36])` between these two
    //      frames).
    //   2. `Context::read_response`'s own doc: "widget interaction
    //      happens at the start of the pass, using the widget rects from
    //      the PREVIOUS pass" — so reading a rect right after the frame
    //      that first computed it correctly still returns the STALE
    //      (sizing-pass) one; a THIRD frame is what makes the correct
    //      rect the "previous pass" `read_response` actually returns.
    // Pre-fix this was invisible: the flyout's anchor sat at literal
    // screen y≈0, where the wrong sizing-pass estimate and the correct
    // position happened to land within a couple of points of each other
    // (both near the top), not the ~360pt gap they differ by once the
    // WIN7-DESKTOP-1 taskbar-position fix moved the anchor to the true
    // screen bottom — so this dance, always technically required, only
    // became load-bearing now.
    drive_vdock_with_sources(&ctx, &mut s, Vec::new(), sz, &sources);
    drive_vdock_with_sources(&ctx, &mut s, Vec::new(), sz, &sources);
    drive_vdock_with_sources(&ctx, &mut s, Vec::new(), sz, &sources);
    let row = ctx
        .read_response(desktop_source_row_id(&sources[0]))
        .expect("the source row is registered")
        .rect
        .center();
    click_vdock_with_sources(&ctx, &mut s, row, sz, &sources);
    assert_eq!(
        s.take_desktop_source_pick().as_deref(),
        Some("peer:oak"),
        "row click latches the selected chooser source id"
    );
    assert_eq!(s.active(), Surface::Desktop, "source pick focuses Desktop");
    assert!(
        !s.desktop_sources_open,
        "routing from the source flyout closes it"
    );
}

#[test]
fn the_desktop_split_button_is_keyboard_reachable() {
    // NAVBAR-U1 — keyboard focus + Enter/Space must activate both halves of the
    // Desktop split control and the compact source rows.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    let sz = egui::vec2(1280.0, 800.0);
    let sources = vec![DesktopRailSource::new(
        "peer:oak",
        "oak",
        "lighthouse-oak",
        "RDP",
        true,
        false,
        true,
    )];
    drive_vdock_with_sources(&ctx, &mut s, Vec::new(), sz, &sources);
    drive_vdock_with_sources(&ctx, &mut s, Vec::new(), sz, &sources);

    ctx.memory_mut(|m| m.request_focus(pick_cell_id(Surface::Desktop)));
    drive_vdock_with_sources(&ctx, &mut s, vec![key(egui::Key::Enter)], sz, &sources);
    assert_eq!(s.active(), Surface::Desktop);
    assert!(
        s.take_desktop_reconnect(),
        "Enter on the Desktop half latches reconnect"
    );

    ctx.memory_mut(|m| m.request_focus(desktop_source_toggle_id()));
    drive_vdock_with_sources(&ctx, &mut s, vec![key(egui::Key::Space)], sz, &sources);
    assert!(
        s.desktop_sources_open,
        "Space on the caret opens the source flyout"
    );

    drive_vdock_with_sources(&ctx, &mut s, Vec::new(), sz, &sources);
    ctx.memory_mut(|m| m.request_focus(desktop_source_row_id(&sources[0])));
    drive_vdock_with_sources(&ctx, &mut s, vec![key(egui::Key::Enter)], sz, &sources);
    assert_eq!(
        s.take_desktop_source_pick().as_deref(),
        Some("peer:oak"),
        "Enter on a source row latches that chooser source"
    );
}

#[test]
fn navbar6_arrow_keys_traverse_picker_glyph_focus() {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    let sz = egui::vec2(1280.0, 900.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    ctx.memory_mut(|m| m.request_focus(pick_cell_id(Surface::Workbench)));
    // One settle frame so `apply_picker_arrow_focus` gets a chance to claim
    // vertical/horizontal arrows as Workbench's OWN `EventFilter`
    // (`set_focus_lock_filter`, the WIN7-DESKTOP-1 regression fix below)
    // BEFORE the ArrowDown frame: egui's `Focus::begin_pass` decides
    // whether to run its OWN built-in spatial arrow-key nav using
    // whatever filter the focused widget ALREADY carried as of the START
    // of a frame, not one set mid-frame, so a filter claimed reactively
    // this same frame arrives one frame too late to matter. A real user
    // always presses a key at least one rendered frame after Tab/click
    // gives a widget focus, so this settle frame matches production
    // timing — it's this test's OWN prior same-instant focus+keypress
    // that was artificial, not a new requirement this fix invented.
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, vec![key(egui::Key::ArrowDown)], sz);
    assert_eq!(
        ctx.memory(|m| m.focused()),
        Some(pick_cell_id(Surface::Voice)),
        "ArrowDown advances from Workbench to the first grouped glyph"
    );

    drive_vdock(&ctx, &mut s, vec![key(egui::Key::ArrowUp)], sz);
    assert_eq!(
        ctx.memory(|m| m.focused()),
        Some(pick_cell_id(Surface::Workbench)),
        "ArrowUp traverses back to Workbench"
    );
}

#[test]
fn navbar6_right_click_glyph_menu_offers_pin_info_and_close_when_closable() {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    let sz = egui::vec2(1280.0, 900.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let workbench = ctx
        .read_response(pick_cell_id(Surface::Workbench))
        .expect("Workbench cell is registered")
        .rect
        .center();
    secondary_click_vdock(&ctx, &mut s, workbench, sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    assert!(
        ctx.read_response(surface_context_item_id(
            Surface::Workbench,
            SurfaceContextItem::Pin
        ))
        .is_some(),
        "right-clicking a glyph opens a Pin row"
    );
    assert!(
        ctx.read_response(surface_context_item_id(
            Surface::Workbench,
            SurfaceContextItem::Info
        ))
        .is_some(),
        "right-clicking a glyph opens an Info row"
    );
    assert!(
        ctx.read_response(surface_context_item_id(
            Surface::Workbench,
            SurfaceContextItem::Close
        ))
        .is_none(),
        "anchor glyphs omit Close rather than showing a placebo"
    );

    let pin = ctx
        .read_response(surface_context_item_id(
            Surface::Workbench,
            SurfaceContextItem::Pin,
        ))
        .expect("Pin row remains registered")
        .rect
        .center();
    click_vdock(&ctx, &mut s, pin, sz);
    assert!(s.pinned(), "Pin toggles the dock hold-open state");
    click_vdock(&ctx, &mut s, egui::pos2(600.0, 400.0), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let initial_browser = ctx
        .read_response(pick_cell_id(Surface::Browser))
        .expect("Browser cell is registered")
        .rect
        .center();
    assert!(initial_browser.x > 0.0, "Browser has a concrete cell");
    s.set_active(Surface::Browser);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    assert_eq!(s.active(), Surface::Browser);
    let browser = ctx
        .read_response(pick_cell_id(Surface::Browser))
        .expect("Browser cell remains registered after activation")
        .rect
        .center();
    secondary_click_vdock(&ctx, &mut s, browser, sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let close = ctx
        .read_response(surface_context_item_id(
            Surface::Browser,
            SurfaceContextItem::Close,
        ))
        .expect("closable Browser glyph exposes Close")
        .rect
        .center();
    click_vdock(&ctx, &mut s, close, sz);
    assert_eq!(
        s.active(),
        Surface::Workbench,
        "Close on the closable Desktop glyph returns to Workbench"
    );
}

#[test]
fn the_status_chevron_opens_and_dismisses_the_detail_panel() {
    // NOTIF-4 — the detail panel is now mounted from the bottom rail; Esc and
    // click-away both dismiss it.
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
        .expect("bottom-rail status caret renders")
        .rect
        .center();
    click_vdock(&ctx, &mut s, caret, sz);
    assert!(
        s.status_panel_open,
        "bottom-rail caret opens the status panel"
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
    assert!(s.status_panel_open, "test seam reopens the panel");
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    click_vdock(&ctx, &mut s, egui::pos2(500.0, 500.0), sz);
    assert!(!s.status_panel_open, "click-away dismisses the panel");
}

#[test]
fn the_status_panel_routes_device_controls_and_grade_rows() {
    // NOTIF-4 — device controls route to System; grade rows request the same
    // Explorer node-focus path as the dock mini-list.
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
        grades(vec![
            grade("me", 95, true, false),
            grade("oak", 42, false, false),
        ]),
        StatusSegments::default(),
    );
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    s.open_status_panel_for_test();
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let device = ctx
        .read_response(status::status_panel_device_id())
        .expect("device control band registered")
        .rect
        .center();
    assert_ne!(s.active, Surface::System, "start off System");
    click_vdock(&ctx, &mut s, device, sz);
    assert_eq!(s.active, Surface::System, "device band routes to System");
    assert!(
        !s.status_panel_open,
        "routing from the panel closes the auxiliary panel"
    );

    s.open_status_panel_for_test();
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let oak = ctx
        .read_response(status::status_panel_grade_id("oak"))
        .expect("peer grade row registered")
        .rect
        .center();
    click_vdock(&ctx, &mut s, oak, sz);
    assert_eq!(
        s.take_node_focus().as_deref(),
        Some("oak"),
        "tapping a panel grade row records a node-focus request"
    );
}

// ── VDOCK-4: the system 2×2 quad + Power menu (design #7/#17/#18) ──────────

#[test]
fn the_system_quad_cells_are_settings_desktop_lock_power() {
    // Design #7/#17 — the four cells, row-major, sized to match the ~18px status
    // quad icons (#12/#23, smaller than the 24px app glyph).
    assert_eq!(
        SYSTEM_QUAD,
        [
            SysCell::Settings,
            SysCell::ShowDesktop,
            SysCell::Lock,
            SysCell::Power
        ],
        "the system quad is Settings · Show-Desktop · Lock · Power"
    );
    assert!(
        (SYS_QUAD_ICON - 18.0).abs() < f32::EPSILON,
        "the quad glyph edge is ~18px (design #23)"
    );
    assert!(
        SYS_QUAD_ICON < ICON_LOGICAL,
        "the quad icon is smaller than the 24px app glyph (#12)"
    );
}

#[test]
fn system_quad_cells_use_status_pip_tint_language() {
    // NOTIF-12 — inactive controls share the status-pip dim baseline; hover or
    // active state reveals each action's semantic Carbon tone.
    assert_eq!(
        sys_cell_tint(SysCell::Settings, false, false),
        Style::TEXT_DIM
    );
    assert_eq!(
        sys_cell_tint(SysCell::Settings, true, false),
        Style::SUPPORT_INFO
    );
    assert_eq!(
        sys_cell_tint(SysCell::ShowDesktop, true, false),
        Style::SUPPORT_SUCCESS
    );
    assert_eq!(
        sys_cell_tint(SysCell::Lock, false, true),
        Style::SUPPORT_WARNING
    );
    assert_eq!(
        sys_cell_tint(SysCell::Power, true, false),
        Style::SUPPORT_ERROR
    );
}

#[test]
fn the_system_quad_lays_out_as_a_2x2_in_the_final_dock_row() {
    // Design #7/#8 — the four cells form a 2×2 of DOCK_W/2 cells in the reserved
    // final DOCK_W row (directly beneath NOTIF-3's status strip).
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle(); // reveal the dock so its Area (and the quad) mount
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let rect_of = |cell| {
        ctx.read_response(sys_cell_id(cell))
            .expect("system-quad cell registered")
            .rect
    };
    let (tl, tr, bl, br) = (
        rect_of(SysCell::Settings),
        rect_of(SysCell::ShowDesktop),
        rect_of(SysCell::Lock),
        rect_of(SysCell::Power),
    );
    let cell = DOCK_W / 2.0;
    for r in [tl, tr, bl, br] {
        assert!((r.width() - cell).abs() < 1.0, "cell is DOCK_W/2 wide");
        assert!((r.height() - cell).abs() < 1.0, "cell is DOCK_W/2 tall");
    }
    // Two columns: left cells share a left edge, right cells one cell over.
    assert!((tl.left() - bl.left()).abs() < 1.0, "left column aligned");
    assert!(
        (tr.left() - tl.right()).abs() < 1.0,
        "right column one cell over"
    );
    assert!((br.left() - tr.left()).abs() < 1.0, "right column aligned");
    // Two rows: top cells share a top edge, bottom cells one row down.
    assert!((tl.top() - tr.top()).abs() < 1.0, "top row aligned");
    assert!(
        (bl.top() - tl.bottom()).abs() < 1.0,
        "bottom row one cell down"
    );
    assert!((br.top() - bl.top()).abs() < 1.0, "bottom row aligned");
    // The quad sits directly above the Windows-style bottom taskbar (the
    // default DockState is Density::Mouse, so the compact rail height applies).
    assert!(
        (tl.top() - (sz.y - DOCK_W - NOTIFICATION_RAIL_H)).abs() < 1.0,
        "the system quad occupies the row above the bottom rail"
    );
    // It spans the full column width (two DOCK_W/2 columns).
    assert!(
        (tr.right() - tl.left() - DOCK_W).abs() < 1.0,
        "the quad spans the column width"
    );
}

#[test]
fn each_system_quad_cell_dispatches_its_route_or_action() {
    // §7 — every system-quad cell drives its real target on a click: Settings →
    // System, Show-Desktop → the existing Desktop route, Lock → a curtain lock
    // request the shell drains, Power → the armed menu opens.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    // Read all four centres up front (a click shifts no rect); click Power last
    // so its popup can't overlap the earlier cells.
    let centre = |cell| {
        ctx.read_response(sys_cell_id(cell))
            .expect("system-quad cell registered")
            .rect
            .center()
    };
    let (settings, desktop, lock, power) = (
        centre(SysCell::Settings),
        centre(SysCell::ShowDesktop),
        centre(SysCell::Lock),
        centre(SysCell::Power),
    );

    // Settings → System.
    assert_ne!(s.active, Surface::System, "start off System");
    click_vdock(&ctx, &mut s, settings, sz);
    assert_eq!(s.active, Surface::System, "Settings routes to System");

    // Show-Desktop → Desktop (the existing route).
    click_vdock(&ctx, &mut s, desktop, sz);
    assert_eq!(s.active, Surface::Desktop, "Show-Desktop routes to Desktop");

    // Lock → a pending curtain lock request the shell drains (once).
    click_vdock(&ctx, &mut s, lock, sz);
    assert_eq!(
        s.take_request(),
        Some(DockRequest::Lock),
        "Lock records a curtain lock request"
    );
    assert!(
        s.take_request().is_none(),
        "the request drains once (the shell reads it a single time)"
    );

    // Power → the armed menu opens.
    assert!(!s.power.open, "the Power menu is closed by default");
    click_vdock(&ctx, &mut s, power, sz);
    assert!(s.power.open, "clicking Power opens its menu (#18)");
}

#[test]
fn the_power_menu_arms_reboot_and_shutdown_before_firing() {
    // Design #18 — the two host-down verbs demand a typed echo before they fire.
    // The pure arming gate: an empty / mistyped echo never arms; only the exact
    // (case-insensitive) verb name does.
    let mut menu = PowerMenu::default();
    menu.arm(PowerItem::Reboot);
    assert!(!menu.armed(), "an empty echo never arms Reboot");
    menu.arming.as_mut().expect("arming set").echo = "nope".to_owned();
    assert!(!menu.armed(), "a mistyped echo never arms Reboot");
    menu.arming.as_mut().expect("arming set").echo = "reboot".to_owned();
    assert!(menu.armed(), "the exact verb name (any case) arms it");

    // The fired verb drives the REAL seam the shell drains: Reboot → PowerVerb::
    // Reboot, Shutdown → PowerVerb::PowerOff; each drains once.
    let mut s = DockState::default();
    s.power.arm(PowerItem::Reboot);
    s.power.arming.as_mut().expect("arming set").echo = "Reboot".to_owned();
    assert!(s.power.armed(), "the dock's arming gate matches");
    s.fire_power(PowerItem::Reboot);
    assert_eq!(
        s.take_request(),
        Some(DockRequest::Power(PowerVerb::Reboot)),
        "a confirmed Reboot records the real logind verb"
    );
    assert!(s.take_request().is_none(), "the request drains once");
    assert!(!s.power.open, "firing a verb closes the menu");

    s.fire_power(PowerItem::Shutdown);
    assert_eq!(
        s.take_request(),
        Some(DockRequest::Power(PowerVerb::PowerOff)),
        "Shutdown maps to logind PowerOff"
    );

    // Suspend acts at once (no arming); Lock routes to the curtain, not a verb.
    s.fire_power(PowerItem::Suspend);
    assert_eq!(
        s.take_request(),
        Some(DockRequest::Power(PowerVerb::Suspend))
    );
    s.fire_power(PowerItem::Lock);
    assert_eq!(
        s.take_request(),
        Some(DockRequest::Lock),
        "the menu's Lock item drops the curtain, not a logind verb"
    );
}

#[test]
fn clicking_reboot_in_the_menu_only_arms_it_and_fires_nothing() {
    // Design #18 end-to-end: opening the Power menu and clicking Reboot enters
    // the typed-arming stage — it does NOT reboot (no power request fires until
    // the echo is confirmed). Guards the "one click reboots" trap.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    // Open the menu off the Power cell.
    let power = ctx
        .read_response(sys_cell_id(SysCell::Power))
        .expect("Power cell registered")
        .rect
        .center();
    click_vdock(&ctx, &mut s, power, sz);
    assert!(s.power.open, "the menu opened");

    // Settle so the popup rows register, then click Reboot.
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let reboot = ctx
        .read_response(power_item_id(PowerItem::Reboot))
        .expect("the Reboot menu row renders in the popup")
        .rect
        .center();
    click_vdock(&ctx, &mut s, reboot, sz);

    assert!(
        s.power.arming.as_ref().map(|a| a.verb) == Some(PowerItem::Reboot),
        "clicking Reboot enters its typed-arming stage"
    );
    assert!(
        s.take_request().is_none(),
        "Reboot fires NOTHING until the echo is typed-armed (#18)"
    );

    // The top-level Power menu offers exactly the four locked items.
    assert_eq!(
        POWER_MENU,
        [
            PowerItem::Lock,
            PowerItem::Suspend,
            PowerItem::Reboot,
            PowerItem::Shutdown
        ],
        "the Power menu is Lock / Suspend / Reboot / Shutdown (#18)"
    );
}

// ── NODE-GRADE-2: the grade mini-list band (design #5/#7/#8/#18/#19) ───────

#[test]
fn the_grade_band_has_no_height_without_grades() {
    // Pre-poll / empty grades → the band claims 0, so the dock's layout is
    // byte-identical to the pre-NODE-GRADE dock (§7 honest: no fake rows).
    assert!(
        grade_band_height(&NodeGrades::default()).abs() < f32::EPSILON,
        "an empty grade set paints no band"
    );
    assert!(
        grade_band_height(&grades(vec![grade("me", 90, true, false)])) > 0.0,
        "one grade claims a band"
    );
}

#[test]
fn the_grade_rows_sit_above_the_bottom_status_tray_local_first() {
    // Design #18/#19 — the grade mini-list paints in the bottom zone ABOVE the
    // NOTIF-3 status strip, in the given render order (local pinned first). The
    // rows register addressable rects and every one clears the strip.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    // Local "me" pinned first, then a worst-first peer.
    s.set_status_inputs(
        MeshSummary::default(),
        None,
        0,
        false,
        Vec::new(),
        grades(vec![
            grade("me", 95, true, false),
            grade("oak", 40, false, false),
        ]),
        StatusSegments::default(),
    );
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let me = ctx
        .read_response(grade_row_id("me"))
        .expect("the local grade row is registered")
        .rect;
    let oak = ctx
        .read_response(grade_row_id("oak"))
        .expect("the peer grade row is registered")
        .rect;
    // Local pinned first (renders above the peer), matching the fold order.
    assert!(
        me.top() < oak.top(),
        "the local node's row is pinned first (#18)"
    );
    // Both rows sit above the bottom rail's status tray.
    let tray = ctx
        .read_response(status::segment_pip_id(status::StatusSegment::Alerts))
        .expect("the bottom status tray is registered")
        .rect;
    assert!(
        tray.width() < DOCK_W,
        "the bottom status tray uses micro icons"
    );
    // Each row spans the full column width (the dock idiom).
    assert!(
        (me.width() - DOCK_W).abs() < 1.0,
        "a grade row is the full column"
    );
}

#[test]
fn tapping_a_grade_row_records_a_node_focus_request() {
    // Design #7 — a grade row tap records the host's Explorer-hero focus request
    // the shell drains (routing to the Mesh Map's Explorer lens). The request
    // drains exactly once (the shell reads it a single frame).
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
        grades(vec![grade("oak", 40, false, false)]),
        StatusSegments::default(),
    );
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    let oak = ctx
        .read_response(grade_row_id("oak"))
        .expect("the grade row is registered")
        .rect
        .center();
    assert!(
        s.take_node_focus().is_none(),
        "no focus request before the tap"
    );
    click_vdock(&ctx, &mut s, oak, sz);
    assert_eq!(
        s.take_node_focus().as_deref(),
        Some("oak"),
        "tapping a grade row records that node's hero-focus request (#7)"
    );
    assert!(
        s.take_node_focus().is_none(),
        "the focus request drains once"
    );
}

#[test]
fn the_grade_overflow_expander_reveals_the_hidden_peers() {
    // Design #8 — past the worst-N cap the extra peers fold into a '…' expander:
    // the '…' cell is present, the capped peer is hidden until it opens, and a
    // popup row still routes to its hero (then closes the popup).
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    // One local + (GRADE_MAX_ROWS) peers → one peer spills past the cap.
    let mut rows = vec![grade("me", 99, true, false)];
    let peers = ["p1", "p2", "p3", "p4", "p5"];
    assert_eq!(peers.len(), GRADE_MAX_ROWS, "seed exactly one over the cap");
    for (i, name) in peers.iter().enumerate() {
        // Ascending scores so the render order is stable; the last is hidden.
        rows.push(grade(
            name,
            10 + u8::try_from(i).unwrap_or(0) * 10,
            false,
            false,
        ));
    }
    let hidden_host = peers[peers.len() - 1];
    s.set_status_inputs(
        MeshSummary::default(),
        None,
        0,
        false,
        Vec::new(),
        grades(rows),
        StatusSegments::default(),
    );
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);

    assert!(
        ctx.read_response(grade_overflow_id()).is_some(),
        "the '…' expander is present when peers spill past the cap"
    );
    assert!(
        ctx.read_response(grade_row_id(hidden_host)).is_none(),
        "the capped peer is hidden until the expander opens"
    );

    // Open the expander.
    let more = ctx
        .read_response(grade_overflow_id())
        .expect("the '…' cell is registered")
        .rect
        .center();
    click_vdock(&ctx, &mut s, more, sz);
    assert!(s.grades_overflow_open, "clicking '…' opens the expander");

    // Settle the popup, then tap the hidden peer inside it.
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let hidden = ctx
        .read_response(grade_row_id(hidden_host))
        .expect("the hidden peer renders in the expander popup")
        .rect
        .center();
    click_vdock(&ctx, &mut s, hidden, sz);
    assert_eq!(
        s.take_node_focus().as_deref(),
        Some(hidden_host),
        "a tap in the expander routes to that node's hero"
    );
    assert!(
        !s.grades_overflow_open,
        "routing from the expander closes it"
    );
}

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
    // clock) plus the pin and the Desktop-source caret all export real
    // `Button` nodes, not just the tray pips `status.rs` already covered.
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz); // settle
    let out = drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let nodes = accesskit_nodes(&out);

    for label in [
        "Start",
        "Desktop",
        "Sessions",
        "Notification panel",
        "Clock",
        "Pin",
        "Desktop sources",
    ] {
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
fn win7_7_the_pin_and_notification_toggle_report_their_state_via_accesskit_value() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    Style::install(&ctx);
    let mut s = DockState::default();
    s.toggle();
    let sz = egui::vec2(1280.0, 800.0);
    drive_vdock(&ctx, &mut s, Vec::new(), sz); // settle
    let out = drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let nodes = accesskit_nodes(&out);
    let pin = nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some("Pin"))
        .expect("the pin exports an accesskit node");
    assert_eq!(pin.value(), Some("Not pinned"));
    let toggle = nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some("Notification panel"))
        .expect("the notification toggle exports an accesskit node");
    assert_eq!(toggle.value(), Some("Collapsed"));

    // Pin it, and open the notification panel — the SAME nodes must now
    // report the opposite state, not a value frozen at first paint.
    s.toggle_pin();
    s.status_panel_open = true;
    let out2 = drive_vdock(&ctx, &mut s, Vec::new(), sz);
    let nodes2 = accesskit_nodes(&out2);
    let pin2 = nodes2
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some("Pin"))
        .expect("the pin still exports an accesskit node");
    assert_eq!(pin2.value(), Some("Pinned"));
    let toggle2 = nodes2
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some("Notification panel"))
        .expect("the notification toggle still exports an accesskit node");
    assert_eq!(toggle2.value(), Some("Expanded"));
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
