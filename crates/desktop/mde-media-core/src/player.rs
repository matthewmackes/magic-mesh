//! The [`Player`] — a state machine over a [`MediaEngine`].
//!
//! `Player` owns the transport verbs the future surface (MEDIA-8) calls —
//! `load` / `play` / `pause` / `seek` / `stop` — and turns the engine's raw
//! [`EngineSignal`]s into ordered [`PlayerEvent`]s plus a single authoritative
//! [`PlayerState`]. It is generic over the engine, so the same code runs over
//! the real mpv wrapper *and* over [`FakeMpv`](crate::FakeMpv) in tests.
//!
//! ```text
//!            load()                 FileLoaded (pump)          EndFile(Eof)
//!   Idle ─────────────▶ Loading ───────────────────▶ Playing ───────────▶ Ended
//!    ▲                     │  \__ paused_intent __▶ Paused ◀── pause() ──┐   │
//!    │ stop() (any loaded state) ─────────────▶ Stopped                 play()/
//!    └──────────────────────── load() (from any state) ◀────────────────┘  seek()
//! ```

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::audio::AudioConfig;
use crate::engine::{EndReason, EngineError, EngineSignal, MediaEngine, Track};
use crate::video::VideoConfig;

/// The authoritative playback state.
///
/// Exactly one is current at any time; [`Player::state`] returns it and every
/// transition emits a [`PlayerEvent::StateChanged`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayerState {
    /// Nothing loaded — the initial state, and after a fresh construction.
    Idle,
    /// A `load` was issued; awaiting the engine's `FileLoaded` before playback.
    Loading,
    /// Media is loaded and playing.
    Playing,
    /// Media is loaded and paused (position held).
    Paused,
    /// Playback was explicitly stopped and the file unloaded.
    Stopped,
    /// The media played to its natural end (EOF); still loaded at the end.
    Ended,
}

impl PlayerState {
    /// Whether media is loaded in this state (so transport verbs apply).
    #[must_use]
    const fn has_media(self) -> bool {
        matches!(
            self,
            Self::Loading | Self::Playing | Self::Paused | Self::Ended
        )
    }
}

/// An observable change emitted by the [`Player`], drained via
/// [`Player::drain_events`].
///
/// The surface renders from these: transport-state for the play/pause button,
/// position/duration for the scrubber, tracks for the track menu.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    /// The playback state transitioned to the given [`PlayerState`].
    StateChanged(PlayerState),
    /// The playback position advanced/seeked to this many seconds.
    PositionChanged(f64),
    /// The media duration became known (or changed) — seconds.
    DurationChanged(f64),
    /// The enumerated track list changed (new media loaded).
    TracksChanged(Vec<Track>),
    /// The media reached its natural end.
    EndReached,
    /// An error was surfaced (engine error or invalid transport request).
    Error(String),
}

/// A transport request the player refused, without touching the engine.
///
/// Refusing loudly (rather than silently no-op'ing) keeps the state machine
/// honest and unit-testable — e.g. `seek` with nothing loaded is a real error,
/// not a swallowed request.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum PlayerError {
    /// The engine rejected the underlying command.
    #[error(transparent)]
    Engine(#[from] EngineError),
    /// The verb does not apply in the current state.
    #[error("cannot {op} while {state:?}")]
    InvalidState {
        /// The transport verb that was refused (`"play"`, `"seek"`, …).
        op: &'static str,
        /// The state it was refused in.
        state: PlayerState,
    },
}

/// The player core: a [`MediaEngine`] plus the state machine over it.
///
/// Construct with [`Player::new`], drive with the transport verbs, call
/// [`pump`](Player::pump) each tick to fold in engine signals + live
/// position/duration, and read the resulting [`PlayerEvent`]s with
/// [`drain_events`](Player::drain_events).
#[derive(Debug)]
pub struct Player<E: MediaEngine> {
    engine: E,
    state: PlayerState,
    /// Desired paused-ness, applied when the file becomes ready (mpv autoplays
    /// on load unless `pause` is set, so we record intent across `Loading`).
    paused_intent: bool,
    media: Option<String>,
    position: f64,
    duration: Option<f64>,
    tracks: Vec<Track>,
    /// The applied audio-processing config (MEDIA-3). Set via
    /// [`set_audio_config`](Player::set_audio_config); mpv keeps these ao/af/
    /// `ReplayGain`/gapless properties across loads, so it is apply-on-change.
    audio: AudioConfig,
    /// The applied video decode + adjustments config (MEDIA-4). Set via
    /// [`set_video_config`](Player::set_video_config); mpv keeps these hwdec/vf/
    /// `video-*`/deinterlace properties across loads, so it is apply-on-change.
    video: VideoConfig,
    events: VecDeque<PlayerEvent>,
}

