//! AIR-15 core (v6.1) — now-playing state + transport, over the Bus.
//!
//! The now-playing footer polls `action/music/get-state` for the live
//! transport snapshot and resolves the current `song_id` to a title via
//! `action/music/get-song`. Transport buttons drive `action/music/{pause,
//! resume}` + the queue `{next,prev}` (each followed by `play` to skip
//! during playback). Per the Q96 Bus-canonical lock the GUI never calls
//! Airsonic directly. [`parse_state`] / [`parse_song_meta`] are pure +
//! unit-tested; the Bus round-trips reuse [`crate::album`]'s helpers.
//!
//! The maxi-player's Queue / Lyrics / Peers tabs + the scrub bar + volume
//! slider are AIR-15 follow-ons; this is the transport core (the app's
//! first in-app play/pause/skip after playback starts).

use serde_json::Value;

use crate::album::{req, with_bus};

/// Poll cadence for the live now-playing snapshot.
pub const POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// The live transport snapshot from `get-state`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NowState {
    /// `true` when the engine is actively playing (not paused).
    pub playing: bool,
    /// `true` when a track is loaded (even if paused); `false` when the queue is idle.
    pub active: bool,
    /// Airsonic song id of the current track; empty when nothing is loaded.
    pub song_id: String,
    /// Current playback position in milliseconds.
    pub position_ms: u64,
    /// AIR-15.b.3 — playback volume 0.0..=1.0 (engine.volume() via get-state).
    pub volume: f32,
    /// AUDIT-MESH-4 — `true` when this peer has a working audio output device.
    /// `false` on a headless peer: the panel shows "no audio device here"
    /// rather than a misleading idle transport.
    pub audio_available: bool,
    /// AUDIT-MESH-4 — `true` when no Airsonic server is configured yet, so the
    /// panel can prompt the operator to configure one instead of looking idle.
    pub needs_airsonic: bool,
    /// MUSIC-RFX-2/4 — `true` when the current track is seekable (a finite
    /// track). `false` for a live/radio stream, so the maxi view shows an
    /// interactive scrub slider only when it can actually seek.
    pub seekable: bool,
}

impl NowState {
    /// Whether anything is loaded (a footer is worth showing).
    #[must_use]
    pub fn has_track(&self) -> bool {
        !self.song_id.is_empty()
    }

    /// AUDIT-MESH-4 — whether the panel should show a "configure Airsonic"
    /// prompt: the daemon answered, no server is set up, and nothing is loaded.
    #[must_use]
    pub fn needs_config(&self) -> bool {
        self.needs_airsonic && !self.has_track()
    }
}

