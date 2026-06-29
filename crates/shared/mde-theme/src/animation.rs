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

use crate::frame_timer::{FrameSample, FrameTimer};
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

    /// MOTION-A11Y-3 — a flash-safe looping constructor for pulses/blinks/
    /// shimmer. The requested `period` is **clamped up** to
    /// [`crate::motion::MIN_PULSE_PERIOD_MS`] so the loop can never flash faster
    /// than [`crate::motion::MAX_PULSE_HZ`] (the WCAG 2.3.1 3 Hz seizure
    /// threshold), regardless of the caller. Use this — not
    /// [`LoopingTween::starting_at`] — for any *visual flash* loop; the plain
    /// constructor stays for non-flashing continuous timelines (e.g. a slow beacon
    /// sweep) where the cap doesn't apply.
    ///
    /// ```
    /// use std::time::{Duration, Instant};
    /// use mde_theme::animation::LoopingTween;
    /// use mde_theme::motion::MAX_PULSE_HZ;
    /// // A 50 ms (20 Hz) request is clamped up to the ≤3 Hz floor.
    /// let p = LoopingTween::pulse(Instant::now(), Duration::from_millis(50));
    /// assert!(p.hz() <= MAX_PULSE_HZ + 1e-3);
    /// ```
    #[must_use]
    pub fn pulse(start: Instant, period: Duration) -> Self {
        let floor = Duration::from_millis(crate::motion::MIN_PULSE_PERIOD_MS);
        Self {
            start,
            period: period.max(floor),
        }
    }

    /// MOTION-A11Y-3 — this loop's flash frequency in Hz (one full cycle = one
    /// flash). `0.0` for a degenerate zero period. Lets a test/assertion verify a
    /// loop respects [`crate::motion::MAX_PULSE_HZ`].
    #[must_use]
    pub fn hz(self) -> f32 {
        let secs = self.period.as_secs_f32();
        if secs <= f32::EPSILON {
            0.0
        } else {
            1.0 / secs
        }
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

/// MOTION-NET-2 — dim end of a skeleton placeholder's shimmer (alpha).
pub const SKELETON_ALPHA_DIM: f32 = 0.10;
/// MOTION-NET-2 — bright end of a skeleton placeholder's shimmer (alpha).
pub const SKELETON_ALPHA_BRIGHT: f32 = 0.22;

/// MOTION-NET-2 — the alpha for a greyed skeleton placeholder at `phase` (0→1).
/// The tile "breathes" between [`SKELETON_ALPHA_DIM`] and
/// [`SKELETON_ALPHA_BRIGHT`] (eased ping-pong, like [`pulse_scale`]) so a slow
/// load reads as active. **Under reduce-motion it is STATIC** at the mid alpha —
/// a plain grey block, no shimmer (the a11y contract: motion is never the only
/// cue). Pure.
#[must_use]
pub fn shimmer_alpha(phase: f32, reduce_motion: bool) -> f32 {
    if reduce_motion {
        return (SKELETON_ALPHA_DIM + SKELETON_ALPHA_BRIGHT) / 2.0;
    }
    let p = phase.clamp(0.0, 1.0);
    let triangle = if p < 0.5 { p * 2.0 } else { 2.0 - p * 2.0 };
    let smoothed = ease(triangle, Easing::EaseInOut);
    lerp_f32(SKELETON_ALPHA_DIM, SKELETON_ALPHA_BRIGHT, smoothed)
}

/// MOTION-INFRA-2 — the standard shell transition kinds. Each maps an eased
/// progress `t` (0→1, from [`Animator::value`]) to concrete [`RenderParams`] the
/// consumer applies to its themed widget (alpha → a container/text color alpha,
/// `translate_y` → padding/offset, `scale` → size). Centralizing the math here is
/// the reusable-transition layer; the actual Element wrapping stays consumer-side
/// (the iced 0.13 fork has no opacity/transform widget — interpolate color-alpha
/// + size instead). Compositor-friendly: only alpha/translate/scale, never layout
/// thrash.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Transition {
    /// Opacity 0→1 (element appearing).
    FadeIn,
    /// Opacity 1→0 (element leaving).
    FadeOut,
    /// Fade in while sliding up from `distance` px below to rest.
    SlideUp(f32),
    /// Hover lift — rises `rise` px (negative `translate_y`) as `t` grows.
    Lift(f32),
    /// Press depress — scales down by `depth` (e.g. `0.04` ⇒ 0.96 at full press).
    Press(f32),
}

/// MOTION-INFRA-2 — the render parameters a [`Transition`] yields at a given
/// progress. Consumers apply what's relevant (most use one or two fields).
///
/// ## MOTION-PERF-2 — the transform/opacity-only invariant
///
/// `RenderParams` carries **exactly** opacity ([`alpha`](RenderParams::alpha)) and
/// transform ([`translate_y`](RenderParams::translate_y),
/// [`scale`](RenderParams::scale)) — and *structurally no width/height/layout
/// field*. That is the whole MOTION-PERF-2 guard: an animation expressed through
/// this struct can only ever drive compositor-cheap properties, so it can never
/// trigger a per-frame relayout. A consumer **must** map these onto
/// transform-equivalents (a padding *offset* for `translate_y`, a render `scale`)
/// and **must not** re-measure or resize the widget per frame from them. The
/// [`is_compositor_safe`](RenderParams::is_compositor_safe) predicate is the
/// runtime backstop a consumer can `debug_assert` before applying a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderParams {
    /// Opacity multiplier `0.0..=1.0`.
    pub alpha: f32,
    /// Vertical offset in px (negative = up).
    pub translate_y: f32,
    /// Scale multiplier (1.0 = natural size).
    pub scale: f32,
}

