//! AIR-6 (v6.1) — MPRIS `org.mpris.MediaPlayer2` surface for the music
//! daemon.
//!
//! `org.mpris.MediaPlayer2` is an FDO-standard remote-control interface —
//! the allowed D-Bus interop carve-out per EPIC-RETIRE-DBUS (MDE-internal
//! control stays on the Bus; the FDO + `org.mpris` standards keep D-Bus).
//! It is what `playerctl`, the `XF86Audio*` media keys (sway → playerctl →
//! MPRIS), and the lock-screen now-playing widget (AIR-19) read.
//!
//! Two interfaces share the `/org/mpris/MediaPlayer2` object: the root
//! [`MediaPlayer2`] (Identity / Quit / Raise) + the [`Player`] transport
//! surface. Both drive the shared [`EngineHandle`](crate::engine) (AIR-5)
//! and the on-disk [`queue`](crate::queue) — the same surfaces the Bus
//! responder (AIR-2) drives, so MPRIS + Bus + GUI stay one source of truth.
//!
//! The engine lives on the daemon's audio thread; this surface runs on its
//! own thread with its own tokio runtime + zbus connection, sharing only
//! the `Send + Sync` [`EngineHandle`]. [`spawn`] starts it and returns an
//! [`MprisHandle`] that stops it on drop; it degrades to a no-op when no
//! session bus is reachable (a headless peer keeps Bus + queue working
//! without MPRIS).
//!
//! `Shuffle` + `LoopStatus` persist to a small sidecar and are honored on
//! the explicit `Next`/`Previous`/`Play` paths. Auto-advance *at a track's
//! natural end* (which is what would make repeat fire on its own) is the
//! AIR-2.c queue-driver's job and is not yet built for **any** mode — the
//! daemon has no track-end callback today; that is a pre-existing gap, not
//! one this surface introduces. `CanSeek` is reported `false` because the
//! AIR-5 engine has no seek yet (the AIR-15 scrub bar adds it).

// MPRIS plumbing trips a few pedantic/nursery lints that are noise here:
// the f32↔f64 volume + u64→i64 position conversions are intentional and
// bounded, the zbus interface getters are async by zbus contract even when
// their body is trivial, and the module name repeats the parent crate's
// domain. The real fallibility (poisoned locks, absent session bus) is
// handled in code below, not suppressed.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::unused_async,
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::missing_const_for_fn,
    clippy::implicit_hasher,
    // The MPRIS contract forces `&self` on the no-op methods (Seek /
    // SetPosition / OpenUri) + on `client()`, and forces ignored params
    // that zbus still deserializes off the wire — so the `_`-prefix is
    // correct for our body even though the generated dispatch reads them.
    clippy::unused_self,
    clippy::used_underscore_binding
)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use zbus::interface;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use crate::airsonic::Client;
use crate::creds;
use crate::engine::{EngineHandle, SourceCodec};
use crate::queue;
use crate::state::{self, MusicState};

/// The MPRIS well-known bus name (`org.mpris.MediaPlayer2.<id>` convention).
pub const BUS_NAME: &str = "org.mpris.MediaPlayer2.mde-music";
/// The object path both MPRIS interfaces are served at.
pub const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";
/// The MPRIS "no track" sentinel trackid.
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";

// ───────────────────────── pure helpers ─────────────────────────

/// MPRIS `PlaybackStatus` string from the engine's active/playing flags.
#[must_use]
fn playback_status(active: bool, playing: bool) -> &'static str {
    match (active, playing) {
        (true, true) => "Playing",
        (true, false) => "Paused",
        (false, _) => "Stopped",
    }
}

/// Build the MPRIS `mpris:trackid` object path for an Airsonic song id.
/// MPRIS requires a valid D-Bus object path, so non-`[A-Za-z0-9_]` bytes
/// are escaped `_HH`. An empty id yields the standard no-track sentinel.
#[must_use]
fn track_path(song_id: &str) -> String {
    if song_id.is_empty() {
        return NO_TRACK.to_string();
    }
    let mut p = String::from("/org/mackes/mde/music/track/");
    for b in song_id.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' => p.push(b as char),
            _ => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                p.push('_');
                p.push(HEX[(b >> 4) as usize] as char);
                p.push(HEX[(b & 0x0f) as usize] as char);
            }
        }
    }
    p
}

/// MPRIS `LoopStatus`: off / repeat-one / repeat-all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
enum LoopStatus {
    /// No repeat.
    #[default]
    None,
    /// Repeat the current track.
    Track,
    /// Repeat the whole queue.
    Playlist,
}

