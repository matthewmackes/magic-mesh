//! [`VncSession`] — the egui-facing surface of a VNC/RFB desktop.
//!
//! The session owns the persistent [`Framebuffer`], the negotiated
//! [`PixelFormat`], and the input state the shell drives:
//!
//! * [`VncSession::apply_framebuffer_update`] decodes a `FramebufferUpdate` off
//!   the wire into the framebuffer (the decode side; see [`crate::encoding`]).
//! * [`VncSession::frame`] hands the shell the latest desktop as an
//!   [`egui::ColorImage`] (only when something changed) — the shell uploads it to
//!   a `TextureHandle` (lock 21, render egui-native).
//! * [`VncSession::send_input`] resolves an [`egui::Event`] into wire-ready RFB
//!   [`RfbClientMessage`]s (pointer mask / keysym / wheel / text), synthesising
//!   modifier-key transitions from egui's modifier snapshot, and queues them for
//!   the transport.
//!
//! This state machine is fully **unit-tested without a server**: decode is fed
//! through [`VncSession::apply_framebuffer_update`] / [`VncSession::apply_rect`]
//! exactly as the live transport feeds it, and queued input is drained with
//! [`VncSession::take_input`]. The live RFB transport (handshake + the TCP read
//! pump that fills the framebuffer and flushes the queue) is the integration-
//! gated layer — it calls these same methods, so the tested path and the shipped
//! path do not diverge.

use crate::config::{ConfigError, VncConfig};
use crate::egui::{ColorImage, Event};
use crate::encoding::{decode_framebuffer_update, decode_rect, DecodeError, Reader, Rectangle};
use crate::input::{map_event, map_text, ModifierState, VncInputEvent};
use crate::pixel::{Framebuffer, PixelFormat};
use crate::wire::RfbClientMessage;

/// The egui-facing RFB desktop: a framebuffer the shell renders + an input queue
/// the transport drains.
pub struct VncSession {
    config: VncConfig,
    format: PixelFormat,
    framebuffer: Framebuffer,
    /// Set whenever the framebuffer changed since the last [`VncSession::frame`].
    dirty: bool,
    /// Wire-ready input messages awaiting the transport, in arrival order.
    pending: Vec<RfbClientMessage>,
    /// Last absolute pointer position pushed (framebuffer pixels).
    pointer: (u16, u16),
    /// Live pointer button mask pushed to the guest (RFB sends it in full).
    buttons: u8,
    /// Modifier keys already held on the guest (synthesised from egui snapshots).
    modifiers: ModifierState,
}

impl VncSession {
    /// Build a session for `config`, sizing the framebuffer to the configured
    /// initial size and defaulting to the canonical 32-bpp true-colour
    /// [`PixelFormat`]. The framebuffer starts opaque black and is marked dirty so
    /// the first [`VncSession::frame`] yields an image for the shell to upload.
    ///
    /// The live transport calls [`VncSession::resize`] / [`VncSession::set_format`]
    /// once the server's `ServerInit` is read.
    ///
    /// # Errors
    /// [`ConfigError`] if `config` fails [`VncConfig::validate`].
    pub fn new(config: VncConfig) -> Result<Self, ConfigError> {
        config.validate()?;
        let framebuffer = Framebuffer::new(usize::from(config.width), usize::from(config.height));
        Ok(Self {
            config,
            format: PixelFormat::rgba8888(),
            framebuffer,
            dirty: true,
            pending: Vec::new(),
            pointer: (0, 0),
            buttons: 0,
            modifiers: ModifierState::default(),
        })
    }

    /// The configuration this session was built from.
    #[must_use]
    pub const fn config(&self) -> &VncConfig {
        &self.config
    }

    /// The negotiated pixel format.
    #[must_use]
    pub const fn format(&self) -> PixelFormat {
        self.format
    }

