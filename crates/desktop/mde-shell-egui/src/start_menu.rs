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
//!
//! **WIN7-3 update:** the left pane described above as an empty placeholder is
//! now the real live-tile grid (locks #6/#7/#8/#23): all 17 [`Surface::ALL`]
//! entries, grouped into lock #8's 7 function-based groups (Mesh Control ·
//! Desktop & Session · Media · Files & Data · Web & Tools · Comms · System —
//! [`TILE_GROUPS`]), each a uniform [`TILE_W`]×[`TILE_H`] tile (lock #6 — one
//! size, no variants). A tile wears the SAME glyph the app picker already
//! draws (`Surface::icon_id`) plus a NEW text label (`Surface::label`,
//! added this unit): the picker itself deliberately carries no per-icon
//! captions (`dock.rs`'s own PICKER-1 lock), so there was no existing label
//! table to inherit, only the icon one. A click reuses the picker's own
//! click-vs-Enter/Space activation predicate (`dock::response_activated`,
//! widened to `pub(crate)` for this reuse, not reimplemented) and records
//! the surface in a new [`StartMenuState::tile_activation`] slot, drained by
//! `main.rs` exactly like an embedded Console `Goto` request — both panes
//! end in the same "go to this surface, close the whole menu" outcome (lock
//! #23), just raised from different data. [`LEFT_PANE_W`] is no longer
//! WIN7-2's arbitrary 288pt placeholder; it is now sized to the real grid
//! this unit renders. Static content only (lock #5's live-fact rotation is
//! WIN7-4's job) — this unit leaves a [`tile_status_tint`] seam WIN7-4 can
//! light up rather than hardcoding "never any live data."

use mde_egui::egui;
use mde_egui::{Motion, Style};

use crate::console::{self, ConsoleState};
use crate::dock::{icon_texture, response_activated, Surface, DOCK_W};

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

// ── tile grid geometry (WIN7-3, locks #6/#7/#8) ─────────────────────────────

/// One tile's height — `SP_XL + SP_M` (48pt), the SAME cell-height
/// composition `dock.rs`'s own (module-private) `CELL_W` icon-cell token
/// already uses, restated here per this module's own established
/// per-file-restatement idiom (see [`HAIRLINE_W`] above). Every one of the
/// 17 tiles shares this ONE size (lock #6 — no small/wide/large variants).
const TILE_H: f32 = Style::SP_XL + Style::SP_M;

/// One tile's width — `SP_XL · 2.5` (80pt): wider than tall, so a full
/// surface label (e.g. "Infra as Code") has real room beside the shorter
/// ones. Every tile still shares this ONE width (lock #6 rules out
/// per-tile small/wide/large *variants*, not a non-square aspect ratio).
const TILE_W: f32 = Style::SP_XL * 2.5;

/// The gap between adjacent tiles, in both directions of the grid.
const TILE_GAP: f32 = Style::SP_XS;

/// How many tiles sit in one row before wrapping. The widest of lock #8's 7
/// groups (Mesh Control / Media / Files & Data / Web & Tools) has exactly 3
/// members, so every one of today's groups renders as a single tidy row —
/// pinned by a test below rather than just assumed. [`left_pane`]'s render
/// loop still wraps generally (N rows, not hardcoded to 1 — `usize::div_ceil`)
/// so a group that later grows past 3 members degrades to a second row
/// instead of silently overlapping.
const TILE_COLUMNS: usize = 3;

/// A tile-group heading's height — matches `console.rs`'s own
/// (module-private) `HEADING_H` exactly (`SP_L`), so the two panes' section
/// labels read as one visual rhythm (Console's heading sits right next to
/// this pane in the same panel).
const GROUP_HEADING_H: f32 = Style::SP_L;

/// The gap after one group's tile row(s), before the next group's heading.
const GROUP_GAP: f32 = Style::SP_XS;

/// The pane's inner padding on every edge — matches `console.rs`'s own
/// `list_pane` `SP_S` inset idiom.
const PANE_PAD: f32 = Style::SP_S;

/// The tile's icon glyph size — the SAME 24px [`Style::SP_L`] the app
/// picker's own cells already draw their glyphs at (one icon language, not a
/// second size invented here).
const TILE_ICON: f32 = Style::SP_L;

/// The status-tint dot's radius ([`tile_status_tint`]'s seam, WIN7-4).
const TILE_STATUS_DOT_R: f32 = Style::SP_XS / 2.0;

