//! AIR-5 (v6.1) — native gapless playback engine.
//!
//! The engine decodes a track's bytes with **Symphonia** (pure-Rust:
//! FLAC / MP3 / Vorbis / AAC / WAV) and plays them through **cpal**
//! (ALSA → PipeWire on this host). Tracks handed to [`Engine::play`] are
//! decoded back-to-back into one continuous sample ring, so album
//! playback is **gapless by construction** — the next track's samples
//! land immediately after the current track's, with no drain in between.
//!
//! Opus (Ogg-Opus) is decoded through **libopus** (AIR-5.b): Symphonia 0.5
//! ships no Opus codec, but its Ogg demuxer still maps the stream + yields
//! Opus audio packets, so [`decode_opus`] feeds those to libopus.
//!
//! Per §0.12 the engine is reachable from a runtime entry point
//! (`mde-musicd play <song-id>…`); per §0.15 the audible-output
//! acceptance (gap-free album playback) is a release HW-bench item. The
//! decode/output side effects therefore aren't unit-tested here — the
//! mechanically-checkable core (codec hinting, the gapless schedule, the
//! volume/resample/channel-map math, the underrun-fill contract) is, and
//! is the same code the side-effecting paths drive.

// Pure DSP / doc style lints that are noise for an audio module: the
// resampler + channel mapper do intentional, bounded integer↔float
// casts; product names in prose (PipeWire / ALSA) aren't code; the audio
// callback's brief lock-in-condition is deliberate; and the unit tests
// compare exact f32 values. The decode/output paths' real robustness
// (poisoned-lock recovery, graceful thread-spawn failure) is handled in
// code below, not suppressed. Mirrors the inline-allow idiom used for
// DSP math elsewhere (e.g. start_menu.rs).
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::doc_markdown,
    clippy::suboptimal_flops,
    clippy::significant_drop_in_scrutinee,
    clippy::float_cmp,
    clippy::too_long_first_doc_paragraph,
    clippy::default_trait_access,
    clippy::missing_const_for_fn
)]

use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use symphonia::core::audio::{SampleBuffer, SignalSpec};
use symphonia::core::codecs::{CodecParameters, DecoderOptions, CODEC_TYPE_NULL, CODEC_TYPE_OPUS};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSourceStream, ReadOnlySource};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

use crate::cache;
use crate::reconnect::{
    backoff_delay_secs, DEFAULT_BASE_SECS, DEFAULT_CAP_SECS, RECONNECT_CONNECT_TIMEOUT_SECS,
    RECONNECT_REQUEST_TIMEOUT_SECS,
};

/// Gapless pre-buffer lead (ms): the higher-level queue driver (AIR-2.c)
/// starts resolving the next track's stream URL once the current track
/// has this much or less remaining (R— AIR-5 lock). [`Engine::near_end`]
/// exposes the signal; the engine's own `play(list)` is already gapless
/// without it.
pub const GAPLESS_LEAD_MS: u64 = 5_000;
/// Maximum number of bounded mid-track reconnects before playback stops.
pub const MAX_MIDTRACK_RECONNECTS: u32 = 3;

// ───────────────────────── pure helpers ─────────────────────────

/// Source container/codec inferred from a track's file suffix. Drives
/// the Symphonia probe [`Hint`] (a hint only speeds + disambiguates
/// probing — the actual format is verified from the bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceCodec {
    /// FLAC (`.flac`).
    Flac,
    /// MPEG-1/2 Layer III (`.mp3`).
    Mp3,
    /// Ogg Vorbis (`.ogg`).
    Vorbis,
    /// AAC, typically in an MP4/M4A container (`.m4a` / `.aac`).
    Aac,
    /// PCM WAV (`.wav`).
    Wav,
    /// Opus (Ogg-Opus) — decoded via libopus (AIR-5.b).
    Opus,
    /// Unknown suffix: probe from the bytes with no extension hint.
    Unknown,
}

/// One logical queue track with an ordered set of admitted source candidates.
/// Candidates are retried only when an earlier source fails before producing
/// decodable audio; the engine still records one gapless boundary per logical
/// track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackTrack {
    /// Network or local source URL plus its decoder hint, best first.
    pub candidates: Vec<(String, SourceCodec)>,
}

impl PlaybackTrack {
    /// Construct one logical track from a single source for legacy callers.
    #[must_use]
    pub fn single(url: String, codec: SourceCodec) -> Self {
        Self {
            candidates: vec![(url, codec)],
        }
    }
}

impl SourceCodec {
    /// Classify from a Subsonic `suffix` (or a filename extension).
    #[must_use]
    pub fn from_suffix(suffix: &str) -> Self {
        match suffix
            .trim()
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "flac" => Self::Flac,
            "mp3" => Self::Mp3,
            "ogg" | "oga" | "vorbis" => Self::Vorbis,
            "aac" | "m4a" | "mp4" | "alac" => Self::Aac,
            "wav" | "wave" => Self::Wav,
            "opus" => Self::Opus,
            _ => Self::Unknown,
        }
    }

    /// The Symphonia probe extension hint (`None` when there's nothing
    /// useful to hint with).
    #[must_use]
    pub fn hint_ext(self) -> Option<&'static str> {
        match self {
            Self::Flac => Some("flac"),
            Self::Mp3 => Some("mp3"),
            Self::Vorbis => Some("ogg"),
            Self::Aac => Some("m4a"),
            Self::Wav => Some("wav"),
            Self::Opus | Self::Unknown => None,
        }
    }

    fn cache_suffix(self) -> &'static str {
        match self {
            Self::Flac => "flac",
            Self::Mp3 => "mp3",
            Self::Vorbis => "ogg",
            Self::Aac => "m4a",
            Self::Wav => "wav",
            Self::Opus => "opus",
            Self::Unknown => "audio",
        }
    }
}

fn stream_cache_identity(url: &str, codec: SourceCodec) -> Option<(String, String)> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let is_stream_endpoint = parsed
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        == Some("stream");
    if !is_stream_endpoint {
        return None;
    }
    let song_id = parsed
        .query_pairs()
        .find(|(key, _)| key == "id")
        .map(|(_, value)| value.into_owned())?;
    if song_id.trim().is_empty() {
        return None;
    }
    Some((song_id, codec.cache_suffix().to_string()))
}

/// Private source URL used to route an offline queue entry through the same
/// decoder/cache path as a live Airsonic stream. It is never sent to the
/// network: [`decode_track`] handles this scheme locally.
const CACHED_STREAM_SCHEME: &str = "mde-cache";

/// Build a bounded, URL-encoded local source for a cached song.
#[must_use]
pub fn cached_stream_url(song_id: &str) -> String {
    let mut url = reqwest::Url::parse("mde-cache:///stream")
        .expect("the built-in cached stream URL must be valid");
    url.query_pairs_mut().append_pair("id", song_id);
    url.to_string()
}

fn is_cached_stream_url(url: &str) -> bool {
    reqwest::Url::parse(url).map_or(false, |parsed| parsed.scheme() == CACHED_STREAM_SCHEME)
}

/// Encode a daemon-admitted local file for the private decoder boundary. This
/// locator is never part of a Clock contract or daemon reply.
pub(crate) fn local_file_stream_url(path: &std::path::Path) -> Option<String> {
    reqwest::Url::from_file_path(path)
        .ok()
        .map(|url| url.to_string())
}

fn local_file_stream_path(url: &str) -> Option<std::path::PathBuf> {
    let parsed = reqwest::Url::parse(url).ok()?;
    (parsed.scheme() == "file")
        .then(|| parsed.to_file_path().ok())
        .flatten()
}

/// Open a daemon-admitted local audio file without following a peer-controlled
/// replacement symlink between URL admission and decoder access.
fn open_admitted_local_file(path: &std::path::Path) -> Result<std::fs::File, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("inspect admitted local audio {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "admitted local audio is not a regular file: {}",
            path.display()
        ));
    }

    #[cfg(target_os = "linux")]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            // Linux's O_NOFOLLOW is stable and avoids adding a direct libc
            // dependency to this service for one open flag.
            .custom_flags(0o400000)
            .open(path)
    };
    #[cfg(all(unix, not(target_os = "linux")))]
    let file = std::fs::File::open(path);
    #[cfg(not(unix))]
    let file = std::fs::File::open(path);

    let file = file.map_err(|error| {
        format!(
            "open admitted local Clock audio {}: {error}",
            path.display()
        )
    })?;
    let opened = file.metadata().map_err(|error| {
        format!(
            "inspect opened local Clock audio {}: {error}",
            path.display()
        )
    })?;
    if !opened.is_file() || opened.len() != metadata.len() {
        return Err(format!(
            "admitted local audio changed before decode: {}",
            path.display()
        ));
    }
    Ok(file)
}

/// Should the queue driver begin pre-buffering the next track? True once
/// the current track is within [`GAPLESS_LEAD_MS`] of its end (and its
/// duration is known).
#[must_use]
pub fn should_prebuffer_next(position_ms: u64, duration_ms: u64, lead_ms: u64) -> bool {
    duration_ms > 0 && duration_ms.saturating_sub(position_ms) <= lead_ms
}

