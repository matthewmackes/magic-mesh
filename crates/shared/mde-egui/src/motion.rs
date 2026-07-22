//! `Motion` — the small shared duration/easing table (governance §4, lock 10).
//!
//! E12 retires the bespoke `mde_theme::motion` engine and its lint gate. Motion
//! is now just egui's built-in `animate_bool` driven by a handful of named
//! durations, so every surface eases the same way without a separate framework.

use std::{
    hash::Hash,
    sync::atomic::{AtomicU8, Ordering},
};

use egui::{Color32, Context, Pos2, Rect, Vec2};

/// Process-global **reduce-motion** preference (a11y-07): a motion / vestibular-comfort
/// toggle. When set, the shared eased helpers ([`Motion::animate`] /
/// [`Motion::animate_value`]) collapse to their settled endpoint immediately instead of
/// gliding. `false` by default (motion on — the current behaviour). The shell drives it
/// from its persisted appearance config at startup and on every change; it is read on
/// the hot per-frame animate path, so a plain `Relaxed` atomic is the right weight (a
/// UI-comfort flag, not a synchronisation edge). Deliberately global — every surface
/// paints through the one shared `Motion` table, so one flag damps them all without
/// threading a parameter through every widget.
static MOTION_MODE: AtomicU8 = AtomicU8::new(MotionMode::Normal as u8);

/// The shared motion table. Durations are in **seconds** (egui's animation unit).
pub struct Motion;

/// Runtime motion policy for every shared animation helper.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionMode {
    /// Full movement and the normal preset timings.
    Normal = 0,
    /// Shortened/faded motion for vestibular comfort.
    Reduced = 1,
    /// No travel: values jump to their endpoints.
    Disabled = 2,
}

impl Default for MotionMode {
    fn default() -> Self {
        Self::Normal
    }
}

impl MotionMode {
    #[must_use]
    fn from_u8(raw: u8) -> Self {
        match raw {
            1 => Self::Reduced,
            2 => Self::Disabled,
            _ => Self::Normal,
        }
    }

    /// Whether this mode should report `Motion::reduce_motion() == true` for
    /// backwards-compatible callers that only understand the old boolean.
    #[must_use]
    pub fn is_reduced(self) -> bool {
        !matches!(self, Self::Normal)
    }
}

/// Named motion presets for production chrome. They are deliberately semantic
/// rather than caller-local duration literals so the shell can tune one table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionPreset {
    /// Hover, focus, press, and selected/toggle micro states.
    Control,
    /// Taskbar, drawers, sheets, and Start-like panels.
    Panel,
    /// Anchored menus, context menus, browser/site popups.
    Popover,
    /// Modal/dialog presentation and dismissal.
    Dialog,
    /// Workspace, browser page, or route changes.
    Page,
    /// Springboard app open/close: the surface scales up out of its home tile
    /// and back down into it (PLATFORM-INTERFACES Q24: zoom-from-tile). Under
    /// reduced motion the zoom is endpoint-only — the large spatial travel is
    /// exactly what vestibular comfort avoids, so the app swaps in place.
    ZoomTile,
    /// List/card insert, remove, expand, collapse, and selection rails.
    Layout,
    /// Release/snap/cancel settling after direct manipulation.
    DragSettle,
}

/// Easing curve used by a [`MotionSpec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionEasing {
    /// Linear interpolation.
    Linear,
    /// Cubic smoothstep: eased in and out with no overshoot.
    SmoothStep,
}

impl MotionEasing {
    /// Sample this easing curve at normalized progress `t`.
    #[must_use]
    pub fn sample(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::SmoothStep => t * t * (3.0 - 2.0 * t),
        }
    }
}

/// A concrete timing/spring configuration resolved from a [`MotionPreset`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionSpec {
    /// The semantic preset this spec came from.
    pub preset: MotionPreset,
    /// Normal-mode duration in seconds.
    pub normal_secs: f32,
    /// Reduced-mode duration in seconds. `0.0` means endpoint-only.
    pub reduced_secs: f32,
    /// Easing curve for duration-driven values.
    pub easing: MotionEasing,
    /// Optional spring parameters for callers that use spring settling.
    pub spring: Option<Spring>,
}

impl MotionSpec {
    /// Build a concrete motion spec.
    #[must_use]
    pub const fn new(
        preset: MotionPreset,
        normal_secs: f32,
        reduced_secs: f32,
        easing: MotionEasing,
        spring: Option<Spring>,
    ) -> Self {
        Self {
            preset,
            normal_secs,
            reduced_secs,
            easing,
            spring,
        }
    }

    /// Resolve the canonical spec for a named preset.
    #[must_use]
    pub const fn for_preset(preset: MotionPreset) -> Self {
        match preset {
            MotionPreset::Control => Self::new(preset, 0.10, 0.0, MotionEasing::SmoothStep, None),
            MotionPreset::Panel => Self::new(
                preset,
                0.18,
                0.06,
                MotionEasing::SmoothStep,
                Some(Spring::SNAPPY),
            ),
            MotionPreset::Popover => Self::new(preset, 0.14, 0.06, MotionEasing::SmoothStep, None),
            MotionPreset::Dialog => Self::new(preset, 0.18, 0.06, MotionEasing::SmoothStep, None),
            MotionPreset::Page => Self::new(preset, 0.22, 0.08, MotionEasing::SmoothStep, None),
            MotionPreset::ZoomTile => Self::new(
                preset,
                0.32,
                0.0,
                MotionEasing::SmoothStep,
                Some(Spring::GENTLE),
            ),
            MotionPreset::Layout => Self::new(
                preset,
                0.18,
                0.08,
                MotionEasing::SmoothStep,
                Some(Spring::SNAPPY),
            ),
            MotionPreset::DragSettle => Self::new(
                preset,
                0.24,
                0.0,
                MotionEasing::SmoothStep,
                Some(Spring::SNAPPY),
            ),
        }
    }

