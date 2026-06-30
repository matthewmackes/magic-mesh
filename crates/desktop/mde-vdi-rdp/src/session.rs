//! [`RdpSession`] — the egui-facing surface of an RDP desktop.
//!
//! The session owns the persistent desktop [`Framebuffer`] and the input state
//! the shell drives:
//!
//! * [`RdpSession::frame`] hands the shell the latest desktop as an
//!   [`egui::ColorImage`] (only when something changed) — the shell uploads it to
//!   a `TextureHandle` (lock 21, render egui-native).
//! * [`RdpSession::send_input`] maps an [`egui::Event`] to RDP input intents
//!   (pointer / key / wheel / text), synthesising modifier key transitions from
//!   egui's modifier snapshot, and queues them for the wire pump.
//!
//! This state machine is **ironrdp-free and fully unit-tested without a server**:
//! decode is fed through [`RdpSession::apply_rect`] / [`RdpSession::apply_full_frame`]
//! exactly as the live wire pump feeds it, and queued input is drained with
//! [`RdpSession::take_input`]. The live connection sequence that fills the
//! framebuffer from a real peer and flushes the queue onto the wire is layered on
//! in [`crate::wire`] (gated, see that module) — it calls these same methods, so
//! the tested path and the shipped path do not diverge.

use crate::config::{ConfigError, RdpConfig};
use crate::egui::{ColorImage, Event};
use crate::input::{map_event, map_text, ModifierState, RdpInputEvent};
use crate::pixel::{Framebuffer, FramebufferError, PixelFormat};

/// The egui-facing RDP desktop: a framebuffer the shell renders + an input queue
/// the wire pump drains.
pub struct RdpSession {
    config: RdpConfig,
    framebuffer: Framebuffer,
    /// Set whenever the framebuffer changed since the last [`RdpSession::frame`].
    dirty: bool,
    /// Input intents awaiting the wire pump, in arrival order.
    pending: Vec<RdpInputEvent>,
    /// Last absolute pointer position pushed (desktop pixels).
    pointer: (u16, u16),
    /// Modifier keys already held on the guest (synthesised from egui snapshots).
    modifiers: ModifierState,
}

impl RdpSession {
    /// Build a session for `config`, sizing the framebuffer to the negotiated
    /// desktop. The framebuffer starts as opaque black and is marked dirty so the
    /// first [`RdpSession::frame`] yields an image for the shell to upload.
    ///
    /// # Errors
    /// [`ConfigError`] if `config` fails [`RdpConfig::validate`].
    pub fn new(config: RdpConfig) -> Result<Self, ConfigError> {
        config.validate()?;
        let framebuffer = Framebuffer::new(usize::from(config.width), usize::from(config.height));
        Ok(Self {
            config,
            framebuffer,
            dirty: true,
            pending: Vec::new(),
            pointer: (0, 0),
            modifiers: ModifierState::default(),
        })
    }

    /// The configuration this session was built from.
    #[must_use]
    pub const fn config(&self) -> &RdpConfig {
        &self.config
    }

    /// The negotiated desktop size `(width, height)` in pixels.
    #[must_use]
    pub const fn desktop_size(&self) -> (u16, u16) {
        (self.config.width, self.config.height)
    }

    /// The last pointer position pushed to the guest (desktop pixels).
    #[must_use]
    pub const fn pointer_position(&self) -> (u16, u16) {
        self.pointer
    }

    // ── Decode side (fed by the wire pump or by tests) ──────────────────────

    /// Blit a decoded rectangle into the desktop and mark it dirty.
    ///
    /// # Errors
    /// [`FramebufferError`] from [`Framebuffer::apply_rect`] for a malformed
    /// update (out of bounds / truncated / narrow stride).
    pub fn apply_rect(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        format: PixelFormat,
        src: &[u8],
        src_stride: usize,
    ) -> Result<(), FramebufferError> {
        self.framebuffer
            .apply_rect(x, y, w, h, format, src, src_stride)?;
        self.dirty = true;
        Ok(())
    }

