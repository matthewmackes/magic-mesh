//! MOTION-FEEDBACK-1 — reusable, token-driven control-feedback helpers.
//!
//! Every interactive control (button, tab, nav row, toolbar item) wants the
//! same three micro-interactions, and wants them to read identically across the
//! shell:
//!
//!   * **hover-lift** — the surface rises a few px on pointer-enter,
//!   * **press-depress** — it sinks/shrinks the instant the pointer goes *down*
//!     (no input delay: the depress is applied on press-down, not after a tween
//!     warms up), and
//!   * an **animated focus ring** — a keyboard-focus outline that grows in.
//!
//! This module is the single reusable layer. It is **pure glue** over the
//! existing primitives — the [`Motion`] presets ([`Motion::hover`] /
//! [`Motion::press`] / [`Motion::focus`]) for the durations + easing, the
//! [`crate::animation`] helpers ([`lift_on_hover`], [`Tween`], [`ease`]) for the
//! interpolation, and the palette accent token for the ring color — so no
//! control hand-rolls timing, re-derives a duration literal, or re-implements
//! the reduce-motion contract. Applying these [`FeedbackParams`] /
//! [`FocusRing`] to a concrete themed widget across the GUI crates is follow-up
//! work; this unit ships only the reusable helper.
//!
//! ## Reduce-motion contract (Q32)
//!
//! Under reduce-motion the **state change is kept but the movement is dropped**:
//!
//!   * hover-lift and press-depress collapse to *no* `translate_y` / `scale`
//!     change — the control stays geometrically at rest. The consumer still
//!     swaps the hover/press color token (that is the non-motion cue), so the
//!     state is never invisible.
//!   * the focus ring snaps to full width/opacity immediately instead of
//!     growing in — it is *present* (the accent ring is the state cue) but not
//!     *animated*.
//!
//! ## Usage
//!
//! ```
//! use std::time::Instant;
//! use mde_theme::feedback::ControlFeedback;
//!
//! let now = Instant::now();
//! // A control reports its interaction state + when each state last changed.
//! let fb = ControlFeedback::new()
//!     .hovered(true, now)   // hover entered at `now`
//!     .pressed(false)
//!     .focused(true, now);  // focus arrived at `now`
//!
//! let reduce_motion = false;
//! let geom = fb.params(now, reduce_motion); // translate_y / scale to apply
//! let _ = geom.is_at_rest();
//! // The focus ring grows in over Motion::focus(); a few frames later it is
//! // fully drawn. (Under reduce-motion it is at full width immediately.)
//! let dur = mde_theme::motion::Motion::focus().duration;
//! let ring = fb.focus_ring(now + dur, reduce_motion); // outline alpha / width
//! assert!(ring.is_visible());
//! ```

use std::time::{Duration, Instant};

use crate::animation::{ease, lift_on_hover, RenderParams, Transition, Tween};
use crate::motion::Motion;

/// How far [`ControlFeedback::new`] backdates its state timestamps so a
/// freshly-built control reads as fully settled. Comfortably exceeds every
/// interaction duration (hover/press/focus are all ≤110 ms), so all default
/// tweens are already complete at `now`.
const SETTLED_BACKDATE: Duration = Duration::from_secs(1);

/// MOTION-FEEDBACK-1 — how far a control lifts on hover (px, upward).
///
/// A small nudge: enough to read as "alive", never enough to disturb layout.
/// Carbon micro-interaction scale; component dimension, not density-scaled.
pub const HOVER_LIFT_PX: f32 = 2.0;

/// MOTION-FEEDBACK-1 — how deep a control depresses on press, as a scale-down
/// fraction (`0.04` ⇒ shrinks to 0.96 at full press).
///
/// The depress is applied on press-*down*, so a control is at this depth the
/// instant the pointer lands.
pub const PRESS_DEPTH: f32 = 0.04;

/// MOTION-FEEDBACK-1 — the focus-ring outline width at full appearance (px).
///
/// Aliases the existing platform focus-ring weight the Object Card uses
/// ([`crate::components::CARD_FOCUS_OUTLINE_WIDTH`]) so every control's focus
/// ring reads at one single-sourced width (§4: no scattered metric literals).
pub const FOCUS_RING_WIDTH_PX: f32 = crate::components::CARD_FOCUS_OUTLINE_WIDTH;

/// MOTION-FEEDBACK-1 — the focus-ring outline offset from the control edge (px).
///
/// Aliases [`crate::components::CARD_FOCUS_OUTLINE_OFFSET`] — single-sourced
/// with the Object Card focus ring.
pub const FOCUS_RING_OFFSET_PX: f32 = crate::components::CARD_FOCUS_OUTLINE_OFFSET;

