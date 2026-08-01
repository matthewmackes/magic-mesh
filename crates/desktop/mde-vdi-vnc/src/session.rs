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
//! * [`VncSession::send_clipboard_to_guest`] queues a bounded RFB
//!   `ClientCutText`; [`VncSession::receive_server_cut_text`] accepts bounded
//!   `ServerCutText` from the transport while suppressing echoes of our own
//!   outbound cut text.
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
use crate::wire::{RfbClientMessage, RfbControlMessage, RfbCutText, RfbCutTextError};
use mde_egui::clipboard::TextClipboard;
use mde_vdi_core::{DamageLog, DamageRect, FrameDamage};
use std::time::{Duration, Instant};

/// A server echo is only suppressible for the short interval in which it can
/// plausibly be the response to our host→guest materialization. Keeping this
/// finite ensures a later, legitimate guest copy of identical text is visible.
const CLIENT_CUT_TEXT_ECHO_WINDOW: Duration = Duration::from_secs(2);

/// Debounce duplicate server notifications without suppressing a later copy.
const SERVER_CUT_TEXT_DUPLICATE_WINDOW: Duration = Duration::from_millis(250);

/// The egui-facing RFB desktop: a framebuffer the shell renders + an input queue
/// the transport drains.
pub struct VncSession {
    config: VncConfig,
    format: PixelFormat,
    framebuffer: Framebuffer,
    /// Set whenever the framebuffer changed since the last [`VncSession::frame`].
    dirty: bool,
    /// The changed rectangles accumulated since the last frame — the partial-upload
    /// hint the shell reads via [`VncSession::frame_with_damage`] (perf-7). Purely
    /// additive: `dirty` still gates whether a frame is emitted, so a stale or empty
    /// log only ever degrades the shell to a (correct) full upload.
    damage: DamageLog,
    /// Wire-ready input messages awaiting the transport, in arrival order.
    pending: Vec<RfbClientMessage>,
    /// The newest bounded guest→host clipboard text received from RFB
    /// `ServerCutText` and awaiting the shell/mesh clipboard publisher. There is
    /// one slot because clipboard state is latest-value-wins; this prevents a
    /// chatty guest from growing an unbounded queue while the shell is busy.
    pending_guest_clipboard: Vec<RfbCutText>,
    /// Last host→guest clipboard payload sent as `ClientCutText`, with its send
    /// time; used once to suppress a plausible server echo.
    pending_client_cut_text_echo: Option<(RfbCutText, Instant)>,
    /// Last accepted/suppressed server clipboard payload and its receive time;
    /// used to debounce only immediate duplicate guest cut-text events.
    last_server_cut_text: Option<(RfbCutText, Instant)>,
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

/// The direct-seat text clipboard view of a VNC session.
///
/// The shared [`TextClipboard`] trait deliberately has no error return because
/// a local clipboard provider may be temporarily unavailable. VNC still keeps
/// its protocol error observable: [`Self::write_text_checked`] returns the
/// bounded RFB error, while the trait implementation records it for
/// [`Self::take_error`]. A successful write queues a real `ClientCutText`; the
/// live connection flushes that queue through [`crate::connect::VncConnection`].
/// Guest `ServerCutText` values are consumed from the same latest-value-wins
/// queue, so no second clipboard store or Wayland helper is involved.
pub struct VncTextClipboard<'a> {
    session: &'a mut VncSession,
    last_error: Option<RfbCutTextError>,
}

impl<'a> VncTextClipboard<'a> {
    /// Borrow the VNC session's native text clipboard seam.
    #[must_use]
    pub(crate) fn new(session: &'a mut VncSession) -> Self {
        Self {
            session,
            last_error: None,
        }
    }

    /// Queue host/seat text for the guest and return the real RFB validation
    /// result. Nothing is queued when the UTF-8 payload exceeds the 1 MiB cap.
    pub fn write_text_checked(&mut self, text: &str) -> Result<(), RfbCutTextError> {
        self.session.send_clipboard_to_guest(text)
    }

    /// Take an error captured by the infallible [`TextClipboard::write_text`]
    /// callback, if the caller did not use [`Self::write_text_checked`].
    pub fn take_error(&mut self) -> Option<RfbCutTextError> {
        self.last_error.take()
    }
}

