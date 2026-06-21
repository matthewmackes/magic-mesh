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

    /// MOTION-TRANS-1 — route/panel switch entrance. When the operator switches
    /// Workbench panels/views the incoming body crosses in over a Carbon
    /// `moderate-02` (240 ms) entrance — the same "expansion / reveal" tier as
    /// `panel_mount`, so a route switch reads as one motion vocabulary with the
    /// sidebar mount it accompanies. The design's "productive entrance ~150–240
    /// ms" lands at the top of that band (a full view change deserves the longer
    /// reveal); single-shot ease-out. Reduce-motion collapses it through
    /// [`Motion::resolved`] / [`crate::Tween::resolved`] to the ≤80 ms cap.
    #[must_use]
    pub const fn route_switch() -> Self {
        Self {
            duration: DURATION_MODERATE_02,
            easing: Easing::EaseOut,
            looping: false,
        }
    }

    /// MOTION-FEEDBACK-3 — the shared **popup / menu / drawer / Hub** enter-exit
    /// timing. Every transient overlay surface (the Application Menu launcher, the
    /// power menu, the Notification Hub, Workbench dialogs/drawers) opens + closes
    /// on this one preset, so the whole shell shares one popup vocabulary. A
    /// popover is a quick state change, so Carbon `moderate-01` (150 ms) ease-out —
    /// snappier than a full `panel_mount`/route reveal (240 ms) but long enough to
    /// read as an intentional fade-scale. Single-shot; reduce-motion collapses it
    /// through [`Motion::resolved`] / [`crate::Tween::resolved`] to the ≤80 ms
    /// crossfade (and the scale is dropped — see [`crate::animation::popup`]).
    #[must_use]
    pub const fn popup() -> Self {
        Self {
            duration: DURATION_MODERATE_01,
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

/// UX-9 (b) — notification bell pulse maximum scale factor.
/// Component dimension, not density-scaled.
pub const PULSE_MAX_SCALE: f32 = 1.15;

/// UX-9 (a) — panel mount translate-Y start offset (px).
/// Component dimension, not density-scaled.
pub const PANEL_MOUNT_TRANSLATE_Y_PX: f32 = 4.0;

/// MUSIC-DOCK-2 — the distance (px) a shell **dock** slides up from below the
/// bottom screen edge when it maps. A dock open is a full-surface entrance (the
/// whole dock arrives from off-screen), so its travel is larger than the in-view
/// micro-interaction rises (the 8 px row-reveal / route-switch slide) — a
/// deliberate "rises into place from the bottom" gesture. Paired with
/// [`Motion::panel_mount`] (Carbon `moderate-02` 240 ms entrance, the
/// expansion/reveal tier) for its duration + ease-out curve, and fed through
/// [`crate::animation::slide_in`] so reduce-motion collapses the slide to a
/// crossfade (no movement). Component dimension, not density-scaled.
pub const DOCK_SLIDE_PX: f32 = 48.0;

/// MUSIC-DOCK-3 — the dimensions + inner spacing of the always-mapped
/// minimize-to-handle tab (the small bottom-center `♪ Music` chip the dock
/// collapses to instead of quitting). A handle is a persistent, compact "bring
/// it back" affordance, so it is sized as a single short pill — wide enough for
/// the glyph + the now-playing title, one line tall — and is a fixed **component
/// dimension** (UX-24: never density-scaled, like the dock slide travel + the
/// dialog/toast component sizes that live alongside it here). Carbon-grid
/// aligned: the width is the dialog `MAX_WIDTH` × ⅗ band, the height the toast
/// progress tier × the row scale, the padding/gap drawn from the Carbon
/// spacing/typography steps. Reused by the consumer for the handle layer
/// surface size + its content layout (§4 single-source).
pub mod dock_handle {
    /// Tab width (px) — a compact pill holding `♪ Music` + a truncated title.
    pub const WIDTH: f32 = 288.0;
    /// Tab height (px) — one Carbon row tall (a single line of content + pad).
    pub const HEIGHT: f32 = 36.0;
    /// Horizontal inner padding (px) — the Carbon `sm` spacing step (8).
    pub const H_PAD: f32 = 8.0;
    /// Vertical inner padding (px) — the Carbon `xs2` spacing step (4).
    pub const V_PAD: f32 = 4.0;
    /// Gap between the glyph and the label (px) — the Carbon `xs` step (6).
    pub const GAP: f32 = 6.0;
    /// Label font size (sp) — the Carbon caption tier (12).
    pub const LABEL_SIZE: f32 = 12.0;
}

/// MOTION-FEEDBACK-3 — the popup enter/exit **scale delta**: the subtle amount a
/// popup/menu/drawer/Hub is scaled *down* at the start of its open (and back
/// down to at the end of its close), e.g. `0.04` ⇒ the surface enters at 0.96×
/// and grows to 1.0×. Kept small so the surface reads as a gentle fade-scale,
/// never a distracting zoom (matches the Carbon "grow from origin" micro-scale).
/// The single source for the shell-wide popup vocabulary; dropped entirely under
/// reduce-motion (see [`crate::animation::popup`]). Component dimension, not
/// density-scaled.
pub const POPUP_SCALE_DELTA: f32 = 0.04;

/// NOTIFY-HUB-2 — the Notification-Hub entrance-motion tokens (the slide-in-from-
/// the-right travel, the bounded blink-cycle count, and the blink wash peak alpha)
/// — single-sourced here so the Hub's `crate::animation::hub` math + the binary's
/// render read one set of Carbon-grid component dimensions (no off-scale literals
/// in the GUI, §4). Component dimensions, not density-scaled (UX-24).
pub mod hub {
    /// The distance (px) a fresh alert travels in from the right edge as it
    /// enters — a Carbon micro-interaction slide (a touch larger than the 8 px
    /// row-reveal since the Hub is anchored to the right edge and the item arrives
    /// from off the panel's leading side, in the same family as the 48 px dock
    /// slide but much shorter — a single row, not a whole surface).
    pub const ENTER_SLIDE_PX: f32 = 16.0;

    /// The number of blink cycles a fresh alert flashes in its severity colour —
    /// **bounded to exactly 2** then settles (never an infinite pulse, so it burns
    /// no CPU at rest — MOTION-PERF-1 / DoD).
    pub const BLINK_CYCLES: f32 = 2.0;

    /// Peak alpha of the severity-tinted blink wash at a flash crest. Kept a
    /// gentle Carbon support-colour wash (a 2-flash attention cue, not a strobe),
    /// in the same family as the selection/hover tint ramp.
    pub const BLINK_PEAK_ALPHA: f32 = 0.45;
}

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
    fn popup_is_carbon_moderate_01_ease_out_single_shot() {
        // MOTION-FEEDBACK-3 — the shared popup/menu/drawer/Hub enter-exit preset:
        // a quick Carbon moderate-01 (150 ms) ease-out, single-shot.
        let m = Motion::popup();
        assert_eq!(m.duration, DURATION_MODERATE_01);
        assert_eq!(m.duration, Duration::from_millis(150));
        assert_eq!(m.easing, Easing::EaseOut);
        assert!(!m.looping);
        // Reduce-motion collapses it to the ≤80 ms linear crossfade.
        let r = m.resolved(true);
        assert_eq!(r.duration, Duration::from_millis(REDUCE_MOTION_CAP_MS));
        assert_eq!(r.easing, Easing::Linear);
    }

    #[test]
    fn popup_scale_delta_is_a_subtle_micro_scale() {
        // MOTION-FEEDBACK-3 — the popup grows from 0.96× (1.0 − delta) to 1.0×:
        // small enough to read as a gentle fade-scale, never a zoom.
        assert!((POPUP_SCALE_DELTA - 0.04).abs() < f32::EPSILON);
        assert!(POPUP_SCALE_DELTA > 0.0 && POPUP_SCALE_DELTA < 0.1);
    }

    #[test]
    fn dock_slide_px_is_a_full_surface_entrance_travel() {
        // MUSIC-DOCK-2 — a dock rises from off-screen below the bottom edge, so its
        // slide travel is a larger, full-surface entrance — distinctly bigger than
        // the 8 px in-view micro-interaction rises (row reveal / route switch),
        // but bounded so the dock settles within its `panel_mount` (240 ms) window.
        assert!((DOCK_SLIDE_PX - 48.0).abs() < f32::EPSILON);
        assert!(
            DOCK_SLIDE_PX > PANEL_MOUNT_TRANSLATE_Y_PX,
            "a dock entrance travels further than an in-place panel mount"
        );
        assert!(
            DOCK_SLIDE_PX > 8.0 && DOCK_SLIDE_PX <= 64.0,
            "a full-surface dock rise, but not an over-long fly-in"
        );
    }

    #[test]
    fn dock_handle_is_a_compact_single_line_pill() {
        // MUSIC-DOCK-3 — the minimize-to-handle tab is a small bottom-center pill:
        // wide enough for the glyph + a title, one Carbon row tall, with the inner
        // pad/gap drawn from the Carbon spacing steps + a caption-tier label. A
        // handle must stay smaller than the dock it collapses from (a handle, not
        // a second dock), and never taller than it is wide (a horizontal chip).
        assert!((dock_handle::WIDTH - 288.0).abs() < f32::EPSILON);
        assert!((dock_handle::HEIGHT - 36.0).abs() < f32::EPSILON);
        assert!(
            dock_handle::WIDTH > dock_handle::HEIGHT,
            "the handle is a horizontal pill, wider than it is tall"
        );
        assert!(
            dock_handle::HEIGHT < dialog::TITLE_ROW_HEIGHT,
            "the handle tab is shorter than a full dialog title row"
        );
        // Inner pad/gap are exactly the Carbon spacing steps (sm=8, xs2=4, xs=6).
        assert!((dock_handle::H_PAD - 8.0).abs() < f32::EPSILON);
        assert!((dock_handle::V_PAD - 4.0).abs() < f32::EPSILON);
        assert!((dock_handle::GAP - 6.0).abs() < f32::EPSILON);
        assert!((dock_handle::LABEL_SIZE - 12.0).abs() < f32::EPSILON);
        assert_eq!(
            dock_handle::H_PAD,
            crate::spacing::BASE[2] as f32,
            "H_PAD is the Carbon sm spacing step"
        );
        assert_eq!(
            dock_handle::V_PAD,
            crate::spacing::BASE[0] as f32,
            "V_PAD is the Carbon xs2 spacing step"
        );
        assert_eq!(
            dock_handle::GAP,
            crate::spacing::BASE[1] as f32,
            "GAP is the Carbon xs spacing step"
        );
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
