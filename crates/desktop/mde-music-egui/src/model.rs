//! The render-agnostic view-model for the music surface (E12-5).
//!
//! This module holds **no egui or audio types** — only the data the UI renders
//! and the small state machine that advances it. The worker thread (network +
//! audio) sends [`Update`]s in; [`MusicState::apply`] folds them into the state;
//! the egui view reads the state and emits [`Command`]s back. Because it is free
//! of a GPU and a sound device, the whole thing is unit-tested below.
//!
//! It reuses `mde-musicd`'s own types directly (§6 glue, not reimplementation):
//! the [`Album`] / [`Song`] rows the Airsonic client already parses, the
//! [`Client`] that builds the authenticated stream URL, and the engine's
//! [`SourceCodec`] classifier.

use mde_musicd::airsonic::{Album, Client, SearchResult3, Song};
use mde_musicd::domain::{ContentKind, ContentRef, MusicWorkspaceSnapshotV1};
use mde_musicd::engine::SourceCodec;

/// A music server visible to one desktop seat. Higher operator priority wins;
/// latency is only a tie-break.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatServer {
    /// Stable seat/node identity shown to the operator.
    pub seat: String,
    /// HTTP or HTTPS Airsonic base URL.
    pub url: String,
    /// Operator-assigned preference; larger values win.
    pub operator_priority: u32,
    /// Last measured ping latency, when reachable.
    pub latency_ms: Option<u32>,
}

impl SeatServer {
    /// Construct an unprobed candidate at the default priority.
    #[must_use]
    pub fn new(seat: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            seat: seat.into(),
            url: url.into(),
            operator_priority: 0,
            latency_ms: None,
        }
    }
}

/// Choose the default seat deterministically: operator priority first, then
/// measured latency, then identity/URL for a stable final tie-break.
#[must_use]
pub fn select_default_server(servers: &[SeatServer]) -> Option<usize> {
    servers
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| {
            a.operator_priority
                .cmp(&b.operator_priority)
                .then_with(|| match (b.latency_ms, a.latency_ms) {
                    (Some(b), Some(a)) => b.cmp(&a),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                })
                .then_with(|| b.seat.cmp(&a.seat))
                .then_with(|| b.url.cmp(&a.url))
        })
        .map(|(index, _)| index)
}

/// A proposed route change that requires explicit operator approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailoverRequest {
    /// Current seat identity.
    pub from: String,
    /// Candidate seat identity.
    pub to: String,
    /// Human-readable reason for the proposal.
    pub reason: String,
}

/// The lifecycle of a value fetched asynchronously from the Airsonic client:
/// untouched, in flight, loaded, or failed.
///
/// Generic so the album library and an album's track list share one honest "not
/// real data yet" representation instead of an empty `Vec` masquerading as a
/// loaded-but-empty result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Fetch<T> {
    /// Not requested yet.
    #[default]
    Idle,
    /// A request is in flight on the worker thread.
    Loading,
    /// Loaded successfully.
    Ready(T),
    /// Loaded from the last successful response while the server is offline.
    Cached(T),
    /// The request failed; carries a human-readable reason to surface.
    Failed(String),
}

/// An album opened for browsing: its metadata plus its lazily-fetched, ordered
/// track list.
#[derive(Debug, Clone)]
pub struct OpenAlbum {
    /// The album whose tracks are shown.
    pub album: Album,
    /// The album's ordered tracks (fetched via the client's `getAlbum`).
    pub tracks: Fetch<Vec<Song>>,
}