impl RenderParams {
    /// MOTION-PERF-2 — the runtime guard for the transform/opacity-only invariant.
    /// `true` when every field holds a sane, compositor-applicable value: `alpha`
    /// in `0.0..=1.0`, a finite `translate_y`, and a strictly-positive finite
    /// `scale`. (A non-finite or non-positive value would force the renderer down
    /// a degenerate/relayout path.) Consumers `debug_assert!(params.is_compositor_safe())`
    /// before applying a frame so a future mis-tuned transition can't silently
    /// regress the no-relayout guarantee.
    ///
    /// ```
    /// use mde_theme::animation::Transition;
    /// let p = Transition::SlideUp(8.0).params(0.5);
    /// assert!(p.is_compositor_safe()); // alpha∈[0,1], finite translate, scale>0
    /// ```
    #[must_use]
    pub fn is_compositor_safe(self) -> bool {
        (0.0..=1.0).contains(&self.alpha)
            && self.translate_y.is_finite()
            && self.scale.is_finite()
            && self.scale > 0.0
    }
}

impl Transition {
    /// Resolve this transition at eased progress `t` (clamped to `0.0..=1.0`).
    #[must_use]
    pub fn params(self, t: f32) -> RenderParams {
        let t = t.clamp(0.0, 1.0);
        let base = RenderParams {
            alpha: 1.0,
            translate_y: 0.0,
            scale: 1.0,
        };
        match self {
            Self::FadeIn => RenderParams { alpha: t, ..base },
            Self::FadeOut => RenderParams {
                alpha: 1.0 - t,
                ..base
            },
            Self::SlideUp(distance) => RenderParams {
                alpha: t,
                translate_y: (1.0 - t) * distance,
                ..base
            },
            Self::Lift(rise) => RenderParams {
                translate_y: -t * rise,
                ..base
            },
            Self::Press(depth) => RenderParams {
                scale: 1.0 - t * depth,
                ..base
            },
        }
    }
}

/// MOTION-INFRA-1 — a tiny animation registry. Holds the active tweens keyed by
/// a caller id and is advanced by ONE subscription tick, so N concurrent
/// animations across a surface share a single timer instead of each arming its
/// own. [`Animator::is_idle`] reports when nothing is in flight, so the consumer
/// can stop ticking at rest (no idle/offscreen CPU — MOTION-PERF-1). Pure state
/// (no toolkit dep); the consumer reads [`Animator::value`] in its `view`.
///
/// ## MOTION-INFRA-3 — idle/visibility-hardened tick gating
///
/// A subscription must arm a per-frame tick **only while an animation is
/// actually in flight AND the surface is visible**. The animator therefore
/// tracks the surface's visibility ([`Animator::set_visible`]) and exposes
/// [`Animator::needs_tick`] — the single predicate a consumer's `subscription()`
/// gates on. The contract this enforces:
///
/// * **At rest, zero ticks.** No tween in flight ⇒ `needs_tick` is `false`.
/// * **Hidden/closed surface animates nothing.** Even with a stale in-flight
///   tween, a surface marked not-visible reports `needs_tick == false`: a
///   collapsed popup or a closed window arms no animation clock at all.
///
/// ## MOTION-INFRA-3 — opt-in per-frame timing
///
/// When `MDE_FRAME_DEBUG` is set ([`crate::frame_timer::FRAME_DEBUG_ENV`]), the
/// animator's [`Animator::frame_tick`] yields a [`FrameSample`] carrying the
/// per-frame milliseconds for the surface, so a slow/stuttering motion is
/// loggable. The flag is read **once** at [`Animator::new`]/`Default` (via
/// [`FrameTimer::from_env`]); when unset the timer is the zero-cost
/// [`FrameTimer::Off`] variant and `frame_tick` never reads the clock or
/// allocates — default OFF, zero cost.
#[derive(Debug, Clone)]
pub struct Animator {
    tweens: HashMap<String, Tween>,
    /// Whether the surface this animator drives is on-screen. A not-visible
    /// surface arms no tick regardless of in-flight tweens (hidden popup,
    /// collapsed panel, closed window). Visible by default — most surfaces are.
    visible: bool,
    /// Opt-in per-frame instrumentation. Armed iff `MDE_FRAME_DEBUG` is set when
    /// the animator is built; [`FrameTimer::Off`] (zero cost) otherwise.
    frame_timer: FrameTimer,
}

impl Default for Animator {
    fn default() -> Self {
        Self {
            tweens: HashMap::new(),
            visible: true,
            // Read the env flag exactly once, here. `tick()`/`frame_tick()`
            // never touch the environment again.
            frame_timer: FrameTimer::from_env("animator"),
        }
    }
}