/// MOTION-FEEDBACK-1 — the interaction state of one control.
///
/// The consumer rebuilds this each frame from its own hit-testing/focus
/// tracking, recording the [`Instant`] each state last *changed* so the tweens
/// know where they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlFeedback {
    hovered: bool,
    /// When `hovered` last toggled — drives the hover-lift tween.
    hover_since: Instant,
    /// `true` while the pointer is held down. The depress fires on the down
    /// edge with no warm-up tween (see [`Self::params`]), so — unlike hover and
    /// focus — there is no `press_since` timestamp: the press has no
    /// time-dependent geometry to drive.
    pressed: bool,
    focused: bool,
    /// When `focused` last became true — drives the focus-ring grow-in tween.
    focus_since: Instant,
}

impl ControlFeedback {
    /// A control at rest — not hovered, pressed, or focused. The state
    /// timestamps are backdated past the longest interaction duration so a
    /// freshly built feedback reads as fully "settled": every tween is already
    /// complete, so [`Self::params`] reports rest and [`Self::focus_ring`]
    /// reports no ring (no phantom leave-animation on the first frame).
    #[must_use]
    pub fn new() -> Self {
        // Backdate so the default (un-toggled) tweens are already complete.
        let settled = Instant::now()
            .checked_sub(SETTLED_BACKDATE)
            .unwrap_or_else(Instant::now);
        Self {
            hovered: false,
            hover_since: settled,
            pressed: false,
            focused: false,
            focus_since: settled,
        }
    }

    /// Set the hover target + when it last changed (pointer enter/leave time).
    #[must_use]
    pub const fn hovered(mut self, hovered: bool, since: Instant) -> Self {
        self.hovered = hovered;
        self.hover_since = since;
        self
    }

    /// Set whether the pointer is currently held down on the control. The
    /// depress is applied on press-down with no input delay (see
    /// [`Self::params`]), so this takes no timestamp.
    #[must_use]
    pub const fn pressed(mut self, pressed: bool) -> Self {
        self.pressed = pressed;
        self
    }

    /// Set the focus target + when focus last arrived (drives the ring grow-in).
    #[must_use]
    pub const fn focused(mut self, focused: bool, since: Instant) -> Self {
        self.focused = focused;
        self.focus_since = since;
        self
    }

    /// MOTION-FEEDBACK-1 — the geometric feedback (`translate_y` + `scale`) to
    /// apply to the control's themed widget at `now`.
    ///
    /// Combines hover-lift and press-depress:
    ///
    ///   * **hover-lift** rises [`HOVER_LIFT_PX`] over [`Motion::hover`] via the
    ///     shared [`lift_on_hover`] helper (so it animates in/out on
    ///     enter/leave).
    ///   * **press-depress fires on press-down with no input delay**: the
    ///     moment `pressed` is true the `scale` is at the full depressed value
    ///     ([`PRESS_DEPTH`]); there is no tween warm-up, so the control reacts
    ///     on the *down* edge of the click, not after a 70 ms ramp. (Carbon
    ///     micro-interaction guidance: a press must feel instant.)
    ///
    /// **Reduce-motion drops the movement** (both `translate_y` and `scale`
    /// stay at rest); the consumer conveys hover/press with the color tokens
    /// instead — the state change is kept, only the motion is gone.
    #[must_use]
    pub fn params(self, now: Instant, reduce_motion: bool) -> FeedbackParams {
        if reduce_motion {
            // Keep the state, drop the movement: geometry stays at rest.
            return FeedbackParams {
                translate_y: 0.0,
                scale: 1.0,
            };
        }
        // Hover-lift via the shared MOTION-INFRA-2 helper (animates on
        // enter/leave). Under full motion it returns the eased translate_y.
        //
        // `lift_on_hover`'s leave branch interpolates -rise → 0 over the hover
        // duration, so it assumes a not-hovered control was *previously* lifted.
        // A control that reports `!hovered` with a *fresh* `hover_since` it was
        // never actually up for (e.g. a consumer that stamps `since = now` when
        // initializing per-frame state) would otherwise read as momentarily
        // lifted. Short-circuit to rest once the leave tween has elapsed, so a
        // settled non-hovered control is always exactly at rest regardless of
        // the caller's timestamp convention.
        let lift: RenderParams = if !self.hovered
            && Tween::resolved(self.hover_since, Motion::hover().duration, false).is_complete(now)
        {
            RenderParams {
                alpha: 1.0,
                translate_y: 0.0,
                scale: 1.0,
            }
        } else {
            lift_on_hover(self.hover_since, now, HOVER_LIFT_PX, self.hovered, false)
        };
        // Press-depress through the shared MOTION-INFRA-2 mapping so the
        // press-scale formula lives in ONE place (`Transition::Press`) — a
        // future retune of press feedback updates every consumer at once. The
        // depress fires on press-DOWN with no input delay: we pass progress
        // `1.0` (fully pressed) the instant `pressed` is true rather than
        // ramping a tween, so the scale is at full depth on the down edge.
        let press_progress = if self.pressed { 1.0 } else { 0.0 };
        let scale = Transition::Press(PRESS_DEPTH).params(press_progress).scale;
        FeedbackParams {
            translate_y: lift.translate_y,
            scale,
        }
    }

