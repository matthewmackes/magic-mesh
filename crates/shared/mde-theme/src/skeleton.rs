//! BEAUT-THEME — the Carbon skeleton / placeholder primitive.
//!
//! A skeleton is the grey "loading shape" a surface paints in place of content
//! that hasn't arrived yet.
//!
//! It stands in for a list row, a card, or an avatar so the layout settles
//! instantly and the wait reads as *active* rather than *frozen* — the
//! perceived-performance win. This module is the **reusable foundation** every
//! surface consumes; the actual widget wrapping (an iced container with the
//! resolved fill colour) stays consumer-side so the toolkit dep never leaks into
//! `mde-theme` (mirrors [`crate::components`]).
//!
//! It is deliberately **glue, not a reimplementation**: the shimmer alpha curve,
//! its dim/bright bounds, the looping clock, the reduce-motion contract, and the
//! idle/visibility tick-gating all already live in [`crate::animation`] +
//! [`crate::motion`]. This module composes them into one skeleton-shaped API.
//!
//! ## The reduce-motion + kill-switch contract (Carbon §7 / MOTION-CORE)
//!
//! A skeleton is **never** motion-only. When the user prefers reduced motion
//! (the `a11y.reduce_motion` [`Preferences`](crate::Preferences) flag) **or** the
//! global motion kill switch is off ([`crate::prefs::MotionPrefs::enabled`] =
//! `false`), the shimmer
//! collapses to a **static** plain-grey placeholder at the mid alpha — the shape
//! is still there (the layout/“something is loading” cue is structural, not
//! motion), it just doesn't breathe. [`SkeletonShimmer::reduced`] folds both
//! signals into the single `reduce` flag, so a consumer resolves it **once** from
//! [`Preferences`](crate::Preferences) and the rest is pure.
//!
//! ## Idle-gating (MOTION-PERF-1 / MOTION-INFRA-3)
//!
//! While shimmering, a surface must arm a per-frame tick **only while the
//! skeleton is visible and motion is live** — a skeleton on a hidden/collapsed
//! surface, or under reduce-motion / the kill switch, must wake the CPU zero
//! times. [`SkeletonShimmer::needs_tick`] is the single predicate a
//! `subscription()` gates on; it enforces the same `visible && live` rule as
//! [`Animator::needs_tick`](crate::animation::Animator::needs_tick). A surface
//! that already drives one-shot tweens through an [`Animator`] folds the shimmer
//! into the same subscription by `OR`-ing the two predicates
//! (`animator.needs_tick(now) || shimmer.needs_tick(visible)`) — the shimmer's
//! perpetual loop is **not** an [`Animator`] tween (the registry holds completing
//! one-shots, not loops), so it keeps its own clock and is read via
//! [`SkeletonShimmer::alpha`].

use std::time::Instant;

use crate::animation::{shimmer_alpha, LoopingTween};
use crate::color::Rgba;
use crate::motion::Motion;
use crate::palette::Palette;
use crate::prefs::Preferences;
use crate::radii::Radii;

/// The shape of one skeleton placeholder block, in px.
///
/// Pure geometry — width/height/corner-radius — pulled from tokens, never raw
/// literals at the call site. A consumer paints a rounded rectangle of this size
/// filled with the resolved shimmer colour.
///
/// `width == None` means "fill the available width" (a list-row skeleton spans
/// its container); a concrete `width` pins a fixed block (an avatar, a chip).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkeletonBlock {
    /// Block width in px, or `None` to fill the available width.
    pub width: Option<u16>,
    /// Block height in px.
    pub height: u16,
    /// Corner radius in px — a [`Radii`] token, not a literal.
    pub radius: u16,
}

impl SkeletonBlock {
    /// A single text-line placeholder: a short, fixed-width rounded bar at the
    /// `text_line` height, `sm` corner radius. `width` pins the bar (a label);
    /// pass `None` to span the container (a paragraph line).
    #[must_use]
    pub const fn line(width: Option<u16>, radii: Radii) -> Self {
        Self {
            width,
            height: TEXT_LINE_HEIGHT_PX,
            radius: radii.sm,
        }
    }

