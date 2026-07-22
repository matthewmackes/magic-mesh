//! `switcher` — WL-UX-006/U15: the **Construct app switcher** — the Q16 card
//! grid of recently-active surfaces with preview snapshots, mounted from the
//! U09 scaffold's reserved `mount_switcher_slot` (the slot construct.rs labels
//! "U12 switcher"; this unit landed as U15 of the fan-out).
//!
// PLATFORM-INTERFACES Q16 — the locked switcher: "a card grid of open
// surfaces with live preview snapshots — snapshot-on-leave with a plate
// fallback, flick-up to close, Super+Tab / bottom-swipe-up-hold to open."
// NEVER a live-render requirement: a card shows the newest *snapshot* the
// shell already holds, or the honest accent plate.
//!
//! ## The recents model, honestly
//!
//! Shell surfaces are **stateless singletons** — there is no process behind a
//! card to terminate. The switcher therefore keeps an ordered **recents ring**
//! (most-recent-first, capped at the number of real surfaces): a surface
//! entering the foreground promotes to the front, and flick-up "close" only
//! **removes the card from the ring** — it forgets recency, it tears nothing
//! down (the surface remains one launcher/Front-Door tap away, unchanged).
//! The ring freezes while the grid is showing, so a card mid-flick is never
//! resurrected by the same-frame promote of the still-foreground surface.
//!
//! ## Snapshots: what is real today
//!
//! * The **Desktop card** shows the live VDI frame texture the taskbar preview
//!   already holds (`vdi::taskbar_preview_frame` — a real decoded frame).
//! * Every other card shows the **fallback plate** — the Q22 tile treatment
//!   ([`Style::tile_plate_fill`] over the launcher-group accent + the white
//!   surface glyph) — which the Q16 lock names as the legitimate no-snapshot
//!   rendering, not a placeholder.
//! * **Snapshot-on-leave for arbitrary egui surfaces is deferred to U29**: the
//!   only in-tree rasterizer (`mde_egui::capture`) is the headless offscreen
//!   wgpu PNG path — wiring it (or a live copy of the shell's own render
//!   target) to fire on surface-leave needs `main.rs` surgery beyond this
//!   unit's slot. The hook is this module's [`mount`] `desktop_preview`
//!   parameter generalized to a per-surface texture map; U29 feeds it.
//!
//! ## Input
//!
//! Open/close arrives ONLY as the drained [`ChromeIntent::Switcher`] (U09's
//! one-dispatcher rule; Super+Tab today, bottom-swipe-up-hold with U16 — and
//! note Super+Tab also still dual-routes the legacy `SessionSwitch` behavior
//! until the U29 cutover). While open: Escape / scrim-click closes,
//! click/Enter switches, arrow keys move the selection **in lock-step**
//! (selection is this module's own state, keys consumed up front, and
//! deliberately NO `request_focus` — driving egui's focus a second time is
//! the double-step desync `mde_egui::nav_chrome` documents). Flick-up on a
//! card (mouse drag or the TouchTranslator's synthesized primary pointer)
//! past the threshold removes it, with the shared [`Motion`] `DragSettle`
//! spring flying the card out (reduced motion: instant removal).

use mde_egui::motion::Spring;
use mde_egui::{egui, Motion, MotionPreset, Style};

use crate::construct::{ChromeIntent, ConstructChrome};
use crate::dock::{icon_texture, Surface, LAUNCHER_GROUPS};

/// Stable id of the switcher's foreground overlay layer.
const SWITCHER_AREA: &str = "construct-switcher-area";
/// Stable id of the full-screen scrim's click target.
const SWITCHER_SCRIM: &str = "construct-switcher-scrim";
/// Stable egui-memory key the per-frame [`SwitcherState`] persists under —
/// the U09 contract keeps `main.rs` free of new fields, so the recents ring
/// rides egui memory exactly like the backdrop's wallpaper cache.
const STATE_KEY: &str = "construct-switcher-state";

/// Card width in logical points.
const CARD_W: f32 = 256.0;
/// Height of the label + glyph header band at the top of each card.
const HEADER_H: f32 = 36.0;
/// Height of the preview region under the header (16:10 of [`CARD_W`]).
const PREVIEW_H: f32 = 160.0;
/// Full card height.
const CARD_H: f32 = HEADER_H + PREVIEW_H;
/// Gap between grid cards.
const GRID_GAP: f32 = Style::SP_M;
/// Outer margin the grid keeps from the screen edges.
const GRID_MARGIN: f32 = Style::SP_XL;
/// Upward drag distance (as a fraction of card height) past which a release
/// removes the card from recents — the flick-up-to-close threshold.
const FLICK_CLOSE_FRACTION: f32 = 0.30;
/// Edge length of the big white plate glyph (the Q22 tile silhouette).
const PLATE_GLYPH: f32 = 48.0;
/// Edge length of the small header glyph beside the card label.
const HEADER_GLYPH: f32 = 20.0;
/// Thickness of the current-surface accent bar on a card's bottom edge.
const CURRENT_BAR_H: f32 = 3.0;

