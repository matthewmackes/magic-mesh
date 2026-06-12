//! `mde-music` binary — AIR-10/11 shell.
//!
//! Renders the 7-card library hub + a breadcrumb the user navigates,
//! plus an Airsonic connection banner (from the shared creds). The live
//! grids behind each card + playback land with the `mde-musicd` data
//! path (AIR-10.b / AIR-2); this shell is the §0.12 runtime-reachable
//! entry point that makes the [`hub`]/[`nav`] models live.

use iced::widget::{
    button, column, container, image, row, scrollable, stack, text, text_input, Space,
};
use iced::{Element, Length, Size, Subscription, Task};

use mde_music::album::{self, AlbumView};
use mde_music::color;
use mde_music::hub::HubCard;
use mde_music::library::{self, LibraryItem};
use mde_music::nav::{NavState, Route};
use mde_music::nowplaying::{self, NowState};
use mde_music::prefs::{self, SortKey};
use mde_music::search::{self, SearchResults};
use mde_musicd::creds::{self, Creds};

fn main() -> iced::Result {
    iced::application(
        |_state: &State| String::from("MDE Music"),
        State::update,
        State::view,
    )
    .subscription(State::subscription)
    .theme(|_state: &State| mde_music_iced_theme())
    .window_size(Size::new(1100.0, 720.0))
    .run_with(|| (State::new(), Task::none()))
}

/// Convert an mde-theme Carbon token (`Rgba`, u8 channels) to this crate's
/// `iced::Color` at alpha `a`. mde-music's iced version skews from the one
/// `mde_theme::into_iced_color()` targets, so the conversion is by hand — the
/// single sanctioned spot for raw channel math, keeping every call site on a
/// token rather than a literal (§4).
fn carbon(rgba: mde_theme::Rgba, a: f32) -> iced::Color {
    iced::Color {
        r: f32::from(rgba.r) / 255.0,
        g: f32::from(rgba.g) / 255.0,
        b: f32::from(rgba.b) / 255.0,
        a,
    }
}

/// Build the media player's `iced::Theme` from the canonical
/// `mde_theme::Palette` (E5.3) — the Q2 indigo accent, Apple-charcoal
/// background, and the centralized semantic tokens, single-sourced so
/// the player's chrome matches the rest of the MDE dark desktop instead
/// of iced's default light theme. (The now-playing surface keeps its
/// album-art-driven accent on top of this base.)
#[must_use]
fn mde_music_iced_theme() -> iced::Theme {
    use mde_theme::Palette;
    // Opaque conversion of an mde_theme token — delegates to the module-level
    // `carbon` helper (the one place channel math lives).
    fn c(rgba: mde_theme::Rgba) -> iced::Color {
        carbon(rgba, 1.0)
    }
    let p = Palette::dark();
    // NB: this crate's iced `theme::Palette` predates the `warning`
    // field (a dep skew from the workbench's iced), so it carries the
    // 5 base roles only.
    let palette = iced::theme::Palette {
        background: c(p.background),
        text: c(p.text),
        primary: c(p.accent),
        success: c(p.success),
        danger: c(p.danger),
    };
    iced::Theme::custom("MDE Music".to_string(), palette)
}

/// The first-run "connect your Airsonic server" form, shown until valid
/// creds exist.
#[derive(Default)]
struct FirstRunForm {
    url: String,
    user: String,
    pass: String,
    error: Option<String>,
}

struct State {
    nav: NavState,
    /// `Some` until the operator connects a server (first run); `None`
    /// once creds exist and the library shell is shown.
    form: Option<FirstRunForm>,
    /// The Airsonic connection status line (set once connected).
    connection: String,
    /// The current category's items (fetched from the daemon over the Bus).
    items: Vec<LibraryItem>,
    /// True while a category fetch is in flight.
    loading: bool,
    /// Last fetch error (e.g. "daemon not responding"), shown in-pane.
    load_error: Option<String>,
    /// AIR-14 — the live search query, its debounce generation, and the
    /// last results. `search_open` gates the results sheet over the page.
    search_query: String,
    search_seq: u64,
    search_results: Option<SearchResults>,
    searching: bool,
    search_error: Option<String>,
    search_open: bool,
    /// AIR-12 — the currently-open album page (None until one is opened).
    album: Option<AlbumView>,
    album_loading: bool,
    album_error: Option<String>,
    /// AIR-15 — the now-playing footer's live snapshot + resolved title.
    now_state: NowState,
    now_title: String,
    now_artist: String,
    /// AIR-15.b.2 — the current track's decoded cover art (maxi header).
    now_art: Option<image::Handle>,
    /// AIR-15.b.2 — current track duration (ms) for the maxi scrub bar.
    now_duration_ms: u64,
    /// AIR-16 — the open album's dominant cover colour + contrast text
    /// (Indigo until the cover art resolves).
    album_color: (u8, u8, u8),
    album_text_color: (u8, u8, u8),
    /// AIR-12/AIR-16 — the open album's decoded cover art (None until it
    /// resolves; the source for both the rendered image + the tint colour).
    album_art: Option<image::Handle>,
    /// AIR-11.b — the persisted library-grid sort order.
    sort: SortKey,
    /// AIR-11.c — last-known window width (tracked via the WindowResized
    /// subscription); the library grid derives its column count from it.
    grid_width: f32,
    /// AIR-11.c.2 — per-route grid scroll offset (y) so navigating away from
    /// a category and back preserves the scroll position within a session.
    grid_scroll: std::collections::HashMap<String, f32>,
    /// AIR-11.c.3 — per-card cover-art cache (LibraryItem.id → decoded
    /// thumbnail handle), populated by the ItemsLoaded fan-out.
    art_cache: std::collections::HashMap<String, image::Handle>,
    /// AIR-15.b — maxi-player (full-window) open flag + its queue snapshot.
    maxi_open: bool,
    queue_songs: Vec<String>,
    queue_current: usize,
    /// Resolved queue song-id -> title (fan-out via get-song).
    queue_titles: std::collections::HashMap<String, String>,
    /// AIR-15.b.4 — maxi tab + the current track's lyrics lines.
    maxi_tab: MaxiTab,
    maxi_lyrics: Vec<String>,
    /// AIR-15.b.5 — the mesh peer roster (Peers tab).
    maxi_peers: Vec<nowplaying::PeerState>,
}

