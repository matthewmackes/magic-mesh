//! AIR-2 (v6.1) — Bus-native control surface for the music daemon.
//!
//! Per the Q96 Bus-canonical lock (EPIC-RETIRE-DBUS), the daemon's
//! MDE-internal control is **Bus**, not a new `dev.mackes.MDE.Music`
//! D-Bus interface. The GUI (and `mde-bus publish`) send requests on
//! `action/music/<verb>`; the responder applies them to the shared
//! [`Queue`] and writes the result to `reply/<request-ulid>`. (MPRIS
//! `org.mpris.MediaPlayer2` — FDO-standard — stays D-Bus for media-key /
//! lock-screen interop; that + the play flow are AIR-2.c, gated on the
//! AIR-5 audio engine.)
//!
//! The verb dispatch ([`dispatch_queue_action`]) is a pure function over
//! the [`Queue`], fully unit-testable; [`serve`] is the thin poll loop
//! (the standard mackesd Bus-responder shape) that drives it off the
//! Bus persistence store.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

use crate::airsonic::Client;
use crate::creds;
use crate::engine::{Engine, SourceCodec};
use crate::queue::{self, Queue};
use crate::state::{self, MusicState};

/// Poll cadence for the action topics.
pub const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// The queue-control verbs served on `action/music/<verb>` (synchronous
/// — they only touch the local queue file).
pub const ACTION_VERBS: [&str; 10] = [
    "enqueue",
    "enqueue-after",
    "clear",
    "next",
    "prev",
    "get-queue",
    // MUSIC-RFX-1 — queue management.
    "queue-move",
    "queue-remove",
    "queue-remove-many",
    "queue-move-to-next",
];

/// The library-browse verbs served on `action/music/<verb>`
/// (asynchronous — each proxies an Airsonic REST call).
pub const BROWSE_VERBS: [&str; 18] = [
    "list-albums",
    "list-artists",
    "search",
    "get-album",
    "list-genres",
    "albums-by-genre",
    "get-song",
    "get-cover-art",
    "list-podcasts",
    "list-radio",
    "podcast-episodes",
    "list-recents",
    "list-playlists",
    "get-playlist",
    "get-lyrics",
    // MUSIC-RFX-3 — playlist write verbs (proxy the Subsonic create/update/delete
    // endpoints; a re-query of list-playlists reflects the change).
    "playlist-create",
    "playlist-update",
    "playlist-delete",
];

/// The transport verbs served on `action/music/<verb>` (AIR-2.d — drive
/// the AIR-5 playback engine).
pub const TRANSPORT_VERBS: [&str; 6] =
    ["play", "pause", "resume", "stop", "set-volume", "get-state"];

/// AIR-15.b.5 — peer-roster + take-over verbs. They read/write the AIR-8
/// state files (`music-state-by-peer/`, handoff intents) and need neither
/// the engine nor the airsonic client.
pub const PEER_VERBS: [&str; 2] = ["peer-states", "take-over"];

/// Authoritative-state write cadence while playing (AIR-8's 5 s heartbeat,
/// so a stale owner frees the mesh after `STATE_STALE_MS`).
pub const STATE_WRITE_INTERVAL: Duration = Duration::from_secs(5);

/// Result of dispatching one action: the JSON reply + whether the queue
/// changed (and so must be persisted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dispatch {
    /// JSON written to `reply/<request-ulid>`.
    pub reply_json: String,
    /// Whether the queue changed and must be persisted.
    pub mutated: bool,
}

/// Extract a song-id from a request body: either a bare string or
/// `{"song_id": "..."}`.
#[must_use]
fn song_id_from(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(s) = v.get("song_id").and_then(serde_json::Value::as_str) {
            return Some(s.to_string());
        }
        if let Some(s) = v.as_str() {
            return Some(s.to_string());
        }
    }
    // Fall back to the raw body as the id.
    Some(trimmed.trim_matches('"').to_string())
}

fn queue_reply(q: &Queue, mutated: bool) -> Dispatch {
    Dispatch {
        reply_json: json!({
            "ok": true,
            "len": q.len(),
            "current": q.current(),
            "songs": q.songs,
        })
        .to_string(),
        mutated,
    }
}

fn error_reply(message: &str) -> Dispatch {
    Dispatch {
        reply_json: json!({ "ok": false, "error": message }).to_string(),
        mutated: false,
    }
}

