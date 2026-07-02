//! `gestures` — SURFACE-11: touch **gesture recognition** folded from the multitouch
//! contact stream (design `docs/design/surface-tablet-enablement.md`, lock 16).
//!
//! SURFACE-8 turned the DRM seat's raw digitizer into a stream of [`RawContact`]s and a
//! per-contact [`egui::Event::Touch`], keeping **every** contact so 2+-finger gestures
//! survive intact. This module is the pure, headless-testable recognizer that folds
//! that same contact stream into higher-level [`Gesture`]s — it does **not** rebuild the
//! multitouch pipeline (§6), it consumes it:
//!
//! * **two-finger scroll** — the centroid of two contacts moving → a scroll delta
//!   ([`Gesture::Scroll`], fed to egui as [`egui::Event::MouseWheel`]);
//! * **pinch-zoom** — the changing spread between two contacts → a zoom factor
//!   ([`Gesture::Zoom`], fed to egui as [`egui::Event::Zoom`]);
//! * **long-press** — a stationary single finger held past a dwell → a secondary
//!   (right) click ([`Gesture::SecondaryClick`], synthesized as an egui secondary
//!   [`egui::Event::PointerButton`] press+release);
//! * **edge-swipe** — a single finger that begins in a screen-edge zone and travels
//!   inward far enough ([`Gesture::EdgeSwipe`]), exposed to the shell on the
//!   [side channel](push_edge_swipe) so a swipe-from-edge can reveal the dock / tablet
//!   bar. Same seat→shell thread-local idiom as [`crate::formfactor`] (§6: this shared
//!   crate never touches the Bus).
//!
//! The recognizer works in **logical egui points** — the seat feeds each
//! [`RawContact`] together with the active [`TouchTransform`] (the very transform
//! SURFACE-8's [`TouchTranslator`] uses), so a gesture is measured in the same space the
//! UI lays out in and rotates/scales with the display. The one time-dependent gesture
//! (long-press) is driven by [`GestureRecognizer::tick`], which the present loop calls
//! each frame so a held-still finger fires without any new contact event.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::Duration;

use crate::touch::{RawContact, TouchTransform};

/// A screen edge a swipe can originate from — the reveal affordance for the dock /
/// tablet bar (a left/bottom swipe raises the shell; SURFACE-11 lock 16).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// The left screen edge.
    Left,
    /// The right screen edge.
    Right,
    /// The top screen edge.
    Top,
    /// The bottom screen edge.
    Bottom,
}

/// A gesture recognized from the multitouch contact stream. Each maps to an egui input
/// event (scroll / zoom / secondary click) or, for the edge-swipe, to a shell signal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gesture {
    /// Two-finger pan — a scroll delta in logical points (the two-contact centroid's
    /// movement since the last frame). Fed to egui as [`egui::Event::MouseWheel`].
    Scroll(egui::Vec2),
    /// Pinch — a zoom factor: the ratio of the current to the previous finger spread
    /// (`> 1.0` spreads apart / zooms in). Fed to egui as [`egui::Event::Zoom`].
    Zoom(f32),
    /// A stationary single-finger long-press → a secondary (right) click at `pos`
    /// (points). Synthesized as an egui secondary [`egui::Event::PointerButton`].
    SecondaryClick(egui::Pos2),
    /// A single finger that began at a screen edge and travelled inward past the
    /// threshold — the dock / tablet-bar reveal, raised as a shell signal.
    EdgeSwipe(Edge),
}

/// Tunable thresholds for the recognizer. Defaults match common touch-platform feel
/// (≈500 ms long-press, a modest finger-slop, a ~24 pt edge zone).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GestureConfig {
    /// How long a single finger must dwell (within [`long_press_slop`](Self::long_press_slop))
    /// before it becomes a secondary click.
    pub long_press: Duration,
    /// Max movement (points) a finger may drift and still count as a long-press rather
    /// than a drag.
    pub long_press_slop: f32,
    /// How close to a screen edge (points) a contact must start to arm an edge-swipe.
    pub edge_zone: f32,
    /// How far inward (points) an edge contact must travel to fire the edge-swipe.
    pub edge_min_travel: f32,
}

