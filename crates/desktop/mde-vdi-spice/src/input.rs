//! egui input event → protocol-neutral SPICE input intent.
//!
//! The shell hands raw [`egui::Event`]s to the session; this module turns them
//! into [`SpiceInputEvent`]s — pointer move/button/wheel and key down/up by
//! **PC/AT set-1 scancode**. SPICE's inputs channel is scancode-only (there is no
//! unicode-keyboard path like RDP's), so a keystroke rides its physical scancode
//! and the guest re-maps it through its own keyboard layout — exactly the scancode
//! identity RDP already carries, so the set-1 map is **shared** (it lives once in
//! [`mde_vdi_core`]); SPICE adds only the wire packing ([`to_spice`]).
//!
//! This module is transport-free and fully unit-tested with synthetic events
//! (governance §7). The thin adapter that turns a [`SpiceInputEvent`] into an
//! actual `spice-client` `send_key_down` / `send_mouse_*` call is layered on top
//! in [`crate::connect`]; keeping the egui→intent mapping pure here means the
//! egui-facing surface is real and tested independent of the wire sender.

use crate::egui::{Event, Key, MouseWheelUnit, PointerButton, Vec2};
use mde_vdi_core::{clamp_u16, dominant_axis, ModKey, ModifierTracker};

// The set-1 scancode identity + map are the single source in mde-vdi-core; SPICE
// re-exports them unchanged so `mde_vdi_spice::{Scancode, scancode_for}` keep
// working, and adds only the SPICE wire packing ([`to_spice`]) on top.
pub use mde_vdi_core::{scancode_for, Scancode};

/// Set-1 scancodes for the three core modifier keys the session synthesises from
/// egui's `Modifiers` snapshot — re-exported from [`mde_vdi_core`] (RDP + SPICE
/// share these identities).
pub(crate) use mde_vdi_core::{ALT_SCANCODE, CTRL_SCANCODE, SHIFT_SCANCODE};

/// Pack a set-1 [`Scancode`] into the `u32` word `spice-client`'s
/// [`send_key_down`](spice_client::SpiceClient) puts on the wire.
///
/// SPICE inputs write the code as little-endian bytes and the QEMU server reads
/// them until the first zero byte, so an `E0`-extended key is packed `0xe0` (low
/// byte) then the make code — `0xe0 | (code << 8)` — and a plain key is just the
/// make code. This is the one SPICE-specific bit of the shared scancode identity,
/// so it stays here rather than in the shared core.
#[must_use]
pub const fn to_spice(scancode: Scancode) -> u32 {
    if scancode.extended {
        0xe0 | ((scancode.code as u32) << 8)
    } else {
        scancode.code as u32
    }
}

/// A mouse button, protocol-neutral.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    /// Primary (usually left).
    Left,
    /// Secondary (usually right).
    Right,
    /// Wheel / middle button.
    Middle,
}

/// A protocol-neutral input intent derived from an egui event. The session feeds
/// these to the SPICE inputs channel (and tracks pointer position from them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpiceInputEvent {
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
    /// A wheel rotation. `delta` is in notches; positive = forward/up (or right
    /// when `horizontal`). SPICE expresses the wheel as button clicks, so the
    /// connect layer turns each notch into a wheel-button press/release.
    Wheel {
        /// Signed rotation in notches.
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
}

/// Map an egui pointer button to the protocol-neutral [`MouseButton`]. The extra
/// (browser back/forward) buttons SPICE's inputs channel does not model fold onto
/// the middle button.
#[must_use]
const fn map_button(b: PointerButton) -> MouseButton {
    match b {
        PointerButton::Primary => MouseButton::Left,
        PointerButton::Secondary => MouseButton::Right,
        PointerButton::Middle | PointerButton::Extra1 | PointerButton::Extra2 => {
            MouseButton::Middle
        }
    }
}

/// Map an egui wheel event to a single [`SpiceInputEvent::Wheel`] (the dominant
/// axis, in notches). Returns `None` for a zero rotation.
#[allow(
    clippy::cast_possible_truncation,
    reason = "notch count is clamped into the i16 range before the cast"
)]
fn map_wheel(unit: MouseWheelUnit, delta: Vec2) -> Option<SpiceInputEvent> {
    // Pick the dominant axis (vertical wins ties); the shared core owns the choice.
    let (value, horizontal) = dominant_axis(delta);
    // Line/Page scroll is already in notches; pixel ("Point") scroll is quantised
    // to whole notches (SPICE expresses the wheel as discrete button clicks).
    let notches = match unit {
        MouseWheelUnit::Line | MouseWheelUnit::Page => value,
        MouseWheelUnit::Point => value / 120.0,
    };
    let rounded = notches
        .round()
        .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
    if rounded == 0 {
        None
    } else {
        Some(SpiceInputEvent::Wheel {
            delta: rounded,
            horizontal,
        })
    }
}

/// Tracks the modifier state already pushed to the guest.
///
/// The session diffs this against each event's modifier snapshot to emit the right
/// Shift/Ctrl/Alt key transitions (egui carries modifiers as a snapshot on every
/// event rather than as discrete key events). A thin wrapper over the shared
/// [`ModifierTracker`]: the diff algorithm lives in the core, and SPICE renders
/// each transition as a [`SpiceInputEvent::Key`] by set-1 scancode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModifierState(ModifierTracker);

