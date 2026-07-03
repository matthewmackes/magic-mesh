//! egui input event → protocol-neutral SPICE input intent + the PC/AT scancode
//! map.
//!
//! The shell hands raw [`egui::Event`]s to the session; this module turns them
//! into [`SpiceInputEvent`]s — pointer move/button/wheel and key down/up by
//! **PC/AT set-1 scancode**. SPICE's inputs channel is scancode-only (there is no
//! unicode-keyboard path like RDP's), so a keystroke rides its physical scancode
//! and the guest re-maps it through its own keyboard layout — exactly the
//! scancode identity RDP already carries, so the set-1 map is shared shape.
//!
//! This module is transport-free and fully unit-tested with synthetic events
//! (governance §7). The thin adapter that turns a [`SpiceInputEvent`] into an
//! actual `spice-client` `send_key_down` / `send_mouse_*` call is layered on top
//! in [`crate::connect`]; keeping the egui→intent mapping pure here means the
//! egui-facing surface is real and tested independent of the wire sender.

use crate::egui::{Event, Key, MouseWheelUnit, PointerButton, Vec2};

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

/// A PC/AT **set-1** hardware scancode plus whether it is an `E0`-extended key.
///
/// The arrow / navigation cluster and right-hand modifiers are extended. SPICE
/// keyboard input is scancode-based, so this is the layout-independent identity
/// the guest re-maps through its own keyboard layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scancode {
    /// The set-1 make code (the session sends down/up as a flag, so only the make
    /// code is carried here).
    pub code: u8,
    /// Whether the key is `E0`-prefixed (extended).
    pub extended: bool,
}

impl Scancode {
    /// A non-extended set-1 scancode.
    #[must_use]
    const fn plain(code: u8) -> Self {
        Self {
            code,
            extended: false,
        }
    }

    /// An `E0`-extended set-1 scancode.
    #[must_use]
    const fn ext(code: u8) -> Self {
        Self {
            code,
            extended: true,
        }
    }

