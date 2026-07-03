//! MEDIA-12: network-stream URL classification.
//!
//! Design lock (`docs/design/mesh-media-player.md`, feature 24): the player opens
//! **direct stream URLs** (`http`/`https`/`hls`/`rtsp`/`mms` …) and resolves
//! **web-page videos** via a bundled `yt-dlp` (see [`crate::ytdlp`]). Before either
//! path runs, an opened string has to be *classified*: is it a direct stream mpv
//! plays natively, a web page that needs resolving, a local file, or nonsense?
//!
//! [`classify_url`] is that pure, dependency-free fold (no network, no mpv). The
//! [`Player`](crate::Player) then handles each [`UrlKind`]:
//!
//! - [`UrlKind::DirectStream`] / [`UrlKind::LocalFile`] → [`Player::load`] the
//!   string verbatim (mpv/ffmpeg opens `http(s)`/`hls`/`rtsp`/`mms`/`rtmp`/`srt`
//!   streams and local files directly — the existing play path, §6 glue).
//! - [`UrlKind::WebPage`] → resolve the direct media URL with [`crate::ytdlp`],
//!   then [`Player::load`] the result.
//! - [`UrlKind::Invalid`] → surfaced honestly to the operator; nothing is loaded.
//!
//! [`Player::load`]: crate::Player::load

/// The explicit streaming-transport schemes mpv/ffmpeg opens directly (no `yt-dlp`).
///
/// `http`/`https` are handled separately (they can be either a direct media URL or
/// a web page), so they are not listed here.
const DIRECT_STREAM_SCHEMES: &[&str] = &[
    "rtsp", "rtsps", "rtmp", "rtmps", "rtmpe", "rtmpt", "mms", "mmsh", "mmst", "hls", "udp", "rtp",
    "srt",
];

/// Path extensions that mark an `http(s)` URL as a *direct* media/playlist stream
/// (so it is handed straight to the player) rather than a web page to resolve.
///
/// Covers container files, audio files, and adaptive-streaming manifests
/// (`.m3u8` HLS, `.mpd` DASH). Matched case-insensitively on the URL's last path
/// segment (query/fragment stripped).
const MEDIA_EXTENSIONS: &[&str] = &[
    // Adaptive-streaming manifests + playlists.
    ".m3u8", ".m3u", ".mpd", ".f4m", ".ism", // Video containers.
    ".mp4", ".m4v", ".mkv", ".webm", ".mov", ".avi", ".flv", ".ts", ".mpg", ".mpeg", ".wmv",
    ".3gp", ".ogv", // Audio containers.
    ".mp3", ".aac", ".flac", ".ogg", ".oga", ".opus", ".wav", ".m4a", ".wma",
];

/// The classification of an opened URL / path string (MEDIA-12).
///
/// A pure, per-scheme verdict from [`classify_url`] — the surface renders it and the
/// player routes on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlKind {
    /// A stream mpv plays natively — an explicit streaming scheme
    /// (`rtsp`/`rtmp`/`mms`/`hls`/`srt`/…) or an `http(s)` URL that points at a
    /// media file or adaptive manifest. Handed straight to [`Player::load`].
    ///
    /// [`Player::load`]: crate::Player::load
    DirectStream,
    /// An `http(s)` web page whose media must be resolved by `yt-dlp`
    /// ([`crate::ytdlp`]) before playback.
    WebPage,
    /// A local filesystem path (a bare path, or a `file:`/drive-letter URL).
    LocalFile,
    /// Not a playable URL or path (empty, or an unsupported scheme).
    Invalid,
}

impl UrlKind {
    /// A short human label for the kind (for the status line / tests).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::DirectStream => "direct stream",
            Self::WebPage => "web page",
            Self::LocalFile => "local file",
            Self::Invalid => "invalid",
        }
    }

    /// Whether the string is played by handing it straight to the
    /// [`Player`](crate::Player) with no `yt-dlp` resolution — a direct stream or a
    /// local file.
    #[must_use]
    pub const fn is_direct(self) -> bool {
        matches!(self, Self::DirectStream | Self::LocalFile)
    }
}

/// Classify an opened URL / path string into a [`UrlKind`] (MEDIA-12).
///
/// Pure + dependency-free — no network, no mpv. The rules, per scheme:
///
/// - empty / whitespace → [`UrlKind::Invalid`].
/// - no URL scheme (a bare path like `/media/clip.mkv` or `./rel.mp4`) →
///   [`UrlKind::LocalFile`].
/// - `file:` or a single-letter drive scheme (`C:\…`) → [`UrlKind::LocalFile`].
/// - an explicit streaming scheme ([`DIRECT_STREAM_SCHEMES`]) →
///   [`UrlKind::DirectStream`].
/// - `http`/`https` → [`UrlKind::DirectStream`] when the path ends in a known media
///   extension ([`MEDIA_EXTENSIONS`]), else [`UrlKind::WebPage`] (resolve via
///   `yt-dlp`).
/// - any other scheme (`ftp:`, `mailto:`, …) → [`UrlKind::Invalid`] (we do not
///   pretend to play it — §7).
#[must_use]
pub fn classify_url(input: &str) -> UrlKind {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return UrlKind::Invalid;
    }
    // No recognizable scheme → treat it as a filesystem path.
    let Some(scheme) = url_scheme(trimmed) else {
        return UrlKind::LocalFile;
    };
    let scheme = scheme.to_ascii_lowercase();
    if scheme == "file" || scheme.len() == 1 {
        // `file://…`, or a single-letter Windows drive (`C:\Users\…`).
        UrlKind::LocalFile
    } else if DIRECT_STREAM_SCHEMES.iter().any(|&s| s == scheme) {
        UrlKind::DirectStream
    } else if scheme == "http" || scheme == "https" {
        if is_media_url(trimmed) {
            UrlKind::DirectStream
        } else {
            UrlKind::WebPage
        }
    } else {
        UrlKind::Invalid
    }
}

