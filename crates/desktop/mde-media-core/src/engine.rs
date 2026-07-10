//! The engine seam — the trait the [`Player`](crate::Player) drives, plus the
//! value types that cross it.
//!
//! The whole point of MEDIA-1 is that the player-state machine is testable
//! *without a real mpv*: [`MediaEngine`] is the one narrow interface between the
//! player core and the decoder. The real implementation
//! ([`crate::mpv::MpvEngine`], feature `mpv`) translates each call into an mpv
//! command/property/event; the in-tree [`FakeMpv`](crate::FakeMpv) simulates the
//! same behaviour deterministically. §6: this is glue over mpv, not a reimplemented
//! decoder — the seam is intentionally thin (load/pause/seek/stop + observed
//! position/duration/tracks + a drained signal queue), mirroring mpv's own
//! command + property + event model.

use serde::{Deserialize, Serialize};

use crate::audio::AudioConfig;
use crate::controls::{PlaybackControls, ScreenshotMode};
use crate::subtitle::{SubtitleConfig, TrackSelection};
use crate::video::VideoConfig;

/// A failure surfaced by a [`MediaEngine`].
///
/// The real engine maps mpv's `mpv_error` codes into these; [`FakeMpv`] raises
/// them on scripted failure. They are deliberately coarse — the player core only
/// needs to distinguish "the backend rejected this" from "nothing is loaded".
///
/// [`FakeMpv`]: crate::FakeMpv
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum EngineError {
    /// The backend rejected a command/property (mpv error, malformed URL, …).
    #[error("media engine backend error: {0}")]
    Backend(String),
    /// An operation needing loaded media was issued with nothing loaded.
    #[error("no media is loaded")]
    NotLoaded,
}

/// The kind of a media track, as reported by mpv's `track-list/N/type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    /// A video track.
    Video,
    /// An audio track.
    Audio,
    /// A subtitle track.
    Subtitle,
}

impl TrackKind {
    /// Parse mpv's `type` string (`"video"`/`"audio"`/`"sub"`) into a [`TrackKind`].
    ///
    /// Returns [`None`] for any other/unknown type so the caller can skip it
    /// rather than guess.
    #[must_use]
    pub fn from_mpv(kind: &str) -> Option<Self> {
        match kind {
            "video" => Some(Self::Video),
            "audio" => Some(Self::Audio),
            "sub" | "subtitle" => Some(Self::Subtitle),
            _ => None,
        }
    }
}

/// One selectable track (video / audio / subtitle) of the loaded media.
///
/// This is the enumerated-track model MEDIA-5 (multi-track selection) builds on;
/// it derives serde because the session record (MEDIA-16 roaming) carries the
/// selected-track ids across seats.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    /// mpv's per-track id (1-based within its kind); the handle used to select it.
    pub id: i64,
    /// Whether this is a video, audio, or subtitle track.
    pub kind: TrackKind,
    /// The human title, if the container carries one.
    pub title: Option<String>,
    /// The BCP-47 / ISO language tag, if known (`"eng"`, `"jpn"`, …).
    pub lang: Option<String>,
    /// The codec short name (`"h264"`, `"aac"`, `"ass"`, …), if known.
    pub codec: Option<String>,
    /// Whether the container marks this track default.
    pub default: bool,
    /// Whether mpv currently has this track selected.
    pub selected: bool,
}

/// Why playback of a file stopped (mpv's `mpv_end_file_reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// The file played to its natural end.
    Eof,
    /// Playback was stopped on request (our `stop()`, a new `loadfile`, …).
    Stopped,
    /// The file stopped because of an error (decode/network/…).
    Error,
}

/// One decoded video frame, captured off the engine (MEDIA-2 phase 1,
/// `docs/gpu_encoder.md` "Render API to egui texture first").
///
/// Produced by [`MediaEngine::latest_frame`]; `mde-media-egui`'s frame sink
/// uploads `rgba` to an `egui::TextureHandle` and `player_stage` paints it in
/// place of the placeholder rect. This is the "first proof" shape (a plain CPU
/// pixel buffer) — a later DRM overlay-plane path (MEDIA-2 phase 2) can replace
/// it with zero-copy scanout without changing this type's meaning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFrame {
    /// The frame width in pixels.
    pub width: u32,
    /// The frame height in pixels.
    pub height: u32,
    /// Tightly packed, top-to-bottom, non-premultiplied RGBA8 rows —
    /// `width * height * 4` bytes, the exact layout
    /// `egui::ColorImage::from_rgba_unmultiplied` wants.
    pub rgba: Vec<u8>,
}