/// The left (tile-grid) pane's width: [`PANE_PAD`] on both sides plus three
/// (see [`TILE_COLUMNS`]) [`TILE_W`]-wide columns, [`TILE_GAP`] apart.
/// WIN7-2 shipped this pane at an arbitrary 288pt placeholder ("WIN7-3 will
/// very likely resize it" — its own doc comment); this is that resize,
/// derived from the real grid this unit renders rather than picked by eye.
/// A test below pins `TILE_COLUMNS == 3` so the `3.0`/`2.0` literals here
/// can't silently drift from the grid they're meant to fit.
const LEFT_PANE_W: f32 = PANE_PAD * 2.0 + TILE_W * 3.0 + TILE_GAP * 2.0;

/// The whole panel's width — the left tile-grid pane plus Console's existing
/// migrated-content width (right pane, locks #4/#10).
const PANEL_W: f32 = LEFT_PANE_W + console::PANEL_W;

/// The panel's height — reuses Console's existing settled height as-is (576pt,
/// already clamped to the screen at mount and already satisfying lock #2's
/// "roughly half-height"); both panes share one height so the panel reads as
/// one unified frame, not two mismatched panels glued together.
const PANEL_H: f32 = console::PANEL_H;

/// The tile grid's total content height: 7 group headings + 7 single tile
/// rows (see [`TILE_COLUMNS`]'s note — every locked group fits one row
/// today) + 6 inter-group gaps. Comfortably inside [`PANEL_H`] minus its own
/// top/bottom [`PANE_PAD`] inset — pinned by a test below rather than
/// trusted by eye. `#[cfg(test)]`: nothing in the render path reads a
/// pre-summed total (`left_pane` accumulates `y` incrementally instead), so
/// this is verification-only data (the `status.rs` `local_grade_pip_id`
/// `#[cfg(test)]`-on-a-top-level-item idiom), not dead weight in a release
/// build.
#[cfg(test)]
const TILE_GRID_CONTENT_H: f32 = 7.0 * (GROUP_HEADING_H + TILE_H) + 6.0 * GROUP_GAP;

// ── tile groups (lock #8: function-based grouping) ──────────────────────────

/// One labelled group of the left pane's tile grid (lock #8). Mirrors
/// `dock.rs`'s `Group` / `console.rs`'s `ConsoleGroup` shape — this module's
/// own copy since the Start Menu's tile grouping is its own domain concern
/// (lock #8), distinct from the app picker's PICKER-1 grouping.
struct TileGroup {
    /// The group heading, painted by [`tile_group_heading`] (visually
    /// matching `console.rs`'s own `heading()` row, per this unit's steer to
    /// match Console's precedent since it sits right next to this pane).
    label: &'static str,
    /// The group's surfaces, kept in [`Surface::ALL`] relative order (the
    /// `dock.rs` `Group::surfaces` L7 convention) — lock #8's own listed
    /// order already satisfies this, checked by a test below.
    surfaces: &'static [Surface],
}

/// The 7 function-based groups in their locked order (lock #8), each listing
/// its surfaces in [`Surface::ALL`] relative order. Unlike the app picker's
/// `GROUPS` (which pulls the Workbench/System/Desktop out to standalone
/// cells), every one of the 17 [`Surface::ALL`] entries sits inside exactly
/// one group here — lock #8 places all 17, none outside. Drives the tile
/// render + the shell tests (the one grouping authority for this pane).
const TILE_GROUPS: [TileGroup; 7] = [
    TileGroup {
        label: "Mesh Control",
        surfaces: &[Surface::Workbench, Surface::MeshView, Surface::InfraCode],
    },
    TileGroup {
        label: "Desktop & Session",
        surfaces: &[Surface::Desktop],
    },
    TileGroup {
        label: "Media",
        surfaces: &[Surface::Music, Surface::Media, Surface::Voice],
    },
    TileGroup {
        label: "Files & Data",
        surfaces: &[Surface::Files, Surface::Bookmarks, Surface::Storage],
    },
    TileGroup {
        label: "Web & Tools",
        surfaces: &[Surface::Browser, Surface::Terminal, Surface::Editor],
    },
    TileGroup {
        label: "Comms",
        surfaces: &[Surface::Chat, Surface::Phones],
    },
    TileGroup {
        label: "System",
        surfaces: &[Surface::System, Surface::About],
    },
];

