//! `springboard` — WL-UX-006/U10: the **Construct Home gesture layer** over the
//! wallpaper-backed desktop. Home is intentionally quiet; app discovery lives
//! in Spotlight/Front Door while this module owns the pull-down gesture.
//!
// PLATFORM-INTERFACES Q5/Q8/Q22 — the locked home is a wallpaper-backed free
// canvas with no desktop title or app tile labels. The bottom Dock is the
// compact launch rail; deeper app discovery remains in Spotlight.
//!
//! ## Input
//!
//! * **Pull-down → Spotlight** (Q11 "pull-down on home grid"): a downward
//!   drag beginning in the grid's upper region past a threshold queues the
//!   distinct [`SpringboardAction::Spotlight`]; the slot body lands it on the
//!   shell's existing Front Door toggle — never a second search path.
//!
//! Chrome overlays mounted above (Front Door, switcher, the centers) own the
//! keyboard while open — the collapsed view passes `overlay_above` and this
//! module consumes nothing then, nor while any widget holds real egui focus
//! (the omnibox/front-door text field).
//!
//! ## The mount seam
//!
//! `main.rs` calls exactly two functions: [`show`] from the collapsed central
//! view (the gesture layer) and [`mount`] from the U09 slot, which drains
//! the interactions [`show`] queued — plus the `ChromeIntent::Home` this slot
//! remains the ONE consumer of — into a single typed [`SpringboardAction`]
//! for the slot body to apply. State rides egui memory (the switcher/backdrop
//! pattern), so `main.rs` grows no new fields.

use mde_egui::{egui, Motion, MotionPreset, Style, TypographyRole};

use crate::construct::{ChromeIntent, ConstructChrome};
use crate::surfaces::{icon_texture, Surface, LAUNCHER_GROUPS};

/// Stable egui-memory key the per-frame [`SpringboardState`] persists under.
const STATE_KEY: &str = "construct-springboard-state";
/// Stable id of the whole-grid drag/gesture target (tiles interact above it).
const SPRINGBOARD_BG: &str = "construct-springboard-bg";

/// Edge of one tile plate in logical points (`SP_XL·2 + SP_L` on the 8px
/// grid = 88px — the Q22 rounded-rect accent plate).
const PLATE_EDGE: f32 = Style::SP_XL * 2.0 + Style::SP_L;
/// Edge of the white surface glyph centered on the plate (`SP_XL + SP_M`).
const GLYPH_EDGE: f32 = Style::SP_XL + Style::SP_M;
/// Height of the label band beneath the plate.
const LABEL_BAND: f32 = Style::SP_L;
/// Width of one grid cell (plate + side breathing room).
const CELL_W: f32 = PLATE_EDGE + Style::SP_M * 2.0;
/// Height of one grid cell (plate + label band).
const CELL_H: f32 = PLATE_EDGE + LABEL_BAND;
/// Gap between grid cells.
const GRID_GAP: f32 = Style::SP_M;
/// Outer margin the grid keeps from the desktop edges.
const GRID_MARGIN: f32 = Style::SP_XL;
/// Pointer travel (either axis) past which an undecided drag classifies as
/// a gesture (dominant-horizontal) or the Spotlight pull (dominant-vertical).
const DRAG_SLOP: f32 = 8.0;
/// The upper fraction of the grid a Spotlight pull must begin in (Q11's
/// "pull-down on home" — a downward drag from the top region, never a stray
/// scroll near the desktop edge).
const PULL_REGION: f32 = 0.4;
/// Downward travel past which an armed pull fires Spotlight, once.
const PULL_FIRE: f32 = 56.0;
/// How far past the tile the open-presence ghost swells (Q24 zoom-from-tile,
/// scoped to this module — the U29 seam below carries the full zoom).
const ZOOM_GHOST_SCALE: f32 = 1.6;

/// One routed springboard outcome for the U09 slot body to apply — the whole
/// `main.rs` contract of this unit (a small match, nothing else).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpringboardAction {
    /// A tile was chosen (click/Enter): route `nav` to this surface, expanded.
    Open(Surface),
    /// The §2.3 Home intent fired: collapse to the base layer.
    Home,
    /// The Q11 on-home pull-down fired: toggle the Spotlight (Front Door).
    Spotlight,
}

/// The tiles of the single desktop in canonical surface order.
#[must_use]
const fn desktop_tiles() -> &'static [Surface] {
    &Surface::ALL
}

/// The launcher-group accent for `surface` off the ONE shared taxonomy table
/// (the switcher's fold — the dock's own `launcher_group_accent` helper is
/// `#[cfg(test)]`-gated, so the production lookup folds the table here).
fn group_accent(surface: Surface) -> Option<egui::Color32> {
    LAUNCHER_GROUPS
        .iter()
        .find(|group| group.surfaces.contains(&surface))
        .map(|group| group.accent)
}