/// AIR-15.b.4 — which maxi-player tab is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaxiTab {
    Queue,
    Lyrics,
    Peers,
}

#[derive(Debug, Clone)]
enum Message {
    /// Open one of the seven hub categories.
    OpenCard(HubCard),
    /// Jump to a breadcrumb segment (0 = Library root).
    Ascend(usize),
    /// A category fetch resolved.
    ItemsLoaded(Vec<LibraryItem>),
    /// A category fetch failed (daemon down / no server).
    ItemsFailed(String),
    /// AIR-11.c — the window resized; updates the adaptive-grid column count.
    WindowResized(f32),
    /// AIR-11.c.2 — the library grid scrolled; record the offset per route.
    GridScrolled(f32),
    /// AIR-11.c.3 — a grid card's cover art fetched (id, decoded handle or None).
    ArtLoaded(String, Option<image::Handle>),
    /// AIR-15.b — toggle the maxi-player full-window surface.
    ToggleMaxi,
    /// AIR-15.b.4 — switch the maxi Queue/Lyrics tab.
    MaxiTabSelected(MaxiTab),
    /// AIR-15.b.4 — the current track's lyrics loaded.
    LyricsLoaded(Vec<String>),
    /// AIR-15.b.5 — the mesh peer roster loaded.
    PeersLoaded(Vec<nowplaying::PeerState>),
    /// AIR-15.b.5 — ask a peer to yield playback (AIR-8 handoff).
    TakeOver(String),
    /// AIR-15.b — the play queue snapshot loaded (song-ids, current index).
    QueueLoaded(Vec<String>, usize),
    /// AIR-15.b — a queue song-id resolved to a title.
    QueueTitle(String, String),
    /// First-run form field edits.
    UrlChanged(String),
    UserChanged(String),
    PassChanged(String),
    /// Validate + save the first-run creds, then show the library.
    Connect,
    /// AIR-14 — search field edited (restarts the debounce).
    SearchInput(String),
    /// The debounce timer for query generation `n` elapsed.
    SearchTick(u64),
    /// A search resolved / failed.
    SearchLoaded(SearchResults),
    SearchFailed(String),
    /// Focus the search field (Cmd-F) / dismiss the sheet (Esc).
    FocusSearch,
    DismissSearch,
    /// Open an album / artist result (navigates the breadcrumb).
    OpenAlbum(String, String),
    OpenArtist(String, String),
    /// Open a genre page (loads the genre's albums).
    OpenGenre(String),
    /// Open a podcast channel page (loads its episodes).
    OpenPodcast(String, String),
    /// Play a podcast episode by its streamId (clear queue + enqueue + play).
    PlayEpisode(String),
    /// AIR-4.b — play a whole playlist by id (fetch its songs → clear+enqueue+play).
    PlayPlaylist(String),
    /// Add a song result to the queue; the reply closes the sheet.
    EnqueueSong(String),
    SearchEnqueued(Result<(), String>),
    /// AIR-12 — album page: the fetch resolved/failed + the action buttons.
    AlbumLoaded(AlbumView),
    AlbumFailed(String),
    PlayAlbum,
    ShuffleAlbum,
    AddAlbumToQueue,
    PlayTrackNext(String),
    AddTrackToQueue(String),
    AlbumActionDone(Result<(), String>),
    /// AIR-12/AIR-16 — the album cover art resolved (decoded image +
    /// dominant + contrast colours).
    ArtReady(Option<image::Handle>, (u8, u8, u8), (u8, u8, u8)),
    /// AIR-11.b — flip the library-grid sort order (+ persist it).
    ToggleSort,
    /// AIR-15 — now-playing footer: poll the live snapshot + transport.
    PollState,
    StateLoaded(NowState),
    SongResolved(String, String, String),
    /// AIR-15.b.2 — the current track's coverArt token resolved.
    NowMetaResolved(Option<String>, u64),
    /// AIR-15.b.2 — the current track's cover art decoded.
    NowArtReady(Option<image::Handle>),
    /// AIR-15.b.3 — the maxi volume slider changed.
    SetVolume(f32),
    PlayPause,
    SkipNext,
    SkipPrev,
    /// A transport command (volume/play/skip) finished — outcome is
    /// irrelevant; the follow-up state fetch is the truth (sweep-3 I8
    /// dropped the never-read `Result` payload).
    TransportDone,
}