    /// Set the negotiated pixel format (the transport, from `ServerInit` /
    /// `SetPixelFormat`).
    pub const fn set_format(&mut self, format: PixelFormat) {
        self.format = format;
    }

    /// The current framebuffer size `(width, height)` in pixels.
    #[must_use]
    pub const fn desktop_size(&self) -> (u16, u16) {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "framebuffer dims are validated into the u16 MIN/MAX range"
        )]
        (
            self.framebuffer.width() as u16,
            self.framebuffer.height() as u16,
        )
    }

    /// The last pointer position pushed to the guest (framebuffer pixels).
    #[must_use]
    pub const fn pointer_position(&self) -> (u16, u16) {
        self.pointer
    }

    /// The live pointer button mask pushed to the guest.
    #[must_use]
    pub const fn button_mask(&self) -> u8 {
        self.buttons
    }

    // ── Decode side (fed by the transport or by tests) ──────────────────────

    /// Decode a `FramebufferUpdate` body into the desktop and mark it dirty if any
    /// rectangle landed. `body` is the message *after* its 1-byte type (0): it
    /// starts at the padding byte (see [`decode_framebuffer_update`]).
    ///
    /// # Errors
    /// [`DecodeError`] for an unsupported format/encoding, truncated bytes, or an
    /// out-of-bounds rectangle.
    pub fn apply_framebuffer_update(&mut self, body: &[u8]) -> Result<u16, DecodeError> {
        let mut reader = Reader::new(body);
        let rects = decode_framebuffer_update(&mut reader, &mut self.framebuffer, self.format)?;
        if rects > 0 {
            self.dirty = true;
        }
        Ok(rects)
    }

    /// Decode a single rectangle's `payload` into the desktop and mark it dirty —
    /// the per-rectangle entry point the transport uses when it reads rectangles
    /// one at a time.
    ///
    /// # Errors
    /// [`DecodeError`] as for [`VncSession::apply_framebuffer_update`].
    pub fn apply_rect(&mut self, rect: &Rectangle, payload: &[u8]) -> Result<(), DecodeError> {
        if !self.format.is_supported() {
            return Err(DecodeError::UnsupportedFormat);
        }
        let mut reader = Reader::new(payload);
        decode_rect(rect, &mut reader, &mut self.framebuffer, self.format)?;
        self.dirty = true;
        Ok(())
    }

    /// Resize the framebuffer (the `DesktopSize` pseudo-encoding / a `ServerInit`
    /// larger than the configured default) and mark it dirty.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.framebuffer
            .resize(usize::from(width), usize::from(height));
        self.dirty = true;
    }

    /// The latest desktop as an [`egui::ColorImage`], or `None` if nothing changed
    /// since the previous call. Clears the dirty flag.
    pub fn frame(&mut self) -> Option<ColorImage> {
        if self.dirty {
            self.dirty = false;
            Some(self.framebuffer.to_color_image())
        } else {
            None
        }
    }

    // ── Input side (driven by the shell) ────────────────────────────────────

    /// Resolve an egui input event into RFB wire messages and queue them.
    /// Modifier transitions are synthesised from the event's modifier snapshot
    /// (egui reports modifiers as state, not as discrete key events) and queued
    /// *before* the event itself, so a Shift+letter chord reaches the guest
    /// correctly. Text commits go through the keysym press/release path.
    pub fn send_input(&mut self, event: &Event) {
        match event {
            Event::Key { modifiers, .. } | Event::PointerButton { modifiers, .. } => {
                let evs = self
                    .modifiers
                    .diff(modifiers.shift, modifiers.ctrl, modifiers.alt);
                for ev in evs {
                    self.apply_intent(ev);
                }
            }
            _ => {}
        }

        if let Event::Text(text) = event {
            for ev in map_text(text) {
                self.apply_intent(ev);
            }
            return;
        }

        if let Some(ev) = map_event(event) {
            self.apply_intent(ev);
        }
    }

    /// Resolve one protocol-neutral intent into queued RFB wire message(s),
    /// updating the tracked pointer position and button mask.
    fn apply_intent(&mut self, ev: VncInputEvent) {
        match ev {
            VncInputEvent::PointerMove { x, y } => {
                self.pointer = (x, y);
                self.pending.push(RfbClientMessage::PointerEvent {
                    button_mask: self.buttons,
                    x,
                    y,
                });
            }
            VncInputEvent::PointerButton { button, down, x, y } => {
                self.pointer = (x, y);
                let bit = button.mask_bit();
                if down {
                    self.buttons |= bit;
                } else {
                    self.buttons &= !bit;
                }
                self.pending.push(RfbClientMessage::PointerEvent {
                    button_mask: self.buttons,
                    x,
                    y,
                });
            }
            VncInputEvent::Wheel { delta, horizontal } => self.apply_wheel(delta, horizontal),
            VncInputEvent::Key { keysym, down } => {
                self.pending
                    .push(RfbClientMessage::KeyEvent { down, keysym });
            }
        }
    }

    /// Expand a wheel rotation into `|delta|` press+release pairs of the matching
    /// RFB wheel button (4 up / 5 down / 6 left / 7 right) at the current pointer.
    fn apply_wheel(&mut self, delta: i16, horizontal: bool) {
        let button = match (horizontal, delta > 0) {
            (false, true) => 4u8, // wheel up
            (false, false) => 5,  // wheel down
            (true, false) => 6,   // wheel left
            (true, true) => 7,    // wheel right
        };
        let bit = 1u8 << (button - 1);
        let (x, y) = self.pointer;
        for _ in 0..delta.unsigned_abs() {
            self.pending.push(RfbClientMessage::PointerEvent {
                button_mask: self.buttons | bit,
                x,
                y,
            });
            self.pending.push(RfbClientMessage::PointerEvent {
                button_mask: self.buttons,
                x,
                y,
            });
        }
    }

    /// Borrow the queued-but-unsent wire messages (inspection / tests).
    #[must_use]
    pub fn pending_input(&self) -> &[RfbClientMessage] {
        &self.pending
    }

    /// Drain the queued wire messages for the transport to send.
    pub fn take_input(&mut self) -> Vec<RfbClientMessage> {
        std::mem::take(&mut self.pending)
    }
}