/// Clamp a volume multiplier into the valid `0.0..=1.0` range.
#[must_use]
pub fn clamp_volume(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

/// MUSIC-RFX-2 — convert a millisecond position to a device-frame count (the
/// playhead unit), so a seek can reset `frames_played` to make `position_ms`
/// report the jump. `rate == 0` (no device) yields 0.
#[must_use]
pub fn ms_to_frames(ms: u64, device_rate: u32) -> u64 {
    if device_rate == 0 {
        0
    } else {
        ms.saturating_mul(u64::from(device_rate)) / 1000
    }
}

/// AIR-2.c — map the audible playhead (`played` device frames) to the track it
/// falls in, given each track's cumulative start-frame offset (ascending). The
/// current track is the last start `<= played`; returns `(index, start_frame)`,
/// or `(0, 0)` when no track has been recorded. A pure function so the gapless
/// boundary math is unit-tested independently of the audio device.
#[must_use]
pub fn track_at_frame(starts: &[u64], played: u64) -> (usize, u64) {
    starts
        .iter()
        .rposition(|&s| s <= played)
        .map_or((0, 0), |i| (i, starts[i]))
}

/// One output sample for the cpal callback: the next ring sample scaled
/// by `volume` when playing, or `None` (→ the callback writes silence and
/// does not advance the playhead) when paused or on a buffer underrun.
#[must_use]
pub fn pull_sample(ring: &mut VecDeque<f32>, playing: bool, volume: f32) -> Option<f32> {
    if !playing {
        return None;
    }
    ring.pop_front().map(|s| s * clamp_volume(volume))
}

/// Linear-interpolation resample of interleaved `input` from `src_rate`
/// to `dst_rate`. A first-pass resampler — good enough to verify the
/// pipeline; the HW bench judges audio quality and drives any upgrade to
/// a windowed-sinc resampler. Returns `input` unchanged when the rates
/// match or an argument is degenerate.
#[must_use]
pub fn resample_linear(input: &[f32], channels: usize, src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if channels == 0 || input.is_empty() || src_rate == 0 || dst_rate == 0 || src_rate == dst_rate {
        return input.to_vec();
    }
    let frames_in = input.len() / channels;
    if frames_in == 0 {
        return input.to_vec();
    }
    let frames_out = (frames_in as u64 * u64::from(dst_rate) / u64::from(src_rate)) as usize;
    let mut out = Vec::with_capacity(frames_out * channels);
    let ratio = f64::from(src_rate) / f64::from(dst_rate);
    for f in 0..frames_out {
        let src_pos = f as f64 * ratio;
        let i0 = src_pos.floor() as usize;
        let frac = (src_pos - i0 as f64) as f32;
        let i1 = (i0 + 1).min(frames_in - 1);
        for c in 0..channels {
            let a = input[i0 * channels + c];
            let b = input[i1 * channels + c];
            out.push(a + (b - a) * frac);
        }
    }
    out
}

/// Map interleaved `input` from `src_ch` channels to `dst_ch`: mono is
/// up-mixed by duplication, anything-to-mono is down-mixed by averaging,
/// and other mismatches copy the overlapping channels (padding with
/// silence). Returns `input` unchanged when the counts match.
#[must_use]
pub fn map_channels(input: &[f32], src_ch: usize, dst_ch: usize) -> Vec<f32> {
    if src_ch == 0 || dst_ch == 0 || src_ch == dst_ch {
        return input.to_vec();
    }
    let frames = input.len() / src_ch;
    let mut out = Vec::with_capacity(frames * dst_ch);
    for f in 0..frames {
        let frame = &input[f * src_ch..f * src_ch + src_ch];
        if src_ch == 1 {
            for _ in 0..dst_ch {
                out.push(frame[0]);
            }
        } else if dst_ch == 1 {
            out.push(frame.iter().sum::<f32>() / src_ch as f32);
        } else {
            for c in 0..dst_ch {
                out.push(frame.get(c).copied().unwrap_or(0.0));
            }
        }
    }
    out
}

// ───────────────────────── engine ─────────────────────────

/// State shared between the audio callback, the decode thread, and the
/// owning [`Engine`]. All fields are lock-free atomics except the sample
/// ring, which is a short critical section on each callback / decode push.
struct Shared {
    /// Decoded, device-rate, device-channel interleaved f32 samples.
    ring: Mutex<VecDeque<f32>>,
    /// Volume multiplier, stored as `f32::to_bits` (atomic).
    volume: AtomicU32,
    /// Play / pause. When false the callback emits silence without
    /// draining the ring, so resume is seamless.
    playing: AtomicBool,
    /// Stop signal for the decode thread.
    stop: AtomicBool,
    /// Set true when the decode thread has finished the whole track list.
    decode_done: AtomicBool,
    /// Device frames actually emitted (drives the playhead).
    frames_played: AtomicU64,
    /// Monotonic count of frames physically handed to the renderer. Unlike
    /// `frames_played`, seeks never rewrite this authority witness.
    rendered_frames: AtomicU64,
    /// AIR-2.c — total device frames the decode thread has pushed into the ring
    /// across the whole track list. Used (with [`track_starts`]) to map the
    /// audible playhead back to a track index so the queue cursor auto-advances
    /// at each gapless track boundary.
    frames_enqueued: AtomicU64,
    /// AIR-2.c — the device-frame offset at which each played track's first
    /// sample sits in the continuous output stream (`track_starts[i]` = the
    /// cumulative `frames_enqueued` recorded just before track `i` began
    /// decoding). The currently-audible track is the last entry `<= frames_played`.
    track_starts: Mutex<Vec<u64>>,
    /// MUSIC-RFX-2 — pending seek target in ms; `-1` = no request. The decode
    /// thread checks this each loop, repositions the format, clears the ring, and
    /// resets the playhead. Only honoured for a seekable (finite) source.
    seek_ms: AtomicI64,
    /// MUSIC-RFX-2 — whether the currently-decoding track is seekable (finite +
    /// buffered into a `Cursor`). A live/radio stream sets this false so a seek
    /// request is a no-op (the GUI hides the scrubber).
    seekable: AtomicBool,
    device_rate: u32,
    device_channels: u16,
    /// Back-pressure target: the decode thread throttles once the ring
    /// holds more than this many samples (≈2 s of audio).
    target_ring: usize,
    /// AIR-2.c — the queue cursor that engine-track 0 corresponds to. The
    /// transport `play` verb hands the engine `queue.current..end`, so the
    /// audible queue index is `play_base + current_track_index()`. The serve
    /// loop's auto-advance driver reads this to move the persisted queue cursor.
    play_base: AtomicUsize,
    /// Set by cpal's asynchronous stream-error callback. Once the physical
    /// renderer is gone this engine must never continue claiming playback;
    /// the daemon drops it and acquires a fresh default device.
    renderer_failed: AtomicBool,
    /// Snapshot captured by the stream-error callback before authority is
    /// revoked. Only finite, actively playing media is eligible for automatic
    /// continuation on a replacement renderer.
    renderer_interrupted_playing: AtomicBool,
    renderer_interrupted_seekable: AtomicBool,
    renderer_interrupted_position_ms: AtomicU64,
}

impl Shared {
    /// AIR-2.c — push decoded device samples into the ring and count the frames
    /// toward [`frames_enqueued`], so the track-boundary map stays accurate.
    fn push_samples(&self, samples: &[f32]) {
        let channels = usize::from(self.device_channels.max(1));
        let mut ring = self
            .ring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Renderer revocation and ring publication share this lock. A decoder
        // may already have passed its outer stop check when cpal reports device
        // loss; refusing the write here prevents that retired generation from
        // repopulating the ring after `mark_renderer_failed` cleared it.
        if self.renderer_failed.load(Ordering::Acquire) {
            return;
        }
        ring.extend(samples.iter().copied());
        self.frames_enqueued
            .fetch_add((samples.len() / channels) as u64, Ordering::Relaxed);
    }

    /// AIR-2.c — record the start of a new track at the current enqueued-frame
    /// offset (called once per track, before its samples are pushed).
    fn begin_track(&self) {
        let at = self.frames_enqueued.load(Ordering::Relaxed);
        self.track_starts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(at);
    }

    /// AIR-2.c — the device-frame offset at which the currently-audible track
    /// began: the largest recorded track-start `<= frames_played`. Returns
    /// `(index, start_frame)`; `(0, 0)` before any track has been recorded.
    fn current_track(&self) -> (usize, u64) {
        let played = self.frames_played.load(Ordering::Relaxed);
        let starts = self
            .track_starts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        track_at_frame(&starts, played)
    }

    /// Current audible position within the track, independent of the decode
    /// thread's buffered-ahead samples.
    fn position_ms(&self) -> u64 {
        if self.device_rate == 0 {
            return 0;
        }
        let played = self.frames_played.load(Ordering::Relaxed);
        let (_, start) = self.current_track();
        played.saturating_sub(start).saturating_mul(1000) / u64::from(self.device_rate)
    }

    /// Drop samples that were decoded beyond the audible playhead before a
    /// reconnect. The resumed HTTP request starts at the saved playhead, so
    /// retaining the buffered tail would duplicate audio.
    fn discard_buffered_tail(&self) {
        self.ring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.frames_enqueued.store(
            self.frames_played.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
    }

    /// Withdraw samples buffered by a source that failed before reaching the
    /// renderer, while preserving any still-queued tail of the preceding
    /// logical track. Returns `false` once even one frame from this source has
    /// become audible, because a byte-zero fallback would then replay audio.
    fn discard_inaudible_candidate(
        &self,
        candidate_start: u64,
        rendered_before: u64,
    ) -> bool {
        let mut ring = self
            .ring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let played = self.frames_played.load(Ordering::Relaxed);
        if played > candidate_start
            && self.rendered_frames.load(Ordering::Relaxed) > rendered_before
        {
            return false;
        }

        let channels = usize::from(self.device_channels.max(1));
        let preceding_frames = candidate_start.saturating_sub(played);
        let preceding_samples = usize::try_from(preceding_frames)
            .unwrap_or(usize::MAX)
            .saturating_mul(channels);
        ring.truncate(preceding_samples);
        if played > candidate_start {
            // A seek can move the logical position without rendering a frame.
            // Revoke that unproven jump before admitting a byte-zero fallback.
            self.frames_played.store(candidate_start, Ordering::Relaxed);
        }
        self.frames_enqueued
            .store(candidate_start, Ordering::Relaxed);
        true
    }
}

/// A cheap-to-clone, `Send + Sync` control surface for the engine. All
/// playback control (play / pause / resume / stop / volume / position)
/// lives here because it only touches the lock-free [`Shared`] state + the
/// decode-thread handle — never the thread-pinned cpal stream. AIR-6's
/// MPRIS thread holds one of these to drive playback off the audio thread.
#[derive(Clone)]
pub struct EngineHandle {
    shared: Arc<Shared>,
    decode: Arc<Mutex<Option<JoinHandle<()>>>>,
}

/// The native playback engine: a live cpal output stream fed by a decode
/// thread. Construct once (it grabs the default output device), then drive
/// it with [`play`](EngineHandle::play) / [`pause`](EngineHandle::pause) /
/// [`stop`](EngineHandle::stop). The engine derefs to its [`EngineHandle`],
/// so those calls work directly on an `Engine`; [`handle`](Engine::handle)
/// hands a clone to another thread.
pub struct Engine {
    handle: EngineHandle,
    /// Kept alive for the engine's lifetime — dropping it stops audio.
    _stream: cpal::Stream,
}

impl std::ops::Deref for Engine {
    type Target = EngineHandle;
    fn deref(&self) -> &EngineHandle {
        &self.handle
    }
}

impl Engine {
    /// Open the default output device and start its (initially silent)
    /// stream.
    ///
    /// # Errors
    /// No output device, an unsupported device sample format, or a
    /// stream-build/-start failure.
    pub fn new() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "no default audio output device".to_string())?;
        let supported = device
            .default_output_config()
            .map_err(|e| format!("query output config: {e}"))?;
        let sample_format = supported.sample_format();
        let config: cpal::StreamConfig = supported.config();
        let device_rate = config.sample_rate; // cpal 0.17: SampleRate = u32
        let device_channels = config.channels;
        let target_ring = (device_rate as usize) * (device_channels as usize) * 2;

        let shared = Arc::new(Shared {
            ring: Mutex::new(VecDeque::new()),
            volume: AtomicU32::new(1.0_f32.to_bits()),
            playing: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            decode_done: AtomicBool::new(true),
            frames_played: AtomicU64::new(0),
            rendered_frames: AtomicU64::new(0),
            frames_enqueued: AtomicU64::new(0),
            track_starts: Mutex::new(Vec::new()),
            seek_ms: AtomicI64::new(-1),
            seekable: AtomicBool::new(false),
            device_rate,
            device_channels,
            target_ring,
            play_base: AtomicUsize::new(0),
            renderer_failed: AtomicBool::new(false),
            renderer_interrupted_playing: AtomicBool::new(false),
            renderer_interrupted_seekable: AtomicBool::new(false),
            renderer_interrupted_position_ms: AtomicU64::new(0),
        });

        let stream = match sample_format {
            cpal::SampleFormat::F32 => build_output_stream::<f32>(&device, &config, shared.clone()),
            cpal::SampleFormat::I16 => build_output_stream::<i16>(&device, &config, shared.clone()),
            cpal::SampleFormat::U16 => build_output_stream::<u16>(&device, &config, shared.clone()),
            other => return Err(format!("unsupported device sample format: {other:?}")),
        }
        .map_err(|e| format!("build output stream: {e}"))?;
        stream
            .play()
            .map_err(|e| format!("start output stream: {e}"))?;

        Ok(Self {
            handle: EngineHandle {
                shared,
                decode: Arc::new(Mutex::new(None)),
            },
            _stream: stream,
        })
    }

    /// A cheap-to-clone, `Send + Sync` control handle to this engine — the
    /// surface the MPRIS thread (AIR-6) drives without touching the
    /// thread-pinned cpal stream.
    #[must_use]
    pub fn handle(&self) -> EngineHandle {
        self.handle.clone()
    }
}

impl EngineHandle {
    /// Play the given tracks back-to-back, gaplessly. Each entry is a
    /// stream URL plus its (hinted) codec. Replaces any current playback.
    pub fn play(&self, tracks: Vec<(String, SourceCodec)>) {
        self.play_from(tracks, 0);
    }

