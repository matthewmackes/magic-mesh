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
//! * The **adaptive-codec surface (E12-10)**: the transport feeds link probes
//!   ([`VncSession::record_rtt`] / [`VncSession::record_stall`] /
//!   [`VncSession::record_frame`]), [`VncSession::autotune`] steps the
//!   [`QualityTier`] on a weak link (manual pin via
//!   [`VncSession::set_quality_mode`]), and every tier change applies **live**:
//!   it queues a `SetPixelFormat` + `SetEncodings` announcement the transport
//!   drains with [`VncSession::take_control`], plus the update-request pacing
//!   in [`VncSession::update_interval_ms`] (see [`crate::tier`]).
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
use crate::link::{
    LadderConfig, LinkEstimate, LinkEstimator, LinkThresholds, QualityLadder, QualityMode,
    QualityTier, TierChange,
};
use crate::pixel::{Framebuffer, PixelFormat};
use crate::tier::VncTierSettings;
use crate::wire::{RfbClientMessage, RfbControlMessage};

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
    /// Rolling link-quality estimates, fed by the transport's probe seam
    /// (E12-10 adaptive codec).
    link: LinkEstimator,
    /// The auto-quality ladder driving the tier from the link grades.
    ladder: QualityLadder,
    /// Auto adaptation vs an operator-pinned tier.
    quality_mode: QualityMode,
    /// Grade cut-offs for [`VncSession::autotune`].
    thresholds: LinkThresholds,
    /// Session-control messages (tier announcements) awaiting the transport.
    pending_control: Vec<RfbControlMessage>,
    /// Decode format to adopt when the queued `SetPixelFormat` is sent — the
    /// server answers everything *after* that message in the new layout, so
    /// the decoder flips at send time ([`VncSession::take_control`]).
    pending_format: Option<PixelFormat>,
    /// Minimum `FramebufferUpdateRequest` spacing of the effective tier.
    update_interval_ms: u64,
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
        let mut session = Self {
            config,
            format: PixelFormat::rgba8888(),
            framebuffer,
            dirty: true,
            pending: Vec::new(),
            pointer: (0, 0),
            buttons: 0,
            modifiers: ModifierState::default(),
            link: LinkEstimator::new(),
            ladder: QualityLadder::new(QualityTier::Full, LadderConfig::default()),
            quality_mode: QualityMode::Auto,
            thresholds: LinkThresholds::default(),
            pending_control: Vec::new(),
            pending_format: None,
            update_interval_ms: 0,
        };
        // Announce the initial (Full) tier: the transport drains this right
        // after the handshake, which is the standard RFB client opening
        // (SetPixelFormat + SetEncodings before the first update request).
        session.apply_tier(QualityTier::Full);
        Ok(session)
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

    // ── Adaptive quality (E12-10) ───────────────────────────────────────────
    //
    // RFB is client-steered at runtime (see `crate::tier`): a tier change
    // queues a complete `SetPixelFormat` + `SetEncodings` announcement for the
    // transport and adjusts the update-request pacing, all mid-session
    // (`VncTierSettings::APPLICATION` is `Live`).

    /// The auto/pinned quality mode.
    #[must_use]
    pub const fn quality_mode(&self) -> QualityMode {
        self.quality_mode
    }

    /// The effective tier: the pinned tier, or the auto ladder's.
    #[must_use]
    pub const fn quality_tier(&self) -> QualityTier {
        match self.quality_mode {
            QualityMode::Pinned(tier) => tier,
            QualityMode::Auto => self.ladder.tier(),
        }
    }

    /// The RFB settings of the effective tier.
    #[must_use]
    pub const fn tier_settings(&self) -> VncTierSettings {
        VncTierSettings::for_tier(self.quality_tier())
    }

    /// Minimum `FramebufferUpdateRequest` spacing the transport must honour —
    /// the effective tier's pacing (the RFB-native rate control).
    #[must_use]
    pub const fn update_interval_ms(&self) -> u64 {
        self.update_interval_ms
    }

    /// Pin a tier or return to auto, reporting the tier change if any.
    ///
    /// VNC applies tiers **live** ([`VncTierSettings::APPLICATION`]): a change
    /// immediately queues the wire announcement for the transport (see
    /// [`VncSession::take_control`]). Returning to auto resumes the ladder
    /// from the pinned tier (hysteresis streaks cleared) instead of replaying
    /// stale pre-pin state.
    pub fn set_quality_mode(&mut self, mode: QualityMode, now_ms: u64) -> Option<TierChange> {
        let from = self.quality_tier();
        if matches!(
            (self.quality_mode, mode),
            (QualityMode::Pinned(_), QualityMode::Auto)
        ) {
            self.ladder.reset_to(from);
        }
        self.quality_mode = mode;
        let to = self.quality_tier();
        if to == from {
            return None;
        }
        self.apply_tier(to);
        Some(TierChange {
            from,
            to,
            at_ms: now_ms,
        })
    }

    /// Queue the complete wire announcement of `tier` and adopt its pacing.
    /// The decode format flips when the transport drains the queue (send
    /// time), because updates still in flight use the old layout.
    fn apply_tier(&mut self, tier: QualityTier) {
        let settings = VncTierSettings::for_tier(tier);
        self.pending_control
            .push(RfbControlMessage::SetPixelFormat(settings.pixel_format));
        self.pending_control
            .push(RfbControlMessage::SetEncodings(settings.encodings.to_vec()));
        self.pending_format = Some(settings.pixel_format);
        self.update_interval_ms = settings.update_interval_ms;
    }

    /// Feed a measured round trip from the transport's probe seam.
    pub fn record_rtt(&mut self, rtt_ms: u32) {
        self.link.record_rtt(rtt_ms);
    }

    /// Feed a loss/stall event (read timeout, aborted update) at `now_ms`.
    pub fn record_stall(&mut self, now_ms: u64) {
        self.link.record_stall(now_ms);
    }

    /// Feed the payload size of one decoded update at `now_ms` (the effective
    /// frame-throughput signal).
    pub fn record_frame(&mut self, now_ms: u64, bytes: usize) {
        self.link.record_frame(now_ms, bytes);
    }

    /// The rolling link estimate as of `now_ms` (HUD / diagnostics).
    #[must_use]
    pub fn link_estimate(&self, now_ms: u64) -> LinkEstimate {
        self.link.estimate(now_ms)
    }

    /// Replace the link-grade thresholds (shell/operator tuning).
    pub const fn set_link_thresholds(&mut self, thresholds: LinkThresholds) {
        self.thresholds = thresholds;
    }

    /// One auto-quality step: grade the current link estimate and let the
    /// ladder move the tier (degrade fast, upgrade slow). A no-op when a tier
    /// is pinned. A returned change is already applied live: its announcement
    /// is queued for [`VncSession::take_control`].
    pub fn autotune(&mut self, now_ms: u64) -> Option<TierChange> {
        if self.quality_mode != QualityMode::Auto {
            return None;
        }
        let grade = self.link.estimate(now_ms).grade(&self.thresholds);
        let change = self.ladder.observe(now_ms, grade)?;
        self.apply_tier(change.to);
        Some(change)
    }

    /// Borrow the queued-but-unsent session-control messages.
    #[must_use]
    pub fn pending_control(&self) -> &[RfbControlMessage] {
        &self.pending_control
    }

    /// Drain the queued session-control messages for the transport to send,
    /// adopting the pending decode format at the same moment: everything the
    /// server sends after the `SetPixelFormat` goes on the wire is in the new
    /// layout. The transport must call this at a safe point between update
    /// cycles (after the last requested update arrived), per RFC 6143 §7.5.1.
    pub fn take_control(&mut self) -> Vec<RfbControlMessage> {
        if let Some(format) = self.pending_format.take() {
            self.format = format;
        }
        std::mem::take(&mut self.pending_control)
    }
}