#[cfg(test)]
mod tests {
    use super::VncSession;
    use crate::config::VncConfig;
    use crate::egui::{Color32, Event, Key, Modifiers, PointerButton, Pos2, Vec2};
    use crate::encoding::Rectangle;
    use crate::input::{ALT_KEYSYM, CTRL_KEYSYM, SHIFT_KEYSYM};
    use crate::wire::RfbClientMessage;

    fn session() -> VncSession {
        VncSession::new(VncConfig::new("host").with_size(16, 16)).expect("valid config")
    }

    // A FramebufferUpdate body (after the 1-byte type): one Raw rect at the origin
    // painting `width` pixels of the first row, each [B,G,R,pad] little-endian.
    fn raw_update(width: u16, pixels: &[[u8; 4]]) -> Vec<u8> {
        let mut body = vec![0x00]; // padding
        body.extend_from_slice(&1u16.to_be_bytes()); // one rect
        body.extend_from_slice(&0u16.to_be_bytes()); // x
        body.extend_from_slice(&0u16.to_be_bytes()); // y
        body.extend_from_slice(&width.to_be_bytes()); // w
        body.extend_from_slice(&1u16.to_be_bytes()); // h
        body.extend_from_slice(&0i32.to_be_bytes()); // Raw
        for p in pixels {
            body.extend_from_slice(p);
        }
        body
    }

    #[test]
    fn new_rejects_invalid_config() {
        assert!(VncSession::new(VncConfig::new("")).is_err());
    }

