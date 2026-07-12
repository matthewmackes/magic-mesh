//! `Motion` — the small shared duration/easing table (governance §4, lock 10).
//!
//! E12 retires the bespoke `mde_theme::motion` engine and its lint gate. Motion
//! is now just egui's built-in `animate_bool` driven by a handful of named
//! durations, so every surface eases the same way without a separate framework.

use std::sync::atomic::{AtomicBool, Ordering};

use egui::Context;

/// Process-global **reduce-motion** preference (a11y-07): a motion / vestibular-comfort
/// toggle. When set, the shared eased helpers ([`Motion::animate`] /
/// [`Motion::animate_value`]) collapse to their settled endpoint immediately instead of
/// gliding. `false` by default (motion on — the current behaviour). The shell drives it
/// from its persisted appearance config at startup and on every change; it is read on
/// the hot per-frame animate path, so a plain `Relaxed` atomic is the right weight (a
/// UI-comfort flag, not a synchronisation edge). Deliberately global — every surface
/// paints through the one shared `Motion` table, so one flag damps them all without
/// threading a parameter through every widget.
static REDUCE_MOTION: AtomicBool = AtomicBool::new(false);

/// The shared motion table. Durations are in **seconds** (egui's animation unit).
pub struct Motion;

/// The visual parameters for a status/severity transition.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatusMotion {
    /// Smooth fade progress for the status tint, `0.0..=1.0`.
    pub fade: f32,
    /// One-shot attention pulse strength, `0.0..=1.0`; non-zero only on worsening.
    pub pulse: f32,
}

/// A **spring** — stiffness/damping for the macOS-style physical transitions the
/// chrome uses (Start open/close, taskbar reveal, panel slides, surface switches,
/// the splash→Workbench hero expansion). Unlike the cubic [`Motion::animate`]
/// helpers a spring carries velocity, so it settles with a natural
/// overshoot-and-relax rather than a fixed-duration ease. [`step`](Self::step) is
/// pure (unit-testable without a frame loop); [`Motion::spring_to`] drives it off
/// the egui clock and honours reduce-motion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    /// Restoring force per unit displacement — higher = faster, tighter settle.
    pub stiffness: f32,
    /// Velocity damping — higher = less overshoot (near-critical for chrome).
    pub damping: f32,
}

impl Spring {
    /// Snappy UI spring — chrome reveals, Start open/close, taskbar slide. Fast
    /// settle with only a hair of overshoot.
    pub const SNAPPY: Self = Self {
        stiffness: 220.0,
        damping: 26.0,
    };
    /// Gentle spring — large sheets and the splash→Workbench hero expansion. Softer
    /// and a touch slower.
    pub const GENTLE: Self = Self {
        stiffness: 120.0,
        damping: 20.0,
    };

    /// One semi-implicit-Euler step from `(pos, vel)` toward `target` over `dt`
    /// seconds → the new `(pos, vel)`. Pure. `dt` is clamped to a 30fps floor so a
    /// long stall (backgrounded tab) can't blow the integrator up.
    #[must_use]
    pub fn step(self, pos: f32, vel: f32, target: f32, dt: f32) -> (f32, f32) {
        let dt = dt.clamp(0.0, 1.0 / 30.0);
        let accel = self.stiffness * (target - pos) - self.damping * vel;
        let vel = vel + accel * dt;
        let pos = pos + vel * dt;
        (pos, vel)
    }

    /// Whether the spring has effectively **settled** at `target` (position and
    /// velocity both within a small epsilon), so the driver can stop repainting.
    #[must_use]
    pub fn settled(self, pos: f32, vel: f32, target: f32) -> bool {
        (target - pos).abs() < 0.01 && vel.abs() < 0.01
    }
}

impl Motion {
    /// Quick feedback — hover, small toggles, focus.
    pub const FAST: f32 = 0.08;
    /// Standard transition — panel reveals, tab switches, most state changes.
    pub const BASE: f32 = 0.18;
    /// Deliberate — larger movement, drawers, first-paint reveals.
    pub const SLOW: f32 = 0.32;

