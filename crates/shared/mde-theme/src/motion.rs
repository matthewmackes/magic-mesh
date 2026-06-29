//! Motion + dialog timing tokens — IBM Carbon v11 motion (E9.5).
//!
//! Centralizes every "how long does this take" constant so animations across
//! the workspace stay coherent. The canonical grid is Carbon's duration scale
//! (`DURATION_FAST_01 … DURATION_SLOW_02`) + easing curves
//! (`EASING_STANDARD`/`ENTRANCE`/`EXIT`); the named [`Motion`] presets snap to
//! it:
//!   * panel / dialog mount — Carbon `moderate-02` (240 ms) entrance
//!   * tooltip fade-in — Carbon `fast-02` (110 ms) entrance
//!   * notification bell pulse — 2 s ease-in-out, looping (off-grid: a
//!     continuous loop, not a transition), max scale 1.15
//!
//! The actual interpolation lives in the consumer (Iced subscription, GTK CSS,
//! etc.) — [`Easing::carbon_bezier`] gives the exact Carbon curve; this module
//! is the durable contract for the *durations* + *parameters*.

use std::time::Duration;

/// Easing curve for a motion token. Consumers translate the
/// enum to their renderer's equivalent (CSS `cubic-bezier`,
/// Iced `iced::animation::Easing`, etc.).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Easing {
    /// Linear interpolation — no easing.
    Linear,
    /// Ease-out — fast start, slow end. Default for entrances
    /// (panels mounting, dialogs appearing).
    EaseOut,
    /// Ease-in — slow start, fast end. Default for exits.
    EaseIn,
    /// Ease-in-out — slow start + slow end. Default for
    /// continuous / looping animations (notification pulse).
    EaseInOut,
}

impl Easing {
    /// The IBM Carbon v11 productive cubic-bézier control points
    /// `(x1, y1, x2, y2)` for this curve. Entrances use Carbon's entrance
    /// easing, exits the exit easing, and continuous/standard motion the
    /// standard productive easing. Consumers translate to their renderer's
    /// `cubic-bezier`. (E9.5 — Carbon motion tokens.)
    #[must_use]
    pub const fn carbon_bezier(self) -> (f32, f32, f32, f32) {
        match self {
            Self::Linear => (0.0, 0.0, 1.0, 1.0),
            Self::EaseOut => EASING_ENTRANCE,
            Self::EaseIn => EASING_EXIT,
            Self::EaseInOut => EASING_STANDARD,
        }
    }
}

/// IBM Carbon v11 motion **duration** scale (`$duration-fast-01 … $duration-slow-02`).
/// The canonical timing grid every animation snaps to: **fast** for
/// micro-interactions (button press, toggle), **moderate** for standard state
/// changes + expansions, **slow** for large / expressive movement. (E9.5.)
pub const DURATION_FAST_01: Duration = Duration::from_millis(70);
/// Carbon `$duration-fast-02` — 110 ms (micro-interactions).
pub const DURATION_FAST_02: Duration = Duration::from_millis(110);
/// Carbon `$duration-moderate-01` — 150 ms (standard state changes).
pub const DURATION_MODERATE_01: Duration = Duration::from_millis(150);
/// Carbon `$duration-moderate-02` — 240 ms (expansions / reveals).
pub const DURATION_MODERATE_02: Duration = Duration::from_millis(240);
/// Carbon `$duration-slow-01` — 400 ms (large movement).
pub const DURATION_SLOW_01: Duration = Duration::from_millis(400);
/// Carbon `$duration-slow-02` — 700 ms (expressive movement).
pub const DURATION_SLOW_02: Duration = Duration::from_millis(700);

/// IBM Carbon v11 motion **easing** curves as cubic-bézier control points
/// `(x1, y1, x2, y2)` — the *productive* (functional-UI) set. `standard` for
/// state changes that start + end on screen, `entrance` for elements appearing,
/// `exit` for elements leaving. (E9.5.)
/// MOTION-CORE-1 — the reduce-motion duration cap (Carbon/Q32: ≤80 ms crossfade).
/// Single-sourced here + mirrored by `accessibility::A11y::transition_duration_ms`.
pub const REDUCE_MOTION_CAP_MS: u64 = 80;

