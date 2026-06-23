//! MOTION-FEEDBACK — the music surface's interactive-motion glue.
//!
//! Pure state + math over the shared `mde_theme::animation` vocabulary (no
//! toolkit dep in here — `main.rs` reads these and applies the offsets/tints to
//! its themed widgets):
//!
//!   * [`button_feedback`] — the transport/buttons' hover-lift + press-depress,
//!     keyed off the widget's `button::Status`. The shared
//!     [`mde_theme::animation::Transition`] `Lift`/`Press` mapping is applied at
//!     full progress: a button re-styles the instant its status flips, so press
//!     fires on **down with no input delay** and no tween/tick is needed for the
//!     button chrome itself. **Under reduce-motion the transform is dropped** —
//!     the hover/press *state change* still reads (a colour-tint depth), but the
//!     surface never moves (Q32: motion is never the only cue).
//!   * [`Reveal`] — the gentle enter tween for the now-playing footer + the
//!     queue rows. A single [`mde_theme::animation::Tween`] per reveal "epoch"
//!     (the now-playing track id / a queue (re)load), read in `view` via
//!     [`Reveal::params`] (a [`mde_theme::animation::slide_in`] fade-and-rise,
//!     collapsing to a pure crossfade under reduce-motion). Queue rows stagger
//!     by a small per-row delay so the list reveals top-down. Tick-driven only
//!     while [`Reveal::is_animating`] is `true` — a settled surface arms no tick
//!     (MOTION-PERF-1: zero idle wakeups).

use std::time::{Duration, Instant};

use mde_theme::animation::{shimmer_alpha, slide_in, LoopingTween, RenderParams, Transition};
use mde_theme::motion::Motion;

/// Hover-lift travel for a transport/nav button (px the control rises on hover).
/// A small, fixed component dimension (the shared Carbon micro-interaction tier
/// is conveyed by `Motion::hover`, not by this travel), so a local constant —
/// not a density-scaled metric. Mirrors the workbench control-feedback rise.
pub const HOVER_RISE_PX: f32 = 2.0;

/// Press-depress depth: the control scales to `1.0 - PRESS_DEPTH` at full press
/// (a subtle 4 % shrink — the depressed read), matching the shared Carbon press
/// feedback used across the shell.
pub const PRESS_DEPTH: f32 = 0.04;

/// The now-playing footer / queue reveal travel: the surface fades in while
/// rising this many px to rest. The Carbon panel-mount reveal offset, kept local
/// (it's a fixed entrance distance, not a layout metric).
pub const REVEAL_RISE_PX: f32 = 6.0;

/// Per-row reveal stagger: each successive queue row starts its slide-in this
/// much later, so a freshly-loaded queue reveals top-down rather than all at
/// once. Capped (see [`Reveal::row_start`]) so a long queue still finishes
/// promptly. A short Carbon-tier beat.
pub const ROW_STAGGER: Duration = Duration::from_millis(28);

/// The most rows that take a distinct stagger delay; beyond this every row
/// shares the last delay so a hundred-row queue doesn't reveal for seconds.
pub const STAGGER_ROW_CAP: u32 = 8;

/// The per-frame reveal tick cadence (~60 fps). Armed only while a reveal tween
/// is in flight (MOTION-PERF-1 — a settled surface costs no idle wakeups).
pub const REVEAL_TICK: Duration = Duration::from_millis(16);

/// MOTION-FEEDBACK — the hover-lift / press-depress [`RenderParams`] for an
/// interactive control at the given `button::Status` value (a settled endpoint,
/// since iced re-styles on every status flip). `hovered`/`pressed` are the two
/// active states; everything else is at rest. **Under `reduce_motion` the
/// transform is dropped** (no translate/scale) so the control never moves — the
/// caller still reflects the hover/press state via its colour-tint depth.
///
/// Pure glue over the shared [`Transition::Lift`] / [`Transition::Press`]
/// vocabulary (the same motion every shell surface speaks); no toolkit types
/// here so it's unit-testable without iced.
#[must_use]
pub fn button_feedback(hovered: bool, pressed: bool, reduce_motion: bool) -> RenderParams {
    let rest = RenderParams {
        alpha: 1.0,
        translate_y: 0.0,
        scale: 1.0,
    };
    if reduce_motion {
        // State change without movement (Q32): the colour tint carries it.
        return rest;
    }
    if pressed {
        // Press wins over hover (a pressed control is always hovered too).
        Transition::Press(PRESS_DEPTH).params(1.0)
    } else if hovered {
        Transition::Lift(HOVER_RISE_PX).params(1.0)
    } else {
        rest
    }
}

