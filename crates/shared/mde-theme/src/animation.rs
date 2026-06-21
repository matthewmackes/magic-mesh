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

/// MOTION-NET-2 — the shimmer **highlight intensity** `0.0..=1.0` for a point at
/// normalized position `pos` (`0.0` = leftmost edge of the placeholder, `1.0` =
/// rightmost) given the current cycle `phase` (`0.0..=1.0`, from a
/// [`LoopingTween::phase`] over [`crate::motion::list::SHIMMER_PERIOD_MS`]).
///
/// The highlight is a soft band that sweeps left→right once per cycle: it peaks
/// (`1.0`) where the band center coincides with `pos`, falling off smoothly to
/// `0.0` for points more than [`SHIMMER_BAND_HALF_WIDTH`] away. The band center
/// travels from before the left edge to past the right edge so every column
/// gets swept exactly once per cycle (no abrupt wrap discontinuity).
///
/// Pure math — no toolkit dep; the consumer maps the returned intensity to a
/// lightened tint over the skeleton's base grey ([`lerp_f32`] / a color lerp).
#[must_use]
pub fn shimmer_highlight(phase: f32, pos: f32) -> f32 {
    let phase = phase.clamp(0.0, 1.0);
    let pos = pos.clamp(0.0, 1.0);
    // Sweep the band center across `[-half, 1 + half]` so the band enters from
    // off the left edge and fully exits past the right edge within one cycle.
    let span = 1.0 + 2.0 * SHIMMER_BAND_HALF_WIDTH;
    let center = -SHIMMER_BAND_HALF_WIDTH + phase * span;
    let dist = (pos - center).abs();
    if dist >= SHIMMER_BAND_HALF_WIDTH {
        0.0
    } else {
        // Smooth cosine falloff: 1.0 at the center → 0.0 at the band edge.
        let x = dist / SHIMMER_BAND_HALF_WIDTH; // 0..1
        let intensity = 0.5 * (1.0 + (std::f32::consts::PI * x).cos());
        intensity.clamp(0.0, 1.0)
    }
}

/// MOTION-NET-2 — half-width of the shimmer highlight band in normalized
/// placeholder-width units. A point further than this from the band center gets
/// no highlight. ~0.35 gives a band ~70 % of the placeholder width — broad
/// enough to read as a soft sheen rather than a hard line.
pub const SHIMMER_BAND_HALF_WIDTH: f32 = 0.35;

/// MOTION-NET-2 — peak extra lightness the shimmer adds over the skeleton's base
/// grey, as a `0.0..=1.0` lerp factor toward the highlight color. Kept subtle
/// (Carbon skeleton shimmer is a gentle sheen, not a flash).
pub const SHIMMER_PEAK_LIFT: f32 = 0.45;

