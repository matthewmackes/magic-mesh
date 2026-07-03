//! MEDIA-4: the typed video-decode + adjustments config that folds to mpv's
//! video path.
//!
//! Design lock (`docs/design/mesh-media-player.md`, Q6/Q13): video decodes with
//! **VA-API and a software fallback**, and the on-screen adjustments — **aspect**
//! override, **zoom**/**pan**, **crop**, **rotate**, **deinterlace**, and extra
//! **video filters** — ride mpv's own decoder selection (`hwdec`), its
//! `video-*` properties, and its video-filter (`vf`) graph. §6: this is *glue* —
//! we describe the decode mode + adjustments and compile them to the strings mpv
//! already understands; no decoder or scaler is reimplemented here.
//!
//! The load-bearing, unit-tested core is the **fold**: a [`VideoConfig`] compiles
//! deterministically to
//!
//! - a [`vf_graph`](VideoConfig::vf_graph) string — the ordered video-filter
//!   chain, the value of mpv's `vf` property; and
//! - a [`properties`](VideoConfig::properties) list — `hwdec` (the VA-API /
//!   software decode mode), `video-aspect-override`, `video-zoom` /
//!   `video-pan-x` / `video-pan-y`, `video-crop`, `video-rotate`, and
//!   `deinterlace`.
//!
//! [`crate::MediaEngine::apply_video_config`] applies both to the engine; the real
//! [`MpvEngine`](crate::mpv::MpvEngine) sets them as mpv properties, and
//! [`FakeMpv`](crate::FakeMpv) records them so the fold is asserted with no system
//! libmpv. Whether VA-API *actually* engages (vs mpv's own software fallback) is a
//! property of the host GPU + driver, so the live hardware-decode path is
//! honest-gated to the `mpv`-feature real-clip smoke on a VA-API GPU — exactly
//! like MEDIA-1's decode path and MEDIA-3's audible `PipeWire` result. The config
//! mapping itself is real and fully tested here.

use serde::{Deserialize, Serialize};

/// Format an `f64` for an mpv property/filter argument.
///
/// Rust's `f64` `Display` already drops the fraction for whole numbers
/// (`16.0` → `"16"`, `-1.5` → `"-1.5"`), which keeps the folded strings stable +
/// readable; this only folds a stray `-0.0` back to `0.0` so a zeroed pan never
/// renders as `"-0"`.
fn fmt_num(x: f64) -> String {
    let x = if x == 0.0 { 0.0 } else { x };
    format!("{x}")
}

/// The hardware-decode mode — mpv's `hwdec` property.
///
/// Design Q6 locks **VA-API with a software fallback**. [`Auto`](Self::Auto) is
/// the default: mpv's `auto-safe` picks a whitelisted hardware decoder (VA-API on
/// a VA-API GPU) and silently falls back to software when none is usable — i.e.
/// "VA-API where available, software otherwise" with no configuration. [`VaApi`]
/// pins VA-API explicitly (mpv still falls back to software internally if the
/// device is absent), and [`Software`](Self::Software) forces pure software
/// decode.
///
/// mpv's own default is `hwdec=no` (software), so the default [`VideoConfig`]
/// actively requests `auto-safe` to honour Q6 — this config must be applied to
/// opt into hardware decode.
///
/// [`VaApi`]: Self::VaApi
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HwDecode {
    /// mpv's `hwdec=auto-safe` — a safe hardware decoder (VA-API on a VA-API GPU)
    /// with an automatic software fallback. The seat default.
    #[default]
    Auto,
    /// mpv's `hwdec=vaapi` — pin VA-API decode (mpv falls back to software
    /// internally if the VA-API device cannot be initialised).
    VaApi,
    /// mpv's `hwdec=no` — force pure software decode.
    Software,
}

impl HwDecode {
    /// The `hwdec` property value.
    const fn as_mpv(self) -> &'static str {
        match self {
            Self::Auto => "auto-safe",
            Self::VaApi => "vaapi",
            Self::Software => "no",
        }
    }
}

/// The display-aspect override — mpv's `video-aspect-override` property.
///
/// [`Auto`](Self::Auto) (the default) folds to `-1`, mpv's "use the container's
/// aspect" sentinel; [`Ratio`](Self::Ratio) forces a display aspect (`16:9`,
/// `4:3`, `2.35:1`, …).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AspectRatio {
    /// Use the container/stream's own aspect (mpv `video-aspect-override=-1`).
    #[default]
    Auto,
    /// Force a display aspect as `width:height` (`video-aspect-override=<w>:<h>`).
    Ratio(f64, f64),
}

