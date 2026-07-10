//! WIN7-2 — the **Start Menu** shell (design `docs/design/win7-desktop-survey.md`,
//! locks #2/#4/#10/#13/#14; WIN7-DESKTOP-1's second implementation unit).
//!
//! The fixed-size overlay panel that replaces the dock's Start-cell-opens-
//! Console behaviour (WIN7-1 relabelled the cell "Start" but left its click
//! wired to Console directly — this unit is the real two-pane Start Menu lock
//! #4 describes). It reuses the SAME floating-`egui::Area` + [`Motion`]-tweened
//! slide-up pattern `console.rs`'s old standalone panel and `dock.rs`'s
//! vertical dock both already use (not a new mechanism): fixed-size (lock #2 —
//! never full-screen, never resizable), anchored bottom-left beside the
//! vertical dock column (`x = DOCK_W`, the Console front door's existing
//! footprint), opening **upward** from the bottom edge.
//!
//! **Panes (lock #4):** left = a placeholder for WIN7-3's live-tile grid (this
//! unit ships the shell only, no tile content); right = [`console::ConsoleState`]
//! embedded via [`console::console_content`] — CONSOLE-1's existing operational
//! front door (groups, Power section, Custom entries, the CONSOLE-2 spawn-tab
//! seam), unchanged and fully working, not a bare label. Migrating Console's
//! *content* here is real work this unit does; *redesigning* its presentation
//! for the new home (lock #10) is WIN7-5's job — today it renders exactly as
//! it always has, just embedded at this panel's right-pane rect instead of
//! mounting its own independent `Area`.
//!
//! **Console's open state is now a mirror, not a source of truth.** Before this
//! unit, `ConsoleState::open` was the Start cell's own toggle latch. Now this
//! module's [`StartMenuState::open`] is the ONE latch (driven by the Start
//! cell click AND the Super key, lock #13 — see `main.rs`'s `mount_start_menu`
//! and its hotkey dispatch); each frame it mirrors into `ConsoleState` via
//! [`console::ConsoleState::set_open`] (the `DockState::set_active` idiom)
//! before rendering, so Console's focus ring / `handle_keys` still read a
//! meaningful "am I showing" bit. The mirror runs the OTHER way too: every
//! action that already closed the whole front door pre-WIN7-2 (a routed link,
//! a spawned tab, a fired power verb, Esc inside the embedded content) still
//! calls `ConsoleState::close` exactly as before (untouched in `console.rs`);
//! this module detects that self-closure (`console.is_open()` having gone
//! false while `state.open` is still true) and dismisses the WHOLE Start Menu
//! with it, so launching anything from the embedded Console still closes the
//! menu the way a Win10 Start Menu always did — never a dangling "Console says
//! closed but the panel is still up" desync.
//!
//! **The VDOCK-1 / Super key overlap (a judgment call, not covered by the
//! survey):** the vertical dock (`dock.rs`) ALREADY binds a clean Super tap to
//! `DockState::toggle` (VDOCK-1, `docs/design/vertical-dock.md` lock #13) —
//! it's the shell's only surface launcher until WIN7-3 lands real tiles here,
//! so this unit must not steal Super away from it (that would strand every
//! surface behind an unpinned, now-unreachable dock). Lock #13 in the win7
//! survey just says "Super opens the Start Menu," without addressing that
//! pre-existing claim. Resolution: `main.rs` applies the SAME clean-Super-tap
//! drain (`HotkeyRouter::take_dock_toggle`) to BOTH `DockState::toggle` AND
//! `StartMenuState::toggle`, so one Super tap reveals both — not a conflict in
//! practice, since the Start Menu already mounts immediately beside the dock
//! column (`x = DOCK_W`), so revealing both together reads as "the whole nav
//! chrome" rather than two unrelated popups. This is a deliberate, flagged
//! choice, not a discovered fact — worth a confirm from the operator, and
//! likely moot once WIN7-3's tiles let the vertical dock retire.
//!
//! **Accesskit (lock #14):** the panel itself carries a role + label before any
//! content lands — `Role::Menu` for the whole panel, `Role::Group` landmarks
//! for each pane — so a screen reader can already navigate the shell. Deep
//! per-tile / per-row accesskit is WIN7-3's (tiles) and WIN7-7's (the full
//! sweep) job, not re-litigated here.

