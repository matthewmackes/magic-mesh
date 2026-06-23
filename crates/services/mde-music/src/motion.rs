//! MUSIC-DOCK-2/3 — the bottom-dock surface's pure motion + collapse state.
//!
//! The layer-shell dock (`bin/mde-music-dock.rs`) is a thin shell over this
//! module: all of the slide-in animation math and the minimize-to-handle state
//! machine live here as pure, toolkit-free logic so they're unit-testable
//! without a live compositor. The binary owns only the Wayland surface + the
//! widget tree; it asks this module "what offset / alpha do I render at NOW"
//! and "am I expanded or collapsed to a handle".
//!
//! Animation is built on the shipped `mde_theme::animation` helpers
//! ([`Tween`] + [`Transition`]) and is **reduce-motion aware** via
//! [`Tween::resolved`] (§ MOTION-CORE-2): with reduce-motion the slide collapses
//! to the ≤80 ms terminal frame, so the dock simply appears.

use std::time::{Duration, Instant};

use mde_theme::animation::{ease, RenderParams, Transition, Tween};
use mde_theme::motion::{Easing, Motion};

/// MUSIC-DOCK-2 — the distance (px) the dock body is pushed down at the start
/// of the slide-in, easing to 0 (flush). Kept below the dock's surface height
/// (`bin/mde-music-dock.rs::DOCK_HEIGHT`) so the offset never shoves the body
/// off the bottom of its own surface — it's a short reveal-from-below, not a
/// full off-screen translate (the surface itself only spans the dock band).
pub const SLIDE_DISTANCE_PX: f32 = 48.0;

/// Whether the dock is shown in full or collapsed to its restore handle
/// (MUSIC-DOCK-3). Click the handle to expand; minimize to collapse.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum DockMode {
    /// The full now-playing dock (art + title + transport).
    #[default]
    Expanded,
    /// Collapsed to a small handle pill; one click restores the dock.
    Handle,
}

impl DockMode {
    /// The opposite mode — minimize ⇄ restore.
    #[must_use]
    pub const fn toggled(self) -> Self {
        match self {
            Self::Expanded => Self::Handle,
            Self::Handle => Self::Expanded,
        }
    }

    /// Whether the full dock body (not just the handle) should render.
    #[must_use]
    pub const fn is_expanded(self) -> bool {
        matches!(self, Self::Expanded)
    }
}

/// MUSIC-DOCK-2/3 — the dock's animation + collapse state. Pure: the binary
/// drives it from one `iced::time::every` tick and reads [`DockMotion::params`]
/// each frame in its `view`.
#[derive(Debug, Clone, Copy)]
pub struct DockMotion {
    /// The in-flight slide-in tween (the show transition).
    slide: Tween,
    /// Expanded vs collapsed-to-handle.
    mode: DockMode,
    /// MOTION-CORE-2 — honour the user's reduce-motion preference: the slide
    /// collapses to the ≤80 ms terminal frame so the dock just appears.
    reduce_motion: bool,
}

impl DockMotion {
    /// Arm the dock's slide-in at `now`, expanded, honouring `reduce_motion`.
    /// Uses the Carbon `panel_mount` motion preset (the standard surface-mount
    /// entrance), resolved against the reduce-motion cap.
    #[must_use]
    pub fn show(now: Instant, reduce_motion: bool) -> Self {
        Self {
            slide: Tween::resolved(now, Motion::panel_mount().duration, reduce_motion),
            mode: DockMode::default(),
            reduce_motion,
        }
    }

    /// The current collapse mode.
    #[must_use]
    pub const fn mode(self) -> DockMode {
        self.mode
    }

    /// MUSIC-DOCK-3 — minimize ⇄ restore. Restoring to [`DockMode::Expanded`]
    /// re-arms the slide so the body animates back in; collapsing to the handle
    /// leaves the slide settled (the handle pill is static, so re-arming it would
    /// only spin the animation clock for nothing — MOTION-PERF-1).
    #[must_use]
    pub fn toggle_minimized(self, now: Instant) -> Self {
        let mode = self.mode.toggled();
        let slide = if mode.is_expanded() {
            Tween::resolved(now, Motion::panel_mount().duration, self.reduce_motion)
        } else {
            // Collapsing to the handle: settle the slide immediately (static).
            Tween::static_frame(now)
        };
        Self {
            slide,
            mode,
            reduce_motion: self.reduce_motion,
        }
    }

    /// MUSIC-DOCK-2 — the slide-in render params at `now`: `alpha` (fade) +
    /// `translate_y` (px the body is pushed down from rest, easing to 0). The
    /// binary applies these to the dock body's container (color-alpha + a
    /// bottom-padding offset, since the iced fork has no transform widget).
    #[must_use]
    pub fn params(self, now: Instant) -> RenderParams {
        let t = ease(self.slide.progress(now), Easing::EaseOut);
        Transition::SlideUp(SLIDE_DISTANCE_PX).params(t)
    }

    /// Whether the slide is still in flight at `now` — drives the binary's
    /// "keep ticking" decision (MOTION-PERF-1: stop the timer at rest).
    #[must_use]
    pub fn is_animating(self, now: Instant) -> bool {
        !self.slide.is_complete(now)
    }
}

/// MUSIC-DOCK — a flattened now-playing snapshot the dock renders, derived from
/// the daemon's [`crate::nowplaying::NowState`] plus the resolved title/artist.
/// Pure data so the view + its formatting are testable without the Bus.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DockTrack {
    /// Resolved track title (empty when nothing is loaded).
    pub title: String,
    /// Resolved artist (may be empty).
    pub artist: String,
    /// `true` when the engine is actively playing (drives the play/pause glyph).
    pub playing: bool,
    /// `true` when a track is loaded at all (even if paused).
    pub has_track: bool,
    /// Playback position in ms (drives the progress fill numerator).
    pub position_ms: u64,
    /// Track duration in ms (the progress fill denominator; 0 ⇒ unknown).
    pub duration_ms: u64,
}

