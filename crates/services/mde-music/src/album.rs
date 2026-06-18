//! AIR-12 (v6.1) — the album detail page (data + Bus actions).
//!
//! Opening an album fetches its tracks from the daemon over the Bus
//! (`action/music/get-album` → reply) and the page acts on them: Play /
//! Shuffle / Add-to-Queue for the whole album, Play-Next / Add for a
//! single track. Per the Q96 Bus-canonical lock the GUI never calls
//! Airsonic directly. [`parse_album`] + [`shuffle_ids`] + [`fmt_duration`]
//! are pure + unit-tested; the Iced view lives in `main.rs` (it needs the
//! binary's `Message`).

use std::time::Duration;

use mde_bus::hooks::config::Priority;
use serde_json::Value;

/// One track row on the album page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    /// Airsonic song id, used for queue/play actions.
    pub id: String,
    /// Display title (falls back to the id when absent from the daemon reply).
    pub title: String,
    /// 1-based track number within the album, `None` when unset.
    pub track_no: Option<u32>,
    /// Track length in seconds as reported by the daemon.
    pub duration: u32,
}

/// A fetched album: metadata + ordered tracks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AlbumView {
    /// Airsonic album id, used as the body of `action/music/get-album` requests.
    pub id: String,
    /// Album title (falls back to the id when the daemon omits it).
    pub name: String,
    /// Primary artist name; empty string when absent.
    pub artist: String,
    /// Release year, `None` when unset in the Airsonic catalogue.
    pub year: Option<u32>,
    /// The `coverArt` id (AIR-16 dominant-colour fetch); empty when absent.
    pub cover_art: String,
    /// Ordered list of tracks as returned by the daemon.
    pub tracks: Vec<Track>,
}

impl AlbumView {
    /// The track ids in album order (for Play / Add-to-Queue).
    #[must_use]
    pub fn track_ids(&self) -> Vec<String> {
        self.tracks.iter().map(|t| t.id.clone()).collect()
    }

    /// Total album duration in seconds.
    #[must_use]
    pub fn total_secs(&self) -> u32 {
        self.tracks.iter().map(|t| t.duration).sum()
    }
}

/// Parse the daemon's `{ok, result:{album:{...}, songs:[...]}}` reply into
/// an [`AlbumView`]. `None` on `ok:false` / malformed / missing album.
#[must_use]
pub fn parse_album(reply_json: &str) -> Option<AlbumView> {
    let v: Value = serde_json::from_str(reply_json).ok()?;
    if v.get("ok").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let result = v.get("result")?;
    let album = result.get("album")?;
    let id = album.get("id").and_then(Value::as_str)?.to_string();
    let name = album
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(&id)
        .to_string();
    let artist = album
        .get("artist")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let cover_art = album
        .get("coverArt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let year = album
        .get("year")
        .and_then(Value::as_u64)
        .and_then(|y| u32::try_from(y).ok());
    let tracks = result
        .get("songs")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    let id = s.get("id").and_then(Value::as_str)?.to_string();
                    let title = s
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or(&id)
                        .to_string();
                    let track_no = s
                        .get("track")
                        .and_then(Value::as_u64)
                        .and_then(|n| u32::try_from(n).ok());
                    let duration = s
                        .get("duration")
                        .and_then(Value::as_u64)
                        .and_then(|n| u32::try_from(n).ok())
                        .unwrap_or(0);
                    Some(Track {
                        id,
                        title,
                        track_no,
                        duration,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(AlbumView {
        id,
        name,
        artist,
        year,
        cover_art,
        tracks,
    })
}

/// Format seconds as `M:SS` for the duration column.
#[must_use]
pub fn fmt_duration(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// Shuffle `ids` in place (Fisher-Yates, wall-clock-seeded xorshift) for
/// the album Shuffle button — multiset-preserving, no `rand` dependency.
#[must_use]
pub fn shuffle_ids(mut ids: Vec<String>) -> Vec<String> {
    let mut r = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos() % u128::from(u64::MAX)).unwrap_or(1))
        .unwrap_or(1)
        .max(1);
    for i in (1..ids.len()).rev() {
        r ^= r << 13;
        r ^= r >> 7;
        r ^= r << 17;
        let j = (r % (i as u64 + 1)) as usize;
        ids.swap(i, j);
    }
    ids
}

// ───────────────────────── Bus actions ─────────────────────────

/// Run `f` against a freshly-opened Bus store on a blocking thread (the
/// rusqlite `Persist` isn't `Send`, so it can't cross Iced's executor); a
/// local current-thread runtime drives the async requests inside.
pub(crate) async fn with_bus<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce(&mde_bus::persist::Persist, &tokio::runtime::Runtime) -> Result<T, String>
        + Send
        + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || -> Result<T, String> {
        let bus_root = mde_bus::default_data_dir().ok_or_else(|| "no Bus data dir".to_string())?;
        let persist =
            mde_bus::persist::Persist::open(bus_root).map_err(|e| format!("Bus store: {e}"))?;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        f(&persist, &rt)
    })
    .await
    .map_err(|e| format!("bus task join error: {e}"))?
}