/// The complete render-agnostic state of the music surface.
///
/// Holds the album library, the album currently opened for its track list (if
/// any), the playback transport mirror, and a transient error banner. The egui
/// view renders this; the worker drives it forward through [`Update`]s.
#[derive(Debug, Default)]
pub struct MusicState {
    /// The album library — the `getAlbumList2` listing.
    pub albums: Fetch<Vec<Album>>,
    /// The album currently opened for browsing, if any.
    pub open_album: Option<OpenAlbum>,
    /// The track the engine is currently playing, if any.
    pub now_playing: Option<Song>,
    /// Whether the engine is in the playing (not paused) state.
    pub playing: bool,
    /// The playhead position within the now-playing track, in milliseconds
    /// (`0` when nothing plays). Driven by the worker's live poll of the engine,
    /// so the transport shows real elapsed time instead of a value frozen at the
    /// moment playback began.
    pub position_ms: u64,
    /// Daemon-projected output volume in thousandths. `None` means the
    /// standalone worker has not supplied a projection, so the control starts
    /// at its honest default rather than claiming a daemon value.
    pub volume_milli: Option<u16>,
    /// A transient playback/engine error to surface (e.g. no audio device).
    pub error: Option<String>,
    /// The seat selected by the worker after candidate probing.
    pub selected_server: Option<SeatServer>,
    /// Candidates known to this seat.
    pub servers: Vec<SeatServer>,
    /// A failover proposal waiting for explicit operator approval.
    pub failover: Option<FailoverRequest>,
    /// True when visible data is being retained from an offline response.
    pub offline: bool,
    /// Last track known to be playable, retained for offline transport state.
    pub cached_track: Option<Song>,
    /// Starred albums from the real provider endpoint.
    pub starred: Fetch<Vec<Album>>,
    /// The latest debounced search result.
    pub search: Fetch<SearchResult3>,
    /// Generation of the latest issued search request.
    pub search_generation: u64,
    /// Query associated with the latest search request.
    pub search_query: String,
    /// Latest validated daemon workspace snapshot, when the retained Bus lane
    /// is available. This is a read-only projection used by storage/downloads.
    pub workspace: Option<MusicWorkspaceSnapshotV1>,
    /// Revision of [`workspace`], used to ignore stale retained rows.
    pub workspace_revision: u64,
}

/// A result message the worker thread sends back to the UI, folded into the
/// [`MusicState`] by [`MusicState::apply`].
#[derive(Debug)]
pub enum Update {
    /// The album library finished loading (or failed).
    Library(Result<Vec<Album>, String>),
    /// Starred albums finished loading.
    Starred(Result<Vec<Album>, String>),
    /// A progressive search result. Older generations are ignored.
    Search {
        /// Request generation.
        generation: u64,
        /// Query sent to the source.
        query: String,
        /// Search response.
        result: Result<SearchResult3, String>,
    },
    /// The worker selected a seat after probing candidates.
    ServerSelected(SeatServer),
    /// A route change is available but requires explicit approval.
    FailoverPending(FailoverRequest),
    /// One album's track list finished loading (or failed). Applied only when it
    /// matches the currently-open album, so a stale reply for a since-closed
    /// album is ignored.
    Tracks {
        /// The album id the tracks belong to.
        album_id: String,
        /// The fetched tracks, or a failure reason.
        result: Result<Vec<Song>, String>,
    },
    /// Playback started for this track (clears any prior error).
    Started(Song),
    /// The play/pause state changed (`true` = playing, `false` = paused).
    Playing(bool),
    /// The playhead advanced within the current track (position in milliseconds).
    /// Emitted by the worker's poll while a track plays, so the transport shows a
    /// live elapsed time rather than a value frozen at the moment playback began.
    Progress(u64),
    /// Playback stopped and the now-playing track was cleared.
    Stopped,
    /// The current track finished on its own (the engine drained) — distinct from
    /// an operator Stop, but it clears the now-playing transport the same way.
    Ended,
    /// A playback/engine error to surface to the operator.
    Error(String),
}

/// An intent the UI sends to the worker thread.
#[derive(Debug)]
pub enum Command {
    /// Fetch the album library.
    LoadLibrary,
    /// Fetch one album's ordered track list.
    LoadAlbum(String),
    /// Play this track through the engine, replacing any current playback.
    Play(Song),
    /// Pause the engine (the buffer is kept; resume is seamless).
    Pause,
    /// Resume the engine after a pause.
    Resume,
    /// Stop the engine and clear the now-playing track.
    Stop,
    /// Select a known server explicitly for this seat.
    SelectServer(String),
    /// Approve the currently pending failover proposal.
    ApproveFailover,
    /// Reject and clear the currently pending failover proposal.
    RejectFailover,
    /// Fetch the provider's starred collection.
    LoadStarred,
    /// Search every admitted source through the worker's fan-out seam.
    Search {
        /// Debounce generation.
        generation: u64,
        /// Query text.
        query: String,
    },
    /// Seek the current finite track.
    Seek(u64),
    /// Set output volume from 0.0 to 1.0.
    SetVolume(f32),
}