/// An in-flight pointer gesture on the grid, classified by dominant axis once
/// travel clears [`DRAG_SLOP`].
#[derive(Debug, Clone, Copy, PartialEq)]
enum Gesture {
    /// Travel under the slop — not yet classified.
    Undecided {
        /// Where the drag began, as a fraction of the grid height.
        origin_frac_y: f32,
        /// Accumulated horizontal travel in points.
        dx: f32,
        /// Accumulated vertical travel in points.
        dy: f32,
    },
    /// A horizontal drag is intentionally inert: the desktop has no pages.
    Ignored,
    /// A vertical pull — fires Spotlight once when armed (upper-region origin)
    /// and past [`PULL_FIRE`]; an unarmed/upward pull is inert, honestly.
    Pull {
        /// Where the drag began, as a fraction of the grid height.
        origin_frac_y: f32,
        /// Accumulated (downward-positive) vertical travel in points.
        dy: f32,
        /// Spotlight already fired for this gesture.
        fired: bool,
    },
}

/// The tile-plate rect within one grid cell (the label band sits beneath).
#[must_use]
fn plate_rect(cell: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(cell.center().x - PLATE_EDGE / 2.0, cell.top()),
        egui::vec2(PLATE_EDGE, PLATE_EDGE),
    )
}

/// The centered cell grid for `n` tiles on `desktop`: the column count and one
/// cell rect per tile, row-major. Columns are whatever the width fits (a real
/// group is small, so this is one row on any sane screen — the tests derive
/// the ≤2-row bound from the real group sizes, never a hardcoded count).
#[allow(
    clippy::cast_precision_loss,   // tile counts are tiny (≤ Surface::ALL.len())
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss         // both casts are of small non-negative values
)]
#[must_use]
fn grid_layout(desktop: egui::Rect, n: usize) -> (usize, Vec<egui::Rect>) {
    if n == 0 {
        return (1, Vec::new());
    }
    let usable_w = (desktop.width() - 2.0 * GRID_MARGIN).max(CELL_W);
    let fit = (((usable_w + GRID_GAP) / (CELL_W + GRID_GAP)).floor() as usize).max(1);
    let cols = fit.min(n);
    let rows = n.div_ceil(cols);
    let grid_w = (cols as f32).mul_add(CELL_W, (cols - 1) as f32 * GRID_GAP);
    let grid_h = (rows as f32).mul_add(CELL_H, (rows - 1) as f32 * GRID_GAP);
    let origin_x = desktop.center().x - grid_w / 2.0;
    let origin_y = (desktop.center().y - grid_h / 2.0).max(desktop.top() + GRID_MARGIN);
    let rects = (0..n)
        .map(|i| {
            let (row, col) = (i / cols, i % cols);
            egui::Rect::from_min_size(
                egui::pos2(
                    (col as f32).mul_add(CELL_W + GRID_GAP, origin_x),
                    (row as f32).mul_add(CELL_H + GRID_GAP, origin_y),
                ),
                egui::vec2(CELL_W, CELL_H),
            )
        })
        .collect();
    (cols, rects)
}

/// The chosen tile's open-presence ghost (Q24 zoom-from-tile, module-scoped).
#[derive(Debug, Clone, Copy, PartialEq)]
struct ZoomGhost {
    /// The surface whose plate is zooming.
    surface: Surface,
    /// The plate rect the zoom grows out of.
    from: egui::Rect,
    /// Normalized progress through the `ZoomTile` spec, `0.0..=1.0`.
    t: f32,
}

/// The springboard's whole model: the keyboard tile selection, the one in-flight
/// gesture, the open-presence ghost, and the action queue the mount slot drains.
/// Pure — every mutation is a plain method, so pull / selection semantics
/// unit-test without a frame loop. Persisted across
/// frames in egui memory (see [`STATE_KEY`]).
#[derive(Debug, Clone)]
pub(crate) struct SpringboardState {
    /// The keyboard-selected tile index within the single desktop.
    selected: usize,
    /// The pointer gesture in flight, if any.
    gesture: Option<Gesture>,
    /// The open-presence ghost in flight, if any.
    zoom: Option<ZoomGhost>,
    /// Interactions queued for [`mount`] — drained every frame, so this never
    /// carries more than one frame's input.
    actions: Vec<SpringboardAction>,
}

impl Default for SpringboardState {
    fn default() -> Self {
        Self {
            selected: 0,
            gesture: None,
            zoom: None,
            actions: Vec::new(),
        }
    }
}