pub const EASING_STANDARD: (f32, f32, f32, f32) = (0.2, 0.0, 0.38, 0.9);
/// Carbon productive `entrance` curve — elements appearing on screen.
pub const EASING_ENTRANCE: (f32, f32, f32, f32) = (0.0, 0.0, 0.38, 0.9);
/// Carbon productive `exit` curve — elements leaving the screen.
pub const EASING_EXIT: (f32, f32, f32, f32) = (0.2, 0.0, 1.0, 0.9);

/// A single motion spec — duration + easing + optional
/// looping flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Motion {
    /// Total animation duration.
    pub duration: Duration,
    /// Easing curve.
    pub easing: Easing,
    /// `true` = animation loops indefinitely (pulse, spinner);
    /// `false` = single-shot (panel mount, dialog enter).
    pub looping: bool,
}

impl Motion {
    /// Sidebar panel mount transition — an expansion, so Carbon
    /// `moderate-02` (240 ms) entrance easing, opacity 0→1 +
    /// translate-Y(4px→0). (E9.5 — reconciled from the UX-9 180 ms to the
    /// Carbon duration grid.)
    #[must_use]
    pub const fn panel_mount() -> Self {
        Self {
            duration: DURATION_MODERATE_02,
            easing: Easing::EaseOut,
            looping: false,
        }
    }

    /// Dialog mount fade — the same Carbon `moderate-02` expansion as panel
    /// mount so the system reads as one motion vocabulary. (E9.5.)
    #[must_use]
    pub const fn dialog_mount() -> Self {
        Self {
            duration: DURATION_MODERATE_02,
            easing: Easing::EaseOut,
            looping: false,
        }
    }

    /// UX-9 (b) — notification bell pulse. 2 s ease-in-out,
    /// looping. Max scale 1.15 (see [`PULSE_MAX_SCALE`]).
    #[must_use]
    pub const fn notification_pulse() -> Self {
        Self {
            duration: Duration::from_millis(2000),
            easing: Easing::EaseInOut,
            looping: true,
        }
    }

    /// Tooltip fade-in — a micro-interaction, so Carbon `fast-02` (110 ms)
    /// entrance easing. (E9.5 — reconciled from the UX-9 120 ms.)
    #[must_use]
    pub const fn tooltip_fade() -> Self {
        Self {
            duration: DURATION_FAST_02,
            easing: Easing::EaseOut,
            looping: false,
        }
    }

    // MOTION-CORE-1 — the shell-wide interaction + state presets, so every GUI
    // resolves its motion from this single source (no scattered literals). All
    // single-shot except `loading`/`refresh` (looping activity indicators).

    /// Hover lift / highlight — the fastest micro-interaction (Carbon `fast-01`,
    /// 70 ms ease-out).
    #[must_use]
    pub const fn hover() -> Self {
        Self {
            duration: DURATION_FAST_01,
            easing: Easing::EaseOut,
            looping: false,
        }
    }

    /// Press / depress feedback — the fastest tier (`fast-01`, 70 ms ease-out).
    #[must_use]
    pub const fn press() -> Self {
        Self {
            duration: DURATION_FAST_01,
            easing: Easing::EaseOut,
            looping: false,
        }
    }

    /// Focus-ring appearance — `fast-02` (110 ms ease-out), the tooltip tier.
    #[must_use]
    pub const fn focus() -> Self {
        Self {
            duration: DURATION_FAST_02,
            easing: Easing::EaseOut,
            looping: false,
        }
    }

    /// Loading indicator (skeleton shimmer / spinner) — a **looping** activity
    /// cue, Carbon `slow-02` (700 ms) ease-in-out.
    #[must_use]
    pub const fn loading() -> Self {
        Self {
            duration: DURATION_SLOW_02,
            easing: Easing::EaseInOut,
            looping: true,
        }
    }

    /// Background-refresh indicator — a **looping** `slow-01` (400 ms)
    /// ease-in-out pulse, subtler/faster than `loading`.
    #[must_use]
    pub const fn refresh() -> Self {
        Self {
            duration: DURATION_SLOW_01,
            easing: Easing::EaseInOut,
            looping: true,
        }
    }