// Compile-time guard: every `Surface::ALL` entry appears in `TILE_GROUPS`
// exactly once (the `dock.rs` `GROUPS` completeness-guard idiom, restated
// here since this table is its own domain concern — lock #8's grouping, not
// `dock.rs`'s picker grouping) — so a future `Surface` addition that forgets
// to place a tile fails the BUILD, not a silent missing/duplicate tile.
const _: () = {
    let mut i = 0;
    while i < Surface::ALL.len() {
        let target = Surface::ALL[i] as usize;
        let mut count = 0;
        let mut g = 0;
        while g < TILE_GROUPS.len() {
            let surfaces = TILE_GROUPS[g].surfaces;
            let mut s = 0;
            while s < surfaces.len() {
                if surfaces[s] as usize == target {
                    count += 1;
                }
                s += 1;
            }
            g += 1;
        }
        assert!(
            count == 1,
            "every Surface::ALL entry must appear in TILE_GROUPS exactly once (lock #8)",
        );
        i += 1;
    }
};

// ── state ────────────────────────────────────────────────────────────────────

/// The Start Menu's cross-frame state: the open latch (driven by the Start
/// cell click and the Super key, lock #13), the same-frame click-away guard
/// (the Console/VDOCK-4 `just_toggled` idiom, restated here since this panel
/// now owns its own outer `Area`/dismiss machinery), and (WIN7-3) a pending
/// tile-click surface activation. Pure (no egui handles), so open/close and
/// tile activation are unit-tested without a GPU. WIN7-4 (tile rotation) and
/// WIN7-8 (multi-seat sync) are what grow this further.
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
    /// WIN7-3 — a live-tile click's pending surface activation (lock #23):
    /// set by [`left_pane`]/[`tile`] when a tile fires, drained once by
    /// `main.rs` ([`Self::take_tile_activation`]) exactly like
    /// [`console::ConsoleState::take_request`]'s `Goto` variant — the SAME
    /// "go to this surface, close the menu" outcome as an embedded Console
    /// row, just raised from the OTHER pane's data.
    tile_activation: Option<Surface>,
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

    /// Drain a pending tile-click surface activation (WIN7-3, lock #23) —
    /// `main.rs` calls this each frame after [`start_menu_panel`] and routes
    /// `nav.surface` exactly as it already does for an embedded Console
    /// `Goto` request (the SAME deferred-wire idiom, §6 — this panel can't
    /// reach the shell nav itself). `None` (drained once) otherwise. `const`
    /// matching [`console::ConsoleState::take_request`]'s identical
    /// `self.pending.take()` shape.
    pub(crate) const fn take_tile_activation(&mut self) -> Option<Surface> {
        self.tile_activation.take()
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
/// module doc's self-closure note), and so (WIN7-3) does a live-tile click
/// (lock #23) — both panes close the WHOLE menu on activation, just via
/// different data ([`ConsoleState::is_open`]'s self-closure vs.
/// [`StartMenuState::tile_activation`]).
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
            // WIN7-3 — a tile click records its surface and closes the WHOLE
            // panel at once (lock #23), the same "activation closes the
            // menu" outcome the embedded Console pane's self-closure below
            // already gives its own rows.
            if let Some(surface) = left_pane(ui, left_rect) {
                state.tile_activation = Some(surface);
                state.close();
            }
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

/// The left pane (WIN7-3, locks #6/#7/#8): [`TILE_GROUPS`]' 7 headed
/// sections, each a row of uniform [`TILE_W`]×[`TILE_H`] tiles in
/// [`Surface::ALL`] order. Returns the clicked tile's surface, if any, for
/// the caller to route + close the whole panel with (lock #23 — a single
/// click activates, mirroring the embedded Console pane's own
/// click-routes-and-closes behaviour). Static content only: WIN7-4 is what
/// makes a tile's face rotate through a few live facts (lock #5); this unit
/// leaves that as one pluggable seam ([`tile_status_tint`]) rather than
/// hardcoding "never any live data."
#[allow(
    clippy::cast_precision_loss, // row/col indices are tiny (< TILE_COLUMNS)
    clippy::suboptimal_flops     // layout arithmetic reads clearer than mul_add
)]
fn left_pane(ui: &egui::Ui, rect: egui::Rect) -> Option<Surface> {
    let mut activated = None;
    let x0 = rect.left() + PANE_PAD;
    let mut y = rect.top() + PANE_PAD;
    for group in &TILE_GROUPS {
        let heading_rect = egui::Rect::from_min_size(
            egui::pos2(x0, y),
            egui::vec2((rect.width() - PANE_PAD * 2.0).max(0.0), GROUP_HEADING_H),
        );
        tile_group_heading(ui, heading_rect, group.label);
        y += GROUP_HEADING_H;

        for (i, &surface) in group.surfaces.iter().enumerate() {
            let col = (i % TILE_COLUMNS) as f32;
            let row = (i / TILE_COLUMNS) as f32;
            let tile_rect = egui::Rect::from_min_size(
                egui::pos2(
                    x0 + col * (TILE_W + TILE_GAP),
                    y + row * (TILE_H + TILE_GAP),
                ),
                egui::vec2(TILE_W, TILE_H),
            );
            if tile(ui, surface, tile_rect) {
                activated = Some(surface);
            }
        }
        let rows = group.surfaces.len().div_ceil(TILE_COLUMNS).max(1);
        y += rows as f32 * (TILE_H + TILE_GAP) - TILE_GAP + GROUP_GAP;
    }
    activated
}

