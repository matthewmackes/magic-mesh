//! UX-9.a — animation tween math.
//!
//! Iced 0.13 doesn't ship an `iced::animation` module
//! (that lands in 0.14, currently BLOCKED by UX-PRE). This
//! module fills the gap with the pure math + a `Tween` state
//! struct that consumers drive from their own
//! `iced::time::every` subscription.
//!
//! ## Usage
//!
//! ```
//! use std::time::Instant;
//! use mde_theme::animation::{ease, Tween};
//! use mde_theme::motion::Easing;
//!
//! let mut t = Tween::starting_at(Instant::now(), std::time::Duration::from_millis(180));
//! // Each tick: progress 0.0 → 1.0 (clamped at 1.0 when done).
//! let value = ease(t.progress(Instant::now()), Easing::EaseOut);
//! let done  = t.is_complete(Instant::now());
//! ```
//!
//! Lerp helpers handle f32 + Color blending so the consumer
//! can interpolate any visible property (opacity, translate,
//! background tint, scale).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::motion::{Easing, Motion};

/// Single-shot tween over a fixed duration. Stateless w.r.t.
/// `Instant`: the consumer asks "what fraction am I at NOW"
/// each tick + the tween reports complete when elapsed
/// >= duration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tween {
    start: Instant,
    duration: Duration,
}

impl Tween {
    /// Start a tween at `start` running for `duration`.
    #[must_use]
    pub fn starting_at(start: Instant, duration: Duration) -> Self {
        Self { start, duration }
    }

    /// Linear progress 0.0 → 1.0 clamped at 1.0 when done.
    #[must_use]
    pub fn progress(self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.start);
        if elapsed >= self.duration {
            return 1.0;
        }
        elapsed.as_secs_f32() / self.duration.as_secs_f32()
    }

    /// Has the tween reached its endpoint?
    #[must_use]
    pub fn is_complete(self, now: Instant) -> bool {
        now.saturating_duration_since(self.start) >= self.duration
    }

    /// When did this tween start?
    #[must_use]
    pub fn start(self) -> Instant {
        self.start
    }

    /// How long is this tween?
    #[must_use]
    pub fn duration(self) -> Duration {
        self.duration
    }

    /// Build a static (zero-duration) tween for use under
    /// `reduce_motion`. `is_complete` returns `true` immediately;
    /// `progress` returns `1.0` immediately — the consumer renders
    /// the final/static frame without any interpolation. Q99.
    #[must_use]
    pub fn static_frame(now: Instant) -> Self {
        Self {
            start: now,
            duration: Duration::ZERO,
        }
    }

    /// MOTION-CORE-2 — the single reduce-motion-aware tween constructor every
    /// consumer should call. With `reduce_motion`, the duration is capped to the
    /// Q32 ≤80 ms crossfade ([`crate::motion::REDUCE_MOTION_CAP_MS`]); otherwise
    /// it's the requested duration. Routing all tweens through this guarantees the
    /// reduce-motion contract (mirrors [`crate::motion::Motion::resolved`]).
    #[must_use]
    pub fn resolved(start: Instant, duration: Duration, reduce_motion: bool) -> Self {
        let duration = if reduce_motion {
            duration.min(Duration::from_millis(crate::motion::REDUCE_MOTION_CAP_MS))
        } else {
            duration
        };
        Self::starting_at(start, duration)
    }
}

/// Looping tween — the timeline restarts every `duration`.
/// Used for the notification bell pulse (UX-9 b) + the
/// future spinner indicator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopingTween {
    start: Instant,
    period: Duration,
}

impl LoopingTween {
    /// Start a looping tween at `start` with `period` per
    /// full cycle.
    #[must_use]
    pub fn starting_at(start: Instant, period: Duration) -> Self {
        Self { start, period }
    }

    /// Fractional phase 0.0 → 1.0 within the current cycle.
    #[must_use]
    pub fn phase(self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.start);
        let period_ms = self.period.as_secs_f32().max(f32::EPSILON);
        let phase = elapsed.as_secs_f32() % period_ms / period_ms;
        // Round to f32 precision; clamp guards against any
        // floating-point overshoot.
        phase.clamp(0.0, 1.0)
    }
}

/// Apply an easing curve to a linear progress value in
/// `[0.0, 1.0]`. Output is also clamped to `[0.0, 1.0]`.
#[must_use]
pub fn ease(t: f32, easing: Easing) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match easing {
        Easing::Linear => t,
        // Cubic ease-out — standard "fast start, slow end".
        Easing::EaseOut => 1.0 - (1.0 - t).powi(3),
        // Cubic ease-in — slow start, fast end.
        Easing::EaseIn => t.powi(3),
        // Cubic ease-in-out — symmetric S-curve. Match the
        // common CSS `ease-in-out` shape.
        Easing::EaseInOut => {
            if t < 0.5 {
                4.0 * t.powi(3)
            } else {
                let f = 2.0 * t - 2.0;
                0.5 * f.powi(3) + 1.0
            }
        }
    }
}