    /// Play one closed-set bundled Clock tone until [`stop`](Self::stop).
    /// Samples are synthesized inside the daemon and never resolve a caller
    /// supplied path, URL, or command.
    pub fn play_bundled_clock_tone(&self, tone_id: &str) -> bool {
        let frequency_hz = match tone_id {
            "bell" | "bright-bell" => 880.0_f32,
            "chime" => 660.0_f32,
            _ => return false,
        };
        self.stop();
        if self.shared.renderer_failed.load(Ordering::Acquire) {
            return false;
        }
        self.shared.stop.store(false, Ordering::Relaxed);
        self.shared.playing.store(true, Ordering::Relaxed);
        self.shared.decode_done.store(false, Ordering::Relaxed);
        self.shared.frames_played.store(0, Ordering::Relaxed);
        self.shared.frames_enqueued.store(0, Ordering::Relaxed);
        self.shared
            .track_starts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.shared.begin_track();

        let shared = self.shared.clone();
        let handle = std::thread::Builder::new()
            .name("mde-musicd-clock-tone".to_string())
            .spawn(move || {
                let frames_per_chunk = (shared.device_rate / 50).max(1) as usize;
                let channels = usize::from(shared.device_channels);
                let mut frame_index = 0_u64;
                while !shared.stop.load(Ordering::Relaxed) {
                    while !shared.stop.load(Ordering::Relaxed)
                        && shared
                            .ring
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .len()
                            > shared.target_ring
                    {
                        std::thread::sleep(Duration::from_millis(8));
                    }
                    if shared.stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let mut samples = Vec::with_capacity(frames_per_chunk * channels);
                    for _ in 0..frames_per_chunk {
                        let cycle_frame = frame_index % u64::from(shared.device_rate);
                        let audible = cycle_frame < u64::from(shared.device_rate) * 3 / 5;
                        let phase =
                            2.0_f32 * std::f32::consts::PI * frequency_hz * frame_index as f32
                                / shared.device_rate as f32;
                        let envelope = if audible { 0.35_f32 } else { 0.0_f32 };
                        let sample = phase.sin() * envelope;
                        samples.extend(std::iter::repeat_n(sample, channels));
                        frame_index = frame_index.saturating_add(1);
                    }
                    shared.push_samples(&samples);
                }
                shared.playing.store(false, Ordering::Relaxed);
                shared.decode_done.store(true, Ordering::Relaxed);
            });
        match handle {
            Ok(joined) => {
                *self
                    .decode
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(joined);
                true
            }
            Err(error) => {
                tracing::error!(%error, "could not start Clock tone producer");
                self.shared.playing.store(false, Ordering::Relaxed);
                self.shared.decode_done.store(true, Ordering::Relaxed);
                false
            }
        }
    }

    /// AIR-2.c — like [`play`](EngineHandle::play) but records the queue cursor
    /// that engine-track 0 corresponds to, so the serve loop's auto-advance
    /// driver can map the audible track back to the right queue index as gapless
    /// playback crosses track boundaries.
    pub fn play_from(&self, tracks: Vec<(String, SourceCodec)>, base_cursor: usize) {
        self.play_from_candidates(
            tracks
                .into_iter()
                .map(|(url, codec)| PlaybackTrack::single(url, codec))
                .collect(),
            base_cursor,
        );
    }

    /// AIR-2.c — start logical queue tracks with ordered source fallbacks.
    /// A fallback is attempted only when its predecessor fails before adding
    /// samples, so the audible queue boundary remains one track per queue row.
    pub fn play_from_candidates(&self, tracks: Vec<PlaybackTrack>, base_cursor: usize) {
        let _ = self.play_from_candidates_internal(tracks, base_cursor, None);
    }

    /// Start logical queue tracks and request an initial finite-track position.
    /// The seek is queued before the decode thread opens the source, so a
    /// position-continuous handoff does not race the decoder's first packet.
    /// Live/unseekable sources fail closed to their normal position-zero start.
    pub fn play_from_candidates_at(
        &self,
        tracks: Vec<PlaybackTrack>,
        base_cursor: usize,
        position_ms: u64,
    ) -> bool {
        self.play_from_candidates_internal(tracks, base_cursor, Some(position_ms))
    }

    fn play_from_candidates_internal(
        &self,
        tracks: Vec<PlaybackTrack>,
        base_cursor: usize,
        initial_position_ms: Option<u64>,
    ) -> bool {
        self.stop();
        if tracks.is_empty() || self.shared.renderer_failed.load(Ordering::Acquire) {
            return false;
        }
        self.shared.stop.store(false, Ordering::Relaxed);
        self.shared.playing.store(true, Ordering::Relaxed);
        self.shared.frames_played.store(0, Ordering::Relaxed);
        self.shared.frames_enqueued.store(0, Ordering::Relaxed);
        self.shared
            .track_starts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.shared.play_base.store(base_cursor, Ordering::Relaxed);
        let initial_seek = initial_position_ms
            .filter(|position| *position <= i64::MAX as u64)
            .map_or(-1, |position| position as i64);
        self.shared.seek_ms.store(initial_seek, Ordering::Relaxed);
        self.shared.decode_done.store(false, Ordering::Relaxed);

        let shared = self.shared.clone();
        let handle = std::thread::Builder::new()
            .name("mde-musicd-decode".to_string())
            .spawn(move || {
                for track in tracks {
                    if shared.stop.load(Ordering::Relaxed) {
                        break;
                    }
                    // AIR-2.c — mark this track's start frame BEFORE feeding any
                    // of its samples, so the boundary map stays accurate.
                    shared.begin_track();
                    let mut played = false;
                    for (url, codec) in track.candidates {
                        if shared.stop.load(Ordering::Relaxed) {
                            break;
                        }
                        shared.seekable.store(false, Ordering::Relaxed);
                        let frames_before = shared.frames_enqueued.load(Ordering::Relaxed);
                        let rendered_before = shared.rendered_frames.load(Ordering::Relaxed);
                        match decode_track(&url, codec, &shared) {
                            Ok(()) => {
                                // A provider can return a syntactically valid
                                // container that reaches clean EOF without one
                                // decodable frame.  That provider has not
                                // acquired audible authority, so treating the
                                // clean return as success would suppress the
                                // next admitted source and silently end the
                                // logical queue track.  Fail over only while
                                // doing so cannot replay already-emitted audio.
                                let emitted_audio = shared
                                    .frames_enqueued
                                    .load(Ordering::Relaxed)
                                    > frames_before;
                                if emitted_audio {
                                    played = true;
                                    break;
                                }
                                tracing::warn!(
                                    "source completed without audio; trying next admitted source"
                                );
                            }
                            Err(error) => {
                                let emitted_audio = shared.frames_enqueued.load(Ordering::Relaxed)
                                    > frames_before;
                                if should_try_fallback(emitted_audio) {
                                    tracing::warn!(
                                        error = %error,
                                        "source failed before audio started; trying next admitted source"
                                    );
                                    continue;
                                }

                                // Decoding into the ring does not grant audible
                                // authority. If the renderer has not crossed this
                                // candidate's boundary, remove only its buffered
                                // samples and retain the preceding track's tail;
                                // the next admitted source can still start at the
                                // same logical boundary without replaying audio.
                                if shared.discard_inaudible_candidate(
                                    frames_before,
                                    rendered_before,
                                ) {
                                    tracing::warn!(
                                        error = %error,
                                        "source failed while buffered but inaudible; trying next admitted source"
                                    );
                                    continue;
                                }

                                // Once this source became audible, a fallback
                                // would replay from byte zero. A Subsonic stream
                                // may instead resume at the audible playhead;
                                // arbitrary live URLs remain fail-closed.
                                if reconnect_after_loss(&url, codec, &shared) {
                                    played = true;
                                    break;
                                }
                                tracing::warn!(
                                    error = %error,
                                    "source failed after audio started; bounded resume unavailable; advancing without replaying fallback"
                                );
                                played = true;
                                break;
                            }
                        }
                    }
                    if !played {
                        tracing::warn!("all admitted playback sources failed for one queue track");
                    }
                }
                // A source loss, exhausted fallback set, or normal end leaves
                // no audible work behind. Clear the playing flag before the
                // daemon samples this state; otherwise a failed provider can
                // leave a silent engine claiming mesh playback ownership.
                if !shared.stop.load(Ordering::Relaxed) {
                    shared.playing.store(false, Ordering::Relaxed);
                    shared.seekable.store(false, Ordering::Relaxed);
                }
                shared.decode_done.store(true, Ordering::Relaxed);
            });
        match handle {
            Ok(joined) => {
                *self
                    .decode
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(joined);
            }
            Err(e) => {
                tracing::error!(error = %e, "could not start decode thread");
                // Nothing will play — let the playhead/idle checks settle.
                self.shared.decode_done.store(true, Ordering::Relaxed);
                self.shared.playing.store(false, Ordering::Relaxed);
                return false;
            }
        }
        true
    }

    /// Pause output (the ring is preserved; [`resume`](Engine::resume)
    /// continues seamlessly).
    pub fn pause(&self) {
        self.shared.playing.store(false, Ordering::Relaxed);
    }

    /// Resume after a [`pause`](Engine::pause).
    pub fn resume(&self) {
        if !self.shared.renderer_failed.load(Ordering::Acquire) {
            self.shared.playing.store(true, Ordering::Relaxed);
        }
    }

    /// Stop playback: signal + join the decode thread and clear the ring.
    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.playing.store(false, Ordering::Relaxed);
        if let Some(handle) = self
            .decode
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = handle.join();
        }
        self.shared
            .ring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.shared
            .track_starts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.shared.frames_enqueued.store(0, Ordering::Relaxed);
        self.shared.decode_done.store(true, Ordering::Relaxed);
        self.shared.seekable.store(false, Ordering::Relaxed);
        self.shared.seek_ms.store(-1, Ordering::Relaxed);
    }

    /// Revoke an unhealthy renderer without joining a decode thread that may
    /// currently be blocked in a failed provider read. Dropping the owning
    /// [`Engine`] removes the physical output stream; the detached decoder sees
    /// `stop` and exits when its bounded/provider operation returns.
    pub fn revoke_renderer(&self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.playing.store(false, Ordering::Release);
        self.shared
            .ring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        let _ = self
            .decode
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        self.shared.decode_done.store(true, Ordering::Release);
        self.shared.seekable.store(false, Ordering::Release);
    }

    /// Set the volume multiplier (clamped to `0.0..=1.0`).
    pub fn set_volume(&self, v: f32) {
        self.shared
            .volume
            .store(clamp_volume(v).to_bits(), Ordering::Relaxed);
    }

    /// The current volume multiplier.
    #[must_use]
    pub fn volume(&self) -> f32 {
        f32::from_bits(self.shared.volume.load(Ordering::Relaxed))
    }

    /// MUSIC-RFX-2 — request a seek to `target_ms` within the current track.
    /// Returns `false` immediately if the current source isn't seekable
    /// (live/radio); otherwise the decode thread performs the reposition on its
    /// next loop iteration and the playhead jumps. The reply is best-effort: a
    /// format that refuses the seek leaves playback where it was.
    pub fn seek(&self, target_ms: u64) -> bool {
        if !self.shared.seekable.load(Ordering::Relaxed) {
            return false;
        }
        self.shared
            .seek_ms
            .store(target_ms.min(i64::MAX as u64) as i64, Ordering::Relaxed);
        true
    }

    /// MUSIC-RFX-2 — whether the current track supports seeking (finite +
    /// buffered source). The GUI shows/hides the scrubber off this.
    #[must_use]
    pub fn is_seekable(&self) -> bool {
        self.shared.seekable.load(Ordering::Relaxed)
    }

    /// Playhead position (ms) WITHIN the currently-audible track, derived from
    /// device frames emitted since that track's gapless boundary. For a single
    /// track (or the first track of an album) this equals the raw playhead; for
    /// later album tracks it resets to zero at each boundary so the GUI scrubber
    /// + the AIR-8 heartbeat report the right position. (AIR-2.c)
    #[must_use]
    pub fn position_ms(&self) -> u64 {
        self.shared.position_ms()
    }

    /// AIR-2.c — the index, relative to the track list handed to
    /// [`play_from`](EngineHandle::play_from), of the currently-audible track.
    /// `0` while the first track plays; advances at each gapless boundary.
    #[must_use]
    pub fn current_track_index(&self) -> usize {
        self.shared.current_track().0
    }

    /// AIR-2.c — the queue cursor that engine-track 0 corresponds to (the cursor
    /// at the moment [`play_from`](EngineHandle::play_from) was called). The
    /// audible queue index is `play_base() + current_track_index()`.
    #[must_use]
    pub fn play_base(&self) -> usize {
        self.shared.play_base.load(Ordering::Relaxed)
    }

    /// Whether the engine is in the playing (not paused) state. Distinct
    /// from [`is_active`](Engine::is_active): a paused engine with samples
    /// still buffered is active but not playing.
    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    /// Whether anything is still playing or buffered.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.shared.decode_done.load(Ordering::Relaxed)
            || !self
                .shared
                .ring
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
    }

    /// Whether the physical output stream is still usable. cpal reports
    /// renderer loss asynchronously, so the owning daemon polls this cheap
    /// signal and replaces the complete engine instead of retaining a silent,
    /// failed stream.
    #[must_use]
    pub fn is_renderer_healthy(&self) -> bool {
        !self.shared.renderer_failed.load(Ordering::Acquire)
    }

    /// Audible position captured when the physical renderer failed. Live
    /// streams and idle/paused engines return `None`: restarting either would
    /// invent continuity the daemon cannot prove.
    #[must_use]
    pub fn interrupted_position_ms(&self) -> Option<u64> {
        (self
            .shared
            .renderer_interrupted_playing
            .load(Ordering::Acquire)
            && self
                .shared
                .renderer_interrupted_seekable
                .load(Ordering::Acquire))
        .then(|| {
            self.shared
                .renderer_interrupted_position_ms
                .load(Ordering::Acquire)
        })
    }

    /// Is the current track within [`GAPLESS_LEAD_MS`] of its end? The
    /// signal the queue driver (AIR-2.c) uses to resolve the next track.
    #[must_use]
    pub fn near_end(&self, track_duration_ms: u64) -> bool {
        should_prebuffer_next(self.position_ms(), track_duration_ms, GAPLESS_LEAD_MS)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Stop audio + join the decode thread. Clones of the handle held
        // elsewhere (the AIR-6 MPRIS thread) stay valid but produce no
        // sound once this stream is dropped.
        self.handle.stop();
    }
}