/// The launcher-group accent for `surface` off the ONE shared taxonomy table
/// (`dock::LAUNCHER_GROUPS`) — the plate's Q22 accent source. (The dock's own
/// `launcher_group_accent` helper is `#[cfg(test)]`-gated, so the production
/// lookup folds the same table here rather than un-gating a shared file.)
fn group_accent(surface: Surface) -> Option<egui::Color32> {
    LAUNCHER_GROUPS
        .iter()
        .find(|group| group.surfaces.contains(&surface))
        .map(|group| group.accent)
}

/// An in-flight upward drag on one card.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CardDrag {
    /// The card being dragged.
    surface: Surface,
    /// Accumulated upward travel in points (never negative).
    offset: f32,
}

/// A released card settling on the shared `DragSettle` spring: either flying
/// out (`target > 0`, removal on arrival) or springing back (`target == 0`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct CardAnim {
    /// The card being animated.
    surface: Surface,
    /// Current upward offset in points.
    offset: f32,
    /// Current spring velocity.
    vel: f32,
    /// Spring target: the fly-out distance, or `0.0` for a spring-back.
    target: f32,
}

/// The switcher's whole model: the recents ring (most-recent-first), the
/// keyboard selection, and the one card drag/settle in flight. Pure — every
/// mutation is a plain method, so the ring/flick semantics unit-test without
/// a frame loop. Persisted across frames in egui memory (see [`STATE_KEY`]).
#[derive(Debug, Clone, Default)]
pub struct SwitcherState {
    /// Recently-active surfaces, most recent first, capped at
    /// `Surface::ALL.len()` (every card is a real surface, so the ring can
    /// never outgrow the platform).
    ring: Vec<Surface>,
    /// Index into [`Self::ring`] of the keyboard-selected card.
    selected: usize,
    /// The upward drag in progress, if any.
    drag: Option<CardDrag>,
    /// The released card still settling, if any.
    anim: Option<CardAnim>,
}

impl SwitcherState {
    /// A surface entered the foreground: promote it to the ring's front
    /// (dedupe — a surface appears once) and cap at the real-surface count.
    pub fn promote(&mut self, surface: Surface) {
        if self.ring.first() == Some(&surface) {
            return;
        }
        self.ring.retain(|s| *s != surface);
        self.ring.insert(0, surface);
        self.ring.truncate(Surface::ALL.len());
        self.clamp_selection();
    }

    /// Forget `surface`'s recency — the Q16 "close". Surfaces are stateless
    /// singletons (module doc): this removes the card from the ring and
    /// nothing else; there is no session or process to end.
    pub fn remove(&mut self, surface: Surface) {
        self.ring.retain(|s| *s != surface);
        self.clamp_selection();
    }

    /// The recents ring, most recent first. Test-only observability (render
    /// reads the field directly; the dock gates its sibling helpers the same
    /// way).
    #[cfg(test)]
    #[must_use]
    pub fn ring(&self) -> &[Surface] {
        &self.ring
    }

    /// Keep the selection on a real card after any ring mutation.
    fn clamp_selection(&mut self) {
        self.selected = self.selected.min(self.ring.len().saturating_sub(1));
    }

    /// Point the selection at `surface` if it is in the ring (used on open so
    /// the current surface starts highlighted, per Q16).
    fn select(&mut self, surface: Surface) {
        if let Some(idx) = self.ring.iter().position(|s| *s == surface) {
            self.selected = idx;
        }
    }

    /// Move the keyboard selection by `(dcol, drow)` on a `cols`-wide grid,
    /// clamped to real cards — the lock-step model (module doc).
    fn move_selection(&mut self, dcol: isize, drow: isize, cols: usize) {
        if self.ring.is_empty() || cols == 0 {
            return;
        }
        let step = dcol + drow * isize::try_from(cols).unwrap_or(1);
        let last = isize::try_from(self.ring.len() - 1).unwrap_or(0);
        let next = isize::try_from(self.selected).unwrap_or(0) + step;
        self.selected = usize::try_from(next.clamp(0, last)).unwrap_or(0);
    }

    /// A drag started on `surface`'s card.
    fn begin_drag(&mut self, surface: Surface) {
        self.drag = Some(CardDrag {
            surface,
            offset: 0.0,
        });
        self.anim = None;
    }

    /// Accumulate one frame's pointer travel (`delta_y` in egui screen
    /// coordinates, negative = up). Only upward travel arms the flick; a
    /// downward drag walks the offset back toward rest.
    fn drag_by(&mut self, delta_y: f32) {
        if let Some(drag) = &mut self.drag {
            drag.offset = (drag.offset - delta_y).max(0.0);
        }
    }

