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
//! * The **adaptive-codec surface (E12-10)**: the wire pump feeds link probes
//!   ([`RdpSession::record_rtt`] / [`RdpSession::record_stall`] /
//!   [`RdpSession::record_frame`]), [`RdpSession::autotune`] steps the target
//!   [`QualityTier`] on a weak link (manual pin via
//!   [`RdpSession::set_quality_mode`]), and — because RDP encoding knobs are
//!   connect-time only — [`RdpSession::needs_reconnect`] reports honestly when
//!   the target can only apply on the next reconnect (see [`crate::tier`]).
//!
//! This state machine is **ironrdp-free and fully unit-tested without a server**:
//! decode is fed through [`RdpSession::apply_rect`] / [`RdpSession::apply_full_frame`]
//! exactly as the live wire pump feeds it, and queued input is drained with
//! [`RdpSession::take_input`]. The live connection sequence that fills the
//! framebuffer from a real peer and flushes the queue onto the wire is layered on
//! in `connect` (behind the `live-connect` feature) — it calls these same
//! methods, so the tested path and the shipped path do not diverge.

use crate::config::{ConfigError, RdpConfig};
use crate::egui::{ColorImage, Event};
use crate::input::{map_event, map_text, ModifierState, RdpInputEvent};
use crate::link::{
    LadderConfig, LinkEstimate, LinkEstimator, LinkThresholds, QualityLadder, QualityMode,
    QualityTier, TierChange,
};
use crate::pixel::{Framebuffer, FramebufferError, PixelFormat};
use crate::tier::RdpTierSettings;
use mde_vdi_core::{DamageLog, DamageRect, FrameDamage};

