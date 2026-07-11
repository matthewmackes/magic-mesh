//! egui input event → protocol-neutral RDP input intent.
//!
//! The shell hands raw [`egui::Event`]s to the session; this module turns them
//! into [`RdpInputEvent`]s — pointer move/button/wheel, key down/up by **hardware
//! scancode**, and unicode text. It is `ironrdp`-free and fully unit-tested with
//! synthetic events (governance §7). The thin adapter that turns an
//! [`RdpInputEvent`] into an actual `ironrdp` input PDU is layered on top in
//! `connect` (behind the `live-connect` feature); keeping the egui→intent mapping
//! pure here means the egui-facing surface is real and tested independent of the
//! wire encoder.
//!
//! The transport-neutral pieces — the [`Scancode`] identity + the PC/AT set-1
//! [`scancode_for`] map (shared with SPICE), the coordinate/wheel helpers, and the
//! generic modifier-diff — live in [`mde_vdi_core`]; this module re-exports the
//! scancode surface unchanged and wires the RDP-specific event shape on top.

use crate::egui::{Event, MouseWheelUnit, PointerButton, Vec2};
use mde_vdi_core::{clamp_u16, dominant_axis, ModKey, ModifierTracker};

// The set-1 scancode identity + map are the single source in mde-vdi-core; RDP
// re-exports them unchanged so `mde_vdi_rdp::{Scancode, scancode_for}` keep working.
pub use mde_vdi_core::{scancode_for, Scancode};

/// A mouse button, protocol-neutral.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    /// Primary (usually left).
    Left,
    /// Secondary (usually right).
    Right,
    /// Wheel / middle button.
    Middle,
    /// Extra button 1 (browser-back).
    X1,
    /// Extra button 2 (browser-forward).
    X2,
}

/// A protocol-neutral input intent derived from an egui event. The session feeds
/// these to the `ironrdp` input state machine (and tracks pointer position from
/// them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RdpInputEvent {
    /// Absolute pointer move to `(x, y)` in desktop pixels.
    PointerMove {
        /// X in desktop pixels.
        x: u16,
        /// Y in desktop pixels.
        y: u16,
    },
    /// A pointer button transition at `(x, y)`.
    PointerButton {
        /// Which button.
        button: MouseButton,
        /// `true` = pressed, `false` = released.
        down: bool,
        /// X in desktop pixels (keeps the guest's pointer position in sync).
        x: u16,
        /// Y in desktop pixels.
        y: u16,
    },
    /// A wheel rotation. `delta` is in RDP rotation units (120 = one notch);
    /// positive = forward/up (or right when `horizontal`).
    Wheel {
        /// Signed rotation in RDP units.
        delta: i16,
        /// `true` = horizontal wheel, `false` = vertical.
        horizontal: bool,
    },
    /// A keyboard key transition by hardware scancode.
    Key {
        /// The set-1 scancode.
        scancode: Scancode,
        /// `true` = pressed, `false` = released.
        down: bool,
    },
    /// A unicode character (RDP fast-path unicode keyboard input) — used for text
    /// that has no clean scancode (composed / IME / AltGr characters).
    Unicode(char),
}

/// Map an egui pointer button to the protocol-neutral [`MouseButton`].
#[must_use]
const fn map_button(b: PointerButton) -> MouseButton {
    match b {
        PointerButton::Primary => MouseButton::Left,
        PointerButton::Secondary => MouseButton::Right,
        PointerButton::Middle => MouseButton::Middle,
        PointerButton::Extra1 => MouseButton::X1,
        PointerButton::Extra2 => MouseButton::X2,
    }
}

/// Map an egui wheel event to a single [`RdpInputEvent::Wheel`] (the dominant
/// axis). Returns `None` for a zero rotation.
#[allow(
    clippy::cast_possible_truncation,
    reason = "rotation is clamped into the i16 range before the cast"
)]
fn map_wheel(unit: MouseWheelUnit, delta: Vec2) -> Option<RdpInputEvent> {
    // Pick the dominant axis (vertical wins ties); the shared core owns the choice.
    let (value, horizontal) = dominant_axis(delta);
    if value == 0.0 {
        return None;
    }
    // RDP rotation is in WHEEL_DELTA (120) units per notch. Line/Page scroll is in
    // notches; pixel ("Point") scroll is mapped 1:1 into the finer rotation range.
    let per_notch = match unit {
        MouseWheelUnit::Line | MouseWheelUnit::Page => 120.0,
        MouseWheelUnit::Point => 1.0,
    };
    let rotation = (value * per_notch)
        .round()
        .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
    if rotation == 0 {
        None
    } else {
        Some(RdpInputEvent::Wheel {
            delta: rotation,
            horizontal,
        })
    }
}