    /// The drag released. Past `threshold` the card is closed: instantly under
    /// reduced motion, else via the `DragSettle` spring flying it `fly_target`
    /// points up and removing it on arrival ([`Self::step_anim`]). Below the
    /// threshold the card springs back to rest.
    fn release_drag(&mut self, threshold: f32, fly_target: f32, reduced: bool) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if drag.offset >= threshold {
            if reduced {
                self.remove(drag.surface);
            } else {
                self.anim = Some(CardAnim {
                    surface: drag.surface,
                    offset: drag.offset,
                    vel: 0.0,
                    target: fly_target.max(threshold),
                });
            }
        } else if drag.offset > 0.5 {
            self.anim = Some(CardAnim {
                surface: drag.surface,
                offset: drag.offset,
                vel: 0.0,
                target: 0.0,
            });
        }
    }

    /// Advance the settle spring by `dt` seconds. A fly-out that reaches its
    /// target removes the card from the ring; a spring-back just settles.
    /// Returns whether an animation is still running (the caller keeps
    /// repainting while it is). Pure given `dt`, so tests pump it directly.
    fn step_anim(&mut self, dt: f32) -> bool {
        let Some(mut anim) = self.anim.take() else {
            return false;
        };
        let spring = Motion::spec(MotionPreset::DragSettle)
            .spring
            .unwrap_or(Spring::SNAPPY);
        let (offset, vel) = spring.step(anim.offset, anim.vel, anim.target, dt);
        anim.offset = offset;
        anim.vel = vel;
        let flying_out = anim.target > 0.0;
        if flying_out && (offset >= anim.target - 1.0 || spring.settled(offset, vel, anim.target)) {
            self.remove(anim.surface);
        } else if !flying_out && spring.settled(offset, vel, 0.0) {
            // Settled back to rest.
        } else {
            self.anim = Some(anim);
        }
        self.anim.is_some()
    }

    /// The overlay is closing with a fly-out still in flight: apply the
    /// removal instantly (the animation has nowhere left to render) and drop
    /// every transient.
    fn finish_transients(&mut self) {
        if let Some(anim) = self.anim.take() {
            if anim.target > 0.0 {
                self.remove(anim.surface);
            }
        }
        self.drag = None;
    }

    /// The current upward paint offset of `surface`'s card (drag or settle).
    fn card_offset(&self, surface: Surface) -> f32 {
        if let Some(drag) = &self.drag {
            if drag.surface == surface {
                return drag.offset;
            }
        }
        if let Some(anim) = &self.anim {
            if anim.surface == surface {
                return anim.offset;
            }
        }
        0.0
    }
}

/// The centered card grid for `n` cards on `screen`: the column count and one
/// rect per card, top-left to bottom-right in ring order. Squarish (columns ≈
/// √n) but never wider than the screen fits. Pure, so the click test aims at
/// real card geometry.
#[allow(
    clippy::cast_precision_loss,   // card counts are tiny (≤ Surface::ALL.len())
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss         // both casts are of small non-negative values
)]
#[must_use]
fn grid_layout(screen: egui::Rect, n: usize) -> (usize, Vec<egui::Rect>) {
    if n == 0 {
        return (1, Vec::new());
    }
    let usable_w = (screen.width() - 2.0 * GRID_MARGIN).max(CARD_W);
    let fit = (((usable_w + GRID_GAP) / (CARD_W + GRID_GAP)).floor() as usize).max(1);
    let square = ((n as f32).sqrt().ceil() as usize).max(1);
    let cols = fit.min(square).min(n).max(1);
    let rows = n.div_ceil(cols);
    let grid_w = (cols as f32).mul_add(CARD_W, (cols - 1) as f32 * GRID_GAP);
    let grid_h = (rows as f32).mul_add(CARD_H, (rows - 1) as f32 * GRID_GAP);
    let origin_x = screen.center().x - grid_w / 2.0;
    let origin_y = (screen.center().y - grid_h / 2.0).max(screen.top() + GRID_MARGIN);
    let rects = (0..n)
        .map(|i| {
            let (row, col) = (i / cols, i % cols);
            egui::Rect::from_min_size(
                egui::pos2(
                    (col as f32).mul_add(CARD_W + GRID_GAP, origin_x),
                    (row as f32).mul_add(CARD_H + GRID_GAP, origin_y),
                ),
                egui::vec2(CARD_W, CARD_H),
            )
        })
        .collect();
    (cols, rects)
}