impl SpringboardState {
    /// Keep the selection on a real tile of the single desktop.
    fn clamp_selected(&mut self) {
        self.selected = self.selected.min(desktop_tiles().len().saturating_sub(1));
    }

    /// Walk the tile selection by `delta` (Tab / ArrowLeft/Right), clamping at
    /// the edges of the single desktop.
    fn select_step(&mut self, delta: isize) {
        let n = isize::try_from(desktop_tiles().len()).unwrap_or(0);
        let next = isize::try_from(self.selected).unwrap_or(0) + delta;
        if (0..n).contains(&next) {
            self.selected = usize::try_from(next).unwrap_or(0);
        }
    }

    /// Move the selection by `delta` grid rows within the desktop.
    fn select_row(&mut self, delta: isize, cols: usize) {
        let n = isize::try_from(desktop_tiles().len()).unwrap_or(0);
        let step = delta * isize::try_from(cols.max(1)).unwrap_or(1);
        let next = isize::try_from(self.selected).unwrap_or(0) + step;
        if (0..n).contains(&next) {
            self.selected = usize::try_from(next).unwrap_or(0);
        }
    }

    /// A drag began at `origin_frac_y` of the grid height.
    fn begin_drag(&mut self, origin_frac_y: f32) {
        self.gesture = Some(Gesture::Undecided {
            origin_frac_y,
            dx: 0.0,
            dy: 0.0,
        });
    }

    /// Accumulate one frame's pointer travel: classify on clearing the slop,
    /// then either ignore a horizontal drag (there are no pages) or arm/fire
    /// the Spotlight pull.
    fn drag_by(&mut self, delta_x: f32, delta_y: f32) {
        let Some(gesture) = self.gesture else {
            return;
        };
        let classified = match gesture {
            Gesture::Undecided {
                origin_frac_y,
                dx,
                dy,
            } => {
                let (dx, dy) = (dx + delta_x, dy + delta_y);
                if dx.abs().max(dy.abs()) < DRAG_SLOP {
                    Gesture::Undecided {
                        origin_frac_y,
                        dx,
                        dy,
                    }
                } else if dx.abs() >= dy.abs() {
                    Gesture::Ignored
                } else {
                    Gesture::Pull {
                        origin_frac_y,
                        dy,
                        fired: false,
                    }
                }
            }
            Gesture::Ignored => Gesture::Ignored,
            Gesture::Pull {
                origin_frac_y,
                dy,
                fired,
            } => Gesture::Pull {
                origin_frac_y,
                dy: dy + delta_y,
                fired,
            },
        };
        self.gesture = Some(match classified {
            Gesture::Ignored => Gesture::Ignored,
            Gesture::Pull {
                origin_frac_y,
                dy,
                fired,
            } => {
                // Q11: the on-home pull-down — fires the Spotlight seam ONCE
                // per gesture, only from the upper region, only downward.
                let fire = !fired && origin_frac_y <= PULL_REGION && dy >= PULL_FIRE;
                if fire {
                    self.actions.push(SpringboardAction::Spotlight);
                }
                Gesture::Pull {
                    origin_frac_y,
                    dy,
                    fired: fired || fire,
                }
            }
            undecided => undecided,
        });
    }

    /// End the current gesture. Horizontal drags are deliberately inert and a
    /// Spotlight pull has already queued its action before release.
    fn release_drag(&mut self) {
        self.gesture = None;
    }

    /// A tile was chosen: queue the open for [`mount`] and arm the Q24
    /// presence ghost (skipped under reduced motion — endpoint-only).
    fn open(&mut self, surface: Surface, from: egui::Rect, reduced: bool) {
        self.actions.push(SpringboardAction::Open(surface));
        if !reduced {
            self.zoom = Some(ZoomGhost {
                surface,
                from,
                t: 0.0,
            });
        }
    }
}

/// Drain the springboard's slot action for this frame: the interactions
/// [`show`] queued (FIFO — one pointer/keyboard can only mean one thing a
/// frame), else the `ChromeIntent::Home` this slot remains the ONE consumer
/// of (U09's contract; the intent is drained unconditionally so it never
/// backs up). The slot body in `main.rs` applies the result to `nav` / the
/// Front Door toggle — direct shell mutation stays out of this module.
#[must_use]
pub(crate) fn mount(
    ctx: &egui::Context,
    construct: &mut ConstructChrome,
) -> Option<SpringboardAction> {
    let state_key = egui::Id::new(STATE_KEY);
    let mut state = ctx
        .data_mut(|d| d.get_temp::<SpringboardState>(state_key))
        .unwrap_or_default();
    let home = construct.take_intent(ChromeIntent::Home);
    let queued = state.actions.drain(..).next();
    ctx.data_mut(|d| d.insert_temp(state_key, state));
    queued.or_else(|| home.then_some(SpringboardAction::Home))
}