    /// Duration in seconds for a runtime mode.
    #[must_use]
    pub fn duration_for(self, mode: MotionMode) -> f32 {
        match mode {
            MotionMode::Normal => self.normal_secs,
            MotionMode::Reduced => self.reduced_secs,
            MotionMode::Disabled => 0.0,
        }
        .max(0.0)
    }

    /// Eased progress for `elapsed` seconds in `mode`.
    #[must_use]
    pub fn progress_at(self, elapsed: f32, mode: MotionMode) -> f32 {
        let duration = self.duration_for(mode);
        if duration <= f32::EPSILON {
            return 1.0;
        }
        self.easing
            .sample((elapsed.max(0.0) / duration).clamp(0.0, 1.0))
    }
}

/// Lifecycle phase for presentation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Not painted and not interactive.
    Hidden,
    /// Painted while moving toward visible.
    Entering,
    /// Painted, settled, and visible.
    Visible,
    /// Painted while moving toward hidden.
    Exiting,
}

impl Phase {
    /// Derive the next phase from the desired visible state and whether the
    /// animation has reached its target.
    #[must_use]
    pub fn resolve(want_visible: bool, settled: bool) -> Self {
        match (want_visible, settled) {
            (false, true) => Self::Hidden,
            (false, false) => Self::Exiting,
            (true, false) => Self::Entering,
            (true, true) => Self::Visible,
        }
    }

    /// Whether a surface in this phase should still be painted.
    #[must_use]
    pub fn is_painted(self) -> bool {
        !matches!(self, Self::Hidden)
    }

    /// Whether background interaction should remain blocked by a modal in this
    /// phase. Exiting remains blocking until the surface is actually hidden.
    #[must_use]
    pub fn modal_blocks_background(self) -> bool {
        self.is_painted()
    }
}

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
    /// Sheet-detent settle — a dragged sheet released onto the detent picked by
    /// [`Motion::detent_target`] (PLATFORM-INTERFACES Q24). Tighter than
    /// [`SNAPPY`](Self::SNAPPY) and near-critical, so the sheet lands on its
    /// detent without sailing past it.
    pub const SHEET: Self = Self {
        stiffness: 260.0,
        damping: 30.0,
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

/// Values that can be interpolated by the shared motion carrier.
pub trait MotionValue: Copy + PartialEq + Send + Sync + 'static {
    /// Distance threshold at which the value is considered settled.
    const EPSILON: f32;

    /// Interpolate from `self` to `target` at eased progress `t`.
    #[must_use]
    fn lerp(self, target: Self, t: f32) -> Self;

    /// Scalar distance used for retarget/rest detection.
    #[must_use]
    fn distance(self, target: Self) -> f32;

    /// Whether every component is finite.
    #[must_use]
    fn is_finite(self) -> bool;
}

impl MotionValue for f32 {
    const EPSILON: f32 = 0.001;

    fn lerp(self, target: Self, t: f32) -> Self {
        lerp_f32(self, target, t)
    }

    fn distance(self, target: Self) -> f32 {
        (target - self).abs()
    }

    fn is_finite(self) -> bool {
        f32::is_finite(self)
    }
}

impl MotionValue for Vec2 {
    const EPSILON: f32 = 0.01;

    fn lerp(self, target: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        Self::new(lerp_f32(self.x, target.x, t), lerp_f32(self.y, target.y, t))
    }

    fn distance(self, target: Self) -> f32 {
        (target - self).length()
    }

    fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

impl MotionValue for Pos2 {
    const EPSILON: f32 = 0.01;

    fn lerp(self, target: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        Self::new(lerp_f32(self.x, target.x, t), lerp_f32(self.y, target.y, t))
    }

    fn distance(self, target: Self) -> f32 {
        (target - self).length()
    }

    fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

impl MotionValue for Rect {
    const EPSILON: f32 = 0.01;

    fn lerp(self, target: Self, t: f32) -> Self {
        Self::from_min_max(
            <Pos2 as MotionValue>::lerp(self.min, target.min, t),
            <Pos2 as MotionValue>::lerp(self.max, target.max, t),
        )
    }

    fn distance(self, target: Self) -> f32 {
        self.min
            .distance(target.min)
            .max(self.max.distance(target.max))
    }

    fn is_finite(self) -> bool {
        self.min.x.is_finite()
            && self.min.y.is_finite()
            && self.max.x.is_finite()
            && self.max.y.is_finite()
    }
}

impl MotionValue for Color32 {
    const EPSILON: f32 = 1.0;

    fn lerp(self, target: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let [sr, sg, sb, sa] = self.to_array();
        let [tr, tg, tb, ta] = target.to_array();
        Color32::from_rgba_premultiplied(
            lerp_u8(sr, tr, t),
            lerp_u8(sg, tg, t),
            lerp_u8(sb, tb, t),
            lerp_u8(sa, ta, t),
        )
    }