/// Mount the app switcher into the U09 slot for this frame.
///
/// Consumes this frame's [`ChromeIntent::Switcher`] (toggling
/// `construct.switcher_open`), feeds the recents ring from the shell's current
/// surface (`current`/`app_expanded` — a surface entering the foreground
/// promotes), and renders the Q16 overlay while open. `desktop_preview` is
/// the live VDI frame texture for the Desktop card when one exists (the U29
/// hook generalizes this to a per-surface snapshot map, module doc).
///
/// Returns the surface a click/Enter chose, if any — the one-line slot body in
/// `main.rs` applies it to `nav` (the U09 "main.rs never changes again" rule
/// keeps direct nav mutation out of this module).
pub fn mount(
    ctx: &egui::Context,
    construct: &mut ConstructChrome,
    current: Surface,
    app_expanded: bool,
    desktop_preview: Option<egui::TextureHandle>,
) -> Option<Surface> {
    let state_key = egui::Id::new(STATE_KEY);
    let mut state = ctx
        .data_mut(|d| d.get_temp::<SwitcherState>(state_key))
        .unwrap_or_default();

    // The ring freezes while the grid shows (module doc), so a card mid-flick
    // is not re-promoted by the still-foreground surface under the overlay.
    if app_expanded && !construct.switcher_open {
        state.promote(current);
    }

    if construct.take_intent(ChromeIntent::Switcher) {
        construct.switcher_open = !construct.switcher_open;
        if construct.switcher_open {
            // Q16: the grid opens with the current surface highlighted.
            state.select(current);
            state.drag = None;
            state.anim = None;
        }
    }

    let mut chosen = None;
    if construct.switcher_open {
        chosen = overlay(
            ctx,
            construct,
            &mut state,
            current,
            desktop_preview.as_ref(),
        );
    } else {
        state.finish_transients();
    }
    if chosen.is_some() {
        construct.switcher_open = false;
    }
    if !construct.switcher_open {
        state.finish_transients();
    }

    ctx.data_mut(|d| d.insert_temp(state_key, state));
    chosen
}

/// Render the open switcher: full-screen scrim, centered card grid, keyboard
/// and pointer handling. Returns the chosen surface, if any.
fn overlay(
    ctx: &egui::Context,
    construct: &mut ConstructChrome,
    state: &mut SwitcherState,
    current: Surface,
    desktop_preview: Option<&egui::TextureHandle>,
) -> Option<Surface> {
    let screen = ctx.screen_rect();
    let (cols, rects) = grid_layout(screen, state.ring.len());

    // Keys first, consumed up front so nothing beneath re-reads them this
    // frame. Selection moves in lock-step with our own model — deliberately
    // no `request_focus` (the nav_chrome double-step gotcha, module doc).
    let (escape, enter, left, right, up, down) = ctx.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
        )
    });
    if left {
        state.move_selection(-1, 0, cols);
    }
    if right {
        state.move_selection(1, 0, cols);
    }
    if up {
        state.move_selection(0, -1, cols);
    }
    if down {
        state.move_selection(0, 1, cols);
    }

    let mut chosen = None;
    let mut close = escape;
    if enter {
        chosen = state.ring.get(state.selected).copied();
        close = close || chosen.is_some();
    }

    egui::Area::new(egui::Id::new(SWITCHER_AREA))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .constrain(false)
        // No egui implicit fade-in: it opacity-multiplies the locked scrim
        // material (dishonest alpha) and leaves the first frames' widgets
        // invisible→non-interactable; platform motion rides `Motion` only.
        .fade_in(false)
        // Not movable, and the Area's OWN whole-area widget senses nothing:
        // by default it senses drag (movable) or click (interactable) and is
        // registered LAST, so it wins egui's same-distance hit tie-break and
        // silently swallows every scrim/card click beneath it.
        .movable(false)
        .sense(egui::Sense::hover())
        .show(ctx, |ui| {
            let (_, _) = ui.allocate_exact_size(screen.size(), egui::Sense::hover());
            // Q16 backdrop: the ONE shared regular scrim material, and a
            // whole-screen click target behind the cards (a scrim click
            // closes; cards interact after, so they sit above in hit order).
            ui.painter().rect_filled(
                screen,
                egui::CornerRadius::ZERO,
                Style::resolve_color(ctx, Style::SCRIM_REGULAR),
            );
            let scrim = ui.interact(screen, egui::Id::new(SWITCHER_SCRIM), egui::Sense::click());
            if scrim.clicked() {
                close = true;
            }

            if state.ring.is_empty() {
                ui.painter().text(
                    screen.center(),
                    egui::Align2::CENTER_CENTER,
                    "No recent apps",
                    egui::FontId::proportional(Style::TYPE_CALLOUT),
                    Style::resolve_color(ctx, Style::TEXT_DIM),
                );
                return;
            }

            let ring = state.ring.clone();
            for (idx, (surface, base_rect)) in ring.iter().copied().zip(&rects).enumerate() {
                if let Some(pick) = card(
                    ui,
                    CardPaint {
                        surface,
                        base_rect: *base_rect,
                        screen,
                        selected: idx == state.selected,
                        is_current: surface == current,
                        preview: (surface == Surface::Desktop)
                            .then_some(desktop_preview)
                            .flatten(),
                    },
                    state,
                ) {
                    chosen = Some(pick);
                }
            }
        });

    // Advance the flick fly-out / spring-back off the frame clock.
    let dt = ctx.input(|i| i.stable_dt);
    if state.step_anim(dt) {
        ctx.request_repaint();
    }

    if chosen.is_some() || close {
        construct.switcher_open = false;
    }
    chosen
}