    /// Pack this scancode into the `u32` `spice-client`'s
    /// [`send_key_down`](spice_client::SpiceClient) puts on the wire.
    ///
    /// SPICE inputs write the code as little-endian bytes and the QEMU server
    /// reads them until the first zero byte, so an `E0`-extended key is packed
    /// `0xe0` (low byte) then the make code — `0xe0 | (code << 8)` — and a plain
    /// key is just the make code. Tested in [`mod@tests`].
    #[must_use]
    pub const fn to_spice(self) -> u32 {
        if self.extended {
            0xe0 | ((self.code as u32) << 8)
        } else {
            self.code as u32
        }
    }
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

/// Round + clamp an egui logical coordinate into the `u16` desktop-pixel range.
#[inline]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "value is rounded then clamped into [0, u16::MAX]; NaN maps to 0"
)]
fn clamp_u16(v: f32) -> u16 {
    if v.is_nan() {
        0
    } else {
        v.round().clamp(0.0, f32::from(u16::MAX)) as u16
    }
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
    // Pick the dominant axis so one event maps to one rotation; vertical wins ties.
    let (value, horizontal) = if delta.y.abs() >= delta.x.abs() {
        (delta.y, false)
    } else {
        (delta.x, true)
    };
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

/// The PC/AT **set-1** scancode for an egui [`Key`], or `None` for an unmapped
/// key.
///
/// Covers letters, digits, the function row, the editing/navigation cluster, and
/// the common ASCII punctuation; the navigation/arrow keys are correctly
/// `E0`-extended.
#[must_use]
pub const fn scancode_for(key: Key) -> Option<Scancode> {
    use Key as K;
    let sc = match key {
        // ── Letters (set-1 make codes) ──────────────────────────────────────
        K::A => 0x1E,
        K::B => 0x30,
        K::C => 0x2E,
        K::D => 0x20,
        K::E => 0x12,
        K::F => 0x21,
        K::G => 0x22,
        K::H => 0x23,
        K::I => 0x17,
        K::J => 0x24,
        K::K => 0x25,
        K::L => 0x26,
        K::M => 0x32,
        K::N => 0x31,
        K::O => 0x18,
        K::P => 0x19,
        K::Q => 0x10,
        K::R => 0x13,
        K::S => 0x1F,
        K::T => 0x14,
        K::U => 0x16,
        K::V => 0x2F,
        K::W => 0x11,
        K::X => 0x2D,
        K::Y => 0x15,
        K::Z => 0x2C,
        // ── Number row ──────────────────────────────────────────────────────
        K::Num1 => 0x02,
        K::Num2 => 0x03,
        K::Num3 => 0x04,
        K::Num4 => 0x05,
        K::Num5 => 0x06,
        K::Num6 => 0x07,
        K::Num7 => 0x08,
        K::Num8 => 0x09,
        K::Num9 => 0x0A,
        K::Num0 => 0x0B,
        // ── Whitespace / editing ────────────────────────────────────────────
        K::Escape => 0x01,
        K::Backspace => 0x0E,
        K::Tab => 0x0F,
        K::Enter => 0x1C,
        K::Space => 0x39,
        // ── ASCII punctuation ───────────────────────────────────────────────
        K::Minus => 0x0C,
        K::Equals => 0x0D,
        K::OpenBracket => 0x1A,
        K::CloseBracket => 0x1B,
        K::Backslash => 0x2B,
        K::Semicolon => 0x27,
        K::Backtick => 0x29,
        K::Comma => 0x33,
        K::Period => 0x34,
        K::Slash => 0x35,
        // ── Function row ────────────────────────────────────────────────────
        K::F1 => 0x3B,
        K::F2 => 0x3C,
        K::F3 => 0x3D,
        K::F4 => 0x3E,
        K::F5 => 0x3F,
        K::F6 => 0x40,
        K::F7 => 0x41,
        K::F8 => 0x42,
        K::F9 => 0x43,
        K::F10 => 0x44,
        K::F11 => 0x57,
        K::F12 => 0x58,
        // ── Editing / navigation cluster (E0-extended) ──────────────────────
        K::Insert => return Some(Scancode::ext(0x52)),
        K::Delete => return Some(Scancode::ext(0x53)),
        K::Home => return Some(Scancode::ext(0x47)),
        K::End => return Some(Scancode::ext(0x4F)),
        K::PageUp => return Some(Scancode::ext(0x49)),
        K::PageDown => return Some(Scancode::ext(0x51)),
        K::ArrowUp => return Some(Scancode::ext(0x48)),
        K::ArrowDown => return Some(Scancode::ext(0x50)),
        K::ArrowLeft => return Some(Scancode::ext(0x4B)),
        K::ArrowRight => return Some(Scancode::ext(0x4D)),
        // Any other key (IME, media keys, ...) has no stable scancode here.
        _ => return None,
    };
    Some(Scancode::plain(sc))
}

/// Set-1 scancodes for the three core modifier keys the session synthesises from
/// egui's `Modifiers` snapshot (egui reports modifiers as state, not as discrete
/// key events).
pub(crate) const SHIFT_SCANCODE: Scancode = Scancode::plain(0x2A); // left shift
pub(crate) const CTRL_SCANCODE: Scancode = Scancode::plain(0x1D); // left ctrl
pub(crate) const ALT_SCANCODE: Scancode = Scancode::plain(0x38); // left alt

/// Tracks the modifier state already pushed to the guest.
///
/// The session diffs this against each event's modifier snapshot to emit the
/// right Shift/Ctrl/Alt key transitions (egui carries modifiers as a snapshot on
/// every event rather than as discrete key events).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModifierState {
    shift: bool,
    ctrl: bool,
    alt: bool,
}