/// The egui-facing RDP desktop: a framebuffer the shell renders + an input queue
/// the wire pump drains.
pub struct RdpSession {
    config: RdpConfig,
    framebuffer: Framebuffer,
    /// Set whenever the framebuffer changed since the last [`RdpSession::frame`].
    dirty: bool,
    /// The changed rectangles accumulated since the last frame — the partial-upload
    /// hint the shell reads via [`RdpSession::frame_with_damage`] (perf-7). Purely
    /// additive: `dirty` still gates whether a frame is emitted, so a stale or empty
    /// log only ever degrades the shell to a (correct) full upload.
    damage: DamageLog,
    /// Input intents awaiting the wire pump, in arrival order.
    pending: Vec<RdpInputEvent>,
    /// Last absolute pointer position pushed (desktop pixels).
    pointer: (u16, u16),
    /// Modifier keys already held on the guest (synthesised from egui snapshots).
    modifiers: ModifierState,
    /// Rolling link-quality estimates, fed by the wire pump's probe seam
    /// (E12-10 adaptive codec).
    link: LinkEstimator,
    /// The auto-quality ladder driving the target tier from the link grades.
    ladder: QualityLadder,
    /// Auto adaptation vs an operator-pinned tier.
    quality_mode: QualityMode,
    /// Grade cut-offs for [`RdpSession::autotune`].
    thresholds: LinkThresholds,
    /// The tier the *current connection* was negotiated with. RDP encoding
    /// knobs are connect-time only ([`RdpTierSettings::APPLICATION`]), so a
    /// target tier differing from this raises [`RdpSession::needs_reconnect`].
    applied_tier: QualityTier,
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
            damage: DamageLog::new(),
            pending: Vec::new(),
            pointer: (0, 0),
            modifiers: ModifierState::default(),
            link: LinkEstimator::new(),
            ladder: QualityLadder::new(QualityTier::Full, LadderConfig::default()),
            quality_mode: QualityMode::Auto,
            thresholds: LinkThresholds::default(),
            applied_tier: QualityTier::Full,
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
        // Record exactly the blitted region so the shell can partial-upload it. The
        // framebuffer already rejected out-of-bounds rects above, so this is a real,
        // in-surface damage rectangle (empty rects are ignored by the log).
        self.damage.push(DamageRect::new(x, y, w, h));
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
        // A whole-desktop replace: the changed region is the entire surface, so the
        // shell must do a full upload.
        self.damage.mark_full();
        Ok(())
    }

    /// The latest desktop as an [`egui::ColorImage`], or `None` if nothing changed
    /// since the previous call. Clears the dirty flag. Equivalent to
    /// [`RdpSession::frame_with_damage`] ignoring the damage hint.
    pub fn frame(&mut self) -> Option<ColorImage> {
        self.frame_with_damage().map(|(image, _)| image)
    }

    /// The latest desktop plus which rectangles changed since the previous call, or
    /// `None` if nothing changed. Clears the dirty flag + drains the damage log.
    ///
    /// The damage is a hint for a partial GPU upload ([`FrameDamage::Rects`]); the
    /// first frame (and any path with no reliable geometry) reports
    /// [`FrameDamage::Full`], so the shell always has a correct upload to fall back
    /// to. `dirty` — not the damage log — decides whether a frame is emitted, so
    /// this never skips a frame `frame` would have produced.
    pub fn frame_with_damage(&mut self) -> Option<(ColorImage, FrameDamage)> {
        if self.dirty {
            self.dirty = false;
            let damage = self.damage.take().unwrap_or(FrameDamage::Full);
            Some((self.framebuffer.to_color_image(), damage))
        } else {
            // Keep the log in step with the (unchanged) dirty flag.
            self.damage.clear();
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

    // ── Adaptive quality (E12-10) ───────────────────────────────────────────
    //
    // RDP negotiates its encoding surface at connect time only (see
    // `crate::tier`), so this session tracks a *target* tier: the ladder / a
    // pin moves the target, `needs_reconnect` reports the gap, and the connect
    // layer closes it by reconnecting with `connect_settings` and calling
    // `mark_tier_applied`. A tier change is never silently a no-op.

    /// The auto/pinned quality mode.
    #[must_use]
    pub const fn quality_mode(&self) -> QualityMode {
        self.quality_mode
    }

    /// The effective *target* tier: the pinned tier, or the auto ladder's.
    #[must_use]
    pub const fn quality_tier(&self) -> QualityTier {
        match self.quality_mode {
            QualityMode::Pinned(tier) => tier,
            QualityMode::Auto => self.ladder.tier(),
        }
    }

    /// The tier the current connection was negotiated with.
    #[must_use]
    pub const fn applied_tier(&self) -> QualityTier {
        self.applied_tier
    }

    /// Pin a tier or return to auto, reporting the target-tier change if any.
    ///
    /// RDP applies tiers **on reconnect only** ([`RdpTierSettings::APPLICATION`]):
    /// a returned change moves the *target*, and the session raises
    /// [`RdpSession::needs_reconnect`] until the connect layer reconnects with
    /// [`RdpSession::connect_settings`] and calls
    /// [`RdpSession::mark_tier_applied`]. Returning to auto resumes the ladder
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
        (to != from).then(|| {
            tracing::info!(
                from = from.label(),
                to = to.label(),
                "rdp quality target changed (applies on reconnect)"
            );
            TierChange {
                from,
                to,
                at_ms: now_ms,
            }
        })
    }

    /// Whether the target tier differs from the negotiated one — the change
    /// can only take effect on a reconnect built from
    /// [`RdpSession::connect_settings`].
    #[must_use]
    pub fn needs_reconnect(&self) -> bool {
        self.quality_tier() != self.applied_tier
    }

    /// The connect-time settings for the target tier — what the next
    /// (re)connect must be built from.
    #[must_use]
    pub fn connect_settings(&self) -> RdpTierSettings {
        RdpTierSettings::for_tier(self.quality_tier())
    }

    /// The connect layer reconnected with [`RdpSession::connect_settings`]:
    /// the target tier is now the negotiated one.
    pub const fn mark_tier_applied(&mut self) {
        self.applied_tier = self.quality_tier();
    }

    /// Feed a measured round trip from the wire pump's probe seam.
    pub fn record_rtt(&mut self, rtt_ms: u32) {
        self.link.record_rtt(rtt_ms);
    }

    /// Feed a loss/stall event (read timeout, aborted frame) at `now_ms`.
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
    /// ladder move the target tier (degrade fast, upgrade slow). A no-op when
    /// a tier is pinned. A returned change moved the *target* only — see
    /// [`RdpSession::needs_reconnect`] for how it takes effect.
    pub fn autotune(&mut self, now_ms: u64) -> Option<TierChange> {
        if self.quality_mode != QualityMode::Auto {
            return None;
        }
        let grade = self.link.estimate(now_ms).grade(&self.thresholds);
        let change = self.ladder.observe(now_ms, grade)?;
        tracing::info!(
            from = change.from.label(),
            to = change.to.label(),
            "rdp auto quality step (applies on reconnect)"
        );
        Some(change)
    }
}

#[cfg(test)]
mod tests {
    use super::RdpSession;
    use crate::config::RdpConfig;
    use crate::egui::{Color32, Event, Key, Modifiers, Pos2};
    use crate::input::{RdpInputEvent, Scancode};
    use crate::link::{QualityMode, QualityTier};
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
    fn first_frame_reports_full_damage() {
        use mde_vdi_core::FrameDamage;
        let mut s = session();
        let (img, damage) = s.frame_with_damage().expect("first frame");
        assert_eq!(img.size, [200, 200]);
        assert_eq!(
            damage,
            FrameDamage::Full,
            "the initial upload is whole-frame"
        );
        assert!(s.frame_with_damage().is_none(), "cleared");
    }

