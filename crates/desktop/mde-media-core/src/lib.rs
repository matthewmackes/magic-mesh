//! `mde-media-core` — the libmpv-backed engine + player core (MEDIA-1,
//! `docs/design/mesh-media-player.md`).
//!
//! MCNF's media player (MEDIA epic) is a VLC-class native player driven by
//! **mpv**. This crate is the load-bearing *engine core*: it wraps mpv behind a
//! narrow, injectable [`MediaEngine`] seam and drives a [`Player`] state machine
//! over it. No GUI lives here — the `mde-media-egui` surface (MEDIA-8) and the
//! AV-integration units (MEDIA-2/3/4) build on this.
//!
//! # The seam (§6 glue, testable without mpv)
//!
//! - [`MediaEngine`] is the one interface between the player and the decoder.
//!   Every method is a single mpv command / property / event — this is glue over
//!   mpv, **not** a reimplemented decoder.
//! - [`crate::mpv::MpvEngine`] (feature `mpv`) is the real implementation. It
//!   links the system `libmpv`, so it is OFF by default and honest-gated to a
//!   host that carries `mpv-libs-devel` — see the crate `Cargo.toml`.
//! - [`FakeMpv`] is a deterministic in-tree engine. It is what the unit tests and
//!   the default `media-smoke` binary drive, so the whole state machine is
//!   exercised with **no system libmpv** — the airgap-safe path.
//!
//! # The player
//!
//! [`Player`] owns the transport verbs (`load`/`play`/`pause`/`seek`/`stop`), the
//! authoritative [`PlayerState`] (`Idle`→`Loading`→`Playing`⇄`Paused`→
//! `Stopped`/`Ended`), the live position/duration, the enumerated [`Track`]s, and
//! an ordered [`PlayerEvent`] stream the surface renders from.
//!
//! # Audio (MEDIA-3)
//!
//! [`Player::set_audio_config`] applies a typed [`AudioConfig`] — `PipeWire` ao
//! (the seat audio path), a graphic [`EqBand`] EQ, [`LoudnessNorm`]/[`ReplayGainMode`]
//! normalization, extra [`AudioFilter`]s, and gapless. It *folds* to mpv's `af`
//! graph + properties (unit-tested against [`FakeMpv`]); the audible `PipeWire`
//! result is honest-gated to the `mpv`-feature real-clip smoke.
//!
//! # Video (MEDIA-4)
//!
//! [`Player::set_video_config`] applies a typed [`VideoConfig`] — the [`HwDecode`]
//! mode (VA-API with software fallback), the [`AspectRatio`] override,
//! zoom/pan, [`Crop`], [`Rotation`], [`Deinterlace`], and extra [`VideoFilter`]s.
//! It *folds* to mpv's `hwdec`/`video-*`/`deinterlace` properties + the `vf` graph
//! (unit-tested against [`FakeMpv`]); whether VA-API actually engages is a
//! host-GPU property, honest-gated to the `mpv`-feature real-clip smoke.
//!
//! # Subtitles + multi-track (MEDIA-5)
//!
//! [`Player::set_track_selection`] applies a typed [`TrackSelection`] — the
//! `aid`/`vid`/`sid` ids picking one enumerated [`Track`] per kind, with
//! [`track_by_language`]/[`Player::select_track_by_language`] resolving a language
//! label to a track. [`Player::set_subtitle_config`] applies a [`SubtitleConfig`] —
//! external `.srt`/`.ass` [`ExternalSub`] loads (`sub-add`) plus the `sub-*`
//! visibility / [`AssOverride`] styling / position / scale / delay. Both *fold* to
//! mpv commands + properties (unit-tested against [`FakeMpv`]). The
//! [`opensubtitles`] module fetches subtitles by movie hash — a pure, fixture-tested
//! [`hash_file`](opensubtitles::hash_file) + [`parse_search_response`], with the one
//! HTTPS egress behind the `opensubtitles` feature, honest-gated like the real-clip
//! `mpv` path.
//!
//! # Playlists + advanced controls (MEDIA-6)
//!
//! [`Playlist`] is the load-bearing, pure queue model — ordered [`PlaylistItem`]s
//! with a cursor, a [`RepeatMode`] (off/one/all), and a **deterministic seedable
//! shuffle**. It owns enqueue/dequeue/reorder + `next_item`/`prev_item` transitions
//! and serde save/load ([`Playlist::save`]/[`Playlist::load`]), all unit-tested with
//! no engine. The [`Player`] embeds one and drives the engine from it —
//! [`Player::play_next`]/[`Player::play_prev`] load the next queued item, and an
//! end-of-file **auto-advances** the queue per its repeat mode (a fresh load, not an
//! `Ended`). [`Player::set_controls`] applies typed [`PlaybackControls`] — playback
//! `speed`, the `audio-delay` A/V-sync offset, `prefetch-playlist` gapless, and the
//! [`AbLoop`] A-B loop — and [`Player::frame_step`]/[`Player::snapshot`]/
//! [`Player::chapter_next`] issue mpv's one-shot `frame-step`/`screenshot`/`chapter`
//! commands. All fold to mpv (unit-tested against [`FakeMpv`]); the on-seat result
//! is honest-gated to the `mpv`-feature real-clip smoke.
//!
//! ```
//! use mde_media_core::{FakeMpv, Player, PlayerState};
//!
//! let mut player = Player::new(FakeMpv::new().with_duration(90.0));
//! player.load("test://clip.mkv").expect("load");
//! player.pump(); // fold in the engine's FileLoaded
//! assert_eq!(player.state(), PlayerState::Playing);
//! player.pause().expect("pause");
//! assert_eq!(player.state(), PlayerState::Paused);
//! ```

// Pragmatic pedantic allows: the type names intentionally echo their module
// (`PlayerState` in `player`), and the pure getters are convenience accessors
// rather than a `#[must_use]`-critical API surface.
#![allow(clippy::module_name_repetitions, clippy::must_use_candidate)]

pub mod audio;
pub mod controls;
pub mod engine;
pub mod fake;
pub mod opensubtitles;
pub mod player;
pub mod playlist;
pub mod subtitle;
pub mod video;

#[cfg(feature = "mpv")]
pub mod mpv;

pub use audio::{
    AudioConfig, AudioDriver, AudioFilter, AudioOutput, EqBand, LoudnessNorm, ReplayGainMode,
};
pub use controls::{AbLoop, PlaybackControls, ScreenshotMode};
pub use engine::{EndReason, EngineError, EngineSignal, MediaEngine, Track, TrackKind};
pub use fake::FakeMpv;
pub use opensubtitles::{parse_search_response, request_headers, search_url, SubtitleSearchResult};
pub use player::{Player, PlayerError, PlayerEvent, PlayerState};
pub use playlist::{Playlist, PlaylistItem, RepeatMode};
pub use subtitle::{
    track_by_language, AssOverride, ExternalSub, SubLoad, SubtitleConfig, TrackSelect,
    TrackSelection,
};
pub use video::{AspectRatio, Crop, Deinterlace, HwDecode, Rotation, VideoConfig, VideoFilter};