#[cfg(test)]
mod tests {
    use super::VncSession;
    use crate::config::VncConfig;
    use crate::egui::{Color32, Event, Key, Modifiers, PointerButton, Pos2, Vec2};
    use crate::encoding::Rectangle;
    use crate::input::{ALT_KEYSYM, CTRL_KEYSYM, SHIFT_KEYSYM};
    use crate::link::{QualityMode, QualityTier};
    use crate::pixel::PixelFormat;
    use crate::tier::PREFERRED_ENCODINGS;
    use crate::wire::{RfbClientMessage, RfbControlMessage};

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
        fn make() -> PixelFormat {
            PixelFormat {
                true_color: false,
                ..PixelFormat::rgba8888()
            }
        }
    }

    // ── Adaptive quality (E12-10) ───────────────────────────────────────────

    #[test]
    fn new_session_announces_the_full_tier() {
        let mut s = session();
        assert_eq!(s.quality_mode(), QualityMode::Auto);
        assert_eq!(s.quality_tier(), QualityTier::Full);
        assert_eq!(s.update_interval_ms(), 16);
        // The standard RFB client opening rides the control queue.
        assert_eq!(
            s.pending_control(),
            &[
                RfbControlMessage::SetPixelFormat(PixelFormat::rgba8888()),
                RfbControlMessage::SetEncodings(PREFERRED_ENCODINGS.to_vec()),
            ]
        );
        let drained = s.take_control();
        assert_eq!(drained.len(), 2);
        assert!(s.pending_control().is_empty(), "drained");
        assert_eq!(s.format(), PixelFormat::rgba8888());
    }

    #[test]
    fn pinning_a_tier_applies_live_via_the_control_queue() {
        let mut s = session();
        let _ = s.take_control(); // consume the opening announcement
        let change = s
            .set_quality_mode(QualityMode::Pinned(QualityTier::Reduced), 1_000)
            .expect("tier changed");
        assert_eq!(change.from, QualityTier::Full);
        assert_eq!(change.to, QualityTier::Reduced);
        assert_eq!(s.update_interval_ms(), 33, "pacing adopted immediately");
        assert_eq!(
            s.pending_control()[0],
            RfbControlMessage::SetPixelFormat(PixelFormat::rgb565())
        );
        // The decode format only flips at send time (updates in flight are
        // still 32-bpp)…
        assert_eq!(s.format(), PixelFormat::rgba8888());
        let _ = s.take_control();
        assert_eq!(s.format(), PixelFormat::rgb565());
        // Re-pinning the same tier is not a change and queues nothing.
        assert!(s
            .set_quality_mode(QualityMode::Pinned(QualityTier::Reduced), 2_000)
            .is_none());
        assert!(s.pending_control().is_empty());
    }

    #[test]
    fn autotune_degrades_live_on_a_sustained_bad_link() {
        let mut s = session();
        let _ = s.take_control();
        s.record_rtt(600); // >= 250 ms grades Bad
        assert!(s.autotune(1_000).is_none(), "hysteresis holds");
        assert!(s.autotune(2_000).is_none());
        let change = s.autotune(3_000).expect("third bad sample steps down");
        assert!(change.is_degrade());
        assert_eq!(change.to, QualityTier::Reduced);
        assert_eq!(s.quality_tier(), QualityTier::Reduced);
        assert_eq!(s.update_interval_ms(), 33);
        assert_eq!(s.pending_control().len(), 2, "announcement queued");
    }

    #[test]
    fn pinned_mode_blocks_autotune_and_unpin_resumes_from_the_pin() {
        let mut s = session();
        let _ = s.take_control();
        s.set_quality_mode(QualityMode::Pinned(QualityTier::Minimal), 0);
        let _ = s.take_control();
        assert_eq!(s.format(), PixelFormat::bgr233());
        assert_eq!(s.update_interval_ms(), 200);
        s.record_rtt(600);
        for i in 0..10_u64 {
            assert!(s.autotune(i * 1_000).is_none(), "pinned: no auto steps");
        }
        // Back to auto: the ladder resumes from Minimal, not from Full.
        assert!(s.set_quality_mode(QualityMode::Auto, 20_000).is_none());
        assert_eq!(s.quality_tier(), QualityTier::Minimal);
        // A recovered link upgrades slowly from there — and applies live.
        for _ in 0..64 {
            s.record_rtt(10);
        }
        assert!(s.autotune(21_000).is_none());
        let change = s.autotune(36_000).expect("15s of good upgrades one step");
        assert_eq!(change.to, QualityTier::Compressed);
        assert_eq!(s.update_interval_ms(), 66);
        assert_eq!(
            s.pending_control()[0],
            RfbControlMessage::SetPixelFormat(PixelFormat::bgr233())
        );
    }

    #[test]
    fn link_probes_shape_the_estimate() {
        let mut s = session();
        s.record_rtt(100);
        s.record_stall(1_000);
        s.record_frame(2_000, 5_000);
        let est = s.link_estimate(2_000);
        assert_eq!(est.rtt_ms, Some(100));
        assert_eq!(est.stalls_in_window, 1);
        assert_eq!(est.throughput_bps, Some(4_000), "5000 B over a 10 s window");
    }

    #[test]
    fn tier_settings_track_the_effective_tier() {
        let mut s = session();
        assert_eq!(s.tier_settings().pixel_format, PixelFormat::rgba8888());
        s.set_quality_mode(QualityMode::Pinned(QualityTier::Compressed), 0);
        assert_eq!(s.tier_settings().pixel_format, PixelFormat::bgr233());
        assert_eq!(s.tier_settings().update_interval_ms, 66);
    }
}
