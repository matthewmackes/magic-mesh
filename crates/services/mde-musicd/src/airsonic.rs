//! AIR-4 (v6.1) — Subsonic/Airsonic REST client.
//!
//! The Subsonic API (which Airsonic / Navidrome / Gonic all speak)
//! authenticates every request with a salted token:
//!
//!   `token = md5(password + salt)`, sent as `t=<token>&s=<salt>`
//!
//! plus `u=<user>`, `v=<api-version>`, `c=<client-name>`, `f=json`.
//! Every endpoint is `<base>/rest/<view>?<params>` and replies with a
//! `{"subsonic-response": {status, version, error?, <data>}}` envelope.
//!
//! This module ships the **core** endpoints the album / artist / search
//! / play flow needs (ping, getArtists, getAlbumList2, search3, plus the
//! `stream` + `getCoverArt` URL builders the playback engine + cache
//! fetch against). The niche endpoints (podcasts / radio / lyrics /
//! genres) land with their consuming UI as AIR-4.b.
//!
//! Everything except the actual HTTP round-trip is a pure function
//! (`auth_token`, `query_params`, `endpoint_url`, `stream_url`,
//! `cover_art_url`, and the `parse_*` helpers over `serde_json::Value`)
//! so the client is fully unit-testable without a live server.

use std::fmt;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

/// Subsonic API version this client advertises by default (`v=`). 1.16.1 covers
/// every endpoint we call. It is the **ceiling**, not a hard floor: a server
/// older than this rejects the request with API error 30 ("incompatible REST
/// protocol version, server must upgrade"), so the client auto-negotiates DOWN to
/// the server's reported version and retries (see [`Client::get`]). This makes
/// `mde-music` work against older Airsonic/Subsonic servers, not just current ones.
pub const API_VERSION: &str = "1.16.1";

/// Subsonic error code returned when the client's requested `v=` is newer than
/// the server supports ("incompatible REST protocol version, server must upgrade").
const ERR_INCOMPATIBLE_VERSION: i64 = 30;

/// Conservative fallback `v=` used when a server rejects our version (error 30)
/// but doesn't report its own version in the envelope. 1.13.0 is implemented by
/// essentially every Subsonic-era server (Subsonic 5.3+).
const FALLBACK_API_VERSION: &str = "1.13.0";

/// AIR — persisted negotiated `v=`. The negotiation in [`Client::get`] only runs
/// on the JSON path; the RAW-byte fetches (the playback `stream` URL + cover-art
/// bytes) build URLs from [`Client::api_version`] and never self-negotiate, and
/// the serve loop builds a FRESH `Client` (version reset to the ceiling) on every
/// poll sweep. So a server that caps below our ceiling would serve error-30 JSON
/// in place of audio/image bytes forever — "plays but silent, no artwork" (live
/// 2026-06-17). Persisting the negotiated version to this file (sibling of the
/// creds) lets every freshly-built client — including the engine's stream URL —
/// SEED at the last-known-good version, so audio + art work on the first try and
/// across daemon restarts.
const VERSION_REL_PATH: &str = ".local/share/mde/airsonic-api-version";

/// Path of the persisted negotiated API version (`$HOME/<VERSION_REL_PATH>`,
/// `/root` when `$HOME` is unset — matching [`crate::creds::default_path`]).
fn version_file_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    std::path::Path::new(&home).join(VERSION_REL_PATH)
}

/// Read the persisted negotiated version, if a valid one was written. Returns
/// `None` (→ seed at the ceiling) when absent/empty/malformed.
fn read_persisted_version() -> Option<String> {
    let raw = std::fs::read_to_string(version_file_path()).ok()?;
    let v = raw.trim();
    // Guard against junk: must look like a dotted version no newer than our ceiling.
    if v.is_empty() || cmp_version(v, API_VERSION) == std::cmp::Ordering::Greater {
        return None;
    }
    Some(v.to_string())
}

/// Persist the negotiated version (best-effort; a write failure just means the
/// next client re-negotiates). Creates the parent dir if needed.
fn write_persisted_version(v: &str) {
    let path = version_file_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, v);
}

/// If `bytes` is a Subsonic error-30 envelope (a capped server answering a
/// raw-byte endpoint with JSON instead of media), return the server's reported
/// `version` (falling back to the conservative version when omitted). `None`
/// when the bytes are real media (not JSON) or a different/no error.
fn error30_server_version(bytes: &[u8]) -> Option<String> {
    // Cheap guard: media bytes won't start with a JSON object brace.
    let first = bytes.iter().find(|b| !b.is_ascii_whitespace())?;
    if *first != b'{' {
        return None;
    }
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let resp = v.get("subsonic-response")?;
    let code = resp
        .get("error")
        .and_then(|e| e.get("code"))
        .and_then(Value::as_i64);
    if code != Some(ERR_INCOMPATIBLE_VERSION) {
        return None;
    }
    Some(
        resp.get("version")
            .and_then(Value::as_str)
            .map_or_else(|| FALLBACK_API_VERSION.to_string(), str::to_string),
    )
}

/// Compare two dotted Subsonic versions (`"1.16.1"`); returns `Ordering` on the
/// numeric components (missing components count as 0). Non-numeric components sort
/// as 0 so a malformed version never panics.
fn cmp_version(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |s: &str| -> Vec<u32> { s.split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    let (va, vb) = (parse(a), parse(b));
    for i in 0..va.len().max(vb.len()) {
        let (x, y) = (
            va.get(i).copied().unwrap_or(0),
            vb.get(i).copied().unwrap_or(0),
        );
        match x.cmp(&y) {
            std::cmp::Ordering::Equal => continue,
            ord => return ord,
        }
    }
    std::cmp::Ordering::Equal
}

/// The lower of two versions (so we never advertise higher than the server or our
/// own ceiling). Pure + testable.
#[must_use]
fn min_version(a: &str, b: &str) -> String {
    if cmp_version(a, b) == std::cmp::Ordering::Greater {
        b.to_string()
    } else {
        a.to_string()
    }
}

/// Client identifier sent as `c=` (shows up in the server's session
/// list).
pub const CLIENT_NAME: &str = "mde-music";

/// Active-active media DNS endpoint written by `mackesd` on each node.
pub const MUSIC_READ_HOST: &str = "music.mesh";

/// Deterministic single-writer media DNS endpoint for playlist mutations.
pub const MUSIC_WRITER_HOST: &str = "music-writer.mesh";

/// Optional override for nonstandard media deployments.
pub const MUSIC_WRITER_URL_ENV: &str = "MDE_MUSIC_WRITER_URL";

/// `token = md5(password + salt)`, lower-hex. The Subsonic auth scheme
/// (avoids sending the password in clear on every request).
#[must_use]
pub fn auth_token(password: &str, salt: &str) -> String {
    let digest = md5::compute(format!("{password}{salt}").as_bytes());
    format!("{digest:x}")
}

/// A failure reaching or parsing the Airsonic server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AirsonicError {
    /// Transport failure (DNS, connect, timeout, non-2xx).
    Http(String),
    /// The server returned `status: "failed"` with a Subsonic error.
    Api {
        /// Subsonic error code (e.g. 40 = wrong credentials).
        code: i64,
        /// Human-readable error description from the server.
        message: String,
    },
    /// The envelope didn't parse / was missing expected fields.
    Parse(String),
}

impl fmt::Display for AirsonicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(e) => write!(f, "airsonic transport error: {e}"),
            Self::Api { code, message } => {
                write!(f, "airsonic API error {code}: {message}")
            }
            Self::Parse(e) => write!(f, "airsonic response parse error: {e}"),
        }
    }
}

impl std::error::Error for AirsonicError {}