    /// Success confirmation — a single-shot `moderate-01` (150 ms) ease-out.
    #[must_use]
    pub const fn success() -> Self {
        Self {
            duration: DURATION_MODERATE_01,
            easing: Easing::EaseOut,
            looping: false,
        }
    }

    /// Error feedback — a single-shot `fast-02` (110 ms) ease-out (a subtle
    /// flash/shake; never a long distracting motion).
    #[must_use]
    pub const fn error() -> Self {
        Self {
            duration: DURATION_FAST_02,
            easing: Easing::EaseOut,
            looping: false,
        }
    }

    /// MOTION-CORE-1/-A11Y-1 — the **reduce-motion contract** (Q32): under
    /// reduce-motion every transition collapses to a ≤80 ms linear crossfade and
    /// loops are dropped (consumers render a static, non-motion indicator). Every
    /// motion consumer must route through this so reduce-motion is guaranteed.
    /// Mirrors [`crate::A11y::transition_duration_ms`]'s 80 ms cap.
    #[must_use]
    pub const fn resolved(self, reduce_motion: bool) -> Self {
        if reduce_motion {
            Self {
                duration: Duration::from_millis(REDUCE_MOTION_CAP_MS),
                easing: Easing::Linear,
                looping: false,
            }
        } else {
            self
        }
    }
}

/// MOTION-A11Y-2 — the two motion *classes*, so a consumer can declare whether a
/// given animation **communicates state** or is merely **decorative polish**. The
/// "disable non-essential motion" preference
/// ([`crate::prefs::MotionPrefs::decorative`]) suppresses [`Decorative`] motion
/// while keeping [`Essential`] cues. Orthogonal to reduce-motion (which gates
/// *every* class to a ≤80 ms crossfade): a user can keep full essential motion yet
/// drop decorative flourishes, or vice-versa.
///
/// [`Essential`]: MotionClass::Essential
/// [`Decorative`]: MotionClass::Decorative
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionClass {
    /// State-communicating motion — loading/progress/refresh activity, async
    /// state transitions, focus appearance, success/error feedback. Never dropped
    /// by the decorative toggle (it is a real cue, not ornament); only
    /// reduce-motion / the kill switch may shorten it.
    Essential,
    /// Decorative polish — hover-lift, skeleton shimmer *breathe*, selection
    /// slide accent, staggered reveal. Removed when the user disables
    /// non-essential motion; the underlying state stays legible via the
    /// non-motion cue (colour token, static placeholder, instant select).
    Decorative,
}

impl MotionClass {
    /// `true` for [`MotionClass::Essential`] — a state cue that must survive the
    /// decorative-motion toggle.
    #[must_use]
    pub const fn is_essential(self) -> bool {
        matches!(self, Self::Essential)
    }
}

/// MOTION-A11Y-3 — the maximum permitted pulse/blink frequency (Hz). WCAG 2.x
/// (2.3.1) flags content flashing **more than 3 times per second** as a seizure
/// risk; MCNF clamps every *looping* pulse to at most this rate so no shimmer,
/// blink, or breathing indicator can flash. A single full bright→dim→bright cycle
/// of a [`LoopingTween`](crate::animation::LoopingTween) is one "flash", so the
/// cap is enforced on the loop period (see [`MIN_PULSE_PERIOD_MS`]).
pub const MAX_PULSE_HZ: f32 = 3.0;

/// MOTION-A11Y-3 — the minimum permitted pulse/blink **period** (ms), the
/// reciprocal of [`MAX_PULSE_HZ`] (`ceil(1000 / 3) = 334 ms`). A
/// [`LoopingTween`](crate::animation::LoopingTween) built via
/// [`LoopingTween::pulse`](crate::animation::LoopingTween::pulse) is clamped to at
/// least this period so it can never exceed the 3 Hz flash threshold, regardless
/// of the caller's requested period.
pub const MIN_PULSE_PERIOD_MS: u64 = 334;

/// UX-9 (b) — notification bell pulse maximum scale factor.
/// Component dimension, not density-scaled.
pub const PULSE_MAX_SCALE: f32 = 1.15;

/// UX-9 (a) — panel mount translate-Y start offset (px).
/// Component dimension, not density-scaled.
pub const PANEL_MOUNT_TRANSLATE_Y_PX: f32 = 4.0;