impl<E: MediaEngine> Player<E> {
    /// Wrap an engine in a fresh [`PlayerState::Idle`] player.
    #[must_use]
    pub const fn new(engine: E) -> Self {
        Self {
            engine,
            state: PlayerState::Idle,
            paused_intent: false,
            media: None,
            position: 0.0,
            duration: None,
            tracks: Vec::new(),
            audio: AudioConfig::new(),
            video: VideoConfig::new(),
            events: VecDeque::new(),
        }
    }

    // ── accessors ────────────────────────────────────────────────────────────

    /// The current playback state.
    #[must_use]
    pub const fn state(&self) -> PlayerState {
        self.state
    }

    /// The current playback position in seconds.
    #[must_use]
    pub const fn position(&self) -> f64 {
        self.position
    }

    /// The media duration in seconds, if known.
    #[must_use]
    pub const fn duration(&self) -> Option<f64> {
        self.duration
    }

    /// The enumerated tracks of the loaded media.
    #[must_use]
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// The URL/path of the loaded media, if any.
    #[must_use]
    pub fn media(&self) -> Option<&str> {
        self.media.as_deref()
    }

    /// The applied audio-processing config (MEDIA-3).
    #[must_use]
    pub const fn audio_config(&self) -> &AudioConfig {
        &self.audio
    }

    /// The applied video decode + adjustments config (MEDIA-4).
    #[must_use]
    pub const fn video_config(&self) -> &VideoConfig {
        &self.video
    }

    /// Borrow the underlying engine (tests drive [`FakeMpv`](crate::FakeMpv)
    /// through this).
    #[must_use]
    pub const fn engine(&self) -> &E {
        &self.engine
    }

    /// Mutably borrow the underlying engine.
    pub const fn engine_mut(&mut self) -> &mut E {
        &mut self.engine
    }

    // ── transport ────────────────────────────────────────────────────────────

    /// Load `url` and begin playback (subject to a later `pause`).
    ///
    /// Valid from any state — a load replaces whatever was loaded. Transitions to
    /// [`PlayerState::Loading`]; the move to `Playing`/`Paused` happens when
    /// [`pump`](Self::pump) sees the engine's [`EngineSignal::FileLoaded`].
    ///
    /// # Errors
    /// Returns [`PlayerError::Engine`] if the engine rejects the load.
    pub fn load(&mut self, url: impl Into<String>) -> Result<(), PlayerError> {
        let url = url.into();
        self.engine.load_file(&url)?;
        self.media = Some(url);
        self.paused_intent = false;
        self.position = 0.0;
        self.duration = None;
        if !self.tracks.is_empty() {
            self.tracks.clear();
        }
        self.set_state(PlayerState::Loading);
        Ok(())
    }

    /// Resume (or start) playback.
    ///
    /// From `Paused` → `Playing`; from `Loading` it records intent (autoplay);
    /// from `Ended` it restarts from the beginning. Refused from `Idle`/`Stopped`
    /// (nothing is loaded to play — `load` first).
    ///
    /// # Errors
    /// [`PlayerError::InvalidState`] from `Idle`/`Stopped`; [`PlayerError::Engine`]
    /// if the engine rejects the unpause/seek.
    pub fn play(&mut self) -> Result<(), PlayerError> {
        match self.state {
            PlayerState::Idle | PlayerState::Stopped => Err(PlayerError::InvalidState {
                op: "play",
                state: self.state,
            }),
            PlayerState::Ended => {
                // Restart the still-loaded file from the top.
                self.engine.seek_absolute(0.0)?;
                self.engine.set_paused(false)?;
                self.paused_intent = false;
                self.set_position(0.0);
                self.set_state(PlayerState::Playing);
                Ok(())
            }
            PlayerState::Loading => {
                self.paused_intent = false;
                self.engine.set_paused(false)?;
                Ok(())
            }
            PlayerState::Playing => Ok(()), // already playing — idempotent
            PlayerState::Paused => {
                self.engine.set_paused(false)?;
                self.paused_intent = false;
                self.set_state(PlayerState::Playing);
                Ok(())
            }
        }
    }