impl DockTrack {
    /// The label shown when no track is loaded.
    pub const IDLE_LABEL: &'static str = "Nothing playing";
    /// The label shown when a track IS loaded but its title hasn't resolved
    /// (a Bus hiccup on `get-song`): never claim "Nothing playing" against an
    /// active track, which would contradict the playing glyph.
    pub const UNTITLED_LABEL: &'static str = "Now playing";

    /// The dock's primary line: the resolved title; a neutral "Now playing"
    /// when a track is loaded but its title hasn't resolved yet; the idle
    /// label only when truly nothing is loaded.
    #[must_use]
    pub fn primary_line(&self) -> &str {
        if !self.has_track {
            Self::IDLE_LABEL
        } else if self.title.is_empty() {
            Self::UNTITLED_LABEL
        } else {
            &self.title
        }
    }

    /// Linear playback progress `0.0..=1.0` (0 when the duration is unknown).
    #[must_use]
    pub fn progress(&self) -> f32 {
        if self.duration_ms == 0 {
            0.0
        } else {
            (self.position_ms as f32 / self.duration_ms as f32).clamp(0.0, 1.0)
        }
    }
}

/// MUSIC-DOCK-2 — the dock's animation tick cadence while the slide is in
/// flight (~60 fps). Mirrors the workbench overlays' frame clock.
pub const TICK: Duration = Duration::from_millis(16);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slide_starts_offscreen_and_rests_flush() {
        // MUSIC-DOCK-2 — at t=0 the body is pushed fully down (SLIDE_DISTANCE)
        // and transparent; once the tween completes it rests at 0 + opaque.
        let t0 = Instant::now();
        let m = DockMotion::show(t0, false);
        let p0 = m.params(t0);
        assert!((p0.translate_y - SLIDE_DISTANCE_PX).abs() < 1e-3);
        assert!(p0.alpha < 0.01, "starts transparent, got {}", p0.alpha);
        let done = t0 + Motion::panel_mount().duration + Duration::from_millis(1);
        let p1 = m.params(done);
        assert!(
            p1.translate_y.abs() < 1e-3,
            "rests flush, got {}",
            p1.translate_y
        );
        assert!(
            (p1.alpha - 1.0).abs() < 1e-3,
            "ends opaque, got {}",
            p1.alpha
        );
        assert!(!m.is_animating(done), "settles after its duration");
    }

    #[test]
    fn reduce_motion_collapses_the_slide() {
        // MOTION-CORE-2 — with reduce-motion the slide is done by the 80 ms cap;
        // the dock effectively just appears (no long travel).
        let t0 = Instant::now();
        let m = DockMotion::show(t0, true);
        let at_cap = t0 + Duration::from_millis(80);
        assert!(!m.is_animating(at_cap), "reduce-motion slide done by 80 ms");
        let p = m.params(at_cap);
        assert!(p.translate_y.abs() < 1e-3 && (p.alpha - 1.0).abs() < 1e-3);
    }

    #[test]
    fn minimize_round_trips_and_rearms_slide() {
        // MUSIC-DOCK-3 — Expanded → Handle → Expanded; restoring re-arms the
        // slide so the body animates back in.
        let t0 = Instant::now();
        let m = DockMotion::show(t0, false);
        assert_eq!(m.mode(), DockMode::Expanded);
        assert!(m.mode().is_expanded());
        let later = t0 + Duration::from_secs(5);
        let collapsed = m.toggle_minimized(later);
        assert_eq!(collapsed.mode(), DockMode::Handle);
        assert!(!collapsed.mode().is_expanded());
        // MOTION-PERF-1 — collapsing to the static handle does NOT arm the
        // slide, so the animation clock stays idle (no wasted ticks).
        assert!(
            !collapsed.is_animating(later),
            "collapse-to-handle leaves the slide settled"
        );
        let restored = collapsed.toggle_minimized(later);
        assert_eq!(restored.mode(), DockMode::Expanded);
        // Restoring re-armed the slide → it animates again from `later`.
        assert!(restored.is_animating(later), "restore re-arms the slide");
    }

    #[test]
    fn track_primary_line_and_progress() {
        let idle = DockTrack::default();
        assert_eq!(idle.primary_line(), DockTrack::IDLE_LABEL);
        assert!((idle.progress() - 0.0).abs() < 1e-6);
        // A loaded track whose title hasn't resolved must NOT read as idle
        // (that would contradict the playing glyph).
        let untitled = DockTrack {
            has_track: true,
            playing: true,
            ..DockTrack::default()
        };
        assert_eq!(untitled.primary_line(), DockTrack::UNTITLED_LABEL);
        let playing = DockTrack {
            title: "Svefn-g-englar".to_string(),
            artist: "Sigur Rós".to_string(),
            playing: true,
            has_track: true,
            position_ms: 30_000,
            duration_ms: 60_000,
        };
        assert_eq!(playing.primary_line(), "Svefn-g-englar");
        assert!((playing.progress() - 0.5).abs() < 1e-3);
        // Unknown duration → no fabricated progress.
        let unknown = DockTrack {
            has_track: true,
            position_ms: 5_000,
            duration_ms: 0,
            ..DockTrack::default()
        };
        assert!((unknown.progress() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn progress_clamps_past_end() {
        let t = DockTrack {
            has_track: true,
            position_ms: 90_000,
            duration_ms: 60_000,
            ..DockTrack::default()
        };
        assert!((t.progress() - 1.0).abs() < 1e-6);
    }
}
