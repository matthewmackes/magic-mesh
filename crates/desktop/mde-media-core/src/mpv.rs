//! The real [`MediaEngine`] over **libmpv2** (mpv 2.x client API).
//!
//! This module is compiled only with the `mpv` feature, because `libmpv2` (via
//! `libmpv2-sys`/bindgen) links the system `libmpv` and needs its C headers at
//! build time. The airgapped build farm does not carry `mpv-libs-devel`, so the
//! default build/test/clippy never compile this file — the engine is verified
//! *fetchable* + pinned in `Cargo.lock`, and this real-clip path is honest-gated
//! to a host where system libmpv is present (mirrors `mde-vdi-rdp`'s
//! `live-connect` leg). Everything the [`Player`](crate::Player) needs is exercised
//! against [`FakeMpv`](crate::FakeMpv) regardless.
//!
//! §6: pure glue — each seam method is one mpv command / property / event; no
//! decoding is reimplemented here.

use libmpv2::events::{Event, PropertyData};
use libmpv2::{mpv_end_file_reason, Mpv};

use crate::audio::AudioConfig;
use crate::engine::{EndReason, EngineError, EngineSignal, MediaEngine, Track, TrackKind};

/// The real mpv-backed engine.
///
/// Owns one [`libmpv2::Mpv`] instance. Construct with [`MpvEngine::new`], then
/// drive it through the [`MediaEngine`] trait exactly like the fake.
pub struct MpvEngine {
    mpv: Mpv,
}

impl std::fmt::Debug for MpvEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MpvEngine").finish_non_exhaustive()
    }
}

/// Map any libmpv2 error into the seam's coarse [`EngineError`].
fn backend(e: impl std::fmt::Display) -> EngineError {
    EngineError::Backend(e.to_string())
}

impl MpvEngine {
    /// Create an mpv instance with the platform defaults suitable for the
    /// player core (video enabled, terminal control off — the shell owns the
    /// seat). Audio defaults to the seat's **PipeWire** ao (MEDIA-3, design Q5);
    /// [`apply_audio_config`](MediaEngine::apply_audio_config) then layers the
    /// device / EQ / loudness / ReplayGain / gapless on top. MEDIA-2/4 layer the
    /// DRM plane / VA-API options similarly.
    ///
    /// # Errors
    /// Returns [`EngineError::Backend`] if mpv fails to initialise.
    pub fn new() -> Result<Self, EngineError> {
        let mpv = Mpv::with_initializer(|init| {
            // The host shell owns the DRM seat and input; mpv must not grab a
            // terminal or its own input handling.
            let _ = init.set_property("terminal", "no");
            let _ = init.set_property("input-default-bindings", "no");
            let _ = init.set_property("input-vo-keyboard", "no");
            let _ = init.set_property("osc", "no");
            // MEDIA-3: audio leaves on the seat's PipeWire server by default.
            let _ = init.set_property("ao", "pipewire");
            Ok(())
        })
        .map_err(backend)?;
        Ok(Self { mpv })
    }

    /// Access the underlying [`libmpv2::Mpv`] for the AV-integration units
    /// (MEDIA-2 DRM plane / MEDIA-3 PipeWire ao / MEDIA-4 VA-API) that set
    /// further mpv options directly.
    #[must_use]
    pub const fn raw(&self) -> &Mpv {
        &self.mpv
    }