/// An artist row from `getArtists`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Deserialize)]
pub struct Artist {
    /// Subsonic artist id (opaque server-assigned string).
    pub id: String,
    /// Display name of the artist.
    pub name: String,
    #[serde(default, rename = "albumCount")]
    /// Number of albums the server has for this artist.
    pub album_count: u32,
}

/// A genre row from `getGenres` (its `value` is also its id for
/// `getAlbumList2?type=byGenre&genre=<value>`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Deserialize)]
pub struct Genre {
    #[serde(rename = "value")]
    /// Genre label (also the `genre=` query value for `getAlbumList2?type=byGenre`).
    pub name: String,
    #[serde(default, rename = "albumCount")]
    /// Number of albums tagged with this genre on the server.
    pub album_count: u32,
    #[serde(default, rename = "songCount")]
    /// Number of songs tagged with this genre on the server.
    pub song_count: u32,
}

/// A podcast channel from `getPodcasts` (AIR-21). Serialized as
/// `{id, title}` so the GUI's `parse_items` reads it like any row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PodcastChannel {
    /// Subsonic channel id — used as the `id=` param for `getPodcasts` to fetch episodes.
    pub id: String,
    /// Display name of the podcast channel.
    pub title: String,
}

/// An internet radio station from `getInternetRadioStations` (SVC-3).
/// `stream_url` is the raw upstream URL the engine plays directly.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RadioStation {
    /// Subsonic station id (opaque server-assigned string).
    pub id: String,
    /// Display name of the radio station.
    pub name: String,
    #[serde(rename = "streamUrl")]
    /// Raw upstream stream URL the playback engine connects to directly.
    pub stream_url: String,
}

/// A podcast episode from `getPodcasts?id=<channel>` (AIR-21). `id` is the
/// episode's `streamId` — the media id the player streams + enqueues.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PodcastEpisode {
    /// Playable media id — the episode's `streamId` (falls back to `id`) for the stream endpoint.
    pub id: String,
    /// Display title of the episode.
    pub title: String,
}

/// An album row from `getAlbumList2` / `search3`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Deserialize)]
pub struct Album {
    /// Subsonic album id (opaque server-assigned string).
    pub id: String,
    /// Album title.
    pub name: String,
    #[serde(default)]
    /// Primary artist display name.
    pub artist: String,
    #[serde(default, rename = "artistId")]
    /// Id of the primary artist (links to `getArtist`).
    pub artist_id: String,
    #[serde(default, rename = "songCount")]
    /// Number of tracks on the album.
    pub song_count: u32,
    #[serde(default, rename = "coverArt")]
    /// Cover-art token — resolve via [`Client::cover_art_url`].
    pub cover_art: String,
    #[serde(default)]
    /// Release year, if the server has it.
    pub year: Option<u32>,
}

/// A song row from `search3` (and, later, `getAlbum`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Deserialize)]
pub struct Song {
    /// Subsonic song id — passed to `stream` / `getSong`.
    pub id: String,
    /// Track title.
    pub title: String,
    #[serde(default)]
    /// Album title the song belongs to.
    pub album: String,
    #[serde(default)]
    /// Primary artist display name.
    pub artist: String,
    #[serde(default)]
    /// Track length in seconds.
    pub duration: u32,
    #[serde(default)]
    /// 1-based disc-ordered track number, if present.
    pub track: Option<u32>,
    #[serde(default)]
    /// File format suffix (`flac` / `mp3` / `opus` / …).
    pub suffix: String,
    /// `coverArt` token — resolve via [`Client::cover_art_url`] for the
    /// MPRIS `mpris:artUrl` + the AIR-12 album art.
    #[serde(default, rename = "coverArt")]
    pub cover_art: String,
}

/// Result of `search3` — three independently-scrollable sections.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchResult3 {
    /// Artist matches from the `artist[]` section of `searchResult3`.
    pub artists: Vec<Artist>,
    /// Album matches from the `album[]` section of `searchResult3`.
    pub albums: Vec<Album>,
    /// Song matches from the `song[]` section of `searchResult3`.
    pub songs: Vec<Song>,
}

/// Result of `getAlbum` — the album's metadata + its ordered track list.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AlbumDetail {
    /// Album metadata (id, name, artist, cover, year, …).
    pub album: Album,
    /// Ordered track list for the album.
    pub songs: Vec<Song>,
}

/// MUSIC-HOME-1 — a snapshot of the server's library for the Music Home page.
/// All counts are live Airsonic data; `reachable` is false when the server can't
/// be reached (the GUI then shows a connection-down server card).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct LibraryStats {
    /// Whether the Airsonic server answered the stats sweep.
    pub reachable: bool,
    /// The configured server base URL or host label shown in the UI.
    pub host: String,
    /// The negotiated Subsonic API version reported by the server/client.
    pub version: String,
    /// Total song count reported by the library.
    pub songs: u64,
    /// Whether the server reports an active library scan.
    pub scanning: bool,
    /// Total artist count reported by the library.
    pub artists: u64,
    /// Total album count reported by the library.
    pub albums: u64,
    /// Total genre count reported by the library.
    pub genres: u64,
    /// Total playlist count reported by the library.
    pub playlists: u64,
    /// Total internet radio station count reported by the library.
    pub radio: u64,
    /// Total podcast count reported by the library.
    pub podcasts: u64,
}

/// Authenticated Subsonic/Airsonic REST client with negotiated API version state.
pub struct Client {
    base_url: String,
    user: String,
    token: String,
    salt: String,
    http: reqwest::Client,
    /// The `v=` we currently advertise. Starts at [`API_VERSION`] and is
    /// negotiated DOWN (interior-mutable, since `get` is `&self`) the first time a
    /// server rejects us with error 30 — so an older server starts working
    /// transparently after one retry.
    version: std::sync::Mutex<String>,
}

impl Client {
    /// Build a client, generating a random per-session salt + token.
    #[must_use]
    pub fn new(base_url: impl Into<String>, user: impl Into<String>, password: &str) -> Self {
        Self::with_salt(base_url, user, password, &ulid::Ulid::new().to_string())
    }

    /// Build a client with an explicit salt (deterministic — used by
    /// tests + by callers that want a stable session salt).
    #[must_use]
    pub fn with_salt(
        base_url: impl Into<String>,
        user: impl Into<String>,
        password: &str,
        salt: &str,
    ) -> Self {
        let base = base_url.into();
        Self {
            base_url: base.trim_end_matches('/').to_string(),
            user: user.into(),
            token: auth_token(password, salt),
            salt: salt.to_string(),
            http: Self::build_http_client(),
            // Seed at the last-known-good negotiated version so the RAW-byte
            // fetches (stream/cover-art) hit a working `v=` immediately, not the
            // ceiling that a capped server would reject with error 30.
            version: std::sync::Mutex::new(
                read_persisted_version().unwrap_or_else(|| API_VERSION.to_string()),
            ),
        }
    }

    /// The `v=` currently advertised (negotiated value; [`API_VERSION`] until a
    /// server forces a downgrade).
    #[must_use]
    pub fn api_version(&self) -> String {
        self.version
            .lock()
            .map(|v| v.clone())
            .unwrap_or_else(|_| API_VERSION.to_string())
    }

    /// AUDIT-MESH-14 — the HTTP client used for every Airsonic call, with hard
    /// connect + total timeouts. Without these (`reqwest::Client::new()` has no
    /// timeout), a call to an unreachable/misconfigured server hangs forever and
    /// wedges `mde-musicd`'s single-threaded Bus serve loop, starving EVERY
    /// `action/music/*` verb (get-state, list-albums, …) — observed live on a
    /// node with no reachable Airsonic. Falls back to the untimed client only if
    /// the builder somehow fails (it won't with these options).
    #[must_use]
    fn build_http_client() -> reqwest::Client {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    }