impl TextClipboard for VncTextClipboard<'_> {
    fn read_text(&mut self) -> Option<String> {
        self.session
            .take_guest_clipboard()
            .into_iter()
            .last()
            .map(RfbCutText::into_text)
    }

    fn write_text(&mut self, text: &str) {
        if let Err(error) = self.write_text_checked(text) {
            self.last_error = Some(error);
        }
    }
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
            damage: DamageLog::new(),
            pending: Vec::new(),
            pending_guest_clipboard: Vec::new(),
            pending_client_cut_text_echo: None,
            last_server_cut_text: None,
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

    /// Current text clipboard capability for this session.
    #[must_use]
    pub const fn clipboard_status(&self) -> VncClipboardCapability {
        vnc_clipboard_status()
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
            // The batch decoder blits its rectangles internally without surfacing
            // their geometry here, so mark the whole surface changed — the shell
            // falls back to a full upload for this path. The per-rectangle
            // [`VncSession::apply_rect`] entry the live transport uses does carry
            // exact geometry (below).
            self.damage.mark_full();
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
        // Record exactly the decoded rectangle so the shell can partial-upload it
        // (empty rects are ignored by the log). `CopyRect` moves pixels *into* this
        // destination rectangle, so the destination bounds are the changed region.
        self.damage.push(DamageRect::new(
            usize::from(rect.x),
            usize::from(rect.y),
            usize::from(rect.width),
            usize::from(rect.height),
        ));
        Ok(())
    }

    /// Resize the framebuffer (the `DesktopSize` pseudo-encoding / a `ServerInit`
    /// larger than the configured default) and mark it dirty.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.framebuffer
            .resize(usize::from(width), usize::from(height));
        self.dirty = true;
        // A resize reallocates the desktop; the shell must full-`set` (which resizes
        // the GPU texture) rather than partial-upload into the old dimensions.
        self.damage.mark_full();
    }

    /// The latest desktop as an [`egui::ColorImage`], or `None` if nothing changed
    /// since the previous call. Clears the dirty flag. Equivalent to
    /// [`VncSession::frame_with_damage`] ignoring the damage hint.
    pub fn frame(&mut self) -> Option<ColorImage> {
        self.frame_with_damage().map(|(image, _)| image)
    }

    /// The latest desktop plus which rectangles changed since the previous call, or
    /// `None` if nothing changed. Clears the dirty flag + drains the damage log.
    ///
    /// The damage is a hint for a partial GPU upload ([`FrameDamage::Rects`]); the
    /// first frame, a resize, and the batch-decode path report [`FrameDamage::Full`],
    /// so the shell always has a correct full upload to fall back to. `dirty` — not
    /// the damage log — decides whether a frame is emitted.
    pub fn frame_with_damage(&mut self) -> Option<(ColorImage, FrameDamage)> {
        if self.dirty {
            self.dirty = false;
            let damage = self.damage.take().unwrap_or(FrameDamage::Full);
            Some((self.framebuffer.to_color_image(), damage))
        } else {
            self.damage.clear();
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

    /// Put messages back at the front of the transport queue after a failed
    /// socket write.  The live connector uses this to make `ClientCutText`
    /// retry-safe: a transient TCP error must not acknowledge a clipboard
    /// materialization that never reached the guest.
    #[allow(
        dead_code,
        reason = "the live-connect transport calls this helper when its feature is enabled"
    )]
    pub(crate) fn requeue_input(&mut self, mut messages: Vec<RfbClientMessage>) {
        messages.append(&mut self.pending);
        self.pending = messages;
    }

    // ── Clipboard side (real RFB ClientCutText / ServerCutText) ─────────────

    /// Queue host clipboard text for materialization into the guest through the
    /// real RFB `ClientCutText` message.
    ///
    /// This only affects the outgoing RFB queue. It does not publish a
    /// guest→host clipboard event, which keeps directionality explicit.
    ///
    /// # Errors
    /// [`RfbCutTextError::TooLarge`] if the UTF-8 payload exceeds the governed
    /// 1 MiB guest transport cap.
    pub fn send_clipboard_to_guest(
        &mut self,
        text: impl Into<String>,
    ) -> Result<(), RfbCutTextError> {
        self.send_clipboard_to_guest_at(text, Instant::now())
    }

    fn send_clipboard_to_guest_at(
        &mut self,
        text: impl Into<String>,
        now: Instant,
    ) -> Result<(), RfbCutTextError> {
        let cut_text = RfbCutText::new(text)?;
        self.pending_client_cut_text_echo = Some((cut_text.clone(), now));
        self.pending.push(RfbClientMessage::ClientCutText(cut_text));
        Ok(())
    }

    /// Apply a bounded guest clipboard payload decoded from RFB `ServerCutText`.
    ///
    /// Echoes of our last `ClientCutText` and duplicate guest payloads are
    /// suppressed here so the shell publishes only real guest→host clipboard
    /// changes onto the mesh lane.
    pub fn receive_server_cut_text(&mut self, cut_text: RfbCutText) -> VncClipboardStatus {
        self.receive_server_cut_text_at(cut_text, Instant::now())
    }

    fn receive_server_cut_text_at(
        &mut self,
        cut_text: RfbCutText,
        now: Instant,
    ) -> VncClipboardStatus {
        let suppress_echo =
            self.pending_client_cut_text_echo
                .as_ref()
                .is_some_and(|(expected, sent_at)| {
                    expected == &cut_text
                        && now.saturating_duration_since(*sent_at) <= CLIENT_CUT_TEXT_ECHO_WINDOW
                });
        if suppress_echo {
            // One-shot: a later guest copy of the same value must not be hidden
            // by a permanently remembered host materialization.
            self.pending_client_cut_text_echo = None;
            return VncClipboardStatus::EchoSuppressed;
        }
        if self
            .pending_client_cut_text_echo
            .as_ref()
            .is_some_and(|(_, sent_at)| {
                now.saturating_duration_since(*sent_at) > CLIENT_CUT_TEXT_ECHO_WINDOW
            })
        {
            self.pending_client_cut_text_echo = None;
        }

        if self
            .last_server_cut_text
            .as_ref()
            .is_some_and(|(previous, received_at)| {
                previous == &cut_text
                    && now.saturating_duration_since(*received_at)
                        <= SERVER_CUT_TEXT_DUPLICATE_WINDOW
            })
        {
            return VncClipboardStatus::DuplicateSuppressed;
        }

        self.last_server_cut_text = Some((cut_text.clone(), now));
        if let Some(pending) = self.pending_guest_clipboard.first_mut() {
            *pending = cut_text;
            VncClipboardStatus::GuestTextReplaced
        } else {
            self.pending_guest_clipboard.push(cut_text);
            VncClipboardStatus::GuestTextQueued
        }
    }

    /// Borrow guest clipboard events waiting for the shell/mesh publisher.
    #[must_use]
    pub fn pending_guest_clipboard(&self) -> &[RfbCutText] {
        &self.pending_guest_clipboard
    }

    /// Drain guest clipboard events waiting for the shell/mesh publisher.
    #[must_use]
    pub fn take_guest_clipboard(&mut self) -> Vec<RfbCutText> {
        std::mem::take(&mut self.pending_guest_clipboard)
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

    /// Borrow the native VNC clipboard through the shared direct-seat contract.
    ///
    /// The returned adapter queues host text in this session and consumes guest
    /// text delivered by `ServerCutText`. Drop it before borrowing the session
    /// again to flush input or inspect the queue.
    pub fn text_clipboard(&mut self) -> VncTextClipboard<'_> {
        VncTextClipboard::new(self)
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

/// Result of handling a guest→host RFB `ServerCutText` payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VncClipboardStatus {
    /// The guest text was accepted and queued for the shell/mesh clipboard lane.
    GuestTextQueued,
    /// A newer guest value replaced one that the shell has not consumed yet.
    GuestTextReplaced,
    /// The server echoed our last host→guest `ClientCutText`; no publication.
    EchoSuppressed,
    /// The guest repeated the last `ServerCutText`; no duplicate publication.
    DuplicateSuppressed,
}