/// Build a typed cpal output stream whose callback drains the shared ring
/// (per the [`pull_sample`] contract) and counts emitted frames toward the
/// playhead. `T` is the device's native sample type.
fn build_output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = shared.device_channels.max(1) as usize;
    let error_shared = shared.clone();
    device.build_output_stream(
        config,
        move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
            let playing = shared.playing.load(Ordering::Relaxed);
            let volume = f32::from_bits(shared.volume.load(Ordering::Relaxed));
            let mut real = 0usize;
            {
                let mut ring = shared
                    .ring
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                for slot in out.iter_mut() {
                    match pull_sample(&mut ring, playing, volume) {
                        Some(s) => {
                            *slot = T::from_sample(s);
                            real += 1;
                        }
                        None => *slot = T::from_sample(0.0),
                    }
                }
                // Keep consumption and its audible-frame authority update in
                // the same critical section. Candidate-failure rollback takes
                // this lock before deciding whether byte-zero fallback is safe.
                shared
                    .frames_played
                    .fetch_add((real / channels) as u64, Ordering::Relaxed);
                shared
                    .rendered_frames
                    .fetch_add((real / channels) as u64, Ordering::Relaxed);
            }
        },
        {
            move |err| {
                tracing::warn!(error = %err, "physical audio renderer failed; daemon will reacquire output");
                mark_renderer_failed(&error_shared);
            }
        },
        None,
    )
}

/// Revoke playback authority immediately after an asynchronous renderer loss.
/// Clearing buffered samples is intentional: they were counted as decoded but
/// can no longer be proven audible, and must not leak into a replacement device
/// as stale output.
fn mark_renderer_failed(shared: &Shared) {
    let interrupted_playing = shared.playing.load(Ordering::Acquire);
    let interrupted_seekable = shared.seekable.load(Ordering::Acquire);
    let interrupted_position_ms = shared.position_ms();
    shared
        .renderer_interrupted_position_ms
        .store(interrupted_position_ms, Ordering::Release);
    shared
        .renderer_interrupted_seekable
        .store(interrupted_seekable, Ordering::Release);
    shared
        .renderer_interrupted_playing
        .store(interrupted_playing, Ordering::Release);
    shared.renderer_failed.store(true, Ordering::Release);
    shared.stop.store(true, Ordering::Relaxed);
    shared.playing.store(false, Ordering::Relaxed);
    shared.seekable.store(false, Ordering::Relaxed);
    shared
        .ring
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    shared.frames_enqueued.store(
        shared.frames_played.load(Ordering::Relaxed),
        Ordering::Relaxed,
    );
}

/// MUSIC-RFX-2 — apply a pending seek (if any) to a seekable `format`. Consumes
/// the request (swaps it back to `-1`); on a successful reposition it clears the
/// ring and resets the playhead so [`EngineHandle::position_ms`] reflects the
/// jump, and returns `true` so the caller resets its decoder. A format that
/// refuses the seek leaves playback untouched.
fn apply_pending_seek(format: &mut dyn FormatReader, track_id: u32, shared: &Shared) -> bool {
    let req = shared.seek_ms.swap(-1, Ordering::Relaxed);
    if req < 0 {
        return false;
    }
    let target_ms = req as u64;
    let time = Time::new(target_ms / 1000, (target_ms % 1000) as f64 / 1000.0);
    if format
        .seek(
            SeekMode::Coarse,
            SeekTo::Time {
                time,
                track_id: Some(track_id),
            },
        )
        .is_err()
    {
        return false;
    }
    shared
        .ring
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    // AIR-2.c — the playhead is cumulative across the gapless track list, so a
    // within-track seek lands at the AUDIBLE track's start offset + the target.
    // `current_track()` keys on `frames_played` (what the listener hears), which
    // is the track the scrubber is scrubbing; the decode thread applying this
    // seek is at most the ~2 s back-pressure buffer ahead, so for the seekable
    // single-/finite-track case this base is the right one. (The previous code
    // reset frames_played to ms_to_frames(target) with no track offset, which
    // mis-mapped every album track past the first back onto track 0.)
    let (_, track_start) = shared.current_track();
    let new_played = track_start + ms_to_frames(target_ms, shared.device_rate);
    shared.frames_played.store(new_played, Ordering::Relaxed);
    // The ring we just cleared was already counted in `frames_enqueued`; those
    // samples will never be emitted, so rewind the enqueued counter to the new
    // playhead. Otherwise the NEXT track's recorded boundary would over-count by
    // the discarded buffer and the boundary→track map would drift.
    shared.frames_enqueued.store(new_played, Ordering::Relaxed);
    true
}

/// Fetch, decode, resample, channel-map, and enqueue one track's samples
/// into the shared ring. Returns when the track is exhausted or `stop` is
/// signalled.
fn decode_track(url: &str, codec: SourceCodec, shared: &Shared) -> Result<(), String> {
    decode_track_at(url, codec, shared, None)
}