    /// The common auth + format query params attached to every call.
    #[must_use]
    pub fn query_params(&self) -> Vec<(String, String)> {
        vec![
            ("u".into(), self.user.clone()),
            ("t".into(), self.token.clone()),
            ("s".into(), self.salt.clone()),
            ("v".into(), self.api_version()),
            ("c".into(), CLIENT_NAME.into()),
            ("f".into(), "json".into()),
        ]
    }

    /// Build the full URL for `view` with `extra` query params appended.
    #[must_use]
    pub fn endpoint_url(&self, view: &str, extra: &[(&str, &str)]) -> String {
        self.endpoint_url_at_base(&self.base_url, view, extra)
    }

    /// Build the full URL for `view` against an explicit base URL.
    #[must_use]
    pub fn endpoint_url_at_base(
        &self,
        base_url: &str,
        view: &str,
        extra: &[(&str, &str)],
    ) -> String {
        let mut params = self.query_params();
        for (k, v) in extra {
            params.push(((*k).to_string(), (*v).to_string()));
        }
        let query = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencode(v)))
            .collect::<Vec<_>>()
            .join("&");
        format!("{}/rest/{view}?{query}", base_url.trim_end_matches('/'))
    }

    /// MEDIA-6 — playlist writes must land on one durable writer instead of the
    /// active-active read endpoint. The default autoconfig URL is
    /// `music.mesh`; swap only that host to `music-writer.mesh`. Manual/legacy
    /// URLs keep their original base unless the operator provides
    /// [`MUSIC_WRITER_URL_ENV`].
    #[must_use]
    pub fn playlist_writer_base_url(&self) -> String {
        if let Ok(override_url) = std::env::var(MUSIC_WRITER_URL_ENV) {
            let trimmed = override_url.trim().trim_end_matches('/');
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        self.base_url.replace(MUSIC_READ_HOST, MUSIC_WRITER_HOST)
    }

    /// Build a URL for a playlist mutation against the deterministic writer.
    #[must_use]
    pub fn playlist_writer_endpoint_url(&self, view: &str, extra: &[(&str, &str)]) -> String {
        self.endpoint_url_at_base(&self.playlist_writer_base_url(), view, extra)
    }

    /// Direct stream URL for a song id (the playback engine + cache
    /// fetch GET this).
    #[must_use]
    pub fn stream_url(&self, song_id: &str) -> String {
        // SVC-3 — radio stations enqueue their raw upstream URL as the
        // "song id"; pass URLs through untouched so the engine streams
        // the station directly instead of asking Subsonic for it.
        if song_id.starts_with("http://") || song_id.starts_with("https://") {
            return song_id.to_string();
        }
        self.endpoint_url("stream", &[("id", song_id)])
    }

    /// Cover-art URL for an id (album/song coverArt token).
    #[must_use]
    pub fn cover_art_url(&self, cover_id: &str) -> String {
        self.endpoint_url("getCoverArt", &[("id", cover_id)])
    }

    /// GET a `view`, returning the inner `subsonic-response` object on
    /// `status: "ok"` or the appropriate [`AirsonicError`].
    ///
    /// # Errors
    /// Transport, API-error, or parse failures.
    pub async fn get(&self, view: &str, extra: &[(&str, &str)]) -> Result<Value, AirsonicError> {
        self.get_at_base(&self.base_url, view, extra).await
    }

    async fn get_at_base(
        &self,
        base_url: &str,
        view: &str,
        extra: &[(&str, &str)],
    ) -> Result<Value, AirsonicError> {
        let body = self.fetch_body_at_base(base_url, view, extra).await?;
        match unwrap_envelope(&body) {
            Err(AirsonicError::Api {
                code: ERR_INCOMPATIBLE_VERSION,
                ..
            }) => {
                // Our advertised v= is newer than the server supports. The Subsonic
                // error envelope still carries the server's own REST `version` — so
                // negotiate down to it (clamped to our ceiling) and retry ONCE.
                // Falls back to a conservative version if the server omits its own.
                let server_v = body
                    .get("subsonic-response")
                    .and_then(|r| r.get("version"))
                    .and_then(Value::as_str)
                    .map_or_else(
                        || FALLBACK_API_VERSION.to_string(),
                        |sv| min_version(sv, API_VERSION),
                    );
                let changed = self
                    .version
                    .lock()
                    .map(|mut cur| {
                        if *cur != server_v {
                            *cur = server_v.clone();
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                if changed {
                    // Persist so the next freshly-built client (and the engine's
                    // stream URL) seeds at this version — fixes silent audio /
                    // missing art that the JSON-only retry never reached.
                    write_persisted_version(&server_v);
                }
                if changed {
                    // Retry with the negotiated version (it sticks for later calls).
                    let body2 = self.fetch_body_at_base(base_url, view, extra).await?;
                    unwrap_envelope(&body2)
                } else {
                    unwrap_envelope(&body)
                }
            }
            other => other,
        }
    }

    async fn fetch_body_at_base(
        &self,
        base_url: &str,
        view: &str,
        extra: &[(&str, &str)],
    ) -> Result<Value, AirsonicError> {
        let url = self.endpoint_url_at_base(base_url, view, extra);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| AirsonicError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(AirsonicError::Http(format!("HTTP {}", resp.status())));
        }
        resp.json()
            .await
            .map_err(|e| AirsonicError::Parse(e.to_string()))
    }

    /// `ping` — returns the server's reported API version on success.
    ///
    /// # Errors
    /// Transport / API / parse failures.
    pub async fn ping(&self) -> Result<String, AirsonicError> {
        // The inner object carries `version`; `get` already verified
        // status == ok, so any success here means reachable.
        let inner = self.get("ping", &[]).await?;
        Ok(inner
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or(API_VERSION)
            .to_string())
    }

    /// `getArtists` — the full artist index, flattened.
    ///
    /// # Errors
    /// Transport / API / parse failures.
    pub async fn get_artists(&self) -> Result<Vec<Artist>, AirsonicError> {
        let inner = self.get("getArtists", &[]).await?;
        Ok(parse_artists(&inner))
    }

    /// `getAlbumList2` — `type` is one of `newest` / `recent` /
    /// `frequent` / `alphabeticalByName` / etc.
    ///
    /// # Errors
    /// Transport / API / parse failures.
    pub async fn get_album_list2(
        &self,
        list_type: &str,
        size: u32,
    ) -> Result<Vec<Album>, AirsonicError> {
        let size = size.to_string();
        let inner = self
            .get("getAlbumList2", &[("type", list_type), ("size", &size)])
            .await?;
        Ok(parse_album_list2(&inner))
    }

    /// MUSIC-HOME-3 — `getStarred2` → the user's starred albums (the Home
    /// "Starred" strip).
    ///
    /// # Errors
    /// Transport / API / parse failures.
    pub async fn get_starred2(&self) -> Result<Vec<Album>, AirsonicError> {
        let inner = self.get("getStarred2", &[]).await?;
        Ok(parse_starred_albums(&inner))
    }

    /// `search3` — three-section search across artists/albums/songs.
    ///
    /// # Errors
    /// Transport / API / parse failures.
    pub async fn search3(&self, query: &str) -> Result<SearchResult3, AirsonicError> {
        let inner = self
            .get(
                "search3",
                &[
                    ("query", query),
                    ("artistCount", "20"),
                    ("albumCount", "20"),
                    ("songCount", "20"),
                ],
            )
            .await?;
        Ok(parse_search3(&inner))
    }

    /// `getSong` — full metadata for one song id. Powers the AIR-6 MPRIS
    /// `Metadata` surface (title / artist / album / length / art) + the
    /// AIR-12 track rows. This is an AIR-4.b endpoint, landed early with
    /// its first consumer (AIR-6) per the §0.12 "an endpoint ships with a
    /// runtime caller, never a dead client method" rule.
    ///
    /// # Errors
    /// Transport / API / parse failures.
    pub async fn get_song(&self, id: &str) -> Result<Song, AirsonicError> {
        let inner = self.get("getSong", &[("id", id)]).await?;
        parse_song(&inner)
    }

    /// `getAlbum` — an album's metadata + its ordered track list (the
    /// AIR-12 album page). AIR-4.b endpoint, landed with its consumer.
    ///
    /// # Errors
    /// Transport / API / parse failures.
    pub async fn get_album(&self, id: &str) -> Result<AlbumDetail, AirsonicError> {
        let inner = self.get("getAlbum", &[("id", id)]).await?;
        parse_album_detail(&inner)
    }

    /// `getGenres` — the server's genre list (AIR-13 genre tiles).
    ///
    /// # Errors
    /// Transport / API / parse failures.
    pub async fn get_genres(&self) -> Result<Vec<Genre>, AirsonicError> {
        let inner = self.get("getGenres", &[]).await?;
        Ok(parse_genres(&inner))
    }

    /// `getArtist` — one artist's albums (the artist browse page). Fixes the
    /// dead "click an artist" path that pushed a breadcrumb but loaded nothing.
    ///
    /// # Errors
    /// Transport / API / parse failures.
    pub async fn get_artist(&self, id: &str) -> Result<Vec<Album>, AirsonicError> {
        let inner = self.get("getArtist", &[("id", id)]).await?;
        Ok(parse_artist_albums(&inner))
    }

    /// `getAlbumList2?type=byGenre&genre=<g>` — the albums in one genre
    /// (the AIR-13 genre page).
    ///
    /// # Errors
    /// Transport / API / parse failures.
    pub async fn get_albums_by_genre(
        &self,
        genre: &str,
        size: u32,
    ) -> Result<Vec<Album>, AirsonicError> {
        let size = size.to_string();
        let inner = self
            .get(
                "getAlbumList2",
                &[("type", "byGenre"), ("genre", genre), ("size", &size)],
            )
            .await?;
        Ok(parse_album_list2(&inner))
    }

    /// Fetch raw cover-art bytes for a `coverArt` id (`getCoverArt` returns
    /// the image binary, not a JSON envelope). Powers the AIR-16
    /// dominant-colour pass.
    ///
    /// # Errors
    /// Transport / HTTP-status failures.
    pub async fn get_cover_art_bytes(&self, cover_id: &str) -> Result<Vec<u8>, AirsonicError> {
        let bytes = self.fetch_cover_bytes(cover_id).await?;
        // A capped server answers getCoverArt with HTTP 200 but an error-30 JSON
        // envelope INSTEAD of image bytes (no art renders). The byte path skips
        // the JSON negotiation in `get`, so detect that envelope here, negotiate
        // the version down from it (persisting for the stream path too), and
        // refetch ONCE at the working version.
        if let Some(server_v) = error30_server_version(&bytes) {
            let target = min_version(&server_v, API_VERSION);
            let changed = self
                .version
                .lock()
                .map(|mut cur| {
                    if *cur != target {
                        *cur = target.clone();
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            if changed {
                write_persisted_version(&target);
                return self.fetch_cover_bytes(cover_id).await;
            }
        }
        Ok(bytes)
    }

    /// Raw GET of the cover-art URL at the current `v=` (no envelope handling).
    async fn fetch_cover_bytes(&self, cover_id: &str) -> Result<Vec<u8>, AirsonicError> {
        let url = self.cover_art_url(cover_id);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| AirsonicError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(AirsonicError::Http(format!("HTTP {}", resp.status())));
        }
        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| AirsonicError::Http(e.to_string()))
    }

    /// `getPodcasts` (no id, no episodes) — the subscribed podcast channels
    /// (AIR-21 hub list).
    ///
    /// # Errors
    /// Transport / API / parse failures.
    pub async fn get_podcast_channels(&self) -> Result<Vec<PodcastChannel>, AirsonicError> {
        let inner = self
            .get("getPodcasts", &[("includeEpisodes", "false")])
            .await?;
        Ok(parse_podcast_channels(&inner))
    }

    /// `getInternetRadioStations` — the server's saved radio stations
    /// (SVC-3, resolves the H6 unbacked Radio card).
    ///
    /// # Errors
    /// Transport / API / parse failures.
    pub async fn get_internet_radio_stations(&self) -> Result<Vec<RadioStation>, AirsonicError> {
        let inner = self.get("getInternetRadioStations", &[]).await?;
        Ok(parse_radio_stations(&inner))
    }

    /// `getPodcasts?id=<channel>&includeEpisodes=true` — one channel's
    /// episodes (the AIR-21 channel page).
    ///
    /// # Errors
    /// Transport / API / parse failures.
    pub async fn get_podcast_episodes(
        &self,
        channel_id: &str,
    ) -> Result<Vec<PodcastEpisode>, AirsonicError> {
        let inner = self
            .get(
                "getPodcasts",
                &[("id", channel_id), ("includeEpisodes", "true")],
            )
            .await?;
        Ok(parse_podcast_episodes(&inner))
    }

    /// `getPlaylists` — the shared writer's playlists (AIR-4.b Playlists tile).
    /// MEDIA-6 keeps playlist reads on the same deterministic endpoint as
    /// mutations so per-instance Navidrome SQLite cannot split visible state.
    ///
    /// # Errors
    /// Transport / API / parse failures.
    pub async fn get_playlists(&self) -> Result<Vec<Playlist>, AirsonicError> {
        let inner = self
            .get_at_base(&self.playlist_writer_base_url(), "getPlaylists", &[])
            .await?;
        Ok(parse_playlists(&inner))
    }

    /// The configured server base URL (host) — for the Home server card.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// MUSIC-HOME-1 — `getScanStatus` → `(scanning, indexed_song_count)`.
    ///
    /// # Errors
    /// Transport / API / parse failures.
    pub async fn get_scan_status(&self) -> Result<(bool, u64), AirsonicError> {
        let inner = self.get("getScanStatus", &[]).await?;
        let s = inner.get("scanStatus");
        let scanning = s
            .and_then(|v| v.get("scanning"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let count = s
            .and_then(|v| v.get("count"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        Ok((scanning, count))
    }

    /// MUSIC-HOME-1 — gather the library snapshot for the Music Home page. Each
    /// sub-call is best-effort: a single failure leaves that count 0 rather than
    /// failing the whole snapshot, but if the server is unreachable (`ping`
    /// fails) `reachable` is false and the rest is skipped. Real Airsonic data
    /// only — no placeholders.
    pub async fn library_stats(&self) -> LibraryStats {
        let mut st = LibraryStats {
            host: self.base_url.clone(),
            version: self.api_version(),
            ..LibraryStats::default()
        };
        match self.ping().await {
            Ok(v) => {
                st.reachable = true;
                if !v.is_empty() {
                    st.version = v;
                }
            }
            Err(_) => return st, // server down → connection-down card
        }
        if let Ok((scanning, songs)) = self.get_scan_status().await {
            st.scanning = scanning;
            st.songs = songs;
        }
        if let Ok(artists) = self.get_artists().await {
            st.artists = artists.len() as u64;
            st.albums = artists.iter().map(|a| u64::from(a.album_count)).sum();
        }
        st.genres = self.get_genres().await.map(|g| g.len() as u64).unwrap_or(0);
        st.playlists = self
            .get_playlists()
            .await
            .map(|p| p.len() as u64)
            .unwrap_or(0);
        st.radio = self
            .get_internet_radio_stations()
            .await
            .map(|r| r.len() as u64)
            .unwrap_or(0);
        st.podcasts = self
            .get_podcast_channels()
            .await
            .map(|p| p.len() as u64)
            .unwrap_or(0);
        st
    }

    /// `getPlaylist?id=` — one playlist's ordered songs. The GUI enqueues
    /// these to play the playlist on click (AIR-4.b, landed with its
    /// consumer — never a dead client method per §0.12).
    ///
    /// # Errors
    /// Transport / API / parse failures.
    pub async fn get_playlist(&self, id: &str) -> Result<Vec<Song>, AirsonicError> {
        let inner = self
            .get_at_base(
                &self.playlist_writer_base_url(),
                "getPlaylist",
                &[("id", id)],
            )
            .await?;
        Ok(parse_playlist_entries(&inner))
    }

    /// MUSIC-RFX-3 — `createPlaylist?name=&songId=…` — create a playlist with an
    /// optional initial track set (repeated `songId`). Returns once the server
    /// acks `status: ok`.
    ///
    /// # Errors
    /// Transport / API / parse failures.
    pub async fn create_playlist(
        &self,
        name: &str,
        song_ids: &[String],
    ) -> Result<(), AirsonicError> {
        let mut extra: Vec<(&str, &str)> = vec![("name", name)];
        extra.extend(song_ids.iter().map(|s| ("songId", s.as_str())));
        self.get_at_base(&self.playlist_writer_base_url(), "createPlaylist", &extra)
            .await
            .map(|_| ())
    }

    /// MUSIC-RFX-3 — `updatePlaylist?playlistId=` — rename (`name`), add tracks
    /// (`songIdToAdd`), and/or remove tracks by position (`songIndexToRemove`).
    ///
    /// # Errors
    /// Transport / API / parse failures.
    pub async fn update_playlist(
        &self,
        id: &str,
        name: Option<&str>,
        add: &[String],
        remove_indices: &[String],
    ) -> Result<(), AirsonicError> {
        let mut extra: Vec<(&str, &str)> = vec![("playlistId", id)];
        if let Some(n) = name {
            extra.push(("name", n));
        }
        extra.extend(add.iter().map(|s| ("songIdToAdd", s.as_str())));
        extra.extend(
            remove_indices
                .iter()
                .map(|s| ("songIndexToRemove", s.as_str())),
        );
        self.get_at_base(&self.playlist_writer_base_url(), "updatePlaylist", &extra)
            .await
            .map(|_| ())
    }

    /// MUSIC-RFX-6b — reorder a playlist **in place**, preserving its id.
    /// Subsonic/Airsonic has no reorder endpoint and `updatePlaylist` only
    /// appends (`songIdToAdd`) / removes-by-index (`songIndexToRemove`), so a
    /// reorder is one `updatePlaylist` that removes every current index and
    /// re-adds the tracks in `ordered_ids` order. Airsonic + Navidrome both
    /// apply removals before additions, so the net result is exactly
    /// `ordered_ids`. Done in a single request so a mid-way failure leaves the
    /// playlist untouched — no destructive delete+recreate (which would mint a
    /// new id and risk loss on partial failure). The current length is read
    /// live so the remove set matches the server, not a stale client count.
    ///
    /// `ordered_ids` is the full track set rearranged; any id it omits is
    /// dropped (the caller sends the complete reordered list).
    ///
    /// # Errors
    /// Transport / API / parse failures.
    pub async fn reorder_playlist(
        &self,
        id: &str,
        ordered_ids: &[String],
    ) -> Result<(), AirsonicError> {
        let current = self.get_playlist(id).await?;
        let remove: Vec<String> = (0..current.len()).map(|i| i.to_string()).collect();
        self.update_playlist(id, None, ordered_ids, &remove).await
    }

    /// MUSIC-RFX-3 — `deletePlaylist?id=` — delete a playlist.
    ///
    /// # Errors
    /// Transport / API / parse failures.
    pub async fn delete_playlist(&self, id: &str) -> Result<(), AirsonicError> {
        self.get_at_base(
            &self.playlist_writer_base_url(),
            "deletePlaylist",
            &[("id", id)],
        )
        .await
        .map(|_| ())
    }

    /// `getLyricsBySongId` (OpenSubsonic) — lyrics for a song, flattened to
    /// plain lines. Empty when the server has none or lacks the extension
    /// (the GUI shows a fallback). AIR-4.b endpoint, lands with AIR-15.b.4.
    ///
    /// # Errors
    /// Transport / API / parse failures.
    pub async fn get_lyrics_by_song_id(&self, id: &str) -> Result<Vec<String>, AirsonicError> {
        let inner = self.get("getLyricsBySongId", &[("id", id)]).await?;
        Ok(parse_lyrics(&inner))
    }
}

// ───────────────────────── pure parse helpers ─────────────────────────

/// Unwrap the `{"subsonic-response": {...}}` envelope: returns the inner
/// object on `status == "ok"`, an [`AirsonicError::Api`] on
/// `status == "failed"`, or [`AirsonicError::Parse`] on a malformed body.
fn unwrap_envelope(body: &Value) -> Result<Value, AirsonicError> {
    let inner = body
        .get("subsonic-response")
        .ok_or_else(|| AirsonicError::Parse("missing subsonic-response".into()))?;
    match inner.get("status").and_then(Value::as_str) {
        Some("ok") => Ok(inner.clone()),
        Some("failed") => {
            let err = inner.get("error");
            let code = err
                .and_then(|e| e.get("code"))
                .and_then(Value::as_i64)
                .unwrap_or(-1);
            let message = err
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_string();
            Err(AirsonicError::Api { code, message })
        }
        other => Err(AirsonicError::Parse(format!(
            "unexpected status: {other:?}"
        ))),
    }
}

/// Flatten `getArtists` → `artists.index[].artist[]` into `Vec<Artist>`.
#[must_use]
pub fn parse_artists(inner: &Value) -> Vec<Artist> {
    inner
        .get("artists")
        .and_then(|a| a.get("index"))
        .and_then(Value::as_array)
        .map(|indexes| {
            indexes
                .iter()
                .filter_map(|idx| idx.get("artist").and_then(Value::as_array))
                .flatten()
                .filter_map(|a| serde_json::from_value(a.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Parse `getAlbumList2` → `albumList2.album[]`.
#[must_use]
pub fn parse_album_list2(inner: &Value) -> Vec<Album> {
    inner
        .get("albumList2")
        .and_then(|a| a.get("album"))
        .and_then(Value::as_array)
        .map(|albums| {
            albums
                .iter()
                .filter_map(|a| serde_json::from_value(a.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// MUSIC-HOME-3 — parse `getStarred2` → `starred2.album[]` into [`Album`] rows.
#[must_use]
pub fn parse_starred_albums(inner: &Value) -> Vec<Album> {
    inner
        .get("starred2")
        .and_then(|s| s.get("album"))
        .and_then(Value::as_array)
        .map(|albums| {
            albums
                .iter()
                .filter_map(|a| serde_json::from_value(a.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a `getArtist` reply's `artist.album[]` into [`Album`] rows.
#[must_use]
pub fn parse_artist_albums(inner: &Value) -> Vec<Album> {
    inner
        .get("artist")
        .and_then(|a| a.get("album"))
        .and_then(Value::as_array)
        .map(|albums| {
            albums
                .iter()
                .filter_map(|a| serde_json::from_value(a.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// One playlist row (AIR-4.b Playlists tile). Subsonic `getPlaylists`
/// returns `playlists.playlist[]`; only id + name drive the grid (extra
/// server fields are ignored on deserialize).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Playlist {
    /// Subsonic playlist id — used as the `id=` param for `getPlaylist`.
    pub id: String,
    /// Display name of the playlist.
    pub name: String,
}

/// Parse `getPlaylists` → `playlists.playlist[]`.
#[must_use]
pub fn parse_playlists(inner: &Value) -> Vec<Playlist> {
    inner
        .get("playlists")
        .and_then(|p| p.get("playlist"))
        .and_then(Value::as_array)
        .map(|pls| {
            pls.iter()
                .filter_map(|p| serde_json::from_value(p.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Parse `getPlaylist` → `playlist.entry[]` (the playlist's ordered songs).
#[must_use]
pub fn parse_playlist_entries(inner: &Value) -> Vec<Song> {
    inner
        .get("playlist")
        .and_then(|p| p.get("entry"))
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| serde_json::from_value(e.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a `getLyricsBySongId` reply into plain lines: the OpenSubsonic
/// `lyricsList.structuredLyrics[].line[].value` shape, falling back to the
/// classic `lyrics.value` (newline-split). Empty when neither is present.
#[must_use]
pub fn parse_lyrics(inner: &Value) -> Vec<String> {
    if let Some(structured) = inner
        .get("lyricsList")
        .and_then(|l| l.get("structuredLyrics"))
        .and_then(Value::as_array)
    {
        for entry in structured {
            if let Some(lines) = entry.get("line").and_then(Value::as_array) {
                let out: Vec<String> = lines
                    .iter()
                    .filter_map(|l| l.get("value").and_then(Value::as_str).map(str::to_string))
                    .collect();
                if !out.is_empty() {
                    return out;
                }
            }
        }
    }
    if let Some(val) = inner
        .get("lyrics")
        .and_then(|l| l.get("value"))
        .and_then(Value::as_str)
    {
        return val.lines().map(str::to_string).collect();
    }
    Vec::new()
}

/// Parse `search3` → `searchResult3.{artist,album,song}[]`.
#[must_use]
pub fn parse_search3(inner: &Value) -> SearchResult3 {
    let sr = inner.get("searchResult3");
    let arr = |key: &str| -> Vec<Value> {
        sr.and_then(|s| s.get(key))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    SearchResult3 {
        artists: arr("artist")
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect(),
        albums: arr("album")
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect(),
        songs: arr("song")
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect(),
    }
}

/// Parse `getSong` → the inner `song` object.
///
/// # Errors
/// [`AirsonicError::Parse`] when the `song` object is missing or malformed.
pub fn parse_song(inner: &Value) -> Result<Song, AirsonicError> {
    inner
        .get("song")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| AirsonicError::Parse(e.to_string()))?
        .ok_or_else(|| AirsonicError::Parse("getSong: missing song object".into()))
}

/// Parse `getAlbum` → the `album` object's metadata + its nested `song[]`
/// track list.
///
/// # Errors
/// [`AirsonicError::Parse`] when the `album` object is missing or malformed.
pub fn parse_album_detail(inner: &Value) -> Result<AlbumDetail, AirsonicError> {
    let album_obj = inner
        .get("album")
        .ok_or_else(|| AirsonicError::Parse("getAlbum: missing album object".into()))?;
    let album: Album = serde_json::from_value(album_obj.clone())
        .map_err(|e| AirsonicError::Parse(e.to_string()))?;
    let songs = album_obj
        .get("song")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| serde_json::from_value(v.clone()).ok())
                .collect()
        })
        .unwrap_or_default();
    Ok(AlbumDetail { album, songs })
}

/// Parse `getGenres` → `genres.genre[]`.
#[must_use]
pub fn parse_genres(inner: &Value) -> Vec<Genre> {
    inner
        .get("genres")
        .and_then(|g| g.get("genre"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|g| serde_json::from_value(g.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Parse `getInternetRadioStations`'s
/// `internetRadioStations.internetRadioStation[]` array (SVC-3).
#[must_use]
pub fn parse_radio_stations(inner: &Value) -> Vec<RadioStation> {
    inner
        .get("internetRadioStations")
        .and_then(|p| p.get("internetRadioStation"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    let id = c.get("id").and_then(Value::as_str)?;
                    let stream_url = c.get("streamUrl").and_then(Value::as_str)?;
                    let name = c.get("name").and_then(Value::as_str).unwrap_or(id);
                    Some(RadioStation {
                        id: id.to_string(),
                        name: name.to_string(),
                        stream_url: stream_url.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse `getPodcasts` → `podcasts.channel[]` (id + title).
#[must_use]
pub fn parse_podcast_channels(inner: &Value) -> Vec<PodcastChannel> {
    inner
        .get("podcasts")
        .and_then(|p| p.get("channel"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    let id = c.get("id").and_then(Value::as_str)?;
                    let title = c.get("title").and_then(Value::as_str).unwrap_or(id);
                    Some(PodcastChannel {
                        id: id.to_string(),
                        title: title.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse `getPodcasts?id=<channel>` → the first channel's `episode[]`. Each
/// episode's playable id is its `streamId` (the media to stream), falling
/// back to its `id`.
#[must_use]
pub fn parse_podcast_episodes(inner: &Value) -> Vec<PodcastEpisode> {
    inner
        .get("podcasts")
        .and_then(|p| p.get("channel"))
        .and_then(Value::as_array)
        .and_then(|chans| chans.first())
        .and_then(|c| c.get("episode"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    let id = e
                        .get("streamId")
                        .or_else(|| e.get("id"))
                        .and_then(Value::as_str)?;
                    let title = e.get("title").and_then(Value::as_str).unwrap_or(id);
                    Some(PodcastEpisode {
                        id: id.to_string(),
                        title: title.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Minimal percent-encoding for query values (space + the reserved
/// chars that break a Subsonic query). Avoids a `url`/`percent-encoding`
/// dep for the handful of chars that actually occur in ids + search
/// terms.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn error30_envelope_detected_in_raw_bytes() {
        // A capped server answers a raw-byte endpoint (cover art / stream) with
        // an error-30 JSON envelope instead of media — detect it + read the
        // server's version so the byte path can negotiate down.
        let env = br#"{"subsonic-response":{"status":"failed","version":"1.15.0",
            "error":{"code":30,"message":"Server must upgrade."}}}"#;
        assert_eq!(error30_server_version(env), Some("1.15.0".to_string()));
        // Omitted version → conservative fallback.
        let no_ver = br#"{"subsonic-response":{"status":"failed",
            "error":{"code":30}}}"#;
        assert_eq!(
            error30_server_version(no_ver),
            Some(FALLBACK_API_VERSION.to_string())
        );
    }

    #[test]
    fn error30_detector_passes_through_real_media() {
        // Real image bytes (PNG magic) are not a JSON envelope → no negotiation.
        let png = [0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 1, 2, 3];
        assert_eq!(error30_server_version(&png), None);
        // A *successful* JSON envelope (different endpoint) must not trigger it.
        let ok = br#"{"subsonic-response":{"status":"ok","version":"1.15.0"}}"#;
        assert_eq!(error30_server_version(ok), None);
        // A non-30 error (e.g. wrong password, code 40) must not trigger it.
        let other = br#"{"subsonic-response":{"status":"failed","version":"1.16.1",
            "error":{"code":40,"message":"Wrong username or password."}}}"#;
        assert_eq!(error30_server_version(other), None);
    }

    #[test]
    fn persisted_version_roundtrip_rejects_too_new() {
        // read_persisted_version refuses a value newer than our ceiling (junk
        // guard): cmp against API_VERSION.
        assert_eq!(
            cmp_version("1.99.0", API_VERSION),
            std::cmp::Ordering::Greater
        );
        assert_ne!(
            cmp_version(FALLBACK_API_VERSION, API_VERSION),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn parse_lyrics_structured_and_classic() {
        let structured = json!({"lyricsList":{"structuredLyrics":[{"line":[
            {"value":"line one"},{"value":"line two"}
        ]}]}});
        assert_eq!(
            parse_lyrics(&structured),
            vec!["line one".to_string(), "line two".to_string()]
        );
        let classic = json!({"lyrics":{"value":"a\nb\nc"}});
        assert_eq!(
            parse_lyrics(&classic),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert!(parse_lyrics(&json!({"nope":1})).is_empty());
    }
    use serde_json::json;

    #[test]
    fn auth_token_matches_subsonic_vector() {
        // The canonical Subsonic doc example: password "sesame",
        // salt "c19b2d" → token "26719a1196d2a940705a59634eb18eab".
        assert_eq!(
            auth_token("sesame", "c19b2d"),
            "26719a1196d2a940705a59634eb18eab"
        );
    }

    #[test]
    fn query_params_carry_every_auth_field() {
        let c = Client::with_salt("http://h:4040", "alice", "pw", "abc");
        let p = c.query_params();
        for key in ["u", "t", "s", "v", "c", "f"] {
            assert!(p.iter().any(|(k, _)| k == key), "missing {key}");
        }
        assert_eq!(p.iter().find(|(k, _)| k == "u").unwrap().1, "alice");
        assert_eq!(p.iter().find(|(k, _)| k == "s").unwrap().1, "abc");
        assert_eq!(p.iter().find(|(k, _)| k == "f").unwrap().1, "json");
        // Default v= is the ceiling until a server negotiates it down.
        assert_eq!(p.iter().find(|(k, _)| k == "v").unwrap().1, API_VERSION);
    }

    #[test]
    fn version_compare_and_min() {
        use std::cmp::Ordering;
        assert_eq!(cmp_version("1.16.1", "1.13.0"), Ordering::Greater);
        assert_eq!(cmp_version("1.13.0", "1.16.1"), Ordering::Less);
        assert_eq!(cmp_version("1.16.1", "1.16.1"), Ordering::Equal);
        assert_eq!(cmp_version("1.9.0", "1.10.0"), Ordering::Less); // numeric, not lexical
                                                                    // min_version never exceeds either input.
        assert_eq!(min_version("1.16.1", "1.13.0"), "1.13.0");
        assert_eq!(min_version("1.13.0", "1.16.1"), "1.13.0");
        // A server that reports a newer version than our ceiling is clamped to us.
        assert_eq!(min_version("1.99.0", API_VERSION), API_VERSION);
    }

    #[test]
    fn negotiated_version_is_advertised_after_downgrade() {
        // Simulate the negotiation effect: lowering the client version flows into
        // the next request's v= (what get() does on error 30).
        let c = Client::with_salt("http://h:4040", "alice", "pw", "abc");
        assert_eq!(c.api_version(), API_VERSION);
        *c.version.lock().unwrap() = "1.13.0".to_string();
        assert_eq!(c.api_version(), "1.13.0");
        assert_eq!(
            c.query_params().iter().find(|(k, _)| k == "v").unwrap().1,
            "1.13.0"
        );
    }

    #[test]
    fn endpoint_url_shape_and_trailing_slash_trim() {
        let c = Client::with_salt("http://h:4040/", "alice", "pw", "abc");
        let u = c.endpoint_url("getArtists", &[]);
        assert!(u.starts_with("http://h:4040/rest/getArtists?"));
        // No double slash from the trimmed base.
        assert!(!u.contains(":4040//rest"));
    }

    #[test]
    fn playlist_writer_url_uses_single_writer_alias_for_music_mesh() {
        let _guard = env_lock();
        std::env::remove_var(MUSIC_WRITER_URL_ENV);
        let c = Client::with_salt("http://music.mesh:4533", "alice", "pw", "abc");
        assert_eq!(
            c.playlist_writer_base_url(),
            "http://music-writer.mesh:4533"
        );
        let u = c.playlist_writer_endpoint_url("createPlaylist", &[("name", "Roadtrip")]);
        assert!(u.starts_with("http://music-writer.mesh:4533/rest/createPlaylist?"));
        assert!(u.contains("name=Roadtrip"));
        let list = c.playlist_writer_endpoint_url("getPlaylists", &[]);
        assert!(list.starts_with("http://music-writer.mesh:4533/rest/getPlaylists?"));
    }

    #[test]
    fn playlist_writer_url_leaves_manual_servers_alone_without_override() {
        let _guard = env_lock();
        std::env::remove_var(MUSIC_WRITER_URL_ENV);
        let c = Client::with_salt("http://airsonic.local:4040", "alice", "pw", "abc");
        assert_eq!(c.playlist_writer_base_url(), "http://airsonic.local:4040");
    }

    #[test]
    fn playlist_writer_url_accepts_operator_override() {
        let _guard = env_lock();
        std::env::set_var(MUSIC_WRITER_URL_ENV, "http://writer.example:4533/");
        let c = Client::with_salt("http://music.mesh:4533", "alice", "pw", "abc");
        assert_eq!(c.playlist_writer_base_url(), "http://writer.example:4533");
        std::env::remove_var(MUSIC_WRITER_URL_ENV);
    }

    #[test]
    fn playlist_write_endpoints_repeat_params() {
        // MUSIC-RFX-3 — createPlaylist carries name + every songId; updatePlaylist
        // carries playlistId + songIdToAdd/songIndexToRemove; deletePlaylist the id.
        let c = Client::with_salt("http://h:4040", "alice", "pw", "abc");
        let create = c.endpoint_url(
            "createPlaylist",
            &[("name", "Roadtrip"), ("songId", "s1"), ("songId", "s2")],
        );
        assert!(create.starts_with("http://h:4040/rest/createPlaylist?"));
        assert!(create.contains("name=Roadtrip"));
        assert_eq!(create.matches("songId=").count(), 2);
        assert!(create.contains("songId=s1") && create.contains("songId=s2"));

        let update = c.endpoint_url(
            "updatePlaylist",
            &[
                ("playlistId", "pl-9"),
                ("songIdToAdd", "s3"),
                ("songIndexToRemove", "0"),
            ],
        );
        assert!(update.contains("playlistId=pl-9"));
        assert!(update.contains("songIdToAdd=s3"));
        assert!(update.contains("songIndexToRemove=0"));

        let del = c.endpoint_url("deletePlaylist", &[("id", "pl-9")]);
        assert!(del.starts_with("http://h:4040/rest/deletePlaylist?"));
        assert!(del.contains("id=pl-9"));
    }

    #[test]
    fn stream_and_cover_urls_carry_id() {
        let c = Client::with_salt("http://h:4040", "alice", "pw", "abc");
        assert!(c.stream_url("song-7").contains("/rest/stream?"));
        assert!(c.stream_url("song-7").contains("id=song-7"));
        assert!(c.cover_art_url("al-3").contains("/rest/getCoverArt?"));
        assert!(c.cover_art_url("al-3").contains("id=al-3"));
    }

    #[test]
    fn urlencode_escapes_spaces_and_reserved() {
        assert_eq!(urlencode("miles davis"), "miles%20davis");
        assert_eq!(urlencode("a/b&c"), "a%2Fb%26c");
        assert_eq!(urlencode("plain-id_1.2~"), "plain-id_1.2~");
    }

    #[test]
    fn parse_song_reads_song_object() {
        let inner = json!({"song": {
            "id": "tr-9", "title": "So What", "album": "Kind of Blue",
            "artist": "Miles Davis", "duration": 545, "track": 1,
            "suffix": "flac", "coverArt": "al-7"
        }});
        let s = parse_song(&inner).unwrap();
        assert_eq!(s.id, "tr-9");
        assert_eq!(s.title, "So What");
        assert_eq!(s.artist, "Miles Davis");
        assert_eq!(s.duration, 545);
        assert_eq!(s.cover_art, "al-7");
        // A missing `song` object is a parse error, not a panic.
        assert!(parse_song(&json!({"nope": 1})).is_err());
    }

    #[test]
    fn parse_album_detail_reads_meta_and_tracks() {
        let inner = json!({"album": {
            "id": "al1", "name": "Kind of Blue", "artist": "Miles Davis",
            "artistId": "ar1", "songCount": 5, "coverArt": "al1", "year": 1959,
            "song": [
                {"id": "s1", "title": "So What", "duration": 545, "track": 1, "suffix": "flac"},
                {"id": "s2", "title": "Freddie Freeloader", "duration": 586, "track": 2}
            ]
        }});
        let d = parse_album_detail(&inner).unwrap();
        assert_eq!(d.album.name, "Kind of Blue");
        assert_eq!(d.album.year, Some(1959));
        assert_eq!(d.songs.len(), 2);
        assert_eq!(d.songs[0].title, "So What");
        assert_eq!(d.songs[1].track, Some(2));
        // A missing `album` object is a parse error.
        assert!(parse_album_detail(&json!({"nope": 1})).is_err());
    }

    #[test]
    fn parse_genres_reads_list() {
        let inner = json!({"genres": {"genre": [
            {"value": "Jazz", "albumCount": 12, "songCount": 140},
            {"value": "Rock", "albumCount": 30}
        ]}});
        let g = parse_genres(&inner);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].name, "Jazz");
        assert_eq!(g[0].album_count, 12);
        assert_eq!(g[1].name, "Rock");
        assert!(parse_genres(&json!({"nope": 1})).is_empty());
    }

    #[test]
    fn parse_podcasts_channels_and_episodes() {
        let chans = json!({"podcasts": {"channel": [
            {"id": "c1", "title": "Rust Talks"},
            {"id": "c2", "title": "Mesh Weekly"}
        ]}});
        let c = parse_podcast_channels(&chans);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].id, "c1");
        assert_eq!(c[0].title, "Rust Talks");
        // Episodes: streamId is the playable id; falls back to id.
        let eps = json!({"podcasts": {"channel": [{"id": "c1", "episode": [
            {"id": "e1", "streamId": "s1", "title": "Ep 1"},
            {"id": "e2", "title": "Ep 2"}
        ]}]}});
        let e = parse_podcast_episodes(&eps);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].id, "s1"); // streamId wins
        assert_eq!(e[1].id, "e2"); // falls back to id
        assert!(parse_podcast_channels(&json!({"nope": 1})).is_empty());
        assert!(parse_podcast_episodes(&json!({"nope": 1})).is_empty());
    }

    #[test]
    fn unwrap_envelope_ok_failed_and_malformed() {
        let ok = json!({"subsonic-response": {"status": "ok", "version": "1.16.1"}});
        assert!(unwrap_envelope(&ok).is_ok());

        let failed = json!({"subsonic-response": {"status": "failed",
            "error": {"code": 40, "message": "Wrong username or password"}}});
        assert_eq!(
            unwrap_envelope(&failed),
            Err(AirsonicError::Api {
                code: 40,
                message: "Wrong username or password".into()
            })
        );

        let malformed = json!({"nope": true});
        assert!(matches!(
            unwrap_envelope(&malformed),
            Err(AirsonicError::Parse(_))
        ));
    }

    #[test]
    fn parse_artists_flattens_index() {
        let inner = json!({"artists": {"index": [
            {"name": "A", "artist": [
                {"id": "1", "name": "ABBA", "albumCount": 9},
                {"id": "2", "name": "Air", "albumCount": 4}
            ]},
            {"name": "M", "artist": [
                {"id": "3", "name": "Miles Davis", "albumCount": 50}
            ]}
        ]}});
        let artists = parse_artists(&inner);
        assert_eq!(artists.len(), 3);
        assert_eq!(artists[0].name, "ABBA");
        assert_eq!(artists[2].album_count, 50);
    }

    #[test]
    fn parse_album_list2_reads_albums() {
        let inner = json!({"albumList2": {"album": [
            {"id": "a1", "name": "Moon Safari", "artist": "Air", "artistId": "2",
             "songCount": 10, "coverArt": "al-a1", "year": 1998}
        ]}});
        let albums = parse_album_list2(&inner);
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].name, "Moon Safari");
        assert_eq!(albums[0].year, Some(1998));
        assert_eq!(albums[0].cover_art, "al-a1");
    }

    #[test]
    fn parse_artist_albums_reads_the_artist_album_list() {
        // getArtist reply shape: {artist: {id, name, album: [...]}}.
        let inner = json!({"artist": {"id": "2", "name": "Air", "album": [
            {"id": "a1", "name": "Moon Safari", "artist": "Air", "artistId": "2"},
            {"id": "a2", "name": "Talkie Walkie", "artist": "Air", "artistId": "2"}
        ]}});
        let albums = parse_artist_albums(&inner);
        assert_eq!(albums.len(), 2);
        assert_eq!(albums[0].name, "Moon Safari");
        assert_eq!(albums[1].name, "Talkie Walkie");
        // empty / missing artist → no albums, no panic.
        assert!(parse_artist_albums(&json!({})).is_empty());
    }

    #[test]
    fn parse_playlists_reads_roster() {
        let inner = json!({"playlists": {"playlist": [
            {"id": "pl1", "name": "Roadtrip", "songCount": 42},
            {"id": "pl2", "name": "Focus"}
        ]}});
        let pls = parse_playlists(&inner);
        assert_eq!(pls.len(), 2);
        assert_eq!(pls[0].id, "pl1");
        assert_eq!(pls[0].name, "Roadtrip");
        assert_eq!(pls[1].name, "Focus");
    }

    #[test]
    fn parse_playlist_entries_reads_songs() {
        let inner = json!({"playlist": {"id": "pl1", "name": "Roadtrip", "entry": [
            {"id": "s1", "title": "Intro"},
            {"id": "s2", "title": "Drive", "suffix": "flac"}
        ]}});
        let songs = parse_playlist_entries(&inner);
        assert_eq!(songs.len(), 2);
        assert_eq!(songs[0].id, "s1");
        assert_eq!(songs[1].title, "Drive");
    }

    #[test]
    fn parse_search3_three_sections() {
        let inner = json!({"searchResult3": {
            "artist": [{"id": "2", "name": "Air", "albumCount": 4}],
            "album":  [{"id": "a1", "name": "Moon Safari"}],
            "song":   [{"id": "s1", "title": "La Femme d'Argent", "duration": 429, "suffix": "flac"}]
        }});
        let r = parse_search3(&inner);
        assert_eq!(r.artists.len(), 1);
        assert_eq!(r.albums.len(), 1);
        assert_eq!(r.songs.len(), 1);
        assert_eq!(r.songs[0].duration, 429);
        assert_eq!(r.songs[0].suffix, "flac");
    }

    #[test]
    fn parse_helpers_tolerate_missing_sections() {
        let empty = json!({"status": "ok"});
        assert!(parse_artists(&empty).is_empty());
        assert!(parse_album_list2(&empty).is_empty());
        assert_eq!(parse_search3(&empty), SearchResult3::default());
    }
}