    /// One **hard-blink** half-cycle, in seconds — the alarm cadence a D/F node
    /// grade flashes on/off at (NODE-GRADE-2 #6/#16). Unlike [`FAST`]/[`BASE`]/
    /// [`SLOW`] (which *ease* a transition) this drives a square on↔off toggle: the
    /// signal is fully on for one [`BLINK`] span, fully dark for the next. Kept on
    /// the shared table so no surface mints its own blink literal (§4).
    pub const BLINK: f32 = 0.5;

    /// Status tint fade duration for pips/segments (NOTIF-1/Q26).
    pub const STATUS_FADE: f32 = Self::BASE;
    /// One-shot status attention pulse duration for worsening only (NOTIF-1/Q26).
    pub const STATUS_PULSE: f32 = 0.48;

    /// Set the process-global **reduce-motion** preference (a11y-07). The shell calls
    /// this from its appearance apply seam — at startup and on every toggle change —
    /// with the persisted value, so every [`animate`](Self::animate) /
    /// [`animate_value`](Self::animate_value) caller settles instantly rather than
    /// easing. Idempotent; `Relaxed` is sufficient for a UI-comfort flag.
    pub fn set_reduce_motion(on: bool) {
        REDUCE_MOTION.store(on, Ordering::Relaxed);
    }

    /// Whether **reduce-motion** (a11y-07) is currently in force — the flag the eased
    /// helpers consult to short-circuit to their endpoint. `false` (motion on) by
    /// default. Note the hard-blink alarm ([`blink`](Self::blink)) deliberately
    /// ignores this: an alarm outranks the comfort preference (NODE-GRADE-2 #16).
    #[must_use]
    pub fn reduce_motion() -> bool {
        REDUCE_MOTION.load(Ordering::Relaxed)
    }

    /// Animate a boolean toward `on`, returning the eased `0.0..=1.0` progress.
    ///
    /// Thin wrapper over egui's [`Context::animate_bool_with_time`] (which eases
    /// with a smooth cubic), keyed by a stable `id`. Pass one of [`Motion::FAST`]
    /// / [`Motion::BASE`] / [`Motion::SLOW`] for `secs` so timing stays on the
    /// shared table rather than a bespoke literal. Under [`reduce_motion`](Self::reduce_motion)
    /// the ease is skipped and the settled endpoint (`1.0` for `on`, else `0.0`) is
    /// reported at once — no travel, and no per-frame repaint request (a11y-07).
    pub fn animate(ctx: &Context, id: impl std::hash::Hash, on: bool, secs: f32) -> f32 {
        if Self::reduce_motion() {
            return if on { 1.0 } else { 0.0 };
        }
        ctx.animate_bool_with_time(egui::Id::new(id), on, secs)
    }

    /// Animate a **scalar** toward `target`, returning the eased current value.
    ///
    /// Thin wrapper over egui's [`Context::animate_value_with_time`], keyed by a
    /// stable `id`. The **first** frame an `id` is seen the stored value is
    /// written straight to `target` — so a freshly-appearing value lands in
    /// place with no ease-in from zero, and only a *subsequent* target change
    /// glides. Pass one of [`Motion::FAST`] / [`Motion::BASE`] / [`Motion::SLOW`]
    /// for `secs` so the cadence stays on the shared table rather than a bespoke
    /// literal. The sibling of [`animate`](Self::animate) for continuous
    /// quantities: eased **spatial** transitions — a mesh node gliding to its new
    /// layout slot as peers join or leave, rather than teleporting. egui repaints
    /// only while the value is still travelling, so a settled value stays idle.
    /// Under [`reduce_motion`](Self::reduce_motion) the glide is skipped and the
    /// `target` is returned immediately — the node lands in place (a11y-07).
    pub fn animate_value(ctx: &Context, id: impl std::hash::Hash, target: f32, secs: f32) -> f32 {
        if Self::reduce_motion() {
            return target;
        }
        ctx.animate_value_with_time(egui::Id::new(id), target, secs)
    }

