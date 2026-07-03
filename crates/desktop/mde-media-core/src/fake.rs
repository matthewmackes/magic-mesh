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

use crate::audio::AudioConfig;
use crate::controls::{PlaybackControls, ScreenshotMode};
use crate::engine::{EndReason, EngineError, EngineSignal, MediaEngine, Track};
use crate::subtitle::{SubtitleConfig, TrackSelection};
use crate::video::VideoConfig;

/// A scriptable fake engine — see the [module docs](self).
///
/// The `fail_*` flags are independent scripted-failure toggles (one per fallible
/// seam method), not a state machine — a `#[allow]` rather than the lint's
/// suggested refactor keeps each failure axis orthogonal and the builders
/// composable.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default)]
pub struct FakeMpv {
    loaded: Option<String>,
    paused: bool,
    position: f64,
    duration: Option<f64>,
    tracks: Vec<Track>,
    signals: VecDeque<EngineSignal>,
    fail_load: bool,
    fail_audio: bool,
    fail_video: bool,
    fail_tracks: bool,
    fail_subtitle: bool,
    fail_controls: bool,
    fail_chapter: bool,
    /// The current chapter index + total, simulating mpv's `chapter`/`chapters`.
    chapter: Option<i64>,
    chapter_count: Option<i64>,
    /// Every `loadfile`/`seek`/`stop` issued, for assertion.
    commands: Vec<String>,
    /// The last `af` graph string applied via [`MediaEngine::apply_audio_config`].
    applied_af: Option<String>,
    /// The last non-`af` properties applied via [`MediaEngine::apply_audio_config`].
    applied_properties: Vec<(String, String)>,
    /// The last `vf` graph string applied via [`MediaEngine::apply_video_config`].
    applied_vf: Option<String>,
    /// The last non-`vf` properties applied via [`MediaEngine::apply_video_config`].
    applied_video_properties: Vec<(String, String)>,
    /// The last `aid`/`vid`/`sid` properties applied via
    /// [`MediaEngine::apply_track_selection`].
    applied_track_properties: Vec<(String, String)>,
    /// The `sub-add` command argv lists applied via
    /// [`MediaEngine::apply_subtitle_config`].
    applied_sub_commands: Vec<Vec<String>>,
    /// The last `sub-*` styling properties applied via
    /// [`MediaEngine::apply_subtitle_config`].
    applied_subtitle_properties: Vec<(String, String)>,
    /// The last `speed`/`audio-delay`/`ab-loop-*` properties applied via
    /// [`MediaEngine::apply_playback_controls`].
    applied_control_properties: Vec<(String, String)>,
    /// Every frame-step issued via [`MediaEngine::frame_step`] (`true` = forward).
    frame_steps: Vec<bool>,
    /// Every snapshot issued via [`MediaEngine::screenshot`], in order.
    screenshots: Vec<ScreenshotMode>,
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

    /// Make every `apply_audio_config` fail with a backend error (to exercise the
    /// audio-config error path through the [`Player`](crate::Player)).
    #[must_use]
    pub const fn failing_audio(mut self) -> Self {
        self.fail_audio = true;
        self
    }

    /// Make every `apply_video_config` fail with a backend error (to exercise the
    /// video-config error path through the [`Player`](crate::Player)).
    #[must_use]
    pub const fn failing_video(mut self) -> Self {
        self.fail_video = true;
        self
    }

    /// Make every `apply_track_selection` fail with a backend error (to exercise
    /// the track-selection error path through the [`Player`](crate::Player)).
    #[must_use]
    pub const fn failing_tracks(mut self) -> Self {
        self.fail_tracks = true;
        self
    }

    /// Make every `apply_subtitle_config` fail with a backend error (to exercise
    /// the subtitle-config error path through the [`Player`](crate::Player)).
    #[must_use]
    pub const fn failing_subtitle(mut self) -> Self {
        self.fail_subtitle = true;
        self
    }

    /// Make every `apply_playback_controls` fail with a backend error (to exercise
    /// the controls error path through the [`Player`](crate::Player)).
    #[must_use]
    pub const fn failing_controls(mut self) -> Self {
        self.fail_controls = true;
        self
    }

    /// Make every `set_chapter` fail with a backend error (to exercise the chapter
    /// error path through the [`Player`](crate::Player)).
    #[must_use]
    pub const fn failing_chapter(mut self) -> Self {
        self.fail_chapter = true;
        self
    }