impl MusicState {
    /// A fresh, idle state (nothing loaded, nothing playing).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the daemon projection only when it is valid and advances
    /// monotonically.
    pub fn apply_workspace_snapshot(&mut self, snapshot: MusicWorkspaceSnapshotV1) {
        // The retained Bus payload is daemon-owned authority, but it still
        // crosses a hostile process boundary.  Do not let malformed typed
        // content replace the last known-good projection or become a local
        // worker fallback.
        if snapshot.revision <= self.workspace_revision {
            return;
        }
        match snapshot.validate() {
            Ok(()) => {
                let now_playing = snapshot
                    .playback
                    .current
                    .as_ref()
                    .and_then(|content| Self::workspace_song(&snapshot, content));
                if let Some(open) = self.open_album.as_mut() {
                    if let Some(item) = snapshot
                        .collections
                        .iter()
                        .flat_map(|collection| collection.items.iter())
                        .find(|item| item.kind == ContentKind::Album && item.id == open.album.id)
                    {
                        open.album.cover_art = item.artwork_ref.clone().unwrap_or_default();
                    }
                }
                self.workspace_revision = snapshot.revision;
                self.playing = snapshot.playback.playing;
                self.position_ms = snapshot.playback.position_ms;
                self.volume_milli = Some(snapshot.playback.volume_milli);
                self.now_playing = now_playing;
                self.workspace = Some(snapshot);
            }
            Err(reason) => {
                self.error = Some(format!("Music daemon projection rejected: {reason}"));
            }
        }
    }

    /// Materialize the daemon's current composite identity into the legacy
    /// `Song` render model. The embedded shell does not run the compatibility
    /// worker, so without this bridge a valid daemon playback projection would
    /// leave the Music player's Now Playing strip permanently empty.
    fn workspace_song(snapshot: &MusicWorkspaceSnapshotV1, current: &ContentRef) -> Option<Song> {
        let catalog_item = snapshot
            .shelves
            .iter()
            .flat_map(|shelf| shelf.items.iter())
            .chain(
                snapshot
                    .collections
                    .iter()
                    .flat_map(|collection| collection.items.iter()),
            )
            .find(|item| {
                item.variants
                    .iter()
                    .any(|variant| variant.content == *current)
            });
        let queue_entry = snapshot
            .queue
            .iter()
            .find(|entry| entry.content == *current);
        let bookmark = snapshot
            .bookmarks
            .iter()
            .find(|entry| entry.content == *current);
        let title = catalog_item
            .map(|item| item.title.clone())
            .filter(|title| !title.trim().is_empty())
            .or_else(|| {
                bookmark
                    .map(|entry| entry.title.clone())
                    .filter(|title| !title.trim().is_empty())
            })
            .or_else(|| {
                queue_entry
                    .map(|entry| entry.title.clone())
                    .filter(|title| !title.trim().is_empty())
            })
            .unwrap_or_else(|| current.remote_id.clone());
        let duration_ms = catalog_item
            .and_then(|item| item.duration_ms)
            .or_else(|| bookmark.and_then(|entry| entry.duration_ms))
            .or(snapshot.playback.duration_ms)
            .unwrap_or_default();
        let duration = duration_ms
            .saturating_add(999)
            .checked_div(1000)
            .unwrap_or_default()
            .min(u64::from(u32::MAX)) as u32;

        Some(Song {
            id: current.remote_id.clone(),
            title,
            album: catalog_item
                .map(|item| item.parent_title.clone())
                .or_else(|| bookmark.map(|entry| entry.parent_title.clone()))
                .unwrap_or_default(),
            artist: catalog_item
                .map(|item| item.creator.clone())
                .or_else(|| bookmark.map(|entry| entry.creator.clone()))
                .unwrap_or_default(),
            duration,
            track: None,
            suffix: String::new(),
            cover_art: catalog_item
                .and_then(|item| item.artwork_ref.clone())
                .or_else(|| bookmark.and_then(|entry| entry.artwork_ref.clone()))
                .unwrap_or_default(),
        })
    }