/// Parse the `get-state` reply (`{ok, playing, active, position_ms,
/// song_id}` — a transport verb, so flat, not `{result}`-wrapped). The
/// idle default on `ok:false` / malformed.
#[must_use]
pub fn parse_state(reply_json: &str) -> NowState {
    let Ok(v) = serde_json::from_str::<Value>(reply_json) else {
        return NowState::default();
    };
    if v.get("ok").and_then(Value::as_bool) != Some(true) {
        return NowState::default();
    }
    NowState {
        playing: v.get("playing").and_then(Value::as_bool).unwrap_or(false),
        active: v.get("active").and_then(Value::as_bool).unwrap_or(false),
        song_id: v
            .get("song_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        position_ms: v.get("position_ms").and_then(Value::as_u64).unwrap_or(0),
        volume: v
            .get("volume")
            .and_then(Value::as_f64)
            .map(|x| x as f32)
            .unwrap_or(1.0),
        // AUDIT-MESH-4 — older daemons omit these; absence means "capable"
        // (audio present) / "configured" (no needs-Airsonic prompt) so the
        // panel doesn't regress to a config prompt against a pre-fix peer.
        audio_available: v
            .get("audio_available")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        needs_airsonic: v
            .get("needs_airsonic")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        // MUSIC-RFX-2/4 — older daemons omit `seekable`; absence means "not
        // seekable" (hide the scrub slider) rather than a false-positive drag.
        seekable: v.get("seekable").and_then(Value::as_bool).unwrap_or(false),
    }
}

/// Parse a `get-song` reply (`{ok, result:{song:{...}}}` — a browse verb,
/// so `{result}`-wrapped) into `(title, artist)`. `None` on failure.
#[must_use]
pub fn parse_song_meta(reply_json: &str) -> Option<(String, String)> {
    let v: Value = serde_json::from_str(reply_json).ok()?;
    if v.get("ok").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let song = v.get("result")?.get("song")?;
    let title = song.get("title").and_then(Value::as_str)?.to_string();
    let artist = song
        .get("artist")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Some((title, artist))
}

// ───────────────────────── Bus actions ─────────────────────────

/// Fetch the live transport snapshot.
///
/// # Errors
/// Bus-store / request / timeout failures (daemon not running).
pub async fn fetch_state() -> Result<NowState, String> {
    with_bus(|p, rt| Ok(parse_state(&req(p, rt, "action/music/get-state", None)?))).await
}

/// Parse the `get-queue` reply (`{ok, len, current, songs:[id]}`) into the
/// queue song-ids + the current-track index.
#[must_use]
pub fn parse_queue(reply_json: &str) -> (Vec<String>, usize) {
    let Ok(v) = serde_json::from_str::<Value>(reply_json) else {
        return (Vec::new(), 0);
    };
    let songs = v
        .get("songs")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let current = v.get("current").and_then(Value::as_u64).unwrap_or(0) as usize;
    (songs, current)
}

/// Fetch the play queue over the Bus (`action/music/get-queue`).
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn fetch_queue() -> Result<(Vec<String>, usize), String> {
    with_bus(|p, rt| Ok(parse_queue(&req(p, rt, "action/music/get-queue", None)?))).await
}

/// MUSIC-RFX-5 — move the queue track at `from` to index `to`
/// (`action/music/queue-move`, body `{"from":,"to":}`). Reorders + persists
/// daemon-side, keeping the cursor on the playing track (RFX-1).
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn queue_move(from: usize, to: usize) -> Result<(), String> {
    let body = serde_json::json!({ "from": from, "to": to }).to_string();
    with_bus(move |p, rt| {
        req(p, rt, "action/music/queue-move", Some(&body))?;
        Ok(())
    })
    .await
}

/// MUSIC-RFX-5 — remove the queue track at `idx`
/// (`action/music/queue-remove`, body `{"index":}`).
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn queue_remove(idx: usize) -> Result<(), String> {
    let body = serde_json::json!({ "index": idx }).to_string();
    with_bus(move |p, rt| {
        req(p, rt, "action/music/queue-remove", Some(&body))?;
        Ok(())
    })
    .await
}

/// MUSIC-RFX-5 — remove a multi-selected set of queue indices
/// (`action/music/queue-remove-many`, body `{"indices":[…]}`).
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn queue_remove_many(indices: Vec<usize>) -> Result<(), String> {
    let body = serde_json::json!({ "indices": indices }).to_string();
    with_bus(move |p, rt| {
        req(p, rt, "action/music/queue-remove-many", Some(&body))?;
        Ok(())
    })
    .await
}

/// MUSIC-RFX-5 — move the queue track at `idx` to play immediately after the
/// current one (`action/music/queue-move-to-next`, body `{"index":}`).
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn queue_move_to_next(idx: usize) -> Result<(), String> {
    let body = serde_json::json!({ "index": idx }).to_string();
    with_bus(move |p, rt| {
        req(p, rt, "action/music/queue-move-to-next", Some(&body))?;
        Ok(())
    })
    .await
}

/// Parse a `get-lyrics` reply (`{ok, result:{lyrics:[line]}}`) into lines.
#[must_use]
pub fn parse_lyrics_reply(reply_json: &str) -> Vec<String> {
    serde_json::from_str::<Value>(reply_json)
        .ok()
        .and_then(|v| {
            v.get("result")?
                .get("lyrics")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
        })
        .unwrap_or_default()
}

/// Fetch the current song's lyrics over the Bus (`action/music/get-lyrics`).
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn fetch_lyrics(song_id: String) -> Result<Vec<String>, String> {
    with_bus(move |p, rt| {
        Ok(parse_lyrics_reply(&req(
            p,
            rt,
            "action/music/get-lyrics",
            Some(&song_id),
        )?))
    })
    .await
}

/// AIR-15.b.5 — a peer's last music activity snapshot (Peers-tab roster).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerState {
    /// Overlay hostname of the peer node.
    pub host: String,
    /// `true` when the peer is actively playing.
    pub playing: bool,
    /// Airsonic song id the peer last reported; empty when idle.
    pub song_id: String,
}

