//! `sheet` — the shared HIG **modality** components: [`Sheet`] and [`Popover`].
//!
//! PLATFORM-INTERFACES Q20: sheets + popovers are THE platform modal idiom —
//! one shared `Sheet` (detents, drag-to-dismiss) and one shared `Popover`
//! (anchored, transient), adopted by every surface, so a dialog learned once is
//! learned everywhere (P3), and modal UI stays rare, purposeful, and
//! dismissible by gesture/Escape (P5).
//!
//! Everything composes the existing substrate rather than re-deriving it (§6):
//!
//! * the scrim is the U04 [`Style::SCRIM_REGULAR`] material — honest alpha, no
//!   blur pass (PLATFORM-INTERFACES Q21);
//! * corners come off the U04 radii ladder — [`Style::RADIUS_XL`] sheet tops,
//!   [`Style::RADIUS_M`] popover bodies (PLATFORM-INTERFACES Q23);
//! * the drag physics are the U05 motion table — [`Motion::detent_target`] for
//!   the release, [`Motion::rubber_band`] for the overscroll,
//!   [`Spring::SHEET`] for the settle, endpoint-only under reduced motion
//!   (PLATFORM-INTERFACES Q24 / a11y-07);
//! * modality itself is the house `egui::Modal` idiom (the same container the
//!   shell's existing dialogs use), which registers the modal layer — egui
//!   refuses focus interest on layers below the top modal layer, so
//!   Tab/Shift-Tab stay trapped inside the open sheet. (The crate has no
//!   focus-restore-on-close idiom yet; that gap is documented here rather than
//!   half-solved.)
//!
//! AccessKit: the crate has no modal-role idiom to match yet (`a11y.rs` is the
//! tree-export bridge only; no module assigns `Role::Dialog`), so the Sheet
//! deliberately does not invent one — the `UiKind::Modal` area it shows through
//! is the hook the eventual a11y role mapping (a11y-02) will hang off.

use egui::{
    pos2, vec2, Area, Color32, Context, CornerRadius, Frame, Id, Margin, Order, Pos2, Rect, Sense,
    Shape, Ui, UiKind, Vec2,
};

use crate::motion::{Motion, MotionMode, MotionPreset, Phase, Spring};
use crate::style::{Elevation, Style};
use crate::widgets::{corner, overlay};

/// PLATFORM-INTERFACES Q20 — the **form-sheet width class**: the fixed body
/// width of the centered form-sheet presentation on wide screens (the HIG
/// form-sheet convention). There is no shared width token to alias (the token
/// modules carry spacing/radius/type scales only), so this is the one named
/// width this module mints — noted in the commit body per the §4 discipline.
pub const FORM_SHEET_W: f32 = 540.0;

/// Popover arrow **height** (anchor-gap depth) — the base spacing token
/// [`Style::SP_S`], so the arrow sits on the same 8px grid as everything else.
pub const ARROW_H: f32 = Style::SP_S;

/// Popover arrow **half-width** — [`Style::SP_S`], giving a 16px base on the
/// spacing grid.
pub const ARROW_HALF_W: f32 = Style::SP_S;

// ─────────────────────────────────────────────────────────────────────────────
// Sheet
// ─────────────────────────────────────────────────────────────────────────────

/// How a [`Sheet`] presents on the current screen (PLATFORM-INTERFACES Q20).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetPresentation {
    /// The full-width **bottom sheet**: slides up from the bottom edge, rounds
    /// only its top corners, and honours every detent.
    Bottom,
    /// The centered **form sheet** on wide screens: a fixed-width card that
    /// slides up to rest at screen center, rounds all corners, and presents at
    /// its tallest detent only (the in-between rungs are a bottom-sheet
    /// affordance).
    Form,
}

/// The presentation a screen of this size gets: the centered form sheet once
/// the full-width bottom sheet would span more than twice the
/// [`FORM_SHEET_W`] width class, the bottom sheet otherwise. Pure.
#[must_use]
pub fn sheet_presentation(screen: Rect) -> SheetPresentation {
    if screen.width() >= FORM_SHEET_W * 2.0 {
        SheetPresentation::Form
    } else {
        SheetPresentation::Bottom
    }
}

/// The sheet's outer rect for an open `fraction` (`0.0` closed … detent
/// fractions of the screen height). Bottom sheets anchor to the bottom edge and
/// grow upward; form sheets keep a fixed body and slide from below the screen
/// (`fraction == 0`) to dead center (`fraction == top_detent`) — a rubber-band
/// overscroll (`fraction > top_detent`) lifts slightly past center. Pure.
#[must_use]
pub fn sheet_rect(
    screen: Rect,
    fraction: f32,
    top_detent: f32,
    presentation: SheetPresentation,
) -> Rect {
    match presentation {
        SheetPresentation::Bottom => {
            let h = (fraction * screen.height()).clamp(0.0, screen.height());
            Rect::from_min_max(
                pos2(screen.left(), screen.bottom() - h),
                screen.right_bottom(),
            )
        }
        SheetPresentation::Form => {
            let w = FORM_SHEET_W.min((screen.width() - 2.0 * Style::SP_XL).max(0.0));
            let h =
                (top_detent * screen.height()).min((screen.height() - 2.0 * Style::SP_XL).max(0.0));
            let progress = if top_detent > f32::EPSILON {
                fraction / top_detent
            } else {
                0.0
            };
            let start_y = screen.bottom() + h * 0.5;
            let center_y = start_y + (screen.center().y - start_y) * progress;
            Rect::from_center_size(pos2(screen.center().x, center_y), vec2(w, h))
        }
    }
}

