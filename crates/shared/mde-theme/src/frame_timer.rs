//! MOTION-PERF-3 — opt-in, debug-only frame-time + redraw instrumentation.
//!
//! Animation cost (UX-9.a tweens, [`crate::animation`]) is only as
//! cheap as its redraw cadence. This module gives every iced/Cosmic
//! surface a way to *measure* that cadence — per-surface frame
//! interval + a running redraw count — so a developer can log where
//! a motion is repainting too often or stuttering.
//!
//! ## Zero cost when off
//!
//! The gate is a single env-var probe (`MDE_FRAME_DEBUG`) read once
//! via [`FrameTimer::from_env`]. When the flag is unset the returned
//! [`FrameTimer`] is the [`FrameTimer::Off`] variant: [`FrameTimer::tick`]
//! is a no-op that allocates nothing, starts no timer, and reads no
//! clock. Only when the operator opts in does the timer capture
//! [`Instant`]s and accumulate samples. A surface holds one
//! `FrameTimer` and calls `tick()` from its redraw handler:
//!
//! ```
//! use mde_theme::frame_timer::FrameTimer;
//!
//! // Construct once per surface (cheap; OFF unless MDE_FRAME_DEBUG set).
//! let mut ft = FrameTimer::from_env("files-grid");
//!
//! // In the redraw handler, each frame:
//! if let Some(sample) = ft.tick() {
//!     // Only `Some` when debug is on AND we have a prior frame to
//!     // diff against — a GUI logs it however it likes.
//!     eprintln!(
//!         "{}: frame #{} dt={:.2}ms",
//!         sample.surface, sample.frame, sample.interval.as_secs_f64() * 1000.0
//!     );
//! }
//! ```

use std::time::{Duration, Instant};

/// Env var that arms per-surface frame instrumentation.
///
/// Any value other than the literal `0` (or empty) counts as on,
/// matching the `MDE_MOTION_DISABLED` / `MDE_REDUCE_MOTION` convention
/// in [`crate::prefs`].
pub const FRAME_DEBUG_ENV: &str = "MDE_FRAME_DEBUG";

/// One frame measurement.
///
/// Yielded by [`FrameTimer::tick`] only when instrumentation is armed
/// and a previous frame exists to diff against. Cheap, `Copy`, and
/// carries the surface label so a logger doesn't have to thread it
/// separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSample {
    /// Static label of the surface that produced this frame.
    pub surface: &'static str,
    /// 1-based ordinal of this frame within the surface's lifetime.
    pub frame: u64,
    /// Wall-clock gap since the previous `tick()` — the frame
    /// interval (≈ `1 / fps`).
    pub interval: Duration,
}

/// Running totals an armed timer keeps across the surface lifetime.
///
/// Exposed via [`FrameTimer::stats`] so a surface can log a summary
/// (e.g. on teardown) without re-deriving from individual samples.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameStats {
    /// Total redraws observed (every `tick()`, including the first).
    pub redraws: u64,
    /// Summed inter-frame intervals — `redraws - 1` samples, since the
    /// first frame has no predecessor to diff against.
    pub elapsed: Duration,
}

impl FrameStats {
    /// Mean frame interval over all measured gaps, or `None` until at
    /// least two frames have been observed.
    #[must_use]
    pub fn mean_interval(self) -> Option<Duration> {
        let gaps = self.redraws.checked_sub(1).filter(|&g| g > 0)?;
        // `redraws` fits a u32 divisor for any realistic session; clamp
        // defensively so the conversion can't truncate to zero.
        let gaps = u32::try_from(gaps).unwrap_or(u32::MAX);
        Some(self.elapsed / gaps)
    }

    /// Mean frames-per-second derived from [`Self::mean_interval`],
    /// or `None` until enough frames (or if the mean interval is zero).
    #[must_use]
    pub fn mean_fps(self) -> Option<f64> {
        let secs = self.mean_interval()?.as_secs_f64();
        (secs > 0.0).then(|| 1.0 / secs)
    }
}

/// Per-surface frame instrumentation. Two states with the same API:
///
/// - [`FrameTimer::Off`] — the default. `tick()` does nothing and
///   nothing is allocated or clocked. This is what every surface gets
///   unless [`FRAME_DEBUG_ENV`] is set.
/// - [`FrameTimer::On`] — armed: each `tick()` reads the clock, bumps
///   the redraw count, and (after the first frame) yields a
///   [`FrameSample`].
#[derive(Clone, Debug)]
pub enum FrameTimer {
    /// Disabled — zero-cost. `tick()` returns `None`.
    Off,
    /// Armed — accumulates timing for `surface`.
    On(ArmedTimer),
}

/// State carried by an armed [`FrameTimer::On`]. Public so the enum
/// can be matched, but constructed only through [`FrameTimer`].
#[derive(Clone, Debug)]
pub struct ArmedTimer {
    surface: &'static str,
    last: Option<Instant>,
    stats: FrameStats,
}

impl FrameTimer {
    /// Build a timer for `surface`, armed iff [`FRAME_DEBUG_ENV`] is set
    /// to anything but `0`/empty. Read the env once at construction so
    /// the hot `tick()` path never touches the environment.
    #[must_use]
    pub fn from_env(surface: &'static str) -> Self {
        let armed = std::env::var_os(FRAME_DEBUG_ENV)
            .is_some_and(|v| !v.is_empty() && v != "0");
        Self::with_enabled(surface, armed)
    }