    /// MOTION-FEEDBACK-1 — the animated focus ring to draw around the control at
    /// `now`.
    ///
    /// When `focused`, the ring fades + grows from nothing to full
    /// [`FOCUS_RING_WIDTH_PX`] over [`Motion::focus`] (Carbon `fast-02`). When
    /// not focused the ring is invisible (alpha 0, width 0).
    ///
    /// **Under reduce-motion the ring snaps to full width/opacity immediately**
    /// — it is present (the accent outline is the focus cue, never dropped) but
    /// not animated.
    #[must_use]
    pub fn focus_ring(self, now: Instant, reduce_motion: bool) -> FocusRing {
        if !self.focused {
            return FocusRing {
                alpha: 0.0,
                width: 0.0,
                offset: FOCUS_RING_OFFSET_PX,
            };
        }
        if reduce_motion {
            // State kept (ring present), movement/animation dropped: full ring
            // immediately.
            return FocusRing {
                alpha: 1.0,
                width: FOCUS_RING_WIDTH_PX,
                offset: FOCUS_RING_OFFSET_PX,
            };
        }
        let motion = Motion::focus();
        let tw = Tween::resolved(self.focus_since, motion.duration, false);
        let t = ease(tw.progress(now), motion.easing);
        FocusRing {
            alpha: t,
            width: FOCUS_RING_WIDTH_PX * t,
            offset: FOCUS_RING_OFFSET_PX,
        }
    }
}

impl Default for ControlFeedback {
    fn default() -> Self {
        Self::new()
    }
}

/// MOTION-FEEDBACK-1 — the geometric feedback a control applies at one frame.
///
/// `translate_y` is the hover-lift offset (negative = up); `scale` is the
/// press-depress multiplier (`1.0` = natural size). The consumer maps these
/// onto its themed widget (padding offset + size), the same way it consumes
/// [`RenderParams`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeedbackParams {
    /// Vertical offset in px (negative = lifted up on hover).
    pub translate_y: f32,
    /// Scale multiplier (`1.0` = natural; `< 1.0` while pressed).
    pub scale: f32,
}

impl FeedbackParams {
    /// Whether this frame differs from the control's resting geometry — the
    /// consumer can skip applying a transform (and skip ticking the animation
    /// subscription) when it does not.
    #[must_use]
    pub const fn is_at_rest(self) -> bool {
        self.translate_y.abs() < f32::EPSILON && (self.scale - 1.0).abs() < f32::EPSILON
    }
}

/// MOTION-FEEDBACK-1 — the animated focus-ring outline at one frame.
///
/// The consumer draws an outline of `width` px at `offset` px from the control
/// edge, in the palette accent color at `alpha` opacity. Width + alpha grow
/// together while the ring animates in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FocusRing {
    /// Outline opacity `0.0..=1.0` (0 = no ring).
    pub alpha: f32,
    /// Outline width in px (grows 0 → [`FOCUS_RING_WIDTH_PX`]).
    pub width: f32,
    /// Outline offset from the control edge in px.
    pub offset: f32,
}

