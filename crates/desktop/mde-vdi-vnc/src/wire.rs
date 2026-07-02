//! Wire-ready RFB client→server messages + their byte encoding.
//!
//! The session resolves egui input ([`crate::input`]) into the two
//! [`RfbClientMessage`] input messages an interactive VDI session sends on
//! every event:
//!
//! * `PointerEvent` (type 5) — the full button mask + absolute position.
//! * `KeyEvent` (type 4) — a key down/up by X11 keysym.
//!
//! The adaptive-codec ladder ([`crate::link`] / [`crate::tier`]) additionally
//! sends the rare [`RfbControlMessage`] session-control messages when the
//! quality tier changes:
//!
//! * `SetPixelFormat` (type 0) — re-negotiate the wire pixel layout.
//! * `SetEncodings` (type 2) — restate the encoding preference.
//!
//! Encoding them to bytes is pure and server-free, so it is unit-tested here
//! against the exact RFB byte layout (RFC 6143 §7.5.1 / §7.5.2 / §7.5.4 /
//! §7.5.5). *Sending* these over the Nebula TCP link is the live transport —
//! the integration-gated layer, not the unit path: a resolved message is
//! bytes, putting them on a socket is a connection.

use crate::encoding::Encoding;
use crate::pixel::PixelFormat;

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

/// An RFB client→server session-control message — the E12-10 adaptive-codec
/// surface: re-negotiate the wire pixel layout / encoding preference
/// mid-session.
///
/// Kept separate from [`RfbClientMessage`] (the per-event input queue: hot,
/// fixed-size, `Copy`) because control messages are rare, variable-size, and
/// drained by the transport at a safe point between update cycles — after a
/// `SetPixelFormat` goes on the wire, every *subsequent* `FramebufferUpdate`
/// uses the new layout, so the sender flips the decode format at send time
/// (see `VncSession::take_control`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RfbControlMessage {
    /// `SetPixelFormat` (message type 0): all subsequent framebuffer updates
    /// use this pixel layout (RFC 6143 §7.5.1).
    SetPixelFormat(PixelFormat),
    /// `SetEncodings` (message type 2): the client's encoding preference, most
    /// preferred first (RFC 6143 §7.5.2).
    SetEncodings(Vec<Encoding>),
}

impl RfbControlMessage {
    /// The RFB message-type byte.
    #[must_use]
    pub const fn message_type(&self) -> u8 {
        match self {
            Self::SetPixelFormat(_) => 0,
            Self::SetEncodings(_) => 2,
        }
    }

    /// Append this message's RFB wire bytes to `out`.
    ///
    /// * `SetPixelFormat` → `[0, pad×3, PIXEL_FORMAT (16 bytes)]` — the
    ///   `PIXEL_FORMAT` layout mirrors [`crate::encoding::parse_pixel_format`].
    /// * `SetEncodings` → `[2, pad, count (u16 BE), count × encoding (i32 BE)]`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::SetPixelFormat(pf) => {
                out.push(0);
                out.extend_from_slice(&[0, 0, 0]); // padding
                out.push(pf.bits_per_pixel);
                out.push(pf.depth);
                out.push(u8::from(pf.big_endian));
                out.push(u8::from(pf.true_color));
                out.extend_from_slice(&pf.red_max.to_be_bytes());
                out.extend_from_slice(&pf.green_max.to_be_bytes());
                out.extend_from_slice(&pf.blue_max.to_be_bytes());
                out.push(pf.red_shift);
                out.push(pf.green_shift);
                out.push(pf.blue_shift);
                out.extend_from_slice(&[0, 0, 0]); // padding
            }
            Self::SetEncodings(encodings) => {
                out.push(2);
                out.push(0); // padding
                let count = u16::try_from(encodings.len()).unwrap_or(u16::MAX);
                out.extend_from_slice(&count.to_be_bytes());
                for enc in encodings.iter().take(usize::from(count)) {
                    out.extend_from_slice(&enc.code().to_be_bytes());
                }
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
    use super::{RfbClientMessage, RfbControlMessage};
    use crate::encoding::{parse_pixel_format, Encoding, Reader};
    use crate::pixel::PixelFormat;

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

    #[test]
    fn set_pixel_format_wire_layout_round_trips() {
        let msg = RfbControlMessage::SetPixelFormat(PixelFormat::rgb565());
        assert_eq!(msg.message_type(), 0);
        let bytes = msg.to_bytes();
        assert_eq!(bytes.len(), 20, "type + 3 pad + 16-byte PIXEL_FORMAT");
        assert_eq!(&bytes[..4], &[0, 0, 0, 0]);
        // The 16-byte body must parse back through the server-side reader.
        let mut reader = Reader::new(&bytes[4..]);
        let parsed = parse_pixel_format(&mut reader).expect("round trip");
        assert_eq!(parsed, PixelFormat::rgb565());
    }

    #[test]
    fn set_encodings_wire_layout() {
        let msg = RfbControlMessage::SetEncodings(vec![
            Encoding::CopyRect,
            Encoding::Hextile,
            Encoding::Rre,
            Encoding::Raw,
        ]);
        assert_eq!(msg.message_type(), 2);
        assert_eq!(
            msg.to_bytes(),
            vec![
                2, 0, // type, padding
                0, 4, // count (u16 BE) = 4 encodings
                0, 0, 0, 1, // CopyRect
                0, 0, 0, 5, // Hextile
                0, 0, 0, 2, // RRE
                0, 0, 0, 0, // Raw
            ]
        );
    }
}