    /// The current phase of the shared **hard blink** (NODE-GRADE-2 #6/#16): `true`
    /// while the alarm should show, `false` while it is dark, flipping every
    /// [`BLINK`] seconds off the egui clock. A square wave, NOT an eased fade — an
    /// alarm reads as a hard flash — and it deliberately *ignores reduce-motion* (the
    /// alarm outranks the preference, #16). Schedules the next repaint so an
    /// unattended alarm keeps flashing without pointer input (egui otherwise sleeps
    /// once idle). The pure phase math is [`blink_at`].
    #[must_use]
    pub fn blink(ctx: &Context) -> bool {
        let on = Self::blink_at(ctx.input(|i| i.time), Self::BLINK);
        ctx.request_repaint_after(std::time::Duration::from_secs_f32(Self::BLINK));
        on
    }

    /// The pure hard-blink phase at clock `time` (seconds) for a `period`-second
    /// half-cycle: on across `[0, period)`, off across `[period, 2·period)`, then
    /// repeating. Split out so the square wave is unit-tested without an egui clock.
    /// A non-positive `period` is degenerate and reads steadily on.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn blink_at(time: f64, period: f32) -> bool {
        if period <= 0.0 {
            return true;
        }
        // Even half-cycle index → on, odd → off. `rem_euclid` keeps a (never-
        // expected) negative clock well-defined rather than flipping the phase.
        let idx = (time / f64::from(period)).floor() as i64;
        idx.rem_euclid(2) == 0
    }

    /// Fold elapsed time since a status change into the shared NOTIF-1 transition:
    /// every change gets a smooth fade, and only a **worsening** gets one bounded
    /// attention pulse. Improving/resolving states return `pulse = 0.0` so they
    /// calm down rather than demand attention.
    #[must_use]
    pub fn status_transition_at(elapsed: f32, worsened: bool) -> StatusMotion {
        let elapsed = elapsed.max(0.0);
        let fade = (elapsed / Self::STATUS_FADE).clamp(0.0, 1.0);
        let pulse = if worsened && elapsed < Self::STATUS_PULSE {
            let phase = elapsed / Self::STATUS_PULSE;
            // One smooth rise/fall, capped well below the a11y flashing limit.
            (std::f32::consts::PI * phase).sin().max(0.0)
        } else {
            0.0
        };
        StatusMotion { fade, pulse }
    }

    /// Per-second exponential friction for an **inertial** fling (higher = stops
    /// sooner). Used by [`inertial_decay`](Self::inertial_decay).
    pub const INERTIAL_FRICTION: f32 = 6.0;
    /// The maximum **rubber-band** overscroll past an edge, in logical px — the
    /// asymptote [`rubber_band`](Self::rubber_band) compresses overshoot toward.
    pub const RUBBER_SLACK: f32 = 48.0;

    /// Drive a **spring** toward `target`, keyed by a stable `id`: reads the stored
    /// `(pos, vel)` from egui memory, advances it one frame off the egui clock, and
    /// requests a repaint while it is still travelling. Like
    /// [`animate_value`](Self::animate_value) the **first** sight of an `id` lands on
    /// `target` in place (no spring-in from zero); only a later `target` change
    /// springs. Under [`reduce_motion`](Self::reduce_motion) it returns `target`
    /// immediately (a11y — no travel, no repaint churn).
    pub fn spring_to(ctx: &Context, id: impl std::hash::Hash, target: f32, spring: Spring) -> f32 {
        if Self::reduce_motion() {
            return target;
        }
        let id = egui::Id::new(id);
        let dt = ctx.input(|i| i.stable_dt);
        let (pos, vel) = ctx
            .data_mut(|d| d.get_temp::<(f32, f32)>(id))
            .unwrap_or((target, 0.0));
        let (pos, vel) = spring.step(pos, vel, target, dt);
        ctx.data_mut(|d| d.insert_temp(id, (pos, vel)));
        if !spring.settled(pos, vel, target) {
            ctx.request_repaint();
        }
        pos
    }

    /// Exponential friction **decay** of a scroll/fling `velocity` over `dt` — the
    /// momentum left after a flick. Frame-rate-independent (decays by
    /// `exp(-FRICTION·dt)`); velocity asymptotes to `0`. Pure.
    #[must_use]
    pub fn inertial_decay(velocity: f32, dt: f32) -> f32 {
        velocity * (-Self::INERTIAL_FRICTION * dt.max(0.0)).exp()
    }

    /// **Rubber-band** an overscrolled position `x` back toward `[lo, hi]`: within
    /// range it passes straight through; past an edge the displacement is compressed
    /// so it asymptotes and never travels more than [`RUBBER_SLACK`] past the edge
    /// (the iOS overscroll feel). Pure. `hi < lo` is treated as an empty range that
    /// pins to `lo`.
    #[must_use]
    pub fn rubber_band(x: f32, lo: f32, hi: f32) -> f32 {
        let hi = hi.max(lo);
        if x < lo {
            lo - Self::band(lo - x)
        } else if x > hi {
            hi + Self::band(x - hi)
        } else {
            x
        }
    }

    /// Compressed overshoot for [`rubber_band`](Self::rubber_band): maps a raw past-
    /// edge displacement `d ≥ 0` into `[0, RUBBER_SLACK)`, ≈`d` near the edge and
    /// asymptoting to [`RUBBER_SLACK`] as `d → ∞`.
    #[must_use]
    fn band(d: f32) -> f32 {
        let s = Self::RUBBER_SLACK;
        s * (1.0 - (-d.max(0.0) / s).exp())
    }

    /// `smoothstep` on a `0..=1` progress — the ease every micro-interaction factor
    /// below shares (eased in and out, clamped).
    #[must_use]
    fn smoothstep(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// **Hover-lift** factor for a `t`∈`0..=1` hover progress → a bounded `0..=1`
    /// the caller maps to a small upward offset / brightness bump. Feed it a
    /// reduce-motion-aware `t` (e.g. [`animate`](Self::animate) with [`FAST`]) so it
    /// collapses to `0`/`1` when motion is reduced.
    #[must_use]
    pub fn hover_lift(t: f32) -> f32 {
        Self::smoothstep(t)
    }

    /// **Press-scale** factor for a `t`∈`0..=1` press progress → a scale in
    /// `[0.97, 1.0]` (a subtle squash on press; `1.0` at rest).
    #[must_use]
    pub fn press_scale(t: f32) -> f32 {
        1.0 - 0.03 * t.clamp(0.0, 1.0)
    }

    /// **Focus-glow** factor `0..=1` for a `t`∈`0..=1` focus progress → the focus
    /// ring/glow alpha.
    #[must_use]
    pub fn focus_glow(t: f32) -> f32 {
        Self::smoothstep(t)
    }

    /// **Toggle-knob** position `0..=1` for a `t`∈`0..=1` on-progress → where an
    /// animated switch's knob sits between off and on.
    #[must_use]
    pub fn toggle_knob(t: f32) -> f32 {
        Self::smoothstep(t)
    }
}