/// Render the springboard as the collapsed shell's base layer (Q5), drawing
/// over the wallpaper backdrop exactly where the session EmptyState drew.
/// `overlay_above` is true while any Construct overlay / the Front Door is
/// open above — the keyboard then stays theirs (module doc).
pub(crate) fn show(ui: &mut egui::Ui, _overlay_above: bool) {
    let ctx = ui.ctx().clone();
    let state_key = egui::Id::new(STATE_KEY);
    let mut state = ctx
        .data_mut(|d| d.get_temp::<SpringboardState>(state_key))
        .unwrap_or_default();
    handle_drag(ui, &mut state, ui.max_rect());

    ctx.data_mut(|d| d.insert_temp(state_key, state));
}

#[cfg(test)]
fn show_grid(ui: &mut egui::Ui, overlay_above: bool) {
    let ctx = ui.ctx().clone();
    let state_key = egui::Id::new(STATE_KEY);
    let mut state = ctx
        .data_mut(|d| d.get_temp::<SpringboardState>(state_key))
        .unwrap_or_default();
    state.clamp_selected();
    let page_rect = ui.max_rect();
    handle_keys(&ctx, &mut state, page_rect, overlay_above);
    handle_drag(ui, &mut state, page_rect);
    paint_desktop(ui, &mut state, page_rect);
    paint_zoom_ghost(ui, &mut state);
    ctx.data_mut(|d| d.insert_temp(state_key, state));
}

/// Keyboard handling: keys consumed up front, selection moved in lock-step
/// with the module's own model — deliberately NO `request_focus` (the
/// nav_chrome double-step gotcha, module doc). Nothing is consumed while an
/// overlay is above or any widget holds real egui focus (a text field).
fn handle_keys(
    ctx: &egui::Context,
    state: &mut SpringboardState,
    page_rect: egui::Rect,
    overlay_above: bool,
) {
    if overlay_above || ctx.memory(|m| m.focused().is_some()) {
        return;
    }
    let (left, right, up, down, tab, back_tab, enter, _page_up, _page_down) = ctx.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Tab),
            i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            i.consume_key(egui::Modifiers::NONE, egui::Key::PageUp),
            i.consume_key(egui::Modifiers::NONE, egui::Key::PageDown),
        )
    });
    if left || back_tab {
        state.select_step(-1);
    }
    if right || tab {
        state.select_step(1);
    }
    let (cols, _) = grid_layout(page_rect, desktop_tiles().len());
    if up {
        state.select_row(-1, cols);
    }
    if down {
        state.select_row(1, cols);
    }
    if enter {
        if let Some(surface) = desktop_tiles().get(state.selected).copied() {
            let (_, cells) = grid_layout(page_rect, desktop_tiles().len());
            let from = cells
                .get(state.selected)
                .map_or(page_rect, |cell| plate_rect(*cell));
            state.open(surface, from, Motion::reduce_motion());
        }
    }
}

/// The whole-grid gesture target: registered BEFORE the tiles so they win the
/// hit order for clicks while a drag anywhere (tiles included — they sense
/// only clicks) can pull Spotlight. Horizontal drags are inert because the
/// desktop has no pages.
fn handle_drag(ui: &egui::Ui, state: &mut SpringboardState, page_rect: egui::Rect) {
    let bg = ui.interact(
        page_rect,
        egui::Id::new(SPRINGBOARD_BG),
        egui::Sense::drag(),
    );
    if bg.drag_started() {
        let frac = bg.interact_pointer_pos().map_or(1.0, |pos| {
            ((pos.y - page_rect.top()) / page_rect.height().max(1.0)).clamp(0.0, 1.0)
        });
        state.begin_drag(frac);
    }
    if bg.dragged() {
        let delta = bg.drag_delta();
        state.drag_by(delta.x, delta.y);
    }
    if bg.drag_stopped() {
        state.release_drag();
    }
}

/// Paint the one desktop's complete tile list. Each tile retains the accent of
/// its taxonomy group, but no group heading is painted on the desktop.
fn paint_desktop(ui: &egui::Ui, state: &mut SpringboardState, page_rect: egui::Rect) {
    let painter = ui.painter().clone();
    let reduced = Motion::reduce_motion();
    let tiles = desktop_tiles();
    let (_, cells) = grid_layout(page_rect, tiles.len());
    for (tile_idx, (surface, cell)) in tiles.iter().copied().zip(&cells).enumerate() {
        let selected = tile_idx == state.selected;
        let accent = group_accent(surface).unwrap_or(Style::ACCENT);
        if tile(ui, &painter, surface, *cell, accent, selected) {
            state.selected = tile_idx;
            state.open(surface, plate_rect(*cell), reduced);
        }
    }
}

