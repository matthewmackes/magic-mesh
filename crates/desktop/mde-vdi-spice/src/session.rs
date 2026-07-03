//! [`SpiceSession`] — the egui-facing surface of a SPICE desktop.
//!
//! The session owns the persistent [`Framebuffer`] and the input state the shell
//! drives:
//!
//! * [`SpiceSession::apply_surface`] takes a `spice-client` decoded display
//!   surface (the whole primary framebuffer the display channel hands back) and
//!   folds it into the framebuffer (the decode side; see [`crate::pixel`]).
//! * [`SpiceSession::frame`] hands the shell the latest desktop as an
//!   [`egui::ColorImage`] (only when something changed) — the shell uploads it to
//!   a `TextureHandle` (lock 21, render egui-native).
//! * [`SpiceSession::send_input`] resolves an [`egui::Event`] into wire-ready
//!   SPICE [`SpiceInputEvent`]s (pointer move/button/wheel + scancode key),
//!   synthesising modifier-key transitions from egui's modifier snapshot, and
//!   queues them for the transport ([`crate::connect`]).
//!
//! This state machine is fully **unit-tested without a server**: decode is fed
//! through [`SpiceSession::apply_surface`] with a synthetic
//! [`spice_client::DisplaySurface`] exactly as the live transport feeds it, and
//! queued input is drained with [`SpiceSession::take_input`]. The live SPICE
//! transport (the async connect + the display/inputs channel pump) is the
//! integration-gated layer ([`crate::connect`]) — it calls these same methods, so
//! the tested path and the shipped path do not diverge.

use spice_client::DisplaySurface;

use crate::config::{ConfigError, SpiceConfig};
use crate::egui::{ColorImage, Event};
use crate::input::{map_event, ModifierState, SpiceInputEvent};
use crate::pixel::{Framebuffer, FramebufferError, SurfaceFormat};

/// The egui-facing SPICE desktop: a framebuffer the shell renders + an input
/// queue the transport drains.
pub struct SpiceSession {
    config: SpiceConfig,
    framebuffer: Framebuffer,
    /// Set whenever the framebuffer changed since the last [`SpiceSession::frame`].
    dirty: bool,
    /// Wire-ready input intents awaiting the transport, in arrival order.
    pending: Vec<SpiceInputEvent>,
    /// Last absolute pointer position pushed (framebuffer pixels).
    pointer: (u16, u16),
    /// Modifier keys already held on the guest (synthesised from egui snapshots).
    modifiers: ModifierState,
}

