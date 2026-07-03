//! Integration test: the MEDIA-10 playback + sync surface driven end-to-end
//! through a fixture transport — `PlaybackInfo` → negotiation → decision, the
//! `/Sessions/Playing*` progress reports + mark-played, cross-library search, and
//! the Live-TV / DVR browse — all against recorded JSON, no live network.

use std::cell::RefCell;

use mde_jellyfin::{
    build_playback_decision, resume_position_secs, ClientCapabilities, ClientInfo, HttpMethod,
    HttpRequest, HttpResponse, HttpTransport, ItemsQuery, JellyfinClient, PlaybackMethod,
    PlaybackReport, StreamMediaType, TransportError,
};

const PLAYBACK_INFO: &str = include_str!("fixtures/playback_info.json");
const LIVE_TV_CHANNELS: &str = include_str!("fixtures/live_tv_channels.json");
const GUIDE: &str = include_str!("fixtures/guide.json");
const RECORDINGS: &str = include_str!("fixtures/recordings.json");
const SEARCH_MIXED: &str = include_str!("fixtures/search_mixed.json");
const RESUME: &str = include_str!("fixtures/resume.json");

/// A transport that records every request URL + method (so a report's routing is
/// asserted) and replays a fixture body for browse routes, `204` for the reports.
#[derive(Default)]
struct RecordingTransport {
    seen: RefCell<Vec<(HttpMethod, String)>>,
}

impl RecordingTransport {
    fn route(url: &str) -> Option<&'static str> {
        if url.contains("/PlaybackInfo") {
            Some(PLAYBACK_INFO)
        } else if url.contains("/LiveTv/Channels") {
            Some(LIVE_TV_CHANNELS)
        } else if url.contains("/LiveTv/Programs") {
            Some(GUIDE)
        } else if url.contains("/LiveTv/Recordings") {
            Some(RECORDINGS)
        } else if url.contains("/Items/Resume") {
            Some(RESUME)
        } else if url.contains("/Items") {
            Some(SEARCH_MIXED)
        } else {
            None
        }
    }

    /// The URLs seen so far, in order.
    fn urls(&self) -> Vec<String> {
        self.seen.borrow().iter().map(|(_, u)| u.clone()).collect()
    }

    /// The HTTP methods seen so far, in order.
    fn methods(&self) -> Vec<HttpMethod> {
        self.seen.borrow().iter().map(|(m, _)| *m).collect()
    }
}

impl HttpTransport for RecordingTransport {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.seen
            .borrow_mut()
            .push((request.method, request.url.clone()));
        // The `/Sessions/Playing*` reports + mark-played/unplayed answer 204.
        if request.url.contains("/Sessions/Playing") || request.url.contains("/PlayedItems/") {
            return Ok(HttpResponse {
                status: 204,
                body: Vec::new(),
            });
        }
        Ok(Self::route(&request.url).map_or_else(
            || HttpResponse {
                status: 404,
                body: b"{}".to_vec(),
            },
            |body| HttpResponse {
                status: 200,
                body: body.as_bytes().to_vec(),
            },
        ))
    }
}

fn device() -> ClientInfo {
    ClientInfo::new("mde-media", "workstation", "device-42", "12.0.0")
}

fn client() -> JellyfinClient<RecordingTransport> {
    JellyfinClient::new(
        "https://jelly.mesh:8096",
        device(),
        RecordingTransport::default(),
    )
    .with_auth("TOKEN", "user-9f3a")
}

/// A capability profile that can direct-play the 4K HEVC/EAC3/MKV fixture.
fn capable() -> ClientCapabilities {
    ClientCapabilities::new()
        .with_containers(["mkv", "mp4"])
        .with_video_codecs(["h264", "hevc"])
        .with_audio_codecs(["aac", "eac3"])
}

/// A profile that lacks EAC3 → the same title must transcode.
fn limited() -> ClientCapabilities {
    ClientCapabilities::new()
        .with_containers(["mkv", "mp4"])
        .with_video_codecs(["h264", "hevc"])
        .with_audio_codecs(["aac"])
}

#[test]
fn playback_info_then_direct_play_when_capable() {
    let client = client();
    let info = client
        .playback_info("movie-1", &capable())
        .expect("playback info");
    assert_eq!(info.play_session_id.as_deref(), Some("play-session-7"));
    let source = &info.media_sources[0];
    assert_eq!(source.container.as_deref(), Some("mkv"));

    let decision = build_playback_decision(
        client.base_url(),
        "movie-1",
        source,
        &capable(),
        StreamMediaType::Video,
        Some("TOKEN"),
        info.play_session_id.as_deref(),
    );
    assert_eq!(decision.method, PlaybackMethod::DirectPlay);
    assert!(decision.url.contains("/Videos/movie-1/stream?"));
    assert!(decision.url.contains("static=true"));
    assert_eq!(decision.media_source_id.as_deref(), Some("src-4k"));
    assert_eq!(decision.play_session_id.as_deref(), Some("play-session-7"));
}