/// Interact + paint one tile. Returns whether it was clicked.
///
// PLATFORM-INTERFACES Q5/Q8/Q22 — the locked tile: a RADIUS_XL rounded-rect
// plate in the ONE shared `tile_plate_fill(group accent)` derivation, the
// white surface glyph through the dock's shared loader, and the label
// beneath. No badges, no live data.
fn tile(
    ui: &egui::Ui,
    painter: &egui::Painter,
    surface: Surface,
    cell: egui::Rect,
    accent: egui::Color32,
    selected: bool,
) -> bool {
    let ctx = ui.ctx().clone();
    let plate = plate_rect(cell);
    let id = egui::Id::new((SPRINGBOARD_BG, "tile", surface));
    let resp = ui.interact(plate, id, egui::Sense::click());

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let radius = egui::CornerRadius::same(Style::RADIUS_XL as u8);
    painter.rect_filled(
        plate,
        radius,
        Style::resolve_color(&ctx, Style::tile_plate_fill(accent)),
    );
    if selected {
        // The keyboard selection wears the platform 2 px focus-ring treatment
        // (lock-step model state, not egui focus — module doc).
        painter.rect_stroke(
            plate,
            radius,
            Style::focus_stroke(),
            egui::StrokeKind::Inside,
        );
    } else if resp.hovered() {
        painter.rect_stroke(plate, radius, Style::hairline(), egui::StrokeKind::Inside);
    }

    let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
    if let Some(tex) = icon_texture(&ctx, surface.icon_id(), GLYPH_EDGE, Style::TILE_GLYPH) {
        let glyph =
            egui::Rect::from_center_size(plate.center(), egui::vec2(GLYPH_EDGE, GLYPH_EDGE));
        painter.image(tex.id(), glyph, uv, egui::Color32::WHITE);
    }
    let label = painter.layout_job(Style::typography_job(
        surface.label(),
        TypographyRole::Label,
        Style::resolve_color(&ctx, Style::TEXT),
        f32::INFINITY,
    ));
    painter.galley(
        egui::pos2(
            cell.center().x - label.size().x / 2.0,
            plate.bottom() + (LABEL_BAND - label.size().y) / 2.0,
        ),
        label,
        Style::resolve_color(&ctx, Style::TEXT),
    );

    resp.clicked()
}