impl FocusRing {
    /// Whether the ring should be drawn at all this frame.
    #[must_use]
    pub const fn is_visible(self) -> bool {
        self.alpha > f32::EPSILON && self.width > f32::EPSILON
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn press_fires_on_down_with_no_input_delay() {
        // MOTION-FEEDBACK-1 acceptance: the depress is applied on press-DOWN —
        // the scale is already at full depth at the instant the pointer lands
        // (t == press_since), NOT after the 70 ms hover/press tween warms up.
        let now = Instant::now();
        let fb = ControlFeedback::new().pressed(true);
        let at_down = fb.params(now, false);
        assert!(
            (at_down.scale - (1.0 - PRESS_DEPTH)).abs() < 1e-6,
            "press must be at full depth on the down edge, got {}",
            at_down.scale
        );
        // And it stays depressed for as long as the pointer is held — it does
        // not ramp.
        let later = fb.params(now + Duration::from_millis(35), false);
        assert!((later.scale - (1.0 - PRESS_DEPTH)).abs() < 1e-6);
        // Released ⇒ back to natural size.
        let up = ControlFeedback::new().pressed(false);
        assert!((up.params(now, false).scale - 1.0).abs() < 1e-6);
    }

    #[test]
    fn reduce_motion_keeps_state_but_drops_movement() {
        // MOTION-FEEDBACK-1 acceptance: under reduce-motion the geometry stays
        // at rest (no lift, no depress) — the consumer keeps the state via the
        // color token, but there is zero movement.
        let now = Instant::now();
        let fb = ControlFeedback::new()
            .hovered(true, now)
            .pressed(true)
            .focused(true, now);
        // Sample across the whole tween window: never any movement.
        for ms in [0, 35, 70, 110, 240] {
            let t = now + Duration::from_millis(ms);
            let g = fb.params(t, true);
            assert_eq!(g.translate_y, 0.0, "no hover-lift under reduce-motion @{ms}ms");
            assert_eq!(g.scale, 1.0, "no press-depress under reduce-motion @{ms}ms");
            assert!(g.is_at_rest());
        }
        // The focus-ring STATE is kept — the ring is present immediately at full
        // width/opacity (it just doesn't animate in).
        let ring = fb.focus_ring(now, true);
        assert!(ring.is_visible(), "focus ring state kept under reduce-motion");
        assert!((ring.width - FOCUS_RING_WIDTH_PX).abs() < 1e-6);
        assert!((ring.alpha - 1.0).abs() < 1e-6);
    }

    #[test]
    fn hover_lift_rises_over_the_hover_motion_with_full_motion() {
        // With motion on, hover lifts upward (negative translate_y) over the
        // Motion::hover() duration — reusing the shared lift helper.
        let now = Instant::now();
        let dur = Motion::hover().duration; // 70 ms
        let fb = ControlFeedback::new().hovered(true, now);
        // At t=0 it's still at rest; at the end of the hover tween it's lifted.
        assert!(fb.params(now, false).translate_y.abs() < 1e-4);
        let lifted = fb.params(now + dur, false);
        assert!(
            (lifted.translate_y + HOVER_LIFT_PX).abs() < 1e-4,
            "lifts to -HOVER_LIFT_PX, got {}",
            lifted.translate_y
        );
    }

    #[test]
    fn focus_ring_grows_in_then_settles_with_full_motion() {
        // Focus ring fades + widens 0 → full over Motion::focus(), then holds.
        let now = Instant::now();
        let dur = Motion::focus().duration; // 110 ms
        let fb = ControlFeedback::new().focused(true, now);
        let start = fb.focus_ring(now, false);
        assert!(start.alpha < 1e-3, "ring starts invisible");
        assert!(start.width < 1e-3);
        let end = fb.focus_ring(now + dur, false);
        assert!((end.alpha - 1.0).abs() < 1e-4, "ring ends opaque");
        assert!(
            (end.width - FOCUS_RING_WIDTH_PX).abs() < 1e-4,
            "ring ends at full width"
        );
        assert!((end.offset - FOCUS_RING_OFFSET_PX).abs() < 1e-6);
    }

    #[test]
    fn unfocused_control_draws_no_ring() {
        let now = Instant::now();
        let fb = ControlFeedback::new().focused(false, now);
        let ring = fb.focus_ring(now, false);
        assert!(!ring.is_visible());
        assert_eq!(ring.alpha, 0.0);
        assert_eq!(ring.width, 0.0);
        // Same under reduce-motion: no focus ⇒ no ring.
        assert!(!fb.focus_ring(now, true).is_visible());
    }

    #[test]
    fn settled_unhovered_control_is_at_rest_for_any_timestamp() {
        // Regression: lift_on_hover's leave branch assumes a not-hovered control
        // was previously lifted, so a fresh `hover_since` on a never-lifted
        // control would read as momentarily lifted. A settled (tween-elapsed)
        // non-hovered control must be exactly at rest regardless of the caller's
        // timestamp convention.
        let now = Instant::now();
        let dur = Motion::hover().duration;
        // hover_since well in the past ⇒ leave tween elapsed ⇒ at rest.
        let fb = ControlFeedback::new().hovered(false, now - dur * 2);
        assert!(fb.params(now, false).is_at_rest());
    }

    #[test]
    fn resting_control_is_geometrically_at_rest() {
        let now = Instant::now();
        let fb = ControlFeedback::new();
        let g = fb.params(now, false);
        assert!(g.is_at_rest(), "an untouched control has no transform");
        assert!(!fb.focus_ring(now, false).is_visible());
    }

    #[test]
    fn ring_tokens_match_the_object_card_focus_outline() {
        // MOTION-FEEDBACK-1 reuses the existing platform focus-ring weight +
        // offset rather than introducing a competing metric.
        assert!((FOCUS_RING_WIDTH_PX - crate::components::CARD_FOCUS_OUTLINE_WIDTH).abs() < 1e-6);
        assert!((FOCUS_RING_OFFSET_PX - crate::components::CARD_FOCUS_OUTLINE_OFFSET).abs() < 1e-6);
    }
}