/// UX-9 (c) + CR-10 — dialog spec constants.
/// Locked component dimensions, not density-scaled per UX-24.
pub mod dialog {
    /// Maximum dialog width (px). Classic ChromeOS: 480 px.
    pub const MAX_WIDTH: f32 = 480.0;
    /// Backdrop opacity. CR-10 (2026-05-24) overrides UX-9 0.50 →
    /// 0.60 per the Classic ChromeOS 60 % black spec.
    pub const BACKDROP_OPACITY: f32 = 0.60;
    /// Title row height (px). Classic ChromeOS 48 px.
    pub const TITLE_ROW_HEIGHT: f32 = 48.0;
    /// Button row height (px). Classic ChromeOS 64 px.
    pub const BUTTON_ROW_HEIGHT: f32 = 64.0;
    /// Title font size (sp). Classic ChromeOS 18 sp weight 500.
    pub const TITLE_FONT_SIZE: f32 = 18.0;
    /// Horizontal inner padding (px). Classic ChromeOS 16 px.
    pub const H_PAD: f32 = 16.0;
    /// Gap between action buttons (px).
    pub const BUTTON_GAP: f32 = 8.0;
}

/// CR-10 / ANIM-3.b.1 — toast / notification chip constants.
/// Classic ChromeOS spec 2026-05-24.
pub mod toast {
    /// Fixed chip width (px).
    pub const WIDTH: f32 = 320.0;
    /// Auto-dismiss after this many milliseconds.
    pub const DISMISS_MS: u64 = 5000;
    /// Height of the bottom progress strip (px).
    pub const PROGRESS_HEIGHT: f32 = 2.0;
    /// Gap above the Shelf (px).
    pub const POSITION_GAP: f32 = 8.0;
    // ANIM-3.b.1 — Q97 action-button inline-expand tokens.
    /// Action button text size (sp). Small so buttons don't crowd the chip.
    pub const ACTION_SIZE: f32 = 12.0;
    /// Horizontal padding inside each action button (px).
    pub const ACTION_H_PAD: f32 = 8.0;
    /// Vertical padding inside each action button (px).
    pub const ACTION_V_PAD: f32 = 4.0;
    /// Alpha for action button text in resting (non-hover) state.
    pub const ACTION_RESTING_ALPHA: f32 = 0.65;
    /// Alpha for the accent-tinted hover background on action buttons.
    pub const ACTION_HOVER_BG_ALPHA: f32 = 0.12;
}

/// ANIM-4 — list/stagger + skeleton + selection timing tokens.
/// Cite: motion-language.md §2.4, §2.6, §2.8, §2.9.
/// Locks: Q15 (capped-8 stagger), Q18 (selection slide),
/// Q19 (skeleton shimmer → crossfade).
pub mod list {
    /// Maximum number of items that stagger individually (Q15).
    /// Items at or beyond this index appear at the cap delay so
    /// long lists don't crawl. With step=20ms the spread is 0–140ms.
    pub const STAGGER_CAP: usize = 8;

    /// Per-item stagger step (ms). Item i gets delay
    /// `min(i, STAGGER_CAP - 1) * STAGGER_STEP_MS`.
    pub const STAGGER_STEP_MS: u32 = 20;

    /// Reveal fade-in duration for each staggered list item (ms).
    /// Shorter than the standard 150ms so staggered items feel crisp
    /// even at the tail of the cap.
    pub const STAGGER_REVEAL_MS: u32 = 120;

    /// Selection indicator slide duration (ms). Q18.
    /// Matches motion-language.md §2.6: 150ms ease-out.
    pub const SELECTION_SLIDE_MS: u32 = 150;

    /// Skeleton shimmer oscillation period (ms). Q19.
    /// One full sweep of the shimmer highlight across the placeholder.
    pub const SHIMMER_PERIOD_MS: u64 = 1200;

    /// Duration to crossfade from skeleton shimmer to loaded content
    /// (ms). Q19. Matches the standard 150ms transition.
    pub const SKELETON_CROSSFADE_MS: u32 = 150;
}