impl Animator {
    /// An empty animator (nothing animating), visible by default. Reads the
    /// `MDE_FRAME_DEBUG` gate once (see the type docs).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build an animator with an explicit frame-debug decision, bypassing the
    /// `MDE_FRAME_DEBUG` env probe. Lets a test (or a GUI's own debug toggle)
    /// arm per-frame timing without mutating process env — which is `unsafe`
    /// and racy under a test harness. Starts empty + visible.
    #[must_use]
    pub fn with_frame_debug(enabled: bool) -> Self {
        Self {
            tweens: HashMap::new(),
            visible: true,
            frame_timer: FrameTimer::with_enabled("animator", enabled),
        }
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
    /// (MOTION-PERF-1: zero idle wakeups). This is in-flight state only; it does
    /// **not** consider visibility. Gate a subscription on [`Animator::needs_tick`]
    /// instead, which also folds in whether the surface is on-screen.
    #[must_use]
    pub fn is_idle(&self, now: Instant) -> bool {
        self.tweens.values().all(|tw| tw.is_complete(now))
    }

    /// MOTION-INFRA-3 — mark whether the surface this animator drives is
    /// on-screen. A hidden/closed surface (collapsed popup, closed window) arms
    /// no tick even mid-animation, so [`Animator::needs_tick`] returns `false`
    /// while not visible. Returns `&mut self` so a consumer can chain it from a
    /// visibility/focus message handler.
    pub fn set_visible(&mut self, visible: bool) -> &mut Self {
        self.visible = visible;
        self
    }

    /// MOTION-INFRA-3 — is the surface currently considered on-screen?
    #[must_use]
    pub const fn is_visible(&self) -> bool {
        self.visible
    }

    /// MOTION-INFRA-3 — **the** tick predicate a `subscription()` gates on: an
    /// animation tick is needed only when something is in flight **and** the
    /// surface is visible. At rest, or while hidden/closed, this is `false`, so
    /// the per-frame clock is never armed (MOTION-PERF-1: zero idle/offscreen
    /// wakeups). Equivalent to `self.is_visible() && !self.is_idle(now)`.
    #[must_use]
    pub fn needs_tick(&self, now: Instant) -> bool {
        self.visible && !self.is_idle(now)
    }

    /// MOTION-INFRA-3 — is per-frame debug instrumentation armed
    /// (`MDE_FRAME_DEBUG` was set at construction)? Lets a consumer skip building
    /// a log line entirely when off.
    #[must_use]
    pub const fn frame_debug_enabled(&self) -> bool {
        self.frame_timer.is_enabled()
    }

    /// MOTION-INFRA-3 — record one animation frame at `now` for the optional
    /// `MDE_FRAME_DEBUG` instrumentation, returning the inter-frame
    /// [`FrameSample`] (per-frame milliseconds) when the gate is armed and a
    /// prior frame exists to diff against. When the flag is unset this is the
    /// zero-cost path: a single discriminant check returning `None`, with no
    /// clock read and no allocation. Call once per tick alongside [`Animator::gc`].
    pub fn frame_tick(&mut self, now: Instant) -> Option<FrameSample> {
        self.frame_timer.tick_at(now)
    }
}

// ── MOTION-INFRA-2 — reusable enter/exit/crossfade/hover helpers ──────────────
//
// One-call, token-driven, reduce-motion-aware bridges from "a panel mounted at
// `start`, it's now `now`" to the concrete [`RenderParams`] a themed widget
// applies (alpha / translate_y / scale). They are pure glue over the existing
// primitives — a [`Motion`] preset for the duration+easing, [`Tween::resolved`]
// for the reduce-motion duration cap, [`ease`] for the curve, and a
// [`Transition`] for the property mapping — so a panel never hand-rolls timing
// or re-implements the reduce-motion contract.
//
// The reduce-motion contract (Q32, mirrors [`Motion::resolved`]): every transform
// that would *move* a surface (slide, hover-lift) collapses to a pure opacity
// crossfade with NO translate/scale — motion is never the only cue and there is
// no positional thrash — and the ≤80 ms linear cap from [`Tween::resolved`]
// applies, so the static/final frame is reached almost immediately.

/// MOTION-INFRA-2 — opacity-only enter; returns the [`RenderParams`] at `now`.
///
/// The surface fades 0→1 over the [`Motion::panel_mount`] duration (Carbon
/// `moderate-02`, 240 ms; ≤80 ms under reduce-motion). No transform, so it's
/// identical with or without reduce-motion — a fade is already the
/// reduce-motion-safe primitive every other helper collapses to.
#[must_use]
pub fn fade_in(start: Instant, now: Instant, reduce_motion: bool) -> RenderParams {
    let motion = Motion::panel_mount();
    let tw = Tween::resolved(start, motion.duration, reduce_motion);
    let t = ease(tw.progress(now), motion_easing(motion, reduce_motion));
    Transition::FadeIn.params(t)
}

/// MOTION-INFRA-2 — fade-and-rise enter; returns the [`RenderParams`] at `now`.
///
/// The surface fades 0→1 while sliding up from `distance` px below to rest, over
/// the [`Motion::panel_mount`] duration. **Under reduce-motion the slide is
/// dropped** — it collapses to a pure [`fade_in`] crossfade (no `translate_y`, so
/// zero layout reflow / positional motion). `distance` defaults sensibly to
/// [`PANEL_MOUNT_TRANSLATE_Y_PX`] at the call site.
#[must_use]
pub fn slide_in(start: Instant, now: Instant, distance: f32, reduce_motion: bool) -> RenderParams {
    if reduce_motion {
        // Collapse the transform to a crossfade — opacity only, no movement.
        return fade_in(start, now, true);
    }
    let motion = Motion::panel_mount();
    let tw = Tween::resolved(start, motion.duration, false);
    let t = ease(tw.progress(now), motion.easing);
    Transition::SlideUp(distance).params(t)
}

/// MOTION-INFRA-2 — crossfade two surfaces; returns `(outgoing, incoming)`.
///
/// Both share the same `start`: the outgoing fades 1→0 and the incoming 0→1 over
/// the same [`Motion::dialog_mount`] duration (so a swap reads as one motion).
/// Both are opacity-only — the reduce-motion-safe primitive — so the ≤80 ms cap
/// is the only change under reduce-motion. This is the helper every other
/// transform collapses to under reduce-motion.
#[must_use]
pub fn crossfade(
    start: Instant,
    now: Instant,
    reduce_motion: bool,
) -> (RenderParams, RenderParams) {
    let motion = Motion::dialog_mount();
    let tw = Tween::resolved(start, motion.duration, reduce_motion);
    let t = ease(tw.progress(now), motion_easing(motion, reduce_motion));
    (Transition::FadeOut.params(t), Transition::FadeIn.params(t))
}

/// MOTION-INFRA-2 — hover lift; returns the [`RenderParams`] for `now`.
///
/// As `hovered` toggles, the surface rises `rise` px (negative `translate_y`) over
/// the [`Motion::hover`] duration (Carbon `fast-01`, 70 ms). `start` is when the
/// hover state last changed; `hovered` is the *target* (rising on enter, settling
/// back on leave). **Under reduce-motion the lift is dropped** — the surface stays
/// at rest with no transform (hover is conveyed by color/elevation tokens, not
/// motion).
#[must_use]
pub fn lift_on_hover(
    start: Instant,
    now: Instant,
    rise: f32,
    hovered: bool,
    reduce_motion: bool,
) -> RenderParams {
    let rest = RenderParams {
        alpha: 1.0,
        translate_y: 0.0,
        scale: 1.0,
    };
    if reduce_motion {
        // No positional motion under reduce-motion — stay at rest.
        return rest;
    }
    let motion = Motion::hover();
    let tw = Tween::resolved(start, motion.duration, false);
    let t = ease(tw.progress(now), motion.easing);
    // `hovered` drives the direction. On enter the offset runs rest → -rise as the
    // tween progresses; on leave it runs -rise → rest (the same tween, reversed).
    let translate_y = if hovered {
        lerp_f32(0.0, -rise, t)
    } else {
        lerp_f32(-rise, 0.0, t)
    };
    RenderParams {
        translate_y,
        ..rest
    }
}

/// MOTION-INFRA-2 — the easing a helper should use given `reduce_motion`. Mirrors
/// [`Motion::resolved`], which forces a linear crossfade under reduce-motion; full
/// motion keeps the preset's curve.
const fn motion_easing(motion: Motion, reduce_motion: bool) -> Easing {
    if reduce_motion {
        Easing::Linear
    } else {
        motion.easing
    }
}

// ── MOTION-TRANS-3 — keyed-diff list/table reveal ─────────────────────────────

/// MOTION-TRANS-3 — a keyed-diff reveal for a refreshing list/table.
///
/// On each refresh the consumer hands this the current set of stable **row keys**;
/// it diffs them against the previous frame and arms a [`slide_in`] reveal for
/// every **genuinely new** row, so an inserted row slides up + fades into place
/// while every row that was already on screen stays put — a periodic refresh
/// therefore never restrokes the whole list (and, paired with a stable
/// `scrollable` id on the consumer side, never jumps the scroll position). The
/// first sync is treated as the list simply *appearing*, so opening a panel does
/// not mass-reveal its whole backlog.
///
/// Removals are intentionally **not** animated here: the iced 0.13 fork has no
/// clip/opacity/transform widget to height-collapse a *dynamic-height* row, so a
/// vanished row is simply dropped on the next sync (the row-level reveal is the
/// expressible half; a collapse-on-remove would need a clip widget the toolkit
/// lacks). Pure state (no toolkit dep) — the consumer reads [`Self::row_params`]
/// in its `view` and applies the alpha/translate to its themed row.
///
/// Tick-driven like [`Animator`]: advance with [`Self::gc`] from one subscription
/// the consumer arms only while [`Self::is_idle`] is `false` (zero idle wakeups).
#[derive(Debug, Clone, Default)]
pub struct KeyedListReveal {
    /// row key → its in-flight reveal tween.
    entering: HashMap<String, Tween>,
    /// the row keys present as of the last [`Self::sync`].
    seen: std::collections::HashSet<String>,
    /// whether a first sync has happened (so the initial list doesn't mass-reveal).
    seeded: bool,
    reduce_motion: bool,
}

impl KeyedListReveal {
    /// A fresh, idle reveal. `reduce_motion` caps every reveal to the Carbon
    /// ≤80 ms crossfade and drops the slide (via [`slide_in`]'s contract).
    #[must_use]
    pub fn new(reduce_motion: bool) -> Self {
        Self {
            entering: HashMap::new(),
            seen: std::collections::HashSet::new(),
            seeded: false,
            reduce_motion,
        }
    }

