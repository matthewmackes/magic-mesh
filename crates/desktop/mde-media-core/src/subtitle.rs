//! MEDIA-5: the typed subtitle + multi-track-selection config that folds to mpv's
//! subtitle / track path.
//!
//! Design lock (`docs/design/mesh-media-player.md`, Q7/Q8): the player carries
//! **embedded + external (.srt/.ass) subtitles** with ASS styling / positioning /
//! delay, plus **full audio / subtitle / video track selection with language
//! labels**. All of it rides mpv's own `sub-add` command, its `sub-*` properties,
//! and the `aid`/`sid`/`vid` track-selection properties. §6: this is *glue* — we
//! describe which tracks are active + how subtitles look and compile that to the
//! strings/commands mpv already understands; no subtitle renderer or demuxer is
//! reimplemented here.
//!
//! Two load-bearing, unit-tested folds live here:
//!
//! - [`TrackSelection`] compiles to the `aid`/`vid`/`sid`
//!   [`properties`](TrackSelection::properties) that pick one enumerated
//!   [`Track`](crate::Track) per kind. [`track_by_language`] resolves a BCP-47 /
//!   ISO language label against the enumerated tracks — the "with language labels"
//!   acceptance.
//! - [`SubtitleConfig`] compiles to the `sub-add`
//!   [`commands`](SubtitleConfig::commands) that load external subtitle files and
//!   the `sub-*` styling [`properties`](SubtitleConfig::properties)
//!   (position / scale / delay / ASS-override / visibility).
//!
//! [`crate::MediaEngine::apply_track_selection`] /
//! [`apply_subtitle_config`](crate::MediaEngine::apply_subtitle_config) apply
//! them; the real [`MpvEngine`](crate::mpv::MpvEngine) runs the commands + sets the
//! properties, and [`FakeMpv`](crate::FakeMpv) records them so the fold is asserted
//! with no system libmpv. The *rendered* subtitle over a real seat is honest-gated
//! to the `mpv`-feature real-clip smoke, exactly like MEDIA-1's decode path.

use serde::{Deserialize, Serialize};

use crate::engine::{Track, TrackKind};

/// Format an `f64` for an mpv property argument (stable, no stray `-0`).
///
/// Mirrors [`crate::audio`]/[`crate::video`]: `f64`'s `Display` already drops the
/// fraction for whole numbers (`0.0` → `"0"`, `-2.5` → `"-2.5"`); this only folds
/// a `-0.0` back to `0.0`.
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

/// How a single track kind (`aid`/`vid`/`sid`) is selected.
///
/// mpv's per-kind selection property accepts an id, the string `auto`
/// (mpv chooses), or `no` (the track kind is disabled).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrackSelect {
    /// Let mpv pick (its default track selection) — folds to `auto`.
    #[default]
    Auto,
    /// Disable this track kind entirely — folds to `no` (e.g. subtitles off).
    Off,
    /// Select the enumerated [`Track::id`] of this kind — folds to `<id>`.
    Id(i64),
}

impl TrackSelect {
    /// The mpv property value (`auto` / `no` / the id).
    #[must_use]
    pub fn as_mpv(self) -> String {
        match self {
            Self::Auto => "auto".to_owned(),
            Self::Off => "no".to_owned(),
            Self::Id(id) => id.to_string(),
        }
    }
}

/// The selected audio / video / subtitle track — mpv's `aid` / `vid` / `sid`.
///
/// This is the "full audio/subtitle/video track selection" of the acceptance; the
/// ids reference the enumerated [`Track`]s mpv reports in `track-list` (MEDIA-1).
/// [`track_by_language`] turns a language label into the id to select here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TrackSelection {
    /// The active audio track (`aid`).
    pub audio: TrackSelect,
    /// The active video track (`vid`).
    pub video: TrackSelect,
    /// The active subtitle track (`sid`).
    pub subtitle: TrackSelect,
}

impl TrackSelection {
    /// The default selection: mpv chooses every track kind (`auto`).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            audio: TrackSelect::Auto,
            video: TrackSelect::Auto,
            subtitle: TrackSelect::Auto,
        }
    }

    /// Compile the `aid` / `vid` / `sid` mpv properties, always in that order.
    ///
    /// All three are always emitted (each carries the neutral `auto` when
    /// untouched), so applying a selection re-establishes every track control —
    /// matching [`VideoConfig`](crate::VideoConfig)'s always-present primaries.
    #[must_use]
    pub fn properties(&self) -> Vec<(String, String)> {
        vec![
            ("aid".to_owned(), self.audio.as_mpv()),
            ("vid".to_owned(), self.video.as_mpv()),
            ("sid".to_owned(), self.subtitle.as_mpv()),
        ]
    }
}