impl LoopStatus {
    /// The MPRIS wire string for this status.
    #[must_use]
    fn as_mpris(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Track => "Track",
            Self::Playlist => "Playlist",
        }
    }

    /// Parse an MPRIS `LoopStatus` string (`None` for an unknown value).
    #[must_use]
    fn from_mpris(s: &str) -> Option<Self> {
        match s {
            "None" => Some(Self::None),
            "Track" => Some(Self::Track),
            "Playlist" => Some(Self::Playlist),
            _ => None,
        }
    }
}

/// Persisted MPRIS playback policy (shuffle + repeat). Honored by the
/// explicit `Play`/`Next`/`Previous` paths (see the module note on the
/// not-yet-built auto-advance).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct PlaybackMode {
    #[serde(default)]
    shuffle: bool,
    #[serde(default)]
    loop_status: LoopStatus,
}

/// Sidecar path for the playback mode within `dir`.
#[must_use]
fn mode_path(dir: &Path) -> PathBuf {
    dir.join("music-playback-mode.json")
}

/// Read the playback mode (defaults when absent/malformed — it is a
/// rebuildable preference, never a hard error).
#[must_use]
fn read_mode(dir: &Path) -> PlaybackMode {
    std::fs::read_to_string(mode_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Write the playback mode (best-effort; creates the parent dir).
fn write_mode(dir: &Path, mode: &PlaybackMode) {
    if let Ok(json) = serde_json::to_string_pretty(mode) {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(mode_path(dir), json);
    }
}

/// A 64-bit seed mixing the per-call ULID randomness with the wall clock —
/// enough entropy to shuffle a playback queue without pulling in a
/// `rand` dependency.
#[must_use]
fn rng_seed() -> u64 {
    let bytes = ulid::Ulid::new().to_bytes();
    let mut x = 0u64;
    for (i, &b) in bytes.iter().take(8).enumerate() {
        x |= u64::from(b) << (i * 8);
    }
    (x ^ state::now_ms()).max(1)
}

/// Advance an xorshift64 state in place + return it.
#[must_use]
fn xorshift(mut r: u64) -> u64 {
    r ^= r << 13;
    r ^= r >> 7;
    r ^= r << 17;
    r
}

/// Fisher-Yates shuffle of `s` (multiset-preserving).
fn shuffle_slice(s: &mut [String]) {
    let mut r = rng_seed();
    for i in (1..s.len()).rev() {
        r = xorshift(r);
        let j = (r % (i as u64 + 1)) as usize;
        s.swap(i, j);
    }
}

/// A random index in `0..n` that is **not** `exclude` (when `n > 1`).
#[must_use]
fn random_other_index(n: usize, exclude: usize) -> usize {
    if n <= 1 {
        return 0;
    }
    let r = xorshift(rng_seed());
    let mut idx = (r % n as u64) as usize;
    if idx == exclude {
        idx = (idx + 1) % n;
    }
    idx
}

/// Resolved metadata for the current track, cached so the MPRIS `Metadata`
/// getter is a cheap build with no borrows and no blocking on a miss.
#[derive(Debug, Clone, Default)]
struct NowPlaying {
    song_id: String,
    title: String,
    artist: String,
    album: String,
    /// Track length in microseconds (MPRIS `mpris:length`); 0 = unknown.
    length_us: i64,
    art_url: String,
    stream_url: String,
}

/// Insert an owned `s` string value under `key` when non-empty.
fn insert_str(m: &mut HashMap<String, OwnedValue>, key: &str, val: &str) {
    if !val.is_empty() {
        if let Ok(v) = OwnedValue::try_from(Value::from(val.to_string())) {
            m.insert(key.to_string(), v);
        }
    }
}

/// Build the MPRIS `Metadata` `a{sv}` dict from the cached now-playing
/// track. Always carries a valid `mpris:trackid`; the `xesam:*` + length +
/// art keys are present when known.
#[must_use]
fn metadata_map(now: &NowPlaying) -> HashMap<String, OwnedValue> {
    let mut m: HashMap<String, OwnedValue> = HashMap::new();
    if let Ok(path) = ObjectPath::try_from(track_path(&now.song_id)) {
        if let Ok(v) = OwnedValue::try_from(Value::from(path)) {
            m.insert("mpris:trackid".to_string(), v);
        }
    }
    insert_str(&mut m, "xesam:title", &now.title);
    insert_str(&mut m, "xesam:album", &now.album);
    insert_str(&mut m, "xesam:url", &now.stream_url);
    insert_str(&mut m, "mpris:artUrl", &now.art_url);
    if !now.artist.is_empty() {
        if let Ok(v) = OwnedValue::try_from(Value::from(vec![now.artist.clone()])) {
            m.insert("xesam:artist".to_string(), v);
        }
    }
    if now.length_us > 0 {
        if let Ok(v) = OwnedValue::try_from(Value::from(now.length_us)) {
            m.insert("mpris:length".to_string(), v);
        }
    }
    m
}

// ───────────────────────── root interface ─────────────────────────

/// `org.mpris.MediaPlayer2` — the root media-player object.
struct MediaPlayer2;

#[interface(name = "org.mpris.MediaPlayer2")]
impl MediaPlayer2 {
    /// Bring the player to the foreground by launching the `mde-music`
    /// window (best-effort).
    async fn raise(&self) {
        let _ = std::process::Command::new("mde-music").spawn();
    }

    /// MPRIS `Quit`. The daemon is a session service managed by systemd
    /// (AIR-1), so this is a no-op rather than killing playback for the
    /// whole session.
    async fn quit(&self) {}

    #[zbus(property)]
    async fn can_quit(&self) -> bool {
        false
    }

    #[zbus(property)]
    async fn can_raise(&self) -> bool {
        true
    }

    #[zbus(property)]
    async fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property)]
    async fn identity(&self) -> String {
        "MDE Music".to_string()
    }

    #[zbus(property)]
    async fn desktop_entry(&self) -> String {
        "mde-music".to_string()
    }

    #[zbus(property)]
    async fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(property)]
    async fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