    fn distance(self, target: Self) -> f32 {
        let [sr, sg, sb, sa] = self.to_array();
        let [tr, tg, tb, ta] = target.to_array();
        (i16::from(sr) - i16::from(tr))
            .abs()
            .max((i16::from(sg) - i16::from(tg)).abs())
            .max((i16::from(sb) - i16::from(tb)).abs())
            .max((i16::from(sa) - i16::from(ta)).abs()) as f32
    }

    fn is_finite(self) -> bool {
        true
    }
}

#[must_use]
fn lerp_u8(start: u8, target: u8, t: f32) -> u8 {
    lerp_f32(f32::from(start), f32::from(target), t)
        .round()
        .clamp(0.0, 255.0) as u8
}

#[must_use]
fn lerp_f32(start: f32, target: f32, t: f32) -> f32 {
    start + (target - start) * t.clamp(0.0, 1.0)
}

/// A clamped opacity value (`0.0..=1.0`) for typed opacity animation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionOpacity(pub f32);

impl MotionOpacity {
    /// Construct opacity, clamped to `0.0..=1.0`.
    #[must_use]
    pub fn new(value: f32) -> Self {
        Self(value.clamp(0.0, 1.0))
    }

    /// Return the clamped opacity value.
    #[must_use]
    pub fn value(self) -> f32 {
        self.0
    }
}

impl MotionValue for MotionOpacity {
    const EPSILON: f32 = 0.001;

    fn lerp(self, target: Self, t: f32) -> Self {
        Self::new(lerp_f32(self.0, target.0, t))
    }

    fn distance(self, target: Self) -> f32 {
        self.0.distance(target.0)
    }

    fn is_finite(self) -> bool {
        self.0.is_finite()
    }
}

/// A non-negative scale value for typed scale animation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionScale(pub f32);

impl MotionScale {
    /// Construct scale, clamped to `>= 0.0`.
    #[must_use]
    pub fn new(value: f32) -> Self {
        Self(value.max(0.0))
    }

    /// Return the non-negative scale value.
    #[must_use]
    pub fn value(self) -> f32 {
        self.0
    }
}

impl MotionValue for MotionScale {
    const EPSILON: f32 = 0.001;

    fn lerp(self, target: Self, t: f32) -> Self {
        Self::new(lerp_f32(self.0, target.0, t))
    }

    fn distance(self, target: Self) -> f32 {
        self.0.distance(target.0)
    }

    fn is_finite(self) -> bool {
        self.0.is_finite()
    }
}

/// Reusable animation carrier for a typed value. It owns retargeting,
/// completion detection, progress, and phase; the egui helpers below store this
/// type in `Context` memory behind stable IDs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Animated<T: MotionValue> {
    from: T,
    value: T,
    target: T,
    elapsed: f32,
    duration: f32,
    progress: f32,
    phase: Phase,
}

/// Scalar animated value.
pub type AnimatedScalar = Animated<f32>;
/// Two-dimensional animated value.
pub type AnimatedVec2 = Animated<Vec2>;
/// Size animated value (`Vec2::x = width`, `Vec2::y = height`).
pub type AnimatedSize = Animated<Vec2>;
/// Rect animated value.
pub type AnimatedRect = Animated<Rect>;
/// Opacity animated value.
pub type AnimatedOpacity = Animated<MotionOpacity>;
/// Scale animated value.
pub type AnimatedScale = Animated<MotionScale>;
/// Color animated value.
pub type AnimatedColor = Animated<Color32>;

impl<T: MotionValue> Animated<T> {
    /// Create a carrier already settled at `value`.
    #[must_use]
    pub fn settled(value: T) -> Self {
        Self {
            from: value,
            value,
            target: value,
            elapsed: 0.0,
            duration: 0.0,
            progress: 1.0,
            phase: Phase::Visible,
        }
    }

    /// Advance toward `target` by `dt` seconds under `spec`/`mode`.
    ///
    /// When the target changes mid-flight, the new animation starts from the
    /// current visual value, preserving continuity. `dt` is clamped to a 30 FPS
    /// floor so long stalls cannot produce giant jumps after a VT switch or a
    /// delayed page flip.
    pub fn advance(&mut self, target: T, spec: MotionSpec, mode: MotionMode, dt: f32) {
        if !target.is_finite() {
            return;
        }

        if self.target.distance(target) > T::EPSILON {
            self.from = self.value;
            self.target = target;
            self.elapsed = 0.0;
            self.progress = 0.0;
            self.phase = Phase::Entering;
        }

        self.duration = spec.duration_for(mode);
        if self.duration <= f32::EPSILON || matches!(mode, MotionMode::Disabled) {
            self.from = target;
            self.value = target;
            self.target = target;
            self.elapsed = 0.0;
            self.progress = 1.0;
            self.phase = Phase::Visible;
            return;
        }

        if self.value.distance(self.target) <= T::EPSILON {
            self.value = self.target;
            self.progress = 1.0;
            self.phase = Phase::Visible;
            return;
        }

        let dt = dt.clamp(0.0, 1.0 / 30.0);
        self.elapsed = (self.elapsed + dt).min(self.duration);
        self.progress = spec.progress_at(self.elapsed, mode);
        self.value = self.from.lerp(self.target, self.progress);

        if self.progress >= 1.0 || self.value.distance(self.target) <= T::EPSILON {
            self.value = self.target;
            self.progress = 1.0;
            self.phase = Phase::Visible;
        } else {
            self.phase = Phase::Entering;
        }
    }

