//! Phase 5.x — accessibility + ergonomics preferences.
//!
//! Holds the user-facing toggles that affect rendering decisions:
//! reading direction, motion preference, focus-ring visibility,
//! keyboard-pane focus tracking.
//!
//! All values are pure data — the Iced view layer reads
//! [`Accessibility`] each frame and renders accordingly.
//!
//! Source of truth at runtime: environment + cosmic-config fall-
//! back, loaded once at app start. The pure-fn `load_from_env`
//! reads three env vars so the tests cover the full source order
//! without needing a cosmic-config dep:
//!
//!   * `MDE_REDUCED_MOTION=1` → [`Motion::Reduced`]
//!   * `MDE_DIRECTION=rtl`    → [`Direction::Rtl`]
//!   * `MDE_FOCUS_VISIBLE=1`  → [`FocusVisibility::AlwaysVisible`]
//!
//! cosmic-config integration lands when Phase 4.5 vendors the
//! upstream config plumbing; until then env-vars are the contract.

use std::collections::HashMap;

/// Reading direction. Default LTR (matches the Iced default).
/// `Rtl` flips the sidebar to the right and mirrors every
/// directional chevron.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    /// Left-to-right (English, etc.).
    #[default]
    Ltr,
    /// Right-to-left (Arabic, Hebrew, etc.).
    Rtl,
}

impl Direction {
    /// `true` when text + chrome should flow right-to-left.
    #[must_use]
    pub fn is_rtl(self) -> bool {
        matches!(self, Self::Rtl)
    }

    /// Parse a string value, case-insensitive. Returns `None` for
    /// unrecognised inputs so callers can fall back to `Default`.
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ltr" | "" => Some(Self::Ltr),
            "rtl" => Some(Self::Rtl),
            _ => None,
        }
    }
}

/// Motion preference. `Reduced` skips the transfer-progress sweep
/// animation + any other non-essential motion in the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Motion {
    /// Full animation set.
    #[default]
    Normal,
    /// Honor `prefers-reduced-motion`.
    Reduced,
}

impl Motion {
    /// `true` when reduced-motion mode is active.
    #[must_use]
    pub fn is_reduced(self) -> bool {
        matches!(self, Self::Reduced)
    }

    /// Animations longer than this duration (ms) should be skipped
    /// when motion is reduced. Picked from the PF6 guidance: short
    /// (≤ 150 ms) cosmetic transitions stay since they aid
    /// comprehension, but longer sweeps + decorative loops drop.
    #[must_use]
    pub fn keep_animation(self, duration_ms: u32) -> bool {
        match self {
            Self::Normal => true,
            Self::Reduced => duration_ms <= 150,
        }
    }
}

/// Focus-ring visibility policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusVisibility {
    /// PF6 default — show the ring only after a keyboard
    /// interaction (matches the CSS `:focus-visible` pseudo).
    #[default]
    Auto,
    /// Always render the focus ring on every focused widget. Used
    /// when the user explicitly requests it for accessibility.
    AlwaysVisible,
}

impl FocusVisibility {
    /// Compute whether the ring should render. `Auto` honors
    /// `keyboard_active`; `AlwaysVisible` ignores it.
    #[must_use]
    pub fn should_render(self, keyboard_active: bool) -> bool {
        match self {
            Self::Auto => keyboard_active,
            Self::AlwaysVisible => true,
        }
    }
}

/// Bag of accessibility settings. Cheap to clone; the view layer
/// can hold it on stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Accessibility {
    /// Reading direction.
    pub direction: Direction,
    /// Motion preference.
    pub motion: Motion,
    /// Focus-ring policy.
    pub focus: FocusVisibility,
}

impl Accessibility {
    /// Build a fresh [`Accessibility`] from the locked env vars.
    /// Useful for tests + the binary's startup path.
    #[must_use]
    pub fn load_from_env<F>(get: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let motion = match get("MDE_REDUCED_MOTION").as_deref() {
            Some("1") | Some("true") | Some("on") => Motion::Reduced,
            _ => Motion::Normal,
        };
        let direction = get("MDE_DIRECTION")
            .as_deref()
            .and_then(Direction::from_str)
            .unwrap_or_default();
        let focus = match get("MDE_FOCUS_VISIBLE").as_deref() {
            Some("1") | Some("true") | Some("on") => FocusVisibility::AlwaysVisible,
            _ => FocusVisibility::Auto,
        };
        Self {
            direction,
            motion,
            focus,
        }
    }
}