/// MOTION-NET-2 — the per-column lift factor (`0.0..=1.0`) to lerp the skeleton
/// base toward its highlight color, for a column at normalized position `pos`.
/// With `reduce_motion` the sweep is dropped entirely and a flat `0.0` is
/// returned (static grey, no shimmer — the Q32/reduce-motion contract). `phase`
/// is the current cycle phase from a [`LoopingTween`].
#[must_use]
pub fn shimmer_lift(phase: f32, pos: f32, reduce_motion: bool) -> f32 {
    if reduce_motion {
        0.0
    } else {
        shimmer_highlight(phase, pos) * SHIMMER_PEAK_LIFT
    }
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
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderParams {
    /// Opacity multiplier `0.0..=1.0`.
    pub alpha: f32,
    /// Vertical offset in px (negative = up).
    pub translate_y: f32,
    /// Scale multiplier (1.0 = natural size).
    pub scale: f32,
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

/// MOTION-INFRA-2 — the crossfade pair: the [`RenderParams`] for the *outgoing*
/// and *incoming* content during a content swap. Both share the same eased
/// progress so the alphas always sum to ~1.0 (no flash of empty/overlapping
/// fully-opaque content). Consumers render both stacked and apply each alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crossfade {
    /// The content leaving (alpha 1→0).
    pub out: RenderParams,
    /// The content arriving (alpha 0→1).
    pub incoming: RenderParams,
}

/// MOTION-INFRA-2 — fade an element **in** (opacity 0→1) at eased progress
/// `t`. Pure presentation: opacity only, never layout — Wayland/compositor
/// friendly. `reduce_motion` is honored by the caller routing `t` through a
/// [`Tween::resolved`]/[`Animator`] (whose duration is already capped to the
/// ≤80 ms crossfade), so this helper just maps the resolved progress to alpha.
#[must_use]
pub fn fade_in(t: f32) -> RenderParams {
    Transition::FadeIn.params(t)
}

/// MOTION-INFRA-2 — fade an element **out** (opacity 1→0) at eased progress `t`.
/// Opacity only.
#[must_use]
pub fn fade_out(t: f32) -> RenderParams {
    Transition::FadeOut.params(t)
}

/// MOTION-INFRA-2 — slide an element **in**: fade (0→1) while translating up from
/// `distance` px below to rest. Under `reduce_motion` the slide is dropped (the
/// Q32 contract: collapse to a crossfade — opacity only, no movement) so the
/// element still fades but never moves. `t` is the eased progress.
///
/// `translate_y` is a transform offset, NOT a layout property — apply it as the
/// element's own offset so sibling layout never reflows (acceptance: no layout
/// reflow during the transition).
#[must_use]
pub fn slide_in(t: f32, distance: f32, reduce_motion: bool) -> RenderParams {
    if reduce_motion {
        // Crossfade-only: keep the alpha ramp, drop the movement.
        Transition::FadeIn.params(t)
    } else {
        Transition::SlideUp(distance).params(t)
    }
}

/// MOTION-INFRA-2 — crossfade old→new content at eased progress `t`: the outgoing
/// content fades 1→0 while the incoming fades 0→1, sharing one clock. This is the
/// reduce-motion-safe swap primitive (it is *already* opacity-only, so it is
/// identical with or without reduce-motion — the Q32 contract collapses every
/// transition to exactly this crossfade).
#[must_use]
pub fn crossfade(t: f32) -> Crossfade {
    Crossfade {
        out: Transition::FadeOut.params(t),
        incoming: Transition::FadeIn.params(t),
    }
}

/// MOTION-INFRA-2 — hover lift: raise an element `rise` px (a transform offset,
/// never a layout change) as the hover tween progresses `t` (0=rest → 1=lifted).
/// Under `reduce_motion` the lift is dropped (no movement) — hover is decorative
/// motion, so reduce-motion renders the resting frame. `t` is the eased progress
/// of a [`Motion::hover`]-driven tween.
#[must_use]
pub fn lift_on_hover(t: f32, rise: f32, reduce_motion: bool) -> RenderParams {
    if reduce_motion {
        Transition::Lift(rise).params(0.0)
    } else {
        Transition::Lift(rise).params(t)
    }
}

// ─────────────────────────────────────────────────────────────────────────
// MOTION-FEEDBACK-2 — cards / lists / tables: selection + row-state feedback.
//
// Selection and row hover share ONE pure decision (mirroring the FEEDBACK-1
// `Feedback`/`FeedbackStyle` vocabulary for buttons): a row is `Rest`,
// `Hover`ed, `Selected`, or `Selected`+hovered, and that maps to a single
// Carbon accent-wash alpha + an optional accent selection rail. Centralizing
// it here (pure, toolkit-free) is the whole point — every selectable surface
// resolves selection identically, and the math is unit-testable without a GUI.
// ─────────────────────────────────────────────────────────────────────────

/// MOTION-FEEDBACK-2 — the interaction state of a *row/card/cell* in a
/// selectable list/table/grid, distilled the same way FEEDBACK-1's `Feedback`
/// distills a button. Selection is a persistent state that composes with the
/// transient hover (a selected row can also be hovered).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowState {
    /// Not selected, pointer away.
    Rest,
    /// Not selected, pointer over the row.
    Hover,
    /// Selected, pointer away.
    Selected,
    /// Selected and the pointer is also over it.
    SelectedHover,
}

impl RowState {
    /// Build a row state from the two orthogonal inputs every list view already
    /// tracks: whether this row is the selected one, and whether it's hovered.
    #[must_use]
    pub fn new(selected: bool, hovered: bool) -> Self {
        match (selected, hovered) {
            (true, true) => Self::SelectedHover,
            (true, false) => Self::Selected,
            (false, true) => Self::Hover,
            (false, false) => Self::Rest,
        }
    }

    /// Is this row selected (in either selected variant)?
    #[must_use]
    pub fn is_selected(self) -> bool {
        matches!(self, Self::Selected | Self::SelectedHover)
    }

    /// The Carbon accent-wash alpha for this state, picking the matching
    /// `Palette` tint token's alpha so the row wash mirrors the palette:
    /// rest 0.0, hover [`Palette::hover_tint`] (0.08), selected
    /// [`Palette::selected_tint`] (0.16), selected+hover
    /// [`Palette::selected_hover_tint`] (0.20). The caller composites the accent
    /// at this alpha over the row's base background.
    #[must_use]
    pub fn wash_alpha(self) -> f32 {
        match self {
            Self::Rest => 0.0,
            Self::Hover => 0.08,
            Self::Selected => 0.16,
            Self::SelectedHover => 0.20,
        }
    }
}

/// MOTION-FEEDBACK-2 — the resolved visual deltas for a selectable row: the
/// Carbon accent-wash alpha laid over the row background, and the width (px) of
/// the leading accent selection rail (Carbon's selected-row indicator). Pure
/// data — list/table builders fold it into their themed `container::Style`, so
/// every selectable surface paints selection the same way.
///
/// Reduce-motion does not change the *state* read (the wash + rail still flip on
/// selection — selection is information, not decoration); the FEEDBACK-2
/// movement that reduce-motion collapses is the staggered *reveal* (see
/// [`stagger`]) and the selection-slide *animation*, not this resting style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowStyle {
    /// Alpha of the Carbon accent wash over the row's base background
    /// (`0.0` at rest). From [`RowState::wash_alpha`].
    pub wash_alpha: f32,
    /// Width in px of the leading accent selection rail; `0.0` when unselected.
    /// Carbon marks a selected row with a 2 px accent bar on its leading edge.
    pub rail_px: f32,
}