#[cfg(test)]
#[allow(clippy::assertions_on_constants)]
mod tests {
    use super::{Motion, Spring};

    #[test]
    fn durations_are_positive_and_ordered() {
        assert!(Motion::FAST > 0.0);
        assert!(Motion::FAST < Motion::BASE);
        assert!(Motion::BASE < Motion::SLOW);
        assert!(
            Motion::BLINK > 0.0,
            "the alarm blink half-cycle is a real span"
        );
        assert!(Motion::STATUS_FADE > 0.0);
        assert!(Motion::STATUS_PULSE > Motion::STATUS_FADE);
    }

    #[test]
    fn blink_is_a_hard_square_wave() {
        // On for the first half-cycle, off for the next, then on again — a hard
        // on/off, not an eased ramp (NODE-GRADE-2 #6).
        assert!(Motion::blink_at(0.0, 0.5), "on at t=0");
        assert!(Motion::blink_at(0.49, 0.5), "still on just before the flip");
        assert!(
            !Motion::blink_at(0.5, 0.5),
            "off at the half-cycle boundary"
        );
        assert!(!Motion::blink_at(0.99, 0.5), "still off");
        assert!(Motion::blink_at(1.0, 0.5), "on again a full cycle later");
        // A degenerate period never divides by zero — it just reads on.
        assert!(Motion::blink_at(3.0, 0.0));
    }