    /// Diff this frame's `keys` (the current row keys) against the previous sync:
    /// arm a reveal for each key that is **new** (skipped on the very first sync),
    /// forget reveals for rows that vanished, and record the current set. Call
    /// once from the consumer's load/refresh handler with the freshly-loaded keys.
    pub fn sync<I, S>(&mut self, keys: I, now: Instant)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let current: std::collections::HashSet<String> = keys.into_iter().map(Into::into).collect();
        if self.seeded {
            for k in &current {
                if !self.seen.contains(k) {
                    self.entering.insert(
                        k.clone(),
                        Tween::resolved(now, Motion::panel_mount().duration, self.reduce_motion),
                    );
                }
            }
        }
        // Drop reveals for rows that are no longer present (removed before settling).
        self.entering.retain(|k, _| current.contains(k));
        self.seen = current;
        self.seeded = true;
    }

    /// The reveal [`RenderParams`] (alpha + `translate_y`) for the row keyed `key`
    /// at `now`: a slide-up + fade for a freshly-inserted row, or the fully-shown
    /// rest frame for a row that was already present (or any unknown key). Under
    /// reduce-motion the slide drops to a crossfade ([`slide_in`]).
    #[must_use]
    pub fn row_params(&self, key: &str, now: Instant) -> RenderParams {
        self.entering.get(key).map_or(
            RenderParams {
                alpha: 1.0,
                translate_y: 0.0,
                scale: 1.0,
            },
            |tw| {
                slide_in(
                    tw.start(),
                    now,
                    crate::motion::PANEL_MOUNT_TRANSLATE_Y_PX,
                    self.reduce_motion,
                )
            },
        )
    }