/// Find the [`Track::id`] of the first track of `kind` whose language tag matches
/// `lang` (ASCII case-insensitive), preferring a container-default track.
///
/// This is the "with language labels" selection: given the enumerated
/// [`Track`]s (MEDIA-1) and a wanted language (`"eng"`, `"jpn"`, …), it returns
/// the id to hand to [`TrackSelect::Id`]. Returns [`None`] when no track of that
/// kind carries the language, so the caller can fall back to [`TrackSelect::Auto`]
/// rather than guess.
#[must_use]
pub fn track_by_language(tracks: &[Track], kind: TrackKind, lang: &str) -> Option<i64> {
    let matches = tracks.iter().filter(|t| {
        t.kind == kind
            && t.lang
                .as_deref()
                .is_some_and(|l| l.eq_ignore_ascii_case(lang))
    });
    // Prefer a track the container marks default; otherwise the first match.
    matches
        .clone()
        .find(|t| t.default)
        .or_else(|| matches.min_by_key(|t| t.id))
        .map(|t| t.id)
}

/// How mpv treats embedded ASS/SSA subtitle styling — mpv's `sub-ass-override`.
///
/// ASS subtitles carry their own styling (font, colour, position); this decides
/// whether the player's `sub-*` overrides win. mpv's own default is `scale`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AssOverride {
    /// Honour the script's styling verbatim (`sub-ass-override=no`).
    No,
    /// Apply the player's `sub-*` overrides on top of the script
    /// (`sub-ass-override=yes`).
    Yes,
    /// mpv's default — apply overrides but keep the script's own scaling
    /// (`sub-ass-override=scale`).
    #[default]
    Scale,
    /// Force the player's plain styling, discarding most script styling
    /// (`sub-ass-override=force`).
    Force,
    /// Strip the script's styling entirely, rendering as plain text
    /// (`sub-ass-override=strip`).
    Strip,
}

impl AssOverride {
    /// The `sub-ass-override` property value.
    const fn as_mpv(self) -> &'static str {
        match self {
            Self::No => "no",
            Self::Yes => "yes",
            Self::Scale => "scale",
            Self::Force => "force",
            Self::Strip => "strip",
        }
    }
}

/// Whether a loaded external subtitle is selected immediately or merely added.
///
/// Folds to the `flags` argument of mpv's `sub-add` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SubLoad {
    /// Add and select the subtitle now (`sub-add <url> select`).
    #[default]
    Select,
    /// Add the subtitle but only auto-select if nothing else is chosen
    /// (`sub-add <url> auto`).
    Auto,
    /// Add the subtitle but do not select it (`sub-add <url> cached`).
    Cached,
}

impl SubLoad {
    /// The `sub-add` flags token.
    const fn as_mpv(self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Auto => "auto",
            Self::Cached => "cached",
        }
    }
}

/// An external subtitle file to load — folds to one mpv `sub-add` command.
///
/// Handles the ".srt/.ass" (and any other mpv-readable) external subtitle of the
/// acceptance. The optional `title`/`lang` become the loaded track's label so it
/// reads with a language label alongside the embedded tracks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalSub {
    /// The path or URL of the subtitle file (`.srt`, `.ass`, `.vtt`, …).
    pub path: String,
    /// Whether to select it now, auto-select, or just cache it.
    pub load: SubLoad,
    /// A human title for the loaded track, if any.
    pub title: Option<String>,
    /// A BCP-47 / ISO language tag for the loaded track, if any (`"eng"`, …).
    pub lang: Option<String>,
}

impl ExternalSub {
    /// An external subtitle at `path`, added + selected, with no explicit
    /// title/lang (mpv derives them from the filename).
    #[must_use]
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            load: SubLoad::Select,
            title: None,
            lang: None,
        }
    }

    /// Fold to the mpv `sub-add` command argument vector (without the command
    /// name): `[<path>, <flags>[, <title>[, <lang>]]]`.
    ///
    /// mpv's `sub-add` is positional, so a `lang` with no `title` still emits an
    /// (empty) title slot to keep the language in the right position.
    #[must_use]
    pub fn command_args(&self) -> Vec<String> {
        let mut args = vec![self.path.clone(), self.load.as_mpv().to_owned()];
        match (&self.title, &self.lang) {
            (Some(title), Some(lang)) => {
                args.push(title.clone());
                args.push(lang.clone());
            }
            (Some(title), None) => args.push(title.clone()),
            (None, Some(lang)) => {
                // Hold the title slot so lang lands in the 4th position.
                args.push(String::new());
                args.push(lang.clone());
            }
            (None, None) => {}
        }
        args
    }
}