    /// Pause playback (holding position).
    ///
    /// From `Playing` → `Paused`; from `Loading` it records intent so the file
    /// opens paused. Refused from `Idle`/`Stopped`.
    ///
    /// # Errors
    /// [`PlayerError::InvalidState`] from `Idle`/`Stopped`; [`PlayerError::Engine`]
    /// if the engine rejects the pause.
    pub fn pause(&mut self) -> Result<(), PlayerError> {
        match self.state {
            PlayerState::Idle | PlayerState::Stopped => Err(PlayerError::InvalidState {
                op: "pause",
                state: self.state,
            }),
            PlayerState::Paused | PlayerState::Ended => {
                self.paused_intent = true;
                Ok(())
            }
            PlayerState::Loading => {
                self.paused_intent = true;
                self.engine.set_paused(true)?;
                Ok(())
            }
            PlayerState::Playing => {
                self.engine.set_paused(true)?;
                self.paused_intent = true;
                self.set_state(PlayerState::Paused);
                Ok(())
            }
        }
    }

    /// Toggle between play and pause.
    ///
    /// # Errors
    /// Propagates [`play`](Self::play) / [`pause`](Self::pause) errors.
    pub fn toggle_pause(&mut self) -> Result<(), PlayerError> {
        match self.state {
            PlayerState::Playing => self.pause(),
            _ => self.play(),
        }
    }

    /// Seek to an absolute `position_secs` (clamped to `[0, duration]`).
    ///
    /// Valid while media is playable (`Playing`/`Paused`/`Ended`); a seek out of
    /// `Ended` lands the player `Paused` at the target. Refused from
    /// `Idle`/`Loading`/`Stopped`.
    ///
    /// # Errors
    /// [`PlayerError::InvalidState`] when not playable; [`PlayerError::Engine`] if
    /// the engine rejects the seek.
    pub fn seek(&mut self, position_secs: f64) -> Result<(), PlayerError> {
        match self.state {
            PlayerState::Playing | PlayerState::Paused | PlayerState::Ended => {
                let target = self.clamp_position(position_secs);
                self.engine.seek_absolute(target)?;
                self.set_position(target);
                if self.state == PlayerState::Ended {
                    // Left EOF — settle to Paused at the new position.
                    self.paused_intent = true;
                    self.set_state(PlayerState::Paused);
                }
                Ok(())
            }
            PlayerState::Idle | PlayerState::Loading | PlayerState::Stopped => {
                Err(PlayerError::InvalidState {
                    op: "seek",
                    state: self.state,
                })
            }
        }
    }

    /// Stop playback and unload the file.
    ///
    /// Valid from any loaded state → [`PlayerState::Stopped`]. Refused from
    /// `Idle` (nothing to stop).
    ///
    /// # Errors
    /// [`PlayerError::InvalidState`] from `Idle`; [`PlayerError::Engine`] if the
    /// engine rejects the stop.
    pub fn stop(&mut self) -> Result<(), PlayerError> {
        if self.state == PlayerState::Idle {
            return Err(PlayerError::InvalidState {
                op: "stop",
                state: self.state,
            });
        }
        self.engine.stop()?;
        self.media = None;
        self.tracks.clear();
        self.set_position(0.0);
        self.duration = None;
        self.set_state(PlayerState::Stopped);
        Ok(())
    }

    // ── audio (MEDIA-3) ──────────────────────────────────────────────────────

    /// Apply an audio-processing [`AudioConfig`] to the engine — the `PipeWire` ao,
    /// the EQ + loudness `af` graph, and the `ReplayGain` + gapless properties —
    /// and record it as the current config on success.
    ///
    /// Valid in any state (these are global mpv properties that persist across
    /// loads); the config is left unchanged if the engine rejects it.
    ///
    /// # Errors
    /// Returns [`PlayerError::Engine`] if the engine rejects a property set; the
    /// stored config is then untouched.
    pub fn set_audio_config(&mut self, config: AudioConfig) -> Result<(), PlayerError> {
        self.engine.apply_audio_config(&config)?;
        self.audio = config;
        Ok(())
    }

    // ── video (MEDIA-4) ──────────────────────────────────────────────────────