/// MOTION-FEEDBACK — the depth `0.0..=1.0` of a control's background-tint shift
/// for the given state, the **reduce-motion-safe** companion to
/// [`button_feedback`]: it is identical with or without reduce-motion, so the
/// hover/press read survives when the lift/depress transform is suppressed. The
/// caller brightens (hover) / darkens (press) its base fill by this fraction.
#[must_use]
pub const fn feedback_tint_depth(hovered: bool, pressed: bool) -> f32 {
    if pressed {
        0.12
    } else if hovered {
        0.08
    } else {
        0.0
    }
}

/// MOTION-FEEDBACK — a single in-flight enter tween for a revealing surface (the
/// now-playing footer, or the queue list). Holds the epoch's start instant; the
/// duration + easing + reduce-motion cap come from the shared
/// [`Motion::panel_mount`] preset via [`slide_in`], so this never hand-rolls
/// timing. `None`-valued (absent) reveals read as fully settled.
#[derive(Clone, Copy, Debug)]
pub struct Reveal {
    start: Instant,
    reduce_motion: bool,
}

impl Reveal {
    /// Begin a reveal at `start`. Call when the revealed content's identity
    /// changes (a new now-playing track; a (re)loaded queue).
    #[must_use]
    pub const fn starting_at(start: Instant, reduce_motion: bool) -> Self {
        Self {
            start,
            reduce_motion,
        }
    }

    /// The worst-case start-offset of the last staggered row. **Zero under
    /// reduce-motion** — the contract wants the static/final frame reached almost
    /// immediately (≤80 ms), so a staggered trickle is dropped and every row
    /// reveals from the same start (a single ≤80 ms crossfade). Capped otherwise
    /// so a long queue still finishes promptly.
    #[must_use]
    fn max_stagger(self) -> Duration {
        if self.reduce_motion {
            Duration::ZERO
        } else {
            ROW_STAGGER * STAGGER_ROW_CAP
        }
    }

    /// The whole reveal window: the per-tween settle duration plus the worst-case
    /// row stagger (so a staggered list's last row is counted as "still animating"
    /// until it too settles). Resolved against reduce-motion via the same cap the
    /// tweens use, and with the stagger dropped under reduce-motion
    /// ([`Self::max_stagger`]).
    #[must_use]
    fn window(self) -> Duration {
        let base = if self.reduce_motion {
            Duration::from_millis(mde_theme::motion::REDUCE_MOTION_CAP_MS)
        } else {
            Motion::panel_mount().duration
        };
        base + self.max_stagger()
    }

    /// `true` while any part of this reveal (including the last staggered row) is
    /// still in flight at `now` — the caller arms its reveal tick only then.
    #[must_use]
    pub fn is_animating(self, now: Instant) -> bool {
        now.saturating_duration_since(self.start) < self.window()
    }

    /// The start instant for row `row_idx` of a staggered list: the reveal start
    /// plus a capped per-row delay so the list reveals top-down without a long
    /// queue dragging on. Row 0 (and any single revealed surface) uses the base
    /// start; under reduce-motion the stagger is dropped so every row starts
    /// together ([`Self::max_stagger`]).
    #[must_use]
    fn row_start(self, row_idx: u32) -> Instant {
        if self.reduce_motion {
            return self.start;
        }
        let steps = row_idx.min(STAGGER_ROW_CAP);
        self.start + ROW_STAGGER * steps
    }

    /// The [`RenderParams`] (fade-and-rise) for the surface at `now`. `row_idx`
    /// staggers list rows (pass `0` for a single surface like the now-playing
    /// footer). Delegates to the shared [`slide_in`] helper, so the reduce-motion
    /// contract (slide collapses to a pure crossfade) is honored centrally.
    #[must_use]
    pub fn params(self, now: Instant, row_idx: u32) -> RenderParams {
        slide_in(
            self.row_start(row_idx),
            now,
            REVEAL_RISE_PX,
            self.reduce_motion,
        )
    }
}

/// MOTION-FEEDBACK — the reveal params for an optional [`Reveal`]: an absent
/// reveal (or a wrong-type state) reads as fully settled (`alpha = 1`, no
/// offset), so a caller can apply this unconditionally in `view`.
#[must_use]
pub fn reveal_params(reveal: Option<Reveal>, now: Instant, row_idx: u32) -> RenderParams {
    reveal.map_or(
        RenderParams {
            alpha: 1.0,
            translate_y: 0.0,
            scale: 1.0,
        },
        |r| r.params(now, row_idx),
    )
}