impl Default for GestureConfig {
    fn default() -> Self {
        Self {
            long_press: Duration::from_millis(500),
            long_press_slop: 12.0,
            edge_zone: 24.0,
            edge_min_travel: 48.0,
        }
    }
}

/// A single tracked contact: where it went down, where it is now, and when it landed.
#[derive(Debug, Clone, Copy)]
struct Contact {
    down: egui::Pos2,
    cur: egui::Pos2,
    down_at: Duration,
}

/// Below this finger spread (points) a pinch ratio is not computed (avoids a divide by
/// a near-zero baseline when two contacts sit on top of each other).
const MIN_SPREAD: f32 = 1.0;
/// A zoom factor within this of `1.0` is dropped as jitter (no real pinch).
const ZOOM_DEADBAND: f32 = 0.001;

/// Stateful recognizer folding [`RawContact`]s → [`Gesture`]s (design lock 16).
///
/// It tracks the live contacts in logical points and remembers the two-finger centroid
/// and spread frame-to-frame so a pan/pinch is a delta, and the single-finger
/// down-position + time so a long-press / edge-swipe can be decided. It shares the
/// SURFACE-8 [`TouchTransform`] rather than re-deriving coordinates (§6).
#[derive(Debug, Clone)]
pub struct GestureRecognizer {
    cfg: GestureConfig,
    slots: BTreeMap<u32, Contact>,
    /// The two-finger centroid last frame — the baseline a pan delta is measured from.
    last_centroid: Option<egui::Pos2>,
    /// The two-finger spread (distance) last frame — the baseline a pinch ratio uses.
    last_spread: Option<f32>,
    /// The current single-finger gesture already fired a long-press (don't repeat).
    long_press_fired: bool,
    /// The current single-finger gesture already fired an edge-swipe (don't repeat),
    /// and — when a finger lingers after a multitouch — a guard against a spurious
    /// long-press from the leftover contact.
    edge_swipe_fired: bool,
}

impl GestureRecognizer {
    /// Build a recognizer with explicit thresholds.
    #[must_use]
    pub const fn new(cfg: GestureConfig) -> Self {
        Self {
            cfg,
            slots: BTreeMap::new(),
            last_centroid: None,
            last_spread: None,
            long_press_fired: false,
            edge_swipe_fired: false,
        }
    }

    /// Fold one raw contact into the recognizer, appending any gestures it completes.
    ///
    /// `transform` is the same SURFACE-8 [`TouchTransform`] the [`crate::touch::TouchTranslator`]
    /// uses, so contacts are measured in logical points and edges track the (possibly
    /// rotated/scaled) screen. `now` is the seat's monotonic clock (for long-press).
    pub fn feed(
        &mut self,
        contact: RawContact,
        transform: &TouchTransform,
        now: Duration,
        out: &mut Vec<Gesture>,
    ) {
        match contact {
            RawContact::Down { slot, u, v, .. } => {
                let was = self.slots.len();
                let pos = transform.to_points(u, v);
                self.slots.insert(
                    slot,
                    Contact {
                        down: pos,
                        cur: pos,
                        down_at: now,
                    },
                );
                // A fresh single-finger gesture (0 → 1) re-arms long-press / edge-swipe.
                if was == 0 && self.slots.len() == 1 {
                    self.long_press_fired = false;
                    self.edge_swipe_fired = false;
                }
                self.rebaseline_pair();
            }
            RawContact::Move { slot, u, v, .. } => {
                let pos = transform.to_points(u, v);
                if let Some(c) = self.slots.get_mut(&slot) {
                    c.cur = pos;
                }
                self.on_move(transform, out);
            }
            RawContact::Up { slot } | RawContact::Cancel { slot } => {
                let was = self.slots.len();
                self.slots.remove(&slot);
                match self.slots.len() {
                    0 => {
                        self.long_press_fired = false;
                        self.edge_swipe_fired = false;
                    }
                    // A finger lingering after a two-finger gesture must NOT suddenly
                    // long-press / edge-swipe — suppress until every finger lifts.
                    1 if was >= 2 => {
                        self.long_press_fired = true;
                        self.edge_swipe_fired = true;
                    }
                    _ => {}
                }
                self.rebaseline_pair();
            }
        }
    }

