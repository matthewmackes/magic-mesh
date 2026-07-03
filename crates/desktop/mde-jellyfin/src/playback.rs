//! Playback negotiation (MEDIA-10): direct-play / direct-stream vs a server
//! transcode, chosen from the player's decode capabilities + an item's
//! [`MediaSourceInfo`](crate::MediaSourceInfo).
//!
//! Jellyfin can serve a title three ways, cheapest first:
//!
//! 1. **Direct play** — stream the original file bytes untouched. Valid only when
//!    the player can demux the container *and* decode every stream's codec.
//! 2. **Direct stream** — the server remuxes into a container the player can open
//!    but does **not** re-encode (cheap): the codecs are fine, only the container
//!    is not.
//! 3. **Transcode** — the server re-encodes to an HLS stream (expensive): some
//!    codec the player cannot decode.
//!
//! [`decide_method`] is the pure fold that makes that choice from a
//! [`ClientCapabilities`] set (built, in the app, from `mde-media-core`'s
//! `MpvCapabilities` — §6 glue) and the source's container + stream codecs.
//! [`build_playback_decision`] then forms the concrete stream URL. Both are pure
//! and fixture-tested — no network, no libmpv.
//!
//! [`build_playback_info_request`] is the one call that needs a server: it asks
//! Jellyfin to resolve the playable sources for the client's capability profile
//! (`POST /Items/{id}/PlaybackInfo`); actually fetching it is honest-gated to a
//! live server, but the request + the negotiation over its response are tested.

use std::collections::BTreeSet;

use crate::client::{json_headers, render_query, trim_base, ClientInfo};
use crate::models::MediaSourceInfo;
use crate::net::HttpRequest;

/// Normalize a container / codec label for case-insensitive matching.
fn normalize(label: &str) -> String {
    label.trim().to_ascii_lowercase()
}

/// The containers + codecs the *client* (the local player) can play without a
/// server transcode — the negotiation input, and the Jellyfin `DeviceProfile`
/// the server's own resolution is told to honour.
///
/// This is the Jellyfin-domain mirror of the player's decode set: the app builds
/// it from `mde-media-core`'s `MpvCapabilities` (§6 glue), and the negotiation +
/// the `PlaybackInfo` device profile both read it — so both are unit-testable
/// against a synthetic set with no libmpv.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClientCapabilities {
    containers: BTreeSet<String>,
    video_codecs: BTreeSet<String>,
    audio_codecs: BTreeSet<String>,
}

impl ClientCapabilities {
    /// An empty profile — every source negotiates to a transcode until populated.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add supported containers (case-insensitive), consuming + returning `self`.
    #[must_use]
    pub fn with_containers<I, S>(mut self, containers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.containers
            .extend(containers.into_iter().map(|c| normalize(c.as_ref())));
        self
    }

    /// Add supported video codecs (case-insensitive), consuming + returning `self`.
    #[must_use]
    pub fn with_video_codecs<I, S>(mut self, codecs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.video_codecs
            .extend(codecs.into_iter().map(|c| normalize(c.as_ref())));
        self
    }

    /// Add supported audio codecs (case-insensitive), consuming + returning `self`.
    #[must_use]
    pub fn with_audio_codecs<I, S>(mut self, codecs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.audio_codecs
            .extend(codecs.into_iter().map(|c| normalize(c.as_ref())));
        self
    }

    /// Whether the player can demux `container` (case-insensitive).
    #[must_use]
    pub fn supports_container(&self, container: &str) -> bool {
        self.containers.contains(&normalize(container))
    }

    /// Whether the player can decode the video codec `codec` (case-insensitive).
    #[must_use]
    pub fn supports_video_codec(&self, codec: &str) -> bool {
        self.video_codecs.contains(&normalize(codec))
    }

    /// Whether the player can decode the audio codec `codec` (case-insensitive).
    #[must_use]
    pub fn supports_audio_codec(&self, codec: &str) -> bool {
        self.audio_codecs.contains(&normalize(codec))
    }

    /// A comma-joined list of the containers (for the `DeviceProfile` body).
    fn containers_csv(&self) -> String {
        join(&self.containers)
    }

