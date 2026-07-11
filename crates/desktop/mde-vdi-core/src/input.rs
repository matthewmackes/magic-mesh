//! The transport-neutral egui-input helpers the VDI backends share.
//!
//! egui reports pointer/keyboard/wheel input the same way to every backend, so the
//! first steps of turning an [`egui::Event`] into protocol input are identical
//! across RDP/VNC/SPICE and live here:
//!
//! * [`clamp_u16`] — round + clamp a logical coordinate into the pixel range.
//! * [`dominant_axis`] — pick the winning scroll axis of a wheel delta (the
//!   per-transport scaling into rotation units / notches stays in each crate).
//! * [`Scancode`] + [`scancode_for`] — the PC/AT **set-1** hardware scancode
//!   identity and map. RDP and SPICE are both scancode-based and share this
//!   exactly; VNC is X11-keysym-based and keeps its own map (a legitimate
//!   divergence, not duplication).
//! * [`ModifierTracker`] + [`ModKey`] — egui carries modifiers as a per-event
//!   snapshot, so every backend diffs that snapshot against the state already
//!   pushed to the guest and emits releases-before-presses. The diff algorithm is
//!   identical; only the emitted event type + the modifier-key identity differ, so
//!   the tracker is generic over both.

use crate::egui::{Key, Vec2};

/// Round + clamp an egui logical coordinate into the `u16` pixel range.
///
/// Shared by every backend's pointer mapping: a desktop coordinate is at most
/// `u16::MAX`, negatives clamp to `0`, and a `NaN` maps to `0` rather than
/// producing an undefined cast.
#[inline]
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "value is rounded then clamped into [0, u16::MAX]; NaN maps to 0"
)]
pub fn clamp_u16(v: f32) -> u16 {
    if v.is_nan() {
        0
    } else {
        v.round().clamp(0.0, f32::from(u16::MAX)) as u16
    }
}

/// Pick the dominant scroll axis of an egui wheel `delta`.
///
/// Returns `(value, horizontal)` where `value` is the signed distance on the
/// winning axis and `horizontal` is `true` when the X axis dominates. The vertical
/// axis wins ties (the common mouse wheel). Each backend applies its own scaling to
/// `value` afterwards — RDP into `WHEEL_DELTA` rotation units, VNC/SPICE into
/// notches — so only the axis choice is shared here.
#[must_use]
pub fn dominant_axis(delta: Vec2) -> (f32, bool) {
    if delta.y.abs() >= delta.x.abs() {
        (delta.y, false)
    } else {
        (delta.x, true)
    }
}

/// A PC/AT **set-1** hardware scancode plus whether it is an `E0`-extended key
/// (the arrow / navigation cluster, right-hand modifiers, etc.).
///
/// RDP and SPICE keyboard input are both scancode-based, so this is the
/// layout-independent identity the guest re-maps through its own keyboard layout.
/// The break code is the make code with bit 7 set; the session carries down/up as a
/// separate flag, so only the make code lives here. SPICE additionally packs this
/// into its wire word (`0xe0`-prefix for extended keys) in its own crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scancode {
    /// The set-1 make code.
    pub code: u8,
    /// Whether the key is `E0`-prefixed (extended).
    pub extended: bool,
}

impl Scancode {
    /// A non-extended set-1 scancode.
    #[must_use]
    pub const fn plain(code: u8) -> Self {
        Self {
            code,
            extended: false,
        }
    }

    /// An `E0`-extended set-1 scancode.
    #[must_use]
    pub const fn ext(code: u8) -> Self {
        Self {
            code,
            extended: true,
        }
    }
}

/// Set-1 scancodes for the three core modifier keys the session synthesises from
/// egui's `Modifiers` snapshot. Left-hand variants — the identity RDP and SPICE
/// both send. Also reachable via [`ModKey::scancode`].
pub const SHIFT_SCANCODE: Scancode = Scancode::plain(0x2A); // left shift
/// Set-1 scancode for left Ctrl.
pub const CTRL_SCANCODE: Scancode = Scancode::plain(0x1D); // left ctrl
/// Set-1 scancode for left Alt.
pub const ALT_SCANCODE: Scancode = Scancode::plain(0x38); // left alt

/// The PC/AT **set-1** scancode for an egui [`Key`], or `None` for a key with no
/// stable scancode (handled as unicode text by the backend instead).
///
/// Covers letters, digits, the function row, the editing/navigation cluster, and
/// the common ASCII punctuation; the navigation/arrow keys are correctly
/// `E0`-extended. This is the **single source** of the set-1 map for both the RDP
/// and SPICE backends — a scancode fix is made here, once.
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

/// One of the three core modifier keys egui reports as a per-event snapshot.
///
/// [`ModifierTracker::diff`] names which modifier changed with this, and each
/// backend turns it into its own event: RDP/SPICE via [`ModKey::scancode`], VNC via
/// its own X11 keysym map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModKey {
    /// The Shift modifier.
    Shift,
    /// The Ctrl modifier.
    Ctrl,
    /// The Alt modifier.
    Alt,
}

impl ModKey {
    /// The PC/AT set-1 scancode for this modifier — the identity RDP and SPICE both
    /// send. (VNC maps the same [`ModKey`] to an X11 keysym instead.)
    #[must_use]
    pub const fn scancode(self) -> Scancode {
        match self {
            Self::Shift => SHIFT_SCANCODE,
            Self::Ctrl => CTRL_SCANCODE,
            Self::Alt => ALT_SCANCODE,
        }
    }
}

