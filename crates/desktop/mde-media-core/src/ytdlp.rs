//! MEDIA-12: the `yt-dlp` resolution seam (subprocess, honest-gated on the tool).
//!
//! Design lock (`docs/design/mesh-media-player.md`, feature 24 + the Egress-features
//! Risks note): web-page videos are resolved to a direct media URL by a bundled
//! **`yt-dlp`**, an egress feature that must "gate cleanly, fail soft offline".
//!
//! The seam mirrors the [`MediaEngine`](crate::MediaEngine) idiom (and the
//! `mde-seat` `PwRunner`/`PwCli` tool seam): one narrow, injectable trait
//! ([`YtDlpResolver`]) with a real subprocess implementation ([`YtDlpCli`]) and a
//! **pure, fixture-tested parser** ([`parse_dump_json`]) between them, so every byte
//! of output projection is exercised with *no real `yt-dlp` and no network*.
//!
//! - [`YtDlpResolver`] is the interface the surface drives: [`is_available`] probes
//!   the tool, [`resolve`] turns a web page into the direct media URL(s).
//! - [`YtDlpCli`] is the real implementation. It shells out to `yt-dlp` — no
//!   compile-time dependency and no linking, so it is **always compiled** (unlike
//!   the feature-gated `mpv` engine). It is **honest-gated at runtime**: a `yt-dlp`
//!   that is absent from `PATH` surfaces as [`YtDlpError::NotInstalled`] — never a
//!   stub that pretends to have resolved something (§7).
//! - [`parse_dump_json`] projects `yt-dlp --dump-single-json` output into a typed
//!   [`ResolvedMedia`]. Pure — the live client feeds it the captured stdout; the
//!   tests feed it recorded fixtures.
//!
//! [`is_available`]: YtDlpResolver::is_available
//! [`resolve`]: YtDlpResolver::resolve

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The `yt-dlp` executable name, resolved on `PATH`.
pub const YT_DLP_BIN: &str = "yt-dlp";

/// The media `yt-dlp` resolved from a web page — the direct URL(s) the
/// [`Player`](crate::Player) then loads, plus the reported title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ResolvedMedia {
    /// The web-page URL that was resolved (echoed for the status line / logs).
    pub source_url: String,
    /// The title `yt-dlp` reported, if any.
    pub title: Option<String>,
    /// The direct media URL(s), best/primary first. For a simple progressive
    /// stream this is a single entry; a DASH selection may carry the separate
    /// video + audio stream URLs.
    pub urls: Vec<String>,
}

impl ResolvedMedia {
    /// The primary (best) direct media URL — the one handed to the player — or
    /// [`None`] when `yt-dlp` reported no playable URL.
    #[must_use]
    pub fn primary(&self) -> Option<&str> {
        self.urls.first().map(String::as_str)
    }

    /// Whether no direct media URL was resolved.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.urls.is_empty()
    }
}

/// A failure from the `yt-dlp` resolution seam.
///
/// Every variant is honest + recoverable: the caller surfaces it and carries on
/// (nothing is loaded) — the fail-soft contract of an egress feature.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum YtDlpError {
    /// `yt-dlp` is not installed / not on `PATH` — the honest tool-absent gate.
    #[error("yt-dlp is not installed")]
    NotInstalled,
    /// `yt-dlp` ran but exited non-zero (unsupported site, private video, offline,
    /// egress blocked); carries its trimmed stderr.
    #[error("yt-dlp failed: {0}")]
    Failed(String),
    /// `yt-dlp`'s output could not be parsed as the expected JSON.
    #[error("yt-dlp output parse error: {0}")]
    Parse(String),
    /// `yt-dlp` ran and parsed, but exposed no playable media URL.
    #[error("yt-dlp resolved no playable media URL")]
    NoMedia,
}

/// The injectable `yt-dlp` seam (MEDIA-12).
///
/// Production impl is [`YtDlpCli`] (the real subprocess); tests inject a fake that
/// returns recorded output + a scripted availability, so the resolution glue is
/// exercised with no real `yt-dlp` and no network.
pub trait YtDlpResolver {
    /// Whether `yt-dlp` is available to run — the surface disables / honest-gates
    /// the "open web link" affordance on this before the operator clicks.
    fn is_available(&self) -> bool;

    /// Resolve a web-page URL to its direct media URL(s).
    ///
    /// # Errors
    /// [`YtDlpError::NotInstalled`] when the tool is absent; [`YtDlpError::Failed`]
    /// when it exits non-zero; [`YtDlpError::Parse`] / [`YtDlpError::NoMedia`] when
    /// its output is unusable.
    fn resolve(&self, page_url: &str) -> Result<ResolvedMedia, YtDlpError>;
}