/// The chosen tile's open-presence ghost: the plate swells and fades on the
/// `ZoomTile` spec while the collapsed layer cross-fades out beneath the
/// opening surface.
///
/// **U29 seam** — the FULL Q24 zoom-from-tile (the opening surface's own body
/// scaling up out of this tile rect, and back down into it on close) needs
/// the expanded layer's render pass and lands with the U29 cutover; this
/// module's reach ends at the base layer, so the presence half lives here and
/// U29 picks up the same `from` rect for the body half.
fn paint_zoom_ghost(ui: &egui::Ui, state: &mut SpringboardState) {
    let Some(mut zoom) = state.zoom.take() else {
        return;
    };
    let ctx = ui.ctx().clone();
    let spec = Motion::spec(MotionPreset::ZoomTile);
    let eased = spec.easing.sample(zoom.t);
    let scale = (ZOOM_GHOST_SCALE - 1.0).mul_add(eased, 1.0);
    let fade = 1.0 - eased;
    let rect = egui::Rect::from_center_size(zoom.from.center(), zoom.from.size() * scale);
    let accent = group_accent(zoom.surface).unwrap_or(Style::ACCENT);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let radius = egui::CornerRadius::same(Style::RADIUS_XL as u8);
    ui.painter().rect_filled(
        rect,
        radius,
        Style::resolve_color(&ctx, Style::tile_plate_fill(accent)).linear_multiply(fade),
    );
    let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
    if let Some(tex) = icon_texture(&ctx, zoom.surface.icon_id(), GLYPH_EDGE, Style::TILE_GLYPH) {
        let glyph =
            egui::Rect::from_center_size(rect.center(), egui::vec2(GLYPH_EDGE, GLYPH_EDGE) * scale);
        ui.painter().image(
            tex.id(),
            glyph,
            uv,
            egui::Color32::WHITE.linear_multiply(fade),
        );
    }
    zoom.t += ctx.input(|i| i.stable_dt) / spec.normal_secs.max(f32::EPSILON);
    if zoom.t < 1.0 {
        state.zoom = Some(zoom);
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::construct::ChromeInput;
    use std::time::Duration;

    fn ctx() -> egui::Context {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        ctx
    }

    const SCREEN: egui::Vec2 = egui::vec2(1280.0, 800.0);

    fn raw(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
            events,
            ..Default::default()
        }
    }

    /// One headless collapsed-central-view frame — a `CentralPanel` hosting
    /// [`show`], the exact call shape of `central_view`'s collapsed branch.
    /// Returns the panel's inner rect (for aiming pointer events) + output.
    fn frame_with(
        ctx: &egui::Context,
        overlay_above: bool,
        events: Vec<egui::Event>,
    ) -> (egui::Rect, egui::FullOutput) {
        let mut inner = egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN);
        let out = ctx.run(raw(events), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                inner = ui.max_rect();
                show_grid(ui, overlay_above);
            });
        });
        (inner, out)
    }

    fn frame(ctx: &egui::Context, events: Vec<egui::Event>) -> (egui::Rect, egui::FullOutput) {
        frame_with(ctx, false, events)
    }

    /// One production-shaped collapsed-central-view frame. Unlike [`frame`],
    /// this calls [`show`] exactly once: `central_view` owns one CentralPanel
    /// pass per egui frame, so pointer regressions must not rely on a doubled
    /// test mount.
    fn production_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> (egui::Rect, egui::FullOutput) {
        let mut inner = egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN);
        let out = ctx.run(raw(events), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                inner = ui.max_rect();
                show(ui, false);
            });
        });
        (inner, out)
    }

    fn state_of(ctx: &egui::Context) -> SpringboardState {
        ctx.data_mut(|d| d.get_temp::<SpringboardState>(egui::Id::new(STATE_KEY)))
            .unwrap_or_default()
    }

    fn set_state(ctx: &egui::Context, state: SpringboardState) {
        ctx.data_mut(|d| d.insert_temp(egui::Id::new(STATE_KEY), state));
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn painted_fills(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Color32> {
        let mut fills = Vec::new();
        for clipped in shapes {
            collect_fills(&clipped.shape, &mut fills);
        }
        fills
    }

    fn collect_fills(shape: &egui::Shape, out: &mut Vec<egui::Color32>) {
        match shape {
            egui::Shape::Rect(rect) => out.push(rect.fill),
            egui::Shape::Vec(shapes) => {
                for s in shapes {
                    collect_fills(s, out);
                }
            }
            _ => {}
        }
    }

    fn painted_texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
        let mut texts = Vec::new();
        for clipped in shapes {
            collect_texts(&clipped.shape, &mut texts);
        }
        texts
    }

    fn collect_texts(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
            egui::Shape::Vec(shapes) => {
                for s in shapes {
                    collect_texts(s, out);
                }
            }
            _ => {}
        }
    }

    // --- the single-desktop projection: the runtime twin of the compile-time guard

    #[test]
    fn desktop_lists_every_surface_exactly_once() {
        let all_tiles = desktop_tiles();
        let total = all_tiles.len();
        assert_eq!(
            total,
            Surface::ALL.len(),
            "the desktop tile list must be exactly the launchable platform"
        );
        for surface in Surface::ALL {
            assert_eq!(all_tiles.iter().filter(|s| **s == surface).count(), 1);
        }
        assert_eq!(all_tiles, &Surface::ALL);
    }

    // --- grid math (pure) ---------------------------------------------------------

    #[test]
    fn grid_layout_is_centered_bounded_for_the_single_desktop() {
        let page = egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN);
        let n = desktop_tiles().len();
        let (cols, cells) = grid_layout(page, n);
        assert!(cols >= 1);
        assert_eq!(cells.len(), n);
        for cell in &cells {
            assert!(cell.left() >= page.left(), "{cell:?}");
            assert!(cell.right() <= page.right(), "{cell:?}");
            assert!(cell.top() >= page.top(), "{cell:?}");
        }
        // One tile sits dead-centre horizontally.
        let (_, cells) = grid_layout(page, 1);
        assert!((cells[0].center().x - page.center().x).abs() < 0.5);
    }

    // --- selection on the single desktop (pure) ----------------------------------

    #[test]
    fn select_step_walks_the_complete_desktop_and_clamps_at_the_edges() {
        let mut s = SpringboardState::default();
        s.select_step(-1);
        assert_eq!(s.selected, 0, "the very first tile clamps");
        let last = desktop_tiles().len() - 1;
        for _ in 0..=last {
            s.select_step(1);
        }
        assert_eq!(
            s.selected, last,
            "stepping past the last tile clamps on the single desktop"
        );
        s.select_step(-1);
        assert_eq!(
            s.selected,
            last - 1,
            "stepping back walks to the preceding icon"
        );
        s.selected = last;
        s.select_step(1);
        assert_eq!(s.selected, last);
    }

    #[test]
    fn vertical_moves_stay_within_the_single_desktop() {
        let mut s = SpringboardState::default();
        let (cols, _) = grid_layout(
            egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN),
            desktop_tiles().len(),
        );
        s.select_row(1, cols);
        assert!(s.selected < desktop_tiles().len());
    }

    // --- the Q11 pull-down → Spotlight seam (pure) --------------------------------

    #[test]
    fn a_pull_down_from_the_upper_region_fires_spotlight_exactly_once() {
        let mut s = SpringboardState::default();
        s.begin_drag(0.2);
        s.drag_by(0.0, 30.0);
        assert!(s.actions.is_empty(), "under the fire threshold: armed only");
        s.drag_by(0.0, 40.0);
        assert_eq!(s.actions, vec![SpringboardAction::Spotlight]);
        s.drag_by(0.0, 40.0);
        assert_eq!(s.actions.len(), 1, "one fire per gesture");
        s.release_drag();
    }

    #[test]
    fn a_low_origin_or_horizontal_drag_never_spotlights() {
        // Low origin: a long downward drag from the grid's lower region is inert.
        let mut s = SpringboardState::default();
        s.begin_drag(0.8);
        s.drag_by(0.0, 200.0);
        assert!(s.actions.is_empty(), "a low-origin pull never fires");
        s.release_drag();
        // Dominant-horizontal: inert, not the pull, because there are no pages.
        s.begin_drag(0.2);
        s.drag_by(-200.0, 40.0);
        assert!(s.actions.is_empty(), "a horizontal drag is not the pull");
        assert!(matches!(s.gesture, Some(Gesture::Ignored)));
        s.release_drag();
    }

    // --- the open presence ghost (Q24, module-scoped) -----------------------------

    #[test]
    fn open_queues_the_action_and_arms_the_zoom_ghost_unless_reduced() {
        let from = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(PLATE_EDGE, PLATE_EDGE));
        let mut s = SpringboardState::default();
        s.open(Surface::Music, from, false);
        assert_eq!(s.actions, vec![SpringboardAction::Open(Surface::Music)]);
        assert!(s.zoom.is_some(), "the ZoomTile presence ghost arms on open");
        let mut s = SpringboardState::default();
        s.open(Surface::Music, from, true);
        assert!(s.zoom.is_none(), "reduced motion: endpoint-only, no ghost");
    }

    // --- the mount seam -----------------------------------------------------------

    #[test]
    fn mount_routes_queued_actions_then_the_home_intent_exactly_once() {
        let ctx = ctx();
        let mut construct = ConstructChrome::default();
        assert_eq!(mount(&ctx, &mut construct), None, "a quiet frame is quiet");

        // A queued open (what a click/Enter in `show` records) routes out FIFO.
        let mut s = SpringboardState::default();
        s.open(
            Surface::Workbench,
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1.0, 1.0)),
            true,
        );
        set_state(&ctx, s);
        assert_eq!(
            mount(&ctx, &mut construct),
            Some(SpringboardAction::Open(Surface::Workbench))
        );
        assert_eq!(
            mount(&ctx, &mut construct),
            None,
            "an action drains exactly once"
        );

        // The §2.3 Home row (Super tap over an expanded app) → Home, once.
        construct.dispatch(&ChromeInput {
            super_tap: true,
            super_tab: false,
            app_expanded: true,
            remote_session_focused: false,
            edges: Vec::new(),
            now: Duration::ZERO,
        });
        assert_eq!(mount(&ctx, &mut construct), Some(SpringboardAction::Home));
        assert_eq!(
            mount(&ctx, &mut construct),
            None,
            "this slot is the ONE Home consumer and drains it exactly once"
        );
    }

    // --- keyboard flow (integration) ----------------------------------------------

    #[test]
    fn enter_opens_the_selected_tile_and_arrows_walk_in_lock_step() {
        let ctx = ctx();
        let mut construct = ConstructChrome::default();
        frame(&ctx, Vec::new()); // settle
        frame(&ctx, vec![key(egui::Key::Enter)]);
        assert_eq!(
            mount(&ctx, &mut construct),
            Some(SpringboardAction::Open(desktop_tiles()[0])),
            "Enter opens the first (selected) tile of the desktop"
        );
        frame(&ctx, vec![key(egui::Key::ArrowRight)]);
        assert_eq!(state_of(&ctx).selected, 1, "ArrowRight walks the tiles");
        frame(&ctx, vec![key(egui::Key::Enter)]);
        assert_eq!(
            mount(&ctx, &mut construct),
            Some(SpringboardAction::Open(desktop_tiles()[1])),
            "Enter routes the arrow-selected neighbor"
        );
    }

    #[test]
    fn page_keys_are_harmless_on_the_single_desktop() {
        let ctx = ctx();
        frame(&ctx, Vec::new());
        frame(&ctx, vec![key(egui::Key::PageDown)]);
        assert_eq!(
            state_of(&ctx).selected,
            0,
            "PageDown cannot leave the desktop"
        );
        frame(&ctx, vec![key(egui::Key::PageUp)]);
        assert_eq!(
            state_of(&ctx).selected,
            0,
            "PageUp cannot leave the desktop"
        );
    }

    #[test]
    fn an_overlay_above_owns_the_keyboard() {
        let ctx = ctx();
        frame(&ctx, Vec::new());
        frame_with(&ctx, true, vec![key(egui::Key::ArrowRight)]);
        assert_eq!(
            state_of(&ctx).selected,
            0,
            "with chrome above, the springboard consumes nothing"
        );
        frame(&ctx, vec![key(egui::Key::ArrowRight)]);
        assert_eq!(state_of(&ctx).selected, 1, "alone again, keys are its own");
    }

    // --- pointer flow (integration) -------------------------------------------------

    #[test]
    fn a_click_on_a_tile_routes_that_surface_out() {
        let ctx = ctx();
        let mut construct = ConstructChrome::default();
        let (inner, _) = frame(&ctx, Vec::new());
        let (inner2, _) = frame(&ctx, Vec::new());
        assert_eq!(inner, inner2, "the panel rect is stable across frames");
        let page_rect = inner;
        let (_, cells) = grid_layout(page_rect, desktop_tiles().len());
        let target = plate_rect(cells[1]).center();
        frame(
            &ctx,
            vec![
                egui::Event::PointerMoved(target),
                egui::Event::PointerButton {
                    pos: target,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
        frame(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: target,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }],
        );
        assert_eq!(
            mount(&ctx, &mut construct),
            Some(SpringboardAction::Open(desktop_tiles()[1])),
            "the clicked tile routes its surface"
        );
    }

    #[test]
    fn production_home_does_not_route_pointer_clicks_to_launcher_tiles() {
        let ctx = ctx();
        let mut construct = ConstructChrome::default();
        let (inner, _) = production_frame(&ctx, Vec::new());
        let (inner2, _) = production_frame(&ctx, Vec::new());
        assert_eq!(inner, inner2, "the production panel rect is stable");
        let target = inner.center();
        production_frame(
            &ctx,
            vec![
                egui::Event::PointerMoved(target),
                egui::Event::PointerButton {
                    pos: target,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
        assert_eq!(mount(&ctx, &mut construct), None);
    }

    #[test]
    fn a_pointer_pull_down_from_the_top_reaches_mount_as_spotlight() {
        let ctx = ctx();
        let mut construct = ConstructChrome::default();
        let (inner, _) = frame(&ctx, Vec::new());
        frame(&ctx, Vec::new());
        let start = egui::pos2(inner.center().x, inner.top() + inner.height() * 0.1);
        frame(
            &ctx,
            vec![
                egui::Event::PointerMoved(start),
                egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
        let pulled = egui::pos2(start.x, start.y + PULL_FIRE + 40.0);
        frame(&ctx, vec![egui::Event::PointerMoved(pulled)]);
        frame(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: pulled,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }],
        );
        assert_eq!(
            mount(&ctx, &mut construct),
            Some(SpringboardAction::Spotlight),
            "the Q11 on-home pull-down reaches the slot as the Spotlight seam"
        );
    }

    // --- the collapsed base layer paints honestly -----------------------------------

    #[test]
    fn the_production_home_has_no_launcher_titles_or_tile_plates() {
        let ctx = ctx();
        production_frame(&ctx, Vec::new());
        let (_, out) = production_frame(&ctx, Vec::new());
        let fills = painted_fills(&out.shapes);
        assert!(
            !fills.iter().any(|fill| {
                LAUNCHER_GROUPS
                    .iter()
                    .any(|group| *fill == Style::tile_plate_fill(group.accent))
            }),
            "the production Home must not paint launcher plates: {fills:?}"
        );
        let texts = painted_texts(&out.shapes);
        for surface in desktop_tiles() {
            assert!(
                !texts.iter().any(|text| text == surface.label()),
                "launcher title leaked into Home: {}",
                surface.label()
            );
        }
        for title in [
            "Mesh Control",
            "Desktop & Session",
            "Files & Data",
            "Web",
            "Developer Tools",
            "Mesh Teams",
        ] {
            assert!(
                !texts.iter().any(|text| text == title),
                "desktop title leaked: {title}"
            );
        }
        assert!(
            !texts.iter().any(|t| t.contains("No active session")),
            "Q5: the session EmptyState is retired from the collapsed view"
        );
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the springboard must tessellate real geometry"
        );
    }
}