    #[must_use]
    /// Current visual value.
    pub fn value(self) -> T {
        self.value
    }

    #[must_use]
    /// Current target value.
    pub fn target(self) -> T {
        self.target
    }

    #[must_use]
    /// Eased progress toward the current target, `0.0..=1.0`.
    pub fn progress(self) -> f32 {
        self.progress
    }

    #[must_use]
    /// Elapsed seconds accumulated toward the current target.
    pub fn elapsed(self) -> f32 {
        self.elapsed
    }

    #[must_use]
    /// Active duration in seconds after resolving the current mode.
    pub fn duration(self) -> f32 {
        self.duration
    }

    #[must_use]
    /// Presentation phase implied by this animated value.
    pub fn phase(self) -> Phase {
        self.phase
    }

    #[must_use]
    /// Whether the carrier is visually settled at its target.
    pub fn is_settled(self) -> bool {
        self.phase == Phase::Visible && self.value.distance(self.target) <= T::EPSILON
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

    /// Set the process-global motion mode.
    pub fn set_mode(mode: MotionMode) {
        MOTION_MODE.store(mode as u8, Ordering::Relaxed);
    }

    /// Current process-global motion mode.
    #[must_use]
    pub fn mode() -> MotionMode {
        MotionMode::from_u8(MOTION_MODE.load(Ordering::Relaxed))
    }

    /// Resolve one semantic preset into its concrete timing/spring table.
    #[must_use]
    pub fn spec(preset: MotionPreset) -> MotionSpec {
        MotionSpec::for_preset(preset)
    }

    /// Set the process-global **reduce-motion** preference (a11y-07). The shell calls
    /// this from its appearance apply seam — at startup and on every toggle change —
    /// with the persisted value, so every [`animate`](Self::animate) /
    /// [`animate_value`](Self::animate_value) caller settles instantly rather than
    /// easing. Idempotent; `Relaxed` is sufficient for a UI-comfort flag.
    pub fn set_reduce_motion(on: bool) {
        Self::set_mode(if on {
            MotionMode::Reduced
        } else {
            MotionMode::Normal
        });
    }

    /// Whether **reduce-motion** (a11y-07) is currently in force — the flag the eased
    /// helpers consult to short-circuit to their endpoint. `false` (motion on) by
    /// default. Note the hard-blink alarm ([`blink`](Self::blink)) deliberately
    /// ignores this: an alarm outranks the comfort preference (NODE-GRADE-2 #16).
    #[must_use]
    pub fn reduce_motion() -> bool {
        Self::mode().is_reduced()
    }

    /// Drive a typed animated value from egui memory using the current global
    /// motion mode. This is the shared stable-ID carrier for new components.
    pub fn animate_typed<T: MotionValue>(
        ctx: &Context,
        id: impl Hash,
        target: T,
        preset: MotionPreset,
    ) -> Animated<T> {
        Self::animate_typed_with_mode(ctx, id, target, preset, Self::mode())
    }

    /// Drive a typed animated value using an explicit mode, useful for tests or
    /// surfaces previewing a mode without mutating the process-global setting.
    pub fn animate_typed_with_mode<T: MotionValue>(
        ctx: &Context,
        id: impl Hash,
        target: T,
        preset: MotionPreset,
        mode: MotionMode,
    ) -> Animated<T> {
        let id = egui::Id::new(id);
        let spec = Self::spec(preset);
        let dt = ctx.input(|i| i.stable_dt);
        let mut animated = ctx
            .data_mut(|d| d.get_temp::<Animated<T>>(id))
            .unwrap_or_else(|| Animated::settled(target));
        animated.advance(target, spec, mode, dt);
        ctx.data_mut(|d| d.insert_temp(id, animated));
        if !animated.is_settled() {
            ctx.request_repaint();
        }
        animated
    }

    /// Drive a scalar value using a named preset.
    pub fn animate_scalar(
        ctx: &Context,
        id: impl Hash,
        target: f32,
        preset: MotionPreset,
    ) -> AnimatedScalar {
        Self::animate_typed(ctx, id, target, preset)
    }

    /// Drive a 2D/offset value using a named preset.
    pub fn animate_vec2(
        ctx: &Context,
        id: impl Hash,
        target: Vec2,
        preset: MotionPreset,
    ) -> AnimatedVec2 {
        Self::animate_typed(ctx, id, target, preset)
    }

    /// Drive a size value using a named preset.
    pub fn animate_size(
        ctx: &Context,
        id: impl Hash,
        target: Vec2,
        preset: MotionPreset,
    ) -> AnimatedSize {
        Self::animate_typed(ctx, id, target, preset)
    }

    /// Drive a rect value using a named preset.
    pub fn animate_rect(
        ctx: &Context,
        id: impl Hash,
        target: Rect,
        preset: MotionPreset,
    ) -> AnimatedRect {
        Self::animate_typed(ctx, id, target, preset)
    }

    /// Drive an opacity value using a named preset.
    pub fn animate_opacity(
        ctx: &Context,
        id: impl Hash,
        target: MotionOpacity,
        preset: MotionPreset,
    ) -> AnimatedOpacity {
        Self::animate_typed(ctx, id, target, preset)
    }

    /// Drive a scale value using a named preset.
    pub fn animate_scale(
        ctx: &Context,
        id: impl Hash,
        target: MotionScale,
        preset: MotionPreset,
    ) -> AnimatedScale {
        Self::animate_typed(ctx, id, target, preset)
    }

    /// Drive a color value using a named preset.
    pub fn animate_color(
        ctx: &Context,
        id: impl Hash,
        target: Color32,
        preset: MotionPreset,
    ) -> AnimatedColor {
        Self::animate_typed(ctx, id, target, preset)
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
    /// Release-velocity magnitude (sheet fractions **per second**) past which a
    /// dragged sheet commits toward the next detent in the velocity's direction
    /// instead of snapping to the nearest. Used by
    /// [`detent_target`](Self::detent_target) (PLATFORM-INTERFACES Q24).
    pub const DETENT_FLING: f32 = 0.5;
    /// Swipe-velocity magnitude (pages **per second**) past which a page swipe
    /// advances one page in its direction instead of settling on the nearest.
    /// Used by [`page_settle`](Self::page_settle) (PLATFORM-INTERFACES Q24).
    pub const PAGE_FLING: f32 = 0.5;

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

    /// The **detent** a released sheet settles at (PLATFORM-INTERFACES Q24 sheet
    /// detent physics): given the sheet's fractional position `pos` (`0.0`
    /// closed … `1.0` tallest), the drag-release `velocity` in fractions/second
    /// (positive = opening), and the sheet's ascending `detents` (e.g.
    /// `[0.35, 0.9]`), pick the fraction to rest at. A release slower than
    /// [`DETENT_FLING`](Self::DETENT_FLING) snaps to the **nearest** detent — a
    /// slow release never dismisses by surprise; a faster release commits one
    /// detent in the velocity's direction. A fast **downward** fling with no
    /// detent left below resolves to `0.0` — the drag-to-dismiss gesture. Pure;
    /// drive the returned target with [`spring_to`](Self::spring_to) +
    /// [`Spring::SHEET`], which already collapses to the endpoint under
    /// reduce-motion (a11y-07) — the resting detent itself is mode-independent.
    /// An empty `detents` slice is degenerate and dismisses.
    #[must_use]
    pub fn detent_target(pos: f32, velocity: f32, detents: &[f32]) -> f32 {
        let Some(&highest) = detents.last() else {
            return 0.0;
        };
        if velocity <= -Self::DETENT_FLING {
            // Committed downward: the next detent below, or dismiss when the
            // fling is already past the lowest one.
            detents
                .iter()
                .rev()
                .copied()
                .find(|&d| d < pos)
                .unwrap_or(0.0)
        } else if velocity >= Self::DETENT_FLING {
            // Committed upward: the next detent above, or hold at the tallest.
            detents
                .iter()
                .copied()
                .find(|&d| d > pos)
                .unwrap_or(highest)
        } else {
            detents
                .iter()
                .copied()
                .min_by(|a, b| (a - pos).abs().total_cmp(&(b - pos).abs()))
                .unwrap_or(highest)
        }
    }

    /// The **page** an interruptible swipe lands on (PLATFORM-INTERFACES Q24
    /// page swipes): `offset` is the live scroll position in page units (`0.0` =
    /// first page, fractional mid-swipe), `velocity` in pages/second (positive =
    /// toward higher indices). A swipe faster than
    /// [`PAGE_FLING`](Self::PAGE_FLING) advances one page in its direction; a
    /// slower release settles on the nearest page; the result is always clamped
    /// to the real `0..page_count` range. Composes with the existing gesture
    /// primitives — decay the live fling with
    /// [`inertial_decay`](Self::inertial_decay), compress the ends with
    /// [`rubber_band`](Self::rubber_band) — neither of which picks a page. Pure;
    /// animate toward the returned index via [`spring_to`](Self::spring_to) or a
    /// [`MotionPreset::Page`] carrier, both of which collapse to the endpoint
    /// under reduce-motion (a11y-07) — the landing page itself is
    /// mode-independent. A zero `page_count` is degenerate and reads page `0`.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn page_settle(offset: f32, velocity: f32, page_count: usize) -> usize {
        if page_count == 0 {
            return 0;
        }
        let target = if velocity >= Self::PAGE_FLING {
            offset.floor() + 1.0
        } else if velocity <= -Self::PAGE_FLING {
            offset.ceil() - 1.0
        } else {
            offset.round()
        };
        target.clamp(0.0, (page_count - 1) as f32) as usize
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
    use super::{
        AnimatedColor, AnimatedOpacity, AnimatedRect, AnimatedScalar, AnimatedScale, AnimatedSize,
        AnimatedVec2, Motion, MotionEasing, MotionMode, MotionOpacity, MotionPreset, MotionScale,
        MotionSpec, Phase, Spring,
    };
    use egui::{pos2, vec2, Color32, Rect};

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
    fn named_presets_resolve_to_accessible_mode_durations() {
        for preset in [
            MotionPreset::Control,
            MotionPreset::Panel,
            MotionPreset::Popover,
            MotionPreset::Dialog,
            MotionPreset::Page,
            MotionPreset::ZoomTile,
            MotionPreset::Layout,
            MotionPreset::DragSettle,
        ] {
            let spec = Motion::spec(preset);
            assert_eq!(spec.preset, preset);
            assert!(spec.duration_for(MotionMode::Normal) > 0.0);
            assert!(
                spec.duration_for(MotionMode::Reduced) <= spec.duration_for(MotionMode::Normal),
                "{preset:?} reduced duration should not exceed normal"
            );
            assert_eq!(spec.duration_for(MotionMode::Disabled), 0.0);
            assert_eq!(spec.progress_at(999.0, MotionMode::Disabled), 1.0);
        }

        assert_eq!(
            Motion::spec(MotionPreset::Control).duration_for(MotionMode::Reduced),
            0.0,
            "micro-control motion collapses under reduced motion"
        );
        assert!(
            Motion::spec(MotionPreset::Panel).spring.is_some(),
            "panels carry a snappy spring option"
        );
        assert!(
            Motion::spec(MotionPreset::DragSettle).spring.is_some(),
            "drag release has a spring settle option"
        );
    }

    #[test]
    fn zoom_tile_is_a_hero_transition_that_collapses_under_reduced_motion() {
        // PLATFORM-INTERFACES Q24: zoom-from-tile — the springboard open/close.
        let spec = Motion::spec(MotionPreset::ZoomTile);
        assert_eq!(spec.preset, MotionPreset::ZoomTile);
        assert!(
            spec.duration_for(MotionMode::Normal)
                > Motion::spec(MotionPreset::Page).duration_for(MotionMode::Normal),
            "the tile zoom is more deliberate than an in-place page change"
        );
        assert_eq!(
            spec.spring,
            Some(Spring::GENTLE),
            "hero expansions share the gentle spring"
        );
        // Under reduced motion the zoom is endpoint-only — instant swap, no
        // travel (a11y-07), matching the Control/DragSettle convention.
        assert_eq!(spec.duration_for(MotionMode::Reduced), 0.0);
        assert_eq!(
            spec.progress_at(0.0, MotionMode::Reduced),
            1.0,
            "reduced-motion zoom lands on its endpoint on the first frame"
        );
        assert_eq!(spec.duration_for(MotionMode::Disabled), 0.0);
    }

    #[test]
    fn sheet_detents_snap_to_nearest_below_the_fling_threshold() {
        let detents = [0.35, 0.9];
        assert_eq!(Motion::detent_target(0.5, 0.0, &detents), 0.35);
        assert_eq!(Motion::detent_target(0.7, 0.0, &detents), 0.9);
        // A slow release below the lowest detent climbs back to it — a sheet
        // never dismisses without a committed fling.
        assert_eq!(
            Motion::detent_target(0.1, -Motion::DETENT_FLING * 0.5, &detents),
            0.35
        );
        // No detents at all is degenerate: nowhere to rest, so dismiss.
        assert_eq!(Motion::detent_target(0.5, 0.0, &[]), 0.0);
    }

    #[test]
    fn sheet_detents_commit_in_the_fling_direction() {
        let detents = [0.35, 0.9];
        // A fast upward fling from the low detent commits to the tall one, and
        // holds at the tallest when there is nothing further above.
        assert_eq!(
            Motion::detent_target(0.4, Motion::DETENT_FLING * 2.0, &detents),
            0.9
        );
        assert_eq!(
            Motion::detent_target(0.95, Motion::DETENT_FLING * 2.0, &detents),
            0.9
        );
        // A fast downward fling from the tall detent steps to the low one — one
        // detent per fling, never straight past it.
        assert_eq!(
            Motion::detent_target(0.8, -Motion::DETENT_FLING * 2.0, &detents),
            0.35
        );
    }

    #[test]
    fn sheet_fast_downward_fling_below_the_lowest_detent_dismisses() {
        let detents = [0.35, 0.9];
        assert_eq!(
            Motion::detent_target(0.3, -Motion::DETENT_FLING * 2.0, &detents),
            0.0,
            "drag-to-dismiss: committed downward with no detent left below"
        );
        // The paired settle spring is real and near-critical, like its siblings.
        assert!(Spring::SHEET.stiffness > 0.0 && Spring::SHEET.damping > 0.0);
    }

    #[test]
    fn page_settle_rounds_nearest_flings_one_page_and_clamps() {
        // Below the fling threshold: nearest page wins.
        assert_eq!(Motion::page_settle(0.4, 0.0, 3), 0);
        assert_eq!(Motion::page_settle(0.6, 0.0, 3), 1);
        // Past the threshold: one page in the fling's direction, even when the
        // swipe has barely travelled (the interruptible-swipe commit).
        assert_eq!(Motion::page_settle(0.2, Motion::PAGE_FLING * 2.0, 3), 1);
        assert_eq!(Motion::page_settle(1.8, -Motion::PAGE_FLING * 2.0, 3), 1);
        // …but never off either end, and an empty pager reads page 0.
        assert_eq!(Motion::page_settle(2.6, Motion::PAGE_FLING * 2.0, 3), 2);
        assert_eq!(Motion::page_settle(0.1, -Motion::PAGE_FLING * 2.0, 3), 0);
        assert_eq!(Motion::page_settle(0.5, 0.0, 0), 0);
    }

    #[test]
    fn motion_phase_models_modal_lifecycle() {
        assert_eq!(Phase::resolve(false, true), Phase::Hidden);
        assert_eq!(Phase::resolve(false, false), Phase::Exiting);
        assert_eq!(Phase::resolve(true, false), Phase::Entering);
        assert_eq!(Phase::resolve(true, true), Phase::Visible);

        assert!(!Phase::Hidden.is_painted());
        assert!(Phase::Exiting.is_painted());
        assert!(Phase::Entering.modal_blocks_background());
        assert!(Phase::Exiting.modal_blocks_background());
    }

    #[test]
    fn animated_scalar_retargets_from_current_visual_state_and_settles() {
        let spec = Motion::spec(MotionPreset::Page);
        let mut value = AnimatedScalar::settled(0.0);
        value.advance(100.0, spec, MotionMode::Normal, 1.0 / 120.0);
        assert!(value.value() > 0.0);
        assert!(value.value() < 100.0);
        assert_eq!(value.phase(), Phase::Entering);
        assert!(!value.is_settled());

        let before_retarget = value.value();
        value.advance(50.0, spec, MotionMode::Normal, 1.0 / 120.0);
        assert!(
            value.value() >= before_retarget && value.value() < 50.0,
            "retarget starts from the current visual value, got {} from {before_retarget}",
            value.value()
        );

        for _ in 0..60 {
            value.advance(50.0, spec, MotionMode::Normal, 1.0 / 60.0);
        }
        assert_eq!(value.value(), 50.0);
        assert!(value.is_settled());
        assert_eq!(value.phase(), Phase::Visible);
    }

    #[test]
    fn animated_values_clamp_large_frame_gaps() {
        let spec = Motion::spec(MotionPreset::Page);
        let mut value = AnimatedScalar::settled(0.0);
        value.advance(100.0, spec, MotionMode::Normal, 10.0);
        assert!(
            value.elapsed() <= 1.0 / 30.0 + f32::EPSILON,
            "large frame gap was clamped, elapsed={}",
            value.elapsed()
        );
        assert!(
            value.value() < 100.0,
            "clamping prevents a long pause from jumping straight to the target"
        );
    }

    #[test]
    fn animated_values_handle_common_refresh_intervals_without_poisoning_state() {
        let refresh_intervals = [
            1.0 / 240.0,
            1.0 / 144.0,
            1.0 / 120.0,
            1.0 / 60.0,
            1.0 / 30.0,
            1.0 / 24.0,
        ];
        let presets = [
            MotionPreset::Control,
            MotionPreset::Panel,
            MotionPreset::Popover,
            MotionPreset::Dialog,
            MotionPreset::Page,
            MotionPreset::ZoomTile,
            MotionPreset::Layout,
            MotionPreset::DragSettle,
        ];

        for preset in presets {
            let spec = Motion::spec(preset);
            for dt in refresh_intervals {
                let mut value = AnimatedScalar::settled(0.0);
                value.advance(100.0, spec, MotionMode::Normal, dt);
                assert!(
                    value.elapsed() <= dt.min(1.0 / 30.0) + f32::EPSILON,
                    "{preset:?} did not clamp dt {dt}: elapsed={}",
                    value.elapsed()
                );
                assert!(value.value().is_finite(), "{preset:?} value poisoned");
                assert!(value.target().is_finite(), "{preset:?} target poisoned");
                assert!(
                    (0.0..=1.0).contains(&value.progress()),
                    "{preset:?} progress out of range: {}",
                    value.progress()
                );
            }

            let mut value = AnimatedScalar::settled(0.0);
            for frame in 0..120 {
                let dt = refresh_intervals[frame % refresh_intervals.len()];
                value.advance(100.0, spec, MotionMode::Normal, dt);
                assert!(value.value().is_finite(), "{preset:?} value poisoned");
                assert!(
                    (0.0..=1.0).contains(&value.progress()),
                    "{preset:?} progress out of range: {}",
                    value.progress()
                );
            }
            assert!(value.is_settled(), "{preset:?} did not settle");
        }
    }

    #[test]
    fn easing_curves_are_bounded_and_zero_duration_is_endpoint() {
        assert_eq!(MotionEasing::SmoothStep.sample(0.0), 0.0);
        assert_eq!(MotionEasing::SmoothStep.sample(1.0), 1.0);
        assert_eq!(MotionEasing::Linear.sample(0.0), 0.0);
        assert_eq!(MotionEasing::Linear.sample(1.0), 1.0);

        let mut previous = 0.0;
        for t in [0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0] {
            let sampled = MotionEasing::SmoothStep.sample(t);
            assert!((0.0..=1.0).contains(&sampled));
            assert!(
                sampled >= previous,
                "smoothstep should not reverse at t={t}: {sampled} < {previous}"
            );
            previous = sampled;
        }
        assert_eq!(MotionEasing::SmoothStep.sample(-1.0), 0.0);
        assert_eq!(MotionEasing::SmoothStep.sample(2.0), 1.0);

        let zero = MotionSpec::new(MotionPreset::Page, 0.0, 0.0, MotionEasing::SmoothStep, None);
        let mut value = AnimatedScalar::settled(0.0);
        value.advance(100.0, zero, MotionMode::Normal, 0.0);
        assert_eq!(value.value(), 100.0);
        assert!(value.is_settled());
    }

    #[test]
    fn reversal_and_non_finite_targets_do_not_snap_or_poison_state() {
        let spec = Motion::spec(MotionPreset::Layout);
        let mut value = AnimatedScalar::settled(0.0);
        for _ in 0..4 {
            value.advance(100.0, spec, MotionMode::Normal, 1.0 / 60.0);
        }
        let before_reverse = value.value();
        assert!(before_reverse > 0.0 && before_reverse < 100.0);

        value.advance(-50.0, spec, MotionMode::Normal, 1.0 / 60.0);
        assert!(
            value.value() < before_reverse && value.value() > -50.0,
            "reversal continues from the live value: {} from {before_reverse}",
            value.value()
        );
        assert!(value.value().is_finite());

        let before_bad_target = value.value();
        value.advance(f32::INFINITY, spec, MotionMode::Normal, 999.0);
        assert_eq!(
            value.value(),
            before_bad_target,
            "non-finite targets are ignored rather than poisoning the carrier"
        );
        assert!(value.value().is_finite());
    }

    #[test]
    fn reduced_mode_substitutions_are_distinct_from_disabled_mode() {
        let panel = Motion::spec(MotionPreset::Panel);
        let mut reduced = AnimatedScalar::settled(0.0);
        reduced.advance(100.0, panel, MotionMode::Reduced, 1.0 / 120.0);
        assert!(
            reduced.value() > 0.0 && reduced.value() < 100.0,
            "panel reduced mode keeps a short fade/travel, got {}",
            reduced.value()
        );

        let mut reduced_control = AnimatedScalar::settled(0.0);
        reduced_control.advance(
            100.0,
            Motion::spec(MotionPreset::Control),
            MotionMode::Reduced,
            0.0,
        );
        assert_eq!(
            reduced_control.value(),
            100.0,
            "control reduced mode is endpoint-only"
        );

        let mut disabled = AnimatedScalar::settled(0.0);
        disabled.advance(100.0, panel, MotionMode::Disabled, 0.0);
        assert_eq!(disabled.value(), 100.0);
        assert!(disabled.is_settled());
    }

    #[test]
    fn disabled_mode_lands_animated_values_on_endpoint() {
        let spec = Motion::spec(MotionPreset::Panel);
        let mut value = AnimatedScalar::settled(0.0);
        value.advance(100.0, spec, MotionMode::Disabled, 0.0);
        assert_eq!(value.value(), 100.0);
        assert_eq!(value.progress(), 1.0);
        assert!(value.is_settled());
    }

    #[test]
    fn typed_interpolation_covers_size_rect_opacity_scale_and_color() {
        let spec = MotionSpec::new(
            MotionPreset::Layout,
            2.0 / 30.0,
            0.0,
            MotionEasing::Linear,
            None,
        );

        let mut offset = AnimatedVec2::settled(vec2(0.0, 10.0));
        offset.advance(vec2(10.0, 30.0), spec, MotionMode::Normal, 1.0 / 30.0);
        assert_eq!(offset.value(), vec2(5.0, 20.0));

        let mut size = AnimatedSize::settled(vec2(0.0, 0.0));
        size.advance(vec2(10.0, 20.0), spec, MotionMode::Normal, 1.0 / 30.0);
        assert_eq!(size.value(), vec2(5.0, 10.0));

        let mut rect = AnimatedRect::settled(Rect::from_min_max(pos2(0.0, 0.0), pos2(10.0, 10.0)));
        rect.advance(
            Rect::from_min_max(pos2(10.0, 20.0), pos2(30.0, 40.0)),
            spec,
            MotionMode::Normal,
            1.0 / 30.0,
        );
        assert_eq!(
            rect.value(),
            Rect::from_min_max(pos2(5.0, 10.0), pos2(20.0, 25.0))
        );

        let mut opacity = AnimatedOpacity::settled(MotionOpacity::new(0.0));
        opacity.advance(
            MotionOpacity::new(2.0),
            spec,
            MotionMode::Normal,
            1.0 / 30.0,
        );
        assert_eq!(opacity.value().value(), 0.5);

        let mut scale = AnimatedScale::settled(MotionScale::new(1.0));
        scale.advance(MotionScale::new(0.5), spec, MotionMode::Normal, 1.0 / 30.0);
        assert_eq!(scale.value().value(), 0.75);

        let mut color = AnimatedColor::settled(Color32::BLACK);
        color.advance(Color32::WHITE, spec, MotionMode::Normal, 1.0 / 30.0);
        assert_eq!(color.value(), Color32::from_rgb(128, 128, 128));
    }

    #[test]
    fn egui_context_driver_uses_stable_ids_and_explicit_modes() {
        let ctx = egui::Context::default();
        let first = Motion::animate_typed_with_mode(
            &ctx,
            "typed-stable-id",
            0.0,
            MotionPreset::Control,
            MotionMode::Normal,
        );
        assert!(first.is_settled());
        assert_eq!(first.value(), 0.0);

        let second = Motion::animate_typed_with_mode(
            &ctx,
            "typed-stable-id",
            10.0,
            MotionPreset::Panel,
            MotionMode::Disabled,
        );
        assert!(second.is_settled());
        assert_eq!(second.value(), 10.0);

        let third = Motion::animate_scalar(&ctx, "typed-stable-id", 20.0, MotionPreset::Control);
        assert_eq!(third.target(), 20.0);
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