impl State {
    fn new() -> Self {
        let (form, connection) = match creds::load() {
            Ok(c) => (None, format!("Connected to {}", c.server_url)),
            Err(_) => (Some(FirstRunForm::default()), String::new()),
        };
        Self {
            nav: NavState::new(),
            form,
            connection,
            items: Vec::new(),
            loading: false,
            load_error: None,
            search_query: String::new(),
            search_seq: 0,
            search_results: None,
            searching: false,
            search_error: None,
            search_open: false,
            album: None,
            album_loading: false,
            album_error: None,
            now_state: NowState::default(),
            now_title: String::new(),
            now_artist: String::new(),
            now_art: None,
            now_duration_ms: 0,
            album_color: color::accent_rgb(),
            album_text_color: (255, 255, 255),
            album_art: None,
            sort: prefs::load().sort,
            grid_width: 1100.0,
            grid_scroll: prefs::load().scroll.into_iter().collect(),
            art_cache: std::collections::HashMap::new(),
            maxi_open: false,
            queue_songs: Vec::new(),
            queue_current: 0,
            queue_titles: std::collections::HashMap::new(),
            maxi_tab: MaxiTab::Queue,
            maxi_lyrics: Vec::new(),
            maxi_peers: Vec::new(),
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::OpenCard(card) => {
                self.nav.push(Route::Category(card));
                self.items.clear();
                self.load_error = None;
                // Fetch the category from the daemon over the Bus (AIR-10.b)
                // when it's backed by a verb; the rest are AIR-4.b endpoints.
                if let Some(verb) = library::verb_for(card) {
                    self.loading = true;
                    Task::perform(library::fetch(verb), |r| match r {
                        Ok(items) => Message::ItemsLoaded(items),
                        Err(e) => Message::ItemsFailed(e),
                    })
                } else {
                    Task::none()
                }
            }
            Message::ItemsLoaded(items) => {
                self.items = items;
                self.loading = false;
                // AIR-11.c.2 — restore this category's saved scroll offset.
                let y = self
                    .grid_scroll
                    .get(&self.nav.current().segment())
                    .copied()
                    .unwrap_or(0.0);
                let restore = iced::widget::scrollable::scroll_to(
                    grid_scroll_id(),
                    iced::widget::scrollable::AbsoluteOffset { x: 0.0, y },
                );
                // AIR-11.c.3 — fan out per-card cover-art fetches (the AIR-7
                // mesh cache makes re-fetches cheap; visible perf is §0.15 bench).
                let art = Task::batch(self.items.iter().filter_map(|it| {
                    it.art_id.clone().map(|aid| {
                        let id = it.id.clone();
                        Task::perform(color::fetch_cover_art(aid), move |r| {
                            Message::ArtLoaded(
                                id.clone(),
                                r.ok().map(|b| image::Handle::from_bytes(b)),
                            )
                        })
                    })
                }));
                Task::batch([restore, art])
            }
            Message::ItemsFailed(e) => {
                self.items.clear();
                self.loading = false;
                self.load_error = Some(e);
                Task::none()
            }
            Message::ArtLoaded(id, handle) => {
                if let Some(h) = handle {
                    self.art_cache.insert(id, h);
                }
                Task::none()
            }
            Message::ToggleMaxi => {
                self.maxi_open = !self.maxi_open;
                if self.maxi_open {
                    Task::perform(nowplaying::fetch_queue(), |r| match r {
                        Ok((songs, current)) => Message::QueueLoaded(songs, current),
                        Err(_) => Message::QueueLoaded(Vec::new(), 0),
                    })
                } else {
                    Task::none()
                }
            }
            Message::QueueLoaded(songs, current) => {
                self.queue_current = current;
                self.queue_songs = songs;
                let tasks: Vec<Task<Message>> = self
                    .queue_songs
                    .iter()
                    .filter(|id| !self.queue_titles.contains_key(*id))
                    .map(|id| {
                        let key = id.clone();
                        Task::perform(nowplaying::resolve_song(id.clone()), move |r| {
                            Message::QueueTitle(key.clone(), r.map(|(t, _)| t).unwrap_or_default())
                        })
                    })
                    .collect();
                Task::batch(tasks)
            }
            Message::QueueTitle(id, title) => {
                self.queue_titles.insert(id, title);
                Task::none()
            }
            Message::MaxiTabSelected(t) => {
                self.maxi_tab = t;
                match t {
                    MaxiTab::Lyrics
                        if self.maxi_lyrics.is_empty() && !self.now_state.song_id.is_empty() =>
                    {
                        Task::perform(
                            nowplaying::fetch_lyrics(self.now_state.song_id.clone()),
                            |r| Message::LyricsLoaded(r.unwrap_or_default()),
                        )
                    }
                    MaxiTab::Peers => Task::perform(nowplaying::fetch_peer_states(), |r| {
                        Message::PeersLoaded(r.unwrap_or_default())
                    }),
                    _ => Task::none(),
                }
            }
            Message::LyricsLoaded(lines) => {
                self.maxi_lyrics = lines;
                Task::none()
            }
            Message::PeersLoaded(peers) => {
                self.maxi_peers = peers;
                Task::none()
            }
            Message::TakeOver(peer) => Task::perform(nowplaying::take_over(peer), |_| {
                Message::MaxiTabSelected(MaxiTab::Peers)
            }),
            Message::Ascend(index) => {
                self.nav.ascend_to(index);
                Task::none()
            }
            Message::UrlChanged(s) => {
                if let Some(f) = &mut self.form {
                    f.url = s;
                }
                Task::none()
            }
            Message::UserChanged(s) => {
                if let Some(f) = &mut self.form {
                    f.user = s;
                }
                Task::none()
            }
            Message::PassChanged(s) => {
                if let Some(f) = &mut self.form {
                    f.pass = s;
                }
                Task::none()
            }
            Message::Connect => {
                if let Some(f) = &mut self.form {
                    if creds::is_valid(&f.url, &f.user) {
                        let c = Creds {
                            server_url: f.url.trim().to_string(),
                            username: f.user.trim().to_string(),
                            password: f.pass.clone(),
                        };
                        match creds::save(&c) {
                            Ok(()) => {
                                self.connection = format!("Connected to {}", c.server_url);
                                self.nav = NavState::new();
                                self.form = None;
                            }
                            Err(e) => f.error = Some(format!("Couldn't save: {e}")),
                        }
                    } else {
                        f.error =
                            Some("Enter an http(s):// server URL and a username.".to_string());
                    }
                }
                Task::none()
            }
            Message::SearchInput(q) => {
                self.search_query = q;
                self.search_seq += 1;
                self.search_error = None;
                if self.search_query.trim().is_empty() {
                    self.search_open = false;
                    self.search_results = None;
                    self.searching = false;
                    Task::none()
                } else {
                    self.search_open = true;
                    // Restart the debounce: only this generation's tick fires.
                    let seq = self.search_seq;
                    Task::perform(
                        async move {
                            tokio::time::sleep(search::DEBOUNCE).await;
                            seq
                        },
                        Message::SearchTick,
                    )
                }
            }
            Message::SearchTick(seq) => {
                // Stale timer (the user kept typing) → ignore.
                if seq != self.search_seq || self.search_query.trim().is_empty() {
                    return Task::none();
                }
                self.searching = true;
                let query = self.search_query.trim().to_string();
                Task::perform(search::fetch_search(query), |r| match r {
                    Ok(results) => Message::SearchLoaded(results),
                    Err(e) => Message::SearchFailed(e),
                })
            }
            Message::SearchLoaded(results) => {
                self.search_results = Some(results);
                self.searching = false;
                Task::none()
            }
            Message::SearchFailed(e) => {
                self.search_results = None;
                self.searching = false;
                self.search_error = Some(e);
                Task::none()
            }
            Message::FocusSearch => {
                self.search_open = true;
                text_input::focus(search_id())
            }
            Message::DismissSearch => {
                self.dismiss_search();
                Task::none()
            }
            Message::OpenAlbum(id, name) => {
                self.nav.push(Route::Album(id.clone(), name));
                self.dismiss_search();
                self.album = None;
                self.album_error = None;
                self.album_loading = true;
                self.album_color = color::accent_rgb();
                self.album_text_color = (255, 255, 255);
                self.album_art = None;
                Task::perform(album::fetch_album(id), |r| match r {
                    Ok(a) => Message::AlbumLoaded(a),
                    Err(e) => Message::AlbumFailed(e),
                })
            }
            Message::OpenArtist(id, name) => {
                self.nav.push(Route::Artist(id, name));
                self.dismiss_search();
                Task::none()
            }
            Message::OpenGenre(genre) => {
                self.nav.push(Route::Genre(genre.clone()));
                self.dismiss_search();
                self.items.clear();
                self.load_error = None;
                self.loading = true;
                Task::perform(library::fetch_albums_by_genre(genre), |r| match r {
                    Ok(items) => Message::ItemsLoaded(items),
                    Err(e) => Message::ItemsFailed(e),
                })
            }
            Message::OpenPodcast(id, name) => {
                self.nav.push(Route::Podcast(id.clone(), name));
                self.dismiss_search();
                self.items.clear();
                self.load_error = None;
                self.loading = true;
                Task::perform(library::fetch_podcast_episodes(id), |r| match r {
                    Ok(items) => Message::ItemsLoaded(items),
                    Err(e) => Message::ItemsFailed(e),
                })
            }
            Message::PlayEpisode(stream_id) => {
                Task::perform(album::play_ids(vec![stream_id]), Message::AlbumActionDone)
            }
            Message::PlayPlaylist(id) => {
                Task::perform(album::play_playlist(id), Message::AlbumActionDone)
            }
            Message::WindowResized(w) => {
                self.grid_width = w;
                Task::none()
            }
            Message::GridScrolled(y) => {
                let key = self.nav.current().segment();
                self.grid_scroll.insert(key, y);
                // AIR-11.c.4 — persist scroll offsets for cross-launch restore
                // (a tiny non-fsync write; negligible per frame).
                prefs::save(&prefs::MusicPrefs {
                    sort: self.sort,
                    scroll: self.grid_scroll.clone().into_iter().collect(),
                });
                Task::none()
            }
            Message::EnqueueSong(id) => Task::perform(search::enqueue(id), Message::SearchEnqueued),
            Message::SearchEnqueued(result) => {
                match result {
                    // Queued — closing the sheet is the confirmation.
                    Ok(()) => self.dismiss_search(),
                    Err(e) => self.search_error = Some(e),
                }
                Task::none()
            }
            Message::AlbumLoaded(a) => {
                let cover = a.cover_art.clone();
                self.album = Some(a);
                self.album_loading = false;
                if cover.is_empty() {
                    Task::none()
                } else {
                    Task::perform(color::fetch_cover_art(cover), |r| match r {
                        Ok(bytes) if !bytes.is_empty() => {
                            let handle = image::Handle::from_bytes(bytes.clone());
                            let (d, t) = color::extract(&bytes)
                                .unwrap_or((color::accent_rgb(), (255, 255, 255)));
                            Message::ArtReady(Some(handle), d, t)
                        }
                        _ => Message::ArtReady(None, color::accent_rgb(), (255, 255, 255)),
                    })
                }
            }
            Message::ArtReady(handle, dominant, text) => {
                self.album_art = handle;
                self.album_color = dominant;
                self.album_text_color = text;
                Task::none()
            }
            Message::ToggleSort => {
                self.sort = self.sort.toggled();
                prefs::save(&prefs::MusicPrefs {
                    sort: self.sort,
                    scroll: self.grid_scroll.clone().into_iter().collect(),
                });
                Task::none()
            }
            Message::AlbumFailed(e) => {
                self.album = None;
                self.album_loading = false;
                self.album_error = Some(e);
                Task::none()
            }
            Message::PlayAlbum => match &self.album {
                Some(a) => Task::perform(album::play_ids(a.track_ids()), Message::AlbumActionDone),
                None => Task::none(),
            },
            Message::ShuffleAlbum => match &self.album {
                Some(a) => Task::perform(
                    album::play_ids(album::shuffle_ids(a.track_ids())),
                    Message::AlbumActionDone,
                ),
                None => Task::none(),
            },
            Message::AddAlbumToQueue => match &self.album {
                Some(a) => {
                    Task::perform(album::enqueue_ids(a.track_ids()), Message::AlbumActionDone)
                }
                None => Task::none(),
            },
            Message::PlayTrackNext(id) => {
                Task::perform(album::play_next(id), Message::AlbumActionDone)
            }
            Message::AddTrackToQueue(id) => {
                Task::perform(album::enqueue_ids(vec![id]), Message::AlbumActionDone)
            }
            Message::AlbumActionDone(result) => {
                if let Err(e) = result {
                    self.album_error = Some(e);
                }
                Task::none()
            }
            Message::PollState => Task::perform(nowplaying::fetch_state(), |r| {
                Message::StateLoaded(r.unwrap_or_default())
            }),
            Message::StateLoaded(s) => {
                let changed = s.song_id != self.now_state.song_id;
                self.now_state = s;
                if changed {
                    self.now_title.clear();
                    self.now_artist.clear();
                    self.now_art = None;
                    self.maxi_lyrics.clear();
                    let id = self.now_state.song_id.clone();
                    if !id.is_empty() {
                        let resolve = {
                            let id = id.clone();
                            Task::perform(nowplaying::resolve_song(id.clone()), move |r| {
                                let (t, a) = r.unwrap_or_else(|_| (id.clone(), String::new()));
                                Message::SongResolved(id.clone(), t, a)
                            })
                        };
                        let meta = Task::perform(nowplaying::resolve_now_meta(id), |r| {
                            let (cover, duration) = r.unwrap_or((None, 0));
                            Message::NowMetaResolved(cover, duration)
                        });
                        return Task::batch([resolve, meta]);
                    }
                }
                Task::none()
            }
            Message::SongResolved(id, title, artist) => {
                if id == self.now_state.song_id {
                    self.now_title = title;
                    self.now_artist = artist;
                }
                Task::none()
            }
            Message::NowMetaResolved(cover, duration) => {
                self.now_duration_ms = duration;
                match cover {
                    Some(c) => Task::perform(color::fetch_cover_art(c), |r| {
                        Message::NowArtReady(r.ok().map(|b| image::Handle::from_bytes(b)))
                    }),
                    None => Task::none(),
                }
            }
            Message::NowArtReady(handle) => {
                self.now_art = handle;
                Task::none()
            }
            Message::SetVolume(v) => {
                self.now_state.volume = v;
                Task::perform(nowplaying::set_volume(v), |_| Message::TransportDone)
            }
            Message::PlayPause => {
                Task::perform(nowplaying::play_pause(self.now_state.playing), |_| {
                    Message::TransportDone
                })
            }
            Message::SkipNext => Task::perform(nowplaying::skip_next(), |_| Message::TransportDone),
            Message::SkipPrev => Task::perform(nowplaying::skip_prev(), |_| Message::TransportDone),
            Message::TransportDone => Task::perform(nowplaying::fetch_state(), |r| {
                Message::StateLoaded(r.unwrap_or_default())
            }),
        }
    }