use mde_egui::egui;
use mde_egui::{Motion, Style};

use crate::console::{self, ConsoleState};
use crate::dock::DOCK_W;

// ── geometry ─────────────────────────────────────────────────────────────────

/// The stable id of the Start Menu's floating [`egui::Area`] layer.
const START_MENU_AREA: &str = "start-menu-area";

/// The egui memory key for the panel's slide animation (the `console.rs`
/// `SLIDE_KEY` / `dock.rs` `DOCK_SLIDE_KEY` idiom, restated here since the
/// slide/Area machinery now lives in this module instead).
const SLIDE_KEY: &str = "start-menu-slide";

/// A 1px hairline rule (the dock's/console's `HAIRLINE_W` restated —
/// module-private in each, the established per-module idiom).
const HAIRLINE_W: f32 = 1.0;

/// The left (tile-grid) pane's placeholder width. WIN7-3 replaces the whole
/// pane's content with the real uniform-tile grid (lock #6) and will very
/// likely resize it to fit; picked as a round `SP_XL` multiple (the
/// `console.rs` `RAIL_W`/`LIST_W`/`PANEL_W` token-composition idiom) rather
/// than an arbitrary literal, not a measured final value.
const LEFT_PANE_W: f32 = Style::SP_XL * 9.0;

/// The whole panel's width — the placeholder left pane plus Console's existing
/// migrated-content width (right pane, locks #4/#10).
const PANEL_W: f32 = LEFT_PANE_W + console::PANEL_W;

/// The panel's height — reuses Console's existing settled height as-is (576pt,
/// already clamped to the screen at mount and already satisfying lock #2's
/// "roughly half-height"); both panes share one height so the panel reads as
/// one unified frame, not two mismatched panels glued together.
const PANEL_H: f32 = console::PANEL_H;

// ── state ────────────────────────────────────────────────────────────────────

/// The Start Menu's cross-frame state: the open latch (driven by the Start
/// cell click and the Super key, lock #13) and the same-frame click-away guard
/// (the Console/VDOCK-4 `just_toggled` idiom, restated here since this panel
/// now owns its own outer `Area`/dismiss machinery). Pure (no egui handles),
/// so open/close is unit-tested without a GPU. Deliberately minimal for this
/// unit — WIN7-3/4 (the tile grid + rotation) and WIN7-8 (multi-seat sync) are
/// what actually grow this state; empty panes need no more than open/close.
#[derive(Debug, Default)]
pub struct StartMenuState {
    /// Whether the panel is up — toggled by a Start-cell click or a clean
    /// Super tap (lock #13); the single source of truth `main.rs` mirrors into
    /// [`ConsoleState`] each frame ([`console::ConsoleState::set_open`]).
    open: bool,
    /// Set on any edge (open or close) and cleared at the end of the panel's
    /// own frame — the same-frame click-away guard: the very click/key that
    /// opened the panel must not immediately read as a click-away dismissal
    /// (the Console/VDOCK-4 `just_toggled` idiom).
    just_toggled: bool,
}

impl StartMenuState {
    /// Whether the panel is up.
    pub(crate) const fn is_open(&self) -> bool {
        self.open
    }

    /// Toggle the panel open/closed — the Start-cell click and the Super-tap
    /// hotkey path both drain into this (lock #13, "both, not either/or").
    pub(crate) fn toggle(&mut self) {
        self.open = !self.open;
        self.just_toggled = true;
    }

    /// Close the panel (Esc / click-away / an embedded Console action closing
    /// itself). A no-op while already closed, so a redundant close (e.g. Esc
    /// racing the embedded content's own Esc-close, see the module doc) never
    /// re-arms the click-away guard for no reason.
    fn close(&mut self) {
        if self.open {
            self.open = false;
            self.just_toggled = true;
        }
    }
}

// ── render ───────────────────────────────────────────────────────────────────