impl SpiceSession {
    /// Build a session for `config`, sizing the framebuffer to the configured
    /// initial size. The framebuffer starts opaque black and is marked dirty so
    /// the first [`SpiceSession::frame`] yields an image for the shell to upload.
    ///
    /// The live transport calls [`SpiceSession::apply_surface`] once the display
    /// channel decodes the guest's first primary surface, which resizes the
    /// framebuffer to the real desktop.
    ///
    /// # Errors
    /// [`ConfigError`] if `config` fails [`SpiceConfig::validate`].
    pub fn new(config: SpiceConfig) -> Result<Self, ConfigError> {
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
    pub const fn config(&self) -> &SpiceConfig {
        &self.config
    }

    /// The current framebuffer size `(width, height)` in pixels.
    #[must_use]
    pub const fn desktop_size(&self) -> (u16, u16) {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "framebuffer dims are validated/derived within the u16 MIN/MAX range"
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

    // ── Decode side (fed by the transport or by tests) ──────────────────────

    /// Fold a `spice-client` decoded display surface into the desktop and mark it
    /// dirty. This is the exact entry point the live display-channel pump feeds
    /// ([`crate::connect`]); a test feeds a synthetic [`DisplaySurface`] (its
    /// fields are public) so the connect→frame seam is proven without a server.
    ///
    /// # Errors
    /// [`FramebufferError`] for a zero-dimension or truncated surface — a
    /// malformed surface degrades rather than panicking.
    pub fn apply_surface(&mut self, surface: &DisplaySurface) -> Result<(), FramebufferError> {
        let w = surface.width as usize;
        let h = surface.height as usize;
        let format = SurfaceFormat::from_tag(surface.format);
        self.framebuffer
            .apply_surface(w, h, format, &surface.data)?;
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

    /// Resolve an egui input event into SPICE input intents and queue them.
    /// Modifier transitions are synthesised from the event's modifier snapshot
    /// (egui reports modifiers as state, not as discrete key events) and queued
    /// *before* the event itself, so a Shift+letter chord reaches the guest
    /// correctly.
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

        if let Some(ev) = map_event(event) {
            self.apply_intent(ev);
        }
    }

    /// Queue one intent, updating the tracked pointer position.
    fn apply_intent(&mut self, ev: SpiceInputEvent) {
        match ev {
            SpiceInputEvent::PointerMove { x, y } | SpiceInputEvent::PointerButton { x, y, .. } => {
                self.pointer = (x, y);
            }
            SpiceInputEvent::Wheel { .. } | SpiceInputEvent::Key { .. } => {}
        }
        self.pending.push(ev);
    }

    /// Borrow the queued-but-unsent input intents (inspection / tests).
    #[must_use]
    pub fn pending_input(&self) -> &[SpiceInputEvent] {
        &self.pending
    }

    /// Drain the queued input intents for the transport to send.
    pub fn take_input(&mut self) -> Vec<SpiceInputEvent> {
        std::mem::take(&mut self.pending)
    }
}

#[cfg(test)]
mod tests {
    use super::SpiceSession;
    use crate::config::SpiceConfig;
    use crate::egui::{Color32, Event, Key, Modifiers, PointerButton, Pos2};
    use crate::input::{Scancode, SpiceInputEvent};
    use spice_client::DisplaySurface;

    fn session() -> SpiceSession {
        SpiceSession::new(SpiceConfig::new("host").with_size(16, 16)).expect("valid config")
    }

    /// A synthetic decoded primary surface: `w × h` RGBA pixels, the first pixel
    /// set to `first` and the rest opaque black — exactly the shape
    /// `spice-client`'s display channel hands back (`format = 32`, RGBA data).
    fn surface(w: u32, h: u32, first: [u8; 4]) -> DisplaySurface {
        let mut data = vec![0u8; (w * h * 4) as usize];
        for a in data.iter_mut().skip(3).step_by(4) {
            *a = 0xFF; // opaque
        }
        data[..4].copy_from_slice(&first);
        DisplaySurface {
            width: w,
            height: h,
            format: 32,
            data,
        }
    }

    #[test]
    fn new_rejects_invalid_config() {
        assert!(SpiceSession::new(SpiceConfig::new("")).is_err());
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
    fn applied_surface_makes_a_new_frame_available() {
        // The core connect→frame seam: a decoded surface arrives → a frame is
        // available for the shell to upload. This is the exact method the live
        // display-channel pump feeds.
        let mut s = session();
        let _ = s.frame(); // consume the initial frame
        s.apply_surface(&surface(2, 1, [0xFF, 0x00, 0x00, 0xFF]))
            .expect("surface");
        let img = s.frame().expect("frame after surface");
        assert_eq!(img.size, [2, 1]);
        assert_eq!(img.pixels[0], Color32::from_rgb(0xFF, 0, 0), "red pixel");
        assert_eq!(img.pixels[1], Color32::from_rgb(0, 0, 0), "black pixel");
        assert!(s.frame().is_none(), "no further change");
    }

    #[test]
    fn a_surface_resizes_the_desktop() {
        let mut s = session();
        let _ = s.frame();
        s.apply_surface(&surface(32, 24, [0, 0, 0, 0xFF]))
            .expect("surface");
        assert_eq!(s.desktop_size(), (32, 24));
        assert_eq!(s.frame().expect("resized frame").size, [32, 24]);
    }

    #[test]
    fn a_truncated_surface_is_rejected_not_panicked() {
        let mut s = session();
        let bad = DisplaySurface {
            width: 4,
            height: 4,
            format: 32,
            data: vec![0u8; 8], // far short of 4*4*4
        };
        assert!(s.apply_surface(&bad).is_err());
    }

    #[test]
    fn pointer_move_queues_the_intent_and_tracks_position() {
        let mut s = session();
        s.send_input(&Event::PointerMoved(Pos2::new(7.0, 9.0)));
        assert_eq!(s.pointer_position(), (7, 9));
        assert_eq!(
            s.take_input(),
            vec![SpiceInputEvent::PointerMove { x: 7, y: 9 }]
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
                SpiceInputEvent::Key {
                    scancode: Scancode {
                        code: 0x2A, // left shift
                        extended: false,
                    },
                    down: true,
                },
                SpiceInputEvent::Key {
                    scancode: Scancode {
                        code: 0x1E, // 'a'
                        extended: false,
                    },
                    down: true,
                },
            ]
        );
    }

    #[test]
    fn button_press_tracks_pointer_and_queues_button() {
        let mut s = session();
        s.send_input(&Event::PointerButton {
            pos: Pos2::new(3.0, 4.0),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::default(),
        });
        assert_eq!(s.pointer_position(), (3, 4));
        assert_eq!(
            s.take_input(),
            vec![SpiceInputEvent::PointerButton {
                button: crate::input::MouseButton::Left,
                down: true,
                x: 3,
                y: 4,
            }]
        );
    }
}