    /// Fold a worker [`Update`] into the state.
    pub fn apply(&mut self, update: Update) {
        match update {
            Update::Library(Ok(albums)) => {
                self.offline = false;
                self.albums = Fetch::Ready(albums);
            }
            Update::Library(Err(e)) => {
                self.albums = match std::mem::replace(&mut self.albums, Fetch::Idle) {
                    Fetch::Ready(albums) | Fetch::Cached(albums) => {
                        self.offline = true;
                        Fetch::Cached(albums)
                    }
                    _ => Fetch::Failed(e),
                };
            }
            Update::Starred(result) => {
                self.starred = match result {
                    Ok(albums) => Fetch::Ready(albums),
                    Err(error) => Fetch::Failed(error),
                };
            }
            Update::Search {
                generation,
                query,
                result,
            } => {
                if generation <= self.search_generation {
                    return;
                }
                self.search_generation = generation;
                self.search_query = query;
                self.search = match result {
                    Ok(page) => Fetch::Ready(page),
                    Err(error) => Fetch::Failed(error),
                };
            }
            Update::ServerSelected(server) => {
                self.selected_server = Some(server);
                self.failover = None;
            }
            Update::FailoverPending(request) => self.failover = Some(request),
            Update::Tracks { album_id, result } => {
                // Ignore a reply for an album the operator has since navigated
                // away from — only the open album's tracks are live.
                if let Some(open) = self.open_album.as_mut() {
                    if open.album.id == album_id {
                        open.tracks = match result {
                            Ok(songs) => {
                                self.offline = false;
                                Fetch::Ready(songs)
                            }
                            Err(e) => match std::mem::replace(&mut open.tracks, Fetch::Idle) {
                                Fetch::Ready(songs) | Fetch::Cached(songs) => {
                                    self.offline = true;
                                    Fetch::Cached(songs)
                                }
                                _ => Fetch::Failed(e),
                            },
                        };
                    }
                }
            }
            Update::Started(song) => {
                self.cached_track = Some(song.clone());
                self.now_playing = Some(song);
                self.playing = true;
                self.position_ms = 0;
                self.error = None;
            }
            Update::Playing(playing) => self.playing = playing,
            Update::Progress(ms) => self.position_ms = ms,
            // A user Stop and a track finishing on its own both leave the
            // transport empty and the playhead at zero.
            Update::Stopped | Update::Ended => {
                self.now_playing = None;
                self.playing = false;
                self.position_ms = 0;
            }
            Update::Error(e) => {
                self.error = Some(e);
                self.offline = true;
            }
        }
    }

    /// Open `album` for browsing, marking its track list as in-flight. The caller
    /// then issues a [`Command::LoadAlbum`] for the album id.
    pub fn open(&mut self, album: Album) {
        self.open_album = Some(OpenAlbum {
            album,
            tracks: Fetch::Loading,
        });
    }

    /// Close the open album, returning to the library listing.
    pub fn close(&mut self) {
        self.open_album = None;
    }
}

/// The engine-ready `(stream_url, codec)` pair for a track.
///
/// The authenticated Airsonic `stream` URL the engine's decode thread fetches,
/// plus the codec hint classified from the track's file suffix. This is the glue
/// that hands a library [`Song`] to [`mde_musicd::engine::EngineHandle::play`] —
/// both halves come from `mde-musicd`, so playback is its real engine, not a
/// reimplementation.
#[must_use]
pub fn track_for_engine(client: &Client, song: &Song) -> (String, SourceCodec) {
    (
        client.stream_url(&song.id),
        SourceCodec::from_suffix(&song.suffix),
    )
}

