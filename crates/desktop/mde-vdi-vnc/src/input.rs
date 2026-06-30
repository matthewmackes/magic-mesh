//! egui input event → protocol-neutral RFB input intent + the X11 keysym map.
//!
//! The shell hands raw [`egui::Event`]s to the session; this module turns them
//! into [`VncInputEvent`]s — pointer move / button / wheel, and key down/up by
//! **X11 keysym** (RFB keyboard input is keysym-based, not scancode-based, unlike
//! RDP). It is fully unit-tested with synthetic events (governance §7). The
//! session ([`crate::session`]) resolves these intents into wire-ready RFB
//! `PointerEvent` / `KeyEvent` messages ([`crate::wire`]), tracking the pointer
//! button mask and the modifier state egui reports as a per-event snapshot.

use crate::egui::{Event, Key, MouseWheelUnit, PointerButton, Vec2};

/// A protocol-neutral pointer button.
///
/// RFB carries a button *mask* (the live state of buttons 1–8) in every
/// `PointerEvent`; this names the three core buttons plus the "back" extra
/// (button 8). Wheel rotation is a separate [`VncInputEvent::Wheel`] the session
/// expands into button 4–7 clicks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// Primary / left — RFB button 1 (mask bit 0).
    Left,
    /// Wheel / middle — RFB button 2 (mask bit 1).
    Middle,
    /// Secondary / right — RFB button 3 (mask bit 2).
    Right,
    /// Extra "back" — RFB button 8 (mask bit 7).
    Back,
}

impl Button {
    /// The RFB button-mask bit this button sets.
    #[must_use]
    pub const fn mask_bit(self) -> u8 {
        match self {
            Self::Left => 0x01,
            Self::Middle => 0x02,
            Self::Right => 0x04,
            Self::Back => 0x80,
        }
    }
}

/// A protocol-neutral input intent derived from an egui event.
///
/// The session resolves these into RFB wire messages, tracking the pointer
/// position + button mask (which RFB sends in full on every pointer event).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VncInputEvent {
    /// Absolute pointer move to `(x, y)` in framebuffer pixels (buttons unchanged).
    PointerMove {
        /// X in framebuffer pixels.
        x: u16,
        /// Y in framebuffer pixels.
        y: u16,
    },
    /// A pointer button transition at `(x, y)`.
    PointerButton {
        /// Which button.
        button: Button,
        /// `true` = pressed, `false` = released.
        down: bool,
        /// X in framebuffer pixels.
        x: u16,
        /// Y in framebuffer pixels.
        y: u16,
    },
    /// A wheel rotation, in notches. Positive = up (or right when `horizontal`);
    /// the session emits `|delta|` button-4/5/6/7 click pairs.
    Wheel {
        /// Signed notch count.
        delta: i16,
        /// `true` = horizontal wheel, `false` = vertical.
        horizontal: bool,
    },
    /// A keyboard key transition by X11 keysym.
    Key {
        /// The X11 keysym.
        keysym: u32,
        /// `true` = pressed, `false` = released.
        down: bool,
    },
}

/// X11 keysyms for the three core modifier keys the session synthesises from
/// egui's `Modifiers` snapshot (egui reports modifiers as state, not as discrete
/// key events). Left-hand variants, matching the common server keymaps.
pub(crate) const SHIFT_KEYSYM: u32 = 0xFFE1; // Shift_L
pub(crate) const CTRL_KEYSYM: u32 = 0xFFE3; // Control_L
pub(crate) const ALT_KEYSYM: u32 = 0xFFE9; // Alt_L

/// egui logical Point (pixel) scroll distance treated as one wheel notch.
const POINTS_PER_NOTCH: f32 = 50.0;

/// Round + clamp an egui logical coordinate into the `u16` framebuffer-pixel
/// range.
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