/// One tile-group heading — visually matches `console.rs`'s own
/// (module-private) `heading()` row exactly (same uppercased micro-label,
/// same `SMALL`/`TEXT_DIM` treatment, same `SP_XS` left inset), restated
/// here since it's private to that module and this pane paints via explicit
/// rects rather than `console.rs`'s layout-managed `ui.allocate_exact_size`
/// (this module's own established direct-painter style, e.g. [`paint_frame`]).
fn tile_group_heading(ui: &egui::Ui, rect: egui::Rect, label: &str) {
    ui.painter().text(
        egui::pos2(rect.left() + Style::SP_XS, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label.to_uppercase(),
        egui::FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );
}

/// One live tile (WIN7-3, locks #6/#8/#23): a uniform [`TILE_W`]×[`TILE_H`]
/// cell wearing the surface's existing picker glyph (`Surface::icon_id`, the
/// SAME [`icon_texture`] loader + the SAME 24px [`TILE_ICON`] size the app
/// picker's own cells use) over its new tile label (`Surface::label`). A
/// hover brightens both the fill and the tint — the same two-tone contract
/// the app picker's own cells already use (§4, one hover language, not a
/// second one invented here). A click (or Enter/Space while focused —
/// [`response_activated`], reused verbatim rather than reimplemented)
/// returns `true` so [`left_pane`] can route + close the whole panel (lock
/// #23). Exports its own accesskit `Button` node (lock #14, WIN7-3's own
/// per-tile parity, not WIN7-7's later full sweep) —
/// [`install_tile_accessibility`].
fn tile(ui: &egui::Ui, surface: Surface, rect: egui::Rect) -> bool {
    let resp = ui.interact(rect, tile_id(surface), egui::Sense::click());
    let hovered = resp.hovered();
    let painter = ui.painter().clone();

    if hovered {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    let tint = if hovered {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };

    if let Some(tex) = icon_texture(ui.ctx(), surface.icon_id(), TILE_ICON, tint) {
        let icon = egui::Rect::from_center_size(
            egui::pos2(rect.center().x, rect.top() + Style::SP_XS + TILE_ICON / 2.0),
            egui::vec2(TILE_ICON, TILE_ICON),
        );
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(tex.id(), icon, uv, egui::Color32::WHITE);
    }

    // The WIN7-4 seam (§7 honest-gating): a status-colour dot if this
    // surface's already-published state has one for free. Always `None`
    // today — see `tile_status_tint`'s own doc for why this unit leaves it
    // wired-but-inert rather than half-plumbing a live source.
    if let Some(color) = tile_status_tint(surface) {
        painter.circle_filled(
            egui::pos2(
                rect.right() - Style::SP_XS - TILE_STATUS_DOT_R,
                rect.top() + Style::SP_XS + TILE_STATUS_DOT_R,
            ),
            TILE_STATUS_DOT_R,
            color,
        );
    }

    // The label — bottom-centred, clipped to the tile so a long name (e.g.
    // "Infra as Code") trims cleanly at the tile edge instead of spilling
    // into its neighbour.
    painter.with_clip_rect(rect).text(
        egui::pos2(rect.center().x, rect.bottom() - Style::SP_XS),
        egui::Align2::CENTER_BOTTOM,
        surface.label(),
        egui::FontId::proportional(Style::SMALL),
        tint,
    );

    install_tile_accessibility(ui.ctx(), surface, rect);
    response_activated(ui, &resp)
}

/// The stable id of one tile's interactive rect (the `dock.rs` `pick_cell_id`
/// idiom restated — tests read a tile's settled `Rect` back to click its
/// exact centre, the addressable-cell idiom).
fn tile_id(surface: Surface) -> egui::Id {
    egui::Id::new(("start-menu-tile", surface))
}

/// The seam WIN7-4 (design lock #5's rotating live-tile content) hooks its
/// per-surface "tile fact" source into: a status-colour dot when the SAME
/// already-published state the dock's own picker badges
/// (`dock::badge_for`) read has one for this surface (§7 honest-gating —
/// never invented data). This unit ships the seam wired but inert (`None`
/// for every surface) rather than reading real state here: doing that for
/// real would mean threading `StatusInputs`/`DockState` into
/// [`start_menu_panel`]'s own signature for a single static dot, when
/// WIN7-4 has to build that live-fact plumbing "for real" anyway (the design
/// doc's own "a new thin per-surface tile fact trait/source" note) — better
/// built once, together, than half-wired twice.
const fn tile_status_tint(_surface: Surface) -> Option<egui::Color32> {
    None
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
/// can already navigate the shell before WIN7-5/7 land Console's own per-row
/// accesskit / the full sweep (the `status.rs` `install_status_accessibility`
/// idiom, restated here since this crate's dock/console panels have none
/// yet). WIN7-3 lands the tiles' own per-tile accesskit separately —
/// [`install_tile_accessibility`], called per tile from [`tile`].
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

/// The stable accesskit node id for one tile (WIN7-3, lock #14).
fn tile_accesskit_id(surface: Surface) -> egui::Id {
    egui::Id::new(("start-menu-tile-accesskit", surface))
}

/// Install one tile's own accesskit node (lock #14 — "every tile", not just
/// the panel level): a `Button` role with the surface's display label and
/// bounds, plus the `Click` action — the SAME shape `status.rs`'s
/// `install_segment_accessibility` already uses for its own per-item pips
/// (role + label + bounds + `add_action(Click)`), restated here since that
/// helper is module-private there.
fn install_tile_accessibility(ctx: &egui::Context, surface: Surface, rect: egui::Rect) {
    let _ = ctx.accesskit_node_builder(tile_accesskit_id(surface), |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(surface.label());
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
    });
}

#[cfg(test)]
mod tests {
    use super::{start_menu_panel, StartMenuState, DOCK_W, PANEL_H, PANEL_W};
    use crate::console::{self, ConsoleState};
    use crate::dock::Surface;
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

    // ── WIN7-3: live tiles (locks #6/#7/#8/#23) ──────────────────────────────

    #[test]
    fn the_17_surfaces_are_grouped_into_lock_8s_7_function_based_groups() {
        // Lock #8's exact taxonomy + order — the `dock.rs`
        // `the_locked_group_taxonomy_and_order` precedent, restated for this
        // pane's own (different) grouping table.
        use Surface::{
            About, Bookmarks, Browser, Chat, Desktop, Editor, Files, InfraCode, Media, MeshView,
            Music, Phones, Storage, System, Terminal, Voice, Workbench,
        };
        let expect: [(&str, &[Surface]); 7] = [
            ("Mesh Control", &[Workbench, MeshView, InfraCode]),
            ("Desktop & Session", &[Desktop]),
            ("Media", &[Music, Media, Voice]),
            ("Files & Data", &[Files, Bookmarks, Storage]),
            ("Web & Tools", &[Browser, Terminal, Editor]),
            ("Comms", &[Chat, Phones]),
            ("System", &[System, About]),
        ];
        assert_eq!(
            super::TILE_GROUPS.len(),
            expect.len(),
            "seven groups (lock #8)"
        );
        for (g, (label, surfaces)) in super::TILE_GROUPS.iter().zip(expect) {
            assert_eq!(g.label, label, "group order");
            assert_eq!(
                g.surfaces, surfaces,
                "{label} membership + within-group order"
            );
        }
        // Unlike the app picker (which pulls Workbench/System/Desktop out to
        // standalone cells), lock #8 places ALL 17 Surface::ALL entries
        // inside a group — none sit outside. The compile-time guard above
        // already enforces "exactly once"; re-prove it here at runtime too
        // (the dock.rs belt-and-suspenders convention).
        let mut placed: Vec<Surface> = Vec::new();
        for g in &super::TILE_GROUPS {
            placed.extend_from_slice(g.surfaces);
        }
        assert_eq!(
            placed.len(),
            Surface::ALL.len(),
            "every surface placed once"
        );
        for surface in Surface::ALL {
            assert_eq!(
                placed.iter().filter(|&&s| s == surface).count(),
                1,
                "{surface:?} must be placed in exactly one tile group"
            );
        }
    }

    #[test]
    fn the_tile_grid_content_fits_the_shared_panel_height_without_overflow() {
        // The panel is fixed-size (lock #2) and shares ONE height across
        // both panes (`PANEL_H`) — the tile grid's own derived content
        // height, plus its top/bottom `PANE_PAD` inset, must fit inside it.
        // Asserted directly on the compile-time geometry (the WIN7-2
        // `PANEL_H`/`PANEL_W` constant-assertion precedent), no
        // GPU/context needed.
        assert!(
            super::TILE_GRID_CONTENT_H + super::PANE_PAD * 2.0 <= PANEL_H,
            "the tile grid overflows the shared panel height"
        );
        // The widest locked group (3 members) is what `LEFT_PANE_W`'s
        // literal `3.0`/`2.0` and `TILE_GRID_CONTENT_H`'s "one row per
        // group" literal `7.0`/`6.0` both depend on — pin the assumption so
        // a future `TILE_GROUPS` edit that breaks it fails a test, not a
        // silently wrong layout.
        assert_eq!(super::TILE_COLUMNS, 3);
        assert!(
            super::TILE_GROUPS
                .iter()
                .all(|g| g.surfaces.len() <= super::TILE_COLUMNS),
            "a group wider than TILE_COLUMNS wraps to a second row, which \
             TILE_GRID_CONTENT_H's literal derivation does not account for"
        );
    }

    #[test]
    fn all_17_tiles_render_at_one_uniform_size_and_stay_within_the_left_pane() {
        // Lock #6 — one uniform tile size for all 17, no variants — proven
        // on REAL rendered rects (the addressable-cell idiom via
        // `tile_id`), not just on the shared constants two tiles happen to
        // both reference.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 2);

        let left_pane_right_edge = DOCK_W + super::LEFT_PANE_W;
        for surface in Surface::ALL {
            let rect = ctx
                .read_response(super::tile_id(surface))
                .unwrap_or_else(|| panic!("{surface:?} tile is not registered"))
                .rect;
            assert!(
                (rect.width() - super::TILE_W).abs() < 0.01,
                "{surface:?} tile width drifted from the uniform TILE_W"
            );
            assert!(
                (rect.height() - super::TILE_H).abs() < 0.01,
                "{surface:?} tile height drifted from the uniform TILE_H"
            );
            assert!(
                rect.right() <= left_pane_right_edge + 0.01,
                "{surface:?} tile overflows the left pane's right edge"
            );
        }
    }

    #[test]
    fn clicking_a_tile_activates_its_surface_and_closes_the_whole_start_menu() {
        // Lock #23 — a single click activates, mirroring the embedded
        // Console pane's own click-routes-and-closes contract (proven above
        // in `activating_an_embedded_console_row_closes_the_whole_start_menu`)
        // — proven here for the OTHER pane's tiles. Picks a tile that is
        // neither the default surface nor the first tile in its group, so
        // the assertion actually distinguishes "the right one" from "any
        // one."
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 2);
        let rect = ctx
            .read_response(super::tile_id(Surface::Phones))
            .expect("the Phones tile is registered")
            .rect;
        click(&ctx, &mut s, &mut console, rect.center(), SZ);
        assert_eq!(
            s.take_tile_activation(),
            Some(Surface::Phones),
            "the clicked tile's surface is recorded for main.rs to route"
        );
        assert!(
            !s.is_open(),
            "a tile click closes the whole Start Menu, matching the embedded \
             Console pane's own activation contract"
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

    #[test]
    fn every_tile_exports_a_labelled_button_role_for_accesskit() {
        // Lock #14 — "every tile", not just the panel level proven above
        // (`console.rs`'s own rows export none yet — WIN7-7's later full
        // sweep — so a tile's label is unambiguous among this frame's
        // exported nodes).
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        let out = drive(&ctx, &mut s, &mut console, Vec::new(), SZ);
        let nodes = accesskit_nodes(&out);

        for surface in Surface::ALL {
            let node = nodes
                .iter()
                .map(|(_, n)| n)
                .find(|n| n.label() == Some(surface.label()))
                .unwrap_or_else(|| panic!("{surface:?} tile exports no accesskit node"));
            assert_eq!(
                node.role(),
                egui::accesskit::Role::Button,
                "{surface:?} tile's accesskit role"
            );
        }
    }
}