/// Parse a `peer-states` reply (`{ok, result:{peers:[{peer,playing,song_id}]}}`).
#[must_use]
pub fn parse_peer_states(reply_json: &str) -> Vec<PeerState> {
    serde_json::from_str::<Value>(reply_json)
        .ok()
        .and_then(|v| {
            v.get("result")?
                .get("peers")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|p| {
                            Some(PeerState {
                                host: p.get("peer").and_then(Value::as_str)?.to_string(),
                                playing: p.get("playing").and_then(Value::as_bool).unwrap_or(false),
                                song_id: p
                                    .get("song_id")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string(),
                            })
                        })
                        .collect()
                })
        })
        .unwrap_or_default()
}

/// Fetch the peer roster over the Bus (`action/music/peer-states`).
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn fetch_peer_states() -> Result<Vec<PeerState>, String> {
    with_bus(|p, rt| {
        Ok(parse_peer_states(&req(
            p,
            rt,
            "action/music/peer-states",
            None,
        )?))
    })
    .await
}

/// Post an AIR-8 take-over intent asking `peer` to yield
/// (`action/music/take-over`, the peer host in the body).
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn take_over(peer: String) -> Result<(), String> {
    with_bus(move |p, rt| {
        req(p, rt, "action/music/take-over", Some(&peer))?;
        Ok(())
    })
    .await
}

/// Resolve a song id to `(title, artist)` via `get-song`, falling back to
/// the id as the title when the lookup fails (so the footer never blanks).
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn resolve_song(id: String) -> Result<(String, String), String> {
    with_bus(move |p, rt| {
        let reply = req(p, rt, "action/music/get-song", Some(&id))?;
        Ok(parse_song_meta(&reply).unwrap_or((id.clone(), String::new())))
    })
    .await
}

/// Extract the `coverArt` token from a `get-song` reply
/// (`{ok, result:{song:{coverArt}}}`), if present + non-empty.
#[must_use]
pub fn parse_cover_id(reply_json: &str) -> Option<String> {
    let v: Value = serde_json::from_str(reply_json).ok()?;
    if v.get("ok").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let cover = v
        .get("result")?
        .get("song")?
        .get("coverArt")
        .and_then(Value::as_str)?;
    (!cover.is_empty()).then(|| cover.to_string())
}

/// Resolve a song id to its `coverArt` token via `get-song` (for the maxi /
/// now-playing cover art).
///
/// # Errors
/// Bus-store / request / timeout failures.
/// Extract the song's duration (server reports seconds; returned as ms) from
/// a `get-song` reply, for the maxi scrub bar. 0 when absent.
#[must_use]
pub fn parse_song_duration(reply_json: &str) -> u64 {
    serde_json::from_str::<Value>(reply_json)
        .ok()
        .and_then(|v| {
            v.get("result")?
                .get("song")?
                .get("duration")
                .and_then(Value::as_u64)
        })
        .unwrap_or(0)
        * 1000
}

/// Resolve a song id to its `(coverArt token, duration_ms)` via `get-song`
/// (for the maxi cover art + scrub bar).
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn resolve_now_meta(song_id: String) -> Result<(Option<String>, u64), String> {
    with_bus(move |p, rt| {
        let reply = req(p, rt, "action/music/get-song", Some(&song_id))?;
        Ok((parse_cover_id(&reply), parse_song_duration(&reply)))
    })
    .await
}

/// Set the playback volume (0.0..=1.0) via `action/music/set-volume` (the
/// daemon accepts a bare number body).
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn set_volume(v: f32) -> Result<(), String> {
    with_bus(move |p, rt| {
        req(p, rt, "action/music/set-volume", Some(&v.to_string()))?;
        Ok(())
    })
    .await
}

/// MUSIC-RFX-2/4 — seek the current (finite) track to `position_ms` via
/// `action/music/seek` (the daemon accepts a bare-number body). A no-op for a
/// live stream daemon-side; the maxi view only offers the slider when seekable.
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn seek(position_ms: u64) -> Result<(), String> {
    with_bus(move |p, rt| {
        req(p, rt, "action/music/seek", Some(&position_ms.to_string()))?;
        Ok(())
    })
    .await
}

/// Toggle play/pause based on the current state (`pause` when playing,
/// `resume` otherwise).
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn play_pause(currently_playing: bool) -> Result<(), String> {
    let verb = if currently_playing {
        "action/music/pause"
    } else {
        "action/music/resume"
    };
    with_bus(move |p, rt| {
        req(p, rt, verb, None)?;
        Ok(())
    })
    .await
}