    /// Close the search sheet + clear its state (shared by Esc, navigating
    /// to a result, and a successful enqueue).
    fn dismiss_search(&mut self) {
        self.search_open = false;
        self.search_query.clear();
        self.search_results = None;
        self.search_error = None;
    }

    /// Keyboard shortcuts: Cmd/Ctrl-F focuses search, Esc dismisses it.
    fn subscription(&self) -> Subscription<Message> {
        let keys = iced::keyboard::on_key_press(|key, modifiers| {
            use iced::keyboard::key::Named;
            use iced::keyboard::Key;
            match key {
                Key::Character(c) if c.as_str() == "f" && modifiers.command() => {
                    Some(Message::FocusSearch)
                }
                Key::Named(Named::Escape) => Some(Message::DismissSearch),
                _ => None,
            }
        });
        // AIR-11.c — track window width so the library grid can reflow
        // its columns (iced 0.13's facade has no `responsive`; the resize
        // event drives the adaptive layout instead).
        let resizes = iced::event::listen_with(|event, _status, _id| match event {
            iced::Event::Window(iced::window::Event::Resized(size)) => {
                Some(Message::WindowResized(size.width))
            }
            _ => None,
        });
        // Poll the now-playing snapshot once the library is shown (there's
        // no daemon to ask on the first-run connect form).
        if self.form.is_some() {
            Subscription::batch([keys, resizes])
        } else {
            Subscription::batch([
                keys,
                resizes,
                iced::time::every(nowplaying::POLL).map(|_| Message::PollState),
            ])
        }
    }