/// Dispatch a peer verb against the AIR-8 state `dir`. `peer-states`
/// returns every peer's last snapshot (the Peers-tab roster);
/// `take-over` posts a handoff intent asking `<body>` (a host, or empty
/// to claim an idle mesh) to yield, via AIR-8 `post_takeover`.
#[must_use]
pub fn dispatch_peer(verb: &str, body: &str, dir: &Path) -> String {
    match verb {
        "peer-states" => {
            json!({ "ok": true, "result": { "peers": state::read_all_peer_states(dir) } })
                .to_string()
        }
        "take-over" => {
            let to = body.trim().trim_matches('"').to_string();
            let to_peer = if to.is_empty() { None } else { Some(to) };
            match state::post_takeover(dir, &state::local_host(), to_peer, state::now_ms()) {
                Ok(intent) => json!({ "ok": true, "intent_id": intent.intent_id }).to_string(),
                Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
            }
        }
        other => json!({ "ok": false, "error": format!("unknown peer verb: {other}") }).to_string(),
    }
}

/// Apply one `action/music/<verb>` request to `q`, returning the reply.
#[must_use]
pub fn dispatch_queue_action(verb: &str, body: &str, q: &mut Queue) -> Dispatch {
    match verb {
        "enqueue" => match song_id_from(body) {
            Some(id) => {
                q.enqueue(id);
                queue_reply(q, true)
            }
            None => error_reply("enqueue: missing song_id"),
        },
        "enqueue-after" => match song_id_from(body) {
            Some(id) => {
                q.enqueue_after_current(id);
                queue_reply(q, true)
            }
            None => error_reply("enqueue-after: missing song_id"),
        },
        "clear" => {
            q.clear();
            queue_reply(q, true)
        }
        "next" => {
            q.next();
            queue_reply(q, true)
        }
        "prev" => {
            q.prev();
            queue_reply(q, true)
        }
        "get-queue" => queue_reply(q, false),
        // MUSIC-RFX-1 — queue management. Indices come from the JSON body.
        "queue-move" => {
            let v: serde_json::Value = serde_json::from_str(body).unwrap_or(json!({}));
            match (
                v.get("from").and_then(serde_json::Value::as_u64),
                v.get("to").and_then(serde_json::Value::as_u64),
            ) {
                (Some(f), Some(t)) => {
                    let ok = q.move_track(f as usize, t as usize);
                    queue_reply(q, ok)
                }
                _ => error_reply("queue-move: need {from,to}"),
            }
        }
        "queue-remove" => match index_from(body) {
            Some(i) => {
                let ok = q.remove(i);
                queue_reply(q, ok)
            }
            None => error_reply("queue-remove: need {index}"),
        },
        "queue-remove-many" => {
            let v: serde_json::Value = serde_json::from_str(body).unwrap_or(json!({}));
            let idxs: Vec<usize> = v
                .get("indices")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_u64().map(|n| n as usize))
                        .collect()
                })
                .unwrap_or_default();
            let removed = q.remove_many(&idxs);
            queue_reply(q, removed > 0)
        }
        "queue-move-to-next" => match index_from(body) {
            Some(i) => {
                let ok = q.move_to_next(i);
                queue_reply(q, ok)
            }
            None => error_reply("queue-move-to-next: need {index}"),
        },
        other => error_reply(&format!("unknown verb: {other}")),
    }
}