    #[test]
    fn apply_rect_reports_its_rectangle_as_damage() {
        use mde_vdi_core::{DamageRect, FrameDamage};
        let mut s = session();
        let _ = s.frame_with_damage(); // consume the initial full frame
        let src = [0x00, 0x00, 0xFF, 0xFF]; // one BGRA red pixel
        s.apply_rect(3, 5, 1, 1, PixelFormat::Bgra, &src, 4)
            .expect("apply");
        let (_img, damage) = s.frame_with_damage().expect("frame");
        assert_eq!(
            damage,
            FrameDamage::Rects(vec![DamageRect::new(3, 5, 1, 1)]),
            "the exact blitted region is the damage"
        );
    }

    #[test]
    fn apply_full_frame_reports_full_damage() {
        use mde_vdi_core::FrameDamage;
        let mut s = session();
        let _ = s.frame_with_damage();
        let src = vec![0x00u8; 200 * 200 * 4];
        s.apply_full_frame(PixelFormat::Bgra, &src).expect("full");
        let (_img, damage) = s.frame_with_damage().expect("frame");
        assert_eq!(damage, FrameDamage::Full, "a full replace uploads whole");
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

    // ── Adaptive quality (E12-10) ───────────────────────────────────────────

    #[test]
    fn quality_starts_auto_full_with_nothing_pending() {
        let s = session();
        assert_eq!(s.quality_mode(), QualityMode::Auto);
        assert_eq!(s.quality_tier(), QualityTier::Full);
        assert_eq!(s.applied_tier(), QualityTier::Full);
        assert!(!s.needs_reconnect());
    }

    #[test]
    fn pinning_a_tier_raises_needs_reconnect_until_marked_applied() {
        let mut s = session();
        let change = s
            .set_quality_mode(QualityMode::Pinned(QualityTier::Compressed), 1_000)
            .expect("target changed");
        assert_eq!(change.from, QualityTier::Full);
        assert_eq!(change.to, QualityTier::Compressed);
        assert!(s.needs_reconnect(), "RDP tiers are reconnect-gated");
        assert_eq!(s.connect_settings().color_depth, 16);
        // The connect layer reconnects with those settings…
        s.mark_tier_applied();
        assert!(!s.needs_reconnect());
        assert_eq!(s.applied_tier(), QualityTier::Compressed);
        // Re-pinning the same tier is not a change.
        assert!(s
            .set_quality_mode(QualityMode::Pinned(QualityTier::Compressed), 2_000)
            .is_none());
    }

    #[test]
    fn autotune_degrades_the_target_on_a_sustained_bad_link() {
        let mut s = session();
        // Three straight samples with a laggy RTT (>= 250 ms grades Bad).
        s.record_rtt(600);
        for i in 1..=3_u64 {
            let step = s.autotune(i * 1_000);
            if i < 3 {
                assert!(step.is_none(), "hysteresis: not before 3 bad samples");
            } else {
                let change = step.expect("third bad sample steps down");
                assert!(change.is_degrade());
                assert_eq!(change.to, QualityTier::Reduced);
            }
        }
        assert_eq!(s.quality_tier(), QualityTier::Reduced);
        assert!(
            s.needs_reconnect(),
            "the step is honest: reconnect required"
        );
        assert_eq!(s.applied_tier(), QualityTier::Full, "nothing switched live");
    }

    #[test]
    fn pinned_mode_blocks_autotune_and_unpin_resumes_from_the_pin() {
        let mut s = session();
        s.set_quality_mode(QualityMode::Pinned(QualityTier::Minimal), 0);
        s.mark_tier_applied();
        s.record_rtt(600);
        for i in 0..10_u64 {
            assert!(s.autotune(i * 1_000).is_none(), "pinned: no auto steps");
        }
        assert_eq!(s.quality_tier(), QualityTier::Minimal);
        // Back to auto: the ladder resumes from the pinned tier, not from Full.
        assert!(s.set_quality_mode(QualityMode::Auto, 20_000).is_none());
        assert_eq!(s.quality_tier(), QualityTier::Minimal);
        // A recovered link then upgrades slowly from there.
        s.record_rtt(10);
        for _ in 0..64 {
            s.record_rtt(10); // converge the EWMA well under good_rtt
        }
        assert!(s.autotune(21_000).is_none());
        let change = s.autotune(36_000).expect("15s of good upgrades one step");
        assert_eq!(change.to, QualityTier::Compressed);
        assert!(s.needs_reconnect());
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
}