/// Extract the URL scheme (the token before the first `:`) when `input` carries a
/// well-formed one, else [`None`] (a bare path).
///
/// A scheme is `ALPHA *( ALPHA / DIGIT / "+" / "-" )` — deliberately *without* `.`
/// so a `host.tld:port` authority is not mistaken for a scheme. A single alphabetic
/// scheme (a Windows drive letter) is returned as-is and classified as a local file.
fn url_scheme(input: &str) -> Option<&str> {
    let end = input.find(':')?;
    let scheme = &input[..end];
    let mut chars = scheme.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if chars.all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-') {
        Some(scheme)
    } else {
        None
    }
}

/// Whether an `http(s)` URL's last path segment ends in a known media/playlist
/// extension ([`MEDIA_EXTENSIONS`]) — i.e. it is a *direct* stream, not a web page.
///
/// The query string + fragment are stripped first so `…/master.m3u8?token=…` still
/// classifies as a direct stream.
fn is_media_url(url: &str) -> bool {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path = after_scheme
        .split(['?', '#'])
        .next()
        .unwrap_or(after_scheme);
    let last = path.rsplit('/').next().unwrap_or(path);
    let lower = last.to_ascii_lowercase();
    MEDIA_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_streaming_schemes_are_direct_streams() {
        for url in [
            "rtsp://cam.mesh:554/stream1",
            "rtsps://cam.mesh/secure",
            "rtmp://live.mesh/app/key",
            "rtmps://live.mesh/app/key",
            "mms://media.example/live",
            "mmsh://media.example/live",
            "hls://edge.example/master",
            "udp://@239.0.0.1:1234",
            "rtp://239.0.0.1:5004",
            "srt://relay.mesh:9000",
        ] {
            assert_eq!(classify_url(url), UrlKind::DirectStream, "{url}");
        }
    }

    #[test]
    fn http_urls_to_a_media_file_or_manifest_are_direct_streams() {
        for url in [
            "http://server.mesh/movie.mp4",
            "https://cdn.example/video.mkv",
            "https://cdn.example/audio.flac",
            "https://edge.example/live/master.m3u8",
            "https://edge.example/live/master.m3u8?token=abc123",
            "https://dash.example/manifest.mpd#t=10",
            "https://EXAMPLE.com/CLIP.MP4", // case-insensitive
        ] {
            assert_eq!(classify_url(url), UrlKind::DirectStream, "{url}");
        }
    }

    #[test]
    fn http_urls_without_a_media_extension_are_web_pages() {
        for url in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ",
            "https://vimeo.com/12345678",
            "http://example.com/some/article",
            "https://example.com/", // bare host
        ] {
            assert_eq!(classify_url(url), UrlKind::WebPage, "{url}");
        }
    }

    #[test]
    fn paths_and_file_urls_are_local_files() {
        for url in [
            "/media/movies/clip.mkv",
            "./relative/song.mp3",
            "../up/one.webm",
            "clip.mkv",            // bare name, no scheme
            "file:///media/a.mp4", // file URL
            "file:/media/a.mp4",
            r"C:\Users\me\Videos\v.mkv", // Windows drive letter
            "/tmp/weird:name.mkv",       // a colon in a path segment, not a scheme
        ] {
            assert_eq!(classify_url(url), UrlKind::LocalFile, "{url}");
        }
    }

    #[test]
    fn empty_and_unsupported_schemes_are_invalid() {
        for url in [
            "",
            "   ",
            "\t\n",
            "mailto:someone@example.com",
            "ftp://ftp.example/pub/file.mp4",
            "javascript:alert(1)",
            "gopher://old.example/1",
        ] {
            assert_eq!(classify_url(url), UrlKind::Invalid, "{url:?}");
        }
    }

    #[test]
    fn classify_trims_surrounding_whitespace() {
        assert_eq!(
            classify_url("  https://cdn.example/clip.mp4  "),
            UrlKind::DirectStream
        );
        assert_eq!(classify_url("\thttps://youtu.be/abc\n"), UrlKind::WebPage);
    }

    #[test]
    fn url_kind_label_and_is_direct() {
        assert_eq!(UrlKind::DirectStream.label(), "direct stream");
        assert_eq!(UrlKind::WebPage.label(), "web page");
        assert_eq!(UrlKind::LocalFile.label(), "local file");
        assert_eq!(UrlKind::Invalid.label(), "invalid");

        assert!(UrlKind::DirectStream.is_direct());
        assert!(UrlKind::LocalFile.is_direct());
        assert!(!UrlKind::WebPage.is_direct());
        assert!(!UrlKind::Invalid.is_direct());
    }
}