impl AspectRatio {
    /// The classic 16:9 widescreen aspect.
    pub const SIXTEEN_NINE: Self = Self::Ratio(16.0, 9.0);
    /// The classic 4:3 fullscreen aspect.
    pub const FOUR_THREE: Self = Self::Ratio(4.0, 3.0);
    /// The 2.35:1 anamorphic "cinemascope" aspect.
    pub const CINEMASCOPE: Self = Self::Ratio(2.35, 1.0);

    /// The `video-aspect-override` property value.
    fn as_mpv(self) -> String {
        match self {
            Self::Auto => "-1".to_owned(),
            Self::Ratio(w, h) => format!("{}:{}", fmt_num(w), fmt_num(h)),
        }
    }
}

/// A quarter-turn video rotation — mpv's `video-rotate` property.
///
/// Restricted to the four right angles (mpv accepts arbitrary degrees, but the
/// adjustments UI rotates in quarter turns); folds to the clockwise degree count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Rotation {
    /// No rotation (`video-rotate=0`).
    #[default]
    None,
    /// Rotate 90° clockwise (`video-rotate=90`).
    Cw90,
    /// Rotate 180° (`video-rotate=180`).
    Cw180,
    /// Rotate 270° clockwise / 90° counter-clockwise (`video-rotate=270`).
    Cw270,
}

impl Rotation {
    /// The clockwise rotation in degrees, the `video-rotate` property value.
    const fn degrees(self) -> u16 {
        match self {
            Self::None => 0,
            Self::Cw90 => 90,
            Self::Cw180 => 180,
            Self::Cw270 => 270,
        }
    }
}

/// The deinterlace mode — mpv's `deinterlace` property.
///
/// mpv inserts the appropriate deinterlacer for the active decode path (the
/// VA-API `vavpp` filter under VA-API, or a software deinterlacer), so this is a
/// mode toggle, not a hand-built filter — §6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Deinterlace {
    /// No deinterlacing (`deinterlace=no`).
    #[default]
    Off,
    /// Always deinterlace (`deinterlace=yes`).
    On,
    /// Deinterlace only when the stream is flagged interlaced (`deinterlace=auto`).
    Auto,
}

impl Deinterlace {
    /// The `deinterlace` property value.
    const fn as_mpv(self) -> &'static str {
        match self {
            Self::Off => "no",
            Self::On => "yes",
            Self::Auto => "auto",
        }
    }
}

/// A rectangular crop of the decoded frame — mpv's `video-crop` property.
///
/// Folds to mpv's `<w>x<h>+<x>+<y>` geometry (a `width`×`height` window offset
/// `x` px from the left and `y` px from the top of the source frame).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Crop {
    /// The crop window width in source pixels.
    pub width: u32,
    /// The crop window height in source pixels.
    pub height: u32,
    /// The left offset of the window in source pixels.
    pub x: u32,
    /// The top offset of the window in source pixels.
    pub y: u32,
}

impl Crop {
    /// A crop `width`×`height` window offset `x`/`y` px from the top-left.
    #[must_use]
    pub const fn new(width: u32, height: u32, x: u32, y: u32) -> Self {
        Self {
            width,
            height,
            x,
            y,
        }
    }

    /// Fold to the `video-crop` geometry string (`<w>x<h>+<x>+<y>`).
    fn to_mpv(self) -> String {
        format!("{}x{}+{}+{}", self.width, self.height, self.x, self.y)
    }
}

/// An extra mpv/ffmpeg video filter appended verbatim to the `vf` graph.
///
/// The escape hatch for filters MEDIA-4 does not model first-class (e.g.
/// `hqdn3d`, `unsharp`, `gradfun`). Folds to `name` or `name=<args>`; the caller
/// supplies mpv-valid `args`, since escaping arbitrary filter syntax is mpv's job
/// (§6 — we do not re-parse the vf mini-language).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoFilter {
    /// The mpv/ffmpeg filter name (`hqdn3d`, `unsharp`, …).
    pub name: String,
    /// The filter's argument string (mpv `k=v:k=v` form), if any.
    pub args: Option<String>,
}

impl VideoFilter {
    /// A filter with no arguments.
    #[must_use]
    pub const fn bare(name: String) -> Self {
        Self { name, args: None }
    }