// ───────────────────────── player interface ─────────────────────────

/// `org.mpris.MediaPlayer2.Player` — the transport surface. Cheap to clone
/// (everything behind it is `Arc`/`PathBuf`), so zbus can register it.
#[derive(Clone)]
struct Player {
    engine: EngineHandle,
    queue_path: PathBuf,
    data_dir: PathBuf,
    now: Arc<Mutex<NowPlaying>>,
}

impl Player {
    /// Build a player bound to the shared engine + the on-disk queue/state.
    fn new(engine: EngineHandle, queue_path: PathBuf, data_dir: PathBuf) -> Self {
        Self {
            engine,
            queue_path,
            data_dir,
            now: Arc::new(Mutex::new(NowPlaying::default())),
        }
    }

    /// Load the mesh-shared Airsonic creds → a client (per-call, so a
    /// mid-session connect is picked up). `None` when no creds are set.
    fn client(&self) -> Option<Client> {
        creds::load()
            .ok()
            .map(|c| Client::new(&c.server_url, &c.username, &c.password))
    }

    /// Write this peer's authoritative [`MusicState`] (AIR-8 heartbeat).
    fn write_state(&self, playing: bool, song_id: &str, position_ms: u64) {
        let st = MusicState {
            peer: state::local_host(),
            playing,
            song_id: song_id.to_string(),
            position_ms,
            updated_ms: state::now_ms(),
        };
        let _ = state::write_state(&self.data_dir, &st);
    }

    /// Resolve + cache the metadata for `song_id` via `getSong`, falling
    /// back to the id as the title when the lookup fails (so `Metadata`
    /// is never empty for a loaded track).
    async fn resolve_now(&self, client: &Client, song_id: &str) {
        let mut np = NowPlaying {
            song_id: song_id.to_string(),
            stream_url: client.stream_url(song_id),
            ..NowPlaying::default()
        };
        if let Ok(song) = client.get_song(song_id).await {
            np.title = song.title;
            np.artist = song.artist;
            np.album = song.album;
            np.length_us = i64::from(song.duration).saturating_mul(1_000_000);
            if !song.cover_art.is_empty() {
                np.art_url = client.cover_art_url(&song.cover_art);
            }
        }
        if np.title.is_empty() {
            np.title = song_id.to_string();
        }
        *self
            .now
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = np;
    }