impl ModifierState {
    /// Diff the stored state against the live `(shift, ctrl, alt)` snapshot,
    /// update self, and return the modifier key transitions to send first.
    /// Releases are emitted before presses so a chord re-press is unambiguous.
    pub fn diff(&mut self, shift: bool, ctrl: bool, alt: bool) -> Vec<SpiceInputEvent> {
        let mut out = Vec::new();
        let mut emit = |changed: bool, now: bool, sc: Scancode, want_down: bool| {
            if changed && now == want_down {
                out.push(SpiceInputEvent::Key {
                    scancode: sc,
                    down: now,
                });
            }
        };
        // Pass 1: releases (want_down == false).
        emit(self.shift != shift, shift, SHIFT_SCANCODE, false);
        emit(self.ctrl != ctrl, ctrl, CTRL_SCANCODE, false);
        emit(self.alt != alt, alt, ALT_SCANCODE, false);
        // Pass 2: presses (want_down == true).
        emit(self.shift != shift, shift, SHIFT_SCANCODE, true);
        emit(self.ctrl != ctrl, ctrl, CTRL_SCANCODE, true);
        emit(self.alt != alt, alt, ALT_SCANCODE, true);
        self.shift = shift;
        self.ctrl = ctrl;
        self.alt = alt;
        out
    }
}

/// Map a single egui [`Event`] to one [`SpiceInputEvent`], or `None` if it
/// carries no SPICE input.
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
        map_button, map_event, scancode_for, ModifierState, MouseButton, Scancode, SpiceInputEvent,
        ALT_SCANCODE, CTRL_SCANCODE, SHIFT_SCANCODE,
    };
    use crate::egui::{Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, Vec2};

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
    fn navigation_keys_are_extended() {
        for (key, code) in [
            (Key::ArrowUp, 0x48u8),
            (Key::ArrowDown, 0x50),
            (Key::ArrowLeft, 0x4B),
            (Key::ArrowRight, 0x4D),
            (Key::Home, 0x47),
            (Key::End, 0x4F),
            (Key::PageUp, 0x49),
            (Key::PageDown, 0x51),
            (Key::Insert, 0x52),
            (Key::Delete, 0x53),
        ] {
            assert_eq!(
                scancode_for(key),
                Some(Scancode {
                    code,
                    extended: true,
                }),
                "{key:?} must be E0-extended"
            );
        }
    }

    #[test]
    fn extended_scancode_packs_the_e0_prefix() {
        // Plain key: just the make code.
        assert_eq!(Scancode::plain(0x1E).to_spice(), 0x1E);
        // Extended key (Up arrow, 0x48): 0xe0 low byte, code next → 0x48e0.
        assert_eq!(Scancode::ext(0x48).to_spice(), 0x48e0);
        // Little-endian on the wire → bytes [0xe0, 0x48, 0, 0], which the QEMU
        // server reads as the 0xe0 prefix then the make code.
        assert_eq!(
            Scancode::ext(0x48).to_spice().to_le_bytes(),
            [0xe0, 0x48, 0, 0]
        );
    }

    #[test]
    fn function_row_and_punctuation_have_scancodes() {
        assert_eq!(scancode_for(Key::F1), Some(Scancode::plain(0x3B)));
        assert_eq!(scancode_for(Key::F12), Some(Scancode::plain(0x58)));
        assert_eq!(scancode_for(Key::Minus), Some(Scancode::plain(0x0C)));
        assert_eq!(scancode_for(Key::Slash), Some(Scancode::plain(0x35)));
    }

    #[test]
    fn scancodes_are_unique_across_the_mapped_set() {
        let keys = [
            Key::A,
            Key::B,
            Key::Q,
            Key::Z,
            Key::Num0,
            Key::Num1,
            Key::Num9,
            Key::Escape,
            Key::Enter,
            Key::Space,
            Key::Tab,
            Key::Backspace,
            Key::F1,
            Key::F12,
            Key::Minus,
            Key::Equals,
            Key::Slash,
            Key::ArrowUp,
            Key::ArrowDown,
            Key::Home,
            Key::End,
            Key::Insert,
            Key::Delete,
        ];
        let mut seen = std::collections::HashSet::new();
        for k in keys {
            let sc = scancode_for(k).expect("mapped key");
            assert!(seen.insert((sc.code, sc.extended)), "{k:?} collides");
        }
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