/// BEAUT-MUSIC — the per-frame cadence for the skeleton shimmer (~30 fps).
///
/// The breathe is slow (a ~1.5 s `loading` period — see [`Shimmer`]), so half
/// the reveal rate keeps the placeholder lively at a fraction of the wakeups, and
/// it is armed only while a skeleton is on screen (MOTION-PERF-1 — a settled,
/// content-painted surface costs nothing).
pub const SHIMMER_TICK: Duration = Duration::from_millis(33);

/// BEAUT-MUSIC — the looping breathe driving the library/now-playing skeleton
/// placeholders shown while the daemon state loads.
///
/// A single [`mde_theme::animation::LoopingTween`] on the shared
/// [`Motion::loading`] preset; [`Shimmer::alpha`] is the
/// [`mde_theme::animation::shimmer_alpha`] ping-pong (glue, not a
/// reimplementation), which is **STATIC at the mid alpha under reduce-motion** —
/// a plain grey block, no movement (Q32: motion is never the only cue; the
/// structure itself is the placeholder).
///
/// Tick-gating: a skeleton arms [`SHIMMER_TICK`] only while it is on screen, and
/// **never under reduce-motion** ([`Shimmer::animates`] is `false`), so the
/// breathe costs zero idle wakeups once real content lands.
#[derive(Clone, Copy, Debug)]
pub struct Shimmer {
    tween: LoopingTween,
    reduce_motion: bool,
}

impl Shimmer {
    /// Begin the shimmer breathe at `start`. One per surface; restarting is
    /// harmless (the phase is derived from `start`, not accumulated).
    #[must_use]
    pub fn starting_at(start: Instant, reduce_motion: bool) -> Self {
        Self {
            tween: LoopingTween::starting_at(start, Motion::loading().duration),
            reduce_motion,
        }
    }

    /// The skeleton tile alpha `0.10..=0.22` at `now` — the breathing grey. A
    /// pure delegate to [`mde_theme::animation::shimmer_alpha`] over this
    /// shimmer's looping phase, so the reduce-motion contract (a static mid
    /// grey) lives centrally in `mde-theme`.
    #[must_use]
    pub fn alpha(self, now: Instant) -> f32 {
        shimmer_alpha(self.tween.phase(now), self.reduce_motion)
    }

    /// Whether this shimmer actually moves: `true` only when motion is on. Under
    /// reduce-motion the alpha is phase-independent, so a caller arms **no tick**
    /// (the static grey is correct on the first frame and never changes).
    #[must_use]
    pub const fn animates(self) -> bool {
        !self.reduce_motion
    }
}

/// BEAUT-MUSIC — the gentle first-paint reveal for a whole settling surface (the
/// welcome card on first open, the Home dashboard once stats land).
///
/// Unlike [`Reveal`] (which staggers a list), this is a single fade-and-rise
/// epoch with no per-row stagger; it shares the same [`Motion::panel_mount`]
/// timing + the reduce-motion crossfade collapse via [`slide_in`]. A finished
/// mount reads as fully settled, so a caller applies [`MountReveal::params`]
/// unconditionally and disarms its tick once [`MountReveal::is_animating`] is
/// `false`.
#[derive(Clone, Copy, Debug)]
pub struct MountReveal {
    start: Instant,
    reduce_motion: bool,
}

impl MountReveal {
    /// Begin a mount reveal at `start` (call when the surface first appears).
    #[must_use]
    pub const fn starting_at(start: Instant, reduce_motion: bool) -> Self {
        Self {
            start,
            reduce_motion,
        }
    }

    /// The whole settle window: the shared `panel_mount` duration, capped to the
    /// ≤80 ms reduce-motion crossfade so a settled frame is reached promptly.
    #[must_use]
    const fn window(self) -> Duration {
        if self.reduce_motion {
            Duration::from_millis(mde_theme::motion::REDUCE_MOTION_CAP_MS)
        } else {
            Motion::panel_mount().duration
        }
    }

    /// `true` while the mount fade-and-rise is still in flight at `now`.
    #[must_use]
    pub fn is_animating(self, now: Instant) -> bool {
        now.saturating_duration_since(self.start) < self.window()
    }