/// Map an egui pointer button to the protocol-neutral [`Button`], or `None` for a
/// button RFB's 8-bit mask has no slot for (egui's `Extra2` / "forward").
#[must_use]
pub const fn map_button(b: PointerButton) -> Option<Button> {
    match b {
        PointerButton::Primary => Some(Button::Left),
        PointerButton::Middle => Some(Button::Middle),
        PointerButton::Secondary => Some(Button::Right),
        PointerButton::Extra1 => Some(Button::Back),
        PointerButton::Extra2 => None,
    }
}

/// Map an egui wheel event to a single [`VncInputEvent::Wheel`] (the dominant
/// axis, vertical wins ties). Returns `None` for a sub-notch / zero rotation.
#[allow(
    clippy::cast_possible_truncation,
    reason = "notch count is clamped into the i16 range before the cast"
)]
fn map_wheel(unit: MouseWheelUnit, delta: Vec2) -> Option<VncInputEvent> {
    let (value, horizontal) = if delta.y.abs() >= delta.x.abs() {
        (delta.y, false)
    } else {
        (delta.x, true)
    };
    if value == 0.0 {
        return None;
    }
    let notches = match unit {
        MouseWheelUnit::Line | MouseWheelUnit::Page => value,
        MouseWheelUnit::Point => value / POINTS_PER_NOTCH,
    };
    let rounded = notches
        .round()
        .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
    if rounded == 0 {
        None
    } else {
        Some(VncInputEvent::Wheel {
            delta: rounded,
            horizontal,
        })
    }
}

/// The X11 keysym for an egui [`Key`], or `None` for a key with no stable keysym.
///
/// (Such keys are handled as composed text via [`map_text`] instead.)
/// Letters/digits/ASCII punctuation map to their **base** (unshifted) keysym —
/// the session sends the `Shift_L` keysym separately, exactly as a real keyboard
/// does.
#[must_use]
pub const fn keysym_for(key: Key) -> Option<u32> {
    use Key as K;
    let sym = match key {
        // ── Letters → lowercase ASCII keysym (0x61..=0x7A) ──────────────────
        K::A => 0x61,
        K::B => 0x62,
        K::C => 0x63,
        K::D => 0x64,
        K::E => 0x65,
        K::F => 0x66,
        K::G => 0x67,
        K::H => 0x68,
        K::I => 0x69,
        K::J => 0x6A,
        K::K => 0x6B,
        K::L => 0x6C,
        K::M => 0x6D,
        K::N => 0x6E,
        K::O => 0x6F,
        K::P => 0x70,
        K::Q => 0x71,
        K::R => 0x72,
        K::S => 0x73,
        K::T => 0x74,
        K::U => 0x75,
        K::V => 0x76,
        K::W => 0x77,
        K::X => 0x78,
        K::Y => 0x79,
        K::Z => 0x7A,
        // ── Number row → ASCII digit keysym (0x30..=0x39) ───────────────────
        K::Num0 => 0x30,
        K::Num1 => 0x31,
        K::Num2 => 0x32,
        K::Num3 => 0x33,
        K::Num4 => 0x34,
        K::Num5 => 0x35,
        K::Num6 => 0x36,
        K::Num7 => 0x37,
        K::Num8 => 0x38,
        K::Num9 => 0x39,
        // ── Whitespace / editing (the 0xFF00 function block) ────────────────
        K::Escape => 0xFF1B,
        K::Backspace => 0xFF08,
        K::Tab => 0xFF09,
        K::Enter => 0xFF0D,
        K::Space => 0x20,
        // ── ASCII punctuation → its unshifted keysym ────────────────────────
        K::Minus => 0x2D,
        K::Equals => 0x3D,
        K::OpenBracket => 0x5B,
        K::CloseBracket => 0x5D,
        K::Backslash => 0x5C,
        K::Semicolon => 0x3B,
        K::Backtick => 0x60,
        K::Comma => 0x2C,
        K::Period => 0x2E,
        K::Slash => 0x2F,
        // ── Function row (F1..F12) ──────────────────────────────────────────
        K::F1 => 0xFFBE,
        K::F2 => 0xFFBF,
        K::F3 => 0xFFC0,
        K::F4 => 0xFFC1,
        K::F5 => 0xFFC2,
        K::F6 => 0xFFC3,
        K::F7 => 0xFFC4,
        K::F8 => 0xFFC5,
        K::F9 => 0xFFC6,
        K::F10 => 0xFFC7,
        K::F11 => 0xFFC8,
        K::F12 => 0xFFC9,
        // ── Editing / navigation cluster ────────────────────────────────────
        K::Insert => 0xFF63,
        K::Delete => 0xFFFF,
        K::Home => 0xFF50,
        K::End => 0xFF57,
        K::PageUp => 0xFF55,
        K::PageDown => 0xFF56,
        K::ArrowUp => 0xFF52,
        K::ArrowDown => 0xFF54,
        K::ArrowLeft => 0xFF51,
        K::ArrowRight => 0xFF53,
        // Any other key (media, IME, ...) has no stable keysym here.
        _ => return None,
    };
    Some(sym)
}

