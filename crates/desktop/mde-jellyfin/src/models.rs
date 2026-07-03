//! The typed serde projection of the Jellyfin REST responses.
//!
//! Jellyfin serves `PascalCase` JSON; each struct maps it to idiomatic
//! `snake_case` Rust via `#[serde(rename_all = "PascalCase")]`. Every field is
//! `#[serde(default)]`
//! or an `Option` so a trimmed / evolving server payload deserializes softly
//! rather than failing the whole browse.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The universal Jellyfin item — one row of any `/Items`-shaped response.
///
/// A single struct models every kind (`Series`, `Season`, `Episode`, `Movie`,
/// `BoxSet`, `MusicArtist`, `MusicAlbum`, `Audio`, …); [`item_type`](Self::item_type)
/// discriminates. The client only projects the fields the browse surface needs;
/// unknown fields are ignored.
///
/// (`Eq` is intentionally not derived — [`UserData::played_percentage`] is an
/// `f64`, which is only `PartialEq`.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct BaseItemDto {
    /// The item's server GUID (the id every child/image/season query keys on).
    pub id: String,
    /// The display name / title, if present.
    #[serde(default)]
    pub name: Option<String>,
    /// The item kind (`"Series"`, `"Season"`, `"Episode"`, `"Movie"`,
    /// `"BoxSet"`, `"MusicAlbum"`, `"MusicArtist"`, `"Audio"`, …).
    #[serde(rename = "Type", default)]
    pub item_type: Option<String>,
    /// The long description / synopsis, if present.
    #[serde(default)]
    pub overview: Option<String>,
    /// The production / release year, if reported.
    #[serde(default)]
    pub production_year: Option<i32>,
    /// The item's ordinal within its parent — the episode number, or the track
    /// number for audio.
    #[serde(default)]
    pub index_number: Option<i32>,
    /// The parent's ordinal — the season number for an episode.
    #[serde(default)]
    pub parent_index_number: Option<i32>,
    /// The owning series' GUID (set on seasons + episodes).
    #[serde(default)]
    pub series_id: Option<String>,
    /// The owning series' name (set on seasons + episodes).
    #[serde(default)]
    pub series_name: Option<String>,
    /// The owning season's GUID (set on episodes; the key
    /// [`build_show_tree`](crate::build_show_tree) folds on).
    #[serde(default)]
    pub season_id: Option<String>,
    /// The immediate parent's GUID (a series for a season, a library for a
    /// top-level view) — the `ParentId` a child query passes back.
    #[serde(default)]
    pub parent_id: Option<String>,
    /// For a library view / collection: its kind (`"movies"`, `"tvshows"`,
    /// `"music"`, `"boxsets"`, `"playlists"`).
    #[serde(default)]
    pub collection_type: Option<String>,
    /// The runtime in 100-ns ticks (10 000 000 per second), if reported.
    #[serde(default)]
    pub run_time_ticks: Option<i64>,
    /// The item's genre labels.
    #[serde(default)]
    pub genres: Vec<String>,
    /// The count of direct children (seasons under a series, episodes under a
    /// season), if reported.
    #[serde(default)]
    pub child_count: Option<i32>,
    /// Image kind → tag map (`{"Primary": "<tag>", "Logo": "<tag>", …}`); the
    /// tag is fed to [`image_url`](crate::image_url) to build a cache-stable
    /// artwork URL.
    #[serde(default)]
    pub image_tags: BTreeMap<String, String>,
    /// The tags of the item's backdrop images (positional, not keyed).
    #[serde(default)]
    pub backdrop_image_tags: Vec<String>,
    /// The per-user playback state (resume position, played, favorite), if the
    /// query requested it.
    #[serde(default)]
    pub user_data: Option<UserData>,
    /// The playable [`MediaSourceInfo`]s for this item — the container + the
    /// per-stream codecs playback negotiation (MEDIA-10) reads. Populated when a
    /// browse requests the `MediaSources` field, or by
    /// [`playback_info`](crate::JellyfinClient::playback_info).
    #[serde(default)]
    pub media_sources: Vec<MediaSourceInfo>,
}

