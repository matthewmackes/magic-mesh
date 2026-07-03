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
///
/// [`load_file`]: MediaEngine::load_file
/// [`set_paused`]: MediaEngine::set_paused
/// [`seek_absolute`]: MediaEngine::seek_absolute
/// [`stop`]: MediaEngine::stop
/// [`position`]: MediaEngine::position
/// [`duration`]: MediaEngine::duration
/// [`tracks`]: MediaEngine::tracks
/// [`poll`]: MediaEngine::poll
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
}