    /// A card / tile placeholder: a fill-width block at `height`, `md` corner
    /// radius (the same 8 px the real card uses, so the swap-in is seamless).
    #[must_use]
    pub const fn card(height: u16, radii: Radii) -> Self {
        Self {
            width: None,
            height,
            radius: radii.md,
        }
    }

    /// A circular avatar / monogram placeholder of `diameter` px (full radius —
    /// the renderer clamps `radius` to half the side, yielding a circle).
    #[must_use]
    pub const fn avatar(diameter: u16) -> Self {
        Self {
            width: Some(diameter),
            height: diameter,
            // Full pill radius reads as a circle at a 1:1 aspect.
            radius: diameter / 2,
        }
    }
}

/// Default height of a single-line text skeleton bar, in px. A component
/// dimension (not density-scaled, per UX-24) sized to the body type line-box.
pub const TEXT_LINE_HEIGHT_PX: u16 = 12;

/// The animated fill of a skeleton — it resolves the placeholder's tint at any
/// instant.
///
/// Honours reduce-motion + the motion kill switch. Construct it from
/// [`Preferences`] via [`SkeletonShimmer::from_prefs`] so both motion signals are
/// folded in at one site; everything after that is pure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkeletonShimmer {
    /// The looping clock the shimmer phase is read from. Carbon `slow-02`
    /// (700 ms) per [`Motion::loading`] — one period per breathe cycle.
    clock: LoopingTween,
    /// Folded motion signal: `reduce_motion || !motion.enabled`. When set the
    /// shimmer is **static** (no breathe) and no tick is ever needed.
    reduce: bool,
}

impl SkeletonShimmer {
    /// Start a shimmer clock at `start`, resolving the motion contract from the
    /// user's [`Preferences`]: the breathe is suppressed when reduce-motion is on
    /// **or** the global motion kill switch is off. The single §7 entry point.
    #[must_use]
    pub fn from_prefs(start: Instant, prefs: &Preferences) -> Self {
        Self::new(start, Self::reduced(prefs))
    }

    /// Lower-level constructor with an explicit, pre-folded `reduce` flag — for
    /// tests and for a consumer that already has the resolved boolean. Prefer
    /// [`SkeletonShimmer::from_prefs`] in product code.
    #[must_use]
    pub fn new(start: Instant, reduce: bool) -> Self {
        // The breathe period is the shared `loading` activity preset (Carbon
        // slow-02, 700 ms looping) — one source for every shimmer/spinner.
        let period = Motion::loading().duration;
        Self {
            clock: LoopingTween::starting_at(start, period),
            reduce,
        }
    }

    /// Fold the two motion signals — the a11y reduce-motion preference and the
    /// global [`MotionPrefs`](crate::prefs::MotionPrefs) kill switch — into the
    /// single boolean the shimmer obeys. Either being set means "no breathe".
    #[must_use]
    pub const fn reduced(prefs: &Preferences) -> bool {
        prefs.a11y.reduce_motion || !prefs.motion.enabled
    }

    /// `true` when the shimmer is static (no breathe) — reduce-motion is on or
    /// the motion kill switch is off. A static skeleton is a plain grey block.
    #[must_use]
    pub const fn is_static(self) -> bool {
        self.reduce
    }

    /// The shimmer fill **alpha** at `now` — the breathe between
    /// [`SKELETON_ALPHA_DIM`] and [`SKELETON_ALPHA_BRIGHT`], or the static mid
    /// alpha under reduce-motion / the kill switch. Pure delegate to
    /// [`shimmer_alpha`]; the a11y static-fallback lives there, single-sourced.
    #[must_use]
    pub fn alpha(self, now: Instant) -> f32 {
        shimmer_alpha(self.clock.phase(now), self.reduce)
    }

    /// The resolved fill **colour** at `now`: the theme's foreground tint at the
    /// current shimmer alpha, composited over the surface the skeleton sits on.
    /// Carbon §4: the tint is the palette `text` token (one ramp step over the
    /// card surface), never a raw grey — so it tracks dark/light automatically.
    #[must_use]
    pub fn fill(self, now: Instant, palette: &Palette) -> Rgba {
        palette.text.with_alpha(self.alpha(now))
    }