    /// A comma-joined list of the video codecs.
    fn video_csv(&self) -> String {
        join(&self.video_codecs)
    }

    /// A comma-joined list of the audio codecs.
    fn audio_csv(&self) -> String {
        join(&self.audio_codecs)
    }

    /// The container [`build_playback_decision`] asks the server to remux into for
    /// a direct-stream: the first of `mkv` / `mp4` / `ts` the player supports,
    /// falling back to any supported container.
    fn remux_container(&self) -> Option<&str> {
        ["mkv", "mp4", "ts"]
            .into_iter()
            .find(|c| self.containers.contains(*c))
            .or_else(|| self.containers.iter().next().map(String::as_str))
    }
}

/// Join a set into a stable comma-separated string.
fn join(set: &BTreeSet<String>) -> String {
    set.iter().cloned().collect::<Vec<_>>().join(",")
}

/// Which of the three delivery paths negotiation chose for a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackMethod {
    /// Stream the original bytes untouched (container + all codecs supported).
    DirectPlay,
    /// Server remuxes into a supported container without re-encoding (codecs fine,
    /// container not).
    DirectStream,
    /// Server transcodes to HLS (a codec the player cannot decode).
    Transcode,
}

impl PlaybackMethod {
    /// The Jellyfin `PlayMethod` wire token for a playback report.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::DirectPlay => "DirectPlay",
            Self::DirectStream => "DirectStream",
            Self::Transcode => "Transcode",
        }
    }
}

/// Whether a source is streamed from the `Videos` or `Audio` endpoint (music
/// playback rides the same negotiation through the `Audio` path).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamMediaType {
    /// A video title — the `/Videos/{id}` stream endpoints.
    Video,
    /// A music track — the `/Audio/{id}` stream endpoints.
    Audio,
}

impl StreamMediaType {
    /// The path segment (`"Videos"` / `"Audio"`) for this media type.
    #[must_use]
    pub const fn segment(self) -> &'static str {
        match self {
            Self::Video => "Videos",
            Self::Audio => "Audio",
        }
    }
}

/// The negotiated way to play one source: the [`PlaybackMethod`], the concrete
/// stream `url` the player loads, and the ids a progress report echoes back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackDecision {
    /// The chosen delivery path.
    pub method: PlaybackMethod,
    /// The URL to hand the player (`Player::load`).
    pub url: String,
    /// The container the player will actually receive (the source's for
    /// direct-play, the remux target for direct-stream, `"ts"` for a transcode).
    pub container: Option<String>,
    /// The `mediaSourceId` this decision streams.
    pub media_source_id: Option<String>,
    /// The `PlaySessionId` (from `PlaybackInfo`) the progress reports carry.
    pub play_session_id: Option<String>,
}

/// Choose the delivery path for `source` given the player's `caps` — the pure
/// negotiation fold (MEDIA-10).
///
/// A codec the player cannot decode forces a [`Transcode`](PlaybackMethod::Transcode);
/// otherwise a supported container is a [`DirectPlay`](PlaybackMethod::DirectPlay)
/// and an unsupported one a [`DirectStream`](PlaybackMethod::DirectStream) (remux
/// only). A source with no enumerated streams is decided on its container alone.
#[must_use]
pub fn decide_method(source: &MediaSourceInfo, caps: &ClientCapabilities) -> PlaybackMethod {
    let video_ok = source
        .video_codecs()
        .all(|codec| caps.supports_video_codec(codec));
    let audio_ok = source
        .audio_codecs()
        .all(|codec| caps.supports_audio_codec(codec));
    if !(video_ok && audio_ok) {
        return PlaybackMethod::Transcode;
    }
    let container_ok = source
        .container
        .as_deref()
        .is_some_and(|c| caps.supports_container(c));
    if container_ok {
        PlaybackMethod::DirectPlay
    } else {
        PlaybackMethod::DirectStream
    }
}

