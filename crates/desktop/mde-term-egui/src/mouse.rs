//! SGR (1006) mouse reporting (TERM-13) — encode egui pointer activity into the
//! xterm SGR mouse-report byte sequences a TUI in mouse-mode reads.
//!
//! This is pure GLUE (§6): the VT engine ([`alacritty_terminal`], via
//! [`crate::engine::Terminal`]) already tracks which mouse modes the running app
//! enabled (DECSET 1000/1002/1003 + the 1006 SGR encoding); this module only
//! turns the egui pointer events the widget already sees into the exact SGR
//! report the app expects, so vim/htop/tmux get the mouse. The widget
//! ([`crate::widget`]) owns the policy — the Shift-bypass that always falls
//! through to native selection, and gating reporting on the app's mode.
//!
//! The SGR (1006) report is `ESC [ < Cb ; Cx ; Cy M` for a press / motion and
//! `ESC [ < Cb ; Cx ; Cy m` (lowercase final byte) for a release, with `Cx`/`Cy`
//! the **1-based** column/row and `Cb` the button code plus the motion / modifier
//! bits. Unlike the legacy X10 scheme it has no 223-cell coordinate ceiling, so a
//! click in the far corner of a large grid reports faithfully.

use mde_egui::egui::{Modifiers, PointerButton};

/// A reportable mouse button — the three xterm tracks. Extra (browser
/// back/forward) buttons have no SGR code and are dropped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseButton {
    /// Primary / left button — SGR base code `0`.
    Left,
    /// Middle button — SGR base code `1`.
    Middle,
    /// Secondary / right button — SGR base code `2`.
    Right,
}

impl MouseButton {
    /// The SGR base button code, before the motion (`+32`) and modifier bits.
    const fn base(self) -> u8 {
        match self {
            Self::Left => 0,
            Self::Middle => 1,
            Self::Right => 2,
        }
    }

    /// Map an egui pointer button to a reportable one; `None` for the extra
    /// buttons the xterm protocol can't encode.
    #[must_use]
    pub const fn from_egui(button: PointerButton) -> Option<Self> {
        match button {
            PointerButton::Primary => Some(Self::Left),
            PointerButton::Middle => Some(Self::Middle),
            PointerButton::Secondary => Some(Self::Right),
            _ => None,
        }
    }
}

/// One reportable mouse event.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseEvent {
    /// A button went down.
    Press(MouseButton),
    /// A button came up (SGR encodes the released button, `m` final byte).
    Release(MouseButton),
    /// Pointer motion with `button` held — DECSET 1002 drag reporting.
    Drag(MouseButton),
    /// Pointer motion with no button held — DECSET 1003 any-motion (hover).
    Motion,
    /// Wheel up — the SGR pseudo-button `64`.
    ScrollUp,
    /// Wheel down — the SGR pseudo-button `65`.
    ScrollDown,
}

