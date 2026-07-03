//! Playback progress + sync (MEDIA-10): report a playing session to the server,
//! resume across devices, and mark titles played.
//!
//! Jellyfin persists the playback position from the client's progress reports, so
//! a title paused on one seat resumes on another — the server is the source of
//! truth. This module is the report side of that loop:
//!
//! - [`build_report_start_request`] → `POST /Sessions/Playing` when playback opens.
//! - [`build_report_progress_request`] → `POST /Sessions/Playing/Progress` on a
//!   heartbeat / pause / seek (this is what advances the server-side resume point).
//! - [`build_report_stopped_request`] → `POST /Sessions/Playing/Stopped` on stop.
//! - [`build_mark_played_request`] / [`build_mark_unplayed_request`] toggle the
//!   played flag (`POST` / `DELETE /Users/{id}/PlayedItems/{itemId}`).
//!
//! Reading the resume point is [`resume_position_secs`] over an item's
//! [`UserData`](crate::UserData) `PlaybackPositionTicks` (already carried by every
//! browse response). All the builders + conversions are pure and fixture-tested;
//! the round-trip to a live server is honest-gated.

use crate::client::{json_headers, trim_base, ClientInfo};
use crate::models::UserData;
use crate::net::HttpRequest;
use crate::playback::PlaybackMethod;

/// Jellyfin measures playback time in 100-nanosecond **ticks** — 10 000 000 per
/// second.
pub const TICKS_PER_SECOND: i64 = 10_000_000;

/// Convert a position in seconds to Jellyfin ticks (clamped at zero).
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn secs_to_ticks(secs: f64) -> i64 {
    if secs.is_finite() && secs > 0.0 {
        #[allow(clippy::cast_precision_loss)]
        let ticks = secs * TICKS_PER_SECOND as f64;
        ticks as i64
    } else {
        0
    }
}

/// Convert Jellyfin ticks to a position in seconds.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn ticks_to_secs(ticks: i64) -> f64 {
    ticks.max(0) as f64 / TICKS_PER_SECOND as f64
}

/// The resume position (seconds) an item carries, or [`None`] when it is at the
/// start — the read side of cross-device resume (the write side is a progress
/// report that persists the position server-side).
#[must_use]
pub fn resume_position_secs(user_data: &UserData) -> Option<f64> {
    if user_data.playback_position_ticks > 0 {
        Some(ticks_to_secs(user_data.playback_position_ticks))
    } else {
        None
    }
}

/// The state one playback report carries.
///
/// The item, its session ids, the current position, and how it is being played.
/// One struct feeds all three `/Sessions/Playing*` endpoints; each builder
/// projects the fields that endpoint takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackReport {
    /// The item being played.
    pub item_id: String,
    /// The media source id the [`PlaybackDecision`](crate::PlaybackDecision) streams.
    pub media_source_id: Option<String>,
    /// The `PlaySessionId` from `PlaybackInfo`, tying the reports together.
    pub play_session_id: Option<String>,
    /// The current position, in ticks.
    pub position_ticks: i64,
    /// Whether playback is currently paused.
    pub is_paused: bool,
    /// How the item is being delivered (`PlayMethod`).
    pub play_method: PlaybackMethod,
    /// The active audio stream index, if a selection is made.
    pub audio_stream_index: Option<i32>,
    /// The active subtitle stream index, if a selection is made.
    pub subtitle_stream_index: Option<i32>,
}

impl PlaybackReport {
    /// A report for `item_id` at the start (position 0, playing, direct-play).
    #[must_use]
    pub fn new(item_id: impl Into<String>) -> Self {
        Self {
            item_id: item_id.into(),
            media_source_id: None,
            play_session_id: None,
            position_ticks: 0,
            is_paused: false,
            play_method: PlaybackMethod::DirectPlay,
            audio_stream_index: None,
            subtitle_stream_index: None,
        }
    }

    /// Set the position from seconds (builder form).
    #[must_use]
    pub fn at_secs(mut self, secs: f64) -> Self {
        self.position_ticks = secs_to_ticks(secs);
        self
    }

    /// Set the media-source + play-session ids (builder form).
    #[must_use]
    pub fn with_session(
        mut self,
        media_source_id: Option<String>,
        play_session_id: Option<String>,
    ) -> Self {
        self.media_source_id = media_source_id;
        self.play_session_id = play_session_id;
        self
    }