/// MOTION-FEEDBACK-2 — width (px) of the accent selection rail Carbon draws on a
/// selected row's leading edge. Matches the FEEDBACK-1 focus-ring width so the
/// shell's accent indicators read at one consistent weight.
pub const SELECTION_RAIL_PX: f32 = 2.0;

impl RowStyle {
    /// Resolve the resting selection/row style for `state`.
    #[must_use]
    pub fn resolve(state: RowState) -> Self {
        Self {
            wash_alpha: state.wash_alpha(),
            rail_px: if state.is_selected() {
                SELECTION_RAIL_PX
            } else {
                0.0
            },
        }
    }
}

/// MOTION-FEEDBACK-2 — staggered list/table reveal timing. Pure math over the
/// existing `mde-theme::motion::list` tokens (`STAGGER_STEP_MS`, `STAGGER_CAP`,
/// `STAGGER_REVEAL_MS`): item `i` starts revealing after a capped per-item
/// delay and fades/slides in over `STAGGER_REVEAL_MS`, so a freshly-loaded list
/// cascades in rather than snapping. Capped at `STAGGER_CAP` items so a long
/// list never crawls, and the whole cascade is bounded ≤500 ms.
///
/// Consumers feed each row's eased reveal `progress` into the INFRA-2
/// [`slide_in`]/[`fade_in`] helpers (opacity/transform only). Under
/// reduce-motion the stagger collapses (every row reveals at once, progress
/// `1.0`) per the Q32 contract — selection still works, the cascade does not.
pub mod stagger {
    use crate::motion::list::{STAGGER_CAP, STAGGER_REVEAL_MS, STAGGER_STEP_MS};

    /// The start delay (ms) before row `index` begins its reveal. Capped at
    /// `STAGGER_CAP - 1` steps so rows at/after the cap all start together — the
    /// tail of a long list reveals as one block instead of crawling. With the
    /// default tokens (step 20 ms, cap 8) the last per-item delay is 140 ms.
    #[must_use]
    pub fn delay_ms(index: usize) -> u32 {
        let capped = index.min(STAGGER_CAP.saturating_sub(1));
        (capped as u32) * STAGGER_STEP_MS
    }

    /// Total wall-clock span (ms) of the whole cascade: the last-starting row's
    /// delay plus one reveal duration. This is what the consumer's tick must run
    /// for; the design caps it ≤500 ms (asserted in tests).
    #[must_use]
    pub fn total_ms() -> u32 {
        delay_ms(STAGGER_CAP.saturating_sub(1)) + STAGGER_REVEAL_MS
    }

    /// Linear reveal progress `0.0..=1.0` for row `index` at `elapsed_ms` since
    /// the cascade began. `0.0` before the row's delay, ramping to `1.0` over
    /// `STAGGER_REVEAL_MS`. Under `reduce_motion` every row is fully revealed
    /// immediately (`1.0`) — the cascade collapses but the content still appears.
    ///
    /// The returned value is *linear*; the consumer eases it (e.g.
    /// [`crate::animation::ease`] with `EaseOut`) before feeding
    /// [`crate::animation::slide_in`]/[`fade_in`].
    #[must_use]
    pub fn reveal_progress(index: usize, elapsed_ms: u32, reduce_motion: bool) -> f32 {
        if reduce_motion {
            return 1.0;
        }
        let start = delay_ms(index);
        if elapsed_ms <= start {
            return 0.0;
        }
        let into = elapsed_ms - start;
        if into >= STAGGER_REVEAL_MS {
            1.0
        } else {
            into as f32 / STAGGER_REVEAL_MS as f32
        }
    }

    /// Whether the cascade is still animating at `elapsed_ms` (some row hasn't
    /// finished revealing). The consumer gates its reveal tick on this so the
    /// subscription stops the instant the last row settles — no idle animation
    /// (MOTION-PERF-1). Always `false` under `reduce_motion` (nothing animates).
    #[must_use]
    pub fn is_animating(elapsed_ms: u32, reduce_motion: bool) -> bool {
        !reduce_motion && elapsed_ms < total_ms()
    }
}

/// MOTION-TRANS-1 — route/panel switch transition phase. When the operator
/// switches Workbench views (a `View::Panel`/`View::Group` change) the incoming
/// body crosses in over the [`Motion::route_switch`] duration (Carbon
/// `moderate-02` 240 ms, productive entrance). This is the *pure* phase math —
/// "how far into the switch am I, what render params does the body take, has it
/// settled" — composed from the existing INFRA-2 helpers ([`crate::crossfade`] /
/// [`crate::slide_in`]) and the [`Motion::route_switch`] token; it re-implements
/// no motion math of its own (it reuses [`ease`] + [`slide_in`] like
/// [`stagger`]). The consumer holds a start [`Instant`] (set on the switch) and a
/// now [`Instant`] (advanced by its idle-gated tick), passes the elapsed ms here,
/// and applies the returned params to the body wrapper.
///
/// Reduce-motion: the contract collapses every transition to a ≤80 ms crossfade
/// (opacity only, no movement). Because the iced-0.13 libcosmic fork has no
/// per-element opacity widget (the same limitation FEEDBACK-2's reveal recorded),
/// the *runtime* body transition is rendered on the transform channel only and
/// the true opacity blend is deferred; under reduce-motion that transform is
/// dropped, leaving an instant switch — exactly the reduce-motion intent.
pub mod route {
    use super::{ease, slide_in, RenderParams};
    use crate::motion::Easing;
    use crate::motion::Motion;

