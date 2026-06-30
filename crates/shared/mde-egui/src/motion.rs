//! `Motion` — the small shared duration/easing table (governance §4, lock 10).
//!
//! E12 retires the bespoke `mde_theme::motion` engine and its lint gate. Motion
//! is now just egui's built-in `animate_bool` driven by a handful of named
//! durations, so every surface eases the same way without a separate framework.

use egui::Context;

/// The shared motion table. Durations are in **seconds** (egui's animation unit).
pub struct Motion;

impl Motion {
    /// Quick feedback — hover, small toggles, focus.
    pub const FAST: f32 = 0.08;
    /// Standard transition — panel reveals, tab switches, most state changes.
    pub const BASE: f32 = 0.18;
    /// Deliberate — larger movement, drawers, first-paint reveals.
    pub const SLOW: f32 = 0.32;

    /// Animate a boolean toward `on`, returning the eased `0.0..=1.0` progress.
    ///
    /// Thin wrapper over egui's [`Context::animate_bool_with_time`] (which eases
    /// with a smooth cubic), keyed by a stable `id`. Pass one of [`Motion::FAST`]
    /// / [`Motion::BASE`] / [`Motion::SLOW`] for `secs` so timing stays on the
    /// shared table rather than a bespoke literal.
    pub fn animate(ctx: &Context, id: impl std::hash::Hash, on: bool, secs: f32) -> f32 {
        ctx.animate_bool_with_time(egui::Id::new(id), on, secs)
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
    }

    #[test]
    fn animate_is_bounded_and_keyed() {
        // Render-agnostic: a fresh context with no elapsed time reports the
        // resting endpoint (0 for false), and the call is pure/total.
        let ctx = egui::Context::default();
        let t = Motion::animate(&ctx, "motion-test", false, Motion::BASE);
        assert!((0.0..=1.0).contains(&t), "progress {t} out of range");
    }
}