    /// Fold to its `vf` chain entry.
    fn to_vf(&self) -> String {
        match &self.args {
            Some(args) if !args.is_empty() => format!("{}={}", self.name, args),
            _ => self.name.clone(),
        }
    }
}

/// The typed video decode + adjustments configuration for the
/// [`Player`](crate::Player).
///
/// It folds — deterministically and without a real mpv — to the mpv `vf` graph
/// ([`vf_graph`](Self::vf_graph)) plus the `hwdec` / `video-*` / `deinterlace`
/// [`properties`](Self::properties). [`Player::set_video_config`] applies it.
///
/// [`Player::set_video_config`]: crate::Player::set_video_config
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoConfig {
    /// The hardware-decode mode (default: [`HwDecode::Auto`] — VA-API where
    /// available, software otherwise).
    pub hwdec: HwDecode,
    /// The display-aspect override (default: [`AspectRatio::Auto`] — the
    /// container's aspect).
    pub aspect: AspectRatio,
    /// Zoom as a log2 factor (mpv `video-zoom`: `0` = none, `1` = 2×, `-1` = ½×);
    /// emitted only when non-zero.
    pub zoom: f64,
    /// Horizontal pan as a fraction of the frame (mpv `video-pan-x`); emitted only
    /// when non-zero.
    pub pan_x: f64,
    /// Vertical pan as a fraction of the frame (mpv `video-pan-y`); emitted only
    /// when non-zero.
    pub pan_y: f64,
    /// An optional rectangular crop (mpv `video-crop`); emitted only when set.
    pub crop: Option<Crop>,
    /// Quarter-turn rotation (mpv `video-rotate`).
    pub rotate: Rotation,
    /// Deinterlace mode (mpv `deinterlace`).
    pub deinterlace: Deinterlace,
    /// Extra user video filters, appended to the `vf` graph in order.
    pub filters: Vec<VideoFilter>,
}

