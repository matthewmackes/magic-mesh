//! MEDIA-6: the typed advanced-playback controls that fold to mpv's playback path.
//!
//! Where MEDIA-3/4/5 tune the audio / video / subtitle paths, MEDIA-6 owns the
//! *transport-adjacent* fine controls of the acceptance — **playback speed**,
//! the **A/V-sync offset** (audio-delay), **gapless** playlist prefetch, and the
//! **A-B loop** — plus the one-shot commands ([chapters](crate::Player::set_chapter),
//! [frame-step](crate::Player::frame_step), [snapshot](crate::Player::snapshot))
//! the [`Player`](crate::Player) issues directly. All of it rides mpv's own
//! `speed` / `audio-delay` / `prefetch-playlist` / `ab-loop-a` / `ab-loop-b`
//! properties. §6: this is *glue* — we describe the controls and compile them to
//! the strings mpv already understands; no playback engine is reimplemented here.
//!
//! The load-bearing, unit-tested core is the **fold**: a [`PlaybackControls`]
//! compiles deterministically to a [`properties`](PlaybackControls::properties)
//! list. [`crate::MediaEngine::apply_playback_controls`] applies it; the real
//! [`MpvEngine`](crate::mpv::MpvEngine) sets them as mpv properties, and
//! [`FakeMpv`](crate::FakeMpv) records them so the fold is asserted with no system
//! libmpv. The *audible/visible* result over a real seat is honest-gated to the
//! `mpv`-feature real-clip smoke, exactly like MEDIA-1's decode path.

use serde::{Deserialize, Serialize};

/// Format an `f64` for an mpv property argument (stable, no stray `-0`).
///
/// Mirrors [`crate::audio`]/[`crate::video`]/[`crate::subtitle`]: `f64`'s `Display`
/// already drops the fraction for whole numbers (`1.0` → `"1"`, `-2.5` → `"-2.5"`);
/// this only folds a `-0.0` back to `0.0`.
fn fmt_num(x: f64) -> String {
    let x = if x == 0.0 { 0.0 } else { x };
    format!("{x}")
}

/// `"yes"`/`"no"` — mpv's boolean property spelling.
const fn yes_no(on: bool) -> &'static str {
    if on {
        "yes"
    } else {
        "no"
    }
}

/// The A-B loop — mpv's `ab-loop-a` / `ab-loop-b` properties.
///
/// When a [`Range`](Self::Range) is set, mpv loops playback between the two
/// timestamps (the "A-B loop" of the acceptance); [`Off`](Self::Off) clears both
/// endpoints (`ab-loop-a=no` / `ab-loop-b=no`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AbLoop {
    /// No loop — both endpoints cleared (`ab-loop-a=no`, `ab-loop-b=no`).
    #[default]
    Off,
    /// Loop between `a` and `b` seconds (`ab-loop-a=<a>`, `ab-loop-b=<b>`).
    Range {
        /// The loop start in seconds (mpv `ab-loop-a`).
        a: f64,
        /// The loop end in seconds (mpv `ab-loop-b`).
        b: f64,
    },
}

impl AbLoop {
    /// The `(ab-loop-a, ab-loop-b)` property values (`"no"`/`"no"` when off).
    #[must_use]
    pub fn as_mpv(self) -> (String, String) {
        match self {
            Self::Off => ("no".to_owned(), "no".to_owned()),
            Self::Range { a, b } => (fmt_num(a), fmt_num(b)),
        }
    }
}

/// What a snapshot captures — the flag of mpv's `screenshot` command.
///
/// [`Subtitles`](Self::Subtitles) (mpv's default) captures the rendered video
/// frame including subtitles; [`Video`](Self::Video) captures the decoded frame
/// without subtitles/OSD; [`Window`](Self::Window) captures exactly what is on
/// screen (OSD included).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotMode {
    /// mpv `screenshot subtitles` — the rendered frame incl. subtitles.
    #[default]
    Subtitles,
    /// mpv `screenshot video` — the decoded frame without subtitles/OSD.
    Video,
    /// mpv `screenshot window` — exactly what is on screen (OSD included).
    Window,
}

impl ScreenshotMode {
    /// The `screenshot` command flag token.
    #[must_use]
    pub const fn as_mpv(self) -> &'static str {
        match self {
            Self::Subtitles => "subtitles",
            Self::Video => "video",
            Self::Window => "window",
        }
    }
}