/// MOTION-CORE-2 — critically-damped spring position at normalized time `t`
/// (`[0.0, 1.0]`), settling **monotonically** to ~1.0 with **no overshoot** — a
/// spring *feel* for press/hover without the distracting bounce. Uses the
/// critical-damping closed form `1 - (1 + k·t)·e^(−k·t)` (k chosen so it's ~98%
/// settled at `t = 1`). Output is clamped to `[0.0, 1.0]`.
#[must_use]
pub fn spring(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    // Critical-damping rate: higher = snappier. k=6 settles to ~0.98 at t=1.
    const K: f32 = 6.0;
    (1.0 - (1.0 + K * t) * (-K * t).exp()).clamp(0.0, 1.0)
}

/// Linear interpolation between two f32 values. `t` is
/// clamped to `[0.0, 1.0]`.
#[must_use]
pub fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    a + (b - a) * t
}

/// Ping-pong tween value used by the notification pulse:
/// scales smoothly from 1.0 → max → 1.0 → max → 1.0 …
/// per full period. `phase` is in `[0.0, 1.0]`.
#[must_use]
pub fn pulse_scale(phase: f32, max_scale: f32) -> f32 {
    // Phase 0.0 → 0.5 grows; 0.5 → 1.0 shrinks.
    let p = phase.clamp(0.0, 1.0);
    let triangle = if p < 0.5 { p * 2.0 } else { 2.0 - p * 2.0 };
    // Ease-in-out smoothing on the triangle so the pulse
    // breathes instead of corner-ticking at the peak.
    let smoothed = ease(triangle, Easing::EaseInOut);
    lerp_f32(1.0, max_scale, smoothed)
}

/// MOTION-INFRA-1 — a tiny animation registry. Holds the active tweens keyed by
/// a caller id and is advanced by ONE subscription tick, so N concurrent
/// animations across a surface share a single timer instead of each arming its
/// own. [`Animator::is_idle`] reports when nothing is in flight, so the consumer
/// can stop ticking at rest (no idle/offscreen CPU — MOTION-PERF-1). Pure state
/// (no toolkit dep); the consumer reads [`Animator::value`] in its `view`.
#[derive(Debug, Default, Clone)]
pub struct Animator {
    tweens: HashMap<String, Tween>,
}

impl Animator {
    /// An empty animator (nothing animating).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Start (or restart) the animation under `id` from `start`, using the
    /// preset's duration resolved against `reduce_motion`
    /// ([`Tween::resolved`]).
    pub fn start(
        &mut self,
        id: impl Into<String>,
        start: Instant,
        motion: Motion,
        reduce_motion: bool,
    ) {
        self.tweens.insert(
            id.into(),
            Tween::resolved(start, motion.duration, reduce_motion),
        );
    }

    /// The eased value `0.0..=1.0` for `id` at `now`, or `1.0` when there is no
    /// such animation (treat "not animating" as "at the final frame").
    #[must_use]
    pub fn value(&self, id: &str, now: Instant, easing: Easing) -> f32 {
        self.tweens
            .get(id)
            .map_or(1.0, |tw| ease(tw.progress(now), easing))
    }

    /// Whether `id` currently has an in-flight (incomplete) animation.
    #[must_use]
    pub fn is_animating(&self, id: &str, now: Instant) -> bool {
        self.tweens.get(id).is_some_and(|tw| !tw.is_complete(now))
    }

    /// Drop every completed tween (call once per tick). Returns the count still
    /// in flight — pair with the subscription so it stops when this hits 0.
    pub fn gc(&mut self, now: Instant) -> usize {
        self.tweens.retain(|_, tw| !tw.is_complete(now));
        self.tweens.len()
    }