/// Everything one card paints from — bundled so [`card`] stays one argument
/// wide of the mutable state.
struct CardPaint<'a> {
    /// Which surface this card stands for.
    surface: Surface,
    /// The card's grid rect before any flick offset.
    base_rect: egui::Rect,
    /// The full screen rect (for the fly-out distance).
    screen: egui::Rect,
    /// Whether the keyboard selection is on this card.
    selected: bool,
    /// Whether this card is the surface currently in the foreground.
    is_current: bool,
    /// The live snapshot texture for this card, when one exists (Desktop's
    /// VDI frame today; the U29 hook widens this, module doc).
    preview: Option<&'a egui::TextureHandle>,
}

/// Interact + paint one switcher card. Returns the surface on a click.
fn card(ui: &mut egui::Ui, paint: CardPaint<'_>, state: &mut SwitcherState) -> Option<Surface> {
    let ctx = ui.ctx().clone();
    let offset = state.card_offset(paint.surface);
    let rect = paint.base_rect.translate(egui::vec2(0.0, -offset));
    let id = egui::Id::new((SWITCHER_AREA, "card", paint.surface));
    let resp = ui.interact(rect, id, egui::Sense::click_and_drag());

    // Flick-up to close (touch arrives as the synthesized primary pointer).
    if resp.drag_started() {
        state.begin_drag(paint.surface);
    }
    if resp.dragged() {
        state.drag_by(resp.drag_delta().y);
    }
    if resp.drag_stopped() {
        // Fly-out target: the distance at which the card has fully cleared
        // the screen top from its resting place.
        let fly_target = paint.base_rect.bottom() - paint.screen.top();
        state.release_drag(
            CARD_H * FLICK_CLOSE_FRACTION,
            fly_target,
            Motion::reduce_motion(),
        );
    }

    let painter = ui.painter().clone();
    let radius = egui::CornerRadius::same(Style::RADIUS_L as u8);
    let fill = if paint.selected || resp.hovered() {
        Style::SURFACE_HI
    } else {
        Style::SURFACE
    };
    painter.rect_filled(rect, radius, Style::resolve_color(&ctx, fill));
    painter.rect_stroke(
        rect,
        radius,
        if paint.selected {
            // The Q16 highlight rides the platform 2 px focus-ring treatment
            // (selection is lock-step keyboard state, not egui focus).
            Style::focus_stroke()
        } else {
            Style::hairline()
        },
        egui::StrokeKind::Inside,
    );

    // Header band: small glyph + label.
    let header = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), HEADER_H));
    let glyph_edge = HEADER_GLYPH;
    let glyph_rect = egui::Rect::from_center_size(
        egui::pos2(header.left() + Style::SP_M, header.center().y),
        egui::vec2(glyph_edge, glyph_edge),
    );
    let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
    if let Some(tex) = icon_texture(&ctx, paint.surface.icon_id(), glyph_edge, Style::TEXT) {
        painter.image(tex.id(), glyph_rect, uv, egui::Color32::WHITE);
    }
    painter.text(
        egui::pos2(glyph_rect.right() + Style::SP_S, header.center().y),
        egui::Align2::LEFT_CENTER,
        paint.surface.label(),
        egui::FontId::proportional(Style::TYPE_CALLOUT),
        Style::resolve_color(&ctx, Style::TEXT),
    );

    // Preview region: the real snapshot when one exists, else the Q22 plate.
    let preview_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + Style::SP_S, rect.top() + HEADER_H),
        egui::pos2(rect.right() - Style::SP_S, rect.bottom() - Style::SP_S),
    );
    let preview_radius = egui::CornerRadius::same(Style::RADIUS_M as u8);
    if let Some(tex) = paint.preview {
        // Letterbox the live frame on the honest dark ground.
        painter.rect_filled(
            preview_rect,
            preview_radius,
            Style::resolve_color(&ctx, Style::BG),
        );
        let size = tex.size();
        #[allow(clippy::cast_precision_loss)] // texture edges are small ints
        let (tw, th) = (size[0] as f32, size[1] as f32);
        if tw > 0.0 && th > 0.0 {
            let scale = (preview_rect.width() / tw).min(preview_rect.height() / th);
            let draw = egui::Rect::from_center_size(
                preview_rect.center(),
                egui::vec2(tw * scale, th * scale),
            );
            painter.image(tex.id(), draw, uv, egui::Color32::WHITE);
        }
    } else {
        // PLATFORM-INTERFACES Q16/Q22 — the locked fallback plate: the group
        // accent composited by the ONE shared derivation + the white surface
        // glyph. Not a placeholder; the honest no-snapshot rendering.
        let accent = group_accent(paint.surface).unwrap_or(Style::ACCENT);
        painter.rect_filled(
            preview_rect,
            preview_radius,
            Style::resolve_color(&ctx, Style::tile_plate_fill(accent)),
        );
        if let Some(tex) = icon_texture(
            &ctx,
            paint.surface.icon_id(),
            PLATE_GLYPH,
            Style::TILE_GLYPH,
        ) {
            let glyph = egui::Rect::from_center_size(
                preview_rect.center(),
                egui::vec2(PLATE_GLYPH, PLATE_GLYPH),
            );
            painter.image(tex.id(), glyph, uv, egui::Color32::WHITE);
        }
    }

    // The foreground surface's card wears the taskbar's accent underline.
    if paint.is_current {
        let underline = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.bottom() - CURRENT_BAR_H),
            egui::vec2(rect.width(), CURRENT_BAR_H),
        );
        painter.rect_filled(
            underline,
            egui::CornerRadius::ZERO,
            Style::resolve_color(&ctx, Style::ACCENT),
        );
    }

    resp.clicked().then_some(paint.surface)
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

    /// One headless mount frame — the same `Context::run` → `tessellate` path
    /// the DRM runner drives, with the slot's exact call shape.
    fn frame(
        ctx: &egui::Context,
        construct: &mut ConstructChrome,
        current: Surface,
        expanded: bool,
        events: Vec<egui::Event>,
    ) -> (Option<Surface>, egui::FullOutput) {
        let mut routed = None;
        let out = ctx.run(raw(events), |ctx| {
            routed = mount(ctx, construct, current, expanded, None);
        });
        (routed, out)
    }

    fn state_of(ctx: &egui::Context) -> SwitcherState {
        ctx.data_mut(|d| d.get_temp::<SwitcherState>(egui::Id::new(STATE_KEY)))
            .unwrap_or_default()
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

    // --- the recents ring (pure) --------------------------------------------------

    #[test]
    fn ring_promotes_to_front_dedupes_and_caps_at_the_real_surface_count() {
        let mut s = SwitcherState::default();
        s.promote(Surface::Music);
        s.promote(Surface::Files);
        assert_eq!(s.ring(), [Surface::Files, Surface::Music]);
        // Re-entering promotes, never duplicates.
        s.promote(Surface::Music);
        assert_eq!(s.ring(), [Surface::Music, Surface::Files]);
        // Cap: cycling every surface (plus the out-of-picker Timers) can never
        // outgrow the platform's real surface count.
        for surface in Surface::ALL {
            s.promote(surface);
        }
        s.promote(Surface::Timers);
        assert!(s.ring().len() <= Surface::ALL.len());
        let mut dedup = s.ring().to_vec();
        dedup.sort_by_key(|surface| *surface as usize);
        dedup.dedup();
        assert_eq!(dedup.len(), s.ring().len(), "a surface appears once");
    }

    #[test]
    fn ring_remove_forgets_recency_and_clamps_the_selection() {
        let mut s = SwitcherState::default();
        s.promote(Surface::Music);
        s.promote(Surface::Files);
        s.promote(Surface::Terminal);
        s.selected = 2;
        s.remove(Surface::Music); // the selected last card vanishes
        assert_eq!(s.ring(), [Surface::Terminal, Surface::Files]);
        assert_eq!(s.selected, 1, "selection clamps onto a real card");
        s.remove(Surface::Browser); // absent surface: a no-op, no panic
        assert_eq!(s.ring().len(), 2);
    }

    // --- flick-up to close (pure) -------------------------------------------------

    #[test]
    fn flick_past_the_threshold_removes_instantly_under_reduced_motion() {
        let mut s = SwitcherState::default();
        s.promote(Surface::Music);
        s.promote(Surface::Files);
        s.begin_drag(Surface::Music);
        s.drag_by(-(CARD_H * FLICK_CLOSE_FRACTION + 10.0)); // upward past threshold
        s.release_drag(CARD_H * FLICK_CLOSE_FRACTION, 400.0, true);
        assert!(!s.ring().contains(&Surface::Music));
        assert!(s.anim.is_none(), "reduced motion: no fly-out ghost");
    }

    #[test]
    fn flick_past_the_threshold_flies_out_on_the_spring_then_removes() {
        let mut s = SwitcherState::default();
        s.promote(Surface::Music);
        s.promote(Surface::Files);
        s.begin_drag(Surface::Music);
        s.drag_by(-(CARD_H * FLICK_CLOSE_FRACTION + 10.0));
        s.release_drag(CARD_H * FLICK_CLOSE_FRACTION, 400.0, false);
        assert!(
            s.ring().contains(&Surface::Music),
            "the card stays in the ring while the fly-out is in flight"
        );
        let mut frames = 0;
        while s.step_anim(1.0 / 60.0) {
            frames += 1;
            assert!(frames < 600, "the fly-out spring must settle");
        }
        assert!(!s.ring().contains(&Surface::Music), "removal on arrival");
    }

    #[test]
    fn a_sub_threshold_release_springs_back_and_keeps_the_card() {
        let mut s = SwitcherState::default();
        s.promote(Surface::Music);
        s.begin_drag(Surface::Music);
        s.drag_by(-8.0); // a nudge, well under the threshold
        s.release_drag(CARD_H * FLICK_CLOSE_FRACTION, 400.0, false);
        let mut frames = 0;
        while s.step_anim(1.0 / 60.0) {
            frames += 1;
            assert!(frames < 600, "the spring-back must settle");
        }
        assert!(
            s.ring().contains(&Surface::Music),
            "no removal below threshold"
        );
    }

    #[test]
    fn a_downward_drag_never_arms_the_flick() {
        let mut s = SwitcherState::default();
        s.promote(Surface::Music);
        s.begin_drag(Surface::Music);
        s.drag_by(500.0); // downward travel clamps at rest
        s.release_drag(CARD_H * FLICK_CLOSE_FRACTION, 400.0, true);
        assert!(s.ring().contains(&Surface::Music));
    }

    // --- the intent seam ----------------------------------------------------------

    #[test]
    fn the_switcher_intent_toggles_the_open_flag_and_highlights_the_current_surface() {
        let ctx = ctx();
        let mut construct = ConstructChrome::default();
        // Two foreground surfaces build the ring.
        frame(&ctx, &mut construct, Surface::Music, true, Vec::new());
        frame(&ctx, &mut construct, Surface::Files, true, Vec::new());
        let super_tab = ChromeInput {
            super_tap: false,
            super_tab: true,
            app_expanded: true,
            remote_session_focused: false,
            edges: Vec::new(),
            now: Duration::ZERO,
        };
        construct.dispatch(&super_tab);
        frame(&ctx, &mut construct, Surface::Files, true, Vec::new());
        assert!(construct.switcher_open, "Super+Tab opens the grid");
        assert_eq!(
            state_of(&ctx).selected,
            0,
            "the current (front-of-ring) surface opens highlighted"
        );
        construct.dispatch(&super_tab);
        frame(&ctx, &mut construct, Surface::Files, true, Vec::new());
        assert!(!construct.switcher_open, "Super+Tab again closes it");
    }

    // --- the overlay --------------------------------------------------------------

    /// Build a ctx + construct with ring `[Files, Music]`, switcher open, and
    /// TWO overlay settle frames already run: the first is egui's invisible
    /// Area sizing pass (its widgets register non-interactive), the second
    /// registers the real interactive widgets that the next frame's pointer
    /// hit-testing reads (the curtain-test idiom; the live DRM loop repaints
    /// continuously so both frames are long gone before a human can click).
    fn open_switcher() -> (egui::Context, ConstructChrome) {
        let ctx = ctx();
        let mut construct = ConstructChrome::default();
        frame(&ctx, &mut construct, Surface::Music, true, Vec::new());
        frame(&ctx, &mut construct, Surface::Files, true, Vec::new());
        construct.switcher_open = true;
        frame(&ctx, &mut construct, Surface::Files, true, Vec::new());
        frame(&ctx, &mut construct, Surface::Files, true, Vec::new());
        (ctx, construct)
    }

    #[test]
    fn the_open_flag_renders_the_scrim_backdrop_and_a_real_card_grid() {
        let (ctx, mut construct) = open_switcher();
        let (_, out) = frame(&ctx, &mut construct, Surface::Files, true, Vec::new());
        let fills = painted_fills(&out.shapes);
        assert!(
            fills.contains(&Style::SCRIM_REGULAR),
            "the backdrop must be the ONE shared regular scrim material: {fills:?}"
        );
        assert!(
            fills.contains(&Style::SURFACE) || fills.contains(&Style::SURFACE_HI),
            "cards must paint real chrome fills: {fills:?}"
        );
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "the grid must tessellate real geometry");
    }

    #[test]
    fn the_plate_fallback_renders_for_a_surface_with_no_snapshot_texture() {
        let (ctx, mut construct) = open_switcher();
        let (_, out) = frame(&ctx, &mut construct, Surface::Files, true, Vec::new());
        let fills = painted_fills(&out.shapes);
        for surface in [Surface::Music, Surface::Files] {
            let accent = group_accent(surface).unwrap_or(Style::ACCENT);
            assert!(
                fills.contains(&Style::tile_plate_fill(accent)),
                "{surface:?}'s card must show the Q22 accent plate: {fills:?}"
            );
        }
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty());
    }

    #[test]
    fn enter_routes_the_selected_surface_out_and_closes() {
        let (ctx, mut construct) = open_switcher();
        let (routed, _) = frame(
            &ctx,
            &mut construct,
            Surface::Files,
            true,
            vec![key(egui::Key::Enter)],
        );
        assert_eq!(
            routed,
            Some(Surface::Files),
            "Enter picks the highlighted (front-of-ring) card"
        );
        assert!(!construct.switcher_open, "a pick closes the grid");
    }

    #[test]
    fn arrow_keys_move_the_selection_in_lock_step() {
        let (ctx, mut construct) = open_switcher();
        frame(
            &ctx,
            &mut construct,
            Surface::Files,
            true,
            vec![key(egui::Key::ArrowRight)],
        );
        assert_eq!(state_of(&ctx).selected, 1, "ArrowRight walks the grid");
        frame(
            &ctx,
            &mut construct,
            Surface::Files,
            true,
            vec![key(egui::Key::ArrowRight)],
        );
        assert_eq!(state_of(&ctx).selected, 1, "the selection clamps, no wrap");
        frame(
            &ctx,
            &mut construct,
            Surface::Files,
            true,
            vec![key(egui::Key::ArrowLeft)],
        );
        assert_eq!(state_of(&ctx).selected, 0);
    }

    #[test]
    fn a_click_on_a_card_routes_that_surface_out() {
        let (ctx, mut construct) = open_switcher();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN);
        let (_, rects) = grid_layout(screen, 2);
        let target = rects[1].center(); // the second (older, Music) card
        let (_, _) = frame(
            &ctx,
            &mut construct,
            Surface::Files,
            true,
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
        let (routed, _) = frame(
            &ctx,
            &mut construct,
            Surface::Files,
            true,
            vec![egui::Event::PointerButton {
                pos: target,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }],
        );
        assert_eq!(routed, Some(Surface::Music), "the clicked card routes");
        assert!(!construct.switcher_open);
    }

    #[test]
    fn escape_and_a_scrim_click_clear_the_open_flag() {
        // Escape.
        let (ctx, mut construct) = open_switcher();
        let (routed, _) = frame(
            &ctx,
            &mut construct,
            Surface::Files,
            true,
            vec![key(egui::Key::Escape)],
        );
        assert_eq!(routed, None, "Escape routes nothing");
        assert!(!construct.switcher_open, "Escape closes");

        // A click on the scrim (off every card).
        let (ctx, mut construct) = open_switcher();
        let miss = egui::pos2(8.0, 8.0);
        frame(
            &ctx,
            &mut construct,
            Surface::Files,
            true,
            vec![
                egui::Event::PointerMoved(miss),
                egui::Event::PointerButton {
                    pos: miss,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
        let (routed, _) = frame(
            &ctx,
            &mut construct,
            Surface::Files,
            true,
            vec![egui::Event::PointerButton {
                pos: miss,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }],
        );
        assert_eq!(routed, None, "a scrim click routes nothing");
        assert!(!construct.switcher_open, "a scrim click closes");
    }

    #[test]
    fn an_empty_ring_still_renders_the_overlay_honestly() {
        let ctx = ctx();
        let mut construct = ConstructChrome::default();
        construct.switcher_open = true;
        // Never-expanded shell: the ring is empty; the overlay must not panic
        // and must still paint the scrim + the honest empty caption. One
        // settle frame first: egui sizes a fresh Area with an invisible
        // sizing pass (the live DRM loop repaints continuously anyway).
        frame(&ctx, &mut construct, Surface::Desktop, false, Vec::new());
        let (routed, out) = frame(&ctx, &mut construct, Surface::Desktop, false, Vec::new());
        assert_eq!(routed, None);
        let fills = painted_fills(&out.shapes);
        assert!(fills.contains(&Style::SCRIM_REGULAR));
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty());
    }

    #[test]
    fn grid_layout_is_centered_squarish_and_screen_bounded() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN);
        for n in 0..=Surface::ALL.len() {
            let (cols, rects) = grid_layout(screen, n);
            assert!(cols >= 1);
            assert_eq!(rects.len(), n);
            for rect in &rects {
                assert!(rect.left() >= screen.left(), "{n} cards: {rect:?}");
                assert!(rect.right() <= screen.right(), "{n} cards: {rect:?}");
                assert!(rect.top() >= screen.top(), "{n} cards: {rect:?}");
            }
        }
        // One card sits dead-centre horizontally.
        let (_, rects) = grid_layout(screen, 1);
        assert!((rects[0].center().x - screen.center().x).abs() < 0.5);
    }
}