/// Tracks the Shift/Ctrl/Alt state already pushed to the guest so a backend can
/// emit the right modifier key transitions.
///
/// egui carries modifiers as a snapshot on every event rather than as discrete
/// Shift/Ctrl/Alt key events, so each backend diffs the live snapshot against this
/// tracker. The diff algorithm — releases before presses so a chord re-press is
/// unambiguous — is identical across transports; only the emitted event type and
/// the per-modifier identity differ, so [`diff`](ModifierTracker::diff) is generic
/// over both via a `make` closure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModifierTracker {
    shift: bool,
    ctrl: bool,
    alt: bool,
}

impl ModifierTracker {
    /// Diff the stored state against the live `(shift, ctrl, alt)` snapshot, update
    /// self, and return the modifier-key transitions to send — **releases before
    /// presses** so a chord re-press is unambiguous. `make` builds each backend's
    /// own event from the [`ModKey`] that changed and its new pressed state.
    pub fn diff<E>(
        &mut self,
        shift: bool,
        ctrl: bool,
        alt: bool,
        mut make: impl FnMut(ModKey, bool) -> E,
    ) -> Vec<E> {
        let changes = [
            (ModKey::Shift, self.shift, shift),
            (ModKey::Ctrl, self.ctrl, ctrl),
            (ModKey::Alt, self.alt, alt),
        ];
        let mut out = Vec::new();
        // Pass 1: releases (the new state is `false`).
        for &(key, was, now) in &changes {
            if was != now && !now {
                out.push(make(key, false));
            }
        }
        // Pass 2: presses (the new state is `true`).
        for &(key, was, now) in &changes {
            if was != now && now {
                out.push(make(key, true));
            }
        }
        self.shift = shift;
        self.ctrl = ctrl;
        self.alt = alt;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{
        clamp_u16, dominant_axis, scancode_for, ModKey, ModifierTracker, Scancode, ALT_SCANCODE,
        CTRL_SCANCODE, SHIFT_SCANCODE,
    };
    use crate::egui::{Key, Vec2};

    #[test]
    fn clamp_rounds_and_clamps() {
        assert_eq!(clamp_u16(12.6), 13);
        assert_eq!(clamp_u16(-3.0), 0, "negatives clamp to 0");
        assert_eq!(
            clamp_u16(f32::from(u16::MAX) + 100.0),
            u16::MAX,
            "clamps high"
        );
        assert_eq!(clamp_u16(f32::NAN), 0, "NaN maps to 0");
    }

    #[test]
    fn dominant_axis_prefers_vertical_on_ties() {
        // Equal magnitude → vertical wins (the common wheel).
        assert_eq!(dominant_axis(Vec2::new(2.0, 2.0)), (2.0, false));
        assert_eq!(dominant_axis(Vec2::new(0.0, -1.0)), (-1.0, false));
        // X strictly dominates → horizontal.
        assert_eq!(dominant_axis(Vec2::new(-3.0, 1.0)), (-3.0, true));
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
    fn function_row_and_punctuation_have_scancodes() {
        assert_eq!(scancode_for(Key::F1), Some(Scancode::plain(0x3B)));
        assert_eq!(scancode_for(Key::F12), Some(Scancode::plain(0x58)));
        assert_eq!(scancode_for(Key::Minus), Some(Scancode::plain(0x0C)));
        assert_eq!(scancode_for(Key::Slash), Some(Scancode::plain(0x35)));
        assert_eq!(scancode_for(Key::A), Some(Scancode::plain(0x1E)));
    }

    #[test]
    fn scancodes_are_unique_across_the_mapped_set() {
        // No two distinct keys collide on the same (code, extended) identity.
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
    fn modkey_scancodes_match_the_constants() {
        assert_eq!(ModKey::Shift.scancode(), SHIFT_SCANCODE);
        assert_eq!(ModKey::Ctrl.scancode(), CTRL_SCANCODE);
        assert_eq!(ModKey::Alt.scancode(), ALT_SCANCODE);
    }

    #[test]
    fn modifier_diff_emits_release_before_press_and_holds_state() {
        // A simple event shape so the generic tracker can be exercised directly.
        #[derive(Debug, PartialEq, Eq)]
        struct Ev(ModKey, bool);
        let mut m = ModifierTracker::default();
        // Shift goes down.
        assert_eq!(
            m.diff(true, false, false, |k, d| Ev(k, d)),
            vec![Ev(ModKey::Shift, true)]
        );
        // No change → nothing emitted.
        assert!(m.diff(true, false, false, |k, d| Ev(k, d)).is_empty());
        // Shift up + Ctrl down in one step: the release is emitted before the press.
        assert_eq!(
            m.diff(false, true, false, |k, d| Ev(k, d)),
            vec![Ev(ModKey::Shift, false), Ev(ModKey::Ctrl, true)]
        );
    }

    #[test]
    fn modifier_diff_handles_alt() {
        #[derive(Debug, PartialEq, Eq)]
        struct Ev(ModKey, bool);
        let mut m = ModifierTracker::default();
        assert_eq!(
            m.diff(false, false, true, |k, d| Ev(k, d)),
            vec![Ev(ModKey::Alt, true)]
        );
    }
}
