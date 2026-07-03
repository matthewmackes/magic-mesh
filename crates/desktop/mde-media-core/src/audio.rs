//! MEDIA-3: the typed audio-processing config that folds to mpv's audio path.
//!
//! Design lock (`docs/design/mesh-media-player.md`, Q5/Q14): audio leaves mpv on
//! the seat's **`PipeWire`** server, and the processing chain — a graphic **EQ**,
//! extra **audio filters**, **loudness normalization / `ReplayGain`**, and
//! **gapless** — rides mpv's own audio-filter (`af`) graph plus a handful of mpv
//! properties. §6: this is *glue* — we describe the chain and compile it to the
//! strings mpv already understands; no DSP is reimplemented here.
//!
//! The load-bearing, unit-tested core is the **fold**: an [`AudioConfig`] compiles
//! deterministically to
//!
//! - an [`af_graph`](AudioConfig::af_graph) string — the ordered filter chain
//!   (EQ bands → loudness → user filters), the value of mpv's `af` property; and
//! - a [`properties`](AudioConfig::properties) list — `ao` (the `PipeWire` ao),
//!   optional `audio-device`, `replaygain*`, and `gapless-audio`.
//!
//! [`crate::MediaEngine::apply_audio_config`] applies both to the engine; the real
//! [`MpvEngine`](crate::mpv::MpvEngine) sets them as mpv properties, and
//! [`FakeMpv`](crate::FakeMpv) records them so the fold is asserted with no system
//! libmpv. The *audible* result over a real `PipeWire` seat is honest-gated to the
//! `mpv`-feature real-clip smoke, exactly like MEDIA-1's decode path.

use serde::{Deserialize, Serialize};

/// Format an `f64` for an mpv property/filter argument.
///
/// Rust's `f64` `Display` already drops the fraction for whole numbers
/// (`1000.0` → `"1000"`, `-4.5` → `"-4.5"`), which keeps the folded strings
/// stable + readable; this only folds a stray `-0.0` back to `0.0` so a zeroed
/// gain never renders as `"-0"`.
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

/// The mpv audio-output driver — the seat's audio path.
///
/// Design Q5 locks **`PipeWire`** as the seat audio server, so that is the default;
/// [`Auto`](Self::Auto) lets mpv probe (emitting no `ao`), and
/// [`Custom`](Self::Custom) names any other mpv ao (`alsa`, `pulse`, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AudioDriver {
    /// mpv's native `PipeWire` ao (`ao=pipewire`) — the seat default.
    #[default]
    PipeWire,
    /// Let mpv auto-probe the ao (no explicit `ao` property is set).
    Auto,
    /// Any other mpv ao by name (`alsa`, `pulse`, `jack`, …).
    Custom(String),
}

impl AudioDriver {
    /// The `ao` property value to set, or [`None`] to leave mpv auto-probing.
    fn ao_property(&self) -> Option<String> {
        match self {
            Self::PipeWire => Some("pipewire".to_owned()),
            Self::Auto => None,
            Self::Custom(name) => Some(name.clone()),
        }
    }
}

/// Where mpv sends audio: a [driver](AudioDriver) plus an optional specific device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AudioOutput {
    /// The ao driver (default: `PipeWire` — the seat audio server).
    pub driver: AudioDriver,
    /// A specific ao device to select (`audio-device`), or [`None`] for the
    /// driver's default sink.
    pub device: Option<String>,
}

/// One band of the graphic EQ — a peaking biquad.
///
/// Folds to ffmpeg's `equalizer` audio filter (a single peaking-EQ section):
/// `equalizer=f=<freq>:t=q:w=<q>:g=<gain>`. Several bands chained across the
/// spectrum make the graphic equalizer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EqBand {
    /// Centre frequency in Hz.
    pub freq_hz: f64,
    /// Boost (`+`) or cut (`-`) at the centre, in dB.
    pub gain_db: f64,
    /// The band width as a Q factor (higher = narrower).
    pub q: f64,
}

impl EqBand {
    /// The ISO octave centre frequencies of a classic 10-band graphic EQ (Hz).
    pub const ISO_10_BAND_HZ: [f64; 10] = [
        31.25, 62.5, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
    ];

    /// A reasonable Q for octave-spaced graphic-EQ bands (~1 octave wide).
    pub const OCTAVE_Q: f64 = 1.41;

    /// A band at `freq_hz` with `gain_db` boost/cut and width `q`.
    #[must_use]
    pub const fn new(freq_hz: f64, gain_db: f64, q: f64) -> Self {
        Self {
            freq_hz,
            gain_db,
            q,
        }
    }