    #[test]
    fn blink_drives_off_the_context_clock() {
        // Render-agnostic: a fresh context sits at t=0, so the blink starts ON, and
        // the call is pure/total (it schedules its own repaint, never panics).
        let ctx = egui::Context::default();
        assert!(Motion::blink(&ctx), "the blink starts on at the zero clock");
    }

    #[test]
    fn animate_is_bounded_and_keyed() {
        // Render-agnostic: a fresh context with no elapsed time reports the
        // resting endpoint (0 for false), and the call is pure/total.
        let ctx = egui::Context::default();
        let t = Motion::animate(&ctx, "motion-test", false, Motion::BASE);
        assert!((0.0..=1.0).contains(&t), "progress {t} out of range");
    }

    #[test]
    fn animate_value_lands_on_target_on_first_sight() {
        // Render-agnostic: the first time an id is seen the eased value is written
        // straight to the target (no ease-in from zero), so a just-appeared node
        // lands in place; re-reading the same target holds it steady. The call is
        // pure/total on a fresh context.
        let ctx = egui::Context::default();
        let first = Motion::animate_value(&ctx, "motion-value-test", 42.0, Motion::SLOW);
        assert_eq!(first, 42.0, "first sight of an id lands on the target");
        let held = Motion::animate_value(&ctx, "motion-value-test", 42.0, Motion::SLOW);
        assert_eq!(held, 42.0, "an unchanged target stays put");
    }

    #[test]
    fn reduce_motion_collapses_animations_to_their_endpoint() {
        // a11y-07: with reduce-motion set, the eased helpers report the SETTLED
        // endpoint on the very first frame — no ease-in, whatever the duration. The
        // flag is process-global (every surface shares the one Motion table), so set
        // it, assert, and restore it so sibling tests (same process) keep the
        // default-off behaviour. The endpoint values themselves also stay in range for
        // any concurrent sibling, so this never races them into a false failure.
        let ctx = egui::Context::default();
        assert!(!Motion::reduce_motion(), "reduce-motion is off by default");

        Motion::set_reduce_motion(true);
        assert!(Motion::reduce_motion(), "the setter takes effect");
        assert_eq!(
            Motion::animate(&ctx, "rm-bool-on", true, Motion::SLOW),
            1.0,
            "a bool animation toward ON lands on 1.0 at once, not the eased ramp"
        );
        assert_eq!(
            Motion::animate(&ctx, "rm-bool-off", false, Motion::SLOW),
            0.0,
            "…and toward OFF lands on 0.0 at once"
        );
        // A scalar lands on the target immediately even when the target CHANGES — the
        // case a fresh id can't cover (a fresh id lands on first sight regardless).
        let _ = Motion::animate_value(&ctx, "rm-val", 0.0, Motion::SLOW);
        assert_eq!(
            Motion::animate_value(&ctx, "rm-val", 100.0, Motion::SLOW),
            100.0,
            "a scalar animation lands on a CHANGED target immediately under reduce-motion"
        );

        Motion::set_reduce_motion(false);
        assert!(
            !Motion::reduce_motion(),
            "restored to the default for sibling tests"
        );
    }

    #[test]
    fn a_spring_settles_at_its_target() {
        // Integrate the pure step from rest at 0 toward 100; within a second of
        // frames it lands (position + velocity within epsilon), and never runs away.
        let spring = Spring::SNAPPY;
        let (mut pos, mut vel) = (0.0f32, 0.0f32);
        for _ in 0..120 {
            (pos, vel) = spring.step(pos, vel, 100.0, 1.0 / 60.0);
            assert!(
                pos.is_finite() && vel.is_finite(),
                "integrator stayed finite"
            );
        }
        assert!(
            spring.settled(pos, vel, 100.0),
            "settled near target: pos={pos} vel={vel}"
        );
        assert!((pos - 100.0).abs() < 1.0, "landed on the target");
    }