/// One playable source of an item — the physical file the server would stream,
/// with the container + the codecs playback negotiation (MEDIA-10) reasons over.
///
/// A single item can carry several ([`BaseItemDto::media_sources`]) — different
/// versions / qualities; each is negotiated independently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct MediaSourceInfo {
    /// The media-source GUID — the `mediaSourceId` a stream/transcode URL keys on.
    #[serde(default)]
    pub id: Option<String>,
    /// The container format (`"mkv"`, `"mp4"`, …) — the key input to the
    /// direct-play vs direct-stream decision.
    #[serde(default)]
    pub container: Option<String>,
    /// The server-side path / URL of the source, if reported.
    #[serde(default)]
    pub path: Option<String>,
    /// The delivery protocol (`"File"`, `"Http"`, `"Hls"`), if reported.
    #[serde(default)]
    pub protocol: Option<String>,
    /// The server's own verdict that the client may stream the bytes untouched.
    #[serde(default)]
    pub supports_direct_play: bool,
    /// The server's own verdict that the source may be remuxed without re-encode.
    #[serde(default)]
    pub supports_direct_stream: bool,
    /// The server's own verdict that the source can be transcoded.
    #[serde(default)]
    pub supports_transcoding: bool,
    /// The server-supplied transcoding URL (an HLS `.m3u8`), when it built one.
    #[serde(default)]
    pub transcoding_url: Option<String>,
    /// The container the server would transcode into, if reported.
    #[serde(default)]
    pub transcoding_container: Option<String>,
    /// The transcode sub-protocol (`"hls"`), if reported.
    #[serde(default)]
    pub transcoding_sub_protocol: Option<String>,
    /// The source runtime in 100-ns ticks, if reported.
    #[serde(default)]
    pub run_time_ticks: Option<i64>,
    /// The overall bitrate in bits/s, if reported.
    #[serde(default)]
    pub bitrate: Option<i64>,
    /// The audio/video/subtitle streams inside this source.
    #[serde(default)]
    pub media_streams: Vec<MediaStream>,
}

impl MediaSourceInfo {
    /// The codecs of every video stream in this source (in stream order).
    pub fn video_codecs(&self) -> impl Iterator<Item = &str> {
        self.media_streams
            .iter()
            .filter(|s| s.is_video())
            .filter_map(|s| s.codec.as_deref())
    }

    /// The codecs of every audio stream in this source (in stream order).
    pub fn audio_codecs(&self) -> impl Iterator<Item = &str> {
        self.media_streams
            .iter()
            .filter(|s| s.is_audio())
            .filter_map(|s| s.codec.as_deref())
    }

    /// The index of the default (or first) audio stream, for a playback report.
    #[must_use]
    pub fn default_audio_index(&self) -> Option<i32> {
        self.default_stream_index(StreamKind::Audio)
    }

    /// The index of the default (or first) subtitle stream, for a playback report.
    #[must_use]
    pub fn default_subtitle_index(&self) -> Option<i32> {
        self.default_stream_index(StreamKind::Subtitle)
    }

    /// The index of the default stream of `kind`, falling back to the first.
    fn default_stream_index(&self, kind: StreamKind) -> Option<i32> {
        let of_kind = || self.media_streams.iter().filter(|s| s.kind() == Some(kind));
        of_kind()
            .find(|s| s.is_default)
            .or_else(|| of_kind().next())
            .map(|s| s.index)
    }
}

/// The kind of a [`MediaStream`] — the `Type` discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    /// A video stream.
    Video,
    /// An audio stream.
    Audio,
    /// A subtitle stream.
    Subtitle,
}