    /// Set the delivery method (builder form).
    #[must_use]
    pub const fn with_method(mut self, method: PlaybackMethod) -> Self {
        self.play_method = method;
        self
    }

    /// Set the paused flag (builder form).
    #[must_use]
    pub const fn paused(mut self, is_paused: bool) -> Self {
        self.is_paused = is_paused;
        self
    }

    /// The current position in seconds.
    #[must_use]
    pub fn position_secs(&self) -> f64 {
        ticks_to_secs(self.position_ticks)
    }

    /// The fields common to every report body.
    fn base_body(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut map = serde_json::Map::new();
        map.insert("ItemId".into(), self.item_id.clone().into());
        map.insert("PositionTicks".into(), self.position_ticks.into());
        if let Some(id) = &self.media_source_id {
            map.insert("MediaSourceId".into(), id.clone().into());
        }
        if let Some(id) = &self.play_session_id {
            map.insert("PlaySessionId".into(), id.clone().into());
        }
        if let Some(index) = self.audio_stream_index {
            map.insert("AudioStreamIndex".into(), index.into());
        }
        if let Some(index) = self.subtitle_stream_index {
            map.insert("SubtitleStreamIndex".into(), index.into());
        }
        map
    }
}

/// Serialize a body map to JSON bytes (empty on the impossible serialize failure).
fn body_bytes(map: serde_json::Map<String, serde_json::Value>) -> Vec<u8> {
    serde_json::to_vec(&serde_json::Value::Object(map)).unwrap_or_default()
}

/// Build the `POST /Sessions/Playing` report — playback has opened.
#[must_use]
pub fn build_report_start_request(
    base_url: &str,
    report: &PlaybackReport,
    device: &ClientInfo,
    token: Option<&str>,
) -> HttpRequest {
    let mut body = report.base_body();
    body.insert("PlayMethod".into(), report.play_method.as_wire().into());
    body.insert("CanSeek".into(), true.into());
    body.insert("IsPaused".into(), report.is_paused.into());
    HttpRequest::post(
        format!("{}/Sessions/Playing", trim_base(base_url)),
        json_headers(device, token, true),
        body_bytes(body),
    )
}

/// Build the `POST /Sessions/Playing/Progress` heartbeat — this is what advances
/// the server-side resume point that another device restores.
#[must_use]
pub fn build_report_progress_request(
    base_url: &str,
    report: &PlaybackReport,
    device: &ClientInfo,
    token: Option<&str>,
) -> HttpRequest {
    let mut body = report.base_body();
    body.insert("PlayMethod".into(), report.play_method.as_wire().into());
    body.insert("IsPaused".into(), report.is_paused.into());
    body.insert("EventName".into(), "TimeUpdate".into());
    HttpRequest::post(
        format!("{}/Sessions/Playing/Progress", trim_base(base_url)),
        json_headers(device, token, true),
        body_bytes(body),
    )
}

/// Build the `POST /Sessions/Playing/Stopped` report — playback ended; the final
/// position is the persisted resume point.
#[must_use]
pub fn build_report_stopped_request(
    base_url: &str,
    report: &PlaybackReport,
    device: &ClientInfo,
    token: Option<&str>,
) -> HttpRequest {
    HttpRequest::post(
        format!("{}/Sessions/Playing/Stopped", trim_base(base_url)),
        json_headers(device, token, true),
        body_bytes(report.base_body()),
    )
}

/// Build the `POST /Users/{userId}/PlayedItems/{itemId}` request — mark played.
#[must_use]
pub fn build_mark_played_request(
    base_url: &str,
    user_id: &str,
    item_id: &str,
    device: &ClientInfo,
    token: Option<&str>,
) -> HttpRequest {
    HttpRequest::post(
        format!(
            "{}/Users/{}/PlayedItems/{}",
            trim_base(base_url),
            user_id,
            item_id
        ),
        json_headers(device, token, true),
        Vec::new(),
    )
}