    /// (Re)issue gapless playback of the queue tail from the current track,
    /// honoring shuffle (the tail handed to the engine is shuffled, current
    /// kept first), and cache the current track's metadata. No-op without a
    /// client or on an empty queue.
    async fn start_play(&self) {
        let Some(client) = self.client() else { return };
        let q = queue::read_from(&self.queue_path);
        let Some(current) = q.current().map(ToString::to_string) else {
            return;
        };
        let mode = read_mode(&self.data_dir);
        let mut tail: Vec<String> = q.songs.iter().skip(q.current).cloned().collect();
        if mode.shuffle && tail.len() > 2 {
            shuffle_slice(&mut tail[1..]);
        }
        let tracks: Vec<(String, SourceCodec)> = tail
            .iter()
            .map(|id| (client.stream_url(id), SourceCodec::Unknown))
            .collect();
        if tracks.is_empty() {
            return;
        }
        self.engine.play(tracks);
        self.resolve_now(&client, &current).await;
        self.write_state(true, &current, 0);
    }

    /// Advance the queue cursor (forward = `Next`, else `Previous`) per the
    /// shuffle/repeat mode, persist it, and re-issue playback when active.
    async fn advance(&self, forward: bool) {
        let mut q = queue::read_from(&self.queue_path);
        if q.is_empty() {
            return;
        }
        let was_active = self.engine.is_active();
        let mode = read_mode(&self.data_dir);
        let mut hit_end = false;
        if forward {
            if mode.loop_status == LoopStatus::Track {
                // Repeat-one: cursor unchanged, replay the current track.
            } else if mode.shuffle && q.len() > 1 {
                q.current = random_other_index(q.len(), q.current);
            } else if q.next().is_none() {
                if mode.loop_status == LoopStatus::Playlist {
                    q.current = 0;
                } else {
                    hit_end = true;
                }
            }
        } else if mode.loop_status != LoopStatus::Track
            && q.prev().is_none()
            && mode.loop_status == LoopStatus::Playlist
        {
            // Previous hit the start under Playlist repeat → wrap to the end.
            // (`q.prev()` runs whenever we're not repeat-one, stepping the
            // cursor back or holding at the first track when there's none.)
            q.current = q.len() - 1;
        }
        let _ = queue::write_to(&self.queue_path, &q);
        if hit_end {
            self.engine.stop();
            self.write_state(false, "", 0);
            *self
                .now
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = NowPlaying::default();
            return;
        }
        if was_active {
            self.start_play().await;
        } else if let (Some(client), Some(cur)) =
            (self.client(), q.current().map(ToString::to_string))
        {
            self.resolve_now(&client, &cur).await;
        }
    }

    /// Emit `PropertiesChanged` for the properties that a transport action
    /// mutates as a side effect (`PlaybackStatus` + `Metadata`). Volume /
    /// shuffle / loop changes are emitted automatically by zbus on `Set`.
    async fn notify(&self, emitter: &SignalEmitter<'_>) {
        let _ = self.playback_status_changed(emitter).await;
        let _ = self.metadata_changed(emitter).await;
    }
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    /// MPRIS `Play`: resume a paused buffer, or start the queue when
    /// stopped. A no-op while already playing.
    async fn play(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        if self.engine.is_playing() {
            return;
        } else if self.engine.is_active() {
            self.engine.resume();
            let q = queue::read_from(&self.queue_path);
            self.write_state(true, q.current().unwrap_or(""), self.engine.position_ms());
        } else {
            self.start_play().await;
        }
        self.notify(&emitter).await;
    }

    /// MPRIS `Pause`: hold the buffer (resume is seamless).
    async fn pause(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        self.engine.pause();
        let q = queue::read_from(&self.queue_path);
        self.write_state(false, q.current().unwrap_or(""), self.engine.position_ms());
        self.notify(&emitter).await;
    }

    /// MPRIS `PlayPause`: toggle between the two.
    async fn play_pause(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        if self.engine.is_playing() {
            self.engine.pause();
            let q = queue::read_from(&self.queue_path);
            self.write_state(false, q.current().unwrap_or(""), self.engine.position_ms());
        } else if self.engine.is_active() {
            self.engine.resume();
            let q = queue::read_from(&self.queue_path);
            self.write_state(true, q.current().unwrap_or(""), self.engine.position_ms());
        } else {
            self.start_play().await;
        }
        self.notify(&emitter).await;
    }

    /// MPRIS `Stop`: stop + clear the buffer and the now-playing record.
    async fn stop(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        self.engine.stop();
        self.write_state(false, "", 0);
        *self
            .now
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = NowPlaying::default();
        self.notify(&emitter).await;
    }

    /// MPRIS `Next`: advance per the shuffle/repeat mode.
    async fn next(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        self.advance(true).await;
        self.notify(&emitter).await;
    }

    /// MPRIS `Previous`: step back per the repeat mode.
    async fn previous(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        self.advance(false).await;
        self.notify(&emitter).await;
    }