/// Tracks the modifier state already pushed to the guest so the session can emit
/// the right modifier key transitions (egui carries modifiers as a snapshot on
/// every event rather than as discrete Shift/Ctrl/Alt key events).
///
/// A thin wrapper over the shared [`ModifierTracker`]: the diff algorithm lives in
/// the core, and RDP renders each transition as an [`RdpInputEvent::Key`] by set-1
/// scancode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModifierState(ModifierTracker);

impl ModifierState {
    /// Diff the stored state against the live `(shift, ctrl, alt)` snapshot, update
    /// self, and return the modifier key transitions to send first. Releases are
    /// emitted before presses so a chord re-press is unambiguous.
    pub fn diff(&mut self, shift: bool, ctrl: bool, alt: bool) -> Vec<RdpInputEvent> {
        self.0
            .diff(shift, ctrl, alt, |key: ModKey, down| RdpInputEvent::Key {
                scancode: key.scancode(),
                down,
            })
    }
}

/// Map a single egui [`Event`] to one [`RdpInputEvent`], or `None` if the event
/// carries no RDP input (focus changes, IME, etc.). Text input that needs unicode
/// is handled separately by [`map_text`].
///
/// Keyboard mapping prefers the **physical** key (layout-independent) when egui
/// provides it, falling back to the logical key.
#[must_use]
pub fn map_event(event: &Event) -> Option<RdpInputEvent> {
    match event {
        Event::PointerMoved(pos) => Some(RdpInputEvent::PointerMove {
            x: clamp_u16(pos.x),
            y: clamp_u16(pos.y),
        }),
        Event::PointerButton {
            pos,
            button,
            pressed,
            ..
        } => Some(RdpInputEvent::PointerButton {
            button: map_button(*button),
            down: *pressed,
            x: clamp_u16(pos.x),
            y: clamp_u16(pos.y),
        }),
        Event::MouseWheel { unit, delta, .. } => map_wheel(*unit, *delta),
        Event::Key {
            key,
            physical_key,
            pressed,
            ..
        } => {
            let k = physical_key.unwrap_or(*key);
            scancode_for(k).map(|scancode| RdpInputEvent::Key {
                scancode,
                down: *pressed,
            })
        }
        _ => None,
    }
}

/// Map an egui text-commit string to a unicode key event per character. Used for
/// composed / IME / AltGr text that has no clean scancode path.
#[must_use]
pub fn map_text(text: &str) -> Vec<RdpInputEvent> {
    text.chars().map(RdpInputEvent::Unicode).collect()
}

#[cfg(test)]
mod tests {
    use super::{
        map_button, map_event, map_text, scancode_for, ModifierState, MouseButton, RdpInputEvent,
        Scancode,
    };
    use crate::egui::{Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, Vec2};
    use mde_vdi_core::{ALT_SCANCODE, CTRL_SCANCODE, SHIFT_SCANCODE};

    #[test]
    fn scancode_map_is_the_shared_core_source() {
        // The set-1 table lives once in mde-vdi-core; RDP re-exports it, so a
        // scancode fix is applied in exactly one place for RDP + SPICE.
        for key in [
            Key::A,
            Key::Z,
            Key::Enter,
            Key::ArrowUp,
            Key::F12,
            Key::Slash,
        ] {
            assert_eq!(scancode_for(key), mde_vdi_core::scancode_for(key));
        }
    }

    #[test]
    fn pointer_move_maps_and_clamps() {
        let ev = Event::PointerMoved(Pos2::new(12.6, -3.0));
        assert_eq!(
            map_event(&ev),
            Some(RdpInputEvent::PointerMove { x: 13, y: 0 }) // rounds, clamps <0 to 0
        );
    }