/// Build the direct-play stream URL — the original bytes, untouched
/// (`/{Videos|Audio}/{id}/stream?static=true`).
#[must_use]
pub fn direct_play_url(
    base_url: &str,
    item_id: &str,
    media_source_id: Option<&str>,
    media_type: StreamMediaType,
    token: Option<&str>,
) -> String {
    format!(
        "{}/{}/{}/stream{}",
        trim_base(base_url),
        media_type.segment(),
        item_id,
        render_query(&[
            ("static", "true".to_string()),
            (
                "mediaSourceId",
                media_source_id.unwrap_or_default().to_string()
            ),
            ("api_key", token.unwrap_or_default().to_string()),
        ]),
    )
}

/// Build the direct-stream (remux) URL into `container` — the server repackages
/// without re-encoding (`/{Videos|Audio}/{id}/stream.{container}`).
#[must_use]
pub fn direct_stream_url(
    base_url: &str,
    item_id: &str,
    media_source_id: Option<&str>,
    container: &str,
    media_type: StreamMediaType,
    token: Option<&str>,
) -> String {
    format!(
        "{}/{}/{}/stream.{}{}",
        trim_base(base_url),
        media_type.segment(),
        item_id,
        container,
        render_query(&[
            ("static", "false".to_string()),
            (
                "mediaSourceId",
                media_source_id.unwrap_or_default().to_string()
            ),
            ("api_key", token.unwrap_or_default().to_string()),
        ]),
    )
}

/// Build the HLS transcode URL (`/{Videos|Audio}/{id}/main.m3u8`) — the server
/// re-encodes to an adaptive stream.
#[must_use]
pub fn transcode_url(
    base_url: &str,
    item_id: &str,
    media_source_id: Option<&str>,
    media_type: StreamMediaType,
    token: Option<&str>,
    play_session_id: Option<&str>,
) -> String {
    format!(
        "{}/{}/{}/main.m3u8{}",
        trim_base(base_url),
        media_type.segment(),
        item_id,
        render_query(&[
            (
                "mediaSourceId",
                media_source_id.unwrap_or_default().to_string()
            ),
            ("api_key", token.unwrap_or_default().to_string()),
            (
                "playSessionId",
                play_session_id.unwrap_or_default().to_string()
            ),
        ]),
    )
}

/// Resolve a possibly-relative server URL (Jellyfin's `TranscodingUrl` is a
/// site-root path) against `base_url`.
fn absolutize(base_url: &str, url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else {
        format!("{}{}", trim_base(base_url), url)
    }
}

/// Negotiate + build the full [`PlaybackDecision`] for `source`: pick the method
/// from `caps`, then form the stream URL (preferring the server's own
/// `TranscodingUrl` for a transcode when it supplied one).
#[must_use]
pub fn build_playback_decision(
    base_url: &str,
    item_id: &str,
    source: &MediaSourceInfo,
    caps: &ClientCapabilities,
    media_type: StreamMediaType,
    token: Option<&str>,
    play_session_id: Option<&str>,
) -> PlaybackDecision {
    let method = decide_method(source, caps);
    let sid = source.id.as_deref();
    let (url, container) = match method {
        PlaybackMethod::DirectPlay => (
            direct_play_url(base_url, item_id, sid, media_type, token),
            source.container.clone(),
        ),
        PlaybackMethod::DirectStream => {
            let remux = caps
                .remux_container()
                .or(source.container.as_deref())
                .unwrap_or("mkv")
                .to_string();
            let url = direct_stream_url(base_url, item_id, sid, &remux, media_type, token);
            (url, Some(remux))
        }
        PlaybackMethod::Transcode => {
            let url = source.transcoding_url.as_deref().map_or_else(
                || transcode_url(base_url, item_id, sid, media_type, token, play_session_id),
                |server_url| absolutize(base_url, server_url),
            );
            (url, Some("ts".to_string()))
        }
    };
    PlaybackDecision {
        method,
        url,
        container,
        media_source_id: source.id.clone(),
        play_session_id: play_session_id.map(ToString::to_string),
    }
}