/// PLATFORM-INTERFACES Q21 — the sheet's scrim: the shared **regular
/// material** ([`Style::SCRIM_REGULAR`]), faded in with presentation progress
/// toward the first detent so the push-back never pops. Pure.
#[must_use]
pub fn sheet_scrim(fraction: f32, first_detent: f32) -> Color32 {
    let t = if first_detent > f32::EPSILON {
        (fraction / first_detent).clamp(0.0, 1.0)
    } else {
        1.0
    };
    Style::SCRIM_REGULAR.gamma_multiply(t)
}

/// Persistent presentation state for one [`Sheet`] — the caller owns it across
/// frames (open fraction, spring velocity, active detent target, drag
/// tracking). All motion advances through [`advance`](Self::advance), which is
/// pure and mode-explicit so the physics are testable without a frame loop
/// (the [`crate::motion::Animated`] convention).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SheetState {
    /// Current visual open fraction (`0.0` closed … `1.0` full height).
    fraction: f32,
    /// Settle-spring velocity, fractions/second.
    vel: f32,
    /// The detent fraction the sheet is resting at / springing toward
    /// (`0.0` while dismissing). Snapped onto a real detent by the show pass.
    target: f32,
    /// Whether the sheet wants to be presented.
    open: bool,
    /// A drag on the grab handle is in flight (the spring is bypassed).
    dragging: bool,
    /// Live drag velocity in fractions/second (positive = opening).
    drag_vel: f32,
}

impl SheetState {
    /// A sheet at rest, fully closed.
    #[must_use]
    pub const fn closed() -> Self {
        Self {
            fraction: 0.0,
            vel: 0.0,
            target: 0.0,
            open: false,
            dragging: false,
            drag_vel: 0.0,
        }
    }

    /// Present the sheet. A fresh open rises to the **lowest** detent; a sheet
    /// already targeting a detent keeps it.
    pub fn open(&mut self) {
        self.open = true;
    }

    /// Present the sheet at (the detent nearest to) `fraction`.
    pub fn open_at(&mut self, fraction: f32) {
        self.open = true;
        self.target = fraction.clamp(0.0, 1.0);
    }

    /// Dismiss the sheet: it springs closed ([`Spring::SHEET`]) and stops
    /// painting once settled. PLATFORM-INTERFACES Q20/P5 — every dismissal path
    /// (Escape, scrim click, downward fling, programmatic) funnels here.
    pub fn dismiss(&mut self) {
        self.open = false;
        self.target = 0.0;
    }

    /// Whether the sheet wants to be presented (it may still be painting its
    /// exit travel when `false` — see [`phase`](Self::phase)).
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Current visual open fraction, `0.0..=1.0` (+ rubber-band overscroll).
    #[must_use]
    pub const fn fraction(&self) -> f32 {
        self.fraction
    }

    /// The detent fraction currently targeted (`0.0` while dismissing).
    #[must_use]
    pub const fn target(&self) -> f32 {
        self.target
    }

    /// Whether a grab-handle drag is in flight.
    #[must_use]
    pub const fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// The presentation lifecycle phase — the shared [`Phase`] model, so an
    /// exiting sheet still paints and still blocks the background
    /// ([`Phase::modal_blocks_background`]).
    #[must_use]
    pub fn phase(&self) -> Phase {
        Phase::resolve(self.open, self.spring_settled())
    }

    /// Whether the sheet paints anything this frame.
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.phase().is_painted()
    }

    /// Advance the settle spring one frame (a no-op while dragging).
    ///
    /// PLATFORM-INTERFACES Q24: the settle is [`Spring::SHEET`] — near-critical,
    /// so the sheet lands on its detent without sailing past it. Under reduced
    /// motion the travel collapses to the endpoint immediately (a11y-07, the
    /// [`Motion::spring_to`] convention). Pure and mode-explicit: the physics
    /// are unit-testable without an egui clock or the process-global mode.
    pub fn advance(&mut self, dt: f32, mode: MotionMode) {
        if self.dragging {
            return;
        }
        if mode.is_reduced() {
            self.fraction = self.target;
            self.vel = 0.0;
            return;
        }
        let (pos, vel) = Spring::SHEET.step(self.fraction, self.vel, self.target, dt);
        if Spring::SHEET.settled(pos, vel, self.target) {
            self.fraction = self.target;
            self.vel = 0.0;
        } else {
            self.fraction = pos;
            self.vel = vel;
        }
    }

    /// Whether the spring has come to rest at its target.
    fn spring_settled(&self) -> bool {
        !self.dragging && Spring::SHEET.settled(self.fraction, self.vel, self.target)
    }
}

/// What one [`Sheet::show`] frame reported.
#[derive(Debug)]
pub struct SheetResponse<R> {
    /// The content closure's return value.
    pub inner: R,
    /// The sheet began dismissing **this frame** (scrim click, Escape, or a
    /// committed downward fling).
    pub dismissed: bool,
    /// The sheet's outer rect this frame.
    pub rect: Rect,
}