    #[test]
    fn spring_to_collapses_to_target_under_reduce_motion() {
        let ctx = egui::Context::default();
        Motion::set_reduce_motion(true);
        assert_eq!(
            Motion::spring_to(&ctx, "spring-rm", 42.0, Spring::GENTLE),
            42.0,
            "under reduce-motion the spring reports its target at once"
        );
        Motion::set_reduce_motion(false);
    }

    #[test]
    fn inertial_velocity_decays_toward_zero() {
        let mut v = 1000.0f32;
        for _ in 0..120 {
            v = Motion::inertial_decay(v, 1.0 / 60.0);
        }
        assert!(v.abs() < 1.0, "a fling decays to a stop, got {v}");
        assert!(
            Motion::inertial_decay(500.0, 1.0 / 60.0) < 500.0,
            "friction always slows it"
        );
    }

    #[test]
    fn rubber_band_passes_through_in_range_and_stays_bounded_past_the_edge() {
        // Inside the range: identity.
        assert_eq!(Motion::rubber_band(50.0, 0.0, 100.0), 50.0);
        // Far past an edge: compressed and never beyond edge ± SLACK (the asymptote
        // is reachable at float precision, so the bound is inclusive).
        let over = Motion::rubber_band(100_000.0, 0.0, 100.0);
        assert!(
            over > 100.0 && over <= 100.0 + Motion::RUBBER_SLACK,
            "bounded overscroll: {over}"
        );
        let under = Motion::rubber_band(-100_000.0, 0.0, 100.0);
        assert!(
            under < 0.0 && under >= -Motion::RUBBER_SLACK,
            "bounded under-scroll: {under}"
        );
    }

    #[test]
    fn micro_interaction_factors_are_bounded() {
        for t in [0.0, 0.25, 0.5, 0.75, 1.0, -0.3, 1.7] {
            assert!(
                (0.0..=1.0).contains(&Motion::hover_lift(t)),
                "hover_lift {t}"
            );
            assert!(
                (0.0..=1.0).contains(&Motion::focus_glow(t)),
                "focus_glow {t}"
            );
            assert!(
                (0.0..=1.0).contains(&Motion::toggle_knob(t)),
                "toggle_knob {t}"
            );
            let s = Motion::press_scale(t);
            assert!(
                (0.97..=1.0).contains(&s),
                "press_scale {t} → {s} out of squash range"
            );
        }
        // Endpoints are the resting/settled values, so a reduce-motion-collapsed t
        // (0 or 1 from Motion::animate) yields no half-state.
        assert_eq!(Motion::hover_lift(0.0), 0.0);
        assert_eq!(Motion::hover_lift(1.0), 1.0);
        assert_eq!(Motion::press_scale(0.0), 1.0);
    }

    #[test]
    fn status_transition_fades_and_pulses_only_on_worsening() {
        let start = Motion::status_transition_at(0.0, true);
        assert_eq!(start.fade, 0.0);
        assert_eq!(start.pulse, 0.0);

        let mid = Motion::status_transition_at(Motion::STATUS_PULSE / 2.0, true);
        assert!(mid.fade > 0.0 && mid.fade <= 1.0);
        assert!(mid.pulse > 0.9, "pulse peaks once on worsening: {mid:?}");

        let settled = Motion::status_transition_at(Motion::STATUS_PULSE + 0.01, true);
        assert_eq!(settled.pulse, 0.0);
        assert_eq!(
            Motion::status_transition_at(Motion::STATUS_FADE * 2.0, true).fade,
            1.0
        );

        let improving = Motion::status_transition_at(Motion::STATUS_PULSE / 2.0, false);
        assert!(improving.fade > 0.0);
        assert_eq!(
            improving.pulse, 0.0,
            "improving states fade without an attention pulse"
        );
    }
}