/// Skip to the next queued track + (re)play it.
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn skip_next() -> Result<(), String> {
    with_bus(|p, rt| {
        req(p, rt, "action/music/next", None)?;
        req(p, rt, "action/music/play", None)?;
        Ok(())
    })
    .await
}

/// Skip to the previous queued track + (re)play it.
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn skip_prev() -> Result<(), String> {
    with_bus(|p, rt| {
        req(p, rt, "action/music/prev", None)?;
        req(p, rt, "action/music/play", None)?;
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lyrics_reply_reads_lines() {
        let r = r#"{"ok":true,"result":{"lyrics":["one","two"]}}"#;
        assert_eq!(
            parse_lyrics_reply(r),
            vec!["one".to_string(), "two".to_string()]
        );
        assert!(parse_lyrics_reply(r#"{"ok":true,"result":{}}"#).is_empty());
    }

    #[test]
    fn parse_peer_states_reads_roster() {
        let r = r#"{"ok":true,"result":{"peers":[
            {"peer":"anvil","playing":true,"song_id":"s1"},
            {"peer":"forge","playing":false}
        ]}}"#;
        let ps = parse_peer_states(r);
        assert_eq!(ps.len(), 2);
        assert_eq!(ps[0].host, "anvil");
        assert!(ps[0].playing);
        assert_eq!(ps[0].song_id, "s1");
        assert_eq!(ps[1].host, "forge");
        assert!(!ps[1].playing);
        assert!(parse_peer_states("nope").is_empty());
    }

    #[test]
    fn parse_queue_reads_songs_and_current() {
        let (songs, current) =
            parse_queue(r#"{"ok":true,"len":2,"current":1,"songs":["s1","s2"]}"#);
        assert_eq!(songs, vec!["s1".to_string(), "s2".to_string()]);
        assert_eq!(current, 1);
        assert_eq!(parse_queue("nope"), (Vec::new(), 0));
    }

    #[test]
    fn parse_cover_id_reads_cover_art() {
        assert_eq!(
            parse_cover_id(r#"{"ok":true,"result":{"song":{"title":"X","coverArt":"al-9"}}}"#)
                .as_deref(),
            Some("al-9")
        );
        assert_eq!(
            parse_cover_id(r#"{"ok":true,"result":{"song":{"title":"X"}}}"#),
            None
        );
        assert_eq!(parse_cover_id("nope"), None);
    }

    #[test]
    fn parse_song_duration_seconds_to_ms() {
        assert_eq!(
            parse_song_duration(r#"{"ok":true,"result":{"song":{"duration":215}}}"#),
            215_000
        );
        assert_eq!(parse_song_duration("nope"), 0);
    }

    #[test]
    fn parse_state_reads_transport_snapshot() {
        let r = parse_state(
            r#"{"ok":true,"playing":true,"active":true,"position_ms":42000,"song_id":"s7"}"#,
        );
        assert!(r.playing);
        assert!(r.active);
        assert_eq!(r.song_id, "s7");
        assert_eq!(r.position_ms, 42_000);
        assert!(r.has_track());
        // MUSIC-RFX-2/4 — seekable defaults off, reads through when present.
        assert!(!r.seekable);
        assert!(parse_state(r#"{"ok":true,"song_id":"s","seekable":true}"#).seekable);
        // ok:false / malformed → idle default.
        assert_eq!(parse_state(r#"{"ok":false}"#), NowState::default());
        assert_eq!(parse_state("not json"), NowState::default());
        assert!(!NowState::default().has_track());
    }

    #[test]
    fn parse_song_meta_reads_title_artist() {
        let r = parse_song_meta(
            r#"{"ok":true,"result":{"song":{"id":"s7","title":"So What","artist":"Miles Davis"}}}"#,
        );
        assert_eq!(r, Some(("So What".to_string(), "Miles Davis".to_string())));
        // A missing artist → empty string, not failure.
        let r2 = parse_song_meta(r#"{"ok":true,"result":{"song":{"id":"x","title":"T"}}}"#);
        assert_eq!(r2, Some(("T".to_string(), String::new())));
        // Failures → None.
        assert_eq!(parse_song_meta(r#"{"ok":false}"#), None);
        assert_eq!(parse_song_meta("nope"), None);
    }
}