    #[test]
    fn first_frame_is_the_initial_black_desktop_then_clears() {
        let mut s = session();
        let img = s.frame().expect("first frame is available");
        assert_eq!(img.size, [16, 16]);
        assert_eq!(img.pixels[0], Color32::from_rgb(0, 0, 0));
        assert!(s.frame().is_none(), "no further change");
    }

    #[test]
    fn applied_update_makes_a_new_frame_available() {
        let mut s = session();
        let _ = s.frame(); // consume the initial frame
        let body = raw_update(2, &[[0, 0, 0xFF, 0], [0xFF, 0, 0, 0]]); // red, blue
        let n = s.apply_framebuffer_update(&body).expect("update");
        assert_eq!(n, 1);
        let img = s.frame().expect("frame after update");
        assert_eq!(img.pixels[0], Color32::from_rgb(0xFF, 0, 0));
        assert_eq!(img.pixels[1], Color32::from_rgb(0, 0, 0xFF));
        assert!(s.frame().is_none(), "no further change");
    }

    #[test]
    fn apply_rect_decodes_a_single_rectangle() {
        let mut s = session();
        let _ = s.frame();
        let rect = Rectangle {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            encoding: 0,
        };
        s.apply_rect(&rect, &[0x00, 0xFF, 0x00, 0x00])
            .expect("rect"); // green
        let img = s.frame().expect("frame");
        assert_eq!(img.pixels[0], Color32::from_rgb(0, 0xFF, 0));
    }

    #[test]
    fn resize_changes_desktop_size_and_dirties() {
        let mut s = session();
        let _ = s.frame();
        s.resize(32, 24);
        assert_eq!(s.desktop_size(), (32, 24));
        assert_eq!(s.frame().expect("resized frame").size, [32, 24]);
    }

    #[test]
    fn pointer_move_queues_pointer_event_with_mask() {
        let mut s = session();
        s.send_input(&Event::PointerMoved(Pos2::new(7.0, 9.0)));
        assert_eq!(s.pointer_position(), (7, 9));
        assert_eq!(
            s.take_input(),
            vec![RfbClientMessage::PointerEvent {
                button_mask: 0,
                x: 7,
                y: 9,
            }]
        );
        assert!(s.pending_input().is_empty(), "drained");
    }

    #[test]
    fn button_press_then_release_tracks_the_mask() {
        let mut s = session();
        s.send_input(&Event::PointerButton {
            pos: Pos2::new(3.0, 4.0),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::default(),
        });
        assert_eq!(s.button_mask(), 0x01);
        s.send_input(&Event::PointerButton {
            pos: Pos2::new(3.0, 4.0),
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::default(),
        });
        assert_eq!(s.button_mask(), 0x00);
        assert_eq!(
            s.take_input(),
            vec![
                RfbClientMessage::PointerEvent {
                    button_mask: 0x01,
                    x: 3,
                    y: 4,
                },
                RfbClientMessage::PointerEvent {
                    button_mask: 0x00,
                    x: 3,
                    y: 4,
                },
            ]
        );
    }

    #[test]
    fn shift_letter_chord_synthesises_modifier_then_key() {
        let mut s = session();
        s.send_input(&Event::Key {
            key: Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        });
        assert_eq!(
            s.take_input(),
            vec![
                RfbClientMessage::KeyEvent {
                    down: true,
                    keysym: SHIFT_KEYSYM,
                },
                RfbClientMessage::KeyEvent {
                    down: true,
                    keysym: 0x61, // 'a'
                },
            ]
        );
    }

    #[test]
    fn text_commit_queues_keysym_press_release() {
        let mut s = session();
        s.send_input(&Event::Text("hi".to_string()));
        assert_eq!(
            s.take_input(),
            vec![
                RfbClientMessage::KeyEvent {
                    down: true,
                    keysym: 0x68, // 'h'
                },
                RfbClientMessage::KeyEvent {
                    down: false,
                    keysym: 0x68,
                },
                RfbClientMessage::KeyEvent {
                    down: true,
                    keysym: 0x69, // 'i'
                },
                RfbClientMessage::KeyEvent {
                    down: false,
                    keysym: 0x69,
                },
            ]
        );
    }