/// Helper for tests: build a `get` closure from a `HashMap`.
#[must_use]
pub fn env_from(map: HashMap<&'static str, &'static str>) -> impl Fn(&str) -> Option<String> {
    move |k| map.get(k).map(|s| (*s).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_defaults_to_ltr() {
        let d = Direction::default();
        assert_eq!(d, Direction::Ltr);
        assert!(!d.is_rtl());
    }

    #[test]
    fn direction_from_str_round_trip() {
        assert_eq!(Direction::from_str("ltr"), Some(Direction::Ltr));
        assert_eq!(Direction::from_str("LTR"), Some(Direction::Ltr));
        assert_eq!(Direction::from_str("rtl"), Some(Direction::Rtl));
        assert_eq!(Direction::from_str("RTL"), Some(Direction::Rtl));
        assert_eq!(Direction::from_str(""), Some(Direction::Ltr));
        assert!(Direction::from_str("xy").is_none());
    }

    #[test]
    fn motion_defaults_to_normal() {
        let m = Motion::default();
        assert!(!m.is_reduced());
    }

    #[test]
    fn motion_reduced_keeps_short_animations_drops_long_ones() {
        // PF6 cutoff: 150 ms.
        assert!(Motion::Reduced.keep_animation(0));
        assert!(Motion::Reduced.keep_animation(100));
        assert!(Motion::Reduced.keep_animation(150));
        assert!(!Motion::Reduced.keep_animation(151));
        assert!(!Motion::Reduced.keep_animation(500));
        // Normal mode keeps everything.
        for ms in [0, 100, 500, 5000] {
            assert!(Motion::Normal.keep_animation(ms));
        }
    }

    #[test]
    fn focus_visibility_auto_follows_keyboard_state() {
        assert!(!FocusVisibility::Auto.should_render(false));
        assert!(FocusVisibility::Auto.should_render(true));
    }

    #[test]
    fn focus_visibility_always_renders() {
        assert!(FocusVisibility::AlwaysVisible.should_render(false));
        assert!(FocusVisibility::AlwaysVisible.should_render(true));
    }

    #[test]
    fn accessibility_loads_default_when_env_unset() {
        let a = Accessibility::load_from_env(|_| None);
        assert_eq!(a, Accessibility::default());
    }

    #[test]
    fn accessibility_loads_reduced_motion_from_env() {
        let env = env_from(HashMap::from([("MDE_REDUCED_MOTION", "1")]));
        let a = Accessibility::load_from_env(env);
        assert_eq!(a.motion, Motion::Reduced);
    }

    #[test]
    fn accessibility_loads_rtl_from_env() {
        let env = env_from(HashMap::from([("MDE_DIRECTION", "rtl")]));
        let a = Accessibility::load_from_env(env);
        assert_eq!(a.direction, Direction::Rtl);
    }

    #[test]
    fn accessibility_loads_always_visible_focus_from_env() {
        let env = env_from(HashMap::from([("MDE_FOCUS_VISIBLE", "1")]));
        let a = Accessibility::load_from_env(env);
        assert_eq!(a.focus, FocusVisibility::AlwaysVisible);
    }

    #[test]
    fn accessibility_loads_combined_env() {
        let env = env_from(HashMap::from([
            ("MDE_REDUCED_MOTION", "true"),
            ("MDE_DIRECTION", "rtl"),
            ("MDE_FOCUS_VISIBLE", "on"),
        ]));
        let a = Accessibility::load_from_env(env);
        assert_eq!(a.motion, Motion::Reduced);
        assert_eq!(a.direction, Direction::Rtl);
        assert_eq!(a.focus, FocusVisibility::AlwaysVisible);
    }

    #[test]
    fn accessibility_unknown_values_fall_back_to_default() {
        let env = env_from(HashMap::from([
            ("MDE_REDUCED_MOTION", "maybe"),
            ("MDE_DIRECTION", "diagonal"),
            ("MDE_FOCUS_VISIBLE", "sometimes"),
        ]));
        let a = Accessibility::load_from_env(env);
        assert_eq!(a, Accessibility::default());
    }
}
