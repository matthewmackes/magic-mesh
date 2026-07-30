//! `springboard` — WL-UX-012/U10: the **Construct Home gesture layer** over the
//! wallpaper-backed desktop. Home is intentionally quiet; app discovery lives
//! in Spotlight/Front Door while this module owns the pull-down gesture.
//!
// PLATFORM-INTERFACES Q5/Q8/Q22 — the locked home is a wallpaper-backed free
// canvas with no desktop title or app tile labels. The full-width Construct
// taskbar is the launch/search rail; deeper app discovery remains in Spotlight.
//!
//! ## Input
//!
//! * **Pull-down → Spotlight** (Q11 "pull-down on Home"): a downward drag
//!   beginning in Home's upper region past a threshold queues the
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

use mde_egui::egui;
#[cfg(test)]
use mde_egui::Style;

use crate::construct::{ChromeIntent, ConstructChrome};

/// Stable egui-memory key the per-frame [`SpringboardState`] persists under.
const STATE_KEY: &str = "construct-springboard-state";
/// Stable id of the wallpaper-backed Home drag/gesture target.
const SPRINGBOARD_BG: &str = "construct-springboard-bg";

/// Pointer travel (either axis) past which an undecided drag classifies as
/// a gesture (dominant-horizontal) or the Spotlight pull (dominant-vertical).
const DRAG_SLOP: f32 = 8.0;
/// The upper fraction of Home a Spotlight pull must begin in (Q11's
/// "pull-down on home" — a downward drag from the top region, never a stray
/// scroll near the desktop edge).
const PULL_REGION: f32 = 0.4;
/// Downward travel past which an armed pull fires Spotlight, once.
const PULL_FIRE: f32 = 56.0;
/// One routed springboard outcome for the U09 slot body to apply — the whole
/// `main.rs` contract of this unit (a small match, nothing else).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpringboardAction {
    /// The §2.3 Home intent fired: collapse to the base layer.
    Home,
    /// The Q11 on-home pull-down fired: toggle the Spotlight (Front Door).
    Spotlight,
}

/// An in-flight pointer gesture on Home, classified by dominant axis once
/// travel clears [`DRAG_SLOP`].
#[derive(Debug, Clone, Copy, PartialEq)]
enum Gesture {
    /// Travel under the slop — not yet classified.
    Undecided {
        /// Where the drag began, as a fraction of the Home height.
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
        /// Where the drag began, as a fraction of the Home height.
        origin_frac_y: f32,
        /// Accumulated (downward-positive) vertical travel in points.
        dy: f32,
        /// Spotlight already fired for this gesture.
        fired: bool,
    },
}

/// The springboard's whole model: the one in-flight gesture and the action
/// queue the mount slot drains.
/// Pure — every mutation is a plain method, so pull semantics
/// unit-test without a frame loop. Persisted across
/// frames in egui memory (see [`STATE_KEY`]).
#[derive(Debug, Clone)]
pub(crate) struct SpringboardState {
    /// The pointer gesture in flight, if any.
    gesture: Option<Gesture>,
    /// Interactions queued for [`mount`] — drained every frame, so this never
    /// carries more than one frame's input.
    actions: Vec<SpringboardAction>,
}

impl Default for SpringboardState {
    fn default() -> Self {
        Self {
            gesture: None,
            actions: Vec::new(),
        }
    }
}

impl SpringboardState {
    /// A drag began at `origin_frac_y` of the Home height.
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
pub(crate) fn show(ui: &mut egui::Ui, overlay_above: bool) {
    let ctx = ui.ctx().clone();
    let state_key = egui::Id::new(STATE_KEY);
    let mut state = ctx
        .data_mut(|d| d.get_temp::<SpringboardState>(state_key))
        .unwrap_or_default();
    // Front Door/search and the other Construct overlays own the pointer while
    // open.  The Home layer is still mounted underneath them for the cross-fade,
    // but it must not accept a pull-down through the overlay and produce a
    // second, hidden launcher/search transition.
    if overlay_above {
        state.gesture = None;
    } else {
        handle_drag(ui, &mut state, ui.max_rect());
    }

    ctx.data_mut(|d| d.insert_temp(state_key, state));
}

/// The Home gesture target. Horizontal drags are inert because Home has no
/// pages; launch and search belong to the full-width taskbar.
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
                show(ui, overlay_above);
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
    fn production_frame_with_overlay(
        ctx: &egui::Context,
        overlay_above: bool,
        events: Vec<egui::Event>,
    ) -> (egui::Rect, egui::FullOutput) {
        let mut inner = egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN);
        let out = ctx.run(raw(events), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                inner = ui.max_rect();
                show(ui, overlay_above);
            });
        });
        (inner, out)
    }

    fn production_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> (egui::Rect, egui::FullOutput) {
        production_frame_with_overlay(ctx, false, events)
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
        // Low origin: a long downward drag from Home's lower region is inert.
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

    // --- the mount seam -----------------------------------------------------------

    #[test]
    fn mount_routes_the_home_intent_exactly_once() {
        let ctx = ctx();
        let mut construct = ConstructChrome::default();
        assert_eq!(mount(&ctx, &mut construct), None, "a quiet frame is quiet");

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

    #[test]
    fn production_home_overlay_blocks_the_hidden_launcher_pull_gesture() {
        let ctx = ctx();
        let mut construct = ConstructChrome::default();
        let (inner, _) = production_frame(&ctx, Vec::new());
        let start = egui::pos2(inner.center().x, inner.top() + inner.height() * 0.1);
        let pulled = egui::pos2(start.x, start.y + PULL_FIRE + 40.0);

        production_frame_with_overlay(
            &ctx,
            true,
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
        production_frame_with_overlay(&ctx, true, vec![egui::Event::PointerMoved(pulled)]);
        production_frame_with_overlay(
            &ctx,
            true,
            vec![egui::Event::PointerButton {
                pos: pulled,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }],
        );

        assert_eq!(
            mount(&ctx, &mut construct),
            None,
            "Front Door/search must own the pointer; Home cannot fire a hidden second launcher"
        );
    }

    // --- the collapsed base layer paints honestly -----------------------------------

    #[test]
    fn the_production_home_has_no_launcher_titles_or_tile_plates() {
        let ctx = ctx();
        production_frame(&ctx, Vec::new());
        let (_, out) = production_frame(&ctx, Vec::new());
        let texts = painted_texts(&out.shapes);
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
