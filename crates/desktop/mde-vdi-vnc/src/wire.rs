//! Wire-ready RFB client→server input messages + their byte encoding.
//!
//! The session resolves egui input ([`crate::input`]) into these two messages —
//! the only client messages an interactive VDI session sends after the handshake:
//!
//! * `PointerEvent` (type 5) — the full button mask + absolute position.
//! * `KeyEvent` (type 4) — a key down/up by X11 keysym.
//!
//! Encoding them to bytes is pure and server-free, so it is unit-tested here
//! against the exact RFB byte layout (RFC 6143 §7.5.4 / §7.5.5). *Sending* these
//! over the Nebula TCP link is the live transport — the integration-gated layer,
//! not the unit path: a resolved message is bytes, putting them on a socket is a
//! connection.

/// An RFB client→server input message, resolved by the session from egui input
/// and ready for the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RfbClientMessage {
    /// `PointerEvent` (message type 5): the live state of all pointer buttons as
    /// a mask, plus the absolute pointer position in framebuffer pixels.
    PointerEvent {
        /// Bitmask of currently-pressed buttons (bit 0 = button 1).
        button_mask: u8,
        /// X in framebuffer pixels.
        x: u16,
        /// Y in framebuffer pixels.
        y: u16,
    },
    /// `KeyEvent` (message type 4): a key transition by X11 keysym.
    KeyEvent {
        /// `true` = pressed, `false` = released.
        down: bool,
        /// The X11 keysym.
        keysym: u32,
    },
}

impl RfbClientMessage {
    /// The RFB message-type byte.
    #[must_use]
    pub const fn message_type(&self) -> u8 {
        match self {
            Self::KeyEvent { .. } => 4,
            Self::PointerEvent { .. } => 5,
        }
    }

    /// Append this message's RFB wire bytes to `out`.
    ///
    /// * `KeyEvent`  → `[4, down, pad, pad, keysym(u32 BE)]` (8 bytes).
    /// * `PointerEvent` → `[5, mask, x(u16 BE), y(u16 BE)]` (6 bytes).
    pub fn encode(&self, out: &mut Vec<u8>) {
        match *self {
            Self::KeyEvent { down, keysym } => {
                out.push(4);
                out.push(u8::from(down));
                out.extend_from_slice(&[0, 0]); // padding
                out.extend_from_slice(&keysym.to_be_bytes());
            }
            Self::PointerEvent { button_mask, x, y } => {
                out.push(5);
                out.push(button_mask);
                out.extend_from_slice(&x.to_be_bytes());
                out.extend_from_slice(&y.to_be_bytes());
            }
        }
    }

    /// This message's RFB wire bytes.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode(&mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::RfbClientMessage;

    #[test]
    fn pointer_event_wire_layout() {
        let msg = RfbClientMessage::PointerEvent {
            button_mask: 0x01,
            x: 0x1234,
            y: 0x5678,
        };
        assert_eq!(msg.message_type(), 5);
        assert_eq!(msg.to_bytes(), vec![5, 0x01, 0x12, 0x34, 0x56, 0x78]);
    }

    #[test]
    fn key_event_wire_layout() {
        let down = RfbClientMessage::KeyEvent {
            down: true,
            keysym: 0xFF0D, // Return
        };
        assert_eq!(down.message_type(), 4);
        assert_eq!(down.to_bytes(), vec![4, 1, 0, 0, 0x00, 0x00, 0xFF, 0x0D]);

        let up = RfbClientMessage::KeyEvent {
            down: false,
            keysym: 0x61, // 'a'
        };
        assert_eq!(up.to_bytes(), vec![4, 0, 0, 0, 0x00, 0x00, 0x00, 0x61]);
    }

    #[test]
    fn encode_appends_without_clearing() {
        let mut buf = vec![0xAA];
        RfbClientMessage::PointerEvent {
            button_mask: 0,
            x: 0,
            y: 0,
        }
        .encode(&mut buf);
        assert_eq!(buf, vec![0xAA, 5, 0, 0, 0, 0, 0]);
    }
}
