//! Runtime input policy shared by Settings and the bare DRM seat.
//!
//! The native DRM runner owns libinput directly, so compositor settings such as
//! pointer speed or natural scrolling need a tiny process-global handoff rather
//! than a Wayland compositor knob. The System Settings surface persists the user
//! policy and calls [`set_input_policy`]; the DRM loop reads the current policy
//! while translating libinput events into egui input.

use std::sync::atomic::{AtomicBool, AtomicI16, AtomicU16, Ordering};

use crate::egui;

const MIN_POINTER_SPEED: i16 = -100;
const MAX_POINTER_SPEED: i16 = 100;
const MIN_SCROLL_SPEED: u16 = 25;
const MAX_SCROLL_SPEED: u16 = 300;

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
const BTN_SIDE: u32 = 0x113;
const BTN_EXTRA: u32 = 0x114;

static POINTER_SPEED_PERCENT: AtomicI16 = AtomicI16::new(0);
static SCROLL_SPEED_PERCENT: AtomicU16 = AtomicU16::new(100);
static LEFT_HANDED: AtomicBool = AtomicBool::new(false);
static NATURAL_SCROLL: AtomicBool = AtomicBool::new(false);
static TOUCHSCREEN_ENABLED: AtomicBool = AtomicBool::new(true);
static TWO_FINGER_SCROLL: AtomicBool = AtomicBool::new(true);
static EDGE_GESTURES: AtomicBool = AtomicBool::new(true);
static LONG_PRESS_SECONDARY: AtomicBool = AtomicBool::new(true);

/// The live input controls consumed by the bare DRM runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputPolicy {
    /// Pointer speed as a percentage delta: `0` is libinput's current delta, negative
    /// is slower, positive is faster.
    pub pointer_speed_percent: i16,
    /// Wheel/touchpad scroll speed as a percentage where `100` is unchanged.
    pub scroll_speed_percent: u16,
    /// Swap primary and secondary pointer buttons.
    pub left_handed: bool,
    /// Reverse wheel/touchpad scroll deltas.
    pub natural_scroll: bool,
    /// Accept touchscreen contacts from the bare seat.
    pub touchscreen_enabled: bool,
    /// Accept two-finger scroll gestures from touchpads/touchscreens.
    pub two_finger_scroll: bool,
    /// Allow edge swipes to raise shell affordances.
    pub edge_gestures: bool,
    /// Allow long-press to synthesize a secondary click.
    pub long_press_secondary: bool,
}

impl Default for InputPolicy {
    fn default() -> Self {
        Self {
            pointer_speed_percent: 0,
            scroll_speed_percent: 100,
            left_handed: false,
            natural_scroll: false,
            touchscreen_enabled: true,
            two_finger_scroll: true,
            edge_gestures: true,
            long_press_secondary: true,
        }
    }
}

impl InputPolicy {
    /// Clamp all persisted/user-fed values into the runtime-supported range.
    #[must_use]
    pub const fn normalized(mut self) -> Self {
        if self.pointer_speed_percent < MIN_POINTER_SPEED {
            self.pointer_speed_percent = MIN_POINTER_SPEED;
        } else if self.pointer_speed_percent > MAX_POINTER_SPEED {
            self.pointer_speed_percent = MAX_POINTER_SPEED;
        }
        if self.scroll_speed_percent < MIN_SCROLL_SPEED {
            self.scroll_speed_percent = MIN_SCROLL_SPEED;
        } else if self.scroll_speed_percent > MAX_SCROLL_SPEED {
            self.scroll_speed_percent = MAX_SCROLL_SPEED;
        }
        self
    }

    /// Multiplier applied to relative pointer deltas.
    #[must_use]
    pub fn pointer_scale(self) -> f32 {
        let p = f32::from(self.normalized().pointer_speed_percent);
        (1.0 + p / 100.0).clamp(0.25, 2.0)
    }

    /// Multiplier applied to wheel and touchpad scroll deltas.
    #[must_use]
    pub fn scroll_scale(self) -> f32 {
        f32::from(self.normalized().scroll_speed_percent) / 100.0
    }
}