    /// The distance (px) the incoming body slides up into place during a route
    /// switch — a short Carbon micro-interaction rise so the switch reads as an
    /// intentional settle, never a long fly-in. Matches the FEEDBACK-2 row-reveal
    /// slide so the shell's enter motions share one travel.
    pub const SWITCH_SLIDE_PX: f32 = 8.0;

    /// Total wall-clock span (ms) of a route-switch transition: the
    /// [`Motion::route_switch`] duration. The consumer's idle-gated tick runs
    /// for exactly this long, then stops (no idle animation — MOTION-PERF-1).
    #[must_use]
    pub fn total_ms() -> u32 {
        u32::try_from(Motion::route_switch().duration.as_millis()).unwrap_or(u32::MAX)
    }

    /// Linear switch progress `0.0..=1.0` at `elapsed_ms` since the switch began:
    /// `0.0` at the instant of the switch, ramping to `1.0` over [`total_ms`].
    /// Under `reduce_motion` the switch is instant (`1.0` immediately) — the body
    /// appears at rest with no transition (the Q32 collapse, made concrete by the
    /// fork's missing opacity widget; see the module note).
    #[must_use]
    pub fn progress(elapsed_ms: u32, reduce_motion: bool) -> f32 {
        if reduce_motion {
            return 1.0;
        }
        let total = total_ms();
        if total == 0 || elapsed_ms >= total {
            1.0
        } else {
            elapsed_ms as f32 / total as f32
        }
    }

    /// The render params for the incoming body at `elapsed_ms` since the switch:
    /// the INFRA-2 [`slide_in`] entrance (fade ramp + slide up from
    /// [`SWITCH_SLIDE_PX`] below) eased with the [`Motion::route_switch`] curve
    /// (Carbon productive ease-out, so the body decelerates into place). The
    /// `alpha` channel is computed but unused by the fork-limited runtime wrapper
    /// (deferred until an opacity widget lands); `translate_y` is the live
    /// transform channel. Under `reduce_motion` the slide is dropped (no
    /// movement), leaving the body at rest.
    #[must_use]
    pub fn body_params(elapsed_ms: u32, reduce_motion: bool) -> RenderParams {
        let t = ease(progress(elapsed_ms, reduce_motion), Easing::EaseOut);
        slide_in(t, SWITCH_SLIDE_PX, reduce_motion)
    }

    /// Whether the switch transition is still animating at `elapsed_ms` (the
    /// body hasn't finished crossing in). The consumer gates its transition tick
    /// on this so the subscription stops the instant the body settles — no idle
    /// animation (MOTION-PERF-1). Always `false` under `reduce_motion` (the
    /// switch is instant, nothing animates).
    #[must_use]
    pub fn is_animating(elapsed_ms: u32, reduce_motion: bool) -> bool {
        !reduce_motion && elapsed_ms < total_ms()
    }
}

/// MOTION-FEEDBACK-3 — the **one** popup / menu / drawer / Hub enter-exit
/// vocabulary. The launcher (Application Menu), the power menu, the Notification
/// Hub, and Workbench dialogs/drawers all open + close on this single preset so
/// the whole shell shares one motion idiom (folds in APPS-FX-1 + NOTIFY-FX-1).
///
/// **Open** (enter): fade in (alpha 0→1) while the surface grows from
/// `1 − POPUP_SCALE_DELTA` (≈0.96×) to 1.0× — a gentle Carbon "grow from origin"
/// fade-scale over [`Motion::popup`] (Carbon `moderate-01`, 150 ms ease-out).
/// **Close** (exit): the reverse — fade out (alpha 1→0) while it shrinks back to
/// `1 − POPUP_SCALE_DELTA`.
///
/// **Reduce-motion ⇒ crossfade only.** Under reduce-motion the scale channel is
/// dropped (the surface never moves/scales — `scale == 1.0` at every progress);
/// only the alpha crossfade survives, capped to the ≤80 ms cap by the consumer
/// routing its clock through [`Tween::resolved`] / [`Animator`]. This re-uses the
/// INFRA-2 [`fade_in`]/[`fade_out`] alpha math + the [`lerp_f32`] scale lerp; it
/// authors no new easing or motion math of its own (it mirrors [`route`]).
///
/// The consumer holds an [`Animator`] (started on open/close with
/// [`Motion::popup`]) and reads [`Animator::value`] for the eased progress, then
/// calls [`enter_params`](popup::enter_params) / [`exit_params`](popup::exit_params)
/// to get the [`RenderParams`] it applies to the popup wrapper (alpha → a
/// container/text color alpha, scale → widget size). The tick subscription is
/// idle-parked via [`Animator::is_idle`] (zero redraw at rest — MOTION-PERF-1).
pub mod popup {
    use super::{fade_in, fade_out, lerp_f32, RenderParams};
    use crate::motion::POPUP_SCALE_DELTA;