/// The X11 keysym for a Unicode character (the composed / IME / shifted path).
///
/// Latin-1 (and ASCII) characters are their own keysym; the control characters
/// map to the function block; anything else uses the X11 Unicode convention
/// (`0x0100_0000 + codepoint`).
#[must_use]
pub fn keysym_for_char(c: char) -> u32 {
    let cp = c as u32;
    match c {
        '\u{08}' => 0xFF08,                     // BackSpace
        '\t' => 0xFF09,                         // Tab
        '\n' | '\r' => 0xFF0D,                  // Return
        '\u{1B}' => 0xFF1B,                     // Escape
        '\u{7F}' => 0xFFFF,                     // Delete
        _ if (0x20..=0xFF).contains(&cp) => cp, // ASCII printable + Latin-1
        _ => 0x0100_0000 + cp,                  // X11 Unicode keysym
    }
}

/// Map a single egui [`Event`] to one [`VncInputEvent`], or `None` if the event
/// carries no RFB input (focus changes, IME, text — text goes via [`map_text`]).
///
/// Keyboard mapping prefers the **physical** key (layout-independent) when egui
/// provides it, falling back to the logical key.
#[must_use]
pub fn map_event(event: &Event) -> Option<VncInputEvent> {
    match event {
        Event::PointerMoved(pos) => Some(VncInputEvent::PointerMove {
            x: clamp_u16(pos.x),
            y: clamp_u16(pos.y),
        }),
        Event::PointerButton {
            pos,
            button,
            pressed,
            ..
        } => map_button(*button).map(|button| VncInputEvent::PointerButton {
            button,
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
            keysym_for(k).map(|keysym| VncInputEvent::Key {
                keysym,
                down: *pressed,
            })
        }
        _ => None,
    }
}

/// Map an egui text-commit string to a press+release [`VncInputEvent::Key`] pair
/// per character.
///
/// RFB has no unicode-typing path — a character is a keysym down then up. Used
/// for composed / IME / shifted text.
#[must_use]
pub fn map_text(text: &str) -> Vec<VncInputEvent> {
    let mut out = Vec::with_capacity(text.chars().count() * 2);
    for c in text.chars() {
        let keysym = keysym_for_char(c);
        out.push(VncInputEvent::Key { keysym, down: true });
        out.push(VncInputEvent::Key {
            keysym,
            down: false,
        });
    }
    out
}

/// Tracks the modifier state already pushed to the guest.
///
/// Lets the session emit the right `Shift_L` / `Control_L` / `Alt_L` keysym
/// transitions: egui carries modifiers as a snapshot on every event rather than
/// as discrete key events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModifierState {
    shift: bool,
    ctrl: bool,
    alt: bool,
}