/// One `action/music/<topic>` request, returning the reply body.
pub(crate) fn req(
    persist: &mde_bus::persist::Persist,
    rt: &tokio::runtime::Runtime,
    topic: &str,
    body: Option<&str>,
) -> Result<String, String> {
    let reply = rt
        .block_on(mde_bus::rpc::request(
            persist,
            topic,
            Priority::Default,
            None,
            body,
            Duration::from_secs(5),
        ))
        .map_err(|e| format!("daemon not responding ({e})"))?;
    Ok(reply.body.unwrap_or_default())
}

/// Fetch an album over the Bus (`action/music/get-album`, id in the body).
///
/// # Errors
/// Bus-store / request / timeout failures, or an unparseable reply.
pub async fn fetch_album(id: String) -> Result<AlbumView, String> {
    with_bus(move |persist, rt| {
        let reply = req(persist, rt, "action/music/get-album", Some(&id))?;
        parse_album(&reply).ok_or_else(|| "album not found".to_string())
    })
    .await
}

/// Replace the queue with `ids` and start playback (Play / Shuffle the
/// album): clear → enqueue each → play, in one Bus session.
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn play_ids(ids: Vec<String>) -> Result<(), String> {
    with_bus(move |persist, rt| {
        req(persist, rt, "action/music/clear", None)?;
        for id in &ids {
            req(persist, rt, "action/music/enqueue", Some(id))?;
        }
        req(persist, rt, "action/music/play", None)?;
        Ok(())
    })
    .await
}

/// AIR-4.b — play a whole playlist by id: fetch its songs over the Bus
/// (`action/music/get-playlist`), then clear → enqueue each → play. The
/// Playlists hub card's click action. Reuses [`crate::library::parse_items`]
/// to pull the song ids out of the daemon's `{songs:[…]}` reply.
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn play_playlist(id: String) -> Result<(), String> {
    with_bus(move |persist, rt| {
        let reply = req(persist, rt, "action/music/get-playlist", Some(&id))?;
        let ids: Vec<String> = crate::library::parse_items(&reply)
            .into_iter()
            .map(|item| item.id)
            .collect();
        req(persist, rt, "action/music/clear", None)?;
        for sid in &ids {
            req(persist, rt, "action/music/enqueue", Some(sid))?;
        }
        req(persist, rt, "action/music/play", None)?;
        Ok(())
    })
    .await
}

/// MUSIC-RFX-6 — create a new (empty) playlist by name
/// (`action/music/playlist-create`, body `{"name":}`). Tracks are added via
/// RFX-7's add-to-playlist; reorder is RFX-6b.
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn playlist_create(name: String) -> Result<(), String> {
    let body = serde_json::json!({ "name": name }).to_string();
    with_bus(move |persist, rt| {
        req(persist, rt, "action/music/playlist-create", Some(&body))?;
        Ok(())
    })
    .await
}