/// Project a `yt-dlp --dump-single-json` document into a [`ResolvedMedia`].
///
/// Pure + tolerant (§6 glue — no re-implementation of `yt-dlp`): it pulls the
/// reported `title` and gathers the direct URL(s) `yt-dlp` selected, in priority
/// order and de-duplicated:
///
/// 1. the top-level `url` (simple / progressive extractors),
/// 2. each `requested_downloads[].url` (the selected merged download(s)),
/// 3. each `requested_formats[].url` (the separate video + audio streams of a DASH
///    selection).
///
/// # Errors
/// [`YtDlpError::Parse`] when `body` is not JSON; [`YtDlpError::NoMedia`] when no
/// direct URL is present.
pub fn parse_dump_json(source_url: &str, body: &str) -> Result<ResolvedMedia, YtDlpError> {
    let root: Value = serde_json::from_str(body).map_err(|e| YtDlpError::Parse(e.to_string()))?;
    let title = root.get("title").and_then(Value::as_str).map(str::to_owned);

    let mut urls: Vec<String> = Vec::new();
    push_url(&mut urls, root.get("url"));
    for list in [
        root.get("requested_downloads"),
        root.get("requested_formats"),
    ] {
        if let Some(entries) = list.and_then(Value::as_array) {
            for entry in entries {
                push_url(&mut urls, entry.get("url"));
            }
        }
    }

    if urls.is_empty() {
        return Err(YtDlpError::NoMedia);
    }
    Ok(ResolvedMedia {
        source_url: source_url.to_owned(),
        title,
        urls,
    })
}

/// Append a non-empty, not-yet-seen string URL from a JSON value to `urls`.
fn push_url(urls: &mut Vec<String>, value: Option<&Value>) {
    if let Some(url) = value.and_then(Value::as_str) {
        let url = url.trim();
        if !url.is_empty() && !urls.iter().any(|seen| seen == url) {
            urls.push(url.to_owned());
        }
    }
}

/// The real `yt-dlp` resolver — shells out to the bundled tool (MEDIA-12).
///
/// No compile-time dependency (a subprocess, not a linked library), so it is always
/// compiled — airgap-safe to *build*. It is **honest-gated at runtime**: on a host
/// with no `yt-dlp` on `PATH`, [`is_available`](YtDlpResolver::is_available) is
/// `false` and [`resolve`](YtDlpResolver::resolve) returns
/// [`YtDlpError::NotInstalled`]. The live resolution is exercised only where the
/// tool + egress exist; the [`parse_dump_json`] projection it composes is fully
/// tested with recorded fixtures.
#[derive(Debug, Clone, Copy, Default)]
pub struct YtDlpCli;

impl YtDlpCli {
    /// Map a failed `Command` spawn to a typed error: a missing binary is the honest
    /// [`YtDlpError::NotInstalled`]; anything else is [`YtDlpError::Failed`].
    fn spawn_error(e: &std::io::Error) -> YtDlpError {
        if e.kind() == std::io::ErrorKind::NotFound {
            YtDlpError::NotInstalled
        } else {
            YtDlpError::Failed(e.to_string())
        }
    }
}

impl YtDlpResolver for YtDlpCli {
    fn is_available(&self) -> bool {
        std::process::Command::new(YT_DLP_BIN)
            .arg("--version")
            .output()
            .is_ok_and(|out| out.status.success())
    }