/// The shared modal **sheet** (PLATFORM-INTERFACES Q20): a surface sliding up
/// from the bottom edge (or presenting as a centered form sheet on wide
/// screens) that rests at caller-supplied **detents**, follows a drag on its
/// grab handle with rubber-band overscroll, and dismisses by downward fling,
/// scrim click, or Escape.
///
/// ```ignore
/// let sheet = Sheet::new("settings-sheet", &[0.35, 0.9]);
/// if let Some(resp) = sheet.show(ctx, &mut state, |ui| { /* content */ }) {
///     if resp.dismissed { /* … */ }
/// }
/// ```
pub struct Sheet<'a> {
    id: Id,
    detents: &'a [f32],
}

impl<'a> Sheet<'a> {
    /// Build a sheet keyed by a stable `id_source`, resting at `detents` —
    /// ascending fractions of the screen height in `(0.0, 1.0]`
    /// (e.g. `&[0.35, 0.9]`).
    #[must_use]
    pub fn new(id_source: impl std::hash::Hash, detents: &'a [f32]) -> Self {
        Self {
            id: Id::new(id_source),
            detents,
        }
    }

    /// The stable id of the grab-handle strip (exposed for headless tests and
    /// for callers that need `Context::read_response` on it).
    #[must_use]
    pub fn handle_id(&self) -> Id {
        self.id.with("drag-handle")
    }

    /// Show the sheet for this frame. Returns `None` while fully hidden
    /// (closed **and** settled — an exiting sheet still paints and still
    /// blocks the background).
    pub fn show<R>(
        &self,
        ctx: &Context,
        state: &mut SheetState,
        content: impl FnOnce(&mut Ui) -> R,
    ) -> Option<SheetResponse<R>> {
        debug_assert!(
            self.detents
                .iter()
                .all(|d| d.is_finite() && *d > 0.0 && *d <= 1.0),
            "sheet detents are fractions in (0.0, 1.0]"
        );
        debug_assert!(
            self.detents.windows(2).all(|w| w[0] < w[1]),
            "sheet detents must be sorted ascending"
        );

        let screen = ctx.screen_rect();
        let presentation = sheet_presentation(screen);
        // Q20: the centered form sheet presents at its tallest detent only.
        let detents: &[f32] = match presentation {
            SheetPresentation::Bottom => self.detents,
            SheetPresentation::Form => &self.detents[self.detents.len().saturating_sub(1)..],
        };
        let (Some(&first), Some(&top)) = (detents.first(), detents.last()) else {
            // Degenerate: no detents means nowhere to rest — dismiss (the same
            // resolution `Motion::detent_target` gives an empty slice).
            *state = SheetState::closed();
            return None;
        };

        // A fresh `open()` presents at the lowest detent; then keep the resting
        // target on a real detent (this snaps `open_at` picks and self-heals
        // across detent-set / presentation changes between frames).
        if state.open && state.target <= f32::EPSILON {
            state.target = first;
        }
        if state.open && !state.dragging {
            state.target = Motion::detent_target(state.target, 0.0, detents);
        }

        let dt = ctx.input(|i| i.stable_dt);
        state.advance(dt, Motion::mode());
        if !state.phase().is_painted() {
            return None;
        }
        if !state.spring_settled() {
            ctx.request_repaint();
        }

        let rect = sheet_rect(screen, state.fraction, top, presentation);
        let area = Area::new(self.id)
            .kind(UiKind::Modal)
            .sense(Sense::hover())
            .order(Order::Foreground)
            .interactable(true)
            .movable(false)
            .constrain(false)
            .fixed_pos(rect.min);

        // `egui::Modal` is the house modality idiom (the shell's existing
        // dialogs): it paints the scrim, reports the outside-click, and
        // registers the modal layer — egui refuses focus interest on layers
        // below the top modal layer, so Tab/Shift-Tab stay trapped inside the
        // open sheet. There is no focus-restore-on-close idiom in the crate to
        // match yet — documented gap, not half-solved here.
        let mut fling_dismissed = false;
        let modal = egui::Modal::new(self.id)
            .area(area)
            // Q21: the regular scrim material, faded with presentation progress.
            .backdrop_color(sheet_scrim(state.fraction, first))
            .frame(sheet_frame(presentation));
        let shown = modal.show(ctx, |ui| {
            ui.set_clip_rect(rect);
            let inner_size = vec2(
                (rect.width() - 2.0 * Style::SP_M).max(0.0),
                (rect.height() - 2.0 * Style::SP_S).max(0.0),
            );
            ui.set_min_size(inner_size);
            fling_dismissed = self.drag_handle(ui, state, screen.height(), top, detents);
            content(ui)
        });

        let mut dismissed = fling_dismissed;
        // Q20/P5: dismissible by gesture AND Escape — the scrim click and the
        // (top-modal-only, consumed) Escape both close.
        if shown.should_close() {
            state.dismiss();
            dismissed = true;
        }
        Some(SheetResponse {
            inner: shown.inner,
            dismissed,
            rect,
        })
    }