/// One elementary stream inside a [`MediaSourceInfo`] — its kind + codec are the
/// per-stream facts negotiation checks against the player's decode capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct MediaStream {
    /// The stream kind (`"Video"`, `"Audio"`, `"Subtitle"`, …).
    #[serde(rename = "Type", default)]
    pub stream_type: Option<String>,
    /// The codec short name (`"h264"`, `"aac"`, `"ass"`, …), if reported.
    #[serde(default)]
    pub codec: Option<String>,
    /// The stream's index within the source (the `AudioStreamIndex` /
    /// `SubtitleStreamIndex` a playback report carries).
    #[serde(default)]
    pub index: i32,
    /// The BCP-47 / ISO language tag, if reported.
    #[serde(default)]
    pub language: Option<String>,
    /// A human display title, if reported.
    #[serde(default)]
    pub display_title: Option<String>,
    /// Whether the container marks this stream default.
    #[serde(default)]
    pub is_default: bool,
    /// The channel count (audio), if reported.
    #[serde(default)]
    pub channels: Option<i32>,
    /// The pixel width (video), if reported.
    #[serde(default)]
    pub width: Option<i32>,
    /// The pixel height (video), if reported.
    #[serde(default)]
    pub height: Option<i32>,
}

impl MediaStream {
    /// The typed [`StreamKind`] of this stream, or [`None`] for an unknown type.
    #[must_use]
    pub fn kind(&self) -> Option<StreamKind> {
        match self.stream_type.as_deref() {
            Some("Video") => Some(StreamKind::Video),
            Some("Audio") => Some(StreamKind::Audio),
            Some("Subtitle") => Some(StreamKind::Subtitle),
            _ => None,
        }
    }

    /// Whether this is a video stream.
    #[must_use]
    pub fn is_video(&self) -> bool {
        self.kind() == Some(StreamKind::Video)
    }

    /// Whether this is an audio stream.
    #[must_use]
    pub fn is_audio(&self) -> bool {
        self.kind() == Some(StreamKind::Audio)
    }

    /// Whether this is a subtitle stream.
    #[must_use]
    pub fn is_subtitle(&self) -> bool {
        self.kind() == Some(StreamKind::Subtitle)
    }
}

/// The response of `POST /Items/{id}/PlaybackInfo`.
///
/// The server's playable [`MediaSourceInfo`]s (with its own `Supports*` verdicts
/// and any transcode URL) plus the `PlaySessionId` that ties the progress
/// reports to this playback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackInfoResponse {
    /// The playable sources the server resolved for the item.
    #[serde(default)]
    pub media_sources: Vec<MediaSourceInfo>,
    /// The session id to echo back in `/Sessions/Playing*` progress reports.
    #[serde(default)]
    pub play_session_id: Option<String>,
}

/// The per-user playback state attached to a [`BaseItemDto`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct UserData {
    /// Where playback was last left, in 100-ns ticks — the resume point
    /// Continue-Watching restores.
    #[serde(default)]
    pub playback_position_ticks: i64,
    /// Whether the item is marked fully played.
    #[serde(default)]
    pub played: bool,
    /// How many times the item has been played.
    #[serde(default)]
    pub play_count: i32,
    /// Whether the user favorited the item.
    #[serde(default)]
    pub is_favorite: bool,
    /// For a folder (series/season): the count of unplayed children, if
    /// reported.
    #[serde(default)]
    pub unplayed_item_count: Option<i32>,
    /// The played fraction `0.0..=100.0` the server computed, if reported.
    #[serde(default)]
    pub played_percentage: Option<f64>,
}

/// The `QueryResult` envelope every `/Items`-shaped endpoint returns
/// (`/Users/{id}/Items`, `/Shows/Seasons`, `/Shows/Episodes`, `/Shows/NextUp`,
/// `/Items/Resume`, `/Genres`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ItemsResponse {
    /// The page of items.
    #[serde(default)]
    pub items: Vec<BaseItemDto>,
    /// The total match count across all pages (for paging).
    #[serde(default)]
    pub total_record_count: i64,
    /// The start offset this page represents.
    #[serde(default)]
    pub start_index: i64,
}