/// Mount the Start Menu for this frame: a fixed-size two-pane panel (lock #2)
/// sliding up from the bottom edge, anchored beside the vertical dock column
/// (lock #4's bottom-left footprint, the Console front door's former spot).
/// Fully hidden + settled it mounts **no layer at all** (the dock/console
/// passthrough guarantee), so a closed Start Menu steals no input from the
/// surface beneath — and even open, it only claims its own footprint (it
/// overlays, never hides/replaces the active surface behind it). Esc, a click
/// away, and a second trigger all dismiss; so does an embedded Console action
/// that fires for real (a routed link, a spawned tab, a power verb — see the
/// module doc's self-closure note).
#[allow(clippy::suboptimal_flops)] // the slide offset reads clearer than mul_add
pub fn start_menu_panel(
    ctx: &egui::Context,
    state: &mut StartMenuState,
    console: &mut ConsoleState,
) {
    let t = Motion::animate(ctx, SLIDE_KEY, state.open, Motion::BASE);
    if t <= 0.001 {
        state.just_toggled = false;
        return;
    }

    // Mirror the ONE source of truth into Console before it renders (the
    // `DockState::set_active` idiom) so its focus ring / `handle_keys` read a
    // meaningful "am I showing" bit even though it no longer self-toggles.
    console.set_open(state.open);

    let screen = ctx.screen_rect();
    let panel_h = PANEL_H.min(screen.height() - Style::SP_XL);
    // The slide-up: the panel's top rides from the screen bottom (t=0) to its
    // settled height (t=1) — the console.rs precedent, restated here since the
    // Area now lives in this module.
    let top = screen.bottom() - t * panel_h;

    let area = egui::Area::new(egui::Id::new(START_MENU_AREA))
        .order(egui::Order::Foreground)
        .fade_in(false)
        .constrain(false)
        .fixed_pos(egui::pos2(DOCK_W, top))
        .show(ctx, |ui| {
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(PANEL_W, panel_h), egui::Sense::hover());
            paint_frame(ui, rect);
            let left_rect =
                egui::Rect::from_min_size(rect.min, egui::vec2(LEFT_PANE_W, rect.height()));
            let right_rect = egui::Rect::from_min_max(
                egui::pos2(rect.left() + LEFT_PANE_W, rect.top()),
                rect.max,
            );
            install_accessibility(ui.ctx(), rect, left_rect, right_rect);
            if state.open && esc_pressed(ui) {
                state.close();
            }
            left_pane(ui, left_rect);
            console::console_content(ui, right_rect, console);
        });

    // An embedded Console action fired for real this frame (a routed link, a
    // spawned tab, a power verb) and already called `ConsoleState::close`
    // (unchanged console.rs behaviour) — propagate that self-closure to the
    // WHOLE panel so launching anything still closes the menu, matching the
    // pre-WIN7-2 behaviour (the module doc's self-closure note).
    if state.open && !console.is_open() {
        state.close();
    }

    // Click-away dismissal — but never on the very frame the trigger opened it
    // (that click/key lands outside the panel and must not self-dismiss; the
    // Console/VDOCK-4 `just_toggled` guard).
    if state.open && !state.just_toggled && area.response.clicked_elsewhere() {
        state.close();
    }
    state.just_toggled = false;

    // Keep frames flowing while the slide is in flight (the dock/console tween
    // idiom).
    if t > 0.001 && t < 0.999 {
        ctx.request_repaint();
    }
}

/// The panel's outer chrome: the solid SURFACE sheet, the outer hairline, and
/// the left|right pane divider (§4 tokens) — the frame `console.rs`'s old
/// standalone panel used to paint for itself; this module owns it now since
/// it's the outer panel, and the embedded [`console::console_content`] paints
/// only its OWN inner rail|list divider (no doubled-up border).
fn paint_frame(ui: &egui::Ui, rect: egui::Rect) {
    let painter = ui.painter().clone();
    painter.rect_filled(rect, Style::RADIUS, Style::SURFACE);
    painter.rect_stroke(
        rect,
        Style::RADIUS,
        egui::Stroke::new(HAIRLINE_W, Style::BORDER),
        egui::StrokeKind::Inside,
    );
    painter.vline(
        rect.left() + LEFT_PANE_W,
        (rect.top() + Style::SP_XS)..=(rect.bottom() - Style::SP_XS),
        egui::Stroke::new(HAIRLINE_W, Style::BORDER),
    );
}