    /// Advance the time-dependent gestures (long-press) without a new contact — the
    /// present loop calls this each frame so a finger held perfectly still still fires.
    pub fn tick(&mut self, now: Duration, out: &mut Vec<Gesture>) {
        if self.slots.len() != 1 || self.long_press_fired || self.edge_swipe_fired {
            return;
        }
        // Exactly one contact, not yet consumed by another gesture.
        if let Some(c) = self.slots.values().next() {
            let drifted = (c.cur - c.down).length();
            if drifted <= self.cfg.long_press_slop
                && now.saturating_sub(c.down_at) >= self.cfg.long_press
            {
                out.push(Gesture::SecondaryClick(c.cur));
                self.long_press_fired = true;
            }
        }
    }

    /// Two-finger move → a scroll (centroid delta) and/or a zoom (spread ratio); a
    /// single-finger move from an edge → an edge-swipe once it travels far enough.
    fn on_move(&mut self, transform: &TouchTransform, out: &mut Vec<Gesture>) {
        match self.slots.len() {
            2 => {
                let (centroid, spread) = self.pair_metrics();
                if let Some(prev) = self.last_centroid {
                    let delta = centroid - prev;
                    if delta != egui::Vec2::ZERO {
                        out.push(Gesture::Scroll(delta));
                    }
                }
                if let Some(prev) = self.last_spread {
                    if prev >= MIN_SPREAD && spread >= MIN_SPREAD {
                        let factor = spread / prev;
                        if (factor - 1.0).abs() > ZOOM_DEADBAND {
                            out.push(Gesture::Zoom(factor));
                        }
                    }
                }
                self.last_centroid = Some(centroid);
                self.last_spread = Some(spread);
            }
            1 if !self.edge_swipe_fired => {
                if let Some(c) = self.slots.values().next().copied() {
                    // The logical screen extent = the transform's far corner.
                    let size = transform.to_points(1.0, 1.0);
                    if let Some(edge) = self.edge_of(c.down, size) {
                        if inward_travel(edge, c.down, c.cur) >= self.cfg.edge_min_travel {
                            out.push(Gesture::EdgeSwipe(edge));
                            self.edge_swipe_fired = true;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Recompute the two-finger baselines: set them when exactly two fingers are down
    /// (so the next move is a clean delta from here), clear them otherwise.
    fn rebaseline_pair(&mut self) {
        if self.slots.len() == 2 {
            let (centroid, spread) = self.pair_metrics();
            self.last_centroid = Some(centroid);
            self.last_spread = Some(spread);
        } else {
            self.last_centroid = None;
            self.last_spread = None;
        }
    }

    /// The centroid (midpoint) and spread (distance) of the two active contacts. Only
    /// called when `self.slots.len() == 2`.
    fn pair_metrics(&self) -> (egui::Pos2, f32) {
        let mut it = self.slots.values();
        let a = it.next().map_or(egui::Pos2::ZERO, |c| c.cur);
        let b = it.next().map_or(egui::Pos2::ZERO, |c| c.cur);
        let centroid = egui::pos2(f32::midpoint(a.x, b.x), f32::midpoint(a.y, b.y));
        (centroid, a.distance(b))
    }

    /// Which edge zone a down-position sits in (nearest wins), or `None` if interior.
    fn edge_of(&self, down: egui::Pos2, size: egui::Pos2) -> Option<Edge> {
        let z = self.cfg.edge_zone;
        if down.x <= z {
            Some(Edge::Left)
        } else if down.x >= size.x - z {
            Some(Edge::Right)
        } else if down.y <= z {
            Some(Edge::Top)
        } else if down.y >= size.y - z {
            Some(Edge::Bottom)
        } else {
            None
        }
    }
}

/// The inward travel (points) of a contact that began at `edge` — the component of its
/// movement pointing away from that edge into the screen.
fn inward_travel(edge: Edge, down: egui::Pos2, cur: egui::Pos2) -> f32 {
    match edge {
        Edge::Left => cur.x - down.x,
        Edge::Right => down.x - cur.x,
        Edge::Top => cur.y - down.y,
        Edge::Bottom => down.y - cur.y,
    }
}

// --- the seat → shell edge-swipe side channel ---------------------------------------
//
// Same idiom as `crate::formfactor::push_formfactor`: a process-thread-local hand-off
// across the runner→surface seam (the DRM present loop and the shell render run on one
// thread). The seat pushes each recognized edge-swipe; the shell drains them once per
// frame and reacts (e.g. a left/bottom swipe raises the dock). Empty on the windowed
// fallback (no seat), so the reveal self-gates to the real DRM seat (§7).

thread_local! {
    static EDGE_SWIPES: RefCell<Vec<Edge>> = const { RefCell::new(Vec::new()) };
}

/// Push a recognized edge-swipe from the seat to the shell (a cheap thread-local write).
pub fn push_edge_swipe(edge: Edge) {
    EDGE_SWIPES.with(|q| q.borrow_mut().push(edge));
}

/// Drain every pending edge-swipe (the shell calls this once per frame). Empty on the
/// windowed fallback — the reveal self-gates to the real seat.
#[must_use]
pub fn drain_edge_swipes() -> Vec<Edge> {
    EDGE_SWIPES.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1000×1000 unrotated, unscaled transform: normalized coords map straight to
    /// points (u·1000, v·1000), so the fixtures read as pixels.
    fn xf() -> TouchTransform {
        TouchTransform::new(1000, 1000, 1.0)
    }

    fn down(slot: u32, u: f32, v: f32) -> RawContact {
        RawContact::Down {
            slot,
            u,
            v,
            force: None,
        }
    }
    fn mv(slot: u32, u: f32, v: f32) -> RawContact {
        RawContact::Move {
            slot,
            u,
            v,
            force: None,
        }
    }

    fn scrolls(gs: &[Gesture]) -> Vec<egui::Vec2> {
        gs.iter()
            .filter_map(|g| match g {
                Gesture::Scroll(d) => Some(*d),
                _ => None,
            })
            .collect()
    }

    // --- two-finger scroll ----------------------------------------------------------

    #[test]
    fn two_finger_pan_folds_to_a_scroll_delta() {
        let mut r = GestureRecognizer::new(GestureConfig::default());
        let t = xf();
        let mut out = Vec::new();
        let now = Duration::ZERO;
        // Two fingers down (no scroll yet — the baseline is set).
        r.feed(down(0, 0.4, 0.4), &t, now, &mut out);
        r.feed(down(1, 0.6, 0.4), &t, now, &mut out);
        assert!(
            scrolls(&out).is_empty(),
            "no scroll on the initial touch-down"
        );
        out.clear();
        // Both fingers slide down by 0.1 (=100 pt): the centroid moves +100 in y.
        r.feed(mv(0, 0.4, 0.5), &t, now, &mut out);
        r.feed(mv(1, 0.6, 0.5), &t, now, &mut out);
        let total: egui::Vec2 = scrolls(&out)
            .iter()
            .fold(egui::Vec2::ZERO, |acc, d| acc + *d);
        assert!(total.y > 90.0 && total.x.abs() < 1.0, "{total:?}");
        // A single finger alone never scrolls.
        let mut solo = GestureRecognizer::new(GestureConfig::default());
        let mut o2 = Vec::new();
        solo.feed(down(0, 0.5, 0.5), &t, now, &mut o2);
        solo.feed(mv(0, 0.5, 0.7), &t, now, &mut o2);
        assert!(scrolls(&o2).is_empty(), "one finger is not a scroll");
    }

    // --- pinch-zoom -----------------------------------------------------------------

    #[test]
    fn pinch_apart_folds_to_a_zoom_in_factor() {
        let mut r = GestureRecognizer::new(GestureConfig::default());
        let t = xf();
        let mut out = Vec::new();
        let now = Duration::ZERO;
        // Fingers 200 pt apart, then spread to 400 pt → a ~2× zoom-in.
        r.feed(down(0, 0.4, 0.5), &t, now, &mut out);
        r.feed(down(1, 0.6, 0.5), &t, now, &mut out);
        out.clear();
        r.feed(mv(0, 0.3, 0.5), &t, now, &mut out);
        r.feed(mv(1, 0.7, 0.5), &t, now, &mut out);
        let zoom: f32 = out
            .iter()
            .filter_map(|g| match g {
                Gesture::Zoom(z) => Some(*z),
                _ => None,
            })
            .product();
        assert!(zoom > 1.5, "spreading fingers zoom in: {zoom}");
    }

    #[test]
    fn pinch_together_folds_to_a_zoom_out_factor() {
        let mut r = GestureRecognizer::new(GestureConfig::default());
        let t = xf();
        let mut out = Vec::new();
        let now = Duration::ZERO;
        r.feed(down(0, 0.3, 0.5), &t, now, &mut out);
        r.feed(down(1, 0.7, 0.5), &t, now, &mut out);
        out.clear();
        r.feed(mv(0, 0.45, 0.5), &t, now, &mut out);
        r.feed(mv(1, 0.55, 0.5), &t, now, &mut out);
        let zoom: f32 = out
            .iter()
            .filter_map(|g| match g {
                Gesture::Zoom(z) => Some(*z),
                _ => None,
            })
            .product();
        assert!(
            zoom < 1.0 && zoom > 0.0,
            "pinching together zooms out: {zoom}"
        );
    }

    // --- long-press → secondary click -----------------------------------------------

    #[test]
    fn stationary_finger_past_the_dwell_is_a_secondary_click() {
        let cfg = GestureConfig {
            long_press: Duration::from_millis(300),
            ..GestureConfig::default()
        };
        let mut r = GestureRecognizer::new(cfg);
        let t = xf();
        let mut out = Vec::new();
        r.feed(down(0, 0.5, 0.5), &t, Duration::ZERO, &mut out);
        // Before the dwell elapses: nothing.
        r.tick(Duration::from_millis(200), &mut out);
        assert!(out.is_empty(), "no click before the dwell");
        // Past the dwell: one secondary click at the finger (500,500), fired once.
        r.tick(Duration::from_millis(350), &mut out);
        r.tick(Duration::from_millis(900), &mut out);
        let clicks: Vec<egui::Pos2> = out
            .iter()
            .filter_map(|g| match g {
                Gesture::SecondaryClick(p) => Some(*p),
                _ => None,
            })
            .collect();
        assert_eq!(clicks, vec![egui::pos2(500.0, 500.0)], "exactly one click");
    }

    #[test]
    fn a_dragging_finger_does_not_long_press() {
        let cfg = GestureConfig {
            long_press: Duration::from_millis(300),
            long_press_slop: 12.0,
            ..GestureConfig::default()
        };
        let mut r = GestureRecognizer::new(cfg);
        let t = xf();
        let mut out = Vec::new();
        r.feed(down(0, 0.5, 0.5), &t, Duration::ZERO, &mut out);
        // Drift 50 pt (past the 12 pt slop) — a drag, not a press.
        r.feed(mv(0, 0.55, 0.5), &t, Duration::from_millis(100), &mut out);
        r.tick(Duration::from_millis(400), &mut out);
        assert!(
            !out.iter().any(|g| matches!(g, Gesture::SecondaryClick(_))),
            "a moved finger is a drag, never a long-press"
        );
    }

    #[test]
    fn a_two_finger_gesture_never_long_presses() {
        let cfg = GestureConfig {
            long_press: Duration::from_millis(100),
            ..GestureConfig::default()
        };
        let mut r = GestureRecognizer::new(cfg);
        let t = xf();
        let mut out = Vec::new();
        r.feed(down(0, 0.4, 0.5), &t, Duration::ZERO, &mut out);
        r.feed(down(1, 0.6, 0.5), &t, Duration::ZERO, &mut out);
        r.tick(Duration::from_millis(500), &mut out);
        assert!(
            !out.iter().any(|g| matches!(g, Gesture::SecondaryClick(_))),
            "two fingers down is a pinch/scroll, not a long-press"
        );
        // Lift one: the lingering finger must still not long-press.
        r.feed(
            RawContact::Up { slot: 0 },
            &t,
            Duration::from_millis(500),
            &mut out,
        );
        r.tick(Duration::from_millis(900), &mut out);
        assert!(
            !out.iter().any(|g| matches!(g, Gesture::SecondaryClick(_))),
            "a finger left over from a multitouch must not long-press"
        );
    }

    // --- edge-swipe -----------------------------------------------------------------

    #[test]
    fn a_swipe_from_the_left_edge_fires_once() {
        let mut r = GestureRecognizer::new(GestureConfig::default());
        let t = xf(); // 1000 pt wide
        let mut out = Vec::new();
        let now = Duration::ZERO;
        // Down at x=10 pt (inside the 24 pt edge zone), swipe inward to x=200 pt.
        r.feed(down(0, 0.01, 0.5), &t, now, &mut out);
        r.feed(mv(0, 0.07, 0.5), &t, now, &mut out); // 60 pt inward — past 48 pt min travel
        let edges: Vec<Edge> = out
            .iter()
            .filter_map(|g| match g {
                Gesture::EdgeSwipe(e) => Some(*e),
                _ => None,
            })
            .collect();
        assert_eq!(
            edges,
            vec![Edge::Left],
            "left-edge swipe fires exactly once"
        );
        out.clear();
        // Continuing inward does not re-fire.
        r.feed(mv(0, 0.30, 0.5), &t, now, &mut out);
        assert!(
            !out.iter().any(|g| matches!(g, Gesture::EdgeSwipe(_))),
            "the edge-swipe fires once per gesture"
        );
    }

    #[test]
    fn a_swipe_starting_mid_screen_is_not_an_edge_swipe() {
        let mut r = GestureRecognizer::new(GestureConfig::default());
        let t = xf();
        let mut out = Vec::new();
        let now = Duration::ZERO;
        // Down well away from any edge; a long inward move is a plain drag.
        r.feed(down(0, 0.5, 0.5), &t, now, &mut out);
        r.feed(mv(0, 0.9, 0.5), &t, now, &mut out);
        assert!(
            !out.iter().any(|g| matches!(g, Gesture::EdgeSwipe(_))),
            "an interior swipe is not an edge-swipe"
        );
    }

    #[test]
    fn each_edge_maps_to_its_variant() {
        let t = xf();
        let now = Duration::ZERO;
        // Bottom edge: down at v≈1.0, swipe up (v decreasing) past the threshold.
        let mut r = GestureRecognizer::new(GestureConfig::default());
        let mut out = Vec::new();
        r.feed(down(0, 0.5, 0.99), &t, now, &mut out);
        r.feed(mv(0, 0.5, 0.90), &t, now, &mut out); // 90 pt inward
        assert!(out.contains(&Gesture::EdgeSwipe(Edge::Bottom)), "{out:?}");
        // Right edge.
        let mut r2 = GestureRecognizer::new(GestureConfig::default());
        let mut o2 = Vec::new();
        r2.feed(down(0, 0.99, 0.5), &t, now, &mut o2);
        r2.feed(mv(0, 0.90, 0.5), &t, now, &mut o2);
        assert!(o2.contains(&Gesture::EdgeSwipe(Edge::Right)), "{o2:?}");
    }

    // --- side channel ---------------------------------------------------------------

    #[test]
    fn edge_swipe_side_channel_round_trips() {
        let _ = drain_edge_swipes(); // clear any residue on this thread
        assert!(drain_edge_swipes().is_empty());
        push_edge_swipe(Edge::Left);
        push_edge_swipe(Edge::Bottom);
        assert_eq!(drain_edge_swipes(), vec![Edge::Left, Edge::Bottom]);
        // Drained once — empty thereafter.
        assert!(drain_edge_swipes().is_empty());
    }
}