    fn resolve(&self, page_url: &str) -> Result<ResolvedMedia, YtDlpError> {
        // `--dump-single-json` resolves + prints the metadata without downloading;
        // `-f best/bestvideo+bestaudio` prefers a single progressive stream, falling
        // back to the separate video+audio streams `parse_dump_json` also gathers.
        let output = std::process::Command::new(YT_DLP_BIN)
            .args([
                "--no-playlist",
                "--no-warnings",
                "--dump-single-json",
                "-f",
                "best/bestvideo+bestaudio",
            ])
            .arg(page_url)
            .output()
            .map_err(|e| Self::spawn_error(&e))?;
        if !output.status.success() {
            return Err(YtDlpError::Failed(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ));
        }
        let body = String::from_utf8_lossy(&output.stdout);
        parse_dump_json(page_url, &body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A recorded resolver: a scripted availability + a captured `yt-dlp` JSON body,
    /// so the seam is driven with no real tool and no network — the fixture path the
    /// acceptance asks for.
    struct RecordedResolver {
        available: bool,
        body: String,
    }

    impl YtDlpResolver for RecordedResolver {
        fn is_available(&self) -> bool {
            self.available
        }

        fn resolve(&self, page_url: &str) -> Result<ResolvedMedia, YtDlpError> {
            if !self.available {
                return Err(YtDlpError::NotInstalled);
            }
            parse_dump_json(page_url, &self.body)
        }
    }

    // A trimmed but shape-faithful `yt-dlp --dump-single-json` document for a single
    // progressive video (the common YouTube-ish case).
    const PROGRESSIVE_JSON: &str = r#"{
      "id": "dQw4w9WgXcQ",
      "title": "Never Gonna Give You Up",
      "extractor": "youtube",
      "webpage_url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
      "url": "https://rr3.googlevideo.com/videoplayback?itag=22&sig=abc",
      "ext": "mp4",
      "formats": [
        { "format_id": "18", "url": "https://rr3.googlevideo.com/videoplayback?itag=18" },
        { "format_id": "22", "url": "https://rr3.googlevideo.com/videoplayback?itag=22&sig=abc" }
      ]
    }"#;

    #[test]
    fn parses_a_progressive_dump_json() {
        let resolved = parse_dump_json(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            PROGRESSIVE_JSON,
        )
        .expect("parse");
        assert_eq!(resolved.title.as_deref(), Some("Never Gonna Give You Up"));
        assert_eq!(
            resolved.primary(),
            Some("https://rr3.googlevideo.com/videoplayback?itag=22&sig=abc")
        );
        assert_eq!(
            resolved.source_url,
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
        assert!(!resolved.is_empty());
    }

    #[test]
    fn parses_requested_downloads_when_no_top_level_url() {
        // Some extractors expose the selected URL only under requested_downloads.
        let body = r#"{
          "title": "Clip",
          "requested_downloads": [
            { "url": "https://cdn.example/clip-720.mp4", "ext": "mp4" }
          ]
        }"#;
        let resolved = parse_dump_json("https://example.com/clip", body).expect("parse");
        assert_eq!(resolved.urls, vec!["https://cdn.example/clip-720.mp4"]);
    }

    #[test]
    fn gathers_separate_dash_video_and_audio_urls_deduped() {
        // A DASH selection: a top-level muxed url plus the separate a/v streams; the
        // duplicate of the top-level url in requested_formats is de-duplicated.
        let body = r#"{
          "title": "DASH Title",
          "url": "https://cdn.example/muxed.mp4",
          "requested_formats": [
            { "format_id": "137", "url": "https://cdn.example/video-1080.mp4" },
            { "format_id": "140", "url": "https://cdn.example/audio.m4a" },
            { "format_id": "muxed", "url": "https://cdn.example/muxed.mp4" }
          ]
        }"#;
        let resolved = parse_dump_json("https://example.com/dash", body).expect("parse");
        assert_eq!(
            resolved.urls,
            vec![
                "https://cdn.example/muxed.mp4",
                "https://cdn.example/video-1080.mp4",
                "https://cdn.example/audio.m4a",
            ]
        );
        assert_eq!(resolved.primary(), Some("https://cdn.example/muxed.mp4"));
    }

    #[test]
    fn no_media_url_is_an_honest_error() {
        // Parsed fine, but nothing playable (e.g. a metadata-only extractor result).
        let body = r#"{ "title": "No streams here", "formats": [] }"#;
        assert_eq!(
            parse_dump_json("https://example.com/x", body),
            Err(YtDlpError::NoMedia)
        );
    }

    #[test]
    fn malformed_json_is_a_parse_error_not_a_panic() {
        assert!(matches!(
            parse_dump_json("https://example.com/x", "not json"),
            Err(YtDlpError::Parse(_))
        ));
    }

    #[test]
    fn recorded_resolver_available_resolves_from_captured_output() {
        let resolver = RecordedResolver {
            available: true,
            body: PROGRESSIVE_JSON.to_owned(),
        };
        assert!(resolver.is_available());
        let media = resolver
            .resolve("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
            .expect("resolve");
        assert_eq!(media.title.as_deref(), Some("Never Gonna Give You Up"));
    }

    #[test]
    fn recorded_resolver_tool_absent_is_honest_not_installed() {
        let resolver = RecordedResolver {
            available: false,
            body: String::new(),
        };
        assert!(!resolver.is_available());
        assert_eq!(
            resolver.resolve("https://youtu.be/abc"),
            Err(YtDlpError::NotInstalled)
        );
    }

    #[test]
    fn resolved_media_primary_and_is_empty_defaults() {
        let empty = ResolvedMedia::default();
        assert!(empty.is_empty());
        assert_eq!(empty.primary(), None);
    }
}