    /// The scale a popup starts at when opening (and ends at when closing):
    /// `1.0 − POPUP_SCALE_DELTA` (≈0.96×). The single source for the open/close
    /// scale endpoints so every popup surface grows/shrinks by the same amount.
    #[must_use]
    pub fn start_scale() -> f32 {
        1.0 - POPUP_SCALE_DELTA
    }

    /// MOTION-FEEDBACK-3 — the **open / enter** render params at eased progress
    /// `t` (`0.0` = just opened, `1.0` = fully open). Fade in (alpha 0→1) while
    /// scaling from [`start_scale`] up to 1.0. Under `reduce_motion` the scale is
    /// dropped (the surface stays at 1.0× — crossfade only, no movement), leaving
    /// just the alpha ramp.
    #[must_use]
    pub fn enter_params(t: f32, reduce_motion: bool) -> RenderParams {
        let alpha = fade_in(t).alpha;
        let scale = if reduce_motion {
            1.0
        } else {
            lerp_f32(start_scale(), 1.0, t)
        };
        RenderParams {
            alpha,
            translate_y: 0.0,
            scale,
        }
    }

    /// MOTION-FEEDBACK-3 — the **close / exit** render params at eased progress
    /// `t` (`0.0` = fully open, `1.0` = fully closed). The reverse of
    /// [`enter_params`]: fade out (alpha 1→0) while scaling from 1.0 back down to
    /// [`start_scale`]. Under `reduce_motion` the scale is dropped (crossfade
    /// only).
    #[must_use]
    pub fn exit_params(t: f32, reduce_motion: bool) -> RenderParams {
        let alpha = fade_out(t).alpha;
        let scale = if reduce_motion {
            1.0
        } else {
            lerp_f32(1.0, start_scale(), t)
        };
        RenderParams {
            alpha,
            translate_y: 0.0,
            scale,
        }
    }
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
    fn fade_in_out_are_opacity_only_and_complementary() {
        // MOTION-INFRA-2 — enter/exit helpers touch alpha only (never translate /
        // scale), so they can never cause a layout reflow.
        let fin = fade_in(0.3);
        assert!((fin.alpha - 0.3).abs() < 1e-6);
        assert_eq!(fin.translate_y, 0.0);
        assert_eq!(fin.scale, 1.0);
        // At every progress fade_in + fade_out alphas sum to 1.0.
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            assert!((fade_in(t).alpha + fade_out(t).alpha - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn slide_in_fades_and_slides_then_collapses_under_reduce_motion() {
        // MOTION-INFRA-2 — full motion: starts `distance` below + transparent,
        // rests at offset 0 + opaque.
        let start = slide_in(0.0, 8.0, false);
        assert_eq!(start.alpha, 0.0);
        assert_eq!(start.translate_y, 8.0);
        let end = slide_in(1.0, 8.0, false);
        assert_eq!(end.alpha, 1.0);
        assert_eq!(end.translate_y, 0.0);
        // reduce_motion: crossfade-only — alpha still ramps, but NO movement at any
        // progress (Q32 contract: collapse to a crossfade, never move).
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let p = slide_in(t, 8.0, true);
            assert!((p.alpha - t).abs() < 1e-6, "alpha must still ramp at t={t}");
            assert_eq!(p.translate_y, 0.0, "no movement under reduce_motion");
        }
    }

    #[test]
    fn crossfade_alphas_sum_to_one_at_every_progress() {
        // MOTION-INFRA-2 — old→new swap: outgoing 1→0, incoming 0→1, sharing one
        // clock so the alphas always sum to ~1.0 (no flash / double-opaque frame).
        let mid = crossfade(0.5);
        assert!((mid.out.alpha - 0.5).abs() < 1e-6);
        assert!((mid.incoming.alpha - 0.5).abs() < 1e-6);
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let c = crossfade(t);
            assert!((c.out.alpha + c.incoming.alpha - 1.0).abs() < 1e-6);
            // Crossfade is opacity-only — safe under reduce-motion unchanged.
            assert_eq!(c.out.translate_y, 0.0);
            assert_eq!(c.incoming.scale, 1.0);
        }
    }

    #[test]
    fn lift_on_hover_rises_and_drops_under_reduce_motion() {
        // MOTION-INFRA-2 — hover lift raises by `rise` px (negative y) as t grows.
        assert_eq!(lift_on_hover(0.0, 6.0, false).translate_y, 0.0);
        assert_eq!(lift_on_hover(1.0, 6.0, false).translate_y, -6.0);
        // Decorative motion: reduce_motion renders the resting frame (no lift) at
        // any progress.
        assert_eq!(lift_on_hover(1.0, 6.0, true).translate_y, 0.0);
        // Lift is transform-only (never alpha/scale change).
        assert_eq!(lift_on_hover(1.0, 6.0, false).alpha, 1.0);
        assert_eq!(lift_on_hover(1.0, 6.0, false).scale, 1.0);
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

    // ── MOTION-NET-2 — shimmer-phase math ─────────────────────────────────

    #[test]
    fn shimmer_highlight_is_bounded_and_zero_far_from_band() {
        // Intensity is always in [0,1]; a column far from the sweeping band
        // gets no highlight.
        for i in 0..=10 {
            let phase = i as f32 / 10.0;
            for j in 0..=10 {
                let pos = j as f32 / 10.0;
                let h = shimmer_highlight(phase, pos);
                assert!((0.0..=1.0).contains(&h), "h={h} out of range");
            }
        }
        // At phase 0 the band center sits at -half (off the left edge), so the
        // far-right column is well outside the band → no highlight.
        assert_eq!(shimmer_highlight(0.0, 1.0), 0.0);
    }

    #[test]
    fn shimmer_highlight_sweeps_left_to_right() {
        // Early in the cycle the left edge is brighter than the right; late in
        // the cycle the right edge is brighter than the left — i.e. the band
        // moves left→right across the placeholder.
        let early = (shimmer_highlight(0.2, 0.0), shimmer_highlight(0.2, 1.0));
        assert!(
            early.0 > early.1,
            "early sweep should favor the left: {early:?}"
        );
        let late = (shimmer_highlight(0.8, 0.0), shimmer_highlight(0.8, 1.0));
        assert!(
            late.1 > late.0,
            "late sweep should favor the right: {late:?}"
        );
    }

    #[test]
    fn shimmer_highlight_peaks_when_band_center_hits_the_column() {
        // The band center travels [-half, 1+half] across phase 0..1, so for a
        // mid column (pos=0.5) the center coincides at phase 0.5 → near-peak.
        let peak = shimmer_highlight(0.5, 0.5);
        assert!(peak > 0.9, "center hit should be ~1.0, got {peak}");
        // Off to either side of that phase the same column is dimmer.
        assert!(shimmer_highlight(0.3, 0.5) < peak);
        assert!(shimmer_highlight(0.7, 0.5) < peak);
    }

    #[test]
    fn shimmer_lift_drops_to_static_under_reduce_motion() {
        // Reduce-motion contract: no sweep at all — a flat 0 lift (static grey)
        // at every phase/position.
        for i in 0..=10 {
            let phase = i as f32 / 10.0;
            for j in 0..=10 {
                let pos = j as f32 / 10.0;
                assert_eq!(
                    shimmer_lift(phase, pos, true),
                    0.0,
                    "reduce_motion must be flat grey"
                );
            }
        }
        // With motion on, the peak lift never exceeds the SHIMMER_PEAK_LIFT cap.
        let lift = shimmer_lift(0.5, 0.5, false);
        assert!(lift > 0.0, "motion-on must shimmer");
        assert!(lift <= SHIMMER_PEAK_LIFT + 1e-6, "lift {lift} exceeds cap");
    }

    #[test]
    fn shimmer_clamps_out_of_range_inputs() {
        // Defensive: out-of-range phase/pos never panics or escapes [0,1].
        assert!((0.0..=1.0).contains(&shimmer_highlight(-1.0, 2.0)));
        assert!((0.0..=1.0).contains(&shimmer_highlight(2.0, -1.0)));
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

    // ── MOTION-FEEDBACK-2 — selection / row-state math ────────────────────

    #[test]
    fn row_state_composes_selection_and_hover() {
        // The two orthogonal inputs every list view tracks → the four states.
        assert_eq!(RowState::new(false, false), RowState::Rest);
        assert_eq!(RowState::new(false, true), RowState::Hover);
        assert_eq!(RowState::new(true, false), RowState::Selected);
        assert_eq!(RowState::new(true, true), RowState::SelectedHover);
        // is_selected tracks the selection axis regardless of hover.
        assert!(!RowState::Rest.is_selected());
        assert!(!RowState::Hover.is_selected());
        assert!(RowState::Selected.is_selected());
        assert!(RowState::SelectedHover.is_selected());
    }

    #[test]
    fn row_wash_alpha_ramps_with_state_and_mirrors_palette_tokens() {
        // Rest never washes; the ramp strictly increases hover < selected <
        // selected+hover, mirroring the Palette tint-token alphas so the row
        // wash matches the §4 single-source tokens.
        let p = crate::Palette::dark();
        assert_eq!(RowState::Rest.wash_alpha(), 0.0);
        assert!((RowState::Hover.wash_alpha() - p.hover_tint().a).abs() < 1e-6);
        assert!((RowState::Selected.wash_alpha() - p.selected_tint().a).abs() < 1e-6);
        assert!((RowState::SelectedHover.wash_alpha() - p.selected_hover_tint().a).abs() < 1e-6);
        assert!(RowState::Hover.wash_alpha() < RowState::Selected.wash_alpha());
        assert!(RowState::Selected.wash_alpha() < RowState::SelectedHover.wash_alpha());
    }

    #[test]
    fn row_style_draws_rail_only_when_selected() {
        // The accent selection rail appears iff the row is selected, at the
        // shared SELECTION_RAIL_PX weight; the wash tracks RowState::wash_alpha.
        for st in [RowState::Rest, RowState::Hover] {
            let s = RowStyle::resolve(st);
            assert_eq!(s.rail_px, 0.0, "unselected ⇒ no rail");
            assert_eq!(s.wash_alpha, st.wash_alpha());
        }
        for st in [RowState::Selected, RowState::SelectedHover] {
            let s = RowStyle::resolve(st);
            assert!((s.rail_px - SELECTION_RAIL_PX).abs() < f32::EPSILON);
            assert_eq!(s.wash_alpha, st.wash_alpha());
        }
    }

    // ── MOTION-FEEDBACK-2 — staggered reveal timing ───────────────────────

    #[test]
    fn stagger_delay_steps_then_caps() {
        use crate::motion::list::{STAGGER_CAP, STAGGER_STEP_MS};
        // Item i is delayed i*step until the cap, then every later item shares
        // the cap delay (the tail reveals as one block, no crawl).
        assert_eq!(stagger::delay_ms(0), 0);
        assert_eq!(stagger::delay_ms(1), STAGGER_STEP_MS);
        assert_eq!(stagger::delay_ms(3), 3 * STAGGER_STEP_MS);
        let cap_delay = (STAGGER_CAP as u32 - 1) * STAGGER_STEP_MS;
        assert_eq!(stagger::delay_ms(STAGGER_CAP - 1), cap_delay);
        // At and beyond the cap: clamped, never larger.
        assert_eq!(stagger::delay_ms(STAGGER_CAP), cap_delay);
        assert_eq!(stagger::delay_ms(10_000), cap_delay);
    }

    #[test]
    fn stagger_total_is_bounded_under_500ms() {
        use crate::motion::list::STAGGER_REVEAL_MS;
        // The whole cascade = last delay + one reveal, and the design caps it at
        // ≤500 ms so even the worst case never feels slow.
        let total = stagger::total_ms();
        assert_eq!(total, stagger::delay_ms(usize::MAX) + STAGGER_REVEAL_MS);
        assert!(total <= 500, "cascade must be ≤500ms, got {total}");
        // Default tokens: 140ms last delay + 120ms reveal = 260ms.
        assert_eq!(total, 260);
    }

    #[test]
    fn stagger_reveal_progress_ramps_per_row_and_collapses_under_reduce_motion() {
        use crate::motion::list::STAGGER_REVEAL_MS;
        // Row 0 starts immediately; row 2 waits for its delay before ramping.
        assert_eq!(stagger::reveal_progress(0, 0, false), 0.0);
        assert_eq!(
            stagger::reveal_progress(0, STAGGER_REVEAL_MS, false),
            1.0,
            "row 0 fully revealed after one reveal duration"
        );
        // Row 2 (delay 40ms) hasn't started at 40ms, is partway at +half reveal.
        let d2 = stagger::delay_ms(2);
        assert_eq!(stagger::reveal_progress(2, d2, false), 0.0);
        let mid = d2 + STAGGER_REVEAL_MS / 2;
        let p = stagger::reveal_progress(2, mid, false);
        assert!(p > 0.0 && p < 1.0, "row 2 mid-reveal, got {p}");
        assert!((p - 0.5).abs() < 0.05, "linear half-way ≈ 0.5, got {p}");
        // Past its window: fully revealed.
        assert_eq!(
            stagger::reveal_progress(2, d2 + STAGGER_REVEAL_MS, false),
            1.0
        );
        // Reduce-motion collapses the cascade: every row instantly at 1.0.
        assert_eq!(stagger::reveal_progress(0, 0, true), 1.0);
        assert_eq!(stagger::reveal_progress(99, 0, true), 1.0);
    }

    #[test]
    fn stagger_is_animating_stops_at_total_and_is_inert_under_reduce_motion() {
        // The reveal tick must run until the last row settles, then stop (no
        // idle animation — MOTION-PERF-1).
        assert!(stagger::is_animating(0, false));
        assert!(stagger::is_animating(stagger::total_ms() - 1, false));
        assert!(
            !stagger::is_animating(stagger::total_ms(), false),
            "cascade done ⇒ tick stops"
        );
        // Reduce-motion never schedules a reveal tick.
        assert!(!stagger::is_animating(0, true));
    }

    // ── MOTION-TRANS-1 — route/panel switch transition-phase math ─────────

    #[test]
    fn route_total_is_the_carbon_route_switch_duration() {
        // The transition span equals the Motion::route_switch token (Carbon
        // moderate-02 = 240 ms) — single-sourced, no literal.
        assert_eq!(route::total_ms(), 240);
        assert_eq!(
            route::total_ms() as u128,
            Motion::route_switch().duration.as_millis()
        );
    }

    #[test]
    fn route_progress_ramps_zero_to_one_and_collapses_under_reduce_motion() {
        let total = route::total_ms();
        // 0 at the switch instant, 1.0 once the span elapses, monotone between.
        assert_eq!(route::progress(0, false), 0.0);
        let mid = route::progress(total / 2, false);
        assert!((mid - 0.5).abs() < 0.05, "half-way ≈ 0.5, got {mid}");
        assert_eq!(route::progress(total, false), 1.0);
        // Past the span clamps at 1.0 (never overshoots).
        assert_eq!(route::progress(total + 1_000, false), 1.0);
        // Reduce-motion: instant — fully settled immediately at any elapsed.
        assert_eq!(route::progress(0, true), 1.0);
        assert_eq!(route::progress(total / 2, true), 1.0);
    }

    #[test]
    fn route_body_params_slide_in_then_settle_and_drop_movement_under_reduce_motion() {
        // Full motion: the incoming body starts SWITCH_SLIDE_PX below + transparent
        // (the deferred fade channel), and rests at offset 0 + fully opaque. The
        // ease-out curve means it's already past the linear midpoint by half-time.
        let start = route::body_params(0, false);
        assert_eq!(start.translate_y, route::SWITCH_SLIDE_PX);
        assert_eq!(start.alpha, 0.0);
        assert_eq!(start.scale, 1.0, "transition is transform/opacity only");
        let end = route::body_params(route::total_ms(), false);
        assert_eq!(end.translate_y, 0.0);
        assert_eq!(end.alpha, 1.0);
        // Monotone settle: the offset only ever shrinks toward rest.
        let mut prev = route::SWITCH_SLIDE_PX + 1.0;
        for i in 0..=10 {
            let e = (route::total_ms() as f32 * (i as f32 / 10.0)) as u32;
            let off = route::body_params(e, false).translate_y;
            assert!(off <= prev + 1e-4, "offset must shrink monotonically");
            assert!((0.0..=route::SWITCH_SLIDE_PX).contains(&off));
            prev = off;
        }
        // Reduce-motion: no movement at all (instant switch — the body is at rest
        // at every elapsed), the Q32 collapse made concrete by the fork's missing
        // opacity widget.
        for i in 0..=10 {
            let e = (route::total_ms() as f32 * (i as f32 / 10.0)) as u32;
            assert_eq!(
                route::body_params(e, true).translate_y,
                0.0,
                "reduce_motion ⇒ no movement"
            );
        }
    }

    // ── MOTION-FEEDBACK-3 — shared popup/menu/drawer/Hub enter-exit ────────

    #[test]
    fn popup_enter_fades_in_and_grows_then_drops_scale_under_reduce_motion() {
        use crate::motion::POPUP_SCALE_DELTA;
        // Full motion: opens at alpha 0 + start_scale (≈0.96×), rests at alpha 1 +
        // 1.0× — a gentle grow-from-origin fade-scale.
        let start = popup::enter_params(0.0, false);
        assert_eq!(start.alpha, 0.0);
        assert!((start.scale - (1.0 - POPUP_SCALE_DELTA)).abs() < 1e-6);
        assert_eq!(start.translate_y, 0.0, "popup never translates");
        let end = popup::enter_params(1.0, false);
        assert_eq!(end.alpha, 1.0);
        assert!((end.scale - 1.0).abs() < 1e-6);
        // Monotone: alpha + scale both only ever grow toward the open frame.
        let (mut pa, mut ps) = (-1.0, 0.0);
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let p = popup::enter_params(t, false);
            assert!(p.alpha >= pa - 1e-4, "alpha must grow monotonically");
            assert!(p.scale >= ps - 1e-4, "scale must grow monotonically");
            assert!((1.0 - POPUP_SCALE_DELTA..=1.0).contains(&p.scale));
            pa = p.alpha;
            ps = p.scale;
        }
        // Reduce-motion: crossfade only — alpha still ramps, scale is flat 1.0 at
        // every progress (no movement, the Q32 contract).
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let p = popup::enter_params(t, true);
            assert!((p.alpha - t).abs() < 1e-6, "alpha still ramps under RM");
            assert_eq!(p.scale, 1.0, "no scale under reduce_motion");
            assert_eq!(p.translate_y, 0.0);
        }
    }

    #[test]
    fn popup_exit_is_the_reverse_of_enter_and_collapses_under_reduce_motion() {
        use crate::motion::POPUP_SCALE_DELTA;
        // Close: starts open (alpha 1, 1.0×), ends closed (alpha 0, start_scale).
        let start = popup::exit_params(0.0, false);
        assert_eq!(start.alpha, 1.0);
        assert!((start.scale - 1.0).abs() < 1e-6);
        let end = popup::exit_params(1.0, false);
        assert_eq!(end.alpha, 0.0);
        assert!((end.scale - (1.0 - POPUP_SCALE_DELTA)).abs() < 1e-6);
        // Enter(t) and exit(t) alphas are complementary (one fades in as the other
        // fades out) — the shared crossfade vocabulary.
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let a = popup::enter_params(t, false).alpha;
            let b = popup::exit_params(t, false).alpha;
            assert!((a + b - 1.0).abs() < 1e-6, "enter+exit alpha must sum to 1");
        }
        // Reduce-motion: scale flat at 1.0 (crossfade only) for the exit too.
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            assert_eq!(popup::exit_params(t, true).scale, 1.0);
        }
    }

    #[test]
    fn popup_start_scale_matches_the_token() {
        use crate::motion::POPUP_SCALE_DELTA;
        assert!((popup::start_scale() - (1.0 - POPUP_SCALE_DELTA)).abs() < 1e-6);
    }

    #[test]
    fn route_is_animating_stops_at_total_and_is_inert_under_reduce_motion() {
        // The transition tick runs until the body settles, then stops (no idle
        // animation — MOTION-PERF-1).
        assert!(route::is_animating(0, false));
        assert!(route::is_animating(route::total_ms() - 1, false));
        assert!(
            !route::is_animating(route::total_ms(), false),
            "switch done ⇒ tick stops"
        );
        // Reduce-motion never schedules a transition tick (instant switch).
        assert!(!route::is_animating(0, true));
    }
}