/// The left pane — WIN7-2 ships the shell only, so this is a labelled
/// placeholder (per the unit's own "empty panes to start" scope); WIN7-3
/// replaces it with the real grouped, uniform-size live-tile grid (locks
/// #6/#7/#8).
fn left_pane(ui: &egui::Ui, rect: egui::Rect) {
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "Tiles \u{2014} coming in WIN7-3",
        egui::FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );
}

/// Whether Esc should dismiss the WHOLE Start Menu this frame — inert while a
/// text field owns the keyboard (the embedded Custom-entry form's draft
/// fields can hold Esc-to-cancel focus one day; the same gate
/// `console::handle_keys` already applies to its own Esc, restated here so
/// typing in the embedded form can't also collapse the outer panel).
fn esc_pressed(ui: &egui::Ui) -> bool {
    if ui.ctx().memory(|m| m.focused().is_some()) {
        return false;
    }
    ui.input(|i| i.key_pressed(egui::Key::Escape))
}

// ── accesskit (lock #14) ─────────────────────────────────────────────────────

/// Convert an egui rect to an accesskit one (the `status.rs` helper, restated
/// module-locally — each panel's accesskit section owns its own copy).
fn accesskit_rect(rect: egui::Rect) -> egui::accesskit::Rect {
    egui::accesskit::Rect {
        x0: rect.min.x.into(),
        y0: rect.min.y.into(),
        x1: rect.max.x.into(),
        y1: rect.max.y.into(),
    }
}

/// The stable accesskit node id for the whole panel (`Role::Menu`).
fn start_menu_accesskit_id() -> egui::Id {
    egui::Id::new("start-menu-accesskit")
}

/// The stable accesskit node id for the left (tile-grid) pane landmark.
fn tiles_pane_accesskit_id() -> egui::Id {
    egui::Id::new("start-menu-tiles-pane-accesskit")
}

/// The stable accesskit node id for the right (embedded Console) pane
/// landmark.
fn console_pane_accesskit_id() -> egui::Id {
    egui::Id::new("start-menu-console-pane-accesskit")
}

/// Install the panel-level accesskit tree (lock #14 — "the panel itself needs
/// proper roles/labels even before its content is filled in"): the whole
/// panel as a `Menu`, and each pane as a landmark `Group`, so a screen reader
/// can already navigate the shell before WIN7-3/5/7 land the tiles' / Console's
/// own per-row accesskit (the `status.rs` `install_status_accessibility`
/// idiom, restated here since this crate's dock/console panels have none yet).
fn install_accessibility(
    ctx: &egui::Context,
    rect: egui::Rect,
    left: egui::Rect,
    right: egui::Rect,
) {
    let _ = ctx.accesskit_node_builder(start_menu_accesskit_id(), |node| {
        node.set_role(egui::accesskit::Role::Menu);
        node.set_label("Start Menu");
        node.set_bounds(accesskit_rect(rect));
    });
    let _ = ctx.accesskit_node_builder(tiles_pane_accesskit_id(), |node| {
        node.set_role(egui::accesskit::Role::Group);
        node.set_label("Start Menu tiles");
        node.set_bounds(accesskit_rect(left));
    });
    let _ = ctx.accesskit_node_builder(console_pane_accesskit_id(), |node| {
        node.set_role(egui::accesskit::Role::Group);
        node.set_label("Console");
        node.set_bounds(accesskit_rect(right));
    });
}

#[cfg(test)]
mod tests {
    use super::{start_menu_panel, StartMenuState, DOCK_W, PANEL_H, PANEL_W};
    use crate::console::{self, ConsoleState};
    use mde_egui::egui;
    use mde_egui::Style;

    const SZ: egui::Vec2 = egui::Vec2::new(1280.0, 800.0);