    fn view(&self) -> Element<'_, Message> {
        if let Some(f) = &self.form {
            return self.first_run_view(f);
        }
        if self.maxi_open {
            return self.maxi_view();
        }
        self.library_view()
    }

    /// The first-run connect form.
    fn first_run_view(&self, f: &FirstRunForm) -> Element<'_, Message> {
        let mut col = column![
            text("Connect your music").size(22),
            Space::with_height(Length::Fixed(8.0)),
            text("Point MDE Music at your Airsonic / Navidrome server.").size(13),
            Space::with_height(Length::Fixed(16.0)),
            text_input("https://music.your-mesh:4040", &f.url).on_input(Message::UrlChanged),
            text_input("username", &f.user).on_input(Message::UserChanged),
            text_input("password", &f.pass)
                .secure(true)
                .on_input(Message::PassChanged),
            Space::with_height(Length::Fixed(12.0)),
            button(text("Connect")).on_press(Message::Connect),
        ]
        .spacing(8)
        .padding(28)
        .max_width(440);
        if let Some(err) = &f.error {
            col = col.push(Space::with_height(Length::Fixed(8.0)));
            col = col.push(text(err.clone()).size(13));
        }
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// The library shell (hub + breadcrumb).
    fn library_view(&self) -> Element<'_, Message> {
        // Breadcrumb — each segment is a button that ascends to it.
        let mut crumbs = row![].spacing(6);
        let segments = self.nav.breadcrumb();
        let last = segments.len().saturating_sub(1);
        for (i, seg) in segments.iter().enumerate() {
            if i > 0 {
                crumbs = crumbs.push(text("›"));
            }
            // The ellipsis isn't navigable; the current (last) segment is
            // shown as plain text.
            if seg == "…" || i == last {
                crumbs = crumbs.push(text(seg.clone()));
            } else {
                crumbs = crumbs.push(button(text(seg.clone())).on_press(Message::Ascend(i)));
            }
        }

        // Body — the hub renders its seven cards; a category page renders
        // an honest empty state until the daemon data path lands.
        let body: Element<'_, Message> = match self.nav.current() {
            Route::Hub => {
                let mut cards = column![].spacing(8);
                for card in HubCard::all() {
                    cards =
                        cards.push(button(text(card.label())).on_press(Message::OpenCard(card)));
                }
                cards.into()
            }
            Route::Album(..) => self.album_page(),
            route => {
                // AIR-11.b — title + a sort toggle; items lay out in a
                // wrapping 160×160 card grid, ordered by the persisted sort.
                let title_row = row![
                    text(route.segment()).size(20),
                    Space::with_width(Length::Fill),
                    button(text(format!("Sort: {}", self.sort.label())).size(12))
                        .on_press(Message::ToggleSort),
                ]
                .spacing(8);
                let mut col = column![title_row].spacing(10);
                if self.loading {
                    col = col.push(text("Loading…").size(13));
                } else if let Some(err) = &self.load_error {
                    col = col.push(text(err.clone()).size(13));
                } else if self.items.is_empty() {
                    col = col.push(
                        text("Nothing here yet — start mde-musicd to load your library.").size(13),
                    );
                } else {
                    let mut items = self.items.clone();
                    prefs::apply_sort(&mut items, self.sort);
                    // AIR-11.c — width-adaptive grid: the column count is
                    // derived from the live viewport width via iced
                    // `responsive`, so the 160px cards reflow as the window
                    // resizes (replacing the AIR-11.b fixed 5-column layout).
                    // Per-card cover art + scroll-position persistence remain
                    // the AIR-11.c.2 follow-on.
                    // AIR-11.c — width-adaptive columns: the count is derived
                    // from the live window width (tracked via the WindowResized
                    // subscription) so the 160px cards reflow on resize, replacing
                    // the AIR-11.b fixed 5-column layout. Per-card cover art +
                    // scroll-position persistence remain the AIR-11.c.2 follow-on.
                    let cols = ((self.grid_width + 8.0) / 168.0).floor().max(1.0) as usize;
                    let mut grid = column![].spacing(8);
                    for chunk in items.chunks(cols) {
                        let mut r = row![].spacing(8);
                        for item in chunk {
                            let card_content: Element<'_, Message> =
                                if let Some(handle) = self.art_cache.get(&item.id) {
                                    column![
                                        image(handle.clone())
                                            .width(Length::Fill)
                                            .height(Length::Fixed(120.0)),
                                        text(item.label.clone()).size(12),
                                    ]
                                    .spacing(4)
                                    .into()
                                } else {
                                    text(item.label.clone()).into()
                                };
                            let mut btn = button(card_content)
                                .width(Length::Fixed(160.0))
                                .height(Length::Fixed(160.0));
                            btn = match route {
                                Route::Category(HubCard::Albums)
                                | Route::Genre(_)
                                | Route::Category(HubCard::Recents) => btn.on_press(
                                    Message::OpenAlbum(item.id.clone(), item.label.clone()),
                                ),
                                Route::Category(HubCard::Artists) => btn.on_press(
                                    Message::OpenArtist(item.id.clone(), item.label.clone()),
                                ),
                                Route::Category(HubCard::Genres) => {
                                    btn.on_press(Message::OpenGenre(item.label.clone()))
                                }
                                Route::Category(HubCard::Podcasts) => btn.on_press(
                                    Message::OpenPodcast(item.id.clone(), item.label.clone()),
                                ),
                                Route::Category(HubCard::Playlists) => {
                                    btn.on_press(Message::PlayPlaylist(item.id.clone()))
                                }
                                Route::Podcast(..) => {
                                    btn.on_press(Message::PlayEpisode(item.id.clone()))
                                }
                                // SVC-3 — a radio station's id IS its stream
                                // URL; clicking plays it directly.
                                Route::Category(HubCard::Radio) => {
                                    btn.on_press(Message::PlayEpisode(item.id.clone()))
                                }
                                _ => btn,
                            };
                            r = r.push(btn);
                        }
                        grid = grid.push(r);
                    }
                    col = col.push(
                        scrollable(grid)
                            .id(grid_scroll_id())
                            .on_scroll(|vp| Message::GridScrolled(vp.absolute_offset().y)),
                    );
                }
                col.into()
            }
        };

        let search_field = text_input("Search artists, albums, songs…", &self.search_query)
            .id(search_id())
            .on_input(Message::SearchInput)
            .padding(8)
            .width(Length::Fixed(340.0));
        let header = row![
            text(&self.connection).size(13),
            Space::with_width(Length::Fill),
            search_field,
        ]
        .spacing(12);

        let mut page_col = column![
            header,
            Space::with_height(Length::Fixed(12.0)),
            crumbs,
            Space::with_height(Length::Fixed(16.0)),
            body,
        ]
        .padding(20)
        .width(Length::Fill)
        .height(Length::Fill);
        if let Some(footer) = self.now_playing_footer() {
            page_col = page_col.push(footer);
        }
        let page = container(page_col).width(Length::Fill).height(Length::Fill);

        // AIR-14 — overlay the results sheet while a search is active.
        if self.search_open {
            stack![page, self.search_sheet()].into()
        } else {
            page.into()
        }
    }

    /// The AIR-14 results sheet: Artists / Albums / Songs sections over the
    /// page. Artist + album rows navigate the breadcrumb; song rows enqueue.
    fn search_sheet(&self) -> Element<'_, Message> {
        let mut col = column![text("Search").size(18)]
            .spacing(10)
            .padding(20)
            .max_width(720);
        if self.searching {
            col = col.push(text("Searching…").size(13));
        } else if let Some(err) = &self.search_error {
            col = col.push(text(err.clone()).size(13));
        } else if let Some(results) = &self.search_results {
            if results.is_empty() {
                col = col.push(text("No results.").size(13));
            } else {
                col = col.push(result_section("Artists", &results.artists, |it| {
                    Message::OpenArtist(it.id.clone(), it.label.clone())
                }));
                col = col.push(result_section("Albums", &results.albums, |it| {
                    Message::OpenAlbum(it.id.clone(), it.label.clone())
                }));
                col = col.push(result_section("Songs", &results.songs, |it| {
                    Message::EnqueueSong(it.id.clone())
                }));
            }
        }
        col = col.push(Space::with_height(Length::Fixed(8.0)));
        col = col.push(button(text("Close")).on_press(Message::DismissSearch));
        container(scrollable(col))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(40)
            .into()
    }

    /// AIR-12 — the album detail page: an art-placeholder column + the
    /// album header (Play / Shuffle / Add) + the numbered track list (each
    /// row can Play-Next or Add-to-Queue). Cover-art *image* rendering is a
    /// follow-on (art-over-Bus); the layout uses a glyph placeholder.
    fn album_page(&self) -> Element<'_, Message> {
        if self.album_loading {
            return text("Loading album…").size(13).into();
        }
        if let Some(err) = &self.album_error {
            return text(err.clone()).size(13).into();
        }
        let Some(a) = &self.album else {
            return text("No album loaded.").size(13).into();
        };

        // Header: title / artist / (year ·) N tracks · duration + actions.
        let mut meta = format!(
            "{} track(s) · {}",
            a.tracks.len(),
            album::fmt_duration(a.total_secs())
        );
        if let Some(y) = a.year {
            meta = format!("{y} · {meta}");
        }
        let actions = row![
            button(text("Play")).on_press(Message::PlayAlbum),
            button(text("Shuffle")).on_press(Message::ShuffleAlbum),
            button(text("Add to Queue")).on_press(Message::AddAlbumToQueue),
        ]
        .spacing(8);
        let header = column![
            text(a.name.clone()).size(24),
            text(a.artist.clone()).size(15),
            text(meta).size(12),
            Space::with_height(Length::Fixed(10.0)),
            actions,
        ]
        .spacing(4);

        // Numbered track rows with per-track Play-Next / Add-to-Queue.
        let mut list = column![].spacing(4);
        for (i, t) in a.tracks.iter().enumerate() {
            let no = t
                .track_no
                .unwrap_or_else(|| u32::try_from(i + 1).unwrap_or(0));
            let track_row = row![
                text(format!("{no}.")).size(13).width(Length::Fixed(32.0)),
                text(t.title.clone()).size(13).width(Length::Fill),
                text(album::fmt_duration(t.duration))
                    .size(12)
                    .width(Length::Fixed(56.0)),
                button(text("Play Next").size(11)).on_press(Message::PlayTrackNext(t.id.clone())),
                button(text("+ Queue").size(11)).on_press(Message::AddTrackToQueue(t.id.clone())),
            ]
            .spacing(8);
            list = list.push(track_row);
        }

        // Art placeholder (left) + header/tracks (right). The art-over-Bus
        // image fetch is a follow-on; a glyph stands in for now.
        let art: Element<'_, Message> = match &self.album_art {
            Some(handle) => image(handle.clone())
                .width(Length::Fixed(220.0))
                .height(Length::Fixed(220.0))
                .into(),
            None => container(text("♪").size(48))
                .width(Length::Fixed(220.0))
                .height(Length::Fixed(220.0))
                .padding(86)
                .into(),
        };
        // AIR-16 — tint the header band to the cover's dominant colour
        // (Indigo until it resolves) with a WCAG-contrast text colour.
        let (cr, cg, cb) = self.album_color;
        let (tr, tg, tb) = self.album_text_color;
        let header_band = container(header)
            .padding(16)
            .width(Length::Fill)
            .style(move |_| iced::widget::container::Style {
                background: Some(iced::Color::from_rgb8(cr, cg, cb).into()), // carbon-ok: dynamic album-art colour, not a UI token
                text_color: Some(iced::Color::from_rgb8(tr, tg, tb)), // carbon-ok: dynamic album-art colour
                ..Default::default()
            });
        let content = column![
            header_band,
            Space::with_height(Length::Fixed(16.0)),
            scrollable(list)
        ]
        .spacing(8)
        .width(Length::Fill);
        row![art, content].spacing(20).into()
    }

    /// AIR-15 — the always-visible now-playing + transport footer (shown
    /// once a track is loaded). The maxi-player's Queue / Lyrics / Peers
    /// tabs + scrub + volume slider are follow-ons; this is the in-app
    /// transport core (the first play/pause/skip after playback starts).
    fn now_playing_footer(&self) -> Option<Element<'_, Message>> {
        if !self.now_state.has_track() {
            return None;
        }
        let title = if self.now_title.is_empty() {
            self.now_state.song_id.clone()
        } else {
            self.now_title.clone()
        };
        let label = if self.now_artist.is_empty() {
            title
        } else {
            format!("{title} — {}", self.now_artist)
        };
        let play_pause = if self.now_state.playing {
            "Pause"
        } else {
            "Play"
        };
        let status = if self.now_state.playing {
            "Playing"
        } else if self.now_state.active {
            "Paused"
        } else {
            "Stopped"
        };
        Some(
            row![
                text(label).size(13).width(Length::Fill),
                button(text("Prev").size(12)).on_press(Message::SkipPrev),
                button(text(play_pause).size(12)).on_press(Message::PlayPause),
                button(text("Next").size(12)).on_press(Message::SkipNext),
                button(text("Maximize").size(12)).on_press(Message::ToggleMaxi),
                text(status).size(12),
            ]
            .spacing(10)
            .padding(10)
            .into(),
        )
    }

    /// AIR-15.b — the maxi-player full-window surface: now-playing header
    /// (title/artist + transport) + the Queue tab (the play queue with the
    /// current track marked). Large art + scrub-progress + volume slider +
    /// Lyrics/Peers tabs are the AIR-15.b.2 follow-on.
    fn maxi_view(&self) -> Element<'_, Message> {
        // §4: muted/accent come from the Carbon palette, not raw literals.
        let p = mde_theme::Palette::dark();
        let muted = carbon(p.text_muted, 1.0);
        let accent = carbon(p.accent, 1.0);
        let title = if self.now_title.is_empty() {
            self.now_state.song_id.clone()
        } else {
            self.now_title.clone()
        };
        let play_pause = if self.now_state.playing {
            "Pause"
        } else {
            "Play"
        };
        let art: Element<'_, Message> = match &self.now_art {
            Some(h) => image(h.clone())
                .width(Length::Fixed(240.0))
                .height(Length::Fixed(240.0))
                .into(),
            None => Space::with_height(Length::Fixed(0.0)).into(),
        };
        let ratio = (self.now_state.position_ms as f32 / self.now_duration_ms.max(1) as f32)
            .clamp(0.0, 1.0);
        let scrub: Element<'_, Message> = column![
            iced::widget::progress_bar(0.0..=1.0, ratio).height(Length::Fixed(6.0)),
            text(format!(
                "{}:{:02} / {}:{:02}",
                self.now_state.position_ms / 60000,
                (self.now_state.position_ms / 1000) % 60,
                self.now_duration_ms / 60000,
                (self.now_duration_ms / 1000) % 60,
            ))
            .size(11),
        ]
        .spacing(2)
        .into();
        let volume_slider: Element<'_, Message> =
            iced::widget::slider(0.0..=1.0, self.now_state.volume, Message::SetVolume)
                .step(0.01_f32)
                .width(Length::Fixed(200.0))
                .into();
        let header = column![
            row![
                text("Now Playing").size(22).width(Length::Fill),
                button(text("Close").size(13)).on_press(Message::ToggleMaxi),
            ]
            .align_y(iced::Alignment::Center),
            art,
            text(title).size(28),
            text(self.now_artist.clone()).size(16),
            scrub,
            volume_slider,
            row![
                button(text("Prev").size(13)).on_press(Message::SkipPrev),
                button(text(play_pause).size(13)).on_press(Message::PlayPause),
                button(text("Next").size(13)).on_press(Message::SkipNext),
            ]
            .spacing(10),
        ]
        .spacing(8);

        let mut queue =
            column![text(format!("Queue ({} tracks)", self.queue_songs.len())).size(15)].spacing(4);
        for (i, sid) in self.queue_songs.iter().enumerate() {
            let label = self
                .queue_titles
                .get(sid)
                .filter(|t| !t.is_empty())
                .cloned()
                .unwrap_or_else(|| sid.clone());
            let marker = if i == self.queue_current {
                "▶ "
            } else {
                "   "
            };
            queue = queue.push(text(format!("{marker}{label}")).size(13));
        }

        let lyrics: Element<'_, Message> = if self.maxi_lyrics.is_empty() {
            text("No lyrics for this track")
                .size(13)
                .color(muted)
                .into()
        } else {
            let mut col = column![].spacing(2);
            for line in &self.maxi_lyrics {
                col = col.push(text(line.clone()).size(13));
            }
            col.into()
        };
        let peers: Element<'_, Message> = if self.maxi_peers.is_empty() {
            text("No peers on the mesh").size(13).color(muted).into()
        } else {
            let mut col = column![].spacing(6);
            for p in &self.maxi_peers {
                let status = if p.playing { "● playing" } else { "paused" };
                col = col.push(
                    row![
                        text(p.host.clone()).size(14).width(Length::Fixed(150.0)),
                        text(status).size(12).color(muted),
                        button(text("Take over").size(12))
                            .on_press(Message::TakeOver(p.host.clone())),
                    ]
                    .spacing(12),
                );
            }
            col.into()
        };
        let tab = |label: &'static str, t: MaxiTab| {
            let color = if self.maxi_tab == t { accent } else { muted };
            button(text(label).size(13).color(color)).on_press(Message::MaxiTabSelected(t))
        };
        let tab_bar = row![
            tab("Queue", MaxiTab::Queue),
            tab("Lyrics", MaxiTab::Lyrics),
            tab("Peers", MaxiTab::Peers)
        ]
        .spacing(8);
        let body: Element<'_, Message> = match self.maxi_tab {
            MaxiTab::Queue => scrollable(queue).into(),
            MaxiTab::Lyrics => scrollable(lyrics).into(),
            MaxiTab::Peers => scrollable(peers).into(),
        };
        let page = column![header, tab_bar, body]
            .spacing(8)
            .padding(24)
            .width(Length::Fill)
            .height(Length::Fill);
        container(page)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