    /// The grab-handle strip: a full-width, [`crate::style::Density`]-sized drag
    /// target with the capsule grabber, plus the drag physics. Returns `true`
    /// when a committed downward fling dismissed the sheet this frame.
    fn drag_handle(
        &self,
        ui: &mut Ui,
        state: &mut SheetState,
        avail_h: f32,
        top: f32,
        detents: &[f32],
    ) -> bool {
        let density = Style::density(ui.ctx());
        let (strip, _) = ui.allocate_exact_size(
            vec2(ui.available_width(), density.min_hit_target()),
            Sense::hover(),
        );
        let resp = ui.interact(strip, self.handle_id(), Sense::drag());
        // The capsule grabber, centered in the touch strip: SP_XL × SP_XS on
        // the spacing grid, fully rounded.
        let capsule = Rect::from_center_size(strip.center(), vec2(Style::SP_XL, Style::SP_XS));
        ui.painter()
            .rect_filled(capsule, corner(Style::SP_XS), Style::TEXT_DIM);

        let avail_h = avail_h.max(f32::EPSILON);
        let dt = ui.input(|i| i.stable_dt).max(f32::EPSILON);
        if resp.dragged() {
            state.dragging = true;
            let dy = resp.drag_delta().y;
            if dy.abs() > f32::EPSILON {
                // Q24: follow the finger 1:1, rubber-banding the overscroll
                // above the top detent (and below fully-closed) in the px
                // domain the RUBBER_SLACK token lives in.
                let dfrac = -dy / avail_h;
                let raw_px = (state.fraction + dfrac) * avail_h;
                state.fraction = Motion::rubber_band(raw_px, 0.0, top * avail_h) / avail_h;
                state.drag_vel = dfrac / dt;
            } else {
                // A held-still finger sheds its momentum, so pause-then-release
                // reads as a slow release (nearest detent), never a stale fling.
                state.drag_vel = Motion::inertial_decay(state.drag_vel, dt);
            }
        }
        if resp.drag_stopped() {
            state.dragging = false;
            // Q24: the release settles on the detent the fling picks — `0.0`
            // is the drag-to-dismiss resolution.
            let target = Motion::detent_target(state.fraction, state.drag_vel, detents);
            state.vel = state.drag_vel; // carry the release momentum into the spring
            state.drag_vel = 0.0;
            state.target = target;
            if target <= f32::EPSILON {
                state.open = false;
                return true;
            }
        }
        false
    }
}

/// The sheet body frame: base surface fill, hairline border, the deepest
/// (modal) elevation shadow, and Q23 [`Style::RADIUS_XL`] corners — top-only
/// for the bottom sheet (it grows out of the screen edge), all four for the
/// floating form sheet.
fn sheet_frame(presentation: SheetPresentation) -> Frame {
    Frame::NONE
        .fill(Style::SURFACE)
        .stroke(Style::hairline())
        .corner_radius(sheet_corner(presentation))
        .inner_margin(sheet_margin())
        .shadow(Elevation::Modal.egui_shadow())
}

/// Q23 — the sheet corner geometry off the U04 radii ladder.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn sheet_corner(presentation: SheetPresentation) -> CornerRadius {
    let xl = Style::RADIUS_XL as u8;
    match presentation {
        SheetPresentation::Bottom => CornerRadius {
            nw: xl,
            ne: xl,
            sw: 0,
            se: 0,
        },
        SheetPresentation::Form => CornerRadius::same(xl),
    }
}

/// The sheet's inner padding on the spacing grid.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn sheet_margin() -> Margin {
    Margin::symmetric(Style::SP_M as i8, Style::SP_S as i8)
}

// ─────────────────────────────────────────────────────────────────────────────
// Popover
// ─────────────────────────────────────────────────────────────────────────────

/// Which side of its anchor a popover opens on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopoverSide {
    /// Below the anchor (the preferred side).
    Below,
    /// Above the anchor (the flip when the bottom edge would clip).
    Above,
}

/// Where a popover lands relative to its anchor — the pure placement result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PopoverPlacement {
    /// The chosen side.
    pub side: PopoverSide,
    /// The popover body rect (frame outer bounds).
    pub rect: Rect,
    /// The arrow's point, touching the anchor-facing edge of the gap.
    pub arrow_tip: Pos2,
}

/// Place a popover of `size` against `anchor` within `screen`: prefer below,
/// flip above when the bottom edge would clip, clamp inside the screen with a
/// [`Style::SP_S`] margin, and aim the arrow tip at the anchor's center
/// (held clear of the Q23 rounded corners). Pure (PLATFORM-INTERFACES Q20).
#[must_use]
pub fn popover_placement(anchor: Rect, size: Vec2, screen: Rect) -> PopoverPlacement {
    let margin = Style::SP_S;
    let below_y = anchor.bottom() + ARROW_H;
    let above_y = anchor.top() - ARROW_H - size.y;
    let (side, y) = if below_y + size.y <= screen.bottom() - margin {
        (PopoverSide::Below, below_y)
    } else if above_y >= screen.top() + margin {
        (PopoverSide::Above, above_y)
    } else {
        // Neither side fits whole: stay below, pulled up inside the screen.
        (
            PopoverSide::Below,
            (screen.bottom() - margin - size.y).max(screen.top() + margin),
        )
    };
    let min_x = screen.left() + margin;
    let x = (anchor.center().x - size.x * 0.5)
        .clamp(min_x, (screen.right() - margin - size.x).max(min_x));
    let rect = Rect::from_min_size(pos2(x, y), size);
    let arrow_tip = match side {
        PopoverSide::Below => pos2(arrow_x(rect, anchor), rect.top() - ARROW_H),
        PopoverSide::Above => pos2(arrow_x(rect, anchor), rect.bottom() + ARROW_H),
    };
    PopoverPlacement {
        side,
        rect,
        arrow_tip,
    }
}