/// Build the `DELETE /Users/{userId}/PlayedItems/{itemId}` request — mark unplayed.
#[must_use]
pub fn build_mark_unplayed_request(
    base_url: &str,
    user_id: &str,
    item_id: &str,
    device: &ClientInfo,
    token: Option<&str>,
) -> HttpRequest {
    HttpRequest::delete(
        format!(
            "{}/Users/{}/PlayedItems/{}",
            trim_base(base_url),
            user_id,
            item_id
        ),
        json_headers(device, token, false),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::HttpMethod;

    fn device() -> ClientInfo {
        ClientInfo::new("mde-media", "workstation", "device-42", "12.0.0")
    }

    fn report() -> PlaybackReport {
        PlaybackReport::new("movie-1")
            .with_session(Some("src-1".into()), Some("sess-9".into()))
            .with_method(PlaybackMethod::Transcode)
            .at_secs(30.0)
    }

    #[test]
    fn ticks_round_trip_seconds() {
        assert_eq!(secs_to_ticks(1.0), 10_000_000);
        assert_eq!(secs_to_ticks(0.0), 0);
        assert_eq!(secs_to_ticks(-5.0), 0);
        assert!((ticks_to_secs(30_000_000) - 3.0).abs() < f64::EPSILON);
        // A round-trip preserves whole seconds.
        assert!((ticks_to_secs(secs_to_ticks(42.0)) - 42.0).abs() < f64::EPSILON);
    }

    #[test]
    fn resume_position_reads_user_data_ticks() {
        let ud = UserData {
            playback_position_ticks: 30_000_000,
            ..UserData::default()
        };
        assert_eq!(resume_position_secs(&ud), Some(3.0));
        // At the start → no resume.
        assert_eq!(resume_position_secs(&UserData::default()), None);
    }

    #[test]
    fn start_report_carries_play_method_and_position() {
        let req = build_report_start_request("https://j.mesh/", &report(), &device(), Some("T"));
        assert_eq!(req.method, HttpMethod::Post);
        assert_eq!(req.url, "https://j.mesh/Sessions/Playing");
        let json: serde_json::Value =
            serde_json::from_slice(&req.body.expect("body")).expect("json");
        assert_eq!(json["ItemId"], "movie-1");
        assert_eq!(json["MediaSourceId"], "src-1");
        assert_eq!(json["PlaySessionId"], "sess-9");
        assert_eq!(json["PositionTicks"], 300_000_000_i64);
        assert_eq!(json["PlayMethod"], "Transcode");
        assert_eq!(json["CanSeek"], true);
    }

    #[test]
    fn progress_report_tags_a_time_update_event() {
        let req = build_report_progress_request("https://j.mesh", &report(), &device(), None);
        assert_eq!(req.url, "https://j.mesh/Sessions/Playing/Progress");
        let json: serde_json::Value =
            serde_json::from_slice(&req.body.expect("body")).expect("json");
        assert_eq!(json["EventName"], "TimeUpdate");
        assert_eq!(json["PositionTicks"], 300_000_000_i64);
    }

    #[test]
    fn stopped_report_carries_final_position() {
        let req = build_report_stopped_request("https://j.mesh", &report(), &device(), None);
        assert_eq!(req.url, "https://j.mesh/Sessions/Playing/Stopped");
        let json: serde_json::Value =
            serde_json::from_slice(&req.body.expect("body")).expect("json");
        assert_eq!(json["ItemId"], "movie-1");
        assert_eq!(json["PositionTicks"], 300_000_000_i64);
        // The stopped body does not carry the transient IsPaused/EventName.
        assert!(json.get("EventName").is_none());
    }

    #[test]
    fn mark_played_posts_and_unplayed_deletes() {
        let played =
            build_mark_played_request("https://j.mesh", "user-1", "movie-1", &device(), None);
        assert_eq!(played.method, HttpMethod::Post);
        assert_eq!(
            played.url,
            "https://j.mesh/Users/user-1/PlayedItems/movie-1"
        );

        let unplayed =
            build_mark_unplayed_request("https://j.mesh", "user-1", "movie-1", &device(), None);
        assert_eq!(unplayed.method, HttpMethod::Delete);
        assert_eq!(
            unplayed.url,
            "https://j.mesh/Users/user-1/PlayedItems/movie-1"
        );
    }

    #[test]
    fn report_position_secs_reads_back() {
        assert!((report().position_secs() - 30.0).abs() < f64::EPSILON);
    }
}
