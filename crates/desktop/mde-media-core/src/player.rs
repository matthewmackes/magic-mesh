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
use crate::controls::{PlaybackControls, ScreenshotMode};
use crate::engine::{EndReason, EngineError, EngineSignal, MediaEngine, Track, TrackKind};
use crate::playlist::Playlist;
use crate::resume::ResumeState;
use crate::subtitle::{track_by_language, SubtitleConfig, TrackSelect, TrackSelection};
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
    /// The queue advanced (via [`play_next`](Player::play_next) /
    /// [`play_prev`](Player::play_prev), or an auto-advance on end-of-file) to the
    /// playlist item at this index — the surface highlights the now-playing row.
    PlaylistAdvanced(usize),
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
    /// The active audio/video/subtitle track selection (MEDIA-5). Set via
    /// [`set_track_selection`](Player::set_track_selection); the `aid`/`vid`/`sid`
    /// ids are per-file, so a fresh `load` resets it to [`TrackSelection::new`].
    tracks_selection: TrackSelection,
    /// The applied subtitle config (MEDIA-5). Set via
    /// [`set_subtitle_config`](Player::set_subtitle_config); the `sub-*` styling
    /// persists across loads but the loaded external files are per-session.
    subtitle: SubtitleConfig,
    /// The applied advanced-playback controls (MEDIA-6). Set via
    /// [`set_controls`](Player::set_controls); the `speed`/`audio-delay`/`ab-loop`
    /// properties are global mpv state, so it is apply-on-change.
    controls: PlaybackControls,
    /// The playback queue (MEDIA-6). The player loads from it via
    /// [`play_next`](Player::play_next)/[`play_prev`](Player::play_prev) and
    /// auto-advances it on end-of-file per its [`RepeatMode`](crate::RepeatMode).
    playlist: Playlist,
    /// The resume / watch-history store (MEDIA-7). The player resumes from the stored
    /// position when a title finishes loading, updates it on seek / stop, marks a
    /// title completed at its natural end, and counts a play on each load. Empty by
    /// default (so a player with no history behaves exactly as before).
    resume: ResumeState,
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
            tracks_selection: TrackSelection::new(),
            subtitle: SubtitleConfig::new(),
            controls: PlaybackControls::new(),
            playlist: Playlist::new(),
            resume: ResumeState::new(),
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

    /// The active audio/video/subtitle track selection (MEDIA-5).
    #[must_use]
    pub const fn track_selection(&self) -> &TrackSelection {
        &self.tracks_selection
    }

    /// The applied subtitle config (MEDIA-5).
    #[must_use]
    pub const fn subtitle_config(&self) -> &SubtitleConfig {
        &self.subtitle
    }

    /// The applied advanced-playback controls (MEDIA-6).
    #[must_use]
    pub const fn controls(&self) -> &PlaybackControls {
        &self.controls
    }

    /// The playback queue (MEDIA-6).
    #[must_use]
    pub const fn playlist(&self) -> &Playlist {
        &self.playlist
    }

    /// Mutably borrow the playback queue to enqueue/dequeue/reorder/shuffle it.
    pub const fn playlist_mut(&mut self) -> &mut Playlist {
        &mut self.playlist
    }

    /// Replace the whole playback queue (MEDIA-6) — e.g. after a
    /// [`Playlist::load`](crate::Playlist::load).
    pub fn set_playlist(&mut self, playlist: Playlist) {
        self.playlist = playlist;
    }

    /// The resume / watch-history store (MEDIA-7) — read
    /// [`continue_watching`](crate::ResumeState::continue_watching) /
    /// [`recents`](crate::ResumeState::recents) /
    /// [`most_played`](crate::ResumeState::most_played) from it.
    #[must_use]
    pub const fn resume_state(&self) -> &ResumeState {
        &self.resume
    }

    /// Mutably borrow the resume store (e.g. to
    /// [`forget`](crate::ResumeState::forget) an item).
    pub const fn resume_state_mut(&mut self) -> &mut ResumeState {
        &mut self.resume
    }

    /// Replace the whole resume store (MEDIA-7) — e.g. after a
    /// [`ResumeState::load`](crate::ResumeState::load) at startup so playback resumes
    /// across sessions.
    pub fn set_resume_state(&mut self, resume: ResumeState) {
        self.resume = resume;
    }

    /// Checkpoint the current playback position into the resume store on demand — a
    /// surface calls this periodically (e.g. every few seconds) so a crash still
    /// leaves a resume point. A no-op when nothing is loaded.
    pub fn checkpoint_resume(&mut self) {
        if let Some(media) = self.media.clone() {
            self.resume
                .record_position(&media, self.position, self.duration);
        }
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
        // The `aid`/`vid`/`sid` ids are per-file — a new title enumerates fresh
        // tracks, so the selection returns to mpv's automatic choice.
        self.tracks_selection = TrackSelection::new();
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
                // Remember where the user seeked to (MEDIA-7 resume).
                if let Some(media) = self.media.clone() {
                    self.resume.record_position(&media, target, self.duration);
                }
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
        // Remember where playback was stopped (MEDIA-7 resume) before unloading.
        let resume_key = self.media.clone();
        let (resume_pos, resume_dur) = (self.position, self.duration);
        self.engine.stop()?;
        if let Some(key) = resume_key {
            self.resume.record_position(&key, resume_pos, resume_dur);
        }
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

    // ── subtitles + multi-track (MEDIA-5) ────────────────────────────────────

    /// Apply a [`TrackSelection`] — the `aid`/`vid`/`sid` ids selecting one
    /// enumerated [`Track`] per kind — and record it on success.
    ///
    /// Valid in any state; the selection is left unchanged if the engine rejects
    /// it. The ids reference the current [`tracks`](Self::tracks) (MEDIA-1), so a
    /// selection is meaningful only once media is loaded.
    ///
    /// # Errors
    /// Returns [`PlayerError::Engine`] if the engine rejects a property set; the
    /// stored selection is then untouched.
    pub fn set_track_selection(&mut self, selection: TrackSelection) -> Result<(), PlayerError> {
        self.engine.apply_track_selection(&selection)?;
        self.tracks_selection = selection;
        Ok(())
    }

    /// Select the track of `kind` whose language label matches `lang` (via
    /// [`track_by_language`] over the loaded [`tracks`](Self::tracks)), applying
    /// the updated [`TrackSelection`]. Returns `true` when a matching track was
    /// found + selected, `false` when none matched (the selection is left
    /// unchanged) — the "select by language label" acceptance, tied to the real
    /// enumerated tracks.
    ///
    /// # Errors
    /// Returns [`PlayerError::Engine`] if the engine rejects the property set.
    pub fn select_track_by_language(
        &mut self,
        kind: TrackKind,
        lang: &str,
    ) -> Result<bool, PlayerError> {
        let Some(id) = track_by_language(&self.tracks, kind, lang) else {
            return Ok(false);
        };
        let mut selection = self.tracks_selection.clone();
        match kind {
            TrackKind::Audio => selection.audio = TrackSelect::Id(id),
            TrackKind::Video => selection.video = TrackSelect::Id(id),
            TrackKind::Subtitle => selection.subtitle = TrackSelect::Id(id),
        }
        self.set_track_selection(selection)?;
        Ok(true)
    }

    /// Apply a [`SubtitleConfig`] — load its external `.srt`/`.ass` files
    /// (`sub-add`) and set the `sub-*` styling/position/delay properties — and
    /// record it on success.
    ///
    /// Valid in any state; the config is left unchanged if the engine rejects it.
    /// mpv rejects `sub-add` with nothing loaded, so loading external subtitles is
    /// meaningful only once media is open.
    ///
    /// # Errors
    /// Returns [`PlayerError::Engine`] if the engine rejects a command or property
    /// set; the stored config is then untouched.
    pub fn set_subtitle_config(&mut self, config: SubtitleConfig) -> Result<(), PlayerError> {
        self.engine.apply_subtitle_config(&config)?;
        self.subtitle = config;
        Ok(())
    }

    // ── advanced controls (MEDIA-6) ──────────────────────────────────────────

    /// Apply a [`PlaybackControls`] — the `speed`, `audio-delay` A/V-sync offset,
    /// `prefetch-playlist` gapless flag, and A-B loop — and record it on success.
    ///
    /// Valid in any state (these are global mpv properties that persist across
    /// loads); the controls are left unchanged if the engine rejects them.
    ///
    /// # Errors
    /// Returns [`PlayerError::Engine`] if the engine rejects a property set; the
    /// stored controls are then untouched.
    pub fn set_controls(&mut self, controls: PlaybackControls) -> Result<(), PlayerError> {
        self.engine.apply_playback_controls(&controls)?;
        self.controls = controls;
        Ok(())
    }

    /// Step one frame forward (mpv `frame-step`). Frame-stepping needs a decoded
    /// frame, so it is valid only while media is playable (`Playing`/`Paused`/
    /// `Ended`) — typically while paused.
    ///
    /// # Errors
    /// [`PlayerError::InvalidState`] when no frame is available;
    /// [`PlayerError::Engine`] if the engine rejects the command.
    pub fn frame_step(&mut self) -> Result<(), PlayerError> {
        self.require_playable("frame-step")?;
        self.engine.frame_step(true)?;
        Ok(())
    }

    /// Step one frame backward (mpv `frame-back-step`). See [`frame_step`](Self::frame_step).
    ///
    /// # Errors
    /// [`PlayerError::InvalidState`] when no frame is available;
    /// [`PlayerError::Engine`] if the engine rejects the command.
    pub fn frame_back_step(&mut self) -> Result<(), PlayerError> {
        self.require_playable("frame-back-step")?;
        self.engine.frame_step(false)?;
        Ok(())
    }

    /// Take a snapshot of the current frame (mpv `screenshot`) in the given
    /// [`ScreenshotMode`]. Valid only while media is playable.
    ///
    /// # Errors
    /// [`PlayerError::InvalidState`] when no frame is available;
    /// [`PlayerError::Engine`] if the engine rejects the command.
    pub fn snapshot(&mut self, mode: ScreenshotMode) -> Result<(), PlayerError> {
        self.require_playable("snapshot")?;
        self.engine.screenshot(mode)?;
        Ok(())
    }

    /// The current chapter index (0-based), if the media is chaptered.
    #[must_use]
    pub fn chapter(&self) -> Option<i64> {
        self.engine.chapter()
    }

    /// The number of chapters in the loaded media, if known.
    #[must_use]
    pub fn chapter_count(&self) -> Option<i64> {
        self.engine.chapter_count()
    }

    /// Seek to chapter `chapter` (clamped to the media's chapter range). Valid only
    /// while media is playable.
    ///
    /// # Errors
    /// [`PlayerError::InvalidState`] when nothing is playable;
    /// [`PlayerError::Engine`] if the engine rejects the seek.
    pub fn set_chapter(&mut self, chapter: i64) -> Result<(), PlayerError> {
        self.require_playable("set-chapter")?;
        let target = self.clamp_chapter(chapter);
        self.engine.set_chapter(target)?;
        Ok(())
    }

    /// Jump to the next chapter (clamped to the last). Valid only while playable.
    ///
    /// # Errors
    /// Propagates [`set_chapter`](Self::set_chapter)'s errors.
    pub fn chapter_next(&mut self) -> Result<(), PlayerError> {
        let current = self.engine.chapter().unwrap_or(0);
        self.set_chapter(current + 1)
    }

    /// Jump to the previous chapter (clamped to the first). Valid only while
    /// playable.
    ///
    /// # Errors
    /// Propagates [`set_chapter`](Self::set_chapter)'s errors.
    pub fn chapter_prev(&mut self) -> Result<(), PlayerError> {
        let current = self.engine.chapter().unwrap_or(0);
        self.set_chapter(current - 1)
    }

    // ── playlist / queue (MEDIA-6) ───────────────────────────────────────────

    /// Advance the queue and load its next item, per the playlist's
    /// [`RepeatMode`](crate::RepeatMode) + shuffle order. Returns the new current
    /// item index, or [`None`] when there is no next item (empty queue, or the end
    /// of a non-repeating queue) — the player is then left as-is.
    ///
    /// # Errors
    /// Returns [`PlayerError::Engine`] if loading the next item fails.
    pub fn play_next(&mut self) -> Result<Option<usize>, PlayerError> {
        self.advance_playlist(true)
    }

    /// Step the queue back and load its previous item, per the playlist's
    /// [`RepeatMode`](crate::RepeatMode) + shuffle order. Returns the new current
    /// item index, or [`None`] when there is no previous item.
    ///
    /// # Errors
    /// Returns [`PlayerError::Engine`] if loading the previous item fails.
    pub fn play_prev(&mut self) -> Result<Option<usize>, PlayerError> {
        self.advance_playlist(false)
    }

    /// Move the queue cursor forward/back and load the resulting item, emitting a
    /// [`PlayerEvent::PlaylistAdvanced`] on success.
    fn advance_playlist(&mut self, forward: bool) -> Result<Option<usize>, PlayerError> {
        let next_url = if forward {
            self.playlist.next_item()
        } else {
            self.playlist.prev_item()
        }
        .map(|item| item.url.clone());
        let Some(url) = next_url else {
            return Ok(None);
        };
        let idx = self.playlist.current_index();
        self.load(url)?;
        if let Some(i) = idx {
            self.events.push_back(PlayerEvent::PlaylistAdvanced(i));
        }
        Ok(idx)
    }

    /// End-of-file queue advance: load the next queued item (per repeat/shuffle) if
    /// there is one. Returns whether the player auto-advanced (so the caller knows
    /// whether to settle into [`PlayerState::Ended`] instead). A load failure is
    /// surfaced as a [`PlayerEvent::Error`] and treated as "no advance".
    fn try_advance_on_eof(&mut self) -> bool {
        let Some(url) = self.playlist.next_item().map(|item| item.url.clone()) else {
            return false;
        };
        let idx = self.playlist.current_index();
        if let Err(err) = self.load(url) {
            self.events.push_back(PlayerEvent::Error(err.to_string()));
            return false;
        }
        if let Some(i) = idx {
            self.events.push_back(PlayerEvent::PlaylistAdvanced(i));
        }
        true
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
                    // MEDIA-7: count a play, and resume from the stored position when
                    // one exists (empty history → a no-op, so the load is unchanged).
                    if let Some(media) = self.media.clone() {
                        self.resume.mark_started(&media);
                        if let Some(pos) = self.resume.resume_position(&media) {
                            let target = self.clamp_position(pos);
                            if self.engine.seek_absolute(target).is_ok() {
                                self.set_position(target);
                            }
                        }
                    }
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
                    // MEDIA-7: the item played to its natural end → mark it completed
                    // so it starts over (not resumes) next time (continue-watching).
                    if let Some(finished) = self.media.clone() {
                        self.resume.mark_completed(&finished, self.duration);
                    }
                    // A queued next item (per the playlist's repeat/shuffle)
                    // auto-advances the player; an empty/exhausted queue ends it.
                    if !self.try_advance_on_eof() {
                        self.set_state(PlayerState::Ended);
                        self.events.push_back(PlayerEvent::EndReached);
                    }
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

    /// Refuse a control that needs a decoded frame (`frame-step`, `snapshot`,
    /// chapter nav) unless the media is playable (`Playing`/`Paused`/`Ended`).
    const fn require_playable(&self, op: &'static str) -> Result<(), PlayerError> {
        match self.state {
            PlayerState::Playing | PlayerState::Paused | PlayerState::Ended => Ok(()),
            state => Err(PlayerError::InvalidState { op, state }),
        }
    }

    /// Clamp a chapter index to `[0, chapter_count)` when the count is known.
    fn clamp_chapter(&self, chapter: i64) -> i64 {
        let lo = chapter.max(0);
        match self.engine.chapter_count() {
            Some(count) if count > 0 => lo.min(count - 1),
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

    // ── subtitles + multi-track (MEDIA-5) ────────────────────────────────────

    #[test]
    fn default_track_selection_and_subtitle_config_are_auto() {
        let p = player();
        assert_eq!(p.track_selection(), &TrackSelection::new());
        assert_eq!(p.subtitle_config(), &SubtitleConfig::new());
    }

    #[test]
    fn set_track_selection_applies_fold_and_stores_it() {
        let mut p = player();
        p.load("x").expect("load");
        p.pump();

        let sel = TrackSelection {
            audio: TrackSelect::Id(1),
            video: TrackSelect::Id(1),
            subtitle: TrackSelect::Id(1),
        };
        p.set_track_selection(sel.clone()).expect("apply selection");
        assert_eq!(p.engine().applied_track_properties(), sel.properties());
        assert_eq!(p.track_selection(), &sel);
    }

    #[test]
    fn select_track_by_language_selects_enumerated_track() {
        // sample_tracks() carries an eng audio (id 1) + an eng subtitle (id 1).
        let mut p = player();
        p.load("x").expect("load");
        p.pump();

        let found = p
            .select_track_by_language(TrackKind::Subtitle, "eng")
            .expect("select");
        assert!(found, "the eng subtitle track exists");
        assert_eq!(p.track_selection().subtitle, TrackSelect::Id(1));
        // The fold reached the engine.
        assert!(p
            .engine()
            .applied_track_properties()
            .contains(&("sid".to_owned(), "1".to_owned())));

        // A language with no track leaves the selection unchanged, no error.
        let before = p.track_selection().clone();
        let missing = p
            .select_track_by_language(TrackKind::Audio, "fra")
            .expect("no-match is not an error");
        assert!(!missing);
        assert_eq!(p.track_selection(), &before);
    }

    #[test]
    fn set_subtitle_config_applies_commands_and_properties() {
        use crate::subtitle::{AssOverride, ExternalSub};

        let mut p = player();
        p.load("x").expect("load");
        p.pump();

        let cfg = SubtitleConfig {
            external: vec![ExternalSub::new("/subs/movie.eng.srt")],
            ass_override: AssOverride::Force,
            pos: 95,
            delay: 0.5,
            ..SubtitleConfig::new()
        };
        p.set_subtitle_config(cfg.clone())
            .expect("apply subtitle config");

        assert_eq!(p.engine().applied_sub_commands(), cfg.commands().as_slice());
        assert_eq!(
            p.engine().applied_subtitle_properties(),
            cfg.properties().as_slice()
        );
        assert!(p
            .engine()
            .applied_sub_commands()
            .iter()
            .any(|argv| argv.contains(&"/subs/movie.eng.srt".to_owned())));
        assert_eq!(p.subtitle_config(), &cfg);
    }

    #[test]
    fn set_subtitle_config_error_leaves_stored_config_untouched() {
        use crate::subtitle::{ExternalSub, SubtitleConfig};

        let mut p = Player::new(FakeMpv::new().failing_subtitle());
        let before = p.subtitle_config().clone();
        let err = p
            .set_subtitle_config(SubtitleConfig {
                external: vec![ExternalSub::new("x.srt")],
                ..SubtitleConfig::new()
            })
            .expect_err("engine rejects subtitle config");
        assert!(matches!(err, PlayerError::Engine(EngineError::Backend(_))));
        assert_eq!(p.subtitle_config(), &before);
    }

    #[test]
    fn set_track_selection_error_leaves_stored_selection_untouched() {
        let mut p = Player::new(FakeMpv::new().failing_tracks());
        let before = p.track_selection().clone();
        let err = p
            .set_track_selection(TrackSelection {
                audio: TrackSelect::Id(2),
                ..TrackSelection::new()
            })
            .expect_err("engine rejects track selection");
        assert!(matches!(err, PlayerError::Engine(EngineError::Backend(_))));
        assert_eq!(p.track_selection(), &before);
    }

    #[test]
    fn load_resets_track_selection_to_auto() {
        let mut p = player();
        p.load("first").expect("load");
        p.pump();
        p.set_track_selection(TrackSelection {
            audio: TrackSelect::Id(2),
            ..TrackSelection::new()
        })
        .expect("select");
        assert_eq!(p.track_selection().audio, TrackSelect::Id(2));
        // A new title enumerates fresh tracks → selection returns to auto.
        p.load("second").expect("reload");
        assert_eq!(p.track_selection(), &TrackSelection::new());
    }

    // ── advanced controls (MEDIA-6) ──────────────────────────────────────────

    #[test]
    fn default_controls_are_neutral() {
        let p = player();
        assert_eq!(p.controls(), &PlaybackControls::new());
    }

    #[test]
    fn set_controls_applies_fold_to_engine_and_stores_it() {
        use crate::controls::AbLoop;

        let mut p = player();
        p.load("x").expect("load");
        p.pump();

        let cfg = PlaybackControls {
            speed: 2.0,
            audio_delay: -0.1,
            ab_loop: AbLoop::Range { a: 5.0, b: 15.0 },
            ..PlaybackControls::new()
        };
        p.set_controls(cfg).expect("apply controls");

        assert_eq!(
            p.engine().applied_control_properties(),
            cfg.properties().as_slice()
        );
        assert!(p
            .engine()
            .applied_control_properties()
            .contains(&("speed".to_owned(), "2".to_owned())));
        assert_eq!(p.controls(), &cfg);
    }

    #[test]
    fn set_controls_error_leaves_stored_controls_untouched() {
        let mut p = Player::new(FakeMpv::new().failing_controls());
        let before = *p.controls();
        let err = p
            .set_controls(PlaybackControls {
                speed: 3.0,
                ..PlaybackControls::new()
            })
            .expect_err("engine rejects controls");
        assert!(matches!(err, PlayerError::Engine(EngineError::Backend(_))));
        assert_eq!(p.controls(), &before);
    }

    #[test]
    fn frame_step_and_snapshot_need_a_playable_frame() {
        // Refused with nothing loaded.
        let mut p = player();
        assert!(matches!(
            p.frame_step(),
            Err(PlayerError::InvalidState {
                op: "frame-step",
                ..
            })
        ));
        assert!(matches!(
            p.snapshot(ScreenshotMode::Subtitles),
            Err(PlayerError::InvalidState { op: "snapshot", .. })
        ));

        // Once paused on a loaded clip, both drive the engine.
        p.load("x").expect("load");
        p.pump();
        p.pause().expect("pause");
        p.frame_step().expect("step forward");
        p.frame_back_step().expect("step back");
        assert_eq!(p.engine().frame_steps(), &[true, false]);
        p.snapshot(ScreenshotMode::Video).expect("snapshot");
        assert_eq!(p.engine().screenshots(), &[ScreenshotMode::Video]);
    }

    #[test]
    fn chapter_navigation_reads_sets_and_clamps() {
        let mut p = Player::new(FakeMpv::new().with_duration(120.0).with_chapters(4));

        // Refused before load (no playable frame).
        assert!(matches!(
            p.set_chapter(1),
            Err(PlayerError::InvalidState {
                op: "set-chapter",
                ..
            })
        ));

        p.load("x").expect("load");
        p.pump();
        assert_eq!(p.chapter(), Some(0));
        assert_eq!(p.chapter_count(), Some(4));

        p.chapter_next().expect("next chapter");
        assert_eq!(p.chapter(), Some(1));

        // Set past the end clamps to the last chapter (count - 1).
        p.set_chapter(99).expect("set clamps high");
        assert_eq!(p.chapter(), Some(3));

        // Prev past the start clamps to 0.
        p.set_chapter(0).expect("to first");
        p.chapter_prev().expect("prev clamps low");
        assert_eq!(p.chapter(), Some(0));
    }

    // ── playlist / queue (MEDIA-6) ───────────────────────────────────────────

    fn queued_player(urls: &[&str]) -> Player<FakeMpv> {
        use crate::playlist::PlaylistItem;
        let mut p = Player::new(FakeMpv::new().with_duration(5.0));
        let items = urls.iter().map(|u| PlaylistItem::new(*u)).collect();
        p.set_playlist(Playlist::from_items(items));
        p
    }

    #[test]
    fn play_next_and_prev_load_queued_items() {
        let mut p = queued_player(&["a", "b", "c"]);

        // Fresh queue current is item 0; `next` advances to item 1 and loads it.
        let idx = p.play_next().expect("next");
        assert_eq!(idx, Some(1));
        assert_eq!(p.state(), PlayerState::Loading);
        assert_eq!(p.media(), Some("b"));
        let ev = p.drain_events();
        assert!(ev.contains(&PlayerEvent::PlaylistAdvanced(1)));
        p.pump();
        assert_eq!(p.state(), PlayerState::Playing);

        let back = p.play_prev().expect("prev");
        assert_eq!(back, Some(0));
        assert_eq!(p.media(), Some("a"));
    }

    #[test]
    fn eof_auto_advances_the_queue_and_wraps_on_repeat_all() {
        use crate::playlist::RepeatMode;

        let mut p = queued_player(&["a", "b", "c"]);
        p.playlist_mut().set_repeat(RepeatMode::All);

        p.load("a").expect("load a");
        p.pump();
        assert_eq!(p.state(), PlayerState::Playing);
        assert_eq!(p.media(), Some("a"));

        // End "a" → auto-advance loads "b" (not Ended).
        p.engine_mut().reach_eof();
        p.pump();
        assert_eq!(p.state(), PlayerState::Loading);
        assert_eq!(p.media(), Some("b"));
        assert_eq!(p.playlist().current_index(), Some(1));
        assert!(p.drain_events().contains(&PlayerEvent::PlaylistAdvanced(1)));
        p.pump();
        assert_eq!(p.state(), PlayerState::Playing);

        // End "b" → "c".
        p.engine_mut().reach_eof();
        p.pump();
        assert_eq!(p.media(), Some("c"));
        p.pump();

        // End "c" with repeat-all → wrap back to "a".
        p.engine_mut().reach_eof();
        p.pump();
        assert_eq!(p.media(), Some("a"));
        assert_eq!(p.playlist().current_index(), Some(0));
    }

    #[test]
    fn eof_ends_after_last_item_when_repeat_off() {
        let mut p = queued_player(&["a", "b"]);

        p.load("a").expect("load a");
        p.pump();
        p.engine_mut().reach_eof();
        p.pump(); // → "b"
        assert_eq!(p.media(), Some("b"));
        p.pump();

        // "b" is the last item, repeat off → the player ends (no wrap).
        p.engine_mut().reach_eof();
        p.pump();
        assert_eq!(p.state(), PlayerState::Ended);
        assert!(p.drain_events().contains(&PlayerEvent::EndReached));
    }

    #[test]
    fn eof_repeat_one_reloads_the_same_item() {
        use crate::playlist::RepeatMode;

        let mut p = queued_player(&["a", "b"]);
        p.playlist_mut().set_repeat(RepeatMode::One);

        p.load("a").expect("load a");
        p.pump();
        p.engine_mut().reach_eof();
        p.pump();
        // Repeat-one reloads "a", never advancing to "b".
        assert_eq!(p.media(), Some("a"));
        assert_eq!(p.playlist().current_index(), Some(0));
    }

    // ── local library + resume (MEDIA-7) ─────────────────────────────────────

    #[test]
    fn resume_on_load_seeks_to_stored_position() {
        let mut p = player(); // duration 120
        p.resume_state_mut()
            .record_position("movie.mkv", 45.0, Some(120.0));

        p.load("movie.mkv").expect("load");
        p.pump(); // FileLoaded → resume seek to the stored position
        assert_eq!(p.state(), PlayerState::Playing);
        assert!((p.position() - 45.0).abs() < f64::EPSILON);
        // The resume actually reached the engine as a seek.
        assert!(p
            .engine()
            .commands()
            .iter()
            .any(|c| c.starts_with("seek 45")));
    }

    #[test]
    fn empty_history_load_does_not_seek() {
        // With no stored position, a load behaves exactly as before (no resume seek).
        let mut p = player();
        p.load("fresh.mkv").expect("load");
        p.pump();
        assert!(p.position().abs() < f64::EPSILON);
        assert!(!p.engine().commands().iter().any(|c| c.starts_with("seek")));
    }

    #[test]
    fn seek_and_stop_update_the_resume_store() {
        let mut p = player();
        p.load("clip.mkv").expect("load");
        p.pump();

        p.seek(30.0).expect("seek");
        assert_eq!(p.resume_state().resume_position("clip.mkv"), Some(30.0));

        // Stop remembers where playback was left, before the file is unloaded.
        p.seek(60.0).expect("seek again");
        p.stop().expect("stop");
        assert_eq!(p.media(), None);
        assert_eq!(p.resume_state().resume_position("clip.mkv"), Some(60.0));
    }

    #[test]
    fn checkpoint_resume_records_the_live_position() {
        let mut p = player();
        p.load("c.mkv").expect("load");
        p.pump();
        p.engine_mut().advance(20.0);
        p.pump(); // live clock → position 20
        p.checkpoint_resume();
        assert_eq!(p.resume_state().resume_position("c.mkv"), Some(20.0));
    }

    #[test]
    fn natural_end_marks_completed_so_it_does_not_resume() {
        let mut p = player(); // duration 120
        p.load("ep.mkv").expect("load");
        p.pump();
        p.seek(50.0).expect("seek"); // a mid-title resume point
        assert_eq!(p.resume_state().resume_position("ep.mkv"), Some(50.0));

        p.engine_mut().reach_eof();
        p.pump();
        assert_eq!(p.state(), PlayerState::Ended);
        // Watched to the end → next time it starts over, not resumes.
        assert_eq!(p.resume_state().resume_position("ep.mkv"), None);
        assert!(p.resume_state().get("ep.mkv").expect("entry").completed);
    }

    #[test]
    fn each_load_counts_a_play_for_recents_and_most_played() {
        let mut p = player();
        for url in ["a.mkv", "b.mkv", "a.mkv"] {
            p.load(url).expect("load");
            p.pump();
        }
        // "a" played twice, "b" once.
        assert_eq!(p.resume_state().most_played(10), vec!["a.mkv", "b.mkv"]);
        // The last thing loaded leads the recents.
        assert_eq!(p.resume_state().recents(10), vec!["a.mkv", "b.mkv"]);
    }

    #[test]
    fn set_resume_state_replaces_the_store_and_resumes() {
        let mut p = player();
        let mut restored = ResumeState::new();
        restored.record_position("x.mkv", 33.0, Some(200.0));
        p.set_resume_state(restored);

        p.load("x.mkv").expect("load");
        p.pump();
        assert!((p.position() - 33.0).abs() < f64::EPSILON);
    }
}