#[test]
fn same_title_transcodes_when_a_codec_is_unsupported() {
    let client = client();
    let info = client
        .playback_info("movie-1", &limited())
        .expect("playback info");
    let decision = build_playback_decision(
        client.base_url(),
        "movie-1",
        &info.media_sources[0],
        &limited(),
        StreamMediaType::Video,
        Some("TOKEN"),
        info.play_session_id.as_deref(),
    );
    assert_eq!(decision.method, PlaybackMethod::Transcode);
    assert!(decision.url.contains("/Videos/movie-1/main.m3u8?"));
    assert!(decision.url.contains("playSessionId=play-session-7"));
}

#[test]
fn progress_reports_and_mark_played_drive_the_session_endpoints() {
    let client = client();
    let report = PlaybackReport::new("movie-1")
        .with_session(Some("src-4k".into()), Some("play-session-7".into()))
        .with_method(PlaybackMethod::DirectPlay)
        .at_secs(12.0);

    client.report_playback_start(&report).expect("start");
    let progress = report.at_secs(90.0);
    client
        .report_playback_progress(&progress)
        .expect("progress");
    client.report_playback_stopped(&progress).expect("stopped");
    client.mark_played("movie-1").expect("mark played");
    client.mark_unplayed("movie-1").expect("mark unplayed");

    let urls = client.transport().urls();
    assert!(urls.iter().any(|u| u.ends_with("/Sessions/Playing")));
    assert!(urls
        .iter()
        .any(|u| u.ends_with("/Sessions/Playing/Progress")));
    assert!(urls
        .iter()
        .any(|u| u.ends_with("/Sessions/Playing/Stopped")));
    assert!(urls
        .iter()
        .any(|u| u.ends_with("/Users/user-9f3a/PlayedItems/movie-1")));
    // The delete (mark-unplayed) hit the same path with the DELETE verb.
    assert!(client.transport().methods().contains(&HttpMethod::Delete));
}

#[test]
fn cross_library_search_returns_mixed_kinds() {
    let client = client();
    let resp = client.search("matrix", &[]).expect("search");
    assert_eq!(resp.items.len(), 2);
    let kinds: Vec<&str> = resp
        .items
        .iter()
        .filter_map(|i| i.item_type.as_deref())
        .collect();
    assert!(kinds.contains(&"Movie"));
    assert!(kinds.contains(&"MusicAlbum"));
}

#[test]
fn live_tv_channels_guide_and_recordings_parse() {
    let client = client();
    let channels = client.live_tv_channels().expect("channels");
    assert_eq!(channels.items.len(), 2);
    assert_eq!(channels.items[0].item_type.as_deref(), Some("TvChannel"));

    let guide = client
        .live_tv_guide(&["ch-news".to_string()])
        .expect("guide");
    assert_eq!(guide.items.len(), 2);
    assert_eq!(guide.items[0].item_type.as_deref(), Some("Program"));

    let recordings = client.recordings().expect("recordings");
    assert_eq!(recordings.items.len(), 1);
    assert_eq!(recordings.items[0].item_type.as_deref(), Some("Recording"));
}

#[test]
fn resume_position_reads_across_devices_from_a_browsed_item() {
    // The /Items/Resume row carries the server-persisted position another device set.
    let client = client();
    let resume = client.resume().expect("resume");
    let user_data = resume.items[0].user_data.as_ref().expect("user data");
    // The fixture's 3_000_000_000 ticks = 300 s.
    assert_eq!(resume_position_secs(user_data), Some(300.0));
}

#[test]
fn search_query_narrows_by_media_type() {
    // The typed query the search convenience builds carries the MediaTypes filter.
    let query = ItemsQuery::default()
        .search_term("matrix")
        .recursive()
        .media_types(["Audio"]);
    let req = mde_jellyfin::build_items_request(
        "https://jelly.mesh:8096",
        "user-9f3a",
        &query,
        &device(),
        Some("TOKEN"),
    );
    assert!(req.url.contains("MediaTypes=Audio"));
    assert!(req.url.contains("SearchTerm=matrix"));
    assert!(req.url.contains("Recursive=true"));
}