/// The typed advanced-playback configuration for the [`Player`](crate::Player).
///
/// It folds — deterministically and without a real mpv — to the `speed` /
/// `audio-delay` / `prefetch-playlist` / `ab-loop-*`
/// [`properties`](Self::properties). [`Player::set_controls`] applies it.
///
/// [`Player::set_controls`]: crate::Player::set_controls
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlaybackControls {
    /// Playback speed multiplier (mpv `speed`, `1.0` = normal).
    pub speed: f64,
    /// The A/V-sync offset in seconds (mpv `audio-delay`; `+` delays audio,
    /// pulling it later relative to video).
    pub audio_delay: f64,
    /// Gapless playlist playback — pre-demux the next playlist entry (mpv
    /// `prefetch-playlist`) so the queue advances without a gap.
    pub gapless: bool,
    /// The A-B loop endpoints (mpv `ab-loop-a` / `ab-loop-b`).
    pub ab_loop: AbLoop,
}

impl PlaybackControls {
    /// The neutral controls: normal speed, no A/V offset, gapless prefetch on
    /// (design Q9, matching [`AudioConfig`](crate::AudioConfig)'s gapless default),
    /// no A-B loop.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            speed: 1.0,
            audio_delay: 0.0,
            gapless: true,
            ab_loop: AbLoop::Off,
        }
    }

    /// Compile the mpv properties this config sets, always in a stable order:
    /// `speed`, `audio-delay`, `prefetch-playlist`, then `ab-loop-a` / `ab-loop-b`.
    ///
    /// Every property is always emitted (each carries its neutral value when
    /// untouched), so applying a config re-establishes all four controls — matching
    /// [`VideoConfig`](crate::VideoConfig)'s always-present primaries.
    #[must_use]
    pub fn properties(&self) -> Vec<(String, String)> {
        let (ab_a, ab_b) = self.ab_loop.as_mpv();
        vec![
            ("speed".to_owned(), fmt_num(self.speed)),
            ("audio-delay".to_owned(), fmt_num(self.audio_delay)),
            (
                "prefetch-playlist".to_owned(),
                yes_no(self.gapless).to_owned(),
            ),
            ("ab-loop-a".to_owned(), ab_a),
            ("ab-loop-b".to_owned(), ab_b),
        ]
    }
}

impl Default for PlaybackControls {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_normal_speed_gapless_no_loop() {
        let c = PlaybackControls::new();
        assert_eq!(
            c.properties(),
            vec![
                ("speed".to_owned(), "1".to_owned()),
                ("audio-delay".to_owned(), "0".to_owned()),
                ("prefetch-playlist".to_owned(), "yes".to_owned()),
                ("ab-loop-a".to_owned(), "no".to_owned()),
                ("ab-loop-b".to_owned(), "no".to_owned()),
            ]
        );
    }

    #[test]
    fn speed_and_audio_delay_fold_without_stray_negative_zero() {
        let c = PlaybackControls {
            speed: 1.5,
            audio_delay: -0.25,
            ..PlaybackControls::new()
        };
        let props = c.properties();
        assert!(props.contains(&("speed".to_owned(), "1.5".to_owned())));
        assert!(props.contains(&("audio-delay".to_owned(), "-0.25".to_owned())));

        // A zeroed offset renders "0", never "-0".
        let zeroed = PlaybackControls {
            audio_delay: -0.0,
            ..PlaybackControls::new()
        };
        assert!(zeroed
            .properties()
            .contains(&("audio-delay".to_owned(), "0".to_owned())));
    }

    #[test]
    fn gapless_off_folds_prefetch_no() {
        let c = PlaybackControls {
            gapless: false,
            ..PlaybackControls::new()
        };
        assert!(c
            .properties()
            .contains(&("prefetch-playlist".to_owned(), "no".to_owned())));
    }

    #[test]
    fn ab_loop_range_folds_to_a_and_b() {
        let c = PlaybackControls {
            ab_loop: AbLoop::Range { a: 12.5, b: 48.0 },
            ..PlaybackControls::new()
        };
        let props = c.properties();
        assert!(props.contains(&("ab-loop-a".to_owned(), "12.5".to_owned())));
        assert!(props.contains(&("ab-loop-b".to_owned(), "48".to_owned())));
    }

    #[test]
    fn ab_loop_off_clears_both_endpoints() {
        assert_eq!(AbLoop::Off.as_mpv(), ("no".to_owned(), "no".to_owned()));
    }

    #[test]
    fn screenshot_modes_fold_to_flag() {
        assert_eq!(ScreenshotMode::Subtitles.as_mpv(), "subtitles");
        assert_eq!(ScreenshotMode::Video.as_mpv(), "video");
        assert_eq!(ScreenshotMode::Window.as_mpv(), "window");
    }

    #[test]
    fn config_round_trips_through_serde() {
        let c = PlaybackControls {
            speed: 0.75,
            audio_delay: 0.1,
            gapless: false,
            ab_loop: AbLoop::Range { a: 3.0, b: 9.5 },
        };
        let json = serde_json::to_string(&c).expect("serialize");
        let back: PlaybackControls = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(c, back);
        // The fold is identical after a round-trip.
        assert_eq!(c.properties(), back.properties());
    }
}