/// Publish a new live input policy for the DRM runner.
pub fn set_input_policy(policy: InputPolicy) {
    let policy = policy.normalized();
    POINTER_SPEED_PERCENT.store(policy.pointer_speed_percent, Ordering::Relaxed);
    SCROLL_SPEED_PERCENT.store(policy.scroll_speed_percent, Ordering::Relaxed);
    LEFT_HANDED.store(policy.left_handed, Ordering::Relaxed);
    NATURAL_SCROLL.store(policy.natural_scroll, Ordering::Relaxed);
    TOUCHSCREEN_ENABLED.store(policy.touchscreen_enabled, Ordering::Relaxed);
    TWO_FINGER_SCROLL.store(policy.two_finger_scroll, Ordering::Relaxed);
    EDGE_GESTURES.store(policy.edge_gestures, Ordering::Relaxed);
    LONG_PRESS_SECONDARY.store(policy.long_press_secondary, Ordering::Relaxed);
}

/// Read the current live input policy.
#[must_use]
pub fn input_policy() -> InputPolicy {
    InputPolicy {
        pointer_speed_percent: POINTER_SPEED_PERCENT.load(Ordering::Relaxed),
        scroll_speed_percent: SCROLL_SPEED_PERCENT.load(Ordering::Relaxed),
        left_handed: LEFT_HANDED.load(Ordering::Relaxed),
        natural_scroll: NATURAL_SCROLL.load(Ordering::Relaxed),
        touchscreen_enabled: TOUCHSCREEN_ENABLED.load(Ordering::Relaxed),
        two_finger_scroll: TWO_FINGER_SCROLL.load(Ordering::Relaxed),
        edge_gestures: EDGE_GESTURES.load(Ordering::Relaxed),
        long_press_secondary: LONG_PRESS_SECONDARY.load(Ordering::Relaxed),
    }
    .normalized()
}

/// Map a Linux input button code into egui's button model, applying left-handed
/// swapping for primary/secondary buttons.
#[must_use]
pub fn pointer_button(raw_button: u32, left_handed: bool) -> Option<egui::PointerButton> {
    match raw_button {
        BTN_LEFT if left_handed => Some(egui::PointerButton::Secondary),
        BTN_LEFT => Some(egui::PointerButton::Primary),
        BTN_RIGHT if left_handed => Some(egui::PointerButton::Primary),
        BTN_RIGHT => Some(egui::PointerButton::Secondary),
        BTN_MIDDLE => Some(egui::PointerButton::Middle),
        BTN_SIDE => Some(egui::PointerButton::Extra1),
        BTN_EXTRA => Some(egui::PointerButton::Extra2),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_policy_clamps_runtime_ranges() {
        let policy = InputPolicy {
            pointer_speed_percent: -500,
            scroll_speed_percent: 500,
            ..InputPolicy::default()
        }
        .normalized();

        assert_eq!(policy.pointer_speed_percent, -100);
        assert_eq!(policy.scroll_speed_percent, 300);
        assert!((policy.pointer_scale() - 0.25).abs() < f32::EPSILON);
        assert!((policy.scroll_scale() - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn pointer_policy_round_trips_through_the_runtime_handoff() {
        let original = input_policy();
        let policy = InputPolicy {
            pointer_speed_percent: -35,
            scroll_speed_percent: 125,
            left_handed: true,
            natural_scroll: true,
            touchscreen_enabled: false,
            two_finger_scroll: false,
            edge_gestures: false,
            long_press_secondary: false,
        };
        set_input_policy(policy);
        assert_eq!(input_policy(), policy);
        assert!((input_policy().pointer_scale() - 0.65).abs() < f32::EPSILON);
        set_input_policy(original);
    }

    #[test]
    fn left_handed_mapping_swaps_primary_and_secondary() {
        assert_eq!(
            pointer_button(BTN_LEFT, false),
            Some(egui::PointerButton::Primary)
        );
        assert_eq!(
            pointer_button(BTN_RIGHT, false),
            Some(egui::PointerButton::Secondary)
        );
        assert_eq!(
            pointer_button(BTN_LEFT, true),
            Some(egui::PointerButton::Secondary)
        );
        assert_eq!(
            pointer_button(BTN_RIGHT, true),
            Some(egui::PointerButton::Primary)
        );
        assert_eq!(
            pointer_button(BTN_MIDDLE, true),
            Some(egui::PointerButton::Middle)
        );
        assert_eq!(pointer_button(0xffff, false), None);
    }
}