/// The typed subtitle configuration for the [`Player`](crate::Player).
///
/// It folds — deterministically and without a real mpv — to the `sub-add`
/// [`commands`](Self::commands) that load external subtitle files plus the `sub-*`
/// styling [`properties`](Self::properties). Which embedded/loaded track is *active*
/// is [`TrackSelection::subtitle`] (`sid`); this type owns loading + look.
/// [`Player::set_subtitle_config`] applies it.
///
/// [`Player::set_subtitle_config`]: crate::Player::set_subtitle_config
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubtitleConfig {
    /// External subtitle files to load (each folds to a `sub-add` command).
    pub external: Vec<ExternalSub>,
    /// Whether subtitles are shown at all (mpv `sub-visibility`).
    pub visible: bool,
    /// How embedded ASS styling is overridden (mpv `sub-ass-override`).
    pub ass_override: AssOverride,
    /// Vertical position, 0 (top) – 100 (default) – 150 (bottom, mpv `sub-pos`);
    /// emitted only when not the default 100.
    pub pos: u8,
    /// Subtitle scale factor (mpv `sub-scale`, default `1.0`); emitted only when
    /// not `1.0`.
    pub scale: f64,
    /// Subtitle delay in seconds (mpv `sub-delay`, `+` = later); emitted only when
    /// non-zero.
    pub delay: f64,
}

impl SubtitleConfig {
    /// mpv's default subtitle vertical position.
    pub const DEFAULT_POS: u8 = 100;

    /// The default config: no external files, subtitles visible, mpv's `scale`
    /// ASS-override mode, default position/scale/delay. Matches mpv's own defaults
    /// except for pinning them explicitly, so it need not be applied until changed.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            external: Vec::new(),
            visible: true,
            ass_override: AssOverride::Scale,
            pos: Self::DEFAULT_POS,
            scale: 1.0,
            delay: 0.0,
        }
    }

    /// Compile the ordered `sub-add` commands (with the command name) for every
    /// external subtitle file, in declared order.
    ///
    /// Each entry is a full argv (`["sub-add", <path>, <flags>, …]`) ready to hand
    /// to mpv's command interface.
    #[must_use]
    pub fn commands(&self) -> Vec<Vec<String>> {
        self.external
            .iter()
            .map(|sub| {
                let mut argv = vec!["sub-add".to_owned()];
                argv.extend(sub.command_args());
                argv
            })
            .collect()
    }

    /// Compile the `sub-*` styling mpv properties.
    ///
    /// `sub-visibility` and `sub-ass-override` are always emitted (each carries a
    /// neutral value, so applying a config re-establishes them); the finer
    /// position / scale / delay are emitted only when non-default — mirroring
    /// [`AudioConfig`](crate::AudioConfig)/[`VideoConfig`](crate::VideoConfig).
    #[must_use]
    pub fn properties(&self) -> Vec<(String, String)> {
        let mut props = vec![
            ("sub-visibility".to_owned(), yes_no(self.visible).to_owned()),
            (
                "sub-ass-override".to_owned(),
                self.ass_override.as_mpv().to_owned(),
            ),
        ];
        if self.pos != Self::DEFAULT_POS {
            props.push(("sub-pos".to_owned(), self.pos.to_string()));
        }
        if (self.scale - 1.0).abs() > f64::EPSILON {
            props.push(("sub-scale".to_owned(), fmt_num(self.scale)));
        }
        if self.delay.abs() > f64::EPSILON {
            props.push(("sub-delay".to_owned(), fmt_num(self.delay)));
        }
        props
    }
}