    /// Build a timer with an explicit on/off decision, bypassing the
    /// env probe. Lets a GUI wire the flag to its own debug toggle and
    /// keeps the gating unit-testable without mutating process env
    /// (which is `unsafe` and racy under a test harness).
    #[must_use]
    pub fn with_enabled(surface: &'static str, enabled: bool) -> Self {
        if enabled {
            Self::On(ArmedTimer {
                surface,
                last: None,
                stats: FrameStats::default(),
            })
        } else {
            Self::Off
        }
    }

    /// Is this timer armed? Lets a surface skip building a log message
    /// entirely when off.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        matches!(self, Self::On(_))
    }

    /// Record a redraw at `now`, returning the inter-frame sample if one
    /// is available (armed, and not the very first frame). Split from
    /// [`Self::tick`] so the accumulator is testable without a real
    /// clock.
    pub fn tick_at(&mut self, now: Instant) -> Option<FrameSample> {
        let Self::On(t) = self else {
            return None;
        };
        t.stats.redraws += 1;
        let sample = t.last.map(|prev| {
            let interval = now.saturating_duration_since(prev);
            t.stats.elapsed += interval;
            FrameSample {
                surface: t.surface,
                frame: t.stats.redraws,
                interval,
            }
        });
        t.last = Some(now);
        sample
    }

    /// Record a redraw at the current instant. The hot path a surface
    /// calls each frame. When off this is a single discriminant check
    /// and an early `None` — no clock read, no allocation.
    pub fn tick(&mut self) -> Option<FrameSample> {
        if matches!(self, Self::Off) {
            return None;
        }
        self.tick_at(Instant::now())
    }

    /// Snapshot of the running totals, or [`FrameStats::default`] when
    /// off (zero redraws / zero elapsed).
    #[must_use]
    pub fn stats(&self) -> FrameStats {
        match self {
            Self::Off => FrameStats::default(),
            Self::On(t) => t.stats,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_timer_never_samples_or_counts() {
        let mut ft = FrameTimer::with_enabled("surf", false);
        assert!(!ft.is_enabled());
        // No clock, no sample, no accumulation — many ticks stay inert.
        for _ in 0..10 {
            assert_eq!(ft.tick(), None);
            assert_eq!(ft.tick_at(Instant::now()), None);
        }
        assert_eq!(ft.stats(), FrameStats::default());
        assert_eq!(ft.stats().redraws, 0);
    }

    #[test]
    fn first_frame_counts_but_yields_no_interval() {
        let mut ft = FrameTimer::with_enabled("surf", true);
        assert!(ft.is_enabled());
        let t0 = Instant::now();
        // First frame has no predecessor → no sample, but it IS counted.
        assert_eq!(ft.tick_at(t0), None);
        assert_eq!(ft.stats().redraws, 1);
        assert_eq!(ft.stats().elapsed, Duration::ZERO);
        assert_eq!(ft.stats().mean_interval(), None);
    }

    #[test]
    fn subsequent_frames_yield_intervals_and_accumulate() {
        let mut ft = FrameTimer::with_enabled("grid", true);
        let t0 = Instant::now();
        let _ = ft.tick_at(t0);
        let s1 = ft.tick_at(t0 + Duration::from_millis(16)).expect("sample");
        assert_eq!(s1.surface, "grid");
        assert_eq!(s1.frame, 2);
        assert_eq!(s1.interval, Duration::from_millis(16));

        let s2 = ft.tick_at(t0 + Duration::from_millis(40)).expect("sample");
        assert_eq!(s2.frame, 3);
        assert_eq!(s2.interval, Duration::from_millis(24));

        let stats = ft.stats();
        assert_eq!(stats.redraws, 3);
        // Two measured gaps: 16ms + 24ms = 40ms; mean = 20ms.
        assert_eq!(stats.elapsed, Duration::from_millis(40));
        assert_eq!(stats.mean_interval(), Some(Duration::from_millis(20)));
    }

    #[test]
    fn mean_fps_is_inverse_of_mean_interval() {
        let mut ft = FrameTimer::with_enabled("surf", true);
        let t0 = Instant::now();
        let _ = ft.tick_at(t0);
        // One 20ms gap → 50 fps.
        let _ = ft.tick_at(t0 + Duration::from_millis(20));
        let fps = ft.stats().mean_fps().expect("fps");
        assert!((fps - 50.0).abs() < 1e-9, "got {fps}");
    }

    #[test]
    fn mean_helpers_none_before_two_frames() {
        let stats = FrameStats::default();
        assert_eq!(stats.mean_interval(), None);
        assert_eq!(stats.mean_fps(), None);
    }

    #[test]
    fn non_monotonic_clock_does_not_panic() {
        // saturating_duration_since guards a backwards clock reading.
        let mut ft = FrameTimer::with_enabled("surf", true);
        let t0 = Instant::now();
        let _ = ft.tick_at(t0 + Duration::from_millis(100));
        let s = ft.tick_at(t0).expect("sample");
        assert_eq!(s.interval, Duration::ZERO);
    }
}