    /// Drop every settled reveal (call once per tick); returns the count still in
    /// flight so the consumer's subscription can stop when it reaches 0.
    pub fn gc(&mut self, now: Instant) -> usize {
        self.entering.retain(|_, tw| !tw.is_complete(now));
        self.entering.len()
    }

    /// `true` when no row is mid-reveal — the consumer stops ticking (zero idle
    /// wakeups).
    #[must_use]
    pub fn is_idle(&self, now: Instant) -> bool {
        self.entering.values().all(|tw| tw.is_complete(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::{PANEL_MOUNT_TRANSLATE_Y_PX, PULSE_MAX_SCALE};

    #[test]
    fn transition_params_map_progress_correctly() {
        // MOTION-INFRA-2 — each transition kind yields the right render params at
        // the endpoints (consumers apply alpha/translate_y/scale to themed widgets).
        assert_eq!(Transition::FadeIn.params(0.0).alpha, 0.0);
        assert_eq!(Transition::FadeIn.params(1.0).alpha, 1.0);
        assert_eq!(Transition::FadeOut.params(1.0).alpha, 0.0);
        // SlideUp: starts `distance` below, rests at 0 + fully opaque.
        let s0 = Transition::SlideUp(8.0).params(0.0);
        assert_eq!(s0.translate_y, 8.0);
        assert_eq!(s0.alpha, 0.0);
        let s1 = Transition::SlideUp(8.0).params(1.0);
        assert_eq!(s1.translate_y, 0.0);
        assert_eq!(s1.alpha, 1.0);
        // Lift rises (negative y); Press depresses scale.
        assert_eq!(Transition::Lift(6.0).params(1.0).translate_y, -6.0);
        assert!((Transition::Press(0.04).params(1.0).scale - 0.96).abs() < 1e-6);
        // Clamped: out-of-range t doesn't overshoot.
        assert_eq!(Transition::FadeIn.params(2.0).alpha, 1.0);
    }

    #[test]
    fn transition_outputs_are_compositor_safe_across_the_sweep() {
        // MOTION-PERF-2 — every transition kind, at every sampled progress, yields
        // only sane transform/opacity values (no relayout-forcing field exists on
        // the struct, and the values stay in the compositor-safe ranges).
        let kinds = [
            Transition::FadeIn,
            Transition::FadeOut,
            Transition::SlideUp(12.0),
            Transition::Lift(6.0),
            Transition::Press(0.04),
        ];
        for k in kinds {
            for i in 0..=20 {
                let t = i as f32 / 20.0;
                let p = k.params(t);
                assert!(
                    p.is_compositor_safe(),
                    "{k:?} at t={t} produced a non-compositor-safe frame: {p:?}"
                );
            }
            // Out-of-range progress is clamped, so even that stays safe.
            assert!(k.params(-1.0).is_compositor_safe());
            assert!(k.params(2.0).is_compositor_safe());
        }
    }

    #[test]
    fn pulse_clamps_period_to_the_flash_cap() {
        // MOTION-A11Y-3 — a too-fast pulse period is clamped up to the 3 Hz floor;
        // a slow period is left as requested.
        let t0 = Instant::now();
        let floor = Duration::from_millis(crate::motion::MIN_PULSE_PERIOD_MS);
        // 50 ms (20 Hz) requested → clamped to the 334 ms floor (≈3 Hz).
        let fast = LoopingTween::pulse(t0, Duration::from_millis(50));
        assert!(
            fast.hz() <= crate::motion::MAX_PULSE_HZ + 1e-3,
            "clamped pulse must not exceed the cap, got {} Hz",
            fast.hz()
        );
        // The clamp lands exactly on the floor period.
        assert!((fast.hz() - LoopingTween::starting_at(t0, floor).hz()).abs() < 1e-6);
        // A 2 s pulse (0.5 Hz) is well under the cap → unchanged.
        let slow = LoopingTween::pulse(t0, Duration::from_secs(2));
        assert!((slow.hz() - 0.5).abs() < 1e-3);
    }

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
    fn animator_needs_no_tick_at_rest() {
        // MOTION-INFRA-3 — the core idle-hardening contract: with nothing in
        // flight the animator reports "no ticks needed", so the subscription
        // never arms a per-frame clock (MOTION-PERF-1: zero idle wakeups).
        let t0 = Instant::now();
        let a = Animator::new();
        assert!(a.is_idle(t0), "empty animator is idle");
        assert!(a.is_visible(), "a fresh animator is visible by default");
        assert!(
            !a.needs_tick(t0),
            "at rest (no tween) ⇒ no tick needed even while visible"
        );
    }

    #[test]
    fn animator_needs_tick_only_while_in_flight_and_visible() {
        // MOTION-INFRA-3 — needs_tick == (visible AND in-flight). A tick arms
        // only when both hold; it stops the instant either drops out.
        let t0 = Instant::now();
        let mut a = Animator::new();
        a.start("fade", t0, Motion::panel_mount(), false); // 240 ms
        assert!(a.needs_tick(t0), "visible + in-flight ⇒ tick needed");
        // Once the tween settles, the tick stops even though still visible.
        let done = t0 + Duration::from_millis(300);
        assert!(a.is_idle(done));
        assert!(
            !a.needs_tick(done),
            "settled ⇒ no tick needed (visible but idle)"
        );
    }

    #[test]
    fn hidden_surface_arms_no_tick_even_mid_animation() {
        // MOTION-INFRA-3 — a hidden/closed popup animates nothing: with an
        // in-flight tween but the surface marked not-visible, needs_tick is
        // false, so a collapsed popup / closed window arms zero animation ticks.
        let t0 = Instant::now();
        let mut a = Animator::new();
        a.start("open", t0, Motion::panel_mount(), false); // in flight
        assert!(
            !a.is_idle(t0),
            "tween is in flight (idle-state is independent of visibility)"
        );
        assert!(a.needs_tick(t0), "visible + in-flight ⇒ tick needed");
        // Hide the surface: the in-flight tween is unchanged but no tick arms.
        a.set_visible(false);
        assert!(!a.is_visible());
        assert!(
            !a.needs_tick(t0),
            "hidden surface ⇒ zero ticks even mid-animation"
        );
        assert!(
            !a.is_idle(t0),
            "hiding does not retroactively complete the tween"
        );
        // Re-showing it resumes ticking while still in flight.
        a.set_visible(true);
        assert!(
            a.needs_tick(t0),
            "re-shown + still in-flight ⇒ tick resumes"
        );
    }

    #[test]
    fn frame_debug_flag_default_off_is_zero_cost() {
        // MOTION-INFRA-3 — the MDE_FRAME_DEBUG gate is read once at construction
        // and OFF by default: frame_tick yields no samples and reads no clock.
        let t0 = Instant::now();
        let mut a = Animator::with_frame_debug(false);
        assert!(
            !a.frame_debug_enabled(),
            "debug must be OFF unless explicitly armed"
        );
        for i in 0..5 {
            assert_eq!(
                a.frame_tick(t0 + Duration::from_millis(i * 16)),
                None,
                "OFF frame_tick yields nothing"
            );
        }
    }

    #[test]
    fn frame_debug_flag_when_armed_reports_per_frame_ms() {
        // MOTION-INFRA-3 — armed, frame_tick reports the per-frame interval so a
        // developer can log per-frame milliseconds. The flag is gated (only the
        // armed animator samples) — proving it's an opt-in switch, not always-on.
        let t0 = Instant::now();
        let mut a = Animator::with_frame_debug(true);
        assert!(a.frame_debug_enabled(), "explicitly armed");
        // First frame: counted, but no predecessor to diff against.
        assert_eq!(a.frame_tick(t0), None, "first frame has no interval");
        // Second frame, 16 ms later → a sample carrying the per-frame ms.
        let s = a
            .frame_tick(t0 + Duration::from_millis(16))
            .expect("armed timer yields a sample after the first frame");
        assert_eq!(s.surface, "animator");
        assert_eq!(s.interval, Duration::from_millis(16));
        assert!(
            (s.interval.as_secs_f64() * 1000.0 - 16.0).abs() < 1e-6,
            "interval is the per-frame milliseconds"
        );
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

    #[test]
    fn shimmer_alpha_oscillates_with_motion_and_is_static_under_reduce_motion() {
        // MOTION-NET-2: with motion ON the skeleton breathes between the dim and
        // bright bounds across a phase cycle.
        let lo = shimmer_alpha(0.0, false);
        let mid = shimmer_alpha(0.5, false);
        let hi_again = shimmer_alpha(1.0, false);
        assert!((lo - SKELETON_ALPHA_DIM).abs() < 1e-3, "phase 0 = dim");
        assert!(
            (mid - SKELETON_ALPHA_BRIGHT).abs() < 1e-3,
            "phase 0.5 = bright"
        );
        assert!(
            (hi_again - SKELETON_ALPHA_DIM).abs() < 1e-3,
            "phase 1 back to dim (ping-pong)"
        );
        // Every motion-on alpha stays within the bounds.
        for i in 0..=10 {
            let a = shimmer_alpha(i as f32 / 10.0, false);
            assert!((SKELETON_ALPHA_DIM..=SKELETON_ALPHA_BRIGHT).contains(&a));
        }
        // reduce-motion → STATIC mid grey regardless of phase (the a11y contract).
        let r0 = shimmer_alpha(0.0, true);
        let r5 = shimmer_alpha(0.5, true);
        assert_eq!(r0, r5, "reduce-motion alpha is phase-independent");
        assert!((r0 - (SKELETON_ALPHA_DIM + SKELETON_ALPHA_BRIGHT) / 2.0).abs() < 1e-6);
    }

    // ── MOTION-INFRA-2 — fade_in / slide_in / crossfade / lift_on_hover ────────

    #[test]
    fn fade_in_runs_zero_to_one_over_panel_mount_duration() {
        let t0 = Instant::now();
        let full = Motion::panel_mount().duration; // 240 ms
        assert!(fade_in(t0, t0, false).alpha < 1e-4, "starts transparent");
        let end = fade_in(t0, t0 + full, false);
        assert!((end.alpha - 1.0).abs() < 1e-4, "ends opaque");
        // A fade never moves or scales the surface.
        assert_eq!(end.translate_y, 0.0);
        assert_eq!(end.scale, 1.0);
    }

    #[test]
    fn fade_in_reduce_motion_caps_at_80ms() {
        let t0 = Instant::now();
        let cap = Duration::from_millis(crate::motion::REDUCE_MOTION_CAP_MS);
        // At the 80 ms cap the fade is already complete under reduce-motion.
        assert!((fade_in(t0, t0 + cap, true).alpha - 1.0).abs() < 1e-4);
        // Past the panel_mount duration it is obviously done either way.
        assert!((fade_in(t0, t0 + Motion::panel_mount().duration, true).alpha - 1.0).abs() < 1e-4);
    }

    #[test]
    fn slide_in_rises_and_fades_with_motion_on() {
        let t0 = Instant::now();
        let dist = PANEL_MOUNT_TRANSLATE_Y_PX;
        let s0 = slide_in(t0, t0, dist, false);
        assert_eq!(s0.translate_y, dist, "starts `distance` px below");
        assert!(s0.alpha < 1e-4, "starts transparent");
        let s1 = slide_in(t0, t0 + Motion::panel_mount().duration, dist, false);
        assert!((s1.translate_y).abs() < 1e-4, "rests at 0");
        assert!((s1.alpha - 1.0).abs() < 1e-4, "ends opaque");
    }

    #[test]
    fn slide_in_collapses_to_crossfade_under_reduce_motion() {
        // Reduce-motion drops the slide → no positional motion (zero translate_y)
        // at every sampled frame; only opacity changes (== fade_in).
        let t0 = Instant::now();
        let dist = 12.0;
        for ms in [0, 20, 40, 80, 240] {
            let now = t0 + Duration::from_millis(ms);
            let r = slide_in(t0, now, dist, true);
            assert_eq!(r.translate_y, 0.0, "no slide under reduce-motion at {ms}ms");
            assert_eq!(r.scale, 1.0);
            // Identical to the pure fade path.
            assert!((r.alpha - fade_in(t0, now, true).alpha).abs() < 1e-6);
        }
    }

    #[test]
    fn crossfade_is_complementary_and_opacity_only() {
        let t0 = Instant::now();
        let dur = Motion::dialog_mount().duration;
        // Start: outgoing fully visible, incoming hidden.
        let (out0, in0) = crossfade(t0, t0, false);
        assert!((out0.alpha - 1.0).abs() < 1e-4);
        assert!(in0.alpha < 1e-4);
        // Mid: the two alphas sum to ~1 (a true crossfade, no flicker/black).
        let (outm, inm) = crossfade(t0, t0 + dur / 2, false);
        assert!(
            (outm.alpha + inm.alpha - 1.0).abs() < 1e-3,
            "alphas sum to 1"
        );
        // End: outgoing gone, incoming full.
        let (out1, in1) = crossfade(t0, t0 + dur, false);
        assert!(out1.alpha < 1e-4);
        assert!((in1.alpha - 1.0).abs() < 1e-4);
        // Never any transform on either side.
        assert_eq!(out0.translate_y, 0.0);
        assert_eq!(in0.scale, 1.0);
    }

    #[test]
    fn lift_on_hover_rises_then_settles_and_is_static_under_reduce_motion() {
        let t0 = Instant::now();
        let rise = 6.0;
        let dur = Motion::hover().duration; // 70 ms
                                            // Enter: at rest at t=0, lifted by -rise at the end.
        assert!(lift_on_hover(t0, t0, rise, true, false).translate_y.abs() < 1e-4);
        let lifted = lift_on_hover(t0, t0 + dur, rise, true, false);
        assert!((lifted.translate_y + rise).abs() < 1e-4, "lifts to -rise");
        // Leave: starts at the lifted offset, returns to rest.
        let leaving0 = lift_on_hover(t0, t0, rise, false, false);
        assert!(
            (leaving0.translate_y + rise).abs() < 1e-4,
            "leave starts lifted"
        );
        let settled = lift_on_hover(t0, t0 + dur, rise, false, false);
        assert!(settled.translate_y.abs() < 1e-4, "leave settles to rest");
        // Reduce-motion: never any vertical motion, regardless of hovered/time.
        for hovered in [true, false] {
            for ms in [0, 35, 70, 200] {
                let r = lift_on_hover(t0, t0 + Duration::from_millis(ms), rise, hovered, true);
                assert_eq!(r.translate_y, 0.0, "no lift under reduce-motion");
                assert_eq!(r.alpha, 1.0);
                assert_eq!(r.scale, 1.0);
            }
        }
    }

    // ── MOTION-TRANS-3 — KeyedListReveal ───────────────────────────────────────

    #[test]
    fn first_sync_never_mass_reveals_the_initial_list() {
        // Opening a panel (first sync) treats the whole set as already present —
        // every row is at rest, nothing strobes.
        let t0 = Instant::now();
        let mut r = KeyedListReveal::new(false);
        r.sync(["a", "b", "c"], t0);
        assert!(r.is_idle(t0), "first sync arms no reveal");
        for k in ["a", "b", "c"] {
            let p = r.row_params(k, t0);
            assert!((p.alpha - 1.0).abs() < 1e-4, "{k} starts at rest");
            assert_eq!(p.translate_y, 0.0);
        }
    }

    #[test]
    fn a_newly_inserted_row_reveals_then_settles() {
        let t0 = Instant::now();
        let mut r = KeyedListReveal::new(false);
        r.sync(["a", "b"], t0); // seed
                                // A later refresh adds "c": only "c" reveals; "a"/"b" stay put.
        r.sync(["a", "b", "c"], t0);
        assert!(!r.is_idle(t0), "the inserted row is mid-reveal");
        let c0 = r.row_params("c", t0);
        assert!(
            c0.alpha < 1.0 && c0.translate_y > 0.0,
            "c starts low + faded"
        );
        assert_eq!(
            r.row_params("a", t0).translate_y,
            0.0,
            "existing row never moves"
        );
        assert!((r.row_params("a", t0).alpha - 1.0).abs() < 1e-4);
        // After the panel-mount duration the reveal settles + the tick can stop.
        let done = t0 + Motion::panel_mount().duration;
        let c1 = r.row_params("c", done);
        assert!((c1.alpha - 1.0).abs() < 1e-4 && c1.translate_y.abs() < 1e-4);
        assert!(r.is_idle(done));
        assert_eq!(r.gc(done), 0, "gc clears the settled reveal");
    }

    #[test]
    fn a_removed_row_drops_its_reveal_and_unchanged_rows_dont_restrobe() {
        let t0 = Instant::now();
        let mut r = KeyedListReveal::new(false);
        r.sync(["a", "b"], t0);
        r.sync(["a", "b", "c"], t0); // c revealing
        assert!(!r.is_idle(t0));
        // c is removed before it settled (accept/reject) → its reveal is dropped,
        // and the steady rows a/b never animate on the refresh.
        r.sync(["a", "b"], t0);
        assert!(
            r.is_idle(t0),
            "removing the only revealing row leaves nothing in flight"
        );
        assert_eq!(
            r.row_params("c", t0).alpha,
            1.0,
            "a dropped row reads as rest"
        );
        assert_eq!(r.row_params("a", t0).translate_y, 0.0);
    }

    #[test]
    fn reduce_motion_reveal_drops_the_slide_and_caps_at_80ms() {
        let t0 = Instant::now();
        let mut r = KeyedListReveal::new(true);
        r.sync(["a"], t0);
        r.sync(["a", "b"], t0);
        // No positional slide under reduce-motion (crossfade only)...
        for ms in [0, 40, 80] {
            let p = r.row_params("b", t0 + Duration::from_millis(ms));
            assert_eq!(p.translate_y, 0.0, "no slide under reduce-motion at {ms}ms");
        }
        // ...and it settles within the Carbon 80 ms cap.
        assert!(r.is_idle(t0 + Duration::from_millis(80) + Duration::from_millis(1)));
    }
}