impl VideoFrame {
    /// Whether every pixel is the same RGBA colour (a uniform fill — all
    /// black, all one flat colour, …) — the honest-gate check the L1
    /// fixture-decode test uses to prove a real, non-degenerate frame came
    /// back rather than an empty/placeholder buffer.
    ///
    /// Compares whole 4-byte pixels, not raw bytes: a uniform *black* frame
    /// (every pixel `[0, 0, 0, 255]`) must read as blank even though its own
    /// bytes aren't all numerically equal (0 vs. the 255 alpha channel).
    #[must_use]
    pub fn is_blank(&self) -> bool {
        let mut pixels = self.rgba.chunks_exact(4);
        pixels
            .next()
            .is_none_or(|first| pixels.all(|pixel| pixel == first))
    }

    /// A cheap, stable content checksum (FNV-1a) over the raw pixel bytes.
    ///
    /// The frame sink uses this to skip a redundant GPU texture upload when the
    /// throttled capture cadence outpaces the decode rate (the same bytes come
    /// back); the L1 fixture-decode test uses it to assert a real, repeatable,
    /// nonzero-content decode.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a 64 offset basis
        for &byte in &self.rgba {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01B3); // FNV-1a 64 prime
        }
        hash
    }
}

/// An asynchronous notification drained from the engine by [`Player::pump`].
///
/// These are the raw engine-side facts (mpv events); the player-state machine
/// interprets them into [`PlayerEvent`](crate::PlayerEvent)s and drives the
/// [`PlayerState`](crate::PlayerState) transitions. Keeping them separate keeps
/// the seam faithful to mpv (which is event-driven) while the *policy* lives in
/// the player.
///
/// [`Player::pump`]: crate::Player::pump
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineSignal {
    /// The media finished loading (its duration + track list are now readable).
    FileLoaded,
    /// The current file stopped playing, for the given reason.
    EndFile(EndReason),
    /// An out-of-band engine error message (surfaced from the event stream).
    Error(String),
}

/// The narrow interface the [`Player`](crate::Player) drives.
///
/// A real implementation wraps mpv; [`FakeMpv`](crate::FakeMpv) simulates it for
/// tests. Every method maps to an mpv command/property/event:
///
/// | seam method        | mpv                                   |
/// |--------------------|---------------------------------------|
/// | [`load_file`]      | `loadfile <url>`                      |
/// | [`set_paused`]     | `set_property pause yes/no`           |
/// | [`seek_absolute`]  | `seek <pos> absolute`                 |
/// | [`stop`]           | `stop`                                |
/// | [`position`]       | `get_property time-pos`               |
/// | [`duration`]       | `get_property duration`               |
/// | [`tracks`]         | `get_property track-list`             |
/// | [`poll`]           | drain `wait_event`                    |
/// | [`apply_audio_config`] | set `af` + `ao`/`replaygain`/… props |
/// | [`apply_video_config`] | set `vf` + `hwdec`/`video-*`/… props |
/// | [`apply_track_selection`] | set `aid`/`vid`/`sid` props |
/// | [`apply_subtitle_config`] | `sub-add` cmds + `sub-*` props |
/// | [`apply_playback_controls`] | set `speed`/`audio-delay`/`ab-loop-*` props |
/// | [`frame_step`]     | `frame-step` / `frame-back-step`      |
/// | [`screenshot`]     | `screenshot <flag>`                   |
/// | [`chapter`]        | `get_property chapter`                |
/// | [`chapter_count`]  | `get_property chapters`               |
/// | [`set_chapter`]    | `set_property chapter`                |
/// | [`latest_frame`]   | `screenshot-to-file` (MEDIA-2 phase 1) |
///
/// [`load_file`]: MediaEngine::load_file
/// [`set_paused`]: MediaEngine::set_paused
/// [`seek_absolute`]: MediaEngine::seek_absolute
/// [`stop`]: MediaEngine::stop
/// [`position`]: MediaEngine::position
/// [`duration`]: MediaEngine::duration
/// [`tracks`]: MediaEngine::tracks
/// [`poll`]: MediaEngine::poll
/// [`apply_audio_config`]: MediaEngine::apply_audio_config
/// [`apply_video_config`]: MediaEngine::apply_video_config
/// [`apply_track_selection`]: MediaEngine::apply_track_selection
/// [`apply_subtitle_config`]: MediaEngine::apply_subtitle_config
/// [`apply_playback_controls`]: MediaEngine::apply_playback_controls
/// [`frame_step`]: MediaEngine::frame_step
/// [`screenshot`]: MediaEngine::screenshot
/// [`chapter`]: MediaEngine::chapter
/// [`chapter_count`]: MediaEngine::chapter_count
/// [`set_chapter`]: MediaEngine::set_chapter
/// [`latest_frame`]: MediaEngine::latest_frame
pub trait MediaEngine {
    /// Begin loading `url` (local path or stream URL). Playback readiness arrives
    /// later as an [`EngineSignal::FileLoaded`] from [`poll`](Self::poll).
    ///
    /// # Errors
    /// Returns [`EngineError::Backend`] if the engine rejects the load outright.
    fn load_file(&mut self, url: &str) -> Result<(), EngineError>;

