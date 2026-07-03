//! The configurable terminal bell (TERM-12) — a visual and/or audible reaction
//! to the ASCII BEL (`0x07`).
//!
//! The engine surfaces a `BEL` as [`crate::engine::TermEvent::Bell`]; this state
//! machine folds it, per the pane's [`BellConfig`], into a [`BellEffect`]:
//!
//! * **visual** — a brief pane flash that decays over [`FLASH_SECS`]; the pane
//!   chrome reads [`Bell::flash_alpha`] each frame (no toolkit needed here — the
//!   fold is pure).
//! * **audible** — since the DRM/egui surface has no audio stack of its own, the
//!   "audible" bell rides the shared [`crate::notify::NotifyBus`] seam (a short
//!   toast the desktop's notify hub sounds), rather than a hand-rolled beep.
//!
//! Both are independent knobs, so a pane can be silent, visual-only,
//! audible-only, or both — the "on/off/style" the unit asks for.

/// How long a visual bell flash lasts, in seconds.
pub const FLASH_SECS: f64 = 0.25;

/// A pane's bell style — the two independent knobs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BellConfig {
    /// Flash the pane on BEL.
    pub visual: bool,
    /// Raise an audible notice (through the notify seam) on BEL.
    pub audible: bool,
}

impl Default for BellConfig {
    /// Visual-only — the quiet, glanceable default (audible needs the notify hop).
    fn default() -> Self {
        Self {
            visual: true,
            audible: false,
        }
    }
}

impl BellConfig {
    /// The bell fully off — a BEL is swallowed.
    #[must_use]
    pub const fn off() -> Self {
        Self {
            visual: false,
            audible: false,
        }
    }

    /// Visual flash only.
    #[must_use]
    pub const fn visual_only() -> Self {
        Self {
            visual: true,
            audible: false,
        }
    }

    /// Audible notice only.
    #[must_use]
    pub const fn audible_only() -> Self {
        Self {
            visual: false,
            audible: true,
        }
    }

    /// Both a flash and an audible notice.
    #[must_use]
    pub const fn both() -> Self {
        Self {
            visual: true,
            audible: true,
        }
    }

    /// Whether any reaction is enabled.
    #[must_use]
    pub const fn is_enabled(self) -> bool {
        self.visual || self.audible
    }
}

/// What a BEL should produce this instant, per the pane's config.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct BellEffect {
    /// Start (or refresh) the visual flash.
    pub flash: bool,
    /// Raise an audible notice through the notify seam.
    pub notify: bool,
}

/// A pane's bell: its config plus the decaying visual-flash clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct Bell {
    config: BellConfig,
    /// The frame time (seconds) of the last visual flash, if one is decaying.
    last_flash_at: Option<f64>,
}

impl Bell {
    /// A bell with `config`.
    #[must_use]
    pub const fn new(config: BellConfig) -> Self {
        Self {
            config,
            last_flash_at: None,
        }
    }

    /// The current config.
    #[must_use]
    pub const fn config(self) -> BellConfig {
        self.config
    }

    /// Replace the config (a picker/config change).
    pub const fn set_config(&mut self, config: BellConfig) {
        self.config = config;
    }

    /// Fold a BEL that arrived at frame time `now`: start the flash if visual is
    /// on, and report whether an audible notice is due.
    pub const fn ring(&mut self, now: f64) -> BellEffect {
        let effect = BellEffect {
            flash: self.config.visual,
            notify: self.config.audible,
        };
        if effect.flash {
            self.last_flash_at = Some(now);
        }
        effect
    }

    /// The visual-flash intensity at frame time `now`, in `[0, 1]` — full at the
    /// ring, decaying linearly to `0` over [`FLASH_SECS`]. `0.0` when no flash is
    /// active (or the flash has decayed), so the chrome can skip the overlay.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)] // 0..1 f64 → f32 loses no meaningful precision.
    pub fn flash_alpha(self, now: f64) -> f32 {
        match self.last_flash_at {
            Some(t) if now >= t && now - t < FLASH_SECS => (1.0 - (now - t) / FLASH_SECS) as f32,
            _ => 0.0,
        }
    }

    /// Whether a visual flash is currently painting.
    #[must_use]
    pub fn is_flashing(self, now: f64) -> bool {
        self.flash_alpha(now) > 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_off_bell_swallows_the_bel() {
        let mut bell = Bell::new(BellConfig::off());
        let effect = bell.ring(1.0);
        assert_eq!(
            effect,
            BellEffect {
                flash: false,
                notify: false
            }
        );
        assert!(!bell.is_flashing(1.0));
    }

    #[test]
    fn a_visual_bell_flashes_and_decays() {
        let mut bell = Bell::new(BellConfig::visual_only());
        let effect = bell.ring(1.0);
        assert!(effect.flash && !effect.notify);
        // Full at the ring, dimmer partway, gone after the window.
        assert!((bell.flash_alpha(1.0) - 1.0).abs() < 1e-6);
        let mid = bell.flash_alpha(1.0 + FLASH_SECS / 2.0);
        assert!(mid > 0.0 && mid < 1.0);
        assert!(bell.flash_alpha(1.0 + FLASH_SECS).abs() < f32::EPSILON);
        assert!(!bell.is_flashing(2.0));
    }

    #[test]
    fn an_audible_bell_asks_for_a_notice_but_no_flash() {
        let mut bell = Bell::new(BellConfig::audible_only());
        let effect = bell.ring(3.0);
        assert!(effect.notify && !effect.flash);
        assert!(!bell.is_flashing(3.0));
    }

    #[test]
    fn a_both_bell_does_flash_and_notice() {
        let mut bell = Bell::new(BellConfig::both());
        let effect = bell.ring(0.0);
        assert_eq!(
            effect,
            BellEffect {
                flash: true,
                notify: true
            }
        );
        assert!(bell.is_flashing(0.1));
    }

    #[test]
    fn config_is_reconfigurable() {
        let mut bell = Bell::default();
        assert!(bell.config().visual && !bell.config().audible);
        bell.set_config(BellConfig::both());
        assert!(bell.config().is_enabled());
        assert!(bell.ring(0.0).notify);
    }
}