    /// Pre-set the media as having `count` chapters, current chapter 0 (simulating
    /// mpv's `chapters`/`chapter` properties on chaptered media).
    #[must_use]
    pub const fn with_chapters(mut self, count: i64) -> Self {
        self.chapter_count = Some(count);
        self.chapter = Some(0);
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

    /// The last `af` filter graph applied via
    /// [`apply_audio_config`](MediaEngine::apply_audio_config), if any.
    #[must_use]
    pub fn applied_af(&self) -> Option<&str> {
        self.applied_af.as_deref()
    }

    /// The last non-`af` properties applied via
    /// [`apply_audio_config`](MediaEngine::apply_audio_config).
    #[must_use]
    pub fn applied_properties(&self) -> &[(String, String)] {
        &self.applied_properties
    }

    /// The last `vf` filter graph applied via
    /// [`apply_video_config`](MediaEngine::apply_video_config), if any.
    #[must_use]
    pub fn applied_vf(&self) -> Option<&str> {
        self.applied_vf.as_deref()
    }

    /// The last non-`vf` properties applied via
    /// [`apply_video_config`](MediaEngine::apply_video_config).
    #[must_use]
    pub fn applied_video_properties(&self) -> &[(String, String)] {
        &self.applied_video_properties
    }

    /// The last `aid`/`vid`/`sid` properties applied via
    /// [`apply_track_selection`](MediaEngine::apply_track_selection).
    #[must_use]
    pub fn applied_track_properties(&self) -> &[(String, String)] {
        &self.applied_track_properties
    }

    /// The `sub-add` command argv lists applied via
    /// [`apply_subtitle_config`](MediaEngine::apply_subtitle_config).
    #[must_use]
    pub fn applied_sub_commands(&self) -> &[Vec<String>] {
        &self.applied_sub_commands
    }

    /// The last `sub-*` styling properties applied via
    /// [`apply_subtitle_config`](MediaEngine::apply_subtitle_config).
    #[must_use]
    pub fn applied_subtitle_properties(&self) -> &[(String, String)] {
        &self.applied_subtitle_properties
    }

    /// The last `speed`/`audio-delay`/`ab-loop-*` properties applied via
    /// [`apply_playback_controls`](MediaEngine::apply_playback_controls).
    #[must_use]
    pub fn applied_control_properties(&self) -> &[(String, String)] {
        &self.applied_control_properties
    }

    /// The frame-steps issued via [`frame_step`](MediaEngine::frame_step) so far
    /// (`true` = forward, `false` = back).
    #[must_use]
    pub fn frame_steps(&self) -> &[bool] {
        &self.frame_steps
    }

    /// The snapshots issued via [`screenshot`](MediaEngine::screenshot) so far.
    #[must_use]
    pub fn screenshots(&self) -> &[ScreenshotMode] {
        &self.screenshots
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

    fn apply_audio_config(&mut self, config: &AudioConfig) -> Result<(), EngineError> {
        if self.fail_audio {
            return Err(EngineError::Backend("audio config rejected".to_owned()));
        }
        // Record exactly what the fold produced, so tests assert the af graph +
        // properties without a real mpv.
        self.applied_af = Some(config.af_graph());
        self.applied_properties = config.properties();
        Ok(())
    }

    fn apply_video_config(&mut self, config: &VideoConfig) -> Result<(), EngineError> {
        if self.fail_video {
            return Err(EngineError::Backend("video config rejected".to_owned()));
        }
        // Record exactly what the fold produced, so tests assert the vf graph +
        // hwdec/video-* properties without a real mpv (or a real GPU).
        self.applied_vf = Some(config.vf_graph());
        self.applied_video_properties = config.properties();
        Ok(())
    }

    fn apply_track_selection(&mut self, selection: &TrackSelection) -> Result<(), EngineError> {
        if self.fail_tracks {
            return Err(EngineError::Backend("track selection rejected".to_owned()));
        }
        // Record the folded aid/vid/sid property set for assertion (no real mpv).
        self.applied_track_properties = selection.properties();
        Ok(())
    }

    fn apply_subtitle_config(&mut self, config: &SubtitleConfig) -> Result<(), EngineError> {
        if self.fail_subtitle {
            return Err(EngineError::Backend("subtitle config rejected".to_owned()));
        }
        // Record the folded sub-add commands + sub-* properties for assertion.
        self.applied_sub_commands = config.commands();
        self.applied_subtitle_properties = config.properties();
        Ok(())
    }

    fn apply_playback_controls(&mut self, controls: &PlaybackControls) -> Result<(), EngineError> {
        if self.fail_controls {
            return Err(EngineError::Backend(
                "playback controls rejected".to_owned(),
            ));
        }
        // Record the folded speed/audio-delay/ab-loop property set for assertion.
        self.applied_control_properties = controls.properties();
        Ok(())
    }

    fn frame_step(&mut self, forward: bool) -> Result<(), EngineError> {
        self.commands.push(
            if forward {
                "frame-step"
            } else {
                "frame-back-step"
            }
            .to_owned(),
        );
        self.frame_steps.push(forward);
        Ok(())
    }

    fn screenshot(&mut self, mode: ScreenshotMode) -> Result<(), EngineError> {
        self.commands.push(format!("screenshot {}", mode.as_mpv()));
        self.screenshots.push(mode);
        Ok(())
    }

    fn chapter(&self) -> Option<i64> {
        self.loaded.as_ref().and(self.chapter)
    }

    fn chapter_count(&self) -> Option<i64> {
        self.loaded.as_ref().and(self.chapter_count)
    }

    fn set_chapter(&mut self, chapter: i64) -> Result<(), EngineError> {
        if self.fail_chapter {
            return Err(EngineError::Backend("chapter set rejected".to_owned()));
        }
        self.commands.push(format!("set chapter {chapter}"));
        self.chapter = Some(chapter);
        Ok(())
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

    #[test]
    fn apply_audio_config_records_fold_without_media() {
        use crate::audio::{AudioConfig, EqBand, ReplayGainMode};

        let mut e = FakeMpv::new();
        // No media loaded — audio properties are global and still apply.
        assert_eq!(e.applied_af(), None);
        let cfg = AudioConfig {
            eq: vec![EqBand::new(1000.0, 3.0, 1.0)],
            replaygain: ReplayGainMode::Track,
            ..AudioConfig::new()
        };
        e.apply_audio_config(&cfg).expect("apply");
        assert_eq!(e.applied_af(), Some("equalizer=f=1000:t=q:w=1:g=3"));
        assert!(e
            .applied_properties()
            .contains(&("replaygain".to_owned(), "track".to_owned())));
        assert!(e
            .applied_properties()
            .contains(&("ao".to_owned(), "pipewire".to_owned())));
    }

    #[test]
    fn failing_audio_surfaces_backend_error() {
        use crate::audio::AudioConfig;

        let mut e = FakeMpv::new().failing_audio();
        assert!(matches!(
            e.apply_audio_config(&AudioConfig::new()),
            Err(EngineError::Backend(_))
        ));
        assert_eq!(e.applied_af(), None);
    }

    #[test]
    fn apply_video_config_records_fold_without_media_or_gpu() {
        use crate::video::{HwDecode, VideoConfig, VideoFilter};

        let mut e = FakeMpv::new();
        // No media loaded, no GPU — the decode/adjust properties are global and
        // still apply (whether VA-API engages is honest-gated to a real GPU).
        assert_eq!(e.applied_vf(), None);
        let cfg = VideoConfig {
            hwdec: HwDecode::VaApi,
            filters: vec![VideoFilter::bare("hqdn3d".to_owned())],
            ..VideoConfig::new()
        };
        e.apply_video_config(&cfg).expect("apply");
        assert_eq!(e.applied_vf(), Some("hqdn3d"));
        assert!(e
            .applied_video_properties()
            .contains(&("hwdec".to_owned(), "vaapi".to_owned())));
        assert!(e
            .applied_video_properties()
            .contains(&("deinterlace".to_owned(), "no".to_owned())));
    }

    #[test]
    fn failing_video_surfaces_backend_error() {
        use crate::video::VideoConfig;

        let mut e = FakeMpv::new().failing_video();
        assert!(matches!(
            e.apply_video_config(&VideoConfig::new()),
            Err(EngineError::Backend(_))
        ));
        assert_eq!(e.applied_vf(), None);
    }

    #[test]
    fn apply_track_selection_records_aid_vid_sid() {
        use crate::subtitle::{TrackSelect, TrackSelection};

        let mut e = FakeMpv::new();
        assert!(e.applied_track_properties().is_empty());
        let sel = TrackSelection {
            audio: TrackSelect::Id(2),
            video: TrackSelect::Auto,
            subtitle: TrackSelect::Off,
        };
        e.apply_track_selection(&sel).expect("apply");
        assert!(e
            .applied_track_properties()
            .contains(&("aid".to_owned(), "2".to_owned())));
        assert!(e
            .applied_track_properties()
            .contains(&("sid".to_owned(), "no".to_owned())));
    }

    #[test]
    fn failing_tracks_surfaces_backend_error() {
        use crate::subtitle::TrackSelection;

        let mut e = FakeMpv::new().failing_tracks();
        assert!(matches!(
            e.apply_track_selection(&TrackSelection::new()),
            Err(EngineError::Backend(_))
        ));
        assert!(e.applied_track_properties().is_empty());
    }

    #[test]
    fn apply_subtitle_config_records_commands_and_properties() {
        use crate::subtitle::{ExternalSub, SubtitleConfig};

        let mut e = FakeMpv::new();
        assert!(e.applied_sub_commands().is_empty());
        let cfg = SubtitleConfig {
            external: vec![ExternalSub::new("/subs/x.srt")],
            delay: 0.5,
            ..SubtitleConfig::new()
        };
        e.apply_subtitle_config(&cfg).expect("apply");
        assert_eq!(
            e.applied_sub_commands(),
            &[vec![
                "sub-add".to_owned(),
                "/subs/x.srt".to_owned(),
                "select".to_owned()
            ]]
        );
        assert!(e
            .applied_subtitle_properties()
            .contains(&("sub-delay".to_owned(), "0.5".to_owned())));
    }

    #[test]
    fn failing_subtitle_surfaces_backend_error() {
        use crate::subtitle::SubtitleConfig;

        let mut e = FakeMpv::new().failing_subtitle();
        assert!(matches!(
            e.apply_subtitle_config(&SubtitleConfig::new()),
            Err(EngineError::Backend(_))
        ));
        assert!(e.applied_sub_commands().is_empty());
    }

    #[test]
    fn apply_playback_controls_records_speed_delay_and_ab_loop() {
        use crate::controls::{AbLoop, PlaybackControls};

        let mut e = FakeMpv::new();
        assert!(e.applied_control_properties().is_empty());
        let controls = PlaybackControls {
            speed: 1.25,
            audio_delay: 0.05,
            ab_loop: AbLoop::Range { a: 10.0, b: 20.0 },
            ..PlaybackControls::new()
        };
        e.apply_playback_controls(&controls).expect("apply");
        assert!(e
            .applied_control_properties()
            .contains(&("speed".to_owned(), "1.25".to_owned())));
        assert!(e
            .applied_control_properties()
            .contains(&("ab-loop-a".to_owned(), "10".to_owned())));
    }

    #[test]
    fn failing_controls_surfaces_backend_error() {
        use crate::controls::PlaybackControls;

        let mut e = FakeMpv::new().failing_controls();
        assert!(matches!(
            e.apply_playback_controls(&PlaybackControls::new()),
            Err(EngineError::Backend(_))
        ));
        assert!(e.applied_control_properties().is_empty());
    }

    #[test]
    fn frame_step_records_direction_and_command() {
        let mut e = FakeMpv::new();
        e.load_file("clip").expect("load");
        e.frame_step(true).expect("step forward");
        e.frame_step(false).expect("step back");
        assert_eq!(e.frame_steps(), &[true, false]);
        assert!(e.commands().contains(&"frame-step".to_owned()));
        assert!(e.commands().contains(&"frame-back-step".to_owned()));
    }

    #[test]
    fn screenshot_records_mode_and_command() {
        let mut e = FakeMpv::new();
        e.load_file("clip").expect("load");
        e.screenshot(ScreenshotMode::Video).expect("snapshot");
        assert_eq!(e.screenshots(), &[ScreenshotMode::Video]);
        assert!(e.commands().contains(&"screenshot video".to_owned()));
    }

    #[test]
    fn chapters_read_and_set_only_with_media() {
        let mut e = FakeMpv::new().with_chapters(5);
        // Chapterful, but nothing loaded yet → no chapter reported.
        assert_eq!(e.chapter(), None);
        assert_eq!(e.chapter_count(), None);

        e.load_file("clip").expect("load");
        assert_eq!(e.chapter(), Some(0));
        assert_eq!(e.chapter_count(), Some(5));

        e.set_chapter(3).expect("set chapter");
        assert_eq!(e.chapter(), Some(3));
        assert!(e.commands().contains(&"set chapter 3".to_owned()));
    }

    #[test]
    fn failing_chapter_surfaces_backend_error() {
        let mut e = FakeMpv::new().with_chapters(3).failing_chapter();
        e.load_file("clip").expect("load");
        assert!(matches!(e.set_chapter(1), Err(EngineError::Backend(_))));
    }
}