    /// Set the paused flag on the engine (`true` = paused, `false` = playing).
    ///
    /// # Errors
    /// Returns [`EngineError`] if the backend rejects the property set.
    fn set_paused(&mut self, paused: bool) -> Result<(), EngineError>;

    /// Seek to an absolute position, in seconds from the start.
    ///
    /// # Errors
    /// Returns [`EngineError`] if the backend rejects the seek.
    fn seek_absolute(&mut self, position_secs: f64) -> Result<(), EngineError>;

    /// Stop playback and unload the current file.
    ///
    /// # Errors
    /// Returns [`EngineError`] if the backend rejects the stop.
    fn stop(&mut self) -> Result<(), EngineError>;

    /// The current playback position in seconds, or [`None`] when unknown / no
    /// media is loaded.
    fn position(&self) -> Option<f64>;

    /// The loaded media's duration in seconds, or [`None`] when unknown (not yet
    /// loaded, or a live stream).
    fn duration(&self) -> Option<f64>;

    /// The enumerated tracks of the loaded media (empty when nothing is loaded).
    fn tracks(&self) -> Vec<Track>;

    /// Drain any pending engine signals. Returns them in arrival order; an empty
    /// vec means nothing happened since the last poll.
    fn poll(&mut self) -> Vec<EngineSignal>;

    /// Apply an [`AudioConfig`] (MEDIA-3): route the ao (the `PipeWire` seat audio
    /// path), install the EQ + loudness `af` filter graph, and set the
    /// `ReplayGain` + gapless properties. These are global mpv properties settable
    /// with or without media loaded, so no [`EngineError::NotLoaded`] arises here.
    ///
    /// §6: pure ao/af/property glue — the config *folds* to the mpv strings; no
    /// DSP is reimplemented.
    ///
    /// # Errors
    /// Returns [`EngineError::Backend`] if the backend rejects a property set.
    fn apply_audio_config(&mut self, config: &AudioConfig) -> Result<(), EngineError>;

    /// Apply a [`VideoConfig`] (MEDIA-4): select the `hwdec` decode mode (VA-API
    /// with software fallback), set the `video-*` aspect/zoom/pan/crop/rotate
    /// adjustments + `deinterlace`, and install the `vf` filter graph. Like the
    /// audio path these are global mpv properties settable with or without media
    /// loaded, so no [`EngineError::NotLoaded`] arises here.
    ///
    /// §6: pure hwdec/vf/`video-*` property glue — the config *folds* to the mpv
    /// strings; no decoder or scaler is reimplemented. Whether VA-API actually
    /// engages is a host-GPU property (honest-gated to the real-clip smoke).
    ///
    /// # Errors
    /// Returns [`EngineError::Backend`] if the backend rejects a property set.
    fn apply_video_config(&mut self, config: &VideoConfig) -> Result<(), EngineError>;

    /// Apply a [`TrackSelection`] (MEDIA-5): set the `aid` / `vid` / `sid`
    /// properties that pick one enumerated [`Track`] per kind. These are global
    /// mpv properties (settable with or without media loaded — though the ids only
    /// resolve once a `track-list` exists), so no [`EngineError::NotLoaded`] arises
    /// here.
    ///
    /// §6: pure `aid`/`vid`/`sid` property glue — the selection *folds* to the mpv
    /// strings; no demuxer is reimplemented.
    ///
    /// # Errors
    /// Returns [`EngineError::Backend`] if the backend rejects a property set.
    fn apply_track_selection(&mut self, selection: &TrackSelection) -> Result<(), EngineError>;

    /// Apply a [`SubtitleConfig`] (MEDIA-5): run the `sub-add` commands that load
    /// external subtitle files (`.srt`/`.ass`), then set the `sub-*`
    /// visibility/ASS-override/position/scale/delay styling properties. Loading an
    /// external subtitle before any media is open is rejected by mpv, so this may
    /// surface [`EngineError::Backend`]; the styling properties are global.
    ///
    /// §6: pure `sub-add`/`sub-*` glue — the config *folds* to the mpv
    /// commands/strings; no subtitle renderer is reimplemented.
    ///
    /// # Errors
    /// Returns [`EngineError::Backend`] if the backend rejects a command or
    /// property set.
    fn apply_subtitle_config(&mut self, config: &SubtitleConfig) -> Result<(), EngineError>;