/// Encode one `event` at 0-based grid `(col, row)` with `mods` as an SGR (1006)
/// mouse report. Coordinates are emitted **1-based** per the xterm protocol.
///
/// The modifier bits (`shift 4`, `alt/meta 8`, `ctrl 16`) are folded onto the
/// button code exactly as xterm does; in practice the widget's Shift-bypass
/// means a Shift-held pointer never reaches here (it does native selection
/// instead), so `shift` is only ever set on a synthesized report.
#[must_use]
pub fn encode_sgr(event: MouseEvent, col: usize, row: usize, mods: Modifiers) -> Vec<u8> {
    let (mut cb, final_byte) = match event {
        MouseEvent::Press(button) => (button.base(), b'M'),
        MouseEvent::Release(button) => (button.base(), b'm'),
        MouseEvent::Drag(button) => (button.base() + 32, b'M'),
        // Buttonless motion reports as button 3 (none) with the motion bit.
        MouseEvent::Motion => (3 + 32, b'M'),
        MouseEvent::ScrollUp => (64, b'M'),
        MouseEvent::ScrollDown => (65, b'M'),
    };
    if mods.shift {
        cb += 4;
    }
    if mods.alt {
        cb += 8;
    }
    if mods.ctrl {
        cb += 16;
    }
    let cx = col + 1;
    let cy = row + 1;
    format!("\x1b[<{cb};{cx};{cy}{}", final_byte as char).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::{encode_sgr, MouseButton, MouseEvent};
    use mde_egui::egui::{Modifiers, PointerButton};

    /// Encode with no modifiers and read the bytes back as a string.
    fn enc(event: MouseEvent, col: usize, row: usize) -> String {
        String::from_utf8(encode_sgr(event, col, row, Modifiers::NONE)).expect("ascii")
    }

    #[test]
    fn left_press_and_release_flip_only_the_final_byte() {
        // 0-based (col 2, row 4) → 1-based (3, 5); left base 0.
        assert_eq!(
            enc(MouseEvent::Press(MouseButton::Left), 2, 4),
            "\x1b[<0;3;5M"
        );
        assert_eq!(
            enc(MouseEvent::Release(MouseButton::Left), 2, 4),
            "\x1b[<0;3;5m"
        );
    }

    #[test]
    fn middle_and_right_press_carry_their_button_bases() {
        assert_eq!(
            enc(MouseEvent::Press(MouseButton::Middle), 0, 0),
            "\x1b[<1;1;1M"
        );
        assert_eq!(
            enc(MouseEvent::Press(MouseButton::Right), 0, 0),
            "\x1b[<2;1;1M"
        );
    }

    #[test]
    fn drag_sets_the_motion_bit_over_the_held_button() {
        // Left drag: base 0 + 32 = 32; right drag: base 2 + 32 = 34.
        assert_eq!(
            enc(MouseEvent::Drag(MouseButton::Left), 0, 0),
            "\x1b[<32;1;1M"
        );
        assert_eq!(
            enc(MouseEvent::Drag(MouseButton::Right), 9, 0),
            "\x1b[<34;10;1M"
        );
    }

    #[test]
    fn buttonless_hover_motion_is_code_35() {
        // Any-motion (1003) with no button: 3 + 32 = 35.
        assert_eq!(enc(MouseEvent::Motion, 4, 2), "\x1b[<35;5;3M");
    }

    #[test]
    fn wheel_up_and_down_are_pseudo_buttons_64_and_65() {
        assert_eq!(enc(MouseEvent::ScrollUp, 0, 0), "\x1b[<64;1;1M");
        assert_eq!(enc(MouseEvent::ScrollDown, 0, 0), "\x1b[<65;1;1M");
    }

    #[test]
    fn ctrl_and_alt_fold_their_modifier_bits_onto_the_button() {
        // ctrl 16 + alt 8 = 24 added to left press base 0.
        let mods = Modifiers {
            ctrl: true,
            alt: true,
            ..Modifiers::NONE
        };
        let s = String::from_utf8(encode_sgr(MouseEvent::Press(MouseButton::Left), 0, 0, mods))
            .expect("ascii");
        assert_eq!(s, "\x1b[<24;1;1M");
    }

    #[test]
    fn large_coordinates_have_no_223_cell_ceiling() {
        // The legacy X10 scheme caps at 223; SGR is plain decimal.
        assert_eq!(
            enc(MouseEvent::Press(MouseButton::Left), 511, 299),
            "\x1b[<0;512;300M"
        );
    }

    #[test]
    fn from_egui_maps_the_three_tracked_buttons_and_drops_extras() {
        assert_eq!(
            MouseButton::from_egui(PointerButton::Primary),
            Some(MouseButton::Left)
        );
        assert_eq!(
            MouseButton::from_egui(PointerButton::Middle),
            Some(MouseButton::Middle)
        );
        assert_eq!(
            MouseButton::from_egui(PointerButton::Secondary),
            Some(MouseButton::Right)
        );
        assert_eq!(MouseButton::from_egui(PointerButton::Extra1), None);
        assert_eq!(MouseButton::from_egui(PointerButton::Extra2), None);
    }
}
