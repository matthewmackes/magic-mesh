//! `Motion` — the small shared duration/easing table (governance §4, lock 10).
//!
//! E12 retires the bespoke `mde_theme::motion` engine and its lint gate. Motion
//! is now just egui's built-in `animate_bool` driven by a handful of named
//! durations, so every surface eases the same way without a separate framework.

use egui::Context;

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

    /// Animate a boolean toward `on`, returning the eased `0.0..=1.0` progress.
    ///
    /// Thin wrapper over egui's [`Context::animate_bool_with_time`] (which eases
    /// with a smooth cubic), keyed by a stable `id`. Pass one of [`Motion::FAST`]
    /// / [`Motion::BASE`] / [`Motion::SLOW`] for `secs` so timing stays on the
    /// shared table rather than a bespoke literal.
    pub fn animate(ctx: &Context, id: impl std::hash::Hash, on: bool, secs: f32) -> f32 {
        ctx.animate_bool_with_time(egui::Id::new(id), on, secs)
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
}

#[cfg(test)]
#[allow(clippy::assertions_on_constants)]
mod tests {
    use super::Motion;

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