/// The arrow tip's x: the anchor's center, held clear of the popover's
/// rounded corners (Q23) so the arrow never breaks the corner curvature.
fn arrow_x(body: Rect, anchor: Rect) -> f32 {
    let lo = body.left() + Style::RADIUS_M + ARROW_HALF_W;
    let hi = body.right() - Style::RADIUS_M - ARROW_HALF_W;
    if lo <= hi {
        anchor.center().x.clamp(lo, hi)
    } else {
        body.center().x
    }
}

/// The shared anchored **popover** (PLATFORM-INTERFACES Q20): a transient
/// container placed against an anchor rect with an arrow tip toward it —
/// [`Style::RADIUS_M`] body, the Overlay elevation shadow, **no scrim** (it is
/// non-modal), dismissed by an outside press or Escape. Pointer-first, but the
/// paddings and the minimum body size scale with the installed
/// [`crate::style::Density`] so it stays touch-friendly.
pub struct Popover {
    id: Id,
}

impl Popover {
    /// Build a popover keyed by a stable `id_source`.
    #[must_use]
    pub fn new(id_source: impl std::hash::Hash) -> Self {
        Self {
            id: Id::new(id_source),
        }
    }

    /// Show the popover while `*open`. Returns the content's value while it
    /// paints (including the short exit fade), `None` once fully hidden.
    ///
    /// The caller owns `open` (its anchor widget toggles it); a press outside
    /// the popover — except on the anchor itself, which is the caller's
    /// toggle — or an Escape sets it `false` (Q20/P5: dismissible by
    /// gesture/Escape).
    pub fn show<R>(
        &self,
        ctx: &Context,
        open: &mut bool,
        anchor: Rect,
        content: impl FnOnce(&mut Ui) -> R,
    ) -> Option<R> {
        // Q20: transient — the popover fades on the Popover motion preset,
        // endpoint-only under reduced motion (the mode-aware shared carrier).
        let t = Motion::animate_scalar(
            ctx,
            self.id.with("fade"),
            if *open { 1.0 } else { 0.0 },
            MotionPreset::Popover,
        )
        .value();
        if !*open && t <= f32::EPSILON {
            return None;
        }

        let density = Style::density(ctx);
        let hit = density.min_hit_target();
        let screen = ctx.screen_rect();
        // Standard egui two-pass sizing: place with last frame's measured size
        // (the Density hit target until first measured).
        let size = ctx
            .data(|d| d.get_temp::<Vec2>(self.size_id()))
            .unwrap_or(Vec2::splat(hit));
        let placement = popover_placement(anchor, size, screen);

        #[allow(clippy::cast_possible_truncation)]
        let margin = Margin::same((Style::SP_S * density.spacing_scale()) as i8);
        let out = Area::new(self.id)
            .kind(UiKind::Popup)
            .order(Order::Foreground)
            .interactable(true)
            .movable(false)
            .constrain(false)
            .fixed_pos(placement.rect.min)
            .show(ctx, |ui| {
                ui.set_opacity(t);
                // The overlay surface primitive: SURFACE fill, hairline,
                // RADIUS_M, Overlay shadow — margins rescaled for the density.
                let body = overlay().inner_margin(margin).show(ui, |ui| {
                    ui.set_min_size(Vec2::splat(hit));
                    content(ui)
                });
                paint_arrow(ui.painter(), body.response.rect, placement.side, anchor);
                body.inner
            });
        ctx.data_mut(|d| d.insert_temp(self.size_id(), out.response.rect.size()));

        // Dismiss on an outside press / Escape (Q20/P5). A press on the anchor
        // is the caller's toggle — excluded so it doesn't close-then-reopen.
        let pressed_outside = ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !out.response.rect.contains(p) && !anchor.contains(p))
        });
        if *open
            && (pressed_outside
                || ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)))
        {
            *open = false;
        }
        Some(out.inner)
    }

    /// Where the previous frame's measured body size is remembered.
    fn size_id(&self) -> Id {
        self.id.with("size")
    }
}