/// Extract a `Vec<String>` from a JSON object field that's an array of strings
/// (e.g. `song_ids`, `add`, `remove_indices`). Numbers are stringified so a
/// caller can send `remove_indices: [0,2]` as numbers or strings. Missing /
/// non-array → empty.
fn str_array(v: &serde_json::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    x.as_str()
                        .map(str::to_string)
                        .or_else(|| x.as_u64().map(|n| n.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a queue index from a request body: `{"index":N}` or a bare number.
fn index_from(body: &str) -> Option<usize> {
    let v: serde_json::Value = serde_json::from_str(body.trim()).ok()?;
    v.get("index")
        .and_then(serde_json::Value::as_u64)
        .or_else(|| v.as_u64())
        .map(|n| n as usize)
}

/// Reply JSON for a library-browse verb. Proxies the Airsonic REST call
/// via the shared [`Client`]; missing creds / server errors become an
/// `{ok:false,error}` reply rather than a panic. I/O, so not pure — the
/// URL-building + parse logic it leans on is unit-tested in [`crate::airsonic`].
fn dispatch_browse(
    verb: &str,
    body: &str,
    client: &Client,
    rt: &tokio::runtime::Runtime,
) -> String {
    let result: Result<serde_json::Value, String> = rt.block_on(async {
        match verb {
            "list-albums" => client
                .get_album_list2("newest", 100)
                .await
                .map(|a| json!({ "albums": a }))
                .map_err(|e| e.to_string()),
            "list-artists" => client
                .get_artists()
                .await
                .map(|a| json!({ "artists": a }))
                .map_err(|e| e.to_string()),
            "search" => {
                let query = song_id_from(body).unwrap_or_default();
                client
                    .search3(&query)
                    .await
                    .map(|r| json!({ "artists": r.artists, "albums": r.albums, "songs": r.songs }))
                    .map_err(|e| e.to_string())
            }
            "get-album" => {
                let id = song_id_from(body).unwrap_or_default();
                client
                    .get_album(&id)
                    .await
                    .map(|a| json!({ "album": a.album, "songs": a.songs }))
                    .map_err(|e| e.to_string())
            }
            "list-genres" => client
                .get_genres()
                .await
                .map(|g| json!({ "genres": g }))
                .map_err(|e| e.to_string()),
            "get-song" => {
                let id = song_id_from(body).unwrap_or_default();
                client
                    .get_song(&id)
                    .await
                    .map(|s| json!({ "song": s }))
                    .map_err(|e| e.to_string())
            }
            "albums-by-genre" => {
                let genre = song_id_from(body).unwrap_or_default();
                client
                    .get_albums_by_genre(&genre, 200)
                    .await
                    .map(|a| json!({ "albums": a }))
                    .map_err(|e| e.to_string())
            }
            "get-cover-art" => {
                use base64::Engine;
                let id = song_id_from(body).unwrap_or_default();
                // MUSIC-ART-SYNC — serve from the communal mesh cache first (art
                // pulled by any node, reused mesh-wide + offline); on a miss,
                // fetch from Airsonic and write it through for every other node.
                if let Some(bytes) = crate::cache::read_shared_artwork(&id) {
                    Ok(json!({
                        "art": base64::engine::general_purpose::STANDARD.encode(&bytes)
                    }))
                } else {
                    client
                        .get_cover_art_bytes(&id)
                        .await
                        .map(|bytes| {
                            crate::cache::write_shared_artwork(&id, &bytes);
                            json!({
                                "art": base64::engine::general_purpose::STANDARD.encode(&bytes)
                            })
                        })
                        .map_err(|e| e.to_string())
                }
            }
            "list-podcasts" => client
                .get_podcast_channels()
                .await
                .map(|c| json!({ "podcasts": c }))
                .map_err(|e| e.to_string()),
            // SVC-3 — the Radio hub card: the server's saved stations.
            "list-radio" => client
                .get_internet_radio_stations()
                .await
                .map(|r| json!({ "radio": r }))
                .map_err(|e| e.to_string()),
            "podcast-episodes" => {
                let id = song_id_from(body).unwrap_or_default();
                client
                    .get_podcast_episodes(&id)
                    .await
                    .map(|e| json!({ "episodes": e }))
                    .map_err(|e| e.to_string())
            }
            // AIR-4.b — Recents hub card: recently-added albums (reuses
            // getAlbumList2 with type=recent).
            "list-recents" => client
                .get_album_list2("recent", 100)
                .await
                .map(|a| json!({ "albums": a }))
                .map_err(|e| e.to_string()),
            // AIR-4.b — Playlists hub card: the playlist roster, then a
            // single playlist's songs (the GUI enqueues these to play it).
            "list-playlists" => client
                .get_playlists()
                .await
                .map(|p| json!({ "playlists": p }))
                .map_err(|e| e.to_string()),
            "get-playlist" => {
                let id = song_id_from(body).unwrap_or_default();
                client
                    .get_playlist(&id)
                    .await
                    .map(|s| json!({ "songs": s }))
                    .map_err(|e| e.to_string())
            }
            "get-lyrics" => {
                let id = song_id_from(body).unwrap_or_default();
                client
                    .get_lyrics_by_song_id(&id)
                    .await
                    .map(|lines| json!({ "lyrics": lines }))
                    .map_err(|e| e.to_string())
            }
            // MUSIC-RFX-3 — playlist write verbs. Body is a JSON object:
            //   playlist-create {"name":..,"song_ids":[..]?}
            //   playlist-update {"id":..,"name":..?,"add":[..]?,"remove_indices":[..]?}
            //   playlist-delete {"id":..} | "<id>"
            "playlist-create" => {
                let v: serde_json::Value = serde_json::from_str(body.trim()).unwrap_or(json!({}));
                let name = v.get("name").and_then(serde_json::Value::as_str);
                match name {
                    Some(name) if !name.is_empty() => {
                        let song_ids = str_array(&v, "song_ids");
                        client
                            .create_playlist(name, &song_ids)
                            .await
                            .map(|()| json!({ "created": name }))
                            .map_err(|e| e.to_string())
                    }
                    _ => Err("playlist-create: need {name}".to_string()),
                }
            }
            "playlist-update" => {
                let v: serde_json::Value = serde_json::from_str(body.trim()).unwrap_or(json!({}));
                match v.get("id").and_then(serde_json::Value::as_str) {
                    Some(id) if !id.is_empty() => {
                        let name = v
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .filter(|s| !s.is_empty());
                        let add = str_array(&v, "add");
                        let remove = str_array(&v, "remove_indices");
                        client
                            .update_playlist(id, name, &add, &remove)
                            .await
                            .map(|()| json!({ "updated": id }))
                            .map_err(|e| e.to_string())
                    }
                    _ => Err("playlist-update: need {id}".to_string()),
                }
            }
            "playlist-delete" => {
                let id = song_id_from(body).unwrap_or_default();
                if id.is_empty() {
                    Err("playlist-delete: need {id}".to_string())
                } else {
                    client
                        .delete_playlist(&id)
                        .await
                        .map(|()| json!({ "deleted": id }))
                        .map_err(|e| e.to_string())
                }
            }
            other => Err(format!("unknown browse verb: {other}")),
        }
    });
    match result {
        Ok(v) => json!({ "ok": true, "result": v }).to_string(),
        Err(e) => json!({ "ok": false, "error": e }).to_string(),
    }
}

/// One browse-poll sweep: for each browse verb, dispatch new requests
/// against a freshly-built Airsonic client. A missing-creds state replies
/// with an error (the GUI prompts the operator to connect).
pub fn poll_browse(
    persist: &Persist,
    rt: &tokio::runtime::Runtime,
    cursors: &mut HashMap<String, String>,
) {
    let client = creds::load()
        .ok()
        .map(|c| Client::new(&c.server_url, &c.username, &c.password));
    for verb in BROWSE_VERBS {
        let topic = format!("action/music/{verb}");
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(_) => continue,
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let reply = match &client {
                Some(c) => dispatch_browse(verb, msg.body.as_deref().unwrap_or(""), c, rt),
                None => {
                    json!({ "ok": false, "error": "no Airsonic server configured" }).to_string()
                }
            };
            let _ = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply),
            );
        }
    }
}

