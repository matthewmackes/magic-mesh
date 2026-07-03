//! [`FakeMpv`] — a deterministic in-memory [`MediaEngine`] for tests and the
//! headless smoke.
//!
//! It simulates just enough of mpv's observable behaviour to exercise the whole
//! [`Player`](crate::Player) state machine without a real decoder: a `loadfile`
//! opens media and queues a [`FileLoaded`](EngineSignal::FileLoaded); the clock
//! only advances when explicitly told to; `stop` unloads and queues an
//! [`EndFile`](EngineSignal::EndFile). Because it is a real, reachable engine
//! (not a `#[cfg(test)]` mock) it is also what the default `media-smoke` binary
//! drives — so MEDIA-1 is runtime-observable even where system libmpv is absent,
//! and later units (e.g. MEDIA-8 headless mount tests) can reuse it.

use std::collections::VecDeque;

use crate::engine::{EndReason, EngineError, EngineSignal, MediaEngine, Track};

/// A scriptable fake engine — see the [module docs](self).
#[derive(Debug, Clone, Default)]
pub struct FakeMpv {
    loaded: Option<String>,
    paused: bool,
    position: f64,
    duration: Option<f64>,
    tracks: Vec<Track>,
    signals: VecDeque<EngineSignal>,
    fail_load: bool,
    /// Every `loadfile`/`seek`/`stop` issued, for assertion.
    commands: Vec<String>,
}

impl FakeMpv {
    /// A fresh fake with no media, no duration, no tracks.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-set the duration reported once media loads (seconds).
    #[must_use]
    pub const fn with_duration(mut self, secs: f64) -> Self {
        self.duration = Some(secs);
        self
    }

    /// Pre-set the track list reported once media loads.
    #[must_use]
    pub fn with_tracks(mut self, tracks: Vec<Track>) -> Self {
        self.tracks = tracks;
        self
    }

    /// Make the next (and every) `load_file` fail with a backend error.
    #[must_use]
    pub const fn failing_load(mut self) -> Self {
        self.fail_load = true;
        self
    }

    // ── test-drive helpers ─────────────────────────────────────────────────────

    /// Advance the playback clock by `secs` (only when not paused), as if time
    /// passed. Clamped to the duration when known.
    pub fn advance(&mut self, secs: f64) {
        if self.loaded.is_some() && !self.paused {
            self.position += secs;
            if let Some(dur) = self.duration {
                if self.position > dur {
                    self.position = dur;
                }
            }
        }
    }

    /// Queue a natural end-of-file signal (as if the clip played out).
    pub fn reach_eof(&mut self) {
        self.signals
            .push_back(EngineSignal::EndFile(EndReason::Eof));
    }

    /// Queue an error-reason end-of-file signal (as if decode failed).
    pub fn fail_playback(&mut self) {
        self.signals
            .push_back(EngineSignal::EndFile(EndReason::Error));
    }

    /// Queue an arbitrary signal (used to test out-of-band error handling).
    pub fn push_signal(&mut self, signal: EngineSignal) {
        self.signals.push_back(signal);
    }

    /// Whether the engine is currently paused (test assertion helper).
    #[must_use]
    pub const fn is_paused(&self) -> bool {
        self.paused
    }

    /// The commands issued so far (`"loadfile …"`, `"seek …"`, `"stop"`).
    #[must_use]
    pub fn commands(&self) -> &[String] {
        &self.commands
    }
}

impl MediaEngine for FakeMpv {
    fn load_file(&mut self, url: &str) -> Result<(), EngineError> {
        self.commands.push(format!("loadfile {url}"));
        if self.fail_load {
            return Err(EngineError::Backend(format!("cannot open {url}")));
        }
        self.loaded = Some(url.to_owned());
        self.paused = false;
        self.position = 0.0;
        self.signals.push_back(EngineSignal::FileLoaded);
        Ok(())
    }

    fn set_paused(&mut self, paused: bool) -> Result<(), EngineError> {
        if self.loaded.is_none() {
            return Err(EngineError::NotLoaded);
        }
        self.paused = paused;
        Ok(())
    }

    fn seek_absolute(&mut self, position_secs: f64) -> Result<(), EngineError> {
        if self.loaded.is_none() {
            return Err(EngineError::NotLoaded);
        }
        self.commands.push(format!("seek {position_secs} absolute"));
        self.position = position_secs.max(0.0);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), EngineError> {
        self.commands.push("stop".to_owned());
        self.loaded = None;
        self.position = 0.0;
        self.signals
            .push_back(EngineSignal::EndFile(EndReason::Stopped));
        Ok(())
    }

    fn position(&self) -> Option<f64> {
        self.loaded.as_ref().map(|_| self.position)
    }

    fn duration(&self) -> Option<f64> {
        self.loaded.as_ref().and(self.duration)
    }

    fn tracks(&self) -> Vec<Track> {
        if self.loaded.is_some() {
            self.tracks.clone()
        } else {
            Vec::new()
        }
    }

    fn poll(&mut self) -> Vec<EngineSignal> {
        self.signals.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_then_poll_yields_file_loaded() {
        let mut e = FakeMpv::new().with_duration(10.0);
        e.load_file("clip").expect("load");
        assert_eq!(e.poll(), vec![EngineSignal::FileLoaded]);
        assert_eq!(e.duration(), Some(10.0));
        assert_eq!(e.position(), Some(0.0));
        assert_eq!(e.commands(), ["loadfile clip"]);
    }

    #[test]
    fn transport_before_load_is_not_loaded() {
        let mut e = FakeMpv::new();
        assert_eq!(e.set_paused(true), Err(EngineError::NotLoaded));
        assert_eq!(e.seek_absolute(1.0), Err(EngineError::NotLoaded));
        assert_eq!(e.position(), None);
        assert!(e.tracks().is_empty());
    }

    #[test]
    fn advance_respects_pause_and_clamps() {
        let mut e = FakeMpv::new().with_duration(5.0);
        e.load_file("clip").expect("load");
        e.advance(3.0);
        assert_eq!(e.position(), Some(3.0));
        e.set_paused(true).expect("pause");
        e.advance(3.0); // paused → no movement
        assert_eq!(e.position(), Some(3.0));
        e.set_paused(false).expect("unpause");
        e.advance(100.0); // clamps to duration
        assert_eq!(e.position(), Some(5.0));
    }

    #[test]
    fn stop_unloads_and_signals() {
        let mut e = FakeMpv::new();
        e.load_file("clip").expect("load");
        let _ = e.poll();
        e.stop().expect("stop");
        assert_eq!(e.poll(), vec![EngineSignal::EndFile(EndReason::Stopped)]);
        assert_eq!(e.position(), None);
    }
}