    /// The fade-and-rise [`RenderParams`] at `now` — a [`slide_in`] over a small
    /// [`REVEAL_RISE_PX`] travel that collapses to a pure crossfade under
    /// reduce-motion (no translate/scale).
    #[must_use]
    pub fn params(self, now: Instant) -> RenderParams {
        slide_in(self.start, now, REVEAL_RISE_PX, self.reduce_motion)
    }
}

/// BEAUT-MUSIC — the mount params for an optional [`MountReveal`]: an absent
/// mount reads as fully settled (`alpha = 1`, no offset), so a caller applies
/// this unconditionally in `view`.
#[must_use]
pub fn mount_params(mount: Option<MountReveal>, now: Instant) -> RenderParams {
    mount.map_or(
        RenderParams {
            alpha: 1.0,
            translate_y: 0.0,
            scale: 1.0,
        },
        |m| m.params(now),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hover_lifts_press_depresses_with_motion_on() {
        // Hover rises (negative translate_y), press shrinks scale — the shared
        // Lift/Press vocabulary, applied at the settled endpoint.
        let hover = button_feedback(true, false, false);
        assert!(
            (hover.translate_y + HOVER_RISE_PX).abs() < 1e-6,
            "hover lifts by -rise, got {}",
            hover.translate_y
        );
        assert_eq!(hover.scale, 1.0, "hover never scales");
        let press = button_feedback(true, true, false);
        assert!(
            (press.scale - (1.0 - PRESS_DEPTH)).abs() < 1e-6,
            "press depresses scale, got {}",
            press.scale
        );
        // Press wins over hover (no lift while pressed).
        assert_eq!(press.translate_y, 0.0, "pressed control doesn't also lift");
        // At rest: no transform.
        let rest = button_feedback(false, false, false);
        assert_eq!(rest.translate_y, 0.0);
        assert_eq!(rest.scale, 1.0);
    }

    #[test]
    fn reduce_motion_keeps_state_without_movement() {
        // Q32 — the hover/press transform is dropped entirely under reduce-motion;
        // every state resolves to rest (no translate/scale). The tint depth (the
        // companion state cue) is unaffected, so the change still reads.
        for (h, p) in [(true, false), (true, true), (false, false)] {
            let r = button_feedback(h, p, true);
            assert_eq!(r.translate_y, 0.0, "no lift under reduce-motion");
            assert_eq!(r.scale, 1.0, "no depress under reduce-motion");
            assert_eq!(r.alpha, 1.0);
        }
        // The tint cue is identical regardless of reduce-motion.
        assert!(feedback_tint_depth(true, false) > 0.0, "hover tints");
        assert!(
            feedback_tint_depth(true, true) > feedback_tint_depth(true, false),
            "press tints deeper than hover"
        );
        assert_eq!(feedback_tint_depth(false, false), 0.0, "rest is untinted");
    }

    #[test]
    fn reveal_fades_and_rises_then_settles() {
        let t0 = Instant::now();
        let r = Reveal::starting_at(t0, false);
        // At the start the surface is transparent and risen (translate_y > 0).
        let p0 = r.params(t0, 0);
        assert!(p0.alpha < 1e-3, "starts transparent, got {}", p0.alpha);
        assert!(p0.translate_y > 0.0, "starts risen below rest");
        assert!(r.is_animating(t0), "in flight at the start");
        // After the whole window it is opaque, at rest, and no longer animating.
        let done = t0 + r.window() + Duration::from_millis(1);
        let pe = reveal_params(Some(r), done, 0);
        assert!((pe.alpha - 1.0).abs() < 1e-3, "ends opaque");
        assert!(pe.translate_y.abs() < 1e-3, "rests at 0");
        assert!(!r.is_animating(done), "settled ⇒ no tick armed");
    }

    #[test]
    fn rows_stagger_top_down_and_cap() {
        let t0 = Instant::now();
        let r = Reveal::starting_at(t0, false);
        // A later row starts later, so at t0 it is "more transparent" (less far
        // into its tween) than row 0 — the top-down reveal.
        let a0 = r.params(t0, 0).alpha;
        let a3 = r.params(t0, 3).alpha;
        assert!(a3 <= a0, "later rows reveal after earlier ones");
        // Beyond the cap, the start is clamped: row CAP and row CAP+50 share it.
        assert_eq!(
            r.row_start(STAGGER_ROW_CAP),
            r.row_start(STAGGER_ROW_CAP + 50),
            "stagger is capped so a long queue still finishes"
        );
    }

    #[test]
    fn reduce_motion_reveal_collapses_to_crossfade_and_settles_fast() {
        // The slide collapses to a pure opacity crossfade (no translate) and the
        // stagger is dropped — every row reveals from the same start within the
        // ≤80 ms cap (the reduce-motion contract: the static frame is reached
        // almost immediately, never a multi-hundred-ms trickle).
        let t0 = Instant::now();
        let r = Reveal::starting_at(t0, true);
        for ms in [0, 20, 80] {
            let p = r.params(t0 + Duration::from_millis(ms), 2);
            assert_eq!(p.translate_y, 0.0, "no slide under reduce-motion at {ms}ms");
            assert_eq!(p.scale, 1.0);
        }
        // No stagger under reduce-motion: every row shares the base start.
        assert_eq!(
            r.row_start(0),
            r.row_start(7),
            "no stagger under reduce-motion"
        );
        // The whole reveal is settled at the cap (no stagger tail).
        let cap = Duration::from_millis(mde_theme::motion::REDUCE_MOTION_CAP_MS);
        assert_eq!(r.window(), cap, "reduce-motion window is exactly the cap");
        assert!(
            !r.is_animating(t0 + cap + Duration::from_millis(1)),
            "settled within the cap — no idle ticks past 80 ms"
        );
    }

    #[test]
    fn absent_reveal_reads_as_settled() {
        let p = reveal_params(None, Instant::now(), 0);
        assert_eq!(p.alpha, 1.0);
        assert_eq!(p.translate_y, 0.0);
        assert_eq!(p.scale, 1.0);
    }

    #[test]
    fn shimmer_breathes_with_motion_and_is_static_under_reduce_motion() {
        // BEAUT-MUSIC — with motion on the skeleton breathes within the
        // mde-theme bounds and the alpha varies across the cycle; it animates so
        // a caller arms the tick.
        let t0 = Instant::now();
        let s = Shimmer::starting_at(t0, false);
        assert!(s.animates(), "motion-on shimmer arms a tick");
        let period = mde_theme::motion::Motion::loading().duration;
        let lo = s.alpha(t0);
        let mid = s.alpha(t0 + period / 2);
        assert!((mde_theme::animation::SKELETON_ALPHA_DIM
            ..=mde_theme::animation::SKELETON_ALPHA_BRIGHT)
            .contains(&lo));
        assert!(
            (mid - lo).abs() > 1e-3,
            "the breathe actually moves across the cycle ({lo} vs {mid})"
        );
        // Reduce-motion: a static mid grey, phase-independent, and NO tick armed.
        let r = Shimmer::starting_at(t0, true);
        assert!(!r.animates(), "reduce-motion shimmer arms no tick");
        assert_eq!(
            r.alpha(t0),
            r.alpha(t0 + period / 2),
            "reduce-motion alpha is phase-independent (static grey)"
        );
    }

    #[test]
    fn mount_reveal_fades_and_rises_then_settles() {
        let t0 = Instant::now();
        let m = MountReveal::starting_at(t0, false);
        let p0 = m.params(t0);
        assert!(p0.alpha < 1e-3, "starts transparent");
        assert!(p0.translate_y > 0.0, "starts risen below rest");
        assert!(m.is_animating(t0));
        let done = t0 + Motion::panel_mount().duration + Duration::from_millis(1);
        let pe = mount_params(Some(m), done);
        assert!((pe.alpha - 1.0).abs() < 1e-3, "ends opaque");
        assert!(pe.translate_y.abs() < 1e-3, "rests at 0");
        assert!(!m.is_animating(done), "settled ⇒ no tick armed");
    }

    #[test]
    fn mount_reveal_collapses_to_crossfade_and_settles_fast_under_reduce_motion() {
        let t0 = Instant::now();
        let m = MountReveal::starting_at(t0, true);
        for ms in [0, 20, 80] {
            let p = m.params(t0 + Duration::from_millis(ms));
            assert_eq!(p.translate_y, 0.0, "no slide under reduce-motion at {ms}ms");
            assert_eq!(p.scale, 1.0);
        }
        let cap = Duration::from_millis(mde_theme::motion::REDUCE_MOTION_CAP_MS);
        assert!(
            !m.is_animating(t0 + cap + Duration::from_millis(1)),
            "settled within the ≤80 ms cap — no idle ticks past it"
        );
    }

    #[test]
    fn absent_mount_reads_as_settled() {
        let p = mount_params(None, Instant::now());
        assert_eq!(p.alpha, 1.0);
        assert_eq!(p.translate_y, 0.0);
        assert_eq!(p.scale, 1.0);
    }
}