    /// Build the classic 10-band graphic EQ from per-band gains (dB), placed at
    /// the [ISO octave centres](Self::ISO_10_BAND_HZ) with the octave
    /// [`Q`](Self::OCTAVE_Q).
    #[must_use]
    pub fn iso_10_band(gains_db: [f64; 10]) -> Vec<Self> {
        Self::ISO_10_BAND_HZ
            .iter()
            .zip(gains_db)
            .map(|(&freq_hz, gain_db)| Self::new(freq_hz, gain_db, Self::OCTAVE_Q))
            .collect()
    }

    /// Fold this band to its `equalizer=…` mpv/ffmpeg audio-filter string.
    fn to_af(self) -> String {
        format!(
            "equalizer=f={}:t=q:w={}:g={}",
            fmt_num(self.freq_hz),
            fmt_num(self.q),
            fmt_num(self.gain_db)
        )
    }
}

/// `ReplayGain` mode — mpv's `replaygain` property (applies the file's embedded
/// `ReplayGain` tags as a volume gain; no live measurement).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReplayGainMode {
    /// No `ReplayGain` (`replaygain=no`).
    #[default]
    Off,
    /// Per-track `ReplayGain` (`replaygain=track`).
    Track,
    /// Per-album `ReplayGain` (`replaygain=album`).
    Album,
}

impl ReplayGainMode {
    /// The `replaygain` property value.
    const fn as_mpv(self) -> &'static str {
        match self {
            Self::Off => "no",
            Self::Track => "track",
            Self::Album => "album",
        }
    }
}

/// Live loudness normalization, folded into the `af` graph.
///
/// Complements [`ReplayGainMode`] (which uses embedded tags): this *measures* and
/// corrects loudness in the filter chain.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LoudnessNorm {
    /// No loudness normalization filter.
    #[default]
    Off,
    /// EBU R128 normalization via ffmpeg's `loudnorm`
    /// (`loudnorm=I=<lufs>:TP=<dbtp>:LRA=<lu>`).
    Ebu {
        /// Integrated-loudness target in LUFS (e.g. `-16.0`).
        target_lufs: f64,
        /// Maximum true peak in dBTP (e.g. `-1.5`).
        true_peak_db: f64,
        /// Target loudness range in LU (e.g. `11.0`).
        range_lu: f64,
    },
    /// Dynamic loudness normalization via ffmpeg's `dynaudnorm` (defaults).
    Dynamic,
}

impl LoudnessNorm {
    /// Fold to an `af` filter string, or [`None`] when off.
    fn to_af(self) -> Option<String> {
        match self {
            Self::Off => None,
            Self::Ebu {
                target_lufs,
                true_peak_db,
                range_lu,
            } => Some(format!(
                "loudnorm=I={}:TP={}:LRA={}",
                fmt_num(target_lufs),
                fmt_num(true_peak_db),
                fmt_num(range_lu)
            )),
            Self::Dynamic => Some("dynaudnorm".to_owned()),
        }
    }
}

/// An extra mpv/ffmpeg audio filter appended verbatim to the `af` graph.
///
/// The escape hatch for filters MEDIA-3 does not model first-class (e.g.
/// `acompressor`, `aecho`). Folds to `name` or `name=<args>`; the caller supplies
/// mpv-valid `args`, since escaping arbitrary filter syntax is mpv's job (§6 — we
/// do not re-parse the af mini-language).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioFilter {
    /// The mpv/ffmpeg filter name (`acompressor`, `aecho`, …).
    pub name: String,
    /// The filter's argument string (mpv `k=v:k=v` form), if any.
    pub args: Option<String>,
}

impl AudioFilter {
    /// A filter with no arguments.
    #[must_use]
    pub const fn bare(name: String) -> Self {
        Self { name, args: None }
    }

    /// Fold to its `af` chain entry.
    fn to_af(&self) -> String {
        match &self.args {
            Some(args) if !args.is_empty() => format!("{}={}", self.name, args),
            _ => self.name.clone(),
        }
    }
}

/// The typed audio-processing configuration for the [`Player`](crate::Player).
///
/// It folds — deterministically and without a real mpv — to the mpv `af` graph
/// ([`af_graph`](Self::af_graph)) plus the ao / `ReplayGain` / gapless
/// [`properties`](Self::properties). [`Player::set_audio_config`] applies it.
///
/// [`Player::set_audio_config`]: crate::Player::set_audio_config
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioConfig {
    /// The ao driver + device (default: the `PipeWire` seat sink).
    pub output: AudioOutput,
    /// Graphic-EQ bands (empty = flat), folded first into the `af` graph.
    pub eq: Vec<EqBand>,
    /// Live loudness normalization filter (folded after the EQ).
    pub loudness: LoudnessNorm,
    /// Tag-based `ReplayGain` mode (an mpv property, not an `af` filter).
    pub replaygain: ReplayGainMode,
    /// `ReplayGain` pre-amp in dB (mpv `replaygain-preamp`; emitted only when
    /// non-zero).
    pub replaygain_preamp_db: f64,
    /// Prevent `ReplayGain` from clipping (mpv `replaygain-clip`; emitted only when
    /// set).
    pub replaygain_clip: bool,
    /// Extra user audio filters, appended to the `af` graph after loudness.
    pub filters: Vec<AudioFilter>,
    /// Gapless audio across playlist items (mpv `gapless-audio`).
    pub gapless: bool,
}