// ───────────────────────── transport (AIR-2.d) ─────────────────────────

/// A parsed transport request — the pure half of the play flow, decided
/// from the verb + body without touching the engine (so it's
/// unit-testable). [`apply_transport`] runs the side effects.
#[derive(Debug, Clone, PartialEq)]
pub enum TransportCommand {
    /// Play the queue from the current track, gaplessly.
    Play,
    /// Pause (the buffer is preserved; resume is seamless).
    Pause,
    /// Resume after a pause.
    Resume,
    /// Stop + clear the buffer.
    Stop,
    /// Set the volume multiplier (`0.0..=1.0`, clamped by the engine).
    SetVolume(f32),
    /// Report the current playback state (no side effect).
    GetState,
}

/// Parse an `action/music/<verb>` transport request into a command. The
/// `set-volume` body is a bare number or `{"volume": N}`. `None` for an
/// unknown verb.
#[must_use]
pub fn parse_transport(verb: &str, body: &str) -> Option<TransportCommand> {
    match verb {
        "play" => Some(TransportCommand::Play),
        "pause" => Some(TransportCommand::Pause),
        "resume" => Some(TransportCommand::Resume),
        "stop" => Some(TransportCommand::Stop),
        "get-state" => Some(TransportCommand::GetState),
        "set-volume" => parse_volume(body).map(TransportCommand::SetVolume),
        _ => None,
    }
}

/// Volume from a bare number (`"0.6"`) or `{"volume": 0.6}`.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)] // serde_json f64 → engine f32
fn parse_volume(body: &str) -> Option<f32> {
    let trimmed = body.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(n) = v.get("volume").and_then(serde_json::Value::as_f64) {
            return Some(n as f32);
        }
        if let Some(n) = v.as_f64() {
            return Some(n as f32);
        }
    }
    trimmed.parse::<f32>().ok()
}

/// Write this peer's authoritative [`MusicState`] (AIR-8) — the playing
/// peer heartbeats it so the mesh knows who owns playback.
fn write_playback_state(playing: bool, song_id: &str, position_ms: u64) {
    let st = MusicState {
        peer: state::local_host(),
        playing,
        song_id: song_id.to_string(),
        position_ms,
        updated_ms: state::now_ms(),
    };
    let _ = state::write_state(&state::data_dir(), &st);
}