    /// True when nothing is animating — the subscription should stop ticking
    /// (MOTION-PERF-1: zero idle wakeups).
    #[must_use]
    pub fn is_idle(&self, now: Instant) -> bool {
        self.tweens.values().all(|tw| tw.is_complete(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::PULSE_MAX_SCALE;

    #[test]
    fn animator_runs_many_off_one_clock_and_goes_idle() {
        // MOTION-INFRA-1 — N animations share one animator; is_idle reports when
        // all settle (so the consumer's single subscription can stop).
        let t0 = Instant::now();
        let mut a = Animator::new();
        assert!(a.is_idle(t0), "empty animator is idle");
        a.start("fade", t0, Motion::panel_mount(), false); // 240 ms
        a.start("hover", t0, Motion::hover(), false); // 70 ms
        assert!(!a.is_idle(t0), "two in-flight ⇒ not idle");
        assert!(a.is_animating("fade", t0));
        // Midway: value is between 0 and 1, still animating.
        let mid = t0 + Duration::from_millis(35);
        let v = a.value("fade", mid, Easing::Linear);
        assert!(v > 0.0 && v < 1.0, "fade interpolating, got {v}");
        // After the longest duration everything settles + gc clears it.
        let done = t0 + Duration::from_millis(300);
        assert!(a.is_idle(done), "all settled ⇒ idle");
        assert_eq!(a.gc(done), 0, "gc drops completed tweens");
        assert!((a.value("missing", done, Easing::Linear) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn animator_reduce_motion_settles_fast() {
        let t0 = Instant::now();
        let mut a = Animator::new();
        a.start("x", t0, Motion::loading(), true); // capped to 80 ms
        assert!(a.is_idle(t0 + Duration::from_millis(80)));
    }

    #[test]
    fn spring_is_monotonic_and_never_overshoots() {
        // MOTION-CORE-2 — critically-damped spring: starts at 0, rises
        // monotonically, settles near (and never above) 1.0 — no bounce.
        assert!(spring(0.0).abs() < 1e-6, "spring(0) must be 0");
        let mut prev = spring(0.0);
        for i in 1..=20 {
            let t = i as f32 / 20.0;
            let v = spring(t);
            assert!(
                v >= prev - 1e-6,
                "must be monotonic non-decreasing at t={t}"
            );
            assert!(v <= 1.0 + 1e-6, "must never overshoot 1.0 at t={t}");
            prev = v;
        }
        assert!(
            spring(1.0) > 0.95,
            "must be ~settled by t=1, got {}",
            spring(1.0)
        );
    }

    #[test]
    fn resolved_caps_duration_under_reduce_motion() {
        // MOTION-CORE-2 — the single reduce-motion-aware tween constructor caps to
        // the Q32 80 ms crossfade; full motion keeps the requested duration.
        let now = Instant::now();
        let full = Tween::resolved(now, Duration::from_millis(400), false);
        assert_eq!(full.duration(), Duration::from_millis(400));
        let reduced = Tween::resolved(now, Duration::from_millis(400), true);
        assert_eq!(reduced.duration(), Duration::from_millis(80));
        // A short tween already under the cap is left as-is.
        let short = Tween::resolved(now, Duration::from_millis(40), true);
        assert_eq!(short.duration(), Duration::from_millis(40));
    }

    #[test]
    fn tween_progress_starts_at_zero_and_ends_at_one() {
        let t0 = Instant::now();
        let tw = Tween::starting_at(t0, Duration::from_millis(180));
        assert!((tw.progress(t0) - 0.0).abs() < 1e-4);
        let t_mid = t0 + Duration::from_millis(90);
        assert!((tw.progress(t_mid) - 0.5).abs() < 0.05);
        let t_end = t0 + Duration::from_millis(180);
        assert!((tw.progress(t_end) - 1.0).abs() < 1e-4);
        // Past the end the value clamps at 1.0.
        let t_after = t0 + Duration::from_millis(360);
        assert!((tw.progress(t_after) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn tween_is_complete_after_duration() {
        let t0 = Instant::now();
        let tw = Tween::starting_at(t0, Duration::from_millis(100));
        assert!(!tw.is_complete(t0));
        assert!(!tw.is_complete(t0 + Duration::from_millis(50)));
        assert!(tw.is_complete(t0 + Duration::from_millis(100)));
        assert!(tw.is_complete(t0 + Duration::from_millis(500)));
    }

    #[test]
    fn ease_out_smoothly_finishes() {
        // ease_out(0.0) = 0; ease_out(1.0) = 1; midpoint
        // should be > 0.5 (curve is concave-down).
        assert!((ease(0.0, Easing::EaseOut) - 0.0).abs() < 1e-4);
        assert!((ease(1.0, Easing::EaseOut) - 1.0).abs() < 1e-4);
        assert!(ease(0.5, Easing::EaseOut) > 0.5);
    }

    #[test]
    fn ease_in_starts_slow() {
        assert!((ease(0.0, Easing::EaseIn) - 0.0).abs() < 1e-4);
        assert!((ease(1.0, Easing::EaseIn) - 1.0).abs() < 1e-4);
        assert!(ease(0.5, Easing::EaseIn) < 0.5);
    }

    #[test]
    fn ease_in_out_is_symmetric_around_midpoint() {
        // f(0.5) ≈ 0.5; f(0.25) + f(0.75) ≈ 1.0
        assert!((ease(0.5, Easing::EaseInOut) - 0.5).abs() < 0.01);
        let a = ease(0.25, Easing::EaseInOut);
        let b = ease(0.75, Easing::EaseInOut);
        assert!((a + b - 1.0).abs() < 0.01, "{a} + {b} should ~= 1.0");
    }

    #[test]
    fn lerp_clamps_t_outside_unit_interval() {
        assert!((lerp_f32(0.0, 10.0, 0.5) - 5.0).abs() < 1e-4);
        // Negative t clamps to 0 — returns a.
        assert!((lerp_f32(0.0, 10.0, -1.0) - 0.0).abs() < 1e-4);
        // t > 1 clamps to 1 — returns b.
        assert!((lerp_f32(0.0, 10.0, 2.0) - 10.0).abs() < 1e-4);
    }

    #[test]
    fn pulse_scale_returns_one_at_endpoints() {
        // Phase 0 + phase 1 both = beginning of a cycle = scale 1.0.
        assert!((pulse_scale(0.0, PULSE_MAX_SCALE) - 1.0).abs() < 1e-4);
        assert!((pulse_scale(1.0, PULSE_MAX_SCALE) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn pulse_scale_peaks_near_half_phase() {
        // Mid-cycle hits ~max_scale (smoothed, so very close
        // but not exactly at 1.15).
        let peak = pulse_scale(0.5, PULSE_MAX_SCALE);
        assert!(peak > 1.10, "peak should be near 1.15 max, got {peak}");
        assert!(peak <= PULSE_MAX_SCALE + 1e-4);
    }

    #[test]
    fn looping_tween_phase_cycles() {
        let t0 = Instant::now();
        let period = Duration::from_millis(2000);
        let lt = LoopingTween::starting_at(t0, period);
        assert!((lt.phase(t0) - 0.0).abs() < 1e-4);
        // 25 % through the cycle.
        let t_q = t0 + Duration::from_millis(500);
        assert!((lt.phase(t_q) - 0.25).abs() < 0.01);
        // Past one full cycle, phase wraps back to 0.
        let t_one = t0 + period;
        assert!(lt.phase(t_one) < 0.01);
    }

    #[test]
    fn tween_from_motion_round_trip() {
        // Plumbing test: building a Tween from `Motion::panel_mount()` carries
        // its Carbon `moderate-02` (240 ms) duration through without drift
        // (E9.5 — reconciled from the former UX-9 180 ms).
        let m = Motion::panel_mount();
        let tw = Tween::starting_at(Instant::now(), m.duration);
        assert_eq!(tw.duration(), Duration::from_millis(240));
    }

    // ── Q99 reduce-motion static-render assertions ────────────────────────

    #[test]
    fn static_frame_tween_is_immediately_complete() {
        // Q99: reduce-motion path. Tween::static_frame() must report
        // complete + progress=1.0 at the instant it's created so the
        // consumer renders the final static frame without interpolation.
        let now = Instant::now();
        let tw = Tween::static_frame(now);
        assert!(
            tw.is_complete(now),
            "static_frame must be complete at t=start"
        );
        assert!(
            (tw.progress(now) - 1.0).abs() < 1e-6,
            "static_frame progress must be 1.0 at t=start"
        );
    }

    #[test]
    fn static_frame_tween_has_zero_duration() {
        let tw = Tween::static_frame(Instant::now());
        assert_eq!(tw.duration(), Duration::ZERO);
    }

    #[test]
    fn reduce_motion_a11y_tween_completes_at_80ms() {
        // Q4 + Q99: when A11y::reduce_motion=true, transition_duration_ms
        // caps to 80 ms. A tween built with that cap must be complete
        // at exactly t=start+80ms — the consumer sees the static/final
        // frame no later than 80 ms after the animation begins.
        use crate::accessibility::A11y;
        let a11y = A11y {
            reduce_motion: true,
            ..A11y::default()
        };
        let cap_ms = a11y.transition_duration_ms(180) as u64;
        assert_eq!(cap_ms, 80, "Q4: reduce_motion must cap at 80 ms");
        let now = Instant::now();
        let tw = Tween::starting_at(now, Duration::from_millis(cap_ms));
        let at_cap = now + Duration::from_millis(cap_ms);
        assert!(
            tw.is_complete(at_cap),
            "reduce_motion tween must be complete at t=start+80ms"
        );
        assert!(
            (tw.progress(at_cap) - 1.0).abs() < 1e-6,
            "reduce_motion tween progress must be 1.0 at the cap"
        );
    }

    #[test]
    fn reduce_motion_off_does_not_collapse_duration() {
        // Normal motion must NOT collapse. Guards against accidental
        // always-static renders if the reduce_motion flag defaults wrong.
        use crate::accessibility::A11y;
        let a11y = A11y::default();
        assert!(!a11y.reduce_motion);
        let cap_ms = a11y.transition_duration_ms(180) as u64;
        assert_eq!(
            cap_ms, 180,
            "reduce_motion=false must preserve standard duration"
        );
    }
}