impl AudioConfig {
    /// The default config: `PipeWire` out, flat EQ, no loudness/`ReplayGain`, gapless
    /// on. Matches mpv's own defaults except for pinning the `PipeWire` ao and
    /// enabling gapless (design Q9), so it need not be applied until changed.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            output: AudioOutput {
                driver: AudioDriver::PipeWire,
                device: None,
            },
            eq: Vec::new(),
            loudness: LoudnessNorm::Off,
            replaygain: ReplayGainMode::Off,
            replaygain_preamp_db: 0.0,
            replaygain_clip: false,
            filters: Vec::new(),
            gapless: true,
        }
    }

    /// Compile the ordered `af` filter graph: EQ bands, then loudness, then the
    /// user filters, joined with `,` (mpv's chain separator).
    ///
    /// An empty string means "no filters" — applying it clears mpv's `af` chain.
    #[must_use]
    pub fn af_graph(&self) -> String {
        let mut parts: Vec<String> = self.eq.iter().map(|b| b.to_af()).collect();
        parts.extend(self.loudness.to_af());
        parts.extend(self.filters.iter().map(AudioFilter::to_af));
        parts.join(",")
    }

    /// Compile the non-`af` mpv properties this config sets, in a stable order:
    /// `ao` (unless auto), optional `audio-device`, `replaygain`, optional
    /// `replaygain-preamp`/`replaygain-clip`, and `gapless-audio`.
    #[must_use]
    pub fn properties(&self) -> Vec<(String, String)> {
        let mut props = Vec::new();
        if let Some(ao) = self.output.driver.ao_property() {
            props.push(("ao".to_owned(), ao));
        }
        if let Some(device) = &self.output.device {
            props.push(("audio-device".to_owned(), device.clone()));
        }
        props.push(("replaygain".to_owned(), self.replaygain.as_mpv().to_owned()));
        if self.replaygain_preamp_db.abs() > f64::EPSILON {
            props.push((
                "replaygain-preamp".to_owned(),
                fmt_num(self.replaygain_preamp_db),
            ));
        }
        if self.replaygain_clip {
            props.push(("replaygain-clip".to_owned(), "yes".to_owned()));
        }
        props.push(("gapless-audio".to_owned(), yes_no(self.gapless).to_owned()));
        props
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_pipewire_flat_gapless() {
        let cfg = AudioConfig::new();
        // No filters at all → empty af graph (clears mpv's chain).
        assert_eq!(cfg.af_graph(), "");
        // PipeWire ao pinned, ReplayGain off, gapless on.
        assert_eq!(
            cfg.properties(),
            vec![
                ("ao".to_owned(), "pipewire".to_owned()),
                ("replaygain".to_owned(), "no".to_owned()),
                ("gapless-audio".to_owned(), "yes".to_owned()),
            ]
        );
    }

    #[test]
    fn single_eq_band_folds_to_equalizer_filter() {
        let cfg = AudioConfig {
            eq: vec![EqBand::new(1000.0, 3.0, 1.0)],
            ..AudioConfig::new()
        };
        assert_eq!(cfg.af_graph(), "equalizer=f=1000:t=q:w=1:g=3");
    }

    #[test]
    fn ten_band_graphic_eq_places_iso_centres_in_order() {
        let gains = [2.0, 1.0, 0.0, -1.0, -2.0, 0.0, 1.0, 2.0, 3.0, -4.5];
        let cfg = AudioConfig {
            eq: EqBand::iso_10_band(gains),
            ..AudioConfig::new()
        };
        let graph = cfg.af_graph();
        let bands: Vec<&str> = graph.split(',').collect();
        assert_eq!(bands.len(), 10);
        // First band at 31.25 Hz with +2 dB, last at 16 kHz with -4.5 dB, at the
        // octave Q.
        assert_eq!(bands[0], "equalizer=f=31.25:t=q:w=1.41:g=2");
        assert_eq!(bands[9], "equalizer=f=16000:t=q:w=1.41:g=-4.5");
        // A zeroed band renders g=0, never g=-0.
        assert_eq!(bands[2], "equalizer=f=125:t=q:w=1.41:g=0");
    }

    #[test]
    fn loudness_ebu_folds_after_eq() {
        let cfg = AudioConfig {
            eq: vec![EqBand::new(100.0, -2.0, 1.0)],
            loudness: LoudnessNorm::Ebu {
                target_lufs: -16.0,
                true_peak_db: -1.5,
                range_lu: 11.0,
            },
            ..AudioConfig::new()
        };
        assert_eq!(
            cfg.af_graph(),
            "equalizer=f=100:t=q:w=1:g=-2,loudnorm=I=-16:TP=-1.5:LRA=11"
        );
    }

    #[test]
    fn dynamic_loudness_folds_to_dynaudnorm() {
        let cfg = AudioConfig {
            loudness: LoudnessNorm::Dynamic,
            ..AudioConfig::new()
        };
        assert_eq!(cfg.af_graph(), "dynaudnorm");
    }

    #[test]
    fn user_filters_append_after_loudness_with_and_without_args() {
        let cfg = AudioConfig {
            loudness: LoudnessNorm::Dynamic,
            filters: vec![
                AudioFilter {
                    name: "acompressor".to_owned(),
                    args: Some("ratio=4".to_owned()),
                },
                AudioFilter::bare("aphaser".to_owned()),
            ],
            ..AudioConfig::new()
        };
        assert_eq!(cfg.af_graph(), "dynaudnorm,acompressor=ratio=4,aphaser");
    }

    #[test]
    fn replaygain_modes_fold_to_property() {
        for (mode, expected) in [
            (ReplayGainMode::Off, "no"),
            (ReplayGainMode::Track, "track"),
            (ReplayGainMode::Album, "album"),
        ] {
            let cfg = AudioConfig {
                replaygain: mode,
                ..AudioConfig::new()
            };
            let rg = cfg
                .properties()
                .into_iter()
                .find(|(k, _)| k == "replaygain")
                .map(|(_, v)| v);
            assert_eq!(rg.as_deref(), Some(expected));
        }
    }

    #[test]
    fn replaygain_preamp_and_clip_emitted_only_when_set() {
        // Preamp of 0 + no clip protection → neither property present.
        let flat = AudioConfig {
            replaygain: ReplayGainMode::Track,
            ..AudioConfig::new()
        };
        let flat_keys: Vec<String> = flat.properties().into_iter().map(|(k, _)| k).collect();
        assert!(!flat_keys.iter().any(|k| k == "replaygain-preamp"));
        assert!(!flat_keys.iter().any(|k| k == "replaygain-clip"));

        // Non-zero preamp + clip protection → both present, preamp formatted.
        let tuned = AudioConfig {
            replaygain: ReplayGainMode::Album,
            replaygain_preamp_db: -3.0,
            replaygain_clip: true,
            ..AudioConfig::new()
        };
        let props = tuned.properties();
        assert!(props.contains(&("replaygain-preamp".to_owned(), "-3".to_owned())));
        assert!(props.contains(&("replaygain-clip".to_owned(), "yes".to_owned())));
    }

    #[test]
    fn gapless_off_and_custom_output_device() {
        let cfg = AudioConfig {
            output: AudioOutput {
                driver: AudioDriver::Custom("alsa".to_owned()),
                device: Some("alsa/hw:1,0".to_owned()),
            },
            gapless: false,
            ..AudioConfig::new()
        };
        let props = cfg.properties();
        assert!(props.contains(&("ao".to_owned(), "alsa".to_owned())));
        assert!(props.contains(&("audio-device".to_owned(), "alsa/hw:1,0".to_owned())));
        assert!(props.contains(&("gapless-audio".to_owned(), "no".to_owned())));
    }

    #[test]
    fn auto_driver_emits_no_ao() {
        let cfg = AudioConfig {
            output: AudioOutput {
                driver: AudioDriver::Auto,
                device: None,
            },
            ..AudioConfig::new()
        };
        assert!(!cfg.properties().iter().any(|(k, _)| k == "ao"));
    }

    #[test]
    fn config_round_trips_through_serde() {
        let cfg = AudioConfig {
            eq: EqBand::iso_10_band([1.0; 10]),
            loudness: LoudnessNorm::Ebu {
                target_lufs: -14.0,
                true_peak_db: -1.0,
                range_lu: 9.0,
            },
            replaygain: ReplayGainMode::Album,
            replaygain_preamp_db: 2.5,
            replaygain_clip: true,
            filters: vec![AudioFilter::bare("aphaser".to_owned())],
            gapless: false,
            ..AudioConfig::new()
        };
        let json = serde_json::to_string(&cfg).expect("serialize");
        let back: AudioConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cfg, back);
        // The fold is identical after a round-trip.
        assert_eq!(cfg.af_graph(), back.af_graph());
        assert_eq!(cfg.properties(), back.properties());
    }
}