/// The public user identity carried by an [`AuthenticationResult`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct PublicUser {
    /// The user's GUID — the `UserId` every browse query is scoped to.
    pub id: String,
    /// The user's display name.
    #[serde(default)]
    pub name: String,
    /// The server GUID the user belongs to, if reported.
    #[serde(default)]
    pub server_id: Option<String>,
}

/// The result of a successful login (username/password or Quick Connect
/// exchange) — the `AccessToken` + the authenticated user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct AuthenticationResult {
    /// The bearer token to put in the `Authorization` header of every
    /// subsequent request (see [`ServerAuth`](crate::ServerAuth)).
    pub access_token: String,
    /// The server GUID, if reported.
    #[serde(default)]
    pub server_id: Option<String>,
    /// The authenticated user (its [`PublicUser::id`] is the saved `UserId`).
    #[serde(default)]
    pub user: PublicUser,
}

/// The Quick Connect state — the response of both `/QuickConnect/Initiate` and
/// each `/QuickConnect/Connect` poll.
///
/// On initiate, [`authenticated`](Self::authenticated) is `false` and the
/// client shows the [`code`](Self::code) for the user to approve in an already
/// signed-in session; polling `/QuickConnect/Connect?secret=…` with the
/// [`secret`](Self::secret) flips `authenticated` to `true`, after which the
/// [`secret`](Self::secret) is exchanged for an [`AuthenticationResult`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct QuickConnectState {
    /// Whether the user has approved this request yet.
    #[serde(default)]
    pub authenticated: bool,
    /// The opaque secret the client polls with and finally exchanges.
    #[serde(default)]
    pub secret: String,
    /// The short human code the user types into an authorized session.
    #[serde(default)]
    pub code: String,
    /// The device GUID the request is bound to, if reported.
    #[serde(default)]
    pub device_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_item_maps_pascalcase_and_type_alias() {
        let json = r#"{
            "Id": "abc",
            "Name": "The Show",
            "Type": "Series",
            "ProductionYear": 2019,
            "Genres": ["Drama","Sci-Fi"],
            "ImageTags": { "Primary": "tagp", "Logo": "tagl" },
            "UserData": { "PlaybackPositionTicks": 42, "Played": false, "PlayCount": 3 }
        }"#;
        let item: BaseItemDto = serde_json::from_str(json).expect("parse base item");
        assert_eq!(item.id, "abc");
        assert_eq!(item.name.as_deref(), Some("The Show"));
        assert_eq!(item.item_type.as_deref(), Some("Series"));
        assert_eq!(item.production_year, Some(2019));
        assert_eq!(item.genres, vec!["Drama", "Sci-Fi"]);
        assert_eq!(
            item.image_tags.get("Primary").map(String::as_str),
            Some("tagp")
        );
        let ud = item.user_data.expect("user data present");
        assert_eq!(ud.playback_position_ticks, 42);
        assert_eq!(ud.play_count, 3);
        assert!(!ud.played);
    }

    #[test]
    fn sparse_item_deserializes_softly() {
        // Only the required Id — every other field defaults, no error.
        let item: BaseItemDto = serde_json::from_str(r#"{"Id":"x"}"#).expect("sparse item");
        assert_eq!(item.id, "x");
        assert!(item.name.is_none());
        assert!(item.genres.is_empty());
        assert!(item.user_data.is_none());
    }

    #[test]
    fn items_envelope_parses_paging() {
        let json = r#"{"Items":[{"Id":"1"},{"Id":"2"}],"TotalRecordCount":57,"StartIndex":10}"#;
        let resp: ItemsResponse = serde_json::from_str(json).expect("parse envelope");
        assert_eq!(resp.items.len(), 2);
        assert_eq!(resp.total_record_count, 57);
        assert_eq!(resp.start_index, 10);
    }

    #[test]
    fn empty_envelope_defaults() {
        let resp: ItemsResponse = serde_json::from_str("{}").expect("empty envelope");
        assert!(resp.items.is_empty());
        assert_eq!(resp.total_record_count, 0);
    }

    #[test]
    fn auth_result_projects_token_and_user() {
        let json = r#"{
            "AccessToken": "TOKEN-xyz",
            "ServerId": "srv-1",
            "User": { "Id": "user-9", "Name": "matthew" }
        }"#;
        let res: AuthenticationResult = serde_json::from_str(json).expect("parse auth");
        assert_eq!(res.access_token, "TOKEN-xyz");
        assert_eq!(res.server_id.as_deref(), Some("srv-1"));
        assert_eq!(res.user.id, "user-9");
        assert_eq!(res.user.name, "matthew");
    }

    #[test]
    fn base_item_parses_media_sources_and_streams() {
        let json = r#"{
            "Id": "movie-1",
            "Type": "Movie",
            "MediaSources": [{
                "Id": "src-a",
                "Container": "mkv",
                "Protocol": "File",
                "SupportsDirectPlay": true,
                "RunTimeTicks": 12000000000,
                "MediaStreams": [
                    { "Type": "Video", "Codec": "h264", "Index": 0, "Height": 1080 },
                    { "Type": "Audio", "Codec": "aac", "Index": 1, "IsDefault": true, "Channels": 6 },
                    { "Type": "Subtitle", "Codec": "subrip", "Index": 2 }
                ]
            }]
        }"#;
        let item: BaseItemDto = serde_json::from_str(json).expect("parse item with sources");
        assert_eq!(item.media_sources.len(), 1);
        let src = &item.media_sources[0];
        assert_eq!(src.container.as_deref(), Some("mkv"));
        assert!(src.supports_direct_play);
        assert_eq!(src.video_codecs().collect::<Vec<_>>(), vec!["h264"]);
        assert_eq!(src.audio_codecs().collect::<Vec<_>>(), vec!["aac"]);
        // The default audio stream (index 1) + the only subtitle (index 2).
        assert_eq!(src.default_audio_index(), Some(1));
        assert_eq!(src.default_subtitle_index(), Some(2));
        assert!(src.media_streams[2].is_subtitle());
    }

    #[test]
    fn default_audio_index_falls_back_to_first_when_none_default() {
        let src: MediaSourceInfo = serde_json::from_str(
            r#"{"MediaStreams":[{"Type":"Audio","Index":3},{"Type":"Audio","Index":5}]}"#,
        )
        .expect("parse source");
        // No IsDefault → the first audio stream (index 3).
        assert_eq!(src.default_audio_index(), Some(3));
        assert_eq!(src.default_subtitle_index(), None);
    }

    #[test]
    fn playback_info_response_projects_sources_and_session() {
        let json = r#"{
            "MediaSources": [{ "Id": "s1", "Container": "mp4" }],
            "PlaySessionId": "session-xyz"
        }"#;
        let resp: PlaybackInfoResponse = serde_json::from_str(json).expect("parse playbackinfo");
        assert_eq!(resp.media_sources.len(), 1);
        assert_eq!(resp.play_session_id.as_deref(), Some("session-xyz"));
    }

    #[test]
    fn quick_connect_state_flips_authenticated() {
        let pending: QuickConnectState =
            serde_json::from_str(r#"{"Authenticated":false,"Secret":"s","Code":"123456"}"#)
                .expect("parse pending");
        assert!(!pending.authenticated);
        assert_eq!(pending.code, "123456");

        let done: QuickConnectState =
            serde_json::from_str(r#"{"Authenticated":true,"Secret":"s","Code":"123456"}"#)
                .expect("parse done");
        assert!(done.authenticated);
    }
}