/// The client-capability [`DeviceProfile`](https://api.jellyfin.org/) body the
/// server negotiates against — direct-play profiles from `caps`, plus the HLS /
/// http transcode fallbacks.
fn device_profile(caps: &ClientCapabilities) -> serde_json::Value {
    let containers = caps.containers_csv();
    let video = caps.video_csv();
    let audio = caps.audio_csv();
    serde_json::json!({
        "MaxStreamingBitrate": 120_000_000,
        "MaxStaticBitrate": 100_000_000,
        "DirectPlayProfiles": [
            { "Container": containers, "Type": "Video", "VideoCodec": video, "AudioCodec": audio },
            { "Container": containers, "Type": "Audio", "AudioCodec": audio }
        ],
        "TranscodingProfiles": [
            {
                "Container": "ts", "Type": "Video", "Protocol": "hls",
                "VideoCodec": "h264", "AudioCodec": "aac", "Context": "Streaming"
            },
            {
                "Container": "mp3", "Type": "Audio", "Protocol": "http",
                "AudioCodec": "mp3", "Context": "Streaming"
            }
        ]
    })
}

/// Build the `POST /Items/{id}/PlaybackInfo?userId=…` request — ask the server to
/// resolve the playable sources for this client's capability profile.
///
/// The response ([`PlaybackInfoResponse`](crate::PlaybackInfoResponse)) carries
/// the sources with the server's own `Supports*` verdicts + a `PlaySessionId`;
/// negotiation then runs [`build_playback_decision`] over them.
#[must_use]
pub fn build_playback_info_request(
    base_url: &str,
    user_id: &str,
    item_id: &str,
    caps: &ClientCapabilities,
    device: &ClientInfo,
    token: Option<&str>,
) -> HttpRequest {
    let url = format!(
        "{}/Items/{}/PlaybackInfo{}",
        trim_base(base_url),
        item_id,
        render_query(&[("userId", user_id.to_string())]),
    );
    let body = serde_json::json!({
        "UserId": user_id,
        "MaxStreamingBitrate": 120_000_000,
        "EnableDirectPlay": true,
        "EnableDirectStream": true,
        "EnableTranscoding": true,
        "AllowVideoStreamCopy": true,
        "AllowAudioStreamCopy": true,
        "DeviceProfile": device_profile(caps),
    });
    HttpRequest::post(
        url,
        json_headers(device, token, true),
        serde_json::to_vec(&body).unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::MediaStream;

    fn caps() -> ClientCapabilities {
        ClientCapabilities::new()
            .with_containers(["mkv", "mp4"])
            .with_video_codecs(["h264", "hevc"])
            .with_audio_codecs(["aac", "flac"])
    }

    fn source(container: &str, video: &[&str], audio: &[&str]) -> MediaSourceInfo {
        let mut streams = Vec::new();
        let mut index = 0;
        for codec in video {
            streams.push(MediaStream {
                stream_type: Some("Video".into()),
                codec: Some((*codec).into()),
                index,
                ..MediaStream::default()
            });
            index += 1;
        }
        for codec in audio {
            streams.push(MediaStream {
                stream_type: Some("Audio".into()),
                codec: Some((*codec).into()),
                index,
                ..MediaStream::default()
            });
            index += 1;
        }
        MediaSourceInfo {
            id: Some("src-1".into()),
            container: Some(container.into()),
            media_streams: streams,
            ..MediaSourceInfo::default()
        }
    }

    #[test]
    fn supported_container_and_codecs_direct_play() {
        let src = source("mkv", &["h264"], &["aac"]);
        assert_eq!(decide_method(&src, &caps()), PlaybackMethod::DirectPlay);
    }

    #[test]
    fn unsupported_container_but_ok_codecs_direct_streams() {
        // avi container is not in caps, but h264 + aac are → remux only.
        let src = source("avi", &["h264"], &["aac"]);
        assert_eq!(decide_method(&src, &caps()), PlaybackMethod::DirectStream);
    }

    #[test]
    fn unsupported_video_codec_forces_transcode() {
        // vp9 not in caps → the server must re-encode, container irrelevant.
        let src = source("mkv", &["vp9"], &["aac"]);
        assert_eq!(decide_method(&src, &caps()), PlaybackMethod::Transcode);
    }

    #[test]
    fn unsupported_audio_codec_forces_transcode() {
        let src = source("mkv", &["h264"], &["ac3"]);
        assert_eq!(decide_method(&src, &caps()), PlaybackMethod::Transcode);
    }

    #[test]
    fn empty_caps_always_transcode_when_codecs_present() {
        let src = source("mkv", &["h264"], &["aac"]);
        assert_eq!(
            decide_method(&src, &ClientCapabilities::new()),
            PlaybackMethod::Transcode
        );
    }

    #[test]
    fn decision_builds_static_direct_play_url() {
        let src = source("mkv", &["h264"], &["aac"]);
        let d = build_playback_decision(
            "https://jelly.mesh:8096/",
            "movie-1",
            &src,
            &caps(),
            StreamMediaType::Video,
            Some("TOKEN"),
            None,
        );
        assert_eq!(d.method, PlaybackMethod::DirectPlay);
        assert!(d
            .url
            .starts_with("https://jelly.mesh:8096/Videos/movie-1/stream?"));
        assert!(d.url.contains("static=true"));
        assert!(d.url.contains("mediaSourceId=src-1"));
        assert!(d.url.contains("api_key=TOKEN"));
        assert_eq!(d.container.as_deref(), Some("mkv"));
    }

    #[test]
    fn decision_builds_direct_stream_remux_url_into_a_supported_container() {
        let src = source("avi", &["h264"], &["aac"]);
        let d = build_playback_decision(
            "https://jelly.mesh:8096",
            "movie-2",
            &src,
            &caps(),
            StreamMediaType::Video,
            Some("T"),
            None,
        );
        assert_eq!(d.method, PlaybackMethod::DirectStream);
        // caps prefers mkv as the remux target.
        assert!(d.url.contains("/Videos/movie-2/stream.mkv?"));
        assert_eq!(d.container.as_deref(), Some("mkv"));
    }

    #[test]
    fn decision_builds_hls_transcode_url_for_music_through_audio_path() {
        let src = source("flac", &[], &["dts"]); // dts unsupported → transcode
        let d = build_playback_decision(
            "https://jelly.mesh:8096",
            "track-9",
            &src,
            &caps(),
            StreamMediaType::Audio,
            Some("T"),
            Some("sess-1"),
        );
        assert_eq!(d.method, PlaybackMethod::Transcode);
        assert!(d.url.contains("/Audio/track-9/main.m3u8?"));
        assert!(d.url.contains("playSessionId=sess-1"));
        assert_eq!(d.container.as_deref(), Some("ts"));
    }

    #[test]
    fn transcode_prefers_the_servers_own_url_when_present() {
        let mut src = source("mkv", &["vp9"], &["aac"]);
        src.transcoding_url = Some("/videos/movie-3/main.m3u8?api_key=SRV&x=1".into());
        let d = build_playback_decision(
            "https://jelly.mesh:8096",
            "movie-3",
            &src,
            &caps(),
            StreamMediaType::Video,
            Some("T"),
            None,
        );
        assert_eq!(
            d.url,
            "https://jelly.mesh:8096/videos/movie-3/main.m3u8?api_key=SRV&x=1"
        );
    }

    #[test]
    fn playback_info_request_carries_the_device_profile() {
        let device = ClientInfo::new("mde-media", "workstation", "device-42", "12.0.0");
        let req = build_playback_info_request(
            "https://jelly.mesh:8096/",
            "user-1",
            "movie-1",
            &caps(),
            &device,
            Some("T"),
        );
        assert!(req
            .url
            .starts_with("https://jelly.mesh:8096/Items/movie-1/PlaybackInfo?userId=user-1"));
        let body = req.body.expect("post body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("body json");
        assert_eq!(json["UserId"], "user-1");
        assert_eq!(json["EnableTranscoding"], true);
        // The direct-play profile lists the client's containers + codecs.
        let profile = &json["DeviceProfile"]["DirectPlayProfiles"][0];
        assert_eq!(profile["Container"], "mkv,mp4");
        assert_eq!(profile["VideoCodec"], "h264,hevc");
        assert_eq!(profile["AudioCodec"], "aac,flac");
        // The Authorization line carries the token.
        assert!(req
            .headers
            .iter()
            .any(|(k, v)| k == "Authorization" && v.contains("Token=\"T\"")));
    }
}