    #[test]
    fn ctrl_click_holds_ctrl_around_the_button() {
        let mut s = session();
        s.send_input(&Event::PointerButton {
            pos: Pos2::new(1.0, 0.0),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        });
        let drained = s.take_input();
        assert_eq!(drained.len(), 2, "ctrl-down then the button");
        assert_eq!(
            drained[0],
            RfbClientMessage::KeyEvent {
                down: true,
                keysym: CTRL_KEYSYM,
            }
        );
        assert_eq!(
            drained[1],
            RfbClientMessage::PointerEvent {
                button_mask: 0x01,
                x: 1,
                y: 0,
            }
        );
        assert_eq!(s.pointer_position(), (1, 0));
    }

    #[test]
    fn vertical_wheel_emits_button4_click_pairs() {
        let mut s = session();
        s.send_input(&Event::PointerMoved(Pos2::new(5.0, 5.0)));
        let _ = s.take_input();
        // Two notches up → two press/release pairs of button 4 (mask bit 3 = 0x08).
        s.send_input(&Event::MouseWheel {
            unit: crate::egui::MouseWheelUnit::Line,
            delta: Vec2::new(0.0, 2.0),
            modifiers: Modifiers::default(),
        });
        let drained = s.take_input();
        assert_eq!(drained.len(), 4);
        assert_eq!(
            drained[0],
            RfbClientMessage::PointerEvent {
                button_mask: 0x08,
                x: 5,
                y: 5,
            }
        );
        assert_eq!(
            drained[1],
            RfbClientMessage::PointerEvent {
                button_mask: 0x00,
                x: 5,
                y: 5,
            }
        );
        assert_eq!(drained[2], drained[0]);
        assert_eq!(drained[3], drained[1]);
    }

    #[test]
    fn alt_modifier_released_when_dropped() {
        const F1_KEYSYM: u32 = 0xFFBE;
        let mut s = session();
        // Alt down with a key.
        s.send_input(&Event::Key {
            key: Key::F1,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers {
                alt: true,
                ..Modifiers::default()
            },
        });
        // Key up with Alt released in the same snapshot.
        s.send_input(&Event::Key {
            key: Key::F1,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: Modifiers::default(),
        });
        let drained = s.take_input();
        // The modifier diff is queued before the key event within each send_input
        // (mirroring the RDP backend), so Alt presses before F1 and, when dropped,
        // releases ahead of the F1 release: alt-down, F1-down, alt-up, F1-up.
        assert_eq!(
            drained,
            vec![
                RfbClientMessage::KeyEvent {
                    down: true,
                    keysym: ALT_KEYSYM,
                },
                RfbClientMessage::KeyEvent {
                    down: true,
                    keysym: F1_KEYSYM,
                },
                RfbClientMessage::KeyEvent {
                    down: false,
                    keysym: ALT_KEYSYM,
                },
                RfbClientMessage::KeyEvent {
                    down: false,
                    keysym: F1_KEYSYM,
                },
            ]
        );
    }

    #[test]
    fn unsupported_format_surfaces_on_apply() {
        let mut s = session();
        s.set_format(PixelFormatNonTrueColor::make());
        let body = raw_update(1, &[[0, 0, 0, 0]]);
        assert!(s.apply_framebuffer_update(&body).is_err());
    }

    // A tiny helper producing a palette (non-true-colour) format for the guard.
    struct PixelFormatNonTrueColor;
    impl PixelFormatNonTrueColor {
        fn make() -> crate::pixel::PixelFormat {
            crate::pixel::PixelFormat {
                true_color: false,
                ..crate::pixel::PixelFormat::rgba8888()
            }
        }
    }
}