    /// Read the enumerated tracks via mpv's `track-list/*` properties.
    fn read_tracks(&self) -> Vec<Track> {
        let count = self
            .mpv
            .get_property::<i64>("track-list/count")
            .unwrap_or(0);
        let mut tracks = Vec::with_capacity(count.max(0) as usize);
        for i in 0..count {
            let kind = self
                .mpv
                .get_property::<String>(&format!("track-list/{i}/type"))
                .ok()
                .and_then(|t| TrackKind::from_mpv(&t));
            let Some(kind) = kind else { continue };
            let id = self
                .mpv
                .get_property::<i64>(&format!("track-list/{i}/id"))
                .unwrap_or(i + 1);
            tracks.push(Track {
                id,
                kind,
                title: self
                    .mpv
                    .get_property::<String>(&format!("track-list/{i}/title"))
                    .ok()
                    .filter(|s| !s.is_empty()),
                lang: self
                    .mpv
                    .get_property::<String>(&format!("track-list/{i}/lang"))
                    .ok()
                    .filter(|s| !s.is_empty()),
                codec: self
                    .mpv
                    .get_property::<String>(&format!("track-list/{i}/codec"))
                    .ok()
                    .filter(|s| !s.is_empty()),
                default: self
                    .mpv
                    .get_property::<bool>(&format!("track-list/{i}/default"))
                    .unwrap_or(false),
                selected: self
                    .mpv
                    .get_property::<bool>(&format!("track-list/{i}/selected"))
                    .unwrap_or(false),
            });
        }
        tracks
    }
}

/// Translate an mpv `EndFile` reason into the seam's [`EndReason`].
fn end_reason(reason: libmpv2::EndFileReason) -> EndReason {
    if reason == mpv_end_file_reason::Eof {
        EndReason::Eof
    } else if reason == mpv_end_file_reason::Error {
        EndReason::Error
    } else {
        // Stop / Quit / Redirect all read as an intentional stop for the player.
        EndReason::Stopped
    }
}

impl MediaEngine for MpvEngine {
    fn load_file(&mut self, url: &str) -> Result<(), EngineError> {
        self.mpv.command("loadfile", &[url]).map_err(backend)
    }

    fn set_paused(&mut self, paused: bool) -> Result<(), EngineError> {
        self.mpv.set_property("pause", paused).map_err(backend)
    }

    fn seek_absolute(&mut self, position_secs: f64) -> Result<(), EngineError> {
        self.mpv
            .command("seek", &[&position_secs.to_string(), "absolute"])
            .map_err(backend)
    }

    fn stop(&mut self) -> Result<(), EngineError> {
        self.mpv.command("stop", &[]).map_err(backend)
    }

    fn position(&self) -> Option<f64> {
        self.mpv.get_property::<f64>("time-pos").ok()
    }

    fn duration(&self) -> Option<f64> {
        self.mpv.get_property::<f64>("duration").ok()
    }

    fn tracks(&self) -> Vec<Track> {
        self.read_tracks()
    }

    fn poll(&mut self) -> Vec<EngineSignal> {
        let mut out = Vec::new();
        // Non-blocking drain of the event queue (timeout 0.0 = don't wait).
        while let Some(ev) = self.mpv.wait_event(0.0) {
            match ev {
                Ok(Event::FileLoaded) => out.push(EngineSignal::FileLoaded),
                Ok(Event::EndFile(reason)) => {
                    out.push(EngineSignal::EndFile(end_reason(reason)));
                }
                Ok(Event::Shutdown) => {
                    out.push(EngineSignal::EndFile(EndReason::Stopped));
                    break;
                }
                Ok(Event::PropertyChange {
                    name: "time-pos",
                    change: PropertyData::Double(_),
                    ..
                }) => { /* live clock is read directly in Player::pump */ }
                Ok(_) => {}
                Err(e) => out.push(EngineSignal::Error(e.to_string())),
            }
        }
        out
    }

    fn apply_audio_config(&mut self, config: &AudioConfig) -> Result<(), EngineError> {
        // The EQ + loudness filter chain (an empty string clears mpv's `af`).
        self.mpv
            .set_property("af", config.af_graph().as_str())
            .map_err(backend)?;
        // ao / audio-device / replaygain* / gapless-audio.
        for (key, value) in config.properties() {
            self.mpv
                .set_property(key.as_str(), value.as_str())
                .map_err(backend)?;
        }
        Ok(())
    }
}