/// CR-10 / ANIM-3.b.1 — right-click context menu constants.
/// Classic ChromeOS spec 2026-05-24.
pub mod context_menu {
    /// Minimum menu width (px).
    pub const MIN_WIDTH: f32 = 220.0;
    /// Height of each non-separator row (px).
    pub const ROW_HEIGHT: f32 = 28.0;
    /// Keyboard-shortcut label font size (sp).
    pub const KBD_SIZE: f32 = 11.0;
    /// Primary label font size (sp).
    pub const LABEL_SIZE: f32 = 13.0;
    /// Left padding for the icon column (px).
    pub const ICON_L_PAD: f32 = 12.0;
    /// Left padding between icon and label (px).
    pub const LABEL_L_PAD: f32 = 8.0;
    /// Right padding for the kbd shortcut column (px).
    pub const KBD_R_PAD: f32 = 12.0;
    // ANIM-3.b.1 — Q44 open stagger tokens.
    /// Overall menu fade-in + item stagger window (ms). Approximates
    /// "grow from cursor" in iced 0.13 (no scale transforms available).
    /// Cite: motion-language.md §2.3.
    pub const OPEN_FADE_MS: u32 = 120;
    /// Maximum items that stagger individually. Items at or beyond this
    /// index all appear at the cap delay. Mirrors list::STAGGER_CAP.
    pub const ITEM_STAGGER_CAP: usize = 8;
    /// Per-item stagger step (ms). Mirrors list::STAGGER_STEP_MS.
    pub const ITEM_STAGGER_STEP_MS: u32 = 20;
    /// Each item's individual fade-in duration (ms). Shorter than
    /// OPEN_FADE_MS so late items settle quickly.
    pub const ITEM_REVEAL_MS: u32 = 80;
}

/// ANIM-8.c.2 — icon fill-morph timing tokens (Q32).
/// Material Symbols fill axis animated outline→filled on active.
pub mod icon {
    /// Q32: fill-morph duration (ms). Outline→filled in ~150 ms ease-out.
    pub const FILL_MORPH_MS: u32 = 150;

