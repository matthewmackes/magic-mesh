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
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use symphonia::core::audio::{SampleBuffer, SignalSpec};
use symphonia::core::codecs::{CodecParameters, DecoderOptions, CODEC_TYPE_NULL, CODEC_TYPE_OPUS};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Gapless pre-buffer lead (ms): the higher-level queue driver (AIR-2.c)
/// starts resolving the next track's stream URL once the current track
/// has this much or less remaining (R— AIR-5 lock). [`Engine::near_end`]
/// exposes the signal; the engine's own `play(list)` is already gapless
/// without it.
pub const GAPLESS_LEAD_MS: u64 = 5_000;

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
    device_rate: u32,
    device_channels: u16,
    /// Back-pressure target: the decode thread throttles once the ring
    /// holds more than this many samples (≈2 s of audio).
    target_ring: usize,
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
            device_rate,
            device_channels,
            target_ring,
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
        self.stop();
        if tracks.is_empty() {
            return;
        }
        self.shared.stop.store(false, Ordering::Relaxed);
        self.shared.playing.store(true, Ordering::Relaxed);
        self.shared.frames_played.store(0, Ordering::Relaxed);
        self.shared.decode_done.store(false, Ordering::Relaxed);

        let shared = self.shared.clone();
        let handle = std::thread::Builder::new()
            .name("mde-musicd-decode".to_string())
            .spawn(move || {
                for (url, codec) in tracks {
                    if shared.stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Err(e) = decode_track(&url, codec, &shared) {
                        tracing::warn!(error = %e, "decode_track failed");
                    }
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
            }
        }
    }

    /// Pause output (the ring is preserved; [`resume`](Engine::resume)
    /// continues seamlessly).
    pub fn pause(&self) {
        self.shared.playing.store(false, Ordering::Relaxed);
    }

    /// Resume after a [`pause`](Engine::pause).
    pub fn resume(&self) {
        self.shared.playing.store(true, Ordering::Relaxed);
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
        self.shared.decode_done.store(true, Ordering::Relaxed);
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

    /// Playhead position (ms), derived from device frames emitted.
    #[must_use]
    pub fn position_ms(&self) -> u64 {
        let frames = self.shared.frames_played.load(Ordering::Relaxed);
        if self.shared.device_rate == 0 {
            0
        } else {
            frames * 1000 / u64::from(self.shared.device_rate)
        }
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
            }
            shared
                .frames_played
                .fetch_add((real / channels) as u64, Ordering::Relaxed);
        },
        |err| tracing::warn!(error = %err, "audio stream error"),
        None,
    )
}

/// Fetch, decode, resample, channel-map, and enqueue one track's samples
/// into the shared ring. Returns when the track is exhausted or `stop` is
/// signalled.
fn decode_track(url: &str, codec: SourceCodec, shared: &Shared) -> Result<(), String> {
    let bytes = reqwest::blocking::get(url)
        .and_then(reqwest::blocking::Response::error_for_status)
        .and_then(reqwest::blocking::Response::bytes)
        .map_err(|e| format!("fetch {url}: {e}"))?
        .to_vec();

    let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
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
        .map_err(|e| format!("probe {url}: {e}"))?;
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
        .map_err(|e| format!("decoder for {url}: {e}"))?;

    let dst_rate = shared.device_rate;
    let dst_ch = shared.device_channels as usize;

    loop {
        if shared.stop.load(Ordering::Relaxed) {
            break;
        }
        // End of stream (UnexpectedEof) or a fatal reset — this track is
        // done; the caller advances to the next one gaplessly.
        let Ok(packet) = format.next_packet() else {
            break;
        };
        if packet.track_id() != track_id {
            continue;
        }
        let audio_ref = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymphoniaError::DecodeError(_)) => continue, // recoverable
            Err(_) => break,
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
        shared
            .ring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(mapped);
    }
    Ok(())
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
        let Ok(packet) = format.next_packet() else {
            break;
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
        shared
            .ring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(mapped);
    }
    Ok(())
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
    fn codec_hint() {
        assert_eq!(SourceCodec::Flac.hint_ext(), Some("flac"));
        assert_eq!(SourceCodec::Vorbis.hint_ext(), Some("ogg"));
        assert_eq!(SourceCodec::Unknown.hint_ext(), None);
        // Opus rides the Ogg container — probed from bytes, no suffix hint.
        assert_eq!(SourceCodec::Opus.hint_ext(), None);
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