/// Decode a track, optionally asking a Subsonic stream endpoint to begin at a
/// bounded resume offset. A reconnect never falls back to the full cached
/// source or overwrites the complete-track cache with an offset response.
fn decode_track_at(
    url: &str,
    codec: SourceCodec,
    shared: &Shared,
    resume_ms: Option<u64>,
) -> Result<(), String> {
    let request_url = resume_ms
        .and_then(|position| resume_stream_url(url, position))
        .unwrap_or_else(|| url.to_owned());
    if resume_ms.is_some() && request_url == url {
        return Err(format!(
            "source does not expose a resumable stream endpoint: {url}"
        ));
    }
    let allow_cache = resume_ms.is_none();
    let cache_identity = stream_cache_identity(url, codec);
    let source: Box<dyn symphonia::core::io::MediaSource> =
        if let Some(path) = local_file_stream_path(url) {
            shared.seekable.store(true, Ordering::Relaxed);
            Box::new(open_admitted_local_file(&path)?)
        } else if is_cached_stream_url(url) {
            Box::new(Cursor::new(cached_track_source(
                cache_identity.as_ref(),
                &format!("offline cache unavailable for {url}"),
                shared,
            )?))
        } else {
            match fetch_stream_response(
                &request_url,
                resume_ms.map(|_| Duration::from_secs(RECONNECT_REQUEST_TIMEOUT_SECS)),
            )
            .and_then(|response| {
                response
                    .error_for_status()
                    .map_err(|error| error.to_string())
            }) {
                Ok(resp) => {
                    // AIR — radio/live streams are infinite (no Content-Length / chunked), so
                    // buffering the whole body with `.bytes()` never returns → "error decoding
                    // response body" + an audio underrun (the reported Radio bug). Stream those
                    // through a pipe into an unseekable source instead. A finite track (a song
                    // from the Airsonic `stream` endpoint, which sends Content-Length) is still
                    // buffered into a seekable Cursor so format decoders that seek keep working.
                    let finite = resp.content_length().is_some_and(|n| n > 0);
                    // MUSIC-RFX-2 — only a finite (Cursor-backed) track is seekable; a live
                    // stream stays false so the scrubber is hidden + a seek request no-ops.
                    shared.seekable.store(finite, Ordering::Relaxed);
                    if finite {
                        let bytes = match resp.bytes() {
                            Ok(bytes) => bytes.to_vec(),
                            Err(e) if allow_cache => cached_track_source(
                                cache_identity.as_ref(),
                                &format!("read body {request_url}: {e}"),
                                shared,
                            )?,
                            Err(e) => return Err(format!("read body {request_url}: {e}")),
                        };
                        if allow_cache {
                            if let Some((song_id, suffix)) = &cache_identity {
                                let _ = cache::write_cached_track(
                                    &cache::cache_dir(),
                                    song_id,
                                    suffix,
                                    &bytes,
                                    cache::now_ms(),
                                    false,
                                );
                            }
                        }
                        Box::new(Cursor::new(bytes))
                    } else {
                        // Keep the response as the decoder's reader. A producer pipe would
                        // turn a provider read error into an indistinguishable clean EOF by
                        // discarding the producer's `io::copy` result.
                        Box::new(ReadOnlySource::new(resp))
                    }
                }
                Err(e) if allow_cache => Box::new(Cursor::new(cached_track_source(
                    cache_identity.as_ref(),
                    &format!("fetch {request_url}: {e}"),
                    shared,
                )?)),
                Err(e) => return Err(format!("fetch {request_url}: {e}")),
            }
        };

    let mss = MediaSourceStream::new(source, Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = codec.hint_ext() {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("probe {request_url}: {e}"))?;
    let mut format = probed.format;

    let track = format
        .default_track()
        .filter(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .or_else(|| {
            format
                .tracks()
                .iter()
                .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        })
        .ok_or_else(|| format!("{url}: no decodable audio track"))?;
    let track_id = track.id;
    let codec_params = track.codec_params.clone();

    // Opus has no Symphonia decoder (0.5 ships none), but Symphonia's Ogg
    // demuxer still maps it — OpusHead/OpusTags are consumed, the params
    // carry the 48 kHz rate + pre-skip delay + channel layout, and
    // `next_packet` yields raw Opus audio packets. Decode those with
    // libopus. Detection keys off the *probed* codec, not the suffix hint:
    // the play paths hand decode_track `SourceCodec::Unknown`.
    if codec_params.codec == CODEC_TYPE_OPUS {
        return decode_opus(format.as_mut(), track_id, &codec_params, shared);
    }

    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_params, &DecoderOptions::default())
        .map_err(|e| format!("decoder for {request_url}: {e}"))?;

    let dst_rate = shared.device_rate;
    let dst_ch = shared.device_channels as usize;

    loop {
        if shared.stop.load(Ordering::Relaxed) {
            break;
        }
        // MUSIC-RFX-2 — honour a pending seek before pulling the next packet.
        if apply_pending_seek(format.as_mut(), track_id, shared) {
            decoder.reset();
        }
        // Symphonia represents a clean unseekable EOF as UnexpectedEof. Any
        // other packet-read error is a provider/source failure and must reach
        // the candidate policy above instead of silently advancing.
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(error) if is_clean_stream_eof(&error) => break,
            Err(error) => return Err(format!("read {request_url}: {error}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let audio_ref = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymphoniaError::DecodeError(_)) => continue, // recoverable
            Err(error) => return Err(format!("decode {request_url}: {error}")),
        };
        let spec: SignalSpec = *audio_ref.spec();
        let cap = audio_ref.capacity() as u64;
        if cap == 0 {
            continue;
        }
        let mut sample_buf = SampleBuffer::<f32>::new(cap, spec);
        sample_buf.copy_interleaved_ref(audio_ref);
        let src_ch = spec.channels.count().max(1);
        let resampled = resample_linear(sample_buf.samples(), src_ch, spec.rate, dst_rate);
        let mapped = map_channels(&resampled, src_ch, dst_ch);

        // Back-pressure: keep the ring bounded so we don't decode an
        // entire FLAC into RAM ahead of the playhead.
        while !shared.stop.load(Ordering::Relaxed)
            && shared
                .ring
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len()
                > shared.target_ring
        {
            std::thread::sleep(Duration::from_millis(8));
        }
        // AIR-2.c — push + count frames so the track-boundary map stays accurate.
        shared.push_samples(&mapped);
    }
    Ok(())
}

/// Fetch one stream response. Initial/radio requests intentionally retain the
/// existing no-total-timeout behavior because an active live stream has no
/// finite body. Resumed finite-track requests are different: a provider that
/// accepts the reconnect and then stalls must not pin the decoder thread
/// forever, so they get a bounded connect timeout and per-request deadline.
fn fetch_stream_response(
    request_url: &str,
    reconnect_timeout: Option<Duration>,
) -> Result<reqwest::blocking::Response, String> {
    if let Some(timeout) = reconnect_timeout {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(RECONNECT_CONNECT_TIMEOUT_SECS))
            .build()
            .map_err(|error| format!("build reconnect HTTP client: {error}"))?;
        client
            .get(request_url)
            .timeout(timeout)
            .send()
            .map_err(|error| error.to_string())
    } else {
        reqwest::blocking::get(request_url).map_err(|error| error.to_string())
    }
}

/// Return a Subsonic stream URL with its integer-second resume offset, or
/// `None` for arbitrary radio/direct URLs that cannot prove this contract.
fn resume_stream_url(url: &str, position_ms: u64) -> Option<String> {
    let mut parsed = reqwest::Url::parse(url).ok()?;
    let is_stream = parsed
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        == Some("stream");
    let has_song_id = parsed
        .query_pairs()
        .any(|(key, value)| key == "id" && !value.trim().is_empty());
    if !is_stream || !has_song_id {
        return None;
    }
    let offset_secs = position_ms.saturating_add(999) / 1000;
    let pairs = parsed
        .query_pairs()
        .filter(|(key, _)| key != "timeOffset")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    {
        let mut query = parsed.query_pairs_mut();
        query.clear();
        for (key, value) in pairs {
            query.append_pair(&key, &value);
        }
        query.append_pair("timeOffset", &offset_secs.to_string());
    }
    Some(parsed.to_string())
}

/// Retry a mid-track Subsonic stream from the audible playhead. The wait is
/// bounded and interruptible so stop/shutdown remains responsive.
fn reconnect_after_loss(url: &str, codec: SourceCodec, shared: &Shared) -> bool {
    if resume_stream_url(url, shared.position_ms()).is_none() {
        // A direct/radio URL cannot prove a position-continuous retry, so it
        // must not restart from byte zero. Once this source has acquired
        // audible authority, however, its already-decoded tail remains the
        // authoritative continuation. Preserve that tail so the renderer can
        // drain it and the next logical track starts at the true enqueue
        // boundary instead of cutting playback at the provider-loss instant.
        return false;
    }
    for attempt in 0..MAX_MIDTRACK_RECONNECTS {
        let delay_secs = backoff_delay_secs(attempt, DEFAULT_BASE_SECS, DEFAULT_CAP_SECS);
        let deadline = Instant::now() + Duration::from_secs(delay_secs);
        while Instant::now() < deadline {
            if shared.stop.load(Ordering::Relaxed) {
                return false;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            std::thread::sleep(remaining.min(Duration::from_millis(100)));
        }
        if shared.stop.load(Ordering::Relaxed) {
            return false;
        }
        let position_ms = shared.position_ms();
        shared.discard_buffered_tail();
        match decode_track_at(url, codec, shared, Some(position_ms)) {
            Ok(()) => {
                tracing::info!(attempt = attempt + 1, position_ms, "music stream resumed");
                return true;
            }
            Err(error) => tracing::warn!(
                attempt = attempt + 1,
                position_ms,
                error = %error,
                "music stream reconnect attempt failed"
            ),
        }
    }
    false
}

fn cached_track_source(
    cache_identity: Option<&(String, String)>,
    error: &str,
    shared: &Shared,
) -> Result<Vec<u8>, String> {
    if let Some((song_id, _)) = cache_identity {
        if let Some(bytes) =
            cache::read_cached_track_bytes(&cache::cache_dir(), song_id, cache::now_ms())
        {
            shared.seekable.store(true, Ordering::Relaxed);
            tracing::warn!(
                song_id = %song_id,
                error = %error,
                "using cached Airsonic stream after live fetch failed"
            );
            return Ok(bytes);
        }
    }
    Err(error.to_string())
}

/// Opus output is always 48 kHz.
const OPUS_RATE: u32 = 48_000;
/// Maximum Opus frame size, samples per channel (120 ms @ 48 kHz) — the
/// decode output buffer must hold at least this much.
const OPUS_MAX_FRAME: usize = 5_760;

/// Drop the first `to_skip` frames (per channel) of interleaved `samples`,
/// returning the kept slice + the frames still left to skip. The Ogg-Opus
/// `OpusHead` pre-skip is discarded this way, carrying any remainder across
/// the first few packets.
#[must_use]
fn drop_pre_skip(samples: &[f32], channels: usize, to_skip: usize) -> (&[f32], usize) {
    if to_skip == 0 || channels == 0 {
        return (samples, 0);
    }
    let frames = samples.len() / channels;
    let skip = to_skip.min(frames);
    (&samples[skip * channels..], to_skip - skip)
}

/// Decode an Ogg-Opus stream's packets with libopus, resample + channel-map
/// to the device, and enqueue into the shared ring. Symphonia has already
/// demuxed the Ogg container (consuming the OpusHead/OpusTags headers);
/// `params` carries the fixed 48 kHz rate, the channel layout, and the
/// pre-skip `delay`. Mono + stereo are supported (the libopus simple
/// decoder's range); a surround stream returns an error rather than
/// mis-rendering. Mirrors [`decode_track`]'s resample → channel-map → ring
/// → back-pressure contract.
fn decode_opus(
    format: &mut dyn FormatReader,
    track_id: u32,
    params: &CodecParameters,
    shared: &Shared,
) -> Result<(), String> {
    let channels = params.channels.map_or(2, |c| c.count()).max(1);
    let opus_channels = match channels {
        1 => opus::Channels::Mono,
        2 => opus::Channels::Stereo,
        n => {
            return Err(format!(
                "opus: {n}-channel (surround) streams are not supported — mono/stereo only"
            ))
        }
    };
    let mut decoder = opus::Decoder::new(OPUS_RATE, opus_channels)
        .map_err(|e| format!("opus decoder init: {e}"))?;
    // Pre-skip: samples per channel (at 48 kHz) to discard from the front.
    let mut to_skip = params.delay.unwrap_or(0) as usize;
    let dst_rate = shared.device_rate;
    let dst_ch = shared.device_channels as usize;
    let mut pcm = vec![0.0_f32; OPUS_MAX_FRAME * channels];

    loop {
        if shared.stop.load(Ordering::Relaxed) {
            break;
        }
        // MUSIC-RFX-2 — honour a pending seek; reset the opus decoder so it
        // doesn't carry inter-frame state across the discontinuity.
        if apply_pending_seek(format, track_id, shared) {
            let _ = decoder.reset_state();
            // The encoder pre-skip belongs to the stream start; past a seek there
            // is nothing more to discard.
            to_skip = 0;
        }
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(error) if is_clean_stream_eof(&error) => break,
            Err(error) => return Err(format!("read opus stream: {error}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        // A corrupt packet is recoverable — skip it, keep the stream alive.
        let Ok(frames) = decoder.decode_float(packet.buf(), &mut pcm, false) else {
            continue;
        };
        let (samples, remaining) = drop_pre_skip(&pcm[..frames * channels], channels, to_skip);
        to_skip = remaining;
        if samples.is_empty() {
            continue;
        }
        let resampled = resample_linear(samples, channels, OPUS_RATE, dst_rate);
        let mapped = map_channels(&resampled, channels, dst_ch);
        while !shared.stop.load(Ordering::Relaxed)
            && shared
                .ring
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len()
                > shared.target_ring
        {
            std::thread::sleep(Duration::from_millis(8));
        }
        // AIR-2.c — push + count frames so the track-boundary map stays accurate.
        shared.push_samples(&mapped);
    }
    Ok(())
}

/// Symphonia uses `UnexpectedEof` for a normal end of an unseekable stream.
/// Other I/O errors carry provider/network failure and must remain visible to
/// the caller so source fallback policy can distinguish them.
fn is_clean_stream_eof(error: &SymphoniaError) -> bool {
    matches!(
        error,
        SymphoniaError::IoError(io_error)
            if io_error.kind() == std::io::ErrorKind::UnexpectedEof
    )
}

/// Candidate fallbacks start from byte zero, so they are safe only when the
/// failed source emitted no audio for this logical queue track.
#[must_use]
fn should_try_fallback(emitted_audio: bool) -> bool {
    !emitted_audio
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_from_suffix_classifies() {
        assert_eq!(SourceCodec::from_suffix("flac"), SourceCodec::Flac);
        assert_eq!(SourceCodec::from_suffix("song.MP3"), SourceCodec::Mp3);
        assert_eq!(SourceCodec::from_suffix("ogg"), SourceCodec::Vorbis);
        assert_eq!(SourceCodec::from_suffix("track.m4a"), SourceCodec::Aac);
        assert_eq!(SourceCodec::from_suffix("wav"), SourceCodec::Wav);
        assert_eq!(SourceCodec::from_suffix("opus"), SourceCodec::Opus);
        assert_eq!(SourceCodec::from_suffix("xyz"), SourceCodec::Unknown);
    }

    #[test]
    fn admitted_local_clock_file_decodes_without_a_network_locator() {
        let frames = 480_u32;
        let channels = 2_u16;
        let sample_rate = 48_000_u32;
        let bits = 16_u16;
        let block_align = channels * (bits / 8);
        let data_len = frames * u32::from(block_align);
        let mut wav = Vec::with_capacity(44 + data_len as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&channels.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
        wav.extend_from_slice(&block_align.to_le_bytes());
        wav.extend_from_slice(&bits.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for frame in 0..frames {
            let sample = if frame % 2 == 0 {
                2_000_i16
            } else {
                -2_000_i16
            };
            wav.extend_from_slice(&sample.to_le_bytes());
            wav.extend_from_slice(&(-sample).to_le_bytes());
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("alarm.wav");
        std::fs::write(&path, wav).unwrap();
        let locator = local_file_stream_url(&path).expect("absolute test path");
        let shared = Shared {
            ring: Mutex::new(VecDeque::new()),
            volume: AtomicU32::new(1.0_f32.to_bits()),
            playing: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            decode_done: AtomicBool::new(false),
            frames_played: AtomicU64::new(0),
            rendered_frames: AtomicU64::new(0),
            frames_enqueued: AtomicU64::new(0),
            track_starts: Mutex::new(vec![0]),
            seek_ms: AtomicI64::new(-1),
            seekable: AtomicBool::new(false),
            device_rate: sample_rate,
            device_channels: channels,
            target_ring: 96_000,
            play_base: AtomicUsize::new(0),
            renderer_failed: AtomicBool::new(false),
            renderer_interrupted_playing: AtomicBool::new(false),
            renderer_interrupted_seekable: AtomicBool::new(false),
            renderer_interrupted_position_ms: AtomicU64::new(0),
        };

        decode_track(&locator, SourceCodec::Wav, &shared).unwrap();
        assert!(shared.seekable.load(Ordering::Relaxed));
        assert!(!shared
            .ring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn admitted_local_audio_refuses_symlink_replacement() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("secret.wav");
        std::fs::write(&target, b"not daemon-admitted audio").unwrap();
        let link = dir.path().join("alarm.wav");
        symlink(&target, &link).unwrap();

        let error = open_admitted_local_file(&link).unwrap_err();
        assert!(
            error.contains("not a regular file") || error.contains("Too many levels"),
            "unexpected symlink refusal: {error}"
        );
    }

    #[test]
    fn ms_to_frames_converts_playhead_units() {
        // MUSIC-RFX-2 — a seek resets frames_played = ms_to_frames(target).
        assert_eq!(ms_to_frames(0, 48_000), 0);
        assert_eq!(ms_to_frames(1_000, 48_000), 48_000); // 1s @ 48k = 48k frames
        assert_eq!(ms_to_frames(500, 44_100), 22_050); // 0.5s @ 44.1k
        assert_eq!(ms_to_frames(1_000, 0), 0); // no device → 0, no panic
                                               // A huge target saturates rather than wrapping.
        assert_eq!(ms_to_frames(u64::MAX, 48_000), u64::MAX / 1000);
    }

    #[test]
    fn renderer_failure_revokes_authority_and_refuses_stale_restart() {
        let shared = Arc::new(Shared {
            ring: Mutex::new(VecDeque::from([0.75, -0.75, 0.5, -0.5])),
            volume: AtomicU32::new(1.0_f32.to_bits()),
            playing: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            decode_done: AtomicBool::new(false),
            frames_played: AtomicU64::new(2),
            rendered_frames: AtomicU64::new(2),
            frames_enqueued: AtomicU64::new(4),
            track_starts: Mutex::new(vec![0]),
            seek_ms: AtomicI64::new(-1),
            seekable: AtomicBool::new(true),
            device_rate: 48_000,
            device_channels: 2,
            target_ring: 96_000,
            play_base: AtomicUsize::new(0),
            renderer_failed: AtomicBool::new(false),
            renderer_interrupted_playing: AtomicBool::new(false),
            renderer_interrupted_seekable: AtomicBool::new(false),
            renderer_interrupted_position_ms: AtomicU64::new(0),
        });
        let handle = EngineHandle {
            shared: shared.clone(),
            decode: Arc::new(Mutex::new(None)),
        };

        mark_renderer_failed(&shared);

        assert!(!handle.is_renderer_healthy());
        assert!(!handle.is_playing(), "failed output must yield ownership");
        assert!(!handle.is_seekable());
        assert!(
            shared
                .ring
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty(),
            "samples not proven audible must be discarded"
        );
        assert_eq!(shared.frames_enqueued.load(Ordering::Relaxed), 2);

        handle.resume();
        assert!(
            !handle.is_playing(),
            "a stale handle cannot reclaim playback"
        );
        assert!(!handle.play_from_candidates_at(
            vec![PlaybackTrack::single(
                "https://provider.invalid/stale.mp3".to_string(),
                SourceCodec::Mp3,
            )],
            0,
            0,
        ));
        assert!(!handle.is_playing());
    }

    #[test]
    fn revoked_renderer_generation_cannot_republish_inflight_audio() {
        let shared = Shared {
            ring: Mutex::new(VecDeque::from([0.25, -0.25])),
            volume: AtomicU32::new(1.0_f32.to_bits()),
            playing: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            decode_done: AtomicBool::new(false),
            frames_played: AtomicU64::new(1),
            rendered_frames: AtomicU64::new(1),
            frames_enqueued: AtomicU64::new(2),
            track_starts: Mutex::new(vec![0]),
            seek_ms: AtomicI64::new(-1),
            seekable: AtomicBool::new(true),
            device_rate: 48_000,
            device_channels: 2,
            target_ring: 96_000,
            play_base: AtomicUsize::new(0),
            renderer_failed: AtomicBool::new(false),
            renderer_interrupted_playing: AtomicBool::new(false),
            renderer_interrupted_seekable: AtomicBool::new(false),
            renderer_interrupted_position_ms: AtomicU64::new(0),
        };

        mark_renderer_failed(&shared);
        // Model a decoder that passed its loop-level stop check immediately
        // before cpal revoked this renderer generation.
        shared.push_samples(&[0.9, -0.9, 0.8, -0.8]);

        assert!(
            shared
                .ring
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty(),
            "a retired decoder generation must not repopulate revoked audio"
        );
        assert_eq!(
            shared.frames_enqueued.load(Ordering::Relaxed),
            shared.frames_played.load(Ordering::Relaxed),
            "rejected stale audio must not advance the queue boundary"
        );
    }

    #[test]
    fn track_at_frame_maps_the_playhead_to_a_gapless_track() {
        // AIR-2.c — three tracks starting at frames 0, 100, 250 in the
        // continuous output stream.
        let starts = [0u64, 100, 250];
        // Before the first boundary is even crossed → track 0.
        assert_eq!(track_at_frame(&starts, 0), (0, 0));
        assert_eq!(track_at_frame(&starts, 99), (0, 0));
        // Exactly on a boundary belongs to the new track.
        assert_eq!(track_at_frame(&starts, 100), (1, 100));
        assert_eq!(track_at_frame(&starts, 249), (1, 100));
        assert_eq!(track_at_frame(&starts, 250), (2, 250));
        // Past the last start stays on the last track.
        assert_eq!(track_at_frame(&starts, 9_999), (2, 250));
        // No track recorded yet → track 0 at frame 0 (no panic).
        assert_eq!(track_at_frame(&[], 42), (0, 0));
    }

    #[test]
    fn codec_hint() {
        assert_eq!(SourceCodec::Flac.hint_ext(), Some("flac"));
        assert_eq!(SourceCodec::Vorbis.hint_ext(), Some("ogg"));
        assert_eq!(SourceCodec::Unknown.hint_ext(), None);
        // Opus rides the Ogg container — probed from bytes, no suffix hint.
        assert_eq!(SourceCodec::Opus.hint_ext(), None);
        assert_eq!(SourceCodec::Opus.cache_suffix(), "opus");
        assert_eq!(SourceCodec::Unknown.cache_suffix(), "audio");
    }

    #[test]
    fn stream_cache_identity_extracts_gateway_stream_id() {
        let url = "http://gateway.mesh:4040/mde/airsonic/source-1/rest/stream?u=alice&id=song%2F7&v=1.16.1";
        assert_eq!(
            stream_cache_identity(url, SourceCodec::Flac),
            Some(("song/7".to_string(), "flac".to_string()))
        );
        assert_eq!(
            stream_cache_identity(
                "http://gateway.mesh:4040/mde/airsonic/source-1/rest/getCoverArt?id=song%2F7",
                SourceCodec::Flac
            ),
            None,
            "cover art has its own cache, not the audio cache"
        );
        assert_eq!(
            stream_cache_identity("https://radio.example/live.mp3", SourceCodec::Mp3),
            None,
            "raw radio URLs are not finite Airsonic track cache entries"
        );
    }

    #[test]
    fn cached_stream_url_round_trips_opaque_song_id_without_networking() {
        let url = cached_stream_url("song/7?edition=lossless");
        assert!(is_cached_stream_url(&url));
        assert_eq!(
            stream_cache_identity(&url, SourceCodec::Flac),
            Some(("song/7?edition=lossless".to_string(), "flac".to_string()))
        );
    }

    #[test]
    fn packet_read_only_treats_unexpected_eof_as_clean_completion() {
        let clean_eof = SymphoniaError::IoError(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "end of stream",
        ));
        let provider_reset = SymphoniaError::IoError(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "provider disconnected",
        ));
        assert!(is_clean_stream_eof(&clean_eof));
        assert!(!is_clean_stream_eof(&provider_reset));
        assert!(!is_clean_stream_eof(&SymphoniaError::DecodeError(
            "malformed packet",
        )));
    }

    #[test]
    fn reconnect_request_timeout_rejects_a_provider_that_stalls_after_headers() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stalled provider");
        let address = listener.local_addr().expect("stalled provider address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept reconnect");
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n")
                .expect("write stalled headers");
            thread::sleep(Duration::from_millis(250));
        });

        let url = format!("http://127.0.0.1:{}/rest/stream?id=song-7", address.port());
        let result = fetch_stream_response(&url, Some(Duration::from_millis(50)))
            .and_then(|response| {
                response
                    .error_for_status()
                    .map_err(|error| error.to_string())
            })
            .and_then(|mut response| {
                let mut body = Vec::new();
                response
                    .read_to_end(&mut body)
                    .map(|_| body)
                    .map_err(|error| error.to_string())
            });

        assert!(
            result.is_err(),
            "a stalled resumed body must fail boundedly"
        );
        server.join().expect("stalled provider completed");
    }

    #[test]
    fn fallback_is_bounded_to_failures_before_audio() {
        assert!(should_try_fallback(false));
        assert!(
            !should_try_fallback(true),
            "replaying a partially-heard track from byte zero would duplicate audio"
        );
    }

    #[test]
    fn audible_live_provider_loss_preserves_queued_tail_until_track_handoff() {
        let shared = Arc::new(Shared {
            ring: Mutex::new(VecDeque::from([0.75, -0.75, 0.5, -0.5])),
            volume: AtomicU32::new(1.0_f32.to_bits()),
            playing: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            decode_done: AtomicBool::new(false),
            frames_played: AtomicU64::new(2_400),
            rendered_frames: AtomicU64::new(2_400),
            frames_enqueued: AtomicU64::new(2_402),
            track_starts: Mutex::new(vec![0]),
            seek_ms: AtomicI64::new(-1),
            seekable: AtomicBool::new(false),
            device_rate: 48_000,
            device_channels: 2,
            target_ring: 96_000,
            play_base: AtomicUsize::new(0),
            renderer_failed: AtomicBool::new(false),
            renderer_interrupted_playing: AtomicBool::new(false),
            renderer_interrupted_seekable: AtomicBool::new(false),
            renderer_interrupted_position_ms: AtomicU64::new(0),
        });

        assert!(!reconnect_after_loss(
            "https://radio.example/live.mp3",
            SourceCodec::Mp3,
            &shared
        ));
        assert_eq!(
            shared
                .ring
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![0.75, -0.75, 0.5, -0.5],
            "provider loss must not cut the authoritative source's decoded tail"
        );
        assert_eq!(
            shared.frames_enqueued.load(Ordering::Relaxed),
            2_402,
            "the enqueue boundary must retain the two frames still due to render"
        );
        assert_eq!(shared.frames_played.load(Ordering::Relaxed), 2_400);

        shared.begin_track();
        assert_eq!(
            *shared
                .track_starts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![0, 2_402],
            "the next track must begin after the retained live tail"
        );
    }

    #[test]
    fn buffered_but_inaudible_source_loss_cannot_suppress_admitted_fallback() {
        // Frames 8..10 are the still-queued tail of the preceding track;
        // frames 10..12 belong to a failed replacement candidate. None of the
        // candidate reached the renderer, so its samples must be withdrawn
        // without discarding the preceding track or suppressing fallback.
        let shared = Shared {
            ring: Mutex::new(VecDeque::from([
                0.1, -0.1, 0.2, -0.2, 0.8, -0.8, 0.9, -0.9,
            ])),
            volume: AtomicU32::new(1.0_f32.to_bits()),
            playing: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            decode_done: AtomicBool::new(false),
            frames_played: AtomicU64::new(8),
            rendered_frames: AtomicU64::new(0),
            frames_enqueued: AtomicU64::new(12),
            track_starts: Mutex::new(vec![0, 10]),
            seek_ms: AtomicI64::new(-1),
            seekable: AtomicBool::new(false),
            device_rate: 48_000,
            device_channels: 2,
            target_ring: 96_000,
            play_base: AtomicUsize::new(0),
            renderer_failed: AtomicBool::new(false),
            renderer_interrupted_playing: AtomicBool::new(false),
            renderer_interrupted_seekable: AtomicBool::new(false),
            renderer_interrupted_position_ms: AtomicU64::new(0),
        };

        assert!(shared.discard_inaudible_candidate(10, 0));
        assert_eq!(
            shared
                .ring
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![0.1, -0.1, 0.2, -0.2],
            "only the failed candidate's unheard samples may be withdrawn"
        );
        assert_eq!(shared.frames_enqueued.load(Ordering::Relaxed), 10);
        assert_eq!(
            shared.frames_played.load(Ordering::Relaxed),
            8,
            "preserved preceding audio must not be counted before it is rendered"
        );
    }

    #[test]
    fn provider_failure_clears_playing_authority_after_decode_exits() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind failed provider");
        let address = listener.local_addr().expect("failed provider address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept failed provider");
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request);
            stream
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .expect("write failed provider response");
        });

        let shared = Arc::new(Shared {
            ring: Mutex::new(VecDeque::new()),
            volume: AtomicU32::new(1.0_f32.to_bits()),
            playing: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            decode_done: AtomicBool::new(true),
            frames_played: AtomicU64::new(0),
            rendered_frames: AtomicU64::new(0),
            frames_enqueued: AtomicU64::new(0),
            track_starts: Mutex::new(Vec::new()),
            seek_ms: AtomicI64::new(-1),
            seekable: AtomicBool::new(false),
            device_rate: 48_000,
            device_channels: 2,
            target_ring: 96_000,
            play_base: AtomicUsize::new(0),
            renderer_failed: AtomicBool::new(false),
            renderer_interrupted_playing: AtomicBool::new(false),
            renderer_interrupted_seekable: AtomicBool::new(false),
            renderer_interrupted_position_ms: AtomicU64::new(0),
        });
        let handle = EngineHandle {
            shared: shared.clone(),
            decode: Arc::new(Mutex::new(None)),
        };
        handle.play_from_candidates(
            vec![PlaybackTrack::single(
                format!(
                    "http://127.0.0.1:{}/rest/stream?id=authority-loss-{}",
                    address.port(),
                    address.port()
                ),
                SourceCodec::Unknown,
            )],
            0,
        );

        for _ in 0..200 {
            if shared.decode_done.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(shared.decode_done.load(Ordering::Relaxed));
        assert!(
            !handle.is_playing(),
            "failed provider must not retain ownership"
        );
        assert!(
            !handle.is_active(),
            "failed provider must leave no active audio"
        );

        handle.stop();
        server.join().expect("failed provider fixture completed");
    }

    #[test]
    fn resumable_stream_url_preserves_identity_and_uses_bounded_offset() {
        let url = "http://gateway.mesh:4040/mde/airsonic/source-1/rest/stream?u=alice&id=song%2F7&v=1.16.1";
        let resumed = resume_stream_url(url, 42_001).expect("Subsonic stream is resumable");
        assert!(resumed.contains("id=song%2F7"));
        assert!(resumed.contains("timeOffset=43"));
        assert_eq!(
            resume_stream_url("https://radio.example/live.mp3", 42_000),
            None
        );
    }

    #[test]
    fn reconnect_budget_is_bounded_and_interruptible() {
        assert_eq!(MAX_MIDTRACK_RECONNECTS, 3);
        assert_eq!(
            backoff_delay_secs(0, DEFAULT_BASE_SECS, DEFAULT_CAP_SECS),
            1
        );
        assert_eq!(
            backoff_delay_secs(2, DEFAULT_BASE_SECS, DEFAULT_CAP_SECS),
            4
        );
    }

    #[test]
    fn two_catalog_outage_uses_next_admitted_source_once_without_duplicate_boundary() {
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};
        use std::thread;

        fn read_request(stream: &mut TcpStream) -> String {
            let mut bytes = Vec::new();
            let mut buf = [0_u8; 1024];
            loop {
                let n = stream.read(&mut buf).expect("fixture request read");
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&buf[..n]);
                if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
                assert!(bytes.len() < 16 * 1024, "fixture request remains bounded");
            }
            String::from_utf8(bytes).expect("fixture request is UTF-8")
        }

        fn wav_fixture() -> Vec<u8> {
            let frames = 1_000_u32;
            let channels = 2_u16;
            let sample_rate = 48_000_u32;
            let bits = 16_u16;
            let block_align = channels * (bits / 8);
            let data_len = frames * u32::from(block_align);
            let mut wav = Vec::with_capacity(44 + data_len as usize);
            wav.extend_from_slice(b"RIFF");
            wav.extend_from_slice(&(36 + data_len).to_le_bytes());
            wav.extend_from_slice(b"WAVEfmt ");
            wav.extend_from_slice(&16_u32.to_le_bytes());
            wav.extend_from_slice(&1_u16.to_le_bytes());
            wav.extend_from_slice(&channels.to_le_bytes());
            wav.extend_from_slice(&sample_rate.to_le_bytes());
            wav.extend_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
            wav.extend_from_slice(&block_align.to_le_bytes());
            wav.extend_from_slice(&bits.to_le_bytes());
            wav.extend_from_slice(b"data");
            wav.extend_from_slice(&data_len.to_le_bytes());
            for frame in 0..frames {
                let sample = if frame % 2 == 0 {
                    2_000_i16
                } else {
                    -2_000_i16
                };
                wav.extend_from_slice(&sample.to_le_bytes());
                wav.extend_from_slice(&(-sample).to_le_bytes());
            }
            wav
        }

        let audio = wav_fixture();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind catalog fixture");
        let address = listener.local_addr().expect("catalog fixture address");
        let server = thread::spawn(move || {
            let mut paths = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().expect("accept catalog request");
                let request = read_request(&mut stream);
                let path = request
                    .split_whitespace()
                    .nth(1)
                    .expect("request path")
                    .to_owned();
                let reply = if path == "/catalog-a" {
                    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_vec()
                } else {
                    assert_eq!(path, "/catalog-b");
                    let mut response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        audio.len(),
                        ""
                    )
                    .into_bytes();
                    response.extend_from_slice(&audio);
                    response
                };
                stream.write_all(&reply).expect("write catalog response");
                paths.push(path);
            }
            paths
        });

        let shared = Arc::new(Shared {
            ring: Mutex::new(VecDeque::new()),
            volume: AtomicU32::new(1.0_f32.to_bits()),
            playing: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            decode_done: AtomicBool::new(true),
            frames_played: AtomicU64::new(0),
            rendered_frames: AtomicU64::new(0),
            frames_enqueued: AtomicU64::new(0),
            track_starts: Mutex::new(Vec::new()),
            seek_ms: AtomicI64::new(-1),
            seekable: AtomicBool::new(false),
            device_rate: 48_000,
            device_channels: 2,
            target_ring: 1_000_000,
            play_base: AtomicUsize::new(0),
            renderer_failed: AtomicBool::new(false),
            renderer_interrupted_playing: AtomicBool::new(false),
            renderer_interrupted_seekable: AtomicBool::new(false),
            renderer_interrupted_position_ms: AtomicU64::new(0),
        });
        let handle = EngineHandle {
            shared: shared.clone(),
            decode: Arc::new(Mutex::new(None)),
        };
        handle.play_from_candidates(
            vec![PlaybackTrack {
                candidates: vec![
                    (
                        format!("http://127.0.0.1:{}/catalog-a", address.port()),
                        SourceCodec::Wav,
                    ),
                    (
                        format!("http://127.0.0.1:{}/catalog-b", address.port()),
                        SourceCodec::Wav,
                    ),
                ],
            }],
            0,
        );

        for _ in 0..200 {
            if shared.decode_done.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(shared.decode_done.load(Ordering::Relaxed));
        assert!(
            shared.frames_enqueued.load(Ordering::Relaxed) > 0,
            "the healthy second catalog must produce decoded audio"
        );
        assert_eq!(
            shared
                .track_starts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            &[0],
            "two source candidates still represent one logical queue track"
        );
        handle.stop();
        assert_eq!(
            server.join().expect("catalog fixture completed"),
            vec!["/catalog-a", "/catalog-b"]
        );
    }

    #[test]
    fn zero_audio_provider_cannot_suppress_healthy_admitted_fallback() {
        use std::thread;

        fn wav_fixture(frames: u32, sample: i16) -> Vec<u8> {
            let channels = 2_u16;
            let sample_rate = 48_000_u32;
            let bits = 16_u16;
            let block_align = channels * (bits / 8);
            let data_len = frames * u32::from(block_align);
            let mut wav = Vec::with_capacity(44 + data_len as usize);
            wav.extend_from_slice(b"RIFF");
            wav.extend_from_slice(&(36 + data_len).to_le_bytes());
            wav.extend_from_slice(b"WAVEfmt ");
            wav.extend_from_slice(&16_u32.to_le_bytes());
            wav.extend_from_slice(&1_u16.to_le_bytes());
            wav.extend_from_slice(&channels.to_le_bytes());
            wav.extend_from_slice(&sample_rate.to_le_bytes());
            wav.extend_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
            wav.extend_from_slice(&block_align.to_le_bytes());
            wav.extend_from_slice(&bits.to_le_bytes());
            wav.extend_from_slice(b"data");
            wav.extend_from_slice(&data_len.to_le_bytes());
            for _ in 0..frames {
                wav.extend_from_slice(&sample.to_le_bytes());
                wav.extend_from_slice(&(-sample).to_le_bytes());
            }
            wav
        }

        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty-provider.wav");
        let healthy = dir.path().join("healthy-provider.wav");
        std::fs::write(&empty, wav_fixture(0, 0)).unwrap();
        std::fs::write(&healthy, wav_fixture(1_000, 12_000)).unwrap();

        let shared = Arc::new(Shared {
            ring: Mutex::new(VecDeque::new()),
            volume: AtomicU32::new(1.0_f32.to_bits()),
            playing: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            decode_done: AtomicBool::new(true),
            frames_played: AtomicU64::new(0),
            rendered_frames: AtomicU64::new(0),
            frames_enqueued: AtomicU64::new(0),
            track_starts: Mutex::new(Vec::new()),
            seek_ms: AtomicI64::new(-1),
            seekable: AtomicBool::new(false),
            device_rate: 48_000,
            device_channels: 2,
            target_ring: 1_000_000,
            play_base: AtomicUsize::new(0),
            renderer_failed: AtomicBool::new(false),
            renderer_interrupted_playing: AtomicBool::new(false),
            renderer_interrupted_seekable: AtomicBool::new(false),
            renderer_interrupted_position_ms: AtomicU64::new(0),
        });
        let handle = EngineHandle {
            shared: shared.clone(),
            decode: Arc::new(Mutex::new(None)),
        };
        handle.play_from_candidates(
            vec![PlaybackTrack {
                candidates: vec![
                    (local_file_stream_url(&empty).unwrap(), SourceCodec::Wav),
                    (local_file_stream_url(&healthy).unwrap(), SourceCodec::Wav),
                ],
            }],
            0,
        );

        for _ in 0..200 {
            if shared.decode_done.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(shared.decode_done.load(Ordering::Relaxed));
        assert_eq!(
            shared
                .track_starts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            &[0],
            "provider failover must retain one logical queue boundary"
        );
        assert_eq!(shared.frames_enqueued.load(Ordering::Relaxed), 1_000);
        assert!(
            shared
                .ring
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .any(|sample| sample.abs() > 0.3),
            "the healthy fallback must become audible"
        );
        handle.stop();
    }

    #[test]
    fn midstream_reset_reconnects_at_audible_offset_and_discards_ahead_buffer() {
        use socket2::Socket;
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};
        use std::thread;

        fn read_request(stream: &mut TcpStream) -> String {
            let mut bytes = Vec::new();
            let mut buf = [0_u8; 1024];
            loop {
                let n = stream
                    .read(&mut buf)
                    .expect("reconnect fixture request read");
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&buf[..n]);
                if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
                assert!(bytes.len() < 16 * 1024, "reconnect request remains bounded");
            }
            String::from_utf8(bytes).expect("reconnect request is UTF-8")
        }

        fn wav_fixture(frames: u32, sample: i16) -> Vec<u8> {
            let channels = 2_u16;
            let sample_rate = 48_000_u32;
            let bits = 16_u16;
            let block_align = channels * (bits / 8);
            let data_len = frames * u32::from(block_align);
            let mut wav = Vec::with_capacity(44 + data_len as usize);
            wav.extend_from_slice(b"RIFF");
            wav.extend_from_slice(&(36 + data_len).to_le_bytes());
            wav.extend_from_slice(b"WAVEfmt ");
            wav.extend_from_slice(&16_u32.to_le_bytes());
            wav.extend_from_slice(&1_u16.to_le_bytes());
            wav.extend_from_slice(&channels.to_le_bytes());
            wav.extend_from_slice(&sample_rate.to_le_bytes());
            wav.extend_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
            wav.extend_from_slice(&block_align.to_le_bytes());
            wav.extend_from_slice(&bits.to_le_bytes());
            wav.extend_from_slice(b"data");
            wav.extend_from_slice(&data_len.to_le_bytes());
            for _ in 0..frames {
                wav.extend_from_slice(&sample.to_le_bytes());
                wav.extend_from_slice(&(-sample).to_le_bytes());
            }
            wav
        }

        let first = wav_fixture(9_600, 1_000);
        let continuation = wav_fixture(2_400, 12_000);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind reconnect fixture");
        let address = listener.local_addr().expect("reconnect fixture address");
        let server = thread::spawn(move || {
            let (mut initial, _) = listener.accept().expect("accept initial stream");
            let request = read_request(&mut initial);
            assert!(request.contains("/rest/stream?id=song-7"));
            initial
                .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                .expect("write initial headers");
            initial
                .write_all(&first[..44 + 4_800 * 4])
                .expect("write partial initial audio");
            initial.flush().expect("flush partial initial audio");
            let reset_socket = Socket::from(initial);
            reset_socket
                .set_linger(Some(Duration::ZERO))
                .expect("arm reset on initial stream");
            drop(reset_socket);

            let (mut resumed, _) = listener.accept().expect("accept resumed stream");
            let request = read_request(&mut resumed);
            assert!(
                request.contains("id=song-7"),
                "song identity must be retained"
            );
            assert!(
                request.contains("timeOffset=1"),
                "reconnect must use the audible playhead's bounded second offset, request={request:?}"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                continuation.len()
            );
            resumed
                .write_all(response.as_bytes())
                .expect("write resumed headers");
            resumed
                .write_all(&continuation)
                .expect("write resumed audio");
            vec![request]
        });

        let shared = Arc::new(Shared {
            ring: Mutex::new(VecDeque::new()),
            volume: AtomicU32::new(1.0_f32.to_bits()),
            playing: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            decode_done: AtomicBool::new(false),
            frames_played: AtomicU64::new(2_400),
            rendered_frames: AtomicU64::new(2_400),
            frames_enqueued: AtomicU64::new(0),
            track_starts: Mutex::new(Vec::new()),
            seek_ms: AtomicI64::new(-1),
            seekable: AtomicBool::new(false),
            device_rate: 48_000,
            device_channels: 2,
            target_ring: 1_000_000,
            play_base: AtomicUsize::new(0),
            renderer_failed: AtomicBool::new(false),
            renderer_interrupted_playing: AtomicBool::new(false),
            renderer_interrupted_seekable: AtomicBool::new(false),
            renderer_interrupted_position_ms: AtomicU64::new(0),
        });
        shared.begin_track();

        let url = format!("http://127.0.0.1:{}/rest/stream?id=song-7", address.port());
        let initial_result = decode_track(&url, SourceCodec::Wav, &shared);
        assert!(
            initial_result.is_err(),
            "the reset source must reach bounded reconnect handling"
        );
        assert!(
            shared.frames_enqueued.load(Ordering::Relaxed) > 2_400,
            "the failed stream must have produced buffered-ahead audio"
        );
        assert_eq!(
            shared.position_ms(),
            50,
            "fixture playhead before reconnect"
        );

        assert!(reconnect_after_loss(&url, SourceCodec::Wav, &shared));
        assert_eq!(shared.position_ms(), 50);
        assert_eq!(
            shared.frames_enqueued.load(Ordering::Relaxed),
            4_800,
            "discarding the 2,400-frame ahead tail then adding 2,400 resumed frames"
        );
        let ring = shared
            .ring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(!ring.is_empty(), "resumed stream must enqueue audio");
        assert!(
            ring.iter().any(|sample| sample.abs() > 0.3),
            "resumed samples must be audible rather than silence"
        );
        drop(ring);
        assert_eq!(server.join().expect("reconnect fixture completed").len(), 1);
    }

    #[test]
    fn finite_handoff_start_seeks_before_decoding_audio() {
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};
        use std::thread;

        fn read_request(stream: &mut TcpStream) {
            let mut bytes = Vec::new();
            let mut buf = [0_u8; 1024];
            loop {
                let n = stream.read(&mut buf).expect("handoff fixture request read");
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&buf[..n]);
                if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
                assert!(bytes.len() < 16 * 1024, "handoff request remains bounded");
            }
        }

        fn wav_fixture(frames: u32) -> Vec<u8> {
            let channels = 2_u16;
            let sample_rate = 48_000_u32;
            let bits = 16_u16;
            let block_align = channels * (bits / 8);
            let data_len = frames * u32::from(block_align);
            let mut wav = Vec::with_capacity(44 + data_len as usize);
            wav.extend_from_slice(b"RIFF");
            wav.extend_from_slice(&(36 + data_len).to_le_bytes());
            wav.extend_from_slice(b"WAVEfmt ");
            wav.extend_from_slice(&16_u32.to_le_bytes());
            wav.extend_from_slice(&1_u16.to_le_bytes());
            wav.extend_from_slice(&channels.to_le_bytes());
            wav.extend_from_slice(&sample_rate.to_le_bytes());
            wav.extend_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
            wav.extend_from_slice(&block_align.to_le_bytes());
            wav.extend_from_slice(&bits.to_le_bytes());
            wav.extend_from_slice(b"data");
            wav.extend_from_slice(&data_len.to_le_bytes());
            for frame in 0..frames {
                let sample = if frame < 2_400 { 1_000_i16 } else { 12_000_i16 };
                wav.extend_from_slice(&sample.to_le_bytes());
                wav.extend_from_slice(&(-sample).to_le_bytes());
            }
            wav
        }

        let audio = wav_fixture(4_800);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind handoff fixture");
        let address = listener.local_addr().expect("handoff fixture address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept handoff source");
            read_request(&mut stream);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                audio.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write handoff headers");
            stream.write_all(&audio).expect("write handoff audio");
        });

        let shared = Arc::new(Shared {
            ring: Mutex::new(VecDeque::new()),
            volume: AtomicU32::new(1.0_f32.to_bits()),
            playing: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            decode_done: AtomicBool::new(true),
            frames_played: AtomicU64::new(0),
            rendered_frames: AtomicU64::new(0),
            frames_enqueued: AtomicU64::new(0),
            track_starts: Mutex::new(Vec::new()),
            seek_ms: AtomicI64::new(-1),
            seekable: AtomicBool::new(false),
            device_rate: 48_000,
            device_channels: 2,
            target_ring: 1_000_000,
            play_base: AtomicUsize::new(0),
            renderer_failed: AtomicBool::new(false),
            renderer_interrupted_playing: AtomicBool::new(false),
            renderer_interrupted_seekable: AtomicBool::new(false),
            renderer_interrupted_position_ms: AtomicU64::new(0),
        });
        let handle = EngineHandle {
            shared: shared.clone(),
            decode: Arc::new(Mutex::new(None)),
        };
        let url = format!(
            "http://127.0.0.1:{}/rest/stream?id=handoff-song",
            address.port()
        );
        assert!(handle.play_from_candidates_at(
            vec![PlaybackTrack::single(url, SourceCodec::Wav)],
            0,
            50
        ));
        for _ in 0..200 {
            if shared.decode_done.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(shared.decode_done.load(Ordering::Relaxed));
        assert_eq!(shared.position_ms(), 50);
        assert_eq!(
            shared.frames_played.load(Ordering::Relaxed),
            ms_to_frames(50, 48_000)
        );
        assert!(
            shared.frames_enqueued.load(Ordering::Relaxed) > 2_400,
            "target decoder must enqueue audio after the requested handoff position"
        );
        assert!(
            shared
                .ring
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .any(|sample| sample.abs() > 0.3),
            "target decoder must produce non-silent resumed audio"
        );
        handle.stop();
        server.join().expect("handoff fixture completed");
    }

    #[test]
    fn opus_round_trip_decodes_an_encoded_frame() {
        // Prove the libopus binding works end-to-end in this build: encode a
        // 20 ms stereo frame (960 samples/ch @ 48 kHz) then decode it back —
        // the same `opus::Decoder::decode_float` path `decode_opus` drives.
        let mut enc =
            opus::Encoder::new(OPUS_RATE, opus::Channels::Stereo, opus::Application::Audio)
                .expect("opus encoder");
        let frame = 960; // 20 ms @ 48 kHz
        let input = vec![0.0_f32; frame * 2];
        let mut packet = vec![0u8; 4000];
        let n = enc.encode_float(&input, &mut packet).expect("opus encode");
        packet.truncate(n);

        let mut dec = opus::Decoder::new(OPUS_RATE, opus::Channels::Stereo).expect("opus decoder");
        let mut out = vec![0.0_f32; OPUS_MAX_FRAME * 2];
        let frames = dec
            .decode_float(&packet, &mut out, false)
            .expect("opus decode");
        assert_eq!(
            frames, frame,
            "decoded frame count matches the encoded frame"
        );
    }

    #[test]
    fn pre_skip_drops_leading_frames() {
        // 4 stereo frames; skip 2 → keep the last 2 (4 samples), 0 remaining.
        let s = [0., 1., 2., 3., 4., 5., 6., 7.];
        let (kept, rem) = drop_pre_skip(&s, 2, 2);
        assert_eq!(kept, &[4., 5., 6., 7.]);
        assert_eq!(rem, 0);
        // Skip more than present → keep nothing, carry the remainder onward.
        let (kept, rem) = drop_pre_skip(&s, 2, 6);
        assert!(kept.is_empty());
        assert_eq!(rem, 2);
        // No skip → passthrough.
        let (kept, rem) = drop_pre_skip(&s, 2, 0);
        assert_eq!(kept.len(), 8);
        assert_eq!(rem, 0);
    }

    #[test]
    fn prebuffer_fires_only_within_lead() {
        // 4:00 track, 3:54 in → 6 s left → not yet (lead 5 s).
        assert!(!should_prebuffer_next(234_000, 240_000, GAPLESS_LEAD_MS));
        // 3:55.1 in → 4.9 s left → fire.
        assert!(should_prebuffer_next(235_100, 240_000, GAPLESS_LEAD_MS));
        // Exactly at the lead boundary → fire.
        assert!(should_prebuffer_next(235_000, 240_000, GAPLESS_LEAD_MS));
        // Unknown duration → never.
        assert!(!should_prebuffer_next(1_000, 0, GAPLESS_LEAD_MS));
        // Past the end → fire.
        assert!(should_prebuffer_next(999_999, 240_000, GAPLESS_LEAD_MS));
    }

    #[test]
    fn volume_clamps() {
        assert_eq!(clamp_volume(-0.5), 0.0);
        assert_eq!(clamp_volume(0.3), 0.3);
        assert_eq!(clamp_volume(2.0), 1.0);
    }

    #[test]
    fn pull_sample_plays_pauses_and_underruns() {
        let mut ring = VecDeque::from([1.0_f32, 0.5]);
        // Playing at half volume → scaled sample, ring advances.
        assert_eq!(pull_sample(&mut ring, true, 0.5), Some(0.5));
        assert_eq!(ring.len(), 1);
        // Paused → silence, ring preserved.
        assert_eq!(pull_sample(&mut ring, false, 1.0), None);
        assert_eq!(ring.len(), 1);
        // Drain the last, then underrun → None.
        assert_eq!(pull_sample(&mut ring, true, 1.0), Some(0.5));
        assert_eq!(pull_sample(&mut ring, true, 1.0), None);
    }

    #[test]
    fn resample_identity_up_and_down() {
        let stereo = [0.0, 1.0, 0.2, 0.8, 0.4, 0.6, 0.6, 0.4]; // 4 frames, 2ch
                                                               // Same rate → identity.
        assert_eq!(resample_linear(&stereo, 2, 48_000, 48_000), stereo.to_vec());
        // Upsample 2× → ~double the frames.
        let up = resample_linear(&stereo, 2, 24_000, 48_000);
        assert_eq!(up.len() / 2, 8);
        // First output frame equals the first input frame.
        assert!((up[0] - 0.0).abs() < 1e-6 && (up[1] - 1.0).abs() < 1e-6);
        // Downsample 2× → ~half the frames.
        let down = resample_linear(&stereo, 2, 48_000, 24_000);
        assert_eq!(down.len() / 2, 2);
        // Empty + degenerate inputs pass through.
        assert!(resample_linear(&[], 2, 48_000, 24_000).is_empty());
        assert_eq!(resample_linear(&stereo, 2, 0, 24_000), stereo.to_vec());
    }

    #[test]
    fn channel_map_up_down_and_identity() {
        // Mono → stereo duplicates each sample.
        assert_eq!(map_channels(&[0.1, 0.2], 1, 2), vec![0.1, 0.1, 0.2, 0.2]);
        // Stereo → mono averages the pair.
        assert_eq!(map_channels(&[0.0, 1.0, 0.4, 0.6], 2, 1), vec![0.5, 0.5]);
        // Equal counts → identity.
        assert_eq!(map_channels(&[0.3, 0.7], 2, 2), vec![0.3, 0.7]);
        // Degenerate → passthrough.
        assert_eq!(map_channels(&[0.3, 0.7], 0, 2), vec![0.3, 0.7]);
    }
}