    /// Compute the fill axis `t` value (0.0=outlined → 1.0=filled) at
    /// `elapsed_ms` into the morph. Easing: ease-out (√t). Snaps to 1.0
    /// under reduced motion.
    #[must_use]
    pub fn fill_morph_t(elapsed_ms: u64, reduce_motion: bool) -> f32 {
        if reduce_motion {
            return 1.0;
        }
        let raw = (elapsed_ms as f32 / FILL_MORPH_MS as f32).clamp(0.0, 1.0);
        raw.sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_mount_is_carbon_moderate_02_ease_out() {
        let m = Motion::panel_mount();
        assert_eq!(m.duration, DURATION_MODERATE_02); // Carbon 240 ms
        assert_eq!(m.duration, Duration::from_millis(240));
        assert_eq!(m.easing, Easing::EaseOut);
        assert!(!m.looping);
    }

    #[test]
    fn interaction_and_state_presets_pinned() {
        // MOTION-CORE-1 — every interaction/state preset resolves from the Carbon
        // grid; single-shot for feedback, looping for activity indicators.
        assert_eq!(Motion::hover().duration, DURATION_FAST_01);
        assert!(!Motion::hover().looping);
        assert_eq!(Motion::press().duration, DURATION_FAST_01);
        assert_eq!(Motion::focus().duration, DURATION_FAST_02);
        assert_eq!(Motion::success().duration, DURATION_MODERATE_01);
        assert!(!Motion::success().looping);
        assert_eq!(Motion::error().duration, DURATION_FAST_02);
        // Activity indicators loop.
        assert_eq!(Motion::loading().duration, DURATION_SLOW_02);
        assert!(Motion::loading().looping);
        assert_eq!(Motion::refresh().duration, DURATION_SLOW_01);
        assert!(Motion::refresh().looping);
    }

    #[test]
    fn resolved_honors_reduce_motion_contract() {
        // MOTION-CORE-1/-A11Y-1 — reduce-motion collapses to a ≤80 ms linear
        // crossfade and drops looping; otherwise the preset is unchanged.
        assert_eq!(REDUCE_MOTION_CAP_MS, 80);
        let normal = Motion::loading();
        assert_eq!(
            normal.resolved(false),
            normal,
            "no change when motion is on"
        );
        let reduced = Motion::loading().resolved(true);
        assert_eq!(reduced.duration, Duration::from_millis(80));
        assert_eq!(reduced.easing, Easing::Linear);
        assert!(!reduced.looping, "loops are dropped under reduce-motion");
        // A short single-shot is also capped (never exceeds 80 ms).
        assert!(Motion::panel_mount().resolved(true).duration <= Duration::from_millis(80));
    }

    #[test]
    fn carbon_duration_scale_is_pinned() {
        // IBM Carbon v11 $duration-* grid (E9.5 ground truth).
        assert_eq!(DURATION_FAST_01, Duration::from_millis(70));
        assert_eq!(DURATION_FAST_02, Duration::from_millis(110));
        assert_eq!(DURATION_MODERATE_01, Duration::from_millis(150));
        assert_eq!(DURATION_MODERATE_02, Duration::from_millis(240));
        assert_eq!(DURATION_SLOW_01, Duration::from_millis(400));
        assert_eq!(DURATION_SLOW_02, Duration::from_millis(700));
    }

    #[test]
    fn carbon_easing_curves_and_mapping_are_pinned() {
        assert_eq!(EASING_STANDARD, (0.2, 0.0, 0.38, 0.9));
        assert_eq!(EASING_ENTRANCE, (0.0, 0.0, 0.38, 0.9));
        assert_eq!(EASING_EXIT, (0.2, 0.0, 1.0, 0.9));
        // The abstract Easing enum maps onto the Carbon productive curves.
        assert_eq!(Easing::EaseOut.carbon_bezier(), EASING_ENTRANCE);
        assert_eq!(Easing::EaseIn.carbon_bezier(), EASING_EXIT);
        assert_eq!(Easing::EaseInOut.carbon_bezier(), EASING_STANDARD);
    }

    #[test]
    fn pulse_rate_cap_is_three_hz() {
        // MOTION-A11Y-3 — the flash threshold + its reciprocal period.
        assert!((MAX_PULSE_HZ - 3.0).abs() < f32::EPSILON);
        // ceil(1000 / 3) == 334 ms — the period that yields exactly ≤3 Hz.
        assert_eq!(MIN_PULSE_PERIOD_MS, 334);
        assert!(1000.0 / MIN_PULSE_PERIOD_MS as f32 <= MAX_PULSE_HZ);
    }

    #[test]
    fn every_looping_preset_is_within_the_flash_cap() {
        // MOTION-A11Y-3 — no built-in looping activity indicator may flash faster
        // than 3 Hz. One full loop period is one bright→dim→bright "flash".
        for m in [
            Motion::notification_pulse(), // 2000 ms → 0.5 Hz
            Motion::loading(),            // 700 ms → 1.43 Hz
            Motion::refresh(),            // 400 ms → 2.5 Hz
        ] {
            assert!(m.looping, "preset under test must be a loop");
            let hz = 1000.0 / m.duration.as_millis() as f32;
            assert!(
                hz <= MAX_PULSE_HZ + f32::EPSILON,
                "{:?} flashes at {hz} Hz, exceeds the {MAX_PULSE_HZ} Hz cap",
                m.duration
            );
        }
    }

    #[test]
    fn motion_class_partitions_essential_and_decorative() {
        // MOTION-A11Y-2 — the class predicate the decorative toggle keys off.
        assert!(MotionClass::Essential.is_essential());
        assert!(!MotionClass::Decorative.is_essential());
    }

    #[test]
    fn notification_pulse_is_two_seconds_looping() {
        let m = Motion::notification_pulse();
        assert_eq!(m.duration, Duration::from_millis(2000));
        assert!(m.looping);
    }

    #[test]
    fn tooltip_fade_is_carbon_fast_02() {
        let m = Motion::tooltip_fade();
        assert_eq!(m.duration, DURATION_FAST_02); // Carbon 110 ms
        assert_eq!(m.duration, Duration::from_millis(110));
    }

    #[test]
    fn dialog_mount_matches_panel_mount_duration() {
        assert_eq!(
            Motion::dialog_mount().duration,
            Motion::panel_mount().duration
        );
    }

    #[test]
    fn pulse_scale_locked_to_1_15() {
        assert!((PULSE_MAX_SCALE - 1.15).abs() < f32::EPSILON);
    }

    #[test]
    fn dialog_max_width_locked_to_480() {
        assert!((dialog::MAX_WIDTH - 480.0).abs() < f32::EPSILON);
    }

    #[test]
    fn dialog_backdrop_is_sixty_percent() {
        // CR-10 Classic ChromeOS spec: 60 % black (was UX-9 50 %).
        assert!((dialog::BACKDROP_OPACITY - 0.60).abs() < f32::EPSILON);
    }

    #[test]
    fn dialog_title_row_is_48px_and_button_row_64px() {
        assert!((dialog::TITLE_ROW_HEIGHT - 48.0).abs() < f32::EPSILON);
        assert!((dialog::BUTTON_ROW_HEIGHT - 64.0).abs() < f32::EPSILON);
    }

    #[test]
    fn toast_width_is_320_and_dismiss_5s() {
        assert!((toast::WIDTH - 320.0).abs() < f32::EPSILON);
        assert_eq!(toast::DISMISS_MS, 5000);
    }

    #[test]
    fn context_menu_min_width_is_220_and_row_28() {
        assert!((context_menu::MIN_WIDTH - 220.0).abs() < f32::EPSILON);
        assert!((context_menu::ROW_HEIGHT - 28.0).abs() < f32::EPSILON);
    }

    #[test]
    fn list_stagger_cap_is_8_and_step_20ms() {
        // Q15 acceptance: capped at 8, 20ms step → 0..140ms spread.
        assert_eq!(list::STAGGER_CAP, 8);
        assert_eq!(list::STAGGER_STEP_MS, 20);
        let last_stagger_ms = (list::STAGGER_CAP as u32 - 1) * list::STAGGER_STEP_MS;
        assert_eq!(last_stagger_ms, 140);
    }

    #[test]
    fn list_selection_slide_matches_motion_language_spec() {
        // motion-language.md §2.6: selection underlay slides 150ms ease-out.
        assert_eq!(list::SELECTION_SLIDE_MS, 150);
    }

    #[test]
    fn list_shimmer_period_is_1200ms() {
        // Q19: shimmer sweeps once per 1200ms.
        assert_eq!(list::SHIMMER_PERIOD_MS, 1200);
    }

    // ANIM-8.c.2 — icon fill-morph acceptance (Q32).

    #[test]
    fn icon_fill_morph_duration_locked_to_150ms() {
        assert_eq!(icon::FILL_MORPH_MS, 150);
    }

    #[test]
    fn icon_fill_morph_t_at_zero_is_outlined() {
        let t = icon::fill_morph_t(0, false);
        assert!((t - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn icon_fill_morph_t_at_duration_is_filled() {
        let t = icon::fill_morph_t(u64::from(icon::FILL_MORPH_MS), false);
        assert!((t - 1.0).abs() < f32::EPSILON, "expected 1.0 got {t}");
    }

    #[test]
    fn icon_fill_morph_t_reduce_motion_snaps_to_filled() {
        assert_eq!(icon::fill_morph_t(0, true), 1.0);
        assert_eq!(icon::fill_morph_t(50, true), 1.0);
    }

    #[test]
    fn icon_fill_morph_t_midpoint_is_between_0_and_1() {
        let t = icon::fill_morph_t(75, false);
        assert!(t > 0.0 && t < 1.0, "midpoint t should be (0,1), got {t}");
    }

    #[test]
    fn context_menu_stagger_tokens_match_design_lock() {
        // ANIM-3.b.1 Q44: cap mirrors list, step 20ms, reveal 80ms.
        assert_eq!(context_menu::ITEM_STAGGER_CAP, 8);
        assert_eq!(context_menu::ITEM_STAGGER_STEP_MS, 20);
        assert_eq!(context_menu::ITEM_REVEAL_MS, 80);
        assert_eq!(context_menu::OPEN_FADE_MS, 120);
    }

    #[test]
    fn toast_action_tokens_match_design_lock() {
        // ANIM-3.b.1 Q97: action button resting at 65%, hover bg 12% alpha.
        assert!((toast::ACTION_SIZE - 12.0).abs() < f32::EPSILON);
        assert!((toast::ACTION_RESTING_ALPHA - 0.65).abs() < f32::EPSILON);
        assert!((toast::ACTION_HOVER_BG_ALPHA - 0.12).abs() < f32::EPSILON);
    }
}