    /// Apply a video decode + adjustments [`VideoConfig`] to the engine — the
    /// `hwdec` decode mode (VA-API with software fallback), the aspect / zoom /
    /// pan / crop / rotate / deinterlace `video-*` properties, and the `vf` filter
    /// graph — and record it as the current config on success.
    ///
    /// Valid in any state (these are global mpv properties that persist across
    /// loads); the config is left unchanged if the engine rejects it.
    ///
    /// # Errors
    /// Returns [`PlayerError::Engine`] if the engine rejects a property set; the
    /// stored config is then untouched.
    pub fn set_video_config(&mut self, config: VideoConfig) -> Result<(), PlayerError> {
        self.engine.apply_video_config(&config)?;
        self.video = config;
        Ok(())
    }

    // ── the tick ─────────────────────────────────────────────────────────────

    /// Fold one tick of engine activity into the state machine.
    ///
    /// Call this each frame (or on the engine's wakeup): it drains
    /// [`EngineSignal`]s (advancing the state machine + emitting the matching
    /// [`PlayerEvent`]s) and refreshes the live position/duration while playing.
    /// Cheap when nothing changed.
    pub fn pump(&mut self) {
        for signal in self.engine.poll() {
            self.apply_signal(signal);
        }
        // While a file is open, track the engine's live clock.
        if matches!(self.state, PlayerState::Playing | PlayerState::Paused) {
            if let Some(pos) = self.engine.position() {
                if (pos - self.position).abs() > f64::EPSILON {
                    self.set_position(pos);
                }
            }
            if self.duration.is_none() {
                if let Some(dur) = self.engine.duration() {
                    self.set_duration(dur);
                }
            }
        }
    }

    fn apply_signal(&mut self, signal: EngineSignal) {
        match signal {
            EngineSignal::FileLoaded => {
                if self.state == PlayerState::Loading {
                    if let Some(dur) = self.engine.duration() {
                        self.set_duration(dur);
                    }
                    let tracks = self.engine.tracks();
                    self.tracks.clone_from(&tracks);
                    self.events.push_back(PlayerEvent::TracksChanged(tracks));
                    let next = if self.paused_intent {
                        PlayerState::Paused
                    } else {
                        PlayerState::Playing
                    };
                    self.set_state(next);
                }
            }
            EngineSignal::EndFile(EndReason::Eof) => {
                if self.state.has_media() {
                    self.set_state(PlayerState::Ended);
                    self.events.push_back(PlayerEvent::EndReached);
                }
            }
            EngineSignal::EndFile(EndReason::Stopped) => {
                // Our own stop() already transitioned; only react to an
                // engine-originated stop that we did not drive.
                if self.state != PlayerState::Stopped && self.state != PlayerState::Idle {
                    self.set_state(PlayerState::Stopped);
                }
            }
            EngineSignal::EndFile(EndReason::Error) => {
                self.events
                    .push_back(PlayerEvent::Error("playback ended on error".into()));
                if self.state != PlayerState::Stopped {
                    self.set_state(PlayerState::Stopped);
                }
            }
            EngineSignal::Error(msg) => {
                self.events.push_back(PlayerEvent::Error(msg));
            }
        }
    }

    /// Drain all pending observable events since the last drain.
    #[must_use]
    pub fn drain_events(&mut self) -> Vec<PlayerEvent> {
        self.events.drain(..).collect()
    }

    // ── internals ────────────────────────────────────────────────────────────

    fn set_state(&mut self, next: PlayerState) {
        if self.state != next {
            self.state = next;
            self.events.push_back(PlayerEvent::StateChanged(next));
        }
    }

    fn set_position(&mut self, pos: f64) {
        self.position = pos;
        self.events.push_back(PlayerEvent::PositionChanged(pos));
    }

    fn set_duration(&mut self, dur: f64) {
        self.duration = Some(dur);
        self.events.push_back(PlayerEvent::DurationChanged(dur));
    }