    /// MOTION-INFRA-3 — the tick predicate a `subscription()` gates on for a
    /// skeleton: a per-frame tick is needed only when the surface is `visible`
    /// **and** the shimmer is live (not static). Under reduce-motion / the kill
    /// switch, or while hidden, this is `false`, so the clock is never armed
    /// (MOTION-PERF-1: zero idle/offscreen wakeups). Mirrors the
    /// [`Animator::needs_tick`](crate::animation::Animator::needs_tick) rule.
    #[must_use]
    pub const fn needs_tick(self, visible: bool) -> bool {
        visible && !self.reduce
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accessibility::A11y;
    use crate::animation::{Animator, SKELETON_ALPHA_BRIGHT, SKELETON_ALPHA_DIM};
    use crate::prefs::MotionPrefs;
    use crate::theme::Theme;
    use std::time::Duration;

    fn prefs_with(reduce_motion: bool, motion_enabled: bool) -> Preferences {
        Preferences {
            a11y: A11y {
                reduce_motion,
                ..A11y::default()
            },
            motion: MotionPrefs {
                enabled: motion_enabled,
                ..MotionPrefs::default()
            },
            ..Preferences::default()
        }
    }

    #[test]
    fn block_line_uses_sm_radius_and_text_line_height() {
        let r = Radii::defaults();
        let pinned = SkeletonBlock::line(Some(80), r);
        assert_eq!(pinned.width, Some(80));
        assert_eq!(pinned.height, TEXT_LINE_HEIGHT_PX);
        assert_eq!(pinned.radius, r.sm);
        // `None` width spans the container.
        assert_eq!(SkeletonBlock::line(None, r).width, None);
    }

    #[test]
    fn block_card_uses_md_radius_and_fills_width() {
        let r = Radii::defaults();
        let c = SkeletonBlock::card(96, r);
        assert_eq!(c.width, None);
        assert_eq!(c.height, 96);
        // Same 8 px corner the real card uses, so the swap-in is seamless.
        assert_eq!(c.radius, r.md);
    }

    #[test]
    fn block_avatar_is_a_circle() {
        let a = SkeletonBlock::avatar(40);
        assert_eq!(a.width, Some(40));
        assert_eq!(a.height, 40);
        // radius == side/2 ⇒ a circle at 1:1.
        assert_eq!(a.radius, 20);
    }

    #[test]
    fn shimmer_breathes_when_motion_is_live() {
        // Motion on, reduce off ⇒ the alpha oscillates over a period.
        let t0 = Instant::now();
        let s = SkeletonShimmer::from_prefs(t0, &prefs_with(false, true));
        assert!(!s.is_static());
        let period = Motion::loading().duration;
        let lo = s.alpha(t0); // phase 0 ⇒ dim end
        let mid = s.alpha(t0 + period / 2); // phase 0.5 ⇒ bright end
        assert!(
            mid > lo,
            "shimmer must brighten from the dim end toward the peak: {lo} !< {mid}"
        );
        // And it stays within the published Carbon skeleton bounds.
        for i in 0..=10 {
            let a = s.alpha(t0 + Duration::from_millis(70 * i));
            assert!(
                (SKELETON_ALPHA_DIM..=SKELETON_ALPHA_BRIGHT).contains(&a),
                "{a}"
            );
        }
    }

    #[test]
    fn reduce_motion_makes_the_shimmer_static() {
        let t0 = Instant::now();
        let s = SkeletonShimmer::from_prefs(t0, &prefs_with(true, true));
        assert!(s.is_static());
        // Static ⇒ the same mid alpha at every instant (a plain grey block).
        let mid = (SKELETON_ALPHA_DIM + SKELETON_ALPHA_BRIGHT) / 2.0;
        for ms in [0u64, 175, 350, 700, 1400] {
            let a = s.alpha(t0 + Duration::from_millis(ms));
            assert!(
                (a - mid).abs() < 1e-6,
                "static skeleton must not breathe: {a}"
            );
        }
    }

    #[test]
    fn kill_switch_also_makes_the_shimmer_static() {
        // MOTION-CORE-3: the global kill switch suppresses the breathe even with
        // reduce-motion off — folded into the one `reduce` flag.
        let t0 = Instant::now();
        let s = SkeletonShimmer::from_prefs(t0, &prefs_with(false, false));
        assert!(s.is_static(), "kill switch must force a static skeleton");
        let mid = (SKELETON_ALPHA_DIM + SKELETON_ALPHA_BRIGHT) / 2.0;
        assert!((s.alpha(t0 + Duration::from_millis(350)) - mid).abs() < 1e-6);
    }

    #[test]
    fn reduced_folds_both_motion_signals() {
        assert!(!SkeletonShimmer::reduced(&prefs_with(false, true)));
        assert!(SkeletonShimmer::reduced(&prefs_with(true, true)));
        assert!(SkeletonShimmer::reduced(&prefs_with(false, false)));
        assert!(SkeletonShimmer::reduced(&prefs_with(true, false)));
    }

    #[test]
    fn needs_tick_only_while_visible_and_live() {
        let t0 = Instant::now();
        let live = SkeletonShimmer::from_prefs(t0, &prefs_with(false, true));
        let still = SkeletonShimmer::from_prefs(t0, &prefs_with(true, true));
        // Live + visible ⇒ tick. Live + hidden ⇒ no tick (offscreen).
        assert!(live.needs_tick(true));
        assert!(!live.needs_tick(false));
        // Static ⇒ never ticks, even visible (reduce-motion / kill switch).
        assert!(!still.needs_tick(true));
        assert!(!still.needs_tick(false));
    }

    #[test]
    fn shimmer_co_gates_with_an_animator_subscription() {
        // The documented co-gate: a surface that also drives one-shot tweens ORs
        // the two predicates into one subscription. A static (reduce-motion)
        // shimmer must contribute zero wakeups, so the combined gate is driven
        // purely by the animator; a live visible shimmer keeps the gate armed
        // even when the animator's own tweens have settled.
        let t0 = Instant::now();
        let done = t0 + Duration::from_secs(10); // well past any tween.

        let mut anim = Animator::new();
        anim.start("enter", t0, Motion::panel_mount(), false);
        anim.set_visible(true);
        // The animator's one-shot has completed by `done`, so on its own it is idle.
        assert!(!anim.needs_tick(done));

        // Live shimmer keeps the combined gate armed past the tween's end.
        let live = SkeletonShimmer::from_prefs(t0, &prefs_with(false, true));
        assert!(anim.needs_tick(done) || live.needs_tick(anim.is_visible()));

        // Static shimmer adds nothing — the combined gate follows the animator,
        // which is idle, so the subscription correctly stops.
        let still = SkeletonShimmer::from_prefs(t0, &prefs_with(true, true));
        assert!(!(anim.needs_tick(done) || still.needs_tick(anim.is_visible())));
    }

    #[test]
    fn fill_tints_the_palette_text_token_at_the_shimmer_alpha() {
        // Carbon §4: the fill is the palette `text` step at the shimmer alpha —
        // not a raw grey — so it tracks the theme.
        let t0 = Instant::now();
        let pal = Palette::for_theme(Theme::Dark);
        let s = SkeletonShimmer::from_prefs(t0, &prefs_with(true, true)); // static
        let f = s.fill(t0, &pal);
        assert_eq!((f.r, f.g, f.b), (pal.text.r, pal.text.g, pal.text.b));
        let mid = (SKELETON_ALPHA_DIM + SKELETON_ALPHA_BRIGHT) / 2.0;
        assert!((f.a - mid).abs() < 1e-6);
        // Light theme tints from the light `text` token, not a hardcoded grey.
        let light = Palette::for_theme(Theme::Light);
        let lf = s.fill(t0, &light);
        assert_eq!(
            (lf.r, lf.g, lf.b),
            (light.text.r, light.text.g, light.text.b)
        );
        assert_ne!((lf.r, lf.g, lf.b), (f.r, f.g, f.b));
    }
}