/// The protocol capability exposed by the VNC session's real clipboard seam.
///
/// Basic RFB has separate `ClientCutText` and `ServerCutText` messages. The
/// session uses both, so this report is bidirectional; it is intentionally a
/// VNC-local type because the shared RDP/SPICE capability enum names channels
/// those protocols do not share.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VncClipboardChannel {
    /// RFB `ClientCutText` / `ServerCutText` messages.
    RfbCutText,
}

/// VNC text clipboard capability, including the concrete protocol channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VncClipboardCapability {
    /// Host/mesh clipboard materialization into the guest.
    pub host_to_guest: VncClipboardChannel,
    /// Guest clipboard publication back to the host/mesh lane.
    pub guest_to_host: VncClipboardChannel,
}

impl VncClipboardCapability {
    /// The currently implemented bidirectional RFB clipboard capability.
    #[must_use]
    pub const fn rfb_cut_text() -> Self {
        Self {
            host_to_guest: VncClipboardChannel::RfbCutText,
            guest_to_host: VncClipboardChannel::RfbCutText,
        }
    }

    /// Whether both directions have real protocol messages behind them.
    #[must_use]
    pub const fn is_bidirectional(self) -> bool {
        matches!(
            (self.host_to_guest, self.guest_to_host),
            (
                VncClipboardChannel::RfbCutText,
                VncClipboardChannel::RfbCutText
            )
        )
    }
}