/// Apply one transport request to the engine + queue, returning the reply
/// JSON. Side effects (engine + the AIR-8 state write); the pure
/// verb→command parse is [`parse_transport`].
fn apply_transport(
    verb: &str,
    body: &str,
    engine: Option<&Engine>,
    client: Option<&Client>,
    queue: &Queue,
) -> String {
    let Some(cmd) = parse_transport(verb, body) else {
        return json!({ "ok": false, "error": format!("unknown transport verb: {verb}") })
            .to_string();
    };
    let no_audio =
        || json!({ "ok": false, "error": "no audio output device on this peer" }).to_string();
    match cmd {
        // AUDIT-MESH-4: get-state is answered unconditionally so the Music
        // panel can render an honest idle / needs-audio / needs-Airsonic state
        // even on a headless peer (no audio device) or before Airsonic creds
        // are configured. `audio_available` / `needs_airsonic` let the panel
        // tell those apart instead of silently looking "idle". The mutating
        // verbs below still require a live engine.
        TransportCommand::GetState => json!({
            "ok": true,
            "playing": engine.map_or(false, |e| e.is_playing()),
            "active": engine.map_or(false, |e| e.is_active()),
            "position_ms": engine.map_or(0, |e| e.position_ms()),
            "volume": engine.map_or(1.0_f32, |e| e.volume()),
            "song_id": queue.current(),
            "audio_available": engine.is_some(),
            "needs_airsonic": client.is_none(),
        })
        .to_string(),
        TransportCommand::Play => {
            let Some(engine) = engine else {
                return no_audio();
            };
            let Some(client) = client else {
                return json!({ "ok": false, "error": "no Airsonic server configured" })
                    .to_string();
            };
            // Gapless album: hand the engine current..end in one list.
            let upcoming: Vec<(String, SourceCodec)> = queue
                .songs
                .iter()
                .skip(queue.current)
                .map(|id| (client.stream_url(id), SourceCodec::Unknown))
                .collect();
            if upcoming.is_empty() {
                return json!({ "ok": false, "error": "queue is empty" }).to_string();
            }
            engine.play(upcoming);
            let song = queue.current().unwrap_or("");
            write_playback_state(true, song, 0);
            json!({ "ok": true, "playing": true, "song_id": song }).to_string()
        }
        TransportCommand::Pause => {
            let Some(engine) = engine else {
                return no_audio();
            };
            engine.pause();
            write_playback_state(false, queue.current().unwrap_or(""), engine.position_ms());
            json!({ "ok": true, "playing": false }).to_string()
        }
        TransportCommand::Resume => {
            let Some(engine) = engine else {
                return no_audio();
            };
            engine.resume();
            write_playback_state(true, queue.current().unwrap_or(""), engine.position_ms());
            json!({ "ok": true, "playing": true }).to_string()
        }
        TransportCommand::Stop => {
            let Some(engine) = engine else {
                return no_audio();
            };
            engine.stop();
            write_playback_state(false, "", 0);
            json!({ "ok": true, "playing": false }).to_string()
        }
        TransportCommand::SetVolume(v) => {
            let Some(engine) = engine else {
                return no_audio();
            };
            engine.set_volume(v);
            json!({ "ok": true, "volume": engine.volume() }).to_string()
        }
    }
}

/// One transport-poll sweep: dispatch new `action/music/{play,pause,…}`
/// requests to the engine. A fresh Airsonic client is loaded per sweep so
/// a mid-session connect is picked up.
pub fn poll_transport(
    persist: &Persist,
    queue_path: &Path,
    engine: Option<&Engine>,
    cursors: &mut HashMap<String, String>,
) {
    let client = creds::load()
        .ok()
        .map(|c| Client::new(&c.server_url, &c.username, &c.password));
    let queue = queue::read_from(queue_path);
    for verb in TRANSPORT_VERBS {
        let topic = format!("action/music/{verb}");
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(_) => continue,
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let reply = apply_transport(
                verb,
                msg.body.as_deref().unwrap_or(""),
                engine,
                client.as_ref(),
                &queue,
            );
            let _ = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply),
            );
        }
    }
}

/// Heartbeat this peer's playback state every [`STATE_WRITE_INTERVAL`]
/// while playing (AIR-8).
fn write_periodic_state(engine: Option<&Engine>, queue_path: &Path) {
    let Some(engine) = engine else { return };
    if !engine.is_playing() {
        return;
    }
    let queue = queue::read_from(queue_path);
    write_playback_state(true, queue.current().unwrap_or(""), engine.position_ms());
}