    /// MPRIS `Seek`. The AIR-5 engine has no seek yet (`CanSeek` is
    /// `false`), so per the MPRIS contract this has no effect.
    async fn seek(&self, _offset: i64) {}

    /// MPRIS `SetPosition`. No effect while `CanSeek` is `false`.
    async fn set_position(&self, _track_id: ObjectPath<'_>, _position: i64) {}

    /// MPRIS `OpenUri`. Playback is queue-driven (Airsonic ids), so an
    /// arbitrary URI open is unsupported.
    async fn open_uri(&self, _uri: String) {}

    #[zbus(property)]
    async fn playback_status(&self) -> String {
        playback_status(self.engine.is_active(), self.engine.is_playing()).to_string()
    }

    #[zbus(property)]
    async fn loop_status(&self) -> String {
        read_mode(&self.data_dir).loop_status.as_mpris().to_string()
    }

    #[zbus(property)]
    async fn set_loop_status(&self, value: String) {
        if let Some(ls) = LoopStatus::from_mpris(&value) {
            let mut mode = read_mode(&self.data_dir);
            mode.loop_status = ls;
            write_mode(&self.data_dir, &mode);
        }
    }

    #[zbus(property)]
    async fn shuffle(&self) -> bool {
        read_mode(&self.data_dir).shuffle
    }

    #[zbus(property)]
    async fn set_shuffle(&self, value: bool) {
        let mut mode = read_mode(&self.data_dir);
        mode.shuffle = value;
        write_mode(&self.data_dir, &mode);
    }

    #[zbus(property)]
    async fn metadata(&self) -> HashMap<String, OwnedValue> {
        // Self-heal against Bus/GUI-initiated track changes: if the queue's
        // current track no longer matches the cache, re-resolve it.
        let q = queue::read_from(&self.queue_path);
        let current = q.current().unwrap_or("").to_string();
        let cached = self
            .now
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .song_id
            .clone();
        if current != cached {
            if current.is_empty() {
                *self
                    .now
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = NowPlaying::default();
            } else if let Some(client) = self.client() {
                self.resolve_now(&client, &current).await;
            }
        }
        let np = self
            .now
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        metadata_map(&np)
    }

    #[zbus(property)]
    async fn volume(&self) -> f64 {
        f64::from(self.engine.volume())
    }

    #[zbus(property)]
    async fn set_volume(&self, value: f64) {
        self.engine.set_volume(value as f32);
    }

    #[zbus(property)]
    async fn position(&self) -> i64 {
        i64::try_from(self.engine.position_ms().saturating_mul(1000)).unwrap_or(i64::MAX)
    }

    #[zbus(property)]
    async fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    async fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    async fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    async fn can_go_next(&self) -> bool {
        let q = queue::read_from(&self.queue_path);
        q.len() > 1 || read_mode(&self.data_dir).loop_status != LoopStatus::None
    }

    #[zbus(property)]
    async fn can_go_previous(&self) -> bool {
        let q = queue::read_from(&self.queue_path);
        q.len() > 1 || read_mode(&self.data_dir).loop_status != LoopStatus::None
    }

    #[zbus(property)]
    async fn can_play(&self) -> bool {
        true
    }

    #[zbus(property)]
    async fn can_pause(&self) -> bool {
        true
    }

    #[zbus(property)]
    async fn can_seek(&self) -> bool {
        false
    }

    #[zbus(property)]
    async fn can_control(&self) -> bool {
        true
    }
}

// ───────────────────────── spawn / lifecycle ─────────────────────────

/// A running MPRIS surface. Stops the surface thread on [`stop`](Self::stop)
/// or on drop.
pub struct MprisHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl MprisHandle {
    /// Signal the MPRIS thread to stop, then join it.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for MprisHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start the MPRIS `org.mpris.MediaPlayer2` surface on its own thread,
/// sharing the engine + on-disk queue/state with the Bus responder. The
/// returned [`MprisHandle`] keeps it alive until dropped.
///
/// Degrades to a no-op (a handle whose thread exits immediately) when there
/// is no reachable session bus — a headless peer keeps Bus + queue working
/// without MPRIS.
#[must_use]
pub fn spawn(engine: EngineHandle, queue_path: PathBuf, data_dir: PathBuf) -> MprisHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let join = std::thread::Builder::new()
        .name("mde-musicd-mpris".to_string())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    eprintln!("mde-musicd: MPRIS runtime unavailable: {e}");
                    return;
                }
            };
            rt.block_on(async move {
                let player = Player::new(engine, queue_path, data_dir);
                let built = zbus::connection::Builder::session()
                    .and_then(|b| b.name(BUS_NAME))
                    .and_then(|b| b.serve_at(OBJECT_PATH, MediaPlayer2))
                    .and_then(|b| b.serve_at(OBJECT_PATH, player));
                let _conn = match built {
                    Ok(b) => match b.build().await {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!(
                                "mde-musicd: no MPRIS session bus ({e}); media-key control disabled"
                            );
                            return;
                        }
                    },
                    Err(e) => {
                        eprintln!("mde-musicd: MPRIS setup failed: {e}");
                        return;
                    }
                };
                // Keep the connection alive until asked to stop.
                while !stop_thread.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            });
        })
        .ok();
    MprisHandle { stop, join }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn playback_status_maps_engine_flags() {
        assert_eq!(playback_status(true, true), "Playing");
        assert_eq!(playback_status(true, false), "Paused");
        assert_eq!(playback_status(false, false), "Stopped");
        assert_eq!(playback_status(false, true), "Stopped");
    }

    #[test]
    fn track_path_is_a_valid_object_path() {
        // Plain id → namespaced path; reserved bytes escaped _HH.
        assert_eq!(track_path("ab_9"), "/org/mackes/mde/music/track/ab_9");
        assert_eq!(track_path("a/b"), "/org/mackes/mde/music/track/a_2Fb");
        // Every produced path parses as a D-Bus object path.
        for id in ["song-7", "a b", "", "tr/1:x"] {
            assert!(
                ObjectPath::try_from(track_path(id)).is_ok(),
                "bad path for {id:?}"
            );
        }
        assert_eq!(track_path(""), NO_TRACK);
    }

    #[test]
    fn loop_status_round_trips() {
        for (s, v) in [
            ("None", LoopStatus::None),
            ("Track", LoopStatus::Track),
            ("Playlist", LoopStatus::Playlist),
        ] {
            assert_eq!(LoopStatus::from_mpris(s), Some(v));
            assert_eq!(v.as_mpris(), s);
        }
        assert_eq!(LoopStatus::from_mpris("Sideways"), None);
        assert_eq!(LoopStatus::default(), LoopStatus::None);
    }

    #[test]
    fn playback_mode_persists() {
        let dir = tempdir().unwrap();
        assert_eq!(read_mode(dir.path()), PlaybackMode::default());
        let mode = PlaybackMode {
            shuffle: true,
            loop_status: LoopStatus::Playlist,
        };
        write_mode(dir.path(), &mode);
        assert_eq!(read_mode(dir.path()), mode);
    }

    #[test]
    fn metadata_map_always_has_trackid_and_optional_fields() {
        // Loaded track → trackid + title + artist (array) + url + length.
        let now = NowPlaying {
            song_id: "tr-9".into(),
            title: "So What".into(),
            artist: "Miles Davis".into(),
            album: "Kind of Blue".into(),
            length_us: 545_000_000,
            art_url: "http://h/art".into(),
            stream_url: "http://h/stream".into(),
        };
        let m = metadata_map(&now);
        assert!(m.contains_key("mpris:trackid"));
        assert!(m.contains_key("xesam:title"));
        assert!(m.contains_key("xesam:artist"));
        assert!(m.contains_key("mpris:length"));
        assert!(m.contains_key("mpris:artUrl"));
        // Empty now-playing → just the no-track id, no optional keys.
        let empty = metadata_map(&NowPlaying::default());
        assert!(empty.contains_key("mpris:trackid"));
        assert!(!empty.contains_key("xesam:title"));
        assert!(!empty.contains_key("mpris:length"));
    }

    #[test]
    fn shuffle_preserves_the_multiset() {
        let mut v: Vec<String> = (0..20).map(|i| i.to_string()).collect();
        let original = v.clone();
        shuffle_slice(&mut v);
        let mut sorted = v.clone();
        sorted.sort();
        let mut orig_sorted = original;
        orig_sorted.sort();
        assert_eq!(sorted, orig_sorted, "shuffle must not add/drop elements");
    }

    #[test]
    fn random_other_index_avoids_exclude() {
        for _ in 0..50 {
            let idx = random_other_index(5, 2);
            assert!(idx < 5);
            assert_ne!(idx, 2);
        }
        // Degenerate sizes.
        assert_eq!(random_other_index(1, 0), 0);
        assert_eq!(random_other_index(0, 0), 0);
    }
}