/// Paint the popover's arrow tip toward the anchor: a [`Style::SURFACE`]-filled
/// triangle whose base overlaps the body's hairline (so the border never draws
/// across the arrow), with the two exposed edges wearing the shared hairline.
fn paint_arrow(painter: &egui::Painter, body: Rect, side: PopoverSide, anchor: Rect) {
    let x = arrow_x(body, anchor);
    let (base_y, tip_y) = match side {
        PopoverSide::Below => (
            body.top() + Style::STROKE_HAIRLINE,
            body.top() + Style::STROKE_HAIRLINE - ARROW_H,
        ),
        PopoverSide::Above => (
            body.bottom() - Style::STROKE_HAIRLINE,
            body.bottom() - Style::STROKE_HAIRLINE + ARROW_H,
        ),
    };
    let left = pos2(x - ARROW_HALF_W, base_y);
    let tip = pos2(x, tip_y);
    let right = pos2(x + ARROW_HALF_W, base_y);
    painter.add(Shape::convex_polygon(
        vec![left, tip, right],
        Style::SURFACE,
        egui::Stroke::NONE,
    ));
    painter.line_segment([left, tip], Style::hairline());
    painter.line_segment([tip, right], Style::hairline());
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::motion::MotionMode;

    const DT: f64 = 1.0 / 60.0;
    /// A screen narrow enough for the bottom-sheet presentation (< 2×540).
    const SHEET_SCREEN: Vec2 = Vec2::new(800.0, 600.0);
    /// A screen wide enough for the form-sheet presentation (≥ 2×540).
    const WIDE_SCREEN: Vec2 = Vec2::new(1280.0, 720.0);
    const DETENTS: [f32; 2] = [0.35, 0.9];

    fn run_frame(
        ctx: &egui::Context,
        size: Vec2,
        t: f64,
        events: Vec<egui::Event>,
        mut ui_fn: impl FnMut(&egui::Context),
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            time: Some(t),
            events,
            ..Default::default()
        };
        ctx.run(input, |ctx| ui_fn(ctx))
    }

    fn press_events(pos: Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
        ]
    }

    fn release_events(pos: Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ]
    }

    fn escape_event() -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]
    }

    /// One sheet frame through the full public API.
    fn sheet_frame_at(
        ctx: &egui::Context,
        t: f64,
        events: Vec<egui::Event>,
        state: &mut SheetState,
    ) -> bool {
        let mut shown = false;
        run_frame(ctx, SHEET_SCREEN, t, events, |ctx| {
            shown = Sheet::new("t-sheet", &DETENTS)
                .show(ctx, state, |ui| {
                    ui.label("sheet body");
                })
                .is_some();
        });
        shown
    }

    /// Run idle frames until the spring settles.
    fn settle(ctx: &egui::Context, t: &mut f64, state: &mut SheetState) {
        for _ in 0..240 {
            *t += DT;
            sheet_frame_at(ctx, *t, vec![], state);
        }
    }

    // ── pure geometry / material ─────────────────────────────────────────────

    #[test]
    fn presentation_switches_to_the_form_sheet_on_wide_screens() {
        // PLATFORM-INTERFACES Q20: bottom sheet on narrow, form sheet on wide.
        let narrow = Rect::from_min_size(Pos2::ZERO, SHEET_SCREEN);
        let wide = Rect::from_min_size(Pos2::ZERO, WIDE_SCREEN);
        assert_eq!(sheet_presentation(narrow), SheetPresentation::Bottom);
        assert_eq!(sheet_presentation(wide), SheetPresentation::Form);
        let boundary = Rect::from_min_size(Pos2::ZERO, vec2(FORM_SHEET_W * 2.0, 600.0));
        assert_eq!(sheet_presentation(boundary), SheetPresentation::Form);
    }

    #[test]
    fn sheet_rect_bottom_anchors_and_form_centers_at_full_presentation() {
        let screen = Rect::from_min_size(Pos2::ZERO, SHEET_SCREEN);
        let r = sheet_rect(screen, 0.35, 0.9, SheetPresentation::Bottom);
        assert_eq!(r.bottom(), screen.bottom(), "bottom sheet hugs the edge");
        assert_eq!(r.width(), screen.width(), "bottom sheet spans full width");
        assert!((r.height() - 0.35 * screen.height()).abs() < 0.01);

        let wide = Rect::from_min_size(Pos2::ZERO, WIDE_SCREEN);
        let full = sheet_rect(wide, 0.9, 0.9, SheetPresentation::Form);
        assert!(
            (full.center() - wide.center()).length() < 0.5,
            "a fully-presented form sheet rests at screen center: {full:?}"
        );
        assert!(full.width() <= FORM_SHEET_W);
        let hidden = sheet_rect(wide, 0.0, 0.9, SheetPresentation::Form);
        assert!(
            hidden.top() >= wide.bottom(),
            "an unpresented form sheet sits below the screen: {hidden:?}"
        );
    }

    #[test]
    fn sheet_scrim_is_the_regular_material_faded_by_presentation() {
        // PLATFORM-INTERFACES Q21: the shared regular material, never a
        // locally-minted alpha.
        assert_eq!(sheet_scrim(0.35, 0.35), Style::SCRIM_REGULAR);
        assert_eq!(sheet_scrim(0.9, 0.35), Style::SCRIM_REGULAR);
        assert_eq!(sheet_scrim(0.0, 0.35), Color32::TRANSPARENT);
        let mid = sheet_scrim(0.175, 0.35);
        assert!(mid.a() > 0 && mid.a() < Style::SCRIM_REGULAR.a());
    }

    // ── state / motion ───────────────────────────────────────────────────────

    #[test]
    fn sheet_state_springs_to_its_detent_and_reduced_motion_is_instant() {
        let mut s = SheetState::closed();
        assert_eq!(s.phase(), Phase::Hidden);
        s.open_at(0.35);
        for _ in 0..300 {
            s.advance(1.0 / 60.0, MotionMode::Normal);
        }
        assert!((s.fraction() - 0.35).abs() < 0.01, "{}", s.fraction());
        assert_eq!(s.phase(), Phase::Visible);

        // PLATFORM-INTERFACES Q24 / a11y-07: reduced motion is endpoint-only.
        let mut r = SheetState::closed();
        r.open_at(0.9);
        r.advance(1.0 / 60.0, MotionMode::Reduced);
        assert_eq!(r.fraction(), 0.9, "reduced motion lands at once");
        r.dismiss();
        r.advance(1.0 / 60.0, MotionMode::Disabled);
        assert_eq!(r.phase(), Phase::Hidden);
        assert!(
            Phase::Exiting.modal_blocks_background(),
            "an exiting sheet still blocks the background"
        );
    }

    // ── integration: drag physics through the public API ────────────────────

    #[test]
    fn sheet_slow_drag_release_settles_on_the_nearest_detent() {
        // PLATFORM-INTERFACES Q24: a release below DETENT_FLING snaps nearest.
        let ctx = egui::Context::default();
        let mut state = SheetState::closed();
        state.open();
        let mut t = 0.0;
        settle(&ctx, &mut t, &mut state);
        assert!(
            (state.fraction() - 0.35).abs() < 0.02,
            "opens at the lowest detent: {}",
            state.fraction()
        );

        let handle = ctx
            .read_response(Sheet::new("t-sheet", &DETENTS).handle_id())
            .expect("grab handle registered")
            .rect;
        let mut pos = handle.center();
        t += DT;
        sheet_frame_at(&ctx, t, press_events(pos), &mut state);
        // Drag up slowly (3 px/frame at 600 px ⇒ 0.3 fractions/s < DETENT_FLING)
        // from 0.35 toward ~0.7 — nearer the 0.9 detent than the 0.35 one.
        for _ in 0..70 {
            pos.y -= 3.0;
            t += DT;
            sheet_frame_at(&ctx, t, vec![egui::Event::PointerMoved(pos)], &mut state);
        }
        assert!(state.is_dragging(), "the handle drag is live");
        t += DT;
        sheet_frame_at(&ctx, t, release_events(pos), &mut state);
        settle(&ctx, &mut t, &mut state);
        assert!(
            (state.fraction() - 0.9).abs() < 0.02,
            "a slow release settles on the NEAREST detent: {}",
            state.fraction()
        );
        assert!(state.is_open());
    }

    #[test]
    fn sheet_fast_downward_fling_dismisses() {
        // PLATFORM-INTERFACES Q24: a committed downward fling below the lowest
        // detent is the drag-to-dismiss gesture.
        let ctx = egui::Context::default();
        let mut state = SheetState::closed();
        state.open();
        let mut t = 0.0;
        settle(&ctx, &mut t, &mut state);

        let handle = ctx
            .read_response(Sheet::new("t-sheet", &DETENTS).handle_id())
            .expect("grab handle registered")
            .rect;
        let mut pos = handle.center();
        t += DT;
        sheet_frame_at(&ctx, t, press_events(pos), &mut state);
        // One fast downward move: 40 px in one 60 Hz frame ⇒ −4 fractions/s.
        pos.y += 40.0;
        t += DT;
        sheet_frame_at(&ctx, t, vec![egui::Event::PointerMoved(pos)], &mut state);
        t += DT;
        sheet_frame_at(&ctx, t, release_events(pos), &mut state);
        assert!(!state.is_open(), "the fling commits the dismissal");

        settle(&ctx, &mut t, &mut state);
        t += DT;
        let shown = sheet_frame_at(&ctx, t, vec![], &mut state);
        assert!(!shown, "a dismissed sheet stops painting once settled");
        assert!(state.fraction() < 0.01, "{}", state.fraction());
    }

    #[test]
    fn sheet_scrim_click_closes() {
        // PLATFORM-INTERFACES Q20/P5: dismissible by gesture — the scrim click.
        let ctx = egui::Context::default();
        let mut state = SheetState::closed();
        state.open();
        let mut t = 0.0;
        settle(&ctx, &mut t, &mut state);
        assert!(state.is_open());

        // Click well above the sheet (sheet top ≈ 390 at the 0.35 detent).
        let outside = pos2(400.0, 50.0);
        t += DT;
        sheet_frame_at(&ctx, t, press_events(outside), &mut state);
        t += DT;
        sheet_frame_at(&ctx, t, release_events(outside), &mut state);
        assert!(!state.is_open(), "a scrim click dismisses the sheet");
    }

    #[test]
    fn sheet_escape_dismisses() {
        // PLATFORM-INTERFACES Q20/P5: dismissible by Escape.
        let ctx = egui::Context::default();
        let mut state = SheetState::closed();
        state.open();
        let mut t = 0.0;
        settle(&ctx, &mut t, &mut state);
        assert!(state.is_open());

        t += DT;
        sheet_frame_at(&ctx, t, escape_event(), &mut state);
        assert!(!state.is_open(), "Escape dismisses the sheet");
    }

    #[test]
    fn sheet_tessellates_nonempty_while_open() {
        let ctx = egui::Context::default();
        let mut state = SheetState::closed();
        state.open();
        let mut t = 0.0;
        let mut out = None;
        for _ in 0..10 {
            t += DT;
            out = Some(run_frame(&ctx, SHEET_SCREEN, t, vec![], |ctx| {
                Sheet::new("t-sheet", &DETENTS).show(ctx, &mut state, |ui| {
                    ui.label("sheet body");
                });
            }));
        }
        let out = out.expect("frames ran");
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "an open sheet paints real primitives");
    }

    // ── popover: pure placement ──────────────────────────────────────────────

    #[test]
    fn popover_prefers_below_and_points_its_arrow_at_the_anchor() {
        // PLATFORM-INTERFACES Q20: below-first placement, arrow toward anchor.
        let screen = Rect::from_min_size(Pos2::ZERO, WIDE_SCREEN);
        let anchor = Rect::from_min_size(pos2(600.0, 100.0), vec2(48.0, 24.0));
        let p = popover_placement(anchor, vec2(200.0, 120.0), screen);
        assert_eq!(p.side, PopoverSide::Below);
        assert_eq!(p.rect.top(), anchor.bottom() + ARROW_H);
        assert_eq!(p.arrow_tip.x, anchor.center().x);
        assert_eq!(
            p.arrow_tip.y,
            anchor.bottom(),
            "the arrow tip touches the anchor's bottom edge"
        );
    }

    #[test]
    fn popover_flips_above_at_the_bottom_edge() {
        let screen = Rect::from_min_size(Pos2::ZERO, WIDE_SCREEN);
        let anchor = Rect::from_min_size(pos2(600.0, 680.0), vec2(48.0, 24.0));
        let p = popover_placement(anchor, vec2(200.0, 120.0), screen);
        assert_eq!(p.side, PopoverSide::Above, "clipped below ⇒ flips above");
        assert_eq!(p.rect.bottom(), anchor.top() - ARROW_H);
        assert_eq!(p.arrow_tip.y, anchor.top());
        assert!(p.rect.bottom() <= screen.bottom());
    }

    #[test]
    fn popover_placement_clamps_inside_the_screen() {
        let screen = Rect::from_min_size(Pos2::ZERO, WIDE_SCREEN);
        // An anchor hugging the left edge: the body clamps in, the arrow stays
        // clear of the rounded corner (Q23) while reaching toward the anchor.
        let anchor = Rect::from_min_size(pos2(2.0, 100.0), vec2(24.0, 24.0));
        let p = popover_placement(anchor, vec2(200.0, 120.0), screen);
        assert!(p.rect.left() >= screen.left() + Style::SP_S);
        assert!(p.arrow_tip.x >= p.rect.left() + Style::RADIUS_M + ARROW_HALF_W - 0.01);
    }

    // ── popover: behavior through show ───────────────────────────────────────

    #[test]
    fn popover_shows_paints_and_escape_dismisses() {
        let ctx = egui::Context::default();
        let pop = Popover::new("t-pop");
        let anchor = Rect::from_min_size(pos2(600.0, 100.0), vec2(48.0, 24.0));
        let mut open = true;
        let mut t = 0.0;
        let mut shown = false;

        t += DT;
        run_frame(&ctx, WIDE_SCREEN, t, vec![], |ctx| {
            shown = pop
                .show(ctx, &mut open, anchor, |ui| {
                    ui.label("transient choice");
                })
                .is_some();
        });
        assert!(shown && open, "an open popover paints and stays open idle");
        // The first frame is egui's invisible Area sizing pass — assert real
        // primitives on the second, placed frame.
        t += DT;
        let out = run_frame(&ctx, WIDE_SCREEN, t, vec![], |ctx| {
            shown = pop
                .show(ctx, &mut open, anchor, |ui| {
                    ui.label("transient choice");
                })
                .is_some();
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "the popover paints real primitives");

        // PLATFORM-INTERFACES Q20/P5: Escape dismisses.
        t += DT;
        run_frame(&ctx, WIDE_SCREEN, t, escape_event(), |ctx| {
            let _ = pop.show(ctx, &mut open, anchor, |ui| {
                ui.label("transient choice");
            });
        });
        assert!(!open, "Escape dismisses the popover");

        // The exit fade ends and the popover stops painting.
        for _ in 0..60 {
            t += DT;
            run_frame(&ctx, WIDE_SCREEN, t, vec![], |ctx| {
                shown = pop
                    .show(ctx, &mut open, anchor, |ui| {
                        ui.label("transient choice");
                    })
                    .is_some();
            });
        }
        assert!(!shown, "a dismissed popover stops painting after its fade");
    }

    #[test]
    fn popover_outside_press_closes_but_anchor_press_does_not() {
        let ctx = egui::Context::default();
        let pop = Popover::new("t-pop2");
        let anchor = Rect::from_min_size(pos2(600.0, 100.0), vec2(48.0, 24.0));
        let mut open = true;
        let mut t = 0.0;
        let show = |ctx: &egui::Context, open: &mut bool| {
            let _ = pop.show(ctx, open, anchor, |ui| {
                ui.label("transient choice");
            });
        };

        t += DT;
        run_frame(&ctx, WIDE_SCREEN, t, vec![], |ctx| show(ctx, &mut open));

        // A press on the anchor is the caller's toggle — the popover must NOT
        // self-close (it would close-then-reopen on every toggle click).
        t += DT;
        run_frame(&ctx, WIDE_SCREEN, t, press_events(anchor.center()), |ctx| {
            show(ctx, &mut open);
        });
        assert!(open, "an anchor press is left to the caller's toggle");
        t += DT;
        run_frame(
            &ctx,
            WIDE_SCREEN,
            t,
            release_events(anchor.center()),
            |ctx| {
                show(ctx, &mut open);
            },
        );

        // A press anywhere else dismisses at once (Q20: transient).
        t += DT;
        run_frame(
            &ctx,
            WIDE_SCREEN,
            t,
            press_events(pos2(30.0, 600.0)),
            |ctx| {
                show(ctx, &mut open);
            },
        );
        assert!(!open, "an outside press dismisses the popover");
    }
}