    fn clamp_position(&self, pos: f64) -> f64 {
        let lo = pos.max(0.0);
        match self.duration {
            Some(dur) if dur > 0.0 => lo.min(dur),
            _ => lo,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::TrackKind;
    use crate::fake::FakeMpv;

    fn sample_tracks() -> Vec<Track> {
        vec![
            Track {
                id: 1,
                kind: TrackKind::Video,
                title: Some("Main".into()),
                lang: None,
                codec: Some("h264".into()),
                default: true,
                selected: true,
            },
            Track {
                id: 1,
                kind: TrackKind::Audio,
                title: None,
                lang: Some("eng".into()),
                codec: Some("aac".into()),
                default: true,
                selected: true,
            },
            Track {
                id: 1,
                kind: TrackKind::Subtitle,
                title: None,
                lang: Some("eng".into()),
                codec: Some("ass".into()),
                default: false,
                selected: false,
            },
        ]
    }

    fn player() -> Player<FakeMpv> {
        Player::new(
            FakeMpv::new()
                .with_duration(120.0)
                .with_tracks(sample_tracks()),
        )
    }

    /// Every state-changing action emits a `StateChanged`, so `drain_events`
    /// after each step reflects the transition list.
    fn states(events: &[PlayerEvent]) -> Vec<PlayerState> {
        events
            .iter()
            .filter_map(|e| match e {
                PlayerEvent::StateChanged(s) => Some(*s),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn starts_idle() {
        let p = player();
        assert_eq!(p.state(), PlayerState::Idle);
        assert!(p.position().abs() < f64::EPSILON);
        assert_eq!(p.duration(), None);
        assert!(p.tracks().is_empty());
        assert_eq!(p.media(), None);
    }

    #[test]
    fn load_goes_loading_then_playing_on_pump() {
        let mut p = player();
        p.load("test://clip.mkv").expect("load");
        assert_eq!(p.state(), PlayerState::Loading);
        assert_eq!(p.media(), Some("test://clip.mkv"));
        assert_eq!(states(&p.drain_events()), vec![PlayerState::Loading]);

        p.pump(); // engine delivered FileLoaded
        assert_eq!(p.state(), PlayerState::Playing);
        assert_eq!(p.duration(), Some(120.0));
        assert_eq!(p.tracks().len(), 3);

        let ev = p.drain_events();
        // duration + tracks known, then Playing.
        assert!(ev.contains(&PlayerEvent::DurationChanged(120.0)));
        assert!(ev
            .iter()
            .any(|e| matches!(e, PlayerEvent::TracksChanged(t) if t.len() == 3)));
        assert_eq!(states(&ev), vec![PlayerState::Playing]);
    }

    #[test]
    fn load_paused_intent_opens_paused() {
        let mut p = player();
        p.load("x").expect("load");
        p.pause().expect("pause while loading records intent");
        assert_eq!(p.state(), PlayerState::Loading); // not applied until ready
        p.pump();
        assert_eq!(p.state(), PlayerState::Paused);
    }

    #[test]
    fn play_pause_toggle_cycle() {
        let mut p = player();
        p.load("x").expect("load");
        p.pump();
        assert_eq!(p.state(), PlayerState::Playing);

        p.pause().expect("pause");
        assert_eq!(p.state(), PlayerState::Paused);
        assert!(p.engine().is_paused());

        p.play().expect("play");
        assert_eq!(p.state(), PlayerState::Playing);
        assert!(!p.engine().is_paused());

        p.toggle_pause().expect("toggle→pause");
        assert_eq!(p.state(), PlayerState::Paused);
        p.toggle_pause().expect("toggle→play");
        assert_eq!(p.state(), PlayerState::Playing);
    }

    #[test]
    fn position_tracks_engine_clock_while_playing() {
        let mut p = player();
        p.load("x").expect("load");
        p.pump();
        p.engine_mut().advance(30.0);
        p.pump();
        assert!((p.position() - 30.0).abs() < f64::EPSILON);
        let ev = p.drain_events();
        assert!(ev.contains(&PlayerEvent::PositionChanged(30.0)));
    }

    #[test]
    fn seek_clamps_and_moves() {
        let mut p = player();
        p.load("x").expect("load");
        p.pump();
        p.seek(45.0).expect("seek");
        assert!((p.position() - 45.0).abs() < f64::EPSILON);
        // Clamp above duration.
        p.seek(999.0).expect("seek past end");
        assert!((p.position() - 120.0).abs() < f64::EPSILON);
        // Clamp below zero.
        p.seek(-10.0).expect("seek before start");
        assert!(p.position().abs() < f64::EPSILON);
    }

    #[test]
    fn eof_reaches_ended_and_replay_restarts() {
        let mut p = player();
        p.load("x").expect("load");
        p.pump();
        p.engine_mut().advance(120.0);
        p.engine_mut().reach_eof();
        p.pump();
        assert_eq!(p.state(), PlayerState::Ended);
        assert!(p.drain_events().contains(&PlayerEvent::EndReached));

        // Replay from Ended.
        p.play().expect("replay");
        assert_eq!(p.state(), PlayerState::Playing);
        assert!(p.position().abs() < f64::EPSILON);
    }

    #[test]
    fn seek_out_of_ended_lands_paused() {
        let mut p = player();
        p.load("x").expect("load");
        p.pump();
        p.engine_mut().reach_eof();
        p.pump();
        assert_eq!(p.state(), PlayerState::Ended);
        p.seek(10.0).expect("seek from ended");
        assert_eq!(p.state(), PlayerState::Paused);
        assert!((p.position() - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn stop_unloads_and_forbids_replay() {
        let mut p = player();
        p.load("x").expect("load");
        p.pump();
        p.stop().expect("stop");
        assert_eq!(p.state(), PlayerState::Stopped);
        assert_eq!(p.media(), None);
        assert!(p.tracks().is_empty());
        assert_eq!(p.duration(), None);

        // Play/pause/seek are refused after stop — must load again.
        assert!(matches!(
            p.play(),
            Err(PlayerError::InvalidState { op: "play", .. })
        ));
        assert!(matches!(
            p.seek(1.0),
            Err(PlayerError::InvalidState { op: "seek", .. })
        ));
    }

    #[test]
    fn transport_refused_while_idle() {
        let mut p = player();
        assert!(matches!(
            p.play(),
            Err(PlayerError::InvalidState {
                op: "play",
                state: PlayerState::Idle
            })
        ));
        assert!(matches!(
            p.pause(),
            Err(PlayerError::InvalidState { op: "pause", .. })
        ));
        assert!(matches!(
            p.seek(1.0),
            Err(PlayerError::InvalidState { op: "seek", .. })
        ));
        assert!(matches!(
            p.stop(),
            Err(PlayerError::InvalidState { op: "stop", .. })
        ));
    }

    #[test]
    fn seek_refused_while_loading() {
        let mut p = player();
        p.load("x").expect("load");
        assert_eq!(p.state(), PlayerState::Loading);
        assert!(matches!(
            p.seek(5.0),
            Err(PlayerError::InvalidState {
                op: "seek",
                state: PlayerState::Loading
            })
        ));
    }

    #[test]
    fn load_failure_surfaces_engine_error_and_stays_idle() {
        let mut p = Player::new(FakeMpv::new().failing_load());
        let err = p.load("bad://url").expect_err("load must fail");
        assert!(matches!(err, PlayerError::Engine(EngineError::Backend(_))));
        assert_eq!(p.state(), PlayerState::Idle);
        assert_eq!(p.media(), None);
    }

    #[test]
    fn load_replaces_current_media_from_any_state() {
        let mut p = player();
        p.load("first").expect("load");
        p.pump();
        p.pause().expect("pause");
        assert_eq!(p.state(), PlayerState::Paused);
        // Load a new title from Paused.
        p.load("second").expect("reload");
        assert_eq!(p.state(), PlayerState::Loading);
        assert_eq!(p.media(), Some("second"));
        assert!(p.position().abs() < f64::EPSILON);
        p.pump();
        assert_eq!(p.state(), PlayerState::Playing);
    }

    #[test]
    fn playback_error_end_stops_and_surfaces() {
        let mut p = player();
        p.load("x").expect("load");
        p.pump();
        let _ = p.drain_events(); // discard the load→play transitions
        p.engine_mut().fail_playback();
        p.pump();
        assert_eq!(p.state(), PlayerState::Stopped);
        let ev = p.drain_events();
        assert!(ev.iter().any(|e| matches!(e, PlayerEvent::Error(_))));
        assert_eq!(states(&ev), vec![PlayerState::Stopped]);
    }

    #[test]
    fn engine_originated_error_surfaces_and_stops() {
        let mut p = player();
        p.load("x").expect("load");
        p.pump();
        p.engine_mut()
            .push_signal(EngineSignal::Error("decode blew up".into()));
        p.pump();
        let ev = p.drain_events();
        assert!(ev
            .iter()
            .any(|e| matches!(e, PlayerEvent::Error(m) if m == "decode blew up")));
    }

    #[test]
    fn default_audio_config_is_pipewire() {
        use crate::audio::AudioDriver;
        let p = player();
        assert_eq!(p.audio_config().output.driver, AudioDriver::PipeWire);
        assert!(p.audio_config().eq.is_empty());
    }

    #[test]
    fn set_audio_config_applies_fold_to_engine_and_stores_it() {
        use crate::audio::{AudioConfig, EqBand, LoudnessNorm, ReplayGainMode};

        let mut p = player();
        p.load("x").expect("load");
        p.pump();

        let cfg = AudioConfig {
            eq: EqBand::iso_10_band([1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.0]),
            loudness: LoudnessNorm::Ebu {
                target_lufs: -16.0,
                true_peak_db: -1.5,
                range_lu: 11.0,
            },
            replaygain: ReplayGainMode::Album,
            gapless: true,
            ..AudioConfig::new()
        };
        p.set_audio_config(cfg.clone()).expect("apply audio config");

        // The engine received exactly the folded af graph + properties.
        assert_eq!(p.engine().applied_af(), Some(cfg.af_graph().as_str()));
        assert_eq!(p.engine().applied_properties(), cfg.properties().as_slice());
        // The graph ends with the loudness filter after the 10 EQ bands.
        assert!(p
            .engine()
            .applied_af()
            .unwrap()
            .ends_with("loudnorm=I=-16:TP=-1.5:LRA=11"));
        // And the player stored it.
        assert_eq!(p.audio_config(), &cfg);
    }

    #[test]
    fn set_audio_config_error_leaves_stored_config_untouched() {
        use crate::audio::{AudioConfig, ReplayGainMode};

        let mut p = Player::new(FakeMpv::new().failing_audio());
        let before = p.audio_config().clone();
        let err = p
            .set_audio_config(AudioConfig {
                replaygain: ReplayGainMode::Track,
                ..AudioConfig::new()
            })
            .expect_err("engine rejects audio config");
        assert!(matches!(err, PlayerError::Engine(EngineError::Backend(_))));
        // Rejected → the stored config is unchanged (still the default).
        assert_eq!(p.audio_config(), &before);
    }

    #[test]
    fn default_video_config_requests_auto_safe_hwdec() {
        use crate::video::HwDecode;
        let p = player();
        assert_eq!(p.video_config().hwdec, HwDecode::Auto);
        assert!(p.video_config().filters.is_empty());
    }

    #[test]
    fn set_video_config_applies_fold_to_engine_and_stores_it() {
        use crate::video::{
            AspectRatio, Crop, Deinterlace, HwDecode, Rotation, VideoConfig, VideoFilter,
        };

        let mut p = player();
        p.load("x").expect("load");
        p.pump();

        let cfg = VideoConfig {
            hwdec: HwDecode::VaApi,
            aspect: AspectRatio::SIXTEEN_NINE,
            crop: Some(Crop::new(1280, 536, 0, 92)),
            rotate: Rotation::Cw90,
            deinterlace: Deinterlace::On,
            filters: vec![VideoFilter::bare("hqdn3d".to_owned())],
            ..VideoConfig::new()
        };
        p.set_video_config(cfg.clone()).expect("apply video config");

        // The engine received exactly the folded vf graph + properties.
        assert_eq!(p.engine().applied_vf(), Some(cfg.vf_graph().as_str()));
        assert_eq!(
            p.engine().applied_video_properties(),
            cfg.properties().as_slice()
        );
        // The VA-API decode mode reached the engine verbatim.
        assert!(p
            .engine()
            .applied_video_properties()
            .contains(&("hwdec".to_owned(), "vaapi".to_owned())));
        // And the player stored it.
        assert_eq!(p.video_config(), &cfg);
    }

    #[test]
    fn set_video_config_error_leaves_stored_config_untouched() {
        use crate::video::{HwDecode, VideoConfig};

        let mut p = Player::new(FakeMpv::new().failing_video());
        let before = p.video_config().clone();
        let err = p
            .set_video_config(VideoConfig {
                hwdec: HwDecode::VaApi,
                ..VideoConfig::new()
            })
            .expect_err("engine rejects video config");
        assert!(matches!(err, PlayerError::Engine(EngineError::Backend(_))));
        // Rejected → the stored config is unchanged (still the default).
        assert_eq!(p.video_config(), &before);
    }
}