impl Default for SubtitleConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: i64, kind: TrackKind, lang: Option<&str>, default: bool) -> Track {
        Track {
            id,
            kind,
            title: None,
            lang: lang.map(ToOwned::to_owned),
            codec: None,
            default,
            selected: false,
        }
    }

    // ── track selection ─────────────────────────────────────────────────────

    #[test]
    fn default_selection_is_auto_for_every_kind() {
        let sel = TrackSelection::new();
        assert_eq!(
            sel.properties(),
            vec![
                ("aid".to_owned(), "auto".to_owned()),
                ("vid".to_owned(), "auto".to_owned()),
                ("sid".to_owned(), "auto".to_owned()),
            ]
        );
    }

    #[test]
    fn track_select_folds_to_id_auto_and_no() {
        assert_eq!(TrackSelect::Auto.as_mpv(), "auto");
        assert_eq!(TrackSelect::Off.as_mpv(), "no");
        assert_eq!(TrackSelect::Id(3).as_mpv(), "3");
    }

    #[test]
    fn explicit_selection_folds_ids_and_subtitles_off() {
        let sel = TrackSelection {
            audio: TrackSelect::Id(2),
            video: TrackSelect::Id(1),
            subtitle: TrackSelect::Off,
        };
        assert_eq!(
            sel.properties(),
            vec![
                ("aid".to_owned(), "2".to_owned()),
                ("vid".to_owned(), "1".to_owned()),
                ("sid".to_owned(), "no".to_owned()),
            ]
        );
    }

    // ── language-label selection ────────────────────────────────────────────

    #[test]
    fn track_by_language_matches_case_insensitively() {
        let tracks = vec![
            track(1, TrackKind::Audio, Some("eng"), false),
            track(2, TrackKind::Audio, Some("jpn"), false),
            track(1, TrackKind::Subtitle, Some("ENG"), false),
        ];
        assert_eq!(
            track_by_language(&tracks, TrackKind::Audio, "JPN"),
            Some(2),
            "audio jpn matches case-insensitively"
        );
        assert_eq!(
            track_by_language(&tracks, TrackKind::Subtitle, "eng"),
            Some(1),
            "subtitle eng matches the ENG-tagged sub track"
        );
    }

    #[test]
    fn track_by_language_prefers_default_then_lowest_id() {
        let tracks = vec![
            track(3, TrackKind::Audio, Some("eng"), false),
            track(1, TrackKind::Audio, Some("eng"), false),
            track(2, TrackKind::Audio, Some("eng"), true), // container default
        ];
        // The default eng track wins even though it is not the lowest id.
        assert_eq!(track_by_language(&tracks, TrackKind::Audio, "eng"), Some(2));
    }

    #[test]
    fn track_by_language_none_when_absent() {
        let tracks = vec![track(1, TrackKind::Audio, Some("eng"), false)];
        assert_eq!(track_by_language(&tracks, TrackKind::Audio, "fra"), None);
        assert_eq!(track_by_language(&tracks, TrackKind::Subtitle, "eng"), None);
    }

    // ── external subtitle sub-add commands ──────────────────────────────────

    #[test]
    fn external_sub_defaults_to_add_and_select() {
        let sub = ExternalSub::new("/subs/movie.eng.srt");
        assert_eq!(
            sub.command_args(),
            vec!["/subs/movie.eng.srt".to_owned(), "select".to_owned()]
        );
    }

    #[test]
    fn external_sub_with_title_and_lang_fills_positional_args() {
        let sub = ExternalSub {
            path: "/subs/movie.ass".to_owned(),
            load: SubLoad::Auto,
            title: Some("Director's Commentary".to_owned()),
            lang: Some("eng".to_owned()),
        };
        assert_eq!(
            sub.command_args(),
            vec![
                "/subs/movie.ass".to_owned(),
                "auto".to_owned(),
                "Director's Commentary".to_owned(),
                "eng".to_owned(),
            ]
        );
    }

    #[test]
    fn external_sub_lang_without_title_holds_the_title_slot() {
        let sub = ExternalSub {
            path: "/subs/x.srt".to_owned(),
            load: SubLoad::Cached,
            title: None,
            lang: Some("jpn".to_owned()),
        };
        // The empty title slot keeps `jpn` in mpv's 4th positional argument.
        assert_eq!(
            sub.command_args(),
            vec![
                "/subs/x.srt".to_owned(),
                "cached".to_owned(),
                String::new(),
                "jpn".to_owned(),
            ]
        );
    }

    #[test]
    fn config_commands_prepend_sub_add_in_order() {
        let cfg = SubtitleConfig {
            external: vec![ExternalSub::new("a.srt"), ExternalSub::new("b.ass")],
            ..SubtitleConfig::new()
        };
        assert_eq!(
            cfg.commands(),
            vec![
                vec![
                    "sub-add".to_owned(),
                    "a.srt".to_owned(),
                    "select".to_owned()
                ],
                vec![
                    "sub-add".to_owned(),
                    "b.ass".to_owned(),
                    "select".to_owned()
                ],
            ]
        );
    }

    // ── subtitle styling properties ─────────────────────────────────────────

    #[test]
    fn default_subtitle_config_emits_only_visibility_and_override() {
        let cfg = SubtitleConfig::new();
        assert!(cfg.commands().is_empty());
        assert_eq!(
            cfg.properties(),
            vec![
                ("sub-visibility".to_owned(), "yes".to_owned()),
                ("sub-ass-override".to_owned(), "scale".to_owned()),
            ]
        );
    }

    #[test]
    fn ass_override_modes_fold_to_property() {
        for (mode, expected) in [
            (AssOverride::No, "no"),
            (AssOverride::Yes, "yes"),
            (AssOverride::Scale, "scale"),
            (AssOverride::Force, "force"),
            (AssOverride::Strip, "strip"),
        ] {
            let cfg = SubtitleConfig {
                ass_override: mode,
                ..SubtitleConfig::new()
            };
            assert!(cfg
                .properties()
                .contains(&("sub-ass-override".to_owned(), expected.to_owned())));
        }
    }

    #[test]
    fn position_scale_delay_emitted_only_when_non_default() {
        // Neutral → none of the three fine properties present.
        let flat = SubtitleConfig::new();
        let flat_keys: Vec<String> = flat.properties().into_iter().map(|(k, _)| k).collect();
        assert!(!flat_keys.iter().any(|k| k == "sub-pos"));
        assert!(!flat_keys.iter().any(|k| k == "sub-scale"));
        assert!(!flat_keys.iter().any(|k| k == "sub-delay"));

        // Styled + positioned + delayed → all three present, formatted.
        let tuned = SubtitleConfig {
            pos: 90,
            scale: 1.25,
            delay: -0.5,
            ..SubtitleConfig::new()
        };
        let props = tuned.properties();
        assert!(props.contains(&("sub-pos".to_owned(), "90".to_owned())));
        assert!(props.contains(&("sub-scale".to_owned(), "1.25".to_owned())));
        assert!(props.contains(&("sub-delay".to_owned(), "-0.5".to_owned())));
    }

    #[test]
    fn hidden_subtitles_fold_visibility_no() {
        let cfg = SubtitleConfig {
            visible: false,
            ..SubtitleConfig::new()
        };
        assert!(cfg
            .properties()
            .contains(&("sub-visibility".to_owned(), "no".to_owned())));
    }

    #[test]
    fn full_subtitle_stack_folds_commands_and_properties() {
        // An external .ass with styling + delay — the full Q7 set.
        let cfg = SubtitleConfig {
            external: vec![ExternalSub {
                path: "/subs/anime.ass".to_owned(),
                load: SubLoad::Select,
                title: Some("Fansub".to_owned()),
                lang: Some("jpn".to_owned()),
            }],
            visible: true,
            ass_override: AssOverride::Force,
            pos: 95,
            scale: 1.1,
            delay: 0.25,
        };
        assert_eq!(
            cfg.commands(),
            vec![vec![
                "sub-add".to_owned(),
                "/subs/anime.ass".to_owned(),
                "select".to_owned(),
                "Fansub".to_owned(),
                "jpn".to_owned(),
            ]]
        );
        assert_eq!(
            cfg.properties(),
            vec![
                ("sub-visibility".to_owned(), "yes".to_owned()),
                ("sub-ass-override".to_owned(), "force".to_owned()),
                ("sub-pos".to_owned(), "95".to_owned()),
                ("sub-scale".to_owned(), "1.1".to_owned()),
                ("sub-delay".to_owned(), "0.25".to_owned()),
            ]
        );
    }

    // ── serde round-trip ────────────────────────────────────────────────────

    #[test]
    fn configs_round_trip_through_serde() {
        let sel = TrackSelection {
            audio: TrackSelect::Id(2),
            video: TrackSelect::Auto,
            subtitle: TrackSelect::Off,
        };
        let json = serde_json::to_string(&sel).expect("serialize");
        let back: TrackSelection = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(sel, back);
        assert_eq!(sel.properties(), back.properties());

        let cfg = SubtitleConfig {
            external: vec![ExternalSub::new("x.srt")],
            visible: false,
            ass_override: AssOverride::Strip,
            pos: 80,
            scale: 0.9,
            delay: 1.5,
        };
        let json = serde_json::to_string(&cfg).expect("serialize");
        let back: SubtitleConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cfg, back);
        assert_eq!(cfg.commands(), back.commands());
        assert_eq!(cfg.properties(), back.properties());
    }
}