impl ModifierState {
    /// Diff the stored state against the live `(shift, ctrl, alt)` snapshot,
    /// update self, and return the modifier keysym transitions to send first.
    /// Releases are emitted before presses so a chord re-press is unambiguous.
    pub fn diff(&mut self, shift: bool, ctrl: bool, alt: bool) -> Vec<VncInputEvent> {
        let mut out = Vec::new();
        let mut emit = |changed: bool, now: bool, keysym: u32, want_down: bool| {
            if changed && now == want_down {
                out.push(VncInputEvent::Key { keysym, down: now });
            }
        };
        // Pass 1: releases (want_down == false).
        emit(self.shift != shift, shift, SHIFT_KEYSYM, false);
        emit(self.ctrl != ctrl, ctrl, CTRL_KEYSYM, false);
        emit(self.alt != alt, alt, ALT_KEYSYM, false);
        // Pass 2: presses (want_down == true).
        emit(self.shift != shift, shift, SHIFT_KEYSYM, true);
        emit(self.ctrl != ctrl, ctrl, CTRL_KEYSYM, true);
        emit(self.alt != alt, alt, ALT_KEYSYM, true);
        self.shift = shift;
        self.ctrl = ctrl;
        self.alt = alt;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{
        keysym_for, keysym_for_char, map_button, map_event, map_text, Button, ModifierState,
        VncInputEvent, ALT_KEYSYM, CTRL_KEYSYM, SHIFT_KEYSYM,
    };
    use crate::egui::{Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, Vec2};

    #[test]
    fn pointer_move_maps_and_clamps() {
        let ev = Event::PointerMoved(Pos2::new(12.6, -3.0));
        assert_eq!(
            map_event(&ev),
            Some(VncInputEvent::PointerMove { x: 13, y: 0 })
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
            Some(VncInputEvent::PointerButton {
                button: Button::Right,
                down: true,
                x: 40,
                y: 50,
            })
        );
    }

    #[test]
    fn button_mapping_and_mask_bits() {
        assert_eq!(map_button(PointerButton::Primary), Some(Button::Left));
        assert_eq!(map_button(PointerButton::Middle), Some(Button::Middle));
        assert_eq!(map_button(PointerButton::Secondary), Some(Button::Right));
        assert_eq!(map_button(PointerButton::Extra1), Some(Button::Back));
        assert_eq!(map_button(PointerButton::Extra2), None, "no RFB slot");
        assert_eq!(Button::Left.mask_bit(), 0x01);
        assert_eq!(Button::Middle.mask_bit(), 0x02);
        assert_eq!(Button::Right.mask_bit(), 0x04);
        assert_eq!(Button::Back.mask_bit(), 0x80);
    }

    #[test]
    fn extra2_button_event_is_dropped() {
        let ev = Event::PointerButton {
            pos: Pos2::new(1.0, 1.0),
            button: PointerButton::Extra2,
            pressed: true,
            modifiers: Modifiers::default(),
        };
        assert_eq!(map_event(&ev), None);
    }

    #[test]
    fn vertical_wheel_notches() {
        let ev = Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: Vec2::new(0.0, 2.0),
            modifiers: Modifiers::default(),
        };
        assert_eq!(
            map_event(&ev),
            Some(VncInputEvent::Wheel {
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
            Some(VncInputEvent::Wheel {
                delta: -1,
                horizontal: true,
            })
        );
    }

    #[test]
    fn pixel_wheel_below_one_notch_is_dropped() {
        let ev = Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: Vec2::new(0.0, 10.0), // < POINTS_PER_NOTCH
            modifiers: Modifiers::default(),
        };
        assert_eq!(map_event(&ev), None);
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
    fn letter_key_maps_to_lowercase_keysym() {
        let ev = Event::Key {
            key: Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::default(),
        };
        assert_eq!(
            map_event(&ev),
            Some(VncInputEvent::Key {
                keysym: 0x61,
                down: true,
            })
        );
    }

    #[test]
    fn key_prefers_physical_over_logical() {
        let ev = Event::Key {
            key: Key::A,
            physical_key: Some(Key::Q),
            pressed: false,
            repeat: false,
            modifiers: Modifiers::default(),
        };
        assert_eq!(
            map_event(&ev),
            Some(VncInputEvent::Key {
                keysym: 0x71, // q
                down: false,
            })
        );
    }

    #[test]
    fn navigation_and_function_keysyms() {
        assert_eq!(keysym_for(Key::ArrowUp), Some(0xFF52));
        assert_eq!(keysym_for(Key::ArrowDown), Some(0xFF54));
        assert_eq!(keysym_for(Key::ArrowLeft), Some(0xFF51));
        assert_eq!(keysym_for(Key::ArrowRight), Some(0xFF53));
        assert_eq!(keysym_for(Key::Home), Some(0xFF50));
        assert_eq!(keysym_for(Key::End), Some(0xFF57));
        assert_eq!(keysym_for(Key::Delete), Some(0xFFFF));
        assert_eq!(keysym_for(Key::Enter), Some(0xFF0D));
        assert_eq!(keysym_for(Key::Escape), Some(0xFF1B));
        assert_eq!(keysym_for(Key::F1), Some(0xFFBE));
        assert_eq!(keysym_for(Key::F12), Some(0xFFC9));
        assert_eq!(keysym_for(Key::Space), Some(0x20));
        assert_eq!(keysym_for(Key::Slash), Some(0x2F));
    }

    #[test]
    fn keysyms_are_unique_across_the_mapped_set() {
        let keys = [
            Key::A,
            Key::Z,
            Key::Num0,
            Key::Num9,
            Key::Escape,
            Key::Enter,
            Key::Space,
            Key::Tab,
            Key::Minus,
            Key::Slash,
            Key::F1,
            Key::F12,
            Key::ArrowUp,
            Key::ArrowDown,
            Key::ArrowLeft,
            Key::ArrowRight,
            Key::Home,
            Key::End,
            Key::Insert,
            Key::Delete,
            Key::PageUp,
            Key::PageDown,
        ];
        let mut seen = std::collections::HashSet::new();
        for k in keys {
            let sym = keysym_for(k).expect("mapped key");
            assert!(seen.insert(sym), "{k:?} collides on keysym {sym:#x}");
        }
    }

    #[test]
    fn char_keysyms_cover_ascii_latin1_and_unicode() {
        assert_eq!(keysym_for_char('a'), 0x61);
        assert_eq!(keysym_for_char('A'), 0x41);
        assert_eq!(keysym_for_char(' '), 0x20);
        assert_eq!(keysym_for_char('\n'), 0xFF0D);
        assert_eq!(keysym_for_char('é'), 0xE9, "Latin-1 is its own keysym");
        assert_eq!(keysym_for_char('€'), 0x0100_0000 + 0x20AC, "unicode keysym");
    }

    #[test]
    fn text_event_is_not_mapped_by_map_event() {
        assert_eq!(map_event(&Event::Text("é".to_string())), None);
    }

    #[test]
    fn map_text_yields_press_release_per_char() {
        assert_eq!(
            map_text("a"),
            vec![
                VncInputEvent::Key {
                    keysym: 0x61,
                    down: true,
                },
                VncInputEvent::Key {
                    keysym: 0x61,
                    down: false,
                },
            ]
        );
        assert!(map_text("").is_empty());
    }

    #[test]
    fn modifier_diff_emits_press_then_holds_state() {
        let mut m = ModifierState::default();
        assert_eq!(
            m.diff(true, false, false),
            vec![VncInputEvent::Key {
                keysym: SHIFT_KEYSYM,
                down: true,
            }]
        );
        assert!(m.diff(true, false, false).is_empty());
        // Shift up, Ctrl down in one step: release emitted before press.
        assert_eq!(
            m.diff(false, true, false),
            vec![
                VncInputEvent::Key {
                    keysym: SHIFT_KEYSYM,
                    down: false,
                },
                VncInputEvent::Key {
                    keysym: CTRL_KEYSYM,
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
            vec![VncInputEvent::Key {
                keysym: ALT_KEYSYM,
                down: true,
            }]
        );
    }
}