    /// Replace the whole desktop from a full-frame buffer and mark it dirty.
    ///
    /// # Errors
    /// [`FramebufferError::ShortSource`] if `src` is smaller than the desktop.
    pub fn apply_full_frame(
        &mut self,
        format: PixelFormat,
        src: &[u8],
    ) -> Result<(), FramebufferError> {
        self.framebuffer.apply_full(format, src)?;
        self.dirty = true;
        Ok(())
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

    /// Map an egui input event to RDP input intents and queue them. Modifier key
    /// transitions are synthesised from the event's modifier snapshot (egui
    /// reports modifiers as state, not as discrete key events) and queued *before*
    /// the event itself, so a Shift+letter chord reaches the guest correctly.
    pub fn send_input(&mut self, event: &Event) {
        match event {
            Event::Key { modifiers, .. } | Event::PointerButton { modifiers, .. } => {
                let evs = self
                    .modifiers
                    .diff(modifiers.shift, modifiers.ctrl, modifiers.alt);
                self.pending.extend(evs);
            }
            _ => {}
        }

        if let Event::Text(text) = event {
            self.pending.extend(map_text(text));
            return;
        }

        if let Some(ev) = map_event(event) {
            self.track_pointer(ev);
            self.pending.push(ev);
        }
    }

    /// Keep the cached pointer position in step with the queued intent.
    fn track_pointer(&mut self, ev: RdpInputEvent) {
        match ev {
            RdpInputEvent::PointerMove { x, y } | RdpInputEvent::PointerButton { x, y, .. } => {
                self.pointer = (x, y)
            }
            _ => {}
        }
    }

    /// Borrow the queued-but-unsent input intents (inspection / tests).
    #[must_use]
    pub fn pending_input(&self) -> &[RdpInputEvent] {
        &self.pending
    }

    /// Drain the queued input intents for the wire pump to encode + send.
    pub fn take_input(&mut self) -> Vec<RdpInputEvent> {
        std::mem::take(&mut self.pending)
    }
}

#[cfg(test)]
mod tests {
    use super::RdpSession;
    use crate::config::RdpConfig;
    use crate::egui::{Color32, Event, Key, Modifiers, Pos2};
    use crate::input::{RdpInputEvent, Scancode};
    use crate::pixel::PixelFormat;

    // The smallest RDP-legal desktop (validate() enforces a 200px minimum); tests
    // paint a tiny rect at the origin and assert on the first row's pixels.
    fn session() -> RdpSession {
        RdpSession::new(RdpConfig::new("host", "u", "p").with_resolution(200, 200))
            .expect("valid config")
    }

    #[test]
    fn new_rejects_invalid_config() {
        assert!(RdpSession::new(RdpConfig::new("", "u", "p")).is_err());
    }

    #[test]
    fn first_frame_is_the_initial_black_desktop_then_clears() {
        let mut s = session();
        let img = s.frame().expect("first frame is available");
        assert_eq!(img.size, [200, 200]);
        assert_eq!(img.pixels[0], Color32::from_rgb(0, 0, 0));
        // Second call with no change → None.
        assert!(s.frame().is_none());
    }

    #[test]
    fn applied_update_makes_a_new_frame_available() {
        let mut s = session();
        let _ = s.frame(); // consume the initial frame
                           // Paint a 2x1 rect at the origin of the 200x200 desktop.
        let src = [
            0x00, 0x00, 0xFF, 0xFF, // BGRA red
            0xFF, 0x00, 0x00, 0xFF, // BGRA blue
        ];
        s.apply_rect(0, 0, 2, 1, PixelFormat::Bgra, &src, 8)
            .expect("apply");
        let img = s.frame().expect("frame after update");
        assert_eq!(img.pixels[0], Color32::from_rgb(0xFF, 0, 0));
        assert_eq!(img.pixels[1], Color32::from_rgb(0, 0, 0xFF));
        assert!(s.frame().is_none(), "no further change");
    }

    #[test]
    fn pointer_move_queues_and_tracks_position() {
        let mut s = session();
        s.send_input(&Event::PointerMoved(Pos2::new(7.0, 9.0)));
        assert_eq!(s.pointer_position(), (7, 9));
        assert_eq!(
            s.take_input(),
            vec![RdpInputEvent::PointerMove { x: 7, y: 9 }]
        );
        assert!(s.pending_input().is_empty(), "drained");
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
                RdpInputEvent::Key {
                    scancode: Scancode {
                        code: 0x2A, // left shift
                        extended: false,
                    },
                    down: true,
                },
                RdpInputEvent::Key {
                    scancode: Scancode {
                        code: 0x1E, // A
                        extended: false,
                    },
                    down: true,
                },
            ]
        );
    }

    #[test]
    fn text_event_queues_unicode_per_char() {
        let mut s = session();
        s.send_input(&Event::Text("hi".to_string()));
        assert_eq!(
            s.take_input(),
            vec![RdpInputEvent::Unicode('h'), RdpInputEvent::Unicode('i'),]
        );
    }

    #[test]
    fn ctrl_click_holds_ctrl_around_the_button() {
        let mut s = session();
        s.send_input(&Event::PointerButton {
            pos: Pos2::new(1.0, 0.0),
            button: crate::egui::PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        });
        let drained = s.take_input();
        assert_eq!(drained.len(), 2, "ctrl-down then the button");
        assert!(matches!(
            drained[0],
            RdpInputEvent::Key {
                scancode: Scancode { code: 0x1D, .. }, // left ctrl
                down: true,
            }
        ));
        assert!(matches!(
            drained[1],
            RdpInputEvent::PointerButton {
                button: crate::input::MouseButton::Left,
                down: true,
                ..
            }
        ));
        assert_eq!(s.pointer_position(), (1, 0));
    }
}
