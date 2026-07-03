//! Per-pane activity / silence watching (TERM-12) — Terminator's "watch for
//! activity" and "watch for silence".
//!
//! Pure and headless: the watcher only *folds* an engine output counter (see
//! [`crate::engine::Terminal::bytes_seen`]) against a wall clock (egui's frame
//! time in seconds) into an edge event. The widget owns the effect — publishing
//! the notice through the shared [`crate::notify::NotifyBus`] seam — so the fold
//! is unit-tested without any Bus or toolkit.
//!
//! * **Activity**: output resumes after the pane has been quiet for
//!   [`ActivityWatch::threshold`] seconds — one edge per quiet-then-active cycle
//!   (a debounce, so a busy pane doesn't spam).
//! * **Silence**: the pane produces no output for `threshold` seconds — one edge
//!   per silence onset, re-armed by the next output.

/// The default quiet window, in seconds (Terminator's silence default).
pub const DEFAULT_THRESHOLD: f64 = 10.0;

/// Which watch, if any, a pane is under.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum WatchMode {
    /// Not watched (the default).
    #[default]
    Off,
    /// Fire when output resumes after a quiet window.
    Activity,
    /// Fire when the pane falls quiet for the whole window.
    Silence,
}

/// The edge a fold produced.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WatchEvent {
    /// New output arrived after a quiet window.
    Activity,
    /// The pane fell quiet for the whole window.
    Silence,
}

/// A pane's activity/silence watcher.
#[derive(Clone, Copy, Debug)]
pub struct ActivityWatch {
    mode: WatchMode,
    threshold: f64,
    /// The last engine output counter observed.
    last_seq: u64,
    /// The frame time (seconds) of the last observed output.
    last_output_at: f64,
    /// Whether a baseline observation has been taken.
    armed: bool,
    /// Whether the silence edge has fired since the last output (one per onset).
    silence_fired: bool,
}

impl Default for ActivityWatch {
    fn default() -> Self {
        Self {
            mode: WatchMode::Off,
            threshold: DEFAULT_THRESHOLD,
            last_seq: 0,
            last_output_at: 0.0,
            armed: false,
            silence_fired: false,
        }
    }
}

impl ActivityWatch {
    /// The current watch mode.
    #[must_use]
    pub const fn mode(self) -> WatchMode {
        self.mode
    }

    /// Whether the pane is under any watch.
    #[must_use]
    pub const fn is_active(self) -> bool {
        !matches!(self.mode, WatchMode::Off)
    }

    /// The quiet window in seconds.
    #[must_use]
    pub const fn threshold(self) -> f64 {
        self.threshold
    }

    /// Set the quiet window (clamped to at least a tenth of a second).
    pub const fn set_threshold(&mut self, secs: f64) {
        self.threshold = secs.max(0.1);
    }

    /// Set the watch mode outright, re-arming the silence edge.
    pub const fn set_mode(&mut self, mode: WatchMode) {
        self.mode = mode;
        self.silence_fired = false;
    }

    /// Toggle `mode` on, or back to [`WatchMode::Off`] when it is already active
    /// (the keybind semantics — the same chord turns its watch off).
    pub fn toggle(&mut self, mode: WatchMode) {
        let next = if self.mode == mode {
            WatchMode::Off
        } else {
            mode
        };
        self.set_mode(next);
    }

    /// Fold one frame's observation — the engine's current output counter `seq`
    /// and the frame time `now` (seconds) — into an edge event, if one is due.
    ///
    /// The first call only takes a baseline (never fires).
    pub fn observe(&mut self, seq: u64, now: f64) -> Option<WatchEvent> {
        if !self.armed {
            self.armed = true;
            self.last_seq = seq;
            self.last_output_at = now;
            return None;
        }

        if seq > self.last_seq {
            let quiet_before = now - self.last_output_at >= self.threshold;
            self.last_seq = seq;
            self.last_output_at = now;
            self.silence_fired = false;
            if matches!(self.mode, WatchMode::Activity) && quiet_before {
                return Some(WatchEvent::Activity);
            }
            return None;
        }

        if matches!(self.mode, WatchMode::Silence)
            && !self.silence_fired
            && now - self.last_output_at >= self.threshold
        {
            self.silence_fired = true;
            return Some(WatchEvent::Silence);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watch(mode: WatchMode) -> ActivityWatch {
        let mut w = ActivityWatch::default();
        w.set_threshold(5.0);
        w.set_mode(mode);
        w
    }

    #[test]
    fn off_never_fires() {
        let mut w = watch(WatchMode::Off);
        assert_eq!(w.observe(0, 0.0), None); // baseline
        assert_eq!(w.observe(100, 100.0), None);
        assert_eq!(w.observe(100, 200.0), None);
    }

    #[test]
    fn silence_fires_once_after_the_window_then_rearms_on_output() {
        let mut w = watch(WatchMode::Silence);
        assert_eq!(w.observe(10, 0.0), None); // baseline: last output at t=0
                                              // Still within the window → quiet, but not long enough.
        assert_eq!(w.observe(10, 3.0), None);
        // Crossed the 5s window with no new output → one Silence edge.
        assert_eq!(w.observe(10, 6.0), Some(WatchEvent::Silence));
        // Does not re-fire while it stays quiet.
        assert_eq!(w.observe(10, 20.0), None);
        // New output re-arms; then a fresh window fires again.
        assert_eq!(w.observe(11, 21.0), None);
        assert_eq!(w.observe(11, 27.0), Some(WatchEvent::Silence));
    }

    #[test]
    fn activity_fires_on_output_after_a_quiet_window() {
        let mut w = watch(WatchMode::Activity);
        assert_eq!(w.observe(10, 0.0), None); // baseline
                                              // Quiet for > 5s, then output arrives → Activity.
        assert_eq!(w.observe(20, 8.0), Some(WatchEvent::Activity));
        // Continuous output (no preceding quiet) does not re-fire.
        assert_eq!(w.observe(30, 8.5), None);
        assert_eq!(w.observe(40, 9.0), None);
    }

    #[test]
    fn a_toggle_turns_a_mode_on_then_back_off() {
        let mut w = ActivityWatch::default();
        assert_eq!(w.mode(), WatchMode::Off);
        w.toggle(WatchMode::Activity);
        assert_eq!(w.mode(), WatchMode::Activity);
        assert!(w.is_active());
        w.toggle(WatchMode::Activity);
        assert_eq!(w.mode(), WatchMode::Off);
        // Toggling a different mode from Off selects it.
        w.toggle(WatchMode::Silence);
        assert_eq!(w.mode(), WatchMode::Silence);
    }
}