/// Format a length (whole seconds) as `m:ss`. Takes `u64` so it renders both a
/// track's tagged duration and the engine's live playhead (a `u64` millisecond
/// count / 1000) without a truncating cast.
#[must_use]
pub fn format_duration(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// A one-line album subtitle — `artist · N tracks · year` — omitting any part the
/// server did not provide (no zero-track or empty-artist filler).
#[must_use]
pub fn album_subtitle(album: &Album) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !album.artist.trim().is_empty() {
        parts.push(album.artist.clone());
    }
    if album.song_count > 0 {
        let plural = if album.song_count == 1 { "" } else { "s" };
        parts.push(format!("{} track{plural}", album.song_count));
    }
    if let Some(year) = album.year {
        parts.push(year.to_string());
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(id: &str) -> Album {
        Album {
            id: id.to_string(),
            name: format!("Album {id}"),
            artist: "Artist".to_string(),
            artist_id: String::new(),
            song_count: 10,
            cover_art: String::new(),
            year: Some(2021),
        }
    }

    fn song(id: &str, suffix: &str, duration: u32) -> Song {
        Song {
            id: id.to_string(),
            title: format!("Track {id}"),
            album: "Album".to_string(),
            artist: "Artist".to_string(),
            duration,
            track: None,
            suffix: suffix.to_string(),
            cover_art: String::new(),
        }
    }

    #[test]
    fn fetch_defaults_to_idle() {
        assert_eq!(Fetch::<Vec<Album>>::default(), Fetch::Idle);
        assert_eq!(MusicState::new().albums, Fetch::Idle);
    }

    #[test]
    fn library_update_moves_to_ready_then_failed() {
        let mut s = MusicState::new();
        s.apply(Update::Library(Ok(vec![album("1"), album("2")])));
        assert!(matches!(&s.albums, Fetch::Ready(a) if a.len() == 2));
        // A later failure replaces the loaded state (honest, not silently kept).
        s.apply(Update::Library(Err("server down".to_string())));
        assert!(matches!(&s.albums, Fetch::Cached(albums) if albums.len() == 2));
    }

    #[test]
    fn opening_an_album_marks_its_tracks_loading() {
        let mut s = MusicState::new();
        s.open(album("7"));
        let open = s.open_album.as_ref().expect("an album is open");
        assert_eq!(open.album.id, "7");
        assert_eq!(open.tracks, Fetch::Loading);
    }

    #[test]
    fn tracks_fill_only_the_matching_open_album() {
        let mut s = MusicState::new();
        s.open(album("7"));
        // A stale reply for a different album id is ignored.
        s.apply(Update::Tracks {
            album_id: "999".to_string(),
            result: Ok(vec![song("a", "flac", 1)]),
        });
        assert_eq!(s.open_album.as_ref().expect("open").tracks, Fetch::Loading);
        // The matching reply fills it.
        s.apply(Update::Tracks {
            album_id: "7".to_string(),
            result: Ok(vec![song("a", "flac", 10), song("b", "mp3", 20)]),
        });
        assert!(
            matches!(&s.open_album.as_ref().expect("open").tracks, Fetch::Ready(t) if t.len() == 2)
        );
    }

    #[test]
    fn closing_clears_the_open_album() {
        let mut s = MusicState::new();
        s.open(album("3"));
        assert!(s.open_album.is_some());
        s.close();
        assert!(s.open_album.is_none());
    }

    #[test]
    fn transport_updates_drive_now_playing_and_play_state() {
        let mut s = MusicState::new();
        s.apply(Update::Error("stale".to_string()));
        s.apply(Update::Started(song("42", "flac", 200)));
        assert_eq!(s.now_playing.as_ref().expect("playing").id, "42");
        assert!(s.playing);
        // Starting playback clears a prior error banner.
        assert!(s.error.is_none());
        // Pause / resume toggle only the play state, keeping the track.
        s.apply(Update::Playing(false));
        assert!(!s.playing);
        assert!(s.now_playing.is_some());
        s.apply(Update::Playing(true));
        assert!(s.playing);
        // Stop clears the track entirely.
        s.apply(Update::Stopped);
        assert!(s.now_playing.is_none());
        assert!(!s.playing);
    }

    #[test]
    fn error_update_sets_the_banner() {
        let mut s = MusicState::new();
        s.apply(Update::Error("no audio device".to_string()));
        assert_eq!(s.error.as_deref(), Some("no audio device"));
    }

    #[test]
    fn progress_updates_track_the_playhead_and_reset_on_start() {
        let mut s = MusicState::new();
        s.apply(Update::Started(song("1", "flac", 200)));
        assert_eq!(s.position_ms, 0);
        s.apply(Update::Progress(12_345));
        assert_eq!(s.position_ms, 12_345);
        // Starting a new track rewinds the playhead to zero.
        s.apply(Update::Started(song("2", "mp3", 60)));
        assert_eq!(s.position_ms, 0);
    }

    #[test]
    fn daemon_snapshot_projects_transport_volume_with_playback_state() {
        use mde_musicd::domain::{
            MusicStorageSnapshot, MusicWorkspaceSnapshotV1, PlaybackSnapshot,
        };

        let mut state = MusicState::new();
        state.apply_workspace_snapshot(MusicWorkspaceSnapshotV1 {
            schema_version: 1,
            revision: 8,
            shelves: Vec::new(),
            bookmarks: Vec::new(),
            collections: Vec::new(),
            search: None,
            playback: PlaybackSnapshot {
                current: None,
                playing: true,
                position_ms: 12_000,
                duration_ms: Some(60_000),
                volume_milli: 375,
                shuffle: false,
                repeat: "off".to_owned(),
                queue_revision: 2,
                seekable: true,
            },
            queue: Vec::new(),
            downloads: Vec::new(),
            storage: MusicStorageSnapshot {
                used_bytes: 0,
                cap_bytes: 1,
            },
            targets: Vec::new(),
            sources: Vec::new(),
            any_source_reachable: true,
        });

        assert!(state.playing);
        assert_eq!(state.position_ms, 12_000);
        assert_eq!(state.volume_milli, Some(375));
    }

    #[test]
    fn daemon_snapshot_materializes_now_playing_metadata_for_embedded_shell() {
        use mde_musicd::domain::{
            BookmarkItem, CatalogItem, ContentKind, ContentRef, LibraryCollection,
            MusicStorageSnapshot, MusicWorkspaceSnapshotV1, PlaybackSnapshot, SourceVariant,
        };

        let content = ContentRef::new("source-one", "song-1", ContentKind::Music).unwrap();
        let mut state = MusicState::new();
        state.apply_workspace_snapshot(MusicWorkspaceSnapshotV1 {
            schema_version: 1,
            revision: 9,
            shelves: Vec::new(),
            bookmarks: Vec::new(),
            collections: vec![LibraryCollection {
                key: "songs".to_owned(),
                title: "Songs".to_owned(),
                kind: ContentKind::Music,
                items: vec![CatalogItem {
                    id: "song-1".to_owned(),
                    kind: ContentKind::Music,
                    title: "A daemon song".to_owned(),
                    creator: "An artist".to_owned(),
                    parent_title: "An album".to_owned(),
                    duration_ms: Some(181_000),
                    artwork_ref: Some("art-1".to_owned()),
                    starred: false,
                    cached: true,
                    variants: vec![SourceVariant {
                        content: content.clone(),
                        cached: true,
                        reachable: true,
                        operator_priority: 1,
                        latency_ms: Some(4),
                    }],
                }],
                mutable: false,
                offset: 0,
                page_size: 0,
                has_more: false,
            }],
            search: None,
            playback: PlaybackSnapshot {
                current: Some(content),
                playing: true,
                position_ms: 12_000,
                duration_ms: Some(181_000),
                volume_milli: 850,
                shuffle: false,
                repeat: "off".to_owned(),
                queue_revision: 1,
                seekable: true,
            },
            queue: Vec::new(),
            downloads: Vec::new(),
            storage: MusicStorageSnapshot {
                used_bytes: 0,
                cap_bytes: 1,
            },
            targets: Vec::new(),
            sources: Vec::new(),
            any_source_reachable: true,
        });

        let now_playing = state.now_playing.as_ref().expect("daemon current track");
        assert_eq!(now_playing.title, "A daemon song");
        assert_eq!(now_playing.artist, "An artist");
        assert_eq!(now_playing.album, "An album");
        assert_eq!(now_playing.duration, 181);
        assert_eq!(now_playing.cover_art, "art-1");
        assert!(state.playing);

        let mut bookmark_snapshot = state.workspace.clone().expect("retained snapshot");
        let bookmark_content =
            ContentRef::new("source-one", "episode-1", ContentKind::Episode).unwrap();
        bookmark_snapshot.revision = 10;
        bookmark_snapshot.collections.clear();
        bookmark_snapshot.bookmarks = vec![BookmarkItem {
            content: bookmark_content.clone(),
            title: "A bookmarked episode".to_owned(),
            creator: "A host".to_owned(),
            parent_title: "A podcast".to_owned(),
            position_ms: 42_000,
            duration_ms: Some(300_000),
            artwork_ref: Some("episode-art".to_owned()),
        }];
        bookmark_snapshot.playback.current = Some(bookmark_content);
        bookmark_snapshot.playback.playing = false;
        bookmark_snapshot.playback.position_ms = 42_000;
        bookmark_snapshot.playback.duration_ms = Some(300_000);
        state.apply_workspace_snapshot(bookmark_snapshot);
        let now_playing = state.now_playing.as_ref().expect("bookmark current track");
        assert_eq!(now_playing.title, "A bookmarked episode");
        assert_eq!(now_playing.artist, "A host");
        assert_eq!(now_playing.album, "A podcast");
        assert_eq!(now_playing.duration, 300);
        assert_eq!(now_playing.cover_art, "episode-art");
    }

    #[test]
    fn daemon_snapshot_rejects_invalid_content_without_overwriting_projection() {
        use mde_musicd::domain::{
            MusicStorageSnapshot, MusicWorkspaceSnapshotV1, PlaybackSnapshot,
        };

        let mut state = MusicState::new();
        state.apply_workspace_snapshot(MusicWorkspaceSnapshotV1 {
            schema_version: 1,
            revision: 8,
            shelves: Vec::new(),
            bookmarks: Vec::new(),
            collections: Vec::new(),
            search: None,
            playback: PlaybackSnapshot {
                current: None,
                playing: true,
                position_ms: 12_000,
                duration_ms: Some(60_000),
                volume_milli: 375,
                shuffle: false,
                repeat: "off".to_owned(),
                queue_revision: 2,
                seekable: true,
            },
            queue: Vec::new(),
            downloads: Vec::new(),
            storage: MusicStorageSnapshot {
                used_bytes: 0,
                cap_bytes: 1,
            },
            targets: Vec::new(),
            sources: Vec::new(),
            any_source_reachable: true,
        });

        let mut invalid = state.workspace.clone().expect("valid daemon projection");
        invalid.revision = 9;
        invalid.storage.cap_bytes = 0;
        invalid.playback.playing = false;
        invalid.playback.position_ms = 0;
        state.apply_workspace_snapshot(invalid);

        assert_eq!(state.workspace_revision, 8);
        assert!(state.playing);
        assert_eq!(state.position_ms, 12_000);
        assert_eq!(state.volume_milli, Some(375));
        assert_eq!(
            state
                .workspace
                .as_ref()
                .expect("retained projection")
                .revision,
            8
        );
        assert_eq!(
            state.error.as_deref(),
            Some("Music daemon projection rejected: invalid_storage_cap")
        );
    }

    #[test]
    fn a_track_ending_on_its_own_clears_the_transport() {
        let mut s = MusicState::new();
        s.apply(Update::Started(song("9", "flac", 30)));
        s.apply(Update::Progress(29_000));
        // The worker's poll saw the engine drain and reports the natural end.
        s.apply(Update::Ended);
        assert!(s.now_playing.is_none(), "the finished track is cleared");
        assert!(!s.playing);
        assert_eq!(s.position_ms, 0);
    }

    #[test]
    fn track_for_engine_builds_an_authenticated_stream_url_and_codec() {
        // Deterministic salt → a stable, assertable URL.
        let client = Client::with_salt("http://airsonic.mesh:4040", "alice", "pw", "salt");
        let (url, codec) = track_for_engine(&client, &song("713", "flac", 100));
        assert!(
            url.contains("/rest/stream"),
            "uses the stream endpoint: {url}"
        );
        assert!(url.contains("id=713"), "carries the song id: {url}");
        assert!(url.contains("u=alice"), "carries the auth user: {url}");
        assert_eq!(codec, SourceCodec::Flac);
        // The suffix drives the codec hint.
        let (_, mp3) = track_for_engine(&client, &song("8", "mp3", 60));
        assert_eq!(mp3, SourceCodec::Mp3);
    }

    #[test]
    fn format_duration_is_minutes_and_padded_seconds() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(7), "0:07");
        assert_eq!(format_duration(67), "1:07");
        assert_eq!(format_duration(3725), "62:05");
    }

    #[test]
    fn album_subtitle_joins_present_parts_and_omits_missing() {
        assert_eq!(album_subtitle(&album("1")), "Artist · 10 tracks · 2021");
        // Singular track count.
        let mut single = album("2");
        single.song_count = 1;
        single.year = None;
        assert_eq!(album_subtitle(&single), "Artist · 1 track");
        // Nothing known → empty (the view then renders no subtitle line).
        let bare = Album {
            id: "3".to_string(),
            name: "Bare".to_string(),
            artist: String::new(),
            artist_id: String::new(),
            song_count: 0,
            cover_art: String::new(),
            year: None,
        };
        assert!(album_subtitle(&bare).is_empty());
    }

    #[test]
    fn server_default_prefers_operator_priority_then_latency() {
        let mut slow = SeatServer::new("slow", "https://slow.example");
        slow.operator_priority = 2;
        slow.latency_ms = Some(40);
        let mut fast = SeatServer::new("fast", "https://fast.example");
        fast.operator_priority = 2;
        fast.latency_ms = Some(10);
        let mut preferred = SeatServer::new("preferred", "https://preferred.example");
        preferred.operator_priority = 3;
        preferred.latency_ms = Some(200);
        assert_eq!(
            select_default_server(&[slow.clone(), fast.clone(), preferred]),
            Some(2)
        );
        assert_eq!(select_default_server(&[fast, slow]), Some(0));
    }

    #[test]
    fn failed_fetch_retains_cached_library_and_track_data() {
        let mut state = MusicState::new();
        state.apply(Update::Library(Ok(vec![album("cached")])));
        state.apply(Update::Library(Err("offline".to_string())));
        assert!(matches!(&state.albums, Fetch::Cached(albums) if albums[0].id == "cached"));
        state.open(album("cached"));
        state.apply(Update::Tracks {
            album_id: "cached".to_string(),
            result: Ok(vec![song("track", "flac", 20)]),
        });
        state.apply(Update::Tracks {
            album_id: "cached".to_string(),
            result: Err("offline".to_string()),
        });
        assert!(matches!(
            &state.open_album.as_ref().expect("album").tracks,
            Fetch::Cached(tracks) if tracks[0].id == "track"
        ));
        assert!(state.offline);
    }

    #[test]
    fn replayed_search_generation_cannot_replace_the_accepted_result() {
        let mut state = MusicState::new();
        let first = SearchResult3 {
            albums: vec![album("first")],
            artists: Vec::new(),
            songs: Vec::new(),
        };
        let replay = SearchResult3 {
            albums: vec![album("replay")],
            artists: Vec::new(),
            songs: Vec::new(),
        };

        state.apply(Update::Search {
            generation: 1,
            query: "first".to_owned(),
            result: Ok(first),
        });
        state.apply(Update::Search {
            generation: 1,
            query: "replay".to_owned(),
            result: Ok(replay),
        });

        assert_eq!(state.search_generation, 1);
        assert_eq!(state.search_query, "first");
        assert!(matches!(
            &state.search,
            Fetch::Ready(page) if page.albums[0].id == "first"
        ));
    }
}