/// Run the Bus responder loop.
///
/// Polls the queue-control, library-browse, and transport
/// `action/music/<verb>` topics, dispatches them (queue + browse + the
/// AIR-5 engine), and replies on `reply/<ulid>`. Heartbeats the AIR-8
/// playback state while playing. Loops until `should_stop()` returns true.
///
/// # Panics
/// If the internal tokio runtime (for the async browse proxy) can't be
/// built — an environment fault, not a runtime condition.
pub fn serve<F: Fn() -> bool>(bus_root: PathBuf, queue_path: &Path, should_stop: F) {
    let mut persist = match Persist::open(bus_root.clone()) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "opening Bus store failed");
            return;
        }
    };
    // MUSIC-WEDGE-2 — track the index.sqlite inode so we can detect a swap.
    // Another process hitting a read-only DB triggers the BOOT-REC-3 self-heal
    // recreate (unlink + new file); that strands every OTHER process on the now-
    // DELETED inode, so the daemon keeps reading/writing a dead file and stops
    // seeing new requests — the "daemon not responding" wedge after long uptime
    // (live: the daemon's fd pointed at `index.sqlite (deleted)`). We follow the
    // live DB by reopening when the inode changes.
    let mut seen_inode = index_inode(&bus_root);
    // MUSIC-WEDGE — seed every poll cursor at the topic's CURRENT tail so a
    // restart skips the historical backlog. Without this, the first sweep's
    // `list_since(None)` returns every request ever made on each action topic
    // and the single-threaded loop reprocesses the whole backlog (each browse
    // verb = an Airsonic round-trip) before answering anything new — observed
    // live as the daemon "not responding" after a restart, and a stale `play`
    // could even replay. New (post-start) requests have a larger ULID and are
    // still picked up normally.
    let mut cursors: HashMap<String, String> = seed_cursors_at_tail(&persist);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime for browse proxy");
    // The engine grabs the default output device; on a headless peer (no
    // audio) it's absent and transport verbs reply with an error while
    // queue + browse keep working.
    let engine = Engine::new()
        .map_err(|e| {
            tracing::warn!(error = %e, "no audio output — playback disabled; queue + browse still served");
        })
        .ok();
    // AIR-6: bring up the MPRIS surface sharing this engine, so media keys
    // (sway → playerctl → MPRIS) + the lock-screen widget drive the same
    // playback the Bus does. Held for the serve loop's lifetime; dropping
    // it (when serve returns) stops the surface thread. A headless peer
    // with no audio engine — or no session bus — simply skips it.
    let _mpris = engine
        .as_ref()
        .map(|e| crate::mpris::spawn(e.handle(), queue_path.to_path_buf(), state::data_dir()));
    let mut last_state_write = Instant::now();
    while !should_stop() {
        // MUSIC-WEDGE-2 — if the index inode swapped under us (another process
        // recreated it), reopen so we follow the live DB instead of a deleted
        // one. Cheap stat per sweep; reopen only on an actual change. Cursors
        // carry over — new requests have larger ULIDs and are still picked up.
        let now_inode = index_inode(&bus_root);
        if now_inode.is_some() && now_inode != seen_inode {
            if let Ok(p) = Persist::open(bus_root.clone()) {
                tracing::warn!(
                    "bus index inode changed under us — reopened the store (MUSIC-WEDGE-2)"
                );
                persist = p;
                seen_inode = now_inode;
            }
        }
        // AUDIT-MESH-14 — run the FAST, local-only responders (queue control,
        // transport/get-state, peer roster) BEFORE the network-bound browse
        // proxy. `poll_browse` does blocking Airsonic REST calls; if it ran
        // first, a slow/unreachable server would starve every transport reply
        // in this single-threaded loop (observed live: get-state timed out at
        // 9s, just under poll_browse's ~10s HTTP timeout). With transport first,
        // get-state is answered within POLL_INTERVAL of the request regardless
        // of browse latency. (The Airsonic client also has connect/total
        // timeouts now so browse itself can't hang forever.)
        poll_once(&persist, queue_path, &mut cursors);
        poll_transport(&persist, queue_path, engine.as_ref(), &mut cursors);
        poll_peers(&persist, &mut cursors);
        poll_browse(&persist, &rt, &mut cursors);
        if last_state_write.elapsed() >= STATE_WRITE_INTERVAL {
            write_periodic_state(engine.as_ref(), queue_path);
            last_state_write = Instant::now();
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// The inode of the bus `index.sqlite`, or `None` if it can't be stat'd. Used to
/// detect a BOOT-REC-3 recreate (unlink + new file = new inode) so the serve
/// loop can reopen instead of being stranded on the deleted file (MUSIC-WEDGE-2).
fn index_inode(bus_root: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(bus_root.join("index.sqlite"))
        .ok()
        .map(|m| m.ino())
}

/// MUSIC-WEDGE — build the initial cursor map seeded at each polled topic's
/// current tail (its newest ULID), so the serve loop only handles requests that
/// arrive AFTER startup and never reprocesses the historical backlog. Topics with
/// no messages get no entry (cursor `None` → first real message is picked up).
#[must_use]
pub fn seed_cursors_at_tail(persist: &Persist) -> HashMap<String, String> {
    let mut cursors = HashMap::new();
    let verbs = ACTION_VERBS
        .iter()
        .chain(BROWSE_VERBS.iter())
        .chain(TRANSPORT_VERBS.iter())
        .chain(PEER_VERBS.iter());
    for verb in verbs {
        let topic = format!("action/music/{verb}");
        if let Ok(Some(latest)) = persist.latest_ulid(&topic) {
            cursors.insert(topic, latest);
        }
    }
    cursors
}

/// One poll sweep over the AIR-15.b.5 peer verbs (`peer-states`,
/// `take-over`) — reads/writes the AIR-8 state dir, replies on reply/<ulid>.
pub fn poll_peers(persist: &Persist, cursors: &mut HashMap<String, String>) {
    let dir = state::data_dir();
    for verb in PEER_VERBS {
        let topic = format!("action/music/{verb}");
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(_) => continue,
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let reply = dispatch_peer(verb, msg.body.as_deref().unwrap_or(""), &dir);
            let _ = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply),
            );
        }
    }
}