    /// Drive ONE headless frame of the Start Menu over a stand-in surface (the
    /// dock/console tests' `drive_vdock`/`drive` idiom — the same
    /// `Context::run` path the DRM runner drives, minus the GPU).
    fn drive(
        ctx: &egui::Context,
        state: &mut StartMenuState,
        console: &mut ConsoleState,
        events: Vec<egui::Event>,
        size: egui::Vec2,
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), size)),
            events,
            ..Default::default()
        };
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = ui.button("surface");
            });
            start_menu_panel(ctx, state, console);
        })
    }

    /// Drive `frames` quiet headless frames on the dock tests' 1280x800 size.
    fn run(
        ctx: &egui::Context,
        state: &mut StartMenuState,
        console: &mut ConsoleState,
        frames: usize,
    ) {
        for _ in 0..frames {
            drive(ctx, state, console, Vec::new(), SZ);
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

    /// Click `center` — press one frame, release the next (the dock/console
    /// tests' click model). The caller primes the layout first.
    fn click(
        ctx: &egui::Context,
        state: &mut StartMenuState,
        console: &mut ConsoleState,
        center: egui::Pos2,
        size: egui::Vec2,
    ) {
        drive(
            ctx,
            state,
            console,
            vec![egui::Event::PointerMoved(center), press_at(center)],
            size,
        );
        drive(ctx, state, console, vec![release_at(center)], size);
    }

    /// The Start Menu's floating-Area `LayerId`.
    fn start_menu_layer() -> egui::LayerId {
        egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new(super::START_MENU_AREA),
        )
    }

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

    // ── open/close (lock #13: Super tap + Start-cell click, both) ───────────

    #[test]
    fn the_start_menu_toggle_opens_and_a_second_toggle_closes_it() {
        // The pure state contract both trigger paths (the Start-cell click,
        // drained in `main.rs`'s `mount_start_menu`; a clean Super tap, drained
        // in the hotkey dispatch block alongside VDOCK-1's own dock toggle —
        // see the module doc) fold into: a toggle opens, a second toggle
        // (either trigger) closes (lock #13's "both, not either/or").
        let mut s = StartMenuState::default();
        assert!(!s.is_open(), "closed by default");
        s.toggle();
        assert!(s.is_open(), "a toggle opens the Start Menu");
        s.toggle();
        assert!(!s.is_open(), "a second toggle closes it");
    }

    // ── geometry (lock #2: fixed-size, bottom-left-anchored, not full-screen) ─

    #[test]
    fn a_closed_start_menu_mounts_no_layer_and_an_open_one_claims_its_fixed_bottom_left_footprint()
    {
        // Lock #2 — fixed-size (never full-screen, never resizable): the
        // constants themselves ARE the whole footprint (the
        // `the_vertical_dock_is_a_48px_full_height_column` precedent — assert
        // directly on the compile-time geometry, no runtime resize path
        // exists to test against).
        assert!(
            (PANEL_H - 576.0).abs() < f32::EPSILON,
            "the panel reuses Console's existing settled height"
        );
        assert!(
            PANEL_W < SZ.x,
            "narrower than a real screen — never full width"
        );
        assert!(
            PANEL_H < SZ.y,
            "shorter than a real screen — never full height"
        );

        // Closed + settled -> no layer at all (the dock/console passthrough
        // guarantee): input over the panel's would-be footprint reaches the
        // surface beneath.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        run(&ctx, &mut s, &mut console, 2);
        let inside = egui::pos2(DOCK_W + 10.0, SZ.y - 10.0);
        assert_ne!(
            ctx.layer_id_at(inside),
            Some(start_menu_layer()),
            "a CLOSED Start Menu must not float an intercepting layer"
        );

        // Open on a fresh context (the slide latch settles at the open
        // endpoint on first sight, the console.rs precedent) -> claims exactly
        // its bottom-left footprint, anchored beside the dock column.
        let ctx2 = egui::Context::default();
        Style::install(&ctx2);
        let mut s2 = StartMenuState::default();
        let mut console2 = ConsoleState::with_store(None);
        s2.toggle();
        run(&ctx2, &mut s2, &mut console2, 1);
        assert_eq!(
            ctx2.layer_id_at(inside),
            Some(start_menu_layer()),
            "an OPEN Start Menu claims its bottom-left footprint"
        );
    }

    #[test]
    fn the_open_start_menu_does_not_cover_the_rest_of_the_screen() {
        // Lock #2 (not full-screen) + the design's "it overlays, never hides
        // the active surface behind it": the top-right corner (far from the
        // bottom-left footprint) and the strip LEFT of the dock column (the
        // panel sits BESIDE the dock, never under/over it) both stay
        // unclaimed while the Start Menu is open.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 1);

        let top_right = egui::pos2(SZ.x - 10.0, 10.0);
        assert_ne!(
            ctx.layer_id_at(top_right),
            Some(start_menu_layer()),
            "the Start Menu must not blanket the whole screen"
        );
        let left_of_dock = egui::pos2(DOCK_W - 10.0, SZ.y - 10.0);
        assert_ne!(
            ctx.layer_id_at(left_of_dock),
            Some(start_menu_layer()),
            "the panel is anchored beside the dock column, not under it"
        );
    }

    // ── dismiss (Esc / click-away / an embedded action closing for real) ────

    #[test]
    fn esc_and_click_away_close_the_start_menu_but_never_on_the_opening_frame() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 1);
        assert!(s.is_open());
        drive(&ctx, &mut s, &mut console, vec![key(egui::Key::Escape)], SZ);
        assert!(!s.is_open(), "Esc dismisses the Start Menu");

        let ctx2 = egui::Context::default();
        Style::install(&ctx2);
        let mut s2 = StartMenuState::default();
        let mut console2 = ConsoleState::with_store(None);
        s2.toggle();
        // The very frame the trigger opened it: its click lands outside the
        // panel — the guard must swallow it (the power-menu / Console
        // `just_toggled` idiom).
        let far = egui::pos2(SZ.x - 40.0, 40.0);
        drive(
            &ctx2,
            &mut s2,
            &mut console2,
            vec![egui::Event::PointerMoved(far), release_at(far)],
            SZ,
        );
        assert!(s2.is_open(), "the opening click must not self-dismiss");
        run(&ctx2, &mut s2, &mut console2, 1); // settle
        click(&ctx2, &mut s2, &mut console2, far, SZ);
        assert!(!s2.is_open(), "a click away dismisses the Start Menu");
    }

    #[test]
    fn activating_an_embedded_console_row_closes_the_whole_start_menu() {
        // Proves the WIN7-2 embedding actually works end-to-end, not just
        // architecturally: a click on the embedded Console's pinned Terminal
        // row (the right pane) fires the SAME `ConsoleRequest` it always has,
        // AND closes the WHOLE Start Menu — the self-closure propagation the
        // module doc describes — not just Console's own mirrored `open` bit.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 2);
        let row = ctx
            .read_response(console::console_entry_id(0))
            .expect("the embedded pinned Terminal row is registered")
            .rect;
        click(&ctx, &mut s, &mut console, row.center(), SZ);
        assert_eq!(
            console.take_request(),
            Some(console::ConsoleRequest::Goto(
                crate::dock::Surface::Terminal
            )),
            "the embedded row still fires the real Console request"
        );
        assert!(!console.is_open(), "Console's own close() fired, unchanged");
        assert!(
            !s.is_open(),
            "...and the Start Menu propagated that self-closure to the WHOLE panel"
        );
    }

    // ── accesskit (lock #14) ─────────────────────────────────────────────────

    #[test]
    fn the_panel_exports_a_menu_role_and_labelled_panes_before_any_content_lands() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        let out = drive(&ctx, &mut s, &mut console, Vec::new(), SZ);
        let nodes = accesskit_nodes(&out);

        let menu = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some("Start Menu"))
            .expect("Start Menu node");
        assert_eq!(menu.role(), egui::accesskit::Role::Menu);

        let tiles = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some("Start Menu tiles"))
            .expect("tiles pane node");
        assert_eq!(tiles.role(), egui::accesskit::Role::Group);

        let console_pane = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some("Console"))
            .expect("console pane node");
        assert_eq!(console_pane.role(), egui::accesskit::Role::Group);
    }
}