    /// Apply a [`PlaybackControls`] (MEDIA-6): set the `speed` multiplier, the
    /// `audio-delay` A/V-sync offset, the `prefetch-playlist` gapless flag, and the
    /// `ab-loop-a` / `ab-loop-b` loop endpoints. These are global mpv properties
    /// settable with or without media loaded, so no [`EngineError::NotLoaded`]
    /// arises here.
    ///
    /// §6: pure `speed`/`audio-delay`/`ab-loop-*` property glue — the controls
    /// *fold* to the mpv strings; no playback engine is reimplemented.
    ///
    /// # Errors
    /// Returns [`EngineError::Backend`] if the backend rejects a property set.
    fn apply_playback_controls(&mut self, controls: &PlaybackControls) -> Result<(), EngineError>;

    /// Step one frame `forward` (mpv `frame-step`) or backward (`frame-back-step`).
    ///
    /// Frame-stepping is only meaningful with media loaded (typically while
    /// paused); the [`Player`](crate::Player) guards the transport state, so an
    /// engine may map an ill-timed step to a no-op rather than an error.
    ///
    /// # Errors
    /// Returns [`EngineError`] if the backend rejects the command.
    fn frame_step(&mut self, forward: bool) -> Result<(), EngineError>;

    /// Take a snapshot of the current frame (mpv `screenshot`), capturing the mode
    /// selected by [`ScreenshotMode`] (with / without subtitles, or the whole
    /// window).
    ///
    /// # Errors
    /// Returns [`EngineError`] if the backend rejects the command (e.g. nothing is
    /// loaded to capture).
    fn screenshot(&mut self, mode: ScreenshotMode) -> Result<(), EngineError>;

    /// The current chapter index (mpv `chapter`, 0-based), or [`None`] when unknown
    /// / the media is chapterless.
    fn chapter(&self) -> Option<i64>;

    /// The number of chapters in the loaded media (mpv `chapters`), or [`None`] when
    /// unknown.
    fn chapter_count(&self) -> Option<i64>;

    /// Seek to chapter `chapter` (mpv's `chapter` property; 0-based). The
    /// [`Player`](crate::Player) clamps to the valid range for next/prev chapter
    /// navigation.
    ///
    /// # Errors
    /// Returns [`EngineError`] if the backend rejects the property set.
    fn set_chapter(&mut self, chapter: i64) -> Result<(), EngineError>;

    /// The newest decoded video frame available right now (MEDIA-2 phase 1,
    /// `docs/gpu_encoder.md`), or [`None`] when nothing is loaded, there is no
    /// video track, or the engine has not produced one yet.
    ///
    /// A *read* of current state, like [`position`](Self::position) /
    /// [`duration`](Self::duration) / [`chapter`](Self::chapter) — not a
    /// command — so failures fold to [`None`] rather than an [`EngineError`],
    /// exactly like those. `FakeMpv` has nothing to decode, so it always
    /// returns whatever was scripted (or [`None`] by default) — the airgap-safe
    /// default build never fabricates a frame.
    fn latest_frame(&mut self) -> Option<VideoFrame>;
}

#[cfg(test)]
mod tests {
    use super::VideoFrame;

    fn frame(rgba: Vec<u8>) -> VideoFrame {
        VideoFrame {
            width: 2,
            height: 1,
            rgba,
        }
    }

    #[test]
    fn a_uniform_frame_is_blank() {
        assert!(frame(vec![0, 0, 0, 255, 0, 0, 0, 255]).is_blank());
        assert!(
            frame(vec![]).is_blank(),
            "an empty buffer is degenerate too"
        );
    }

    #[test]
    fn a_varied_frame_is_not_blank() {
        assert!(!frame(vec![10, 20, 30, 255, 40, 50, 60, 255]).is_blank());
    }

    #[test]
    fn checksum_is_stable_and_content_sensitive() {
        let a = frame(vec![1, 2, 3, 255, 4, 5, 6, 255]);
        let b = frame(vec![1, 2, 3, 255, 4, 5, 6, 255]);
        let c = frame(vec![1, 2, 3, 255, 4, 5, 7, 255]);
        assert_eq!(a.checksum(), b.checksum(), "identical bytes checksum equal");
        assert_ne!(
            a.checksum(),
            c.checksum(),
            "a changed byte changes the checksum"
        );
        assert_ne!(a.checksum(), 0, "a nonblank frame has a nonzero checksum");
    }
}