    #[test]
    fn pointer_button_maps_with_position() {
        let ev = Event::PointerButton {
            pos: Pos2::new(40.0, 50.0),
            button: PointerButton::Secondary,
            pressed: true,
            modifiers: Modifiers::default(),
        };
        assert_eq!(
            map_event(&ev),
            Some(RdpInputEvent::PointerButton {
                button: MouseButton::Right,
                down: true,
                x: 40,
                y: 50,
            })
        );
    }

    #[test]
    fn all_pointer_buttons_map() {
        assert_eq!(map_button(PointerButton::Primary), MouseButton::Left);
        assert_eq!(map_button(PointerButton::Secondary), MouseButton::Right);
        assert_eq!(map_button(PointerButton::Middle), MouseButton::Middle);
        assert_eq!(map_button(PointerButton::Extra1), MouseButton::X1);
        assert_eq!(map_button(PointerButton::Extra2), MouseButton::X2);
    }

    #[test]
    fn vertical_wheel_uses_rdp_rotation_units() {
        let ev = Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: Vec2::new(0.0, 2.0),
            modifiers: Modifiers::default(),
        };
        assert_eq!(
            map_event(&ev),
            Some(RdpInputEvent::Wheel {
                delta: 240, // 2 notches * 120
                horizontal: false,
            })
        );
    }

    #[test]
    fn horizontal_wheel_when_x_dominates() {
        let ev = Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: Vec2::new(-1.0, 0.0),
            modifiers: Modifiers::default(),
        };
        assert_eq!(
            map_event(&ev),
            Some(RdpInputEvent::Wheel {
                delta: -120,
                horizontal: true,
            })
        );
    }

    #[test]
    fn zero_wheel_is_dropped() {
        let ev = Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: Vec2::ZERO,
            modifiers: Modifiers::default(),
        };
        assert_eq!(map_event(&ev), None);
    }

    #[test]
    fn key_down_maps_to_scancode() {
        let ev = Event::Key {
            key: Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::default(),
        };
        assert_eq!(
            map_event(&ev),
            Some(RdpInputEvent::Key {
                scancode: Scancode {
                    code: 0x1E,
                    extended: false,
                },
                down: true,
            })
        );
    }

    #[test]
    fn key_prefers_physical_over_logical() {
        // Logical 'A' but physical 'Q' (e.g. a remapped layout) → Q's scancode.
        let ev = Event::Key {
            key: Key::A,
            physical_key: Some(Key::Q),
            pressed: false,
            repeat: false,
            modifiers: Modifiers::default(),
        };
        assert_eq!(
            map_event(&ev),
            Some(RdpInputEvent::Key {
                scancode: Scancode {
                    code: 0x10, // Q
                    extended: false,
                },
                down: false,
            })
        );
    }

    #[test]
    fn text_event_is_not_mapped_by_map_event() {
        // Text needs the unicode path, not a scancode — map_event leaves it.
        assert_eq!(map_event(&Event::Text("é".to_string())), None);
    }

    #[test]
    fn map_text_yields_one_unicode_event_per_char() {
        assert_eq!(
            map_text("aé"),
            vec![RdpInputEvent::Unicode('a'), RdpInputEvent::Unicode('é')]
        );
        assert!(map_text("").is_empty());
    }

    #[test]
    fn modifier_diff_emits_press_then_holds_state() {
        let mut m = ModifierState::default();
        // Shift goes down.
        assert_eq!(
            m.diff(true, false, false),
            vec![RdpInputEvent::Key {
                scancode: SHIFT_SCANCODE,
                down: true,
            }]
        );
        // No change → nothing.
        assert!(m.diff(true, false, false).is_empty());
        // Shift up, Ctrl down in one step: release emitted before press.
        assert_eq!(
            m.diff(false, true, false),
            vec![
                RdpInputEvent::Key {
                    scancode: SHIFT_SCANCODE,
                    down: false,
                },
                RdpInputEvent::Key {
                    scancode: CTRL_SCANCODE,
                    down: true,
                },
            ]
        );
    }

    #[test]
    fn modifier_diff_handles_alt() {
        let mut m = ModifierState::default();
        assert_eq!(
            m.diff(false, false, true),
            vec![RdpInputEvent::Key {
                scancode: ALT_SCANCODE,
                down: true,
            }]
        );
    }
}