/// Report VNC's real bidirectional text clipboard capability.
#[must_use]
pub const fn vnc_clipboard_status() -> VncClipboardCapability {
    VncClipboardCapability::rfb_cut_text()
}

#[cfg(test)]
mod tests {
    use super::{
        VncClipboardChannel, VncClipboardStatus, VncSession, CLIENT_CUT_TEXT_ECHO_WINDOW,
        SERVER_CUT_TEXT_DUPLICATE_WINDOW,
    };
    use crate::config::VncConfig;
    use crate::egui::{Color32, Event, Key, Modifiers, PointerButton, Pos2, Vec2};
    use crate::encoding::Rectangle;
    use crate::input::{ALT_KEYSYM, CTRL_KEYSYM, SHIFT_KEYSYM};
    use crate::link::{QualityMode, QualityTier};
    use crate::pixel::PixelFormat;
    use crate::tier::PREFERRED_ENCODINGS;
    use crate::wire::{RfbClientMessage, RfbControlMessage, RfbCutText, RFB_CUT_TEXT_MAX_BYTES};
    use mde_egui::clipboard::TextClipboard;
    use std::time::{Duration, Instant};

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
    fn clipboard_status_reports_real_bidirectional_rfb_cut_text() {
        let session = session();
        let status = session.clipboard_status();
        assert!(status.is_bidirectional());
        assert_eq!(status.host_to_guest, VncClipboardChannel::RfbCutText);
        assert_eq!(status.guest_to_host, VncClipboardChannel::RfbCutText);
        assert_eq!(status, super::vnc_clipboard_status());
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
    fn first_frame_reports_full_damage() {
        use mde_vdi_core::FrameDamage;
        let mut s = session();
        let (img, damage) = s.frame_with_damage().expect("first frame");
        assert_eq!(img.size, [16, 16]);
        assert_eq!(damage, FrameDamage::Full);
        assert!(s.frame_with_damage().is_none(), "cleared");
    }

    #[test]
    fn apply_rect_reports_its_rectangle_as_damage() {
        use mde_vdi_core::{DamageRect, FrameDamage};
        let mut s = session();
        let _ = s.frame_with_damage(); // consume the initial full frame
        let rect = Rectangle {
            x: 4,
            y: 6,
            width: 2,
            height: 3,
            encoding: 0,
        };
        // 2x3 raw pixels (each [B,G,R,pad]) — content is irrelevant to the geometry.
        let payload = vec![0x00u8; 2 * 3 * 4];
        s.apply_rect(&rect, &payload).expect("rect");
        let (_img, damage) = s.frame_with_damage().expect("frame");
        assert_eq!(
            damage,
            FrameDamage::Rects(vec![DamageRect::new(4, 6, 2, 3)]),
            "the exact decoded rectangle is the damage"
        );
    }

    #[test]
    fn batch_update_and_resize_report_full_damage() {
        use mde_vdi_core::FrameDamage;
        let mut s = session();
        let _ = s.frame_with_damage();
        // The batch decoder does not surface per-rect geometry → full upload.
        let body = raw_update(2, &[[0, 0, 0xFF, 0], [0xFF, 0, 0, 0]]);
        s.apply_framebuffer_update(&body).expect("update");
        let (_img, damage) = s.frame_with_damage().expect("frame");
        assert_eq!(damage, FrameDamage::Full, "batch path is whole-frame");
        // A resize reallocates the desktop → full upload (texture must resize).
        s.resize(32, 24);
        let (_img, damage) = s.frame_with_damage().expect("resized frame");
        assert_eq!(damage, FrameDamage::Full, "resize is whole-frame");
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
    fn host_clipboard_queues_only_client_cut_text() {
        let mut s = session();
        s.send_clipboard_to_guest("host→guest")
            .expect("bounded clipboard text");
        assert_eq!(
            s.pending_input(),
            &[RfbClientMessage::ClientCutText(
                RfbCutText::new("host→guest").expect("bounded text")
            )],
            "host clipboard materialization must be a real RFB ClientCutText"
        );
        assert!(
            s.pending_guest_clipboard().is_empty(),
            "host→guest must not fabricate a guest→host clipboard event"
        );
    }

    #[test]
    fn host_clipboard_respects_the_one_mib_guest_cap() {
        let mut s = session();
        let too_big = "x".repeat(RFB_CUT_TEXT_MAX_BYTES + 1);
        assert!(
            s.send_clipboard_to_guest(too_big).is_err(),
            "over-cap guest clipboard materialization must fail"
        );
        assert!(
            s.pending_input().is_empty(),
            "failed materialization must not leave a partial RFB message queued"
        );
    }

    #[test]
    fn shared_seat_clipboard_adapter_round_trips_native_utf8() {
        let mut s = session();
        {
            let mut clipboard = s.text_clipboard();
            clipboard.write_text("seat→guest\r\n日本語");
            assert!(
                clipboard.take_error().is_none(),
                "valid UTF-8 must be accepted by the native RFB path"
            );
        }
        assert_eq!(
            s.take_input(),
            vec![RfbClientMessage::ClientCutText(
                RfbCutText::new("seat→guest\r\n日本語").expect("bounded UTF-8")
            )]
        );

        let status = s
            .receive_server_cut_text(RfbCutText::new("guest→seat\n日本語").expect("bounded UTF-8"));
        assert_eq!(status, VncClipboardStatus::GuestTextQueued);
        let mut clipboard = s.text_clipboard();
        assert_eq!(clipboard.read_text().as_deref(), Some("guest→seat\n日本語"));
        assert_eq!(clipboard.read_text(), None, "guest text is consumed once");
    }

    #[test]
    fn shared_seat_clipboard_adapter_surfaces_rfb_errors_without_queueing() {
        let mut s = session();
        let too_big = "x".repeat(RFB_CUT_TEXT_MAX_BYTES + 1);
        let mut clipboard = s.text_clipboard();
        clipboard.write_text(&too_big);
        assert!(
            clipboard.take_error().is_some(),
            "the infallible seat callback must retain an observable RFB error"
        );
        assert!(
            clipboard.write_text_checked(&too_big).is_err(),
            "checked callers receive the protocol error directly"
        );
        drop(clipboard);
        assert!(
            s.pending_input().is_empty(),
            "rejected text must not queue a partial ClientCutText"
        );
        let mut clipboard = s.text_clipboard();
        assert_eq!(
            clipboard.read_text(),
            None,
            "no guest text is honest absence"
        );
    }

    #[test]
    fn failed_transport_requeue_preserves_client_cut_text_order() {
        let mut s = session();
        s.send_clipboard_to_guest("first")
            .expect("bounded clipboard text");
        s.send_clipboard_to_guest("second")
            .expect("bounded clipboard text");
        let queued = s.take_input();

        s.send_input(&Event::Text("third".to_owned()));
        s.requeue_input(queued);

        let drained = s.take_input();
        assert!(matches!(
            &drained[0],
            RfbClientMessage::ClientCutText(text) if text.text() == "first"
        ));
        assert!(matches!(
            &drained[1],
            RfbClientMessage::ClientCutText(text) if text.text() == "second"
        ));
        assert!(
            drained.len() > 2,
            "new text input remains queued after the requeued clipboard messages"
        );
    }

    #[test]
    fn guest_clipboard_queues_inbound_text_without_outbound_echo() {
        let mut s = session();
        let status = s.receive_server_cut_text(RfbCutText::new("guest→host").expect("bounded"));
        assert_eq!(status, VncClipboardStatus::GuestTextQueued);
        assert!(
            s.pending_input().is_empty(),
            "guest→host must not queue a ClientCutText echo"
        );
        assert_eq!(
            s.take_guest_clipboard()
                .into_iter()
                .map(RfbCutText::into_text)
                .collect::<Vec<_>>(),
            vec!["guest→host".to_string()]
        );
    }

    #[test]
    fn server_echo_of_last_host_clipboard_is_suppressed() {
        let mut s = session();
        s.send_clipboard_to_guest("same text")
            .expect("bounded clipboard text");
        let _ = s.take_input();
        let status = s.receive_server_cut_text(RfbCutText::new("same text").expect("bounded"));
        assert_eq!(status, VncClipboardStatus::EchoSuppressed);
        assert!(
            s.pending_guest_clipboard().is_empty(),
            "server echo of our ClientCutText must not publish back to host"
        );
    }

    #[test]
    fn server_echo_guard_is_one_shot_and_later_identical_copy_survives() {
        let mut s = session();
        let now = Instant::now();
        let echo = RfbCutText::new("same text").expect("bounded");

        s.send_clipboard_to_guest_at("same text", now)
            .expect("bounded clipboard text");
        let _ = s.take_input();
        assert_eq!(
            s.receive_server_cut_text_at(echo.clone(), now + Duration::from_millis(1)),
            VncClipboardStatus::EchoSuppressed
        );
        assert!(s.pending_guest_clipboard().is_empty());

        assert_eq!(
            s.receive_server_cut_text_at(echo, now + Duration::from_millis(2)),
            VncClipboardStatus::GuestTextQueued,
            "a later legitimate identical guest copy must survive the one-shot guard"
        );
        assert_eq!(
            s.take_guest_clipboard()
                .into_iter()
                .map(RfbCutText::into_text)
                .collect::<Vec<_>>(),
            vec!["same text".to_owned()]
        );
    }

    #[test]
    fn expired_echo_and_duplicate_guards_do_not_hide_guest_copies() {
        let mut s = session();
        let now = Instant::now();

        s.send_clipboard_to_guest_at("same text", now)
            .expect("bounded clipboard text");
        let _ = s.take_input();
        assert_eq!(
            s.receive_server_cut_text_at(
                RfbCutText::new("same text").expect("bounded"),
                now + CLIENT_CUT_TEXT_ECHO_WINDOW + Duration::from_millis(1),
            ),
            VncClipboardStatus::GuestTextQueued
        );
        let _ = s.take_guest_clipboard();

        assert_eq!(
            s.receive_server_cut_text_at(
                RfbCutText::new("same text").expect("bounded"),
                now + CLIENT_CUT_TEXT_ECHO_WINDOW
                    + SERVER_CUT_TEXT_DUPLICATE_WINDOW
                    + Duration::from_millis(2),
            ),
            VncClipboardStatus::GuestTextQueued
        );
    }

    #[test]
    fn pending_guest_clipboard_is_latest_value_wins() {
        let mut s = session();
        let now = Instant::now();
        assert_eq!(
            s.receive_server_cut_text_at(RfbCutText::new("old").expect("bounded"), now),
            VncClipboardStatus::GuestTextQueued
        );
        assert_eq!(
            s.receive_server_cut_text_at(
                RfbCutText::new("newest").expect("bounded"),
                now + Duration::from_millis(1),
            ),
            VncClipboardStatus::GuestTextReplaced
        );
        assert_eq!(s.pending_guest_clipboard().len(), 1);
        assert_eq!(
            s.take_guest_clipboard()
                .into_iter()
                .map(RfbCutText::into_text)
                .collect::<Vec<_>>(),
            vec!["newest".to_owned()]
        );
    }

    #[test]
    fn duplicate_guest_clipboard_is_suppressed() {
        let mut s = session();
        assert_eq!(
            s.receive_server_cut_text(RfbCutText::new("guest").expect("bounded")),
            VncClipboardStatus::GuestTextQueued
        );
        let _ = s.take_guest_clipboard();
        assert_eq!(
            s.receive_server_cut_text(RfbCutText::new("guest").expect("bounded")),
            VncClipboardStatus::DuplicateSuppressed
        );
        assert!(s.pending_guest_clipboard().is_empty());
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