/// One poll sweep across the queue-control action verbs (extracted so tests
/// can drive it deterministically without the sleep loop).
pub fn poll_once(persist: &Persist, queue_path: &Path, cursors: &mut HashMap<String, String>) {
    let mut q = queue::read_from(queue_path);
    for verb in ACTION_VERBS {
        let topic = format!("action/music/{verb}");
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(_) => continue,
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let d = dispatch_queue_action(verb, msg.body.as_deref().unwrap_or(""), &mut q);
            let _ = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&d.reply_json),
            );
            if d.mutated {
                let _ = queue::write_to(queue_path, &q);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_peer_roster_and_take_over() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(dispatch_peer("peer-states", "", dir.path()).contains("\"peers\":[]"));
        let t = dispatch_peer("take-over", "anvil", dir.path());
        assert!(t.contains("\"ok\":true") && t.contains("intent_id"));
        assert_eq!(state::read_intents(dir.path()).len(), 1);
        assert!(dispatch_peer("bogus", "", dir.path()).contains("\"ok\":false"));
    }

    #[test]
    fn str_array_reads_strings_and_numbers() {
        // MUSIC-RFX-3 — playlist write bodies carry string id arrays and numeric
        // index arrays; both flatten to Vec<String>.
        let v: serde_json::Value =
            serde_json::from_str(r#"{"song_ids":["s1","s2"],"remove_indices":[0,2]}"#).unwrap();
        assert_eq!(str_array(&v, "song_ids"), vec!["s1", "s2"]);
        assert_eq!(str_array(&v, "remove_indices"), vec!["0", "2"]);
        // Missing / non-array → empty.
        assert!(str_array(&v, "absent").is_empty());
        assert!(str_array(&json!({"x": "scalar"}), "x").is_empty());
    }

    #[test]
    fn playlist_write_verbs_are_browse_verbs() {
        // MUSIC-RFX-3 — the three write verbs are served on the browse poll.
        for verb in ["playlist-create", "playlist-update", "playlist-delete"] {
            assert!(
                BROWSE_VERBS.contains(&verb),
                "{verb} missing from BROWSE_VERBS"
            );
        }
    }

    #[test]
    fn song_id_parsing_forms() {
        assert_eq!(song_id_from(r#"{"song_id":"s1"}"#).as_deref(), Some("s1"));
        assert_eq!(song_id_from(r#""s2""#).as_deref(), Some("s2"));
        assert_eq!(song_id_from("s3").as_deref(), Some("s3"));
        assert_eq!(song_id_from("  "), None);
    }

    #[test]
    fn dispatch_enqueue_and_get() {
        let mut q = Queue::default();
        let d = dispatch_queue_action("enqueue", r#"{"song_id":"a"}"#, &mut q);
        assert!(d.mutated);
        assert!(d.reply_json.contains("\"ok\":true"));
        assert!(d.reply_json.contains("\"len\":1"));
        // get-queue doesn't mutate.
        let g = dispatch_queue_action("get-queue", "", &mut q);
        assert!(!g.mutated);
        assert!(g.reply_json.contains("\"current\":\"a\""));
    }

    #[test]
    fn dispatch_enqueue_after_and_walk() {
        let mut q = Queue::default();
        let _ = dispatch_queue_action("enqueue", "a", &mut q);
        let _ = dispatch_queue_action("enqueue", "b", &mut q);
        let _ = dispatch_queue_action("enqueue-after", "x", &mut q);
        assert_eq!(q.songs, vec!["a", "x", "b"]);
        let d = dispatch_queue_action("next", "", &mut q);
        assert!(d.mutated);
        assert_eq!(q.current(), Some("x"));
    }

    #[test]
    fn seed_cursors_at_tail_skips_backlog() {
        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().join("bus")).unwrap();
        let queue_path = dir.path().join("queue.json");
        // A stale enqueue sits in the backlog from "before the restart".
        persist
            .write(
                "action/music/enqueue",
                Priority::Default,
                None,
                Some(r#"{"song_id":"stale"}"#),
            )
            .unwrap();
        // Seed cursors at the tail (simulating daemon startup), then poll.
        let mut cursors = seed_cursors_at_tail(&persist);
        poll_once(&persist, &queue_path, &mut cursors);
        // The stale request is NOT replayed — the queue stays empty.
        assert!(queue::read_from(&queue_path).songs.is_empty());
        // A NEW request after seeding IS handled.
        let fresh = persist
            .write(
                "action/music/enqueue",
                Priority::Default,
                None,
                Some(r#"{"song_id":"fresh"}"#),
            )
            .unwrap();
        poll_once(&persist, &queue_path, &mut cursors);
        assert_eq!(queue::read_from(&queue_path).songs, vec!["fresh"]);
        assert!(persist
            .list_since(&reply_topic(&fresh.ulid), None)
            .unwrap()
            .iter()
            .any(|m| m.body.as_deref().unwrap_or("").contains("\"ok\":true")));
    }

    #[test]
    fn poll_once_round_trips_a_request() {
        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().join("bus")).unwrap();
        let queue_path = dir.path().join("queue.json");
        // A GUI publishes an enqueue request on the action topic.
        let req = persist
            .write(
                "action/music/enqueue",
                Priority::Default,
                None,
                Some(r#"{"song_id":"t1"}"#),
            )
            .unwrap();
        let mut cursors = HashMap::new();
        poll_once(&persist, &queue_path, &mut cursors);
        // A reply landed on reply/<ulid> with ok:true.
        let replies = persist.list_since(&reply_topic(&req.ulid), None).unwrap();
        assert_eq!(replies.len(), 1);
        assert!(replies[0].body.as_deref().unwrap().contains("\"ok\":true"));
        // The queue was persisted with the enqueued track.
        assert_eq!(queue::read_from(&queue_path).songs, vec!["t1"]);
        // A second poll with the advanced cursor does nothing new.
        poll_once(&persist, &queue_path, &mut cursors);
        assert_eq!(
            persist
                .list_since(&reply_topic(&req.ulid), None)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn dispatch_clear_and_errors() {
        let mut q = Queue::default();
        let _ = dispatch_queue_action("enqueue", "a", &mut q);
        let c = dispatch_queue_action("clear", "", &mut q);
        assert!(c.mutated);
        assert!(q.is_empty());
        // Missing id.
        let e = dispatch_queue_action("enqueue", "", &mut q);
        assert!(e.reply_json.contains("\"ok\":false"));
        // Unknown verb.
        let u = dispatch_queue_action("frobnicate", "", &mut q);
        assert!(u.reply_json.contains("unknown verb"));
    }

    #[test]
    fn parse_transport_verbs() {
        assert_eq!(parse_transport("play", ""), Some(TransportCommand::Play));
        assert_eq!(parse_transport("pause", ""), Some(TransportCommand::Pause));
        assert_eq!(
            parse_transport("resume", ""),
            Some(TransportCommand::Resume)
        );
        assert_eq!(parse_transport("stop", ""), Some(TransportCommand::Stop));
        assert_eq!(
            parse_transport("get-state", ""),
            Some(TransportCommand::GetState)
        );
        assert_eq!(parse_transport("teleport", ""), None);
    }

    #[test]
    fn parse_transport_set_volume_forms() {
        // bare number, JSON object, and an out-of-range value (engine clamps).
        assert_eq!(
            parse_transport("set-volume", "0.6"),
            Some(TransportCommand::SetVolume(0.6))
        );
        assert_eq!(
            parse_transport("set-volume", r#"{"volume":0.25}"#),
            Some(TransportCommand::SetVolume(0.25))
        );
        assert_eq!(
            parse_transport("set-volume", "2"),
            Some(TransportCommand::SetVolume(2.0))
        );
        // Non-numeric body → no command.
        assert_eq!(parse_transport("set-volume", "loud"), None);
    }

    #[test]
    fn get_state_is_answered_without_engine_or_creds() {
        // AUDIT-MESH-4: a headless peer with no audio device + no Airsonic creds
        // must still answer get-state with ok:true and honest capability flags
        // (so the panel shows "configure Airsonic" / "no audio device" rather
        // than a silent blank). Mutating verbs still return the no-audio error.
        let queue = queue::Queue::default();
        let reply = apply_transport("get-state", "", None, None, &queue);
        let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["active"], false);
        assert_eq!(v["playing"], false);
        assert_eq!(v["audio_available"], false);
        assert_eq!(v["needs_airsonic"], true);

        // A mutating verb without an engine is still refused.
        let play = apply_transport("play", "", None, None, &queue);
        let pv: serde_json::Value = serde_json::from_str(&play).unwrap();
        assert_eq!(pv["ok"], false);
    }
}