impl VideoConfig {
    /// The default config: `auto-safe` decode (VA-API with software fallback),
    /// container aspect, no zoom/pan/crop/rotation, deinterlace off, no filters.
    ///
    /// This differs from mpv's own defaults by requesting hardware decode
    /// (`hwdec=auto-safe` vs mpv's `no`), per design Q6 — so unlike a flat audio
    /// config it *is* meaningful to apply even untouched.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            hwdec: HwDecode::Auto,
            aspect: AspectRatio::Auto,
            zoom: 0.0,
            pan_x: 0.0,
            pan_y: 0.0,
            crop: None,
            rotate: Rotation::None,
            deinterlace: Deinterlace::Off,
            filters: Vec::new(),
        }
    }

    /// Compile the ordered `vf` filter graph from the user filters, joined with
    /// `,` (mpv's chain separator).
    ///
    /// An empty string means "no filters" — applying it clears mpv's `vf` chain.
    #[must_use]
    pub fn vf_graph(&self) -> String {
        self.filters
            .iter()
            .map(VideoFilter::to_vf)
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Compile the non-`vf` mpv properties this config sets, in a stable order:
    /// `hwdec`, `video-aspect-override`, optional `video-zoom`/`video-pan-x`/
    /// `video-pan-y`, optional `video-crop`, `video-rotate`, and `deinterlace`.
    ///
    /// `hwdec`, `video-aspect-override`, `video-rotate`, and `deinterlace` are
    /// always emitted (each carries a neutral value — `auto-safe`, `-1`, `0`,
    /// `no`), so applying a config re-establishes those primary controls; the
    /// finer zoom/pan/crop adjustments are emitted only when non-neutral, matching
    /// [`AudioConfig`](crate::AudioConfig)'s conditional properties.
    #[must_use]
    pub fn properties(&self) -> Vec<(String, String)> {
        let mut props = vec![
            ("hwdec".to_owned(), self.hwdec.as_mpv().to_owned()),
            ("video-aspect-override".to_owned(), self.aspect.as_mpv()),
        ];
        if self.zoom.abs() > f64::EPSILON {
            props.push(("video-zoom".to_owned(), fmt_num(self.zoom)));
        }
        if self.pan_x.abs() > f64::EPSILON {
            props.push(("video-pan-x".to_owned(), fmt_num(self.pan_x)));
        }
        if self.pan_y.abs() > f64::EPSILON {
            props.push(("video-pan-y".to_owned(), fmt_num(self.pan_y)));
        }
        if let Some(crop) = self.crop {
            props.push(("video-crop".to_owned(), crop.to_mpv()));
        }
        props.push(("video-rotate".to_owned(), self.rotate.degrees().to_string()));
        props.push((
            "deinterlace".to_owned(),
            self.deinterlace.as_mpv().to_owned(),
        ));
        props
    }
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_auto_safe_hwdec_container_aspect() {
        let cfg = VideoConfig::new();
        // No user filters → empty vf graph (clears mpv's chain).
        assert_eq!(cfg.vf_graph(), "");
        // Default requests hardware decode with software fallback (Q6), container
        // aspect, upright, not deinterlaced.
        assert_eq!(
            cfg.properties(),
            vec![
                ("hwdec".to_owned(), "auto-safe".to_owned()),
                ("video-aspect-override".to_owned(), "-1".to_owned()),
                ("video-rotate".to_owned(), "0".to_owned()),
                ("deinterlace".to_owned(), "no".to_owned()),
            ]
        );
    }

    #[test]
    fn hwdec_modes_fold_to_property() {
        for (mode, expected) in [
            (HwDecode::Auto, "auto-safe"),
            (HwDecode::VaApi, "vaapi"),
            (HwDecode::Software, "no"),
        ] {
            let cfg = VideoConfig {
                hwdec: mode,
                ..VideoConfig::new()
            };
            let hwdec = cfg
                .properties()
                .into_iter()
                .find(|(k, _)| k == "hwdec")
                .map(|(_, v)| v);
            assert_eq!(hwdec.as_deref(), Some(expected));
        }
    }

    #[test]
    fn vaapi_decode_pins_the_property() {
        // The VA-API pin the acceptance calls out: hwdec=vaapi appears verbatim.
        let cfg = VideoConfig {
            hwdec: HwDecode::VaApi,
            ..VideoConfig::new()
        };
        assert!(cfg
            .properties()
            .contains(&("hwdec".to_owned(), "vaapi".to_owned())));
    }

    #[test]
    fn aspect_ratio_folds_to_width_colon_height() {
        let sixteen_nine = VideoConfig {
            aspect: AspectRatio::SIXTEEN_NINE,
            ..VideoConfig::new()
        };
        assert!(sixteen_nine
            .properties()
            .contains(&("video-aspect-override".to_owned(), "16:9".to_owned())));

        let cinemascope = VideoConfig {
            aspect: AspectRatio::CINEMASCOPE,
            ..VideoConfig::new()
        };
        // Decimal ratios keep their fraction; the denominator drops its `.0`.
        assert!(cinemascope
            .properties()
            .contains(&("video-aspect-override".to_owned(), "2.35:1".to_owned())));
    }

    #[test]
    fn auto_aspect_folds_to_minus_one_sentinel() {
        let cfg = VideoConfig::new();
        assert!(cfg
            .properties()
            .contains(&("video-aspect-override".to_owned(), "-1".to_owned())));
    }

    #[test]
    fn rotation_folds_to_clockwise_degrees() {
        for (rot, deg) in [
            (Rotation::None, "0"),
            (Rotation::Cw90, "90"),
            (Rotation::Cw180, "180"),
            (Rotation::Cw270, "270"),
        ] {
            let cfg = VideoConfig {
                rotate: rot,
                ..VideoConfig::new()
            };
            assert!(cfg
                .properties()
                .contains(&("video-rotate".to_owned(), deg.to_owned())));
        }
    }

    #[test]
    fn deinterlace_modes_fold_to_property() {
        for (mode, expected) in [
            (Deinterlace::Off, "no"),
            (Deinterlace::On, "yes"),
            (Deinterlace::Auto, "auto"),
        ] {
            let cfg = VideoConfig {
                deinterlace: mode,
                ..VideoConfig::new()
            };
            assert!(cfg
                .properties()
                .contains(&("deinterlace".to_owned(), expected.to_owned())));
        }
    }

    #[test]
    fn crop_folds_to_geometry_and_only_when_set() {
        // No crop → no video-crop property at all.
        let none = VideoConfig::new();
        assert!(!none.properties().iter().any(|(k, _)| k == "video-crop"));

        // A crop window folds to <w>x<h>+<x>+<y>.
        let cropped = VideoConfig {
            crop: Some(Crop::new(1280, 720, 40, 20)),
            ..VideoConfig::new()
        };
        assert!(cropped
            .properties()
            .contains(&("video-crop".to_owned(), "1280x720+40+20".to_owned())));
    }

    #[test]
    fn zoom_and_pan_emitted_only_when_non_zero() {
        // Neutral zoom/pan → none of the three fine properties present.
        let flat = VideoConfig::new();
        let flat_keys: Vec<String> = flat.properties().into_iter().map(|(k, _)| k).collect();
        assert!(!flat_keys.iter().any(|k| k == "video-zoom"));
        assert!(!flat_keys.iter().any(|k| k == "video-pan-x"));
        assert!(!flat_keys.iter().any(|k| k == "video-pan-y"));

        // Non-zero zoom + pan → all three present, formatted (no stray `-0`).
        let tuned = VideoConfig {
            zoom: 0.5,
            pan_x: -0.25,
            pan_y: 0.0,
            ..VideoConfig::new()
        };
        let props = tuned.properties();
        assert!(props.contains(&("video-zoom".to_owned(), "0.5".to_owned())));
        assert!(props.contains(&("video-pan-x".to_owned(), "-0.25".to_owned())));
        // pan_y is exactly zero → still omitted.
        assert!(!props.iter().any(|(k, _)| k == "video-pan-y"));
    }

    #[test]
    fn single_video_filter_folds_to_vf_graph() {
        let cfg = VideoConfig {
            filters: vec![VideoFilter {
                name: "hqdn3d".to_owned(),
                args: Some("4:3:6:4.5".to_owned()),
            }],
            ..VideoConfig::new()
        };
        assert_eq!(cfg.vf_graph(), "hqdn3d=4:3:6:4.5");
    }

    #[test]
    fn video_filters_chain_in_order_with_and_without_args() {
        let cfg = VideoConfig {
            filters: vec![
                VideoFilter {
                    name: "unsharp".to_owned(),
                    args: Some("5:5:1.0".to_owned()),
                },
                VideoFilter::bare("vflip".to_owned()),
            ],
            ..VideoConfig::new()
        };
        assert_eq!(cfg.vf_graph(), "unsharp=5:5:1.0,vflip");
    }

    #[test]
    fn full_adjustment_stack_folds_together() {
        // Every adjustment at once: VA-API decode, forced aspect, zoom+pan, crop,
        // rotation, deinterlace, and a denoise filter — the full Q13 set.
        let cfg = VideoConfig {
            hwdec: HwDecode::VaApi,
            aspect: AspectRatio::SIXTEEN_NINE,
            zoom: 1.0,
            pan_x: 0.1,
            pan_y: -0.1,
            crop: Some(Crop::new(1920, 800, 0, 140)),
            rotate: Rotation::Cw90,
            deinterlace: Deinterlace::On,
            filters: vec![VideoFilter::bare("gradfun".to_owned())],
        };
        assert_eq!(cfg.vf_graph(), "gradfun");
        assert_eq!(
            cfg.properties(),
            vec![
                ("hwdec".to_owned(), "vaapi".to_owned()),
                ("video-aspect-override".to_owned(), "16:9".to_owned()),
                ("video-zoom".to_owned(), "1".to_owned()),
                ("video-pan-x".to_owned(), "0.1".to_owned()),
                ("video-pan-y".to_owned(), "-0.1".to_owned()),
                ("video-crop".to_owned(), "1920x800+0+140".to_owned()),
                ("video-rotate".to_owned(), "90".to_owned()),
                ("deinterlace".to_owned(), "yes".to_owned()),
            ]
        );
    }

    #[test]
    fn config_round_trips_through_serde() {
        let cfg = VideoConfig {
            hwdec: HwDecode::VaApi,
            aspect: AspectRatio::CINEMASCOPE,
            zoom: -0.5,
            pan_x: 0.2,
            pan_y: 0.3,
            crop: Some(Crop::new(720, 480, 8, 0)),
            rotate: Rotation::Cw270,
            deinterlace: Deinterlace::Auto,
            filters: vec![VideoFilter {
                name: "hqdn3d".to_owned(),
                args: Some("2".to_owned()),
            }],
        };
        let json = serde_json::to_string(&cfg).expect("serialize");
        let back: VideoConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cfg, back);
        // The fold is identical after a round-trip.
        assert_eq!(cfg.vf_graph(), back.vf_graph());
        assert_eq!(cfg.properties(), back.properties());
    }
}