/// The stable widget id for the AIR-14 search field (so Cmd-F can focus it).
fn search_id() -> text_input::Id {
    text_input::Id::new("mde-music-search")
}

/// AIR-11.c.2 — stable id for the library card grid's scrollable, so the
/// scroll position can be saved on scroll + restored on category re-entry.
fn grid_scroll_id() -> iced::widget::scrollable::Id {
    iced::widget::scrollable::Id::new("mde-music-grid")
}

/// Render one search section: a heading + a clickable row per item. An
/// empty section renders nothing. `on_click` maps an item to its message.
fn result_section<'a>(
    title: &'a str,
    items: &'a [LibraryItem],
    on_click: impl Fn(&LibraryItem) -> Message,
) -> Element<'a, Message> {
    let mut col = column![].spacing(4);
    if items.is_empty() {
        return col.into();
    }
    col = col.push(text(title).size(14));
    for item in items {
        col = col.push(button(text(item.label.clone())).on_press(on_click(item)));
    }
    col = col.push(Space::with_height(Length::Fixed(10.0)));
    col.into()
}

#[cfg(test)]
mod theme_tests {
    use super::mde_music_iced_theme;

    #[test]
    fn iced_theme_builds_from_the_mde_palette() {
        // E5.3 — the player theme derives from mde_theme::Palette::dark()
        // (the MDE dark base), not iced's default light theme. Its
        // extended palette background must equal the palette's charcoal.
        let theme = mde_music_iced_theme();
        let bg = theme.extended_palette().background.base.color;
        let p = mde_theme::Palette::dark();
        assert!((bg.r - f32::from(p.background.r) / 255.0).abs() < 0.01);
        assert!((bg.g - f32::from(p.background.g) / 255.0).abs() < 0.01);
        assert!((bg.b - f32::from(p.background.b) / 255.0).abs() < 0.01);
    }
}