impl ModifierState {
    /// Diff the stored state against the live `(shift, ctrl, alt)` snapshot, update
    /// self, and return the modifier key transitions to send first. Releases are
    /// emitted before presses so a chord re-press is unambiguous.
    pub fn diff(&mut self, shift: bool, ctrl: bool, alt: bool) -> Vec<SpiceInputEvent> {
        self.0
            .diff(shift, ctrl, alt, |key: ModKey, down| SpiceInputEvent::Key {
                scancode: key.scancode(),
                down,
            })
    }
}

/// Map a single egui [`Event`] to one [`SpiceInputEvent`], or `None` if it carries
/// no SPICE input.
///
/// Focus changes, IME and text commits map to `None` — SPICE keyboard is
/// scancode-only, so a `Key` event carries the input, not the `Text` commit.
/// Keyboard mapping prefers the **physical** key (layout-independent) when egui
/// provides it, falling back to the logical key.
#[must_use]
pub fn map_event(event: &Event) -> Option<SpiceInputEvent> {
    match event {
        Event::PointerMoved(pos) => Some(SpiceInputEvent::PointerMove {
            x: clamp_u16(pos.x),
            y: clamp_u16(pos.y),
        }),
        Event::PointerButton {
            pos,
            button,
            pressed,
            ..
        } => Some(SpiceInputEvent::PointerButton {
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
            scancode_for(k).map(|scancode| SpiceInputEvent::Key {
                scancode,
                down: *pressed,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        map_button, map_event, scancode_for, to_spice, ModifierState, MouseButton, Scancode,
        SpiceInputEvent, ALT_SCANCODE, CTRL_SCANCODE, SHIFT_SCANCODE,
    };
    use crate::egui::{Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, Vec2};

    #[test]
    fn scancode_map_is_the_shared_core_source() {
        // The set-1 table lives once in mde-vdi-core; SPICE re-exports it, so a
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
            Some(SpiceInputEvent::PointerMove { x: 13, y: 0 }) // rounds, clamps <0 to 0
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
            Some(SpiceInputEvent::PointerButton {
                button: MouseButton::Right,
                down: true,
                x: 40,
                y: 50,
            })
        );
    }

    #[test]
    fn extra_buttons_fold_onto_middle() {
        assert_eq!(map_button(PointerButton::Primary), MouseButton::Left);
        assert_eq!(map_button(PointerButton::Secondary), MouseButton::Right);
        assert_eq!(map_button(PointerButton::Middle), MouseButton::Middle);
        assert_eq!(map_button(PointerButton::Extra1), MouseButton::Middle);
        assert_eq!(map_button(PointerButton::Extra2), MouseButton::Middle);
    }

    #[test]
    fn vertical_wheel_is_in_notches() {
        let ev = Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: Vec2::new(0.0, 2.0),
            modifiers: Modifiers::default(),
        };
        assert_eq!(
            map_event(&ev),
            Some(SpiceInputEvent::Wheel {
                delta: 2,
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
            Some(SpiceInputEvent::Wheel {
                delta: -1,
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
            Some(SpiceInputEvent::Key {
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
            Some(SpiceInputEvent::Key {
                scancode: Scancode {
                    code: 0x10, // Q
                    extended: false,
                },
                down: false,
            })
        );
    }

    #[test]
    fn extended_scancode_packs_the_e0_prefix() {
        // Plain key: just the make code.
        assert_eq!(to_spice(Scancode::plain(0x1E)), 0x1E);
        // Extended key (Up arrow, 0x48): 0xe0 low byte, code next → 0x48e0.
        assert_eq!(to_spice(Scancode::ext(0x48)), 0x48e0);
        // Little-endian on the wire → bytes [0xe0, 0x48, 0, 0], which the QEMU
        // server reads as the 0xe0 prefix then the make code.
        assert_eq!(
            to_spice(Scancode::ext(0x48)).to_le_bytes(),
            [0xe0, 0x48, 0, 0]
        );
    }

    #[test]
    fn text_event_is_not_mapped() {
        // SPICE keyboard is scancode-only: the physical Key event carries the
        // input, and the Text commit is dropped (no unicode path).
        assert_eq!(map_event(&Event::Text("é".to_string())), None);
    }

    #[test]
    fn modifier_diff_emits_press_then_holds_state() {
        let mut m = ModifierState::default();
        assert_eq!(
            m.diff(true, false, false),
            vec![SpiceInputEvent::Key {
                scancode: SHIFT_SCANCODE,
                down: true,
            }]
        );
        assert!(m.diff(true, false, false).is_empty());
        // Shift up, Ctrl down in one step: release emitted before press.
        assert_eq!(
            m.diff(false, true, false),
            vec![
                SpiceInputEvent::Key {
                    scancode: SHIFT_SCANCODE,
                    down: false,
                },
                SpiceInputEvent::Key {
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
            vec![SpiceInputEvent::Key {
                scancode: ALT_SCANCODE,
                down: true,
            }]
        );
    }
}