/// MUSIC-RFX-7 — add a track to a playlist (`action/music/playlist-update`,
/// body `{"id":,"add":[song_id]}`). The reusable "Add to playlist" primitive.
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn playlist_add_track(playlist_id: String, song_id: String) -> Result<(), String> {
    let body = serde_json::json!({ "id": playlist_id, "add": [song_id] }).to_string();
    with_bus(move |persist, rt| {
        req(persist, rt, "action/music/playlist-update", Some(&body))?;
        Ok(())
    })
    .await
}

/// MUSIC-RFX-6 — rename a playlist (`action/music/playlist-update`, body
/// `{"id":,"name":}`).
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn playlist_rename(id: String, name: String) -> Result<(), String> {
    let body = serde_json::json!({ "id": id, "name": name }).to_string();
    with_bus(move |persist, rt| {
        req(persist, rt, "action/music/playlist-update", Some(&body))?;
        Ok(())
    })
    .await
}

/// MUSIC-RFX-6 — delete a playlist (`action/music/playlist-delete`, body
/// `{"id":}`).
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn playlist_delete(id: String) -> Result<(), String> {
    let body = serde_json::json!({ "id": id }).to_string();
    with_bus(move |persist, rt| {
        req(persist, rt, "action/music/playlist-delete", Some(&body))?;
        Ok(())
    })
    .await
}

/// Append `ids` to the queue without disrupting playback (Add to Queue).
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn enqueue_ids(ids: Vec<String>) -> Result<(), String> {
    with_bus(move |persist, rt| {
        for id in &ids {
            req(persist, rt, "action/music/enqueue", Some(id))?;
        }
        Ok(())
    })
    .await
}

/// Insert one track right after the current one (Play Next).
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn play_next(id: String) -> Result<(), String> {
    with_bus(move |persist, rt| {
        req(persist, rt, "action/music/enqueue-after", Some(&id))?;
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_album_meta_and_tracks() {
        let reply = r#"{"ok":true,"result":{
            "album":{"id":"al1","name":"Kind of Blue","artist":"Miles Davis","year":1959},
            "songs":[
                {"id":"s1","title":"So What","track":1,"duration":545},
                {"id":"s2","title":"Freddie Freeloader","track":2,"duration":586}
            ]
        }}"#;
        let a = parse_album(reply).unwrap();
        assert_eq!(a.name, "Kind of Blue");
        assert_eq!(a.artist, "Miles Davis");
        assert_eq!(a.year, Some(1959));
        assert_eq!(a.tracks.len(), 2);
        assert_eq!(a.tracks[0].title, "So What");
        assert_eq!(a.tracks[1].track_no, Some(2));
        assert_eq!(a.track_ids(), vec!["s1", "s2"]);
        assert_eq!(a.total_secs(), 545 + 586);
    }

    #[test]
    fn parse_failures_and_fallbacks() {
        assert!(parse_album(r#"{"ok":false,"error":"x"}"#).is_none());
        assert!(parse_album("not json").is_none());
        assert!(parse_album(r#"{"ok":true,"result":{}}"#).is_none());
        // Missing title falls back to id; absent songs → empty track list.
        let a = parse_album(r#"{"ok":true,"result":{"album":{"id":"x"}}}"#).unwrap();
        assert_eq!(a.name, "x");
        assert!(a.tracks.is_empty());
    }

    #[test]
    fn duration_formats_mss() {
        assert_eq!(fmt_duration(0), "0:00");
        assert_eq!(fmt_duration(9), "0:09");
        assert_eq!(fmt_duration(545), "9:05");
        assert_eq!(fmt_duration(586), "9:46");
    }

    #[test]
    fn shuffle_preserves_the_track_set() {
        let ids: Vec<String> = (0..25).map(|i| i.to_string()).collect();
        let mut shuffled = shuffle_ids(ids.clone());
        shuffled.sort();
        let mut original = ids;
        original.sort();
        assert_eq!(shuffled, original);
        // Degenerate sizes don't panic.
        assert!(shuffle_ids(vec![]).is_empty());
        assert_eq!(shuffle_ids(vec!["solo".into()]), vec!["solo".to_string()]);
    }
}
